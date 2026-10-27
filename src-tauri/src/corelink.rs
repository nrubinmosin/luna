//! The window's line to the core (coreproc.rs): find the one running for this
//! data dir or start it, say hello, and hold the connection for as long as
//! the window lives. Everything the page asks of the sessions goes down it
//! (`core_call`, `core_send`), and everything the sessions send the page
//! comes up it and on to the webview.
//!
//! A core of another build is never worked with. One with no session running
//! is asked to stop and replaced at once; one with sessions is left alone
//! until the user says to restart it, which stops them (they resume after).

use crate::ipc::{self, Discovery, Event, Hello, Req, Res, Welcome};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{self, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long a core just started gets to write its `core.json`.
const START_WAIT: Duration = Duration::from_secs(10);
/// How long a core asked to stop gets: its sessions' grace and then some.
const STOP_WAIT: Duration = Duration::from_secs(30);

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct LinkState {
    /// starting | up | down | mismatch
    pub state: &'static str,
    pub core: Option<Welcome>,
    pub error: Option<String>,
}

/// The connection in use: its writing end and the requests waiting on it.
struct Conn {
    writer: Mutex<TcpStream>,
    pending: Mutex<HashMap<u64, Sender<Result<Value, String>>>>,
}

static CURRENT: Mutex<Option<Arc<Conn>>> = Mutex::new(None);
/// Which connection is the current one: a restart replaces it, and the old
/// one's end must not then read as the new core going down.
static GENERATION: AtomicU64 = AtomicU64::new(0);
static NEXT_ID: AtomicU64 = AtomicU64::new(10);

fn link() -> &'static Mutex<LinkState> {
    static L: std::sync::OnceLock<Mutex<LinkState>> = std::sync::OnceLock::new();
    L.get_or_init(|| Mutex::new(LinkState { state: "starting", ..Default::default() }))
}

fn set(state: LinkState) {
    *link().lock().unwrap_or_else(|e| e.into_inner()) = state.clone();
    crate::emit::to_ui("core://state", state);
}

/// From setup, off the main thread: starting a core takes a moment.
pub fn start() {
    std::thread::spawn(connect);
}

fn connect() {
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    set(LinkState { state: "starting", ..Default::default() });
    match attach() {
        Ok(Attached::Ours(stream, core)) => {
            let Ok(writer) = stream.try_clone() else {
                set(LinkState { state: "down", core: None, error: Some("cannot use the connection".into()) });
                return;
            };
            let conn = Arc::new(Conn { writer: Mutex::new(writer), pending: Mutex::new(HashMap::new()) });
            *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::clone(&conn));
            set(LinkState { state: "up", core: Some(core), error: None });
            hold(stream, conn, generation);
        }
        Ok(Attached::Foreign(core)) => {
            crate::log::warn(
                "core",
                &format!(
                    "the running core is build {} (core {}) with {} session(s); waiting for the user to restart it",
                    core.build, core.fingerprint, core.sessions
                ),
            );
            set(LinkState { state: "mismatch", core: Some(core), error: None });
        }
        Err(e) => {
            crate::log::warn("core", &format!("no core: {e}"));
            set(LinkState { state: "down", core: None, error: Some(e) });
        }
    }
}

/// Reads the core's frames until its end of the connection closes, which is
/// the core gone: nothing else closes it while the window lives.
fn hold(mut stream: TcpStream, conn: Arc<Conn>, generation: u64) {
    while let Ok((kind, payload)) = ipc::read_frame(&mut stream) {
        match kind {
            ipc::EVENT => {
                if let Ok(ev) = serde_json::from_slice::<Event>(&payload) {
                    crate::emit::to_ui(&ev.event, ev.payload);
                }
            }
            ipc::RES => {
                let Ok(res) = serde_json::from_slice::<Res>(&payload) else { continue };
                let waiting = conn.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&res.id);
                if let Some(tx) = waiting {
                    let _ = tx.send(match res.err {
                        Some(err) => Err(err),
                        None => Ok(res.ok.unwrap_or(Value::Null)),
                    });
                }
            }
            _ => {}
        }
    }
    // Whoever is still waiting hears it now rather than never.
    for (_, tx) in conn.pending.lock().unwrap_or_else(|e| e.into_inner()).drain() {
        let _ = tx.send(Err("the core stopped".into()));
    }
    if GENERATION.load(Ordering::SeqCst) == generation {
        *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = None;
        crate::log::warn("core", "the core went away");
        set(LinkState { state: "down", core: None, error: None });
    }
}

enum Attached {
    Ours(TcpStream, Welcome),
    /// Another build's, with sessions in it.
    Foreign(Welcome),
}

/// The running core of this build, started if need be.
fn attach() -> Result<Attached, String> {
    if let Some(found) = discover() {
        match hello(&found, "ui") {
            Ok((stream, core)) if core.fingerprint == env!("LUNA_CORE_FP") => return Ok(Attached::Ours(stream, core)),
            Ok((_, core)) if core.sessions > 0 => return Ok(Attached::Foreign(core)),
            Ok((stream, core)) => {
                crate::log::warn("core", &format!("the running core is build {} (core {}); replacing it", core.build, core.fingerprint));
                stop(stream, found.pid);
            }
            // A core.json left by a core that is gone; a new one overwrites it.
            Err(_) => {}
        }
    }
    let before = discover().map(|d| d.pid);
    launch()?;
    let deadline = std::time::Instant::now() + START_WAIT;
    while std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
        let Some(found) = discover().filter(|d| Some(d.pid) != before) else { continue };
        return hello(&found, "ui")
            .map(|(stream, core)| Attached::Ours(stream, core))
            .map_err(|e| format!("the new core does not answer: {e}"));
    }
    Err(format!("no core came up within {}s", START_WAIT.as_secs()))
}

fn discover() -> Option<Discovery> {
    serde_json::from_slice(&std::fs::read(ipc::discovery_file()).ok()?).ok()
}

fn hello(found: &Discovery, role: &str) -> io::Result<(TcpStream, Welcome)> {
    let mut stream = TcpStream::connect_timeout(&([127, 0, 0, 1], found.port).into(), Duration::from_secs(2))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    // Keystrokes go out as they are typed, not when a buffer fills.
    stream.set_nodelay(true)?;
    let args = serde_json::to_value(Hello { proto: ipc::PROTO, token: found.token.clone(), role: role.into() })?;
    ipc::send(&mut stream, ipc::REQ, &Req { id: 1, cmd: "hello".into(), args })?;
    let res: Res = ipc::recv(&mut stream, ipc::RES)?;
    let core: Welcome = match (res.ok, res.err) {
        (Some(ok), None) => serde_json::from_value(ok)?,
        (_, err) => return Err(io::Error::other(err.unwrap_or_else(|| "refused".into()))),
    };
    stream.set_read_timeout(None)?;
    Ok((stream, core))
}

/// Asks a core to stop and waits for it to be gone, which its end of the
/// connection closing says: the process exiting is what closes it. It asks
/// its sessions to quit first, so this can take some seconds. One still
/// there well past their grace is killed with everything it started, the
/// sessions' CLIs among them: stopping must leave nothing of Luna running.
fn stop(mut stream: TcpStream, pid: u32) {
    let _ = ipc::send(&mut stream, ipc::REQ, &Req { id: 2, cmd: "shutdown".into(), args: Value::Null });
    let deadline = std::time::Instant::now() + STOP_WAIT;
    let gone = loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() || stream.set_read_timeout(Some(left)).is_err() {
            break false;
        }
        match ipc::read_frame(&mut stream) {
            Ok(_) => {}
            Err(e) => break !matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut),
        }
    };
    if !gone {
        crate::log::warn("core", &format!("the core (pid {pid}) did not stop within {}s; killing it", STOP_WAIT.as_secs()));
        crate::pty::kill_tree(Some(pid));
    }
}

/// Asks the core a command and waits for the answer.
fn request(cmd: String, args: Value) -> Result<Value, String> {
    // The panes ask for their sessions as soon as the page is up, which can be
    // before the core is: while it is being found or started, they wait.
    let deadline = std::time::Instant::now() + START_WAIT * 2;
    let conn = loop {
        if let Some(conn) = CURRENT.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            break conn;
        }
        let starting = link().lock().unwrap_or_else(|e| e.into_inner()).state == "starting";
        if !starting || std::time::Instant::now() >= deadline {
            return Err("the core is not running".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = channel();
    conn.pending.lock().unwrap_or_else(|e| e.into_inner()).insert(id, tx);
    let frame = ipc::encode(ipc::REQ, &Req { id, cmd, args }).map_err(|e| e.to_string())?;
    if let Err(e) = write(&conn, &frame) {
        conn.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
        return Err(format!("the core is not reachable: {e}"));
    }
    rx.recv().unwrap_or_else(|_| Err("the core stopped".into()))
}

fn write(conn: &Conn, frame: &[u8]) -> io::Result<()> {
    let mut w = conn.writer.lock().unwrap_or_else(|e| e.into_inner());
    w.write_all(frame)?;
    w.flush()
}

/// Starts a core of this build, detached: it is meant to outlive the window.
fn launch() -> Result<(), String> {
    let exe = core_exe()?;
    let data = crate::paths::data_dir();
    let log = crate::log::path().clone();
    let mut cmd = std::process::Command::new(&exe);
    cmd.arg(crate::coreproc::FLAG)
        .arg("--data-dir")
        .arg(&data)
        .arg("--log")
        .arg(&log)
        // Not in any project folder, so it never holds one open.
        .current_dir(&data)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    spawn_detached(cmd).map_err(|e| format!("cannot start {}: {e}", exe.display()))
}

/// Out of any job the window was started in, so closing that job does not
/// take the core; a job that forbids breaking away refuses the first try.
#[cfg(windows)]
fn spawn_detached(mut cmd: std::process::Command) -> io::Result<()> {
    use std::os::windows::process::CommandExt;
    use windows::Win32::System::Threading::{CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS};
    let flags = DETACHED_PROCESS.0 | CREATE_NEW_PROCESS_GROUP.0;
    cmd.creation_flags(flags | CREATE_BREAKAWAY_FROM_JOB.0);
    cmd.spawn().or_else(|_| cmd.creation_flags(flags).spawn()).map(drop)
}

/// A process group of its own, so a signal to the window's group (a terminal
/// closing) does not reach it.
#[cfg(unix)]
fn spawn_detached(mut cmd: std::process::Command) -> io::Result<()> {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0).spawn().map(drop)
}

/// The exe the core runs from. On Windows a copy per build under
/// `<data>/core/<fingerprint>/`: the installer replaces `luna.exe` while a
/// core keeps running, which it could not do to a file in use. Elsewhere the
/// running file can be replaced under a process, and the exe itself serves.
fn core_exe() -> Result<std::path::PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    if !cfg!(windows) {
        return Ok(exe);
    }
    let stem = exe.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "luna".into());
    let dir = ipc::core_dir().join(env!("LUNA_CORE_FP"));
    let copy = dir.join(format!("{stem}-core.exe"));
    if !copy.exists() {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let tmp = copy.with_extension("exe.tmp");
        std::fs::copy(&exe, &tmp).map_err(|e| format!("copy to {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &copy).map_err(|e| e.to_string())?;
    }
    Ok(copy)
}

/// Has the core stop: its sessions are asked to quit first, then it leaves.
/// Returns once it is gone.
pub fn shut_down() {
    // The connection in use is about to end on purpose: not news for the log
    // or the page.
    GENERATION.fetch_add(1, Ordering::SeqCst);
    *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = None;
    if let Some(found) = discover() {
        if let Ok((stream, _)) = hello(&found, "ctl") {
            stop(stream, found.pid);
        }
    }
}

#[tauri::command]
pub fn core_state() -> LinkState {
    link().lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Stops the running core, whatever its build — its sessions quit and resume
/// in the new one — and starts one of this build.
#[tauri::command]
pub fn core_restart() {
    std::thread::spawn(|| {
        // The page's calls wait for the new core rather than fail meanwhile.
        set(LinkState { state: "starting", ..Default::default() });
        shut_down();
        connect();
    });
}

/// A command for the sessions, answered by the core (corecmd.rs).
#[tauri::command]
pub async fn core_call(cmd: String, args: Value) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || request(cmd, args)).await.map_err(|e| e.to_string())?
}

/// Keystrokes and resizes: written to the core in the order they come, and
/// not waited on. Sync, so the page's calls keep their order.
#[tauri::command]
pub fn core_send(cmd: String, args: Value) -> Result<(), String> {
    let conn = CURRENT.lock().unwrap_or_else(|e| e.into_inner()).clone().ok_or("the core is not running")?;
    let frame = ipc::encode(ipc::SEND, &Req { id: 0, cmd, args }).map_err(|e| e.to_string())?;
    write(&conn, &frame).map_err(|e| format!("the core is not reachable: {e}"))
}
