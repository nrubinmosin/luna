//! What the machine does about the sessions: stays awake while they work,
//! and — when asked — shuts down, restarts, hibernates or sleeps once they are
//! all done.
//!
//! Both follow activity/. Keep-awake is a Windows power request
//! (`powercfg /requests` lists it as Luna's) held while any session is busy
//! and released a minute after the last one goes quiet. A session waiting for
//! a person does not hold the machine: nobody is answering it.
//!
//! Shutdown-when-done is armed for the run, not saved. Armed, it waits for a
//! quiet window in which nothing is busy *or waiting* — a pending permission
//! is unfinished work, while a session parked on a usage limit is not: its
//! turn is over and nothing moves before the reset (activity/, *stalled*);
//! arming with someone waiting says so in a notification — then counts down a minute with a notification, and
//! any activity in that minute cancels the count and leaves the arm in place.
//! Sleep and hibernate leave the sessions running and stay armed, so after a
//! wake the same rule applies again; shutdown first asks each CLI to quit, and
//! restart winds Luna down as a core leaving does and asks Windows to start the
//! window at the next sign-in, once.
//!
//! A rule may drain: until it fires, agents start nothing new, so it converges
//! rather than wait on work that keeps making work. Only the tools are refused
//! (`admit`, from mcp.rs); nothing a person does goes through there. A call let in
//! before a drain counts as at work until it has ended (`Entered`), and the arming
//! waits for it (`settle`). What a CLI starts by itself, a native subagent or a
//! `/loop`, Luna has no say in.
//!
//! The test build and unit tests never touch the machine: see `dry`.
//!
//! This module holds the state of the rule and what the window is shown of
//! it. The rest is split by what it does:
//!
//! - `rule`: each tick: the hold, the quiet window, the countdown, the firing.
//! - `machine`: what is done to the machine and said of it — notifications,
//!   the firing itself, each OS's calls, and the guard that keeps tests off it.
//! - `admission`: the agents' calls a draining rule refuses or waits for.
//! - `commands`: the window's commands and `luna_power`'s.

use crate::activity::Summary;
use crate::throttle::now_ms;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock};

mod admission;
mod commands;
mod machine;
mod rule;
#[cfg(test)]
mod testkit;

// Every file of the module reaches the others' items through `use super::*`,
// and the rest of the crate keeps its `crate::power::X` paths.
pub use admission::*;
pub use commands::*;
pub use machine::*;
pub use rule::*;
#[cfg(test)]
use testkit::*;

/// How long after the last busy sample the awake hold is let go.
const HOLD_LINGER_MS: u64 = 60_000;
/// The warning before the action fires.
const COUNTDOWN_MS: u64 = 60_000;
/// While armed and held up, the log says by whom: at once when the reason
/// changes (but not more often than this), and at least this often regardless,
/// so a night that ends with the PC still on can be read back.
const BLOCKED_LOG_MIN_MS: u64 = 60_000;
const BLOCKED_LOG_EVERY_MS: u64 = 10 * 60_000;
/// While the awake hold stands, the log says this often what keeps it: a hold
/// set in the evening and still on in the morning said nothing more, and
/// nobody could tell why the PC had stayed up.
const HOLD_LOG_EVERY_MS: u64 = 60 * 60_000;
/// How long a drain being armed waits for calls already let in: past a
/// `luna_spawn`'s own timeout. Longer, the rule still does not fire ahead of them.
const SETTLE_WAIT: std::time::Duration = std::time::Duration::from_secs(120);
/// RunOnce takes a command of no more than this many characters.
const RUN_ONCE_MAX: usize = 260;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Shutdown,
    Restart,
    Hibernate,
    Sleep,
    /// Runs the whole automaton and only writes a log line at the end.
    Log,
}

impl Action {
    pub fn parse(s: &str) -> Option<Action> {
        match s {
            "shutdown" => Some(Action::Shutdown),
            "restart" => Some(Action::Restart),
            "hibernate" => Some(Action::Hibernate),
            "sleep" => Some(Action::Sleep),
            "log" => Some(Action::Log),
            _ => None,
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Action::Shutdown => "shutdown",
            Action::Restart => "restart",
            Action::Hibernate => "hibernate",
            Action::Sleep => "sleep",
            Action::Log => "log",
        }
    }
}

#[derive(Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Armed {
    pub action: Action,
    pub quiet_s: u64,
    pub armed_at_ms: u64,
    /// Agents start nothing new until it fires or is disarmed.
    pub drain: bool,
}

/// What the UI draws: the chip, its tooltip, and the menu's current values.
#[derive(Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PowerState {
    pub keep_awake: bool,
    /// The awake request is in force right now.
    pub holding: bool,
    pub armed: Option<Armed>,
    pub countdown_ends_at_ms: Option<u64>,
    pub summary: Summary,
}

#[derive(Default)]
struct State {
    armed: Option<Armed>,
    countdown_ends_at_ms: Option<u64>,
    holding: bool,
    /// When the hold may be released, once nothing is busy.
    release_at_ms: Option<u64>,
    /// Who held the machine at the last busy sample, for the line that says
    /// the hold was let go: Windows sleeps at that moment when nobody has
    /// touched the PC for longer than its sleep timeout.
    held_by: String,
    /// When the hold was set, and when the log last said what keeps it.
    held_since_ms: u64,
    hold_logged_ms: u64,
    /// The last state sent to the UI, to send only changes.
    last_sent: Option<PowerState>,
    firing: bool,
    /// What is firing: agents are refused until that is over, drain or not, as
    /// what they start now the action would only take down with it.
    fired: Option<Action>,
    /// Agents' calls under way that may start work (`Entered`).
    admitted: usize,
    /// The last "armed, not firing" line and when it went out.
    blocked_why: String,
    blocked_logged_ms: u64,
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(Default::default)
}

fn snapshot(st: &State, summary: &Summary) -> PowerState {
    PowerState {
        keep_awake: crate::settings::get().keep_awake,
        holding: st.holding,
        armed: st.armed.clone(),
        countdown_ends_at_ms: st.countdown_ends_at_ms,
        summary: summary.clone(),
    }
}

pub fn current() -> PowerState {
    let st = state().lock().unwrap_or_else(|e| e.into_inner());
    snapshot(&st, &crate::activity::summary())
}

fn publish(st: &mut State, summary: &Summary) {
    let now = snapshot(st, summary);
    if st.last_sent.as_ref() != Some(&now) {
        crate::emit::to_ui("power://state", now.clone());
        st.last_sent = Some(now);
    }
}
