//! Sessions run by sessions. A chat started with Luna's MCP server attached
//! can spawn other chats — another model, another account — send them text,
//! read what they answered, wait for them, and kill or delete them, worktree
//! included. This module is the registry behind those tools: who is who,
//! who may touch whom, and the round trips to the UI that owns the chat list.
//!
//! The core does not know chats, only pty sessions (see ARCHITECTURE.md), so
//! a spawn is a request to the frontend — `agent://spawn` out, `agent_spawned`
//! back — and a delete tells the frontend to drop the row after the session
//! and its worktree are gone. Lineage lives here: every session registered
//! with a parent is that parent's, and a caller may only reach its own
//! descendants — except to list every session, type a message into one and
//! read or wait on its answer, which is how sessions talk to each other; only
//! killing and deleting stay with the lineage. Children outlive their parent:
//! they pass to its parent, or with none stand alone, the window's to manage
//! (see `forget`).
//!
//! A helper does not report back on its own the way a subagent does: its
//! answer sits in its own transcript until someone waits for it or reads it.
//! A caller that asked something (spawn, send) and ended its turn without
//! collecting the answer would sit idle beside a helper that finished long
//! ago, so the notifier here types a one-line notice into the caller once it
//! is idle: the helper answered, stopped on a prompt, or exited.

mod answer;
mod ask;
mod delivery;
mod list;
mod lost;
mod notifier;
mod owed;
mod persist;
pub(crate) mod privilege;
mod read;
mod registry;
mod resume;
mod said;
pub(crate) mod spawn;
mod teardown;
#[cfg(test)]
mod test_support;
mod wait;

pub use answer::answer;
pub use ask::{on_hook, resolve_option, state_of, Ask, AskQuestion};
pub use delivery::{held_count, notes_dir, send};
pub use list::{list, sessions, SessionFilter};
pub use lost::undelivered_left;
#[cfg(test)]
pub(crate) use notifier::riders_go;
pub use notifier::{notify, NOTICE_TAG};
pub use owed::{resumed, retried};
pub use persist::{conversation_of, freeze_chats, restore_chats, save_chats, take_up_spawns, tie_back, ties_of, Ties};
pub(crate) use privilege::{command_prefix, command_reach_now};
pub use read::{read, Message};
pub(crate) use registry::mint;
pub use registry::{
    caller_of, chat_name, chat_names, has_tools, label, parent_of, parent_on_start, promoted, register, register_dev_caller,
    resumes_on_restart, session_exited, settings_of, DEV_CALLER,
};
pub use resume::agent_resumes;
pub use spawn::account::account_usage;
pub use spawn::worktree::{helper_worktree_name, name_to_go_on, report_codex_worktrees, worktree_for_start};
pub use spawn::{agent_spawned, agent_spawns, chat_starting, spawn, spawn_is, SpawnParams};
#[cfg(test)]
pub use teardown::news_for;
pub use teardown::{delete, delete_by_user, forget, kill, queued_refused};
pub use wait::{wait, wait_many};

use delivery::held_sender;
use notifier::notifier;

/// Starts the notifier and what types in the sends held for people, once.
pub fn init() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        std::thread::Builder::new().name("luna-notifier".into()).spawn(notifier).expect("spawn notifier thread");
        std::thread::Builder::new().name("luna-held".into()).spawn(held_sender).expect("spawn held sender thread");
    });
}
