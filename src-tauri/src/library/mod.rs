//! The user's own skills and instructions files, kept in Luna's library and
//! laid out where the CLIs read them: into the accounts each is meant for —
//! all of them, a provider's, or ones picked by name; a skill as
//! `skills/<name>`, instructions as `CLAUDE.md` (Claude Code) or `AGENTS.md`
//! (Codex) in its root, one per account — and for the sessions Luna starts
//! in folders the user names, their worktrees included. Nothing goes into such
//! a folder: a copy there would reach no worktree (each CLI makes one of what
//! git tracks) and be one `git add` away from the team's repository. A
//! folder's skills wait where only a session given them sees them —
//! `<data>/library-folders/<folder>/<account>/.claude/skills/<name>` for
//! Claude Code, passed as `--add-dir`; for Codex a plugin in the account's
//! own plugin cache, off until the session's `-c` turns it on — and
//! `session_args` lays the library out and hands them out at every start. A
//! skill goes into the accounts it is meant for as a whole; a folder can say
//! which accounts' sessions there get it instead, the deepest folder that
//! says anything of it deciding. One it gives to an account without the
//! skill as a whole waits for that account's sessions there; one it keeps
//! from an account that has it as a whole, or an outer folder gives, is
//! hidden by the CLI at the session's start, the copy staying where it is.
//!
//! The library is the source and a destination holds a copy. Luna remembers
//! what it wrote where (`<data>/library-state.json`, the bytes themselves in
//! `library-blobs/` for "since Luna wrote it" differences), so it touches only
//! its own copies: anything else at a destination is left alone, a copy
//! changed or deleted there is never silently replaced — the window asks
//! whether to take it into the library or put the library's back, and the
//! side that loses goes to `library-backups/` first — and a copy no longer
//! wanted goes only while it is still exactly what Luna wrote.
//!
//! This module holds what all of it shares: the library's folders, the
//! ledger of what Luna wrote, and the lock a pass runs under. The rest is
//! split by what it does:
//!
//! - `reach`: which accounts a skill or an instructions file is meant for,
//!   and which instructions an account gets.
//! - `audience`: whose sessions one is for — the person's chats, helpers, or
//!   both — and how each CLI keeps it from the others.
//! - `skills`: the library's skills and instructions files, read and checked.
//! - `files`: a copy's files on disk, and the blobs and backups kept of them.
//! - `folders`: the folders the user names: where their skills wait, and what
//!   a session started in one is given.
//! - `pass`: one pass over every copy the library wants, laying it out or
//!   only looking.
//! - `overview`: what the window is shown of the library and its copies.
//! - `commands`: the window's commands.
//! - `system`: the system skills, text Luna serves at its own trigger and
//!   never lays out, and the one override of each the library may hold.

use crate::accounts::AccountInfo;
use crate::provider::Provider;
use crate::settings::{Audience, LibraryFolder, LibrarySettings, Reach};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

mod audience;
mod commands;
mod files;
mod folders;
mod overview;
mod pass;
mod reach;
mod skills;
mod system;
#[cfg(test)]
mod testkit;

// Every file of the module reaches the others' items through `use super::*`,
// and the rest of the crate keeps its `crate::library::X` paths.
pub(crate) use audience::*;
pub(crate) use commands::*;
use files::*;
pub(crate) use folders::*;
pub(crate) use overview::*;
pub(crate) use pass::*;
pub(crate) use reach::*;
pub(crate) use skills::*;
pub(crate) use system::*;
#[cfg(test)]
pub(crate) use testkit::GoldenLibrary;
#[cfg(test)]
use testkit::*;

/// A unit's files by path relative to it, `/`-separated; the instructions
/// file is a unit of one, under "".
type Files = BTreeMap<String, Vec<u8>>;
/// The same as hashes.
type Hashes = BTreeMap<String, String>;
/// The library's skills by name, with their files when fit to be laid out.
type Skills = BTreeMap<String, (SkillInfo, Result<Files, String>)>;
/// The library's instructions by name: the file's bytes, None while blank
/// (nothing to lay out), or why it will not do.
type Instructions = BTreeMap<String, Result<Option<Vec<u8>>, String>>;

/// One pass at a time: the window can fire several changes in a row, and two
/// passes writing the same copy would each read the other's half. The window
/// and the core both run passes (a session's start is the core's), so the
/// mutex has a lock file beside the ledger for the other process (`lock`).
static LOCK: Mutex<()> = Mutex::new(());

/// Both locks, held for as long as it lives. The window's library commands
/// are `command(async)` for it: off the main thread, which would otherwise
/// freeze while one waits here for a pass of the core's.
struct Locked {
    _file: Option<std::fs::File>,
    _threads: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
thread_local! {
    /// Where this test's lock file is, in place of the one beside the ledger.
    static LOCK_AT: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

fn lock_path() -> PathBuf {
    #[cfg(test)]
    if let Some(path) = LOCK_AT.with(|p| p.borrow().clone()) {
        return path;
    }
    Dirs::current().ledger().with_extension("json.lock")
}

/// The lock file, open and locked. Opening it is tried three times, 50 ms
/// apart: on Windows a scanner or an indexer holding the file for a moment
/// fails the open, not the lock.
fn lock_file(path: &Path) -> std::io::Result<std::fs::File> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tries = 1;
    let file = loop {
        match crate::secret::open_private(path) {
            Ok(file) => break file,
            Err(_) if tries < 3 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => return Err(e),
        }
    };
    file.lock()?;
    Ok(file)
}

/// The lock everything that writes a copy, the ledger or the library goes
/// under. One that cannot be had is an error: the window and the core each
/// going ahead under their own mutex would each save the ledger as they read
/// it, losing or bringing back the other's records.
fn lock() -> Result<Locked, String> {
    let threads = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = lock_path();
    match lock_file(&path) {
        Ok(file) => Ok(Locked { _file: Some(file), _threads: threads }),
        Err(e) => Err(format!(
            "Nothing laid out: no lock on {}: {e}. Without it Luna's window and its core could undo each other's \
             changes; Luna's data folder needs a disk that can lock files",
            path.display()
        )),
    }
}

/// `lock` for what only reads — the overview the window takes at every focus,
/// a copy's differences: without the lock file it goes on under the mutex,
/// said once a run. The ledger is written whole (`Ledger::save`), so at
/// worst what it shows is a pass behind for a moment.
fn lock_to_read() -> Locked {
    static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    let threads = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = lock_path();
    let file = lock_file(&path);
    if let Err(e) = &file {
        if !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
            crate::log::warn("library", &format!("library lock {}: {e}; reading without it", path.display()));
        }
    }
    Locked { _file: file.ok(), _threads: threads }
}

/// The folder of instructions files, `<name>.md` each.
const INSTRUCTIONS_DIR: &str = "instructions";
/// Where the one instructions file lived while there could be only one;
/// `migrate_instructions` moves it into the folder.
const OLD_INSTRUCTIONS: &str = "instructions.md";
const SKILL_FILE: &str = "SKILL.md";
/// Backups older than this go at the next pass.
const BACKUP_DAYS: u64 = 30;
/// Under Luna's data: the skills each folder's Claude Code sessions get.
const FOLDERS_DIR: &str = "library-folders";
/// The marketplace a Codex account caches Luna's folder plugins under — no
/// name Codex keeps for its own, so none of its syncs prunes it.
pub(crate) const CODEX_MARKET: &str = "luna";
/// Before a Claude Code copy's name where another skill a session there sees
/// has that name: Claude Code would list ours as `<folder>:<name>`, scoped to
/// files under Luna's own folder, which no work ever is.
const RENAMED: &str = "luna-";

/// Where the instructions go in an account of this provider.
fn instructions_name(provider: Provider) -> &'static str {
    match provider {
        Provider::Claude => "CLAUDE.md",
        Provider::Codex => "AGENTS.md",
    }
}

/// The library's own folders and files, and Luna's notes beside it.
struct Dirs {
    root: PathBuf,
    /// `library-state.json`, `library-blobs/`, `library-backups/` live here.
    data: PathBuf,
}

impl Dirs {
    fn current() -> Dirs {
        let data = crate::paths::data_dir();
        let root = crate::settings::get().library.source.map(PathBuf::from).unwrap_or_else(|| data.join("library"));
        Dirs { root, data }
    }
    fn instructions(&self) -> PathBuf {
        self.root.join(INSTRUCTIONS_DIR)
    }
    fn instructions_file(&self, name: &str) -> PathBuf {
        self.instructions().join(format!("{name}.md"))
    }
    fn skills(&self) -> PathBuf {
        self.root.join("skills")
    }
    fn folders(&self) -> PathBuf {
        self.data.join(FOLDERS_DIR)
    }
    fn ledger(&self) -> PathBuf {
        self.data.join("library-state.json")
    }
    fn blobs(&self) -> PathBuf {
        self.data.join("library-blobs")
    }
    fn backups(&self) -> PathBuf {
        self.data.join("library-backups")
    }
    /// An edit of the library: private in the data dir's own `library/`
    /// (`write_file`), as that folder has it in a source of the user's
    /// choosing (`write_copy`), which is theirs, not Luna's to lock down.
    fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), String> {
        if self.root.starts_with(&self.data) {
            write_file(path, bytes)
        } else {
            write_copy(path, bytes)
        }
    }
}

/// What the library has written into the account folder `account_path`, by
/// destination, as its ledger says: what the window lists as the library's
/// layout among what Luna applied to a chat on that account (applied.rs).
pub fn laid_out_in(account_path: &str) -> Vec<String> {
    let sep = if cfg!(windows) { '\\' } else { '/' };
    let under = format!("{}{sep}", crate::paths::path_key(account_path));
    Ledger::load(&Dirs::current()).written.into_keys().filter(|dest| crate::paths::path_key(dest).starts_with(&under)).collect()
}

/// What Luna last wrote at each destination, file by file.
#[derive(Serialize, Deserialize, Default)]
struct Ledger {
    written: BTreeMap<String, Hashes>,
    /// The instructions copies Luna wrote with a line before them saying whose
    /// they are (audience.rs `worded`), by destination, with the audience it
    /// named: only that line, and only there, comes off when such a copy is
    /// taken into the library.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    worded: BTreeMap<String, Audience>,
    /// Luna's copies no longer wanted that were changed where they lie, left
    /// there (pass.rs), by destination, with what Luna last wrote there: shown
    /// under Needs a look until the window takes one out or keeps it as the
    /// user's own, the file goes, the place is wanted again or left out.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    kept: BTreeMap<String, Hashes>,
}

impl Ledger {
    /// What Luna has now written at `key`: these files, said for `banner`'s
    /// sessions alone where that is not every session's.
    fn wrote(&mut self, key: &str, hashes: Hashes, banner: Audience) {
        self.written.insert(key.to_string(), hashes);
        match banner {
            Audience::Both => self.worded.remove(key),
            other => self.worded.insert(key.to_string(), other),
        };
    }
    /// Luna's note of `key` gone, files and line alike.
    fn forget(&mut self, key: &str) -> Option<Hashes> {
        self.worded.remove(key);
        self.written.remove(key)
    }
}

impl Ledger {
    fn load(dirs: &Dirs) -> Ledger {
        std::fs::read_to_string(dirs.ledger()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }
    fn save(&self, dirs: &Dirs) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        // Luna's own, in the data dir: private like the rest of it, where
        // `write_file` is for the copies in account folders.
        crate::secret::write_private(&dirs.ledger(), text.as_bytes()).map_err(|e| format!("{}: {e}", dirs.ledger().display()))
    }
}
