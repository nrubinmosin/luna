//! What the machine does about the sessions: stays awake while they work,
//! and — when asked — shuts down, hibernates or sleeps once they are all done.
//!
//! Both follow activity.rs. Keep-awake is a Windows power request
//! (`powercfg /requests` lists it as Luna's) held while any session is busy
//! and released a minute after the last one goes quiet. A session waiting for
//! a person does not hold the machine: nobody is answering it.
//!
//! Shutdown-when-done is armed for the run, not saved. Armed, it waits for a
//! quiet window in which nothing is busy *or waiting* — a pending permission
//! is unfinished work — then counts down a minute with a notification, and
//! any activity in that minute cancels the count and leaves the arm in place.
//! Sleep and hibernate leave the sessions running and stay armed, so after a
//! wake the same rule applies again; shutdown first asks each CLI to quit.

use crate::activity::Summary;
use crate::throttle::now_ms;
use serde::{Deserialize, Serialize};
use std::sync::{Mutex, OnceLock};

/// How long after the last busy sample the awake hold is let go.
const HOLD_LINGER_MS: u64 = 60_000;
/// The warning before the action fires.
const COUNTDOWN_MS: u64 = 60_000;
/// How long a CLI gets to quit on its own before the shutdown proceeds — more
/// than a chat delete gives it, since its exit hooks are what we are waiting for.
const QUIT_GRACE: std::time::Duration = std::time::Duration::from_secs(5);
/// While armed and held up, the log says by whom: at once when the reason
/// changes (but not more often than this), and at least this often regardless,
/// so a night that ends with the PC still on can be read back.
const BLOCKED_LOG_MIN_MS: u64 = 60_000;
const BLOCKED_LOG_EVERY_MS: u64 = 10 * 60_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Shutdown,
    Hibernate,
    Sleep,
    /// Runs the whole automaton and only writes a log line at the end.
    Log,
}

impl Action {
    pub fn parse(s: &str) -> Option<Action> {
        match s {
            "shutdown" => Some(Action::Shutdown),
            "hibernate" => Some(Action::Hibernate),
            "sleep" => Some(Action::Sleep),
            "log" => Some(Action::Log),
            _ => None,
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Action::Shutdown => "shutdown",
            Action::Hibernate => "hibernate",
            Action::Sleep => "sleep",
            Action::Log => "log",
        }
    }
}

#[derive(Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Armed {
    pub action: Action,
    pub quiet_s: u64,
    pub armed_at_ms: u64,
}

/// What the UI draws: the chip, its tooltip, and the menu's current values.
#[derive(Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PowerState {
    pub keep_awake: bool,
    /// The awake request is in force right now.
    pub holding: bool,
    pub armed: Option<Armed>,
    pub countdown_ends_at_ms: Option<u64>,
    pub summary: Summary,
}

#[derive(Default)]
struct State {
    armed: Option<Armed>,
    countdown_ends_at_ms: Option<u64>,
    holding: bool,
    /// When the hold may be released, once nothing is busy.
    release_at_ms: Option<u64>,
    /// The last state sent to the UI, to send only changes.
    last_sent: Option<PowerState>,
    firing: bool,
    /// The last "armed, not firing" line and when it went out.
    blocked_why: String,
    blocked_logged_ms: u64,
}

/// One line naming every session that holds the machine, or "" when none does.
fn blockers(summary: &Summary) -> String {
    summary
        .sessions
        .iter()
        .filter(|s| s.busy || s.turn == crate::activity::Turn::Waiting)
        .map(|s| format!("{} {}", s.id, s.why()))
        .collect::<Vec<_>>()
        .join(" | ")
}

/// Whether the blocked line is due: on a change after a minute's quiet, or
/// every ten minutes while nothing changes. Pure, for the test.
fn blocked_log_due(st: &mut State, why: &str, now: u64) -> bool {
    let since = now.saturating_sub(st.blocked_logged_ms);
    let due = (why != st.blocked_why && since >= BLOCKED_LOG_MIN_MS) || since >= BLOCKED_LOG_EVERY_MS || st.blocked_logged_ms == 0;
    if due {
        st.blocked_why = why.to_string();
        st.blocked_logged_ms = now;
    }
    due
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(Default::default)
}

fn snapshot(st: &State, summary: &Summary) -> PowerState {
    PowerState {
        keep_awake: crate::settings::get().keep_awake,
        holding: st.holding,
        armed: st.armed.clone(),
        countdown_ends_at_ms: st.countdown_ends_at_ms,
        summary: summary.clone(),
    }
}

pub fn current() -> PowerState {
    let st = state().lock().unwrap_or_else(|e| e.into_inner());
    snapshot(&st, &crate::activity::summary())
}

fn publish(app: &tauri::AppHandle, st: &mut State, summary: &Summary) {
    let now = snapshot(st, summary);
    if st.last_sent.as_ref() != Some(&now) {
        crate::emit::to_ui(app, "power://state", now.clone());
        st.last_sent = Some(now);
    }
}

/// What one step of the automaton asks the outside world to do.
#[derive(Debug, PartialEq, Eq)]
enum Effect {
    Hold(bool),
    Countdown { quiet_for_s: u64 },
    Cancelled,
    Fire(Action),
}

/// The automaton proper: pure over the state, so it can be driven by a test.
/// `holding` is only assumed to change where an effect says so.
fn advance(st: &mut State, keep_awake: bool, summary: &Summary, now: u64) -> Vec<Effect> {
    let mut out = Vec::new();

    // -- keep awake -----------------------------------------------------
    let want = (keep_awake && summary.busy > 0) || st.armed.is_some();
    if want {
        st.release_at_ms = None;
        if !st.holding {
            out.push(Effect::Hold(true));
        }
    } else if st.holding {
        let at = *st.release_at_ms.get_or_insert(now + HOLD_LINGER_MS);
        if now >= at {
            st.release_at_ms = None;
            out.push(Effect::Hold(false));
        }
    }

    // -- when done --------------------------------------------------------
    if let Some(armed) = st.armed.clone() {
        let blocked = summary.busy > 0 || summary.waiting > 0;
        if let Some(ends) = st.countdown_ends_at_ms {
            if blocked {
                st.countdown_ends_at_ms = None;
                out.push(Effect::Cancelled);
            } else if now >= ends && !st.firing {
                st.firing = true;
                st.countdown_ends_at_ms = None;
                // Sleep and hibernate keep the arm: the rule applies again after
                // the wake, over a fresh quiet window.
                if matches!(armed.action, Action::Shutdown | Action::Log) {
                    st.armed = None;
                }
                out.push(Effect::Fire(armed.action));
            }
        } else if !blocked {
            let quiet_for = summary.idle_since_ms.map(|t| now.saturating_sub(t)).unwrap_or(0);
            if quiet_for >= armed.quiet_s * 1000 {
                st.countdown_ends_at_ms = Some(now + COUNTDOWN_MS);
                out.push(Effect::Countdown { quiet_for_s: quiet_for / 1000 });
            }
        }
    }
    out
}

/// The sampler's call, once per sample.
pub fn tick(app: &tauri::AppHandle, summary: &Summary) {
    let now = now_ms();
    let settings = crate::settings::get();
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    let action = st.armed.as_ref().map(|a| a.action);

    for effect in advance(&mut st, settings.keep_awake, summary, now) {
        match effect {
            Effect::Hold(on) => st.holding = os::hold(on),
            Effect::Cancelled => {
                crate::log::warn("power", &format!("countdown cancelled: {} busy, {} waiting", summary.busy, summary.waiting));
            }
            Effect::Countdown { quiet_for_s } => {
                let a = action.unwrap_or(Action::Log);
                crate::log::warn("power", &format!("all sessions quiet for {quiet_for_s}s; {} in {}s", a.as_str(), COUNTDOWN_MS / 1000));
                notify(app, &format!("Luna will {} the PC in a minute", verb(a)), "Every session is done. Click the power chip in Luna to cancel.");
            }
            Effect::Fire(a) => {
                let app = app.clone();
                std::thread::spawn(move || {
                    fire(&app, a);
                    state().lock().unwrap_or_else(|e| e.into_inner()).firing = false;
                });
            }
        }
    }

    // -- say why an armed rule is not firing --------------------------------
    if st.armed.is_some() && st.countdown_ends_at_ms.is_none() && !st.firing {
        let blocked = summary.busy > 0 || summary.waiting > 0;
        if blocked {
            let why = blockers(summary);
            if blocked_log_due(&mut st, &why, now) {
                crate::log::warn("power", &format!("armed, not firing: {why}"));
            }
        } else if !st.blocked_why.is_empty() {
            let quiet_for = summary.idle_since_ms.map(|t| now.saturating_sub(t)).unwrap_or(0) / 1000;
            crate::log::warn("power", &format!("armed, all quiet for {quiet_for}s; countdown when the window is full"));
            st.blocked_why.clear();
            st.blocked_logged_ms = 0;
        }
    }

    publish(app, &mut st, summary);
}

/// The sampler noticed the clock jump: a sleep just ended.
pub fn woke(app: &tauri::AppHandle) {
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    if st.countdown_ends_at_ms.take().is_some() {
        crate::log::info("power", "countdown dropped across a sleep");
    }
    publish(app, &mut st, &crate::activity::summary());
}

fn verb(a: Action) -> &'static str {
    match a {
        Action::Shutdown => "shut down",
        Action::Hibernate => "hibernate",
        Action::Sleep => "put to sleep",
        Action::Log => "log-only",
    }
}

fn notify(app: &tauri::AppHandle, title: &str, body: &str) {
    use tauri_plugin_notification::NotificationExt;
    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        crate::log::warn("power", &format!("notification failed: {e}"));
    }
}

/// Off the sampler thread: quitting the CLIs takes seconds.
fn fire(app: &tauri::AppHandle, action: Action) {
    use tauri::Manager;
    crate::log::warn("power", &format!("firing: {}", action.as_str()));
    match action {
        Action::Log => notify(app, "Luna: log-only action fired", "All sessions were done; nothing was shut down."),
        Action::Sleep | Action::Hibernate => {
            crate::activity::reset_idle();
            if !os::suspend(action == Action::Hibernate) {
                crate::log::error("power", "SetSuspendState refused");
            }
        }
        Action::Shutdown => {
            let quit = app.state::<crate::pty::PtyManager>().shut_down_all(QUIT_GRACE);
            crate::log::warn("power", &format!("{quit} sessions closed; shutting down"));
            if os::shutdown() {
                // Windows ends the process from WM_ENDSESSION (sysmenu.rs);
                // leaving now just makes it tidy.
                std::thread::sleep(std::time::Duration::from_secs(2));
                app.exit(0);
            } else {
                crate::log::error("power", "shutdown command failed; sessions are closed but the PC is up");
                notify(app, "Luna could not shut down the PC", "The sessions were closed. See luna.log.");
            }
        }
    }
}

// ---------------------------------------------------------------- commands --

#[tauri::command]
pub fn power_state() -> PowerState {
    current()
}

#[tauri::command]
pub fn set_keep_awake(app: tauri::AppHandle, on: bool) -> Result<PowerState, String> {
    crate::settings::update(|s| s.keep_awake = on)?;
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    if !on && st.holding && st.armed.is_none() {
        os::hold(false);
        st.holding = false;
        st.release_at_ms = None;
    }
    publish(&app, &mut st, &crate::activity::summary());
    Ok(snapshot(&st, &crate::activity::summary()))
}

#[tauri::command]
pub fn arm_power_off(app: tauri::AppHandle, action: String, quiet_s: u64) -> Result<PowerState, String> {
    let action = Action::parse(&action).ok_or_else(|| format!("unknown action {action}"))?;
    let quiet_s = quiet_s.clamp(30, 6 * 3600);
    crate::settings::update(|s| {
        s.power_action = action.as_str().to_string();
        s.quiet_window_s = quiet_s;
    })?;
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    st.armed = Some(Armed { action, quiet_s, armed_at_ms: now_ms() });
    st.countdown_ends_at_ms = None;
    st.blocked_why.clear();
    st.blocked_logged_ms = 0;
    crate::log::warn("power", &format!("armed: {} after {quiet_s}s quiet", action.as_str()));
    publish(&app, &mut st, &crate::activity::summary());
    Ok(snapshot(&st, &crate::activity::summary()))
}

#[tauri::command]
pub fn disarm_power_off(app: tauri::AppHandle) -> PowerState {
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    if st.armed.take().is_some() {
        crate::log::warn("power", "disarmed");
    }
    st.countdown_ends_at_ms = None;
    publish(&app, &mut st, &crate::activity::summary());
    snapshot(&st, &crate::activity::summary())
}

/// Let go of the awake request on the way out.
pub fn clear() {
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    if st.holding {
        os::hold(false);
        st.holding = false;
    }
}

// ---------------------------------------------------------------------- os --

#[cfg(windows)]
mod os {
    use std::sync::{Mutex, OnceLock};
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Power::{
        PowerClearRequest, PowerCreateRequest, PowerRequestSystemRequired, PowerSetRequest, SetSuspendState,
    };
    use windows::Win32::System::Threading::{POWER_REQUEST_CONTEXT_SIMPLE_STRING, REASON_CONTEXT};

    struct Request(HANDLE);
    unsafe impl Send for Request {}

    fn request() -> &'static Mutex<Option<Request>> {
        static R: OnceLock<Mutex<Option<Request>>> = OnceLock::new();
        R.get_or_init(|| {
            // The reason shows in `powercfg /requests`. Lives as long as the
            // request does, hence leaked.
            let reason: &'static mut Vec<u16> =
                Box::leak(Box::new("Luna: sessions are working".encode_utf16().chain(std::iter::once(0)).collect()));
            let mut ctx = REASON_CONTEXT::default();
            // POWER_REQUEST_CONTEXT_VERSION
            ctx.Version = 0;
            ctx.Flags = POWER_REQUEST_CONTEXT_SIMPLE_STRING;
            ctx.Reason.SimpleReasonString = windows::core::PWSTR(reason.as_mut_ptr());
            // SAFETY: the context and its string outlive the request.
            match unsafe { PowerCreateRequest(&ctx) } {
                Ok(h) => Mutex::new(Some(Request(h))),
                Err(e) => {
                    crate::log::error("power", &format!("PowerCreateRequest: {e}"));
                    Mutex::new(None)
                }
            }
        })
    }

    /// Sets or clears the system-required request. Returns whether it is held.
    pub fn hold(on: bool) -> bool {
        let guard = request().lock().unwrap_or_else(|e| e.into_inner());
        let Some(r) = guard.as_ref() else { return false };
        // SAFETY: a request handle this module created.
        let res = unsafe {
            if on {
                PowerSetRequest(r.0, PowerRequestSystemRequired)
            } else {
                PowerClearRequest(r.0, PowerRequestSystemRequired)
            }
        };
        match res {
            Ok(()) => {
                crate::log::info("power", if on { "awake hold set" } else { "awake hold cleared" });
                on
            }
            Err(e) => {
                crate::log::warn("power", &format!("power request: {e}"));
                !on
            }
        }
    }

    pub fn suspend(hibernate: bool) -> bool {
        // SAFETY: no pointers; the call returns once the machine wakes.
        unsafe { SetSuspendState(hibernate, false, false) }
    }

    pub fn shutdown() -> bool {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("shutdown")
            .args(["/s", "/t", "0"])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

#[cfg(not(windows))]
mod os {
    pub fn hold(_on: bool) -> bool {
        false
    }
    pub fn suspend(_hibernate: bool) -> bool {
        false
    }
    pub fn shutdown() -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board(busy: usize, waiting: usize, idle_since_ms: Option<u64>) -> Summary {
        Summary { busy, waiting, idle_since_ms, sessions: vec![] }
    }

    fn armed(action: Action, quiet_s: u64) -> State {
        State { armed: Some(Armed { action, quiet_s, armed_at_ms: 0 }), ..Default::default() }
    }

    #[test]
    fn hold_follows_busy_with_a_linger() {
        let mut st = State::default();
        assert_eq!(advance(&mut st, true, &board(1, 0, None), 0), vec![Effect::Hold(true)]);
        st.holding = true;
        // Held while busy; a quiet sample starts the linger, not the release.
        assert_eq!(advance(&mut st, true, &board(1, 0, None), 10_000), vec![]);
        assert_eq!(advance(&mut st, true, &board(0, 0, Some(20_000)), 20_000), vec![]);
        assert_eq!(advance(&mut st, true, &board(0, 0, Some(20_000)), 20_000 + HOLD_LINGER_MS - 1), vec![]);
        assert_eq!(advance(&mut st, true, &board(0, 0, Some(20_000)), 20_000 + HOLD_LINGER_MS), vec![Effect::Hold(false)]);
    }

    #[test]
    fn waiting_does_not_hold_and_keep_awake_off_does_not_hold() {
        let mut st = State::default();
        assert_eq!(advance(&mut st, true, &board(0, 2, None), 0), vec![]);
        assert_eq!(advance(&mut st, false, &board(3, 0, None), 0), vec![]);
    }

    #[test]
    fn armed_holds_regardless() {
        let mut st = armed(Action::Sleep, 120);
        assert_eq!(advance(&mut st, false, &board(0, 0, Some(0)), 0), vec![Effect::Hold(true)]);
    }

    #[test]
    fn arm_quiet_countdown_fire() {
        let mut st = armed(Action::Shutdown, 120);
        st.holding = true;
        // Busy: nothing. Quiet but not for long enough: nothing.
        assert_eq!(advance(&mut st, true, &board(1, 0, None), 0), vec![]);
        assert_eq!(advance(&mut st, true, &board(0, 0, Some(1_000)), 60_000), vec![]);
        // Quiet for the window: the countdown starts.
        assert_eq!(
            advance(&mut st, true, &board(0, 0, Some(1_000)), 121_000),
            vec![Effect::Countdown { quiet_for_s: 120 }]
        );
        assert_eq!(st.countdown_ends_at_ms, Some(121_000 + COUNTDOWN_MS));
        // Not yet. Then fire, and shutdown drops the arm.
        assert_eq!(advance(&mut st, true, &board(0, 0, Some(1_000)), 150_000), vec![]);
        assert_eq!(
            advance(&mut st, true, &board(0, 0, Some(1_000)), 121_000 + COUNTDOWN_MS),
            vec![Effect::Fire(Action::Shutdown)]
        );
        assert!(st.armed.is_none());
        assert!(st.firing);
    }

    #[test]
    fn activity_during_the_countdown_cancels_and_keeps_the_arm() {
        let mut st = armed(Action::Hibernate, 60);
        st.holding = true;
        assert_eq!(advance(&mut st, true, &board(0, 0, Some(0)), 60_000), vec![Effect::Countdown { quiet_for_s: 60 }]);
        assert_eq!(advance(&mut st, true, &board(0, 1, None), 70_000), vec![Effect::Cancelled]);
        assert!(st.armed.is_some());
        assert_eq!(st.countdown_ends_at_ms, None);
        // The quiet window starts over from the new idle stretch.
        assert_eq!(advance(&mut st, true, &board(0, 0, Some(80_000)), 100_000), vec![]);
        assert_eq!(advance(&mut st, true, &board(0, 0, Some(80_000)), 140_000), vec![Effect::Countdown { quiet_for_s: 60 }]);
    }

    #[test]
    fn waiting_blocks_the_countdown() {
        let mut st = armed(Action::Shutdown, 60);
        st.holding = true;
        assert_eq!(advance(&mut st, true, &board(0, 1, None), 600_000), vec![]);
        assert_eq!(st.countdown_ends_at_ms, None);
    }

    #[test]
    fn sleep_keeps_the_arm_after_firing() {
        let mut st = armed(Action::Sleep, 60);
        st.holding = true;
        advance(&mut st, true, &board(0, 0, Some(0)), 60_000);
        assert_eq!(advance(&mut st, true, &board(0, 0, Some(0)), 60_000 + COUNTDOWN_MS), vec![Effect::Fire(Action::Sleep)]);
        assert!(st.armed.is_some());
    }

    #[test]
    fn blocked_line_goes_out_on_change_and_on_schedule() {
        let mut st = State::default();
        assert!(blocked_log_due(&mut st, "a mid-turn", 1_000));
        // Same reason, too soon: quiet.
        assert!(!blocked_log_due(&mut st, "a mid-turn", 1_000 + BLOCKED_LOG_MIN_MS));
        // A new reason within the minute waits; after it, goes out.
        assert!(!blocked_log_due(&mut st, "b printing", 1_000 + BLOCKED_LOG_MIN_MS - 1));
        assert!(blocked_log_due(&mut st, "b printing", 1_000 + BLOCKED_LOG_MIN_MS));
        // Nothing changes: the ten-minute heartbeat.
        let t = 1_000 + BLOCKED_LOG_MIN_MS;
        assert!(!blocked_log_due(&mut st, "b printing", t + BLOCKED_LOG_EVERY_MS - 1));
        assert!(blocked_log_due(&mut st, "b printing", t + BLOCKED_LOG_EVERY_MS));
    }

    #[test]
    fn a_firing_in_progress_does_not_fire_twice() {
        let mut st = armed(Action::Sleep, 60);
        st.holding = true;
        st.countdown_ends_at_ms = Some(1);
        st.firing = true;
        assert_eq!(advance(&mut st, true, &board(0, 0, Some(0)), 100), vec![]);
    }
}
