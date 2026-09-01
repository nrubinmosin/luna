//! Where a new chat's model, effort and permission mode come from when nobody
//! has said otherwise: Claude Code's own settings files, read in the CLI's own
//! precedence order.
//!
//! Luna always spawns the CLI with `--model/--effort/--permission-mode`, so
//! whatever the settings say would otherwise be overridden on every session.
//! Reading them here is what makes those flags agree with the files instead of
//! fighting them — the new-chat dialog opens on these values, and only a
//! deliberate change in the dialog departs from them.

use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// One resolved value and the file it came from, so the dialog can say why it
/// is showing what it is showing.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Setting {
    pub value: String,
    /// `account` | `project` | `project-local` | `managed`.
    pub source: String,
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct Defaults {
    pub model: Option<Setting>,
    pub effort: Option<Setting>,
    pub permission_mode: Option<Setting>,
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
/// set are all the same answer: nothing said, so the caller keeps its own
/// default.
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

    let mut out = Defaults::default();
    for (source, path) in files {
        let Some(cfg) = read_object(&path) else { continue };
        if let Some(s) = take(source, cfg.get("model")) {
            out.model = Some(s);
        }
        if let Some(s) = take(source, cfg.get("effortLevel")) {
            out.effort = Some(s);
        }
        if let Some(s) = take(source, cfg.pointer("/permissions/defaultMode")) {
            out.permission_mode = Some(s);
        }
    }
    out
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
        let model = d.model.unwrap();
        assert_eq!(model.value, "sonnet");
        assert_eq!(model.source, "project");
        // Untouched by the project file, so the account still answers for them.
        assert_eq!(d.effort.unwrap().value, "xhigh");
        assert_eq!(d.permission_mode.unwrap().source, "account");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn no_account_and_no_folder_reads_neither() {
        // The machine-wide file is still consulted — it is the one scope that
        // does not depend on either argument — so this asserts on the source
        // rather than on emptiness, which would fail on a managed machine.
        let d = claude_defaults(String::new(), String::new());
        for s in [d.model, d.effort, d.permission_mode].into_iter().flatten() {
            assert_eq!(s.source, "managed");
        }
    }
}
