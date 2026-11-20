//! The status bar and the orphan line of the native window: the page's
//! `StatusBar`, `PowerChip`, update and CLI chips and `OrphanSessions`, on
//! GPUI. The model (vitals.rs) holds what they show and does what they ask
//! of the host; the views (bar.rs, orphan.rs) draw it, static and in the
//! one font.
//!
//! - **Counts**: working, waiting (a click goes to the next waiting chat,
//!   as Ctrl+Shift+W does), background (a click opens the helpers' popover
//!   through `Seams::helpers`).
//! - **Accounts** with a meter per usage window, polled on the page's own
//!   cadence (usage.rs).
//! - **The Luna update** (updates.rs): a chip from `update_check` and
//!   `update://progress`; its click opens the confirmation, the one every
//!   install of Luna's goes through (the Settings' Update... opens it too);
//!   an install refused with `unsaved: ` offers Retry, Install anyway and
//!   Cancel.
//! - **The CLIs**: a chip while one downloads or failed to, from
//!   `cli://status`.
//! - **Power** (power.rs): the chip and its panel, from `power://state` and
//!   the power commands, and the tray's awake dot (`tray_awake`).
//! - **Settings**: a button that calls `Seams::settings`.
//! - **The orphan line**: shown only while a session runs with no chat.

mod bar;
#[cfg(any(test, feature = "shots"))]
pub(crate) mod demo;
pub mod dto;
mod orphan;
mod power;
mod updates;
mod usage;
mod vitals;

#[cfg(test)]
mod tests;

pub use bar::{HEIGHT, StatusBar};
pub use orphan::OrphanLine;
pub use updates::{Update, percent};
pub use vitals::{Arm, Popover, SCAN_TIMEOUT, Vitals};

/// Something done to the model by a demo scene (demo.rs) once the window is up.
#[cfg(any(test, feature = "shots"))]
pub type Act = fn(&mut Vitals, &mut gpui::Context<Vitals>);

/// The commands the status bar names, all called: the host answers each
/// (a test in luna_lib holds its dispatch to this list, with
/// `desktop::cmd::CALLED`).
pub mod cmd {
    /// `{}`: `PowerStateDto`.
    pub const POWER_STATE: &str = "power_state";
    /// `{ on }`: the new `PowerStateDto`.
    pub const SET_KEEP_AWAKE: &str = "set_keep_awake";
    /// `{ action, quietS, drain }`: the new `PowerStateDto`.
    pub const ARM_POWER_OFF: &str = "arm_power_off";
    pub const DISARM_POWER_OFF: &str = "disarm_power_off";
    /// `{ accountPath, force }`: that account's usage (`AccountLimitsDto`
    /// for a Claude account, `CodexLimitsDto` for a Codex one), or null.
    pub const ACCOUNT_LIMITS: &str = "account_limits";
    pub const CODEX_LIMITS: &str = "codex_limits";
    /// `{ usage }`: every account's usage for the agents' account list.
    pub const ACCOUNT_USAGE: &str = "account_usage";
    /// `{ provider }`: `CliStatusDto`.
    pub const CLI_STATUS: &str = "cli_status";
    /// `{ provider }`: checks now and installs what is newer.
    pub const CLI_UPDATE_NOW: &str = "cli_update_now";
    /// `{ known }`, the ids of every chat: the sessions no chat has.
    pub const ORPHAN_SESSIONS: &str = "orphan_sessions";
    /// `{ id }`.
    pub const KILL_SESSION: &str = "kill_session";
    /// `{}`: every session asked to quit, before an install that replaces
    /// the core.
    pub const STOP_ALL_SESSIONS: &str = "stop_all_sessions";
    /// `{}`: the install is off, the chats' ties thaw.
    pub const UPDATE_CALLED_OFF: &str = "update_called_off";

    pub const CALLED: &[&str] = &[
        POWER_STATE,
        SET_KEEP_AWAKE,
        ARM_POWER_OFF,
        DISARM_POWER_OFF,
        ACCOUNT_LIMITS,
        CODEX_LIMITS,
        ACCOUNT_USAGE,
        CLI_STATUS,
        CLI_UPDATE_NOW,
        ORPHAN_SESSIONS,
        KILL_SESSION,
        STOP_ALL_SESSIONS,
        UPDATE_CALLED_OFF,
    ];
}

/// The events the status bar listens to, beside `event::ALL`.
pub mod event {
    /// A `PowerStateDto`, whenever it changes; only the latest counts.
    pub const POWER_STATE: &str = "power://state";
    /// A `CliStatusDto` of one provider, at each step of its update pass.
    pub const CLI_STATUS: &str = "cli://status";

    pub const ALL: &[&str] = &[POWER_STATE, CLI_STATUS];
    /// Of those, the ones where only the latest counts.
    pub const LATEST: &[&str] = &[POWER_STATE];

    /// The key a core's event is stamped with where it is read off the line
    /// (`stamp`): the link revision of the `up` that began the connection it
    /// came on. The power rule's states are numbered from 1 in each core run,
    /// so a state of a core since gone, still waiting in the inbox, cannot be
    /// told from a new core's by its own revision alone.
    pub const CORE: &str = "core";

    /// Stamps a core's event with the link revision of its connection. Only
    /// the events a core's numbering matters to (`LATEST`) are stamped.
    pub fn stamp(name: &str, payload: &mut serde_json::Value, up: u64) {
        if LATEST.contains(&name)
            && let Some(object) = payload.as_object_mut()
        {
            object.insert(CORE.into(), up.into());
        }
    }

    /// The core an event was stamped with; None for one that was not.
    pub fn core_of(payload: &serde_json::Value) -> Option<u64> {
        payload.get(CORE).and_then(serde_json::Value::as_u64)
    }
}
