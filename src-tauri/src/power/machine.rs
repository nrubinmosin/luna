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
/// How long past the grace the PC may still be up before the shutdown counts as
/// taken back (`shutdown /a`, or another program's AbortSystemShutdown): Windows
/// closes every other program first, each with up to 20 s to answer.
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
    /// Whether this process is still running at `deadline`, by when the OS was
    /// to have ended it: true means the PC did not go down.
    fn still_up_at(&mut self, deadline: Instant) -> bool;
    /// Has the window start every chat's session over, and the helpers their
    /// stopped turns, as after a core restart.
    fn start_over(&mut self);
}

/// How a shutdown or restart ended, for the log and the tests.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    /// The OS said no; nothing was closed.
    Refused(String),
    /// The OS said yes and the sessions are closed; it ends this process.
    GoingDown,
    /// The OS said yes, the sessions closed, and the PC stayed up: the
    /// shutdown was taken back, and the chats start over.
    TakenBack,
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
/// yes is followed by the sessions closing inside the grace; and a PC still up
/// well past it means the shutdown was taken back, on which the chats start over.
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
    if !steps.still_up_at(asked_at + grace + SHUTDOWN_OVERRUN) {
        return Outcome::GoingDown;
    }
    // Only ever reached when it did not happen: the OS ends this process when it does.
    crate::log::error(
        "power",
        &format!(
            "the PC is still up {}s after the {noun} was due: taken back (shutdown /a, or another program), so the chats start over",
            (grace + SHUTDOWN_OVERRUN).as_secs()
        ),
    );
    take_back_relaunch(&relaunch, steps);
    steps.start_over();
    notify(
        &format!("Luna's {noun} was canceled"),
        &format!("The system did not {verb} the PC. The chats are starting over where they were. See luna.log."),
    );
    Outcome::TakenBack
}

/// The log's line for an OS that said no, the error as it gave it.
fn refusal_line(action: Action, e: &str) -> String {
    format!("the OS refused to {} the PC: {e}; nothing was closed, the sessions run on", verb(action))
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

    fn still_up_at(&mut self, deadline: Instant) -> bool {
        // The run's record says it leaves on purpose, for the next start to read:
        // the OS ends this process with no word to it. Taken back below if it
        // turns out not to.
        crate::lifecycle::exiting("shut-down-when-done: the OS is taking the PC down");
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            std::thread::sleep(left.min(Duration::from_secs(1)));
        }
        crate::lifecycle::note_exit(None);
        true
    }

    fn start_over(&mut self) {
        // The helpers as the next core would take them up: turns may start again,
        // and those stopped mid-turn go on by themselves where they were spawned to.
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
    use std::time::Duration;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_NOT_ALL_ASSIGNED, ERROR_SUCCESS, HANDLE, LUID, WIN32_ERROR};
    use windows::Win32::Security::{
        AdjustTokenPrivileges, LookupPrivilegeValueW, LUID_AND_ATTRIBUTES, SE_PRIVILEGE_ENABLED, SE_SHUTDOWN_NAME, TOKEN_ADJUST_PRIVILEGES,
        TOKEN_PRIVILEGES, TOKEN_QUERY,
    };
    use windows::Win32::System::Power::{
        PowerClearRequest, PowerCreateRequest, PowerRequestSystemRequired, PowerSetRequest, SetSuspendState,
    };
    use windows::Win32::System::Shutdown::{
        InitiateShutdownW, SHTDN_REASON_FLAG_PLANNED, SHTDN_REASON_MAJOR_OTHER, SHTDN_REASON_MINOR_OTHER, SHUTDOWN_FORCE_SELF,
        SHUTDOWN_POWEROFF, SHUTDOWN_RESTART,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken, POWER_REQUEST_CONTEXT_SIMPLE_STRING, REASON_CONTEXT};

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

    /// Turns SeShutdownPrivilege on in this process's token. Every user of a
    /// workstation holds it, disabled until a program that means to shut down
    /// asks, as `shutdown.exe` does for itself; a token without it says so here.
    fn enable_shutdown_privilege() -> Result<(), String> {
        let mut token = HANDLE::default();
        // SAFETY: the pseudo-handle of this process; `token` takes a real handle,
        // closed below.
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token) }
            .map_err(|e| format!("OpenProcessToken: {e}"))?;
        let adjusted = enable_shutdown_privilege_on(token);
        // SAFETY: the handle OpenProcessToken gave, closed once.
        let _ = unsafe { CloseHandle(token) };
        adjusted
    }

    fn enable_shutdown_privilege_on(token: HANDLE) -> Result<(), String> {
        let mut luid = LUID::default();
        // SAFETY: a null system name is this machine; `luid` is written.
        unsafe { LookupPrivilegeValueW(PCWSTR::null(), SE_SHUTDOWN_NAME, &mut luid) }.map_err(|e| format!("LookupPrivilegeValueW: {e}"))?;
        let state =
            TOKEN_PRIVILEGES { PrivilegeCount: 1, Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }] };
        // SAFETY: `state` outlives the call; no previous state is asked for.
        unsafe { AdjustTokenPrivileges(token, false, Some(&raw const state), 0, None, None) }
            .map_err(|e| format!("AdjustTokenPrivileges: {e}"))?;
        // It returns success having enabled nothing, and says so only here.
        // SAFETY: reads this thread's last error.
        match unsafe { GetLastError() } {
            ERROR_SUCCESS => Ok(()),
            ERROR_NOT_ALL_ASSIGNED => Err("this user does not hold SeShutdownPrivilege (ERROR_NOT_ALL_ASSIGNED)".to_string()),
            other => Err(format!("AdjustTokenPrivileges: {}", other.to_hresult().message())),
        }
    }

    /// What Windows' own notice says during the grace.
    const MESSAGE: PCWSTR = windows::core::w!("Luna: every session is done, and the PC goes down as you asked.");

    /// Asks Windows for a shutdown or restart `grace` from now — what `shutdown.exe`
    /// does, minus the process in between, which failed here without a word:
    /// `InitiateShutdownW` with the privilege turned on first, and its Win32 error
    /// when it says no. The flags: POWEROFF or RESTART and nothing else of the kind,
    /// so the PC shuts down fully rather than into Fast Startup's hibernation (no
    /// HYBRID) and no program is told to reopen (no RESTARTAPPS); FORCE_SELF, so a
    /// program of the user's that will not close cannot hold the PC up all night
    /// with nobody there to answer Windows' "this app is preventing shutdown" — the
    /// user asked for the PC to go down; not FORCE_OTHERS: another user signed in is
    /// not this one's to sign out, so Windows refuses (ERROR_SHUTDOWN_USERS_LOGGED_ON)
    /// and nothing is closed. A planned reason, as Event Viewer records it.
    pub fn shutdown(restart: bool, grace: Duration) -> Result<Duration, String> {
        enable_shutdown_privilege()?;
        let what = (if restart { SHUTDOWN_RESTART } else { SHUTDOWN_POWEROFF }) | SHUTDOWN_FORCE_SELF;
        let reason = SHTDN_REASON_FLAG_PLANNED | SHTDN_REASON_MAJOR_OTHER | SHTDN_REASON_MINOR_OTHER;
        // SAFETY: a null machine name is this machine; the message is a literal.
        let code = unsafe { InitiateShutdownW(PCWSTR::null(), MESSAGE, grace.as_secs() as u32, what, reason) };
        WIN32_ERROR(code).ok().map_err(|e| format!("InitiateShutdownW: {e}"))?;
        Ok(grace)
    }

    /// Takes a shutdown asked for back while its grace runs: the live check's
    /// way of not turning the PC off (`windows_takes_the_shutdown_request_and_its_abort`).
    #[cfg(test)]
    pub fn abort_shutdown() -> Result<(), String> {
        enable_shutdown_privilege()?;
        // SAFETY: a null machine name is this machine.
        unsafe { windows::Win32::System::Shutdown::AbortSystemShutdownW(PCWSTR::null()) }.map_err(|e| format!("AbortSystemShutdownW: {e}"))
    }

    const RUN_ONCE_KEY: PCWSTR = windows::core::w!("Software\\Microsoft\\Windows\\CurrentVersion\\RunOnce");
    const RUN_ONCE_VALUE: PCWSTR = windows::core::w!("Luna");

    /// Has Windows run `command` at the user's next sign-in, once. Outside Luna's
    /// data folder because only the OS can do this: a value under RunOnce is the
    /// one-shot it offers, and it removes the value itself.
    pub fn run_once(command: &str) -> Result<(), String> {
        use windows::core::HSTRING;
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
    use std::time::Duration;

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

    /// Runs a command to its end and says how it failed: the spawn error, or
    /// its exit status with what it printed to stderr.
    fn run(program: &str, args: &[&str]) -> Result<(), String> {
        let out = std::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| format!("could not run {program}: {e}"))?;
        if out.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let said = if stderr.is_empty() { String::new() } else { format!(": {stderr}") };
        Err(format!("{program} {} ended with {}{said}", args.join(" "), out.status))
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
    /// or a PC that stays up after it said yes.
    #[derive(Default)]
    struct Recorder {
        steps: Vec<String>,
        refuse: Option<&'static str>,
        stays_up: bool,
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
        fn still_up_at(&mut self, _deadline: Instant) -> bool {
            self.steps.push("still_up_at".into());
            self.stays_up
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
        assert_eq!(rec.steps, ["ask poweroff in 30s", "close_sessions", "still_up_at"]);

        let run_once = with_window_exe();
        let mut rec = Recorder::default();
        assert_eq!(run(Action::Restart, &mut rec), Outcome::GoingDown);
        assert_eq!(rec.steps, [run_once.as_str(), "ask restart in 30s", "close_sessions", "still_up_at"]);
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
    fn a_shutdown_taken_back_starts_the_chats_over() {
        let mut rec = Recorder { stays_up: true, ..Default::default() };
        assert_eq!(run(Action::Shutdown, &mut rec), Outcome::TakenBack);
        assert_eq!(rec.steps, ["ask poweroff in 30s", "close_sessions", "still_up_at", "start_over"]);

        let run_once = with_window_exe();
        let mut rec = Recorder { stays_up: true, ..Default::default() };
        assert_eq!(run(Action::Restart, &mut rec), Outcome::TakenBack);
        assert_eq!(rec.steps, [run_once.as_str(), "ask restart in 30s", "close_sessions", "still_up_at", "clear_run_once", "start_over"]);
    }

    /// The real call, by hand and once: asks Windows for a shutdown a minute away
    /// and takes it back at once, Windows' notice showing in between. The one test
    /// here that reaches the OS, on purpose.
    #[test]
    #[cfg(windows)]
    #[ignore = "live: asks Windows for a shutdown and aborts it; run by hand, alone"]
    fn windows_takes_the_shutdown_request_and_its_abort() {
        let asked = os::shutdown(false, Duration::from_secs(60));
        eprintln!("InitiateShutdownW(poweroff, 60 s): {asked:?}");
        let aborted = os::abort_shutdown();
        eprintln!("AbortSystemShutdownW: {aborted:?}");
        asked.expect("InitiateShutdownW");
        aborted.expect("AbortSystemShutdownW");
    }
}
