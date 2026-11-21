//! The power chip's words, from `power://state` and the chats' names: what
//! the chip says, who holds the PC awake, who the armed rule waits on, and
//! the tray's awake tooltip. Pure: the model (vitals.rs) asks, the bar draws.

use super::dto::{PowerState, SessionActivity, Turn};

/// What the keep-awake switch holds the PC for, as the core counts a session
/// active (activity/record.rs `Life::active`, ADR-0031): a turn held on a
/// question is not over, and nothing is measured of what a terminal prints.
pub const KEEP_AWAKE_NOTE: &str = "Held while any session is active: mid-turn, waiting for your answer, running anything it started, or in a state Luna cannot tell. Let go a minute after the last one goes quiet.";

/// What the chip is, which decides its fill and its click.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The rule fired its warning: counting down, with a Cancel.
    Countdown,
    /// Armed, waiting for every session to be done.
    Armed,
    /// The PC is held awake by a session.
    Awake,
    /// Nothing held, nothing armed.
    Idle,
    /// The core is not reachable: what the rule does is not known, and no
    /// countdown is shown, as one may or may not be running there.
    Unknown,
}

/// The chip: its kind and its words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chip {
    pub kind: Kind,
    pub text: String,
}

/// Seconds left of a countdown ending at `ends_ms`, rounded up; 0 once past.
pub fn left_s(ends_ms: u64, now_ms: u64) -> u64 {
    ends_ms.saturating_sub(now_ms).div_ceil(1000)
}

pub fn chip(state: &PowerState, now_ms: u64) -> Chip {
    if let Some(ends) = state.countdown_ends_at_ms {
        let verb = state.armed.as_ref().map_or("shutdown", |a| a.action.name());
        return Chip { kind: Kind::Countdown, text: format!("{verb} in {}s", left_s(ends, now_ms)) };
    }
    if let Some(armed) = &state.armed {
        // Someone waiting for an answer holds the rule indefinitely: said on
        // the chip itself, not only in its popover.
        let mut text = format!("{} when done", armed.action.name());
        if armed.drain {
            text += " · draining";
        }
        if state.summary.waiting > 0 {
            text += &format!(" · {} waiting for you", state.summary.waiting);
        }
        return Chip { kind: Kind::Armed, text };
    }
    if state.holding {
        let busy = state.summary.sessions.iter().filter(|s| s.busy).count();
        return Chip { kind: Kind::Awake, text: if busy > 0 { format!("awake · {busy}") } else { "awake".into() } };
    }
    Chip { kind: Kind::Idle, text: "Power".into() }
}

/// The sessions keeping the PC awake, by name and what they do.
pub fn holders(state: &PowerState, name: impl Fn(&str) -> String) -> Vec<String> {
    state.summary.sessions.iter().filter(|s| s.busy).map(|s| format!("{} — {}", name(&s.id), s.doing())).collect()
}

/// The sessions an armed rule waits on: mid-turn, running something, or
/// waiting for a person. One parked on a usage limit is done as far as power
/// goes.
pub fn blockers(state: &PowerState, name: impl Fn(&str) -> String) -> Vec<String> {
    let blocks = |s: &&SessionActivity| s.busy || (s.turn == Turn::Waiting && s.stalled.is_none());
    state.summary.sessions.iter().filter(blocks).map(|s| format!("{} — {}", name(&s.id), s.doing())).collect()
}

/// The sessions counted as done though their turn did not finish: stopped on
/// a limit or another error, its turn over, nothing running beside it.
pub fn stalled(state: &PowerState, name: impl Fn(&str) -> String) -> Vec<String> {
    state
        .summary
        .sessions
        .iter()
        .filter(|s| !s.busy)
        .filter_map(|s| s.stalled.as_ref().map(|why| format!("{} — stopped on {why}", name(&s.id))))
        .collect()
}

/// What the tray's awake dot says (`tray_awake`'s `holders`): who holds the
/// PC, or a word for a hold that just lingers; None while nothing holds it.
pub fn tray_holders(state: &PowerState, name: impl Fn(&str) -> String) -> Option<String> {
    if !state.holding {
        return None;
    }
    let held = holders(state, name).join("\n");
    Some(if held.is_empty() { "a session that just went quiet".into() } else { held })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::dto::{Armed, PowerAction, Summary};

    fn session(id: &str, turn: Turn, busy: bool) -> SessionActivity {
        SessionActivity { id: id.into(), turn, busy, ..Default::default() }
    }

    fn name(id: &str) -> String {
        format!("chat {id}")
    }

    #[test]
    fn the_chip_says_what_the_rule_is_doing() {
        let mut s = PowerState::default();
        assert_eq!(chip(&s, 0), Chip { kind: Kind::Idle, text: "Power".into() });
        s.holding = true;
        s.summary = Summary { sessions: vec![session("a", Turn::Busy, true), session("b", Turn::Idle, false)], ..Default::default() };
        assert_eq!(chip(&s, 0).text, "awake · 1");
        s.armed = Some(Armed { action: PowerAction::Sleep, quiet_s: 120, drain: true, ..Default::default() });
        s.summary.waiting = 2;
        assert_eq!(chip(&s, 0), Chip { kind: Kind::Armed, text: "sleep when done · draining · 2 waiting for you".into() });
        s.countdown_ends_at_ms = Some(61_500);
        assert_eq!(chip(&s, 1_000), Chip { kind: Kind::Countdown, text: "sleep in 61s".into() });
        // Rounded up, and never below zero.
        assert_eq!(left_s(10_001, 10_000), 1);
        assert_eq!(left_s(10_000, 10_000), 0);
        assert_eq!(left_s(5_000, 10_000), 0);
    }

    /// The switch's note says what the core holds the PC for: a session
    /// waiting for an answer holds it, and output is no part of it.
    #[test]
    fn the_keep_awake_note_counts_what_the_core_counts() {
        assert!(KEEP_AWAKE_NOTE.contains("waiting for your answer"), "{KEEP_AWAKE_NOTE}");
        assert!(KEEP_AWAKE_NOTE.contains("cannot tell"), "not known counts as active: {KEEP_AWAKE_NOTE}");
        assert!(!KEEP_AWAKE_NOTE.contains("does not hold") && !KEEP_AWAKE_NOTE.contains("printing"), "{KEEP_AWAKE_NOTE}");
    }

    /// As the core sends them (activity::SessionLife): asking is active
    /// (ADR-0031), a turn stopped on a limit is over and not active.
    #[test]
    fn who_holds_the_pc_and_who_the_rule_waits_on() {
        let mut stopped = session("c", Turn::Idle, false);
        stopped.stalled = Some("a usage limit".into());
        let mut running = session("a", Turn::Idle, true);
        running.procs = vec!["cargo.exe".into()];
        let s = PowerState {
            holding: true,
            summary: Summary { sessions: vec![running, session("b", Turn::Waiting, true), stopped], ..Default::default() },
            ..Default::default()
        };
        assert_eq!(holders(&s, name), ["chat a — background: cargo.exe", "chat b — waiting for you"]);
        assert_eq!(blockers(&s, name), ["chat a — background: cargo.exe", "chat b — waiting for you"]);
        assert_eq!(stalled(&s, name), ["chat c — stopped on a usage limit"]);
        assert_eq!(tray_holders(&s, name).as_deref(), Some("chat a — background: cargo.exe\nchat b — waiting for you"));
        // Asking beside work it started says both.
        let mut both = session("d", Turn::Waiting, true);
        both.procs = vec!["npm.exe".into()];
        assert_eq!(both.doing(), "waiting for you, running npm.exe");
        // A hold with no busy session still says so; none says nothing.
        let lingering = PowerState { holding: true, ..Default::default() };
        assert_eq!(tray_holders(&lingering, name).as_deref(), Some("a session that just went quiet"));
        assert_eq!(tray_holders(&PowerState::default(), name), None);
    }
}
