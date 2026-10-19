//! Where a new chat's model, effort and permission mode come from when nobody
//! has said otherwise: Claude Code's own settings files, read in the CLI's own
//! precedence order.
//!
//! Luna always spawns the CLI with `--model/--effort/--permission-mode`, so
//! whatever the settings say would otherwise be overridden on every session.
//! Reading them here is what makes those flags agree with the files instead of
//! fighting them — the new-chat dialog opens on these values, and only a
//! deliberate change in the dialog departs from them.
//!
//! The values come out in the CLI's own spelling and already settled: one
//! Luna has no equivalent for reads as nothing said, and nothing said is
//! Luna's stock choice (`source: stock`). The dialog, a spawn and
//! `luna_list` all take them from here, so what an agent is told it would
//! get is what it gets.

use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// One resolved value and the file it came from, so the dialog can say why it
/// is showing what it is showing.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Setting {
    pub value: String,
    /// `account` | `project` | `project-local` | `managed` | `stock`.
    pub source: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Defaults {
    pub model: Setting,
    pub effort: Setting,
    pub permission_mode: Setting,
}

/// What a chat starts with when no file says anything Luna can use.
const STOCK_MODEL: &str = "opus";
const STOCK_EFFORT: &str = "high";
const STOCK_PERMISSION_MODE: &str = "bypassPermissions";

/// The models Luna lists, as the CLI's aliases.
const MODELS: [&str; 4] = ["fable", "opus", "sonnet", "haiku"];
const EFFORTS: [&str; 6] = ["low", "medium", "high", "xhigh", "max", "ultracode"];
const PERMISSION_MODES: [&str; 4] = ["default", "acceptEdits", "plan", "bypassPermissions"];

/// `model` can be an alias (`opus`), a compound one (`opusplan`) or a full id
/// (`claude-sonnet-5`); all three name their model plainly enough to match on.
fn model_alias(raw: &str) -> Option<&'static str> {
    let v = raw.to_lowercase();
    MODELS.iter().find(|m| v.contains(**m)).copied()
}

fn effort_level(raw: &str) -> Option<&'static str> {
    let v = raw.to_lowercase();
    EFFORTS.iter().find(|e| **e == v).copied()
}

fn permission_mode(raw: &str) -> Option<&'static str> {
    PERMISSION_MODES.iter().find(|m| **m == raw).copied()
}

/// The value the files settled on, in the spelling Luna passes on, or the
/// stock one when they said nothing usable.
fn settle(said: Option<Setting>, usable: fn(&str) -> Option<&'static str>, stock: &str) -> Setting {
    said.and_then(|s| usable(&s.value).map(|v| Setting { value: v.to_owned(), source: s.source }))
        .unwrap_or_else(|| Setting { value: stock.to_owned(), source: "stock".to_owned() })
}

fn read_object(path: &Path) -> Option<Value> {
    let raw = std::fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&raw).ok()?;
    value.is_object().then_some(value)
}

/// The machine-wide file an administrator can put down. Highest precedence in
/// the CLI, so it is read last below.
#[cfg(windows)]
fn managed_file() -> PathBuf {
    let root = std::env::var("PROGRAMDATA").unwrap_or_else(|_| r"C:\ProgramData".into());
    PathBuf::from(root).join("ClaudeCode").join("managed-settings.json")
}

#[cfg(target_os = "macos")]
fn managed_file() -> PathBuf {
    PathBuf::from("/Library/Application Support/ClaudeCode/managed-settings.json")
}

#[cfg(all(unix, not(target_os = "macos")))]
fn managed_file() -> PathBuf {
    PathBuf::from("/etc/claude-code/managed-settings.json")
}

fn take(source: &str, raw: Option<&Value>) -> Option<Setting> {
    let value = raw?.as_str()?.trim();
    if value.is_empty() {
        return None;
    }
    Some(Setting {
        value: value.to_owned(),
        source: source.to_owned(),
    })
}

/// Model, effort and permission mode as the CLI would resolve them for this
/// account in this folder. A missing file, an unreadable one and a key nobody
/// set are all the same answer: nothing said, so the stock value stands.
#[tauri::command]
pub fn claude_defaults(account_path: String, folder: String) -> Defaults {
    // Lowest precedence first: every file that has something to say overwrites
    // what the one before it said. The account folder is the CLI's user scope
    // (it is the CLAUDE_CONFIG_DIR its sessions run with), then the project's
    // own two files, then the managed one.
    let mut files: Vec<(&str, PathBuf)> = Vec::new();
    if !account_path.is_empty() {
        files.push(("account", Path::new(&account_path).join("settings.json")));
    }
    if !folder.is_empty() {
        let dot = Path::new(&folder).join(".claude");
        files.push(("project", dot.join("settings.json")));
        files.push(("project-local", dot.join("settings.local.json")));
    }
    files.push(("managed", managed_file()));

    let (mut model, mut effort, mut mode) = (None, None, None);
    for (source, path) in files {
        let Some(cfg) = read_object(&path) else { continue };
        if let Some(s) = take(source, cfg.get("model")) {
            model = Some(s);
        }
        if let Some(s) = take(source, cfg.get("effortLevel")) {
            effort = Some(s);
        }
        if let Some(s) = take(source, cfg.pointer("/permissions/defaultMode")) {
            mode = Some(s);
        }
    }
    Defaults {
        model: settle(model, model_alias, STOCK_MODEL),
        effort: settle(effort, effort_level, STOCK_EFFORT),
        permission_mode: settle(mode, permission_mode, STOCK_PERMISSION_MODE),
    }
}

/// The same as the flags a helper would be spawned with, for `luna_list`.
pub fn flags(account_path: &str, folder: &str) -> Value {
    let d = claude_defaults(account_path.to_owned(), folder.to_owned());
    serde_json::json!({ "model": d.model.value, "effort": d.effort.value, "permissionMode": d.permission_mode.value })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn project_settings_win_over_the_account() {
        let root = std::env::temp_dir().join(format!("luna-defaults-{}", std::process::id()));
        let account = root.join("account");
        let folder = root.join("project");
        write(
            &account.join("settings.json"),
            r#"{ "model": "opus", "effortLevel": "xhigh",
                 "permissions": { "defaultMode": "bypassPermissions" } }"#,
        );
        write(
            &folder.join(".claude").join("settings.json"),
            r#"{ "model": "sonnet" }"#,
        );

        let d = claude_defaults(
            account.to_string_lossy().into_owned(),
            folder.to_string_lossy().into_owned(),
        );
        assert_eq!(d.model.value, "sonnet");
        assert_eq!(d.model.source, "project");
        // Untouched by the project file, so the account still answers for them.
        assert_eq!(d.effort.value, "xhigh");
        assert_eq!(d.permission_mode.source, "account");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn values_come_out_settled_or_stock() {
        let root = std::env::temp_dir().join(format!("luna-defaults-settled-{}", std::process::id()));
        let account = root.join("account");
        write(
            &account.join("settings.json"),
            r#"{ "model": "claude-Sonnet-5-5[1m]", "effortLevel": "turbo",
                 "permissions": { "defaultMode": "dontAsk" } }"#,
        );
        let d = claude_defaults(account.to_string_lossy().into_owned(), String::new());
        // A managed file on the machine may have its own say over any of them.
        if d.model.source != "managed" {
            assert_eq!((d.model.value.as_str(), d.model.source.as_str()), ("sonnet", "account"), "a full id names its alias");
        }
        // Neither is one Luna can put in a title bar: nothing was said.
        if d.effort.source != "managed" {
            assert_eq!((d.effort.value.as_str(), d.effort.source.as_str()), (STOCK_EFFORT, "stock"));
        }
        if d.permission_mode.source != "managed" {
            assert_eq!(d.permission_mode.value, STOCK_PERMISSION_MODE);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn no_account_and_no_folder_reads_neither() {
        // The machine-wide file is still consulted — it is the one scope that
        // does not depend on either argument — so this asserts on the source
        // rather than on emptiness, which would fail on a managed machine.
        let d = claude_defaults(String::new(), String::new());
        for s in [d.model, d.effort, d.permission_mode] {
            assert!(s.source == "managed" || s.source == "stock", "{}", s.source);
        }
    }
}
