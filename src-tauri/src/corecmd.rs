//! The commands the core answers for the window: everything that touches the
//! running sessions. The window passes them on (corelink.rs `request` /
//! `core_send`) and they land here, by name and with camelCase arguments.
//!
//! Only the commands in `COMMANDS` come through, each on its own lane (the
//! window sends each on it, and the core's endpoint takes it on no other,
//! coreproc.rs `off_lane`), and their path arguments are typed (boundary.rs):
//! a path of the wrong shape, or an account folder Luna does not list, is
//! refused as the arguments are read, before the function runs.

use crate::boundary::{AbsPath, AccountPath};
use crate::provider::Provider;
use serde_json::Value;
use std::collections::HashMap;

/// How a command of the window's reaches the core.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lane {
    /// `request`: answered, and waited on.
    Call,
    /// `core_send`: keystrokes and resizes, written in order, never answered.
    Send,
}

/// Every command the window passes on to the core, and its lane. The window
/// refuses any other, or one on another lane (native.rs, corelink.rs
/// `passes`), and so does the core: `dispatch` any other, the endpoint one
/// on another lane (coreproc.rs `off_lane`). The protocol's own `hello`,
/// `info` and `shutdown` are not here, and only the link itself sends them.
/// native.rs's tests hold the window's names to these.
pub const COMMANDS: &[(&str, Lane)] = &[
    ("ensure_claude_session", Lane::Call),
    ("ensure_codex_session", Lane::Call),
    ("write_session", Lane::Send),
    ("resize_session", Lane::Send),
    ("kill_session", Lane::Call),
    ("delete_session", Lane::Call),
    ("stop_all_sessions", Lane::Call),
    ("update_called_off", Lane::Call),
    ("orphan_sessions", Lane::Call),
    ("kill_orphan", Lane::Call),
    ("orphan_live", Lane::Call),
    ("sessions_meta", Lane::Call),
    ("session_modes", Lane::Call),
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
    ("sync_settings_now", Lane::Call),
    ("codex_folder_trusted", Lane::Call),
    ("codex_trust_folder", Lane::Call),
    ("claude_trust_folder", Lane::Call),
];

/// The lane `cmd` comes on, or None for a command the window may not pass on.
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

/// Deserializes the window's arguments into the function's parameters, calls
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
        Ok(serde_json::to_value(crate::runtime::block_on($f($(_a.$name.arg()),*))).map_err(|e| e.to_string())?)
    }};
    (wait res $args:expr, $f:path $(, $name:ident : $ty:ty)*) => {{
        let _a = call!(@args $args, $($name : $ty),*);
        Ok(serde_json::to_value(crate::runtime::block_on($f($(_a.$name.arg()),*))?).map_err(|e| e.to_string())?)
    }};
}

/// Answers one command. Blocks for as long as the command takes; the core
/// runs each on a thread of its own, keystrokes and the few that must keep
/// their order excepted (`in_order`).
pub fn dispatch(cmd: &str, args: Value) -> Result<Value, String> {
    dispatch_as(cmd, args, None, crate::agents::update_call_offs())
}

/// `dispatch`, asked on a connection from the window process `asker`
/// (coreproc.rs `requests`): an update's stop it starts makes it the
/// update's asker (`agents::freeze_for_update`). `call_offs`: the update's
/// call offs counted as the request was read, which an update's stop is
/// held to (`agents::freeze_for_update`).
pub fn dispatch_as(cmd: &str, args: Value, asker: Option<crate::agents::Window>, call_offs: u64) -> Result<Value, String> {
    use crate::{account_settings, accounts, agents, power, pty};
    if lane(cmd).is_none() {
        return Err(format!("the core has no command {cmd}"));
    }
    // A command with no arguments may come with none at all.
    let args = if args.is_null() { Value::Object(Default::default()) } else { args };
    match cmd {
        "ensure_claude_session" => call!(wait res args, pty::ensure_claude_session,
            id: String, folder: AbsPath, account_path: AccountPath, model: String, effort: String,
            permission_mode: String, worktree: bool, worktree_name: Option<String>, resume: Option<String>,
            tools: Option<bool>, read_only: Option<bool>, agent: Option<String>, account_id: Option<String>),
        "ensure_codex_session" => call!(wait res args, pty::ensure_codex_session,
            id: String, folder: AbsPath, account_path: AccountPath, model: Option<String>, effort: String,
            approval: String, sandbox: String, worktree: Option<bool>, resume: Option<String>, login: bool,
            tools: Option<bool>, account_id: Option<String>),
        "write_session" => call!(res args, pty::write_session, id: String, data: String, reply: Option<bool>),
        "resize_session" => {
            call!(res args, pty::resize_session, id: String, cols: u16, rows: u16, pixel_width: Option<u16>, pixel_height: Option<u16>)
        }
        "kill_session" => call!(wait res args, pty::kill_session, id: String),
        "kill_orphan" => call!(wait res args, pty::kill_orphan, id: String, generation: u64),
        "orphan_live" => call!(ok args, pty::orphan_live, id: String, generation: u64),
        "delete_session" => call!(wait res args, pty::delete_session, id: String),
        "stop_all_sessions" => {
            let a = call!(@args args, confirmed: Vec<String>);
            serde_json::to_value(crate::runtime::block_on(pty::stop_all_sessions(asker, a.confirmed, call_offs))?)
                .map_err(|e| e.to_string())
        }
        "update_called_off" => call!(ok args, pty::update_called_off),
        "orphan_sessions" => call!(wait res args, pty::orphan_sessions, known: Vec<String>),
        // Each chat's account path is checked there, once a path a call.
        "sessions_meta" => call!(ok args, pty::sessions_meta, chats: Vec<pty::MetaAsk>),
        "session_modes" => call!(ok args, pty::session_modes, id: String),
        "background_helpers" => call!(ok args, crate::background::background_helpers),
        "background_promote" => call!(res args, crate::background::background_promote, id: String),
        "background_promoted" => call!(res args, crate::background::background_promoted, id: String, error: Option<String>),
        "background_kill" => call!(ok args, crate::background::background_kill, id: String),
        "background_delete" => call!(res args, crate::background::background_delete,
            id: String, stop_working: Option<bool>, of_chat: Option<String>),
        "background_at_stake" => call!(ok args, crate::background::background_at_stake, ids: Vec<String>),
        "background_transcript" => call!(res args, crate::background::background_transcript,
            id: String, since: Option<u64>, file: Option<String>),
        "chat_names" => call!(ok args, agents::chat_names,
            names: HashMap<String, String>, groups: Option<HashMap<String, u32>>),
        "account_usage" => call!(ok args, agents::account_usage, usage: HashMap<String, Value>, sender: Option<String>, seq: Option<u64>),
        "power_state" => call!(ok args, power::power_state),
        "set_keep_awake" => call!(res args, power::set_keep_awake, on: bool),
        "arm_power_off" => call!(res args, power::arm_power_off, action: String, quiet_s: u64, drain: Option<bool>),
        "disarm_power_off" => call!(ok args, power::disarm_power_off),
        "rename_account" => call!(res args, accounts::rename_account, provider: Provider, name: String, to: String),
        "sync_settings_now" => call!(res args, account_settings::sync_settings_now, provider: Provider),
        "codex_folder_trusted" => call!(res args, crate::codex::trust::codex_folder_trusted, account_path: AccountPath, folder: AbsPath),
        "codex_trust_folder" => call!(res args, crate::codex::trust::codex_trust_folder, account_path: AccountPath, folder: AbsPath),
        "claude_trust_folder" => call!(res args, crate::claude::trust::claude_trust_folder, account_path: AccountPath, folder: AbsPath),
        other => Err(format!("the core has no command {other}")),
    }
}

/// Calls answered on the connection's own thread, one after another, in the
/// order they came: what the window asks them for depends on that order (a
/// reattach reads the modes after the session it ensured), and each takes no
/// time. Keystrokes and resizes, sent unanswered, run there too.
pub fn in_order(cmd: &str) -> bool {
    matches!(cmd, "session_modes" | "chat_names" | "account_usage" | "power_state")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The writes that may wait out a model ask's lock on the account's
    /// `.claude.json` (account_settings `rewriting_claude_config`, up to 35 s,
    /// claude/models.rs `ASK_WAIT`) reach the core by `core_call`, which the
    /// window runs off its own thread, and the core answers each on a thread
    /// of its own: neither the window nor the core's connection waits on them.
    #[test]
    fn writes_that_may_wait_on_a_model_ask_run_off_every_shared_thread() {
        let cmd = "claude_trust_folder";
        assert!(COMMANDS.iter().any(|(c, lane)| *c == cmd && matches!(lane, Lane::Call)), "{cmd} is a core call");
        assert!(!in_order(cmd), "{cmd} is not answered on the connection's thread");
    }

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
        // The call dispatcher refuses protocol commands and retired calls.
        for cmd in ["hello", "info", "shutdown", "background_move", "background_move_targets", ""] {
            assert!(lane(cmd).is_none(), "{cmd}");
            assert!(dispatch(cmd, Value::Null).unwrap_err().contains("no command"), "{cmd}");
        }
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
            ("codex_folder_trusted", json!({ "accountPath": elsewhere, "folder": elsewhere })),
            ("codex_folder_trusted", json!({ "accountPath": up, "folder": elsewhere })),
            ("codex_trust_folder", json!({ "accountPath": elsewhere, "folder": elsewhere })),
            ("claude_trust_folder", json!({ "accountPath": elsewhere, "folder": elsewhere })),
        ] {
            // Refused as the arguments are read: the session or trust function
            // behind it never runs.
            let err = dispatch(cmd, args).unwrap_err();
            let why = ["not a usable path", "account folder", "Documents directory"];
            assert!(err.starts_with("bad arguments") && why.iter().any(|w| err.contains(w)), "{cmd}: {err}");
        }
    }

    /// One look at every chat: each answered, in the order asked, and a chat
    /// whose account path is none of Luna's answered as one with no session,
    /// nothing under the path read. One bad path does not fail the others.
    #[test]
    fn every_chat_is_looked_at_in_one_call_a_bad_account_path_answering_nothing() {
        let elsewhere = if cfg!(windows) { r"C:\Windows" } else { "/etc" };
        let chats = json!({ "chats": [
            { "id": "sm-a", "accountPath": elsewhere },
            { "id": "sm-b", "accountPath": "projects/p" },
            { "id": "sm-c", "accountPath": elsewhere },
        ] });
        let none = |id: &str| json!({ "id": id, "meta": null });
        assert_eq!(dispatch("sessions_meta", chats), Ok(json!([none("sm-a"), none("sm-b"), none("sm-c")])));
        assert_eq!(dispatch("sessions_meta", json!({ "chats": [] })), Ok(json!([])));
        assert!(dispatch("sessions_meta", json!({ "chats": [{ "id": "x" }] })).unwrap_err().starts_with("bad arguments"));
    }
}
