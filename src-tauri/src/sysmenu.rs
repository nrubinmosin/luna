//! Windows' own window menu — Restore/Move/Size/Close — which it opens on Alt
//! and on a click in the top-left corner of the frame.
//!
//! The window is undecorated, so that menu has nothing to act on that the app's
//! own title bar does not already offer, and it is squarely in the way: Alt is
//! half of Alt+Shift, so switching keyboard layout landed on it.
//!
//! Two of the messages that raise it are swallowed and the rest of the system
//! menu is left alone, so Alt+F4 and the taskbar's thumbnail menu still work —
//! which is why this subclasses the window rather than dropping WS_SYSMENU, the
//! shorter fix that would have taken those with it.

/// Swallow the messages that open the window menu, for the life of the window.
#[cfg(windows)]
pub fn silence(window: &tauri::WebviewWindow) {
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging::{SC_KEYMENU, SC_MOUSEMENU, WM_SYSCOMMAND};

    /// Only has to be stable and unique among this window's subclasses, of
    /// which there is one.
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

#[cfg(not(windows))]
pub fn silence(_window: &tauri::WebviewWindow) {}
