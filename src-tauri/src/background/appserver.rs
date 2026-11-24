//! A Codex helper's turn over `codex app-server` (experimental in 0.169.0):
//! JSON-RPC 2.0 on the process's stdio, one message a line. Luna is the
//! client: `initialize`, `config/read`, `thread/start` or `thread/resume`,
//! and `turn/start`. Then it reads
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
use std::time::Instant;

const MCP_ELICITATION: &str = "mcpServer/elicitation/request";
const USER_INPUT: &str = "item/tool/requestUserInput";
const MCP_APPROVAL_PREFIX: &str = "mcp_tool_call_approval_";

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

enum ConfigExchange {
    // Only protocol fixtures initialize an already prepared thread.
    Initialize(Option<Box<CodexOpen>>),
    Read { open: Box<CodexOpen>, requested: Instant },
    Ready,
}

/// A Codex turn's side of its conversation with its server.
pub(super) struct Rpc {
    /// `thread/start` or `thread/resume` with its params: what follows the
    /// handshake (command.rs `CodexOpen::prepare`).
    open: Value,
    exchange: ConfigExchange,
    start_deadline: Instant,
    pub(super) thread_opened: bool,
    /// The turn's text and effort, for `turn/start` once the thread is there.
    input: String,
    effort: String,
    /// What `turn/start` names its message (`clientUserMessageId`): the id
    /// of the line its batch went as (`Queued::line`), which the rollout keeps
    /// as the message's `client_id`: what a core taking the helper up settles
    /// its fate by (persist.rs `settle_sent`).
    client_id: String,
    /// Whether Luna supplied its MCP server to this process.
    luna_supplied: bool,
    /// Whether its startup failure has already been noted for this turn.
    luna_startup_failed: bool,
    /// `turn/start` as it is to go, held until the text it carries is saved as
    /// sent (`send_turn`).
    pending: Option<Value>,
    /// The thread the server opened: the helper's conversation only once
    /// `turn/start` is answered, as Codex writes a thread at its first turn
    /// ("no rollout found" for one never given a turn).
    pub(super) thread: Option<String>,
    /// The policy in the thread's OPEN result, saved once TURN names it.
    approval: Option<Value>,
    /// The turn's id, once `turn/start` has answered: what a steer and an
    /// interrupt name.
    pub(super) turn: Option<String>,
    /// Items begun and not completed, by id, with what each was: an
    /// interrupted turn's get no `item/completed` (0.169.0), so Luna closes
    /// them in its log.
    pub(super) open_items: Vec<(String, String)>,
    /// Spawn items begun before Codex's fallback approval question.
    spawn_items: HashMap<String, Ask>,
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
    pub(super) fn new(open: Value, input: String, effort: String, client_id: String, luna_supplied: bool) -> Rpc {
        Rpc {
            open,
            exchange: ConfigExchange::Ready,
            start_deadline: Instant::now() + std::time::Duration::from_millis(HANDSHAKE_MS),
            thread_opened: false,
            input,
            effort,
            client_id,
            luna_supplied,
            luna_startup_failed: false,
            pending: None,
            thread: None,
            approval: None,
            turn: None,
            open_items: Vec::new(),
            spawn_items: HashMap::new(),
            interrupting: false,
            done: false,
            broken: None,
        }
    }

    pub(super) fn starting(open: CodexOpen, input: String, effort: String, client_id: String) -> Self {
        let mut rpc = Self::new(Value::Null, input, effort, client_id, false);
        rpc.exchange = ConfigExchange::Initialize(Some(Box::new(open)));
        rpc
    }

    #[cfg(test)]
    fn fixture_initialize(&mut self) {
        self.exchange = ConfigExchange::Initialize(None);
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
    /// The saved policy changed when a thread could not resume or started its turn.
    pub(super) approval_changed: bool,
    /// Read the catalog for this launch after releasing the registry lock.
    pub(super) compaction: Option<(String, u64, crate::codex::config::Compaction)>,
    pub(super) config_validated: bool,
}

/// One message off a turn's server, into helper `id`, under the registry's
/// lock (run.rs `take_event`).
#[cfg(test)]
pub(super) fn on_message(r: &mut Registry, id: &str, v: &Value) -> Heard {
    on_frame(r, id, Ok(v), Instant::now())
}

pub(super) fn on_frame(r: &mut Registry, id: &str, frame: Result<&Value, &str>, received: Instant) -> Heard {
    let mut heard = Heard::default();
    let Some(e) = r.entries.get_mut(id) else { return heard };
    let rpc = rpc_of(e);
    let exchanging = rpc.is_some_and(|rpc| !matches!(rpc.exchange, ConfigExchange::Ready));
    if exchanging && rpc.is_some_and(|rpc| rpc.done) {
        return heard;
    }
    if rpc.is_some_and(|rpc| !rpc.done && rpc.turn.is_none() && received >= rpc.start_deadline) {
        let why = if exchanging { "the app-server's configuration exchange timed out".into() } else { start_timeout() };
        broke(e, why, &mut heard);
        return heard;
    }
    let v = match frame {
        Ok(v) => v,
        Err(why) => {
            broke(e, why.into(), &mut heard);
            return heard;
        }
    };
    let (method, rid) = (v.get("method").and_then(Value::as_str), v.get("id"));
    if exchanging {
        match (method, rid) {
            (Some(_), None) => {} // Startup notifications cannot open a thread or ask for tools.
            (None, Some(rid))
                if rid.is_string()
                    && v.get("method").is_none()
                    && (v["result"].is_object() ^ v["error"].is_object())
                    && (v.get("result").is_none() || v.get("error").is_none()) =>
            {
                answered(e, rid.as_str().unwrap_or_default(), v, received, &mut heard);
            }
            _ => broke(e, "unexpected message during the app-server's configuration exchange".into(), &mut heard),
        }
        return heard;
    }

    if let (Some(method), Some(rid)) = (method, rid) {
        heard.asks.extend(server_request(r, id, rid, method, &v["params"]));
        return heard;
    }
    match (method, rid) {
        (Some(method), _) => notified(e, method, &v["params"], &mut heard),
        (None, Some(rid)) => answered(e, rid.as_str().unwrap_or(""), v, received, &mut heard),
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
    let why = crate::codex::snapshot::without_excerpts(&why);
    note(e, &format!("error: {}", one_line(&why)));
    if let Some(rpc) = rpc_mut(e) {
        rpc.broken = Some(why.clone());
    }
    e.error = Some(why);
    close(e, heard);
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
    let inherited_approval = !e.spec.auto_review && e.spec.approval.is_none();
    heard.approval_changed = e.observed.codex_approval.take().is_some();
    let Some(rpc) = rpc_mut(e) else { return };
    rpc.open["method"] = json!("thread/start");
    if let Some(params) = rpc.open["params"].as_object_mut() {
        params.remove("threadId");
        if inherited_approval {
            params.remove("approvalPolicy");
            if let Some(config) = params.get_mut("config").and_then(Value::as_object_mut) {
                if let Some(approval) = config
                    .get_mut("mcp_servers")
                    .and_then(|servers| servers.get_mut("luna"))
                    .and_then(|luna| luna.get_mut("tools"))
                    .and_then(|tools| tools.get_mut("luna_session_spawn"))
                    .and_then(|spawn| spawn.get_mut("approval_mode"))
                {
                    *approval = json!("prompt");
                }
                if config.is_empty() {
                    params.remove("config");
                }
            }
        }
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
    if let Some(want) = asked.get("approvalPolicy") {
        let got = &opened["approvalPolicy"];
        if got != want {
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
fn answered(e: &mut Entry, rid: &str, v: &Value, received: Instant, heard: &mut Heard) {
    if rpc_of(e).is_some_and(|rpc| rpc.done) {
        return;
    }
    let ok = v.get("error").is_none();
    match rid {
        INIT => {
            let Some(rpc) = rpc_mut(e) else { return };
            let ConfigExchange::Initialize(open) = &mut rpc.exchange else {
                return broke(e, "duplicate initialize response".into(), heard);
            };
            if !ok {
                return failed(e, "initialize", v, heard);
            }
            let line = if let Some(open) = open.take() {
                let Some(cwd) = open.request["params"]["cwd"].as_str() else {
                    return broke(e, "the helper's config/read has no cwd".into(), heard);
                };
                let line = format!("{}\n", crate::codex::snapshot::request(cwd));
                rpc.exchange = ConfigExchange::Read { open, requested: Instant::now() };
                line
            } else {
                // Protocol fixtures recorded with an already prepared thread.
                rpc.exchange = ConfigExchange::Ready;
                let method = rpc.open["method"].as_str().unwrap_or("thread/start");
                request(OPEN, method, rpc.open["params"].clone())
            };
            if !send(e, notification("initialized")) || !send(e, line) {
                close(e, heard);
            }
        }
        crate::codex::snapshot::READ_ID => {
            let Some(rpc) = rpc_of(e) else { return };
            let ConfigExchange::Read { open: pending, requested } = &rpc.exchange else {
                return broke(e, "unsolicited config/read response".into(), heard);
            };
            let luna_supplied = pending.luna_url.is_some();
            let elapsed = received.saturating_duration_since(*requested);
            let prepared = crate::codex::snapshot::Snapshot::response(v)
                .and_then(|snapshot| Ok((pending.prepare(&snapshot, &e.spec)?, snapshot.compaction()?)));
            let (PreparedOpen { open, applied }, compaction) = match prepared {
                Ok(prepared) => prepared,
                Err(why) => return broke(e, redacted(&why), heard),
            };
            if let Some(launch) = &e.compaction {
                heard.compaction = Some((launch.account_path.clone(), launch.start, compaction));
            }
            let keys = applied.iter().map(|item| item.key).collect::<Vec<_>>().join(", ");
            crate::log::info("codex-config", &format!("helper config/read replied in {} ms, applied [{keys}]", elapsed.as_millis()));
            let line = request(OPEN, open["method"].as_str().expect("prepared open has a method"), open["params"].clone());
            if let Some(rpc) = rpc_mut(e) {
                rpc.exchange = ConfigExchange::Ready;
                rpc.open = open;
                rpc.luna_supplied = luna_supplied;
            }
            heard.config_validated = true;
            if !send(e, line) {
                close(e, heard);
            }
        }
        OPEN if rpc_of(e).is_some_and(|rpc| !matches!(rpc.exchange, ConfigExchange::Ready)) => {
            broke(e, "the thread answered before config/read".into(), heard);
        }
        _ if rpc_of(e).is_some_and(|rpc| !matches!(rpc.exchange, ConfigExchange::Ready)) => {
            broke(e, "unexpected response during the app-server's configuration exchange".into(), heard);
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
            if let Some(rpc) = rpc_mut(e) {
                rpc.thread_opened = true;
            }
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
                rpc.approval = r.get("approvalPolicy").filter(|policy| !policy.is_null()).cloned();
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
            let opened = rpc_mut(e).and_then(|rpc| {
                rpc.turn = Some(turn);
                Some((rpc.thread.clone()?, rpc.approval.clone()))
            });
            // From here the thread is written: it is the helper's
            // conversation, and the server has the turn's text, not to be
            // handed back.
            if let Some((thread, approval)) = opened {
                if e.session.as_ref() != Some(&thread) || e.observed.codex_approval.is_none() {
                    heard.approval_changed = e.observed.codex_approval != approval;
                    e.observed.codex_approval = approval;
                }
                if e.session.as_ref() != Some(&thread) {
                    e.session = Some(thread);
                    mark_by_thread(e);
                }
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
        "mcpServer/startupStatus/updated" if p["name"] == crate::codex::launch::LUNA_SERVER && p["status"] == "failed" => {
            if let Some(rpc) = rpc_mut(e).filter(|rpc| rpc.luna_supplied && !rpc.done && !rpc.luna_startup_failed) {
                rpc.luna_startup_failed = true;
                let status = "MCP startup failed. This helper continues without Luna's tools.";
                note(e, &format!("Luna tools: {status}"));
                crate::log::info("codex-config", &format!("applied [{}] startup: {status}", crate::applied::Item::LunaTools.key()));
            }
        }
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
                if rpc.luna_supplied
                    && item["type"] == "mcpToolCall"
                    && item["server"] == "luna"
                    && item["tool"] == "luna_session_spawn"
                    && item["arguments"].is_object()
                    && rpc.spawn_items.len() < ASKS_PER_TURN
                {
                    rpc.spawn_items
                        .insert(id.to_string(), Ask::permission(Some("luna_session_spawn".into()), Some(item["arguments"].to_string())));
                }
            }
        }
        "item/completed" => {
            let item = &p["item"];
            if let (Some(id), Some(rpc)) = (item["id"].as_str(), rpc_mut(e)) {
                rpc.open_items.retain(|(open, _)| open != id);
                rpc.spawn_items.remove(id);
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
            e.held.retain(|h| &h.input["id"] != rid || h.input.get("method").is_none());
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
        MCP_ELICITATION if p["serverName"] == "luna" && p["mode"] == "form" => {
            let meta = &p["_meta"];
            let tool = meta["tool_name"].as_str()?;
            if meta["codex_approval_kind"] != "mcp_tool_call" || !crate::mcp::tool_names().iter().any(|name| name == tool) {
                return None;
            }
            let detail = match meta.get("tool_params") {
                Some(params) if params.is_object() => params.to_string(),
                None if tool != "luna_session_spawn" => "{}".into(),
                _ => return None,
            };
            Ask::permission(Some(tool.into()), Some(detail))
        }
        USER_INPUT => {
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

fn mcp_question(params: &Value) -> Option<&str> {
    params["questions"].as_array()?.iter().filter_map(|q| q["id"].as_str()).find(|id| id.starts_with(MCP_APPROVAL_PREFIX))
}

/// A request of the server's. An approval or a question is held for an
/// answer, its serial returned for `reply_when_answered`; what else it asks
/// (other MCP elicitations, a dynamic tool, a token refresh, an attestation)
/// has nobody to answer it and is refused at once, as a
/// Claude Code helper refuses its CLI's own requests.
fn server_request(r: &mut Registry, id: &str, rid: &Value, method: &str, p: &Value) -> Option<u64> {
    let Registry { entries, next_serial, asked, .. } = r;
    let serial = *next_serial;
    let e = entries.get_mut(id)?;
    let is_elicitation = method == MCP_ELICITATION;
    let fallback = (method == USER_INPUT).then(|| mcp_question(p)).flatten();
    let is_mcp = is_elicitation || fallback.is_some();
    if is_mcp && e.held.iter().any(|h| h.input["method"] == method && h.input["id"] == *rid) {
        return None;
    }
    let open = rpc_of(e).map(|rpc| rpc.open_items.clone()).unwrap_or_default();
    // A saved token can survive a hub bind failure. Only a process launched
    // with Luna's server can relay its approvals. The name check also relies
    // on launch rejecting config collisions (see the threat model's B4).
    let ask = if is_mcp && (!rpc_of(e).is_some_and(|rpc| rpc.luna_supplied) || e.held.len() >= ASKS_PER_TURN) {
        None
    } else if let Some(qid) = fallback {
        let call = qid.strip_prefix(MCP_APPROVAL_PREFIX)?;
        rpc_of(e)
            .and_then(|rpc| rpc.spawn_items.get(call))
            .cloned()
            .filter(|_| p["itemId"] == call && p["questions"].as_array().is_some_and(|questions| questions.len() == 1))
            .map(|ask| ask.asked_by(Some(crate::agents::AskSource::session())))
    } else {
        ask_of(method, p, &open)
    };
    let Some(mut ask) = ask else {
        let line = match method {
            MCP_ELICITATION => reply(rid, json!({ "action": "decline" })),
            USER_INPUT if fallback.is_some() => reply(rid, reply_to(method, p, None)),
            _ => refusal(rid, &format!("a Luna helper has nobody to answer {method}")),
        };
        if send(e, line) {
            note(e, &format!("refused the server's request: {method}"));
        }
        return None;
    };
    ask.id = Some(ask_id_of(serial));
    note(e, &format!("waits: {}", ask.line()));
    // An elicitation's reply needs only its id. The ask keeps the display
    // detail within the same cap as other permissions.
    let input = if is_elicitation {
        json!({ "id": rid, "method": method })
    } else if let Some(qid) = fallback {
        json!({ "id": rid, "method": method, "params": { "questions": [{ "id": qid }] } })
    } else {
        json!({ "id": rid, "method": method, "params": p })
    };
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
        USER_INPUT => {
            if let Some(id) = mcp_question(p) {
                return json!({ "answers": { id: { "answers": [if allowed { "Allow" } else { "Cancel" }] } } });
            }
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
        // Codex's spawn approval form has no note field, and a decline has no content.
        MCP_ELICITATION => json!({ "action": if allowed { "accept" } else { "decline" } }),
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
        let deadline = Instant::now() + ASK_TIMEOUT;
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
            let left = deadline.saturating_duration_since(Instant::now());
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
    (rpc.turn.is_none() && now.saturating_sub(run.started_ms) > HANDSHAKE_MS).then(start_timeout)
}

fn start_timeout() -> String {
    format!("its app-server did not start the turn within {} minutes", HANDSHAKE_MS / 60_000)
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
        running_with_luna(id, open, false)
    }

    fn running_with_luna(id: &str, open: Value, luna_supplied: bool) -> std::sync::mpsc::Receiver<String> {
        let (feed, lines) = std::sync::mpsc::channel();
        let mut e = entry(Provider::Codex);
        let rpc = Rpc::new(open, "the task".into(), "low".into(), "client-1".into(), luna_supplied);
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

    #[test]
    fn only_a_supplied_luna_servers_startup_failure_is_noted_once() {
        let id = "as-luna-startup";
        let failed = |server| {
            json!({
                "method": "mcpServer/startupStatus/updated",
                "params": {"name": server, "status": "failed", "error": "a private value"},
            })
        };
        for supplied in [false, true] {
            let lines = running_with_luna(id, Value::Null, supplied);
            hear(id, &failed("another-server"));
            hear(id, &failed("luna"));
            hear(id, &failed("luna"));
            let e = lock().entries.remove(id).unwrap();
            assert_eq!(e.log.len(), usize::from(supplied));
            if supplied {
                assert_eq!(e.log[0], "Luna tools: MCP startup failed. This helper continues without Luna's tools.");
            }
            assert!(lines.try_recv().is_err());
        }
    }

    /// Plays the server's side of a fixture into `id`, answering each of its
    /// requests with `answer` as a caller would; returns what Luna wrote.
    fn play(
        id: &str,
        messages: &[(bool, Value)],
        lines: &std::sync::mpsc::Receiver<String>,
        answer: impl Fn(&Ask) -> (Vec<String>, Option<bool>),
    ) -> Vec<Value> {
        if messages.iter().find(|(to_luna, _)| *to_luna).is_some_and(|(_, msg)| msg["id"] == INIT) {
            rpc_mut(lock().entries.get_mut(id).unwrap()).unwrap().fixture_initialize();
        }
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

    #[test]
    fn a_turn_waits_for_config_before_start_resume_or_replacement() {
        let fixture: Value = serde_json::from_str(include_str!("testdata/codex-0.171.0-config-read.json")).unwrap();
        let cwd = if cfg!(windows) { r"C:\fixture\destination" } else { "/fixture/destination" };
        for (suffix, thread) in [("start", None), ("resume", Some("beef"))] {
            let id = format!("config-read-{suffix}");
            let (feed, lines) = std::sync::mpsc::channel();
            let mut e = entry(Provider::Codex);
            e.spec.folder = cwd.to_owned();
            e.spec.model = None;
            e.spec.connectors = false;
            e.compaction = Some(crate::to_compact::Launch {
                provider: Provider::Codex,
                account_path: e.spec.account_path.clone(),
                folder: cwd.into(),
                start: 7,
                claude_window: None,
                codex: None,
            });
            e.session = thread.map(|_| "abc21135-25c7-484f-8cb3-32c4cff0ab29".to_owned());
            let (_, open) = build_command(&e, &id, &crate::library::role_texts(role_of(&e.spec)), None).unwrap();
            let rpc = Rpc::starting(open.unwrap(), "task".into(), "low".into(), "client".into());
            e.run = Some(Run { feed: Some(feed), rpc: Some(rpc), ..Run::claimed(Some(1), 0) });
            lock().entries.insert(id.clone(), e);

            hear(&id, &json!({"id": INIT, "result": {}}));
            let wrote = || lines.try_iter().map(|line| serde_json::from_str::<Value>(&line).unwrap()).collect::<Vec<_>>();
            let first = wrote();
            assert_eq!(methods(&first), ["initialized", "config/read"]);
            assert_eq!(first[1]["params"]["cwd"], cwd);
            let heard = hear(&id, &json!({"id": crate::codex::snapshot::READ_ID, "result": fixture["loaded"]}));
            let (_, start, inputs) = heard.compaction.unwrap();
            assert_eq!(start, 7);
            assert_eq!(inputs.token_limit, fixture["loaded"]["config"]["model_auto_compact_token_limit"].as_i64());
            let opened = wrote();
            assert_eq!(methods(&opened), [if thread.is_some() { "thread/resume" } else { "thread/start" }]);
            let config = &opened[0]["params"]["config"];
            assert_eq!(config["features.apps"], false);
            assert_eq!(config["mcp_servers"]["team.docs"]["enabled"], false);
            assert_eq!(config["plugins"]["_default"]["enabled"], false);

            if thread.is_some() {
                hear(&id, &json!({"id": OPEN, "error": {"message": "no rollout found"}}));
                let retried = wrote();
                assert_eq!(methods(&retried), ["thread/start"]);
                assert_eq!(retried[0]["params"]["cwd"], cwd);
                assert_eq!(&retried[0]["params"]["config"], config);
            }
            lock().entries.remove(&id);
        }
    }

    #[test]
    fn config_read_failure_closes_stdin_before_the_thread_is_requested() {
        let id = "appserver-config-refusal";
        let (feed, lines) = std::sync::mpsc::channel();
        let mut e = entry(Provider::Codex);
        let (_, open) = build_command(&e, id, &crate::library::role_texts(role_of(&e.spec)), None).unwrap();
        let rpc = Rpc::starting(open.unwrap(), "task".into(), "low".into(), "client".into());
        e.run = Some(Run { feed: Some(feed), rpc: Some(rpc), ..Run::claimed(Some(1), 0) });
        lock().entries.insert(id.into(), e);
        hear(id, &json!({"id": INIT, "result": {}}));
        assert_eq!(lines.try_iter().count(), 2);
        hear(id, &json!({"id": crate::codex::snapshot::READ_ID, "error": {"message": "settings unavailable\n  2 | PRIVATE CONFIG"}}));
        assert!(matches!(lines.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected)));
        let e = lock().entries.remove(id).unwrap();
        assert_eq!(rpc_of(&e).unwrap().broken.as_deref(), Some("Codex could not read its launch configuration: settings unavailable"));
    }

    #[test]
    fn configuration_replies_are_ordered_single_use_and_on_time() {
        use crate::codex::snapshot::READ_ID;
        let fixture: Value = serde_json::from_str(include_str!("testdata/codex-0.171.0-config-read.json")).unwrap();
        let config = json!({"id": READ_ID, "result": fixture["loaded"]});
        let init = json!({"id": INIT, "result": {}});
        for (case, before, next, late, why) in [
            ("repeat-init", vec![&init], init.clone(), false, "duplicate initialize"),
            ("repeat-config", vec![&init, &config], config.clone(), false, "unsolicited config/read"),
            ("early-open", vec![&init], json!({"id": OPEN, "result": {}}), false, "the thread answered before config/read"),
            ("early-config", vec![], config.clone(), false, "unsolicited config/read"),
            ("late-init", vec![], init.clone(), true, "configuration exchange timed out"),
            ("late-config", vec![&init], config.clone(), true, "configuration exchange timed out"),
            ("late-open", vec![&init, &config], json!({"id": OPEN, "result": {}}), true, "did not start the turn"),
            ("bad-init", vec![], json!({"id": INIT, "result": []}), false, "unexpected message"),
        ] {
            let id = format!("appserver-config-{case}");
            let (feed, lines) = std::sync::mpsc::channel();
            let mut e = entry(Provider::Codex);
            let (_, open) = build_command(&e, &id, &crate::library::role_texts(role_of(&e.spec)), None).unwrap();
            let rpc = Rpc::starting(open.unwrap(), "task".into(), "low".into(), "client".into());
            let deadline = rpc.start_deadline;
            e.run = Some(Run { feed: Some(feed), rpc: Some(rpc), ..Run::claimed(Some(1), 0) });
            lock().entries.insert(id.clone(), e);
            for message in before {
                let heard = hear(&id, message);
                assert_eq!(heard.config_validated, message["id"] == READ_ID, "{case}");
            }
            lines.try_iter().for_each(drop);
            let received = if late { deadline } else { deadline - std::time::Duration::from_secs(1) };
            let heard = on_frame(&mut lock(), &id, Ok(&next), received);
            assert!(!heard.config_validated && heard.ended, "{case}");
            assert!(matches!(lines.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected)), "{case}");
            let e = lock().entries.remove(&id).unwrap();
            assert!(rpc_of(&e).unwrap().broken.as_deref().is_some_and(|error| error.contains(why)), "{case}: {:?}", e.error);
        }
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

    #[test]
    fn the_threads_reported_policy_is_kept_after_its_first_turn_starts() {
        let id = "as-observed-approval";
        let lines = running(id, json!({ "method": "thread/start", "params": {} }));
        hear(id, &json!({ "id": OPEN, "result": { "thread": { "id": "first-thread" }, "approvalPolicy": "never" } }));
        assert!(lock().entries[id].observed.codex_approval.is_none());
        assert!(lines.try_recv().is_ok());
        hear(id, &json!({ "id": TURN, "result": { "turn": { "id": "first-turn" } } }));
        let mut r = lock();
        let e = r.entries.remove(id).unwrap();
        assert_eq!(e.session.as_deref(), Some("first-thread"));
        assert_eq!(serde_json::to_value(&e.observed).unwrap()["codexApproval"], "never");
    }

    /// A saved thread id Codex has no rollout for (0.169.0, seen live: "no
    /// rollout found for thread id …"), or a resume that answers with another
    /// thread: a new thread is started with the turn's text, once.
    #[test]
    fn a_thread_that_cannot_be_resumed_is_started_afresh_with_the_text() {
        for (answer, named_policy) in [
            (json!({ "id": OPEN, "error": { "code": -32600, "message": "no rollout found for thread id t-old" } }), None),
            (json!({ "id": OPEN, "result": { "thread": { "id": "t-other" } } }), Some("on-request")),
        ] {
            let id = "as-fresh";
            let skills = json!([{ "path": "C:/acc/skills/review/SKILL.md", "enabled": false }]);
            let mut open = json!({ "method": "thread/resume", "params": {
                "threadId": "t-old", "cwd": "C:\\p", "developerInstructions": "role", "approvalPolicy": named_policy.unwrap_or("never"),
                "config": { "skills.config": skills }
            } });
            if named_policy.is_none() {
                open["params"]["config"]["mcp_servers"] = json!({"luna":{"tools":{"luna_session_spawn":{"approval_mode":"approve"}}}});
            }
            let lines = running(id, open);
            {
                let mut r = lock();
                let e = r.entries.get_mut(id).unwrap();
                e.session = Some("t-old".into());
                e.spec.approval = named_policy.map(str::to_owned);
                e.observed.codex_approval = Some(json!("never"));
            }
            assert!(hear(id, &answer).approval_changed);
            assert!(lock().entries[id].observed.codex_approval.is_none());
            let line: Value = serde_json::from_str(&lines.try_recv().unwrap()).unwrap();
            assert_eq!(line["method"], "thread/start", "{answer}");
            assert!(line["params"].get("threadId").is_none() && line["params"]["developerInstructions"] == "role", "{line}");
            assert_eq!(line["params"]["approvalPolicy"].as_str(), named_policy);
            assert_eq!(line["params"]["config"]["skills.config"], skills);
            assert_eq!(
                line.pointer("/params/config/mcp_servers/luna/tools/luna_session_spawn/approval_mode").and_then(Value::as_str),
                named_policy.is_none().then_some("prompt")
            );
            hear(id, &json!({ "id": OPEN, "result": { "thread": { "id": "t-new" }, "approvalPolicy": "on-request" } }));
            let start: Value = serde_json::from_str(&lines.try_recv().unwrap()).unwrap();
            assert_eq!(
                (start["params"]["threadId"].as_str(), start["params"]["input"][0]["text"].as_str()),
                (Some("t-new"), Some("the task"))
            );
            assert!(hear(id, &json!({ "id": TURN, "result": { "turn": { "id": "u-1" } } })).approval_changed);
            assert_eq!(lock().entries[id].session.as_deref(), Some("t-new"));
            assert_eq!(lock().entries[id].observed.codex_approval, Some(json!("on-request")));
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
        let (_, open) = command_and_open(&e, "as-typed", &crate::library::role_texts(role_of(&e.spec)), None).unwrap();
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
        assert_eq!(delivery_turn(id, "steered"), None);
        // As 0.169.0 says it once the model has (seen live with clientUserMessageId).
        let read = json!({ "method": "item/started", "params": { "item": { "type": "userMessage", "id": "u", "clientId": "steered" } } });
        assert!(hear(id, &read).landed);
        assert!(delivery_turn(id, "steered").is_some());
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

    fn spawn_elicitation() -> Value {
        json!({
            "id": 7,
            "method": MCP_ELICITATION,
            "params": {
                "threadId": "thread",
                "turnId": "turn",
                "serverName": "luna",
                "mode": "form",
                "message": "Allow the luna MCP server to run tool \"luna_session_spawn\"?",
                "requestedSchema": { "type": "object", "properties": {} },
                "_meta": {
                    "codex_approval_kind": "mcp_tool_call",
                    "tool_name": "luna_session_spawn",
                    "tool_params": { "folder": "C:/p", "prompt": "review it", "sandbox": "danger-full-access" }
                }
            }
        })
    }

    #[test]
    fn an_on_request_leads_spawn_waits_for_its_parents_decision() {
        use crate::agents::{answer, forget, register};
        let (parent, other, id) = ("as-spawn-parent", "as-spawn-other", "as-spawn-lead");
        for who in [parent, other] {
            register(who, Provider::Codex, "C:/p", "C:/acc", None, true, json!({ "approval": "on-request" }));
        }
        let token = register(id, Provider::Codex, "C:/p", "C:/acc", Some(parent), true, json!({ "approval": "on-request" }));
        let lines = running_with_luna(id, Value::Null, true);
        {
            let mut r = lock();
            let spec = &mut r.entries.get_mut(id).unwrap().spec;
            spec.tools = true;
            spec.token = token;
        }
        let req = spawn_elicitation();
        for (allow, action) in [(false, "decline"), (true, "accept")] {
            let heard = hear(id, &req);
            let [serial] = heard.asks[..] else { panic!("one approval, got {heard:?}") };
            let pending = ask(id).expect("the lead holds a permission ask");
            assert_eq!((pending.kind, pending.tool.as_deref()), ("permission", Some("luna_session_spawn")));
            assert_eq!(pending.detail.as_deref(), Some(req["params"]["_meta"]["tool_params"].to_string().as_str()));
            let ask_id = pending.id.as_deref();
            assert!(ask_id.is_some());
            assert!(answer(parent, id, None, &[], Some(true), None, false).is_err_and(|err| err.contains("requires its askId")));
            let epoch = ask_epoch();
            let other_core = if epoch.as_bytes()[1] == b'0' { '1' } else { '0' };
            let stale = format!("a{other_core}{}{serial}", &epoch[2..]);
            assert!(answer(parent, id, Some(&stale), &[], Some(true), None, false).is_err_and(|err| err.contains("unknown")));
            assert!(hear(id, &req).asks.is_empty(), "the same request is held once");
            reply_when_answered(id.into(), serial);
            assert!(answer(other, id, ask_id, &[], Some(true), None, false).is_err());
            assert!(lines.try_recv().is_err(), "Codex has no reply allowing the spawn to run yet");
            answer(parent, id, ask_id, &[], Some(allow), Some("reviewed"), false).unwrap();
            let wrote: Value = serde_json::from_str(&lines.recv_timeout(std::time::Duration::from_secs(5)).unwrap()).unwrap();
            assert_eq!(wrote["id"], req["id"]);
            assert_eq!(wrote["result"], json!({ "action": action }));
            assert!(ask(id).is_none());
        }
        lock().entries.remove(id);
        for who in [id, parent, other] {
            forget(who);
        }
    }

    #[test]
    fn the_fallback_spawn_question_uses_the_matching_items_arguments_and_a_one_shot_decision() {
        use crate::agents::{answer, forget, register};
        let (parent, other, id) = ("as-fallback-parent", "as-fallback-other", "as-fallback-lead");
        for who in [parent, other] {
            register(who, Provider::Codex, "C:/p", "C:/acc", None, true, json!({}));
        }
        let token = register(id, Provider::Codex, "C:/p", "C:/acc", Some(parent), true, json!({}));
        let lines = running_with_luna(id, Value::Null, true);
        {
            let mut r = lock();
            let spec = &mut r.entries.get_mut(id).unwrap().spec;
            spec.tools = true;
            spec.token = token;
        }
        let args = json!({ "prompt": "review it", "sandbox": "danger-full-access" });
        let qid = "mcp_tool_call_approval_call-1";
        let request = json!({
            "id": 9, "method": USER_INPUT, "params": {
                "itemId": "call-1", "questions": [{
                    "id": qid, "question": "Allow the luna MCP server to run tool \"luna_session_spawn\"?",
                    "options": [{"label":"Allow"}, {"label":"Allow for this session"}, {"label":"Cancel"}]
                }]
            }
        });
        let read_reply = || {
            let line = lines.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
            serde_json::from_str::<Value>(&line).unwrap()["result"]["answers"][qid]["answers"].clone()
        };
        assert!(hear(id, &request).asks.is_empty(), "missing item");
        assert_eq!(read_reply(), json!(["Cancel"]));
        hear(
            id,
            &json!({
                "method": "item/started", "params": { "item": {
                    "id": "call-1", "type": "mcpToolCall", "server": "luna", "tool": "luna_session_spawn", "arguments": args
                }}
            }),
        );
        for (allow, expected) in [(false, "Cancel"), (true, "Allow")] {
            let serial = hear(id, &request).asks[0];
            let pending = ask(id).unwrap();
            let ask_id = pending.id.as_deref();
            assert_eq!((pending.kind, pending.tool.as_deref()), ("permission", Some("luna_session_spawn")));
            assert_eq!(pending.detail.as_deref(), Some(args.to_string().as_str()));
            assert!(answer(other, id, ask_id, &[], Some(true), None, false).is_err());
            assert!(answer(parent, id, ask_id, &["Allow for this session".into()], None, None, false).is_err());
            answer(parent, id, ask_id, &[], Some(allow), None, false).unwrap();
            reply_when_answered(id.into(), serial);
            assert_eq!(read_reply(), json!([expected]));
        }
        lock().entries.remove(id);
        for who in [id, parent, other] {
            forget(who);
        }
    }

    #[test]
    fn a_server_resolution_discards_an_answer_queued_before_the_reply_worker() {
        let id = "as-settled-with-answer";
        let lines = running(id, Value::Null);
        let request = json!({ "id": 17, "method": "item/commandExecution/requestApproval", "params": { "command": "echo hi" } });
        let serial = hear(id, &request).asks[0];
        answer(id, None, &[], Some(true), None, |_| Ok(())).unwrap();
        assert!(hear(id, &json!({ "method": "serverRequest/resolved", "params": { "requestId": 17 } })).settled);
        reply_when_answered(id.into(), serial);
        assert!(lines.recv_timeout(std::time::Duration::from_millis(100)).is_err());
        lock().entries.remove(id);
    }

    #[test]
    fn a_saved_token_without_lunas_server_cannot_relay_an_approval() {
        let id = "as-spawn-no-server";
        let lines = running(id, Value::Null);
        {
            let mut r = lock();
            let spec = &mut r.entries.get_mut(id).unwrap().spec;
            spec.tools = true;
            spec.connectors = true;
            spec.token = Some("test-token".into());
        }
        assert!(hear(id, &spawn_elicitation()).asks.is_empty());
        let wrote: Value = serde_json::from_str(&lines.try_recv().unwrap()).unwrap();
        assert_eq!(wrote["result"], json!({ "action": "decline" }));
        hear(
            id,
            &json!({ "method": "item/started", "params": { "item": {
                "id": "call-1", "type": "mcpToolCall", "server": "luna", "tool": "luna_session_spawn", "arguments": {}
            } } }),
        );
        let qid = "mcp_tool_call_approval_call-1";
        let fallback = json!({ "id": 9, "method": USER_INPUT, "params": { "itemId": "call-1", "questions": [{ "id": qid }] } });
        assert!(hear(id, &fallback).asks.is_empty());
        let wrote: Value = serde_json::from_str(&lines.try_recv().unwrap()).unwrap();
        assert_eq!(wrote["result"]["answers"][qid]["answers"], json!(["Cancel"]));
        assert!(ask(id).is_none());
        lock().entries.remove(id);
    }

    #[test]
    fn other_elicitations_and_excess_luna_approvals_are_declined() {
        let id = "as-spawn-refuse";
        let lines = running_with_luna(id, Value::Null, true);
        for (key, value) in [
            ("/params/serverName", json!("other")),
            ("/params/mode", json!("url")),
            ("/params/_meta/codex_approval_kind", json!("browser_auth")),
            ("/params/_meta/tool_name", json!("some_other_tool")),
            ("/params/_meta/tool_params", Value::Null),
        ] {
            let mut req = spawn_elicitation();
            *req.pointer_mut(key).unwrap() = value;
            assert!(hear(id, &req).asks.is_empty(), "{key}");
            let wrote: Value = serde_json::from_str(&lines.try_recv().unwrap()).unwrap();
            assert_eq!(wrote["result"], json!({ "action": "decline" }));
        }
        for n in 0..ASKS_PER_TURN {
            let mut req = spawn_elicitation();
            req["id"] = json!(n);
            assert_eq!(hear(id, &req).asks.len(), 1);
        }
        let mut req = spawn_elicitation();
        req["id"] = json!(999);
        assert!(hear(id, &req).asks.is_empty());
        let wrote: Value = serde_json::from_str(&lines.try_recv().unwrap()).unwrap();
        assert_eq!(wrote["result"], json!({ "action": "decline" }));
        let mut r = lock();
        r.entries.remove(id);
        r.asked.retain(|_, to| to != id);
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
        rpc_mut(lock().entries.get_mut(id).unwrap()).unwrap().fixture_initialize();
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
        assert_eq!(crate::codex::snapshot::without_excerpts(codex), kept);
        let id = "as-broken-config";
        let _lines = running(id, Value::Null);
        rpc_mut(lock().entries.get_mut(id).unwrap()).unwrap().fixture_initialize();
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
        let rpc = Rpc::new(Value::Null, String::new(), String::new(), String::new(), false);
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
