//! What the host's answers and events carry, as the window reads them: the
//! shapes `src/ipc/*.ts` declares for the page. luna_lib's tests serialize its
//! own types and read them back as these, so the two cannot drift apart.

use serde::{Deserialize, Serialize};

/// The window's line to the core (corelink.rs `LinkState`; `CoreStateDto` in
/// src/ipc/coreProcess.ts).
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CoreState {
    pub state: LinkState,
    #[serde(default)]
    pub core: Option<Core>,
    #[serde(default)]
    pub error: Option<String>,
}

/// Where the line to the core stands.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum LinkState {
    /// Being found, started, or replaced: what asks for it waits.
    #[default]
    Starting,
    Up,
    /// It stopped, and its sessions with it.
    Down,
    /// One of another build runs the sessions.
    Mismatch,
    /// No core that is not run as administrator.
    Elevated,
}

/// Who the core is (ipc.rs `Welcome`; `CoreDto`).
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Core {
    pub pid: u32,
    pub version: String,
    pub build: String,
    pub fingerprint: String,
    /// Epoch ms.
    pub started: u64,
    #[serde(default)]
    pub sessions: usize,
    /// Why it starts no session: it runs as administrator.
    #[serde(default)]
    pub refused: Option<String>,
}

/// The theme the person chose (`ThemePref` in src/app/shell.store.ts).
#[derive(Deserialize, Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ThemePref {
    /// As the window's appearance: the system's, on every platform.
    #[default]
    System,
    Light,
    Dark,
}

impl ThemePref {
    /// The next one the theme button goes to, in the page's order.
    pub fn next(self) -> ThemePref {
        match self {
            ThemePref::System => ThemePref::Light,
            ThemePref::Light => ThemePref::Dark,
            ThemePref::Dark => ThemePref::System,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ThemePref::System => "system",
            ThemePref::Light => "light",
            ThemePref::Dark => "dark",
        }
    }
}
