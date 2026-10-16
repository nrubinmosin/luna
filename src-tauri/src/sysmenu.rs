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
//! be beaten to it.

/// Swallow the messages that open the window menu, for the life of the window.
#[cfg(windows)]
pub fn silence(window: &tauri::WebviewWindow) {
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging::{SC_KEYMENU, SC_MOUSEMENU, WM_SYSCOMMAND};

    /// Only has to be stable and unique among this window's subclasses.
    const ID: usize = 1;

    unsafe extern "system" fn proc_(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        _data: usize,
    ) -> LRESULT {
        if msg == WM_SYSCOMMAND {
            // Windows reserves the low four bits of a system command for its
            // own use; the command itself is the high twelve.
            let cmd = (wparam.0 & 0xfff0) as u32;
            // SC_KEYMENU is the Alt press, SC_MOUSEMENU the click on the frame.
            if cmd == SC_KEYMENU || cmd == SC_MOUSEMENU {
                return LRESULT(0);
            }
        }
        DefSubclassProc(hwnd, msg, wparam, lparam)
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
#[cfg(windows)]
pub fn exit_with_session() {
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumThreadWindows, ENDSESSION_CLOSEAPP, ENDSESSION_CRITICAL, ENDSESSION_LOGOFF, WM_ENDSESSION,
    };

    /// Distinct from the menu hook's id: both may sit on the main window.
    const ID: usize = 2;

    unsafe extern "system" fn proc_(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        _data: usize,
    ) -> LRESULT {
        // wParam FALSE is a shutdown cancelled after WM_QUERYENDSESSION, which
        // nothing here acted on.
        if msg == WM_ENDSESSION && wparam.0 != 0 {
            // From a shutdown, Windows ends the process once this returns; from
            // Restart Manager it waits for the process to end itself. Exiting
            // covers both. The CLI sessions are console processes and get
            // Windows' own shutdown notice; nothing here needs to kill them.
            let flags = lparam.0 as u32;
            let why = if flags & ENDSESSION_CLOSEAPP != 0 {
                "Windows' Restart Manager closed it (an installer or an update wanted its files)"
            } else if flags & ENDSESSION_LOGOFF != 0 {
                "Windows is signing the user out"
            } else if flags & ENDSESSION_CRITICAL != 0 {
                "Windows is shutting down by force"
            } else {
                "Windows is shutting down or restarting"
            };
            crate::lifecycle::exiting(why);
            std::process::exit(0);
        }
        DefSubclassProc(hwnd, msg, wparam, lparam)
    }

    unsafe extern "system" fn hook(hwnd: HWND, count: LPARAM) -> BOOL {
        if SetWindowSubclass(hwnd, Some(proc_), ID, 0).as_bool() {
            *(count.0 as *mut usize) += 1;
        }
        BOOL(1) // keep enumerating
    }

    let mut hooked: usize = 0;
    // SAFETY: enumerates the calling thread's own windows and subclasses them
    // from that same thread; `hooked` outlives the synchronous enumeration.
    unsafe {
        let _ = EnumThreadWindows(
            GetCurrentThreadId(),
            Some(hook),
            LPARAM(&mut hooked as *mut usize as isize),
        );
    }
    if hooked == 0 {
        crate::log::warn("app", "could not hook WM_ENDSESSION on any window");
    }
}

#[cfg(not(windows))]
pub fn silence(_window: &tauri::WebviewWindow) {}

#[cfg(not(windows))]
pub fn exit_with_session() {}
