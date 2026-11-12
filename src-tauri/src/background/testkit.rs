//! The hooks other modules' tests drive helpers through, and the helpers this
//! module's own tests share.

use super::*;

/// Whom it is told it answers to, for the tests of its passing to another.
#[cfg(test)]
pub fn told_parent_for_test(id: &str) -> Option<String> {
    lock().entries.get(id)?.spec.parent.clone()
}

/// Text that never reached a helper, by whom, for the tests of who hears of it.
/// A notice with `mark` in it, given to a helper as `send_marked` does, the part
/// that starts a CLI aside. Returns whether it was queued.
#[cfg(test)]
pub fn place_for_test(id: &str, text: &str, mark: BudgetMark) -> bool {
    place(lock().entries.get_mut(id).unwrap(), Queued::new(None, text.to_string(), vec![mark]), false) != Placed::Started
}

/// The texts queued for its next turn, in order.
#[cfg(test)]
pub fn queued_for_test(id: &str) -> Vec<String> {
    lock().entries.get(id).map(|e| e.queue.iter().map(|q| q.text.clone()).collect()).unwrap_or_default()
}

/// Being opened as a chat by the person, or not.
#[cfg(test)]
pub fn promoting_for_test(id: &str, on: bool) {
    lock().entries.get_mut(id).unwrap().promoting = on;
}

/// A turn of its runs.
#[cfg(test)]
pub fn set_running_for_test(id: &str) {
    lock().entries.get_mut(id).unwrap().run = Some(Run::claimed(None, 0));
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

/// It stops on a permission, as a turn's `luna_permission` holds it.
#[cfg(test)]
pub fn hold_for_test(id: &str) {
    let ask = Ask::from_claude_tool("Bash", &json!({ "command": "x" }));
    lock().entries.get_mut(id).unwrap().held.push_back(Held { serial: 1, ask, input: Value::Null, answer: None });
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
        "provider": "claude", "accountPath": "C:\\acc", "folder": folder, "worktree": false, "model": null,
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
        unsure: false,
        opening: opening.map(str::to_owned),
        opening_id: None,
        promoting_since: None,
        cut_short: false,
        observed: Default::default(),
        ties: Default::default(),
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

/// What a test helper's turns left running: each by name and pid, quiet for so
/// many ms.
#[cfg(test)]
pub(crate) fn left_for_test(id: &str, left: &[(&str, u32, u64)]) {
    let now = crate::throttle::now_ms();
    let mut r = lock();
    let e = r.entries.get_mut(id).expect("a test helper");
    e.left_running = !left.is_empty();
    e.left = left
        .iter()
        .map(|(exe, pid, quiet)| Leftover {
            pid: *pid,
            exe: exe.to_string(),
            started_ms: 0,
            cpu_ms: 0,
            io_bytes: 0,
            read_ms: now,
            seen_ms: now,
            moved_ms: now - quiet,
        })
        .collect();
}

/// A test helper whose CLI said it stopped what its last turn left in the
/// background (`stops_background`).
#[cfg(test)]
pub(crate) fn background_stopped_for_test(id: &str) {
    let mut r = lock();
    let e = r.entries.get_mut(id).expect("a test helper");
    e.observed.background_stopped = true;
    e.observed.stopped_turn = e.turns_ended;
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
/// by name and pid, quiet for so many ms (`Run::watched`).
#[cfg(test)]
pub(crate) fn watched_for_test(id: &str, watched: &[(&str, u32, u64)]) {
    let now = crate::throttle::now_ms();
    let mut r = lock();
    let run = r.entries.get_mut(id).and_then(|e| e.run.as_mut()).expect("a test helper in a turn");
    run.watched = watched
        .iter()
        .map(|(exe, pid, quiet)| Leftover {
            pid: *pid,
            exe: exe.to_string(),
            started_ms: 0,
            cpu_ms: 0,
            io_bytes: 0,
            read_ms: now,
            seen_ms: now,
            moved_ms: now - quiet,
        })
        .collect();
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
            worktree: false,
            worktree_name: None,
            model: None,
            effort: "low".into(),
            mode: "default".into(),
            auto_review: false,
            tools: false,
            hooks_unasked: false,
            read_only: false,
            agent: None,
            parent: None,
            connectors: false,
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
        log: VecDeque::new(),
        held: VecDeque::new(),
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
        left: Vec::new(),
        ending_left: false,
        promoting: false,
        promoting_since: 0,
        deleting: false,
        handing: false,
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
            worktree: false,
            worktree_name: None,
            model: Some("haiku".into()),
            effort: "low".into(),
            mode: "read-only".into(),
            auto_review: false,
            tools: false,
            hooks_unasked: false,
            read_only: false,
            agent: None,
            parent: None,
            connectors: false,
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
        log: VecDeque::new(),
        held: VecDeque::new(),
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
        left: Vec::new(),
        ending_left: false,
        promoting: false,
        promoting_since: 0,
        deleting: false,
        handing: false,
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
