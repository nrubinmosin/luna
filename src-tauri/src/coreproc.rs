//! The core: this same exe started as `--core`, with no window, where the
//! sessions live so that the window can come and go without them.
//!
//! The window starts it (corelink.rs) and stays connected; the core outlives
//! a window that crashes, hangs or is killed, and a window started after
//! finds it again through `<data>/core/core.json`. One core per data dir —
//! a lock file held for the core's life sees to that, and the OS lets go of
//! it however the core ends. With no window and no session for a while, the
//! core leaves by itself.
//!
//! Everything that runs the sessions runs here: the ptys (pty/), the hooks'
//! listener and the agents' MCP server (hub.rs, agents/), the activity
//! sampler and power (activity/, power/). What they send the page goes to
//! every connected window; what the page asks of them comes in by name
//! (corecmd.rs).

use crate::admit::{Deadline, Slot, Slots};
use crate::ipc::{self, Discovery, Event, Hello, Req, Res, Welcome};
use std::io::{self, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::PathBuf;
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
/// Connections let in at once before their hello: any local process can
/// connect, and only a hello with the token earns more than this. Windows and
/// one-off commands each say hello at once, so this is never full in use.
const STRANGERS: usize = 32;
/// For the whole hello, from the connection on: a window sends it at once,
/// and gives up on a core that has not answered within as long (corelink.rs).
const HELLO_DEADLINE: Duration = Duration::from_secs(5);
/// A hello's frame: a token, a role and a number, far smaller than this.
const HELLO_MAX: usize = 4096;

/// A connected window's outgoing queue, and its socket to close if it falls
/// behind.
struct Window {
    id: u64,
    tx: SyncSender<Vec<u8>>,
    stream: TcpStream,
}

static WINDOWS: Mutex<Vec<Window>> = Mutex::new(Vec::new());
/// Events no window was there to take that the next one must still get: a
/// chat an agent deleted would otherwise come back with the next window and
/// resume, a helper asked for would never be made, and a chat that passed to
/// another parent would go back to naming the one that is gone.
static HELD: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());
const KEPT_FOR_A_WINDOW: [&str; 3] = ["agent://deleted", "agent://spawn", "agent://reparent"];
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
/// the file and returns true. For scripts/release.ps1, which publishes it with the
/// update so a window can tell an update that leaves the core as it is; an
/// exe built for the windows subsystem has no console to print to.
pub fn fingerprint_from_args() -> bool {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("--core-fingerprint") {
        return false;
    }
    if let Some(file) = args.get(1) {
        let _ = std::fs::write(file, env!("LUNA_CORE_FP"));
    }
    true
}

/// `luna --stop-core`: has the running core stop — its sessions quit first —
/// and returns true once it is gone. For the uninstaller, which cannot remove
/// the copy of the exe a running core holds.
pub fn stop_from_args() -> bool {
    if std::env::args().nth(1).as_deref() != Some("--stop-core") {
        return false;
    }
    crate::corelink::shut_down();
    true
}

fn run() {
    // Before anything is written there. A failure is logged and shown in
    // Settings' checks; the token folders below are made private regardless.
    let _ = crate::paths::protect_data_dir();
    let dir = ipc::core_dir();
    // Owner-only: `core.json` in it is the key to the whole core.
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
    crate::lifecycle::start_core();
    // Here and not in the window: session_meta, which turns a Claude
    // transcript's tokens into a share of the window, runs in this process.
    crate::claude::models::refresh_soon();
    sweep_old_copies();
    keep_full_speed();

    let listener = match TcpListener::bind(("127.0.0.1", 0)) {
        Ok(l) => l,
        Err(e) => {
            crate::log::error("core", &format!("cannot listen: {e}"));
            return;
        }
    };

    // What the sessions send the page goes to every window there is.
    crate::emit::install(broadcast);
    // The helpers the last core left and what it kept of the chats, before a
    // window or an MCP call can ask after them.
    crate::agents::restore_chats();
    crate::background::restore();
    crate::agents::take_up_spawns();
    // The hook listener before any session can be spawned with hooks
    // pointing at it; the activity sampler feeds power/ from then on.
    crate::hub::start();
    crate::agents::init();
    crate::agents::register_dev_caller();
    crate::background::resume_stopped();
    crate::activity::start();
    crate::pty::prune_session_files_periodically();
    crate::media::prune_periodically();

    let token = ipc::mint_token();
    let found = Discovery {
        pid: std::process::id(),
        port: listener.local_addr().map(|a| a.port()).unwrap_or(0),
        token: token.clone(),
        created_ms: crate::procs::created_ms(std::process::id()).unwrap_or(0),
    };
    if let Err(e) = write_discovery(&found) {
        crate::log::error("core", &format!("cannot write core.json: {e}"));
        return;
    }

    ALONE_SINCE.store(started, Ordering::SeqCst);
    std::thread::spawn(leave_when_alone);
    let strangers = Slots::new(STRANGERS);
    for stream in listener.incoming().flatten() {
        // Past the cap the connection is closed at once, with nothing said.
        let Some(slot) = strangers.take() else { continue };
        let token = token.clone();
        std::thread::spawn(move || {
            if let Err(e) = serve(stream, slot, &token, started) {
                crate::log::info("core", &format!("connection ended: {e}"));
            }
        });
    }
    drop(lock);
}

/// Through a temporary file, so a window never reads half of it, and the
/// owner's alone: it holds the token.
fn write_discovery(d: &Discovery) -> io::Result<()> {
    crate::secret::write_private(&ipc::discovery_file(), &serde_json::to_vec(d)?)
}

/// Leaves, taking `core.json` along so no window tries the dead port. Every
/// session is asked to quit first, as a chat delete would, so each leaves its
/// transcript whole; a background helper's turn has no quit to be asked and
/// is stopped, its process tree with it, so nothing runs on unowned.
fn leave(reason: &str) -> ! {
    crate::lifecycle::exiting(reason);
    let _ = std::fs::remove_file(ipc::discovery_file());
    wind_down();
    crate::lifecycle::end_process()
}

/// Stops every session so that the next core takes them up where they stood: what
/// a core leaving does, and a restart of the PC. Returns how many chats were closed.
pub fn wind_down() -> usize {
    crate::background::stop_all();
    crate::agents::freeze_chats();
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
            let _ = w.stream.shutdown(Shutdown::Both);
            false
        }
        Err(TrySendError::Disconnected(_)) => false,
    });
}

/// The first frame of a connection, which must be a hello with the token:
/// read within `within` and `HELLO_MAX` bytes, the stranger's slot held until
/// then. Returns the request's id and the hello.
fn hello(stream: &TcpStream, _slot: Slot, token: &str, within: Duration) -> io::Result<(u64, Hello)> {
    let (kind, payload) = ipc::read_frame_within(&mut Deadline::new(stream, within), HELLO_MAX)?;
    stream.set_read_timeout(None)?;
    let first: Option<Req> = (kind == ipc::REQ).then(|| serde_json::from_slice(&payload).ok()).flatten();
    let hello = first.filter(|r| r.cmd == "hello").and_then(|r| Some((r.id, serde_json::from_value::<Hello>(r.args).ok()?)));
    // Nothing for someone without the token, not even why.
    hello
        .filter(|(_, h)| crate::secret::same(&h.token, token))
        .ok_or_else(|| io::Error::new(io::ErrorKind::PermissionDenied, "no valid hello"))
}

/// One connection: the hello, then requests until the other end goes.
fn serve(mut stream: TcpStream, slot: Slot, token: &str, started: u64) -> io::Result<()> {
    let (first_id, hello) = hello(&stream, slot, token, HELLO_DEADLINE)?;
    if hello.proto != ipc::PROTO {
        let err = format!("protocol {} spoken here, {} asked for", ipc::PROTO, hello.proto);
        ipc::send(&mut stream, ipc::RES, &Res { id: first_id, ok: None, err: Some(err) })?;
        return Ok(());
    }
    let welcome = serde_json::to_value(Welcome::of_this_process(started)).ok();
    ipc::send(&mut stream, ipc::RES, &Res { id: first_id, ok: welcome, err: None })?;

    // Everything this connection is sent from here on goes through its queue
    // and one writer thread, so answers and events never interleave mid-frame.
    let (tx, rx) = sync_channel::<Vec<u8>>(QUEUE);
    let mut out = stream.try_clone()?;
    std::thread::spawn(move || {
        for frame in rx {
            if out.write_all(&frame).and_then(|()| out.flush()).is_err() {
                break;
            }
        }
        let _ = out.shutdown(Shutdown::Both);
    });

    static NEXT: AtomicU64 = AtomicU64::new(1);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    if hello.role == "ui" {
        let mut windows = WINDOWS.lock().unwrap_or_else(|e| e.into_inner());
        // Under the windows' lock, so nothing held can slip past in between.
        for frame in HELD.lock().unwrap_or_else(|e| e.into_inner()).drain(..) {
            let _ = tx.try_send(frame);
        }
        windows.push(Window { id, tx: tx.clone(), stream: stream.try_clone()? });
    }
    let replies = Replies { tx, stream: Arc::new(stream.try_clone()?) };
    let result = requests(&mut stream, &replies, started);
    WINDOWS.lock().unwrap_or_else(|e| e.into_inner()).retain(|w| w.id != id);
    result
}

/// Where a connection's answers go: its queue, and its socket, closed when an
/// answer cannot be queued so the requests waiting on it fail rather than
/// wait for good.
#[derive(Clone)]
struct Replies {
    tx: SyncSender<Vec<u8>>,
    stream: Arc<TcpStream>,
}

fn requests(stream: &mut TcpStream, replies: &Replies, started: u64) -> io::Result<()> {
    loop {
        let (kind, payload) = ipc::read_frame(stream)?;
        let req: Req = serde_json::from_slice(&payload).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        match req.cmd.as_str() {
            "info" => answer(replies, req.id, serde_json::to_value(Welcome::of_this_process(started)).map_err(|e| e.to_string())),
            "shutdown" => {
                answer(replies, req.id, Ok(serde_json::Value::Null));
                leave("the window asked it to stop");
            }
            cmd if kind == ipc::SEND => {
                // Nobody waits for the answer to a keystroke.
                let _ = crate::corecmd::dispatch(cmd, req.args);
            }
            cmd if crate::corecmd::in_order(cmd) => answer(replies, req.id, crate::corecmd::dispatch(cmd, req.args)),
            _ => {
                let replies = replies.clone();
                std::thread::spawn(move || answer(&replies, req.id, crate::corecmd::dispatch(&req.cmd, req.args)));
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
            let _ = replies.stream.shutdown(Shutdown::Both);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn with_no_window_only_what_must_not_be_missed_is_held() {
        broadcast("pty://output", serde_json::json!({ "id": "x", "data": "hi" }));
        broadcast("agent://deleted", serde_json::json!({ "id": "x" }));
        let held = HELD.lock().unwrap().len();
        assert_eq!(held, 1, "a deleted chat waits for the next window; output does not");
        broadcast("agent://reparent", serde_json::json!({ "id": "x", "parent": "y" }));
        assert_eq!(HELD.lock().unwrap().len(), 2, "and so does a chat's new parent");
        HELD.lock().unwrap().clear();
    }

    /// `hello` on the far end of a fresh connection the client side of which
    /// `client` drives; what `hello` made of it, and how long that took.
    fn hello_from(within: Duration, client: impl FnOnce(TcpStream) + Send + 'static) -> (io::Result<(u64, Hello)>, Duration) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let peer = std::thread::spawn(move || client(TcpStream::connect(addr).unwrap()));
        let (stream, _) = listener.accept().unwrap();
        let slots = Slots::new(1);
        let begun = std::time::Instant::now();
        let got = hello(&stream, slots.take().unwrap(), "t0k3n", within);
        let took = begun.elapsed();
        assert_eq!(slots.taken(), 0, "the stranger's slot is given back");
        drop(stream);
        let _ = peer.join();
        (got, took)
    }

    fn hello_req(token: &str) -> Req {
        let args = serde_json::json!({ "proto": ipc::PROTO, "token": token, "role": "ctl" });
        Req { id: 7, cmd: "hello".into(), args }
    }

    #[test]
    fn a_hello_with_the_token_is_let_in() {
        let (got, _) = hello_from(HELLO_DEADLINE, |mut c| ipc::send(&mut c, ipc::REQ, &hello_req("t0k3n")).unwrap());
        let (id, h) = got.unwrap();
        assert_eq!((id, h.role.as_str()), (7, "ctl"));
    }

    #[test]
    fn a_stranger_gets_nothing() {
        let (got, _) = hello_from(HELLO_DEADLINE, |mut c| {
            ipc::send(&mut c, ipc::REQ, &hello_req("guess")).unwrap();
            // Nothing comes back before the core closes the connection.
            c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let mut out = Vec::new();
            let _ = c.read_to_end(&mut out);
            assert!(out.is_empty());
        });
        assert_eq!(got.err().unwrap().kind(), io::ErrorKind::PermissionDenied);
        let (got, _) = hello_from(HELLO_DEADLINE, |mut c| {
            ipc::send(&mut c, ipc::REQ, &Req { id: 1, cmd: "info".into(), args: serde_json::json!({}) }).unwrap()
        });
        assert_eq!(got.err().unwrap().kind(), io::ErrorKind::PermissionDenied, "a request before the hello");
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
            let frame = ipc::encode(ipc::REQ, &hello_req("t0k3n")).unwrap();
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
            let frame = ipc::encode(ipc::REQ, &hello_req("t0k3n")).unwrap();
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
}
