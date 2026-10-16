//! The core: this same exe started as `--core`, with no window, for the
//! sessions to live in so that the window can come and go without them.
//!
//! The window starts it (corelink.rs) and stays connected; the core outlives
//! a window that crashes, hangs or is killed, and a window started after
//! finds it again through `<data>/core/core.json`. One core per data dir —
//! a lock file held for the core's life sees to that, and the OS lets go of
//! it however the core ends. With no window for a while and nothing to keep,
//! the core leaves by itself.

use crate::ipc::{self, Discovery, Hello, Req, Res, Welcome};
use std::io;
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

pub const FLAG: &str = "--core";
/// How long the core waits with no window before it goes.
const IDLE_EXIT: Duration = Duration::from_secs(10 * 60);

/// Windows connected right now.
static WINDOWS: AtomicUsize = AtomicUsize::new(0);
/// When the last one left, epoch ms; the core's start until one comes.
static ALONE_SINCE: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// `luna --core --data-dir <dir> --log <path>`: runs as the core and returns
/// true, or false for an ordinary launch.
pub fn from_args() -> bool {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some(FLAG) {
        return false;
    }
    let value = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).map(PathBuf::from);
    let (Some(data), Some(log)) = (value("--data-dir"), value("--log")) else { return true };
    crate::paths::set_data_dir(data);
    crate::log::set_path(log);
    crate::log::set_role("core");
    crate::log::install_panic_hook();
    run();
    true
}

fn run() {
    let dir = ipc::core_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
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
    sweep_old_copies();

    let listener = match TcpListener::bind(("127.0.0.1", 0)) {
        Ok(l) => l,
        Err(e) => {
            crate::log::error("core", &format!("cannot listen: {e}"));
            return;
        }
    };
    let token = ipc::mint_token();
    let found = Discovery {
        pid: std::process::id(),
        port: listener.local_addr().map(|a| a.port()).unwrap_or(0),
        token: token.clone(),
    };
    if let Err(e) = write_discovery(&found) {
        crate::log::error("core", &format!("cannot write core.json: {e}"));
        return;
    }

    ALONE_SINCE.store(started, Ordering::SeqCst);
    std::thread::spawn(leave_when_alone);
    for stream in listener.incoming().flatten() {
        let token = token.clone();
        std::thread::spawn(move || {
            if let Err(e) = serve(stream, &token, started) {
                crate::log::info("core", &format!("connection ended: {e}"));
            }
        });
    }
    drop(lock);
}

/// Through a temporary file, so a window never reads half of it.
fn write_discovery(d: &Discovery) -> io::Result<()> {
    let path = ipc::discovery_file();
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(d)?)?;
    std::fs::rename(&tmp, &path)
}

/// Leaves, taking `core.json` along so no window tries the dead port.
fn leave(reason: &str) -> ! {
    crate::lifecycle::exiting(reason);
    let _ = std::fs::remove_file(ipc::discovery_file());
    std::process::exit(0)
}

fn leave_when_alone() {
    loop {
        std::thread::sleep(Duration::from_secs(30));
        let alone = now_ms().saturating_sub(ALONE_SINCE.load(Ordering::SeqCst));
        if WINDOWS.load(Ordering::SeqCst) == 0 && alone >= IDLE_EXIT.as_millis() as u64 {
            leave("no window for ten minutes");
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

/// One connection: the hello, then requests until the other end goes.
fn serve(mut stream: TcpStream, token: &str, started: u64) -> io::Result<()> {
    let first: Req = ipc::recv(&mut stream, ipc::REQ)?;
    let hello: Option<Hello> = (first.cmd == "hello").then(|| serde_json::from_value(first.args).ok()).flatten();
    let Some(hello) = hello.filter(|h| h.token == token) else {
        // Nothing for someone without the token, not even why.
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "no valid hello"));
    };
    if hello.proto != ipc::PROTO {
        let err = format!("protocol {} spoken here, {} asked for", ipc::PROTO, hello.proto);
        ipc::send(&mut stream, ipc::RES, &Res { id: first.id, ok: None, err: Some(err) })?;
        return Ok(());
    }
    let welcome = serde_json::to_value(Welcome::of_this_process(started)).ok();
    ipc::send(&mut stream, ipc::RES, &Res { id: first.id, ok: welcome, err: None })?;

    let window = hello.role == "ui";
    if window {
        WINDOWS.fetch_add(1, Ordering::SeqCst);
    }
    let result = requests(&mut stream, started);
    if window && WINDOWS.fetch_sub(1, Ordering::SeqCst) == 1 {
        ALONE_SINCE.store(now_ms(), Ordering::SeqCst);
    }
    result
}

fn requests(stream: &mut TcpStream, started: u64) -> io::Result<()> {
    loop {
        let req: Req = ipc::recv(stream, ipc::REQ)?;
        let res = match req.cmd.as_str() {
            "info" => Res { id: req.id, ok: serde_json::to_value(Welcome::of_this_process(started)).ok(), err: None },
            "shutdown" => {
                let _ = ipc::send(stream, ipc::RES, &Res { id: req.id, ok: Some(serde_json::Value::Null), err: None });
                leave("the window asked it to stop");
            }
            other => Res { id: req.id, ok: None, err: Some(format!("unknown command {other}")) },
        };
        ipc::send(stream, ipc::RES, &res)?;
    }
}
