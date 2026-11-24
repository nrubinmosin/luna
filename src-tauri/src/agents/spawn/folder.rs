//! Where a spawn's caller works, and who else is there.

use crate::agents::list::disk_meta;
use crate::agents::registry::{is_descendant, persons_chat_in, reg};

/// Live sessions other than `skip` that run in `dir`, by what the CLI says
/// its cwd is: the project folder for a chat in it, the worktree for one in
/// a worktree. Only those in `caller`'s reach: any for a person's chat, its
/// own subtree for a helper, which learns of no session past it (ADR-0025).
pub(in crate::agents) fn count_neighbors(dir: &str, caller: &str, skip: &[&str]) -> usize {
    let ids: Vec<String> = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let all = persons_chat_in(&r, caller);
        r.known.keys().filter(|id| all || is_descendant(&r, caller, id)).cloned().collect()
    };
    let pty = crate::pty::manager();
    ids.into_iter()
        .filter(|id| !skip.contains(&id.as_str()))
        .filter(|id| {
            let cwd = crate::background::working_dir(id).or_else(|| {
                let probe = pty.probe(id)?;
                Some(disk_meta(&probe).and_then(|m| m.cwd).unwrap_or(probe.cwd))
            });
            cwd.is_some_and(|cwd| crate::pty::same_dir(&cwd, dir))
        })
        .count()
}

/// The worktree a caller started with `--worktree` runs in. It registers
/// with the project folder, the only one known at spawn, and the CLI makes
/// the worktree itself; what it leaves on disk says where that went.
pub(in crate::agents) fn caller_worktree(caller: &str) -> Option<String> {
    let folder = reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(caller)?.folder.clone();
    let cwd = match crate::background::working_dir(caller) {
        Some(dir) => dir,
        None => crate::pty::manager().probe(caller)?.working_dir()?,
    };
    // Where the CLI says its shell is, which a `cd` sets: no plain absolute
    // path, or on a share its own folder is not on, it is none, before
    // anything there is looked at; in Luna's own folder, it is none before
    // its `.git` is read.
    if !crate::boundary::derived_ok(&cwd, &folder) || crate::agents::admission::reaches_luna(&cwd) {
        return None;
    }
    // Its root: the caller's shell may be in a folder of it.
    let (root, _) = crate::checkout::root_of(&cwd)?;
    crate::checkout::is_worktree_of(&folder, &root).then_some(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::registry::register;
    use crate::agents::teardown::forget;
    use crate::provider::Provider;

    /// `sameFolder` counts, for a helper, only the sessions under it; a person's
    /// chat hears of every session in the folder.
    #[test]
    fn a_helper_hears_of_no_neighbor_past_its_own_subtree() {
        let dir = "C:\\nb-shared\\project";
        let bg = serde_json::json!({ "background": true });
        register("nb-top", Provider::Claude, dir, "C:\\acc\\anthropic\\one", None, true, serde_json::json!({}));
        register("nb-lead", Provider::Claude, dir, "C:\\acc\\anthropic\\one", Some("nb-top"), true, bg.clone());
        register("nb-mine", Provider::Claude, dir, "C:\\acc\\anthropic\\one", Some("nb-lead"), false, bg.clone());
        register("nb-sib", Provider::Claude, dir, "C:\\acc\\anthropic\\one", Some("nb-top"), false, bg.clone());
        register("nb-other", Provider::Claude, dir, "C:\\acc\\anthropic\\one", None, false, serde_json::json!({}));
        register("nb-theirs", Provider::Claude, dir, "C:\\acc\\anthropic\\one", Some("nb-other"), false, bg);
        let ids = ["nb-lead", "nb-mine", "nb-sib", "nb-theirs"];
        for id in ids {
            crate::background::take_up_for_test(id, dir, 1, &[], None);
        }
        let near = |caller: &str| count_neighbors(dir, caller, &[caller]);
        assert_eq!(near("nb-lead"), 1, "its own only");
        assert_eq!(near("nb-mine"), 0);
        assert_eq!(near("nb-top"), 4, "a person's chat hears of every one");
        for id in ids {
            crate::background::remove(id);
        }
        for id in ["nb-mine", "nb-lead", "nb-sib", "nb-theirs", "nb-other", "nb-top"] {
            forget(id);
        }
    }
}
