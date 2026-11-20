//! Every control in reach on small screens. Each demo scene of the shots
//! (demo.rs; the light ones, since the theme changes no size) is opened over
//! its fake host, its popover, menu, panel or dialog opened as the shots open
//! it, and the window resized to each of `SIZES`; every element named with a
//! selector (selector.rs) that is drawn there must show whole, with some size
//! to click. Showing whole means inside the window and inside the content
//! mask it was painted under, which every clipping ancestor narrows: a button
//! cut off by its dialog's or panel's edge is caught though the dialog itself
//! is on screen. A control a scroller clips is in reach when wheeling that
//! scroller brings it into view, as a person would. The title bar's and the
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
use crate::selector::{Selector, clips};
use crate::title_bar::NARROW_BELOW;
use gpui::{
    Bounds, Context, InteractiveElement, IntoElement, ParentElement, Pixels, Point, Render, ScrollDelta, ScrollEvent, Size,
    StatefulInteractiveElement, Styled, TestAppContext, VisualTestContext, Window, div, point, px, size,
};
use std::collections::BTreeSet;
use std::sync::Mutex;

/// The window's size in logical pixels: the size it first opens at, its
/// minimum (lib.rs `SIZE`, `MIN_SIZE`) and 1000x700, then the laptop screens
/// the WebView window was measured on, at their display scaling: 1280x720 and
/// 1024x600, 1280x720 at 125% and 150%, and 1024x600 at 150%. The last three
/// are under the minimum: no window there is as big as it asks, so they are
/// checked as the screens they are. scripts/gpui-shots.ps1 -Fit shoots the
/// scenes at the same sizes.
pub const SIZES: [(f32, f32); 8] =
    [(1280., 800.), (1000., 700.), (900., 600.), (1280., 720.), (1024., 600.), (1024., 576.), (853., 480.), (683., 400.)];

/// How much of a control may not show on each side before it is cut off: a
/// pixel of rounding, and the border a control that clips its own children
/// takes off the mask its probe is painted under.
const SLACK: f32 = 1.5;

/// How far one wheel step scrolls, and how many a control may take.
const WHEEL: f32 = 40.;
const WHEELS: usize = 100;

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

/// The scenes that open a popup, and a selector of what must then be drawn,
/// or the check would measure the scene beneath it and pass.
const OPENS: &[(&str, &str)] = &[
    ("pane-menu", "menu-rename"),
    ("peek-menu", "menu-put-1"),
    ("title-menu", "title-library"),
    ("status-power-panel", "power-panel"),
    ("status-update-panel", "update-panel"),
    ("library-reach", "lib-reach-0"),
    ("library-folder-menu", "lib-folder-ins-0-0"),
    ("accounts-parent", "accounts-parent-claude-0"),
    ("dialog-new-chat-accounts", "newchat-account-0"),
    ("dialog-new-chat-models", "newchat-model-0"),
];

/// What is known to be out of reach, and why: a scene whose name starts with
/// `scene`, at `size`, a selector starting with `selector`. The test fails on
/// anything else out of reach, and on an entry that no longer matches, so the
/// list only shrinks as the window's own slices fix them.
struct Known {
    scene: &'static str,
    size: &'static str,
    selector: &'static str,
    why: &'static str,
}

const KNOWN: &[Known] = &[
    // The account rows under the question (or the sign-in window) the scene
    // opens, which takes every wheel while it is up, as it does a person's:
    // the same rows scroll into view in `accounts` at these sizes.
    Known { scene: "accounts-delete", size: "853x480", selector: "accounts-", why: "rows under the delete question" },
    Known { scene: "accounts-delete", size: "683x400", selector: "accounts-", why: "rows under the delete question" },
    Known { scene: "accounts-signin", size: "853x480", selector: "accounts-", why: "rows under the sign-in window" },
    Known { scene: "accounts-signin", size: "683x400", selector: "accounts-", why: "rows under the sign-in window" },
];

/// Whether `selector`, one of `ALWAYS`, must be drawn on this platform, in
/// the scene `scene` at `width`: on a narrow window Clear board and Auto-tile
/// are rows of the Menu's list (title_bar.rs `NARROW_BELOW`), drawn once it
/// is open.
fn required(selector: &str, scene: &str, width: f32) -> bool {
    let in_menu = matches!(selector, "clear-board" | "auto-tile") && px(width) < NARROW_BELOW && scene != "title-menu";
    !(cfg!(target_os = "macos") && selector.starts_with("window-")) && !in_menu
}

/// Where `a` and `b` meet: the part of `a` inside `b`, or, where they do not
/// overlap, the edge between them with no size, as gpui narrows a content mask.
fn meet(a: Bounds<Pixels>, b: Bounds<Pixels>) -> Bounds<Pixels> {
    let (left, top) = (a.left().max(b.left()), a.top().max(b.top()));
    let (right, bottom) = (a.right().min(b.right()).max(left), a.bottom().min(b.bottom()).max(top));
    Bounds::from_corners(point(left, top), point(right, bottom))
}

/// How far `b` reaches past each edge of `view`, left, top, right and
/// bottom: how much of it the view cuts off there.
fn cuts(b: Bounds<Pixels>, view: Bounds<Pixels>) -> [f32; 4] {
    [view.left() - b.left(), view.top() - b.top(), b.right() - view.right(), b.bottom() - view.bottom()].map(|c| f32::from(c).max(0.))
}

fn rect(b: Bounds<Pixels>) -> String {
    format!("{}x{} at ({}, {})", f32::from(b.size.width), f32::from(b.size.height), f32::from(b.origin.x), f32::from(b.origin.y))
}

/// What of the window a control painted under `mask` (the whole window when
/// its probe noted none) can show in.
fn view_of(mask: Option<Bounds<Pixels>>, size: Size<Pixels>) -> Bounds<Pixels> {
    let window = Bounds { origin: Point::default(), size };
    mask.map_or(window, |m| meet(m, window))
}

/// What is wrong with a control laid out at `b` and painted under `mask` in
/// a window of `size`, if anything: squeezed to nothing, past the window's
/// edge, or cut off by a frame that clips it.
fn misfit(b: Bounds<Pixels>, mask: Option<Bounds<Pixels>>, size: Size<Pixels>) -> Option<String> {
    if b.size.width < px(1.) || b.size.height < px(1.) {
        return Some(format!("squeezed to {}", rect(b)));
    }
    if cuts(b, view_of(None, size)).iter().any(|&c| c > SLACK) {
        return Some(format!("past the window's edge, {}", rect(b)));
    }
    let view = view_of(mask, size);
    cuts(b, view).iter().any(|&c| c > SLACK).then(|| {
        let shown = meet(b, view);
        let shown = if shown.size.width > px(0.) && shown.size.height > px(0.) { rect(shown) } else { "nothing".to_owned() };
        format!("cut off by the frame round it: {} shows {shown} (the frame shows {})", rect(b), rect(view))
    })
}

/// Wheels the scroller round `selector` toward it until it shows whole, as a
/// person scrolls to a control: one axis at a time, a step at a time, for as
/// long as each step brings it nearer.
fn scroll_to(cx: &mut VisualTestContext, selector: &'static str, size: Size<Pixels>) {
    // How far it lies past its view, all sides together, and where both are.
    let hidden = |cx: &mut VisualTestContext| {
        let b = cx.debug_bounds(selector)?;
        let view = view_of(clips::of(selector), size);
        Some((cuts(b, view).iter().sum::<f32>(), b, view))
    };
    for _ in 0..WHEELS {
        let Some((before, b, view)) = hidden(cx) else { return };
        let [left, top, right, bottom] = cuts(b, view);
        let step = |before: f32, after: f32| {
            if after > SLACK {
                -WHEEL
            } else if before > SLACK {
                WHEEL
            } else {
                0.
            }
        };
        let (dy, dx) = (step(top, bottom), step(left, right));
        let delta = if dy != 0. {
            point(px(0.), px(dy))
        } else if dx != 0. {
            point(px(dx), px(0.))
        } else {
            return;
        };
        // Over the scroller: the point of its view nearest the control, just
        // inside the edge the control lies past (a view with no size, of a
        // list scrolled wholly out of the scroller round it, is that
        // scroller's edge); or else, where something inside takes the wheel
        // for itself (an editor), the view's middle or a corner.
        let inset = px(2.);
        let along = |c: Pixels, lo: Pixels, hi: Pixels| {
            if c < lo + inset {
                lo + inset
            } else if c > hi - inset {
                hi - inset
            } else {
                c
            }
        };
        let nearest = point(along(b.center().x, view.left(), view.right()), along(b.center().y, view.top(), view.bottom()));
        let (x0, x1, y0, y1) = (view.left() + inset, view.right() - inset, view.top() + inset, view.bottom() - inset);
        let moved = [nearest, view.center(), point(x0, y0), point(x1, y0), point(x0, y1), point(x1, y1)].into_iter().any(|at| {
            cx.simulate_event(ScrollEvent { position: at, delta: ScrollDelta::Pixels(delta), ..Default::default() });
            cx.run_until_parked();
            hidden(cx).is_some_and(|(after, _, _)| after < before)
        });
        if !moved {
            return;
        }
    }
}

/// Every selector out of reach in the window of `cx` at `size`, as
/// `(selector, why)`: `candidates` each, and `ALWAYS` not drawn in the scene
/// named `scene`.
fn out_of_reach(cx: &mut VisualTestContext, scene: &str, candidates: &[&'static str], size: Size<Pixels>) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    for &selector in candidates {
        let Some(b) = cx.debug_bounds(selector) else {
            if ALWAYS.contains(&selector) && required(selector, scene, f32::from(size.width)) {
                out.push((selector, "not drawn".to_owned()));
            }
            continue;
        };
        if misfit(b, clips::of(selector), size).is_none() {
            continue;
        }
        scroll_to(cx, selector, size);
        let Some(b) = cx.debug_bounds(selector) else { continue };
        if let Some(why) = misfit(b, clips::of(selector), size) {
            out.push((selector, why));
        }
    }
    out
}

/// The selectors the probes have noted, and `ALWAYS`, in order, each kept
/// for the life of the program (`debug_bounds` takes its key so) and once.
fn candidates() -> Vec<&'static str> {
    static KEPT: Mutex<BTreeSet<&'static str>> = Mutex::new(BTreeSet::new());
    let mut kept = KEPT.lock().unwrap();
    let mut all: Vec<&'static str> = ALWAYS.to_vec();
    for s in clips::noted() {
        let s = match kept.get(s.as_str()) {
            Some(&k) => k,
            None => {
                let k: &'static str = Box::leak(s.into_boxed_str());
                kept.insert(k);
                k
            }
        };
        all.push(s);
    }
    all.sort();
    all.dedup();
    all
}

#[cfg(debug_assertions)]
#[gpui::test]
fn every_control_of_every_scene_is_in_reach_on_small_screens(cx: &mut TestAppContext) {
    // A sign-in terminal's parser has a thread of its own.
    cx.executor().allow_parking();
    let mut problems = Vec::new();
    let mut known_seen = Vec::new();
    let scenes: Vec<_> = demo::scenes().into_iter().filter(|s| s.theme == ThemePref::Light).collect();
    assert!(scenes.len() > 40, "the scenes are all there");
    for scene in &scenes {
        clips::forget();
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
        if let Some((_, selector)) = OPENS.iter().find(|(s, _)| *s == scene.name) {
            assert!(vcx.debug_bounds(selector).is_some(), "{}: {selector} is not drawn: the scene opens no popup", scene.name);
        }
        for (w, h) in SIZES {
            let at = size(px(w), px(h));
            vcx.simulate_resize(at);
            vcx.run_until_parked();
            let named = format!("{w}x{h}");
            // The status bar's "..." list holds the update's chip where the
            // window is narrow.
            if scene.name == "status-more" && px(w) < NARROW_BELOW && vcx.debug_bounds("status-update").is_none() {
                problems.push(format!("{} at {named}: the \"...\" list is not drawn", scene.name));
            }
            for (selector, why) in out_of_reach(vcx, scene.name, &candidates(), at) {
                let known =
                    KNOWN.iter().position(|k| scene.name.starts_with(k.scene) && k.size == named && selector.starts_with(k.selector));
                if let Some(i) = known {
                    known_seen.push(i);
                } else {
                    problems.push(format!("{} at {named}: {selector} is {why}", scene.name));
                }
            }
        }
        vcx.update(|window, _| window.remove_window());
        vcx.run_until_parked();
    }
    for (i, k) in KNOWN.iter().enumerate() {
        // On macOS the window's own buttons are not drawn to be out of reach.
        if !known_seen.contains(&i) && required(k.selector, k.scene, 1000.) {
            problems.push(format!("{}* at {}: {}* is in reach now ({}); take it off KNOWN", k.scene, k.size, k.selector, k.why));
        }
    }
    assert!(problems.is_empty(), "{} out of reach:\n{}", problems.len(), problems.join("\n"));
}

/// A frame with a Send button in it, for the check's own test: the frame on
/// screen, the button where `button_top` puts it, which the frame clips or
/// scrolls to.
struct Framed {
    button_top: f32,
    scrolls: bool,
}

impl Render for Framed {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let frame = div().id("frame").absolute().top(px(20.)).left(px(20.)).w(px(200.)).h(px(100.)).flex().flex_col();
        let frame = if self.scrolls { frame.overflow_y_scroll() } else { frame.overflow_hidden() };
        let send = div().id("send").selector(|| "fit-send".into()).flex_none().w(px(80.)).h(px(24.)).child("Send");
        div().size_full().child(frame.child(div().flex_none().h(px(self.button_top))).child(send))
    }
}

#[cfg(debug_assertions)]
#[gpui::test]
fn a_control_cut_off_inside_a_frame_on_screen_is_out_of_reach(cx: &mut TestAppContext) {
    clips::forget();
    let (view, cx) = cx.add_window_view(|_, _| Framed { button_top: 150., scrolls: false });
    let at = size(px(400.), px(300.));
    cx.simulate_resize(at);
    cx.run_until_parked();
    let send = cx.debug_bounds("fit-send").expect("the button is laid out");
    assert!(send.bottom() < at.height, "the button is inside the window: only its frame hides it");
    let found = out_of_reach(cx, "", &["fit-send"], at);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].1.starts_with("cut off by the frame"), "{found:?}");

    // Half of it past the frame's bottom edge is still cut off.
    view.update(cx, |v, cx| {
        v.button_top = 88.;
        cx.notify();
    });
    cx.run_until_parked();
    assert_eq!(out_of_reach(cx, "", &["fit-send"], at).len(), 1, "half a button");

    // Moved up into the frame, it fits.
    view.update(cx, |v, cx| {
        v.button_top = 40.;
        cx.notify();
    });
    cx.run_until_parked();
    assert_eq!(out_of_reach(cx, "", &["fit-send"], at), vec![]);

    // Below the fold of a frame that scrolls, the wheel brings it in.
    view.update(cx, |v, cx| {
        v.button_top = 150.;
        v.scrolls = true;
        cx.notify();
    });
    cx.run_until_parked();
    assert_eq!(out_of_reach(cx, "", &["fit-send"], at), vec![]);
    let send = cx.debug_bounds("fit-send").expect("still laid out");
    assert!(send.bottom() <= px(120. + SLACK), "scrolled into view: {}", rect(send));
}

#[test]
fn a_control_off_the_edge_squeezed_or_cut_off_is_out_of_reach() {
    let window = size(px(900.), px(600.));
    let at = |x: f32, y: f32, w: f32, h: f32| Bounds { origin: point(px(x), px(y)), size: size(px(w), px(h)) };
    assert_eq!(misfit(at(870., 4., 25., 18.), None, window), None);
    assert!(misfit(at(880., 4., 25., 18.), None, window).is_some(), "past the right edge");
    assert!(misfit(at(10., 590., 25., 18.), None, window).is_some(), "past the bottom");
    assert!(misfit(at(10., 10., 0., 18.), None, window).is_some(), "no width");
    // Inside a frame from (100, 100) to (300, 200).
    let frame = Some(at(100., 100., 200., 100.));
    assert_eq!(misfit(at(150., 150., 60., 20.), frame, window), None);
    assert_eq!(misfit(at(150., 181., 60., 20.), frame, window), None, "a pixel of rounding");
    assert!(misfit(at(150., 190., 60., 20.), frame, window).is_some(), "past the frame's bottom");
    assert!(misfit(at(150., 300., 60., 20.), frame, window).is_some(), "wholly hidden by it");
    assert!(misfit(at(280., 150., 60., 20.), frame, window).is_some(), "past its right edge");
}
