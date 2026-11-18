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
#[cfg(test)]
pub(crate) use control::{fake_person_keys, fake_terminal_for_test, fake_terminal_of};
pub(crate) use folder::folder_refusal;
pub(crate) use probe::{
    complete_lines, complete_lines_in, live_cwds, lossy_lines, prune_session_files_periodically, read_lossy_lines, sessions_meta, tail,
    unplaced_worktrees, ActivityProbe, MetaAsk,
};
pub(crate) use screen::{sane_title, session_modes, title_from_prompt, CliTitle, Composer, SessionMeta};
pub(crate) use spawn::{clean_env, finish_spec};
#[cfg(test)]
pub(crate) use start::curl_program;
pub(crate) use start::{
    claude_account_excludes, claude_instructions_dir, claude_mcp_file, claude_memory_off, claude_settings_file, codex_git_envs,
    codex_guard_args, codex_mcp_args, codex_role_args, codex_sandbox_args, codex_sandbox_path, codex_trust, ensure_claude_session,
    ensure_codex_session, helper_prompt, session_envs, without_note, MCP_TOOL_TIMEOUT_MS,
};

use crate::provider::Provider;
use portable_pty::{ChildKiller, PtySize};
use screen::{Drawn, Modes, Scrollback};
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
    alive: Arc<AtomicBool>,
    /// When the child last wrote anything — Codex has no live status file, so
    /// a quiet screen mid-turn is how an approval prompt is told apart from
    /// work in progress.
    last_output_ms: Arc<AtomicU64>,
    /// `last_output_ms` without the repaints Luna's nudges bring on
    /// (`reader_stamp`): what the activity sampler counts as the session
    /// printing. A pane shown, refit or nudged resizes the pty, focus moving
    /// between panes sends focus reports, and the CLI's redraw that follows
    /// is Luna's doing, not the session at work.
    work_output_ms: Arc<AtomicU64>,
    pid: Option<u32>,
    cwd: String,
    /// The worktree it was started into though that is not its `cwd`
    /// (`Launch::enters`): in it from the start for whoever asks whether a
    /// worktree is in use, where Claude Code's registry says so only once the
    /// CLI has come up.
    enters: Option<String>,
    /// Started with a new worktree whose path its CLI picks
    /// (`Launch::makes_worktree`): until its working folder says where, what
    /// appears where its CLI makes them may be its own (`unplaced_worktrees`).
    makes_worktree: bool,
    /// On the wall clock, taken just before the process was spawned.
    spawned_at_ms: u128,
    /// The same moment on the monotonic clock (`worktree::Unplaced::since`).
    spawned_at: std::time::Instant,
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
    /// The same count of a person's keys, never cleared by a look at the
    /// screen: the veto a move's line goes by (`keys_left_text`).
    keys_draft: Arc<AtomicU64>,
    /// The last keys went to a dialog, not the composer (`composer_keys`).
    keys_dialog: Arc<AtomicBool>,
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
/// command, the agents, the activity sampler, power, the threads a session
/// runs on.
pub fn manager() -> &'static PtyManager {
    static M: std::sync::OnceLock<PtyManager> = std::sync::OnceLock::new();
    M.get_or_init(PtyManager::default)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Milliseconds on a clock that never steps back, counted from the first time
/// this process read it. For spans between two moments of this process only
/// (the repaint window below); a wall-clock stamp is `now_ms`.
fn mono_ms() -> u64 {
    static BASE: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    BASE.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64
}

/// How long after a nudge a session's output is taken for the repaint it
/// brought on. A nudge is what the writer passes on that makes an idle CLI
/// redraw: a resize, or a write of focus reports alone (`only_focus_reports`).
/// A CLI redraws its screen within a frame or two of a resize (Codex's reflow
/// within about 225 ms, docs/cli-compatibility.md); a pane's repaint nudge is
/// two resizes 60 ms apart. Each nudge starts the window over, so while a
/// split is dragged nothing the session prints counts until 1 s after the
/// last resize, and a short burst of real output wholly inside the window is
/// never counted; a turn at work, or a process under it, still keeps the
/// session busy. A heuristic: a repaint delivered more than 1 s after its
/// nudge (a machine under heavy load) still counts as output.
pub(crate) const REPAINT_WINDOW_MS: u64 = 1_000;

/// Whether output read at `now` is the session's own, rather than the repaint
/// of the nudge the writer last passed on at `last_nudge` (None: never). Both
/// on `mono_ms`'s clock, so a wall clock set back cannot stretch the window.
fn is_work_output(now: u64, last_nudge: Option<u64>) -> bool {
    last_nudge.is_none_or(|at| now.saturating_sub(at) > REPAINT_WINDOW_MS)
}

/// What the writer's nudge stamp holds before the first nudge.
const NEVER_NUDGED: u64 = u64::MAX;

/// What the reader stores as the work stamp (`Session::work_output_ms`) for
/// output read at `now_wall` (`now_ms`) and `now_mono` (`mono_ms`), the
/// writer's nudge stamp being `last_nudge` as it holds it (`NEVER_NUDGED`
/// before the first): `now_wall`, or None for a nudge's repaint. The window
/// is judged on the monotonic clock; the stamp is wall-clock time, as the
/// sampler compares it with its own now.
pub(crate) fn reader_stamp(now_wall: u64, now_mono: u64, last_nudge: u64) -> Option<u64> {
    let nudged = Some(last_nudge).filter(|&at| at != NEVER_NUDGED);
    is_work_output(now_mono, nudged).then_some(now_wall)
}

/// `reader_stamp` for output the reader thread has just read at `now_wall`.
fn reader_stamp_now(now_wall: u64, last_nudge: u64) -> Option<u64> {
    reader_stamp(now_wall, mono_ms(), last_nudge)
}

/// Whether input bytes are focus reports and nothing else: one or more of
/// `ESC [ I` (focus in) and `ESC [ O` (focus out), which xterm.js sends on its
/// own when focus moves into or out of a pane whose CLI turned on focus
/// reporting (`?1004`, kept in `KEPT_MODES`). A CLI redraws on them as on a
/// resize, so the writer takes such a write for a nudge. Anything else in the
/// same write (a key, a mouse report, a paste) makes it the person's input.
fn only_focus_reports(bytes: &[u8]) -> bool {
    !bytes.is_empty() && bytes.chunks(3).all(|r| r == b"\x1b[I" || r == b"\x1b[O")
}

/// Writes input to the pty, a nudge (`only_focus_reports`, judged before any
/// rewriting) stamped into `last_nudge` on both sides of the write as a
/// resize is: the CLI may redraw before a writer descheduled after the write
/// gets to stamp it, and the stamp after covers a write stalled on a busy
/// child.
fn write_input(writer: &mut impl std::io::Write, bytes: &[u8], nudge: bool, last_nudge: &AtomicU64) -> std::io::Result<()> {
    if nudge {
        last_nudge.store(mono_ms(), Ordering::Relaxed);
    }
    writer.write_all(bytes).and_then(|()| writer.flush())?;
    if nudge {
        last_nudge.store(mono_ms(), Ordering::Relaxed);
    }
    Ok(())
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

    /// The repaint a nudge brings on is not the session's output; what comes
    /// after the window is, and so is all of it when no nudge was made.
    #[test]
    fn output_just_after_a_nudge_is_its_repaint() {
        let nudged = 100_000;
        assert!(!is_work_output(nudged, Some(nudged)));
        assert!(!is_work_output(nudged + 400, Some(nudged)));
        assert!(!is_work_output(nudged + REPAINT_WINDOW_MS, Some(nudged)));
        assert!(is_work_output(nudged + REPAINT_WINDOW_MS + 1, Some(nudged)));
        assert!(is_work_output(0, None), "never nudged");
        assert!(is_work_output(nudged, None), "never nudged");
        // The writer stamped a nudge after the reader took its time: that
        // read is the nudge's too.
        assert!(!is_work_output(nudged - 3, Some(nudged)));
    }

    /// The reader stores wall time for the session's own output and nothing
    /// for a nudge's repaint, judged on the monotonic clock alone; before
    /// the first nudge, every read is the session's own.
    #[test]
    fn the_reader_stamps_wall_time_for_work_alone() {
        let wall = 1_700_000_000_000;
        assert_eq!(reader_stamp(wall, 5_000, 5_000), None, "read as the nudge is stamped");
        assert_eq!(reader_stamp(wall, 5_400, 5_000), None);
        assert_eq!(reader_stamp(wall, 5_000 + REPAINT_WINDOW_MS, 5_000), None, "the window's last ms");
        assert_eq!(reader_stamp(wall, 5_000 + REPAINT_WINDOW_MS + 1, 5_000), Some(wall));
        assert_eq!(reader_stamp(wall, 4_997, 5_000), None, "stamped after the read");
        assert_eq!(reader_stamp(wall, 0, NEVER_NUDGED), Some(wall), "never nudged");
        assert_eq!(reader_stamp(wall, 5_000, NEVER_NUDGED), Some(wall), "never nudged");
    }

    /// The reader thread's wrapper reads the monotonic clock itself, so only
    /// what no pause of the test thread can change is asserted here: a
    /// session never nudged counts at once, with the wall time it was given,
    /// and a stamp the clock has not reached yet (the writer stamping while
    /// the reader reads) hides the read. The window's edges are
    /// `reader_stamp`'s, tested above on explicit clocks.
    #[test]
    fn the_reader_judges_the_window_on_the_monotonic_clock() {
        let wall = now_ms();
        assert_eq!(reader_stamp_now(wall, NEVER_NUDGED), Some(wall));
        assert_eq!(reader_stamp_now(wall, mono_ms() + 3_600_000), None);
    }

    /// Only a write of focus reports and nothing else is a nudge: one or
    /// several, in or out. A key, a mouse report or a paste beside one, a
    /// sequence cut short, or nothing at all is not.
    #[test]
    fn a_write_of_focus_reports_alone_is_a_nudge() {
        assert!(only_focus_reports(b"\x1b[I"));
        assert!(only_focus_reports(b"\x1b[O"));
        assert!(only_focus_reports(b"\x1b[O\x1b[I"));
        assert!(only_focus_reports(b"\x1b[I\x1b[O\x1b[I\x1b[O"));
        assert!(!only_focus_reports(b""), "empty");
        assert!(!only_focus_reports(b"\x1b[Ia"), "a key after it");
        assert!(!only_focus_reports(b"a\x1b[I"), "a key before it");
        assert!(!only_focus_reports(b"\x1b[I\r"));
        assert!(!only_focus_reports(b"\x1b[I\x1b[<0;10;5M"), "a mouse report");
        assert!(!only_focus_reports(b"\x1b[200~\x1b[I\x1b[201~"), "a paste");
        assert!(!only_focus_reports(b"\x1b[I\x1b["), "cut short");
        assert!(!only_focus_reports(b"\x1b["));
        assert!(!only_focus_reports(b"\x1b"));
        assert!(!only_focus_reports(b"\x1b[A"), "an arrow");
        assert!(!only_focus_reports(b"\x1bOI"), "SS3, not CSI");
        assert!(!only_focus_reports(b"\x1b[1I"), "a parameter");
        assert!(!only_focus_reports(b"I\x1b[O"));
    }

    /// A pty end that notes the nudge stamp as each write reaches it.
    struct SeesStamp<'a> {
        stamp: &'a AtomicU64,
        seen: Vec<u64>,
        written: Vec<u8>,
    }

    impl std::io::Write for SeesStamp<'_> {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.seen.push(self.stamp.load(Ordering::Relaxed));
            self.written.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// A nudge is stamped before its bytes reach the pty, so a redraw read
    /// before the writer comes back to stamp it is still the nudge's; other
    /// input leaves the stamp as it was.
    #[test]
    fn a_nudge_is_stamped_before_its_bytes_are_written() {
        let stamp = AtomicU64::new(NEVER_NUDGED);
        let mut pty = SeesStamp { stamp: &stamp, seen: Vec::new(), written: Vec::new() };
        write_input(&mut pty, b"a", false, &stamp).unwrap();
        assert_eq!(pty.seen, [NEVER_NUDGED]);
        assert_eq!(stamp.load(Ordering::Relaxed), NEVER_NUDGED, "a key is no nudge");

        let before = mono_ms();
        write_input(&mut pty, b"\x1b[I", true, &stamp).unwrap();
        let at_write = pty.seen[1];
        assert!(at_write != NEVER_NUDGED && at_write >= before, "stamped before the write");
        assert!(stamp.load(Ordering::Relaxed) >= at_write, "and again after it");
        assert_eq!(pty.written, b"a\x1b[I");
    }

    /// The clock the window runs on only moves forward.
    #[test]
    fn the_monotonic_clock_never_steps_back() {
        let a = mono_ms();
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert!(mono_ms() >= a + 5);
    }
}
