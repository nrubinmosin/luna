//! A Codex helper's turn over `codex app-server` (experimental in 0.169.0):
//! JSON-RPC 2.0 on the process's stdio, one message a line. Luna is the
//! client: `initialize`, `thread/start` or `thread/resume`, `turn/start`; then
//! the server's notifications until `turn/completed`, at which Luna closes
//! stdin and the server goes, which ends the turn as any helper's process
//! does (events.rs `turn_over`). One process a turn, as `codex exec` was:
//! ADR-0032 says why not one a helper for its whole life.
//!
//! Between `turn/start`'s answer and the turn's end, text sent to the helper
//! joins the turn as `turn/steer` (queue.rs `write_line`), delivered once the
//! server says it took it; a stop asks for `turn/interrupt` first and kills
//! the process only if the turn does not end within `INTERRUPT_GRACE`
//! (control.rs). The server's approvals and questions are held as a Claude
//! Code helper's `luna_permission` calls are, for its caller to answer with
//! `luna_session_answer`, and the answer goes back as the reply to the
//! server's request (`reply_when_answered`).

use super::*;

/// The ids of Luna's own requests. A turn's process makes one of each but
/// steers, whose id carries the text's (`Queued::id`).
const INIT: &str = "init";
const OPEN: &str = "open";
const TURN: &str = "turn";
const STEER: &str = "steer:";
const INTERRUPT: &str = "interrupt";

/// How long a turn asked to stop (`turn/interrupt`) has to end before its
/// process is killed. 0.169.0 ended an interrupted 40 s sleep in 0.12 s.
pub(super) const INTERRUPT_GRACE: std::time::Duration = std::time::Duration::from_secs(5);
/// How long the server has to go once its stdin is closed at the turn's end
/// (0.169.0: under 0.1 s) before Luna ends it, and only it: what the turn
/// left running stays the helper's (`turn_over`).
const EXIT_GRACE: std::time::Duration = std::time::Duration::from_secs(10);

/// A Codex turn's side of its conversation with its server.
pub(super) struct Rpc {
    /// `thread/start` or `thread/resume` with its params: what follows the
    /// handshake (command.rs `codex_open`).
    open: Value,
    /// The turn's text and effort, for `turn/start` once the thread is there.
    input: String,
    effort: String,
    /// The turn's id, once `turn/start` has answered: what a steer and an
    /// interrupt name.
    pub(super) turn: Option<String>,
    /// Items begun and not completed, by id, with what each was: an
    /// interrupted turn's get no `item/completed` (0.169.0), so Luna closes
    /// them in its log.
    pub(super) open_items: Vec<(String, String)>,
    /// `turn/interrupt` has gone out.
    pub(super) interrupting: bool,
    /// `turn/completed` came: stdin is closed and the server goes.
    pub(super) done: bool,
}

impl Rpc {
    pub(super) fn new(open: Value, input: String, effort: String) -> Rpc {
        Rpc { open, input, effort, turn: None, open_items: Vec::new(), interrupting: false, done: false }
    }

    /// Whether text may join the turn now: it has an id, and is neither
    /// being stopped nor over.
    pub(super) fn steerable(&self) -> Option<&str> {
        self.turn.as_deref().filter(|_| !self.interrupting && !self.done)
    }
}

fn request(id: &str, method: &str, params: Value) -> String {
    format!("{}\n", json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
}

fn notification(method: &str) -> String {
    format!("{}\n", json!({ "jsonrpc": "2.0", "method": method }))
}

fn reply(id: &Value, result: Value) -> String {
    format!("{}\n", json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

fn refusal(id: &Value, message: &str) -> String {
    format!("{}\n", json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": message } }))
}

/// A text as the server takes input.
fn input(text: &str) -> Value {
    json!([{ "type": "text", "text": text }])
}

/// The first line a turn's server reads. `experimentalApi`: 0.169.0's server
/// was seen to take steers, interrupts and approvals with it.
pub(super) fn initialize_line() -> String {
    request(
        INIT,
        "initialize",
        json!({
            "clientInfo": { "name": "luna", "title": "Luna", "version": env!("CARGO_PKG_VERSION") },
            "capabilities": { "experimentalApi": true },
        }),
    )
}

/// `q` as a steer of the turn running: its id carries the text's, which the
/// server's answer gives back (`took`).
pub(super) fn steer_line(thread: &str, turn: &str, q: &Queued) -> String {
    request(&format!("{STEER}{}", q.id), "turn/steer", json!({ "threadId": thread, "expectedTurnId": turn, "input": input(&q.text) }))
}

/// Asks the turn running to stop, if it has an id and is not over: true if
/// asked. Its `turn/completed` ends it as any turn's end does; what it had
/// begun and not finished is closed then.
pub(super) fn ask_to_stop(e: &mut Entry) -> bool {
    let thread = e.session.clone();
    let Some(run) = e.run.as_mut() else { return false };
    let Some(rpc) = run.rpc.as_mut() else { return false };
    let (Some(thread), Some(turn), false) = (thread, rpc.turn.clone(), rpc.done) else { return false };
    let Some(feed) = run.feed.as_ref() else { return false };
    if rpc.interrupting {
        return true;
    }
    if feed.send(request(INTERRUPT, "turn/interrupt", json!({ "threadId": thread, "turnId": turn }))).is_err() {
        return false;
    }
    rpc.interrupting = true;
    note(e, "asked its turn to stop (turn/interrupt)");
    true
}

/// What one message from a turn's server came to, for its reader to act on
/// with the registry's lock let go.
#[derive(Default, Debug, PartialEq)]
pub(super) struct Heard {
    /// Text the turn was given is the model's now: saved at once.
    pub(super) landed: bool,
    /// Asks held for an answer, by serial (`reply_when_answered`).
    pub(super) asks: Vec<u64>,
    /// The turn has completed and stdin is closed: the server is to go
    /// (`see_out`).
    pub(super) ended: bool,
}

/// One message off a turn's server, into helper `id`, under the registry's
/// lock (run.rs `take_event`).
pub(super) fn on_message(r: &mut Registry, id: &str, v: &Value) -> Heard {
    let mut heard = Heard::default();
    let (method, rid) = (v.get("method").and_then(Value::as_str), v.get("id"));
    if let (Some(method), Some(rid)) = (method, rid) {
        heard.asks.extend(server_request(r, id, rid, method, &v["params"]));
        return heard;
    }
    let Some(e) = r.entries.get_mut(id) else { return heard };
    match (method, rid) {
        (Some(method), _) => notified(e, method, &v["params"], &mut heard),
        (None, Some(rid)) => answered(e, rid.as_str().unwrap_or(""), v, &mut heard),
        (None, None) => {}
    }
    heard
}

/// Writes `line` to the turn's stdin, if it is still open.
fn send(e: &Entry, line: String) -> bool {
    e.run.as_ref().and_then(|run| run.feed.as_ref()).is_some_and(|feed| feed.send(line).is_ok())
}

/// The turn is over or cannot go on: stdin closes, and the server with it.
fn close(e: &mut Entry, heard: &mut Heard) {
    if let Some(run) = e.run.as_mut() {
        run.feed = None;
        if let Some(rpc) = run.rpc.as_mut() {
            rpc.done = true;
        }
        heard.ended = true;
    }
}

/// A request of Luna's refused: the turn cannot go on.
fn failed(e: &mut Entry, what: &str, v: &Value, heard: &mut Heard) {
    let why = v["error"]["message"].as_str().unwrap_or("refused").to_string();
    note(e, &format!("error: {what}: {}", one_line(&why)));
    e.error = Some(format!("{what}: {why}"));
    close(e, heard);
}

/// The server's answer to a request of Luna's.
fn answered(e: &mut Entry, rid: &str, v: &Value, heard: &mut Heard) {
    let ok = v.get("error").is_none();
    match rid {
        INIT if !ok => failed(e, "initialize", v, heard),
        INIT => {
            let open = e.run.as_ref().and_then(|run| run.rpc.as_ref()).map(|rpc| rpc.open.clone()).unwrap_or(Value::Null);
            let method = open["method"].as_str().unwrap_or("thread/start").to_string();
            let sent = send(e, notification("initialized")) && send(e, request(OPEN, &method, open["params"].clone()));
            if !sent {
                close(e, heard);
            }
        }
        OPEN if !ok => failed(e, "the thread would not open", v, heard),
        OPEN => {
            let r = &v["result"];
            if let Some(thread) = r["thread"]["id"].as_str() {
                e.session = Some(thread.to_string());
                mark_by_thread(e);
            }
            note(
                e,
                &format!(
                    "thread {}: approval {}, sandbox {}",
                    e.session.as_deref().unwrap_or("?"),
                    r["approvalPolicy"].as_str().unwrap_or("?"),
                    r["sandbox"]["type"].as_str().unwrap_or("?")
                ),
            );
            let thread = e.session.clone().unwrap_or_default();
            let Some(rpc) = e.run.as_ref().and_then(|run| run.rpc.as_ref()) else { return };
            let mut params = json!({ "threadId": thread, "input": input(&rpc.input) });
            if !rpc.effort.is_empty() {
                params["effort"] = json!(rpc.effort);
            }
            if !send(e, request(TURN, "turn/start", params)) {
                close(e, heard);
            }
        }
        TURN if !ok => failed(e, "the turn would not start", v, heard),
        TURN => {
            let turn = v["result"]["turn"]["id"].as_str().map(str::to_owned);
            if let Some(rpc) = e.run.as_mut().and_then(|run| run.rpc.as_mut()) {
                rpc.turn = turn;
            }
            // The server has the turn's text: not to be handed back.
            heard.landed = stdin_took(e);
        }
        INTERRUPT if !ok => note(e, &format!("turn/interrupt refused: {}", one_line(v["error"]["message"].as_str().unwrap_or("")))),
        INTERRUPT => {}
        steer if steer.starts_with(STEER) => {
            let qid = &steer[STEER.len()..];
            if ok {
                heard.landed |= took(e, qid);
            } else {
                // Missed the turn (it ended first, 0.169.0: "no active turn to
                // steer"): kept in `fed`, it starts the next turn
                // (`sort_out_untaken`).
                note(e, &format!("the turn did not take a steer: {}", one_line(v["error"]["message"].as_str().unwrap_or(""))));
            }
        }
        _ => {}
    }
}

/// What an item is, for the log: its kind, and for a command the command.
fn item_label(item: &Value) -> String {
    let kind = item["type"].as_str().unwrap_or("item");
    match kind {
        "commandExecution" => format!("command {}", one_line(item["command"].as_str().unwrap_or(""))),
        "fileChange" => {
            let paths: Vec<&str> =
                item["changes"].as_array().map(|c| c.iter().filter_map(|c| c["path"].as_str()).collect()).unwrap_or_default();
            format!("file change {}", paths.join(", "))
        }
        other => other.to_string(),
    }
}

/// A notification of the server's.
fn notified(e: &mut Entry, method: &str, p: &Value, heard: &mut Heard) {
    match method {
        "turn/started" => crate::activity::touch(),
        // The model this turn runs on, not the one asked for: what the
        // rollout says is not read for it (events.rs `turn_over`).
        "model/rerouted" => {
            if let Some(to) = p["toModel"].as_str() {
                e.observed.model = Some(to.to_string());
                e.observed.rerouted = true;
                note(e, &format!("model rerouted: {} -> {to}", p["fromModel"].as_str().unwrap_or("?")));
            }
        }
        "item/started" => {
            let item = &p["item"];
            if item["type"] != "userMessage" {
                e.observed.acted = true;
            }
            if let (Some(id), Some(rpc)) = (item["id"].as_str(), e.run.as_mut().and_then(|run| run.rpc.as_mut())) {
                rpc.open_items.push((id.to_string(), item_label(item)));
            }
        }
        "item/completed" => {
            let item = &p["item"];
            if let (Some(id), Some(rpc)) = (item["id"].as_str(), e.run.as_mut().and_then(|run| run.rpc.as_mut())) {
                rpc.open_items.retain(|(open, _)| open != id);
            }
            match item["type"].as_str() {
                Some("agentMessage") => note(e, &format!("said: {}", one_line(item["text"].as_str().unwrap_or("")))),
                Some("commandExecution") => {
                    let end = match (item["exitCode"].as_i64(), item["status"].as_str()) {
                        (Some(code), _) => code.to_string(),
                        (None, Some(status)) => status.to_string(),
                        (None, None) => "?".into(),
                    };
                    note(e, &format!("ran: {} → {end}", one_line(item["command"].as_str().unwrap_or(""))));
                }
                Some("userMessage" | "reasoning") | None => {}
                Some(other) => note(e, other),
            }
        }
        // An error the server retries is no failure: the turn goes on.
        "error" if p["willRetry"].as_bool() == Some(true) => {
            note(e, &format!("retrying: {}", one_line(p["error"]["message"].as_str().unwrap_or(""))));
        }
        "error" => {
            let why = error_of(&p["error"]);
            note(e, &format!("error: {}", one_line(p["error"]["message"].as_str().unwrap_or(&why))));
            e.error = Some(why);
        }
        "turn/completed" => {
            let turn = &p["turn"];
            let ours = e.run.as_ref().and_then(|run| run.rpc.as_ref()).and_then(|rpc| rpc.turn.as_deref());
            if ours.is_some_and(|ours| turn["id"].as_str() != Some(ours)) {
                return;
            }
            match turn["status"].as_str() {
                Some("completed") => {
                    note(e, "turn done");
                    e.error = None;
                }
                Some("interrupted") => {
                    let open =
                        e.run.as_mut().and_then(|run| run.rpc.as_mut()).map(|rpc| std::mem::take(&mut rpc.open_items)).unwrap_or_default();
                    for (_, what) in open.iter().filter(|(_, what)| what != "userMessage") {
                        note(e, &format!("did not finish, the turn interrupted: {what}"));
                    }
                    note(e, "turn interrupted");
                }
                status => {
                    if e.error.is_none() || !turn["error"].is_null() {
                        e.error = Some(match turn["error"].is_null() {
                            true => format!("the turn ended {}", status.unwrap_or("without a status")),
                            false => error_of(&turn["error"]),
                        });
                    }
                    note(e, &format!("turn {}", status.unwrap_or("ended")));
                }
            }
            crate::activity::touch();
            close(e, heard);
        }
        _ => {}
    }
}

/// A turn's error as the record reads it (activity/record.rs `Outcome`): a
/// usage limit as Luna words it, from `codexErrorInfo`, else the message.
fn error_of(error: &Value) -> String {
    let info = &error["codexErrorInfo"];
    let kind = info.as_str().or_else(|| info.as_object().and_then(|o| o.keys().next()).map(String::as_str));
    if let Some(word) = kind.map(snake).as_deref().and_then(crate::codex::session::limit_word) {
        return word.to_string();
    }
    error["message"].as_str().unwrap_or("error").to_string()
}

/// `usageLimitExceeded` as the rollout spells it, `usage_limit_exceeded`.
fn snake(camel: &str) -> String {
    let mut out = String::with_capacity(camel.len() + 4);
    for c in camel.chars() {
        if c.is_ascii_uppercase() {
            out.push('_');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// What a server request asks, as Luna holds it: an approval as a
/// permission, a question as a question. None for one Luna answers at once.
fn ask_of(method: &str, p: &Value, open: &[(String, String)]) -> Option<Ask> {
    let opened = |id: &Value| open.iter().find(|(i, _)| Some(i.as_str()) == id.as_str()).map(|(_, what)| what.clone());
    let with_reason = |detail: String| match p["reason"].as_str().filter(|r| !r.is_empty()) {
        Some(reason) => format!("{detail} ({reason})"),
        None => detail,
    };
    let ask = match method {
        "item/commandExecution/requestApproval" => {
            Ask::permission(Some("shell".into()), Some(with_reason(p["command"].as_str().unwrap_or("").to_string())))
        }
        "execCommandApproval" => {
            let command: Vec<&str> = p["command"].as_array().map(|c| c.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
            Ask::permission(Some("shell".into()), Some(with_reason(command.join(" "))))
        }
        "item/fileChange/requestApproval" => {
            let what = opened(&p["itemId"]).unwrap_or_else(|| "file change".into());
            let root = p["grantRoot"].as_str().map(|r| format!(", writes under {r} for the session")).unwrap_or_default();
            Ask::permission(Some("apply_patch".into()), Some(with_reason(format!("{what}{root}"))))
        }
        "applyPatchApproval" => {
            let paths: Vec<&String> = p["fileChanges"].as_object().map(|o| o.keys().collect()).unwrap_or_default();
            Ask::permission(
                Some("apply_patch".into()),
                Some(with_reason(format!("file change {}", paths.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")))),
            )
        }
        "item/permissions/requestApproval" => Ask::permission(Some("permissions".into()), Some(with_reason(p["permissions"].to_string()))),
        "item/tool/requestUserInput" => {
            let questions: Vec<crate::agents::AskQuestion> = p["questions"]
                .as_array()?
                .iter()
                .filter_map(|q| {
                    let text = q["question"].as_str()?.trim().to_string();
                    let options = q["options"]
                        .as_array()
                        .map(|o| o.iter().filter_map(|o| o["label"].as_str().map(str::to_owned)).collect())
                        .unwrap_or_default();
                    Some(crate::agents::AskQuestion { text, options, multi: false, list_at: None })
                })
                .collect();
            if questions.is_empty() {
                return None;
            }
            Ask::question(questions, p["isBlocking"].as_bool().unwrap_or(true))
        }
        _ => return None,
    };
    Some(ask.asked_by(Some(crate::agents::AskSource::session())))
}

/// A request of the server's. An approval or a question is held for an
/// answer, its serial returned for `reply_when_answered`; what else it asks
/// (an MCP server's elicitation, a dynamic tool, a token refresh, an
/// attestation) has nobody to answer it and is refused at once, as a
/// Claude Code helper refuses its CLI's own requests.
fn server_request(r: &mut Registry, id: &str, rid: &Value, method: &str, p: &Value) -> Option<u64> {
    let Registry { entries, next_serial, asked, .. } = r;
    let serial = *next_serial;
    let e = entries.get_mut(id)?;
    let open = e.run.as_ref().and_then(|run| run.rpc.as_ref()).map(|rpc| rpc.open_items.clone()).unwrap_or_default();
    let Some(mut ask) = ask_of(method, p, &open) else {
        let line = match method {
            "mcpServer/elicitation/request" => reply(rid, json!({ "action": "decline" })),
            _ => refusal(rid, &format!("a Luna helper has nobody to answer {method}")),
        };
        if send(e, line) {
            note(e, &format!("refused the server's request: {method}"));
        }
        return None;
    };
    ask.id = Some(format!("a{serial}"));
    note(e, &format!("waits: {}", ask.line()));
    let input = json!({ "id": rid, "method": method, "params": p });
    e.held.push_back(Held { serial, ask, input, tool_use_id: None, answer: None });
    *next_serial += 1;
    asked.insert(serial, id.to_string());
    Some(serial)
}

/// The reply to the server's request held as `serial`, from the answer
/// `ask.rs` `answer` built (Claude Code's shape: `behavior`, `updatedInput`);
/// None (no answer in time) as a refusal.
fn reply_to(method: &str, p: &Value, answer: Option<&Value>) -> Value {
    let allowed = answer.is_some_and(|a| a["behavior"] == "allow");
    match method {
        "item/tool/requestUserInput" => {
            let picked = answer.filter(|_| allowed).map(|a| &a["updatedInput"]["answers"]);
            let mut answers = serde_json::Map::new();
            for q in p["questions"].as_array().into_iter().flatten() {
                let (Some(qid), Some(text)) = (q["id"].as_str(), q["question"].as_str()) else { continue };
                if let Some(said) = picked.and_then(|a| a[text.trim()].as_str()) {
                    answers.insert(qid.to_string(), json!({ "answers": [said] }));
                }
            }
            json!({ "answers": answers })
        }
        "item/permissions/requestApproval" if allowed => json!({ "permissions": p["permissions"], "scope": "turn" }),
        "item/permissions/requestApproval" => json!({ "permissions": {} }),
        "execCommandApproval" | "applyPatchApproval" => json!({ "decision": if allowed { "approved" } else { "denied" } }),
        _ => json!({ "decision": if allowed { "accept" } else { "decline" } }),
    }
}

/// Holds the server's request `serial` of helper `id` until its caller
/// answers (`ask.rs` `answer`), the turn ends, or ASK_TIMEOUT passes, and
/// replies on the turn's stdin: unanswered, a refusal. A thread of its own,
/// as `luna_permission`'s wait is the MCP call's.
pub(super) fn reply_when_answered(id: String, serial: u64) {
    let of = crate::agents::label(&id);
    let spawned = std::thread::Builder::new().name(format!("luna-bg-ask-{id}")).spawn(move || {
        let deadline = std::time::Instant::now() + ASK_TIMEOUT;
        let (m, cv) = reg();
        let mut r = m.lock().unwrap_or_else(|e| e.into_inner());
        let answer = loop {
            let held = r.entries.get_mut(&id).filter(|e| e.run.is_some()).and_then(|e| e.held.iter_mut().find(|h| h.serial == serial));
            match held {
                Some(h) => {
                    if let Some(a) = h.answer.take() {
                        break Some(a);
                    }
                }
                // Stopped, deleted, or its turn is over: nothing to answer.
                None => return,
            }
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                break None;
            }
            r = cv.wait_timeout(r, left).unwrap_or_else(|e| e.into_inner()).0;
        };
        if let Some(e) = r.entries.get_mut(&id) {
            if let Some(at) = e.held.iter().position(|h| h.serial == serial) {
                let input = e.held.remove(at).map(|h| h.input).unwrap_or(Value::Null);
                let result = reply_to(input["method"].as_str().unwrap_or(""), &input["params"], answer.as_ref());
                if !send(e, reply(&input["id"], result)) {
                    note(e, "its turn went before the answer could reach it");
                }
            }
        }
        drop(r);
        changed();
    });
    if let Err(err) = spawned {
        crate::log::warn("background", &format!("cannot hold {of}'s ask for an answer: {err}"));
    }
}

/// The turn has completed and its stdin is closed: the server goes by
/// itself. Should it still be there after EXIT_GRACE, Luna ends it, and only
/// it: what the turn started stays the helper's (`turn_over`).
pub(super) fn see_out(id: &str) {
    let Some(pid) = lock().entries.get(id).and_then(|e| e.run.as_ref()).and_then(|run| run.pid) else { return };
    let of = crate::agents::label(id);
    let id = id.to_string();
    let spawned = std::thread::Builder::new().name(format!("luna-bg-exit-{id}")).spawn(move || {
        let deadline = std::time::Instant::now() + EXIT_GRACE;
        let (m, cv) = reg();
        let mut r = m.lock().unwrap_or_else(|e| e.into_inner());
        while r.entries.get(&id).and_then(|e| e.run.as_ref()).is_some_and(|run| run.pid == Some(pid)) {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                drop(r);
                crate::log::warn("background", &format!("{}: its app-server stayed after its turn; ending it", crate::agents::label(&id)));
                if let Some(started) = crate::procs::created_ms(pid) {
                    crate::procs::end(pid, started);
                }
                return;
            }
            r = cv.wait_timeout(r, left).unwrap_or_else(|e| e.into_inner()).0;
        }
    });
    if let Err(err) = spawned {
        crate::log::warn("background", &format!("cannot see {of}'s app-server out: {err}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixture's messages as `codex app-server` 0.169.0 sent and took them
    /// (test-build/appserver-probe, paths and secrets taken out): the
    /// server's, with the ids of its answers put in Luna's words for the
    /// request each answers, and the probe's own requests and replies, which
    /// Luna's lines are checked against.
    fn fixture(name: &str) -> Vec<(bool, Value)> {
        let text = match name {
            "turn" => include_str!("testdata/codex-0.169.0-turn.jsonl"),
            "steer" => include_str!("testdata/codex-0.169.0-steer.jsonl"),
            "interrupt" => include_str!("testdata/codex-0.169.0-interrupt.jsonl"),
            "approval-accept" => include_str!("testdata/codex-0.169.0-approval-accept.jsonl"),
            "approval-decline" => include_str!("testdata/codex-0.169.0-approval-decline.jsonl"),
            "question" => include_str!("testdata/codex-0.169.0-question.jsonl"),
            "resume" => include_str!("testdata/codex-0.169.0-resume.jsonl"),
            "failed" => include_str!("testdata/codex-0.169.0-failed.jsonl"),
            _ => unreachable!(),
        };
        let mut ids: HashMap<String, String> = HashMap::new();
        let mut out = Vec::new();
        for line in text.lines() {
            let m: Value = serde_json::from_str(line).unwrap();
            let mut msg = m["msg"].clone();
            let to_luna = m["dir"] == "<";
            if !to_luna {
                if let (Some(id), Some(method)) = (msg["id"].as_i64(), msg["method"].as_str()) {
                    let ours = match method {
                        "initialize" => INIT.to_string(),
                        "thread/start" | "thread/resume" => OPEN.to_string(),
                        "turn/start" => TURN.to_string(),
                        "turn/interrupt" => INTERRUPT.to_string(),
                        "turn/steer" => format!("{STEER}steered"),
                        other => other.to_string(),
                    };
                    ids.insert(id.to_string(), ours);
                }
            } else if msg.get("method").is_none() {
                let id = msg["id"].to_string();
                if let Some(ours) = ids.get(&id) {
                    msg["id"] = json!(ours);
                }
            }
            out.push((to_luna, msg));
        }
        out
    }

    /// A Codex helper in a turn whose process is up, its stdin a channel.
    fn running(id: &str, open: Value) -> std::sync::mpsc::Receiver<String> {
        let (feed, lines) = std::sync::mpsc::channel();
        let mut e = entry(Provider::Codex);
        e.run = Some(Run { feed: Some(feed), rpc: Some(Rpc::new(open, "the task".into(), "low".into())), ..Run::claimed(Some(1), 0) });
        lock().entries.insert(id.into(), e);
        lines
    }

    fn hear(id: &str, v: &Value) -> Heard {
        on_message(&mut lock(), id, v)
    }

    /// Plays the server's side of a fixture into `id`, answering each of its
    /// requests with `answer` as a caller would; returns what Luna wrote.
    fn play(
        id: &str,
        messages: &[(bool, Value)],
        lines: &std::sync::mpsc::Receiver<String>,
        answer: impl Fn(&Ask) -> (Vec<String>, Option<bool>),
    ) -> Vec<Value> {
        let mut wrote = Vec::new();
        for (to_luna, msg) in messages.iter().filter(|(to_luna, _)| *to_luna) {
            let _ = to_luna;
            let heard = hear(id, msg);
            for serial in heard.asks {
                let ask = lock().entries[id].held.iter().find(|h| h.serial == serial).unwrap().ask.clone();
                let (answers, allow) = answer(&ask);
                super::super::answer(id, ask.id.as_deref(), &answers, allow, None, |_| Ok(())).unwrap();
                reply_when_answered(id.to_string(), serial);
                let line = lines.recv_timeout(std::time::Duration::from_secs(5)).expect("the reply goes out");
                wrote.push(serde_json::from_str(&line).unwrap());
            }
            while let Ok(line) = lines.try_recv() {
                wrote.push(serde_json::from_str(&line).unwrap());
            }
        }
        wrote
    }

    fn methods(wrote: &[Value]) -> Vec<String> {
        wrote.iter().map(|v| v["method"].as_str().map_or_else(|| format!("reply {}", v["id"]), str::to_owned)).collect()
    }

    #[test]
    fn a_first_turn_opens_its_thread_starts_the_turn_and_closes_stdin_at_its_end() {
        let id = "as-turn";
        let open = json!({ "method": "thread/start", "params": { "cwd": "C:\\work", "developerInstructions": "role" } });
        let lines = running(id, open);
        lock().entries.get_mut(id).unwrap().in_flight = vec!["the task".into()];
        let wrote = play(id, &fixture("turn"), &lines, |_| unreachable!());
        assert_eq!(methods(&wrote), ["initialized", "thread/start", "turn/start"]);
        assert_eq!(wrote[1]["params"]["developerInstructions"], "role");
        assert_eq!(wrote[2]["params"]["threadId"], "01a1f5d9-3600-7113-b9cb-fea7896d9978");
        assert_eq!(wrote[2]["params"]["input"][0]["text"], "the task");
        assert_eq!(wrote[2]["params"]["effort"], "low");
        let r = lock();
        let e = &r.entries[id];
        assert_eq!(e.session.as_deref(), Some("01a1f5d9-3600-7113-b9cb-fea7896d9978"));
        assert_eq!(e.observed.model, None, "what it ran on is the rollout's to say once its model answered");
        assert!(e.in_flight.is_empty(), "turn/start's answer: the server has it");
        assert!(e.error.is_none() && e.observed.acted);
        let run = e.run.as_ref().unwrap();
        assert!(run.feed.is_none() && run.rpc.as_ref().unwrap().done, "turn/completed closes stdin");
        assert!(e.log.iter().any(|l| l == "said: PROBE-OK"), "{:?}", e.log);
        drop(r);
        lock().entries.remove(id);
    }

    #[test]
    fn a_resumed_thread_goes_on_from_its_id() {
        let id = "as-resume";
        let lines = running(id, json!({ "method": "thread/resume", "params": { "threadId": "t" } }));
        let wrote = play(id, &fixture("resume"), &lines, |_| unreachable!());
        assert_eq!(methods(&wrote), ["initialized", "thread/resume", "turn/start"]);
        assert!(lock().entries[id].error.is_none());
        lock().entries.remove(id);
    }

    #[test]
    fn text_sent_mid_turn_steers_it_and_is_delivered_when_the_server_takes_it() {
        let id = "as-steer";
        let lines = running(id, Value::Null);
        let messages = fixture("steer");
        // The turn has started (its answer is the fixture's first).
        hear(id, &messages.iter().find(|(to, m)| *to && m["id"] == TURN).unwrap().1);
        {
            let mut r = lock();
            let e = r.entries.get_mut(id).unwrap();
            assert_eq!(e.run.as_ref().unwrap().rpc.as_ref().unwrap().steerable(), Some("01a1f5d9-577b-7170-bfa0-0fb1888296cf"));
            e.session = Some("01a1f5d9-3600-7113-b9cb-fea7896d9978".into());
            let q = Queued { id: "steered".into(), ..Queued::from("PINEAPPLE") };
            assert!(write_line(e, q).is_ok());
            assert_eq!(e.fed, ["PINEAPPLE"]);
        }
        let line: Value = serde_json::from_str(&lines.try_recv().unwrap()).unwrap();
        assert_eq!((line["method"].as_str(), line["id"].as_str()), (Some("turn/steer"), Some("steer:steered")));
        assert_eq!(line["params"]["expectedTurnId"], "01a1f5d9-577b-7170-bfa0-0fb1888296cf");
        let rest: Vec<(bool, Value)> = messages.into_iter().filter(|(_, m)| m["id"] != TURN).collect();
        play(id, &rest, &lines, |_| unreachable!());
        let r = lock();
        let e = &r.entries[id];
        assert!(e.fed.is_empty(), "the server's answer: taken in");
        assert!(e.log.iter().any(|l| l == "said: slept PINEAPPLE"), "{:?}", e.log);
        drop(r);
        lock().entries.remove(id);
    }

    #[test]
    fn a_steer_that_missed_its_turn_stays_for_the_next() {
        let id = "as-steer-missed";
        let _lines = running(id, Value::Null);
        lock().entries.get_mut(id).unwrap().fed = vec![Queued { id: "late".into(), line: Some("late".into()), ..Queued::from("late") }];
        hear(id, &json!({ "id": "steer:late", "error": { "code": -32600, "message": "no active turn to steer" } }));
        let mut r = lock();
        let e = r.entries.get_mut(id).unwrap();
        assert_eq!(e.fed, ["late"]);
        assert_eq!(sort_out_untaken(e), ["late"], "it starts the next turn");
        drop(r);
        lock().entries.remove(id);
    }

    #[test]
    fn an_interrupted_turn_closes_what_it_left_open() {
        let id = "as-interrupt";
        let lines = running(id, Value::Null);
        lock().entries.get_mut(id).unwrap().session = Some("t".into());
        let messages = fixture("interrupt");
        let (before, after): (Vec<_>, Vec<_>) = {
            let at = messages.iter().position(|(to, m)| !to && m["method"] == "turn/interrupt").unwrap();
            (messages[..at].to_vec(), messages[at..].to_vec())
        };
        // Its first two are a steer of the turn before: not this one's.
        play(id, &before[2..], &lines, |_| unreachable!());
        assert!(ask_to_stop(lock().entries.get_mut(id).unwrap()));
        let line: Value = serde_json::from_str(&lines.try_recv().unwrap()).unwrap();
        assert_eq!(
            (line["method"].as_str(), line["params"]["turnId"].as_str()),
            (Some("turn/interrupt"), Some("01a1f5d9-7c0c-7272-8a18-73b22379a79f"))
        );
        assert!(lock().entries[id].run.as_ref().unwrap().rpc.as_ref().unwrap().steerable().is_none(), "no steer into a turn being stopped");
        play(id, &after, &lines, |_| unreachable!());
        let r = lock();
        let e = &r.entries[id];
        assert!(e.log.iter().any(|l| l.starts_with("did not finish, the turn interrupted: command ")), "{:?}", e.log);
        assert!(e.run.as_ref().unwrap().feed.is_none(), "stdin closed: the server goes");
        assert!(e.error.is_none(), "an interrupt is no failure of the CLI's");
        drop(r);
        lock().entries.remove(id);
    }

    #[test]
    fn an_approval_is_held_for_the_caller_and_its_answer_goes_back_to_the_server() {
        for (name, allow, decision) in [("approval-accept", true, "accept"), ("approval-decline", false, "decline")] {
            let id = format!("as-{name}");
            let lines = running(&id, json!({ "method": "thread/start", "params": {} }));
            let messages = fixture(name);
            let wrote = play(&id, &messages, &lines, |ask| {
                assert_eq!((ask.kind, ask.tool.as_deref()), ("permission", Some("shell")));
                assert!(ask.detail.as_deref().unwrap().contains("Set-Content"), "{ask:?}");
                (Vec::new(), Some(allow))
            });
            let replied = wrote.iter().find(|v| v.get("method").is_none()).expect("a reply");
            assert_eq!(replied["result"]["decision"], decision, "{name}");
            // The probe's own reply to the same request, for the same id.
            let probe = messages.iter().find(|(to, m)| !to && m.get("method").is_none()).unwrap();
            assert_eq!(replied["id"], probe.1["id"]);
            assert!(lock().entries[&id].held.is_empty());
            lock().entries.remove(&id);
        }
    }

    #[test]
    fn a_question_is_held_and_answered_by_its_id() {
        let id = "as-question";
        let lines = running(id, json!({ "method": "thread/start", "params": {} }));
        let wrote = play(id, &fixture("question"), &lines, |ask| {
            assert_eq!(ask.kind, "question");
            assert_eq!(ask.questions[0].options, ["Red", "Blue"]);
            (vec!["2".into()], None)
        });
        let replied = wrote.iter().find(|v| v.get("method").is_none()).unwrap();
        assert_eq!(replied["result"], json!({ "answers": { "preferred_color": { "answers": ["Blue"] } } }));
        lock().entries.remove(id);
    }

    #[test]
    fn a_failed_turn_says_why_and_a_usage_limit_reads_as_one() {
        let id = "as-failed";
        let lines = running(id, json!({ "method": "thread/start", "params": {} }));
        play(id, &fixture("failed"), &lines, |_| unreachable!());
        let error = lock().entries[id].error.clone();
        assert_eq!(error.as_deref(), Some("The 'no-such-model-xyz' model is not supported when using Codex with a ChatGPT account."));
        assert_eq!(crate::activity::Outcome::of_error(error.as_deref()), crate::activity::Outcome::Failed);
        // In the schema of 0.169.0, not provoked live.
        for (info, word) in [(json!("usageLimitExceeded"), "usage_limit"), (json!({ "rateLimitExceeded": {} }), "rate_limit")] {
            assert_eq!(error_of(&json!({ "message": "You've hit your usage limit.", "codexErrorInfo": info })), word);
            assert_eq!(crate::activity::Outcome::of_error(Some(word)), crate::activity::Outcome::UsageLimit);
        }
        hear(id, &json!({ "method": "error", "params": { "error": { "message": "Reconnecting... 1/5" }, "willRetry": true } }));
        lock().entries.get_mut(id).unwrap().error = None;
        hear(id, &json!({ "method": "error", "params": { "error": { "message": "x" }, "willRetry": true } }));
        assert!(lock().entries[id].error.is_none(), "a retry is no failure");
        lock().entries.remove(id);
    }

    #[test]
    fn requests_nobody_can_answer_are_refused_at_once() {
        let id = "as-refuse";
        let lines = running(id, Value::Null);
        for (method, want) in [
            ("mcpServer/elicitation/request", json!({ "action": "decline" })),
            ("account/chatgptAuthTokens/refresh", Value::Null),
            ("item/tool/call", Value::Null),
        ] {
            let heard = hear(id, &json!({ "id": 7, "method": method, "params": {} }));
            assert!(heard.asks.is_empty());
            let line: Value = serde_json::from_str(&lines.try_recv().unwrap()).unwrap();
            assert_eq!(line["id"], 7);
            match want {
                Value::Null => assert_eq!(line["error"]["code"], -32601, "{method}"),
                want => assert_eq!(line["result"], want),
            }
        }
        lock().entries.remove(id);
    }

    #[test]
    fn an_ask_its_turn_outlives_unanswered_gets_no_reply() {
        let id = "as-gone";
        let lines = running(id, Value::Null);
        let heard = hear(id, &json!({ "id": 0, "method": "item/commandExecution/requestApproval", "params": { "command": "x" } }));
        let serial = heard.asks[0];
        lock().entries.get_mut(id).unwrap().run = None;
        reply_when_answered(id.to_string(), serial);
        assert!(lines.recv_timeout(std::time::Duration::from_millis(300)).is_err());
        lock().entries.remove(id);
    }
}
