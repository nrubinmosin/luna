//! The settings window: the page's Settings dialog, less what ADR-0030 cut
//! (the CLIs' channels, pins, marks and known-good list, recommendations,
//! the waiting marker, the worktree fetch, Codex hooks without asking, the
//! leads' handoff). A list of pages on the left, the page on the right; each
//! row reads its setting from the host as the page opens and writes it as it
//! changes, answered with the setting as it then stands.
//!
//! Long work (a CLI's check, an update's download) says where it is in
//! words; nothing spins.

mod view;

pub use view::SettingsPanel;
#[cfg(test)]
pub(crate) use view::{cli_line, roll_back_asked};

use crate::panels::Provider;
use serde::Deserialize;

/// The pages, in the order the list has them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    General,
    Chats,
    Agents,
    Clis,
    Updates,
}

impl Page {
    pub const ALL: [Page; 5] = [Page::General, Page::Chats, Page::Agents, Page::Clis, Page::Updates];

    pub fn title(self) -> &'static str {
        match self {
            Page::General => "General",
            Page::Chats => "Chats",
            Page::Agents => "Agents & trust",
            Page::Clis => "CLIs",
            Page::Updates => "Updates",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Page::General => "general",
            Page::Chats => "chats",
            Page::Agents => "agents",
            Page::Clis => "clis",
            Page::Updates => "updates",
        }
    }
}

/// Where the accounts live (`AccountsRootInfo`).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AccountsRoot {
    pub path: String,
    pub is_default: bool,
}

/// How helpers are held (`HelperSettingsDto`, less the leads' handoff).
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HelperSettings {
    pub max_helpers: u32,
    pub min: u32,
    pub max: u32,
    /// 0: never.
    pub helper_context_pct: u32,
}

impl Default for HelperSettings {
    fn default() -> Self {
        HelperSettings { max_helpers: 8, min: 1, max: 32, helper_context_pct: 50 }
    }
}

/// The shares of a helper's context window its notices may say it is past.
pub const CONTEXT_PCTS: [u32; 8] = [0, 30, 40, 50, 60, 70, 80, 90];

/// A CLI's update pass as it stands (`CliStatusDto`).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CliStatus {
    pub provider: Option<Provider>,
    /// `idle`, `checking`, `downloading` or `error`.
    pub phase: String,
    /// The version Luna's own copy is at.
    pub version: Option<String>,
    pub path: String,
    /// The newest the CLI's release offers, once a check came back.
    pub latest: Option<String>,
    pub got: u64,
    pub total: Option<u64>,
    pub error: Option<String>,
    /// Why the last pass left the version where it was on purpose.
    pub note: Option<String>,
    pub checked_at_ms: Option<f64>,
}

impl CliStatus {
    /// A check or a download under way.
    pub fn passing(&self) -> bool {
        self.phase == "checking" || self.phase == "downloading"
    }

    /// The line under the version, in words.
    pub fn line(&self) -> String {
        match self.phase.as_str() {
            "checking" => "Checking for a new version...".into(),
            "downloading" => {
                let pct = self.total.filter(|t| *t > 0).map(|t| format!(": {}%", (self.got * 100 / t).min(100))).unwrap_or_default();
                format!("Downloading {}{pct}", self.latest.as_deref().unwrap_or("the new version"))
            }
            "error" => format!("The update failed: {}", self.error.as_deref().unwrap_or("unknown error")),
            _ if self.version.is_none() => "No working copy yet: sessions start once Luna has installed one.".into(),
            _ => match (&self.note, self.checked_at_ms) {
                (Some(n), _) => n.clone(),
                (None, Some(at)) => format!("Checked {}", crate::panels::local_time(at as i64)),
                (None, None) => "Not checked since Luna started.".into(),
            },
        }
    }
}

/// One installed version (`CliVersionRowDto`): what of it the window shows.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct VersionRow {
    pub dir: String,
    pub version: String,
    pub current: bool,
    pub previous: bool,
    pub in_use: bool,
    /// Older than the oldest this Luna runs: never gone back to.
    pub below_floor: bool,
    /// Turned away, and why: by the automatic rollback, as a rule.
    pub bad: Option<String>,
}

/// The watch over a new version's first sessions (`CliCanaryDto`).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Canary {
    pub version: String,
    pub watched: u32,
    pub settled: u32,
    pub failures: Vec<String>,
    pub warned: bool,
}

/// A CLI's installed versions (`CliVersionsDto`): what of them the window
/// shows.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CliVersions {
    /// Newest first.
    pub versions: Vec<VersionRow>,
    pub canary: Option<Canary>,
    /// The version Roll back goes to, when the record keeps one.
    pub way_back: Option<WayBack>,
}

/// The version `cli_roll_back` goes back to (`CliWayBackDto`).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WayBack {
    pub version: String,
    /// Why Roll back would be refused now, in the core's words; None where
    /// it is open.
    pub refusal: Option<String>,
}

/// What a manual rollback did (`RolledBack`).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct RolledBack {
    pub from: String,
    pub to: String,
}

impl CliVersions {
    pub fn current(&self) -> Option<&VersionRow> {
        self.versions.iter().find(|r| r.current)
    }

    /// What the Roll back row says: the button's label, the line beside it,
    /// and whether the button works: only where the record keeps a way back,
    /// nothing refuses it and a current version is shown to go back from.
    pub fn roll_back_offer(&self) -> (String, String, bool) {
        match &self.way_back {
            None => ("Roll back".into(), "No earlier version is kept to go back to.".into(), false),
            Some(b) => {
                let label = format!("Roll back to {}", b.version);
                match &b.refusal {
                    Some(why) => (label, format!("Roll back is refused: {why}."), false),
                    None => (label, format!("Goes back to {}, the version kept before this one.", b.version), self.current().is_some()),
                }
            }
        }
    }
}

/// One CLI on the CLIs page.
#[derive(Clone, Debug, Default)]
pub struct CliState {
    pub status: Option<CliStatus>,
    pub versions: Option<CliVersions>,
    /// Why the last Check failed.
    pub error: Option<String>,
    /// How the last Roll back went: what it did, or the core's words for why
    /// it did nothing.
    pub rolled: Option<Result<RolledBack, String>>,
    /// A Roll back is with the core.
    pub rolling: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_clis_status_says_where_its_pass_is() {
        let s = |v: serde_json::Value| serde_json::from_value::<CliStatus>(v).unwrap().line();
        assert_eq!(s(json!({ "phase": "checking", "version": "1.0.0" })), "Checking for a new version...");
        assert_eq!(s(json!({ "phase": "downloading", "latest": "1.1.0", "got": 50, "total": 200 })), "Downloading 1.1.0: 25%");
        assert_eq!(s(json!({ "phase": "error", "error": "offline" })), "The update failed: offline");
        assert!(s(json!({ "phase": "idle" })).starts_with("No working copy"));
        assert_eq!(s(json!({ "phase": "idle", "version": "1.0.0", "note": "held back" })), "held back");
        assert!(s(json!({ "phase": "idle", "version": "1.0.0", "checkedAtMs": 1_763_500_000_000u64 })).starts_with("Checked "));
    }
}
