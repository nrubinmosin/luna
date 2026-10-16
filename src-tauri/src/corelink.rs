//! The window's line to the core (coreproc.rs): find the one running for this
//! data dir or start it, say hello, and hold the connection for as long as
//! the window lives — the core counts on it to know a window is there, and
//! the window learns from its end that the core has gone.
//!
//! A core of another build is never worked with: it is asked to stop and a
//! core of this build is started in its place.

use crate::ipc::{self, Discovery, Hello, Req, Res, Welcome};
use serde::Serialize;
use std::io;
use std::net::TcpStream;
use std::sync::Mutex;
use std::time::Duration;

/// How long a core just started gets to write its `core.json`.
const START_WAIT: Duration = Duration::from_secs(10);

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct LinkState {
    /// starting | up | down
    pub state: &'static str,
    pub core: Option<Welcome>,
    pub error: Option<String>,
}

fn link() -> &'static Mutex<LinkState> {
    static L: std::sync::OnceLock<Mutex<LinkState>> = std::sync::OnceLock::new();
    L.get_or_init(|| Mutex::new(LinkState { state: "starting", ..Default::default() }))
}

/// Which connection is the current one: a restart replaces it, and the old
/// one's end must not then read as the new core going down.
static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn set(state: LinkState) {
    *link().lock().unwrap_or_else(|e| e.into_inner()) = state.clone();
    crate::emit::to_ui("core://state", state);
}

/// From setup, off the main thread: starting a core takes a moment.
pub fn start() {
    std::thread::spawn(connect);
}

fn connect() {
    use std::sync::atomic::Ordering;
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    set(LinkState { state: "starting", ..Default::default() });
    match attach() {
        Ok((stream, core)) => {
            set(LinkState { state: "up", core: Some(core), error: None });
            hold(stream, generation);
        }
        Err(e) => {
            crate::log::warn("core", &format!("no core: {e}"));
            set(LinkState { state: "down", core: None, error: Some(e) });
        }
    }
}

/// Reads until the core's end of the connection closes, which is the core
/// gone: nothing else closes it while the window lives.
fn hold(mut stream: TcpStream, generation: u64) {
    while ipc::read_frame(&mut stream).is_ok() {}
    if GENERATION.load(std::sync::atomic::Ordering::SeqCst) == generation {
        crate::log::warn("core", "the core went away");
        set(LinkState { state: "down", core: None, error: Some("the core stopped".into()) });
    }
}

/// The running core of this build, started if need be.
fn attach() -> Result<(TcpStream, Welcome), String> {
    if let Some(found) = discover() {
        match hello(&found, "ui") {
            Ok((stream, core)) if core.fingerprint == env!("LUNA_CORE_FP") => return Ok((stream, core)),
            Ok((stream, core)) => {
                crate::log::warn(
                    "core",
                    &format!("the running core is build {} (core {}); replacing it", core.build, core.fingerprint),
                );
                stop(stream);
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
        return hello(&found, "ui").map_err(|e| format!("the new core does not answer: {e}"));
    }
    Err(format!("no core came up within {}s", START_WAIT.as_secs()))
}

fn discover() -> Option<Discovery> {
    serde_json::from_slice(&std::fs::read(ipc::discovery_file()).ok()?).ok()
}

fn hello(found: &Discovery, role: &str) -> io::Result<(TcpStream, Welcome)> {
    let mut stream = TcpStream::connect_timeout(&([127, 0, 0, 1], found.port).into(), Duration::from_secs(2))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let args = serde_json::to_value(Hello { proto: ipc::PROTO, token: found.token.clone(), role: role.into() })?;
    ipc::send(&mut stream, ipc::REQ, &Req { id: 1, cmd: "hello".into(), args })?;
    let res: Res = ipc::recv(&mut stream, ipc::RES)?;
    let core: Welcome = match (res.ok, res.err) {
        (Some(ok), _) => serde_json::from_value(ok)?,
        (None, err) => return Err(io::Error::other(err.unwrap_or_else(|| "refused".into()))),
    };
    stream.set_read_timeout(None)?;
    Ok((stream, core))
}

/// Asks a core to stop and waits for it to be gone, which its end of the
/// connection closing says: the process exiting is what closes it.
fn stop(mut stream: TcpStream) {
    let _ = ipc::send(&mut stream, ipc::REQ, &Req { id: 2, cmd: "shutdown".into(), args: serde_json::Value::Null });
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    while ipc::read_frame(&mut stream).is_ok() {}
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

#[tauri::command]
pub fn core_state() -> LinkState {
    link().lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Stops the running core, if any, and starts one afresh.
#[tauri::command]
pub fn core_restart() {
    std::thread::spawn(|| {
        if let Some(found) = discover() {
            if let Ok((stream, _)) = hello(&found, "ctl") {
                stop(stream);
            }
        }
        connect();
    });
}
