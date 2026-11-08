//! What is done to the machine and said of it: the notifications, the firing —
//! the OS asked first, the sessions wound down inside the grace it gives, the
//! window asked back after a restart — and each OS's calls, all of it stopped
//! short in the test build and the unit tests.

use super::*;
use std::time::{Duration, Instant};

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

/// How long the OS gets before it takes the PC down once it has said yes: the
/// sessions are closed inside it — every CLI at once, each with `pty::GRACE` —
/// and the helpers' turns stopped, with room to spare.
pub(super) const SHUTDOWN_GRACE: Duration = Duration::from_secs(30);
/// How long past the grace the PC is given to begin going down before what
/// became of the request is looked into: Windows closes every other program
/// first, each with up to 20 s to answer.
pub(super) const SHUTDOWN_OVERRUN: Duration = Duration::from_secs(60);

/// What a shutdown or restart does to the machine and the sessions, in the order
/// `power_off` does it, so a test can watch that order and script a refusal
/// without the PC going anywhere: the OS behind `Real`, a recorder in tests.
pub(super) trait PowerSteps {
    /// Asks the OS to turn the PC off, or restart it, `grace` from now, and
    /// returns the grace it granted. Nothing is closed before it has said yes.
    fn ask(&mut self, restart: bool, grace: Duration) -> Result<Duration, String>;
    /// Has the OS start `command` at the user's next sign-in, once.
    fn set_run_once(&mut self, command: &str) -> Result<(), String>;
    /// Takes that back: the PC is not restarting after all.
    fn clear_run_once(&mut self) -> Result<(), String>;
    /// Winds the core down as a restart of the PC needs: helper turns stopped,
    /// the chats frozen as they stand, every session asked to quit. How many.
    fn close_sessions(&mut self) -> usize;
    /// Waits for the OS to end this process — due at `due`, given until
    /// `deadline` — and says what it finds of the request by then.
    fn fate_by(&mut self, asked_at: Instant, due: Instant, deadline: Instant) -> Fate;
    /// Has the window start every chat's session over, and the helpers their
    /// stopped turns, as after a core restart.
    fn start_over(&mut self);
}

/// What became of a shutdown the OS said yes to, by the time this process was
/// to have been ended. The OS gives no word of it directly: `InitiateShutdownW`
/// returns at once and the shutdown follows on its own, or not at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Fate {
    /// The session is being taken down (`SM_SHUTTINGDOWN`): the OS ends this
    /// process any moment, however long it takes over the other programs.
    GoingDown,
    /// The OS recorded the request as taken back: `shutdown /a`, or another
    /// program's `AbortSystemShutdown` (User32 event 1075 in the System log).
    Canceled,
    /// Neither sign: the shutdown has not begun, and nothing says it was taken
    /// back. Nothing is undone on that alone.
    Unknown,
}

/// How a shutdown or restart ended, for the log and the tests.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    /// The OS said no; nothing was closed.
    Refused(String),
    /// The OS said yes and the sessions are closed; it ends this process.
    GoingDown,
    /// The OS said yes, the sessions closed, and it recorded the request as
    /// taken back: the chats start over.
    TakenBack,
    /// The OS said yes, the sessions closed, and nothing says what became of
    /// it: the chats stay closed, the RunOnce value stays, and the person is
    /// told how to bring them back.
    Unclear,
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
pub(super) fn fire(action: Action) {
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
        Action::Shutdown | Action::Restart => {
            power_off(action, &mut Real);
        }
    }
}

/// A shutdown or restart, the OS asked first. The other way round — every CLI
/// closed, then `shutdown /s` — left dead panes and a PC up all night when the
/// command failed, which it did without a word. A refusal now costs nothing; a
/// yes is followed by the sessions closing inside the grace; and what became of
/// the request once it was due decides the rest: a shutdown under way is left to
/// the OS, one recorded as taken back has the chats start over, and one nothing
/// speaks for undoes nothing, as a slow shutdown must not find new work started
/// and its RunOnce gone.
pub(super) fn power_off(action: Action, steps: &mut dyn PowerSteps) -> Outcome {
    let restart = action == Action::Restart;
    let (verb, noun) = (verb(action), action.as_str());
    // The relaunch first: a RunOnce value is cheap to take back if the OS says
    // no, and the restart's whole point is Luna coming back.
    let relaunch = restart.then(|| relaunch_command().and_then(|command| steps.set_run_once(&command).map(|()| command)));
    if let Some(asked) = &relaunch {
        crate::log::warn("power", &relaunch_line(asked));
    }
    let asked_at = Instant::now();
    let grace = match steps.ask(restart, SHUTDOWN_GRACE) {
        Ok(grace) => grace,
        Err(e) => {
            crate::log::error("power", &refusal_line(action, &e));
            take_back_relaunch(&relaunch, steps);
            notify(
                &format!("Luna could not {verb} the PC"),
                &format!("The system refused: {e}. The sessions are still running. See luna.log."),
            );
            return Outcome::Refused(e);
        }
    };
    crate::log::warn("power", &format!("the OS will {verb} the PC in {}s; closing the sessions meanwhile", grace.as_secs()));
    let quit = steps.close_sessions();
    crate::log::warn("power", &format!("{quit} sessions closed; the {noun} is the OS's now"));
    let waited = grace + SHUTDOWN_OVERRUN;
    match steps.fate_by(asked_at, asked_at + grace, asked_at + waited) {
        Fate::GoingDown => Outcome::GoingDown,
        Fate::Canceled => {
            crate::log::error("power", &format!("the {noun} was taken back (shutdown /a, or another program): the chats start over"));
            take_back_relaunch(&relaunch, steps);
            steps.start_over();
            notify(
                &format!("Luna's {noun} was canceled"),
                &format!("The system did not {verb} the PC. The chats are starting over where they were. See luna.log."),
            );
            Outcome::TakenBack
        }
        Fate::Unknown => {
            crate::log::error("power", &unknown_fate_line(action, waited, matches!(relaunch, Some(Ok(_)))));
            notify(
                &format!("Luna's {noun} has not happened"),
                "The system has neither begun it nor recorded a cancellation, so the chats stay closed. To bring them back, restart Luna's core from Settings, or quit Luna and start it again.",
            );
            Outcome::Unclear
        }
    }
}

/// The log's line for an OS that said no, the error as it gave it.
fn refusal_line(action: Action, e: &str) -> String {
    format!("the OS refused to {} the PC: {e}; nothing was closed, the sessions run on", verb(action))
}

/// The log's line for a request nothing speaks for: what was not done, and why.
fn unknown_fate_line(action: Action, waited: Duration, run_once_set: bool) -> String {
    let kept = if run_once_set { "; the RunOnce value stays, as the PC may yet restart" } else { "" };
    format!(
        "{}s after the {} was asked for the session is not shutting down and the System log has no cancellation (User32 1075): the chats stay closed, nothing is undone on time alone{kept}",
        waited.as_secs(),
        action.as_str()
    )
}

/// Removes the RunOnce value a restart set, if it did: the PC is not restarting.
fn take_back_relaunch(relaunch: &Option<Result<String, String>>, steps: &mut dyn PowerSteps) {
    if matches!(relaunch, Some(Ok(_))) {
        if let Err(e) = steps.clear_run_once() {
            crate::log::warn("power", &format!("could not remove the RunOnce value: {e}; Luna starts once more at the next sign-in"));
        }
    }
}

/// The machine itself, every call to it behind `to_machine`.
struct Real;

impl PowerSteps for Real {
    fn ask(&mut self, restart: bool, grace: Duration) -> Result<Duration, String> {
        to_machine(Err("a dry run asks nothing".to_string()), || os::shutdown(restart, grace))
    }

    fn set_run_once(&mut self, command: &str) -> Result<(), String> {
        to_machine(Err("a dry run sets nothing".to_string()), || os::run_once(command))
    }

    fn clear_run_once(&mut self) -> Result<(), String> {
        to_machine(Ok(()), os::clear_run_once)
    }

    fn close_sessions(&mut self) -> usize {
        crate::coreproc::wind_down()
    }

    fn fate_by(&mut self, asked_at: Instant, due: Instant, deadline: Instant) -> Fate {
        // The run's record says it leaves on purpose, for the next start to read:
        // the OS ends this process with no word to it. Taken back below if the
        // request turns out not to have gone through.
        crate::lifecycle::exiting("shut-down-when-done: the OS is taking the PC down");
        let pause = |until: Instant| std::thread::sleep(until.saturating_duration_since(Instant::now()).min(Duration::from_secs(1)));
        while Instant::now() < due {
            pause(due);
        }
        // Only once due: whether the session metric is up during the grace is not
        // documented, and a shutdown taken back inside it must not read as under way.
        while Instant::now() < deadline {
            if os::shutting_down() {
                crate::log::warn("power", "the session is shutting down; waiting for the OS to end this process");
                return Fate::GoingDown;
            }
            pause(deadline);
        }
        crate::lifecycle::note_exit(None);
        match os::shutdown_taken_back_since(asked_at) {
            Ok(true) => Fate::Canceled,
            Ok(false) => Fate::Unknown,
            Err(e) => {
                crate::log::warn("power", &format!("could not look for a cancellation in the System log: {e}"));
                Fate::Unknown
            }
        }
    }

    fn start_over(&mut self) {
        // The helpers as the next core would take them up: their jobs tied again,
        // turns may start again, and those stopped mid-turn go on by themselves
        // where they were spawned to.
        crate::background::reopen();
        crate::background::resume_stopped();
        // The window's panes start every chat's session over from its transcript,
        // as when a new core replaces one that stopped (core.store.ts).
        crate::emit::to_ui("pty://start-over", serde_json::json!({ "why": "the shutdown was taken back" }));
    }
}

#[cfg(windows)]
pub(super) mod os {
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_NOT_ALL_ASSIGNED, ERROR_SUCCESS, HANDLE, LUID, WIN32_ERROR};
    use windows::Win32::Security::{
        AdjustTokenPrivileges, LookupPrivilegeValueW, LUID_AND_ATTRIBUTES, SE_PRIVILEGE_ENABLED, SE_SHUTDOWN_NAME, TOKEN_ADJUST_PRIVILEGES,
        TOKEN_PRIVILEGES, TOKEN_QUERY,
    };
    use windows::Win32::System::EventLog::{EvtClose, EvtNext, EvtQuery, EvtQueryChannelPath, EvtQueryReverseDirection};
    use windows::Win32::System::Power::{
        PowerClearRequest, PowerCreateRequest, PowerRequestSystemRequired, PowerSetRequest, SetSuspendState,
    };
    use windows::Win32::System::Shutdown::{
        InitiateShutdownW, SHTDN_REASON_FLAG_PLANNED, SHTDN_REASON_MAJOR_OTHER, SHTDN_REASON_MINOR_OTHER, SHUTDOWN_FORCE_SELF,
        SHUTDOWN_POWEROFF, SHUTDOWN_RESTART,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken, POWER_REQUEST_CONTEXT_SIMPLE_STRING, REASON_CONTEXT};
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_SHUTTINGDOWN};

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

    /// Runs `f` with SeShutdownPrivilege turned on in this process's token, and
    /// puts the privilege back as it was after, so the core holds it for the call
    /// alone. Every user of a workstation has it, disabled until a program that
    /// means to shut down asks, as `shutdown.exe` does for itself; a token without
    /// it says so here, and `f` does not run.
    pub(super) fn with_shutdown_privilege<T>(f: impl FnOnce() -> T) -> Result<T, String> {
        let mut token = HANDLE::default();
        // SAFETY: the pseudo-handle of this process; `token` takes a real handle,
        // closed below.
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token) }
            .map_err(|e| format!("OpenProcessToken: {e}"))?;
        let out = enable_shutdown_privilege_on(token).map(|before| {
            let out = f();
            // SAFETY: `before` is what AdjustTokenPrivileges wrote: the one
            // privilege as it was, or none if it was on already.
            if let Err(e) = unsafe { AdjustTokenPrivileges(token, false, Some(&raw const before), 0, None, None) } {
                crate::log::warn("power", &format!("could not put SeShutdownPrivilege back as it was: {e}"));
            }
            out
        });
        // SAFETY: the handle OpenProcessToken gave, closed once.
        let _ = unsafe { CloseHandle(token) };
        out
    }

    /// Turns the privilege on and returns its state before, for putting back.
    fn enable_shutdown_privilege_on(token: HANDLE) -> Result<TOKEN_PRIVILEGES, String> {
        let mut luid = LUID::default();
        // SAFETY: a null system name is this machine; `luid` is written.
        unsafe { LookupPrivilegeValueW(PCWSTR::null(), SE_SHUTDOWN_NAME, &mut luid) }.map_err(|e| format!("LookupPrivilegeValueW: {e}"))?;
        let state =
            TOKEN_PRIVILEGES { PrivilegeCount: 1, Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }] };
        let mut before = TOKEN_PRIVILEGES::default();
        let mut written = 0u32;
        // SAFETY: `state` outlives the call; `before` has room for the one
        // privilege it may change, and the call is told so.
        unsafe {
            AdjustTokenPrivileges(
                token,
                false,
                Some(&raw const state),
                size_of::<TOKEN_PRIVILEGES>() as u32,
                Some(&raw mut before),
                Some(&raw mut written),
            )
        }
        .map_err(|e| format!("AdjustTokenPrivileges: {e}"))?;
        // It returns success having enabled nothing, and says so only here.
        // SAFETY: reads this thread's last error.
        match unsafe { GetLastError() } {
            ERROR_SUCCESS => Ok(before),
            ERROR_NOT_ALL_ASSIGNED => Err("this user does not hold SeShutdownPrivilege (ERROR_NOT_ALL_ASSIGNED)".to_string()),
            other => Err(format!("AdjustTokenPrivileges: {}", other.to_hresult().message())),
        }
    }

    /// What Windows' own notice says during the grace.
    const MESSAGE: PCWSTR = windows::core::w!(
        "Luna: every session is done, and the PC goes down as you asked. Programs still open are closed by force; unsaved work in them is lost."
    );

    /// Asks Windows for a shutdown or restart `grace` from now — what `shutdown.exe`
    /// does, minus the process in between, which failed here without a word:
    /// `InitiateShutdownW` with the privilege on for the call, and its Win32 error
    /// when it says no. The flags: POWEROFF or RESTART and nothing else of the kind,
    /// so the PC shuts down fully rather than into Fast Startup's hibernation (no
    /// HYBRID) and no program is told to reopen (no RESTARTAPPS); FORCE_SELF, so a
    /// program of the user's that will not close cannot hold the PC up all night
    /// with nobody there to answer Windows' "this app is preventing shutdown" — the
    /// user asked for the PC to go down, and the notice says what that costs; not
    /// FORCE_OTHERS: another user signed in is not this one's to sign out, so
    /// Windows refuses (ERROR_SHUTDOWN_USERS_LOGGED_ON) and nothing is closed. A
    /// planned reason, as Event Viewer records it.
    pub fn shutdown(restart: bool, grace: Duration) -> Result<Duration, String> {
        let what = (if restart { SHUTDOWN_RESTART } else { SHUTDOWN_POWEROFF }) | SHUTDOWN_FORCE_SELF;
        let reason = SHTDN_REASON_FLAG_PLANNED | SHTDN_REASON_MAJOR_OTHER | SHTDN_REASON_MINOR_OTHER;
        // SAFETY: a null machine name is this machine; the message is a literal.
        let code = with_shutdown_privilege(|| unsafe { InitiateShutdownW(PCWSTR::null(), MESSAGE, grace.as_secs() as u32, what, reason) })?;
        WIN32_ERROR(code).ok().map_err(|e| format!("InitiateShutdownW: {e}"))?;
        Ok(grace)
    }

    /// Whether the session is being shut down: the one sign Windows gives a
    /// process with no window, set once the grace is over and winlogon takes the
    /// session down (GetSystemMetrics, SM_SHUTTINGDOWN).
    pub fn shutting_down() -> bool {
        // SAFETY: no pointers; a failure reads as 0, which is "no".
        unsafe { GetSystemMetrics(SM_SHUTTINGDOWN) != 0 }
    }

    /// Whether Windows has recorded a shutdown request as taken back since
    /// `asked_at`: winlogon writes User32 event 1075 to the System log on an
    /// `AbortSystemShutdown` (`shutdown /a`), the one trace of it. Asked of the
    /// log by XPath, the window a little wider than the wait to cover the clocks.
    pub fn shutdown_taken_back_since(asked_at: Instant) -> Result<bool, String> {
        let since_ms = asked_at.elapsed().as_millis() + 5_000;
        let query = HSTRING::from(format!(
            "*[System[Provider[@Name='User32'] and (EventID=1075) and TimeCreated[timediff(@SystemTime) <= {since_ms}]]]"
        ));
        let flags = EvtQueryChannelPath.0 | EvtQueryReverseDirection.0;
        // SAFETY: a null session is the local log; the path and query outlive the
        // call, and every handle it gives is closed below.
        unsafe {
            let results = EvtQuery(None, windows::core::w!("System"), &query, flags).map_err(|e| format!("EvtQuery: {e}"))?;
            let mut events = [0isize; 1];
            let mut returned = 0u32;
            let found = EvtNext(results, &mut events, 0, 0, &mut returned).is_ok() && returned > 0;
            for event in &events[..returned as usize] {
                let _ = EvtClose(windows::Win32::System::EventLog::EVT_HANDLE(*event));
            }
            let _ = EvtClose(results);
            Ok(found)
        }
    }

    /// Takes a shutdown asked for back while its grace runs: the live check's
    /// way of not turning the PC off (`windows_takes_the_shutdown_request_and_its_abort`).
    #[cfg(test)]
    pub fn abort_shutdown() -> Result<(), String> {
        // SAFETY: a null machine name is this machine.
        with_shutdown_privilege(|| unsafe { windows::Win32::System::Shutdown::AbortSystemShutdownW(PCWSTR::null()) })?
            .map_err(|e| format!("AbortSystemShutdownW: {e}"))
    }

    const RUN_ONCE_KEY: PCWSTR = windows::core::w!("Software\\Microsoft\\Windows\\CurrentVersion\\RunOnce");
    const RUN_ONCE_VALUE: PCWSTR = windows::core::w!("Luna");

    /// Has Windows run `command` at the user's next sign-in, once. Outside Luna's
    /// data folder because only the OS can do this: a value under RunOnce is the
    /// one-shot it offers, and it removes the value itself.
    pub fn run_once(command: &str) -> Result<(), String> {
        use windows::Win32::System::Registry::{RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};
        let value = HSTRING::from(command);
        // The terminating zero counts, as REG_SZ wants it.
        let bytes = ((value.len() + 1) * 2) as u32;
        let data = Some(value.as_ptr() as *const core::ffi::c_void);
        // SAFETY: both names are literals, and `value` outlives the call, which copies.
        unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, RUN_ONCE_KEY, RUN_ONCE_VALUE, REG_SZ.0, data, bytes) }.ok().map_err(|e| e.to_string())
    }

    /// Removes the value `run_once` set: the PC is not restarting after all.
    pub fn clear_run_once() -> Result<(), String> {
        use windows::Win32::System::Registry::{RegDeleteKeyValueW, HKEY_CURRENT_USER};
        // SAFETY: both names are literals.
        unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_ONCE_KEY, RUN_ONCE_VALUE) }.ok().map_err(|e| e.to_string())
    }
}

#[cfg(not(windows))]
pub(super) mod os {
    use std::time::{Duration, Instant};

    pub fn hold(_on: bool) -> bool {
        false
    }
    pub fn suspend(_hibernate: bool) -> bool {
        false
    }

    /// `shutdown(8)` as Linux and macOS both have it: `-h` halts, which powers
    /// the machine off on either, `-r` restarts, and the delay is in whole
    /// minutes, so the grace rounds up to one. Root's to run as a rule; a refusal
    /// comes back with what it printed.
    pub fn shutdown(restart: bool, grace: Duration) -> Result<Duration, String> {
        let minutes = grace.as_secs().div_ceil(60).max(1);
        run("shutdown", &[if restart { "-r" } else { "-h" }, &format!("+{minutes}")])?;
        Ok(Duration::from_secs(minutes * 60))
    }

    /// Runs a command to its end, its stderr into a file in the data folder, and
    /// says how it failed: the spawn error, or its exit status with what it said.
    /// Not `.output()`: macOS's `shutdown` forks a child that keeps stderr for as
    /// long as the delay runs, and a pipe to it would be read to its end only as
    /// the PC went down.
    fn run(program: &str, args: &[&str]) -> Result<(), String> {
        let path = crate::paths::data_dir().join("shutdown.stderr");
        let stderr = std::fs::File::create(&path).map_err(|e| format!("could not make {}: {e}", path.display()))?;
        let status = std::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(stderr)
            .status()
            .map_err(|e| format!("could not run {program}: {e}"))?;
        let said = std::fs::read_to_string(&path).unwrap_or_default().trim().to_string();
        let _ = std::fs::remove_file(&path);
        if status.success() {
            return Ok(());
        }
        let said = if said.is_empty() { String::new() } else { format!(": {said}") };
        Err(format!("{program} {} ended with {status}{said}", args.join(" ")))
    }

    /// Nothing here tells a session going down from one that is not.
    pub fn shutting_down() -> bool {
        false
    }

    /// Nothing here records a `shutdown -c`.
    pub fn shutdown_taken_back_since(_asked_at: Instant) -> Result<bool, String> {
        Ok(false)
    }

    pub fn run_once(_command: &str) -> Result<(), String> {
        Err("only Windows has RunOnce".into())
    }
    pub fn clear_run_once() -> Result<(), String> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(to_machine(Err::<Duration, &str>("dry"), || panic!("shutdown")), Err("dry"));
        assert_eq!(to_machine(Err::<(), &str>("dry"), || panic!("RunOnce")), Err("dry"));
        assert_eq!(to_machine(Ok::<(), &str>(()), || panic!("RunOnce removal")), Ok(()));
        assert_eq!(past(), 0);
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

    /// Writes down each step a firing takes and plays the OS: one that says no,
    /// and what it is found to have done with a yes.
    struct Recorder {
        steps: Vec<String>,
        refuse: Option<&'static str>,
        fate: Fate,
    }

    impl Default for Recorder {
        fn default() -> Self {
            Recorder { steps: Vec::new(), refuse: None, fate: Fate::GoingDown }
        }
    }

    impl PowerSteps for Recorder {
        fn ask(&mut self, restart: bool, grace: Duration) -> Result<Duration, String> {
            self.steps.push(format!("ask {} in {}s", if restart { "restart" } else { "poweroff" }, grace.as_secs()));
            match self.refuse {
                Some(e) => Err(e.to_string()),
                None => Ok(grace),
            }
        }
        fn set_run_once(&mut self, command: &str) -> Result<(), String> {
            self.steps.push(format!("set_run_once {command}"));
            Ok(())
        }
        fn clear_run_once(&mut self) -> Result<(), String> {
            self.steps.push("clear_run_once".into());
            Ok(())
        }
        fn close_sessions(&mut self) -> usize {
            self.steps.push("close_sessions".into());
            2
        }
        fn fate_by(&mut self, asked_at: Instant, due: Instant, deadline: Instant) -> Fate {
            assert!(asked_at < due && due < deadline, "the grace, then the overrun");
            self.steps.push("fate_by".into());
            self.fate
        }
        fn start_over(&mut self) {
            self.steps.push("start_over".into());
        }
    }

    /// A restart has a window exe to relaunch: this test binary stands in.
    fn with_window_exe() -> String {
        let exe = std::env::current_exe().unwrap();
        set_window_exe(exe.clone());
        format!("set_run_once {}", run_once_command(&exe).unwrap())
    }

    fn run(action: Action, recorder: &mut Recorder) -> Outcome {
        power_off(action, recorder)
    }

    #[test]
    fn the_os_is_asked_before_any_session_is_closed() {
        let mut rec = Recorder::default();
        assert_eq!(run(Action::Shutdown, &mut rec), Outcome::GoingDown);
        assert_eq!(rec.steps, ["ask poweroff in 30s", "close_sessions", "fate_by"]);

        let run_once = with_window_exe();
        let mut rec = Recorder::default();
        assert_eq!(run(Action::Restart, &mut rec), Outcome::GoingDown);
        assert_eq!(rec.steps, [run_once.as_str(), "ask restart in 30s", "close_sessions", "fate_by"]);
    }

    #[test]
    fn a_refusal_closes_nothing_and_says_what_the_os_said() {
        let mut rec = Recorder { refuse: Some("InitiateShutdownW: ERROR_SHUTDOWN_USERS_LOGGED_ON (1191)"), ..Default::default() };
        let outcome = run(Action::Shutdown, &mut rec);
        assert_eq!(outcome, Outcome::Refused("InitiateShutdownW: ERROR_SHUTDOWN_USERS_LOGGED_ON (1191)".into()));
        assert_eq!(rec.steps, ["ask poweroff in 30s"], "no session closed, nothing started over");
        let Outcome::Refused(e) = outcome else { unreachable!() };
        let line = refusal_line(Action::Shutdown, &e);
        assert!(line.contains("ERROR_SHUTDOWN_USERS_LOGGED_ON (1191)") && line.contains("nothing was closed"), "{line}");
    }

    #[test]
    fn a_restart_the_os_refuses_takes_its_run_once_back() {
        let run_once = with_window_exe();
        let mut rec = Recorder { refuse: Some("no"), ..Default::default() };
        assert_eq!(run(Action::Restart, &mut rec), Outcome::Refused("no".into()));
        assert_eq!(rec.steps, [run_once.as_str(), "ask restart in 30s", "clear_run_once"]);
    }

    #[test]
    fn a_shutdown_recorded_as_taken_back_starts_the_chats_over() {
        let mut rec = Recorder { fate: Fate::Canceled, ..Default::default() };
        assert_eq!(run(Action::Shutdown, &mut rec), Outcome::TakenBack);
        assert_eq!(rec.steps, ["ask poweroff in 30s", "close_sessions", "fate_by", "start_over"]);

        let run_once = with_window_exe();
        let mut rec = Recorder { fate: Fate::Canceled, ..Default::default() };
        assert_eq!(run(Action::Restart, &mut rec), Outcome::TakenBack);
        assert_eq!(rec.steps, [run_once.as_str(), "ask restart in 30s", "close_sessions", "fate_by", "clear_run_once", "start_over"]);
    }

    /// Time alone is no cancellation: a slow shutdown must not find new work
    /// started and its RunOnce value gone.
    #[test]
    fn a_shutdown_nothing_speaks_for_undoes_nothing_and_keeps_the_run_once() {
        let mut rec = Recorder { fate: Fate::Unknown, ..Default::default() };
        assert_eq!(run(Action::Shutdown, &mut rec), Outcome::Unclear);
        assert_eq!(rec.steps, ["ask poweroff in 30s", "close_sessions", "fate_by"]);

        let run_once = with_window_exe();
        let mut rec = Recorder { fate: Fate::Unknown, ..Default::default() };
        assert_eq!(run(Action::Restart, &mut rec), Outcome::Unclear);
        assert_eq!(rec.steps, [run_once.as_str(), "ask restart in 30s", "close_sessions", "fate_by"]);
        let line = unknown_fate_line(Action::Restart, Duration::from_secs(90), true);
        assert!(line.contains("90s") && line.contains("RunOnce value stays") && line.contains("1075"), "{line}");
        assert!(!unknown_fate_line(Action::Shutdown, Duration::from_secs(90), false).contains("RunOnce"));
    }

    /// Whether SeShutdownPrivilege is on in this process's token, read back from
    /// the token itself.
    #[cfg(windows)]
    fn shutdown_privilege_on() -> bool {
        use windows::core::PCWSTR;
        use windows::Win32::Foundation::{CloseHandle, HANDLE, LUID};
        use windows::Win32::Security::{
            GetTokenInformation, LookupPrivilegeValueW, TokenPrivileges, LUID_AND_ATTRIBUTES, SE_PRIVILEGE_ENABLED, SE_SHUTDOWN_NAME,
            TOKEN_PRIVILEGES, TOKEN_QUERY,
        };
        use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
        let mut token = HANDLE::default();
        let mut luid = LUID::default();
        let mut len = 0u32;
        // SAFETY: this process's own token, read and closed here; the buffer is
        // sized by the first call and aligned for the entries the second fills.
        unsafe {
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).unwrap();
            LookupPrivilegeValueW(PCWSTR::null(), SE_SHUTDOWN_NAME, &mut luid).unwrap();
            let _ = GetTokenInformation(token, TokenPrivileges, None, 0, &mut len);
            let mut buf = vec![0u32; (len as usize).div_ceil(4)];
            GetTokenInformation(token, TokenPrivileges, Some(buf.as_mut_ptr().cast()), len, &mut len).unwrap();
            CloseHandle(token).unwrap();
            let count = buf[0] as usize;
            let first = buf.as_ptr().cast::<u8>().add(std::mem::offset_of!(TOKEN_PRIVILEGES, Privileges)).cast::<LUID_AND_ATTRIBUTES>();
            std::slice::from_raw_parts(first, count).iter().any(|p| p.Luid == luid && (p.Attributes.0 & SE_PRIVILEGE_ENABLED.0) != 0)
        }
    }

    /// The privilege is the call's alone: on inside, as it was after, a refusal
    /// of the call included.
    #[cfg(windows)]
    #[test]
    fn the_shutdown_privilege_is_on_for_the_call_alone() {
        let before = shutdown_privilege_on();
        let inside = os::with_shutdown_privilege(shutdown_privilege_on).unwrap();
        assert!(inside, "on for the call");
        assert_eq!(shutdown_privilege_on(), before, "as it was after");
        assert!(!before, "a process starts with it off, so the restore is seen to do something");
    }

    /// The real call, by hand and once: asks Windows for a shutdown a minute away
    /// and takes it back at once, Windows' notice showing in between; then that
    /// the signs `fate_by` goes by read as they should: the session metric, and the
    /// cancellation in the System log. The one test here that reaches the OS, on
    /// purpose.
    #[test]
    #[cfg(windows)]
    #[ignore = "live: asks Windows for a shutdown and aborts it; run by hand, alone"]
    fn windows_takes_the_shutdown_request_and_its_abort() {
        let asked_at = Instant::now();
        let asked = os::shutdown(false, Duration::from_secs(60));
        eprintln!("InitiateShutdownW(poweroff, 60 s): {asked:?}");
        eprintln!("SM_SHUTTINGDOWN during the grace: {}", os::shutting_down());
        let aborted = os::abort_shutdown();
        eprintln!("AbortSystemShutdownW: {aborted:?}");
        eprintln!("SM_SHUTTINGDOWN after the abort: {}", os::shutting_down());
        eprintln!("SeShutdownPrivilege on after the calls: {}", shutdown_privilege_on());
        // The log takes a moment to have it.
        let mut recorded = Ok(false);
        for _ in 0..50 {
            recorded = os::shutdown_taken_back_since(asked_at);
            if recorded == Ok(true) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        eprintln!("User32 1075 in the System log since the request: {recorded:?}");
        asked.expect("InitiateShutdownW");
        aborted.expect("AbortSystemShutdownW");
        assert_eq!(recorded, Ok(true), "the abort is in the System log");
    }
}
