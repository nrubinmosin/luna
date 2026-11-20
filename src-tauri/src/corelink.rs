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

use crate::backlog::Backlog;
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
/// How long a connect to a core's endpoint gets.
const CONNECT_WAIT: Duration = Duration::from_secs(2);
/// How long a core gets to answer a hello.
const HELLO_WAIT: Duration = Duration::from_secs(5);
/// What a core that would not stop in `STOP_WAIT` is allowed for its kill
/// (`pty::kill_tree`, its whole tree, which gives `taskkill` 5 s).
const KILL_SLACK: Duration = Duration::from_secs(5);
/// How long a request waits for a core while one is found or started, and
/// while the one running is replaced (a restart, or another build's after an
/// update): a budget for a hello to the old one, its stop and kill, the new
/// one's start and a hello to it, 7 + 30 + 5 + 10 + 7 = 59 s. Not a bound:
/// each step's timeout is per read or per call (a hello's answer read in
/// frames, a launch), so a slow replacement can take longer, and a request
/// waiting then fails as with no core. The old core's stop asks its sessions to
/// quit within `pty::GRACE` (10 s) inside `STOP_WAIT`; an update's
/// `stop_all_sessions` runs in the old window before the install, outside
/// this wait.
const LINK_WAIT: Duration = Duration::from_secs(
    2 * (CONNECT_WAIT.as_secs() + HELLO_WAIT.as_secs()) + STOP_WAIT.as_secs() + KILL_SLACK.as_secs() + START_WAIT.as_secs(),
);

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct LinkState {
    /// starting | up | down | mismatch | elevated
    pub state: &'static str,
    pub core: Option<Welcome>,
    pub error: Option<String>,
    /// One more with every state set (`set`): the native window tells a
    /// state that came late, after a newer one, by it, and a restart asked
    /// for a state that has moved on since (`core_restart`). The page reads
    /// no further than `state`, `core` and `error`.
    pub revision: u64,
}

/// The connection in use: its writing end and the requests waiting on it.
/// `pending` is None once the connection has ended: a request let in after
/// the last of them heard so would wait for an answer nothing can bring.
///
/// Frames go out through `out` to one thread of the connection's own
/// (`writer`), in the order they were handed over. A write blocks while the
/// core is not reading, and `core_send` runs on the window's main thread:
/// writing there itself, under a lock that request threads held through
/// their own writes, it held the window up behind them.
///
/// What waits to be written is bounded by its bytes (`OUT_MAX`): past that a
/// frame is refused with an error, and so is every one after it until the
/// writer has caught up with all of them, so the frames that do go out keep
/// the order they were handed over in with no gap between them.
struct Conn {
    out: Sender<Vec<u8>>,
    backlog: Arc<Backlog>,
    /// Bytes past which frames are refused (`OUT_MAX`).
    max: usize,
    pending: Mutex<Option<HashMap<u64, Sender<Result<Value, String>>>>>,
}

/// How many bytes of frames may wait for the writer: keystrokes and a paste
/// or two, many times over, where a core that has stopped reading would
/// otherwise have the window hold whatever is typed at it.
const OUT_MAX: usize = 8 * 1024 * 1024;

impl Conn {
    /// A connection writing to `stream` from a thread of its own.
    fn over(stream: impl Write + Send + 'static) -> Arc<Conn> {
        Self::over_bounded(stream, OUT_MAX)
    }

    /// `over`, refusing past `max` bytes waiting.
    fn over_bounded(stream: impl Write + Send + 'static, max: usize) -> Arc<Conn> {
        Arc::new_cyclic(|conn| {
            let (out, frames) = channel();
            let backlog = Arc::new(Backlog::default());
            let (conn, written) = (std::sync::Weak::clone(conn), Arc::clone(&backlog));
            let spawned = std::thread::Builder::new().name("luna-core-writer".into()).spawn(move || writer(stream, frames, conn, &written));
            if let Err(e) = spawned {
                // No writer: every frame handed over is refused, as to a
                // core that cannot be reached.
                crate::log::error("core", &format!("cannot start the connection's writer: {e}"));
            }
            Conn { out, backlog, pending: Mutex::new(Some(HashMap::new())), max }
        })
    }

    /// Hands a frame to the writer; Err once it has stopped, and while what
    /// waits for it is over its bound (`Backlog`). One frame alone may be
    /// larger than the bound.
    fn send(&self, frame: Vec<u8>) -> Result<(), String> {
        let len = frame.len() as u64;
        if let Err(refused) = self.backlog.reserve_in_order(len, self.max as u64) {
            let kb = refused.waiting / 1024;
            if refused.first {
                crate::log::warn("core", &format!("the core is not taking input: {kb} KB waiting to be written; refusing more"));
            }
            return Err(format!("the core is not taking input: {kb} KB waiting to be written"));
        }
        self.out.send(frame).map_err(|_| {
            self.backlog.release(len);
            "the core is not reachable".to_string()
        })
    }
}

/// Writes the frames handed over, in order, until every sender is gone (the
/// connection let go) or a write fails. On a failure the connection is over:
/// it is logged, whoever waits on an answer hears that the core cannot be
/// reached, no request is let in after, and no frame is taken.
fn writer(mut stream: impl Write, frames: std::sync::mpsc::Receiver<Vec<u8>>, conn: std::sync::Weak<Conn>, backlog: &Backlog) {
    for frame in frames {
        if let Err(e) = stream.write_all(&frame).and_then(|_| stream.flush()) {
            let waiting = conn.upgrade().and_then(|c| c.pending.lock().unwrap_or_else(|e| e.into_inner()).take());
            let waiting: Vec<_> = waiting.into_iter().flatten().collect();
            crate::log::warn("core", &format!("writing to the core failed: {e}; {} request(s) told so", waiting.len()));
            for (_, tx) in waiting {
                let _ = tx.send(Err(format!("the core is not reachable: {e}")));
            }
            return;
        }
        backlog.release(frame.len() as u64);
    }
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

/// The link's new state, numbered after the one before it. Emitted outside
/// the lock, so two set at once can reach the window in either order: the
/// revision says which is the latest.
fn set(mut state: LinkState) -> u64 {
    let revision = {
        let mut link = link().lock().unwrap_or_else(|e| e.into_inner());
        state.revision = link.revision + 1;
        *link = state.clone();
        state.revision
    };
    crate::emit::to_ui("core://state", state);
    revision
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
                    set(LinkState { state: "down", core: None, error: Some("cannot use the connection".into()), ..Default::default() });
                    return;
                };
                let conn = Conn::over(writer);
                // A restart has taken over from here, or is stopping this core
                // to start the next: no core for this connection to stand for.
                // Looked at under the lock every taking of the line holds, so a
                // way back that took its time cannot land over a restart's.
                // `up` is the link revision of this core's `up`, which its
                // events are stamped with (`status::event::stamp`).
                let up = {
                    let _taking = TAKING.lock().unwrap_or_else(|e| e.into_inner());
                    if GENERATION.load(Ordering::SeqCst) != generation || (rejoined && RESTARTING.load(Ordering::SeqCst)) {
                        return;
                    }
                    if rejoined {
                        // The panes start over, as after any core's start.
                        set(LinkState { state: "starting", ..Default::default() });
                    }
                    *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::clone(&conn));
                    set(LinkState { state: "up", core: Some(core), error: None, ..Default::default() })
                };
                hold(stream, conn, up);
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
                set(LinkState { state: "down", core: None, error: None, ..Default::default() });
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
            set(LinkState { state: "mismatch", core: Some(core), error: None, ..Default::default() });
        }
        Ok(Attached::Elevated { core, why }) => {
            crate::log::error("core", &why);
            set(LinkState { state: "elevated", core, error: Some(why), ..Default::default() });
        }
        Err(e) => {
            crate::log::warn("core", &format!("no core: {e}"));
            set(LinkState { state: "down", core: None, error: Some(e), ..Default::default() });
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
///
/// `up` is the link revision of the `up` that began this connection: the
/// power rule's states carry it (`status::event::stamp`), so the window can
/// tell a late state of a core since replaced from the new core's.
fn hold(mut stream: Stream, conn: Arc<Conn>, up: u64) {
    while let Ok((kind, payload)) = ipc::read_frame(&mut stream) {
        match kind {
            ipc::EVENT => {
                if let Ok(mut ev) = serde_json::from_slice::<Event>(&payload) {
                    luna_window::status::event::stamp(&ev.event, &mut ev.payload, up);
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
    /// An older version's, its sessions stopped for an update
    /// (`Welcome::stopped_for_update`), whose sessions are all what idle
    /// helpers' turns left (`Welcome::left_only`), as that stop leaves it: replaced at
    /// once if it says so still as it is asked to stop (`stop_if_left_only`),
    /// else left as `Foreign`.
    ReplaceLeftOnly,
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
    if !v1 && core.fingerprint == env!("LUNA_CORE_FP") {
        return Verdict::Attach;
    }
    if core.sessions == 0 {
        return Verdict::Replace;
    }
    // What idle helpers' turns left is no work of the person's to ask about
    // when this window is the update of that core: one of an older version
    // whose sessions were stopped for an update, a stop the person agreed to,
    // which ended what it could; the core's going ends the rest. Not a core
    // left by an install by hand beside it, nor a newer version's, nor the same
    // version's built otherwise (a dev build). A pane or a turn is work.
    let older = crate::cli::version_cmp(&core.version, env!("CARGO_PKG_VERSION")).is_lt();
    if !v1 && older && core.stopped_for_update && core.sessions == core.left_only {
        return Verdict::ReplaceLeftOnly;
    }
    Verdict::Foreign
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
        Verdict::ReplaceLeftOnly => {
            let Line::Current(stream) = line else { return Met::Done(Attached::Foreign(core)) };
            crate::log::warn(
                "core",
                &format!(
                    "the running core is an older version's (build {}, core {}) with only what helpers' turns left running; replacing it",
                    core.build, core.fingerprint
                ),
            );
            if stop_if_left_only(stream, found.pid()) {
                Met::Replaced
            } else {
                Met::Done(Attached::Foreign(core))
            }
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
    let mut stream = Stream::connect(&found.endpoint, CONNECT_WAIT)?;
    let server = stream.server_pid()?;
    if server != found.pid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("the core's endpoint is served by pid {server}, not by the core's pid {}", found.pid),
        ));
    }
    stream.set_read_timeout(Some(HELLO_WAIT))?;
    let args = serde_json::to_value(Hello { proto: ipc::PROTO, role: role.into(), fingerprint: Some(env!("LUNA_CORE_FP").into()) })?;
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
fn stop<S: Read + Write + crate::endpoint::Timeouts>(stream: S, pid: u32) {
    stop_as(stream, pid, Value::Null);
}

/// `stop`, but only if the core still holds nothing but what helpers' turns
/// left (`Verdict::ReplaceLeftOnly`): it looks again as it is asked, and
/// refuses with a pane or a helper's turn in it (coreproc.rs `left_only_now`).
/// Says whether it went; one that refused is left running, untouched.
fn stop_if_left_only<S: Read + Write + crate::endpoint::Timeouts>(stream: S, pid: u32) -> bool {
    stop_as(stream, pid, serde_json::json!({ "ifLeftOnly": true }))
}

/// Asks the core to stop with `args` and, unless it refuses, waits for it to
/// be gone, as `stop` does. Says whether it went.
fn stop_as<S: Read + Write + crate::endpoint::Timeouts>(mut stream: S, pid: u32, args: Value) -> bool {
    let _ = ipc::send(&mut stream, ipc::REQ, &Req { id: 2, cmd: "shutdown".into(), args });
    let deadline = std::time::Instant::now() + STOP_WAIT;
    let gone = loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() || stream.set_read_timeout(Some(left)).is_err() {
            break false;
        }
        match ipc::read_frame(&mut stream) {
            Ok((ipc::RES, payload)) => {
                let refused = serde_json::from_slice::<Res>(&payload).ok().filter(|res| res.id == 2).and_then(|res| res.err);
                if let Some(why) = refused {
                    crate::log::info("core", &format!("the core (pid {pid}) would not stop: {why}"));
                    return false;
                }
            }
            Ok(_) => {}
            Err(e) => break !matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut),
        }
    };
    if !gone {
        crate::log::warn("core", &format!("the core (pid {pid}) did not stop within {}s; killing it", STOP_WAIT.as_secs()));
        crate::pty::kill_tree(Some(pid));
    }
    true
}

/// Asks the core a command and waits for the answer.
pub(crate) fn request(cmd: String, args: Value) -> Result<Value, String> {
    request_within(cmd, args, None)
}

/// `request`, waiting `within` at most where given, a core still starting
/// included: past it the request is given up, its entry gone from the
/// connection's pending ones, so a caller that stopped waiting (the native
/// window's look at the sessions, every few seconds) leaves no thread
/// blocked behind it on a connection that has stopped answering.
pub(crate) fn request_within(cmd: String, args: Value, within: Option<Duration>) -> Result<Value, String> {
    let started = std::time::Instant::now();
    // The panes ask for their sessions as soon as the page is up, which can be
    // before the core is: while it is being found, started or replaced, they
    // wait rather than fail.
    let deadline = started + within.map_or(LINK_WAIT, |w| w.min(LINK_WAIT));
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
    ask(&conn, cmd, args, within.map(|w| w.saturating_sub(started.elapsed())))
}

/// One request on `conn`, its answer waited for `within` at most where
/// given: past it, its pending entry goes.
fn ask(conn: &Conn, cmd: String, args: Value, within: Option<Duration>) -> Result<Value, String> {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let frame = ipc::encode(ipc::REQ, &Req { id, cmd, args }).map_err(|e| e.to_string())?;
    let (tx, rx) = channel();
    match conn.pending.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        Some(pending) => pending.insert(id, tx),
        None => return Err("the core stopped".into()),
    };
    let forget = || {
        if let Some(pending) = conn.pending.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            pending.remove(&id);
        }
    };
    if let Err(e) = conn.send(frame) {
        forget();
        return Err(e);
    }
    let Some(within) = within else { return rx.recv().unwrap_or_else(|_| Err("the core stopped".into())) };
    match rx.recv_timeout(within) {
        Ok(answer) => answer,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            forget();
            // An answer that came as the entry went is still the answer.
            rx.try_recv().unwrap_or_else(|_| Err(format!("no answer from the core within {} ms", within.as_millis())))
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err("the core stopped".into()),
    }
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
/// in the new one — and starts one of this build. `revision`: that of the
/// state the restart was offered on (the native window's banner); nothing
/// happens once the link has moved on from it, as a core that came back
/// meanwhile is not one to stop. The page passes none.
#[tauri::command]
pub fn core_restart(revision: Option<u64>) {
    let link = link().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(asked) = revision.filter(|&r| r != link.revision) {
        crate::log::info("core", &format!("a restart asked for state {asked} is not done: the link is at state {} now", link.revision));
        return;
    }
    // Under the link's lock, so no state comes between the check and this.
    if RESTARTING.swap(true, Ordering::SeqCst) {
        return;
    }
    drop(link);
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

/// Keystrokes and resizes: handed to the connection's writer in the order
/// they come, and not waited on, written or answered. Sync, so the page's
/// calls keep their order; the write is the writer's (`Conn`).
#[tauri::command]
pub fn core_send(cmd: String, args: Value) -> Result<(), String> {
    passes(&cmd, corecmd::Lane::Send)?;
    let conn = CURRENT.lock().unwrap_or_else(|e| e.into_inner()).clone().ok_or("the core is not running")?;
    let frame = ipc::encode(ipc::SEND, &Req { id: 0, cmd, args }).map_err(|e| e.to_string())?;
    conn.send(frame)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every state set is numbered after the one before it, and a restart
    /// asked for a state the link has moved on from starts nothing.
    #[test]
    fn states_are_numbered_and_a_restart_for_a_past_one_does_nothing() {
        let before = core_state().revision;
        set(LinkState { state: "down", ..Default::default() });
        let after = core_state().revision;
        assert!(after > before);
        core_restart(Some(after - 1));
        assert!(!RESTARTING.load(Ordering::SeqCst));
        assert_eq!(core_state().revision, after, "nothing was set");
        let sent = crate::emit::sent_for_test("core://state");
        assert!(sent.iter().any(|v| v["revision"] == after && v["state"] == "down"), "the event carries it");
    }

    /// A pipe end whose writes wait until `open`, as a core that is not
    /// reading holds them, or fail at once with `fail`.
    struct Pipe {
        open: Arc<(Mutex<bool>, std::sync::Condvar)>,
        got: Arc<Mutex<Vec<u8>>>,
        fail: bool,
    }

    impl Write for Pipe {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            if self.fail {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "gone"));
            }
            let (lock, cv) = &*self.open;
            let _open = cv.wait_while(lock.lock().unwrap(), |open| !*open).unwrap();
            self.got.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Keystrokes are handed over, never written by the caller: a core that
    /// is not reading holds none of them, and they go out in the order sent.
    #[test]
    fn a_send_never_waits_on_the_write_and_frames_keep_their_order() {
        let open = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let got = Arc::new(Mutex::new(Vec::new()));
        let conn = Conn::over(Pipe { open: Arc::clone(&open), got: Arc::clone(&got), fail: false });
        let started = std::time::Instant::now();
        for frame in [b"one ".to_vec(), b"two ".to_vec(), b"three".to_vec()] {
            conn.send(frame).unwrap();
        }
        assert!(started.elapsed() < Duration::from_secs(1), "handed over, not written");
        assert!(got.lock().unwrap().is_empty(), "the pipe holds them");
        *open.0.lock().unwrap() = true;
        open.1.notify_all();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while got.lock().unwrap().len() < 13 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(&got.lock().unwrap()[..], b"one two three");
    }

    /// A core that does not read: past the bound each frame is refused with
    /// an error, and so is every one after until all that waited is written;
    /// what went out went in order, with no gap.
    #[test]
    fn past_its_bound_input_is_refused_until_the_writer_has_caught_up() {
        let open = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let got = Arc::new(Mutex::new(Vec::new()));
        let conn = Conn::over_bounded(Pipe { open: Arc::clone(&open), got: Arc::clone(&got), fail: false }, 10);
        conn.send(b"aaaa".to_vec()).unwrap();
        conn.send(b"bbbb".to_vec()).unwrap();
        let refused = conn.send(b"cccc".to_vec()).unwrap_err();
        assert!(refused.contains("not taking input"), "{refused}");
        assert!(conn.send(b"d".to_vec()).is_err(), "nothing slips in behind a refusal");
        *open.0.lock().unwrap() = true;
        open.1.notify_all();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while conn.backlog.waiting() > 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        conn.send(b"eeee".to_vec()).unwrap();
        while got.lock().unwrap().len() < 12 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(&got.lock().unwrap()[..], b"aaaabbbbeeee");
        // One frame larger than the bound goes when nothing waits.
        conn.send(vec![b'f'; 64]).unwrap();
    }

    /// A request bounded in time is given up once the time is over, on a
    /// connection that takes it and never answers, and its pending entry
    /// goes with it: nothing waits behind it.
    #[test]
    fn a_bounded_request_gives_up_and_leaves_nothing_pending() {
        let open = Arc::new((Mutex::new(true), std::sync::Condvar::new()));
        let conn = Conn::over(Pipe { open, got: Arc::default(), fail: false });
        let started = std::time::Instant::now();
        let err = ask(&conn, "sessions_meta".into(), Value::Null, Some(Duration::from_millis(100))).unwrap_err();
        assert!(err.contains("no answer from the core within 100 ms"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(conn.pending.lock().unwrap().as_ref().unwrap().is_empty(), "its entry is gone");
    }

    /// A write that fails ends the connection: who waits on an answer hears
    /// so, and nothing is taken after.
    #[test]
    fn a_failed_write_tells_the_waiting_and_takes_nothing_more() {
        let conn = Conn::over(Pipe { open: Arc::default(), got: Arc::default(), fail: true });
        let (tx, rx) = channel();
        conn.pending.lock().unwrap().as_mut().unwrap().insert(1, tx);
        conn.send(b"frame".to_vec()).unwrap();
        let heard = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(heard.unwrap_err().contains("not reachable"));
        assert!(conn.pending.lock().unwrap().is_none(), "no request let in after");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while conn.send(b"more".to_vec()).is_ok() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(conn.send(b"more".to_vec()).is_err(), "the writer is gone");
    }

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
            left_only: 0,
            stopped_for_update: false,
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

    /// After an update, the old core may have nothing in it but what idle
    /// helpers' turns left running (their CLI Claude Code's or Codex's alike):
    /// replaced as one with no sessions, its going ending those jobs. A pane or
    /// a helper's turn beside them is work to ask about, and so is any session
    /// of a core older than the count, which does not say what is left only.
    #[test]
    fn a_core_of_an_older_version_with_only_helpers_leftovers_is_replaced() {
        let other = |sessions, left_only| Welcome { left_only, stopped_for_update: true, ..core("0000000000000000", sessions, None) };
        assert_eq!(verdict(&other(2, 2), false, false), Verdict::ReplaceLeftOnly, "leftovers only");
        // No update stopped it: one left running by an install by hand beside it.
        assert_eq!(verdict(&Welcome { stopped_for_update: false, ..other(2, 2) }, false, false), Verdict::Foreign, "no update");
        assert_eq!(verdict(&other(3, 2), false, false), Verdict::Foreign, "a turn or a pane beside them");
        // Only the update of that core: not a newer version's, nor the same
        // version's built otherwise (a dev build), which nothing here stopped.
        let at = |version: &str| Welcome { version: version.into(), ..other(2, 2) };
        assert_eq!(verdict(&at(env!("CARGO_PKG_VERSION")), false, false), Verdict::Foreign, "same version");
        assert_eq!(verdict(&at("999.0.0"), false, false), Verdict::Foreign, "a newer version");
        assert_eq!(verdict(&at("0.0.1"), false, false), Verdict::ReplaceLeftOnly, "an older version");
        assert_eq!(verdict(&at("0.0.1"), false, true), Verdict::Foreign, "protocol 1, which says no such thing");
        // By semver's order: a beta is older than its release, so a beta's core
        // is the released window's to replace, and not the other way round.
        assert!(crate::cli::version_cmp("0.20.0-beta.1", "0.20.0").is_lt());
        assert!(crate::cli::version_cmp("0.20.0", "0.20.0-beta.1").is_gt());
        assert_eq!(verdict(&Welcome { left_only: 2, ..core(env!("LUNA_CORE_FP"), 2, None) }, false, false), Verdict::Attach);
        assert_eq!(verdict(&Welcome { left_only: 1, ..core(env!("LUNA_CORE_FP"), 1, None) }, true, false), Verdict::Elevated);
        let older: Welcome = serde_json::from_value(serde_json::json!({
            "pid": 1, "version": "0.19.0", "build": "b", "fingerprint": "0000000000000000", "started": 0, "sessions": 1
        }))
        .unwrap();
        assert_eq!((older.left_only, older.stopped_for_update), (0, false));
        assert_eq!(verdict(&older, false, false), Verdict::Foreign, "an older core's session is asked about, as ever");
    }

    /// A core asked to stop only if it holds nothing but what helpers' turns
    /// left looks again: one that refuses (a pane or a turn since its hello)
    /// is left running, not killed, and the window treats it as `Foreign`;
    /// one that agrees goes as any stop.
    #[test]
    fn a_core_that_refuses_a_stop_if_left_only_is_left_running() {
        let fake_core = |refuse: bool| {
            let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let port = listener.local_addr().unwrap().port();
            let heard = std::thread::spawn(move || {
                let (mut s, _) = listener.accept().unwrap();
                let req: Req = ipc::recv(&mut s, ipc::REQ).unwrap();
                let res = if refuse {
                    Res { id: req.id, ok: None, err: Some("the core has a session or a helper in a turn now".into()) }
                } else {
                    Res { id: req.id, ok: Some(Value::Null), err: None }
                };
                ipc::send(&mut s, ipc::RES, &res).unwrap();
                if refuse {
                    // Still there: the window must not wait on it to go.
                    std::thread::sleep(Duration::from_secs(2));
                }
                req
            });
            (std::net::TcpStream::connect(("127.0.0.1", port)).unwrap(), heard)
        };
        let (stream, heard) = fake_core(true);
        let started = std::time::Instant::now();
        // A pid no process has: were it killed, nothing would be.
        assert!(!stop_if_left_only(stream, u32::MAX - 7), "refused");
        assert!(started.elapsed() < Duration::from_secs(2), "not waited on to go");
        let req = heard.join().unwrap();
        assert_eq!((req.cmd.as_str(), &req.args), ("shutdown", &serde_json::json!({ "ifLeftOnly": true })));
        let (stream, heard) = fake_core(false);
        assert!(stop_if_left_only(stream, u32::MAX - 7), "gone");
        heard.join().unwrap();
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
