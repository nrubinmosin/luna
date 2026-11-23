//! The agents' calls a rule that drains refuses until it has fired, and those
//! let in before it, which it counts as at work and waits for as it is armed.

use super::*;

/// Told when a call that was let in has ended (`Entered`).
pub(super) static ENTERED_ENDED: Condvar = Condvar::new();

/// Why an agent is to start nothing now, if it is not to.
fn refusal(st: &State) -> Option<String> {
    if st.closing {
        return Some(format!("{SHUTTING_DOWN}: the PC is about to be powered off or put to sleep, so nothing new starts. End your turn."));
    }
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
    /// Admission was closed (`State::closing`): even what a drain lets
    /// through, a structured answer, is refused.
    closed: bool,
}

impl Entered<'_> {
    pub fn draining(&self) -> bool {
        self.refusal.is_some()
    }

    /// Why nothing at all is let in now, a power-off being decided or under way.
    pub fn shut(&self) -> Option<String> {
        self.refusal.clone().filter(|_| self.closed)
    }
}

impl Drop for Entered<'_> {
    fn drop(&mut self) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.admitted = st.admitted.saturating_sub(1);
        drop(st);
        // A call that came and went between two reads was activity all the
        // same: a power-off's window and countdown start over.
        crate::activity::touch();
        self.ended.notify_all();
    }
}

fn enter_on<'a>(state: &'a Mutex<State>, ended: &'a Condvar) -> Entered<'a> {
    crate::activity::touch();
    let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
    st.admitted += 1;
    Entered { state, ended, refusal: refusal(&st), closed: st.closing }
}

/// Held across a `luna_session_answer`, which a drain narrows (`agents::answer`). Not to be
/// taken with the registry's lock held: `tick` takes the two the other way round.
pub fn enter() -> Entered<'static> {
    enter_on(state(), &ENTERED_ENDED)
}

/// Luna typing text into a pane, a paste and its Enter (and what puts a draft
/// aside and back around them) as one piece of work: refused up front while a
/// power-off is being decided or under way, so nothing is pasted that its Enter
/// would then be refused; once let in, counted like an agent's call, so the
/// decision waits on it or is canceled by it, and the pane writes it makes
/// (`pty::write_in`) are not refused half way. A drain does not refuse it:
/// notices and answers are not new work. Dropped, it is activity again.
pub fn typing() -> Result<Entered<'static>, String> {
    typing_on(state(), &ENTERED_ENDED)
}

fn typing_on<'a>(state: &'a Mutex<State>, ended: &'a Condvar) -> Result<Entered<'a>, String> {
    let entered = enter_on(state, ended);
    match entered.shut() {
        Some(why) => Err(why),
        None => Ok(entered),
    }
}

/// A `luna_session_spawn` or `luna_session_send`, which a drain refuses: `work`
/// runs admitted from start to end, so a power-off counts it as at work until
/// whatever it started is on a read.
pub fn admitted<T>(work: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    admitted_on(state(), &ENTERED_ENDED, work)
}

fn admitted_on<T>(state: &Mutex<State>, ended: &Condvar, work: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    let entered = enter_on(state, ended);
    if let Some(why) = &entered.refusal {
        return Err(why.clone());
    }
    let done = work();
    drop(entered);
    done
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
/// is closed for it: a restart needs RunOnce to bring Luna back, and a sleep or
/// hibernate `SetSuspendState`, which only Windows has here (`machine::os`).
pub(super) fn unsupported(action: Action, windows: bool) -> Option<String> {
    if windows {
        return None;
    }
    match action {
        Action::Restart => Some("Luna can restart the PC on Windows only".to_string()),
        Action::Hibernate => Some("Luna can hibernate the PC on Windows only".to_string()),
        Action::Sleep => Some("Luna can put the PC to sleep on Windows only".to_string()),
        Action::Shutdown | Action::Log => None,
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// An agent's call that may start work, as the hub admits it, on a state of
    /// the test's own.
    pub(in crate::power) fn enter_for_test(state: &Mutex<State>) -> Result<(), String> {
        static ENDED: Condvar = Condvar::new();
        admitted_on(state, &ENDED, || Ok(()))
    }

    #[test]
    fn a_firing_refuses_new_work_with_no_drain_too() {
        for action in [Action::Restart, Action::Shutdown, Action::Sleep] {
            let mut st = armed(action, 60);
            assert_eq!(refusal(&st), None, "armed, not firing");
            assert_eq!(fired(&mut st), vec![Effect::Fire(action)]);
            assert!(refusal(&st).unwrap().contains("under way"), "{action:?}");
            st.firing = false;
            st.fired = None;
            assert_eq!(refusal(&st), None);
        }
    }

    #[test]
    fn what_the_os_cannot_do_is_refused_before_anything() {
        for action in [Action::Restart, Action::Hibernate, Action::Sleep] {
            assert!(unsupported(action, false).unwrap().contains("Windows only"), "{action:?}");
            assert_eq!(unsupported(action, true), None, "{action:?}");
        }
        assert_eq!(unsupported(Action::Shutdown, false), None);
        assert_eq!(unsupported(Action::Log, false), None, "logs only, anywhere");
        if !cfg!(windows) {
            for action in ["restart", "hibernate", "sleep"] {
                let e = arm_for_agent("me", action, Some(60), Some(true)).unwrap_err();
                assert!(e.contains("Windows only"), "{e}");
            }
            assert!(state().lock().unwrap().armed.is_none(), "nothing armed on a refusal");
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

    /// A send is at work until it is through, not only while it is checked:
    /// the rule sees it counted for the whole of it.
    #[test]
    fn an_admitted_call_counts_for_the_whole_of_its_work() {
        let state = Mutex::new(State::default());
        let ended = Condvar::new();
        let during = admitted_on(&state, &ended, || Ok(state.lock().unwrap().admitted)).unwrap();
        assert_eq!(during, 1, "counted while it works");
        assert_eq!(state.lock().unwrap().admitted, 0, "and not after");
        close(&mut state.lock().unwrap(), &AtomicBool::new(false), true);
        let shut = admitted_on(&state, &ended, || -> Result<(), String> { panic!("no work runs while admission is closed") });
        assert!(shut.unwrap_err().contains(SHUTTING_DOWN));
    }

    /// Luna's own typing is let in or refused as a whole, up front: refused
    /// while admission is closed with nothing left counted; let in, it holds
    /// a countdown's decision (so the decision cancels, or waits) and a drain
    /// does not refuse it.
    #[test]
    fn typing_is_refused_up_front_when_closed_and_counts_when_let_in() {
        let state = Mutex::new(State::default());
        let ended = Condvar::new();
        let gate = AtomicBool::new(false);
        let typing = typing_on(&state, &ended).expect("open");
        assert_eq!(state.lock().unwrap().admitted, 1);
        // A countdown's decision reads it as work under way.
        let mut st = armed(Action::Shutdown, 60);
        advance(&mut st, false, &board(0, Some(0)), at(60_000));
        st.admitted = state.lock().unwrap().admitted;
        let (now, wall) = on_time(&st, at(60_000 + COUNTDOWN_MS));
        assert_eq!(confirm(&mut st, &board(0, Some(0)), at(0), now, wall), Effect::Canceled);
        drop(typing);
        assert_eq!(state.lock().unwrap().admitted, 0);
        close(&mut state.lock().unwrap(), &gate, true);
        assert!(typing_on(&state, &ended).err().unwrap().contains(SHUTTING_DOWN));
        assert_eq!(state.lock().unwrap().admitted, 0, "a refusal leaves nothing counted");
        close(&mut state.lock().unwrap(), &gate, false);
        // Draining, a notice or an answer still goes in.
        state.lock().unwrap().armed = draining(Action::Shutdown, 60).armed;
        assert!(typing_on(&state, &ended).is_ok());
    }

    #[test]
    fn a_call_let_in_holds_the_rule_up_and_stops_its_count() {
        let mut st = draining(Action::Restart, 60);
        st.admitted = 1;
        let quiet = board(0, Some(0));
        assert_eq!(advance(&mut st, true, &quiet, at(120_000)), vec![], "no count over a call that may start work");
        st.admitted = 0;
        assert_eq!(advance(&mut st, true, &quiet, at(120_000)), vec![Effect::Countdown { quiet_for_s: 120 }]);
        st.admitted = 1;
        assert_eq!(advance(&mut st, true, &quiet, at(121_000)), vec![Effect::Canceled]);
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
                st.armed =
                    Some(Armed { action: Action::Restart, quiet_s: 60, armed_at_ms: 0, armed_at: std::time::Instant::now(), drain: true });
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
