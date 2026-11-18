//! The window's own title bar: Luna's name, the theme button and the window's
//! buttons, on the title blue, the rest of it a handle to move the window by.
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
use crate::theme::Palette;
use crate::widgets::button;
use gpui::{
    Context, FontWeight, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, ParentElement, Pixels, Render,
    StatefulInteractiveElement, Styled, Window, WindowControlArea, div, px,
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
}

impl TitleBar {
    pub fn new(models: Models, cx: &mut Context<Self>) -> TitleBar {
        cx.observe(&models.shell, |_, _, cx| cx.notify()).detach();
        TitleBar { models, renders: 0 }
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

        let models = self.models.clone();
        let theme = div()
            .id("theme")
            .debug_selector(|| "theme".into())
            .flex_none()
            .on_click(move |_, _, cx| models.cycle_theme(cx))
            .child(button(&p, theme_label(pref)));

        bar.child(name).child(drag).child(theme).children(window_buttons(&p))
    }
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
