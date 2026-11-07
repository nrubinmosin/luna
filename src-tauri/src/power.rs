//! What the machine does about the sessions: stays awake while they work,
//! and — when asked — shuts down, restarts, hibernates or sleeps once they are
//! all done.
//!
//! Both follow activity.rs. Keep-awake is a Windows power request
//! (`powercfg /requests` lists it as Luna's) held while any session is busy
//! and released a minute after the last one goes quiet. A session waiting for
//! a person does not hold the machine: nobody is answering it.
//!
//! Shutdown-when-done is armed for the run, not saved. Armed, it waits for a
//! quiet window in which nothing is busy *or waiting* — a pending permission
//! is unfinished work, while a session parked on a usage limit is not: its
//! turn is over and nothing moves before the reset (activity.rs, *stalled*);
//! arming with someone waiting says so in a notification — then counts down a minute with a notification, and
//! any activity in that minute cancels the count and leaves the arm in place.
//! Sleep and hibernate leave the sessions running and stay armed, so after a
//! wake the same rule applies again; shutdown first asks each CLI to quit, and
//! restart winds Luna down as a core leaving does and asks Windows to start the
//! window at the next sign-in, once.
//!
//! A rule may drain: until it fires, agents start nothing new, so it converges
//! rather than wait on work that keeps making work. Only the tools are refused
//! (`admit`, from mcp.rs); nothing a person does goes through there. A call let in
//! before a drain counts as at work until it has ended (`Entered`), and the arming
//! waits for it (`settle`). What a CLI starts by itself, a native subagent or a
//! `/loop`, Luna has no say in.
//!
//! The test build and unit tests never touch the machine: see `dry`.

use crate::activity::Summary;
use crate::throttle::now_ms;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock};

/// How long after the last busy sample the awake hold is let go.
const HOLD_LINGER_MS: u64 = 60_000;
/// The warning before the action fires.
const COUNTDOWN_MS: u64 = 60_000;
/// While armed and held up, the log says by whom: at once when the reason
/// changes (but not more often than this), and at least this often regardless,
/// so a night that ends with the PC still on can be read back.
const BLOCKED_LOG_MIN_MS: u64 = 60_000;
const BLOCKED_LOG_EVERY_MS: u64 = 10 * 60_000;
/// While the awake hold stands, the log says this often what keeps it: a hold
/// set in the evening and still on in the morning said nothing more, and
/// nobody could tell why the PC had stayed up.
const HOLD_LOG_EVERY_MS: u64 = 60 * 60_000;
/// How long a drain being armed waits for calls already let in: past a
/// `luna_spawn`'s own timeout. Longer, the rule still does not fire ahead of them.
const SETTLE_WAIT: std::time::Duration = std::time::Duration::from_secs(120);
/// RunOnce takes a command of no more than this many characters.
const RUN_ONCE_MAX: usize = 260;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Shutdown,
    Restart,
    Hibernate,
    Sleep,
    /// Runs the whole automaton and only writes a log line at the end.
    Log,
}

impl Action {
    pub fn parse(s: &str) -> Option<Action> {
        match s {
            "shutdown" => Some(Action::Shutdown),
            "restart" => Some(Action::Restart),
            "hibernate" => Some(Action::Hibernate),
            "sleep" => Some(Action::Sleep),
            "log" => Some(Action::Log),
            _ => None,
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Action::Shutdown => "shutdown",
            Action::Restart => "restart",
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
    /// Agents start nothing new until it fires or is disarmed.
    pub drain: bool,
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
    /// Who held the machine at the last busy sample, for the line that says
    /// the hold was let go: Windows sleeps at that moment when nobody has
    /// touched the PC for longer than its sleep timeout.
    held_by: String,
    /// When the hold was set, and when the log last said what keeps it.
    held_since_ms: u64,
    hold_logged_ms: u64,
    /// The last state sent to the UI, to send only changes.
    last_sent: Option<PowerState>,
    firing: bool,
    /// What is firing: agents are refused until that is over, drain or not, as
    /// what they start now the action would only take down with it.
    fired: Option<Action>,
    /// Agents' calls under way that may start work (`Entered`).
    admitted: usize,
    /// The last "armed, not firing" line and when it went out.
    blocked_why: String,
    blocked_logged_ms: u64,
}

/// Whether a rule is held up: by a session, or by a call that may yet start one.
fn held_up(st: &State, summary: &Summary) -> bool {
    summary.busy > 0 || summary.waiting > 0 || st.admitted > 0
}

/// One line naming every session that holds the machine, or "" when none does.
fn blockers(summary: &Summary) -> String {
    summary
        .sessions
        .iter()
        .filter(|s| s.busy || (s.turn == crate::activity::Turn::Waiting && s.stalled.is_none()))
        .map(|s| format!("{} {}", crate::agents::label(&s.id), s.why()))
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

fn publish(st: &mut State, summary: &Summary) {
    let now = snapshot(st, summary);
    if st.last_sent.as_ref() != Some(&now) {
        crate::emit::to_ui("power://state", now.clone());
        st.last_sent = Some(now);
    }
}

/// What one step of the automaton asks the outside world to do.
#[derive(Debug, PartialEq, Eq)]
enum Effect {
    Hold(bool),
    Countdown { quiet_for_s: u64 },
    // lang-check: allow
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
        let blocked = held_up(st, summary);
        if let Some(ends) = st.countdown_ends_at_ms {
            if blocked {
                st.countdown_ends_at_ms = None;
                // lang-check: allow
                out.push(Effect::Cancelled);
            } else if now >= ends && !st.firing {
                st.firing = true;
                st.countdown_ends_at_ms = None;
                st.fired = Some(armed.action);
                // Its end is reached: what a wake brings is free to work.
                if let Some(a) = st.armed.as_mut() {
                    a.drain = false;
                }
                // Sleep and hibernate keep the arm: the rule applies again after
                // the wake, over a fresh quiet window.
                if matches!(armed.action, Action::Shutdown | Action::Restart | Action::Log) {
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
pub fn tick(summary: &Summary) {
    let now = now_ms();
    let settings = crate::settings::get();
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    let action = st.armed.as_ref().map(|a| a.action);

    let busy_now = || {
        summary
            .sessions
            .iter()
            .filter(|s| s.busy)
            .map(|s| format!("{} {}", crate::agents::label(&s.id), s.why()))
            .collect::<Vec<_>>()
            .join(" | ")
    };
    for effect in advance(&mut st, settings.keep_awake, summary, now) {
        match effect {
            Effect::Hold(on) => {
                st.holding = os::hold(on);
                if on && st.holding {
                    let why = if summary.busy > 0 { busy_now() } else { "armed".to_string() };
                    crate::log::warn("power", &format!("awake hold set: {why}"));
                    st.held_since_ms = now;
                    st.hold_logged_ms = now;
                } else if !on && !st.holding {
                    crate::log::warn("power", &format!("awake hold let go: nothing busy for a minute; last held by {}", st.held_by));
                }
            }
            // lang-check: allow
            Effect::Cancelled => {
                crate::log::warn("power", &format!("countdown canceled: {} busy, {} waiting", summary.busy, summary.waiting));
            }
            Effect::Countdown { quiet_for_s } => {
                let a = action.unwrap_or(Action::Log);
                crate::log::warn("power", &format!("all sessions quiet for {quiet_for_s}s; {} in {}s", a.as_str(), COUNTDOWN_MS / 1000));
                let body = match summary.stalled {
                    0 => "Every session is done. Click the power chip in Luna to cancel.".to_string(),
                    n => format!("Every session is done ({n} stopped on a usage limit or another API error). Click the power chip in Luna to cancel."),
                };
                notify(&format!("Luna will {} the PC in a minute", verb(a)), &body);
            }
            Effect::Fire(a) => {
                std::thread::spawn(move || {
                    fire(a);
                    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
                    st.firing = false;
                    st.fired = None;
                });
            }
        }
    }

    if st.holding && summary.busy > 0 {
        st.held_by = busy_now();
        if now.saturating_sub(st.hold_logged_ms) >= HOLD_LOG_EVERY_MS {
            st.hold_logged_ms = now;
            let hours = now.saturating_sub(st.held_since_ms) / (60 * 60_000);
            crate::log::warn("power", &format!("awake hold kept for {hours} h: {}", st.held_by));
        }
    }

    // -- say why an armed rule is not firing --------------------------------
    if st.armed.is_some() && st.countdown_ends_at_ms.is_none() && !st.firing {
        if held_up(&st, summary) {
            let why = Some(blockers(summary)).filter(|w| !w.is_empty()).unwrap_or_else(|| "an agent's call may yet start work".to_string());
            // Changes are judged on who is holding and how, not on the full
            // line: that carries "printed 12s ago" and whichever grep or
            // sleep happened to be running, so it differed on every check
            // and the line went out every minute for hours.
            let who = summary
                .sessions
                .iter()
                .filter(|s| s.busy || (s.turn == crate::activity::Turn::Waiting && s.stalled.is_none()))
                .map(|s| format!("{} {}", s.id, if s.busy { "busy" } else { "waiting" }))
                .collect::<Vec<_>>()
                .join(" | ");
            if blocked_log_due(&mut st, &who, now) {
                crate::log::warn("power", &format!("armed, not firing: {why}"));
            }
        } else if !st.blocked_why.is_empty() {
            let quiet_for = summary.idle_since_ms.map(|t| now.saturating_sub(t)).unwrap_or(0) / 1000;
            crate::log::warn("power", &format!("armed, all quiet for {quiet_for}s; countdown when the window is full"));
            st.blocked_why.clear();
            st.blocked_logged_ms = 0;
        }
    }

    publish(&mut st, summary);
}

/// The sampler noticed the clock jump: a sleep just ended.
pub fn woke() {
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    if st.countdown_ends_at_ms.take().is_some() {
        crate::log::info("power", "countdown dropped across a sleep");
    }
    publish(&mut st, &crate::activity::summary());
}

/// The sessions that wait for a person, and those stalled on an API error,
/// by their chat names.
fn parked(summary: &Summary) -> (Vec<String>, Vec<String>) {
    let name = |id: &str| crate::agents::chat_name(id).unwrap_or_else(|| id.to_string());
    let mut waiting = Vec::new();
    let mut stalled = Vec::new();
    for s in summary.sessions.iter().filter(|s| !s.busy && s.turn == crate::activity::Turn::Waiting) {
        match s.stalled {
            Some(e) => stalled.push(format!("{} ({e})", name(&s.id))),
            None => waiting.push(name(&s.id)),
        }
    }
    (waiting, stalled)
}

fn plural(n: usize) -> String {
    if n == 1 {
        "1 session is".to_string()
    } else {
        format!("{n} sessions are")
    }
}

fn verb(a: Action) -> &'static str {
    match a {
        Action::Shutdown => "shut down",
        Action::Restart => "restart",
        Action::Hibernate => "hibernate",
        Action::Sleep => "put to sleep",
        Action::Log => "log-only",
    }
}

/// A desktop notification, shown by the window: the app may well be hidden
/// in the tray when it matters.
pub fn notify(title: &str, body: &str) {
    send(None, title, body);
}

/// One about a chat, which a click on it opens.
pub fn notify_chat(chat_id: &str, title: &str, body: &str) {
    send(Some(chat_id), title, body);
}

fn send(chat_id: Option<&str>, title: &str, body: &str) {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Notice<'a> {
        title: &'a str,
        body: &'a str,
        chat_id: Option<&'a str>,
    }
    crate::emit::to_ui("ui://notify", Notice { title, body, chat_id });
}

/// Whether this build leaves the machine alone, logging what it would have done:
/// the test build, and unit tests. Asked everywhere the PC would be touched, and
/// what arming logs and `luna_power` returns says it, from this same call.
pub fn dry() -> bool {
    cfg!(test) || crate::paths::is_test()
}

/// The window's exe, which the core is not: it runs from a copy (corelink.rs).
static WINDOW_EXE: OnceLock<PathBuf> = OnceLock::new();

pub fn set_window_exe(exe: PathBuf) {
    let _ = WINDOW_EXE.set(exe);
}

/// The command RunOnce is to run: the exe, quoted as a path with spaces must be.
fn run_once_command(exe: &Path) -> Result<String, String> {
    let exe = exe.to_string_lossy();
    if exe.contains('"') {
        return Err(format!("{exe} has a quote in it"));
    }
    let command = format!("\"{exe}\"");
    if command.chars().count() >= RUN_ONCE_MAX {
        return Err(format!("{command} is too long for RunOnce"));
    }
    Ok(command)
}

/// The window's start command after a restart, checked as far as can be: Windows
/// drops a RunOnce value before running it, so one that fails is not tried again.
fn relaunch_command() -> Result<String, String> {
    let exe = WINDOW_EXE.get().ok_or("this core was not told the window's exe")?;
    if !exe.is_file() {
        return Err(format!("{} is not there", exe.display()));
    }
    run_once_command(exe)
}

/// What the log says of the relaunch. A failure does not stop the restart: the PC
/// comes back all the same, and Luna is then started by hand.
fn relaunch_line(asked: &Result<String, String>) -> String {
    match asked {
        Ok(command) => format!("RunOnce set: {command} starts at the next sign-in"),
        Err(e) => {
            format!("could not set Luna to start at the next sign-in: {e}; restarting all the same, Luna stays down until started by hand")
        }
    }
}

/// Times `to_machine` went past `dry` in a test: it never may.
#[cfg(test)]
static PAST_DRY: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The one way to the machine: `real`, or in a dry build `dry_answer`. A unit test
/// goes no further than here whatever `dry` says.
fn to_machine<T>(dry_answer: T, real: impl FnOnce() -> T) -> T {
    if dry() || past_dry() {
        dry_answer
    } else {
        real()
    }
}

#[cfg(test)]
fn past_dry() -> bool {
    PAST_DRY.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    true
}

#[cfg(not(test))]
fn past_dry() -> bool {
    false
}

/// Off the sampler thread: quitting the CLIs takes seconds.
fn fire(action: Action) {
    if dry() && action != Action::Log {
        let relaunch = if action == Action::Restart { format!("; RunOnce would be: {:?}", relaunch_command()) } else { Default::default() };
        crate::log::warn("power", &format!("dry run: would {}; the sessions and the PC are left alone{relaunch}", action.as_str()));
        notify("Luna: dry run", &format!("Would {} the PC now. Nothing was done.", verb(action)));
        if matches!(action, Action::Sleep | Action::Hibernate) {
            crate::activity::reset_idle();
        }
        // As long as closing the CLIs would take: what is refused meanwhile can be tried.
        if !cfg!(test) {
            std::thread::sleep(crate::pty::GRACE);
        }
        return;
    }
    crate::log::warn("power", &format!("firing: {}", action.as_str()));
    match action {
        Action::Log => notify("Luna: log-only action fired", "All sessions were done; nothing was shut down."),
        Action::Sleep | Action::Hibernate => {
            crate::activity::reset_idle();
            if !to_machine(false, || os::suspend(action == Action::Hibernate)) {
                crate::log::error("power", "SetSuspendState refused");
            }
        }
        Action::Shutdown => {
            let quit = crate::pty::manager().shut_down_all(crate::pty::GRACE);
            crate::log::warn("power", &format!("{quit} sessions closed; shutting down"));
            if to_machine(false, os::shutdown) {
                crate::lifecycle::exiting("shut-down-when-done turned the PC off");
                // Windows ends the process from WM_ENDSESSION (sysmenu.rs);
                // leaving now just makes it tidy.
                std::thread::sleep(std::time::Duration::from_secs(2));
                crate::lifecycle::end_process();
            } else {
                crate::log::error("power", "shutdown command failed; sessions are closed but the PC is up");
                notify("Luna could not shut down the PC", "The sessions were closed. See luna.log.");
            }
        }
        Action::Restart => {
            let asked = relaunch_command()
                .and_then(|command| to_machine(Err("a dry run sets nothing".to_string()), || os::run_once(&command)).map(|()| command));
            crate::log::warn("power", &relaunch_line(&asked));
            let quit = crate::coreproc::wind_down();
            crate::log::warn("power", &format!("{quit} sessions closed; restarting"));
            if to_machine(false, os::restart) {
                crate::lifecycle::exiting("restart-when-done restarted the PC");
                // As for a shutdown: Windows ends the process from WM_ENDSESSION.
                std::thread::sleep(std::time::Duration::from_secs(2));
                crate::lifecycle::end_process();
            } else {
                crate::log::error("power", "restart command failed; sessions are closed but the PC is up");
                notify("Luna could not restart the PC", "The sessions were closed: quit Luna and start it again. See luna.log.");
            }
        }
    }
}

// --------------------------------------------------------------- admission --

/// Told when a call that was let in has ended (`Entered`).
static ENTERED_ENDED: Condvar = Condvar::new();

/// Why an agent is to start nothing now, if it is not to.
fn refusal(st: &State) -> Option<String> {
    if let Some(action) = st.fired {
        return Some(format!("Luna's {} is under way, so nothing new starts. End your turn.", action.as_str()));
    }
    let action = st.armed.as_ref().filter(|a| a.drain).map(|a| a.action)?;
    Some(format!(
        "Luna is draining: a {} is armed for when all sessions are quiet, so nothing new starts. What runs may finish, and luna_answer (a pending ask only), luna_kill and luna_delete still work. End your turn.",
        action.as_str()
    ))
}

/// A tool call that may start work, for as long as it runs: counted under the lock
/// that arms a drain, so it is either in before the drain, and waited for, or
/// told it is draining.
pub struct Entered<'a> {
    state: &'a Mutex<State>,
    ended: &'a Condvar,
    refusal: Option<String>,
}

impl Entered<'_> {
    pub fn draining(&self) -> bool {
        self.refusal.is_some()
    }
}

impl Drop for Entered<'_> {
    fn drop(&mut self) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.admitted = st.admitted.saturating_sub(1);
        drop(st);
        self.ended.notify_all();
    }
}

fn enter_on<'a>(state: &'a Mutex<State>, ended: &'a Condvar) -> Entered<'a> {
    let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
    st.admitted += 1;
    Entered { state, ended, refusal: refusal(&st) }
}

/// Held across a `luna_answer`, which a drain narrows (`agents::answer`). Not to be
/// taken with the registry's lock held: `tick` takes the two the other way round.
pub fn enter() -> Entered<'static> {
    enter_on(state(), &ENTERED_ENDED)
}

/// Held across a `luna_spawn` or `luna_send`, which a drain refuses.
pub fn admit() -> Result<Entered<'static>, String> {
    let entered = enter();
    match &entered.refusal {
        Some(why) => Err(why.clone()),
        None => Ok(entered),
    }
}

/// Waits, for as long as `wait`, for the calls let in before a drain to end, new
/// ones being refused by now. Returns how many are left, which still hold the rule.
fn settle<'a>(mut st: MutexGuard<'a, State>, ended: &Condvar, wait: std::time::Duration) -> (MutexGuard<'a, State>, usize) {
    let deadline = std::time::Instant::now() + wait;
    while st.admitted > 0 {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            break;
        }
        st = ended.wait_timeout(st, left).unwrap_or_else(|e| e.into_inner()).0;
    }
    let left = st.admitted;
    (st, left)
}

/// Why this machine cannot do `action`, for a rule to be refused before anything
/// is closed for it.
fn unsupported(action: Action, windows: bool) -> Option<String> {
    (action == Action::Restart && !windows).then(|| "Luna can restart the PC on Windows only".to_string())
}

// ---------------------------------------------------------------- commands --

#[tauri::command]
pub fn power_state() -> PowerState {
    current()
}

#[tauri::command]
pub fn set_keep_awake(on: bool) -> Result<PowerState, String> {
    crate::settings::update(|s| s.keep_awake = on)?;
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    if !on && st.holding && st.armed.is_none() {
        os::hold(false);
        st.holding = false;
        st.release_at_ms = None;
    }
    publish(&mut st, &crate::activity::summary());
    Ok(snapshot(&st, &crate::activity::summary()))
}

#[tauri::command]
pub fn arm_power_off(action: String, quiet_s: u64, drain: Option<bool>) -> Result<PowerState, String> {
    let action = Action::parse(&action).ok_or_else(|| format!("unknown action {action}"))?;
    if let Some(why) = unsupported(action, cfg!(windows)) {
        return Err(why);
    }
    let quiet_s = quiet_s.clamp(30, 6 * 3600);
    crate::settings::update(|s| {
        s.power_action = action.as_str().to_string();
        s.quiet_window_s = quiet_s;
    })?;
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    // Left unsaid it stands: a second agent's arm does not lift a drain by the way.
    let drain = drain.unwrap_or_else(|| st.armed.as_ref().is_some_and(|a| a.drain));
    st.armed = Some(Armed { action, quiet_s, armed_at_ms: now_ms(), drain });
    st.countdown_ends_at_ms = None;
    st.blocked_why.clear();
    st.blocked_logged_ms = 0;
    let summary = crate::activity::summary();
    let (waiting, stalled) = parked(&summary);
    let mut line = format!("armed: {} after {quiet_s}s quiet", action.as_str());
    if drain {
        line.push_str(", draining");
    }
    if dry() {
        line.push_str(" (dry run: this build only logs what it would do)");
    }
    if !waiting.is_empty() {
        line.push_str(&format!("; waiting for a person: {}", waiting.join(", ")));
    }
    if !stalled.is_empty() {
        line.push_str(&format!("; stopped on an API error, counted as done: {}", stalled.join(", ")));
    }
    crate::log::warn("power", &line);
    // Said at once, not found out in the morning: the menu closes on Arm,
    // and nothing else would tell that the rule cannot fire as things stand.
    if !waiting.is_empty() {
        notify(&format!("{} waiting for you — Luna will not {} until answered", plural(waiting.len()), verb(action)), &waiting.join(", "));
    }
    publish(&mut st, &summary);
    if drain {
        let (held, left) = settle(st, &ENTERED_ENDED, SETTLE_WAIT);
        st = held;
        if left > 0 {
            crate::log::warn("power", &format!("{left} call(s) let in before the drain still run; the rule waits for them"));
        }
    }
    Ok(snapshot(&st, &crate::activity::summary()))
}

#[tauri::command]
pub fn disarm_power_off() -> PowerState {
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    if st.armed.take().is_some() {
        crate::log::warn("power", "disarmed");
    }
    st.countdown_ends_at_ms = None;
    publish(&mut st, &crate::activity::summary());
    snapshot(&st, &crate::activity::summary())
}

/// A rule as `luna_power` gives it: as it stands now, which is none if somebody
/// disarmed it while a drain's arming waited.
fn armed_as_told(armed: Option<&Armed>) -> serde_json::Value {
    armed.map_or(serde_json::Value::Null, |a| serde_json::json!({ "action": a.action.as_str(), "quietS": a.quiet_s, "drain": a.drain }))
}

/// An agent's `luna_power`: the rule the power chip arms, so the PC goes down
/// the way Luna takes it down — once every session is quiet, each CLI asked to
/// quit first — rather than by a `shutdown` that takes the other chats' work
/// with it. "off" disarms. Says what still holds the rule up, the caller aside:
/// its own turn ends once it has answered, and whether this is a dry run (`dry`),
/// for a test to see before any countdown can end.
pub fn arm_for_agent(caller: &str, action: &str, quiet_s: Option<u64>, drain: Option<bool>) -> Result<serde_json::Value, String> {
    if action == "off" {
        disarm_power_off();
        return Ok(serde_json::json!({ "armed": null }));
    }
    let Some(a) = Action::parse(action).filter(|a| *a != Action::Log) else {
        return Err(format!("unknown action {action:?}; use shutdown, restart, hibernate, sleep or off"));
    };
    let quiet_s = quiet_s.unwrap_or(crate::settings::get().quiet_window_s);
    let state = arm_power_off(a.as_str().to_string(), quiet_s, drain)?;
    let name = |id: &str| crate::agents::chat_name(id).unwrap_or_else(|| id.to_string());
    let busy: Vec<String> = state.summary.sessions.iter().filter(|s| s.busy && s.id != caller).map(|s| name(&s.id)).collect();
    let (waiting, _) = parked(&state.summary);
    let mut out = serde_json::json!({
        "armed": armed_as_told(state.armed.as_ref()),
        "busy": busy,
        "waitingForAPerson": waiting,
    });
    if dry() {
        out["dryRun"] = serde_json::json!(true);
    }
    Ok(out)
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
            let mut ctx = REASON_CONTEXT {
                // POWER_REQUEST_CONTEXT_VERSION
                Version: 0,
                Flags: POWER_REQUEST_CONTEXT_SIMPLE_STRING,
                ..Default::default()
            };
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
            Ok(()) => on,
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

    fn shutdown_with(flag: &str) -> bool {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("shutdown")
            .args([flag, "/t", "0"])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    pub fn shutdown() -> bool {
        shutdown_with("/s")
    }

    pub fn restart() -> bool {
        shutdown_with("/r")
    }

    /// Has Windows run `command` at the user's next sign-in, once. Outside Luna's
    /// data folder because only the OS can do this: a value under RunOnce is the
    /// one-shot it offers, and it removes the value itself.
    pub fn run_once(command: &str) -> Result<(), String> {
        use windows::core::{w, HSTRING};
        use windows::Win32::System::Registry::{RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};
        let value = HSTRING::from(command);
        // The terminating zero counts, as REG_SZ wants it.
        let bytes = ((value.len() + 1) * 2) as u32;
        // SAFETY: both names are literals, and `value` outlives the call, which copies.
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                w!("Software\\Microsoft\\Windows\\CurrentVersion\\RunOnce"),
                w!("Luna"),
                REG_SZ.0,
                Some(value.as_ptr() as *const core::ffi::c_void),
                bytes,
            )
        }
        .ok()
        .map_err(|e| e.to_string())
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
    pub fn restart() -> bool {
        false
    }
    pub fn run_once(_command: &str) -> Result<(), String> {
        Err("only Windows has RunOnce".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board(busy: usize, waiting: usize, idle_since_ms: Option<u64>) -> Summary {
        Summary { busy, waiting, stalled: 0, idle_since_ms, sessions: vec![] }
    }

    /// A line held for a notice to ride with (agents.rs `riders_go`) keeps its
    /// debt open, which is nothing to a power-off: whatever lets one start counting
    /// lets the line through that gate first, and the turn it starts cancels the
    /// count. Only that gate: a line a draft holds back does not stop a count.
    #[test]
    fn a_power_off_does_not_go_ahead_of_a_line_held_back() {
        let mut st = armed(Action::Shutdown, 0);
        st.holding = true;
        let now = 1_000_000;
        let (fresh, rest_owed) = (now - 30_000, false);
        // Someone at work: nothing fires, and the line may wait.
        let at_work = board(1, 0, None);
        assert!(!advance(&mut st, false, &at_work, now).iter().any(|e| matches!(e, Effect::Countdown { .. })));
        assert!(!crate::agents::riders_go(false, rest_owed, fresh, now, at_work.busy));
        // All quiet: the count starts, and the line is free to go.
        let quiet = board(0, 0, Some(now - 1000));
        assert!(advance(&mut st, false, &quiet, now).iter().any(|e| matches!(e, Effect::Countdown { .. })));
        assert!(crate::agents::riders_go(false, rest_owed, fresh, now, quiet.busy));
        // lang-check: allow
        assert_eq!(advance(&mut st, false, &at_work, now + 1000), vec![Effect::Cancelled], "and its caller's turn stops the count");
    }

    fn armed(action: Action, quiet_s: u64) -> State {
        State { armed: Some(Armed { action, quiet_s, armed_at_ms: 0, drain: false }), ..Default::default() }
    }

    fn draining(action: Action, quiet_s: u64) -> State {
        let mut st = armed(action, quiet_s);
        st.armed.as_mut().unwrap().drain = true;
        st.holding = true;
        st
    }

    /// Runs a rule, all quiet, to its firing.
    fn fired(st: &mut State) -> Vec<Effect> {
        let quiet = board(0, 0, Some(0));
        advance(st, true, &quiet, 60_000);
        advance(st, true, &quiet, 60_000 + COUNTDOWN_MS)
    }

    #[test]
    fn nothing_here_touches_the_machine_in_a_test() {
        assert!(dry());
        let past = || PAST_DRY.load(std::sync::atomic::Ordering::SeqCst);
        for action in [Action::Sleep, Action::Hibernate, Action::Shutdown, Action::Restart] {
            fire(action);
        }
        assert_eq!(past(), 0, "fire stops short of the machine");
        // Every way out, should `fire` ever not stop first.
        assert!(!to_machine(false, || panic!("suspend")));
        assert!(!to_machine(false, || panic!("shutdown")));
        assert!(!to_machine(false, || panic!("restart")));
        assert_eq!(to_machine(Err::<(), &str>("dry"), || panic!("RunOnce")), Err("dry"));
        assert_eq!(past(), 0);
    }

    #[test]
    fn a_firing_refuses_new_work_with_no_drain_too() {
        for action in [Action::Restart, Action::Shutdown, Action::Sleep] {
            let mut st = armed(action, 60);
            st.holding = true;
            assert_eq!(refusal(&st), None, "armed, not firing");
            assert_eq!(fired(&mut st), vec![Effect::Fire(action)]);
            assert!(refusal(&st).unwrap().contains("under way"), "{action:?}");
            st.firing = false;
            st.fired = None;
            assert_eq!(refusal(&st), None);
        }
    }

    #[test]
    fn luna_power_gives_the_rule_as_it_stands_after_settling() {
        assert_eq!(armed_as_told(None), serde_json::Value::Null, "disarmed while it waited");
        let rule = Armed { action: Action::Restart, quiet_s: 60, armed_at_ms: 0, drain: true };
        assert_eq!(armed_as_told(Some(&rule)), serde_json::json!({ "action": "restart", "quietS": 60, "drain": true }));
    }

    #[test]
    fn restart_fires_once_and_drops_the_arm_as_a_shutdown_does() {
        assert_eq!(Action::parse("restart"), Some(Action::Restart));
        let mut st = armed(Action::Restart, 60);
        st.holding = true;
        assert_eq!(fired(&mut st), vec![Effect::Fire(Action::Restart)]);
        assert!(st.armed.is_none());
    }

    #[test]
    fn restart_is_refused_before_anything_where_the_os_cannot() {
        assert!(unsupported(Action::Restart, false).unwrap().contains("Windows only"));
        assert_eq!(unsupported(Action::Restart, true), None);
        assert_eq!(unsupported(Action::Shutdown, false), None);
        if !cfg!(windows) {
            let e = arm_for_agent("me", "restart", Some(60), Some(true)).unwrap_err();
            assert!(e.contains("Windows only"), "{e}");
        }
    }

    #[test]
    fn the_relaunch_is_quoted_and_a_bad_one_does_not_stop_the_restart() {
        let command = run_once_command(Path::new("C:\\Program Files\\Luna\\luna.exe")).unwrap();
        assert_eq!(command, "\"C:\\Program Files\\Luna\\luna.exe\"");
        assert!(relaunch_line(&Ok(command)).contains("next sign-in"));
        assert!(run_once_command(Path::new("C:\\a\"b.exe")).is_err());
        assert!(run_once_command(Path::new(&"x".repeat(RUN_ONCE_MAX))).is_err());
        assert!(relaunch_line(&Err("no".into())).contains("restarting all the same"));
    }

    #[test]
    fn a_drain_refuses_until_its_rule_has_fired_and_not_after() {
        let plain = armed(Action::Shutdown, 60);
        assert_eq!(refusal(&plain), None, "arming alone leaves agents free");
        for action in [Action::Sleep, Action::Hibernate, Action::Restart, Action::Shutdown] {
            let mut st = draining(action, 60);
            assert!(refusal(&st).unwrap().contains(&format!("a {} is armed", action.as_str())));
            assert_eq!(fired(&mut st), vec![Effect::Fire(action)]);
            assert!(refusal(&st).is_some(), "still, while it fires");
            // What the firing thread does once it is over.
            st.firing = false;
            st.fired = None;
            assert_eq!(refusal(&st), None, "{action:?}");
            let kept = matches!(action, Action::Sleep | Action::Hibernate);
            assert_eq!(st.armed.is_some(), kept);
        }
    }

    #[test]
    fn a_call_let_in_holds_the_rule_up_and_stops_its_count() {
        let mut st = draining(Action::Restart, 60);
        st.admitted = 1;
        let quiet = board(0, 0, Some(0));
        assert_eq!(advance(&mut st, true, &quiet, 120_000), vec![], "no count over a call that may start work");
        st.admitted = 0;
        assert_eq!(advance(&mut st, true, &quiet, 120_000), vec![Effect::Countdown { quiet_for_s: 120 }]);
        st.admitted = 1;
        // lang-check: allow
        assert_eq!(advance(&mut st, true, &quiet, 121_000), vec![Effect::Cancelled]);
    }

    #[test]
    fn a_call_let_in_before_a_drain_is_waited_for_and_one_after_it_is_refused() {
        let state = Mutex::new(State::default());
        let ended = Condvar::new();
        let early = enter_on(&state, &ended);
        assert!(!early.draining());
        std::thread::scope(|s| {
            let arming = s.spawn(|| {
                let mut st = state.lock().unwrap();
                st.armed = Some(Armed { action: Action::Restart, quiet_s: 60, armed_at_ms: 0, drain: true });
                settle(st, &ended, std::time::Duration::from_secs(30)).1
            });
            while state.lock().unwrap().armed.is_none() {
                std::thread::yield_now();
            }
            assert!(enter_on(&state, &ended).draining(), "refused from the moment it is armed");
            std::thread::sleep(std::time::Duration::from_millis(50));
            assert!(!arming.is_finished(), "the arming waits for the call let in");
            drop(early);
            assert_eq!(arming.join().unwrap(), 0);
        });
        assert_eq!(state.lock().unwrap().admitted, 0, "a refused call left nothing counted");
        // One that outlasts the wait is said, and still holds the rule.
        let late = enter_on(&state, &ended);
        let st = state.lock().unwrap();
        assert_eq!(settle(st, &ended, std::time::Duration::from_millis(10)).1, 1);
        drop(late);
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
        assert_eq!(advance(&mut st, true, &board(0, 0, Some(1_000)), 121_000), vec![Effect::Countdown { quiet_for_s: 120 }]);
        assert_eq!(st.countdown_ends_at_ms, Some(121_000 + COUNTDOWN_MS));
        // Not yet. Then fire, and shutdown drops the arm.
        assert_eq!(advance(&mut st, true, &board(0, 0, Some(1_000)), 150_000), vec![]);
        assert_eq!(advance(&mut st, true, &board(0, 0, Some(1_000)), 121_000 + COUNTDOWN_MS), vec![Effect::Fire(Action::Shutdown)]);
        assert!(st.armed.is_none());
        assert!(st.firing);
    }

    #[test]
    fn activity_during_the_countdown_cancels_and_keeps_the_arm() {
        let mut st = armed(Action::Hibernate, 60);
        st.holding = true;
        assert_eq!(advance(&mut st, true, &board(0, 0, Some(0)), 60_000), vec![Effect::Countdown { quiet_for_s: 60 }]);
        // lang-check: allow
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
    fn stalled_sessions_do_not_block_the_countdown() {
        // Two sessions sit on the usage-limit dialog; nothing else is up.
        let mut st = armed(Action::Shutdown, 60);
        st.holding = true;
        let b = Summary { stalled: 2, ..board(0, 0, Some(0)) };
        assert_eq!(advance(&mut st, true, &b, 60_000), vec![Effect::Countdown { quiet_for_s: 60 }]);
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
    fn an_agent_arms_only_what_the_chip_offers() {
        for bad in ["reboot", "log", ""] {
            let e = arm_for_agent("me", bad, None, None).unwrap_err();
            assert!(e.contains("use shutdown, restart, hibernate, sleep or off"), "{e}");
        }
        assert!(state().lock().unwrap().armed.is_none(), "nothing armed on a refusal");
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
