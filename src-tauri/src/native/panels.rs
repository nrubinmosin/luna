//! The commands of the native window's library, settings and accounts panels
//! (luna_window's `panels::cmd`): each a name to a function over the same
//! functions the Tauri commands are, its arguments read as the page sends
//! them. Paths are held to boundary.rs's checks as they are read (`AbsPath`,
//! `AccountPath`), tighter where the Tauri command takes a plain string: a
//! command cannot be reached with a path that fails them. A few are the
//! native window's alone: its own preferences, a skill's rename, and helpers
//! above their caller, whose confirmation is the window's own.

use super::{Command, Writer};
use crate::boundary::{AbsPath, AccountPath, OrEmpty};
use crate::provider::Provider;
use crate::settings::{Audience, HelperMode};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// A command's arguments, as the page sends them.
fn args<T: DeserializeOwned>(args: Value) -> Result<T, String> {
    serde_json::from_value(args).map_err(|e| format!("bad arguments: {e}"))
}

fn answer<T: Serialize>(value: T) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|e| e.to_string())
}

/// A command over a function of the named arguments, each read as its type
/// and handed over as the function takes it (`Into`). `ok` for a function
/// that cannot fail, `res` for one that answers a `Result`.
macro_rules! own {
    (ok $f:path $(, $arg:ident : $ty:ty)*) => {
        |a| {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            #[allow(dead_code)]
            struct A { $($arg: $ty),* }
            let A { $($arg),* } = args(a)?;
            answer($f($($arg.into()),*))
        }
    };
    (res $f:path $(, $arg:ident : $ty:ty)*) => {
        |a| {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            #[allow(dead_code)]
            struct A { $($arg: $ty),* }
            let A { $($arg),* } = args(a)?;
            answer($f($($arg.into()),*)?)
        }
    };
}

use crate::library as lib;

/// Every command of the panels' the window may call, by name.
pub(super) const OWN: &[(&str, Command)] = &[
    // The library.
    ("library_overview", own!(res lib::library_overview)),
    ("library_sync", own!(res lib::library_sync)),
    ("library_set_source", |a| {
        #[derive(Deserialize)]
        struct A {
            path: OrEmpty<AbsPath>,
        }
        let A { path } = args(a)?;
        answer(lib::library_set_source(path.into_string())?)
    }),
    ("library_set_instructions", own!(res lib::library_set_instructions, name: Option<String>, accounts: Vec<String>)),
    ("library_set_accounts", own!(res lib::library_set_accounts, skill: String, accounts: Vec<String>)),
    ("library_set_audience", own!(res lib::library_set_audience, entry: lib::Entry, audience: Audience)),
    ("library_set_folder", own!(res lib::library_set_folder, path: AbsPath, listed: bool)),
    ("library_set_folder_skill", own!(res lib::library_set_folder_skill, path: AbsPath, skill: String, accounts: Option<Vec<String>>)),
    (
        "library_set_folder_instructions",
        own!(res lib::library_set_folder_instructions, path: AbsPath, account: String, inherit: bool, name: Option<String>),
    ),
    ("library_read", own!(res lib::library_read, entry: lib::Entry)),
    ("library_write", own!(res lib::library_write, entry: lib::Entry, text: String)),
    (
        "library_write_checked",
        own!(
            res lib::library_write_checked,
            entry: lib::Entry,
            text: String,
            root: String,
            root_id: String,
            revision: Option<String>,
            force: bool
        ),
    ),
    ("library_put_back", own!(res lib::library_put_back, dests: Vec<String>, replace: Vec<String>)),
    ("library_kept", own!(res lib::library_kept, dests: Vec<String>, take_out: bool)),
    ("library_draft", |_| answer(kept_draft())),
    ("library_check_skill", own!(ok lib::library_check_skill, skill: String, text: String)),
    ("library_check_system", own!(ok lib::library_check_system, name: String, text: String)),
    ("library_system_compare", own!(res lib::library_system_compare, name: String)),
    ("library_system_reset", own!(res lib::library_system_reset, name: String)),
    ("library_system_adopt", own!(res lib::library_system_adopt, skill: String)),
    ("library_new_skill", own!(res lib::library_new_skill, name: String)),
    ("library_rename_skill", own!(res lib::library_rename_skill, from: String, to: String)),
    ("library_delete_skill", own!(res lib::library_delete_skill, name: String)),
    ("library_new_instructions", own!(res lib::library_new_instructions, name: String)),
    ("library_rename_instructions", own!(res lib::library_rename_instructions, from: String, to: String)),
    ("library_delete_instructions", own!(res lib::library_delete_instructions, name: String)),
    ("library_open", own!(res lib::library_open, skill: Option<String>)),
    // Settings.
    ("window_prefs", |_| answer(read_prefs())),
    ("app_version", |_| answer(env!("CARGO_PKG_VERSION"))),
    ("get_accounts_root", own!(res crate::settings::get_accounts_root)),
    ("set_accounts_root", |a| {
        #[derive(Deserialize)]
        struct A {
            path: OrEmpty<AbsPath>,
        }
        let A { path } = args(a)?;
        answer(crate::settings::set_accounts_root(path.into_string())?)
    }),
    ("helpers_above_caller", own!(ok crate::agents::privilege::helpers_above_caller)),
    ("set_helpers_above_caller", set_helpers_above_caller),
    ("helper_settings", own!(ok crate::agents::spawn::helper_settings)),
    ("set_max_helpers", own!(res crate::agents::spawn::set_max_helpers, n: u32)),
    ("set_helper_mode", own!(res crate::agents::spawn::set_helper_mode, mode: HelperMode)),
    ("set_helper_context_pct", own!(res crate::agents::spawn::set_helper_context_pct, pct: u32)),
    ("codex_trusted_roots", own!(ok crate::codex::trust::codex_trusted_roots)),
    ("set_codex_trusted_roots", |a| {
        #[derive(Deserialize)]
        struct A {
            roots: Vec<AbsPath>,
        }
        let A { roots } = args(a)?;
        answer(crate::codex::trust::set_codex_trusted_roots(roots.into_iter().map(String::from).collect())?)
    }),
    ("cli_status", own!(ok crate::cli::cli_status, provider: Provider)),
    ("cli_versions", own!(ok crate::cli::cli_versions, provider: Provider)),
    ("cli_update_now", own!(ok crate::cli::cli_update_now, provider: Provider)),
    // Accounts.
    ("list_accounts", own!(res crate::accounts::list_accounts)),
    ("create_account", own!(res crate::accounts::create_account, provider: Provider, name: String)),
    ("delete_account", own!(res crate::accounts::delete_account, provider: Provider, name: String)),
    ("account_identity", own!(res crate::accounts::account_identity, provider: Provider, name: String)),
    (
        "delete_account_checked",
        own!(res crate::accounts::delete_account_checked, provider: Provider, name: String, path: AbsPath, id: String),
    ),
    ("account_limits", |a| limits(a, crate::claude::limits::fetch_limits)),
    ("codex_limits", |a| limits(a, crate::codex::limits::fetch_limits)),
    ("settings_sync_state", own!(ok crate::account_settings::settings_sync_state)),
    ("set_settings_parent", own!(res crate::account_settings::set_settings_parent, provider: Provider, parent: Option<String>)),
    ("set_settings_sync_on_start", own!(res crate::account_settings::set_settings_sync_on_start, on: bool)),
];

/// Every command of the panels' the window may send, by name.
pub(super) const OWN_SENT: &[(&str, Command)] = &[("set_window_prefs", set_window_prefs), ("set_library_draft", set_library_draft)];

/// The native window's own, which the page has no Tauri command for: its
/// preferences (the page keeps them in its local storage), Luna's version
/// (the page has it from the updater plugin), a skill's rename, a save bound
/// to what it was read from, the editor's draft, and a deletion bound to the
/// account folder its question named.
#[cfg(test)]
pub(super) const NATIVE_ONLY: &[&str] = &[
    "window_prefs",
    "set_window_prefs",
    "app_version",
    "library_rename_skill",
    "library_write_checked",
    "library_draft",
    "set_library_draft",
    "account_identity",
    "delete_account_checked",
];

/// `{ accountPath, force }`: the account's limits, past the CLI's cache and a
/// 429 cool-off when `force` (the refresh button).
fn limits<T: Serialize>(a: Value, fetch: fn(&str, bool) -> Result<T, String>) -> Result<Value, String> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        account_path: AccountPath,
        #[serde(default)]
        force: bool,
    }
    let A { account_path, force } = args(a)?;
    answer(fetch(&String::from(account_path), force)?)
}

/// `{ on, confirmed }`: helpers above their caller. Turning it on lifts the
/// privilege cap for every tooled session (threat model §3), so it takes a
/// person's yes: in this window, its own confirmation, which nothing but a
/// click in it answers (it hosts no page); `confirmed` says it was given.
/// The answer is the setting as it stands after.
fn set_helpers_above_caller(a: Value) -> Result<Value, String> {
    #[derive(Deserialize)]
    struct A {
        on: bool,
        #[serde(default)]
        confirmed: bool,
    }
    let A { on, confirmed } = args(a)?;
    answer(crate::agents::privilege::set_above(on, || confirmed)?)
}

/// What the native window keeps of its own beside its theme, in
/// `<data>/window.json`: what the page keeps in its local storage.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct WindowPrefs {
    /// Desktop notifications play the system's sound.
    pub sound: bool,
    /// How the new-chat dialog's boxes start in a folder with no start of
    /// its own.
    pub new_chat: NewChatStart,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct NewChatStart {
    /// In a worktree of its own (offered only in a folder in git).
    pub worktree: bool,
    /// With Luna's tools: off unless asked, a session with them pays for
    /// them in context.
    pub tools: bool,
}

impl Default for WindowPrefs {
    fn default() -> Self {
        WindowPrefs { sound: true, new_chat: NewChatStart::default() }
    }
}

impl Default for NewChatStart {
    fn default() -> Self {
        NewChatStart { worktree: true, tools: false }
    }
}

pub(crate) fn read_prefs() -> WindowPrefs {
    super::read_settings().prefs
}

/// `{ prefs }`, sent on the window's thread: handed to the one writer, so a
/// later change is never overwritten by an earlier one.
fn set_window_prefs(a: Value) -> Result<Value, String> {
    #[derive(Deserialize)]
    struct A {
        prefs: WindowPrefs,
    }
    let A { prefs } = args(a)?;
    prefs_writer().put(prefs);
    Ok(Value::Null)
}

fn write_prefs(prefs: &WindowPrefs) -> Result<(), String> {
    super::update_settings(|s| s.prefs = prefs.clone()).map_err(|e| format!("cannot keep the window's preferences: {e}"))
}

pub(super) fn prefs_writer() -> &'static Arc<Writer<WindowPrefs>> {
    static WRITER: OnceLock<Arc<Writer<WindowPrefs>>> = OnceLock::new();
    WRITER.get_or_init(|| Writer::new("prefs", write_prefs))
}

/// How long quitting waits for the last preferences to be written.
pub(super) const PREFS_DRAIN: Duration = Duration::from_secs(2);

/// The library editor's text not saved, as the window last had it: kept in
/// `<data>/library-draft.json` (private) while it differs from the file, so
/// a window that goes with it unsaved (the tray's Quit or Restart window, a
/// crash) offers it at the next open of the library. What the editor read it
/// from goes with it: a restored draft is saved under the same checks.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Draft {
    /// The editor's that keeps it, which a forget names.
    #[serde(default)]
    pub id: String,
    /// As the window names it: `{"skill": name}` and the like.
    pub entry: Value,
    pub root: String,
    #[serde(default)]
    pub root_id: String,
    pub revision: Option<String>,
    pub text: String,
}

/// The data dir's own name for the draft (paths.rs `OWN_NAMES`).
pub(crate) const DRAFT_FILE: &str = "library-draft.json";

/// The most of the draft file read: the editor's largest file
/// (library/system.rs `EDIT_MAX_BYTES`, 1 MiB) as JSON escapes it at worst,
/// and what goes with it.
const DRAFT_MAX: u64 = 8 * 1024 * 1024;

fn draft_file() -> std::path::PathBuf {
    crate::paths::data_dir().join(DRAFT_FILE)
}

/// The draft file at `path`, read only as a private plain file of Luna's
/// (`secret::read_private`: opened as itself, a link or a file others may
/// open refused) and no further than `DRAFT_MAX`; refused, it is said in the
/// log and none is offered.
fn read_draft(path: &std::path::Path) -> Option<Draft> {
    match crate::secret::read_private(path, DRAFT_MAX) {
        Ok(Some(bytes)) => serde_json::from_slice(&bytes)
            .map_err(|e| crate::log::warn("window", &format!("the editor's draft {}: {e}", path.display())))
            .ok(),
        Ok(None) => None,
        Err(e) => {
            crate::log::warn("window", &format!("the editor's draft is not offered: {e}"));
            None
        }
    }
}

/// The draft as last kept or forgotten, in the order the window sent them;
/// read from the file the first time.
fn draft_state() -> std::sync::MutexGuard<'static, Option<Option<Draft>>> {
    static STATE: std::sync::Mutex<Option<Option<Draft>>> = std::sync::Mutex::new(None);
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

fn kept_draft() -> Option<Draft> {
    draft_state().get_or_insert_with(|| read_draft(&draft_file())).clone()
}

/// What is kept after the window sends `draft` (kept, in place of any) or a
/// forget of the draft `forget` names: a draft another editor kept since,
/// under another id, stays. A clear that names no draft leaves what is kept:
/// only a named one is ever cleared.
fn next_draft(kept: Option<Draft>, draft: Option<Draft>, forget: Option<&str>) -> Option<Draft> {
    match (draft, forget) {
        (Some(d), _) => Some(d),
        (None, Some(id)) => kept.filter(|k| k.id != id),
        (None, None) => kept,
    }
}

/// `{ draft }` or `{ draft: null, forget: id }`, sent on the window's
/// thread: decided here in the order sent, then the latest written by the
/// one writer, so a draft forgotten after a save is not brought back by an
/// earlier keystroke's, nor one kept since taken by an older forget.
fn set_library_draft(a: Value) -> Result<Value, String> {
    #[derive(Deserialize)]
    struct A {
        draft: Option<Draft>,
        #[serde(default)]
        forget: Option<String>,
    }
    let A { draft, forget } = args(a)?;
    if draft.is_none() && forget.is_none() {
        let why = "a clear names no draft (`forget`); nothing was cleared";
        crate::log::warn("window", &format!("the editor's draft: {why}"));
        return Err(why.into());
    }
    let mut state = draft_state();
    let kept = state.get_or_insert_with(|| read_draft(&draft_file()));
    let next = next_draft(kept.clone(), draft, forget.as_deref());
    if next != *kept {
        *kept = next.clone();
        draft_writer().put(next);
    }
    Ok(Value::Null)
}

fn write_draft(draft: &Option<Draft>) -> Result<(), String> {
    let path = draft_file();
    match draft {
        Some(d) => {
            let json = serde_json::to_vec(d).map_err(|e| e.to_string())?;
            crate::secret::write_private(&path, &json).map_err(|e| format!("cannot keep the editor's draft: {e}"))
        }
        None => match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("cannot forget the editor's draft: {e}")),
            _ => Ok(()),
        },
    }
}

pub(super) fn draft_writer() -> &'static Arc<Writer<Option<Draft>>> {
    static WRITER: OnceLock<Arc<Writer<Option<Draft>>>> = OnceLock::new();
    WRITER.get_or_init(|| Writer::new("draft", write_draft))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A path that fails boundary.rs's checks never reaches the command: not
    /// as a library folder, a library or accounts folder, a trusted folder,
    /// or an account whose limits are asked.
    #[test]
    fn a_bad_path_is_refused_as_it_is_read() {
        let find = |cmd: &str| OWN.iter().find(|(c, _)| *c == cmd).map(|(_, f)| *f).unwrap();
        for (cmd, args) in [
            ("library_set_folder", json!({ "path": "relative/folder", "listed": true })),
            ("library_set_folder_skill", json!({ "path": "C:/a/../b", "skill": "x", "accounts": null })),
            ("library_set_folder_instructions", json!({ "path": "/a/./b", "account": "claude/a", "inherit": true, "name": null })),
            ("library_set_source", json!({ "path": "not/absolute" })),
            ("set_accounts_root", json!({ "path": "not/absolute" })),
            ("set_codex_trusted_roots", json!({ "roots": ["/fine", "nope"] })),
            ("account_limits", json!({ "accountPath": "/not/an/account", "force": false })),
            ("codex_limits", json!({ "accountPath": "relative" })),
        ] {
            let e = find(cmd)(args).unwrap_err();
            assert!(e.starts_with("bad arguments"), "{cmd}: {e}");
        }
        // Empty is the default, for the two that take one.
        assert!(serde_json::from_value::<OrEmpty<AbsPath>>(json!("")).is_ok());
    }

    #[test]
    fn the_preferences_read_as_the_window_writes_them_and_default_as_the_page() {
        let p: WindowPrefs = serde_json::from_value(json!({ "sound": false, "newChat": { "tools": true } })).unwrap();
        assert_eq!(p, WindowPrefs { sound: false, new_chat: NewChatStart { worktree: true, tools: true } });
        assert_eq!(serde_json::from_value::<WindowPrefs>(json!({})).unwrap(), WindowPrefs::default());
        assert!(WindowPrefs::default().sound && !WindowPrefs::default().new_chat.tools);
        let back: luna_window::panels::Prefs = serde_json::from_value(serde_json::to_value(&p).unwrap()).unwrap();
        assert_eq!((back.sound, back.new_chat.worktree, back.new_chat.tools), (false, true, true));
        assert!(set_window_prefs(json!({ "prefs": { "sound": "loud" } })).unwrap_err().starts_with("bad arguments"));
    }

    fn draft(id: &str) -> Draft {
        Draft {
            id: id.into(),
            entry: json!({ "skill": "review" }),
            root: "/lib".into(),
            root_id: "1:2".into(),
            revision: None,
            text: "t".into(),
        }
    }

    /// A forget takes the draft it names alone: one kept since by another
    /// editor stays, so an old offer thrown away loses no newer text.
    #[test]
    fn a_forget_takes_only_the_draft_it_names() {
        assert_eq!(next_draft(Some(draft("new")), None, Some("old")), Some(draft("new")));
        assert_eq!(next_draft(Some(draft("old")), None, Some("old")), None);
        assert_eq!(next_draft(Some(draft("old")), Some(draft("new")), None), Some(draft("new")));
        assert_eq!(next_draft(None, None, Some("old")), None);
    }

    /// A clear that names no draft clears nothing, the one kept stays; the
    /// command refuses it where it is sent.
    #[test]
    fn a_clear_without_an_id_is_refused() {
        assert_eq!(next_draft(Some(draft("kept")), None, None), Some(draft("kept")));
        let e = set_library_draft(json!({ "draft": null })).unwrap_err();
        assert!(e.contains("names no draft"), "{e}");
    }

    /// The draft file is read only as a private plain file of Luna's: a link
    /// at its name, a file others may open, or one too large is not offered.
    #[test]
    fn a_draft_file_not_luna_s_alone_is_not_offered() {
        let dir = std::env::temp_dir().join(format!("luna-draft-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(DRAFT_FILE);
        let json = serde_json::to_vec(&draft("a")).unwrap();
        crate::secret::write_private(&path, &json).unwrap();
        assert_eq!(read_draft(&path), Some(draft("a")));
        // Forged: written as any file is, open to others.
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, &json).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        // Everyone may read it: what a temp folder's inherited ACL alone may
        // not say.
        #[cfg(windows)]
        assert!(std::process::Command::new("icacls")
            .arg(&path)
            .args(["/grant", "*S-1-1-0:(R)"])
            .output()
            .is_ok_and(|o| o.status.success()));
        assert_eq!(read_draft(&path), None);
        std::fs::remove_file(&path).unwrap();
        // A link to a private file elsewhere.
        let real = dir.join("real.json");
        crate::secret::write_private(&real, &json).unwrap();
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&real, &path).is_ok();
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_file(&real, &path).is_ok();
        if linked {
            assert_eq!(read_draft(&path), None);
            std::fs::remove_file(&path).unwrap();
        }
        // Too large.
        let big = serde_json::to_vec(&Draft { text: "x".repeat(DRAFT_MAX as usize), ..draft("a") }).unwrap();
        crate::secret::write_private(&path, &big).unwrap();
        assert_eq!(read_draft(&path), None);
        assert_eq!(read_draft(&dir.join("none.json")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Turning helpers above their caller on takes the window's yes; off
    /// takes nothing.
    #[test]
    fn helpers_above_their_caller_go_on_only_with_a_yes() {
        assert_eq!(set_helpers_above_caller(json!({ "on": true })).unwrap(), json!(false));
        assert_eq!(set_helpers_above_caller(json!({ "on": true, "confirmed": false })).unwrap(), json!(false));
    }
}
