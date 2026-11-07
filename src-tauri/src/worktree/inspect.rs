//! What deleting a worktree would throw away: its uncommitted changes and the
//! commits on no branch that stays and no remote, asked for when someone means to
//! delete.

use super::*;

/// What deleting a worktree would throw away.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeInfo {
    pub path: String,
    pub branch: Option<String>,
    /// Entries `git status` lists: modified, staged, untracked.
    pub uncommitted: Option<u32>,
    /// Commits that removing it would lose for good: on its HEAD or on a
    /// branch that goes with it, and on no branch that stays and no remote
    /// (`commits_lost`, what removal itself goes by).
    pub unique_commits: Option<u32>,
    /// Newest of the directory's and its index's change times, ms since epoch.
    pub touched_ms: Option<u64>,
    /// Git cannot read it as a checkout (a half-deleted directory, say), so
    /// nothing above could be counted.
    pub broken: bool,
}

fn mtime_ms(p: &Path) -> Option<u64> {
    let t = std::fs::metadata(p).ok()?.modified().ok()?;
    Some(t.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as u64)
}

/// What `git status` lists in a worktree, a line per change, untracked files
/// and whatever a submodule hides included (a repo's config may ask to hide it);
/// not files git ignores, or build output would stop every delete. None when git
/// cannot read it as a checkout.
fn changes_in(path: &str) -> Option<Vec<String>> {
    // Without its own `.git`, git would walk up and answer for the main
    // checkout instead.
    let status = Path::new(path)
        .join(".git")
        .exists()
        .then(|| {
            git(path)
                .args(["-c", "core.quotepath=off", "status", "--porcelain", "--untracked-files=normal", "--ignore-submodules=none"])
                .output()
                .ok()
        })
        .flatten()
        .filter(|o| o.status.success())?;
    Some(String::from_utf8_lossy(&status.stdout).lines().filter(|l| !l.is_empty()).map(str::to_owned).collect())
}

/// How many commits go for good with this worktree: those its HEAD or a branch
/// going with it can reach (`throwaway_branches`, deleted whatever they hold)
/// and no branch that stays and no remote can. A detached HEAD, as Codex's
/// worktrees have, is reachable from nothing else once it is gone; a branch the
/// session named itself stays, its commits with it. Asked of the repository: a
/// checkout removed by hand leaves its branch to go all the same. None: unknown.
fn commits_lost(folder: &str, path: &str, checked_out: Option<&str>) -> Option<u32> {
    let going = throwaway_branches(path, checked_out);
    let mut rev = git(folder);
    rev.args(["rev-list", "--count"]);
    let mut tips = 0;
    if Path::new(path).join(".git").exists() {
        let head = git(path).args(["rev-parse", "--verify", "HEAD"]).output().ok().filter(|o| o.status.success())?;
        rev.arg(String::from_utf8_lossy(&head.stdout).trim());
        tips += 1;
    }
    for b in &going {
        let there = git(folder).args(["show-ref", "--verify", "--quiet", &format!("refs/heads/{b}")]).output().ok()?.status.success();
        if there {
            rev.arg(format!("refs/heads/{b}"));
            tips += 1;
        }
    }
    if tips == 0 {
        return Some(0);
    }
    rev.arg("--not");
    for b in &going {
        rev.arg(format!("--exclude={b}"));
    }
    rev.args(["--branches", "--remotes"]);
    rev.output().ok().filter(|o| o.status.success()).and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
}

/// The most changes a refusal to remove a worktree names: it is read by a
/// model that has the whole of `git status` a command away.
const UNSAVED_NAMED: usize = 10;

/// What removing this worktree would lose for good, in words: the changes in
/// it and the commits that would go with it, its checkout gone or not. None
/// when that is nothing; what cannot be told counts as something.
pub fn unsaved_work(folder: &str, path: &str) -> Option<String> {
    let mut said = Vec::new();
    if Path::new(path).exists() {
        let Some(changes) = changes_in(path) else {
            return Some("files git cannot read as a checkout, so what they are worth is unknown".into());
        };
        if !changes.is_empty() {
            let names: Vec<&str> = changes.iter().take(UNSAVED_NAMED).map(|l| l.get(3..).unwrap_or(l)).collect();
            let more = match changes.len().saturating_sub(UNSAVED_NAMED) {
                0 => String::new(),
                n => format!(" and {n} more"),
            };
            let what =
                if changes.len() == 1 { "1 uncommitted change".to_string() } else { format!("{} uncommitted changes", changes.len()) };
            said.push(format!("{what} ({}{more})", names.join(", ")));
        }
    }
    match commits_lost(folder, path, branch_of(folder, path).as_deref()) {
        None => said.push("commits git could not count".into()),
        Some(0) => {}
        Some(1) => said.push("1 commit on no other branch or remote".into()),
        Some(n) => said.push(format!("{n} commits on no other branch or remote")),
    }
    (!said.is_empty()).then(|| said.join(" and "))
}

fn inspect(folder: &str, path: &str) -> WorktreeInfo {
    let branch = branch_of(folder, path);
    let wt = Path::new(path);
    // A linked worktree's `.git` is a file naming its admin dir; the index
    // lives there and moves with every add, commit and checkout.
    let index = std::fs::read_to_string(wt.join(".git"))
        .ok()
        .and_then(|t| t.trim().strip_prefix("gitdir:").map(|g| PathBuf::from(g.trim()).join("index")));
    let touched_ms = [mtime_ms(wt), index.as_deref().and_then(mtime_ms)].into_iter().flatten().max();

    let Some(changes) = changes_in(path) else {
        return WorktreeInfo { path: path.to_string(), branch, uncommitted: None, unique_commits: None, touched_ms, broken: true };
    };
    let unique_commits = commits_lost(folder, path, branch.as_deref());

    WorktreeInfo { path: path.to_string(), branch, uncommitted: Some(changes.len() as u32), unique_commits, touched_ms, broken: false }
}

/// Looks inside each worktree before it is offered for deletion. One git
/// status per worktree, side by side: a large checkout takes a second or two.
#[tauri::command]
pub async fn inspect_worktrees(folder: String, paths: Vec<String>) -> Result<Vec<WorktreeInfo>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        std::thread::scope(|scope| {
            let jobs: Vec<_> = paths.iter().filter(|p| is_worktree_of(&folder, p)).map(|p| scope.spawn(|| inspect(&folder, p))).collect();
            jobs.into_iter().filter_map(|j| j.join().ok()).collect()
        })
    })
    .await
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_worktree_says_what_removing_it_would_lose() {
        let Some((root, repo, wt)) = codex_layout("unsaved") else { return };
        assert_eq!(unsaved_work(&repo, &wt), None, "clean, its HEAD on the main branch too");
        std::fs::write(Path::new(&wt).join("new file.txt"), "x").unwrap();
        let said = unsaved_work(&repo, &wt).unwrap();
        assert!(said.starts_with("1 uncommitted change (") && said.contains("new file.txt"), "{said}");
        for i in 0..UNSAVED_NAMED + 3 {
            std::fs::write(Path::new(&wt).join(format!("f{i}.txt")), "x").unwrap();
        }
        let said = unsaved_work(&repo, &wt).unwrap();
        assert!(said.contains("14 uncommitted changes") && said.contains("and 4 more"), "{said}");
        let ok = |c: &mut Command| c.output().is_ok_and(|o| o.status.success());
        assert!(ok(git(&wt).args(["add", "-A"])));
        assert!(ok(git(&wt).args(["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "-m", "w"])));
        assert_eq!(unsaved_work(&repo, &wt).unwrap(), "1 commit on no other branch or remote");
        assert!(ok(git(&wt).args(["branch", "kept"])));
        assert_eq!(unsaved_work(&repo, &wt), None, "another branch holds it now");
        std::fs::remove_file(Path::new(&wt).join(".git")).unwrap();
        assert!(unsaved_work(&repo, &wt).unwrap().contains("cannot read"), "unknown is not nothing");
        assert_eq!(unsaved_work(&repo, &root.join("gone").to_string_lossy()), None, "gone takes nothing with it");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_branch_that_goes_with_the_worktree_is_no_home_for_its_commits_and_one_that_stays_is() {
        let Some((root, repo, _)) = codex_layout("branches") else { return };
        let wt = root.join("repo").join(".claude").join("worktrees").join("w1");
        let wt = wt.to_string_lossy().into_owned();
        let ok = |c: &mut Command| c.output().is_ok_and(|o| o.status.success());
        assert!(ok(git(&repo).args(["worktree", "add", "-q", "-b", "worktree-w1", &wt])));
        let commit =
            |msg: &str| ok(git(&wt).args(["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", msg]));
        assert_eq!(unsaved_work(&repo, &wt), None, "nothing past what it was made from");
        assert!(commit("on the throwaway branch"));
        assert_eq!(unsaved_work(&repo, &wt).unwrap(), "1 commit on no other branch or remote", "branch -D takes it");
        // Moved onto a branch of its own: that one stays, with the commit.
        assert!(ok(git(&wt).args(["switch", "-q", "-c", "keeper"])));
        assert!(commit("on its own branch"));
        assert_eq!(unsaved_work(&repo, &wt), None, "nothing is lost with `keeper` staying");
        // Back on the throwaway one, past what `keeper` has.
        assert!(ok(git(&wt).args(["switch", "-q", "worktree-w1"])));
        assert!(commit("again"));
        assert_eq!(unsaved_work(&repo, &wt).unwrap(), "1 commit on no other branch or remote");
        // And a throwaway branch left behind counts though HEAD is elsewhere.
        assert!(ok(git(&wt).args(["switch", "-q", "keeper"])));
        assert_eq!(unsaved_work(&repo, &wt).unwrap(), "1 commit on no other branch or remote");
        // What removal says it did.
        assert_eq!(throwaway_branches(&wt, Some("keeper")), ["worktree-w1"]);
        remove_worktree_now(repo.clone(), wt.clone()).unwrap();
        let branches = String::from_utf8_lossy(&git(&repo).args(["branch", "--list"]).output().unwrap().stdout).into_owned();
        assert!(branches.contains("keeper") && !branches.contains("worktree-w1"), "{branches}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_checkout_gone_leaves_its_branch_to_answer_for() {
        let Some((root, repo, _)) = codex_layout("gone") else { return };
        let wt = root.join("repo").join(".claude").join("worktrees").join("w2").to_string_lossy().into_owned();
        let ok = |c: &mut Command| c.output().is_ok_and(|o| o.status.success());
        assert!(ok(git(&repo).args(["worktree", "add", "-q", "-b", "worktree-w2", &wt])));
        assert!(ok(git(&wt).args(["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "c"])));
        std::fs::remove_dir_all(&wt).unwrap();
        assert_eq!(unsaved_work(&repo, &wt).unwrap(), "1 commit on no other branch or remote", "branch -D would take it");
        assert!(ok(git(&repo).args(["branch", "kept", "worktree-w2"])));
        assert_eq!(unsaved_work(&repo, &wt), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_hidden_submodule_change_counts_and_an_ignored_file_does_not() {
        let Some((root, repo, wt)) = codex_layout("sub") else { return };
        let ok = |c: &mut Command| c.output().is_ok_and(|o| o.status.success());
        let user = ["-c", "user.email=t@t", "-c", "user.name=t"];
        std::fs::write(Path::new(&wt).join(".gitignore"), "build/\nsub-ignored\n").unwrap();
        let sub = Path::new(&wt).join("sub").to_string_lossy().into_owned();
        assert!(ok(git(&wt).args(["init", "-q", "sub"])));
        assert!(ok(git(&sub).args(user).args(["commit", "-q", "--allow-empty", "-m", "s1"])));
        assert!(ok(git(&wt).args(["add", "-A"])));
        assert!(ok(git(&wt).args(user).args(["commit", "-q", "-m", "w"])));
        assert!(ok(git(&wt).args(["branch", "kept"])));
        assert_eq!(ignored_in(&wt), Some(false), "nothing ignored yet: a delete says nothing of it");
        std::fs::create_dir_all(Path::new(&wt).join("build")).unwrap();
        std::fs::write(Path::new(&wt).join("build").join("out.bin"), "x").unwrap();
        assert_eq!(unsaved_work(&repo, &wt), None, "build output would stop every delete");
        assert_eq!(ignored_in(&wt), Some(true), "and goes with it, which a delete says");
        // Moved on in the submodule, with the repo's config asking not to see it.
        assert!(ok(git(&repo).args(["config", "diff.ignoreSubmodules", "all"])));
        assert!(ok(git(&sub).args(user).args(["commit", "-q", "--allow-empty", "-m", "s2"])));
        let said = unsaved_work(&repo, &wt).unwrap_or_default();
        assert!(said.contains("1 uncommitted change") && said.contains("sub"), "{said}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
