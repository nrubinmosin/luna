//! The look: two retro, high-contrast palettes. Solid fills, 1 px bevels,
//! black frames; no gradients, shadows or rounding; status fills saturated,
//! with black text on them. Body text is at 7:1 or better against the ground
//! it sits on. Nothing moves: gpui-component's motion is off and its caret
//! steady.

use crate::font::FAMILY;
use gpui::{App, Hsla, Pixels, px, rgb};
use gpui_component::input::CursorBlink;
use gpui_component::{Theme, ThemeMode};
use std::time::Duration;

/// The one text size of the window's chrome.
pub const TEXT_SIZE: Pixels = px(12.);

fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

/// The colors every view paints with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub dark: bool,
    /// Controls and bars.
    pub surface: Hsla,
    pub text: Hsla,
    pub text_dim: Hsla,
    /// A bevel's lit and shaded edges.
    pub hi: Hsla,
    pub lo: Hsla,
    /// Every frame: the window's, a pane's, a badge's.
    pub frame: Hsla,
    pub title_bg: Hsla,
    pub title_fg: Hsla,
    /// What the panes sit on.
    pub board: Hsla,
    /// A sunken field: an input, a meter's track.
    pub well: Hsla,
    /// Text on a status fill, whatever the theme.
    pub on_status: Hsla,
    /// Status fills: working or up, waiting for a person, waiting on Luna
    /// itself, and failed.
    pub ok: Hsla,
    pub attention: Hsla,
    pub info: Hsla,
    pub error: Hsla,
}

impl Palette {
    pub fn dark() -> Palette {
        Palette {
            dark: true,
            surface: c(0x262626),
            text: c(0xffffff),     // 15.9:1 on surface
            text_dim: c(0xc8c8c8), // 10.1:1 on surface
            hi: c(0x7a7a7a),
            lo: c(0x000000),
            frame: c(0x000000),
            title_bg: c(0x0000a8), // VGA blue; white on it is 12:1
            title_fg: c(0xffffff),
            board: c(0x000000),
            well: c(0x000000),
            on_status: c(0x000000),
            ok: c(0x00e000),
            attention: c(0xffd000),
            info: c(0x00c8ff),
            error: c(0xff3c3c),
        }
    }

    pub fn light() -> Palette {
        Palette {
            dark: false,
            surface: c(0xc0c0c0),
            text: c(0x000000),     // 11.5:1 on surface
            text_dim: c(0x303030), // 7.3:1 on surface
            hi: c(0xffffff),
            lo: c(0x000000),
            frame: c(0x000000),
            title_bg: c(0x000080), // navy; white on it is 16:1
            title_fg: c(0xffffff),
            board: c(0x008080), // the old desktop teal
            well: c(0xffffff),
            on_status: c(0x000000),
            ok: c(0x00c000),
            attention: c(0xffc000),
            info: c(0x00c0ff),
            error: c(0xff3c3c),
        }
    }

    pub fn of(dark: bool) -> Palette {
        if dark { Palette::dark() } else { Palette::light() }
    }
}

/// gpui-component's theme to match: the editor and whatever else of its
/// widgets the window hosts later paint in the same colors, square, flat and
/// still, in the one font.
pub fn apply_to_components(p: &Palette, cx: &mut App) {
    // The mode first, in an edit of its own: switching it loads that mode's
    // registered theme over whatever colors the same edit set (Theme::update).
    Theme::change(if p.dark { ThemeMode::Dark } else { ThemeMode::Light }, None, cx);
    Theme::update(cx, |t| {
        t.font_family = FAMILY.into();
        t.mono_font_family = FAMILY.into();
        t.font_size = TEXT_SIZE;
        t.mono_font_size = TEXT_SIZE;
        t.radius = px(0.);
        t.radius_lg = px(0.);
        t.window_radius = px(0.);
        t.shadow = false;
        t.focus_ring = false;
        t.cursor_blink = CursorBlink::steady();
        let m = &mut t.motion;
        for d in [&mut m.duration_instant, &mut m.duration_fast, &mut m.duration_normal, &mut m.duration_slow] {
            *d = Duration::ZERO;
        }

        let (sel_bg, sel_fg) = if p.dark { (c(0xffffff), c(0x000000)) } else { (c(0x000080), c(0xffffff)) };
        let k = &mut t.colors;
        k.background = p.surface;
        k.foreground = p.text;
        k.border = p.frame;
        k.input = p.frame;
        k.ring = p.frame;
        k.caret = p.text;
        k.selection = sel_bg.opacity(0.35);
        k.accent = sel_bg;
        k.accent_foreground = sel_fg;
        k.muted = p.surface;
        k.muted_foreground = p.text_dim;
        k.popover = p.surface;
        k.popover_foreground = p.text;
        k.primary = p.title_bg;
        k.primary_hover = p.title_bg;
        k.primary_active = p.title_bg;
        k.primary_foreground = p.title_fg;
        k.secondary = p.surface;
        k.secondary_hover = p.surface;
        k.secondary_active = p.hi;
        k.secondary_foreground = p.text;
        k.button = p.surface;
        k.button_hover = p.surface;
        k.button_active = p.hi;
        k.button_foreground = p.text;
        k.list = p.well;
        k.list_even = p.well;
        k.list_hover = p.surface;
        k.list_active = sel_bg;
        k.list_active_border = p.frame;
        k.title_bar = p.title_bg;
        k.title_bar_border = p.frame;
        k.window_border = p.frame;
        k.overlay = p.frame.opacity(0.4);
        k.scrollbar = p.well;
        k.scrollbar_thumb = p.hi;
        k.scrollbar_thumb_hover = p.hi;
        k.success = p.ok;
        k.success_foreground = p.on_status;
        k.warning = p.attention;
        k.warning_foreground = p.on_status;
        k.info = p.info;
        k.info_foreground = p.on_status;
        k.danger = p.error;
        k.danger_foreground = p.on_status;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG's contrast ratio of two colors.
    fn contrast(a: Hsla, b: Hsla) -> f32 {
        let lum = |c: Hsla| {
            let c = c.to_rgb();
            let ch = |v: f32| if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
            0.2126 * ch(c.r) + 0.7152 * ch(c.g) + 0.0722 * ch(c.b)
        };
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    /// Body text at 7:1 (WCAG AAA); the black on a status fill, bold and
    /// short, at 4.5:1 (AA) at least, which a red that still reads as red
    /// allows.
    #[test]
    fn text_is_legible_on_its_ground() {
        for p in [Palette::dark(), Palette::light()] {
            assert!(contrast(p.text, p.surface) >= 7.0, "{p:?}");
            assert!(contrast(p.text_dim, p.surface) >= 7.0, "{p:?}");
            assert!(contrast(p.title_fg, p.title_bg) >= 7.0, "{p:?}");
            for fill in [p.ok, p.attention, p.info, p.error] {
                assert!(contrast(p.on_status, fill) >= 4.5, "{fill:?} in {p:?}");
            }
        }
    }
}
