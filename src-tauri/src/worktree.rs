//! Git worktrees for isolated chats. Claude Code makes its own under
//! `<folder>/.claude/worktrees` (`--worktree`, branch `worktree-*`); Codex has
//! no equivalent Luna wants to lean on, so Luna makes one for it under
//! `<folder>/.codex/worktrees` (branch `codex-*`) and starts the session
//! inside. Removal, orphan detection and the branch cleanup are shared.

use crate::provider::Provider;
use std::path::{Path, PathBuf};
use std::process::Command;

fn norm(p: &str) -> String {
    p.replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

fn git(folder: &str) -> Command {
    let mut c = Command::new("git");
    c.arg("-C").arg(folder);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

fn worktrees_dir(folder: &str, provider: Provider) -> PathBuf {
    let dot = match provider {
        Provider::Claude => ".claude",
        Provider::Codex => ".codex",
    };
    Path::new(folder).join(dot).join("worktrees")
}

/// True when `candidate` sits under this folder's `.claude/worktrees` or
/// `.codex/worktrees`. Every destructive path below is gated on this.
pub fn is_worktree_of(folder: &str, candidate: &str) -> bool {
    let c = norm(candidate);
    Provider::ALL.iter().any(|&p| {
        let base = norm(&worktrees_dir(folder, p).to_string_lossy());
        c.len() > base.len() && c.starts_with(&base)
    })
}

/// The project folder a Luna-made worktree belongs to: the part of the path
/// before `.claude\worktrees` or `.codex\worktrees`. A Codex chat in a
/// worktree is started in the worktree itself, so that is the folder it knows.
pub fn project_of(path: &str) -> Option<String> {
    let p = path.replace('/', "\\");
    let lower = p.to_ascii_lowercase();
    ["\\.claude\\worktrees\\", "\\.codex\\worktrees\\"]
        .iter()
        .filter_map(|m| lower.find(m))
        .min()
        .filter(|&at| at > 0)
        .map(|at| p[..at].to_string())
}

/// Branch checked out in the given worktree, per `git worktree list`.
fn branch_of(folder: &str, worktree_path: &str) -> Option<String> {
    let out = git(folder)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let want = norm(worktree_path);
    let mut current: Option<String> = None;
    for line in text.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            current = Some(norm(p));
        } else if let Some(b) = line.strip_prefix("branch refs/heads/") {
            if current.as_deref() == Some(want.as_str()) {
                return Some(b.trim().to_string());
            }
        }
    }
    None
}

/// Six hex characters nobody will type twice: time and pid through the
/// standard hasher.
fn short_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    h.write_u32(std::process::id());
    format!("{:06x}", h.finish() & 0xff_ffff)
}

/// Keeps the worktree directory out of `git status` without touching the
/// repo's own `.gitignore` — the same trick Claude Code uses for its folder.
fn exclude(folder: &str, pattern: &str) {
    let Ok(out) = git(folder).args(["rev-parse", "--git-common-dir"]).output() else { return };
    if !out.status.success() {
        return;
    }
    let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let dir = if Path::new(&dir).is_absolute() { PathBuf::from(dir) } else { Path::new(folder).join(dir) };
    let file = dir.join("info").join("exclude");
    let existing = std::fs::read_to_string(&file).unwrap_or_default();
    if existing.lines().any(|l| l.trim() == pattern) {
        return;
    }
    let _ = std::fs::create_dir_all(file.parent().unwrap());
    let mut text = existing;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(pattern);
    text.push('\n');
    let _ = std::fs::write(&file, text);
}

/// Makes a worktree for a Codex chat on a fresh branch off HEAD and returns
/// its path. Off the main thread: `worktree add` checks out a whole tree.
#[tauri::command]
pub async fn create_worktree(folder: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || create_worktree_now(&folder))
        .await
        .map_err(|e| e.to_string())?
}

fn create_worktree_now(folder: &str) -> Result<String, String> {
    let inside = git(folder)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if !inside.status.success() {
        return Err("not a git repository — turn off the worktree option for this folder".into());
    }
    let name = format!("codex-{}", short_id());
    let dir = worktrees_dir(folder, Provider::Codex);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(&name);
    let path_str = path.to_string_lossy().into_owned();
    let out = git(folder)
        .args(["worktree", "add", "-b", &name, &path_str])
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git worktree add failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    exclude(folder, ".codex/worktrees/");
    crate::log::info("worktree", &format!("created {path_str} on branch {name}"));
    Ok(path_str)
}

// Every git call below is slow enough to be felt — `worktree remove` on a large
// checkout has taken ten seconds here. Tauri runs a synchronous command on the
// main thread, where that time is charged to every other pending IPC call:
// pasting into a chat froze for as long as an unrelated chat's worktree took to
// delete. The commands are therefore `async` wrappers, which Tauri runs off the
// main thread, over the blocking helpers the rest of the crate keeps calling.
#[tauri::command]
pub async fn remove_worktree(folder: String, worktree_path: String) -> Result<(), String> {
    remove_worktree_now(folder, worktree_path)
}

pub fn remove_worktree_now(folder: String, worktree_path: String) -> Result<(), String> {
    if !is_worktree_of(&folder, &worktree_path) {
        return Err("refusing: path is not a worktree of this folder".into());
    }
    if !Path::new(&worktree_path).exists() {
        // Still worth pruning a stale admin entry and its branch.
        let branch = branch_of(&folder, &worktree_path);
        let _ = git(&folder).args(["worktree", "unlock", &worktree_path]).output();
        let _ = git(&folder).args(["worktree", "prune"]).output();
        drop_branch(&folder, branch);
        return Ok(());
    }

    // Resolve the branch before the worktree disappears from git's records.
    let branch = branch_of(&folder, &worktree_path);

    // The killed session may hold file locks for a moment — retry briefly.
    // Claude Code locks the worktree it makes ("claude session <name> (pid
    // N)") and leaves the lock behind when killed; one --force refuses a
    // locked worktree, the second overrides the lock.
    let mut removed = false;
    for _ in 0..3 {
        match git(&folder)
            .args(["worktree", "remove", "--force", "--force", &worktree_path])
            .output()
        {
            Ok(o) if o.status.success() => {
                removed = true;
                break;
            }
            _ => std::thread::sleep(std::time::Duration::from_millis(500)),
        }
    }
    if !removed {
        std::fs::remove_dir_all(&worktree_path).map_err(|e| e.to_string())?;
        // prune skips a locked entry, and the branch it keeps checked out
        // would then refuse `branch -D` below.
        let _ = git(&folder).args(["worktree", "unlock", &worktree_path]).output();
        let _ = git(&folder).args(["worktree", "prune"]).output();
    }

    crate::log::info("worktree", &format!("removed {worktree_path} (branch {branch:?})"));
    drop_branch(&folder, branch);
    Ok(())
}

/// Deletes the throwaway branch made alongside the directory. Restricted to
/// the two throwaway namings — Claude Code's `worktree-*` and Luna's
/// `codex-*`: anything else is a branch the user checked out themselves, and
/// `-D` would discard real commits.
fn drop_branch(folder: &str, branch: Option<String>) {
    let Some(branch) = branch else { return };
    if !branch.starts_with("worktree-") && !branch.starts_with("codex-") {
        return;
    }
    let _ = git(folder).args(["branch", "-D", &branch]).output();
}

/// A worktree younger than this is assumed to belong to a session that has not
/// reported its path yet, and is never treated as an orphan.
const GRACE_SECS: u64 = 300;

fn younger_than(path: &Path, secs: u64) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .and_then(|t| t.elapsed().map_err(|_| std::io::ErrorKind::Other.into()))
        .map(|age| age.as_secs() < secs)
        .unwrap_or(true) // unreadable mtime → treat as fresh, i.e. leave it alone
}

/// Worktree directories under this folder that no live chat claims — leftovers
/// from crashes, from chats deleted before their path was known, and from
/// chats deleted with their worktree kept. Cheap on purpose: it runs every
/// minute per folder, so it reads directories only; what each one holds is
/// `inspect_worktrees`' job, asked for when someone means to delete.
#[tauri::command]
pub async fn orphan_worktrees(
    state: crate::pty::PtyState<'_>,
    folder: String,
    in_use: Vec<String>,
    account_paths: Vec<String>,
) -> Result<Vec<String>, String> {
    let live = crate::pty::live_cwds(&state);
    tauri::async_runtime::spawn_blocking(move || orphan_worktrees_now(folder, in_use, account_paths, live))
        .await
        .map_err(|e| e.to_string())?
}

/// True when the (normalised) `cwd` is the worktree or anywhere inside it.
fn claims(cwd: &str, worktree: &str) -> bool {
    cwd == worktree || cwd.strip_prefix(worktree).is_some_and(|rest| rest.starts_with('\\'))
}

/// The user's own Claude Code config, for sessions started from a terminal
/// rather than from Luna: they sit in worktrees too, and Luna's accounts know
/// nothing of them.
fn own_claude_dir() -> Option<String> {
    std::env::var("CLAUDE_CONFIG_DIR")
        .ok()
        .filter(|d| !d.is_empty())
        .or_else(|| std::env::var("USERPROFILE").ok().map(|h| format!("{h}\\.claude")))
}

fn orphan_worktrees_now(
    folder: String,
    in_use: Vec<String>,
    account_paths: Vec<String>,
    live_pty_cwds: Vec<String>,
) -> Result<Vec<String>, String> {
    let mut claimed: Vec<String> = in_use.iter().map(|p| norm(p)).collect();
    // Claude Code moves into its worktree on its own; only its registry says
    // where. Codex sessions run where Luna put them.
    for account in account_paths.iter().cloned().chain(own_claude_dir()) {
        claimed.extend(crate::claude::session::live_cwds(&account).iter().map(|p| norm(p)));
    }
    claimed.extend(live_pty_cwds.iter().map(|p| norm(p)));

    let mut out = vec![];
    for provider in Provider::ALL {
        let dir = worktrees_dir(&folder, provider);
        if !dir.exists() {
            continue;
        }
        for entry in std::fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten() {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let path = entry.path();
            if younger_than(&path, GRACE_SECS) {
                continue;
            }
            let path = path.to_string_lossy().into_owned();
            let key = norm(&path);
            // A session sitting in a subfolder of the worktree is still using it.
            if !claimed.iter().any(|c| claims(c, &key)) {
                out.push(path);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// What deleting a worktree would throw away.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeInfo {
    pub path: String,
    pub branch: Option<String>,
    /// Entries `git status` lists: modified, staged, untracked.
    pub uncommitted: Option<u32>,
    /// Commits on the worktree's HEAD that no other branch, local or remote,
    /// has — the ones `branch -D` would lose for good.
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

fn inspect(folder: &str, path: &str) -> WorktreeInfo {
    let branch = branch_of(folder, path);
    let wt = Path::new(path);
    // A linked worktree's `.git` is a file naming its admin dir; the index
    // lives there and moves with every add, commit and checkout.
    let index = std::fs::read_to_string(wt.join(".git"))
        .ok()
        .and_then(|t| t.trim().strip_prefix("gitdir:").map(|g| PathBuf::from(g.trim()).join("index")));
    let touched_ms = [mtime_ms(wt), index.as_deref().and_then(mtime_ms)].into_iter().flatten().max();

    // Without its own `.git`, git would walk up and answer for the main
    // checkout instead.
    let status = wt
        .join(".git")
        .exists()
        .then(|| git(path).args(["status", "--porcelain", "--untracked-files=normal"]).output().ok())
        .flatten()
        .filter(|o| o.status.success());
    let Some(status) = status else {
        return WorktreeInfo { path: path.to_string(), branch, uncommitted: None, unique_commits: None, touched_ms, broken: true };
    };
    let uncommitted = String::from_utf8_lossy(&status.stdout).lines().filter(|l| !l.is_empty()).count() as u32;

    let mut rev = git(path);
    rev.args(["rev-list", "--count", "HEAD", "--not"]);
    if let Some(b) = &branch {
        // Or the branch itself counts as another place the commits live.
        rev.arg(format!("--exclude={b}"));
    }
    rev.args(["--branches", "--remotes"]);
    let unique_commits = rev
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok());

    WorktreeInfo { path: path.to_string(), branch, uncommitted: Some(uncommitted), unique_commits, touched_ms, broken: false }
}

/// Looks inside each worktree before it is offered for deletion. One git
/// status per worktree, side by side: a large checkout takes a second or two.
#[tauri::command]
pub async fn inspect_worktrees(folder: String, paths: Vec<String>) -> Result<Vec<WorktreeInfo>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        std::thread::scope(|scope| {
            let jobs: Vec<_> = paths
                .iter()
                .filter(|p| is_worktree_of(&folder, p))
                .map(|p| scope.spawn(|| inspect(&folder, p)))
                .collect();
            jobs.into_iter().filter_map(|j| j.join().ok()).collect()
        })
    })
    .await
    .map_err(|e| e.to_string())
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SweepFailure {
    pub path: String,
    pub error: String,
}

/// What a sweep did, path by path.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SweepResult {
    pub removed: Vec<String>,
    pub failed: Vec<SweepFailure>,
}

/// Removes the worktrees the user picked, and of those only the ones that are
/// still orphans: the list is re-derived here rather than trusted from the UI,
/// which may be minutes stale by the time the dialog is confirmed.
#[tauri::command]
pub async fn remove_orphan_worktrees(
    state: crate::pty::PtyState<'_>,
    folder: String,
    in_use: Vec<String>,
    account_paths: Vec<String>,
    paths: Vec<String>,
) -> Result<SweepResult, String> {
    let live = crate::pty::live_cwds(&state);
    tauri::async_runtime::spawn_blocking(move || {
        let orphans: Vec<String> =
            orphan_worktrees_now(folder.clone(), in_use, account_paths, live)?.iter().map(|p| norm(p)).collect();
        let mut result = SweepResult { removed: vec![], failed: vec![] };
        for path in paths {
            if !orphans.contains(&norm(&path)) {
                result.failed.push(SweepFailure { path, error: "in use again — left alone".into() });
                continue;
            }
            match remove_worktree_now(folder.clone(), path.clone()) {
                Ok(()) => result.removed.push(path),
                Err(error) => {
                    crate::log::warn("worktree", &format!("could not remove {path}: {error}"));
                    result.failed.push(SweepFailure { path, error });
                }
            }
        }
        Ok(result)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_worktree_path_names_its_project() {
        assert_eq!(project_of(r"E:\p\x\.codex\worktrees\codex-1a2b3c").as_deref(), Some(r"E:\p\x"));
        assert_eq!(project_of("E:/p/x/.claude/worktrees/a-b-c").as_deref(), Some(r"E:\p\x"));
        assert_eq!(project_of(r"E:\p\x"), None);
        let wt = r"E:\P\X\.Codex\Worktrees\codex-1";
        assert!(is_worktree_of(&project_of(wt).unwrap(), wt));
    }

    use super::*;

    #[test]
    fn worktree_membership_covers_both_providers() {
        assert!(is_worktree_of(r"E:\p", r"E:\p\.claude\worktrees\x"));
        assert!(is_worktree_of(r"E:\p", r"e:/p/.codex/worktrees/codex-abc"));
        assert!(!is_worktree_of(r"E:\p", r"E:\p\.codex\worktrees"));
        assert!(!is_worktree_of(r"E:\p", r"E:\q\.codex\worktrees\x"));
    }

    #[test]
    fn a_session_in_a_subfolder_claims_the_worktree() {
        let wt = norm(r"E:\p\.claude\worktrees\a");
        assert!(claims(&norm(r"e:/p/.claude/worktrees/a"), &wt));
        assert!(claims(&norm(r"E:\p\.claude\worktrees\a\src"), &wt));
        assert!(!claims(&norm(r"E:\p\.claude\worktrees\ab"), &wt));
        assert!(!claims(&norm(r"E:\p"), &wt));
    }

    #[test]
    fn short_ids_are_six_hex() {
        let id = short_id();
        assert_eq!(id.len(), 6);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
