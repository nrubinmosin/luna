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

use std::sync::Mutex;

#[derive(Default)]
struct Ways {
    /// Each start on its way in, by a number of its own: the folders it goes
    /// into.
    entering: Vec<(u64, Vec<String>)>,
    removing: Vec<String>,
    next: u64,
}

static WAYS: Mutex<Ways> = Mutex::new(Ways { entering: Vec::new(), removing: Vec::new(), next: 0 });

fn ways() -> std::sync::MutexGuard<'static, Ways> {
    WAYS.lock().unwrap_or_else(|e| e.into_inner())
}

/// A path as compared here: either slash, no trailing one, and case aside,
/// as `worktree_shared` has always compared them.
fn key(p: &str) -> String {
    p.replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

/// Whether `path` is the worktree `wt` or inside it.
pub(crate) fn within(path: &str, wt: &str) -> bool {
    let (p, wt) = (key(path), key(wt));
    p == wt || p.starts_with(&format!("{wt}\\"))
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
    let mut w = ways();
    if let Some(wt) = w.removing.iter().find(|wt| paths.iter().any(|p| within(p, wt))) {
        return Err(format!("{wt} is being removed with the chat that was in it; start again once it is gone"));
    }
    w.next += 1;
    let n = w.next;
    w.entering.push((n, paths.iter().map(|p| p.to_string()).collect()));
    Ok(Entering(n))
}

/// A worktree held for its removal, until it is dropped.
pub(crate) struct Removing(String);

impl Drop for Removing {
    fn drop(&mut self) {
        let mut w = ways();
        if let Some(i) = w.removing.iter().position(|wt| *wt == self.0) {
            w.removing.remove(i);
        }
    }
}

/// Holds `wt` for its removal, unless a start is on its way into it or
/// `others_in_it` (asked under the same lock) finds someone there: then None,
/// and the worktree stays.
pub(crate) fn removing(wt: &str, others_in_it: impl FnOnce() -> bool) -> Option<Removing> {
    let mut w = ways();
    if w.entering.iter().any(|(_, paths)| paths.iter().any(|p| within(p, wt))) || others_in_it() {
        return None;
    }
    w.removing.push(wt.to_string());
    Some(Removing(wt.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let on_the_way = entering(&[&inside.to_uppercase()]).expect("let in once it is gone");
        assert!(removing(wt, || false).is_none(), "kept for the start on its way in");
        drop(on_the_way);
        assert!(removing(wt, || true).is_none(), "kept for whoever is in it");
        assert!(removing(wt, || false).is_some());
    }
}
