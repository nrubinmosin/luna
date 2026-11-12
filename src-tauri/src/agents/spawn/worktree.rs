//! The worktrees spawns run in: a caller's handed on, one made for a helper
//! and named by Luna, and what is left of one a helper began.

use crate::agents::list::disk_meta;
use crate::agents::registry::{chat_name, label, reg, Known};
use serde::Serialize;

/// Where a session in a worktree works: the path, and the branch and commit
/// there.
#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Worktree {
    #[serde(rename = "worktree", skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
}

/// Where a session's CLI works now, as far as Luna can say.
fn working_dir_of(pty: &crate::pty::PtyManager, id: &str) -> Option<String> {
    if crate::background::has(id) {
        crate::background::made_worktree(id).or_else(|| crate::background::working_dir(id))
    } else {
        pty.probe(id).and_then(|p| p.working_dir())
    }
}

/// The worktree a session works in — the one it made or the one it was
/// handed — with its branch; nothing for one in a plain folder. Its root,
/// wherever in it the CLI says it is now.
pub(in crate::agents) fn worktree_of(pty: &crate::pty::PtyManager, id: &str) -> Worktree {
    let Some((dir, _)) = working_dir_of(pty, id).and_then(|d| crate::worktree::root_of(&d)) else {
        return Worktree::default();
    };
    let head = crate::worktree::head_at(&dir).unwrap_or_default();
    Worktree { branch: head.branch, sha: head.sha, path: Some(dir) }
}

/// `worktree_of`, and for a session in a plain folder the branch and commit there:
/// what a caller running several needs to tell where each of them has got to.
pub(in crate::agents) fn checkout_of(pty: &crate::pty::PtyManager, id: &str, k: &Known) -> Worktree {
    let mut worktree = worktree_of(pty, id);
    if worktree.path.is_none() {
        let head = crate::worktree::head_of(&working_dir_of(pty, id).unwrap_or_else(|| k.folder.clone())).unwrap_or_default();
        worktree.branch = head.branch;
        worktree.sha = head.sha;
    }
    worktree
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Neighbor {
    pub id: String,
    pub name: String,
    pub busy: bool,
}

/// Live sessions other than `skip` that run in `dir`, by what the CLI says
/// its cwd is: the project folder for a chat in it, the worktree for one in
/// a worktree.
pub(in crate::agents) fn neighbors_in(dir: &str, skip: &[&str]) -> Vec<Neighbor> {
    let ids: Vec<String> = reg().lock().unwrap_or_else(|e| e.into_inner()).known.keys().cloned().collect();
    let pty = crate::pty::manager();
    let mut out = Vec::new();
    for id in ids.into_iter().filter(|id| !skip.contains(&id.as_str())) {
        let cwd = match (crate::background::working_dir(&id), pty.probe(&id)) {
            (Some(dir), _) => dir,
            (None, Some(probe)) => disk_meta(&probe).and_then(|m| m.cwd).unwrap_or_else(|| probe.cwd.clone()),
            (None, None) => continue,
        };
        if crate::pty::same_dir(&cwd, dir) {
            out.push(Neighbor {
                busy: crate::activity::turn_state(&id).is_some_and(|t| t.busy),
                name: chat_name(&id).unwrap_or_default(),
                id,
            });
        }
    }
    out
}

/// The worktree a caller started with `--worktree` runs in. It registers
/// with the project folder, the only one known at spawn, and the CLI makes
/// the worktree itself; what it leaves on disk says where that went.
pub(in crate::agents) fn caller_worktree(caller: &str) -> Option<String> {
    let folder = reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(caller)?.folder.clone();
    let cwd = match crate::background::working_dir(caller) {
        Some(dir) => dir,
        None => crate::pty::manager().probe(caller)?.working_dir()?,
    };
    // Its root: the caller's shell may be in a folder of it.
    let (root, _) = crate::worktree::root_of(&cwd)?;
    crate::worktree::is_worktree_of(&folder, &root).then_some(root)
}

/// One helper's worktree made at a time per repository. Each CLI makes its own
/// with `git worktree add` as its first turn starts, and two of those in one
/// repository at once fail on each other ("unable to stat just-written file",
/// seen with parallel spawns). Keyed by the repository (`repository_key`), so a
/// spawn from the main checkout and one from a worktree of it wait in one line;
/// a folder in no repository Luna can read waits by its own path.
pub(in crate::agents) fn worktree_queue(folder: &str) -> std::sync::Arc<std::sync::Mutex<()>> {
    type Queues = std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<std::sync::Mutex<()>>>>;
    static QUEUES: std::sync::OnceLock<Queues> = std::sync::OnceLock::new();
    let key = crate::worktree::repository_key(folder).unwrap_or_else(|| crate::paths::path_key(folder));
    let mut queues = QUEUES.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    // Those nobody holds or waits on any more.
    queues.retain(|_, q| std::sync::Arc::strong_count(q) > 1);
    queues.entry(key).or_default().clone()
}

/// What Luna names the worktree of a Claude Code helper it spawns, and so knows to be
/// that helper's: nothing else goes by this name, where a worktree merely new since the
/// spawn may be anybody's. Claude Code takes it by `--worktree=<name>`, makes it if it
/// is not there and goes back in if it is. Codex takes no name.
pub fn helper_worktree_name(id: &str) -> String {
    format!("luna-{id}")
}

/// What a helper that has no conversation asks for, its worktree named `name`: that
/// name if nothing is there, as the CLI then makes it, or if what is there is still as
/// the CLI made it with nobody in it (`worktree::as_made`), as going back in is then the
/// same as starting in a new one. Anything else is left alone, its parent hears of it
/// (`worktrees_left_line`), and the helper takes the next name. None past a few of those:
/// the CLI names its own.
pub fn name_to_go_on(id: &str, name: &str, folder: &str, account_path: &str) -> Option<String> {
    let mut busy = crate::pty::live_cwds();
    busy.extend(crate::worktree::claude_sessions_running(account_path));
    let mut left = Vec::new();
    let mut chosen = None;
    for n in 1..=9 {
        let candidate = if n == 1 { name.to_string() } else { format!("{name}-{n}") };
        let path = crate::worktree::named_path(folder, &candidate);
        let there = std::path::Path::new(&path).exists();
        if there && (!crate::worktree::as_made(folder, &path) || crate::worktree::someone_in(&path, &busy)) {
            left.push(path);
            continue;
        }
        if there {
            crate::log::warn("agents", &format!("{} goes back into {path}, which a core cut it off in", label(id)));
        }
        chosen = Some(candidate);
        break;
    }
    leave_worktrees(id, left);
    chosen.filter(|n| crate::worktree::name_ok(n))
}

/// A Codex helper starting afresh: what is new of Codex's since it was spawned may be
/// what a core cut it off making, and its parent hears. It is not gone back into, as
/// nothing says whose it is: Codex takes no name, and says where it works only with a
/// thread, which is then a conversation to resume. Ahead of what this start makes.
pub fn report_codex_worktrees(id: &str, folder: &str, account_path: &str) {
    let Some(before) = reg().lock().unwrap_or_else(|e| e.into_inner()).worktrees_before.remove(id) else { return };
    leave_worktrees(id, crate::worktree::codex_new_since(folder, account_path, &before, &crate::pty::live_cwds()));
}

fn leave_worktrees(id: &str, left: Vec<String>) {
    if left.is_empty() {
        return;
    }
    crate::log::warn("agents", &format!("{} leaves what a core cut it off in: {}", label(id), left.join(", ")));
    {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let told = r.worktrees_left.entry(id.to_string()).or_default();
        for path in left {
            if !told.contains(&path) {
                told.push(path);
            }
        }
    }
    crate::background::save();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::lost::{undelivered_note, worktrees_left_line};
    use crate::agents::owed::hand_over;
    use crate::agents::persist::{tie_back, ties_in};
    use crate::agents::registry::{reg, register};
    use crate::agents::spawn::Spawned;
    use crate::agents::teardown::forget;
    use crate::provider::Provider;

    #[test]
    fn a_worktree_still_being_made_says_so() {
        let spawned = |worktree: Worktree, worktree_pending: bool| {
            serde_json::to_value(Spawned {
                account_choice: None,
                id: "w".into(),
                turns_ended: 0,
                settings: serde_json::Value::Null,
                same_folder: Vec::new(),
                worktree,
                worktree_pending,
                mcp_left_on: Vec::new(),
            })
            .unwrap()
        };
        let pending = spawned(Worktree::default(), true);
        assert_eq!(pending["worktreePending"], true);
        assert!(pending.get("worktree").is_none());
        let made = spawned(Worktree { path: Some("C:\\w".into()), ..Default::default() }, false);
        assert_eq!(made["worktree"], "C:\\w");
        assert!(made.get("worktreePending").is_none());
    }

    /// A throwaway repository with worktrees made as Claude Code makes them.
    fn repo_with(tag: &str, worktrees: &[&str]) -> Option<(std::path::PathBuf, String, Vec<String>)> {
        let root = std::env::temp_dir().join(format!("luna-rw-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).ok()?;
        // As a user's temp folder may be: git's own say of it is in long names.
        let root = crate::worktree::short_spelling(&root);
        let repo = root.to_string_lossy().into_owned();
        let git = |dir: &str, args: &[&str]| {
            let mut c = std::process::Command::new("git");
            c.arg("-C").arg(dir).args(["-c", "user.email=t@t", "-c", "user.name=t"]).args(args);
            c.output().is_ok_and(|o| o.status.success())
        };
        if !git(&repo, &["init", "-q"]) || !git(&repo, &["commit", "-q", "--allow-empty", "-m", "i"]) {
            return None;
        }
        let mut made = Vec::new();
        for name in worktrees {
            let wt = crate::worktree::named_path(&repo, name);
            if !git(&repo, &["worktree", "add", "-q", "-b", &format!("worktree-{name}"), &wt]) {
                return None;
            }
            made.push(wt);
        }
        Some((root, repo, made))
    }

    /// Spawns making worktrees in one repository wait in one line, from its main
    /// checkout or a worktree of it alike; another repository's do not wait on
    /// them. Under the line, worktrees made side by side all come out.
    #[test]
    fn worktrees_are_made_one_at_a_time_per_repository() {
        let Some((root, repo, made)) = repo_with("queue", &["luna-q"]) else { return };
        let Some((other_root, other, _)) = repo_with("queue-other", &[]) else { return };
        let (main, from_worktree, elsewhere) = (worktree_queue(&repo), worktree_queue(&made[0]), worktree_queue(&other));
        assert!(std::sync::Arc::ptr_eq(&main, &from_worktree), "one repository, one line");
        assert!(!std::sync::Arc::ptr_eq(&main, &elsewhere), "another repository's is its own");
        // Held, the next in that repository waits; another repository's does not.
        let held = main.lock().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let waiter = {
            let (repo, tx) = (repo.clone(), tx.clone());
            std::thread::spawn(move || {
                let q = worktree_queue(&repo);
                let _turn = q.lock().unwrap();
                tx.send("repo").unwrap();
            })
        };
        let _other_turn = elsewhere.lock().unwrap();
        assert!(rx.recv_timeout(std::time::Duration::from_millis(300)).is_err(), "it waits for the one at work");
        drop(held);
        assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(10)), Ok("repo"));
        waiter.join().unwrap();
        // As spawns side by side would: each `git worktree add` under the line.
        let adds: Vec<_> = (0..4)
            .map(|i| {
                let repo = repo.clone();
                std::thread::spawn(move || {
                    let q = worktree_queue(&repo);
                    let _turn = q.lock().unwrap();
                    let wt = crate::worktree::named_path(&repo, &format!("luna-par-{i}"));
                    let mut c = std::process::Command::new("git");
                    c.arg("-C").arg(&repo).args(["worktree", "add", "-q", "-b", &format!("worktree-luna-par-{i}"), &wt]);
                    c.output().is_ok_and(|o| o.status.success())
                })
            })
            .collect();
        assert!(adds.into_iter().all(|t| t.join().unwrap()), "every worktree made");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&other_root);
    }

    /// A Claude Code helper a core cut off in its first turn starts afresh in the
    /// worktree Luna named for it only while that is as the CLI made it.
    #[test]
    fn a_fresh_start_goes_back_only_into_the_worktree_luna_named_and_only_as_made() {
        let Some((root, repo, made)) = repo_with("reuse", &["luna-a", "luna-b", "someone-elses"]) else { return };
        let (a, b, elses) = (&made[0], &made[1], &made[2]);
        let background = serde_json::json!({ "background": true });
        let helper = |id: &str| {
            register(id, Provider::Claude, &repo, "C:\\no\\such\\account", Some("rw-parent"), false, background.clone());
        };
        let start = |id: &str, name: &str| name_to_go_on(id, name, &repo, "C:\\no\\such\\account");

        // Its own, as made. The clean one beside it, new as it is, is nobody's to take.
        helper("rw-a");
        assert_eq!(start("rw-a", "luna-a").as_deref(), Some("luna-a"));
        assert_eq!(worktrees_left_line("rw-a"), "");
        // Not there: Claude Code makes it by that name.
        helper("rw-new");
        assert_eq!(start("rw-new", "luna-new").as_deref(), Some("luna-new"));

        // Something in it: left alone, told of, and the next name from here on.
        std::fs::write(std::path::Path::new(b).join("half-done.txt"), "work").unwrap();
        helper("rw-b");
        assert_eq!(start("rw-b", "luna-b").as_deref(), Some("luna-b-2"));
        let said = worktrees_left_line("rw-b");
        assert!(
            said.contains("worktreeLeft") && said.contains(b.as_str()) && !said.contains(elses.as_str()) && !said.contains(a.as_str()),
            "{said}"
        );
        assert_eq!(start("rw-b", "luna-b").as_deref(), Some("luna-b-2"));
        assert_eq!(worktrees_left_line("rw-b"), said, "the one it left is not told of twice");
        // A name that would be a flag never goes out as one.
        helper("rw-flag");
        assert_eq!(start("rw-flag", "--dangerously-skip-permissions"), None);

        // Told once: with what is told first, or with a read; a cancel brings it back.
        let note = undelivered_note("rw-parent", "rw-b").unwrap();
        assert!(note.starts_with("worktreeLeft") && note.contains(b.as_str()), "{note}");
        assert!(undelivered_note("rw-nobody", "rw-b").is_none());
        let undo = hand_over("rw-parent", "rw-b", 1, None);
        assert_eq!(worktrees_left_line("rw-b"), "");
        undo();
        assert!(worktrees_left_line("rw-b").contains(b.as_str()));

        // Written down, it comes back with the helper.
        let ties = {
            let r = reg().lock().unwrap();
            ties_in(&r, "rw-b").unwrap()
        };
        assert_eq!(ties.worktrees_left, std::slice::from_ref(b));
        forget("rw-b");
        tie_back("rw-b", Provider::Claude, &repo, "C:\\no\\such\\account", ties);
        assert_eq!(reg().lock().unwrap().worktrees_left["rw-b"], std::slice::from_ref(b));
        for id in ["rw-a", "rw-new", "rw-b", "rw-flag"] {
            forget(id);
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
