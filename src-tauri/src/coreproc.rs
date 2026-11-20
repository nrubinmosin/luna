//! The core: this same exe started as `--core`, with no window, where the
//! sessions live so that the window can come and go without them.
//!
//! The window starts it (corelink.rs) and stays connected; the core outlives
//! a window that crashes, hangs or is killed, and a window started after
//! finds it again through `<data>/core/core.json`, which names its endpoint
//! (endpoint/). Who may connect is the OS's to say (peer.rs): there is no
//! token. One core per data dir — a lock file held for the core's life sees
//! to that, and the OS lets go of it however the core ends. With no window
//! and no session for a while, the core leaves by itself.
//!
//! Everything that runs the sessions runs here: the ptys (pty/), the hooks'
//! listener and the agents' MCP server (hub.rs, agents/), the sessions'
//! records and power (activity/, power/). What they send the page goes to
//! every connected window; what the page asks of them comes in by name
//! (corecmd.rs).

use crate::admit::{Deadline, Slot, Slots};
use crate::endpoint::{Listener, Stream};
use crate::ipc::{self, Discovery, Event, Hello, Req, Res, Welcome};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const FLAG: &str = "--core";
/// How long the core waits with no window and no session before it goes.
const IDLE_EXIT: Duration = Duration::from_secs(10 * 60);
/// Frames queued for one window before it counts as gone: a page that stopped
/// reading must not make a session's output wait. It reconnects and each of
/// its terminals reattaches from the scrollback.
const QUEUE: usize = 8192;
/// The same in bytes: a frame of a session's output, or a reattach's
/// scrollback, can run to megabytes, and `QUEUE` of them is more than a
/// window that stopped reading should have the core hold.
const QUEUE_BYTES: u64 = 64 * 1024 * 1024;
/// Connections let in at once before their hello: only this user's own
/// processes get this far (peer.rs), but one of them stuck, or many at once,
/// gets no more than this. Windows and one-off commands each say hello at
/// once, so this is never full in use.
const STRANGERS: usize = 32;
/// For the whole hello, from the connection on: a window sends it at once,
/// and gives up on a core that has not answered within as long (corelink.rs).
const HELLO_DEADLINE: Duration = Duration::from_secs(5);
/// A hello's frame: a role and a number, far smaller than this.
const HELLO_MAX: usize = 4096;

/// A connected window's outgoing queue, and its end of the connection to
/// close if it falls behind.
struct Window {
    id: u64,
    tx: Outbox,
    stream: Stream,
}

/// A connection's outgoing queue, bounded both in frames (`QUEUE`) and in
/// the bytes not written yet (`QUEUE_BYTES`); past either it is full, as a
/// window that stopped reading. One frame alone may be larger than the bound.
#[derive(Clone)]
struct Outbox {
    tx: SyncSender<Vec<u8>>,
    /// Bytes queued and not written; the connection's writer gives back
    /// what it has written.
    waiting: Arc<crate::backlog::Backlog>,
}

impl Outbox {
    /// A queue of `frames` and the end its writer reads.
    fn new(frames: usize) -> (Outbox, std::sync::mpsc::Receiver<Vec<u8>>) {
        let (tx, rx) = sync_channel(frames);
        (Outbox { tx, waiting: Arc::default() }, rx)
    }

    fn try_send(&self, frame: Vec<u8>) -> Result<(), TrySendError<Vec<u8>>> {
        let len = frame.len() as u64;
        if self.waiting.reserve(len, QUEUE_BYTES).is_err() {
            return Err(TrySendError::Full(frame));
        }
        self.tx.try_send(frame).inspect_err(|_| self.waiting.release(len))
    }
}

static WINDOWS: Mutex<Vec<Window>> = Mutex::new(Vec::new());
/// Events no window was there to take that the next one must still get: a
/// chat an agent deleted would otherwise come back with the next window and
/// resume, and a helper asked for would never be made.
static HELD: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());
const KEPT_FOR_A_WINDOW: [&str; 2] = ["agent://deleted", "agent://spawn"];
/// Since when nothing has kept the core: no window, no session. Epoch ms.
static ALONE_SINCE: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// `luna --core --data-dir <dir> --log <path> --window-exe <path>`: runs as the
/// core and returns true, or false for an ordinary launch.
pub fn from_args() -> bool {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some(FLAG) {
        return false;
    }
    let value = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).map(PathBuf::from);
    let (Some(data), Some(log)) = (value("--data-dir"), value("--log")) else {
        return true;
    };
    crate::paths::set_data_dir(data);
    if let Some(exe) = value("--window-exe") {
        crate::power::set_window_exe(exe);
    }
    crate::log::set_path(log);
    crate::log::set_role("core");
    crate::log::install_panic_hook();
    run();
    true
}

/// `luna --core-fingerprint <file>`: writes this build's core fingerprint to
/// the file and returns true; an exe built for the windows subsystem has no
/// console to print to. The release publishes the fingerprint with the update,
/// so a window can tell an update that leaves the core as it is, and reads it
/// from the file build.rs leaves beside the exe; on Windows,
/// scripts/release-bundle.ps1 asks the exe too and stops if the two differ.
pub fn fingerprint_from_args() -> bool {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("--core-fingerprint") {
        return false;
    }
    if let Some(file) = args.get(1) {
        let _ = write_fingerprint(Path::new(file));
    }
    true
}

fn write_fingerprint(file: &Path) -> io::Result<()> {
    std::fs::write(file, env!("LUNA_CORE_FP"))
}

/// `luna --stop-core`: has the running core stop — its sessions quit first —
/// and returns true once it is gone. For the uninstaller, which cannot remove
/// the copy of the exe a running core holds. A core still running after it
/// (one held for an update refuses, `requests`) ends the process with
/// `STILL_RUNNING`: the uninstaller then leaves the core's folder, whose
/// `core.json` is how a window finds that core.
pub fn stop_from_args() -> bool {
    if std::env::args().nth(1).as_deref() != Some("--stop-core") {
        return false;
    }
    if !crate::corelink::shut_down() {
        std::process::exit(STILL_RUNNING);
    }
    true
}

/// `--stop-core`'s exit code where a core still runs (windows/hooks.nsh).
const STILL_RUNNING: i32 = 3;

fn run() {
    // Before anything is written there. A failure is logged and shown in
    // Settings' checks; the token folders below are made private regardless.
    let _ = crate::paths::protect_data_dir();
    let dir = ipc::core_dir();
    // Owner-only: off Windows the core's socket is in it, and the folder's
    // mode is what keeps other users from reaching it.
    if let Err(e) = crate::secret::private_dir(&dir) {
        crate::log::error("core", &format!("cannot create {}: {e}", dir.display()));
        return;
    }
    // Held for the life of the process; a second core started meanwhile finds
    // it taken and leaves the first one be.
    let Ok(lock) = std::fs::File::options().create(true).truncate(false).write(true).open(dir.join("core.lock")) else {
        crate::log::error("core", "cannot open core.lock");
        return;
    };
    if lock.try_lock().is_err() {
        crate::log::info("core", "another core already runs for this data dir");
        return;
    }

    let started = now_ms();
    crate::log::start(&format!(
        "starting luna core {} build {} core {}",
        env!("CARGO_PKG_VERSION"),
        env!("LUNA_BUILD"),
        env!("LUNA_CORE_FP")
    ));
    // Run as administrator, the core still answers, so that a window hears
    // why, but starts nothing: every request but `info` and `shutdown` is
    // refused (`requests`), and nothing below that would start a session or
    // helper of its own accord runs.
    let refused = crate::elevation::refusal();
    if let Some(why) = refused {
        crate::log::error("core", &format!("{why} The core starts no session or helper."));
    }
    crate::lifecycle::start_core();
    sweep_old_copies();
    keep_full_speed();
    crate::peer::note_launch();

    // Sockets a core before this one left behind, gone with it.
    #[cfg(unix)]
    crate::endpoint::sweep_stale();
    let endpoint = crate::endpoint::fresh_name();
    let listener = match Listener::bind(&endpoint) {
        Ok(l) => l,
        Err(e) => {
            crate::log::error("core", &format!("cannot listen: {e}"));
            return;
        }
    };

    // What the sessions send the page goes to every window there is.
    crate::emit::install(broadcast);
    if refused.is_none() {
        // A CLI rollback the last core went away in the middle of, finished
        // before any session starts on the version it was rolling back from.
        crate::cli::finish_rollbacks();
        // Here and not in the window: session_meta, which turns a Claude
        // transcript's tokens into a share of the window, runs in this process.
        crate::claude::context::refresh_soon();
        // The helpers the last core left and what it kept of the chats, before a
        // window or an MCP call can ask after them.
        // An update's stop the last core of this build left standing, first:
        // nothing it restores may start while it stands.
        crate::agents::restore_update_stop();
        crate::agents::restore_chats();
        crate::background::restore();
        crate::agents::take_up_spawns();
        crate::library::note_retired_system();
        // The hook listener before any session can be spawned with hooks
        // pointing at it; the records' reads feed power/ from then on.
        crate::hub::start();
        crate::agents::init();
        crate::agents::register_dev_caller();
        crate::background::resume_stopped();
        crate::activity::start();
        crate::pty::prune_session_files_periodically();
        crate::media::prune_periodically();
    }

    let found = Discovery {
        pid: std::process::id(),
        created_ms: crate::procs::created_ms(std::process::id()).unwrap_or(0),
        proto: ipc::PROTO,
        endpoint: endpoint.clone(),
    };
    if let Err(e) = write_discovery(&found) {
        crate::log::error("core", &format!("cannot write core.json: {e}"));
        return;
    }
    #[cfg(unix)]
    let _ = ENDPOINT.set(crate::endpoint::socket_path(&endpoint));

    ALONE_SINCE.store(started, Ordering::SeqCst);
    std::thread::spawn(leave_when_alone);
    let strangers = Slots::new(STRANGERS);
    loop {
        let stream = match listener.accept() {
            Ok(stream) => stream,
            Err(e) => {
                // Not a client's doing: the OS refused the next instance. Said
                // once a second at most, and tried again.
                crate::log::warn("core", &format!("cannot accept on the endpoint: {e}"));
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        };
        // Past the cap the connection is closed at once, with nothing said.
        let Some(slot) = strangers.take() else { continue };
        std::thread::spawn(move || {
            if let Err(e) = serve(stream, slot, started) {
                crate::log::info("core", &format!("connection ended: {e}"));
            }
        });
    }
}

/// The socket file to take along on the way out, off Windows, where a pipe
/// goes with its process.
#[cfg(unix)]
static ENDPOINT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Through a temporary file, so a window never reads half of it. Private to
/// this user like everything in the core folder, though it holds nothing that
/// lets anyone in.
fn write_discovery(d: &Discovery) -> io::Result<()> {
    crate::secret::write_private(&ipc::discovery_file(), &serde_json::to_vec(d)?)
}

/// Leaves, taking `core.json` along so no window tries the dead endpoint.
/// Every session is asked to quit first, as a chat delete would, so each
/// leaves its transcript whole; a background helper's turn has no quit to be
/// asked and is stopped, its process tree with it, so nothing runs on
/// unowned.
#[cfg(not(test))]
fn leave(reason: &str) -> ! {
    crate::lifecycle::exiting(reason);
    let _ = std::fs::remove_file(ipc::discovery_file());
    #[cfg(unix)]
    if let Some(socket) = ENDPOINT.get() {
        let _ = std::fs::remove_file(socket);
    }
    wind_down(crate::agents::Stop::Leaving);
    crate::lifecycle::end_process()
}

/// In a test, the core's leaving would end the test run: it panics instead,
/// which the request tests catch (`asked`).
#[cfg(test)]
fn leave(reason: &str) -> ! {
    panic!("{LEFT}: {reason}")
}

#[cfg(test)]
const LEFT: &str = "the core left";

/// Stops every session so that the next core takes them up where they stood: what
/// a core leaving does, and a shutdown or restart of the PC. The chats' ties stay
/// frozen until `why` is called off (`agents::thaw_chats`), or the process ends.
/// Returns how many chats were closed.
pub fn wind_down(why: crate::agents::Stop) -> usize {
    crate::background::stop_all();
    crate::agents::freeze_chats(why);
    crate::pty::manager().shut_down_all(crate::pty::GRACE)
}

fn leave_when_alone() {
    loop {
        std::thread::sleep(Duration::from_secs(30));
        let kept = !WINDOWS.lock().unwrap_or_else(|e| e.into_inner()).is_empty()
            || crate::pty::manager().live_count() > 0
            || crate::background::running() > 0;
        if kept {
            ALONE_SINCE.store(now_ms(), Ordering::SeqCst);
            continue;
        }
        if now_ms().saturating_sub(ALONE_SINCE.load(Ordering::SeqCst)) >= IDLE_EXIT.as_millis() as u64 {
            leave("no window and no session for ten minutes");
        }
    }
}

/// The copies of the exe older builds of the core ran from (corelink.rs).
/// One still running cannot be deleted, and stays until a later sweep.
fn sweep_old_copies() {
    let Ok(entries) = std::fs::read_dir(ipc::core_dir()) else { return };
    let mine = env!("LUNA_CORE_FP");
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let is_copy = name.len() == 16 && name.chars().all(|c| c.is_ascii_hexdigit());
        if is_copy && name != mine && e.path().is_dir() {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// Windows puts a process with no window of its own on efficiency mode when
/// it decides to — slower timers, lower clocks — and the sessions' output
/// would crawl with it. Opted out for the core's life.
#[cfg(windows)]
fn keep_full_speed() {
    use windows::Win32::System::Threading::{
        GetCurrentProcess, ProcessPowerThrottling, SetProcessInformation, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION, PROCESS_POWER_THROTTLING_STATE,
    };
    let state = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED | PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION,
        StateMask: 0,
    };
    // SAFETY: the current process's pseudo-handle and a struct of the size
    // passed.
    let set = unsafe {
        SetProcessInformation(
            GetCurrentProcess(),
            ProcessPowerThrottling,
            &state as *const _ as *const std::ffi::c_void,
            size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        )
    };
    if let Err(e) = set {
        crate::log::warn("core", &format!("could not opt out of efficiency mode: {e}"));
    }
}

#[cfg(not(windows))]
fn keep_full_speed() {}

/// The core's `emit::to_ui`: the event goes into every window's queue. A
/// window whose queue is full has stopped reading; it is let go rather than
/// waited for.
fn broadcast(event: &str, payload: serde_json::Value) {
    let Ok(frame) = ipc::encode(ipc::EVENT, &Event { event: event.to_string(), payload }) else { return };
    let mut windows = WINDOWS.lock().unwrap_or_else(|e| e.into_inner());
    if windows.is_empty() && KEPT_FOR_A_WINDOW.contains(&event) {
        HELD.lock().unwrap_or_else(|e| e.into_inner()).push(frame);
        return;
    }
    windows.retain(|w| match w.tx.try_send(frame.clone()) {
        Ok(()) => true,
        Err(TrySendError::Full(_)) => {
            crate::log::warn("core", "a window fell behind on the sessions' output; letting it go");
            w.stream.shutdown();
            false
        }
        Err(TrySendError::Disconnected(_)) => false,
    });
}

/// The first frame of a connection, which must be a hello: read within
/// `within` and `HELLO_MAX` bytes, the stranger's slot held until then.
/// Returns the request's id and the hello.
fn hello(stream: &Stream, _slot: Slot, within: Duration) -> io::Result<(u64, Hello)> {
    let (kind, payload) = ipc::read_frame_within(&mut Deadline::new(stream, within), HELLO_MAX)?;
    stream.set_read_timeout(None)?;
    let first: Option<Req> = (kind == ipc::REQ).then(|| serde_json::from_slice(&payload).ok()).flatten();
    first
        .filter(|r| r.cmd == "hello")
        .and_then(|r| Some((r.id, serde_json::from_value::<Hello>(r.args).ok()?)))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "no valid hello"))
}

/// One connection: who opened it, the hello, then requests until the other
/// end goes.
fn serve(mut stream: Stream, slot: Slot, started: u64) -> io::Result<()> {
    // Who opened the endpoint decides, before anything is read: this user's
    // own processes, unconfined, at the core's integrity (peer.rs, threat
    // model F3). Fail closed, and say nothing back. A core run as
    // administrator, which does no work, lets an ordinary window in below its
    // integrity to stop it and nothing else (`stop_only`): that window has
    // no other way to.
    let (stop_only, peer) = match crate::peer::admission(&stream, crate::elevation::refusal().is_some()) {
        Ok(admitted) => (admitted.stop_only, admitted.pid),
        Err(why) => {
            crate::log::warn("core", &format!("refused a connection to the core: {why}"));
            stream.shutdown();
            return Ok(());
        }
    };
    let (first_id, hello) = hello(&stream, slot, HELLO_DEADLINE)?;
    if hello.proto != ipc::PROTO {
        let err = format!("protocol {} spoken here, {} asked for", ipc::PROTO, hello.proto);
        ipc::send(&mut stream, ipc::RES, &Res { id: first_id, ok: None, err: Some(err) })?;
        return Ok(());
    }
    // The caller, by its process: an update's stop stands against a
    // `shutdown` from any but the window that asked for it (`requests`).
    let caller = (peer != 0).then(|| crate::agents::Window::of(peer));
    // The window: what it says of its build may call off an update's stop
    // that no install followed (agents `window_came`), and its asking for
    // that stop is noted (`requests`).
    let window = caller.filter(|_| hello.role == "ui" && !stop_only);
    if let Some(window) = window {
        crate::agents::window_came(window, hello.fingerprint.as_deref());
    }
    let welcome = serde_json::to_value(Welcome::of_this_process(started)).ok();
    ipc::send(&mut stream, ipc::RES, &Res { id: first_id, ok: welcome, err: None })?;

    // Everything this connection is sent from here on goes through its queue
    // and one writer thread, so answers and events never interleave mid-frame.
    let (tx, rx) = Outbox::new(QUEUE);
    let mut out = stream.try_clone()?;
    let waiting = Arc::clone(&tx.waiting);
    std::thread::spawn(move || {
        for frame in rx {
            if out.write_all(&frame).and_then(|()| out.flush()).is_err() {
                break;
            }
            waiting.release(frame.len() as u64);
        }
        out.shutdown();
    });

    static NEXT: AtomicU64 = AtomicU64::new(1);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    // One let in only to stop the core hears none of its events either.
    if hello.role == "ui" && !stop_only {
        let mut windows = WINDOWS.lock().unwrap_or_else(|e| e.into_inner());
        // Under the windows' lock, so nothing held can slip past in between.
        for frame in HELD.lock().unwrap_or_else(|e| e.into_inner()).drain(..) {
            let _ = tx.try_send(frame);
        }
        windows.push(Window { id, tx: tx.clone(), stream: stream.try_clone()? });
    }
    let replies = Replies { tx, stream: Arc::new(stream.try_clone()?) };
    let newer = crate::agents::newer_build(hello.fingerprint.as_deref(), hello.version.as_deref());
    let caller = Caller { process: caller, window, newer };
    let result = requests(&mut stream, &replies, started, stop_only, &caller);
    WINDOWS.lock().unwrap_or_else(|e| e.into_inner()).retain(|w| w.id != id);
    result
}

/// Runs the real `serve` on one accepted connection, for the live tests of
/// the peer check (`live.rs`): the test owns the listener, so removing the
/// check from `serve` is seen there. A refused peer returns `Ok(())` before
/// anything is read; one let in goes on to the hello.
#[cfg(test)]
pub(crate) fn serve_for_test(stream: Stream) -> io::Result<()> {
    let slots = Slots::new(STRANGERS);
    serve(stream, slots.take().expect("a slot"), now_ms())
}

/// Where a connection's answers go: its queue, and its end of the connection,
/// closed when an answer cannot be queued so the requests waiting on it fail
/// rather than wait for good.
#[derive(Clone)]
struct Replies {
    tx: Outbox,
    stream: Arc<Stream>,
}

/// Said to a connection let in only to stop the core (`serve`).
const STOP_ONLY: &str = "this connection may only stop the core: it runs as administrator, above the caller";

/// Why `cmd` is not answered on this connection, if it is not. A core run as
/// administrator says who it is and stops, nothing more; to a connection let
/// in below its integrity (`stop_only`) it only stops.
fn turned_away(cmd: &str, stop_only: bool) -> Option<String> {
    if stop_only {
        return (cmd != "shutdown").then(|| STOP_ONLY.to_string());
    }
    crate::elevation::refusal().filter(|_| !matches!(cmd, "info" | "shutdown")).map(String::from)
}

/// Why a core is not to leave on a window's word that it holds only what
/// helpers' turns left (corelink.rs `Verdict::ReplaceLeftOnly`), its hello's
/// count being a look some time ago: a pane, or a helper's turn started since.
const AT_WORK: &str = "the core has a session or a helper in a turn now";

/// A shutdown asked `ifLeftOnly`: looked at again as it is to be made, helper
/// turns closed off under their registry's lock as they are looked at, so none
/// starts between this look and the leaving (`background::close_if_left_only`);
/// panes before and after it, as one opened meanwhile is work too.
fn left_only_now() -> Result<(), String> {
    left_only_given(|| crate::pty::manager().live_count() > 0, crate::background::close_if_left_only, crate::background::Closed::reopen)
}

/// `left_only_now`, with what it looks at and does given.
fn left_only_given<T>(panes: impl Fn() -> bool, close: impl FnOnce() -> Option<T>, reopen: impl FnOnce(T)) -> Result<(), String> {
    if panes() {
        return Err(AT_WORK.into());
    }
    let Some(closed) = close() else { return Err(AT_WORK.into()) };
    if panes() {
        reopen(closed);
        return Err(AT_WORK.into());
    }
    Ok(())
}

/// Who is asking on a connection (`serve`): its process, the window it is
/// if a window's, and whether its hello names a newer build than this core's
/// (`agents::newer_build`).
struct Caller {
    process: Option<crate::agents::Window>,
    window: Option<crate::agents::Window>,
    newer: bool,
}

/// Why `cmd` is not taken in a frame of `kind`, if it is not: every command
/// comes on its own lane alone (`corecmd::COMMANDS`; the protocol's `info`
/// and `shutdown` are calls), so a call sent unanswered never runs on the
/// connection's reader thread, holding up its next frames, and a keystroke
/// is never answered.
fn off_lane(cmd: &str, kind: u8) -> Option<String> {
    use crate::corecmd::Lane;
    let on = if kind == ipc::SEND { Lane::Send } else { Lane::Call };
    let lane = if matches!(cmd, "info" | "shutdown") { Some(Lane::Call) } else { crate::corecmd::lane(cmd) };
    match lane {
        Some(lane) if lane == on => None,
        Some(Lane::Call) => Some(format!("{cmd} is a call, not taken unanswered")),
        Some(Lane::Send) => Some(format!("{cmd} is sent unanswered, not taken as a call")),
        None => Some(format!("the core has no command {cmd}")),
    }
}

fn requests(stream: &mut Stream, replies: &Replies, started: u64, stop_only: bool, caller: &Caller) -> io::Result<()> {
    // A frame dropped for its lane is logged once a connection, not once a
    // frame: the next would only say the same.
    let mut off_lane_logged = false;
    loop {
        let (kind, payload) = ipc::read_frame(stream)?;
        let req: Req = serde_json::from_slice(&payload).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        if let Some(why) = off_lane(&req.cmd, kind) {
            if kind != ipc::SEND {
                answer(replies, req.id, Err(why));
            } else if !std::mem::replace(&mut off_lane_logged, true) {
                crate::log::warn("core", &format!("a frame sent unanswered was dropped: {why}"));
            }
            continue;
        }
        if let Some(why) = turned_away(&req.cmd, stop_only) {
            if kind != ipc::SEND {
                answer(replies, req.id, Err(why));
            }
            continue;
        }
        match req.cmd.as_str() {
            "info" => answer(replies, req.id, serde_json::to_value(Welcome::of_this_process(started)).map_err(|e| e.to_string())),
            "shutdown" => {
                // Answered, as anything refused is, and the connection kept:
                // a window's Quit hears it at once and goes on without the
                // core (corelink.rs `stop_as`). Admitted, it is held until the
                // process ends: no update's stop begins meanwhile.
                let leaving = match crate::agents::admit_shutdown(caller.process, caller.newer) {
                    Ok(leaving) => leaving,
                    Err(why) => {
                        crate::log::warn("core", &format!("a shutdown was refused: {why}"));
                        answer(replies, req.id, Err(why.into()));
                        continue;
                    }
                };
                if req.args.get("ifLeftOnly").and_then(serde_json::Value::as_bool) == Some(true) {
                    if let Err(why) = left_only_now() {
                        drop(leaving);
                        answer(replies, req.id, Err(why));
                        continue;
                    }
                }
                answer(replies, req.id, Ok(serde_json::Value::Null));
                leave("the window asked it to stop");
            }
            cmd if kind == ipc::SEND => {
                // Nobody waits for the answer to a keystroke.
                let _ = crate::corecmd::dispatch(cmd, req.args);
            }
            cmd if crate::corecmd::in_order(cmd) => answer(replies, req.id, crate::corecmd::dispatch(cmd, req.args)),
            _ => {
                // The window an update's stop is asked on is its asker, if
                // that stop freezes the chats (`agents::freeze_for_update`).
                let (replies, asker) = (replies.clone(), caller.window);
                std::thread::spawn(move || answer(&replies, req.id, crate::corecmd::dispatch_as(&req.cmd, req.args, asker)));
            }
        }
    }
}

fn answer(replies: &Replies, id: u64, result: Result<serde_json::Value, String>) {
    let res = match result {
        Ok(ok) => Res { id, ok: Some(ok), err: None },
        Err(err) => Res { id, ok: None, err: Some(err) },
    };
    // An answer too large for a frame is still an answer: the error says why.
    let frame = ipc::encode(ipc::RES, &res)
        .or_else(|e| ipc::encode(ipc::RES, &Res { id, ok: None, err: Some(format!("the answer could not be sent: {e}")) }));
    // A full queue is a window that stopped reading. The answer is lost, and
    // so is the connection: the window hears it end, which fails every call
    // still waiting on it. A queue nobody reads any more is one whose writer
    // has closed the connection already.
    match frame.map(|f| replies.tx.try_send(f)) {
        Ok(Ok(())) | Ok(Err(TrySendError::Disconnected(_))) => {}
        Ok(Err(TrySendError::Full(_))) | Err(_) => {
            crate::log::warn("core", &format!("could not answer request {id}; closing the connection"));
            replies.stream.shutdown();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    /// A shutdown asked `ifLeftOnly` looks again: a pane before the helpers'
    /// look refuses it untouched, a helper in a turn refuses it, and a pane
    /// opened while the helpers were closed off refuses it and opens them
    /// again. Only with neither does the core leave, its helpers closed.
    #[test]
    fn a_shutdown_if_left_only_looks_again_and_refuses_work() {
        use std::cell::Cell;
        let (closes, reopens) = (Cell::new(0), Cell::new(0));
        let run = |panes: Vec<bool>, at_work: bool| {
            let panes = std::cell::RefCell::new(panes.into_iter());
            left_only_given(
                || panes.borrow_mut().next().unwrap_or(false),
                || {
                    closes.set(closes.get() + 1);
                    (!at_work).then_some(())
                },
                |()| reopens.set(reopens.get() + 1),
            )
        };
        assert_eq!(run(vec![true], false), Err(AT_WORK.into()), "a pane");
        assert_eq!((closes.get(), reopens.get()), (0, 0), "nothing closed for it");
        assert_eq!(run(vec![false], true), Err(AT_WORK.into()), "a helper in a turn");
        assert_eq!((closes.get(), reopens.get()), (1, 0));
        assert_eq!(run(vec![false, true], false), Err(AT_WORK.into()), "a pane opened meanwhile");
        assert_eq!((closes.get(), reopens.get()), (2, 1), "opened again");
        assert_eq!(run(vec![false, false], false), Ok(()));
        assert_eq!((closes.get(), reopens.get()), (3, 1), "closed, to leave");
    }

    /// A window's queue is full past its bytes as well as past its frames;
    /// what the writer has written makes room again.
    #[test]
    fn a_window_queue_is_full_past_its_bytes() {
        let (tx, rx) = Outbox::new(QUEUE);
        let half = (QUEUE_BYTES / 2) as usize;
        assert!(tx.try_send(vec![0; half]).is_ok());
        assert!(tx.try_send(vec![0; half]).is_ok());
        assert!(matches!(tx.try_send(vec![0; 1]), Err(TrySendError::Full(_))));
        let written = rx.recv().unwrap();
        tx.waiting.release(written.len() as u64);
        assert!(tx.try_send(vec![0; 1]).is_ok());
        let (alone, _rx) = Outbox::new(QUEUE);
        assert!(alone.try_send(vec![0; QUEUE_BYTES as usize + 1]).is_ok(), "one frame alone may be larger");
    }

    #[test]
    fn with_no_window_only_what_must_not_be_missed_is_held() {
        broadcast("pty://output", serde_json::json!({ "id": "x", "data": "hi" }));
        broadcast("agent://deleted", serde_json::json!({ "id": "x" }));
        let held = HELD.lock().unwrap().len();
        assert_eq!(held, 1, "a deleted chat waits for the next window; output does not");
        broadcast("agent://spawn", serde_json::json!({ "id": "y" }));
        assert_eq!(HELD.lock().unwrap().len(), 2, "and so does a spawn request");
        HELD.lock().unwrap().clear();
    }

    /// `hello` on the core's end of a fresh connection to the endpoint, the
    /// client side of which `client` drives; what `hello` made of it, and how
    /// long that took.
    fn hello_from(within: Duration, client: impl FnOnce(Stream) + Send + 'static) -> (io::Result<(u64, Hello)>, Duration) {
        let (_listener, c, server) = crate::endpoint::tests::pair();
        let peer = std::thread::spawn(move || client(c));
        let slots = Slots::new(1);
        let begun = std::time::Instant::now();
        let got = hello(&server, slots.take().unwrap(), within);
        let took = begun.elapsed();
        assert_eq!(slots.taken(), 0, "the stranger's slot is given back");
        drop(server);
        let _ = peer.join();
        (got, took)
    }

    fn hello_req() -> Req {
        let args = serde_json::json!({ "proto": ipc::PROTO, "role": "ctl" });
        Req { id: 7, cmd: "hello".into(), args }
    }

    #[test]
    fn a_hello_is_let_in() {
        let (got, _) = hello_from(HELLO_DEADLINE, |mut c| ipc::send(&mut c, ipc::REQ, &hello_req()).unwrap());
        let (id, h) = got.unwrap();
        assert_eq!((id, h.role.as_str(), h.proto), (7, "ctl", ipc::PROTO));
    }

    #[test]
    fn anything_but_a_hello_first_gets_nothing() {
        let (got, _) = hello_from(HELLO_DEADLINE, |mut c| {
            ipc::send(&mut c, ipc::REQ, &Req { id: 1, cmd: "info".into(), args: serde_json::json!({}) }).unwrap();
            // Nothing comes back before the core closes the connection.
            c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let mut out = Vec::new();
            let _ = c.read_to_end(&mut out);
            assert!(out.is_empty());
        });
        assert_eq!(got.err().unwrap().kind(), io::ErrorKind::InvalidData, "a request before the hello");
        let (got, _) = hello_from(HELLO_DEADLINE, |mut c| {
            ipc::send(&mut c, ipc::REQ, &Req { id: 1, cmd: "hello".into(), args: serde_json::json!({ "role": 3 }) }).unwrap()
        });
        assert_eq!(got.err().unwrap().kind(), io::ErrorKind::InvalidData, "a hello of the wrong shape");
        let (got, _) = hello_from(HELLO_DEADLINE, |mut c| ipc::send(&mut c, ipc::EVENT, &hello_req()).unwrap());
        assert_eq!(got.err().unwrap().kind(), io::ErrorKind::InvalidData, "a hello in a frame of the wrong kind");
    }

    #[test]
    fn an_oversized_or_truncated_hello_is_refused() {
        // A frame claiming a megabyte: refused on its length, before any of it is read.
        let (got, took) = hello_from(HELLO_DEADLINE, |mut c| {
            c.write_all(&(1u32 << 20).to_le_bytes()).unwrap();
            c.write_all(&[ipc::REQ]).unwrap();
            std::thread::sleep(Duration::from_millis(200));
        });
        assert_eq!(got.err().unwrap().kind(), io::ErrorKind::InvalidData);
        assert!(took < Duration::from_secs(2), "{took:?}");
        // Half a frame, then the connection closed.
        let (got, _) = hello_from(HELLO_DEADLINE, |mut c| {
            let frame = ipc::encode(ipc::REQ, &hello_req()).unwrap();
            c.write_all(&frame[..frame.len() / 2]).unwrap();
        });
        assert_eq!(got.err().unwrap().kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn a_silent_or_trickling_stranger_is_let_go_at_the_deadline() {
        let within = Duration::from_millis(300);
        let (got, took) = hello_from(within, |_c| std::thread::sleep(Duration::from_millis(1500)));
        assert!(got.is_err());
        assert!(took < Duration::from_secs(1), "{took:?}");
        let (got, took) = hello_from(within, |mut c| {
            let frame = ipc::encode(ipc::REQ, &hello_req()).unwrap();
            for b in frame {
                if c.write_all(&[b]).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        assert!(got.is_err());
        assert!(took < Duration::from_secs(1), "{took:?}");
    }

    /// The whole of `serve` for a plain client of this user's: let in, its
    /// hello answered with who the core is. A hello of another protocol (a
    /// window of Luna 0.11, if one got this far) is told so and let go.
    #[test]
    fn serve_welcomes_a_hello_and_names_a_protocol_it_does_not_speak() {
        for (proto, welcomed) in [(ipc::PROTO, true), (1, false)] {
            let (_listener, mut c, server) = crate::endpoint::tests::pair();
            let core = std::thread::spawn(move || serve_for_test(server));
            let args = serde_json::json!({ "proto": proto, "role": "ctl" });
            ipc::send(&mut c, ipc::REQ, &Req { id: 3, cmd: "hello".into(), args }).unwrap();
            c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let res: Res = ipc::recv(&mut c, ipc::RES).expect("an answer");
            assert_eq!(res.id, 3);
            if welcomed {
                let core: Welcome = serde_json::from_value(res.ok.expect("welcomed")).unwrap();
                assert_eq!(core.pid, std::process::id());
            } else {
                assert!(res.err.is_some_and(|e| e.contains("protocol 2 spoken here, 1 asked for")));
            }
            drop(c);
            let _ = core.join();
        }
    }

    /// What `requests` answers `cmd` with, in a core whose refusal
    /// (elevation.rs) is `why`.
    fn answered(why: Option<&'static str>, cmd: &str) -> Res {
        answered_on(why, cmd, false)
    }

    /// The same on a connection let in only to stop the core when `stop_only`.
    fn answered_on(why: Option<&'static str>, cmd: &str, stop_only: bool) -> Res {
        let (_listener, mut c, mut stream) = crate::endpoint::tests::pair();
        let cmd = cmd.to_string();
        // The request, then the end of the connection, which ends `requests`.
        let peer = std::thread::spawn(move || {
            ipc::send(&mut c, ipc::REQ, &Req { id: 9, cmd, args: serde_json::Value::Null }).unwrap();
            std::thread::sleep(Duration::from_millis(200));
        });
        let (tx, rx) = Outbox::new(4);
        let replies = Replies { tx, stream: Arc::new(stream.try_clone().unwrap()) };
        crate::elevation::refuse_here(why);
        let caller = Caller { process: None, window: None, newer: false };
        let _ = requests(&mut stream, &replies, 0, stop_only, &caller);
        crate::elevation::refuse_here(None);
        peer.join().unwrap();
        let frame = rx.recv_timeout(Duration::from_secs(10)).expect("an answer");
        let (_, payload) = ipc::read_frame(&mut io::Cursor::new(frame)).unwrap();
        serde_json::from_slice(&payload).unwrap()
    }

    /// Every command is taken on its own lane and on no other: the window's
    /// as `corecmd::COMMANDS` has them, the protocol's `info` and `shutdown`
    /// as calls, and one the core does not have on neither.
    #[test]
    fn each_command_is_taken_on_its_own_lane_alone() {
        use crate::corecmd::{Lane, COMMANDS};
        let kind = |lane: Lane| if lane == Lane::Send { ipc::SEND } else { ipc::REQ };
        let other = |lane: Lane| if lane == Lane::Send { ipc::REQ } else { ipc::SEND };
        for &(cmd, lane) in COMMANDS.iter().chain([("info", Lane::Call), ("shutdown", Lane::Call)].iter()) {
            assert_eq!(off_lane(cmd, kind(lane)), None, "{cmd} on its lane");
            assert!(off_lane(cmd, other(lane)).is_some(), "{cmd} on the other lane");
        }
        for k in [ipc::REQ, ipc::SEND] {
            assert!(off_lane("nope", k).is_some_and(|why| why.contains("no command")));
            assert!(off_lane("hello", k).is_some(), "the hello comes first, and once");
        }
    }

    /// On a connection, a call sent unanswered and the protocol's `shutdown`
    /// sent so are dropped, never run (a shutdown run would leave, which
    /// panics in a test), and a keystroke asked as a call is answered with
    /// why: the connection's next frames are read and answered as ever.
    #[test]
    fn a_frame_on_the_wrong_lane_is_dropped_or_answered_with_why() {
        let (_listener, mut c, mut stream) = crate::endpoint::tests::pair();
        let peer = std::thread::spawn(move || {
            let req = |id: u64, cmd: &str| Req { id, cmd: cmd.into(), args: serde_json::Value::Null };
            ipc::send(&mut c, ipc::SEND, &req(0, "kill_session")).unwrap();
            ipc::send(&mut c, ipc::SEND, &req(0, "shutdown")).unwrap();
            ipc::send(&mut c, ipc::REQ, &req(8, "write_session")).unwrap();
            ipc::send(&mut c, ipc::REQ, &req(9, "session_modes")).unwrap();
            std::thread::sleep(Duration::from_millis(200));
        });
        let (tx, rx) = Outbox::new(8);
        let replies = Replies { tx, stream: Arc::new(stream.try_clone().unwrap()) };
        let caller = Caller { process: None, window: None, newer: false };
        let _ = requests(&mut stream, &replies, 0, false, &caller);
        peer.join().unwrap();
        let mut answers = Vec::new();
        while let Ok(frame) = rx.recv_timeout(Duration::from_millis(500)) {
            let (_, payload) = ipc::read_frame(&mut io::Cursor::new(frame)).unwrap();
            answers.push(serde_json::from_slice::<Res>(&payload).unwrap());
        }
        let ids: Vec<u64> = answers.iter().map(|r| r.id).collect();
        assert_eq!(ids, [8, 9], "the frames sent unanswered got none");
        assert!(answers[0].err.as_deref().is_some_and(|e| e.contains("not taken as a call")), "{:?}", answers[0].err);
        // Turned away by the elevated core's own rule where the tests run
        // elevated, else read as a call: either way answered.
        assert!(answers[1].err.as_deref().is_none_or(|e| e.contains("bad arguments") || crate::elevation::refusal().is_some()));
    }

    /// What `requests` answers a `shutdown` asked by `caller` with, and
    /// whether the core left (`leave`, which panics in a test).
    fn shutdown_asked_by(caller: Caller) -> (Res, bool) {
        let (_listener, mut c, mut stream) = crate::endpoint::tests::pair();
        let peer = std::thread::spawn(move || {
            ipc::send(&mut c, ipc::REQ, &Req { id: 9, cmd: "shutdown".into(), args: serde_json::Value::Null }).unwrap();
            std::thread::sleep(Duration::from_millis(200));
        });
        let (tx, rx) = Outbox::new(4);
        let replies = Replies { tx, stream: Arc::new(stream.try_clone().unwrap()) };
        let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| requests(&mut stream, &replies, 0, false, &caller)));
        let left = ran.is_err_and(|e| e.downcast_ref::<String>().is_some_and(|m| m.starts_with(LEFT)));
        peer.join().unwrap();
        let frame = rx.recv_timeout(Duration::from_secs(10)).expect("an answer");
        let (_, payload) = ipc::read_frame(&mut io::Cursor::new(frame)).unwrap();
        (serde_json::from_slice(&payload).unwrap(), left)
    }

    /// A core held for an update answers another caller's `shutdown` with
    /// an error it can read, keeps the connection and stays; it stops for
    /// the window that froze it, for a newer build, and for anyone once the
    /// update is called off, answering first.
    #[test]
    fn a_shutdown_while_an_update_holds_the_core_is_answered_and_the_core_stays() {
        let _alone = crate::agents::stops_for_test();
        let asker = crate::agents::Window { pid: 4242, created_ms: Some(7) };
        let other = crate::agents::Window { pid: 4343, created_ms: Some(8) };
        let by = |process: Option<crate::agents::Window>, newer: bool| Caller { process, window: process, newer };
        drop(crate::agents::freeze_for_update(Some(asker)).unwrap());
        for caller in [by(Some(other), false), by(None, false)] {
            let (res, left) = shutdown_asked_by(caller);
            assert!(!left, "the core stays");
            assert!(res.err.is_some_and(|e| e.contains("held for an update")));
        }
        for caller in [by(Some(asker), false), by(Some(other), true)] {
            let (res, left) = shutdown_asked_by(caller);
            assert!(left && res.err.is_none(), "answered, then it leaves");
        }
        crate::agents::call_off_update();
        let (res, left) = shutdown_asked_by(by(Some(other), false));
        assert!(left && res.err.is_none(), "anyone's once the update is off");
    }

    /// The uninstaller removes the core's folder only where `--stop-core`
    /// says no core runs: a core held for an update refuses it, and without
    /// its `core.json` no window would find it.
    #[test]
    fn the_uninstaller_keeps_the_folder_of_a_core_still_running() {
        let hooks = include_str!("../windows/hooks.nsh");
        let after = &hooks[hooks.find("--stop-core'").expect("the uninstaller stops the core")..];
        let checked = after.find("${If} $0 == 0").expect("its exit code checked");
        assert!(checked < after.find("RMDir /r").expect("the folder removed"));
    }

    #[test]
    fn a_core_run_as_administrator_says_who_it_is_and_does_nothing_else() {
        // Elevated itself (CI's runners and containers), the test process is
        // refused for its own reason before any it adds.
        let why = crate::elevation::refusal().unwrap_or(crate::elevation::ADMIN);
        if crate::elevation::refusal().is_none() {
            assert!(answered(None, "power_state").err.is_none(), "answered as ever when not elevated");
        }
        assert_eq!(answered(Some(why), "power_state").err.as_deref(), Some(why));
        assert_eq!(answered(Some(why), "ensure_claude_session").err.as_deref(), Some(why));
        assert_eq!(answered(Some(why), "background_helpers").err.as_deref(), Some(why));
        let core: Welcome = serde_json::from_value(answered(Some(why), "info").ok.expect("info is answered")).unwrap();
        assert_eq!(core.refused.as_deref(), Some(why), "and says why");
    }

    #[test]
    fn a_window_let_in_below_an_elevated_core_may_stop_it_and_nothing_else() {
        let why = crate::elevation::refusal().unwrap_or(crate::elevation::ADMIN);
        // Not even who it is: only the stop goes through. (Its answer ends this
        // process, so the stop itself is checked before `requests` acts on it.)
        for cmd in ["info", "power_state", "ensure_claude_session", "background_helpers"] {
            assert_eq!(answered_on(Some(why), cmd, true).err.as_deref(), Some(STOP_ONLY), "{cmd}");
            assert_eq!(turned_away(cmd, true).as_deref(), Some(STOP_ONLY), "{cmd}");
        }
        // A keystroke asked as a call is refused for its lane first.
        assert!(answered_on(Some(why), "write_session", true).err.is_some_and(|e| e.contains("not taken as a call")));
        assert_eq!(turned_away("write_session", true).as_deref(), Some(STOP_ONLY));
        assert_eq!(turned_away("shutdown", true), None, "the stop is answered");
        // On a connection with the full way in, a refusing core's own rule.
        refuse_here_for(why, || {
            assert_eq!(turned_away("shutdown", false), None);
            assert_eq!(turned_away("info", false), None);
            assert_eq!(turned_away("power_state", false).as_deref(), Some(why));
        });
    }

    fn refuse_here_for(why: &'static str, f: impl FnOnce()) {
        crate::elevation::refuse_here(Some(why));
        f();
        crate::elevation::refuse_here(None);
    }

    /// What build.rs leaves beside the exe, read when this build was compiled:
    /// the file the release publishes in `latest.json` without running the exe.
    const LEFT_BESIDE_THE_EXE: &str = include_str!(env!("LUNA_CORE_FP_FILE"));

    #[test]
    fn the_flag_reports_the_fingerprint_left_beside_the_exe() {
        let dir = crate::paths::data_dir().join("coreproc-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("core-fingerprint.txt");
        write_fingerprint(&file).unwrap();
        let reported = std::fs::read_to_string(&file).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(reported, LEFT_BESIDE_THE_EXE);
        assert!(reported.len() == 16 && reported.bytes().all(|b| b.is_ascii_hexdigit()), "{reported:?}");
    }
}
