//! The host of the native window (crates/window): what answers the window's
//! commands and feeds it its events. The window asks for what the page asks
//! for, by the same names and with the same camelCase arguments; the answers
//! come from the same functions, with no Tauri in between.
//!
//! - A command of the sessions' (`corecmd::COMMANDS`) goes to the core as the
//!   page's does through `core_call` and `core_send`.
//! - One of the window's own goes through `OWN`, a name to a function over
//!   the Tauri commands' own functions (lib.rs `app_handler!`), and a few the
//!   native window alone has (its theme).
//! - Every event sent to the UI (`emit::to_ui`) goes into the window's inbox.
//!
//! Each call runs on a thread of its own, as `core_call` runs on a blocking
//! one: the window waits for the answer on a task, never on its thread.

use crate::corecmd::{self, Lane};
use luna_window::{Answer, Host, Mailbox, ThemePref};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

/// A command of the window's own.
type Command = fn(Value) -> Result<Value, String>;

/// Every command of the window's own it may call, by name.
const OWN: &[(&str, Command)] = &[
    ("core_state", |_| serde_json::to_value(crate::corelink::core_state()).map_err(|e| e.to_string())),
    ("core_restart", |_| {
        crate::corelink::core_restart();
        Ok(Value::Null)
    }),
    ("set_window_theme", set_window_theme),
];

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
        crate::corelink::core_send(cmd.to_owned(), args)
    }
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

fn set_window_theme(args: Value) -> Result<Value, String> {
    #[derive(Deserialize)]
    struct Args {
        theme: ThemePref,
    }
    let Args { theme } = serde_json::from_value(args).map_err(|e| format!("bad arguments: {e}"))?;
    let mut settings = read_settings();
    settings.theme = theme;
    let json = serde_json::to_vec_pretty(&settings).map_err(|e| e.to_string())?;
    crate::secret::write_private(&settings_file(), &json).map_err(|e| format!("cannot keep the theme: {e}"))?;
    Ok(Value::Null)
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

/// Runs the native window until it is closed: what `setup()` does for the
/// WebView window that needs no Tauri, then the window itself. The core runs
/// on after it, as it does after the WebView window.
pub(crate) fn run() {
    if log_facade::set_boxed_logger(Box::new(ToLunaLog)).is_ok() {
        log_facade::set_max_level(log_facade::LevelFilter::Warn);
    }
    let (mailbox, inbox) = luna_window::mailbox();
    // Events reach the window through its inbox; installed before anything
    // that could send one starts.
    let events = mailbox.clone();
    crate::emit::install(move |event, payload| {
        events.event(event, payload);
    });
    // The build stamp as well as the version: test builds between two
    // releases share a version, and only the commit tells them apart.
    crate::log::start(&format!(
        "starting luna {} build {} core {} (native window)",
        env!("CARGO_PKG_VERSION"),
        env!("LUNA_BUILD"),
        env!("LUNA_CORE_FP")
    ));
    // The previous run's end is reported before anything new is logged; the
    // heartbeat pings the window's thread through its inbox.
    let ping: Mailbox = mailbox.clone();
    crate::lifecycle::start(Box::new(move |f| ping.run_on_ui(f)));
    // The sessions, the hooks' listener, the agents and power all run in the
    // core (coreproc.rs); the window finds or starts it.
    crate::corelink::start();
    // Installs the CLIs on a fresh machine and keeps them current after.
    crate::cli::refresh_periodically();
    // A parent account's settings onto the others, when that is asked for.
    crate::account_settings::sync_on_start();

    let title = if crate::paths::is_test() { "Luna Test" } else { "Luna" };
    let options = luna_window::Options {
        title: title.into(),
        theme: theme(),
        background: crate::background(),
        on_quit: Box::new(|| {
            crate::lifecycle::exiting("the window was closed; the core and its sessions stay");
            // A Claude Code model ask may still be saving a renewed token;
            // its cleanup runs only while Luna does.
            crate::claude::models::wait_for_asks();
            crate::emit::stop();
            crate::power::clear();
        }),
    };
    // `mailbox` lives as long as the window: dropping the last one would end
    // the window's inbox.
    luna_window::run(std::sync::Arc::new(NativeHost), inbox, options);
    drop(mailbox);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Every command the window calls has an answer here, and none of the
    /// window's own shadows one of the core's.
    #[test]
    fn every_command_the_window_names_is_dispatched() {
        for cmd in luna_window::cmd::ALL {
            let own = OWN.iter().any(|(c, _)| c == cmd);
            assert!(own || corecmd::lane(cmd) == Some(Lane::Call), "{cmd} has no answer in native.rs");
        }
        for (cmd, _) in OWN {
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
        for (cmd, _) in OWN {
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
                let sent = crate::corelink::LinkState { state, core: core.clone(), error: error.clone() };
                let got: CoreState = serde_json::from_value(serde_json::to_value(&sent).unwrap()).unwrap();
                assert_eq!(got.state, seen);
                assert_eq!(got.error, error);
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
