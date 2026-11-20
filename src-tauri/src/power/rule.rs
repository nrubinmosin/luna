//! The rule at each read of the records: the machine held while any session
//! is active and let go a minute after, and an armed rule's quiet window,
//! countdown and firing, with the line that says what holds it up.
//!
//! Every span here is on the monotonic clock (`Instant`): a wall clock set
//! forward shortens nothing. The window and the countdown both run from the
//! last activity of any kind (`Summary::last_activity`, activity/ `touch`),
//! which a turn or a call that came and went between two reads moves too; and
//! the firing itself waits on a fresh read taken at that moment (`confirm`).

use super::*;
use std::time::{Duration, Instant};

/// Whether a rule is held up: by an active session (`Life::active`, not known
/// included), or by an agent's call that may yet start one.
fn held_up(st: &State, summary: &Summary) -> bool {
    summary.busy > 0 || st.admitted > 0
}

/// The last activity a board says, a board made before any read being no
/// quiet at all.
fn last_of(summary: &Summary, now: Instant) -> Instant {
    summary.last_activity.unwrap_or(now)
}

/// One line naming every session that is active, or "" when none is.
fn blockers(summary: &Summary) -> String {
    summary
        .sessions
        .iter()
        .filter(|s| s.busy)
        .map(|s| format!("{} {}", crate::agents::label(&s.id), s.why()))
        .collect::<Vec<_>>()
        .join(" | ")
}

/// Whether the blocked line is due: on a change after a minute's quiet, or
/// every ten minutes while nothing changes. Pure, for the test.
fn blocked_log_due(st: &mut State, why: &str, now: u64) -> bool {
    let since = now.saturating_sub(st.blocked_logged_ms);
    let due = (why != st.blocked_why && since >= BLOCKED_LOG_MIN_MS) || since >= BLOCKED_LOG_EVERY_MS || st.blocked_logged_ms == 0;
    if due {
        st.blocked_why = why.to_string();
        st.blocked_logged_ms = now;
    }
    due
}

/// What one step of the automaton asks the outside world to do.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Effect {
    Hold(bool),
    Countdown {
        quiet_for_s: u64,
    },
    Canceled,
    /// The countdown has run out with nothing seen since it began: a fresh
    /// read decides (`confirm`).
    Due,
    /// The clocks moved across the countdown (a suspend during a read, a
    /// clock set): canceled, and the window starts over.
    Moved,
    Fire(Action),
}

/// The automaton proper: pure over the state, so it can be driven by a test.
/// `holding` is only assumed to change where an effect says so.
///
/// Keep-awake holds while any session is active, and a minute after; an armed
/// rule adds nothing to that. The countdown starts once no session is active
/// and the last activity is a whole quiet window back (and so is the arming);
/// any activity after the countdown began cancels it, and the window then runs
/// from that activity.
pub(super) fn advance(st: &mut State, keep_awake: bool, summary: &Summary, now: Instant) -> Vec<Effect> {
    let mut out = Vec::new();
    let last = last_of(summary, now);

    // -- keep awake -----------------------------------------------------
    if keep_awake && summary.busy > 0 {
        st.release_at = None;
        if !st.holding {
            out.push(Effect::Hold(true));
        }
    } else if st.holding {
        let at = *st.release_at.get_or_insert(now + Duration::from_millis(HOLD_LINGER_MS));
        if now >= at {
            st.release_at = None;
            out.push(Effect::Hold(false));
        }
    }

    // -- when done --------------------------------------------------------
    let Some(armed) = st.armed.clone() else { return out };
    if let Some(c) = st.countdown {
        if held_up(st, summary) || last > c.started {
            st.countdown = None;
            out.push(Effect::Canceled);
        } else if now >= c.ends && !st.firing {
            out.push(Effect::Due);
        }
    } else if !held_up(st, summary) {
        let window = Duration::from_secs(armed.quiet_s);
        let quiet_since = last.max(armed.armed_at);
        let quiet_for = now.saturating_duration_since(quiet_since);
        if quiet_for >= window {
            let wall = now_ms();
            st.countdown = Some(Countdown {
                started: now,
                ends: now + Duration::from_millis(COUNTDOWN_MS),
                started_wall_ms: wall,
                ends_wall_ms: wall + COUNTDOWN_MS,
            });
            out.push(Effect::Countdown { quiet_for_s: quiet_for.as_secs() });
        }
    }
    out
}

/// A countdown that has run out, decided on a fresh read (`fresh`) and the
/// last activity as it stands this moment (`last`): it fires only with no
/// session active and nothing at all since the countdown began; else it is
/// canceled and the window starts over. The two clocks (`now`, `wall_ms`) are
/// read at this moment and checked against the countdown's start: the reads
/// before it were taken as a minute went by, so a span far past that or one
/// the wall clock disagrees with is a suspend or a clock step across a read,
/// and the quiet before it is not to be trusted (`Effect::Moved`).
pub(super) fn confirm(st: &mut State, fresh: &Summary, last: Instant, now: Instant, wall_ms: u64) -> Effect {
    let Some(c) = st.countdown.take() else { return Effect::Canceled };
    let Some(armed) = st.armed.clone() else { return Effect::Canceled };
    let mono = now.saturating_duration_since(c.started);
    let wall = wall_ms as i64 - c.started_wall_ms as i64;
    if crate::activity::span_moved(mono, Duration::from_millis(COUNTDOWN_MS), wall) {
        return Effect::Moved;
    }
    if held_up(st, fresh) || last > c.started || now < c.ends || st.firing {
        return Effect::Canceled;
    }
    st.firing = true;
    st.fired = Some(armed.action);
    // Its end is reached: what a wake brings is free to work.
    if let Some(a) = st.armed.as_mut() {
        a.drain = false;
    }
    // Sleep and hibernate keep the arm: the rule applies again after the
    // wake, over a fresh quiet window.
    if matches!(armed.action, Action::Shutdown | Action::Restart | Action::Log) {
        st.armed = None;
    }
    Effect::Fire(armed.action)
}

/// A countdown that has run out, decided behind a barrier: admission is closed
/// first (`closing`: agents' calls refused, helper turns and Luna's writes into
/// panes too), then a fresh read is taken (`fresh`, with the power lock let go,
/// as a read takes the activity, session and helper locks) and the stamp read
/// (`stamp`) under the lock again, after it. Whatever started before the gate
/// closed is on that read or moved the stamp; whatever comes after is refused,
/// or (a hook, a person's key) moves the stamp too. Fired, admission stays
/// closed until the firing is over; else it opens again and the window starts
/// over. Nothing is closed and the OS is not asked until this has decided. A
/// panic in the read leaves it open again (`Reopen`), as a panic after closing
/// would hold every agent out until a restart.
pub(super) fn barrier<'a>(
    mut st: MutexGuard<'a, State>,
    lock: &'a Mutex<State>,
    gate: &'a AtomicBool,
    fresh: impl FnOnce() -> Summary,
    stamp: impl FnOnce() -> Instant,
    clock: impl Fn() -> (Instant, u64),
) -> (MutexGuard<'a, State>, Summary, Effect) {
    close(&mut st, gate, true);
    drop(st);
    let mut reopen = Reopen { lock, gate, firing: false, armed: true };
    let board = fresh();
    let mut st = lock.lock().unwrap_or_else(|e| e.into_inner());
    let last = stamp();
    let (now, wall) = clock();
    let decided = confirm(&mut st, &board, last, now, wall);
    if !matches!(decided, Effect::Fire(_)) {
        close(&mut st, gate, false);
    }
    reopen.armed = false;
    (st, board, decided)
}

/// Opens admission again when dropped, a panic's unwind included: held across
/// the barrier's read (`armed`) and over a firing (`firing`, which is then
/// over too).
struct Reopen<'a> {
    lock: &'a Mutex<State>,
    gate: &'a AtomicBool,
    firing: bool,
    armed: bool,
}

impl Drop for Reopen<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let mut st = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        if self.firing {
            st.firing = false;
            st.fired = None;
        }
        close(&mut st, self.gate, false);
    }
}

/// What a firing's thread runs: `fire`, then, however it ends, the firing is
/// over: refused, taken back, a wake, a panic. Luna may start work again.
pub(super) fn fired_and_over(lock: &Mutex<State>, gate: &AtomicBool, fire: impl FnOnce()) {
    let _over = Reopen { lock, gate, firing: true, armed: true };
    fire();
}

/// The reader's call, once per read of the records (`summary`), with `fresh`
/// to take another read the moment a countdown runs out.
pub fn tick(summary: &Summary, fresh: impl FnOnce() -> Summary) {
    let now_wall = now_ms();
    let settings = crate::settings::get();
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    let action = st.armed.as_ref().map(|a| a.action);
    let mut effects = advance(&mut st, settings.keep_awake, summary, Instant::now());
    let mut board = summary.clone();
    if effects.contains(&Effect::Due) {
        let decided;
        (st, board, decided) =
            barrier(st, state(), &crate::starts::POWER, fresh, crate::activity::last_activity, || (Instant::now(), now_ms()));
        effects.retain(|e| *e != Effect::Due);
        effects.push(decided);
    }
    let summary = &board;

    for effect in effects {
        match effect {
            Effect::Hold(on) => {
                st.holding = os::hold(on);
                if on && st.holding {
                    crate::log::info("power", &format!("awake hold set: {}", blockers(summary)));
                    st.held_since_ms = now_wall;
                    st.hold_logged_ms = now_wall;
                } else if !on && !st.holding {
                    crate::log::info("power", &format!("awake hold let go: nothing active for a minute; last held by {}", st.held_by));
                }
            }
            Effect::Canceled => {
                crate::log::warn("power", &format!("countdown canceled, the quiet window starts over: {}", why_held(&st, summary)));
            }
            Effect::Moved => {
                crate::log::warn(
                    "power",
                    "the clocks moved across the countdown (a sleep or a clock change during a read); the quiet window starts over",
                );
                crate::activity::reset_idle();
            }
            Effect::Countdown { quiet_for_s } => {
                let a = action.unwrap_or(Action::Log);
                crate::log::warn("power", &format!("no session active for {quiet_for_s}s; {} in {}s", a.as_str(), COUNTDOWN_MS / 1000));
                notify(
                    &format!("Luna will {} the PC in a minute", verb(a)),
                    "No session has been active for the whole quiet window. Click the power chip in Luna to cancel.",
                );
            }
            Effect::Due => {}
            Effect::Fire(a) => {
                std::thread::spawn(move || fired_and_over(state(), &crate::starts::POWER, || fire(a)));
            }
        }
    }

    if st.holding && summary.busy > 0 {
        st.held_by = blockers(summary);
        if now_wall.saturating_sub(st.hold_logged_ms) >= HOLD_LOG_EVERY_MS {
            st.hold_logged_ms = now_wall;
            let hours = now_wall.saturating_sub(st.held_since_ms) / (60 * 60_000);
            crate::log::info("power", &format!("awake hold kept for {hours} h: {}", st.held_by));
        }
    }

    // -- say why an armed rule is not firing --------------------------------
    if st.armed.is_some() && st.countdown.is_none() && !st.firing {
        if held_up(&st, summary) {
            // Changes are judged on who is active and how, not on the full
            // line, whose names of what runs may change at every read.
            let who =
                summary.sessions.iter().filter(|s| s.busy).map(|s| format!("{} {:?}", s.id, s.life.phase)).collect::<Vec<_>>().join(" | ");
            if blocked_log_due(&mut st, &who, now_wall) {
                crate::log::warn("power", &format!("armed, not firing: {}", why_held(&st, summary)));
            }
        } else if !st.blocked_why.is_empty() {
            crate::log::warn("power", "armed, nothing active; countdown when the quiet window is full");
            st.blocked_why.clear();
            st.blocked_logged_ms = 0;
        }
    }

    publish(&mut st, summary);
}

/// What holds a rule up, in a line for the log.
fn why_held(st: &State, summary: &Summary) -> String {
    Some(blockers(summary)).filter(|w| !w.is_empty()).unwrap_or_else(|| match st.admitted {
        0 => "activity since the countdown began".to_string(),
        n => format!("{n} agent call(s) that may yet start work"),
    })
}

/// The reader noticed the clocks move: a sleep just ended, or the clock was
/// set. The quiet window starts over (activity `reset_idle`), and so does any
/// countdown.
pub fn woke() {
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    if st.countdown.take().is_some() {
        crate::log::info("power", "countdown dropped across a sleep or a clock change");
    }
    publish(&mut st, &crate::activity::summary());
}

/// The sessions active now, and of those the ones held on a question or a
/// permission, by their chat names.
pub(super) fn active_names(summary: &Summary, but: Option<&str>) -> (Vec<String>, Vec<String>) {
    let name = |id: &str| crate::agents::chat_name(id).unwrap_or_else(|| id.to_string());
    let active: Vec<&crate::activity::SessionLife> = summary.sessions.iter().filter(|s| s.busy && Some(s.id.as_str()) != but).collect();
    let waiting = active.iter().filter(|s| s.life.phase == crate::activity::Phase::Asking).map(|s| name(&s.id)).collect();
    (active.iter().map(|s| name(&s.id)).collect(), waiting)
}

pub(super) fn plural(n: usize) -> String {
    if n == 1 {
        "1 session is".to_string()
    } else {
        format!("{n} sessions are")
    }
}

pub(super) fn verb(a: Action) -> &'static str {
    match a {
        Action::Shutdown => "shut down",
        Action::Restart => "restart",
        Action::Hibernate => "hibernate",
        Action::Sleep => "put to sleep",
        Action::Log => "log-only",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const END: u64 = 60_000 + COUNTDOWN_MS;

    /// A line held for a notice to ride with (agents/notifier/mod.rs `riders_go`) keeps its
    /// debt open, which is nothing to a power-off: whatever lets one start counting
    /// lets the line through that gate first, and the turn it starts cancels the
    /// count. Only that gate: a line a draft holds back does not stop a count.
    #[test]
    fn a_power_off_does_not_go_ahead_of_a_line_held_back() {
        let mut st = armed(Action::Shutdown, 0);
        let now = 1_000_000;
        let (fresh, rest_owed) = (now - 30_000, false);
        // Someone at work: nothing fires, and the line may wait.
        let at_work = board(1, None);
        assert!(!advance(&mut st, false, &at_work, at(now)).iter().any(|e| matches!(e, Effect::Countdown { .. })));
        assert!(!crate::agents::riders_go(false, rest_owed, fresh, now, at_work.busy));
        // All quiet: the count starts, and the line is free to go.
        let quiet = board(0, Some(now - 1000));
        assert!(advance(&mut st, false, &quiet, at(now)).iter().any(|e| matches!(e, Effect::Countdown { .. })));
        assert!(crate::agents::riders_go(false, rest_owed, fresh, now, quiet.busy));
        assert_eq!(advance(&mut st, false, &at_work, at(now + 1000)), vec![Effect::Canceled], "and its caller's turn stops the count");
    }

    #[test]
    fn restart_fires_once_and_drops_the_arm_as_a_shutdown_does() {
        assert_eq!(Action::parse("restart"), Some(Action::Restart));
        let mut st = armed(Action::Restart, 60);
        assert_eq!(fired(&mut st), vec![Effect::Fire(Action::Restart)]);
        assert!(st.armed.is_none());
    }

    #[test]
    fn hold_follows_activity_with_a_linger() {
        let mut st = State::default();
        assert_eq!(advance(&mut st, true, &board(1, None), at(0)), vec![Effect::Hold(true)]);
        st.holding = true;
        // Held while active; a quiet read starts the linger, not the release.
        assert_eq!(advance(&mut st, true, &board(1, None), at(10_000)), vec![]);
        assert_eq!(advance(&mut st, true, &board(0, Some(20_000)), at(20_000)), vec![]);
        assert_eq!(advance(&mut st, true, &board(0, Some(20_000)), at(20_000 + HOLD_LINGER_MS - 1)), vec![]);
        assert_eq!(advance(&mut st, true, &board(0, Some(20_000)), at(20_000 + HOLD_LINGER_MS)), vec![Effect::Hold(false)]);
    }

    #[test]
    fn keep_awake_off_does_not_hold() {
        let mut st = State::default();
        assert_eq!(advance(&mut st, false, &board(3, None), at(0)), vec![]);
    }

    /// Arming adds no hold of its own: a quiet board with a rule armed lets
    /// the machine go as an unarmed one does.
    #[test]
    fn an_armed_rule_does_not_hold_the_machine() {
        let mut st = armed(Action::Sleep, 120);
        assert_eq!(advance(&mut st, true, &board(0, Some(0)), at(0)), vec![]);
        assert!(!st.holding);
        let mut st = armed(Action::Shutdown, 120);
        st.holding = true;
        assert_eq!(advance(&mut st, true, &board(0, Some(0)), at(0)), vec![]);
        assert_eq!(advance(&mut st, true, &board(0, Some(0)), at(HOLD_LINGER_MS)), vec![Effect::Hold(false)], "let go a minute after");
    }

    #[test]
    fn arm_quiet_countdown_due_fire() {
        let mut st = armed(Action::Shutdown, 120);
        // Active: nothing. Quiet but not for long enough: nothing.
        assert_eq!(advance(&mut st, false, &board(1, None), at(0)), vec![]);
        assert_eq!(advance(&mut st, false, &board(0, Some(1_000)), at(60_000)), vec![]);
        // Quiet for the window: the countdown starts.
        assert_eq!(advance(&mut st, false, &board(0, Some(1_000)), at(121_000)), vec![Effect::Countdown { quiet_for_s: 120 }]);
        // Not yet. Then due, and a fresh read with nothing since fires it.
        assert_eq!(advance(&mut st, false, &board(0, Some(1_000)), at(150_000)), vec![]);
        let end = at(121_000 + COUNTDOWN_MS);
        assert_eq!(advance(&mut st, false, &board(0, Some(1_000)), end), vec![Effect::Due]);
        let (now, wall) = on_time(&st, end);
        assert_eq!(confirm(&mut st, &board(0, Some(1_000)), at(1_000), now, wall), Effect::Fire(Action::Shutdown));
        assert!(st.armed.is_none());
        assert!(st.firing);
    }

    /// A turn that starts and ends inside the countdown, between two reads:
    /// no read saw it active, but it moved the last activity, which cancels.
    #[test]
    fn a_turn_inside_the_countdown_cancels_it() {
        let mut st = armed(Action::Shutdown, 60);
        assert_eq!(advance(&mut st, false, &board(0, Some(0)), at(60_000)), vec![Effect::Countdown { quiet_for_s: 60 }]);
        assert_eq!(advance(&mut st, false, &board(0, Some(80_000)), at(85_000)), vec![Effect::Canceled]);
        assert!(st.armed.is_some() && st.countdown.is_none());
        // The window then runs from that turn, whole.
        assert_eq!(advance(&mut st, false, &board(0, Some(80_000)), at(139_999)), vec![]);
        assert_eq!(advance(&mut st, false, &board(0, Some(80_000)), at(140_000)), vec![Effect::Countdown { quiet_for_s: 60 }]);
    }

    /// The countdown runs out on a quiet read, but a hook came after that read
    /// (a prompt): the fresh read at the moment of firing, or the last
    /// activity as it stands then, cancels it.
    #[test]
    fn the_firing_waits_on_a_fresh_read() {
        let mut st = armed(Action::Shutdown, 60);
        advance(&mut st, false, &board(0, Some(0)), at(60_000));
        assert_eq!(advance(&mut st, false, &board(0, Some(0)), at(END)), vec![Effect::Due]);
        let (now, wall) = on_time(&st, at(END));
        assert_eq!(confirm(&mut st, &board(1, None), at(0), now, wall), Effect::Canceled, "a session active on the fresh read");
        assert!(st.armed.is_some() && !st.firing);
        let mut st = armed(Action::Shutdown, 60);
        advance(&mut st, false, &board(0, Some(0)), at(60_000));
        advance(&mut st, false, &board(0, Some(0)), at(END));
        let (now, wall) = on_time(&st, at(END));
        assert_eq!(confirm(&mut st, &board(0, Some(0)), at(END - 10), now, wall), Effect::Canceled, "a hook since the countdown began");
    }

    /// A countdown run out on a quiet sample, decided behind the barrier.
    fn due() -> (Mutex<State>, AtomicBool, (Instant, u64)) {
        let mut st = armed(Action::Shutdown, 60);
        advance(&mut st, false, &board(0, Some(0)), at(60_000));
        assert_eq!(advance(&mut st, false, &board(0, Some(0)), at(END)), vec![Effect::Due]);
        let clock = on_time(&st, at(END));
        (Mutex::new(st), AtomicBool::new(false), clock)
    }

    /// A hook that starts a turn after the sample the countdown ran out on
    /// moves the stamp, which is read after the fresh read and under the lock:
    /// nothing fires, and admission, closed for the decision, opens again.
    #[test]
    fn a_hook_after_the_sample_cancels_the_firing() {
        let (lock, gate, clock) = due();
        let hook = Mutex::new(at(0));
        let fresh = || {
            // Admission is closed before the read: an agent's call is refused.
            assert!(gate.load(std::sync::atomic::Ordering::SeqCst));
            let call = admission::tests::enter_for_test(&lock);
            assert!(call.unwrap_err().contains(SHUTTING_DOWN));
            // The hook comes now, after the sample and before the decision.
            *hook.lock().unwrap() = at(END + 5);
            board(0, Some(0))
        };
        let (st, _, decided) = barrier(lock.lock().unwrap(), &lock, &gate, fresh, || *hook.lock().unwrap(), || clock);
        assert_eq!(decided, Effect::Canceled);
        assert!(st.armed.is_some() && st.countdown.is_none() && !st.firing && !st.closing);
        assert!(!gate.load(std::sync::atomic::Ordering::SeqCst), "open again");
    }

    /// The Enter a person presses after the fresh read found the rollout
    /// ended moves the stamp (pty/ `write_session`), read after that: the
    /// turn it starts is not shut down under.
    #[test]
    fn an_enter_after_the_rollout_read_cancels_the_firing() {
        let (lock, gate, clock) = due();
        let stamp = Mutex::new(at(0));
        let fresh = || {
            let rollout_ended = board(0, Some(0));
            *stamp.lock().unwrap() = at(END + 1);
            rollout_ended
        };
        let (_st, _, decided) = barrier(lock.lock().unwrap(), &lock, &gate, fresh, || *stamp.lock().unwrap(), || clock);
        assert_eq!(decided, Effect::Canceled);
        // With nothing since, the same barrier fires and keeps admission closed.
        let (lock, gate, clock) = due();
        let (st, _, decided) = barrier(lock.lock().unwrap(), &lock, &gate, || board(0, Some(0)), || at(0), || clock);
        assert_eq!(decided, Effect::Fire(Action::Shutdown));
        assert!(st.closing && gate.load(std::sync::atomic::Ordering::SeqCst), "closed until the firing is over");
    }

    /// An agent's call that comes and goes between two reads leaves nothing
    /// on any read but the last activity it moved: the window starts over.
    #[test]
    fn a_call_between_reads_restarts_the_window() {
        let mut st = armed(Action::Shutdown, 60);
        // Quiet since 0; a send came and went at 50 s (`touch` on its way in and out).
        assert_eq!(advance(&mut st, false, &board(0, Some(50_000)), at(70_000)), vec![], "not a whole window since the call");
        assert_eq!(advance(&mut st, false, &board(0, Some(50_000)), at(110_000)), vec![Effect::Countdown { quiet_for_s: 60 }]);
        // One in the countdown cancels it.
        assert_eq!(advance(&mut st, false, &board(0, Some(115_000)), at(120_000)), vec![Effect::Canceled]);
    }

    /// Only the monotonic clock counts: the deadline the window shows is fixed
    /// as the countdown starts, and the countdown runs out by the monotonic
    /// clock alone (a wall clock set forward is the reader's `clock_moved`,
    /// which starts the window over).
    #[test]
    fn a_wall_clock_jump_shortens_nothing() {
        let mut st = armed(Action::Shutdown, 60);
        assert_eq!(advance(&mut st, false, &board(0, Some(0)), at(60_000)), vec![Effect::Countdown { quiet_for_s: 60 }]);
        let shown = st.countdown.unwrap().ends_wall_ms;
        assert_eq!(advance(&mut st, false, &board(0, Some(0)), at(70_000)), vec![], "ten seconds on, whatever the wall says");
        assert_eq!(st.countdown.unwrap().ends_wall_ms, shown);
    }

    /// A session not known yet, or held on a question, is active: neither
    /// lets the rule count down.
    #[test]
    fn an_unknown_or_asking_session_blocks_the_rule() {
        use crate::activity::{Life, Phase, SessionLife};
        for phase in [Phase::Unknown, Phase::Asking, Phase::Running] {
            let mut st = armed(Action::Shutdown, 60);
            let b = Summary::of(vec![SessionLife::of("s", Life { phase, ..Default::default() })], at(0));
            assert_eq!(b.busy, 1, "{phase:?}");
            assert_eq!(advance(&mut st, false, &b, at(600_000)), vec![], "{phase:?}");
            assert_eq!(st.countdown, None);
        }
        // Ended on a usage limit, with nothing beside it: not active, the rule goes.
        let limited = Life { phase: Phase::Ended, outcome: Some(crate::activity::Outcome::UsageLimit), ..Default::default() };
        let b = Summary::of(vec![SessionLife::of("s", limited)], at(0));
        assert_eq!((b.busy, b.stalled), (0, 1));
        let mut st = armed(Action::Shutdown, 60);
        assert_eq!(advance(&mut st, false, &b, at(60_000)), vec![Effect::Countdown { quiet_for_s: 60 }]);
    }

    /// A board of sessions that have given no evidence yet — just started, a
    /// registry or rollout missing or unreadable, a job that will not list, a
    /// helper taken up after a core restart whose old job nobody saw out —
    /// keeps the PC awake and never lets a power-off count down, however long.
    #[test]
    fn missing_evidence_keeps_the_pc_awake_and_never_fires() {
        use crate::activity::{Background, Life, Phase, SessionLife};
        let mut lives = crate::activity::lives_without_evidence_for_test();
        let restored_helper =
            Life { phase: Phase::Ended, background: Background { unlisted: true, ..Default::default() }, ..Default::default() };
        lives.push(SessionLife::of("restored", restored_helper));
        for one in lives.iter().cloned().map(|s| vec![s]).chain([lives.clone()]) {
            let mut st = armed(Action::Shutdown, 60);
            let b = Summary::of(one, at(0));
            assert!(b.busy > 0, "{:?}", b.sessions);
            assert_eq!(advance(&mut st, true, &b, at(0)), vec![Effect::Hold(true)]);
            st.holding = true;
            for hour in 1..=24 {
                let effects = advance(&mut st, true, &b, at(hour * 3_600_000));
                assert!(effects.is_empty(), "{effects:?}: nothing counts down, nothing lets the PC go");
            }
            assert!(st.countdown.is_none() && st.holding && !st.firing);
        }
    }

    /// A rule armed on a board quiet for long waits its whole window from the
    /// arming all the same.
    #[test]
    fn the_window_counts_from_the_arming_at_the_earliest() {
        let mut st = armed(Action::Shutdown, 60);
        st.armed.as_mut().unwrap().armed_at = at(1_000_000);
        assert_eq!(advance(&mut st, false, &board(0, Some(0)), at(1_030_000)), vec![]);
        assert_eq!(advance(&mut st, false, &board(0, Some(0)), at(1_060_000)), vec![Effect::Countdown { quiet_for_s: 60 }]);
    }

    /// A call an agent made that may start work holds the rule as a session does.
    #[test]
    fn a_call_let_in_holds_the_rule() {
        let mut st = armed(Action::Shutdown, 60);
        st.admitted = 1;
        assert_eq!(advance(&mut st, false, &board(0, Some(0)), at(600_000)), vec![]);
        st.admitted = 0;
        assert_eq!(advance(&mut st, false, &board(0, Some(0)), at(600_000)), vec![Effect::Countdown { quiet_for_s: 600 }]);
    }

    #[test]
    fn sleep_keeps_the_arm_after_firing() {
        let mut st = armed(Action::Sleep, 60);
        assert_eq!(fired(&mut st), vec![Effect::Fire(Action::Sleep)]);
        assert!(st.armed.is_some());
    }

    #[test]
    fn blocked_line_goes_out_on_change_and_on_schedule() {
        let mut st = State::default();
        assert!(blocked_log_due(&mut st, "a mid-turn", 1_000));
        // Same reason, too soon: quiet.
        assert!(!blocked_log_due(&mut st, "a mid-turn", 1_000 + BLOCKED_LOG_MIN_MS));
        // A new reason within the minute waits; after it, goes out.
        assert!(!blocked_log_due(&mut st, "b printing", 1_000 + BLOCKED_LOG_MIN_MS - 1));
        assert!(blocked_log_due(&mut st, "b printing", 1_000 + BLOCKED_LOG_MIN_MS));
        // Nothing changes: the ten-minute heartbeat.
        let t = 1_000 + BLOCKED_LOG_MIN_MS;
        assert!(!blocked_log_due(&mut st, "b printing", t + BLOCKED_LOG_EVERY_MS - 1));
        assert!(blocked_log_due(&mut st, "b printing", t + BLOCKED_LOG_EVERY_MS));
    }

    #[test]
    fn a_firing_in_progress_does_not_fire_twice() {
        let mut st = armed(Action::Sleep, 60);
        st.countdown = Some(Countdown { started: at(0), ends: at(1), started_wall_ms: 0, ends_wall_ms: 0 });
        st.firing = true;
        assert_eq!(advance(&mut st, true, &board(0, Some(0)), at(100)), vec![]);
        assert_eq!(confirm(&mut st, &board(0, Some(0)), at(0), at(100), 100), Effect::Canceled);
    }

    /// A suspend during the fresh read (or the one before it) leaves the
    /// clocks far past the countdown's end: the monotonic clock counts through
    /// it on Windows, the wall clock jumps on a step. Neither fires; admission
    /// opens and the window starts over.
    #[test]
    fn a_suspend_or_clock_step_across_the_read_cancels_the_firing() {
        let (_, _, (_, wall)) = due();
        let ms = |n: u64| Duration::from_millis(n);
        let slept = (at(END) + ms(8 * 3_600_000), wall + 8 * 3_600_000);
        let stepped = (at(END), wall + 3_600_000);
        for clock in [slept, stepped] {
            let (lock, gate, _) = due();
            let (st, _, decided) = barrier(lock.lock().unwrap(), &lock, &gate, || board(0, Some(0)), || at(0), || clock);
            assert_eq!(decided, Effect::Moved);
            assert!(st.armed.is_some() && st.countdown.is_none() && !st.firing && !st.closing, "armed, no count, open");
            assert!(!gate.load(std::sync::atomic::Ordering::SeqCst), "open again");
        }
        // The same read on time fires.
        let (lock, gate, clock) = due();
        let (_st, _, decided) = barrier(lock.lock().unwrap(), &lock, &gate, || board(0, Some(0)), || at(0), || clock);
        assert_eq!(decided, Effect::Fire(Action::Shutdown));
    }

    /// A panic in the barrier's read leaves admission open, not closed for
    /// good; one in the firing too, the firing being over.
    #[test]
    fn a_panic_in_the_read_or_the_firing_leaves_admission_open() {
        let (lock, gate, clock) = due();
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            barrier(lock.lock().unwrap(), &lock, &gate, || panic!("the read failed"), || at(0), || clock)
        }));
        assert!(panicked.is_err());
        assert!(!gate.load(std::sync::atomic::Ordering::SeqCst), "the gate is open");
        let st = lock.lock().unwrap_or_else(|e| e.into_inner());
        assert!(!st.closing && st.armed.is_some(), "and the rule is still armed");
        drop(st);
        // A firing that panics: closed by the barrier that fired, open and over after.
        let (lock, gate, clock) = due();
        let (st, _, decided) = barrier(lock.lock().unwrap(), &lock, &gate, || board(0, Some(0)), || at(0), || clock);
        assert_eq!(decided, Effect::Fire(Action::Shutdown));
        assert!(st.closing && st.firing && gate.load(std::sync::atomic::Ordering::SeqCst));
        drop(st);
        let panicked =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fired_and_over(&lock, &gate, || panic!("the firing failed"))));
        assert!(panicked.is_err());
        let st = lock.lock().unwrap_or_else(|e| e.into_inner());
        assert!(!st.closing && !st.firing && st.fired.is_none());
        assert!(!gate.load(std::sync::atomic::Ordering::SeqCst));
    }
}
