//! What the panels are built of, in the window's look: a framed window with
//! a title bar, raised buttons, square boxes and radios,
//! segmented choices, a drop-down list, sunken fields and meters, and lines
//! in a status fill for what went wrong. Nothing here moves or blinks.
//!
//! Every control that does something carries an id that is also its debug
//! selector, so the tests click it where it is drawn.

use crate::scale::u;
use crate::selector::Selector;
use crate::theme::Palette;
use crate::widgets::{Tipped, bevel};
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, Div, ElementId, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement, Pixels, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Window, anchored, deferred, div, relative, rgb,
};
use std::rc::Rc;

/// A click's handler.
pub type OnClick = Rc<dyn Fn(&mut Window, &mut App)>;

/// Text on a control that cannot be used now: grayed, as the period had it.
pub fn disabled_text(p: &Palette) -> Hsla {
    rgb(if p.dark { 0x7a7a7a } else { 0x808080 }).into()
}

fn id_of(id: &str) -> ElementId {
    ElementId::Name(SharedString::from(id.to_owned()))
}

/// A window inside the window: a title bar with its title and a close box,
/// over `body`, in a black frame. `width` fixed; as tall as its body up to
/// the window's height.
pub fn frame(
    p: &Palette,
    id: &str,
    title: impl Into<SharedString>,
    width: Pixels,
    close: OnClick,
    body: impl IntoElement,
) -> Stateful<Div> {
    let close_id = format!("{id}-close");
    let bar = div()
        .flex()
        .flex_none()
        .items_center()
        .gap(u(6.))
        .h(u(24.))
        .px(u(4.))
        .bg(p.title_bg)
        .text_color(p.title_fg)
        .border_b_1()
        .border_color(p.frame)
        .child(div().flex_1().min_w_0().truncate().font_weight(FontWeight::BOLD).child(title.into()))
        .child(button(&close_id, p, "X", true, close));
    let selector = id.to_owned();
    div()
        .id(id_of(id))
        .selector(move || selector)
        .flex()
        .flex_col()
        .w(width)
        .max_w_full()
        .max_h_full()
        .bg(p.surface)
        .text_color(p.text)
        .border_1()
        .border_color(p.frame)
        .child(bar)
        .child(body)
}

/// A raised button, pressed in while it is held; grayed and inert when not
/// `enabled`.
pub fn button(id: &str, p: &Palette, label: impl Into<SharedString>, enabled: bool, on_click: OnClick) -> Stateful<Div> {
    face(id, p, label.into(), enabled, false, on_click)
}

/// The button a panel's main action is: on the title color.
pub fn primary(id: &str, p: &Palette, label: impl Into<SharedString>, enabled: bool, on_click: OnClick) -> Stateful<Div> {
    face(id, p, label.into(), enabled, true, on_click)
}

fn face(id: &str, p: &Palette, label: SharedString, enabled: bool, main: bool, on_click: OnClick) -> Stateful<Div> {
    let (bg, fg) = match (enabled, main) {
        (false, _) => (p.surface, disabled_text(p)),
        (true, true) => (p.title_bg, p.title_fg),
        (true, false) => (p.surface, p.text),
    };
    let inner = div()
        .flex()
        .items_center()
        .justify_center()
        .px(u(8.))
        .h(u(18.))
        .bg(bg)
        .text_color(fg)
        .font_weight(FontWeight::BOLD)
        .whitespace_nowrap()
        .child(label);
    let selector = id.to_owned();
    div().id(id_of(id)).selector(move || selector.clone()).flex_none().child(bevel(p, true, inner)).when(enabled, |d| {
        d.on_click(move |_, window, cx| {
            cx.stop_propagation();
            on_click(window, cx)
        })
    })
}

/// A square box and its label: `X` in it when on.
pub fn check(id: &str, p: &Palette, label: impl Into<SharedString>, on: bool, enabled: bool, on_click: OnClick) -> Stateful<Div> {
    tick(id, p, label.into(), on, enabled, false, on_click)
}

/// A radio: a square too (nothing is round here), a block in it when chosen.
pub fn radio(id: &str, p: &Palette, label: impl Into<SharedString>, on: bool, enabled: bool, on_click: OnClick) -> Stateful<Div> {
    tick(id, p, label.into(), on, enabled, true, on_click)
}

fn tick(id: &str, p: &Palette, label: SharedString, on: bool, enabled: bool, radio: bool, on_click: OnClick) -> Stateful<Div> {
    let fg = if enabled { p.text } else { disabled_text(p) };
    let mark = match (on, radio) {
        (false, _) => div(),
        (true, false) => div().text_color(fg).font_weight(FontWeight::BOLD).line_height(u(11.)).child("X"),
        (true, true) => div().size(u(5.)).bg(fg),
    };
    let square =
        bevel(p, false, div().size(u(11.)).flex().items_center().justify_center().bg(if enabled { p.well } else { p.surface }).child(mark));
    let selector = id.to_owned();
    div()
        .id(id_of(id))
        .selector(move || selector.clone())
        .flex()
        .flex_none()
        .items_center()
        .gap(u(6.))
        .min_h(u(18.))
        .text_color(fg)
        .child(div().flex_none().child(square))
        .when(!label.is_empty(), |d| d.child(div().min_w_0().child(label)))
        .when(enabled, |d| {
            d.on_click(move |_, window, cx| {
                cx.stop_propagation();
                on_click(window, cx)
            })
        })
}

/// One of a few choices side by side, the chosen one pressed in.
pub fn segmented(id: &str, p: &Palette, choices: Vec<(SharedString, bool, OnClick)>, enabled: bool) -> Div {
    segmented_tipped(id, p, choices.into_iter().map(|(label, chosen, on_click)| (label, chosen, on_click, None)).collect(), enabled, false)
}

/// The same, a choice with a hint showing it as its tooltip where the
/// window offers them now (`tips`, widgets.rs `tips_on`).
pub fn segmented_tipped(
    id: &str,
    p: &Palette,
    choices: Vec<(SharedString, bool, OnClick, Option<SharedString>)>,
    enabled: bool,
    tips: bool,
) -> Div {
    let row = div().flex().flex_none().flex_wrap();
    row.children(choices.into_iter().enumerate().map(|(i, (label, chosen, on_click, hint))| {
        let fg = if enabled { p.text } else { disabled_text(p) };
        let inner = div()
            .flex()
            .items_center()
            .px(u(8.))
            .h(u(18.))
            .bg(if chosen { p.well } else { p.surface })
            .text_color(fg)
            .when(chosen, |d| d.font_weight(FontWeight::BOLD))
            .whitespace_nowrap()
            .child(label);
        let selector = format!("{id}-{i}");
        let sel = selector.clone();
        div()
            .id(id_of(&selector))
            .selector(move || sel.clone())
            .child(bevel(p, !chosen, inner))
            .when_some(hint.filter(|_| tips), |d, hint| d.tip(true, p, vec![hint]))
            .when(enabled && !chosen, |d| {
                d.on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    on_click(window, cx)
                })
            })
    }))
}

/// A drop-down list: its face says what is chosen; open, the items hang
/// under it over whatever is there, until one is picked or a click lands
/// elsewhere (`on_close`). An item is `(label, chosen, pick)`; `keep_open`
/// for a list of boxes ticked one after another. Items of a group follow its
/// heading, indented, as the items are given in order.
pub struct Dropdown {
    pub id: String,
    pub face: SharedString,
    pub open: bool,
    pub enabled: bool,
    pub width: Pixels,
    pub items: Vec<DropItem>,
    pub on_toggle: OnClick,
    pub on_close: OnClick,
}

pub struct DropItem {
    pub label: SharedString,
    pub chosen: bool,
    /// A box ticked, not a choice made: the list stays open.
    pub tick: bool,
    pub enabled: bool,
    /// The heading it is listed under; None for the items before any.
    pub group: Option<SharedString>,
    pub pick: OnClick,
}

impl Dropdown {
    pub fn render(self, p: &Palette) -> Stateful<Div> {
        let Dropdown { id, face, open, enabled, width, items, on_toggle, on_close } = self;
        let fg = if enabled { p.text } else { disabled_text(p) };
        let inner = div()
            .flex()
            .items_center()
            .gap(u(4.))
            .w(width)
            .h(u(18.))
            .px(u(4.))
            .bg(if enabled { p.well } else { p.surface })
            .text_color(fg)
            .child(div().flex_1().min_w_0().truncate().child(face))
            .child(div().flex_none().font_weight(FontWeight::BOLD).child("v"));
        let selector = id.clone();
        let toggle = on_toggle.clone();
        let mut root =
            div().id(id_of(&id)).selector(move || selector.clone()).flex_none().child(bevel(p, false, inner)).when(enabled, |d| {
                d.on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    toggle(window, cx)
                })
            });
        if open {
            let list_selector = format!("{id}-list");
            let list = div()
                .id(id_of(&list_selector))
                .selector(move || list_selector)
                .occlude()
                .min_w(width)
                .max_h(u(320.))
                .overflow_y_scroll()
                .bg(p.surface)
                .border_1()
                .border_color(p.frame)
                .py(u(2.))
                .on_mouse_down_out(move |_, window, cx| on_close(window, cx))
                .children(grouped(p, &id, items));
            // Hung under the face: anchored where it is laid out, which a
            // box just below the face's top left puts there.
            let below = div()
                .absolute()
                .top(u(20.))
                .left_0()
                .child(deferred(anchored().snap_to_window_with_margin(u(8.)).child(list)).with_priority(1));
            root = root.relative().child(below);
        }
        root
    }
}

/// The list's lines: each item, and a group's heading before its first.
/// Items keep their numbers (`{id}-{i}`) whatever the headings between them.
fn grouped(p: &Palette, id: &str, items: Vec<DropItem>) -> Vec<AnyElement> {
    let mut out = Vec::with_capacity(items.len());
    let mut under: Option<SharedString> = None;
    for (i, item) in items.into_iter().enumerate() {
        if item.group.is_some() && item.group != under {
            let selector = format!("{id}-group-{i}");
            out.push(
                div()
                    .selector(move || selector.clone())
                    .flex()
                    .items_center()
                    .px(u(6.))
                    .h(u(20.))
                    .text_color(p.text_dim)
                    .font_weight(FontWeight::BOLD)
                    .whitespace_nowrap()
                    .children(item.group.clone())
                    .into_any_element(),
            );
        }
        under.clone_from(&item.group);
        out.push(drop_item(p, id, i, item));
    }
    out
}

fn drop_item(p: &Palette, id: &str, i: usize, item: DropItem) -> AnyElement {
    let selector = format!("{id}-{i}");
    let sel = selector.clone();
    let DropItem { label, chosen, tick: is_tick, enabled, group, pick } = item;
    let fg = if enabled { p.text } else { disabled_text(p) };
    let line = div()
        .id(id_of(&selector))
        .selector(move || sel.clone())
        .flex()
        .items_center()
        .gap(u(6.))
        .px(u(6.))
        .h(u(20.))
        .text_color(fg)
        .whitespace_nowrap()
        .when(group.is_some(), |d| d.pl(u(14.)));
    let line = if is_tick {
        let mark = if chosen { "[X]" } else { "[ ]" };
        line.child(div().flex_none().child(mark)).child(label)
    } else if chosen {
        line.bg(p.title_bg).text_color(p.title_fg).child(label)
    } else {
        line.child(label)
    };
    line.when(enabled, |d| {
        d.on_click(move |_, window, cx| {
            cx.stop_propagation();
            pick(window, cx)
        })
    })
    .into_any_element()
}

/// A section's heading, ruled under.
pub fn section(p: &Palette, label: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .items_center()
        .gap(u(8.))
        .mt(u(10.))
        .mb(u(4.))
        .pb(u(2.))
        .border_b_1()
        .border_color(p.frame)
        .font_weight(FontWeight::BOLD)
        .child(label.into())
}

/// A line of explanation, in the dim text.
pub fn note(p: &Palette, text: impl Into<SharedString>) -> Div {
    div().text_color(p.text_dim).child(text.into())
}

/// What a line in a status fill says: trouble, a question, or Luna at work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Trouble,
    Attention,
    Working,
    Done,
}

/// A line in a status fill, black text on it.
pub fn alert(p: &Palette, tone: Tone, text: impl Into<SharedString>) -> Div {
    let fill = match tone {
        Tone::Trouble => p.error,
        Tone::Attention => p.attention,
        Tone::Working => p.info,
        Tone::Done => p.ok,
    };
    div().px(u(6.)).py(u(2.)).bg(fill).text_color(p.on_status).border_1().border_color(p.frame).child(text.into())
}

/// A setting's row: its name and what it does on the left, its control on
/// the right, and anything more under both.
pub fn row(p: &Palette, label: impl Into<SharedString>, desc: Option<SharedString>, control: impl IntoElement) -> Div {
    div()
        .flex()
        .items_start()
        .gap(u(12.))
        .py(u(4.))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(div().font_weight(FontWeight::BOLD).child(label.into()))
                .children(desc.map(|d| div().text_color(p.text_dim).child(d))),
        )
        .child(div().flex_none().flex().items_center().gap(u(4.)).child(control))
}

/// A sunken track filled to `used` (0 to 1), in `fill`.
pub fn meter(p: &Palette, used: Option<f32>, fill: Hsla) -> Div {
    let track = div().h(u(12.)).flex_1().bg(p.well);
    let track = match used {
        Some(u) => track.child(div().h_full().w(relative(u.clamp(0., 1.))).bg(fill).border_r_1().border_color(p.frame)),
        None => track,
    };
    bevel(p, false, track).flex_1()
}

/// A question over a panel: what it asks, and its two answers.
pub struct Confirm {
    pub title: SharedString,
    pub body: SharedString,
    pub yes: SharedString,
    pub on_yes: OnClick,
    pub on_no: OnClick,
}

impl Confirm {
    /// Over the whole panel, which it keeps from being clicked.
    pub fn render(self, p: &Palette, id: &str) -> Div {
        let Confirm { title, body, yes, on_yes, on_no } = self;
        let buttons = div()
            .flex()
            .justify_end()
            .gap(u(6.))
            .child(button(&format!("{id}-no"), p, "Cancel", true, on_no.clone()))
            .child(primary(&format!("{id}-yes"), p, yes, true, on_yes));
        let body = div().flex().flex_col().gap(u(10.)).p(u(10.)).child(div().child(body)).child(buttons);
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(p.frame.opacity(0.35))
            .occlude()
            .child(frame(p, id, title, u(440.), on_no, body))
    }
}

/// A path that may not fit: its end kept in view, the whole of it on hover.
pub fn path_text(p: &Palette, path: impl Into<SharedString>) -> Div {
    div().min_w_0().text_color(p.text_dim).text_ellipsis_start().whitespace_nowrap().overflow_hidden().child(path.into())
}

/// A folder's last part, as its column or row is named.
pub fn base_name(path: &str) -> String {
    let trimmed = path.trim_end_matches(['/', '\\']);
    trimmed.rsplit(['/', '\\']).next().filter(|s| !s.is_empty()).unwrap_or(path).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_is_named_by_its_last_part() {
        assert_eq!(base_name("C:\\work\\luna\\"), "luna");
        assert_eq!(base_name("/home/me/repo"), "repo");
        assert_eq!(base_name("/"), "/");
    }
}
