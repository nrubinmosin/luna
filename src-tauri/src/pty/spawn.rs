//! A chat's CLI in its pty: the process started under the chat's spawn gate,
//! and the threads that write to it, read and coalesce its output, answer
//! its cursor queries and notice when it has gone.

use super::control::end_leftovers;
use super::screen::{as_key_events, backlog_at, drawn_screen, render_screen, Drawn, Modes, Scrollback, Sent};
use super::{manager, now_ms, write_input, Input, InputTx, ReaderEnd, Session, READER_WAIT};
use crate::provider::Provider;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::Serialize;
use std::io::Read;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const SCROLLBACK_CAP: usize = 2 * 1024 * 1024;
/// Let the buffer run this far past the cap before trimming, so the trim costs
/// one pass per slack rather than one per read.
const SCROLLBACK_SLACK: usize = 256 * 1024;
/// Reads (8 KB at most each) the reader may have waiting for the emitter
/// before it waits itself: 4 MB, many frames' worth. The emitter only puts
/// them into an event, which `emit::to_ui` hands on without waiting, so this
/// is reached only when it is starved of time; the reader waiting then holds
/// the CLI's writes back, and none of its output is dropped or reordered.
const OUTPUT_PIECES: usize = 512;
/// How long output is allowed to pile up before it is sent to the UI — one
/// frame at 60Hz, short enough to feel immediate and long enough that a
/// repainting TUI does not turn into hundreds of events per second.
const FRAME: std::time::Duration = std::time::Duration::from_millis(16);
/// What a Claude Code or Codex session puts in the environment of everything
/// it runs: its identity, its IPC, its sandbox flags, and the bearer token of
/// another Luna's tools and the URL its status hooks post to. None of it
/// belongs to a session Luna starts (see spawn); a session given Luna's tools
/// or hooks gets this Luna's after.
pub(super) const INHERITED_SESSION_VARS: &[&str] = &[
    "LUNA_MCP_TOKEN",
    "LUNA_HOOK_URL",
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

/// A piece of a session's output. `offset`: where its text starts in all the
/// text the session has sent, in UTF-8 bytes (`Sent`), which a pane that
/// attaches matches against its backlog's `backlog_end`; the page ignores it.
#[derive(Serialize, Clone)]
struct PtyOutput<'a> {
    id: &'a str,
    data: &'a str,
    offset: u64,
}

/// A session's exit, with its spawn's number (`Session::generation`): the
/// window applies it to the session its start answered with that number
/// (`Attached`), not to one started under the chat's id since.
#[derive(Serialize, Clone)]
struct PtyExit<'a> {
    id: &'a str,
    code: Option<u32>,
    generation: u64,
}

/// What a start answers: what the session has printed so far (empty for one
/// that has just started), and its spawn's number, which its exit event
/// carries. For the native window's terminal, which the page ignores: the
/// text offset the backlog ends at (`PtyOutput::offset`), so a pane takes
/// the events from exactly there, and the escapes that put the modes back as
/// of there (`Sent`); a session just started ends at 0 and has none to put
/// back.
#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Attached {
    pub backlog: String,
    pub generation: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backlog_end: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission: Option<luna_window::dto::ResolvedPermission>,
}

impl Attached {
    /// A session just started: nothing printed, nothing sent yet.
    fn fresh(generation: u64, permission: Option<luna_window::dto::ResolvedPermission>) -> Attached {
        Attached { backlog: String::new(), generation, backlog_end: Some(0), modes: None, permission }
    }
}

fn resolved_permission(id: &str) -> Option<luna_window::dto::ResolvedPermission> {
    serde_json::from_value(crate::agents::settings_of(id)).ok()
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
    /// The account counted as in use since before the start's library pass
    /// (`library::lay_out_for`), until the process is counted itself.
    pub(super) reserved: Option<crate::account_use::Running>,
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

/// Whether a live session on `held_path`, whose folder was `held_dir` as it
/// started, is on the account folder a start asks for: `path`, now `asked_dir`.
/// Where the start names the identity the folder must have (`account_id`),
/// the session's own is held to it, whatever the paths say. Else the same
/// path is the same folder, as it always was, and so is another path to a
/// folder of the same identity (a root changed to another link to it): ids
/// that a volume makes up or moves cannot refuse a reattach the path allows.
pub(super) fn same_account(held_path: &str, held_dir: Option<&str>, path: &str, account_id: Option<&str>, asked_dir: Option<&str>) -> bool {
    if let Some(id) = account_id {
        return held_dir == Some(id);
    }
    super::same_dir(held_path, path) || matches!((held_dir, asked_dir), (Some(held), Some(asked)) if held == asked)
}

/// What the first start of an adopted orphan's pane is refused with where the
/// session it was adopted for is no longer the one under its id
/// (`spawn`): nothing is started, and the chat starts again only when
/// the person asks it to.
pub(crate) const ADOPTED_GONE: &str = "the session this chat was adopted from has ended; start the chat again to resume it";

// Creating a session means a process spawn, and for a worktree the CLI's own git — seconds of
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
// process holding a token Luna no longer knew. `account_path` is the account
// folder the start asks for, and `account_id` the identity it must have where
// one is known (a new account's first sign-in): a live session of the id on
// another folder is not attached to (a login's id names the account, not its
// folder, and the root may have moved since), and the start is refused.
//
// With `expect` it is an attach to that start of the session alone (the first
// start of an orphan's pane once adopted, which the orphan line was told of):
// where that is not the one running under the id, gone since the adopt or
// another start of the id, nothing is launched and the start is refused with
// `ADOPTED_GONE`. Both hold where both are given: the start, by the map alone,
// before any folder is read, then the account folder, then the attach.
pub(super) async fn spawn(
    id: String,
    account_path: &str,
    account_id: Option<&str>,
    expect: Option<super::Start>,
    launch: impl FnOnce() -> Result<Launch, String>,
) -> Result<Attached, String> {
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

    // A chat deleted is gone for good, whatever still asks for it: a pane
    // that had not let go yet waited out the delete on the gate, and would
    // otherwise start a session of a chat with no row, its registry entry,
    // token and files cleared by the delete. Ids are never given out again.
    if state.deleted.lock().unwrap_or_else(|e| e.into_inner()).contains(&id) {
        crate::log::warn("pty", &format!("not starting {id}: the chat was deleted"));
        return Err("this chat was deleted".into());
    }

    // Under the gate: whoever waited here may find the session already
    // spawned, and it is attached to only on the account folder asked for.
    // The map is looked at first, and a folder read only where the answer
    // needs it, outside the map (every other chat's keys go through it; a
    // share that does not answer would hold them all): the gate keeps the
    // session in place meanwhile.
    let live = {
        let sessions = state.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions.get(&id).filter(|s| s.alive.load(Ordering::SeqCst)).map(|s| (s.account_path.clone(), s.account_dir.clone(), s.generation))
    };
    // An adopted orphan's first start: that start of the session, or none,
    // told by the map before any folder is read.
    if let Some(expect) = &expect {
        if !live.as_ref().is_some_and(|(_, _, generation)| expect.is(*generation)) {
            crate::log::info("pty", &format!("not starting {id}: the session it was adopted from is not the one running"));
            return Err(ADOPTED_GONE.into());
        }
    }
    if let Some((held_path, held_dir, held_generation)) = live {
        // A session whose folder could not be read as it started is read
        // again, once, where a start names the identity it must have.
        let held_dir = match (held_dir, account_id) {
            (None, Some(_)) => {
                let read = crate::accounts::folder_identity(&held_path);
                let mut sessions = state.sessions.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(s) = sessions.get_mut(&id).filter(|s| s.generation == held_generation) {
                    s.account_dir.clone_from(&read);
                }
                read
            }
            (held_dir, _) => held_dir,
        };
        // The folder asked for, read only where no identity is named and the
        // paths differ: the same path attaches with no read at all.
        let asked_dir = (account_id.is_none() && !super::same_dir(&held_path, account_path))
            .then(|| crate::accounts::folder_identity(account_path))
            .flatten();
        if !same_account(&held_path, held_dir.as_deref(), account_path, account_id, asked_dir.as_deref()) {
            let why = format!("{id} is running on another account folder ({held_path}); close it before starting it on {account_path}");
            crate::log::warn("pty", &format!("not attaching: {why}"));
            return Err(why);
        }
    }
    let exited = {
        let mut sessions = state.sessions.lock().unwrap_or_else(|e| e.into_inner());
        match sessions.get(&id) {
            Some(s) if s.alive.load(Ordering::SeqCst) && expect.as_ref().is_none_or(|e| e.is(s.generation)) => {
                Err((Arc::clone(&s.scrollback), Arc::clone(&s.sent), s.generation))
            }
            _ if expect.is_some() => {
                crate::log::info("pty", &format!("not starting {id}: the session it was adopted from is not the one running"));
                return Err(ADOPTED_GONE.into());
            }
            _ => Ok(sessions.remove(&id)),
        }
    };
    // Its scrollback, up to two megabytes, copied out once the session map
    // is let go: every other chat's keys go through the map. The gate still
    // holds the session in place.
    let exited = match exited {
        Ok(exited) => exited,
        Err((scrollback, sent, generation)) => {
            let (bytes, end, modes) = backlog_at(&scrollback, &sent);
            let backlog = String::from_utf8_lossy(&bytes).into_owned();
            return Ok(Attached { backlog, generation, backlog_end: Some(end), modes: Some(modes), permission: resolved_permission(&id) });
        }
    };
    // A CLI that exited by itself: what it left running goes with the old
    // session, before a new one takes the chat, and its reader has said it
    // exited before the new one can be taken for it (`ReaderEnd`).
    if let Some(old) = exited {
        end_leftovers(&old);
        if !old.reader_end.wait(READER_WAIT) {
            crate::log::warn("pty", &format!("{id}'s last session did not finish exiting in {READER_WAIT:?}; starting anyway"));
        }
    }
    // Let in by the start gate before anything of the launch is built, and
    // counted until it is registered: an update's stop starts no pane, and
    // waits for this one before it takes its snapshot (`crate::starts`). An
    // attach to a running session, above, starts nothing.
    let launching = crate::starts::launching(&id).inspect_err(|why| crate::log::warn("pty", &format!("not starting {id}: {why}")))?;
    // Before the launch, whose registration clears the chat's last exit: an
    // earlier session's exit is said by now or never (`next_generation`).
    let generation = state.next_generation(&id);
    // Finished here, so a test's capture holds the process as it will run.
    let launch = launch()?.finished();
    let permission = resolved_permission(&id);
    #[cfg(test)]
    said::note(&format!("launch {id}"));
    // A test reads what a start would run, and nothing runs (`capture`).
    #[cfg(test)]
    let Some(launch) = capture::keep(&id, launch) else {
        return Ok(Attached::fresh(generation, permission));
    };
    let Launch { provider, cwd, account_path, args, envs, resume, describe, applied: _, reserved } = launch;

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

    // What the account folder is as the session starts, which a later start
    // asking for it is held to (`same_account`).
    let account_dir = crate::accounts::folder_identity(&account_path);
    // Counted from here until the process has exited, however long a delete
    // takes over it: the map lets go of a session before it is gone.
    let on_account = crate::account_use::running_on(&account_path);
    drop(reserved);

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize { rows: INITIAL_ROWS, cols: INITIAL_COLS, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| e.to_string())?;

    // Luna's own copy of the CLI (see cli.rs), nothing else; its version
    // directory held, like the account, until the process has exited.
    let (exe, on_version) = crate::cli::launch(provider, Some(&id))?;
    let mut cmd = CommandBuilder::new(exe);
    cmd.args(&args);
    // A throwaway account of the test build's may ask for a stand-in instead
    // of its CLI (stand_in.rs).
    #[cfg(luna_test)]
    if let Some((program, stand_in_args)) = super::stand_in::read(&account_path) {
        crate::log::info("pty", &format!("{id} runs the test stand-in {program} instead of its CLI"));
        cmd = CommandBuilder::new(program);
        cmd.args(&stand_in_args);
    }
    cmd.cwd(&cwd);
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
    // The chat's mark, as a helper's turn has its own: off Windows its job is
    // what carries it too, so a shell that leaves the CLI's tree, group and
    // session is still the chat's (keeper/ `marked`).
    #[cfg(unix)]
    cmd.env(crate::keeper::MARK, crate::keeper::mark_of(&id));

    // Ahead of the process: what it makes (a rollout) is born after this,
    // however long what follows its start waits on the registry's lock.
    let spawned_at_ms = now_ms() as u128;
    // Where the thread it resumes ends before it writes a line: what comes
    // after is this process's, whatever the wall clock says (codex/session
    // `Since`).
    if provider == Provider::Codex {
        crate::codex::session::note_spawn(&id, spawned_at_ms, &account_path, resume.as_deref());
    }
    // Created only while an update's stop still waits for this launch, if one
    // is under way: past its wait the launch is refused here, before any CLI
    // runs that would then have to be asked to quit (`Launching::create`).
    let created = launching
        .create(|| pair.slave.spawn_command(cmd))
        .inspect_err(|why| crate::log::warn("pty", &format!("not starting {id}: {why}")))?;
    let child = created.map_err(|e| {
        crate::log::error("pty", &format!("spawn failed for {id} in {cwd}: {e}"));
        e.to_string()
    })?;
    // Up: the canary may count it (a spawn that failed never is).
    crate::cli::session_running(&id);
    drop(pair.slave);

    let pid = child.process_id();
    // Into a job before it has had time to spawn anything, so every child it
    // ever starts is listed with it.
    let job = pid.and_then(crate::procs::adopt).map(Arc::new);
    if let Some(job) = &job {
        crate::procs::marked(job, &id);
    }
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
    let (input_tx, input_rx, input_waiting) = InputTx::new(&id);
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
                let len = msg.len();
                match msg {
                    Input::Bytes(chunk) if writable => {
                        // Key events only where ConPTY reads them (see `as_key_events`).
                        let chunk = if cfg!(windows) && provider == Provider::Codex { as_key_events(&chunk) } else { chunk };
                        if let Err(e) = write_input(&mut writer, &chunk) {
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
                input_waiting.release(len);
            }
        });
    }

    // Sized once for the cap: grown by doubling, a deque settles at twice
    // what it holds, and drain never gives the slack back.
    let scrollback = Arc::new(Mutex::new(Scrollback::with_capacity(SCROLLBACK_CAP + SCROLLBACK_SLACK + 8192)));
    let drawn = Arc::new(Mutex::new(Drawn::default()));
    let modes = Arc::new(Mutex::new(Modes::default()));
    let sent = Arc::new(Mutex::new(Sent::default()));
    let alive = Arc::new(AtomicBool::new(true));
    let reader_end = Arc::new(ReaderEnd::default());
    let last_output_ms = Arc::new(AtomicU64::new(now_ms()));

    {
        let id = id.clone();
        let scrollback = Arc::clone(&scrollback);
        let drawn = Arc::clone(&drawn);
        let modes = Arc::clone(&modes);
        let alive = Arc::clone(&alive);
        let last_output_ms = Arc::clone(&last_output_ms);
        let child = Arc::clone(&child);
        let size = Arc::clone(&size);
        let reader_end = Arc::clone(&reader_end);

        // The reader hands bytes to an emitter thread instead of emitting them
        // itself. A repainting TUI produces a steady stream of small reads, and
        // one event per read means one IPC message plus one JSON payload per
        // read, per chat, all landing on the webview's single thread — the app
        // got slower with every busy chat. The emitter coalesces whatever
        // arrives inside a frame into one event. What waits for it is bounded
        // (`OUTPUT_PIECES`): past that the reader waits, and the CLI's writes
        // with it, rather than drop or reorder output.
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(OUTPUT_PIECES);
        let reading = id.clone();
        let sent = Arc::clone(&sent);
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

                let (text, used) = text_of(&carry);
                // Counted before it goes out: a backlog taken in between
                // has it, and the pane drops the event by its offset.
                let offset = sent.lock().unwrap_or_else(|e| e.into_inner()).took(&carry[..used], text.len());
                if !text.is_empty() {
                    crate::emit::to_ui("pty://output", PtyOutput { id: &id, data: &text, offset });
                }
                carry.drain(..used);
            }
            id
        });

        std::thread::spawn(move || {
            let mut chunk = [0u8; 8192];
            // The start of a cursor query cut off at the end of a read.
            let mut carry: Vec<u8> = Vec::new();
            // The emitter has fallen behind once: said once a session.
            let mut behind = false;
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
                                let mut sb = scrollback.lock().unwrap_or_else(|e| e.into_inner());
                                // Trimming to the cap on every read used to move the
                                // whole two megabytes each time; a deque drops from
                                // the front without touching the rest, and trimming
                                // in one go per overshoot keeps it off the hot path.
                                // Before the append, so it never outgrows the
                                // capacity it was made with.
                                let kept = sb.bytes.len();
                                if kept + piece.len() > SCROLLBACK_CAP + SCROLLBACK_SLACK {
                                    let excess = (kept + piece.len() - SCROLLBACK_CAP).min(kept);
                                    sb.bytes.drain(..excess);
                                }
                                sb.bytes.extend(piece.iter().copied());
                                sb.written += piece.len() as u64;
                                drop(sb);
                                if !hand_on(&tx, piece, &mut behind, &reading) {
                                    break 'read;
                                }
                            }
                            if i < last {
                                // A query followed this piece: answer with where the
                                // cursor stands after it, as a terminal would. The
                                // screen kept is caught up with the output since.
                                let size = *size.lock().unwrap_or_else(|e| e.into_inner());
                                let (row, col) = drawn_screen(&drawn, &scrollback, size, |s| s.cursor_position());
                                if let Some(input) = replies.upgrade() {
                                    let _ = input.reply(format!("\x1b[{};{}R", row + 1, col + 1).into_bytes());
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
            let Ok(id) = emitter.join() else {
                reader_end.finish(|| {});
                return;
            };
            let code = exit_code(&child);
            drop(on_account);
            drop(on_version);
            // For the canary over a new CLI version's first sessions, with the
            // screen it left for what it said as it went. An exit whose code
            // cannot be read counts as one with an error.
            crate::cli::session_ended(&id, code != Some(0), || {
                let (rows, cols) = *size.lock().unwrap_or_else(|e| e.into_inner());
                crate::cli::LastWords::of_screen(last_screen(
                    &scrollback.lock().unwrap_or_else(|e| e.into_inner()).last(LAST_SCREEN),
                    rows,
                    cols,
                ))
            });
            crate::log::info("pty", &format!("session {} exited, code {code:?}", crate::agents::label(&id)));
            let pm = manager();
            reader_exited(&id, generation, code, &reader_end);
            // The entry stays, for as long as the chat does: its transcript, its
            // folder and its last screen are still asked for (luna_message_list, the
            // exit notice, deleting it). What only a running session needs
            // goes now — the writer thread and the pty with it, and all but the
            // tail of the scrollback — or a CLI that quit on its own held
            // megabytes and a console host until Luna restarted.
            if let Some(s) = pm.sessions.lock().unwrap_or_else(|e| e.into_inner()).get_mut(&id).filter(|s| Arc::ptr_eq(&s.alive, &alive)) {
                s.input_tx = Arc::new(InputTx::closed(&id));
            }
            // This session's own output, with the session map let go: a look
            // drawing its screen anew holds `drawn` for as long as that takes.
            let_go_of_output(&scrollback, &drawn);
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
                // Timed for the canary here, within the second, not once the
                // output that a process it left may hold open is read out.
                crate::cli::session_exited(&id);
                let pm = manager();
                let mut sessions = pm.sessions.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(s) = sessions.get_mut(&id).filter(|s| Arc::ptr_eq(&s.alive, &alive)) {
                    s.input_tx = Arc::new(InputTx::closed(&id));
                }
                break;
            }
        });
    }

    state.register(
        &id,
        &launching,
        Session {
            provider,
            generation,
            reader_end,
            input_tx,
            killer,
            child,
            scrollback,
            drawn,
            modes,
            sent,
            alive,
            last_output_ms,
            pid,
            cwd,
            spawned_at_ms,
            resume,
            account_path,
            account_dir,
            job,
            size,
            typed_ms: Arc::new(AtomicU64::new(0)),
            draft: Arc::new(AtomicU64::new(0)),
            draft_seen: Arc::new(Mutex::new((0, false))),
            held_keys: Arc::new(Mutex::new(None)),
            stashing: Arc::new(AtomicBool::new(false)),
        },
    )?;
    drop(launching);

    Ok(Attached::fresh(generation, permission))
}

/// The text an output event carries for `carry`, and how many of its bytes
/// that is. A byte that is not UTF-8 goes out as U+FFFD, the way the
/// scrollback replays it to a pane that attaches: stopping at it held back,
/// and then dropped, everything after it in the frame. Only a sequence cut
/// off at the very end waits for the next read.
fn text_of(carry: &[u8]) -> (String, usize) {
    let mut text = String::new();
    let mut rest: &[u8] = carry;
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
    (text, carry.len() - rest.len())
}

/// Hands a piece of `who`'s output on to its emitter, waiting while the
/// channel is full (`OUTPUT_PIECES`): the reader then reads nothing more, and
/// the CLI's writes wait, but nothing is dropped and nothing overtakes. The
/// first wait is logged (`behind`). False once the emitter is gone.
fn hand_on(tx: &std::sync::mpsc::SyncSender<Vec<u8>>, piece: Vec<u8>, behind: &mut bool, who: &str) -> bool {
    match tx.try_send(piece) {
        Err(std::sync::mpsc::TrySendError::Full(piece)) => {
            if !*behind {
                *behind = true;
                crate::log::warn("pty", &format!("{who}'s output is ahead of what goes to the window; reading it waits"));
            }
            tx.send(piece).is_ok()
        }
        sent => sent.is_ok(),
    }
}

/// The reader's word that session `id` of spawn `generation` exited with
/// `code`, said through its `ReaderEnd`: nothing once whoever took the
/// session has stopped waiting for it, nor once the chat's spawn number has
/// moved on (a session spawned under the same id since, after an exit of
/// its own or past a taker's wait, or the session taken and let go). Said
/// under that number's lock, all of it: the registry's exit and the event
/// are in before a start of the chat goes on to its launch, whose
/// registration clears the exit, and so never land on the session after.
pub(super) fn reader_exited(id: &str, generation: u64, code: Option<u32>, end: &ReaderEnd) {
    end.finish(|| exit_word(id, generation, code));
}

/// The reader's word itself, under the spawn numbers' lock: said only while
/// `generation` is still `id`'s.
pub(super) fn exit_word(id: &str, generation: u64, code: Option<u32>) {
    let mut generations = manager().generations.lock().unwrap_or_else(|e| e.into_inner());
    if generations.get(id) == Some(&generation) {
        say_exit(&mut generations, id, generation, code);
    }
}

/// Says that `id`'s session of spawn `generation`, still the one in
/// `generations`, exited with `code`: to the registry and to the window,
/// and once, its number taken away under the same lock.
pub(super) fn say_exit(generations: &mut std::collections::HashMap<String, u64>, id: &str, generation: u64, code: Option<u32>) {
    generations.remove(id);
    crate::agents::session_exited(id, code);
    crate::emit::to_ui("pty://exit", PtyExit { id, code, generation });
    #[cfg(test)]
    said::note(&format!("exit {id}"));
}

/// The child's exit code once it has exited, None if that cannot be read.
/// Asked again and again rather than waited on: a wait holds the child's
/// lock for as long as the process runs, and a shutdown asking whether it
/// has exited (control.rs `shut_down`, before its kill) would wait on that
/// lock for good where the output ended before the process did.
fn exit_code(child: &Mutex<Box<dyn portable_pty::Child + Send + Sync>>) -> Option<u32> {
    loop {
        match child.lock().unwrap_or_else(|e| e.into_inner()).try_wait() {
            Ok(Some(status)) => return Some(status.exit_code()),
            Ok(None) => {}
            Err(_) => return None,
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// What the tests' sessions said, in order, across threads: an exit
/// published, a start's launch built.
#[cfg(test)]
pub(super) mod said {
    use std::sync::Mutex;

    static SAID: Mutex<Vec<String>> = Mutex::new(Vec::new());

    pub(in crate::pty) fn note(what: &str) {
        SAID.lock().unwrap_or_else(|e| e.into_inner()).push(what.to_string());
    }

    /// Where `what` was said, if it was.
    pub(in crate::pty) fn at(what: &str) -> Option<usize> {
        SAID.lock().unwrap_or_else(|e| e.into_inner()).iter().position(|s| s == what)
    }
}

/// The same clean-up for a process started without a pty (background/).
pub(crate) fn clean_env(cmd: &mut Command) {
    for k in removed_env() {
        cmd.env_remove(k);
    }
}

/// The variables a CLI process starts without, though this one has them: the
/// inherited session's (`INHERITED_SESSION_VARS`), and NO_COLOR only when it
/// came with such a session — set by hand, it stays. The fullscreen switch
/// always goes: a core started from a fullscreen Claude chat's shell has it
/// from that chat, and a helper or any other CLI would draw fullscreen on it.
/// A Claude chat is given it again after this (`claude/launch.rs`).
pub(crate) fn removed_env() -> Vec<&'static str> {
    let mut out = INHERITED_SESSION_VARS.to_vec();
    if INHERITED_SESSION_VARS.iter().any(|k| std::env::var_os(k).is_some()) {
        out.push("NO_COLOR");
    }
    out.push(crate::claude::launch::NO_FLICKER_ENV);
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

/// What an exited session keeps of its output: the tail of the scrollback
/// (`EXITED_TAIL`), its screen drawn again from that if anyone looks.
fn let_go_of_output(scrollback: &Mutex<Scrollback>, drawn: &Mutex<Drawn>) {
    {
        let mut sb = scrollback.lock().unwrap_or_else(|e| e.into_inner());
        let excess = sb.bytes.len().saturating_sub(EXITED_TAIL);
        sb.bytes.drain(..excess);
        sb.bytes.shrink_to_fit();
    }
    *drawn.lock().unwrap_or_else(|e| e.into_inner()) = Drawn::default();
}

/// The screen a CLI left as it exited, drawn from the end of its output: what
/// it said as it gave up on a start, for the canary (cli/canary.rs).
fn last_screen(output: &[u8], rows: u16, cols: u16) -> String {
    render_screen(&output[output.len().saturating_sub(LAST_SCREEN)..], rows, cols)
}

/// How much of the end of a CLI's output `last_screen` draws: far more than a
/// screen's worth for a CLI that printed an error and went.
const LAST_SCREEN: usize = 64 * 1024;

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

    /// The output read in pieces of any length and sent in events cut at
    /// any place, with bytes that are not UTF-8 and characters split across
    /// reads among it, and a backlog taken between any two steps: the
    /// events' offsets follow one another with no gap; the backlog is the
    /// text up to its end, and the events from there on, the one across it
    /// cut, the rest of it, nothing twice and nothing missing; its modes are
    /// those of the bytes it holds.
    #[test]
    fn a_backlog_and_the_events_after_it_join_exactly() {
        let output: Vec<u8> = [
            &b"plain \xe2\x80\x94 dash, \x1b[?2004h"[..],
            b"bad \xff byte, cut \xe2\x80",
            b"\x94 across, \x1b[?1h\x1b[?2004l",
            "\u{65e5}\u{672c}".as_bytes(),
            b" end \xf0\x9f",
            b"\x98\x80 \x1b[?1005h!",
        ]
        .concat();
        let whole = String::from_utf8_lossy(&output).into_owned();
        let mut seed = 7u64;
        let mut roll = |n: usize| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) as usize % n
        };
        for _ in 0..300 {
            let sb = Mutex::new(Scrollback::with_capacity(output.len()));
            let sent = Mutex::new(Sent::default());
            let (mut events, mut carry, mut read, mut backlog) = (Vec::<(u64, String)>::new(), Vec::new(), 0, None);
            let take_at = roll(output.len() / 2);
            let mut step = 0;
            while read < output.len() || !carry.is_empty() {
                if read < output.len() && roll(2) == 0 {
                    // The reader keeps a piece, then hands it on.
                    let piece = &output[read..(read + 1 + roll(9)).min(output.len())];
                    let mut s = sb.lock().unwrap();
                    s.bytes.extend(piece.iter().copied());
                    s.written += piece.len() as u64;
                    drop(s);
                    carry.extend_from_slice(piece);
                    read += piece.len();
                } else {
                    let (text, used) = text_of(&carry);
                    let offset = sent.lock().unwrap().took(&carry[..used], text.len());
                    if !text.is_empty() {
                        events.push((offset, text));
                    }
                    carry.drain(..used);
                }
                step += 1;
                if step == take_at {
                    backlog = Some(backlog_at(&sb, &sent));
                }
            }
            let mut at = 0;
            for (offset, text) in &events {
                assert_eq!(*offset, at, "{events:?}");
                at += text.len() as u64;
            }
            assert_eq!(events.iter().map(|(_, t)| t.as_str()).collect::<String>(), whole);
            let Some((bytes, end, modes)) = backlog else { continue };
            let end_at = end as usize;
            assert_eq!(String::from_utf8_lossy(&bytes), whole[..end_at]);
            let after: String =
                events.iter().filter(|(o, t)| *o + t.len() as u64 > end).map(|(o, t)| &t[end.saturating_sub(*o) as usize..]).collect();
            assert_eq!(after, whole[end_at..]);
            let mut from_bytes = Sent::default();
            from_bytes.took(&bytes, 0);
            assert_eq!(modes, from_bytes.modes());
        }
    }

    #[test]
    fn a_session_is_not_started_as_administrator() {
        crate::elevation::refuse_here(Some(crate::elevation::ADMIN));
        let why = crate::elevation::refusal().map(String::from);
        let got = crate::runtime::block_on(spawn("run-as-admin".into(), "acc", None, None, || -> Result<Launch, String> {
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
        let captured = crate::runtime::block_on(spawn("captured-as-admin".into(), "acc", None, None, ok));
        let live = crate::runtime::block_on(spawn("live-as-admin".into(), "acc", None, None, ok));
        crate::elevation::refuse_here(None);
        let _ = capture::take("captured-as-admin");
        assert_eq!(captured, Err("built".into()), "the launch was built, to be captured");
        assert!(live.is_err_and(|e| e != "built"), "refused before it was built");
    }

    /// Output handed on faster than the emitter takes it waits, all of it in
    /// order: nothing is dropped and nothing overtakes; the wait is noted once.
    #[test]
    fn output_ahead_of_the_emitter_waits_in_order() {
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(2);
        let emitter = std::thread::spawn(move || {
            let mut got = Vec::new();
            while let Ok(piece) = rx.recv() {
                std::thread::sleep(std::time::Duration::from_millis(2));
                got.push(piece);
            }
            got
        });
        let mut behind = false;
        for n in 0..50u8 {
            assert!(hand_on(&tx, vec![n], &mut behind, "out-test"));
        }
        drop(tx);
        let got = emitter.join().unwrap();
        assert_eq!(got, (0..50u8).map(|n| vec![n]).collect::<Vec<_>>());
        assert!(behind, "the reader waited");
        let (gone, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(1);
        drop(rx);
        assert!(!hand_on(&gone, vec![1], &mut behind, "out-test"), "an emitter gone ends the reader");
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

    /// An exited session keeps the tail of its output, and its kept screen
    /// goes: drawn again, it is drawn from that tail.
    #[test]
    fn an_exited_sessions_output_is_cut_to_its_tail_and_its_screen_let_go() {
        let scrollback = Mutex::new(Scrollback::with_capacity(16));
        let drawn = Mutex::new(Drawn::default());
        {
            let mut sb = scrollback.lock().unwrap();
            let out = b"x".repeat(EXITED_TAIL * 2);
            sb.bytes.extend(out.iter().copied());
            sb.written = out.len() as u64;
        }
        drawn_screen(&drawn, &scrollback, (5, 20), |_| ());
        assert_eq!(drawn.lock().unwrap().replays(), 1);
        let_go_of_output(&scrollback, &drawn);
        assert_eq!(scrollback.lock().unwrap().bytes.len(), EXITED_TAIL);
        assert_eq!(scrollback.lock().unwrap().written, (EXITED_TAIL * 2) as u64, "the generation goes on");
        assert_eq!(drawn.lock().unwrap().replays(), 0, "the screen kept is gone");
        drawn_screen(&drawn, &scrollback, (5, 20), |_| ());
        assert_eq!(drawn.lock().unwrap().replays(), 1, "and drawn again from the tail");
    }

    #[test]
    fn a_panes_last_screen_shows_the_canary_claude_codes_worktree_error() {
        // As Claude Code 2.1.319 writes it to stderr, red with the folder in
        // bold, into a pane nobody has sized, after more output than is drawn;
        // and as a console host may paint it again, a row at a time, the line
        // broken at the 100th column, mid-word.
        let written = b"\x1b[31mError: Can only use --worktree in a git repository, but \x1b[1mE:\\Projects\\Personal\\luna;C\x1b[22m \
                        is not a git repository. Configure a WorktreeCreate hook to use --worktree with other VCS systems.\x1b[39m\r\n";
        let painted = b"Error: Can only use --worktree in a git repository, but E:\\Projects\\Personal\\luna;C is not a git rep\r\n\
                        ository. Configure a WorktreeCreate hook to use --worktree with other VCS systems.\r\n";
        for said in [&written[..], &painted[..]] {
            let mut output = b"earlier output\r\n".repeat(LAST_SCREEN / 8);
            output.extend_from_slice(said);
            let screen = last_screen(&output, INITIAL_ROWS, INITIAL_COLS);
            let words = crate::cli::LastWords::of_screen(screen.clone());
            assert_eq!(crate::cli::cause_outside(&words), Some("a worktree asked for outside a git repository"), "{screen}");
        }
        let words = crate::cli::LastWords::of_screen(last_screen(b"error: unexpected argument '--x'\r\n", INITIAL_ROWS, INITIAL_COLS));
        assert_eq!(crate::cli::cause_outside(&words), None);
    }

    #[test]
    fn nothing_of_a_codex_session_reaches_one_luna_starts() {
        for k in ["CODEX_SESSION_ID", "CODEX_VERSION", "CODEX_CI", "CODEX_PERMISSION_PROFILE", "CODEX_TOOL_CALL_ID"] {
            assert!(INHERITED_SESSION_VARS.contains(&k), "{k}");
        }
        assert!(!INHERITED_SESSION_VARS.contains(&"CODEX_HOME"), "a person's to set, and Luna sets its own for Codex");
    }

    /// A core started from a fullscreen Claude chat's shell has the chat's
    /// switch; a helper or a model probe started from it must not.
    #[test]
    fn the_fullscreen_switch_is_taken_off_every_cli_process() {
        assert!(removed_env().contains(&"CLAUDE_CODE_NO_FLICKER"));
        let mut cmd = Command::new("claude");
        cmd.env("CLAUDE_CODE_NO_FLICKER", "1");
        clean_env(&mut cmd);
        let got = cmd.get_envs().find(|(k, _)| *k == "CLAUDE_CODE_NO_FLICKER").map(|(_, v)| v.map(|v| v.to_owned()));
        assert_eq!(got, Some(None), "removed from the child's environment, not left set");
    }
}
