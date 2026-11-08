//! A tool call from its arguments to its result: which tool, checked how, and
//! what of the agent layer's answer goes back, logged a line per call.

use super::args::arguments_fit;
use super::guide::guide_text;
use super::tools::{names, REMOVED};
use super::{content, Call};
use crate::agents;
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
        "worktree",
        "tools",
        "autoDelete",
        "budgetTokens",
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
        "dropWorktree",
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
        "force",
        "action",
        "quietSec",
        "drain",
        "topic",
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
            "worktreeRemoved",
            "worktreeLeft",
        ];
        let got: Vec<String> = OUTCOME.iter().filter_map(|k| v.get(*k).filter(|v| !v.is_null()).map(|v| format!("{k}={v}"))).collect();
        crate::log::info("mcp", &format!("{line} in {ms}ms: {}", if got.is_empty() { "ok".to_string() } else { got.join(" ") }));
    }
    out
}

fn failure(text: String) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": true })
}

fn run(caller: &str, params: &Value, this: &Call) -> Value {
    let name = params["name"].as_str().unwrap_or("");
    let a = &params["arguments"];
    // A background helper without tools holds a token for its permission
    // handler only.
    if name == "luna_permission" {
        if !crate::background::has(caller) {
            return failure("not a background helper".into());
        }
        return json!({ "content": content(crate::background::permission(caller, a)) });
    }
    if !agents::has_tools(caller) {
        return failure(format!("{name} is not one of this session's tools"));
    }
    if REMOVED.contains(&name) {
        return failure(format!(
            "{name} is gone since Luna 0.11; the tools now: {}. luna_guide_get says which does what.",
            names().join(", ")
        ));
    }
    if let Err(e) = arguments_fit(name, a) {
        return failure(e);
    }
    let key = a["requestKey"].as_str().filter(|k| !k.is_empty() && KEYED.contains(&name));
    let Some(key) = key else { return act(caller, name, a, this) };
    if key.chars().count() > KEY_CHARS {
        return failure(format!("a requestKey is at most {KEY_CHARS} characters"));
    }
    let slot = (caller.to_string(), name.to_string(), key.to_string());
    match keyed(&slot, fingerprint(a)) {
        Keyed::Done(v) => v,
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
    /// Made: its result again.
    Done(Value),
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
    print: String,
    result: Option<Value>,
}

/// Keyed calls by caller, tool and key.
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
    sorted(a).to_string()
}

/// Takes `slot` for a call about to be made, unless one made or under way has it.
fn keyed(slot: &(String, String, String), print: String) -> Keyed {
    let mut slots = key_slots().lock().unwrap_or_else(|e| e.into_inner());
    slots.retain(|_, s| s.at.elapsed() < KEY_KEEP);
    match slots.get(slot) {
        Some(s) if s.print != print => Keyed::Other,
        Some(Slot { result: Some(v), .. }) => Keyed::Done(v.clone()),
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
    // Too large to keep: made, and a retry would make it again.
    if out["isError"] == true || out.to_string().len() > KEPT_RESULT_BYTES {
        slots.remove(&slot);
    } else if let Some(s) = slots.get_mut(&slot) {
        s.at = Instant::now();
        s.result = Some(out.clone());
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
        "luna_session_get" => agents::scope_all(scope).and_then(|all| agents::session_get(caller, &id, all)).map(|s| json!(s)),
        "luna_session_spawn" => serde_json::from_value::<agents::SpawnParams>(a.clone())
            .map_err(|e| format!("bad arguments: {e}"))
            .and_then(|p| {
                let _admitted = crate::power::admit()?;
                agents::spawn(caller, p)
            })
            .map(|s| json!(s)),
        "luna_session_send" => crate::power::admit()
            .and_then(|_admitted| agents::in_scope(caller, &id, scope, "send to"))
            .and_then(|()| agents::send(caller, &id, a["text"].as_str().unwrap_or(""), a["interrupt"].as_bool().unwrap_or(false)))
            .map(|s| json!(s)),
        "luna_session_answer" => agents::in_scope(caller, &id, scope, "answer").and_then(|()| {
            let entered = crate::power::enter();
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
            .map(|w| {
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
        "luna_account_get" => {
            agents::account_get(caller, a["name"].as_str().unwrap_or(""), a["provider"].as_str(), a["folder"].as_str()).map(|r| json!(r))
        }
        "luna_run_list" => run_list(caller, a),
        "luna_guide_get" => {
            // Plain text, not a JSON value spelled out.
            return match guide_text(a["topic"].as_str()) {
                Ok(text) => json!({ "content": [{ "type": "text", "text": text }] }),
                Err(e) => failure(e),
            };
        }
        "luna_notification_send" => agents::notify(caller, a["text"].as_str().unwrap_or("")).map(|()| json!({ "ok": true })),
        "luna_power_set" => {
            crate::power::arm_for_agent(caller, a["action"].as_str().unwrap_or(""), a["quietSec"].as_u64(), a["drain"].as_bool())
        }
        _ => Err(format!("unknown tool {name}; the tools: {}", names().join(", "))),
    };
    match out {
        Ok(v) => json!({ "content": content(v) }),
        Err(e) => failure(e),
    }
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

/// A wait's result as its `reply` and `screen` ask: the report alone unless
/// told `all` (`rest` saying how many more there are, `file` holding them all),
/// or no reply with `none`; the screen only with `screen: true`. `report` is
/// the agent layer's report part of this result, taken off it before it was
/// turned into JSON.
pub(super) fn as_asked(v: &mut Value, report: Option<agents::ReportPart>, a: &Value) {
    let Some(o) = v.as_object_mut() else { return };
    if a["screen"] != true {
        o.remove("screen");
    }
    match a["reply"].as_str().unwrap_or("report") {
        "all" => {}
        "none" => {
            o.remove("reply");
        }
        _ => {
            let Some(r) = report.filter(|_| o.contains_key("reply")) else { return };
            o.insert("reply".into(), r.text.into());
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
    let w = agents::wait_many(
        caller,
        &ids,
        all,
        a["until"].as_str().unwrap_or("turn_done"),
        a["timeoutSec"].as_u64().unwrap_or(600),
        a["scope"].as_str(),
        this,
    )?;
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
    let (drop, force) = (a["dropWorktree"].as_bool().unwrap_or(false), a["force"].as_bool().unwrap_or(false));
    let one = |id: &str| -> Result<Value, String> {
        match agents::delete(caller, id, drop, force) {
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
    let all = agents::scope_all(a["scope"].as_str())?;
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
        // A result too large to keep is not kept.
        assert!(matches!(keyed(&slot("kb-big", "k"), "p".into()), Keyed::New));
        settle_key(slot("kb-big", "k"), &json!({ "text": "x".repeat(KEPT_RESULT_BYTES) }));
        assert_eq!(held("kb-big"), 0);
        key_slots().lock().unwrap().retain(|(c, _, _), _| !c.starts_with("kb-"));
    }
}
