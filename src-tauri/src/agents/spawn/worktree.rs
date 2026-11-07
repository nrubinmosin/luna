//! The worktrees spawns run in: a caller's handed on, one made for a helper
//! and named by Luna, and what is left of one a helper began.

use crate::agents::list::disk_meta;
use crate::agents::registry::{chat_name, label, reg, Known};
use crate::provider::Provider;
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

/// What Luna names the worktree of a Claude Code helper it spawns, and so knows to be
/// that helper's: nothing else goes by this name, where a worktree merely new since the
/// spawn may be anybody's. Claude Code takes it by `--worktree=<name>`, makes it if it
/// is not there and goes back in if it is. Codex takes no name.
pub fn helper_worktree_name(id: &str) -> String {
    format!("luna-{id}")
}

/// The name a visible helper's CLI is to ask for, starting on no prompt of its own:
/// in a conversation the one it has, afresh the one it is still to go by. None for a
/// session Luna named none for: its CLI names its own, as a chat of a person's does.
pub fn worktree_for_start(id: &str, folder: &str, account_path: &str, conversation: bool) -> Option<String> {
    let name = reg().lock().unwrap_or_else(|e| e.into_inner()).worktree_names.get(id).cloned()?;
    if conversation {
        return Some(name);
    }
    let name = name_to_go_on(id, &name, folder, account_path);
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    match &name {
        Some(name) => r.worktree_names.insert(id.to_string(), name.clone()),
        None => r.worktree_names.remove(id),
    };
    name
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

/// `id`'s CLI is not back to go on in any of it: its parent hears of what it began, and
/// opened after that it makes its own.
pub(in crate::agents) fn leave_half_made(id: &str, k: &Known) {
    if k.provider == Provider::Codex {
        return report_codex_worktrees(id, &k.folder, &k.account_path);
    }
    let Some(name) = reg().lock().unwrap_or_else(|e| e.into_inner()).worktree_names.get(id).cloned() else { return };
    let path = crate::worktree::named_path(&k.folder, &name);
    if std::path::Path::new(&path).exists() {
        reg().lock().unwrap_or_else(|e| e.into_inner()).worktree_names.insert(id.to_string(), format!("{name}-2"));
        leave_worktrees(id, vec![path]);
    }
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
    use crate::agents::persist::ties_in;
    use crate::agents::registry::{reg, register};
    use crate::agents::spawn::Spawned;
    use crate::agents::teardown::forget;
    use crate::agents::tests::{cut_short_chat, take_up};

    #[test]
    fn a_worktree_still_being_made_says_so() {
        let spawned = |worktree: Worktree, worktree_pending: bool| {
            serde_json::to_value(Spawned {
                account_choice: None,
                id: "w".into(),
                turns_ended: 0,
                settings: serde_json::Value::Null,
                started: true,
                screen: None,
                ask: None,
                starting: false,
                warnings: Vec::new(),
                same_folder: Vec::new(),
                worktree,
                worktree_pending,
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

    /// A visible helper its name was given, as `spawn_is` gives it.
    fn named_helper(id: &str, repo: &str, name: &str) {
        register(id, Provider::Claude, repo, "C:\\no\\such\\account", Some("rw-parent"), false, serde_json::json!({}));
        reg().lock().unwrap().worktree_names.insert(id.to_string(), name.to_string());
    }

    #[test]
    fn a_fresh_start_goes_back_only_into_the_worktree_luna_named_and_only_as_made() {
        let Some((root, repo, made)) = repo_with("reuse", &["luna-a", "luna-b", "someone-elses"]) else { return };
        let (a, b, elses) = (&made[0], &made[1], &made[2]);
        let start = |id: &str| worktree_for_start(id, &repo, "C:\\no\\such\\account", false);

        // Its own, as made. The clean one beside it, new as it is, is nobody's to take.
        named_helper("rw-a", &repo, "luna-a");
        assert_eq!(start("rw-a").as_deref(), Some("luna-a"));
        assert_eq!(worktrees_left_line("rw-a"), "");
        // Not there: Claude Code makes it by that name.
        named_helper("rw-new", &repo, "luna-new");
        assert_eq!(start("rw-new").as_deref(), Some("luna-new"));
        // Named none, a person's chat: its CLI names its own, and nothing is taken.
        register("rw-none", Provider::Claude, &repo, "C:\\no\\such\\account", None, false, serde_json::json!({}));
        assert_eq!(start("rw-none"), None);

        // Something in it: left alone, told of, and the next name from here on.
        std::fs::write(std::path::Path::new(b).join("half-done.txt"), "work").unwrap();
        named_helper("rw-b", &repo, "luna-b");
        assert_eq!(start("rw-b").as_deref(), Some("luna-b-2"));
        let said = worktrees_left_line("rw-b");
        assert!(
            said.contains("worktreeLeft") && said.contains(b.as_str()) && !said.contains(elses.as_str()) && !said.contains(a.as_str()),
            "{said}"
        );
        assert_eq!(reg().lock().unwrap().worktree_names["rw-b"], "luna-b-2");
        assert_eq!(start("rw-b").as_deref(), Some("luna-b-2"));
        assert_eq!(worktrees_left_line("rw-b"), said, "the one it left is not told of twice");
        // In a conversation it is its own, whatever is in it.
        named_helper("rw-talk", &repo, "luna-b");
        assert_eq!(worktree_for_start("rw-talk", &repo, "C:\\no\\such\\account", true).as_deref(), Some("luna-b"));
        // A name that would be a flag never goes out as one.
        named_helper("rw-flag", &repo, "--dangerously-skip-permissions");
        assert_eq!(start("rw-flag"), None);

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
        assert_eq!(ties.worktree_name.as_deref(), Some("luna-b-2"));
        let mut chat = cut_short_chat("rw-back", "rw-parent", false, true, false);
        chat.ties.worktree_name = Some("luna-back".into());
        chat.ties.worktrees_left = vec!["b".into()];
        take_up(vec![chat], Vec::new(), Vec::new());
        {
            let r = reg().lock().unwrap();
            assert_eq!(r.worktree_names["rw-back"], "luna-back");
            assert_eq!(r.worktrees_left["rw-back"], ["b"]);
        }
        for id in ["rw-a", "rw-new", "rw-none", "rw-b", "rw-talk", "rw-flag", "rw-back"] {
            forget(id);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_helper_not_back_has_what_luna_named_told_of_and_then_makes_another() {
        let Some((root, repo, made)) = repo_with("notback", &["luna-gone"]) else { return };
        named_helper("nb-chat", &repo, "luna-gone");
        let k = reg().lock().unwrap().known["nb-chat"].clone();
        leave_half_made("nb-chat", &k);
        assert!(worktrees_left_line("nb-chat").contains(made[0].as_str()));
        assert_eq!(reg().lock().unwrap().worktree_names["nb-chat"], "luna-gone-2");
        forget("nb-chat");
        let _ = std::fs::remove_dir_all(&root);
    }
}
