//! The rule at each tick of the sampler: the machine held while a session works
//! and let go a minute after, and an armed rule's quiet window, countdown and
//! firing, with the line that says what holds it up.

use super::*;

/// Whether a rule is held up: by a session, or by a call that may yet start one.
fn held_up(st: &State, summary: &Summary) -> bool {
    summary.busy > 0 || summary.waiting > 0 || st.admitted > 0
}

/// One line naming every session that holds the machine, or "" when none does.
fn blockers(summary: &Summary) -> String {
    summary
        .sessions
        .iter()
        .filter(|s| s.busy || (s.turn == crate::activity::Turn::Waiting && s.stalled.is_none()))
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
pub(super) fn advance(st: &mut State, keep_awake: bool, summary: &Summary, now: u64) -> Vec<Effect> {
    let mut out = Vec::new();

    // -- keep awake -----------------------------------------------------
    let want = (keep_awake && summary.busy > 0) || st.armed.is_some();
    if want {
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
            let quiet_for = summary.idle_since_ms.map(|t| now.saturating_sub(t)).unwrap_or(0);
            if quiet_for >= armed.quiet_s * 1000 {
                st.countdown_ends_at_ms = Some(now + COUNTDOWN_MS);
                out.push(Effect::Countdown { quiet_for_s: quiet_for / 1000 });
            }
        }
    }
    out
}

/// The sampler's call, once per sample.
pub fn tick(summary: &Summary) {
    let now = now_ms();
    let settings = crate::settings::get();
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    let action = st.armed.as_ref().map(|a| a.action);

    let busy_now = || {
        summary
            .sessions
            .iter()
            .filter(|s| s.busy)
            .map(|s| format!("{} {}", crate::agents::label(&s.id), s.why()))
            .collect::<Vec<_>>()
            .join(" | ")
    };
    for effect in advance(&mut st, settings.keep_awake, summary, now) {
        match effect {
            Effect::Hold(on) => {
                st.holding = os::hold(on);
                if on && st.holding {
                    let why = if summary.busy > 0 { busy_now() } else { "armed".to_string() };
                    crate::log::info("power", &format!("awake hold set: {why}"));
                    st.held_since_ms = now;
                    st.hold_logged_ms = now;
                } else if !on && !st.holding {
                    crate::log::info("power", &format!("awake hold let go: nothing busy for a minute; last held by {}", st.held_by));
                }
            }
            Effect::Canceled => {
                crate::log::warn("power", &format!("countdown canceled: {} busy, {} waiting", summary.busy, summary.waiting));
            }
            Effect::Countdown { quiet_for_s } => {
                let a = action.unwrap_or(Action::Log);
                crate::log::warn("power", &format!("all sessions quiet for {quiet_for_s}s; {} in {}s", a.as_str(), COUNTDOWN_MS / 1000));
                let body = match summary.stalled {
                    0 => "Every session is done. Click the power chip in Luna to cancel.".to_string(),
                    n => format!("Every session is done ({n} stopped on a usage limit or another API error). Click the power chip in Luna to cancel."),
                };
                notify(&format!("Luna will {} the PC in a minute", verb(a)), &body);
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
        st.held_by = busy_now();
        if now.saturating_sub(st.hold_logged_ms) >= HOLD_LOG_EVERY_MS {
            st.hold_logged_ms = now;
            let hours = now.saturating_sub(st.held_since_ms) / (60 * 60_000);
            crate::log::info("power", &format!("awake hold kept for {hours} h: {}", st.held_by));
        }
    }

    // -- say why an armed rule is not firing --------------------------------
    if st.armed.is_some() && st.countdown_ends_at_ms.is_none() && !st.firing {
        if held_up(&st, summary) {
            let why = Some(blockers(summary)).filter(|w| !w.is_empty()).unwrap_or_else(|| "an agent's call may yet start work".to_string());
            // Changes are judged on who is holding and how, not on the full
            // line: that carries "printed 12s ago" and whichever grep or
            // sleep happened to be running, so it differed on every check
            // and the line went out every minute for hours.
            let who = summary
                .sessions
                .iter()
                .filter(|s| s.busy || (s.turn == crate::activity::Turn::Waiting && s.stalled.is_none()))
                .map(|s| format!("{} {}", s.id, if s.busy { "busy" } else { "waiting" }))
                .collect::<Vec<_>>()
                .join(" | ");
            if blocked_log_due(&mut st, &who, now) {
                crate::log::warn("power", &format!("armed, not firing: {why}"));
            }
        } else if !st.blocked_why.is_empty() {
            let quiet_for = summary.idle_since_ms.map(|t| now.saturating_sub(t)).unwrap_or(0) / 1000;
            crate::log::warn("power", &format!("armed, all quiet for {quiet_for}s; countdown when the window is full"));
            st.blocked_why.clear();
            st.blocked_logged_ms = 0;
        }
    }

    publish(&mut st, summary);
}

/// The sampler noticed the clock jump: a sleep just ended.
pub fn woke() {
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    if st.countdown_ends_at_ms.take().is_some() {
        crate::log::info("power", "countdown dropped across a sleep");
    }
    publish(&mut st, &crate::activity::summary());
}

/// The sessions that wait for a person, and those stalled on an API error,
/// by their chat names.
pub(super) fn parked(summary: &Summary) -> (Vec<String>, Vec<String>) {
    let name = |id: &str| crate::agents::chat_name(id).unwrap_or_else(|| id.to_string());
    let mut waiting = Vec::new();
    let mut stalled = Vec::new();
    for s in summary.sessions.iter().filter(|s| !s.busy && s.turn == crate::activity::Turn::Waiting) {
        match s.stalled {
            Some(e) => stalled.push(format!("{} ({e})", name(&s.id))),
            None => waiting.push(name(&s.id)),
        }
    }
    (waiting, stalled)
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
        st.holding = true;
        let now = 1_000_000;
        let (fresh, rest_owed) = (now - 30_000, false);
        // Someone at work: nothing fires, and the line may wait.
        let at_work = board(1, 0, None);
        assert!(!advance(&mut st, false, &at_work, now).iter().any(|e| matches!(e, Effect::Countdown { .. })));
        assert!(!crate::agents::riders_go(false, rest_owed, fresh, now, at_work.busy));
        // All quiet: the count starts, and the line is free to go.
        let quiet = board(0, 0, Some(now - 1000));
        assert!(advance(&mut st, false, &quiet, now).iter().any(|e| matches!(e, Effect::Countdown { .. })));
        assert!(crate::agents::riders_go(false, rest_owed, fresh, now, quiet.busy));
        assert_eq!(advance(&mut st, false, &at_work, now + 1000), vec![Effect::Canceled], "and its caller's turn stops the count");
    }

    #[test]
    fn restart_fires_once_and_drops_the_arm_as_a_shutdown_does() {
        assert_eq!(Action::parse("restart"), Some(Action::Restart));
        let mut st = armed(Action::Restart, 60);
        st.holding = true;
        assert_eq!(fired(&mut st), vec![Effect::Fire(Action::Restart)]);
        assert!(st.armed.is_none());
    }

    #[test]
    fn hold_follows_busy_with_a_linger() {
        let mut st = State::default();
        assert_eq!(advance(&mut st, true, &board(1, 0, None), 0), vec![Effect::Hold(true)]);
        st.holding = true;
        // Held while busy; a quiet sample starts the linger, not the release.
        assert_eq!(advance(&mut st, true, &board(1, 0, None), 10_000), vec![]);
        assert_eq!(advance(&mut st, true, &board(0, 0, Some(20_000)), 20_000), vec![]);
        assert_eq!(advance(&mut st, true, &board(0, 0, Some(20_000)), 20_000 + HOLD_LINGER_MS - 1), vec![]);
        assert_eq!(advance(&mut st, true, &board(0, 0, Some(20_000)), 20_000 + HOLD_LINGER_MS), vec![Effect::Hold(false)]);
    }

    #[test]
    fn waiting_does_not_hold_and_keep_awake_off_does_not_hold() {
        let mut st = State::default();
        assert_eq!(advance(&mut st, true, &board(0, 2, None), 0), vec![]);
        assert_eq!(advance(&mut st, false, &board(3, 0, None), 0), vec![]);
    }

    #[test]
    fn armed_holds_regardless() {
        let mut st = armed(Action::Sleep, 120);
        assert_eq!(advance(&mut st, false, &board(0, 0, Some(0)), 0), vec![Effect::Hold(true)]);
    }

    #[test]
    fn arm_quiet_countdown_fire() {
        let mut st = armed(Action::Shutdown, 120);
        st.holding = true;
        // Busy: nothing. Quiet but not for long enough: nothing.
        assert_eq!(advance(&mut st, true, &board(1, 0, None), 0), vec![]);
        assert_eq!(advance(&mut st, true, &board(0, 0, Some(1_000)), 60_000), vec![]);
        // Quiet for the window: the countdown starts.
        assert_eq!(advance(&mut st, true, &board(0, 0, Some(1_000)), 121_000), vec![Effect::Countdown { quiet_for_s: 120 }]);
        assert_eq!(st.countdown_ends_at_ms, Some(121_000 + COUNTDOWN_MS));
        // Not yet. Then fire, and shutdown drops the arm.
        assert_eq!(advance(&mut st, true, &board(0, 0, Some(1_000)), 150_000), vec![]);
        assert_eq!(advance(&mut st, true, &board(0, 0, Some(1_000)), 121_000 + COUNTDOWN_MS), vec![Effect::Fire(Action::Shutdown)]);
        assert!(st.armed.is_none());
        assert!(st.firing);
    }

    #[test]
    fn activity_during_the_countdown_cancels_and_keeps_the_arm() {
        let mut st = armed(Action::Hibernate, 60);
        st.holding = true;
        assert_eq!(advance(&mut st, true, &board(0, 0, Some(0)), 60_000), vec![Effect::Countdown { quiet_for_s: 60 }]);
        assert_eq!(advance(&mut st, true, &board(0, 1, None), 70_000), vec![Effect::Canceled]);
        assert!(st.armed.is_some());
        assert_eq!(st.countdown_ends_at_ms, None);
        // The quiet window starts over from the new idle stretch.
        assert_eq!(advance(&mut st, true, &board(0, 0, Some(80_000)), 100_000), vec![]);
        assert_eq!(advance(&mut st, true, &board(0, 0, Some(80_000)), 140_000), vec![Effect::Countdown { quiet_for_s: 60 }]);
    }

    #[test]
    fn waiting_blocks_the_countdown() {
        let mut st = armed(Action::Shutdown, 60);
        st.holding = true;
        assert_eq!(advance(&mut st, true, &board(0, 1, None), 600_000), vec![]);
        assert_eq!(st.countdown_ends_at_ms, None);
    }

    #[test]
    fn stalled_sessions_do_not_block_the_countdown() {
        // Two sessions sit on the usage-limit dialog; nothing else is up.
        let mut st = armed(Action::Shutdown, 60);
        st.holding = true;
        let b = Summary { stalled: 2, ..board(0, 0, Some(0)) };
        assert_eq!(advance(&mut st, true, &b, 60_000), vec![Effect::Countdown { quiet_for_s: 60 }]);
    }

    #[test]
    fn sleep_keeps_the_arm_after_firing() {
        let mut st = armed(Action::Sleep, 60);
        st.holding = true;
        advance(&mut st, true, &board(0, 0, Some(0)), 60_000);
        assert_eq!(advance(&mut st, true, &board(0, 0, Some(0)), 60_000 + COUNTDOWN_MS), vec![Effect::Fire(Action::Sleep)]);
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
        st.holding = true;
        st.countdown_ends_at_ms = Some(1);
        st.firing = true;
        assert_eq!(advance(&mut st, true, &board(0, 0, Some(0)), 100), vec![]);
    }
}
