//! An account is a folder: `<root>/anthropic/<name>` for Claude Code (its
//! `CLAUDE_CONFIG_DIR`) and `<root>/openai/<name>` for Codex (its
//! `CODEX_HOME`). Creation is mkdir, deletion is rm -rf, the list is readdir —
//! no database of its own, so moving the root is moving folders.

use crate::provider::Provider;
use serde::Serialize;
use std::fs;
use std::path::PathBuf;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AccountInfo {
    pub provider: Provider,
    pub name: String,
    pub path: String,
}

/// The folder the user picked in settings, or `Documents/luna-accounts`.
pub fn accounts_root() -> Result<PathBuf, String> {
    if let Some(p) = crate::settings::get().accounts_root {
        return Ok(PathBuf::from(p));
    }
    let docs = dirs::document_dir().ok_or("Documents directory not found")?;
    Ok(docs.join("luna-accounts"))
}

fn provider_root(provider: Provider) -> Result<PathBuf, String> {
    Ok(accounts_root()?.join(provider.accounts_dir()))
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 64 {
        return Err("Account name must be 1..64 characters".into());
    }
    if name
        .chars()
        .any(|c| matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control())
    {
        return Err("Account name contains invalid characters".into());
    }
    if name == "." || name == ".." {
        return Err("Invalid account name".into());
    }
    Ok(())
}

#[tauri::command]
pub fn list_accounts() -> Result<Vec<AccountInfo>, String> {
    let mut out = vec![];
    for provider in Provider::ALL {
        let root = provider_root(provider)?;
        if !root.exists() {
            continue;
        }
        for entry in fs::read_dir(&root).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name().to_string_lossy().into_owned();
            // `<name>.lock` beside an account is Claude Code's token-refresh
            // lock (Luna takes it too) — there for a second, never an account.
            if name.ends_with(".lock") {
                continue;
            }
            if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
                out.push(AccountInfo {
                    provider,
                    name,
                    path: entry.path().to_string_lossy().into_owned(),
                });
            }
        }
    }
    out.sort_by(|a, b| {
        a.provider
            .index()
            .cmp(&b.provider.index())
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(out)
}

#[tauri::command]
pub fn create_account(provider: Provider, name: String) -> Result<AccountInfo, String> {
    validate_name(&name)?;
    let dir = provider_root(provider)?.join(&name);
    if dir.exists() {
        return Err(format!("Account \"{name}\" already exists"));
    }
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.to_string_lossy().into_owned();
    if provider == Provider::Codex {
        crate::codex::config::seed(&path)?;
    }
    Ok(AccountInfo { provider, name, path })
}

/// Renames the account's folder, which is all an account's name is. Not while
/// a session runs on it: the CLI holds files in the folder (Windows refuses the
/// rename), and a running session would lose its config under it. The names
/// Luna keeps elsewhere — agents' blocked list, the settings parent — follow.
#[tauri::command]
pub fn rename_account(
    state: crate::pty::PtyState<'_>,
    provider: Provider,
    name: String,
    to: String,
) -> Result<AccountInfo, String> {
    let to = to.trim().to_string();
    let root = provider_root(provider)?;
    let path = root.join(&name).to_string_lossy().into_owned();
    if state.account_in_use(&path) {
        return Err("Chats are running on this account — stop them first".into());
    }
    let dest = rename_in(&root, &name, &to)?;

    let (old_key, new_key) = (format!("{}/{name}", provider.as_str()), format!("{}/{to}", provider.as_str()));
    crate::settings::update(|s| {
        for k in s.agent_blocked_accounts.iter_mut().filter(|k| **k == old_key) {
            *k = new_key.clone();
        }
        let parent = match provider {
            Provider::Claude => &mut s.settings_parents.claude,
            Provider::Codex => &mut s.settings_parents.codex,
        };
        if parent.as_deref() == Some(name.as_str()) {
            *parent = Some(to.clone());
        }
    })?;
    crate::log::info("accounts", &format!("renamed {old_key} to {new_key}"));
    Ok(AccountInfo { provider, name: to, path: dest.to_string_lossy().into_owned() })
}

/// The folder part of a rename, under one provider's root.
fn rename_in(root: &std::path::Path, name: &str, to: &str) -> Result<PathBuf, String> {
    validate_name(name)?;
    validate_name(to)?;
    let from = root.join(name);
    let dest = root.join(to);
    if !from.is_dir() {
        return Err(format!("Account \"{name}\" is not there any more"));
    }
    // A change of case only is a rename too, and on Windows the "existing"
    // folder is the same one.
    if dest.exists() && !name.eq_ignore_ascii_case(to) {
        return Err(format!("Account \"{to}\" already exists"));
    }
    fs::rename(&from, &dest).map_err(|e| format!("Could not rename the folder: {e}"))?;
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rename_moves_the_folder_and_refuses_what_it_should() {
        let root = std::env::temp_dir().join(format!("luna-rename-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("work")).unwrap();
        fs::create_dir_all(root.join("home")).unwrap();
        fs::write(root.join("work").join("settings.json"), "{}").unwrap();

        assert!(rename_in(&root, "work", "home").unwrap_err().contains("already exists"));
        assert!(rename_in(&root, "work", "a/b").is_err());
        assert!(rename_in(&root, "gone", "x").unwrap_err().contains("not there"));

        let dest = rename_in(&root, "work", "office").unwrap();
        assert!(dest.join("settings.json").is_file());
        assert!(!root.join("work").exists());
        // Only the case changes: still a rename.
        rename_in(&root, "office", "Office").unwrap();
        assert!(root.join("Office").join("settings.json").is_file());
        let _ = fs::remove_dir_all(&root);
    }
}

#[tauri::command]
pub fn delete_account(provider: Provider, name: String) -> Result<(), String> {
    validate_name(&name)?;
    let dir = provider_root(provider)?.join(&name);
    if !dir.exists() {
        return Ok(());
    }
    fs::remove_dir_all(&dir).map_err(|e| e.to_string())
}
