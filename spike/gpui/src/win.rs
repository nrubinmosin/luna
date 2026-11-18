// Where an off-screen window goes: past the right edge of every display.

use gpui::{App, Bounds, Pixels, Size, point, px};

/// A margin past the virtual screen's right edge, in physical pixels.
const MARGIN_PX: i32 = 200;

/// Bounds of `size` past the right edge of every display, for a window opened
/// with `focus: false` (which the patched gpui-pre-windows keeps where it is
/// put). gpui-pre-windows converts the origin to physical pixels with the
/// primary display's scale, and each display's logical bounds are scaled by
/// its own factor, so the right edge comes from the virtual screen in physical
/// pixels and is divided by the primary's scale: the largest logical right
/// edge would land inside a display scaled more than the primary.
#[cfg(windows)]
pub fn offscreen_bounds(size: Size<Pixels>, _cx: &App) -> Bounds<Pixels> {
    use windows::Win32::{
        Foundation::POINT,
        Graphics::Gdi::{MONITOR_DEFAULTTOPRIMARY, MonitorFromPoint},
        UI::{
            HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI},
            WindowsAndMessaging::{
                GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
            },
        },
    };
    // The process is per-monitor DPI aware (gpui-pre's manifest), so these
    // are physical pixels.
    let (right, top) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN) + GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
        )
    };
    let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
    let primary = unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) };
    let _ = unsafe { GetDpiForMonitor(primary, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
    let scale = dpi_x.max(96) as f32 / 96.;
    Bounds::new(
        point(px((right + MARGIN_PX) as f32 / scale), px(top as f32 / scale)),
        size,
    )
}

/// Elsewhere the displays' own bounds are all there is.
#[cfg(not(windows))]
pub fn offscreen_bounds(size: Size<Pixels>, cx: &App) -> Bounds<Pixels> {
    let right = cx
        .displays()
        .iter()
        .map(|d| d.bounds().right())
        .fold(px(0.), |a, b| a.max(b));
    Bounds::new(point(right + px(MARGIN_PX as f32), px(0.)), size)
}

/// The primary display's refresh rate, for late frames against its vsync.
#[cfg(windows)]
pub fn refresh_hz() -> Option<f64> {
    use windows::Win32::Graphics::Gdi::{DEVMODEW, ENUM_CURRENT_SETTINGS, EnumDisplaySettingsW};
    let mut mode = DEVMODEW {
        dmSize: std::mem::size_of::<DEVMODEW>() as u16,
        ..Default::default()
    };
    let ok = unsafe { EnumDisplaySettingsW(None, ENUM_CURRENT_SETTINGS, &mut mode) }.as_bool();
    (ok && mode.dmDisplayFrequency > 1).then_some(mode.dmDisplayFrequency as f64)
}

#[cfg(not(windows))]
pub fn refresh_hz() -> Option<f64> {
    None
}
