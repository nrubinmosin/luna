//! The small parts every view is built of: 1 px bevels and buttons in them.

use crate::theme::Palette;
use gpui::{Div, FontWeight, IntoElement, ParentElement, Styled, div, px};

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
            .px(px(8.))
            .h(px(18.))
            .bg(p.surface)
            .text_color(p.text)
            .font_weight(FontWeight::BOLD)
            .child(label),
    )
}
