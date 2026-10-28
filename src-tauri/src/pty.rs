use crate::provider::Provider;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, PtySize};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
// The kill path shells out (taskkill, pkill); a target with neither has no
// use for it and would warn about it on every build.
#[cfg(any(windows, unix))]
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
/// A session's pty before any pane has sized it — and for good, when no pane
/// ever does (a chat an agent spawned and nobody opened).
/// What a Claude Code or Codex session puts in the environment of everything
/// it runs: its identity, its IPC, its sandbox flags. None of it belongs to a
/// session Luna starts (see spawn).
const INHERITED_SESSION_VARS: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_PID",
    "CODEX_THREAD_ID",
    "CODEX_SANDBOX",
    "CODEX_SANDBOX_NETWORK_DISABLED",
];
/// What a session that has exited keeps of its scrollback: enough for its
/// last screen.
const EXITED_TAIL: usize = 32 * 1024;
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
    /// under it (procs.rs). None where Windows refused or off Windows.
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
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Everything a spawn needs that is not the provider's business to work out.
struct Launch {
    provider: Provider,
    /// Where the process starts: the project folder, `--worktree` and all, or
    /// a worktree the CLI made earlier that a resumed session goes back to.
    cwd: String,
    account_path: String,
    args: Vec<String>,
    envs: Vec<(&'static str, String)>,
    resume: Option<String>,
    describe: String,
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
async fn spawn(id: String, launch: impl FnOnce() -> Launch) -> Result<String, String> {
    let state = manager();
    let gate = {
        let mut gates = state.spawning.lock().unwrap();
        // Nobody holds a gate whose only reference is the map's own, so this
        // keeps the table to the chats actually starting right now.
        gates.retain(|_, g| Arc::strong_count(g) > 1);
        Arc::clone(gates.entry(id.clone()).or_default())
    };
    let _spawning = gate.lock().unwrap();

    // Under the gate: whoever waited here may find the session already spawned.
    {
        let mut sessions = state.sessions.lock().unwrap();
        if let Some(s) = sessions.get(&id) {
            if s.alive.load(Ordering::SeqCst) {
                let mut buf = s.scrollback.lock().unwrap();
                return Ok(String::from_utf8_lossy(buf.make_contiguous()).into_owned());
            }
            sessions.remove(&id);
        }
    }
    let Launch { provider, cwd, account_path, args, envs, resume, describe } = launch();

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

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: INITIAL_ROWS,
            cols: INITIAL_COLS,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| e.to_string())?;

    // Luna's own copy of the CLI (see cli.rs), or the bare name on PATH until
    // the first download has landed.
    let mut cmd = CommandBuilder::new(crate::cli::binary(provider));
    cmd.cwd(&cwd);
    cmd.args(&args);
    // A Luna started from inside a CLI session — a test build run by an
    // agent, say — inherits that session's identity and its tool environment.
    // Claude Code under CLAUDE_CODE_CHILD_SESSION takes itself for a subagent
    // and stops saving transcripts (where `luna_read` and the chat titles come
    // from), and the NO_COLOR an agent's shell sets turned every terminal here
    // black and white. The sessions here are nobody's children. NO_COLOR is
    // only dropped when it came with such a session: set by hand, it stays.
    for k in INHERITED_SESSION_VARS {
        cmd.env_remove(k);
    }
    if INHERITED_SESSION_VARS.iter().any(|k| std::env::var_os(k).is_some()) {
        cmd.env_remove("NO_COLOR");
    }
    for (k, v) in &envs {
        cmd.env(k, v);
    }
    cmd.env("TERM", "xterm-256color");

    let child = pair.slave.spawn_command(cmd).map_err(|e| {
        crate::log::error("pty", &format!("spawn failed for {id} in {cwd}: {e}"));
        e.to_string()
    })?;
    drop(pair.slave);

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

                let valid_to = match std::str::from_utf8(&carry) {
                    Ok(_) => carry.len(),
                    Err(e) => e.valid_up_to(),
                };
                if valid_to > 0 {
                    let text = unsafe { std::str::from_utf8_unchecked(&carry[..valid_to]) };
                    crate::emit::to_ui("pty://output", PtyOutput { id: &id, data: text });
                }
                carry.drain(..valid_to);
                if carry.len() > 4 {
                    carry.clear(); // not a split sequence, just invalid bytes
                }
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
            let code = child
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .wait()
                .ok()
                .map(|st| st.exit_code());
            crate::log::info("pty", &format!("session {id} exited, code {code:?}"));
            crate::emit::to_ui("pty://exit", PtyExit { id: &id, code });
            // The entry stays, for as long as the chat does: its transcript, its
            // folder and its last screen are still asked for (luna_read, the
            // exit notice, deleting it). What only a running session needs
            // goes now — the writer thread and the pty with it, and all but the
            // tail of the scrollback — or a CLI that quit on its own held
            // megabytes and a console host until Luna restarted.
            let pm = manager();
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

    crate::log::info(
        "pty",
        &format!("spawned {id} ({}) pid {pid:?} {describe} in {cwd}", provider.as_str()),
    );

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
        },
    );

    Ok(String::new())
}

/// The hooks a Claude Code session is started with: each event posts its
/// stdin to Luna's loopback listener (hub.rs) and prints nothing — a hook's
/// stdout is added to the model's context, silence is free. Written to a file
/// under the data dir rather than passed inline, so the command line stays
/// readable and quoting is not a concern. None while the listener is off.
fn claude_hooks_file(chat_id: &str) -> Option<std::path::PathBuf> {
    let url = crate::hub::hook_url(chat_id)?;
    // curl.exe by name on Windows: there `curl` in PowerShell is an alias for
    // Invoke-WebRequest. Its stdin — the hook's JSON — goes up as the body;
    // the 204 answer has no body, so nothing comes back to print.
    let curl = if cfg!(windows) { "curl.exe" } else { "curl" };
    let command = format!("{curl} -s -m 3 -X POST --data-binary @- {url}");
    let hook = |timeout: u32| {
        serde_json::json!([{ "hooks": [{ "type": "command", "command": command, "timeout": timeout }] }])
    };
    let settings = serde_json::json!({
        "hooks": {
            "UserPromptSubmit": hook(5),
            "PostToolUse": hook(5),
            "Stop": hook(5),
            "StopFailure": hook(5),
            "Notification": hook(5),
            "SessionEnd": hook(5),
            // What a prompt asks, which neither the screen nor the transcript
            // gives whole: the question and its options (the transcript gets
            // the call only once it is answered), the command up for leave.
            "PreToolUse": [{ "matcher": "AskUserQuestion", "hooks": [{ "type": "command", "command": command, "timeout": 5 }] }],
            "PermissionRequest": hook(5),
        }
    });
    let dir = crate::paths::data_dir().join("hooks");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{chat_id}.json"));
    std::fs::write(&path, serde_json::to_string_pretty(&settings).ok()?).ok()?;
    Some(path)
}

/// The same clean-up for a process started without a pty (background.rs).
pub(crate) fn clean_env(cmd: &mut std::process::Command) {
    for k in INHERITED_SESSION_VARS {
        cmd.env_remove(k);
    }
    if INHERITED_SESSION_VARS.iter().any(|k| std::env::var_os(k).is_some()) {
        cmd.env_remove("NO_COLOR");
    }
}

/// Luna's name for Codex's `--approve-for-me`: what Codex would ask goes to a
/// reviewer model. It takes no `--ask-for-approval` or `--sandbox` and runs
/// in `workspace-write`.
pub(crate) const AUTO_REVIEW: &str = "auto-review";
pub(crate) const AUTO_REVIEW_SANDBOX: &str = "workspace-write";
/// Codex runs the project's hooks without asking for trust (codex/hooks.rs).
pub(crate) const HOOKS_UNASKED: &str = "--dangerously-bypass-hook-trust";

/// The override that gives Codex its Windows sandbox, for the modes that
/// need one. Without it Codex refuses every command under `read-only` and
/// `workspace-write`, a `Get-Content` included. The unelevated kind runs each
/// command with a restricted token: no administrator, no extra users; under
/// `workspace-write` Codex grants its sandbox write access to the folder
/// once, in the folder's ACL.
pub(crate) fn codex_windows_sandbox(sandbox: &str) -> Option<String> {
    (cfg!(windows) && matches!(sandbox, "read-only" | "workspace-write")).then(|| "windows.sandbox=\"unelevated\"".to_string())
}

/// PATH for a Codex session in that sandbox: without `…\Microsoft\WindowsApps`.
/// Codex takes the first `pwsh.exe` on PATH for its shell, and the Store
/// PowerShell's there is an app alias the restricted token cannot start —
/// `CreateProcessAsUserW failed: 5` on every command, whatever it was. Without
/// it Codex falls back to a PowerShell installed the ordinary way, or to
/// Windows PowerShell; Store apps' aliases are out of the sandbox's reach
/// either way.
pub(crate) fn codex_sandbox_path(sandbox: &str) -> Option<String> {
    codex_windows_sandbox(sandbox)?;
    let path = std::env::var("PATH").ok()?;
    Some(without_windows_apps(&path))
}

fn without_windows_apps(path: &str) -> String {
    path.split(';').filter(|p| !p.to_lowercase().replace('/', "\\").contains("\\microsoft\\windowsapps")).collect::<Vec<_>>().join(";")
}

/// Luna's MCP server as `-c` overrides for Codex, the token through
/// `LUNA_MCP_TOKEN`: neither touches the account's config.toml.
pub(crate) fn codex_mcp_args() -> Vec<String> {
    let Some(url) = crate::hub::mcp_url() else { return Vec::new() };
    let mut args = Vec::new();
    for value in [
        format!("mcp_servers.luna.url=\"{url}\""),
        "mcp_servers.luna.bearer_token_env_var=\"LUNA_MCP_TOKEN\"".into(),
        // Codex gives an MCP call 60 s by default.
        format!("mcp_servers.luna.tool_timeout_sec={}", MCP_TOOL_TIMEOUT_MS / 1000),
        // And outside the full bypass it may stop to ask before a server's
        // tool runs; `approve` lets Luna's through, as `--allowedTools` does
        // for Claude Code.
        "mcp_servers.luna.default_tools_approval_mode=\"approve\"".into(),
    ] {
        args.push("-c".into());
        args.push(value);
    }
    args
}

/// How long a Luna tool call may take, in ms, as both CLIs are told: `wait`
/// holds its request for up to 30 min, `spawn` for up to two.
const MCP_TOOL_TIMEOUT_MS: u64 = 1_900_000;

/// The MCP config a tooled Claude Code session starts with: Luna's server on
/// the hub listener, the session's own bearer token in the header. A file
/// per chat under the data dir, like the hooks.
pub(crate) fn claude_mcp_file(chat_id: &str, token: &str) -> Option<std::path::PathBuf> {
    let url = crate::hub::mcp_url()?;
    let config = serde_json::json!({
        "mcpServers": {
            "luna": {
                "type": "http",
                "url": url,
                "headers": { "Authorization": format!("Bearer {token}") },
                // Claude Code drops an HTTP tool call that has sent nothing
                // for 5 min, and the server's `timeout` is what raises that
                // (as well as the hard limit per call). A `wait` answers
                // with one plain JSON body at the end, nothing before it.
                "timeout": MCP_TOOL_TIMEOUT_MS,
                // Not behind tool search: a model that has spawned a helper
                // found the tools to talk to it deferred, and had to look each
                // one up first. All eight are what the box was ticked for.
                "alwaysLoad": true
            }
        }
    });
    let dir = crate::paths::data_dir().join("mcp");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{chat_id}.json"));
    std::fs::write(&path, serde_json::to_string_pretty(&config).ok()?).ok()?;
    Some(path)
}

/// Removes the hook and MCP files of sessions that are not running. Each is
/// written afresh when its session starts or resumes, so one belonging to a
/// chat that is gone — deleted some way other than through Luna, lost to a
/// crash — is only clutter, and one belonging to a chat that is not running
/// comes back when it does. Ten minutes' grace, so a file is never pulled
/// from under a session that is being spawned right now.
fn prune_session_files(live: &std::collections::HashSet<String>) -> usize {
    let mut removed = 0;
    for sub in ["hooks", "mcp"] {
        let Ok(entries) = std::fs::read_dir(crate::paths::data_dir().join(sub)) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(id) = path.file_stem().and_then(|s| s.to_str()) else { continue };
            if path.extension().and_then(|e| e.to_str()) != Some("json") || live.contains(id) {
                continue;
            }
            let fresh = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_none_or(|age| age < std::time::Duration::from_secs(600));
            if !fresh && std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
    }
    // Long messages handed over as files (agents.rs `spill`): read at once,
    // kept a week for whoever wants to see what was sent.
    if let Ok(entries) = std::fs::read_dir(crate::agents::notes_dir()) {
        for entry in entries.flatten() {
            let old = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > std::time::Duration::from_secs(7 * 24 * 3600));
            if old && std::fs::remove_file(entry.path()).is_ok() {
                removed += 1;
            }
        }
    }
    removed
}

/// At startup and every few hours after, like the media sweep: the app can
/// run for weeks without a restart.
pub fn prune_session_files_periodically() {
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(15 * 60));
        let live: std::collections::HashSet<String> = {
            let state = manager();
            let sessions = state.sessions.lock().unwrap_or_else(|e| e.into_inner());
            sessions.keys().cloned().collect()
        };
        let removed = prune_session_files(&live);
        if removed > 0 {
            crate::log::info("pty", &format!("removed {removed} hook, mcp and note file(s) no longer needed"));
        }
        std::thread::sleep(std::time::Duration::from_secs(6 * 3600 - 15 * 60));
    });
}

/// What the activity sampler needs of a live session (activity.rs).
pub struct ActivityProbe {
    pub id: String,
    pub provider: Provider,
    pub pid: Option<u32>,
    pub cwd: String,
    pub spawned_at_ms: u128,
    pub resume: Option<String>,
    pub account_path: String,
    pub last_output_ms: u64,
    pub job: Option<Arc<crate::procs::Job>>,
    /// Codex only: its screen shows an approval dialog (see
    /// `codex::session::asks`). Filled in by the activity sampler, which
    /// renders the screen outside the session map.
    pub asking: bool,
}

impl ActivityProbe {
    /// Where the CLI works now, when it has moved from where it started: both
    /// make their `--worktree` checkout after the spawn, and only what they
    /// leave on disk says where — Claude Code's registry, Codex's rollout.
    pub fn working_dir(&self) -> Option<String> {
        match self.provider {
            Provider::Claude => crate::claude::session::session_cwd(self.pid, &self.cwd, self.spawned_at_ms, &self.account_path),
            Provider::Codex => crate::codex::session::rollout_cwd(&crate::codex::session::Live {
                chat_id: &self.id,
                cwd: &self.cwd,
                spawned_at_ms: self.spawned_at_ms,
                resume: self.resume.as_deref(),
                account_path: &self.account_path,
                last_output_ms: self.last_output_ms,
            }),
        }
    }
}

fn activity_probe(id: &str, s: &Session) -> ActivityProbe {
    ActivityProbe {
        id: id.to_string(),
        provider: s.provider,
        pid: s.pid,
        cwd: s.cwd.clone(),
        spawned_at_ms: s.spawned_at_ms,
        resume: s.resume.clone(),
        account_path: s.account_path.clone(),
        last_output_ms: s.last_output_ms.load(Ordering::Relaxed),
        job: s.job.clone(),
        asking: false,
    }
}

impl PtyManager {
    /// Every live session, copied out from under the map.
    pub fn activity_probes(&self) -> Vec<ActivityProbe> {
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions
            .iter()
            .filter(|(_, s)| s.alive.load(Ordering::SeqCst))
            .map(|(id, s)| activity_probe(id, s))
            .collect()
    }

    /// One live session, or None.
    pub fn probe(&self, id: &str) -> Option<ActivityProbe> {
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions.get(id).filter(|s| s.alive.load(Ordering::SeqCst)).map(|s| activity_probe(id, s))
    }

    /// How long ago a person last typed into the session, None if never.
    pub fn typed_ago_ms(&self, id: &str) -> Option<u64> {
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        let at = sessions.get(id)?.typed_ms.load(Ordering::Relaxed);
        (at > 0).then(|| now_ms().saturating_sub(at))
    }

    /// Whether a person has left something typed and not sent in the
    /// session's composer.
    pub fn has_draft(&self, id: &str) -> bool {
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions.get(id).is_some_and(|s| s.draft.load(Ordering::Relaxed) & !DRAFT_BACKSLASH > 0)
    }

    /// Whether a running session uses this account folder.
    pub fn account_in_use(&self, account_path: &str) -> bool {
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions.values().any(|s| s.alive.load(Ordering::SeqCst) && same_dir(&s.account_path, account_path))
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

    /// What the session's terminal shows right now: the scrollback replayed
    /// through a terminal emulator at the pty's size, as plain text, rows
    /// joined by newlines. This is how an agent sees what its helper is
    /// stuck on — a first-run notice, a picker, an error the transcript will
    /// never carry — and what a person would see in the pane.
    pub fn screen(&self, id: &str) -> Option<String> {
        let (scrollback, size) = {
            let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
            let s = sessions.get(id)?;
            (Arc::clone(&s.scrollback), Arc::clone(&s.size))
        };
        let bytes: Vec<u8> = {
            let mut sb = scrollback.lock().unwrap_or_else(|e| e.into_inner());
            sb.make_contiguous().to_vec()
        };
        let (rows, cols) = *size.lock().unwrap_or_else(|e| e.into_inner());
        Some(render_screen(&bytes, rows, cols))
    }

    /// Queues bytes for the session's writer thread.
    pub fn write(&self, id: &str, data: &[u8]) -> Result<(), String> {
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        let s = sessions.get(id).ok_or("no such session")?;
        s.input_tx.send(Input::Bytes(data.to_vec())).map_err(|_| "session writer is gone".to_string())
    }

    /// Types text the way a terminal pastes it: between bracketed-paste
    /// markers once the CLI has asked for them. Thrown in bare, a few
    /// thousand characters reached Claude Code as only their last couple of
    /// hundred — a notice lost its head, the tag that says what it is with
    /// it. Codex on Windows takes its text as key events (see `as_key_events`),
    /// which carry every character already and would take the markers for
    /// typing.
    pub fn paste(&self, id: &str, text: &str) -> Result<(), String> {
        let bracketed = {
            let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
            let s = sessions.get(id).ok_or("no such session")?;
            let keys = cfg!(windows) && s.provider == Provider::Codex;
            !keys && s.modes.lock().unwrap_or_else(|e| e.into_inner()).on.get(&2004) == Some(&true)
        };
        if bracketed {
            self.write(id, format!("\x1b[200~{text}\x1b[201~").as_bytes())
        } else {
            self.write(id, text.as_bytes())
        }
    }

    /// Asks the session to quit and kills it if it does not.
    pub async fn kill(&self, id: &str) {
        take_and_shut_down(self, id).await;
    }

    /// Whether a running session other than `id` is in this worktree: one
    /// started in it, or one that moved into it itself.
    fn worktree_in_use_by_others(&self, id: &str, wt: &str) -> bool {
        let norm = |p: &str| p.replace('/', "\\").trim_end_matches('\\').to_lowercase();
        let wt = norm(wt);
        let inside = |p: &str| {
            let p = norm(p);
            p == wt || p.starts_with(&format!("{wt}\\"))
        };
        // Probed off the session map: the disk reads below must not hold it.
        self.activity_probes()
            .iter()
            .filter(|p| p.id != id)
            .any(|p| inside(&p.cwd) || p.working_dir().is_some_and(|real| inside(&real)))
    }

    /// Tears a chat's session down: resolve its worktree, quit the process,
    /// drop attachments and the hooks file, and the worktree only if asked.
    /// Returns the worktree path either way.
    pub async fn delete(
        &self,
        id: &str,
        folder: &str,
        worktree_path: Option<String>,
        drop_worktree: bool,
    ) -> Result<Option<String>, String> {
        // Resolve before killing: once the process is gone its registry entry goes too.
        let resolved = worktree_path.filter(|p| !p.is_empty()).or_else(|| {
            let probe = self.probe(id)?;
            let scwd = probe.working_dir().unwrap_or(probe.cwd);
            crate::worktree::is_worktree_of(folder, &scwd).then_some(scwd)
        });
        // A helper spawned from a chat in a worktree runs in that same
        // worktree. Dropping "its" worktree would take the other chat's work
        // with it, so a worktree another running session is in stays.
        let shared = resolved.as_deref().is_some_and(|wt| self.worktree_in_use_by_others(id, wt));

        take_and_shut_down(self, id).await;
        // Everything else that kept count of it, so nothing keeps the chat on
        // after it is gone: the agent registry (luna_sessions, the helper
        // limit, its token), the turn counters, the Codex rollout it held.
        crate::agents::forget(id);
        crate::activity::forget(id);
        crate::codex::session::release(id);

        let _ = crate::media::clear_media(id.to_string());
        for sub in ["hooks", "mcp"] {
            let _ = std::fs::remove_file(crate::paths::data_dir().join(sub).join(format!("{id}.json")));
        }
        crate::log::info("delete", &format!("chat {id}, worktree {resolved:?}, dropping {drop_worktree}"));

        match &resolved {
            Some(wt) if drop_worktree && shared => {
                crate::log::warn("delete", &format!("kept worktree {wt}: another running session is in it"));
                Ok(None)
            }
            Some(wt) if drop_worktree => {
                crate::worktree::remove_worktree_now(folder.to_string(), wt.clone()).map(|_| resolved.clone())
            }
            Some(_) => Ok(resolved),
            None => Ok(None),
        }
    }

    /// Asks every live session to quit, each with `grace` to do it on its
    /// own, and returns how many there were. Blocks; for the shutdown path.
    pub fn shut_down_all(&self, grace: std::time::Duration) -> usize {
        let taken: Vec<Session> = {
            let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
            let ids: Vec<String> = sessions.iter().filter(|(_, s)| s.alive.load(Ordering::SeqCst)).map(|(id, _)| id.clone()).collect();
            ids.into_iter().filter_map(|id| sessions.remove(&id)).collect()
        };
        let n = taken.len();
        let handles: Vec<_> = taken.into_iter().map(|s| std::thread::spawn(move || shut_down(s, grace))).collect();
        for h in handles {
            let _ = h.join();
        }
        n
    }
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn ensure_claude_session(
    id: String,
    folder: String,
    account_path: String,
    model: String,
    effort: String,
    permission_mode: String,
    worktree: bool,
    // With `worktree`: an existing one to go into by name, which Claude Code
    // makes again, same name and branch, if it has been removed meanwhile.
    worktree_name: Option<String>,
    resume: Option<String>,
    // Luna's MCP tools (agents.rs): off unless the chat asked.
    tools: Option<bool>,
    // The chat that spawned this one, if an agent did.
    parent: Option<String>,
    // An opening prompt, for a chat an agent starts.
    prompt: Option<String>,
) -> Result<String, String> {
    // Built only if no process is running (see `spawn`).
    spawn(id.clone(), move || {
        // A new worktree is about to branch off origin; a resumed session
        // already has its worktree. Here, under the spawn gate, so a pane
        // asking for the session meanwhile waits for this spawn — the one
        // with the opening prompt — instead of starting its own without it.
        if worktree && resume.is_none() {
            crate::worktree::fetch_origin(&folder);
        }
        let mut args = vec![
            "--model".to_string(),
            model.clone(),
            "--effort".to_string(),
            effort,
            "--permission-mode".to_string(),
            permission_mode.clone(),
        ];
        if let Some(session_id) = &resume {
            args.push("--resume".into());
            args.push(session_id.clone());
        }
        if worktree {
            args.push("--worktree".into());
            // The characters Claude Code takes in a name, and a guard against
            // one being read as the flag after it.
            if let Some(name) = worktree_name.as_deref().filter(|n| {
                !n.is_empty() && !n.starts_with('-') && n.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
            }) {
                args.push(name.to_string());
            }
        }
        // Rewritten at every spawn: the listener's port is new with every Luna run.
        if let Some(hooks) = claude_hooks_file(&id) {
            args.push("--settings".into());
            args.push(hooks.to_string_lossy().into_owned());
        }
        let tools = tools.unwrap_or(false);
        let settings = serde_json::json!({
            "model": model, "effort": args[3], "permissionMode": permission_mode, "worktree": worktree,
        });
        // A login session runs in the account folder itself (the providers'
        // `loginSession`): not a chat, so not one agents may see or write to.
        let token = if same_dir(&folder, &account_path) {
            None
        } else {
            crate::agents::register(&id, Provider::Claude, &folder, &account_path, parent.as_deref(), tools, settings)
        };
        if let Some(mcp) = token.as_deref().and_then(|t| claude_mcp_file(&id, t)) {
            // Luna's tools without a permission prompt in every mode: the
            // user ticked the box that gives them. Variadic like
            // `--mcp-config`, so it goes where a flag follows it.
            args.push("--allowedTools".into());
            args.push("mcp__luna".into());
            args.push("--mcp-config".into());
            args.push(mcp.to_string_lossy().into_owned());
        }
        // A long prompt or message from its agent comes as a file there
        // (agents.rs `spill`), to read without asking.
        if parent.is_some() {
            let notes = crate::agents::notes_dir();
            if std::fs::create_dir_all(&notes).is_ok() {
                args.push("--add-dir".into());
                args.push(notes.to_string_lossy().into_owned());
            }
        }
        if let Some(p) = prompt.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
            // `--mcp-config` takes any number of files, and took the prompt
            // after it for one more: "MCP config file not found: <prompt>".
            args.push("--".into());
            args.push(p.to_string());
        }
        let envs = vec![
            ("CLAUDE_CONFIG_DIR", account_path.clone()),
            // Luna updates the CLI itself; the CLI's own updater would install a
            // second copy under the user profile that nothing here ever runs.
            ("DISABLE_AUTOUPDATER", "1".to_string()),
        ];
        Launch {
            provider: Provider::Claude,
            cwd: folder,
            account_path,
            args,
            envs,
            resume,
            describe: format!("model {model} perm {permission_mode} worktree {worktree}"),
        }
    })
    .await
}

/// A Codex session. `folder` is where it runs: the project, which `worktree`
/// has Codex leave for a checkout of its own (`--worktree`), or a worktree it
/// made earlier, to resume in. `login` runs `codex login` instead of a chat.
/// `hooks_unasked` runs the project's hooks without their trust prompt
/// (codex/hooks.rs).
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn ensure_codex_session(
    id: String,
    folder: String,
    account_path: String,
    model: Option<String>,
    effort: String,
    approval: String,
    sandbox: String,
    worktree: Option<bool>,
    resume: Option<String>,
    login: bool,
    tools: Option<bool>,
    hooks_unasked: Option<bool>,
    parent: Option<String>,
    prompt: Option<String>,
) -> Result<String, String> {
    // Built only if no process is running (see `spawn`).
    spawn(id.clone(), move || {
        let mut args: Vec<String> = vec![];
        let mut envs = vec![("CODEX_HOME", account_path.clone())];
        let describe;
        if login {
            args.push("login".into());
            describe = "login".to_string();
        } else {
            if let Some(session_id) = &resume {
                args.push("resume".into());
                args.push(session_id.clone());
            }
            args.push("-C".into());
            args.push(folder.clone());
            // The session runs in this process, in Luna's pty, never in Codex's
            // shared background server: Luna stops, deletes and counts it as a
            // process of its own. The `-c` overrides below need that anyway;
            // saying so keeps Codex from warning about it on every start.
            args.push("--no-daemon".into());
            // A resumed thread already has its worktree; Codex refuses the
            // flag on `resume`.
            if worktree.unwrap_or(false) && resume.is_none() {
                args.push("--worktree".into());
            }
            if let Some(m) = model.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
                args.push("--model".into());
                args.push(m.to_string());
            }
            args.push("-c".into());
            args.push(format!("model_reasoning_effort=\"{effort}\""));
            let sandbox: &str = if approval == AUTO_REVIEW { AUTO_REVIEW_SANDBOX } else { &sandbox };
            // The pair Codex itself spells as one flag.
            if approval == "never" && sandbox == "danger-full-access" {
                args.push("--dangerously-bypass-approvals-and-sandbox".into());
            } else {
                if approval == AUTO_REVIEW {
                    args.push("--approve-for-me".into());
                } else {
                    args.push("--ask-for-approval".into());
                    args.push(approval.clone());
                    args.push("--sandbox".into());
                    args.push(sandbox.to_string());
                }
                if let Some(sb) = codex_windows_sandbox(sandbox) {
                    args.push("-c".into());
                    args.push(sb);
                }
                if let Some(path) = codex_sandbox_path(sandbox) {
                    envs.push(("PATH", path));
                }
            }
            let hooks_unasked = hooks_unasked.unwrap_or(false);
            if hooks_unasked {
                args.push(HOOKS_UNASKED.into());
            }
            let tools = tools.unwrap_or(false);
            let settings = serde_json::json!({
                "model": model.as_deref().map(str::trim).filter(|m| !m.is_empty()),
                "effort": effort, "approval": approval, "sandbox": sandbox, "hooksUnasked": hooks_unasked,
            });
            let token = crate::agents::register(&id, Provider::Codex, &folder, &account_path, parent.as_deref(), tools, settings);
            if let Some(token) = token.filter(|_| crate::hub::mcp_url().is_some()) {
                args.extend(codex_mcp_args());
                envs.push(("LUNA_MCP_TOKEN", token));
            }
            if let Some(p) = prompt.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
                // A prompt that starts with a dash is still a prompt.
                args.push("--".into());
                args.push(p.to_string());
            }
            describe = format!(
                "model {} effort {effort} approval {approval} sandbox {sandbox}",
                model.as_deref().unwrap_or("default")
            );
        }
        Launch {
            provider: Provider::Codex,
            cwd: folder,
            account_path,
            args,
            envs,
            resume: if login { None } else { resume },
            describe,
        }
    })
    .await
}

#[tauri::command]
pub fn write_session(id: String, data: String) -> Result<(), String> {
    let state = manager();
    let sessions = state.sessions.lock().unwrap();
    let s = sessions.get(&id).ok_or("no such session")?;
    // Keys, not what the terminal reports by itself (focus, mouse), which
    // arrives as escape sequences on the same path.
    if !data.starts_with('\x1b') {
        s.typed_ms.store(now_ms(), Ordering::Relaxed);
    }
    let draft = draft_after(s.draft.load(Ordering::Relaxed), &data);
    s.draft.store(draft, Ordering::Relaxed);
    // Queueing, so the command returns at once however busy the pty is.
    s.input_tx
        .send(Input::Bytes(data.into_bytes()))
        .map_err(|_| "session writer is gone".to_string())
}

/// Queued behind the keystrokes that preceded it and applied on the writer
/// thread. A failure there is logged rather than returned: by the time it
/// happens the caller has moved on, and it never looked at the result anyway.
#[tauri::command]
pub fn resize_session(id: String, cols: u16, rows: u16) -> Result<(), String> {
    let state = manager();
    let sessions = state.sessions.lock().unwrap();
    let s = sessions.get(&id).ok_or("no such session")?;
    s.input_tx
        .send(Input::Resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        }))
        .map_err(|_| "session writer is gone".to_string())
}

// The pty child is the CLI; everything it spawned (node, MCP servers, shells)
// is a grandchild and survives a plain kill. Sweep the tree first, then kill
// the child directly as a backstop.
pub(crate) fn kill_tree(pid: Option<u32>) {
    let Some(pid) = pid else { return };
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .output();
    }
    // The pty made the CLI a session and process-group leader, so its
    // children are in the group whose id is its pid unless they left it;
    // those that did are still its children by parent pid.
    #[cfg(unix)]
    for by in ["-g", "-P"] {
        let _ = Command::new("pkill").args(["-KILL", by, &pid.to_string()]).output();
    }
    #[cfg(not(any(windows, unix)))]
    let _ = pid;
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

/// How long a session gets to leave on its own before it is taken down.
/// Neither CLI is out within two seconds of its second Ctrl+C: Claude Code
/// runs its exit hooks first — among them Luna's own SessionEnd hook, a curl
/// with a 3 s timeout — and Codex shuts its MCP servers down. At 1.5 s every
/// deleted chat ended in the hard kill this is meant to avoid. The same for
/// every session at once: an update, Quit and shut-down-when-done.
pub const GRACE: std::time::Duration = std::time::Duration::from_secs(10);
/// Longer than any CLI waits to see whether an Esc starts an escape sequence.
const ESC_GAP: std::time::Duration = std::time::Duration::from_millis(300);
/// Ctrl+C is one keystroke to the CLI and two in a row mean quit; one write
/// carrying both lands in a single input tick, where the second cannot see
/// the state the first set. Space them the way a hand would.
const CTRL_C_GAP: std::time::Duration = std::time::Duration::from_millis(150);
/// How long the CLI gets after the Ctrl+Cs to be on its way out before an
/// exit dialog is assumed to be holding it.
const EXIT_DIALOG_WAIT: std::time::Duration = std::time::Duration::from_millis(1500);

/// Asks the CLI to quit, and kills it only if it does not.
///
/// `taskkill /F` ends the process without its exit hooks, and the CLI keeps
/// state that only those hooks clean up. A fullscreen launch records itself as
/// pending until it has been healthy for ten seconds; a launch that dies with
/// the record still there costs the next launch on that account its fullscreen
/// renderer ("didn't finish starting last time"), and two of them turn the
/// renderer off. Deleting a chat you had just made, or closing the login
/// window, did exactly that. So first the keystrokes a user would type —
/// Ctrl+C, and again when it asks — and the hard kill only for a process that
/// is past listening. Codex reads Ctrl+C the same way: interrupt, then quit.
///
/// Blocks for up to `grace`: callers run it off the main thread.
fn shut_down(mut s: Session, grace: std::time::Duration) {
    let exited = || {
        s.child
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .try_wait()
            .map(|status| status.is_some())
            .unwrap_or(true)
    };
    let deadline = std::time::Instant::now() + grace;
    // A modal question — Claude Code's "wait for the limit to reset?", a
    // picker — ignores Ctrl+C and answers Esc, and sat out the whole grace
    // into the hard kill. Esc first, then a pause long enough that the CLI
    // does not read it and the Ctrl+C after as one Alt+Ctrl+C.
    if s.input_tx.send(Input::Bytes(vec![0x1b])).is_ok() {
        std::thread::sleep(ESC_GAP);
    }
    // A turn in flight takes one Ctrl+C to interrupt before the next two ask
    // for and confirm the exit.
    for _ in 0..3 {
        if exited() || s.input_tx.send(Input::Bytes(b"\x03".to_vec())).is_err() {
            break;
        }
        std::thread::sleep(CTRL_C_GAP);
    }
    let wait = |until: std::time::Instant| {
        while !exited() && std::time::Instant::now() < until.min(deadline) {
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    };
    // Claude Code in a worktree answers the quit with "Exiting worktree
    // session — keep or remove?", which Ctrl+C does not dismiss: every deleted
    // worktree chat sat out the whole grace and then took the hard kill. Enter
    // takes the focused default, Keep — the right answer either way, since
    // Luna removes the worktree itself afterwards when the delete asks for
    // that. The Ctrl+Cs have already cleared the prompt, so an Enter that
    // finds no dialog submits nothing.
    wait(std::time::Instant::now() + EXIT_DIALOG_WAIT);
    if !exited() {
        let _ = s.input_tx.send(Input::Bytes(b"\r".to_vec()));
    }
    wait(deadline);
    if !exited() {
        crate::log::warn("pty", &format!("pid {:?} did not quit on Ctrl+C within {grace:?}; killing it", s.pid));
    }
    // Either way: the CLI's own children (shells, MCP servers) do not follow it
    // out, and a taskkill on a pid that has just exited is a no-op.
    kill_tree(s.pid);
    let _ = s.killer.kill();
}

/// Takes the session out of the map first and shuts it down after: the
/// shutdown waits on the process, and waiting under the map would put every
/// other chat's keystrokes behind it.
async fn take_and_shut_down(state: &PtyManager, id: &str) {
    let session = state.sessions.lock().unwrap().remove(id);
    if let Some(s) = session {
        let _ = tauri::async_runtime::spawn_blocking(move || shut_down(s, GRACE)).await;
    }
}

/// Asks every session to quit the way a delete does, before an update: the
/// installer ends the process, and a CLI given its exit hooks leaves a whole
/// transcript where one killed mid-write would not. Returns how many it
/// stopped.
#[tauri::command]
pub async fn stop_all_sessions() -> Result<usize, String> {
    tauri::async_runtime::spawn_blocking(|| manager().shut_down_all(GRACE))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn kill_session(id: String) -> Result<(), String> {
    take_and_shut_down(manager(), &id).await;
    Ok(())
}

/// Tears a chat down in one shot: resolve where the session actually lives,
/// kill it, drop its worktree (and the branch made for it) and its
/// attachments. Doing this in one command removes the race the UI had — it
/// used to rely on a 4s poll having already reported the worktree path.
#[tauri::command]
// Async for the same reason as the worktree commands: removing a worktree can
// take seconds, and on the main thread every keystroke in every other chat
// waits behind it.
pub async fn delete_session(
    id: String,
    folder: String,
    worktree_path: Option<String>,
    // Off by default at the call site: the worktree may hold work that exists
    // nowhere else, and a deleted chat is a cheaper mistake than a deleted
    // branch. Returns the path either way, so the caller can say where the
    // kept worktree is.
    drop_worktree: bool,
) -> Result<Option<String>, String> {
    manager().delete(&id, &folder, worktree_path, drop_worktree).await
}

/// The cwd of every live session this app runs. A running session must never
/// have its worktree swept, even if the UI has not yet learned where it lives.
pub fn live_cwds() -> Vec<String> {
    manager()
        .activity_probes()
        .into_iter()
        .flat_map(|p| {
            let moved = p.working_dir();
            [Some(p.cwd), moved].into_iter().flatten()
        })
        .collect()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrphanSession {
    /// The chat id the session was spawned under. Re-creating a chat row with
    /// this exact id is what reattaches it, scrollback included.
    pub id: String,
    pub provider: Provider,
    pub pid: Option<u32>,
    pub cwd: String,
    pub account_path: String,
    pub title: Option<String>,
    pub status: Option<String>,
}

/// Sessions this app is still running that no chat claims any more.
///
/// A chat row can go missing while its session keeps working — dropped by a
/// half-written state restore, say — and there is then no
/// way to reach the session from the UI at all: it holds an account's tokens
/// and answers to nobody. The frontend passes the chat ids it knows, and
/// anything alive outside that set comes back here.
///
/// Only this app's own ptys are considered. Claude Code registers its
/// subagents and shells in the same account registry, so walking that instead
/// would report every busy chat's children as orphans.
#[tauri::command]
pub async fn orphan_sessions(known: Vec<String>) -> Result<Vec<OrphanSession>, String> {
    let state = manager();
    let candidates: Vec<Probe> = {
        let sessions = state.sessions.lock().unwrap();
        sessions
            .iter()
            .filter(|(id, s)| !known.contains(id) && s.alive.load(Ordering::SeqCst))
            .map(|(id, s)| probe_of(id, s))
            .collect()
    };
    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    // Naming one means reading the head of its transcript, so do it off the
    // main thread like session_meta does.
    tauri::async_runtime::spawn_blocking(move || {
        candidates
            .into_iter()
            .map(|p| {
                let meta = meta_from_disk(&p);
                OrphanSession {
                    id: p.id,
                    provider: p.provider,
                    pid: p.pid,
                    cwd: p.cwd,
                    account_path: p.account_path,
                    title: meta.as_ref().and_then(|m| {
                        m.title.clone().or_else(|| m.first_prompt.clone()).or_else(|| m.name.clone())
                    }),
                    status: meta.and_then(|m| m.status),
                }
            })
            .collect()
    })
    .await
    .map_err(|e| e.to_string())
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SessionMeta {
    pub name: Option<String>,
    /// Claude Code's own vocabulary as the registry has it, or for Codex
    /// `working` | `waiting` | `resting`.
    pub status: Option<String>,
    pub cwd: Option<String>,
    pub session_id: Option<String>,
    /// "auto" (AI-titled), "user" (renamed by hand) or "derived" (just the cwd
    /// folder name, which for a worktree session is meaningless noise).
    pub name_source: Option<String>,
    /// The model the session is actually running, where the CLI records it
    /// (Codex's `turn_context`); None where the chat setting is the truth.
    pub model: Option<String>,
    pub context: Option<f64>,
    pub context_tokens: Option<f64>,
    /// Window the percentage was computed against, so the UI can show what it
    /// assumed instead of silently pinning a longer session at 100%.
    pub context_window: Option<f64>,
    /// The CLI's own title for the session: a rename, else the one it
    /// generated (Claude Code's `ai-title`, the same it puts on the terminal
    /// tab), else None.
    pub title: Option<String>,
    /// The title is a rename the user gave it in the CLI, not one the CLI
    /// came up with.
    pub title_renamed: bool,
    /// First real prompt of the session — the chat's title until the CLI
    /// comes up with one.
    pub first_prompt: Option<String>,
}

/// A session's title as its CLI has it, and whether the user typed it.
#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CliTitle {
    pub title: String,
    pub renamed: bool,
}

/// A title has to be something a person can read. Both sources have handed us
/// strings that were not: a registry name made of the terminal's own cursor
/// and mouse reports, which is what a chat row read as for its first minutes.
/// Rejecting them here keeps that out of every place a title is shown.
pub fn sane_title(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty() && !t.chars().any(char::is_control)).then(|| t.to_string())
}

/// A prompt trimmed to a title: whitespace flattened, 48 characters. Slash
/// commands, replayed tool output and system reminders are not titles.
pub fn title_from_prompt(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() || text.starts_with('<') || text.starts_with('/') {
        return None;
    }
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut title: String = flat.chars().take(48).collect();
    if flat.chars().count() > 48 {
        title.push('…');
    }
    sane_title(&title)
}

/// Replays raw pty output through a terminal emulator and returns the visible
/// screen as text, blank trailing lines dropped. The scrollback is capped
/// (SCROLLBACK_CAP), so a very long session starts mid-stream; a fullscreen
/// TUI repaints everything on each frame, so that costs nothing visible.
pub fn render_screen(bytes: &[u8], rows: u16, cols: u16) -> String {
    let mut parser = vt100::Parser::new(rows.max(1), cols.max(1), 0);
    parser.process(bytes);
    let contents = parser.screen().contents();
    let mut lines: Vec<&str> = contents.lines().map(str::trim_end).collect();
    while lines.last().map(|l| l.is_empty()).unwrap_or(false) {
        lines.pop();
    }
    lines.join("\n")
}

/// Codex reads key presses, not text. ConPTY makes a key press out of each
/// character it is given, on the console's keyboard layout, and a character
/// that layout has no key for reached Codex as nothing: on a Russian layout
/// `[ ] { } < > ' ` ~ @ # $ ^ & |` and typographic quotes and dashes all
/// vanished, from a person's typing and from `luna_send` alike. Given as
/// win32-input-mode key events (ConPTY is opened with WIN32_INPUT_MODE), every
/// character arrives as itself whatever the layout. Ctrl+<letter> is the same
/// story: ConPTY hands Codex the control byte and Codex asks the layout which
/// letter that key is, so on a Russian one Ctrl+C came in as Ctrl+С — no
/// shortcut of its: a selection was dropped instead of copied. Sent as the
/// Latin letter with Ctrl held, it is Ctrl+C on any layout. The control bytes
/// of keys of their own (Backspace, Tab, Enter) and escape sequences — arrows,
/// focus and mouse reports, a cursor report — go through as they are.
fn as_key_events(bytes: &[u8]) -> Vec<u8> {
    use std::fmt::Write as _;
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(bytes.len() * 24);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            out.push(c);
            match chars.next() {
                // CSI: parameters and intermediates up to the final byte.
                Some('[') => {
                    out.push('[');
                    for n in chars.by_ref() {
                        out.push(n);
                        if ('\x40'..='\x7e').contains(&n) {
                            break;
                        }
                    }
                }
                // SS3 (application cursor keys, F1-F4): one more character.
                Some('O') => {
                    out.push('O');
                    if let Some(n) = chars.next() {
                        out.push(n);
                    }
                }
                // Alt with a key.
                Some(n) => out.push(n),
                None => {}
            }
        } else if let Some(letter) = ctrl_letter(c) {
            // Vk;Sc;Uc;Kd;Cs;Rc with LEFT_CTRL_PRESSED. The real virtual key,
            // so a console still in processed input mode takes Ctrl+C as one.
            let vk = letter.to_ascii_uppercase() as u32;
            let uc = letter as u32;
            let _ = write!(out, "\x1b[{vk};0;{uc};1;8;1_\x1b[{vk};0;{uc};0;8;1_");
        } else if c < ' ' || c == '\x7f' {
            out.push(c);
        } else {
            let mut units = [0u16; 2];
            let units = c.encode_utf16(&mut units);
            // Vk;Sc;Uc;Kd;Cs;Rc — down, then up. The halves of a surrogate
            // pair go down only: Codex took the release of the first half for
            // the end of the character and dropped it.
            let pair = units.len() == 2;
            for u in units.iter() {
                let _ = write!(out, "\x1b[0;0;{u};1;0;1_");
                if !pair {
                    let _ = write!(out, "\x1b[0;0;{u};0;0;1_");
                }
            }
        }
    }
    out.into_bytes()
}

/// The letter a terminal's Ctrl+<letter> byte stands for, except the bytes that
/// are keys of their own: Backspace, Tab, Ctrl+J and Enter.
fn ctrl_letter(c: char) -> Option<char> {
    match c {
        '\x08' | '\t' | '\n' | '\r' => None,
        '\x01'..='\x1a' => Some((b'a' + c as u8 - 1) as char),
        _ => None,
    }
}

/// Set in the draft state when the last character typed was a backslash.
const DRAFT_BACKSLASH: u64 = 1 << 63;

/// The composer's length after a person's keys, roughly: characters add,
/// Backspace takes one off, Enter, Ctrl+C and Ctrl+U empty it — but not an
/// Enter right after a backslash, which is how a Claude Code prompt gets its
/// next line — and the sequences a terminal sends for other keys (arrows,
/// focus, mouse) do nothing, except a bracketed paste, whose text counts.
/// Edits it cannot follow (a word deleted with Ctrl+W, a selection cut) leave
/// it high, which only holds a notice until the next Enter. The top bit
/// carries the backslash across writes, one key arriving per write.
fn draft_after(state: u64, data: &str) -> u64 {
    let mut n = state & !DRAFT_BACKSLASH;
    let mut backslash = state & DRAFT_BACKSLASH != 0;
    let mut chars = data.chars().peekable();
    while let Some(c) = chars.next() {
        let was_backslash = std::mem::replace(&mut backslash, c == '\\');
        match c {
            '\r' if was_backslash => n += 1,
            '\r' | '\x03' | '\x15' => n = 0,
            '\x7f' | '\x08' => n = n.saturating_sub(1),
            // A sequence types nothing — the paste markers included; the
            // text between them is counted as it comes.
            '\x1b' => {
                if chars.peek() == Some(&'[') {
                    chars.next();
                    for p in chars.by_ref() {
                        if ('\x40'..='\x7e').contains(&p) {
                            break;
                        }
                    }
                } else if chars.next() == Some('O') {
                    // SS3: arrows in application mode, F1-F4.
                    chars.next();
                }
            }
            c if c < ' ' => {}
            _ => n += 1,
        }
    }
    n | if backslash { DRAFT_BACKSLASH } else { 0 }
}

/// Where the cursor stands once `bytes` have been drawn at this size, zero-based
/// (row, col).
fn cursor_at(bytes: &[u8], rows: u16, cols: u16) -> (u16, u16) {
    let mut parser = vt100::Parser::new(rows.max(1), cols.max(1), 0);
    parser.process(bytes);
    parser.screen().cursor_position()
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

/// The DEC private modes worth carrying over to a pane that attaches late:
/// cursor visibility, cursor-key mode, mouse reporting, focus reporting and
/// bracketed paste. A CLI switches these once, at start, and never again —
/// so they are long gone from the capped scrollback, and a reattaching pane
/// resets its terminal anyway (see terminals.ts). Without them the pane drew
/// its own cursor wherever the CLI last wrote, on top of the one the CLI
/// draws in its input box, and the stray one wandered the screen with every
/// repaint. The alternate screen (1049) is left out on purpose: the pane
/// repaints onto the normal screen after the reset. So is synchronized output
/// (2026), which would freeze the pane if it were replayed mid-frame.
const KEPT_MODES: &[u16] = &[1, 25, 1000, 1002, 1003, 1004, 1006, 2004];

/// The last state of each of `KEPT_MODES`, read off the output as it streams,
/// so the scrollback cap cannot take it away. The scanner keeps its place
/// between reads, so a sequence split across two of them still counts.
#[derive(Default)]
struct Modes {
    on: std::collections::BTreeMap<u16, bool>,
    /// 0 ground, 1 after ESC, 2 after ESC [, 3 in the parameters of ESC [ ?.
    state: u8,
    params: Vec<u8>,
}

impl Modes {
    fn feed(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.state = match (self.state, b) {
                (_, 0x1b) => 1,
                (1, b'[') => 2,
                (2, b'?') => {
                    self.params.clear();
                    3
                }
                (3, b'0'..=b'9' | b';') if self.params.len() < 64 => {
                    self.params.push(b);
                    3
                }
                (3, b'h' | b'l') => {
                    self.apply(b == b'h');
                    0
                }
                _ => 0,
            };
        }
    }

    fn apply(&mut self, set: bool) {
        for p in self.params.split(|&c| c == b';') {
            let n = std::str::from_utf8(p).ok().and_then(|p| p.parse::<u16>().ok());
            if let Some(n) = n.filter(|n| KEPT_MODES.contains(n)) {
                self.on.insert(n, set);
            }
        }
    }

    fn replay(&self) -> String {
        self.on
            .iter()
            .map(|(n, set)| format!("\x1b[?{n}{}", if *set { 'h' } else { 'l' }))
            .collect()
    }
}

/// The last `max` bytes of a file as text, or None if it cannot be read.
pub fn tail(path: &std::path::Path, max: u64) -> Option<String> {
    use std::io::{Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let take = len.min(max);
    f.seek(SeekFrom::End(-(take as i64))).ok()?;
    let mut bytes = Vec::with_capacity(take as usize);
    f.read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// Calls `each` with every complete line of a file from byte `from` on and
/// returns the offset just past the last of them. A last line without its
/// newline is still being written: it is left, cursor and all, for the read
/// after.
pub fn complete_lines(path: &std::path::Path, from: u64, mut each: impl FnMut(&str)) -> u64 {
    use std::io::{BufRead, BufReader, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else { return from };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let from = from.min(len);
    if f.seek(SeekFrom::Start(from)).is_err() {
        return from;
    }
    let mut reader = BufReader::new(f);
    let mut line = Vec::new();
    let mut read = from;
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(n) if line.ends_with(b"\n") => read += n as u64,
            _ => break,
        }
        each(String::from_utf8_lossy(&line).trim_end_matches(['\r', '\n']));
    }
    read
}

/// What a session-meta read needs, copied out from under the map.
struct Probe {
    id: String,
    provider: Provider,
    pid: Option<u32>,
    cwd: String,
    spawned_at_ms: u128,
    resume: Option<String>,
    account_path: String,
    last_output_ms: u64,
}

fn probe_of(id: &str, s: &Session) -> Probe {
    Probe {
        id: id.to_string(),
        provider: s.provider,
        pid: s.pid,
        cwd: s.cwd.clone(),
        spawned_at_ms: s.spawned_at_ms,
        resume: s.resume.clone(),
        account_path: s.account_path.clone(),
        last_output_ms: s.last_output_ms.load(Ordering::Relaxed),
    }
}

fn meta_from_disk(p: &Probe) -> Option<SessionMeta> {
    match p.provider {
        Provider::Claude => crate::claude::session::meta(p.pid, &p.cwd, p.spawned_at_ms, &p.account_path),
        Provider::Codex => crate::codex::session::meta(&crate::codex::session::Live {
            chat_id: &p.id,
            cwd: &p.cwd,
            spawned_at_ms: p.spawned_at_ms,
            resume: p.resume.as_deref(),
            account_path: &p.account_path,
            last_output_ms: p.last_output_ms,
        }),
    }
}

// Name, status and context of a live session, read off what its CLI leaves on
// disk — free, no tokens.
//
// Async on purpose: this runs every few seconds for every open pane and reads
// the tail of a transcript that grows into the megabytes. As a sync command it
// would do that on the main thread, i.e. with the window frozen.
#[tauri::command]
//
// A chat with no live session answers None rather than failing: the watcher
// polls every chat every few seconds, resting ones included, so treating an
// absent session as an error meant thousands of warnings a day in the log —
// enough to push everything worth reading out of it.
pub async fn session_meta(id: String, account_path: String) -> Result<Option<SessionMeta>, String> {
    let state = manager();
    let probe = {
        let sessions = state.sessions.lock().unwrap();
        let Some(s) = sessions.get(&id) else {
            return Ok(None);
        };
        if !s.alive.load(Ordering::SeqCst) {
            return Ok(None);
        }
        let mut p = probe_of(&id, s);
        // The caller's idea of the account wins, as it always has: the chat
        // row is the source of truth for which folder a chat belongs to.
        if !account_path.is_empty() {
            p.account_path = account_path;
        }
        p
    };

    let codex = probe.provider == Provider::Codex;
    let mut meta = tauri::async_runtime::spawn_blocking(move || meta_from_disk(&probe))
        .await
        .map_err(|e| e.to_string())?;
    // Codex's approval dialog is not in its rollout and the screen does not
    // go quiet under it, so the rollout calls the session working; the
    // activity sampler reads the dialog off the screen (`codex::session::asks`).
    if let Some(m) = meta.as_mut().filter(|m| codex && m.status.as_deref() == Some("working")) {
        if crate::activity::turn_state(&id).is_some_and(|t| t.turn == crate::activity::Turn::Waiting) {
            m.status = Some("waiting".into());
        }
    }
    Ok(meta)
}

/// The CLI's title for a session that is not running, by its id — how a chat
/// restored from an earlier run picks up the title its CLI gave it.
#[tauri::command]
pub async fn saved_title(provider: Provider, account_path: String, session_id: String) -> Option<CliTitle> {
    tauri::async_runtime::spawn_blocking(move || match provider {
        Provider::Claude => crate::claude::session::saved_title(&account_path, &session_id),
        Provider::Codex => crate::codex::session::thread_title(&account_path, &session_id),
    })
    .await
    .ok()
    .flatten()
}

/// The escapes that put a pane's terminal back in the modes the session's CLI
/// switched — written after a reattaching pane resets (see `Modes`). Empty for
/// a session that is not there.
#[tauri::command]
pub fn session_modes(id: String) -> String {
    let state = manager();
    let modes = {
        let sessions = state.sessions.lock().unwrap();
        match sessions.get(&id) {
            Some(s) => Arc::clone(&s.modes),
            None => return String::new(),
        }
    };
    let replay = modes.lock().unwrap_or_else(|e| e.into_inner()).replay();
    replay
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_survive_split_reads_and_keep_only_the_last_word() {
        let mut m = Modes::default();
        m.feed(b"\x1b[?25l\x1b[?20");
        m.feed(b"04h\x1b[?1049h\x1b[?2026h\x1b[?1000;1006h");
        m.feed(b"text\x1b[?1000l\x1b[31m\x1b[?");
        m.feed(b"1004h");
        assert_eq!(m.replay(), "\x1b[?25l\x1b[?1000l\x1b[?1004h\x1b[?1006h\x1b[?2004h");
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
    fn codex_gets_characters_as_key_events_and_sequences_as_they_are() {
        let k = |u: u32| format!("\x1b[0;0;{u};1;0;1_\x1b[0;0;{u};0;0;1_");
        assert_eq!(String::from_utf8(as_key_events(b"[")).unwrap(), k(91));
        // Enter, an arrow, a focus report, a cursor report, Alt+x: untouched.
        let raw = "\r\x1b[A\x1b[I\x1b[12;3R\x1bx\x1bOP\x7f";
        assert_eq!(String::from_utf8(as_key_events(raw.as_bytes())).unwrap(), raw);
        // Outside the basic plane: both halves of the surrogate pair, pressed.
        let down = |u: u32| format!("\x1b[0;0;{u};1;0;1_");
        assert_eq!(String::from_utf8(as_key_events("😀".as_bytes())).unwrap(), format!("{}{}", down(0xD83D), down(0xDE00)));
        assert_eq!(String::from_utf8(as_key_events("a\rя".as_bytes())).unwrap(), format!("{}\r{}", k(97), k(0x44F)));
        // Ctrl+C and Ctrl+T by their Latin letters, whatever the layout.
        let ctrl = |vk: u32, u: u32| format!("\x1b[{vk};0;{u};1;8;1_\x1b[{vk};0;{u};0;8;1_");
        assert_eq!(String::from_utf8(as_key_events(b"\x03\x14")).unwrap(), format!("{}{}", ctrl(67, 99), ctrl(84, 116)));
        // Backspace, Tab, Ctrl+J and Enter stay keys of their own.
        assert_eq!(String::from_utf8(as_key_events(b"\x08\t\n\r")).unwrap(), "\x08\t\n\r");
    }

    #[test]
    fn a_draft_is_what_was_typed_since_enter() {
        assert_eq!(draft_after(0, "abc"), 3);
        assert_eq!(draft_after(3, "\x7f"), 2);
        assert_eq!(draft_after(2, "\r"), 0);
        assert_eq!(draft_after(0, "\x1b[A\x1b[I\x1b[<35;1;14M\x1bOP"), 0, "keys that type nothing");
        assert_eq!(draft_after(0, "\x1b[200~hi\x1b[201~"), 2, "a paste is text");
        assert_eq!(draft_after(5, "\x03"), 0);
        assert_eq!(draft_after(5, "\x15"), 0);
        assert_eq!(draft_after(0, "привет"), 6);
        assert_eq!(draft_after(1, "\x7f\x7f\x7f"), 0);
        // A backslash and Enter, one key per write: a second line, not a send.
        let s = draft_after(draft_after(0, "ab\\"), "\r");
        assert_eq!(s & !DRAFT_BACKSLASH, 4);
        assert_eq!(draft_after(draft_after(s, "c"), "\r"), 0);
    }

    #[test]
    fn the_sandbox_path_has_no_store_aliases() {
        let path = r"C:\Users\n\AppData\Local\Microsoft\WindowsApps\Microsoft.PowerShell_8wekyb3d8bbwe;C:\WINDOWS\system32;C:\Users\n\AppData\Local\Microsoft\WindowsApps;C:\Program Files\PowerShell\7";
        assert_eq!(without_windows_apps(path), r"C:\WINDOWS\system32;C:\Program Files\PowerShell\7");
        assert_eq!(codex_sandbox_path("danger-full-access"), None, "full access keeps the PATH it had");
    }

    #[test]
    fn a_login_session_is_told_by_its_folder() {
        assert!(same_dir("C:\\acc\\anthropic\\one\\", "C:/acc/anthropic/one"));
        assert!(!same_dir("C:\\p", "C:\\acc\\anthropic\\one"));
    }

    #[test]
    fn the_answer_is_where_the_cursor_stands() {
        assert_eq!(cursor_at(b"", 30, 100), (0, 0));
        assert_eq!(cursor_at(b"hello\r\nab", 30, 100), (1, 2));
        assert_eq!(cursor_at(b"\x1b[5;7H", 30, 100), (4, 6));
    }
}
