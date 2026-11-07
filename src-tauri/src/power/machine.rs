//! What is done to the machine and said of it: the notifications, the firing —
//! sessions wound down first, the window asked back after a restart — and each
//! OS's calls, all of it stopped short in the test build and the unit tests.

use super::*;

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

#[cfg(windows)]
pub(super) mod os {
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
pub(super) mod os {
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
    fn the_relaunch_is_quoted_and_a_bad_one_does_not_stop_the_restart() {
        let command = run_once_command(Path::new("C:\\Program Files\\Luna\\luna.exe")).unwrap();
        assert_eq!(command, "\"C:\\Program Files\\Luna\\luna.exe\"");
        assert!(relaunch_line(&Ok(command)).contains("next sign-in"));
        assert!(run_once_command(Path::new("C:\\a\"b.exe")).is_err());
        assert!(run_once_command(Path::new(&"x".repeat(RUN_ONCE_MAX))).is_err());
        assert!(relaunch_line(&Err("no".into())).contains("restarting all the same"));
    }
}
