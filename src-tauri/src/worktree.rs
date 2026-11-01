//! Git worktrees for isolated chats. Each CLI makes its own from
//! `--worktree`: Claude Code under `<folder>/.claude/worktrees/<name>` on a
//! branch `worktree-<name>`, Codex under `<CODEX_HOME>/worktrees/<4 hex>/<repo>`
//! on a detached HEAD. Luna only finds them again — to remove one with its
//! chat, or sweep the ones no chat claims — and never makes one itself.

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

fn claude_worktrees(folder: &str) -> PathBuf {
    Path::new(folder).join(".claude").join("worktrees")
}

/// Whether the path has a `.` or `..` in it: the text it reads as is not
/// where it leads, and nothing is deleted by such a path.
fn has_dot_parts(path: &str) -> bool {
    path.split(['\\', '/']).any(|c| c == "." || c == "..")
}

fn under_claude_worktrees(folder: &str, candidate: &str) -> bool {
    if has_dot_parts(candidate) {
        return false;
    }
    let (c, base) = (norm(candidate), norm(&claude_worktrees(folder).to_string_lossy()));
    // One folder past a separator, as Claude Code names them:
    // `.claude\worktrees-old\x` is no worktree of the folder, and
    // `.claude\worktrees\x\src` only a part of one.
    c.strip_prefix(&base)
        .and_then(|rest| rest.strip_prefix('\\'))
        .is_some_and(|name| !name.is_empty() && !name.contains('\\'))
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
    // <repo>/.git/worktrees/<id> → <repo>. A submodule's points at `.git/modules`.
    let worktrees = gitdir.parent()?;
    let dot_git = worktrees.parent()?;
    if worktrees.file_name()? != "worktrees" || dot_git.file_name()? != ".git" {
        return None;
    }
    dot_git.parent().map(|r| r.to_string_lossy().into_owned())
}

/// True when `candidate` is a worktree a CLI made for this folder: under its
/// `.claude/worktrees`, or one Codex made whose checkout points back at it.
/// Every destructive path below is gated on this.
pub fn is_worktree_of(folder: &str, candidate: &str) -> bool {
    under_claude_worktrees(folder, candidate)
        || (!has_dot_parts(candidate)
            && codex_shaped(candidate)
            && repo_of_linked(candidate).is_some_and(|r| norm(&r) == norm(folder)))
}

/// The project folder a CLI-made worktree belongs to, given that worktree or a
/// folder of its checkout: for Claude Code the part of the path before
/// `.claude\worktrees`, for Codex the repository its checkout points back at. The
/// shape of a path only says where to look: its `.git` file says whether that is a
/// worktree of the repository. Taken by shape alone, any folder under a worktree (a
/// test project in its `test-build`) read as the main checkout, and a helper sent there
/// with a worktree of its own made it in that checkout. One read of a file, and git
/// only for a folder below a worktree's root, which nothing asks after on a beat.
pub fn project_of(path: &str) -> Option<String> {
    const MARK: &str = "\\.claude\\worktrees\\";
    // Found in a backslashed copy, cut from one that keeps the platform's own
    // separators: `/home/u/p` must not come back as `\home\u\p`. Both have the
    // same byte offsets.
    let p = if cfg!(windows) { path.replace('/', "\\") } else { path.to_string() };
    let low = p.replace('/', "\\").to_ascii_lowercase();
    let marks: Vec<usize> = low.match_indices(MARK).map(|(at, _)| at).filter(|&at| at > 0).collect();
    // The innermost first: a project can lie inside a worktree of another.
    for &at in marks.iter().rev() {
        let project = &p[..at];
        let below = &p[at + MARK.len()..];
        let name = below.find(['\\', '/']).unwrap_or(below.len());
        if name == 0 {
            continue;
        }
        let root = &p[..at + MARK.len() + name];
        if !Path::new(root).exists() {
            // Gone: Claude Code removes a clean one as its session quits, and makes
            // it again by name. Where it was is all there is to go by, in a repository
            // or somewhere itself gone.
            if !Path::new(project).exists() || Path::new(project).join(".git").exists() {
                return Some(project.to_string());
            }
            continue;
        }
        let Some(repo) = repo_of_linked(root).filter(|r| Path::new(r).is_dir()) else { continue };
        if !in_checkout(root, below[name..].trim_matches(['\\', '/'])) {
            break;
        }
        return Some(if same_place(&repo, project) { project.to_string() } else { repo });
    }
    codex_shaped(path).then(|| repo_of_linked(path)).flatten()
}

/// Whether a folder below a worktree's root (`rest`, empty for the root) is part of
/// its checkout: no repository of its own on the way down, and nothing git ignores,
/// as build output and test folders are.
fn in_checkout(root: &str, rest: &str) -> bool {
    if rest.is_empty() {
        return true;
    }
    let mut dir = PathBuf::from(root);
    for part in rest.split(['\\', '/']).filter(|c| !c.is_empty()) {
        dir.push(part);
        if dir.join(".git").exists() {
            return false;
        }
    }
    // 0: ignored, 1: not. Anything else says nothing, and it goes as its root does.
    !git(root).args(["check-ignore", "-q", "--", &rest.replace('\\', "/")]).output().is_ok_and(|o| o.status.code() == Some(0))
}

/// Whether two paths lead to one folder, however each is written.
fn same_place(a: &str, b: &str) -> bool {
    norm(a) == norm(b) || matches!((std::fs::canonicalize(a), std::fs::canonicalize(b)), (Ok(x), Ok(y)) if x == y)
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
    let branch = origin_default(folder);
    let deadline = std::time::Instant::now() + FETCH_TIMEOUT;
    for attempt in 1..=2 {
        let mut cmd = git(folder);
        cmd.args(["fetch", "--quiet", "origin"]);
        if let Some(branch) = &branch {
            cmd.arg(&branch["origin/".len()..]);
        }
        cmd.env("GIT_TERMINAL_PROMPT", "0")
            .env("GCM_INTERACTIVE", "never")
            // Its message is read below.
            .env("LC_ALL", "C")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped());
        let Ok(mut child) = cmd.spawn() else { return };
        let (status, why) = loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => return,
                Ok(Some(status)) => {
                    let mut why = String::new();
                    if let Some(mut err) = child.stderr.take() {
                        let _ = std::io::Read::read_to_string(&mut err, &mut why);
                    }
                    break (status, why);
                }
                Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(std::time::Duration::from_millis(100)),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    crate::log::warn("worktree", &format!("fetch before a worktree in {folder} took over {FETCH_TIMEOUT:?}; using origin as it was"));
                    return;
                }
            }
        };
        // Another git updating the same ref at that moment — an editor's own
        // fetch, Claude Code's — is done with it in moments, and a fetch
        // after it is all this needs.
        let retry = std::time::Duration::from_millis(500);
        if attempt == 1 && why.contains("cannot lock ref") && std::time::Instant::now() + retry < deadline {
            std::thread::sleep(retry);
            continue;
        }
        crate::log::warn(
            "worktree",
            &format!("fetch before a worktree in {folder} failed ({status}): {}; using origin as it was", why.trim()),
        );
        return;
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

/// The throwaway branches of a worktree, which go with it: the one it was made
/// on, and the one checked out at the end if that is throwaway too.
/// Restricted to Claude Code's `worktree-*` naming: anything else is a branch
/// someone made on purpose, and `-D` would discard real commits.
fn throwaway_branches(worktree_path: &str, checked_out: Option<&str>) -> Vec<String> {
    let mut branches: Vec<String> = made_branch(worktree_path).into_iter().chain(checked_out.map(str::to_owned)).filter(|b| b.starts_with("worktree-")).collect();
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
        .or_else(|| dirs::home_dir().map(|h| h.join(".claude").to_string_lossy().into_owned()))
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

/// The worktrees a CLI has made for this folder on this account.
pub fn made_for(folder: &str, account_path: &str) -> Vec<String> {
    candidates(folder, &[account_path.to_string()]).into_iter().map(|p| p.to_string_lossy().into_owned()).collect()
}

/// Those of `now` not in `before` and with no session in them: what a spawn cut
/// off by a core going had begun (agents.rs `worktrees_left`).
pub fn left_since(now: Vec<String>, before: &[String], live_cwds: &[String]) -> Vec<String> {
    let claimed: Vec<String> = live_cwds.iter().map(|p| norm(p)).collect();
    now.into_iter()
        .filter(|p| !before.iter().any(|b| norm(b) == norm(p)))
        .filter(|p| !claimed.iter().any(|c| claims(c, &norm(p))))
        .collect()
}

/// Whether a worktree a CLI left is as a CLI would make it now: readable as a
/// checkout, on the branch it was made on (Codex's: on none), with nothing that
/// removing it would lose. Starting there is no different from starting in a new one.
fn as_good_as_new(folder: &str, path: &str) -> bool {
    let on_its_own = branch_of(folder, path) == made_branch(path);
    on_its_own && Path::new(path).join(".git").is_file() && unsaved_work(folder, path).is_none()
}

/// Where a session of this account's runs by Claude Code's own registry, and a
/// person's own sessions from a terminal. Only those with a process still there: a
/// CLI that went with a core leaves its entry behind, and the worktree it names is
/// the very one to go on in.
pub fn claude_sessions_running(account_path: &str) -> Vec<String> {
    [Some(account_path.to_string()), own_claude_dir()].into_iter().flatten().flat_map(|dir| crate::claude::session::running_cwds(&dir)).collect()
}

/// What a helper cut off before its CLI said where it works may have begun: the
/// worktrees of its own CLI's kind new since it was spawned (`before`) that nobody
/// claims. Those as good as new, then those holding something. Which of the first
/// was whose does not matter: nothing in one tells it from the next.
pub fn half_made(provider: Provider, folder: &str, account_path: &str, before: &[String], claimed: &[String]) -> (Vec<String>, Vec<String>) {
    let mut left: Vec<String> = left_since(made_for(folder, account_path), before, claimed)
        .into_iter()
        .filter(|p| match provider {
            Provider::Claude => under_claude_worktrees(folder, p),
            Provider::Codex => codex_shaped(p),
        })
        .collect();
    left.sort();
    left.into_iter().partition(|p| as_good_as_new(folder, p))
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
        .then(|| git(path).args(["-c", "core.quotepath=off", "status", "--porcelain", "--untracked-files=normal", "--ignore-submodules=none"]).output().ok())
        .flatten()
        .filter(|o| o.status.success())?;
    Some(String::from_utf8_lossy(&status.stdout).lines().filter(|l| !l.is_empty()).map(str::to_owned).collect())
}

/// How many commits HEAD has that no other branch and no remote has.
fn unique_commits(path: &str, branch: Option<&str>) -> Option<u32> {
    let mut rev = git(path);
    rev.args(["rev-list", "--count", "HEAD", "--not"]);
    if let Some(b) = branch {
        // Or the branch itself counts as another place the commits live.
        rev.arg(format!("--exclude={b}"));
    }
    rev.args(["--branches", "--remotes"]);
    rev.output().ok().filter(|o| o.status.success()).and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
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
            let what = if changes.len() == 1 { "1 uncommitted change".to_string() } else { format!("{} uncommitted changes", changes.len()) };
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
    let unique_commits = unique_commits(path, branch.as_deref());

    WorktreeInfo { path: path.to_string(), branch, uncommitted: Some(changes.len() as u32), unique_commits, touched_ms, broken: false }
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
    fn what_a_spawn_left_is_what_is_new_with_nobody_in_it() {
        let now = vec![r"E:\p\.claude\worktrees\old".to_string(), r"E:\p\.claude\worktrees\new".to_string(), r"E:\p\.claude\worktrees\busy".to_string()];
        let before = ["e:/p/.claude/worktrees/old".to_string()];
        let live = [r"E:\p\.claude\worktrees\busy\sub".to_string()];
        assert_eq!(left_since(now, &before, &live), [r"E:\p\.claude\worktrees\new"]);
    }

    #[test]
    fn what_a_cut_off_start_left_is_gone_on_in_only_as_good_as_new() {
        let Some((root, repo, codex)) = codex_layout("half-made") else { return };
        let account = root.join("account").to_string_lossy().into_owned();
        let ok = |c: &mut Command| c.output().is_ok_and(|o| o.status.success());
        let user = ["-c", "user.email=t@t", "-c", "user.name=t"];
        let claude = |name: &str| {
            let wt = root.join("repo").join(".claude").join("worktrees").join(name).to_string_lossy().into_owned();
            assert!(ok(git(&repo).args(["worktree", "add", "-q", "-b", &format!("worktree-{name}"), &wt])));
            wt
        };
        let (fresh, dirty, moved, committed) = (claude("fresh"), claude("dirty"), claude("moved"), claude("committed"));
        std::fs::write(Path::new(&dirty).join("half-done.txt"), "work").unwrap();
        assert!(ok(git(&moved).args(["switch", "-q", "-c", "its-own"])));
        assert!(ok(git(&committed).args(user).args(["commit", "-q", "--allow-empty", "-m", "work"])));
        let (good, holding) = half_made(Provider::Claude, &repo, &account, &[], &[]);
        assert_eq!(good, [fresh.clone()], "clean, on the branch it was made on, nothing of its own");
        assert_eq!(holding, [committed.clone(), dirty.clone(), moved.clone()], "named, never gone on in");

        // Each CLI goes on in what is its own kind: Codex's has no branch to be on.
        assert_eq!(half_made(Provider::Codex, &repo, &account, &[], &[]), (vec![codex.clone()], Vec::new()));
        // What was there as it was spawned, or anybody's, is nothing it began.
        assert!(half_made(Provider::Claude, &repo, &account, &[fresh.clone()], &[]).0.is_empty());
        assert!(half_made(Provider::Claude, &repo, &account, &[], &[fresh.clone()]).0.is_empty());
        assert!(half_made(Provider::Claude, &repo, &account, &[], &[format!("{fresh}{}src", std::path::MAIN_SEPARATOR)]).0.is_empty(), "a session in a subfolder of it");

        // A checkout cut short reads as changes; one with no `.git` as no checkout.
        std::fs::remove_file(Path::new(&codex).join(".git")).unwrap();
        assert!(!as_good_as_new(&repo, &codex));
        for wt in [&dirty, &moved, &committed, &fresh] {
            let _ = git(&repo).args(["worktree", "remove", "-f", "-f", wt]).output();
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_worktree_a_cli_made_is_found_for_its_folder_and_account() {
        let Some((root, repo, wt)) = codex_layout("made-for") else { return };
        let account = root.join("account").to_string_lossy().into_owned();
        assert_eq!(made_for(&repo, &account), [wt.clone()]);
        assert!(made_for(&repo, r"C:\no\such\account").is_empty());
        let _ = std::fs::remove_dir_all(&root);
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
    fn a_folder_is_of_a_worktree_only_if_git_says_so_whatever_its_path_looks_like() {
        let root = std::env::temp_dir().join(format!("luna-wt-of-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let ok = |dir: &Path, args: &[&str]| git(&s(dir)).args(["-c", "user.email=t@t", "-c", "user.name=t"]).args(args).output().is_ok_and(|o| o.status.success());
        if !ok(&repo, &["init", "-q"]) {
            return;
        }
        std::fs::write(repo.join(".gitignore"), "test-build/\n").unwrap();
        assert!(ok(&repo, &["add", "-A"]) && ok(&repo, &["commit", "-q", "-m", "i"]));
        let of = |p: &Path| project_of(&s(p)).map(|found| norm(&found));
        let is_repo = Some(norm(&s(&repo)));
        let worktrees = repo.join(".claude").join("worktrees");

        // A real one, and a folder of its checkout, tracked or not, there yet or not.
        let wt = worktrees.join("real");
        assert!(ok(&repo, &["worktree", "add", "-q", "-b", "worktree-real", &s(&wt)]));
        assert_eq!(of(&wt), is_repo);
        std::fs::create_dir_all(wt.join("src")).unwrap();
        assert_eq!(of(&wt.join("src")), is_repo);
        assert_eq!(of(&wt.join("src").join("not-yet")), is_repo);

        // What its git ignores is no part of it: the test folder this is about.
        let plain = wt.join("test-build").join("harness").join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        assert_eq!(of(&plain), None, "a plain folder under <wt>\\test-build");
        assert_eq!(of(&wt.join("test-build")), None);

        // A repository of its own there, and what that one's CLIs make, are its own.
        let own = wt.join("test-build").join("own");
        std::fs::create_dir_all(&own).unwrap();
        assert!(ok(&own, &["init", "-q"]) && ok(&own, &["commit", "-q", "--allow-empty", "-m", "i"]));
        assert_eq!(of(&own), None);
        std::fs::create_dir_all(own.join("deep")).unwrap();
        assert_eq!(of(&own.join("deep")), None);
        let own_wt = own.join(".claude").join("worktrees").join("its");
        assert!(ok(&own, &["worktree", "add", "-q", "-b", "worktree-its", &s(&own_wt)]));
        assert_eq!(of(&own_wt), Some(norm(&s(&own))));
        let codex = wt.join("test-build").join("account").join("worktrees").join("0a1b").join("own");
        std::fs::create_dir_all(codex.parent().unwrap()).unwrap();
        assert!(ok(&own, &["worktree", "add", "-q", "--detach", &s(&codex)]));
        assert_eq!(of(&codex), Some(norm(&s(&own))), "Codex's, in an account under a worktree");

        // A worktree made from inside a worktree is of the repository.
        let nested = wt.join(".claude").join("worktrees").join("nested");
        assert!(ok(&wt, &["worktree", "add", "-q", "-b", "worktree-nested", &s(&nested)]));
        assert_eq!(of(&nested), is_repo);

        // In the right place and no worktree: a plain folder, a clone of its own, a
        // submodule's `.git` file, one that points at another repository's.
        let bare = worktrees.join("plain");
        std::fs::create_dir_all(&bare).unwrap();
        assert_eq!(of(&bare), None);
        let clone = worktrees.join("clone");
        std::fs::create_dir_all(&clone).unwrap();
        assert!(ok(&clone, &["init", "-q"]));
        assert_eq!(of(&clone), None);
        let sub = worktrees.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join(".git"), format!("gitdir: {}", s(&repo.join(".git").join("modules").join("sub")))).unwrap();
        assert_eq!(of(&sub), None);
        let moved = worktrees.join("moved");
        std::fs::create_dir_all(&moved).unwrap();
        std::fs::write(moved.join(".git"), format!("gitdir: {}", s(&root.join("gone").join(".git").join("worktrees").join("moved")))).unwrap();
        assert_eq!(of(&moved), None, "its repository is not there");

        // Gone, as Claude Code leaves a clean one: where it was is all there is, in a
        // repository; in a folder that is none, nothing says it ever was one.
        assert_eq!(of(&worktrees.join("gone")), is_repo);
        assert_eq!(of(&worktrees.join("gone").join("src")), is_repo);
        let folder = root.join("no-repo");
        std::fs::create_dir_all(folder.join(".claude").join("worktrees").join("there")).unwrap();
        assert_eq!(of(&folder.join(".claude").join("worktrees").join("gone")), None);
        assert_eq!(of(&folder.join(".claude").join("worktrees").join("there")), None);
        assert_eq!(of(&worktrees), None, "the folder that holds them");

        let _ = std::fs::remove_dir_all(&root);
    }

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
        let commit = |msg: &str| ok(git(&wt).args(["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", msg]));
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
        std::fs::create_dir_all(Path::new(&wt).join("build")).unwrap();
        std::fs::write(Path::new(&wt).join("build").join("out.bin"), "x").unwrap();
        assert_eq!(unsaved_work(&repo, &wt), None, "build output would stop every delete");
        // Moved on in the submodule, with the repo's config asking not to see it.
        assert!(ok(git(&repo).args(["config", "diff.ignoreSubmodules", "all"])));
        assert!(ok(git(&sub).args(user).args(["commit", "-q", "--allow-empty", "-m", "s2"])));
        let said = unsaved_work(&repo, &wt).unwrap_or_default();
        assert!(said.contains("1 uncommitted change") && said.contains("sub"), "{said}");
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
        assert!(!is_worktree_of(r"E:\p", r"E:\p\.claude\worktrees\..\.."), "leads back to the folder");
        assert!(!is_worktree_of(r"E:\p", r"E:\p\.claude\worktrees\x\..\..\.."));
        assert!(!is_worktree_of(r"E:\p", r"E:\p\.claude\worktrees\.\x"));
        assert!(!is_worktree_of(r"E:\p", r"E:\p\.claude\worktrees-old\x"));
        assert!(!is_worktree_of(r"E:\p", r"E:\p\.claude\worktrees\x\src"), "a part of one");
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
