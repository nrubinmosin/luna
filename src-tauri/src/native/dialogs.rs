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
