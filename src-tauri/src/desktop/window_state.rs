//! The window's size, place and maximized state, kept from one run to the
//! next as Tauri's window-state plugin kept them, but in the data dir
//! (`window-state.json`) rather than under the identifier in the profile.
//! What 0.19 saved there is not read: the first native start opens as a
//! fresh install does.
//!
//! The window saves its normal bounds (where it goes when not maximized)
//! and whether it is maximized; on the next start `fit` holds them to the
//! displays there are now, so a window last seen on a monitor since
//! unplugged comes back on one that is there, whole. All in the coordinates
//! the window's toolkit gives the displays in (GPUI's logical pixels); this
//! module only compares them.
//!
//! The test build run in the background (`crate::background`) neither
//! restores nor saves: its window is placed off screen on purpose, and the
//! next run that is not in the background must not open there.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A rectangle: its top left and size.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Bounds {
    fn overlap(&self, other: &Bounds) -> f64 {
        let w = (self.x + self.width).min(other.x + other.width) - self.x.max(other.x);
        let h = (self.y + self.height).min(other.y + other.height) - self.y.max(other.y);
        if w > 0.0 && h > 0.0 {
            w * h
        } else {
            0.0
        }
    }
}

/// What is kept of the window.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowState {
    /// Where the window is when not maximized.
    pub bounds: Bounds,
    pub maximized: bool,
}

/// The file in the data dir.
pub fn path() -> PathBuf {
    crate::paths::data_dir().join("window-state.json")
}

/// What the last run saved; `None` the first time, for a file that does not
/// read, and in the background.
pub fn load() -> Option<WindowState> {
    if crate::background() {
        return None;
    }
    load_from(&path())
}

/// Keeps `state` for the next run. Not in the background.
pub fn save(state: &WindowState) -> Result<(), String> {
    if crate::background() {
        return Ok(());
    }
    save_to(&path(), state)
}

pub fn load_from(path: &Path) -> Option<WindowState> {
    let text = std::fs::read(path).ok()?;
    let state: WindowState = serde_json::from_slice(&text).ok()?;
    let b = state.bounds;
    let sane = [b.x, b.y, b.width, b.height].iter().all(|v| v.is_finite()) && b.width > 0.0 && b.height > 0.0;
    sane.then_some(state)
}

pub fn save_to(path: &Path, state: &WindowState) -> Result<(), String> {
    let text = serde_json::to_vec_pretty(state).map_err(|e| e.to_string())?;
    crate::secret::write_private(path, &text).map_err(|e| format!("{}: {e}", path.display()))
}

/// `state` held to `displays` (their usable areas, the first the primary):
/// no smaller than `min`, no larger than the display it lands on, and wholly
/// on it. It lands on the display it overlaps most, or, overlapping none,
/// centered on the primary. No displays known: as it was.
pub fn fit(state: WindowState, displays: &[Bounds], min: (f64, f64)) -> WindowState {
    let Some(primary) = displays.first() else { return state };
    let b = state.bounds;
    let best = displays.iter().map(|d| (d, b.overlap(d))).filter(|(_, o)| *o > 0.0).max_by(|a, z| a.1.total_cmp(&z.1)).map(|(d, _)| d);
    let display = best.unwrap_or(primary);
    // A display smaller than the minimum gets the window at its own size.
    let width = b.width.max(min.0).min(display.width);
    let height = b.height.max(min.1).min(display.height);
    let (x, y) = match best {
        Some(_) => (b.x, b.y),
        None => (display.x + (display.width - width) / 2.0, display.y + (display.height - height) / 2.0),
    };
    let x = x.clamp(display.x, display.x + display.width - width);
    let y = y.clamp(display.y, display.y + display.height - height);
    WindowState { bounds: Bounds { x, y, width, height }, maximized: state.maximized }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn b(x: f64, y: f64, width: f64, height: f64) -> Bounds {
        Bounds { x, y, width, height }
    }

    fn state(bounds: Bounds) -> WindowState {
        WindowState { bounds, maximized: false }
    }

    const MIN: (f64, f64) = (900.0, 600.0);
    /// A laptop screen and a larger one to its right.
    const SCREENS: [Bounds; 2] = [b(0.0, 0.0, 1920.0, 1040.0), b(1920.0, 0.0, 2560.0, 1400.0)];

    #[test]
    fn a_window_on_a_display_stays_where_it_was() {
        let s = state(b(2100.0, 100.0, 1280.0, 800.0));
        assert_eq!(fit(s, &SCREENS, MIN), s);
    }

    #[test]
    fn a_window_on_a_display_gone_comes_back_centered_on_the_primary() {
        let s = WindowState { bounds: b(5000.0, 200.0, 1280.0, 800.0), maximized: true };
        let fitted = fit(s, &SCREENS[..1], MIN);
        assert_eq!(fitted.bounds, b(320.0, 120.0, 1280.0, 800.0));
        assert!(fitted.maximized);
    }

    #[test]
    fn a_window_across_two_displays_lands_whole_on_the_one_it_covers_most() {
        // Mostly on the right one, its left edge on the laptop.
        let s = state(b(1800.0, 50.0, 1280.0, 800.0));
        assert_eq!(fit(s, &SCREENS, MIN).bounds, b(1920.0, 50.0, 1280.0, 800.0));
        // Hanging off the bottom of the laptop.
        let s = state(b(100.0, 700.0, 1280.0, 800.0));
        assert_eq!(fit(s, &SCREENS, MIN).bounds, b(100.0, 240.0, 1280.0, 800.0));
    }

    #[test]
    fn the_size_is_held_between_the_minimum_and_the_display() {
        let s = state(b(0.0, 0.0, 400.0, 300.0));
        assert_eq!(fit(s, &SCREENS, MIN).bounds, b(0.0, 0.0, 900.0, 600.0));
        let s = state(b(0.0, 0.0, 4000.0, 3000.0));
        assert_eq!(fit(s, &SCREENS[..1], MIN).bounds, b(0.0, 0.0, 1920.0, 1040.0));
        // A display under the minimum: the window takes the display's size.
        let tiny = [b(0.0, 0.0, 800.0, 500.0)];
        assert_eq!(fit(s, &tiny, MIN).bounds, b(0.0, 0.0, 800.0, 500.0));
    }

    #[test]
    fn no_displays_known_leaves_it_as_it_was() {
        let s = state(b(-3000.0, 0.0, 1.0, 1.0));
        assert_eq!(fit(s, &[], MIN), s);
    }

    #[test]
    fn what_is_saved_is_read_back_and_what_does_not_read_is_none() {
        let dir = std::env::temp_dir().join(format!("luna-window-state-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("window-state.json");
        let s = WindowState { bounds: b(10.5, 20.0, 1280.0, 800.0), maximized: true };
        save_to(&file, &s).unwrap();
        assert_eq!(load_from(&file), Some(s));
        std::fs::write(&file, "{\"bounds\":{\"x\":0,\"y\":0,\"width\":0,\"height\":5},\"maximized\":false}").unwrap();
        assert_eq!(load_from(&file), None);
        std::fs::write(&file, "not json").unwrap();
        assert_eq!(load_from(&file), None);
        assert_eq!(load_from(&dir.join("missing.json")), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
