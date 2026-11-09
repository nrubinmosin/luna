//! The window's line to the core (coreproc.rs): find the one running for this
//! data dir or start it, say hello, and hold the connection for as long as
//! the window lives. Everything the page asks of the sessions goes down it
//! (`core_call`, `core_send`), only the commands `corecmd::COMMANDS` lists for
//! the page, and everything the sessions send the page comes up it and on to
//! the webview.
//!
//! A core of another build is never worked with. One with no session running
//! is asked to stop and replaced at once; one with sessions is left alone
//! until the user says to restart it, which stops them (they resume after).
//! A core of protocol 1 (Luna 0.11 and before, v1.rs) is always such a one,
//! reached the old way only to ask that much.
//!
//! Nor is a core run as administrator, whose CLIs would run so too
//! (elevation.rs): one with no session is replaced, one with sessions waits
//! for a restart. A window that is itself elevated starts the core with the
//! desktop shell's ordinary token, and where there is none starts no core.

mod v1;

use crate::corecmd;
use crate::endpoint::Stream;
use crate::ipc::{self, Discovery, Event, Found, Hello, Req, Res, Welcome};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
    /// starting | up | down | mismatch | elevated
    pub state: &'static str,
    pub core: Option<Welcome>,
    pub error: Option<String>,
}

/// The connection in use: its writing end and the requests waiting on it.
/// `pending` is None once the connection has ended: a request let in after
/// the last of them heard so would wait for an answer nothing can bring.
struct Conn {
    writer: Mutex<Stream>,
    pending: Mutex<Option<HashMap<u64, Sender<Result<Value, String>>>>>,
}

static CURRENT: Mutex<Option<Arc<Conn>>> = Mutex::new(None);
/// Which connection is the current one: a restart replaces it, and the old
/// one's end must not then read as the new core going down.
static GENERATION: AtomicU64 = AtomicU64::new(0);
static NEXT_ID: AtomicU64 = AtomicU64::new(10);
/// A restart under way: one at a time, or two would both say hello to the new
/// core as `ui` and every event would reach the page twice. `connect` lets go
/// of it once the new link is set up.
static RESTARTING: AtomicBool = AtomicBool::new(false);
/// Held while a connection is made the current one or let go: a way back to
/// a core that let the window go (`rejoin`) checks there that no restart has
/// begun since, and a restart's own link waits for it to be done.
static TAKING: Mutex<()> = Mutex::new(());

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
    let attached = attach();
    // Set up, whichever way it went: a restart may be asked again. What
    // follows holds the connection for the core's whole life.
    RESTARTING.store(false, Ordering::SeqCst);
    match attached {
        Ok(Attached::Ours(stream, core)) => {
            let (mut link, mut rejoined) = (Some((stream, core)), false);
            while let Some((stream, core)) = link.take() {
                let Ok(writer) = stream.try_clone() else {
                    set(LinkState { state: "down", core: None, error: Some("cannot use the connection".into()) });
                    return;
                };
                let conn = Arc::new(Conn { writer: Mutex::new(writer), pending: Mutex::new(Some(HashMap::new())) });
                // A restart has taken over from here, or is stopping this core
                // to start the next: no core for this connection to stand for.
                // Looked at under the lock every taking of the line holds, so a
                // way back that took its time cannot land over a restart's.
                {
                    let _taking = TAKING.lock().unwrap_or_else(|e| e.into_inner());
                    if GENERATION.load(Ordering::SeqCst) != generation || (rejoined && RESTARTING.load(Ordering::SeqCst)) {
                        return;
                    }
                    if rejoined {
                        // The panes start over, as after any core's start.
                        set(LinkState { state: "starting", ..Default::default() });
                    }
                    *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::clone(&conn));
                    set(LinkState { state: "up", core: Some(core), error: None });
                }
                hold(stream, conn);
                {
                    let _taking = TAKING.lock().unwrap_or_else(|e| e.into_inner());
                    if GENERATION.load(Ordering::SeqCst) != generation || RESTARTING.load(Ordering::SeqCst) {
                        return;
                    }
                    *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = None;
                }
                link = rejoin();
                rejoined = true;
            }
            // Not over a restart that began while the way back was tried.
            let _taking = TAKING.lock().unwrap_or_else(|e| e.into_inner());
            if GENERATION.load(Ordering::SeqCst) == generation && !RESTARTING.load(Ordering::SeqCst) {
                crate::log::warn("core", "the core went away");
                set(LinkState { state: "down", core: None, error: None });
            }
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
        Ok(Attached::Elevated { core, why }) => {
            crate::log::error("core", &why);
            set(LinkState { state: "elevated", core, error: Some(why) });
        }
        Err(e) => {
            crate::log::warn("core", &format!("no core: {e}"));
            set(LinkState { state: "down", core: None, error: Some(e) });
        }
    }
}

/// A core that let this window go is still there to go back to: it does that
/// to a window that fell behind on the output (coreproc.rs `broadcast`), while
/// its sessions run on. Only to a core of this build that answers; one that is
/// gone stays gone until the user restarts it (CoreBanner), never behind their
/// back. Going back, the window says it is starting, so its panes start over
/// and each reattaches from the scrollback (core.store.ts).
fn rejoin() -> Option<(Stream, Welcome)> {
    let Found::Current(found) = discover().filter(Found::running)? else { return None };
    match hello(&found, "ui") {
        Ok((stream, core)) if core.fingerprint == env!("LUNA_CORE_FP") => {
            crate::log::warn("core", "the core let this window go and is still running; going back to it");
            Some((stream, core))
        }
        _ => None,
    }
}

/// Reads the core's frames until its end of the connection closes: the core
/// gone, or one that let this window go (`rejoin`).
fn hold(mut stream: Stream, conn: Arc<Conn>) {
    while let Ok((kind, payload)) = ipc::read_frame(&mut stream) {
        match kind {
            ipc::EVENT => {
                if let Ok(ev) = serde_json::from_slice::<Event>(&payload) {
                    crate::emit::to_ui(&ev.event, ev.payload);
                }
            }
            ipc::RES => {
                let Ok(res) = serde_json::from_slice::<Res>(&payload) else { continue };
                let mut pending = conn.pending.lock().unwrap_or_else(|e| e.into_inner());
                let waiting = pending.as_mut().and_then(|p| p.remove(&res.id));
                drop(pending);
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
    let waiting = conn.pending.lock().unwrap_or_else(|e| e.into_inner()).take();
    for (_, tx) in waiting.into_iter().flatten() {
        let _ = tx.send(Err("the core stopped".into()));
    }
}

enum Attached {
    Ours(Stream, Welcome),
    /// Another build's, with sessions in it.
    Foreign(Welcome),
    /// No core to work with that is not run as administrator (elevation.rs):
    /// this window cannot start one, or the one running is elevated. `core`
    /// is that one when a restart would replace it: it has sessions.
    Elevated {
        core: Option<Welcome>,
        why: String,
    },
}

/// What a window does with the core it finds running.
#[derive(Debug, PartialEq)]
enum Verdict {
    Attach,
    /// Another build's with sessions: left until the person says to restart it.
    Foreign,
    /// Stopped and replaced at once: another build's with no session, or an
    /// elevated one with none.
    Replace,
    /// Run as administrator, with sessions: left until the person says to
    /// restart it, never worked with. Their CLIs run elevated.
    Elevated,
}

/// What this window read of the core's token, fail closed: only a token read
/// as ordinary is one. One that could not be read at all is not worked with.
fn counts_as_elevated(read: Option<bool>) -> bool {
    read != Some(false)
}

/// `elevated`: what the core's token says, read by this window
/// (`counts_as_elevated`); a core older than the check does not say so itself.
/// `v1`: a core of protocol 1, another build's whatever it says.
fn verdict(core: &Welcome, elevated: bool, v1: bool) -> Verdict {
    if elevated || core.refused.is_some() {
        return if core.sessions > 0 { Verdict::Elevated } else { Verdict::Replace };
    }
    match (!v1 && core.fingerprint == env!("LUNA_CORE_FP"), core.sessions > 0) {
        (true, _) => Verdict::Attach,
        (false, true) => Verdict::Foreign,
        (false, false) => Verdict::Replace,
    }
}

/// An elevated core that this window can neither talk to nor stop: only one
/// of its own rights can, or time. A core of this build or later lets an
/// ordinary window in to stop it (peer.rs `StopOnly`), so this is another's:
/// with no session, it leaves ten minutes after its last window went
/// (coreproc.rs `leave_when_alone`).
const ELEVATED_STUCK: &str = "A Luna core started as administrator is running, and Luna cannot stop it without \
    administrator rights. With no session in it, it leaves by itself ten minutes after its last window closed: start Luna \
    again then. Or sign out of Windows, or run `luna --stop-core` as administrator.";

/// The running core of this build, started if need be.
fn attach() -> Result<Attached, String> {
    // An elevated core that would not answer: if no new core can start beside
    // it, that one is why. Its pid alone proves nothing: after a reboot it may
    // be some process of SYSTEM's that this window cannot open either.
    let mut silent_elevated = None;
    // A core.json whose core is gone — the PC went down under it, and its
    // pid may be another program's by now — is not tried: a connect to its
    // endpoint would only fail, or reach whatever took its name.
    if let Some(found) = discover().filter(Found::running) {
        let elevated = counts_as_elevated(crate::elevation::process_elevated(found.pid()));
        match meet(&found, elevated) {
            Met::Ours(stream, core) => return Ok(Attached::Ours(stream, core)),
            Met::Done(attached) => return Ok(attached),
            Met::Replaced => {}
            // An elevated core may refuse a window that is not (a check of the
            // caller's integrity), and its lock keeps any other from starting.
            Met::Silent if elevated => silent_elevated = Some(found),
            // A core that does not answer; a new one overwrites its core.json.
            Met::Silent => {}
        }
    }
    let how = match crate::elevation::plan(&crate::elevation::System) {
        crate::elevation::Launch::Refused(r) => return Ok(Attached::Elevated { core: None, why: r.message() }),
        how => how,
    };
    let before = discover().map(|d| d.pid());
    match launch(&how) {
        // Not a core that stopped: a restart would only fail the same way.
        Err(e) if how != crate::elevation::Launch::Directly => {
            let why = format!("{} Starting its core with this desktop's ordinary rights failed: {e}", crate::elevation::ADMIN);
            return Ok(Attached::Elevated { core: None, why });
        }
        started => started?,
    }
    let deadline = std::time::Instant::now() + START_WAIT;
    while std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
        // The one just started speaks this protocol; a protocol-1 core that
        // some older window started meanwhile is not it.
        let Some(Found::Current(found)) = discover().filter(|d| Some(d.pid()) != before) else { continue };
        let (stream, core) = hello(&found, "ui").map_err(|e| format!("the new core does not answer: {e}"))?;
        // Started elevated after all: it runs nothing, and is not left to idle.
        if let Some(why) = core.refused.clone() {
            stop(stream, found.pid);
            return Ok(Attached::Elevated { core: None, why });
        }
        return Ok(Attached::Ours(stream, core));
    }
    if silent_elevated.is_some_and(|d| d.running()) {
        return Ok(Attached::Elevated { core: None, why: ELEVATED_STUCK.into() });
    }
    Err(format!("no core came up within {}s", START_WAIT.as_secs()))
}

/// What came of meeting the core a `core.json` names.
enum Met {
    /// Of this build: the line to keep.
    Ours(Stream, Welcome),
    /// Left as it is, for the person to decide on, or one that cannot be
    /// replaced from here.
    Done(Attached),
    /// Stopped: a new one is to be started.
    Replaced,
    /// It did not answer.
    Silent,
}

/// The line to a core met, by the protocol it speaks.
enum Line {
    Current(Stream),
    V1(v1::Link),
}

impl Line {
    fn stop(self, pid: u32) {
        match self {
            Line::Current(stream) => stop(stream, pid),
            Line::V1(link) => link.stop(pid),
        }
    }
}

/// Says hello to the core `found` names, as `role`, in its protocol.
fn greet(found: &Found, role: &str) -> io::Result<(Line, Welcome)> {
    match found {
        Found::Current(d) => hello(d, role).map(|(s, core)| (Line::Current(s), core)),
        Found::V1(d) => v1::hello(d, role).map(|(link, core)| (Line::V1(link), core)),
    }
}

/// Says hello to the running core `found` names and decides on it
/// (`verdict`): kept, left alone, or stopped to be replaced. A protocol-1 core
/// is never kept: it hears a hello, and a shutdown when it has no session, and
/// nothing else.
fn meet(found: &Found, elevated: bool) -> Met {
    let Ok((line, core)) = greet(found, "ui") else { return Met::Silent };
    match verdict(&core, elevated, matches!(line, Line::V1(_))) {
        Verdict::Attach => match line {
            Line::Current(stream) => Met::Ours(stream, core),
            // Never so (`verdict`); were it, another build's all the same.
            Line::V1(_) => Met::Done(Attached::Foreign(core)),
        },
        Verdict::Foreign => Met::Done(Attached::Foreign(core)),
        Verdict::Elevated => {
            let why = format!(
                "The sessions are running in a Luna core started as administrator ({} running). Restart it to run them \
                 normally: they quit and resume in the new one.",
                core.sessions
            );
            Met::Done(Attached::Elevated { core: Some(core), why })
        }
        Verdict::Replace => {
            let how = if elevated || core.refused.is_some() { "run as administrator" } else { "of another build" };
            crate::log::warn("core", &format!("the running core is {how} (build {}, core {}); replacing it", core.build, core.fingerprint));
            line.stop(found.pid());
            if elevated && found.running() {
                return Met::Done(Attached::Elevated { core: None, why: ELEVATED_STUCK.into() });
            }
            Met::Replaced
        }
    }
}

fn discover() -> Option<Found> {
    Found::parse(&std::fs::read(ipc::discovery_file()).ok()?)
}

/// Connects to the core `found` names and says hello. The endpoint's server
/// must be the process the file names, or nothing is said: a name served by
/// anyone else (one who took it while it was free) learns nothing, and the
/// window opens it for identification only, so it can act as nobody.
fn hello(found: &Discovery, role: &str) -> io::Result<(Stream, Welcome)> {
    let mut stream = Stream::connect(&found.endpoint, Duration::from_secs(2))?;
    let server = stream.server_pid()?;
    if server != found.pid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("the core's endpoint is served by pid {server}, not by the core's pid {}", found.pid),
        ));
    }
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let args = serde_json::to_value(Hello { proto: ipc::PROTO, role: role.into() })?;
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
/// The core's endpoint, or a protocol-1 core's socket (v1.rs).
fn stop<S: Read + Write + crate::endpoint::Timeouts>(mut stream: S, pid: u32) {
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
pub(crate) fn request(cmd: String, args: Value) -> Result<Value, String> {
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
    let frame = ipc::encode(ipc::REQ, &Req { id, cmd, args }).map_err(|e| e.to_string())?;
    let (tx, rx) = channel();
    match conn.pending.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        Some(pending) => pending.insert(id, tx),
        None => return Err("the core stopped".into()),
    };
    if let Err(e) = write(&conn, &frame) {
        if let Some(pending) = conn.pending.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            pending.remove(&id);
        }
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
/// `how` is never a refusal: `attach` has stopped at one.
fn launch(how: &crate::elevation::Launch) -> Result<(), String> {
    let exe = core_exe()?;
    let cmd = core_command(&exe);
    let started = match how {
        // The Secondary Logon service creates it, not this window: it
        // inherits none of the window's handles, and no console.
        crate::elevation::Launch::AsShell(shell) => {
            crate::log::info("core", "this window runs as administrator; starting the core with the desktop shell's token");
            crate::elevation::spawn_as_shell(*shell, &cmd)
        }
        _ => spawn_detached(cmd),
    };
    started.map_err(|e| format!("cannot start {}: {e}", exe.display()))
}

/// The core's command line: where its data and log are, and the window's exe.
/// Nothing that lets anyone in, as there is nothing of the kind to pass.
fn core_command(exe: &std::path::Path) -> std::process::Command {
    let data = crate::paths::data_dir();
    let mut cmd = std::process::Command::new(exe);
    cmd.arg(crate::coreproc::FLAG)
        .arg("--data-dir")
        .arg(&data)
        .arg("--log")
        .arg(crate::log::path())
        // Its own exe is a copy, and a restart of the PC starts this one again.
        .args(std::env::current_exe().ok().map(|exe| [std::ffi::OsString::from("--window-exe"), exe.into()]).into_iter().flatten())
        // Not in any project folder, so it never holds one open.
        .current_dir(&data)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    cmd
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
    // or the page, and not one for a way back (`rejoin`) to make current again.
    {
        let _taking = TAKING.lock().unwrap_or_else(|e| e.into_inner());
        GENERATION.fetch_add(1, Ordering::SeqCst);
        *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
    // Through the core's own endpoint, which lets in this user's unconfined
    // processes alone (and an administrator, to stop it): `luna --stop-core`
    // is no shutdown for anyone who asks. A protocol-1 core is stopped the
    // way it listens, with its token.
    if let Some(found) = discover().filter(Found::running) {
        if let Ok((line, _)) = greet(&found, "ctl") {
            line.stop(found.pid());
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
    if RESTARTING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| {
        // The page's calls wait for the new core rather than fail meanwhile.
        set(LinkState { state: "starting", ..Default::default() });
        shut_down();
        connect();
    });
}

/// The page may have the window pass on only the commands the core lists for
/// it, each on its own lane (`corecmd::COMMANDS`): the protocol's `shutdown`
/// and `info`, and a command sent on the wrong lane, stop here.
fn passes(cmd: &str, on: corecmd::Lane) -> Result<(), String> {
    match corecmd::lane(cmd) {
        Some(lane) if lane == on => Ok(()),
        _ => Err(format!("the window does not pass {cmd:?} on to the core")),
    }
}

/// A command for the sessions, answered by the core (corecmd.rs).
#[tauri::command]
pub async fn core_call(cmd: String, args: Value) -> Result<Value, String> {
    passes(&cmd, corecmd::Lane::Call)?;
    tauri::async_runtime::spawn_blocking(move || request(cmd, args)).await.map_err(|e| e.to_string())?
}

/// Keystrokes and resizes: written to the core in the order they come, and
/// not waited on. Sync, so the page's calls keep their order.
#[tauri::command]
pub fn core_send(cmd: String, args: Value) -> Result<(), String> {
    passes(&cmd, corecmd::Lane::Send)?;
    let conn = CURRENT.lock().unwrap_or_else(|e| e.into_inner()).clone().ok_or("the core is not running")?;
    let frame = ipc::encode(ipc::SEND, &Req { id: 0, cmd, args }).map_err(|e| e.to_string())?;
    write(&conn, &frame).map_err(|e| format!("the core is not reachable: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_cannot_have_the_window_pass_on_what_the_core_does_not_list() {
        // Refused before any core is looked for. Passed on, the protocol's own
        // commands would stop the core or answer for it, and a command on the
        // wrong lane would run unanswered or be waited on.
        let refused = |r: Result<(), String>| r.is_err_and(|e| e.contains("does not pass"));
        for cmd in ["shutdown", "info", "hello", "", "ensure_claude_session", "kill_session"] {
            assert!(refused(core_send(cmd.into(), Value::Null)), "core_send {cmd}");
        }
        for cmd in ["shutdown", "info", "hello", "", "write_session", "resize_session", "nope"] {
            let r = tauri::async_runtime::block_on(core_call(cmd.into(), Value::Null)).map(drop);
            assert!(refused(r), "core_call {cmd}");
        }
        assert!(passes("write_session", corecmd::Lane::Send).is_ok());
        assert!(passes("ensure_codex_session", corecmd::Lane::Call).is_ok());
    }

    fn core(fingerprint: &str, sessions: usize, refused: Option<&str>) -> Welcome {
        Welcome {
            pid: 1,
            version: "0".into(),
            build: "b".into(),
            fingerprint: fingerprint.into(),
            started: 0,
            sessions,
            refused: refused.map(String::from),
        }
    }

    #[test]
    fn a_core_run_as_administrator_is_never_worked_with() {
        let ours = env!("LUNA_CORE_FP");
        // Of this very build, and elevated by its token: an older core says
        // nothing of it, so the window's own look is what counts.
        assert_eq!(verdict(&core(ours, 2, None), true, false), Verdict::Elevated, "sessions in it: the person restarts it");
        assert_eq!(verdict(&core(ours, 0, None), true, false), Verdict::Replace, "none: replaced at once");
        assert_eq!(verdict(&core("0000000000000000", 3, None), true, false), Verdict::Elevated);
        // One that says so itself, whatever the window could read of its token.
        assert_eq!(verdict(&core(ours, 0, Some(crate::elevation::ADMIN)), false, false), Verdict::Replace);
    }

    #[test]
    fn a_core_whose_token_cannot_be_read_is_not_attached() {
        let ours = env!("LUNA_CORE_FP");
        assert!(counts_as_elevated(None), "unread is not ordinary");
        assert!(counts_as_elevated(Some(true)));
        assert!(!counts_as_elevated(Some(false)));
        // Of this very build and answering: still not worked with.
        assert_eq!(verdict(&core(ours, 2, None), counts_as_elevated(None), false), Verdict::Elevated);
        assert_eq!(verdict(&core(ours, 0, None), counts_as_elevated(None), false), Verdict::Replace);
    }

    #[test]
    fn a_core_that_is_not_elevated_is_judged_by_its_build() {
        let ours = env!("LUNA_CORE_FP");
        assert_eq!(verdict(&core(ours, 2, None), false, false), Verdict::Attach);
        assert_eq!(verdict(&core("0000000000000000", 2, None), false, false), Verdict::Foreign);
        assert_eq!(verdict(&core("0000000000000000", 0, None), false, false), Verdict::Replace);
    }

    /// A core of protocol 1 is another build's whatever it says of itself:
    /// never attached, left alone with sessions, replaced without.
    #[test]
    fn a_protocol_1_core_is_never_attached() {
        let ours = env!("LUNA_CORE_FP");
        assert_eq!(verdict(&core(ours, 2, None), false, true), Verdict::Foreign);
        assert_eq!(verdict(&core(ours, 0, None), false, true), Verdict::Replace);
        assert_eq!(verdict(&core(ours, 1, None), true, true), Verdict::Elevated);
    }

    /// A Luna 0.11 core, found by the token in its `core.json`, hears a
    /// hello and, when it has no session, a shutdown: nothing else, whatever
    /// the window would otherwise ask. With sessions it is left alone for the
    /// person to restart (the mismatch banner).
    #[test]
    fn a_protocol_1_core_hears_a_hello_and_a_shutdown_and_nothing_else() {
        let idle = v1::tests::old_core(0);
        assert!(matches!(meet(&Found::V1(idle.found.clone()), false), Met::Replaced));
        assert_eq!(*idle.heard.lock().unwrap(), ["hello", "shutdown"]);
        let hello = idle.hello.lock().unwrap().clone().expect("a hello with the token");
        assert_eq!((hello["proto"].as_u64(), hello["role"].as_str()), (Some(1), Some("ui")));

        let busy = v1::tests::old_core(2);
        match meet(&Found::V1(busy.found.clone()), false) {
            Met::Done(Attached::Foreign(core)) => assert_eq!((core.sessions, core.version.as_str()), (2, "0.11.0")),
            _ => panic!("a protocol-1 core with sessions is left for the person"),
        }
        assert_eq!(*busy.heard.lock().unwrap(), ["hello"]);

        // `--stop-core` and Restart reach it the same way.
        let stopped = v1::tests::old_core(3);
        let (line, _) = greet(&Found::V1(stopped.found.clone()), "ctl").expect("hello");
        line.stop(stopped.found.pid);
        assert_eq!(*stopped.heard.lock().unwrap(), ["hello", "shutdown"]);
    }

    /// The core is started with where its data and log are and nothing else:
    /// no secret on its command line, none in an environment of its own.
    #[test]
    fn the_core_is_started_with_no_secret() {
        let cmd = core_command(std::path::Path::new("luna-core.exe"));
        let args: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        let flags: Vec<&str> = args.iter().map(String::as_str).filter(|a| a.starts_with("--")).collect();
        assert_eq!(flags, ["--core", "--data-dir", "--log", "--window-exe"]);
        assert_eq!(args.len(), 7, "{args:?}");
        assert_eq!(cmd.get_envs().count(), 0, "no variable set for it");
    }

    /// The window speaks to the endpoint only if the core `core.json` names
    /// serves it: one served by any other process hears nothing at all.
    #[test]
    fn an_endpoint_served_by_another_process_hears_nothing() {
        let name = crate::endpoint::tests::test_name();
        let listener = crate::endpoint::Listener::bind(&name).unwrap();
        let (tx, heard) = channel();
        std::thread::spawn(move || {
            let server = listener.accept().unwrap();
            server.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut got = Vec::new();
            let _ = (&server).read_to_end(&mut got);
            let _ = tx.send(got);
        });
        // The server waiting for its client before the client comes: one that
        // came and went before it was waited for is no client at all.
        std::thread::sleep(Duration::from_millis(300));
        // This process serves it; the file names another.
        let found = Discovery { pid: std::process::id() + 1, created_ms: 0, proto: ipc::PROTO, endpoint: name };
        let err = hello(&found, "ui").err().expect("refused");
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied, "{err}");
        let got = heard.recv_timeout(Duration::from_secs(10)).expect("the server saw the connection");
        assert!(got.is_empty(), "nothing was said to it: {got:?}");
    }
}
