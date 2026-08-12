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

fn worktrees_dir(folder: &str) -> PathBuf {
    Path::new(folder).join(".claude").join("worktrees")
}

/// True when `candidate` sits under this folder's `.claude/worktrees`. Every
/// destructive path below is gated on this.
pub fn is_worktree_of(folder: &str, candidate: &str) -> bool {
    let base = norm(&worktrees_dir(folder).to_string_lossy());
    let c = norm(candidate);
    c.len() > base.len() && c.starts_with(&base)
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

#[tauri::command]
pub fn remove_worktree(folder: String, worktree_path: String) -> Result<(), String> {
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

    drop_branch(&folder, branch);
    Ok(())
}

/// Deletes the throwaway branch `--worktree` created alongside the directory.
/// Restricted to the CLI's own `worktree-*` naming: anything else is a branch
/// the user checked out themselves, and `-D` would discard real commits.
fn drop_branch(folder: &str, branch: Option<String>) {
    let Some(branch) = branch else { return };
    if !branch.starts_with("worktree-") {
        return;
    }
    let _ = git(folder).args(["branch", "-D", &branch]).output();
}

/// A worktree younger than this is assumed to belong to a session that has not
/// reported its path yet, and is never treated as an orphan.
const GRACE_SECS: u64 = 300;

/// Every cwd the CLI currently has a live session in, across the given account
/// config dirs. A running session must never be swept, even if the UI has not
/// yet learned where it lives.
fn live_session_cwds(account_paths: &[String]) -> Vec<String> {
    let mut out = vec![];
    for account in account_paths {
        let dir = Path::new(account).join("sessions");
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let Ok(text) = std::fs::read_to_string(entry.path()) else { continue };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { continue };
            if let Some(cwd) = v["cwd"].as_str() {
                out.push(norm(cwd));
            }
        }
    }
    out
}

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
pub fn orphan_worktrees(
    folder: String,
    in_use: Vec<String>,
    account_paths: Vec<String>,
) -> Result<Vec<String>, String> {
    let dir = worktrees_dir(&folder);
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut claimed: Vec<String> = in_use.iter().map(|p| norm(p)).collect();
    claimed.extend(live_session_cwds(&account_paths));

    let mut out = vec![];
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
    out.sort();
    Ok(out)
}

#[tauri::command]
pub fn remove_orphan_worktrees(
    folder: String,
    in_use: Vec<String>,
    account_paths: Vec<String>,
) -> Result<usize, String> {
    // Re-derive the list here rather than trusting one from the UI: it may be
    // seconds stale, and everything below is destructive.
    let orphans = orphan_worktrees(folder.clone(), in_use, account_paths)?;
    let mut n = 0;
    for path in orphans {
        if remove_worktree(folder.clone(), path).is_ok() {
            n += 1;
        }
    }
    Ok(n)
}
