//! Boards of sessions and rules armed or draining, for the tests of the rule and
//! of what it lets in.

use super::*;

pub(super) fn board(busy: usize, waiting: usize, idle_since_ms: Option<u64>) -> Summary {
    Summary { busy, waiting, stalled: 0, idle_since_ms, sessions: vec![] }
}

pub(super) fn armed(action: Action, quiet_s: u64) -> State {
    State { armed: Some(Armed { action, quiet_s, armed_at_ms: 0, drain: false }), ..Default::default() }
}

pub(super) fn draining(action: Action, quiet_s: u64) -> State {
    let mut st = armed(action, quiet_s);
    st.armed.as_mut().unwrap().drain = true;
    st.holding = true;
    st
}

/// Runs a rule, all quiet, to its firing.
pub(super) fn fired(st: &mut State) -> Vec<Effect> {
    let quiet = board(0, 0, Some(0));
    advance(st, true, &quiet, 60_000);
    advance(st, true, &quiet, 60_000 + COUNTDOWN_MS)
}
