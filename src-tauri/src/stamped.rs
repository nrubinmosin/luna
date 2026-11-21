//! What is read off a file, kept for as long as the file stays as it was: the
//! same length, change time and identity. The core reads the ends of every
//! live chat's transcript or rollout every few seconds (`sessions_meta`, the
//! records' reads), and at idle none of them has changed since the last
//! look: one `stat` says so, where a read of the tail cost up to 256 KB.
//!
//! A file's change time is only as fine as the file system keeps it (a
//! kernel tick on Linux, two seconds on FAT), and a file rewritten in place
//! (Claude Code's registry entry: `busy` and `idle` are the same length) can
//! change twice within one. So, as git does with its index, what was read of
//! a file changed in the last `RACY` is not kept: it is read again until the
//! file has stood still that long. A file being written to is read every
//! time, as before; one at rest is read once a `REVALIDATE`, which also
//! catches a file put back by a tool that keeps its length and change time.
//! A read that failed keeps nothing: the next look reads again.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

/// What a file is known by: its length and last change as the OS has them,
/// and which file it is: its inode off Windows, its creation time on Windows
/// (where std reads no file index without opening the file). On Windows the
/// creation time does not always tell a file put in another's place: NTFS
/// tunneling gives a file created or renamed into a name within 15 s of the
/// last one leaving it that one's creation time, so a file rewritten by a
/// rename onto its name can keep its stamp. Its length and change time
/// mostly differ anyway, and what was read is read again a minute on
/// whatever the stamp (`ByStamp`), which bounds how long such a file is
/// taken for the one before.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
    identity: Identity,
}

#[cfg(unix)]
type Identity = (u64, u64);
#[cfg(not(unix))]
type Identity = Option<SystemTime>;

/// The stamp of the file at `path`, None where it cannot be read.
pub fn stamp(path: &Path) -> Option<Stamp> {
    let m = std::fs::metadata(path).ok()?;
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        (m.dev(), m.ino())
    };
    #[cfg(not(unix))]
    let identity = m.created().ok();
    Some(Stamp { len: m.len(), modified: m.modified().ok(), identity })
}

/// How long a file must have stood unchanged for what was read of it to be
/// kept.
const RACY: Duration = Duration::from_secs(2);
/// How long what was read of a file at rest is trusted before it is read
/// again anyway: once a minute, against the fraction of that a tick read.
const REVALIDATE: Duration = Duration::from_secs(60);
/// An entry nobody has asked for in this long is let go: the file of a chat
/// that was deleted, or of a conversation it has moved on from.
const UNUSED: Duration = Duration::from_secs(10 * 60);
/// How often a cache looks for such entries, as it is asked.
const SWEEP_EVERY: Duration = Duration::from_secs(60);

struct Kept<V> {
    stamp: Stamp,
    value: V,
    /// When it was read, and when last asked for.
    read: Instant,
    used: Instant,
}

struct Map<V> {
    entries: HashMap<PathBuf, Kept<V>>,
    swept: Instant,
}

/// One kind of result per file, kept by the file's stamp. Bounded twice:
/// entries unused for `UNUSED` go, and past `cap` the one used longest ago
/// makes room.
pub struct ByStamp<V> {
    cap: usize,
    map: Mutex<Map<V>>,
}

impl<V: Clone> ByStamp<V> {
    pub fn new(cap: usize) -> Self {
        Self { cap: cap.max(1), map: Mutex::new(Map { entries: HashMap::new(), swept: Instant::now() }) }
    }

    /// What `read` makes of the file at `path`, read again once its stamp
    /// has changed or `REVALIDATE` has gone by. `read` answers None where the
    /// read failed: nothing is kept then, and the caller gets None. The stamp
    /// is taken before the read: a file written to during it is read again
    /// next time, never kept as newer than it is. A file that is not there,
    /// or changed within `RACY`, is read every time, and nothing kept of it.
    pub fn get(&self, path: &Path, read: impl FnOnce(&Path) -> Option<V>) -> Option<V> {
        let at = Instant::now();
        let now = stamp(path);
        let settled = now.and_then(|s| s.modified).and_then(|m| SystemTime::now().duration_since(m).ok()).is_some_and(|age| age >= RACY);
        {
            let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
            // Every look sweeps, a hit or not: a cache only ever hit would
            // otherwise keep what no one asks for.
            if at.duration_since(map.swept) >= SWEEP_EVERY {
                map.entries.retain(|_, k| at.duration_since(k.used) < UNUSED);
                map.swept = at;
            }
            if !settled {
                map.entries.remove(path);
            } else if let Some(k) = map.entries.get_mut(path).filter(|k| Some(k.stamp) == now && at.duration_since(k.read) < REVALIDATE) {
                k.used = at;
                return Some(k.value.clone());
            }
        }
        // Read with no lock held: other files' callers do not wait on it.
        let value = read(path)?;
        let (Some(now), true) = (now, settled) else { return Some(value) };
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        if map.entries.len() >= self.cap && !map.entries.contains_key(path) {
            if let Some(oldest) = map.entries.iter().min_by_key(|(_, k)| k.used).map(|(p, _)| p.clone()) {
                map.entries.remove(&oldest);
            }
        }
        map.entries.insert(path.to_path_buf(), Kept { stamp: now, value: value.clone(), read: at, used: at });
        Some(value)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.map.lock().unwrap().entries.len()
    }

    /// The entry of `path` as if read and last asked for `ago` before now,
    /// and the last sweep as long ago, for the tests of both.
    #[cfg(test)]
    fn age_for_test(&self, path: &Path, ago: Duration) {
        let mut map = self.map.lock().unwrap();
        let then = Instant::now().checked_sub(ago).unwrap();
        if let Some(k) = map.entries.get_mut(path) {
            (k.read, k.used) = (then, then);
        }
        map.swept = map.swept.min(Instant::now().checked_sub(SWEEP_EVERY).unwrap());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::io::Write;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("luna-stamped-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// The file's change time set `ago` back: a file at rest.
    fn changed_ago(path: &Path, ago: Duration) {
        let f = std::fs::OpenOptions::new().append(true).open(path).unwrap();
        f.set_modified(SystemTime::now() - ago).unwrap();
    }

    #[test]
    fn an_unchanged_file_is_not_read_again_and_a_changed_one_is() {
        let d = dir("reads");
        let path = d.join("t.jsonl");
        std::fs::write(&path, "one\n").unwrap();
        changed_ago(&path, Duration::from_secs(3600));
        let cache: ByStamp<String> = ByStamp::new(8);
        let reads = Cell::new(0);
        let get = || {
            cache.get(&path, |p| {
                reads.set(reads.get() + 1);
                std::fs::read_to_string(p).ok()
            })
        };
        assert_eq!(get().as_deref(), Some("one\n"));
        assert_eq!(get().as_deref(), Some("one\n"));
        assert_eq!(reads.get(), 1, "unchanged: one read");

        // Longer, at rest again: read again, then kept.
        std::fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(b"two\n").unwrap();
        changed_ago(&path, Duration::from_secs(3000));
        assert_eq!(get().as_deref(), Some("one\ntwo\n"));
        assert_eq!(get().as_deref(), Some("one\ntwo\n"));
        assert_eq!(reads.get(), 2);

        // The same length, written later: read again by its change time.
        std::fs::OpenOptions::new().write(true).open(&path).unwrap().write_all(b"ONE\nTWO\n").unwrap();
        changed_ago(&path, Duration::from_secs(2400));
        assert_eq!(get().as_deref(), Some("ONE\nTWO\n"));
        assert_eq!(get().as_deref(), Some("ONE\nTWO\n"));
        assert_eq!(reads.get(), 3);

        // Changed just now, the same length again, as a registry entry going
        // from busy to idle within one tick of the clock: read every time
        // until it has stood still for `RACY`.
        std::fs::OpenOptions::new().write(true).open(&path).unwrap().write_all(b"one\ntwo\n").unwrap();
        assert_eq!(get().as_deref(), Some("one\ntwo\n"));
        std::fs::OpenOptions::new().write(true).open(&path).unwrap().write_all(b"ONE\nTWO\n").unwrap();
        assert_eq!(get().as_deref(), Some("ONE\nTWO\n"));
        assert_eq!(reads.get(), 5, "nothing kept of a file still moving");

        // Gone: read each time, nothing kept.
        std::fs::remove_file(&path).unwrap();
        assert_eq!(get(), None);
        assert_eq!(get(), None);
        assert_eq!(reads.get(), 7);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A read that fails (a scanner holding the file, say) is no content: the
    /// file, unchanged, is read again next time, and what it says then kept.
    #[test]
    fn a_failed_read_keeps_nothing_and_the_next_look_reads_again() {
        let d = dir("failed");
        let path = d.join("t.jsonl");
        std::fs::write(&path, "kept\n").unwrap();
        changed_ago(&path, Duration::from_secs(3600));
        let cache: ByStamp<String> = ByStamp::new(8);
        assert_eq!(cache.get(&path, |_| None), None, "the read failed");
        assert_eq!(cache.len(), 0);
        let reads = Cell::new(0);
        let get = || {
            cache.get(&path, |p| {
                reads.set(reads.get() + 1);
                std::fs::read_to_string(p).ok()
            })
        };
        assert_eq!(get().as_deref(), Some("kept\n"), "the same stamp, read again");
        assert_eq!(get().as_deref(), Some("kept\n"));
        assert_eq!(reads.get(), 1, "and kept once read");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A file put back by a tool that keeps its length and change time is
    /// read again within `REVALIDATE`, however unchanged it looks.
    #[test]
    fn a_file_at_rest_is_read_again_once_a_while() {
        let d = dir("revalidate");
        let path = d.join("t.jsonl");
        let at = SystemTime::now() - Duration::from_secs(3600);
        std::fs::write(&path, "old!\n").unwrap();
        std::fs::OpenOptions::new().append(true).open(&path).unwrap().set_modified(at).unwrap();
        let cache: ByStamp<String> = ByStamp::new(8);
        let get = || cache.get(&path, |p| std::fs::read_to_string(p).ok());
        assert_eq!(get().as_deref(), Some("old!\n"));
        // Put back in place, length and change time as they were.
        std::fs::OpenOptions::new().write(true).open(&path).unwrap().write_all(b"new!\n").unwrap();
        std::fs::OpenOptions::new().append(true).open(&path).unwrap().set_modified(at).unwrap();
        assert_eq!(get().as_deref(), Some("old!\n"), "it looks unchanged");
        cache.age_for_test(&path, REVALIDATE);
        assert_eq!(get().as_deref(), Some("new!\n"), "read again after a while");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn entries_of_files_no_longer_asked_for_are_let_go() {
        let d = dir("evict");
        let paths: Vec<PathBuf> = (0..4).map(|i| d.join(format!("{i}.jsonl"))).collect();
        paths.iter().for_each(|p| {
            std::fs::write(p, "x").unwrap();
            changed_ago(p, Duration::from_secs(3600));
        });
        let cache: ByStamp<u32> = ByStamp::new(3);
        for (i, p) in paths.iter().take(3).enumerate() {
            cache.get(p, |_| Some(i as u32));
        }
        assert_eq!(cache.len(), 3);
        // Full: the one used longest ago makes room.
        cache.get(&paths[1], |_| unreachable!("kept"));
        cache.get(&paths[2], |_| unreachable!("kept"));
        cache.get(&paths[3], |_| Some(3));
        assert_eq!(cache.len(), 3);
        let reads = Cell::new(0);
        cache.get(&paths[0], |_| {
            reads.set(1);
            Some(0)
        });
        assert_eq!(reads.get(), 1, "the oldest went to make room");

        // Unused past UNUSED: gone at the next sweep, even one made by a look
        // that only hits another entry.
        cache.age_for_test(&paths[0], UNUSED + Duration::from_secs(1));
        cache.age_for_test(&paths[3], UNUSED + Duration::from_secs(1));
        cache.get(&paths[2], |_| unreachable!("a hit"));
        assert_eq!(cache.len(), 1, "only the file still asked for");
        let _ = std::fs::remove_dir_all(&d);
    }
}
