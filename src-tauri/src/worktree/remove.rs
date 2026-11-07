//! Removing a worktree with its chat: only one git records, and with it the
//! throwaway branches it was made on and left on.

use super::*;

// Every git call below is slow enough to be felt — `worktree remove` on a large
// checkout has taken ten seconds here. Tauri runs a synchronous command on the
// main thread, where that time is charged to every other pending IPC call, so
// the commands that reach these are `async` wrappers, which Tauri runs off the
// main thread, over the blocking helpers the rest of the crate keeps calling.
pub fn remove_worktree_now(folder: String, worktree_path: String) -> Result<(), String> {
    if !Path::new(&worktree_path).exists() {
        // Still worth pruning a stale admin entry and its branch. A Codex
        // worktree that is gone cannot show whose it was; git's prune only
        // drops entries whose folder is missing, so nothing else is at risk.
        if !under_claude_worktrees(&folder, &worktree_path) && !codex_made(&worktree_path) {
            return Err("refusing: path is not a worktree of this folder".into());
        }
        let branch = branch_of(&folder, &worktree_path);
        let _ = git(&folder).args(["worktree", "unlock", &worktree_path]).output();
        let _ = git(&folder).args(["worktree", "prune"]).output();
        drop_branches(&folder, &worktree_path, branch);
        return Ok(());
    }
    if !is_worktree_of(&folder, &worktree_path) {
        return Err("refusing: path is not a worktree of this folder".into());
    }
    // A Codex one goes by its `.git` file above, which proves nothing alone:
    // git must have it too, or the fallback below would delete by hand a
    // folder git never made. Claude Code's are by place, and one git has lost
    // track of (a removal stopped halfway) is still for that fallback.
    if codex_made(&worktree_path) && !registered(&folder, &worktree_path) {
        return Err("refusing: git keeps no record of this worktree".into());
    }

    // Resolve the branch before the worktree disappears from git's records.
    let branch = branch_of(&folder, &worktree_path);

    // The killed session's processes — the CLI, and whatever shell or build it
    // had running — let go of their files over a few seconds, not at once:
    // three tries half a second apart lost that race to an open handle
    // ("os error 32"). Back off for up to ~8s in all. Claude Code locks the
    // worktree it makes and leaves the lock behind when killed; one --force
    // refuses a locked worktree, the second overrides the lock.
    let mut removed = false;
    let mut unknown = false;
    let mut why = String::new();
    for attempt in 1..=6u64 {
        match git(&folder)
            .args(["worktree", "remove", "--force", "--force", &worktree_path])
            // Its message is read below.
            .env("LC_ALL", "C")
            .output()
        {
            Ok(o) if o.status.success() => {
                removed = true;
                break;
            }
            Ok(o) => why = String::from_utf8_lossy(&o.stderr).trim().to_string(),
            Err(e) => why = e.to_string(),
        }
        // A directory git keeps no record of (its entry pruned, or its `.git`
        // file gone with a removal that stopped halfway) is never going to be
        // git's to remove: the waiting goes to removing it by hand instead.
        if why.contains("is not a working tree") {
            unknown = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(300 * attempt));
    }
    if !removed {
        let mut gone = std::fs::remove_dir_all(&worktree_path);
        let retries = if unknown { 8 } else { 3 };
        for _ in 0..retries {
            if gone.is_ok() || !Path::new(&worktree_path).exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1000));
            gone = std::fs::remove_dir_all(&worktree_path);
        }
        if let Err(e) = gone {
            crate::log::warn("worktree", &format!("could not remove {worktree_path}: git said {why:?}; {e}"));
            // Names the worktree, so the UI and the caller's own log line say
            // which folder something still holds.
            return Err(format!("could not remove worktree {worktree_path}: {e}"));
        }
        // prune skips a locked entry, and the branch it keeps checked out
        // would then refuse `branch -D` below.
        let _ = git(&folder).args(["worktree", "unlock", &worktree_path]).output();
        let _ = git(&folder).args(["worktree", "prune"]).output();
    }

    let deleted = drop_branches(&folder, &worktree_path, branch.clone());
    let kept = branch.filter(|b| !deleted.contains(b));
    crate::log::info("worktree", &format!("removed {worktree_path}: branches deleted {deleted:?}, kept {kept:?}"));
    // Codex keeps each checkout in a bucket of its own, which git leaves
    // behind empty.
    if codex_made(&worktree_path) {
        if let Some(bucket) = Path::new(&worktree_path).parent() {
            let _ = std::fs::remove_dir(bucket);
        }
    }
    Ok(())
}

/// The branch Claude Code made the worktree on, `worktree-<dir>`. What is
/// checked out there at the end may be another branch — a session is free to
/// switch — and the one it was made on would then be left behind. Codex makes
/// no branch: its worktrees start on a detached HEAD.
pub(super) fn made_branch(worktree_path: &str) -> Option<String> {
    if codex_shaped(worktree_path) {
        return None;
    }
    let dir = Path::new(worktree_path).file_name()?.to_str()?;
    Some(format!("worktree-{dir}"))
}

/// The throwaway branches of a worktree, which go with it: the one it was made
/// on, and the one checked out at the end if that is throwaway too.
/// Restricted to Claude Code's `worktree-*` naming: anything else is a branch
/// someone made on purpose, and `-D` would discard real commits.
pub(super) fn throwaway_branches(worktree_path: &str, checked_out: Option<&str>) -> Vec<String> {
    let mut branches: Vec<String> =
        made_branch(worktree_path).into_iter().chain(checked_out.map(str::to_owned)).filter(|b| b.starts_with("worktree-")).collect();
    branches.dedup();
    branches
}

/// Deletes the throwaway branches of a removed worktree, and returns the ones
/// that went.
fn drop_branches(folder: &str, worktree_path: &str, checked_out: Option<String>) -> Vec<String> {
    throwaway_branches(worktree_path, checked_out.as_deref())
        .into_iter()
        .filter(|branch| git(folder).args(["branch", "-D", branch]).output().is_ok_and(|o| o.status.success()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_shaped_like_codexs_is_deleted_only_in_a_codex_account_and_with_gits_record() {
        let Some((root, repo, wt)) = codex_layout("anchor") else { return };
        let dot_git = std::fs::read_to_string(Path::new(&wt).join(".git")).unwrap();
        let forge = |at: PathBuf| {
            std::fs::create_dir_all(&at).unwrap();
            std::fs::write(at.join(".git"), &dot_git).unwrap();
            std::fs::write(at.join("keep.txt"), "mine").unwrap();
            at.to_string_lossy().into_owned()
        };
        // Its shape and a `.git` file pointing at the repository, out of any account.
        let astray = forge(root.join("elsewhere").join("x").join("worktrees").join("0a1b").join("repo"));
        assert!(codex_shaped(&astray) && !is_worktree_of(&repo, &astray));
        assert!(remove_worktree_now(repo.clone(), astray.clone()).is_err());
        assert!(Path::new(&astray).join("keep.txt").exists());
        // In the account, where Codex would put it, but never made by git.
        let forged = forge(root.join("account").join("worktrees").join("ffff").join("repo"));
        assert!(is_worktree_of(&repo, &forged), "its place and its `.git` file say so");
        assert!(remove_worktree_now(repo.clone(), forged.clone()).unwrap_err().contains("no record"));
        assert!(Path::new(&forged).join("keep.txt").exists());
        // The real one goes, bucket and all.
        assert!(registered(&repo, &wt));
        remove_worktree_now(repo.clone(), wt.clone()).unwrap();
        assert!(!Path::new(&wt).exists() && !Path::new(&wt).parent().unwrap().exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_worktree_names_the_branch_it_was_made_on() {
        assert_eq!(
            made_branch(&native(r"E:\p\.claude\worktrees\merry-coalescing-cosmos")).as_deref(),
            Some("worktree-merry-coalescing-cosmos")
        );
        assert_eq!(made_branch(&native(r"E:\acc\worktrees\d6ff\repo")), None);
    }
}
