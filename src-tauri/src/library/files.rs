//! A copy's files on disk, read and written whole, and what Luna keeps of what
//! it wrote: the bytes by hash, for differences, and backups of the side that
//! loses when a changed copy is settled.

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
/// A link anywhere in it is an error: Luna neither follows one nor writes
/// over it.
pub(super) fn read_dir_files(dir: &Path) -> Result<Files, String> {
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
/// there beyond them goes, and folders that leaves empty go too. Only ever on
/// Luna's own copy, or with the user's word and a backup taken.
pub(super) fn write_at(dest: &Path, file: bool, files: &Files) -> Result<(), String> {
    if file {
        return write_copy(dest, files.get("").map(Vec::as_slice).unwrap_or_default());
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
            write_copy(&join_rel(dest, rel), bytes)?;
        }
    }
    Ok(())
}

pub(super) fn remove_at(dest: &Path, file: bool) -> Result<(), String> {
    let r = if file { std::fs::remove_file(dest) } else { std::fs::remove_dir_all(dest) };
    r.map_err(|e| format!("{}: {e}", dest.display()))
}

/// Whether a ledger entry is a single file: it has the one key "".
pub(super) fn is_file(h: &Hashes) -> bool {
    h.len() == 1 && h.contains_key("")
}

/// The bytes of every file Luna writes, by hash: what a copy was when Luna
/// wrote it, for the differences shown once it has changed.
pub(super) fn keep_blobs(dirs: &Dirs, files: &Files) {
    for bytes in files.values() {
        let path = dirs.blobs().join(hash(bytes));
        if !path.exists() {
            let _ = write_file(&path, bytes);
        }
    }
}

pub(super) fn blob(dirs: &Dirs, h: &str) -> Option<Vec<u8>> {
    std::fs::read(dirs.blobs().join(h)).ok()
}

/// Blobs no ledger entry names any more, a kept copy's included: its
/// differences are shown too.
pub(super) fn sweep_blobs(dirs: &Dirs, ledger: &Ledger) {
    let wanted: BTreeSet<&String> = ledger.written.values().chain(ledger.kept.values()).flat_map(|h| h.values()).collect();
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
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S-%3f");
    let tail: Vec<String> = from.components().rev().take(2).map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    let tail: String = tail.into_iter().rev().collect::<Vec<_>>().join("-").chars().filter(|c| !matches!(c, ':' | '\\' | '/')).collect();
    let backups = dirs.backups();
    std::fs::create_dir_all(&backups).map_err(|e| format!("{}: {e}", backups.display()))?;
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
/// (`no_link_between`), the deepest of those holding it: an account folder
/// (one may lie in Luna's data folder); Luna's data folder, whose
/// `library-folders/` is Luna's own and so looked at too; or a folder the
/// library lists, for a copy an earlier Luna laid into it itself
/// (`<folder>/<name>`), listed whether or not anything goes there now. None
/// for a path in none of them: such a copy is never taken out.
pub(super) fn top_of(dirs: &Dirs, accounts: &[AccountInfo], folders: &[LibraryFolder], dest: &Path) -> Option<PathBuf> {
    accounts
        .iter()
        .map(|a| PathBuf::from(&a.path))
        .chain([dirs.data.clone()])
        .chain(folders.iter().map(|f| PathBuf::from(&f.path)))
        .filter(|t| within(dest, t))
        .max_by_key(|t| t.components().count())
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

pub(super) fn sweep_backups(dirs: &Dirs) {
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

        // Two accounts' copies of one name, backed up in a row (one
        // millisecond, as a batch makes them): each keeps its own bytes.
        let (a, b) = (f.account(0).join("skills").join("review"), f.account(1).join("skills").join("review"));
        let one = back_up(&f.dirs, &a, &Files::from([("SKILL.md".to_string(), b"first".to_vec())])).unwrap();
        let two = back_up(&f.dirs, &b, &Files::from([("SKILL.md".to_string(), b"second".to_vec())])).unwrap();
        assert_ne!(one, two);
        assert_eq!(std::fs::read(one.join("review").join("SKILL.md")).unwrap(), b"first");
        assert_eq!(std::fs::read(two.join("review").join("SKILL.md")).unwrap(), b"second");
        assert_eq!(std::fs::read(one.join("from.txt")).unwrap(), a.to_string_lossy().as_bytes());

        // A blob no entry names goes at the sweep.
        ledger.written.clear();
        sweep_blobs(&f.dirs, &ledger);
        assert!(blob(&f.dirs, &hash(b"Be brief.\n")).is_none());
    }
}
