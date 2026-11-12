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

/// The line a CLI writes as it gives up on a start for a cause outside
/// itself, by how that line begins, and the cause; each as it was seen
/// (docs/cli-compatibility.md). An exit that wrote one is the folder's doing,
/// not the version's. Claude Code 2.1.319 (stderr, in a pane and under `-p`)
/// and `codex exec` 0.167.0 (stderr) on `--worktree` outside a repository.
/// Codex's interactive start there is refused before it runs (pty/folder.rs),
/// so its own words are not needed.
const NOT_THE_VERSION: &[(&str, &str)] = &[
    ("Error: Can only use --worktree in a git repository, but ", "a worktree asked for outside a git repository"),
    ("Error: git command failed: fatal: not a git repository", "a worktree asked for outside a git repository"),
];

/// The cause outside the CLI that `said` names, if a line of it is one of
/// `NOT_THE_VERSION`'s, from the line's start: the words anywhere else (a
/// prompt on the screen, a folder named so, a path in another error) name
/// nothing, and an exit beside them still counts. Colors are taken out first.
/// A pane's screen keeps a line its width wrapped as one; a line broken
/// before its first words end is not known, and counts.
pub(crate) fn cause_outside(said: &str) -> Option<&'static str> {
    said.lines().map(without_escapes).find_map(|line| {
        let line = line.trim_start();
        NOT_THE_VERSION.iter().find(|(start, _)| line.starts_with(start)).map(|&(_, cause)| cause)
    })
}

/// `line` without its escape sequences (a color, a cursor move): CSI ones to
/// their final byte, any other to the character after the escape.
fn without_escapes(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        if chars.next() == Some('[') {
            for c in chars.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&c) {
                    break;
                }
            }
        }
    }
    out
}

/// The canary of one version, as Settings shows it.
#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Canary {
    pub version: String,
    /// New with each reset, so a session watched for an earlier watch of the
    /// same version (flipped away and back meanwhile) counts for none.
    #[serde(default)]
    pub run: String,
    /// Sessions started on it so far, up to `WATCH`.
    pub watched: u32,
    /// Watched sessions past `EARLY` with no failure: ended well, stopped,
    /// or still running. `WATCH` of them with no failures is a version that
    /// got through its first sessions; a core that went away with some
    /// still inside `EARLY` leaves them uncounted, and the count short.
    #[serde(default)]
    pub settled: u32,
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
    let run = crate::secret::random_hex(8);
    locked(root, |c| *c = Some(Canary { version: version.to_string(), run, ..Default::default() }))
}

/// Which watch a session counts for: a version, as one reset began watching it.
#[derive(Clone, Debug, PartialEq)]
struct Watch {
    version: String,
    run: String,
}

impl Watch {
    fn of(&self, c: &Canary) -> bool {
        c.version == self.version && c.run == self.run
    }
}

/// A session starts on `version`: the watch it counts for, if the canary
/// watches it.
fn watch_in(root: &Path, version: &str) -> Result<Option<Watch>, String> {
    locked(root, |c| match c {
        Some(c) if c.version == version && c.watched < WATCH => {
            c.watched += 1;
            Some(Watch { version: c.version.clone(), run: c.run.clone() })
        }
        _ => None,
    })
}

/// A watched session said nothing of its version: its watch goes to the next
/// session.
fn unwatch_in(root: &Path, w: &Watch) -> Result<(), String> {
    locked(root, |c| {
        if let Some(c) = c.as_mut().filter(|c| w.of(c)) {
            c.watched = c.watched.saturating_sub(1);
        }
    })
}

/// A watched session failed for `why`: the canary as it now stands when this
/// failure is the one that raised the warning.
fn fail_in(root: &Path, w: &Watch, why: &str) -> Result<Option<Canary>, String> {
    locked(root, |c| match c {
        Some(c) if w.of(c) => {
            c.failures.push(why.to_string());
            let raised = !c.warned && c.failures.len() >= WARN_AT;
            c.warned |= raised;
            raised.then(|| c.clone())
        }
        _ => None,
    })
}

/// A watched session ended well or got past `EARLY` with no failure: the
/// canary as it now stands when this one is the last of `WATCH` and none
/// failed.
fn settle_in(root: &Path, w: &Watch) -> Result<Option<Canary>, String> {
    locked(root, |c| match c {
        Some(c) if w.of(c) && c.settled < WATCH => {
            c.settled += 1;
            (c.settled == WATCH && c.failures.is_empty()).then(|| c.clone())
        }
        _ => None,
    })
}

/// Settles a watched session; once its version is through, the log says so
/// and the window reads the versions again (`cli://canary`) for Settings' tag.
fn settled(provider: Provider, root: &Path, w: &Watch) {
    match settle_in(root, w) {
        Ok(Some(c)) => {
            crate::log::info(
                "cli",
                &format!("canary: {} {}: its first {WATCH} sessions got past their start", provider.as_str(), c.version),
            );
            crate::emit::to_ui("cli://canary", serde_json::json!({ "provider": provider, "version": c.version }));
        }
        Ok(None) => {}
        Err(e) => crate::log::warn("cli", &format!("canary: {e}")),
    }
}

/// "Keep it": the warning stays in the record, its badge goes.
pub(super) fn dismiss_in(root: &Path) -> Result<(), String> {
    locked(root, |c| {
        if let Some(c) = c {
            c.dismissed = true;
        }
    })
}

/// A session the canary watches, in this core.
struct Watched {
    provider: Provider,
    root: std::path::PathBuf,
    watch: Watch,
    /// Its CLI's start.
    at: Instant,
    /// When its CLI's process was reaped (`session_exited`): what it left on
    /// its pipes may take a while yet to be read before `session_ended`.
    exited: Option<Instant>,
}

impl Watched {
    /// How long its CLI ran, or has run so far.
    fn ran(&self) -> Duration {
        self.exited.unwrap_or_else(Instant::now).saturating_duration_since(self.at)
    }
}

type Watching = HashMap<String, Watched>;

fn watching() -> &'static Mutex<Watching> {
    static W: OnceLock<Mutex<Watching>> = OnceLock::new();
    W.get_or_init(Default::default)
}

/// The core starts `session`'s CLI from `version` under `root`.
pub(super) fn session_started(provider: Provider, root: &Path, version: &str, session: &str) {
    let watch = match watch_in(root, version) {
        Ok(Some(w)) => w,
        Ok(None) => return,
        Err(e) => {
            crate::log::warn("cli", &format!("canary: {e}"));
            return;
        }
    };
    if crate::paths::is_test() && fs::symlink_metadata(root.join(FORCE_FAIL)).is_ok() {
        failed(provider, root, &watch, &format!("{session}: forced by the test build's {FORCE_FAIL} switch"));
        return;
    }
    let at = Instant::now();
    watching()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(session.to_string(), Watched { provider, root: root.to_path_buf(), watch, at, exited: None });
    // Still running once `EARLY` is up: nothing it does from then on counts,
    // so it settles now rather than whenever it ends. One whose process has
    // gone is `session_ended`'s to judge, by when it went.
    let id = session.to_string();
    let spawned = std::thread::Builder::new().name("luna-canary".into()).spawn(move || {
        std::thread::sleep(EARLY);
        let mut w = watching().lock().unwrap_or_else(|e| e.into_inner());
        if w.get(&id).is_some_and(|s| s.at == at && s.exited.is_none()) {
            let s = w.remove(&id).expect("just looked up");
            drop(w);
            settled(s.provider, &s.root, &s.watch);
        }
    });
    if let Err(e) = spawned {
        crate::log::warn("cli", &format!("canary: {e}; {session} settles only as it ends"));
    }
}

/// `session`'s CLI process has been reaped, before what it wrote is read to
/// the end: its run is timed from here, not from `session_ended`.
pub fn session_exited(session: &str) {
    if let Some(s) = watching().lock().unwrap_or_else(|e| e.into_inner()).get_mut(session) {
        s.exited.get_or_insert_with(Instant::now);
    }
}

/// `session`'s CLI has exited, with an error or not. `last_words`: what it
/// left on its screen (a pane's) or its stderr (a helper's), read only for a
/// watched session's early error, for a cause outside the CLI.
pub fn session_ended(session: &str, error: bool, last_words: impl FnOnce() -> String) {
    let Some(s) = watching().lock().unwrap_or_else(|e| e.into_inner()).remove(session) else { return };
    let ran = s.ran();
    if !error || ran >= EARLY {
        settled(s.provider, &s.root, &s.watch);
        return;
    }
    let (after, version) = (ran.as_secs(), &s.watch.version);
    match cause_outside(&last_words()) {
        Some(cause) => {
            crate::log::info(
                "cli",
                &format!("canary: {} {version}: {session} exited {after}s after it started, for {cause}: not counted", s.provider.as_str()),
            );
            if let Err(e) = unwatch_in(&s.root, &s.watch) {
                crate::log::warn("cli", &format!("canary: {e}"));
            }
        }
        None => failed(s.provider, &s.root, &s.watch, &format!("{session}: exited with an error {after}s after it started")),
    }
}

/// Luna is stopping `session` itself — a chat deleted or restarted, a login
/// window closed, a helper's turn stopped, Luna quitting — so whatever its CLI
/// exits with is Luna's doing: not counted, and one stopped within `EARLY`
/// gives its watch to the next session. Called before the stop.
pub fn session_stopped(session: &str) {
    let Some(s) = watching().lock().unwrap_or_else(|e| e.into_inner()).remove(session) else { return };
    if s.ran() >= EARLY {
        settled(s.provider, &s.root, &s.watch);
    } else if let Err(e) = unwatch_in(&s.root, &s.watch) {
        crate::log::warn("cli", &format!("canary: {e}"));
    }
}

fn failed(provider: Provider, root: &Path, w: &Watch, why: &str) {
    crate::log::warn("cli", &format!("canary: {} {}: {why}", provider.as_str(), w.version));
    match fail_in(root, w, why) {
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

    /// The watch a session started on `version` now would count for.
    fn watch(root: &Path, version: &str) -> Watch {
        Watch { version: version.into(), run: read_in(root).map(|c| c.run).unwrap_or_default() }
    }

    /// Makes `session`'s start half a minute older.
    fn age(session: &str) {
        let mut w = watching().lock().unwrap();
        let at = &mut w.get_mut(session).unwrap().at;
        *at = at.checked_sub(EARLY).expect("a clock half a minute old");
    }

    #[test]
    fn two_failures_among_the_first_sessions_raise_one_warning() {
        let root = scratch("warn");
        reset_in(&root, "2.0.0").unwrap();
        // A session on another version is none of this canary's.
        assert!(watch_in(&root, "1.0.0").unwrap().is_none());
        for _ in 0..WATCH {
            assert_eq!(watch_in(&root, "2.0.0").unwrap(), Some(watch(&root, "2.0.0")));
        }
        assert!(watch_in(&root, "2.0.0").unwrap().is_none(), "only the first {WATCH}");
        let w = watch(&root, "2.0.0");
        assert_eq!(fail_in(&root, &w, "a").unwrap(), None);
        let raised = fail_in(&root, &w, "b").unwrap().expect("the second failure warns");
        assert_eq!((raised.watched, raised.failures.len(), raised.warned), (WATCH, 2, true));
        assert_eq!(fail_in(&root, &w, "c").unwrap(), None, "once");
        assert_eq!(fail_in(&root, &watch(&root, "1.0.0"), "d").unwrap(), None);

        dismiss_in(&root).unwrap();
        assert!(read_in(&root).unwrap().dismissed);
        // The next version starts afresh.
        reset_in(&root, "3.0.0").unwrap();
        let fresh = read_in(&root).unwrap();
        assert_eq!(fresh, Canary { version: "3.0.0".into(), run: fresh.run.clone(), ..Default::default() });
        assert_eq!(fresh.run.len(), 16);
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
        age("canary-c");
        session_ended("canary-c", true, String::new);
        let c = read_in(&root).unwrap();
        assert_eq!((c.failures.len(), c.settled), (1, 2), "a and c got past their start: {c:?}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_version_gets_through_once_its_watched_sessions_settle_with_no_failure() {
        let root = scratch("settle");
        reset_in(&root, "2.0.0").unwrap();
        for _ in 0..WATCH {
            watch_in(&root, "2.0.0").unwrap().unwrap();
        }
        let w = watch(&root, "2.0.0");
        assert_eq!(settle_in(&root, &w).unwrap(), None);
        assert_eq!(settle_in(&root, &watch(&root, "1.0.0")).unwrap(), None, "another version's");
        assert_eq!(settle_in(&root, &w).unwrap(), None, "started is not settled: one is still to come");
        let through = settle_in(&root, &w).unwrap().expect("the last of them");
        assert_eq!((through.watched, through.settled), (WATCH, WATCH));
        assert_eq!(settle_in(&root, &w).unwrap(), None, "once");
        // One failure among them: no way through, though no warning either.
        reset_in(&root, "3.0.0").unwrap();
        let w = watch(&root, "3.0.0");
        fail_in(&root, &w, "a").unwrap();
        assert!((0..WATCH).all(|_| settle_in(&root, &w).unwrap().is_none()));
        // A watch of the same version from before a reset counts for nothing.
        reset_in(&root, "3.0.0").unwrap();
        assert!(settle_in(&root, &w).unwrap().is_none() && fail_in(&root, &w, "b").unwrap().is_none());
        unwatch_in(&root, &w).unwrap();
        let c = read_in(&root).unwrap();
        assert_eq!((c.watched, c.settled, c.failures.len()), (0, 0, 0), "{c:?}");
        // A canary.json from before the field: none settled.
        fs::write(root.join(FILE), r#"{"version":"4.0.0","watched":3,"failures":[],"warned":false,"dismissed":false}"#).unwrap();
        assert_eq!(read_in(&root).unwrap().settled, 0);
        let _ = fs::remove_dir_all(&root);
    }

    /// A helper's process gone with an error inside its half minute, whose
    /// pipes take longer than that to read to the end, still fails it: the
    /// timer leaves it be, and its run is timed to when it was reaped.
    #[test]
    fn an_early_exit_read_out_late_still_counts() {
        let root = scratch("reaped");
        reset_in(&root, "2.0.0").unwrap();
        session_started(Provider::Codex, &root, "2.0.0", "canary-late");
        session_exited("canary-late");
        {
            let mut w = watching().lock().unwrap();
            let s = w.get_mut("canary-late").unwrap();
            // Started 10 s before it was reaped, and read out a minute later.
            let exited = s.exited.unwrap().checked_sub(Duration::from_secs(60)).expect("a clock a minute old");
            s.exited = Some(exited);
            s.at = exited.checked_sub(Duration::from_secs(10)).expect("a clock 70 s old");
        }
        session_ended("canary-late", true, String::new);
        let c = read_in(&root).unwrap();
        assert_eq!((c.failures.len(), c.settled), (1, 0), "{c:?}");
        assert!(c.failures[0].contains("10s after it started"), "{c:?}");
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
        // The next sessions are watched in their place, and still count: one
        // with the words on its screen, but not as the CLI's own line.
        let mixed = "> fix: git says not a git repository in C:\\work\\not a git repository\nTypeError: undefined is not a function";
        for id in ["canary-wt-c", "canary-wt-d"] {
            session_started(Provider::Claude, &root, "2.0.0", id);
            session_ended(id, true, || mixed.into());
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
        age("canary-stop-b");
        session_stopped("canary-stop-b");
        session_ended("canary-stop-b", true, String::new);
        let c = read_in(&root).unwrap();
        assert_eq!((c.watched, c.failures.len()), (1, 0), "{c:?}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn only_a_cause_outside_the_cli_is_known_by_its_words() {
        const WORKTREE: Option<&str> = Some("a worktree asked for outside a git repository");
        // Claude Code's line, red under `-p` with colors forced, among others.
        let claude = "\u{1b}[31mError: Can only use --worktree in a git repository, but \u{1b}[1m/w/x\u{1b}[22m is not a git \
                      repository. Configure a WorktreeCreate hook to use --worktree with other VCS systems.\u{1b}[39m";
        assert_eq!(cause_outside(&format!("starting\n{claude}\n")), WORKTREE);
        assert_eq!(
            cause_outside("  Error: git command failed: fatal: not a git repository (or any of the parent directories): .git"),
            WORKTREE
        );
        // The words elsewhere than at the start of the CLI's own line name
        // nothing, and the version's own failure beside them counts.
        let mixed = [
            "> why does git say not a git repository here?\nTypeError: undefined is not a function",
            "Error: cannot open C:\\work\\not a git repository\\CLAUDE.md\nTypeError: x is not a function",
            "fatal: not a git repository (or any of the parent directories): .git\npanic: index out of range",
            "Managed worktrees require a local Git repository.",
        ];
        for said in mixed.into_iter().chain(["", "Error: Invalid API key", "segmentation fault"]) {
            assert_eq!(cause_outside(said), None, "{said}");
        }
    }
}
