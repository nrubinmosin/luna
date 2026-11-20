//! The hooks other modules' tests drive helpers through, and the helpers this
//! module's own tests share.

use super::*;

/// Whom it is told it answers to, for the tests of its passing to another.
#[cfg(test)]
pub fn told_parent_for_test(id: &str) -> Option<String> {
    lock().entries.get(id)?.spec.parent.clone()
}

/// Its next turn's parent, as a spawn sets it: no change of the registry's
/// written before.
#[cfg(test)]
pub fn set_parent_for_test(id: &str, parent: Option<String>) {
    let mut r = lock();
    r.parent_changes.remove(id);
    if let Some(e) = r.entries.get_mut(id) {
        e.spec.parent = parent;
    }
}

/// Text that never reached a helper, by whom, for the tests of who hears of it.
/// A notice with `mark` in it, given to a helper as `send_marked` does, the part
/// that starts a CLI aside. Returns whether it was queued.
#[cfg(test)]
pub fn place_for_test(id: &str, text: &str, mark: BudgetMark) -> bool {
    place(lock().entries.get_mut(id).unwrap(), Queued::new(None, text.to_string(), vec![mark]), false, true) != Placed::Started
}

/// The texts queued for its next turn, in order.
#[cfg(test)]
pub fn queued_for_test(id: &str) -> Vec<String> {
    lock().entries.get(id).map(|e| e.queue.iter().map(|q| q.text.clone()).collect()).unwrap_or_default()
}

/// Being opened as a chat by the person, or not: an opening that never lapses.
/// Tests run side by side, and one that lapses openings (`lapse_openings`, by
/// way of `activity`) at the real time would take back one begun at 0, the
/// default, past PROMOTE_TIMEOUT_MS since.
#[cfg(test)]
pub fn promoting_for_test(id: &str, on: bool) {
    let mut r = lock();
    let e = r.entries.get_mut(id).unwrap();
    e.promoting = on;
    e.promoting_since = u64::MAX;
}

/// A turn of its runs.
#[cfg(test)]
pub fn set_running_for_test(id: &str) {
    lock().entries.get_mut(id).unwrap().run = Some(Run::claimed(None, 0));
}

/// Text queued for its next turn with no turn running, as `turn_over` leaves
/// it for a moment.
#[cfg(test)]
pub fn queue_for_test(id: &str, text: &str) {
    lock().entries.get_mut(id).unwrap().queue.push(Queued::from(text));
}

/// A turn begins on what is queued, as `turn_over` starts the next.
#[cfg(test)]
pub fn start_queued_for_test(id: &str) {
    let mut r = lock();
    let e = r.entries.get_mut(id).unwrap();
    e.in_flight = std::mem::take(&mut e.queue);
}

/// Its CLI's stdin has taken what the turn was started on.
#[cfg(test)]
pub fn stdin_took_for_test(id: &str) {
    assert!(stdin_took(lock().entries.get_mut(id).unwrap()));
}

/// Its turn ended without its stdin taking anything, as a CLI gone first leaves it.
#[cfg(test)]
pub fn turn_ended_for_test(id: &str) {
    turn_over(id, Some(1));
}

/// Luna has stopped its turn's process (`Run::luna_stopped`), its exit to come.
#[cfg(test)]
pub(crate) fn luna_stopped_for_test(id: &str) {
    if let Some(run) = lock().entries.get_mut(id).and_then(|e| e.run.as_mut()) {
        run.luna_stopped = true;
    }
}

/// Its turn's process exited with `code` (None: killed), as its reader finds it.
#[cfg(test)]
pub(crate) fn exited_for_test(id: &str, code: Option<i32>) {
    turn_over(id, code);
}

/// It stops on a permission, as an ask its CLI made on its stdout holds it.
#[cfg(test)]
pub fn hold_for_test(id: &str) {
    let ask = Ask::from_claude_tool("Bash", &json!({ "command": "x" }));
    lock().entries.get_mut(id).unwrap().held.push_back(Held {
        serial: 1,
        ask,
        input: Value::Null,
        tool_use_id: None,
        answer: None,
        reply: None,
    });
}

/// It stops on a question its model asked (`AskUserQuestion`), held the same way.
#[cfg(test)]
pub fn hold_question_for_test(id: &str) {
    let q = crate::agents::AskQuestion { text: "Which?".into(), options: vec!["Red".into(), "Blue".into()], multi: false, list_at: None };
    let ask = Ask::question(vec![q], false);
    lock().entries.get_mut(id).unwrap().held.push_back(Held {
        serial: 1,
        ask,
        input: Value::Null,
        tool_use_id: None,
        answer: None,
        reply: None,
    });
}

/// `luna_session_stop`: what was queued is dropped.
#[cfg(test)]
pub fn killed_for_test(id: &str) {
    stop(lock().entries.get_mut(id).unwrap());
}

/// What `helpers.json` keeps of it, written and read back as a core started later
/// finds it.
#[cfg(test)]
pub fn across_a_core_for_test(id: &str) {
    let bytes = serde_json::to_vec(&snapshot().into_iter().filter(|s| s.id == id).collect::<Vec<Saved>>()).unwrap();
    let saved: Vec<Saved> = serde_json::from_slice(&bytes).unwrap();
    let e = taken_up(saved.into_iter().next().unwrap());
    lock().entries.insert(id.to_string(), e);
}

/// `resume`: what was kept undelivered goes on in a turn by itself.
#[cfg(test)]
pub fn resume_for_test(id: &str) {
    let mut r = lock();
    let e = r.entries.get_mut(id).unwrap();
    e.in_flight = std::mem::take(&mut e.undelivered);
}

#[cfg(test)]
pub fn lose_for_test(id: &str, from: &str, text: &str) {
    lock().entries.get_mut(id).unwrap().undelivered.push(Queued::new(Some(from.to_string()), text.to_string(), Vec::new()));
}

/// A helper as a core takes it up, for the tests of what its caller is told.
#[cfg(test)]
pub fn take_up_for_test(id: &str, folder: &str, turns_ended: u32, queue: &[&str], opening: Option<&str>) {
    let spec: Spec = serde_json::from_value(json!({
        "provider": "claude", "accountPath": "C:\\acc", "folder": folder, "model": null,
        "effort": "low", "mode": "default", "autoReview": false, "tools": false, "hooksUnasked": false,
    }))
    .unwrap();
    let e = taken_up(Saved {
        id: id.to_string(),
        spec,
        started_ms: 0,
        session: opening.is_none().then(|| "sid".to_string()),
        cwd: None,
        turns_ended,
        running: true,
        queued: !queue.is_empty(),
        queue: queue.iter().map(|t| Queued::from(*t)).collect(),
        in_flight: Vec::new(),
        fed: Vec::new(),
        undelivered: Vec::new(),
        budgets_landed: Vec::new(),
        tied: true,
        left_seen_out: false,
        unsure: false,
        opening: opening.map(str::to_owned),
        opening_id: None,
        promoting_since: None,
        cut_short: false,
        observed: Default::default(),
        ties: Default::default(),
        retired_switch: None,
        no_handoff: true,
    });
    lock().entries.insert(id.to_string(), e);
}

/// For the agent layer's tests: a helper as it stands between turns, there to
/// talk to with no process behind it.
#[cfg(test)]
pub(crate) fn idle_for_test(id: &str, provider: Provider, turns_ended: u32, left_running: bool) {
    let mut e = entry(provider);
    e.turns_ended = turns_ended;
    e.left_running = left_running;
    lock().entries.insert(id.to_string(), e);
}

/// A test helper whose last turn a core's going stopped, as one taken up again
/// after a second restart is: no error of that turn's left to say so.
#[cfg(test)]
pub(crate) fn cut_for_test(id: &str) {
    let mut r = lock();
    let e = r.entries.get_mut(id).expect("a test helper");
    e.cut_short = true;
    e.error = None;
}

/// What a test helper's turns left running: each by name and pid.
#[cfg(test)]
pub(crate) fn left_for_test(id: &str, left: &[(&str, u32)]) {
    let mut r = lock();
    let e = r.entries.get_mut(id).expect("a test helper");
    e.left_running = !left.is_empty();
    e.left = left.iter().map(|(exe, pid)| Leftover { pid: *pid, exe: exe.to_string(), started_ms: 0 }).collect();
}

/// A test helper whose CLI said it stopped what its last turn left in the
/// background (`stops_background`), with `owed` owed an answer then.
#[cfg(test)]
pub(crate) fn background_stopped_for_test(id: &str, owed: &[&str]) {
    let mut r = lock();
    let e = r.entries.get_mut(id).expect("a test helper");
    let owed = owed.iter().map(|c| c.to_string()).collect();
    record_loss(&mut e.observed, e.turns_ended, StopCause::Ceiling, None, Vec::new(), owed);
}

/// A test helper whose turn `turn` took the subagents `names` with it, with
/// `owed` owed an answer then.
#[cfg(test)]
pub(crate) fn loss_for_test(id: &str, turn: u32, owed: &[&str], names: &[&str]) {
    let mut r = lock();
    let e = r.entries.get_mut(id).expect("a test helper");
    let (owed, names) = (owed.iter().map(|c| c.to_string()).collect(), names.iter().map(|n| n.to_string()).collect());
    record_loss(&mut e.observed, turn, StopCause::Ceiling, Some(LostBy::Exited), names, owed);
}

/// A line on a test helper's turn's stderr, as its reader takes it.
#[cfg(test)]
pub(crate) fn stderr_for_test(id: &str, line: &str) {
    take_stderr(id, &OutReader::default(), line.as_bytes());
}

/// A test helper as the next core takes it up from what this one saves: through
/// `helpers.json`'s JSON and back.
#[cfg(test)]
pub(crate) fn restart_for_test(id: &str) {
    let saved = snapshot().into_iter().find(|s| s.id == id).expect("a test helper");
    let saved: Saved = serde_json::from_value(serde_json::to_value(&saved).unwrap()).unwrap();
    let e = taken_up(saved);
    lock().entries.insert(id.to_string(), e);
}

/// What a test helper's job shows while its answered turn waits: each process
/// by name and pid (`Run::watched`).
#[cfg(test)]
pub(crate) fn watched_for_test(id: &str, watched: &[(&str, u32)]) {
    let mut r = lock();
    let run = r.entries.get_mut(id).and_then(|e| e.run.as_mut()).expect("a test helper in a turn");
    run.watched = watched.iter().map(|(exe, pid)| Leftover { pid: *pid, exe: exe.to_string(), started_ms: 0 }).collect();
}

/// A test helper made a lead whose CLI has written its result with a
/// background task still running, its stdin kept open for notices
/// (events.rs `keeps_listening`): what is written to it is at the other end.
#[cfg(test)]
pub(crate) fn listening_for_test(id: &str) -> std::sync::mpsc::Receiver<String> {
    let mut r = lock();
    let e = r.entries.get_mut(id).expect("a test helper");
    e.spec.tools = true;
    let lines = feeding(e);
    let run = e.run.as_mut().unwrap();
    run.background_tasks = 1;
    run.after_result = true;
    run.listening_since = Some(crate::throttle::now_ms());
    lines
}

/// An event of its Claude Code process, as the reader of its stdout takes it
/// (run.rs `take_event`), its queue moved onto stdin told to the debts.
#[cfg(test)]
pub(crate) fn claude_event_for_test(id: &str, v: &Value) {
    let moved = {
        let mut r = lock();
        let e = r.entries.get_mut(id).expect("a test helper");
        let drained = e.drained.len();
        on_claude_event(e, v);
        drained_since(e, drained)
    };
    if !moved.is_empty() {
        crate::agents::moved_onto_stdin(id, &moved);
    }
}

/// A test helper in a turn whose CLI has written its result (`waits_on_background`).
#[cfg(test)]
pub(crate) fn after_result_for_test(id: &str) {
    if let Some(run) = lock().entries.get_mut(id).and_then(|e| e.run.as_mut()) {
        run.after_result = true;
    }
}

#[cfg(test)]
pub(crate) fn gone_for_test(id: &str) {
    lock().entries.remove(id);
}

/// Its turn is over and what was queued is reserved for the next, as
/// `turn_over` reserves it, the turn's command not made yet.
#[cfg(test)]
pub(crate) fn reserved_for_test(id: &str) {
    let mut r = lock();
    let e = r.entries.get_mut(id).expect("a test helper");
    claim(e);
    reserve(e);
}

/// The reserved turn's command is about to be made (run.rs `spawn_turn`):
/// whether it is made, and the turn starting, or `overtaken` by a move.
#[cfg(test)]
pub(crate) fn launch_for_test(id: &str) -> bool {
    let mut r = lock();
    let e = r.entries.get_mut(id).expect("a test helper");
    if overtaken(e) {
        return false;
    }
    if let Some(run) = e.run.as_mut() {
        run.starting = true;
    }
    true
}

/// The turn whose command was made could not start, as `run_turn` gives one up.
#[cfg(test)]
pub(crate) fn launch_failed_for_test(id: &str) {
    if let Some(e) = lock().entries.get_mut(id) {
        e.run = None;
    }
    reg().1.notify_all();
}

/// What is queued reserved and its senders asked, as the next turn's start
/// does (`go_on`): the text that turn would start on, the claim left in place
/// and no CLI started.
#[cfg(test)]
pub(crate) fn settled_for_test(id: &str) -> Option<String> {
    let res = {
        let mut r = lock();
        let e = r.entries.get_mut(id).expect("a test helper");
        claim(e);
        reserve(e)
    };
    settle_batch(id, res).map(|b| b.text)
}

/// The helper in a turn with no process, so nothing real can be killed.
#[cfg(test)]
pub(crate) fn in_turn_for_test(id: &str) {
    if let Some(e) = lock().entries.get_mut(id) {
        claim(e);
    }
}

/// A test helper in a turn that has said nothing for `quiet` ms.
#[cfg(test)]
pub(crate) fn quiet_for_test(id: &str, quiet: u64) {
    let now = crate::throttle::now_ms();
    let mut r = lock();
    let e = r.entries.get_mut(id).expect("a test helper");
    if e.run.is_none() {
        claim(e);
    }
    if let Some(run) = e.run.as_mut() {
        run.started_ms = now - quiet;
    }
    e.observed.event_ms = now - quiet;
}

/// A helper as `agents`'s tests need one, with no process behind it: in a
/// turn or between turns, on a conversation they can write the transcript of.
#[cfg(test)]
pub fn test_helper(id: &str, account_path: &str, session: Option<&str>, busy: bool) {
    let e = Entry {
        spec: Spec {
            provider: Provider::Claude,
            account_path: account_path.into(),
            folder: account_path.into(),
            model: None,
            effort: "low".into(),
            mode: "default".into(),
            own_mode: false,
            auto_review: false,
            tools: false,
            read_only: false,
            agent: None,
            parent: None,
            connectors: false,
            constraints: Vec::new(),
            token: None,
        },
        started_ms: 0,
        session: session.map(str::to_owned),
        cwd: None,
        run: busy.then(|| Run::claimed(None, 1)),
        turns_ended: 0,
        queue: Vec::new(),
        in_flight: Vec::new(),
        fed: Vec::new(),
        drained: Vec::new(),
        log: VecDeque::new(),
        held: VecDeque::new(),
        calls: VecDeque::new(),
        error: None,
        cut_short: false,
        undelivered: Vec::new(),
        budgets_landed: Vec::new(),
        tied: false,
        resumable: false,
        unsure: false,
        opening: None,
        opening_id: None,
        job: None,
        left_running: false,
        left_unlisted: false,
        outside: Vec::new(),
        left_seen_out: false,
        left: Vec::new(),
        ending_left: false,
        promoting: false,
        promoting_since: 0,
        deleting: false,
        moving: false,
        handing: false,
        waits_since: None,
        observed: Observed::default(),
    };
    lock().entries.insert(id.to_string(), e);
}

/// A test helper's turn has ended, its process never having been.
#[cfg(test)]
pub fn test_turn_ended(id: &str) {
    let mut r = lock();
    let e = r.entries.get_mut(id).expect("a test helper");
    e.run = None;
    e.turns_ended += 1;
}

pub(super) fn entry(provider: Provider) -> Entry {
    Entry {
        spec: Spec {
            provider,
            account_path: "C:\\acc".into(),
            folder: "C:\\p".into(),
            model: Some("haiku".into()),
            effort: "low".into(),
            mode: "read-only".into(),
            own_mode: false,
            auto_review: false,
            tools: false,
            read_only: false,
            agent: None,
            parent: None,
            connectors: false,
            constraints: Vec::new(),
            token: None,
        },
        started_ms: 0,
        session: None,
        cwd: None,
        run: None,
        turns_ended: 0,
        queue: Vec::new(),
        in_flight: Vec::new(),
        fed: Vec::new(),
        drained: Vec::new(),
        log: VecDeque::new(),
        held: VecDeque::new(),
        calls: VecDeque::new(),
        error: None,
        cut_short: false,
        undelivered: Vec::new(),
        budgets_landed: Vec::new(),
        tied: false,
        resumable: false,
        unsure: false,
        opening: None,
        opening_id: None,
        job: None,
        left_running: false,
        left_unlisted: false,
        outside: Vec::new(),
        left_seen_out: false,
        left: Vec::new(),
        ending_left: false,
        promoting: false,
        promoting_since: 0,
        deleting: false,
        moving: false,
        handing: false,
        waits_since: None,
        observed: Observed::default(),
    }
}

/// An entry in a turn with no process yet, so nothing real can be killed.
pub(super) fn in_claimed_turn(id: &str) {
    let mut e = entry(Provider::Claude);
    claim(&mut e);
    lock().entries.insert(id.into(), e);
}

pub(super) fn entry_like(e: &Entry) -> Entry {
    let mut new = entry(e.spec.provider);
    new.spec = e.spec.clone();
    new
}

pub(super) fn value_of<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter().position(|a| a == flag).map(|at| args[at + 1].as_str())
}

pub(super) fn pending(uuid: &str, text: &str) -> Queued {
    let mut q = Queued::new(Some("x".into()), text.to_string(), Vec::new());
    q.id = uuid.to_string();
    q.line = Some(uuid.to_string());
    q
}

pub(super) fn with_lines(tied: bool) -> Saved {
    let spec = entry(Provider::Claude).spec;
    let mut s: Saved = serde_json::from_value(json!({
        "id": "s", "spec": serde_json::to_value(&spec).unwrap(), "startedMs": 0, "session": "sid", "cwd": null, "turnsEnded": 1,
        "running": true, "queued": false, "cutShort": false, "ties": serde_json::to_value(crate::agents::Ties::default()).unwrap(),
    }))
    .unwrap();
    s.tied = tied;
    s
}

pub(super) fn unnamed(opening: &str, id: Option<&str>) -> Saved {
    let mut s = with_lines(true);
    s.session = None;
    s.opening = Some(opening.to_string());
    s.opening_id = id.map(str::to_owned);
    s
}

/// A Claude Code turn with its stdin open: what goes down `feed` is at the
/// other end, and the process has no pid to kill.
pub(super) fn feeding(e: &mut Entry) -> std::sync::mpsc::Receiver<String> {
    let (feed, lines) = std::sync::mpsc::channel();
    e.run = Some(Run { feed: Some(feed), line: Some("opening-line".into()), ..Run::claimed(None, 0) });
    lines
}

/// A pipe its writer has left: what was written, then a read that blocks
/// for as long as something else holds it open, as a stray job does.
pub(super) struct Pipe(pub(super) std::sync::mpsc::Receiver<Vec<u8>>);

impl std::io::Read for Pipe {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let Ok(bytes) = self.0.recv() else { return Ok(0) };
        buf[..bytes.len()].copy_from_slice(&bytes);
        Ok(bytes.len())
    }
}

/// Reads `lines` off such a pipe, each event taking `each` to handle, and
/// returns once `await_last_words` does: what was handled by then.
pub(super) fn drained_with_a_stray(lines: &[&str], each: std::time::Duration) -> Vec<String> {
    use std::sync::{Arc, Mutex};
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    for line in lines {
        tx.send(format!("{line}\n").into_bytes()).unwrap();
    }
    let reader = Arc::new(OutReader::default());
    let got = Arc::new(Mutex::new(Vec::new()));
    let (drained, read_out) = std::sync::mpsc::channel::<()>();
    let (on_it, into) = (reader.clone(), got.clone());
    std::thread::spawn(move || {
        let _drained = drained;
        on_it.read_events(Pipe(rx), |v| {
            std::thread::sleep(each);
            into.lock().unwrap().push(v["type"].as_str().unwrap_or("").to_string());
            true
        });
    });
    reader.await_last_words(&read_out, std::time::Duration::from_millis(150), std::time::Duration::from_secs(20));
    let seen = got.lock().unwrap().clone();
    drop(tx); // Only now does the stray let go.
    seen
}

/// A stdin that takes what `pass` lets through, one write at a time.
pub(super) struct Stalled {
    pub(super) pass: std::sync::mpsc::Receiver<()>,
    pub(super) wrote: std::sync::Arc<Mutex<Vec<u8>>>,
}

impl Write for Stalled {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.pass.recv().map_err(|_| std::io::ErrorKind::BrokenPipe)?;
        self.wrote.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A Codex test helper in a turn on thread `thread`, begun at `started_ms`,
/// its rollouts under the account folder `account`.
#[cfg(test)]
pub(crate) fn codex_in_turn_for_test(id: &str, account: &str, thread: &str, started_ms: u64) {
    let mut e = entry(Provider::Codex);
    e.spec.account_path = account.into();
    e.session = Some(thread.into());
    e.run = Some(Run::claimed(None, started_ms));
    lock().entries.insert(id.to_string(), e);
}

/// A test helper's turn's process exits with `code`.
#[cfg(test)]
pub(crate) fn turn_over_for_test(id: &str, code: Option<i32>) {
    turn_over(id, code);
}

/// Its turn's prompt told it of move `moved` (agents/reparent.rs `move_note`),
/// and its model has answered it or not, for the tests of when that is told.
#[cfg(test)]
pub(crate) fn carried_move_for_test(id: &str, moved: crate::agents::Moved, acted: bool) {
    let mut r = lock();
    let e = r.entries.get_mut(id).expect("a test helper");
    if e.run.is_none() {
        claim(e);
    }
    e.run.as_mut().unwrap().moved = Some(moved);
    e.observed.acted = acted;
}
