//! Where a spawn's caller works, and who else is there.

use crate::agents::list::disk_meta;
use crate::agents::registry::{chat_name, is_descendant, persons_chat_in, reg};
use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Neighbor {
    pub id: String,
    pub name: String,
    pub busy: bool,
}

/// Live sessions other than `skip` that run in `dir`, by what the CLI says
/// its cwd is: the project folder for a chat in it, the worktree for one in
/// a worktree. Only those in `caller`'s reach: any for a person's chat, its
/// own subtree for a helper, which learns of no session past it (ADR-0025).
pub(in crate::agents) fn neighbors_in(dir: &str, caller: &str, skip: &[&str]) -> Vec<Neighbor> {
    let ids: Vec<String> = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let all = persons_chat_in(&r, caller);
        r.known.keys().filter(|id| all || is_descendant(&r, caller, id)).cloned().collect()
    };
    let pty = crate::pty::manager();
    let mut out = Vec::new();
    for id in ids.into_iter().filter(|id| !skip.contains(&id.as_str())) {
        let cwd = match (crate::background::working_dir(&id), pty.probe(&id)) {
            (Some(dir), _) => dir,
            (None, Some(probe)) => disk_meta(&probe).and_then(|m| m.cwd).unwrap_or_else(|| probe.cwd.clone()),
            (None, None) => continue,
        };
        if crate::pty::same_dir(&cwd, dir) {
            out.push(Neighbor {
                busy: crate::activity::turn_state(&id).is_some_and(|t| t.busy),
                name: chat_name(&id).unwrap_or_default(),
                id,
            });
        }
    }
    out
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
    // Where the CLI says its shell is, which a `cd` sets: on a share its own
    // folder is not on, it is none, before anything there is looked at.
    if crate::boundary::foreign_share(&cwd, &folder) {
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

    /// `sameFolder` names, for a helper, only the sessions under it; a person's
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
        let near = |caller: &str| {
            let mut n: Vec<String> =
                neighbors_in(dir, caller, &[caller]).into_iter().map(|n| n.id).filter(|i| i.starts_with("nb-")).collect();
            n.sort();
            n
        };
        assert_eq!(near("nb-lead"), ["nb-mine"], "its own only: not its parent's other helper, not another chat's");
        assert_eq!(near("nb-mine"), Vec::<String>::new());
        assert_eq!(near("nb-top"), ["nb-lead", "nb-mine", "nb-sib", "nb-theirs"], "a person's chat hears of every one");
        for id in ids {
            crate::background::remove(id);
        }
        for id in ["nb-mine", "nb-lead", "nb-sib", "nb-theirs", "nb-other", "nb-top"] {
            forget(id);
        }
    }
}
