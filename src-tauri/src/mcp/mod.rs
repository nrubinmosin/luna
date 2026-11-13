//! Luna as an MCP server, for the sessions that asked for it: JSON-RPC over
//! streamable HTTP on the hub listener (`POST /mcp`, bearer token per
//! session). Sixteen tools (tools.rs), described as briefly as a model can
//! still use them — every word here lands in the caller's context once per
//! session — a short `instructions` string in place of any system-prompt
//! flag, and the rest in `luna_guide_get`, a topic at a time (guide.rs). A
//! session without the tools is told nothing of them.
//!
//! Each request is answered with a plain JSON body (the transport allows it;
//! no SSE stream is ever opened), so `wait` simply holds its HTTP request
//! until the session it watches gets there — or the client cancels it.

mod args;
mod dispatch;
mod guide;
mod tools;

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

const INSTRUCTIONS: &str = "Luna runs this session. Its luna_* tools start and drive helper sessions in the \
background: another model or account, Codex or Claude Code, another folder or a git worktree. Spawn with a \
prompt that stands alone, then end your turn: once a helper answers, asks or exits, Luna types a notice into \
your session opening \"[Luna notice\" (Luna's words, not the user's) that quotes a short answer or names a \
file. luna_guide_get has the details, a topic at a time. Session ids are Luna's; your CLI's task and subagent \
ids are not, and a subagent of yours using these tools acts as you.";

const RUNNING: u8 = 0;
const COMMITTED: u8 = 1;
const CANCELED: u8 = 2;

/// How long a call that has been answered stays where a cancel finds it: a
/// client that cancels as the response crosses its cancel drops it unread.
pub(crate) const LATE_CANCEL: Duration = Duration::from_secs(10);

/// One tool call's fate, decided once: committed to its response (a `wait`
/// or `read` has handed an owed answer over in it) or canceled (no
/// response). A cancel that comes after the commit undoes the hand-over
/// instead, so the answer is owed again and the notice brings it.
pub struct Call {
    state: AtomicU8,
    undo: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl Call {
    pub(crate) fn new() -> Call {
        Call { state: AtomicU8::new(RUNNING), undo: Mutex::new(None) }
    }

    pub fn canceled(&self) -> bool {
        self.state.load(Ordering::SeqCst) == CANCELED
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

    pub(crate) fn cancel(&self) {
        if self.state.compare_exchange(RUNNING, CANCELED, Ordering::SeqCst, Ordering::SeqCst).is_err()
            && self.state.load(Ordering::SeqCst) == COMMITTED
        {
            let undo = self.undo.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some(undo) = undo {
                undo();
            }
        }
    }
}

// lang-check: allow
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
        m.retain(|_, entry| entry.1.is_none_or(|at| at.elapsed() < LATE_CANCEL));
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
/// response from the client, or a request it canceled.
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
    // A `luna_session_wait` held open is the call this matters for: let go, it must
    // not take the answer it was waiting for with it.
    // lang-check: allow
    if method == "notifications/cancelled" {
        let key = (caller.to_string(), params["requestId"].to_string());
        let call = in_flight().lock().unwrap_or_else(|e| e.into_inner()).get(&key).map(|(c, _)| c.clone());
        if let Some(c) = call {
            c.cancel();
        }
    }
    // Without an id it is a notification; without a method, a response to
    // nothing this server asks. Neither is answered.
    let id = req.get("id").cloned().filter(|_| !method.is_empty() && !method.starts_with("notifications/"))?;
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
        "tools/list" => Ok(json!({ "tools": tools::tools_for(caller) })),
        "tools/call" => {
            let call_out = InFlight::start(caller, &id);
            let r = dispatch::call(caller, &params, &call_out.call);
            // A canceled request gets no response.
            if call_out.call.canceled() {
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

/// What this server puts in a model's context, by the name
/// docs/user-guide/context.md measures it under (context_page.rs): the
/// instructions, the tool list and the handoff's tool as a client sends them
/// on (annotations are the client's, not the model's), and the guide's index
/// and sections, each a tool result when asked for.
#[cfg(test)]
pub(crate) fn context_texts() -> Vec<(String, String)> {
    fn bare(mut v: Value) -> String {
        if let Some(o) = v.as_object_mut() {
            o.remove("annotations");
        }
        for t in v.as_array_mut().into_iter().flatten() {
            if let Some(o) = t.as_object_mut() {
                o.remove("annotations");
            }
        }
        v.to_string()
    }
    let mut out = vec![
        ("instructions".to_string(), INSTRUCTIONS.to_string()),
        ("tools".to_string(), bare(tools::tools())),
        ("handoff-tool".to_string(), bare(tools::handoff_tool())),
        ("guide-index".to_string(), guide::guide_text(None).unwrap_or_default()),
    ];
    for (title, text) in guide::sections() {
        let word = title.split_whitespace().next().unwrap_or_default().to_lowercase();
        out.push((format!("guide-{word}"), text.to_string()));
    }
    out
}

/// A result as text blocks: the JSON, then the terminal if it has one, for
/// each session of a wait on several. A screen left inside the JSON reaches
/// the model with its line breaks as `\n` escapes — the whole terminal on one
/// line, a diff on it past reading.
fn content(mut v: Value) -> Vec<Value> {
    let screen = v.as_object_mut().and_then(|o| o.remove("screen"));
    let mut screens = Vec::new();
    for r in v.get_mut("results").and_then(Value::as_array_mut).into_iter().flatten() {
        let id = r["id"].as_str().unwrap_or("?").to_string();
        if let Some(Value::String(s)) = r.as_object_mut().and_then(|o| o.remove("screen")) {
            screens.push(json!({ "type": "text", "text": format!("screen of {id}:\n{s}") }));
        }
    }
    let mut blocks = vec![json!({ "type": "text", "text": v.to_string() })];
    if let Some(Value::String(s)) = screen {
        blocks.push(json!({ "type": "text", "text": format!("screen:\n{s}") }));
    }
    blocks.extend(screens);
    blocks
}

#[cfg(test)]
mod tests {
    use super::dispatch::{as_asked, lineage_of, runs_in_scope};
    use super::guide::{guide_text, sections, FIRST_READ, GUIDE, TOPICS};
    use super::tools::{tools, REMOVED};
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
        assert!(init["result"]["instructions"].as_str().unwrap().contains("luna_guide_get"));
        assert!(handle("me", r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
        let list = handle("me", r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#).unwrap();
        let names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            [
                "luna_session_list",
                "luna_session_get",
                "luna_session_spawn",
                "luna_session_send",
                "luna_session_answer",
                "luna_session_wait",
                "luna_session_stop",
                "luna_session_delete",
                "luna_message_list",
                "luna_report_get",
                "luna_account_list",
                "luna_account_get",
                "luna_run_list",
                "luna_guide_get",
                "luna_notification_send",
                "luna_power_set",
                // A person's chat with Luna's tools (agents/handoff `may_list`).
                "luna_session_handoff"
            ]
        );
        // One without them is offered none, and may call none.
        agents::register("plain", crate::provider::Provider::Claude, "C:/p", "C:/acc/anthropic/one", None, false, json!({}));
        assert_eq!(handle("plain", r#"{"jsonrpc":"2.0","id":9,"method":"tools/list"}"#).unwrap()["result"]["tools"], json!([]));
        let init = handle("plain", r#"{"jsonrpc":"2.0","id":11,"method":"initialize","params":{}}"#).unwrap();
        assert!(init["result"].get("instructions").is_none(), "no word of tools it does not have");
        let r = handle("plain", r#"{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"luna_session_list","arguments":{}}}"#)
            .unwrap();
        assert_eq!(r["result"]["isError"], true);
        assert_eq!(handle("me", r#"{"jsonrpc":"2.0","id":3,"method":"ping"}"#).unwrap()["result"], json!({}));
        assert_eq!(handle("me", r#"{"jsonrpc":"2.0","id":4,"method":"nope"}"#).unwrap()["error"]["code"], -32601);
        assert_eq!(handle("me", "{").unwrap()["error"]["code"], -32700);
    }

    #[test]
    fn only_a_background_claude_code_helper_is_offered_the_permission_handler() {
        use crate::provider::Provider;
        let names = |id: &str| -> Vec<String> {
            let r = handle(id, r#"{"jsonrpc":"2.0","id":20,"method":"tools/list"}"#).unwrap();
            r["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect()
        };
        // As Claude Code's CLI calls it: between turns nobody is there to
        // answer, so the handler says no at once.
        let ask = |id: &str| {
            let body = json!({ "jsonrpc": "2.0", "id": 21, "method": "tools/call",
                "params": { "name": "luna_permission", "arguments": { "tool_name": "Bash", "input": { "command": "ls" } } } });
            handle(id, &body.to_string()).unwrap()["result"].clone()
        };
        let refused = |r: &Value, why: &str| r["isError"] == true && r["content"][0]["text"].as_str().unwrap_or("").contains(why);
        let permission = "luna_permission".to_string();
        tooled("pt-chat");
        for (id, provider, tools) in
            [("pt-claude", Provider::Claude, false), ("pt-claude-tools", Provider::Claude, true), ("pt-codex", Provider::Codex, true)]
        {
            let acc = if provider == Provider::Claude { "C:/acc/anthropic/one" } else { "C:/acc/openai/two" };
            agents::register(id, provider, "C:/p", acc, Some("pt-chat"), tools, json!({ "background": true }));
            crate::background::idle_for_test(id, provider, 1, false);
        }
        // A Claude Code helper: the handler alone without Luna's tools, beside
        // them with, and its CLI's call reaches it.
        assert_eq!(names("pt-claude"), ["luna_permission"]);
        let listed = names("pt-claude-tools");
        assert!(listed.contains(&permission) && listed.contains(&"luna_session_list".to_string()), "{listed:?}");
        for id in ["pt-claude", "pt-claude-tools"] {
            let r = ask(id);
            assert!(r.get("isError").is_none(), "{id}: {r}");
            let answer: Value = serde_json::from_str(r["content"][0]["text"].as_str().unwrap()).unwrap();
            assert_eq!(answer["behavior"], "deny", "{id}: {answer}");
        }
        // A Codex helper with Luna's tools: the rest, not this, and a call to it
        // is a tool there is not. No ask is held for it.
        let listed = names("pt-codex");
        assert!(listed.contains(&"luna_session_list".to_string()) && !listed.contains(&permission), "{listed:?}");
        assert!(refused(&ask("pt-codex"), "unknown tool luna_permission"));
        assert!(crate::background::ask("pt-codex").is_none());
        // A chat, either provider, with Luna's tools or without, is no helper.
        agents::register("pt-codex-chat", Provider::Codex, "C:/p", "C:/acc/openai/two", None, true, json!({}));
        agents::register("pt-plain", Provider::Claude, "C:/p", "C:/acc/anthropic/one", None, false, json!({}));
        for id in ["pt-chat", "pt-codex-chat"] {
            assert!(!names(id).contains(&permission), "{id}");
            assert!(refused(&ask(id), "unknown tool luna_permission"), "{id}");
        }
        assert!(names("pt-plain").is_empty());
        assert!(refused(&ask("pt-plain"), "not one of this session's tools"));
        for id in ["pt-claude", "pt-claude-tools", "pt-codex"] {
            crate::background::gone_for_test(id);
        }
        for id in ["pt-claude", "pt-claude-tools", "pt-codex", "pt-codex-chat", "pt-plain", "pt-chat"] {
            agents::forget(id);
        }
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
        // lang-check: allow
        assert!(handle("cancel-test", r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":7}}"#).is_none());
        assert!(call.call.canceled());
        assert!(!other.call.canceled(), "a string id is another call");
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
        // Canceled first: the commit fails and undoes it itself.
        let first = Call::new();
        first.cancel();
        assert!(!first.commit(undo()));
        assert_eq!(undone.load(Ordering::SeqCst), 1);
        // Committed first: the response goes out, and a cancel after it,
        // even once the call has ended, puts the answer back, once.
        let late = InFlight::start("late-cancel", &json!(3));
        assert!(late.call.commit(undo()));
        assert!(!late.call.canceled());
        drop(late);
        // lang-check: allow
        let cancel = r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":3}}"#;
        assert!(handle("late-cancel", cancel).is_none());
        assert!(handle("late-cancel", cancel).is_none());
        assert_eq!(undone.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_tool_on_a_foreign_session_is_an_error_result() {
        tooled("me");
        let r = handle(
            "me",
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"luna_session_stop","arguments":{"id":"someone"}}}"#,
        )
        .unwrap();
        assert_eq!(r["result"]["isError"], true);
        assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains("not one you spawned"));
        // Text may go to any session, but only one Luna runs.
        let r = handle(
            "me",
            r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"luna_session_send","arguments":{"id":"nobody","text":"hi"}}}"#,
        )
        .unwrap();
        assert_eq!(r["result"]["isError"], true);
        assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains("unknown session"));
    }

    #[test]
    fn a_session_not_ones_own_is_reached_only_with_scope_all() {
        tooled("rs-me");
        agents::register("rs-kid", crate::provider::Provider::Claude, "C:/p", "C:/acc/anthropic/one", Some("rs-me"), false, json!({}));
        agents::register("rs-chat", crate::provider::Provider::Codex, "C:/p", "C:/acc/openai/two", None, false, json!({}));
        let call = |name: &str, args: Value| {
            let body = json!({ "jsonrpc": "2.0", "id": 16, "method": "tools/call", "params": { "name": name, "arguments": args } });
            let r = handle("rs-me", &body.to_string()).unwrap()["result"].clone();
            (r["isError"] == true, r["content"][0]["text"].as_str().unwrap_or("").to_string())
        };
        let refused = |(failed, text): (bool, String)| failed && text.contains("scope: \"all\"");
        assert!(refused(call("luna_message_list", json!({ "id": "rs-chat" }))));
        assert!(refused(call("luna_session_wait", json!({ "id": "rs-chat", "timeoutSec": 1 }))));
        assert!(refused(call("luna_session_wait", json!({ "ids": ["rs-kid", "rs-chat"], "timeoutSec": 1 }))));
        assert!(refused(call("luna_session_send", json!({ "id": "rs-chat", "text": "hi" }))));
        assert!(refused(call("luna_message_list", json!({ "id": "rs-chat", "scope": "mine" }))));
        // An answer is at least a send: its question or permission is another's.
        assert!(refused(call("luna_session_answer", json!({ "id": "rs-chat", "answers": ["1"] }))));
        assert!(refused(call("luna_session_answer", json!({ "id": "rs-chat", "allow": false }))));
        // Told so, the scope lets it by and the tool goes on to its own answer.
        let (_, text) = call("luna_message_list", json!({ "id": "rs-chat", "scope": "all" }));
        assert!(!text.contains("scope"), "{text}");
        let (_, text) = call("luna_session_send", json!({ "id": "rs-chat", "text": "hi", "scope": "all" }));
        assert!(text.contains("not running"), "{text}");
        let (_, text) = call("luna_session_answer", json!({ "id": "rs-chat", "answers": ["1"], "scope": "all" }));
        assert!(text.contains("not running"), "{text}");
        // Its own needs nothing said.
        let (_, text) = call("luna_message_list", json!({ "id": "rs-kid" }));
        assert!(!text.contains("scope"), "{text}");
        let (_, text) = call("luna_session_answer", json!({ "id": "rs-kid", "answers": ["1"] }));
        assert!(text.contains("not running"), "{text}");
        for id in ["rs-kid", "rs-chat", "rs-me"] {
            agents::forget(id);
        }
    }

    /// A helper with tools (a lead) reaches its own subtree and nothing past it,
    /// in every verb, `scope: "all"` or not; nor does it notify the person or
    /// set the PC's power. Its parent, a person's chat, keeps all of it.
    #[test]
    fn a_helper_reaches_only_its_own_subtree_in_every_tool() {
        let claude = crate::provider::Provider::Claude;
        let bg = json!({ "background": true });
        tooled("lr-top");
        agents::register("lr-lead", claude, "C:/p", "C:/acc/anthropic/one", Some("lr-top"), true, bg.clone());
        agents::register("lr-mine", claude, "C:/p", "C:/acc/anthropic/one", Some("lr-lead"), false, bg.clone());
        agents::register("lr-sib", claude, "C:/p", "C:/acc/anthropic/one", Some("lr-top"), false, bg);
        agents::register("lr-chat", claude, "C:/p", "C:/acc/anthropic/one", None, false, json!({}));
        let call = |caller: &str, name: &str, args: Value| {
            let body = json!({ "jsonrpc": "2.0", "id": 19, "method": "tools/call", "params": { "name": name, "arguments": args } });
            let r = handle(caller, &body.to_string()).unwrap()["result"].clone();
            (r["isError"] == true, r["content"][0]["text"].as_str().unwrap_or("").to_string())
        };
        let refused = |(failed, text): (bool, String), what: &str| assert!(failed && text.contains(what), "{text}");
        for target in ["lr-top", "lr-sib", "lr-chat"] {
            for scope in [json!(null), json!("all")] {
                let past = if scope.is_null() { "not under you" } else { "is for a person's chats" };
                refused(call("lr-lead", "luna_message_list", json!({ "id": target, "scope": scope })), past);
                refused(call("lr-lead", "luna_report_get", json!({ "id": target, "scope": scope })), past);
                refused(call("lr-lead", "luna_session_wait", json!({ "id": target, "timeoutSec": 1, "scope": scope })), past);
                refused(call("lr-lead", "luna_session_wait", json!({ "ids": ["lr-mine", target], "timeoutSec": 1, "scope": scope })), past);
                refused(call("lr-lead", "luna_session_send", json!({ "id": target, "text": "hi", "scope": scope })), past);
                refused(call("lr-lead", "luna_session_answer", json!({ "id": target, "answers": ["1"], "scope": scope })), past);
                refused(call("lr-lead", "luna_session_get", json!({ "id": target, "scope": scope })), past);
                refused(call("lr-lead", "luna_session_list", json!({ "ids": [target], "scope": scope })), past);
            }
            refused(call("lr-lead", "luna_session_stop", json!({ "id": target })), "not one you spawned");
            refused(call("lr-lead", "luna_session_delete", json!({ "id": target })), "not one you spawned");
        }
        refused(call("lr-lead", "luna_session_list", json!({ "scope": "all" })), "is for a person's chats");
        refused(call("lr-lead", "luna_run_list", json!({ "scope": "all" })), "is for a person's chats");
        refused(call("lr-lead", "luna_notification_send", json!({ "text": "done" })), "is for a person's chats");
        refused(call("lr-lead", "luna_power_set", json!({ "action": "cancel" })), "is for a person's chats");
        // Its own: itself and what is under it.
        let (failed, text) = call("lr-lead", "luna_session_list", json!({ "ids": ["lr-lead", "lr-mine"] }));
        assert!(!failed && text.contains("lr-mine") && text.contains("\"places\""), "{text}");
        let (failed, text) = call("lr-lead", "luna_session_get", json!({ "id": "lr-mine" }));
        assert!(!failed, "{text}");
        let (_, text) = call("lr-lead", "luna_message_list", json!({ "id": "lr-mine" }));
        assert!(!text.contains("not under you"), "{text}");
        // The person's chat at the top keeps its reach.
        let (_, text) = call("lr-top", "luna_message_list", json!({ "id": "lr-chat", "scope": "all" }));
        assert!(!text.contains("scope") && !text.contains("not under you"), "{text}");
        // By id: a list of every session is paged, and other tests' fill it.
        let (failed, text) = call("lr-top", "luna_session_list", json!({ "ids": ["lr-chat"], "scope": "all" }));
        assert!(!failed && text.contains("lr-chat"), "{text}");
        // Past the gate to the tool's own check, with nothing armed or disarmed.
        let (_, text) = call("lr-top", "luna_power_set", json!({ "action": "explode" }));
        assert!(text.contains("unknown action"), "{text}");
        for id in ["lr-mine", "lr-lead", "lr-sib", "lr-chat", "lr-top"] {
            agents::forget(id);
        }
    }

    #[test]
    fn what_another_session_asks_or_was_asked_needs_scope_all() {
        tooled("ms-me");
        let background = json!({ "background": true });
        let claude = crate::provider::Provider::Claude;
        agents::register("ms-chat", claude, "C:/p", "C:/acc/anthropic/one", None, false, json!({}));
        agents::register("ms-theirs", claude, "C:/p", "C:/acc/anthropic/one", Some("ms-chat"), false, background.clone());
        agents::register("ms-kid", claude, "C:/p", "C:/acc/anthropic/one", Some("ms-me"), false, background);
        for id in ["ms-theirs", "ms-kid"] {
            crate::background::take_up_for_test(id, "C:/p", 1, &[], None);
            crate::background::set_running_for_test(id);
            crate::background::hold_for_test(id);
        }
        let call = |name: &str, args: Value| {
            let body = json!({ "jsonrpc": "2.0", "id": 18, "method": "tools/call", "params": { "name": name, "arguments": args } });
            let r = handle("ms-me", &body.to_string()).unwrap()["result"].clone();
            assert!(r.get("isError").is_none(), "{r}");
            serde_json::from_str::<Value>(r["content"][0]["text"].as_str().unwrap()).unwrap()
        };
        let row = |v: &Value, id: &str| v["sessions"].as_array().unwrap().iter().find(|s| s["id"] == id).cloned().unwrap();
        // A row of any session: its state, not what it asks or was asked.
        let listed = call("luna_session_list", json!({ "ids": ["ms-theirs", "ms-kid"] }));
        let theirs = row(&listed, "ms-theirs");
        assert!(theirs["state"].is_string() && theirs.get("askId").is_none() && theirs.get("task").is_none(), "{theirs}");
        // One whole: the ask itself, the same way.
        assert!(call("luna_session_get", json!({ "id": "ms-theirs" })).get("ask").is_none());
        assert!(call("luna_session_get", json!({ "id": "ms-theirs", "scope": "all" }))["ask"].is_object());
        assert!(call("luna_session_get", json!({ "id": "ms-kid" }))["ask"].is_object());
        for id in ["ms-theirs", "ms-kid"] {
            crate::background::remove(id);
        }
        for id in ["ms-kid", "ms-theirs", "ms-chat", "ms-me"] {
            agents::forget(id);
        }
    }

    #[test]
    fn past_spawns_are_the_callers_own_unless_scope_all() {
        tooled("hs-me");
        agents::register("hs-kid", crate::provider::Provider::Claude, "C:/p", "C:/acc/anthropic/one", Some("hs-me"), true, json!({}));
        let run = |id: &str, parent: &str| crate::agentlog::Run {
            id: id.into(),
            at: "t".into(),
            ended: None,
            how: None,
            parent: parent.into(),
            parent_name: None,
            folder: "C:/p".into(),
            worktree: false,
            provider: "claude".into(),
            account: "one".into(),
            settings: json!({}),
            name: None,
            prompt: "the whole prompt".into(),
            auto_delete: false,
            transcript: None,
            worktree_path: None,
            last: Some("the whole reply".into()),
        };
        let runs = || vec![run("r1", "hs-me"), run("r2", "hs-kid"), run("r3", "someone-else"), run("r4", "")];
        let ids = |runs: Vec<crate::agentlog::Run>| runs.into_iter().map(|r| r.id).collect::<Vec<_>>();
        let lineage = lineage_of(&runs());
        assert_eq!(ids(runs_in_scope("hs-me", runs(), false, &lineage)), ["r1", "r2"], "its own and its helpers'");
        assert_eq!(ids(runs_in_scope("hs-me", runs(), true, &lineage)), ["r1", "r2", "r3", "r4"]);
        // A -> B -> C, and B deleted: C passed up to A, its run still naming B.
        agents::register("hs-b", crate::provider::Provider::Claude, "C:/p", "C:/acc/anthropic/one", Some("hs-me"), true, json!({}));
        agents::register("hs-c", crate::provider::Provider::Claude, "C:/p", "C:/acc/anthropic/one", Some("hs-b"), false, json!({}));
        agents::forget("hs-b");
        assert_eq!(agents::parent_of("hs-c").as_deref(), Some("hs-me"));
        let chain = || vec![run("hs-b", "hs-me"), run("hs-c", "hs-b"), run("hs-x", "someone-else")];
        let lineage = lineage_of(&chain());
        assert_eq!(ids(runs_in_scope("hs-me", chain(), false, &lineage)), ["hs-b", "hs-c"], "by C's owner now");
        // And with C gone too: by the lineage the runs record.
        agents::forget("hs-c");
        assert_eq!(ids(runs_in_scope("hs-me", chain(), false, &lineage)), ["hs-b", "hs-c"], "by B's run");
        assert!(runs_in_scope("someone-else", vec![run("hs-c", "hs-b")], false, &lineage).is_empty(), "not another's");
        // A -> B, the person took B over, and B spawned C since: neither run is
        // A's, by the owners now, and once both are gone by the log's word on
        // the takeover, which cuts B's lineage.
        agents::register("hs-tb", crate::provider::Provider::Claude, "C:/p", "C:/acc/anthropic/one", Some("hs-me"), true, json!({}));
        agents::take_over("hs-tb", || Some(Vec::new())).unwrap();
        agents::register("hs-tc", crate::provider::Provider::Claude, "C:/p", "C:/acc/anthropic/one", Some("hs-tb"), false, json!({}));
        let took = |id: &str, parent: &str| crate::agentlog::Run { how: Some(crate::agentlog::TAKEN_OVER.into()), ..run(id, parent) };
        let taken = || vec![took("hs-tb", "hs-me"), run("hs-tc", "hs-tb")];
        let lineage = lineage_of(&taken());
        assert!(runs_in_scope("hs-me", taken(), false, &lineage).is_empty(), "the person's chat and its helper are not A's");
        agents::forget("hs-tc");
        agents::forget("hs-tb");
        assert!(runs_in_scope("hs-me", taken(), false, &lineage).is_empty(), "nor once both are gone");
        assert_eq!(ids(runs_in_scope("hs-me", taken(), true, &lineage)), ["hs-tb", "hs-tc"], "with scope all, as anyone's");
        // Through the tool: a scope it does not know is refused, not taken for mine.
        let body = json!({ "jsonrpc": "2.0", "id": 19, "method": "tools/call",
            "params": { "name": "luna_run_list", "arguments": { "scope": "everyone" } } });
        assert_eq!(handle("hs-me", &body.to_string()).unwrap()["result"]["isError"], true);
        agents::forget("hs-kid");
        agents::forget("hs-me");
    }

    #[test]
    fn every_tool_holds_its_arguments_to_its_schema() {
        tooled("sa-me");
        let call = |name: &str, args: Value| {
            let body = json!({ "jsonrpc": "2.0", "id": 17, "method": "tools/call", "params": { "name": name, "arguments": args } });
            let r = handle("sa-me", &body.to_string()).unwrap()["result"].clone();
            (r["isError"] == true, r["content"][0]["text"].as_str().unwrap_or("").to_string())
        };
        let list = tools();
        for tool in list.as_array().unwrap() {
            let name = tool["name"].as_str().unwrap();
            let props = tool["inputSchema"]["properties"].as_object().unwrap();
            // A key it does not have, named back with those it does.
            let (failed, e) = call(name, json!({ "bogus_key": 1 }));
            assert!(failed && e.contains("no argument `bogus_key`"), "{name}: {e}");
            for key in props.keys() {
                assert!(e.contains(key.as_str()), "{name} names {key}: {e}");
            }
            // Each of its keys given a value of no type it takes.
            for (key, spec) in props {
                let wrong = match spec["type"].as_str() {
                    Some("string") => json!(7),
                    None => json!({ "not": "this" }),
                    Some(_) => json!("7"),
                };
                let (failed, e) = call(name, json!({ key.as_str(): wrong }));
                assert!(failed && e.contains(&format!("`{key}` takes")), "{name}.{key}: {e}");
            }
            // An id that names nothing.
            if props.contains_key("id") {
                let (failed, e) = call(name, json!({ "id": "", "ids": null }));
                assert!(failed && e.contains("is required"), "{name}: {e}");
            }
            assert!(!call(name, json!("args")).1.is_empty(), "{name}: not an object");
        }
        // The cases that cost something when passed over.
        let (failed, e) = call("luna_session_spawn", json!({ "prompt": "review it", "read_only": true }));
        assert!(failed && e.contains("did you mean `readOnly`?"), "{e}");
        // Helpers run in the background only: what asked for a visible one, or
        // pressed keys on one's screen, is refused as any key the tool lacks.
        for (tool, args) in [
            ("luna_session_spawn", json!({ "prompt": "go", "visible": true })),
            ("luna_session_spawn", json!({ "prompt": "go", "visible": false })),
            ("luna_session_answer", json!({ "id": "x", "keys": "{enter}" })),
        ] {
            let key = args.as_object().unwrap().keys().find(|k| *k != "prompt" && *k != "id").unwrap().clone();
            let (failed, e) = call(tool, args);
            assert!(failed && e.contains(&format!("`{key}`")) && e.contains("takes"), "{tool}.{key}: {e}");
        }
        assert!(call("luna_session_send", json!({ "id": "x", "text": "stop", "interrupt": "true" }))
            .1
            .contains("`interrupt` takes true or false"));
        assert!(call("luna_message_list", json!({ "id": "x", "last": -1 })).1.contains("`last` takes a whole number"));
        assert!(call("luna_session_wait", json!({})).1.contains("id or ids is required"));
        assert!(call("luna_session_wait", json!({ "id": "" })).1.contains("id or ids is required"));
        assert!(call("luna_session_send", json!({ "id": "x" })).1.contains("text is required"));
        assert!(call("luna_session_spawn", json!({ "prompt": "go", "account": 3 }))
            .1
            .contains("`account` takes a string or a list of strings"));
        assert!(call("luna_power_set", json!({ "action": "explode" })).1.contains("unknown action"));
        // What the schema allows still goes through to the tool.
        let (failed, e) = call("luna_guide_get", json!({ "topic": null }));
        assert!(!failed, "a null is a key left out: {e}");
        let (_, e) = call("luna_session_wait", json!({ "ids": ["sa-nobody"], "timeoutSec": 1 }));
        assert!(e.contains("unknown session"), "{e}");
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
        assert!(text.len() < 9000, "tools/list is {} chars", text.len());
        assert!(INSTRUCTIONS.len() < 1000, "instructions are {} bytes", INSTRUCTIONS.len());
        // Read a section at a time: none is to cost more than a couple of
        // thousand tokens, nor the index more than a few hundred.
        for (title, text) in sections() {
            assert!(text.len() < 7_000, "{title} is {} bytes", text.len());
        }
        let index = guide_text(None).unwrap();
        assert!(index.len() < 1_200, "the guide's index is {} bytes: {index}", index.len());
    }

    #[test]
    fn the_guide_comes_a_topic_at_a_time() {
        let sections = sections();
        // Each topic is answered by a section there is, and each section by a topic.
        for (topic, title) in TOPICS {
            let text = guide_text(Some(topic)).unwrap();
            assert!(text.starts_with(&format!("## {title}")), "{topic}: {text}");
        }
        for (title, _) in &sections {
            assert!(TOPICS.iter().any(|(_, t)| t == title), "nothing leads to {title}");
        }
        // Every tool is a topic, and what the index suggests first is too; the
        // instructions point to the guide and name no topic to read.
        for t in tools().as_array().unwrap() {
            let name = t["name"].as_str().unwrap();
            assert!(TOPICS.iter().any(|(topic, _)| *topic == name), "{name} has no topic");
        }
        for topic in FIRST_READ {
            assert!(TOPICS.iter().any(|(t, _)| *t == topic), "{topic}");
        }
        assert!(INSTRUCTIONS.contains("luna_guide_get") && !INSTRUCTIONS.contains("topics "));
        // No topic: the opening and what to ask for, never the whole.
        let index = guide_text(None).unwrap();
        assert!(index.starts_with("# Luna: sessions, accounts and the luna_* tools") && index.contains("Restart: restart"), "{index}");
        assert!(index.len() < GUIDE.len() / 4);
        assert!(guide_text(Some("weather")).unwrap_err().contains("luna_session_spawn"));
        // Nothing a caller is told names a tool there is not.
        for old in REMOVED {
            for (title, text) in &sections {
                let named = text.match_indices(old).any(|(i, _)| !text[i + old.len()..].starts_with('_'));
                assert!(!named, "{title} names {old}");
            }
            assert!(!INSTRUCTIONS.contains(&format!("{old} ")), "the instructions name {old}");
        }
    }

    /// Every session with Luna's tools, helpers included, reads the same
    /// guide, instructions and tool list; the handoff, which only a person's
    /// chat is to know of, is in none of them: no topic, no section, no word in
    /// the index, the enum or a tool's description. That chat has it from the
    /// tool itself, listed to it alone (agents/handoff `may_list`).
    #[test]
    fn what_every_tooled_session_reads_leaves_the_handoff_to_its_own_tool() {
        assert!(TOPICS.iter().all(|(topic, title)| !topic.contains("handoff") && !title.to_lowercase().contains("handoff")));
        for (title, text) in sections() {
            assert!(!text.to_lowercase().contains("handoff"), "{title}");
        }
        assert!(!guide_text(None).unwrap().to_lowercase().contains("handoff"));
        assert!(guide_text(Some("luna_session_handoff")).is_err());
        assert!(!GUIDE.to_lowercase().contains("handoff") && !INSTRUCTIONS.to_lowercase().contains("handoff"));
        assert!(!tools().to_string().to_lowercase().contains("handoff"));
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
        let call = |args: &str| {
            let body =
                format!(r#"{{"jsonrpc":"2.0","id":12,"method":"tools/call","params":{{"name":"luna_guide_get","arguments":{args}}}}}"#);
            handle("guide-test", &body).unwrap()["result"].clone()
        };
        let r = call(r#"{"topic":"luna_session_spawn"}"#);
        assert!(r.get("isError").is_none());
        assert!(r["content"][0]["text"].as_str().unwrap().starts_with("## Spawning"));
        let index = call("{}");
        assert!(index["content"][0]["text"].as_str().unwrap().contains("topic"), "{index}");
        assert_eq!(call(r#"{"topic":"weather"}"#)["isError"], true);
    }

    #[test]
    fn reading_tools_say_so() {
        let list = tools();
        let hint =
            |name: &str, key: &str| list.as_array().unwrap().iter().find(|t| t["name"] == name).unwrap()["annotations"][key].as_bool();
        for name in [
            "luna_session_list",
            "luna_session_get",
            "luna_message_list",
            "luna_report_get",
            "luna_session_wait",
            "luna_account_list",
            "luna_account_get",
            "luna_run_list",
            "luna_guide_get",
        ] {
            assert_eq!(hint(name, "readOnlyHint"), Some(true), "{name}");
        }
        for name in ["luna_session_stop", "luna_session_delete", "luna_power_set"] {
            assert_eq!(hint(name, "destructiveHint"), Some(true), "{name}");
        }
        assert_eq!(hint("luna_session_spawn", "readOnlyHint"), Some(false));
        assert_eq!(hint("luna_notification_send", "destructiveHint"), Some(false));
    }

    #[test]
    fn a_report_is_one_message_at_most_and_says_what_it_left() {
        tooled("rr-me");
        agents::register("rr-kid", crate::provider::Provider::Claude, "C:/p", "C:/acc/anthropic/one", Some("rr-me"), false, json!({}));
        let body = json!({ "jsonrpc": "2.0", "id": 15, "method": "tools/call",
            "params": { "name": "luna_report_get", "arguments": { "id": "rr-kid", "ack": true } } });
        let r = handle("rr-me", &body.to_string()).unwrap()["result"].clone();
        assert!(r.get("isError").is_none(), "{r}");
        let v: Value = serde_json::from_str(r["content"][0]["text"].as_str().unwrap()).unwrap();
        assert!(v["messages"].as_array().is_some_and(|m| m.len() <= 1), "{v}");
        assert_eq!(v["transcriptMissing"], true, "no transcript, nothing to report: {v}");
        assert!(v.get("acked").is_none(), "nothing owed, nothing collected: {v}");
        agents::forget("rr-kid");
    }

    #[test]
    fn a_notification_is_held_to_one_per_gap() {
        let call = |text: &str| {
            let body = json!({ "jsonrpc": "2.0", "id": 8, "method": "tools/call",
                "params": { "name": "luna_notification_send", "arguments": { "text": text } } });
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
        // A wait on several: each screen its own block, named.
        let blocks = content(json!({ "outcome": "timeout", "results": [
            { "id": "a", "outcome": "timeout", "screen": "A\nB" },
            { "id": "b", "outcome": "pending" }
        ] }));
        assert_eq!(blocks.len(), 2);
        assert!(!blocks[0]["text"].as_str().unwrap().contains("screen"));
        assert_eq!(blocks[1]["text"], "screen of a:\nA\nB");
    }

    #[test]
    fn a_wait_gives_its_report_unless_told_and_its_screen_only_when_told() {
        let report = || Some(agents::ReportPart { text: "The report.".into(), rest: 2, guessed: true, file: Some("C:/notes/a.md".into()) });
        // As the agent layer leaves it for `report`: its reply the report already.
        let waited = || json!({ "outcome": "turn_done", "state": "idle", "reply": "The report.", "screen": "A\nB" });
        let shaped = |a: Value| {
            let mut v = waited();
            as_asked(&mut v, report(), &a);
            v
        };
        // By default: the report, how many more, where all of it is; no screen.
        let v = shaped(json!({}));
        assert_eq!(v["reply"], "The report.");
        assert_eq!((v["rest"].as_u64(), v["file"].as_str(), v["reportGuessed"].as_bool()), (Some(2), Some("C:/notes/a.md"), Some(true)));
        assert!(v.get("screen").is_none(), "{v}");
        // All of it, as asked; none at all; the screen when told.
        assert!(shaped(json!({ "reply": "all" })).get("rest").is_none(), "all of it: nothing more to point at");
        assert!(shaped(json!({ "reply": "none" })).get("reply").is_none());
        assert_eq!(shaped(json!({ "screen": true }))["screen"], "A\nB");
        // No reply to begin with: nothing is made up.
        let mut bare = json!({ "outcome": "timeout", "state": "working" });
        as_asked(&mut bare, None, &json!({}));
        assert_eq!(bare, json!({ "outcome": "timeout", "state": "working" }));
    }

    #[test]
    fn a_removed_tool_is_refused_with_the_tools_there_are() {
        tooled("rm-me");
        for old in REMOVED {
            let body = json!({ "jsonrpc": "2.0", "id": 20, "method": "tools/call", "params": { "name": old, "arguments": {} } });
            let r = handle("rm-me", &body.to_string()).unwrap()["result"].clone();
            let text = r["content"][0]["text"].as_str().unwrap();
            assert!(
                r["isError"] == true && text.contains("is gone") && text.contains("luna_session_list, luna_session_get"),
                "{old}: {text}"
            );
        }
        // Accounts are a resource of their own now.
        let body =
            json!({ "jsonrpc": "2.0", "id": 21, "method": "tools/call", "params": { "name": "luna_account_list", "arguments": {} } });
        let r = handle("rm-me", &body.to_string()).unwrap()["result"].clone();
        assert!(r.get("isError").is_none(), "{r}");
        let body = json!({ "jsonrpc": "2.0", "id": 22, "method": "tools/call",
            "params": { "name": "luna_account_get", "arguments": { "name": "rm-no-such-account" } } });
        assert_eq!(handle("rm-me", &body.to_string()).unwrap()["result"]["isError"], true);
    }

    #[test]
    fn a_change_retried_with_its_request_key_is_made_once() {
        tooled("rk-me");
        agents::register(
            "rk-kid",
            crate::provider::Provider::Claude,
            "C:/p",
            "C:/acc/anthropic/one",
            Some("rk-me"),
            false,
            json!({ "background": true }),
        );
        crate::background::take_up_for_test("rk-kid", "C:/p", 1, &[], None);
        crate::background::set_running_for_test("rk-kid");
        let send = |key: &str, text: &str| {
            let body = json!({ "jsonrpc": "2.0", "id": 24, "method": "tools/call",
                "params": { "name": "luna_session_send", "arguments": { "id": "rk-kid", "text": text, "requestKey": key } } });
            handle("rk-me", &body.to_string()).unwrap()["result"].clone()
        };
        assert!(send(&"k".repeat(129), "Fix it.")["content"][0]["text"].as_str().unwrap().contains("at most 128"));
        let first = send("k1", "Fix it.");
        assert!(first.get("isError").is_none(), "{first}");
        assert_eq!(send("k1", "Fix it."), first, "the first call's result, again");
        // The key stands for that call: other text under it is refused, not passed off.
        let other = send("k1", "Something else.");
        assert!(other["isError"] == true && other["content"][0]["text"].as_str().unwrap().contains("other arguments"), "{other}");
        // The same call in another key order is the same call.
        let body = json!({ "jsonrpc": "2.0", "id": 26, "method": "tools/call",
            "params": { "name": "luna_session_send", "arguments": { "requestKey": "k1", "text": "Fix it.", "id": "rk-kid" } } });
        assert_eq!(handle("rk-me", &body.to_string()).unwrap()["result"], first);
        assert_eq!(crate::background::queued_for_test("rk-kid"), ["Fix it."], "and the text went in once");
        // Another key is another call.
        assert!(send("k2", "And the docs.").get("isError").is_none());
        assert_eq!(crate::background::queued_for_test("rk-kid").len(), 2);
        // A failed call made nothing: a retry makes it anew.
        let body = json!({ "jsonrpc": "2.0", "id": 25, "method": "tools/call",
            "params": { "name": "luna_session_send", "arguments": { "id": "rk-nobody", "text": "hi", "requestKey": "k3" } } });
        assert_eq!(handle("rk-me", &body.to_string()).unwrap()["result"]["isError"], true);
        assert!(!key_slots_hold("rk-me", "k3"));
        crate::background::remove("rk-kid");
        agents::forget("rk-kid");
    }

    /// Whether a keyed call of `caller` with `key` is kept.
    fn key_slots_hold(caller: &str, key: &str) -> bool {
        dispatch::key_held(caller, key)
    }

    #[test]
    fn a_delete_of_one_gone_already_answers_already() {
        tooled("da-me");
        // Never Luna's: an error, as a typo is.
        let call = |args: Value| {
            let body = json!({ "jsonrpc": "2.0", "id": 23, "method": "tools/call", "params": { "name": "luna_session_delete", "arguments": args } });
            handle("da-me", &body.to_string()).unwrap()["result"].clone()
        };
        assert_eq!(call(json!({ "id": "da-never" }))["isError"], true);
        let several = call(json!({ "ids": ["da-never", "da-never-2"] }));
        let v: Value = serde_json::from_str(several["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(v["results"].as_array().unwrap().len(), 2, "{v}");
        assert!(v["results"][0]["error"].is_string(), "each its own: {v}");
        assert_eq!(call(json!({ "id": "x", "ids": ["y"] }))["isError"], true, "one or the other");
    }

    #[test]
    fn a_wait_on_several_takes_its_arguments_apart_from_one() {
        tooled("several-test");
        let wait = |args: Value| {
            let body =
                json!({ "jsonrpc": "2.0", "id": 13, "method": "tools/call", "params": { "name": "luna_session_wait", "arguments": args } });
            let r = handle("several-test", &body.to_string()).unwrap();
            assert_eq!(r["result"]["isError"], true, "{r}");
            r["result"]["content"][0]["text"].as_str().unwrap().to_string()
        };
        assert!(wait(json!({ "id": "x", "ids": ["y"] })).contains("not both"));
        assert!(wait(json!({ "ids": ["y"], "afterTurn": 2 })).contains("afterTurn goes with id"));
        assert!(wait(json!({ "ids": "y" })).contains("a list"));
        assert!(wait(json!({ "ids": [1] })).contains("a list"));
        assert!(wait(json!({ "ids": ["y"], "mode": "some" })).contains("unknown mode"));
        assert!(wait(json!({ "ids": [] })).contains("empty"));
        let many: Vec<String> = (0..17).map(|i| format!("wm-{i}")).collect();
        assert!(wait(json!({ "ids": many })).contains("16 at most"), "a thread each: bounded");
        assert!(wait(json!({ "ids": ["nobody"] })).contains("unknown session"));
    }

    #[test]
    fn an_unknown_until_is_an_error_result() {
        tooled("me");
        let r = handle(
            "me",
            r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"luna_session_wait","arguments":{"id":"uu-x","until":"forever"}}}"#,
        )
        .unwrap();
        assert_eq!(r["result"]["isError"], true);
        assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains("unknown until"));
    }
}
