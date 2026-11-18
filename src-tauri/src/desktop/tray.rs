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
//! called on the loop's thread; the callbacks are `Send + Sync` because the
//! crate asks that of them. They should hand the work to the window and
//! return. No tray on Linux, which Luna does not support: `new` says so.

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
const OPEN: &str = "open";
const RESTART: &str = "restart";
const QUIT: &str = "quit";

/// The tray icon, shown for as long as this lives.
pub struct Tray {
    #[cfg(any(windows, target_os = "macos"))]
    icon: tray_icon::TrayIcon,
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

            let Actions { open, restart, quit } = actions;
            let open: std::sync::Arc<dyn Fn() + Send + Sync> = std::sync::Arc::from(open);
            let on_click = std::sync::Arc::clone(&open);
            MenuEvent::set_event_handler(Some(move |event: MenuEvent| match event.id.as_ref() {
                OPEN => open(),
                RESTART => restart(),
                QUIT => quit(),
                _ => {}
            }));
            TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
                if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                    on_click();
                }
            }));
            Ok(Tray { icon, base })
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
    /// The handlers hold the window's callbacks; they go with the icon.
    fn drop(&mut self) {
        #[cfg(any(windows, target_os = "macos"))]
        {
            tray_icon::menu::MenuEvent::set_event_handler(None::<fn(tray_icon::menu::MenuEvent)>);
            tray_icon::TrayIconEvent::set_event_handler(None::<fn(tray_icon::TrayIconEvent)>);
        }
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
