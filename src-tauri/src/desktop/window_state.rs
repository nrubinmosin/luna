//! The window's size, place and maximized state, kept from one run to the
//! next as Tauri's window-state plugin kept them, but in the data dir
//! (`window-state.json`) rather than under the identifier in the profile.
//! The file is the place's alone: the native window's chat list has its own
//! (native/state_file.rs), and a file here that holds one is never written
//! over before that list has moved (`save_to`).
//! What 0.19 saved there is not read: the first native start opens as a
//! fresh install does.
//!
//! The window saves its normal bounds (where it goes when not maximized),
//! whether it is maximized, and the display it is on by that display's
//! identity, with where on it; on the next start `fit` puts it back there on
//! that display wherever the displays have moved, and holds it to the
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowState {
    /// Where the window is when not maximized.
    pub bounds: Bounds,
    pub maximized: bool,
    /// The display it was on, where that display has an identity; a file
    /// from before there was one reads without it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<OnDisplay>,
}

/// The display a window is on, by its identity, and where on it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OnDisplay {
    pub id: String,
    /// The window's top left from the top left of the display's usable area.
    pub dx: f64,
    pub dy: f64,
}

/// A display there is now: its identity from one run to the next, where it
/// has one, and its usable area.
#[derive(Clone, Debug, PartialEq)]
pub struct Display {
    pub id: Option<String>,
    pub area: Bounds,
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
    let mut state: WindowState = serde_json::from_slice(&text).ok()?;
    let b = state.bounds;
    let sane = [b.x, b.y, b.width, b.height].iter().all(|v| v.is_finite()) && b.width > 0.0 && b.height > 0.0;
    // A place on a display that does not read is the place alone.
    if state.on.as_ref().is_some_and(|on| !on.dx.is_finite() || !on.dy.is_finite()) {
        state.on = None;
    }
    sane.then_some(state)
}

/// Writes `state` to `path`, unless what is there is a chat list no other
/// file has yet: builds before the native window's chats had a file of
/// their own kept them here, and the window moves them on its next read
/// (native/state_file.rs). Until then the place waits.
pub fn save_to(path: &Path, state: &WindowState) -> Result<(), String> {
    let data = path.parent().unwrap_or(Path::new("."));
    if !crate::native::chats_file(data).exists() && std::fs::read(path).is_ok_and(|b| crate::native::holds_chats(&b)) {
        return Err(format!("{} holds the chat list of an earlier build, not yet moved: the place is not written over it", path.display()));
    }
    let text = serde_json::to_vec_pretty(state).map_err(|e| e.to_string())?;
    crate::secret::write_private(path, &text).map_err(|e| format!("{}: {e}", path.display()))
}

/// `state` held to `displays` (the first the primary): no smaller than
/// `min`, no larger than the display it lands on, and wholly on it. It lands
/// on the display it was kept on (`WindowState::on`) where that is still
/// there, where it was on it, wherever the displays are now; else on the
/// display it overlaps most; else, overlapping none, centered on the
/// primary. The result names the display it landed on, where that has an
/// identity. No displays known: as it was.
pub fn fit(state: WindowState, displays: &[Display], min: (f64, f64)) -> WindowState {
    let Some(primary) = displays.first() else { return state };
    let b = state.bounds;
    let kept = state.on.as_ref().and_then(|on| displays.iter().find(|d| d.id.as_deref() == Some(on.id.as_str())).map(|d| (d, on)));
    let (display, at) = match kept {
        Some((d, on)) => (d, Some((d.area.x + on.dx, d.area.y + on.dy))),
        None => {
            let best = displays
                .iter()
                .map(|d| (d, b.overlap(&d.area)))
                .filter(|(_, o)| *o > 0.0)
                .max_by(|a, z| a.1.total_cmp(&z.1))
                .map(|(d, _)| d);
            match best {
                Some(d) => (d, Some((b.x, b.y))),
                None => (primary, None),
            }
        }
    };
    let area = display.area;
    // A display smaller than the minimum gets the window at its own size.
    let width = b.width.max(min.0).min(area.width);
    let height = b.height.max(min.1).min(area.height);
    let (x, y) = at.unwrap_or((area.x + (area.width - width) / 2.0, area.y + (area.height - height) / 2.0));
    let x = within(x, area.x, area.x + area.width - width);
    let y = within(y, area.y, area.y + area.height - height);
    let on = display.id.clone().map(|id| OnDisplay { id, dx: x - area.x, dy: y - area.y });
    WindowState { bounds: Bounds { x, y, width, height }, maximized: state.maximized, on }
}

/// `v` held to `lo..=hi`, `lo` winning where rounding put `hi` below it:
/// `(x + w) - w` can come out under `x` for a fractional origin, and
/// `f64::clamp` panics on a range the wrong way round (or with a NaN).
fn within(v: f64, lo: f64, hi: f64) -> f64 {
    v.min(hi).max(lo)
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn b(x: f64, y: f64, width: f64, height: f64) -> Bounds {
        Bounds { x, y, width, height }
    }

    fn state(bounds: Bounds) -> WindowState {
        WindowState { bounds, maximized: false, on: None }
    }

    /// Displays of these areas, none with an identity.
    fn plain(areas: &[Bounds]) -> Vec<Display> {
        areas.iter().map(|&area| Display { id: None, area }).collect()
    }

    const MIN: (f64, f64) = (900.0, 600.0);
    /// A laptop screen and a larger one to its right.
    const SCREENS: [Bounds; 2] = [b(0.0, 0.0, 1920.0, 1040.0), b(1920.0, 0.0, 2560.0, 1400.0)];

    #[test]
    fn a_window_on_a_display_stays_where_it_was() {
        let s = state(b(2100.0, 100.0, 1280.0, 800.0));
        assert_eq!(fit(s.clone(), &plain(&SCREENS), MIN), s);
    }

    #[test]
    fn a_window_on_a_display_gone_comes_back_centered_on_the_primary() {
        let s = WindowState { bounds: b(5000.0, 200.0, 1280.0, 800.0), maximized: true, on: None };
        let fitted = fit(s, &plain(&SCREENS[..1]), MIN);
        assert_eq!(fitted.bounds, b(320.0, 120.0, 1280.0, 800.0));
        assert!(fitted.maximized);
    }

    #[test]
    fn a_window_across_two_displays_lands_whole_on_the_one_it_covers_most() {
        // Mostly on the right one, its left edge on the laptop.
        let s = state(b(1800.0, 50.0, 1280.0, 800.0));
        assert_eq!(fit(s, &plain(&SCREENS), MIN).bounds, b(1920.0, 50.0, 1280.0, 800.0));
        // Hanging off the bottom of the laptop.
        let s = state(b(100.0, 700.0, 1280.0, 800.0));
        assert_eq!(fit(s, &plain(&SCREENS), MIN).bounds, b(100.0, 240.0, 1280.0, 800.0));
    }

    /// Kept on a monitor that is still there, the window goes back to its
    /// place on that monitor, which has moved to the laptop's left since;
    /// that monitor gone, the old rule holds, on what is left.
    #[test]
    fn a_window_goes_back_on_the_display_it_was_kept_on() {
        let on = |id: &str, dx: f64, dy: f64| Some(OnDisplay { id: id.into(), dx, dy });
        let s = WindowState { bounds: b(2020.0, 50.0, 1280.0, 800.0), maximized: false, on: on("monitor", 100.0, 50.0) };
        let moved = [
            Display { id: Some("laptop".into()), area: SCREENS[0] },
            Display { id: Some("monitor".into()), area: b(-2560.0, 0.0, 2560.0, 1400.0) },
        ];
        let fitted = fit(s.clone(), &moved, MIN);
        assert_eq!(fitted.bounds, b(-2460.0, 50.0, 1280.0, 800.0));
        assert_eq!(fitted.on, on("monitor", 100.0, 50.0));
        // Held to it as to any display: a place past its edge comes back on it.
        let far = WindowState { on: on("monitor", 2000.0, 50.0), ..s.clone() };
        assert_eq!(fit(far, &moved, MIN).bounds, b(-1280.0, 50.0, 1280.0, 800.0));
        // Gone: centered on the primary, which it is now on.
        let fitted = fit(s, &moved[..1], MIN);
        assert_eq!(fitted.bounds, b(320.0, 120.0, 1280.0, 800.0));
        assert_eq!(fitted.on, on("laptop", 320.0, 120.0));
    }

    #[test]
    fn the_size_is_held_between_the_minimum_and_the_display() {
        let s = state(b(0.0, 0.0, 400.0, 300.0));
        assert_eq!(fit(s, &plain(&SCREENS), MIN).bounds, b(0.0, 0.0, 900.0, 600.0));
        let s = state(b(0.0, 0.0, 4000.0, 3000.0));
        assert_eq!(fit(s.clone(), &plain(&SCREENS[..1]), MIN).bounds, b(0.0, 0.0, 1920.0, 1040.0));
        // A display under the minimum: the window takes the display's size.
        let tiny = plain(&[b(0.0, 0.0, 800.0, 500.0)]);
        assert_eq!(fit(s, &tiny, MIN).bounds, b(0.0, 0.0, 800.0, 500.0));
    }

    #[test]
    fn a_display_with_a_fractional_origin_takes_a_window_as_wide_as_it() {
        // (0.203 + 1279.7) - 1279.7 is 0.20299999999997453: the range the
        // window's left edge is held to comes out the wrong way round.
        let display = b(0.203, 0.5, 1279.7, 719.9);
        assert!((display.x + display.width) - display.width < display.x);
        let fitted = fit(state(b(-50.0, 0.0, 4000.0, 3000.0)), &plain(&[display]), MIN);
        assert_eq!(fitted.bounds, b(0.203, 0.5, 1279.7, 719.9));
        assert!((1.0..=2.0).contains(&within(f64::NAN, 1.0, 2.0)));
    }

    #[test]
    fn no_displays_known_leaves_it_as_it_was() {
        let s = state(b(-3000.0, 0.0, 1.0, 1.0));
        assert_eq!(fit(s.clone(), &[], MIN), s);
    }

    #[test]
    fn what_is_saved_is_read_back_and_what_does_not_read_is_none() {
        let dir = std::env::temp_dir().join(format!("luna-window-state-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("window-state.json");
        let s = WindowState { bounds: b(10.5, 20.0, 1280.0, 800.0), maximized: true, on: None };
        save_to(&file, &s).unwrap();
        assert_eq!(load_from(&file), Some(s));
        let s = WindowState { on: Some(OnDisplay { id: "monitor".into(), dx: 10.5, dy: 20.0 }), ..state(b(10.5, 20.0, 1280.0, 800.0)) };
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
