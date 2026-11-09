//! Which version directories a running CLI process starts from, across the
//! two processes that care: the core spawns the sessions, the window runs the
//! update pass that sweeps old versions (`prune_in`).
//!
//! A count in memory would only be the core's, so each process Luna starts
//! from a managed copy holds a shared lock on `<root>/in-use/<dir>` until it
//! has exited, and the sweep takes a directory only once it can hold that
//! file's exclusive lock. The OS lets go of a lock with the process that held
//! it, so a core that dies leaves no directory pinned for good. On Windows a
//! running exe cannot be deleted anyway; on Unix this is what keeps the sweep
//! from taking a running Codex session's directory, helpers and all, from
//! under it.

use crate::provider::Provider;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

pub(super) const DIR: &str = "in-use";

/// A process started from a version directory; dropped once it has exited.
#[must_use]
pub struct InUse {
    _lock: fs::File,
}

/// `<root>/in-use/<dir>`, its folder made where it is not there yet; None
/// where that folder is a link or junction: a lock file made or removed
/// through one would be outside Luna's tree.
fn lock_path(root: &Path, dir: &str) -> Option<PathBuf> {
    let locks = root.join(DIR);
    let _ = fs::create_dir_all(&locks);
    let meta = fs::symlink_metadata(&locks).ok()?;
    (meta.is_dir() && !super::is_link(&meta)).then(|| locks.join(dir))
}

/// A shared hold on `dir` under `root`, waited for while the exclusive one is
/// had: by `busy` for a moment, or by the sweep while it removes a directory
/// that is no longer current (the caller then reads `current` again). None
/// where the lock cannot be had at all (a data dir that will not take the
/// file): there the process is started all the same, and only the Unix sweep
/// is the worse for it.
pub(super) fn hold(root: &Path, dir: &str) -> Option<InUse> {
    let file = crate::secret::open_private(&lock_path(root, dir)?).ok()?;
    file.lock_shared().ok()?;
    Some(InUse { _lock: file })
}

/// What the sweep holds while it removes `dir`: None while any process holds
/// it shared. The lock file goes with the hold, once the directory has.
pub(super) struct Sweeping {
    file: fs::File,
    path: PathBuf,
}

/// None too where the lock file cannot be had: the directory stays, which
/// costs only its room on disk.
pub(super) fn try_sweep(root: &Path, dir: &str) -> Option<Sweeping> {
    let path = lock_path(root, dir)?;
    let file = crate::secret::open_private(&path).ok()?;
    file.try_lock().ok()?;
    Some(Sweeping { file, path })
}

impl Sweeping {
    /// The directory is gone: its lock file is no use to anyone.
    pub(super) fn done(self) {
        let Sweeping { file, path } = self;
        drop(file);
        let _ = fs::remove_file(path);
    }
}

/// Whether a process of Luna's runs from `dir` now.
pub(super) fn busy(root: &Path, dir: &str) -> bool {
    let Some(path) = lock_path(root, dir) else { return true };
    if fs::symlink_metadata(&path).is_err() {
        return false;
    }
    match crate::secret::open_private(&path) {
        Ok(file) => file.try_lock().is_err(),
        // Unreadable is taken as held: the sweep is the one that loses by it.
        Err(_) => true,
    }
}

/// The version each session's CLI last started on, in the process that
/// starts them (the core): for `luna_session_get` and diagnostics.
fn sessions() -> &'static Mutex<HashMap<String, (Provider, String)>> {
    static S: OnceLock<Mutex<HashMap<String, (Provider, String)>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

pub(super) fn note_session(session: &str, provider: Provider, version: &str) {
    sessions().lock().unwrap_or_else(|e| e.into_inner()).insert(session.to_string(), (provider, version.to_string()));
}

/// The CLI version a session's process last started from, if Luna started
/// one from a managed copy.
pub fn session_version(session: &str) -> Option<String> {
    sessions().lock().unwrap_or_else(|e| e.into_inner()).get(session).map(|(_, v)| v.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("luna-cli-inuse-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_held_directory_is_busy_and_not_swept_until_let_go() {
        let root = scratch("held");
        assert!(!busy(&root, "1.0.0"), "never started from");
        let one = hold(&root, "1.0.0").unwrap();
        let two = hold(&root, "1.0.0").unwrap();
        assert!(busy(&root, "1.0.0"));
        assert!(try_sweep(&root, "1.0.0").is_none(), "two processes run from it");
        drop(one);
        assert!(busy(&root, "1.0.0"), "one still does");
        drop(two);
        assert!(!busy(&root, "1.0.0"));
        let sweep = try_sweep(&root, "1.0.0").unwrap();
        // A start waits the sweep out.
        let (got, came) = std::sync::mpsc::channel();
        let at = root.clone();
        let waiter = std::thread::spawn(move || {
            let held = hold(&at, "1.0.0");
            got.send(()).unwrap();
            held.is_some()
        });
        assert!(came.recv_timeout(std::time::Duration::from_millis(150)).is_err(), "it started under the sweep");
        sweep.done();
        came.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert!(waiter.join().unwrap());
        let _ = fs::remove_dir_all(&root);
    }
}
