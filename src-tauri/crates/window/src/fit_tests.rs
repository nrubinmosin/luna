//! Every control in reach at the window's smallest size and two common ones:
//! the native counterpart of scripts/fit-check.ps1, which measures the
//! WebView. Each demo scene of the shots (demo.rs; the light ones, since the
//! theme changes no size) is opened over its fake host, its popover, menu,
//! panel or dialog opened as the shots open it, and the window resized to
//! each of `SIZES`; every element with a debug selector drawn there must lie
//! on screen with some size to click. The candidates are the selectors the
//! crate names, read off its sources (string literals shaped like one), and
//! those it makes with a number or a key (`made`). The title bar's and the
//! status bar's controls must be drawn in every scene.
//!
//! The test platform lays text out at 0.6 em a character, a little wider than
//! Cascadia Mono's 0.586, so what fits here fits on screen. Selectors are
//! recorded only in debug builds, so this runs in CI's Linux and macOS jobs
//! and not in the release-built Windows tests; scripts/gpui-shots.ps1 -Fit
//! runs it, and shoots the scenes at the same sizes to look at.

#![cfg_attr(not(debug_assertions), allow(dead_code, unused_imports))]

use crate::demo;
use crate::dto::ThemePref;
use crate::root::LunaWindow;
use gpui::{Bounds, Pixels, Size, TestAppContext, px, size};
use std::path::Path;

/// The window's minimum (lib.rs `MIN_SIZE`), and two sizes people run it at.
pub const SIZES: [(f32, f32); 3] = [(900., 600.), (1000., 700.), (1280., 800.)];

/// What a scene must draw whatever it shows: the title bar's and the status
/// bar's controls (`required`: on macOS the system's traffic lights take the
/// window's own buttons' place).
const ALWAYS: &[&str] = &[
    "window-min",
    "window-max",
    "window-close",
    "board-1",
    "board-2",
    "board-3",
    "board-4",
    "board-5",
    "layout-1",
    "layout-2",
    "layout-3",
    "layout-4",
    "clear-board",
    "auto-tile",
    "title-menu",
    "theme",
    "status-waiting",
    "status-background",
    "status-accounts",
    "status-power",
    "status-settings",
];

/// Whether `selector`, one of `ALWAYS`, must be drawn on this platform.
fn required(selector: &str) -> bool {
    !(cfg!(target_os = "macos") && selector.starts_with("window-"))
}

/// The selectors made with a number or a key, as their makers make them.
fn made() -> Vec<String> {
    let mut out = Vec::new();
    for n in 1..=5 {
        out.push(format!("board-{n}"));
    }
    for n in 1..=4 {
        out.push(format!("layout-{n}"));
        for part in ["", "-copy-id", "-menu", "-unseat"] {
            out.push(format!("pane-{n}{part}"));
        }
    }
    for part in ["-copy-id", "-menu"] {
        out.push(format!("peek{part}"));
    }
    for key in ["claude", "codex"] {
        out.push(format!("status-cli-{key}"));
    }
    for key in ["library", "accounts"] {
        out.push(format!("title-{key}"));
    }
    for key in ["rename", "copy-id", "unseat"] {
        out.push(format!("menu-{key}"));
    }
    for n in 1..=4 {
        for verb in ["move", "swap", "put"] {
            out.push(format!("menu-{verb}-{n}"));
        }
    }
    for secs in [60, 120, 300, 600, 1800] {
        out.push(format!("power-quiet-{secs}"));
    }
    for action in ["shutdown", "restart", "hibernate", "sleep"] {
        out.push(format!("power-action-{action}"));
    }
    out
}

/// The string literals in the crate's sources shaped like a selector:
/// lowercase words joined by dashes, and each with `-close` after it, as
/// `ui::frame` names its close button. Read where the crate was built: the
/// debug runs that use it run in the checkout (CI's Windows tests run
/// elsewhere, and are release builds that record no selectors).
fn named() -> Vec<String> {
    fn walk(dir: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).expect("the sources").flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).expect("a source");
                for (i, part) in text.split('"').enumerate() {
                    let shaped = i % 2 == 1
                        && part.contains('-')
                        && part.len() <= 40
                        && part.starts_with(|c: char| c.is_ascii_lowercase())
                        && !part.ends_with('-')
                        && part.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
                    if shaped {
                        out.push(part.to_owned());
                        out.push(format!("{part}-close"));
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut out);
    out
}

/// What is wrong with `b` in a window of `size`, if anything: off the
/// window's edge, or squeezed to nothing.
fn misfit(b: Bounds<Pixels>, size: Size<Pixels>) -> Option<String> {
    // Half a pixel of rounding either way.
    let slack = px(0.5);
    let off = b.origin.x < -slack || b.origin.y < -slack || b.right() > size.width + slack || b.bottom() > size.height + slack;
    let empty = b.size.width < px(1.) || b.size.height < px(1.);
    (off || empty).then(|| {
        format!("{}x{} at ({}, {})", f32::from(b.size.width), f32::from(b.size.height), f32::from(b.origin.x), f32::from(b.origin.y))
    })
}

#[cfg(debug_assertions)]
#[gpui::test]
fn every_control_of_every_scene_is_in_reach_at_the_smallest_size(cx: &mut TestAppContext) {
    // A sign-in terminal's parser has a thread of its own.
    cx.executor().allow_parking();
    let named = named();
    assert!(named.iter().any(|s| s == "clear-board"), "the literals are read");
    let mut candidates: Vec<String> = named.into_iter().chain(made()).collect();
    candidates.sort();
    candidates.dedup();
    // `debug_bounds` takes its key for the program's life.
    let candidates: Vec<&'static str> = candidates.into_iter().map(|s| &*Box::leak(s.into_boxed_str())).collect();
    let mut problems = Vec::new();
    let scenes: Vec<_> = demo::scenes().into_iter().filter(|s| s.theme == ThemePref::Light).collect();
    assert!(scenes.len() > 40, "the scenes are all there");
    for scene in &scenes {
        let (host, _mailbox, inbox) = scene.host();
        let models = cx.update(|cx| crate::start(host, inbox, scene.theme, cx));
        models.status.update(cx, |v, _| crate::status::demo::hold(v));
        cx.update(|cx| scene.tell(&models, cx));
        let window_models = models.clone();
        let (view, vcx) = cx.add_window_view(move |window, cx| LunaWindow::new(window_models, window, cx));
        vcx.run_until_parked();
        // What the shots open before they shoot (shots.rs `show`, `drive`).
        vcx.update(|_, cx| scene.tell_events(&models, cx));
        vcx.run_until_parked();
        if let Some(act) = scene.act {
            models.status.update(vcx, act);
        }
        if let Some(act) = scene.window_act {
            vcx.update(|window, cx| view.update(cx, |v, cx| act(v, window, cx)));
        }
        if let Some(at) = scene.menu {
            let board = view.read_with(vcx, |v, _| v.board.clone());
            board.update(vcx, |b, cx| {
                b.menu = Some(at);
                cx.notify();
            });
        }
        if let Some(id) = scene.click {
            models.workspace.update(vcx, |w, cx| w.show_chat(id, cx));
        }
        vcx.run_until_parked();
        for line in &scene.drive {
            let (verb, rest) = line.split_once(' ').unwrap_or((line, ""));
            let done =
                vcx.update(|window, cx| crate::panels::drive(verb, rest, window, cx) || crate::dialogs::drive(verb, rest, window, cx));
            assert!(done, "{}: {line} did nothing", scene.name);
            vcx.run_until_parked();
        }
        for (w, h) in SIZES {
            let at = size(px(w), px(h));
            vcx.simulate_resize(at);
            vcx.run_until_parked();
            for &selector in &candidates {
                match vcx.debug_bounds(selector) {
                    Some(b) => {
                        if let Some(why) = misfit(b, at) {
                            problems.push(format!("{} at {w}x{h}: {selector} is {why}", scene.name));
                        }
                    }
                    None if ALWAYS.contains(&selector) && required(selector) => {
                        problems.push(format!("{} at {w}x{h}: {selector} is not drawn", scene.name))
                    }
                    None => {}
                }
            }
        }
        vcx.update(|window, _| window.remove_window());
        vcx.run_until_parked();
    }
    assert!(problems.is_empty(), "{} out of reach:\n{}", problems.len(), problems.join("\n"));
}

#[test]
fn a_selector_off_the_edge_or_squeezed_to_nothing_is_out_of_reach() {
    let window = size(px(900.), px(600.));
    let at = |x: f32, y: f32, w: f32, h: f32| Bounds { origin: gpui::point(px(x), px(y)), size: size(px(w), px(h)) };
    assert_eq!(misfit(at(870., 4., 25., 18.), window), None);
    assert!(misfit(at(880., 4., 25., 18.), window).is_some(), "past the right edge");
    assert!(misfit(at(10., 590., 25., 18.), window).is_some(), "past the bottom");
    assert!(misfit(at(10., 10., 0., 18.), window).is_some(), "no width");
    assert!(made().contains(&"board-5".to_owned()));
}
