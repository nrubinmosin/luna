//! The handoff file a session names, checked and copied into Luna's own
//! folder before anything else happens: what the next conversation reads is
//! the copy, fixed at the moment of the call, with its SHA-256 on record. The
//! file is data the next conversation reads, never run.
//!
//! What is taken: an absolute path to a Markdown or text file, itself no link
//! or junction (opened so that one is not followed), at most MAX_BYTES of
//! UTF-8 text with something in it, and not inside Luna's data folder,
//! wherever its path leads: that is where the core's token and other chats'
//! handoffs are, and Luna would otherwise copy for a session what its own
//! read deny keeps from it (threat model F3).

use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

/// The most a handoff file may hold: a page or two of judgment is what a
/// handoff is, and the next conversation reads all of it first.
pub(in crate::agents) const MAX_BYTES: u64 = 128 * 1024;

/// The extensions a handoff file may have.
const EXTENSIONS: [&str; 3] = ["md", "markdown", "txt"];

/// A file taken: Luna's copy, its hash, and the path it was named by.
#[derive(Debug)]
pub(super) struct Snapshot {
    pub path: PathBuf,
    pub sha256: String,
    pub source: String,
    pub size: usize,
}

/// `file` checked as the module says, its bytes as they were read.
pub(super) fn read_checked(file: &str, data_dir: &Path) -> Result<(PathBuf, Vec<u8>), String> {
    let path = crate::boundary::absolute(file.trim()).map_err(|e| format!("file: {e}; give the absolute path of the handoff"))?;
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if !EXTENSIONS.contains(&ext.as_str()) {
        return Err(format!("file: {} is not a .md (or .txt) file; write the handoff in Markdown", path.display()));
    }
    let meta = std::fs::symlink_metadata(&path).map_err(|e| format!("file: {}: {e}", path.display()))?;
    if is_link(&meta) || !meta.is_file() {
        return Err(format!("file: {} is a link, a junction or not a file; give the file itself", path.display()));
    }
    // Where the path leads, links on the way followed: Luna's folder is not
    // to be reached through one.
    let real = std::fs::canonicalize(&path).map_err(|e| format!("file: {}: {e}", path.display()))?;
    if let Ok(data) = std::fs::canonicalize(data_dir) {
        if real.starts_with(&data) {
            return Err(format!(
                "file: {} is inside Luna's own folder; write the handoff in your project or a temporary folder",
                path.display()
            ));
        }
    }
    let mut f = open_no_follow(&path).map_err(|e| format!("file: {}: {e}", path.display()))?;
    // The handle's own word: what was opened is a file, not what the look
    // above saw at that name a moment before.
    let opened = f.metadata().map_err(|e| format!("file: {}: {e}", path.display()))?;
    if is_link(&opened) || !opened.is_file() {
        return Err(format!("file: {} is a link, a junction or not a file; give the file itself", path.display()));
    }
    let mut bytes = Vec::new();
    f.by_ref().take(MAX_BYTES + 1).read_to_end(&mut bytes).map_err(|e| format!("file: {}: {e}", path.display()))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(format!(
            "file: {} is over {} KB; a handoff says what matters, and points at the rest",
            path.display(),
            MAX_BYTES / 1024
        ));
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| format!("file: {} is not UTF-8 text", path.display()))?;
    if text.contains('\0') {
        return Err(format!("file: {} holds a NUL byte; it is not text", path.display()));
    }
    if text.trim().is_empty() {
        return Err(format!("file: {} is empty", path.display()));
    }
    Ok((path, bytes))
}

/// Checks `file` and copies it to `<dir>/<n>.md`, made private and new: a
/// file already there under that number is never written into.
pub(super) fn snapshot(file: &str, data_dir: &Path, dir: &Path, n: u32) -> Result<Snapshot, String> {
    let (source, bytes) = read_checked(file, data_dir)?;
    std::fs::create_dir_all(dir).map_err(|e| format!("could not make {}: {e}", dir.display()))?;
    let path = dir.join(format!("{n}.md"));
    let mut out = crate::secret::create_private(&path).map_err(|e| format!("could not write {}: {e}", path.display()))?;
    std::io::Write::write_all(&mut out, &bytes).map_err(|e| format!("could not write {}: {e}", path.display()))?;
    let sha256 = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    Ok(Snapshot { path, sha256, source: source.to_string_lossy().into_owned(), size: bytes.len() })
}

/// A link or a junction, whatever std calls it: Windows counts any reparse
/// point that stands for another name as one.
fn is_link(meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    meta.file_type().is_symlink()
}

/// The file at `path` itself, a link there opened as the link (and then
/// refused by its metadata) rather than followed.
fn open_no_follow(path: &Path) -> std::io::Result<std::fs::File> {
    let mut o = std::fs::OpenOptions::new();
    o.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        o.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.custom_flags(libc::O_NOFOLLOW);
    }
    o.open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("luna-handoff-file-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_handoff_file_is_copied_whole_with_its_hash() {
        let dir = scratch("ok");
        let data = dir.join("data");
        std::fs::create_dir_all(&data).unwrap();
        let file = dir.join("handoff.md");
        std::fs::write(&file, "# Objective\nShip it.\n").unwrap();
        let out = data.join("handoffs").join("c1");
        let s = snapshot(&file.to_string_lossy(), &data, &out, 1).unwrap();
        assert_eq!(s.path, out.join("1.md"));
        assert_eq!(std::fs::read_to_string(&s.path).unwrap(), "# Objective\nShip it.\n");
        let expected: String = Sha256::digest(b"# Objective\nShip it.\n").iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(s.sha256, expected);
        // A later change to the file is not the snapshot's.
        std::fs::write(&file, "changed").unwrap();
        assert_eq!(std::fs::read_to_string(&s.path).unwrap(), "# Objective\nShip it.\n");
        // A number taken is never written over.
        assert!(snapshot(&file.to_string_lossy(), &data, &out, 1).unwrap_err().contains("could not write"));
        assert!(snapshot(&file.to_string_lossy(), &data, &out, 2).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_is_not_a_handoff_file_is_refused_and_nothing_copied() {
        let dir = scratch("bad");
        let data = dir.join("data");
        std::fs::create_dir_all(data.join("core")).unwrap();
        let out = dir.join("out");
        let refused = |file: &Path, why: &str| {
            let e = snapshot(&file.to_string_lossy(), &data, &out, 1).unwrap_err();
            assert!(e.contains(why), "{}: {e}", file.display());
        };
        let write = |name: &str, bytes: &[u8]| {
            let p = dir.join(name);
            std::fs::write(&p, bytes).unwrap();
            p
        };
        refused(Path::new("handoff.md"), "absolute");
        refused(&dir.join("..").join("x.md"), "absolute");
        refused(&write("notes.json", b"{}"), "Markdown");
        refused(&write("key", b"secret"), "Markdown");
        refused(&dir.join("missing.md"), "missing.md");
        std::fs::create_dir_all(dir.join("folder.md")).unwrap();
        refused(&dir.join("folder.md"), "not a file");
        refused(&write("empty.md", b" \n\t"), "empty");
        refused(&write("nul.md", b"a\0b"), "NUL");
        refused(&write("latin.md", &[0x66, 0xe9, 0x65]), "UTF-8");
        refused(&write("big.md", &vec![b'a'; MAX_BYTES as usize + 1]), "KB");
        assert!(snapshot(&write("max.md", &vec![b'a'; MAX_BYTES as usize]).to_string_lossy(), &data, &out, 9).is_ok());
        // Inside Luna's folder: the core's token is there.
        std::fs::write(data.join("core").join("core.md"), "token").unwrap();
        refused(&data.join("core").join("core.md"), "Luna's own folder");
        // A link, to Luna's folder or anywhere: where links can be made here.
        let link = dir.join("link.md");
        if make_link(&data.join("core").join("core.md"), &link) {
            refused(&link, "link");
        }
        assert!(!out.join("1.md").exists(), "nothing refused was copied");
        let _ = std::fs::remove_dir_all(&dir);
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
}
