//! The rule at each read of the records: the machine held while any session
//! is active and let go a minute after, and an armed rule's quiet window,
//! countdown and firing, with the line that says what holds it up.

use super::*;

/// Whether a rule is held up: by an active session (`Life::active`, not known
/// included), or by an agent's call that may yet start one.
fn held_up(st: &State, summary: &Summary) -> bool {
    summary.busy > 0 || st.admitted > 0
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
    Countdown { quiet_for_s: u64 },
    Canceled,
    Fire(Action),
}

/// The automaton proper: pure over the state, so it can be driven by a test.
/// `holding` is only assumed to change where an effect says so.
///
/// Keep-awake holds while any session is active, and a minute after; an armed
/// rule adds nothing to that. The rule fires only after no session has been
/// active for the whole quiet window (`Summary::idle_since_ms`, which any
/// activity starts over) and then for the whole countdown: activity in
/// either starts the window over.
pub(super) fn advance(st: &mut State, keep_awake: bool, summary: &Summary, now: u64) -> Vec<Effect> {
    let mut out = Vec::new();

    // -- keep awake -----------------------------------------------------
    if keep_awake && summary.busy > 0 {
        st.release_at_ms = None;
        if !st.holding {
            out.push(Effect::Hold(true));
        }
    } else if st.holding {
        let at = *st.release_at_ms.get_or_insert(now + HOLD_LINGER_MS);
        if now >= at {
            st.release_at_ms = None;
            out.push(Effect::Hold(false));
        }
    }

    // -- when done --------------------------------------------------------
    if let Some(armed) = st.armed.clone() {
        let blocked = held_up(st, summary);
        if let Some(ends) = st.countdown_ends_at_ms {
            if blocked {
                st.countdown_ends_at_ms = None;
                out.push(Effect::Canceled);
            } else if now >= ends && !st.firing {
                st.firing = true;
                st.countdown_ends_at_ms = None;
                st.fired = Some(armed.action);
                // Its end is reached: what a wake brings is free to work.
                if let Some(a) = st.armed.as_mut() {
                    a.drain = false;
                }
                // Sleep and hibernate keep the arm: the rule applies again after
                // the wake, over a fresh quiet window.
                if matches!(armed.action, Action::Shutdown | Action::Restart | Action::Log) {
                    st.armed = None;
                }
                out.push(Effect::Fire(armed.action));
            }
        } else if !blocked {
            // The window counts from the later of the last activity and the
            // arming: a rule armed on a board quiet for an hour still waits it out.
            let quiet_since = summary.idle_since_ms.map(|t| t.max(armed.armed_at_ms));
            let quiet_for = quiet_since.map(|t| now.saturating_sub(t)).unwrap_or(0);
            if quiet_since.is_some() && quiet_for >= armed.quiet_s * 1000 {
                st.countdown_ends_at_ms = Some(now + COUNTDOWN_MS);
                out.push(Effect::Countdown { quiet_for_s: quiet_for / 1000 });
            }
        }
    }
    out
}

/// The reader's call, once per read of the records.
pub fn tick(summary: &Summary) {
    let now = now_ms();
    let settings = crate::settings::get();
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    let action = st.armed.as_ref().map(|a| a.action);

    for effect in advance(&mut st, settings.keep_awake, summary, now) {
        match effect {
            Effect::Hold(on) => {
                st.holding = os::hold(on);
                if on && st.holding {
                    crate::log::info("power", &format!("awake hold set: {}", blockers(summary)));
                    st.held_since_ms = now;
                    st.hold_logged_ms = now;
                } else if !on && !st.holding {
                    crate::log::info("power", &format!("awake hold let go: nothing active for a minute; last held by {}", st.held_by));
                }
            }
            Effect::Canceled => {
                crate::log::warn("power", &format!("countdown canceled, the quiet window starts over: {}", why_held(&st, summary)));
            }
            Effect::Countdown { quiet_for_s } => {
                let a = action.unwrap_or(Action::Log);
                crate::log::warn("power", &format!("no session active for {quiet_for_s}s; {} in {}s", a.as_str(), COUNTDOWN_MS / 1000));
                notify(
                    &format!("Luna will {} the PC in a minute", verb(a)),
                    "No session has been active for the whole quiet window. Click the power chip in Luna to cancel.",
                );
            }
            Effect::Fire(a) => {
                std::thread::spawn(move || {
                    fire(a);
                    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
                    st.firing = false;
                    st.fired = None;
                });
            }
        }
    }

    if st.holding && summary.busy > 0 {
        st.held_by = blockers(summary);
        if now.saturating_sub(st.hold_logged_ms) >= HOLD_LOG_EVERY_MS {
            st.hold_logged_ms = now;
            let hours = now.saturating_sub(st.held_since_ms) / (60 * 60_000);
            crate::log::info("power", &format!("awake hold kept for {hours} h: {}", st.held_by));
        }
    }

    // -- say why an armed rule is not firing --------------------------------
    if st.armed.is_some() && st.countdown_ends_at_ms.is_none() && !st.firing {
        if held_up(&st, summary) {
            // Changes are judged on who is active and how, not on the full
            // line, whose names of what runs may change at every read.
            let who =
                summary.sessions.iter().filter(|s| s.busy).map(|s| format!("{} {:?}", s.id, s.life.phase)).collect::<Vec<_>>().join(" | ");
            if blocked_log_due(&mut st, &who, now) {
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
        0 => "nothing".to_string(),
        n => format!("{n} agent call(s) that may yet start work"),
    })
}

/// The reader noticed the clock jump: a sleep just ended. The quiet window
/// starts over (activity `reset_idle`), and so does any countdown.
pub fn woke() {
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    if st.countdown_ends_at_ms.take().is_some() {
        crate::log::info("power", "countdown dropped across a sleep");
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
        assert!(!advance(&mut st, false, &at_work, now).iter().any(|e| matches!(e, Effect::Countdown { .. })));
        assert!(!crate::agents::riders_go(false, rest_owed, fresh, now, at_work.busy));
        // All quiet: the count starts, and the line is free to go.
        let quiet = board(0, Some(now - 1000));
        assert!(advance(&mut st, false, &quiet, now).iter().any(|e| matches!(e, Effect::Countdown { .. })));
        assert!(crate::agents::riders_go(false, rest_owed, fresh, now, quiet.busy));
        assert_eq!(advance(&mut st, false, &at_work, now + 1000), vec![Effect::Canceled], "and its caller's turn stops the count");
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
        assert_eq!(advance(&mut st, true, &board(1, None), 0), vec![Effect::Hold(true)]);
        st.holding = true;
        // Held while active; a quiet read starts the linger, not the release.
        assert_eq!(advance(&mut st, true, &board(1, None), 10_000), vec![]);
        assert_eq!(advance(&mut st, true, &board(0, Some(20_000)), 20_000), vec![]);
        assert_eq!(advance(&mut st, true, &board(0, Some(20_000)), 20_000 + HOLD_LINGER_MS - 1), vec![]);
        assert_eq!(advance(&mut st, true, &board(0, Some(20_000)), 20_000 + HOLD_LINGER_MS), vec![Effect::Hold(false)]);
    }

    #[test]
    fn keep_awake_off_does_not_hold() {
        let mut st = State::default();
        assert_eq!(advance(&mut st, false, &board(3, None), 0), vec![]);
    }

    /// Arming adds no hold of its own: a quiet board with a rule armed lets
    /// the machine go as an unarmed one does.
    #[test]
    fn an_armed_rule_does_not_hold_the_machine() {
        let mut st = armed(Action::Sleep, 120);
        assert_eq!(advance(&mut st, true, &board(0, Some(0)), 0), vec![]);
        assert!(!st.holding);
        let mut st = armed(Action::Shutdown, 120);
        st.holding = true;
        assert_eq!(advance(&mut st, true, &board(0, Some(0)), 0), vec![]);
        assert_eq!(advance(&mut st, true, &board(0, Some(0)), HOLD_LINGER_MS), vec![Effect::Hold(false)], "let go a minute after");
    }

    #[test]
    fn arm_quiet_countdown_fire() {
        let mut st = armed(Action::Shutdown, 120);
        // Active: nothing. Quiet but not for long enough: nothing.
        assert_eq!(advance(&mut st, false, &board(1, None), 0), vec![]);
        assert_eq!(advance(&mut st, false, &board(0, Some(1_000)), 60_000), vec![]);
        // Quiet for the window: the countdown starts.
        assert_eq!(advance(&mut st, false, &board(0, Some(1_000)), 121_000), vec![Effect::Countdown { quiet_for_s: 120 }]);
        assert_eq!(st.countdown_ends_at_ms, Some(121_000 + COUNTDOWN_MS));
        // Not yet. Then fire, and shutdown drops the arm.
        assert_eq!(advance(&mut st, false, &board(0, Some(1_000)), 150_000), vec![]);
        assert_eq!(advance(&mut st, false, &board(0, Some(1_000)), 121_000 + COUNTDOWN_MS), vec![Effect::Fire(Action::Shutdown)]);
        assert!(st.armed.is_none());
        assert!(st.firing);
    }

    /// Activity during the countdown cancels it and keeps the arm; the quiet
    /// window then counts from the end of that activity, whole.
    #[test]
    fn activity_during_the_countdown_cancels_and_restarts_the_window() {
        let mut st = armed(Action::Hibernate, 60);
        assert_eq!(advance(&mut st, false, &board(0, Some(0)), 60_000), vec![Effect::Countdown { quiet_for_s: 60 }]);
        assert_eq!(advance(&mut st, false, &board(1, None), 70_000), vec![Effect::Canceled]);
        assert!(st.armed.is_some());
        assert_eq!(st.countdown_ends_at_ms, None);
        // The quiet window starts over from the new quiet stretch.
        assert_eq!(advance(&mut st, false, &board(0, Some(80_000)), 100_000), vec![]);
        assert_eq!(advance(&mut st, false, &board(0, Some(80_000)), 139_999), vec![]);
        assert_eq!(advance(&mut st, false, &board(0, Some(80_000)), 140_000), vec![Effect::Countdown { quiet_for_s: 60 }]);
    }

    /// Activity inside the window, even a short one between two reads that
    /// the board's quiet stamp records, starts the window over.
    #[test]
    fn activity_inside_the_window_starts_it_over() {
        let mut st = armed(Action::Shutdown, 60);
        assert_eq!(advance(&mut st, true, &board(0, Some(0)), 50_000), vec![]);
        // A turn began and ended at 55 s, between reads.
        assert_eq!(advance(&mut st, true, &board(0, Some(55_000)), 60_000), vec![], "not 60 s quiet any more");
        assert_eq!(advance(&mut st, true, &board(0, Some(55_000)), 115_000), vec![Effect::Countdown { quiet_for_s: 60 }]);
    }

    /// A session not known yet, or held on a question, is active: neither
    /// lets the rule count down.
    #[test]
    fn an_unknown_or_asking_session_blocks_the_rule() {
        use crate::activity::{Life, Phase, SessionLife};
        for phase in [Phase::Unknown, Phase::Asking, Phase::Running] {
            let mut st = armed(Action::Shutdown, 60);
            let b = Summary::of(vec![SessionLife::of("s", Life { phase, ..Default::default() })], None);
            assert_eq!(b.busy, 1, "{phase:?}");
            assert_eq!(advance(&mut st, false, &b, 600_000), vec![], "{phase:?}");
            assert_eq!(st.countdown_ends_at_ms, None);
        }
        // Ended on a usage limit, with nothing beside it: not active, the rule goes.
        let limited = Life { phase: Phase::Ended, outcome: Some(crate::activity::Outcome::UsageLimit), ..Default::default() };
        let b = Summary::of(vec![SessionLife::of("s", limited)], Some(0));
        assert_eq!((b.busy, b.stalled), (0, 1));
        let mut st = armed(Action::Shutdown, 60);
        assert_eq!(advance(&mut st, false, &b, 60_000), vec![Effect::Countdown { quiet_for_s: 60 }]);
    }

    /// A rule armed on a board quiet for long waits its whole window from the
    /// arming all the same.
    #[test]
    fn the_window_counts_from_the_arming_at_the_earliest() {
        let mut st = armed(Action::Shutdown, 60);
        st.armed.as_mut().unwrap().armed_at_ms = 1_000_000;
        assert_eq!(advance(&mut st, false, &board(0, Some(0)), 1_030_000), vec![]);
        assert_eq!(advance(&mut st, false, &board(0, Some(0)), 1_060_000), vec![Effect::Countdown { quiet_for_s: 60 }]);
    }

    /// A call an agent made that may start work holds the rule as a session does.
    #[test]
    fn a_call_let_in_holds_the_rule() {
        let mut st = armed(Action::Shutdown, 60);
        st.admitted = 1;
        assert_eq!(advance(&mut st, false, &board(0, Some(0)), 600_000), vec![]);
        st.admitted = 0;
        assert_eq!(advance(&mut st, false, &board(0, Some(0)), 600_000), vec![Effect::Countdown { quiet_for_s: 600 }]);
    }

    #[test]
    fn sleep_keeps_the_arm_after_firing() {
        let mut st = armed(Action::Sleep, 60);
        advance(&mut st, false, &board(0, Some(0)), 60_000);
        assert_eq!(advance(&mut st, false, &board(0, Some(0)), 60_000 + COUNTDOWN_MS), vec![Effect::Fire(Action::Sleep)]);
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
        st.countdown_ends_at_ms = Some(1);
        st.firing = true;
        assert_eq!(advance(&mut st, true, &board(0, Some(0)), 100), vec![]);
    }
}
