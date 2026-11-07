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

use crate::accounts::AccountInfo;
use crate::provider::Provider;
use crate::settings::{LibraryFolder, LibrarySettings, Reach};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

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

fn lock() -> Locked {
    let threads = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = Dirs::current().ledger().with_extension("json.lock");
    let file = crate::secret::open_private(&path, std::fs::OpenOptions::new().write(true)).and_then(|f| f.lock().map(|()| f));
    // Without it a pass still runs, as one did before passes came from two
    // processes.
    if let Err(e) = &file {
        crate::log::warn("library", &format!("library lock {}: {e}", path.display()));
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
const CODEX_MARKET: &str = "luna";
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

// ------------------------------------------------------------------ reach --

const ALL: &str = "all";

/// How closely a reach takes in this account: 2 by its name, 1 by its
/// provider, 0 as one of all; None when it does not.
fn closeness(reach: &[String], a: &AccountInfo) -> Option<u8> {
    let provider = a.provider.as_str();
    reach
        .iter()
        .filter_map(|r| match r {
            r if r.split_once('/') == Some((provider, a.name.as_str())) => Some(2),
            r if r == provider => Some(1),
            r if r == ALL => Some(0),
            _ => None,
        })
        .max()
}

/// Whether a reach takes in this account.
fn reaches(reach: &[String], a: &AccountInfo) -> bool {
    closeness(reach, a).is_some()
}

/// Each entry `all`, a provider or `provider/account`.
fn check_reach(reach: &[String]) -> Result<(), String> {
    let provider = |p: &str| Provider::ALL.iter().any(|x| x.as_str() == p);
    for r in reach {
        let fits = r == ALL || provider(r) || r.split_once('/').is_some_and(|(p, n)| provider(p) && !n.is_empty() && !n.contains('/'));
        if !fits {
            return Err(format!("{r} is not all, a provider or provider/account"));
        }
    }
    Ok(())
}

/// A reach as the user ticked it, checked and made the shortest that takes in
/// the same accounts: `all` alone, a provider without its own accounts,
/// nothing twice.
fn tidy_reach(reach: Vec<String>) -> Result<Reach, String> {
    check_reach(&reach)?;
    if reach.iter().any(|r| r == ALL) {
        return Ok(vec![ALL.into()]);
    }
    let mut out: Reach = Vec::new();
    for r in &reach {
        let covered = r.split_once('/').is_some_and(|(p, _)| reach.iter().any(|x| x == p));
        if !covered && !out.contains(r) {
            out.push(r.clone());
        }
    }
    Ok(out)
}

/// Gives one instructions (None: no instructions at all) this reach, and
/// takes each of its entries away from every other: an account named, a
/// provider and `all` are each one instructions' at a time, so where two
/// reaches meet on an account the closer one decides and never a tie. An
/// account is named beside `all` or its provider on purpose here, unlike in
/// a skill's reach — named, it beats another's provider — so nothing but
/// repeats is tidied away.
fn give_instructions(lib: &mut LibrarySettings, name: Option<&str>, mut reach: Reach) {
    let mut seen = BTreeSet::new();
    reach.retain(|r| seen.insert(r.clone()));
    for (n, r) in lib.instructions.iter_mut() {
        if Some(n.as_str()) != name {
            r.retain(|x| !reach.contains(x));
        }
    }
    match name {
        Some(n) => {
            lib.no_instructions.retain(|x| !reach.contains(x));
            lib.instructions.insert(n.to_string(), reach);
        }
        None => lib.no_instructions = reach,
    }
    lib.instructions.retain(|_, r| !r.is_empty());
}

/// The instructions an account gets, of those `have` says the library has:
/// the one whose reach takes it in most closely, or none when that is
/// `no_instructions`' (which wins a tie, as the safer side). Two at one
/// closeness only come of settings edited by hand: the first by name wins.
fn chosen<'a>(lib: &'a LibrarySettings, have: impl Fn(&str) -> bool, a: &AccountInfo) -> Option<&'a str> {
    let mut best: Option<(u8, Option<&str>)> = closeness(&lib.no_instructions, a).map(|c| (c, None));
    for (name, reach) in lib.instructions.iter().filter(|(n, _)| have(n)) {
        if let Some(c) = closeness(reach, a) {
            if best.is_none_or(|(b, _)| c > b) {
                best = Some((c, Some(name.as_str())));
            }
        }
    }
    best.and_then(|(_, n)| n)
}

/// An account renamed (`to`) or deleted (None), as `provider/name`: every
/// reach that named it follows. A folder's reach left empty stays, keeping
/// the skill from every account there, as it kept it from all but that one.
pub fn rename_account(lib: &mut LibrarySettings, from: &str, to: Option<&str>) {
    let folders = lib.folders.iter_mut().flat_map(|f| f.skills.values_mut());
    let reaches =
        lib.instructions.values_mut().chain(lib.skills.values_mut()).chain(std::iter::once(&mut lib.no_instructions)).chain(folders);
    for reach in reaches {
        match to {
            Some(to) => reach.iter_mut().filter(|r| *r == from).for_each(|r| *r = to.to_string()),
            None => reach.retain(|r| r != from),
        }
    }
    lib.instructions.retain(|_, reach| !reach.is_empty());
    lib.skills.retain(|_, reach| !reach.is_empty());
    for f in &mut lib.folders {
        if let Some(choice) = f.instructions.remove(from) {
            if let Some(to) = to {
                f.instructions.insert(to.to_string(), choice);
            }
        }
    }
}

// ----------------------------------------------------------------- places --

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
}

/// What Luna last wrote at each destination, file by file.
#[derive(Serialize, Deserialize, Default)]
struct Ledger {
    written: BTreeMap<String, Hashes>,
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

// ----------------------------------------------------------------- skills --

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    /// Why it is laid out nowhere, while it is not fit to be.
    pub problem: Option<String>,
    /// The accounts it goes into, wherever no folder says otherwise.
    pub accounts: Reach,
}

/// A name both CLIs take for a skill, and the only kind Luna makes a folder
/// of; instructions are named the same way, for one rule in the window.
fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 64 {
        return Err("A name is 1 to 64 characters".into());
    }
    if !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') || name.starts_with('-') {
        return Err("A name is lowercase letters, digits and hyphens".into());
    }
    Ok(())
}

/// `name` and `description` from SKILL.md's front matter — what both CLIs
/// need of it — as plain strings: quoted, folded (`>`) and literal (`|`)
/// values and indented continuation lines all read.
fn front_matter(text: &str) -> Result<(Option<String>, Option<String>), String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.lines();
    if lines.next().map(str::trim_end) != Some("---") {
        return Err("SKILL.md does not open with front matter (a --- line)".into());
    }
    let mut keys: BTreeMap<&str, (String, bool)> = BTreeMap::new();
    let mut current: Option<&str> = None;
    for line in lines {
        if line.trim_end() == "---" {
            let get = |k: &str| {
                keys.get(k).map(|(v, _)| {
                    let v = v.trim();
                    let unquoted = (v.len() >= 2
                        && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\''))))
                    .then(|| &v[1..v.len() - 1]);
                    unquoted.unwrap_or(v).to_string()
                })
            };
            return Ok((get("name"), get("description")));
        }
        if line.starts_with([' ', '\t']) || line.trim().is_empty() {
            if let Some((value, literal)) = current.and_then(|k| keys.get_mut(k)) {
                if !line.trim().is_empty() {
                    if !value.is_empty() {
                        value.push(if *literal { '\n' } else { ' ' });
                    }
                    value.push_str(line.trim());
                }
            }
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            current = None;
            continue;
        };
        let value = value.trim();
        let (value, literal) = match value {
            ">" | ">-" | ">+" => ("", false),
            "|" | "|-" | "|+" => ("", true),
            v => (v, false),
        };
        current = Some(key.trim());
        keys.insert(key.trim(), (value.to_string(), literal));
    }
    Err("SKILL.md's front matter is never closed (a second --- line)".into())
}

/// Why this SKILL.md would not do for a skill of this name, if it would not.
fn check_skill(name: &str, text: &str) -> Result<String, String> {
    check_name(name)?;
    let (n, description) = front_matter(text)?;
    match n.as_deref() {
        None | Some("") => return Err("The front matter has no name".into()),
        Some(n) if n != name => return Err(format!("The front matter says name: {n}, the folder is {name}")),
        _ => {}
    }
    let description = description.filter(|d| !d.is_empty()).ok_or("The front matter has no description")?;
    if description.chars().count() > 1024 {
        return Err("The description is longer than 1024 characters".into());
    }
    Ok(description)
}

/// Every skill in the library, by name, with its files when it is fit to be
/// laid out. A `skills/` that will not list is an error, not an empty one: a
/// skill missed reads as taken out of the library, and its copies with it.
fn read_skills(dirs: &Dirs) -> Result<Skills, String> {
    let mut out = BTreeMap::new();
    let failed = |e: std::io::Error| format!("{}: {e}", dirs.skills().display());
    let entries = match std::fs::read_dir(dirs.skills()) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(failed(e)),
    };
    for entry in entries {
        let entry = entry.map_err(failed)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || !entry.file_type().map_err(failed)?.is_dir() {
            continue;
        }
        let files = read_dir_files(&entry.path());
        let checked = files.as_ref().map_err(String::clone).and_then(|f| {
            let text = f.get(SKILL_FILE).ok_or("There is no SKILL.md")?;
            check_skill(&name, &String::from_utf8_lossy(text))
        });
        let (description, problem) = match &checked {
            Ok(d) => (d.clone(), None),
            Err(e) => (String::new(), Some(e.clone())),
        };
        let files = checked.and(files);
        out.insert(name.clone(), (SkillInfo { name, description, problem, accounts: Vec::new() }, files));
    }
    Ok(out)
}

// ----------------------------------------------------------- instructions --

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct InstructionsInfo {
    pub name: String,
    /// Its first heading, to tell it by (`instructions_title`); empty without one.
    pub description: String,
    /// Why it is laid out nowhere, while it is not fit to be.
    pub problem: Option<String>,
    /// The accounts its reach takes in; each gets it unless a closer reach
    /// gives it other instructions.
    pub accounts: Reach,
}

/// Every instructions file in the library, by name. A folder that will not
/// list is an error, as `skills/` is; so is the old `instructions.md` not
/// moved into it yet: read as no instructions, either would take their
/// copies out.
fn read_instructions(dirs: &Dirs) -> Result<Instructions, String> {
    let old = dirs.root.join(OLD_INSTRUCTIONS);
    if old.is_file() && !dirs.instructions_file(crate::settings::FIRST_INSTRUCTIONS).exists() {
        return Err(format!("{} is yet to move into {}", old.display(), dirs.instructions().display()));
    }
    let mut out = BTreeMap::new();
    let failed = |e: std::io::Error| format!("{}: {e}", dirs.instructions().display());
    let entries = match std::fs::read_dir(dirs.instructions()) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(failed(e)),
    };
    for entry in entries {
        let entry = entry.map_err(failed)?;
        let file = entry.file_name().to_string_lossy().into_owned();
        let Some(name) = file.strip_suffix(".md") else { continue };
        if name.starts_with('.') || entry.file_type().map_err(failed)?.is_dir() {
            continue;
        }
        let text = check_name(name).and_then(|()| std::fs::read(entry.path()).map_err(|e| format!("{}: {e}", entry.path().display())));
        out.insert(name.to_string(), text.map(|t| (!String::from_utf8_lossy(&t).trim().is_empty()).then_some(t)));
    }
    Ok(out)
}

/// What tells instructions apart in the window: their first `#` heading, or
/// the first `##` where they have none — the names their author gave them.
/// Without either, nothing: a line of prose cut off mid-sentence, or one
/// section's heading, would claim to say what the whole file is about.
/// Lines in a fenced block are code, so a shell comment there is no heading.
fn instructions_title(text: &str) -> String {
    let mut fenced = false;
    let mut second = None;
    for line in text.lines().map(str::trim) {
        if line.starts_with("```") || line.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        let level = line.chars().take_while(|&c| c == '#').count();
        let rest = &line[level..];
        // `#tag` and `###` are not what the file is called.
        if fenced || !(1..=2).contains(&level) || !(rest.is_empty() || rest.starts_with([' ', '\t'])) {
            continue;
        }
        let title = rest.trim();
        if title.is_empty() {
            continue;
        }
        if level == 1 {
            return title.to_string();
        }
        second.get_or_insert_with(|| title.to_string());
    }
    second.unwrap_or_default()
}

/// The window's line for instructions: what tells them apart, or what is
/// wrong with them.
fn instructions_info(name: &str, text: &Result<Option<Vec<u8>>, String>, lib: &LibrarySettings) -> InstructionsInfo {
    let (description, problem) = match text {
        Ok(Some(t)) => (instructions_title(&String::from_utf8_lossy(t)), None),
        Ok(None) => (String::new(), Some("Blank: laid out nowhere until it has something in it".to_string())),
        Err(e) => (String::new(), Some(e.clone())),
    };
    InstructionsInfo { name: name.to_string(), description, problem, accounts: lib.instructions.get(name).cloned().unwrap_or_default() }
}

/// Moves the one instructions file of old, `instructions.md`, to
/// `instructions/main.md`, the name settings already read its reach under.
/// Once moved there is nothing left to move. With both there — one put back
/// by hand, a library shared with an older Luna — neither is touched; the
/// old one stays, and the window says so.
fn migrate_instructions(dirs: &Dirs) {
    let old = dirs.root.join(OLD_INSTRUCTIONS);
    let new = dirs.instructions_file(crate::settings::FIRST_INSTRUCTIONS);
    if !old.is_file() || std::fs::symlink_metadata(&new).is_ok() {
        return;
    }
    // A rename within the library: the file is at one name or the other at
    // every moment, never at neither.
    match std::fs::create_dir_all(dirs.instructions()).and_then(|()| std::fs::rename(&old, &new)) {
        Ok(()) => crate::log::info("library", &format!("moved {} to {}", old.display(), new.display())),
        Err(e) => crate::log::warn("library", &format!("could not move {} to {}: {e}", old.display(), new.display())),
    }
}

/// A name for instructions taken from an account: the account's, made one
/// `check_name` takes, and none the library has yet.
fn new_instructions_name(dirs: &Dirs, account: &str) -> String {
    let base: String = account.to_lowercase().chars().map(|c| if c.is_ascii_lowercase() || c.is_ascii_digit() { c } else { '-' }).collect();
    let base: String = base.trim_start_matches('-').chars().take(56).collect();
    let base = if base.is_empty() { "instructions".to_string() } else { base };
    let free = |n: &str| std::fs::symlink_metadata(dirs.instructions_file(n)).is_err();
    if free(&base) {
        return base;
    }
    (2..).map(|i| format!("{base}-{i}")).find(|n| free(n)).unwrap_or(base)
}

// ------------------------------------------------------------------ files --

fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn hashes(files: &Files) -> Hashes {
    files.iter().map(|(k, v)| (k.clone(), hash(v))).collect()
}

fn join_rel(base: &Path, rel: &str) -> PathBuf {
    rel.split('/').filter(|s| !s.is_empty()).fold(base.to_path_buf(), |p, s| p.join(s))
}

/// Every file under `dir`, by relative path, `write_file`'s temp files aside.
/// A link anywhere in it is an error: Luna neither follows one nor writes
/// over it.
fn read_dir_files(dir: &Path) -> Result<Files, String> {
    fn walk(dir: &Path, prefix: &str, out: &mut Files) -> Result<(), String> {
        let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let rel = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if kind.is_symlink() {
                return Err(format!("{} is a link", entry.path().display()));
            } else if kind.is_dir() {
                walk(&entry.path(), &rel, out)?;
            } else if name.ends_with(TMP_SUFFIX) {
                continue;
            } else {
                out.insert(rel, std::fs::read(entry.path()).map_err(|e| format!("{}: {e}", entry.path().display()))?);
            }
        }
        Ok(())
    }
    let mut out = Files::new();
    walk(dir, "", &mut out)?;
    Ok(out)
}

/// The end of `write_file`'s temp names. No file so named is read as part of
/// a skill or a copy: a skill's own file of the name would be written over by
/// a write beside it, and one left by a write that died taken for the skill's.
const TMP_SUFFIX: &str = ".luna-tmp";

/// Through a temp file beside it, so a reader never sees half. The temp name
/// is this write's own (process and count), so two writers never share one.
fn write_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}-{n}{TMP_SUFFIX}", std::process::id()));
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{}: {e}", path.display())
    })?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{}: {e}", path.display())
    })
}

/// Folders under `dir` that hold nothing any more, deepest first.
fn prune_empty(dir: &Path) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                prune_empty(&entry.path());
                let _ = std::fs::remove_dir(entry.path());
            }
        }
    }
}

/// What a destination holds now: None when nothing is there. A link, or a
/// folder where a file belongs (and the other way round), is not Luna's to
/// read or write.
fn read_at(dest: &Path, file: bool) -> Result<Option<Files>, String> {
    let meta = match std::fs::symlink_metadata(dest) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", dest.display())),
    };
    if meta.file_type().is_symlink() {
        return Err(format!("{} is a link — Luna leaves it alone", dest.display()));
    }
    match (file, meta.is_dir()) {
        (true, false) => Ok(Some(Files::from([(String::new(), std::fs::read(dest).map_err(|e| format!("{}: {e}", dest.display()))?)]))),
        (false, true) => read_dir_files(dest).map(Some),
        (true, true) => Err(format!("{} is a folder", dest.display())),
        (false, false) => Err(format!("{} is a file, not a skill's folder", dest.display())),
    }
}

/// Makes a destination hold exactly `files`: what differs is written, what is
/// there beyond them goes, and folders that leaves empty go too. Only ever on
/// Luna's own copy, or with the user's word and a backup taken.
fn write_at(dest: &Path, file: bool, files: &Files) -> Result<(), String> {
    if file {
        return write_file(dest, files.get("").map(Vec::as_slice).unwrap_or_default());
    }
    std::fs::create_dir_all(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    let old = read_dir_files(dest)?;
    // What goes, goes first: a file renamed only in case (`guide.md` to
    // `GUIDE.md`) is one file where case is ignored, which a write and then
    // the old name's removal would leave with neither.
    for rel in old.keys().filter(|r| !files.contains_key(*r)) {
        let p = join_rel(dest, rel);
        std::fs::remove_file(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    prune_empty(dest);
    for (rel, bytes) in files {
        if old.get(rel) != Some(bytes) {
            write_file(&join_rel(dest, rel), bytes)?;
        }
    }
    Ok(())
}

fn remove_at(dest: &Path, file: bool) -> Result<(), String> {
    let r = if file { std::fs::remove_file(dest) } else { std::fs::remove_dir_all(dest) };
    r.map_err(|e| format!("{}: {e}", dest.display()))
}

/// Whether a ledger entry is a single file: it has the one key "".
fn is_file(h: &Hashes) -> bool {
    h.len() == 1 && h.contains_key("")
}

// ------------------------------------------------------- blobs and backups --

/// The bytes of every file Luna writes, by hash: what a copy was when Luna
/// wrote it, for the differences shown once it has changed.
fn keep_blobs(dirs: &Dirs, files: &Files) {
    for bytes in files.values() {
        let path = dirs.blobs().join(hash(bytes));
        if !path.exists() {
            let _ = write_file(&path, bytes);
        }
    }
}

fn blob(dirs: &Dirs, h: &str) -> Option<Vec<u8>> {
    std::fs::read(dirs.blobs().join(h)).ok()
}

/// Blobs no ledger entry names any more.
fn sweep_blobs(dirs: &Dirs, ledger: &Ledger) {
    let wanted: BTreeSet<&String> = ledger.written.values().flat_map(|h| h.values()).collect();
    if let Ok(entries) = std::fs::read_dir(dirs.blobs()) {
        for entry in entries.flatten() {
            if !wanted.contains(&entry.file_name().to_string_lossy().into_owned()) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// Puts `files` — what is about to be replaced at `from` — under a folder of
/// its own in the backups, named for the moment and the last two parts of the
/// path (the whole path would outgrow Windows' limit): the file, or the folder
/// with all in it, by its own name, and the whole path beside it in `from.txt`,
/// where no file of a skill's own can be.
fn back_up(dirs: &Dirs, from: &Path, files: &Files) -> Result<PathBuf, String> {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S-%3f");
    let tail: Vec<String> = from.components().rev().take(2).map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    let tail: String = tail.into_iter().rev().collect::<Vec<_>>().join("-").chars().filter(|c| !matches!(c, ':' | '\\' | '/')).collect();
    let dir = dirs.backups().join(format!("{stamp}-{tail}"));
    let saved = dir.join(from.file_name().unwrap_or_default());
    for (rel, bytes) in files {
        let path = if rel.is_empty() { saved.clone() } else { join_rel(&saved, rel) };
        write_file(&path, bytes)?;
    }
    write_file(&dir.join("from.txt"), from.to_string_lossy().as_bytes())?;
    Ok(dir)
}

fn sweep_backups(dirs: &Dirs) {
    let limit = std::time::Duration::from_secs(BACKUP_DAYS * 86_400);
    if let Ok(entries) = std::fs::read_dir(dirs.backups()) {
        for entry in entries.flatten() {
            let old = entry.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > limit);
            if old {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
}

// ---------------------------------------------------------------- folders --

/// Each folder of the library with the name its skills wait under: the one it
/// keeps (`LibraryFolder::slug`), unless an earlier folder keeps that too, else
/// one made for it (`fresh_slug`) past those kept. A list with none kept, as
/// before they were, gets the names it always had: by its order.
fn folder_slugs(folders: &[LibraryFolder]) -> Vec<(&LibraryFolder, String)> {
    let mut taken = BTreeSet::new();
    let kept: Vec<bool> = folders.iter().map(|f| slug_ok(&f.slug) && taken.insert(f.slug.clone())).collect();
    folders
        .iter()
        .zip(kept)
        .map(|(f, kept)| {
            if kept {
                return (f, f.slug.clone());
            }
            let slug = fresh_slug(&f.path, &taken);
            taken.insert(slug.clone());
            (f, slug)
        })
        .collect()
}

/// A name for the folder at `path` to keep its skills under: its own name as
/// `check_name` takes names — the prefix Codex lists them by, and a part of a
/// `-c` key, which Codex splits at every dot — with `-2`, `-3` after one taken.
fn fresh_slug(path: &str, taken: &BTreeSet<String>) -> String {
    let own = Path::new(path).file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    let base: String = own.chars().map(|c| if c.is_ascii_lowercase() || c.is_ascii_digit() { c } else { '-' }).collect();
    let base: String = base.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-").chars().take(56).collect();
    let base = if base.is_empty() { "folder".to_string() } else { base.trim_end_matches('-').to_string() };
    std::iter::once(base.clone()).chain((2..).map(|i| format!("{base}-{i}"))).find(|s| !taken.contains(s)).unwrap_or(base)
}

/// Whether a kept slug is one `fresh_slug` could have made: a settings file
/// edited by hand is not to put a dot or a path in a `-c` key.
fn slug_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && !s.starts_with('-')
        && !s.ends_with('-')
        && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Every folder keeps the name it goes by now, so that none passes to another
/// as an earlier one goes.
fn settle_slugs(lib: &mut LibrarySettings) {
    let slugs: Vec<String> = folder_slugs(&lib.folders).into_iter().map(|(_, s)| s).collect();
    for (f, slug) in lib.folders.iter_mut().zip(slugs) {
        f.slug = slug;
    }
}

/// The names folders' skills wait under on disk: `library-folders` and each
/// Codex account's marketplace. A folder gone may have left copies changed
/// since there, which stay: a new folder is not to take them up as its own.
fn slugs_on_disk(dirs: &Dirs) -> BTreeSet<String> {
    let names = |dir: PathBuf| -> Vec<String> {
        std::fs::read_dir(dir).map(|e| e.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default()
    };
    let mut out: BTreeSet<String> = names(dirs.folders()).into_iter().collect();
    for a in crate::accounts::list_accounts().unwrap_or_default().iter().filter(|a| a.provider == Provider::Codex) {
        out.extend(names(codex_market_dir(a)));
    }
    out
}

/// Where a folder's skills for one Claude Code account wait: the folder a
/// session there is given with `--add-dir`, its `.claude/skills` listed as the
/// session's own.
fn claude_folder_dir(dirs: &Dirs, slug: &str, a: &AccountInfo) -> PathBuf {
    dirs.folders().join(slug).join(&a.name)
}

/// Luna's marketplace folder in a Codex account's plugin cache.
fn codex_market_dir(a: &AccountInfo) -> PathBuf {
    Path::new(&a.path).join("plugins").join("cache").join(CODEX_MARKET)
}

/// A folder's plugin in a Codex account: its version folder, `local` — the
/// one Codex takes over any version — holding the manifest and `skills/`.
fn codex_plugin_dir(a: &AccountInfo, slug: &str) -> PathBuf {
    codex_market_dir(a).join(slug).join("local")
}

/// The names of the skills a Claude Code session in `folder` on this account
/// sees besides the library's: the `.claude/skills` of the folder and of each
/// folder up to the repository's root (where Claude Code stops looking), the
/// account's `skills/` but for Luna's own copies there — the skill itself,
/// which a folder then gives to no session of the account, or one on its way
/// out — and the skills it syncs from claude.ai there.
fn claude_names_taken(folder: &str, a: &AccountInfo, ledger: &Ledger) -> BTreeSet<String> {
    let names = |dir: PathBuf| -> Vec<String> {
        std::fs::read_dir(dir).map(|e| e.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default()
    };
    let mut out = BTreeSet::new();
    for dir in Path::new(folder).ancestors() {
        out.extend(names(dir.join(".claude").join("skills")));
        if dir.join(".git").exists() {
            break;
        }
    }
    let skills = Path::new(&a.path).join("skills");
    for synced in names(skills.join("synced")) {
        out.extend(names(skills.join("synced").join(synced)));
    }
    let lunas: BTreeSet<String> = ledger.written.keys().map(|k| ident(Path::new(k))).collect();
    out.extend(names(skills.clone()).into_iter().filter(|n| !lunas.contains(&ident(&skills.join(n)))));
    out
}

/// The names a folder's Claude Code copies on this account go by where a
/// skill a session there sees has theirs (`free_names`); `going` is every
/// skill the folder gives the account, as each takes a name from the rest.
fn claude_shown_names(f: &LibraryFolder, a: &AccountInfo, ledger: &Ledger, going: &[&str]) -> BTreeMap<String, String> {
    free_names(going, &claude_names_taken(&f.path, a, ledger))
}

/// Whether the account has the skill as a whole, in its own `skills/`.
fn account_copy_for(lib: &LibrarySettings, name: &str, a: &AccountInfo) -> bool {
    lib.skills.get(name).is_some_and(|reach| reaches(reach, a))
}

/// Whether a folder gives its sessions on the account a copy of the skill of
/// their own: where it gives the skill to the account, unless the account
/// has it as a whole already — twice over, Claude Code would list the second
/// copy scoped to Luna's folder — or a folder this one lies in gives it a
/// copy: every session here lies in that one too, and would list both (Codex
/// as `<outer>:<name>` and `<inner>:<name>`).
fn folder_copy_for(lib: &LibrarySettings, f: &LibraryFolder, name: &str, a: &AccountInfo) -> bool {
    let gives = |g: &LibraryFolder| g.skills.get(name).is_some_and(|reach| reaches(reach, a));
    let outer =
        |g: &&LibraryFolder| ident(Path::new(&g.path)) != ident(Path::new(&f.path)) && within(Path::new(&f.path), Path::new(&g.path));
    gives(f) && !account_copy_for(lib, name, a) && !lib.folders.iter().filter(outer).any(gives)
}

/// What the folders a session's work lies in say of a skill: the accounts
/// whose sessions get it there, or nothing (None) — and then it goes by what
/// the skill is meant for. The deepest of them that names it decides, so one
/// inside another can take back what the outer one says.
fn deciding<'a>(lies_in: impl IntoIterator<Item = &'a LibraryFolder>, name: &str) -> Option<&'a Reach> {
    lies_in
        .into_iter()
        .filter(|f| f.skills.contains_key(name))
        .max_by_key(|f| ident(Path::new(&f.path)).len())
        .and_then(|f| f.skills.get(name))
}

/// What the folders a session's work lies in say of its account's
/// instructions: other instructions by name, none (`Some(None)`), or nothing
/// (None) — and then it has its account's. The deepest folder with an entry
/// that takes the account in decides, by its closest entry.
fn folder_instructions<'a>(lies_in: impl IntoIterator<Item = &'a LibraryFolder>, a: &AccountInfo) -> Option<Option<&'a str>> {
    lies_in
        .into_iter()
        .filter_map(|f| {
            let (_, choice) = f
                .instructions
                .iter()
                .filter_map(|(entry, choice)| Some((closeness(std::slice::from_ref(entry), a)?, choice)))
                .max_by_key(|(c, _)| *c)?;
            Some((ident(Path::new(&f.path)).len(), choice.as_deref()))
        })
        .max_by_key(|(depth, _)| *depth)
        .map(|(_, choice)| choice)
}

/// Whether the sessions on an account whose work lies in these folders get
/// the skill.
fn gets_in<'a>(lib: &LibrarySettings, lies_in: impl IntoIterator<Item = &'a LibraryFolder>, name: &str, a: &AccountInfo) -> bool {
    match deciding(lies_in, name) {
        Some(reach) => reaches(reach, a),
        None => account_copy_for(lib, name, a),
    }
}

/// The name each of `ours` goes by where `taken` has its own: `luna-<name>`,
/// or with `-2`, `-3` after it where that is taken too — by another skill
/// there or one of ours. The names that stay as they are are not in it.
fn free_names(ours: &[&str], taken: &BTreeSet<String>) -> BTreeMap<String, String> {
    let mut used: BTreeSet<String> = ours.iter().map(|n| n.to_string()).chain(taken.iter().cloned()).collect();
    let mut out = BTreeMap::new();
    for name in ours.iter().filter(|n| taken.contains(**n)) {
        let base: String = format!("{RENAMED}{name}").chars().take(60).collect();
        let base = base.trim_end_matches('-').to_string();
        let Some(free) = std::iter::once(base.clone()).chain((2..100).map(|i| format!("{base}-{i}"))).find(|c| !used.contains(c)) else {
            continue;
        };
        used.insert(free.clone());
        out.insert(name.to_string(), free);
    }
    out
}

/// A skill's files with `name:` in its SKILL.md's front matter made `name` —
/// a folder's copy under another name, which both CLIs want the same as its
/// folder's. Nothing else in the file changes.
fn with_name(mut files: Files, name: &str) -> Files {
    if let Some(bytes) = files.get_mut(SKILL_FILE) {
        let text = String::from_utf8_lossy(bytes).into_owned();
        let mut out = String::with_capacity(text.len() + name.len());
        let mut front = false;
        let mut done = false;
        for (i, line) in text.split_inclusive('\n').enumerate() {
            let bare = line.trim_end_matches(['\r', '\n']);
            if i == 0 {
                front = bare.trim_start_matches('\u{feff}').trim_end() == "---";
            } else if front && !done {
                if bare.trim_end() == "---" {
                    front = false;
                } else if !line.starts_with([' ', '\t']) && bare.split_once(':').is_some_and(|(k, _)| k.trim() == "name") {
                    out.push_str(&format!("name: {name}{}", &line[bare.len()..]));
                    done = true;
                    continue;
                }
            }
            out.push_str(line);
        }
        *bytes = out.into_bytes();
    }
    files
}

/// The manifest of each folder's plugin in each Codex account: there while
/// the plugin's `skills/` holds a copy — Codex loads a plugin by it — and
/// gone, with the folders that leaves empty, once none does. Luna's own file
/// in Luna's own marketplace folder: not a copy of anything, so neither noted
/// nor asked about.
fn codex_manifests(lib: &LibrarySettings, accounts: &[AccountInfo]) {
    let slugs: BTreeMap<String, &str> = folder_slugs(&lib.folders).into_iter().map(|(f, s)| (s, f.path.as_str())).collect();
    for a in accounts.iter().filter(|a| a.provider == Provider::Codex) {
        let market = codex_market_dir(a);
        let mut names: BTreeSet<String> = slugs.keys().cloned().collect();
        if let Ok(entries) = std::fs::read_dir(&market) {
            names.extend(entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()));
        }
        for slug in names {
            let plugin = codex_plugin_dir(a, &slug);
            let manifest = plugin.join(".codex-plugin").join("plugin.json");
            // Luna's own folders, and none that leads elsewhere: through a
            // link on the way the manifest would be written, or taken out,
            // wherever that leads.
            let way = [market.clone(), market.join(&slug), plugin.clone(), plugin.join(".codex-plugin"), manifest.clone()];
            if let Some(link) = way.iter().find(|p| std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink())) {
                if link_is_news(link) {
                    crate::log::warn("library", &format!("{} is a link — Luna leaves the plugin there alone", link.display()));
                }
                continue;
            }
            let holds = std::fs::read_dir(plugin.join("skills")).is_ok_and(|mut e| e.next().is_some());
            if holds {
                // A folder no longer listed holds only copies changed since,
                // left in place; its manifest stays as it was.
                let Some(path) = slugs.get(&slug) else { continue };
                let manifest_json = serde_json::json!({
                    "name": slug,
                    "description": format!("Skills Luna's library gives sessions in {path}"),
                    "skills": "./skills/",
                });
                let text = serde_json::to_string_pretty(&manifest_json).unwrap_or_default() + "\n";
                if std::fs::read(&manifest).ok().as_deref() != Some(text.as_bytes()) {
                    if let Err(e) = write_file(&manifest, text.as_bytes()) {
                        crate::log::warn("library", &format!("could not write {}: {e}", manifest.display()));
                    }
                }
            } else {
                let _ = std::fs::remove_file(&manifest);
                for dir in [plugin.join("skills"), plugin.join(".codex-plugin"), plugin.clone(), market.join(&slug), market.clone()] {
                    let _ = std::fs::remove_dir(dir);
                }
            }
        }
    }
}

/// What `library-folders` holds for folders no longer listed, once nothing
/// is left in it: copies changed since stay, and so do the folders around
/// them. A listed folder's are kept even empty, for the sessions given them.
fn prune_folder_dirs(dirs: &Dirs, lib: &LibrarySettings) {
    let listed: BTreeSet<String> = folder_slugs(&lib.folders).into_iter().map(|(_, s)| s).collect();
    if let Ok(entries) = std::fs::read_dir(dirs.folders()) {
        for entry in entries.flatten().filter(|e| !listed.contains(&*e.file_name().to_string_lossy())) {
            prune_empty(&entry.path());
            let _ = std::fs::remove_dir(entry.path());
        }
    }
}

/// What a session Luna starts gets of the library's folders.
#[derive(Default, Debug, PartialEq)]
pub struct SessionSkills {
    /// For its CLI: Claude Code an `--add-dir` per folder, Codex a `-c` per
    /// folder's plugin and one for the skills its folders keep from it.
    pub args: Vec<String>,
    /// Claude Code: the skills it would list that its folders keep from it,
    /// by the names it lists them under — `skillOverrides` in the settings
    /// it is started with (pty.rs `claude_settings_file`), the one way to
    /// keep a skill from a session that its account has as a whole.
    pub hidden: Vec<String>,
    /// Claude Code: the account's instructions file, where a folder the
    /// session lies in gives it other instructions or none —
    /// `claudeMdExcludes` in the same settings. Codex has no way to leave its
    /// account's `AGENTS.md` out of one session, so a folder says nothing to
    /// it of instructions.
    pub excluded: Vec<String>,
    /// Claude Code: the library's file of the other instructions, which its
    /// caller gives the session as a `CLAUDE.md` of its own
    /// (pty.rs `claude_instructions_dir`).
    pub instructions: Option<PathBuf>,
}

/// What a session Luna starts gets of the library's folders, for each folder
/// its work lies in — the folder it starts in, or the project a worktree of
/// it belongs to: Claude Code an `--add-dir` of the skills waiting there for
/// this account, Codex a `-c` turning that folder's plugin on where the
/// account has one; and, from either, the skills those folders keep from it
/// (`deciding`) hidden. Due at every start, resumes and each helper turn
/// included: one started without them keeps the skills named in its
/// conversation, and none of them answers. Its caller lays the library out
/// first (`lay_out`), so the session reads its copies as the library has
/// them now — off any lock of the caller's, as a pass reads and writes every
/// account's copies.
pub fn session_args(provider: Provider, account_path: &str, folder: &str) -> SessionSkills {
    let lib = crate::settings::get().library;
    if lib.folders.is_empty() {
        return SessionSkills::default();
    }
    let account = crate::accounts::list_accounts()
        .unwrap_or_default()
        .into_iter()
        .find(|a| a.provider == provider && crate::pty::same_dir(&a.path, account_path));
    match account {
        Some(a) => folder_args(&Dirs::current(), &lib, &a, folder),
        None => SessionSkills::default(),
    }
}

/// The library's folders a session's work lies in: those the folder it starts
/// in lies in — in a worktree, the same folder of the project it belongs to
/// (`<worktree>/src` is `<project>/src`), wherever the worktree itself lies.
fn folders_of<'a>(lib: &'a LibrarySettings, folder: &str) -> Vec<(&'a LibraryFolder, String)> {
    let work = crate::worktree::root_of(folder)
        .and_then(|(root, project)| Path::new(folder).strip_prefix(&root).ok().map(|rel| Path::new(&project).join(rel)))
        .unwrap_or_else(|| PathBuf::from(folder));
    folder_slugs(&lib.folders).into_iter().filter(|(f, _)| within(&work, Path::new(&f.path))).collect()
}

fn folder_args(dirs: &Dirs, lib: &LibrarySettings, a: &AccountInfo, folder: &str) -> SessionSkills {
    let lies_in = folders_of(lib, folder);
    let mut out = SessionSkills::default();
    for (f, slug) in &lies_in {
        match a.provider {
            Provider::Claude => {
                // Made if it is not there yet, skills or none: a session
                // started without its `.claude/skills` never sees one added
                // to it later.
                let dir = claude_folder_dir(dirs, slug, a);
                match std::fs::create_dir_all(dir.join(".claude").join("skills")) {
                    Ok(()) => out.args.extend(["--add-dir".to_string(), dir.to_string_lossy().into_owned()]),
                    Err(e) => crate::log::warn("library", &format!("could not make {} for {}: {e}", dir.display(), f.path)),
                }
            }
            Provider::Codex => {
                if codex_plugin_dir(a, slug).join(".codex-plugin").join("plugin.json").is_file() {
                    out.args.extend(["-c".to_string(), format!("plugins.{slug}@{CODEX_MARKET}.enabled=true")]);
                }
            }
        }
    }
    if a.provider == Provider::Claude {
        if let Some(choice) = folder_instructions(lies_in.iter().map(|(f, _)| *f), a) {
            let own = Path::new(&a.path).join(instructions_name(a.provider));
            // Instructions the library no longer has are none.
            let text = choice.and_then(|n| std::fs::read(dirs.instructions_file(n)).ok());
            // What the account's file holds already stays; anything else —
            // none at all, or a copy of the account's changed there — leaves
            // out whatever it holds, the library's or not.
            let there = text.as_ref().is_some_and(|t| std::fs::read(&own).is_ok_and(|o| o == *t));
            if !there {
                out.excluded.push(own.to_string_lossy().replace('\\', "/"));
                out.instructions = choice.filter(|_| text.as_ref().is_some_and(|t| !t.is_empty())).map(|n| dirs.instructions_file(n));
            }
        }
    }
    let hidden = hidden_here(dirs, lib, a, &lies_in);
    match a.provider {
        Provider::Claude => out.hidden = hidden,
        // One `-c` for them all: a second on the key would take the first's
        // place. Codex adds its entries to the account's own `skills.config`
        // ones, selector by selector, rather than putting them in their place.
        Provider::Codex if !hidden.is_empty() => {
            let entries: Vec<String> = hidden.iter().map(|n| format!("{{name=\"{n}\",enabled=false}}")).collect();
            out.args.extend(["-c".to_string(), format!("skills.config=[{}]", entries.join(","))]);
        }
        Provider::Codex => {}
    }
    out
}

/// The skills a session on this account would see that the folders it lies
/// in keep from it, by the names its CLI lists them under: the account's own
/// copy by the skill's name; a copy an outer folder gives, where an inner one
/// keeps it from this account, by its Claude Code name (`luna-<name>` where
/// it was renamed) or as Codex's `<folder>:<name>`. Both CLIs hide a skill by
/// name, so one of the same name the repository has goes too.
fn hidden_here(dirs: &Dirs, lib: &LibrarySettings, a: &AccountInfo, lies_in: &[(&LibraryFolder, String)]) -> Vec<String> {
    let named: BTreeSet<&String> = lib.skills.keys().chain(lies_in.iter().flat_map(|(f, _)| f.skills.keys())).collect();
    let mut out = BTreeSet::new();
    // Read only for a Claude Code copy an outer folder gives, whose name the
    // layout may have changed.
    let mut ledger = None;
    for name in named {
        if gets_in(lib, lies_in.iter().map(|(f, _)| *f), name, a) {
            continue;
        }
        if account_copy_for(lib, name, a) {
            out.insert(name.clone());
        }
        for (f, slug) in lies_in.iter().filter(|(f, _)| folder_copy_for(lib, f, name, a)) {
            match a.provider {
                Provider::Claude => {
                    let ledger = ledger.get_or_insert_with(|| Ledger::load(dirs));
                    // As `units` takes them: the skills the library has.
                    let going: Vec<&str> = f
                        .skills
                        .keys()
                        .filter(|n| folder_copy_for(lib, f, n, a) && dirs.skills().join(n).is_dir())
                        .map(String::as_str)
                        .collect();
                    let shown = claude_shown_names(f, a, ledger, &going);
                    out.insert(shown.get(name.as_str()).cloned().unwrap_or_else(|| name.clone()));
                }
                Provider::Codex => {
                    out.insert(format!("{slug}:{name}"));
                }
            }
        }
    }
    out.into_iter().collect()
}

// ------------------------------------------------------------------- pass --

/// One copy the library wants somewhere.
struct Unit {
    /// The instructions file itself, or the skill's folder.
    dest: PathBuf,
    file: bool,
    /// None for instructions.
    skill: Option<String>,
    /// Which instructions, for instructions.
    instructions: Option<String>,
    /// The name a folder's copy goes by where the skill's own is taken
    /// (`free_names`); its `source` is then the skill under that name.
    shown: Option<String>,
    target: Target,
    /// What the library has for it, or why it has nothing fit to lay out.
    source: Result<Files, String>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Target {
    Account {
        provider: Provider,
        name: String,
    },
    /// What the sessions in a folder get on one account.
    Folder {
        path: String,
        provider: Provider,
        name: String,
    },
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    /// The same as the library.
    Current,
    /// Luna's copy, behind the library; a pass brings it up to date.
    Behind,
    /// Not there yet; a pass puts it there.
    Missing,
    /// Changed there since Luna wrote it.
    Changed,
    /// Deleted there since Luna wrote it.
    Gone,
    /// Something Luna did not write is in its place.
    Taken,
    /// Left out on purpose.
    Skipped,
    /// Cannot be laid out: `problem` says why.
    Failed,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Place {
    pub dest: String,
    pub target: Target,
    pub skill: Option<String>,
    pub instructions: Option<String>,
    /// The name the copy goes by, where it is not the skill's own.
    pub shown_as: Option<String>,
    pub status: Status,
    /// `changed`, with the library changed too since Luna wrote it.
    pub both: bool,
    pub problem: Option<String>,
}

#[derive(Default)]
struct Pass {
    places: Vec<Place>,
    /// Copies no longer wanted, left where they are because they had changed.
    kept: Vec<String>,
}

/// Looks at every copy the library wants and, with `act`, does what needs no
/// asking: puts in what is missing, brings Luna's own copies up to the
/// library, adopts a destination that already holds exactly the library's,
/// and takes out the copies no longer wanted that are still as Luna wrote
/// them. Changed, deleted and foreign destinations are only reported.
fn pass(dirs: &Dirs, units: &[Unit], skipped: &[String], ledger: &mut Ledger, act: bool) -> Pass {
    let mut out = Pass::default();
    // Each copy wanted by `ident`, with its path as spelled now: two
    // spellings of one file are one copy.
    let mut wanted: BTreeMap<String, String> = BTreeMap::new();
    for u in units {
        let key = u.dest.to_string_lossy().into_owned();
        let id = ident(&u.dest);
        if wanted.contains_key(&id) {
            continue;
        }
        wanted.insert(id.clone(), key.clone());
        let place = |status, both, problem| Place {
            dest: key.clone(),
            target: u.target.clone(),
            skill: u.skill.clone(),
            instructions: u.instructions.clone(),
            shown_as: u.shown.clone(),
            status,
            both,
            problem,
        };
        if skipped.iter().any(|s| ident(Path::new(s)) == id) {
            out.places.push(place(Status::Skipped, false, None));
            continue;
        }
        // Noted under another spelling of the same path (an account renamed
        // in case only, on Windows): the same copy, noted as spelled now.
        if !ledger.written.contains_key(&key) {
            let alias = ledger.written.keys().find(|k| ident(Path::new(k)) == id).cloned();
            if let Some(h) = alias.and_then(|a| ledger.written.remove(&a)) {
                ledger.written.insert(key.clone(), h);
            }
        }
        let source = match &u.source {
            Ok(s) => s,
            Err(e) => {
                out.places.push(place(Status::Failed, false, Some(e.clone())));
                continue;
            }
        };
        let want = hashes(source);
        let write = |ledger: &mut Ledger| -> Result<(), String> {
            keep_blobs(dirs, source);
            if let Err(e) = write_at(&u.dest, u.file, source) {
                // Stopped partway (a file there held open, say): what is there
                // is still all Luna's, and noted as such it stays Luna's to
                // finish next time instead of reading as changed by someone.
                if let Ok(Some(now)) = read_at(&u.dest, u.file) {
                    ledger.written.insert(key.clone(), hashes(&now));
                }
                return Err(e);
            }
            ledger.written.insert(key.clone(), want.clone());
            Ok(())
        };
        let status = match read_at(&u.dest, u.file) {
            Err(e) => Err(e),
            Ok(None) if ledger.written.contains_key(&key) => Ok(Status::Gone),
            Ok(None) => Ok(Status::Missing),
            Ok(Some(have)) => {
                let have = hashes(&have);
                match ledger.written.get(&key) {
                    None if have == want => {
                        if act {
                            keep_blobs(dirs, source);
                            ledger.written.insert(key.clone(), want.clone());
                        }
                        Ok(Status::Current)
                    }
                    None => Ok(Status::Taken),
                    Some(w) if *w != have => {
                        out.places.push(place(Status::Changed, *w != want, None));
                        continue;
                    }
                    Some(_) if have == want => Ok(Status::Current),
                    Some(_) => Ok(Status::Behind),
                }
            }
        };
        let status = match status {
            Ok(Status::Missing | Status::Behind) if act => write(ledger).map(|()| Status::Current),
            other => other,
        };
        match status {
            Ok(s) => out.places.push(place(s, false, None)),
            Err(e) => out.places.push(place(Status::Failed, false, Some(e))),
        }
    }
    if act {
        let gone: Vec<String> = ledger.written.keys().filter(|k| wanted.get(&ident(Path::new(k))) != Some(*k)).cloned().collect();
        for key in gone {
            let h = ledger.written.remove(&key).unwrap_or_default();
            let dest = PathBuf::from(&key);
            // Another spelling of a copy still wanted is that copy's file:
            // only the note goes.
            if wanted.contains_key(&ident(&dest)) || in_library(dirs, &dest) {
                continue;
            }
            match read_at(&dest, is_file(&h)) {
                Ok(None) => {}
                Ok(Some(have)) if hashes(&have) == h => {
                    if let Err(e) = remove_at(&dest, is_file(&h)) {
                        crate::log::warn("library", &format!("could not take out {key}: {e}"));
                        ledger.written.insert(key, h);
                    }
                }
                _ => out.kept.push(key),
            }
        }
    }
    out
}

/// Every copy the library wants, from the settings and what the library and
/// the accounts folder hold now; the ledger tells Luna's own copies there from
/// skills of the same name that are not.
fn units(
    dirs: &Dirs,
    lib: &LibrarySettings,
    accounts: &[AccountInfo],
    skills: &Result<Skills, String>,
    instructions: &Result<Instructions, String>,
    ledger: &Ledger,
) -> Vec<Unit> {
    let mut out = Vec::new();
    let account = |a: &AccountInfo| Target::Account { provider: a.provider, name: a.name.clone() };
    // A skill's files; while the skills will not list, every one a failure.
    let files_of = |name: &str| match skills {
        Ok(found) => found.get(name).map(|(_, files)| files.clone()),
        Err(e) => Some(Err(e.clone())),
    };
    // Only instructions the library has compete for an account; while they
    // will not list, every one named in the settings does, and fails.
    let have = |name: &str| instructions.as_ref().map_or(true, |found| found.contains_key(name));
    for a in accounts {
        let Some(name) = chosen(lib, have, a) else { continue };
        // Blank is no instructions; a file that will not read (held open
        // mid-save, say) is a failure like a broken skill's, and takes none
        // of its copies out.
        let source = match instructions.as_ref().map(|found| &found[name]) {
            Ok(Ok(None)) => continue,
            Ok(Ok(Some(text))) => Ok(Files::from([(String::new(), text.clone())])),
            Ok(Err(e)) | Err(e) => Err(e.clone()),
        };
        out.push(Unit {
            dest: Path::new(&a.path).join(instructions_name(a.provider)),
            file: true,
            skill: None,
            instructions: Some(name.to_string()),
            shown: None,
            target: account(a),
            source,
        });
    }
    for name in lib.skills.keys() {
        let Some(files) = files_of(name.as_str()) else { continue };
        for a in accounts.iter().filter(|a| account_copy_for(lib, name, a)) {
            out.push(Unit {
                dest: Path::new(&a.path).join("skills").join(name),
                file: false,
                skill: Some(name.clone()),
                instructions: None,
                shown: None,
                target: account(a),
                source: files.clone(),
            });
        }
    }
    for (f, slug) in folder_slugs(&lib.folders) {
        let there = Path::new(&f.path).is_dir();
        for a in accounts {
            let going: Vec<(&String, Result<Files, String>)> =
                f.skills.keys().filter(|n| folder_copy_for(lib, f, n, a)).filter_map(|n| Some((n, files_of(n)?))).collect();
            if going.is_empty() {
                continue;
            }
            let shown = match a.provider {
                Provider::Claude if there => claude_shown_names(f, a, ledger, &going.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>()),
                _ => BTreeMap::new(),
            };
            for (name, files) in going {
                let shown = shown.get(name.as_str()).cloned();
                let dest = match a.provider {
                    Provider::Claude => {
                        claude_folder_dir(dirs, &slug, a).join(".claude").join("skills").join(shown.as_deref().unwrap_or(name))
                    }
                    Provider::Codex => codex_plugin_dir(a, &slug).join("skills").join(name),
                };
                let source = match (there, &shown) {
                    (false, _) => Err(format!("{} is not there", f.path)),
                    (true, Some(s)) => files.map(|files| with_name(files, s)),
                    (true, None) => files,
                };
                out.push(Unit {
                    dest,
                    file: false,
                    skill: Some(name.clone()),
                    instructions: None,
                    shown,
                    target: Target::Folder { path: f.path.clone(), provider: a.provider, name: a.name.clone() },
                    source,
                });
            }
        }
    }
    for u in &mut out {
        if in_library(dirs, &u.dest) {
            u.source = Err(format!("{} is in the library itself", u.dest.display()));
        }
    }
    out
}

/// A path as the file system tells paths apart: `/`-separated, without a
/// trailing one, and on Windows — where either slash separates and case is
/// ignored — lower-case, so two spellings of one file come out the same.
fn ident(p: &Path) -> String {
    let s = p.to_string_lossy();
    if cfg!(windows) {
        s.replace('\\', "/").trim_end_matches('/').to_lowercase()
    } else {
        s.trim_end_matches('/').to_string()
    }
}

/// Whether `a` is `b` or inside it, per `ident`.
fn within(a: &Path, b: &Path) -> bool {
    let (a, b) = (ident(a), ident(b));
    a == b || a.strip_prefix(&b).is_some_and(|rest| rest.starts_with('/'))
}

/// A destination in the library's skills (a library pointed at an account
/// folder, which has its `skills/` where the library does), or one holding
/// the library: its copy would be the
/// source itself, adopted as Luna's and taken out — the library's only one —
/// the day it is no longer wanted there.
fn in_library(dirs: &Dirs, dest: &Path) -> bool {
    let skills = dirs.skills();
    if within(dest, &skills) || within(&dirs.root, dest) {
        return true;
    }
    // Reached through a link or junction (a project's `.agents/skills`
    // pointing at the library's) the spelling differs but the files do not.
    let (dest, skills, root) = (resolved(dest), resolved(&skills), resolved(&dirs.root));
    within(&dest, &skills) || within(&root, &dest)
}

/// `p` with the part of it that is there canonicalized, links and junctions
/// followed, and the rest put back on. Both sides of a comparison go through
/// it, so they carry the same form (`\\?\` on Windows).
pub(crate) fn resolved(p: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut at = p;
    loop {
        if let Ok(real) = std::fs::canonicalize(at) {
            return rest.iter().rev().fold(real, |acc, part| acc.join(part));
        }
        match (at.parent(), at.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name);
                at = parent;
            }
            _ => return p.to_path_buf(),
        }
    }
}

// --------------------------------------------------------------- overview --

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderInfo {
    pub path: String,
    /// The accounts whose sessions here get a skill, by the skills it says
    /// anything of.
    pub skills: BTreeMap<String, Reach>,
    /// The instructions its sessions get in place of their account's, by the
    /// reach entry they are for; None: none at all.
    pub instructions: BTreeMap<String, Option<String>>,
    pub found: bool,
    /// The accounts, as `provider/name`, whose sessions here would get each
    /// skill were the folder to say nothing of it (`inherited`); a skill no
    /// account's would get is not there.
    pub inherited: BTreeMap<String, Vec<String>>,
}

/// The accounts whose sessions in a folder would get a skill were it to say
/// nothing of it — by what the skill is meant for, or by the deepest folder
/// it lies in that says something.
fn inherited(lib: &LibrarySettings, accounts: &[AccountInfo], f: &LibraryFolder, name: &str) -> Vec<String> {
    let outer: Vec<&LibraryFolder> = lib
        .folders
        .iter()
        .filter(|g| ident(Path::new(&g.path)) != ident(Path::new(&f.path)) && within(Path::new(&f.path), Path::new(&g.path)))
        .collect();
    accounts
        .iter()
        .filter(|a| gets_in(lib, outer.iter().copied(), name, a))
        .map(|a| format!("{}/{}", a.provider.as_str(), a.name))
        .collect()
}

/// An account's instructions file Luna did not write, where no instructions of
/// the library go: something to start new instructions from.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FoundFile {
    pub dest: String,
    pub target: Target,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    pub root: String,
    pub is_default: bool,
    /// False while the library's folder is not there: nothing is laid out,
    /// and nothing taken out, until it is.
    pub found: bool,
    pub instructions: Vec<InstructionsInfo>,
    /// The accounts that get no instructions however others reach them.
    pub no_instructions: Reach,
    /// The instructions each account gets, by `provider/name`; an account
    /// that gets none is not there.
    pub instructions_for: BTreeMap<String, String>,
    /// The `instructions.md` of before there could be several, still in the
    /// library beside `instructions/main.md`: nothing is laid out from it.
    pub old_instructions: Option<String>,
    pub skills: Vec<SkillInfo>,
    /// Every account, to pick a reach from.
    pub accounts: Vec<AccountInfo>,
    pub folders: Vec<FolderInfo>,
    pub places: Vec<Place>,
    pub found_instructions: Vec<FoundFile>,
    /// Codex accounts with an `AGENTS.override.md`, which Codex reads instead
    /// of the `AGENTS.md` Luna writes.
    pub overridden: Vec<String>,
    pub kept: Vec<String>,
}

/// Whether these kept copies are not what the log said last: a pass comes
/// before every session's start, and each list once is enough.
/// Whether a link `codex_manifests` keeps clear of is yet to be said: once a
/// run, as a pass comes with every session's start.
fn link_is_news(link: &Path) -> bool {
    static SAID: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());
    SAID.lock().unwrap_or_else(|e| e.into_inner()).insert(link.to_path_buf())
}

fn kept_is_news(kept: &[String]) -> bool {
    static SAID: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let mut said = SAID.lock().unwrap_or_else(|e| e.into_inner());
    if *said == kept {
        return false;
    }
    *said = kept.to_vec();
    true
}

fn overview(act: bool) -> Result<Overview, String> {
    let _guard = lock();
    overview_locked(act)
}

fn overview_locked(act: bool) -> Result<Overview, String> {
    let dirs = Dirs::current();
    // A pass goes by settings as the file has them now, or not at all.
    let mut lib = if act { crate::settings::get_checked()?.library } else { crate::settings::get().library };
    // Folders listed before they kept their names keep the ones they have
    // gone by, which is what is on disk.
    if act && lib.folders.iter().any(|f| f.slug.is_empty()) {
        crate::settings::update(|s| settle_slugs(&mut s.library))?;
        settle_slugs(&mut lib);
    }
    let accounts = crate::accounts::list_accounts()?;
    let found = dirs.root.is_dir();
    if act && found {
        migrate_instructions(&dirs);
    }
    let skills = if found { read_skills(&dirs) } else { Ok(BTreeMap::new()) };
    let instructions = if found { read_instructions(&dirs) } else { Ok(BTreeMap::new()) };
    let mut ledger = Ledger::load(&dirs);
    // A library that is not there (a drive not plugged in, a folder moved)
    // wants nothing, and a pass on that would take every copy out.
    let units = if found { units(&dirs, &lib, &accounts, &skills, &instructions, &ledger) } else { Vec::new() };
    let p = pass(&dirs, &units, &lib.skipped, &mut ledger, act && found);
    if act && found {
        codex_manifests(&lib, &accounts);
        prune_folder_dirs(&dirs, &lib);
        ledger.save(&dirs)?;
        sweep_blobs(&dirs, &ledger);
        sweep_backups(&dirs);
        if !p.kept.is_empty() && kept_is_news(&p.kept) {
            crate::log::info("library", &format!("left in place, changed since Luna wrote them: {}", p.kept.join(", ")));
        }
    }
    // Where instructions do go, a file Luna did not write is the place's own
    // business (`taken`), settled in place.
    let found_instructions = accounts
        .iter()
        .map(|a| (a, Path::new(&a.path).join(instructions_name(a.provider))))
        .filter(|(_, dest)| dest.is_file() && !ledger.written.contains_key(&*dest.to_string_lossy()))
        .filter(|(_, dest)| !units.iter().any(|u| ident(&u.dest) == ident(dest)))
        .map(|(a, dest)| FoundFile {
            dest: dest.to_string_lossy().into_owned(),
            target: Target::Account { provider: a.provider, name: a.name.clone() },
        })
        .collect();
    let have = |name: &str| instructions.as_ref().map_or(true, |found| found.contains_key(name));
    let instructions_for =
        accounts.iter().filter_map(|a| Some((format!("{}/{}", a.provider.as_str(), a.name), chosen(&lib, have, a)?.to_string()))).collect();
    let old_instructions = dirs.root.join(OLD_INSTRUCTIONS);
    let skills: Vec<SkillInfo> = skills
        .unwrap_or_default()
        .into_values()
        .map(|(info, _)| SkillInfo { accounts: lib.skills.get(&info.name).cloned().unwrap_or_default(), ..info })
        .collect();
    let folders = lib
        .folders
        .iter()
        .map(|f| FolderInfo {
            found: Path::new(&f.path).is_dir(),
            inherited: skills
                .iter()
                .map(|s| (s.name.clone(), inherited(&lib, &accounts, f, &s.name)))
                .filter(|(_, gets)| !gets.is_empty())
                .collect(),
            path: f.path.clone(),
            skills: f.skills.clone(),
            instructions: f.instructions.clone(),
        })
        .collect();
    let overridden = accounts
        .iter()
        .filter(|a| a.provider == Provider::Codex && Path::new(&a.path).join("AGENTS.override.md").is_file())
        .map(|a| a.name.clone())
        .collect();
    Ok(Overview {
        root: dirs.root.to_string_lossy().into_owned(),
        is_default: lib.source.is_none(),
        found,
        instructions: instructions
            .as_ref()
            .map(|i| i.iter().map(|(name, text)| instructions_info(name, text, &lib)).collect())
            .unwrap_or_default(),
        instructions_for,
        old_instructions: old_instructions.is_file().then(|| old_instructions.to_string_lossy().into_owned()),
        skills,
        no_instructions: lib.no_instructions,
        accounts,
        folders,
        places: p.places,
        found_instructions,
        overridden,
        kept: p.kept,
    })
}

/// A settings change, then a pass to lay it out.
fn change(f: impl FnOnce(&mut LibrarySettings)) -> Result<Overview, String> {
    crate::settings::update(|s| f(&mut s.library))?;
    overview(true)
}

// --------------------------------------------------------------- commands --

/// What the library holds and where each copy stands, without writing.
#[tauri::command(async)]
pub fn library_overview() -> Result<Overview, String> {
    overview(false)
}

/// Lays the library out: everything that needs no asking.
#[tauri::command(async)]
pub fn library_sync() -> Result<Overview, String> {
    overview(true)
}

/// A pass as Sync makes one, for a caller that only needs it done. A file
/// changed in the library by another program — a model improving a skill
/// there — is seen by no window, and the accounts would keep the old copy
/// until a click in it.
pub fn lay_out(when: &str) {
    if let Err(e) = overview(true) {
        crate::log::warn("library", &format!("laying the library out {when}: {e}"));
    }
}

/// Points the library at another folder; empty = back to `<data>/library`.
/// Nothing is moved.
#[tauri::command(async)]
pub fn library_set_source(path: String) -> Result<Overview, String> {
    let path = path.trim().to_string();
    if !path.is_empty() && !Path::new(&path).is_absolute() {
        return Err("The library's folder must be an absolute path".into());
    }
    crate::log::info("library", &format!("library folder: {}", if path.is_empty() { "<default>" } else { &path }));
    change(|l| l.source = (!path.is_empty()).then_some(path))
}

/// The accounts instructions go into (`name` None: the accounts that get
/// none), each entry taken from whichever other instructions had it; empty =
/// none.
#[tauri::command(async)]
pub fn library_set_instructions(name: Option<String>, accounts: Vec<String>) -> Result<Overview, String> {
    if let Some(n) = &name {
        check_name(n)?;
    }
    check_reach(&accounts)?;
    change(|l| give_instructions(l, name.as_deref(), accounts))
}

/// The accounts a skill goes into; empty = none.
#[tauri::command(async)]
pub fn library_set_accounts(skill: String, accounts: Vec<String>) -> Result<Overview, String> {
    check_name(&skill)?;
    let reach = tidy_reach(accounts)?;
    change(|l| {
        if reach.is_empty() {
            l.skills.remove(&skill);
        } else {
            l.skills.insert(skill, reach);
        }
    })
}

/// Lists a folder, saying nothing of any skill yet, or takes it off the list
/// with all it says.
#[tauri::command(async)]
pub fn library_set_folder(path: String, listed: bool) -> Result<Overview, String> {
    if !Path::new(&path).is_absolute() {
        return Err("A folder must be an absolute path".into());
    }
    let on_disk = slugs_on_disk(&Dirs::current());
    change(|l| {
        // Kept before any goes, so that none passes to another.
        settle_slugs(l);
        let at = l.folders.iter().position(|f| crate::pty::same_dir(&f.path, &path));
        match (at, listed) {
            (Some(i), false) => {
                l.folders.remove(i);
            }
            (None, true) => {
                let mut taken: BTreeSet<String> = l.folders.iter().map(|f| f.slug.clone()).collect();
                taken.extend(on_disk);
                let slug = fresh_slug(&path, &taken);
                l.folders.push(LibraryFolder { path, slug, ..Default::default() });
            }
            _ => {}
        }
    })
}

/// The accounts whose sessions in a listed folder get a skill (empty: none
/// of them), or — None — the folder saying nothing of it, so that they go by
/// what it is meant for and by the folders it lies in.
#[tauri::command(async)]
pub fn library_set_folder_skill(path: String, skill: String, accounts: Option<Vec<String>>) -> Result<Overview, String> {
    check_name(&skill)?;
    let reach = accounts.map(tidy_reach).transpose()?;
    if crate::settings::get().library.folders.iter().all(|f| !crate::pty::same_dir(&f.path, &path)) {
        return Err(format!("{path} is not one of the library's folders"));
    }
    change(|l| {
        let Some(f) = l.folders.iter_mut().find(|f| crate::pty::same_dir(&f.path, &path)) else { return };
        match reach {
            Some(reach) => f.skills.insert(skill, reach),
            None => f.skills.remove(&skill),
        };
    })
}

/// What sessions on an account (a reach entry: as a rule `provider/name`) in
/// a listed folder get in place of its instructions: `name`'s, or none at all
/// (None); `inherit`, what holds outside the folder.
#[tauri::command(async)]
pub fn library_set_folder_instructions(path: String, account: String, inherit: bool, name: Option<String>) -> Result<Overview, String> {
    check_reach(std::slice::from_ref(&account))?;
    if let Some(n) = &name {
        check_name(n)?;
    }
    if crate::settings::get().library.folders.iter().all(|f| !crate::pty::same_dir(&f.path, &path)) {
        return Err(format!("{path} is not one of the library's folders"));
    }
    change(|l| {
        let Some(f) = l.folders.iter_mut().find(|f| crate::pty::same_dir(&f.path, &path)) else { return };
        match inherit {
            true => f.instructions.remove(&account),
            false => f.instructions.insert(account, name),
        };
    })
}

/// Leaves a destination out of every pass, or takes it back in. Left out, it
/// is no longer Luna's copy: whatever is there stays, as anyone's file.
#[tauri::command(async)]
pub fn library_skip(dest: String, on: bool) -> Result<Overview, String> {
    let _guard = lock();
    let dirs = Dirs::current();
    let mut ledger = Ledger::load(&dirs);
    // Under any spelling of the path, as a pass goes by.
    let id = ident(Path::new(&dest));
    let noted = ledger.written.len();
    if on {
        ledger.written.retain(|k, _| ident(Path::new(k)) != id);
    }
    if ledger.written.len() != noted {
        ledger.save(&dirs)?;
    }
    crate::settings::update(|s| {
        s.library.skipped.retain(|d| ident(Path::new(d)) != id);
        if on {
            s.library.skipped.push(dest.clone());
        }
    })?;
    overview_locked(true)
}

/// What a destination is to the library.
enum Located {
    /// A copy the library wants now: where it comes from, whether it is one
    /// file, the skill's name for a skill, what the copy is to hold — the
    /// library's, under the name it goes by there (`shown`) where that is
    /// another — or why the library has nothing fit for it.
    Copy { from: PathBuf, file: bool, skill: Option<String>, shown: Option<String>, source: Result<Files, String> },
    /// An account's instructions file where no instructions of the library go.
    Unclaimed(AccountInfo),
}

fn locate(dirs: &Dirs, dest: &str) -> Result<Located, String> {
    let lib = crate::settings::get().library;
    let accounts = crate::accounts::list_accounts()?;
    let units = units(dirs, &lib, &accounts, &read_skills(dirs), &read_instructions(dirs), &Ledger::load(dirs));
    if let Some(u) = units.into_iter().find(|u| u.dest.to_string_lossy() == dest) {
        let from = match (&u.skill, &u.instructions) {
            (Some(s), _) => dirs.skills().join(s),
            (None, Some(i)) => dirs.instructions_file(i),
            (None, None) => return Err(format!("{dest} is laid out from nothing")),
        };
        return Ok(Located::Copy { from, file: u.file, skill: u.skill, shown: u.shown, source: u.source });
    }
    match accounts.into_iter().find(|a| Path::new(&a.path).join(instructions_name(a.provider)).to_string_lossy() == dest) {
        Some(a) => Ok(Located::Unclaimed(a)),
        None => Err(format!("{dest} is not a place the library is laid out to")),
    }
}

/// Makes an account's own instructions file new instructions in the library,
/// named after the account and going into it alone: the account's file
/// stays as it is, now Luna's copy, and whether the instructions go further
/// is picked in the window after.
fn take_unclaimed(dirs: &Dirs, ledger: &mut Ledger, dest: &str, a: &AccountInfo) -> Result<(), String> {
    let there = read_at(Path::new(dest), true)?.ok_or("Nothing is there to take")?;
    if there.values().all(|t| String::from_utf8_lossy(t).trim().is_empty()) {
        return Err(format!("{dest} is blank"));
    }
    let name = new_instructions_name(dirs, &a.name);
    write_at(&dirs.instructions_file(&name), true, &there)?;
    keep_blobs(dirs, &there);
    ledger.written.insert(dest.to_string(), hashes(&there));
    ledger.save(dirs)?;
    let key = format!("{}/{}", a.provider.as_str(), a.name);
    crate::settings::update(|s| give_instructions(&mut s.library, Some(&name), vec![key]))?;
    crate::log::info("library", &format!("took {dest} into the library as instructions {name}"));
    Ok(())
}

/// Settles a copy that changed, went or was never Luna's: `take` makes it the
/// library's (and so every other copy's), otherwise the library's goes back
/// in its place. Whichever side is replaced is backed up first. An account's
/// file where no instructions go is taken as new instructions of its own.
#[tauri::command(async)]
pub fn library_resolve(dest: String, take: bool) -> Result<Overview, String> {
    let _guard = lock();
    let dirs = Dirs::current();
    let located = locate(&dirs, &dest)?;
    let to = PathBuf::from(&dest);
    if in_library(&dirs, &to) {
        return Err(format!("{dest} is in the library itself"));
    }
    let mut ledger = Ledger::load(&dirs);
    let (from, file, skill, shown, source) = match located {
        Located::Copy { from, file, skill, shown, source } => (from, file, skill, shown, source),
        Located::Unclaimed(a) if take => {
            take_unclaimed(&dirs, &mut ledger, &dest, &a)?;
            return overview_locked(true);
        }
        Located::Unclaimed(_) => return Err("No instructions of the library go there".into()),
    };
    let there = read_at(&to, file)?;
    if take {
        let there = there.ok_or("Nothing is there to take")?;
        // A copy under another name goes back under the skill's own.
        let taken = match (&skill, &shown) {
            (Some(name), Some(_)) => with_name(there.clone(), name),
            _ => there.clone(),
        };
        if let Some(name) = &skill {
            let text = taken.get(SKILL_FILE).ok_or("There is no SKILL.md there")?;
            check_skill(name, &String::from_utf8_lossy(text))?;
        }
        if let Some(old) = read_at(&from, file)? {
            back_up(&dirs, &from, &old)?;
        }
        write_at(&from, file, &taken)?;
        keep_blobs(&dirs, &there);
        ledger.written.insert(dest.clone(), hashes(&there));
        crate::log::info("library", &format!("took {dest} into the library"));
    } else {
        let library = source.map_err(|e| format!("The library has nothing fit for it: {e}"))?;
        if let Some(name) = shown.as_ref().or(skill.as_ref()) {
            let text = library.get(SKILL_FILE).ok_or("The library's copy has no SKILL.md")?;
            check_skill(name, &String::from_utf8_lossy(text))?;
        }
        if let Some(old) = &there {
            back_up(&dirs, &to, old)?;
        }
        write_at(&to, file, &library)?;
        keep_blobs(&dirs, &library);
        ledger.written.insert(dest.clone(), hashes(&library));
        crate::log::info("library", &format!("put the library's copy back at {dest}"));
    }
    ledger.save(&dirs)?;
    overview_locked(true)
}

/// One file of a copy as it is in each of three places: what Luna wrote
/// there last, the library now, and there now. None where it is not.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDiff {
    pub path: String,
    pub base: Option<String>,
    pub library: Option<String>,
    pub there: Option<String>,
    /// Not text: shown as changed, not line by line.
    pub binary: bool,
}

#[tauri::command(async)]
pub fn library_diff(dest: String) -> Result<Vec<FileDiff>, String> {
    let _guard = lock();
    let dirs = Dirs::current();
    let Located::Copy { from, file, source, .. } = locate(&dirs, &dest)? else {
        return Err("No instructions of the library go there".into());
    };
    let ledger = Ledger::load(&dirs);
    let base: Files =
        ledger.written.get(&dest).map(|h| h.iter().filter_map(|(k, v)| Some((k.clone(), blob(&dirs, v)?))).collect()).unwrap_or_default();
    // As the copy is to be, under the name it goes by there; a skill not fit
    // to be laid out, as the library has it.
    let library = match source {
        Ok(files) => files,
        Err(_) => read_at(&from, file)?.unwrap_or_default(),
    };
    let there = read_at(Path::new(&dest), file)?.unwrap_or_default();
    let paths: BTreeSet<&String> = base.keys().chain(library.keys()).chain(there.keys()).collect();
    let name = Path::new(&dest).file_name().unwrap_or_default().to_string_lossy().into_owned();
    Ok(paths
        .into_iter()
        .filter(|p| !(base.get(*p) == library.get(*p) && library.get(*p) == there.get(*p)))
        .map(|p| {
            let all = [base.get(p), library.get(p), there.get(p)];
            let binary = all.iter().flatten().any(|b| std::str::from_utf8(b).is_err());
            let text =
                |b: Option<&Vec<u8>>| if binary { b.map(|_| String::new()) } else { b.map(|b| String::from_utf8_lossy(b).into_owned()) };
            FileDiff {
                path: if p.is_empty() { name.clone() } else { p.clone() },
                base: text(all[0]),
                library: text(all[1]),
                there: text(all[2]),
                binary,
            }
        })
        .collect())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryFile {
    pub path: String,
    pub text: String,
    pub exists: bool,
}

/// The file the editor has open: `{"skill": name}` or `{"instructions": name}`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Entry {
    Skill(String),
    Instructions(String),
}

fn library_path(dirs: &Dirs, entry: &Entry) -> Result<PathBuf, String> {
    match entry {
        Entry::Skill(s) => {
            check_name(s)?;
            Ok(dirs.skills().join(s).join(SKILL_FILE))
        }
        Entry::Instructions(i) => {
            check_name(i)?;
            Ok(dirs.instructions_file(i))
        }
    }
}

/// Instructions, or a skill's SKILL.md, for the editor.
#[tauri::command(async)]
pub fn library_read(entry: Entry) -> Result<LibraryFile, String> {
    let path = library_path(&Dirs::current(), &entry)?;
    let (text, exists) = match std::fs::read_to_string(&path) {
        Ok(t) => (t, true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (String::new(), false),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    Ok(LibraryFile { path: path.to_string_lossy().into_owned(), text, exists })
}

/// Saves what the editor has, then lays it out. A skill whose SKILL.md does
/// not do is still saved — it is the user's draft — and laid out nowhere until
/// it does.
#[tauri::command(async)]
pub fn library_write(entry: Entry, text: String) -> Result<Overview, String> {
    let _guard = lock();
    let path = library_path(&Dirs::current(), &entry)?;
    write_file(&path, text.as_bytes())?;
    overview_locked(true)
}

/// Why this SKILL.md would not be laid out, or None when it would.
#[tauri::command(async)]
pub fn library_check_skill(skill: String, text: String) -> Option<String> {
    check_skill(&skill, &text).err()
}

#[tauri::command(async)]
pub fn library_new_skill(name: String) -> Result<Overview, String> {
    let name = name.trim().to_string();
    check_name(&name)?;
    let _guard = lock();
    let dirs = Dirs::current();
    let dir = dirs.skills().join(&name);
    if dir.exists() {
        return Err(format!("The library already has a skill named {name}"));
    }
    let text = format!(
        "---\nname: {name}\ndescription: What this skill does and when to use it. The model reads only this line until it decides to.\n---\n\n# {name}\n\nThe steps, rules and examples to follow.\n"
    );
    write_file(&dir.join(SKILL_FILE), text.as_bytes())?;
    overview_locked(false)
}

/// Takes a skill out of the library — into the backups — and out of every
/// list; its copies go too, those still as Luna wrote them.
#[tauri::command(async)]
pub fn library_delete_skill(name: String) -> Result<Overview, String> {
    check_name(&name)?;
    let _guard = lock();
    let dirs = Dirs::current();
    let dir = dirs.skills().join(&name);
    if let Some(files) = read_at(&dir, false)? {
        back_up(&dirs, &dir, &files)?;
        std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    crate::settings::update(|s| {
        s.library.skills.remove(&name);
        for f in &mut s.library.folders {
            f.skills.remove(&name);
        }
    })?;
    crate::log::info("library", &format!("skill {name} taken out of the library"));
    overview_locked(true)
}

/// New instructions, blank and going nowhere until written and picked for an
/// account. A reach left in the settings by a file of the name deleted by
/// hand goes: new instructions start from none.
#[tauri::command(async)]
pub fn library_new_instructions(name: String) -> Result<Overview, String> {
    let name = name.trim().to_string();
    check_name(&name)?;
    let _guard = lock();
    let dirs = Dirs::current();
    let path = dirs.instructions_file(&name);
    if std::fs::symlink_metadata(&path).is_ok() {
        return Err(format!("The library already has instructions named {name}"));
    }
    write_file(&path, b"")?;
    crate::settings::update(|s| {
        s.library.instructions.remove(&name);
    })?;
    overview_locked(false)
}

/// Renames instructions, their reach with them. Their copies stay as they
/// are: the ledger knows a copy by where it lies, not by what it came from.
#[tauri::command(async)]
pub fn library_rename_instructions(from: String, to: String) -> Result<Overview, String> {
    let to = to.trim().to_string();
    check_name(&from)?;
    check_name(&to)?;
    let _guard = lock();
    let dirs = Dirs::current();
    let (old, new) = (dirs.instructions_file(&from), dirs.instructions_file(&to));
    if from == to {
        return overview_locked(false);
    }
    if std::fs::symlink_metadata(&new).is_ok() {
        return Err(format!("The library already has instructions named {to}"));
    }
    std::fs::rename(&old, &new).map_err(|e| format!("{}: {e}", old.display()))?;
    // The file and its reach move together or not at all: the file renamed
    // without its reach would take every one of its copies out.
    let moved = crate::settings::update(|s| {
        if let Some(reach) = s.library.instructions.remove(&from) {
            s.library.instructions.insert(to.clone(), reach);
        }
        for choice in s.library.folders.iter_mut().flat_map(|f| f.instructions.values_mut()) {
            if choice.as_deref() == Some(from.as_str()) {
                *choice = Some(to.clone());
            }
        }
    });
    if let Err(e) = moved {
        let _ = std::fs::rename(&new, &old);
        return Err(e);
    }
    crate::log::info("library", &format!("instructions {from} renamed {to}"));
    overview_locked(true)
}

/// Takes instructions out of the library — into the backups — and out of the
/// settings. The accounts they went to get the next closest instructions, or
/// none; their copies are replaced or go, those still as Luna wrote them.
#[tauri::command(async)]
pub fn library_delete_instructions(name: String) -> Result<Overview, String> {
    check_name(&name)?;
    let _guard = lock();
    let dirs = Dirs::current();
    let path = dirs.instructions_file(&name);
    if let Some(files) = read_at(&path, true)? {
        back_up(&dirs, &path, &files)?;
        std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    crate::settings::update(|s| {
        s.library.instructions.remove(&name);
        // A folder that gave them goes by what holds outside it again.
        for f in &mut s.library.folders {
            f.instructions.retain(|_, choice| choice.as_deref() != Some(name.as_str()));
        }
    })?;
    crate::log::info("library", &format!("instructions {name} taken out of the library"));
    overview_locked(true)
}

/// Opens the library's folder, or a skill's, in the file manager — for the
/// files beside SKILL.md, which the editor does not cover.
#[tauri::command(async)]
pub fn library_open(skill: Option<String>) -> Result<(), String> {
    let dirs = Dirs::current();
    let dir = match skill {
        Some(s) => {
            check_name(&s)?;
            dirs.skills().join(s)
        }
        None => dirs.root,
    };
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program).arg(&dir).spawn().map(|_| ()).map_err(|e| format!("{program}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{LibraryFolder, FIRST_INSTRUCTIONS};

    struct Fixture {
        base: PathBuf,
        dirs: Dirs,
        accounts: Vec<AccountInfo>,
    }

    impl Fixture {
        fn new(tag: &str) -> Fixture {
            let base = std::env::temp_dir().join(format!("luna-library-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&base);
            let dirs = Dirs { root: base.join("library"), data: base.join("data") };
            let accounts =
                [(Provider::Claude, "anthropic", "work"), (Provider::Codex, "openai", "home"), (Provider::Claude, "anthropic", "home")]
                    .into_iter()
                    .map(|(provider, vendor, name)| {
                        let path = base.join("accounts").join(vendor).join(name);
                        std::fs::create_dir_all(&path).unwrap();
                        AccountInfo { provider, name: name.into(), path: path.to_string_lossy().into_owned() }
                    })
                    .collect();
            std::fs::create_dir_all(&dirs.root).unwrap();
            Fixture { base, dirs, accounts }
        }
        fn skill(&self, name: &str, body: &str) {
            let text = format!("---\nname: {name}\ndescription: Does {name}.\n---\n{body}\n");
            write_file(&self.dirs.skills().join(name).join(SKILL_FILE), text.as_bytes()).unwrap();
        }
        fn instructions(&self, name: &str, text: &str) {
            write_file(&self.dirs.instructions_file(name), text.as_bytes()).unwrap();
        }
        fn account(&self, i: usize) -> PathBuf {
            PathBuf::from(&self.accounts[i].path)
        }
        fn read(&self, i: usize, file: &str) -> String {
            std::fs::read_to_string(self.account(i).join(file)).unwrap_or_default()
        }
        fn run(&self, lib: &LibrarySettings, ledger: &mut Ledger, act: bool) -> Pass {
            let skills = read_skills(&self.dirs);
            let instructions = read_instructions(&self.dirs);
            let units = units(&self.dirs, lib, &self.accounts, &skills, &instructions, ledger);
            pass(&self.dirs, &units, &lib.skipped, ledger, act)
        }
        fn status(p: &Pass, dest: &Path) -> Status {
            // By components: a test's `join("a/b")` keeps its slash on Windows.
            p.places.iter().find(|x| Path::new(&x.dest) == dest).map(|x| x.status).expect("a place for it")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    fn all() -> Reach {
        vec![ALL.into()]
    }

    /// These skills, each into every account.
    fn skills(names: &[&str]) -> BTreeMap<String, Reach> {
        names.iter().map(|n| (n.to_string(), all())).collect()
    }

    /// The first instructions, `main`, with this reach.
    fn first(reach: Reach) -> BTreeMap<String, Reach> {
        BTreeMap::from([(FIRST_INSTRUCTIONS.to_string(), reach)])
    }

    fn reach(r: &[&str]) -> Reach {
        r.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_reach_takes_in_all_a_provider_or_one_account() {
        let f = Fixture::new("reach");
        f.skill("review", "v1");
        f.instructions("main", "Be brief.\n");
        let lib = LibrarySettings {
            instructions: first(reach(&["codex"])),
            skills: BTreeMap::from([("review".into(), reach(&["claude/work"]))]),
            ..Default::default()
        };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        assert!(!f.account(0).join("CLAUDE.md").exists());
        assert!(f.account(1).join("AGENTS.md").is_file());
        assert!(f.account(0).join("skills/review/SKILL.md").is_file());
        assert!(!f.account(1).join("skills/review").exists());

        // Narrowed: the account no longer meant goes, as for a skill unticked.
        let lib = LibrarySettings { skills: BTreeMap::from([("review".into(), reach(&["codex/home"]))]), ..lib };
        f.run(&lib, &mut ledger, true);
        assert!(!f.account(0).join("skills/review").exists());
        assert!(f.account(1).join("skills/review/SKILL.md").is_file());

        assert_eq!(tidy_reach(reach(&["claude/work", "all"])).unwrap(), all());
        assert_eq!(tidy_reach(reach(&["claude/work", "codex/home", "claude", "codex/home"])).unwrap(), reach(&["codex/home", "claude"]));
        assert!(tidy_reach(reach(&["gemini"])).is_err());
        assert!(tidy_reach(reach(&["claude/"])).is_err());
    }

    #[test]
    fn a_renamed_account_keeps_its_reach_and_a_deleted_one_leaves_it() {
        let mut lib = LibrarySettings {
            instructions: BTreeMap::from([("main".into(), reach(&["claude/work", "codex"])), ("work".into(), reach(&["claude/work"]))]),
            no_instructions: reach(&["claude/work"]),
            skills: BTreeMap::from([("review".into(), vec!["claude/work".into()]), ("notes".into(), all())]),
            ..Default::default()
        };
        rename_account(&mut lib, "claude/work", Some("claude/office"));
        assert_eq!(lib.instructions["main"], reach(&["claude/office", "codex"]));
        assert_eq!(lib.instructions["work"], reach(&["claude/office"]));
        assert_eq!(lib.no_instructions, reach(&["claude/office"]));
        assert_eq!(lib.skills["review"], vec!["claude/office".to_string()]);
        rename_account(&mut lib, "claude/office", None);
        assert_eq!(lib.instructions["main"], reach(&["codex"]));
        assert!(!lib.instructions.contains_key("work"), "instructions that go nowhere any more are dropped");
        assert!(lib.no_instructions.is_empty());
        assert!(!lib.skills.contains_key("review"), "a skill that goes nowhere any more is dropped");
        assert_eq!(lib.skills["notes"], all());
    }

    /// A pass before every session's start says the same kept copies once.
    #[test]
    fn kept_copies_are_logged_once_a_list() {
        let a = vec!["E:/acc/skills/x".to_string()];
        assert!(kept_is_news(&a));
        assert!(!kept_is_news(&a), "the same list again is no news");
        assert!(kept_is_news(&["E:/acc/skills/y".to_string()]));
    }

    #[test]
    fn front_matter_reads_the_ways_yaml_writes_a_line() {
        let ok = |t: &str| check_skill("review", t);
        assert_eq!(ok("---\nname: review\ndescription: Reviews code.\n---\nbody").unwrap(), "Reviews code.");
        assert_eq!(ok("---\nname: \"review\"\ndescription: 'Quoted: yes'\n---\n").unwrap(), "Quoted: yes");
        assert_eq!(ok("---\nname: review\ndescription: >\n  Folded\n  over lines\nother: x\n---\n").unwrap(), "Folded over lines");
        assert_eq!(ok("---\nname: review\ndescription: Plain\n  continued\n---\n").unwrap(), "Plain continued");
        assert!(ok("no front matter").unwrap_err().contains("front matter"));
        assert!(ok("---\nname: review\ndescription: x\n").unwrap_err().contains("never closed"));
        assert!(ok("---\nname: other\ndescription: x\n---\n").unwrap_err().contains("other"));
        assert!(ok("---\nname: review\n---\n").unwrap_err().contains("description"));
        assert!(check_skill("Bad Name", "---\nname: Bad Name\ndescription: x\n---\n").is_err());
    }

    #[test]
    fn instructions_are_told_by_their_first_heading() {
        assert_eq!(instructions_title("# Talking to the operator\n\n## Language\n"), "Talking to the operator");
        assert_eq!(instructions_title("Some prose first.\n\n## Language\n\n# Late title\n"), "Late title");
        assert_eq!(instructions_title("Some prose first.\n\n## Language\n## Reports\n"), "Language");
        assert_eq!(instructions_title("```sh\n# a comment\n```\n## Real\n"), "Real");
        assert_eq!(instructions_title("#tag\n### Deep\n#\n"), "");
        assert_eq!(instructions_title("Only prose, cut off somewhere.\n"), "");
    }

    #[test]
    fn a_pass_lays_out_once_and_keeps_up_with_the_library() {
        let f = Fixture::new("lay");
        f.skill("review", "v1");
        f.instructions("main", "Be brief.\n");
        let lib = LibrarySettings { instructions: first(all()), skills: skills(&["review"]), ..Default::default() };
        let mut ledger = Ledger::default();

        let p = f.run(&lib, &mut ledger, false);
        assert_eq!(Fixture::status(&p, &f.account(0).join("CLAUDE.md")), Status::Missing);
        assert!(!f.account(0).join("CLAUDE.md").exists(), "looking writes nothing");

        let p = f.run(&lib, &mut ledger, true);
        assert!(p.places.iter().all(|x| x.status == Status::Current), "{:?}", p.places);
        assert_eq!(std::fs::read_to_string(f.account(0).join("CLAUDE.md")).unwrap(), "Be brief.\n");
        assert_eq!(std::fs::read_to_string(f.account(1).join("AGENTS.md")).unwrap(), "Be brief.\n");
        assert!(f.account(1).join("skills/review/SKILL.md").is_file());

        // Nothing new: nothing written.
        let stamp = std::fs::metadata(f.account(0).join("CLAUDE.md")).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        f.run(&lib, &mut ledger, true);
        assert_eq!(std::fs::metadata(f.account(0).join("CLAUDE.md")).unwrap().modified().unwrap(), stamp);

        // The library moves on; Luna's own copies follow, a file taken out of
        // the skill goes from them too.
        f.skill("review", "v2");
        write_file(&f.dirs.skills().join("review/notes/a.md"), b"a").unwrap();
        let p = f.run(&lib, &mut ledger, false);
        assert_eq!(Fixture::status(&p, &f.account(0).join("skills/review")), Status::Behind);
        f.run(&lib, &mut ledger, true);
        assert!(std::fs::read_to_string(f.account(0).join("skills/review/SKILL.md")).unwrap().contains("v2"));
        assert!(f.account(0).join("skills/review/notes/a.md").is_file());
        std::fs::remove_dir_all(f.dirs.skills().join("review/notes")).unwrap();
        f.run(&lib, &mut ledger, true);
        assert!(!f.account(0).join("skills/review/notes").exists());
    }

    #[test]
    fn a_copy_changed_there_is_asked_about_never_replaced() {
        let f = Fixture::new("changed");
        f.instructions("main", "Be brief.\n");
        let lib = LibrarySettings { instructions: first(all()), ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        let claude_md = f.account(0).join("CLAUDE.md");
        std::fs::write(&claude_md, "Be brief. And kind.\n").unwrap();

        let p = f.run(&lib, &mut ledger, true);
        let place = p.places.iter().find(|x| x.dest == claude_md.to_string_lossy()).unwrap();
        assert_eq!((place.status, place.both), (Status::Changed, false));
        assert_eq!(std::fs::read_to_string(&claude_md).unwrap(), "Be brief. And kind.\n");

        // The library changes too: still asked, and said that both did.
        f.instructions("main", "Be terse.\n");
        let p = f.run(&lib, &mut ledger, true);
        let place = p.places.iter().find(|x| x.dest == claude_md.to_string_lossy()).unwrap();
        assert_eq!((place.status, place.both), (Status::Changed, true));
        assert_eq!(std::fs::read_to_string(&claude_md).unwrap(), "Be brief. And kind.\n");
        // The other account, unchanged there, follows the library.
        assert_eq!(std::fs::read_to_string(f.account(1).join("AGENTS.md")).unwrap(), "Be terse.\n");

        // Deleted there: gone, not put back unasked.
        std::fs::remove_file(&claude_md).unwrap();
        let p = f.run(&lib, &mut ledger, true);
        assert_eq!(Fixture::status(&p, &claude_md), Status::Gone);
        assert!(!claude_md.exists());
    }

    #[test]
    fn what_luna_did_not_write_is_left_alone() {
        let f = Fixture::new("foreign");
        f.skill("review", "mine");
        let lib = LibrarySettings { skills: skills(&["review"]), ..Default::default() };
        // The account already has a skill of that name, and another one.
        write_file(&f.account(0).join("skills/review/SKILL.md"), b"someone else's").unwrap();
        write_file(&f.account(0).join("skills/other/SKILL.md"), b"other").unwrap();
        // The second account holds exactly the library's: adopted as it is.
        let ours = std::fs::read(f.dirs.skills().join("review/SKILL.md")).unwrap();
        write_file(&f.account(1).join("skills/review/SKILL.md"), &ours).unwrap();
        let mut ledger = Ledger::default();

        let p = f.run(&lib, &mut ledger, true);
        assert_eq!(Fixture::status(&p, &f.account(0).join("skills/review")), Status::Taken);
        assert_eq!(Fixture::status(&p, &f.account(1).join("skills/review")), Status::Current);
        assert_eq!(std::fs::read(f.account(0).join("skills/review/SKILL.md")).unwrap(), b"someone else's");

        // No longer wanted: Luna's copy goes, the foreign ones stay.
        let none = LibrarySettings::default();
        f.run(&none, &mut ledger, true);
        assert!(!f.account(1).join("skills/review").exists());
        assert!(f.account(0).join("skills/review/SKILL.md").is_file());
        assert!(f.account(0).join("skills/other/SKILL.md").is_file());
        assert!(f.account(1).join("skills").is_dir(), "only the skill's own folder goes");
    }

    #[test]
    fn a_copy_no_longer_wanted_but_changed_is_kept() {
        let f = Fixture::new("kept");
        f.skill("review", "v1");
        let lib = LibrarySettings { skills: BTreeMap::from([("review".into(), reach(&["claude/work"]))]), ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        let copy = f.account(0).join("skills").join("review");
        assert!(copy.join("SKILL.md").is_file());
        std::fs::write(copy.join("extra.md"), "added there").unwrap();

        let p = f.run(&LibrarySettings::default(), &mut ledger, true);
        assert_eq!(p.kept, vec![copy.to_string_lossy().into_owned()]);
        assert!(copy.join("extra.md").is_file());
        assert!(ledger.written.is_empty(), "no longer Luna's");
    }

    #[test]
    fn a_folder_that_is_gone_is_not_made_again() {
        let f = Fixture::new("gone-folder");
        f.skill("review", "v1");
        let folder = f.base.join("deleted-repo/skills");
        let lib = LibrarySettings {
            folders: vec![LibraryFolder { path: folder.to_string_lossy().into_owned(), skills: given(&["review"]), ..Default::default() }],
            ..Default::default()
        };
        let p = f.run(&lib, &mut Ledger::default(), true);
        assert!(p.places.iter().all(|p| p.status == Status::Failed));
        assert!(!f.base.join("deleted-repo").exists());
        assert!(!f.dirs.folders().exists(), "nothing waits for a folder that is not there");
    }

    /// These skills, each to every account's sessions in a folder.
    fn given(names: &[&str]) -> BTreeMap<String, Reach> {
        skills(names)
    }

    /// A repository at `<base>/<name>`, and a folder of the library on it
    /// giving these skills to every account's sessions there.
    fn repo_folder(f: &Fixture, name: &str, skills: &[&str]) -> (PathBuf, LibraryFolder) {
        let repo = f.base.join(name);
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        (repo.clone(), LibraryFolder { path: repo.to_string_lossy().into_owned(), skills: given(skills), ..Default::default() })
    }

    #[test]
    fn a_folders_skills_wait_for_its_sessions_and_never_go_into_it() {
        let f = Fixture::new("folder-or");
        f.skill("review", "v1");
        let (repo, folder) = repo_folder(&f, "asuri-app", &["review"]);
        // Meant for claude/work as a whole, and for every account in the folder.
        let lib = LibrarySettings {
            skills: BTreeMap::from([("review".into(), reach(&["claude/work"]))]),
            folders: vec![folder],
            ..Default::default()
        };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        codex_manifests(&lib, &f.accounts);
        assert!(f.account(0).join("skills/review/SKILL.md").is_file());
        assert!(!f.dirs.folders().join("asuri-app/work").exists(), "it has it as a whole already");
        assert!(f.dirs.folders().join("asuri-app/home/.claude/skills/review/SKILL.md").is_file());
        let plugin = f.account(1).join("plugins/cache/luna/asuri-app/local");
        assert!(plugin.join("skills/review/SKILL.md").is_file());
        let manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(plugin.join(".codex-plugin/plugin.json")).unwrap()).unwrap();
        assert_eq!(manifest["name"], "asuri-app");
        assert_eq!(manifest["skills"], "./skills/");
        assert!(!repo.join("review").exists() && !repo.join(".claude").exists(), "nothing goes into the folder");

        // Its accounts only in the folder: none as a whole.
        let mut lib = lib;
        lib.skills.clear();
        lib.folders[0].skills = BTreeMap::from([("review".into(), reach(&["claude/work"]))]);
        let p = f.run(&lib, &mut ledger, true);
        codex_manifests(&lib, &f.accounts);
        assert!(p.kept.is_empty());
        assert!(!f.account(0).join("skills/review").exists());
        assert!(f.dirs.folders().join("asuri-app/work/.claude/skills/review/SKILL.md").is_file());
        assert!(!f.dirs.folders().join("asuri-app/home/.claude/skills/review").exists());
        assert!(!f.account(1).join("plugins/cache/luna").exists(), "a plugin with nothing in it goes, manifest and all");

        // A folder no longer listed: what waited for it goes.
        let lib = LibrarySettings { folders: Vec::new(), ..lib };
        f.run(&lib, &mut ledger, true);
        prune_folder_dirs(&f.dirs, &lib);
        assert!(!f.dirs.folders().join("asuri-app").exists());
    }

    /// A file of a skill renamed only in case comes out under its new name,
    /// where case is ignored too: not written and then removed as the old one.
    #[test]
    fn a_file_renamed_only_in_case_is_still_there() {
        let f = Fixture::new("case-rename");
        let dest = f.base.join("copy");
        let mut files = Files::new();
        files.insert(SKILL_FILE.to_string(), b"skill".to_vec());
        files.insert("guide.md".to_string(), b"v1".to_vec());
        write_at(&dest, false, &files).unwrap();
        files.remove("guide.md");
        files.insert("GUIDE.md".to_string(), b"v2".to_vec());
        write_at(&dest, false, &files).unwrap();
        assert_eq!(read_dir_files(&dest).unwrap(), files);
    }

    #[test]
    fn copies_once_laid_into_a_folder_go_as_ever() {
        let f = Fixture::new("folder-old");
        f.skill("review", "v1");
        let (repo, folder) = repo_folder(&f, "repo", &["review"]);
        // Where a folder's skill went before: `<folder>/<name>`, noted as Luna's.
        let old = repo.join("review");
        let files = read_dir_files(&f.dirs.skills().join("review")).unwrap();
        write_at(&old, false, &files).unwrap();
        let mut ledger = Ledger::default();
        ledger.written.insert(old.to_string_lossy().into_owned(), hashes(&files));
        f.run(&LibrarySettings { folders: vec![folder], ..Default::default() }, &mut ledger, true);
        assert!(!old.exists());
        assert!(f.dirs.folders().join("repo/work/.claude/skills/review/SKILL.md").is_file());
    }

    #[test]
    fn a_claude_copy_whose_name_is_taken_there_goes_by_another() {
        let f = Fixture::new("folder-rename");
        f.skill("review", "v1");
        f.skill("notes", "v1");
        f.skill("luna-review", "ours too");
        let (repo, folder) = repo_folder(&f, "repo", &["review", "notes", "luna-review"]);
        // The repository has a review of its own; claude/home's account a notes.
        write_file(&repo.join(".claude/skills/review/SKILL.md"), b"---\nname: review\ndescription: The repo's.\n---\n").unwrap();
        write_file(&f.account(2).join("skills/notes/SKILL.md"), b"---\nname: notes\ndescription: The account's.\n---\n").unwrap();
        let lib = LibrarySettings { folders: vec![folder], ..Default::default() };
        let mut ledger = Ledger::default();
        let p = f.run(&lib, &mut ledger, true);
        let work = f.dirs.folders().join("repo/work/.claude/skills");
        let home = f.dirs.folders().join("repo/home/.claude/skills");
        let renamed = std::fs::read_to_string(work.join("luna-review-2/SKILL.md")).unwrap();
        assert!(renamed.starts_with("---\nname: luna-review-2\ndescription: Does review.\n"), "{renamed}");
        assert!(check_skill("luna-review-2", &renamed).is_ok());
        assert!(work.join("luna-review/SKILL.md").is_file(), "ours keeps the name it has");
        assert!(work.join("notes/SKILL.md").is_file() && !work.join("luna-notes").exists());
        assert!(home.join("luna-notes/SKILL.md").is_file());
        assert!(f.account(1).join("plugins/cache/luna/repo/local/skills/review/SKILL.md").is_file(), "Codex prefixes them all");
        let shown = p.places.iter().find(|x| Path::new(&x.dest) == work.join("luna-review-2")).unwrap();
        assert_eq!(shown.skill.as_deref(), Some("review"));
        assert_eq!(shown.shown_as.as_deref(), Some("luna-review-2"));

        // Compared with the copy as it is to be: nothing to do next time.
        let p = f.run(&lib, &mut ledger, true);
        assert_eq!(Fixture::status(&p, &work.join("luna-review-2")), Status::Current);
    }

    #[test]
    fn a_name_put_on_a_copy_comes_off_it() {
        let text = "---\r\nname: \"review\"\r\ndescription: >\r\n  name: not this one\r\n---\r\nname: nor this\r\n";
        let files = Files::from([(SKILL_FILE.to_string(), text.as_bytes().to_vec()), ("ref.md".to_string(), b"name: x".to_vec())]);
        let renamed = with_name(files.clone(), "luna-review");
        assert_eq!(String::from_utf8_lossy(&renamed[SKILL_FILE]), text.replace("name: \"review\"", "name: luna-review"));
        assert_eq!(renamed["ref.md"], files["ref.md"]);
        let back = with_name(renamed, "review");
        assert!(check_skill("review", &String::from_utf8_lossy(&back[SKILL_FILE])).is_ok());

        let taken = BTreeSet::from(["x".to_string(), "luna-y".to_string()]);
        let names = free_names(&["x", "luna-x", "y"], &taken);
        assert_eq!(names, BTreeMap::from([("x".to_string(), "luna-x-2".to_string())]));
    }

    #[test]
    fn a_folders_name_is_one_codex_takes_in_a_key() {
        let folders: Vec<LibraryFolder> = ["C:/a/Asuri App", "D:/b/asuri.app", "E:/c/asuri-app", "/"]
            .iter()
            .map(|p| LibraryFolder { path: p.to_string(), ..Default::default() })
            .collect();
        let slugs: Vec<String> = folder_slugs(&folders).into_iter().map(|(_, s)| s).collect();
        assert_eq!(slugs, ["asuri-app", "asuri-app-2", "asuri-app-3", "folder"]);
        assert!(slugs.iter().all(|s| check_name(s).is_ok()));
    }

    /// Kept, a folder's name stays its own as an earlier folder goes; one the
    /// file holds that no folder could have been given is not taken.
    #[test]
    fn a_folder_keeps_its_name_when_an_earlier_one_goes() {
        let mut lib = LibrarySettings {
            folders: ["E:/a/app", "E:/b/app", "E:/c/app"]
                .iter()
                .map(|p| LibraryFolder { path: p.to_string(), ..Default::default() })
                .collect(),
            ..Default::default()
        };
        settle_slugs(&mut lib);
        let slugs = |lib: &LibrarySettings| folder_slugs(&lib.folders).into_iter().map(|(_, s)| s).collect::<Vec<_>>();
        assert_eq!(slugs(&lib), ["app", "app-2", "app-3"], "as they always were, by their order");
        lib.folders.remove(0);
        assert_eq!(slugs(&lib), ["app-2", "app-3"]);
        // A new one takes none listed and none on disk.
        let taken: BTreeSet<String> = lib.folders.iter().map(|f| f.slug.clone()).chain(["app".to_string()]).collect();
        assert_eq!(fresh_slug("E:/d/app", &taken), "app-4");
        lib.folders[0].slug = "../x".into();
        lib.folders[1].slug = "app-2".into();
        assert_eq!(slugs(&lib), ["app", "app-2"], "a bad one is made anew, past the one kept");
    }

    /// A link on the way to a plugin's manifest: neither written through nor
    /// taken out through.
    #[cfg(unix)]
    #[test]
    fn a_manifest_behind_a_link_is_left_alone() {
        let f = Fixture::new("manifest-link");
        f.skill("review", "v1");
        let (_, folder) = repo_folder(&f, "repo", &["review"]);
        let lib = LibrarySettings { folders: vec![folder], ..Default::default() };
        let elsewhere = f.base.join("elsewhere");
        std::fs::create_dir_all(elsewhere.join("skills").join("x")).unwrap();
        std::fs::write(elsewhere.join("skills").join("x").join("SKILL.md"), "x").unwrap();
        let plugin = codex_plugin_dir(&f.accounts[1], "repo");
        std::fs::create_dir_all(plugin.join("skills").join("review")).unwrap();
        std::fs::write(plugin.join("skills").join("review").join("SKILL.md"), "v1").unwrap();
        std::os::unix::fs::symlink(&elsewhere, plugin.join(".codex-plugin")).unwrap();
        codex_manifests(&lib, &f.accounts);
        assert!(!elsewhere.join("plugin.json").exists(), "not written through the link");
        std::fs::remove_dir_all(plugin.join("skills")).unwrap();
        std::fs::write(elsewhere.join("plugin.json"), "theirs").unwrap();
        codex_manifests(&lib, &f.accounts);
        assert_eq!(std::fs::read_to_string(elsewhere.join("plugin.json")).unwrap(), "theirs", "nor taken out through it");
    }

    #[test]
    fn a_session_in_a_folder_or_a_worktree_of_it_is_given_its_skills() {
        let f = Fixture::new("folder-args");
        f.skill("review", "v1");
        let (repo, folder) = repo_folder(&f, "repo", &["review"]);
        let lib = LibrarySettings { folders: vec![folder], ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        codex_manifests(&lib, &f.accounts);
        let repo_s = repo.to_string_lossy().into_owned();
        let wait = f.dirs.folders().join("repo").join("work");

        let args = folder_args(&f.dirs, &lib, &f.accounts[0], &repo_s).args;
        assert_eq!(args, ["--add-dir".to_string(), wait.to_string_lossy().into_owned()]);
        let sub = repo.join("src").to_string_lossy().into_owned();
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[0], &sub).args, args, "a folder inside it is in it");
        let claude_wt = repo.join(".claude/worktrees/x").to_string_lossy().into_owned();
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[0], &claude_wt).args, args);

        // Made for a session even with nothing in it yet: one added later
        // reaches only a session that had its `.claude/skills` from the start.
        let empty = LibrarySettings { folders: vec![LibraryFolder { path: repo_s.clone(), ..Default::default() }], ..Default::default() };
        let home = folder_args(&f.dirs, &empty, &f.accounts[2], &repo_s).args;
        assert_eq!(home[1], f.dirs.folders().join("repo").join("home").to_string_lossy());
        assert!(f.dirs.folders().join("repo/home/.claude/skills").is_dir());

        // Codex, in a worktree it made of the repository: known by its `.git` file.
        let wt = f.account(1).join("worktrees/0a1b/repo");
        write_file(&wt.join(".git"), format!("gitdir: {}\n", repo.join(".git").join("worktrees").join("repo").display()).as_bytes())
            .unwrap();
        let codex = ["-c".to_string(), "plugins.repo@luna.enabled=true".to_string()];
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[1], &repo_s).args, codex);
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[1], &wt.to_string_lossy()).args, codex);
        f.run(&empty, &mut ledger, true);
        codex_manifests(&empty, &f.accounts);
        assert!(folder_args(&f.dirs, &empty, &f.accounts[1], &repo_s).args.is_empty(), "no plugin, nothing to turn on");

        let elsewhere = f.base.join("elsewhere").to_string_lossy().into_owned();
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[0], &elsewhere), SessionSkills::default());
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[1], &elsewhere), SessionSkills::default());
    }

    #[test]
    fn a_folder_keeps_a_skill_from_its_sessions_and_one_inside_it_takes_it_back() {
        let f = Fixture::new("folder-off");
        f.skill("review", "v1");
        f.skill("notes", "v1");
        // `work` gives notes and keeps review; `work/repo` inside it keeps notes.
        let outer = f.base.join("work");
        std::fs::create_dir_all(&outer).unwrap();
        let outer_folder = LibraryFolder {
            path: outer.to_string_lossy().into_owned(),
            skills: BTreeMap::from([("notes".into(), all()), ("review".into(), Vec::new())]),
            ..Default::default()
        };
        let (repo, mut inner) = repo_folder(&f, "work/repo", &[]);
        inner.skills.insert("notes".into(), Vec::new());
        let lib = LibrarySettings {
            skills: BTreeMap::from([("review".into(), all()), ("notes".into(), reach(&["claude/work"]))]),
            folders: vec![outer_folder, inner],
            ..Default::default()
        };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        codex_manifests(&lib, &f.accounts);
        assert!(f.account(0).join("skills/review/SKILL.md").is_file(), "the account's copy stays where it is");
        assert!(f.dirs.folders().join("work/home/.claude/skills/notes/SKILL.md").is_file());

        // In the outer folder: review hidden, its copy the account's own.
        let other = outer.join("other").to_string_lossy().into_owned();
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[0], &other).hidden, ["review"]);
        let codex = folder_args(&f.dirs, &lib, &f.accounts[1], &other).args;
        assert_eq!(codex, ["-c", "plugins.work@luna.enabled=true", "-c", "skills.config=[{name=\"review\",enabled=false}]"]);

        // In the repository: notes too, the outer folder's copy by the name
        // each CLI lists it under, or the account's own.
        let repo_s = repo.to_string_lossy().into_owned();
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[2], &repo_s).hidden, ["notes", "review"]);
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[0], &repo_s).hidden, ["notes", "review"]);
        let codex = folder_args(&f.dirs, &lib, &f.accounts[1], &repo_s).args;
        assert_eq!(
            codex.last().map(String::as_str),
            Some("skills.config=[{name=\"review\",enabled=false},{name=\"work:notes\",enabled=false}]")
        );

        // Ticked in the repository again, the closer folder decides.
        let mut lib = lib;
        lib.folders[1] = LibraryFolder { skills: given(&["review", "notes"]), ..lib.folders[1].clone() };
        f.run(&lib, &mut ledger, true);
        assert!(folder_args(&f.dirs, &lib, &f.accounts[2], &repo_s).hidden.is_empty());
        let elsewhere = f.base.join("elsewhere").to_string_lossy().into_owned();
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[0], &elsewhere), SessionSkills::default());
    }

    #[test]
    fn a_folder_says_what_its_sessions_get_of_a_skill_it_names_not() {
        let f = Fixture::new("folder-inherited");
        let outer = f.base.join("work").to_string_lossy().into_owned();
        let inner = f.base.join("work").join("repo").to_string_lossy().into_owned();
        let lib = LibrarySettings {
            skills: BTreeMap::from([("review".into(), all()), ("notes".into(), reach(&["claude/work"]))]),
            folders: vec![
                LibraryFolder { path: outer.clone(), skills: BTreeMap::from([("review".into(), reach(&["codex"]))]), ..Default::default() },
                LibraryFolder { path: inner.clone(), ..Default::default() },
            ],
            ..Default::default()
        };
        let (o, i) = (&lib.folders[0], &lib.folders[1]);
        assert_eq!(
            inherited(&lib, &f.accounts, o, "review"),
            ["claude/work", "codex/home", "claude/home"],
            "every account has it as a whole"
        );
        assert_eq!(inherited(&lib, &f.accounts, i, "review"), ["codex/home"], "the folder it lies in gives it to Codex alone");
        assert_eq!(inherited(&lib, &f.accounts, o, "notes"), ["claude/work"]);
        assert!(inherited(&lib, &f.accounts, o, "tidy").is_empty(), "meant for none, given nowhere");
    }

    /// One account everywhere, another only in one folder, and none of them
    /// in another: what a reach of the folder's own says, account by account.
    #[test]
    fn a_folder_gives_a_skill_to_some_accounts_and_keeps_it_from_others() {
        let f = Fixture::new("folder-reach");
        f.skill("review", "v1");
        let (app, mut app_folder) = repo_folder(&f, "app", &[]);
        let (web, mut web_folder) = repo_folder(&f, "web", &[]);
        // claude/work everywhere; claude/home and codex too, in app; in web,
        // Codex alone.
        app_folder.skills.insert("review".into(), reach(&["claude", "codex"]));
        web_folder.skills.insert("review".into(), reach(&["codex"]));
        let lib = LibrarySettings {
            skills: BTreeMap::from([("review".into(), reach(&["claude/work"]))]),
            folders: vec![app_folder, web_folder],
            ..Default::default()
        };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        codex_manifests(&lib, &f.accounts);
        assert!(f.account(0).join("skills/review/SKILL.md").is_file());
        assert!(!f.account(2).join("skills/review").exists());
        assert!(f.dirs.folders().join("app/home/.claude/skills/review/SKILL.md").is_file());
        assert!(!f.dirs.folders().join("app/work/.claude/skills/review").exists(), "work has it as a whole");
        assert!(!f.dirs.folders().join("web/home/.claude/skills/review").exists());
        assert!(f.account(1).join("plugins/cache/luna/app/local/skills/review/SKILL.md").is_file());
        assert!(f.account(1).join("plugins/cache/luna/web/local/skills/review/SKILL.md").is_file());

        let (app_s, web_s) = (app.to_string_lossy().into_owned(), web.to_string_lossy().into_owned());
        assert!(folder_args(&f.dirs, &lib, &f.accounts[0], &app_s).hidden.is_empty());
        assert!(folder_args(&f.dirs, &lib, &f.accounts[2], &app_s).hidden.is_empty());
        // In web the account that has it as a whole does not get it; Codex does.
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[0], &web_s).hidden, ["review"]);
        assert!(folder_args(&f.dirs, &lib, &f.accounts[2], &web_s).hidden.is_empty(), "nothing of it to hide");
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[1], &web_s).args, ["-c", "plugins.web@luna.enabled=true"]);
    }

    #[test]
    fn a_folder_gives_its_claude_sessions_other_instructions_or_none() {
        let f = Fixture::new("folder-instructions");
        f.instructions("main", "# Main\n");
        f.instructions("terse", "# Terse\n");
        f.instructions("blank", "");
        let (repo, mut folder) = repo_folder(&f, "repo", &[]);
        // work: none; every other Claude Code account: terse; Codex: none,
        // which it has no way to take.
        folder.instructions =
            BTreeMap::from([("claude/work".into(), None), ("claude".into(), Some("terse".into())), ("codex/home".into(), None)]);
        let mut lib = LibrarySettings { instructions: first(all()), folders: vec![folder], ..Default::default() };
        let repo_s = repo.join("src").to_string_lossy().into_owned();
        let claude_md = |i: usize| format!("{}/CLAUDE.md", f.account(i).to_string_lossy().replace('\\', "/"));
        let appended = |s: &SessionSkills| s.instructions.clone();

        let work = folder_args(&f.dirs, &lib, &f.accounts[0], &repo_s);
        assert_eq!(work.excluded, [claude_md(0)]);
        assert_eq!(appended(&work), None);
        let home = folder_args(&f.dirs, &lib, &f.accounts[2], &repo_s);
        assert_eq!(home.excluded, [claude_md(2)]);
        assert_eq!(appended(&home), Some(f.dirs.instructions_file("terse")));
        let codex = folder_args(&f.dirs, &lib, &f.accounts[1], &repo_s);
        assert!(codex.excluded.is_empty() && codex.args.is_empty());
        let elsewhere = f.base.join("elsewhere").to_string_lossy().into_owned();
        assert!(folder_args(&f.dirs, &lib, &f.accounts[0], &elsewhere).excluded.is_empty());

        // What the account's file holds already: nothing to change — but a
        // copy changed there is no longer what was picked. Blank: none. Gone
        // from the library: none.
        lib.folders[0].instructions =
            BTreeMap::from([("claude/work".into(), Some("main".into())), ("claude/home".into(), Some("blank".into()))]);
        f.run(&lib, &mut Ledger::default(), true);
        assert_eq!(f.read(0, "CLAUDE.md"), "# Main\n");
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[0], &repo_s), SessionSkills { args: work.args.clone(), ..Default::default() });
        std::fs::write(f.account(0).join("CLAUDE.md"), "# Main, changed here\n").unwrap();
        let changed = folder_args(&f.dirs, &lib, &f.accounts[0], &repo_s);
        assert_eq!((changed.excluded.clone(), appended(&changed)), (vec![claude_md(0)], Some(f.dirs.instructions_file("main"))));
        let blank = folder_args(&f.dirs, &lib, &f.accounts[2], &repo_s);
        assert_eq!((blank.excluded.len(), appended(&blank)), (1, None));
        lib.folders[0].instructions = BTreeMap::from([("claude".into(), Some("gone".into()))]);
        let gone = folder_args(&f.dirs, &lib, &f.accounts[2], &repo_s);
        assert_eq!((gone.excluded.len(), appended(&gone)), (1, None));

        // A folder inside it decides for the accounts it names; the outer one
        // for the rest.
        let inner = LibraryFolder {
            path: repo.join("src").to_string_lossy().into_owned(),
            instructions: BTreeMap::from([("claude/home".into(), Some("main".into()))]),
            ..Default::default()
        };
        lib.folders[0].instructions = BTreeMap::from([("claude".into(), None)]);
        lib.folders.push(inner);
        std::fs::write(f.account(2).join("CLAUDE.md"), "# Main\n").unwrap();
        assert!(folder_args(&f.dirs, &lib, &f.accounts[2], &repo_s).excluded.is_empty());
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[0], &repo_s).excluded, [claude_md(0)]);

        // Renamed and deleted accounts take their entries along.
        rename_account(&mut lib, "claude/home", Some("claude/personal"));
        assert_eq!(lib.folders[1].instructions.keys().collect::<Vec<_>>(), ["claude/personal"]);
        rename_account(&mut lib, "claude/personal", None);
        assert!(lib.folders[1].instructions.is_empty());
    }

    /// A folder inside a repository holds in that folder of each of its
    /// worktrees, wherever the worktree lies.
    #[test]
    fn a_folder_inside_a_repository_holds_in_its_worktrees_too() {
        let f = Fixture::new("folder-in-worktree");
        f.skill("review", "v1");
        let (repo, _) = repo_folder(&f, "repo", &[]);
        let src = LibraryFolder {
            path: repo.join("src").to_string_lossy().into_owned(),
            skills: BTreeMap::from([("review".into(), Vec::new())]),
            ..Default::default()
        };
        let lib = LibrarySettings { skills: skills(&["review"]), folders: vec![src], ..Default::default() };
        f.run(&lib, &mut Ledger::default(), true);
        // A Codex worktree of it, in the account's own folder.
        let wt = f.account(1).join("worktrees/0a1b/repo");
        write_file(&wt.join(".git"), format!("gitdir: {}\n", repo.join(".git").join("worktrees").join("repo").display()).as_bytes())
            .unwrap();
        std::fs::create_dir_all(wt.join("src")).unwrap();
        let hide = ["-c".to_string(), "skills.config=[{name=\"review\",enabled=false}]".to_string()];
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[1], &wt.join("src").to_string_lossy()).args, hide);
        assert!(
            folder_args(&f.dirs, &lib, &f.accounts[1], &wt.to_string_lossy()).args.is_empty(),
            "the worktree's root is the repository's"
        );
    }

    /// Where an outer folder and one inside it both give an account a skill,
    /// the outer one's copy serves the sessions of both.
    #[test]
    fn folders_one_inside_another_give_an_account_one_copy() {
        let f = Fixture::new("folder-nested-copy");
        f.skill("review", "v1");
        let (repo, outer) = repo_folder(&f, "repo", &["review"]);
        std::fs::create_dir_all(repo.join("web")).unwrap();
        let inner =
            LibraryFolder { path: repo.join("web").to_string_lossy().into_owned(), skills: given(&["review"]), ..Default::default() };
        let mut lib = LibrarySettings { folders: vec![outer, inner], ..Default::default() };
        f.run(&lib, &mut Ledger::default(), true);
        codex_manifests(&lib, &f.accounts);
        assert!(f.dirs.folders().join("repo/work/.claude/skills/review/SKILL.md").is_file());
        assert!(!f.dirs.folders().join("web/work/.claude/skills/review").exists());
        assert!(!f.account(1).join("plugins/cache/luna/web").exists());
        let web = repo.join("web").to_string_lossy().into_owned();
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[1], &web).args, ["-c", "plugins.repo@luna.enabled=true"]);

        // Kept from the account outside, given again inside: the inside's own.
        lib.folders[0].skills.insert("review".into(), reach(&["codex"]));
        f.run(&lib, &mut Ledger::default(), true);
        assert!(f.dirs.folders().join("web/work/.claude/skills/review/SKILL.md").is_file());
        assert!(folder_args(&f.dirs, &lib, &f.accounts[0], &web).hidden.is_empty());
    }

    #[test]
    fn a_broken_skill_is_laid_out_nowhere_and_its_copies_stay() {
        let f = Fixture::new("broken");
        f.skill("review", "v1");
        let lib = LibrarySettings { skills: skills(&["review"]), ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        std::fs::write(f.dirs.skills().join("review/SKILL.md"), "no front matter").unwrap();
        let p = f.run(&lib, &mut ledger, true);
        assert!(p.places.iter().all(|x| x.status == Status::Failed));
        assert!(f.account(0).join("skills/review/SKILL.md").is_file(), "a draft mid-edit takes nothing out");
        assert!(std::fs::read_to_string(f.account(0).join("skills/review/SKILL.md")).unwrap().contains("v1"));
    }

    #[cfg(windows)]
    #[test]
    fn another_spelling_of_a_copy_is_the_same_copy() {
        let mut f = Fixture::new("case");
        f.instructions("main", "Be brief.\n");
        let lib = LibrarySettings { instructions: first(all()), ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        // The account renamed in case only: the same folder, spelled anew.
        f.accounts[0].path = f.accounts[0].path.to_uppercase();
        let claude_md = f.account(0).join("CLAUDE.md");
        let p = f.run(&lib, &mut ledger, true);
        assert_eq!(Fixture::status(&p, &claude_md), Status::Current);
        assert!(claude_md.is_file(), "the old spelling's note takes nothing out");
        let noted: Vec<String> = ledger.written.keys().filter(|k| ident(Path::new(k)) == ident(&claude_md)).cloned().collect();
        assert_eq!(noted, vec![claude_md.to_string_lossy().into_owned()]);
    }

    #[test]
    fn a_temp_file_left_by_a_write_is_no_part_of_a_skill() {
        let f = Fixture::new("tmp");
        f.skill("review", "v1");
        let lib = LibrarySettings { skills: skills(&["review"]), ..Default::default() };
        // Left in the library by a save that died: not laid out.
        std::fs::write(f.dirs.skills().join("review/SKILL.md.1-0.luna-tmp"), "half").unwrap();
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        let copy = f.account(0).join("skills/review");
        assert!(copy.join("SKILL.md").is_file());
        assert!(!copy.join("SKILL.md.1-0.luna-tmp").exists());
        // Left beside a copy: the copy is still as Luna wrote it.
        std::fs::write(copy.join("SKILL.md.2-0.luna-tmp"), "half").unwrap();
        let p = f.run(&lib, &mut ledger, false);
        assert_eq!(Fixture::status(&p, &copy), Status::Current);
    }

    #[test]
    fn skipped_places_are_neither_written_nor_reported() {
        let f = Fixture::new("skip");
        f.instructions("main", "Be brief.\n");
        let claude_md = f.account(0).join("CLAUDE.md").to_string_lossy().into_owned();
        let lib = LibrarySettings { instructions: first(all()), skipped: vec![claude_md.clone()], ..Default::default() };
        let p = f.run(&lib, &mut Ledger::default(), true);
        assert_eq!(Fixture::status(&p, Path::new(&claude_md)), Status::Skipped);
        assert!(!Path::new(&claude_md).exists());
        assert!(f.account(1).join("AGENTS.md").is_file());
    }

    #[test]
    fn blank_instructions_lay_nothing_out() {
        let f = Fixture::new("blank");
        f.instructions("main", " \n");
        let lib = LibrarySettings { instructions: first(all()), ..Default::default() };
        let p = f.run(&lib, &mut Ledger::default(), true);
        assert!(p.places.is_empty());
        assert!(!f.account(0).join("CLAUDE.md").exists());
    }

    #[test]
    fn backups_and_blobs_keep_what_a_diff_and_an_undo_need() {
        let f = Fixture::new("blobs");
        f.instructions("main", "Be brief.\n");
        let lib = LibrarySettings { instructions: first(all()), ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        let dest = f.account(0).join("CLAUDE.md").to_string_lossy().into_owned();
        let base = blob(&f.dirs, &ledger.written[&dest][""]).unwrap();
        assert_eq!(base, b"Be brief.\n");

        let saved = back_up(&f.dirs, Path::new(&dest), &Files::from([(String::new(), b"mine".to_vec())])).unwrap();
        assert_eq!(std::fs::read(saved.join("CLAUDE.md")).unwrap(), b"mine");
        // A skill's folder, a `from.txt` of its own in it: kept beside Luna's.
        let skill = f.dirs.root.join("skills").join("notes");
        let files = Files::from([("SKILL.md".to_string(), b"s".to_vec()), ("from.txt".to_string(), b"its own".to_vec())]);
        let saved = back_up(&f.dirs, &skill, &files).unwrap();
        assert_eq!(std::fs::read(saved.join("notes").join("from.txt")).unwrap(), b"its own");
        assert_eq!(std::fs::read(saved.join("from.txt")).unwrap(), skill.to_string_lossy().as_bytes());

        // A blob no entry names goes at the sweep.
        ledger.written.clear();
        sweep_blobs(&f.dirs, &ledger);
        assert!(blob(&f.dirs, &hash(b"Be brief.\n")).is_none());
    }

    #[test]
    fn the_one_instructions_file_of_old_moves_to_main_once() {
        let f = Fixture::new("migrate");
        let old = f.dirs.root.join(OLD_INSTRUCTIONS);
        let main = f.dirs.instructions_file(FIRST_INSTRUCTIONS);
        std::fs::write(&old, "Be brief.\n").unwrap();
        assert!(read_instructions(&f.dirs).is_err(), "not moved yet: no instructions is not what the library has");

        migrate_instructions(&f.dirs);
        assert!(!old.exists());
        assert_eq!(std::fs::read_to_string(&main).unwrap(), "Be brief.\n");
        let found = read_instructions(&f.dirs).unwrap();
        assert_eq!(found[FIRST_INSTRUCTIONS], Ok(Some(b"Be brief.\n".to_vec())));
        migrate_instructions(&f.dirs);
        assert_eq!(std::fs::read_to_string(&main).unwrap(), "Be brief.\n", "a second run moves nothing");

        // Both there: neither is touched, and the folder's is the one read.
        std::fs::write(&old, "Older.\n").unwrap();
        migrate_instructions(&f.dirs);
        assert_eq!(std::fs::read_to_string(&old).unwrap(), "Older.\n");
        assert_eq!(std::fs::read_to_string(&main).unwrap(), "Be brief.\n");
        assert_eq!(read_instructions(&f.dirs).unwrap()[FIRST_INSTRUCTIONS], Ok(Some(b"Be brief.\n".to_vec())));
    }

    #[test]
    fn settings_of_one_instructions_read_as_mains() {
        let read = |json: &str| serde_json::from_str::<LibrarySettings>(json).unwrap();
        assert_eq!(read(r#"{"instructions": ["claude", "codex/home"]}"#).instructions, first(reach(&["claude", "codex/home"])));
        assert_eq!(read(r#"{"instructions": true}"#).instructions, first(all()));
        assert!(read(r#"{"instructions": []}"#).instructions.is_empty());
        assert!(read(r#"{"instructions": false}"#).instructions.is_empty());
        let now = read(r#"{"instructions": {"main": ["all"], "work": ["claude/work"]}, "noInstructions": ["codex/home"]}"#);
        assert_eq!(now.instructions["work"], reach(&["claude/work"]));
        assert_eq!(now.no_instructions, reach(&["codex/home"]));
        let back = read(&serde_json::to_string(&now).unwrap());
        assert_eq!((back.instructions, back.no_instructions), (now.instructions, now.no_instructions));
    }

    #[test]
    fn an_account_gets_the_instructions_that_name_it_most_closely() {
        let f = Fixture::new("closest");
        f.instructions("main", "For everyone.\n");
        f.instructions("claude", "For Claude Code.\n");
        f.instructions("work", "For work.\n");
        let mut lib = LibrarySettings {
            instructions: BTreeMap::from([
                ("main".into(), all()),
                ("claude".into(), reach(&["claude"])),
                ("work".into(), reach(&["claude/work"])),
            ]),
            ..Default::default()
        };
        let mut ledger = Ledger::default();
        let p = f.run(&lib, &mut ledger, true);
        assert_eq!(f.read(0, "CLAUDE.md"), "For work.\n", "named beats a provider and all");
        assert_eq!(f.read(2, "CLAUDE.md"), "For Claude Code.\n", "a provider beats all");
        assert_eq!(f.read(1, "AGENTS.md"), "For everyone.\n");
        assert_eq!(p.places.iter().filter(|x| x.instructions.is_some()).count(), 3, "one instructions file per account");

        // Named as getting none: Luna's copy goes, though `all` takes it in.
        lib.no_instructions = reach(&["claude/home"]);
        f.run(&lib, &mut ledger, true);
        assert!(!f.account(2).join("CLAUDE.md").exists());

        // Instructions not in the library compete for nothing: the next
        // closest has the account.
        std::fs::remove_file(f.dirs.instructions_file("work")).unwrap();
        f.run(&lib, &mut ledger, true);
        assert_eq!(f.read(0, "CLAUDE.md"), "For Claude Code.\n");

        // Two at one closeness (settings edited by hand): none wins over
        // instructions, the first by name over the rest.
        lib.instructions.insert("alpha".into(), reach(&["claude/home"]));
        lib.instructions.insert("beta".into(), reach(&["codex/home"]));
        lib.instructions.insert("gamma".into(), reach(&["codex/home"]));
        let have = |_: &str| true;
        assert_eq!(chosen(&lib, have, &f.accounts[2]), None);
        assert_eq!(chosen(&lib, have, &f.accounts[1]), Some("beta"));
    }

    #[test]
    fn giving_instructions_a_reach_takes_it_from_the_others() {
        let mut lib = LibrarySettings { instructions: first(reach(&["all", "codex/home"])), ..Default::default() };
        give_instructions(&mut lib, Some("work"), reach(&["claude/work"]));
        assert_eq!(lib.instructions["main"], reach(&["all", "codex/home"]), "other entries stay");
        assert_eq!(lib.instructions["work"], reach(&["claude/work"]));

        // A provider and an account of it are two entries: both stand.
        give_instructions(&mut lib, Some("cli"), reach(&["claude"]));
        assert_eq!(lib.instructions["work"], reach(&["claude/work"]));

        // Main named for the work account: work loses it, and goes nowhere.
        give_instructions(&mut lib, Some("main"), reach(&["all", "codex/home", "claude/work", "claude/work"]));
        assert_eq!(lib.instructions["main"], reach(&["all", "codex/home", "claude/work"]), "kept beside all, once");
        assert!(!lib.instructions.contains_key("work"));

        // None for an account takes it from instructions, and back.
        give_instructions(&mut lib, None, reach(&["codex/home"]));
        assert_eq!(lib.instructions["main"], reach(&["all", "claude/work"]));
        give_instructions(&mut lib, Some("cli"), reach(&["claude", "codex/home"]));
        assert!(lib.no_instructions.is_empty());

        // `all` is one instructions' at a time.
        give_instructions(&mut lib, Some("cli"), reach(&["all"]));
        assert_eq!(lib.instructions["main"], reach(&["claude/work"]));
        assert_eq!(lib.instructions["cli"], all());
        give_instructions(&mut lib, Some("cli"), Vec::new());
        assert!(!lib.instructions.contains_key("cli"));
    }

    #[test]
    fn a_new_winner_replaces_lunas_copy_and_never_a_changed_one() {
        let f = Fixture::new("winner");
        f.instructions("main", "For everyone.\n");
        f.instructions("work", "For work.\n");
        let mut lib = LibrarySettings { instructions: first(all()), ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        assert_eq!(f.read(0, "CLAUDE.md"), "For everyone.\n");

        give_instructions(&mut lib, Some("work"), reach(&["claude/work"]));
        let p = f.run(&lib, &mut ledger, false);
        assert_eq!(Fixture::status(&p, &f.account(0).join("CLAUDE.md")), Status::Behind);
        f.run(&lib, &mut ledger, true);
        assert_eq!(f.read(0, "CLAUDE.md"), "For work.\n");

        // Changed by hand, then given other instructions: asked, not replaced.
        std::fs::write(f.account(2).join("CLAUDE.md"), "Mine now.\n").unwrap();
        give_instructions(&mut lib, Some("work"), reach(&["claude/work", "claude/home"]));
        let p = f.run(&lib, &mut ledger, true);
        let place = p.places.iter().find(|x| Path::new(&x.dest) == f.account(2).join("CLAUDE.md")).unwrap();
        assert_eq!((place.status, place.both, place.instructions.as_deref()), (Status::Changed, true, Some("work")));
        assert_eq!(f.read(2, "CLAUDE.md"), "Mine now.\n");

        // Back to the first: Luna's copy follows again.
        give_instructions(&mut lib, Some("work"), Vec::new());
        f.run(&lib, &mut ledger, true);
        assert_eq!(f.read(0, "CLAUDE.md"), "For everyone.\n");
    }

    #[test]
    fn copies_laid_out_before_several_instructions_stay_lunas() {
        let f = Fixture::new("upgrade");
        // As the Luna of one instructions file left things: the file in the
        // library's root, a reach of its own, its copies in the ledger, one
        // of them since changed by hand.
        std::fs::write(f.dirs.root.join(OLD_INSTRUCTIONS), "Be brief.\n").unwrap();
        let mut ledger = Ledger::default();
        for (i, file) in [(0, "CLAUDE.md"), (1, "AGENTS.md"), (2, "CLAUDE.md")] {
            let dest = f.account(i).join(file);
            std::fs::write(&dest, "Be brief.\n").unwrap();
            ledger.written.insert(dest.to_string_lossy().into_owned(), hashes(&Files::from([(String::new(), b"Be brief.\n".to_vec())])));
        }
        std::fs::write(f.account(2).join("CLAUDE.md"), "Be brief. Mine.\n").unwrap();
        let before = ledger.written.clone();
        let lib: LibrarySettings = serde_json::from_str(r#"{"instructions": ["all"]}"#).unwrap();

        // Before the move, nothing is taken out for want of instructions.
        let p = f.run(&lib, &mut ledger, true);
        assert!(p.places.iter().all(|x| x.status == Status::Failed), "{:?}", p.places);
        assert_eq!(f.read(0, "CLAUDE.md"), "Be brief.\n");

        migrate_instructions(&f.dirs);
        let p = f.run(&lib, &mut ledger, true);
        assert_eq!(Fixture::status(&p, &f.account(0).join("CLAUDE.md")), Status::Current);
        assert_eq!(Fixture::status(&p, &f.account(1).join("AGENTS.md")), Status::Current);
        assert_eq!(Fixture::status(&p, &f.account(2).join("CLAUDE.md")), Status::Changed, "changed, not someone else's");
        assert_eq!(ledger.written, before);
    }

    #[test]
    fn instructions_taken_from_an_account_are_named_after_it() {
        let f = Fixture::new("names");
        assert_eq!(new_instructions_name(&f.dirs, "Work Laptop"), "work-laptop");
        assert_eq!(new_instructions_name(&f.dirs, "--"), "instructions");
        f.instructions("work", "x");
        assert_eq!(new_instructions_name(&f.dirs, "work"), "work-2");
        assert!(check_name(&new_instructions_name(&f.dirs, "Ünïcode.name")).is_ok());
    }
}
