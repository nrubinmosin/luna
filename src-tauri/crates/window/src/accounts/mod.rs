//! The accounts window: every account with its CLI and its usage limits;
//! add, rename and delete one; the parent account whose settings file the
//! others of its CLI copy. What the page's Accounts window does, less what
//! ADR-0030 cut (the recommendations, the config editor, what Luna applied).
//! Signing an account in is the CLI's login in the terminal pane, in a window
//! of its own over this one (login.rs).
//!
//! The limits are asked as the window opens and on Refresh, never on a
//! timer: their reset times are said as the clock time they fall on, which
//! stays true while the window is up.

mod login;
mod view;

pub use view::AccountsPanel;
#[cfg(test)]
pub(crate) use view::LimitsState;

use crate::panels::Provider;
use serde::Deserialize;

/// An account (`AccountInfo`).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Account {
    pub provider: Provider,
    pub name: String,
    pub path: String,
}

/// An account's folder as the Delete... question names it
/// (`account_identity`): its path, and its identity on the drive.
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
pub struct Identity {
    pub path: String,
    pub id: String,
}

/// One usage window: what it is, how full (0 to 1), when it resets.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UsageWindow {
    pub label: String,
    pub used: f32,
    pub resets_at: Option<String>,
}

/// An account's limits, either CLI's, as the window shows them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Limits {
    pub signed_in: bool,
    pub email: Option<String>,
    pub plan: Option<String>,
    pub have_usage: bool,
    pub windows: Vec<UsageWindow>,
    pub stale: bool,
    pub refresh_error: Option<String>,
    pub rate_limited: bool,
}

/// Claude Code's (`AccountLimitsDto`).
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct ClaudeLimits {
    signed_in: bool,
    email: Option<String>,
    plan: Option<String>,
    have_usage: bool,
    h5: Option<f32>,
    week: Option<f32>,
    reset_h5: Option<String>,
    reset_week: Option<String>,
    scoped: Vec<Row>,
    other: Vec<Row>,
    stale: bool,
    refresh_error: Option<String>,
    rate_limited: Option<u64>,
}

/// Codex's (`CodexLimitsDto`).
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct CodexLimits {
    signed_in: bool,
    email: Option<String>,
    plan: Option<String>,
    have_usage: bool,
    windows: Vec<Row>,
    stale: bool,
    rate_limited: Option<u64>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Row {
    label: String,
    used: f32,
    resets_at: Option<String>,
}

impl Limits {
    /// The answer to `account_limits` or `codex_limits`, by the account's CLI.
    pub fn read(provider: Provider, value: serde_json::Value) -> Result<Limits, String> {
        let bad = |e: serde_json::Error| format!("limits the window cannot read: {e}");
        Ok(match provider {
            Provider::Claude => {
                let l: ClaudeLimits = serde_json::from_value(value).map_err(bad)?;
                let mut windows = Vec::new();
                if let Some(used) = l.h5 {
                    windows.push(UsageWindow { label: "5 hours".into(), used, resets_at: l.reset_h5 });
                }
                if let Some(used) = l.week {
                    windows.push(UsageWindow { label: "week".into(), used, resets_at: l.reset_week });
                }
                windows.extend(l.scoped.into_iter().map(|r| UsageWindow {
                    label: format!("{} - week", r.label),
                    used: r.used,
                    resets_at: r.resets_at,
                }));
                windows.extend(l.other.into_iter().map(|r| UsageWindow { label: r.label, used: r.used, resets_at: r.resets_at }));
                Limits {
                    signed_in: l.signed_in,
                    email: l.email,
                    plan: l.plan,
                    have_usage: l.have_usage,
                    windows,
                    stale: l.stale,
                    refresh_error: l.refresh_error,
                    rate_limited: l.rate_limited.is_some(),
                }
            }
            Provider::Codex => {
                let l: CodexLimits = serde_json::from_value(value).map_err(bad)?;
                Limits {
                    signed_in: l.signed_in,
                    email: l.email,
                    plan: l.plan,
                    have_usage: l.have_usage,
                    windows: l.windows.into_iter().map(|r| UsageWindow { label: r.label, used: r.used, resets_at: r.resets_at }).collect(),
                    stale: l.stale,
                    refresh_error: None,
                    rate_limited: l.rate_limited.is_some(),
                }
            }
        })
    }

    /// Why there are no numbers to show, when there are none.
    pub fn usage_note(&self) -> Option<String> {
        if self.have_usage {
            return None;
        }
        Some(
            if self.stale {
                if self.refresh_error.is_some() { "token refresh failed" } else { "waiting for a token refresh" }
            } else if self.rate_limited {
                "usage rate-limited"
            } else {
                "no usage data yet"
            }
            .into(),
        )
    }

    /// The fullest window, 0 to 1.
    pub fn worst(&self) -> f32 {
        self.windows.iter().map(|w| w.used).fold(0., f32::max)
    }
}

/// Which account's settings file the others of its CLI copy
/// (`SyncStateDto`).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SyncState {
    pub claude: Option<String>,
    pub codex: Option<String>,
    pub on_start: bool,
}

impl SyncState {
    pub fn parent(&self, p: Provider) -> Option<&str> {
        match p {
            Provider::Claude => self.claude.as_deref(),
            Provider::Codex => self.codex.as_deref(),
        }
    }
}

/// What a sync did (`SyncReportDto`).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct SyncReport {
    pub synced: Vec<String>,
    pub failed: Vec<String>,
}

impl SyncReport {
    pub fn said(&self) -> String {
        let n = self.synced.len();
        if self.failed.is_empty() {
            format!("Copied to {n} account{}", if n == 1 { "" } else { "s" })
        } else {
            format!("Copied to {n}; failed: {}", self.failed.join("; "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn either_clis_limits_read_as_one_list_of_windows() {
        let claude = Limits::read(
            Provider::Claude,
            json!({ "signedIn": true, "plan": "Max", "haveUsage": true, "h5": 0.25, "week": 0.5, "resetH5": "2026-11-19T15:00:00Z",
                    "scoped": [{ "label": "Fable", "used": 0.9, "resetsAt": null }], "other": [] }),
        )
        .unwrap();
        let labels: Vec<&str> = claude.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, ["5 hours", "week", "Fable - week"]);
        assert_eq!(claude.worst(), 0.9);
        assert_eq!(claude.usage_note(), None);
        let codex =
            Limits::read(Provider::Codex, json!({ "signedIn": false, "haveUsage": false, "windows": [], "rateLimited": 30 })).unwrap();
        assert_eq!(codex.usage_note().as_deref(), Some("usage rate-limited"));
        assert!(Limits::read(Provider::Codex, json!({ "windows": "no" })).is_err());
    }

    #[test]
    fn a_sync_says_what_it_copied() {
        assert_eq!(SyncReport { synced: vec!["a".into()], failed: vec![] }.said(), "Copied to 1 account");
        assert_eq!(SyncReport { synced: vec![], failed: vec!["b: busy".into()] }.said(), "Copied to 0; failed: b: busy");
    }
}
