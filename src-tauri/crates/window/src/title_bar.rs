//! The window's own title bar: Luna's name, the board's controls (the five
//! boards, the four layouts, Clear board, Auto-tile), the buttons that open
//! the library and the accounts windows (panels/, in one Menu: Settings is
//! the status bar's), the theme button and
//! the window's buttons, on the title blue, the rest of it a handle to move
//! the window by.
//!
//! On Windows the parts are hit-test areas, so the
//! system moves, maximizes on a double-click, minimizes and closes as it does
//! for any title bar. The bar is a cached view, and gpui keeps a window's
//! control areas only from a frame that painted them (its `reuse_paint`
//! carries hitboxes over but not `window_control_hitboxes`): a frame drawn for
//! a terminal alone left the hit-test with none, so the buttons answered as
//! the client area, and a first click only moved the keyboard off the
//! terminal. So the bar records where its parts were laid out (`Controls`),
//! and root.rs puts `ControlAreas` after it, uncached, to make them control
//! areas in every frame. Elsewhere gpui has no such
//! hit-test: the drag area starts the move itself and a double-click zooms,
//! and the buttons call the window's own. On macOS the system's traffic
//! lights stay, in a title bar made transparent over this one, and take the
//! buttons' place.

use crate::dto::ThemePref;
use crate::models::{Models, Shell};
use crate::panels::{self, Panel};
use crate::scale::{self, u};
use crate::selector::Selector;
use crate::state::{BOARDS, board_label};
use crate::theme::Palette;
use crate::widgets::{Tipped, button, tips_on, toggle};
use gpui::prelude::FluentBuilder;
use gpui::{
    App, Bounds, Context, Element, ElementId, FontWeight, GlobalElementId, Hitbox, HitboxBehavior, InspectorElementId, InteractiveElement,
    IntoElement, LayoutId, MouseButton, MouseDownEvent, ParentElement, Pixels, Position, Render, StatefulInteractiveElement, Style, Styled,
    Window, WindowControlArea, canvas, deferred, div,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Its height at the window's scale: the view is cached at this size
/// (root.rs).
pub fn height() -> Pixels {
    u(28.)
}

/// The window's width below which the bar is compact: the captions "Board"
/// and "Panes" go, and Auto-tile and the theme say their state by the
/// button's face and one word, so that every button stays in reach at the
/// window's minimum width (lib.rs `MIN_SIZE`). The full bar takes about
/// 930 px in Cascadia Mono, the compact one about 810. Widths here are at
/// 100%, against the window's width at its scale (scale.rs `width`).
pub const COMPACT_BELOW: f32 = 980.;

/// The window's width below which Clear board and Auto-tile leave the bar for
/// the Menu's list, so that the bar's other buttons stay in reach on the
/// small screens the window is run on (the compact bar takes about 770 px
/// with them); the status bar goes narrow at the same width (status/bar.rs).
pub const NARROW_BELOW: f32 = 800.;

/// Room left for macOS's traffic lights, at the bar's left.
#[cfg(target_os = "macos")]
const TRAFFIC_LIGHTS: Pixels = gpui::px(72.);

pub struct TitleBar {
    models: Models,
    /// Times rendered, for the tests that hold renders to what changed.
    pub(crate) renders: usize,
    /// The Menu's list is open.
    menu: bool,
    /// Where its drag area and window buttons were last laid out.
    pub(crate) controls: Controls,
}

/// The bar's window control areas as last laid out (window coordinates),
/// shared with the `ControlAreas` root.rs draws after the bar: the bar's
/// render clears them and its next prepaint records them again, and a frame
/// that reuses the cached bar leaves them as they were, still right, since a
/// cached view is reused only at the same bounds.
#[derive(Clone, Default)]
pub(crate) struct Controls {
    laid: Rc<RefCell<Vec<(WindowControlArea, Bounds<Pixels>)>>>,
    /// Frames `ControlAreas` has painted, for the tests.
    pub(crate) painted: Rc<Cell<usize>>,
}

impl Controls {
    /// What is laid out now, for the tests.
    #[cfg(test)]
    pub(crate) fn laid(&self) -> Vec<(WindowControlArea, Bounds<Pixels>)> {
        self.laid.borrow().clone()
    }

    /// A child that records its parent's bounds as `area` when laid out; the
    /// parent must be `relative`.
    fn record(&self, area: WindowControlArea) -> impl IntoElement {
        let laid = self.laid.clone();
        canvas(move |bounds, _, _| laid.borrow_mut().push((area, bounds)), |_, _, _, _| {}).absolute().inset_0()
    }
}

/// The bar's control areas, made so in every frame: an element without
/// layout or paint of its own, drawn uncached right after the bar, whose
/// hitboxes (normal ones, so the bar's own buttons under them still see the
/// mouse) are the window's control areas.
pub(crate) struct ControlAreas(pub(crate) Controls);

impl IntoElement for ControlAreas {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for ControlAreas {
    type RequestLayoutState = ();
    type PrepaintState = Vec<(WindowControlArea, Hitbox)>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let style = Style { position: Position::Absolute, ..Style::default() };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
        let laid = self.0.laid.borrow();
        laid.iter().map(|&(area, bounds)| (area, window.insert_hitbox(bounds, HitboxBehavior::Normal))).collect()
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        hitboxes: &mut Self::PrepaintState,
        window: &mut Window,
        _cx: &mut App,
    ) {
        for (area, hitbox) in hitboxes.drain(..) {
            window.insert_window_control_hitbox(area, hitbox);
        }
        self.0.painted.set(self.0.painted.get() + 1);
    }
}

impl TitleBar {
    pub fn new(models: Models, cx: &mut Context<Self>) -> TitleBar {
        cx.observe(&models.shell, |_, _, cx| cx.notify()).detach();
        // Only what the bar shows: a chat's status changing redraws none of it.
        let mut shown = None;
        cx.observe(&models.workspace, move |_, ws, cx| {
            let s = &ws.read(cx).state;
            let now = (s.board, s.layout(), s.auto_tile);
            if shown.replace(now) != Some(now) {
                cx.notify();
            }
        })
        .detach();
        TitleBar { models, renders: 0, menu: false, controls: Controls::default() }
    }
}

/// What the theme button says for each choice; `compact`, the choice alone.
pub fn theme_label(pref: ThemePref, compact: bool) -> &'static str {
    match (pref, compact) {
        (ThemePref::System, false) => "Theme: system",
        (ThemePref::Light, false) => "Theme: light",
        (ThemePref::Dark, false) => "Theme: dark",
        (ThemePref::System, true) => "System",
        (ThemePref::Light, true) => "Light",
        (ThemePref::Dark, true) => "Dark",
    }
}

impl Render for TitleBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        self.controls.laid.borrow_mut().clear();
        let shell: &Shell = self.models.shell.read(cx);
        let (p, pref) = (shell.palette(), shell.pref());
        // The view is cached at the bar's size, so a resize draws it again.
        let compact = scale::width(window) < COMPACT_BELOW;
        let narrow = scale::width(window) < NARROW_BELOW;
        let tips = tips_on(window);

        let bar = div()
            .size_full()
            .flex()
            .items_center()
            .gap(u(4.))
            .px(u(4.))
            .bg(p.title_bg)
            .text_color(p.title_fg)
            .border_b_1()
            .border_color(p.frame);
        #[cfg(target_os = "macos")]
        let bar = bar.pl(TRAFFIC_LIGHTS);

        let name = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(u(6.))
            .pr(u(10.))
            .child(div().w(u(14.)).h(u(14.)).bg(p.attention).border_1().border_color(p.frame))
            .child(div().font_weight(FontWeight::BOLD).text_size(u(14.)).child("Luna"));

        let drag = div()
            .id("title-drag")
            .flex_1()
            .h_full()
            .relative()
            .on_mouse_down(MouseButton::Left, |e: &MouseDownEvent, window, _| {
                if !cfg!(windows) {
                    if e.click_count == 2 {
                        window.zoom_window();
                    } else {
                        window.start_window_move();
                    }
                }
            })
            .child(self.controls.record(WindowControlArea::Drag));

        let state = &self.models.workspace.read(cx).state;
        let (board, layout, auto_tile) = (state.board, state.layout(), state.auto_tile);
        let ws = self.models.workspace.clone();
        let boards = div().flex().flex_none().gap(u(2.)).children((0..BOARDS).map(|b| {
            let ws = ws.clone();
            div()
                .id(("board", b))
                .selector(move || format!("board-{}", b + 1))
                .on_click(move |_, _, cx| ws.update(cx, |w, cx| w.set_board(b, cx)))
                .tip(tips, &p, vec![format!("Board {}", board_label(b)).into()])
                .child(toggle(&p, b == board, board_label(b)))
        }));
        let layouts = div().flex().flex_none().gap(u(2.)).children((1..=4).map(|n| {
            let ws = ws.clone();
            div()
                .id(("layout", n))
                .selector(move || format!("layout-{n}"))
                .on_click(move |_, _, cx| ws.update(cx, |w, cx| w.set_layout(n, cx)))
                .tip(tips, &p, vec![layout_name(n).into()])
                .child(toggle(&p, n == layout, layout_glyph(n)))
        }));
        let clear = {
            let ws = ws.clone();
            div()
                .id("clear-board")
                .selector(|| "clear-board".into())
                .flex_none()
                .on_click(move |_, _, cx| ws.update(cx, |w, cx| w.clear_board(cx)))
                .child(button(&p, "Clear board"))
        };
        let auto = {
            let ws = ws.clone();
            div()
                .id("auto-tile")
                .selector(|| "auto-tile".into())
                .flex_none()
                .on_click(move |_, _, cx| ws.update(cx, |w, cx| w.set_auto_tile(!auto_tile, cx)))
                .child(toggle(&p, auto_tile, auto_label(auto_tile, compact)))
        };
        let models = self.models.clone();
        let theme = div()
            .id("theme")
            .selector(|| "theme".into())
            .flex_none()
            .on_click(move |_, _, cx| models.cycle_theme(cx))
            .child(button(&p, theme_label(pref, compact)));

        let panels = self.panels_menu(&p, narrow.then_some(auto_tile), cx);

        bar.child(name)
            .children((!compact).then(|| div().flex_none().child("Board")))
            .child(boards)
            .child(div().flex_none().pl(u(6.)).when(!compact, |d| d.child("Panes")))
            .child(layouts)
            .child(div().w(u(6.)))
            .children((!narrow).then_some(clear))
            .children((!narrow).then_some(auto))
            .child(drag)
            .child(panels)
            .child(theme)
            .children(window_buttons(&p, &self.controls))
    }
}

impl TitleBar {
    /// The Menu: Library and Accounts. Settings is the status bar's, where
    /// the page keeps it. On a narrow window (`auto_tile` is then the
    /// setting's state) Clear board and Auto-tile are its rows too.
    fn panels_menu(&self, p: &Palette, auto_tile: Option<bool>, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let row = |id: &'static str, label: &'static str| {
            div().id(id).selector(move || id.into()).px(u(8.)).py(u(2.)).bg(p.surface).text_color(p.text).whitespace_nowrap().child(label)
        };
        let mut rows: Vec<gpui::AnyElement> =
            [("title-library", Panel::Library, "Library"), ("title-accounts", Panel::Accounts, "Accounts")]
                .map(|(id, panel, label)| {
                    row(id, label)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.menu = false;
                            cx.notify();
                            panels::open(panel, window, cx);
                        }))
                        .into_any_element()
                })
                .into();
        if let Some(on) = auto_tile {
            let ws = self.models.workspace.clone();
            rows.push(
                row("clear-board", "Clear board")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.menu = false;
                        cx.notify();
                        ws.update(cx, |w, cx| w.clear_board(cx));
                    }))
                    .into_any_element(),
            );
            let ws = self.models.workspace.clone();
            rows.push(
                row("auto-tile", auto_label(on, false))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.menu = false;
                        cx.notify();
                        ws.update(cx, |w, cx| w.set_auto_tile(!on, cx));
                    }))
                    .into_any_element(),
            );
        }
        let list = div()
            .id("title-menu-list")
            .selector(|| "title-menu-list".into())
            .absolute()
            .top(u(22.))
            .right(u(0.))
            .flex()
            .flex_col()
            .py(u(2.))
            .bg(p.surface)
            .border_1()
            .border_color(p.hi)
            .occlude()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.menu = false;
                cx.notify();
            }))
            .children(rows);
        div()
            .id("title-menu")
            .selector(|| "title-menu".into())
            .relative()
            .flex_none()
            .on_click(cx.listener(|this, _, _, cx| {
                this.menu = !this.menu;
                cx.notify();
            }))
            .child(button(p, "Menu"))
            .children(self.menu.then(|| deferred(list).with_priority(1)))
    }
}

impl TitleBar {
    /// Esc: the Menu's list closed, if it was open.
    pub fn close_menu(&mut self, cx: &mut Context<Self>) -> bool {
        if std::mem::take(&mut self.menu) {
            cx.notify();
            return true;
        }
        false
    }

    /// The Menu's list opened, as its click opens it (the shots').
    #[cfg(any(test, feature = "shots"))]
    pub(crate) fn open_menu(&mut self, cx: &mut Context<Self>) {
        self.menu = true;
        cx.notify();
    }
}

/// Auto-tile's label: its state in words, which the compact bar leaves to
/// the button's face (pressed in while on).
fn auto_label(on: bool, compact: bool) -> &'static str {
    match (on, compact) {
        (_, true) => "Auto-tile",
        (true, false) => "Auto-tile: on",
        (false, false) => "Auto-tile: off",
    }
}

/// A layout's button: its number of panes.
fn layout_glyph(n: usize) -> String {
    n.to_string()
}

/// A layout button's tooltip: what its digit means, and its key.
fn layout_name(n: usize) -> String {
    format!("{n} pane{} (Ctrl+Shift+{n})", if n == 1 { "" } else { "s" })
}

/// Minimize, maximize and close, except on macOS, whose own buttons stay.
fn window_buttons(p: &Palette, controls: &Controls) -> Vec<gpui::AnyElement> {
    if cfg!(target_os = "macos") {
        return Vec::new();
    }
    let part = |id: &'static str, area: WindowControlArea, label: &'static str| {
        div()
            .id(id)
            .selector(move || id.into())
            .flex_none()
            .relative()
            .on_click(move |_, window, _| {
                // Windows does these itself, from the hit-test.
                if !cfg!(windows) {
                    match area {
                        WindowControlArea::Min => window.minimize_window(),
                        WindowControlArea::Max => window.zoom_window(),
                        WindowControlArea::Close => window.remove_window(),
                        WindowControlArea::Drag => {}
                    }
                }
            })
            .child(button(p, label))
            .child(controls.record(area))
            .into_any_element()
    };
    vec![
        div().w(u(8.)).into_any_element(),
        part("window-min", WindowControlArea::Min, "_"),
        part("window-max", WindowControlArea::Max, "[]"),
        part("window-close", WindowControlArea::Close, "X"),
    ]
}
