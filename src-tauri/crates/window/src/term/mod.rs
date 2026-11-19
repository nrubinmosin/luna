//! A terminal pane: one CLI session running in Luna's core, shown and typed
//! into. The grid and the parser are alacritty_terminal's; everything around
//! them is the pane's own.
//!
//! - The session's output reaches the pane through its `OutputSink`
//!   (parser.rs), on the thread that reads the core's line, and is parsed on
//!   the pane's parser thread: never on the window's thread, never through
//!   the inbox. The parser wakes the pane to draw; an idle session draws
//!   nothing, and nothing blinks, so nothing runs on a timer.
//! - The pane starts or reattaches its session (`SessionSpec`) once the core
//!   is up, and waits, saying so, while it is found or replaced; a reattach
//!   replays the core's backlog before the live output, as the page does
//!   (`started`). Its exit and the input it refused are said in the pane.
//! - Keys (keys.rs), the mouse (mouse.rs), the IME, paste and drop
//!   (paste.rs) go to the session as `write_session`, its size as
//!   `resize_session` (io.rs); selection and scrollback stay in the pane.
//! - It paints with element.rs, in the window's theme (colors.rs).

mod boxdraw;
pub mod colors;
mod element;
mod io;
mod keys;
mod mouse;
mod parser;
pub mod paste;

#[cfg(any(test, feature = "shots"))]
pub mod bench;
#[cfg(test)]
mod tests;

use crate::dto::LinkState;
use crate::host::cmd;
use crate::models::Models;
use alacritty_terminal::event::{Event, WindowSize};
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::index::{Column, Line, Point as GridPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::TermMode;
use element::{Geom, Metrics, TermElement};
use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use gpui::prelude::*;
use gpui::{
    App, Bounds, Context, DragMoveEvent, EntityInputHandler, ExternalPaths, FocusHandle, Focusable, KeyBinding, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollEvent, SharedString, Subscription, Task, UTF16Selection, Window,
    actions, div, point, px, size,
};
use io::Io;
use keys::Mods;
use parser::{Parser, UiMsg};
use paste::{Clip, Provider, Quoting};
use serde::Deserialize;
use serde_json::{Value, json};
use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;
use unicode_width::UnicodeWidthStr;

actions!(terminal, [Copy, CopyOrInterrupt, Paste]);

/// The key context the pane's bindings live in.
const CONTEXT: &str = "Terminal";

/// The pane's text size; the font is the window's one (font.rs).
pub const FONT_SIZE: Pixels = px(14.);

/// How long the repaint nudge keeps the PTY a row shorter (the page's 60 ms).
const NUDGE: Duration = Duration::from_millis(60);

/// The keys the pane takes before the session sees them, as the page's
/// terminal has them (src/features/panes/terminal/keys.ts and the WebView's
/// own clipboard keys): Ctrl+C copies a selection and is ^C without one;
/// Ctrl+V, Ctrl+Shift+V and Shift+Insert paste; Ctrl+Shift+C and Ctrl+Insert
/// copy; on macOS Cmd+C and Cmd+V as well.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-c", CopyOrInterrupt, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-c", Copy, Some(CONTEXT)),
        KeyBinding::new("ctrl-insert", Copy, Some(CONTEXT)),
        KeyBinding::new("ctrl-v", Paste, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-v", Paste, Some(CONTEXT)),
        KeyBinding::new("shift-insert", Paste, Some(CONTEXT)),
    ]);
    #[cfg(target_os = "macos")]
    cx.bind_keys([KeyBinding::new("cmd-c", Copy, Some(CONTEXT)), KeyBinding::new("cmd-v", Paste, Some(CONTEXT))]);
}

/// The session a pane shows, and how it is started: `start` is
/// `cmd::ENSURE_CLAUDE_SESSION` or `cmd::ENSURE_CODEX_SESSION`, `args` the
/// page's arguments for it (src/features/panes/terminals.ts `startSession`).
/// Starting a session that runs already reattaches to it.
#[derive(Clone, Debug)]
pub struct SessionSpec {
    pub id: String,
    pub provider: Provider,
    pub start: &'static str,
    pub args: Value,
}

/// What a start answers (`Attached` in src/ipc/sessions.ts).
#[derive(Deserialize)]
struct Attached {
    #[serde(default)]
    backlog: String,
    #[serde(default)]
    generation: u64,
}

/// Where the pane's session stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// For the core to be up: then it starts.
    Waiting,
    /// Its start is asked and not answered yet.
    Starting,
    /// It runs; its exit carries this spawn's number.
    Live(u64),
    /// It exited, or could not start; the pane says which.
    Ended,
}

struct Marked {
    text: String,
    /// The IME's selection inside `text`, in UTF-16 units.
    selected: Range<usize>,
}

pub struct TermPane {
    models: Models,
    spec: SessionSpec,
    focus: FocusHandle,
    parser: Parser,
    io: Rc<Io>,
    geom: Rc<Cell<Geom>>,
    metrics: Rc<Cell<Option<Metrics>>>,
    phase: Phase,
    /// Counts the starts asked: an answer to an older one is dropped.
    asked: u64,
    /// It ran once in this pane: a start after that begins on a clean grid.
    ran: bool,
    /// Exits heard while the start was in flight, by spawn number.
    heard: Vec<u64>,
    marked: Option<Marked>,
    selecting: bool,
    /// Buttons pressed while the program reports the mouse, by
    /// `button_index`, so overlapping presses release the right one.
    pressed: [bool; 3],
    last_cell: Option<(usize, usize)>,
    scroll_acc: f32,
    /// Files dragged in from outside the window, while they are over the
    /// pane (`drag_moved`).
    dragging: Option<Vec<std::path::PathBuf>>,
    /// Times rendered, for the tests that hold renders to what changed.
    pub(crate) renders: usize,
    #[cfg(feature = "shots")]
    pub(crate) bench: Option<Rc<std::cell::RefCell<bench::FrameStats>>>,
    _events: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl TermPane {
    pub fn new(models: Models, spec: SessionSpec, window: &mut Window, cx: &mut Context<Self>) -> TermPane {
        let (parser, rx) = Parser::start(80, 24);
        let focus = cx.focus_handle();
        let subscriptions = vec![
            cx.observe(&models.shell, |_, _, cx| cx.notify()),
            cx.observe(&models.core, |this, _, cx| this.on_core(cx)),
            cx.on_focus_in(&focus, window, |this, _, cx| this.report_focus(true, cx)),
            cx.on_focus_out(&focus, window, |this, _, _, cx| this.report_focus(false, cx)),
        ];
        models.panes.borrow_mut().push(cx.weak_entity());
        let events = cx.spawn(async move |this, cx| pump(this, rx, cx).await);
        let io = Rc::new(Io::new(models.host.clone(), spec.id.clone()));
        let mut pane = TermPane {
            models,
            spec,
            focus,
            parser,
            io,
            geom: Rc::new(Cell::new(Geom::default())),
            metrics: Rc::new(Cell::new(None)),
            phase: Phase::Waiting,
            asked: 0,
            ran: false,
            heard: Vec::new(),
            marked: None,
            selecting: false,
            pressed: [false; 3],
            last_cell: None,
            scroll_acc: 0.0,
            dragging: None,
            renders: 0,
            #[cfg(feature = "shots")]
            bench: None,
            _events: events,
            _subscriptions: subscriptions,
        };
        pane.on_core(cx);
        pane
    }

    pub fn id(&self) -> &str {
        &self.spec.id
    }

    #[cfg(test)]
    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.focus, cx);
    }

    /// The core's line changed: a pane waiting starts its session once the
    /// core is up; one whose core went away waits for the next, and starts
    /// (resumes, or reattaches) there.
    fn on_core(&mut self, cx: &mut Context<Self>) {
        let up = self.models.core.read(cx).link().state == LinkState::Up;
        match (up, self.phase) {
            (true, Phase::Waiting) => self.start(cx),
            (false, Phase::Starting | Phase::Live(_)) => {
                self.phase = Phase::Waiting;
                self.io.set_live(false);
                cx.notify();
            }
            _ => cx.notify(),
        }
    }

    /// Asks the core for the session: started, or reattached if it runs. Its
    /// output is held from here until the answer, so a backlog comes first.
    fn start(&mut self, cx: &mut Context<Self>) {
        self.asked += 1;
        self.phase = Phase::Starting;
        self.heard.clear();
        if self.ran {
            // What the session before showed goes, as the page's new terminal
            // does without it.
            self.parser.notice("\x1bc");
        }
        self.parser.hold();
        self.models.host.route_output(&self.spec.id, Some(self.parser.sink()));
        let answer = self.models.host.call(self.spec.start, self.spec.args.clone());
        let asked = self.asked;
        cx.spawn(async move |this, cx| {
            let answer = answer.await;
            let _ = this.update(cx, |this, cx| this.started(asked, answer, cx));
        })
        .detach();
        cx.notify();
    }

    fn started(&mut self, asked: u64, answer: Result<Value, String>, cx: &mut Context<Self>) {
        if asked != self.asked || self.phase != Phase::Starting {
            return;
        }
        let attached = answer.and_then(|v| serde_json::from_value::<Attached>(v).map_err(|e| e.to_string()));
        let Attached { backlog, generation } = match attached {
            Ok(a) => a,
            // The core went away under the start: it waits for the next.
            Err(_) if self.models.core.read(cx).link().state != LinkState::Up => {
                self.phase = Phase::Waiting;
                cx.notify();
                return;
            }
            Err(e) => {
                log::warn!("{} {}: {e}", self.spec.start, self.spec.id);
                self.parser.attach(Vec::new());
                self.parser.notice(&format!("\r\n\x1b[31m[failed to start session: {e}]\x1b[0m\r\n"));
                self.phase = Phase::Ended;
                cx.notify();
                return;
            }
        };
        self.ran = true;
        self.phase = Phase::Live(generation);
        // It ended before its start had answered: what it printed, then that.
        if self.heard.contains(&generation) {
            self.parser.attach(backlog.into_bytes());
            self.exited(cx);
            return;
        }
        if backlog.is_empty() {
            // A fresh session: its PTY starts at a placeholder size, so this
            // is its first real resize, and the CLI paints itself.
            self.parser.attach(Vec::new());
            self.io.set_live(true);
            cx.notify();
            return;
        }
        self.io.set_live(true);
        self.replay(asked, backlog, cx);
    }

    /// Reattached to a running session, as the page's `replayTail` does: the
    /// backlog is a tail whose start (the switch to the alternate screen, the
    /// modes the CLI set) was trimmed long ago, so it is replayed, then wiped,
    /// the modes the core kept for the session put back, and the CLI asked to
    /// paint its screen again. The live output waits behind all that.
    fn replay(&mut self, asked: u64, backlog: String, cx: &mut Context<Self>) {
        let modes = self.models.host.call(cmd::SESSION_MODES, json!({ "id": self.spec.id }));
        cx.spawn(async move |this, cx| {
            let modes = modes.await.ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                if asked != this.asked {
                    return;
                }
                let mut before = backlog.into_bytes();
                before.extend_from_slice(b"\x1bc");
                before.extend_from_slice(modes.as_bytes());
                this.parser.attach(before);
                this.nudge_repaint(cx);
            });
        })
        .detach();
    }

    /// Makes the CLI paint its screen again: it paints on input or on a
    /// resize only, so the PTY is made a row shorter and, a moment later, the
    /// size the pane is at then (the page's `nudgeRepaint`).
    fn nudge_repaint(&mut self, cx: &mut Context<Self>) {
        let Some(size) = self.io.size() else { return };
        self.io.send_size(io::PtySize { rows: size.rows.saturating_sub(1).max(1), ..size });
        let io = self.io.clone();
        cx.spawn(async move |_, cx| {
            cx.background_executor().timer(NUDGE).await;
            if let Some(size) = io.size() {
                io.send_size(size);
            }
        })
        .detach();
    }

    /// `pty://exit` for this session.
    pub(crate) fn on_exit(&mut self, generation: u64, cx: &mut Context<Self>) {
        match self.phase {
            Phase::Starting => self.heard.push(generation),
            Phase::Live(g) if g == generation => self.exited(cx),
            _ => {}
        }
    }

    fn exited(&mut self, cx: &mut Context<Self>) {
        self.io.set_live(false);
        self.phase = Phase::Ended;
        self.parser.notice("\r\n\x1b[2m[session exited]\x1b[0m\r\n");
        cx.notify();
    }

    /// `pty://input-refused` for this session: keys it did not take were not
    /// typed, and a keystroke's error comes back to nobody, so the pane says
    /// it, as the page does.
    pub(crate) fn on_refused(&mut self, waiting_kb: u64, cx: &mut Context<Self>) {
        let said = format!("[keys not delivered: the session is not taking input, {waiting_kb} KB waiting]");
        self.parser.notice(&format!("\r\n\x1b[31m{said}\x1b[0m\r\n"));
        cx.notify();
    }

    fn on_msgs(&mut self, batch: Vec<UiMsg>, cx: &mut Context<Self>) {
        let mut redraw = false;
        for msg in batch {
            match msg {
                UiMsg::Wake => {
                    self.parser.took_wake();
                    #[cfg(feature = "shots")]
                    if let Some(b) = &self.bench {
                        b.borrow_mut().woke();
                    }
                    redraw = true;
                }
                // Answers to the program's queries: device attributes, modes,
                // the colors it paints over, the text area's size. The cursor
                // position query never gets here: the core answers it.
                UiMsg::Term(Event::PtyWrite(text)) => self.io.write(&text),
                UiMsg::Term(Event::TextAreaSizeRequest(reply)) => {
                    let g = self.geom.get();
                    let s = g.pty_size();
                    let ws = WindowSize {
                        num_lines: s.rows,
                        num_cols: s.cols,
                        cell_width: s.width / s.cols.max(1),
                        cell_height: s.height / s.rows.max(1),
                    };
                    self.io.write(&reply(ws));
                }
                UiMsg::Term(Event::ColorRequest(index, reply)) => {
                    let palette = self.palette(cx);
                    self.io.write(&reply(parser::answer_color(palette, &self.parser.term, index)));
                }
                // The title is the core's to read (Codex's thread id); the
                // program sets no clipboard; nothing rings or blinks.
                UiMsg::Term(_) => {}
            }
        }
        if redraw {
            cx.notify();
        }
    }

    fn palette(&self, cx: &App) -> &'static colors::TermPalette {
        colors::TermPalette::of(self.models.shell.read(cx).dark())
    }

    fn mode(&self) -> TermMode {
        *self.parser.term.lock().mode()
    }

    /// Input for the session, from the keyboard, a paste, the IME or the
    /// mouse: the view goes back to the bottom and a selection is dropped.
    fn send_input(&mut self, text: &str, cx: &mut Context<Self>) {
        {
            let mut term = self.parser.term.lock();
            if term.grid().display_offset() != 0 {
                term.scroll_display(Scroll::Bottom);
            }
            term.selection = None;
        }
        self.io.write(text);
        cx.notify();
    }

    /// Focus in and out, to a program that asked for them (mode 1004), each
    /// as a write of its own: the core tells such a write from a person's
    /// keys (docs/cli-compatibility.md, 2.1.325).
    fn report_focus(&mut self, focused: bool, cx: &mut Context<Self>) {
        if self.mode().contains(TermMode::FOCUS_IN_OUT) {
            self.io.write(if focused { "\x1b[I" } else { "\x1b[O" });
        }
        cx.notify();
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
            self.parser.term.lock().scroll_display(scroll);
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
        let key = keys::Key { key: &ks.key, key_char: ks.key_char.as_deref(), mods, prefer_text: ev.prefer_character_input };
        if let Some(bytes) = keys::encode(&key, modes) {
            self.send_input(&String::from_utf8_lossy(&bytes), cx);
            cx.stop_propagation();
        }
    }

    /// The selection's text, if there is any.
    fn selection(&self) -> Option<String> {
        self.parser.term.lock().selection_to_string().filter(|t| !t.is_empty())
    }

    fn copy(&mut self, _: &Copy, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.selection() {
            self.models.clipboard.write_text(text, cx);
        }
    }

    /// Ctrl+C: a selection is copied and cleared, as in any Windows terminal;
    /// with none it goes on to the session as ^C.
    fn copy_or_interrupt(&mut self, _: &CopyOrInterrupt, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = self.selection() else {
            cx.propagate();
            return;
        };
        self.models.clipboard.write_text(text, cx);
        self.parser.term.lock().selection = None;
        cx.notify();
    }

    fn paste(&mut self, _: &Paste, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(clip) = self.models.clipboard.read(cx) else { return };
        match clip {
            Clip::Text(text) => self.paste_text(&text, cx),
            Clip::Files(paths) => self.paste_paths(&paths, cx),
            // The CLI reads the clipboard itself on its own key.
            Clip::Image => {
                let key = self.spec.provider.image_key();
                self.send_input(key, cx);
            }
        }
    }

    fn paste_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let bracketed = self.mode().contains(TermMode::BRACKETED_PASTE);
        self.send_input(&keys::paste(text, bracketed), cx);
    }

    /// Files pasted or dropped: their paths, quoted, as a paste.
    fn paste_paths(&mut self, paths: &[std::path::PathBuf], cx: &mut Context<Self>) {
        if let Some(text) = paste::paths_text(paths, Quoting::HERE) {
            self.paste_text(&text, cx);
        }
    }

    /// Files dragged from outside the window moved: kept while they are
    /// over the pane, for the drop (`drag_released`). Not gpui's `on_drop`:
    /// a drop from outside counts as no mouse input, so after a key press
    /// gpui takes the pane for not hovered and drops nothing.
    fn drag_moved(&mut self, e: &DragMoveEvent<ExternalPaths>, _window: &mut Window, cx: &mut Context<Self>) {
        let over = e.bounds.contains(&e.event.position).then(|| e.drag(cx).paths().to_vec());
        if over != self.dragging {
            self.dragging = over;
            cx.notify();
        }
    }

    /// The left button came up anywhere in the window (element.rs): files
    /// dragged over the pane and let go `inside` it are typed in.
    pub(crate) fn drag_released(&mut self, inside: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(paths) = self.dragging.take() else { return };
        if inside {
            window.focus(&self.focus, cx);
            self.paste_paths(&paths, cx);
        }
        cx.notify();
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
        let offset = self.parser.term.lock().grid().display_offset() as i32;
        GridPoint::new(Line(row as i32 - offset), Column(col))
    }

    fn report(&mut self, button: mouse::Button, action: mouse::Action, mods: Mods, col: usize, row: usize) {
        if let Some(text) = mouse::encode(self.mode(), button, action, mods, col, row) {
            self.io.write(&text);
        }
    }

    fn mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let Some((col, row, side)) = self.cell_at(ev.position) else { return };
        let mods = mods_of(&ev.modifiers);
        // Shift bypasses the program's mouse mode, for selecting.
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
            self.parser.term.lock().selection = Some(Selection::new(ty, point, side));
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
            if let Some(sel) = self.parser.term.lock().selection.as_mut() {
                sel.update(point, side);
            }
            cx.notify();
        }
    }

    fn mouse_up(&mut self, ev: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some((i, button)) = button_index(ev.button).filter(|&(i, _)| self.pressed[i]) {
            self.pressed[i] = false;
            // Released outside the grid: reported at the last cell reported,
            // or the program is left with a button held down. (Mouse mode
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
            let mut term = self.parser.term.lock();
            if term.selection.as_ref().is_some_and(|s| s.is_empty()) {
                term.selection = None;
            }
            drop(term);
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
            // A full-screen program without mouse mode gets the wheel as
            // arrows (mode 1007).
            let app = mode.contains(TermMode::APP_CURSOR);
            let key = match (lines > 0, app) {
                (true, true) => "\x1bOA",
                (true, false) => "\x1b[A",
                (false, true) => "\x1bOB",
                (false, false) => "\x1b[B",
            };
            self.io.write(&key.repeat(lines.unsigned_abs() as usize));
        } else {
            self.parser.term.lock().scroll_display(Scroll::Delta(lines));
            cx.notify();
        }
    }

    /// What the pane says over its grid while it waits: never a failure.
    fn waiting(&self, cx: &App) -> Option<&'static str> {
        let core = self.models.core.read(cx);
        match (self.phase, core.link().state) {
            (Phase::Waiting, LinkState::Starting) if core.was_up() => {
                Some("Waiting for Luna's core: it is being replaced. The session resumes in the new one.")
            }
            (Phase::Waiting, LinkState::Starting) => Some("Waiting for Luna's core: the session starts once it is up."),
            (Phase::Waiting, _) => Some("Waiting for Luna's core: the session starts once it runs again."),
            (Phase::Starting, _) => Some("Starting the session..."),
            _ => None,
        }
    }

    #[cfg(any(test, feature = "shots"))]
    pub(crate) fn parser(&self) -> &Parser {
        &self.parser
    }

    #[cfg(any(test, feature = "shots"))]
    pub(crate) fn geom(&self) -> Geom {
        self.geom.get()
    }

    #[cfg(test)]
    pub(crate) fn marked_text(&self) -> Option<&str> {
        self.marked.as_ref().map(|m| m.text.as_str())
    }
}

async fn pump(this: gpui::WeakEntity<TermPane>, mut rx: UnboundedReceiver<UiMsg>, cx: &mut gpui::AsyncApp) {
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

impl Focusable for TermPane {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TermPane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        #[cfg(feature = "shots")]
        if let Some(b) = &self.bench {
            b.borrow_mut().render_start(std::time::Instant::now());
        }
        let shell = self.models.shell.read(cx).palette();
        let palette = self.palette(cx);
        let strip = self.waiting(cx).map(|said| {
            div()
                .id("term-waiting")
                .debug_selector(|| "term-waiting".into())
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .px(px(8.))
                .py(px(2.))
                .bg(shell.info)
                .text_color(shell.on_status)
                .border_b_1()
                .border_color(shell.frame)
                .child(SharedString::from(said))
        });
        let frame = if self.dragging.is_some() { shell.attention } else { colors::hsla(palette.bg) };
        div()
            .id("terminal")
            .debug_selector(|| "terminal".into())
            .relative()
            .size_full()
            .p(px(4.))
            .bg(colors::hsla(palette.bg))
            .border_2()
            .border_color(frame)
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::copy_or_interrupt))
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
            // Files from outside the window; a drag of the window's own
            // things (a chat, a pane) is not taken here.
            .on_drag_move(cx.listener(Self::drag_moved))
            .child(TermElement {
                view: cx.entity(),
                term: self.parser.term.clone(),
                io: self.io.clone(),
                geom: self.geom.clone(),
                metrics: self.metrics.clone(),
                focus: self.focus.clone(),
                marked: self.marked.as_ref().map(|m| m.text.clone()),
                font_size: FONT_SIZE,
                palette,
                #[cfg(feature = "shots")]
                bench: self.bench.clone(),
            })
            .children(strip)
    }
}

// The IME. Composition (marked) text lives only here, drawn at the cursor; it
// reaches the session when the IME commits it (replace_text_in_range) or when
// the platform ends the composition keeping it (unmark_text). Plain typing
// arrives the same way, as committed text.
impl EntityInputHandler for TermPane {
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
        if let Some(m) = self.marked.take()
            && !m.text.is_empty()
        {
            self.send_input(&m.text, cx);
        }
        cx.notify();
    }

    fn replace_text_in_range(&mut self, _range: Option<Range<usize>>, text: &str, _window: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        if !text.is_empty() {
            self.send_input(text, cx);
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
        // Kept on the grid: a composition running past the right edge would
        // put the candidate window off it.
        let col = (col + before).min(g.cols.saturating_sub(1));
        let origin = point(g.origin.x + g.cell_w * col as f32, g.origin.y + g.line_h * row as f32);
        Some(Bounds::new(origin, size(g.cell_w, g.line_h)))
    }

    fn character_index_for_point(&mut self, _point: Point<Pixels>, _window: &mut Window, _cx: &mut Context<Self>) -> Option<usize> {
        None
    }
}
