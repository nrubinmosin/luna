mod account_settings;
mod account_use;
mod accounts;
mod activity;
mod admit;
mod agentlog;
mod agents;
mod applied;
mod background;
mod backlog;
mod boundary;
mod checkout;
mod claude;
mod cli;
mod codex;
#[cfg(test)]
mod context_page;
mod corecmd;
mod corelink;
mod coreproc;
mod desktop;
mod elevation;
mod emit;
mod endpoint;
// build.rs stamps the binary with it; compiled here for its tests.
#[cfg(test)]
mod fingerprint;
#[cfg(test)]
mod golden;
mod held;
mod http;
mod hub;
mod identity;
mod ipc;
#[cfg(unix)]
mod keeper;
mod library;
mod lifecycle;
#[cfg(test)]
mod live;
mod log;
mod mcp;
mod media;
mod native;
mod paths;
mod peer;
mod power;
mod procs;
mod provider;
mod pty;
mod runtime;
mod secret;
mod settings;
mod stamped;
mod starts;
mod sysmenu;
mod throttle;
mod token_use;

/// The transcript readers as pure functions of a file's bytes, for the fuzz
/// targets in fuzz/: the one way into the crate from outside it.
#[doc(hidden)]
pub mod transcripts {
    pub use crate::agents::{Ask, AskQuestion, Message};
    pub use crate::claude::session::{messages_in, read_transcript, TranscriptRead};
    pub use crate::codex::session::{read_rollout, RolloutRead, Tail};
}

/// The test build kept out of the way of the person at the screen while an
/// agent drives it (`LUNA_TEST_WINDOW=background`, which
/// `scripts/test-build.ps1 -Background` sets): its window is made off screen,
/// without taking focus and without a taskbar button. The release ignores the
/// variable.
pub(crate) fn background() -> bool {
    paths::is_test() && std::env::var("LUNA_TEST_WINDOW").is_ok_and(|v| v == "background")
}

/// The exe: the window (native.rs), or, as its arguments say, the core, the
/// exit watcher or the keeper.
pub fn run() {
    // The exit watcher, the keeper and the core are this same exe; none may
    // become a second Luna window.
    if lifecycle::watch_from_args()
        || procs::keeper_from_args()
        || coreproc::from_args()
        || coreproc::fingerprint_from_args()
        || coreproc::stop_from_args()
    {
        return;
    }
    let restarting = lifecycle::wait_for_previous();
    log::install_panic_hook();
    // One window per data folder: a second launch brings the running one up
    // and exits, before it starts anything (the run record, the CLI refresh,
    // the settings sync). No claim under LUNA_DEV_TOKEN in the test build,
    // which a script runs beside the Luna a person is using; in the
    // background a second launch, a script's, leaves the window where it is
    // (desktop/single.rs). A window a tray Restart started waits out the one
    // it replaces rather than hand itself over to it. Held until the process
    // ends. A claim that fails stops the launch: a window without it would
    // run beside the one that has it, two windows on one core and one data
    // folder.
    let _single = match desktop::single::claim(desktop::bring_up, restarting) {
        Ok(desktop::single::Claim::First(guard)) => guard,
        Ok(desktop::single::Claim::Second) => return,
        Err(e) => {
            log::error("app", &format!("{e}; this launch of Luna stops"));
            return;
        }
    };
    // Nothing of the window's was started when it cannot open (no usable GPU
    // device, shaders that do not compile): the log says why, the person is
    // told where to read it, and the core, if one runs, runs on with its
    // sessions.
    if let Err(why) = native::run() {
        log::error("window", &format!("the window cannot open: {why}"));
        cannot_open(&why);
    }
}

/// Says on screen that the window could not open, and where the log says
/// why: a launch that ends with nothing shown, after an update say, would
/// otherwise look as if Luna never started. Windows only; not for the test
/// build in the background, which puts nothing on the screen of the person
/// at it. The message box is modal and returns once it is closed.
fn cannot_open(why: &str) {
    #[cfg(windows)]
    if !background() {
        use windows::core::HSTRING;
        use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK, MB_SETFOREGROUND};
        let text = HSTRING::from(window_failed_text(why, log::path()));
        let title = HSTRING::from(desktop::product_name());
        // SAFETY: both strings outlive the call; no owner window.
        unsafe { MessageBoxW(None, &text, &title, MB_OK | MB_ICONERROR | MB_SETFOREGROUND) };
    }
    #[cfg(not(windows))]
    let _ = why;
}

/// What `cannot_open` says.
#[cfg_attr(not(windows), allow(dead_code))]
fn window_failed_text(why: &str, log: &std::path::Path) -> String {
    format!(
        "{} could not open its window: {why}\n\nYour chats and their sessions are untouched. The log has the details:\n{}",
        desktop::product_name(),
        log.display()
    )
}

#[cfg(test)]
mod tests {
    /// The message names why and the log a person can open.
    #[test]
    fn a_window_that_cannot_open_names_the_log() {
        let log = std::path::Path::new("D:/Luna/luna.log");
        let text = super::window_failed_text("no usable Direct3D device", log);
        assert!(text.contains("no usable Direct3D device") && text.contains(&log.display().to_string()), "{text}");
    }
}
