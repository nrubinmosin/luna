//! The commands the core answers for the window: everything that touches the
//! running sessions. The page calls them as it always has; the window passes
//! them on (corelink.rs `core_call` / `core_send`) and they land here, by name
//! and with the page's own camelCase arguments, on the same functions the
//! Tauri commands are.

use crate::provider::Provider;
use serde_json::Value;
use std::collections::HashMap;

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
        Ok(serde_json::to_value($f($(_a.$name),*)).map_err(|e| e.to_string())?)
    }};
    (res $args:expr, $f:path $(, $name:ident : $ty:ty)*) => {{
        let _a = call!(@args $args, $($name : $ty),*);
        Ok(serde_json::to_value($f($(_a.$name),*)?).map_err(|e| e.to_string())?)
    }};
    (wait ok $args:expr, $f:path $(, $name:ident : $ty:ty)*) => {{
        let _a = call!(@args $args, $($name : $ty),*);
        Ok(serde_json::to_value(tauri::async_runtime::block_on($f($(_a.$name),*))).map_err(|e| e.to_string())?)
    }};
    (wait res $args:expr, $f:path $(, $name:ident : $ty:ty)*) => {{
        let _a = call!(@args $args, $($name : $ty),*);
        Ok(serde_json::to_value(tauri::async_runtime::block_on($f($(_a.$name),*))?).map_err(|e| e.to_string())?)
    }};
}

/// Answers one command. Blocks for as long as the command takes; the core
/// runs each on a thread of its own, keystrokes and the few that must keep
/// their order excepted (`in_order`).
pub fn dispatch(cmd: &str, args: Value) -> Result<Value, String> {
    use crate::{accounts, agents, power, pty, worktree};
    // A command with no arguments may come with none at all.
    let args = if args.is_null() { Value::Object(Default::default()) } else { args };
    match cmd {
        "ensure_claude_session" => call!(wait res args, pty::ensure_claude_session,
            id: String, folder: String, account_path: String, model: String, effort: String,
            permission_mode: String, worktree: bool, worktree_name: Option<String>, resume: Option<String>,
            tools: Option<bool>, parent: Option<String>, prompt: Option<String>),
        "ensure_codex_session" => call!(wait res args, pty::ensure_codex_session,
            id: String, folder: String, account_path: String, model: Option<String>, effort: String,
            approval: String, sandbox: String, resume: Option<String>, login: bool, tools: Option<bool>,
            parent: Option<String>, prompt: Option<String>),
        "write_session" => call!(res args, pty::write_session, id: String, data: String),
        "resize_session" => call!(res args, pty::resize_session, id: String, cols: u16, rows: u16),
        "kill_session" => call!(wait res args, pty::kill_session, id: String),
        "delete_session" => call!(wait res args, pty::delete_session,
            id: String, folder: String, account_path: String, worktree_path: Option<String>, drop_worktree: bool),
        "stop_all_sessions" => call!(wait res args, pty::stop_all_sessions),
        "orphan_sessions" => call!(wait res args, pty::orphan_sessions, known: Vec<String>),
        "session_meta" => call!(wait res args, pty::session_meta, id: String, account_path: String),
        "session_modes" => call!(ok args, pty::session_modes, id: String),
        "agent_spawned" => call!(ok args, agents::agent_spawned,
            request_id: u64, chat_id: Option<String>, name: Option<String>, error: Option<String>),
        "chat_names" => call!(ok args, agents::chat_names,
            names: HashMap<String, String>, groups: Option<HashMap<String, u32>>),
        "account_usage" => call!(ok args, agents::account_usage, usage: HashMap<String, Value>),
        "power_state" => call!(ok args, power::power_state),
        "set_keep_awake" => call!(res args, power::set_keep_awake, on: bool),
        "arm_power_off" => call!(res args, power::arm_power_off, action: String, quiet_s: u64),
        "disarm_power_off" => call!(ok args, power::disarm_power_off),
        "rename_account" => call!(res args, accounts::rename_account, provider: Provider, name: String, to: String),
        "orphan_worktrees" => call!(wait res args, worktree::orphan_worktrees,
            folder: String, in_use: Vec<String>, account_paths: Vec<String>),
        "remove_orphan_worktrees" => call!(wait res args, worktree::remove_orphan_worktrees,
            folder: String, in_use: Vec<String>, account_paths: Vec<String>, paths: Vec<String>),
        other => Err(format!("the core has no command {other}")),
    }
}

/// Commands answered on the connection's own thread, one after another, in
/// the order they came: what the page sends them for depends on that order
/// (a reattach reads the modes after the session it ensured), and each takes
/// no time.
pub fn in_order(cmd: &str) -> bool {
    matches!(
        cmd,
        "write_session" | "resize_session" | "session_modes" | "chat_names" | "account_usage" | "agent_spawned" | "power_state"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_arrive_as_the_page_names_them() {
        // A session that is not there, asked for in the page's own words:
        // the call reaches the function and its answer comes back.
        assert_eq!(dispatch("session_modes", serde_json::json!({ "id": "nobody" })), Ok(Value::from("")));
        assert!(dispatch("write_session", serde_json::json!({ "id": "nobody", "data": "x" })).unwrap_err().contains("no such session"));
        assert!(dispatch("session_modes", serde_json::json!({})).unwrap_err().contains("bad arguments"));
        assert!(dispatch("nope", Value::Null).unwrap_err().contains("no command"));
        assert!(dispatch("power_state", Value::Null).is_ok(), "no arguments at all");
    }
}
