//! What the disk and the jobs say of each terminal session, read every few
//! seconds and folded into its record: Claude Code's registry (its status, and
//! the background shells it names), Codex's rollout and its subagents'
//! rollouts, and what each session's job holds besides its CLI. Then the board
//! — these records with every helper's (`background::lives`) — goes to power/.
//!
//! A read is no measurement: it settles what a hook missed (Claude Code), and
//! it is the only word there is where no hook speaks (Codex). The one look at
//! a screen is Codex's approval dialog, and only while its rollout says a turn
//! runs: the rollout records no such dialog.

use super::*;

/// How often the disk and the jobs are read.
pub const READ_EVERY: std::time::Duration = std::time::Duration::from_secs(5);
/// The wall clock and the monotonic one may drift apart between two reads by
/// this much before the machine is taken to have slept (Linux's monotonic
/// clock stops in a suspend) or its clock to have been set: either way the
/// quiet window starts over.
const CLOCK_SLACK_MS: i64 = 30_000;

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
    /// background shells it names. None where no entry is its own for sure.
    registry: Option<(Turn, Option<u64>, u32)>,
    /// The API error the transcript's last line is, looked for only while the
    /// registry says a dialog is up.
    api_error: Option<&'static str>,
    rollout: Option<RolloutRead>,
    /// Codex's subagents at work, by their own rollouts.
    agents: Option<Vec<String>>,
    /// What its job holds besides its CLI; None when it would not list.
    job: Option<Vec<(crate::procs::ProcRead, String)>>,
    /// Codex: an approval dialog on its screen while its rollout says a turn runs.
    dialog: bool,
}

fn read(p: &crate::pty::ActivityProbe) -> Read {
    let job = crate::procs::in_job(p.job.as_deref(), p.pid);
    match p.provider {
        Provider::Claude => {
            let registry = crate::claude::session::status(p.pid, p.resume.as_deref(), &p.cwd, p.spawned_at_ms, &p.account_path)
                .map(|(word, at, shells)| (turn_of(&word), at, shells));
            let api_error = registry
                .filter(|(turn, _, _)| *turn == Turn::Waiting)
                .and_then(|_| crate::claude::session::last_api_error(p.pid, p.resume.as_deref(), &p.cwd, p.spawned_at_ms, &p.account_path));
            Read { registry, api_error, job, ..Default::default() }
        }
        Provider::Codex => {
            let live = codex_live(p);
            let rollout = crate::codex::session::rollout_read(&live);
            let working = rollout.as_ref().is_some_and(|r| r.working && !r.unreadable);
            let dialog = working && crate::pty::manager().screen(&p.id).is_some_and(|s| crate::codex::session::approval_on(&s));
            Read { rollout, agents: crate::codex::session::children_at_work(&live), job, dialog, ..Default::default() }
        }
    }
}

/// Folds one read into a session's track. Whether anything it read differs
/// from the read before, or the session is active: activity, either way.
fn take(t: &mut Track, p: &crate::pty::ActivityProbe, r: Read, now: u64) -> bool {
    t.codex = p.provider == Provider::Codex;
    // A respawn (new settings, a resume) is a new CLI: what ran beside the
    // old one's turns went with it, and its furniture is its own.
    if t.spawned_at_ms != 0 && t.spawned_at_ms != p.spawned_at_ms {
        t.agents.clear();
        (t.stop_tasks, t.stop_crons) = (0, 0);
        t.turned = false;
        // Unless its first prompt has gone in already (`submitting`), its
        // furniture taken as it did.
        if t.furnished_for != p.spawned_at_ms {
            t.furniture.clear();
        }
        t.first_turn_ms = None;
    }
    let disk = r.registry.map(|(turn, _, _)| turn);
    let registry_none_named = r.registry.is_some_and(|(_, _, shells)| shells == 0);
    t.disk_at = r.registry.and_then(|(_, at, _)| at);
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
    t.dialog = r.dialog;
    // A turn of this process seen at this read: for Codex its rollout, for
    // Claude Code its registry (its hooks say so at once, `on_hook`).
    let rollout_turned = r.rollout.as_ref().is_some_and(|r| r.working || r.completed > 0 || r.first_started_ms.is_some());
    let turned_now = t.turned || rollout_turned || matches!(disk, Some(Turn::Busy | Turn::Waiting));
    // Until the first turn, all its job holds is the CLI's own. Not at the
    // read that finds the first turn begun: what is in the job now may be its;
    // nor once its first prompt has gone in (`submitting`), which took the
    // last look.
    if !turned_now && t.furnished_for != p.spawned_at_ms {
        t.furniture.extend(r.job.iter().flatten().map(|(r, _)| *r));
    }
    t.turned = turned_now;
    if let Some(first) = r.rollout.as_ref().and_then(|r| r.first_started_ms) {
        t.first_turn_ms.get_or_insert(first);
    }
    let completed = r.rollout.as_ref().filter(|r| !r.unreadable).map(|r| r.completed);
    let len = r.rollout.as_ref().map_or(0, |r| r.len);
    t.rollout = r.rollout;
    t.unlisted = r.job.is_none();
    t.job = r.job;
    // What `Stop` said ran beside the turn goes once a look finds none of it:
    // nothing in the job its turns started, and a registry read that names
    // none. A registry that does not read says nothing, and leaves it.
    if !t.codex && t.stop_tasks > 0 && registry_none_named && shells_of(t).is_some_and(|s| s.is_empty()) {
        t.stop_tasks = 0;
    }
    // Codex: the rollout's count is the truth.
    count_on(t, p.spawned_at_ms, completed);
    refold(t, now);
    // Claude Code: `Stop` counts (on_hook). A turn whose end no hook said — a
    // `Stop` missed and the registry's idle believed, a start hook missed and
    // the registry's busy believed, no hooks at all — counts as its record
    // goes from a turn to none; a busy the registry was left at after a
    // `Stop` began no turn, and ends none.
    let busy = matches!(t.life.phase, Phase::Running | Phase::Asking);
    if !t.codex {
        if busy && t.hook != Some(Turn::Idle) {
            t.ended_by_hook = false;
            t.read_counted = false;
        }
        if busy && t.hook == Some(Turn::Idle) && disk == Some(Turn::Busy) && t.disk_at.is_some_and(|at| at > t.hook_at_ms) {
            t.ended_by_hook = false;
            t.read_counted = false;
        }
        if t.was_busy && !busy && t.life.phase == Phase::Ended && !t.ended_by_hook {
            t.turns_ended += 1;
            t.life.turns_ended = t.turns_ended;
            t.ended_by_hook = true;
            // Its `Stop`, should it come after all, counts nothing (hooks.rs `count_stop`).
            t.read_counted = true;
        }
    }
    t.was_busy = busy;
    let signature = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (t.disk, t.disk_at, t.registry_shells, len, completed, r.dialog).hash(&mut h);
        t.job.as_ref().map(|job| job.iter().map(|(r, _)| *r).collect::<Vec<_>>()).hash(&mut h);
        let mut agents: Vec<&String> = t.agents.keys().collect();
        agents.sort();
        agents.hash(&mut h);
        (t.turns_ended, t.life.phase as u8).hash(&mut h);
        h.finish()
    };
    let changed = signature != t.signature;
    t.signature = signature;
    changed || t.life.active()
}

/// One pass over every live terminal session, plus the helpers' records as they
/// stand (`helpers`), which keep no track here. Called by the reader thread;
/// public so a test can drive it with probes of its own.
///
/// The registry, rollouts, transcripts and jobs are read outside the state
/// lock: a hook is answered only once `on_hook` has had it, and Claude Code
/// holds its turn until the hook's curl returns.
pub fn reconcile(probes: Vec<crate::pty::ActivityProbe>, helpers: Vec<(String, Life)>) -> Summary {
    let reads: Vec<Read> = probes.iter().map(read).collect();
    let now = now_ms();
    let mut sessions = Vec::with_capacity(probes.len() + helpers.len());
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    let st = &mut *st;
    let live: Vec<&str> = probes.iter().map(|p| p.id.as_str()).collect();
    let before = st.tracks.len();
    drop_gone(&mut st.tracks, &mut st.ended, &live);
    let mut moved = st.tracks.len() != before;
    for (p, r) in probes.iter().zip(reads) {
        let t = track(&mut st.tracks, &mut st.ended, &p.id);
        moved |= take(t, p, r, now);
        sessions.push(SessionLife::of(&p.id, t.life.clone()));
    }
    sessions.extend(helpers.into_iter().map(|(id, life)| SessionLife::of(&id, life)));
    if moved || sessions.iter().any(|s| s.busy) {
        stamp(st);
    }
    st.summary = Summary::of(sessions, st.last_activity);
    st.summary.clone()
}

/// The usage limit a chat's last turn ended on, off its record: Claude Code's
/// `StopFailure` or its transcript's API error, Codex's rollout. None for a
/// chat not running or not stopped on one.
pub fn limit_of(chat_id: &str) -> Option<String> {
    life(chat_id)?.limit().map(str::to_owned)
}

/// Whether the clocks say the machine slept or had its clock set between two
/// reads: the monotonic gap far past the read's period, or the wall clock's
/// gap off from the monotonic one. Either way no quiet stretch is to be
/// trusted across it.
fn clock_moved(mono: std::time::Duration, wall_ms: i64) -> bool {
    span_moved(mono, std::time::Duration::ZERO, wall_ms)
}

/// `clock_moved` over a span that was to take `expected` (a countdown): the
/// monotonic time past it far beyond a read's period, or the wall clock's gap
/// off from the monotonic one.
pub fn span_moved(mono: std::time::Duration, expected: std::time::Duration, wall_ms: i64) -> bool {
    mono.saturating_sub(expected) > READ_EVERY * 6 || (wall_ms - mono.as_millis() as i64).abs() > CLOCK_SLACK_MS
}

/// The reader thread: reads, folds, hands the board to power/, with a fresh
/// read for the moment a power-off is due.
pub fn start() {
    std::thread::Builder::new()
        .name("luna-activity".into())
        .spawn(move || {
            let mut last = (std::time::Instant::now(), now_ms() as i64);
            let read = || reconcile(crate::pty::manager().activity_probes(), crate::background::lives());
            loop {
                std::thread::sleep(READ_EVERY);
                let (mono, wall) = (last.0.elapsed(), now_ms() as i64 - last.1);
                last = (std::time::Instant::now(), now_ms() as i64);
                if clock_moved(mono, wall) {
                    crate::log::warn("activity", &format!("clocks moved ({mono:?} monotonic, {wall} ms wall); quiet stretch reset"));
                    reset_idle();
                    crate::power::woke();
                }
                crate::power::tick(&read(), read);
            }
        })
        .expect("spawn activity thread");
}

/// The records of terminal sessions none of whose sources has said anything
/// yet, each as a read folds it, the missing evidence made by what the disk
/// does: a session just started (no registry entry, no rollout), a registry
/// entry or a `sessions` folder that will not read, a Codex rollout that
/// holds no turn yet or that will not open. Each is a real read (`read`) of
/// files in a folder of the test's own. One thing is not made so: a job that
/// will not list, as the OS cannot be made to refuse a snapshot here, so
/// those two are `Read`s by hand. For the power rule's test that missing
/// evidence never reads as quiet.
#[cfg(test)]
pub(crate) fn lives_without_evidence_for_test() -> Vec<SessionLife> {
    let root = std::env::temp_dir().join(format!("luna-no-evidence-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let now = now_ms();
    let spawned = now as u128 - 60_000;
    let probe = |tag: &str, provider, account: &std::path::Path, pid: Option<u32>| crate::pty::ActivityProbe {
        id: format!("no-evidence-{tag}"),
        provider,
        pid,
        cwd: "E:\\p".into(),
        enters: None,
        hold: Default::default(),
        spawned_at_ms: spawned,
        resume: None,
        account_path: account.to_string_lossy().into_owned(),
        job: None,
    };
    let account = |tag: &str| {
        let dir = root.join(tag);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    };
    let mut lives = Vec::new();
    let mut folded = |p: &crate::pty::ActivityProbe, r: Read| {
        let mut t = Track::default();
        take(&mut t, p, r, now);
        lives.push(SessionLife::of(&p.id, t.life.clone()));
    };

    // Claude Code: no registry entry yet; an entry that is a folder, which
    // will not read; and a `sessions` that is a file, which will not list.
    let none = probe("claude-none", Provider::Claude, &account("claude-none"), Some(4_001));
    folded(&none, read(&none));
    let acct = account("claude-entry");
    std::fs::create_dir_all(acct.join("sessions").join("4002.json")).unwrap();
    let entry = probe("claude-entry", Provider::Claude, &acct, Some(4_002));
    folded(&entry, read(&entry));
    let acct = account("claude-dir");
    std::fs::write(acct.join("sessions"), "not a folder").unwrap();
    let dir = probe("claude-dir", Provider::Claude, &acct, Some(4_003));
    folded(&dir, read(&dir));

    // Codex: no rollout yet; one with no turn in it; one that will not open.
    let codex_none = probe("codex-none", Provider::Codex, &account("codex"), None);
    folded(&codex_none, read(&codex_none));
    let acct = account("codex-rollouts");
    crate::codex::session::cli_rollout_for_test(&acct, now - 30_000, "evidence-empty");
    let blank = probe("codex-blank", Provider::Codex, &acct, None);
    let blank_read = read(&blank);
    assert!(blank_read.rollout.as_ref().is_some_and(|r| r.len > 0 && !r.seen_turn && !r.unreadable), "a rollout with no turn in it");
    folded(&blank, blank_read);
    let shut = crate::codex::session::cli_rollout_for_test(&acct, now - 20_000, "evidence-shut");
    let unreadable = probe("codex-shut", Provider::Codex, &acct, None);
    // Found while it reads, then kept from being read.
    assert!(read(&unreadable).rollout.is_some_and(|r| !r.unreadable));
    if let Some(_held) = deny_reads(&shut) {
        let r = read(&unreadable);
        assert!(r.rollout.as_ref().is_some_and(|r| r.unreadable), "a rollout that will not open says so");
        folded(&unreadable, r);
    }

    // A job that will not list, beside a turn the sources say ended.
    let by_hand = probe("hand", Provider::Claude, &account("hand"), None);
    folded(&by_hand, Read { registry: Some((Turn::Idle, None, 0)), job: None, ..Default::default() });
    let by_hand = probe("hand-codex", Provider::Codex, &account("hand-codex"), None);
    let ended = RolloutRead { seen_turn: true, completed: 1, len: 900, ..Default::default() };
    folded(&by_hand, Read { rollout: Some(ended), job: None, ..Default::default() });

    for p in [&codex_none, &blank, &unreadable] {
        crate::codex::session::release(&p.id);
    }
    let _ = std::fs::remove_dir_all(&root);
    lives
}

/// Keeps `path` from being opened for as long as what is returned lives: held
/// open without sharing on Windows, mode 000 elsewhere. None where the machine
/// cannot refuse the read (a process that is root).
#[cfg(test)]
fn deny_reads(path: &std::path::Path) -> Option<Box<dyn std::any::Any>> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(path).ok()?;
        Some(Box::new(held))
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000)).ok()?;
        std::fs::File::open(path).is_err().then(|| Box::new(()) as Box<dyn std::any::Any>)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Missing evidence is never idle: every such record is active.
    #[test]
    fn missing_evidence_is_active() {
        for s in lives_without_evidence_for_test() {
            assert!(s.busy && s.life.active(), "{:?}", s.life);
        }
    }

    /// A rollout that will not open or read says nothing of its turn: the
    /// record, which the reads fold, is not known (active), never the ended
    /// the scan before had it as, and so are a registry entry that will not
    /// read and a `sessions` that will not list. By real files, as a read
    /// takes them.
    #[test]
    fn what_the_disk_will_not_say_reads_unknown_on_the_record() {
        let lives = lives_without_evidence_for_test();
        let of = |tag: &str| lives.iter().find(|s| s.id == format!("no-evidence-{tag}")).map(|s| s.life.phase);
        for tag in ["claude-none", "claude-entry", "claude-dir", "codex-none", "codex-blank"] {
            assert_eq!(of(tag), Some(Phase::Unknown), "{tag}");
        }
        // The one the machine could not be made to refuse a read of (root, off Windows).
        if cfg!(windows) {
            assert_eq!(of("codex-shut"), Some(Phase::Unknown), "a rollout that will not open");
        }
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

    /// A sleep, a clock set forward or back: the quiet window starts over. A
    /// read on time with both clocks agreeing does not.
    #[test]
    fn a_suspend_or_a_clock_step_is_told() {
        let s = std::time::Duration::from_secs;
        assert!(!clock_moved(s(5), 5_000));
        assert!(!clock_moved(s(6), 5_200), "a slow read");
        assert!(clock_moved(s(31), 31_000), "a long gap on the monotonic clock: asleep (Windows)");
        assert!(clock_moved(s(5), 3_600_000), "the wall clock jumped an hour, or Linux slept with its monotonic clock stopped");
        assert!(clock_moved(s(5), -60_000), "the wall clock set back");
        assert!(!span_moved(s(65), s(60), 65_000), "a countdown decided on time");
        assert!(span_moved(s(60 + 31), s(60), 91_000), "a countdown whose read outlasted a suspend");
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

    fn proc(pid: u32, started_ms: u64, name: &str) -> (crate::procs::ProcRead, String) {
        (crate::procs::ProcRead { pid, started_ms }, name.to_string())
    }

    /// Claude Code's registry, read: its turn word settles the record where
    /// no hook spoke, and a background shell it names keeps an ended turn's
    /// session active (2.1.327's `shell`).
    #[test]
    fn a_claude_registry_read_folds_into_the_record() {
        let p = probe("rc-claude", Provider::Claude);
        let mut t = Track::default();
        let job = || Some(Vec::new());
        assert_eq!(fold(&mut t, &p, Read { job: job(), ..Default::default() }, 1_000).phase, Phase::Unknown, "no registry yet");
        let busy = || Read { registry: Some((Turn::Busy, Some(1_500), 0)), job: job(), ..Default::default() };
        assert_eq!(fold(&mut t, &p, busy(), 2_000).phase, Phase::Running);
        let idle = |shells| Read { registry: Some((Turn::Idle, Some(2_500), shells)), job: job(), ..Default::default() };
        let l = fold(&mut t, &p, idle(1), 3_000);
        assert_eq!((l.phase, l.background.shells, l.turns_ended), (Phase::Ended, 1, 1), "no hooks: a transition counts");
        assert!(l.active());
        assert!(!fold(&mut t, &p, idle(0), 8_000).active());
        // The limit dialog is up and the transcript's last line says why.
        let limit =
            Read { registry: Some((Turn::Waiting, Some(9_000), 0)), api_error: Some("rate_limit"), job: job(), ..Default::default() };
        let l = fold(&mut t, &p, limit, 9_500);
        assert_eq!((l.phase, l.outcome, l.limit()), (Phase::Ended, Some(Outcome::UsageLimit), Some("rate_limit")));
        assert!(!l.active());
        let asking = Read { registry: Some((Turn::Waiting, Some(10_000), 0)), job: job(), ..Default::default() };
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

    /// A busy hook just said is not aged out by registry idle time that ran up
    /// before it; a `Stop` missed still is, by the time since the hook, once
    /// the registry set its idle after the hook, and the turn it ended counts,
    /// so a `wait` returns. That `Stop`, come late, does not count it again.
    #[test]
    fn a_claude_hook_ages_only_from_its_own_time_and_a_missed_stop_counts_once() {
        let p = probe("rc-age", Provider::Claude);
        let idle = |at| Read { registry: Some((Turn::Idle, Some(at), 0)), job: Some(Vec::new()), ..Default::default() };
        let mut t = Track::default();
        fold(&mut t, &p, idle(500), 1_000);
        // Idle a minute; then a prompt's hook.
        apply(&mut t, HookEvent::Submitted, 61_000);
        let l = fold(&mut t, &p, idle(500), 62_000);
        assert_eq!(l.phase, Phase::Running, "an idle that ran up before the hook says nothing of it");
        let l = fold(&mut t, &p, idle(500), 61_000 + STALE_HOOK_MS + 1);
        assert_eq!(l.phase, Phase::Running, "nor does it ever, set before the hook");
        let l = fold(&mut t, &p, idle(61_500), 61_000 + 2 * STALE_HOOK_MS);
        assert_eq!((l.phase, l.turns_ended), (Phase::Ended, 1), "idle set since the hook, for long: a Stop missed, and a turn ended");
        // The Stop, late: the same turn, counted once.
        apply(&mut t, HookEvent::Idle, 61_000 + 2 * STALE_HOOK_MS + 10);
        assert_eq!(t.turns_ended, 1);
        // The next turn's counts as ever.
        apply(&mut t, HookEvent::Submitted, 200_000);
        apply(&mut t, HookEvent::Idle, 201_000);
        assert_eq!(t.turns_ended, 2);
    }

    /// A `Stop` heard, then a turn whose start hook was missed: the registry's
    /// busy, set after the `Stop`, stands however long, and its end counts.
    #[test]
    fn a_newer_registry_busy_stands_after_a_missed_start_hook() {
        let p = probe("rc-start", Provider::Claude);
        let mut t = Track { hook: Some(Turn::Idle), hook_at_ms: 1_000, ended_by_hook: true, ..Default::default() };
        let busy = Read { registry: Some((Turn::Busy, Some(2_000), 0)), job: Some(Vec::new()), ..Default::default() };
        assert_eq!(fold(&mut t, &p, busy, 3_000).phase, Phase::Running);
        let busy = Read { registry: Some((Turn::Busy, Some(2_000), 0)), job: Some(Vec::new()), ..Default::default() };
        assert_eq!(fold(&mut t, &p, busy, 3_000 + 10 * STALE_DISK_MS).phase, Phase::Running, "a genuine turn is not aged out");
        let idle = Read { registry: Some((Turn::Idle, Some(900_000), 0)), job: Some(Vec::new()), ..Default::default() };
        let l = fold(&mut t, &p, idle, 900_100);
        assert_eq!((l.phase, l.turns_ended), (Phase::Ended, 1));
        // A busy left over from before a Stop begins no turn, and ends none.
        let mut t = Track { hook: Some(Turn::Idle), hook_at_ms: 1_000, ended_by_hook: true, ..Default::default() };
        let left = || Read { registry: Some((Turn::Busy, Some(500), 0)), job: Some(Vec::new()), ..Default::default() };
        fold(&mut t, &p, left(), 1_500);
        let l = fold(&mut t, &p, left(), 1_500 + STALE_DISK_MS + 1);
        assert_eq!((l.phase, l.turns_ended), (Phase::Ended, 0));
    }

    /// Claude Code before 2.1.327 says nothing of its background shells: the
    /// job does. What was there before the first turn is its own furniture.
    #[test]
    fn a_claude_shell_is_seen_in_its_job_whatever_the_version() {
        let p = probe("rc-shell", Provider::Claude);
        let mut t = Track::default();
        let mcp = proc(10, 100, "node.exe");
        let read = |turn: Turn, job: Vec<(crate::procs::ProcRead, String)>| Read {
            registry: Some((turn, None, 0)),
            job: Some(job),
            ..Default::default()
        };
        fold(&mut t, &p, read(Turn::Idle, vec![mcp.clone()]), 1_000);
        // The first turn, by its hook: the job then is the CLI's own.
        t.hook = Some(Turn::Busy);
        t.turned = true;
        let build = proc(20, 5_000, "cargo.exe");
        fold(&mut t, &p, read(Turn::Busy, vec![mcp.clone(), build.clone()]), 6_000);
        // `Stop` with no lists (2.1.325): ended, the build still in its job.
        t.hook = Some(Turn::Idle);
        t.hook_at_ms = 7_000;
        t.ended_by_hook = true;
        let l = fold(&mut t, &p, read(Turn::Idle, vec![mcp.clone(), build]), 8_000);
        assert_eq!((l.phase, l.background.shells), (Phase::Ended, 1));
        assert_eq!(l.background.names, ["cargo.exe (pid 20)"]);
        assert!(l.active(), "the PC stays on under the build");
        let l = fold(&mut t, &p, read(Turn::Idle, vec![mcp]), 9_000);
        assert!(!l.active(), "the MCP server is the CLI's own");
        // A job that will not list: not known.
        let l = fold(&mut t, &p, Read { registry: Some((Turn::Idle, None, 0)), job: None, ..Default::default() }, 10_000);
        assert!(l.background.unlisted && l.active());
    }

    /// `Stop` named a task; once the job and the registry show none of it, it
    /// no longer holds the PC.
    #[test]
    fn a_stop_count_clears_once_a_look_finds_nothing() {
        let p = probe("rc-stop", Provider::Claude);
        let mut t = Track { hook: Some(Turn::Idle), turned: true, stop_tasks: 1, ended_by_hook: true, ..Default::default() };
        let job =
            |j: Vec<(crate::procs::ProcRead, String)>| Read { registry: Some((Turn::Idle, None, 0)), job: Some(j), ..Default::default() };
        assert!(fold(&mut t, &p, job(vec![proc(5, 9, "bash.exe")]), 1_000).active(), "its shell is still there");
        assert_eq!(t.stop_tasks, 1);
        assert!(!fold(&mut t, &p, job(Vec::new()), 2_000).active(), "gone: let go");
        assert_eq!(t.stop_tasks, 0);
        // A job that will not list proves nothing gone.
        let mut t = Track { hook: Some(Turn::Idle), turned: true, stop_tasks: 1, ended_by_hook: true, ..Default::default() };
        fold(&mut t, &p, Read { registry: Some((Turn::Idle, None, 0)), job: None, ..Default::default() }, 1_000);
        assert_eq!(t.stop_tasks, 1);
        // Nor does a registry that will not read, the job empty: a background
        // agent of the turn's is in no job.
        assert!(fold(&mut t, &p, Read { registry: None, job: Some(Vec::new()), ..Default::default() }, 2_000).active());
        assert_eq!(t.stop_tasks, 1);
    }

    /// The furniture is taken as the first prompt goes in (`submitting`): what
    /// a turn starts after that is no furniture, though the read that comes
    /// before the rollout shows the turn sees it in the job first.
    #[test]
    fn what_comes_after_the_first_prompt_is_no_furniture() {
        let p = probe("rc-furnish", Provider::Codex);
        let mut t = Track::default();
        let mcp = proc(10, 100, "node.exe");
        let shell = proc(20, 3_000, "bash.exe");
        fold(&mut t, &p, Read { job: Some(vec![mcp.clone()]), ..Default::default() }, 1_000);
        furnish(&mut t, p.spawned_at_ms, Some(vec![mcp.clone()]));
        // The turn's shell is up; the rollout says nothing of the turn yet.
        fold(&mut t, &p, Read { job: Some(vec![mcp.clone(), shell.clone()]), ..Default::default() }, 4_000);
        assert!(!t.furniture.contains(&shell.0), "taken after the first prompt");
        let ended = Read {
            rollout: Some(RolloutRead { seen_turn: true, completed: 1, len: 10, ..Default::default() }),
            job: Some(vec![mcp, shell]),
            ..Default::default()
        };
        let l = fold(&mut t, &p, ended, 6_000);
        assert_eq!((l.phase, l.background.names.as_slice()), (Phase::Ended, ["bash.exe (pid 20)".to_string()].as_slice()));
        assert!(l.active(), "the shell the turn left holds the PC");
    }

    /// Codex: its rollout's own turn boundaries, its count, and its job against
    /// what was in it before the first turn, a first turn begun and ended
    /// between two reads included.
    #[test]
    fn a_codex_rollout_read_folds_into_the_record() {
        let p = probe("rc-codex", Provider::Codex);
        let mut t = Track::default();
        let mcp = proc(10, 100, "node.exe");
        let quiet = || Read { job: Some(vec![mcp.clone()]), ..Default::default() };
        assert_eq!(fold(&mut t, &p, quiet(), 1_000).phase, Phase::Unknown, "no rollout yet: not known");
        assert!(fold(&mut t, &p, quiet(), 1_500).active(), "and active");
        // The first turn starts a server early and ends, all between two reads.
        let server = proc(20, 3_000, "node.exe");
        let done = |completed, job| Read {
            rollout: Some(RolloutRead {
                seen_turn: true,
                completed,
                first_started_ms: Some(2_000),
                len: 10 * completed as u64,
                ..Default::default()
            }),
            job: Some(job),
            ..Default::default()
        };
        let l = fold(&mut t, &p, done(1, vec![mcp.clone(), server.clone()]), 6_000);
        assert_eq!((l.phase, l.turns_ended, l.background.shells), (Phase::Ended, 1, 1), "{l:?}");
        assert!(l.active(), "the server it left");
        let l = fold(&mut t, &p, done(1, vec![mcp.clone()]), 7_000);
        assert!(!l.active());
        // The rollout found once and not now: not known.
        let lost = Read { job: Some(vec![mcp.clone()]), ..Default::default() };
        assert_eq!(fold(&mut t, &p, lost, 8_000).phase, Phase::Unknown);
        let unreadable = Read {
            rollout: Some(RolloutRead { unreadable: true, ..Default::default() }),
            job: Some(vec![mcp.clone()]),
            ..Default::default()
        };
        assert_eq!(fold(&mut t, &p, unreadable, 9_000).phase, Phase::Unknown);
        // A turn on an approval dialog asks.
        let dialog = Read {
            rollout: Some(RolloutRead { seen_turn: true, working: true, completed: 1, len: 20, ..Default::default() }),
            job: Some(vec![mcp]),
            dialog: true,
            ..Default::default()
        };
        assert_eq!(fold(&mut t, &p, dialog, 10_000).phase, Phase::Asking);
    }

    /// A record first made when its session is already past its first turn
    /// (its record lost, as after a core restart, its CLI still running): what
    /// its job holds then is not taken for furniture, as nothing says it came
    /// up with the CLI; it counts until it goes. A respawn starts the
    /// furniture over, for the new CLI.
    #[test]
    fn a_session_first_seen_past_its_first_turn_counts_what_its_job_holds() {
        let p = probe("rc-late", Provider::Codex);
        let mut t = Track::default();
        let left = proc(30, 50, "python.exe");
        let read = |job| Read {
            rollout: Some(RolloutRead { seen_turn: true, completed: 2, len: 40, ..Default::default() }),
            job: Some(job),
            ..Default::default()
        };
        let l = fold(&mut t, &p, read(vec![left.clone()]), 1_000);
        assert!(l.active() && l.background.names == ["python.exe (pid 30)"], "{l:?}");
        // Respawned: a new CLI process with nothing of its own turns yet.
        let again = crate::pty::ActivityProbe { spawned_at_ms: 2, ..probe("rc-late", Provider::Codex) };
        let fresh = Read { job: Some(vec![proc(40, 60, "node.exe")]), ..Default::default() };
        take(&mut t, &again, fresh, 2_000);
        assert!(!t.turned && t.furniture.contains(&crate::procs::ProcRead { pid: 40, started_ms: 60 }));
    }

    /// The board: activity of any kind, a read whose inputs changed with no
    /// session active included, moves the last-activity stamp.
    #[test]
    fn a_change_between_reads_is_activity() {
        let p = probe("rc-sig", Provider::Codex);
        let mut t = Track::default();
        let r = |len| Read {
            rollout: Some(RolloutRead { seen_turn: true, completed: 1, len, ..Default::default() }),
            job: Some(Vec::new()),
            ..Default::default()
        };
        assert!(take(&mut t, &p, r(10), 1_000), "first sight");
        assert!(!take(&mut t, &p, r(10), 2_000), "nothing changed, nothing active");
        assert!(take(&mut t, &p, r(20), 3_000), "its rollout grew: a turn came and went between reads");
    }

    /// The board: one active session, unknown included, keeps it from quiet.
    #[test]
    fn the_board_is_quiet_only_with_nothing_active() {
        let now = std::time::Instant::now();
        let quiet = Life { phase: Phase::Ended, ..Default::default() };
        let b = Summary::of(vec![SessionLife::of("a", quiet.clone()), SessionLife::of("b", Life::default())], now);
        assert_eq!((b.busy, b.waiting, b.idle_since_ms), (1, 0, None));
        let b = Summary::of(vec![SessionLife::of("a", quiet)], now);
        assert!(b.busy == 0 && b.idle_since_ms.is_some() && b.last_activity == Some(now));
    }
}
