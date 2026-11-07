//! Paths in this system's spelling, places to compare them by, and a repository
//! with a worktree laid out as Codex lays one out, for the tests that need git.

use super::*;

/// A path written `E:\p\x` as this platform has it (`/E/p/x` off Windows),
/// for what takes one apart by the platform's own separators.
pub(super) fn native(path: &str) -> String {
    if cfg!(windows) {
        path.to_string()
    } else {
        format!("/{}", path.replace(':', "").replace('\\', "/"))
    }
}

/// The folder a path leads to, to compare by: what Luna says of a project may be
/// in git's spelling (long names) and a test's own in short ones. One gone, by
/// its text.
pub(super) fn place(path: &str) -> String {
    std::fs::canonicalize(path).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| norm(path))
}

/// A repository with one linked worktree where Codex puts them, under a
/// fake account (`<root>/account`, `root` taken for the accounts root's
/// Codex folder): (root, repo, worktree), spelled short where the volume
/// has short names. None where there is no git.
pub(super) fn codex_layout(tag: &str) -> Option<(PathBuf, String, String)> {
    let root = std::env::temp_dir().join(format!("luna-wt-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).ok()?;
    let root = short_spelling(&root);
    TEST_CODEX_ACCOUNTS.lock().unwrap().push(root.to_string_lossy().into_owned());
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).ok()?;
    let repo_s = repo.to_string_lossy().into_owned();
    let ok = |c: &mut Command| c.output().is_ok_and(|o| o.status.success());
    if !ok(git(&repo_s).args(["init", "-q"]))
        || !ok(git(&repo_s).args(["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "i"]))
    {
        return None;
    }
    let wt = root.join("account").join("worktrees").join("0a1b").join("repo");
    std::fs::create_dir_all(wt.parent()?).ok()?;
    let wt_s = wt.to_string_lossy().into_owned();
    ok(git(&repo_s).args(["worktree", "add", "-q", "--detach", &wt_s])).then_some((root, repo_s, wt_s))
}
