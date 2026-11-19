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
/// there beyond them goes, and folders that leaves empty go too. For the
/// library's own files, and a test's; a copy in a destination goes through
/// `write_over`, which leaves alone what is not Luna's there.
pub(super) fn write_at(dest: &Path, file: bool, files: &Files) -> Result<(), String> {
    let there = if file || !dest.is_dir() { Files::new() } else { read_dir_files(dest)? };
    write_over(dest, file, files, &there)?;
    if !file {
        prune_empty(dest);
    }
    Ok(())
}

/// Makes Luna's part of a destination `files`, where `there` is that part as
/// it was just read (`luna_part`): what of `there` the library no longer has
/// goes, with the folders that leaves empty, and what differs is written.
/// Nothing else in a skill's folder is looked at, written or taken out.
pub(super) fn write_over(dest: &Path, file: bool, files: &Files, there: &Files) -> Result<(), String> {
    if file {
        return write_copy(dest, files.get("").map(Vec::as_slice).unwrap_or_default());
    }
    std::fs::create_dir_all(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    // What goes, goes first: a file renamed only in case (`guide.md` to
    // `GUIDE.md`) is one file where case is ignored, which a write and then
    // the old name's removal would leave with neither.
    for rel in there.keys().filter(|r| !files.contains_key(*r)) {
        remove_file_in(dest, rel)?;
    }
    for (rel, bytes) in files {
        if there.get(rel) != Some(bytes) {
            write_copy(&join_rel(dest, rel), bytes)?;
        }
    }
    Ok(())
}

/// The file `rel` under `dest` taken out, and each folder on the way to it
/// that this leaves empty, up to `dest` itself (not included).
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

/// Takes Luna's part of a destination out, `there` as `luna_part` just read
/// it: the file, or in a skill's folder those files alone, then the folder
/// if that leaves it empty. What else a person or a tool put in it stays,
/// and so does the folder around it.
pub(super) fn take_out(dest: &Path, file: bool, there: &Files) -> Result<(), String> {
    if file {
        return std::fs::remove_file(dest).map_err(|e| format!("{}: {e}", dest.display()));
    }
    for rel in there.keys() {
        remove_file_in(dest, rel)?;
    }
    let _ = std::fs::remove_dir(dest);
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
