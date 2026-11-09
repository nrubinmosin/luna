//! The folder a session is to start in, checked before anything of the start
//! is made: by a chat's start (`start`), by the new-chat dialog ahead of
//! Create (`chat_folder_refusal`) and by a helper's spawn (agents/spawn).

use crate::boundary::{AbsPath, AccountPath};
use crate::provider::Provider;
use std::path::Path;

/// Why a session of `provider` on `account_path` cannot start in `folder`,
/// `worktree` saying whether its start asks the CLI to make one; None when
/// nothing stands in the way.
///
/// - The folder is not there: portable-pty would start the CLI in the
///   user's profile instead, without a word.
/// - A worktree is asked for outside a git repository: both CLIs give up at
///   once and leave a pane on a CLI that has exited, Claude Code with "Can
///   only use --worktree in a git repository" (2.1.319), Codex with git's own
///   "not a git repository" (0.167.0). Claude Code makes one outside git
///   through a `WorktreeCreate` hook, so one named in the settings files Luna
///   reads lets the start through.
pub fn folder_refusal(provider: Provider, folder: &str, account_path: &str, worktree: bool) -> Option<String> {
    let dir = Path::new(folder);
    if !dir.is_dir() {
        return Some(if dir.exists() {
            format!("{folder} is a file, not a folder")
        } else {
            format!("the folder {folder} does not exist")
        });
    }
    if worktree && !in_git_repository(dir) && !(provider == Provider::Claude && claude_worktree_hook(account_path, folder)) {
        return Some(format!(
            "{folder} is not in a git repository, so no worktree can be made there: start without one, or in a folder of a repository"
        ));
    }
    None
}

/// Whether `dir` is in a git checkout: it or a folder above it has a `.git`,
/// the repository's folder or a linked worktree's file, as `worktree::head_of`
/// finds a checkout. Read off the disk: no git is run for a dialog's answer.
pub(crate) fn in_git_repository(dir: &Path) -> bool {
    dir.ancestors().any(|d| d.join(".git").exists())
}

/// Whether the settings Claude Code reads for a session in `folder` on
/// `account_path` name a `WorktreeCreate` hook: the account's `settings.json`,
/// the project's `.claude/settings.json` and `.claude/settings.local.json`.
/// A plugin's hooks and managed settings are not read.
fn claude_worktree_hook(account_path: &str, folder: &str) -> bool {
    let project = Path::new(folder).join(".claude");
    [Path::new(account_path).join("settings.json"), project.join("settings.json"), project.join("settings.local.json")]
        .iter()
        .filter_map(|file| std::fs::read_to_string(file).ok())
        .filter_map(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .any(|v| v["hooks"]["WorktreeCreate"].as_array().is_some_and(|hooks| !hooks.is_empty()))
}

/// `folder_refusal` for the new-chat dialog, said before Create rather than by
/// a chat whose CLI exits as it starts. The page's paths are held to their
/// shape and the account to one Luna lists (boundary.rs) first.
#[tauri::command]
pub fn chat_folder_refusal(provider: Provider, account_path: AccountPath, folder: AbsPath, worktree: bool) -> Option<String> {
    folder_refusal(provider, &String::from(folder), &String::from(account_path), worktree)
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
    fn a_folder_that_is_not_there_is_refused_for_either_cli() {
        let dir = scratch("gone");
        let gone = dir.join("luna;C");
        let file = dir.join("a-file");
        std::fs::write(&file, "").unwrap();
        for provider in Provider::ALL {
            let why = folder_refusal(provider, &gone.to_string_lossy(), "", false).expect("refused");
            assert!(why.contains("does not exist"), "{why}");
            assert!(folder_refusal(provider, &file.to_string_lossy(), "", false).is_some_and(|w| w.contains("not a folder")));
            assert_eq!(folder_refusal(provider, &dir.to_string_lossy(), "", false), None);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_worktree_needs_a_git_repository_or_a_claude_code_hook_that_makes_one() {
        let dir = scratch("git");
        let (repo, plain, account) = (dir.join("repo"), dir.join("plain"), dir.join("account"));
        for d in [repo.join(".git"), repo.join("src"), plain.clone(), account.clone()] {
            std::fs::create_dir_all(d).unwrap();
        }
        // A temp folder inside somebody's repository (a home folder kept in
        // git) cannot stand for one outside: the case is left out there.
        let outside = !in_git_repository(&plain);
        let (acc, plain) = (account.to_string_lossy().into_owned(), plain.to_string_lossy().into_owned());
        for provider in Provider::ALL {
            assert_eq!(folder_refusal(provider, &repo.to_string_lossy(), &acc, true), None);
            assert_eq!(folder_refusal(provider, &repo.join("src").to_string_lossy(), &acc, true), None, "anywhere in it");
            assert_eq!(folder_refusal(provider, &plain, &acc, false), None, "no worktree, no repository needed");
            if outside {
                let why = folder_refusal(provider, &plain, &acc, true).expect("refused");
                assert!(why.contains("not in a git repository"), "{why}");
            }
        }
        // A linked worktree's `.git` is a file.
        let linked = dir.join("linked");
        std::fs::create_dir_all(&linked).unwrap();
        std::fs::write(linked.join(".git"), "gitdir: ../repo/.git/worktrees/linked\n").unwrap();
        assert_eq!(folder_refusal(Provider::Codex, &linked.to_string_lossy(), &acc, true), None);
        if outside {
            // Claude Code's WorktreeCreate hook, in the account's settings or
            // the project's, makes a worktree outside git; Codex has none.
            let hook = r#"{"hooks":{"WorktreeCreate":[{"hooks":[{"type":"command","command":"jj workspace add"}]}]}}"#;
            std::fs::write(account.join("settings.json"), hook).unwrap();
            assert_eq!(folder_refusal(Provider::Claude, &plain, &acc, true), None);
            assert!(folder_refusal(Provider::Codex, &plain, &acc, true).is_some());
            std::fs::remove_file(account.join("settings.json")).unwrap();
            let project = Path::new(&plain).join(".claude");
            std::fs::create_dir_all(&project).unwrap();
            std::fs::write(project.join("settings.local.json"), hook).unwrap();
            assert_eq!(folder_refusal(Provider::Claude, &plain, &acc, true), None);
            std::fs::write(project.join("settings.local.json"), r#"{"hooks":{"WorktreeCreate":[]}}"#).unwrap();
            assert!(folder_refusal(Provider::Claude, &plain, &acc, true).is_some(), "an empty list makes nothing");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
