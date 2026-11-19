//! The host of the native window (crates/window): what answers the window's
//! commands and feeds it its events. The window asks for what the page asks
//! for, by the same names and with the same camelCase arguments; the answers
//! come from the same functions, with no Tauri in between.
//!
//! - A command of the sessions' (`corecmd::COMMANDS`) goes to the core as the
//!   page's does through `core_call` and `core_send`.
//! - One of the window's own goes through `OWN` (called) or `OWN_SENT`, a
//!   name to a function over the Tauri commands' own functions (lib.rs
//!   `app_handler!`), and a few the native window alone has (its theme).
//! - The events sent to the UI (`emit::to_ui`) that the window listens to
//!   (`luna_window::event::ALL`) go into its inbox; the rest are dropped
//!   before they are serialized. A session's output goes to the sink the
//!   window routed for it (`Outputs`), if any, never through the inbox.
//!
//! Each call runs on a thread of its own, as `core_call` runs on a blocking
//! one: the window waits for the answer on a task, never on its thread.

use crate::corecmd::{self, Lane};
use luna_window::{Answer, Host, Mailbox, OutputSink, ThemePref};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

/// A command of the window's own.
type Command = fn(Value) -> Result<Value, String>;

/// Every command of the window's own it may call, by name.
const OWN: &[(&str, Command)] =
    &[("core_state", |_| serde_json::to_value(crate::corelink::core_state()).map_err(|e| e.to_string())), ("core_restart", core_restart)];

/// Every command of the window's own it may send, by name: taken on the
/// window's thread, so each hands its work over and returns at once.
const OWN_SENT: &[(&str, Command)] = &[("set_window_theme", set_window_theme)];

/// `{ revision }`: the link's state the restart was offered on.
fn core_restart(args: Value) -> Result<Value, String> {
    #[derive(Deserialize)]
    struct Args {
        revision: Option<u64>,
    }
    let Args { revision } = serde_json::from_value(args).map_err(|e| format!("bad arguments: {e}"))?;
    crate::corelink::core_restart(revision);
    Ok(Value::Null)
}

/// Answers one of the window's commands. Blocks for as long as it takes.
pub(crate) fn dispatch(cmd: &str, args: Value) -> Result<Value, String> {
    match corecmd::lane(cmd) {
        Some(Lane::Call) => crate::corelink::request(cmd.to_owned(), args),
        Some(Lane::Send) => Err(format!("{cmd} is sent, not called")),
        None => match OWN.iter().find(|(c, _)| *c == cmd) {
            Some((_, f)) => f(args),
            None => Err(format!("the window has no command {cmd}")),
        },
    }
}

pub(crate) struct NativeHost;

impl Host for NativeHost {
    fn call(&self, cmd: &str, args: Value) -> Answer {
        let (tx, rx) = futures::channel::oneshot::channel();
        let what = cmd.to_owned();
        let spawned = std::thread::Builder::new().name("luna-window-call".into()).spawn(move || {
            let _ = tx.send(dispatch(&what, args));
        });
        let cmd = cmd.to_owned();
        Box::pin(async move {
            spawned.map_err(|e| format!("cannot run {cmd}: {e}"))?;
            rx.await.unwrap_or_else(|_| Err(format!("{cmd} ended without an answer")))
        })
    }

    fn send(&self, cmd: &str, args: Value) -> Result<(), String> {
        match OWN_SENT.iter().find(|(c, _)| *c == cmd) {
            Some((_, f)) => f(args).map(drop),
            None => crate::corelink::core_send(cmd.to_owned(), args),
        }
    }

    fn route_output(&self, session: &str, sink: Option<Arc<dyn OutputSink>>) {
        outputs().route(session, sink);
    }
}

/// The sinks the window routed sessions' output to (`Host::route_output`),
/// by session id. Fed on the thread that reads the core's events (corelink.rs
/// `hold`), each chunk handed to its sink there: a session's output never
/// waits in the window's inbox, nor holds up what waits there.
#[derive(Default)]
struct Outputs(Mutex<HashMap<String, Arc<dyn OutputSink>>>);

impl Outputs {
    fn route(&self, session: &str, sink: Option<Arc<dyn OutputSink>>) {
        let mut sinks = self.0.lock().unwrap_or_else(|e| e.into_inner());
        match sink {
            Some(sink) => sinks.insert(session.to_owned(), sink),
            None => sinks.remove(session),
        };
    }

    /// Any routed at all: `pty://output` is not even serialized otherwise.
    fn any(&self) -> bool {
        !self.0.lock().unwrap_or_else(|e| e.into_inner()).is_empty()
    }

    /// One `pty://output` payload, `{ id, data }`, to its session's sink. A
    /// sink that says it is gone is let go of, unless another was routed in
    /// its place meanwhile.
    fn feed(&self, payload: &Value) {
        let (Some(id), Some(data)) = (payload.get("id").and_then(Value::as_str), payload.get("data").and_then(Value::as_str)) else {
            return;
        };
        let sink = self.0.lock().unwrap_or_else(|e| e.into_inner()).get(id).cloned();
        let Some(sink) = sink else { return };
        if !sink.bytes(data.as_bytes()) {
            let mut sinks = self.0.lock().unwrap_or_else(|e| e.into_inner());
            if sinks.get(id).is_some_and(|s| Arc::ptr_eq(s, &sink)) {
                sinks.remove(id);
            }
        }
    }
}

fn outputs() -> &'static Outputs {
    static OUTPUTS: OnceLock<Outputs> = OnceLock::new();
    OUTPUTS.get_or_init(Outputs::default)
}

/// The events the native window's side of `emit` takes: those its inbox
/// does, and a session's output while a sink is routed.
fn wanted(event: &str) -> bool {
    luna_window::event::wants(event) || (event == luna_window::event::PTY_OUTPUT && outputs().any())
}

/// What the native window keeps of its own between runs, in
/// `<data>/window.json`: the core's settings (settings.rs) are not the
/// window's to write.
#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct WindowSettings {
    theme: ThemePref,
}

fn settings_file() -> PathBuf {
    crate::paths::data_dir().join("window.json")
}

fn read_settings() -> WindowSettings {
    std::fs::read(settings_file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// The theme the person chose last; the system's when none was, or the file
/// cannot be read.
pub(crate) fn theme() -> ThemePref {
    read_settings().theme
}

/// `{ theme }`, sent on the window's thread: handed to the one writer
/// (`theme_writer`), so a later choice is never overwritten by an earlier one.
fn set_window_theme(args: Value) -> Result<Value, String> {
    #[derive(Deserialize)]
    struct Args {
        theme: ThemePref,
    }
    let Args { theme } = serde_json::from_value(args).map_err(|e| format!("bad arguments: {e}"))?;
    theme_writer().put(theme);
    Ok(Value::Null)
}

fn write_theme(theme: &ThemePref) -> Result<(), String> {
    let mut settings = read_settings();
    settings.theme = *theme;
    let json = serde_json::to_vec_pretty(&settings).map_err(|e| e.to_string())?;
    crate::secret::write_private(&settings_file(), &json).map_err(|e| format!("cannot keep the theme: {e}"))
}

fn theme_writer() -> &'static Arc<Writer<ThemePref>> {
    static WRITER: OnceLock<Arc<Writer<ThemePref>>> = OnceLock::new();
    WRITER.get_or_init(|| Writer::new("theme", write_theme))
}

/// How long quitting waits for the last theme chosen to be written.
const THEME_DRAIN: Duration = Duration::from_secs(2);

/// One writer of a value where only the latest counts, off the caller's
/// thread: each value put is numbered after the one before and replaces any
/// not written yet, and one thread writes them in turn, so an earlier value is
/// never written after a later one. `drain` waits for the last.
struct Writer<T> {
    what: &'static str,
    slot: Mutex<Slot<T>>,
    changed: Condvar,
    write: fn(&T) -> Result<(), String>,
}

struct Slot<T> {
    /// The latest value put and its number, while it waits to be written.
    next: Option<(u64, T)>,
    /// The number of the latest value put, and of the latest written (or
    /// failed: a failure is logged, and a later value may do better).
    put: u64,
    done: u64,
    /// The writing thread runs; the first `put` starts it.
    running: bool,
}

impl<T: Send + 'static> Writer<T> {
    fn new(what: &'static str, write: fn(&T) -> Result<(), String>) -> Arc<Writer<T>> {
        let slot = Slot { next: None, put: 0, done: 0, running: false };
        Arc::new(Writer { what, slot: Mutex::new(slot), changed: Condvar::new(), write })
    }

    /// Hands `value` over to be written; returns at once.
    fn put(self: &Arc<Self>, value: T) {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        slot.put += 1;
        slot.next = Some((slot.put, value));
        if !slot.running {
            let writer = Arc::clone(self);
            let name = format!("luna-{}-writer", self.what);
            slot.running = std::thread::Builder::new().name(name).spawn(move || writer.run()).is_ok();
            if !slot.running {
                // No thread to write it: written here, as it was put.
                let (n, value) = slot.next.take().expect("just put");
                let wrote = (self.write)(&value);
                self.finish(&mut slot, n, wrote);
                return;
            }
        }
        self.changed.notify_all();
    }

    fn run(&self) {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            slot = self.changed.wait_while(slot, |s| s.next.is_none()).unwrap_or_else(|e| e.into_inner());
            let (n, value) = slot.next.take().expect("waited for it");
            drop(slot);
            let wrote = (self.write)(&value);
            slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
            self.finish(&mut slot, n, wrote);
        }
    }

    fn finish(&self, slot: &mut Slot<T>, n: u64, wrote: Result<(), String>) {
        if let Err(e) = wrote {
            crate::log::warn("window", &format!("{}: {e}", self.what));
        }
        slot.done = n;
        self.changed.notify_all();
    }

    /// Waits until the latest value put is written, `within` at most; false
    /// if it was not by then.
    fn drain(&self, within: Duration) -> bool {
        let slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        let (slot, _) = self.changed.wait_timeout_while(slot, within, |s| s.done < s.put).unwrap_or_else(|e| e.into_inner());
        slot.done >= slot.put
    }
}

/// gpui's and the window's own warnings and errors, into Luna's log.
struct ToLunaLog;

impl log_facade::Log for ToLunaLog {
    fn enabled(&self, metadata: &log_facade::Metadata) -> bool {
        metadata.level() <= log_facade::Level::Warn
    }

    fn log(&self, record: &log_facade::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let message = match record.target() {
            "" => record.args().to_string(),
            target => format!("{target}: {}", record.args()),
        };
        match record.level() {
            log_facade::Level::Error => crate::log::error("window", &message),
            _ => crate::log::warn("window", &message),
        }
    }

    fn flush(&self) {}
}

/// Runs the native window until it is closed: the window, then, once it is
/// open, what `setup()` does for the WebView window that needs no Tauri. The
/// core runs on after it, as it does after the WebView window. Err with why
/// when the window could not open (no usable GPU device, shaders that do not
/// compile): nothing was started then, and the WebView window can take over
/// (lib.rs `run`). On macOS that never comes back: gpui ends the process.
pub(crate) fn run() -> Result<(), String> {
    if log_facade::set_boxed_logger(Box::new(ToLunaLog)).is_ok() {
        log_facade::set_max_level(log_facade::LevelFilter::Warn);
    }
    let (mailbox, inbox) = luna_window::mailbox();
    // The build stamp as well as the version: test builds between two
    // releases share a version, and only the commit tells them apart.
    crate::log::start(&format!(
        "starting luna {} build {} core {} (native window)",
        env!("CARGO_PKG_VERSION"),
        env!("LUNA_BUILD"),
        env!("LUNA_CORE_FP")
    ));
    let title = if crate::paths::is_test() { "Luna Test" } else { "Luna" };
    let services = mailbox.clone();
    let options = luna_window::Options {
        title: title.into(),
        theme: theme(),
        background: crate::background(),
        on_open: Box::new(move || start(services)),
        on_quit: Box::new(|| {
            crate::lifecycle::exiting("the window was closed; the core and its sessions stay");
            if !theme_writer().drain(THEME_DRAIN) {
                crate::log::warn("window", "the theme chosen last was not written in time");
            }
            // A Claude Code model ask may still be saving a renewed token;
            // its cleanup runs only while Luna does.
            crate::claude::models::wait_for_asks();
            crate::emit::stop();
            crate::power::clear();
        }),
    };
    // `mailbox` lives as long as the window: dropping the last one would end
    // the window's inbox.
    let ran = luna_window::run(Arc::new(NativeHost), inbox, options);
    drop(mailbox);
    ran
}

/// What runs beside the open window, from its thread.
fn start(mailbox: Mailbox) {
    // Events reach the window through its inbox, a session's output its
    // sink; installed before anything that could send one starts.
    let events = mailbox.clone();
    crate::emit::install_for(wanted, move |event, payload| {
        if event == luna_window::event::PTY_OUTPUT {
            outputs().feed(&payload);
        } else {
            events.event(event, payload);
        }
    });
    // The previous run's end is reported before the heartbeat starts; it
    // pings the window's thread through its inbox. No single-instance check
    // runs before it yet: the native window's desktop services (single
    // instance, tray, notifications, window state) come in a later change.
    crate::lifecycle::start(Box::new(move |f| mailbox.run_on_ui(f)));
    // The sessions, the hooks' listener, the agents and power all run in the
    // core (coreproc.rs); the window finds or starts it.
    crate::corelink::start();
    // Installs the CLIs on a fresh machine and keeps them current after.
    crate::cli::refresh_periodically();
    // A parent account's settings onto the others, when that is asked for.
    crate::account_settings::sync_on_start();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Every command the window calls has an answer here, every one it
    /// sends a taker, and none of the window's own shadows one of the core's.
    #[test]
    fn every_command_the_window_names_is_dispatched() {
        for cmd in luna_window::cmd::ALL {
            let own = OWN.iter().any(|(c, _)| c == cmd);
            assert!(own || corecmd::lane(cmd) == Some(Lane::Call), "{cmd} has no answer in native.rs");
        }
        for cmd in luna_window::cmd::SENT {
            let own = OWN_SENT.iter().any(|(c, _)| c == cmd);
            assert!(own || corecmd::lane(cmd) == Some(Lane::Send), "{cmd} has no taker in native.rs");
        }
        for (cmd, _) in OWN.iter().chain(OWN_SENT) {
            assert!(corecmd::lane(cmd).is_none(), "{cmd} is the core's");
        }
        assert!(dispatch("nope", Value::Null).unwrap_err().contains("no command"));
        assert!(dispatch("write_session", json!({})).unwrap_err().contains("sent, not called"));
    }

    /// The window's own commands that are Tauri commands too are the ones
    /// the WebView window registers, by the same name: the two windows ask
    /// for the same thing the same way.
    #[test]
    fn the_windows_own_commands_are_the_pages_where_the_page_has_them() {
        let lib = include_str!("lib.rs");
        let start = lib.find("generate_handler![").unwrap();
        let registered = &lib[start..start + lib[start..].find(']').unwrap()];
        let native_only = ["set_window_theme"];
        for (cmd, _) in OWN.iter().chain(OWN_SENT) {
            let tauri = registered.contains(&format!("::{cmd},"));
            assert!(tauri != native_only.contains(cmd), "{cmd}: registered {tauri}");
        }
    }

    /// Every event the window listens to is one Luna sends.
    #[test]
    fn every_event_the_window_hears_is_sent() {
        let corelink = include_str!("corelink.rs");
        for event in luna_window::event::ALL {
            assert!(corelink.contains(&format!("\"{event}\"")), "{event} is sent nowhere");
        }
        let output = luna_window::event::PTY_OUTPUT;
        assert!(include_str!("pty/spawn.rs").contains(&format!("\"{output}\"")), "{output} is sent nowhere");
    }

    struct Pane(Mutex<Vec<u8>>, bool);

    impl OutputSink for Pane {
        fn bytes(&self, data: &[u8]) -> bool {
            self.0.lock().unwrap().extend_from_slice(data);
            self.1
        }
    }

    /// Only the events the window takes reach its side of `emit`; a
    /// session's output goes to the sink routed for it, and to nothing else,
    /// and is not even taken while none is.
    #[test]
    fn a_sessions_output_goes_to_its_sink_and_nothing_else_comes_in_but_what_the_window_takes() {
        assert!(wanted(luna_window::event::CORE_STATE));
        assert!(!wanted("agents://changed"));
        assert!(!wanted(luna_window::event::PTY_OUTPUT), "no sink routed");
        let outputs = Outputs::default();
        let (a, b) = (Arc::new(Pane(Mutex::default(), true)), Arc::new(Pane(Mutex::default(), false)));
        outputs.route("a", Some(a.clone()));
        outputs.route("b", Some(b.clone()));
        assert!(outputs.any());
        outputs.feed(&json!({ "id": "a", "data": "hello " }));
        outputs.feed(&json!({ "id": "c", "data": "nobody's" }));
        outputs.feed(&json!({ "id": "a", "data": "world" }));
        outputs.feed(&json!({ "id": "a" }));
        assert_eq!(*a.0.lock().unwrap(), b"hello world");
        // A sink that says it is gone is let go of after its last chunk.
        outputs.feed(&json!({ "id": "b", "data": "x" }));
        outputs.feed(&json!({ "id": "b", "data": "y" }));
        assert_eq!(*b.0.lock().unwrap(), b"x");
        outputs.route("a", None);
        outputs.feed(&json!({ "id": "a", "data": "!" }));
        assert_eq!(*a.0.lock().unwrap(), b"hello world");
        assert!(!outputs.any());
    }

    /// Theme choices made faster than they are written: the last one is the
    /// one kept, an earlier one is never written after it, and `drain` waits
    /// for it.
    #[test]
    fn the_theme_writer_keeps_the_latest_choice_and_drains_it() {
        static WRITTEN: Mutex<Vec<u32>> = Mutex::new(Vec::new());
        fn slow(v: &u32) -> Result<(), String> {
            std::thread::sleep(Duration::from_millis(30));
            WRITTEN.lock().unwrap().push(*v);
            if *v == 3 {
                Err("disk full".into())
            } else {
                Ok(())
            }
        }
        let writer = Writer::new("test", slow);
        for v in 1..=20 {
            writer.put(v);
        }
        assert!(writer.drain(Duration::from_secs(10)));
        let written = WRITTEN.lock().unwrap().clone();
        assert_eq!(written.last(), Some(&20));
        assert!(written.windows(2).all(|w| w[0] < w[1]), "in order: {written:?}");
        assert!(written.len() < 20, "superseded choices are skipped: {written:?}");
        // A failure is logged and does not stop the next.
        writer.put(3);
        writer.put(21);
        assert!(writer.drain(Duration::from_secs(10)));
        assert_eq!(WRITTEN.lock().unwrap().last(), Some(&21));
        // Nothing waiting: drained at once.
        assert!(writer.drain(Duration::ZERO));
    }

    /// The core's state as the core link holds it, read back as the window
    /// reads it: every state, with a core and without.
    #[test]
    fn the_core_state_reaches_the_window_as_it_was() {
        use luna_window::dto::{CoreState, LinkState as Seen};
        let core = crate::ipc::Welcome {
            pid: 7,
            version: "0.20.0".into(),
            build: "abc1234".into(),
            fingerprint: "0011223344556677".into(),
            started: 1_763_500_000_000,
            sessions: 3,
            refused: Some("elevated".into()),
        };
        for (state, seen) in [
            ("starting", Seen::Starting),
            ("up", Seen::Up),
            ("down", Seen::Down),
            ("mismatch", Seen::Mismatch),
            ("elevated", Seen::Elevated),
        ] {
            for (core, error) in [(None, None), (Some(core.clone()), Some("why".to_string()))] {
                let sent = crate::corelink::LinkState { state, core: core.clone(), error: error.clone(), revision: 9 };
                let got: CoreState = serde_json::from_value(serde_json::to_value(&sent).unwrap()).unwrap();
                assert_eq!(got.state, seen);
                assert_eq!(got.error, error);
                assert_eq!(got.revision, 9);
                let Some(core) = core else {
                    assert!(got.core.is_none());
                    continue;
                };
                let got = got.core.unwrap();
                assert_eq!(
                    (got.pid, got.version, got.build, got.fingerprint, got.started, got.sessions, got.refused),
                    (core.pid, core.version, core.build, core.fingerprint, core.started, core.sessions, core.refused)
                );
            }
        }
        // A core that does not say `refused` (one older than the check) reads
        // as none.
        let older = json!({ "state": "up", "core": { "pid": 1, "version": "0.11.0", "build": "x", "fingerprint": "y", "started": 0 }, "error": null });
        assert_eq!(serde_json::from_value::<CoreState>(older).unwrap().core.unwrap().refused, None);
    }

    /// The theme's settings file takes what the window sends and gives back
    /// the system's for anything it cannot read.
    #[test]
    fn the_theme_is_read_as_the_window_names_it() {
        for (text, pref) in [("\"light\"", ThemePref::Light), ("\"dark\"", ThemePref::Dark), ("\"system\"", ThemePref::System)] {
            let s: WindowSettings = serde_json::from_str(&format!("{{\"theme\":{text}}}")).unwrap();
            assert_eq!(s.theme, pref);
        }
        assert!(serde_json::from_str::<WindowSettings>("{\"theme\":\"sepia\"}").is_err());
        assert_eq!(serde_json::from_str::<WindowSettings>("{}").unwrap().theme, ThemePref::System);
        assert!(set_window_theme(json!({ "theme": "sepia" })).unwrap_err().starts_with("bad arguments"));
    }
}
