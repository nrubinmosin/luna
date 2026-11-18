//! Which worktrees a session is on its way into, and which one a delete is
//! removing, behind one lock: a delete asks whether anyone else is in the
//! worktree (`PtyManager::worktree_shared`), and a start that had not yet
//! put its session where that question looks (the session map, the helper
//! registry) went into the worktree as it was removed. Now a start holds its
//! way in (`entering`) from before it starts until it is listed, and whatever
//! removes a worktree (a chat's delete, a helper's, the sweep of orphans)
//! holds it (`removing`) from its question until the removal is over, and
//! neither is let in while the other holds: the delete keeps a
//! worktree a start is on its way into, and a start into a worktree being
//! removed fails, saying so.

use super::{claims, norm_in_use};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Default)]
struct Ways {
    /// Each start on its way in, by a number of its own: the folders it goes
    /// into.
    entering: Vec<(u64, Vec<Place>)>,
    removing: Vec<(String, Place)>,
    next: u64,
}

static WAYS: Mutex<Ways> = Mutex::new(Ways { entering: Vec::new(), removing: Vec::new(), next: 0 });

fn ways() -> std::sync::MutexGuard<'static, Ways> {
    WAYS.lock().unwrap_or_else(|e| e.into_inner())
}

/// A path as the holds compare it, where a miss is the mistake (a worktree
/// removed with a session in it or on its way in): its spelling by
/// `path_key_loose`, and where it leads, resolved through links and
/// junctions (`resolved`). A start through a linked project folder and a
/// delete naming the real one are of one worktree.
pub(crate) struct Place {
    key: String,
    real: Real,
}

/// Where a path leads, as far as the disk says.
enum Real {
    /// Resolved, in `path_key_loose`'s spelling.
    At(String),
    /// Nothing of it is there, up to its root: only its spelling says.
    Nowhere,
    /// A folder on its way could not be read (access denied, a link that
    /// loops): it may lead anywhere, and is taken for every place.
    Unknown,
}

impl Place {
    /// Reads the disk: taken before the holds' lock where it can be.
    pub(crate) fn of(path: &str) -> Place {
        fn canonicalize(path: &Path) -> std::io::Result<PathBuf> {
            std::fs::canonicalize(path)
        }
        Place::by(path, canonicalize)
    }

    fn by(path: &str, canonicalize: impl Fn(&Path) -> std::io::Result<PathBuf>) -> Place {
        Place { key: norm_in_use(path), real: resolved(Path::new(path), canonicalize) }
    }

    /// Whether this is the worktree `wt` or inside it, by either spelling or
    /// by where both lead; so it is where either cannot be told.
    pub(crate) fn within(&self, wt: &Place) -> bool {
        claims(&self.key, &wt.key) || self.real.meets(&wt.real, claims)
    }

    /// Whether this and `other` are one place, by either spelling or by
    /// where both lead; so they are where either cannot be told.
    pub(crate) fn is(&self, other: &Place) -> bool {
        self.key == other.key || self.real.meets(&other.real, |a, b| a == b)
    }
}

impl Real {
    fn meets(&self, other: &Real, by: impl Fn(&str, &str) -> bool) -> bool {
        match (self, other) {
            (Real::At(a), Real::At(b)) => by(a, b),
            (Real::Unknown, _) | (_, Real::Unknown) => true,
            _ => false,
        }
    }
}

/// Where `path` leads: the nearest of it and the folders above it that is
/// there, resolved, and the rest of it on top. A worktree a CLI has not made
/// yet resolves as it will once it is there; only a folder that is not there
/// counts as not made yet, and one that does not read leaves where the path
/// leads unknown: taken for one not there, a folder whose traversal was
/// revoked since would put the path a session was started on through a
/// link beside the link itself, and a removal by the real path would miss
/// it.
fn resolved(path: &Path, canonicalize: impl Fn(&Path) -> std::io::Result<PathBuf>) -> Real {
    let mut rest = Vec::new();
    let mut at = path;
    loop {
        match canonicalize(at) {
            Ok(real) => return Real::At(norm_in_use(&rest.iter().rev().fold(real, |r, part| r.join(part)).to_string_lossy())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Real::Unknown,
        }
        let (Some(name), Some(parent)) = (at.file_name(), at.parent()) else { return Real::Nowhere };
        rest.push(name);
        at = parent;
    }
}

/// A start on its way into folders, until it is dropped.
pub(crate) struct Entering(u64);

impl Drop for Entering {
    fn drop(&mut self) {
        ways().entering.retain(|(n, _)| *n != self.0);
    }
}

/// Holds the way into `paths` for a start, from before it starts until it is
/// listed where a delete looks (the guard dropped then); refused, saying
/// why, where one of them is in a worktree being removed.
pub(crate) fn entering(paths: &[&str]) -> Result<Entering, String> {
    let places: Vec<Place> = paths.iter().map(|p| Place::of(p)).collect();
    let mut w = ways();
    if let Some((wt, _)) = w.removing.iter().find(|(_, wt)| places.iter().any(|p| p.within(wt))) {
        return Err(format!("{wt} is being removed with the chat that was in it; start again once it is gone"));
    }
    w.next += 1;
    let n = w.next;
    w.entering.push((n, places));
    Ok(Entering(n))
}

/// A worktree held for its removal, until it is dropped.
pub(crate) struct Removing(String);

impl Drop for Removing {
    fn drop(&mut self) {
        let mut w = ways();
        if let Some(i) = w.removing.iter().position(|(wt, _)| *wt == self.0) {
            w.removing.remove(i);
        }
    }
}

/// Holds `wt` for its removal, unless a start is on its way into it or
/// `others_in_it` (asked under the same lock) finds someone there: then None,
/// and the worktree stays.
pub(crate) fn removing(wt: &str, others_in_it: impl FnOnce() -> bool) -> Option<Removing> {
    let place = Place::of(wt);
    let mut w = ways();
    if w.entering.iter().any(|(_, paths)| paths.iter().any(|p| p.within(&place))) || others_in_it() {
        return None;
    }
    w.removing.push((wt.to_string(), place));
    Some(Removing(wt.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn within(path: &str, wt: &str) -> bool {
        Place::of(path).within(&Place::of(wt))
    }

    /// A start into a worktree held for removal is refused, saying so, and
    /// let in once the removal is over; a delete keeps a worktree a start is
    /// on its way into, or that someone is in.
    #[test]
    fn a_start_and_a_removal_of_one_worktree_hold_each_other_off() {
        let wt = if cfg!(windows) { r"C:\p\.claude\worktrees\entry-a" } else { "/p/.claude/worktrees/entry-a" };
        let inside = format!("{wt}/src");
        let held = removing(wt, || false).expect("nobody there");
        let refused = entering(&[&inside]).err().expect("refused while it is removed");
        assert!(refused.contains("being removed"), "{refused}");
        assert!(entering(&[wt.trim_end_matches("entry-a")]).is_ok(), "the folder it is in is not it");
        drop(held);
        let on_the_way = entering(&[&inside]).expect("let in once it is gone");
        assert!(removing(wt, || false).is_none(), "kept for the start on its way in");
        drop(on_the_way);
        assert!(removing(wt, || true).is_none(), "kept for whoever is in it");
        assert!(removing(wt, || false).is_some());
    }

    /// A path through a folder that does not read (its traversal revoked
    /// since a session was started on it through a link) may lead to any
    /// worktree, and is held as in it: taken for one not made yet, it would
    /// resolve beside the link, and a removal by the real path would go.
    /// One that is only not there yet is compared as before.
    #[test]
    fn a_path_that_does_not_read_is_taken_for_in_the_worktree() {
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let root = PathBuf::from(if cfg!(windows) { r"C:\" } else { "/" });
        let wt = s(&root.join("real").join(".claude").join("worktrees").join("w"));
        let locked = root.join("locked");
        let session = s(&locked.join("link").join(".claude").join("worktrees").join("w").join("src"));
        let disk = |denied: bool| {
            let (root, locked) = (root.clone(), locked.clone());
            move |p: &Path| match p {
                p if p == root => Ok(root.clone()),
                p if denied && p.starts_with(&locked) => Err(std::io::ErrorKind::PermissionDenied.into()),
                _ => Err(std::io::ErrorKind::NotFound.into()),
            }
        };
        let wt_place = Place::by(&wt, disk(true));
        assert!(Place::by(&session, disk(true)).within(&wt_place), "where it leads cannot be told");
        assert!(wt_place.within(&Place::by(&session, disk(true))), "nor the other way about");
        assert!(Place::by(&session, disk(true)).is(&Place::by(&wt, disk(true))));
        assert!(!Place::by(&session, disk(false)).within(&Place::by(&wt, disk(false))), "not there yet: by where it would be");
    }

    /// One worktree, however it is reached: through a link or a junction to
    /// the project, there yet or not, and in another case or with the other
    /// separator where the platform takes those for one place. On Linux
    /// another case is another folder, and a backslash part of a name.
    #[test]
    fn a_worktree_is_held_however_it_is_reached() {
        let root = std::env::temp_dir().join(format!("luna-entry-place-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let worktrees = |project: &str| root.join(project).join(".claude").join("worktrees");
        let wt = worktrees("real").join("w");
        std::fs::create_dir_all(wt.join("src")).unwrap();
        super::super::link_dir(&root.join("link"), &root.join("real"));
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let (wt, linked) = (s(&wt), worktrees("link").join("w"));
        assert!(within(&s(&linked.join("src")), &wt), "in it through the link");
        assert!(within(&wt, &s(&linked)), "and the other way about");
        assert!(within(&s(&worktrees("link").join("later")), &s(&worktrees("real").join("later"))), "one not made yet");
        assert!(!within(&s(&worktrees("link").join("other")), &wt));
        assert_eq!(within(&wt.to_uppercase(), &wt), cfg!(any(windows, target_os = "macos")), "another case");
        assert_eq!(within(&format!("{wt}\\src"), &wt), cfg!(windows), "a backslash");

        let on_the_way = entering(&[&s(&linked.join("src"))]).unwrap();
        assert!(removing(&wt, || false).is_none(), "kept for a start on its way in through the link");
        drop(on_the_way);
        let held = removing(&s(&linked), || false).expect("nobody there");
        assert!(entering(&[&wt]).is_err(), "a start by the real path is refused while it goes by the link");
        drop(held);
        let _ = std::fs::remove_dir_all(&root);
    }
}
