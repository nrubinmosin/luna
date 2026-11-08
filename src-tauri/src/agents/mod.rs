//! Sessions run by sessions. A chat started with Luna's MCP server attached
//! can spawn helpers — another model, another account — send them text, read
//! what they answered, wait for them, and stop or delete them, worktree
//! included. This module is the registry behind those tools: who is who,
//! who may touch whom.
//!
//! Every helper runs in the background, in the core (background/); the window
//! runs only the person's own chats (docs/adr/0001-session-state-lives-in-the-cli.md),
//! and a helper the person opens as a chat is handed over to them for good
//! (handover.rs). Lineage lives here: every session registered with a parent is
//! that parent's, and a caller may only reach its own descendants — except to
//! list every session, and, told `scope: "all"`, to type a message into one and
//! read or wait on its answer, which is how sessions talk to each other (the
//! message says whose it is); interrupting, stopping and deleting stay with the
//! lineage. Helpers outlive their parent: they pass to its parent, or with none
//! stand alone, the window's to manage (see `forget`).
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
mod handover;
mod held;
mod list;
mod lost;
mod notifier;
mod owed;
mod persist;
pub(crate) mod privilege;
mod read;
mod registry;
mod said;
pub(crate) mod spawn;
mod teardown;
#[cfg(test)]
mod test_support;
mod wait;

pub use answer::answer;
pub use ask::{on_hook, resolve_option, state_of, Ask, AskQuestion};
pub use delivery::{notes_dir, send};
pub use handover::take_over;
pub use held::held_count;
pub use list::{list, sessions, SessionFilter};
pub use lost::undelivered_left;
#[cfg(test)]
pub(crate) use notifier::riders_go;
pub use notifier::{notify, NOTICE_TAG};
pub use owed::{resumed, retried};
pub use persist::{freeze_chats, restore_chats, save_chats, take_up_spawns, tie_back, ties_of, Ties};
pub(crate) use privilege::{command_prefix, command_reach_now};
pub use read::{read, Message};
pub(crate) use registry::mint;
pub use registry::{
    applied_of, caller_of, chat_name, chat_names, has_tools, in_scope, label, own_or_self, parent_of, register, register_dev_caller,
    resumes_on_restart, scope_all, session_exited, set_applied, settings_of, DEV_CALLER,
};
pub use spawn::account::account_usage;
pub use spawn::worktree::{name_to_go_on, report_codex_worktrees};
pub use spawn::{spawn, SpawnParams};
#[cfg(test)]
pub use teardown::news_for;
pub use teardown::{at_stake, delete, delete_by_user, forget, kill, queued_refused, AtStake, UserDeleted};
pub use wait::{wait, wait_many};

use held::held_sender;
use notifier::notifier;

/// Starts the notifier and what types in the sends held for people, once.
pub fn init() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        std::thread::Builder::new().name("luna-notifier".into()).spawn(notifier).expect("spawn notifier thread");
        std::thread::Builder::new().name("luna-held".into()).spawn(held_sender).expect("spawn held sender thread");
    });
}
