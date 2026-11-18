// Where the pane's bytes come from and where its input goes. Every source parses
// on a thread of its own into the shared `Term` behind alacritty's FairMutex,
// then wakes the UI; the UI thread only takes the lock to resize and to copy
// the visible cells, so a burst costs it a lock wait of at most one parse
// chunk, never the parsing itself. The feeders parse PARSE_CHUNK bytes per
// acquisition through the fair lock (the queue the UI's lock() waits in) and
// let go between chunks, however much a tick generated. The PTY goes through
// alacritty's EventLoop, which holds the lock for up to MAX_LOCKED_READ (64 KiB)
// per read batch; that is alacritty's, not changed here.
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event, EventListener, VoidListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::{Color, Processor};
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};

use super::feed::{self, Generator};

/// Bytes a feeder parses per acquisition of the Term lock.
pub const PARSE_CHUNK: usize = 4096;

pub enum UiMsg {
    Term(Event),
    FeedDone,
}

/// Sends the terminal's events to the UI thread.
#[derive(Clone)]
pub struct Listener(UnboundedSender<UiMsg>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let _ = self.0.unbounded_send(UiMsg::Term(event));
    }
}

pub type Shared = Arc<FairMutex<Term<Listener>>>;

pub fn channel() -> (Listener, UnboundedReceiver<UiMsg>) {
    let (tx, rx) = unbounded();
    (Listener(tx), rx)
}

pub fn new_term(listener: Listener, cols: usize, rows: usize) -> Shared {
    Arc::new(FairMutex::new(Term::new(Config::default(), &TermSize::new(cols, rows), listener)))
}

/// Where the pane's input bytes (keys, paste, mouse reports, the terminal's
/// own replies) go.
#[derive(Clone)]
pub enum Io {
    Pty(EventLoopSender),
    /// A fake PTY: everything written is kept, for tests.
    Sink(Arc<Mutex<Vec<u8>>>),
    /// A feeder: input has nowhere to go.
    Discard,
}

impl Io {
    pub fn write(&self, bytes: Vec<u8>) {
        match self {
            Io::Pty(tx) => {
                let _ = tx.send(Msg::Input(bytes.into()));
            }
            Io::Sink(sink) => sink.lock().unwrap().extend_from_slice(&bytes),
            Io::Discard => {}
        }
    }

    pub fn resize(&self, size: WindowSize) {
        if let Io::Pty(tx) = self {
            let _ = tx.send(Msg::Resize(size));
        }
    }

    pub fn shutdown(&self) {
        if let Io::Pty(tx) = self {
            let _ = tx.send(Msg::Shutdown);
        }
    }
}

/// A local shell on a pseudoterminal: ConPTY running `pwsh -NoLogo -NoProfile`
/// on Windows, the user's login shell on a Unix pty elsewhere.
pub fn spawn_pty(listener: Listener, cols: u16, rows: u16) -> std::io::Result<(Shared, Io)> {
    #[cfg(windows)]
    let shell = Some(tty::Shell::new("pwsh".into(), vec!["-NoLogo".into(), "-NoProfile".into()]));
    #[cfg(not(windows))]
    let shell = None;
    let options = tty::Options {
        shell,
        working_directory: None,
        drain_on_exit: true,
        env: [("TERM", "xterm-256color"), ("COLORTERM", "truecolor")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        #[cfg(windows)]
        escape_args: true,
    };
    let size = WindowSize { num_lines: rows, num_cols: cols, cell_width: 8, cell_height: 16 };
    let pty = tty::new(&options, size, 0)?;
    let term = new_term(listener.clone(), cols as usize, rows as usize);
    let event_loop = EventLoop::new(term.clone(), listener, pty, true, false)?;
    let tx = event_loop.channel();
    event_loop.spawn();
    Ok((term, Io::Pty(tx)))
}

/// What a feeder plays.
#[derive(Clone, Debug, PartialEq)]
pub enum Feed {
    /// The generator for `secs`, `rate` frames per 16 ms tick.
    Burst { secs: f64, rate: u32, seed: u32 },
    /// A recording, 16 KiB per tick.
    File(std::path::PathBuf),
}

/// The feeder's counters, shared with the UI.
pub struct FeedState {
    pub epoch: Instant,
    pub generated: AtomicU64,
    /// Bytes parsed into the shared Term; updated under its lock.
    pub parsed: AtomicU64,
    /// When (ns since `epoch`, plus one) the oldest parse the UI has not yet
    /// copied out finished; 0 when there is none.
    pub first_pending_ns: AtomicU64,
    pub size_ready: AtomicBool,
    pub done: AtomicBool,
    pub bursts: AtomicU64,
    pub truecolor_bursts: AtomicU64,
    /// (parsed offset, cols, rows) for the first size and every resize, under
    /// the Term lock, so a replay can resize at the same byte.
    pub resizes: Mutex<Vec<(u64, usize, usize)>>,
    /// Everything fed, for the reference replay and --record.
    pub bytes: Mutex<Vec<u8>>,
    pub feed_secs: Mutex<f64>,
    /// Time from one tick's start to the next's, ms.
    pub tick_ms: Mutex<Vec<f64>>,
    /// Per parse chunk: the feeder's wait for the Term lock and its hold, ms.
    pub lock_wait_ms: Mutex<Vec<f64>>,
    pub lock_hold_ms: Mutex<Vec<f64>>,
}

impl FeedState {
    pub fn new() -> Arc<Self> {
        Arc::new(FeedState {
            epoch: Instant::now(),
            generated: AtomicU64::new(0),
            parsed: AtomicU64::new(0),
            first_pending_ns: AtomicU64::new(0),
            size_ready: AtomicBool::new(false),
            done: AtomicBool::new(false),
            bursts: AtomicU64::new(0),
            truecolor_bursts: AtomicU64::new(0),
            resizes: Mutex::new(Vec::new()),
            bytes: Mutex::new(Vec::new()),
            feed_secs: Mutex::new(0.0),
            tick_ms: Mutex::new(Vec::new()),
            lock_wait_ms: Mutex::new(Vec::new()),
            lock_hold_ms: Mutex::new(Vec::new()),
        })
    }

    pub fn now_ns(&self) -> u64 {
        self.epoch.elapsed().as_nanos() as u64 + 1
    }
}

/// Starts the feeder thread. It waits for the pane's first layout (so the
/// first size is the real one), then plays on a fixed 16 ms tick.
pub fn spawn_feed(feed: Feed, term: Shared, listener: Listener, state: Arc<FeedState>) -> std::io::Result<()> {
    let recording = match &feed {
        Feed::File(path) => Some(std::fs::read(path)?),
        Feed::Burst { .. } => None,
    };
    thread::Builder::new().name("term-feed".into()).spawn(move || {
        while !state.size_ready.load(Ordering::Acquire) {
            thread::sleep(Duration::from_millis(5));
        }
        let mut parser: Processor = Processor::new();
        let mut generator = match feed {
            Feed::Burst { seed, .. } => Generator::new(seed),
            Feed::File(_) => Generator::new(0),
        };
        let tick = Duration::from_millis(16);
        let start = Instant::now();
        let mut next = start;
        let mut last_burst = usize::MAX;
        let mut offset = 0usize;
        let mut last_tick: Option<Instant> = None;
        loop {
            let tick_at = Instant::now();
            if let Some(last) = last_tick {
                let ms = (tick_at - last).as_secs_f64() * 1000.0;
                state.tick_ms.lock().unwrap().push(ms);
                crate::diag::feeder_tick(ms);
            }
            last_tick = Some(tick_at);
            let t = start.elapsed().as_secs_f64();
            let chunk: Vec<u8> = match (&feed, &recording) {
                (Feed::Burst { secs, rate, .. }, _) => {
                    if t >= *secs {
                        break;
                    }
                    let phase = feed::phase(t);
                    if !phase.active {
                        Vec::new()
                    } else {
                        let mut s = String::new();
                        if phase.burst != last_burst {
                            last_burst = phase.burst;
                            state.bursts.fetch_add(1, Ordering::Relaxed);
                            if phase.entropy >= 2.0 {
                                state.truecolor_bursts.fetch_add(1, Ordering::Relaxed);
                            }
                            s += generator.set_kind(phase.kind);
                            generator.entropy = phase.entropy;
                        }
                        let (w, h) = {
                            let term = term.lock();
                            (term.columns(), term.screen_lines())
                        };
                        for _ in 0..*rate {
                            s += &generator.frame(w, h);
                        }
                        s.into_bytes()
                    }
                }
                (Feed::File(_), Some(data)) => {
                    if offset >= data.len() {
                        break;
                    }
                    let end = (offset + 16 * 1024).min(data.len());
                    let chunk = data[offset..end].to_vec();
                    offset = end;
                    chunk
                }
                (Feed::File(_), None) => break,
            };
            if !chunk.is_empty() {
                state.generated.fetch_add(chunk.len() as u64, Ordering::Relaxed);
                state.bytes.lock().unwrap().extend_from_slice(&chunk);
                let (mut waits, mut holds) = (Vec::new(), Vec::new());
                for piece in chunk.chunks(PARSE_CHUNK) {
                    let asked = Instant::now();
                    let mut term = term.lock();
                    let got = Instant::now();
                    parser.advance(&mut *term, piece);
                    state.parsed.fetch_add(piece.len() as u64, Ordering::Release);
                    // Stamped under the lock the frame copies the Term under
                    // (element.rs takes the stamp there): stamped after it, a
                    // frame could show these bytes in between and leave a stamp
                    // for output already shown.
                    let _ = state.first_pending_ns.compare_exchange(0, state.now_ns(), Ordering::AcqRel, Ordering::Relaxed);
                    drop(term);
                    waits.push((got - asked).as_secs_f64() * 1000.0);
                    holds.push(got.elapsed().as_secs_f64() * 1000.0);
                }
                state.lock_wait_ms.lock().unwrap().extend(waits);
                state.lock_hold_ms.lock().unwrap().extend(holds);
                listener.send_event(Event::Wakeup);
            }
            next += tick;
            let now = Instant::now();
            if next > now {
                thread::sleep(next - now);
            } else {
                next = now;
            }
        }
        *state.feed_secs.lock().unwrap() = start.elapsed().as_secs_f64();
        state.done.store(true, Ordering::Release);
        let _ = listener.0.unbounded_send(UiMsg::FeedDone);
    })?;
    Ok(())
}

/// FNV-1a over the text of every line, scrollback first (trailing blanks
/// dropped, wide-char spacers skipped, combining marks kept), and a second one
/// over each cell's character, colors and flags too.
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
    let top = grid.topmost_line().0;
    let bottom = grid.bottommost_line().0;
    let cols = grid.columns();
    let mut line_text = String::new();
    for l in top..=bottom {
        let row = &grid[Line(l)];
        line_text.clear();
        for c in 0..cols {
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

/// A headless Term fed the same bytes with the same resizes at the same
/// offsets, as the reference for "no output dropped".
pub fn replay(bytes: &[u8], resizes: &[(u64, usize, usize)]) -> Term<VoidListener> {
    let (c0, r0) = resizes.first().map(|&(_, c, r)| (c, r)).unwrap_or((80, 24));
    let mut term = Term::new(Config::default(), &TermSize::new(c0, r0), VoidListener);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_matches_a_term_fed_in_chunks() {
        let mut g = Generator::new(3);
        let bytes: Vec<u8> = (0..200).flat_map(|_| g.frame(80, 24).into_bytes()).collect();
        let (listener, _rx) = channel();
        let term = new_term(listener, 80, 24);
        let mut parser: Processor = Processor::new();
        for chunk in bytes.chunks(777) {
            parser.advance(&mut *term.lock(), chunk);
        }
        let reference = replay(&bytes, &[(0, 80, 24)]);
        assert_eq!(checksum(&*term.lock()), checksum(&reference));
        // And a dropped chunk shows.
        let short = replay(&bytes[..bytes.len() - 500], &[(0, 80, 24)]);
        assert_ne!(checksum(&*term.lock()).cells, checksum(&short).cells);
    }
}
