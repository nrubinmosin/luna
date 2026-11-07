//! The worktrees of a folder no live chat claims, found every minute by reading
//! directories only, and the sweep of those the user picks; and what a spawn cut
//! off left.

use super::*;

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
pub async fn orphan_worktrees(folder: String, in_use: Vec<String>, account_paths: Vec<String>) -> Result<Vec<String>, String> {
    let live = crate::pty::live_cwds();
    tauri::async_runtime::spawn_blocking(move || orphan_worktrees_now(folder, in_use, account_paths, live))
        .await
        .map_err(|e| e.to_string())?
}

/// True when the `cwd` is the worktree or anywhere inside it, both by
/// `norm_in_use`: a claim missed leaves the worktree to be deleted under a session.
pub(super) fn claims(cwd: &str, worktree: &str) -> bool {
    cwd == worktree || cwd.strip_prefix(worktree).is_some_and(|rest| rest.starts_with(KEY_SEP))
}

/// The user's own Claude Code config, for sessions started from a terminal
/// rather than from Luna: they sit in worktrees too, and Luna's accounts know
/// nothing of them.
pub(super) fn own_claude_dir() -> Option<String> {
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
pub(super) fn candidates(folder: &str, account_paths: &[String]) -> Vec<PathBuf> {
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
        let orphans: Vec<String> = orphan_worktrees_now(folder.clone(), in_use, account_paths, live)?.iter().map(|p| norm(p)).collect();
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

    #[test]
    fn what_a_spawn_left_is_what_is_new_with_nobody_in_it() {
        let now =
            vec![native(r"E:\p\.claude\worktrees\old"), native(r"E:\p\.claude\worktrees\new"), native(r"E:\p\.claude\worktrees\busy")];
        let before = [native(r"E:\p\.claude\worktrees\old")];
        let live = [native(r"E:\p\.claude\worktrees\busy\sub")];
        assert_eq!(left_since(now, &before, &live), [native(r"E:\p\.claude\worktrees\new")]);
    }

    #[test]
    fn a_worktree_a_cli_made_is_found_for_its_folder_and_account() {
        let Some((root, repo, wt)) = codex_layout("made-for") else { return };
        let account = root.join("account").to_string_lossy().into_owned();
        assert_eq!(made_for(&repo, &account), std::slice::from_ref(&wt));
        assert!(made_for(&repo, r"C:\no\such\account").is_empty());
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
}
