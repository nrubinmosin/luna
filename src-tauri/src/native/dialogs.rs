//! The commands of the native window's dialogs (luna_window's
//! `dialogs::cmd`): the new-chat dialog's look at the account's settings,
//! models, trust and folder, and the crash screen's. Each a name to a
//! function over the functions that do the work, its arguments read as the
//! window sends them and its paths held to boundary.rs's checks as they are
//! read. The sessions' and the helpers' commands (`delete_session`,
//! `background_*`, the trust writes) are the core's (corecmd.rs).

use super::Command;
use crate::boundary::{AbsPath, AccountPath, OrEmpty};
use serde::{Deserialize, Serialize};
use serde_json::Value;

fn args<T: serde::de::DeserializeOwned>(a: Value) -> Result<T, String> {
    serde_json::from_value(a).map_err(|e| format!("bad arguments: {e}"))
}

fn answer<T: Serialize>(value: T) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|e| e.to_string())
}

/// Every command of the dialogs' the window may call, by name.
pub(super) const OWN: &[(&str, Command)] = &[
    ("claude_defaults", |a| {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct A {
            account_path: AccountPath,
            folder: OrEmpty<AbsPath>,
        }
        let A { account_path, folder } = args(a)?;
        answer(crate::claude::defaults::claude_defaults(account_path.into(), folder.into_string()))
    }),
    ("claude_models", |a| {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct A {
            account_path: OrEmpty<AccountPath>,
            ask: Option<bool>,
        }
        let A { account_path, ask } = args(a)?;
        answer(crate::runtime::block_on(crate::claude::models::claude_models(account_path, ask)))
    }),
    ("codex_defaults", |a| {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct A {
            account_path: OrEmpty<AccountPath>,
            folder: OrEmpty<AbsPath>,
        }
        let A { account_path, folder } = args(a)?;
        answer(crate::runtime::block_on(crate::codex::defaults::codex_defaults(account_path, folder)))
    }),
    ("codex_models", |a| {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct A {
            account_path: OrEmpty<AccountPath>,
        }
        let A { account_path } = args(a)?;
        answer(crate::runtime::block_on(crate::codex::models::codex_models(account_path)))
    }),
    ("chat_folder_check", |a| {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct A {
            folder: AbsPath,
            worktree: bool,
        }
        let A { folder, worktree } = args(a)?;
        answer(crate::runtime::block_on(crate::pty::folder::chat_folder_check(folder, worktree)))
    }),
    ("claude_folder_trusted", |a| {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct A {
            account_path: AccountPath,
            folder: AbsPath,
        }
        let A { account_path, folder } = args(a)?;
        answer(crate::claude::trust::claude_folder_trusted(account_path, folder)?)
    }),
    ("crash_report", |_| answer(crate::lifecycle::previous_crash())),
    ("restart_window", super::desktop::restart_window),
];

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    /// The window's commands that may run a CLI at an account folder (a
    /// catalog miss runs Codex with `CODEX_HOME`, Claude Code with
    /// `CLAUDE_CONFIG_DIR`, at the path given), or read under one, take only
    /// an account of Luna's or none, through the window's own dispatch: any
    /// other path is refused as the arguments are read, before anything runs.
    #[test]
    fn commands_that_may_run_a_cli_take_only_an_account_or_none() {
        let elsewhere = if cfg!(windows) { r"C:\Windows" } else { "/etc" };
        let up = if cfg!(windows) { r"C:\p\..\Windows" } else { "/p/../etc" };
        for account in [elsewhere, "relative", up] {
            for (cmd, body) in [
                ("codex_models", json!({ "accountPath": account })),
                ("claude_models", json!({ "accountPath": account, "ask": true })),
                ("codex_defaults", json!({ "accountPath": account, "folder": "" })),
                ("claude_defaults", json!({ "accountPath": account, "folder": "" })),
                ("claude_folder_trusted", json!({ "accountPath": account, "folder": elsewhere })),
                ("account_limits", json!({ "accountPath": account, "force": true })),
                ("codex_limits", json!({ "accountPath": account, "force": true })),
                ("saved_title", json!({ "provider": "claude", "accountPath": account, "sessionId": "x" })),
            ] {
                let r = super::super::dispatch(cmd, body.clone());
                assert!(r.as_ref().is_err_and(|e| e.starts_with("bad arguments")), "{cmd} {body}: {r:?}");
            }
        }
        // A folder of the wrong shape beside no account is refused too.
        let r = super::super::dispatch("codex_defaults", json!({ "accountPath": "", "folder": up }));
        assert!(r.is_err_and(|e| e.starts_with("bad arguments")));
        // None given: nothing to read or run, and an answer all the same.
        let dispatch = |cmd: &str, body: Value| super::super::dispatch(cmd, body);
        assert_eq!(dispatch("codex_models", json!({ "accountPath": "" })), Ok(json!([])));
        assert_eq!(dispatch("claude_models", json!({ "accountPath": "" })), Ok(json!([])));
        assert!(dispatch("codex_defaults", json!({ "accountPath": "", "folder": "" })).is_ok());
        assert_eq!(dispatch("saved_title", json!({ "provider": "codex", "accountPath": "", "sessionId": "x" })), Ok(Value::Null));
    }
}
