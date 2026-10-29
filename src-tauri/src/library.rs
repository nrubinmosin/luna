//! The user's own skills and one instructions file, kept in Luna's library
//! and laid out where the CLIs read them: into the accounts each is meant for
//! — all of them, a provider's, or ones picked by name; a skill as
//! `skills/<name>`, the instructions as `CLAUDE.md` (Claude Code) or
//! `AGENTS.md` (Codex) in its root — and into folders the user names, a
//! skill as `<folder>/<name>`. How a CLI gets to a folder's skills (a link
//! into `.claude/skills` or `.agents/skills`, a git ignore) is the user's.
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
use crate::settings::Reach;
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

/// One pass at a time: the window can fire several changes in a row, and two
/// passes writing the same copy would each read the other's half.
static LOCK: Mutex<()> = Mutex::new(());

const INSTRUCTIONS: &str = "instructions.md";
const SKILL_FILE: &str = "SKILL.md";
/// Backups older than this go at the next pass.
const BACKUP_DAYS: u64 = 30;

/// Where the instructions go in an account of this provider.
fn instructions_name(provider: Provider) -> &'static str {
    match provider {
        Provider::Claude => "CLAUDE.md",
        Provider::Codex => "AGENTS.md",
    }
}

// ------------------------------------------------------------------ reach --

const ALL: &str = "all";

/// Whether a reach takes in this account.
fn reaches(reach: &[String], a: &AccountInfo) -> bool {
    let provider = a.provider.as_str();
    reach.iter().any(|r| r == ALL || r == provider || r.split_once('/') == Some((provider, a.name.as_str())))
}

/// A reach as the user ticked it, checked and made the shortest that takes in
/// the same accounts: `all` alone, a provider without its own accounts,
/// nothing twice.
fn tidy_reach(reach: Vec<String>) -> Result<Reach, String> {
    let provider = |p: &str| Provider::ALL.iter().any(|x| x.as_str() == p);
    for r in &reach {
        let fits = r == ALL || provider(r) || r.split_once('/').is_some_and(|(p, n)| provider(p) && !n.is_empty() && !n.contains('/'));
        if !fits {
            return Err(format!("{r} is not all, a provider or provider/account"));
        }
    }
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

/// An account renamed (`to`) or deleted (None), as `provider/name`: every
/// reach that named it follows.
pub fn rename_account(lib: &mut crate::settings::LibrarySettings, from: &str, to: Option<&str>) {
    for reach in std::iter::once(&mut lib.instructions).chain(lib.skills.values_mut()) {
        match to {
            Some(to) => reach.iter_mut().filter(|r| *r == from).for_each(|r| *r = to.to_string()),
            None => reach.retain(|r| r != from),
        }
    }
    lib.skills.retain(|_, reach| !reach.is_empty());
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
        self.root.join(INSTRUCTIONS)
    }
    fn skills(&self) -> PathBuf {
        self.root.join("skills")
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
        write_file(&dirs.ledger(), text.as_bytes())
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
    /// The accounts it goes into.
    pub accounts: Reach,
}

/// A name both CLIs take for a skill, and the only kind Luna makes a folder of.
fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 64 {
        return Err("A skill's name is 1 to 64 characters".into());
    }
    if !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') || name.starts_with('-') {
        return Err("A skill's name is lowercase letters, digits and hyphens".into());
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
                    let unquoted = (v.len() >= 2 && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\''))))
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
        let entry = entry.map_err(&failed)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || !entry.file_type().map_err(&failed)?.is_dir() {
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
    for (rel, bytes) in files {
        if old.get(rel) != Some(bytes) {
            write_file(&join_rel(dest, rel), bytes)?;
        }
    }
    for rel in old.keys().filter(|r| !files.contains_key(*r)) {
        let p = join_rel(dest, rel);
        std::fs::remove_file(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    prune_empty(dest);
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
/// path (the whole path would outgrow Windows' limit), with the whole of it
/// written beside the files.
fn back_up(dirs: &Dirs, from: &Path, files: &Files) -> Result<PathBuf, String> {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S-%3f");
    let tail: Vec<String> = from.components().rev().take(2).map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    let tail: String = tail.into_iter().rev().collect::<Vec<_>>().join("-").chars().filter(|c| !matches!(c, ':' | '\\' | '/')).collect();
    let dir = dirs.backups().join(format!("{stamp}-{tail}"));
    for (rel, bytes) in files {
        let path = if rel.is_empty() { dir.join(from.file_name().unwrap_or_default()) } else { join_rel(&dir, rel) };
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

// ------------------------------------------------------------------- pass --

/// One copy the library wants somewhere.
struct Unit {
    /// The instructions file itself, or the skill's folder.
    dest: PathBuf,
    file: bool,
    /// None for the instructions.
    skill: Option<String>,
    target: Target,
    /// What the library has for it, or why it has nothing fit to lay out.
    source: Result<Files, String>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Target {
    Account { provider: Provider, name: String },
    Folder { path: String },
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
        let place = |status, both, problem| Place { dest: key.clone(), target: u.target.clone(), skill: u.skill.clone(), status, both, problem };
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
/// the accounts folder hold now.
fn units(dirs: &Dirs, lib: &crate::settings::LibrarySettings, accounts: &[AccountInfo], skills: &Result<Skills, String>) -> Vec<Unit> {
    let mut out = Vec::new();
    let account = |a: &AccountInfo| Target::Account { provider: a.provider, name: a.name.clone() };
    // A skill's files; while the skills will not list, every one a failure.
    let files_of = |name: &str| match skills {
        Ok(found) => found.get(name).map(|(_, files)| files.clone()),
        Err(e) => Some(Err(e.clone())),
    };
    if !lib.instructions.is_empty() {
        // No file is no instructions; one that will not read (held open
        // mid-save, say) is a failure like a broken skill's, and takes none of
        // its copies out.
        let source = match std::fs::read(dirs.instructions()) {
            Ok(text) if String::from_utf8_lossy(&text).trim().is_empty() => None,
            Ok(text) => Some(Ok(Files::from([(String::new(), text)]))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => Some(Err(format!("{}: {e}", dirs.instructions().display()))),
        };
        if let Some(source) = source {
            for a in accounts.iter().filter(|a| reaches(&lib.instructions, a)) {
                out.push(Unit {
                    dest: Path::new(&a.path).join(instructions_name(a.provider)),
                    file: true,
                    skill: None,
                    target: account(a),
                    source: source.clone(),
                });
            }
        }
    }
    for (name, reach) in &lib.skills {
        let Some(files) = files_of(name.as_str()) else { continue };
        for a in accounts.iter().filter(|a| reaches(reach, a)) {
            out.push(Unit {
                dest: Path::new(&a.path).join("skills").join(name),
                file: false,
                skill: Some(name.clone()),
                target: account(a),
                source: files.clone(),
            });
        }
    }
    for f in &lib.folders {
        let there = Path::new(&f.path).is_dir();
        for name in &f.skills {
            let Some(files) = files_of(name.as_str()) else { continue };
            out.push(Unit {
                dest: Path::new(&f.path).join(name),
                file: false,
                skill: Some(name.clone()),
                target: Target::Folder { path: f.path.clone() },
                source: if there { files } else { Err(format!("{} is not there", f.path)) },
            });
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
/// folder, which has its `skills/` where the library does; a folder named
/// inside the library), or one holding the library: its copy would be the
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
fn resolved(p: &Path) -> PathBuf {
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
    pub skills: Vec<String>,
    pub found: bool,
}

/// An account's instructions file Luna did not write: something to start the
/// library's from.
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
    /// The accounts the instructions go into.
    pub instructions: Reach,
    /// The library has instructions to lay out: its file is there and not blank.
    pub has_instructions: bool,
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

fn overview(act: bool) -> Result<Overview, String> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    overview_locked(act)
}

fn overview_locked(act: bool) -> Result<Overview, String> {
    let dirs = Dirs::current();
    let lib = crate::settings::get().library;
    let accounts = crate::accounts::list_accounts()?;
    let found = dirs.root.is_dir();
    let skills = if found { read_skills(&dirs) } else { Ok(BTreeMap::new()) };
    let mut ledger = Ledger::load(&dirs);
    // A library that is not there (a drive not plugged in, a folder moved)
    // wants nothing, and a pass on that would take every copy out.
    let units = if found { units(&dirs, &lib, &accounts, &skills) } else { Vec::new() };
    let p = pass(&dirs, &units, &lib.skipped, &mut ledger, act && found);
    if act && found {
        ledger.save(&dirs)?;
        sweep_blobs(&dirs, &ledger);
        sweep_backups(&dirs);
        if !p.kept.is_empty() {
            crate::log::info("library", &format!("left in place, changed since Luna wrote them: {}", p.kept.join(", ")));
        }
    }
    let found_instructions = accounts
        .iter()
        .map(|a| (a, Path::new(&a.path).join(instructions_name(a.provider))))
        .filter(|(_, dest)| dest.is_file() && !ledger.written.contains_key(&*dest.to_string_lossy()))
        .map(|(a, dest)| FoundFile { dest: dest.to_string_lossy().into_owned(), target: Target::Account { provider: a.provider, name: a.name.clone() } })
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
        has_instructions: std::fs::read_to_string(dirs.instructions()).is_ok_and(|t| !t.trim().is_empty()),
        skills: skills
            .unwrap_or_default()
            .into_values()
            .map(|(info, _)| SkillInfo { accounts: lib.skills.get(&info.name).cloned().unwrap_or_default(), ..info })
            .collect(),
        instructions: lib.instructions,
        accounts,
        folders: lib.folders.into_iter().map(|f| FolderInfo { found: Path::new(&f.path).is_dir(), path: f.path, skills: f.skills }).collect(),
        places: p.places,
        found_instructions,
        overridden,
        kept: p.kept,
    })
}

/// A settings change, then a pass to lay it out.
fn change(f: impl FnOnce(&mut crate::settings::LibrarySettings)) -> Result<Overview, String> {
    crate::settings::update(|s| f(&mut s.library))?;
    overview(true)
}

// --------------------------------------------------------------- commands --

/// What the library holds and where each copy stands, without writing.
#[tauri::command]
pub fn library_overview() -> Result<Overview, String> {
    overview(false)
}

/// Lays the library out: everything that needs no asking.
#[tauri::command]
pub fn library_sync() -> Result<Overview, String> {
    overview(true)
}

/// Points the library at another folder; empty = back to `<data>/library`.
/// Nothing is moved.
#[tauri::command]
pub fn library_set_source(path: String) -> Result<Overview, String> {
    let path = path.trim().to_string();
    if !path.is_empty() && !Path::new(&path).is_absolute() {
        return Err("The library's folder must be an absolute path".into());
    }
    crate::log::info("library", &format!("library folder: {}", if path.is_empty() { "<default>" } else { &path }));
    change(|l| l.source = (!path.is_empty()).then_some(path))
}

/// The accounts the instructions go into; empty = none.
#[tauri::command]
pub fn library_set_instructions(accounts: Vec<String>) -> Result<Overview, String> {
    let reach = tidy_reach(accounts)?;
    change(|l| l.instructions = reach)
}

/// The accounts a skill goes into; empty = none.
#[tauri::command]
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

/// A folder's skills; None stops laying anything out into it.
#[tauri::command]
pub fn library_set_folder(path: String, skills: Option<Vec<String>>) -> Result<Overview, String> {
    if !Path::new(&path).is_absolute() {
        return Err("A folder must be an absolute path".into());
    }
    if let Some(s) = &skills {
        s.iter().try_for_each(|n| check_name(n))?;
    }
    change(|l| {
        let at = l.folders.iter().position(|f| crate::pty::same_dir(&f.path, &path));
        match (at, skills) {
            (Some(i), None) => {
                l.folders.remove(i);
            }
            (Some(i), Some(s)) => l.folders[i].skills = s,
            (None, Some(s)) => l.folders.push(crate::settings::LibraryFolder { path, skills: s }),
            (None, None) => {}
        }
    })
}

/// Leaves a destination out of every pass, or takes it back in. Left out, it
/// is no longer Luna's copy: whatever is there stays, as anyone's file.
#[tauri::command]
pub fn library_skip(dest: String, on: bool) -> Result<Overview, String> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
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

/// Where a destination's copy comes from in the library, and whether it is one
/// file: a copy the library wants now, or an account's instructions file.
fn locate(dirs: &Dirs, dest: &str) -> Result<(PathBuf, bool, Option<String>), String> {
    let lib = crate::settings::get().library;
    let accounts = crate::accounts::list_accounts()?;
    let skills = read_skills(dirs);
    if let Some(u) = units(dirs, &lib, &accounts, &skills).into_iter().find(|u| u.dest.to_string_lossy() == dest) {
        let from = match &u.skill {
            Some(s) => dirs.skills().join(s),
            None => dirs.instructions(),
        };
        return Ok((from, u.file, u.skill));
    }
    if accounts.iter().any(|a| Path::new(&a.path).join(instructions_name(a.provider)).to_string_lossy() == dest) {
        return Ok((dirs.instructions(), true, None));
    }
    Err(format!("{dest} is not a place the library is laid out to"))
}

/// Settles a copy that changed, went or was never Luna's: `take` makes it the
/// library's (and so every other copy's), otherwise the library's goes back
/// in its place. Whichever side is replaced is backed up first.
#[tauri::command]
pub fn library_resolve(dest: String, take: bool) -> Result<Overview, String> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dirs = Dirs::current();
    let (from, file, skill) = locate(&dirs, &dest)?;
    let to = PathBuf::from(&dest);
    if in_library(&dirs, &to) {
        return Err(format!("{dest} is in the library itself"));
    }
    let mut ledger = Ledger::load(&dirs);
    let there = read_at(&to, file)?;
    let library = read_at(&from, file)?;
    if take {
        let there = there.ok_or("Nothing is there to take")?;
        if let Some(name) = &skill {
            let text = there.get(SKILL_FILE).ok_or("There is no SKILL.md there")?;
            check_skill(name, &String::from_utf8_lossy(text))?;
        }
        if let Some(old) = &library {
            back_up(&dirs, &from, old)?;
        }
        write_at(&from, file, &there)?;
        keep_blobs(&dirs, &there);
        ledger.written.insert(dest.clone(), hashes(&there));
        crate::log::info("library", &format!("took {dest} into the library"));
    } else {
        let library = library.ok_or("The library has nothing for it")?;
        if let Some(name) = &skill {
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
    // Taking an account's instructions file while they go nowhere means
    // laying them out into every account.
    if take && skill.is_none() && crate::settings::get().library.instructions.is_empty() {
        crate::settings::update(|s| s.library.instructions = vec![ALL.into()])?;
    }
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

#[tauri::command]
pub fn library_diff(dest: String) -> Result<Vec<FileDiff>, String> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dirs = Dirs::current();
    let (from, file, _) = locate(&dirs, &dest)?;
    let ledger = Ledger::load(&dirs);
    let base: Files = ledger
        .written
        .get(&dest)
        .map(|h| h.iter().filter_map(|(k, v)| Some((k.clone(), blob(&dirs, v)?))).collect())
        .unwrap_or_default();
    let library = read_at(&from, file)?.unwrap_or_default();
    let there = read_at(Path::new(&dest), file)?.unwrap_or_default();
    let paths: BTreeSet<&String> = base.keys().chain(library.keys()).chain(there.keys()).collect();
    let name = Path::new(&dest).file_name().unwrap_or_default().to_string_lossy().into_owned();
    Ok(paths
        .into_iter()
        .filter(|p| !(base.get(*p) == library.get(*p) && library.get(*p) == there.get(*p)))
        .map(|p| {
            let all = [base.get(p), library.get(p), there.get(p)];
            let binary = all.iter().flatten().any(|b| std::str::from_utf8(b).is_err());
            let text = |b: Option<&Vec<u8>>| if binary { b.map(|_| String::new()) } else { b.map(|b| String::from_utf8_lossy(b).into_owned()) };
            FileDiff { path: if p.is_empty() { name.clone() } else { p.clone() }, base: text(all[0]), library: text(all[1]), there: text(all[2]), binary }
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

fn library_path(dirs: &Dirs, skill: Option<&str>) -> Result<PathBuf, String> {
    match skill {
        Some(s) => {
            check_name(s)?;
            Ok(dirs.skills().join(s).join(SKILL_FILE))
        }
        None => Ok(dirs.instructions()),
    }
}

/// The library's instructions, or a skill's SKILL.md, for the editor.
#[tauri::command]
pub fn library_read(skill: Option<String>) -> Result<LibraryFile, String> {
    let path = library_path(&Dirs::current(), skill.as_deref())?;
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
#[tauri::command]
pub fn library_write(skill: Option<String>, text: String) -> Result<Overview, String> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = library_path(&Dirs::current(), skill.as_deref())?;
    write_file(&path, text.as_bytes())?;
    overview_locked(true)
}

/// Why this SKILL.md would not be laid out, or None when it would.
#[tauri::command]
pub fn library_check_skill(skill: String, text: String) -> Option<String> {
    check_skill(&skill, &text).err()
}

#[tauri::command]
pub fn library_new_skill(name: String) -> Result<Overview, String> {
    let name = name.trim().to_string();
    check_name(&name)?;
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
#[tauri::command]
pub fn library_delete_skill(name: String) -> Result<Overview, String> {
    check_name(&name)?;
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dirs = Dirs::current();
    let dir = dirs.skills().join(&name);
    if let Some(files) = read_at(&dir, false)? {
        back_up(&dirs, &dir, &files)?;
        std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    crate::settings::update(|s| {
        s.library.skills.remove(&name);
        for f in &mut s.library.folders {
            f.skills.retain(|n| *n != name);
        }
    })?;
    crate::log::info("library", &format!("skill {name} taken out of the library"));
    overview_locked(true)
}

/// Opens the library's folder, or a skill's, in the file manager — for the
/// files beside SKILL.md, which the editor does not cover.
#[tauri::command]
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
    use crate::settings::{LibraryFolder, LibrarySettings};

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
            let accounts = [(Provider::Claude, "anthropic", "work"), (Provider::Codex, "openai", "home")]
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
        fn account(&self, i: usize) -> PathBuf {
            PathBuf::from(&self.accounts[i].path)
        }
        fn run(&self, lib: &LibrarySettings, ledger: &mut Ledger, act: bool) -> Pass {
            let skills = read_skills(&self.dirs);
            let units = units(&self.dirs, lib, &self.accounts, &skills);
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

    #[test]
    fn a_reach_takes_in_all_a_provider_or_one_account() {
        let f = Fixture::new("reach");
        f.skill("review", "v1");
        std::fs::write(f.dirs.instructions(), "Be brief.\n").unwrap();
        let reach = |r: &[&str]| r.iter().map(|s| s.to_string()).collect::<Reach>();
        let lib = LibrarySettings {
            instructions: reach(&["codex"]),
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
            instructions: vec!["claude/work".into(), "codex".into()],
            skills: BTreeMap::from([("review".into(), vec!["claude/work".into()]), ("notes".into(), all())]),
            ..Default::default()
        };
        rename_account(&mut lib, "claude/work", Some("claude/office"));
        assert_eq!(lib.instructions, vec!["claude/office".to_string(), "codex".into()]);
        assert_eq!(lib.skills["review"], vec!["claude/office".to_string()]);
        rename_account(&mut lib, "claude/office", None);
        assert_eq!(lib.instructions, vec!["codex".to_string()]);
        assert!(!lib.skills.contains_key("review"), "a skill that goes nowhere any more is dropped");
        assert_eq!(lib.skills["notes"], all());
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
    fn a_pass_lays_out_once_and_keeps_up_with_the_library() {
        let f = Fixture::new("lay");
        f.skill("review", "v1");
        std::fs::write(f.dirs.instructions(), "Be brief.\n").unwrap();
        let lib = LibrarySettings { instructions: all(), skills: skills(&["review"]), ..Default::default() };
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
        std::fs::write(f.dirs.instructions(), "Be brief.\n").unwrap();
        let lib = LibrarySettings { instructions: all(), ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        let claude_md = f.account(0).join("CLAUDE.md");
        std::fs::write(&claude_md, "Be brief. And kind.\n").unwrap();

        let p = f.run(&lib, &mut ledger, true);
        let place = p.places.iter().find(|x| x.dest == claude_md.to_string_lossy()).unwrap();
        assert_eq!((place.status, place.both), (Status::Changed, false));
        assert_eq!(std::fs::read_to_string(&claude_md).unwrap(), "Be brief. And kind.\n");

        // The library changes too: still asked, and said that both did.
        std::fs::write(f.dirs.instructions(), "Be terse.\n").unwrap();
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
        let folder = f.base.join("repo/.shared-skills");
        std::fs::create_dir_all(&folder).unwrap();
        let lib = LibrarySettings {
            folders: vec![LibraryFolder { path: folder.to_string_lossy().into_owned(), skills: vec!["review".into()] }],
            ..Default::default()
        };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        assert!(folder.join("review/SKILL.md").is_file());
        std::fs::write(folder.join("review/extra.md"), "added there").unwrap();

        let p = f.run(&LibrarySettings::default(), &mut ledger, true);
        assert_eq!(p.kept, vec![folder.join("review").to_string_lossy().into_owned()]);
        assert!(folder.join("review/extra.md").is_file());
        assert!(ledger.written.is_empty(), "no longer Luna's");
    }

    #[test]
    fn a_folder_that_is_gone_is_not_made_again() {
        let f = Fixture::new("gone-folder");
        f.skill("review", "v1");
        let folder = f.base.join("deleted-repo/skills");
        let lib = LibrarySettings {
            folders: vec![LibraryFolder { path: folder.to_string_lossy().into_owned(), skills: vec!["review".into()] }],
            ..Default::default()
        };
        let p = f.run(&lib, &mut Ledger::default(), true);
        assert_eq!(p.places[0].status, Status::Failed);
        assert!(!f.base.join("deleted-repo").exists());
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
        std::fs::write(f.dirs.instructions(), "Be brief.\n").unwrap();
        let lib = LibrarySettings { instructions: all(), ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        // The account renamed in case only: the same folder, spelled anew.
        f.accounts[0].path = f.accounts[0].path.to_uppercase();
        let claude_md = f.account(0).join("CLAUDE.md");
        let p = f.run(&lib, &mut ledger, true);
        assert_eq!(Fixture::status(&p, &claude_md), Status::Current);
        assert!(claude_md.is_file(), "the old spelling's note takes nothing out");
        let noted: Vec<String> = ledger.written.keys().filter(|k| k.ends_with("CLAUDE.md")).cloned().collect();
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
        std::fs::write(f.dirs.instructions(), "Be brief.\n").unwrap();
        let claude_md = f.account(0).join("CLAUDE.md").to_string_lossy().into_owned();
        let lib = LibrarySettings { instructions: all(), skipped: vec![claude_md.clone()], ..Default::default() };
        let p = f.run(&lib, &mut Ledger::default(), true);
        assert_eq!(Fixture::status(&p, Path::new(&claude_md)), Status::Skipped);
        assert!(!Path::new(&claude_md).exists());
        assert!(f.account(1).join("AGENTS.md").is_file());
    }

    #[test]
    fn blank_instructions_lay_nothing_out() {
        let f = Fixture::new("blank");
        std::fs::write(f.dirs.instructions(), " \n").unwrap();
        let lib = LibrarySettings { instructions: all(), ..Default::default() };
        let p = f.run(&lib, &mut Ledger::default(), true);
        assert!(p.places.is_empty());
        assert!(!f.account(0).join("CLAUDE.md").exists());
    }

    #[test]
    fn backups_and_blobs_keep_what_a_diff_and_an_undo_need() {
        let f = Fixture::new("blobs");
        std::fs::write(f.dirs.instructions(), "Be brief.\n").unwrap();
        let lib = LibrarySettings { instructions: all(), ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        let dest = f.account(0).join("CLAUDE.md").to_string_lossy().into_owned();
        let base = blob(&f.dirs, &ledger.written[&dest][""]).unwrap();
        assert_eq!(base, b"Be brief.\n");

        let saved = back_up(&f.dirs, Path::new(&dest), &Files::from([(String::new(), b"mine".to_vec())])).unwrap();
        assert_eq!(std::fs::read(saved.join("CLAUDE.md")).unwrap(), b"mine");

        // A blob no entry names goes at the sweep.
        ledger.written.clear();
        sweep_blobs(&f.dirs, &ledger);
        assert!(blob(&f.dirs, &hash(b"Be brief.\n")).is_none());
    }
}
