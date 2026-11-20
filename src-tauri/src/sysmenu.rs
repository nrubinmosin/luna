//! Two things the window has to be told about Windows, both done by
//! subclassing — hooking the window procedure for the life of the window.
//!
//! One is Windows' own window menu — Restore/Move/Size/Close — which it opens
//! on Alt and on a click in the top-left corner of the frame. The window is
//! undecorated, so that menu has nothing to act on that the app's own title bar
//! does not already offer, and it is squarely in the way: Alt is half of
//! Alt+Shift, so switching keyboard layout landed on it. Two of the messages
//! that raise it are swallowed and the rest of the system menu is left alone,
//! so Alt+F4 and the taskbar's thumbnail menu still work — which is why this
//! subclasses the window rather than dropping WS_SYSMENU, the shorter fix that
//! would have taken those with it.
//!
//! The other is the end of the Windows session. On sign-out, shutdown and
//! reboot, Windows sends every top-level window WM_ENDSESSION and then ends the
//! process itself. tao exits right there too, so the hook is not what ends
//! the process; it is what gets the reason Windows gives into the log first
//! (lifecycle.rs) — on every top-level window of the thread, because Windows
//! hands the message to them one at a time and tao's own hidden window has to
//! be beaten to it. Under the native window the hook first writes what the
//! host holds of the window's (the chat list first: `drain_with_session`,
//! bounded, on this thread, whatever gpui is doing), then says why with how
//! that went, and passes the message on: gpui's hidden platform window runs
//! the app's quit (the window's `on_quit`: its place and theme written, the
//! tray removed, events stopped, the power request let go) and then exits, as
//! the tray's Quit does. gpui cannot run that quit while its app is borrowed
//! (a message handled inside an update): it then only posts WM_QUIT, which
//! Windows may not wait for, so the drain is the hook's own and the run is
//! marked ended on purpose only after it.

#[cfg(windows)]
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

/// How Windows is ending the session, once it has said: not yet (0),
/// Restart Manager closing Luna alone (1), the whole session (2).
#[cfg(windows)]
static ENDING: AtomicU8 = AtomicU8::new(0);

/// The hook ends the process itself (the WebView window), rather than pass
/// the message on to the toolkit's own handler (the native window's gpui).
#[cfg(windows)]
static EXIT_HERE: AtomicBool = AtomicBool::new(true);

/// What the hook writes before it passes the message on (the native
/// window's writers, `drain_with_session`): Ok, or what was not written.
#[cfg(windows)]
static DRAIN: std::sync::OnceLock<fn() -> Result<(), String>> = std::sync::OnceLock::new();

/// Has the session's end, under the native window, write what the host
/// holds before the message goes on to gpui (`drain`: bounded, safe to run
/// on the window's thread whatever gpui is doing). Set before
/// `exit_with_session(false)`.
pub fn drain_with_session(drain: fn() -> Result<(), String>) {
    #[cfg(windows)]
    let _ = DRAIN.set(drain);
    #[cfg(not(windows))]
    let _ = drain;
}

/// Whether Windows is ending the session at all (a shutdown, a restart, a
/// sign-out, or Restart Manager closing Luna): the hook has already drained
/// the writers once (`drain_with_session`).
pub fn session_ending() -> bool {
    #[cfg(windows)]
    {
        ENDING.load(Ordering::SeqCst) != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Whether Windows is ending the whole session (a shutdown, a restart, a
/// sign-out): a way out then waits for no model ask, which ends with
/// everything else, where Restart Manager closing Luna alone ends only Luna.
pub fn session_ending_whole() -> bool {
    #[cfg(windows)]
    {
        ENDING.load(Ordering::SeqCst) == 2
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// What WM_ENDSESSION's flags say ends the session, for the log; and whether
/// it is Restart Manager closing Luna alone.
#[cfg(windows)]
fn why(flags: u32) -> (&'static str, bool) {
    use windows::Win32::UI::WindowsAndMessaging::{ENDSESSION_CLOSEAPP, ENDSESSION_CRITICAL, ENDSESSION_LOGOFF};
    if flags & ENDSESSION_CLOSEAPP != 0 {
        ("Windows' Restart Manager closed it (an installer or an update wanted its files)", true)
    } else if flags & ENDSESSION_LOGOFF != 0 {
        ("Windows is signing the user out", false)
    } else if flags & ENDSESSION_CRITICAL != 0 {
        ("Windows is shutting down by force", false)
    } else {
        ("Windows is shutting down or restarting", false)
    }
}

/// The session's end's reason, once the native window's writers were
/// drained (`DRAIN`), with how that went.
#[cfg(windows)]
fn drained(why: &str) -> String {
    match DRAIN.get().map(|drain| drain()) {
        Some(Ok(())) => format!("{why}; what the window holds was written"),
        Some(Err(e)) => {
            crate::log::error("app", &format!("as the session ended: {e}"));
            format!("{why}; not all the window holds was written ({e})")
        }
        None => why.to_owned(),
    }
}

/// Swallow the messages that open the window menu, for the life of the window.
#[cfg(windows)]
pub fn silence(window: &tauri::WebviewWindow) {
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging::{SC_KEYMENU, SC_MOUSEMENU, WM_SYSCOMMAND};

    /// Only has to be stable and unique among this window's subclasses.
    const ID: usize = 1;

    unsafe extern "system" fn proc_(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
        if msg == WM_SYSCOMMAND {
            // Windows reserves the low four bits of a system command for its
            // own use; the command itself is the high twelve.
            let cmd = (wparam.0 & 0xfff0) as u32;
            // SC_KEYMENU is the Alt press, SC_MOUSEMENU the click on the frame.
            if cmd == SC_KEYMENU || cmd == SC_MOUSEMENU {
                return LRESULT(0);
            }
        }
        // SAFETY: passes on, unchanged, a message Windows handed this subclass.
        unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
    }

    let hwnd = match window.hwnd() {
        Ok(hwnd) => hwnd,
        Err(e) => {
            crate::log::warn("app", &format!("no window handle, Alt menu stays: {e}"));
            return;
        }
    };
    // SAFETY: the handle belongs to the window just handed to us, and this runs
    // once from setup, on the thread that owns it.
    let installed = unsafe { SetWindowSubclass(hwnd, Some(proc_), ID, 0) };
    if !installed.as_bool() {
        crate::log::warn("app", "could not subclass the window; Alt menu stays");
    }
}

/// Leave cleanly when Windows ends the session, before the event loop is torn
/// down underneath the messages still bound for it. Hooks every top-level
/// window this thread owns, so it does not matter which one Windows asks first.
/// `exit_here`: the hook ends the process itself (the WebView window, whose
/// tao exits on the message); else it says why and passes the message on to
/// the toolkit, which quits the app the way the tray's Quit does (the native
/// window: gpui's hidden platform window runs the app's quit, then exits).
#[cfg(windows)]
pub fn exit_with_session(exit_here: bool) {
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging::{EnumThreadWindows, WM_ENDSESSION};

    /// Distinct from the menu hook's id: both may sit on the main window.
    const ID: usize = 2;

    unsafe extern "system" fn proc_(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
        // wParam FALSE is a shutdown canceled after WM_QUERYENDSESSION, which
        // nothing here acted on.
        if msg == WM_ENDSESSION && wparam.0 != 0 {
            // From a shutdown, Windows ends the process once this returns; from
            // Restart Manager it waits for the process to end itself. Exiting
            // covers both. The CLI sessions are console processes and get
            // Windows' own shutdown notice; nothing here needs to kill them.
            let (why, alone) = why(lparam.0 as u32);
            let first = ENDING.swap(if alone { 1 } else { 2 }, Ordering::SeqCst) == 0;
            if !EXIT_HERE.load(Ordering::SeqCst) {
                // The native window: what the host holds is written here,
                // once, whichever window Windows tells first, and the run is
                // marked as ended on purpose only once that is done, with
                // how it went. Killed meanwhile, the next start says the run
                // ended without closing itself, which is then the truth.
                if first {
                    crate::lifecycle::exiting(&drained(why));
                }
            } else {
                crate::lifecycle::exiting(why);
                // Restart Manager ends Luna alone: a model ask's CLI, in a job
                // Luna's death does not end, would be left running unswept, so
                // it gets its grace here as at any other exit (up to 12 s, on
                // this thread: the ask's cleanup needs no window). A shutdown
                // or a sign-out ends the CLI with everything else; waiting
                // would only hold that up (`session_ending_whole`).
                if alone {
                    crate::claude::models::wait_for_asks();
                }
                std::process::exit(0);
            }
        }
        // SAFETY: passes on, unchanged, a message Windows handed this subclass.
        unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
    }

    unsafe extern "system" fn hook(hwnd: HWND, count: LPARAM) -> BOOL {
        // SAFETY: called back from the EnumThreadWindows below, on the thread
        // that owns `hwnd`, with `count` pointing at its `hooked`.
        unsafe {
            if SetWindowSubclass(hwnd, Some(proc_), ID, 0).as_bool() {
                *(count.0 as *mut usize) += 1;
            }
        }
        BOOL(1) // keep enumerating
    }

    EXIT_HERE.store(exit_here, Ordering::SeqCst);
    let mut hooked: usize = 0;
    // SAFETY: enumerates the calling thread's own windows and subclasses them
    // from that same thread; `hooked` outlives the synchronous enumeration.
    unsafe {
        let _ = EnumThreadWindows(GetCurrentThreadId(), Some(hook), LPARAM(&mut hooked as *mut usize as isize));
    }
    if hooked == 0 {
        crate::log::warn("app", "could not hook WM_ENDSESSION on any window");
    }
}

#[cfg(not(windows))]
pub fn silence(_window: &tauri::WebviewWindow) {}

#[cfg(not(windows))]
pub fn exit_with_session(_exit_here: bool) {}

#[cfg(all(test, windows))]
mod tests {
    use super::why;
    use windows::Win32::UI::WindowsAndMessaging::{ENDSESSION_CLOSEAPP, ENDSESSION_CRITICAL, ENDSESSION_LOGOFF};

    /// Restart Manager closing Luna alone is told from the whole session
    /// ending: only it waits for a model ask.
    #[test]
    fn the_reason_tells_restart_manager_from_the_session_ending() {
        assert!(why(ENDSESSION_CLOSEAPP).1);
        assert!(why(ENDSESSION_CLOSEAPP).0.contains("Restart Manager"));
        for whole in [0, ENDSESSION_LOGOFF, ENDSESSION_CRITICAL, ENDSESSION_LOGOFF | ENDSESSION_CRITICAL] {
            assert!(!why(whole).1, "{whole:#x}");
        }
        assert!(why(ENDSESSION_LOGOFF).0.contains("signing the user out"));
    }
}
