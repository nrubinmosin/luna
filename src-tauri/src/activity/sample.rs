//! Each sample: what the disk says of a session's turn — Claude Code's registry,
//! Codex's rollout — and what its processes do, folded with the hook's word and
//! the clock into one verdict per session, and the summary power/ follows.

use super::*;

/// A Codex session as its rollouts are read.
fn codex_live(p: &crate::pty::ActivityProbe) -> crate::codex::session::Live<'_> {
    crate::codex::session::Live {
        chat_id: &p.id,
        cwd: &p.cwd,
        spawned_at_ms: p.spawned_at_ms,
        resume: p.resume.as_deref(),
        account_path: &p.account_path,
        last_output_ms: p.last_output_ms,
    }
}

/// What the disk says about a session's turn right now, and when it last
/// changed, where the CLI says (Claude Code's registry).
fn disk_turn(p: &crate::pty::ActivityProbe) -> Option<(Turn, Option<u64>)> {
    let (raw, at) = match p.provider {
        Provider::Claude => crate::claude::session::status(p.pid, p.resume.as_deref(), &p.cwd, p.spawned_at_ms, &p.account_path)?,
        Provider::Codex => (crate::codex::session::status(&codex_live(p))?, None),
    };
    // Codex keeps no record of a pending approval; its screen says so.
    if p.asking && turn_of(&raw) == Turn::Busy {
        return Some((Turn::Waiting, at));
    }
    Some((turn_of(&raw), at))
}

/// The API error a session's last turn ended on, per its transcript. Codex
/// ends such a turn like any other, so it never sits waiting on one.
fn disk_api_error(p: &crate::pty::ActivityProbe) -> Option<&'static str> {
    match p.provider {
        Provider::Claude => crate::claude::session::last_api_error(p.pid, p.resume.as_deref(), &p.cwd, p.spawned_at_ms, &p.account_path),
        Provider::Codex => None,
    }
}

/// The usage limit a chat's last turn ended on, in Luna's words, where its
/// CLI says: Claude Code's `StopFailure` hook or its transcript's API error;
/// Codex's rollout (`limit_in`), which ends such a turn like any other, so
/// it never sits waiting on one. None for a chat not running.
pub fn limit_of(chat_id: &str, provider: Provider) -> Option<&'static str> {
    limit_at(chat_id, provider).map(|(kind, _)| kind)
}

/// `limit_of`, with when the turn stopped on it where its source says: the
/// hook's arrival, or the rollout line's time. A transcript's API error says
/// no time Luna reads.
pub(super) fn limit_at(chat_id: &str, provider: Provider) -> Option<(&'static str, Option<u64>)> {
    let p = crate::pty::manager().probe(chat_id)?;
    match provider {
        Provider::Claude => failed_at(chat_id)
            .map(|(kind, at)| (kind, Some(at)))
            .or_else(|| {
                crate::claude::session::last_api_error(p.pid, p.resume.as_deref(), &p.cwd, p.spawned_at_ms, &p.account_path)
                    .map(|kind| (kind, None))
            })
            .filter(|(e, _)| *e == "rate_limit"),
        Provider::Codex => {
            let path = crate::codex::session::rollout_for(&codex_live(&p))?;
            let (kind, at) = crate::codex::session::limit_at_in(&crate::pty::tail(&path, 256 * 1024)?)?;
            let at = at.and_then(|t| chrono::DateTime::parse_from_rfc3339(&t).ok()).map(|t| t.timestamp_millis().max(0) as u64);
            Some((kind, at))
        }
    }
}

/// Turns the CLI's own record says this process has finished, where it
/// keeps one.
fn disk_turns_ended(p: &crate::pty::ActivityProbe) -> Option<u32> {
    match p.provider {
        Provider::Claude => None,
        Provider::Codex => crate::codex::session::turns_completed(&codex_live(p)),
    }
}

/// Its subagents at work, where the disk is what tells: Codex's, by their own
/// rollouts. Claude Code's come by hook (`on_hook`).
fn disk_subagents(p: &crate::pty::ActivityProbe) -> Option<Vec<String>> {
    match p.provider {
        Provider::Claude => None,
        Provider::Codex => crate::codex::session::children_at_work(&codex_live(p)),
    }
}

/// The CLIs' words for a turn, folded the way the chat rows fold them.
pub fn turn_of(raw: &str) -> Turn {
    match raw {
        "busy" | "working" => Turn::Busy,
        s if ["wait", "input", "block", "attention", "permission"].iter().any(|w| s.contains(w)) => Turn::Waiting,
        _ => Turn::Idle,
    }
}

/// One session's verdict from its hook word, the disk's word, and the clock.
pub(super) fn combine(t: &mut Track, disk: Option<Turn>, now: u64) -> Turn {
    t.disk = disk;
    match disk {
        Some(Turn::Idle) => {
            t.disk_idle_since_ms.get_or_insert(now);
        }
        _ => t.disk_idle_since_ms = None,
    }
    match disk {
        Some(Turn::Busy) => {
            t.disk_busy_since_ms.get_or_insert(now);
        }
        _ => t.disk_busy_since_ms = None,
    }
    let idle_for = t.disk_idle_since_ms.map(|s| now.saturating_sub(s)).unwrap_or(0);
    // Claude Code calls itself busy while a subagent of it works, so an idle
    // registry that has stood a while says none is left: a SubagentStop that
    // never came must not keep the session at work for good. A Codex rollout
    // says idle whatever its subagents do; theirs say what they do.
    if idle_for > STALE_HOOK_MS && !t.agents_on_disk {
        t.agents.clear();
    }
    let hook = match t.hook {
        // A hook that says busy against a registry that has said idle for a
        // while has missed its Stop.
        Some(Turn::Busy) if idle_for > STALE_HOOK_MS => {
            t.hook = Some(Turn::Idle);
            Some(Turn::Idle)
        }
        // A hook that says waiting against a registry that has said idle for
        // a while: the prompt went away with its turn, and no Stop followed.
        Some(Turn::Waiting) if idle_for > STALE_WAIT_MS => {
            t.hook = Some(Turn::Idle);
            Some(Turn::Idle)
        }
        h => h,
    };
    // The registry says busy, but the hook said the turn ended: a subagent
    // of it at work, or, with nothing spoken since, a registry left behind.
    // Not a turn either way.
    let disk = match (hook, disk) {
        (Some(Turn::Idle), Some(Turn::Busy)) if !t.agents.is_empty() => Some(Turn::Idle),
        (Some(Turn::Idle), Some(Turn::Busy)) => {
            let since = t.disk_busy_since_ms.unwrap_or(now).max(t.hook_at_ms);
            if now.saturating_sub(since) > STALE_DISK_MS {
                Some(Turn::Idle)
            } else {
                disk
            }
        }
        _ => disk,
    };
    match (hook, disk) {
        // A permission prompt sits inside a turn the registry calls busy, a
        // subagent's too, whether or not the turn that started it is over.
        (Some(Turn::Waiting), _) => Turn::Waiting,
        _ if t.agents.values().any(|asks| *asks) => Turn::Waiting,
        // The registry says a dialog is up, so the model is not working,
        // whatever the last hook said: a turn that died on an API error
        // before Claude Code fired `StopFailure` left its hook at busy.
        (Some(Turn::Busy), Some(Turn::Waiting)) => Turn::Waiting,
        (Some(Turn::Busy), _) | (_, Some(Turn::Busy)) => Turn::Busy,
        (None, Some(d)) => d,
        (Some(h), None) => h,
        (Some(Turn::Idle), Some(d)) => d,
        (None, None) => Turn::Idle,
    }
}

/// One session's children at a sample: what of them is at work, and what holds
/// the machine, by label, and what of its turns' leftovers has just let it go.
/// What came up with the session holds it while it moves (`judge`, sample to
/// sample); what its turns started (`young`) until it has sat idle for
/// LEFTOVER_HOLD_MS, by its rate over LEFTOVER_WINDOW_MS, as a helper's
/// leftovers do (`procs::leftovers`): a loop's one busy sample is no work, and
/// counted as moving it would start a power-off's quiet window over each time.
/// One with no name has gone since the read.
fn children_at(
    t: &mut Track,
    children: Vec<ProcRead>,
    born_after: u64,
    name_of: &dyn Fn(u32) -> Option<String>,
    now: u64,
) -> (Vec<String>, Vec<String>) {
    t.procs = procs::without_furniture(procs::judge(&t.prev, &children, born_after, MIN_CPU_MS, MIN_IO_BYTES));
    if let Some(b) = t.procs.as_mut() {
        let young = b.young.clone();
        b.active.retain(|p| !young.contains(p));
    }
    let young: Vec<(ProcRead, String)> = match &t.procs {
        Some(b) => children.iter().filter(|r| b.young.contains(&r.pid)).filter_map(|r| Some((*r, name_of(r.pid)?))).collect(),
        None => Vec::new(),
    };
    let left = procs::leftovers(&t.left, young, now, t.turn_at_ms);
    let gone_quiet = procs::gone_quiet(&t.left, &left, now);
    t.left = left;
    t.prev = children;
    let moving: Vec<u32> = t.procs.as_ref().map(|b| b.active.clone()).unwrap_or_default();
    let mut holding: Vec<String> = moving.iter().filter_map(|&pid| Some(format!("{} (pid {pid})", name_of(pid)?))).collect();
    holding.extend(t.left.iter().filter(|l| l.holds(now) && !moving.contains(&l.pid)).map(procs::Leftover::label));
    holding.sort();
    holding.dedup();
    (holding, gone_quiet)
}

/// Its subagents at work, for what holds the machine: they work in the CLI
/// itself, with no process of their own to show, a job all the same. Not one
/// on a prompt of its own, which waits for a person as a turn on one does.
pub(super) fn agents_at_work(t: &Track) -> Option<String> {
    match t.agents.values().filter(|asks| !**asks).count() {
        0 => None,
        1 => Some("1 subagent".to_string()),
        n => Some(format!("{n} subagents")),
    }
}

/// One pass over every live session, plus the background helpers as they
/// stand (`helpers`), which keep no track here. Called by the sampler thread;
/// public so a test can drive it with probes of its own.
///
/// The registry, rollouts, transcripts and process table are read outside
/// the state lock: a hook is answered only once `on_hook` has had it, and
/// Claude Code holds its turn until the hook's curl returns, so reads under
/// the lock held up every busy session's next step for as long as they took.
pub fn sample(probes: Vec<crate::pty::ActivityProbe>, helpers: Vec<SessionActivity>) -> Summary {
    let reads: Vec<_> =
        probes.iter().map(|p| (disk_turn(p), disk_turns_ended(p), procs::children(p.job.as_deref(), p.pid), disk_subagents(p))).collect();
    // Every child's name, from one snapshot of the process table.
    let named = procs::names(&reads.iter().flat_map(|(_, _, children, _)| children.iter().map(|r| r.pid)).collect::<Vec<_>>());
    let name_of = |pid: u32| named.iter().find(|(p, _)| *p == pid).map(|(_, n)| n.clone());
    let now = now_ms();

    let mut sessions = Vec::with_capacity(probes.len());
    // Per session, whether to look behind its wait for an API error: read once
    // the lock is let go. And whose leftovers have let the machine go, for the log.
    let mut later: Vec<bool> = Vec::with_capacity(probes.len());
    let mut let_go: Vec<(String, Vec<String>)> = Vec::new();
    {
        let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
        let st = &mut *st;

        let live: Vec<&str> = probes.iter().map(|p| p.id.as_str()).collect();
        drop_gone(&mut st.tracks, &mut st.ended, &live);

        for (p, (disk_read, disk_turns, children, disk_agents)) in probes.iter().zip(reads) {
            let t = track(&mut st.tracks, &mut st.ended, &p.id);
            // A respawn (new settings, a resume) is a new CLI with servers of
            // its own: what counts as its turns' work starts over with it.
            if t.spawned_at_ms != 0 && t.spawned_at_ms != p.spawned_at_ms {
                t.first_turn_ms = 0;
                t.prev.clear();
                t.left.clear();
                t.agents.clear();
            }

            let (disk, disk_at) = disk_read.map_or((None, None), |(d, at)| (Some(d), at));
            // A registry that changed after the hook spoke is the later word: a
            // prompt answered "No" ends the turn with no Stop, and the waiting
            // the hook said would otherwise stand for STALE_WAIT_MS.
            if disk == Some(Turn::Idle) && t.hook == Some(Turn::Waiting) && disk_at.is_some_and(|at| at > t.hook_at_ms) {
                t.hook = Some(Turn::Idle);
            }
            // Codex's subagents are what their rollouts say at this sample, read
            // whole each time. None of them is known to wait on a prompt.
            if let Some(agents) = disk_agents {
                t.agents = agents.into_iter().map(|id| (id, false)).collect();
                t.agents_on_disk = true;
            }
            let turn = combine(t, disk, now);
            // Without a hook the start of a turn is seen at the sample after it
            // happened; a child born in between is still the turn's.
            if turn == Turn::Busy && t.first_turn_ms == 0 {
                t.first_turn_ms = now.saturating_sub(TURN_SLACK_MS);
            }
            t.turn = turn;
            if turn != Turn::Idle {
                t.turn_at_ms = now;
            }
            match turn {
                Turn::Idle => t.turn_started_ms = None,
                // A registry still busy after the hook's `Stop` has been left
                // behind: no turn starts on its word.
                _ if t.hook != Some(Turn::Idle) => {
                    t.turn_started_ms.get_or_insert(now);
                }
                _ => {}
            }

            let born_after = match t.first_turn_ms {
                0 => 0,
                first => first.max(p.spawned_at_ms as u64 + STARTUP_MS),
            };
            let (mut holding, gone_quiet) = children_at(t, children, born_after, &name_of, now);
            holding.extend(agents_at_work(t));
            if !gone_quiet.is_empty() {
                let_go.push((p.id.clone(), gone_quiet));
            }
            t.output_fresh = now.saturating_sub(p.last_output_ms) < OUTPUT_FRESH_MS;

            // A turn "ends" for an agent when the model stops, not when its
            // background jobs do: `wait` wants the reply, and the children are
            // the machine's concern (power/), not the caller's.
            let model_busy = t.turn == Turn::Busy;
            // Codex: the rollout's count is the truth.
            count_on(t, p.spawned_at_ms, disk_turns);
            // Claude Code with hooks: `Stop` counts (on_hook). Without any hook
            // heard, the transitions are all there is.
            if disk_turns.is_none() && t.hook.is_none() && t.was_busy && !model_busy {
                t.turns_ended += 1;
            }
            t.was_busy = model_busy;

            let busy = model_busy || !holding.is_empty() || t.output_fresh;
            // Only a waiting session is asked: the hook's word if one came, else
            // the transcript's last line.
            let waiting = t.turn == Turn::Waiting;
            later.push(waiting && t.failed.is_none());
            sessions.push(SessionActivity {
                id: p.id.clone(),
                turn: t.turn,
                procs: holding,
                output_fresh: t.output_fresh,
                busy,
                stalled: t.failed.filter(|_| waiting),
                hook: t.hook,
                disk: t.disk,
                output_age_ms: now.saturating_sub(p.last_output_ms),
            });
        }
    }

    for (id, gone) in let_go {
        crate::log::warn(
            "power",
            &format!(
                "{}: no longer holds the PC by what its turns left, idle {} min on end: {}",
                crate::agents::label(&id),
                procs::LEFTOVER_HOLD_MS / 60_000,
                gone.join(", ")
            ),
        );
    }
    for ((s, p), ask) in sessions.iter_mut().zip(&probes).zip(later) {
        if ask {
            s.stalled = disk_api_error(p);
        }
    }
    sessions.extend(helpers);
    let busy = sessions.iter().filter(|s| s.busy).count();
    let parked = |s: &&SessionActivity| !s.busy && s.turn == Turn::Waiting;
    let waiting = sessions.iter().filter(parked).filter(|s| s.stalled.is_none()).count();
    let stalled = sessions.iter().filter(parked).filter(|s| s.stalled.is_some()).count();
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    let st = &mut *st;
    if busy == 0 && waiting == 0 {
        st.idle_since_ms.get_or_insert(now);
    } else {
        st.idle_since_ms = None;
    }
    st.summary = Summary { busy, waiting, stalled, idle_since_ms: st.idle_since_ms, sessions };
    st.summary.clone()
}

/// The sampler thread: probes, samples, hands the board to power/.
pub fn start() {
    std::thread::Builder::new()
        .name("luna-activity".into())
        .spawn(move || {
            let mut last = std::time::Instant::now();
            loop {
                std::thread::sleep(SAMPLE_EVERY);
                let gap = last.elapsed();
                last = std::time::Instant::now();
                // A sample that took far longer than its period to come round
                // is the machine having been asleep, not the sessions idle.
                if gap > SAMPLE_EVERY * 6 {
                    crate::log::warn("activity", &format!("clock jumped {gap:?}; idle stretch reset"));
                    reset_idle();
                    crate::power::woke();
                }
                let pty = crate::pty::manager();
                let mut probes = pty.activity_probes();
                for p in probes.iter_mut().filter(|p| p.provider == Provider::Codex) {
                    p.asking = pty.screen(&p.id).is_some_and(|s| crate::codex::session::asks(&s));
                }
                let summary = sample(probes, crate::background::activity());
                crate::power::tick(&summary);
            }
        })
        .expect("spawn activity thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A chat's leftovers by a helper's measure: a `tail -F` its tool left held
    /// the machine for as long as it lived.
    #[test]
    fn a_chats_leftover_holds_the_machine_until_long_idle() {
        let read = |pid: u32, cpu_ms: u64| ProcRead { pid, cpu_ms, io_bytes: 0, started_ms: 2_000 };
        let name = |pid: u32| Some(format!("p{pid}.exe"));
        // Started by its turns (after 1_000), which were on until 9_000: as quiet as that.
        let mut t = Track { turn_at_ms: 9_000, ..Default::default() };
        let (first, _) = children_at(&mut t, vec![read(1, 10), read(2, 10)], 1_000, &name, 10_000);
        assert_eq!(first, ["p1.exe (pid 1)", "p2.exe (pid 2)"]);
        assert!(t.left.iter().all(|l| l.moved_ms == 9_000), "birth is no work");
        // Long past LEFTOVER_HOLD_MS, a window after their last read: one burns
        // CPU, one only sits there and lets go.
        let at = 9_000 + procs::LEFTOVER_HOLD_MS + 15_000;
        t.left.iter_mut().for_each(|l| {
            l.read_ms = at - procs::LEFTOVER_WINDOW_MS;
            l.seen_ms = 9_000 + procs::LEFTOVER_HOLD_MS - 1_000;
        });
        let (holding, gone) = children_at(&mut t, vec![read(1, 10), read(2, 2_010)], 1_000, &name, at);
        assert_eq!((holding, gone), (vec!["p2.exe (pid 2)".to_string()], vec!["p1.exe (pid 1)".to_string()]));
        // That one gone, the idle one holds nothing, and is still there for its notice.
        let (holding, gone) = children_at(&mut t, vec![read(1, 11)], 1_000, &name, at + 5_000);
        assert!(holding.is_empty() && gone.is_empty(), "let go once");
        assert!(t.procs.as_ref().is_some_and(|b| b.young == [1]), "its job is not over");
        assert_eq!(procs::quiet_ms(&t.left, at + 5_000), Some(at + 5_000 - 9_000));
    }

    /// A wait loop its turn left (`until grep …; do sleep 10; done`), sampled
    /// every five seconds: a fork's 80 ms in one sample is no work. It neither
    /// holds the machine at that sample, which would start a power-off's quiet
    /// window over, nor starts its LEFTOVER_HOLD_MS over; it lets go once.
    #[test]
    fn a_loops_busy_sample_is_no_work() {
        let read = |cpu_ms: u64| ProcRead { pid: 7, cpu_ms, io_bytes: 0, started_ms: 2_000 };
        let name = |pid: u32| Some(format!("bash{pid}.exe"));
        let mut t = Track { turn_at_ms: 9_000, ..Default::default() };
        children_at(&mut t, vec![read(100)], 1_000, &name, 10_000);
        // Just short of LEFTOVER_HOLD_MS since its turn, as last looked at a sample ago.
        let start = 9_000 + procs::LEFTOVER_HOLD_MS - 30_000;
        t.left.iter_mut().for_each(|l| {
            l.read_ms = start - 5_000;
            l.seen_ms = start - 5_000;
        });
        let mut cpu = 100;
        let mut gone_said = 0;
        for i in 0..36u64 {
            let at = start + i * SAMPLE_EVERY.as_millis() as u64;
            // A tick of CPU now and then, and one sample of 80 ms once it has
            // sat idle long enough to let go.
            cpu += match i {
                8 => 80,
                i if i % 3 == 0 => 16,
                _ => 0,
            };
            let (holding, gone) = children_at(&mut t, vec![read(cpu)], 1_000, &name, at);
            let quiet = at.saturating_sub(9_000) >= procs::LEFTOVER_HOLD_MS;
            assert_eq!(holding.is_empty(), quiet, "sample {i}: {holding:?}");
            gone_said += gone.len();
            assert_eq!(t.left[0].moved_ms, 9_000, "sample {i}: no work, so none is noted");
        }
        assert_eq!(gone_said, 1, "let go, and said so once");
    }

    /// A Codex chat's subagents come off their own rollouts at each sample: its
    /// own rollout idle long past STALE_HOOK_MS says nothing of them, and they
    /// stay its job until theirs say done. Claude Code's, kept from hooks, go
    /// with its registry's long idle.
    #[test]
    fn a_codex_subagent_outlasts_its_chats_idle_rollout() {
        let mut t = Track { agents: [("kid".to_string(), false)].into(), agents_on_disk: true, ..Default::default() };
        assert_eq!(combine(&mut t, Some(Turn::Idle), 1_000), Turn::Idle);
        let late = 1_000 + STALE_HOOK_MS + 5_000;
        assert_eq!(combine(&mut t, Some(Turn::Idle), late), Turn::Idle, "its turn is over");
        assert_eq!(agents_at_work(&t).as_deref(), Some("1 subagent"), "its job is not");
        t.agents_on_disk = false;
        combine(&mut t, Some(Turn::Idle), late + 5_000);
        assert!(t.agents.is_empty());
    }

    /// A SubagentStop that never came: the registry, idle a while, says no
    /// agent is left.
    #[test]
    fn an_idle_registry_ends_lost_agents() {
        let mut t = Track { hook: Some(Turn::Idle), ..Default::default() };
        t.agents.insert("lost".into(), false);
        assert_eq!(combine(&mut t, Some(Turn::Idle), 1_000), Turn::Idle);
        assert_eq!(t.agents.len(), 1, "a registry just gone idle may not have caught up");
        combine(&mut t, Some(Turn::Idle), 1_000 + STALE_HOOK_MS + 1);
        assert!(t.agents.is_empty());
    }

    #[test]
    fn a_dialog_in_the_registry_beats_a_busy_hook() {
        // The turn died on a usage limit with no Stop: the hook still says
        // busy, the registry says the limit dialog is up.
        let mut t = Track { hook: Some(Turn::Busy), ..Default::default() };
        assert_eq!(combine(&mut t, Some(Turn::Waiting), 1000), Turn::Waiting);
    }

    #[test]
    fn folds_the_clis_words() {
        assert_eq!(turn_of("busy"), Turn::Busy);
        assert_eq!(turn_of("working"), Turn::Busy);
        assert_eq!(turn_of("waiting"), Turn::Waiting);
        assert_eq!(turn_of("waiting_for_permission"), Turn::Waiting);
        assert_eq!(turn_of("idle"), Turn::Idle);
        assert_eq!(turn_of("resting"), Turn::Idle);
    }

    #[test]
    fn waiting_hook_beats_busy_registry() {
        let mut t = Track { hook: Some(Turn::Waiting), ..Default::default() };
        assert_eq!(combine(&mut t, Some(Turn::Busy), 1000), Turn::Waiting);
    }

    #[test]
    fn busy_registry_beats_idle_hook() {
        let mut t = Track { hook: Some(Turn::Idle), ..Default::default() };
        assert_eq!(combine(&mut t, Some(Turn::Busy), 1000), Turn::Busy);
    }

    #[test]
    fn stale_busy_hook_yields_to_idle_registry() {
        let mut t = Track { hook: Some(Turn::Busy), ..Default::default() };
        assert_eq!(combine(&mut t, Some(Turn::Idle), 1000), Turn::Busy);
        assert_eq!(combine(&mut t, Some(Turn::Idle), 1000 + STALE_HOOK_MS + 1), Turn::Idle);
        assert_eq!(t.hook, Some(Turn::Idle));
    }

    #[test]
    fn stale_waiting_hook_yields_to_idle_registry() {
        let mut t = Track { hook: Some(Turn::Waiting), ..Default::default() };
        // A live prompt: the registry calls the turn busy, waiting stands.
        assert_eq!(combine(&mut t, Some(Turn::Busy), 1000), Turn::Waiting);
        // The turn was dismissed; the registry says idle, and for a while the
        // hook's word still holds, in case a Stop is on its way.
        assert_eq!(combine(&mut t, Some(Turn::Idle), 2000), Turn::Waiting);
        assert_eq!(combine(&mut t, Some(Turn::Idle), 2000 + STALE_WAIT_MS), Turn::Waiting);
        assert_eq!(combine(&mut t, Some(Turn::Idle), 2000 + STALE_WAIT_MS + 1), Turn::Idle);
        assert_eq!(t.hook, Some(Turn::Idle));
    }

    #[test]
    fn stale_busy_registry_yields_to_idle_hook() {
        // Stop arrived at t=1000; the registry keeps saying busy.
        let mut t = Track { hook: Some(Turn::Idle), hook_at_ms: 1000, ..Default::default() };
        assert_eq!(combine(&mut t, Some(Turn::Busy), 1000), Turn::Busy);
        assert_eq!(combine(&mut t, Some(Turn::Busy), 1000 + STALE_DISK_MS), Turn::Busy);
        assert_eq!(combine(&mut t, Some(Turn::Busy), 1000 + STALE_DISK_MS + 1), Turn::Idle);
        // A registry that goes idle and busy again starts the clock over.
        assert_eq!(combine(&mut t, Some(Turn::Idle), 200_000), Turn::Idle);
        assert_eq!(combine(&mut t, Some(Turn::Busy), 200_005), Turn::Busy);
        // And a hook that speaks again is believed at once.
        t.hook = Some(Turn::Busy);
        t.hook_at_ms = 200_010;
        assert_eq!(combine(&mut t, Some(Turn::Busy), 400_000), Turn::Busy);
    }

    #[test]
    fn without_hooks_a_busy_registry_stands() {
        let mut t = Track::default();
        assert_eq!(combine(&mut t, Some(Turn::Busy), 1), Turn::Busy);
        assert_eq!(combine(&mut t, Some(Turn::Busy), 1 + 10 * STALE_DISK_MS), Turn::Busy);
    }

    #[test]
    fn without_hooks_the_disk_decides() {
        let mut t = Track::default();
        assert_eq!(combine(&mut t, Some(Turn::Waiting), 1), Turn::Waiting);
        assert_eq!(combine(&mut t, None, 2), Turn::Idle);
    }
}
