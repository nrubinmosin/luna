//! What the host's answers and events carry for the status bar, as the window
//! reads them: the shapes of `src/ipc/power.ts`, `cli.ts`, `accounts.ts` and
//! `sessions.ts`. Every field has a default, so a host that sends less (or
//! more) than the page's types name still reads.

use crate::term::paste::Provider;
use serde::{Deserialize, Serialize};

/// What a power-off does (`power::Action`).
#[derive(Deserialize, Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PowerAction {
    #[default]
    Shutdown,
    Restart,
    Hibernate,
    Sleep,
    /// Runs the whole rule and only writes a log line: never offered here.
    Log,
}

impl PowerAction {
    /// The word the host takes, and the chip says.
    pub fn name(self) -> &'static str {
        match self {
            PowerAction::Shutdown => "shutdown",
            PowerAction::Restart => "restart",
            PowerAction::Hibernate => "hibernate",
            PowerAction::Sleep => "sleep",
            PowerAction::Log => "log",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PowerAction::Shutdown => "Shut down",
            PowerAction::Restart => "Restart",
            PowerAction::Hibernate => "Hibernate",
            PowerAction::Sleep => "Sleep",
            PowerAction::Log => "Log",
        }
    }

    /// What the popover tells of each.
    pub fn note(self) -> &'static str {
        match self {
            PowerAction::Shutdown => "closes every session first, then powers off",
            PowerAction::Restart => "closes every session first, then restarts; Luna starts again when you sign in",
            PowerAction::Hibernate | PowerAction::Sleep => "sessions stay; the rule applies again after the wake",
            PowerAction::Log => "only writes a line to the log",
        }
    }
}

/// A rule that is armed (`power::Armed`).
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Armed {
    pub action: PowerAction,
    pub quiet_s: u64,
    pub armed_at_ms: u64,
    /// Agents start nothing new until it fires or is disarmed.
    pub drain: bool,
}

#[derive(Deserialize, Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Turn {
    Busy,
    Waiting,
    #[default]
    Idle,
}

/// Where a session's turn stands (`activity::Phase`).
#[derive(Deserialize, Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Running,
    Asking,
    Ended,
    /// Nothing the CLI says has been read yet (a session with no hooks and no
    /// transcript, say): active, as the core counts it.
    #[default]
    Unknown,
}

/// What a session runs beside its turns (`activity::Background`), as far as
/// the window needs: whether anything does.
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Background {
    pub shells: u32,
    pub crons: u32,
    pub subagents: u32,
    pub unlisted: bool,
}

/// The core's lifecycle record of a session (`activity::Life`), the one
/// source of what a session is doing.
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Life {
    pub phase: Phase,
    pub background: Background,
}

impl Life {
    /// The word a chat's status goes by, as `Life::status` in the core has
    /// it: `busy` (unknown included), `waiting`, `background`, `idle`.
    pub fn status(&self) -> &'static str {
        let background =
            self.background.shells > 0 || self.background.crons > 0 || self.background.subagents > 0 || self.background.unlisted;
        match self.phase {
            Phase::Running | Phase::Unknown => "busy",
            Phase::Asking => "waiting",
            Phase::Ended if background => "background",
            Phase::Ended => "idle",
        }
    }
}

/// One session as the power rule sees it (`activity::SessionLife`).
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionActivity {
    /// The core's record; None from a core that sends none.
    pub life: Option<Life>,
    /// Helpers under it still at work on an answer it is owed: with its own
    /// turn over, it is in the background on their account.
    pub helpers: u32,
    pub id: String,
    pub turn: Turn,
    /// Exe names of the children keeping it busy, when any.
    pub procs: Vec<String>,
    pub output_fresh: bool,
    pub busy: bool,
    /// Waiting only on what an API error left, a usage limit: counted as done.
    pub stalled: Option<String>,
}

impl SessionActivity {
    /// The word its chat's status goes by: its record's (`Life::status`),
    /// `background` for one whose turn is over while helpers under it are
    /// at work on an answer it is owed; None without a record.
    pub fn status(&self) -> Option<&'static str> {
        match self.life.as_ref()?.status() {
            "idle" if self.helpers > 0 => Some("background"),
            word => Some(word),
        }
    }

    /// What the session is doing, in the words the popover uses. Asking is
    /// active (ADR-0031), so it is among those holding the PC, and says so;
    /// a turn ended on an error with nothing running beside it says what it
    /// stopped on.
    pub fn doing(&self) -> String {
        if let (Some(why), false) = (&self.stalled, self.busy) {
            return format!("stopped on {why}");
        }
        let mut why = Vec::new();
        match self.turn {
            Turn::Busy => why.push("mid-turn".to_owned()),
            Turn::Waiting => why.push("waiting for you".to_owned()),
            Turn::Idle => {}
        }
        if !self.procs.is_empty() {
            why.push(format!("{} {}", if self.turn == Turn::Idle { "background:" } else { "running" }, self.procs.join(", ")));
        }
        if self.output_fresh && self.turn != Turn::Busy {
            why.push("printing".into());
        }
        if why.is_empty() { "idle".into() } else { why.join(", ") }
    }
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Summary {
    pub busy: usize,
    pub waiting: usize,
    pub stalled: usize,
    pub idle_since_ms: Option<u64>,
    pub sessions: Vec<SessionActivity>,
}

/// `power://state` and the answers of the power commands (`PowerStateDto`).
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct PowerState {
    pub keep_awake: bool,
    /// The awake request is in force right now.
    pub holding: bool,
    pub armed: Option<Armed>,
    pub countdown_ends_at_ms: Option<u64>,
    pub summary: Summary,
    /// The core's count of the states it sent, from 1 in each core run: an
    /// answer or an event older than the state shown is dropped. A core
    /// that sends none (0) is taken as it comes.
    pub revision: u64,
}

impl Default for PowerState {
    /// The page's `IDLE_POWER`: before the core says, keep-awake is on (it is
    /// the setting's default) and nothing is held or armed.
    fn default() -> PowerState {
        PowerState { keep_awake: true, holding: false, armed: None, countdown_ends_at_ms: None, summary: Summary::default(), revision: 0 }
    }
}

/// `cli://status` and `cli_status` (`CliStatusDto`), as far as the chips go.
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct CliStatus {
    pub provider: Option<Provider>,
    /// `idle`, `checking`, `downloading` or `error`.
    pub phase: String,
    /// The version Luna's own copy is at; none until the first download lands.
    pub version: Option<String>,
    pub latest: Option<String>,
    pub got: u64,
    pub total: Option<u64>,
    pub error: Option<String>,
    /// Why the last pass left the CLI where it was on purpose: with no
    /// version, why nothing was installed (Luna run as administrator, say).
    pub note: Option<String>,
}

/// One row of `account_limits`' `scoped` and `other`.
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ExtraLimit {
    pub kind: String,
    pub label: String,
    pub used: f64,
    pub resets_at: Option<String>,
}

/// `account_limits`' answer (`AccountLimitsDto`).
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ClaudeLimits {
    pub signed_in: bool,
    pub email: Option<String>,
    pub plan: Option<String>,
    pub have_usage: bool,
    pub h5: Option<f64>,
    pub week: Option<f64>,
    pub reset_h5: Option<String>,
    pub reset_week: Option<String>,
    pub scoped: Vec<ExtraLimit>,
    pub other: Vec<ExtraLimit>,
    pub fetched_at_ms: Option<f64>,
    pub stale: bool,
    pub refresh_error: Option<String>,
    pub rate_limited: Option<u64>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct CodexWindow {
    pub id: String,
    pub label: String,
    pub used: f64,
    pub resets_at: Option<String>,
}

/// `codex_limits`' answer (`CodexLimitsDto`).
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct CodexLimits {
    pub signed_in: bool,
    pub email: Option<String>,
    pub plan: Option<String>,
    pub have_usage: bool,
    pub windows: Vec<CodexWindow>,
    pub fetched_at_ms: Option<f64>,
    pub stale: bool,
    pub rate_limited: Option<u64>,
}

/// A session running that no chat reaches (`OrphanSessionDto`).
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Orphan {
    pub id: String,
    pub provider: Option<Provider>,
    pub pid: Option<u32>,
    pub cwd: String,
    pub account_path: String,
    pub title: Option<String>,
    pub status: Option<String>,
}
