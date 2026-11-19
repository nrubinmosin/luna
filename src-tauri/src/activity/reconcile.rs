//! What the disk and the jobs say of each terminal session, read every few
//! seconds and folded into its record: Claude Code's registry (its status, and
//! the background shells it names), Codex's rollout and its subagents'
//! rollouts, and the shells a Codex session's turns left in its job. Then the
//! board — these records with every helper's (`background::lives`) — goes to
//! power/.
//!
//! A read is no measurement: it settles what a hook missed (Claude Code), and
//! it is the only word there is where no hook speaks (Codex). Nothing on any
//! screen is read here.

use super::*;

/// How often the disk and the jobs are read.
pub const READ_EVERY: std::time::Duration = std::time::Duration::from_secs(5);
/// What a CLI starts this soon after it is spawned — its MCP servers, the
/// helpers it brings up with itself — came up with it, not with a turn, and is
/// no shell of the session's however early its first turn.
const STARTUP_MS: u64 = 30_000;

/// A Codex session as its rollouts are read.
pub(super) fn codex_live(p: &crate::pty::ActivityProbe) -> crate::codex::session::Live<'_> {
    crate::codex::session::Live {
        chat_id: &p.id,
        cwd: &p.cwd,
        spawned_at_ms: p.spawned_at_ms,
        resume: p.resume.as_deref(),
        account_path: &p.account_path,
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

/// What one read found of one session, taken with no lock held.
#[derive(Default)]
struct Read {
    /// Claude Code's registry: its turn word, when it last set it, and the
    /// background shells it names.
    registry: Option<(Turn, Option<u64>, u32)>,
    /// The API error the transcript's last line is, looked for only while the
    /// registry says a dialog is up.
    api_error: Option<&'static str>,
    rollout: Option<RolloutRead>,
    /// Codex's subagents at work, by their own rollouts.
    agents: Option<Vec<String>>,
    /// Codex: what its job holds born of its turns, by label.
    job: Vec<String>,
    typed_or_resumed: bool,
}

fn read(p: &crate::pty::ActivityProbe, first_turn_ms: u64) -> Read {
    match p.provider {
        Provider::Claude => {
            let registry = crate::claude::session::status(p.pid, p.resume.as_deref(), &p.cwd, p.spawned_at_ms, &p.account_path)
                .map(|(word, at, shells)| (turn_of(&word), at, shells));
            let api_error = registry
                .filter(|(turn, _, _)| *turn == Turn::Waiting)
                .and_then(|_| crate::claude::session::last_api_error(p.pid, p.resume.as_deref(), &p.cwd, p.spawned_at_ms, &p.account_path));
            Read { registry, api_error, ..Default::default() }
        }
        Provider::Codex => {
            let live = codex_live(p);
            // Before its first turn nothing in its job is a turn's.
            let job = match (first_turn_ms, p.job.as_deref()) {
                (0, _) | (_, None) => Vec::new(),
                (first, Some(job)) => crate::procs::shells(job, p.pid, first.max(p.spawned_at_ms as u64 + STARTUP_MS)),
            };
            Read {
                rollout: crate::codex::session::rollout_read(&live),
                agents: crate::codex::session::children_at_work(&live),
                job,
                typed_or_resumed: p.resume.is_some() || crate::pty::manager().typed_at_ms(&p.id) > 0,
                ..Default::default()
            }
        }
    }
}

/// Folds one read into a session's track.
fn take(t: &mut Track, p: &crate::pty::ActivityProbe, r: Read, now: u64) {
    t.codex = p.provider == Provider::Codex;
    // A respawn (new settings, a resume) is a new CLI: what ran beside the
    // old one's turns went with it.
    if t.spawned_at_ms != 0 && t.spawned_at_ms != p.spawned_at_ms {
        t.first_turn_ms = 0;
        t.agents.clear();
        (t.stop_tasks, t.stop_crons) = (0, 0);
    }
    let disk = r.registry.map(|(turn, _, _)| turn);
    // A registry that changed after the hook spoke is the later word: a
    // prompt answered "No" ends the turn with no Stop, and the waiting the
    // hook said would otherwise stand for STALE_WAIT_MS.
    if let Some((Turn::Idle, Some(at), _)) = r.registry {
        if t.hook == Some(Turn::Waiting) && at > t.hook_at_ms {
            t.hook = Some(Turn::Idle);
        }
    }
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
    t.registry_shells = r.registry.map_or(0, |(_, _, shells)| shells);
    t.api_error = r.api_error;
    if let Some(agents) = r.agents {
        // Codex's subagents are what their rollouts say at this read, read
        // whole each time. None of them is known to wait on a prompt.
        t.agents = agents.into_iter().map(|id| (id, false)).collect();
        t.agents_on_disk = true;
    }
    let completed = r.rollout.as_ref().map(|r| r.completed);
    t.rollout = r.rollout;
    t.typed_or_resumed = r.typed_or_resumed;
    t.job = r.job;
    // Codex: the rollout's count is the truth.
    count_on(t, p.spawned_at_ms, completed);
    refold(t, now);
    // Claude Code with hooks: `Stop` counts (on_hook). Without any hook
    // heard, the transitions are all there is.
    let busy = t.life.phase == Phase::Running;
    if !t.codex && t.hook.is_none() && t.was_busy && !busy {
        t.turns_ended += 1;
        t.life.turns_ended = t.turns_ended;
    }
    t.was_busy = busy;
}

/// One pass over every live terminal session, plus the helpers' records as they
/// stand (`helpers`), which keep no track here. Called by the reader thread;
/// public so a test can drive it with probes of its own.
///
/// The registry, rollouts, transcripts and jobs are read outside the state
/// lock: a hook is answered only once `on_hook` has had it, and Claude Code
/// holds its turn until the hook's curl returns.
pub fn reconcile(probes: Vec<crate::pty::ActivityProbe>, helpers: Vec<(String, Life)>) -> Summary {
    let first_turns: Vec<u64> = {
        let st = state().lock().unwrap_or_else(|e| e.into_inner());
        probes.iter().map(|p| st.tracks.get(&p.id).map_or(0, |t| t.first_turn_ms)).collect()
    };
    let reads: Vec<Read> = probes.iter().zip(first_turns).map(|(p, first)| read(p, first)).collect();
    let now = now_ms();
    let mut sessions = Vec::with_capacity(probes.len() + helpers.len());
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    let st = &mut *st;
    let live: Vec<&str> = probes.iter().map(|p| p.id.as_str()).collect();
    drop_gone(&mut st.tracks, &mut st.ended, &live);
    // A turn that began and ended between two reads (a Codex rollout's count
    // moved on) was activity all the same.
    let mut moved = false;
    for (p, r) in probes.iter().zip(reads) {
        let t = track(&mut st.tracks, &mut st.ended, &p.id);
        let before = t.turns_ended;
        take(t, p, r, now);
        moved |= t.turns_ended != before;
        sessions.push(SessionLife::of(&p.id, t.life.clone()));
    }
    sessions.extend(helpers.into_iter().map(|(id, life)| SessionLife::of(&id, life)));
    let active = sessions.iter().any(|s| s.busy);
    if active || moved {
        st.last_active_ms = now;
    }
    st.summary = Summary::of(sessions, (!active).then_some(st.last_active_ms));
    st.summary.clone()
}

/// The usage limit a chat's last turn ended on, off its record: Claude Code's
/// `StopFailure` or its transcript's API error, Codex's rollout. None for a
/// chat not running or not stopped on one.
pub fn limit_of(chat_id: &str) -> Option<String> {
    life(chat_id)?.limit().map(str::to_owned)
}

/// The reader thread: reads, folds, hands the board to power/.
pub fn start() {
    std::thread::Builder::new()
        .name("luna-activity".into())
        .spawn(move || {
            let mut last = std::time::Instant::now();
            loop {
                std::thread::sleep(READ_EVERY);
                let gap = last.elapsed();
                last = std::time::Instant::now();
                // A read that took far longer than its period to come round
                // is the machine having been asleep, not the sessions quiet.
                if gap > READ_EVERY * 6 {
                    crate::log::warn("activity", &format!("clock jumped {gap:?}; quiet stretch reset"));
                    reset_idle();
                    crate::power::woke();
                }
                let summary = reconcile(crate::pty::manager().activity_probes(), crate::background::lives());
                crate::power::tick(&summary);
            }
        })
        .expect("spawn activity thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_the_clis_words() {
        assert_eq!(turn_of("busy"), Turn::Busy);
        assert_eq!(turn_of("working"), Turn::Busy);
        assert_eq!(turn_of("waiting"), Turn::Waiting);
        assert_eq!(turn_of("waiting_for_permission"), Turn::Waiting);
        assert_eq!(turn_of("idle"), Turn::Idle);
        assert_eq!(turn_of("resting"), Turn::Idle);
    }

    /// A probe of a terminal session, as the pty hands one over.
    fn probe(id: &str, provider: Provider) -> crate::pty::ActivityProbe {
        crate::pty::ActivityProbe {
            id: id.into(),
            provider,
            pid: None,
            cwd: String::new(),
            enters: None,
            hold: Default::default(),
            spawned_at_ms: 1,
            resume: None,
            account_path: String::new(),
            job: None,
        }
    }

    fn fold(t: &mut Track, p: &crate::pty::ActivityProbe, r: Read, now: u64) -> Life {
        take(t, p, r, now);
        t.life.clone()
    }

    /// Claude Code's registry, read: its turn word settles the record where
    /// no hook spoke, and a background shell it names keeps an ended turn's
    /// session active (2.1.327's `shell`).
    #[test]
    fn a_claude_registry_read_folds_into_the_record() {
        let p = probe("rc-claude", Provider::Claude);
        let mut t = Track::default();
        assert_eq!(fold(&mut t, &p, Read::default(), 1_000).phase, Phase::Unknown, "no registry yet");
        let busy = || Read { registry: Some((Turn::Busy, Some(1_500), 0)), ..Default::default() };
        assert_eq!(fold(&mut t, &p, busy(), 2_000).phase, Phase::Running);
        let idle = |shells| Read { registry: Some((Turn::Idle, Some(2_500), shells)), ..Default::default() };
        let l = fold(&mut t, &p, idle(1), 3_000);
        assert_eq!((l.phase, l.background.shells, l.turns_ended), (Phase::Ended, 1, 1), "no hooks: a transition counts");
        assert!(l.active());
        assert!(!fold(&mut t, &p, idle(0), 8_000).active());
        // The limit dialog is up and the transcript's last line says why.
        let limit = Read { registry: Some((Turn::Waiting, Some(9_000), 0)), api_error: Some("rate_limit"), ..Default::default() };
        let l = fold(&mut t, &p, limit, 9_500);
        assert_eq!((l.phase, l.outcome, l.limit()), (Phase::Ended, Some(Outcome::UsageLimit), Some("rate_limit")));
        assert!(!l.active());
        let asking = Read { registry: Some((Turn::Waiting, Some(10_000), 0)), ..Default::default() };
        assert_eq!(fold(&mut t, &p, asking, 10_500).phase, Phase::Asking, "a permission prompt, nothing failed");
    }

    /// A prompt answered "No" ends the turn with no Stop: a registry idle
    /// since the hook's waiting is the later word.
    #[test]
    fn a_newer_idle_registry_ends_a_hooks_wait() {
        let p = probe("rc-no", Provider::Claude);
        let mut t = Track { hook: Some(Turn::Waiting), hook_at_ms: 1_000, ..Default::default() };
        let stale = Read { registry: Some((Turn::Idle, Some(900), 0)), ..Default::default() };
        assert_eq!(fold(&mut t, &p, stale, 2_000).phase, Phase::Asking, "the registry's idle is older than the prompt");
        let newer = Read { registry: Some((Turn::Idle, Some(1_500), 0)), ..Default::default() };
        assert_eq!(fold(&mut t, &p, newer, 2_100).phase, Phase::Ended);
    }

    /// Codex: its rollout's own turn boundaries, its count, and its job.
    #[test]
    fn a_codex_rollout_read_folds_into_the_record() {
        let p = probe("rc-codex", Provider::Codex);
        let mut t = Track::default();
        assert_eq!(fold(&mut t, &p, Read::default(), 1_000).phase, Phase::Ended, "no rollout, nobody typed: no turn yet");
        let typed = Read { typed_or_resumed: true, ..Default::default() };
        assert!(fold(&mut t, &p, typed, 1_500).active(), "typed into with no rollout yet: not known");
        let working = Read { rollout: Some(RolloutRead { working: true, ..Default::default() }), ..Default::default() };
        assert_eq!(fold(&mut t, &p, working, 2_000).phase, Phase::Running);
        assert!(t.first_turn_ms > 0, "its job's shells count from here");
        let done = |job: Vec<String>| Read { rollout: Some(RolloutRead { completed: 1, ..Default::default() }), job, ..Default::default() };
        let l = fold(&mut t, &p, done(vec!["sleep.exe (pid 7)".into()]), 3_000);
        assert_eq!((l.phase, l.turns_ended, l.background.shells), (Phase::Ended, 1, 1));
        assert!(l.active());
        let l = fold(&mut t, &p, done(Vec::new()), 4_000);
        assert!(!l.active() && l.turns_ended == 1);
        let aborted = Read {
            rollout: Some(RolloutRead { completed: 2, failed: Some("interrupted".into()), ..Default::default() }),
            ..Default::default()
        };
        let l = fold(&mut t, &p, aborted, 5_000);
        assert_eq!((l.outcome, l.turns_ended), (Some(Outcome::Failed), 2));
        let kid = Read {
            rollout: Some(RolloutRead { completed: 2, ..Default::default() }),
            agents: Some(vec!["kid".into()]),
            ..Default::default()
        };
        let l = fold(&mut t, &p, kid, 6_000);
        assert_eq!((l.phase, l.background.subagents), (Phase::Ended, 1), "a subagent's own rollout says it works");
    }

    /// The board: one active session, unknown included, keeps it from quiet.
    #[test]
    fn the_board_is_quiet_only_with_nothing_active() {
        let quiet = Life { phase: Phase::Ended, ..Default::default() };
        let unknown = Life::default();
        let b = Summary::of(vec![SessionLife::of("a", quiet.clone()), SessionLife::of("b", unknown)], None);
        assert_eq!((b.busy, b.waiting), (1, 0));
        let b = Summary::of(vec![SessionLife::of("a", quiet)], Some(5));
        assert_eq!((b.busy, b.idle_since_ms), (0, Some(5)));
    }
}
