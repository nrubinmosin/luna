//! Stopping a helper: a kill, an interrupt, what its CLI left running, and the
//! worktree it made.

use super::*;

/// Ends what a helper's turns left running, once the helper is done with.
pub(super) fn end_leftovers(id: &str, job: Option<&crate::procs::Job>) {
    let Some(job) = job else { return };
    let ended = crate::procs::end_leftovers(job, None);
    if !ended.is_empty() {
        crate::log::info("background", &format!("{}: ended what its turns left running: {}", crate::agents::label(id), ended.join(", ")));
    }
}

/// Stops the turn in progress; the conversation stays for the next one. Between
/// turns there is no turn to stop, and what its turns left running is what is
/// left of it to stop: that is ended, and what went is returned by name.
pub fn kill(id: &str) -> Vec<String> {
    let (pid, job, _pin) = {
        let mut r = lock();
        let Some(e) = r.entries.get_mut(id) else { return Vec::new() };
        // Whoever is owed the turn hears it was stopped, not an exit code.
        if e.run.is_some() {
            e.error = Some("it was stopped, by luna_session_stop or from the window".into());
        }
        let job = e.job.clone().filter(|_| e.run.is_none() && e.left_running && !e.ending_left);
        // Only the Stop that takes the ending on sets the mark, and only it
        // lets it go: a second one meanwhile would let a send start a turn in
        // the job the first is still clearing.
        if job.is_some() {
            e.ending_left = true;
        }
        let pid = stop(e);
        // Under the lock, while the turn's own handle still holds the pid: a
        // turn that ends a moment later then leaves it to nobody else.
        (pid, job, pid.map(crate::procs::pin))
    };
    reg().1.notify_all();
    // Its turn's exit is Luna's doing, not its CLI version's.
    crate::cli::session_stopped(id);
    crate::pty::kill_tree(pid);
    let Some(job) = job else { return Vec::new() };
    let ended = crate::procs::end_leftovers(&job, None);
    // A process ended is still in the job for a moment, as the OS tears it down.
    let mut still = crate::procs::left_in(&job);
    for _ in 0..20 {
        if still.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        still = crate::procs::left_in(&job);
    }
    let next = {
        let mut r = lock();
        let Some(e) = r.entries.get_mut(id) else { return ended };
        e.ending_left = false;
        e.left_running = !still.is_empty();
        e.left = leftovers(&e.left, still, crate::throttle::now_ms(), e.observed.turn_ended_ms);
        // What was sent meanwhile is the next turn's, as after any turn.
        (e.run.is_none() && !e.held_back() && !e.queue.is_empty()).then(|| {
            claim(e);
            reserve(e)
        })
    };
    let next = next.and_then(|res| settle_batch(id, res)).map(|b| b.text);
    if !ended.is_empty() {
        crate::log::info(
            "background",
            &format!("{}: stopped between turns, which ended what its turns left running: {}", crate::agents::label(id), ended.join(", ")),
        );
    }
    if let Some(text) = next {
        save();
        if run_turn(id, &text).is_err() {
            if let Some(e) = lock().entries.get_mut(id) {
                e.in_flight.clear();
            }
        }
    }
    changed();
    ended
}

/// Whether anything its turns left still holds the machine (`Leftover::holds`):
/// once none does, an idle helper with only that left goes as any idle one does.
pub fn left_holds(id: &str) -> bool {
    let now = crate::throttle::now_ms();
    lock().entries.get(id).is_some_and(|e| leftovers_hold(e, now))
}

pub(super) fn leftovers_hold(e: &Entry, now: u64) -> bool {
    e.left_running && (e.left.is_empty() || e.left.iter().any(|l| l.holds(now)))
}

/// Between turns, what its turns left running, for `luna_session_get`.
pub(super) fn left_shown(e: &Entry, now: u64) -> Vec<LeftRunning> {
    if e.run.is_some() {
        return Vec::new();
    }
    shown(&e.left, now)
}

/// Leftovers as `luna_session_get` shows them, a helper's or a chat's.
pub fn shown(left: &[Leftover], now: u64) -> Vec<LeftRunning> {
    left.iter()
        .map(|l| LeftRunning {
            exe: l.exe.clone(),
            pid: l.pid,
            started_at: (l.started_ms > 0).then(|| iso(l.started_ms)).flatten(),
            active: l.active(now),
            quiet_min: now.saturating_sub(l.moved_ms) / 60_000,
        })
        .collect()
}

/// Between turns, how long all its turns left running has done nothing — the
/// least of them — and their names for a notice; None with nothing left, or
/// in a turn.
pub fn left_quiet(id: &str) -> Option<(u64, String)> {
    let r = lock();
    let e = r.entries.get(id).filter(|e| e.run.is_none() && e.left_running && !e.left.is_empty())?;
    let now = crate::throttle::now_ms();
    let quiet = crate::procs::quiet_ms(&e.left, now)?;
    Some((quiet, left_names(&e.left)))
}

/// Why its last turn stopped short, if it did.
pub fn last_error(id: &str) -> Option<String> {
    lock().entries.get(id)?.error.clone()
}

/// How an `interrupt` went.
#[derive(Debug, PartialEq, Clone, Copy)]
pub struct Interrupted {
    /// The turn it was on was stopped, and the text starts the next.
    pub stopped: bool,
    /// The text waits behind a turn that goes on: one that would not stop in
    /// time, or that has no process to stop yet. It ends a turn after that one.
    pub queued: bool,
}

/// How long a turn claimed a moment ago gets to have a process to stop.
const CLAIM_WAIT: std::time::Duration = std::time::Duration::from_secs(3);
/// How long a stopped turn's process gets to read as gone.
const STOP_WAIT: std::time::Duration = std::time::Duration::from_secs(15);

/// Stops the turn in progress and starts the next on `text`, behind whatever
/// was queued for after it: the conversation goes on, the work the turn was
/// doing does not. All of it waits in the queue, which the turn's end takes up
/// whole, so nobody's text hangs on how the stop goes: what would not stop
/// still has it all behind it.
pub fn interrupt(id: &str, text: &str, from: Option<&str>) -> Result<Interrupted, String> {
    let free = {
        let mut r = lock();
        let e = r.entries.get_mut(id).ok_or("no such helper")?;
        e.held.clear();
        let free = e.run.is_none() && !e.ending_left && !e.held_back();
        if free {
            claim(e);
        } else {
            note(e, "interrupted");
            e.queue.push(Queued::new(from.map(str::to_owned), text.to_string(), Vec::new()));
        }
        free
    };
    reg().1.notify_all();
    // In the queue it is what a core killed now hands back to its sender.
    save();
    if free {
        run_turn(id, text)?;
        changed();
        return Ok(Interrupted { stopped: false, queued: false });
    }
    let queued = Interrupted { stopped: false, queued: true };
    let deadline = std::time::Instant::now() + CLAIM_WAIT;
    let (pid, _pin) = loop {
        {
            let r = lock();
            let e = r.entries.get(id).ok_or("no such helper")?;
            // Taken up already: that turn ended by itself, and this is the next.
            if e.queue.is_empty() {
                return Ok(queued);
            }
            match e.run.as_ref().map(|run| run.pid) {
                // Pinned under the lock, as `kill` does.
                Some(Some(pid)) => break (pid, crate::procs::pin(pid)),
                Some(None) if std::time::Instant::now() < deadline => {}
                _ => return Ok(queued),
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    crate::pty::kill_tree(Some(pid));
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
    Ok(Interrupted { stopped: true, queued: false })
}

/// The worktree it made, once the CLI has said where: Claude Code as its
/// first turn starts, Codex in its rollout.
pub fn made_worktree(id: &str) -> Option<String> {
    let (cwd, folder, provider, account, session) = {
        let r = lock();
        let e = r.entries.get(id)?;
        if !e.spec.worktree {
            return None;
        }
        (e.cwd.clone(), e.spec.folder.clone(), e.spec.provider, e.spec.account_path.clone(), e.session.clone())
    };
    let cwd = cwd.or_else(|| match provider {
        Provider::Codex => crate::codex::session::thread_cwd(&account, session.as_deref()?),
        Provider::Claude => None,
    })?;
    (!crate::pty::same_dir(&cwd, &folder)).then_some(cwd)
}

/// The worktree `remove` would hand back, with nothing touched: where the CLI
/// has said it works, off Codex's rollout until the turn's end records it.
pub fn removable_worktree(id: &str) -> Option<String> {
    let (cwd, folder, provider, account, session) = {
        let r = lock();
        let e = r.entries.get(id)?;
        (e.cwd.clone(), e.spec.folder.clone(), e.spec.provider, e.spec.account_path.clone(), e.session.clone())
    };
    let cwd = cwd.or_else(|| match provider {
        Provider::Codex => crate::codex::session::thread_cwd(&account, session.as_deref()?),
        Provider::Claude => None,
    })?;
    (!crate::pty::same_dir(&cwd, &folder) && crate::worktree::is_worktree_of(&folder, &cwd)).then_some(cwd)
}

/// Stops its turn and what its turns left, for a look at what they leave behind.
pub fn stop_for_good(id: &str) {
    kill(id);
    let job = lock().entries.get(id).and_then(|e| e.job.clone());
    end_leftovers(id, job.as_deref());
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
    e.deleting = true;
    Ok(DeleteClaim { id: id.to_string(), held: true })
}

/// What waited while helper `id` was held (`Entry::held_back`) goes on once it is
/// not: the turn on what was sent meanwhile, or the resume a restart left it.
pub(super) fn go_on(id: &str) {
    let next = {
        let mut r = lock();
        let Some(e) = r.entries.get_mut(id) else { return };
        (e.run.is_none() && !e.ending_left && !e.held_back() && !e.queue.is_empty()).then(|| {
            claim(e);
            reserve(e)
        })
    };
    let next = next.and_then(|res| settle_batch(id, res)).map(|b| b.text);
    match next {
        Some(text) => {
            save();
            if run_turn(id, &text).is_err() {
                if let Some(e) = lock().entries.get_mut(id) {
                    e.in_flight.clear();
                }
            }
        }
        // A `resumeOnRestart` helper a restart left to go on, which the hold kept
        // back: it goes on now, if nothing else has taken it up.
        None if crate::agents::resumes_on_restart(id) => resume(id),
        None => {}
    }
    changed();
}

/// Stops it and forgets it. Returns the worktree it made, if any.
pub fn remove(id: &str) -> Option<String> {
    let made = removable_worktree(id);
    kill(id);
    let e = lock().entries.remove(id);
    reg().1.notify_all();
    // What each of its turns was started with, as a chat's go when it is deleted.
    for sub in ["hooks", "mcp"] {
        let _ = std::fs::remove_file(crate::paths::data_dir().join(sub).join(format!("{}.json", crate::paths::id_file_name(id))));
    }
    crate::pty::claude_instructions_dir(id, None);
    changed();
    let e = e?;
    crate::agents::undelivered_left(id, &e.undelivered);
    end_leftovers(id, e.job.as_deref());
    made
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What keeps the core: a turn, or between turns what the turns left while
    /// it is at work, by the machine's measure.
    #[test]
    fn leftovers_at_work_keep_the_core_and_long_idle_ones_do_not() {
        let now = 1_793_000_000_000;
        let left = |moved_ms: u64| Leftover {
            pid: 7,
            exe: "cargo.exe".into(),
            started_ms: now - 60_000,
            cpu_ms: 0,
            io_bytes: 0,
            read_ms: now,
            seen_ms: now,
            moved_ms,
        };
        let mut e = entry(Provider::Codex);
        assert!(!leftovers_hold(&e, now), "nothing left");
        e.left_running = true;
        assert!(leftovers_hold(&e, now), "left, not looked at yet");
        e.left = vec![left(now - 60_000)];
        assert!(leftovers_hold(&e, now), "at work a minute ago");
        e.left = vec![left(now - crate::procs::LEFTOVER_HOLD_MS - 1)];
        assert!(!leftovers_hold(&e, now), "idle past the hold: the core may go");
    }

    #[test]
    fn a_finished_helper_left_on_the_list_does_not_keep_a_sync_off_its_account() {
        let path = "/accounts/finished-helper";
        let mut e = entry(Provider::Codex);
        e.spec.account_path = path.into();
        lock().entries.insert("finished-helper".into(), e);
        assert!(uses_account(path));
        assert!(crate::account_settings::sync_gate(Provider::Codex, path).is_ok(), "it has no process to race");
        assert!(crate::account_use::quiet(&[path.to_string()]).is_err(), "a recommendation still goes by the session");
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
        assert_eq!(i, Interrupted { stopped: false, queued: true });
        assert_eq!(lock().entries.get("int-claimed").unwrap().queue, ["from another caller", "from this one"]);
        // When that turn is over, they go together, in the order they came,
        // and an empty queue in the meantime says the text has been taken up.
        let text = joined(&std::mem::take(&mut lock().entries.get_mut("int-claimed").unwrap().queue));
        assert_eq!(text, "from another caller\n\nfrom this one");
        lock().entries.remove("int-claimed");
    }

    #[cfg(windows)]
    use crate::procs::testkit::{burner, ended_within, hidden, idler, job_of, Kids};

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
            worker = crate::procs::left_in(&job).into_iter().find(|(_, n)| n.eq_ignore_ascii_case("ping.exe")).map(|(r, _)| r.pid);
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

    /// A job left between turns holds the machine until it has done nothing for
    /// long, `luna_session_get` says what of it is at work, and a stop between turns ends
    /// all of it.
    #[cfg(windows)]
    #[test]
    fn a_leftover_holds_the_machine_until_long_idle_and_a_stop_ends_it() {
        let mut kids = Kids(vec![idler(120), burner(120)]);
        let job = job_of(&kids.0[0]);
        assert!(crate::procs::join(&job, kids.0[1].id()));
        let mut e = entry(Provider::Claude);
        e.turns_ended = 1;
        e.job = Some(job.clone());
        e.observed.turn_ended_ms = crate::throttle::now_ms();
        lock().entries.insert("left-work".into(), e);
        let sample = || activity().into_iter().find(|a| a.id == "left-work").unwrap();
        let first = sample();
        assert!(first.busy, "just left: as quiet as its turn's end, which is no time ago");
        assert!(turn_state("left-work").unwrap().children_busy);
        // As if both had sat there for longer than LEFTOVER_HOLD_MS since, and
        // were last read a window ago: the next sample takes their rates.
        let (loop_pid, mut burner) = (kids.0[1].id(), None);
        for l in lock().entries.get_mut("left-work").unwrap().left.iter_mut() {
            l.moved_ms = l.moved_ms.saturating_sub(crate::procs::LEFTOVER_HOLD_MS + 60_000);
            l.read_ms = l.read_ms.saturating_sub(crate::procs::LEFTOVER_WINDOW_MS);
            if l.pid == loop_pid {
                burner = Some((l.cpu_ms, l.read_ms));
            }
        }
        // The window the next rate is taken over is the minute just put back,
        // but the loop has only the real time since to burn in: wait until its
        // own CPU since then is twice what that rate needs, however slowly a
        // loaded machine runs it, so its start's I/O is not what makes it busy.
        let (cpu_then, read_then) = burner.expect("the loop is among what is left");
        let cpu_now = || crate::procs::left_in(&job).into_iter().find(|(r, _)| r.pid == loop_pid).map_or(0, |(r, _)| r.cpu_ms);
        let needed = || 2 * crate::procs::LEFTOVER_CPU_MS_PER_S * crate::throttle::now_ms().saturating_sub(read_then) / 1000;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
        while cpu_now().saturating_sub(cpu_then) < needed() {
            assert!(std::time::Instant::now() < deadline, "the loop got too little CPU in 90 s to measure");
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let now = sample();
        assert!(now.busy, "the loop burns a core");
        assert!(
            now.procs.len() == 1 && now.procs[0].to_ascii_lowercase().starts_with("powershell.exe"),
            "and is all that holds the machine, the long idle ping not: {:?}",
            now.procs
        );
        let shown = signs("left-work").unwrap().left_running;
        let active = |exe: &str| shown.iter().find(|l| l.exe.eq_ignore_ascii_case(exe)).map(|l| l.active);
        assert_eq!((active("powershell.exe"), active("PING.EXE")), (Some(true), Some(false)), "{shown:?}");
        assert!(shown.iter().all(|l| l.started_at.is_some()));
        assert!(left_holds("left-work"));
        // A stop between turns is for all that is left; a send meanwhile waits for it.
        let ended = kill("left-work");
        for exe in ["ping.exe", "powershell.exe"] {
            assert!(ended.iter().any(|n| n.eq_ignore_ascii_case(exe)), "{exe}: {ended:?}");
        }
        assert!(!turn_state("left-work").unwrap().children_busy);
        assert!(signs("left-work").unwrap().left_running.is_empty());
        assert!(ended_within(&mut kids.0[0], 3000) && ended_within(&mut kids.0[1], 3000));
        lock().entries.remove("left-work");
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

    /// Long idle, its leftovers hold nothing, and an idle helper with only them
    /// left is as idle as one with nothing (`agents::reap`).
    #[test]
    fn leftovers_idle_past_their_grace_hold_nothing() {
        idle_for_test("held-long", Provider::Claude, 1, false);
        left_for_test("held-long", &[("tail.exe", 22104, crate::procs::LEFTOVER_HOLD_MS - 60_000)]);
        assert!(left_holds("held-long"));
        left_for_test("held-long", &[("tail.exe", 22104, crate::procs::LEFTOVER_HOLD_MS + 60_000)]);
        assert!(!left_holds("held-long"));
        lock().entries.remove("held-long");
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
        // Stopped on purpose, it says so rather than the code that leaves.
        let mut e = entry(Provider::Claude);
        run(&mut e);
        e.run.as_mut().unwrap().pid = None;
        lock().entries.insert("end-killed".into(), e);
        kill("end-killed");
        turn_over("end-killed", Some(1));
        assert_eq!(last_error("end-killed").as_deref(), Some("it was stopped, by luna_session_stop or from the window"));
        lock().entries.remove("end-killed");
        // A clean exit says nothing.
        let mut e = entry(Provider::Claude);
        run(&mut e);
        lock().entries.insert("end-clean".into(), e);
        turn_over("end-clean", Some(0));
        assert_eq!(last_error("end-clean"), None);
        lock().entries.remove("end-clean");
    }
}
