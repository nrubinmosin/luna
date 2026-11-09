//! The sessions in Luna's panes: one CLI process per chat, in a pty of its
//! own, that outlives any pane looking at it. This module holds the session
//! map; the rest is split by what it does with a session:
//!
//! - `start`: the commands that start a chat's CLI, and what each start
//!   settles under the chat's spawn gate (its token, the files it reads,
//!   what its sandbox and guard take from this machine).
//! - `folder`: the folder a session is to start in, refused before anything
//!   of the start is made when its CLI could not run there.
//! - `spawn`: the process in its pty, and the threads that write to it, read
//!   it and notice it has gone.
//! - `control`: keys, resizes, quitting and killing, deleting a chat.
//! - `screen`: what its output shows: the screen, the composer and a draft
//!   in it, the terminal modes and title its CLI set.
//! - `probe`: what is read of it from outside: the activity sampler's probes,
//!   its CLI's files on disk, and the sweep of files no session needs.
//!
//! How each CLI's arguments are laid out is `claude::launch` and
//! `codex::launch`.

mod control;
pub(crate) mod folder;
pub(crate) mod probe;
mod screen;
mod spawn;
mod start;

pub(crate) use control::{
    delete_session, kill_session, kill_tree, orphan_sessions, resize_session, stop_all_sessions, write_session, KeyHold, GRACE,
};
pub(crate) use folder::folder_refusal;
pub(crate) use probe::{
    complete_lines, complete_lines_in, live_cwds, lossy_lines, prune_session_files_periodically, read_lossy_lines, session_meta, tail,
    ActivityProbe,
};
pub(crate) use screen::{sane_title, session_modes, title_from_prompt, CliTitle, Composer, SessionMeta};
pub(crate) use spawn::{clean_env, finish_spec};
#[cfg(test)]
pub(crate) use start::curl_program;
pub(crate) use start::{
    claude_account_excludes, claude_instructions_dir, claude_mcp_file, claude_memory_off, claude_settings_file, codex_git_envs,
    codex_guard_args, codex_mcp_args, codex_sandbox_args, codex_sandbox_path, codex_trust, ensure_claude_session, ensure_codex_session,
    helper_line, helper_prompt, session_envs, without_note, MCP_TOOL_TIMEOUT_MS,
};

use crate::provider::Provider;
use portable_pty::{ChildKiller, PtySize};
use screen::Modes;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// What the UI sends a session, in the order it sent it.
enum Input {
    Bytes(Vec<u8>),
    Resize(PtySize),
}

struct Session {
    provider: Provider,
    /// Input is handed to a writer thread instead of being written inline.
    /// ConPTY stops accepting bytes while the child is not draining them, and a
    /// write that blocks used to do so while holding the session map — freezing
    /// every other chat's IPC behind one busy pane, for hundreds of ms at a
    /// time. A channel also keeps keystrokes in the order they were typed,
    /// which handing each write its own lock would not.
    ///
    /// Resizes travel the same channel, for the same reason: ConPTY's resize
    /// is synchronous and stalls like a write while the child is busy, and it
    /// used to run on the main thread under the session map — the log showed
    /// every pane's keystrokes queued for over a second behind one pane's
    /// window drag. The writer thread owns the master end, so the resize
    /// happens there, after the bytes that preceded it and before the ones
    /// that follow.
    input_tx: Arc<std::sync::mpsc::Sender<Input>>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    /// For asking whether the process is still there without blocking on it;
    /// the reader thread shares it to collect the exit code.
    child: Arc<Mutex<Box<dyn portable_pty::Child + Send + Sync>>>,
    scrollback: Arc<Mutex<VecDeque<u8>>>,
    /// The terminal modes the CLI has switched, whole-session — see `Modes`.
    modes: Arc<Mutex<Modes>>,
    alive: Arc<AtomicBool>,
    /// When the child last wrote anything — Codex has no live status file, so
    /// a quiet screen mid-turn is how an approval prompt is told apart from
    /// work in progress.
    last_output_ms: Arc<AtomicU64>,
    pid: Option<u32>,
    cwd: String,
    spawned_at_ms: u128,
    /// The session id this was resumed with, if any: Codex is found on disk
    /// by it.
    resume: Option<String>,
    /// Kept so a session that has lost its chat row can still be described:
    /// its title and status live under this account.
    account_path: String,
    /// The job the CLI was put in at spawn, for listing what it has running
    /// under it (procs/). None where Windows refused or off Windows.
    job: Option<Arc<crate::procs::Job>>,
    /// The last size the pty was given (rows, cols), so `screen` can render
    /// the scrollback the way the CLI laid it out.
    size: Arc<Mutex<(u16, u16)>>,
    /// When a person last typed into it (see `write_session`), so a notice
    /// from Luna does not land in the middle of their sentence.
    typed_ms: Arc<AtomicU64>,
    /// Characters a person has typed since their last Enter, as far as the
    /// keys tell (see `draft_after`): a notice typed on top of a draft would
    /// go out with it.
    draft: Arc<AtomicU64>,
    /// The screen's word on the draft, for the keys and output it was read
    /// at (see `has_draft`): reading it replays the scrollback.
    draft_seen: Arc<Mutex<(u64, bool)>>,
    /// A person's keys, kept back while Luna types into the composer around
    /// their draft (see `hold_keys`); None the rest of the time.
    held_keys: Arc<Mutex<Option<Vec<String>>>>,
    /// The person has pressed Claude Code's Ctrl+S, and a draft of theirs may
    /// be stashed, to come back unasked (see `has_draft`).
    stashing: Arc<AtomicBool>,
}

#[derive(Default)]
pub struct PtyManager {
    sessions: Mutex<HashMap<String, Session>>,
    /// One lock per chat id, held for as long as that chat is being spawned.
    /// It replaces holding `sessions` across the spawn: two calls for the same
    /// chat still cannot race into two processes, while a chat starting up no
    /// longer blocks every other pane's keystrokes.
    spawning: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

/// The one session manager this process has, whatever asks for it: a
/// command, the agents, the activity sampler, power, the threads a session
/// runs on.
pub fn manager() -> &'static PtyManager {
    static M: std::sync::OnceLock<PtyManager> = std::sync::OnceLock::new();
    M.get_or_init(PtyManager::default)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Whether two paths name the same folder, slashes and a trailing separator
/// aside — and case, where the file system ignores it.
pub fn same_dir(a: &str, b: &str) -> bool {
    let norm = |p: &str| p.replace('\\', "/").trim_end_matches('/').to_string();
    let (a, b) = (norm(a), norm(b));
    if cfg!(windows) {
        a.eq_ignore_ascii_case(&b)
    } else {
        a == b
    }
}

impl PtyManager {
    /// The lock a spawn of this chat holds for as long as it runs.
    fn gate(&self, id: &str) -> Arc<Mutex<()>> {
        let mut gates = self.spawning.lock().unwrap_or_else(|e| e.into_inner());
        // Nobody holds a gate whose only reference is the map's own, so this
        // keeps the table to the chats actually starting right now.
        gates.retain(|_, g| Arc::strong_count(g) > 1);
        Arc::clone(gates.entry(id.to_string()).or_default())
    }

    pub fn provider(&self, id: &str) -> Option<Provider> {
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions.get(id).map(|s| s.provider)
    }

    /// Whether a running session or a background helper uses this account
    /// folder, or a CLI of theirs has yet to exit.
    pub fn account_in_use(&self, account_path: &str) -> bool {
        if crate::account_use::running(account_path) {
            return true;
        }
        let running = {
            let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
            sessions.values().any(|s| s.alive.load(Ordering::SeqCst) && same_dir(&s.account_path, account_path))
        };
        // Asked with the session map let go: no path holds both locks.
        running || crate::background::uses_account(account_path)
    }

    /// How many sessions are running.
    pub fn live_count(&self) -> usize {
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions.values().filter(|s| s.alive.load(Ordering::SeqCst)).count()
    }

    pub fn alive(&self, id: &str) -> bool {
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions.get(id).map(|s| s.alive.load(Ordering::SeqCst)).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_login_session_is_told_by_its_folder() {
        assert!(same_dir("C:\\acc\\anthropic\\one\\", "C:/acc/anthropic/one"));
        assert!(!same_dir("C:\\p", "C:\\acc\\anthropic\\one"));
    }
}
