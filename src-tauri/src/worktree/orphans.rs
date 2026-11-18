//! The worktrees of a folder no live chat claims, found every minute by reading
//! directories only, and the sweep of those the user picks; and what a spawn cut
//! off left.

use super::*;

/// A worktree younger than this is assumed to belong to a session that has not
/// reported its path yet, and is never treated as an orphan.
const GRACE_SECS: u64 = 300;

/// A live session or helper that asked its CLI for a new worktree whose path
/// Luna has not learned yet: Codex makes its checkout in a bucket of its own
/// choosing (`<CODEX_HOME>/worktrees/<first 4 hex of a fresh UUID>/<repo>`,
/// codex-rs `worktree/src/paths.rs`) and says where only in the rollout, which
/// is written with the first turn; Claude Code given `--worktree` with no
/// name picks one under `<repo>/.claude/worktrees/`, and says which in its
/// registry once it is up. What appears there after it asked may be its own
/// (`awaits`): no orphan while it runs with its path unknown.
#[derive(Clone, Debug)]
pub(crate) struct Unplaced {
    pub provider: crate::provider::Provider,
    /// The project folder it was started in.
    pub folder: String,
    pub account_path: String,
    /// When it was started, on the wall clock (ms): `since`.
    pub since_ms: u64,
}

impl Unplaced {
    /// When a session stamped `stamped_ms` on the wall clock just before its
    /// process was spawned, and `age` ago by the monotonic clock, started, as
    /// a worktree's creation time is compared with it: the earlier of the
    /// stamp and now less its age. A wall clock stepped back since the start
    /// moves the second back with it, one stepped forward leaves the first
    /// where it was, so a worktree it made is not taken for older than it.
    pub(crate) fn since(stamped_ms: u64, age: std::time::Duration) -> u64 {
        stamped_ms.min(crate::throttle::now_ms().saturating_sub(age.as_millis() as u64))
    }
}

/// How long before a session's start a worktree may have appeared and still
/// be taken for its own: the session's stamp is taken just before its process
/// is spawned, but file times are written by another clock's reading, and
/// some file systems keep them only to the second or two.
const BORN_SLACK_MS: u64 = 2_000;

/// When `path` appeared, in ms on the wall clock: its creation time, or where
/// the file system keeps none, its last change.
fn born_ms(path: &Path) -> Option<u64> {
    let m = std::fs::metadata(path).ok()?;
    let at = m.created().or_else(|_| m.modified()).ok()?;
    at.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_millis() as u64)
}

/// Whether the worktree `wt` of `folder` may be the one `u` is making: where
/// its CLI makes one for that folder (Claude Code under the folder, Codex
/// under its account), and appeared since it was started. One whose time
/// cannot be read is taken for it. Folders are compared as places
/// (`Place`), as where a miss is the mistake: a worktree missed here is
/// offered and removed as an orphan.
fn awaits(u: &Unplaced, folder: &str, wt: &Path) -> bool {
    use crate::provider::Provider;
    if !Place::of(&u.folder).is(&Place::of(folder)) {
        return false;
    }
    let root = match u.provider {
        Provider::Claude => wt.parent(),
        Provider::Codex => wt.parent().and_then(Path::parent),
    };
    let place = match u.provider {
        Provider::Claude => claude_worktrees(&u.folder),
        Provider::Codex => Path::new(&u.account_path).join("worktrees"),
    };
    if !root.is_some_and(|r| Place::of(&r.to_string_lossy()).is(&Place::of(&place.to_string_lossy()))) {
        return false;
    }
    born_ms(wt).is_none_or(|born| born >= u.since_ms.saturating_sub(BORN_SLACK_MS))
}

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
    // In this order: a session that learns where its worktree is between the
    // two reads is still unplaced in the first or has its path in the second.
    let unplaced = crate::pty::unplaced_worktrees();
    let live = crate::pty::live_cwds();
    tauri::async_runtime::spawn_blocking(move || orphan_worktrees_now(folder, in_use, account_paths, live, &unplaced, GRACE_SECS))
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

/// The orphans of `folder`: worktrees older than `grace` seconds (`GRACE_SECS`)
/// that no folder in use claims, and that no session making a worktree whose
/// path is not known yet may have made (`Unplaced`).
fn orphan_worktrees_now(
    folder: String,
    in_use: Vec<String>,
    account_paths: Vec<String>,
    live_pty_cwds: Vec<String>,
    unplaced: &[Unplaced],
    grace: u64,
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
        if younger_than(&path, grace) || unplaced.iter().any(|u| awaits(u, &folder, &path)) {
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

/// Whether the orphan `path` of `folder` is to stay after all, asked again
/// under its removal's hold: a live session or helper making a worktree
/// whose path is not known yet may have made it (`awaits`), or one is in it
/// (`PtyManager::worktree_shared`). The makers first, as `orphan_worktrees`
/// reads them.
pub(crate) fn kept_from_removal(folder: &str, path: &str) -> bool {
    crate::pty::unplaced_worktrees().iter().any(|u| awaits(u, folder, Path::new(path))) || crate::pty::manager().worktree_shared("", path)
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
/// off by a core going had begun (agents/spawn/settle.rs `worktrees_left`).
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
    // In this order, as `orphan_worktrees` reads them.
    let unplaced = crate::pty::unplaced_worktrees();
    let live = crate::pty::live_cwds();
    tauri::async_runtime::spawn_blocking(move || {
        let orphans: Vec<String> =
            orphan_worktrees_now(folder.clone(), in_use, account_paths, live, &unplaced, GRACE_SECS)?.iter().map(|p| norm(p)).collect();
        let mut result = SweepResult { removed: vec![], failed: vec![] };
        for path in paths {
            // Held from the look to the end of its removal, as a chat's delete
            // holds its worktree (`removing`): a session or helper on its way
            // in, in it or making one that may be it since the orphans were
            // listed keeps it, and one that comes meanwhile is refused.
            let held = orphans.contains(&norm(&path)).then(|| removing(&path, || kept_from_removal(&folder, &path))).flatten();
            let Some(_removing) = held else {
                result.failed.push(SweepFailure { path, error: "in use again — left alone".into() });
                continue;
            };
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

    /// A worktree that appeared, where its CLI makes them, after a live
    /// session there asked for a new one whose path Luna does not know yet is
    /// no orphan, however old; one there before it asked, another account's
    /// or another folder's is, and so is every one once nobody is waiting.
    #[test]
    fn a_worktree_made_after_a_session_asked_for_one_is_no_orphan_while_its_path_is_unknown() {
        use crate::provider::Provider;
        let Some((root, repo, wt)) = codex_layout("unplaced") else { return };
        let account = root.join("account").to_string_lossy().into_owned();
        let claude_wt = claude_worktrees(&repo).join("named-by-claude");
        std::fs::create_dir_all(&claude_wt).unwrap();
        let claude_wt = claude_wt.to_string_lossy().into_owned();
        let orphans = |unplaced: &[Unplaced]| {
            let mut found: Vec<String> = orphan_worktrees_now(repo.clone(), vec![], vec![account.clone()], vec![], unplaced, 0)
                .unwrap()
                .iter()
                .map(|p| place(p))
                .collect();
            found.sort();
            found
        };
        let mut both = vec![place(&wt), place(&claude_wt)];
        both.sort();
        assert_eq!(orphans(&[]), both, "nobody waiting: both orphans");
        let now = crate::throttle::now_ms();
        let asked = |provider, since_ms: u64, folder: &str, account: &str| Unplaced {
            provider,
            folder: folder.to_string(),
            account_path: account.to_string(),
            since_ms,
        };
        let before = now - 60_000;
        assert_eq!(orphans(&[asked(Provider::Codex, before, &repo, &account)]), [place(&claude_wt)], "Codex's, made after it asked");
        assert_eq!(orphans(&[asked(Provider::Claude, before, &repo, &account)]), [place(&wt)], "Claude Code's, made after it asked");
        assert_eq!(orphans(&[asked(Provider::Codex, now + 60_000, &repo, &account)]), both, "there before it asked");
        assert_eq!(orphans(&[asked(Provider::Codex, before, &repo, &native(r"C:\other\account"))]), both, "another account");
        assert_eq!(orphans(&[asked(Provider::Claude, before, &native(r"C:\other\repo"), &account)]), both, "another folder");
        // Its folder as the session was started in it: through a link, or in
        // another case where the platform takes that for the same folder.
        let linked = root.join("linked-repo");
        link_dir(&linked, Path::new(&repo));
        let linked = linked.to_string_lossy().into_owned();
        assert_eq!(orphans(&[asked(Provider::Claude, before, &linked, &account)]), [place(&wt)], "its folder through a link");
        let cased = if cfg!(any(windows, target_os = "macos")) { vec![place(&wt)] } else { both.clone() };
        assert_eq!(orphans(&[asked(Provider::Claude, before, &repo.to_uppercase(), &account)]), cased, "its folder in another case");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A session's start, as a worktree's creation time is compared with it,
    /// is never after the start: a wall clock stepped back an hour since puts
    /// its stamp in the future, and the monotonic clock brings it back; one
    /// stepped forward leaves the stamp as it was.
    #[test]
    fn a_sessions_start_holds_across_a_wall_clock_step() {
        let now = crate::throttle::now_ms();
        let age = std::time::Duration::from_secs(10);
        let back = Unplaced::since(now + 3_600_000, age);
        assert!(back >= now - 10_000 && back < now - 9_000, "{back} for {}", now - 10_000);
        assert_eq!(Unplaced::since(now - 3_600_000, age), now - 3_600_000);
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
