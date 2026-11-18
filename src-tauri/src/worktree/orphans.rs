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
/// registry once it is up. Any worktree of its folder where its CLI makes them
/// may be its own (`Hold`, `Look::Sweep`): no orphan there while it runs with its path
/// unknown, whenever it appeared. Told by when, a wall clock stepped back
/// while the CLI made it and put right before Luna looked would date it
/// before the session's start, and the sweep would take it.
#[derive(Clone, Debug)]
pub(crate) struct Unplaced {
    pub provider: crate::provider::Provider,
    /// The project folder it was started in.
    pub folder: String,
    pub account_path: String,
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
    let held = luna_holds();
    let registries = registries(&account_paths);
    tauri::async_runtime::spawn_blocking(move || orphan_worktrees_now(&folder, &in_use, &account_paths, &registries, &held, GRACE_SECS))
        .await
        .map_err(|e| e.to_string())?
}

/// What Luna's own starts and sessions hold (`Hold`): the starts on their
/// way in first, then the sessions and helpers listed, so a start listed
/// between the two reads is in one of them.
fn luna_holds() -> Hold {
    let mut held = entering_now();
    held.add(&crate::pty::holds(None), true);
    held
}

/// The Claude Code registries to read for sessions in a worktree: each
/// account's, and the user's own (sessions started in a terminal).
fn registries(account_paths: &[String]) -> Vec<String> {
    account_paths.iter().cloned().chain(own_claude_dir()).collect()
}

/// What keeps a folder's worktrees from the sweep: `held` (Luna's own,
/// `luna_holds`), every chat's worktree the window knows (`in_use`), and
/// each live session in a Claude Code `registries`' (Claude Code moves into
/// its worktree on its own; only its registry says where), its process still
/// there: a killed one leaves its entry behind.
fn keepers(in_use: &[String], registries: &[String], held: &Hold) -> Hold {
    let mut keepers = held.clone();
    let mut paths = in_use.to_vec();
    for registry in registries {
        paths.extend(crate::claude::session::running_cwds(registry));
    }
    paths.retain(|p| !p.is_empty());
    paths.sort();
    paths.dedup();
    for p in &paths {
        keepers.add_path(p);
    }
    keepers
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
/// that nobody may be in and no session making a worktree whose path is not
/// known yet may have made (`keepers`).
fn orphan_worktrees_now(
    folder: &str,
    in_use: &[String],
    account_paths: &[String],
    registries: &[String],
    held: &Hold,
    grace: u64,
) -> Result<Vec<String>, String> {
    let keepers = keepers(in_use, registries, held);
    let here = Place::of(folder);
    let mut out: Vec<String> = candidates(folder, account_paths)
        .into_iter()
        .map(|path| path.to_string_lossy().into_owned())
        .filter(|path| !younger_than(Path::new(path), grace) && !keepers.keeps(&Target::of(path, Some(here.clone())), Look::Sweep))
        .collect();
    out.sort();
    Ok(out)
}

/// Whether the orphan `path` of `folder` is to stay after all: the sweep's
/// last look, under its removal's hold (`removing`), at what the listing
/// looked at, Luna's own read anew (`luna_holds`).
pub(crate) fn kept_from_removal(folder: &str, path: &str, in_use: &[String], registries: &[String]) -> bool {
    keepers(in_use, registries, &luna_holds()).keeps(&Target::of(path, Some(Place::of(folder))), Look::Sweep)
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
    let held = luna_holds();
    let registries = registries(&account_paths);
    tauri::async_runtime::spawn_blocking(move || {
        remove_orphans_now(&folder, &in_use, &account_paths, &registries, paths, &held, GRACE_SECS)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// `remove_orphan_worktrees` on what it read of Luna's own first (`held`),
/// which the last look before each removal reads anew.
pub(crate) fn remove_orphans_now(
    folder: &str,
    in_use: &[String],
    account_paths: &[String],
    registries: &[String],
    paths: Vec<String>,
    held: &Hold,
    grace: u64,
) -> Result<SweepResult, String> {
    let orphans: Vec<String> =
        orphan_worktrees_now(folder, in_use, account_paths, registries, held, grace)?.iter().map(|p| norm(p)).collect();
    let mut result = SweepResult { removed: vec![], failed: vec![] };
    for path in paths {
        // Held from the look to the end of its removal, as a chat's delete
        // holds its worktree (`removing`): a session or helper on its way
        // in, in it or making one that may be it since the orphans were
        // listed keeps it, and one that comes meanwhile is refused.
        let held = orphans
            .contains(&norm(&path))
            .then(|| {
                removing(Target::of(&path, Some(Place::of(folder))), Look::Sweep, || kept_from_removal(folder, &path, in_use, registries))
            })
            .flatten();
        let Some(_removing) = held else {
            result.failed.push(SweepFailure { path, error: IN_USE_AGAIN.into() });
            continue;
        };
        match remove_worktree_now(folder.to_string(), path.clone()) {
            Ok(()) => result.removed.push(path),
            Err(error) => {
                crate::log::warn("worktree", &format!("could not remove {path}: {error}"));
                result.failed.push(SweepFailure { path, error });
            }
        }
    }
    Ok(result)
}

/// What the sweep says of an orphan it kept after all.
pub(crate) const IN_USE_AGAIN: &str = "in use again — left alone";

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

    /// A worktree where its CLI makes them, while a live session there asked
    /// for a new one whose path Luna does not know yet, is no orphan, however
    /// old and whenever it appeared: it was there before the session started
    /// here, as one made under a wall clock stepped back would look. Another
    /// account's or another folder's is, and so is every one once nobody is
    /// waiting.
    #[test]
    fn a_worktree_where_a_session_making_one_has_it_made_is_no_orphan_while_its_path_is_unknown() {
        use crate::provider::Provider;
        let Some((root, repo, wt)) = codex_layout("unplaced") else { return };
        let account = root.join("account").to_string_lossy().into_owned();
        let claude_wt = claude_worktrees(&repo).join("named-by-claude");
        std::fs::create_dir_all(&claude_wt).unwrap();
        let claude_wt = claude_wt.to_string_lossy().into_owned();
        let orphans = |unplaced: Option<Unplaced>| {
            let held = Hold::start(&[], unplaced.as_ref());
            let accounts = std::slice::from_ref(&account);
            let mut found: Vec<String> =
                orphan_worktrees_now(&repo, &[], accounts, &[], &held, 0).unwrap().iter().map(|p| place(p)).collect();
            found.sort();
            found
        };
        let mut both = vec![place(&wt), place(&claude_wt)];
        both.sort();
        assert_eq!(orphans(None), both, "nobody waiting: both orphans");
        let asked = |provider, folder: &str, account: &str| {
            Some(Unplaced { provider, folder: folder.to_string(), account_path: account.to_string() })
        };
        assert_eq!(orphans(asked(Provider::Codex, &repo, &account)), [place(&claude_wt)], "Codex's");
        assert_eq!(orphans(asked(Provider::Claude, &repo, &account)), [place(&wt)], "Claude Code's");
        assert_eq!(orphans(asked(Provider::Codex, &repo, &native(r"C:\other\account"))), both, "another account");
        assert_eq!(orphans(asked(Provider::Claude, &native(r"C:\other\repo"), &account)), both, "another folder");
        // Its folder as the session was started in it: through a link, or in
        // another case where the platform takes that for the same folder.
        let linked = root.join("linked-repo");
        link_dir(&linked, Path::new(&repo));
        let linked = linked.to_string_lossy().into_owned();
        assert_eq!(orphans(asked(Provider::Claude, &linked, &account)), [place(&wt)], "its folder through a link");
        let cased = if cfg!(any(windows, target_os = "macos")) { vec![place(&wt)] } else { both.clone() };
        assert_eq!(orphans(asked(Provider::Claude, &repo.to_uppercase(), &account)), cased, "its folder in another case");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A worktree in use by another spelling of its place than the sweep's
    /// is no orphan, and is kept by the last look before its removal: a chat
    /// parked in it by its real path while the project is swept through a
    /// link, Luna's own session in it through the link while the sweep goes
    /// by the real path, a Claude Code session in a terminal there.
    #[test]
    fn a_worktree_in_use_by_another_spelling_is_no_orphan_and_is_kept_from_removal() {
        let root = std::env::temp_dir().join(format!("luna-wt-spelled-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let (real, link) = (root.join("real"), root.join("link"));
        std::fs::create_dir_all(claude_worktrees(&s(&real)).join("w")).unwrap();
        link_dir(&link, &real);
        let wt = |folder: &Path| claude_worktrees(&s(folder)).join("w");
        let terminal = root.join("terminal");
        terminal_session(&terminal, std::process::id(), &s(&wt(&link)));
        let orphans = |folder: &Path, in_use: &[PathBuf], registries: &[&Path], live: &[PathBuf]| {
            let strings = |ps: &[PathBuf]| ps.iter().map(|p| s(p)).collect::<Vec<_>>();
            let registries: Vec<String> = registries.iter().map(|p| s(p)).collect();
            let mut held = Hold::default();
            for p in strings(live) {
                held.add_path(&p);
            }
            orphan_worktrees_now(&s(folder), &strings(in_use), &[], &registries, &held, 0).unwrap()
        };
        assert_eq!(orphans(&link, &[], &[], &[]), [s(&wt(&link))], "nobody in it: an orphan");
        assert!(orphans(&link, &[wt(&real)], &[], &[]).is_empty(), "a chat parked in it by its real path");
        assert!(orphans(&real, &[], &[], &[wt(&link).join("src")]).is_empty(), "a session in it through the link");
        assert!(orphans(&real, &[], &[&terminal], &[]).is_empty(), "a session in a terminal through the link");
        let (wt_link, in_use) = (s(&wt(&link)), [s(&wt(&real))]);
        assert!(!kept_from_removal(&s(&link), &wt_link, &[], &[]), "nobody in it");
        assert!(kept_from_removal(&s(&link), &wt_link, &in_use, &[]), "the last look asks the chats too");
        assert!(kept_from_removal(&s(&real), &s(&wt(&real)), &[], &[s(&terminal)]), "and the terminals' sessions");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A registry entry of a Claude Code session in a terminal, as its file
    /// is laid out: by its pid, its cwd.
    fn terminal_session(registry: &Path, pid: u32, cwd: &str) {
        std::fs::create_dir_all(registry.join("sessions")).unwrap();
        let entry = serde_json::json!({ "pid": pid, "cwd": cwd });
        std::fs::write(registry.join("sessions").join(format!("{pid}.json")), entry.to_string()).unwrap();
    }

    /// A registry entry whose process is gone keeps nothing: one a killed
    /// Claude Code left behind would keep its worktree from the sweep for
    /// good, and every worktree where its cwd does not read.
    #[test]
    fn a_registry_entry_whose_process_is_gone_keeps_no_worktree() {
        let root = std::env::temp_dir().join(format!("luna-wt-stale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let folder = root.join("project");
        let wt = claude_worktrees(&s(&folder)).join("w");
        std::fs::create_dir_all(&wt).unwrap();
        // A process that has been and gone.
        let mut gone = if cfg!(windows) { Command::new("cmd") } else { Command::new("true") };
        if cfg!(windows) {
            gone.args(["/d", "/c", "exit"]);
        }
        let mut child = gone.spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        let registry = root.join("registry");
        terminal_session(&registry, pid, &s(&wt.join("src")));
        let registries = [s(&registry)];
        let found = orphan_worktrees_now(&s(&folder), &[], &[], &registries, &Hold::default(), 0).unwrap();
        assert_eq!(found, [s(&wt)], "its entry left behind keeps nothing");
        terminal_session(&registry, std::process::id(), &s(&wt.join("src")));
        let found = orphan_worktrees_now(&s(&folder), &[], &[], &registries, &Hold::default(), 0).unwrap();
        assert!(found.is_empty(), "one still running keeps it");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A start asking its CLI for a new worktree, on its way in and its
    /// session not listed yet, keeps what its CLI may have made already from
    /// the sweep's last look, as from its listing (`luna_holds`).
    #[test]
    fn the_sweeps_last_look_keeps_what_a_start_on_its_way_in_may_have_made() {
        use crate::provider::Provider;
        let root = std::env::temp_dir().join(format!("luna-wt-entering-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let folder = root.join("project").to_string_lossy().into_owned();
        let wt = claude_worktrees(&folder).join("picked");
        std::fs::create_dir_all(&wt).unwrap();
        let wt = wt.to_string_lossy().into_owned();
        assert!(!kept_from_removal(&folder, &wt, &[], &[]), "nobody making one");
        let making = Unplaced { provider: Provider::Claude, folder: folder.clone(), account_path: "acc".into() };
        let on_the_way = entering(std::sync::Arc::new(Hold::start(&[&folder], Some(&making)))).unwrap();
        assert!(kept_from_removal(&folder, &wt, &[], &[]), "kept for the start making one");
        drop(on_the_way);
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
