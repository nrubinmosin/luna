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
//! Code helper's control requests are, for its caller to answer with
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
/// How long a server has to answer `turn/start` once started: the first
/// thread of a process opened in 5.1 s on 0.169.0, its MCP servers starting.
/// A turn once started is never ended by the clock: one long command can keep
/// it quiet for hours, so its parent hears of the silence instead (agents/
/// notifier `QUIET_TURN_MS`, off `turn_quiet_ms`, which every message of the
/// server moves) and decides to interrupt or delete it.
const HANDSHAKE_MS: u64 = 3 * 60 * 1000;
/// How often a turn is looked at for that (`watch`).
const WATCH_EVERY: std::time::Duration = std::time::Duration::from_secs(5);

/// A Codex turn's side of its conversation with its server.
pub(super) struct Rpc {
    /// `thread/start` or `thread/resume` with its params: what follows the
    /// handshake (command.rs `command_and_open`).
    open: Value,
    /// The turn's text and effort, for `turn/start` once the thread is there.
    input: String,
    effort: String,
    /// What `turn/start` names its message (`clientUserMessageId`): the id
    /// of the line its batch went as (`Queued::line`), which the rollout keeps
    /// as the message's `client_id`: what a core taking the helper up settles
    /// its fate by (persist.rs `settle_sent`).
    client_id: String,
    /// `turn/start` as it is to go, held until the text it carries is saved as
    /// sent (`send_turn`).
    pending: Option<Value>,
    /// The thread the server opened: the helper's conversation only once
    /// `turn/start` is answered, as Codex writes a thread at its first turn
    /// ("no rollout found" for one never given a turn).
    pub(super) thread: Option<String>,
    /// The turn's id, once `turn/start` has answered: what a steer and an
    /// interrupt name.
    pub(super) turn: Option<String>,
    /// Items begun and not completed, by id, with what each was: an
    /// interrupted turn's get no `item/completed` (0.169.0), so Luna closes
    /// them in its log.
    pub(super) open_items: Vec<(String, String)>,
    /// `turn/interrupt` has gone out.
    pub(super) interrupting: bool,
    /// `turn/completed` came, or the turn cannot go on: stdin is closed and
    /// the server goes.
    pub(super) done: bool,
    /// Why the server broke the protocol (a request of Luna's refused, a
    /// thread other than the one asked for, a turn that never ended): its
    /// version's failure for the CLI canary, whatever it exits with.
    pub(super) broken: Option<String>,
}

impl Rpc {
    pub(super) fn new(open: Value, input: String, effort: String, client_id: String) -> Rpc {
        Rpc {
            open,
            input,
            effort,
            client_id,
            pending: None,
            thread: None,
            turn: None,
            open_items: Vec::new(),
            interrupting: false,
            done: false,
            broken: None,
        }
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
/// server's answer gives back, and so does the `userMessage` item it becomes
/// (`clientId`), which is when the model has it (`took`).
pub(super) fn steer_line(thread: &str, turn: &str, q: &Queued) -> String {
    let params = json!({ "threadId": thread, "expectedTurnId": turn, "clientUserMessageId": q.id, "input": input(&q.text) });
    request(&format!("{STEER}{}", q.id), "turn/steer", params)
}

/// Asks the turn running to stop, if it has an id and is not over: true if
/// asked. Its `turn/completed` ends it as any turn's end does; what it had
/// begun and not finished is closed then.
pub(super) fn ask_server_to_stop(e: &mut Entry) -> bool {
    let Some(run) = e.run.as_mut() else { return false };
    let Some(rpc) = run.rpc.as_mut() else { return false };
    let (Some(thread), Some(turn), false) = (rpc.thread.clone(), rpc.turn.clone(), rpc.done) else { return false };
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
    /// An ask went without an answer (`serverRequest/resolved`): the rows change.
    pub(super) settled: bool,
    /// The turn has completed and stdin is closed: the server is to go
    /// (`see_out`).
    pub(super) ended: bool,
    /// The thread is open and `turn/start` waits for its text to be saved as
    /// sent (`send_turn`).
    pub(super) turn_ready: bool,
    /// `turn/start` was answered: its start is through (the CLI canary).
    pub(super) turn_started: bool,
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

fn rpc_of(e: &Entry) -> Option<&Rpc> {
    e.run.as_ref()?.rpc.as_ref()
}

fn rpc_mut(e: &mut Entry) -> Option<&mut Rpc> {
    e.run.as_mut()?.rpc.as_mut()
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

/// The turn cannot go on for `why`, a break of the protocol: its error, and
/// a failure of its CLI's version for the canary (`went_wrong`).
fn broke(e: &mut Entry, why: String, heard: &mut Heard) {
    let why = without_excerpts(&why);
    note(e, &format!("error: {}", one_line(&why)));
    if let Some(rpc) = rpc_mut(e) {
        rpc.broken = Some(why.clone());
    }
    e.error = Some(why);
    close(e, heard);
}

/// Codex's diagnostic without the source lines it quotes: a config that
/// will not parse comes back as its file, line and column and what is wrong
/// there, then the line itself in a gutter ("2 | key = value", with "|" and
/// caret lines about it; 0.171.0, its TOML errors). The file may be a link
/// to one that is no config, a linked worktree's main checkout's say, and
/// what the turn failed for goes to its parent and the log: the gutter
/// lines go, the rest stays.
fn without_excerpts(why: &str) -> String {
    let quoted = |line: &str| {
        let rest = line.trim_start().trim_start_matches(|c: char| c.is_ascii_digit());
        rest.trim_start().starts_with('|')
    };
    why.lines().filter(|line| !quoted(line)).collect::<Vec<_>>().join("\n")
}

/// A request of Luna's refused: the turn cannot go on.
fn failed(e: &mut Entry, what: &str, v: &Value, heard: &mut Heard) {
    let why = v["error"]["message"].as_str().unwrap_or("refused");
    broke(e, format!("{what}: {why}"), heard);
}

/// The thread asked for is not to be had (never given a turn, so never
/// written: "no rollout found", or another came back in its place): a new
/// one is started, given this turn's text and whatever it carries
/// undelivered. Once only: a new thread refused fails the turn.
fn start_fresh(e: &mut Entry, why: &str, heard: &mut Heard) {
    if !rpc_of(e).is_some_and(|rpc| rpc.open["method"] == "thread/resume") {
        return broke(e, format!("the thread would not open: {why}"), heard);
    }
    let Some(rpc) = rpc_mut(e) else { return };
    rpc.open["method"] = json!("thread/start");
    if let Some(params) = rpc.open["params"].as_object_mut() {
        params.remove("threadId");
    }
    let params = rpc.open["params"].clone();
    note(e, &format!("its thread cannot be resumed ({}): a new one is started", one_line(why)));
    if !send(e, request(OPEN, "thread/start", params)) {
        close(e, heard);
    }
}

/// Whether the error of a `thread/resume` says the thread is not there:
/// 0.169.0's words for one never given a turn or unknown.
fn no_such_thread(message: &str) -> bool {
    message.contains("no rollout found")
}

/// Where the thread runs otherwise than its request named: an approval or a
/// sandbox Luna passed, typed, and the server's answer says another (a
/// thread's saved settings win over `-c`, not over these). The turn is not
/// started then: fail closed.
fn settings_differ(asked: &Value, opened: &Value) -> Option<String> {
    if let Some(want) = asked["approvalPolicy"].as_str() {
        let got = &opened["approvalPolicy"];
        if got.as_str() != Some(want) {
            return Some(format!("its thread runs with approval {got}, not the {want} it was started with"));
        }
    }
    if let Some(want) = asked["approvalsReviewer"].as_str() {
        let got = &opened["approvalsReviewer"];
        if got.as_str() != Some(want) {
            return Some(format!("its thread's approvals go to {got}, not the {want} it was started with"));
        }
    }
    if let Some(want) = asked["permissions"].as_str() {
        let got = &opened["activePermissionProfile"]["id"];
        if got.as_str() != Some(want) {
            return Some(format!("its thread runs with permission profile {got}, not the {want} it was started with"));
        }
    }
    if let Some(want) = asked["sandbox"].as_str() {
        let camel = |s: &str| {
            let mut parts = s.split('-');
            let head = parts.next().unwrap_or_default().to_string();
            parts.fold(head, |acc, p| acc + &p[..1].to_uppercase() + &p[1..])
        };
        let got = &opened["sandbox"]["type"];
        if got.as_str() != Some(camel(want).as_str()) {
            return Some(format!("its thread runs in sandbox {got}, not the {want} it was started with"));
        }
    }
    None
}

/// The server's answer to a request of Luna's.
fn answered(e: &mut Entry, rid: &str, v: &Value, heard: &mut Heard) {
    let ok = v.get("error").is_none();
    match rid {
        INIT if !ok => failed(e, "initialize", v, heard),
        INIT => {
            let open = rpc_of(e).map(|rpc| rpc.open.clone()).unwrap_or(Value::Null);
            let method = open["method"].as_str().unwrap_or("thread/start").to_string();
            let sent = send(e, notification("initialized")) && send(e, request(OPEN, &method, open["params"].clone()));
            if !sent {
                close(e, heard);
            }
        }
        OPEN if !ok => {
            let resuming = rpc_of(e).is_some_and(|rpc| rpc.open["method"] == "thread/resume");
            let why = v["error"]["message"].as_str().unwrap_or("refused").to_string();
            match resuming && no_such_thread(&why) {
                true => start_fresh(e, &why, heard),
                false => failed(e, "the thread would not open", v, heard),
            }
        }
        OPEN => {
            let r = &v["result"];
            let Some(open) = rpc_of(e).map(|rpc| rpc.open.clone()) else { return };
            let Some(thread) = r["thread"]["id"].as_str().map(str::to_owned) else {
                return broke(e, "the thread opened with no id".into(), heard);
            };
            let resuming = open["method"] == "thread/resume";
            if resuming && open["params"]["threadId"].as_str() != Some(thread.as_str()) {
                return start_fresh(e, &format!("thread {thread} came back for {}", open["params"]["threadId"]), heard);
            }
            note(
                e,
                &format!(
                    "thread {thread}: approval {}, sandbox {}",
                    r["approvalPolicy"].as_str().unwrap_or("?"),
                    r["sandbox"]["type"].as_str().unwrap_or("?")
                ),
            );
            if let Some(why) = settings_differ(&open["params"], r) {
                return broke(e, why, heard);
            }
            if let Some(rpc) = rpc_mut(e) {
                rpc.thread = Some(thread.clone());
            }
            let Some(rpc) = rpc_mut(e) else { return };
            let mut params = json!({ "threadId": thread, "clientUserMessageId": rpc.client_id, "input": input(&rpc.input) });
            if !rpc.effort.is_empty() {
                params["effort"] = json!(rpc.effort);
            }
            rpc.pending = Some(params);
            heard.turn_ready = true;
        }
        TURN if !ok => failed(e, "the turn would not start", v, heard),
        TURN => {
            let Some(turn) = v["result"]["turn"]["id"].as_str().map(str::to_owned) else {
                return broke(e, "the turn started with no id".into(), heard);
            };
            let thread = rpc_mut(e).and_then(|rpc| {
                rpc.turn = Some(turn);
                rpc.thread.clone()
            });
            // From here the thread is written: it is the helper's
            // conversation, and the server has the turn's text, not to be
            // handed back.
            if let Some(thread) = thread.filter(|t| e.session.as_ref() != Some(t)) {
                e.session = Some(thread);
                mark_by_thread(e);
            }
            heard.landed = stdin_took(e) || heard.landed;
            heard.turn_started = true;
        }
        INTERRUPT if !ok => note(e, &format!("turn/interrupt refused: {}", one_line(v["error"]["message"].as_str().unwrap_or("")))),
        INTERRUPT => {}
        steer if steer.starts_with(STEER) => {
            if ok {
                // Taken by the server, and the model's once its `userMessage`
                // item comes (`notified`): until then an interrupt can drop it.
                note(e, "the turn took a steer");
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

/// Sends the `turn/start` an open thread waits with (`Heard::turn_ready`),
/// once the text it carries is saved as sent: each of the batch's texts, by
/// the line it goes as (`Queued::sent`), and a first turn's opening, which
/// `opening` keeps. A core gone from here on hands them back as
/// undelivered unless the rollout shows they arrived (persist.rs
/// `settle_sent`); never sent again by itself.
pub(super) fn send_turn(id: &str) {
    if let Some(e) = lock().entries.get_mut(id) {
        for q in &mut e.in_flight {
            q.sent = true;
        }
    }
    save();
    let mut r = lock();
    let Some(e) = r.entries.get_mut(id) else { return };
    let Some(params) = rpc_mut(e).and_then(|rpc| rpc.pending.take()) else { return };
    if !send(e, request(TURN, "turn/start", params)) {
        let mut heard = Heard::default();
        close(e, &mut heard);
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
            // A steer is the model's once it is a message of the turn.
            if let Some(client) = item["clientId"].as_str().filter(|_| item["type"] == "userMessage") {
                heard.landed |= took(e, client);
            }
            if let (Some(id), Some(rpc)) = (item["id"].as_str(), rpc_mut(e)) {
                rpc.open_items.push((id.to_string(), item_label(item)));
            }
        }
        "item/completed" => {
            let item = &p["item"];
            if let (Some(id), Some(rpc)) = (item["id"].as_str(), rpc_mut(e)) {
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
        // The server settled a request itself (an interrupt, a `cancel`, its
        // own timeout): its ask goes, and an answer to it is refused as one
        // no longer open (ask.rs `answer`).
        "serverRequest/resolved" => {
            let rid = &p["requestId"];
            let before = e.held.len();
            e.held.retain(|h| h.answer.is_some() || &h.input["id"] != rid || h.input.get("method").is_none());
            if e.held.len() != before {
                note(e, &format!("the server settled request {rid} itself: its ask is closed"));
                heard.settled = true;
            }
        }
        // An error the server retries is no failure: the turn goes on.
        "error" if p["willRetry"].as_bool() == Some(true) => {
            note(e, &format!("retrying: {}", one_line(p["error"]["message"].as_str().unwrap_or(""))));
        }
        "error" => {
            let why = error_of(&p["error"]);
            note(e, &format!("error: {}", one_line(p["error"]["message"].as_str().unwrap_or(&why))));
            // A turn Luna stopped ends on its stop, whatever the server says.
            if !stopped(e) {
                e.error = Some(why);
            }
        }
        "turn/completed" => {
            let turn = &p["turn"];
            let ours = rpc_of(e).and_then(|rpc| rpc.turn.as_deref());
            if ours.is_some_and(|ours| turn["id"].as_str() != Some(ours)) {
                return;
            }
            match turn["status"].as_str() {
                Some("completed") => {
                    note(e, "turn done");
                    if !stopped(e) {
                        e.error = None;
                    }
                }
                Some("interrupted") => {
                    let open = rpc_mut(e).map(|rpc| std::mem::take(&mut rpc.open_items)).unwrap_or_default();
                    for (_, what) in open.iter().filter(|(_, what)| what != "userMessage") {
                        note(e, &format!("did not finish, the turn interrupted: {what}"));
                    }
                    // Steers the server took and the model never read: the
                    // interrupt dropped them (Codex clears its pending input),
                    // and they go back to their senders.
                    let dropped = std::mem::take(&mut e.fed);
                    if !dropped.is_empty() {
                        note(
                            e,
                            &format!("the interrupt dropped {} steer(s) the model had not read: kept for their senders", dropped.len()),
                        );
                        e.undelivered.extend(dropped);
                    }
                    note(e, "turn interrupted");
                }
                status => {
                    if !stopped(e) && (e.error.is_none() || !turn["error"].is_null()) {
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
/// usage limit as Luna words it, from `codexErrorInfo` or, where that names
/// none, from the message's own words (a spend cap, a workspace out of
/// credits: codex::session `limit_of_message`); else the message.
fn error_of(error: &Value) -> String {
    let info = &error["codexErrorInfo"];
    let kind = info.as_str().or_else(|| info.as_object().and_then(|o| o.keys().next()).map(String::as_str));
    let message = error["message"].as_str().unwrap_or("error");
    let word = kind.map(snake).as_deref().and_then(crate::codex::session::limit_word);
    match word.or_else(|| crate::codex::session::limit_of_message(message)) {
        Some(word) => word.to_string(),
        None => message.to_string(),
    }
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
                    Some(crate::agents::AskQuestion {
                        text,
                        options,
                        multi: false,
                        list_at: None,
                        own_words: q["isOther"].as_bool().unwrap_or(false),
                        secret: q["isSecret"].as_bool().unwrap_or(false),
                    })
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
    let open = rpc_of(e).map(|rpc| rpc.open_items.clone()).unwrap_or_default();
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
    e.held.push_back(Held { serial, ask, input, tool_use_id: None, answer: None, reply: None });
    *next_serial += 1;
    asked.insert(serial, id.to_string());
    Some(serial)
}

/// The reply to the server's request held as `serial`, from the answer
/// `ask.rs` `answer` built (`behavior`, and for a question `picked`, one
/// answer a question in their order); None (no answer in time) as a refusal.
/// A question's answers go by its id, never by its text.
fn reply_to(method: &str, p: &Value, answer: Option<&Value>) -> Value {
    let allowed = answer.is_some_and(|a| a["behavior"] == "allow");
    match method {
        "item/tool/requestUserInput" => {
            let picked = answer.filter(|_| allowed).and_then(|a| a["picked"].as_array());
            let mut answers = serde_json::Map::new();
            let asked = p["questions"].as_array().into_iter().flatten().filter(|q| q["question"].is_string());
            for (q, said) in asked.zip(picked.into_iter().flatten()) {
                if let (Some(qid), Some(said)) = (q["id"].as_str(), said.as_str()) {
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
/// as the ask is no call of Luna's to hold.
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
        if over_within(&id, pid, EXIT_GRACE) {
            return;
        }
        crate::log::warn("background", &format!("{}: its app-server stayed after its turn; ending it", crate::agents::label(&id)));
        end_server(&id, pid);
    });
    if let Err(err) = spawned {
        crate::log::warn("background", &format!("cannot see {of}'s app-server out: {err}"));
    }
}

/// Ends a turn's server, and only it, while its run still names it: its
/// exit thread has not reaped it, so the pid is still its own.
fn end_server(id: &str, pid: u32) {
    let r = lock();
    if !r.entries.get(id).and_then(|e| e.run.as_ref()).is_some_and(|run| run.pid == Some(pid) && !run.exited) {
        return;
    }
    #[cfg(windows)]
    if let Some(started) = crate::procs::created_ms(pid) {
        crate::procs::end(pid, started);
    }
    // No creation time to check it by here: the pid is still the server's
    // while its run names it and its exit thread has not reaped it
    // (`Run::exited`, marked under this lock just after the reap).
    #[cfg(unix)]
    // SAFETY: a signal to one pid, no memory involved.
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGKILL);
    }
    drop(r);
}

/// Why a Codex turn running now should be given up on, if it should: a
/// server that has not answered `turn/start` within HANDSHAKE_MS. Nothing
/// else: a turn that has started runs as long as it runs.
pub(super) fn stalled(e: &Entry, now: u64) -> Option<String> {
    let run = e.run.as_ref()?;
    let rpc = run.rpc.as_ref().filter(|rpc| !rpc.done)?;
    (rpc.turn.is_none() && now.saturating_sub(run.started_ms) > HANDSHAKE_MS)
        .then(|| format!("its app-server did not start the turn within {} minutes", HANDSHAKE_MS / 60_000))
}

/// Watches a Codex turn's process `pid` until its turn has started or it is
/// over, and gives up on a server that `stalled`: failed, asked to stop,
/// killed if it will not.
pub(super) fn watch(id: &str, pid: u32) {
    let of = crate::agents::label(id);
    let id = id.to_string();
    let spawned = std::thread::Builder::new().name(format!("luna-bg-watch-{id}")).spawn(move || loop {
        if over_within(&id, pid, WATCH_EVERY) {
            return;
        }
        // Started: from here only its parent ends it.
        if lock().entries.get(&id).and_then(|e| e.run.as_ref()).and_then(|run| run.rpc.as_ref()).is_some_and(|rpc| rpc.turn.is_some()) {
            return;
        }
        let asked = {
            let mut r = lock();
            let Some(e) = r.entries.get_mut(&id).filter(|e| e.run.as_ref().is_some_and(|run| run.pid == Some(pid))) else { return };
            let Some(why) = stalled(e, crate::throttle::now_ms()) else { continue };
            crate::log::warn("background", &format!("{}: {why}; ending the turn as failed", crate::agents::label(&id)));
            note(e, &format!("error: {why}"));
            e.error = Some(why.clone());
            if let Some(run) = e.run.as_mut() {
                run.luna_stopped = true;
                if let Some(rpc) = run.rpc.as_mut() {
                    rpc.broken = Some(why);
                }
            }
            ask_server_to_stop(e)
        };
        if !(asked && over_within(&id, pid, INTERRUPT_GRACE)) {
            crate::pty::kill_tree(Some(pid));
        }
        return;
    });
    if let Err(err) = spawned {
        crate::log::warn("background", &format!("cannot watch {of}'s turn: {err}"));
    }
}

/// As its process goes: whether a Codex turn broke the protocol, and why (a
/// request refused, a turn that never completed while Luna did not stop
/// it), its error set if none was. What the canary counts as a failed
/// session of its version, whatever the server exited with.
pub(super) fn went_wrong(id: &str) -> Option<String> {
    let mut r = lock();
    let e = r.entries.get_mut(id)?;
    let run = e.run.as_ref()?;
    let rpc = run.rpc.as_ref()?;
    let why = match (&rpc.broken, rpc.done || run.luna_stopped) {
        (Some(why), _) => why.clone(),
        (None, true) => return None,
        (None, false) => "its app-server went before its turn completed".to_string(),
    };
    if e.error.is_none() {
        note(e, &format!("error: {why}"));
        e.error = Some(why.clone());
    }
    Some(why)
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
        let rpc = Rpc::new(open, "the task".into(), "low".into(), "client-1".into());
        e.run = Some(Run { feed: Some(feed), rpc: Some(rpc), ..Run::claimed(Some(1), 0) });
        lock().entries.insert(id.into(), e);
        lines
    }

    /// One message into `id`, and what its reader does after: a turn ready
    /// to start goes once its text is saved as sent.
    fn hear(id: &str, v: &Value) -> Heard {
        let heard = on_message(&mut lock(), id, v);
        if heard.turn_ready {
            send_turn(id);
        }
        heard
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
        assert_eq!(wrote[2]["params"]["clientUserMessageId"], "client-1", "what a resumed thread names it by");
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
        let thread = "01a1f5d9-3600-7113-b9cb-fea7896d9978";
        let lines = running(id, json!({ "method": "thread/resume", "params": { "threadId": thread } }));
        let wrote = play(id, &fixture("resume"), &lines, |_| unreachable!());
        assert_eq!(methods(&wrote), ["initialized", "thread/resume", "turn/start"]);
        assert!(lock().entries[id].error.is_none());
        assert_eq!(lock().entries[id].session.as_deref(), Some(thread));
        lock().entries.remove(id);
    }

    /// Codex writes a thread at its first turn: one opened and never given a
    /// turn is not the helper's conversation, whatever stops it meanwhile (a
    /// core's death, a stop, a refused `turn/start`), and its opening prompt
    /// stays as what it is to start on.
    #[test]
    fn a_thread_is_the_helpers_only_once_its_first_turn_has_started() {
        let id = "as-materialize";
        let lines = running(id, json!({ "method": "thread/start", "params": {} }));
        lock().entries.get_mut(id).unwrap().opening = Some("the task".into());
        hear(id, &json!({ "id": OPEN, "result": { "thread": { "id": "t-new" } } }));
        {
            let r = lock();
            let e = &r.entries[id];
            assert_eq!(e.session, None, "opened, not yet written");
            assert_eq!(e.opening.as_deref(), Some("the task"), "what a core taking it up starts on");
        }
        let started: Value = serde_json::from_str(&lines.try_recv().unwrap()).unwrap();
        assert_eq!(started["method"], "turn/start");
        hear(id, &json!({ "id": TURN, "error": { "code": -32600, "message": "no" } }));
        let r = lock();
        let e = &r.entries[id];
        assert_eq!(e.session, None, "a refused turn wrote nothing: the next turn starts a thread afresh");
        assert!(e.error.as_deref().is_some_and(|w| w.contains("would not start")));
        assert!(e.run.as_ref().unwrap().rpc.as_ref().unwrap().broken.is_some(), "the canary's failure");
        drop(r);
        lock().entries.remove(id);
    }

    /// A saved thread id Codex has no rollout for (0.169.0, seen live: "no
    /// rollout found for thread id …"), or a resume that answers with another
    /// thread: a new thread is started with the turn's text, once.
    #[test]
    fn a_thread_that_cannot_be_resumed_is_started_afresh_with_the_text() {
        for answer in [
            json!({ "id": OPEN, "error": { "code": -32600, "message": "no rollout found for thread id t-old" } }),
            json!({ "id": OPEN, "result": { "thread": { "id": "t-other" } } }),
        ] {
            let id = "as-fresh";
            let open =
                json!({ "method": "thread/resume", "params": { "threadId": "t-old", "cwd": "C:\\p", "developerInstructions": "role" } });
            let lines = running(id, open);
            lock().entries.get_mut(id).unwrap().session = Some("t-old".into());
            hear(id, &answer);
            let line: Value = serde_json::from_str(&lines.try_recv().unwrap()).unwrap();
            assert_eq!(line["method"], "thread/start", "{answer}");
            assert!(line["params"].get("threadId").is_none() && line["params"]["developerInstructions"] == "role", "{line}");
            hear(id, &json!({ "id": OPEN, "result": { "thread": { "id": "t-new" } } }));
            let start: Value = serde_json::from_str(&lines.try_recv().unwrap()).unwrap();
            assert_eq!(
                (start["params"]["threadId"].as_str(), start["params"]["input"][0]["text"].as_str()),
                (Some("t-new"), Some("the task"))
            );
            hear(id, &json!({ "id": TURN, "result": { "turn": { "id": "u-1" } } }));
            assert_eq!(lock().entries[id].session.as_deref(), Some("t-new"));
            // Refused a second time: the turn fails, rather than loop.
            hear(id, &json!({ "id": OPEN, "error": { "code": -32600, "message": "no rollout found for thread id t-new" } }));
            lock().entries.remove(id);
        }
    }

    /// The approval and sandbox Luna passes go typed in the thread's request,
    /// and a thread that answers with another (its saved `never` against a
    /// named `on-request`) does not start its turn.
    #[test]
    fn a_saved_never_cannot_beat_a_named_on_request() {
        let mut e = entry(Provider::Codex);
        e.spec.approval = Some("on-request".into());
        e.spec.mode = "workspace-write".into();
        e.session = Some("0198a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b".into());
        let (_, open) = command_and_open(&e, "as-typed", &crate::library::role_texts(role_of(&e.spec))).unwrap();
        let open = open.unwrap();
        assert_eq!(
            (open["params"]["approvalPolicy"].as_str(), open["params"]["sandbox"].as_str()),
            (Some("on-request"), Some("workspace-write"))
        );
        let id = "as-typed";
        let lines = running(id, open);
        hear(
            id,
            &json!({ "id": OPEN, "result": { "thread": { "id": "0198a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b" }, "approvalPolicy": "never", "sandbox": { "type": "workspaceWrite" } } }),
        );
        assert!(lines.try_recv().is_err(), "no turn/start");
        let error = lock().entries[id].error.clone().unwrap();
        assert!(error.contains("approval \"never\"") && error.contains("on-request"), "{error}");
        lock().entries.remove(id);
        let lines = running(
            id,
            json!({ "method": "thread/resume", "params": { "threadId": "0198a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b", "approvalPolicy": "on-request", "sandbox": "read-only" } }),
        );
        hear(
            id,
            &json!({ "id": OPEN, "result": { "thread": { "id": "0198a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b" }, "approvalPolicy": "on-request", "sandbox": { "type": "readOnly" } } }),
        );
        let start: Value = serde_json::from_str(&lines.try_recv().unwrap()).unwrap();
        assert_eq!(start["method"], "turn/start", "as named: the turn goes on");
        lock().entries.remove(id);
    }

    /// Luna's own profile goes typed by its name, and a thread that answers
    /// with another profile does not start its turn.
    #[test]
    fn a_thread_runs_in_the_profile_it_was_named_or_not_at_all() {
        let asked = json!({ "permissions": "luna-b" });
        assert_eq!(settings_differ(&asked, &json!({ "activePermissionProfile": { "id": "luna-b", "extends": ":read-only" } })), None);
        let why = settings_differ(&asked, &json!({ "activePermissionProfile": { "id": "luna-a" } })).unwrap();
        assert!(why.contains("luna-a") && why.contains("luna-b"), "{why}");
        assert!(settings_differ(&asked, &json!({})).is_some(), "none said: not the one named");
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
            e.run.as_mut().unwrap().rpc.as_mut().unwrap().thread = Some("01a1f5d9-3600-7113-b9cb-fea7896d9978".into());
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
        assert_eq!(e.fed, ["PINEAPPLE"], "the server took it; the model has not read it yet");
        drop(r);
        // As 0.169.0 says it once the model has (seen live with clientUserMessageId).
        let read = json!({ "method": "item/started", "params": { "item": { "type": "userMessage", "id": "u", "clientId": "steered" } } });
        assert!(hear(id, &read).landed);
        let r = lock();
        let e = &r.entries[id];
        assert!(e.fed.is_empty(), "its userMessage item: the model's now");
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
        lock().entries.get_mut(id).unwrap().run.as_mut().unwrap().rpc.as_mut().unwrap().thread = Some("t".into());
        let messages = fixture("interrupt");
        let (before, after): (Vec<_>, Vec<_>) = {
            let at = messages.iter().position(|(to, m)| !to && m["method"] == "turn/interrupt").unwrap();
            (messages[..at].to_vec(), messages[at..].to_vec())
        };
        // Its first two are a steer of the turn before: not this one's.
        play(id, &before[2..], &lines, |_| unreachable!());
        assert!(ask_server_to_stop(lock().entries.get_mut(id).unwrap()));
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
        assert_eq!(
            error_of(&json!({ "message": "Your workspace is out of credits. Ask an admin.", "codexErrorInfo": "other" })),
            "credits_depleted"
        );
        assert_eq!(error_of(&json!({ "message": "You hit your spend cap for this month." })), "usage_limit");
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

    /// A request the server settles itself (`serverRequest/resolved`) closes
    /// its ask at once, and a late answer to it is refused, saying so.
    #[test]
    fn an_ask_the_server_settled_is_closed_and_a_late_answer_refused() {
        let id = "as-resolved";
        let lines = running(id, Value::Null);
        let heard = hear(id, &json!({ "id": 4, "method": "item/commandExecution/requestApproval", "params": { "command": "x" } }));
        let serial = heard.asks[0];
        reply_when_answered(id.to_string(), serial);
        let held = ask(id).unwrap();
        let heard = hear(id, &json!({ "method": "serverRequest/resolved", "params": { "threadId": "t", "requestId": 4 } }));
        assert!(heard.settled && ask(id).is_none());
        let late = answer(id, held.id.as_deref(), &[], Some(true), None, |_| Ok(())).unwrap_err();
        assert!(late.contains("settled it itself"), "{late}");
        assert!(lines.recv_timeout(std::time::Duration::from_millis(300)).is_err(), "nothing replies to a settled request");
        lock().entries.remove(id);
    }

    /// Steers the server took and the model never read go back to their
    /// senders when an interrupt drops them.
    #[test]
    fn an_interrupt_hands_back_the_steers_the_model_had_not_read() {
        let id = "as-dropped";
        let _lines = running(id, Value::Null);
        {
            let mut r = lock();
            let e = r.entries.get_mut(id).unwrap();
            e.run.as_mut().unwrap().rpc.as_mut().unwrap().turn = Some("u".into());
            e.fed = vec![Queued { id: "s".into(), line: Some("s".into()), ..Queued::from("steered") }];
        }
        hear(id, &json!({ "method": "turn/completed", "params": { "turn": { "id": "u", "status": "interrupted", "error": null } } }));
        let r = lock();
        assert!(r.entries[id].fed.is_empty());
        assert_eq!(r.entries[id].undelivered, ["steered"]);
        drop(r);
        lock().entries.remove(id);
    }

    /// A server that refused a request of Luna's, or went before its turn
    /// completed without Luna stopping it, is a failed session of its version
    /// for the canary, whatever it exited with.
    #[test]
    fn a_protocol_break_is_a_failure_for_the_canary() {
        let id = "as-canary";
        let _lines = running(id, Value::Null);
        assert_eq!(went_wrong(id).as_deref(), Some("its app-server went before its turn completed"));
        assert!(lock().entries[id].error.is_some(), "and the turn ended on it");
        lock().entries.remove(id);
        let _lines = running(id, Value::Null);
        hear(id, &json!({ "id": INIT, "error": { "code": -32600, "message": "unknown capability" } }));
        assert_eq!(went_wrong(id).as_deref(), Some("initialize: unknown capability"));
        lock().entries.remove(id);
        let _lines = running(id, Value::Null);
        lock().entries.get_mut(id).unwrap().run.as_mut().unwrap().rpc.as_mut().unwrap().turn = Some("u".into());
        hear(id, &json!({ "method": "turn/completed", "params": { "turn": { "id": "u", "status": "completed", "error": null } } }));
        assert_eq!(went_wrong(id), None, "a turn that completed");
        lock().entries.remove(id);
        let _lines = running(id, Value::Null);
        lock().entries.get_mut(id).unwrap().run.as_mut().unwrap().luna_stopped = true;
        assert_eq!(went_wrong(id), None, "Luna stopped it");
        lock().entries.remove(id);
    }

    /// Codex's word on a config that will not parse keeps its file, line and
    /// column and what is wrong, not the source line it quotes: the file may
    /// be a link to one that is no config (0.171.0's text, as `codex` printed
    /// it for a throwaway account).
    #[test]
    fn a_broken_configs_diagnostic_keeps_no_line_of_the_file() {
        let codex = "failed to load bootstrap configuration\n\nCaused by:\n    \
                     0: /acc/config.toml:2:16: string values must be quoted, expected literal string\n    \
                     1: TOML parse error at line 2, column 16\n         |\n       \
                     2 | secret-token = ghp_s3cr3t value\n         |                ^^^^^^^^^^^^^^^^\n       \
                     string values must be quoted, expected literal string\n";
        let kept = "failed to load bootstrap configuration\n\nCaused by:\n    \
                    0: /acc/config.toml:2:16: string values must be quoted, expected literal string\n    \
                    1: TOML parse error at line 2, column 16\n       \
                    string values must be quoted, expected literal string";
        assert_eq!(without_excerpts(codex), kept);
        let id = "as-broken-config";
        let _lines = running(id, Value::Null);
        hear(id, &json!({ "id": INIT, "error": { "code": -32600, "message": codex } }));
        let error = lock().entries[id].error.clone().unwrap();
        assert!(error.contains("config.toml:2:16") && !error.contains("ghp_") && !error.contains("secret-token"), "{error}");
        lock().entries.remove(id);
    }

    /// A server that does not start its turn in time is given up on; a turn
    /// that has started never is, however long it is quiet: its parent hears
    /// of the silence (the quiet notice, off `turn_quiet_ms`) and decides.
    #[test]
    fn only_a_turn_that_never_starts_is_given_up_on() {
        let mut e = entry(Provider::Codex);
        let rpc = Rpc::new(Value::Null, String::new(), String::new(), String::new());
        e.run = Some(Run { rpc: Some(rpc), ..Run::claimed(Some(1), 1_000) });
        assert_eq!(stalled(&e, 1_000 + HANDSHAKE_MS), None);
        assert!(stalled(&e, 1_001 + HANDSHAKE_MS).is_some_and(|w| w.contains("did not start the turn")));
        e.run.as_mut().unwrap().rpc.as_mut().unwrap().turn = Some("u".into());
        e.observed.event_ms = 5_000;
        assert_eq!(stalled(&e, u64::MAX), None, "started: never ended by the clock");
        let id = "as-quiet";
        lock().entries.insert(id.into(), e);
        assert_eq!(turn_quiet_ms(id, 5_000 + 31 * 60_000), Some(31 * 60_000), "what the quiet notice goes by");
        lock().entries.remove(id);
    }

    /// A question that takes only its options refuses words of one's own; its
    /// answers go by the question's id, in order, a secret one as any other.
    #[test]
    fn a_question_is_answered_by_its_id_and_held_to_its_options() {
        let id = "as-options";
        let lines = running(id, Value::Null);
        let questions = json!([
            { "id": "q1", "header": "Color", "question": "Same?", "isOther": false, "isSecret": false, "options": [{ "label": "Red" }, { "label": "Blue" }] },
            { "id": "q2", "header": "Again", "question": "Same?", "isOther": true, "isSecret": true, "options": [] }
        ]);
        let heard =
            hear(id, &json!({ "id": 9, "method": "item/tool/requestUserInput", "params": { "questions": questions, "isBlocking": true } }));
        let serial = heard.asks[0];
        let held = ask(id).unwrap();
        assert!(!held.questions[0].own_words && held.questions[1].own_words && held.questions[1].secret);
        let refused = answer(id, held.id.as_deref(), &["Green".into(), "x".into()], None, None, |_| Ok(())).unwrap_err();
        assert!(refused.contains("question 1 takes one of its options"), "{refused}");
        answer(id, held.id.as_deref(), &["2".into(), "hunter2".into()], None, None, |_| Ok(())).unwrap();
        reply_when_answered(id.to_string(), serial);
        let line: Value = serde_json::from_str(&lines.recv_timeout(std::time::Duration::from_secs(5)).unwrap()).unwrap();
        assert_eq!(
            line["result"],
            json!({ "answers": { "q1": { "answers": ["Blue"] }, "q2": { "answers": ["hunter2"] } } }),
            "two questions of one text"
        );
        assert!(!lock().entries[id].log.iter().any(|l| l.contains("hunter2")), "a secret answer is written nowhere");
        lock().entries.remove(id);
    }
}
