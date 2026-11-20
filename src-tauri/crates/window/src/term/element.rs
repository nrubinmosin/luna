//! The terminal grid as one gpui Element. Prepaint takes the Term's lock once,
//! resizes it (and has the session's PTY told) to the cells that fit, copies
//! the visible cells out with their colors resolved, and lets go; everything
//! after works on the copy: background quads merged per run of equal color,
//! box drawing and block elements as geometry (boxdraw.rs), and the glyphs.
//! Each row is shaped once (layout_line, with font runs only for bold and
//! italic, so a row whose colors change but text does not hits gpui's line
//! cache), and every glyph is painted on its own cell in its own color.

use super::TermPane;
use super::boxdraw::{self, Prim};
use super::colors::{self, Rgb24, TermPalette};
use super::io::{Io, PtySize};
use super::parser::Shared;
use crate::font::FAMILY;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point as GridPoint};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor};
use gpui::{
    App, BorderStyle, Bounds, ContentMask, DispatchPhase, ElementId, ElementInputHandler, Entity, FocusHandle, Font, FontId, FontStyle,
    FontWeight, GlobalElementId, GlyphId, Hsla, InspectorElementId, IntoElement, LayoutId, MouseButton, MouseUpEvent, PathBuilder, Pixels,
    Point, Style, TextRun, UnderlineStyle, Window, fill, font, outline, point, px, relative, size,
};
use std::cell::Cell;
use std::rc::Rc;
use unicode_width::UnicodeWidthStr;

/// The cell grid's geometry as last laid out, for mouse hit-testing and the
/// IME's candidate window.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Geom {
    pub origin: Point<Pixels>,
    pub cell_w: Pixels,
    pub line_h: Pixels,
    pub cols: usize,
    pub rows: usize,
    /// The cursor's cell in the viewport, if it is in view.
    pub cursor: Option<(usize, usize)>,
    pub scale: f32,
}

impl Geom {
    /// The size the session's PTY is told: cells, and the grid's size in
    /// device pixels, from the exact scaled cell metrics, rounded per cell
    /// (what TIOCSWINSZ's pixel fields carry; ConPTY takes cells only).
    pub fn pty_size(&self) -> PtySize {
        pty_size(self.cols, self.rows, self.cell_w, self.line_h, self.scale)
    }
}

pub fn pty_size(cols: usize, rows: usize, cell_w: Pixels, line_h: Pixels, scale: f32) -> PtySize {
    let cell_w = (cell_w.as_f32() * scale).round() as u32;
    let cell_h = (line_h.as_f32() * scale).round() as u32;
    let clamp = |v: u32| v.min(u16::MAX as u32) as u16;
    PtySize { cols: clamp(cols as u32), rows: clamp(rows as u32), width: clamp(cols as u32 * cell_w), height: clamp(rows as u32 * cell_h) }
}

#[derive(Clone, Copy, PartialEq)]
pub struct Metrics {
    pub font_size: Pixels,
    pub scale: f32,
    pub cell_w: Pixels,
    pub line_h: Pixels,
    pub ascent: Pixels,
    pub descent: Pixels,
}

/// Cell metrics from the font: the advance of 'm' and ascent plus descent,
/// the line rounded up to whole device pixels.
pub fn metrics(window: &Window, font_size: Pixels) -> Metrics {
    let ts = window.text_system();
    let id = ts.resolve_font(&font(FAMILY));
    let scale = window.scale_factor();
    let mut cell_w = ts.advance(id, font_size, 'm').map(|s| s.width).unwrap_or(px(0.));
    let ascent = ts.ascent(id, font_size);
    // Font metrics give the descent below the baseline as a negative number.
    let descent = ts.descent(id, font_size).abs();
    // The test platform's text system has no real metrics.
    if cell_w.as_f32().is_nan() || cell_w.as_f32() <= 0.5 {
        cell_w = font_size * 0.6;
    }
    let mut line = (ascent + descent).as_f32();
    if line.is_nan() || line <= 1.0 {
        line = font_size.as_f32() * 1.3;
    }
    let line_h = px((line * scale).ceil() / scale);
    Metrics { font_size, scale, cell_w, line_h, ascent, descent }
}

#[derive(Clone, Copy)]
struct SCell {
    c: char,
    fg: Rgb24,
    bg: Rgb24,
    ul: Rgb24,
    flags: Flags,
}

struct Snap {
    cols: usize,
    rows: usize,
    cells: Vec<SCell>,
    /// Combining marks by cell index.
    zerowidth: Vec<(usize, Vec<char>)>,
    cursor: Option<(usize, usize, CursorShape)>,
    cursor_color: Rgb24,
    default_bg: Rgb24,
    default_fg: Rgb24,
}

struct GlyphOp {
    origin: Point<Pixels>,
    font_id: FontId,
    id: GlyphId,
    color: Hsla,
    emoji: bool,
}

pub struct PaintState {
    font_size: Pixels,
    default_bg: Hsla,
    quads: Vec<(Bounds<Pixels>, Hsla)>,
    shapes: Vec<(Prim, Hsla)>,
    glyphs: Vec<GlyphOp>,
    wavy: Vec<(Point<Pixels>, Pixels, Hsla)>,
    cursor: Vec<gpui::PaintQuad>,
    marked: Option<(gpui::ShapedLine, Point<Pixels>, Bounds<Pixels>)>,
    line_h: Pixels,
}

pub struct TermElement {
    pub view: Entity<TermPane>,
    pub term: Shared,
    pub io: Rc<Io>,
    pub geom: Rc<Cell<Geom>>,
    pub metrics: Rc<Cell<Option<Metrics>>>,
    pub focus: FocusHandle,
    pub marked: Option<String>,
    pub font_size: Pixels,
    pub palette: &'static TermPalette,
    #[cfg(feature = "shots")]
    pub bench: Option<Rc<std::cell::RefCell<super::bench::FrameStats>>>,
}

impl IntoElement for TermElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

fn hsla(c: Rgb24) -> Hsla {
    colors::hsla(c)
}

fn snap(v: f32, scale: f32) -> f32 {
    (v * scale).round() / scale
}

impl gpui::Element for TermElement {
    type RequestLayoutState = ();
    type PrepaintState = PaintState;

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
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        _cx: &mut App,
    ) -> PaintState {
        let scale = window.scale_factor();
        let m = match self.metrics.get() {
            Some(m) if m.font_size == self.font_size && m.scale == scale => m,
            _ => {
                let m = metrics(window, self.font_size);
                self.metrics.set(Some(m));
                m
            }
        };
        let cols = ((bounds.size.width / m.cell_w).floor() as usize).max(2);
        let rows = ((bounds.size.height / m.line_h).floor() as usize).max(1);
        let focused = self.focus.is_focused(window);
        let snap_ = self.snapshot(cols, rows, m, focused);
        self.geom.set(Geom {
            origin: bounds.origin,
            cell_w: m.cell_w,
            line_h: m.line_h,
            cols,
            rows,
            cursor: snap_.cursor.map(|(c, r, _)| (c, r)),
            scale,
        });
        self.build(&snap_, bounds, m, focused, window)
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        st: &mut PaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.handle_input(&self.focus, ElementInputHandler::new(bounds, self.view.clone()), cx);
        // A drop ends on the button's release, wherever it is: by position,
        // not by gpui's hover, which a drop from outside does not move.
        let view = self.view.clone();
        window.on_mouse_event(move |e: &MouseUpEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble && e.button == MouseButton::Left {
                let inside = cx.has_active_drag() && bounds.contains(&e.position);
                view.update(cx, |p, cx| p.drag_released(inside, window, cx));
            }
        });
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            window.paint_quad(fill(bounds, st.default_bg));
            for (b, c) in st.quads.drain(..) {
                window.paint_quad(fill(b, c));
            }
            for (prim, color) in st.shapes.drain(..) {
                match prim {
                    Prim::Rect(b, a) => window.paint_quad(fill(b, color.opacity(a))),
                    Prim::Line(from, to, w) => {
                        let mut pb = PathBuilder::stroke(w);
                        pb.move_to(from);
                        pb.line_to(to);
                        if let Ok(path) = pb.build() {
                            window.paint_path(path, color);
                        }
                    }
                    Prim::Curve(from, ctrl, to, w) => {
                        let mut pb = PathBuilder::stroke(w);
                        pb.move_to(from);
                        pb.curve_to(to, ctrl);
                        if let Ok(path) = pb.build() {
                            window.paint_path(path, color);
                        }
                    }
                }
            }
            for g in st.glyphs.drain(..) {
                let _ = if g.emoji {
                    window.paint_emoji(g.origin, g.font_id, g.id, st.font_size)
                } else {
                    window.paint_glyph(g.origin, g.font_id, g.id, st.font_size, g.color)
                };
            }
            for (origin, width, color) in st.wavy.drain(..) {
                let style = UnderlineStyle { thickness: px(1.), color: Some(color), wavy: true };
                window.paint_underline(origin, width, &style);
            }
            for q in st.cursor.drain(..) {
                window.paint_quad(q);
            }
            if let Some((line, origin, cover)) = st.marked.take() {
                window.paint_quad(fill(cover, st.default_bg));
                let _ = line.paint(origin, st.line_h, gpui::TextAlign::Left, None, window, cx);
            }
        });
        #[cfg(feature = "shots")]
        if let Some(b) = &self.bench {
            b.borrow_mut().paint_end(std::time::Instant::now());
        }
    }
}

impl TermElement {
    /// Resizes the Term (and has the PTY told) if the cells that fit changed,
    /// then copies the viewport out under the one lock of the frame.
    fn snapshot(&self, cols: usize, rows: usize, m: Metrics, focused: bool) -> Snap {
        #[cfg(feature = "shots")]
        let asked = std::time::Instant::now();
        let mut term = self.term.lock();
        #[cfg(feature = "shots")]
        let got = std::time::Instant::now();
        if term.columns() != cols || term.screen_lines() != rows {
            term.resize(TermSize::new(cols, rows));
            #[cfg(feature = "shots")]
            if let Some(b) = &self.bench {
                b.borrow_mut().resized(cols, rows);
            }
        }
        // The cells or the cell's pixel size (a scale change) changed: the
        // session's PTY is told, once it runs.
        self.io.resize(pty_size(cols, rows, m.cell_w, m.line_h, m.scale));
        #[cfg(feature = "shots")]
        if let Some(b) = &self.bench {
            b.borrow_mut().copying(cols, rows);
        }

        let p = self.palette;
        let content = term.renderable_content();
        let offset = content.display_offset;
        let overrides = content.colors;
        let selection = content.selection;
        let cursor_shape = content.cursor.shape;
        let cursor_line = content.cursor.point.line.0 + offset as i32;
        let cursor_col = content.cursor.point.column.0;
        let cursor = (cursor_shape != CursorShape::Hidden && (0..rows as i32).contains(&cursor_line))
            .then(|| (cursor_col.min(cols - 1), cursor_line as usize, cursor_shape));
        let default_bg = p.resolve(Color::Named(NamedColor::Background), overrides);
        let default_fg = p.resolve(Color::Named(NamedColor::Foreground), overrides);
        let cursor_color = p.resolve(Color::Named(NamedColor::Cursor), overrides);
        let block_cursor = focused && matches!(cursor, Some((_, _, CursorShape::Block)));

        let grid = term.grid();
        let mut cells = Vec::with_capacity(cols * rows);
        let mut zerowidth = Vec::new();
        for r in 0..rows {
            let line = Line(r as i32 - offset as i32);
            let row = &grid[line];
            for c in 0..cols {
                let cell = &row[Column(c)];
                let flags = cell.flags;
                let mut fg = match (flags.contains(Flags::DIM), cell.fg) {
                    (true, Color::Named(n)) if (n as usize) < 8 => p.resolve(Color::Named(dim_named(n)), overrides),
                    (true, color) => colors::dim(p.resolve(color, overrides), default_bg),
                    (false, color) => p.resolve(color, overrides),
                };
                let mut bg = p.resolve(cell.bg, overrides);
                if flags.contains(Flags::INVERSE) {
                    std::mem::swap(&mut fg, &mut bg);
                }
                if flags.contains(Flags::HIDDEN) {
                    fg = bg;
                }
                if selection.as_ref().is_some_and(|s| s.contains(GridPoint::new(line, Column(c)))) {
                    (fg, bg) = (p.selection_fg, p.selection_bg);
                }
                if block_cursor && cursor.is_some_and(|(cc, cr, _)| cc == c && cr == r) {
                    fg = bg;
                    bg = cursor_color;
                }
                let ul = cell.underline_color().map(|u| p.resolve(u, overrides)).unwrap_or(fg);
                if let Some(zw) = cell.zerowidth() {
                    zerowidth.push((cells.len(), zw.to_vec()));
                }
                cells.push(SCell { c: cell.c, fg, bg, ul, flags });
            }
        }
        drop(term);
        #[cfg(feature = "shots")]
        if let Some(b) = &self.bench {
            b.borrow_mut().lock(got - asked, got.elapsed());
        }
        Snap { cols, rows, cells, zerowidth, cursor, cursor_color, default_bg, default_fg }
    }

    fn build(&self, s: &Snap, bounds: Bounds<Pixels>, m: Metrics, focused: bool, window: &mut Window) -> PaintState {
        let scale = m.scale;
        let ox = bounds.origin.x.as_f32();
        let oy = bounds.origin.y.as_f32();
        let cw = m.cell_w.as_f32();
        let lh = m.line_h.as_f32();
        let x_at = |col: usize| snap(ox + col as f32 * cw, scale);
        let y_at = |row: usize| snap(oy + row as f32 * lh, scale);
        let baseline = ((lh - (m.ascent + m.descent).as_f32()) / 2.0 + m.ascent.as_f32()).max(0.0);
        let thin = 1.0 / scale;

        let regular = font(FAMILY);
        let fonts = [
            regular.clone(),
            Font { weight: FontWeight::BOLD, ..regular.clone() },
            Font { style: FontStyle::Italic, ..regular.clone() },
            Font { weight: FontWeight::BOLD, style: FontStyle::Italic, ..regular.clone() },
        ];
        let black = gpui::black();

        let mut st = PaintState {
            font_size: m.font_size,
            default_bg: hsla(s.default_bg),
            quads: Vec::new(),
            shapes: Vec::new(),
            glyphs: Vec::new(),
            wavy: Vec::new(),
            cursor: Vec::new(),
            marked: None,
            line_h: m.line_h,
        };
        let mut zw_iter = s.zerowidth.iter().peekable();
        let mut text = String::new();
        let mut runs: Vec<TextRun> = Vec::new();
        // (byte offset in `text`, column, color) of each shaped cell.
        let mut starts: Vec<(usize, usize, Hsla)> = Vec::new();
        let ts = window.text_system().clone();

        for r in 0..s.rows {
            let y0 = y_at(r);
            let y1 = y_at(r + 1);
            let row = &s.cells[r * s.cols..(r + 1) * s.cols];

            // Backgrounds, one quad per run of equal color.
            let mut c = 0;
            while c < s.cols {
                let bg = row[c].bg;
                let mut e = c + 1;
                while e < s.cols && row[e].bg == bg {
                    e += 1;
                }
                if bg != s.default_bg {
                    st.quads.push((Bounds::from_corners(point(px(x_at(c)), px(y0)), point(px(x_at(e)), px(y1))), hsla(bg)));
                }
                c = e;
            }

            // Glyphs, decorations, box drawing.
            text.clear();
            runs.clear();
            starts.clear();
            // (start col, end col, color, kind): 1 underline, 2 double, 3 curly, 4 strikeout.
            let mut deco: Option<(usize, usize, Rgb24, u8)> = None;
            let flush = |deco: &mut Option<(usize, usize, Rgb24, u8)>, st: &mut PaintState| {
                if let Some((a, b, color, kind)) = deco.take() {
                    let (x0, x1) = (x_at(a), x_at(b));
                    let under = snap(y0 + baseline + 2.0, scale).min(y1 - thin);
                    let line = |y: f32| (Bounds::new(point(px(x0), px(y)), size(px(x1 - x0), px(thin))), hsla(color));
                    match kind {
                        1 => st.quads.push(line(under)),
                        2 => {
                            st.quads.push(line(under - 2.0 * thin));
                            st.quads.push(line(under));
                        }
                        3 => st.wavy.push((point(px(x0), px(under - thin)), px(x1 - x0), hsla(color))),
                        _ => st.quads.push(line(snap(y0 + baseline - m.ascent.as_f32() * 0.3, scale))),
                    }
                }
            };
            for (col, cell) in row.iter().enumerate() {
                let idx = r * s.cols + col;
                let f = cell.flags;
                // Underline and strikethrough, merged per run of equal kind and color.
                let kind = if f.contains(Flags::UNDERCURL) {
                    3
                } else if f.contains(Flags::DOUBLE_UNDERLINE) {
                    2
                } else if f.intersects(Flags::ALL_UNDERLINES) {
                    1
                } else {
                    0
                };
                for (k, color) in [(kind, cell.ul), (if f.contains(Flags::STRIKEOUT) { 4 } else { 0 }, cell.fg)] {
                    if k == 0 {
                        continue;
                    }
                    let width = if f.contains(Flags::WIDE_CHAR) { 2 } else { 1 };
                    match &mut deco {
                        Some((_, end, dc, dk)) if *end == col && *dc == color && *dk == k => *end = col + width,
                        _ => {
                            flush(&mut deco, &mut st);
                            deco = Some((col, col + width, color, k));
                        }
                    }
                }
                if f.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                    continue;
                }
                let ch = cell.c;
                while zw_iter.peek().is_some_and(|(i, _)| *i < idx) {
                    zw_iter.next();
                }
                let zw = zw_iter.peek().filter(|(i, _)| *i == idx).map(|(_, z)| z.as_slice());
                if (ch == ' ' || ch == '\0' || f.contains(Flags::HIDDEN)) && zw.is_none() {
                    continue;
                }
                let span = if f.contains(Flags::WIDE_CHAR) { 2 } else { 1 };
                if boxdraw::handles(ch) {
                    let mut prims = Vec::new();
                    boxdraw::draw(ch, x_at(col), y0, x_at(col + span), y1, scale, &mut prims);
                    let color = hsla(cell.fg);
                    st.shapes.extend(prims.into_iter().map(|p| (p, color)));
                    continue;
                }
                let style = f.contains(Flags::BOLD) as usize + 2 * f.contains(Flags::ITALIC) as usize;
                let start = text.len();
                text.push(if ch == '\0' { ' ' } else { ch });
                if let Some(zw) = zw {
                    text.extend(zw.iter());
                }
                let len = text.len() - start;
                match runs.last_mut() {
                    Some(run) if run.font == fonts[style] => run.len += len,
                    _ => runs.push(TextRun {
                        len,
                        font: fonts[style].clone(),
                        color: black,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    }),
                }
                starts.push((start, col, hsla(cell.fg)));
            }
            flush(&mut deco, &mut st);

            if !text.is_empty() {
                let layout = ts.layout_line(&text, m.font_size, &runs, None);
                let mut base_x: Vec<Option<f32>> = vec![None; starts.len()];
                for run in &layout.runs {
                    for g in &run.glyphs {
                        // The cell whose text this glyph comes from.
                        let k = match starts.binary_search_by(|(off, _, _)| off.cmp(&g.index)) {
                            Ok(k) => k,
                            Err(0) => 0,
                            Err(k) => k - 1,
                        };
                        let (_, col, color) = starts[k];
                        let gx = g.position.x.as_f32();
                        let dx = match base_x[k] {
                            Some(b) => gx - b,
                            None => {
                                base_x[k] = Some(gx);
                                0.0
                            }
                        };
                        st.glyphs.push(GlyphOp {
                            origin: point(px(ox + col as f32 * cw + dx), px(y0 + baseline + g.position.y.as_f32())),
                            font_id: run.font_id,
                            id: g.id,
                            color,
                            emoji: g.is_emoji,
                        });
                    }
                }
            }
        }

        // The cursor: steady, never blinking. A focused block is drawn by the
        // cells' colors above; unfocused it is hollow; the rest are drawn over
        // the cell.
        if let Some((col, row, shape)) = s.cursor {
            let wide = s.cells[row * s.cols + col].flags.contains(Flags::WIDE_CHAR);
            let x0 = x_at(col);
            let x1 = x_at(col + if wide { 2 } else { 1 });
            let (y0, y1) = (y_at(row), y_at(row + 1));
            let color = hsla(s.cursor_color);
            let cell = Bounds::from_corners(point(px(x0), px(y0)), point(px(x1), px(y1)));
            if self.marked.is_none() {
                match shape {
                    CursorShape::Block if focused => {}
                    CursorShape::Block | CursorShape::HollowBlock => st.cursor.push(outline(cell, color, BorderStyle::Solid)),
                    CursorShape::Beam => st.cursor.push(fill(Bounds::new(cell.origin, size(px(2.0 * thin), cell.size.height)), color)),
                    CursorShape::Underline => {
                        st.cursor.push(fill(Bounds::new(point(px(x0), px(y1 - 2.0 * thin)), size(px(x1 - x0), px(2.0 * thin))), color))
                    }
                    CursorShape::Hidden => {}
                }
            }
            // IME composition, inline at the cursor and underlined.
            if let Some(marked) = self.marked.as_ref().filter(|t| !t.is_empty()) {
                let fg = hsla(s.default_fg);
                let run = TextRun {
                    len: marked.len(),
                    font: font(FAMILY),
                    color: fg,
                    background_color: None,
                    underline: Some(UnderlineStyle { thickness: px(1.), color: Some(fg), wavy: false }),
                    strikethrough: None,
                };
                let line = ts.shape_line(marked.clone().into(), m.font_size, &[run], None);
                let cells = marked.width().max(1);
                let width = line.width.max(px(cells as f32 * cw));
                let cover = Bounds::new(point(px(x0), px(y0)), size(width, px(y1 - y0)));
                st.marked = Some((line, point(px(x0), px(y0)), cover));
            }
        }
        st
    }
}

fn dim_named(n: NamedColor) -> NamedColor {
    match n {
        NamedColor::Black => NamedColor::DimBlack,
        NamedColor::Red => NamedColor::DimRed,
        NamedColor::Green => NamedColor::DimGreen,
        NamedColor::Yellow => NamedColor::DimYellow,
        NamedColor::Blue => NamedColor::DimBlue,
        NamedColor::Magenta => NamedColor::DimMagenta,
        NamedColor::Cyan => NamedColor::DimCyan,
        _ => NamedColor::DimWhite,
    }
}
