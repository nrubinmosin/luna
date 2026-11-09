//! The folder a session is to start in, checked before anything of the start
//! is made: by a chat's start (`start`), by the new-chat dialog ahead of
//! Create (`chat_folder_check`) and by a helper's spawn (agents/spawn).
//!
//! Refused only where Luna can be sure the CLI could not start there; where a
//! source Luna does not read could still make it work, the start goes ahead
//! and the dialog says what may happen.

use crate::boundary::{AbsPath, AccountPath};
use crate::provider::Provider;
use serde::Serialize;
use std::path::Path;

/// What a folder check says of a start: refused, with why; or let go, with a
/// word for the person when it may still fail.
#[derive(Serialize, Default, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FolderCheck {
    pub refusal: Option<String>,
    pub warning: Option<String>,
}

/// The check of `folder` for a session of `provider` on `account_path`,
/// `worktree` saying whether its start asks the CLI to make one.
///
/// - The folder is not there: refused. portable-pty would start the CLI in
///   the user's profile instead, without a word.
/// - A worktree is asked for outside a git repository (no `.git` in the
///   folder or above it), which both CLIs give up on at once, leaving a pane
///   on a CLI that has exited: Claude Code with "Can only use --worktree in a
///   git repository" (2.1.319), Codex with git's "not a git repository"
///   (0.167.0). Codex makes its worktree with git alone: refused. Claude Code
///   makes one through a `WorktreeCreate` hook too: let go when the settings
///   Luna reads name one; refused when they do not and nothing Luna cannot
///   read could (`unread_hook_source`); let go with a warning when something
///   could.
pub fn folder_check(provider: Provider, folder: &str, account_path: &str, worktree: bool) -> FolderCheck {
    let refused = |why: String| FolderCheck { refusal: Some(why), warning: None };
    let dir = Path::new(folder);
    if !dir.is_dir() {
        return refused(if dir.exists() {
            format!("{folder} is a file, not a folder")
        } else {
            format!("the folder {folder} does not exist")
        });
    }
    if !worktree || in_git_repository(dir) {
        return FolderCheck::default();
    }
    let outside = format!("{folder} is not in a git repository");
    let elsewhere = "start without one, or in a folder of a repository";
    if provider == Provider::Codex {
        return refused(format!("{outside}, and Codex makes a worktree with git alone: {elsewhere}"));
    }
    let settings = claude_settings(account_path, folder);
    if settings.iter().any(|v| v["hooks"]["WorktreeCreate"].as_array().is_some_and(|hooks| !hooks.is_empty())) {
        return FolderCheck::default();
    }
    match unread_hook_source(account_path, &settings) {
        Some(source) => FolderCheck {
            refusal: None,
            warning: Some(format!(
                "{outside}: Claude Code makes a worktree there only through a WorktreeCreate hook, which {source} may \
                 give it and Luna cannot read; without one it exits as it starts"
            )),
        },
        None => refused(format!(
            "{outside}, and no settings give Claude Code a WorktreeCreate hook to make a worktree without git: {elsewhere}"
        )),
    }
}

/// `folder_check`'s refusal alone: what a start or a spawn goes by.
pub fn folder_refusal(provider: Provider, folder: &str, account_path: &str, worktree: bool) -> Option<String> {
    folder_check(provider, folder, account_path, worktree).refusal
}

/// Whether `dir` is in a git checkout: it or a folder above it has a `.git`,
/// the repository's folder or a linked worktree's file, as `worktree::head_of`
/// finds a checkout. Looked at, not read: no git is run, no file opened.
pub(crate) fn in_git_repository(dir: &Path) -> bool {
    dir.ancestors().any(|d| d.join(".git").exists())
}

/// The settings files Claude Code reads for a session in `folder` on
/// `account_path` that Luna reads too: the account's `settings.json`, the
/// project's `.claude/settings.json` and `.claude/settings.local.json`, each
/// that is there and readable as `read_settings` takes it.
fn claude_settings(account_path: &str, folder: &str) -> Vec<serde_json::Value> {
    let project = Path::new(folder).join(".claude");
    [Path::new(account_path).join("settings.json"), project.join("settings.json"), project.join("settings.local.json")]
        .iter()
        .filter_map(|file| read_settings(file))
        .collect()
}

/// The most of a settings file `read_settings` reads: far above any real one.
const SETTINGS_MAX: u64 = 1 << 20;

/// A settings file as JSON, read only as one should be: a regular file of at
/// most `SETTINGS_MAX` bytes. A project's own files are the project's to make
/// anything of, and this runs for the dialog and for an agent's spawn: a FIFO
/// at the name, a link to a device, a file of gigabytes give None, not a
/// thread waiting on a writer or memory filled. A link to a regular file is
/// followed, as Claude Code follows it. On Unix the open does not wait on a
/// FIFO swapped in since the look; what was opened is looked at again.
fn read_settings(path: &Path) -> Option<serde_json::Value> {
    use std::io::Read;
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > SETTINGS_MAX {
        return None;
    }
    let mut open = std::fs::OpenOptions::new();
    open.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open.custom_flags(libc::O_NONBLOCK);
    }
    let file = open.open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(SETTINGS_MAX + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > SETTINGS_MAX {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

/// Where a `WorktreeCreate` hook could come from that Luna does not read, if
/// anything says there is such a source (Claude Code 2.1.319's own list):
/// a plugin enabled in the settings Luna reads; the account's server-managed
/// settings, cached as `remote-settings.json` in it; the machine's managed
/// settings folder (`managed-settings.json`, `managed-settings.d`); on
/// Windows the `SOFTWARE\Policies\ClaudeCode` key, the machine's or the
/// user's; on macOS a managed-preferences file of Anthropic's. None when none
/// of them is there.
fn unread_hook_source(account_path: &str, settings: &[serde_json::Value]) -> Option<&'static str> {
    let plugin = |v: &serde_json::Value| v["enabledPlugins"].as_object().is_some_and(|on| on.values().any(|v| v.as_bool() != Some(false)));
    if settings.iter().any(plugin) {
        return Some("an enabled plugin");
    }
    if Path::new(account_path).join("remote-settings.json").exists() {
        return Some("the account's server-managed settings");
    }
    let managed = Path::new(MANAGED_DIR);
    if managed.join("managed-settings.json").exists() || managed.join("managed-settings.d").exists() {
        return Some("the machine's managed settings");
    }
    #[cfg(windows)]
    if policy_key() {
        return Some("the ClaudeCode policy in the registry");
    }
    #[cfg(target_os = "macos")]
    if std::fs::read_dir("/Library/Managed Preferences")
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| e.file_name().to_string_lossy().to_lowercase().starts_with("com.anthropic"))
    {
        return Some("macOS managed preferences");
    }
    None
}

/// Claude Code's folder of managed settings on this OS (2.1.319).
const MANAGED_DIR: &str = if cfg!(windows) {
    r"C:\Program Files\ClaudeCode"
} else if cfg!(target_os = "macos") {
    "/Library/Application Support/ClaudeCode"
} else {
    "/etc/claude-code"
};

/// Whether `SOFTWARE\Policies\ClaudeCode` is there, the machine's or the
/// user's: Claude Code's policy settings on Windows.
#[cfg(windows)]
fn policy_key() -> bool {
    use windows::core::HSTRING;
    use windows::Win32::System::Registry::{RegCloseKey, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ};
    [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER].into_iter().any(|root| {
        let mut key = HKEY::default();
        // SAFETY: a key opened and, when it was, closed here.
        unsafe {
            let there = RegOpenKeyExW(root, &HSTRING::from(r"SOFTWARE\Policies\ClaudeCode"), None, KEY_READ, &mut key).is_ok();
            if there {
                let _ = RegCloseKey(key);
            }
            there
        }
    })
}

/// Whether this machine has a source of Claude Code's settings Luna does not
/// read, which turns a refusal into a warning for every folder: the tests of a
/// refusal are left out where it does.
#[cfg(test)]
pub(crate) fn machine_has_unread_source() -> bool {
    unread_hook_source("/no/such/account", &[]).is_some()
}

/// `folder_check` for the new-chat dialog, said before Create rather than by
/// a chat whose CLI exits as it starts: its refusal disables Create, its
/// warning is only shown. The page's paths are held to their shape and the
/// account to one Luna lists (boundary.rs) first; the look itself runs off
/// the window's thread, as a folder on a slow share may take its time.
#[tauri::command]
pub async fn chat_folder_check(provider: Provider, account_path: AccountPath, folder: AbsPath, worktree: bool) -> FolderCheck {
    let (account_path, folder) = (String::from(account_path), String::from(folder));
    tauri::async_runtime::spawn_blocking(move || folder_check(provider, &folder, &account_path, worktree))
        .await
        .unwrap_or_else(|e| FolderCheck { refusal: Some(format!("the folder could not be checked: {e}")), warning: None })
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
            assert_eq!(folder_check(provider, &dir.to_string_lossy(), "", false), FolderCheck::default());
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
        let outside = !in_git_repository(&plain) && !machine_has_unread_source();
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
            assert_eq!(folder_check(Provider::Claude, &plain, &acc, true), FolderCheck::default());
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

    /// Where a source Luna does not read could give Claude Code the hook, the
    /// start goes ahead and the dialog says why it may fail: a plugin enabled
    /// in the settings, the account's server-managed settings.
    #[test]
    fn a_hook_source_luna_cannot_read_turns_the_refusal_into_a_warning() {
        let dir = scratch("unread");
        let (plain, account) = (dir.join("plain"), dir.join("account"));
        std::fs::create_dir_all(plain.join(".claude")).unwrap();
        std::fs::create_dir_all(&account).unwrap();
        if in_git_repository(&plain) || machine_has_unread_source() {
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        let (acc, folder) = (account.to_string_lossy().into_owned(), plain.to_string_lossy().into_owned());
        assert!(folder_check(Provider::Claude, &folder, &acc, true).refusal.is_some(), "nothing could make one");
        let plugin = r#"{"enabledPlugins":{"jj@tools":true}}"#;
        std::fs::write(plain.join(".claude").join("settings.json"), plugin).unwrap();
        let check = folder_check(Provider::Claude, &folder, &acc, true);
        assert_eq!(check.refusal, None);
        assert!(check.warning.as_deref().is_some_and(|w| w.contains("an enabled plugin") && w.contains("WorktreeCreate")), "{check:?}");
        std::fs::write(plain.join(".claude").join("settings.json"), r#"{"enabledPlugins":{"jj@tools":false}}"#).unwrap();
        assert!(folder_check(Provider::Claude, &folder, &acc, true).refusal.is_some(), "a plugin turned off gives nothing");
        std::fs::write(account.join("remote-settings.json"), "{}").unwrap();
        let check = folder_check(Provider::Claude, &folder, &acc, true);
        assert!(check.refusal.is_none() && check.warning.is_some_and(|w| w.contains("server-managed")));
        assert!(folder_refusal(Provider::Codex, &folder, &acc, true).is_some(), "Codex has no hook to make one");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_settings_file_too_big_to_be_one_is_not_read() {
        let dir = scratch("big");
        let file = dir.join("settings.json");
        let hook = r#"{"hooks":{"WorktreeCreate":[{"hooks":[]}]}}"#;
        let mut big = hook.as_bytes().to_vec();
        big.resize(SETTINGS_MAX as usize + 1, b' ');
        std::fs::write(&file, &big).unwrap();
        assert_eq!(read_settings(&file), None);
        std::fs::write(&file, hook).unwrap();
        assert!(read_settings(&file).is_some(), "the same at its size");
        assert_eq!(read_settings(&dir), None, "a folder at the name");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A FIFO with no writer at a project's settings name, and a link to a
    /// device: given up on at once, not waited on or read without end. A
    /// link to a regular file is followed, as Claude Code follows it.
    #[cfg(unix)]
    #[test]
    fn a_settings_name_that_is_no_file_is_not_waited_on_or_read() {
        let dir = scratch("special");
        let fifo = dir.join("fifo.json");
        let c = std::ffi::CString::new(fifo.to_string_lossy().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let zero = dir.join("zero.json");
        std::os::unix::fs::symlink("/dev/zero", &zero).unwrap();
        let real = dir.join("real.json");
        std::fs::write(&real, r#"{"hooks":{"WorktreeCreate":[{"hooks":[]}]}}"#).unwrap();
        let linked = dir.join("linked.json");
        std::os::unix::fs::symlink(&real, &linked).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let (f, z, l) = (fifo.clone(), zero.clone(), linked.clone());
        std::thread::spawn(move || {
            let _ = tx.send((read_settings(&f), read_settings(&z), read_settings(&l).is_some()));
        });
        let (from_fifo, from_zero, from_link) = rx.recv_timeout(std::time::Duration::from_secs(5)).expect("returned at once");
        assert_eq!((from_fifo, from_zero, from_link), (None, None, true));
        // Swapped in after the look: the open does not wait either.
        let mut open = std::fs::OpenOptions::new();
        open.read(true);
        use std::os::unix::fs::OpenOptionsExt;
        open.custom_flags(libc::O_NONBLOCK);
        assert!(open.open(&fifo).is_ok_and(|f| f.metadata().is_ok_and(|m| !m.is_file())));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
