//! Boards of sessions and rules armed or draining, for the tests of the rule and
//! of what it lets in.

use super::*;

/// A board of `active` sessions, quiet since `idle_since_ms` when none is.
pub(super) fn board(active: usize, idle_since_ms: Option<u64>) -> Summary {
    Summary { busy: active, waiting: 0, stalled: 0, idle_since_ms, sessions: vec![] }
}

pub(super) fn armed(action: Action, quiet_s: u64) -> State {
    State { armed: Some(Armed { action, quiet_s, armed_at_ms: 0, drain: false }), ..Default::default() }
}

pub(super) fn draining(action: Action, quiet_s: u64) -> State {
    let mut st = armed(action, quiet_s);
    st.armed.as_mut().unwrap().drain = true;
    st
}

/// Runs a rule, all quiet, to its firing.
pub(super) fn fired(st: &mut State) -> Vec<Effect> {
    let quiet = board(0, Some(0));
    advance(st, false, &quiet, 60_000);
    advance(st, false, &quiet, 60_000 + COUNTDOWN_MS)
}
