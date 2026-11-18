// Box drawing (U+2500-257F) and block elements (U+2580-259F) as geometry, not
// font glyphs, so lines meet across cells and blocks tile without gaps at any
// font and scale. Each line character is described by its four arms (up,
// right, down, left: none, light, heavy or double), from the Unicode names;
// the dashed, arc and diagonal ones are drawn on their own. Everything is
// worked out in device pixels and snapped there.
use gpui::{Bounds, Pixels, Point, point, px, size};

pub enum Prim {
    /// A filled rectangle, at `alpha` of the cell's foreground.
    Rect(Bounds<Pixels>, f32),
    /// A stroked straight line.
    Line(Point<Pixels>, Point<Pixels>, Pixels),
    /// A stroked quadratic curve: from, control, to.
    Curve(Point<Pixels>, Point<Pixels>, Point<Pixels>, Pixels),
}

pub fn handles(c: char) -> bool {
    ('\u{2500}'..='\u{259f}').contains(&c)
}

const NONE: u8 = 0;
const LIGHT: u8 = 1;
const HEAVY: u8 = 2;
const DOUBLE: u8 = 3;

/// Arms (up, right, down, left) as four digits, for U+2500-257F; "" where
/// the character is drawn specially.
const ARMS: [&str; 128] = [
    "0101", "0202", "1010", "2020", "", "", "", "", "", "", "", "", "0110", "0210", "0120", "0220", // 2500
    "0011", "0012", "0021", "0022", "1100", "1200", "2100", "2200", "1001", "1002", "2001", "2002", "1110", "1210", "2110", "1120", // 2510
    "2120", "2210", "1220", "2220", "1011", "1012", "2011", "1021", "2021", "2012", "1022", "2022", "0111", "0112", "0211", "0212", // 2520
    "0121", "0122", "0221", "0222", "1101", "1102", "1201", "1202", "2101", "2102", "2201", "2202", "1111", "1112", "1211", "1212", // 2530
    "2111", "1121", "2121", "2112", "2211", "1122", "1221", "2212", "1222", "2122", "2221", "2222", "", "", "", "", // 2540
    "0303", "3030", "0310", "0130", "0330", "0013", "0031", "0033", "1300", "3100", "3300", "1003", "3001", "3003", "1310", "3130", // 2550
    "3330", "1013", "3031", "3033", "0313", "0131", "0333", "1303", "3101", "3303", "1313", "3131", "3333", "", "", "", // 2560
    "", "", "", "", "0001", "1000", "0100", "0010", "0002", "2000", "0200", "0020", "0201", "1020", "0102", "2010", // 2570
];

/// Geometry in device pixels for one cell.
struct Cell {
    x0: i32,
    y0: i32,
    w: i32,
    h: i32,
    /// Light line thickness.
    t: i32,
    scale: f32,
}

impl Cell {
    fn rect(&self, x0: i32, y0: i32, x1: i32, y1: i32, out: &mut Vec<Prim>) {
        if x1 > x0 && y1 > y0 {
            let s = self.scale;
            out.push(Prim::Rect(
                Bounds::new(
                    point(px((self.x0 + x0) as f32 / s), px((self.y0 + y0) as f32 / s)),
                    size(px((x1 - x0) as f32 / s), px((y1 - y0) as f32 / s)),
                ),
                1.0,
            ));
        }
    }

    fn thickness(&self, weight: u8) -> i32 {
        if weight == HEAVY { self.t * 2 } else { self.t }
    }

    /// Start of the band a line of `weight` occupies across `len` (the cell's
    /// width for a vertical line, its height for a horizontal one).
    fn band_start(&self, len: i32, weight: u8) -> i32 {
        (len - self.thickness(weight)) / 2
    }

    /// The span [a, b) a line of `weight` occupies across `len`; for a double
    /// line, from the outer edge of one stroke to that of the other.
    fn band(&self, len: i32, weight: u8) -> (i32, i32) {
        let c = self.band_start(len, LIGHT);
        match weight {
            DOUBLE => (c - self.t, c + 2 * self.t),
            NONE => (c, c + self.t),
            w => {
                let s = self.band_start(len, w);
                (s, s + self.thickness(w))
            }
        }
    }

    fn pt(&self, x: f32, y: f32) -> Point<Pixels> {
        point(px((self.x0 as f32 + x) / self.scale), px((self.y0 as f32 + y) / self.scale))
    }
}

/// Appends the primitives for `c` in the cell [x0, x1) x [y0, y1) (logical
/// pixels) at `scale`. Returns false if `c` is not one it draws.
pub fn draw(c: char, x0: f32, y0: f32, x1: f32, y1: f32, scale: f32, out: &mut Vec<Prim>) -> bool {
    if !handles(c) {
        return false;
    }
    let dx0 = (x0 * scale).round() as i32;
    let dy0 = (y0 * scale).round() as i32;
    let cell = Cell {
        x0: dx0,
        y0: dy0,
        w: ((x1 * scale).round() as i32 - dx0).max(1),
        h: ((y1 * scale).round() as i32 - dy0).max(1),
        t: (((x1 - x0) * scale / 8.0).round() as i32).max(1),
        scale,
    };
    let code = c as u32;
    if code >= 0x2580 {
        block(code, &cell, out);
        return true;
    }
    let arms = ARMS[(code - 0x2500) as usize];
    if arms.is_empty() {
        special(code, &cell, out);
        return true;
    }
    let a: Vec<u8> = arms.bytes().map(|b| b - b'0').collect();
    lines(&cell, [a[0], a[1], a[2], a[3]], out);
    true
}

fn lines(cell: &Cell, [up, right, down, left]: [u8; 4], out: &mut Vec<Prim>) {
    let (w, h, t) = (cell.w, cell.h, cell.t);
    let vmax = up.max(down);
    let hmax = left.max(right);
    // Where a horizontal arm meets the vertical band, and the other way round.
    let (vband_l, vband_r) = cell.band(w, if vmax == NONE { right.max(left) } else { vmax });
    let (hband_t, hband_b) = cell.band(h, if hmax == NONE { up.max(down) } else { hmax });
    let cx = cell.band_start(w, LIGHT);
    let cy = cell.band_start(h, LIGHT);
    let g = t; // distance from the center stroke to each stroke of a double line

    // Horizontal arms.
    for (weight, is_right) in [(right, true), (left, false)] {
        match weight {
            NONE => {}
            DOUBLE => {
                for (y, same, other) in [(cy - g, up, down), (cy + g, down, up)] {
                    // Stop at the near stroke of a double line on this side
                    // (an inner corner), reach the far one when the double line
                    // is on the other side only (an outer corner), meet a single
                    // line's band, or run to the center.
                    let (start, end) = if same == DOUBLE {
                        (cx + g, cx - g + t)
                    } else if other == DOUBLE {
                        (cx - g, cx + g + t)
                    } else if vmax != NONE {
                        (vband_l, vband_r)
                    } else {
                        (cx, cx + t)
                    };
                    if is_right { cell.rect(start, y, w, y + t, out) } else { cell.rect(0, y, end, y + t, out) }
                }
            }
            weight => {
                let s = cell.band_start(h, weight);
                let th = cell.thickness(weight);
                if is_right {
                    let start = if vmax != NONE { vband_l } else { cell.band_start(w, weight) };
                    cell.rect(start, s, w, s + th, out);
                } else {
                    let end = if vmax != NONE { vband_r } else { cell.band_start(w, weight) + th };
                    cell.rect(0, s, end, s + th, out);
                }
            }
        }
    }
    // Vertical arms.
    for (weight, is_down) in [(down, true), (up, false)] {
        match weight {
            NONE => {}
            DOUBLE => {
                for (x, same, other) in [(cx - g, left, right), (cx + g, right, left)] {
                    let (start, end) = if same == DOUBLE {
                        (cy + g, cy - g + t)
                    } else if other == DOUBLE {
                        (cy - g, cy + g + t)
                    } else if hmax != NONE {
                        (hband_t, hband_b)
                    } else {
                        (cy, cy + t)
                    };
                    if is_down { cell.rect(x, start, x + t, h, out) } else { cell.rect(x, 0, x + t, end, out) }
                }
            }
            weight => {
                let s = cell.band_start(w, weight);
                let th = cell.thickness(weight);
                if is_down {
                    let start = if hmax != NONE { hband_t } else { cell.band_start(h, weight) };
                    cell.rect(s, start, s + th, h, out);
                } else {
                    let end = if hmax != NONE { hband_b } else { cell.band_start(h, weight) + th };
                    cell.rect(s, 0, s + th, end, out);
                }
            }
        }
    }
}

fn dashes(cell: &Cell, horizontal: bool, weight: u8, n: i32, out: &mut Vec<Prim>) {
    let th = cell.thickness(weight);
    let len = if horizontal { cell.w } else { cell.h };
    let across = if horizontal { cell.band_start(cell.h, weight) } else { cell.band_start(cell.w, weight) };
    for i in 0..n {
        let a = len * i / n;
        let b = len * (i + 1) / n;
        let dash_end = a + ((b - a) * 3 / 5).max(1);
        if horizontal {
            cell.rect(a, across, dash_end, across + th, out);
        } else {
            cell.rect(across, a, across + th, dash_end, out);
        }
    }
}

fn special(code: u32, cell: &Cell, out: &mut Vec<Prim>) {
    let (w, h) = (cell.w as f32, cell.h as f32);
    let t = cell.t as f32;
    let stroke = px(t / cell.scale);
    // Stroke centers, matching the light bands' centers.
    let cx = cell.band_start(cell.w, LIGHT) as f32 + t / 2.0;
    let cy = cell.band_start(cell.h, LIGHT) as f32 + t / 2.0;
    match code {
        0x2504 => dashes(cell, true, LIGHT, 3, out),
        0x2505 => dashes(cell, true, HEAVY, 3, out),
        0x2506 => dashes(cell, false, LIGHT, 3, out),
        0x2507 => dashes(cell, false, HEAVY, 3, out),
        0x2508 => dashes(cell, true, LIGHT, 4, out),
        0x2509 => dashes(cell, true, HEAVY, 4, out),
        0x250a => dashes(cell, false, LIGHT, 4, out),
        0x250b => dashes(cell, false, HEAVY, 4, out),
        0x254c => dashes(cell, true, LIGHT, 2, out),
        0x254d => dashes(cell, true, HEAVY, 2, out),
        0x254e => dashes(cell, false, LIGHT, 2, out),
        0x254f => dashes(cell, false, HEAVY, 2, out),
        // Arcs: straight into the cell from both edges, a quarter curve
        // between, of radius half the shorter side.
        0x256d..=0x2570 => {
            let r = (w.min(h) / 2.0).floor();
            let (ex, ey) = match code {
                0x256d => (w, h), // down and right
                0x256e => (0.0, h), // down and left
                0x256f => (0.0, 0.0), // up and left
                _ => (w, 0.0), // up and right
            };
            let sx = if ex > cx { 1.0 } else { -1.0 };
            let sy = if ey > cy { 1.0 } else { -1.0 };
            let a = cell.pt(cx + sx * r, cy);
            let b = cell.pt(cx, cy + sy * r);
            out.push(Prim::Line(cell.pt(ex, cy), a, stroke));
            out.push(Prim::Curve(a, cell.pt(cx, cy), b, stroke));
            out.push(Prim::Line(b, cell.pt(cx, ey), stroke));
        }
        0x2571 => out.push(Prim::Line(cell.pt(w, 0.0), cell.pt(0.0, h), stroke)),
        0x2572 => out.push(Prim::Line(cell.pt(0.0, 0.0), cell.pt(w, h), stroke)),
        0x2573 => {
            out.push(Prim::Line(cell.pt(w, 0.0), cell.pt(0.0, h), stroke));
            out.push(Prim::Line(cell.pt(0.0, 0.0), cell.pt(w, h), stroke));
        }
        _ => {}
    }
}

fn block(code: u32, cell: &Cell, out: &mut Vec<Prim>) {
    let (w, h) = (cell.w, cell.h);
    let eighth_y = |n: i32| h * n / 8;
    let eighth_x = |n: i32| w * n / 8;
    let (hw, hh) = (w / 2, h / 2);
    match code {
        0x2580 => cell.rect(0, 0, w, hh, out),
        0x2581..=0x2587 => cell.rect(0, h - eighth_y((code - 0x2580) as i32), w, h, out),
        0x2588 => cell.rect(0, 0, w, h, out),
        0x2589..=0x258f => cell.rect(0, 0, eighth_x(8 - (code - 0x2588) as i32), h, out),
        0x2590 => cell.rect(hw, 0, w, h, out),
        0x2591..=0x2593 => {
            let alpha = (code - 0x2590) as f32 * 0.25;
            cell.rect(0, 0, w, h, out);
            if let Some(Prim::Rect(_, a)) = out.last_mut() {
                *a = alpha;
            }
        }
        0x2594 => cell.rect(0, 0, w, eighth_y(1), out),
        0x2595 => cell.rect(w - eighth_x(1), 0, w, h, out),
        _ => {
            // Quadrants: upper left, upper right, lower left, lower right.
            let q: [bool; 4] = match code {
                0x2596 => [false, false, true, false],
                0x2597 => [false, false, false, true],
                0x2598 => [true, false, false, false],
                0x2599 => [true, false, true, true],
                0x259a => [true, false, false, true],
                0x259b => [true, true, true, false],
                0x259c => [true, true, false, true],
                0x259d => [false, true, false, false],
                0x259e => [false, true, true, false],
                _ => [false, true, true, true], // 259F
            };
            if q[0] {
                cell.rect(0, 0, hw, hh, out);
            }
            if q[1] {
                cell.rect(hw, 0, w, hh, out);
            }
            if q[2] {
                cell.rect(0, hh, hw, h, out);
            }
            if q[3] {
                cell.rect(hw, hh, w, h, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rects(c: char) -> Vec<(f32, f32, f32, f32, f32)> {
        let mut out = Vec::new();
        assert!(draw(c, 0.0, 0.0, 8.0, 16.0, 1.0, &mut out));
        out.iter()
            .filter_map(|p| match p {
                Prim::Rect(b, a) => Some((b.origin.x.as_f32(), b.origin.y.as_f32(), b.size.width.as_f32(), b.size.height.as_f32(), *a)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn lines_span_the_cell_and_meet() {
        // A light horizontal runs edge to edge (two arms overlapping at the
        // center), one pixel thick, mid-height.
        assert_eq!(rects('─'), vec![(3.0, 7.0, 5.0, 1.0, 1.0), (0.0, 7.0, 4.0, 1.0, 1.0)]);
        // A corner's two arms share the center pixel.
        let corner = rects('┌');
        assert!(corner.contains(&(3.0, 7.0, 5.0, 1.0, 1.0)));
        assert!(corner.contains(&(3.0, 7.0, 1.0, 9.0, 1.0)));
        // A cross is four arms.
        assert_eq!(rects('┼').len(), 4);
        // A double horizontal is two strokes.
        assert_eq!(rects('═').len(), 4);
    }

    #[test]
    fn blocks_tile() {
        assert_eq!(rects('█'), vec![(0.0, 0.0, 8.0, 16.0, 1.0)]);
        assert_eq!(rects('▀'), vec![(0.0, 0.0, 8.0, 8.0, 1.0)]);
        assert_eq!(rects('▄'), vec![(0.0, 8.0, 8.0, 8.0, 1.0)]);
        assert_eq!(rects('▌'), vec![(0.0, 0.0, 4.0, 16.0, 1.0)]);
        assert_eq!(rects('▒'), vec![(0.0, 0.0, 8.0, 16.0, 0.5)]);
        assert_eq!(rects('▟').len(), 3);
        assert!(!draw('a', 0.0, 0.0, 8.0, 16.0, 1.0, &mut Vec::new()));
    }
}
