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

#[tauri::command]
pub fn delete_account(provider: Provider, name: String) -> Result<(), String> {
    validate_name(&name)?;
    let dir = provider_root(provider)?.join(&name);
    if !dir.exists() {
        return Ok(());
    }
    fs::remove_dir_all(&dir).map_err(|e| e.to_string())
}
