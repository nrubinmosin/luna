//! An account's own settings file — Claude Code's `settings.json`, Codex's
//! `config.toml` — as Luna can help with it: settings recommended for anyone
//! running the CLI inside Luna, added or taken out key by key, the file
//! itself checked and formatted for editing by hand, and a "parent" account
//! per provider whose file the others get an exact copy of.
//!
//! Recommendations keep what they do not touch: other keys, their order, and
//! in the TOML file the user's comments. Credentials, logins and Codex's
//! trusted projects never travel.
//!
//! This module holds the lock every change of those files goes under. The
//! rest is split by what it does:
//!
//! - `specs`: the settings recommended, and what an account's files say of
//!   each.
//! - `files`: the files read and written, and keys found, set and removed in
//!   them, JSON and TOML alike.
//! - `edit`: changes key by key, to one account or several together, and the
//!   recommendations taken or taken out.
//! - `editor`: the file as text, checked and laid out for editing by hand.
//! - `sync`: a parent account's file copied onto the others.

use crate::provider::Provider;
use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use toml_edit::{DocumentMut, Item};

mod edit;
mod editor;
mod files;
mod specs;
mod sync;
#[cfg(test)]
mod testkit;

// Every file of the module reaches the others' items through `use super::*`,
// and the rest of the crate keeps its `crate::account_settings::X` paths.
pub(crate) use edit::*;
pub(crate) use editor::*;
use files::*;
pub(crate) use specs::*;
pub(crate) use sync::*;
#[cfg(test)]
use testkit::*;

/// Where the per-account locks of `rewriting_claude_config` are, in the data
/// dir (paths.rs `OWN_NAMES`).
pub(crate) const CLAUDE_CONFIG_LOCKS: &str = "claude-config-locks";

/// One rewrite of an account's `.claude.json` at a time, between Luna's own
/// writes (a trust mark, a recommended key) and a Claude Code that Luna
/// starts on the account only to ask it something (claude/models.rs): the
/// CLI rewrites the file from what it read as it starts, so a write made
/// meanwhile would be lost. Per account, so another account's write never
/// waits, and a file lock, since the window and the core both take it.
/// Waits up to `wait` for it, then says why not.
pub(crate) fn rewriting_claude_config(account_path: &str, wait: std::time::Duration) -> Result<std::fs::File, String> {
    use sha2::Digest;
    let dir = crate::paths::data_dir().join(CLAUDE_CONFIG_LOCKS);
    let hash = sha2::Sha256::digest(crate::paths::path_key(account_path).as_bytes());
    let name: String = hash.iter().take(8).map(|b| format!("{b:02x}")).collect();
    let path = dir.join(format!("{name}.lock"));
    let file = crate::secret::private_dir(&dir)
        .and_then(|()| crate::secret::open_private(&path))
        .map_err(|e| format!("no lock on {}: {e}", path.display()))?;
    let deadline = std::time::Instant::now() + wait;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(50))
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(format!("{account_path}: Claude Code is reading this account's settings; try again in a moment"))
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(format!("no lock on {}: {e}", path.display())),
        }
    }
}

/// `editing`'s hold, for as long as it lives.
pub(crate) struct Editing {
    _here: std::sync::MutexGuard<'static, ()>,
    // Unlocked as it closes.
    _everywhere: std::fs::File,
}

#[cfg(test)]
thread_local! {
    /// Where this test's lock is, in place of the data dir's.
    static LOCK_AT: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

fn lock_path() -> PathBuf {
    #[cfg(test)]
    if let Some(path) = LOCK_AT.with(|p| p.borrow().clone()) {
        return path;
    }
    crate::paths::data_dir().join("account-settings.lock")
}

/// One change of the accounts' config files at a time. Each reads its file,
/// changes it and writes it back, and two at once — a sync, a chat starting
/// that writes its folder's trust, a recommendation — would each write back
/// what it read without the other's change. A read that decides a write
/// belongs inside this. A file lock as well as a mutex: the window writes them
/// and so does the core, and a lock that cannot be had is an error, as each
/// going ahead under its own mutex would lose the other's change. The CLIs
/// write them too, which nothing here can hold back; account_use.rs says what
/// can be done about that.
pub(crate) fn editing() -> Result<Editing, String> {
    static L: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let here = L.lock().unwrap_or_else(|e| e.into_inner());
    let path = lock_path();
    let file = crate::secret::open_private(&path).and_then(|f| f.lock().map(|()| f));
    match file {
        Ok(file) => Ok(Editing { _here: here, _everywhere: file }),
        Err(e) => Err(format!("Nothing written: no lock on {}: {e}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_claude_config_rewrite_waits_only_for_the_same_account() {
        let short = std::time::Duration::from_millis(200);
        let one = format!("/luna-test/{}/one", std::process::id());
        let two = format!("/luna-test/{}/two", std::process::id());
        let held = rewriting_claude_config(&one, short).unwrap();
        let started = std::time::Instant::now();
        let why = rewriting_claude_config(&one, short).unwrap_err();
        assert!(why.contains("try again"), "{why}");
        assert!(started.elapsed() >= short, "it waited before giving up");
        // Another account's write does not wait on it.
        let other = rewriting_claude_config(&two, std::time::Duration::ZERO).unwrap();
        drop((held, other));
        assert!(rewriting_claude_config(&one, std::time::Duration::ZERO).is_ok(), "free once let go");
    }

    #[test]
    fn no_shared_lock_means_nothing_is_written() {
        let d = dir("no-lock");
        let p = d.to_string_lossy().into_owned();
        std::fs::write(d.join("settings.json"), r#"{"disableAllHooks":true}"#).unwrap();
        std::fs::write(d.join("config.toml"), "model = \"x\"\n").unwrap();
        let parent = dir("no-lock-parent");
        std::fs::write(parent.join("settings.json"), "{}").unwrap();
        // A folder where the lock file should be: as good as one unwritable
        // while the account files are not.
        LOCK_AT.with(|l| *l.borrow_mut() = Some(d.clone()));
        let off = |d: &Doc| d.bool_at(&["disableAllHooks"]) == Some(true);
        let one = std::slice::from_ref(&p);
        let e = edit_together(KeyFile::ClaudeSettings, one, one, &[Edit::Remove(&["disableAllHooks"])], &off).unwrap_err();
        assert!(e.contains("no lock"), "{e}");
        assert!(apply(Provider::Claude, &p, &["tui".into()], &[]).is_err());
        assert!(write_config_file(Provider::Claude, &p, "{}".into()).is_err());
        assert!(sync_one(Provider::Claude, &parent.to_string_lossy(), &p).is_err());
        assert!(crate::codex::trust::codex_trust_folder(p.clone(), "/somewhere".into()).is_err());
        assert!(crate::claude::trust::trust_folder(&p, "/somewhere").is_err());
        LOCK_AT.with(|l| *l.borrow_mut() = None);
        assert_eq!(std::fs::read_to_string(d.join("settings.json")).unwrap(), r#"{"disableAllHooks":true}"#);
        assert_eq!(std::fs::read_to_string(d.join("config.toml")).unwrap(), "model = \"x\"\n");
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&parent);
    }

    #[test]
    fn the_lock_keeps_out_whoever_holds_the_file_another_way() {
        // What another process does: its own handle on the same file.
        let d = dir("lock-held");
        let path = d.join("account-settings.lock");
        let theirs = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&path).unwrap();
        theirs.lock().unwrap();
        let (got, came) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            LOCK_AT.with(|l| *l.borrow_mut() = Some(path));
            let held = editing().map(|_| ());
            got.send(()).unwrap();
            held
        });
        assert!(came.recv_timeout(std::time::Duration::from_millis(150)).is_err(), "it went ahead under the other's lock");
        drop(theirs);
        came.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        waiter.join().unwrap().unwrap();
        let _ = std::fs::remove_dir_all(&d);
    }
}
