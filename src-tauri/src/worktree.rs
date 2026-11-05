//! Git worktrees for isolated chats. Each CLI makes its own from
//! `--worktree`: Claude Code under `<folder>/.claude/worktrees/<name>` on a
//! branch `worktree-<name>`, Codex under `<CODEX_HOME>/worktrees/<4 hex>/<repo>`
//! on a detached HEAD. Luna only finds them again — to remove one with its
//! chat, or sweep the ones no chat claims — and never makes one itself.

use crate::paths::{path_key as norm, path_key_loose as norm_in_use, KEY_SEP};
use std::path::{Path, PathBuf};
use std::process::Command;

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
        .and_then(|rest| rest.strip_prefix(KEY_SEP))
        .is_some_and(|name| !name.is_empty() && !name.contains(KEY_SEP))
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

/// The CLI-made worktree `path` is in, itself or a folder of its checkout, with
/// the project it belongs to: a session's cwd follows its shell's `cd` (Claude Code
/// moves its own with it), and where it works is the worktree all the same. Its
/// root is the nearest folder up that is one of the project's worktrees by shape,
/// or by its `.git` file (one made from inside another worktree). `project_of`
/// knows a Codex worktree by its root alone, so a folder of one is looked up from.
pub fn root_of(path: &str) -> Option<(String, String)> {
    let codex = || Path::new(path).ancestors().map(|d| d.to_string_lossy().into_owned()).find(|d| codex_shaped(d) && repo_of_linked(d).is_some());
    let project = project_of(path).or_else(|| project_of(&codex()?))?;
    let mut dir = Path::new(path);
    loop {
        let d = dir.to_string_lossy();
        if is_worktree_of(&project, &d) || repo_of_linked(&d).is_some_and(|r| same_place(&r, &project)) {
            return Some((d.into_owned(), project));
        }
        dir = dir.parent()?;
    }
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

/// What a checkout has checked out: the branch, none on a detached HEAD, and
/// its commit, short, none on a branch with no commit yet.
#[derive(Debug, PartialEq, Default)]
pub struct Head {
    pub branch: Option<String>,
    pub sha: Option<String>,
}

/// How many symbolic refs are followed from HEAD: as many as git does.
const REF_HOPS: usize = 5;

/// A short sha as `git log --oneline` gives one.
const SHORT_SHA: usize = 7;

/// `head_at` for the checkout a folder is in, wherever in it.
pub fn head_of(dir: &str) -> Option<Head> {
    let dir = Path::new(dir);
    // One gone would be read as the repository around it.
    if !dir.is_dir() {
        return None;
    }
    head_at(&dir.ancestors().find(|d| d.join(".git").exists())?.to_string_lossy())
}

/// What the checkout at `root` has checked out, off its `.git` alone: `luna_list`
/// asks this of every row of every call, and a git process per row is what it
/// must not cost. As git resolves it: from HEAD through any symbolic refs, each
/// loose or packed. None for what this does not read (reftable, whose HEAD names a
/// ref that cannot exist) or does not lead to a commit (a cycle, a chain past
/// REF_HOPS).
pub fn head_at(root: &str) -> Option<Head> {
    let (git_dir, common) = git_dirs(Path::new(root))?;
    let mut value = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let mut chain: Vec<String> = Vec::new();
    // The last ref of a chain is the branch: `git worktree list` says that one.
    let branch = |chain: &[String]| chain.last().and_then(|name| name.strip_prefix("refs/heads/")).map(str::to_owned);
    loop {
        let Some(name) = value.trim().strip_prefix("ref:").map(str::trim) else {
            let id = value.trim();
            return is_object_id(id).then(|| Head { branch: branch(&chain), sha: Some(id[..SHORT_SHA].to_string()) });
        };
        if chain.len() >= REF_HOPS || chain.iter().any(|seen| seen == name) || !is_ref_name(name) {
            return None;
        }
        chain.push(name.to_string());
        match read_ref(&git_dir, &common, name) {
            Some(next) => value = next,
            // Nothing on it yet.
            None => return Some(Head { branch: branch(&chain), sha: None }),
        }
    }
}

/// A checkout's own git folder, where its HEAD is, and the one its refs are in:
/// for a linked worktree, the repository's.
fn git_dirs(root: &Path) -> Option<(PathBuf, PathBuf)> {
    let dot_git = root.join(".git");
    if dot_git.is_dir() {
        return Some((dot_git.clone(), dot_git));
    }
    let text = std::fs::read_to_string(&dot_git).ok()?;
    let git_dir = root.join(text.trim().strip_prefix("gitdir:")?.trim());
    let common = match std::fs::read_to_string(git_dir.join("commondir")) {
        Ok(text) => git_dir.join(text.trim()),
        Err(_) => git_dir.clone(),
    };
    Some((git_dir, common))
}

/// A ref's value, loose or packed: an object id, or `ref: <name>`. These few are
/// each worktree's own.
fn read_ref(git_dir: &Path, common: &Path, name: &str) -> Option<String> {
    let own = ["refs/bisect/", "refs/worktree/", "refs/rewritten/"].iter().any(|p| name.starts_with(p));
    let dir = if own { git_dir } else { common };
    if let Ok(value) = std::fs::read_to_string(dir.join(name)) {
        return Some(value);
    }
    let packed = std::fs::read_to_string(common.join("packed-refs")).ok()?;
    packed.lines().filter(|l| !l.starts_with(['#', '^'])).find_map(|l| {
        let (id, packed_name) = l.split_once(' ')?;
        (packed_name.trim() == name).then(|| id.to_string())
    })
}

/// A ref as git would have it, so that what a file says is never a way out of
/// the git folder: its parts start with no dot, as `.invalid` of a reftable does.
fn is_ref_name(name: &str) -> bool {
    name.starts_with("refs/") && !name.contains(['\\', ':']) && name.split('/').all(|part| !part.is_empty() && !part.starts_with('.'))
}

/// SHA-1 or SHA-256, in hex.
fn is_object_id(id: &str) -> bool {
    matches!(id.len(), 40 | 64) && id.chars().all(|c| c.is_ascii_hexdigit())
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

/// True when the `cwd` is the worktree or anywhere inside it, both by
/// `norm_in_use`: a claim missed leaves the worktree to be deleted under a session.
fn claims(cwd: &str, worktree: &str) -> bool {
    cwd == worktree || cwd.strip_prefix(worktree).is_some_and(|rest| rest.starts_with(KEY_SEP))
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
    let mut claimed: Vec<String> = in_use.iter().map(|p| norm_in_use(p)).collect();
    // Claude Code moves into its worktree on its own; only its registry says
    // where. Where a Codex session moved, its rollout says (`pty::live_cwds`).
    for account in account_paths.iter().cloned().chain(own_claude_dir()) {
        claimed.extend(crate::claude::session::live_cwds(&account).iter().map(|p| norm_in_use(p)));
    }
    claimed.extend(live_pty_cwds.iter().map(|p| norm_in_use(p)));

    let mut out = vec![];
    for path in candidates(&folder, &account_paths) {
        if younger_than(&path, GRACE_SECS) {
            continue;
        }
        let path = path.to_string_lossy().into_owned();
        let key = norm_in_use(&path);
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
    let claimed: Vec<String> = live_cwds.iter().map(|p| norm_in_use(p)).collect();
    now.into_iter()
        .filter(|p| !before.iter().any(|b| norm_in_use(b) == norm_in_use(p)))
        .filter(|p| !claimed.iter().any(|c| claims(c, &norm_in_use(p))))
        .collect()
}

/// A name for `--worktree` that is one and can only be read as one: what Claude Code
/// takes (it refuses past 64 characters), and no leading dash. As a separate argument
/// `--dangerously-skip-permissions` is that flag, which turns permissions off.
pub fn name_ok(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && !name.starts_with(['-', '.']) && name.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

/// The argument that asks Claude Code for a worktree: that name if it will do, always
/// with `=` so that nothing after the flag is taken for it, else one of its own.
pub fn worktree_arg(name: Option<&str>) -> String {
    match name.filter(|n| name_ok(n)) {
        Some(name) => format!("--worktree={name}"),
        None => "--worktree".to_string(),
    }
}

/// Where Claude Code puts the worktree of this name.
pub fn named_path(folder: &str, name: &str) -> String {
    claude_worktrees(folder).join(name).to_string_lossy().into_owned()
}

/// What Claude Code itself leaves in a worktree it makes: its copy of the project's
/// local settings. Checked on 2.1.314. What `.worktreeinclude` copies also lands
/// there, and is not told from anything else: it reads as work, and the worktree is
/// reported rather than gone on in.
const LEFT_BY_CLAUDE: &str = "!! .claude/settings.local.json";

/// The commit git has this branch created at: the oldest line of its reflog. Luna
/// cannot know it ahead, as Claude Code starts a worktree where it sees fit (origin's
/// default branch, not the folder's HEAD), and a core may go before anything could note
/// it; git writes it as the branch is made. None with no reflog, or none kept.
fn start_commit(folder: &str, branch: &str) -> Option<String> {
    let out = git(folder).args(["reflog", "show", "--format=%H", &format!("refs/heads/{branch}")]).output().ok().filter(|o| o.status.success())?;
    String::from_utf8_lossy(&out.stdout).lines().rev().map(str::trim).find(|l| !l.is_empty()).map(str::to_owned)
}

/// Whether a worktree Luna named for a helper is still as Claude Code made it, so that
/// going back into it is no different from starting in a new one: on the branch made for
/// it, its HEAD the commit that branch was created at, and nothing in it that is not in
/// that commit, what git ignores included, but what the CLI itself leaves. No commit
/// count stands in for the first: a HEAD moved to another branch's tip loses nothing and
/// is not where this started. What cannot be told is not as made.
pub fn as_made(folder: &str, path: &str) -> bool {
    let Some(branch) = made_branch(path) else { return false };
    if !Path::new(path).join(".git").is_file() || branch_of(folder, path).as_deref() != Some(branch.as_str()) {
        return false;
    }
    let head = git(path).args(["rev-parse", "--verify", "HEAD"]).output().ok().filter(|o| o.status.success());
    let head = head.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    if head.is_none() || head != start_commit(folder, &branch) {
        return false;
    }
    let status = git(path)
        .args(["-c", "core.quotepath=off", "status", "--porcelain", "--ignored=matching", "--untracked-files=all", "--ignore-submodules=none"])
        .output()
        .ok()
        .filter(|o| o.status.success());
    status.is_some_and(|o| String::from_utf8_lossy(&o.stdout).lines().all(|l| l.is_empty() || l == LEFT_BY_CLAUDE))
}

/// Whether a worktree holds files git ignores, beside what Claude Code leaves in
/// each it makes: what removing it takes without counting as work. None when git
/// cannot say.
pub fn ignored_in(path: &str) -> Option<bool> {
    let out = git(path)
        .args(["-c", "core.quotepath=off", "status", "--porcelain", "--ignored=matching", "--untracked-files=all"])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    Some(String::from_utf8_lossy(&out.stdout).lines().any(|l| l.starts_with("!! ") && l != LEFT_BY_CLAUDE))
}

/// Whether any of these working folders is this worktree or inside it.
pub fn someone_in(path: &str, cwds: &[String]) -> bool {
    let path = norm_in_use(path);
    cwds.iter().any(|c| claims(&norm_in_use(c), &path))
}

/// Where a session of this account's runs by Claude Code's own registry, and a
/// person's own sessions from a terminal. Only those with a process still there: a
/// CLI that went with a core leaves its entry behind, and the worktree it names is
/// the very one to go on in.
pub fn claude_sessions_running(account_path: &str) -> Vec<String> {
    [Some(account_path.to_string()), own_claude_dir()].into_iter().flatten().flat_map(|dir| crate::claude::session::running_cwds(&dir)).collect()
}

/// The worktrees Codex has made for this folder on this account since `before`, that
/// nobody is in: all there is to tell what a Codex helper cut off may have begun, as it
/// takes no name and says where only once it has a thread.
pub fn codex_new_since(folder: &str, account_path: &str, before: &[String], claimed: &[String]) -> Vec<String> {
    let mut new: Vec<String> = left_since(made_for(folder, account_path), before, claimed).into_iter().filter(|p| codex_shaped(p)).collect();
    new.sort();
    new
}

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
        .then(|| git(path).args(["-c", "core.quotepath=off", "status", "--porcelain", "--untracked-files=normal", "--ignore-submodules=none"]).output().ok())
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
    let unique_commits = commits_lost(folder, path, branch.as_deref());

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

    /// A path written `E:\p\x` as this platform has it (`/E/p/x` off Windows),
    /// for what takes one apart by the platform's own separators.
    fn native(path: &str) -> String {
        if cfg!(windows) {
            path.to_string()
        } else {
            format!("/{}", path.replace(':', "").replace('\\', "/"))
        }
    }

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
        let now = vec![native(r"E:\p\.claude\worktrees\old"), native(r"E:\p\.claude\worktrees\new"), native(r"E:\p\.claude\worktrees\busy")];
        let before = [native(r"E:\p\.claude\worktrees\old")];
        let live = [native(r"E:\p\.claude\worktrees\busy\sub")];
        assert_eq!(left_since(now, &before, &live), [native(r"E:\p\.claude\worktrees\new")]);
    }

    #[test]
    fn a_name_cannot_be_read_as_a_flag_and_is_always_given_with_equals() {
        assert_eq!(worktree_arg(Some("luna-cmvxxrt9k")), "--worktree=luna-cmvxxrt9k");
        assert_eq!(worktree_arg(None), "--worktree");
        for bad in ["--dangerously-skip-permissions", "-x", "", ".", "..", ".hidden", "a b", "a/b", "a\\b", "a=b", &"x".repeat(65)] {
            assert!(!name_ok(bad), "{bad}");
            assert_eq!(worktree_arg(Some(bad)), "--worktree", "no name rather than that one");
        }
        assert!(name_ok("merry-coalescing-cosmos") && name_ok("a_b.c-2") && name_ok(&"x".repeat(64)));
    }

    #[test]
    fn a_worktree_is_gone_back_into_only_as_its_cli_made_it() {
        let Some((root, repo, codex)) = codex_layout("as-made") else { return };
        let ok = |c: &mut Command| c.output().is_ok_and(|o| o.status.success());
        let user = ["-c", "user.email=t@t", "-c", "user.name=t"];
        std::fs::write(Path::new(&repo).join(".gitignore"), "ignored.txt\n.claude/settings.local.json\n").unwrap();
        assert!(ok(git(&repo).args(["add", ".gitignore"])));
        assert!(ok(git(&repo).args(user).args(["commit", "-q", "-m", "ignores"])));
        let start = String::from_utf8_lossy(&git(&repo).args(["rev-parse", "HEAD"]).output().unwrap().stdout).trim().to_string();
        let made = |name: &str| {
            let wt = named_path(&repo, name);
            assert!(ok(git(&repo).args(["worktree", "add", "-q", "-b", &format!("worktree-{name}"), &wt, &start])));
            wt
        };
        assert!(ok(git(&repo).args(user).args(["commit", "-q", "--allow-empty", "-m", "main moves on"])));

        let fresh = made("fresh");
        assert!(as_made(&repo, &fresh), "at the commit its branch was made at, though the folder's HEAD is past it");
        // What Claude Code copies in itself is no work.
        std::fs::create_dir_all(Path::new(&fresh).join(".claude")).unwrap();
        std::fs::write(Path::new(&fresh).join(".claude").join("settings.local.json"), "{}").unwrap();
        assert!(as_made(&repo, &fresh));

        // Anything else in it is, what git ignores too: `unsaved_work` would pass that.
        let ignored = made("ignored");
        std::fs::write(Path::new(&ignored).join("ignored.txt"), "work").unwrap();
        assert_eq!(unsaved_work(&repo, &ignored), None);
        assert!(!as_made(&repo, &ignored));
        let untracked = made("untracked");
        std::fs::write(Path::new(&untracked).join("half-done.txt"), "work").unwrap();
        assert!(!as_made(&repo, &untracked));
        let cut = made("cut");
        std::fs::remove_file(Path::new(&cut).join(".gitignore")).unwrap();
        assert!(!as_made(&repo, &cut), "a checkout cut short reads as changes");

        // A HEAD anywhere else, though no commit would be lost by it: at another
        // branch's tip, past its start, or on another branch.
        let moved = made("moved");
        assert!(ok(git(&moved).args(["reset", "-q", "--hard", "HEAD~1"])));
        assert_eq!(unsaved_work(&repo, &moved), None, "nothing is lost, which is no proof");
        assert!(!as_made(&repo, &moved));
        let committed = made("committed");
        assert!(ok(git(&committed).args(user).args(["commit", "-q", "--allow-empty", "-m", "work"])));
        assert!(!as_made(&repo, &committed));
        let switched = made("switched");
        assert!(ok(git(&switched).args(["switch", "-q", "-c", "its-own"])));
        assert!(!as_made(&repo, &switched));

        // Not a checkout, no record of where it began, Codex's with no branch made.
        let no_git = made("no-git");
        std::fs::remove_file(Path::new(&no_git).join(".git")).unwrap();
        assert!(!as_made(&repo, &no_git));
        let no_log = made("no-log");
        assert!(ok(git(&repo).args(["reflog", "expire", "--expire=now", "--all"])));
        assert!(!as_made(&repo, &no_log), "what cannot be told");
        assert!(!as_made(&repo, &codex));

        // Whose it is goes by the working folders, a subfolder too.
        assert!(someone_in(&fresh, &[format!("{fresh}{}src", std::path::MAIN_SEPARATOR)]));
        assert!(!someone_in(&fresh, &[format!("{fresh}x")]));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn what_a_codex_helper_may_have_begun_is_what_is_new_of_its_kind() {
        let Some((root, repo, codex)) = codex_layout("codex-new") else { return };
        let account = root.join("account").to_string_lossy().into_owned();
        let ok = |c: &mut Command| c.output().is_ok_and(|o| o.status.success());
        // Claude Code's in that folder is none of a Codex helper's.
        assert!(ok(git(&repo).args(["worktree", "add", "-q", "-b", "worktree-c", &named_path(&repo, "c")])));
        assert_eq!(codex_new_since(&repo, &account, &[], &[]), [codex.clone()]);
        assert!(codex_new_since(&repo, &account, &[codex.clone()], &[]).is_empty());
        assert!(codex_new_since(&repo, &account, &[], &[codex.clone()]).is_empty());
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
        assert_eq!(project_of(&native("E:/p/x/.claude/worktrees/a-b-c")), Some(native(r"E:\p\x")));
        assert_eq!(project_of(&native(r"E:\p\x")), None);
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
    fn a_cwd_inside_a_worktree_is_that_worktree() {
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let root = |p: &Path| root_of(&s(p)).map(|(wt, project)| (norm(&wt), norm(&project)));
        // Gone, as a clean one is between turns, from a project gone too: its shape.
        let gone = std::env::temp_dir().join(format!("luna-root-of-gone-{}", std::process::id())).join("x");
        let wt = gone.join(".claude").join("worktrees").join("a-b-c");
        let want = Some((norm(&s(&wt)), norm(&s(&gone))));
        assert_eq!(root(&wt), want);
        assert_eq!(root(&wt.join("src-tauri").join("src")), want, "a shell that went into it");
        assert_eq!(root(&gone.join("src")), None, "no worktree");
        assert_eq!(root(&gone.join(".claude").join("worktrees")), None);
        // Real ones, by what git says of them.
        let Some((tmp, repo, codex)) = codex_layout("root") else { return };
        assert_eq!(root(&Path::new(&codex).join("src")), Some((norm(&codex), norm(&repo))), "Codex's own shape");
        let claude = Path::new(&repo).join(".claude").join("worktrees").join("real");
        assert!(git(&repo).args(["worktree", "add", "-q", "-b", "worktree-real", &s(&claude)]).output().is_ok_and(|o| o.status.success()));
        std::fs::create_dir_all(claude.join("src").join("deep")).unwrap();
        assert_eq!(root(&claude.join("src").join("deep")), Some((norm(&s(&claude)), norm(&repo))));
        let _ = remove_worktree_now(repo.clone(), codex);
        let _ = remove_worktree_now(repo, s(&claude));
        let _ = std::fs::remove_dir_all(&tmp);
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

    #[test]
    fn codex_worktrees_have_a_shape() {
        assert!(codex_shaped(&native(r"E:\acc\worktrees\d6ff\repo")));
        assert!(codex_shaped("/home/u/.codex/worktrees/0a1B/app/"));
        assert!(!codex_shaped(&native(r"E:\acc\worktrees\d6ff")));
        assert!(!codex_shaped(&native(r"E:\acc\worktrees\codex-1\repo")));
        assert!(!codex_shaped(&native(r"E:\p\.codex\worktrees\codex-1a2b3c")));
    }

    #[test]
    fn claude_worktree_membership() {
        let of = |candidate: &str| is_worktree_of(&native(r"E:\p"), &native(candidate));
        assert!(of(r"E:\p\.claude\worktrees\x"));
        assert!(!of(r"E:\p\.claude\worktrees"));
        assert!(!of(r"E:\q\.claude\worktrees\x"));
        assert!(!of(r"E:\p\.codex\worktrees\codex-abc"));
        assert!(!of(r"E:\p\.claude\worktrees\..\.."), "leads back to the folder");
        assert!(!of(r"E:\p\.claude\worktrees\x\..\..\.."));
        assert!(!of(r"E:\p\.claude\worktrees\.\x"));
        assert!(!of(r"E:\p\.claude\worktrees-old\x"));
        assert!(!of(r"E:\p\.claude\worktrees\x\src"), "a part of one");
        // Another spelling of it, on the one system where it certainly is one.
        assert_eq!(is_worktree_of(r"E:\p", "e:/p/.claude/worktrees/x"), cfg!(windows));
    }

    #[test]
    fn a_folder_that_differs_by_case_is_another_folders_before_anything_is_deleted() {
        // The guard: only Windows is sure of it. On macOS that volume may be
        // case-sensitive, and the worse mistake is deleting another's worktree.
        let wt = native(r"E:\p\.claude\worktrees\x");
        assert_eq!(is_worktree_of(&native(r"E:\P"), &wt), cfg!(windows));
        assert_eq!(is_worktree_of(&native(r"E:\p"), &native(r"E:\p\.CLAUDE\worktrees\x")), cfg!(windows));
        let Some((root, repo, wt)) = codex_layout("case") else { return };
        assert!(is_worktree_of(&repo, &wt));
        assert_eq!(is_worktree_of(&repo.to_uppercase(), &wt), cfg!(windows), "by what its checkout points back at");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_session_in_a_folder_that_differs_by_case_keeps_a_worktree_where_that_may_be_one_folder() {
        // The other way about: what is missed here is deleted with a session in it.
        let may_be_one = cfg!(any(windows, target_os = "macos"));
        let (wt, shouted) = (native(r"E:\p\.claude\worktrees\a"), native(r"E:\P\.claude\worktrees\A"));
        assert_eq!(left_since(vec![wt.clone()], &[], std::slice::from_ref(&shouted)).is_empty(), may_be_one, "a session in it");
        assert_eq!(left_since(vec![wt.clone()], std::slice::from_ref(&shouted), &[]).is_empty(), may_be_one, "there before");
    }

    #[test]
    fn a_session_in_a_subfolder_claims_the_worktree() {
        let wt = norm_in_use(&native(r"E:\p\.claude\worktrees\a"));
        assert!(claims(&wt, &wt));
        assert!(claims(&norm_in_use(&native(r"E:\p\.claude\worktrees\a\src")), &wt));
        assert!(!claims(&norm_in_use(&native(r"E:\p\.claude\worktrees\ab")), &wt));
        assert!(!claims(&norm_in_use(&native(r"E:\p")), &wt));
        assert_eq!(claims(&norm_in_use("e:/p/.claude/worktrees/a"), &norm_in_use(r"E:\p\.claude\worktrees\a")), cfg!(windows));
    }

    /// A throwaway checkout's `.git`, with these files in it.
    fn git_folder(tag: &str, files: &[(&str, &str)]) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("luna-head-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let git = root.join(".git");
        std::fs::create_dir_all(&git).unwrap();
        for (name, text) in files {
            let file = git.join(name);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
        }
        (root, git)
    }

    const ID: &str = "0123456789abcdef0123456789abcdef01234567";

    fn on(branch: Option<&str>, sha: Option<&str>) -> Option<Head> {
        Some(Head { branch: branch.map(str::to_owned), sha: sha.map(str::to_owned) })
    }

    #[test]
    fn a_checkouts_head_is_read_off_its_git_folder() {
        let id = format!("{ID}\n");
        let cases: Vec<(&str, Vec<(&str, &str)>, Option<Head>)> = vec![
            ("loose", vec![("HEAD", "ref: refs/heads/main\n"), ("refs/heads/main", &id)], on(Some("main"), Some("0123456"))),
            ("nested", vec![("HEAD", "ref: refs/heads/fix/a\n"), ("refs/heads/fix/a", &id)], on(Some("fix/a"), Some("0123456"))),
            ("detached", vec![("HEAD", &id)], on(None, Some("0123456"))),
            ("unborn", vec![("HEAD", "ref: refs/heads/new\n")], on(Some("new"), None)),
            ("bad-id", vec![("HEAD", "ref: refs/heads/main\n"), ("refs/heads/main", "not an id\n")], None),
            ("short-id", vec![("HEAD", "0123456\n")], None),
            ("reftable", vec![("HEAD", "ref: refs/heads/.invalid\n"), ("refs/heads", "this repository uses the reftable format\n")], None),
            ("way-out", vec![("HEAD", "ref: refs/../../../secret\n")], None),
            ("no-ref", vec![("HEAD", "ref: HEAD\n")], None),
        ];
        for (tag, files, want) in cases {
            let (root, _) = git_folder(tag, &files);
            assert_eq!(head_at(&root.to_string_lossy()), want, "{tag}");
            let _ = std::fs::remove_dir_all(&root);
        }
        let sha256 = "a".repeat(64);
        let (root, _) = git_folder("sha256", &[("HEAD", &sha256)]);
        assert_eq!(head_at(&root.to_string_lossy()), on(None, Some("aaaaaaa")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_ref_is_loose_or_packed_at_every_step_of_its_chain() {
        let packed = format!("# pack-refs with: peeled fully-peeled sorted\n{ID} refs/heads/main\n^{}\n", "f".repeat(40));
        // HEAD -> loose alias -> packed branch: the branch is the last of the chain.
        let (root, _) = git_folder("chain", &[("HEAD", "ref: refs/heads/alias\n"), ("refs/heads/alias", "ref: refs/heads/main\n"), ("packed-refs", &packed)]);
        assert_eq!(head_at(&root.to_string_lossy()), on(Some("main"), Some("0123456")));
        let _ = std::fs::remove_dir_all(&root);
        // A loose ref over a packed one wins, as it is the newer.
        let other = format!("{}\n", "b".repeat(40));
        let (root, _) = git_folder("over", &[("HEAD", "ref: refs/heads/main\n"), ("refs/heads/main", &other), ("packed-refs", &packed)]);
        assert_eq!(head_at(&root.to_string_lossy()), on(Some("main"), Some("bbbbbbb")));
        let _ = std::fs::remove_dir_all(&root);
        // A chain ending on nothing yet: an unborn branch, as HEAD alone would be.
        let (root, _) = git_folder("dangling", &[("HEAD", "ref: refs/heads/alias\n"), ("refs/heads/alias", "ref: refs/heads/new\n")]);
        assert_eq!(head_at(&root.to_string_lossy()), on(Some("new"), None));
        let _ = std::fs::remove_dir_all(&root);
        // Round and round, or on and on: no answer, and it ends.
        let (root, _) = git_folder("cycle", &[("HEAD", "ref: refs/heads/a\n"), ("refs/heads/a", "ref: refs/heads/b\n"), ("refs/heads/b", "ref: refs/heads/a\n")]);
        assert_eq!(head_at(&root.to_string_lossy()), None);
        let _ = std::fs::remove_dir_all(&root);
        let (root, _) = git_folder("self", &[("HEAD", "ref: refs/heads/a\n"), ("refs/heads/a", "ref: refs/heads/a\n")]);
        assert_eq!(head_at(&root.to_string_lossy()), None);
        let _ = std::fs::remove_dir_all(&root);
        let mut long: Vec<(String, String)> = vec![("HEAD".into(), "ref: refs/heads/r0\n".into())];
        for n in 0..=REF_HOPS {
            long.push((format!("refs/heads/r{n}"), format!("ref: refs/heads/r{}\n", n + 1)));
        }
        long.push((format!("refs/heads/r{}", REF_HOPS + 1), format!("{ID}\n")));
        let files: Vec<(&str, &str)> = long.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        let (root, _) = git_folder("long", &files);
        assert_eq!(head_at(&root.to_string_lossy()), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_linked_worktree_has_its_head_of_its_own_and_its_refs_from_the_repository() {
        let id = format!("{ID}\n");
        let (root, git) = git_folder("linked", &[
            ("refs/heads/feature", &id),
            ("worktrees/wt/HEAD", "ref: refs/heads/feature\n"),
            ("worktrees/wt/commondir", "../..\n"),
        ]);
        let wt = root.join("wt");
        std::fs::create_dir_all(wt.join("src")).unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", git.join("worktrees").join("wt").display())).unwrap();
        assert_eq!(head_at(&wt.to_string_lossy()), on(Some("feature"), Some("0123456")));
        // Anywhere in it; and one gone is no reason to read the repository around it.
        assert_eq!(head_of(&wt.join("src").to_string_lossy()), on(Some("feature"), Some("0123456")));
        assert_eq!(head_of(&wt.join("gone").to_string_lossy()), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// What it reads is what git says, where this runs with git.
    #[test]
    fn a_head_read_off_the_files_is_what_git_says() {
        let root = std::env::temp_dir().join(format!("luna-head-real-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let repo = root.to_string_lossy().into_owned();
        let run = |dir: &str, args: &[&str]| -> Option<String> {
            let o = git(dir).args(["-c", "user.email=t@t", "-c", "user.name=t"]).args(args).output().ok()?;
            o.status.success().then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
        };
        if run(&repo, &["init", "-q"]).is_none() || run(&repo, &["commit", "-q", "--allow-empty", "-m", "i"]).is_none() {
            let _ = std::fs::remove_dir_all(&root);
            return;
        }
        let says = |dir: &str| on(run(dir, &["symbolic-ref", "--short", "-q", "HEAD"]).as_deref(), run(dir, &["rev-parse", "--short=7", "HEAD"]).as_deref());
        assert_eq!(head_at(&repo), says(&repo));
        let wt = root.join("wt").to_string_lossy().into_owned();
        run(&repo, &["worktree", "add", "-q", "-b", "side", &wt]).unwrap();
        assert_eq!(head_at(&wt), says(&wt));
        // Packed, from the worktree too; then detached there.
        run(&repo, &["pack-refs", "--all"]).unwrap();
        assert_eq!(head_at(&wt), says(&wt));
        run(&wt, &["checkout", "-q", "--detach"]).unwrap();
        assert_eq!(head_at(&wt), says(&wt));
        let _ = run(&repo, &["worktree", "remove", "--force", &wt]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_worktree_names_the_branch_it_was_made_on() {
        assert_eq!(made_branch(&native(r"E:\p\.claude\worktrees\merry-coalescing-cosmos")).as_deref(), Some("worktree-merry-coalescing-cosmos"));
        assert_eq!(made_branch(&native(r"E:\acc\worktrees\d6ff\repo")), None);
    }
}
