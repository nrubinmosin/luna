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
    let (settings, unread) = claude_settings(account_path, folder);
    if settings.iter().any(|v| v["hooks"]["WorktreeCreate"].as_array().is_some_and(|hooks| !hooks.is_empty())) {
        return FolderCheck::default();
    }
    match unread_hook_source(account_path, &settings, &unread) {
        Some(source) => FolderCheck {
            refusal: None,
            warning: Some(format!(
                "{outside}: Claude Code makes a worktree there only through a WorktreeCreate hook, and Luna cannot tell \
                 whether {source} gives it one; without one it exits as it starts"
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
/// project's `.claude/settings.json` and `.claude/settings.local.json`. Those
/// read, and apart those that are there but could not be (`read_settings`):
/// what they say is not known, which is not the same as nothing.
fn claude_settings(account_path: &str, folder: &str) -> (Vec<serde_json::Value>, Vec<std::path::PathBuf>) {
    let project = Path::new(folder).join(".claude");
    let (mut read, mut unread) = (Vec::new(), Vec::new());
    for file in [Path::new(account_path).join("settings.json"), project.join("settings.json"), project.join("settings.local.json")] {
        match read_settings(&file) {
            Settings::Absent => {}
            Settings::Read(v) => read.push(v),
            Settings::Unread => unread.push(file),
        }
    }
    (read, unread)
}

/// The most of a settings file `read_settings` reads: far above any real one.
const SETTINGS_MAX: u64 = 1 << 20;

/// What `read_settings` made of a settings file's name.
#[derive(Debug, PartialEq)]
enum Settings {
    /// Nothing there.
    Absent,
    Read(serde_json::Value),
    /// Something there that could not be read as a settings file: too big,
    /// not a regular file, an error, not JSON.
    Unread,
}

/// A settings file as JSON, read only as one should be: a regular file of at
/// most `SETTINGS_MAX` bytes. A project's own files are the project's to make
/// anything of, and this runs for the dialog and for an agent's spawn: a FIFO
/// at the name, a link to a device, a file of gigabytes are `Unread`, not a
/// thread waiting on a writer or memory filled. A link to a regular file is
/// followed, as Claude Code follows it. On Unix the open does not wait on a
/// FIFO swapped in since the look; what was opened is looked at again.
fn read_settings(path: &Path) -> Settings {
    use std::io::Read;
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Settings::Absent,
        Err(_) => return Settings::Unread,
    };
    if !meta.is_file() || meta.len() > SETTINGS_MAX {
        return Settings::Unread;
    }
    let mut open = std::fs::OpenOptions::new();
    open.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open.custom_flags(libc::O_NONBLOCK);
    }
    let read = open.open(path).and_then(|file| {
        if !file.metadata()?.is_file() {
            return Err(std::io::Error::other("not a regular file"));
        }
        let mut bytes = Vec::new();
        file.take(SETTINGS_MAX + 1).read_to_end(&mut bytes)?;
        Ok(bytes)
    });
    match read {
        Ok(bytes) if bytes.len() as u64 <= SETTINGS_MAX => serde_json::from_slice(&bytes).map_or(Settings::Unread, Settings::Read),
        _ => Settings::Unread,
    }
}

/// What a look for one source of settings found: there, not there, or no
/// answer (a refused look, an error), which is no proof of nothing.
#[derive(Debug, PartialEq, Clone, Copy)]
enum Found {
    There,
    Absent,
    Unknown,
}

/// `Found` for a path.
fn found(path: &Path) -> Found {
    match path.try_exists() {
        Ok(true) => Found::There,
        Ok(false) => Found::Absent,
        Err(_) => Found::Unknown,
    }
}

/// Where a `WorktreeCreate` hook could come from that Luna has not read, if
/// anything says there may be one (Claude Code 2.1.319's own list): a
/// settings file of the three that is there but could not be read; a plugin
/// enabled in those read; the account's server-managed settings, cached as
/// `remote-settings.json` in it; the machine's managed settings folder
/// (`managed-settings.json`, `managed-settings.d`); on Windows the
/// `SOFTWARE\Policies\ClaudeCode` key, the machine's or the user's; on macOS a
/// managed-preferences file of Anthropic's. A place that could not be looked
/// at counts as one that may hold a hook. None only when every one was looked
/// at and is not there.
fn unread_hook_source(account_path: &str, settings: &[serde_json::Value], unread: &[std::path::PathBuf]) -> Option<String> {
    if let Some(file) = unread.first() {
        return Some(format!("{}, which it could not read,", file.display()));
    }
    let plugin = |v: &serde_json::Value| v["enabledPlugins"].as_object().is_some_and(|on| on.values().any(|v| v.as_bool() != Some(false)));
    if settings.iter().any(plugin) {
        return Some("an enabled plugin".into());
    }
    let said = |what: &str, found: Found| match found {
        Found::There => Some(what.to_string()),
        Found::Unknown => Some(format!("{what}, which it could not look at,")),
        Found::Absent => None,
    };
    let remote = found(&Path::new(account_path).join("remote-settings.json"));
    if let Some(source) = said("the account's server-managed settings", remote) {
        return Some(source);
    }
    let managed = Path::new(MANAGED_DIR);
    for place in [managed.join("managed-settings.json"), managed.join("managed-settings.d")] {
        if let Some(source) = said("the machine's managed settings", found(&place)) {
            return Some(source);
        }
    }
    #[cfg(windows)]
    if let Some(source) = said("the ClaudeCode policy in the registry", policy_key()) {
        return Some(source);
    }
    #[cfg(target_os = "macos")]
    {
        let preferences = match std::fs::read_dir("/Library/Managed Preferences") {
            Ok(entries) => {
                let anthropic = |e: &std::fs::DirEntry| e.file_name().to_string_lossy().to_lowercase().starts_with("com.anthropic");
                if entries.flatten().any(|e| anthropic(&e)) {
                    Found::There
                } else {
                    Found::Absent
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Found::Absent,
            Err(_) => Found::Unknown,
        };
        if let Some(source) = said("macOS managed preferences", preferences) {
            return Some(source);
        }
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
/// user's: Claude Code's policy settings on Windows. There in either is
/// `There`; a look refused or failed in either, and not there in the other,
/// `Unknown`.
#[cfg(windows)]
fn policy_key() -> Found {
    use windows::core::HSTRING;
    use windows::Win32::System::Registry::{RegCloseKey, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ};
    let looks = [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER].map(|root| {
        let mut key = HKEY::default();
        // SAFETY: a key opened and, when it was, closed here.
        unsafe {
            let code = RegOpenKeyExW(root, &HSTRING::from(r"SOFTWARE\Policies\ClaudeCode"), None, KEY_READ, &mut key);
            if code.is_ok() {
                let _ = RegCloseKey(key);
            }
            key_found(code)
        }
    });
    [Found::There, Found::Unknown].into_iter().find(|f| looks.contains(f)).unwrap_or(Found::Absent)
}

/// What a registry key's open said: opened, not there (the key or a key on
/// its path), or anything else, an access denied among it, which says nothing
/// of whether it is there.
#[cfg(windows)]
fn key_found(code: windows::Win32::Foundation::WIN32_ERROR) -> Found {
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS};
    match code {
        ERROR_SUCCESS => Found::There,
        ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => Found::Absent,
        _ => Found::Unknown,
    }
}

/// Whether this machine has a source of Claude Code's settings Luna does not
/// read, which turns a refusal into a warning for every folder: the tests of a
/// refusal are left out where it does.
#[cfg(test)]
pub(crate) fn machine_has_unread_source() -> bool {
    unread_hook_source("/no/such/account", &[], &[]).is_some()
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
        assert_eq!(read_settings(&file), Settings::Unread);
        std::fs::write(&file, hook).unwrap();
        assert!(matches!(read_settings(&file), Settings::Read(_)), "the same at its size");
        assert_eq!(read_settings(&dir), Settings::Unread, "a folder at the name");
        std::fs::write(&file, "{ not json").unwrap();
        assert_eq!(read_settings(&file), Settings::Unread);
        assert_eq!(read_settings(&dir.join("none.json")), Settings::Absent);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A settings file Luna could not read is no proof that it names no hook:
    /// one that names WorktreeCreate, padded past what Luna reads, or a folder
    /// at a settings name, leaves the start to Claude Code with a warning.
    #[test]
    fn a_settings_file_luna_could_not_read_warns_rather_than_refuses() {
        let dir = scratch("unreadable");
        let (plain, account) = (dir.join("plain"), dir.join("account"));
        std::fs::create_dir_all(plain.join(".claude")).unwrap();
        std::fs::create_dir_all(&account).unwrap();
        if in_git_repository(&plain) || machine_has_unread_source() {
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        let (acc, folder) = (account.to_string_lossy().into_owned(), plain.to_string_lossy().into_owned());
        assert!(folder_check(Provider::Claude, &folder, &acc, true).refusal.is_some(), "nothing could make one");
        let mut padded = br#"{"hooks":{"WorktreeCreate":[{"hooks":[{"type":"command","command":"jj workspace add"}]}]}}"#.to_vec();
        padded.resize(SETTINGS_MAX as usize + 1, b' ');
        let local = plain.join(".claude").join("settings.local.json");
        std::fs::write(&local, &padded).unwrap();
        let check = folder_check(Provider::Claude, &folder, &acc, true);
        assert_eq!(check.refusal, None, "{check:?}");
        assert!(check.warning.as_deref().is_some_and(|w| w.contains("settings.local.json") && w.contains("could not read")), "{check:?}");
        std::fs::remove_file(&local).unwrap();
        std::fs::create_dir_all(account.join("settings.json")).unwrap();
        let check = folder_check(Provider::Claude, &folder, &acc, true);
        assert!(check.refusal.is_none() && check.warning.is_some_and(|w| w.contains("could not read")), "a folder at the name");
        assert!(folder_refusal(Provider::Codex, &folder, &acc, true).is_some(), "Codex has no hook to make one");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A registry look that fails other than by "not there" says nothing of
    /// the policy key: no proof it is absent.
    #[cfg(windows)]
    #[test]
    fn a_registry_look_that_fails_is_no_proof_of_no_policy() {
        use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS};
        assert_eq!(key_found(ERROR_SUCCESS), Found::There);
        assert_eq!(key_found(ERROR_FILE_NOT_FOUND), Found::Absent);
        assert_eq!(key_found(ERROR_PATH_NOT_FOUND), Found::Absent);
        assert_eq!(key_found(ERROR_ACCESS_DENIED), Found::Unknown);
        let unknown = unread_hook_source("/no/such/account", &[], &[]);
        assert!(policy_key() == Found::Absent || unknown.is_some(), "a key there or unknown is a source");
    }

    /// A place of settings that could not be looked at is a source as much as
    /// one that is there.
    #[test]
    fn a_place_that_could_not_be_looked_at_counts_as_a_source() {
        let dir = scratch("found");
        assert_eq!(found(&dir), Found::There);
        assert_eq!(found(&dir.join("none")), Found::Absent);
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
            let _ = tx.send((read_settings(&f), read_settings(&z), matches!(read_settings(&l), Settings::Read(_))));
        });
        let (from_fifo, from_zero, from_link) = rx.recv_timeout(std::time::Duration::from_secs(5)).expect("returned at once");
        assert_eq!((from_fifo, from_zero, from_link), (Settings::Unread, Settings::Unread, true));
        // Unread is no proof of no hook: such a name in a project leaves the
        // start to Claude Code with a warning.
        let (plain, account) = (dir.join("plain"), dir.join("account"));
        std::fs::create_dir_all(plain.join(".claude")).unwrap();
        std::fs::create_dir_all(&account).unwrap();
        if !in_git_repository(&plain) && !machine_has_unread_source() {
            std::os::unix::fs::symlink("/dev/zero", plain.join(".claude").join("settings.json")).unwrap();
            let check = folder_check(Provider::Claude, &plain.to_string_lossy(), &account.to_string_lossy(), true);
            assert!(check.refusal.is_none() && check.warning.is_some(), "{check:?}");
        }
        // Swapped in after the look: the open does not wait either.
        let mut open = std::fs::OpenOptions::new();
        open.read(true);
        use std::os::unix::fs::OpenOptionsExt;
        open.custom_flags(libc::O_NONBLOCK);
        assert!(open.open(&fifo).is_ok_and(|f| f.metadata().is_ok_and(|m| !m.is_file())));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
