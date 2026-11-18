//! Who holds a worktree against its removal, and a removal's hold on it,
//! behind one lock. A session holds the places it is in (`Hold`), resolved
//! once, as it starts: where its folder led then is where it is, whatever
//! becomes of the spelling it was started by (a link unlinked, a folder that
//! no longer reads). A start holds its way in (`entering`) from before it
//! starts until it is listed where a removal looks for whoever is in a
//! worktree (the session map, the helper registry), which then carries the
//! same hold; whatever removes a worktree (a chat's delete, a helper's, the
//! sweep of orphans) holds it (`removing`) from that look until the removal is
//! over. Neither is let in while the other holds: the removal keeps a
//! worktree a start is on its way into, and a start into a worktree being
//! removed fails, saying so.

use super::{claims, claude_worktrees, norm_in_use, Unplaced, KEY_SEP};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// What a look at a worktree is for, which decides what may keep it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Look {
    /// A chat's or a helper's own worktree, removed with it as asked: kept
    /// for a session in it, by its spelling or by where both lead. A path
    /// whose way does not read is compared by its spelling alone, as an
    /// unrelated one would keep every worktree from every delete. A session
    /// whose place did not read as it started keeps it so (`Real::Unknown`),
    /// and once listed is compared by its spelling alone too.
    Delete,
    /// The sweep of orphans, which only offers what the person confirms:
    /// kept for whatever may be in it, a path whose way does not read among
    /// them, and for a session making a worktree whose path its CLI picks,
    /// every worktree of its folder where that CLI makes them.
    Sweep,
}

/// A path as the holds compare it, where a miss is the mistake (a worktree
/// removed with a session in it or on its way in): its spelling by
/// `path_key_loose`, and where it leads, resolved through links and
/// junctions (`resolved`). A start through a linked project folder and a
/// delete naming the real one are of one worktree.
#[derive(Clone, Debug)]
pub(crate) struct Place {
    key: String,
    real: Real,
}

/// Where a path leads, as far as the disk said when it was resolved.
#[derive(Clone, Debug)]
enum Real {
    /// Resolved, in `path_key_loose`'s spelling.
    At(String),
    /// Nothing of it is there, up to its root: only its spelling says.
    Nowhere,
    /// A folder on its way could not be read (access denied, a link that
    /// loops): it may lead anywhere (`Look`).
    Unknown,
}

impl Place {
    /// Reads the disk: taken before the holds' lock.
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
    /// by where both lead.
    fn within(&self, wt: &Place, look: Look) -> bool {
        claims(&self.key, &wt.key) || self.real.meets(&wt.real, look, claims)
    }

    /// Whether this and `other` are one place, by either spelling or by
    /// where both lead.
    fn is(&self, other: &Place, look: Look) -> bool {
        self.key == other.key || self.real.meets(&other.real, look, |a, b| a == b)
    }

    /// Whether this is `depth` folders below `root`, by either spelling or
    /// by where both lead.
    fn below(&self, root: &Place, depth: usize, look: Look) -> bool {
        below(&self.key, &root.key, depth) || self.real.meets(&root.real, look, |a, b| below(a, b, depth))
    }
}

impl Real {
    fn meets(&self, other: &Real, look: Look, by: impl Fn(&str, &str) -> bool) -> bool {
        match (self, other) {
            (Real::At(a), Real::At(b)) => by(a, b),
            (Real::Unknown, _) | (_, Real::Unknown) => look == Look::Sweep,
            _ => false,
        }
    }
}

/// Whether `path` is `depth` folders below `root`, both in `path_key_loose`'s
/// spelling.
fn below(path: &str, root: &str, depth: usize) -> bool {
    path.strip_prefix(root)
        .and_then(|rest| rest.strip_prefix(KEY_SEP))
        .is_some_and(|rest| rest.split(KEY_SEP).count() == depth && rest.split(KEY_SEP).all(|part| !part.is_empty()))
}

/// Where `path` leads: the nearest of it and the folders above it that is
/// there, resolved, and the rest of it on top. A worktree a CLI has not made
/// yet resolves as it will once it is there; only a folder that is not there
/// counts as not made yet, and one that does not read leaves where the path
/// leads unknown.
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

/// A session making a worktree whose path its CLI picks (`Unplaced`), as the
/// sweep takes it: any worktree of its folder where its CLI makes them may
/// be its own.
#[derive(Clone, Debug)]
struct Maker {
    folder: Place,
    /// Where its CLI makes them, and how far below: Claude Code's
    /// `<folder>/.claude/worktrees/<name>`, Codex's
    /// `<account>/worktrees/<bucket>/<repo>`.
    root: Place,
    depth: usize,
}

impl Maker {
    fn of(u: &Unplaced) -> Maker {
        let (root, depth) = match u.provider {
            crate::provider::Provider::Claude => (claude_worktrees(&u.folder), 1),
            crate::provider::Provider::Codex => (Path::new(&u.account_path).join("worktrees"), 2),
        };
        Maker { folder: Place::of(&u.folder), root: Place::of(&root.to_string_lossy()), depth }
    }
}

/// A worktree a look is about: its path, where it leads, resolved once, and
/// the folder it is of where the look knows it (the sweep's).
pub(crate) struct Target {
    path: String,
    place: Place,
    folder: Option<Place>,
}

impl Target {
    /// Reads the disk: taken before the holds' lock.
    pub(crate) fn of(path: &str, folder: Option<Place>) -> Target {
        Target { path: path.to_string(), place: Place::of(path), folder }
    }
}

/// What keeps worktrees from removal: the places someone is in, and the
/// sessions making a worktree whose path their CLI picks (`Maker`), each
/// resolved once, as the session started or as a look read them.
#[derive(Clone, Debug, Default)]
pub(crate) struct Hold {
    places: Vec<Place>,
    makers: Vec<Maker>,
}

impl Hold {
    /// A start's, resolved now, as it starts: the folders it goes into and,
    /// where it asks its CLI for a new worktree whose path the CLI picks,
    /// where that CLI makes them.
    pub(crate) fn start(paths: &[&str], makes: Option<&Unplaced>) -> Hold {
        Hold { places: paths.iter().map(|p| Place::of(p)).collect(), makers: makes.map(Maker::of).into_iter().collect() }
    }

    /// Someone in `path`, as it leads now.
    pub(crate) fn add_path(&mut self, path: &str) {
        self.places.push(Place::of(path));
    }

    /// `other`'s places, and its makers where it is still `making` (has not
    /// said where its worktree is).
    pub(crate) fn add(&mut self, other: &Hold, making: bool) {
        self.places.extend(other.places.iter().cloned());
        if making {
            self.makers.extend(other.makers.iter().cloned());
        }
    }

    /// Someone in `path` who started as `start` holds: one under a place it
    /// was started in, by that place's spelling, is where that place led as
    /// it started with the rest of `path` on top (its CLI is there, whatever
    /// has become of the spelling since), and where `path` leads now as well:
    /// the rest may go through a link or a junction of its own (a
    /// `.claude/worktrees` junctioned elsewhere), which only the disk says.
    /// Any other `path` is where it leads now.
    pub(crate) fn add_at(&mut self, start: &Hold, path: &str) {
        let key = norm_in_use(path);
        for p in &start.places {
            if let (Some(rest), Real::At(real)) = (key.strip_prefix(p.key.as_str()), &p.real) {
                if rest.is_empty() || rest.starts_with(KEY_SEP) {
                    self.places.push(Place { real: Real::At(format!("{real}{rest}")), key });
                    break;
                }
            }
        }
        self.add_path(path);
    }

    /// Whether one of its places is the worktree `wt` or inside it.
    fn is_in(&self, wt: &Target, look: Look) -> bool {
        self.places.iter().any(|p| p.within(&wt.place, look))
    }

    /// Whether this keeps `wt` from a removal for `look`: someone is in it,
    /// or, to the sweep, a session making a worktree of its folder may have
    /// made it, as it is where that session's CLI makes them.
    pub(crate) fn keeps(&self, wt: &Target, look: Look) -> bool {
        self.is_in(wt, look)
            || look == Look::Sweep
                && self
                    .makers
                    .iter()
                    .any(|m| wt.folder.as_ref().is_none_or(|f| m.folder.is(f, look)) && wt.place.below(&m.root, m.depth, look))
    }

    /// Whether a start on its way in with this hold keeps `wt` from a
    /// removal for `look`: as a session would, and, whatever the look, where
    /// a place of either does not read, as the sweep compares. Not yet
    /// listed, it would be compared by its spelling alone and go in as the
    /// worktree goes; held off, it costs the removal no more than the
    /// worktree kept, said, for the sweep to offer later.
    fn keeps_on_its_way(&self, wt: &Target, look: Look) -> bool {
        self.keeps(wt, look) || self.is_in(wt, Look::Sweep)
    }
}

/// A removal looking for whoever is in its worktree, outside the holds' lock.
struct Looking {
    n: u64,
    wt: Arc<Target>,
    look: Look,
    /// A start that may be in the worktree let go of its way in meanwhile:
    /// listed by now where the look may already have been.
    missed: bool,
}

struct Ways {
    /// Each start on its way in, by a number of its own, and its hold.
    entering: Vec<(u64, Arc<Hold>)>,
    looking: Vec<Looking>,
    removing: Vec<(u64, Arc<Target>, Look)>,
    next: u64,
}

impl Ways {
    const NONE: Ways = Ways { entering: Vec::new(), looking: Vec::new(), removing: Vec::new(), next: 0 };
}

/// The process's ways in and out; a test that holds every worktree (a place
/// that does not read) keeps its own, so as not to hold off the others'.
static WAYS: Mutex<Ways> = Mutex::new(Ways::NONE);

fn lock(ways: &Mutex<Ways>) -> std::sync::MutexGuard<'_, Ways> {
    ways.lock().unwrap_or_else(|e| e.into_inner())
}

/// A start on its way in, until it is dropped: once it is listed where a
/// removal looks, or has failed.
pub(crate) struct Entering(u64, &'static Mutex<Ways>);

impl Drop for Entering {
    fn drop(&mut self) {
        let mut w = lock(self.1);
        let Some(i) = w.entering.iter().position(|(n, _)| *n == self.0) else { return };
        let (_, hold) = w.entering.swap_remove(i);
        for l in w.looking.iter_mut().filter(|l| hold.keeps_on_its_way(&l.wt, l.look)) {
            l.missed = true;
        }
    }
}

/// Holds the way in for a start, its `hold` resolved before it starts, until
/// it is listed where a removal looks (the guard dropped then); refused,
/// saying why, where one of its places is or may be in a worktree being
/// removed.
pub(crate) fn entering(hold: Arc<Hold>) -> Result<Entering, String> {
    entering_by(&WAYS, hold)
}

fn entering_by(ways: &'static Mutex<Ways>, hold: Arc<Hold>) -> Result<Entering, String> {
    let mut w = lock(ways);
    // As the sweep compares, whatever the removal is: a place of the start's
    // whose way did not read may be in any worktree, and is not let into one
    // going. That costs a start no more than a retry once the removal is
    // through; a session listed with such a place keeps no delete's worktree
    // (`Look::Delete`), or it would keep them all for as long as it runs.
    if let Some((_, wt, look)) = w.removing.iter().find(|(_, wt, _)| hold.is_in(wt, Look::Sweep)) {
        return Err(refusal(&hold, wt, *look));
    }
    w.next += 1;
    let n = w.next;
    w.entering.push((n, hold));
    Ok(Entering(n, ways))
}

/// Why a start holding `hold` is not let in while `wt` is removed for `look`:
/// it goes into it, or, where a path of either does not read, it cannot be
/// told not to.
fn refusal(hold: &Hold, wt: &Target, look: Look) -> String {
    let again = "start again once it is gone";
    if hold.is_in(wt, Look::Delete) {
        return match look {
            Look::Delete => format!("{} is being removed with the session deleted from it; {again}", wt.path),
            Look::Sweep => format!("{} is being removed as a worktree nobody is in; {again}", wt.path),
        };
    }
    if matches!(wt.place.real, Real::Unknown) {
        return format!(
            "{} is being removed, and a folder on its way does not read, so no start can be told to be outside it; {again}",
            wt.path
        );
    }
    format!("{} is being removed, and a folder on the way to where this start goes does not read, so it cannot be told to be outside it; {again}", wt.path)
}

/// What the starts on their way in hold, for a look that reads where the
/// sessions are listed after this: one not among these is listed by then.
pub(crate) fn entering_now() -> Hold {
    let mut held = Hold::default();
    for (_, hold) in &lock(&WAYS).entering {
        held.add(hold, true);
    }
    held
}

/// A worktree held for its removal, until it is dropped.
pub(crate) struct Removing(u64, &'static Mutex<Ways>);

impl Drop for Removing {
    fn drop(&mut self) {
        lock(self.1).removing.retain(|(n, _, _)| *n != self.0);
    }
}

/// Holds `wt` for its removal, unless `others_in_it` finds someone there or
/// a start that may be in it is on its way in, or let go of its way in while
/// that look went on (`Hold::keeps_on_its_way`: one with a place that does
/// not read may be, whatever the look): then None, and the worktree stays.
/// The look reads the disk (where a session works now), and a share or a
/// mapped drive slow to answer would hold every start behind the lock: it
/// is taken outside it.
/// A start listed meanwhile may have been listed after the look went by and
/// is no longer on its way in: its letting go marks the look
/// (`Looking::missed`), and only a start that may be in `wt` does.
pub(crate) fn removing(wt: Target, look: Look, others_in_it: impl FnOnce() -> bool) -> Option<Removing> {
    removing_by(&WAYS, wt, look, others_in_it)
}

fn removing_by(ways: &'static Mutex<Ways>, wt: Target, look: Look, others_in_it: impl FnOnce() -> bool) -> Option<Removing> {
    let wt = Arc::new(wt);
    let n = {
        let mut w = lock(ways);
        w.next += 1;
        let n = w.next;
        w.looking.push(Looking { n, wt: Arc::clone(&wt), look, missed: false });
        n
    };
    let in_it = others_in_it();
    let mut w = lock(ways);
    let missed = w.looking.iter().position(|l| l.n == n).is_none_or(|i| w.looking.swap_remove(i).missed);
    if in_it || missed || w.entering.iter().any(|(_, hold)| hold.keeps_on_its_way(&wt, look)) {
        return None;
    }
    w.removing.push((n, wt, look));
    Some(Removing(n, ways))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::Provider;

    fn within(path: &str, wt: &str) -> bool {
        Hold::start(&[path], None).keeps(&Target::of(wt, None), Look::Delete)
    }

    fn start(paths: &[&str]) -> Result<Entering, String> {
        entering(Arc::new(Hold::start(paths, None)))
    }

    fn delete(wt: &str, others_in_it: impl FnOnce() -> bool) -> Option<Removing> {
        removing(Target::of(wt, None), Look::Delete, others_in_it)
    }

    /// Ways in and out of the test's own, for one that holds every worktree.
    fn apart() -> &'static Mutex<Ways> {
        Box::leak(Box::new(Mutex::new(Ways::NONE)))
    }

    /// A start into a worktree held for removal is refused, saying so, and
    /// let in once the removal is over; a delete keeps a worktree a start is
    /// on its way into, or that someone is in.
    #[test]
    fn a_start_and_a_removal_of_one_worktree_hold_each_other_off() {
        let wt = if cfg!(windows) { r"C:\p\.claude\worktrees\entry-a" } else { "/p/.claude/worktrees/entry-a" };
        let inside = format!("{wt}/src");
        let held = delete(wt, || false).expect("nobody there");
        let refused = start(&[&inside]).err().expect("refused while it is removed");
        assert!(refused.contains("being removed"), "{refused}");
        assert!(start(&[wt.trim_end_matches("entry-a")]).is_ok(), "the folder it is in is not it");
        drop(held);
        let on_the_way = start(&[&inside]).expect("let in once it is gone");
        assert!(delete(wt, || false).is_none(), "kept for the start on its way in");
        drop(on_the_way);
        assert!(delete(wt, || true).is_none(), "kept for whoever is in it");
        assert!(delete(wt, || false).is_some());
    }

    /// Whoever is in the worktree is looked for outside the holds' lock, so
    /// a start elsewhere is not held up by a look that reads a slow disk;
    /// and a start into it that was listed while the removal looked, past
    /// what the look saw and no longer on its way in, keeps it.
    #[test]
    fn a_removal_looks_outside_the_lock_and_keeps_for_a_start_listed_meanwhile() {
        let wt = if cfg!(windows) { r"C:\p\.claude\worktrees\entry-look" } else { "/p/.claude/worktrees/entry-look" };
        let held = delete(wt, || {
            let (said, heard) = std::sync::mpsc::channel();
            let inside = format!("{wt}/src");
            std::thread::spawn(move || {
                // Listed as soon as it is in: its way in let go.
                let went_in = start(&[&inside]).is_ok();
                let _ = said.send(went_in);
            });
            let went_in = heard.recv_timeout(std::time::Duration::from_secs(30));
            assert_eq!(went_in, Ok(true), "a start was held up by the look");
            // The look went by before it was listed.
            false
        });
        assert!(held.is_none(), "kept for the start listed while it looked");
    }

    /// Starts elsewhere, however many come and go while a removal looks,
    /// leave it be: only one that may be in its worktree has it keep it.
    #[test]
    fn starts_elsewhere_while_a_removal_looks_do_not_keep_its_worktree() {
        let wt = if cfg!(windows) { r"C:\p\.claude\worktrees\entry-churn" } else { "/p/.claude/worktrees/entry-churn" };
        let elsewhere = if cfg!(windows) { r"C:\elsewhere\churn" } else { "/elsewhere/churn" };
        let held = delete(wt, || {
            for _ in 0..20 {
                drop(start(&[elsewhere]).unwrap());
            }
            false
        });
        assert!(held.is_some(), "removed: nobody came into it");
    }

    /// A path through a folder that does not read may lead to any worktree:
    /// the sweep, which only offers, takes it for in each; a delete compares
    /// it by its spelling, as every session of Luna's has its places resolved
    /// as it started. One that is only not there yet is compared as before.
    #[test]
    fn a_path_that_does_not_read_keeps_a_worktree_from_the_sweep_alone() {
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
        let place = |p: &str, denied: bool| Place::by(p, disk(denied));
        let wt_place = place(&wt, true);
        assert!(place(&session, true).within(&wt_place, Look::Sweep), "where it leads cannot be told");
        assert!(wt_place.within(&place(&session, true), Look::Sweep), "nor the other way about");
        assert!(place(&session, true).is(&place(&wt, true), Look::Sweep));
        assert!(!place(&session, true).within(&wt_place, Look::Delete), "a delete goes by its spelling");
        assert!(place(&wt, true).within(&wt_place, Look::Delete), "which is the worktree's");
        assert!(!place(&session, false).within(&place(&wt, false), Look::Sweep), "not there yet: by where it would be");
    }

    /// A start one of whose places does not read may be on its way into
    /// any worktree: while a removal is under way, a delete's too, it is
    /// refused, where a delete compares a listed session's such place by its
    /// spelling alone.
    #[test]
    fn a_start_whose_place_does_not_read_is_refused_while_a_worktree_goes() {
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let root = PathBuf::from(if cfg!(windows) { r"C:\" } else { "/" });
        let wt = s(&root.join("entry-unread").join(".claude").join("worktrees").join("w"));
        let session = s(&root.join("entry-denied").join("project"));
        let denied = |p: &Path| -> std::io::Result<PathBuf> {
            if p == root.as_path() {
                Ok(p.to_path_buf())
            } else {
                Err(std::io::ErrorKind::PermissionDenied.into())
            }
        };
        let unread = || Arc::new(Hold { places: vec![Place::by(&session, denied)], makers: Vec::new() });
        assert!(!unread().keeps(&Target::of(&wt, None), Look::Delete), "by its spelling, not in it");
        let held = delete(&wt, || false).expect("nobody there");
        let refused = entering(unread()).err().expect("refused while it goes");
        assert!(refused.contains("being removed") && refused.contains("where this start goes does not read"), "{refused}");
        assert!(!refused.contains("deleted from it"), "it is not said to go in: {refused}");
        drop(held);
    }

    /// A start one of whose places does not read, on its way in as a delete
    /// begins or let go of its way in while the delete looks, keeps the
    /// worktree: not listed yet, it would be compared by its spelling alone
    /// and could go in as the worktree goes.
    #[test]
    fn a_start_whose_place_does_not_read_on_its_way_in_keeps_a_deleted_worktree() {
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let root = PathBuf::from(if cfg!(windows) { r"C:\" } else { "/" });
        let wt = s(&root.join("entry-unread-way").join(".claude").join("worktrees").join("w"));
        let session = s(&root.join("entry-denied-way").join("project"));
        let denied = |p: &Path| -> std::io::Result<PathBuf> {
            if p == root.as_path() {
                Ok(p.to_path_buf())
            } else {
                Err(std::io::ErrorKind::PermissionDenied.into())
            }
        };
        let unread = || Arc::new(Hold { places: vec![Place::by(&session, denied)], makers: Vec::new() });
        let ways = apart();
        let delete = |others_in_it: &dyn Fn() -> bool| removing_by(ways, Target::of(&wt, None), Look::Delete, others_in_it);
        let on_the_way = entering_by(ways, unread()).expect("nothing is being removed");
        assert!(delete(&|| false).is_none(), "kept for it on its way in");
        drop(on_the_way);
        assert!(delete(&|| false).is_some(), "once it is listed, by its spelling");
        let held = delete(&|| {
            drop(entering_by(ways, unread()).expect("let in while the delete only looks"));
            false
        });
        assert!(held.is_none(), "kept for it, let go of its way in while the delete looked");
    }

    /// A removal whose own worktree does not read holds off every start
    /// while it runs, as none can be told to be outside it, and says that,
    /// not that the start goes into a worktree deleted with a session.
    #[test]
    fn a_start_held_off_by_a_removal_that_does_not_read_is_told_why() {
        let root = PathBuf::from(if cfg!(windows) { r"C:\" } else { "/" });
        let wt = root.join("entry-unread-target").join("w").to_string_lossy().into_owned();
        let denied = |p: &Path| -> std::io::Result<PathBuf> {
            if p == root.as_path() {
                Ok(p.to_path_buf())
            } else {
                Err(std::io::ErrorKind::PermissionDenied.into())
            }
        };
        let target = Target { path: wt.clone(), place: Place::by(&wt, denied), folder: None };
        let ways = apart();
        let held = removing_by(ways, target, Look::Delete, || false).expect("nobody there");
        let elsewhere = if cfg!(windows) { r"C:\elsewhere\unread-target" } else { "/elsewhere/unread-target" };
        let refused = entering_by(ways, Arc::new(Hold::start(&[elsewhere], None))).err().expect("refused while it goes");
        assert!(refused.starts_with(&wt) && refused.contains("no start can be told to be outside it"), "{refused}");
        assert!(!refused.contains("deleted from it"), "{refused}");
        drop(held);
        let inside = format!("{wt}/src");
        let held = delete(&wt, || false).expect("nobody there");
        let refused = start(&[&inside]).err().expect("refused: it goes in");
        assert!(refused.contains("with the session deleted from it"), "{refused}");
        drop(held);
    }

    /// A session that moved under a place it was started in, into a folder
    /// that is a junction or a link of its own (a `.claude/worktrees` kept
    /// elsewhere), is in the worktree where that leads too: a delete naming
    /// it by its real path keeps it.
    #[test]
    fn a_move_through_a_link_below_a_start_place_is_held_where_it_leads() {
        let root = std::env::temp_dir().join(format!("luna-entry-below-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let project = root.join("project");
        let elsewhere = root.join("elsewhere");
        std::fs::create_dir_all(project.join(".claude")).unwrap();
        std::fs::create_dir_all(elsewhere.join("w").join("src")).unwrap();
        super::super::link_dir(&project.join(".claude").join("worktrees"), &elsewhere);
        let started = Hold::start(&[&s(&project)], None);
        let mut now = Hold::default();
        now.add_at(&started, &s(&project.join(".claude").join("worktrees").join("w").join("src")));
        assert!(now.keeps(&Target::of(&s(&elsewhere.join("w")), None), Look::Delete), "by the real path");
        assert!(now.keeps(&Target::of(&s(&project.join(".claude").join("worktrees").join("w")), None), Look::Delete));
        super::super::unlink_dir(&project.join(".claude").join("worktrees"));
        let _ = std::fs::remove_dir_all(&root);
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

        let on_the_way = start(&[&s(&linked.join("src"))]).unwrap();
        assert!(delete(&wt, || false).is_none(), "kept for a start on its way in through the link");
        drop(on_the_way);
        let held = delete(&s(&linked), || false).expect("nobody there");
        assert!(start(&[&wt]).is_err(), "a start by the real path is refused while it goes by the link");
        drop(held);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A session started through a link to its project keeps the places it
    /// went into where they led as it started, the worktree it was started
    /// into and one its CLI says it moved to by the link's spelling alike:
    /// once the link is gone, a delete by the real path still finds it in
    /// them. Where the link leads read only then, it would not.
    #[test]
    fn a_session_keeps_its_places_where_they_led_as_it_started() {
        let root = std::env::temp_dir().join(format!("luna-entry-pinned-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let worktrees = |project: &str| root.join(project).join(".claude").join("worktrees");
        std::fs::create_dir_all(worktrees("real").join("w")).unwrap();
        std::fs::create_dir_all(worktrees("real").join("moved")).unwrap();
        super::super::link_dir(&root.join("link"), &root.join("real"));
        let started = Hold::start(&[&s(&root.join("link")), &s(&worktrees("link").join("w"))], None);
        super::super::unlink_dir(&root.join("link"));
        let real = |name: &str| Target::of(&s(&worktrees("real").join(name)), None);
        assert!(started.keeps(&real("w"), Look::Delete), "the worktree it was started into");
        let mut now = Hold::default();
        now.add_at(&started, &s(&worktrees("link").join("moved").join("src")));
        assert!(now.keeps(&real("moved"), Look::Delete), "the one it moved to, by the link's spelling");
        let late = Hold::start(&[&s(&worktrees("link").join("w"))], None);
        assert!(!late.keeps(&real("w"), Look::Delete), "read once the link is gone, it would be missed");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A start asking its CLI for a new worktree whose path the CLI picks
    /// keeps every worktree where that CLI makes them for its folder from the
    /// sweep while it is on its way in, before its session is listed: its
    /// CLI may have made its checkout already. Not from a delete, whose
    /// worktree is the deleted chat's own and no new one.
    #[test]
    fn a_start_making_a_worktree_keeps_where_it_makes_them_from_the_sweep_on_its_way_in() {
        let folder = if cfg!(windows) { r"C:\entry-making" } else { "/entry-making" };
        let wt = claude_worktrees(folder).join("picked-by-its-cli").to_string_lossy().into_owned();
        let making = Unplaced { provider: Provider::Claude, folder: folder.into(), account_path: "acc".into() };
        let sweep = || removing(Target::of(&wt, Some(Place::of(folder))), Look::Sweep, || false);
        let on_the_way = entering(Arc::new(Hold::start(&[folder], Some(&making)))).unwrap();
        assert!(entering_now().keeps(&Target::of(&wt, Some(Place::of(folder))), Look::Sweep), "to the sweep's listing");
        assert!(sweep().is_none(), "and to its removal");
        assert!(delete(&wt, || false).is_some(), "not to a delete");
        drop(on_the_way);
        assert!(sweep().is_some(), "once it is listed, its session holds it");
    }
}
