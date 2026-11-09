//! System skills: text Luna serves at its own trigger, never laid out
//! (docs/adr/0022-content-luna-serves-at-its-own-trigger.md). There is one,
//! `session-handoff`, what a chat with auto-handoff on is given to write its
//! handoff by (agents/handoff). Luna's own text is compiled in, so it cannot be
//! missing; the person may override it with one file for everyone,
//! `<library>/system/session-handoff/SKILL.md`, checked as a library skill is.
//! An override that will not do is not served: Luna's own is, and the reason
//! goes with it.
//!
//! The pass reads only `skills/` and `instructions/`, so `system/` reaches no
//! account or folder. A library skill under `skills/` that takes a system
//! skill's name is not its override (`resolve_in` never reads it) and is laid
//! out nowhere (`reserved`): every session it went to would hear of the
//! handoff, which only a chat with auto-handoff on may.
//!
//! - `handoff_instructions`: what is served now, `Resolved`, with where the
//!   override is or would go (`path`) and why one there is not used.
//! - `Resolved::state`: the same as the window is shown it, `SystemSkillState`.
//! - `LUNA_HANDOFF`: Luna's own text, what an override starts from or is
//!   compared with.
//! - `read_system`, `write_system`, `reset_system`, `adopt_system`: what the
//!   window's System skills section does with an override (commands.rs),
//!   nowhere but `system/<name>` and Luna's backups, and never through a link
//!   anywhere below the library's folder (`inside`). A replaced override goes
//!   to the backups whole, by a rename; an adoption is staged and swapped in
//!   under a lock the resolver shares (`system_lock`), undone if it fails.

use super::*;

/// The handoff's instructions, by their skill name: what the override's folder
/// and its front matter are called, and the name a library skill does not take.
pub const HANDOFF: &str = "session-handoff";

/// Under the library's root: the system skills' overrides, a folder each.
const SYSTEM_DIR: &str = "system";

/// The most an override's SKILL.md may hold: all of it goes into the chat's
/// context at once, as a tool's answer.
pub const SYSTEM_MAX_BYTES: usize = 64 * 1024;

/// Luna's own instructions for a handoff: its version's, compiled in.
pub const LUNA_HANDOFF: &str = include_str!("../../resources/skills/session-handoff/SKILL.md");

/// Whose text is served.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    /// Luna's own, of this version.
    Luna,
    /// The person's override in the library.
    Library,
}

/// What a system skill resolves to now.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    pub source: Source,
    /// The SKILL.md served, front matter and all.
    pub text: String,
    /// SHA-256 of `text`, as hex: what tells one text from another.
    pub sha256: String,
    /// Where the override is, or would be: used only when `source` says so.
    pub path: PathBuf,
    /// Why the override there is not used: there is one, and it will not do.
    pub problem: Option<String>,
}

impl Resolved {
    /// The text past its front matter: what a session reads.
    pub fn body(&self) -> &str {
        body_of(&self.text)
    }

    /// The first 12 characters of the hash, as the window and a log line say it.
    pub fn short_sha(&self) -> &str {
        &self.sha256[..self.sha256.len().min(12)]
    }

    /// As the window shows it.
    pub fn state(&self) -> SystemSkillState {
        SystemSkillState {
            name: HANDOFF,
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

/// Where a system skill stands, for the window.
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

/// A system skill as the window is shown it: whose text is served, Luna's
/// version, the hash of what is served, where the override is or would go, and
/// why it is not used when it is not.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SystemSkillState {
    pub name: &'static str,
    pub state: SystemState,
    /// Luna's version: whose own text this is, where it is Luna's.
    pub version: &'static str,
    pub sha256: String,
    pub path: String,
    pub problem: Option<String>,
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// `text` past its front matter, the blank lines after it too; all of it when
/// it has none.
pub fn body_of(text: &str) -> &str {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.split_inclusive('\n');
    if lines.next().map(str::trim_end) != Some("---") {
        return text;
    }
    let mut at = text.find('\n').map_or(text.len(), |i| i + 1);
    for line in lines {
        at += line.len();
        if line.trim_end() == "---" {
            return text[at..].trim_start_matches(['\r', '\n']);
        }
    }
    text
}

impl Dirs {
    fn system(&self, name: &str) -> PathBuf {
        self.root.join(SYSTEM_DIR).join(name)
    }
}

/// The system skills there are, by name.
const SYSTEM_SKILLS: &[&str] = &[HANDOFF];

/// Luna's own text of the system skill `name`; an error for any other name,
/// so that nothing the window asks for here reaches past `system/<name>`.
pub(super) fn luna_text(name: &str) -> Result<&'static str, String> {
    match name {
        HANDOFF => Ok(LUNA_HANDOFF),
        _ => Err(format!("{name} is not a system skill")),
    }
}

/// Whether a library skill named `name` takes a system skill's name: then it
/// is not that skill's override, and it is laid out nowhere (pass.rs `units`,
/// folders.rs `hidden_here`), its copies taken out as those of a skill no
/// longer wanted. Laid out, it would tell every session of the handoff, which
/// only a chat with auto-handoff on may hear of.
pub(super) fn reserved(name: &str) -> bool {
    SYSTEM_SKILLS.contains(&name)
}

/// Why a library skill of a system skill's name is laid out nowhere, and why
/// no new one is made under it.
pub(super) fn reserved_why(name: &str) -> String {
    format!(
        "{name} is the name of Luna's system skill for handoffs: no skill of that name is laid out, so that no session without \
         Auto-handoff hears of it. Your own text for it is the override, under System skills"
    )
}

/// The most of an override the editor opens: one larger than Luna serves
/// (SYSTEM_MAX_BYTES) still opens, to be cut down to size.
const EDIT_MAX_BYTES: u64 = 1024 * 1024;

fn too_large() -> String {
    format!("SKILL.md is over {} KB", SYSTEM_MAX_BYTES / 1024)
}

/// `rel` under `root`, walked down from it (`boundary::walk_down`): nothing on
/// the way below `root` a link, a junction or a file in a folder's place, and
/// the part of it that is there inside `root` once both are resolved. A check
/// of the last part alone would let an ancestor planted as a link (`skills/` a
/// junction to an account's) take what follows elsewhere. `root` itself is
/// where the person put the library, a link of their choosing if they chose
/// one, and the person's folder, so only looked at: not made private as
/// Luna's own are (`boundary::private_chain`). The last parts need not be there.
fn inside(root: &Path, rel: &[&str]) -> Result<PathBuf, String> {
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
fn plain_tree(dir: &Path) -> Result<(), String> {
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

/// Beside the ledger in Luna's data: held shared while the override is read
/// to be served (`resolve_in`) and exclusive while an adoption swaps one in,
/// so no handoff reads the moment between its renames, when there is none. A
/// file lock: the window adopts, the core serves.
const SYSTEM_LOCK: &str = "library-system.lock";

/// The lock on `system/`, held for as long as it lives; none where the lock
/// file will not open, as with the library's own lock (`lock`).
struct SystemLock {
    _file: Option<std::fs::File>,
}

fn system_lock(dirs: &Dirs, exclusive: bool) -> SystemLock {
    let path = dirs.data.join(SYSTEM_LOCK);
    let _ = std::fs::create_dir_all(&dirs.data);
    let file = crate::secret::open_private(&path).and_then(|f| {
        let locked = if exclusive { f.lock() } else { f.lock_shared() };
        locked.map(|()| f)
    });
    if let Err(e) = &file {
        crate::log::warn("library", &format!("system lock {}: {e}", path.display()));
    }
    SystemLock { _file: file.ok() }
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
/// save made meanwhile included. Where the backups are on another volume than
/// the library, every file is copied first and the folder removed only once
/// the copy is whole. Where it went.
fn move_to_backups(dirs: &Dirs, dir: &Path, was: &Path) -> Result<PathBuf, String> {
    // Luna's own, in its data folder: no link, this user's, private, made so
    // before anything is put in it.
    crate::boundary::private_chain(&dirs.backups(), &[])?;
    let slot = backup_slot(dirs, was);
    std::fs::create_dir(&slot).map_err(|e| format!("{}: {e}", slot.display()))?;
    let undo = || {
        let _ = std::fs::remove_file(slot.join("from.txt"));
        let _ = std::fs::remove_dir(&slot);
    };
    if let Err(e) = write_file(&slot.join("from.txt"), was.to_string_lossy().as_bytes()) {
        undo();
        return Err(e);
    }
    let to = slot.join(was.file_name().unwrap_or_default());
    match rename("backup", dir, &to) {
        Ok(()) => return Ok(slot),
        Err(e) if e.kind() == std::io::ErrorKind::CrossesDevices => {}
        Err(e) => {
            undo();
            return Err(format!("{} to {}: {e}", dir.display(), to.display()));
        }
    }
    if let Err(e) = copy_tree(dir, &to) {
        let _ = std::fs::remove_dir_all(&to);
        undo();
        return Err(format!("{} to {}: {e}", dir.display(), to.display()));
    }
    std::fs::remove_dir_all(dir)
        .map_err(|e| format!("{} is kept in {}, but not all of it could be taken out: {e}", dir.display(), slot.display()))?;
    Ok(slot)
}

/// Why this SKILL.md would not do as the override of `name`, if it would not:
/// checked as a library skill's is, held to SYSTEM_MAX_BYTES, and with
/// something past its front matter.
pub(super) fn check_override(name: &str, text: &str) -> Result<(), String> {
    if text.len() > SYSTEM_MAX_BYTES {
        return Err(too_large());
    }
    check_skill(name, text)?;
    if body_of(text).trim().is_empty() {
        return Err("SKILL.md has nothing past its front matter".into());
    }
    Ok(())
}

/// The override of `name` for the editor; where there is none yet, Luna's own
/// text to start one from, at the path saving it makes (`exists` false).
pub(super) fn read_system(dirs: &Dirs, name: &str) -> Result<LibraryFile, String> {
    let luna = luna_text(name)?;
    let path = dirs.system(name).join(SKILL_FILE);
    let shown = path.to_string_lossy().into_owned();
    let start = || LibraryFile { path: shown.clone(), text: luna.to_string(), exists: false };
    let Some(dir) = override_dir(dirs, name)? else { return Ok(start()) };
    match read_skill_md(&dir, EDIT_MAX_BYTES + 1)? {
        // A folder without SKILL.md: what saving puts there starts from Luna's.
        None => Ok(start()),
        Some(bytes) if bytes.len() as u64 > EDIT_MAX_BYTES => {
            Err(format!("{} is over {} KB, too large to open here", path.display(), EDIT_MAX_BYTES / 1024))
        }
        Some(bytes) => {
            let text = String::from_utf8(bytes).map_err(|_| format!("{} is not UTF-8 text", path.display()))?;
            Ok(LibraryFile { path: shown, text, exists: true })
        }
    }
}

/// Saves the editor's text as the override of `name`. One that will not do
/// (`check_override`) is still saved, as a library skill's draft is: Luna
/// serves its own meanwhile and says why. More than Luna would ever serve is
/// refused, and so is a link or a file in the way.
pub(super) fn write_system(dirs: &Dirs, name: &str, text: &str) -> Result<PathBuf, String> {
    luna_text(name)?;
    if text.len() > SYSTEM_MAX_BYTES {
        return Err(format!("{}, more than Luna serves: not saved", too_large()));
    }
    let path = inside(&dirs.root, &[SYSTEM_DIR, name, SKILL_FILE])?;
    if let Ok(m) = std::fs::symlink_metadata(&path) {
        if !m.is_file() {
            return Err(format!("{} is a folder — Luna leaves it alone", path.display()));
        }
    }
    // Through a temp file and a rename (`Dirs::write`): a handoff reads the
    // old text or the new, never part of either.
    dirs.write(&path, text.as_bytes())?;
    Ok(path)
}

/// Back to Luna's own for `name`: the override's folder, as it is, moves into
/// Luna's backups (kept BACKUP_DAYS, as a replaced library file is), `system/`
/// going too once empty. Where it was put, or None where there was none. A
/// handoff meanwhile reads the override or none, either a text it may be
/// served, so this takes no system lock.
pub(super) fn reset_system(dirs: &Dirs, name: &str) -> Result<Option<PathBuf>, String> {
    luna_text(name)?;
    let Some(dir) = override_dir(dirs, name)? else { return Ok(None) };
    plain_tree(&dir)?;
    let saved = move_to_backups(dirs, &dir, &dir)?;
    let _ = std::fs::remove_dir(dirs.root.join(SYSTEM_DIR));
    Ok(Some(saved))
}

/// Makes the library skill named as the system skill `name` its override:
/// the skill's folder moves to where the override goes, its SKILL.md the one
/// served and the files beside it there for the text to name. Moved as it is:
/// one that will not do is then an override that will not do, shown so, and
/// Luna's own served.
///
/// Staged first: the skill is renamed into `system/` under a name nothing
/// reads, the step that fails where its folder is held open, before anything
/// served changes. Then, under the system lock, an override there is renamed
/// aside and the staged one into its place, each undone if the next fails; the
/// one set aside goes to the backups after.
pub(super) fn adopt_system(dirs: &Dirs, name: &str) -> Result<(), String> {
    luna_text(name)?;
    let from = inside(&dirs.root, &["skills", name])?;
    match std::fs::symlink_metadata(&from) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(format!("The library has no skill named {name}")),
        Err(e) => return Err(format!("{}: {e}", from.display())),
        Ok(m) if !m.is_dir() => return Err(format!("{} is not a skill's folder", from.display())),
        Ok(_) => {}
    }
    plain_tree(&from)?;
    if !from.join(SKILL_FILE).is_file() {
        return Err(format!("{} has no SKILL.md", from.display()));
    }
    let current = override_dir(dirs, name)?;
    if let Some(dir) = &current {
        plain_tree(dir)?;
        // Where the one replaced goes, sound before anything moves.
        crate::boundary::private_chain(&dirs.backups(), &[])?;
    }
    let system = inside(&dirs.root, &[SYSTEM_DIR])?;
    std::fs::create_dir_all(&system).map_err(|e| format!("{}: {e}", system.display()))?;
    let to = system.join(name);
    let staged = system.join(format!(".{name}.adopting-{}", crate::secret::random_hex(4)));
    if let Err(e) = rename("stage", &from, &staged) {
        let _ = std::fs::remove_dir(&system);
        return Err(format!("{} to {}: {e}", from.display(), staged.display()));
    }
    let held = system_lock(dirs, true);
    let aside = match current {
        Some(dir) => {
            let aside = system.join(format!(".{name}.replaced-{}", crate::secret::random_hex(4)));
            if let Err(e) = rename("aside", &dir, &aside) {
                let _ = std::fs::rename(&staged, &from);
                return Err(format!("{} to {}: {e}", dir.display(), aside.display()));
            }
            Some(aside)
        }
        None => None,
    };
    if let Err(e) = rename("swap", &staged, &to) {
        if let Some(aside) = &aside {
            let _ = std::fs::rename(aside, &to);
        }
        let _ = std::fs::rename(&staged, &from);
        return Err(format!("{} to {}: {e}", staged.display(), to.display()));
    }
    drop(held);
    if let Some(aside) = aside {
        // In place already; the one replaced stays set aside in `system/`,
        // read by nothing, where it will not move.
        if let Err(e) = move_to_backups(dirs, &aside, &to) {
            crate::log::warn("library", &format!("the override replaced is kept at {}: {e}", aside.display()));
        }
    }
    Ok(())
}

/// Luna's own text of a system skill beside the library's override of it.
#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SystemCompare {
    pub luna: String,
    /// The override's SKILL.md; None where there is none.
    pub yours: Option<String>,
}

pub(super) fn compare_system(dirs: &Dirs, name: &str) -> Result<SystemCompare, String> {
    let luna = luna_text(name)?.to_string();
    let file = read_system(dirs, name)?;
    Ok(SystemCompare { luna, yours: file.exists.then_some(file.text) })
}

/// What a chat with auto-handoff on is served to write its handoff by, now:
/// the override when there is one that will do, else Luna's own.
pub fn handoff_instructions() -> Resolved {
    resolve_in(&Dirs::current())
}

pub(super) fn resolve_in(dirs: &Dirs) -> Resolved {
    let path = dirs.system(HANDOFF).join(SKILL_FILE);
    let luna = |problem: Option<String>| Resolved {
        source: Source::Luna,
        text: LUNA_HANDOFF.to_string(),
        sha256: sha256_hex(LUNA_HANDOFF.as_bytes()),
        path: path.clone(),
        problem,
    };
    let held = system_lock(dirs, false);
    let read = read_override(dirs);
    drop(held);
    match read {
        Ok(None) => luna(None),
        Ok(Some(text)) => {
            Resolved { source: Source::Library, sha256: sha256_hex(text.as_bytes()), text, path: path.clone(), problem: None }
        }
        Err(why) => luna(Some(why)),
    }
}

/// The override's SKILL.md if it will do; None where there is none; why not
/// where it will not. Its front matter checked as a library skill's is
/// (`check_skill`), and held to SYSTEM_MAX_BYTES of UTF-8 with something past
/// its front matter. SKILL.md alone is read, as it alone is served: opened as
/// itself (no link at it, `system/` or its folder; a FIFO there no reason to
/// wait, `boundary::open_no_follow`) and read no further than the cap, so what
/// lies beside it cannot hold up or swell the read, which the handoff's beat
/// makes for every firm notice.
fn read_override(dirs: &Dirs) -> Result<Option<String>, String> {
    let Some(dir) = override_dir(dirs, HANDOFF)? else { return Ok(None) };
    let bytes = read_skill_md(&dir, SYSTEM_MAX_BYTES as u64 + 1)?.ok_or("There is no SKILL.md")?;
    if bytes.len() > SYSTEM_MAX_BYTES {
        return Err(too_large());
    }
    let text = String::from_utf8(bytes).map_err(|_| "SKILL.md is not UTF-8 text".to_string())?;
    check_override(HANDOFF, &text)?;
    Ok(Some(text))
}

/// The override's SKILL.md in `dir`, opened as itself
/// (`boundary::open_no_follow`: no link at it, and no FIFO there to wait on),
/// held to a plain file of one name of this user's making
/// (`secret::own_file`; not `read_private`, as a library of the person's
/// choosing keeps its own folder's access) and read no further than `cap`
/// bytes; None where there is none.
fn read_skill_md(dir: &Path, cap: u64) -> Result<Option<Vec<u8>>, String> {
    use std::io::Read;
    let path = dir.join(SKILL_FILE);
    let not_a_file = || format!("{} is a link or not a file; the override is the file itself", path.display());
    match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
        Ok(m) if crate::boundary::is_link(&m) || !m.is_file() => return Err(not_a_file()),
        Ok(_) => {}
    }
    let file = crate::boundary::open_no_follow(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    // The handle's own word: what was opened, not what was at the name a
    // moment before.
    let opened = file.metadata().map_err(|e| format!("{}: {e}", path.display()))?;
    if crate::boundary::is_link(&opened) || !opened.is_file() {
        return Err(not_a_file());
    }
    // The person's own file, in a folder of theirs that need not be private
    // (a library of their choosing keeps its folder's access): of their
    // making, of one name, not another user's or another file's second name.
    match crate::secret::own_file(&file) {
        Ok(Ok(())) => {}
        Ok(Err(why)) => return Err(format!("{} is not a file of yours ({why}); the override is your own", path.display())),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    }
    let mut bytes = Vec::new();
    file.take(cap).read_to_end(&mut bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Some(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_override(f: &Fixture, text: &str) -> PathBuf {
        let path = f.dirs.system(HANDOFF).join(SKILL_FILE);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn luna_s_own_text_is_a_skill_of_its_name() {
        assert!(check_skill(HANDOFF, LUNA_HANDOFF).is_ok(), "{:?}", check_skill(HANDOFF, LUNA_HANDOFF));
        assert!(LUNA_HANDOFF.len() <= SYSTEM_MAX_BYTES);
        assert!(!body_of(LUNA_HANDOFF).starts_with("---") && !body_of(LUNA_HANDOFF).trim().is_empty());
    }

    /// Luna's own text goes in with LF endings from every checkout
    /// (.gitattributes), so the hash a handoff's record names is the same on
    /// every machine; a change to the text changes it here, on purpose.
    #[test]
    fn luna_s_own_text_has_one_hash_from_every_checkout() {
        assert!(!LUNA_HANDOFF.contains('\r'));
        assert_eq!(sha256_hex(LUNA_HANDOFF.as_bytes()), "3054a9e14c774f592c9c2fed08da4ff7eb38befdb00321184f3d1b324c8aedd9");
    }

    /// The override is served, never laid out: a pass that lays the
    /// library's skills and instructions into every account leaves no trace of
    /// it in any of them.
    #[test]
    fn the_override_reaches_no_account() {
        let f = Fixture::new("system-nowhere");
        write_override(&f, "---\nname: session-handoff\ndescription: Ours.\n---\nBody\n");
        f.skill("review", "Reviews.");
        f.instructions("main", "Be brief.\n");
        let lib = LibrarySettings { skills: skills(&["review"]), instructions: first(all()), ..Default::default() };
        let p = f.run(&lib, &mut Ledger::default(), true);
        assert!(!p.places.is_empty() && f.account(0).join("skills").join("review").is_dir(), "the rest is laid out");
        fn walk(dir: &Path, found: &mut Vec<PathBuf>) {
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.contains(HANDOFF) || name == SYSTEM_DIR {
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
        assert!(p.places.iter().all(|place| !place.dest.contains(HANDOFF)));
    }

    #[test]
    fn without_an_override_luna_s_own_is_served() {
        let f = Fixture::new("system-none");
        let r = resolve_in(&f.dirs);
        assert_eq!((r.source, r.problem.as_deref(), r.text.as_str()), (Source::Luna, None, LUNA_HANDOFF));
        assert_eq!(r.sha256, sha256_hex(LUNA_HANDOFF.as_bytes()));
        assert_eq!(r.path, f.dirs.root.join("system").join("session-handoff").join("SKILL.md"));
        let s = r.state();
        assert_eq!((s.state, s.version, s.name), (SystemState::Luna, env!("CARGO_PKG_VERSION"), "session-handoff"));
        // A library skill of the same name is not the override.
        f.skill(HANDOFF, "Mine, laid out as a skill.");
        assert_eq!(resolve_in(&f.dirs).source, Source::Luna, "skills/session-handoff is never read for it");
    }

    #[test]
    fn an_override_that_will_do_is_served_over_luna_s() {
        let f = Fixture::new("system-ok");
        let text = "---\nname: session-handoff\ndescription: Ours.\n---\n\n# Our handoff\nWrite it our way.\n";
        let path = write_override(&f, text);
        let r = resolve_in(&f.dirs);
        assert_eq!((r.source, r.problem.as_deref(), r.text.as_str(), &r.path), (Source::Library, None, text, &path));
        assert_eq!(r.sha256, sha256_hex(text.as_bytes()));
        assert_eq!(r.body(), "# Our handoff\nWrite it our way.\n");
        assert_eq!(r.state().state, SystemState::Library);
        assert_eq!(r.short_sha().len(), 12);
    }

    #[test]
    fn a_broken_override_falls_back_to_luna_s_and_says_why() {
        let f = Fixture::new("system-broken");
        for (text, why) in [
            ("no front matter", "front matter"),
            ("---\nname: other\ndescription: x\n---\nbody", "other"),
            ("---\nname: session-handoff\n---\nbody", "description"),
            ("---\nname: session-handoff\ndescription: x\n---\n \n", "nothing past"),
        ] {
            write_override(&f, text);
            let r = resolve_in(&f.dirs);
            assert_eq!((r.source, r.text.as_str()), (Source::Luna, LUNA_HANDOFF), "{text}");
            assert!(r.problem.as_deref().is_some_and(|p| p.contains(why)), "{text}: {:?}", r.problem);
            assert_eq!(r.state().state, SystemState::Broken);
        }
        let path = write_override(&f, "x");
        std::fs::write(&path, [0x2d, 0x2d, 0x2d, 0x0a, 0xff]).unwrap();
        assert!(resolve_in(&f.dirs).problem.unwrap().contains("UTF-8"));
        let big = format!("---\nname: session-handoff\ndescription: x\n---\n{}", "a".repeat(SYSTEM_MAX_BYTES));
        std::fs::write(&path, big).unwrap();
        assert!(resolve_in(&f.dirs).problem.unwrap().contains("KB"));
        // A folder without SKILL.md is an override that will not do, not none.
        std::fs::remove_file(&path).unwrap();
        assert!(resolve_in(&f.dirs).problem.unwrap().contains("no SKILL.md"));
    }

    const OURS: &str = "---\nname: session-handoff\ndescription: Ours.\n---\nBody\n";

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
    fn only_skill_md_is_read_and_no_further_than_the_cap() {
        let f = Fixture::new("system-bounded");
        let path = write_override(&f, OURS);
        let dir = path.parent().unwrap().to_path_buf();
        // What lies beside it is never read: a file many times the cap, a link.
        std::fs::write(dir.join("notes.md"), vec![b'a'; SYSTEM_MAX_BYTES * 8]).unwrap();
        let _ = make_link(&dir.join("notes.md"), &dir.join("linked.md"));
        assert_eq!(resolve_in(&f.dirs).source, Source::Library);
        // SKILL.md a link to a file that would do: not the file itself.
        let real = f.base.join("elsewhere.md");
        std::fs::write(&real, OURS).unwrap();
        std::fs::remove_file(&path).unwrap();
        if make_link(&real, &path) {
            assert!(resolve_in(&f.dirs).problem.unwrap().contains("link or not a file"));
            std::fs::remove_file(&path).unwrap();
        }
        std::fs::create_dir(&path).unwrap();
        assert!(resolve_in(&f.dirs).problem.unwrap().contains("not a file"), "a folder at its name");
    }

    /// A FIFO beside SKILL.md, or at its name, holds no read up: the beat that
    /// resolves the instructions for a firm notice is every chat's.
    #[cfg(unix)]
    #[test]
    fn a_fifo_in_the_override_s_folder_holds_nothing_up() {
        let f = Fixture::new("system-fifo");
        let path = write_override(&f, OURS);
        let fifo = |p: &Path| {
            let c = std::ffi::CString::new(p.to_string_lossy().as_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        };
        let resolved = |f: &Fixture| {
            let dirs = Dirs { root: f.dirs.root.clone(), data: f.dirs.data.clone() };
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || tx.send(resolve_in(&dirs)));
            rx.recv_timeout(std::time::Duration::from_secs(5)).expect("resolved without waiting on a FIFO")
        };
        fifo(&path.with_file_name("pipe.md"));
        assert_eq!(resolved(&f).source, Source::Library, "beside it");
        std::fs::remove_file(&path).unwrap();
        fifo(&path);
        assert!(resolved(&f).problem.unwrap().contains("not a file"), "at its name");
    }

    #[test]
    fn front_matter_is_left_out_of_what_is_read() {
        assert_eq!(body_of("---\nname: a\n---\n\nBody\n"), "Body\n");
        assert_eq!(body_of("\u{feff}---\r\nname: a\r\n---\r\nBody"), "Body");
        assert_eq!(body_of("No front matter\n---\n"), "No front matter\n---\n");
        assert_eq!(body_of("---\nnever closed\n"), "---\nnever closed\n");
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
        let start = read_system(&f.dirs, HANDOFF).unwrap();
        let path = f.dirs.root.join("system").join(HANDOFF).join(SKILL_FILE);
        assert_eq!((start.text.as_str(), start.exists, &start.path), (LUNA_HANDOFF, false, &path.to_string_lossy().into_owned()));
        assert!(!f.dirs.root.join("system").exists(), "reading makes nothing");
        assert_eq!(compare_system(&f.dirs, HANDOFF).unwrap(), SystemCompare { luna: LUNA_HANDOFF.into(), yours: None });

        assert_eq!(write_system(&f.dirs, HANDOFF, OURS).unwrap(), path);
        let r = resolve_in(&f.dirs);
        assert_eq!((r.source, r.text.as_str()), (Source::Library, OURS));
        let read = read_system(&f.dirs, HANDOFF).unwrap();
        assert_eq!((read.text.as_str(), read.exists), (OURS, true));
        assert_eq!(compare_system(&f.dirs, HANDOFF).unwrap().yours.as_deref(), Some(OURS));

        // One that will not do is saved all the same, and Luna's own served.
        write_system(&f.dirs, HANDOFF, "---\nname: session-handoff\n---\nNo description\n").unwrap();
        let r = resolve_in(&f.dirs);
        assert_eq!((r.source, r.state().state), (Source::Luna, SystemState::Broken));
        assert!(r.problem.unwrap().contains("description"));
        assert_eq!(check_override(HANDOFF, OURS), Ok(()));
        assert!(check_override(HANDOFF, "---\nname: session-handoff\ndescription: x\n---\n").unwrap_err().contains("nothing past"));
    }

    /// Only a system skill's own folder is reached, whatever name the window
    /// sends: the commands refuse any other before they touch anything.
    #[test]
    fn no_name_but_a_system_skill_s_reaches_anywhere() {
        let f = Fixture::new("system-names");
        f.skill("review", "v1");
        for name in ["review", "../skills/review", "..", "", "system", "Session-Handoff"] {
            assert!(read_system(&f.dirs, name).is_err(), "{name}");
            assert!(write_system(&f.dirs, name, OURS).is_err(), "{name}");
            assert!(reset_system(&f.dirs, name).is_err(), "{name}");
            assert!(adopt_system(&f.dirs, name).is_err(), "{name}");
            assert!(compare_system(&f.dirs, name).is_err(), "{name}");
            assert!(library_check_system(name.into(), OURS.into()).is_some(), "{name}");
            assert!(library_system_compare(name.into()).is_err(), "{name}");
            assert!(library_system_reset(name.into()).is_err(), "{name}");
            assert!(library_system_adopt(name.into()).is_err(), "{name}");
            assert!(library_read(Entry::System(name.into())).is_err(), "{name}");
            assert!(library_write(Entry::System(name.into()), OURS.into()).is_err(), "{name}");
        }
        assert!(f.dirs.skills().join("review").join(SKILL_FILE).is_file(), "the skill there is untouched");
        assert!(!f.dirs.root.join("system").exists());
        assert_eq!(library_check_system(HANDOFF.into(), OURS.into()), None);
        // It has no audience, and its name takes no skill of yours.
        assert!(library_set_audience(Entry::System(HANDOFF.into()), Audience::Chats).err().is_some_and(|e| e.contains("no audience")));
        assert!(library_new_skill(HANDOFF.into()).err().is_some_and(|e| e.contains("System skills")));
        assert!(library_check_skill(HANDOFF.into(), OURS.into()).is_some_and(|why| why.contains("laid out")));
    }

    #[test]
    fn a_link_on_the_way_is_neither_read_nor_written_through_nor_taken_out() {
        let f = Fixture::new("system-link");
        let elsewhere = f.base.join("elsewhere");
        std::fs::create_dir_all(elsewhere.join(HANDOFF)).unwrap();
        std::fs::write(elsewhere.join(HANDOFF).join(SKILL_FILE), OURS).unwrap();
        let system = f.dirs.root.join("system");
        link_dir(&system, &elsewhere);
        assert!(read_system(&f.dirs, HANDOFF).err().is_some_and(|e| e.contains("link")));
        assert!(write_system(&f.dirs, HANDOFF, "theirs").unwrap_err().contains("link"));
        assert!(reset_system(&f.dirs, HANDOFF).unwrap_err().contains("link"));
        f.skill(HANDOFF, "mine");
        assert!(adopt_system(&f.dirs, HANDOFF).unwrap_err().contains("link"));
        assert!(compare_system(&f.dirs, HANDOFF).is_err());
        assert_eq!(std::fs::read_to_string(elsewhere.join(HANDOFF).join(SKILL_FILE)).unwrap(), OURS, "nothing written there");
        assert!(f.dirs.skills().join(HANDOFF).join(SKILL_FILE).is_file(), "the skill stays where it was");
        assert!(backups(&f).is_empty());
        unlink_dir(&system);

        // A link inside the override's folder: its folder is not taken out
        // through it, nor moved into the backups.
        let dir = f.dirs.root.join("system").join(HANDOFF);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(SKILL_FILE), OURS).unwrap();
        link_dir(&dir.join("more"), &elsewhere);
        assert!(reset_system(&f.dirs, HANDOFF).unwrap_err().contains("link"));
        assert!(dir.join(SKILL_FILE).is_file() && elsewhere.join(HANDOFF).join(SKILL_FILE).is_file());
        unlink_dir(&dir.join("more"));
        // The skill to be made the override, a link: not moved.
        std::fs::remove_dir_all(f.dirs.skills().join(HANDOFF)).unwrap();
        link_dir(&f.dirs.skills().join(HANDOFF), &elsewhere.join(HANDOFF));
        assert!(adopt_system(&f.dirs, HANDOFF).unwrap_err().contains("link"));
        assert_eq!(std::fs::read_to_string(dir.join(SKILL_FILE)).unwrap(), OURS);
        unlink_dir(&f.dirs.skills().join(HANDOFF));
    }

    /// A FIFO in the override's folder or in the skill to make it: refused at
    /// once, never read whole and waited on with the library's lock held; and
    /// one at SKILL.md's name is no file for the editor either.
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
        assert!(within(Box::new(move || reset_system(&d, HANDOFF).map(|_| ()))).unwrap_err().contains("not a plain file"));
        assert!(path.is_file(), "nothing taken out");
        std::fs::remove_file(path.with_file_name("pipe.md")).unwrap();
        f.skill(HANDOFF, "Mine.");
        fifo(&f.dirs.skills().join(HANDOFF).join("pipe.md"));
        let d = dirs();
        assert!(within(Box::new(move || adopt_system(&d, HANDOFF))).unwrap_err().contains("not a plain file"));
        assert!(f.dirs.skills().join(HANDOFF).join(SKILL_FILE).is_file(), "nothing moved");
        std::fs::remove_file(&path).unwrap();
        fifo(&path);
        let d = dirs();
        assert!(within(Box::new(move || read_system(&d, HANDOFF).map(|_| ()))).unwrap_err().contains("not a file"));
    }

    #[test]
    fn an_oversized_override_is_not_saved_and_one_too_large_to_edit_not_opened() {
        let f = Fixture::new("system-size");
        let big = format!("---\nname: session-handoff\ndescription: x\n---\n{}", "a".repeat(SYSTEM_MAX_BYTES));
        assert!(write_system(&f.dirs, HANDOFF, &big).unwrap_err().contains("not saved"));
        assert!(!f.dirs.root.join("system").exists());
        assert!(check_override(HANDOFF, &big).unwrap_err().contains("KB"));
        // Over what Luna serves but not what the editor opens: opened, to be
        // cut down; Luna's own is served meanwhile.
        let path = write_override(&f, &big);
        assert!(read_system(&f.dirs, HANDOFF).unwrap().exists);
        assert_eq!(resolve_in(&f.dirs).state().state, SystemState::Broken);
        std::fs::write(&path, vec![b'a'; EDIT_MAX_BYTES as usize + 1]).unwrap();
        assert!(read_system(&f.dirs, HANDOFF).err().is_some_and(|e| e.contains("too large")));
    }

    #[test]
    fn back_to_luna_s_keeps_the_override_in_the_backups() {
        let f = Fixture::new("system-reset");
        assert_eq!(reset_system(&f.dirs, HANDOFF), Ok(None), "none to take out");
        let path = write_override(&f, OURS);
        std::fs::write(path.with_file_name("example.md"), "beside it").unwrap();
        let saved = reset_system(&f.dirs, HANDOFF).unwrap().expect("backed up");
        assert_eq!(std::fs::read_to_string(saved.join(HANDOFF).join(SKILL_FILE)).unwrap(), OURS);
        assert_eq!(std::fs::read_to_string(saved.join(HANDOFF).join("example.md")).unwrap(), "beside it");
        assert!(saved.starts_with(f.dirs.backups()));
        assert!(!f.dirs.root.join("system").exists(), "nothing left of it in the library");
        assert_eq!(resolve_in(&f.dirs).state().state, SystemState::Luna);
    }

    #[test]
    fn a_skill_of_the_reserved_name_becomes_the_override_files_and_all() {
        let f = Fixture::new("system-adopt");
        assert!(adopt_system(&f.dirs, HANDOFF).unwrap_err().contains("no skill named"));
        f.skill(HANDOFF, "Ours, from before there was an override.");
        std::fs::write(f.dirs.skills().join(HANDOFF).join("template.md"), "# Template").unwrap();
        adopt_system(&f.dirs, HANDOFF).unwrap();
        let dir = f.dirs.root.join("system").join(HANDOFF);
        assert!(!f.dirs.skills().join(HANDOFF).exists(), "moved, not copied");
        assert_eq!(std::fs::read_to_string(dir.join("template.md")).unwrap(), "# Template");
        let r = resolve_in(&f.dirs);
        assert_eq!(r.source, Source::Library);
        assert!(r.body().contains("Ours, from before"));
        assert!(backups(&f).is_empty(), "nothing was there to replace");

        // Made the override where there is one: that one goes to the backups.
        f.skill(HANDOFF, "Newer.");
        adopt_system(&f.dirs, HANDOFF).unwrap();
        assert!(resolve_in(&f.dirs).body().contains("Newer."));
        assert!(!dir.join("template.md").exists(), "the folder moved in whole");
        let saved = backups(&f);
        assert_eq!(saved.len(), 1);
        assert_eq!(std::fs::read_to_string(saved[0].join(HANDOFF).join("template.md")).unwrap(), "# Template");

        // Without a SKILL.md it is no skill to make the override.
        std::fs::create_dir_all(f.dirs.skills().join(HANDOFF)).unwrap();
        assert!(adopt_system(&f.dirs, HANDOFF).unwrap_err().contains("no SKILL.md"));
        assert_eq!(names(&f.dirs.root.join("system")), [HANDOFF], "nothing staged or set aside is left");
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

    /// An adoption that fails at any step leaves what was served, and the
    /// skill, as they were: the move that may fail (its folder held open) comes
    /// before anything served changes, and the swap is undone.
    #[test]
    fn an_adoption_that_fails_changes_nothing() {
        let f = Fixture::new("system-adopt-fails");
        write_override(&f, OURS);
        f.skill(HANDOFF, "Mine.");
        let served = resolve_in(&f.dirs).sha256;
        for step in ["stage", "aside", "swap"] {
            fail(step, std::io::ErrorKind::PermissionDenied);
            assert!(adopt_system(&f.dirs, HANDOFF).is_err(), "{step}");
            heal();
            let r = resolve_in(&f.dirs);
            assert_eq!((r.source, &r.sha256), (Source::Library, &served), "{step}: the override as it was");
            assert!(f.dirs.skills().join(HANDOFF).join(SKILL_FILE).is_file(), "{step}: the skill where it was");
            assert_eq!(names(&f.dirs.root.join("system")), [HANDOFF], "{step}: nothing staged or set aside is left");
            assert!(backups(&f).is_empty(), "{step}");
        }
        // Then it goes through.
        adopt_system(&f.dirs, HANDOFF).unwrap();
        assert!(resolve_in(&f.dirs).body().contains("Mine."));
        assert_eq!(backups(&f).len(), 1, "the one replaced");
    }

    /// The source folder held open with no sharing, as an editor or a scanner
    /// may hold a file on Windows: its rename fails, and the override stays.
    #[cfg(windows)]
    #[test]
    fn an_adoption_of_a_folder_held_open_changes_nothing() {
        use std::os::windows::fs::OpenOptionsExt;
        let f = Fixture::new("system-adopt-held");
        write_override(&f, OURS);
        f.skill(HANDOFF, "Mine.");
        let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(f.dirs.skills().join(HANDOFF).join(SKILL_FILE)).unwrap();
        assert!(adopt_system(&f.dirs, HANDOFF).is_err());
        drop(held);
        assert_eq!(resolve_in(&f.dirs).text, OURS);
        assert_eq!(names(&f.dirs.root.join("system")), [HANDOFF]);
        assert!(f.dirs.skills().join(HANDOFF).join(SKILL_FILE).is_file());
    }

    /// While an adoption holds the system lock, what is served waits for it:
    /// no handoff reads the moment between its renames.
    #[test]
    fn what_is_served_waits_for_an_adoption_s_swap() {
        let f = Fixture::new("system-lock");
        write_override(&f, OURS);
        let held = system_lock(&f.dirs, true);
        let dirs = Dirs { root: f.dirs.root.clone(), data: f.dirs.data.clone() };
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || tx.send(resolve_in(&dirs).source));
        assert!(rx.recv_timeout(std::time::Duration::from_millis(300)).is_err(), "it waits");
        drop(held);
        assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap(), Source::Library);
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
            std::fs::write(dir.join("SKILL.md.1-0.luna-tmp"), "a save under way").unwrap();
            std::fs::create_dir_all(dir.join("more")).unwrap();
            std::fs::write(dir.join("more").join("notes.md"), "notes").unwrap();
            if across {
                fail("backup", std::io::ErrorKind::CrossesDevices);
            }
            let saved = reset_system(&f.dirs, HANDOFF).unwrap().unwrap().join(HANDOFF);
            heal();
            assert_eq!(std::fs::read_to_string(saved.join(SKILL_FILE)).unwrap(), OURS, "across: {across}");
            assert_eq!(std::fs::read_to_string(saved.join("SKILL.md.1-0.luna-tmp")).unwrap(), "a save under way");
            assert_eq!(std::fs::read_to_string(saved.join("more").join("notes.md")).unwrap(), "notes");
            assert!(!dir.exists() && !f.dirs.root.join("system").exists(), "across: {across}");
        }
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
        let r = resolve_in(&f.dirs);
        assert_eq!(r.source, Source::Luna);
        assert!(r.problem.as_deref().is_some_and(|p| p.contains("not a file of yours")), "{:?}", r.problem);
        assert!(read_system(&f.dirs, HANDOFF).err().is_some_and(|e| e.contains("not a file of yours")));
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, OURS).unwrap();
        assert_eq!(resolve_in(&f.dirs).source, Source::Library, "its own file again");
        // Another user's, where this test may hand one over.
        #[cfg(unix)]
        if unsafe { libc::geteuid() } == 0 {
            std::os::unix::fs::chown(&path, Some(4242), Some(4242)).unwrap();
            assert!(resolve_in(&f.dirs).problem.is_some_and(|p| p.contains("owned by uid 4242")));
        }
    }

    /// A link planted above what the window changes: `skills/` a junction to
    /// an account's, `library-backups/` one to elsewhere. Neither adoption nor
    /// Back to Luna's goes through it, and nothing there is touched.
    #[test]
    fn a_link_above_what_is_changed_is_refused() {
        let f = Fixture::new("system-ancestors");
        let theirs = f.account(0).join("skills");
        std::fs::create_dir_all(theirs.join(HANDOFF)).unwrap();
        std::fs::write(theirs.join(HANDOFF).join(SKILL_FILE), OURS).unwrap();
        link_dir(&f.dirs.skills(), &theirs);
        assert!(adopt_system(&f.dirs, HANDOFF).unwrap_err().contains("link"));
        assert!(theirs.join(HANDOFF).join(SKILL_FILE).is_file(), "the account's skill stays where it is");
        assert!(!f.dirs.root.join("system").exists());
        unlink_dir(&f.dirs.skills());

        let elsewhere = f.base.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::create_dir_all(&f.dirs.data).unwrap();
        link_dir(&f.dirs.backups(), &elsewhere);
        let path = write_override(&f, OURS);
        let e = reset_system(&f.dirs, HANDOFF).unwrap_err();
        assert!(e.contains("no private folder of Luna's"), "{e}");
        assert!(path.is_file(), "the override stays");
        assert!(names(&elsewhere).is_empty(), "nothing put through the link");
        // Nor an adoption over an override, whose replaced one goes there too:
        // refused before anything moves.
        f.skill(HANDOFF, "Mine.");
        assert!(adopt_system(&f.dirs, HANDOFF).unwrap_err().contains("no private folder of Luna's"));
        assert_eq!(resolve_in(&f.dirs).text, OURS);
        assert!(f.dirs.skills().join(HANDOFF).join(SKILL_FILE).is_file());
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
        write_system(&dirs, HANDOFF, OURS).unwrap();
        assert_eq!(resolve_in(&dirs).source, Source::Library);
        assert!(reset_system(&dirs, HANDOFF).unwrap().is_some());
        unlink_dir(&via);
    }

    /// A skill of the reserved name among the library's is laid out nowhere:
    /// not into an account, not for a folder's sessions, and its name hidden
    /// from no session's start; a copy of it laid out before goes, unless
    /// changed there since.
    #[test]
    fn a_skill_of_the_reserved_name_is_laid_out_nowhere_and_its_old_copies_go() {
        let f = Fixture::new("system-reserved");
        f.skill(HANDOFF, "Mine.");
        f.skill("review", "v1");
        let repo = f.base.join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let folder = LibraryFolder {
            path: repo.to_string_lossy().into_owned(),
            // Kept from every account's sessions there, as `review` is.
            skills: BTreeMap::from([(HANDOFF.to_string(), Vec::new()), ("review".to_string(), Vec::new())]),
            slug: "repo".into(),
            ..Default::default()
        };
        let mut lib = LibrarySettings { skills: skills(&[HANDOFF, "review"]), folders: vec![folder], ..Default::default() };
        lib.audience.skills.insert(HANDOFF.into(), Audience::Chats);
        let found = read_skills(&f.dirs).unwrap();
        assert!(found[HANDOFF].0.problem.as_deref().is_some_and(|p| p.contains("System skills")));
        assert!(found["review"].0.problem.is_none());

        // Copies an earlier Luna laid out: one as it wrote it, one changed since.
        let mut ledger = Ledger::default();
        for i in [0, 2] {
            let dest = f.account(i).join("skills").join(HANDOFF);
            let files = Files::from([(SKILL_FILE.to_string(), b"old".to_vec())]);
            write_at(&dest, false, &files).unwrap();
            ledger.wrote(&dest.to_string_lossy(), hashes(&files), Audience::Both);
        }
        std::fs::write(f.account(2).join("skills").join(HANDOFF).join(SKILL_FILE), "changed there").unwrap();

        let p = f.run(&lib, &mut ledger, true);
        assert!(p.places.iter().all(|place| place.skill.as_deref() != Some(HANDOFF)), "no place wants it");
        assert!(f.account(0).join("skills").join("review").is_dir(), "the rest is laid out");
        assert!(!f.account(0).join("skills").join(HANDOFF).exists(), "Luna's old copy taken out");
        assert!(!f.account(1).join("skills").join(HANDOFF).exists());
        assert_eq!(p.kept, [f.account(2).join("skills").join(HANDOFF).to_string_lossy().into_owned()], "a changed one stays");
        for a in &f.accounts {
            for kind in [SessionKind::Chat, SessionKind::Helper] {
                let s = folder_args(&f.dirs, &lib, a, &repo.to_string_lossy(), kind);
                let said = format!("{:?} {:?} {:?}", s.args, s.hidden, s.audience);
                assert!(!said.contains(HANDOFF), "{}: {said}", a.name);
                assert!(said.contains("review"), "{}: the folder still keeps review: {said}", a.name);
            }
        }
    }
}
