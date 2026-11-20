//! The folder a session is to start in, checked before anything of the start
//! is made: by a chat's start (`start`), by the new-chat dialog ahead of
//! Create (`chat_folder_check`) and by a helper's spawn (agents/spawn).
//!
//! Refused only where Luna can be sure the CLI could not start there.

use crate::boundary::AbsPath;
use serde::Serialize;
use std::path::Path;

/// What a folder check says of a start: refused, with why; and whether the
/// folder is in a git repository, the only place a worktree is offered.
#[derive(Serialize, Default, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FolderCheck {
    pub refusal: Option<String>,
    pub in_git: bool,
}

/// The check of `folder` for a session whose start, if `worktree`, asks its
/// CLI to make one (`--worktree`).
///
/// - The folder is not there: refused. portable-pty would start the CLI in
///   the user's profile instead, without a word.
/// - A worktree is asked for outside a git repository (no `.git` in the
///   folder or above it), which both CLIs give up on at once, leaving a pane
///   on a CLI that has exited: Claude Code with "Can only use --worktree in a
///   git repository" (2.1.319), Codex with git's "not a git repository"
///   (0.167.0). Refused. Claude Code makes a worktree outside git only through
///   a `WorktreeCreate` hook, which Luna does not look for: it offers no
///   worktree there.
pub fn folder_check(folder: &str, worktree: bool) -> FolderCheck {
    let dir = Path::new(folder);
    if !dir.is_dir() {
        let why = if dir.exists() { format!("{folder} is a file, not a folder") } else { format!("the folder {folder} does not exist") };
        return FolderCheck { refusal: Some(why), in_git: false };
    }
    let in_git = in_git_repository(dir);
    let refusal = (worktree && !in_git).then(|| {
        format!("{folder} is not in a git repository, and a worktree needs one: start without one, or in a folder of a repository")
    });
    FolderCheck { refusal, in_git }
}

/// `folder_check`'s refusal alone: what a start or a spawn goes by.
pub fn folder_refusal(folder: &str, worktree: bool) -> Option<String> {
    folder_check(folder, worktree).refusal
}

/// Whether `dir` is in a git checkout: it or a folder above it has a `.git`,
/// the repository's folder or a linked worktree's file. Looked at, not read: no
/// git is run, no file opened.
pub(crate) fn in_git_repository(dir: &Path) -> bool {
    dir.ancestors().any(|d| d.join(".git").exists())
}

/// `folder_check` for the new-chat dialog, said before Create rather than by
/// a chat whose CLI exits as it starts: its refusal disables Create, and
/// whether the folder is in git decides whether the worktree box is offered.
/// The page's path is held to its shape (boundary.rs) first; the look itself
/// runs off the window's thread, as a folder on a slow share may take its time.
pub async fn chat_folder_check(folder: AbsPath, worktree: bool) -> FolderCheck {
    let folder = String::from(folder);
    crate::runtime::spawn_blocking(move || folder_check(&folder, worktree))
        .await
        .unwrap_or_else(|e| FolderCheck { refusal: Some(format!("the folder could not be checked: {e}")), in_git: false })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("luna-folder-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_folder_that_is_not_there_is_refused() {
        let dir = scratch("gone");
        let gone = dir.join("luna;C");
        let file = dir.join("a-file");
        std::fs::write(&file, "").unwrap();
        let why = folder_refusal(&gone.to_string_lossy(), false).expect("refused");
        assert!(why.contains("does not exist"), "{why}");
        assert!(folder_refusal(&file.to_string_lossy(), false).is_some_and(|w| w.contains("not a folder")));
        assert_eq!(folder_refusal(&dir.to_string_lossy(), false), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_worktree_needs_a_git_repository_and_is_offered_only_in_one() {
        let dir = scratch("git");
        let (repo, plain) = (dir.join("repo"), dir.join("plain"));
        for d in [repo.join(".git"), repo.join("src"), plain.clone()] {
            std::fs::create_dir_all(d).unwrap();
        }
        let (repo_s, src_s, plain_s) =
            (repo.to_string_lossy().into_owned(), repo.join("src").to_string_lossy().into_owned(), plain.to_string_lossy().into_owned());
        for folder in [&repo_s, &src_s] {
            assert_eq!(folder_check(folder, true), FolderCheck { refusal: None, in_git: true }, "anywhere in a repository");
        }
        assert_eq!(folder_refusal(&plain_s, false), None, "no worktree, no repository needed");
        // A temp folder inside somebody's repository (a home folder kept in git)
        // cannot stand for one outside: the case is left out there.
        if !in_git_repository(&plain) {
            let check = folder_check(&plain_s, true);
            assert!(!check.in_git && check.refusal.as_deref().is_some_and(|w| w.contains("not in a git repository")), "{check:?}");
            assert!(!folder_check(&plain_s, false).in_git, "and no worktree box there");
        }
        // A linked worktree's `.git` is a file.
        let linked = dir.join("linked");
        std::fs::create_dir_all(&linked).unwrap();
        std::fs::write(linked.join(".git"), "gitdir: ../repo/.git/worktrees/linked\n").unwrap();
        assert!(folder_check(&linked.to_string_lossy(), true).in_git);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
