//! A worktree asked for by name: the argument that asks Claude Code for it and
//! where it lands; whether one is still as its CLI made it, to be gone back into;
//! who is in one; and what a Codex helper cut off may have begun.

use super::*;

/// A name for `--worktree` that is one and can only be read as one: what Claude Code
/// takes (it refuses past 64 characters), and no leading dash. As a separate argument
/// `--dangerously-skip-permissions` is that flag, which turns permissions off.
pub fn name_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with(['-', '.'])
        && name.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
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
    let out = git(folder)
        .args(["reflog", "show", "--format=%H", &format!("refs/heads/{branch}")])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
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
        .args([
            "-c",
            "core.quotepath=off",
            "status",
            "--porcelain",
            "--ignored=matching",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ])
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
    [Some(account_path.to_string()), own_claude_dir()]
        .into_iter()
        .flatten()
        .flat_map(|dir| crate::claude::session::running_cwds(&dir))
        .collect()
}

/// The worktrees Codex has made for this folder on this account since `before`, that
/// nobody is in: all there is to tell what a Codex helper cut off may have begun, as it
/// takes no name and says where only once it has a thread.
pub fn codex_new_since(folder: &str, account_path: &str, before: &[String], claimed: &[String]) -> Vec<String> {
    let mut new: Vec<String> =
        left_since(made_for(folder, account_path), before, claimed).into_iter().filter(|p| codex_shaped(p)).collect();
    new.sort();
    new
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(codex_new_since(&repo, &account, &[], &[]), std::slice::from_ref(&codex));
        assert!(codex_new_since(&repo, &account, std::slice::from_ref(&codex), &[]).is_empty());
        assert!(codex_new_since(&repo, &account, &[], std::slice::from_ref(&codex)).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
