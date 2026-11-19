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

#[tauri::command(async)]
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
                out.push(AccountInfo { provider, name, path: entry.path().to_string_lossy().into_owned() });
            }
        }
    }
    out.sort_by(|a, b| a.provider.index().cmp(&b.provider.index()).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
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

    /// A junction on Windows (no privilege needed), a symlink elsewhere.
    fn link_dir(link: &Path, target: &Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).unwrap();
        #[cfg(windows)]
        assert!(std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .is_ok_and(|o| o.status.success()));
    }

    /// The provider's folder or the account's a link to a folder elsewhere:
    /// the deletion is refused and nothing there goes.
    #[test]
    fn a_deletion_goes_through_no_link() {
        let base = std::env::temp_dir().join(format!("luna-accounts-delete-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let (root, elsewhere) = (base.join("root"), base.join("elsewhere"));
        fs::create_dir_all(elsewhere.join("work")).unwrap();
        fs::write(elsewhere.join("work").join("keep.json"), "{}").unwrap();
        fs::create_dir_all(&root).unwrap();
        // The provider's folder a junction to another tree.
        let provider = root.join(Provider::Claude.accounts_dir());
        link_dir(&provider, &elsewhere);
        assert!(remove_account_dir(&provider.join("work"), &root).is_err());
        assert!(elsewhere.join("work").join("keep.json").is_file());
        let _ = fs::remove_dir(&provider).or_else(|_| fs::remove_file(&provider));
        // The account's folder a link out of the root.
        fs::create_dir_all(&provider).unwrap();
        link_dir(&provider.join("work"), &elsewhere.join("work"));
        assert!(remove_account_dir(&provider.join("work"), &root).is_err());
        assert!(elsewhere.join("work").join("keep.json").is_file());
        let _ = fs::remove_dir(provider.join("work")).or_else(|_| fs::remove_file(provider.join("work")));
        // A plain one goes; nothing there is nothing to do.
        fs::create_dir_all(provider.join("work")).unwrap();
        remove_account_dir(&provider.join("work"), &root).unwrap();
        assert!(!provider.join("work").exists());
        remove_account_dir(&provider.join("work"), &root).unwrap();
        let _ = fs::remove_dir_all(&base);
    }

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
    let root = accounts_root()?;
    let dir = root.join(provider.accounts_dir()).join(&name);
    // A folder gone already still has what Luna keeps of it cleaned up: a
    // deletion that failed after the folder went is finished by another.
    crate::library::delete_account_held(
        provider,
        &dir,
        &name,
        || remove_account_dir(&dir, &root),
        |library| crate::settings::update(|s| library(&mut s.library)),
    )
}

/// An account's folder removed, checked first as an account folder of Luna's
/// (`boundary::account_in`): neither the provider's folder nor the account's
/// a link or a junction, and the folder inside the accounts root once both
/// are resolved, so a provider folder planted as a junction leads no rm -rf
/// elsewhere. Nothing at the name (a link to nothing included, which
/// `symlink_metadata` still sees) is nothing to remove.
fn remove_account_dir(dir: &Path, root: &Path) -> Result<(), String> {
    match fs::symlink_metadata(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // The provider's folder may be the link that makes the name
            // empty: it is looked at too, as the account would be.
            let provider = dir.parent().unwrap_or(dir);
            match fs::symlink_metadata(provider) {
                Ok(m) if crate::boundary::is_link(&m) => Err(format!("{} is a link — Luna leaves it alone", provider.display())),
                _ => Ok(()),
            }
        }
        Err(e) => Err(format!("{}: {e}", dir.display())),
        Ok(_) => {
            crate::boundary::account_in(&dir.to_string_lossy(), root)?;
            fs::remove_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))
        }
    }
}
