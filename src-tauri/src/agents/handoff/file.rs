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
//! read deny keeps from it (threat model F3). The one place there it is taken
//! from is right in the calling chat's own drafts folder (`own_draft`),
//! reached from the data folder by its own names, no link on the way.
//!
//! The read runs on a thread of its own and is given READ_WAIT: a path on a
//! share that does not answer, or something put at the name that blocks its
//! reader, holds up this one call and nothing else. A thread given up on may
//! stay in the filesystem as long as the share likes, so readers are counted
//! until they actually end (`Readers`): one per chat, READERS in all, and a
//! call past either is refused, not given a thread. The copy is on disk,
//! flushed, before the handoff counts on it.

use crate::boundary::{is_link, open_no_follow};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

/// The most a handoff file may hold: a page or two of judgment is what a
/// handoff is, and the next conversation reads all of it first.
pub(in crate::agents) const MAX_BYTES: u64 = 128 * 1024;

/// How long the file gets to be checked and read.
pub(super) const READ_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// The most reader threads there may be at once, those given up on included.
pub(super) const READERS: usize = 4;

/// The reader threads alive, by chat: what `read_within` admits a new one by.
pub(super) struct Readers {
    by_chat: std::sync::Mutex<std::collections::HashMap<String, usize>>,
    most: usize,
}

impl Readers {
    pub(super) fn new(most: usize) -> Self {
        Readers { by_chat: Default::default(), most }
    }

    /// A place for one more reader of `chat`'s, if it has none and there are
    /// fewer than `most` in all; held until the reader's thread ends.
    fn admit(&'static self, chat: &str) -> Result<Reading, String> {
        let mut m = self.by_chat.lock().unwrap_or_else(|e| e.into_inner());
        if m.get(chat).is_some_and(|n| *n > 0) {
            return Err("a read of an earlier handoff file of this chat has not come back yet (its disk or share does not \
                        answer); write the handoff on a local disk and call again once that one is through"
                .into());
        }
        if m.values().sum::<usize>() >= self.most {
            return Err(format!("{} handoff files are being read and have not come back; call again shortly", self.most));
        }
        *m.entry(chat.to_string()).or_default() += 1;
        Ok(Reading { readers: self, chat: chat.to_string() })
    }

    #[cfg(test)]
    fn alive(&self) -> usize {
        self.by_chat.lock().unwrap_or_else(|e| e.into_inner()).values().sum()
    }
}

/// A reader's place in `Readers`, given back as its thread ends.
struct Reading {
    readers: &'static Readers,
    chat: String,
}

impl Drop for Reading {
    fn drop(&mut self) {
        let mut m = self.readers.by_chat.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = m.get_mut(&self.chat) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                m.remove(&self.chat);
            }
        }
    }
}

/// Luna's readers. Tests share them, many reading at once: the cap a test
/// reaches is that of readers of its own (`Readers::new`).
fn readers() -> &'static Readers {
    static R: std::sync::OnceLock<Readers> = std::sync::OnceLock::new();
    R.get_or_init(|| Readers::new(if cfg!(test) { 64 } else { READERS }))
}

/// The extensions a handoff file may have.
const EXTENSIONS: [&str; 3] = ["md", "markdown", "txt"];

/// A file taken: Luna's copy, its hash, the path it was named by, and its text
/// as copied.
#[derive(Debug)]
pub(super) struct Snapshot {
    pub path: PathBuf,
    pub sha256: String,
    pub source: String,
    pub size: usize,
    pub text: String,
}

/// What a test has happen as a file is read, by its path: a slow share, a
/// look at what is held meanwhile.
#[cfg(test)]
pub(super) fn on_read(file: &str, f: impl FnOnce() + Send + 'static) {
    hooks().lock().unwrap_or_else(|e| e.into_inner()).insert(file.to_string(), Box::new(f));
}

#[cfg(test)]
type Hook = Box<dyn FnOnce() + Send>;

#[cfg(test)]
fn hooks() -> &'static std::sync::Mutex<std::collections::HashMap<String, Hook>> {
    static H: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, Hook>>> = std::sync::OnceLock::new();
    H.get_or_init(Default::default)
}

/// Whether `real`, a file's resolved path inside Luna's folder (`data`
/// resolved, `data_dir` as named), is right in the drafts folder of the chat
/// whose folder of copies is `own`, that folder resolving to where its names
/// say: a link at `handoffs`, the chat's folder or `drafts` would lead
/// elsewhere, and is no drafts folder.
fn own_draft(real: &Path, data_dir: &Path, data: &Path, own: &Path) -> bool {
    let drafts = own.join(super::DRAFTS);
    let Ok(rel) = drafts.strip_prefix(data_dir) else { return false };
    let Ok(resolved) = std::fs::canonicalize(&drafts) else { return false };
    resolved == data.join(rel) && real.parent() == Some(resolved.as_path())
}

/// `file` checked as the module says, its bytes as they were read, for the
/// chat whose folder of copies is `own`.
pub(super) fn read_checked(file: &str, data_dir: &Path, own: &Path) -> Result<(PathBuf, Vec<u8>), String> {
    // Taken out first: a hook that never returns is not to hold the map.
    #[cfg(test)]
    let hook = hooks().lock().unwrap_or_else(|e| e.into_inner()).remove(file);
    #[cfg(test)]
    if let Some(f) = hook {
        f();
    }
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
    // to be reached through one. And a path named inside it is held to the
    // drafts rule wherever it leads: one a link takes out of Luna's folder
    // (a `handoffs` or a drafts folder planted as one) is no drafts folder of
    // Luna's, whose file anyone could have put there.
    let real = std::fs::canonicalize(&path).map_err(|e| format!("file: {}: {e}", path.display()))?;
    let mut a_draft = false;
    if let Ok(data) = std::fs::canonicalize(data_dir) {
        let sep = std::path::MAIN_SEPARATOR;
        let named_inside = crate::paths::path_key(&path.to_string_lossy())
            .starts_with(&format!("{}{sep}", crate::paths::path_key(&data_dir.to_string_lossy())));
        a_draft = real.starts_with(&data) || named_inside;
        if a_draft && !own_draft(&real, data_dir, &data, own) {
            return Err(format!(
                "file: {} is inside Luna's own folder; write the handoff in this chat's drafts folder (luna_session_handoff \
                 without file names it) or outside Luna's folder",
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
    // In Luna's folder, the chat's own making: a file of one name this user
    // holds, not one another left there before the folder was Luna's alone.
    if a_draft {
        if let Err(why) = crate::secret::own_file(&f).map_err(|e| e.to_string())? {
            return Err(format!("file: {} is not this user's own ({why}); write the handoff anew", path.display()));
        }
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

/// `read_checked` for `chat` (its folder of copies `own`) on a thread of its
/// own, admitted by `readers` and given `wait`: a read that has not answered
/// by then is refused, and its thread left to end when it can, keeping its
/// place until it does.
pub(super) fn read_within(
    readers: &'static Readers,
    chat: &str,
    file: &str,
    data_dir: &Path,
    own: &Path,
    wait: std::time::Duration,
) -> Result<(PathBuf, Vec<u8>), String> {
    let place = readers.admit(chat)?;
    let (tx, rx) = std::sync::mpsc::channel();
    let (f, d, o) = (file.to_string(), data_dir.to_path_buf(), own.to_path_buf());
    std::thread::Builder::new()
        .name("luna-handoff-read".into())
        .spawn(move || {
            let read = read_checked(&f, &d, &o);
            // Its place goes before its answer: a caller that has the answer
            // finds the place free for the next read.
            drop(place);
            let _ = tx.send(read);
        })
        .map_err(|e| format!("could not start reading the file: {e}"))?;
    rx.recv_timeout(wait)
        .unwrap_or_else(|_| Err(format!("file: {file} did not read within {} s; write the handoff on a local disk", wait.as_secs().max(1))))
}

/// Checks `file` and copies it to `<dir>/<n>.md`, made private and new (a file
/// already there under that number is never written into), flushed to disk,
/// the folder too where it can be.
pub(super) fn snapshot(chat: &str, file: &str, data_dir: &Path, dir: &Path, n: u32) -> Result<Snapshot, String> {
    snapshot_within(readers(), chat, file, data_dir, dir, n, READ_WAIT)
}

pub(super) fn snapshot_within(
    readers: &'static Readers,
    chat: &str,
    file: &str,
    data_dir: &Path,
    dir: &Path,
    n: u32,
    wait: std::time::Duration,
) -> Result<Snapshot, String> {
    let (source, bytes) = read_within(readers, chat, file, data_dir, dir, wait)?;
    std::fs::create_dir_all(dir).map_err(|e| format!("could not make {}: {e}", dir.display()))?;
    let path = dir.join(format!("{n}.md"));
    let written = crate::secret::create_private(&path).and_then(|mut out| {
        std::io::Write::write_all(&mut out, &bytes)?;
        out.sync_all()
    });
    if let Err(e) = written.and_then(|()| crate::secret::sync_dir(dir)) {
        let _ = std::fs::remove_file(&path);
        return Err(format!("could not write {}: {e}", path.display()));
    }
    let sha256 = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    // UTF-8 already, as `read_checked` took it.
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Ok(Snapshot { path, sha256, source: source.to_string_lossy().into_owned(), size: bytes.len(), text })
}

/// The first handoff number past every copy in `dir` (`<n>.md`, `<n>-facts.md`).
pub(super) fn next_free(dir: &Path) -> u32 {
    let Ok(entries) = std::fs::read_dir(dir) else { return 1 };
    let taken = entries.flatten().filter_map(|e| {
        let name = e.file_name().to_string_lossy().into_owned();
        let stem = name.strip_suffix(".md")?;
        stem.strip_suffix("-facts").unwrap_or(stem).parse::<u32>().ok()
    });
    taken.max().map_or(1, |n| n.saturating_add(1))
}

#[cfg(test)]
pub(super) mod tests {
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
        let s = snapshot("file-ok", &file.to_string_lossy(), &data, &out, 1).unwrap();
        assert_eq!(s.path, out.join("1.md"));
        assert_eq!(std::fs::read_to_string(&s.path).unwrap(), "# Objective\nShip it.\n");
        let expected: String = Sha256::digest(b"# Objective\nShip it.\n").iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(s.sha256, expected);
        // A later change to the file is not the snapshot's.
        std::fs::write(&file, "changed").unwrap();
        assert_eq!(std::fs::read_to_string(&s.path).unwrap(), "# Objective\nShip it.\n");
        // A number taken is never written over.
        assert!(snapshot("file-ok", &file.to_string_lossy(), &data, &out, 1).unwrap_err().contains("could not write"));
        assert!(snapshot("file-ok", &file.to_string_lossy(), &data, &out, 2).is_ok());
        std::fs::write(out.join("7-facts.md"), "facts").unwrap();
        assert_eq!(next_free(&out), 8, "past every copy, facts included");
        assert_eq!(next_free(&dir.join("none")), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_is_not_a_handoff_file_is_refused_and_nothing_copied() {
        let dir = scratch("bad");
        let data = dir.join("data");
        std::fs::create_dir_all(data.join("core")).unwrap();
        let out = dir.join("out");
        let refused = |file: &Path, why: &str| {
            let e = snapshot("file-bad", &file.to_string_lossy(), &data, &out, 1).unwrap_err();
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
        assert!(snapshot("file-bad", &write("max.md", &vec![b'a'; MAX_BYTES as usize]).to_string_lossy(), &data, &out, 9).is_ok());
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

    #[test]
    fn of_luna_s_folder_only_the_calling_chat_s_drafts_are_taken() {
        let dir = scratch("drafts");
        let data = dir.join("data");
        let handoffs = data.join("handoffs");
        let (own, other) = (handoffs.join("c1"), handoffs.join("c2"));
        for d in [&own, &other] {
            std::fs::create_dir_all(d.join(super::super::DRAFTS).join("deep")).unwrap();
        }
        let write = |p: PathBuf| {
            std::fs::write(&p, "# Goal\nShip it.\n").unwrap();
            p
        };
        let take = |file: &Path, own: &Path, n| snapshot("file-drafts", &file.to_string_lossy(), &data, own, n);
        let mine = write(own.join("drafts").join("handoff.md"));
        assert_eq!(std::fs::read_to_string(take(&mine, &own, 1).unwrap().path).unwrap(), "# Goal\nShip it.\n");
        // One another user left in it, where this test may hand one over: not
        // the chat's own making.
        #[cfg(unix)]
        if unsafe { libc::geteuid() } == 0 {
            let left = write(own.join("drafts").join("left.md"));
            std::os::unix::fs::chown(&left, Some(4242), Some(4242)).unwrap();
            let e = take(&left, &own, 9).unwrap_err();
            assert!(e.contains("not this user's own"), "{e}");
        }
        // Another chat's drafts, its own folder of copies, a folder below its
        // drafts: Luna's folder, refused.
        for (file, n) in [
            (write(other.join("drafts").join("handoff.md")), 2),
            (write(own.join("handoff.md")), 3),
            (write(own.join("drafts").join("deep").join("handoff.md")), 4),
        ] {
            let e = take(&file, &own, n).unwrap_err();
            assert!(e.contains("Luna's own folder"), "{}: {e}", file.display());
        }
        // A chat whose drafts folder is a link (a junction on Windows) to
        // another's, or out of Luna's folder to one someone else holds: no
        // drafts of its own, though the path is named inside Luna's folder.
        let outside = dir.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        write(outside.join("handoff.md"));
        for (chat, target, n) in [("c3", other.join("drafts"), 5), ("c4", outside, 6)] {
            let linked = handoffs.join(chat);
            std::fs::create_dir_all(&linked).unwrap();
            assert!(dir_link(&target, &linked.join("drafts")), "a link to {}", target.display());
            let e = take(&linked.join("drafts").join("handoff.md"), &linked, n).unwrap_err();
            assert!(e.contains("Luna's own folder"), "{chat}: {e}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A folder link at `link`: a junction on Windows, which needs no
    /// privilege, a symlink elsewhere.
    pub(in crate::agents::handoff) fn dir_link(target: &Path, link: &Path) -> bool {
        #[cfg(windows)]
        let made = std::process::Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .is_ok_and(|o| o.status.success());
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(target, link).is_ok();
        made
    }

    #[test]
    fn a_read_that_does_not_answer_in_time_is_refused() {
        let dir = scratch("slow");
        let file = dir.join("handoff.md");
        std::fs::write(&file, "# Objective").unwrap();
        let name = file.to_string_lossy().into_owned();
        on_read(&name, || std::thread::sleep(std::time::Duration::from_millis(1500)));
        let started = std::time::Instant::now();
        let e =
            snapshot_within(readers(), "file-slow", &name, &dir.join("data"), &dir.join("out"), 1, std::time::Duration::from_millis(200))
                .unwrap_err();
        assert!(e.contains("did not read within"), "{e}");
        assert!(started.elapsed() < std::time::Duration::from_millis(1200), "not held up by the read");
        assert!(!dir.join("out").join("1.md").exists());
        std::thread::sleep(std::time::Duration::from_millis(1500));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A FIFO swapped in at a checked name: the open itself returns at once,
    /// and the handle says it is no file.
    #[cfg(unix)]
    #[test]
    fn a_fifo_at_the_name_does_not_block_the_open() {
        let dir = scratch("fifo");
        let fifo = dir.join("pipe.md");
        let c = std::ffi::CString::new(fifo.to_string_lossy().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let (tx, rx) = std::sync::mpsc::channel();
        let at = fifo.clone();
        std::thread::spawn(move || {
            let opened = open_no_follow(&at).map(|f| f.metadata().map(|m| m.is_file()));
            let _ = tx.send(opened.is_ok_and(|m| m.is_ok_and(|file| !file)));
        });
        let refused = rx.recv_timeout(std::time::Duration::from_secs(2)).expect("open returned without a writer");
        assert!(refused, "opened, and its handle says it is no file");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn readers_that_never_come_back_hold_their_place_and_no_more_are_started() {
        // Readers of their own, so the cap reached here is no other test's.
        let readers: &'static Readers = Box::leak(Box::new(Readers::new(2)));
        let dir = scratch("stuck");
        // Luna's folder, as the reads are told, apart from the files they read.
        let data = dir.join("data");
        let file = |name: &str| {
            let p = dir.join(format!("{name}.md"));
            std::fs::write(&p, "# Objective").unwrap();
            p.to_string_lossy().into_owned()
        };
        // A share that never answers, for as long as the test holds it so.
        let (release, stuck) = std::sync::mpsc::channel::<()>();
        let stuck = std::sync::Arc::new(std::sync::Mutex::new(stuck));
        let hang = |name: &str| {
            let path = file(name);
            let stuck = stuck.clone();
            on_read(&path, move || {
                let _ = stuck.lock().unwrap().recv();
            });
            path
        };
        let short = std::time::Duration::from_millis(100);
        let a = hang("a");
        assert!(read_within(readers, "chat-a", &a, &data, &dir, short).unwrap_err().contains("did not read within"));
        assert_eq!(readers.alive(), 1, "given up on, its reader still holds its place");
        // That chat gets no second reader while the first is in the filesystem.
        let e = read_within(readers, "chat-a", &file("a2"), &data, &dir, short).unwrap_err();
        assert!(e.contains("has not come back yet"), "{e}");
        assert_eq!(readers.alive(), 1, "refused before any thread");
        // Another chat's: one more, then the cap.
        let b = hang("b");
        assert!(read_within(readers, "chat-b", &b, &data, &dir, short).is_err());
        let e = read_within(readers, "chat-c", &file("c"), &data, &dir, short).unwrap_err();
        assert!(e.contains("being read and have not come back"), "{e}");
        assert_eq!(readers.alive(), 2);
        // The share answers: the readers end, and their places are free again.
        drop(release);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while readers.alive() > 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(readers.alive(), 0);
        assert!(read_within(readers, "chat-a", &file("a3"), &data, &dir, short).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
