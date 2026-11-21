//! One scale for the whole window, the terminal included: the person's
//! choice (`WindowState::ui_scale`, Settings and Ctrl+= / Ctrl+- / Ctrl+0)
//! times the system's text size. GPUI already draws at the display's own
//! scaling (per-monitor DPI on Windows, a display's backing scale on macOS
//! and Linux); this is on top of it, as a page zoom was on top of it in
//! the WebView window.
//!
//! Windows' "Text size" (Accessibility) is the system's text size: WebView2
//! applied it to 0.19's page as a zoom, and GPUI knows nothing of it, so
//! without it a window that looked right at 155% came out at two thirds of
//! the size. Other platforms have no such setting apart from the display's
//! scaling, so there it is 1.
//!
//! Every length of the chrome is written at 100% and goes through `u`;
//! widths the views add up themselves (the status bar's fit, a pane's
//! width) are in those same units, the window's width divided by the scale
//! (`width`). The scale is the window thread's (a thread local), since
//! every view is drawn there and each test runs on its own thread.

use crate::theme;
use gpui::{App, Pixels, Window, px};
use std::cell::Cell;

/// The choices, in percent: what Settings steps through and the keys move
/// between.
pub const STEPS: [u16; 10] = [80, 90, 100, 110, 125, 150, 175, 200, 225, 250];
/// The choice a window starts with, and what Ctrl+0 goes back to.
pub const DEFAULT: u16 = 100;

/// The size of an em in GPUI's rem units at 100% (its own default), which
/// gpui-component's inputs and the `w_4`-style lengths are given in.
const REM: f32 = 16.;

thread_local! {
    /// The scale the views draw at: the choice times the system's text size.
    static SCALE: Cell<f32> = const { Cell::new(1.) };
    /// Whether the system's text size counts: off in the shots, which are
    /// taken at the scale they name whatever the machine's setting.
    static FOLLOW_SYSTEM: Cell<bool> = const { Cell::new(true) };
}

/// The scale the window draws at now.
pub fn current() -> f32 {
    SCALE.with(Cell::get)
}

/// A length of the chrome, given at 100%, at the window's scale.
pub fn u(n: f32) -> Pixels {
    px(n * current())
}

/// The window's width in the units of the chrome's lengths (`u`).
pub fn width(window: &Window) -> f32 {
    f32::from(window.viewport_size().width) / current()
}

/// The chrome's text size.
pub fn text() -> Pixels {
    theme::TEXT_SIZE * current()
}

/// A kept choice held to the steps: the nearest one.
pub fn snap(pct: u16) -> u16 {
    *STEPS.iter().min_by_key(|s| s.abs_diff(pct)).unwrap_or(&DEFAULT)
}

/// The step after `pct` (`up`) or before it, the ends staying where they are.
pub fn step(pct: u16, up: bool) -> u16 {
    let pct = snap(pct);
    let i = STEPS.iter().position(|s| *s == pct).unwrap_or(0);
    let i = if up { (i + 1).min(STEPS.len() - 1) } else { i.saturating_sub(1) };
    STEPS[i]
}

/// The scale a choice draws at, with the system's text size `system`.
pub fn of(pct: u16, system: f32) -> f32 {
    f32::from(snap(pct)) / 100. * system
}

/// Takes the system's text size out of the window's scale (the shots).
#[cfg(feature = "shots")]
pub fn ignore_system() {
    FOLLOW_SYSTEM.with(|f| f.set(false));
}

/// The system's text size, 1 for none: Windows' "Text size" (100% to
/// 225%), read each time it is asked, since it changes under a running
/// window without a message to it. The crate's tests take none, so they
/// lay out the same on every machine.
pub fn system() -> f32 {
    if !FOLLOW_SYSTEM.with(Cell::get) {
        return 1.;
    }
    system_text_size()
}

#[cfg(all(windows, not(test)))]
fn system_text_size() -> f32 {
    windows_text_scale().map_or(1., |pct| f32::from(pct.clamp(100, 225) as u16) / 100.)
}

#[cfg(not(all(windows, not(test))))]
fn system_text_size() -> f32 {
    1.
}

/// `TextScaleFactor` under the user's Accessibility key: what Settings >
/// Accessibility > Text size writes, in percent. None where it was never
/// set, which is 100%.
#[cfg(all(windows, not(test)))]
fn windows_text_scale() -> Option<u32> {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    use windows::core::w;
    let mut value = 0u32;
    let mut len = std::mem::size_of::<u32>() as u32;
    // SAFETY: the buffer is a u32 and `len` its size; RegGetValueW writes no
    // more than that for a DWORD and fails for any other type.
    let read = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Accessibility"),
            w!("TextScaleFactor"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut value).cast()),
            Some(&raw mut len),
        )
    };
    read.is_ok().then_some(value)
}

/// Draws the window at `scale` from the next frame: the chrome's lengths,
/// GPUI's rem (gpui-component's inputs), gpui-component's text, and the
/// terminals' text, whose panes measure their cells again and size their
/// PTYs to fit (term/element.rs). Nothing when it is the scale already.
pub fn apply(scale: f32, window: &mut Window, cx: &mut App) {
    if (scale - current()).abs() < 0.001 && (f32::from(window.rem_size()) - REM * scale).abs() < 0.001 {
        return;
    }
    SCALE.with(|s| s.set(scale));
    window.set_rem_size(px(REM * scale));
    theme::set_component_text(text(), cx);
    // Every view again, the cached ones included.
    window.refresh();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A kept choice off the steps snaps to the nearest; the keys stop at
    /// the ends.
    #[test]
    fn choices_snap_to_the_steps_and_stop_at_the_ends() {
        assert_eq!(snap(100), 100);
        assert_eq!(snap(130), 125);
        assert_eq!(snap(0), 80);
        assert_eq!(snap(900), 250);
        assert_eq!(step(100, true), 110);
        assert_eq!(step(100, false), 90);
        assert_eq!(step(250, true), 250);
        assert_eq!(step(80, false), 80);
        assert_eq!(step(130, true), 150, "from the step it snaps to");
    }

    /// The choice times the system's text size.
    #[test]
    fn the_scale_is_the_choice_times_the_system_text_size() {
        assert_eq!(of(100, 1.), 1.);
        assert_eq!(of(125, 1.), 1.25);
        assert!((of(100, 1.55) - 1.55).abs() < 1e-6);
        assert!((of(150, 1.5) - 2.25).abs() < 1e-6);
    }
}
