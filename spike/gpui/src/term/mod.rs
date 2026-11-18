// One terminal pane on gpui: an alacritty_terminal grid fed by a local shell
// (--pty), by the synthetic burst generator (--feed burst[:SECS]) or by a
// recording (--feed-file PATH), painted by TermElement (element.rs), with key
// encoding (keys.rs), mouse reporting (mouse.rs), selection, copy/paste,
// bracketed paste and IME. The pane is its window's root view, so a frame's
// measured time starts at the root render (stats.rs).
//
//   spike.exe --term | --pty               a local shell
//             --feed burst[:SECS[xRATE]]   the generator (default 60 s, 1 frame per 16 ms tick)
//             --feed-file PATH             a recorded byte stream
//             [--seed N] [--record PATH] [--font-size PX] [--stats PATH] [--term-stats PATH]
//             [--type TEXT]   3 s in, TEXT ("\r" for Enter) is written to the
//                             PTY as typed input, in-process (no desktop input)
//
// With a feed, the JSON summary (frame times, intervals, bytes, the checksum
// check against a headless Term fed the same bytes) is written when the feed
// ends to --term-stats, or to --stats when no editor window is open (the
// editor writes its own stats there at exit); --record writes the bytes fed.
pub mod boxdraw;
pub mod colors;
pub mod element;
pub mod feed;
pub mod keys;
pub mod mouse;
pub mod source;
pub mod stats;
#[cfg(test)]
mod tests;

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use alacritty_terminal::event::Event;
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::index::{Column, Line, Point as GridPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::Processor;
use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use gpui::prelude::*;
use gpui::{
    App, Bounds, ClipboardItem, Context, EntityInputHandler, FocusHandle, Focusable, Global, KeyBinding,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollEvent, Task,
    UTF16Selection, Window, WindowBounds, WindowHandle, WindowOptions, actions, div, point, px, size,
};
use unicode_width::UnicodeWidthStr;

use element::{Geom, Metrics, TermElement};
use keys::Mods;
use source::{Feed, FeedState, Io, Shared, UiMsg};
use stats::FrameStats;

actions!(terminal, [CopyText, PasteText]);

const CONTEXT: &str = "Terminal";

/// The pane's command-line flags.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TermArgs {
    pub enabled: bool,
    pub feed: Option<Feed>,
    pub seed: u32,
    pub record: Option<PathBuf>,
    pub font_size: Option<f32>,
    pub type_text: Option<String>,
    pub stats: Option<PathBuf>,
}

impl TermArgs {
    /// Where the feed summary goes: --term-stats, else the shared --stats
    /// unless the editor window, which writes that file itself, is open.
    pub fn stats_path(&self, shared: Option<&PathBuf>, editor_open: bool) -> Option<PathBuf> {
        self.stats.clone().or_else(|| if editor_open { None } else { shared.cloned() })
    }

    /// Takes `arg` (and its value from `rest`) if it is one of the pane's.
    pub fn parse(&mut self, arg: &str, rest: &mut dyn Iterator<Item = String>) -> bool {
        match arg {
            "--term" | "--pty" => self.enabled = true,
            "--feed" => {
                self.enabled = true;
                let spec = rest.next().unwrap_or_default();
                let tail = spec.strip_prefix("burst").unwrap_or("");
                let tail = tail.strip_prefix(':').unwrap_or(tail);
                let (secs, rate) = tail.split_once('x').unwrap_or((tail, "1"));
                self.feed = Some(Feed::Burst {
                    secs: secs.parse().unwrap_or(60.0),
                    rate: rate.parse().unwrap_or(1).max(1),
                    seed: self.seed,
                });
            }
            "--feed-file" => {
                self.enabled = true;
                self.feed = rest.next().map(|p| Feed::File(p.into()));
            }
            "--seed" => {
                self.seed = rest.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                if let Some(Feed::Burst { seed, .. }) = &mut self.feed {
                    *seed = self.seed;
                }
            }
            "--record" => self.record = rest.next().map(Into::into),
            "--term-stats" => self.stats = rest.next().map(Into::into),
            "--font-size" => self.font_size = rest.next().and_then(|s| s.parse().ok()),
            "--type" => self.type_text = rest.next().map(|t| t.replace("\\r", "\r")),
            _ => return false,
        }
        true
    }
}

pub fn init(cx: &mut App) {
    #[cfg(target_os = "macos")]
    cx.bind_keys([
        KeyBinding::new("cmd-c", CopyText, Some(CONTEXT)),
        KeyBinding::new("cmd-v", PasteText, Some(CONTEXT)),
    ]);
    cx.bind_keys([
        KeyBinding::new("ctrl-shift-c", CopyText, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-v", PasteText, Some(CONTEXT)),
        KeyBinding::new("shift-insert", PasteText, Some(CONTEXT)),
    ]);
}

/// The last feed summary, for whoever writes --stats on exit.
#[derive(Default)]
struct Summary(Option<String>);
impl Global for Summary {}

pub fn summary(cx: &App) -> Option<String> {
    cx.try_global::<Summary>().and_then(|s| s.0.clone())
}

/// Opens the pane in a window of its own, the pane its root view.
pub fn open_window(
    args: &TermArgs,
    stats: Option<PathBuf>,
    bounds: Bounds<Pixels>,
    focus: bool,
    show: bool,
    cx: &mut App,
) -> anyhow::Result<WindowHandle<TermView>> {
    init(cx);
    let source = match &args.feed {
        Some(feed) => Source::Feed(feed.clone()),
        None => Source::Pty,
    };
    let opts = Options {
        font_size: px(args.font_size.unwrap_or(14.0)),
        stats,
        record: args.record.clone(),
        type_text: args.type_text.clone(),
    };
    let window = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(gpui::TitlebarOptions { title: Some("gpui spike terminal".into()), ..Default::default() }),
            focus,
            show,
            // gpui-pre caps a window that is not the active one at 30 fps
            // (33.3 ms) by default; a terminal streaming output wants every
            // vsync, and a --background window is never active.
            inactive_frame_interval: None,
            ..Default::default()
        },
        |window, cx| cx.new(|cx| TermView::new(source, opts, window, cx)),
    )?;
    Ok(window)
}

pub enum Source {
    Pty,
    Feed(Feed),
    /// No process: input goes to the given buffer (tests).
    #[cfg_attr(not(test), allow(dead_code))]
    Sink(Arc<Mutex<Vec<u8>>>),
}

#[derive(Default)]
pub struct Options {
    pub font_size: Pixels,
    pub stats: Option<PathBuf>,
    pub record: Option<PathBuf>,
    pub type_text: Option<String>,
}

struct Marked {
    text: String,
    /// The IME's selection inside `text`, in UTF-16 units.
    selected: Range<usize>,
}

pub struct TermView {
    focus: FocusHandle,
    term: Shared,
    io: Io,
    feed: Option<Arc<FeedState>>,
    feed_spec: Option<Feed>,
    stats: Rc<RefCell<FrameStats>>,
    geom: Rc<Cell<Geom>>,
    metrics: Rc<Cell<Option<Metrics>>>,
    sent_size: Rc<Cell<Option<(u16, u16, u16, u16)>>>,
    opts: Options,
    marked: Option<Marked>,
    selecting: bool,
    /// Buttons pressed while the application reports the mouse, by
    /// `button_index`, so overlapping presses release the right one.
    pressed: [bool; 3],
    last_cell: Option<(usize, usize)>,
    scroll_acc: f32,
    title: String,
    title_shown: String,
    pub exited: bool,
    _events: Task<()>,
}

impl TermView {
    pub fn new(source: Source, opts: Options, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (listener, rx) = source::channel();
        let (term, io, feed, feed_spec) = match source {
            Source::Pty => match source::spawn_pty(listener, 80, 24) {
                Ok((term, io)) => (term, io, None, None),
                Err(e) => {
                    let (l2, _) = source::channel();
                    let term = source::new_term(l2, 80, 24);
                    let msg = format!("spike: could not start the shell: {e}\r\n");
                    let mut parser: Processor = Processor::new();
                    parser.advance(&mut *term.lock(), msg.as_bytes());
                    (term, Io::Discard, None, None)
                }
            },
            Source::Feed(spec) => {
                let term = source::new_term(listener.clone(), 80, 24);
                let state = FeedState::new();
                if let Err(e) = source::spawn_feed(spec.clone(), term.clone(), listener, state.clone()) {
                    eprintln!("spike: feed: {e}");
                }
                (term, Io::Discard, Some(state), Some(spec))
            }
            Source::Sink(buf) => (source::new_term(listener, 80, 24), Io::Sink(buf), None, None),
        };
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let events = cx.spawn(async move |this, cx| Self::pump(this, rx, cx).await);
        if let Some(text) = opts.type_text.clone() {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(std::time::Duration::from_secs(3)).await;
                let _ = this.update(cx, |view, cx| view.send_input(text.into_bytes(), cx));
            })
            .detach();
        }
        TermView {
            focus,
            term,
            io,
            feed,
            feed_spec,
            stats: Rc::new(RefCell::new(FrameStats::default())),
            geom: Rc::new(Cell::new(Geom::default())),
            metrics: Rc::new(Cell::new(None)),
            sent_size: Rc::new(Cell::new(None)),
            opts,
            marked: None,
            selecting: false,
            pressed: [false; 3],
            last_cell: None,
            scroll_acc: 0.0,
            title: String::new(),
            title_shown: String::new(),
            exited: false,
            _events: events,
        }
    }

    async fn pump(this: gpui::WeakEntity<Self>, mut rx: UnboundedReceiver<UiMsg>, cx: &mut gpui::AsyncApp) {
        while let Some(first) = rx.next().await {
            let mut batch = vec![first];
            while let Ok(more) = rx.try_recv() {
                batch.push(more);
            }
            if this.update(cx, |view, cx| view.on_msgs(batch, cx)).is_err() {
                break;
            }
        }
    }

    fn on_msgs(&mut self, batch: Vec<UiMsg>, cx: &mut Context<Self>) {
        let mut redraw = false;
        for msg in batch {
            match msg {
                UiMsg::Term(Event::Wakeup) => redraw = true,
                UiMsg::Term(Event::PtyWrite(text)) => self.io.write(text.into_bytes()),
                UiMsg::Term(Event::Title(t)) => self.title = t,
                UiMsg::Term(Event::ResetTitle) => self.title.clear(),
                UiMsg::Term(Event::TextAreaSizeRequest(reply)) => {
                    self.io.write(reply(self.geom.get().window_size()).into_bytes());
                }
                UiMsg::Term(Event::ColorRequest(index, reply)) => {
                    let c = match index {
                        256 => colors::PALETTE.fg,
                        257 => colors::PALETTE.bg,
                        258 => colors::PALETTE.cursor,
                        i => colors::indexed(i.min(255) as u8),
                    };
                    let rgb = alacritty_terminal::vte::ansi::Rgb { r: (c >> 16) as u8, g: (c >> 8) as u8, b: c as u8 };
                    self.io.write(reply(rgb).into_bytes());
                }
                UiMsg::Term(Event::ChildExit(_)) | UiMsg::Term(Event::Exit) => {
                    self.exited = true;
                    redraw = true;
                }
                // OSC 52 and the rest are left alone: the spike does not let a
                // program read or set the clipboard.
                UiMsg::Term(_) => {}
                // A moment for the frame that shows the last bytes, so output
                // still unshown at the end is counted, not missed.
                UiMsg::FeedDone => {
                    cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(std::time::Duration::from_millis(200)).await;
                        let _ = this.update(cx, |view, cx| view.finish_feed(cx));
                    })
                    .detach();
                }
            }
        }
        if redraw {
            crate::diag::NOTIFIES.fetch_add(1, Ordering::Relaxed);
            cx.notify();
        }
    }

    /// Bytes for the PTY, from the keyboard, a paste, the IME or the mouse:
    /// the view jumps back to the bottom and a selection is dropped.
    fn send_input(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        {
            let mut term = self.term.lock();
            if term.grid().display_offset() != 0 {
                term.scroll_display(Scroll::Bottom);
            }
            term.selection = None;
        }
        self.io.write(bytes);
        cx.notify();
    }

    fn mode(&self) -> TermMode {
        *self.term.lock().mode()
    }

    fn key_down(&mut self, ev: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        if ks.modifiers.platform || ks.modifiers.function {
            return;
        }
        let mods = Mods { shift: ks.modifiers.shift, alt: ks.modifiers.alt, ctrl: ks.modifiers.control };
        // Shift+PageUp/PageDown page through the scrollback, never the PTY.
        if mods == (Mods { shift: true, ..Mods::NONE }) && matches!(ks.key.as_str(), "pageup" | "pagedown") {
            let scroll = if ks.key == "pageup" { Scroll::PageUp } else { Scroll::PageDown };
            self.term.lock().scroll_display(scroll);
            cx.stop_propagation();
            cx.notify();
            return;
        }
        let mode = self.mode();
        let modes = keys::Modes {
            app_cursor: mode.contains(TermMode::APP_CURSOR),
            app_keypad: mode.contains(TermMode::APP_KEYPAD),
            newline: mode.contains(TermMode::LINE_FEED_NEW_LINE),
        };
        let key = keys::Key {
            key: &ks.key,
            key_char: ks.key_char.as_deref(),
            mods,
            prefer_text: ev.prefer_character_input,
        };
        if let Some(bytes) = keys::encode(&key, modes) {
            self.send_input(bytes, cx);
            cx.stop_propagation();
        }
    }

    fn copy(&mut self, _: &CopyText, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.term.lock().selection_to_string().filter(|t| !t.is_empty()) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn paste(&mut self, _: &PasteText, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let bracketed = self.mode().contains(TermMode::BRACKETED_PASTE);
        self.send_input(keys::paste(&text, bracketed), cx);
    }

    /// The viewport cell under `pos` (column, row) and which half of it.
    fn cell_at(&self, pos: Point<Pixels>) -> Option<(usize, usize, Side)> {
        let g = self.geom.get();
        if g.cols == 0 || g.rows == 0 {
            return None;
        }
        let x = ((pos.x - g.origin.x) / g.cell_w).max(0.0);
        let y = ((pos.y - g.origin.y) / g.line_h).max(0.0);
        let col = (x as usize).min(g.cols - 1);
        let row = (y as usize).min(g.rows - 1);
        let side = if x.fract() < 0.5 { Side::Left } else { Side::Right };
        Some((col, row, side))
    }

    fn grid_point(&self, col: usize, row: usize) -> GridPoint {
        let offset = self.term.lock().grid().display_offset() as i32;
        GridPoint::new(Line(row as i32 - offset), Column(col))
    }

    fn report(&mut self, button: mouse::Button, action: mouse::Action, mods: Mods, col: usize, row: usize) {
        if let Some(bytes) = mouse::encode(self.mode(), button, action, mods, col, row) {
            self.io.write(bytes);
        }
    }

    fn mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let Some((col, row, side)) = self.cell_at(ev.position) else { return };
        let mods = mods_of(&ev.modifiers);
        // Shift bypasses the application's mouse mode, for selecting.
        if mouse::reporting(self.mode()) && !ev.modifiers.shift {
            let Some((i, button)) = button_index(ev.button) else { return };
            self.report(button, mouse::Action::Press, mods, col, row);
            self.pressed[i] = true;
            self.last_cell = Some((col, row));
            return;
        }
        if ev.button == MouseButton::Left {
            let ty = match ev.click_count {
                2 => SelectionType::Semantic,
                n if n >= 3 => SelectionType::Lines,
                _ => SelectionType::Simple,
            };
            let point = self.grid_point(col, row);
            self.term.lock().selection = Some(Selection::new(ty, point, side));
            self.selecting = true;
            cx.notify();
        }
    }

    fn mouse_move(&mut self, ev: &MouseMoveEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some((col, row, side)) = self.cell_at(ev.position) else { return };
        let mode = self.mode();
        if mouse::reporting(mode) && !ev.modifiers.shift {
            // Motion reports the first button held (xterm's order), or none.
            let held = [mouse::Button::Left, mouse::Button::Middle, mouse::Button::Right]
                .into_iter()
                .zip(self.pressed)
                .find_map(|(b, down)| down.then_some(b));
            if self.last_cell != Some((col, row)) && mouse::wants(mode, mouse::Action::Motion, held.is_some()) {
                let button = held.unwrap_or(mouse::Button::None);
                self.report(button, mouse::Action::Motion, mods_of(&ev.modifiers), col, row);
                self.last_cell = Some((col, row));
            }
            return;
        }
        if self.selecting && ev.pressed_button == Some(MouseButton::Left) {
            let point = self.grid_point(col, row);
            if let Some(sel) = self.term.lock().selection.as_mut() {
                sel.update(point, side);
            }
            cx.notify();
        }
    }

    fn mouse_up(&mut self, ev: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some((i, button)) = button_index(ev.button).filter(|&(i, _)| self.pressed[i]) {
            self.pressed[i] = false;
            // Released outside the grid: report it at the last cell reported,
            // or the application is left with a button held down. (Mouse mode
            // turned off since the press: encode drops what was not asked for.)
            let cell = self.cell_at(ev.position).map(|(col, row, _)| (col, row)).or(self.last_cell);
            if let Some((col, row)) = cell {
                self.report(button, mouse::Action::Release, mods_of(&ev.modifiers), col, row);
            }
            return;
        }
        if ev.button != MouseButton::Left {
            return;
        }
        if self.selecting {
            self.selecting = false;
            let mut term = self.term.lock();
            if term.selection.as_ref().is_some_and(|s| s.is_empty()) {
                term.selection = None;
            }
            cx.notify();
        }
    }

    fn scroll_wheel(&mut self, ev: &ScrollEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let g = self.geom.get();
        if g.line_h <= px(0.) {
            return;
        }
        self.scroll_acc += ev.delta.pixel_delta(g.line_h).y / g.line_h;
        let lines = self.scroll_acc.trunc() as i32;
        self.scroll_acc -= lines as f32;
        if lines == 0 {
            return;
        }
        let mode = self.mode();
        let shift = ev.modifiers.shift;
        if mouse::reporting(mode) && !shift {
            if let Some((col, row, _)) = self.cell_at(ev.position) {
                let button = if lines > 0 { mouse::Button::WheelUp } else { mouse::Button::WheelDown };
                for _ in 0..lines.unsigned_abs() {
                    self.report(button, mouse::Action::Press, mods_of(&ev.modifiers), col, row);
                }
            }
        } else if !shift && mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
            // A full-screen program without mouse mode gets the wheel as arrows.
            let app = mode.contains(TermMode::APP_CURSOR);
            let key = match (lines > 0, app) {
                (true, true) => "\x1bOA",
                (true, false) => "\x1b[A",
                (false, true) => "\x1bOB",
                (false, false) => "\x1b[B",
            };
            self.io.write(key.repeat(lines.unsigned_abs() as usize).into_bytes());
        } else {
            self.term.lock().scroll_display(Scroll::Delta(lines));
            cx.notify();
        }
    }

    /// The feed is over: checks the grid against a headless Term fed the same
    /// bytes and writes the summary (off the UI thread; the frames that count
    /// were taken while the feed ran).
    fn finish_feed(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.feed.clone() else { return };
        let term = self.term.clone();
        let stats_path = self.opts.stats.clone();
        let record = self.opts.record.clone();
        let spec = self.feed_spec.clone();
        let (frames, intervals, latency, wait, hold, all_frames) = {
            let mut s = self.stats.borrow_mut();
            s.recording = false;
            (
                std::mem::take(&mut s.frame_ms),
                std::mem::take(&mut s.interval_ms),
                std::mem::take(&mut s.latency_ms),
                std::mem::take(&mut s.lock_wait_ms),
                std::mem::take(&mut s.lock_hold_ms),
                s.all_frames,
            )
        };
        let late = latency.iter().filter(|&&l| l > stats::VSYNC_MS).count();
        // And against one vsync of the display itself (164 Hz here: 6.1 ms).
        let vsync_hz = crate::win::refresh_hz().unwrap_or(60.0);
        let late_vsync = latency.iter().filter(|&&l| l > 1000.0 / vsync_hz).count();
        let unshown = state.first_pending_ns.load(Ordering::Acquire) != 0;
        let g = self.geom.get();
        cx.spawn(async move |_this, cx| {
            let json = cx
                .background_executor()
                .spawn(async move {
                    let ui = source::checksum(&*term.lock());
                    let bytes = std::mem::take(&mut *state.bytes.lock().unwrap());
                    let resizes = state.resizes.lock().unwrap().clone();
                    let reference = source::checksum(&source::replay(&bytes, &resizes));
                    if let Some(path) = &record {
                        if let Err(e) = std::fs::write(path, &bytes) {
                            eprintln!("spike: --record {}: {e}", path.display());
                        }
                    }
                    let generated = state.generated.load(Ordering::Acquire);
                    let parsed = state.parsed.load(Ordering::Acquire);
                    let source = match &spec {
                        Some(Feed::Burst { secs, rate, seed }) => {
                            format!("{{\"kind\":\"burst\",\"secs\":{secs},\"rate\":{rate},\"seed\":{seed}}}")
                        }
                        Some(Feed::File(p)) => format!("{{\"kind\":\"file\",\"path\":{}}}", stats::json_str(&p.display().to_string())),
                        None => "null".into(),
                    };
                    format!(
                        concat!(
                            "{{\"source\":{},\"feed_secs\":{:.3},\"grid\":{{\"cols\":{},\"rows\":{},\"cell_w\":{:.3},\"line_h\":{:.3}}},",
                            "\"resizes\":{},\"bursts\":{},\"truecolor_bursts\":{},\"feeder_tick_ms\":{},",
                            "\"frames\":{},\"frames_total\":{},\"frame_ms\":{},\"interval_ms\":{},\"latency_ms\":{},",
                            "\"late_frames\":{},\"display_hz\":{:.1},\"late_frames_display_vsync\":{},\"output_unshown_at_end\":{},",
                            "\"ui_lock_wait_ms\":{},\"ui_lock_hold_ms\":{},\"feeder_lock_wait_ms\":{},\"feeder_lock_hold_ms\":{},\"parse_chunk\":{},",
                            "\"bytes\":{{\"generated\":{},\"parsed\":{},\"equal\":{}}},",
                            "\"checksum\":{{\"ui_text\":\"{:016x}\",\"ref_text\":\"{:016x}\",\"ui_cells\":\"{:016x}\",\"ref_cells\":\"{:016x}\",\"lines\":{},\"ref_lines\":{},\"match\":{}}},",
                            "\"measured\":{}}}"
                        ),
                        source,
                        *state.feed_secs.lock().unwrap(),
                        g.cols,
                        g.rows,
                        g.cell_w.as_f32(),
                        g.line_h.as_f32(),
                        resizes.len().saturating_sub(1),
                        state.bursts.load(Ordering::Relaxed),
                        state.truecolor_bursts.load(Ordering::Relaxed),
                        stats::dist_json(&state.tick_ms.lock().unwrap(), &[17.0, 20.0]),
                        frames.len(),
                        all_frames,
                        stats::dist_json(&frames, &[8.0, 16.0]),
                        stats::dist_json(&intervals, &[17.0, 20.0, 33.4]),
                        stats::dist_json(&latency, &[16.7, 33.4]),
                        late,
                        vsync_hz,
                        late_vsync,
                        unshown,
                        stats::dist_json(&wait, &[1.0, 4.0]),
                        stats::dist_json(&hold, &[1.0, 4.0]),
                        stats::dist_json(&state.lock_wait_ms.lock().unwrap(), &[1.0, 4.0]),
                        stats::dist_json(&state.lock_hold_ms.lock().unwrap(), &[1.0, 4.0]),
                        source::PARSE_CHUNK,
                        generated,
                        parsed,
                        generated == parsed,
                        ui.text,
                        reference.text,
                        ui.cells,
                        reference.cells,
                        ui.lines,
                        reference.lines,
                        ui == reference,
                        stats::json_str(
                            "frame_ms: wall time on the UI thread (Instant, so time the thread was preempted counts) \
                             from the start of the window root's render (TermView::render) to the end of TermElement::paint, for every frame while the feed ran; it does \
                             NOT include gpui's scene submission or the GPU present after paint. latency_ms: from the end of \
                             the parse of the oldest bytes a frame shows to that frame's render start (also not the present). \
                             late_frames: frames whose latency is over one vsync at 60 Hz (16.667 ms), i.e. output waited through a \
                             vsync with no render started for it; late_frames_display_vsync: the same against one vsync of the \
                             primary display (display_hz). output_unshown_at_end: parsed bytes no frame had shown 200 ms \
                             after the feed ended: the signal that nothing was held back. interval_ms: render start to render start, counted when the frame's bytes \
                             were parsed within one vsync of the previous render start. max is exact (to the microsecond), \
                             p99/p50 nearest rank. ui_lock_*: the frame's one Term lock in prepaint (wait to acquire, hold); \
                             feeder_lock_*: per parse_chunk bytes. checksum: FNV-1a of scrollback+screen text and of every \
                             cell (char, colors, flags), the pane's Term vs a headless Term fed the recorded bytes with the \
                             same resizes: both parse the same bytes, so it checks the pane's resize handling and its final \
                             state (scrollback limited to the last 10000 lines), not what each frame showed; bytes.equal \
                             (generated == parsed) holds by construction of the feed"
                        ),
                    )
                })
                .await;
            if let Some(path) = &stats_path {
                if let Err(e) = std::fs::write(path, format!("{json}\n")) {
                    eprintln!("spike: --stats {}: {e}", path.display());
                }
            }
            let _ = cx.update(|cx| cx.set_global(Summary(Some(json))));
        })
        .detach();
    }

    /// Parses `bytes` into the Term as if the program had written them (tests).
    #[cfg(test)]
    pub(crate) fn feed_bytes(&self, bytes: &[u8], cx: &mut Context<Self>) {
        let mut parser: Processor = Processor::new();
        parser.advance(&mut *self.term.lock(), bytes);
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn marked_text(&self) -> Option<&str> {
        self.marked.as_ref().map(|m| m.text.as_str())
    }
}

fn button_index(b: MouseButton) -> Option<(usize, mouse::Button)> {
    match b {
        MouseButton::Left => Some((0, mouse::Button::Left)),
        MouseButton::Middle => Some((1, mouse::Button::Middle)),
        MouseButton::Right => Some((2, mouse::Button::Right)),
        _ => None,
    }
}

fn mods_of(m: &gpui::Modifiers) -> Mods {
    Mods { shift: m.shift, alt: m.alt, ctrl: m.control }
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// The byte offset of the first character that starts at or after UTF-16
/// offset `u` in `s` (an offset inside a surrogate pair rounds forward).
fn utf8_at(s: &str, u: usize) -> usize {
    let mut n = 0;
    for (i, c) in s.char_indices() {
        if n >= u {
            return i;
        }
        n += c.len_utf16();
    }
    s.len()
}

/// The byte offset of the last character that starts at or before UTF-16
/// offset `u` (an offset inside a surrogate pair rounds back).
fn utf8_floor(s: &str, u: usize) -> usize {
    let mut n = 0;
    for (i, c) in s.char_indices() {
        n += c.len_utf16();
        if u < n {
            return i;
        }
    }
    s.len()
}

/// The UTF-16 offset of byte offset `b` (a character boundary) in `s`.
fn utf16_of(s: &str, b: usize) -> usize {
    s[..b].encode_utf16().count()
}

impl Focusable for TermView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TermView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.stats.borrow_mut().render_start(Instant::now());
        crate::idle::count(&crate::idle::TERM_RENDERS);
        let title = match (self.exited, self.title.is_empty()) {
            (true, _) => format!("{} [exited]", if self.title.is_empty() { "gpui spike terminal" } else { &self.title }),
            (false, true) => "gpui spike terminal".to_string(),
            (false, false) => self.title.clone(),
        };
        if title != self.title_shown {
            window.set_window_title(&title);
            self.title_shown = title;
        }
        div()
            .id("terminal")
            .size_full()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste))
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Middle, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Right, cx.listener(Self::mouse_up))
            .on_scroll(cx.listener(Self::scroll_wheel))
            .child(TermElement {
                view: cx.entity(),
                term: self.term.clone(),
                io: self.io.clone(),
                feed: self.feed.clone(),
                stats: self.stats.clone(),
                geom: self.geom.clone(),
                metrics: self.metrics.clone(),
                focus: self.focus.clone(),
                marked: self.marked.as_ref().map(|m| m.text.clone()),
                font_size: self.opts.font_size,
                sent: self.sent_size.clone(),
            })
    }
}

impl Drop for TermView {
    fn drop(&mut self) {
        self.io.shutdown();
    }
}

// The IME. Composition (marked) text lives only here, drawn at the cursor; it
// reaches the PTY when the IME commits it (replace_text_in_range) or when the
// platform ends the composition keeping it (unmark_text). Plain typing
// arrives the same way, as committed text.
impl EntityInputHandler for TermView {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let m = self.marked.as_ref()?;
        let len = utf16_len(&m.text);
        // A boundary inside a surrogate pair widens the range to the whole
        // character, and `adjusted` reports the range actually returned.
        let start = utf8_floor(&m.text, range.start.min(len));
        let end = utf8_at(&m.text, range.end.min(len)).max(start);
        *adjusted = Some(utf16_of(&m.text, start)..utf16_of(&m.text, end));
        Some(m.text[start..end].to_string())
    }

    fn selected_text_range(&mut self, _ignore_disabled: bool, _window: &mut Window, _cx: &mut Context<Self>) -> Option<UTF16Selection> {
        Some(UTF16Selection { range: self.marked.as_ref().map(|m| m.selected.clone()).unwrap_or(0..0), reversed: false })
    }

    fn marked_text_range(&self, _window: &mut Window, _cx: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|m| 0..utf16_len(&m.text))
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(m) = self.marked.take() {
            if !m.text.is_empty() {
                self.send_input(m.text.into_bytes(), cx);
            }
        }
        cx.notify();
    }

    fn replace_text_in_range(&mut self, _range: Option<Range<usize>>, text: &str, _window: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        if !text.is_empty() {
            self.send_input(text.as_bytes().to_vec(), cx);
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        new_text: &str,
        new_selected: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = (!new_text.is_empty()).then(|| {
            let len = utf16_len(new_text);
            let selected = new_selected.map(|r| r.start.min(len)..r.end.min(len)).unwrap_or(len..len);
            Marked { text: new_text.to_string(), selected }
        });
        cx.notify();
    }

    /// The cursor's cell, moved right by the width of the composition before
    /// `range.start`, so the candidate window sits at the text being composed.
    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let g = self.geom.get();
        let (col, row) = g.cursor.unwrap_or((0, 0));
        let before = self.marked.as_ref().map(|m| m.text[..utf8_at(&m.text, range.start)].width()).unwrap_or(0);
        // Kept on the grid: a composition running past the right edge would put
        // the candidate window off it.
        let col = (col + before).min(g.cols.saturating_sub(1));
        let origin = point(g.origin.x + g.cell_w * col as f32, g.origin.y + g.line_h * row as f32);
        Some(Bounds::new(origin, size(g.cell_w, g.line_h)))
    }

    fn character_index_for_point(&mut self, _point: Point<Pixels>, _window: &mut Window, _cx: &mut Context<Self>) -> Option<usize> {
        None
    }
}
