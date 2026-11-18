// Cell colors: the pane's own 16-color palette and defaults, the xterm 256-color
// cube and gray ramp, truecolor as given, and the terminal's OSC 4/10/11
// overrides on top.
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};

/// 0xRRGGBB.
pub type Rgb24 = u32;

pub struct Palette {
    pub ansi: [Rgb24; 16],
    pub fg: Rgb24,
    pub bg: Rgb24,
    pub cursor: Rgb24,
    pub selection: Rgb24,
}

pub const PALETTE: Palette = Palette {
    ansi: [
        0x1d1f21, 0xcc6666, 0xb5bd68, 0xf0c674, 0x81a2be, 0xb294bb, 0x8abeb7, 0xc5c8c6, //
        0x666666, 0xd54e53, 0xb9ca4a, 0xe7c547, 0x7aa6da, 0xc397d8, 0x70c0b1, 0xeaeaea,
    ],
    fg: 0xd4d4d4,
    bg: 0x181a1f,
    cursor: 0xe8e8e8,
    selection: 0x3a4a66,
};

fn pack(c: Rgb) -> Rgb24 {
    ((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32
}

/// Entry `i` of the xterm 256-color table.
pub fn indexed(i: u8) -> Rgb24 {
    match i {
        0..=15 => PALETTE.ansi[i as usize],
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

/// Two thirds of the color, for SGR 2 (dim) on colors without a dim variant.
pub fn dim(c: Rgb24) -> Rgb24 {
    let f = |s: u32| ((c >> s) & 0xff) * 2 / 3;
    (f(16) << 16) | (f(8) << 8) | f(0)
}

pub fn resolve(color: Color, overrides: &Colors) -> Rgb24 {
    match color {
        Color::Spec(c) => pack(c),
        Color::Indexed(i) => overrides[i as usize].map(pack).unwrap_or_else(|| indexed(i)),
        Color::Named(n) => {
            if let Some(c) = overrides[n] {
                return pack(c);
            }
            let i = n as usize;
            if i < 16 {
                return PALETTE.ansi[i];
            }
            match n {
                NamedColor::Foreground | NamedColor::BrightForeground => PALETTE.fg,
                NamedColor::Background => PALETTE.bg,
                NamedColor::Cursor => PALETTE.cursor,
                NamedColor::DimForeground => dim(PALETTE.fg),
                NamedColor::DimBlack => dim(PALETTE.ansi[0]),
                NamedColor::DimRed => dim(PALETTE.ansi[1]),
                NamedColor::DimGreen => dim(PALETTE.ansi[2]),
                NamedColor::DimYellow => dim(PALETTE.ansi[3]),
                NamedColor::DimBlue => dim(PALETTE.ansi[4]),
                NamedColor::DimMagenta => dim(PALETTE.ansi[5]),
                NamedColor::DimCyan => dim(PALETTE.ansi[6]),
                NamedColor::DimWhite => dim(PALETTE.ansi[7]),
                _ => PALETTE.fg,
            }
        }
    }
}

pub fn hsla(c: Rgb24) -> gpui::Hsla {
    gpui::rgb(c).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_256_color_table() {
        assert_eq!(indexed(16), 0x000000);
        assert_eq!(indexed(21), 0x0000ff);
        assert_eq!(indexed(196), 0xff0000);
        assert_eq!(indexed(231), 0xffffff);
        assert_eq!(indexed(232), 0x080808);
        assert_eq!(indexed(255), 0xeeeeee);
    }

    #[test]
    fn truecolor_and_overrides() {
        let mut o = Colors::default();
        assert_eq!(resolve(Color::Spec(Rgb { r: 1, g: 2, b: 3 }), &o), 0x010203);
        o[1] = Some(Rgb { r: 0xaa, g: 0, b: 0 });
        assert_eq!(resolve(Color::Indexed(1), &o), 0xaa0000);
        assert_eq!(resolve(Color::Named(NamedColor::Red), &o), 0xaa0000);
        assert_eq!(resolve(Color::Named(NamedColor::Background), &o), PALETTE.bg);
    }
}
