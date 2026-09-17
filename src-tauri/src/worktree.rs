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
        let _ = git(&folder).args(["worktree", "prune"]).output();
        drop_branch(&folder, branch);
        return Ok(());
    }

    // Resolve the branch before the worktree disappears from git's records.
    let branch = branch_of(&folder, &worktree_path);

    // The killed session may hold file locks for a moment — retry briefly.
    let mut removed = false;
    for _ in 0..3 {
        match git(&folder)
            .args(["worktree", "remove", "--force", &worktree_path])
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
/// from crashes, or from chats deleted before their path was known.
#[tauri::command]
pub async fn orphan_worktrees(
    state: crate::pty::PtyState<'_>,
    folder: String,
    in_use: Vec<String>,
    account_paths: Vec<String>,
) -> Result<Vec<String>, String> {
    let live = crate::pty::live_cwds(&state);
    orphan_worktrees_now(folder, in_use, account_paths, live)
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
    for account in &account_paths {
        claimed.extend(crate::claude::session::live_cwds(account).iter().map(|p| norm(p)));
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
            if !claimed.contains(&norm(&path)) {
                out.push(path);
            }
        }
    }
    out.sort();
    Ok(out)
}

#[tauri::command]
pub async fn remove_orphan_worktrees(
    state: crate::pty::PtyState<'_>,
    folder: String,
    in_use: Vec<String>,
    account_paths: Vec<String>,
) -> Result<usize, String> {
    // Re-derive the list here rather than trusting one from the UI: it may be
    // seconds stale, and everything below is destructive.
    let live = crate::pty::live_cwds(&state);
    let orphans = orphan_worktrees_now(folder.clone(), in_use, account_paths, live)?;
    let mut n = 0;
    for path in orphans {
        if remove_worktree_now(folder.clone(), path).is_ok() {
            n += 1;
        }
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktree_membership_covers_both_providers() {
        assert!(is_worktree_of(r"E:\p", r"E:\p\.claude\worktrees\x"));
        assert!(is_worktree_of(r"E:\p", r"e:/p/.codex/worktrees/codex-abc"));
        assert!(!is_worktree_of(r"E:\p", r"E:\p\.codex\worktrees"));
        assert!(!is_worktree_of(r"E:\p", r"E:\q\.codex\worktrees\x"));
    }

    #[test]
    fn short_ids_are_six_hex() {
        let id = short_id();
        assert_eq!(id.len(), 6);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
