//! The window's own title bar: Luna's name, the board's controls (the five
//! boards, the four layouts, Clear board, Auto-tile), the buttons that open
//! the library, the accounts and settings windows (panels/, in one Menu: all
//! three as buttons would not fit the smallest window), the theme button and
//! the window's buttons, on the title blue, the rest of it a handle to move
//! the window by.
//!
//! On Windows the parts are hit-test areas (`window_control_area`), so the
//! system moves, maximizes on a double-click, minimizes and closes as it does
//! for any title bar. Elsewhere gpui has no such
//! hit-test: the drag area starts the move itself and a double-click zooms,
//! and the buttons call the window's own. On macOS the system's traffic
//! lights stay, in a title bar made transparent over this one, and take the
//! buttons' place.

use crate::dto::ThemePref;
use crate::models::{Models, Shell};
use crate::panels::{self, Panel};
use crate::state::{BOARDS, board_label};
use crate::theme::Palette;
use crate::widgets::{button, toggle};
use gpui::{
    Context, FontWeight, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, ParentElement, Pixels, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, WindowControlArea, deferred, div, px,
};

/// Its height, fixed: the view is cached at this size (root.rs).
pub const HEIGHT: Pixels = px(28.);

/// Room left for macOS's traffic lights, at the bar's left.
#[cfg(target_os = "macos")]
const TRAFFIC_LIGHTS: Pixels = px(72.);

pub struct TitleBar {
    models: Models,
    /// Times rendered, for the tests that hold renders to what changed.
    pub(crate) renders: usize,
    /// The Menu's list is open.
    menu: bool,
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
        TitleBar { models, renders: 0, menu: false }
    }
}

/// What the theme button says for each choice.
pub fn theme_label(pref: ThemePref) -> &'static str {
    match pref {
        ThemePref::System => "Theme: system",
        ThemePref::Light => "Theme: light",
        ThemePref::Dark => "Theme: dark",
    }
}

impl Render for TitleBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let shell: &Shell = self.models.shell.read(cx);
        let (p, pref) = (shell.palette(), shell.pref());

        let bar = div()
            .size_full()
            .flex()
            .items_center()
            .gap(px(4.))
            .px(px(4.))
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
            .gap(px(6.))
            .pr(px(10.))
            .child(div().w(px(14.)).h(px(14.)).bg(p.attention).border_1().border_color(p.frame))
            .child(div().font_weight(FontWeight::BOLD).text_size(px(14.)).child("Luna"));

        let drag = div().id("title-drag").flex_1().h_full().window_control_area(WindowControlArea::Drag).on_mouse_down(
            MouseButton::Left,
            |e: &MouseDownEvent, window, _| {
                if !cfg!(windows) {
                    if e.click_count == 2 {
                        window.zoom_window();
                    } else {
                        window.start_window_move();
                    }
                }
            },
        );

        let state = &self.models.workspace.read(cx).state;
        let (board, layout, auto_tile) = (state.board, state.layout(), state.auto_tile);
        let ws = self.models.workspace.clone();
        let boards = div().flex().flex_none().gap(px(2.)).children((0..BOARDS).map(|b| {
            let ws = ws.clone();
            div()
                .id(("board", b))
                .debug_selector(move || format!("board-{}", b + 1))
                .on_click(move |_, _, cx| ws.update(cx, |w, cx| w.set_board(b, cx)))
                .child(toggle(&p, b == board, board_label(b)))
        }));
        let layouts = div().flex().flex_none().gap(px(2.)).children((1..=4).map(|n| {
            let ws = ws.clone();
            div()
                .id(("layout", n))
                .debug_selector(move || format!("layout-{n}"))
                .on_click(move |_, _, cx| ws.update(cx, |w, cx| w.set_layout(n, cx)))
                .child(toggle(&p, n == layout, layout_glyph(n)))
        }));
        let clear = {
            let ws = ws.clone();
            div()
                .id("clear-board")
                .debug_selector(|| "clear-board".into())
                .flex_none()
                .on_click(move |_, _, cx| ws.update(cx, |w, cx| w.clear_board(cx)))
                .child(button(&p, "Clear board"))
        };
        let auto = {
            let ws = ws.clone();
            div()
                .id("auto-tile")
                .debug_selector(|| "auto-tile".into())
                .flex_none()
                .on_click(move |_, _, cx| ws.update(cx, |w, cx| w.set_auto_tile(!auto_tile, cx)))
                .child(toggle(&p, auto_tile, if auto_tile { "Auto-tile: on" } else { "Auto-tile: off" }))
        };
        let models = self.models.clone();
        let theme = div()
            .id("theme")
            .debug_selector(|| "theme".into())
            .flex_none()
            .on_click(move |_, _, cx| models.cycle_theme(cx))
            .child(button(&p, theme_label(pref)));

        let panels = self.panels_menu(&p, cx);

        bar.child(name)
            .child(div().flex_none().child("Board"))
            .child(boards)
            .child(div().flex_none().pl(px(6.)).child("Panes"))
            .child(layouts)
            .child(div().w(px(6.)))
            .child(clear)
            .child(auto)
            .child(drag)
            .child(panels)
            .child(theme)
            .children(window_buttons(&p))
    }
}

impl TitleBar {
    /// The Menu: Library, Accounts and Settings. Settings is here until the
    /// status bar has it, where the page keeps it.
    fn panels_menu(&self, p: &Palette, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let rows =
            [("library", Panel::Library, "Library"), ("accounts", Panel::Accounts, "Accounts"), ("settings", Panel::Settings, "Settings")]
                .map(|(key, panel, label)| {
                    div()
                        .id(SharedString::from(format!("title-{key}")))
                        .debug_selector(move || format!("title-{key}"))
                        .px(px(8.))
                        .py(px(2.))
                        .bg(p.surface)
                        .text_color(p.text)
                        .whitespace_nowrap()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.menu = false;
                            cx.notify();
                            panels::open(panel, window, cx);
                        }))
                        .child(label)
                });
        let list = div()
            .id("title-menu-list")
            .absolute()
            .top(px(22.))
            .right(px(0.))
            .flex()
            .flex_col()
            .py(px(2.))
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
            .debug_selector(|| "title-menu".into())
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

/// A layout's button: its number of panes.
fn layout_glyph(n: usize) -> String {
    n.to_string()
}

/// Minimize, maximize and close, except on macOS, whose own buttons stay.
fn window_buttons(p: &Palette) -> Vec<gpui::AnyElement> {
    if cfg!(target_os = "macos") {
        return Vec::new();
    }
    let part = |id: &'static str, area: WindowControlArea, label: &'static str| {
        div()
            .id(id)
            .flex_none()
            .window_control_area(area)
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
            .into_any_element()
    };
    vec![
        div().w(px(8.)).into_any_element(),
        part("window-min", WindowControlArea::Min, "_"),
        part("window-max", WindowControlArea::Max, "[]"),
        part("window-close", WindowControlArea::Close, "X"),
    ]
}
