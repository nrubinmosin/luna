//! The commands the core answers for the window: everything that touches the
//! running sessions. The page calls them as it always has; the window passes
//! them on (corelink.rs `core_call` / `core_send`) and they land here, by name
//! and with the page's own camelCase arguments, on the same functions the
//! Tauri commands are.
//!
//! Only the commands in `COMMANDS` come through, each on its own lane, and
//! their path arguments are typed (boundary.rs): a path of the wrong shape, or
//! an account folder Luna does not list, is refused as the arguments are read,
//! before the function runs.

use crate::boundary::{AbsPath, AccountPath};
use crate::provider::Provider;
use serde_json::Value;
use std::collections::HashMap;

/// How a command of the page's reaches the core.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lane {
    /// `core_call`: answered, and waited on.
    Call,
    /// `core_send`: keystrokes and resizes, written in order, never answered.
    Send,
}

/// Every command the window passes on from the page, and its lane. The window
/// refuses any other (corelink.rs), and so does `dispatch`: the protocol's own
/// `hello`, `info` and `shutdown` are not here, and only the window itself
/// sends them. src/ipc/core.ts keeps the same two lists; a test holds them
/// together.
pub const COMMANDS: &[(&str, Lane)] = &[
    ("ensure_claude_session", Lane::Call),
    ("ensure_codex_session", Lane::Call),
    ("write_session", Lane::Send),
    ("resize_session", Lane::Send),
    ("kill_session", Lane::Call),
    ("delete_session", Lane::Call),
    ("stop_all_sessions", Lane::Call),
    ("orphan_sessions", Lane::Call),
    ("session_meta", Lane::Call),
    ("session_modes", Lane::Call),
    ("dismiss_waiting", Lane::Call),
    ("background_helpers", Lane::Call),
    ("background_promote", Lane::Call),
    ("background_promoted", Lane::Call),
    ("background_kill", Lane::Call),
    ("background_delete", Lane::Call),
    ("background_at_stake", Lane::Call),
    ("background_transcript", Lane::Call),
    ("chat_names", Lane::Call),
    ("account_usage", Lane::Call),
    ("power_state", Lane::Call),
    ("set_keep_awake", Lane::Call),
    ("arm_power_off", Lane::Call),
    ("disarm_power_off", Lane::Call),
    ("rename_account", Lane::Call),
    ("orphan_worktrees", Lane::Call),
    ("remove_orphan_worktrees", Lane::Call),
    ("sync_settings_now", Lane::Call),
    ("apply_recommended", Lane::Call),
    ("codex_folder_trusted", Lane::Call),
    ("codex_trust_folder", Lane::Call),
    ("claude_trust_folder", Lane::Call),
    ("recommendations", Lane::Call),
    ("apply_recommendation", Lane::Call),
    ("dismiss_recommendation", Lane::Call),
    ("applied_by_luna", Lane::Call),
];

/// The lane `cmd` comes on, or None for a command the page may not send.
pub fn lane(cmd: &str) -> Option<Lane> {
    COMMANDS.iter().find(|(c, _)| *c == cmd).map(|&(_, l)| l)
}

/// A checked argument into the plain one the function takes: a typed path
/// into its `String`, anything else as it is.
trait Arg<T> {
    fn arg(self) -> T;
}
impl<T> Arg<T> for T {
    fn arg(self) -> T {
        self
    }
}
impl Arg<String> for AbsPath {
    fn arg(self) -> String {
        self.into()
    }
}
impl Arg<String> for AccountPath {
    fn arg(self) -> String {
        self.into()
    }
}
impl Arg<Option<String>> for Option<AbsPath> {
    fn arg(self) -> Option<String> {
        self.map(Into::into)
    }
}
impl Arg<Vec<String>> for Vec<AbsPath> {
    fn arg(self) -> Vec<String> {
        self.into_iter().map(Into::into).collect()
    }
}
impl Arg<Vec<String>> for Vec<AccountPath> {
    fn arg(self) -> Vec<String> {
        self.into_iter().map(Into::into).collect()
    }
}

/// Deserializes the page's arguments into the function's parameters, calls
/// it, and turns what it returns into the answer. `ok` for functions that
/// cannot fail; `res` for those returning `Result<_, String>`; `wait` in front
/// of either for an async one.
macro_rules! call {
    (@args $args:expr, $($name:ident : $ty:ty),*) => {{
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Args { $($name: $ty),* }
        let a: Args = serde_json::from_value($args).map_err(|e| format!("bad arguments: {e}"))?;
        a
    }};
    (ok $args:expr, $f:path $(, $name:ident : $ty:ty)*) => {{
        let _a = call!(@args $args, $($name : $ty),*);
        Ok(serde_json::to_value($f($(_a.$name.arg()),*)).map_err(|e| e.to_string())?)
    }};
    (res $args:expr, $f:path $(, $name:ident : $ty:ty)*) => {{
        let _a = call!(@args $args, $($name : $ty),*);
        Ok(serde_json::to_value($f($(_a.$name.arg()),*)?).map_err(|e| e.to_string())?)
    }};
    (wait ok $args:expr, $f:path $(, $name:ident : $ty:ty)*) => {{
        let _a = call!(@args $args, $($name : $ty),*);
        Ok(serde_json::to_value(tauri::async_runtime::block_on($f($(_a.$name.arg()),*))).map_err(|e| e.to_string())?)
    }};
    (wait res $args:expr, $f:path $(, $name:ident : $ty:ty)*) => {{
        let _a = call!(@args $args, $($name : $ty),*);
        Ok(serde_json::to_value(tauri::async_runtime::block_on($f($(_a.$name.arg()),*))?).map_err(|e| e.to_string())?)
    }};
}

/// Answers one command. Blocks for as long as the command takes; the core
/// runs each on a thread of its own, keystrokes and the few that must keep
/// their order excepted (`in_order`).
pub fn dispatch(cmd: &str, args: Value) -> Result<Value, String> {
    use crate::{account_settings, accounts, agents, checks, power, pty, worktree};
    if lane(cmd).is_none() {
        return Err(format!("the core has no command {cmd}"));
    }
    // A command with no arguments may come with none at all.
    let args = if args.is_null() { Value::Object(Default::default()) } else { args };
    match cmd {
        "ensure_claude_session" => call!(wait res args, pty::ensure_claude_session,
            id: String, folder: AbsPath, account_path: AccountPath, model: String, effort: String,
            permission_mode: String, worktree: bool, worktree_name: Option<String>, resume: Option<String>,
            tools: Option<bool>, read_only: Option<bool>, agent: Option<String>, guard: Option<bool>),
        "ensure_codex_session" => call!(wait res args, pty::ensure_codex_session,
            id: String, folder: AbsPath, account_path: AccountPath, model: Option<String>, effort: String,
            approval: String, sandbox: String, worktree: Option<bool>, resume: Option<String>, login: bool,
            tools: Option<bool>, hooks_unasked: Option<bool>, guard: Option<bool>),
        "write_session" => call!(res args, pty::write_session, id: String, data: String),
        "resize_session" => call!(res args, pty::resize_session, id: String, cols: u16, rows: u16),
        "kill_session" => call!(wait res args, pty::kill_session, id: String),
        "delete_session" => call!(wait res args, pty::delete_session,
            id: String, folder: AbsPath, worktree_path: Option<AbsPath>, drop_worktree: bool),
        "stop_all_sessions" => call!(wait res args, pty::stop_all_sessions),
        "orphan_sessions" => call!(wait res args, pty::orphan_sessions, known: Vec<String>),
        "session_meta" => call!(wait res args, pty::session_meta, id: String, account_path: AccountPath),
        "session_modes" => call!(ok args, pty::session_modes, id: String),
        "dismiss_waiting" => call!(ok args, crate::asking::dismiss_waiting, id: String, key: String),
        "background_helpers" => call!(ok args, crate::background::background_helpers),
        "background_promote" => call!(res args, crate::background::background_promote, id: String),
        "background_promoted" => call!(res args, crate::background::background_promoted, id: String, error: Option<String>),
        "background_kill" => call!(ok args, crate::background::background_kill, id: String),
        "background_delete" => call!(res args, crate::background::background_delete,
            id: String, drop_worktree: Option<bool>, stop_working: Option<bool>, drop_unsaved: Option<bool>),
        "background_at_stake" => call!(ok args, crate::background::background_at_stake, ids: Vec<String>),
        "background_transcript" => call!(res args, crate::background::background_transcript,
            id: String, since: Option<u64>, file: Option<String>),
        "chat_names" => call!(ok args, agents::chat_names,
            names: HashMap<String, String>, groups: Option<HashMap<String, u32>>),
        "account_usage" => call!(ok args, agents::account_usage, usage: HashMap<String, Value>),
        "power_state" => call!(ok args, power::power_state),
        "set_keep_awake" => call!(res args, power::set_keep_awake, on: bool),
        "arm_power_off" => call!(res args, power::arm_power_off, action: String, quiet_s: u64, drain: Option<bool>),
        "disarm_power_off" => call!(ok args, power::disarm_power_off),
        "rename_account" => call!(res args, accounts::rename_account, provider: Provider, name: String, to: String),
        "orphan_worktrees" => call!(wait res args, worktree::orphan_worktrees,
            folder: AbsPath, in_use: Vec<String>, account_paths: Vec<AccountPath>),
        "remove_orphan_worktrees" => call!(wait res args, worktree::remove_orphan_worktrees,
            folder: AbsPath, in_use: Vec<String>, account_paths: Vec<AccountPath>, paths: Vec<AbsPath>),
        "sync_settings_now" => call!(res args, account_settings::sync_settings_now, provider: Provider),
        "apply_recommended" => call!(res args, account_settings::apply_recommended,
            provider: Provider, account_path: AccountPath, add: Vec<String>, remove: Vec<String>),
        "codex_folder_trusted" => call!(res args, crate::codex::trust::codex_folder_trusted, account_path: AccountPath, folder: AbsPath),
        "codex_trust_folder" => call!(res args, crate::codex::trust::codex_trust_folder, account_path: AccountPath, folder: AbsPath),
        "claude_trust_folder" => call!(res args, crate::claude::trust::claude_trust_folder, account_path: AccountPath, folder: AbsPath),
        "applied_by_luna" => call!(ok args, crate::applied::applied_by_luna, chat_id: String),
        "recommendations" => call!(res args, checks::recommendations),
        "apply_recommendation" => call!(res args, checks::apply_recommendation, id: String, account: Option<String>, confirmed: bool),
        "dismiss_recommendation" => call!(res args, checks::dismiss_recommendation, id: String, account: Option<String>, dismissed: bool),
        other => Err(format!("the core has no command {other}")),
    }
}

/// Commands answered on the connection's own thread, one after another, in
/// the order they came: what the page sends them for depends on that order
/// (a reattach reads the modes after the session it ensured), and each takes
/// no time.
pub fn in_order(cmd: &str) -> bool {
    matches!(cmd, "write_session" | "resize_session" | "session_modes" | "chat_names" | "account_usage" | "power_state")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn arguments_arrive_as_the_page_names_them() {
        // A session that is not there, asked for in the page's own words:
        // the call reaches the function and its answer comes back.
        assert_eq!(dispatch("session_modes", json!({ "id": "nobody" })), Ok(Value::from("")));
        assert!(dispatch("write_session", json!({ "id": "nobody", "data": "x" })).unwrap_err().contains("no such session"));
        assert!(dispatch("session_modes", json!({})).unwrap_err().contains("bad arguments"));
        assert!(dispatch("nope", Value::Null).unwrap_err().contains("no command"));
        assert!(dispatch("power_state", Value::Null).is_ok(), "no arguments at all");
    }

    #[test]
    fn every_listed_command_is_answered_and_nothing_else_is() {
        // Arguments of the wrong shape: each listed command gets as far as
        // reading them, and none runs.
        for (cmd, _) in COMMANDS {
            let err = dispatch(cmd, json!("not an object")).unwrap_err();
            assert!(err.contains("bad arguments"), "{cmd}: {err}");
        }
        // The protocol's own commands are the window's, never the page's.
        for cmd in ["hello", "info", "shutdown", ""] {
            assert!(lane(cmd).is_none(), "{cmd}");
            assert!(dispatch(cmd, Value::Null).unwrap_err().contains("no command"), "{cmd}");
        }
    }

    #[test]
    fn the_page_and_the_core_list_the_same_commands() {
        // src/ipc/core.ts routes by its own two sets; one missing here would
        // be refused, one missing there would never be sent.
        let ts = include_str!("../../src/ipc/core.ts");
        let set = |name: &str| -> Vec<String> {
            let start = ts.find(&format!("const {name}")).unwrap_or_else(|| panic!("{name} not in core.ts"));
            let body = &ts[start..];
            let body = &body[body.find("new Set([").unwrap() + 9..];
            let body = &body[..body.find("])").unwrap()];
            let mut v: Vec<String> = body.split(',').map(|s| s.trim().trim_matches('\'').to_string()).filter(|s| !s.is_empty()).collect();
            v.sort();
            v
        };
        let of = |l: Lane| -> Vec<String> {
            let mut v: Vec<String> = COMMANDS.iter().filter(|(_, x)| *x == l).map(|(c, _)| c.to_string()).collect();
            v.sort();
            v
        };
        assert_eq!(set("CORE_CALL"), of(Lane::Call));
        assert_eq!(set("CORE_SEND"), of(Lane::Send));
    }

    #[test]
    fn bad_paths_are_refused_before_anything_runs() {
        let rel = "projects/p";
        let up = if cfg!(windows) { r"C:\p\..\Windows" } else { "/p/../etc" };
        let elsewhere = if cfg!(windows) { r"C:\Windows" } else { "/etc" };
        let session = |folder: &str, account: &str| {
            json!({ "id": "x", "folder": folder, "accountPath": account, "model": "m", "effort": "e",
                    "permissionMode": "default", "worktree": false, "sandbox": "s", "approval": "a", "login": false })
        };
        for (cmd, args) in [
            ("ensure_claude_session", session(rel, elsewhere)),
            ("ensure_claude_session", session(up, elsewhere)),
            ("ensure_codex_session", session(elsewhere, up)),
            // A real folder, but no account of Luna's.
            ("ensure_claude_session", session(elsewhere, elsewhere)),
            ("session_meta", json!({ "id": "x", "accountPath": elsewhere })),
            ("delete_session", json!({ "id": "x", "folder": elsewhere, "worktreePath": up, "dropWorktree": true })),
            ("codex_trust_folder", json!({ "accountPath": elsewhere, "folder": elsewhere })),
            ("claude_trust_folder", json!({ "accountPath": elsewhere, "folder": elsewhere })),
            ("apply_recommended", json!({ "provider": "claude", "accountPath": elsewhere, "add": [], "remove": [] })),
            ("orphan_worktrees", json!({ "folder": rel, "inUse": [], "accountPaths": [] })),
            ("remove_orphan_worktrees", json!({ "folder": elsewhere, "inUse": [], "accountPaths": [], "paths": [up] })),
        ] {
            // Refused as the arguments are read: the session, trust or
            // worktree function behind it never runs.
            let err = dispatch(cmd, args).unwrap_err();
            let why = ["not a usable path", "account folder", "Documents directory"];
            assert!(err.starts_with("bad arguments") && why.iter().any(|w| err.contains(w)), "{cmd}: {err}");
        }
    }
}
