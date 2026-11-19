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
//! Until the session's start has answered, the sink holds what comes: a
//! reattach parses the backlog first and the live output after it
//! (`Parser::attach`).

use super::colors::TermPalette;
use crate::host::OutputSink;
use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config, Osc52, Term};
use alacritty_terminal::vte::ansi::Processor;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

/// Bytes parsed per hold of the Term's lock.
pub const PARSE_CHUNK: usize = 4096;

/// Lines kept above the screen. The CLIs spend most of their time on the
/// alternate screen, which keeps none; this is for the stretches on the normal
/// screen (Codex's history, a shell's output), and the core keeps 2 MB more.
pub const SCROLLBACK: usize = 5000;

pub type Shared = Arc<FairMutex<Term<Listener>>>;

/// What the parser tells the pane, on its thread.
pub enum UiMsg {
    /// New output parsed: draw.
    Wake,
    /// The terminal's own: replies to the program's queries, its title.
    Term(Event),
}

/// The Term's events to the pane. While `quiet`, replies to queries are
/// dropped: those in a replayed backlog were answered when they were new.
#[derive(Clone)]
pub struct Listener {
    tx: UnboundedSender<UiMsg>,
    quiet: Arc<AtomicBool>,
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let reply = matches!(event, Event::PtyWrite(_) | Event::ColorRequest(..) | Event::TextAreaSizeRequest(_));
        if reply && self.quiet.load(Ordering::Acquire) {
            return;
        }
        let _ = self.tx.unbounded_send(UiMsg::Term(event));
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
    /// Output, parsed with the program's queries answered.
    Live(Vec<u8>),
    /// Output from before (a backlog) or the pane's own notices: parsed with
    /// no query answered.
    Quiet(Vec<u8>),
    /// Runs on the parser thread after everything before it, for a test or
    /// the bench to know where the parse is.
    #[cfg_attr(not(any(test, feature = "shots")), allow(dead_code))]
    Mark(Box<dyn FnOnce() + Send>),
}

/// The pane's end of its parser.
pub struct Parser {
    pub term: Shared,
    jobs: Sender<Job>,
    sink: Arc<Sink>,
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
}

struct SinkState {
    jobs: Option<Sender<Job>>,
    /// Output held until the start answers; None once it has.
    held: Option<Vec<Vec<u8>>>,
}

impl OutputSink for Sink {
    fn bytes(&self, data: &[u8]) -> bool {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(held) = s.held.as_mut() {
            held.push(data.to_vec());
            return s.jobs.is_some();
        }
        match &s.jobs {
            Some(jobs) => jobs.send(Job::Live(data.to_vec())).is_ok(),
            None => false,
        }
    }
}

impl Parser {
    /// A Term and the thread that parses into it, and the pane's end of the
    /// channel the thread tells it on.
    pub fn start(cols: usize, rows: usize) -> (Parser, UnboundedReceiver<UiMsg>) {
        let (tx, rx) = unbounded();
        let quiet = Arc::new(AtomicBool::new(false));
        let listener = Listener { tx: tx.clone(), quiet: quiet.clone() };
        let term = new_term(listener, cols, rows);
        let (jobs, work) = channel();
        let woken = Arc::new(AtomicBool::new(false));
        let sink = Arc::new(Sink { state: Mutex::new(SinkState { jobs: Some(jobs.clone()), held: None }) });
        let (thread_term, thread_woken) = (term.clone(), woken.clone());
        let on_parsed = OnParsed::default();
        let thread_on_parsed = on_parsed.clone();
        let spawned = std::thread::Builder::new()
            .name("luna-term-parser".into())
            .spawn(move || parse(work, thread_term, quiet, thread_woken, tx, thread_on_parsed));
        if let Err(e) = spawned {
            log::error!("the terminal's parser did not start: {e}");
        }
        (Parser { term, jobs, sink, woken, on_parsed }, rx)
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

    /// Holds the output that comes from now until `attach`, and drops any
    /// held for a start that was never answered (its core went away).
    pub fn hold(&self) {
        let mut s = self.sink.state.lock().unwrap_or_else(|e| e.into_inner());
        s.held = Some(Vec::new());
    }

    /// The session's start answered: `before` is parsed first (a backlog,
    /// with no query in it answered again, and what the pane puts after it),
    /// then whatever came in since `hold`, live, then everything after.
    pub fn attach(&self, before: Vec<u8>) {
        let mut s = self.sink.state.lock().unwrap_or_else(|e| e.into_inner());
        let held = s.held.take().unwrap_or_default();
        if !before.is_empty() {
            let _ = self.jobs.send(Job::Quiet(before));
        }
        for chunk in held {
            let _ = self.jobs.send(Job::Live(chunk));
        }
    }

    /// Text of the pane's own, parsed after what came before it: a notice
    /// in the session's output, never answered.
    pub fn notice(&self, text: &str) {
        let _ = self.jobs.send(Job::Quiet(text.as_bytes().to_vec()));
    }

    /// Output as if the session had written it (tests).
    #[cfg(test)]
    pub fn feed(&self, bytes: &[u8]) {
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

fn parse(
    work: Receiver<Job>,
    term: Shared,
    quiet: Arc<AtomicBool>,
    woken: Arc<AtomicBool>,
    tx: UnboundedSender<UiMsg>,
    on_parsed: OnParsed,
) {
    let mut parser: Processor = Processor::new();
    while let Ok(job) = work.recv() {
        let (bytes, is_quiet) = match job {
            Job::Live(b) => (b, false),
            Job::Quiet(b) => (b, true),
            Job::Mark(f) => {
                f();
                continue;
            }
        };
        quiet.store(is_quiet, Ordering::Release);
        for piece in bytes.chunks(PARSE_CHUNK) {
            let mut t = term.lock();
            parser.advance(&mut *t, piece);
            if let Some(f) = on_parsed.get() {
                f(piece.len());
            }
        }
        quiet.store(false, Ordering::Release);
        // One wake until the pane has taken it: whatever is parsed meanwhile
        // is drawn by the same frame.
        if !woken.swap(true, Ordering::AcqRel) && tx.unbounded_send(UiMsg::Wake).is_err() {
            return;
        }
    }
}

/// The colors a program's color query (OSC 4/10/11) is answered with.
pub fn answer_color(palette: &TermPalette, term: &Shared, index: usize) -> alacritty_terminal::vte::ansi::Rgb {
    palette.answer(index, term.lock().colors())
}
