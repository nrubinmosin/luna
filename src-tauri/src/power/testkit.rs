//! Boards of sessions and rules armed or draining, for the tests of the rule and
//! of what it lets in, all on one monotonic clock that starts at `at(0)`.

use super::*;
use std::time::{Duration, Instant};

/// `ms` past a moment fixed for the whole test run.
pub(super) fn at(ms: u64) -> Instant {
    static BASE: OnceLock<Instant> = OnceLock::new();
    *BASE.get_or_init(Instant::now) + Duration::from_millis(ms)
}

/// A board of `active` sessions, the last activity at `last_ms` (None: now).
pub(super) fn board(active: usize, last_ms: Option<u64>) -> Summary {
    Summary { busy: active, waiting: 0, stalled: 0, idle_since_ms: None, last_activity: last_ms.map(at), sessions: vec![] }
}

pub(super) fn armed(action: Action, quiet_s: u64) -> State {
    State { armed: Some(Armed { action, quiet_s, armed_at_ms: 0, armed_at: at(0), drain: false }), ..Default::default() }
}

pub(super) fn draining(action: Action, quiet_s: u64) -> State {
    let mut st = armed(action, quiet_s);
    st.armed.as_mut().unwrap().drain = true;
    st
}

/// Runs a rule, all quiet, to its firing: the countdown, its end, and the
/// fresh read that confirms it.
pub(super) fn fired(st: &mut State) -> Vec<Effect> {
    let quiet = board(0, Some(0));
    advance(st, false, &quiet, at(60_000));
    let end = at(60_000 + COUNTDOWN_MS);
    let due = advance(st, false, &quiet, end);
    if due != vec![Effect::Due] {
        return due;
    }
    vec![confirm(st, &quiet, at(0), end)]
}
