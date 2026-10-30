//! Luna as an MCP server, for the sessions that asked for it: JSON-RPC over
//! streamable HTTP on the hub listener (`POST /mcp`, bearer token per
//! session). Eleven tools, described as briefly as a model can still use
//! them — every word here lands in the caller's context once per session —
//! a short `instructions` string in place of any system-prompt flag, and
//! the rest in `luna_guide`, read only by a model that asks for it. A
//! session without the tools is told nothing.
//!
//! Each request is answered with a plain JSON body (the transport allows it;
//! no SSE stream is ever opened), so `wait` simply holds its HTTP request
//! until the session it watches gets there — or the client cancels it.

use crate::agents;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const PROTOCOL: &str = "2025-06-18";
/// The versions a client may ask for that these answers fit; any other is
/// answered with PROTOCOL, for the client to take or leave.
const PROTOCOLS: [&str; 3] = [PROTOCOL, "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "Luna runs this session. Its luna_* tools start and drive other sessions: \
another model, another account, Codex or Claude Code, another folder or a worktree, in the background \
or as a chat the user sees; and they reach every other session in Luna. luna_guide explains them, \
accounts and models included: read it before the first spawn. Once a session you spawned or wrote \
to answers, asks something, stops on a prompt or exits, Luna types a notice into your session, \
opening \"[Luna notice\" (Luna's, not the user's words): end your turn to get it, no need to poll. \
Session ids are Luna's; your CLI's task and subagent ids are not, and a subagent of yours using these \
tools acts as you.";

/// What `luna_guide` returns: everything the instructions leave out.
const GUIDE: &str = include_str!("luna_guide.md");

/// The most of a past run's last message `luna_sessions` history passes on:
/// thirty runs at length overrun what a client takes in one result, and the
/// run's transcript has it whole.
const HISTORY_LAST: usize = 400;

const RUNNING: u8 = 0;
const COMMITTED: u8 = 1;
const CANCELLED: u8 = 2;

/// How long a call that has been answered stays where a cancel finds it: a
/// client that cancels as the response crosses its cancel drops it unread.
const LATE_CANCEL: Duration = Duration::from_secs(10);

/// One tool call's fate, decided once: committed to its response (a `wait`
/// or `read` has handed an owed answer over in it) or cancelled (no
/// response). A cancel that comes after the commit undoes the hand-over
/// instead, so the answer is owed again and the notice brings it.
pub struct Call {
    state: AtomicU8,
    undo: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl Call {
    fn new() -> Call {
        Call { state: AtomicU8::new(RUNNING), undo: Mutex::new(None) }
    }

    pub fn cancelled(&self) -> bool {
        self.state.load(Ordering::SeqCst) == CANCELLED
    }

    /// Commits the call to its response, once the hand-over `undo` reverses
    /// is done. False when a cancel came first: `undo` has run, and nothing
    /// is to be handed over.
    pub fn commit(&self, undo: impl FnOnce() + Send + 'static) -> bool {
        let undo: Box<dyn FnOnce() + Send> = Box::new(undo);
        *self.undo.lock().unwrap_or_else(|e| e.into_inner()) = Some(undo);
        if self.state.compare_exchange(RUNNING, COMMITTED, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
            return true;
        }
        let undo = self.undo.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(undo) = undo {
            undo();
        }
        false
    }

    fn cancel(&self) {
        if self.state.compare_exchange(RUNNING, CANCELLED, Ordering::SeqCst, Ordering::SeqCst).is_err()
            && self.state.load(Ordering::SeqCst) == COMMITTED
        {
            let undo = self.undo.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some(undo) = undo {
                undo();
            }
        }
    }
}

/// Tool calls by caller and JSON-RPC id, for a `notifications/cancelled` to
/// find: those in flight, and those answered within LATE_CANCEL (when).
fn in_flight() -> &'static Mutex<HashMap<(String, String), (Arc<Call>, Option<Instant>)>> {
    static M: OnceLock<Mutex<HashMap<(String, String), (Arc<Call>, Option<Instant>)>>> = OnceLock::new();
    M.get_or_init(Default::default)
}

/// A call counted in `in_flight` for as long as it runs.
struct InFlight {
    key: (String, String),
    call: Arc<Call>,
}

impl InFlight {
    fn start(caller: &str, id: &Value) -> InFlight {
        let key = (caller.to_string(), id.to_string());
        let call = Arc::new(Call::new());
        let mut m = in_flight().lock().unwrap_or_else(|e| e.into_inner());
        m.retain(|_, entry| entry.1.map_or(true, |at| at.elapsed() < LATE_CANCEL));
        m.insert(key.clone(), (call.clone(), None));
        InFlight { key, call }
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        let mut m = in_flight().lock().unwrap_or_else(|e| e.into_inner());
        // Only its own: a client reusing the id has a call of its own there.
        let committed = match m.get(&self.key) {
            Some((c, _)) if Arc::ptr_eq(c, &self.call) => c.state.load(Ordering::SeqCst) == COMMITTED,
            _ => return,
        };
        // One that handed nothing over has nothing a late cancel could undo.
        if committed {
            if let Some(entry) = m.get_mut(&self.key) {
                entry.1 = Some(Instant::now());
            }
        } else {
            m.remove(&self.key);
        }
    }
}

/// One JSON-RPC message in, at most one out: None for a notification, a
/// response from the client, or a request it cancelled.
pub fn handle(caller: &str, body: &str) -> Option<Value> {
    let req: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return Some(error(Value::Null, -32700, &format!("parse error: {e}"))),
    };
    // Batches left the protocol in 2025-06-18.
    if !req.is_object() {
        return Some(error(Value::Null, -32600, "invalid request: one JSON-RPC message per POST"));
    }
    let method = req["method"].as_str().unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(json!({}));
    // A `luna_wait` held open is the call this matters for: let go, it must
    // not take the answer it was waiting for with it.
    if method == "notifications/cancelled" {
        let key = (caller.to_string(), params["requestId"].to_string());
        let call = in_flight().lock().unwrap_or_else(|e| e.into_inner()).get(&key).map(|(c, _)| c.clone());
        if let Some(c) = call {
            c.cancel();
        }
    }
    // Without an id it is a notification; without a method, a response to
    // nothing this server asks. Neither is answered.
    let Some(id) = req.get("id").cloned().filter(|_| !method.is_empty() && !method.starts_with("notifications/")) else {
        return None;
    };
    let result = match method {
        "initialize" => {
            let asked = params["protocolVersion"].as_str().filter(|v| PROTOCOLS.contains(v));
            let mut r = json!({
                "protocolVersion": asked.unwrap_or(PROTOCOL),
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "luna", "version": env!("CARGO_PKG_VERSION") },
            });
            // A helper given only the permission handler gets no word of
            // tools it does not have.
            if agents::has_tools(caller) {
                r["instructions"] = json!(INSTRUCTIONS);
            }
            Ok(r)
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools_for(caller) })),
        "tools/call" => {
            let call_out = InFlight::start(caller, &id);
            let r = call(caller, &params, &call_out.call);
            // A cancelled request gets no response.
            if call_out.call.cancelled() {
                return None;
            }
            Ok(r)
        }
        _ => Err((-32601, format!("method not found: {method}"))),
    };
    Some(match result {
        Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
        Err((code, msg)) => error(id, code, &msg),
    })
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn tools() -> Value {
    let s = |props: Value, required: &[&str]| json!({ "type": "object", "properties": props, "required": required });
    // What a client may assume of each tool without asking the user. All of
    // them stay inside this Luna.
    let reads = json!({ "readOnlyHint": true, "openWorldHint": false });
    let acts = json!({ "readOnlyHint": false, "destructiveHint": false, "openWorldHint": false });
    let ends = json!({ "readOnlyHint": false, "destructiveHint": true, "openWorldHint": false });
    json!([
        {
            "name": "luna_list",
            "description": "Your id (me), the sessions you spawned (state, ask, turnsEnded, worktree, deletesInMin) and the accounts you may use: models, a helper's defaults in your folder, usage (% per window, resets).",
            "inputSchema": s(json!({}), &[]),
            "annotations": reads
        },
        {
            "name": "luna_sessions",
            "description": "Every session in Luna, the user's own chats included: group (the user's window group, I=1), folders, transcript path, parent, you/yours, state. Filters: group (a number or \"mine\"), q (text in name/folder/account/id). history: true = past spawns instead, deleted too (with transcript, last message), newest first (q also searches prompts).",
            "inputSchema": s(json!({ "group": { "type": "string" }, "q": { "type": "string" }, "history": { "type": "boolean" } }), &[]),
            "annotations": reads
        },
        {
            "name": "luna_spawn",
            "description": "Start a helper with an opening prompt, in the background (no row in Luna) unless visible: true. Omitted settings take the account's defaults. Returns id, resolved settings, started (false = not seen on the prompt in 30 s, see screen; a visible Codex may still start: luna_wait first), starting (not drawn yet; prompt held, do not resend), warnings, sameFolder, worktree.",
            "inputSchema": s(json!({
                "prompt": { "type": "string" },
                "provider": { "type": "string", "enum": ["claude", "codex"], "description": "default: yours" },
                "account": { "type": "string", "description": "account name from luna_list; default: yours" },
                "model": { "type": "string", "description": "one luna_list names for the account; claude also a full id" },
                "effort": { "type": "string", "description": "low|medium|high|xhigh|max; claude also ultracode; codex per model" },
                "permissionMode": { "type": "string", "description": "claude: default|acceptEdits|plan|bypassPermissions" },
                "approval": { "type": "string", "description": "codex: never|auto-review|on-request (visible only)" },
                "sandbox": { "type": "string", "description": "codex: read-only|workspace-write|danger-full-access" },
                "folder": { "type": "string", "description": "project folder; default: yours" },
                "worktree": { "type": "boolean", "description": "run in its own git worktree" },
                "name": { "type": "string", "description": "its name in Luna" },
                "tools": { "type": "boolean", "description": "give it these tools too (one level only)" },
                "visible": { "type": "boolean", "description": "a chat the user sees and can type in" },
                "autoDelete": { "type": "boolean", "description": "deleted 30 min after its answer reached you unless read or sent to; default: background true" }
            }), &["prompt"]),
            "annotations": acts
        },
        {
            "name": "luna_send",
            "description": "Send text to a session (any from luna_sessions) as a prompt; it reads as the user's, so say who you are to one you did not spawn. Returns stateBefore, queued (it was working: joins that turn or the next), held (a user's draft is in the way: typed in later), started (false if neither = not taken, see screen), turnsEnded for luna_wait afterTurn.",
            "inputSchema": s(json!({
                "id": { "type": "string" },
                "text": { "type": "string" },
                "interrupt": { "type": "boolean", "description": "stop its current turn first" }
            }), &["id", "text"]),
            "annotations": acts
        },
        {
            "name": "luna_answer",
            "description": "Answer what a session waits on (state question or waiting; ask says what): answers, one per question: an option's number or label, or your own words; allow: true/false for a permission, note: why not. keys: raw keystrokes for any other dialog, e.g. \"{down}{enter}\" (also {esc} {tab} {shift+tab} {up} {left} {right} {space} {backspace}).",
            "inputSchema": s(json!({
                "id": { "type": "string" },
                "answers": { "type": "array", "items": { "type": "string" } },
                "allow": { "type": "boolean" },
                "note": { "type": "string" },
                "keys": { "type": "string" }
            }), &["id"]),
            "annotations": acts
        },
        {
            "name": "luna_read",
            "description": "Messages of any session from its transcript. `since` = cursor from an earlier read; `last` = only the last N messages (default 1; all after `since`). Cut past 60k chars (leftOut): `since` alone goes on from cursor, `last` keeps the newest. Also returns state and ask.",
            "inputSchema": s(json!({
                "id": { "type": "string" },
                "since": { "type": "integer" },
                "last": { "type": "integer" },
                "tools": { "type": "boolean", "description": "include one-line tool call markers" },
                "screen": { "type": "boolean", "description": "include what the terminal shows (dialogs, errors, prompts)" }
            }), &["id"]),
            "annotations": reads
        },
        {
            "name": "luna_wait",
            "description": "Block until a Luna session (not yours; no task id, not a timer) ends a turn (reply: all it said since you asked), stops on a question or permission, or exits. Timeout, waiting and a reply-less turn come with the screen. `afterTurn`: wait for a turn past this turnsEnded (default: the last one you were shown). Optional: end your turn and a notice brings the answer.",
            "inputSchema": s(json!({
                "id": { "type": "string" },
                "until": { "type": "string", "enum": ["turn_done", "waiting", "exit"] },
                "timeoutS": { "type": "integer", "description": "default 600, max 1800" },
                "afterTurn": { "type": "integer" }
            }), &["id"]),
            "annotations": reads
        },
        {
            "name": "luna_kill",
            "description": "Stop a session's process (a background helper: its turn), not auto-deleted. A background helper goes on with luna_send, a visible one once the user resumes it.",
            "inputSchema": s(json!({ "id": { "type": "string" } }), &["id"]),
            "annotations": ends
        },
        {
            "name": "luna_delete",
            "description": "Stop a session and remove it; dropWorktree: also the worktree and branch Luna made for it, unless another session is in it.",
            "inputSchema": s(json!({ "id": { "type": "string" }, "dropWorktree": { "type": "boolean" } }), &["id"]),
            "annotations": ends
        },
        {
            "name": "luna_guide",
            "description": "How Luna's sessions, accounts, models and these tools work.",
            "inputSchema": s(json!({}), &[]),
            "annotations": reads
        },
        {
            "name": "luna_notify",
            "description": "Desktop notification to the user, titled with your chat. For a result or blocker they must see now, not progress; a turn ending on a question already notifies. One per 10 s.",
            "inputSchema": s(json!({ "text": { "type": "string" } }), &["text"]),
            "annotations": acts
        }
    ])
}

/// What a background Claude Code helper's CLI calls before a tool runs and
/// with every `AskUserQuestion` (`--permission-prompt-tool`): never the
/// model's to call, and offered only to such a helper.
fn permission_tool() -> Value {
    json!({
        "name": "luna_permission",
        "description": "Luna's permission handler for this session; never call it yourself.",
        "inputSchema": {
            "type": "object",
            "properties": { "tool_name": { "type": "string" }, "input": { "type": "object" }, "tool_use_id": { "type": "string" } },
            "required": ["tool_name", "input"]
        },
        "annotations": { "readOnlyHint": false, "destructiveHint": false, "openWorldHint": false }
    })
}

/// The tools a caller is offered: Luna's own when it was given them, and
/// the permission handler when it is a background helper.
fn tools_for(caller: &str) -> Value {
    let mut list: Vec<Value> = if crate::agents::has_tools(caller) { tools().as_array().cloned().unwrap_or_default() } else { Vec::new() };
    if crate::background::has(caller) {
        list.push(permission_tool());
    }
    Value::Array(list)
}

/// One line per call in luna.log: who called what on whom, how it ended and
/// how long it took. The arguments that steer a call are named; what is said
/// through it (prompts, sent text, answers, notifications) is not.
fn call(caller: &str, params: &Value, this: &Call) -> Value {
    let started = std::time::Instant::now();
    let out = run(caller, params, this);
    let name = params["name"].as_str().unwrap_or("?");
    let a = &params["arguments"];
    let mut line = format!("{} {name}", agents::label(caller));
    if let Some(id) = a["id"].as_str() {
        line.push_str(&format!(" on {}", agents::label(id)));
    }
    const STEERING: &[&str] = &[
        "provider", "account", "model", "effort", "permissionMode", "sandbox", "approval", "folder", "worktree", "visible",
        "tools", "autoDelete", "name", "until", "timeoutS", "afterTurn", "since", "last", "screen", "interrupt", "dropWorktree",
        "group", "history", "allow",
    ];
    let set: Vec<String> = STEERING.iter().filter_map(|k| a.get(*k).filter(|v| !v.is_null()).map(|v| format!("{k}={v}"))).collect();
    if !set.is_empty() {
        line.push_str(&format!(" ({})", set.join(" ")));
    }
    let ms = started.elapsed().as_millis();
    if this.cancelled() {
        crate::log::info("mcp", &format!("{line} cancelled by the client after {ms}ms"));
    } else if out["isError"] == true {
        let e = out["content"][0]["text"].as_str().unwrap_or("");
        let e: String = e.chars().take(300).collect();
        crate::log::warn("mcp", &format!("{line} failed in {ms}ms: {e}"));
    } else {
        // The first block is the result's JSON; its few fields that say how
        // the call went.
        let v: Value = out["content"][0]["text"].as_str().and_then(|t| serde_json::from_str(t).ok()).unwrap_or(Value::Null);
        const OUTCOME: &[&str] = &["id", "outcome", "started", "starting", "queued", "turnsEnded", "state", "leftOut", "worktreeRemoved"];
        let got: Vec<String> = OUTCOME.iter().filter_map(|k| v.get(*k).filter(|v| !v.is_null()).map(|v| format!("{k}={v}"))).collect();
        crate::log::info("mcp", &format!("{line} in {ms}ms: {}", if got.is_empty() { "ok".to_string() } else { got.join(" ") }));
    }
    out
}

fn run(caller: &str, params: &Value, this: &Call) -> Value {
    let name = params["name"].as_str().unwrap_or("");
    let a = &params["arguments"];
    let id = a["id"].as_str().unwrap_or("").to_string();
    // A background helper without tools holds a token for its permission
    // handler only.
    if name == "luna_permission" {
        if !crate::background::has(caller) {
            return json!({ "content": [{ "type": "text", "text": "not a background helper" }], "isError": true });
        }
        return json!({ "content": content(crate::background::permission(caller, a)) });
    }
    if !agents::has_tools(caller) {
        return json!({ "content": [{ "type": "text", "text": format!("{name} is not one of this session's tools") }], "isError": true });
    }
    let strings = |v: &Value| -> Vec<String> {
        v.as_array().map(|a| a.iter().map(|x| x.as_str().map(str::to_owned).unwrap_or_else(|| x.to_string())).collect()).unwrap_or_default()
    };
    let out: Result<Value, String> = match name {
        "luna_list" => Ok(json!(agents::list(caller))),
        "luna_guide" => return json!({ "content": [{ "type": "text", "text": GUIDE }] }),
        "luna_answer" => agents::answer(
            caller,
            &id,
            &strings(&a["answers"]),
            a["allow"].as_bool(),
            a["note"].as_str(),
            a["keys"].as_str().filter(|k| !k.is_empty()),
        )
        .map(|r| json!(r)),
        "luna_sessions" => serde_json::from_value::<agents::SessionFilter>(a.clone())
            .map_err(|e| format!("bad arguments: {e}"))
            .and_then(|f| match f.history {
                Some(true) => {
                    let mut runs = crate::agentlog::runs(f.q.as_deref());
                    for last in runs.iter_mut().filter_map(|r| r.last.as_mut()) {
                        if last.chars().count() > HISTORY_LAST {
                            *last = format!("{}…", last.chars().take(HISTORY_LAST).collect::<String>());
                        }
                    }
                    Ok(json!({ "runs": runs }))
                }
                _ => agents::sessions(caller, &f).map(|s| json!({ "sessions": s })),
            }),
        "luna_spawn" => serde_json::from_value::<agents::SpawnParams>(a.clone())
            .map_err(|e| format!("bad arguments: {e}"))
            .and_then(|p| agents::spawn(caller, p))
            .map(|s| json!(s)),
        "luna_send" => agents::send(caller, &id, a["text"].as_str().unwrap_or(""), a["interrupt"].as_bool().unwrap_or(false))
            .map(|s| json!(s)),
        "luna_read" => agents::read(
            caller,
            &id,
            a["since"].as_u64(),
            // A cursor asks for everything after it; a bare read, for the
            // last word.
            a["last"].as_u64().map(|n| n as usize).or(a["since"].as_u64().is_none().then_some(1)),
            a["tools"].as_bool().unwrap_or(false),
            a["screen"].as_bool().unwrap_or(false),
            this,
        )
        .map(|r| json!(r)),
        "luna_wait" => agents::wait(
            caller,
            &id,
            a["until"].as_str().unwrap_or("turn_done"),
            a["timeoutS"].as_u64().unwrap_or(600),
            a["afterTurn"].as_u64().map(|n| n as u32),
            this,
        )
        .map(|w| json!(w)),
        "luna_kill" => agents::kill(caller, &id).map(|()| json!({ "ok": true })),
        "luna_delete" => agents::delete(caller, &id, a["dropWorktree"].as_bool().unwrap_or(false))
            .map(|wt| json!({ "ok": true, "worktreeRemoved": wt })),
        "luna_notify" => agents::notify(caller, a["text"].as_str().unwrap_or("")).map(|()| json!({ "ok": true })),
        _ => Err(format!("unknown tool {name}")),
    };
    match out {
        Ok(v) => json!({ "content": content(v) }),
        Err(e) => json!({ "content": [{ "type": "text", "text": e }], "isError": true }),
    }
}

/// A result as text blocks: the JSON, then the terminal if it has one. A
/// screen left inside the JSON reaches the model with its line breaks as `\n`
/// escapes — the whole terminal on one line, a diff on it past reading.
fn content(mut v: Value) -> Vec<Value> {
    let screen = v.as_object_mut().and_then(|o| o.remove("screen"));
    let mut blocks = vec![json!({ "type": "text", "text": v.to_string() })];
    if let Some(Value::String(s)) = screen {
        blocks.push(json!({ "type": "text", "text": format!("screen:\n{s}") }));
    }
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A session that was given the tools.
    fn tooled(id: &str) {
        agents::register(id, crate::provider::Provider::Claude, "C:/p", "C:/acc/anthropic/one", None, true, json!({}));
    }

    #[test]
    fn initialize_lists_tools_and_answers_ping() {
        tooled("me");
        let init = handle("me", r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}"#).unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert!(init["result"]["instructions"].as_str().unwrap().contains("luna_guide"));
        assert!(handle("me", r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
        let list = handle("me", r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#).unwrap();
        let names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["luna_list", "luna_sessions", "luna_spawn", "luna_send", "luna_answer", "luna_read", "luna_wait", "luna_kill", "luna_delete", "luna_guide", "luna_notify"]);
        // One without them is offered none, and may call none.
        agents::register("plain", crate::provider::Provider::Claude, "C:/p", "C:/acc/anthropic/one", None, false, json!({}));
        assert_eq!(handle("plain", r#"{"jsonrpc":"2.0","id":9,"method":"tools/list"}"#).unwrap()["result"]["tools"], json!([]));
        let init = handle("plain", r#"{"jsonrpc":"2.0","id":11,"method":"initialize","params":{}}"#).unwrap();
        assert!(init["result"].get("instructions").is_none(), "no word of tools it does not have");
        let r = handle("plain", r#"{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"luna_list","arguments":{}}}"#).unwrap();
        assert_eq!(r["result"]["isError"], true);
        assert_eq!(handle("me", r#"{"jsonrpc":"2.0","id":3,"method":"ping"}"#).unwrap()["result"], json!({}));
        assert_eq!(handle("me", r#"{"jsonrpc":"2.0","id":4,"method":"nope"}"#).unwrap()["error"]["code"], -32601);
        assert_eq!(handle("me", "{").unwrap()["error"]["code"], -32700);
    }

    #[test]
    fn only_requests_are_answered() {
        tooled("me");
        assert!(handle("me", r#"{"jsonrpc":"2.0","method":"ping"}"#).is_none(), "no id: a notification");
        assert!(handle("me", r#"{"jsonrpc":"2.0","id":1,"result":{}}"#).is_none(), "a response");
        assert_eq!(handle("me", r#"[{"jsonrpc":"2.0","id":1,"method":"ping"}]"#).unwrap()["error"]["code"], -32600);
        let init = handle("me", r#"{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"2099-01-01"}}"#).unwrap();
        assert_eq!(init["result"]["protocolVersion"], PROTOCOL, "a version it does not know is answered with its own");
    }

    #[test]
    fn a_cancel_reaches_the_call_it_names() {
        let call = InFlight::start("cancel-test", &json!(7));
        let other = InFlight::start("cancel-test", &json!("7"));
        assert!(handle("cancel-test", r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":7}}"#).is_none());
        assert!(call.call.cancelled());
        assert!(!other.call.cancelled(), "a string id is another call");
        drop(call);
        assert!(!in_flight().lock().unwrap().contains_key(&("cancel-test".to_string(), "7".to_string())));
    }

    #[test]
    fn a_hand_over_is_undone_by_a_cancel_either_side_of_it() {
        let undone = Arc::new(AtomicU8::new(0));
        let undo = || {
            let u = undone.clone();
            move || {
                u.fetch_add(1, Ordering::SeqCst);
            }
        };
        // Cancelled first: the commit fails and undoes it itself.
        let first = Call::new();
        first.cancel();
        assert!(!first.commit(undo()));
        assert_eq!(undone.load(Ordering::SeqCst), 1);
        // Committed first: the response goes out, and a cancel after it,
        // even once the call has ended, puts the answer back, once.
        let late = InFlight::start("late-cancel", &json!(3));
        assert!(late.call.commit(undo()));
        assert!(!late.call.cancelled());
        drop(late);
        let cancel = r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":3}}"#;
        assert!(handle("late-cancel", cancel).is_none());
        assert!(handle("late-cancel", cancel).is_none());
        assert_eq!(undone.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_tool_on_a_foreign_session_is_an_error_result() {
        tooled("me");
        let r = handle("me", r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"luna_kill","arguments":{"id":"someone"}}}"#).unwrap();
        assert_eq!(r["result"]["isError"], true);
        assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains("not one you spawned"));
        // Text may go to any session, but only one Luna runs.
        let r = handle("me", r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"luna_send","arguments":{"id":"nobody","text":"hi"}}}"#).unwrap();
        assert_eq!(r["result"]["isError"], true);
        assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains("unknown session"));
    }

    #[test]
    fn the_tool_text_stays_small() {
        // The whole list is what every tooled session pays for once; keep it
        // under a few thousand characters. The annotations are the client's,
        // not the model's.
        let mut list = tools();
        for t in list.as_array_mut().unwrap() {
            t.as_object_mut().unwrap().remove("annotations");
        }
        let text = list.to_string();
        assert!(text.len() < 6000, "tools/list is {} chars", text.len());
        assert!(INSTRUCTIONS.len() < 1000, "instructions are {} bytes", INSTRUCTIONS.len());
        // Read only on request, but whole: keep it to a few thousand tokens.
        assert!(GUIDE.len() < 12_000, "the guide is {} bytes", GUIDE.len());
    }

    #[test]
    fn the_instructions_name_the_notice_tag() {
        // What a model is told to look for is what every notice opens with.
        let shown = "[Luna notice";
        assert!(INSTRUCTIONS.contains(shown));
        assert!(agents::NOTICE_TAG.starts_with(shown));
    }

    #[test]
    fn the_guide_is_a_tool_result() {
        tooled("guide-test");
        let r = handle("guide-test", r#"{"jsonrpc":"2.0","id":12,"method":"tools/call","params":{"name":"luna_guide","arguments":{}}}"#).unwrap();
        assert!(r["result"].get("isError").is_none());
        assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains("luna_spawn"));
    }

    #[test]
    fn reading_tools_say_so() {
        let list = tools();
        let hint = |name: &str, key: &str| {
            list.as_array().unwrap().iter().find(|t| t["name"] == name).unwrap()["annotations"][key].as_bool()
        };
        for name in ["luna_list", "luna_sessions", "luna_read", "luna_wait", "luna_guide"] {
            assert_eq!(hint(name, "readOnlyHint"), Some(true), "{name}");
        }
        for name in ["luna_kill", "luna_delete"] {
            assert_eq!(hint(name, "destructiveHint"), Some(true), "{name}");
        }
        assert_eq!(hint("luna_spawn", "readOnlyHint"), Some(false));
        assert_eq!(hint("luna_notify", "destructiveHint"), Some(false));
    }

    #[test]
    fn a_notification_is_held_to_one_per_gap() {
        let call = |text: &str| {
            let body = json!({ "jsonrpc": "2.0", "id": 8, "method": "tools/call",
                "params": { "name": "luna_notify", "arguments": { "text": text } } });
            handle("notify-test", &body.to_string()).unwrap()["result"].clone()
        };
        tooled("notify-test");
        assert_eq!(call("  ")["isError"], true, "an empty one is refused, and does not count");
        assert!(call("Build is green").get("isError").is_none());
        let again = call("And the tests");
        assert_eq!(again["isError"], true);
        assert!(again["content"][0]["text"].as_str().unwrap().contains("try again in"));
    }

    #[test]
    fn a_screen_comes_as_its_own_block() {
        let blocks = content(json!({ "turn": "waiting", "screen": "Allow edit?\n  1. Yes\n  2. No" }));
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["text"], r#"{"turn":"waiting"}"#);
        assert_eq!(blocks[1]["text"], "screen:\nAllow edit?\n  1. Yes\n  2. No");
        assert_eq!(content(json!({ "ok": true })).len(), 1);
    }

    #[test]
    fn an_unknown_until_is_an_error_result() {
        let r = handle("me", r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"luna_wait","arguments":{"id":"x","until":"forever"}}}"#).unwrap();
        assert_eq!(r["result"]["isError"], true);
        assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains("unknown until"));
    }
}
