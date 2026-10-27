//! Git worktrees for isolated chats. Each CLI makes its own from
//! `--worktree`: Claude Code under `<folder>/.claude/worktrees/<name>` on a
//! branch `worktree-<name>`, Codex under `<CODEX_HOME>/worktrees/<4 hex>/<repo>`
//! on a detached HEAD. Luna only finds them again — to remove one with its
//! chat, or sweep the ones no chat claims — and never makes one itself.

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

fn claude_worktrees(folder: &str) -> PathBuf {
    Path::new(folder).join(".claude").join("worktrees")
}

fn under_claude_worktrees(folder: &str, candidate: &str) -> bool {
    let (c, base) = (norm(candidate), norm(&claude_worktrees(folder).to_string_lossy()));
    c.len() > base.len() && c.starts_with(&base)
}

/// Shaped like a worktree Codex made: `…\worktrees\<4 hex>\<repo>`.
pub fn codex_shaped(path: &str) -> bool {
    let p = Path::new(path.trim_end_matches(['\\', '/']));
    let bucket = p.parent();
    let root = bucket.and_then(Path::parent);
    let name = |q: Option<&Path>| q.and_then(Path::file_name).map(|n| n.to_string_lossy().into_owned());
    p.file_name().is_some()
        && name(bucket).is_some_and(|b| b.len() == 4 && b.chars().all(|c| c.is_ascii_hexdigit()))
        && name(root).is_some_and(|r| r.eq_ignore_ascii_case("worktrees"))
}

/// The repository a linked worktree belongs to, off its `.git` file
/// (`gitdir: <repo>/.git/worktrees/<id>`). None when the worktree is gone or
/// is not a linked one.
fn repo_of_linked(worktree: &str) -> Option<String> {
    let text = std::fs::read_to_string(Path::new(worktree).join(".git")).ok()?;
    let gitdir = PathBuf::from(text.trim().strip_prefix("gitdir:")?.trim());
    let gitdir = if gitdir.is_absolute() { gitdir } else { Path::new(worktree).join(gitdir) };
    // <repo>/.git/worktrees/<id> → <repo>
    let dot_git = gitdir.parent()?.parent()?;
    if dot_git.file_name()? != ".git" {
        return None;
    }
    dot_git.parent().map(|r| r.to_string_lossy().into_owned())
}

/// True when `candidate` is a worktree a CLI made for this folder: under its
/// `.claude/worktrees`, or one Codex made whose checkout points back at it.
/// Every destructive path below is gated on this.
pub fn is_worktree_of(folder: &str, candidate: &str) -> bool {
    under_claude_worktrees(folder, candidate)
        || (codex_shaped(candidate) && repo_of_linked(candidate).is_some_and(|r| norm(&r) == norm(folder)))
}

/// The project folder a CLI-made worktree belongs to: for Claude Code the part
/// of the path before `.claude\worktrees`, for Codex the repository its
/// checkout points back at.
pub fn project_of(path: &str) -> Option<String> {
    let p = path.replace('/', "\\");
    if let Some(at) = p.to_ascii_lowercase().find("\\.claude\\worktrees\\").filter(|&at| at > 0) {
        return Some(p[..at].to_string());
    }
    codex_shaped(path).then(|| repo_of_linked(path)).flatten()
}

/// Branch checked out in the given worktree, per `git worktree list`.
pub fn branch_of(folder: &str, worktree_path: &str) -> Option<String> {
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

/// origin's default branch as this clone last heard of it (`origin/main`),
/// if it has an origin that says.
fn origin_default(folder: &str) -> Option<String> {
    let out = git(folder).args(["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"]).output().ok()?;
    let r = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && r.starts_with("origin/")).then_some(r)
}

/// The longest a fetch before a worktree may hold up the chat it is for.
const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Brings origin's default branch up to date before Claude Code makes a
/// worktree off it. Claude Code bases its worktree on that branch but fetches it only when
/// the last fetch is a day old, so a repo fetched in the morning gave every
/// worktree that day the morning's tree. Only remote-tracking refs move.
/// Never interactive and bounded: a fetch that wants a password, or a network
/// that is not there, is given up on and the chat starts on what there is.
pub fn fetch_origin(folder: &str) {
    if !crate::settings::get().fetch_before_worktree
        || !git(folder).args(["remote", "get-url", "origin"]).output().is_ok_and(|o| o.status.success())
    {
        return;
    }
    // One at a time: a new chat's pane and its spawn can both get here, and
    // two fetches into one repo fail on each other's ref lock. The second
    // finds nothing left to fetch.
    static FETCHING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = FETCHING.lock().unwrap_or_else(|e| e.into_inner());
    let mut cmd = git(folder);
    cmd.args(["fetch", "--quiet", "origin"]);
    if let Some(branch) = origin_default(folder) {
        cmd.arg(&branch["origin/".len()..]);
    }
    cmd.env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    let Ok(mut child) = cmd.spawn() else { return };
    let deadline = std::time::Instant::now() + FETCH_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    let mut why = String::new();
                    if let Some(mut err) = child.stderr.take() {
                        let _ = std::io::Read::read_to_string(&mut err, &mut why);
                    }
                    crate::log::warn(
                        "worktree",
                        &format!("fetch before a worktree in {folder} failed ({status}): {}; using origin as it was", why.trim()),
                    );
                }
                return;
            }
            Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(std::time::Duration::from_millis(100)),
            _ => {
                let _ = child.kill();
                crate::log::warn("worktree", &format!("fetch before a worktree in {folder} took over {FETCH_TIMEOUT:?}; using origin as it was"));
                return;
            }
        }
    }
}

#[tauri::command]
pub fn fetch_before_worktree() -> bool {
    crate::settings::get().fetch_before_worktree
}

#[tauri::command]
pub fn set_fetch_before_worktree(on: bool) -> Result<bool, String> {
    crate::settings::update(|s| s.fetch_before_worktree = on)?;
    Ok(on)
}

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
        if !under_claude_worktrees(&folder, &worktree_path) && !codex_shaped(&worktree_path) {
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

    // Resolve the branch before the worktree disappears from git's records.
    let branch = branch_of(&folder, &worktree_path);

    // The killed session's processes — the CLI, and whatever shell or build it
    // had running — let go of their files over a few seconds, not at once:
    // three tries half a second apart lost that race to an open handle
    // ("os error 32"). Back off for up to ~8s in all. Claude Code locks the
    // worktree it makes and leaves the lock behind when killed; one --force
    // refuses a locked worktree, the second overrides the lock.
    let mut removed = false;
    let mut why = String::new();
    for attempt in 1..=6u64 {
        match git(&folder)
            .args(["worktree", "remove", "--force", "--force", &worktree_path])
            .output()
        {
            Ok(o) if o.status.success() => {
                removed = true;
                break;
            }
            Ok(o) => why = String::from_utf8_lossy(&o.stderr).trim().to_string(),
            Err(e) => why = e.to_string(),
        }
        std::thread::sleep(std::time::Duration::from_millis(300 * attempt));
    }
    if !removed {
        let mut gone = std::fs::remove_dir_all(&worktree_path);
        for _ in 0..3 {
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

    crate::log::info("worktree", &format!("removed {worktree_path} (branch {branch:?})"));
    drop_branches(&folder, &worktree_path, branch);
    // Codex keeps each checkout in a bucket of its own, which git leaves
    // behind empty.
    if codex_shaped(&worktree_path) {
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
fn made_branch(worktree_path: &str) -> Option<String> {
    if codex_shaped(worktree_path) {
        return None;
    }
    let dir = Path::new(worktree_path).file_name()?.to_str()?;
    Some(format!("worktree-{dir}"))
}

/// Deletes the throwaway branches of a removed worktree: the one it was made
/// on, and the one checked out at the end if that is throwaway too.
/// Restricted to Claude Code's `worktree-*` naming: anything else is a branch
/// someone made on purpose, and `-D` would discard real commits.
fn drop_branches(folder: &str, worktree_path: &str, checked_out: Option<String>) {
    let mut branches: Vec<String> = made_branch(worktree_path).into_iter().chain(checked_out).collect();
    branches.dedup();
    for branch in branches {
        if branch.starts_with("worktree-") {
            let _ = git(folder).args(["branch", "-D", &branch]).output();
        }
    }
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
    folder: String,
    in_use: Vec<String>,
    account_paths: Vec<String>,
) -> Result<Vec<String>, String> {
    let live = crate::pty::live_cwds();
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
    // where. Where a Codex session moved, its rollout says (`pty::live_cwds`).
    for account in account_paths.iter().cloned().chain(own_claude_dir()) {
        claimed.extend(crate::claude::session::live_cwds(&account).iter().map(|p| norm(p)));
    }
    claimed.extend(live_pty_cwds.iter().map(|p| norm(p)));

    let mut out = vec![];
    for path in candidates(&folder, &account_paths) {
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
    out.sort();
    Ok(out)
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|r| r.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.path()).collect())
        .unwrap_or_default()
}

/// Every worktree a CLI made for this folder: Claude Code's under the folder
/// itself, Codex's under each account that has any, kept when their checkout
/// points back at the folder.
fn candidates(folder: &str, account_paths: &[String]) -> Vec<PathBuf> {
    let mut out = subdirs(&claude_worktrees(folder));
    for account in account_paths {
        for bucket in subdirs(&Path::new(account).join("worktrees")) {
            out.extend(subdirs(&bucket).into_iter().filter(|wt| is_worktree_of(folder, &wt.to_string_lossy())));
        }
    }
    out
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
    folder: String,
    in_use: Vec<String>,
    account_paths: Vec<String>,
    paths: Vec<String>,
) -> Result<SweepResult, String> {
    let live = crate::pty::live_cwds();
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
    use super::*;

    /// A repository with one linked worktree where Codex puts them, under a
    /// fake account: (root, repo, worktree). None where there is no git.
    fn codex_layout(tag: &str) -> Option<(PathBuf, String, String)> {
        let root = std::env::temp_dir().join(format!("luna-wt-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
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

    #[test]
    fn a_worktree_path_names_its_project() {
        assert_eq!(project_of("E:/p/x/.claude/worktrees/a-b-c").as_deref(), Some(r"E:\p\x"));
        assert_eq!(project_of(r"E:\p\x"), None);
        let Some((root, repo, wt)) = codex_layout("project") else { return };
        assert_eq!(project_of(&wt).map(|p| norm(&p)), Some(norm(&repo)));
        assert!(is_worktree_of(&repo, &wt));
        assert!(!is_worktree_of(&root.to_string_lossy(), &wt), "another folder's");
        let account = root.join("account").to_string_lossy().into_owned();
        assert_eq!(candidates(&repo, &[account]).len(), 1);
        remove_worktree_now(repo.clone(), wt.clone()).unwrap();
        assert!(!root.join("account").join("worktrees").join("0a1b").exists(), "the bucket goes too");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn codex_worktrees_have_a_shape() {
        assert!(codex_shaped(r"E:\acc\worktrees\d6ff\repo"));
        assert!(codex_shaped("/home/u/.codex/worktrees/0a1B/app/"));
        assert!(!codex_shaped(r"E:\acc\worktrees\d6ff"));
        assert!(!codex_shaped(r"E:\acc\worktrees\codex-1\repo"));
        assert!(!codex_shaped(r"E:\p\.codex\worktrees\codex-1a2b3c"));
    }

    #[test]
    fn claude_worktree_membership() {
        assert!(is_worktree_of(r"E:\p", r"E:\p\.claude\worktrees\x"));
        assert!(is_worktree_of(r"E:\p", "e:/p/.claude/worktrees/x"));
        assert!(!is_worktree_of(r"E:\p", r"E:\p\.claude\worktrees"));
        assert!(!is_worktree_of(r"E:\p", r"E:\q\.claude\worktrees\x"));
        assert!(!is_worktree_of(r"E:\p", r"E:\p\.codex\worktrees\codex-abc"));
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
    fn a_worktree_names_the_branch_it_was_made_on() {
        assert_eq!(made_branch(r"E:\p\.claude\worktrees\merry-coalescing-cosmos").as_deref(), Some("worktree-merry-coalescing-cosmos"));
        assert_eq!(made_branch(r"E:\acc\worktrees\d6ff\repo"), None);
    }
}
