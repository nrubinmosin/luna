//! The canary: the first sessions on a version that has just become current,
//! watched for what the offline probe cannot see. A session whose CLI exits
//! with an error within `EARLY` of starting counts as a failure; `WARN_AT`
//! failures among the first `WATCH` sessions raise a warning — a desktop
//! notice, a badge in the status bar and a "Roll back" in Settings → CLIs.
//! Nothing rolls back on its own: a person decides (ADR-0019).
//!
//! An exit that says nothing of the version does not count, and gives its
//! watch to the next session: one whose last words name a cause outside the
//! CLI (`NOT_THE_VERSION`), and one Luna stopped itself (`session_stopped`).
//! A start Luna refuses (pty/folder.rs) never runs, so it is never watched.
//! Known causes, not "only exits after the CLI's startup succeeded": a
//! version that dies as it starts is the very thing this is for, and a pane's
//! CLI gives no sign of a finished startup both CLIs share.
//!
//! The window flips `current` and starts a canary for the new version
//! (`reset_in`); the core starts and reaps the sessions and counts them. So
//! the count lives in `<root>/canary.json`, read and written under the file
//! lock `<root>/canary.lock` by both.

use crate::provider::Provider;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const FILE: &str = "canary.json";
const LOCK: &str = "canary.lock";
/// In the test build only: a file of this name at the provider's root makes
/// each watched session a failure as it starts, for the live check of the
/// warning and the rollback.
const FORCE_FAIL: &str = "canary-fail";

/// How many sessions on a new version are watched.
pub(crate) const WATCH: u32 = 3;
/// How many of them failing raise the warning.
pub(crate) const WARN_AT: usize = 2;
/// An exit with an error sooner than this after the start is a failure: a
/// CLI that cannot run its session at all, not one stopped after work.
const EARLY: Duration = Duration::from_secs(30);

/// What a CLI says as it gives up on a start for a cause outside itself, and
/// the cause, each as it was seen (docs/cli-compatibility.md): an exit that
/// says one of these is the folder's doing, not the version's. Claude Code
/// 2.1.319's "Can only use --worktree in a git repository, but <folder> is
/// not a git repository" and Codex 0.167.0's git "fatal: not a git
/// repository" share the first; the second is Codex's own for the same.
const NOT_THE_VERSION: &[(&str, &str)] = &[
    ("not a git repository", "a worktree asked for outside a git repository"),
    ("Managed worktrees require a local Git repository", "a worktree asked for outside a git repository"),
];

/// The cause outside the CLI that `said` names, if it names one. Compared
/// with every space and line break taken out, and case aside: a pane's screen
/// breaks a long line wherever its width falls, mid-word too.
pub(crate) fn cause_outside(said: &str) -> Option<&'static str> {
    let squeezed = |s: &str| s.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect::<String>();
    let said = squeezed(said);
    NOT_THE_VERSION.iter().find(|(words, _)| said.contains(&squeezed(words))).map(|&(_, cause)| cause)
}

/// The canary of one version, as Settings shows it.
#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Canary {
    pub version: String,
    /// Sessions started on it so far, up to `WATCH`.
    pub watched: u32,
    /// Why each watched session failed.
    pub failures: Vec<String>,
    /// `WARN_AT` failures were seen.
    pub warned: bool,
    /// The person said to keep the version all the same.
    pub dismissed: bool,
}

/// Reads, changes and writes back the canary under its lock.
fn locked<T>(root: &Path, f: impl FnOnce(&mut Option<Canary>) -> T) -> Result<T, String> {
    fs::create_dir_all(root).map_err(|e| format!("mkdir {}: {e}", root.display()))?;
    let lock = crate::secret::open_private(&root.join(LOCK)).map_err(|e| format!("{LOCK}: {e}"))?;
    lock.lock().map_err(|e| format!("{LOCK}: {e}"))?;
    let mut canary = read_in(root);
    let before = canary.clone();
    let out = f(&mut canary);
    if canary != before {
        let bytes = match &canary {
            Some(c) => serde_json::to_vec_pretty(c).map_err(|e| e.to_string())?,
            None => b"null".to_vec(),
        };
        crate::secret::write_private(&root.join(FILE), &bytes).map_err(|e| format!("write {FILE}: {e}"))?;
    }
    Ok(out)
}

pub(crate) fn read_in(root: &Path) -> Option<Canary> {
    serde_json::from_slice(&fs::read(root.join(FILE)).ok()?).ok().flatten()
}

/// A version has just become current: watch its first sessions.
pub(super) fn reset_in(root: &Path, version: &str) -> Result<(), String> {
    locked(root, |c| *c = Some(Canary { version: version.to_string(), ..Default::default() }))
}

/// A session starts on `version`: whether it is one the canary watches.
fn watch_in(root: &Path, version: &str) -> Result<bool, String> {
    locked(root, |c| match c {
        Some(c) if c.version == version && c.watched < WATCH => {
            c.watched += 1;
            true
        }
        _ => false,
    })
}

/// A watched session on `version` said nothing of it: its watch goes to the
/// next session.
fn unwatch_in(root: &Path, version: &str) -> Result<(), String> {
    locked(root, |c| {
        if let Some(c) = c.as_mut().filter(|c| c.version == version) {
            c.watched = c.watched.saturating_sub(1);
        }
    })
}

/// A watched session on `version` failed for `why`: the canary as it now
/// stands when this failure is the one that raised the warning.
fn fail_in(root: &Path, version: &str, why: &str) -> Result<Option<Canary>, String> {
    locked(root, |c| match c {
        Some(c) if c.version == version => {
            c.failures.push(why.to_string());
            let raised = !c.warned && c.failures.len() >= WARN_AT;
            c.warned |= raised;
            raised.then(|| c.clone())
        }
        _ => None,
    })
}

/// "Keep it": the warning stays in the record, its badge goes.
pub(super) fn dismiss_in(root: &Path) -> Result<(), String> {
    locked(root, |c| {
        if let Some(c) = c {
            c.dismissed = true;
        }
    })
}

/// The watched sessions of this core: provider, root, version, start.
type Watching = HashMap<String, (Provider, std::path::PathBuf, String, Instant)>;

fn watching() -> &'static Mutex<Watching> {
    static W: OnceLock<Mutex<Watching>> = OnceLock::new();
    W.get_or_init(Default::default)
}

/// The core starts `session`'s CLI from `version` under `root`.
pub(super) fn session_started(provider: Provider, root: &Path, version: &str, session: &str) {
    match watch_in(root, version) {
        Ok(true) => {}
        Ok(false) => return,
        Err(e) => {
            crate::log::warn("cli", &format!("canary: {e}"));
            return;
        }
    }
    if crate::paths::is_test() && fs::symlink_metadata(root.join(FORCE_FAIL)).is_ok() {
        failed(provider, root, version, &format!("{session}: forced by the test build's {FORCE_FAIL} switch"));
        return;
    }
    watching()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(session.to_string(), (provider, root.to_path_buf(), version.to_string(), Instant::now()));
}

/// `session`'s CLI has exited, with an error or not. `last_words`: what it
/// left on its screen (a pane's) or its stderr (a helper's), read only for a
/// watched session's early error, for a cause outside the CLI.
pub fn session_ended(session: &str, error: bool, last_words: impl FnOnce() -> String) {
    let Some((provider, root, version, at)) = watching().lock().unwrap_or_else(|e| e.into_inner()).remove(session) else { return };
    if !error || at.elapsed() >= EARLY {
        return;
    }
    let after = at.elapsed().as_secs();
    match cause_outside(&last_words()) {
        Some(cause) => {
            crate::log::info(
                "cli",
                &format!("canary: {} {version}: {session} exited {after}s after it started, for {cause}: not counted", provider.as_str()),
            );
            if let Err(e) = unwatch_in(&root, &version) {
                crate::log::warn("cli", &format!("canary: {e}"));
            }
        }
        None => failed(provider, &root, &version, &format!("{session}: exited with an error {after}s after it started")),
    }
}

/// Luna is stopping `session` itself — a chat deleted or restarted, a login
/// window closed, a helper's turn stopped, Luna quitting — so whatever its CLI
/// exits with is Luna's doing: not counted, and one stopped within `EARLY`
/// gives its watch to the next session. Called before the stop.
pub fn session_stopped(session: &str) {
    let Some((_, root, version, at)) = watching().lock().unwrap_or_else(|e| e.into_inner()).remove(session) else { return };
    if at.elapsed() < EARLY {
        if let Err(e) = unwatch_in(&root, &version) {
            crate::log::warn("cli", &format!("canary: {e}"));
        }
    }
}

fn failed(provider: Provider, root: &Path, version: &str, why: &str) {
    crate::log::warn("cli", &format!("canary: {} {version}: {why}", provider.as_str()));
    match fail_in(root, version, why) {
        Ok(Some(canary)) => warn(provider, &canary),
        Ok(None) => {}
        Err(e) => crate::log::warn("cli", &format!("canary: {e}")),
    }
}

/// The warning: in the log, to the window (`cli://canary`, which shows the
/// badge and Settings' rollback) and as a desktop notice.
fn warn(provider: Provider, canary: &Canary) {
    let name = match provider {
        Provider::Claude => "Claude Code",
        Provider::Codex => "Codex",
    };
    let body = format!(
        "{} of the {} sessions started on {name} {} so far failed soon after starting. Settings → CLIs can roll back to the version before.",
        canary.failures.len(),
        canary.watched,
        canary.version
    );
    crate::log::warn("cli", &format!("canary warning: {body}"));
    crate::emit::to_ui("cli://canary", serde_json::json!({ "provider": provider, "version": canary.version }));
    crate::power::notify(&format!("{name} {} may be broken", canary.version), &body);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("luna-cli-canary-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn two_failures_among_the_first_sessions_raise_one_warning() {
        let root = scratch("warn");
        reset_in(&root, "2.0.0").unwrap();
        // A session on another version is none of this canary's.
        assert!(!watch_in(&root, "1.0.0").unwrap());
        for _ in 0..WATCH {
            assert!(watch_in(&root, "2.0.0").unwrap());
        }
        assert!(!watch_in(&root, "2.0.0").unwrap(), "only the first {WATCH}");
        assert_eq!(fail_in(&root, "2.0.0", "a").unwrap(), None);
        let raised = fail_in(&root, "2.0.0", "b").unwrap().expect("the second failure warns");
        assert_eq!((raised.watched, raised.failures.len(), raised.warned), (WATCH, 2, true));
        assert_eq!(fail_in(&root, "2.0.0", "c").unwrap(), None, "once");
        assert_eq!(fail_in(&root, "1.0.0", "d").unwrap(), None);

        dismiss_in(&root).unwrap();
        assert!(read_in(&root).unwrap().dismissed);
        // The next version starts afresh.
        reset_in(&root, "3.0.0").unwrap();
        assert_eq!(read_in(&root).unwrap(), Canary { version: "3.0.0".into(), ..Default::default() });
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn only_an_early_exit_with_an_error_of_a_watched_session_counts() {
        let root = scratch("ended");
        reset_in(&root, "2.0.0").unwrap();
        session_started(Provider::Codex, &root, "2.0.0", "canary-a");
        session_started(Provider::Codex, &root, "2.0.0", "canary-b");
        session_started(Provider::Codex, &root, "2.0.0", "canary-c");
        session_started(Provider::Codex, &root, "2.0.0", "canary-d");
        session_ended("canary-a", false, String::new);
        session_ended("canary-b", true, String::new);
        session_ended("canary-d", true, String::new);
        let c = read_in(&root).unwrap();
        assert_eq!((c.watched, c.failures.len(), c.warned), (3, 1, false), "d was not watched, a ended well: {c:?}");
        assert!(c.failures[0].starts_with("canary-b: exited with an error"));
        // A late one: watched, failed after its first half minute.
        {
            let mut w = watching().lock().unwrap();
            let started = &mut w.get_mut("canary-c").unwrap().3;
            *started = started.checked_sub(EARLY).expect("a clock half a minute old");
        }
        session_ended("canary-c", true, String::new);
        assert_eq!(read_in(&root).unwrap().failures.len(), 1);
        let _ = fs::remove_dir_all(&root);
    }

    /// The exit that raised a false warning on the 0.11.0 test build: Claude
    /// Code asked for a worktree in a folder outside git (2.1.319), as a pane
    /// of 100 columns shows it, broken mid-word; and Codex's on stderr.
    #[test]
    fn a_worktree_error_in_a_folder_outside_git_does_not_raise_the_canary() {
        let root = scratch("cause");
        reset_in(&root, "2.0.0").unwrap();
        let claude = "Error: Can only use --worktree in a git repository, but E:\\Projects\\Personal\\luna;C is not a git rep\n\
                      ository. Configure a WorktreeCreate hook to use --worktree with other VCS systems.";
        let codex = "Error: git command failed: fatal: not a git repository (or any of the parent directories): .git";
        for (id, provider, said) in [("canary-wt-a", Provider::Claude, claude), ("canary-wt-b", Provider::Codex, codex)] {
            session_started(provider, &root, "2.0.0", id);
            session_ended(id, true, || said.to_string());
        }
        let c = read_in(&root).unwrap();
        assert_eq!((c.watched, c.failures.len(), c.warned), (0, 0, false), "not counted, and their watches given back: {c:?}");
        // The next sessions are watched in their place, and still count.
        for id in ["canary-wt-c", "canary-wt-d"] {
            session_started(Provider::Claude, &root, "2.0.0", id);
            session_ended(id, true, || "TypeError: undefined is not a function".into());
        }
        let c = read_in(&root).unwrap();
        assert_eq!((c.watched, c.failures.len(), c.warned), (2, 2, true), "{c:?}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_session_luna_stops_is_no_failure_and_an_early_one_gives_its_watch_back() {
        let root = scratch("stopped");
        reset_in(&root, "2.0.0").unwrap();
        session_started(Provider::Claude, &root, "2.0.0", "canary-stop-a");
        session_started(Provider::Claude, &root, "2.0.0", "canary-stop-b");
        // A login window closed at once: stopped, then its CLI exits killed.
        session_stopped("canary-stop-a");
        session_ended("canary-stop-a", true, String::new);
        // One that ran its half minute and more keeps its watch.
        {
            let mut w = watching().lock().unwrap();
            let started = &mut w.get_mut("canary-stop-b").unwrap().3;
            *started = started.checked_sub(EARLY).expect("a clock half a minute old");
        }
        session_stopped("canary-stop-b");
        session_ended("canary-stop-b", true, String::new);
        let c = read_in(&root).unwrap();
        assert_eq!((c.watched, c.failures.len()), (1, 0), "{c:?}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn only_a_cause_outside_the_cli_is_known_by_its_words() {
        assert_eq!(cause_outside("FATAL: NOT A GIT\r\nREPOSITORY"), Some("a worktree asked for outside a git repository"));
        assert_eq!(
            cause_outside("Managed worktrees require a\nlocal Git repository."),
            Some("a worktree asked for outside a git repository")
        );
        for said in ["", "Error: Invalid API key", "segmentation fault", "a git repository was found"] {
            assert_eq!(cause_outside(said), None, "{said}");
        }
    }
}
