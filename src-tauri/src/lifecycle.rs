//! Why Luna stopped, for the log.
//!
//! A run can end four ways, and each leaves a different trace here:
//! - Luna leaves on its own (tray Quit, an update, the end of the Windows
//!   session, shut-down-when-done): the call site names the reason and
//!   `exiting` writes it down before the process goes.
//! - Something kills the process from outside (Task Manager, taskkill,
//!   Stop-Process, a game's anti-cheat): nothing runs inside Luna to say so.
//!   A watcher — this same exe started as `--watch-exit <pid>`, outside the
//!   sessions' jobs and consoles — holds a handle on Luna and logs the exit
//!   code, which tells a kill from a crash, and whether Task Manager was open.
//! - It crashes: a panic is logged by its hook (log.rs); a native crash or an
//!   abort shows in the watcher's line as an NTSTATUS code.
//! - The machine goes down under it (power loss, a BSOD, a hard reset), or the
//!   watcher dies with it (Task Manager's "End process tree"): nothing is left
//!   running to see it. For that a small file beside the log carries the run's
//!   pid, its last sign of life and the last time its event loop answered; the
//!   next start reads it and, when the run never said goodbye, says what it
//!   knows — including that Windows has booted since, or that the window had
//!   been frozen for a while before the end, which is when people kill it.
//!
//! The heartbeat thread that keeps that file fresh also logs the event loop
//! going quiet and coming back, and sleeps of the machine, so a hang or a
//! suspend sits in the log next to whatever ended the run.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tauri::AppHandle;

const TICK: Duration = Duration::from_secs(10);
/// The event loop not running a ping for this long is a hang worth a line.
const HUNG_MS: u64 = 30_000;
/// A tick this late means the machine slept or the process was suspended.
const ASLEEP_MS: u64 = 60_000;

#[derive(Serialize, Deserialize, Clone, Default)]
struct Run {
    pid: u32,
    started: u64,
    /// Last tick of the heartbeat thread, which runs whatever the UI does.
    alive: u64,
    /// Last time the event loop ran a ping.
    answered: u64,
    /// Set by `exiting`; a run that ends without it was ended from outside.
    exit: Option<String>,
}

static RUN: Mutex<Option<Run>> = Mutex::new(None);
static NOTED: AtomicBool = AtomicBool::new(false);
static ANSWERED: AtomicU64 = AtomicU64::new(0);
static PINGING: AtomicBool = AtomicBool::new(false);

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn clock(ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(ms as i64)
        .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| "?".into())
}

fn marker() -> PathBuf {
    crate::log::path().with_extension("run")
}

fn read() -> Option<Run> {
    serde_json::from_str(&std::fs::read_to_string(marker()).ok()?).ok()
}

/// Through a temporary file: a kill mid-write must not leave half a file for
/// the next start to misread.
fn save(run: &Run) {
    let Ok(json) = serde_json::to_string(run) else { return };
    let p = marker();
    let tmp = p.with_extension("run.tmp");
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

/// When this boot of the machine began, to tell "Luna was killed" from "the
/// PC went down under it".
fn booted_ms() -> Option<u64> {
    #[cfg(windows)]
    {
        // SAFETY: no arguments, no state.
        let up = unsafe { windows::Win32::System::SystemInformation::GetTickCount64() };
        Some(now_ms().saturating_sub(up))
    }
    #[cfg(target_os = "linux")]
    {
        let s = std::fs::read_to_string("/proc/uptime").ok()?;
        let up: f64 = s.split_whitespace().next()?.parse().ok()?;
        Some(now_ms().saturating_sub((up * 1000.0) as u64))
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        None
    }
}

/// Reports the previous run if it ended without saying why, then starts
/// keeping this one's record, its watcher and its heartbeat. From setup: a
/// second launch that only focuses the running Luna exits before it, and must
/// not take over the running one's record.
pub fn start(app: AppHandle) {
    // A scripted dev build may run beside another Luna from the same folder.
    if std::env::var_os("LUNA_DEV_TOKEN").is_some() {
        return;
    }
    if let Some(prev) = read() {
        if prev.exit.is_none() && prev.pid != std::process::id() {
            crate::log::warn("app", &describe_unannounced(&prev, booted_ms()));
        }
    }
    let now = now_ms();
    ANSWERED.store(now, Ordering::Relaxed);
    let run = Run { pid: std::process::id(), started: now, alive: now, answered: now, exit: None };
    save(&run);
    *RUN.lock().unwrap_or_else(|e| e.into_inner()) = Some(run);

    watcher::spawn();
    std::thread::spawn(move || heartbeat(app));
}

fn describe_unannounced(prev: &Run, booted: Option<u64>) -> String {
    let mut s = format!(
        "the previous run (pid {}, started {}) ended without closing itself; last alive {}",
        prev.pid,
        clock(prev.started),
        clock(prev.alive)
    );
    let frozen = prev.alive.saturating_sub(prev.answered);
    if frozen >= HUNG_MS {
        s.push_str(&format!(", its window frozen since {} ({}s before)", clock(prev.answered), frozen / 1000));
    }
    if booted.is_some_and(|b| b > prev.alive) {
        s.push_str("; Windows has started again since, so the PC went down under it (power loss, a system crash or a forced restart)");
    } else {
        s.push_str("; see the watcher's line for how it ended, or if there is none the watcher went with it (\"End process tree\")");
    }
    s
}

fn heartbeat(app: AppHandle) {
    let mut last_tick = now_ms();
    let mut hung_logged = false;
    loop {
        std::thread::sleep(TICK);
        let now = now_ms();

        let gap = now.saturating_sub(last_tick);
        if gap >= ASLEEP_MS {
            crate::log::warn("app", &format!("woke after {}s asleep or suspended (since {})", gap / 1000, clock(last_tick)));
            // The loop could not have answered while nothing ran.
            ANSWERED.store(now, Ordering::Relaxed);
            PINGING.store(false, Ordering::Relaxed);
        }
        last_tick = now;

        let answered = ANSWERED.load(Ordering::Relaxed);
        let quiet = now.saturating_sub(answered);
        if quiet >= HUNG_MS && !hung_logged {
            crate::log::warn("app", &format!("the event loop has not answered for {}s", quiet / 1000));
            hung_logged = true;
        } else if quiet < HUNG_MS && hung_logged {
            crate::log::warn("app", "the event loop answers again");
            hung_logged = false;
        }
        // One ping in flight at a time: a stuck loop must not pile them up.
        if !PINGING.swap(true, Ordering::Relaxed) {
            let _ = app.run_on_main_thread(|| {
                ANSWERED.store(now_ms(), Ordering::Relaxed);
                PINGING.store(false, Ordering::Relaxed);
            });
        }

        let mut guard = RUN.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(run) = guard.as_mut() {
            run.alive = now;
            run.answered = answered;
            save(run);
        }
    }
}

/// Logs why Luna is about to leave and marks the run as ended on purpose.
/// The first reason wins: Quit also passes through the generic exit event.
pub fn exiting(reason: &str) {
    if NOTED.swap(true, Ordering::SeqCst) {
        return;
    }
    crate::log::warn("app", &format!("exiting: {reason}"));
    let mut guard = RUN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(run) = guard.as_mut() {
        run.exit = Some(reason.to_string());
        run.alive = now_ms();
        save(run);
    }
}

/// For the frontend's own way out — the updater ends the process from inside
/// its install call — and to take it back when that install fails.
#[tauri::command]
pub fn note_exit(reason: Option<String>) {
    match reason {
        Some(r) => exiting(&r),
        None => {
            if !NOTED.swap(false, Ordering::SeqCst) {
                return;
            }
            crate::log::warn("app", "not exiting after all");
            let mut guard = RUN.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(run) = guard.as_mut() {
                run.exit = None;
                save(run);
            }
        }
    }
}

/// A tray Quit the window has been asked about and not yet answered.
static QUIT_ASKED: AtomicBool = AtomicBool::new(false);

/// Quit from the tray. With nothing running Luna leaves at once; with
/// sessions running the window asks first, since they go down with it, and
/// answers with `quit_app` or `quit_declined`. A second Quit while that
/// question is open leaves without waiting: a page that cannot answer must
/// not make Luna impossible to quit.
pub fn quit_from_tray(app: &AppHandle) {
    use tauri::Manager;
    let live = app.state::<crate::pty::PtyManager>().live_count();
    if live == 0 || QUIT_ASKED.swap(true, Ordering::SeqCst) {
        quit(app.clone());
        return;
    }
    crate::show_main(app);
    crate::emit::to_ui(app, "app://quit-asked", live);
}

/// Every session is asked to quit, as a delete would, so each leaves a whole
/// transcript behind; then Luna goes. Off the event loop: the asking takes
/// seconds.
fn quit(app: AppHandle) {
    exiting("Quit from the tray menu");
    std::thread::spawn(move || {
        use tauri::Manager;
        app.state::<crate::pty::PtyManager>().shut_down_all(crate::pty::GRACE);
        app.exit(0);
    });
}

#[tauri::command]
pub fn quit_app(app: AppHandle) {
    quit(app);
}

#[tauri::command]
pub fn quit_declined() {
    QUIT_ASKED.store(false, Ordering::SeqCst);
}

/// `luna --watch-exit <pid>`: runs as the watcher and returns true, or false
/// for an ordinary launch.
pub fn watch_from_args() -> bool {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some(watcher::FLAG) {
        return false;
    }
    if let Some(pid) = args.next().and_then(|a| a.parse().ok()) {
        watcher::watch(pid);
    }
    true
}

#[cfg(windows)]
mod watcher {
    use std::os::windows::process::CommandExt;
    use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, WaitForSingleObject, CREATE_BREAKAWAY_FROM_JOB, DETACHED_PROCESS, INFINITE,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_SHUTTINGDOWN};

    pub const FLAG: &str = "--watch-exit";

    /// Detached, so it has no console to lose with the sessions' ones, and out
    /// of any job Luna was started in, so closing that job does not take it.
    /// A job that forbids breaking away refuses the first try; the second
    /// still gives a watcher for every other way Luna can end. Started before
    /// any session exists, so it inherits none of their pipes.
    pub fn spawn() {
        let Ok(exe) = std::env::current_exe() else { return };
        let pid = std::process::id().to_string();
        let start = |flags: u32| {
            std::process::Command::new(&exe)
                .args([FLAG, &pid])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .creation_flags(flags)
                .spawn()
        };
        if start(DETACHED_PROCESS.0 | CREATE_BREAKAWAY_FROM_JOB.0)
            .or_else(|_| start(DETACHED_PROCESS.0))
            .is_err()
        {
            crate::log::warn("app", "could not start the exit watcher");
        }
    }

    pub fn watch(pid: u32) {
        // SAFETY: a handle this function opens, waits on and closes.
        let code = unsafe {
            let Ok(h) = OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
                return;
            };
            let waited = WaitForSingleObject(h, INFINITE);
            let mut code = 0u32;
            let got = GetExitCodeProcess(h, &mut code);
            let _ = CloseHandle(h);
            if waited != WAIT_OBJECT_0 || got.is_err() {
                return;
            }
            code
        };
        // Luna said why it left; a clean code adds nothing to that.
        let said = super::read().is_some_and(|r| r.pid == pid && r.exit.is_some());
        if said && code == 0 {
            return;
        }
        let mut line = format!("luna (pid {pid}) ended with exit code {code:#x}: {}", meaning(code));
        // SAFETY: a plain metric query.
        if unsafe { GetSystemMetrics(SM_SHUTTINGDOWN) } != 0 {
            line.push_str("; Windows was shutting down");
        }
        let open = killers_running();
        if !open.is_empty() {
            line.push_str(&format!("; running at that moment: {}", open.join(", ")));
        }
        crate::log::warn("watch", &line);
    }

    fn meaning(code: u32) -> &'static str {
        match code {
            0 => "it exited by itself without saying why",
            1 => "terminated from outside (Task Manager's End task, taskkill /F, or another program)",
            0xFFFF_FFFF => "terminated from outside (PowerShell Stop-Process, .NET Process.Kill or similar)",
            101 => "a panic on the main thread",
            0xC000_013A => "closed as a console application (Ctrl+C or its console window closed)",
            0xC000_0005 => "crashed: access violation",
            0xC000_00FD => "crashed: stack overflow",
            0xC000_0409 => "crashed: fail-fast (an abort, an allocation failure or a stack buffer overrun)",
            0xC000_0374 => "crashed: heap corruption",
            0xC000_0017 | 0xC000_012D => "crashed: out of memory",
            0x8000_0003 => "crashed: breakpoint",
            c if c & 0xC000_0000 == 0xC000_0000 => "crashed (see the NTSTATUS code)",
            _ => "an exit code nothing in Luna uses; something else ended it",
        }
    }

    /// The usual tools for killing a process by hand. Seeing one open when
    /// Luna was terminated is a strong hint it was a person.
    fn killers_running() -> Vec<String> {
        const NAMES: [&str; 5] = ["taskmgr.exe", "procexp.exe", "procexp64.exe", "systeminformer.exe", "processhacker.exe"];
        let mut found = Vec::new();
        // SAFETY: a snapshot this function takes and closes; the entry is
        // sized as the API requires before the first call.
        unsafe {
            let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return found };
            let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
            let mut ok = Process32FirstW(snap, &mut e).is_ok();
            while ok {
                let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                let name = String::from_utf16_lossy(&e.szExeFile[..len]);
                if NAMES.contains(&name.to_lowercase().as_str()) && !found.contains(&name) {
                    found.push(name);
                }
                ok = Process32NextW(snap, &mut e).is_ok();
            }
            let _ = CloseHandle(snap);
        }
        found
    }
}

/// Elsewhere the run record and the next start's report stand alone: a
/// non-parent cannot read another process's exit code.
#[cfg(not(windows))]
mod watcher {
    pub const FLAG: &str = "--watch-exit";
    pub fn spawn() {}
    pub fn watch(_pid: u32) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(alive: u64, answered: u64) -> Run {
        Run { pid: 7, started: 0, alive, answered, exit: None }
    }

    #[test]
    fn a_freeze_before_the_end_is_named() {
        let s = describe_unannounced(&run(100_000, 10_000), Some(0));
        assert!(s.contains("frozen"), "{s}");
        let s = describe_unannounced(&run(100_000, 95_000), Some(0));
        assert!(!s.contains("frozen"), "{s}");
    }

    #[test]
    fn a_boot_after_the_last_sign_of_life_means_the_pc_went_down() {
        assert!(describe_unannounced(&run(100_000, 100_000), Some(200_000)).contains("PC went down"));
        assert!(!describe_unannounced(&run(100_000, 100_000), Some(50_000)).contains("PC went down"));
        assert!(!describe_unannounced(&run(100_000, 100_000), None).contains("PC went down"));
    }
}
