//! A tool call from its arguments to its result: which tool, checked how, and
//! what of the agent layer's answer goes back, logged a line per call.

use super::args::arguments_fit;
use super::guide::guide_text_as;
use super::tools::{names_as, REMOVED};
use super::{content, Call};
use crate::agents;
use crate::boundary::AbsPath;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The tools a caller may retry with a `requestKey` without making their change
/// twice: a spawn can take 90 s, and a client that gave up on it would start a
/// second helper.
const KEYED: [&str; 3] = ["luna_session_spawn", "luna_session_send", "luna_session_delete"];

/// How long a keyed call's result is kept for a retry.
const KEY_KEEP: Duration = Duration::from_secs(60 * 60);

/// What a `requestKey` and what is kept for it may come to: a key's length,
/// how many one caller and all callers hold, and a result's size. Past a count,
/// the oldest finished call goes; one too large to keep is made, not kept.
const KEY_CHARS: usize = 128;
const KEYS_PER_CALLER: usize = 64;
const KEYS_IN_ALL: usize = 1024;
const KEPT_RESULT_BYTES: usize = 16 * 1024;
/// All the results kept whole come to no more than this; past it, or past
/// KEPT_RESULT_BYTES, a call keeps its outcome alone (`Kept::Outcome`).
const KEPT_BYTES_IN_ALL: usize = 2 * 1024 * 1024;

/// The most of a past run's prompt and last reply `luna_run_list` passes on: a
/// row is a pointer to the run, whose transcript has it whole.
const RUN_PROMPT: usize = 160;
const RUN_LAST: usize = 240;

/// How many past runs `luna_run_list` gives unless asked, and at most.
const RUNS: usize = 10;
const RUNS_MAX: usize = 50;

/// One line per call in luna.log: who called what on whom, how it ended and
/// how long it took. The arguments that steer a call are named; what is said
/// through it (prompts, sent text, answers, notifications) is not.
pub(super) fn call(caller: &str, params: &Value, this: &Call) -> Value {
    let started = Instant::now();
    let out = run(caller, params, this);
    let name = params["name"].as_str().unwrap_or("?");
    let a = &params["arguments"];
    let mut line = format!("{} {name}", agents::label(caller));
    if let Some(id) = a["id"].as_str() {
        line.push_str(&format!(" on {}", agents::label(id)));
    }
    const STEERING: &[&str] = &[
        "provider",
        "account",
        "model",
        "effort",
        "permissionMode",
        "sandbox",
        "readOnly",
        "agent",
        "approval",
        "folder",
        "tools",
        "autoDelete",
        "budgetTokens",
        "budgetPct",
        "expectReply",
        "connectors",
        "name",
        "until",
        "timeoutSec",
        "afterTurn",
        "reply",
        "since",
        "last",
        "toolCalls",
        "screen",
        "interrupt",
        "scope",
        "group",
        "state",
        "q",
        "limit",
        "cursor",
        "allow",
        "askId",
        "ack",
        "ids",
        "mode",
        "resumeOnRestart",
        "action",
        "quietSec",
        "drain",
        "topic",
        "file",
    ];
    let set: Vec<String> = STEERING.iter().filter_map(|k| a.get(*k).filter(|v| !v.is_null()).map(|v| format!("{k}={v}"))).collect();
    if !set.is_empty() {
        line.push_str(&format!(" ({})", set.join(" ")));
    }
    let ms = started.elapsed().as_millis();
    if this.canceled() {
        crate::log::info("mcp", &format!("{line} canceled by the client after {ms}ms"));
    } else if out["isError"] == true {
        let e = out["content"][0]["text"].as_str().unwrap_or("");
        let e: String = e.chars().take(300).collect();
        crate::log::warn("mcp", &format!("{line} failed in {ms}ms: {e}"));
    } else {
        // The first block is the result's JSON; its few fields that say how
        // the call went.
        let v: Value = out["content"][0]["text"].as_str().and_then(|t| serde_json::from_str(t).ok()).unwrap_or(Value::Null);
        const OUTCOME: &[&str] = &[
            "id",
            "outcome",
            "failed",
            "finished",
            "started",
            "queued",
            "turnsEnded",
            "state",
            "rest",
            "acked",
            "already",
            "nextCursor",
            "scheduled",
            "n",
        ];
        let got: Vec<String> = OUTCOME.iter().filter_map(|k| v.get(*k).filter(|v| !v.is_null()).map(|v| format!("{k}={v}"))).collect();
        crate::log::info("mcp", &format!("{line} in {ms}ms: {}", if got.is_empty() { "ok".to_string() } else { got.join(" ") }));
    }
    out
}

fn failure(text: String) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": true })
}

/// A call of a tool there is not, with the tools the caller is offered.
fn unknown_tool(caller: &str, name: &str) -> String {
    format!("unknown tool {name}; the tools: {}", names_as(agents::is_persons_chat(caller)).join(", "))
}

fn run(caller: &str, params: &Value, this: &Call) -> Value {
    let name = params["name"].as_str().unwrap_or("");
    let a = &params["arguments"];
    if !agents::has_tools(caller) {
        return failure(format!("{name} is not one of this session's tools"));
    }
    if REMOVED.contains(&name) {
        return failure(format!(
            "{name} is gone since Luna 0.11; the tools now: {}. luna_guide_get says which does what.",
            names_as(agents::is_persons_chat(caller)).join(", ")
        ));
    }
    if let Err(e) = arguments_fit(name, a, agents::is_persons_chat(caller)) {
        return failure(e);
    }
    let key = a["requestKey"].as_str().filter(|k| !k.is_empty() && KEYED.contains(&name));
    let Some(key) = key else { return act(caller, name, a, this) };
    if key.chars().count() > KEY_CHARS {
        return failure(format!("a requestKey is at most {KEY_CHARS} characters"));
    }
    let Some(owner) = key_owner(caller) else {
        return failure("this session's token is no longer valid: it was handed over or started again".into());
    };
    let slot = (owner, name.to_string(), key.to_string());
    match keyed(&slot, fingerprint(a)) {
        Keyed::Done(Kept::Whole(v)) => v,
        // Made once, its result too large to keep: what it made, not a second making.
        Keyed::Done(Kept::Outcome(Some(made))) => {
            let done = json!({ "replayed": true, "id": made, "note": "the first call's result was too large to keep" });
            json!({ "content": content(done) })
        }
        Keyed::Done(Kept::Outcome(None)) => failure(format!(
            "the {name} with requestKey {key:?} was made and succeeded; its result was too large to keep, so it is not given again (nor made again)"
        )),
        Keyed::Running => failure(format!("a {name} with requestKey {key:?} is still under way; its result comes to that call")),
        Keyed::Full => failure(format!("{KEYS_PER_CALLER} keyed calls of yours are under way; retry once one has its result")),
        Keyed::Other => failure(format!(
            "requestKey {key:?} was given to a {name} with other arguments; a key stands for one call, so a new call takes a new key"
        )),
        Keyed::New => {
            let out = act(caller, name, a, this);
            settle_key(slot, &out);
            out
        }
    }
}

/// Where a keyed call stands (`keyed`).
enum Keyed {
    /// Never made, or it failed: made now.
    New,
    Running,
    /// Made: its result again, or what it made.
    Done(Kept),
    /// The key stands for a call with other arguments: refused, not that
    /// call's result passed off as this one's.
    Other,
    /// No room: every kept call that could make way is still under way.
    Full,
}

/// A keyed call: since when, the arguments it was made with (`fingerprint`),
/// and its result once it has one. Kept in the core's memory, so a core restart
/// forgets every key; the guide says so.
struct Slot {
    at: Instant,
    /// SHA-256 of the arguments (`fingerprint`), as hex: the same size whatever they are.
    print: String,
    result: Option<Kept>,
}

/// What is kept of a call made: its result whole, or, too large for that, the
/// id of what it made (none for a call whose result names none).
#[derive(Clone)]
enum Kept {
    Whole(Value),
    Outcome(Option<String>),
}

/// Whose a keyed call is: its caller, as many times handed over as it is now
/// (`agents::key_generation`). A CLI started again has a new token and keeps
/// its keys, as the guide says only a core restart forgets them; a helper
/// handed over to the person keeps its id, and a result kept for the helper's
/// calls is not given to the chat it became, nor the other way. None for a
/// call whose token no longer names its caller.
fn key_owner(caller: &str) -> Option<String> {
    agents::key_generation(caller).map(|generation| format!("{caller} {generation}"))
}

/// Keyed calls by caller (`key_owner`), tool and key.
fn key_slots() -> &'static Mutex<HashMap<(String, String, String), Slot>> {
    static K: OnceLock<Mutex<HashMap<(String, String, String), Slot>>> = OnceLock::new();
    K.get_or_init(Default::default)
}

/// The call a key stands for: its arguments, the key aside, every object's keys
/// in order, so the same call spelled in another order is the same call.
fn fingerprint(a: &Value) -> String {
    fn sorted(v: &Value) -> Value {
        match v {
            Value::Object(o) => {
                let mut keys: Vec<&String> = o.keys().filter(|k| k.as_str() != "requestKey").collect();
                keys.sort();
                Value::Object(keys.into_iter().map(|k| (k.clone(), sorted(&o[k]))).collect())
            }
            Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
            other => other.clone(),
        }
    }
    use sha2::{Digest, Sha256};
    Sha256::digest(sorted(a).to_string().as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// Takes `slot` for a call about to be made, unless one made or under way has it.
fn keyed(slot: &(String, String, String), print: String) -> Keyed {
    let mut slots = key_slots().lock().unwrap_or_else(|e| e.into_inner());
    slots.retain(|_, s| s.at.elapsed() < KEY_KEEP);
    match slots.get(slot) {
        Some(s) if s.print != print => Keyed::Other,
        Some(Slot { result: Some(kept), .. }) => Keyed::Done(kept.clone()),
        Some(_) => Keyed::Running,
        None => {
            // Room for it: the caller's oldest finished call goes past its own
            // count, anyone's past the whole count.
            let mine = slots.keys().filter(|(c, _, _)| *c == slot.0).count();
            if mine >= KEYS_PER_CALLER || slots.len() >= KEYS_IN_ALL {
                let oldest = slots
                    .iter()
                    .filter(|(k, s)| s.result.is_some() && (mine < KEYS_PER_CALLER || k.0 == slot.0))
                    .min_by_key(|(_, s)| s.at)
                    .map(|(k, _)| k.clone());
                match oldest {
                    Some(k) => {
                        slots.remove(&k);
                    }
                    None => return Keyed::Full,
                }
            }
            slots.insert(slot.clone(), Slot { at: Instant::now(), print, result: None });
            Keyed::New
        }
    }
}

/// Keeps a keyed call's result for its retries; one that failed made nothing,
/// and a retry makes it anew.
fn settle_key(slot: (String, String, String), out: &Value) {
    let mut slots = key_slots().lock().unwrap_or_else(|e| e.into_inner());
    if out["isError"] == true {
        slots.remove(&slot);
        return;
    }
    // Whole where it fits, else what it made: a retry is never made again.
    let size = out.to_string().len();
    let whole: usize = slots
        .values()
        .filter_map(|s| match &s.result {
            Some(Kept::Whole(v)) => Some(v.to_string().len()),
            _ => None,
        })
        .sum();
    let kept = if size <= KEPT_RESULT_BYTES && whole + size <= KEPT_BYTES_IN_ALL {
        Kept::Whole(out.clone())
    } else {
        let made = out["content"][0]["text"].as_str().and_then(|t| serde_json::from_str::<Value>(t).ok());
        Kept::Outcome(made.and_then(|v| v["id"].as_str().map(str::to_owned)))
    };
    if let Some(s) = slots.get_mut(&slot) {
        s.at = Instant::now();
        s.result = Some(kept);
    }
}

/// Whether any keyed call of `caller` with `key` is kept, for the tests.
#[cfg(test)]
pub(super) fn key_held(caller: &str, key: &str) -> bool {
    key_slots().lock().unwrap_or_else(|e| e.into_inner()).keys().any(|(c, _, k)| c == caller && k == key)
}

/// The tool itself, its arguments checked.
fn act(caller: &str, name: &str, a: &Value, this: &Call) -> Value {
    let id = a["id"].as_str().unwrap_or("").to_string();
    let scope = a["scope"].as_str();
    let out: Result<Value, String> = match name {
        "luna_session_list" => serde_json::from_value::<agents::SessionQuery>(a.clone())
            .map_err(|e| format!("bad arguments: {e}"))
            .and_then(|q| agents::session_list(caller, &q))
            .map(|p| json!(p)),
        "luna_session_get" => agents::scope_all(caller, scope).and_then(|all| agents::session_get(caller, &id, all)).map(|s| json!(s)),
        "luna_session_spawn" => folder_arg(a)
            .and_then(|_| serde_json::from_value::<agents::SpawnParams>(a.clone()).map_err(|e| format!("bad arguments: {e}")))
            .and_then(|p| crate::power::admitted(|| agents::spawn(caller, p)))
            .map(|s| json!(s)),
        // Admitted for the whole send, the turn it starts included: a
        // power-off counts it as at work until it is through.
        "luna_session_send" => crate::power::admitted(|| {
            agents::in_scope(caller, &id, scope, "send to")?;
            agents::send(
                caller,
                &id,
                a["text"].as_str().unwrap_or(""),
                a["interrupt"].as_bool().unwrap_or(false),
                a["requestKey"].as_str().filter(|k| !k.is_empty()),
                a["expectReply"].as_bool().unwrap_or(true),
            )
        })
        .map(|s| json!(s)),
        "luna_session_answer" => agents::in_scope(caller, &id, scope, "answer").and_then(|()| {
            let entered = crate::power::enter();
            if let Some(why) = entered.shut() {
                return Err(why);
            }
            agents::answer(
                caller,
                &id,
                a["askId"].as_str().filter(|k| !k.is_empty()),
                &strings(&a["answers"]),
                a["allow"].as_bool(),
                a["note"].as_str(),
                entered.draining(),
            )
            .map(|r| json!(r))
        }),
        "luna_session_wait" if !a["ids"].is_null() => wait_several(caller, a, this),
        "luna_session_wait" => agents::in_scope(caller, &id, scope, "wait on")
            .and_then(|()| {
                agents::wait(
                    caller,
                    &id,
                    a["until"].as_str().unwrap_or("turn_done"),
                    a["timeoutSec"].as_u64().unwrap_or(600),
                    a["afterTurn"].as_u64().map(|n| n as u32),
                    scope,
                    this,
                )
            })
            .map(|mut w| {
                if reply_mode(a) == "report" {
                    agents::report_reply(&id, &mut w);
                }
                let report = w.report.clone();
                let mut v = json!(w);
                as_asked(&mut v, report, a);
                v
            }),
        "luna_session_stop" => agents::kill(caller, &id).map(|k| json!(k)),
        "luna_session_delete" => delete(caller, a),
        "luna_message_list" => agents::in_scope(caller, &id, scope, "read")
            .and_then(|()| {
                agents::read(
                    caller,
                    &id,
                    a["since"].as_u64(),
                    // A cursor asks for everything after it; a bare read, for the
                    // last word.
                    a["last"].as_u64().map(|n| n as usize).or(a["since"].as_u64().is_none().then_some(1)),
                    a["toolCalls"].as_bool().unwrap_or(false),
                    a["screen"].as_bool().unwrap_or(false),
                    this,
                )
            })
            .map(|r| json!(r)),
        "luna_report_get" => agents::in_scope(caller, &id, scope, "read")
            .and_then(|()| agents::report_get(caller, &id, a["since"].as_u64(), a["ack"].as_bool().unwrap_or(false), this))
            .map(|r| json!(r)),
        "luna_account_list" => Ok(json!({ "accounts": agents::account_list(a["provider"].as_str()) })),
        "luna_account_get" => folder_arg(a)
            .and_then(|folder| agents::account_get(caller, a["name"].as_str().unwrap_or(""), a["provider"].as_str(), folder))
            .map(|r| json!(r)),
        "luna_run_list" => run_list(caller, a),
        "luna_guide_get" => {
            // Plain text, not a JSON value spelled out.
            return match guide_text_as(a["topic"].as_str(), agents::is_persons_chat(caller)) {
                Ok(text) => json!({ "content": [{ "type": "text", "text": text }] }),
                Err(e) => failure(e),
            };
        }
        "luna_notification_send" => persons_chat_only(caller, name)
            .and_then(|()| agents::notify(caller, a["text"].as_str().unwrap_or("")))
            .map(|()| json!({ "ok": true })),
        "luna_power_set" => persons_chat_only(caller, name).and_then(|()| {
            crate::power::arm_for_agent(caller, a["action"].as_str().unwrap_or(""), a["quietSec"].as_u64(), a["drain"].as_bool())
        }),
        _ => Err(unknown_tool(caller, name)),
    };
    match out {
        Ok(v) => json!({ "content": content(v) }),
        Err(e) => failure(e),
    }
}

/// The `folder` of `luna_session_spawn` and `luna_account_get`, held to what
/// an agent may name (boundary.rs `agent_folder`: no network share, then
/// absolute, no `.` or `..` part, no device or verbatim prefix, no stream
/// suffix) before anything under it is looked at: the project's settings
/// there are read as the user and a helper started there, and a relative path
/// would be taken from the core's working folder, Luna's own. Not named, or
/// blank: the caller's own.
fn folder_arg(a: &Value) -> Result<Option<AbsPath>, String> {
    match a["folder"].as_str().filter(|f| !f.trim().is_empty()) {
        None => Ok(None),
        Some(f) => crate::boundary::agent_folder(f).map(Some).map_err(|e| format!("folder: {e}")),
    }
}

/// The tools that reach the person or the PC itself go only from a person's
/// chat: a helper's word reaches the person through the chat above it, which
/// decides what they hear (`agents::is_persons_chat`).
fn persons_chat_only(caller: &str, tool: &str) -> Result<(), String> {
    if agents::is_persons_chat(caller) {
        return Ok(());
    }
    Err(format!(
        "{tool} is for a person's chats: a helper does not notify the person or turn the PC off. Say what they should hear in \
         your reply to the session that spawned you"
    ))
}

/// A list of strings as given, a number among them as its digits.
fn strings(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().map(|x| x.as_str().map(str::to_owned).unwrap_or_else(|| x.to_string())).collect()).unwrap_or_default()
}

/// A tool's `ids`, whose each must be a session id.
fn id_list(v: &Value) -> Result<Vec<String>, String> {
    v.as_array()
        .ok_or("ids is a list of session ids")?
        .iter()
        .map(|x| x.as_str().map(str::to_owned).ok_or("ids is a list of session ids".to_string()))
        .collect()
}

/// What a wait's `reply` asks: `report` (the default), `all` or `none`.
fn reply_mode(a: &Value) -> &str {
    a["reply"].as_str().unwrap_or("report")
}

/// A wait's result as its `reply` and `screen` ask, its reply already its
/// report where that is asked (`agents::report_reply`, held to the limit
/// there): with it, `rest` (how many more messages there are) and `file`
/// (all of them); no reply with `none`; the screen only with `screen: true`.
/// `report` is the agent layer's report part of this result, taken off it
/// before it was turned into JSON.
pub(super) fn as_asked(v: &mut Value, report: Option<agents::ReportPart>, a: &Value) {
    let Some(o) = v.as_object_mut() else { return };
    if a["screen"] != true {
        o.remove("screen");
    }
    match reply_mode(a) {
        "all" => {}
        "none" => {
            o.remove("reply");
        }
        _ => {
            let Some(r) = report.filter(|_| o.contains_key("reply")) else { return };
            if r.rest > 0 {
                o.insert("rest".into(), r.rest.into());
                if let Some(file) = r.file {
                    o.insert("file".into(), file.into());
                }
            }
            if r.guessed {
                o.insert("reportGuessed".into(), true.into());
            }
        }
    }
}

/// `luna_session_wait` with `ids`. Each `afterTurn` is one session's count, so
/// it has no place here: each waits for what its caller is owed, as one wait
/// does when not told.
fn wait_several(caller: &str, a: &Value, this: &Call) -> Result<Value, String> {
    if !a["id"].is_null() {
        return Err("give id or ids, not both".into());
    }
    if !a["afterTurn"].is_null() {
        return Err(
            "afterTurn goes with id; with ids each waits for what you are owed, else a turn past the last count you were shown".into()
        );
    }
    let ids = id_list(&a["ids"])?;
    for id in &ids {
        agents::in_scope(caller, id, a["scope"].as_str(), "wait on")?;
    }
    let all = match a["mode"].as_str().unwrap_or("any") {
        "any" => false,
        "all" => true,
        other => return Err(format!("unknown mode {other:?}; use any or all")),
    };
    let mut w = agents::wait_many(
        caller,
        &ids,
        all,
        a["until"].as_str().unwrap_or("turn_done"),
        a["timeoutSec"].as_u64().unwrap_or(600),
        a["scope"].as_str(),
        this,
    )?;
    if reply_mode(a) == "report" {
        agents::report_replies(&mut w.results);
    }
    let reports: Vec<Option<agents::ReportPart>> = w.results.iter().map(|r| r.waited.report.clone()).collect();
    let mut v = json!(w);
    for (r, report) in v["results"].as_array_mut().into_iter().flatten().zip(reports) {
        as_asked(r, report, a);
    }
    Ok(v)
}

/// `luna_session_delete` of `id` or each of `ids`: one result for one, a list
/// for several, each its own. One deleted already (an autoDelete helper reaped,
/// a retry) answers `already` rather than an error.
fn delete(caller: &str, a: &Value) -> Result<Value, String> {
    let ids = match a["id"].as_str().filter(|i| !i.is_empty()) {
        Some(id) if a["ids"].is_null() => vec![id.to_string()],
        Some(_) => return Err("give id or ids, not both".into()),
        None => id_list(&a["ids"])?,
    };
    if ids.is_empty() {
        return Err("ids is empty".into());
    }
    let one = |id: &str| -> Result<Value, String> {
        match agents::delete(caller, id) {
            Ok(r) => Ok(json!(r)),
            Err(_) if gone_already(caller, id) => Ok(json!({ "id": id, "already": "deleted" })),
            Err(e) => Err(e),
        }
    };
    if ids.len() == 1 {
        return one(&ids[0]);
    }
    let results: Vec<Value> = ids.iter().map(|id| one(id).unwrap_or_else(|e| json!({ "id": id, "error": e }))).collect();
    Ok(json!({ "results": results }))
}

/// Whether `id` was one of the caller's and has been deleted since: Luna no
/// longer knows it, and its run, the caller's, has ended.
fn gone_already(caller: &str, id: &str) -> bool {
    if agents::is_known(id) {
        return false;
    }
    let runs = crate::agentlog::runs(None);
    let lineage = lineage_of(&runs);
    runs_in_scope(caller, runs, false, &lineage).iter().any(|r| r.id == id && r.ended.is_some())
}

/// `luna_run_list`: past spawns, newest first, as compact rows: a pointer to
/// each run with the start of its prompt and last reply.
fn run_list(caller: &str, a: &Value) -> Result<Value, String> {
    let all = agents::scope_all(caller, a["scope"].as_str())?;
    let limit = a["limit"].as_u64().map_or(RUNS, |n| n as usize).clamp(1, RUNS_MAX);
    let runs = crate::agentlog::runs(a["q"].as_str());
    // Whose each spawner was, from every run: `q` may leave a link out.
    let lineage = if all { HashMap::new() } else { lineage_of(&crate::agentlog::runs(None)) };
    let runs = runs_in_scope(caller, runs, all, &lineage);
    let more = runs.len() > limit;
    let rows: Vec<Value> = runs.into_iter().take(limit).map(run_row).collect();
    let mut out = json!({ "runs": rows });
    if more {
        out["more"] = true.into();
    }
    Ok(out)
}

/// One past run as a row: no settings or folder (`luna_session_get` has a live
/// one's), its prompt and last reply cut, what it does not have left out.
fn run_row(r: crate::agentlog::Run) -> Value {
    let mut o = Map::new();
    let cut = |text: &str, most: usize| {
        let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if flat.chars().count() <= most {
            flat
        } else {
            format!("{}…", flat.chars().take(most).collect::<String>())
        }
    };
    o.insert("id".into(), r.id.into());
    o.insert("at".into(), r.at.into());
    for (k, v) in [("ended", r.ended), ("how", r.how), ("name", r.name)] {
        if let Some(v) = v {
            o.insert(k.into(), v.into());
        }
    }
    o.insert("provider".into(), r.provider.into());
    o.insert("account".into(), r.account.into());
    o.insert("parent".into(), r.parent.into());
    o.insert("prompt".into(), cut(&r.prompt, RUN_PROMPT).into());
    if let Some(last) = r.last {
        o.insert("last".into(), cut(&last, RUN_LAST).into());
    }
    if let Some(t) = r.transcript {
        o.insert("transcript".into(), t.into());
    }
    Value::Object(o)
}

/// Past spawns as a caller may have them: its own and its helpers' and theirs,
/// or with `all` everyone's. A run holds its whole prompt and its last reply.
/// Whose a run is goes, link by link from the helper up, by the session's owner
/// now while Luna knows it (one whose parent was deleted has passed up,
/// `forget`; one the person took over is theirs), and past what Luna no longer
/// knows by the spawner each run records (`lineage`): the caller or one of its
/// own at a link makes the run the caller's, a live session not its own ends it.
pub(super) fn runs_in_scope(
    caller: &str,
    runs: Vec<crate::agentlog::Run>,
    all: bool,
    lineage: &HashMap<String, String>,
) -> Vec<crate::agentlog::Run> {
    let owned = |run: &crate::agentlog::Run| {
        let mut link = run.id.as_str();
        // As deep as a lineage goes (`registry::depth_of`), and a loop no further.
        for _ in 0..16 {
            if link.is_empty() {
                return false;
            }
            if agents::own_or_self(caller, link) {
                return true;
            }
            if agents::is_known(link) {
                return false;
            }
            match lineage.get(link) {
                Some(spawner) => link = spawner,
                None => return false,
            }
        }
        false
    };
    runs.into_iter().filter(|run| all || owned(run)).collect()
}

/// Each spawned session's spawner, as the runs record them: none for a run the
/// person took over, which was its spawner's no more from then on.
pub(super) fn lineage_of(runs: &[crate::agentlog::Run]) -> HashMap<String, String> {
    runs.iter()
        .filter(|run| run.how.as_deref() != Some(crate::agentlog::TAKEN_OVER))
        .map(|run| (run.id.clone(), run.parent.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(caller: &str, key: &str) -> (String, String, String) {
        (caller.to_string(), "luna_session_send".to_string(), key.to_string())
    }

    /// A folder that is not an absolute plain path, each of them a spelling a
    /// caller could send: relative, a `.` or `..` part, a device or verbatim
    /// prefix, a drive with no root; on Windows a root with no drive and a
    /// stream name too.
    fn refused_folders(base: &str) -> Vec<String> {
        let sep = std::path::MAIN_SEPARATOR;
        let mut refused = vec![
            "project".into(),
            ".".into(),
            format!("..{sep}project"),
            format!("{base}{sep}..{sep}elsewhere"),
            format!("{base}{sep}.{sep}here"),
            r"\\?\C:\work".into(),
            r"\\.\pipe\luna".into(),
            "C:work".into(),
        ];
        if cfg!(windows) {
            refused.extend([r"\work".to_string(), format!("{base}:stream")]);
        }
        refused
    }

    /// The text of a call that failed.
    fn failed(caller: &str, tool: &str, a: &Value) -> String {
        let r = act(caller, tool, a, &Call::new());
        assert_eq!(r["isError"], true, "{a}: {r}");
        r["content"][0]["text"].as_str().unwrap_or("").to_owned()
    }

    /// `luna_account_get`'s folder is held to a folder's shape, and kept out of
    /// Luna's own, before anything is read: a refused one fails on its folder,
    /// ahead of the account lookup (which would say the account is unknown).
    /// A plain absolute folder gets through to it.
    #[test]
    fn an_account_s_folder_is_typed_before_anything_is_read() {
        let get = |folder: &str| failed("ag-me", "luna_account_get", &json!({ "name": "ag-no-such-account", "folder": folder }));
        let base = std::env::temp_dir().join("luna-ag-project");
        let base = base.to_string_lossy();
        for bad in refused_folders(&base) {
            let why = get(&bad);
            assert!(why.starts_with("folder: not a usable path"), "{bad:?}: {why}");
        }
        let luna = crate::paths::data_dir().join("sub");
        assert!(get(&luna.to_string_lossy()).contains("Luna's own folder"), "inside the data folder");
        assert!(get(&base).starts_with("no account \"ag-no-such-account\""), "a plain absolute folder is taken");
        let none = failed("ag-me", "luna_account_get", &json!({ "name": "ag-no-such-account" }));
        assert!(none.starts_with("no account"), "no folder: the caller's own");
        for blank in ["", "  "] {
            assert!(get(blank).starts_with("no account"), "a blank folder is none named: {blank:?}");
        }
    }

    /// `luna_session_spawn`'s folder is held to the same shape, in the same
    /// words, before the spawn reads its arguments or anything else: a caller
    /// that is no session hears of its folder first. A plain absolute folder
    /// gets through to the spawn, which then finds no such caller.
    #[test]
    fn a_spawn_s_folder_is_typed_before_anything_is_made() {
        let spawn = |folder: &str| failed("sf-nobody", "luna_session_spawn", &json!({ "prompt": "Look.", "folder": folder }));
        let base = std::env::temp_dir().join("luna-sf-project");
        let base = base.to_string_lossy();
        for bad in refused_folders(&base) {
            let why = spawn(&bad);
            assert!(why.starts_with("folder: not a usable path"), "{bad:?}: {why}");
        }
        assert_eq!(spawn(&base), "caller is not a registered session", "a plain absolute folder is taken");
        let none = failed("sf-nobody", "luna_session_spawn", &json!({ "prompt": "Look." }));
        assert_eq!(none, "caller is not a registered session", "no folder: the caller's own");
        for blank in ["", "  "] {
            assert_eq!(spawn(blank), "caller is not a registered session", "a blank folder is none named: {blank:?}");
        }
    }

    /// A folder left out is the caller's own, held as a named one is, by a
    /// caller that is a session: in Luna's folder, both tools refuse it before
    /// the account is looked up or the spawn goes on. A caller whose shell is
    /// on a share its folder is not on has its own folder taken, not the
    /// share's worktree: the spawn goes on to the provider it names, which
    /// it refuses as unknown before any account is used.
    #[test]
    fn an_omitted_folder_is_held_as_a_named_one() {
        use crate::provider::Provider;
        let in_luna = crate::paths::data_dir().join("of-sub").to_string_lossy().into_owned();
        let local = std::env::temp_dir().join(format!("luna-of-{}", std::process::id()));
        std::fs::create_dir_all(&local).unwrap();
        let local = local.to_string_lossy().into_owned();
        fn as_session(id: &'static str, folder: &str) -> impl Fn(&str, Value) -> String {
            let token = agents::register(id, Provider::Claude, folder, "C:/acc/anthropic/one", None, true, json!({})).unwrap();
            move |tool: &str, a: Value| {
                let _presenting = agents::presenting(id, &token);
                failed(id, tool, &a)
            }
        }
        let call = as_session("of-in-luna", &in_luna);
        for (tool, a) in [
            ("luna_account_get", json!({ "name": "of-no-such-account" })),
            ("luna_account_get", json!({ "name": "of-no-such-account", "folder": " " })),
            ("luna_session_spawn", json!({ "prompt": "Look.", "provider": "luna-none" })),
            ("luna_session_spawn", json!({ "prompt": "Look.", "provider": "luna-none", "folder": "" })),
        ] {
            let why = call(tool, a.clone());
            assert!(why.contains("Luna's own folder"), "{tool} {a}: {why}");
        }
        let call = as_session("of-lead", &local);
        // Where its shell is: Luna's own folder, and on Windows a share, the
        // network through the object namespace, a device.
        let mut shells =
            vec![crate::paths::data_dir().join("of-shell").join(".claude").join("worktrees").join("w").to_string_lossy().into_owned()];
        if cfg!(windows) {
            shells.extend(
                [
                    r"\\luna-no-such-host\share\p\.claude\worktrees\w",
                    r"\\?\GLOBALROOT\Device\Mup\luna-no-such-host\share\p\.claude\worktrees\w",
                    r"\\.\UNC\luna-no-such-host\share\p\.claude\worktrees\w",
                    r"\\.\C:\p\.claude\worktrees\w",
                ]
                .map(String::from),
            );
        }
        for shell in shells {
            crate::background::take_up_for_test("of-lead", &shell, 1, &[], None);
            assert_eq!(agents::caller_worktree_for_test("of-lead"), None, "{shell}");
            let why = call("luna_session_spawn", json!({ "prompt": "Look.", "provider": "luna-none" }));
            assert!(why.starts_with("unknown provider luna-none"), "{shell}: {why}");
        }
        crate::background::remove("of-lead");
        for id in ["of-in-luna", "of-lead"] {
            agents::forget(id);
        }
        let _ = std::fs::remove_dir_all(&local);
    }

    /// Neither tool takes a folder on a network share, in any spelling of
    /// one, and each says to map it to a drive letter, before anything is
    /// looked at (the host named does not exist). A drive letter, a mapped
    /// share's included, still gets through.
    #[cfg(windows)]
    #[test]
    fn a_folder_on_a_network_share_is_refused_by_both_tools() {
        let get = |folder: &str| failed("ns-me", "luna_account_get", &json!({ "name": "ns-no-such-account", "folder": folder }));
        let spawn = |folder: &str| failed("ns-nobody", "luna_session_spawn", &json!({ "prompt": "Look.", "folder": folder }));
        let shares = [
            r"\\luna-no-such-host\share\project",
            r"\\luna-no-such-host\share",
            "//luna-no-such-host/share/project",
            r"\/luna-no-such-host/share\project",
            r"\\?\UNC\luna-no-such-host\share\project",
            r"\\?\unc\luna-no-such-host\share",
            r"\\.\UNC\luna-no-such-host\share",
            "//?/UNC/luna-no-such-host/share",
        ];
        for share in shares {
            for why in [get(share), spawn(share)] {
                assert!(why.starts_with("folder: a network share") && why.contains("map the share to a drive letter"), "{share}: {why}");
            }
        }
        let mapped = r"Z:\luna-ns-mapped\project";
        assert!(get(mapped).starts_with("no account \"ns-no-such-account\""), "a drive letter is taken");
        assert_eq!(spawn(mapped), "caller is not a registered session", "a drive letter is taken");
    }

    /// A keyed call's result outlives its CLI started again with a new token,
    /// as the guide says only a core restart forgets keys, and not a hand-over:
    /// one kept for a helper's call is not given to the chat it became. Through
    /// `run`, as a call comes.
    #[test]
    fn a_kept_result_outlives_a_restart_and_not_a_hand_over() {
        use crate::provider::Provider;
        let (parent, caller) = ("kt-parent", "kt-helper");
        let register = |id: &str, parent: Option<&str>| {
            agents::register(id, Provider::Claude, "C:/p", "C:/acc/anthropic/one", parent, true, json!({ "background": parent.is_some() }))
        };
        register(parent, None);
        let args = json!({ "id": "kt-nobody", "requestKey": "k1" });
        let params = json!({ "name": "luna_session_delete", "arguments": args });
        let kept = json!({ "content": [{ "type": "text", "text": "kept" }] });
        let call = |token: &str| {
            let _presenting = agents::presenting(caller, token);
            run(caller, &params, &Call::new())
        };
        let first = register(caller, Some(parent)).unwrap();
        // What the first call made, kept under the key `run` will look for.
        let owner = {
            let _presenting = agents::presenting(caller, &first);
            key_owner(caller).expect("its token stands")
        };
        let slot = (owner, "luna_session_delete".to_string(), "k1".to_string());
        assert!(matches!(keyed(&slot, fingerprint(&args)), Keyed::New));
        settle_key(slot, &kept);
        assert_eq!(call(&first), kept, "its own retry");
        let again = register(caller, Some(parent)).unwrap();
        assert_eq!(call(&again), kept, "its CLI started again, with a new token, keeps the key");
        crate::background::idle_for_test(caller, Provider::Claude, 1, false);
        crate::background::background_promote(caller.into()).unwrap();
        assert_eq!(crate::background::background_promoted(caller.into(), None), Ok(true));
        let chats = register(caller, None).unwrap();
        assert_ne!(call(&chats), kept, "the chat it became makes its own anew");
        key_slots().lock().unwrap().retain(|(c, _, _), _| !c.starts_with(caller));
        for s in [caller, parent] {
            agents::forget(s);
        }
    }

    #[test]
    fn kept_calls_are_bounded_per_caller_and_in_size() {
        // A caller's own count: its oldest finished call makes way.
        for i in 0..=KEYS_PER_CALLER {
            assert!(matches!(keyed(&slot("kb-one", &format!("k{i}")), "p".into()), Keyed::New));
            settle_key(slot("kb-one", &format!("k{i}")), &json!({ "ok": true }));
        }
        let held = |caller: &str| key_slots().lock().unwrap().keys().filter(|(c, _, _)| c == caller).count();
        assert_eq!(held("kb-one"), KEYS_PER_CALLER);
        assert!(matches!(keyed(&slot("kb-one", "k0"), "p".into()), Keyed::New), "the oldest went");
        // All still under way: no room, and nothing of another's is touched.
        for i in 0..KEYS_PER_CALLER {
            assert!(matches!(keyed(&slot("kb-busy", &format!("k{i}")), "p".into()), Keyed::New));
        }
        assert!(matches!(keyed(&slot("kb-busy", "one-more"), "p".into()), Keyed::Full));
        // A result too large to keep whole: what it made is kept, and a retry
        // gets that, never a second making.
        let made = json!({ "id": "big-helper", "sameFolder": vec!["x".repeat(100); 200] });
        let big = json!({ "content": [{ "type": "text", "text": made.to_string() }] });
        assert!(matches!(keyed(&slot("kb-big", "k"), "p".into()), Keyed::New));
        settle_key(slot("kb-big", "k"), &big);
        assert_eq!(held("kb-big"), 1);
        assert!(matches!(keyed(&slot("kb-big", "k"), "p".into()), Keyed::Done(Kept::Outcome(Some(ref id))) if id == "big-helper"));
        // One that names nothing it made: kept as made all the same.
        assert!(matches!(keyed(&slot("kb-big", "k2"), "p".into()), Keyed::New));
        settle_key(slot("kb-big", "k2"), &json!({ "text": "x".repeat(KEPT_RESULT_BYTES) }));
        assert!(matches!(keyed(&slot("kb-big", "k2"), "p".into()), Keyed::Done(Kept::Outcome(None))));
        // The arguments are kept as a digest, the same size whatever they are.
        assert_eq!(fingerprint(&json!({ "text": "y".repeat(100_000) })).len(), 64);
        assert_eq!(fingerprint(&json!({ "a": 1, "b": 2 })), fingerprint(&json!({ "b": 2, "a": 1 })));
        key_slots().lock().unwrap().retain(|(c, _, _), _| !c.starts_with("kb-"));
    }
}
