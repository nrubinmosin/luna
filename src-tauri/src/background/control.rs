//! Stopping a helper: a kill, an interrupt, and what its CLI left running.

use super::*;

/// Ends what a helper's turns left running, once the helper is done with.
pub(super) fn end_leftovers(id: &str, job: Option<&crate::procs::Job>) {
    let Some(job) = job else { return };
    let ended = crate::procs::end_leftovers(job, None);
    if !ended.is_empty() {
        crate::log::info("background", &format!("{}: ended what its turns left running: {}", crate::agents::label(id), ended.join(", ")));
    }
}

/// What a turn stopped by `luna_session_stop` or the window ends on: its caller
/// hears it was stopped.
pub(crate) const STOPPED_BY_STOP: &str = "it was stopped, by luna_session_stop or from the window";

/// What a turn cut short by a send with `interrupt` ends on: stopped, not
/// failed, whether its CLI stopped it as asked or its process was killed.
pub(crate) const INTERRUPTED: &str = "it was interrupted, by a send with interrupt";

/// Stops the turn in progress; the conversation stays for the next one. A
/// turn is asked to stop first (Claude Code's `interrupt` control request, a
/// Codex server's `turn/interrupt`), and its process killed only if it does
/// not end in time (`end_turn_process`). Between
/// turns there is no turn to stop, and what its turns left running is what is
/// left of it to stop: that is ended, and what went is returned by name.
pub fn kill(id: &str) -> Vec<String> {
    kill_holding(id, ())
}

/// `kill`, with `held` let go once the turn is marked stopped, before its
/// process and what it left running are waited on: what the caller checked
/// the stop against holds until it is made (agents/teardown.rs `kill`).
pub fn kill_holding<T>(id: &str, held: T) -> Vec<String> {
    let (pid, job, pin) = {
        let mut r = lock();
        let Some(e) = r.entries.get_mut(id) else { return Vec::new() };
        // Whoever is owed the turn hears it was stopped, not an exit code.
        if e.run.is_some() {
            e.error = Some(STOPPED_BY_STOP.into());
        }
        let job = e.job.clone().filter(|_| e.run.is_none() && e.left_running && !e.ending_left);
        // Only the Stop that takes the ending on sets the mark, and only it
        // lets it go: a second one meanwhile would let a send start a turn in
        // the job the first is still clearing.
        if job.is_some() {
            e.ending_left = true;
        }
        // Its CLI asked first, while it still reads its stdin, which `stop`
        // then closes.
        let asked = ask_to_stop(e);
        let pid = stop(e);
        // Under the lock, while the turn's own handle still holds the pid: a
        // turn that ends a moment later then leaves it to nobody else.
        (pid.map(|pid| (pid, asked)), job, pid.map(crate::procs::pin))
    };
    drop(held);
    reg().1.notify_all();
    // Its turn's exit is Luna's doing, not its CLI version's.
    crate::cli::session_stopped(id);
    match pid {
        // The wait for its CLI is not the caller's: on a thread of its own,
        // the pid held as it was taken.
        Some((pid, true)) => {
            let helper = id.to_string();
            let waits = std::thread::Builder::new().name(format!("luna-bg-stop-{id}")).spawn(move || {
                let _pin = pin;
                end_turn_process(&helper, pid, true);
            });
            if waits.is_err() {
                crate::pty::kill_tree(Some(pid));
            }
        }
        Some((pid, false)) => crate::pty::kill_tree(Some(pid)),
        None => {}
    }
    let Some(job) = job else {
        settle_unlisted(id);
        return Vec::new();
    };
    end_left_marked(id, &job)
}

/// A stop on a helper that is between turns with no job and what its turns
/// left not known (`left_unlisted`: taken up by a core that was not shown the
/// old job out, which nothing in the record can find again) is the way out of
/// that: the turns that were in no job are walked below by parent links, as
/// `lives` does, and where the walk finds nothing, which a helper with no
/// pid to walk from always does, what it left is settled as not found.
/// Anything the walk does find stays, and stays active.
fn settle_unlisted(id: &str) {
    let turns: Vec<(u32, u64)> = {
        let r = lock();
        let Some(e) = r
            .entries
            .get(id)
            .filter(|e| e.run.is_none() && e.job.is_none() && !e.ending_left && (e.left_unlisted || !e.outside.is_empty()))
        else {
            return;
        };
        e.outside.clone()
    };
    // Read with no lock held: it takes a snapshot of every process.
    let reads = turns.into_iter().map(|(pid, since)| ((pid, since), crate::procs::left_below(pid, since))).collect();
    let settled = {
        let mut r = lock();
        let Some(e) = r.entries.get_mut(id).filter(|e| e.run.is_none() && e.job.is_none()) else { return };
        see_outside(e, reads);
        let settled = e.outside.is_empty() && e.left_unlisted;
        if settled {
            e.left_unlisted = false;
            e.left_seen_out = true;
        }
        settled
    };
    if settled {
        crate::log::info(
            "background",
            &format!("{}: stopped; what its turns left could not be found, and is taken as gone", crate::agents::label(id)),
        );
        crate::activity::touch();
        changed();
    }
}

/// Ends what helper `id`'s turns left running in `job`, between turns, its
/// `ending_left` mark taken by the caller and let go here: what was ended is
/// waited on to leave the job, what stays is noted, and what was sent
/// meanwhile goes in as the next turn. Returns what went, by name.
fn end_left_marked(id: &str, job: &crate::procs::Job) -> Vec<String> {
    let ended = crate::procs::end_leftovers(job, None);
    // A process ended is still in the job for a moment, as the OS tears it down.
    let mut still = crate::procs::left_in(job);
    for _ in 0..20 {
        if still.as_ref().is_some_and(|s| s.is_empty()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        still = crate::procs::left_in(job);
    }
    #[cfg(test)]
    tests::while_ending(id);
    let next = {
        let mut r = lock();
        let Some(e) = r.entries.get_mut(id) else { return ended };
        e.ending_left = false;
        note_left(e, still);
        // What was sent meanwhile is the next turn's, as after any turn, once
        // its line has a place for it.
        claim_queued(&mut r, id)
    };
    let next = next.and_then(|res| settle_batch(id, res)).map(|b| b.text);
    if !ended.is_empty() {
        crate::log::info(
            "background",
            &format!("{}: stopped between turns, which ended what its turns left running: {}", crate::agents::label(id), ended.join(", ")),
        );
    }
    if let Some(text) = next {
        run_waited(id, &text);
    }
    changed();
    ended
}

/// Before an update is installed (`pty::stop_all_sessions`): every turn in
/// progress is stopped as `stop_turns` stops it, and once those turns are over
/// (their end is what notes what each left), or `STOP_WAIT` is up, what helpers
/// between turns have left running is ended too. A turn may start again after
/// (text sent meanwhile, or the update called off): it is a session the
/// updated window asks about before it replaces the old core, as it should be.
/// What only helpers' turns left, here or by a turn that ends later, is told
/// apart from sessions (`running_counts`) and does not hold up the old core's
/// replacement, which ends it. Returns how many turns it stopped.
pub fn stop_for_update() -> usize {
    stop_for_update_of(&|_| true, STOP_WAIT)
}

/// `stop_for_update` for the helpers `of` picks, their turns given `wait` to end.
fn stop_for_update_of(of: &dyn Fn(&str) -> bool, wait: std::time::Duration) -> usize {
    let stopped = stop_turns_of(false, of);
    end_left_after_turns(of, &stopped, wait);
    stopped.len()
}

/// Of the helpers `of` picks: once none is in a turn the stop ended
/// (`stopped`, by helper and claim, `Run::claim`), claimed or with a process,
/// or `wait` is up, what their turns left running is ended, between turns, as a
/// stop between turns ends it (`kill`), all of them at once, as each waits a
/// moment for what it ended to leave. A helper in a turn still, the one stopped
/// or one started since, or one a stop is clearing already, is not touched: a
/// turn's end may take up what was queued behind it at once, into that same
/// job. Returns what went, by name.
fn end_left_after_turns(of: &dyn Fn(&str) -> bool, stopped: &[(String, u64)], wait: std::time::Duration) -> Vec<String> {
    let deadline = std::time::Instant::now() + wait;
    let ours = |id: &String, e: &Entry| of(id) && e.run.as_ref().is_some_and(|run| stopped.iter().any(|(i, c)| i == id && *c == run.claim));
    let (m, cv) = reg();
    let mut r = m.lock().unwrap_or_else(|e| e.into_inner());
    let mut late = Vec::new();
    while r.entries.iter().any(|(id, e)| ours(id, e)) {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            late = r.entries.iter().filter(|(id, e)| ours(id, e)).map(|(id, _)| crate::agents::label(id)).collect();
            break;
        }
        r = cv.wait_timeout(r, left).unwrap_or_else(|e| e.into_inner()).0;
    }
    let jobs: Vec<(String, std::sync::Arc<crate::procs::Job>)> = r
        .entries
        .iter_mut()
        .filter(|(id, _)| of(id))
        .filter_map(|(id, e)| {
            let job = e.job.clone().filter(|_| e.run.is_none() && e.left_running && !e.ending_left)?;
            // Taken as `kill` takes it: a send meanwhile waits for the end.
            e.ending_left = true;
            Some((id.clone(), job))
        })
        .collect();
    drop(r);
    if !late.is_empty() {
        crate::log::warn("background", &format!("turns stopped for an update not over within {wait:?}: {}", late.join(", ")));
    }
    std::thread::scope(|s| {
        let ends: Vec<_> = jobs.iter().map(|(id, job)| s.spawn(move || end_left_marked(id, job))).collect();
        ends.into_iter().flat_map(|end| end.join().unwrap_or_default()).collect()
    })
}

/// The turn on `text`, a batch of what waited that was reserved for it. One
/// that cannot start keeps that text as undelivered, for its senders to hear
/// of with their next notice, as a turn cut short does: it waited through a
/// hold or a stop already, and what will not start (its folder gone, its CLI
/// missing) is often what stopped the turn before.
fn run_waited(id: &str, text: &str) {
    save();
    if run_turn(id, text).is_err() {
        if let Some(e) = lock().entries.get_mut(id) {
            let batch = std::mem::take(&mut e.in_flight);
            e.undelivered.extend(batch);
        }
        save();
    }
}

/// Between turns, what its turns left running, for `luna_session_get`.
pub(super) fn left_shown(e: &Entry) -> Vec<String> {
    if e.run.is_some() {
        return Vec::new();
    }
    e.left.iter().map(Leftover::label).collect()
}

/// Between turns, what its turns left running, by name for a notice; None with
/// nothing left, or in a turn.
pub fn left_named(id: &str) -> Option<String> {
    let r = lock();
    let e = r.entries.get(id).filter(|e| e.run.is_none() && e.left_running && !e.left.is_empty())?;
    Some(left_names(&e.left))
}

/// Why its last turn stopped short on an error, if it did: not Luna's stop
/// (`stop_of`).
pub fn last_error(id: &str) -> Option<String> {
    lock().entries.get(id)?.error.clone().filter(|why| !is_stop(why))
}

/// Whether a turn's error is Luna's stop of it, not the CLI's error.
pub(crate) fn is_stop(why: &str) -> bool {
    why == STOPPED_BY_STOP || why == INTERRUPTED || why == STOPPED
}

/// Why Luna stopped its last turn, if it did and no turn has ended since:
/// kept past the start of the next (`Observed::stopped`), so that a send
/// with `interrupt`, whose text starts a turn at once, does not take it from
/// whoever was owed the turn it cut.
pub fn stop_of(id: &str) -> Option<String> {
    let r = lock();
    let e = r.entries.get(id)?;
    e.observed.stopped.as_ref().filter(|(turn, _)| *turn == e.turns_ended).map(|(_, why)| why.clone())
}

/// How an `interrupt` went.
#[derive(Debug, PartialEq, Clone, Copy)]
pub struct Interrupted {
    /// The turn it was on was stopped, and the text starts the next.
    pub stopped: bool,
    /// The text waits behind a turn that goes on: one that would not stop in
    /// time, or that has no process to stop yet. It ends a turn after that one.
    pub queued: bool,
    /// It had no turn to stop, and its next, on the text, waits for a place on
    /// its line (places.rs): the next turn to end is the one on it.
    pub waits: bool,
}

/// How long a turn claimed a moment ago gets to have a process to stop.
const CLAIM_WAIT: std::time::Duration = std::time::Duration::from_secs(3);
/// How long a Claude Code turn asked to stop (`ask_to_stop`) gets to write
/// its `result` (2.1.325: at once, its tools stopped), and then to go with
/// its stdin closed (2.1.325: about 2 s), before it is killed.
const ASKED_RESULT_WAIT: std::time::Duration = std::time::Duration::from_secs(5);
const ASKED_EXIT_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// The `interrupt` control request, on the stdin of a Claude Code turn's
/// process that still reads it, with its model at work (2.1.325: a
/// `control_response` and then the turn's `result`, `error_during_execution`,
/// its tools stopped as Esc stops them, the conversation kept for the next
/// turn's `--resume`). Not once its `result` is in: what keeps it up then is
/// its background work, which an interrupt does not end. Whether it was
/// written; the caller closes stdin after it, and the process goes once the
/// turn is over (`end_turn_process`).
/// A Codex server's turn is asked by `turn/interrupt` (appserver.rs
/// `ask_server_to_stop`), its stdin left open for its `turn/completed`.
fn ask_to_stop(e: &mut Entry) -> bool {
    if e.spec.provider == Provider::Codex {
        return ask_server_to_stop(e);
    }
    let Some(run) = e.run.as_mut().filter(|run| run.pid.is_some() && !run.after_result) else { return false };
    let Some(feed) = run.feed.as_ref() else { return false };
    let request = format!("luna-interrupt-{}", run.claim);
    let line = json!({ "type": "control_request", "request_id": request, "request": { "subtype": "interrupt" } });
    if feed.send(format!("{line}\n")).is_err() {
        return false;
    }
    run.interrupt = Some(request);
    note(e, "asked its CLI to stop the turn");
    true
}

/// Sees the process `pid` of helper `id`'s turn out: one asked to stop
/// (`ask_to_stop`) gets `ASKED_RESULT_WAIT` for its turn's `result` and
/// `ASKED_EXIT_WAIT` more to go, as it does with its stdin closed; then, or at
/// once if not asked, it is killed with its tree, as what its turn left in
/// the background is.
fn end_turn_process(id: &str, pid: u32, asked: bool) {
    // A Codex turn: its own grace, and what it ran swept once it is over.
    if lock().entries.get(id).is_some_and(|e| e.spec.provider == Provider::Codex) {
        return end_stopped_turn(id, pid, asked);
    }
    let kill = !asked || {
        let answered = wait_on_run(id, pid, ASKED_RESULT_WAIT, |run| run.after_result);
        let gone = (answered != RunWait::TimedOut).then(|| wait_on_run(id, pid, ASKED_EXIT_WAIT, |_| false));
        let kill = kills_after_asking(answered, gone);
        crate::log::info(
            "background",
            &format!(
                "{}: {}",
                crate::agents::label(id),
                match (kill, answered) {
                    (false, _) => "its CLI stopped the turn as asked",
                    (true, RunWait::Removed) => "deleted while its CLI was asked to stop: its process is killed",
                    (true, RunWait::TimedOut) => "its CLI wrote no result after Luna's interrupt: its process is killed",
                    (true, _) => "its CLI did not go after Luna's interrupt: its process is killed",
                }
            ),
        );
        kill
    };
    if kill {
        crate::pty::kill_tree(Some(pid));
    }
}

/// How a wait on a turn's process (`wait_on_run`) came out.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum RunWait {
    /// The turn is over: its process has gone, its end recorded.
    Over,
    /// The turn is where the wait asked for, its process still up.
    Reached,
    /// The helper is gone from the registry (deleted, `remove`): nothing
    /// says whether its process is.
    Removed,
    TimedOut,
}

/// Whether a turn asked to stop is killed after all, by how the waits for
/// its `result` (`answered`) and then its exit (`gone`, where the first did
/// not time out) came out: only a turn seen over is spared. A helper deleted
/// meanwhile is no sign its process went: its turn may have had no job for
/// the delete to end.
fn kills_after_asking(answered: RunWait, gone: Option<RunWait>) -> bool {
    !matches!((answered, gone), (RunWait::Over, _) | (RunWait::Reached, Some(RunWait::Over)))
}

/// Waits up to `wait` for the turn of helper `id` on process `pid` to be
/// over, or to be where `done` says.
fn wait_on_run(id: &str, pid: u32, wait: std::time::Duration, done: impl Fn(&Run) -> bool) -> RunWait {
    let deadline = std::time::Instant::now() + wait;
    let (m, cv) = reg();
    let mut r = m.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        let Some(e) = r.entries.get(id) else { return RunWait::Removed };
        match e.run.as_ref().filter(|run| run.pid == Some(pid)) {
            None => return RunWait::Over,
            Some(run) if done(run) => return RunWait::Reached,
            Some(_) => {}
        }
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return RunWait::TimedOut;
        }
        // Events do not wake this: a short look at a time.
        r = cv.wait_timeout(r, left.min(std::time::Duration::from_millis(100))).unwrap_or_else(|e| e.into_inner()).0;
    }
}
/// How long a stopped turn's process gets to read as gone.
const STOP_WAIT: std::time::Duration = std::time::Duration::from_secs(15);

/// Stops the turn in progress and starts the next on `text`, behind whatever
/// was queued for after it: the conversation goes on, the work the turn was
/// doing does not. All of it waits in the queue, which the turn's end takes up
/// whole, so nobody's text hangs on how the stop goes: what would not stop
/// still has it all behind it.
pub fn interrupt(id: &str, text: &str, from: Option<&str>) -> Result<Interrupted, String> {
    let (free, waits) = {
        let mut r = lock();
        let room = room_for_turn(&r, id, from.is_none());
        let e = r.entries.get_mut(id).ok_or("no such helper")?;
        room_for(id, e, text)?;
        e.held.clear();
        let free = e.run.is_none() && !e.ending_left && !e.held_back();
        // Between turns it has no turn to stop: the text is its next turn's,
        // which waits for a place as any does (places.rs), behind what waits
        // for one already.
        let waits = free && (!room || !e.queue.is_empty());
        if free && !waits {
            claim(e);
        } else {
            note(e, "interrupted");
            e.queue.push(Queued::new(from.map(str::to_owned), text.to_string(), Vec::new()));
            if waits {
                wait_for_place(e);
            }
        }
        (free, waits)
    };
    reg().1.notify_all();
    // In the queue it is what a core killed now hands back to its sender.
    save();
    if waits {
        changed();
        return Ok(Interrupted { stopped: false, queued: false, waits: true });
    }
    if free {
        run_turn(id, text)?;
        changed();
        return Ok(Interrupted { stopped: false, queued: false, waits: false });
    }
    let queued = Interrupted { stopped: false, queued: true, waits: false };
    let deadline = std::time::Instant::now() + CLAIM_WAIT;
    let (pid, asked, _pin) = loop {
        {
            let mut r = lock();
            let e = r.entries.get_mut(id).ok_or("no such helper")?;
            // Taken up already: that turn ended by itself, and this is the next.
            if e.queue.is_empty() {
                return Ok(queued);
            }
            match e.run.as_ref().map(|run| run.pid) {
                // Pinned under the lock, as `kill` does.
                Some(Some(pid)) => {
                    if let Some(run) = e.run.as_mut() {
                        run.luna_stopped = true;
                    }
                    e.error = Some(INTERRUPTED.into());
                    // Its turn's exit is Luna's doing, not its CLI version's
                    // (cli/canary.rs), however it goes.
                    crate::cli::session_stopped(id);
                    // Its CLI asked first. Claude Code's stdin closed behind
                    // the request, so that the queue starts the next process;
                    // a Codex server's stays open for its `turn/completed`.
                    let asked = ask_to_stop(e);
                    if let Some(run) = e.run.as_mut().filter(|run| asked && run.rpc.is_none()) {
                        run.feed = None;
                        run.listening_since = None;
                    }
                    drop_dead_asks(e);
                    break (pid, asked, crate::procs::pin(pid));
                }
                Some(None) if std::time::Instant::now() < deadline => {}
                _ => return Ok(queued),
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    end_turn_process(id, pid, asked);
    // The turn is over once its reader has seen the process go.
    let deadline = std::time::Instant::now() + STOP_WAIT;
    let (m, cv) = reg();
    let mut r = m.lock().unwrap_or_else(|e| e.into_inner());
    while r.entries.get(id).and_then(|e| e.run.as_ref()).is_some_and(|run| run.pid == Some(pid)) {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            drop(r);
            changed();
            return Ok(queued);
        }
        r = cv.wait_timeout(r, left).unwrap_or_else(|e| e.into_inner()).0;
    }
    drop(r);
    changed();
    Ok(Interrupted { stopped: true, queued: false, waits: false })
}

/// Ends the turn of process `pid` that a stop took off: its tree killed, or,
/// a Codex turn `asked` to stop (`turn/interrupt`), killed only if it has not
/// ended within INTERRUPT_GRACE and its turn has not completed either (a
/// server that lingers past `turn/completed` is `see_out`'s to end, alone).
/// An interrupt leaves the turn's command running until the server goes
/// (0.169.0, seen live: its `pwsh` went only with the server), so what the
/// turn started of its own and left is ended then, as a kill would have.
fn end_stopped_turn(id: &str, pid: u32, asked: bool) {
    // When the turn began, and whether it has completed, while it is this turn.
    let of_turn = || {
        let r = lock();
        let run = r.entries.get(id)?.run.as_ref().filter(|run| run.pid == Some(pid))?;
        Some((run.started_ms, run.rpc.as_ref().is_some_and(|rpc| rpc.done)))
    };
    let since = of_turn().map(|(since, _)| since);
    // What its turns had left before this one, by pid and start: off
    // Windows what to spare, where no parent links say what is this turn's.
    #[cfg(not(windows))]
    let before: Vec<(u32, u64)> =
        lock().entries.get(id).map(|e| e.left.iter().map(|l| (l.pid, l.started_ms)).collect()).unwrap_or_default();
    if !asked {
        return crate::pty::kill_tree(Some(pid));
    }
    let ended = over_within(id, pid, INTERRUPT_GRACE);
    let completed = of_turn().is_some_and(|(_, done)| done);
    if !ended && !completed {
        return crate::pty::kill_tree(Some(pid));
    }
    let job = lock().entries.get(id).and_then(|e| e.job.clone());
    if let (Some(job), Some(since)) = (job, since) {
        // Windows: what descends from the server, born in the turn. Elsewhere
        // the keeper's walk of the job (the turns' process groups, sessions
        // and its mark), all but what earlier turns left.
        #[cfg(windows)]
        let left = crate::procs::end_tree_of(&job, pid, since);
        #[cfg(not(windows))]
        let left = {
            let _ = since;
            crate::procs::end_all_but(&job, pid, &before)
        };
        if !left.is_empty() {
            crate::log::info(
                "background",
                &format!("{}: ended what its interrupted turn left: {}", crate::agents::label(id), left.join(", ")),
            );
        }
    }
}

/// Waits up to `wait` for the turn of process `pid` to be over (`turn_over`
/// takes its run): whether it is.
pub(super) fn over_within(id: &str, pid: u32, wait: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + wait;
    let (m, cv) = reg();
    let mut r = m.lock().unwrap_or_else(|e| e.into_inner());
    while r.entries.get(id).and_then(|e| e.run.as_ref()).is_some_and(|run| run.pid == Some(pid)) {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return false;
        }
        r = cv.wait_timeout(r, left).unwrap_or_else(|e| e.into_inner()).0;
    }
    true
}

/// A delete's hold on a helper (`claim_delete`), let go when dropped: by a delete
/// refused, the helper goes on as it was; by one done, it is gone already.
pub struct DeleteClaim {
    id: String,
    held: bool,
}

impl DeleteClaim {
    /// Whether there was a helper to hold: none, the id is a record alone.
    pub fn held(&self) -> bool {
        self.held
    }
}

impl Drop for DeleteClaim {
    fn drop(&mut self) {
        if !self.held {
            return;
        }
        let there = match lock().entries.get_mut(&self.id) {
            Some(e) => {
                e.deleting = false;
                true
            }
            None => false,
        };
        if there {
            go_on(&self.id);
        }
    }
}

/// Holds helper `id` for a delete, from before it looks at anything until it has
/// removed it or let it be (agents/teardown.rs): no turn starts meanwhile, and the
/// person cannot open it as a chat, so what the delete went by still holds when it
/// removes it. Refused while it is being opened, or deleted by another. With no
/// helper by that id, a claim on nothing (`held` false).
pub fn claim_delete(id: &str) -> Result<DeleteClaim, String> {
    let mut r = lock();
    let Some(e) = r.entries.get_mut(id) else { return Ok(DeleteClaim { id: id.to_string(), held: false }) };
    if e.promoting {
        return Err(format!("the person is opening {id} as a chat of their own; it is theirs once it opens"));
    }
    if e.deleting {
        return Err(format!("{id} is being deleted already"));
    }
    if e.moving {
        return Err(format!("the person is moving {id} to another chat; try again once that is through"));
    }
    e.deleting = true;
    Ok(DeleteClaim { id: id.to_string(), held: true })
}

/// A move's hold on the helpers it moves (`claim_move`), let go when dropped.
pub struct MoveClaim {
    held: Vec<String>,
    /// The helpers whose claimed turn it overtook (`Run::overtaken`).
    overtook: Vec<String>,
    /// The move was made (`moved`): an overtaken turn waits for the next.
    moved: bool,
}

impl MoveClaim {
    /// The move went through: a turn it overtook starts after it, under the
    /// new parent. Not said, the move changed nothing, and such a turn starts
    /// as it was claimed.
    pub fn moved(&mut self) {
        self.moved = true;
    }
}

impl Drop for MoveClaim {
    fn drop(&mut self) {
        let mut held = Vec::new();
        {
            let mut r = lock();
            for id in &self.held {
                if let Some(e) = r.entries.get_mut(id) {
                    e.moving = false;
                    held.push(id.clone());
                }
            }
            for id in self.overtook.iter().filter(|_| !self.moved) {
                if let Some(run) = r.entries.get_mut(id).and_then(|e| e.run.as_mut()).filter(|run| run.pid.is_none()) {
                    run.overtaken = false;
                }
            }
        }
        for id in held {
            go_on(&id);
        }
    }
}

/// Holds every helper of `ids` for a move to another chat (agents/reparent.rs),
/// all or none: no turn starts on any of them meanwhile, so the next one starts
/// under its new parent, and none is opened as a chat or deleted under it. A turn
/// running goes on. Refused, naming the helper, while one is being opened as a
/// chat, deleted or moved already (`held_back`). An id with no helper record is
/// passed over.
///
/// A turn whose command is made but whose process is not up yet (`Run::starting`)
/// is let start first, up to LAUNCH_WAIT: it goes on under the parent of before,
/// as one already running does. One claimed whose command is not made yet is
/// `overtaken`: it does not start, and what it was to start on waits for the move
/// to let go (run.rs `spawn_turn`).
pub fn claim_move(ids: &[String]) -> Result<MoveClaim, String> {
    let deadline = std::time::Instant::now() + LAUNCH_WAIT;
    let mut r = lock();
    loop {
        let starting = |id: &String| r.entries.get(id).and_then(|e| e.run.as_ref()).is_some_and(|run| run.starting && run.pid.is_none());
        let Some(id) = ids.iter().find(|id| starting(id)).cloned() else { break };
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return Err(format!("{id} is starting a turn and its process is not up yet; not moved. Try again in a moment"));
        }
        r = reg().1.wait_timeout(r, left).map_or_else(|e| e.into_inner().0, |(r, _)| r);
    }
    if let Some(why) = claim_move_check(&r, ids) {
        return Err(why);
    }
    let (mut held, mut overtook) = (Vec::new(), Vec::new());
    for id in ids {
        if let Some(e) = r.entries.get_mut(id) {
            e.moving = true;
            if let Some(run) = e.run.as_mut().filter(|run| run.pid.is_none() && !run.overtaken) {
                run.overtaken = true;
                overtook.push(id.clone());
            }
            held.push(id.clone());
        }
    }
    Ok(MoveClaim { held, overtook, moved: false })
}

/// How long a move waits for a turn whose command is made to have its process.
const LAUNCH_WAIT: std::time::Duration = std::time::Duration::from_secs(15);

/// Why the helpers of `ids` cannot be moved now (`claim_move`'s refusal), for
/// the window's list of targets; None if nothing holds them back.
pub fn move_held_back(ids: &[String]) -> Option<String> {
    claim_move_check(&lock(), ids)
}

fn claim_move_check(r: &Registry, ids: &[String]) -> Option<String> {
    ids.iter().find_map(|id| {
        let e = r.entries.get(id)?;
        let why = if e.promoting {
            "the person is opening it as a chat of their own"
        } else if e.deleting {
            "it is being deleted"
        } else if e.moving {
            "it is being moved already"
        } else {
            return None;
        };
        Some(format!("{id} is held back ({why}); not moved"))
    })
}

/// Whether a parent has an answer to come from helper `id` as it stands (a turn
/// running, text queued or on its way in, or a question or permission it
/// holds), and the turns it has ended: what a helper moved to another chat owes
/// that chat, and from which turn (agents/reparent.rs). One look, as a turn
/// ending between two would leave the work read as to come and its end counted
/// as past, a debt no turn ever answers. None with no helper by that id.
pub fn work_and_count(id: &str) -> Option<(bool, u32)> {
    lock().entries.get(id).map(|e| {
        let work = e.run.is_some() || !e.queue.is_empty() || !e.in_flight.is_empty() || !e.held.is_empty();
        (work, e.turns_ended)
    })
}

/// How many messages `from` has queued for helper `id` that its CLI has not had
/// yet and that go in whoever it answers to: plain text. What a CLI reads as a
/// command is judged again as it goes in (queue.rs `settle_batch`), against the
/// parent then, and `from` is told if it is dropped.
pub fn queued_from(id: &str, from: &str) -> usize {
    lock().entries.get(id).map_or(0, |e| {
        e.queue
            .iter()
            .chain(&e.in_flight)
            .filter(|q| q.from.as_deref() == Some(from) && crate::agents::command_prefix(&q.text).is_none())
            .count()
    })
}

/// What waited while helper `id` was held (`Entry::held_back`) goes on once it is
/// not: the turn on what was sent meanwhile (`run_waited`), or the resume a
/// restart left it.
pub(super) fn go_on(id: &str) {
    let next = {
        let mut r = lock();
        claim_queued(&mut r, id)
    };
    let next = next.and_then(|res| settle_batch(id, res)).map(|b| b.text);
    match next {
        Some(text) => run_waited(id, &text),
        // A `resumeOnRestart` helper a restart left to go on, which the hold kept
        // back: it goes on now, if nothing else has taken it up.
        None if crate::agents::resumes_on_restart(id) => resume(id),
        None => {}
    }
    changed();
}

/// Admission is open again after a power-off was called off: the turns it
/// refused (`requeue_refused`) start on their queued batch. Nothing else
/// waits with a queue and no turn but what a hold keeps back, which `go_on`
/// leaves be. Not called in a test build, where a turn it started would be real
/// (power/ `close`).
#[cfg_attr(test, allow(dead_code))]
pub fn resume_waiting() {
    let ids: Vec<String> =
        lock().entries.iter().filter(|(_, e)| e.run.is_none() && !e.queue.is_empty()).map(|(id, _)| id.clone()).collect();
    for id in ids {
        go_on(&id);
    }
}

/// Stops it and forgets it.
pub fn remove(id: &str) {
    kill(id);
    let e = {
        let mut r = lock();
        r.parent_changes.remove(id);
        r.entries.remove(id)
    };
    reg().1.notify_all();
    // What each of its turns was started with, as a chat's go when it is deleted.
    for sub in ["hooks", "mcp"] {
        let _ = std::fs::remove_file(crate::paths::data_dir().join(sub).join(format!("{}.json", crate::paths::id_file_name(id))));
    }
    let _ = crate::pty::claude_instructions_dir(id, None);
    changed();
    let Some(e) = e else { return };
    crate::agents::undelivered_left(id, &e.undelivered);
    end_leftovers(id, e.job.as_deref());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A helper's record: its turn's process, a question held, and between
    /// turns what its turns left, a shell running for as long as it is there,
    /// however long it has done nothing. Both CLIs alike.
    #[test]
    fn a_helpers_record_is_its_turn_and_what_its_turns_left() {
        use crate::activity::{Outcome, Phase};
        for provider in [Provider::Claude, Provider::Codex] {
            let mut e = entry(provider);
            let l = life_of(&e);
            assert_eq!((l.phase, l.outcome, l.active()), (Phase::Ended, None, false), "{provider:?}: no turn yet");
            e.run = Some(Run::claimed(Some(1), 5));
            let l = life_of(&e);
            assert_eq!((l.phase, l.turn_started_ms), (Phase::Running, Some(5)), "{provider:?}");
            e.held.push_back(Held {
                serial: 1,
                ask: Ask::permission(Some("Bash".into()), None),
                input: Value::Null,
                tool_use_id: None,
                answer: None,
                reply: None,
            });
            assert_eq!(life_of(&e).phase, Phase::Asking, "{provider:?}: an ask held");
            e.held.clear();
            e.run = None;
            e.turns_ended = 1;
            e.left_running = true;
            e.left = vec![Leftover { pid: 7, exe: "tail.exe".into(), started_ms: 1 }];
            let l = life_of(&e);
            assert_eq!((l.phase, l.outcome, l.background.shells), (Phase::Ended, Some(Outcome::Ok), 1), "{provider:?}");
            assert_eq!(l.background.names, ["tail.exe (pid 7)"]);
            assert!(l.active(), "{provider:?}: no hold to run out: active while it is there");
            assert_eq!(counts_of([&e].into_iter()), (1, 1), "{provider:?}: and keeps the core");
            e.left_running = false;
            e.left.clear();
            e.error = Some("codex exited with code 1".into());
            let l = life_of(&e);
            assert_eq!((l.outcome, l.active()), (Some(Outcome::Failed), false), "{provider:?}");
            assert_eq!(counts_of([&e].into_iter()), (0, 0), "{provider:?}");
            // Luna's stop is a stop, not a failure, either way it came.
            for why in [STOPPED_BY_STOP, INTERRUPTED] {
                e.error = Some(why.into());
                assert_eq!(life_of(&e).outcome, Some(Outcome::Stopped), "{provider:?}: {why}");
            }
        }
    }

    /// A stop's reason is the turn's it cut, not an error of the CLI's, and
    /// outlives the start of the next turn (a send with `interrupt` starts it
    /// at once): read until a turn ends after it.
    #[test]
    fn a_stop_is_told_as_one_until_a_later_turn_ends() {
        const ID: &str = "stop-told";
        let mut e = entry(Provider::Claude);
        e.run = Some(Run::claimed(None, 0));
        e.error = Some(INTERRUPTED.into());
        lock().entries.insert(ID.into(), e);
        turn_over(ID, Some(1));
        assert_eq!((stop_of(ID).as_deref(), last_error(ID)), (Some(INTERRUPTED), None));
        // The next turn starts, its error cleared as a start clears it.
        {
            let mut r = lock();
            let e = r.entries.get_mut(ID).unwrap();
            e.run = Some(Run::claimed(None, 0));
            e.error = None;
        }
        assert_eq!(stop_of(ID).as_deref(), Some(INTERRUPTED), "still the cut turn's");
        turn_over(ID, Some(0));
        assert_eq!(stop_of(ID), None, "a later turn has ended");
        lock().entries.remove(ID);
    }

    #[test]
    fn a_finished_helper_left_on_the_list_does_not_keep_a_sync_off_its_account() {
        let path = "/accounts/finished-helper";
        let mut e = entry(Provider::Codex);
        e.spec.account_path = path.into();
        lock().entries.insert("finished-helper".into(), e);
        assert!(uses_account(path));
        assert!(crate::account_settings::sync_gate(Provider::Codex, path).is_ok(), "it has no process to race");
        let on_it = crate::account_use::running_on(path);
        assert!(crate::account_settings::sync_gate(Provider::Codex, path).is_err(), "its next turn does");
        drop(on_it);
        lock().entries.remove("finished-helper");
    }

    #[test]
    fn a_kill_lets_every_held_prompt_go() {
        in_claimed_turn("kill-asks");
        let ask = |tool: &'static str| std::thread::spawn(move || permission("kill-asks", &json!({ "tool_name": tool, "input": {} })));
        let (a, b) = (ask("Bash"), ask("Edit"));
        for _ in 0..200 {
            if lock().entries.get("kill-asks").unwrap().held.len() == 2 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        kill("kill-asks");
        assert_eq!(a.join().unwrap()["behavior"], "deny");
        assert_eq!(b.join().unwrap()["behavior"], "deny");
        lock().entries.remove("kill-asks");
    }

    #[test]
    fn an_interrupt_keeps_the_queue_and_counts_a_turn_with_no_process_yet() {
        in_claimed_turn("int-claimed");
        lock().entries.get_mut("int-claimed").unwrap().queue.push("from another caller".into());
        // No process turns up to stop: the text waits behind the turn, which
        // it ends a turn after, and nothing is lost.
        let i = interrupt("int-claimed", "from this one", None).unwrap();
        assert_eq!(i, Interrupted { stopped: false, queued: true, waits: false });
        assert_eq!(lock().entries.get("int-claimed").unwrap().queue, ["from another caller", "from this one"]);
        // When that turn is over, they go together, in the order they came,
        // and an empty queue in the meantime says the text has been taken up.
        let text = joined(&std::mem::take(&mut lock().entries.get_mut("int-claimed").unwrap().queue));
        assert_eq!(text, "from another caller\n\nfrom this one");
        lock().entries.remove("int-claimed");
    }

    #[cfg(windows)]
    use crate::procs::testkit::{ended_within, hidden, idler, job_of, Kids};

    /// `--agent no-such-agent`: Claude Code exits at once, and a `git` it had
    /// started runs on in the helper's job for minutes. Only what is the CLI's by
    /// descent goes: an earlier turn's job, and what that starts, stay.
    #[cfg(windows)]
    #[test]
    fn what_a_cli_gone_before_its_model_did_anything_started_goes_with_its_turn() {
        let pause = || std::thread::sleep(std::time::Duration::from_millis(60));
        // A job an earlier turn left running.
        let mut kids = Kids(vec![idler(30)]);
        let job = job_of(&kids.0[0]);
        pause();
        let since = crate::throttle::now_ms();
        pause();
        // This turn's CLI, which starts a ping of its own and goes.
        kids.0.push(hidden("cmd", &["/d", "/c", "ping -n 30 127.0.0.1 >nul"]));
        let cli = kids.0[1].id();
        assert!(crate::procs::join(&job, cli));
        let mut its = None;
        for _ in 0..40 {
            its = crate::procs::left_in(&job)
                .unwrap_or_default()
                .into_iter()
                .find(|(r, n)| n.eq_ignore_ascii_case("ping.exe") && r.pid != kids.0[0].id())
                .map(|(r, _)| r.pid);
            if its.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let its = its.expect("the CLI started its ping");
        let _ = kids.0[1].kill();
        let _ = kids.0[1].wait();
        let mut e = entry(Provider::Claude);
        e.job = Some(job.clone());
        e.run = Some(Run::claimed(Some(cli), since));
        lock().entries.insert("strays".into(), e);
        turn_over("strays", Some(1));
        let alive = |pid: u32| crate::procs::children(Some(&*job), None).iter().any(|r| r.pid == pid);
        for _ in 0..30 {
            if !alive(its) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(!alive(its), "the CLI's own, started this turn");
        assert!(!ended_within(&mut kids.0[0], 300), "an earlier turn's job runs on");
        assert!(turn_state("strays").unwrap().children_busy, "and is what keeps it busy");
        // Its model at work: what the turn starts is its job.
        kids.0.push(idler(30));
        assert!(crate::procs::join(&job, kids.0[2].id()));
        {
            let mut r = lock();
            let e = r.entries.get_mut("strays").unwrap();
            e.run = Some(Run::claimed(Some(cli), since));
            e.observed.acted = true;
        }
        turn_over("strays", Some(0));
        assert!(!ended_within(&mut kids.0[2], 300));
        lock().entries.remove("strays");
    }

    /// Born during a turn that came to nothing, but of what an earlier turn
    /// left, or of a launcher since gone: nothing says it is the CLI's, and it
    /// is spared.
    #[cfg(windows)]
    #[test]
    fn only_what_is_the_clis_by_descent_is_ended_with_its_turn() {
        // `cmd` starting a ping is a leftover starting a worker.
        let mut kids = Kids(vec![hidden("cmd", &["/d", "/c", "ping -n 30 127.0.0.1 >nul"])]);
        let job = job_of(&kids.0[0]);
        let since = crate::throttle::now_ms().saturating_sub(5_000);
        let mut worker = None;
        for _ in 0..40 {
            worker = crate::procs::left_in(&job)
                .unwrap_or_default()
                .into_iter()
                .find(|(_, n)| n.eq_ignore_ascii_case("ping.exe"))
                .map(|(r, _)| r.pid);
            if worker.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let worker = worker.expect("cmd started its ping");
        let alive = |pid: u32| crate::procs::children(Some(&*job), None).iter().any(|r| r.pid == pid);
        // Another CLI's: the worker is not of it.
        let ended = crate::procs::end_tree_of(&job, u32::MAX - 1, since);
        assert!(ended.is_empty() && alive(worker), "{ended:?}");
        // Its launcher gone: no way up, so nothing says whose it is.
        let _ = kids.0[0].kill();
        let _ = kids.0[0].wait();
        assert!(crate::procs::end_tree_of(&job, u32::MAX - 1, since).is_empty() && alive(worker));
        // Of the CLI by descent, as that gone launcher's: it goes.
        let ended = crate::procs::end_tree_of(&job, kids.0[0].id(), since);
        assert!(ended.iter().any(|n| n.eq_ignore_ascii_case("ping.exe")), "{ended:?}");
    }

    /// A job left between turns keeps the helper active for as long as it is
    /// there, idle or not, `luna_session_get` names it, and a stop between
    /// turns ends all of it.
    #[cfg(windows)]
    #[test]
    fn a_leftover_keeps_its_helper_active_while_it_is_there_and_a_stop_ends_it() {
        let mut kids = Kids(vec![idler(120), idler(120)]);
        let job = job_of(&kids.0[0]);
        assert!(crate::procs::join(&job, kids.0[1].id()));
        let mut e = entry(Provider::Claude);
        e.turns_ended = 1;
        e.job = Some(job.clone());
        lock().entries.insert("left-work".into(), e);
        let read = || lives().into_iter().find(|(id, _)| id == "left-work").unwrap().1;
        let l = read();
        assert!(l.active() && l.background.shells == 2, "{l:?}");
        assert!(turn_state("left-work").unwrap().children_busy);
        let shown = signs("left-work").unwrap().left_running;
        assert!(shown.len() == 2 && shown.iter().all(|s| s.to_ascii_lowercase().starts_with("ping")), "{shown:?}");
        // A stop between turns is for all that is left.
        let ended = kill("left-work");
        assert_eq!(ended.iter().filter(|n| n.eq_ignore_ascii_case("ping.exe")).count(), 2, "{ended:?}");
        assert!(!turn_state("left-work").unwrap().children_busy);
        assert!(!read().active());
        assert!(signs("left-work").unwrap().left_running.is_empty());
        assert!(ended_within(&mut kids.0[0], 3000) && ended_within(&mut kids.0[1], 3000));
        lock().entries.remove("left-work");
    }

    /// What helpers' turns left running counts as a session (`running`), which
    /// had the window updated to a new build wait for a restart of the old core
    /// rather than replace it. The update's stop ends it: of a helper between
    /// turns, and of one whose turn the stop cuts short, which notes what it
    /// left only as it ends. Claude Code's and Codex's alike.
    #[cfg(windows)]
    #[test]
    fn an_update_ends_what_helpers_turns_left_so_none_counts_as_a_session() {
        let now = crate::throttle::now_ms;
        for provider in [Provider::Claude, Provider::Codex] {
            let idle = format!("upd-idle-{}", provider.as_str());
            let cut = format!("upd-cut-{}", provider.as_str());
            let mut kids = Kids(vec![idler(60), idler(60)]);
            let mut e = entry(provider);
            e.turns_ended = 1;
            e.job = Some(job_of(&kids.0[0]));
            e.left_running = true;
            e.observed.turn_ended_ms = now();
            lock().entries.insert(idle.clone(), e);
            // In a turn whose CLI the stop kills: what it left is read as it ends.
            let mut e = entry(provider);
            e.job = Some(job_of(&kids.0[1]));
            e.run = Some(Run::claimed(Some(u32::MAX - 1), now()));
            e.observed.acted = true;
            lock().entries.insert(cut.clone(), e);
            let counts = |id: &str| life_of(lock().entries.get(id).unwrap()).active();
            assert!(counts(&idle) && counts(&cut), "{provider:?}: both count as a session");
            let ending = {
                let cut = cut.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(300));
                    turn_over(&cut, Some(1));
                })
            };
            let picked = [idle.clone(), cut.clone()];
            assert_eq!(stop_for_update_of(&|id| picked.iter().any(|p| p == id), STOP_WAIT), 1, "{provider:?}: one turn stopped");
            ending.join().unwrap();
            assert!(ended_within(&mut kids.0[0], 3000) && ended_within(&mut kids.0[1], 3000), "{provider:?}: both ended");
            assert!(!counts(&idle) && !counts(&cut), "{provider:?}: neither counts as a session any more");
            lock().entries.remove(&idle);
            lock().entries.remove(&cut);
        }
    }

    /// A turn the update's stop cannot see the end of in time (claimed and
    /// stopped before its process started, here, its claimer slow), and one
    /// started while it waits (text sent meanwhile), hold the stop no longer
    /// than its bound and are left as they are, their jobs untouched: each is
    /// a turn, a session the updated window asks about. What other helpers'
    /// turns left goes all the same.
    #[cfg(windows)]
    #[test]
    fn turns_not_over_in_time_are_left_and_hold_up_no_helpers_leftovers() {
        let mut kids = Kids(vec![idler(60), idler(60), idler(60)]);
        let mut e = entry(Provider::Claude);
        e.turns_ended = 1;
        e.job = Some(job_of(&kids.0[0]));
        e.left_running = true;
        claim(&mut e);
        lock().entries.insert("upd-claimed".into(), e);
        let mut e = entry(Provider::Codex);
        e.turns_ended = 1;
        e.job = Some(job_of(&kids.0[1]));
        e.left_running = true;
        lock().entries.insert("upd-other".into(), e);
        // What its turns left is still there as it starts the next.
        let mut e = entry(Provider::Codex);
        e.turns_ended = 1;
        e.job = Some(job_of(&kids.0[2]));
        e.left_running = true;
        lock().entries.insert("upd-fresh".into(), e);
        let fresh = std::thread::spawn(|| {
            // Well after the stop has stopped what was under way, and before its wait is up.
            std::thread::sleep(std::time::Duration::from_millis(150));
            claim(lock().entries.get_mut("upd-fresh").unwrap());
        });
        let picked = ["upd-claimed", "upd-other", "upd-fresh"];
        let wait = std::time::Duration::from_millis(500);
        let started = std::time::Instant::now();
        assert_eq!(stop_for_update_of(&|id| picked.contains(&id), wait), 1, "only the turn under way");
        assert!(started.elapsed() >= wait, "it was waited for");
        fresh.join().unwrap();
        assert!(ended_within(&mut kids.0[1], 3000), "the other helper's leftovers ended");
        assert!(kids.0[0].try_wait().unwrap().is_none(), "the stopped turn's job untouched");
        assert!(kids.0[2].try_wait().unwrap().is_none(), "the fresh turn's job untouched, what was left in it too");
        let r = lock();
        assert!(r.entries["upd-claimed"].run.as_ref().is_some_and(|run| run.stopped), "stopped, still claimed");
        assert!(r.entries["upd-fresh"].run.is_some(), "still in its turn: a session");
        drop(r);
        for id in picked {
            lock().entries.remove(id);
        }
    }

    /// Text sent while the update's stop waits on a turn it stopped before its
    /// process started is never lost: that turn does not start, and what was
    /// queued behind it is kept for its sender rather than dropped with it.
    #[test]
    fn a_send_during_an_updates_stop_is_kept_for_its_sender() {
        let mut e = entry(Provider::Claude);
        e.turns_ended = 1;
        claim(&mut e);
        lock().entries.insert("upd-behind".into(), e);
        let stopping = std::thread::spawn(|| {
            let wait = std::time::Duration::from_secs(5);
            stop_for_update_of(&|id| id == "upd-behind", wait)
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !lock().entries.get("upd-behind").unwrap().run.as_ref().is_some_and(|run| run.stopped) {
            assert!(std::time::Instant::now() < deadline, "never stopped");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(send("upd-behind", "while it stops", None), Ok(Placed::Queued));
        // The claim's own start, as its claimer makes it.
        assert!(run_turn("upd-behind", "the claimed turn").is_err());
        assert_eq!(stopping.join().unwrap(), 1);
        let e = lock().entries.remove("upd-behind").unwrap();
        assert!(e.run.is_none() && e.queue.is_empty());
        assert_eq!(e.undelivered.iter().map(|q| q.text.as_str()).collect::<Vec<_>>(), ["while it stops"]);
    }

    /// The person's Stop on a turn claimed but not started drops what was
    /// queued then; text sent after it, queued behind the claim that will not
    /// start, is kept for its sender rather than dropped with it.
    #[test]
    fn a_send_after_a_stop_of_a_claimed_turn_is_kept_for_its_sender() {
        let mut e = entry(Provider::Codex);
        e.turns_ended = 1;
        claim(&mut e);
        e.queue.push(Queued::new(None, "before the stop".into(), Vec::new()));
        lock().entries.insert("stop-claimed".into(), e);
        assert!(kill("stop-claimed").is_empty());
        assert_eq!(send("stop-claimed", "after the stop", None), Ok(Placed::Queued));
        // The claim's own start, as its claimer makes it.
        assert!(run_turn("stop-claimed", "the claimed turn").is_err());
        // Stopped before it started: a stop, with the stop's reason, not a failure.
        assert_eq!(life("stop-claimed").unwrap().outcome, Some(crate::activity::Outcome::Stopped));
        assert_eq!((stop_of("stop-claimed").as_deref(), last_error("stop-claimed")), (Some(STOPPED_BY_STOP), None));
        let e = lock().entries.remove("stop-claimed").unwrap();
        assert!(e.run.is_none() && e.queue.is_empty());
        assert_eq!(e.undelivered.iter().map(|q| q.text.as_str()).collect::<Vec<_>>(), ["after the stop"]);
    }

    /// An end its stream wrote before Luna's stop took it, read after, does not
    /// stand for the turn: the stop's reason does, for either CLI.
    #[test]
    fn a_buffered_end_does_not_overwrite_a_stop() {
        let stopped = |provider: Provider| {
            let mut e = entry(provider);
            let _lines = feeding(&mut e);
            e.run.as_mut().unwrap().luna_stopped = true;
            e.error = Some(STOPPED_BY_STOP.into());
            e
        };
        for end in [
            json!({ "type": "result", "subtype": "error_during_execution", "is_error": true }),
            json!({ "type": "result", "subtype": "success" }),
        ] {
            let mut e = stopped(Provider::Claude);
            on_claude_event(&mut e, &end);
            assert_eq!(e.error.as_deref(), Some(STOPPED_BY_STOP), "{end}");
        }
        // A Codex server's, as 0.169.0 words them (appserver.rs).
        for end in [
            json!({ "method": "error", "params": { "error": { "message": "boom" }, "willRetry": false } }),
            json!({ "method": "turn/completed", "params": { "turn": { "id": "u", "status": "completed", "error": null } } }),
            json!({ "method": "turn/completed", "params": { "turn": { "id": "u", "status": "failed", "error": { "message": "boom" } } } }),
        ] {
            let mut r = lock();
            r.entries.insert("buffered-end".into(), stopped(Provider::Codex));
            on_message(&mut r, "buffered-end", &end);
            let e = r.entries.remove("buffered-end").unwrap();
            assert_eq!(e.error.as_deref(), Some(STOPPED_BY_STOP), "{end}");
        }
    }

    /// A send while a stop between turns ends what is left queues: a turn's CLI
    /// started meanwhile would land in the job being emptied, and be ended too.
    #[test]
    fn a_send_during_a_stop_between_turns_waits_for_it() {
        let mut e = entry(Provider::Claude);
        e.ending_left = true;
        lock().entries.insert("stopping".into(), e);
        assert_eq!(send("stopping", "go on", None), Ok(Placed::Queued));
        assert!(lock().entries.get("stopping").unwrap().run.is_none(), "no turn started");
        assert_eq!(interrupt("stopping", "now", None).map(|i| i.queued), Ok(true));
        assert_eq!(lock().entries.get("stopping").unwrap().queue.len(), 2);
        // A second stop meanwhile has nothing to end and leaves the first's mark.
        assert!(kill("stopping").is_empty());
        assert!(lock().entries.get("stopping").unwrap().ending_left, "still the first stop's");
        assert_eq!(send("stopping", "and this", None), Ok(Placed::Queued), "still queued");
        lock().entries.remove("stopping");
    }

    /// A stop between turns ends what its turns left, and what was sent
    /// meanwhile goes in as the next turn; one that cannot start (its folder
    /// gone) keeps it as undelivered for its sender, never dropped.
    #[cfg(windows)]
    #[test]
    fn what_waited_on_a_stop_between_turns_is_kept_if_its_turn_cannot_start() {
        let kids = Kids(vec![idler(30)]);
        let mut e = entry(Provider::Claude);
        e.turns_ended = 1;
        e.job = Some(job_of(&kids.0[0]));
        e.left_running = true;
        e.spec.folder = "C:\\luna-no-such-folder".into();
        lock().entries.insert("stop-kept".into(), e);
        // Sent while the stop ends what its turns left: it queues for after.
        let send_now = || assert_eq!(send("stop-kept", "from its parent", None), Ok(Placed::Queued));
        WHILE_ENDING.lock().unwrap().push(("stop-kept".into(), Box::new(send_now)));
        assert!(kill("stop-kept").iter().any(|n| n.eq_ignore_ascii_case("ping.exe")));
        let e = lock().entries.remove("stop-kept").unwrap();
        assert!(e.log.iter().any(|l| l.starts_with("could not start the turn")), "{:?}", e.log);
        assert!(e.queue.is_empty() && e.in_flight.is_empty());
        assert_eq!(e.undelivered, ["from its parent"], "kept for its sender");
        drop(kids);
    }

    type WhileEnding = (String, Box<dyn FnOnce() + Send>);

    /// What a test runs in a stop between turns of helper `id`, once, after
    /// it has ended what its turns left and before it looks at what was sent
    /// meanwhile (`kill`).
    static WHILE_ENDING: Mutex<Vec<WhileEnding>> = Mutex::new(Vec::new());

    pub(super) fn while_ending(id: &str) {
        let run = {
            let mut w = WHILE_ENDING.lock().unwrap_or_else(|e| e.into_inner());
            w.iter().position(|(i, _)| i == id).map(|i| w.remove(i).1)
        };
        if let Some(run) = run {
            run();
        }
    }

    #[test]
    fn a_turn_killed_or_crashed_ends_on_an_error() {
        let run = |e: &mut Entry| e.run = Some(Run::claimed(Some(1), 0));
        for (id, code, why) in [("end-crash", Some(1), "codex exited with code 1"), ("end-signal", None, "codex was killed")] {
            let mut e = entry(Provider::Codex);
            run(&mut e);
            lock().entries.insert(id.into(), e);
            turn_over(id, code);
            assert_eq!(last_error(id).as_deref(), Some(why));
            assert_eq!(turn_state(id).unwrap().turns_ended, 1, "it still ends the turn");
            lock().entries.remove(id);
        }
        // Stopped on purpose, it says so rather than the code that leaves:
        // a stop, not an error.
        let mut e = entry(Provider::Claude);
        run(&mut e);
        e.run.as_mut().unwrap().pid = None;
        lock().entries.insert("end-killed".into(), e);
        kill("end-killed");
        turn_over("end-killed", Some(1));
        assert_eq!(stop_of("end-killed").as_deref(), Some("it was stopped, by luna_session_stop or from the window"));
        assert_eq!(last_error("end-killed"), None);
        lock().entries.remove("end-killed");
        // A clean exit says nothing.
        let mut e = entry(Provider::Claude);
        run(&mut e);
        lock().entries.insert("end-clean".into(), e);
        turn_over("end-clean", Some(0));
        assert_eq!(last_error("end-clean"), None);
        lock().entries.remove("end-clean");
    }

    /// A helper taken up by a core that was not shown its old job out has no
    /// job to look in and reads active (`left_unlisted`); a stop on it is the
    /// way out. What it left that a walk by parent links can find stays.
    #[test]
    fn a_stop_settles_a_helper_whose_old_job_nothing_can_find() {
        for provider in [Provider::Claude, Provider::Codex] {
            let id = format!("stop-unlisted-{}", provider.as_str());
            let mut e = entry(provider);
            e.turns_ended = 1;
            e.left_unlisted = true;
            lock().entries.insert(id.clone(), e);
            let counts = |id: &str| life_of(lock().entries.get(id).unwrap()).active();
            assert!(counts(&id), "{provider:?}: not known is active");
            assert!(kill(&id).is_empty());
            assert!(!counts(&id), "{provider:?}: stopped, nothing is found, so it is settled");
            assert!(lock().entries[&id].left_seen_out, "{provider:?}: and stays so for the next core");
            lock().entries.remove(&id);
        }
    }

    /// A stop or an interrupt asks a Claude Code turn's CLI first, with the
    /// `interrupt` control request it took live (2.1.325), while its model is
    /// at work and its stdin open; not a Codex turn, one with no process, one
    /// past its `result` (what keeps it up is its background work, which an
    /// interrupt does not end) or one whose stdin has closed: those are killed.
    #[test]
    fn a_claude_code_turn_at_work_is_asked_to_stop_before_it_is_killed() {
        let at_work = |provider: Provider| {
            let mut e = entry(provider);
            let lines = feeding(&mut e);
            e.run.as_mut().unwrap().pid = Some(4242);
            (e, lines)
        };
        let (mut e, lines) = at_work(Provider::Claude);
        assert!(ask_to_stop(&mut e));
        let v: Value = serde_json::from_str(&lines.try_recv().unwrap()).unwrap();
        let run = e.run.as_ref().unwrap();
        assert_eq!(
            v,
            json!({ "type": "control_request", "request_id": format!("luna-interrupt-{}", run.claim), "request": { "subtype": "interrupt" } })
        );
        assert_eq!(run.interrupt.as_deref(), v["request_id"].as_str());

        let (mut e, lines) = at_work(Provider::Codex);
        assert!(!ask_to_stop(&mut e) && lines.try_recv().is_err(), "Codex");
        let (mut e, lines) = at_work(Provider::Claude);
        e.run.as_mut().unwrap().pid = None;
        assert!(!ask_to_stop(&mut e) && lines.try_recv().is_err(), "no process");
        let (mut e, lines) = at_work(Provider::Claude);
        e.run.as_mut().unwrap().after_result = true;
        assert!(!ask_to_stop(&mut e) && lines.try_recv().is_err(), "past its result");
        let (mut e, _lines) = at_work(Provider::Claude);
        e.run.as_mut().unwrap().feed = None;
        assert!(!ask_to_stop(&mut e) && e.run.as_ref().unwrap().interrupt.is_none(), "stdin closed");
    }

    /// A turn asked to stop is waited on for its `result` and its end; a
    /// helper deleted meanwhile is told apart from a process gone. Only a turn
    /// seen over is spared the kill: one that wrote no result, one still up
    /// after it, and one whose helper was deleted under the wait are killed.
    #[test]
    fn a_turn_asked_to_stop_is_killed_unless_it_is_seen_over() {
        use RunWait::*;
        const ID: &str = "asked-stop";
        let mut e = entry(Provider::Claude);
        let _lines = feeding(&mut e);
        e.run.as_mut().unwrap().pid = Some(4243);
        lock().entries.insert(ID.into(), e);
        let short = std::time::Duration::from_millis(150);
        assert_eq!(wait_on_run(ID, 4243, short, |run| run.after_result), TimedOut, "nothing yet");
        lock().entries.get_mut(ID).unwrap().run.as_mut().unwrap().after_result = true;
        assert_eq!(wait_on_run(ID, 4243, short, |run| run.after_result), Reached, "its result");
        assert_eq!(wait_on_run(ID, 4243, short, |_| false), TimedOut, "still up");
        lock().entries.get_mut(ID).unwrap().run = None;
        assert_eq!(wait_on_run(ID, 4243, short, |_| false), Over, "gone");
        lock().entries.remove(ID);
        assert_eq!(wait_on_run(ID, 4243, short, |_| false), Removed, "deleted: nothing said of its process");

        assert!(!kills_after_asking(Reached, Some(Over)), "stopped as asked");
        assert!(!kills_after_asking(Over, Some(Over)), "gone at once");
        assert!(kills_after_asking(TimedOut, None), "no result");
        assert!(kills_after_asking(Reached, Some(TimedOut)), "up past its result");
        assert!(kills_after_asking(Removed, Some(Removed)), "deleted before its result");
        assert!(kills_after_asking(Reached, Some(Removed)), "deleted after its result");
    }
}
