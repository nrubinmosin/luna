//! The tray icon: Luna's icon with the tooltip "Luna", a menu of Open,
//! Restart window and Quit, and a left click that opens the window, as the
//! Tauri window has it (lib.rs `run`). While the PC is held awake the icon
//! carries a dot and the tooltip says who holds it (`Tray::set_awake`).
//!
//! On `tray-icon`, the crate Tauri's tray is built on. It needs an event loop
//! on the thread that makes the icon, and on macOS that thread must be the
//! main one: so `Tray::new` is called from the window's own loop, which on
//! both platforms is the main thread's (GPUI's), after the app has started.
//! The icon lives as long as the `Tray`; drop it on the same thread.
//!
//! The menu's and the icon's events reach one handler each for the whole
//! process (`MenuEvent::set_event_handler`, `TrayIconEvent::set_event_handler`),
//! called on the loop's thread. Luna sets them once and they call the newest
//! tray's actions (`CURRENT`), so trays may overlap as one replaces another.
//! The callbacks are `Send + Sync` because the crate asks that of its
//! handlers. They should hand the work to the window and return. No tray on Linux, which Luna does not support: `new` says so.

/// What the tray's menu and click do, supplied by the window.
pub struct Actions {
    /// Open, and a left click on the icon: bring the window up.
    pub open: Box<dyn Fn() + Send + Sync>,
    /// Restart window: the window only; the sessions run on in the core.
    pub restart: Box<dyn Fn() + Send + Sync>,
    /// Quit: the sessions, the core and the window together.
    pub quit: Box<dyn Fn() + Send + Sync>,
}

/// An image as RGBA bytes, row by row from the top.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// The tray's icon: 32 px, which Windows scales down to its 16 px tray slot
/// at 100% and shows as it is at 200%.
const ICON_PNG: &[u8] = include_bytes!("../../icons/32x32.png");

/// Windows cuts a tray tooltip at 127 characters.
const TOOLTIP_MAX: usize = 127;

/// The icon decoded.
pub fn base_icon() -> Result<Rgba, String> {
    decode_png(ICON_PNG)
}

/// A PNG as 8-bit RGBA, whatever color type it was stored in.
pub fn decode_png(bytes: &[u8]) -> Result<Rgba, String> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    // Palette and low bit depths expanded, 16 bits cut to 8.
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|e| format!("the icon is not a PNG: {e}"))?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("the icon is too large")?];
    let info = reader.next_frame(&mut buf).map_err(|e| format!("the icon does not decode: {e}"))?;
    buf.truncate(info.buffer_size());
    let pixels = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf.as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 0xFF]).collect(),
        png::ColorType::GrayscaleAlpha => buf.as_chunks::<2>().0.iter().flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 0xFF]).collect(),
        png::ColorType::Indexed => return Err("the icon's palette was not expanded".into()),
    };
    Ok(Rgba { width: info.width, height: info.height, pixels })
}

/// The icon with an amber dot, ringed in black, at its lower right: the PC
/// is held awake.
pub fn with_dot(base: &Rgba) -> Rgba {
    let (w, h) = (base.width as i64, base.height as i64);
    let mut pixels = base.pixels.clone();
    let r = (w.min(h) as f64 * 0.24).max(2.0);
    let (cx, cy) = (w as f64 - r - 1.0, h as f64 - r - 1.0);
    for y in 0..h {
        for x in 0..w {
            let d = ((x as f64 + 0.5 - cx).powi(2) + (y as f64 + 0.5 - cy).powi(2)).sqrt();
            let px = ((y * w + x) * 4) as usize;
            if d <= r - 1.5 {
                pixels[px..px + 4].copy_from_slice(&[0xF5, 0xA6, 0x23, 0xFF]);
            } else if d <= r {
                pixels[px..px + 4].copy_from_slice(&[0x10, 0x10, 0x10, 0xFF]);
            }
        }
    }
    Rgba { width: base.width, height: base.height, pixels }
}

/// The tooltip: the product's name, or who holds the PC awake.
pub fn tooltip(holders: Option<&str>) -> String {
    let tip = match holders {
        Some(h) => format!("{} — keeping the PC awake:\n{h}", super::product_name()),
        None => super::product_name().to_string(),
    };
    if tip.chars().count() > TOOLTIP_MAX {
        format!("{}…", tip.chars().take(TOOLTIP_MAX - 1).collect::<String>())
    } else {
        tip
    }
}

/// The menu's items, by id.
pub const OPEN: &str = "open";
pub const RESTART: &str = "restart";
pub const QUIT: &str = "quit";

/// The actions of the tray shown now, by its number. `tray-icon` keeps one
/// menu handler and one icon handler for the whole process: Luna sets them
/// once (`HANDLERS`), and they reach whichever tray is current through this,
/// so that a tray made before the last one is dropped keeps working, and
/// the last one's drop does not take the new one's handlers with it.
static CURRENT: std::sync::Mutex<Option<(u64, std::sync::Arc<Actions>)>> = std::sync::Mutex::new(None);

/// The number the next tray's actions go by.
static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Makes `actions` the current tray's; returns the number they go by.
fn make_current(actions: Actions) -> u64 {
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = Some((id, std::sync::Arc::new(actions)));
    id
}

/// Lets the actions numbered `id` go, if they are still the current ones.
fn let_go(id: u64) {
    let mut current = CURRENT.lock().unwrap_or_else(|e| e.into_inner());
    if current.as_ref().is_some_and(|(now, _)| *now == id) {
        *current = None;
    }
}

/// Runs what the current tray's menu item `item` does, if anything, as a
/// click on it does; the lock is let go before the callback runs. The test
/// build's hook (native/desktop.rs) calls it for a script.
pub fn on_menu(item: &str) {
    let Some(actions) = CURRENT.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|(_, a)| std::sync::Arc::clone(a)) else {
        return;
    };
    match item {
        OPEN => (actions.open)(),
        RESTART => (actions.restart)(),
        QUIT => (actions.quit)(),
        _ => {}
    }
}

/// The tray icon, shown for as long as this lives.
pub struct Tray {
    #[cfg(any(windows, target_os = "macos"))]
    icon: tray_icon::TrayIcon,
    #[cfg(any(windows, target_os = "macos"))]
    id: u64,
    base: Rgba,
}

impl Tray {
    /// Shows the icon, its menu wired to `actions`. On the event loop's
    /// thread, the main one (see the module's comment).
    pub fn new(actions: Actions) -> Result<Tray, String> {
        #[cfg(any(windows, target_os = "macos"))]
        {
            use tray_icon::menu::{Menu, MenuEvent, MenuItem};
            use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

            let base = base_icon()?;
            let open = MenuItem::with_id(OPEN, "Open", true, None);
            let restart = MenuItem::with_id(RESTART, "Restart window", true, None);
            let quit = MenuItem::with_id(QUIT, "Quit", true, None);
            let menu = Menu::with_items(&[&open, &restart, &quit]).map_err(|e| format!("no tray menu: {e}"))?;
            let icon = TrayIconBuilder::new()
                .with_id("main")
                .with_icon(to_icon(&base)?)
                .with_tooltip(tooltip(None))
                .with_menu(Box::new(menu))
                // The left click opens the window; the menu is the right click's.
                .with_menu_on_left_click(false)
                .build()
                .map_err(|e| format!("no tray icon: {e}"))?;

            static HANDLERS: std::sync::Once = std::sync::Once::new();
            HANDLERS.call_once(|| {
                MenuEvent::set_event_handler(Some(|event: MenuEvent| on_menu(event.id.as_ref())));
                TrayIconEvent::set_event_handler(Some(|event: TrayIconEvent| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                        on_menu(OPEN);
                    }
                }));
            });
            Ok(Tray { icon, id: make_current(actions), base })
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            drop(actions);
            Err("Luna has no tray on this platform".into())
        }
    }

    /// The tray while the PC is held awake: the dot on the icon and who holds
    /// it in the tooltip, so a hidden window still says why the PC will not
    /// sleep. `None` puts the plain icon back.
    pub fn set_awake(&self, holders: Option<&str>) -> Result<(), String> {
        #[cfg(any(windows, target_os = "macos"))]
        {
            let image = if holders.is_some() { with_dot(&self.base) } else { self.base.clone() };
            self.icon.set_icon(Some(to_icon(&image)?)).map_err(|e| e.to_string())?;
            self.icon.set_tooltip(Some(tooltip(holders))).map_err(|e| e.to_string())
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            let _ = (holders, &self.base);
            Ok(())
        }
    }
}

impl Drop for Tray {
    /// The window's callbacks go with the icon, unless a newer tray's have
    /// taken their place; the process-wide handlers stay.
    fn drop(&mut self) {
        #[cfg(any(windows, target_os = "macos"))]
        let_go(self.id);
    }
}

#[cfg(any(windows, target_os = "macos"))]
fn to_icon(image: &Rgba) -> Result<tray_icon::Icon, String> {
    tray_icon::Icon::from_rgba(image.pixels.clone(), image.width, image.height).map_err(|e| format!("not an icon: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(image: &Rgba, x: u32, y: u32) -> &[u8] {
        let at = ((y * image.width + x) * 4) as usize;
        &image.pixels[at..at + 4]
    }

    /// The handlers' dispatch, without an icon (the unit tests make none):
    /// a tray made before the last is dropped keeps its actions.
    #[test]
    fn the_newest_trays_actions_answer_whatever_order_the_trays_go_in() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        let (old, new) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let counting = |n: &Arc<AtomicUsize>| {
            let (a, b, c) = (Arc::clone(n), Arc::clone(n), Arc::clone(n));
            Actions {
                open: Box::new(move || {
                    a.fetch_add(1, Ordering::SeqCst);
                }),
                restart: Box::new(move || {
                    b.fetch_add(10, Ordering::SeqCst);
                }),
                quit: Box::new(move || {
                    c.fetch_add(100, Ordering::SeqCst);
                }),
            }
        };
        let first = make_current(counting(&old));
        let second = make_current(counting(&new));
        let_go(first);
        on_menu(OPEN);
        on_menu(RESTART);
        on_menu(QUIT);
        on_menu("something else");
        assert_eq!((old.load(Ordering::SeqCst), new.load(Ordering::SeqCst)), (0, 111));
        let_go(second);
        on_menu(OPEN);
        assert_eq!(new.load(Ordering::SeqCst), 111);
    }

    #[test]
    fn the_icon_decodes_to_32_pixels_square() {
        let icon = base_icon().unwrap();
        assert_eq!((icon.width, icon.height), (32, 32));
        assert_eq!(icon.pixels.len(), 32 * 32 * 4);
    }

    #[test]
    fn the_dot_sits_at_the_lower_right_and_leaves_the_rest() {
        let base = Rgba { width: 32, height: 32, pixels: vec![0x22; 32 * 32 * 4] };
        let dotted = with_dot(&base);
        // r = 7.68, centered at (23.32, 23.32): amber inside, a black ring.
        assert_eq!(pixel(&dotted, 23, 23), [0xF5, 0xA6, 0x23, 0xFF]);
        assert_eq!(pixel(&dotted, 23, 16), [0x10, 0x10, 0x10, 0xFF]);
        assert_eq!(pixel(&dotted, 0, 0), pixel(&base, 0, 0));
        assert_eq!(pixel(&dotted, 31, 0), pixel(&base, 31, 0));
    }

    #[test]
    fn the_tooltip_names_who_holds_the_pc_and_fits_windows() {
        assert_eq!(tooltip(None), crate::desktop::product_name());
        let tip = tooltip(Some("chat A"));
        assert!(tip.ends_with("keeping the PC awake:\nchat A"), "{tip}");
        let long = tooltip(Some(&"x".repeat(300)));
        assert_eq!(long.chars().count(), TOOLTIP_MAX);
        assert!(long.ends_with('…'));
    }

    #[test]
    fn every_color_type_comes_out_rgba() {
        // A 1x1 gray PNG, encoded here.
        let mut bytes = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut bytes, 1, 1);
            enc.set_color(png::ColorType::Grayscale);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header().unwrap().write_image_data(&[0x80]).unwrap();
        }
        assert_eq!(decode_png(&bytes).unwrap(), Rgba { width: 1, height: 1, pixels: vec![0x80, 0x80, 0x80, 0xFF] });
        assert!(decode_png(b"not a png").is_err());
    }
}
