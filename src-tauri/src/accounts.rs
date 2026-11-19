//! An account is a folder: `<root>/anthropic/<name>` for Claude Code (its
//! `CLAUDE_CONFIG_DIR`) and `<root>/openai/<name>` for Codex (its
//! `CODEX_HOME`). Creation is mkdir, deletion is rm -rf, the list is readdir —
//! no database of its own, so moving the root is moving folders.

use crate::provider::Provider;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AccountInfo {
    pub provider: Provider,
    pub name: String,
    pub path: String,
}

/// The folder the user picked in settings, or the default (`default_root`).
pub fn accounts_root() -> Result<PathBuf, String> {
    if let Some(p) = crate::settings::get().accounts_root {
        return Ok(PathBuf::from(p));
    }
    default_root(crate::paths::is_test(), &crate::paths::data_dir(), dirs::document_dir())
}

/// `Documents/luna-accounts`, or for the test build `accounts` in its own data
/// dir (`test-build/accounts`): a test build left on its default must never
/// reach the real accounts, whose credentials and quota its helpers would spend.
fn default_root(test: bool, data_dir: &Path, documents: Option<PathBuf>) -> Result<PathBuf, String> {
    if test {
        return Ok(data_dir.join("accounts"));
    }
    let docs = documents.ok_or("Documents directory not found")?;
    Ok(docs.join("luna-accounts"))
}

pub(crate) fn provider_root(provider: Provider) -> Result<PathBuf, String> {
    Ok(accounts_root()?.join(provider.accounts_dir()))
}

pub(crate) fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 64 {
        return Err("Account name must be 1..64 characters".into());
    }
    if name.chars().any(|c| matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control()) {
        return Err("Account name contains invalid characters".into());
    }
    if name == "." || name == ".." {
        return Err("Invalid account name".into());
    }
    Ok(())
}

/// The accounts, by provider and name. A provider's folder that is not there
/// is no accounts; one that cannot be read (denied, a share that dropped, a
/// root that is a file) is an error: an empty list would stop the page's and
/// the window's retries and leave every chat without an account to start on.
#[tauri::command(async)]
pub fn list_accounts() -> Result<Vec<AccountInfo>, String> {
    let mut out = vec![];
    for provider in Provider::ALL {
        out.extend(list_in(&provider_root(provider)?, provider)?);
    }
    out.sort_by(|a, b| a.provider.index().cmp(&b.provider.index()).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(out)
}

/// The account folders in `root`, none where it is not there.
fn list_in(root: &Path, provider: Provider) -> Result<Vec<AccountInfo>, String> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(format!("{}: {e}", root.display())),
    };
    let mut out = vec![];
    for entry in entries {
        let entry = entry.map_err(|e| format!("{}: {e}", root.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        // `<name>.lock` beside an account is Claude Code's token-refresh
        // lock (Luna takes it too) — there for a second, never an account.
        if name.ends_with(".lock") {
            continue;
        }
        if entry.file_type().map_err(|e| format!("{}: {e}", entry.path().display()))?.is_dir() {
            out.push(AccountInfo { provider, name, path: entry.path().to_string_lossy().into_owned() });
        }
    }
    Ok(out)
}

#[tauri::command(async)]
pub fn create_account(provider: Provider, name: String) -> Result<AccountInfo, String> {
    validate_name(&name)?;
    let dir = provider_root(provider)?.join(&name);
    if dir.exists() {
        return Err(format!("Account \"{name}\" already exists"));
    }
    // Nothing written into it: what Luna needs of a CLI on it (its own
    // updates kept off, say) goes with each start (codex/launch.rs
    // `NO_UPDATE_CHECK`).
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.to_string_lossy().into_owned();
    Ok(AccountInfo { provider, name, path })
}

/// Renames the account's folder, which is all an account's name is. Not while
/// a session runs on it: the CLI holds files in the folder (Windows refuses the
/// rename), and a running session would lose its config under it. The names
/// Luna keeps elsewhere — agents' blocked list, the settings parent, and all
/// the library knows of the account (`library::rename_account_held`) — follow.
#[tauri::command]
pub fn rename_account(provider: Provider, name: String, to: String) -> Result<AccountInfo, String> {
    let to = to.trim().to_string();
    let root = provider_root(provider)?;
    let from = root.join(&name);
    let in_use = || match crate::pty::manager().account_in_use(&from.to_string_lossy()) {
        true => Err("Chats are running on this account — stop them first".to_string()),
        false => Ok(()),
    };
    in_use()?;
    let (old_key, new_key) = (format!("{}/{name}", provider.as_str()), format!("{}/{to}", provider.as_str()));
    // Settings that cannot be written leave the chats on the old name: the
    // folder goes back under it rather than leave them pointing at nothing.
    let dest = crate::library::rename_account_held(
        provider,
        &from,
        &name,
        &to,
        // Asked again under the library's lock: a start that waited for it
        // counts its account from before (`library::lay_out_for`).
        || in_use().and_then(|()| rename_in(&root, &name, &to)),
        |library| {
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
                library(&mut s.library);
            })
        },
    )?;
    crate::log::info("accounts", &format!("renamed {old_key} to {new_key}"));
    Ok(AccountInfo { provider, name: to, path: dest.to_string_lossy().into_owned() })
}

/// The folder part of a rename, under one provider's root.
fn rename_in(root: &Path, name: &str, to: &str) -> Result<PathBuf, String> {
    validate_name(name)?;
    validate_name(to)?;
    let from = root.join(name);
    let dest = root.join(to);
    if !from.is_dir() {
        return Err(format!("Account \"{name}\" is not there any more"));
    }
    // A change of case only is a rename too, and where the file system ignores
    // case the "existing" folder is the same one; where it does not, it is
    // another account, which a rename onto would replace if it were empty.
    if dest.exists() && !crate::paths::same_dir(&from, &dest) {
        return Err(format!("Account \"{to}\" already exists"));
    }
    fs::rename(&from, &dest).map_err(|e| format!("Could not rename the folder: {e}"))?;
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_test_build_defaults_to_accounts_of_its_own() {
        let data = Path::new("/x/test-build");
        let docs = Some(PathBuf::from("/home/me/Documents"));
        assert_eq!(default_root(true, data, docs.clone()).unwrap(), data.join("accounts"));
        // Even with no Documents folder to fall back on.
        assert_eq!(default_root(true, data, None).unwrap(), data.join("accounts"));
        assert_eq!(default_root(false, data, docs).unwrap(), PathBuf::from("/home/me/Documents").join("luna-accounts"));
        assert!(default_root(false, data, None).is_err());
    }

    /// A root that is not there lists no accounts; one that is there and
    /// cannot be read is an error, so that the callers ask again instead of
    /// taking an empty list for the answer.
    #[test]
    fn a_root_that_cannot_be_read_is_an_error_not_an_empty_list() {
        let root = std::env::temp_dir().join(format!("luna-list-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        assert!(list_in(&root.join("anthropic"), Provider::Claude).unwrap().is_empty(), "not there: none");
        fs::create_dir_all(root.join("anthropic").join("work")).unwrap();
        fs::create_dir_all(root.join("anthropic").join("work.lock")).unwrap();
        fs::write(root.join("anthropic").join("note.txt"), "x").unwrap();
        let listed = list_in(&root.join("anthropic"), Provider::Claude).unwrap();
        assert_eq!(listed.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["work"]);
        // A file where the folder should be: there, and not readable as one.
        let err = list_in(&root.join("anthropic").join("note.txt"), Provider::Claude).err().unwrap();
        assert!(err.contains("note.txt"), "{err}");
        let _ = fs::remove_dir_all(&root);
    }

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

/// Off the main thread (`async`): it waits for the library's lock, which a
/// pass of the core's may hold.
#[tauri::command(async)]
pub fn delete_account(provider: Provider, name: String) -> Result<(), String> {
    validate_name(&name)?;
    let dir = provider_root(provider)?.join(&name);
    // A folder gone already still has what Luna keeps of it cleaned up: a
    // deletion that failed after the folder went is finished by another.
    crate::library::delete_account_held(
        provider,
        &dir,
        &name,
        || match dir.exists() {
            true => fs::remove_dir_all(&dir).map_err(|e| e.to_string()),
            false => Ok(()),
        },
        |library| crate::settings::update(|s| library(&mut s.library)),
    )
}
