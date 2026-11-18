//! Where the test build's background window goes: past the right edge of
//! every display, opened without focus, so it covers nothing and is never
//! activated (the patched gpui-pre-windows keeps such a window where it is
//! put and off the taskbar: vendor/PATCHES.md, patch 2).

use gpui::{App, Bounds, Pixels, Size, point, px};

/// A margin past the virtual screen's right edge, in physical pixels.
const MARGIN_PX: i32 = 200;

/// Bounds of `size` past the right edge of every display. gpui-pre-windows
/// converts the origin to physical pixels with the primary display's scale,
/// and each display's logical bounds are scaled by its own factor, so the
/// right edge comes from the virtual screen in physical pixels, divided by the
/// primary's scale: the largest logical right edge would land inside a
/// display scaled more than the primary.
#[cfg(windows)]
pub fn bounds(size: Size<Pixels>, _cx: &App) -> Bounds<Pixels> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTOPRIMARY, MonitorFromPoint};
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN};
    // SAFETY: plain metric queries. The process is per-monitor DPI aware
    // (gpui's manifest, which every exe linking gpui carries), so these are
    // physical pixels.
    let (right, top) =
        unsafe { (GetSystemMetrics(SM_XVIRTUALSCREEN) + GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN)) };
    let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
    // SAFETY: the primary monitor's handle, and two out-parameters of ours.
    let primary = unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) };
    let _ = unsafe { GetDpiForMonitor(primary, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
    let scale = dpi_x.max(96) as f32 / 96.;
    Bounds::new(point(px((right + MARGIN_PX) as f32 / scale), px(top as f32 / scale)), size)
}

/// Elsewhere the displays' own bounds are all there is. A window manager may
/// still place it on screen (Wayland ignores positions); it takes no focus.
#[cfg(not(windows))]
pub fn bounds(size: Size<Pixels>, cx: &App) -> Bounds<Pixels> {
    let right = cx.displays().iter().map(|d| d.bounds().right()).fold(px(0.), |a, b| a.max(b));
    Bounds::new(point(right + px(MARGIN_PX as f32), px(0.)), size)
}
