//! The few settings the Rust side has to know on its own — today just where
//! the account folders live. They sit in `<data>/settings.json` rather than
//! the webview's localStorage because background threads (models.rs) need
//! them before, and without, any window asking.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Absolute path; None means the default (`Documents/luna-accounts`).
    pub accounts_root: Option<String>,
    /// Hold the machine awake while any session is busy (power.rs).
    pub keep_awake: bool,
    /// How long everything has to be quiet before an armed power-off counts down.
    pub quiet_window_s: u64,
    /// The last action armed, so the menu opens on it: shutdown | hibernate | sleep.
    pub power_action: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            accounts_root: None,
            keep_awake: true,
            quiet_window_s: 120,
            power_action: "shutdown".into(),
        }
    }
}

fn file() -> PathBuf {
    crate::paths::data_dir().join("settings.json")
}

fn cache() -> &'static Mutex<Settings> {
    static S: OnceLock<Mutex<Settings>> = OnceLock::new();
    S.get_or_init(|| {
        let loaded = std::fs::read_to_string(file())
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        Mutex::new(loaded)
    })
}

pub fn get() -> Settings {
    cache().lock().map(|s| s.clone()).unwrap_or_default()
}

fn save(s: &Settings) -> Result<(), String> {
    let path = file();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let raw = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
    std::fs::write(&path, raw).map_err(|e| format!("write {}: {e}", path.display()))?;
    if let Ok(mut c) = cache().lock() {
        *c = s.clone();
    }
    Ok(())
}

/// Changes one or more fields and writes the file.
pub fn update(f: impl FnOnce(&mut Settings)) -> Result<(), String> {
    let mut s = get();
    f(&mut s);
    save(&s)
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AccountsRootInfo {
    pub path: String,
    /// True when no override is set and `path` is the default.
    pub is_default: bool,
}

fn info() -> Result<AccountsRootInfo, String> {
    let s = get();
    Ok(AccountsRootInfo {
        path: crate::accounts::accounts_root()?.to_string_lossy().into_owned(),
        is_default: s.accounts_root.is_none(),
    })
}

#[tauri::command]
pub fn get_accounts_root() -> Result<AccountsRootInfo, String> {
    info()
}

/// Points the account list at another directory — created if missing. Nothing
/// is moved: the accounts that were under the old root stay there, and the
/// list shows whatever folders the new one holds. Empty string = back to the
/// default.
#[tauri::command]
pub fn set_accounts_root(path: String) -> Result<AccountsRootInfo, String> {
    let trimmed = path.trim();
    let mut s = get();
    if trimmed.is_empty() {
        s.accounts_root = None;
    } else {
        let p = PathBuf::from(trimmed);
        if !p.is_absolute() {
            return Err("Accounts folder must be an absolute path".into());
        }
        std::fs::create_dir_all(&p).map_err(|e| format!("cannot create {}: {e}", p.display()))?;
        s.accounts_root = Some(p.to_string_lossy().into_owned());
    }
    save(&s)?;
    crate::log::info("settings", &format!("accounts root: {}", s.accounts_root.as_deref().unwrap_or("<default>")));
    info()
}
