//! A chat's CLI in its pty: the process started under the chat's spawn gate,
//! and the threads that write to it, read and coalesce its output, answer
//! its cursor queries and notice when it has gone.

use super::control::end_leftovers;
use super::screen::{as_key_events, cursor_at, Modes};
use super::{manager, now_ms, Input, Session};
use crate::provider::Provider;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::Serialize;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const SCROLLBACK_CAP: usize = 2 * 1024 * 1024;
/// Let the buffer run this far past the cap before trimming, so the trim costs
/// one pass per slack rather than one per read.
const SCROLLBACK_SLACK: usize = 256 * 1024;
/// How long output is allowed to pile up before it is sent to the UI — one
/// frame at 60Hz, short enough to feel immediate and long enough that a
/// repainting TUI does not turn into hundreds of events per second.
const FRAME: std::time::Duration = std::time::Duration::from_millis(16);
/// What a Claude Code or Codex session puts in the environment of everything
/// it runs: its identity, its IPC, its sandbox flags, and the bearer token of
/// another Luna's tools. None of it belongs to a session Luna starts (see
/// spawn); a session given Luna's tools gets a token of this Luna's after.
pub(super) const INHERITED_SESSION_VARS: &[&str] = &[
    "LUNA_MCP_TOKEN",
    "LUNA_SESSION_ID",
    "LUNA_PARENT_ID",
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_PID",
    "CODEX_THREAD_ID",
    "CODEX_SESSION_ID",
    "CODEX_VERSION",
    "CODEX_CI",
    "CODEX_PERMISSION_PROFILE",
    "CODEX_TOOL_CALL_ID",
    "CODEX_APPLY_PATCH_PRESERVE_LINE_ENDINGS",
    "CODEX_NETWORK_PROXY_ACTIVE",
    "CODEX_NETWORK_PROXY_CREDENTIAL_BROKER_ACTIVE",
    "CODEX_SANDBOX",
    "CODEX_SANDBOX_NETWORK_DISABLED",
];
/// What a session that has exited keeps of its scrollback: enough for its
/// last screen.
const EXITED_TAIL: usize = 32 * 1024;
/// A session's pty before any pane has sized it — and for good, when no pane
/// ever does (a chat an agent spawned and nobody opened).
const INITIAL_ROWS: u16 = 30;
const INITIAL_COLS: u16 = 100;
/// The cursor-position query (DSR 6). ConPTY sends one before anything else
/// (portable-pty opens it with INHERIT_CURSOR) and holds the child's output
/// until the terminal answers; a CLI may ask again later. The core answers
/// every one itself and keeps it out of the stream: a chat an agent spawned
/// has no terminal to answer for it, and sat on a blank screen until someone
/// opened its pane; and the query left in the scrollback was answered again
/// by every pane that later attached, typing `[1;1R` into a running CLI.
const DSR: &[u8] = b"\x1b[6n";

// Output and exit events go out through `emit::to_ui`, which holds them
// against the event loop tearing down at quit — see that module.

#[derive(Serialize, Clone)]
struct PtyOutput<'a> {
    id: &'a str,
    data: &'a str,
}

#[derive(Serialize, Clone)]
struct PtyExit<'a> {
    id: &'a str,
    code: Option<u32>,
}

/// Everything a spawn needs that is not the provider's business to work out.
pub(super) struct Launch {
    pub(super) provider: Provider,
    /// Where the process starts: the project folder, `--worktree` and all, or
    /// a worktree the CLI made earlier that a resumed session goes back to.
    pub(super) cwd: String,
    pub(super) account_path: String,
    pub(super) args: Vec<String>,
    pub(super) envs: Vec<(&'static str, String)>,
    pub(super) resume: Option<String>,
    pub(super) describe: String,
    /// What Luna applied in it (applied.rs), kept for the chat once it runs.
    pub(super) applied: Vec<crate::applied::Applied>,
}

impl Launch {
    /// The launch as its process will run (`finish_spec`): the terminal it
    /// draws into, the folder it starts in, what its environment loses.
    pub(super) fn finished(mut self) -> Launch {
        finish_spec(&mut self.envs, &mut self.applied, &self.cwd, true);
        self
    }
}

/// The starts a test has asked to see (`expect`): `spawn` builds their
/// `Launch`, side effects and all, and hands it here instead of running it.
#[cfg(test)]
pub(super) mod capture {
    use super::Launch;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    static TAKEN: Mutex<BTreeMap<String, Option<Launch>>> = Mutex::new(BTreeMap::new());

    pub(in crate::pty) fn expect(id: &str) {
        TAKEN.lock().unwrap_or_else(|e| e.into_inner()).insert(id.to_string(), None);
    }

    /// Whether a test captures `id`'s launch, which then never runs.
    pub(super) fn expected(id: &str) -> bool {
        TAKEN.lock().unwrap_or_else(|e| e.into_inner()).contains_key(id)
    }

    /// The launch back if nobody expects it, else None: kept for `take`.
    pub(super) fn keep(id: &str, launch: Launch) -> Option<Launch> {
        let mut taken = TAKEN.lock().unwrap_or_else(|e| e.into_inner());
        match taken.get_mut(id) {
            Some(slot) => {
                *slot = Some(launch);
                None
            }
            None => Some(launch),
        }
    }

    pub(in crate::pty) fn take(id: &str) -> Option<Launch> {
        TAKEN.lock().unwrap_or_else(|e| e.into_inner()).remove(id).flatten()
    }
}

// Creating a session means a `git fetch` before a worktree plus a process spawn — seconds of
// blocking work that must not run on the main thread, where it would stall
// every other chat's IPC. It used to hold the session map for that whole
// stretch to keep two calls from double-spawning one chat, which put every
// other pane's write_session behind it: the log shows keystrokes taking well
// over a second whenever a chat was starting. The per-chat gate below buys the
// same guarantee without the map.
//
// `launch` runs only once the gate finds no live process, since building one
// has side effects: it mints the chat's MCP token, which voids the last one,
// and rewrites the files the process reads at start. A pane attaching to a
// running chat ensures it too, and doing all that then left the running
// process holding a token Luna no longer knew.
pub(super) async fn spawn(id: String, launch: impl FnOnce() -> Result<Launch, String>) -> Result<String, String> {
    // Before anything of the launch is built: nothing runs as administrator.
    // A test's captured launch is never run (`capture`), and only so may a
    // test, elevated on CI's runners, get past.
    #[cfg(test)]
    let captured = capture::expected(&id);
    #[cfg(not(test))]
    let captured = false;
    if !captured {
        crate::elevation::guard()?;
    }
    let state = manager();
    let gate = state.gate(&id);
    let _spawning = gate.lock().unwrap_or_else(|e| e.into_inner());

    // Under the gate: whoever waited here may find the session already spawned.
    let exited = {
        let mut sessions = state.sessions.lock().unwrap();
        if let Some(s) = sessions.get(&id) {
            if s.alive.load(Ordering::SeqCst) {
                let mut buf = s.scrollback.lock().unwrap();
                return Ok(String::from_utf8_lossy(buf.make_contiguous()).into_owned());
            }
        }
        sessions.remove(&id)
    };
    // A CLI that exited by itself: what it left running goes with the old
    // session, before a new one takes the chat.
    if let Some(old) = exited {
        end_leftovers(&old);
    }
    // Finished here, so a test's capture holds the process as it will run.
    let launch = launch()?.finished();
    // A test reads what a start would run, and nothing runs (`capture`).
    #[cfg(test)]
    let Some(launch) = capture::keep(&id, launch) else {
        return Ok(String::new());
    };
    let Launch { provider, cwd, account_path, args, envs, resume, describe, applied } = launch;

    // portable-pty starts a process whose cwd is not there in the user
    // profile instead, without a word: a chat whose worktree had been removed
    // came back as an empty session in the home folder.
    if !std::path::Path::new(&cwd).is_dir() {
        crate::log::warn("pty", &format!("not starting {id}: {cwd} does not exist"));
        return Err(format!("{cwd} does not exist"));
    }

    // Spawning without one would silently fall back to the CLI's default
    // config dir under the user profile: wrong account, and first-run
    // onboarding in every pane.
    if account_path.is_empty() {
        return Err("no account config dir for this chat".into());
    }

    // Counted from here until the process has exited, however long a delete
    // takes over it: the map lets go of a session before it is gone.
    let on_account = crate::account_use::running_on(&account_path);

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize { rows: INITIAL_ROWS, cols: INITIAL_COLS, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| e.to_string())?;

    // Luna's own copy of the CLI (see cli.rs), or the bare name on PATH until
    // the first download has landed; its version directory held, like the
    // account, until the process has exited.
    let (exe, on_version) = crate::cli::launch(provider, Some(&id));
    let mut cmd = CommandBuilder::new(exe);
    cmd.cwd(&cwd);
    cmd.args(&args);
    // A Luna started from inside a CLI session — a test build run by an
    // agent, say — inherits that session's identity and its tool environment.
    // Claude Code under CLAUDE_CODE_CHILD_SESSION takes itself for a subagent
    // and stops saving transcripts (where `luna_message_list` and the chat titles come
    // from), and the NO_COLOR an agent's shell sets turned every terminal here
    // black and white. The sessions here are nobody's children. NO_COLOR is
    // only dropped when it came with such a session: set by hand, it stays.
    for k in removed_env() {
        cmd.env_remove(k);
    }
    // TERM among them (`Launch::finished`).
    for (k, v) in &envs {
        cmd.env(k, v);
    }

    let child = pair.slave.spawn_command(cmd).map_err(|e| {
        crate::log::error("pty", &format!("spawn failed for {id} in {cwd}: {e}"));
        e.to_string()
    })?;
    drop(pair.slave);
    crate::agents::set_applied(&id, applied, &account_path, &cwd);

    let pid = child.process_id();
    let spawned_at_ms = now_ms() as u128;
    // Into a job before it has had time to spawn anything, so every child it
    // ever starts is listed with it.
    let job = pid.and_then(crate::procs::adopt).map(Arc::new);
    let mut killer = child.clone_killer();
    // Without both ends there is no session to run, and a CLI already started
    // would run on with nothing tracking it: end it before giving up.
    let ends = pair.master.take_writer().and_then(|w| pair.master.try_clone_reader().map(|r| (w, r)));
    let (mut writer, mut reader) = match ends {
        Ok(ends) => ends,
        Err(e) => {
            let _ = killer.kill();
            return Err(e.to_string());
        }
    };
    let child = Arc::new(Mutex::new(child));

    // A respawn is a new process and, for Codex, a new rollout file.
    if provider == Provider::Codex {
        crate::codex::session::release(&id);
    }

    // Drains on its own thread and ends when the session is dropped, which
    // drops the sender with it — and the master end, which the thread owns,
    // goes down with the thread, exactly when the session used to take it.
    let (input_tx, input_rx) = std::sync::mpsc::channel::<Input>();
    // The reader answers cursor queries through a weak handle: a strong one
    // would keep the writer thread, and with it the pty, alive past the
    // session (see the note above).
    let input_tx = Arc::new(input_tx);
    let replies = Arc::downgrade(&input_tx);
    let size = Arc::new(Mutex::new((INITIAL_ROWS, INITIAL_COLS)));
    {
        let id = id.clone();
        let master = pair.master;
        let size_slot = Arc::clone(&size);
        std::thread::spawn(move || {
            // A broken pipe stays broken: say so once, then keep the master
            // alive for the session's own teardown rather than closing the pty
            // out from under a child that may still be on its way out.
            let mut writable = true;
            for msg in input_rx {
                match msg {
                    Input::Bytes(chunk) if writable => {
                        // Key events only where ConPTY reads them (see `as_key_events`).
                        let chunk = if cfg!(windows) && provider == Provider::Codex { as_key_events(&chunk) } else { chunk };
                        if let Err(e) = writer.write_all(&chunk).and_then(|()| writer.flush()) {
                            crate::log::warn("pty", &format!("write to {id} failed: {e}"));
                            writable = false;
                        }
                    }
                    Input::Bytes(_) => {}
                    Input::Resize(size) => {
                        if let Err(e) = master.resize(size) {
                            crate::log::warn("pty", &format!("resize of {id} failed: {e}"));
                        }
                        if let Ok(mut s) = size_slot.lock() {
                            *s = (size.rows, size.cols);
                        }
                    }
                }
            }
        });
    }

    // Sized once for the cap: grown by doubling, a deque settles at twice
    // what it holds, and drain never gives the slack back.
    let scrollback = Arc::new(Mutex::new(VecDeque::with_capacity(SCROLLBACK_CAP + SCROLLBACK_SLACK + 8192)));
    let modes = Arc::new(Mutex::new(Modes::default()));
    let alive = Arc::new(AtomicBool::new(true));
    let last_output_ms = Arc::new(AtomicU64::new(now_ms()));

    {
        let id = id.clone();
        let scrollback = Arc::clone(&scrollback);
        let modes = Arc::clone(&modes);
        let alive = Arc::clone(&alive);
        let last_output_ms = Arc::clone(&last_output_ms);
        let child = Arc::clone(&child);
        let size = Arc::clone(&size);

        // The reader hands bytes to an emitter thread instead of emitting them
        // itself. A repainting TUI produces a steady stream of small reads, and
        // one event per read means one IPC message plus one JSON payload per
        // read, per chat, all landing on the webview's single thread — the app
        // got slower with every busy chat. The emitter coalesces whatever
        // arrives inside a frame into one event.
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let emitter = std::thread::spawn(move || {
            // Carry bytes of a UTF-8 sequence split across reads.
            let mut carry: Vec<u8> = Vec::new();
            while let Ok(first) = rx.recv() {
                carry.extend_from_slice(&first);
                // Keep collecting for one frame, so a burst becomes one event.
                let deadline = std::time::Instant::now() + FRAME;
                loop {
                    let left = deadline.saturating_duration_since(std::time::Instant::now());
                    if left.is_zero() {
                        break;
                    }
                    match rx.recv_timeout(left) {
                        Ok(more) => carry.extend_from_slice(&more),
                        Err(_) => break,
                    }
                }

                // A byte that is not UTF-8 goes out as U+FFFD, the way the
                // scrollback replays it to a pane that attaches: stopping at
                // it held back, and then dropped, everything after it in the
                // frame. Only a sequence cut off at the very end waits for
                // the next read.
                let mut text = String::new();
                let mut rest: &[u8] = &carry;
                while !rest.is_empty() {
                    match std::str::from_utf8(rest) {
                        Ok(s) => {
                            text.push_str(s);
                            rest = &[];
                        }
                        Err(e) => {
                            let (valid, after) = rest.split_at(e.valid_up_to());
                            text.push_str(unsafe { std::str::from_utf8_unchecked(valid) });
                            rest = after;
                            match e.error_len() {
                                Some(n) => {
                                    text.push(char::REPLACEMENT_CHARACTER);
                                    rest = &rest[n..];
                                }
                                None => break,
                            }
                        }
                    }
                }
                let used = carry.len() - rest.len();
                if !text.is_empty() {
                    crate::emit::to_ui("pty://output", PtyOutput { id: &id, data: &text });
                }
                carry.drain(..used);
            }
            id
        });

        std::thread::spawn(move || {
            let mut chunk = [0u8; 8192];
            // The start of a cursor query cut off at the end of a read.
            let mut carry: Vec<u8> = Vec::new();
            'read: loop {
                match reader.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        last_output_ms.store(now_ms(), Ordering::Relaxed);
                        modes.lock().unwrap_or_else(|e| e.into_inner()).feed(&chunk[..n]);
                        let pieces = split_dsr(&mut carry, &chunk[..n]);
                        let last = pieces.len() - 1;
                        for (i, piece) in pieces.into_iter().enumerate() {
                            if !piece.is_empty() {
                                let mut sb = scrollback.lock().unwrap();
                                // Trimming to the cap on every read used to move the
                                // whole two megabytes each time; a deque drops from
                                // the front without touching the rest, and trimming
                                // in one go per overshoot keeps it off the hot path.
                                // Before the append, so it never outgrows the
                                // capacity it was made with.
                                if sb.len() + piece.len() > SCROLLBACK_CAP + SCROLLBACK_SLACK {
                                    let excess = (sb.len() + piece.len() - SCROLLBACK_CAP).min(sb.len());
                                    sb.drain(..excess);
                                }
                                sb.extend(piece.iter().copied());
                                drop(sb);
                                if tx.send(piece).is_err() {
                                    break 'read;
                                }
                            }
                            if i < last {
                                // A query followed this piece: answer with where the
                                // cursor stands after it, as a terminal would.
                                let bytes = scrollback.lock().unwrap().make_contiguous().to_vec();
                                let (rows, cols) = *size.lock().unwrap_or_else(|e| e.into_inner());
                                let (row, col) = cursor_at(&bytes, rows, cols);
                                if let Some(input) = replies.upgrade() {
                                    let _ = input.send(Input::Bytes(format!("\x1b[{};{}R", row + 1, col + 1).into_bytes()));
                                }
                            }
                        }
                    }
                }
            }
            alive.store(false, Ordering::SeqCst);
            // Closing the channel ends the emitter; join it so the last output
            // of the session is on its way before the exit event goes out.
            drop(tx);
            let Ok(id) = emitter.join() else { return };
            let code = child.lock().unwrap_or_else(|e| e.into_inner()).wait().ok().map(|st| st.exit_code());
            drop(on_account);
            drop(on_version);
            crate::log::info("pty", &format!("session {} exited, code {code:?}", crate::agents::label(&id)));
            let pm = manager();
            // The exit names only the chat. A session already spawned under
            // the same id since — a kill and a respawn quicker than this
            // teardown — is the one its pane is listening to, and this exit
            // would mark that running session dead.
            let replaced = pm.sessions.lock().unwrap_or_else(|e| e.into_inner()).get(&id).is_some_and(|s| !Arc::ptr_eq(&s.alive, &alive));
            if !replaced {
                crate::agents::session_exited(&id, code);
                crate::emit::to_ui("pty://exit", PtyExit { id: &id, code });
            }
            // The entry stays, for as long as the chat does: its transcript, its
            // folder and its last screen are still asked for (luna_message_list, the
            // exit notice, deleting it). What only a running session needs
            // goes now — the writer thread and the pty with it, and all but the
            // tail of the scrollback — or a CLI that quit on its own held
            // megabytes and a console host until Luna restarted.
            let mut sessions = pm.sessions.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(s) = sessions.get_mut(&id).filter(|s| Arc::ptr_eq(&s.alive, &alive)) {
                s.input_tx = Arc::new(std::sync::mpsc::channel().0);
                let mut sb = s.scrollback.lock().unwrap_or_else(|e| e.into_inner());
                let excess = sb.len().saturating_sub(EXITED_TAIL);
                sb.drain(..excess);
                sb.shrink_to_fit();
            }
        });
    }

    crate::log::info("pty", &format!("spawned {} ({}) pid {pid:?} {describe} in {cwd}", crate::agents::label(&id), provider.as_str()));

    // A CLI that quits on its own (`/exit`, a crash) ends its process, but
    // on Windows the pty's output stays open for as long as the pseudoconsole
    // does — so the reader never saw an end, the chat stayed "alive" and the
    // exit event never came. Watch the process instead, and when it is gone
    // let go of the input end: the writer thread ends, takes the master and
    // with it the pseudoconsole, and the reader sees its end as it should.
    {
        let id = id.clone();
        let child = Arc::clone(&child);
        let alive = Arc::clone(&alive);
        std::thread::spawn(move || {
            while alive.load(Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_secs(1));
                let gone = child.lock().unwrap_or_else(|e| e.into_inner()).try_wait().map(|s| s.is_some()).unwrap_or(true);
                if !gone {
                    continue;
                }
                let pm = manager();
                let mut sessions = pm.sessions.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(s) = sessions.get_mut(&id).filter(|s| Arc::ptr_eq(&s.alive, &alive)) {
                    s.input_tx = Arc::new(std::sync::mpsc::channel().0);
                }
                break;
            }
        });
    }

    state.sessions.lock().unwrap().insert(
        id,
        Session {
            provider,
            input_tx,
            killer,
            child,
            scrollback,
            modes,
            alive,
            last_output_ms,
            pid,
            cwd,
            spawned_at_ms,
            resume,
            account_path,
            job,
            size,
            typed_ms: Arc::new(AtomicU64::new(0)),
            draft: Arc::new(AtomicU64::new(0)),
            draft_seen: Arc::new(Mutex::new((0, false))),
            held_keys: Arc::new(Mutex::new(None)),
            stashing: Arc::new(AtomicBool::new(false)),
        },
    );

    Ok(String::new())
}

/// The same clean-up for a process started without a pty (background/).
pub(crate) fn clean_env(cmd: &mut Command) {
    for k in removed_env() {
        cmd.env_remove(k);
    }
}

/// The variables a CLI process starts without, though this one has them: the
/// inherited session's (`INHERITED_SESSION_VARS`), and NO_COLOR only when it
/// came with such a session — set by hand, it stays.
pub(crate) fn removed_env() -> Vec<&'static str> {
    let mut out = INHERITED_SESSION_VARS.to_vec();
    if INHERITED_SESSION_VARS.iter().any(|k| std::env::var_os(k).is_some()) {
        out.push("NO_COLOR");
    }
    out
}

/// The last of a CLI process's spec, in the one place both kinds of start
/// finish it: what its environment loses (`removed_env`), the terminal a
/// pane's CLI draws into (`terminal`), and the folder it starts in, each as
/// what Luna applied. A pane's start (`Launch::finished`) and a helper's turn
/// (background/command.rs `command_for`) end here, so what runs is what the
/// window shows.
pub(crate) fn finish_spec(envs: &mut Vec<(&'static str, String)>, applied: &mut Vec<crate::applied::Applied>, cwd: &str, terminal: bool) {
    use crate::applied::Item;
    if terminal {
        envs.push(("TERM", "xterm-256color".to_string()));
        applied.push(Item::Terminal.with("TERM=xterm-256color"));
    }
    applied.push(Item::Cwd.with(format!("cwd {cwd}")));
    applied.push(Item::InheritedCleared.with(format!("unset {}", removed_env().join(", "))));
}

/// Cuts the cursor queries (DSR) out of a read. Returns the output between
/// them, a query following every piece but the last; `carry` keeps the start
/// of a query split across reads for the next one.
fn split_dsr(carry: &mut Vec<u8>, chunk: &[u8]) -> Vec<Vec<u8>> {
    let mut data = std::mem::take(carry);
    data.extend_from_slice(chunk);
    let mut pieces = Vec::new();
    let mut start = 0;
    while let Some(at) = data[start..].windows(DSR.len()).position(|w| w == DSR) {
        pieces.push(data[start..start + at].to_vec());
        start += at + DSR.len();
    }
    let tail = &data[start..];
    let keep = (1..DSR.len()).rev().find(|&n| tail.ends_with(&DSR[..n])).unwrap_or(0);
    pieces.push(tail[..tail.len() - keep].to_vec());
    *carry = tail[tail.len() - keep..].to_vec();
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_is_not_started_as_administrator() {
        crate::elevation::refuse_here(Some(crate::elevation::ADMIN));
        let why = crate::elevation::refusal().map(String::from);
        let got = tauri::async_runtime::block_on(spawn("run-as-admin".into(), || -> Result<Launch, String> {
            panic!("nothing of the launch is built: it mints a token and writes the CLI's files")
        }));
        crate::elevation::refuse_here(None);
        assert_eq!(got.err(), why);
    }

    #[test]
    fn only_a_captured_launch_goes_past_the_check_in_a_test() {
        // A test that captures its launch runs nothing, so it gets past even
        // a refusal; one that does not is refused, as a live test would be.
        capture::expect("captured-as-admin");
        crate::elevation::refuse_here(Some(crate::elevation::ADMIN));
        let ok = || -> Result<Launch, String> { Err("built".into()) };
        let captured = tauri::async_runtime::block_on(spawn("captured-as-admin".into(), ok));
        let live = tauri::async_runtime::block_on(spawn("live-as-admin".into(), ok));
        crate::elevation::refuse_here(None);
        let _ = capture::take("captured-as-admin");
        assert_eq!(captured, Err("built".into()), "the launch was built, to be captured");
        assert!(live.is_err_and(|e| e != "built"), "refused before it was built");
    }

    #[test]
    fn cursor_queries_are_cut_out_even_across_reads() {
        let mut carry = Vec::new();
        // ConPTY's opening query alone.
        assert_eq!(split_dsr(&mut carry, b"\x1b[6n"), vec![b"".to_vec(), b"".to_vec()]);
        // Output, a query, more output.
        assert_eq!(split_dsr(&mut carry, b"ab\x1b[6ncd"), vec![b"ab".to_vec(), b"cd".to_vec()]);
        // A query split over two reads is held back, then answered once.
        assert_eq!(split_dsr(&mut carry, b"x\x1b["), vec![b"x".to_vec()]);
        assert_eq!(carry, b"\x1b[");
        assert_eq!(split_dsr(&mut carry, b"6ny"), vec![b"".to_vec(), b"y".to_vec()]);
        assert!(carry.is_empty());
        // A held-back escape that turns out to be something else goes through.
        assert_eq!(split_dsr(&mut carry, b"\x1b"), vec![b"".to_vec()]);
        assert_eq!(split_dsr(&mut carry, b"[31m"), vec![b"\x1b[31m".to_vec()]);
    }

    #[test]
    fn nothing_of_a_codex_session_reaches_one_luna_starts() {
        for k in ["CODEX_SESSION_ID", "CODEX_VERSION", "CODEX_CI", "CODEX_PERMISSION_PROFILE", "CODEX_TOOL_CALL_ID"] {
            assert!(INHERITED_SESSION_VARS.contains(&k), "{k}");
        }
        assert!(!INHERITED_SESSION_VARS.contains(&"CODEX_HOME"), "a person's to set, and Luna sets its own for Codex");
    }
}
