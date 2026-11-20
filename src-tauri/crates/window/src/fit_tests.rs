//! Every control in reach on small screens. Each demo scene of the shots
//! (demo.rs; the light ones, since the theme changes no size) is opened over
//! its fake host, its popover, menu, panel or dialog opened as the shots open
//! it, at each of `SIZES` afresh (nothing scrolled), and every element named
//! with a selector (selector.rs) that is drawn there must be in reach:
//!
//! - inside the window and inside the content mask it was painted under,
//!   which every clipping ancestor narrows, with some size: a button cut off
//!   by its dialog's or panel's edge is caught though the dialog itself is on
//!   screen. One a scroller clips is in reach when wheeling that scroller
//!   brings it whole into view, as a person would;
//! - reached by a click at its middle: a hit test there finds its hitbox, so
//!   nothing drawn over it that takes the mouse (`occlude`) is in the way.
//!
//! A scene that opens something names what it opens (`OPENS`), which must be
//! drawn at every size before and after the check, its frame fitting as it
//! opened, with nothing scrolled; a drop-down whose face scrolls out of view,
//! even in part, must close (`SCROLL_AWAY`). The title bar's and the status bar's controls
//! must be drawn in every scene. What is out of reach today is listed exactly
//! (`KNOWN`).
//!
//! The test platform lays text out at 0.6 em a character, a little wider than
//! Cascadia Mono's 0.586, so what fits here fits on screen; button labels stay
//! on one line (`whitespace_nowrap`), so a row too narrow for its buttons
//! shows as a button past its edge. Selectors are recorded only in debug
//! builds, so this runs in CI's Linux and macOS jobs and not in the
//! release-built Windows tests; scripts/gpui-shots.ps1 -Fit runs it, and
//! shoots the scenes at the same sizes to look at.

#![cfg_attr(not(debug_assertions), allow(dead_code, unused_imports))]

use crate::demo::{self, Scene};
use crate::dto::ThemePref;
use crate::root::LunaWindow;
use crate::selector::{Selector, clips};
use crate::title_bar::NARROW_BELOW;
use gpui::{
    Bounds, Context, Entity, InteractiveElement, IntoElement, ParentElement, Pixels, Point, Render, ScrollDelta, ScrollEvent, Size,
    StatefulInteractiveElement, Styled, TestAppContext, VisualTestContext, Window, div, point, px, size,
};
use std::collections::BTreeSet;
use std::sync::Mutex;

/// The window's size in logical pixels and the display's scaling: the size
/// it first opens at, the size it was checked at before, and its minimum
/// (lib.rs `SIZE`, `MIN_SIZE`); then the laptop screens the WebView window
/// was measured on: 1280x720 and 1024x600, 1280x720 at 125% and 150%, and
/// 1024x600 at 150%. The last three are under the minimum: no window there
/// is as big as it asks, so they are checked as the screens they are.
/// scripts/gpui-shots.ps1 -Fit shoots the scenes at the same sizes.
pub const SIZES: [(f32, f32, f32); 8] = [
    (1280., 800., 1.),
    (1000., 700., 1.),
    (900., 600., 1.),
    (1280., 720., 1.),
    (1024., 600., 1.),
    (1024., 576., 1.25),
    (853., 480., 1.5),
    (683., 400., 1.5),
];

/// A size's name, as the findings and `KNOWN` give it.
fn size_name((w, h, scale): (f32, f32, f32)) -> String {
    if scale == 1. { format!("{w}x{h}") } else { format!("{w}x{h}@{scale}") }
}

/// How much of a control may not show on each side before it is cut off: a
/// pixel of rounding, and the border a control that clips its own children
/// takes off the mask its probe is painted under. Some of it must show in
/// any case: a control narrower than this is not let off.
const SLACK: f32 = 1.5;

/// The longest a wheel step scrolls, and how many a control may take.
const WHEEL: f32 = 40.;
const WHEELS: usize = 400;

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

/// What a scene that opens something (an act, a menu, a click or driver
/// lines) must then draw, `shows`, or the check would measure the scene
/// beneath it and pass; and the popup's own frame, which must fit where it
/// opened, with nothing scrolled. A `narrow` one opens only on a window
/// narrower than `NARROW_BELOW`, where what it opens is; on a wider one the
/// scene is checked as one that opens nothing.
struct Opens {
    scene: &'static str,
    shows: &'static str,
    frame: Option<&'static str>,
    narrow: bool,
}

const fn opens(scene: &'static str, shows: &'static str) -> Opens {
    Opens { scene, shows, frame: None, narrow: false }
}

const fn popup(scene: &'static str, shows: &'static str, frame: &'static str) -> Opens {
    Opens { scene, shows, frame: Some(frame), narrow: false }
}

const fn narrow(o: Opens) -> Opens {
    Opens { narrow: true, ..o }
}

const OPENS: &[Opens] = &[
    popup("peek", "peek-close", "peek"),
    opens("pane-menu", "menu-rename"),
    opens("peek-menu", "menu-put-1"),
    popup("title-menu", "title-library", "title-menu-list"),
    opens("sidebar-rename", "rename-input"),
    popup("status-update-panel", "update-install", "update-panel"),
    popup("status-update-unsaved", "update-retry", "update-panel"),
    popup("status-power-panel", "power-disarm", "power-panel"),
    narrow(popup("status-more", "status-update", "status-more-list")),
    opens("orphan-ask", "orphans-kill"),
    popup("library", "library-close", "library"),
    popup("library-skill", "lib-save", "library"),
    popup("library-instructions", "lib-save", "library"),
    popup("library-reach", "lib-reach-0", "lib-reach-list"),
    popup("library-folder", "lib-folder-ins-0", "library"),
    popup("library-folder-menu", "lib-folder-ins-0-0", "lib-folder-ins-0-list"),
    popup("library-system", "lib-compare", "library"),
    popup("library-sync", "lib-sync", "library"),
    popup("settings", "settings-close", "settings"),
    popup("settings-chats", "settings-close", "settings"),
    popup("settings-agents", "settings-close", "settings"),
    popup("settings-clis", "settings-close", "settings"),
    popup("settings-updates", "settings-close", "settings"),
    popup("settings-update-confirm", "update-install", "update-panel"),
    popup("accounts", "accounts-add", "accounts"),
    popup("accounts-add", "accounts-create", "accounts"),
    popup("accounts-parent", "accounts-parent-claude-0", "accounts-parent-claude-list"),
    popup("accounts-rename", "accounts-rename-ok-0", "accounts"),
    popup("accounts-delete", "accounts-ask-yes", "accounts-ask"),
    popup("accounts-signin", "login-close", "login"),
    popup("dialog-new-chat", "newchat-create", "newchat"),
    popup("dialog-new-chat-codex", "newchat-create", "newchat"),
    popup("dialog-new-chat-accounts", "newchat-account-0", "newchat-account-list"),
    popup("dialog-new-chat-models", "newchat-model-0", "newchat-model-list"),
    popup("dialog-new-chat-untrusted", "newchat-trust", "newchat"),
    popup("dialog-new-chat-worktree", "newchat-worktree", "newchat"),
    popup("dialog-new-chat-no-git", "newchat-create", "newchat"),
    popup("dialog-delete", "delete-yes", "delete"),
    popup("dialog-delete-helpers", "delete-yes", "delete"),
    popup("dialog-helpers", "helper-h-review", "helpers"),
    popup("dialog-helpers-idle", "helper-h-done", "helpers"),
    popup("dialog-helpers-watch", "helpers-watch-status", "helpers-watch"),
    popup("dialog-helpers-many", "helper-h-many-13", "helpers"),
    popup("dialog-crash", "crash-reason", "crash"),
];

/// The scenes that act and open nothing: a quick chat makes its chat at once,
/// with no dialog.
const OPENS_NOTHING: &[&str] = &["dialog-quick-chat"];

/// Which controls a hit test must reach: all in a scene that opens nothing;
/// in one that opens a frame, those laid out inside it after it (its own,
/// or drawn over it), as what a popup or a modal covers is meant to be out
/// of reach while it is up; in one that opens a menu, what it must show.
#[derive(Clone, Copy)]
enum Reach {
    All,
    Within(&'static str),
    Just(&'static str),
}

impl Reach {
    fn of(opened: Option<&Opens>) -> Reach {
        match opened {
            None => Reach::All,
            Some(Opens { frame: Some(frame), .. }) => Reach::Within(frame),
            Some(o) => Reach::Just(o.shows),
        }
    }

    fn checks(self, cx: &mut VisualTestContext, selector: &str, b: Bounds<Pixels>) -> bool {
        match self {
            Reach::All => true,
            Reach::Just(s) => s == selector,
            Reach::Within(frame) => {
                let inside = cx.debug_bounds(frame).is_some_and(|f| f.contains(&b.center()));
                inside && clips::order(selector) > clips::order(frame)
            }
        }
    }
}

/// The drop-downs whose face scrolls with what is round it, as `(scene,
/// face, an item of its list)`: wheeled until the face is cut off by its
/// view, the list must be gone rather than hang over what is not there.
/// Each must be scrolled away at one size at least, or the check would pass
/// on nothing: the library's folder page fits at every size, so the face of
/// `library-folder-menu` never moves and it is not here.
const SCROLL_AWAY: &[(&str, &str, &str)] = &[
    ("dialog-new-chat-accounts", "newchat-account", "newchat-account-0"),
    ("dialog-new-chat-models", "newchat-model", "newchat-model-0"),
    ("library-reach", "lib-reach", "lib-reach-0"),
    ("accounts-parent", "accounts-parent-claude", "accounts-parent-claude-0"),
];

/// How a control is out of reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// Past the window's edge, with no scrolling to it.
    Off,
    /// Cut off by a frame that clips it, with no scrolling to it.
    Cut,
    /// Laid out with no width or no height.
    Squeezed,
    /// Something drawn over its middle takes the click.
    Covered,
    /// One of `ALWAYS` not drawn.
    NotDrawn,
    /// Gone once something was scrolled to bring it into view.
    Vanished,
    /// What the scene opens (`OPENS`) not drawn, or gone during the check.
    NotOpen,
    /// The popup's frame does not fit where it opened.
    PopupCut,
    /// A drop-down's list still drawn after its face scrolled out of view, in
    /// part or whole.
    StaysOpen,
}

/// The scenes a `Known` entry is for.
enum Scenes {
    /// Every light scene, for what the title bar or the status bar leaves
    /// out of reach at a size.
    #[allow(dead_code, reason = "no entry is out of reach in every scene today")]
    Every,
    Only(&'static [&'static str]),
}

/// What is out of reach today, and why: each selector in `selectors` in each
/// scene of `scenes`, at `size`, out of reach as `kind`. The test fails on
/// anything else out of reach, and on any of these that is in reach, so the
/// list only shrinks as the window's own slices fix them.
struct Known {
    scenes: Scenes,
    size: &'static str,
    selectors: &'static [&'static str],
    kind: Kind,
    why: &'static str,
}

const KNOWN: &[Known] = &[
    // The account rows under the question (or the sign-in window) the scene
    // opens, which takes every wheel while it is up, as it does a person's:
    // the same rows scroll into view in `accounts` at these sizes.
    Known {
        scenes: Scenes::Only(&["accounts-delete", "accounts-signin"]),
        size: "683x400@1.5",
        selectors: &[
            "accounts-agents-1",
            "accounts-agents-2",
            "accounts-delete-1",
            "accounts-delete-2",
            "accounts-refresh-1",
            "accounts-refresh-2",
            "accounts-rename-1",
            "accounts-rename-2",
            "accounts-signin-2",
        ],
        kind: Kind::Off,
        why: "rows under the question or the sign-in window",
    },
    Known {
        scenes: Scenes::Only(&["accounts-delete", "accounts-signin"]),
        size: "853x480@1.5",
        selectors: &["accounts-agents-2", "accounts-delete-2", "accounts-refresh-2", "accounts-rename-2", "accounts-signin-2"],
        kind: Kind::Off,
        why: "rows under the question or the sign-in window",
    },
    Known {
        scenes: Scenes::Only(&["dialog-helpers-many"]),
        size: "683x400@1.5",
        selectors: &[
            "helper-h-many-12",
            "helper-delete-h-many-12",
            "helper-open-h-many-12",
            "helper-stop-h-many-12",
            "helper-watch-h-many-12",
            "helper-h-many-13",
            "helper-delete-h-many-13",
            "helper-open-h-many-13",
            "helper-stop-h-many-13",
            "helper-watch-h-many-13",
        ],
        kind: Kind::Off,
        why: "the helpers' list is up to 420px tall whatever the window: its last rows go below it, with nothing to scroll",
    },
    Known {
        scenes: Scenes::Only(&["dialog-helpers-many"]),
        size: "853x480@1.5",
        selectors: &[
            "helper-h-many-13",
            "helper-delete-h-many-13",
            "helper-open-h-many-13",
            "helper-stop-h-many-13",
            "helper-watch-h-many-13",
        ],
        kind: Kind::Off,
        why: "as at 683x400, the last row",
    },
    // A drop-down's list hangs where its face was laid out, drawn over the
    // rest (deferred), and closes only on a click outside it: wheeled out of
    // view, the face leaves its list behind.
    Known {
        scenes: Scenes::Only(&["accounts-parent"]),
        size: "853x480@1.5",
        selectors: &["accounts-parent-claude-0"],
        kind: Kind::StaysOpen,
        why: "the parent's list stays with its face scrolled away",
    },
    Known {
        scenes: Scenes::Only(&["accounts-parent"]),
        size: "683x400@1.5",
        selectors: &["accounts-parent-claude-0"],
        kind: Kind::StaysOpen,
        why: "the parent's list stays with its face scrolled away",
    },
    Known {
        scenes: Scenes::Only(&["library-reach"]),
        size: "853x480@1.5",
        selectors: &["lib-reach-0"],
        kind: Kind::StaysOpen,
        why: "the reach list stays with its face scrolled away",
    },
    Known {
        scenes: Scenes::Only(&["library-reach"]),
        size: "683x400@1.5",
        selectors: &["lib-reach-0"],
        kind: Kind::StaysOpen,
        why: "the reach list stays with its face scrolled away",
    },
    Known {
        scenes: Scenes::Only(&["dialog-new-chat-accounts"]),
        size: "683x400@1.5",
        selectors: &["newchat-account-0"],
        kind: Kind::StaysOpen,
        why: "the account list stays with its face scrolled away",
    },
    Known {
        scenes: Scenes::Only(&["dialog-new-chat-models"]),
        size: "683x400@1.5",
        selectors: &["newchat-model-0"],
        kind: Kind::StaysOpen,
        why: "the model list stays with its face scrolled away",
    },
];

/// Whether `selector`, one of `ALWAYS`, must be drawn on this platform.
fn required(selector: &str) -> bool {
    !(cfg!(target_os = "macos") && selector.starts_with("window-"))
}

/// Whether `selector`, one of `ALWAYS`, is a row of the title bar's Menu in
/// the scene `scene` at `width`, drawn only where the Menu is open: on a
/// narrow window Clear board and Auto-tile leave the bar for it (title_bar.rs
/// `NARROW_BELOW`).
fn in_menu(selector: &str, scene: &str, width: Pixels) -> bool {
    matches!(selector, "clear-board" | "auto-tile") && width < NARROW_BELOW && scene != "title-menu"
}

/// Where `a` and `b` meet: the part of `a` inside `b`, or, where they do not
/// overlap, the edge between them with no size, as gpui narrows a content mask.
fn meet(a: Bounds<Pixels>, b: Bounds<Pixels>) -> Bounds<Pixels> {
    let (left, top) = (a.left().max(b.left()), a.top().max(b.top()));
    let (right, bottom) = (a.right().min(b.right()).max(left), a.bottom().min(b.bottom()).max(top));
    Bounds::from_corners(point(left, top), point(right, bottom))
}

fn empty(b: Bounds<Pixels>) -> bool {
    b.size.width <= px(0.) || b.size.height <= px(0.)
}

/// How far `b` reaches past each edge of `view`, left, top, right and
/// bottom: how much of it the view cuts off there.
fn cuts(b: Bounds<Pixels>, view: Bounds<Pixels>) -> [f32; 4] {
    [view.left() - b.left(), view.top() - b.top(), b.right() - view.right(), b.bottom() - view.bottom()].map(|c| f32::from(c).max(0.))
}

/// Whether `view` cuts `b` off: more than the slack on a side, or all of it.
fn cut_off(b: Bounds<Pixels>, view: Bounds<Pixels>) -> bool {
    empty(meet(b, view)) || cuts(b, view).iter().any(|&c| c > SLACK)
}

fn rect(b: Bounds<Pixels>) -> String {
    format!("{}x{} at ({}, {})", f32::from(b.size.width), f32::from(b.size.height), f32::from(b.origin.x), f32::from(b.origin.y))
}

fn window_of(size: Size<Pixels>) -> Bounds<Pixels> {
    Bounds { origin: Point::default(), size }
}

/// What of the window a control painted under `mask` (the whole window when
/// its probe noted none) can show in.
fn view_of(mask: Option<Bounds<Pixels>>, size: Size<Pixels>) -> Bounds<Pixels> {
    mask.map_or(window_of(size), |m| meet(m, window_of(size)))
}

/// What is wrong with a control laid out at `b` and painted under `mask` in
/// a window of `size`, if anything: squeezed to nothing, past the window's
/// edge, or cut off by a frame that clips it.
fn misfit(b: Bounds<Pixels>, mask: Option<Bounds<Pixels>>, size: Size<Pixels>) -> Option<(Kind, String)> {
    if b.size.width < px(1.) || b.size.height < px(1.) {
        return Some((Kind::Squeezed, format!("squeezed to {}", rect(b))));
    }
    if cut_off(b, window_of(size)) {
        return Some((Kind::Off, format!("past the window's edge, {}", rect(b))));
    }
    let view = view_of(mask, size);
    cut_off(b, view).then(|| {
        let shown = meet(b, view);
        let shown = if empty(shown) { "nothing".to_owned() } else { rect(shown) };
        (Kind::Cut, format!("cut off by the frame round it: {} shows {shown} (the frame shows {})", rect(b), rect(view)))
    })
}

/// Whether a click at the middle of `selector`'s part in view would not reach
/// it: a hit test there does not find its probe's hitbox.
fn covered(cx: &mut VisualTestContext, selector: &str, b: Bounds<Pixels>, size: Size<Pixels>) -> bool {
    let Some(hitbox) = clips::hitbox(selector) else { return false };
    let at = meet(b, view_of(clips::of(selector), size)).center();
    !cx.update(|window, _| hitbox.is_hovered_at(at, window))
}

/// Wheels `delta` at each of `points` in turn until a step makes `better`
/// true: false when none did.
fn wheel(
    cx: &mut VisualTestContext,
    points: &[Point<Pixels>],
    delta: Point<Pixels>,
    mut better: impl FnMut(&mut VisualTestContext) -> bool,
) -> bool {
    points.iter().any(|&at| {
        cx.simulate_event(ScrollEvent { position: at, delta: ScrollDelta::Pixels(delta), ..Default::default() });
        cx.run_until_parked();
        better(cx)
    })
}

/// Where to wheel for a control at `b` seen through `view`: the point of the
/// view nearest the control, just inside the edge the control lies past (a
/// view with no size, of a list scrolled wholly out of the scroller round it,
/// is that scroller's edge); or else, where something inside takes the wheel
/// for itself (an editor), the view's middle or a corner.
fn wheel_points(b: Bounds<Pixels>, view: Bounds<Pixels>) -> [Point<Pixels>; 6] {
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
    [nearest, view.center(), point(x0, y0), point(x1, y0), point(x0, y1), point(x1, y1)]
}

/// Wheels the scroller round `selector` toward it until it shows whole, as a
/// person scrolls to a control: one axis at a time, for as long as each step
/// brings it nearer. A step is never longer than the room the view has
/// beside the control, so no position where it fits is stepped over.
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
        let step = |before: f32, after: f32, room: Pixels| {
            let room = f32::from(room).clamp(1., WHEEL);
            if after > SLACK {
                -room
            } else if before > SLACK {
                room
            } else {
                0.
            }
        };
        let dy = step(top, bottom, view.size.height - b.size.height);
        let dx = step(left, right, view.size.width - b.size.width);
        let delta = if dy != 0. {
            point(px(0.), px(dy))
        } else if dx != 0. {
            point(px(dx), px(0.))
        } else {
            return;
        };
        if !wheel(cx, &wheel_points(b, view), delta, |cx| hidden(cx).is_some_and(|(after, _, _)| after < before)) {
            return;
        }
    }
}

/// A selector out of reach: how, and what was seen.
#[derive(Debug, PartialEq)]
struct Finding {
    selector: &'static str,
    kind: Kind,
    why: String,
}

/// Every selector out of reach in the window of `cx` at `size`, the scene
/// named `scene`: `candidates` each, and `ALWAYS` not drawn; covered, those
/// `reach` checks.
fn out_of_reach(cx: &mut VisualTestContext, scene: &str, candidates: &[&'static str], size: Size<Pixels>, reach: Reach) -> Vec<Finding> {
    let mut out = Vec::new();
    for &selector in candidates {
        let Some(mut b) = cx.debug_bounds(selector) else {
            if ALWAYS.contains(&selector) && required(selector) && !in_menu(selector, scene, size.width) {
                out.push(Finding { selector, kind: Kind::NotDrawn, why: "not drawn".into() });
            }
            continue;
        };
        let mut wrong = misfit(b, clips::of(selector), size);
        if wrong.is_some() {
            scroll_to(cx, selector, size);
            let Some(after) = cx.debug_bounds(selector) else {
                out.push(Finding { selector, kind: Kind::Vanished, why: "gone once scrolled to".into() });
                continue;
            };
            b = after;
            wrong = misfit(b, clips::of(selector), size);
        }
        match wrong {
            Some((kind, why)) => out.push(Finding { selector, kind, why }),
            None if reach.checks(cx, selector, b) && covered(cx, selector, b, size) => {
                out.push(Finding { selector, kind: Kind::Covered, why: format!("covered at its middle, {}", rect(b)) })
            }
            None => {}
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

/// Opens `scene` as the shots open it (shots.rs `show`, `drive`), at `at`.
fn open<'a>(cx: &'a mut TestAppContext, scene: &Scene, at: (f32, f32, f32)) -> (Entity<LunaWindow>, &'a mut VisualTestContext) {
    clips::forget();
    let (host, _mailbox, inbox) = scene.host();
    let models = cx.update(|cx| crate::start(host, inbox, scene.theme, cx));
    models.status.update(cx, |v, _| crate::status::demo::hold(v));
    cx.update(|cx| scene.tell(&models, cx));
    let window_models = models.clone();
    let (view, vcx) = cx.add_window_view(move |window, cx| LunaWindow::new(window_models, window, cx));
    vcx.simulate_scale_factor_change(at.2);
    vcx.simulate_resize(size(px(at.0), px(at.1)));
    vcx.run_until_parked();
    vcx.update(|_, cx| scene.tell_events(&models, cx));
    vcx.run_until_parked();
    if let Some(act) = scene.act {
        models.status.update(vcx, act);
        // What the act waits on in time (an install's steps) comes about as
        // in the shots, which give it real seconds.
        vcx.run_until_parked();
        vcx.executor().advance_clock(std::time::Duration::from_secs(5));
    }
    if let Some(act) = scene.window_act {
        vcx.update(|window, cx| view.update(cx, |v, cx| act(v, window, cx)));
    }
    if let Some(menu) = scene.menu {
        let board = view.read_with(vcx, |v, _| v.board.clone());
        board.update(vcx, |b, cx| {
            b.menu = Some(menu);
            cx.notify();
        });
    }
    if let Some(id) = scene.click {
        models.workspace.update(vcx, |w, cx| w.show_chat(id, cx));
    }
    vcx.run_until_parked();
    for line in &scene.drive {
        let (verb, rest) = line.split_once(' ').unwrap_or((line, ""));
        let done = vcx.update(|window, cx| crate::panels::drive(verb, rest, window, cx) || crate::dialogs::drive(verb, rest, window, cx));
        assert!(done, "{}: {line} did nothing", scene.name);
        vcx.run_until_parked();
    }
    (view, vcx)
}

/// Wheels what is round the drop-down's `face` until the face no longer
/// shows whole, either way: None when it never does, else whether its list's
/// `item` is still drawn then.
fn scroll_away(cx: &mut VisualTestContext, face: &'static str, item: &'static str, size: Size<Pixels>) -> Option<bool> {
    for dy in [-WHEEL, WHEEL] {
        for _ in 0..WHEELS {
            let b = cx.debug_bounds(face)?;
            let view = view_of(clips::of(face), size);
            let shown = meet(b, view);
            if cut_off(b, view) {
                return Some(cx.debug_bounds(item).is_some());
            }
            // Over the scroller round the face, wherever the open list does
            // not cover it.
            if !wheel(cx, &wheel_points(shown, view), point(px(0.), px(dy)), |cx| cx.debug_bounds(face) != Some(b)) {
                break;
            }
        }
    }
    None
}

#[cfg(debug_assertions)]
#[gpui::test]
fn every_control_of_every_scene_is_in_reach_on_small_screens(cx: &mut TestAppContext) {
    // A sign-in terminal's parser has a thread of its own.
    cx.executor().allow_parking();
    let mut problems = Vec::new();
    let mut known_seen = BTreeSet::new();
    let mut scrolled_away = BTreeSet::new();
    let scenes: Vec<_> = demo::scenes().into_iter().filter(|s| s.theme == ThemePref::Light).collect();
    assert!(scenes.len() > 40, "the scenes are all there");
    for scene in &scenes {
        let acts =
            scene.act.is_some() || scene.window_act.is_some() || scene.menu.is_some() || scene.click.is_some() || !scene.drive.is_empty();
        let opened = OPENS.iter().find(|o| o.scene == scene.name);
        if acts && opened.is_none() && !OPENS_NOTHING.contains(&scene.name) {
            problems.push(format!("{}: opens something but OPENS names nothing it must show", scene.name));
        }
        for at in SIZES {
            let named = size_name(at);
            let wsize = size(px(at.0), px(at.1));
            let opened = opened.filter(|o| !o.narrow || wsize.width < NARROW_BELOW);
            let (_view, vcx) = open(cx, scene, at);
            let mut found = Vec::new();
            if let Some(o) = opened {
                if vcx.debug_bounds(o.shows).is_none() {
                    found.push(Finding { selector: o.shows, kind: Kind::NotOpen, why: "not drawn: the scene opened nothing".into() });
                }
                if let Some(frame) = o.frame {
                    match vcx.debug_bounds(frame) {
                        None => found.push(Finding { selector: frame, kind: Kind::NotOpen, why: "its frame is not drawn".into() }),
                        Some(b) => {
                            if let Some((_, why)) = misfit(b, clips::of(frame), wsize) {
                                found.push(Finding {
                                    selector: frame,
                                    kind: Kind::PopupCut,
                                    why: format!("does not fit where it opened: {why}"),
                                });
                            }
                        }
                    }
                }
            }
            // The frame is measured above, as it opened.
            let frame = opened.and_then(|o| o.frame);
            let candidates: Vec<_> = candidates().into_iter().filter(|s| Some(*s) != frame).collect();
            found.extend(out_of_reach(vcx, scene.name, &candidates, wsize, Reach::of(opened)));
            if let Some(o) = opened
                && !found.iter().any(|f| f.selector == o.shows && f.kind == Kind::NotOpen)
                && vcx.debug_bounds(o.shows).is_none()
            {
                found.push(Finding { selector: o.shows, kind: Kind::NotOpen, why: "closed during the check".into() });
            }
            for &(_, face, item) in SCROLL_AWAY.iter().filter(|(s, _, _)| *s == scene.name) {
                match scroll_away(vcx, face, item, wsize) {
                    None => {}
                    Some(stays) => {
                        scrolled_away.insert(face);
                        if stays {
                            found.push(Finding {
                                selector: item,
                                kind: Kind::StaysOpen,
                                why: format!("still drawn with {face} out of view"),
                            });
                        }
                    }
                }
            }
            for f in found {
                let known = KNOWN.iter().position(|k| {
                    k.size == named
                        && k.kind == f.kind
                        && k.selectors.contains(&f.selector)
                        && match k.scenes {
                            Scenes::Every => true,
                            Scenes::Only(names) => names.contains(&scene.name),
                        }
                });
                match known {
                    Some(i) => {
                        known_seen.insert((i, scene.name, f.selector));
                    }
                    None => problems.push(format!("{} at {named}: {} is {:?}: {}", scene.name, f.selector, f.kind, f.why)),
                }
            }
            vcx.update(|window, _| window.remove_window());
            vcx.run_until_parked();
        }
    }
    for (i, k) in KNOWN.iter().enumerate() {
        let names: Vec<&str> = match k.scenes {
            Scenes::Every => scenes.iter().map(|s| s.name).collect(),
            Scenes::Only(names) => names.to_vec(),
        };
        for &scene in &names {
            // On macOS the window's own buttons are not drawn to be out of reach.
            for &selector in k.selectors.iter().filter(|s| required(s)) {
                if !known_seen.contains(&(i, scene, selector)) {
                    problems.push(format!("{scene} at {}: {selector} is no longer {:?} ({}); take it off KNOWN", k.size, k.kind, k.why));
                }
            }
        }
    }
    for &(scene, face, _) in SCROLL_AWAY {
        if !scrolled_away.contains(face) {
            problems.push(format!("{scene}: {face} never scrolled out of view at any size, so its list's closing was not checked"));
        }
    }
    assert!(problems.is_empty(), "{} out of reach:\n{}", problems.len(), problems.join("\n"));
}

/// A frame with a Send button in it, for the check's own test: the frame on
/// screen and `frame_h` tall, the button `button_w` wide where `button_top`
/// puts it, which the frame clips or scrolls to; with `cover`, a box that
/// takes the mouse is drawn over the button.
struct Framed {
    frame_h: f32,
    button_top: f32,
    button_w: f32,
    scrolls: bool,
    cover: bool,
}

impl Default for Framed {
    fn default() -> Self {
        Framed { frame_h: 100., button_top: 150., button_w: 80., scrolls: false, cover: false }
    }
}

impl Render for Framed {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let frame = div().id("frame").absolute().top(px(20.)).left(px(20.)).w(px(200.)).h(px(self.frame_h)).flex().flex_col();
        let frame = if self.scrolls { frame.overflow_y_scroll() } else { frame.overflow_hidden() };
        let send = div().id("send").selector(|| "fit-send".into()).flex_none().w(px(self.button_w)).h(px(20.)).child("Send");
        let mut root = div().size_full().child(frame.child(div().flex_none().h(px(self.button_top))).child(send));
        if self.cover {
            root = root.child(div().id("cover").occlude().absolute().top_0().left_0().w(px(300.)).h(px(300.)));
        }
        root
    }
}

#[cfg(debug_assertions)]
#[gpui::test]
fn a_control_cut_off_inside_a_frame_or_covered_is_out_of_reach(cx: &mut TestAppContext) {
    clips::forget();
    let (view, cx) = cx.add_window_view(|_, _| Framed::default());
    let at = size(px(400.), px(300.));
    cx.simulate_resize(at);
    cx.run_until_parked();
    let set = |cx: &mut VisualTestContext, f: fn(&mut Framed)| {
        view.update(cx, |v, cx| {
            *v = Framed::default();
            f(v);
            cx.notify();
        });
        cx.run_until_parked();
        out_of_reach(cx, "", &["fit-send"], at, Reach::All).into_iter().map(|f| f.kind).collect::<Vec<_>>()
    };
    let send = cx.debug_bounds("fit-send").expect("the button is laid out");
    assert!(send.bottom() < at.height, "the button is inside the window: only its frame hides it");
    assert_eq!(set(cx, |_| {}), [Kind::Cut], "wholly hidden by the frame");
    assert_eq!(set(cx, |v| v.button_top = 90.), [Kind::Cut], "half past the frame's bottom edge");
    assert_eq!(set(cx, |v| v.button_w = 1.), [Kind::Cut], "a pixel wide is not let off by the slack");
    assert_eq!(set(cx, |v| v.button_top = 40.), [], "moved up into the frame, it fits");
    assert_eq!(set(cx, |v| v.scrolls = true), [], "below the fold of a frame that scrolls, the wheel brings it in");
    let send = cx.debug_bounds("fit-send").expect("still laid out");
    assert!(send.bottom() <= px(120. + SLACK), "scrolled into view: {}", rect(send));
    // A 30px view, the button 20px tall at 25: in view only for scroll
    // offsets 15 to 25, which a 40px step would pass over.
    assert_eq!(
        set(cx, |v| {
            v.frame_h = 30.;
            v.button_top = 25.;
            v.scrolls = true;
        }),
        [],
        "a step no longer than the room beside it"
    );
    assert_eq!(
        set(cx, |v| {
            v.button_top = 40.;
            v.cover = true;
        }),
        [Kind::Covered],
        "in view, under something that takes the mouse"
    );
}

#[test]
fn a_control_off_the_edge_squeezed_or_cut_off_is_out_of_reach() {
    let window = size(px(900.), px(600.));
    let at = |x: f32, y: f32, w: f32, h: f32| Bounds { origin: point(px(x), px(y)), size: size(px(w), px(h)) };
    let kind = |b, mask| misfit(b, mask, window).map(|(k, _)| k);
    assert_eq!(kind(at(870., 4., 25., 18.), None), None);
    assert_eq!(kind(at(880., 4., 25., 18.), None), Some(Kind::Off), "past the right edge");
    assert_eq!(kind(at(10., 590., 25., 18.), None), Some(Kind::Off), "past the bottom");
    assert_eq!(kind(at(900.2, 10., 1., 18.), None), Some(Kind::Off), "a pixel wide, just past the edge");
    assert_eq!(kind(at(10., 10., 0., 18.), None), Some(Kind::Squeezed), "no width");
    // Inside a frame from (100, 100) to (300, 200).
    let frame = Some(at(100., 100., 200., 100.));
    assert_eq!(kind(at(150., 150., 60., 20.), frame), None);
    assert_eq!(kind(at(150., 181., 60., 20.), frame), None, "a pixel of rounding");
    assert_eq!(kind(at(150., 190., 60., 20.), frame), Some(Kind::Cut), "past the frame's bottom");
    assert_eq!(kind(at(150., 300., 60., 20.), frame), Some(Kind::Cut), "wholly hidden by it");
    assert_eq!(kind(at(280., 150., 60., 20.), frame), Some(Kind::Cut), "past its right edge");
    assert_eq!(kind(at(300.5, 150., 1., 20.), frame), Some(Kind::Cut), "a pixel wide, just outside it");
    assert_eq!(size_name((853., 480., 1.5)), "853x480@1.5");
}
