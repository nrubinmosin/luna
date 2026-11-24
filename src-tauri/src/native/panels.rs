//! The commands of the window's library, settings and accounts panels
//! (luna_window's `panels::cmd`): each a name to a function over the
//! functions that do the work, its arguments read as the window sends them.
//! Paths are held to boundary.rs's checks as they are read (`AbsPath`,
//! `AccountPath`), tighter where the function takes a plain string: a
//! command cannot be reached with a path that fails them.

use super::{Command, Writer};
use crate::boundary::{AbsPath, AccountPath, OrEmpty};
use crate::provider::Provider;
use crate::settings::Audience;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// A command's arguments, as the window sends them.
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
    ("library_draft", |_| answer(kept_draft())),
    ("library_check_system", own!(ok lib::library_check_system, name: String, text: String)),
    ("library_system_compare", own!(res lib::library_system_compare, name: String)),
    ("library_system_reset", own!(res lib::library_system_reset, name: String)),
    ("library_new_skill", own!(res lib::library_new_skill, name: String)),
    ("library_rename_skill", own!(res lib::library_rename_skill, from: String, to: String)),
    ("library_delete_skill", own!(res lib::library_delete_skill, name: String)),
    ("library_new_instructions", own!(res lib::library_new_instructions, name: String)),
    ("library_rename_instructions", own!(res lib::library_rename_instructions, from: String, to: String)),
    ("library_delete_instructions", own!(res lib::library_delete_instructions, name: String)),
    ("library_open", own!(res lib::library_open, skill: Option<String>)),
    // Settings.
    ("window_prefs", |_| answer(read_prefs())),
    ("import_window_prefs", import_window_prefs),
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
    ("helper_settings", own!(ok crate::agents::spawn::helper_settings)),
    ("set_max_helpers", own!(res crate::agents::spawn::set_max_helpers, n: u32)),
    ("set_delegate_auto_compact_tokens", own!(res crate::agents::spawn::set_delegate_auto_compact_tokens, tokens: u32)),
    // Which accounts agents may use: the switch on each row of the accounts panel.
    ("agent_blocked_accounts", own!(ok crate::agents::spawn::account::agent_blocked_accounts)),
    ("set_agent_account", own!(res crate::agents::spawn::account::set_agent_account, provider: Provider, name: String, allowed: bool)),
    ("codex_trusted_roots", own!(ok crate::codex::trust::codex_trusted_roots)),
    ("set_codex_trusted_roots", |a| {
        #[derive(Deserialize)]
        struct A {
            roots: Vec<AbsPath>,
        }
        let A { roots } = args(a)?;
        answer(crate::codex::trust::set_codex_trusted_roots(roots.into_iter().map(String::from).collect())?)
    }),
    ("copy_on_select", own!(ok crate::settings::copy_on_select)),
    ("set_copy_on_select", own!(res crate::settings::set_copy_on_select, value: crate::settings::CopyOnSelect)),
    ("cli_status", own!(ok crate::cli::cli_status, provider: Provider)),
    ("cli_versions", own!(ok crate::cli::cli_versions, provider: Provider)),
    ("cli_update_now", own!(ok crate::cli::cli_update_now, provider: Provider)),
    ("cli_roll_back", own!(res crate::cli::cli_roll_back, provider: Provider, from: String)),
    // Accounts.
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
    /// The window asks whether to turn G-SYNC off in the NVIDIA profile
    /// that holds Luna's exe name (luna_window's `Prefs::gsync_ask`).
    pub gsync_ask: bool,
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
        WindowPrefs { sound: true, new_chat: NewChatStart::default(), gsync_ask: true }
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
    let prefs = serde_json::to_value(prefs).map_err(|e| e.to_string())?;
    super::update_settings(|kept| {
        if let Value::Object(prefs) = prefs {
            kept.extend(prefs);
        }
    })
    .map_err(|e| format!("cannot keep the window's preferences: {e}"))
}

/// `{ sound, newChat: { worktree, tools }, theme }`, each optional: what
/// 0.19's window kept of them (luna_window's import.rs `OldPrefs`), each
/// taken only where `window.json` holds none yet (`read_kept`: in a file
/// 0.20.0 wrote, a key at its default is none). A choice made in this
/// window, or one an import took before, is never written over, so the
/// import goes once. Answered `{ prefs, theme }`: the
/// preferences as they then are, and the theme if it was taken.
fn import_window_prefs(a: Value) -> Result<Value, String> {
    let old: OldPrefs = args(a)?;
    let mut theme = None;
    super::update_settings(|kept| theme = take_old(kept, &old)).map_err(|e| format!("cannot keep the window's preferences: {e}"))?;
    Ok(serde_json::json!({ "prefs": read_prefs(), "theme": theme }))
}

/// What `import_window_prefs` takes.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct OldPrefs {
    sound: Option<bool>,
    new_chat: Option<OldStart>,
    theme: Option<luna_window::ThemePref>,
}

#[derive(Deserialize, Default)]
struct OldStart {
    worktree: Option<bool>,
    tools: Option<bool>,
}

/// `old` into `window.json`'s keys (`kept`), each where none is kept; the
/// theme, where it was taken.
fn take_old(kept: &mut serde_json::Map<String, Value>, old: &OldPrefs) -> Option<luna_window::ThemePref> {
    if let Some(sound) = old.sound {
        kept.entry("sound").or_insert(sound.into());
    }
    if let Some(start) = &old.new_chat {
        // `read_kept` lets a kept `newChat` through only as an object.
        if let Value::Object(kept) = kept.entry("newChat").or_insert_with(|| Value::Object(serde_json::Map::new())) {
            for (key, value) in [("worktree", start.worktree), ("tools", start.tools)] {
                if let Some(value) = value {
                    kept.entry(key).or_insert(value.into());
                }
            }
        }
    }
    let theme = old.theme.filter(|_| !kept.contains_key("theme"))?;
    kept.insert("theme".into(), serde_json::json!(theme));
    Some(theme)
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

    /// An account switched off for agents is named by a provider Luna has and
    /// a name an account could have; anything else is refused before the
    /// settings are touched.
    #[test]
    fn an_agent_account_switch_takes_only_an_accounts_name() {
        let set = OWN.iter().find(|(c, _)| *c == "set_agent_account").map(|(_, f)| *f).unwrap();
        for name in ["", "..", "a/b", r"a\b", "a:b"] {
            let e = set(json!({ "provider": "claude", "name": name, "allowed": false })).unwrap_err();
            assert!(!e.starts_with("bad arguments"), "{name:?} reached the setter's own check: {e}");
        }
        assert!(set(json!({ "provider": "gemini", "name": "work", "allowed": false })).unwrap_err().starts_with("bad arguments"));
        assert!(set(json!({ "provider": "claude", "name": "work" })).unwrap_err().starts_with("bad arguments"));
    }

    #[test]
    fn the_preferences_read_as_the_window_writes_them_and_default_as_the_page() {
        let p: WindowPrefs = serde_json::from_value(json!({ "sound": false, "newChat": { "tools": true } })).unwrap();
        assert_eq!(p, WindowPrefs { sound: false, new_chat: NewChatStart { worktree: true, tools: true }, gsync_ask: true });
        assert_eq!(serde_json::from_value::<WindowPrefs>(json!({})).unwrap(), WindowPrefs::default());
        assert!(WindowPrefs::default().sound && !WindowPrefs::default().new_chat.tools);
        let back: luna_window::panels::Prefs = serde_json::from_value(serde_json::to_value(&p).unwrap()).unwrap();
        assert_eq!((back.sound, back.new_chat.worktree, back.new_chat.tools), (false, true, true));
        assert!(set_window_prefs(json!({ "prefs": { "sound": "loud" } })).unwrap_err().starts_with("bad arguments"));
    }

    /// 0.19's choices are taken where `window.json` holds none, never over
    /// one made here or taken before; a key there that does not read is
    /// none, costing only itself, a box only itself.
    #[test]
    fn the_import_takes_0_19s_choices_only_where_none_is_kept() {
        let old = |a: Value| serde_json::from_value::<OldPrefs>(a).unwrap();
        let mut kept = super::super::kept_of(b"{}");
        take_old(&mut kept, &old(json!({ "sound": false, "newChat": { "worktree": false, "tools": true } })));
        assert_eq!(Value::Object(kept.clone()), json!({ "sound": false, "newChat": { "worktree": false, "tools": true } }));
        take_old(&mut kept, &old(json!({ "sound": true, "newChat": { "worktree": true, "tools": false } })));
        let p: WindowPrefs = serde_json::from_value(Value::Object(kept)).unwrap();
        assert_eq!(p, WindowPrefs { sound: false, new_chat: NewChatStart { worktree: false, tools: true }, gsync_ask: true }, "taken once");
        // Chosen here, at the defaults too: Settings writes the sound and the
        // boxes together, and this build says so (`version`).
        let mine = br#"{ "version": 2, "theme": "light", "sound": true, "newChat": { "worktree": true, "tools": false } }"#;
        let mut chosen = super::super::kept_of(mine);
        take_old(&mut chosen, &old(json!({ "sound": false, "newChat": { "worktree": false, "tools": true }, "theme": "dark" })));
        assert_eq!(
            Value::Object(chosen),
            json!({ "version": 2, "theme": "light", "sound": true, "newChat": { "worktree": true, "tools": false } })
        );
        // Only the theme chosen: the rest is still 0.19's to give.
        let mut theme = super::super::kept_of(br#"{ "version": 2, "theme": "light" }"#);
        assert_eq!(take_old(&mut theme, &old(json!({ "newChat": { "tools": true }, "theme": "dark" }))), None);
        assert_eq!(Value::Object(theme), json!({ "version": 2, "theme": "light", "newChat": { "tools": true } }));
        // None chosen: 0.19's is taken and said, once.
        let mut none = super::super::kept_of(br#"{ "version": 2, "sound": true }"#);
        assert_eq!(take_old(&mut none, &old(json!({ "theme": "dark" }))), Some(luna_window::ThemePref::Dark));
        assert_eq!(take_old(&mut none, &old(json!({ "theme": "light" }))), None);
        assert_eq!(Value::Object(none), json!({ "version": 2, "sound": true, "theme": "dark" }));
        // One box that does not read leaves the other, which the import does
        // not write over.
        let mut bad =
            super::super::kept_of(br#"{ "version": 2, "theme": "sepia", "sound": "loud", "newChat": { "worktree": false, "tools": 1 } }"#);
        assert_eq!(Value::Object(bad.clone()), json!({ "version": 2, "newChat": { "worktree": false } }));
        take_old(&mut bad, &old(json!({ "sound": false, "newChat": { "worktree": true, "tools": true } })));
        assert_eq!(Value::Object(bad), json!({ "version": 2, "sound": false, "newChat": { "worktree": false, "tools": true } }));
        assert!(import_window_prefs(json!({ "sound": "loud" })).unwrap_err().starts_with("bad arguments"));
        assert!(super::super::kept_of(b"[1]").is_empty() && super::super::kept_of(b"{not json").is_empty());
    }

    /// The 0.20.0 window wrote every key at each write, those never chosen
    /// at their defaults: in its file (no `version`) only a key off its
    /// default is a choice, and 0.19's takes the place of the others.
    #[test]
    fn a_0_20_0_files_defaults_are_no_choice() {
        let old = |a: Value| serde_json::from_value::<OldPrefs>(a).unwrap();
        // The theme was changed in 0.20.0, nothing else.
        let file = br#"{ "theme": "dark", "sound": true, "newChat": { "worktree": true, "tools": false } }"#;
        let mut kept = super::super::kept_of(file);
        assert_eq!(Value::Object(kept.clone()), json!({ "theme": "dark" }));
        let taken = take_old(&mut kept, &old(json!({ "sound": false, "newChat": { "worktree": false, "tools": true }, "theme": "light" })));
        assert_eq!(taken, None, "the theme chosen in 0.20.0 stays");
        assert_eq!(Value::Object(kept), json!({ "theme": "dark", "sound": false, "newChat": { "worktree": false, "tools": true } }));
        // Off their defaults in 0.20.0: chosen there, kept.
        let file = br#"{ "theme": "system", "sound": false, "newChat": { "worktree": false, "tools": false } }"#;
        let mut kept = super::super::kept_of(file);
        let taken = take_old(&mut kept, &old(json!({ "sound": true, "newChat": { "worktree": true, "tools": true }, "theme": "dark" })));
        assert_eq!(taken, Some(luna_window::ThemePref::Dark));
        assert_eq!(Value::Object(kept), json!({ "sound": false, "newChat": { "worktree": false, "tools": true }, "theme": "dark" }));
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
}
