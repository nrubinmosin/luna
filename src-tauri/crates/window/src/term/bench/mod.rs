//! The terminal's burst bench, in the shots harness only (`luna-window-shots
//! --bench burst:SECS OUT.json`): a pane in an off-screen window over a fake
//! host, fed the spike's synthetic bursts (feed.rs) through the same seam a
//! session's output takes (the host hands each chunk to the pane's sink, from
//! a thread of its own, every 16 ms), and its frames timed.
//!
//! Measured, as the GPUI spike did (its README, "Terminal pane"):
//! - `frame_ms`: wall time on the window's thread from the start of the
//!   pane's render to the end of its element's paint; not gpui's scene
//!   submission or the present after it.
//! - `latency_ms`: from the end of the parse of the oldest bytes a frame shows
//!   to that frame's render start; `late_frames` those over one vsync at
//!   60 Hz. `interval_ms`: render start to render start while output was
//!   waiting. `wake_ms`: from the same parse to the pane taking the parser's
//!   wake on the window's thread, which tells a window's thread held up
//!   elsewhere from a frame that was not asked for.
//! - `ui_lock_*`: the frame's one Term lock (wait, hold).
//! - Bytes generated and parsed, and FNV-1a checksums of the pane's Term
//!   (scrollback and screen) against a Term fed the same bytes headless,
//!   with the same resizes at the same offsets.

// The tests build it without the harness that runs it.
#![cfg_attr(not(feature = "shots"), allow(dead_code))]

pub mod feed;

#[cfg(feature = "shots")]
use super::TermPane;
use super::parser::SCROLLBACK;
#[cfg(feature = "shots")]
use crate::demo::{self, FakeHost};
use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config, Osc52, Term};
use alacritty_terminal::vte::ansi::{Color, Processor};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// One vsync at 60 Hz: the bound for a late frame.
pub const VSYNC_MS: f64 = 1000.0 / 60.0;

/// What the parser thread tells the frames: how much it parsed, and when the
/// oldest parse no frame has shown yet ended.
pub struct Probe {
    epoch: Instant,
    pub parsed: AtomicU64,
    /// Nanoseconds since `epoch`, plus one, or 0 when nothing waits.
    first_pending_ns: AtomicU64,
    /// The pane has laid out at its size: the feed may start.
    pub size_ready: AtomicBool,
}

impl Probe {
    pub fn new() -> Arc<Probe> {
        Arc::new(Probe {
            epoch: Instant::now(),
            parsed: AtomicU64::new(0),
            first_pending_ns: AtomicU64::new(0),
            size_ready: AtomicBool::new(false),
        })
    }

    /// `n` bytes parsed, under the Term's lock: stamped there, so a frame
    /// copying the Term under the same lock never shows bytes it finds no
    /// stamp for.
    pub fn parsed(&self, n: usize) {
        self.parsed.fetch_add(n as u64, Ordering::Release);
        let now = self.epoch.elapsed().as_nanos() as u64 + 1;
        let _ = self.first_pending_ns.compare_exchange(0, now, Ordering::AcqRel, Ordering::Relaxed);
    }
}

/// The frames' figures, kept by the pane's render and its element.
pub struct FrameStats {
    probe: Arc<Probe>,
    frame_start: Option<Instant>,
    prev_start: Option<Instant>,
    /// Only frames while the feed runs are kept.
    pub recording: bool,
    pub frame_ms: Vec<f64>,
    pub interval_ms: Vec<f64>,
    pub latency_ms: Vec<f64>,
    pub lock_wait_ms: Vec<f64>,
    pub lock_hold_ms: Vec<f64>,
    /// From the end of the oldest parse waiting to the pane taking the
    /// parser's wake on the window's thread.
    pub wake_ms: Vec<f64>,
    /// (bytes parsed, cols, rows) at the first size and at every resize.
    pub resizes: Vec<(u64, usize, usize)>,
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

impl FrameStats {
    pub fn new(probe: Arc<Probe>) -> FrameStats {
        FrameStats {
            probe,
            frame_start: None,
            prev_start: None,
            recording: false,
            frame_ms: Vec::new(),
            interval_ms: Vec::new(),
            latency_ms: Vec::new(),
            lock_wait_ms: Vec::new(),
            lock_hold_ms: Vec::new(),
            wake_ms: Vec::new(),
            resizes: Vec::new(),
        }
    }

    /// The pane took the parser's wake.
    pub fn woke(&mut self) {
        let pending = self.probe.first_pending_ns.load(Ordering::Acquire);
        if pending != 0 && self.recording {
            let parsed_at = self.probe.epoch + Duration::from_nanos(pending - 1);
            self.wake_ms.push(ms(Instant::now().saturating_duration_since(parsed_at)));
        }
    }

    pub fn render_start(&mut self, now: Instant) {
        self.prev_start = self.frame_start.or(self.prev_start);
        self.frame_start = Some(now);
    }

    /// The Term was resized, under its lock.
    pub fn resized(&mut self, cols: usize, rows: usize) {
        self.resizes.push((self.probe.parsed.load(Ordering::Acquire), cols, rows));
    }

    /// The frame copies the Term out, under its lock, at `cols` x `rows`.
    pub fn copying(&mut self, cols: usize, rows: usize) {
        if self.resizes.is_empty() {
            self.resizes.push((self.probe.parsed.load(Ordering::Acquire), cols, rows));
        }
        self.probe.size_ready.store(true, Ordering::Release);
        let pending = self.probe.first_pending_ns.swap(0, Ordering::AcqRel);
        let (Some(start), true, true) = (self.frame_start, pending != 0, self.recording) else { return };
        let parsed_at = self.probe.epoch + Duration::from_nanos(pending - 1);
        self.latency_ms.push(ms(start.saturating_duration_since(parsed_at)));
        if let Some(prev) = self.prev_start
            && parsed_at <= prev + Duration::from_secs_f64(VSYNC_MS / 1000.0)
        {
            self.interval_ms.push(ms(start - prev));
        }
    }

    pub fn lock(&mut self, wait: Duration, hold: Duration) {
        if self.recording {
            self.lock_wait_ms.push(ms(wait));
            self.lock_hold_ms.push(ms(hold));
        }
    }

    pub fn paint_end(&mut self, now: Instant) {
        if let Some(start) = self.frame_start {
            self.prev_start = Some(start);
            if self.recording {
                self.frame_ms.push(ms(now - start));
            }
        }
    }
}

/// max, p99, p50 (nearest rank) and how many are over each bound, as JSON.
pub fn dist_json(v: &[f64], over: &[f64]) -> serde_json::Value {
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    let q = |p: f64| if s.is_empty() { 0.0 } else { s[((s.len() - 1) as f64 * p).round() as usize] };
    let mut out = serde_json::json!({
        "count": s.len(),
        "max": s.last().copied().unwrap_or(0.0),
        "p99": q(0.99),
        "p50": q(0.5),
    });
    for &o in over {
        out[format!("over_{o}ms")] = v.iter().filter(|&&x| x > o).count().into();
    }
    out
}

/// FNV-1a over the text of every line, scrollback first (trailing blanks
/// dropped, wide-character spacers skipped, combining marks kept), and a
/// second one over each cell's character, colors and flags too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Checksum {
    pub text: u64,
    pub cells: u64,
    pub lines: usize,
}

struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 ^= x as u64;
            self.0 = self.0.wrapping_mul(0x100_0000_01b3);
        }
    }
    fn u32(&mut self, v: u32) {
        self.bytes(&v.to_le_bytes());
    }
}

fn color_code(c: Color) -> u32 {
    match c {
        Color::Named(n) => 0x1000_0000 | n as u32,
        Color::Indexed(i) => 0x2000_0000 | i as u32,
        Color::Spec(c) => 0x3000_0000 | ((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32,
    }
}

pub fn checksum<T>(term: &Term<T>) -> Checksum {
    let grid = term.grid();
    let (mut text, mut cells) = (Fnv::new(), Fnv::new());
    let (top, bottom) = (grid.topmost_line().0, grid.bottommost_line().0);
    let mut line_text = String::new();
    for l in top..=bottom {
        let row = &grid[Line(l)];
        line_text.clear();
        for c in 0..grid.columns() {
            let cell = &row[Column(c)];
            cells.u32(cell.c as u32);
            cells.u32(color_code(cell.fg));
            cells.u32(color_code(cell.bg));
            cells.u32(cell.flags.bits() as u32);
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            line_text.push(cell.c);
            if let Some(zw) = cell.zerowidth() {
                line_text.extend(zw.iter());
            }
        }
        text.bytes(line_text.trim_end_matches(' ').as_bytes());
        text.bytes(b"\n");
    }
    Checksum { text: text.0, cells: cells.0, lines: (bottom - top + 1) as usize }
}

/// A Term fed `bytes` headless with the same resizes at the same offsets, as
/// the reference for "no output dropped".
pub fn replay(bytes: &[u8], resizes: &[(u64, usize, usize)]) -> Term<VoidListener> {
    let (c0, r0) = resizes.first().map(|&(_, c, r)| (c, r)).unwrap_or((80, 24));
    let config = Config { scrolling_history: SCROLLBACK, osc52: Osc52::Disabled, ..Config::default() };
    let mut term = Term::new(config, &TermSize::new(c0, r0), VoidListener);
    let mut parser: Processor = Processor::new();
    let mut at = 0usize;
    for &(offset, cols, rows) in resizes.iter().skip(1) {
        let offset = (offset as usize).min(bytes.len());
        parser.advance(&mut term, &bytes[at..offset]);
        at = offset;
        term.resize(TermSize::new(cols, rows));
    }
    parser.advance(&mut term, &bytes[at..]);
    term
}

/// What the feeder thread hands back when it is done.
#[cfg(feature = "shots")]
struct Fed {
    bytes: Vec<u8>,
    secs: f64,
    bursts: u64,
}

/// Plays `secs` of bursts into the session's sink on a 16 ms tick, once the
/// pane has its size.
#[cfg(feature = "shots")]
fn feed(host: Arc<FakeHost>, session: String, term: super::parser::Shared, probe: Arc<Probe>, secs: f64, seed: u32) -> Fed {
    while !probe.size_ready.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(5));
    }
    let mut generator = feed::Generator::new(seed);
    let (tick, start) = (Duration::from_millis(16), Instant::now());
    let (mut next, mut last_burst, mut bytes, mut bursts) = (start, usize::MAX, Vec::new(), 0);
    loop {
        let t = start.elapsed().as_secs_f64();
        if t >= secs {
            break;
        }
        let phase = feed::phase(t);
        if phase.active {
            let mut s = String::new();
            if phase.burst != last_burst {
                last_burst = phase.burst;
                bursts += 1;
                s += generator.set_kind(phase.kind);
                generator.entropy = phase.entropy;
            }
            let (w, h) = {
                let term = term.lock();
                (term.columns(), term.screen_lines())
            };
            s += &generator.frame(w, h);
            bytes.extend_from_slice(s.as_bytes());
            host.output(&session, s.as_bytes());
        }
        next += tick;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            next = now;
        }
    }
    Fed { bytes, secs: start.elapsed().as_secs_f64(), bursts }
}

/// Runs the bench on `pane`, its session's output coming from `host`, and
/// returns the summary.
#[cfg(feature = "shots")]
pub async fn run(
    pane: gpui::Entity<TermPane>,
    host: Arc<FakeHost>,
    secs: f64,
    cx: &mut gpui::AsyncApp,
) -> Result<serde_json::Value, String> {
    let probe = Probe::new();
    let stats = std::rc::Rc::new(std::cell::RefCell::new(FrameStats::new(probe.clone())));
    let term = pane.update(cx, |p, cx| {
        p.bench = Some(stats.clone());
        p.parser().set_probe(probe.clone());
        cx.notify();
        p.parser().term.clone()
    });
    stats.borrow_mut().recording = true;
    let (session, thread_term, thread_probe) = (demo::session().id, term.clone(), probe.clone());
    let feeder = std::thread::Builder::new()
        .name("luna-bench-feed".into())
        .spawn(move || feed(host, session, thread_term, thread_probe, secs, 7))
        .map_err(|e| e.to_string())?;
    while !feeder.is_finished() {
        cx.background_executor().timer(Duration::from_millis(100)).await;
    }
    let fed = feeder.join().map_err(|_| "the feeder panicked".to_string())?;
    // Everything parsed, then a moment for the frame that shows the last of
    // it: output no frame showed by then counts as unshown.
    let (tx, rx) = futures::channel::oneshot::channel();
    pane.update(cx, |p, _| {
        p.parser().mark(move || {
            let _ = tx.send(());
        })
    });
    let _ = rx.await;
    cx.background_executor().timer(Duration::from_millis(200)).await;
    stats.borrow_mut().recording = false;
    let unshown = probe.first_pending_ns.load(Ordering::Acquire) != 0;
    let geom = pane.update(cx, |p, _| p.geom());
    let s = stats.borrow();
    let ui = checksum(&*term.lock());
    let reference = checksum(&replay(&fed.bytes, &s.resizes));
    let parsed = probe.parsed.load(Ordering::Acquire);
    let late = s.latency_ms.iter().filter(|&&l| l > VSYNC_MS).count();
    Ok(serde_json::json!({
        "feed": { "kind": "burst", "secs": fed.secs, "seed": 7, "bursts": fed.bursts },
        "grid": { "cols": geom.cols, "rows": geom.rows, "cell_w": geom.cell_w.as_f32(), "line_h": geom.line_h.as_f32(), "scale": geom.scale },
        "resizes": s.resizes.len().saturating_sub(1),
        "frames": s.frame_ms.len(),
        "frame_ms": dist_json(&s.frame_ms, &[8.0, 16.0]),
        "interval_ms": dist_json(&s.interval_ms, &[17.0, 20.0, 33.4]),
        "latency_ms": dist_json(&s.latency_ms, &[16.7, 33.4]),
        "late_frames": late,
        "wake_ms": dist_json(&s.wake_ms, &[16.7]),
        "output_unshown_at_end": unshown,
        "ui_lock_wait_ms": dist_json(&s.lock_wait_ms, &[1.0, 4.0]),
        "ui_lock_hold_ms": dist_json(&s.lock_hold_ms, &[1.0, 4.0]),
        "bytes": { "generated": fed.bytes.len(), "parsed": parsed, "equal": fed.bytes.len() as u64 == parsed },
        "checksum": {
            "ui_text": format!("{:016x}", ui.text), "ref_text": format!("{:016x}", reference.text),
            "ui_cells": format!("{:016x}", ui.cells), "ref_cells": format!("{:016x}", reference.cells),
            "lines": ui.lines, "match": ui == reference,
        },
        "measured": "frame_ms: the window's thread, from the pane's render start to its element's paint end (no scene submission, no present). \
                     latency_ms: from the end of the parse of the oldest bytes a frame shows to its render start; late_frames over 16.667 ms. \
                     wake_ms: from that parse to the pane taking the parser's wake. \
                     checksum: FNV-1a of scrollback and screen, the pane's Term against one fed the same bytes headless with the same resizes."
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_matches_a_term_fed_in_chunks() {
        let mut g = feed::Generator::new(3);
        let bytes: Vec<u8> = (0..200).flat_map(|_| g.frame(80, 24).into_bytes()).collect();
        let (parser, _rx) = super::super::parser::Parser::start(80, 24);
        for chunk in bytes.chunks(777) {
            parser.feed(chunk);
        }
        parser.settle();
        let reference = replay(&bytes, &[(0, 80, 24)]);
        assert_eq!(checksum(&*parser.term.lock()), checksum(&reference));
        // And a dropped chunk shows.
        let short = replay(&bytes[..bytes.len() - 500], &[(0, 80, 24)]);
        assert_ne!(checksum(&*parser.term.lock()).cells, checksum(&short).cells);
    }

    #[test]
    fn percentiles() {
        let v: Vec<f64> = (1..=100).map(|x| x as f64).collect();
        let d = dist_json(&v, &[16.0]);
        assert_eq!(
            (d["count"].as_u64(), d["max"].as_f64(), d["p50"].as_f64(), d["p99"].as_f64()),
            (Some(100), Some(100.0), Some(51.0), Some(99.0))
        );
        assert_eq!(d["over_16ms"], 84);
    }
}
