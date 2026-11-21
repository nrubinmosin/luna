//! The sessions in Luna's panes: one CLI process per chat, in a pty of its
//! own, that outlives any pane looking at it. This module holds the session
//! map; the rest is split by what it does with a session:
//!
//! - `start`: the commands that start a chat's CLI, and what each start
//!   settles under the chat's spawn gate (its token, the files it reads,
//!   what its sandbox takes from this machine).
//! - `folder`: the folder a session is to start in, refused before anything
//!   of the start is made when its CLI could not run there.
//! - `spawn`: the process in its pty, and the threads that write to it, read
//!   it and notice it has gone.
//! - `control`: keys, resizes, quitting and killing, deleting a chat.
//! - `screen`: what its output shows: the screen, the composer and a draft
//!   in it, the terminal modes and title its CLI set.
//! - `probe`: what is read of it from outside: the records' probes,
//!   its CLI's files on disk, and the sweep of files no session needs.
//!
//! How each CLI's arguments are laid out is `claude::launch` and
//! `codex::launch`.

mod control;
pub(crate) mod folder;
pub(crate) mod probe;
mod screen;
mod spawn;
#[cfg(any(test, luna_test))]
mod stand_in;
mod start;

pub(crate) use control::{
    delete_session, kill_session, kill_tree, orphan_sessions, resize_session, stop_all_sessions, update_called_off, write_session, KeyHold,
    GRACE,
};
pub(crate) use folder::folder_refusal;
pub(crate) use probe::{
    complete_lines, complete_lines_in, lines_from, lossy_lines, prune_session_files_periodically, read_lossy_lines, saved_title,
    sessions_meta, tail, ActivityProbe, MetaAsk,
};
pub(crate) use screen::{sane_title, session_modes, title_from_prompt, CliTitle, Composer, SessionMeta};
pub(crate) use spawn::{clean_env, finish_spec};
// The golden starts expect what a start finds of curl.
#[cfg(test)]
pub(crate) use start::codex_hook_curl;
#[cfg(test)]
pub(crate) use start::codex_modes;
pub(crate) use start::{
    claude_account_excludes, claude_instructions_dir, claude_mcp_file, claude_memory_off, claude_settings_file, codex_git_envs,
    codex_mcp_args, codex_sandbox_args, codex_sandbox_path, codex_trust, ensure_claude_session, ensure_codex_session, helper_prompt,
    session_envs, without_note, MCP_TOOL_TIMEOUT_MS,
};

use crate::provider::Provider;
use portable_pty::{ChildKiller, PtySize};
use screen::{Drawn, Modes, Scrollback, Sent};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// What the UI sends a session, in the order it sent it.
enum Input {
    Bytes(Vec<u8>),
    Resize(PtySize),
}

impl Input {
    /// What it counts for in the bytes waiting (`InputTx`).
    fn len(&self) -> u64 {
        match self {
            Input::Bytes(b) => b.len() as u64,
            Input::Resize(_) => 0,
        }
    }
}

/// The bytes of input a session's writer thread may have waiting before more
/// is refused: keystrokes, pastes and Luna's notices many times over. ConPTY
/// takes no bytes while the child is not draining them, and a CLI that has
/// stopped reading would otherwise have the core hold whatever is typed or
/// sent at it.
const INPUT_MAX: u64 = 8 * 1024 * 1024;

/// A session's end of its input queue: what is handed over goes to the
/// writer thread in order, counted by its bytes until it is written. Past
/// `INPUT_MAX` waiting, input is refused with an error, and so is all input
/// after it until the writer has caught up, so what does go in keeps its
/// order with no gap. The first refusal of a stretch is logged and told to
/// the window (`pty://input-refused`), which shows it in the pane: a
/// keystroke goes to the core as a send nobody waits on, and its error would
/// otherwise reach nobody. One write alone may be larger than the bound.
/// Resizes, and the core's own answers to its CLI's queries (`reply`), are
/// never refused.
struct InputTx {
    id: String,
    tx: std::sync::mpsc::Sender<Input>,
    /// Bytes handed over and not written yet; the writer gives back what it
    /// has written.
    waiting: Arc<crate::backlog::Backlog>,
}

/// What the window is told of input a session refused.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct InputRefused<'a> {
    id: &'a str,
    waiting_kb: u64,
}

impl InputTx {
    /// A queue for session `id`, with the end its writer thread reads and
    /// the backlog it gives written bytes back to.
    fn new(id: &str) -> (InputTx, std::sync::mpsc::Receiver<Input>, Arc<crate::backlog::Backlog>) {
        let (tx, rx) = std::sync::mpsc::channel();
        let waiting: Arc<crate::backlog::Backlog> = Arc::default();
        let theirs = Arc::clone(&waiting);
        (InputTx { id: id.to_string(), tx, waiting }, rx, theirs)
    }

    /// One nobody reads: a session whose process has gone takes no input.
    fn closed(id: &str) -> InputTx {
        Self::new(id).0
    }

    fn send(&self, input: Input) -> Result<(), String> {
        let len = input.len();
        if len == 0 {
            return self.hand_over(input, 0);
        }
        if let Err(refused) = self.waiting.reserve_in_order(len, INPUT_MAX) {
            let waiting_kb = refused.waiting / 1024;
            if refused.first {
                let id = &self.id;
                crate::log::warn("pty", &format!("{id}'s CLI is not taking input: {waiting_kb} KB waiting to be written; refusing more"));
                crate::emit::to_ui("pty://input-refused", InputRefused { id: &self.id, waiting_kb });
            }
            return Err(format!("the session is not taking input: {waiting_kb} KB waiting to be written"));
        }
        self.hand_over(input, len)
    }

    /// The core's own answer to a query its CLI made (the cursor's place):
    /// queued whatever waits, as a terminal would answer. A CLI that waits
    /// for it would otherwise wait for good once its input was refused.
    fn reply(&self, bytes: Vec<u8>) -> Result<(), String> {
        let len = bytes.len() as u64;
        self.waiting.force(len);
        self.hand_over(Input::Bytes(bytes), len)
    }

    /// `input` to the writer, its `len` bytes reserved already.
    fn hand_over(&self, input: Input, len: u64) -> Result<(), String> {
        self.tx.send(input).map_err(|_| {
            self.waiting.release(len);
            "session writer is gone".to_string()
        })
    }
}

/// How long whoever takes a session out of the map (a delete, a kill, a start
/// over a session that exited by itself) waits for its reader thread to have
/// said it exited (`ReaderEnd`), before the chat's id goes to anyone else.
/// The reader is through within moments of its process ending; past this it
/// says nothing more, and a delete or a kill says its exit for it (`retire`);
/// a start over it gives the chat's pane a session of its own instead.
const READER_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// A session's reader thread on its way out. Its word that the session
/// exited (`agents::session_exited`, `pty://exit`) goes by the chat id: said
/// after a respawn under that id had begun, it would mark the new session's
/// exit in the registry and tell its pane it had ended. So whoever takes a
/// session waits for it (`wait`), under the chat's spawn gate, and one that
/// is not through by then says nothing: one that has not begun to is
/// disowned here, and one already saying it is through before the chat's
/// spawn number changes (`PtyManager::generations`), which a word begun
/// later finds changed. A delete or a kill says one not said by then itself.
#[derive(Default)]
struct ReaderEnd {
    state: Mutex<EndState>,
    ended: std::sync::Condvar,
}

#[derive(Default)]
struct EndState {
    publishing: bool,
    done: bool,
    /// Whoever took the session stopped waiting: nothing is to be said.
    disowned: bool,
}

impl ReaderEnd {
    /// Runs `publish`, the reader's word that its session exited, unless
    /// whoever took the session has stopped waiting for it; then marks the
    /// reader through.
    fn finish(&self, publish: impl FnOnce()) {
        let go = {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            s.publishing = !s.disowned;
            s.publishing
        };
        if go {
            publish();
        }
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        (s.publishing, s.done) = (false, true);
        self.ended.notify_all();
    }

    /// Waits up to `within` for the reader to be through, and whether it was.
    /// Past that, a reader that has not begun to say anything never will;
    /// one that has may still be saying it.
    fn wait(&self, within: std::time::Duration) -> bool {
        let deadline = std::time::Instant::now() + within;
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if s.done {
                return true;
            }
            let now = std::time::Instant::now();
            if now >= deadline {
                s.disowned = !s.publishing;
                return false;
            }
            s = self.ended.wait_timeout(s, deadline - now).unwrap_or_else(|e| e.into_inner()).0;
        }
    }
}

struct Session {
    provider: Provider,
    /// Its spawn's number (`PtyManager::generations`), in its exit event and
    /// in what its start answers, so the window tells its exit from that of
    /// a session of the same chat before or after it.
    generation: u64,
    /// Its reader thread on its way out (`ReaderEnd`).
    reader_end: Arc<ReaderEnd>,
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
    /// that follow. What waits for the writer is bounded (`InputTx`).
    input_tx: Arc<InputTx>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    /// For asking whether the process is still there without blocking on it;
    /// the reader thread shares it to collect the exit code.
    child: Arc<Mutex<Box<dyn portable_pty::Child + Send + Sync>>>,
    scrollback: Arc<Mutex<Scrollback>>,
    /// Its screen as last drawn from that output (see `Drawn`).
    drawn: Arc<Mutex<Drawn>>,
    /// The terminal modes the CLI has switched, whole-session — see `Modes`.
    modes: Arc<Mutex<Modes>>,
    /// How far its output has gone out as events (see `Sent`), for the
    /// backlog a pane that attaches is given.
    sent: Arc<Mutex<Sent>>,
    alive: Arc<AtomicBool>,
    /// When the child last wrote anything: the key the drawn screen is cached
    /// by (screen.rs). Nothing judges whether the session works by it.
    last_output_ms: Arc<AtomicU64>,
    pid: Option<u32>,
    cwd: String,
    /// On the wall clock, taken just before the process was spawned.
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
    /// at (see `has_draft`): reading it draws the screen.
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
    /// The chats deleted in this core's life, which no start brings back
    /// (`delete`, `spawn`).
    deleted: Mutex<std::collections::HashSet<String>>,
    /// Each chat's session whose exit is still to be said, by its spawn's
    /// number (`Session::generation`); none once it has been, or once whoever
    /// took it has done waiting for it and said it in its stead. A reader
    /// says its session's exit under this lock and only while its number is
    /// the one here (`spawn::reader_exited`), and a start or a taker changes
    /// it under the same lock (`next_generation`, `retire`): an exit said
    /// late cannot land on the session after it, and none is said twice.
    generations: Mutex<HashMap<String, u64>>,
}

impl PtyManager {
    /// A new number for the session about to start as `id`, once the one
    /// before is through or past waiting for: from here, nothing of an
    /// earlier session of `id` is said to have exited, and one saying so
    /// right now is through before this returns.
    fn next_generation(&self, id: &str) -> u64 {
        // Counted on from when this core started, so no core after it gives
        // a number out again to a window that outlived this one; under 2^53,
        // as the page's numbers are.
        static NEXT: std::sync::OnceLock<AtomicU64> = std::sync::OnceLock::new();
        let mut generations = self.generations.lock().unwrap_or_else(|e| e.into_inner());
        let n = NEXT.get_or_init(|| AtomicU64::new(now_ms() << 10)).fetch_add(1, Ordering::SeqCst);
        generations.insert(id.to_string(), n);
        n
    }

    /// `id`'s session of spawn `generation` taken and waited for: an exit
    /// its reader has not said by now it never says, and this says it in
    /// its stead, once, with `code` (None where it cannot be read): a reader
    /// stuck past its wait would otherwise leave its pane, or the login
    /// window, never told the session is gone. What a reader is saying is
    /// through before this returns.
    fn retire(&self, id: &str, generation: u64, code: Option<u32>) {
        let mut generations = self.generations.lock().unwrap_or_else(|e| e.into_inner());
        if generations.get(id) == Some(&generation) {
            spawn::say_exit(&mut generations, id, generation, code);
        }
    }
}

/// The one session manager this process has, whatever asks for it: a
/// command, the agents, the records' reads, power, the threads a session
/// runs on.
pub fn manager() -> &'static PtyManager {
    static M: std::sync::OnceLock<PtyManager> = std::sync::OnceLock::new();
    M.get_or_init(PtyManager::default)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Writes input to the pty, whole, and flushes it.
fn write_input(writer: &mut impl std::io::Write, bytes: &[u8]) -> std::io::Result<()> {
    writer.write_all(bytes).and_then(|()| writer.flush())
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

    /// The chats whose sessions run, as an update's stop would end them
    /// (`shut_down_all`).
    pub fn live_ids(&self) -> Vec<String> {
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions.iter().filter(|(_, s)| s.alive.load(Ordering::SeqCst)).map(|(id, _)| id.clone()).collect()
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

    /// Input past the bound waiting is refused with an error, and so is all
    /// after it until the writer has caught up; one write larger than the
    /// bound goes on its own, and a resize always goes.
    #[test]
    fn input_past_its_bound_is_refused_until_the_writer_catches_up() {
        let (tx, rx, waiting) = InputTx::new("in-bound");
        let half = (INPUT_MAX / 2) as usize;
        assert!(tx.send(Input::Bytes(vec![b'a'; half])).is_ok());
        assert!(tx.send(Input::Bytes(vec![b'b'; half])).is_ok());
        let refused = tx.send(Input::Bytes(b"c".to_vec())).unwrap_err();
        assert!(refused.contains("not taking input"), "{refused}");
        assert!(tx.send(Input::Resize(PtySize { rows: 1, cols: 1, pixel_width: 0, pixel_height: 0 })).is_ok());
        // The writer takes one: room again, but nothing goes in before it has them all.
        let first = rx.recv().unwrap();
        waiting.release(first.len());
        assert!(tx.send(Input::Bytes(b"d".to_vec())).is_err(), "no gap: refused until caught up");
        // The core's answer to a cursor query goes all the same, behind what waits.
        tx.reply(b"\x1b[1;1R".to_vec()).unwrap();
        let rest: Vec<Input> = rx.try_iter().collect();
        assert!(matches!(rest.last(), Some(Input::Bytes(b)) if b == b"\x1b[1;1R"), "the answer came last");
        for input in rest {
            waiting.release(input.len());
        }
        assert!(tx.send(Input::Bytes(b"e".to_vec())).is_ok());
        let e = rx.recv().unwrap();
        waiting.release(e.len());
        assert!(tx.send(Input::Bytes(vec![b'f'; INPUT_MAX as usize + 1])).is_ok(), "one write alone may be larger");
        assert!(InputTx::closed("in-gone").send(Input::Bytes(b"x".to_vec())).is_err());
    }

    #[test]
    fn a_login_session_is_told_by_its_folder() {
        assert!(same_dir("C:\\acc\\anthropic\\one\\", "C:/acc/anthropic/one"));
        assert!(!same_dir("C:\\p", "C:\\acc\\anthropic\\one"));
    }
}
