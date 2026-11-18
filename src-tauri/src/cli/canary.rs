//! The canary: the first sessions on a version that has just become current,
//! watched for what the offline probe cannot see. A session whose CLI exits
//! with an error within `EARLY` of starting counts as a failure;
//! `ROLL_BACK_AT` failures among the first `WATCH` sessions trip it, and Luna
//! rolls back on its own to the version kept before (`cli.rs` `roll_back`),
//! turns the failed one away until a newer one is out, and says so in the log
//! and a desktop notice (ADR-0030). With nothing to go back to, the notice
//! says that instead and the version stays.
//!
//! An exit that says nothing of the version does not count, and gives its
//! watch to the next session: one whose last words name a cause outside the
//! CLI (`NOT_THE_VERSION`), and one Luna stopped itself (`session_stopped`).
//! A start Luna refuses (pty/folder.rs) never runs, so it is never watched.
//! Known causes, not "only exits after the CLI's startup succeeded": a
//! version that dies as it starts is the very thing this is for, and a pane's
//! CLI gives no sign of a finished startup both CLIs share.
//!
//! The window's update pass starts a canary for the version it makes current
//! (`fresh`, in the same write as the flip); the core starts and reaps the
//! sessions, counts them and rolls back. So the count lives in the tree's
//! one record (`cli/state.rs`), each change of it one write under its lock.

use crate::provider::Provider;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// In the test build only: a file of this name at the provider's root makes
/// each watched session a failure as it starts, for the live check of the
/// rollback.
const FORCE_FAIL: &str = "canary-fail";

/// How many sessions on a new version are watched.
pub(crate) const WATCH: u32 = 3;
/// How many of them failing roll the version back.
pub(crate) const ROLL_BACK_AT: usize = 2;
/// An exit with an error sooner than this after the start is a failure: a
/// CLI that cannot run its session at all, not one stopped after work.
const EARLY: Duration = Duration::from_secs(30);
/// How long after `EARLY` a session still running is taken to be: a pane's
/// exit is seen by a look once a second (pty/spawn.rs), so one gone just
/// inside `EARLY` is reaped, and judged by `session_ended`, by then.
const REAP_SLACK: Duration = Duration::from_secs(2);

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

const SIGN_IN: &str = "a sign-in missing or expired";
const LIMIT: &str = "a usage limit";
const SERVICE: &str = "the network or the provider's service";

/// What a CLI says, anywhere in one of its last lines (`LAST_LINES`), when a
/// turn ends for want of a sign-in, of usage left, or of the network or the
/// API: none of it the version's doing, and each ends a session fast.
/// Lowercase, a typographic apostrophe read as a plain one; each as the
/// binary carries it, Claude Code 2.1.325 and Codex 0.169.0
/// (docs/cli-compatibility.md). A helper's words are its stderr's end and,
/// last, what its own stream said the turn failed on (background/run.rs); a
/// pane's, its last screen. Only the last lines, where a CLI says what it
/// stopped on: such words further up (a reconnect it got past, a prompt)
/// do not hide a crash below them.
const OUTSIDE_ANYWHERE: &[(&str, &str)] = &[
    // Codex: "unexpected status 401 Unauthorized", its `turn.failed`.
    ("401 unauthorized", SIGN_IN),
    // Claude Code: "Not logged in · Please run /login", "Session expired.
    // Please run /login to sign in again.", "Not logged in · Run /login",
    // "Invalid API key · Fix external API key", "Authentication required ·
    // Sign in again", "Authentication error · Try again".
    ("please run /login", SIGN_IN),
    ("not logged in", SIGN_IN),
    ("invalid api key", SIGN_IN),
    ("authentication required", SIGN_IN),
    ("authentication error", SIGN_IN),
    // Codex: "Your access token could not be refreshed. Please log out and
    // sign in again.", "… Please sign in again.".
    ("token could not be refreshed", SIGN_IN),
    ("sign in again", SIGN_IN),
    // Claude Code: "You've hit your session limit", "… · usage limit reached
    // · …", "Your credit balance is too low …". Codex: "You've hit your usage
    // limit.", "Quota exceeded. Check your plan and billing details."
    ("you've hit your session limit", LIMIT),
    ("you've hit your usage limit", LIMIT),
    ("usage limit reached", LIMIT),
    ("credit balance is too low", LIMIT),
    ("quota exceeded", LIMIT),
    // Claude Code: "Connection error.", "Request timed out.", "API Error:
    // 500 …", "… 529 … Overloaded". Codex: "error sending request",
    // "Reconnecting... waiting for network", "stream disconnected",
    // "exceeded retry limit, last status: …", "HTTP status server error".
    ("connection error", SERVICE),
    ("request timed out", SERVICE),
    ("api error: 5", SERVICE),
    ("overloaded", SERVICE),
    ("error sending request", SERVICE),
    ("waiting for network", SERVICE),
    ("stream disconnected", SERVICE),
    ("exceeded retry limit", SERVICE),
    ("http status server error", SERVICE),
];

/// How many of the last lines that are not blank `OUTSIDE_ANYWHERE` is read
/// in.
const LAST_LINES: usize = 3;

/// The cause outside the CLI that `said` names: a line that begins as one of
/// `NOT_THE_VERSION`'s, or one of the last `LAST_LINES` that holds one of
/// `OUTSIDE_ANYWHERE`'s. `NOT_THE_VERSION`'s words anywhere else (a prompt
/// on the screen, a folder named so, a path in another error) name nothing,
/// and an exit beside them still counts. Colors are taken out first. A
/// pane's screen keeps a line its width wrapped as one; a line broken before
/// its first words end is not known, and counts.
pub(crate) fn cause_outside(said: &str) -> Option<&'static str> {
    let lines: Vec<String> = said.lines().map(without_escapes).collect();
    let start = lines.iter().find_map(|line| {
        let line = line.trim_start();
        NOT_THE_VERSION.iter().find(|(start, _)| line.starts_with(start)).map(|&(_, cause)| cause)
    });
    start.or_else(|| {
        lines.iter().rev().filter(|l| !l.trim().is_empty()).take(LAST_LINES).find_map(|line| {
            let lower = line.to_lowercase().replace('\u{2019}', "'");
            OUTSIDE_ANYWHERE.iter().find(|(words, _)| lower.contains(words)).map(|&(_, cause)| cause)
        })
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
    /// `ROLL_BACK_AT` failures were seen: it tripped, and Luna is to roll
    /// back from it.
    pub warned: bool,
    /// Why the rollback it calls for could not be made. None while it is to
    /// be made: a core gone before it was is followed by one that makes it
    /// (`cli.rs` `finish_rollbacks`). A rollback made starts the canary of
    /// the version gone back to in the same write, so this one goes with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
}

/// Reads, changes and writes back the canary, as one change of the record.
fn locked<T>(root: &Path, f: impl FnOnce(&mut Option<Canary>) -> T) -> Result<T, String> {
    super::state::change(root, |s| f(&mut s.canary))
}

#[cfg(test)]
pub(crate) fn read_in(root: &Path) -> Option<Canary> {
    super::state::read_in(root).ok()?.canary
}

/// The watch over a version that has just become current, for the write
/// that makes it so.
pub(super) fn fresh(version: &str) -> Canary {
    Canary { version: version.to_string(), run: crate::secret::random_hex(8), ..Default::default() }
}

/// A canary of `version` and nothing else, as a flip would leave it.
#[cfg(test)]
pub(super) fn reset_in(root: &Path, version: &str) -> Result<(), String> {
    locked(root, |c| *c = Some(fresh(version)))
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
/// failure is the one that trips it.
fn fail_in(root: &Path, w: &Watch, why: &str) -> Result<Option<Canary>, String> {
    locked(root, |c| match c {
        Some(c) if w.of(c) => {
            c.failures.push(why.to_string());
            let raised = !c.warned && c.failures.len() >= ROLL_BACK_AT;
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
        std::thread::sleep(EARLY + REAP_SLACK);
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
        // On a thread of its own: going back holds every file of the version
        // to its sha256 (half a gigabyte for Codex) and sweeps, which the
        // reaper or the helper's turn that saw the exit must not wait on.
        Ok(Some(canary)) => {
            let (at, tripped_one) = (root.to_path_buf(), canary.clone());
            let spawned = std::thread::Builder::new().name("luna-cli-rollback".into()).spawn(move || tripped(provider, &at, &tripped_one));
            if let Err(e) = spawned {
                crate::log::warn("cli", &format!("canary: {e}; rolling back on this thread"));
                tripped(provider, root, &canary);
            }
        }
        Ok(None) => {}
        Err(e) => crate::log::warn("cli", &format!("canary: {e}")),
    }
}

/// The CLI's name, for what the person is told.
pub(super) fn name_of(provider: Provider) -> &'static str {
    match provider {
        Provider::Claude => "Claude Code",
        Provider::Codex => "Codex",
    }
}

/// The canary of `canary.version` has tripped: back to the version kept
/// before (`cli.rs` `roll_back`), and the person told what came of it.
pub(super) fn tripped(provider: Provider, root: &Path, canary: &Canary) {
    let version = &canary.version;
    let seen = seen_of(provider, canary);
    let rolled = super::roll_back(provider, root, version, &format!("rolled back: {seen}"));
    if let Err(e) = &rolled {
        note_outcome(root, version, e);
    }
    tell(provider, version, &seen, &rolled);
}

/// What the canary saw, in words.
pub(super) fn seen_of(provider: Provider, canary: &Canary) -> String {
    format!(
        "{} of the {} sessions started on {} {} so far failed soon after starting.",
        canary.failures.len(),
        canary.watched,
        name_of(provider),
        canary.version
    )
}

/// A rollback from `version` could not be made, for `why`: not to be tried
/// again by the next core.
pub(super) fn note_outcome(root: &Path, version: &str, why: &str) {
    let noted = locked(root, |c| {
        if let Some(c) = c.as_mut().filter(|c| c.version == version) {
            c.outcome = Some(why.to_string());
        }
    });
    if let Err(e) = noted {
        crate::log::warn("cli", &format!("canary: {e}"));
    }
}

/// What a rollback from `version` came to, after `seen`: in the log, to the
/// window (`cli://canary`, which reads the versions again) and as a desktop
/// notice.
pub(super) fn tell(provider: Provider, version: &str, seen: &str, rolled: &Result<String, String>) {
    let name = name_of(provider);
    let (title, body) = match rolled {
        Ok(to) => (
            format!("{name} rolled back to {to}"),
            format!("{seen} Luna went back to {to} and takes no {version} again while it is the newest release. Running sessions keep their version."),
        ),
        Err(e) => (format!("{name} {version} may be broken"), format!("{seen} Luna could not roll back: {e}.")),
    };
    crate::log::warn("cli", &format!("canary: {body}"));
    crate::emit::to_ui("cli://canary", serde_json::json!({ "provider": provider, "version": version }));
    crate::power::notify(&title, &body);
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
    fn two_failures_among_the_first_sessions_trip_it_once() {
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
        let raised = fail_in(&root, &w, "b").unwrap().expect("the second failure trips it");
        assert_eq!((raised.watched, raised.failures.len(), raised.warned), (WATCH, 2, true));
        assert_eq!(fail_in(&root, &w, "c").unwrap(), None, "once");
        assert_eq!(fail_in(&root, &watch(&root, "1.0.0"), "d").unwrap(), None);

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
        // One failure among them: no way through, though no rollback either.
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
        for said in mixed.into_iter().chain(["", "segmentation fault", "error: unexpected argument '--effort' found"]) {
            assert_eq!(cause_outside(said), None, "{said}");
        }
    }

    /// The exits a version is not to blame for and that end a session fast:
    /// each as its CLI words it (Claude Code 2.1.325, Codex 0.169.0).
    #[test]
    fn a_sign_in_a_usage_limit_or_the_network_is_a_cause_outside_the_cli() {
        for (said, cause) in [
            ("Not logged in · Please run /login", SIGN_IN),
            ("Session expired. Please run /login to sign in again.", SIGN_IN),
            ("Invalid API key · Fix external API key", SIGN_IN),
            ("Authentication required · Sign in again", SIGN_IN),
            ("Error: Your access token could not be refreshed. Please log out and sign in again.", SIGN_IN),
            ("You've hit your session limit · resets 3pm", LIMIT),
            ("Opus weekly limit reached · usage limit reached · send a message after it resets", LIMIT),
            ("Your credit balance is too low to access the Anthropic API.", LIMIT),
            ("ERROR: You\u{2019}ve hit your usage limit. Upgrade to Plus to continue.", LIMIT),
            ("Quota exceeded. Check your plan and billing details.", LIMIT),
            ("API Error: Connection error.", SERVICE),
            ("API Error: Request timed out.", SERVICE),
            ("API Error: 529 {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}", SERVICE),
            ("API Error: 500 Internal server error", SERVICE),
            ("Reconnecting... waiting for network", SERVICE),
            ("error: stream disconnected before completion: error sending request for url", SERVICE),
            ("ERROR: exceeded retry limit, last status: 503 Service Unavailable", SERVICE),
        ] {
            assert_eq!(cause_outside(&format!("starting\n\u{1b}[31m{said}\u{1b}[39m\n")), Some(cause), "{said}");
        }
        // Codex's own retries run out on a refused token (background/events.rs).
        assert_eq!(cause_outside("Reconnecting... 2/5\nunexpected status 401 Unauthorized"), Some(SIGN_IN));
        // An API refusal of what Luna sent is the version's to answer for.
        assert_eq!(cause_outside("API Error: 400 due to tool use concurrency issues."), None);
        // Such words above the CLI's last lines do not hide what it died of.
        let earlier = "API Error: Connection error.\nretrying\n> go on\n\nthread 'main' panicked at src/main.rs:1:1:\nindex out of range";
        assert_eq!(cause_outside(earlier), None);
    }

    /// A helper stopped by a usage limit, and a chat whose sign-in expired,
    /// each within half a minute: neither counted, both watches given back,
    /// no rollback.
    #[test]
    fn early_exits_for_causes_outside_the_cli_do_not_trip_it() {
        let root = scratch("outside");
        reset_in(&root, "2.0.0").unwrap();
        let causes = [
            ("canary-out-a", "You've hit your session limit · resets 3pm"),
            ("canary-out-b", "Not logged in · Please run /login"),
            ("canary-out-c", "stream disconnected before completion"),
        ];
        for (id, said) in causes {
            session_started(Provider::Claude, &root, "2.0.0", id);
            session_ended(id, true, || said.to_string());
        }
        let c = read_in(&root).unwrap();
        assert_eq!((c.watched, c.failures.len(), c.warned), (0, 0, false), "{c:?}");
        let _ = fs::remove_dir_all(&root);
    }

    /// A tripped canary whose rollback could not be made says why, and is
    /// then not taken up by the next core.
    #[test]
    fn a_rollback_that_could_not_be_made_is_noted_once() {
        let root = scratch("outcome");
        reset_in(&root, "2.0.0").unwrap();
        note_outcome(&root, "1.0.0", "another version's");
        assert_eq!(read_in(&root).unwrap().outcome, None);
        note_outcome(&root, "2.0.0", "no version before it is kept");
        assert_eq!(read_in(&root).unwrap().outcome.as_deref(), Some("no version before it is kept"));
        let _ = fs::remove_dir_all(&root);
    }
}
