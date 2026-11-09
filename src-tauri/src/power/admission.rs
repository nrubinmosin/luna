//! The agents' calls a rule that drains refuses until it has fired, and those
//! let in before it, which it counts as at work and waits for as it is armed.

use super::*;

/// Told when a call that was let in has ended (`Entered`).
pub(super) static ENTERED_ENDED: Condvar = Condvar::new();

/// Why an agent is to start nothing now, if it is not to.
fn refusal(st: &State) -> Option<String> {
    if let Some(action) = st.fired {
        return Some(format!("Luna's {} is under way, so nothing new starts. End your turn.", action.as_str()));
    }
    let action = st.armed.as_ref().filter(|a| a.drain).map(|a| a.action)?;
    Some(format!(
        "Luna is draining: a {} is armed for when all sessions are quiet, so nothing new starts. What runs may finish, and luna_session_answer (a pending ask only), luna_session_stop and luna_session_delete still work. End your turn.",
        action.as_str()
    ))
}

/// A tool call that may start work, for as long as it runs: counted under the lock
/// that arms a drain, so it is either in before the drain, and waited for, or
/// told it is draining.
pub struct Entered<'a> {
    state: &'a Mutex<State>,
    ended: &'a Condvar,
    refusal: Option<String>,
}

impl Entered<'_> {
    pub fn draining(&self) -> bool {
        self.refusal.is_some()
    }
}

impl Drop for Entered<'_> {
    fn drop(&mut self) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.admitted = st.admitted.saturating_sub(1);
        drop(st);
        self.ended.notify_all();
    }
}

fn enter_on<'a>(state: &'a Mutex<State>, ended: &'a Condvar) -> Entered<'a> {
    let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
    st.admitted += 1;
    Entered { state, ended, refusal: refusal(&st) }
}

/// Held across a `luna_session_answer`, which a drain narrows (`agents::answer`). Not to be
/// taken with the registry's lock held: `tick` takes the two the other way round.
pub fn enter() -> Entered<'static> {
    enter_on(state(), &ENTERED_ENDED)
}

/// Held across a `luna_session_spawn` or `luna_session_send`, which a drain refuses.
pub fn admit() -> Result<Entered<'static>, String> {
    let entered = enter();
    match &entered.refusal {
        Some(why) => Err(why.clone()),
        None => Ok(entered),
    }
}

/// Waits, for as long as `wait`, for the calls let in before a drain to end, new
/// ones being refused by now. Returns how many are left, which still hold the rule.
pub(super) fn settle<'a>(mut st: MutexGuard<'a, State>, ended: &Condvar, wait: std::time::Duration) -> (MutexGuard<'a, State>, usize) {
    let deadline = std::time::Instant::now() + wait;
    while st.admitted > 0 {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            break;
        }
        st = ended.wait_timeout(st, left).unwrap_or_else(|e| e.into_inner()).0;
    }
    let left = st.admitted;
    (st, left)
}

/// Why this machine cannot do `action`, for a rule to be refused before anything
/// is closed for it.
pub(super) fn unsupported(action: Action, windows: bool) -> Option<String> {
    (action == Action::Restart && !windows).then(|| "Luna can restart the PC on Windows only".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_firing_refuses_new_work_with_no_drain_too() {
        for action in [Action::Restart, Action::Shutdown, Action::Sleep] {
            let mut st = armed(action, 60);
            st.holding = true;
            assert_eq!(refusal(&st), None, "armed, not firing");
            assert_eq!(fired(&mut st), vec![Effect::Fire(action)]);
            assert!(refusal(&st).unwrap().contains("under way"), "{action:?}");
            st.firing = false;
            st.fired = None;
            assert_eq!(refusal(&st), None);
        }
    }

    #[test]
    fn restart_is_refused_before_anything_where_the_os_cannot() {
        assert!(unsupported(Action::Restart, false).unwrap().contains("Windows only"));
        assert_eq!(unsupported(Action::Restart, true), None);
        assert_eq!(unsupported(Action::Shutdown, false), None);
        if !cfg!(windows) {
            let e = arm_for_agent("me", "restart", Some(60), Some(true)).unwrap_err();
            assert!(e.contains("Windows only"), "{e}");
        }
    }

    #[test]
    fn a_drain_refuses_until_its_rule_has_fired_and_not_after() {
        let plain = armed(Action::Shutdown, 60);
        assert_eq!(refusal(&plain), None, "arming alone leaves agents free");
        for action in [Action::Sleep, Action::Hibernate, Action::Restart, Action::Shutdown] {
            let mut st = draining(action, 60);
            assert!(refusal(&st).unwrap().contains(&format!("a {} is armed", action.as_str())));
            assert_eq!(fired(&mut st), vec![Effect::Fire(action)]);
            assert!(refusal(&st).is_some(), "still, while it fires");
            // What the firing thread does once it is over.
            st.firing = false;
            st.fired = None;
            assert_eq!(refusal(&st), None, "{action:?}");
            let kept = matches!(action, Action::Sleep | Action::Hibernate);
            assert_eq!(st.armed.is_some(), kept);
        }
    }

    #[test]
    fn a_call_let_in_holds_the_rule_up_and_stops_its_count() {
        let mut st = draining(Action::Restart, 60);
        st.admitted = 1;
        let quiet = board(0, 0, Some(0));
        assert_eq!(advance(&mut st, true, &quiet, 120_000), vec![], "no count over a call that may start work");
        st.admitted = 0;
        assert_eq!(advance(&mut st, true, &quiet, 120_000), vec![Effect::Countdown { quiet_for_s: 120 }]);
        st.admitted = 1;
        assert_eq!(advance(&mut st, true, &quiet, 121_000), vec![Effect::Canceled]);
    }

    #[test]
    fn a_call_let_in_before_a_drain_is_waited_for_and_one_after_it_is_refused() {
        let state = Mutex::new(State::default());
        let ended = Condvar::new();
        let early = enter_on(&state, &ended);
        assert!(!early.draining());
        std::thread::scope(|s| {
            let arming = s.spawn(|| {
                let mut st = state.lock().unwrap();
                st.armed = Some(Armed { action: Action::Restart, quiet_s: 60, armed_at_ms: 0, drain: true });
                settle(st, &ended, std::time::Duration::from_secs(30)).1
            });
            while state.lock().unwrap().armed.is_none() {
                std::thread::yield_now();
            }
            assert!(enter_on(&state, &ended).draining(), "refused from the moment it is armed");
            std::thread::sleep(std::time::Duration::from_millis(50));
            assert!(!arming.is_finished(), "the arming waits for the call let in");
            drop(early);
            assert_eq!(arming.join().unwrap(), 0);
        });
        assert_eq!(state.lock().unwrap().admitted, 0, "a refused call left nothing counted");
        // One that outlasts the wait is said, and still holds the rule.
        let late = enter_on(&state, &ended);
        let st = state.lock().unwrap();
        assert_eq!(settle(st, &ended, std::time::Duration::from_millis(10)).1, 1);
        drop(late);
    }
}
