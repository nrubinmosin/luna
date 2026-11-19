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

    /// A deletion bound to the folder the question named: one made again at
    /// its name between the question and the answer is refused and nothing
    /// goes; the folder named goes, with nothing past a link inside it and a
    /// read-only file in it.
    #[test]
    fn a_deletion_takes_only_the_folder_asked_about() {
        let base = std::env::temp_dir().join(format!("luna-accounts-id-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let (root, elsewhere) = (base.join("root"), base.join("elsewhere"));
        let provider = root.join(Provider::Claude.accounts_dir());
        let dir = provider.join("work");
        fs::create_dir_all(dir.join("plugins")).unwrap();
        fs::create_dir_all(&elsewhere).unwrap();
        fs::write(elsewhere.join("keep.json"), "{}").unwrap();
        link_dir(&dir.join("plugins").join("link"), &elsewhere);
        let ro = dir.join("plugins").join("pack.idx");
        fs::write(&ro, "x").unwrap();
        let mut perms = fs::metadata(&ro).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&ro, perms).unwrap();
        let id = identity_in(&root, &dir).unwrap();
        assert_eq!(identity_in(&root, &dir).unwrap(), id);
        // Kept by another folder while the name is made again, so the drive
        // cannot hand the same id out twice.
        fs::rename(&dir, base.join("aside")).unwrap();
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("new.json"), "{}").unwrap();
        assert_ne!(identity_in(&root, &dir).unwrap(), id);
        assert!(remove_account_dir(&dir, &root, Some(&id)).unwrap_err().contains("replaced"));
        assert!(dir.join("new.json").is_file());
        fs::remove_dir_all(&dir).unwrap();
        fs::rename(base.join("aside"), &dir).unwrap();
        remove_account_dir(&dir, &root, Some(&id)).unwrap();
        assert!(!dir.exists());
        assert!(elsewhere.join("keep.json").is_file());
        let _ = fs::remove_dir_all(&base);
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
        assert!(remove_account_dir(&provider.join("work"), &root, None).is_err());
        assert!(elsewhere.join("work").join("keep.json").is_file());
        let _ = fs::remove_dir(&provider).or_else(|_| fs::remove_file(&provider));
        // The account's folder a link out of the root.
        fs::create_dir_all(&provider).unwrap();
        link_dir(&provider.join("work"), &elsewhere.join("work"));
        assert!(remove_account_dir(&provider.join("work"), &root, None).is_err());
        assert!(elsewhere.join("work").join("keep.json").is_file());
        let _ = fs::remove_dir(provider.join("work")).or_else(|_| fs::remove_file(provider.join("work")));
        // A plain one goes; nothing there is nothing to do.
        fs::create_dir_all(provider.join("work")).unwrap();
        remove_account_dir(&provider.join("work"), &root, None).unwrap();
        assert!(!provider.join("work").exists());
        remove_account_dir(&provider.join("work"), &root, None).unwrap();
        let _ = fs::remove_dir_all(&base);
    }

    /// The accounts root moved (in Settings) between the question and the
    /// deletion, seen under the lock: nothing is deleted, in the root asked
    /// about or the one it is now.
    #[test]
    fn a_root_changed_under_the_lock_deletes_nothing() {
        let base = std::env::temp_dir().join(format!("luna-accounts-root-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let (root, moved) = (base.join("root"), base.join("moved"));
        let work = |r: &Path| r.join(Provider::Claude.accounts_dir()).join("work");
        for r in [&root, &moved] {
            fs::create_dir_all(work(r)).unwrap();
            fs::write(work(r).join("a.json"), "{}").unwrap();
        }
        let e = remove_under_root(moved.clone(), &root, &work(&root), None).unwrap_err();
        assert!(e.contains("changed in Settings"), "{e}");
        assert!(work(&root).join("a.json").is_file() && work(&moved).join("a.json").is_file());
        // Unchanged, it goes, and only the one in the root asked about.
        remove_under_root(root.clone(), &root, &work(&root), None).unwrap();
        assert!(!work(&root).exists() && work(&moved).join("a.json").is_file());
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
    delete_in(provider, &name, None)
}

/// An account's folder as the native window's Delete... question names it:
/// its path, held to `boundary::account_in`, and the folder's identity on
/// its drive (`held::Dir::id`, reached as the deletion reaches it), to which
/// the answer is bound (`delete_account_checked`). An addition beside
/// `delete_account` for the native window (native/panels.rs).
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AccountIdentity {
    pub path: String,
    pub id: String,
}

pub fn account_identity(provider: Provider, name: String) -> Result<AccountIdentity, String> {
    validate_name(&name)?;
    let root = accounts_root()?;
    let dir = root.join(provider.accounts_dir()).join(&name);
    crate::boundary::account_in(&dir.to_string_lossy(), &root)?;
    Ok(AccountIdentity { path: dir.to_string_lossy().into_owned(), id: identity_in(&root, &dir)? })
}

/// The identity of the account folder `dir` under `root`, reached through
/// handles from the root down as `remove_held` reaches it.
fn identity_in(root: &Path, dir: &Path) -> Result<String, String> {
    let (provider, name) = held_parent(root, dir)?;
    let provider = provider.ok_or_else(|| format!("{} is not there", dir.display()))?;
    let account = provider.child_dir(name, false).map_err(|e| format!("{}: {e}", dir.display()))?;
    account.ok_or_else(|| format!("{} is not there", dir.display()))?.id().map_err(|e| format!("{}: {e}", dir.display()))
}

/// Deletes the account only if its folder is still the one the question
/// named: the same path under the accounts root in use, and the same folder
/// on its drive (`id`), held to it through the handle its contents are
/// removed by (`remove_held`), under the library's lock. A folder gone
/// meanwhile is only cleaned up after, as `delete_account` does.
pub fn delete_account_checked(provider: Provider, name: String, path: String, id: String) -> Result<(), String> {
    delete_in(provider, &name, Some((&path, &id)))
}

fn delete_in(provider: Provider, name: &str, named: Option<(&str, &str)>) -> Result<(), String> {
    validate_name(name)?;
    let root = accounts_root()?;
    let dir = root.join(provider.accounts_dir()).join(name);
    if let Some((path, _)) = named {
        if Path::new(path) != dir {
            return Err(format!("The accounts folder is not where it was asked about ({path}); nothing was deleted."));
        }
    }
    // A folder gone already still has what Luna keeps of it cleaned up: a
    // deletion that failed after the folder went is finished by another.
    crate::library::delete_account_held(
        provider,
        &dir,
        name,
        // The root sampled above, looked at again under the lock the deletion
        // goes under: Settings may have moved it meanwhile.
        || remove_under_root(accounts_root()?, &root, &dir, named.map(|(_, id)| id)),
        |library| crate::settings::update(|s| library(&mut s.library)),
    )
}

/// `remove_account_dir`, where `now` (the accounts root as it is looked up
/// under the lock) is still the `root` the question was asked under; else
/// nothing is deleted.
fn remove_under_root(now: PathBuf, root: &Path, dir: &Path, id: Option<&str>) -> Result<(), String> {
    if now != root {
        return Err("The accounts folder was changed in Settings meanwhile; nothing was deleted.".into());
    }
    remove_account_dir(dir, root, id)
}

/// The provider's folder of the account folder `dir` under `root`, held:
/// the root opened as it leads (a link of the person's choosing), the
/// provider's folder in it as itself (a link, a junction or any other
/// reparse point there refused) and, on Unix, on the root's device. None
/// where it is not there; with the account's name.
fn held_parent<'a>(root: &Path, dir: &'a Path) -> Result<(Option<crate::held::Dir>, &'a str), String> {
    let refuse = || format!("{} is not an account folder of Luna's", dir.display());
    let name = dir.file_name().and_then(|n| n.to_str()).ok_or_else(refuse)?;
    let provider = dir.parent().ok_or_else(refuse)?;
    if provider.parent() != Some(root) {
        return Err(refuse());
    }
    let provider_name = provider.file_name().and_then(|n| n.to_str()).ok_or_else(refuse)?;
    let root_dir = match crate::held::Dir::open_followed(root) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((None, name)),
        Err(e) => return Err(format!("{}: {e}", root.display())),
    };
    let held = root_dir.child_dir(provider_name, false).map_err(|e| format!("{}: {e}", provider.display()))?;
    if let Some(held) = &held {
        if !root_dir.same_device(held).map_err(|e| format!("{}: {e}", provider.display()))? {
            return Err(format!("{} is another drive mounted in the accounts folder — Luna deletes nothing there", provider.display()));
        }
    }
    Ok((held, name))
}

/// An account's folder removed, checked first as an account folder of Luna's
/// (`boundary::account_in`): neither the provider's folder nor the account's
/// a link or a junction, and the folder inside the accounts root once both
/// are resolved. Then removed through handles (`held::Dir::remove_tree`):
/// the provider's folder and the account's opened as themselves (any
/// reparse point refused), the account's held to `id` where one is given
/// and on the provider's device, and emptied relative to its own handle, a
/// link inside removed as itself and a mount inside refused before anything
/// goes. Nothing at the name is nothing to remove.
fn remove_account_dir(dir: &Path, root: &Path, id: Option<&str>) -> Result<(), String> {
    if fs::symlink_metadata(dir).is_ok() {
        crate::boundary::account_in(&dir.to_string_lossy(), root)?;
    }
    let (provider, name) = held_parent(root, dir)?;
    let Some(provider) = provider else { return Ok(()) };
    provider.remove_tree(name, id).map(|_| ()).map_err(|e| format!("{}: {e}. Nothing more was deleted.", dir.display()))
}
