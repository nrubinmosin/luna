//! The small parts every view is built of: 1 px bevels and buttons in them,
//! a chat's status as a fill, and tooltips.

use crate::font::FAMILY;
use crate::scale::{self, u};
use crate::selector::Selector;
use crate::state::Status;
use crate::theme::Palette;
use gpui::{
    AnyView, App, Div, FontWeight, Hsla, IntoElement, ParentElement, Pixels, SharedString, StatefulInteractiveElement, Styled, TextRun,
    Window, div, font, px,
};
use gpui_component::tooltip::Tooltip;

/// A 1 px raised (or sunken) bevel around `content`: lit on the top and left,
/// shaded on the bottom and right, swapped when sunken.
pub fn bevel(p: &Palette, raised: bool, content: Div) -> Div {
    let (tl, br) = if raised { (p.hi, p.lo) } else { (p.lo, p.hi) };
    div()
        .flex()
        .border_b_1()
        .border_r_1()
        .border_color(br)
        .min_w_0()
        .child(content.flex_1().min_w_0().border_t_1().border_l_1().border_color(tl))
}

/// A raised button's face with its label, bold, 18 px high.
pub fn button(p: &Palette, label: impl IntoElement) -> Div {
    bevel(
        p,
        true,
        div()
            .flex()
            .items_center()
            .justify_center()
            .px(u(8.))
            .h(u(18.))
            .bg(p.surface)
            .text_color(p.text)
            .font_weight(FontWeight::BOLD)
            .whitespace_nowrap()
            .child(label),
    )
}

/// A button's face pressed in: a choice that is on (a board, a layout).
pub fn pressed(p: &Palette, label: impl IntoElement) -> Div {
    bevel(
        p,
        false,
        div()
            .flex()
            .items_center()
            .justify_center()
            .px(u(8.))
            .h(u(18.))
            .bg(p.well)
            .text_color(p.text)
            .font_weight(FontWeight::BOLD)
            .whitespace_nowrap()
            .child(label),
    )
}

/// A button, raised or pressed in by `on`.
pub fn toggle(p: &Palette, on: bool, label: impl IntoElement) -> Div {
    if on { pressed(p, label) } else { button(p, label) }
}

/// A chat's status as a fill, saturated, with black text on it: working,
/// waiting for the person, background; resting is the plain ground, and a
/// chat the core is not up to tell of the sunken one.
pub fn status_fill(p: &Palette, status: Status) -> (Hsla, Hsla) {
    match status {
        Status::Working => (p.ok, p.on_status),
        Status::Waiting => (p.attention, p.on_status),
        Status::Background => (p.info, p.on_status),
        Status::Resting => (p.surface, p.text_dim),
        Status::NoCore => (p.well, p.text_dim),
    }
}

/// A chat's status as a small framed chip with its word.
pub fn status_chip(p: &Palette, status: Status) -> Div {
    let (bg, fg) = status_fill(p, status);
    div()
        .flex_none()
        .px(u(4.))
        .h(u(16.))
        .flex()
        .items_center()
        .bg(bg)
        .text_color(fg)
        .border_1()
        .border_color(p.frame)
        .font_weight(FontWeight::BOLD)
        .text_size(u(10.))
        .child(status.word().to_uppercase())
}

/// A chat's status as a square, where a word does not fit.
pub fn status_square(p: &Palette, status: Status) -> Div {
    let (bg, _) = status_fill(p, status);
    div().flex_none().w(u(10.)).h(u(10.)).bg(bg).border_1().border_color(p.frame)
}

/// A small framed tag: a draft, a pane's number.
pub fn tag(p: &Palette, fill: Option<Hsla>, label: impl IntoElement) -> Div {
    let (bg, fg) = match fill {
        Some(fill) => (fill, p.on_status),
        None => (p.surface, p.text),
    };
    div()
        .flex_none()
        .px(u(3.))
        .h(u(16.))
        .flex()
        .items_center()
        .bg(bg)
        .text_color(fg)
        .border_1()
        .border_color(p.frame)
        .text_size(u(10.))
        .font_weight(FontWeight::BOLD)
        .child(label)
}

/// The widest a tooltip's text is laid out at 100%; a longer line wraps.
pub const TIP_MAX: f32 = 480.;

/// What a tooltip takes beside its text: gpui-component's margin and
/// padding and its border, unscaled.
const TIP_FRAME: f32 = 2. * (12. + 8. + 1.);

#[cfg(test)]
thread_local! {
    /// The pointer outside the window, for the tests: the test platform's
    /// window says it is never hovered.
    pub static POINTER_OUT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether the window offers tooltips now: not while the pointer is outside
/// it. Windows says the pointer left (WM_MOUSELEAVE) with no move after the
/// last one inside, and gpui takes a tooltip down only on a move off its
/// element, so one left through the window's edge would stay up. gpui draws
/// every view again as the pointer leaves the window and comes back
/// (`Window::refresh` on the hover change), and a view drawn without its
/// tooltip shows none. Elsewhere gpui's hover means the window is active,
/// and the platform's own moves take a tooltip down.
pub fn tips_on(window: &Window) -> bool {
    #[cfg(test)]
    if POINTER_OUT.with(std::cell::Cell::get) {
        return false;
    }
    !cfg!(windows) || window.is_window_hovered()
}

/// A tooltip on an element, a line per entry, where `on` (`tips_on`).
pub trait Tipped: StatefulInteractiveElement + Sized {
    fn tip(self, on: bool, p: &Palette, lines: Vec<SharedString>) -> Self {
        if on { self.tooltip(tip(p, lines)) } else { self }
    }
}

impl<E: StatefulInteractiveElement> Tipped for E {}

/// A tooltip for gpui's own `.tooltip`, a line per entry: gpui-component's
/// Tooltip, flat and in the window's colors. gpui builds it once the pointer
/// has rested on its element half a second (one timer, armed only by a move
/// over it) and drops it as the pointer leaves; nothing redraws while it is
/// up. Not gpui-component's managed tooltips, which slide in and out: the
/// look is static.
///
/// gpui lays a tooltip out with no width to fit in, so its text would not
/// wrap and a long name or path would run out of the window. The text gets
/// a width of its own: its widest line's, at most `TIP_MAX` and the
/// window's width less the frame, and wraps there.
fn tip(p: &Palette, lines: Vec<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let p = *p;
    move |window, cx| {
        let size = scale::text();
        let run =
            |len: usize| TextRun { len, font: font(FAMILY), color: p.text, background_color: None, underline: None, strikethrough: None };
        let widest = lines.iter().map(|l| window.text_system().layout_line(l, size, &[run(l.len())], None).width).fold(px(0.), Pixels::max);
        let room = (window.viewport_size().width - px(TIP_FRAME)).max(px(0.));
        // A pixel over the widest line: its own width, laid out again, must
        // not wrap it.
        let width = (widest + px(1.)).min(u(TIP_MAX)).min(room);
        let lines = lines.clone();
        Tooltip::element(move |_, _| div().selector(|| "tip".into()).w(width).flex().flex_col().children(lines.clone()))
            .bg(p.surface)
            .text_color(p.text)
            .border_color(p.frame)
            .rounded_none()
            .shadow_none()
            .text_size(size)
            .build(window, cx)
    }
}
