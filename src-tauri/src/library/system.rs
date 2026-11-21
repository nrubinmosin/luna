//! System texts: text Luna serves at its own trigger, never laid out
//! (docs/adr/0026-session-roles-at-memory-level.md): what a session is told
//! of its role (role.rs), Luna's default for each role (`role-chat`, empty,
//! `role-lead`, `role-helper`), and the note after a task from another agent
//! (`note-lead`, `note-helper`). Luna's own text is compiled in, so it cannot
//! be missing; the person may override each with one file for everyone,
//! `<library>/system/<name>/TEXT.md`, checked as UTF-8 within its cap (a note
//! also as one plain line). An override that will not do is not served:
//! Luna's own is, and the reason goes with it.
//!
//! What stays Luna's alone, as mechanism rather than behavior: tool schemas,
//! refusals, the guide's facts, the tag a note opens on.
//!
//! The pass reads only `skills/` and `instructions/`, so `system/` reaches no
//! account or folder.
//!
//! - `resolve_text`: what is served now, `Resolved`, with where the override
//!   is or would go (`path`) and why one there is not used.
//! - `Resolved::state`: the same as the window is shown it, `SystemSkillState`.
//! - `read_system`, `write_system`, `reset_system`: what the window's System
//!   texts section does with an override (commands.rs), nowhere but
//!   `system/<name>` and Luna's backups, and never through a link anywhere
//!   below the library's folder (`inside`). A replaced override goes to the
//!   backups whole, by a rename.

use super::*;

/// Under the library's root: the system texts' overrides, a folder each.
const SYSTEM_DIR: &str = "system";

/// The system texts of a role and of its note, by name.
pub const ROLE_CHAT: &str = "role-chat";
pub const ROLE_LEAD: &str = "role-lead";
pub const ROLE_HELPER: &str = "role-helper";
pub const NOTE_LEAD: &str = "note-lead";
pub const NOTE_HELPER: &str = "note-helper";

/// The most a role's override may hold: it is read as memory at every start
/// and turn, and goes on a Codex command line (role.rs `CODEX_ROLE_MAX`).
pub const ROLE_MAX_BYTES: usize = 8 * 1024;

/// The most a note's override may hold, in characters: it goes after every
/// task, one line.
pub const NOTE_MAX_CHARS: usize = 1000;

/// What a system text is served as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TextKind {
    /// A role's default, Markdown with no front matter; empty is none.
    Role,
    /// A note: one line of plain text, never empty.
    Note,
}

/// One system text: its name, what it is, and Luna's own text of it.
pub struct SystemText {
    pub name: &'static str,
    pub kind: TextKind,
    pub luna: &'static str,
}

/// The system texts there are.
pub const SYSTEM_TEXTS: &[SystemText] = &[
    SystemText { name: ROLE_CHAT, kind: TextKind::Role, luna: include_str!("../../resources/system/role-chat.md") },
    SystemText { name: ROLE_LEAD, kind: TextKind::Role, luna: include_str!("../../resources/system/role-lead.md") },
    SystemText { name: ROLE_HELPER, kind: TextKind::Role, luna: include_str!("../../resources/system/role-helper.md") },
    SystemText { name: NOTE_LEAD, kind: TextKind::Note, luna: include_str!("../../resources/system/note-lead.md") },
    SystemText { name: NOTE_HELPER, kind: TextKind::Note, luna: include_str!("../../resources/system/note-helper.md") },
];

/// The system text `name`; an error for any other name, so that nothing the
/// window asks for here reaches past `system/<name>`.
pub(super) fn text_of(name: &str) -> Result<&'static SystemText, String> {
    SYSTEM_TEXTS.iter().find(|t| t.name == name).ok_or_else(|| format!("{name} is not a system text"))
}

impl TextKind {
    /// The most its override may hold, in bytes.
    fn max_bytes(self) -> usize {
        match self {
            TextKind::Role => ROLE_MAX_BYTES,
            // Characters are what a note is held to; four bytes at most each.
            TextKind::Note => NOTE_MAX_CHARS * 4,
        }
    }
}

/// An override's one file in its folder.
const TEXT_FILE: &str = "TEXT.md";

/// Whose text is served.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    /// Luna's own, of this version.
    Luna,
    /// The person's override in the library.
    Library,
}

/// What a system text resolves to now.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    pub name: &'static str,
    pub kind: TextKind,
    pub source: Source,
    /// The text served, as it is.
    pub text: String,
    /// SHA-256 of `text`, as hex: what tells one text from another.
    pub sha256: String,
    /// Where the override is, or would be: used only when `source` says so.
    pub path: PathBuf,
    /// Why the override there is not used: there is one, and it will not do.
    pub problem: Option<String>,
}

impl Resolved {
    /// What a session reads: the text with no blank lines around it.
    pub fn body(&self) -> &str {
        self.text.strip_prefix('\u{feff}').unwrap_or(&self.text).trim()
    }

    /// The first 12 characters of the hash, as the window and a log line say it.
    pub fn short_sha(&self) -> &str {
        &self.sha256[..self.sha256.len().min(12)]
    }

    /// As the window shows it.
    pub fn state(&self) -> SystemSkillState {
        SystemSkillState {
            name: self.name,
            kind: self.kind,
            state: match (self.source, &self.problem) {
                (Source::Library, _) => SystemState::Library,
                (Source::Luna, Some(_)) => SystemState::Broken,
                (Source::Luna, None) => SystemState::Luna,
            },
            version: env!("CARGO_PKG_VERSION"),
            sha256: self.sha256.clone(),
            path: self.path.to_string_lossy().into_owned(),
            problem: self.problem.clone(),
        }
    }
}

/// Where a system text stands, for the window.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SystemState {
    /// Luna's own: there is no override.
    Luna,
    /// The override is served.
    Library,
    /// There is an override, it will not do (`problem`), and Luna's own is served.
    Broken,
}

/// A system text as the window is shown it: whose text is served, Luna's
/// version, the hash of what is served, where the override is or would go, and
/// why it is not used when it is not.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SystemSkillState {
    pub name: &'static str,
    pub kind: TextKind,
    pub state: SystemState,
    /// Luna's version: whose own text this is, where it is Luna's.
    pub version: &'static str,
    pub sha256: String,
    pub path: String,
    pub problem: Option<String>,
}

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

impl Dirs {
    fn system(&self, name: &str) -> PathBuf {
        self.root.join(SYSTEM_DIR).join(name)
    }
}

/// Luna's own text of the system text `name`; an error for any other name,
/// so that nothing the window asks for here reaches past `system/<name>`.
pub(super) fn luna_text(name: &str) -> Result<&'static str, String> {
    text_of(name).map(|t| t.luna)
}

/// The most of an override the editor opens: one larger than Luna serves
/// (`TextKind::max_bytes`) still opens, to be cut down to size.
pub(super) const EDIT_MAX_BYTES: u64 = 1024 * 1024;

fn too_large(kind: TextKind) -> String {
    match kind {
        TextKind::Note => format!("{TEXT_FILE} is over {NOTE_MAX_CHARS} characters"),
        TextKind::Role => format!("{TEXT_FILE} is over {} KB", kind.max_bytes() / 1024),
    }
}

/// `rel` under `root`, walked down from it (`boundary::walk_down`): nothing on
/// the way below `root` a link, a junction or a file in a folder's place, and
/// the part of it that is there inside `root` once both are resolved. A check
/// of the last part alone would let an ancestor planted as a link (`skills/` a
/// junction to an account's) take what follows elsewhere. `root` itself is
/// where the person put the library, a link of their choosing if they chose
/// one, and the person's folder, so only looked at: not made private as
/// Luna's own are (`boundary::private_chain`). The last parts need not be there.
pub(super) fn inside(root: &Path, rel: &[&str]) -> Result<PathBuf, String> {
    let mut there = None;
    let mut gone = false;
    let path = crate::boundary::walk_down(root, rel, |p, depth| {
        if depth == 0 || gone {
            return Ok(());
        }
        match std::fs::symlink_metadata(p) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => gone = true,
            Err(e) => return Err(format!("{}: {e}", p.display())),
            Ok(m) if crate::boundary::is_link(&m) => return Err(format!("{} is a link — Luna leaves it alone", p.display())),
            Ok(m) if depth < rel.len() && !m.is_dir() => return Err(format!("{} is not a folder", p.display())),
            Ok(_) => there = Some(p.to_path_buf()),
        }
        Ok(())
    })?;
    if let Some(there) = there {
        let resolved = |p: &Path| std::fs::canonicalize(p).map_err(|e| format!("{}: {e}", p.display()));
        if !resolved(&there)?.starts_with(resolved(root)?) {
            return Err(format!("{} lies outside {}", there.display(), root.display()));
        }
    }
    Ok(path)
}

/// The override's folder, `<library>/system/<name>` (`inside` the library);
/// None where it is not there.
fn override_dir(dirs: &Dirs, name: &str) -> Result<Option<PathBuf>, String> {
    let dir = inside(&dirs.root, &[SYSTEM_DIR, name])?;
    match std::fs::symlink_metadata(&dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", dir.display())),
        Ok(m) if !m.is_dir() => Err(format!("{} is not a folder", dir.display())),
        Ok(_) => Ok(Some(dir)),
    }
}

/// Refuses a folder that holds anything but plain files and folders, before
/// it is moved or copied: a link, which none of that goes through, or a FIFO
/// or a device, which a copy would wait on with the library's lock held.
pub(super) fn plain_tree(dir: &Path) -> Result<(), String> {
    for entry in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let path = entry.map_err(|e| format!("{}: {e}", dir.display()))?.path();
        let meta = std::fs::symlink_metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        if crate::boundary::is_link(&meta) {
            return Err(format!("{} is a link — Luna leaves it alone", path.display()));
        } else if meta.is_dir() {
            plain_tree(&path)?;
        } else if !meta.is_file() {
            return Err(format!("{} is not a plain file — Luna leaves it alone", path.display()));
        }
    }
    Ok(())
}

// The steps of a move a test makes fail as the disk would, by name.
#[cfg(test)]
thread_local! {
    static FAULTS: std::cell::RefCell<Vec<(&'static str, std::io::ErrorKind)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// A rename that is one named step of a move, for a test to make fail.
fn rename(step: &'static str, from: &Path, to: &Path) -> std::io::Result<()> {
    #[cfg(test)]
    if let Some(kind) = FAULTS.with(|f| f.borrow().iter().find(|(s, _)| *s == step).map(|(_, k)| *k)) {
        return Err(std::io::Error::new(kind, format!("{step} failed for the test")));
    }
    let _ = step;
    std::fs::rename(from, to)
}

// What a test has happen to a folder between a copy of it and the check of
// that copy, as an editor saving into it would.
#[cfg(test)]
thread_local! {
    static MEANWHILE: std::cell::RefCell<Option<Box<dyn FnOnce(&Path)>>> = const { std::cell::RefCell::new(None) };
}

fn meanwhile(dir: &Path) {
    #[cfg(test)]
    if let Some(happen) = MEANWHILE.with(|m| m.borrow_mut().take()) {
        happen(dir);
    }
    let _ = dir;
}

/// Every file and folder under `dir` by its path there, a file with the
/// SHA-256 of its bytes: what a copy must hold to be the folder's whole.
fn tree(dir: &Path) -> std::io::Result<BTreeMap<String, Option<String>>> {
    fn walk(dir: &Path, prefix: &str, out: &mut BTreeMap<String, Option<String>>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let rel = format!("{prefix}{}", entry.file_name().to_string_lossy());
            if entry.file_type()?.is_dir() {
                out.insert(format!("{rel}/"), None);
                walk(&entry.path(), &format!("{rel}/"), out)?;
            } else {
                out.insert(rel, Some(sha256_hex(&std::fs::read(entry.path())?)));
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    walk(dir, "", &mut out)?;
    Ok(out)
}

/// Where a folder sent to the backups is now.
#[derive(Debug, PartialEq)]
pub(super) enum Moved {
    /// In the backups, whole: the folder of the backup.
    Backup(PathBuf),
    /// Set aside where it lay, under a name nothing reads: the copy into the
    /// backups, on another volume, did not come out whole (`set_aside` names
    /// it to the window).
    Aside(PathBuf),
}

/// The system skill 0.19 served for its handoff, which ADR-0030 removed. What
/// it left in `system/`: its override, `session-handoff/`, and what a reset or
/// an adoption cut short set aside as `.session-handoff.<…>`, an adoption's
/// holding the person's own library skill. Nothing reads them any more.
const RETIRED_SKILL: &str = "session-handoff";

/// Whether a name in `system/` is one 0.19's handoff left (`RETIRED_SKILL`),
/// in any case: Windows and macOS find a folder whatever case its name is
/// written in, so 0.19 served an override named `Session-Handoff` there.
fn retired(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name == RETIRED_SKILL || name.starts_with(&format!(".{RETIRED_SKILL}."))
}

/// Folders set aside in `system/`, read by nothing: a former override a copy
/// into the backups did not take whole, a move cut short, and what 0.19's
/// handoff left (`retired`). The window names them, for the person to move or
/// delete.
pub(super) fn set_aside(dirs: &Dirs) -> Vec<String> {
    let Ok(system) = inside(&dirs.root, &[SYSTEM_DIR]) else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(&system) else { return Vec::new() };
    let mut out: Vec<String> = entries
        .flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            retired(&name) || SYSTEM_TEXTS.iter().any(|s| name.starts_with(&format!(".{}.", s.name)))
        })
        .map(|e| e.path().to_string_lossy().into_owned())
        .collect();
    out.sort();
    out
}

/// What 0.19's handoff left in the library's `system/` (`retired`), said once
/// in the log as a core starts. Left where it is: one may hold the person's
/// own skill, theirs to move back or delete.
pub(crate) fn note_retired_system() {
    let dirs = Dirs::current();
    for path in set_aside(&dirs) {
        let name = Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if !retired(&name) {
            continue;
        }
        let what = if name.eq_ignore_ascii_case(RETIRED_SKILL) {
            "the override of 0.19's handoff skill, which nothing reads since 0.20"
        } else {
            "set aside by 0.19's handoff skill (an interrupted Make it the override… left your own skill here), \
             which nothing reads since 0.20"
        };
        crate::log::info("library", &format!("{path}: {what}; left as it is, for you to move or delete"));
    }
}

/// Every file and folder under `from` copied to `to`, a new folder, as they
/// are: for a move between volumes, which no rename makes.
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        } else {
            std::fs::copy(entry.path(), to.join(entry.file_name()))?;
        }
    }
    Ok(())
}

/// Moves the folder `dir` whole into Luna's backups, under a folder of its own
/// named as `back_up` names one for `was`, where it lay, with `from.txt`
/// beside it: a rename, so nothing in it is left out, its temp files and a
/// save made meanwhile included.
///
/// Where the backups are on another volume than the library, no rename
/// reaches them. The folder is then renamed aside first, within its own
/// volume and under a name nothing reads, so that a save by its name lands
/// where it lay rather than in what is being copied; copied from there; and
/// taken out only once the copy holds the same files with the same bytes. A
/// copy that does not come out whole is taken out instead, and the folder
/// stays aside (`Moved::Aside`): out of its place either way, so Luna's own
/// text is served.
fn move_to_backups(dirs: &Dirs, dir: &Path, was: &Path) -> Result<Moved, String> {
    // Luna's own, in its data folder: no link, this user's, private, made so
    // before anything is put in it.
    crate::boundary::private_chain(&dirs.backups(), &[])?;
    let slot = backup_slot(dirs, was)?;
    let undo = || {
        let _ = std::fs::remove_file(slot.join("from.txt"));
        let _ = std::fs::remove_dir(&slot);
    };
    if let Err(e) = write_file(&slot.join("from.txt"), was.to_string_lossy().as_bytes()) {
        undo();
        return Err(e);
    }
    let name = was.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let to = slot.join(&name);
    match rename("backup", dir, &to) {
        Ok(()) => return Ok(Moved::Backup(slot)),
        Err(e) if e.kind() == std::io::ErrorKind::CrossesDevices => {}
        Err(e) => {
            undo();
            return Err(format!("{} to {}: {e}", dir.display(), to.display()));
        }
    }
    let aside = dir.with_file_name(format!(".{name}.leaving-{}", crate::secret::random_hex(4)));
    if let Err(e) = rename("leave", dir, &aside) {
        undo();
        return Err(format!("{} to {}: {e}", dir.display(), aside.display()));
    }
    let whole = copy_tree(&aside, &to).and_then(|()| {
        meanwhile(&aside);
        Ok(tree(&aside)? == tree(&to)?)
    });
    match whole {
        Ok(true) => {
            if let Err(e) = std::fs::remove_dir_all(&aside) {
                // The backup is whole; what is left aside the window names.
                crate::log::warn(
                    "library",
                    &format!("{} is in {}, but {} could not be taken out: {e}", dir.display(), slot.display(), aside.display()),
                );
            }
            Ok(Moved::Backup(slot))
        }
        not_whole => {
            let why = match not_whole {
                Err(e) => e.to_string(),
                _ => "it changed while it was copied".into(),
            };
            crate::log::warn(
                "library",
                &format!("{} could not be copied whole into {} ({why}): kept at {}", dir.display(), slot.display(), aside.display()),
            );
            let _ = std::fs::remove_dir_all(&to);
            undo();
            Ok(Moved::Aside(aside))
        }
    }
}

/// Why this text would not do as the override of `name`, if it would not. A
/// role's text is held to ROLE_MAX_BYTES; empty, it says nothing. A note is one line of
/// plain text, not empty, at most NOTE_MAX_CHARS long, with no brackets: it
/// goes inside the brackets of the note Luna puts after a task, which the
/// readers take off by its closing one (pty/start.rs `without_note`).
pub(super) fn check_override(name: &str, text: &str) -> Result<(), String> {
    let kind = text_of(name)?.kind;
    if text.len() > kind.max_bytes() {
        return Err(too_large(kind));
    }
    match kind {
        TextKind::Role => {}
        TextKind::Note => {
            let line = text.strip_prefix('\u{feff}').unwrap_or(text).trim();
            if line.is_empty() {
                return Err(format!("{TEXT_FILE} is empty: a note says something"));
            }
            if line.chars().count() > NOTE_MAX_CHARS {
                return Err(too_large(kind));
            }
            if line.contains(['\n', '\r']) {
                return Err(format!("{TEXT_FILE} holds more than one line: a note is one"));
            }
            if line.contains(['[', ']']) {
                return Err(format!("{TEXT_FILE} holds a bracket: the note goes inside brackets of its own"));
            }
            if let Some(c) = line.chars().find(|c| c.is_control()) {
                return Err(format!("{TEXT_FILE} holds control character U+{:04X}", c as u32));
            }
        }
    }
    Ok(())
}

/// The override of `name` for the editor; where there is none yet, Luna's own
/// text to start one from, at the path saving it makes (`exists` false).
pub(super) fn read_system(dirs: &Dirs, name: &str) -> Result<LibraryFile, String> {
    let t = text_of(name)?;
    let luna = t.luna;
    let path = dirs.system(name).join(TEXT_FILE);
    let shown = path.to_string_lossy().into_owned();
    let root = dirs.root.to_string_lossy().into_owned();
    let start = || LibraryFile { path: shown.clone(), text: luna.to_string(), exists: false, root: root.clone(), ..Default::default() };
    let Some(dir) = override_dir(dirs, name)? else { return Ok(start()) };
    match read_own_file(&dir, TEXT_FILE, EDIT_MAX_BYTES + 1)? {
        // A folder without TEXT.md: what saving puts there starts from Luna's.
        None => Ok(start()),
        Some(bytes) if bytes.len() as u64 > EDIT_MAX_BYTES => {
            Err(format!("{} is over {} KB, too large to open here", path.display(), EDIT_MAX_BYTES / 1024))
        }
        Some(bytes) => {
            let revision = Some(sha256_hex(&bytes));
            let text = String::from_utf8(bytes).map_err(|_| format!("{} is not UTF-8 text", path.display()))?;
            Ok(LibraryFile { path: shown, text, exists: true, root, revision, ..Default::default() })
        }
    }
}

/// Where the override of `name` lies under the library's root, its folders
/// and then its file, for a save that walks there by handle
/// (`library_write_checked`); `text` more than Luna would ever serve of it
/// is refused, as `write_system` refuses it.
pub(super) fn system_place(name: &str, text: &str) -> Result<(Vec<String>, &'static str), String> {
    let t = text_of(name)?;
    if text.len() > t.kind.max_bytes() {
        return Err(format!("{}, more than Luna serves: not saved", too_large(t.kind)));
    }
    Ok((vec![SYSTEM_DIR.to_string(), t.name.to_string()], TEXT_FILE))
}

/// Saves the editor's text as the override of `name`. One that will not do
/// (`check_override`) is still saved, as a library skill's draft is: Luna
/// serves its own meanwhile and says why. More than Luna would ever serve is
/// refused, and so is a link or a file in the way.
pub(super) fn write_system(dirs: &Dirs, name: &str, text: &str) -> Result<PathBuf, String> {
    let kind = text_of(name)?.kind;
    if text.len() > kind.max_bytes() {
        return Err(format!("{}, more than Luna serves: not saved", too_large(kind)));
    }
    let path = inside(&dirs.root, &[SYSTEM_DIR, name, TEXT_FILE])?;
    if let Ok(m) = std::fs::symlink_metadata(&path) {
        if !m.is_file() {
            return Err(format!("{} is a folder — Luna leaves it alone", path.display()));
        }
    }
    // Through a temp file and a rename (`Dirs::write`): a session's start
    // reads the old text or the new, never part of either.
    dirs.write(&path, text.as_bytes())?;
    Ok(path)
}

/// Back to Luna's own for `name`: the override's folder, as it is, moves into
/// Luna's backups (kept BACKUP_DAYS, as a replaced library file is), or, where
/// a copy to them on another volume does not come out whole, stays aside in
/// `system/` (`move_to_backups`); `system/` goes too once empty. Where it is
/// now, or None where there was none. A session's start meanwhile reads the
/// override or none, either a text it may be served.
pub(super) fn reset_system(dirs: &Dirs, name: &str) -> Result<Option<Moved>, String> {
    luna_text(name)?;
    let Some(dir) = override_dir(dirs, name)? else { return Ok(None) };
    plain_tree(&dir)?;
    let moved = move_to_backups(dirs, &dir, &dir)?;
    let _ = std::fs::remove_dir(dirs.root.join(SYSTEM_DIR));
    Ok(Some(moved))
}

/// Luna's own text of a system text beside the library's override of it.
#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SystemCompare {
    pub luna: String,
    /// The override's TEXT.md; None where there is none.
    pub yours: Option<String>,
}

pub(super) fn compare_system(dirs: &Dirs, name: &str) -> Result<SystemCompare, String> {
    let luna = luna_text(name)?.to_string();
    let file = read_system(dirs, name)?;
    Ok(SystemCompare { luna, yours: file.exists.then_some(file.text) })
}

/// What the system text `name` is served as now: the override when there is
/// one that will do, else Luna's own. None for a name that is no system text.
pub(super) fn resolve_text(dirs: &Dirs, name: &str) -> Option<Resolved> {
    let t = text_of(name).ok()?;
    let path = dirs.system(t.name).join(TEXT_FILE);
    let luna = |problem: Option<String>| Resolved {
        name: t.name,
        kind: t.kind,
        source: Source::Luna,
        text: t.luna.to_string(),
        sha256: sha256_hex(t.luna.as_bytes()),
        path: path.clone(),
        problem,
    };
    Some(match read_override(dirs, t) {
        Ok(None) => luna(None),
        Ok(Some(text)) => Resolved {
            name: t.name,
            kind: t.kind,
            source: Source::Library,
            sha256: sha256_hex(text.as_bytes()),
            text,
            path: path.clone(),
            problem: None,
        },
        Err(why) => luna(Some(why)),
    })
}

/// The override's file if it will do; None where there is none; why not
/// where it will not: checked by `check_override`, as UTF-8 within its kind's
/// cap. Its one file alone is read, as it alone is served: opened as itself
/// (no link at it, `system/` or its folder; a FIFO there no reason to wait,
/// `boundary::open_no_follow`) and read no further than the cap, so what lies
/// beside it cannot hold up or swell the read, which every session's start
/// makes for its role.
fn read_override(dirs: &Dirs, t: &SystemText) -> Result<Option<String>, String> {
    let Some(dir) = override_dir(dirs, t.name)? else { return Ok(None) };
    let (file, max) = (TEXT_FILE, t.kind.max_bytes());
    let bytes = read_own_file(&dir, file, max as u64 + 1)?.ok_or_else(|| format!("There is no {file}"))?;
    if bytes.len() > max {
        return Err(too_large(t.kind));
    }
    let text = String::from_utf8(bytes).map_err(|_| format!("{file} is not UTF-8 text"))?;
    check_override(t.name, &text)?;
    Ok(Some(text))
}

/// The override's `file` in `dir` (`open_plain`), held to a plain file of
/// one name of this user's making (`secret::own_file`; not `read_private`,
/// as a library of the person's choosing keeps its own folder's access) and
/// read no further than `cap` bytes; None where there is none.
fn read_own_file(dir: &Path, file: &str, cap: u64) -> Result<Option<Vec<u8>>, String> {
    let path = dir.join(file);
    let Some(file) = open_plain(&path)? else { return Ok(None) };
    // The person's own file, in a folder of theirs that need not be private
    // (a library of their choosing keeps its folder's access): of their
    // making, of one name, not another user's or another file's second name.
    match crate::secret::own_file(&file) {
        Ok(Ok(())) => {}
        Ok(Err(why)) => return Err(format!("{} is not a file of yours ({why}); the override is your own", path.display())),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    }
    read_capped(&path, file, cap).map(Some)
}

/// A file of the library's at `path`, opened as itself
/// (`boundary::open_no_follow`: no link at it, and no FIFO there to wait on)
/// and held to a plain file by the handle's own word; None where there is
/// none. The folders above it are the caller's to check (`inside`).
fn open_plain(path: &Path) -> Result<Option<std::fs::File>, String> {
    let not_a_file = || format!("{} is a link or not a file — Luna leaves it alone", path.display());
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
        Ok(m) if crate::boundary::is_link(&m) || !m.is_file() => return Err(not_a_file()),
        Ok(_) => {}
    }
    let file = crate::boundary::open_no_follow(path).map_err(|e| format!("{}: {e}", path.display()))?;
    // The handle's own word: what was opened, not what was at the name a
    // moment before.
    let opened = file.metadata().map_err(|e| format!("{}: {e}", path.display()))?;
    if crate::boundary::is_link(&opened) || !opened.is_file() {
        return Err(not_a_file());
    }
    Ok(Some(file))
}

fn read_capped(path: &Path, file: std::fs::File, cap: u64) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut bytes = Vec::new();
    file.take(cap).read_to_end(&mut bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(bytes)
}

/// A file of the library's at `path` (`open_plain`), read no further than
/// `cap` bytes; None where there is none.
pub(super) fn read_plain(path: &Path, cap: u64) -> Result<Option<Vec<u8>>, String> {
    match open_plain(path)? {
        Some(file) => read_capped(path, file, cap).map(Some),
        None => Ok(None),
    }
}

/// Refuses anything at `path` but a plain file, before a write replaces it:
/// a link (which the editor did not open), a folder, a FIFO. Nothing there
/// passes.
pub(super) fn plain_or_none(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("{}: {e}", path.display())),
        Ok(m) if crate::boundary::is_link(&m) || !m.is_file() => {
            Err(format!("{} is a link or not a file — Luna leaves it alone", path.display()))
        }
        Ok(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OURS: &str = "Lead our way.\n";

    fn luna() -> &'static str {
        luna_text(ROLE_LEAD).unwrap()
    }

    fn resolved(dirs: &Dirs) -> Resolved {
        resolve_text(dirs, ROLE_LEAD).unwrap()
    }

    fn write_override(f: &Fixture, text: &str) -> PathBuf {
        let path = f.dirs.system(ROLE_LEAD).join(TEXT_FILE);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }

    /// The override is served, never laid out: a pass that lays the
    /// library's skills and instructions into every account leaves no trace of
    /// it in any of them.
    #[test]
    fn the_override_reaches_no_account() {
        let f = Fixture::new("system-nowhere");
        write_override(&f, OURS);
        f.skill("review", "Reviews.");
        f.instructions("main", "Be brief.\n");
        let lib = LibrarySettings { skills: skills(&["review"]), instructions: first(all()), ..Default::default() };
        let p = f.run(&lib, &mut Ledger::default(), true);
        assert!(!p.places.is_empty() && f.account(0).join("skills").join("review").is_dir(), "the rest is laid out");
        fn walk(dir: &Path, found: &mut Vec<PathBuf>) {
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.contains(ROLE_LEAD) || name == SYSTEM_DIR {
                    found.push(path.clone());
                }
                if path.is_dir() {
                    walk(&path, found);
                }
            }
        }
        let mut found = Vec::new();
        walk(&f.base.join("accounts"), &mut found);
        walk(&f.dirs.folders(), &mut found);
        assert!(found.is_empty(), "{found:?}");
        assert!(p.places.iter().all(|place| !place.dest.contains(ROLE_LEAD)));
    }

    #[test]
    fn without_an_override_luna_s_own_is_served() {
        let f = Fixture::new("system-none");
        let r = resolved(&f.dirs);
        assert_eq!((r.source, r.problem.as_deref(), r.text.as_str()), (Source::Luna, None, luna()));
        assert_eq!(r.sha256, sha256_hex(luna().as_bytes()));
        assert_eq!(r.path, f.dirs.root.join("system").join(ROLE_LEAD).join(TEXT_FILE));
        let s = r.state();
        assert_eq!((s.state, s.version, s.name), (SystemState::Luna, env!("CARGO_PKG_VERSION"), ROLE_LEAD));
        // A library skill of the same name is not the override.
        f.skill(ROLE_LEAD, "Mine, laid out as a skill.");
        assert_eq!(resolved(&f.dirs).source, Source::Luna, "skills/role-lead is never read for it");
    }

    #[test]
    fn an_override_that_will_do_is_served_over_luna_s() {
        let f = Fixture::new("system-ok");
        let text = "\n# Our lead\nLead our way.\n\n";
        let path = write_override(&f, text);
        let r = resolved(&f.dirs);
        assert_eq!((r.source, r.problem.as_deref(), r.text.as_str(), &r.path), (Source::Library, None, text, &path));
        assert_eq!(r.sha256, sha256_hex(text.as_bytes()));
        assert_eq!(r.body(), "# Our lead\nLead our way.");
        assert_eq!(r.state().state, SystemState::Library);
        assert_eq!(r.short_sha().len(), 12);
    }

    #[test]
    fn a_broken_override_falls_back_to_luna_s_and_says_why() {
        let f = Fixture::new("system-broken");
        let path = write_override(&f, "x");
        std::fs::write(&path, [0x2d, 0xff]).unwrap();
        let r = resolved(&f.dirs);
        assert_eq!((r.source, r.text.as_str()), (Source::Luna, luna()));
        assert!(r.problem.as_deref().is_some_and(|p| p.contains("UTF-8")), "{:?}", r.problem);
        assert_eq!(r.state().state, SystemState::Broken);
        std::fs::write(&path, "a".repeat(ROLE_MAX_BYTES + 1)).unwrap();
        assert!(resolved(&f.dirs).problem.unwrap().contains("KB"));
        // A folder without TEXT.md is an override that will not do, not none.
        std::fs::remove_file(&path).unwrap();
        assert!(resolved(&f.dirs).problem.unwrap().contains("no TEXT.md"));
        // A note's own checks: one plain line.
        let note = f.dirs.system(NOTE_LEAD).join(TEXT_FILE);
        std::fs::create_dir_all(note.parent().unwrap()).unwrap();
        std::fs::write(&note, "two\nlines").unwrap();
        let r = resolve_text(&f.dirs, NOTE_LEAD).unwrap();
        assert!(r.problem.as_deref().is_some_and(|p| p.contains("more than one line")), "{:?}", r.problem);
    }

    /// A file link, where this machine lets a test make one (Windows asks for
    /// Developer Mode or an administrator).
    fn make_link(target: &Path, link: &Path) -> bool {
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(target, link);
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(target, link);
        made.is_ok()
    }

    #[test]
    fn only_text_md_is_read_and_no_further_than_the_cap() {
        let f = Fixture::new("system-bounded");
        let path = write_override(&f, OURS);
        let dir = path.parent().unwrap().to_path_buf();
        // What lies beside it is never read: a file many times the cap, a link.
        std::fs::write(dir.join("notes.md"), vec![b'a'; ROLE_MAX_BYTES * 8]).unwrap();
        let _ = make_link(&dir.join("notes.md"), &dir.join("linked.md"));
        assert_eq!(resolved(&f.dirs).source, Source::Library);
        // TEXT.md a link to a file that would do: not the file itself.
        let real = f.base.join("elsewhere.md");
        std::fs::write(&real, OURS).unwrap();
        std::fs::remove_file(&path).unwrap();
        if make_link(&real, &path) {
            assert!(resolved(&f.dirs).problem.unwrap().contains("link or not a file"));
            std::fs::remove_file(&path).unwrap();
        }
        std::fs::create_dir(&path).unwrap();
        assert!(resolved(&f.dirs).problem.unwrap().contains("not a file"), "a folder at its name");
    }

    /// A FIFO beside TEXT.md, or at its name, holds no read up: every
    /// session's start resolves its role's text.
    #[cfg(unix)]
    #[test]
    fn a_fifo_in_the_override_s_folder_holds_nothing_up() {
        let f = Fixture::new("system-fifo");
        let path = write_override(&f, OURS);
        let fifo = |p: &Path| {
            let c = std::ffi::CString::new(p.to_string_lossy().as_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        };
        let resolved_now = |f: &Fixture| {
            let dirs = Dirs { root: f.dirs.root.clone(), data: f.dirs.data.clone() };
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || tx.send(resolved(&dirs)));
            rx.recv_timeout(std::time::Duration::from_secs(5)).expect("resolved without waiting on a FIFO")
        };
        fifo(&path.with_file_name("pipe.md"));
        assert_eq!(resolved_now(&f).source, Source::Library, "beside it");
        std::fs::remove_file(&path).unwrap();
        fifo(&path);
        assert!(resolved_now(&f).problem.unwrap().contains("not a file"), "at its name");
    }

    /// A symlink on Unix, a junction on Windows (no privilege needed).
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

    /// The link itself, never what it leads to.
    fn unlink_dir(link: &Path) {
        let _ = std::fs::remove_dir(link).or_else(|_| std::fs::remove_file(link));
    }

    fn backups(f: &Fixture) -> Vec<PathBuf> {
        std::fs::read_dir(f.dirs.backups()).map(|e| e.flatten().map(|e| e.path()).collect()).unwrap_or_default()
    }

    #[test]
    fn the_editor_starts_an_override_from_luna_s_text_and_what_it_saves_is_served() {
        let f = Fixture::new("system-edit");
        let start = read_system(&f.dirs, ROLE_LEAD).unwrap();
        let path = f.dirs.root.join("system").join(ROLE_LEAD).join(TEXT_FILE);
        assert_eq!((start.text.as_str(), start.exists, &start.path), (luna(), false, &path.to_string_lossy().into_owned()));
        assert!(!f.dirs.root.join("system").exists(), "reading makes nothing");
        assert_eq!(compare_system(&f.dirs, ROLE_LEAD).unwrap(), SystemCompare { luna: luna().into(), yours: None });

        assert_eq!(write_system(&f.dirs, ROLE_LEAD, OURS).unwrap(), path);
        let r = resolved(&f.dirs);
        assert_eq!((r.source, r.text.as_str()), (Source::Library, OURS));
        let read = read_system(&f.dirs, ROLE_LEAD).unwrap();
        assert_eq!((read.text.as_str(), read.exists), (OURS, true));
        assert_eq!(compare_system(&f.dirs, ROLE_LEAD).unwrap().yours.as_deref(), Some(OURS));

        // One that will not do is saved all the same, and Luna's own served.
        write_system(&f.dirs, NOTE_LEAD, "a [bracket]").unwrap();
        let r = resolve_text(&f.dirs, NOTE_LEAD).unwrap();
        assert_eq!((r.source, r.state().state), (Source::Luna, SystemState::Broken));
        assert!(r.problem.unwrap().contains("bracket"));
    }

    /// Only a system text's own folder is reached, whatever name the window
    /// sends: the commands refuse any other before they touch anything.
    #[test]
    fn no_name_but_a_system_text_s_reaches_anywhere() {
        let f = Fixture::new("system-names");
        f.skill("review", "v1");
        for name in ["review", "../skills/review", "..", "", "system", "Role-Lead", "session-handoff"] {
            assert!(read_system(&f.dirs, name).is_err(), "{name}");
            assert!(write_system(&f.dirs, name, OURS).is_err(), "{name}");
            assert!(reset_system(&f.dirs, name).is_err(), "{name}");
            assert!(compare_system(&f.dirs, name).is_err(), "{name}");
            assert!(library_check_system(name.into(), OURS.into()).is_some(), "{name}");
            assert!(library_system_compare(name.into()).is_err(), "{name}");
            assert!(library_system_reset(name.into()).is_err(), "{name}");
            assert!(library_read(Entry::System(name.into())).is_err(), "{name}");
            assert!(library_write(Entry::System(name.into()), OURS.into()).is_err(), "{name}");
        }
        assert!(f.dirs.skills().join("review").join(SKILL_FILE).is_file(), "the skill there is untouched");
        assert!(!f.dirs.root.join("system").exists());
        assert_eq!(library_check_system(ROLE_LEAD.into(), OURS.into()), None);
        // It has no audience.
        assert!(library_set_audience(Entry::System(ROLE_LEAD.into()), Audience::CHATS).err().is_some_and(|e| e.contains("no audience")));
    }

    #[test]
    fn a_link_on_the_way_is_neither_read_nor_written_through_nor_taken_out() {
        let f = Fixture::new("system-link");
        let elsewhere = f.base.join("elsewhere");
        std::fs::create_dir_all(elsewhere.join(ROLE_LEAD)).unwrap();
        std::fs::write(elsewhere.join(ROLE_LEAD).join(TEXT_FILE), OURS).unwrap();
        let system = f.dirs.root.join("system");
        link_dir(&system, &elsewhere);
        assert!(read_system(&f.dirs, ROLE_LEAD).err().is_some_and(|e| e.contains("link")));
        assert!(write_system(&f.dirs, ROLE_LEAD, "theirs").unwrap_err().contains("link"));
        assert!(reset_system(&f.dirs, ROLE_LEAD).unwrap_err().contains("link"));
        assert!(compare_system(&f.dirs, ROLE_LEAD).is_err());
        assert_eq!(std::fs::read_to_string(elsewhere.join(ROLE_LEAD).join(TEXT_FILE)).unwrap(), OURS, "nothing written there");
        assert!(backups(&f).is_empty());
        unlink_dir(&system);

        // A link inside the override's folder: its folder is not taken out
        // through it, nor moved into the backups.
        let dir = f.dirs.root.join("system").join(ROLE_LEAD);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(TEXT_FILE), OURS).unwrap();
        link_dir(&dir.join("more"), &elsewhere);
        assert!(reset_system(&f.dirs, ROLE_LEAD).unwrap_err().contains("link"));
        assert!(dir.join(TEXT_FILE).is_file() && elsewhere.join(ROLE_LEAD).join(TEXT_FILE).is_file());
        unlink_dir(&dir.join("more"));
    }

    /// A FIFO in the override's folder: refused at once, never read whole and
    /// waited on with the library's lock held; and one at TEXT.md's name is
    /// no file for the editor either.
    #[cfg(unix)]
    #[test]
    fn a_fifo_in_the_way_is_refused_without_waiting() {
        let f = Fixture::new("system-fifo-window");
        let fifo = |p: &Path| {
            let c = std::ffi::CString::new(p.to_string_lossy().as_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        };
        let within = |run: Box<dyn FnOnce() -> Result<(), String> + Send>| {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || tx.send(run()));
            rx.recv_timeout(std::time::Duration::from_secs(5)).expect("answered without waiting on a FIFO")
        };
        let dirs = || Dirs { root: f.dirs.root.clone(), data: f.dirs.data.clone() };
        let path = write_override(&f, OURS);
        fifo(&path.with_file_name("pipe.md"));
        let d = dirs();
        assert!(within(Box::new(move || reset_system(&d, ROLE_LEAD).map(|_| ()))).unwrap_err().contains("not a plain file"));
        assert!(path.is_file(), "nothing taken out");
        std::fs::remove_file(path.with_file_name("pipe.md")).unwrap();
        std::fs::remove_file(&path).unwrap();
        fifo(&path);
        let d = dirs();
        assert!(within(Box::new(move || read_system(&d, ROLE_LEAD).map(|_| ()))).unwrap_err().contains("not a file"));
    }

    #[test]
    fn an_oversized_override_is_not_saved_and_one_too_large_to_edit_not_opened() {
        let f = Fixture::new("system-size");
        let big = "a".repeat(ROLE_MAX_BYTES + 1);
        assert!(write_system(&f.dirs, ROLE_LEAD, &big).unwrap_err().contains("not saved"));
        assert!(!f.dirs.root.join("system").exists());
        assert!(check_override(ROLE_LEAD, &big).unwrap_err().contains("KB"));
        // Over what Luna serves but not what the editor opens: opened, to be
        // cut down; Luna's own is served meanwhile.
        let path = write_override(&f, &big);
        assert!(read_system(&f.dirs, ROLE_LEAD).unwrap().exists);
        assert_eq!(resolved(&f.dirs).state().state, SystemState::Broken);
        std::fs::write(&path, vec![b'a'; EDIT_MAX_BYTES as usize + 1]).unwrap();
        assert!(read_system(&f.dirs, ROLE_LEAD).err().is_some_and(|e| e.contains("too large")));
    }

    #[test]
    fn back_to_luna_s_keeps_the_override_in_the_backups() {
        let f = Fixture::new("system-reset");
        assert_eq!(reset_system(&f.dirs, ROLE_LEAD), Ok(None), "none to take out");
        let path = write_override(&f, OURS);
        std::fs::write(path.with_file_name("example.md"), "beside it").unwrap();
        let Some(Moved::Backup(saved)) = reset_system(&f.dirs, ROLE_LEAD).unwrap() else { panic!("backed up") };
        assert_eq!(std::fs::read_to_string(saved.join(ROLE_LEAD).join(TEXT_FILE)).unwrap(), OURS);
        assert_eq!(std::fs::read_to_string(saved.join(ROLE_LEAD).join("example.md")).unwrap(), "beside it");
        assert!(saved.starts_with(f.dirs.backups()));
        assert!(!f.dirs.root.join("system").exists(), "nothing left of it in the library");
        assert_eq!(resolved(&f.dirs).state().state, SystemState::Luna);
    }

    /// Makes a step of a move fail as the disk would, for this test's thread.
    fn fail(step: &'static str, kind: std::io::ErrorKind) {
        FAULTS.with(|f| f.borrow_mut().push((step, kind)));
    }

    fn heal() {
        FAULTS.with(|f| f.borrow_mut().clear());
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> =
            std::fs::read_dir(dir).map(|e| e.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
        names.sort();
        names
    }

    /// Back to Luna's moves the override's folder whole: its temp files and
    /// whatever else is in it, by a rename, or across volumes by a copy of all
    /// of it before the folder goes.
    #[test]
    fn back_to_luna_s_keeps_all_of_the_folder() {
        for across in [false, true] {
            let f = Fixture::new(if across { "system-reset-across" } else { "system-reset-whole" });
            let path = write_override(&f, OURS);
            let dir = path.parent().unwrap().to_path_buf();
            std::fs::write(dir.join("TEXT.md.1-0.luna-tmp"), "a save under way").unwrap();
            std::fs::create_dir_all(dir.join("more")).unwrap();
            std::fs::write(dir.join("more").join("notes.md"), "notes").unwrap();
            if across {
                fail("backup", std::io::ErrorKind::CrossesDevices);
            }
            let Some(Moved::Backup(saved)) = reset_system(&f.dirs, ROLE_LEAD).unwrap() else { panic!("across: {across}: backed up") };
            let saved = saved.join(ROLE_LEAD);
            heal();
            assert_eq!(std::fs::read_to_string(saved.join(TEXT_FILE)).unwrap(), OURS, "across: {across}");
            assert_eq!(std::fs::read_to_string(saved.join("TEXT.md.1-0.luna-tmp")).unwrap(), "a save under way");
            assert_eq!(std::fs::read_to_string(saved.join("more").join("notes.md")).unwrap(), "notes");
            assert!(!dir.exists() && !f.dirs.root.join("system").exists(), "across: {across}");
            assert!(set_aside(&f.dirs).is_empty(), "across: {across}: nothing left aside");
        }
    }

    /// What 0.19's handoff left in `system/`, its skill's override and an
    /// adoption cut short with the person's own skill in it, is named with
    /// what is set aside, and left as it is; a folder of no system text's is not.
    #[test]
    fn what_0_19_s_handoff_left_in_system_is_named_and_kept() {
        let f = Fixture::new("system-retired");
        let system = f.dirs.root.join("system");
        for dir in ["Session-Handoff", ".session-handoff.adopting-ab12", "notes"] {
            std::fs::create_dir_all(system.join(dir)).unwrap();
            std::fs::write(system.join(dir).join("SKILL.md"), "mine").unwrap();
        }
        let named: Vec<String> =
            set_aside(&f.dirs).iter().map(|p| Path::new(p).file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(named, [".session-handoff.adopting-ab12", "Session-Handoff"], "whatever its case");
        assert!(retired("session-handoff") && retired(".Session-Handoff.replaced-1") && !retired("session-handoffs"));
        assert_eq!(std::fs::read_to_string(system.join(".session-handoff.adopting-ab12").join("SKILL.md")).unwrap(), "mine");
    }

    /// Across volumes, a save that lands in the folder while it is copied (a
    /// file changed after its copy was made, a file added) makes the copy no
    /// longer the folder's whole: the copy goes, the folder stays aside under a
    /// name nothing reads, with the save in it, and the window names it. Luna's
    /// own is served, the override being out of its place either way.
    #[test]
    fn a_reset_across_volumes_loses_no_save_made_while_it_copies() {
        let f = Fixture::new("system-reset-meanwhile");
        let path = write_override(&f, OURS);
        std::fs::write(path.with_file_name("notes.md"), "notes").unwrap();
        fail("backup", std::io::ErrorKind::CrossesDevices);
        MEANWHILE.with(|m| {
            *m.borrow_mut() = Some(Box::new(|aside: &Path| {
                std::fs::write(aside.join(TEXT_FILE), "saved while it was copied").unwrap();
                std::fs::write(aside.join("added.md"), "added while it was copied").unwrap();
            }))
        });
        let moved = reset_system(&f.dirs, ROLE_LEAD).unwrap();
        heal();
        let Some(Moved::Aside(kept)) = moved else { panic!("set aside: {moved:?}") };
        assert_eq!(std::fs::read_to_string(kept.join(TEXT_FILE)).unwrap(), "saved while it was copied");
        assert_eq!(std::fs::read_to_string(kept.join("added.md")).unwrap(), "added while it was copied");
        assert_eq!(std::fs::read_to_string(kept.join("notes.md")).unwrap(), "notes");
        assert!(backups(&f).is_empty(), "the copy that was not whole is taken out");
        assert!(!path.parent().unwrap().exists(), "out of its place");
        assert_eq!(resolved(&f.dirs).state().state, SystemState::Luna);
        assert_eq!(set_aside(&f.dirs), [kept.to_string_lossy().into_owned()], "named to the window");
    }

    /// An override that is a second name of another file (a hard link), or a
    /// file of another user's: not the person's own file, so not served, nor
    /// opened in the editor. Read through the window, both say why.
    #[test]
    fn an_override_not_of_the_person_s_own_making_is_not_served() {
        let f = Fixture::new("system-own");
        let path = write_override(&f, OURS);
        let other = f.base.join("other.md");
        std::fs::write(&other, OURS).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::hard_link(&other, &path).unwrap();
        let r = resolved(&f.dirs);
        assert_eq!(r.source, Source::Luna);
        assert!(r.problem.as_deref().is_some_and(|p| p.contains("not a file of yours")), "{:?}", r.problem);
        assert!(read_system(&f.dirs, ROLE_LEAD).err().is_some_and(|e| e.contains("not a file of yours")));
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, OURS).unwrap();
        assert_eq!(resolved(&f.dirs).source, Source::Library, "its own file again");
        // Another user's, where this test may hand one over.
        #[cfg(unix)]
        if unsafe { libc::geteuid() } == 0 {
            std::os::unix::fs::chown(&path, Some(4242), Some(4242)).unwrap();
            assert!(resolved(&f.dirs).problem.is_some_and(|p| p.contains("owned by uid 4242")));
        }
    }

    /// A link planted above what the window changes, `library-backups/` one to
    /// elsewhere: Back to Luna's does not go through it, and nothing there is
    /// touched.
    #[test]
    fn a_link_above_what_is_changed_is_refused() {
        let f = Fixture::new("system-ancestors");
        let elsewhere = f.base.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::create_dir_all(&f.dirs.data).unwrap();
        link_dir(&f.dirs.backups(), &elsewhere);
        let path = write_override(&f, OURS);
        let e = reset_system(&f.dirs, ROLE_LEAD).unwrap_err();
        assert!(e.contains("no private folder of Luna's"), "{e}");
        assert!(path.is_file(), "the override stays");
        assert!(names(&elsewhere).is_empty(), "nothing put through the link");
        unlink_dir(&f.dirs.backups());
    }

    /// The library's own folder may be reached through a link of the person's
    /// choosing: what lies below it is judged against where it leads.
    #[test]
    fn a_library_reached_through_a_link_of_its_own_still_works() {
        let f = Fixture::new("system-root-link");
        let via = f.base.join("via");
        link_dir(&via, &f.dirs.root);
        let dirs = Dirs { root: via.clone(), data: f.dirs.data.clone() };
        write_system(&dirs, ROLE_LEAD, OURS).unwrap();
        assert_eq!(resolved(&dirs).source, Source::Library);
        assert!(reset_system(&dirs, ROLE_LEAD).unwrap().is_some());
        unlink_dir(&via);
    }
}
