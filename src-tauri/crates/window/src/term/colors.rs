//! A cell's colors: the pane's 16-color ramps and defaults, one per theme in
//! the window's retro high-contrast look, the xterm 256-color cube and gray
//! ramp, truecolor as given, and the terminal's OSC 4/10/11 overrides on top.

use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};

/// 0xRRGGBB.
pub type Rgb24 = u32;

/// The colors a pane paints with when the program sets none.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TermPalette {
    pub ansi: [Rgb24; 16],
    pub fg: Rgb24,
    pub bg: Rgb24,
    pub cursor: Rgb24,
    /// A selected cell's colors, both: the window's own selection, white on
    /// navy in the light theme, black on white in the dark.
    pub selection_fg: Rgb24,
    pub selection_bg: Rgb24,
}

/// Every colored entry reads at 5:1 or better on its ground; black is the
/// ground in the dark ramp, and bright white is black in the light one, where
/// programs written for dark terminals put their brightest text.
pub const DARK: TermPalette = TermPalette {
    ansi: [
        0x000000, 0xff5050, 0x30d030, 0xe0c000, 0x6f8cff, 0xe060e0, 0x20c8c8, 0xc0c0c0, //
        0x8a8a8a, 0xff8080, 0x70ff70, 0xffff60, 0x9cb0ff, 0xff88ff, 0x70ffff, 0xffffff,
    ],
    fg: 0xe8e8e8,
    bg: 0x000000,
    cursor: 0xffd000,
    selection_fg: 0x000000,
    selection_bg: 0xffffff,
};

pub const LIGHT: TermPalette = TermPalette {
    ansi: [
        0x000000, 0xb00000, 0x006a00, 0x765000, 0x0000c8, 0x9a009a, 0x006868, 0x6a6a6a, //
        0x505050, 0xd00000, 0x007a00, 0x6a4a00, 0x2020ff, 0xb000b0, 0x007878, 0x000000,
    ],
    fg: 0x000000,
    bg: 0xffffff,
    cursor: 0x000000,
    selection_fg: 0xffffff,
    selection_bg: 0x000080,
};

impl TermPalette {
    pub fn of(dark: bool) -> &'static TermPalette {
        if dark { &DARK } else { &LIGHT }
    }

    /// Entry `i` of the xterm 256-color table.
    pub fn indexed(&self, i: u8) -> Rgb24 {
        match i {
            0..=15 => self.ansi[i as usize],
            16..=231 => {
                let i = i as u32 - 16;
                let step = |n: u32| if n == 0 { 0 } else { 55 + n * 40 };
                (step(i / 36) << 16) | (step((i / 6) % 6) << 8) | step(i % 6)
            }
            232..=255 => {
                let g = 8 + (i as u32 - 232) * 10;
                (g << 16) | (g << 8) | g
            }
        }
    }

    pub fn resolve(&self, color: Color, overrides: &Colors) -> Rgb24 {
        match color {
            Color::Spec(c) => pack(c),
            Color::Indexed(i) => overrides[i as usize].map(pack).unwrap_or_else(|| self.indexed(i)),
            Color::Named(n) => {
                if let Some(c) = overrides[n] {
                    return pack(c);
                }
                let i = n as usize;
                if i < 16 {
                    return self.ansi[i];
                }
                match n {
                    NamedColor::Foreground | NamedColor::BrightForeground => self.fg,
                    NamedColor::Background => self.bg,
                    NamedColor::Cursor => self.cursor,
                    NamedColor::DimForeground => dim(self.fg, self.bg),
                    NamedColor::DimBlack => dim(self.ansi[0], self.bg),
                    NamedColor::DimRed => dim(self.ansi[1], self.bg),
                    NamedColor::DimGreen => dim(self.ansi[2], self.bg),
                    NamedColor::DimYellow => dim(self.ansi[3], self.bg),
                    NamedColor::DimBlue => dim(self.ansi[4], self.bg),
                    NamedColor::DimMagenta => dim(self.ansi[5], self.bg),
                    NamedColor::DimCyan => dim(self.ansi[6], self.bg),
                    NamedColor::DimWhite => dim(self.ansi[7], self.bg),
                    _ => self.fg,
                }
            }
        }
    }

    /// What OSC 4/10/11/12 queries are answered with: index 256 the
    /// foreground, 257 the background, 258 the cursor.
    pub fn answer(&self, index: usize, overrides: &Colors) -> Rgb {
        let c = match index {
            256 => self.resolve(Color::Named(NamedColor::Foreground), overrides),
            257 => self.resolve(Color::Named(NamedColor::Background), overrides),
            258 => self.resolve(Color::Named(NamedColor::Cursor), overrides),
            i => self.resolve(Color::Indexed(i.min(255) as u8), overrides),
        };
        Rgb { r: (c >> 16) as u8, g: (c >> 8) as u8, b: c as u8 }
    }
}

fn pack(c: Rgb) -> Rgb24 {
    ((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32
}

/// SGR 2 (dim): two thirds of the way from the ground to the color, so dim
/// text fades toward the background in either theme.
pub fn dim(c: Rgb24, ground: Rgb24) -> Rgb24 {
    let mix = |s: u32| {
        let (a, b) = (((c >> s) & 0xff) as i32, ((ground >> s) & 0xff) as i32);
        (b + (a - b) * 2 / 3) as u32
    };
    (mix(16) << 16) | (mix(8) << 8) | mix(0)
}

pub fn hsla(c: Rgb24) -> gpui::Hsla {
    gpui::rgb(c).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contrast(a: Rgb24, b: Rgb24) -> f32 {
        let lum = |c: Rgb24| {
            let ch = |s: u32| {
                let v = ((c >> s) & 0xff) as f32 / 255.;
                if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * ch(16) + 0.7152 * ch(8) + 0.0722 * ch(0)
        };
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn the_256_color_table() {
        assert_eq!(DARK.indexed(16), 0x000000);
        assert_eq!(DARK.indexed(21), 0x0000ff);
        assert_eq!(DARK.indexed(196), 0xff0000);
        assert_eq!(DARK.indexed(231), 0xffffff);
        assert_eq!(LIGHT.indexed(232), 0x080808);
        assert_eq!(LIGHT.indexed(255), 0xeeeeee);
        assert_eq!(LIGHT.indexed(1), LIGHT.ansi[1]);
    }

    #[test]
    fn truecolor_and_overrides() {
        let mut o = Colors::default();
        assert_eq!(DARK.resolve(Color::Spec(Rgb { r: 1, g: 2, b: 3 }), &o), 0x010203);
        o[1] = Some(Rgb { r: 0xaa, g: 0, b: 0 });
        assert_eq!(DARK.resolve(Color::Indexed(1), &o), 0xaa0000);
        assert_eq!(DARK.resolve(Color::Named(NamedColor::Red), &o), 0xaa0000);
        assert_eq!(DARK.resolve(Color::Named(NamedColor::Background), &o), DARK.bg);
        // OSC 11 sets the background, and a query answers with it.
        o[NamedColor::Background] = Some(Rgb { r: 0x10, g: 0x20, b: 0x30 });
        assert_eq!(LIGHT.resolve(Color::Named(NamedColor::Background), &o), 0x102030);
        assert_eq!(LIGHT.answer(257, &o), Rgb { r: 0x10, g: 0x20, b: 0x30 });
        assert_eq!(LIGHT.answer(256, &o), Rgb { r: 0, g: 0, b: 0 });
    }

    #[test]
    fn dim_fades_toward_the_ground() {
        assert_eq!(dim(0xffffff, 0x000000), 0xaaaaaa);
        assert_eq!(dim(0x000000, 0xffffff), 0x555555);
    }

    /// Text in any of the colored entries, and the default text, reads on the
    /// pane's ground at 5:1 or better; the selection and the cursor stand out.
    #[test]
    fn colors_are_legible_on_their_ground() {
        for p in [&DARK, &LIGHT] {
            assert!(contrast(p.fg, p.bg) >= 7.0, "{p:?}");
            for i in (1..7).chain(9..15) {
                assert!(contrast(p.ansi[i], p.bg) >= 5.0, "{i} in {p:?}");
            }
            assert!(contrast(p.selection_fg, p.selection_bg) >= 7.0, "{p:?}");
            assert!(contrast(p.cursor, p.bg) >= 7.0, "{p:?}");
        }
    }
}
