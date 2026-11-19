//! The pane's parser: a thread of its own that parses the session's output
//! into the pane's `Term`, and the `OutputSink` the host hands the output to
//! (host.rs), from the thread that reads it off the core's line.
//!
//! The `Term` sits behind alacritty's FairMutex. The parser takes it for at
//! most `PARSE_CHUNK` bytes at a time and lets go between chunks; the window's
//! thread takes it only to resize and to copy the viewport out (element.rs),
//! so a burst of output costs a frame a wait of one chunk's parse at most,
//! never the parsing itself. Once it has parsed, the parser wakes the pane,
//! once until the pane has drawn: an idle session wakes nothing.
//!
//! Until the session's start has answered, the sink holds what comes. A core
//! that numbers its output (each chunk's text offset) answers with the offset
//! its backlog ends at: the pane parses the backlog, then what was held from
//! that offset on, cut there, and what comes after, nothing twice and nothing
//! missed (`Parser::attach_at`); a chunk that starts past where the backlog
//! ended means output was missed in between, and the pane starts over
//! (`UiMsg::Gap`). From an older core, a reattach drops what was held and
//! parses the backlog in its place (`Parser::attach`).
//!
//! Nothing here grows without bound, whatever the program writes (`Budget`):
//! output held or queued for the parser past `OUTPUT_MAX` bytes, and replies
//! waiting for the window's thread past `REPLIES_MAX`, are dropped, and the
//! pane says so in one line once the parser has caught up. An OSC payload is
//! cut at `OSC_MAX` bytes before the parser buffers it (`OscCap`). A
//! synchronized update (mode 2026) is drawn when it ends, at its deadline as
//! alacritty's own event loop has it, or once it has buffered `SYNC_MAX`
//! bytes, whichever comes first, so an unfinished frame never holds back what
//! comes after it, nor is a long one parsed under one hold of the lock.

use super::colors::TermPalette;
use crate::host::OutputSink;
use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config, Osc52, Term};
use alacritty_terminal::vte::ansi::Processor;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Bytes parsed per hold of the Term's lock.
pub const PARSE_CHUNK: usize = 4096;

/// Lines kept above the screen. The CLIs spend most of their time on the
/// alternate screen, which keeps none; this is for the stretches on the normal
/// screen (Codex's history, a shell's output), and the core keeps 2 MB more.
pub const SCROLLBACK: usize = 5000;

/// The most output, in bytes, held or queued for the parser at once: many
/// times what a CLI writes in a second. Past it, output is dropped. The tests
/// reach a smaller one.
pub const OUTPUT_MAX: usize = if cfg!(test) { 1 << 20 } else { 16 << 20 };

/// The most bytes of replies to the program waiting for the window's thread.
pub const REPLIES_MAX: usize = 1 << 20;

/// What a reply that is not text yet (a color, the text area's size) counts
/// against `REPLIES_MAX`.
const REPLY_COST: usize = 64;

/// The longest OSC payload passed on: a title, a link, a color are far
/// shorter, and the program may set no clipboard (OSC 52).
pub const OSC_MAX: usize = 64 << 10;

/// The most a synchronized update buffers before it is drawn anyway: all of
/// it is parsed under one hold of the lock when it ends.
pub const SYNC_MAX: usize = 256 << 10;

pub type Shared = Arc<FairMutex<Term<Listener>>>;

/// What the parser tells the pane, on its thread.
pub enum UiMsg {
    /// New output parsed: draw.
    Wake,
    /// A reply to the program's query, and what it counts against
    /// `REPLIES_MAX` until the pane has taken it (`Parser::took_reply`).
    Reply(Event, usize),
    /// Output was missed after the backlog of the attach `Parser::hold`
    /// numbered so: what comes is held, not parsed, until the pane starts
    /// over.
    Gap(u64),
}

/// What is held or queued, and what was dropped since the pane last said so.
#[derive(Default)]
struct Budget {
    output: AtomicUsize,
    output_dropped: AtomicUsize,
    replies: AtomicUsize,
    replies_dropped: AtomicUsize,
}

impl Budget {
    /// Takes `n` bytes out of `used` if they fit under `max`; else counts
    /// them into `dropped`.
    fn take(used: &AtomicUsize, dropped: &AtomicUsize, n: usize, max: usize) -> bool {
        let fits = used.try_update(Ordering::AcqRel, Ordering::Acquire, |u| (u + n <= max).then_some(u + n)).is_ok();
        if !fits {
            dropped.fetch_add(n, Ordering::AcqRel);
        }
        fits
    }

    fn release(used: &AtomicUsize, n: usize) {
        let _ = used.try_update(Ordering::AcqRel, Ordering::Acquire, |u| Some(u.saturating_sub(n)));
    }

    /// The pane's line about what was dropped since the last one, if anything was.
    fn said(&self) -> Option<String> {
        let output = self.output_dropped.swap(0, Ordering::AcqRel);
        let replies = self.replies_dropped.swap(0, Ordering::AcqRel);
        let mut what = Vec::new();
        if output > 0 {
            what.push(format!("{} KB of output", output.div_ceil(1024)));
        }
        if replies > 0 {
            what.push(format!("{} KB of replies to the program", replies.div_ceil(1024)));
        }
        (!what.is_empty()).then(|| format!("\r\n\x1b[31m[dropped {}: the pane fell behind]\x1b[0m\r\n", what.join(" and ")))
    }
}

/// The Term's events to the pane: only the replies to the program's queries
/// (the title is the core's to read, the program sets no clipboard, nothing
/// rings or blinks). While `quiet`, they are dropped: those in a replayed
/// backlog were answered when they were new.
#[derive(Clone)]
pub struct Listener {
    tx: UnboundedSender<UiMsg>,
    quiet: Arc<AtomicBool>,
    budget: Arc<Budget>,
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let cost = match &event {
            Event::PtyWrite(text) => text.len(),
            Event::ColorRequest(..) | Event::TextAreaSizeRequest(_) => REPLY_COST,
            _ => return,
        };
        if self.quiet.load(Ordering::Acquire) || !Budget::take(&self.budget.replies, &self.budget.replies_dropped, cost, REPLIES_MAX) {
            return;
        }
        let _ = self.tx.unbounded_send(UiMsg::Reply(event, cost));
    }
}

/// A new Term of `cols` x `rows`, its scrollback `SCROLLBACK` lines, no
/// clipboard access for the program (OSC 52).
pub fn new_term(listener: Listener, cols: usize, rows: usize) -> Shared {
    let config = Config { scrolling_history: SCROLLBACK, osc52: Osc52::Disabled, ..Config::default() };
    Arc::new(FairMutex::new(Term::new(config, &TermSize::new(cols, rows), listener)))
}

/// One piece of work for the parser thread.
enum Job {
    /// Output, parsed with the program's queries answered; its bytes count
    /// against `OUTPUT_MAX` until parsed.
    Live(Vec<u8>),
    /// Output from before (a backlog) or the pane's own notices: parsed with
    /// no query answered, after any synchronized update the live output left
    /// open is drawn, and with one it leaves open drawn too, still quiet.
    Quiet(Vec<u8>),
    /// Runs on the parser thread after everything before it, for the pane, a
    /// test or the bench to know where the parse is.
    Mark(Box<dyn FnOnce() + Send>),
}

/// The pane's end of its parser.
pub struct Parser {
    pub term: Shared,
    jobs: Sender<Job>,
    sink: Arc<Sink>,
    budget: Arc<Budget>,
    /// Set from the pane's thread when it took a wake: the next parse wakes
    /// it again.
    woken: Arc<AtomicBool>,
    /// Told the length of every chunk parsed, under the Term's lock: the
    /// bench's (bench.rs), set by nothing else.
    #[cfg_attr(not(feature = "shots"), allow(dead_code))]
    on_parsed: OnParsed,
}

type OnParsed = Arc<std::sync::OnceLock<Box<dyn Fn(usize) + Send + Sync>>>;

/// What the host hands the session's output to.
pub struct Sink {
    state: Mutex<SinkState>,
    budget: Arc<Budget>,
    /// The pane's end, told of a gap (`UiMsg::Gap`).
    tx: UnboundedSender<UiMsg>,
}

struct SinkState {
    jobs: Option<Sender<Job>>,
    /// Output held until the start answers, with its offsets; None once it
    /// has.
    held: Option<Vec<Held>>,
    /// After an attach at an offset (`Parser::attach_at`), until output
    /// reaches past it: where the backlog ended. Output before it is in the
    /// backlog; output that starts after it means some was missed.
    end: Option<u64>,
    /// Counts the holds (`Parser::hold`), for a gap to say which attach it
    /// broke.
    round: u64,
    /// Where the next chunk starts, by the last one's offset: the core sends
    /// its output with none missing and none twice, so a chunk elsewhere is
    /// said in the log.
    next: Option<u64>,
}

/// A chunk of output held, and where its text starts in the session's, if
/// the core said.
struct Held {
    data: Vec<u8>,
    offset: Option<u64>,
}

/// Where a chunk of `len` bytes at offset `at` stands against a backlog
/// that ended at `end`.
#[derive(Debug, PartialEq, Eq)]
enum Cut {
    /// All of it is in the backlog.
    Before,
    /// It goes on from the backlog's end: from this byte of it.
    From(usize),
    /// It starts past the backlog's end: what came in between was missed.
    Gap,
}

fn cut(end: u64, at: u64, len: usize) -> Cut {
    if at > end {
        Cut::Gap
    } else if at + len as u64 <= end {
        Cut::Before
    } else {
        Cut::From((end - at) as usize)
    }
}

impl OutputSink for Sink {
    fn bytes(&self, data: &[u8], offset: Option<u64>) -> bool {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(jobs) = s.jobs.clone() else { return false };
        if let Some(at) = offset {
            if let Some(next) = s.next.filter(|n| *n != at) {
                let what = if at > next { "missing" } else { "twice" };
                log::warn!("terminal output: a chunk at {at} where {next} was next, some of it {what}");
            }
            s.next = Some(at + data.len() as u64);
        }
        if !Budget::take(&self.budget.output, &self.budget.output_dropped, data.len(), OUTPUT_MAX) {
            return true;
        }
        if let Some(held) = s.held.as_mut() {
            held.push(Held { data: data.to_vec(), offset });
            return true;
        }
        let from = match (s.end, offset) {
            (None, _) => 0,
            (Some(_), None) => {
                s.end = None;
                0
            }
            (Some(end), Some(at)) => match cut(end, at, data.len()) {
                Cut::Before => {
                    Budget::release(&self.budget.output, data.len());
                    return true;
                }
                Cut::From(i) => {
                    s.end = None;
                    Budget::release(&self.budget.output, i);
                    i
                }
                // Nothing more is parsed until the pane starts over.
                Cut::Gap => {
                    Budget::release(&self.budget.output, data.len());
                    s.end = None;
                    s.held = Some(Vec::new());
                    let _ = self.tx.unbounded_send(UiMsg::Gap(s.round));
                    return true;
                }
            },
        };
        jobs.send(Job::Live(data[from..].to_vec())).is_ok()
    }
}

impl Parser {
    /// A Term and the thread that parses into it, and the pane's end of the
    /// channel the thread tells it on.
    pub fn start(cols: usize, rows: usize) -> (Parser, UnboundedReceiver<UiMsg>) {
        let (tx, rx) = unbounded();
        let quiet = Arc::new(AtomicBool::new(false));
        let budget = Arc::new(Budget::default());
        let listener = Listener { tx: tx.clone(), quiet: quiet.clone(), budget: budget.clone() };
        let term = new_term(listener, cols, rows);
        let (jobs, work) = channel();
        let woken = Arc::new(AtomicBool::new(false));
        let state = SinkState { jobs: Some(jobs.clone()), held: None, end: None, round: 0, next: None };
        let sink = Arc::new(Sink { state: Mutex::new(state), budget: budget.clone(), tx: tx.clone() });
        let thread = Thread { term: term.clone(), quiet, woken: woken.clone(), tx, on_parsed: OnParsed::default(), budget: budget.clone() };
        let on_parsed = thread.on_parsed.clone();
        let spawned = std::thread::Builder::new().name("luna-term-parser".into()).spawn(move || thread.run(work));
        if let Err(e) = spawned {
            log::error!("the terminal's parser did not start: {e}");
        }
        (Parser { term, jobs, sink, budget, woken, on_parsed }, rx)
    }

    /// The bench's probe, told of every parse from now on.
    #[cfg(feature = "shots")]
    pub fn set_probe(&self, probe: Arc<super::bench::Probe>) {
        let _ = self.on_parsed.set(Box::new(move |n| probe.parsed(n)));
    }

    pub fn sink(&self) -> Arc<Sink> {
        self.sink.clone()
    }

    /// The pane took a wake and is about to draw what was parsed.
    pub fn took_wake(&self) {
        self.woken.store(false, Ordering::Release);
    }

    /// The pane took a reply of `cost` (`UiMsg::Reply`).
    pub fn took_reply(&self, cost: usize) {
        Budget::release(&self.budget.replies, cost);
    }

    /// Holds the output that comes from now until `attach` or `attach_at`,
    /// and drops any held for a start that was never answered (its core
    /// went away): the attach's number.
    pub fn hold(&self) -> u64 {
        let mut s = self.sink.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(held) = s.held.replace(Vec::new()) {
            Budget::release(&self.budget.output, held.iter().map(|h| h.data.len()).sum());
        }
        s.end = None;
        s.next = None;
        s.round += 1;
        s.round
    }

    /// The session's start answered with a backlog that ends at text offset
    /// `end`: `before` is parsed first, quiet (the backlog, with no query in
    /// it answered again, and what the pane puts after it), then `live` (the
    /// backlog's last escape, left unfinished, so the rest of it finds it
    /// whole), then what was held from `end` on, cut there, and everything
    /// after. False, with nothing parsed and output still held, when what
    /// was held starts past `end`: some was missed, and the pane starts over.
    pub fn attach_at(&self, before: Vec<u8>, live: Vec<u8>, end: u64) -> bool {
        let mut s = self.sink.state.lock().unwrap_or_else(|e| e.into_inner());
        let held = s.held.take().unwrap_or_default();
        let total: usize = held.iter().map(|h| h.data.len()).sum();
        let (mut kept, mut dropped, mut boundary) = (Vec::new(), 0, Some(end));
        for Held { mut data, offset } in held {
            match (boundary, offset) {
                (None, _) => kept.push(data),
                (Some(_), None) => {
                    boundary = None;
                    kept.push(data);
                }
                (Some(end), Some(at)) => match cut(end, at, data.len()) {
                    Cut::Before => dropped += data.len(),
                    Cut::From(i) => {
                        boundary = None;
                        dropped += i;
                        data.drain(..i);
                        kept.push(data);
                    }
                    Cut::Gap => {
                        Budget::release(&self.budget.output, total);
                        s.held = Some(Vec::new());
                        return false;
                    }
                },
            }
        }
        Budget::release(&self.budget.output, dropped);
        log::info!("terminal attached at {end}: {dropped} bytes held were in the backlog, {} chunks go after it", kept.len());
        s.end = boundary;
        if !before.is_empty() {
            let _ = self.jobs.send(Job::Quiet(before));
        }
        if !live.is_empty() {
            self.budget.output.fetch_add(live.len(), Ordering::AcqRel);
            let _ = self.jobs.send(Job::Live(live));
        }
        for chunk in kept {
            let _ = self.jobs.send(Job::Live(chunk));
        }
        true
    }

    /// The session's start answered: `before` is parsed first (a backlog,
    /// with no query in it answered again, and what the pane puts after it),
    /// then, if `keep_held`, whatever came in since `hold`, live, then
    /// everything after. A reattach does not keep it: some of it was sent
    /// before the backlog was taken, and is in it already, and the modes the
    /// pane puts back after the backlog may be newer than the rest; the CLI
    /// paints its screen again after (mod.rs `nudge_repaint`).
    pub fn attach(&self, before: Vec<u8>, keep_held: bool) {
        let mut s = self.sink.state.lock().unwrap_or_else(|e| e.into_inner());
        let held = s.held.take().unwrap_or_default();
        if !before.is_empty() {
            let _ = self.jobs.send(Job::Quiet(before));
        }
        if keep_held {
            for chunk in held {
                let _ = self.jobs.send(Job::Live(chunk.data));
            }
        } else {
            Budget::release(&self.budget.output, held.iter().map(|h| h.data.len()).sum());
        }
    }

    /// Resolves once everything sent to the parser so far is parsed.
    pub fn parsed(&self) -> futures::channel::oneshot::Receiver<()> {
        let (tx, rx) = futures::channel::oneshot::channel();
        let _ = self.jobs.send(Job::Mark(Box::new(move || {
            let _ = tx.send(());
        })));
        rx
    }

    /// Text of the pane's own, parsed after what came before it: a notice
    /// in the session's output, never answered.
    pub fn notice(&self, text: &str) {
        let _ = self.jobs.send(Job::Quiet(text.as_bytes().to_vec()));
    }

    /// Output as if the session had written it (tests).
    #[cfg(test)]
    pub fn feed(&self, bytes: &[u8]) {
        self.budget.output.fetch_add(bytes.len(), Ordering::AcqRel);
        let _ = self.jobs.send(Job::Live(bytes.to_vec()));
    }

    /// Calls `f` on the parser thread once everything sent before is parsed.
    #[cfg(any(test, feature = "shots"))]
    pub fn mark(&self, f: impl FnOnce() + Send + 'static) {
        let _ = self.jobs.send(Job::Mark(Box::new(f)));
    }

    /// Blocks until everything sent so far is parsed (tests, the shots).
    #[cfg(any(test, feature = "shots"))]
    pub fn settle(&self) {
        let (tx, rx) = channel();
        self.mark(move || {
            let _ = tx.send(());
        });
        let _ = rx.recv();
    }
}

impl Drop for Parser {
    /// The pane is gone: the host lets go of the sink on its next chunk, and
    /// the thread ends once the last sender has.
    fn drop(&mut self) {
        let mut s = self.sink.state.lock().unwrap_or_else(|e| e.into_inner());
        s.jobs = None;
        s.held = None;
    }
}

/// The parser thread's own.
struct Thread {
    term: Shared,
    quiet: Arc<AtomicBool>,
    woken: Arc<AtomicBool>,
    tx: UnboundedSender<UiMsg>,
    on_parsed: OnParsed,
    budget: Arc<Budget>,
}

impl Thread {
    fn run(self, work: Receiver<Job>) {
        let mut parser: Processor = Processor::new();
        let mut osc = OscCap::default();
        loop {
            // A synchronized update past its deadline is drawn as it is,
            // however busy the session keeps the parser.
            if parser.sync_timeout().sync_timeout().is_some_and(|d| Instant::now() >= d) {
                self.end_sync(&mut parser);
                if !self.wake() {
                    return;
                }
            }
            let job = match work.try_recv() {
                Ok(job) => job,
                Err(TryRecvError::Disconnected) => return,
                // Caught up: what was dropped is said, and the parser waits,
                // until the open synchronized update's deadline at most.
                Err(TryRecvError::Empty) => match (self.budget.said(), parser.sync_timeout().sync_timeout()) {
                    (Some(said), _) => Job::Quiet(said.into_bytes()),
                    (None, Some(deadline)) => match work.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                        Ok(job) => job,
                        Err(RecvTimeoutError::Timeout) => continue,
                        Err(RecvTimeoutError::Disconnected) => return,
                    },
                    (None, None) => match work.recv() {
                        Ok(job) => job,
                        Err(_) => return,
                    },
                },
            };
            let woke = match job {
                Job::Live(b) => {
                    Budget::release(&self.budget.output, b.len());
                    !self.parse(&mut parser, &mut osc, &b, false) || self.wake()
                }
                Job::Quiet(b) => self.quiet(&mut parser, &mut osc, &b),
                // Whoever waits for the parse to get here sees what was
                // dropped before it said too.
                Job::Mark(f) => {
                    let woke = match self.budget.said() {
                        Some(said) => self.quiet(&mut parser, &mut osc, said.as_bytes()),
                        None => true,
                    };
                    f();
                    woke
                }
            };
            if !woke {
                return;
            }
        }
    }

    /// Parses a backlog or a notice: after what the live output left in a
    /// synchronized update is drawn, as live, and with any it leaves open
    /// drawn too, still quiet, before any live byte. False once the pane is
    /// gone.
    fn quiet(&self, parser: &mut Processor, osc: &mut OscCap, bytes: &[u8]) -> bool {
        self.end_sync(parser);
        self.parse(parser, osc, bytes, true);
        self.end_sync_as(parser, true);
        self.wake()
    }

    /// Parses `bytes`, `PARSE_CHUNK` at a time, each under its own hold of
    /// the lock; true when some of it was not buffered by a synchronized
    /// update, and so is there to draw.
    fn parse(&self, parser: &mut Processor, osc: &mut OscCap, bytes: &[u8], quiet: bool) -> bool {
        self.quiet.store(quiet, Ordering::Release);
        let mut shown = false;
        for piece in bytes.chunks(PARSE_CHUNK) {
            let piece = osc.cap(piece);
            let before = parser.sync_bytes_count();
            let mut t = self.term.lock();
            parser.advance(&mut *t, &piece);
            if parser.sync_bytes_count() >= SYNC_MAX {
                parser.stop_sync(&mut *t);
            }
            drop(t);
            shown |= parser.sync_bytes_count() < before + piece.len();
            if let Some(f) = self.on_parsed.get() {
                f(piece.len());
            }
        }
        self.quiet.store(false, Ordering::Release);
        shown
    }

    /// Draws an open synchronized update now.
    fn end_sync(&self, parser: &mut Processor) {
        self.end_sync_as(parser, false);
    }

    fn end_sync_as(&self, parser: &mut Processor, quiet: bool) {
        if parser.sync_timeout().sync_timeout().is_none() {
            return;
        }
        self.quiet.store(quiet, Ordering::Release);
        parser.stop_sync(&mut *self.term.lock());
        self.quiet.store(false, Ordering::Release);
    }

    /// Wakes the pane, once until it has taken the wake: whatever is parsed
    /// meanwhile is drawn by the same frame. False once the pane is gone.
    fn wake(&self) -> bool {
        self.woken.swap(true, Ordering::AcqRel) || self.tx.unbounded_send(UiMsg::Wake).is_ok()
    }
}

/// Cuts every OSC payload (ESC ] ... BEL or ST) to `OSC_MAX` bytes before the
/// parser sees it: vte keeps a payload whole until it ends, and a program
/// could make one as long as it liked. Kept across chunks; the terminator
/// always goes through, so the sequence still ends where it did.
#[derive(Default)]
struct OscCap {
    state: OscState,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum OscState {
    #[default]
    Ground,
    Esc,
    /// Inside a payload, this many bytes of it passed on.
    Osc(usize),
}

impl OscCap {
    fn cap<'a>(&mut self, input: &'a [u8]) -> Cow<'a, [u8]> {
        let mut out: Option<Vec<u8>> = None;
        for (i, &b) in input.iter().enumerate() {
            let keep = self.step(b);
            match (&mut out, keep) {
                (Some(o), true) => o.push(b),
                (None, false) => out = Some(input[..i].to_vec()),
                _ => {}
            }
        }
        out.map_or(Cow::Borrowed(input), Cow::Owned)
    }

    /// Takes one byte; false when it is dropped.
    fn step(&mut self, b: u8) -> bool {
        self.state = match (self.state, b) {
            (_, 0x1b) => OscState::Esc,
            (OscState::Esc, b']') => OscState::Osc(0),
            (OscState::Osc(_), 0x07 | 0x18 | 0x1a) => OscState::Ground,
            (OscState::Osc(n), _) if n >= OSC_MAX => return false,
            (OscState::Osc(n), _) => OscState::Osc(n + 1),
            (OscState::Esc | OscState::Ground, _) => OscState::Ground,
        };
        true
    }
}

/// Where an escape sequence that `bytes` leaves unfinished starts (its ESC),
/// or `bytes.len()` when every one is finished: a backlog taken between two
/// reads of the CLI's output can end inside one, whose rest then comes in the
/// next chunk. Unfinished: a lone ESC, a CSI with no final byte yet, an OSC
/// with no BEL yet, a DCS, SOS, PM or APC (each ended only by ST, whose ESC
/// would be the last), and ESC with intermediates and no final byte. CAN and
/// SUB cancel one.
pub fn unfinished_escape(bytes: &[u8]) -> usize {
    let Some(i) = bytes.iter().rposition(|&b| b == 0x1b) else { return bytes.len() };
    let rest = &bytes[i + 1..];
    if rest.iter().any(|&b| matches!(b, 0x18 | 0x1a)) {
        return bytes.len();
    }
    let finished = match rest.first() {
        None => false,
        Some(b'[') => rest[1..].iter().any(|b| (0x40..=0x7e).contains(b)),
        Some(b']') => rest.contains(&0x07),
        Some(b'P' | b'X' | b'^' | b'_') => false,
        Some(0x20..=0x2f) => rest[1..].iter().any(|b| (0x30..=0x7e).contains(b)),
        Some(_) => true,
    };
    if finished { bytes.len() } else { i }
}

/// The colors a program's color query (OSC 4/10/11) is answered with.
pub fn answer_color(palette: &TermPalette, term: &Shared, index: usize) -> alacritty_terminal::vte::ansi::Rgb {
    palette.answer(index, term.lock().colors())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_osc_payload_is_cut_and_still_ends() {
        let mut cap = OscCap::default();
        let long = format!("\x1b]0;{}\x07after", "t".repeat(OSC_MAX * 2));
        // Split across chunks as the session's output comes.
        let mut out = Vec::new();
        for piece in long.as_bytes().chunks(1000) {
            out.extend_from_slice(&cap.cap(piece));
        }
        assert_eq!(out.len(), 2 + OSC_MAX + 1 + 5);
        assert!(out.ends_with(b"\x07after"));
        // Anything short passes as it is, borrowed.
        assert!(matches!(cap.cap(b"\x1b]8;;http://x\x1b\\link\x1b[1m"), Cow::Borrowed(_)));
    }

    #[test]
    fn a_chunk_is_placed_against_the_backlogs_end() {
        assert_eq!(cut(10, 0, 4), Cut::Before);
        assert_eq!(cut(10, 6, 4), Cut::Before, "ends where the backlog does");
        assert_eq!(cut(10, 8, 4), Cut::From(2));
        assert_eq!(cut(10, 10, 4), Cut::From(0));
        assert_eq!(cut(10, 11, 4), Cut::Gap);
    }

    #[test]
    fn an_escape_left_unfinished_is_found_where_it_starts() {
        for (bytes, at) in [
            (&b"text"[..], 4),
            (b"a\x1b[1m", 5),
            (b"a\x1b[?20", 1),
            (b"a\x1b", 1),
            (b"a\x1b]0;tit", 1),
            (b"a\x1b]0;title\x07b", 12),
            (b"a\x1b]0;title\x1b\\", 12),
            (b"a\x1bP1$r", 1),
            (b"a\x1b(B", 4),
            (b"a\x1b(", 1),
            (b"a\x1bc", 3),
            (b"a\x1b[12\x18", 6),
        ] {
            assert_eq!(unfinished_escape(bytes), at, "{:?}", String::from_utf8_lossy(bytes));
        }
    }

    #[test]
    fn output_past_the_budget_is_dropped_and_said() {
        let budget = Budget::default();
        assert!(Budget::take(&budget.output, &budget.output_dropped, OUTPUT_MAX, OUTPUT_MAX));
        assert!(!Budget::take(&budget.output, &budget.output_dropped, 2048, OUTPUT_MAX));
        Budget::release(&budget.output, OUTPUT_MAX);
        assert!(Budget::take(&budget.output, &budget.output_dropped, 10, OUTPUT_MAX));
        assert_eq!(budget.said().as_deref(), Some("\r\n\x1b[31m[dropped 2 KB of output: the pane fell behind]\x1b[0m\r\n"));
        assert_eq!(budget.said(), None, "said once");
    }
}
