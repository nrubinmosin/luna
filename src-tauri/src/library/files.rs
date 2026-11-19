//! A copy's files on disk, read and written whole, and backups of what a
//! change there had made before the library's goes over it or it is taken
//! out.

use super::*;

fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

pub(super) fn hashes(files: &Files) -> Hashes {
    files.iter().map(|(k, v)| (k.clone(), hash(v))).collect()
}

fn join_rel(base: &Path, rel: &str) -> PathBuf {
    rel.split('/').filter(|s| !s.is_empty()).fold(base.to_path_buf(), |p, s| p.join(s))
}

/// Every file under `dir`, by relative path, `write_file`'s temp files aside.
/// A link or junction in it is no part of it: never followed, never written
/// over or taken out, and said in the log once a run.
pub(super) fn read_dir_files(dir: &Path) -> Result<Files, String> {
    walk_files(dir, false)
}

/// `read_dir_files` with the temp-named files too: all a backup is to keep
/// of a folder about to go whole.
pub(super) fn read_dir_files_all(dir: &Path) -> Result<Files, String> {
    walk_files(dir, true)
}

fn walk_files(dir: &Path, with_tmp: bool) -> Result<Files, String> {
    fn walk(dir: &Path, prefix: &str, with_tmp: bool, out: &mut Files) -> Result<(), String> {
        let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let rel = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
            let meta = std::fs::symlink_metadata(entry.path()).map_err(|e| format!("{}: {e}", entry.path().display()))?;
            if crate::boundary::is_link(&meta) {
                if link_is_news(&entry.path()) {
                    crate::log::info("library", &format!("{} is a link — Luna leaves it alone and reads past it", entry.path().display()));
                }
            } else if meta.is_dir() {
                walk(&entry.path(), &rel, with_tmp, out)?;
            } else if name.ends_with(TMP_SUFFIX) && !with_tmp {
                continue;
            } else {
                out.insert(rel, std::fs::read(entry.path()).map_err(|e| format!("{}: {e}", entry.path().display()))?);
            }
        }
        Ok(())
    }
    let mut out = Files::new();
    walk(dir, "", with_tmp, &mut out)?;
    Ok(out)
}

/// The end of `write_file`'s temp names. No file so named is read as part of
/// a skill or a copy: a skill's own file of the name would be written over by
/// a write beside it, and one left by a write that died taken for the skill's.
pub(super) const TMP_SUFFIX: &str = ".luna-tmp";

/// One of Luna's own files (the library, its blobs and backups), through a
/// temp file beside it, so a reader never sees half, made private as it is
/// made under a random name nobody can have ready
/// (`secret::write_private_as`): they sit in the data dir, which may be open
/// to others.
pub(super) fn write_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    crate::secret::write_private_as(path, bytes, TMP_SUFFIX).map_err(|e| format!("{}: {e}", path.display()))
}

/// A copy in a CLI's folder or a repo's, through a temp file beside it as
/// `write_file`'s, which takes the folder's ACL like everything else there:
/// the folder is the CLI's or the repo's, not Luna's to lock down. The temp
/// name is random and made new (`create_new`), so one planted ahead of it is
/// never written into, nor removed.
pub(super) fn write_copy(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}{TMP_SUFFIX}", crate::secret::random_hex(8)));
    let tmp = PathBuf::from(tmp);
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp).map_err(|e| format!("{}: {e}", path.display()))?;
    let wrote = file.write_all(bytes);
    // Closed before the rename, which Windows refuses for a file held open.
    drop(file);
    wrote.and_then(|()| std::fs::rename(&tmp, path)).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{}: {e}", path.display())
    })
}

/// `from` renamed `to` only where nothing is at `to`, the check and the move
/// one step of the OS's: a name taken meanwhile fails the move rather than
/// being replaced (Windows' `MoveFileExW` without `MOVEFILE_REPLACE_EXISTING`,
/// Linux's `renameat2(RENAME_NOREPLACE)`, macOS's `renamex_np(RENAME_EXCL)`).
/// A file system that cannot do it refuses the move.
pub(super) fn rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use windows::core::HSTRING;
        use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVE_FILE_FLAGS};
        // SAFETY: both names are HSTRINGs alive through the call.
        unsafe { MoveFileExW(&HSTRING::from(from.as_os_str()), &HSTRING::from(to.as_os_str()), MOVE_FILE_FLAGS(0)) }
            .map_err(|e| std::io::Error::from_raw_os_error(e.code().0 & 0xFFFF))
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        use std::os::unix::ffi::OsStrExt;
        let from = std::ffi::CString::new(from.as_os_str().as_bytes())?;
        let to = std::ffi::CString::new(to.as_os_str().as_bytes())?;
        // SAFETY: both are NUL-terminated strings alive through the call.
        #[cfg(target_os = "linux")]
        let moved = unsafe { libc::renameat2(libc::AT_FDCWD, from.as_ptr(), libc::AT_FDCWD, to.as_ptr(), libc::RENAME_NOREPLACE) };
        // SAFETY: as above.
        #[cfg(target_os = "macos")]
        let moved = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
        if moved == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = (from, to);
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "no rename that refuses to replace on this system"))
    }
}

/// Folders under `dir` that hold nothing any more, deepest first.
pub(super) fn prune_empty(dir: &Path) {
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
pub(super) fn read_at(dest: &Path, file: bool) -> Result<Option<Files>, String> {
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
/// there beyond them goes, and folders that leaves empty go too. For the
/// library's own files, and a test's; a copy in a destination goes through
/// `write_over`, which leaves alone what is not Luna's there.
pub(super) fn write_at(dest: &Path, file: bool, files: &Files) -> Result<(), String> {
    let there = read_at(dest, file)?.unwrap_or_default();
    write_over(dest, file, files, &there).map_err(|h| h.to_string())?;
    if !file {
        prune_empty(dest);
    }
    Ok(())
}

/// Why `write_over` or `take_out` stopped; what it had done by then is
/// undone, as far as that goes (`undone`).
#[derive(Debug, PartialEq)]
pub(super) enum Halt {
    /// A file was not as read any more (saved, made or deleted meanwhile).
    Changed(PathBuf),
    /// A step failed: a rename Windows refuses while the file is held open,
    /// say.
    Failed { error: String, undone: bool },
}

impl std::fmt::Display for Halt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Halt::Changed(p) => write!(f, "{} changed there while the library was laid out", p.display()),
            Halt::Failed { error, .. } => f.write_str(error),
        }
    }
}

#[cfg(test)]
thread_local! {
    /// Called with each file's path right before it is looked at again and
    /// written or taken out: where a test stands in for a CLI saving it.
    pub(super) static BEFORE_EACH: std::cell::RefCell<Option<Box<dyn FnMut(&Path)>>> = const { std::cell::RefCell::new(None) };
}

/// Whether the file at `path` is still `expected` (None: not there), looked
/// at right before Luna writes or takes it out. A link or a folder there is
/// never as read.
fn file_still(path: &Path, expected: Option<&Vec<u8>>) -> bool {
    #[cfg(test)]
    BEFORE_EACH.with(|h| {
        if let Some(h) = h.borrow_mut().as_mut() {
            h(path)
        }
    });
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => expected.is_none(),
        Ok(m) if m.is_file() && !crate::boundary::is_link(&m) => expected.is_some_and(|b| std::fs::read(path).is_ok_and(|now| now == *b)),
        _ => false,
    }
}

/// The steps a write or a removal has taken, each file with what it held
/// before (None: nothing), to put back should a later step stop.
struct Steps<'a> {
    dest: &'a Path,
    done: Vec<(String, Option<&'a Vec<u8>>)>,
}

impl<'a> Steps<'a> {
    fn path(&self, rel: &str) -> PathBuf {
        join_rel(self.dest, rel)
    }
    /// One file looked at again, as `before`, and then written (`Some`) or
    /// taken out.
    fn step(&mut self, rel: &str, before: Option<&'a Vec<u8>>, after: Option<&[u8]>) -> Result<(), Halt> {
        let path = self.path(rel);
        if !file_still(&path, before) {
            self.undo();
            return Err(Halt::Changed(path));
        }
        let done = match after {
            Some(bytes) => write_copy(&path, bytes),
            None => remove_file_in(self.dest, rel),
        };
        if let Err(error) = done {
            let undone = self.undo();
            return Err(Halt::Failed { error, undone });
        }
        self.done.push((rel.to_string(), before));
        Ok(())
    }
    /// Each step taken put back, the last first; whether all were.
    fn undo(&mut self) -> bool {
        let mut all = true;
        while let Some((rel, before)) = self.done.pop() {
            let back = match before {
                Some(bytes) => write_copy(&self.path(&rel), bytes),
                None => remove_file_in(self.dest, &rel),
            };
            all &= back.is_ok();
        }
        all
    }
}

/// Makes Luna's part of a destination `files`, where `there` is that part as
/// it was read (`luna_part`) and backed up: what of `there` the library no
/// longer has goes, with the folders that leaves empty, and what differs is
/// written. Each file is looked at again right before it is written or taken
/// out; one not as read (a CLI saving it meanwhile, or one made where there
/// was none) stops it, as does a step that fails, and what it had done is put
/// back, so the place is never left half the library's. Nothing else in a
/// skill's folder is looked at, written or taken out.
pub(super) fn write_over(dest: &Path, file: bool, files: &Files, there: &Files) -> Result<(), Halt> {
    if !file {
        std::fs::create_dir_all(dest).map_err(|e| Halt::Failed { error: format!("{}: {e}", dest.display()), undone: true })?;
    }
    let done = write_steps(&mut Steps { dest, done: Vec::new() }, files, there);
    // A folder made for a copy that did not get in.
    if done.is_err() && !file && there.is_empty() {
        let _ = std::fs::remove_dir(dest);
    }
    done
}

fn write_steps<'a>(steps: &mut Steps<'a>, files: &Files, there: &'a Files) -> Result<(), Halt> {
    // What goes, goes first: a file renamed only in case (`guide.md` to
    // `GUIDE.md`) is one file where case is ignored, which a write and then
    // the old name's removal would leave with neither.
    for (rel, before) in there.iter().filter(|(r, _)| !files.contains_key(*r)) {
        steps.step(rel, Some(before), None)?;
    }
    for (rel, bytes) in files {
        if there.get(rel) != Some(bytes) {
            steps.step(rel, there.get(rel), Some(bytes))?;
        }
    }
    Ok(())
}

/// The file `rel` under `dest` (`dest` itself for ""), taken out, and each
/// folder on the way to it that this leaves empty, up to `dest` itself (not
/// included).
fn remove_file_in(dest: &Path, rel: &str) -> Result<(), String> {
    let p = join_rel(dest, rel);
    std::fs::remove_file(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    let mut at = p.parent();
    while let Some(dir) = at.filter(|d| d.starts_with(dest) && *d != dest) {
        if std::fs::remove_dir(dir).is_err() {
            break;
        }
        at = dir.parent();
    }
    Ok(())
}

/// Takes Luna's part of a destination out, `there` as `luna_part` read it
/// and it was backed up: the file, or in a skill's folder those files alone,
/// each looked at again right before it goes (`write_over`), then the folder
/// if that leaves it empty. What else a person or a tool put in it stays,
/// and so does the folder around it.
pub(super) fn take_out(dest: &Path, file: bool, there: &Files) -> Result<(), Halt> {
    let mut steps = Steps { dest, done: Vec::new() };
    for (rel, before) in there {
        steps.step(rel, Some(before), None)?;
    }
    if !file {
        let _ = std::fs::remove_dir(dest);
    }
    Ok(())
}

/// The part of what a destination holds that is Luna's to compare, write
/// over or take out: the file itself, or in a skill's folder the files at
/// the paths in `owned` (what Luna wrote there, and what the library has for
/// it). Whatever else a person or a tool put in the folder (a `__pycache__/`,
/// a `node_modules/`, a `.DS_Store`, a file of its own) is left out of it,
/// so it is never taken for a change, backed up or removed. Paths are
/// matched as the file system tells them apart: without case on Windows.
pub(super) fn luna_part<'a>(all: Files, file: bool, owned: impl IntoIterator<Item = &'a String>) -> Files {
    if file {
        return all;
    }
    let key = |r: &str| if cfg!(windows) { r.to_lowercase() } else { r.to_string() };
    let owned: BTreeSet<String> = owned.into_iter().map(|r| key(r)).collect();
    all.into_iter().filter(|(rel, _)| owned.contains(&key(rel))).collect()
}

/// Whether a ledger entry is a single file: it has the one key "".
pub(super) fn is_file(h: &Hashes) -> bool {
    h.len() == 1 && h.contains_key("")
}

/// 0.19's `library-blobs/` gone, at the first save of a ledger 0.19 wrote
/// (`Ledger::older`): never in a data folder no Luna of 0.19 used. A link
/// there goes as itself, never what it leads to; `remove_dir_all` takes no
/// link below it into what that leads to either.
pub(super) fn drop_old_blobs(dirs: &Dirs) {
    let old = dirs.old_blobs();
    match std::fs::symlink_metadata(&old) {
        Ok(m) if crate::boundary::is_link(&m) => {
            let _ = std::fs::remove_dir(&old).or_else(|_| std::fs::remove_file(&old));
        }
        Ok(m) if m.is_dir() => {
            let _ = std::fs::remove_dir_all(&old);
        }
        _ => {}
    }
}

/// Puts `files` — what is about to be replaced or taken out at `from` — under
/// a folder of its own in the backups, named for the moment and the last two
/// parts of the path (the whole path would outgrow Windows' limit): the file,
/// or the folder with all in it, by its own name, and the whole path beside it
/// in `from.txt`, where no file of a skill's own can be.
pub(super) fn back_up(dirs: &Dirs, from: &Path, files: &Files) -> Result<PathBuf, String> {
    let dir = backup_slot(dirs, from)?;
    let saved = dir.join(from.file_name().unwrap_or_default());
    for (rel, bytes) in files {
        let path = if rel.is_empty() { saved.clone() } else { join_rel(&saved, rel) };
        write_file(&path, bytes)?;
    }
    write_file(&dir.join("from.txt"), from.to_string_lossy().as_bytes())?;
    Ok(dir)
}

/// A new folder in the backups for what is replaced at `from`, named for the
/// moment and the last two parts of its path (`back_up`), made here and now:
/// two accounts' `skills/review` backed up in one millisecond would otherwise
/// share one, the second written over the first. A name taken gets a count.
pub(super) fn backup_slot(dirs: &Dirs, from: &Path) -> Result<PathBuf, String> {
    let stamp = jiff::ZonedDateTime::now().strftime("%Y%m%d-%H%M%S-%3f");
    let tail: Vec<String> = from.components().rev().take(2).map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    let tail: String = tail.into_iter().rev().collect::<Vec<_>>().join("-").chars().filter(|c| !matches!(c, ':' | '\\' | '/')).collect();
    let backups = dirs.backups();
    std::fs::create_dir_all(&backups).map_err(|e| format!("{}: {e}", backups.display()))?;
    // A link there (a junction to an archive, say) would take the backups,
    // and their sweep, wherever it leads.
    backups_are_lunas(dirs)?;
    for n in 1.. {
        let name = if n == 1 { format!("{stamp}-{tail}") } else { format!("{stamp}-{tail}-{n}") };
        let slot = backups.join(name);
        match std::fs::create_dir(&slot) {
            Ok(()) => return Ok(slot),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("{}: {e}", slot.display())),
        }
    }
    unreachable!("a count runs out of numbers before names")
}

/// The folder a copy at `dest` lies in that no link may stand below
/// (`no_link_between`). First the folders Luna manages copies in, the
/// deepest holding it: an account folder (one may lie in Luna's data folder),
/// or Luna's data folder, whose `library-folders/` is Luna's own and so looked
/// at too. Only where neither holds it, a folder the library lists, for a copy
/// an earlier Luna laid into it itself (`<folder>/<name>`), listed whether or
/// not anything goes there now: a listed folder inside an account's or
/// Luna's data folder would otherwise be a top whose own link goes
/// unlooked at. None for a path in none of them: such a copy is never taken
/// out.
pub(super) fn top_of(dirs: &Dirs, accounts: &[AccountInfo], folders: &[LibraryFolder], dest: &Path) -> Option<PathBuf> {
    let deepest = |tops: Vec<PathBuf>| tops.into_iter().filter(|t| within(dest, t)).max_by_key(|t| t.components().count());
    let managed = accounts.iter().map(|a| PathBuf::from(&a.path)).chain([dirs.data.clone()]).collect();
    deepest(managed).or_else(|| deepest(folders.iter().map(|f| PathBuf::from(&f.path)).collect()))
}

/// Refuses `dest` when a folder on the way down to it from `top` — the account
/// folder it lies in, or Luna's data folder for a copy waiting for a listed
/// folder's sessions — is a link or junction, or no folder: a write or a
/// removal there would land wherever the link leads. `top` itself is the
/// person's, a link of their choosing if they chose one, so only what lies
/// below it is looked at; `dest` itself is `read_at`'s to refuse. A folder not
/// there yet ends the look: nothing below it is either.
pub(super) fn no_link_between(top: &Path, dest: &Path) -> Result<(), String> {
    if !within(dest, top) {
        return Err(format!("{} lies outside {}", dest.display(), top.display()));
    }
    let below: Vec<String> = dest
        .parent()
        .map(|p| p.components().skip(top.components().count()).map(|c| c.as_os_str().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    let below: Vec<&str> = below.iter().map(String::as_str).collect();
    let mut gone = false;
    crate::boundary::walk_down(top, &below, |p, depth| {
        if depth == 0 || gone {
            return Ok(());
        }
        match std::fs::symlink_metadata(p) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => gone = true,
            Err(e) => return Err(format!("{}: {e}", p.display())),
            Ok(m) if crate::boundary::is_link(&m) => {
                return Err(format!("{} is a link or junction — Luna writes nothing through it", p.display()))
            }
            Ok(m) if !m.is_dir() => return Err(format!("{} is not a folder", p.display())),
            Ok(_) => {}
        }
        Ok(())
    })
    .map(|_| ())
}

/// The backups' folder as Luna's own: a folder, no link or junction.
fn backups_are_lunas(dirs: &Dirs) -> Result<(), String> {
    let backups = dirs.backups();
    match std::fs::symlink_metadata(&backups) {
        Ok(m) if crate::boundary::is_link(&m) => {
            Err(format!("{} is a link or junction — Luna keeps no backup through it", backups.display()))
        }
        Ok(m) if !m.is_dir() => Err(format!("{} is not a folder", backups.display())),
        Ok(_) => Ok(()),
        Err(e) => Err(format!("{}: {e}", backups.display())),
    }
}

/// Whether `name` is one `backup_slot` gives: `YYYYmmdd-HHMMSS-mmm-` and
/// the path's tail.
fn is_slot_name(name: &str) -> bool {
    let b = name.as_bytes();
    let digits = |r: std::ops::Range<usize>| b.get(r).is_some_and(|d| d.iter().all(u8::is_ascii_digit));
    b.len() > 20 && digits(0..8) && b[8] == b'-' && digits(9..15) && b[15] == b'-' && digits(16..19) && b[19] == b'-'
}

/// Backups past `BACKUP_DAYS` taken out: only folders of Luna's own slot
/// naming, each as itself. A link among them, or the backups' folder a link,
/// is left alone, and `remove_dir_all` follows no link below.
pub(super) fn sweep_backups(dirs: &Dirs) {
    sweep_backups_past(dirs, std::time::Duration::from_secs(BACKUP_DAYS * 86_400));
}

fn sweep_backups_past(dirs: &Dirs, limit: std::time::Duration) {
    if backups_are_lunas(dirs).is_err() {
        return;
    }
    if let Ok(entries) = std::fs::read_dir(dirs.backups()) {
        for entry in entries.flatten() {
            let Ok(meta) = std::fs::symlink_metadata(entry.path()) else { continue };
            if crate::boundary::is_link(&meta) || !meta.is_dir() || !is_slot_name(&entry.file_name().to_string_lossy()) {
                continue;
            }
            if meta.modified().ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > limit) {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn backups_keep_what_was_there_each_on_its_own() {
        let f = Fixture::new("backups");
        let dest = f.account(0).join("CLAUDE.md").to_string_lossy().into_owned();
        let saved = back_up(&f.dirs, Path::new(&dest), &Files::from([(String::new(), b"mine".to_vec())])).unwrap();
        assert_eq!(std::fs::read(saved.join("CLAUDE.md")).unwrap(), b"mine");
        // A skill's folder, a `from.txt` of its own in it: kept beside Luna's.
        let skill = f.dirs.root.join("skills").join("notes");
        let files = Files::from([("SKILL.md".to_string(), b"s".to_vec()), ("from.txt".to_string(), b"its own".to_vec())]);
        let saved = back_up(&f.dirs, &skill, &files).unwrap();
        assert_eq!(std::fs::read(saved.join("notes").join("from.txt")).unwrap(), b"its own");
        assert_eq!(std::fs::read(saved.join("from.txt")).unwrap(), skill.to_string_lossy().as_bytes());

        // Two accounts' copies of one name, backed up in a row (one
        // millisecond, as a batch makes them): each keeps its own bytes.
        let (a, b) = (f.account(0).join("skills").join("review"), f.account(1).join("skills").join("review"));
        let one = back_up(&f.dirs, &a, &Files::from([("SKILL.md".to_string(), b"first".to_vec())])).unwrap();
        let two = back_up(&f.dirs, &b, &Files::from([("SKILL.md".to_string(), b"second".to_vec())])).unwrap();
        assert_ne!(one, two);
        assert_eq!(std::fs::read(one.join("review").join("SKILL.md")).unwrap(), b"first");
        assert_eq!(std::fs::read(two.join("review").join("SKILL.md")).unwrap(), b"second");
        assert_eq!(std::fs::read(one.join("from.txt")).unwrap(), a.to_string_lossy().as_bytes());
    }

    /// 0.19's blobs go at the first save of a ledger 0.19 wrote, all of
    /// them; a link in their place goes as itself, what it leads to staying.
    /// A data folder with no such ledger keeps a `library-blobs` of its own.
    #[test]
    fn the_old_blobs_go_with_0_19_s_ledger_alone() {
        let f = Fixture::new("old-blobs");
        write_file(&f.dirs.old_blobs().join("00ab"), b"not Luna's").unwrap();
        Ledger::default().save(&f.dirs).unwrap();
        let mut ledger = Ledger::load(&f.dirs);
        ledger.save(&f.dirs).unwrap();
        assert!(f.dirs.old_blobs().join("00ab").is_file(), "no 0.19 ledger: not Luna's to take");

        let old_19 = |f: &Fixture| std::fs::write(f.dirs.ledger(), r#"{ "written": {} }"#).unwrap();
        old_19(&f);
        Ledger::load(&f.dirs).save(&f.dirs).unwrap();
        assert!(std::fs::symlink_metadata(f.dirs.old_blobs()).is_err());
        ledger = Ledger::load(&f.dirs);
        assert!(!ledger.older, "once");

        let elsewhere = f.base.join("elsewhere");
        write_file(&elsewhere.join("keep"), b"not Luna's").unwrap();
        link_dir(&f.dirs.old_blobs(), &elsewhere);
        old_19(&f);
        Ledger::load(&f.dirs).save(&f.dirs).unwrap();
        assert!(std::fs::symlink_metadata(f.dirs.old_blobs()).is_err());
        assert!(elsewhere.join("keep").is_file());
    }

    /// The backups' folder made a link (a junction on Windows): no backup is
    /// written through it, and the sweep takes nothing out through it. In
    /// the folder itself the sweep takes only Luna's own slots, and a link
    /// among them as nothing.
    #[test]
    fn backups_are_kept_and_swept_in_luna_s_own_folder_alone() {
        let f = Fixture::new("backups-link");
        let mine = Files::from([(String::new(), b"mine".to_vec())]);
        let at = f.account(0).join("CLAUDE.md");
        let slot = back_up(&f.dirs, &at, &mine).unwrap();
        let theirs = f.dirs.backups().join("archive");
        write_file(&theirs.join("keep"), b"theirs").unwrap();
        let elsewhere = f.base.join("elsewhere");
        write_file(&elsewhere.join("20200101-000000-000-x").join("keep"), b"theirs").unwrap();
        let linked = f.dirs.backups().join("20200101-000000-000-link");
        link_dir(&linked, &elsewhere);
        std::thread::sleep(std::time::Duration::from_millis(20));
        sweep_backups_past(&f.dirs, std::time::Duration::ZERO);
        assert!(!slot.exists(), "Luna's own slot swept");
        assert!(theirs.join("keep").is_file(), "a folder not of Luna's naming stays");
        assert!(elsewhere.join("20200101-000000-000-x").join("keep").is_file(), "nothing through a link");
        unlink_dir(&linked);

        std::fs::remove_dir_all(f.dirs.backups()).unwrap();
        link_dir(&f.dirs.backups(), &elsewhere);
        assert!(back_up(&f.dirs, &at, &mine).unwrap_err().contains("link"));
        sweep_backups_past(&f.dirs, std::time::Duration::ZERO);
        assert!(elsewhere.join("20200101-000000-000-x").join("keep").is_file());
        assert_eq!(walk(&elsewhere).len(), 1, "nothing written there");
        unlink_dir(&f.dirs.backups());
    }
}
