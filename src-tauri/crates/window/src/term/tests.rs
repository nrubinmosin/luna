//! The pane on GPUI's test platform, over a fake host: every way into the
//! session (keys, Ctrl+C both ways, AltGr, paste with bracketed paste off and
//! on and in order with the keys around it, files pasted and dropped, a
//! bitmap, the mouse, the wheel, the IME, the size and a size refused), the
//! replies to the program's queries, the start and the reattach, the exit and
//! the refused input, the wait for the core and a core replaced under it,
//! output past the pane's bound and synchronized updates left open. Driven through gpui's own dispatch and the input
//! handler the element registers; asserts what the host was sent and what
//! the grid shows. Output goes through the real seam: the fake host hands it
//! to the pane's sink, which its parser thread parses; the test waits for the
//! parse (`settle`) before it lets the window's thread run, so the wake from
//! that thread is always there to take (gpui's test scheduler is told to
//! expect a thread of the pane's own: `allow_parking`).

use super::paste::{Provider, Quoting, quote};
use super::{Phase, SessionSpec, TermPane};
use crate::demo::{self, FakeHost};
use crate::desktop::Clip;
use crate::dto::{CoreState, LinkState, ThemePref};
use crate::host::{Mailbox, cmd, event, mailbox};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::TermMode;
use gpui::{
    Bounds, ElementInputHandler, Entity, ExternalPaths, FileDropEvent, Focusable, InputHandler, KeyDownEvent, Keystroke, Modifiers,
    MouseButton, ScrollDelta, ScrollEvent, TestAppContext, VisualTestContext, point, px, size,
};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

struct Pane<'a> {
    host: Arc<FakeHost>,
    mailbox: Mailbox,
    view: Entity<TermPane>,
    cx: &'a mut VisualTestContext,
    /// Writes taken so far (`take`).
    seen: usize,
    revision: u64,
}

fn spec(provider: Provider) -> SessionSpec {
    let start = match provider {
        Provider::Claude => cmd::ENSURE_CLAUDE_SESSION,
        Provider::Codex => cmd::ENSURE_CODEX_SESSION,
    };
    SessionSpec { id: "s1".into(), provider, start, args: json!({ "id": "s1" }) }
}

fn link(state: LinkState, revision: u64) -> Value {
    serde_json::to_value(CoreState { state, core: (state == LinkState::Up).then(demo::core), error: None, revision }).unwrap()
}

/// `up`, a core of `pid`.
fn up_as(pid: u32, revision: u64) -> Value {
    let core = crate::dto::Core { pid, ..demo::core() };
    serde_json::to_value(CoreState { state: LinkState::Up, core: Some(core), error: None, revision }).unwrap()
}

/// A pane on a session of `provider`, the core first answered as `first`,
/// the start answered with `attached`.
fn open_with(cx: &mut TestAppContext, provider: Provider, first: LinkState, attached: Value) -> Pane<'_> {
    cx.executor().allow_parking();
    let host = FakeHost::new();
    host.answer(cmd::CORE_STATE, link(first, 1));
    host.answer(spec(provider).start, attached);
    let (mailbox, inbox) = mailbox();
    let pane_models = cx.update(|cx| crate::start(host.clone(), inbox, ThemePref::Light, cx));
    let (view, cx) = cx.add_window_view(move |window, cx| {
        let pane = TermPane::new(pane_models, spec(provider), window, cx);
        pane.focus(window, cx);
        pane
    });
    cx.run_until_parked();
    Pane { host, mailbox, view, cx, seen: 0, revision: 1 }
}

fn open(cx: &mut TestAppContext) -> Pane<'_> {
    open_with(cx, Provider::Claude, LinkState::Up, demo::attached("", 1))
}

impl Pane<'_> {
    /// The session prints `bytes`; the pane has parsed and drawn them after.
    fn output(&mut self, bytes: &[u8]) {
        self.host.output("s1", bytes);
        self.settle();
    }

    fn settle(&mut self) {
        self.view.read_with(self.cx, |p, _| p.parser().settle());
        self.cx.run_until_parked();
    }

    /// Everything written to the session since the last take, joined.
    fn take(&mut self) -> String {
        self.take_all().into_iter().map(|a| a["data"].as_str().unwrap().to_owned()).collect()
    }

    /// The writes since the last take, as sent.
    fn take_all(&mut self) -> Vec<Value> {
        let all = self.host.sends_of(cmd::WRITE_SESSION);
        let new = all[self.seen..].to_vec();
        self.seen = all.len();
        new
    }

    fn resizes(&self) -> Vec<Value> {
        self.host.sends_of(cmd::RESIZE_SESSION)
    }

    fn mode(&mut self) -> TermMode {
        self.view.read_with(self.cx, |p, _| *p.parser().term.lock().mode())
    }

    /// The screen's text, row by row, trailing blanks dropped.
    fn screen(&mut self) -> Vec<String> {
        self.view.read_with(self.cx, |p, _| {
            let term = p.parser().term.lock();
            let grid = term.grid();
            (0..grid.screen_lines())
                .map(|r| (0..grid.columns()).map(|c| grid[Line(r as i32)][Column(c)].c).collect::<String>().trim_end().to_owned())
                .collect()
        })
    }

    fn shows(&mut self, text: &str) -> bool {
        self.screen().iter().any(|l| l.contains(text))
    }

    fn phase(&mut self) -> Phase {
        self.view.read_with(self.cx, |p, _| p.phase())
    }

    fn tell(&mut self, name: &str, payload: Value) {
        assert!(self.mailbox.event(name, payload));
        self.cx.run_until_parked();
    }

    fn core(&mut self, state: LinkState) {
        self.revision += 1;
        let revision = self.revision;
        self.tell(event::CORE_STATE, link(state, revision));
    }

    /// The middle of cell (col, row), in the window.
    fn at(&mut self, col: f32, row: f32) -> gpui::Point<gpui::Pixels> {
        let g = self.view.read_with(self.cx, |p, _| p.geom());
        point(g.origin.x + g.cell_w * (col + 0.5), g.origin.y + g.line_h * (row + 0.5))
    }

    /// What the host's clipboard holds from now on.
    fn clipboard(&mut self, clip: Clip) {
        *self.host.desktop.clip.lock().unwrap() = clip;
    }

    /// The text last copied to the host's clipboard.
    fn clipboard_text(&mut self) -> Option<String> {
        self.host.desktop.written.lock().unwrap().last().cloned()
    }

    fn display_offset(&mut self) -> usize {
        self.view.read_with(self.cx, |p, _| p.parser().term.lock().grid().display_offset())
    }

    fn scroll(&mut self, lines: f32, modifiers: Modifiers) {
        let position = self.at(2.0, 2.0);
        self.cx.simulate_event(ScrollEvent { position, delta: ScrollDelta::Lines(point(0., lines)), modifiers, ..Default::default() });
        self.cx.run_until_parked();
    }
}

#[gpui::test]
fn keys_with_modifiers_and_cursor_modes(cx: &mut TestAppContext) {
    let mut p = open(cx);
    p.cx.simulate_keystrokes("up enter tab shift-tab backspace escape f5 ctrl-left alt-b ctrl-z");
    assert_eq!(p.take(), "\x1b[A\r\t\x1b[Z\x7f\x1b\x1b[15~\x1b[1;5D\x1bb\x1a");
    // Alt+Left as the page sends it: a word left.
    p.cx.simulate_keystrokes("alt-left");
    assert_eq!(p.take(), if cfg!(target_os = "macos") { "\x1bb" } else { "\x1b[1;5D" });

    // DECCKM on: cursor keys switch to SS3, modified ones stay CSI.
    p.output(b"\x1b[?1h");
    p.cx.simulate_keystrokes("up home shift-up");
    assert_eq!(p.take(), "\x1bOA\x1bOH\x1b[1;2A");
    p.output(b"\x1b[?1l");
    p.cx.simulate_keystrokes("down");
    assert_eq!(p.take(), "\x1b[B");

    // Printable keys go through the input handler as text.
    p.cx.simulate_keystrokes("a shift-b 1");
    assert_eq!(p.take(), "aB1");
}

#[gpui::test]
fn ctrl_c_copies_a_selection_and_interrupts_without_one(cx: &mut TestAppContext) {
    let mut p = open(cx);
    p.cx.simulate_keystrokes("ctrl-c");
    assert_eq!(p.take(), "\x03", "nothing selected: ^C");

    p.output(b"hello world\r\n");
    // From the left half of the first cell to the right half of the fifth.
    let (from, to) = (p.at(-0.3, 0.0), p.at(4.0, 0.0));
    p.cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    p.cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::none());
    p.cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    p.cx.simulate_keystrokes("ctrl-c");
    assert_eq!(p.take(), "", "a selection is copied, not sent");
    assert_eq!(p.clipboard_text().as_deref(), Some("hello"));
    // And cleared: the next Ctrl+C interrupts again.
    p.cx.simulate_keystrokes("ctrl-c");
    assert_eq!(p.take(), "\x03");
}

#[gpui::test]
fn selection_by_drag_word_and_line_and_copy(cx: &mut TestAppContext) {
    let mut p = open(cx);
    p.output(b"alpha beta gamma\r\nsecond line\r\n");
    let none = Modifiers::none();
    let word = p.at(7.0, 0.0);
    p.cx.simulate_event(gpui::MouseDownEvent {
        position: word,
        modifiers: none,
        button: MouseButton::Left,
        click_count: 2,
        first_mouse: false,
    });
    p.cx.simulate_mouse_up(word, MouseButton::Left, none);
    p.cx.simulate_keystrokes("ctrl-shift-c");
    assert_eq!(p.clipboard_text().as_deref(), Some("beta"));
    let line = p.at(1.0, 1.0);
    p.cx.simulate_event(gpui::MouseDownEvent {
        position: line,
        modifiers: none,
        button: MouseButton::Left,
        click_count: 3,
        first_mouse: false,
    });
    p.cx.simulate_mouse_up(line, MouseButton::Left, none);
    p.cx.simulate_keystrokes("ctrl-insert");
    assert_eq!(p.clipboard_text().as_deref(), Some("second line\n"));
    assert_eq!(p.take(), "", "copying sends nothing");
    // A click alone selects nothing.
    p.cx.simulate_click(word, none);
    p.cx.simulate_keystrokes("ctrl-c");
    assert_eq!(p.take(), "\x03");
}

#[gpui::test]
fn paste_is_bracketed_when_the_program_asks(cx: &mut TestAppContext) {
    let mut p = open(cx);
    p.clipboard(Clip::Text("echo hi\x1b[201~\nrm -rf x\x03\r\n".into()));
    // Mode 2004 off: line ends as CR, and no control character but those:
    // an ESC would type keys of its own, a ^C would interrupt.
    p.cx.simulate_keystrokes("ctrl-v");
    assert_eq!(p.take(), "echo hi[201~\rrm -rf x\r");
    // On: framed, every ESC and ETX removed, so the paste cannot end early.
    p.output(b"\x1b[?2004h");
    for keys in ["ctrl-v", "ctrl-shift-v", "shift-insert"] {
        p.cx.simulate_keystrokes(keys);
        assert_eq!(p.take(), "\x1b[200~echo hi[201~\rrm -rf x\r\x1b[201~", "{keys}");
    }
}

#[gpui::test]
fn files_pasted_or_dropped_are_typed_as_quoted_paths(cx: &mut TestAppContext) {
    let mut p = open(cx);
    let (a, b) = if cfg!(windows) { (r"C:\shots\a b.png", r"C:\notes.txt") } else { ("/shots/a b.png", "/notes.txt") };
    let paths = vec![PathBuf::from(a), PathBuf::from(b)];
    let typed = format!("{} {}", quote(&paths[0], Quoting::HERE).unwrap(), quote(&paths[1], Quoting::HERE).unwrap());
    p.clipboard(Clip::Files(paths.clone()));
    p.cx.simulate_keystrokes("ctrl-v");
    assert_eq!(p.take(), typed);
    // Dropped from outside the window, with the program taking pastes framed.
    p.output(b"\x1b[?2004h");
    let at = p.at(3.0, 3.0);
    // After a key press, as a person would drop one after typing.
    p.cx.simulate_keystrokes("x");
    assert_eq!(p.take(), "x");
    p.cx.simulate_event(FileDropEvent::Entered { position: at, paths: ExternalPaths(paths.clone().into()) });
    p.cx.simulate_event(FileDropEvent::Submit { position: at });
    p.cx.run_until_parked();
    assert_eq!(p.take(), format!("\x1b[200~{typed}\x1b[201~"));
    // Dragged over it and away out of the window: nothing, then or later.
    p.cx.simulate_event(FileDropEvent::Entered { position: at, paths: ExternalPaths(paths.into()) });
    p.cx.simulate_event(FileDropEvent::Exited);
    p.cx.simulate_click(at, Modifiers::none());
    assert_eq!(p.take(), "");
}

/// An image, or no text and no file at all (a bitmap in a format the window
/// does not know), presses the CLI's own image key, as the page does: the
/// CLI reads the clipboard itself.
#[gpui::test]
fn a_bitmap_presses_the_clis_own_image_key(cx: &mut TestAppContext) {
    for (provider, key) in [(Provider::Claude, "\x1bv"), (Provider::Codex, "\x16")] {
        let mut p = open_with(cx, provider, LinkState::Up, demo::attached("", 1));
        p.clipboard(Clip::Image);
        p.cx.simulate_keystrokes("ctrl-v");
        assert_eq!(p.take(), key, "{provider:?}");
        p.clipboard(Clip::Empty);
        p.cx.simulate_keystrokes("ctrl-v");
        assert_eq!(p.take(), key, "{provider:?}: nothing the window reads");
    }
}

/// A paste waits for the clipboard off the window's thread: what is typed
/// meanwhile goes after it, and two pastes keep their order whichever read
/// comes back first.
#[gpui::test]
fn keys_typed_while_a_paste_reads_the_clipboard_go_after_it(cx: &mut TestAppContext) {
    let mut p = open(cx);
    *p.host.desktop.reads.lock().unwrap() = Some(Vec::new());
    p.cx.simulate_keystrokes("ctrl-v");
    p.cx.simulate_input("x");
    p.cx.simulate_keystrokes("ctrl-v enter");
    assert_eq!(p.take(), "", "held behind the paste");
    let reads = p.host.desktop.reads.lock().unwrap().take().unwrap();
    let mut reads = reads.into_iter();
    let (first, second) = (reads.next().unwrap(), reads.next().unwrap());
    // The second read answers first: it waits for the first.
    second.send(Clip::Text("two".into())).unwrap();
    p.cx.run_until_parked();
    assert_eq!(p.take(), "");
    first.send(Clip::Text("one".into())).unwrap();
    p.cx.run_until_parked();
    assert_eq!(p.take(), "onextwo\r");
    // Nothing pending: keys go at once again.
    p.cx.simulate_input("y");
    assert_eq!(p.take(), "y");
}

/// A key waiting behind a paste is encoded as it goes, by the modes the
/// program has by then.
#[gpui::test]
fn a_key_behind_a_paste_is_encoded_by_the_modes_it_goes_with(cx: &mut TestAppContext) {
    let mut p = open(cx);
    *p.host.desktop.reads.lock().unwrap() = Some(Vec::new());
    p.cx.simulate_keystrokes("ctrl-v up");
    p.output(b"\x1b[?1h");
    let read = p.host.desktop.reads.lock().unwrap().take().unwrap().pop().unwrap();
    read.send(Clip::Text("one".into())).unwrap();
    p.cx.run_until_parked();
    assert_eq!(p.take(), "one\x1bOA");
}

/// A clipboard that does not answer holds nothing up for good: past the
/// wait the paste is dropped, said in the pane, and what waited behind it
/// goes; the answer, if it comes after, is dropped. What waits is bounded,
/// and what goes past the bound is dropped and said once.
#[gpui::test]
fn a_clipboard_that_does_not_answer_drops_the_paste_and_lets_keys_go(cx: &mut TestAppContext) {
    let mut p = open(cx);
    *p.host.desktop.reads.lock().unwrap() = Some(Vec::new());
    p.cx.simulate_keystrokes("ctrl-v");
    for _ in 0..super::QUEUE_MAX + 5 {
        p.cx.simulate_input("x");
    }
    assert_eq!(p.take(), "");
    p.settle();
    assert!(p.shows("[input dropped: too much typed while a paste waits]"));
    p.cx.executor().advance_clock(super::CLIPBOARD_WAIT);
    p.cx.run_until_parked();
    assert_eq!(p.take(), "x".repeat(super::QUEUE_MAX - 1), "the paste and what fit behind it");
    p.settle();
    assert!(p.shows("[paste dropped: the clipboard did not answer in 5 s]"));
    let late = p.host.desktop.reads.lock().unwrap().take().unwrap().pop().unwrap();
    assert!(late.send(Clip::Text("late".into())).is_err(), "nobody waits for it");
    p.cx.run_until_parked();
    assert_eq!(p.take(), "");
    p.cx.simulate_input("y");
    assert_eq!(p.take(), "y");
}

#[gpui::test]
fn sgr_mouse_reports_overlapping_buttons_each_released(cx: &mut TestAppContext) {
    let mut p = open(cx);
    p.output(b"\x1b[?1002h\x1b[?1006h");
    let none = Modifiers::none();
    let (a, b, c, d) = (p.at(0.0, 0.0), p.at(1.0, 0.0), p.at(2.0, 0.0), p.at(3.0, 1.0));
    p.cx.simulate_mouse_down(a, MouseButton::Left, none);
    p.cx.simulate_mouse_down(b, MouseButton::Right, none);
    p.cx.simulate_mouse_up(c, MouseButton::Left, none);
    p.cx.simulate_mouse_up(d, MouseButton::Right, none);
    assert_eq!(p.take(), "\x1b[<0;1;1M\x1b[<2;2;1M\x1b[<0;3;1m\x1b[<2;4;2m", "each release names its button");
    // A release of a button never pressed reports nothing.
    p.cx.simulate_mouse_up(a, MouseButton::Middle, none);
    assert_eq!(p.take(), "");
    // Drag reports motion with the button held (1002).
    p.cx.simulate_mouse_down(a, MouseButton::Left, none);
    p.cx.simulate_mouse_move(b, MouseButton::Left, none);
    p.cx.simulate_mouse_up(b, MouseButton::Left, none);
    assert_eq!(p.take(), "\x1b[<0;1;1M\x1b[<32;2;1M\x1b[<0;2;1m");
    // Shift bypasses it, for selecting.
    p.cx.simulate_mouse_down(a, MouseButton::Left, Modifiers::shift());
    p.cx.simulate_mouse_up(a, MouseButton::Left, Modifiers::shift());
    assert_eq!(p.take(), "");
}

#[gpui::test]
fn the_wheel_scrolls_back_reports_or_sends_arrows(cx: &mut TestAppContext) {
    let mut p = open(cx);
    p.output("line\r\n".repeat(200).as_bytes());
    p.scroll(3.0, Modifiers::none());
    assert_eq!(p.display_offset(), 3);
    assert_eq!(p.take(), "", "scrolling back sends nothing");
    p.cx.simulate_keystrokes("shift-pageup");
    assert!(p.display_offset() > 3);
    p.cx.simulate_keystrokes("shift-pagedown shift-pagedown");
    assert_eq!(p.display_offset(), 0);
    // Typing goes back to the bottom.
    p.scroll(5.0, Modifiers::none());
    p.cx.simulate_input("z");
    assert_eq!(p.take(), "z");
    assert_eq!(p.display_offset(), 0);

    // The alternate screen with 1007: arrows, in the cursor keys' mode.
    p.output(b"\x1b[?1049h\x1b[?1007h");
    p.scroll(2.0, Modifiers::none());
    assert_eq!(p.take(), "\x1b[A\x1b[A");
    p.output(b"\x1b[?1h");
    p.scroll(-1.0, Modifiers::none());
    assert_eq!(p.take(), "\x1bOB");
    // A program that reports the mouse gets the wheel as buttons 64 and 65.
    p.output(b"\x1b[?1000h\x1b[?1006h");
    p.scroll(1.0, Modifiers::none());
    p.scroll(-1.0, Modifiers::none());
    assert_eq!(p.take(), "\x1b[<64;3;3M\x1b[<65;3;3M");
}

#[gpui::test]
fn ime_composition_is_marked_then_committed(cx: &mut TestAppContext) {
    let mut p = open(cx);
    p.output(b"abc");
    p.cx.simulate_input("x");
    assert_eq!(p.take(), "x", "plain typing reaches the session through the registered input handler");
    let view = p.view.clone();
    p.cx.update(|window, cx| {
        let mut ime = ElementInputHandler::new(Bounds::default(), view.clone());
        ime.replace_and_mark_text_in_range(None, "に", Some(1..1), window, cx); // lang-check: allow
        assert_eq!(ime.marked_text_range(window, cx), Some(0..1));
        ime.replace_and_mark_text_in_range(None, "日本", Some(2..2), window, cx); // lang-check: allow
        assert_eq!(ime.marked_text_range(window, cx), Some(0..2));
        assert_eq!(ime.selected_text_range(false, window, cx).map(|s| s.range), Some(2..2));
        let mut adjusted = None;
        assert_eq!(ime.text_for_range(1..9, &mut adjusted, window, cx).as_deref(), Some("本")); // lang-check: allow
        assert_eq!(adjusted, Some(1..2));
    });
    assert_eq!(p.view.read_with(p.cx, |v, _| v.marked_text().map(str::to_string)), Some("日本".into())); // lang-check: allow
    assert_eq!(p.take(), "", "nothing is sent while composing");
    p.cx.run_until_parked();
    // The candidate window: the cursor's cell, then one cell per column of
    // the composition before the range.
    let g = p.view.read_with(p.cx, |v, _| v.geom());
    // Typed text is the session's to echo: the cursor is where "abc" left it.
    assert_eq!(g.cursor, Some((3, 0)));
    p.cx.update(|window, cx| {
        let mut ime = ElementInputHandler::new(Bounds::default(), view.clone());
        let at0 = ime.bounds_for_range(0..0, window, cx).unwrap();
        assert_eq!(at0.origin.x, g.origin.x + g.cell_w * 3.0);
        assert_eq!(at0.size.height, g.line_h);
        let at1 = ime.bounds_for_range(1..1, window, cx).unwrap();
        assert_eq!(at1.origin.x, g.origin.x + g.cell_w * 5.0, "日 is two columns"); // lang-check: allow
        ime.replace_text_in_range(None, "日本語", window, cx); // lang-check: allow
        assert_eq!(ime.marked_text_range(window, cx), None);
        // A surrogate pair is never split.
        ime.replace_and_mark_text_in_range(None, "a😀b", None, window, cx);
        let mut adjusted = None;
        assert_eq!(ime.text_for_range(2..4, &mut adjusted, window, cx).as_deref(), Some("😀b"));
        assert_eq!(adjusted, Some(1..4));
        // Ending the composition keeps it: it is sent.
        ime.unmark_text(window, cx);
    });
    assert_eq!(p.take(), "日本語a😀b"); // lang-check: allow
}

#[gpui::test]
fn the_pty_is_told_cells_and_pixels_and_again_on_a_resize(cx: &mut TestAppContext) {
    let mut p = open(cx);
    let first = p.resizes();
    assert_eq!(first.len(), 1, "the first size, as the session starts: {first:?}");
    let g = p.view.read_with(p.cx, |v, _| v.geom());
    let (cols, rows) = (first[0]["cols"].as_u64().unwrap(), first[0]["rows"].as_u64().unwrap());
    assert_eq!((cols as usize, rows as usize), (g.cols, g.rows));
    let cell_w = (g.cell_w.as_f32() * g.scale).round() as u64;
    assert_eq!(first[0]["pixelWidth"].as_u64(), Some(cols * cell_w));
    assert!(first[0]["pixelHeight"].as_u64().unwrap() >= rows);
    // Nothing changed, nothing sent.
    p.output(b"x");
    assert_eq!(p.resizes().len(), 1);
    p.cx.simulate_resize(size(px(500.), px(300.)));
    p.cx.run_until_parked();
    let after = p.resizes();
    assert_eq!(after.len(), 2);
    assert!(after[1]["cols"].as_u64().unwrap() < cols);
    // A scale change resizes the cells in pixels.
    p.cx.simulate_scale_factor_change(p.view.read_with(p.cx, |v, _| v.geom().scale) * 2.0);
    p.cx.update(|window, _| window.refresh());
    p.cx.run_until_parked();
    let scaled = p.resizes();
    assert_eq!(scaled.len(), 3, "{scaled:?}");
    assert!(scaled[2]["pixelWidth"].as_u64() > after[1]["pixelWidth"].as_u64());
}

#[test]
fn pty_sizes_are_exact_device_pixels() {
    let s = super::element::pty_size(97, 29, px(8.203), px(17.0), 1.5);
    // 8.203 * 1.5 = 12.3 -> 12 per cell and 17 * 1.5 = 25.5 -> 26.
    assert_eq!((s.cols, s.rows, s.width, s.height), (97, 29, 97 * 12, 29 * 26));
}

/// A fresh session: what it prints before its start answers is held, then
/// shown, and the first size goes out.
#[gpui::test]
fn a_new_session_shows_what_it_printed_while_starting(cx: &mut TestAppContext) {
    let mut p = open_with(cx, Provider::Claude, LinkState::Starting, demo::attached("", 3));
    assert_eq!(p.phase(), Phase::Waiting);
    assert!(p.host.calls_of(cmd::ENSURE_CLAUDE_SESSION).is_empty());
    p.core(LinkState::Up);
    assert_eq!(p.phase(), Phase::Live(3));
    assert_eq!(p.host.calls_of(cmd::ENSURE_CLAUDE_SESSION), vec![json!({ "id": "s1" })]);
    p.output(b"hello\x1b[c");
    assert!(p.shows("hello"));
    // A live query is answered (DA1).
    assert!(p.take().starts_with("\x1b[?"));
    assert_eq!(p.resizes().len(), 1);
}

/// A reattach: the backlog first, with no old query answered again, wiped
/// and the session's modes put back; what came while the start was asked is
/// dropped, as some of it is in the backlog already and the modes may be
/// newer than the rest; the CLI is asked to repaint (a row less, and back),
/// and only then does the session take keys, framed by the modes put back.
#[gpui::test]
fn a_reattach_replays_the_backlog_and_drops_what_came_meanwhile(cx: &mut TestAppContext) {
    let mut p = open_with(cx, Provider::Claude, LinkState::Starting, demo::attached("OLD SCREEN\x1b[c\r\n", 7));
    p.host.answer(cmd::SESSION_MODES, json!("\x1b[?2004h\x1b[?1h"));
    // Output arrives after the start is asked and before its answer is
    // taken: the backlog may hold it already. Keys meanwhile go nowhere.
    p.view.update(p.cx, |v, cx| v.start(cx));
    assert!(p.host.output("s1", b"OLD SCREEN\x1b[?2004l"));
    p.cx.simulate_input("early");
    p.cx.run_until_parked();
    p.settle();
    assert_eq!(p.phase(), Phase::Live(7));
    assert_eq!(p.host.calls_of(cmd::SESSION_MODES), vec![json!({ "id": "s1" })]);
    assert!(!p.shows("OLD SCREEN"), "the backlog was shown and wiped, and nothing of before drawn again: {:?}", p.screen());
    assert_eq!(p.take(), "", "the backlog's query was answered when it was new; keys before the modes went nowhere");
    assert!(
        p.mode().contains(TermMode::BRACKETED_PASTE | TermMode::APP_CURSOR),
        "the session's modes are back, not undone by stale output"
    );
    let rows: Vec<u64> = p.resizes().iter().map(|r| r["rows"].as_u64().unwrap()).collect();
    let n = rows.len();
    assert!(n >= 2 && rows[n - 2] + 1 == rows[n - 1], "a row less, then back: {rows:?}");
    // The repaint and everything after it is live.
    p.output(b"REPAINTED");
    assert_eq!(p.screen()[0], "REPAINTED");
    p.clipboard(Clip::Text("hi".into()));
    p.cx.simulate_keystrokes("ctrl-v");
    assert_eq!(p.take(), "\x1b[200~hi\x1b[201~");
}

/// A reattach to a core that numbers its output: the backlog is shown and
/// wiped, the modes it came with put back, and the output from its end on
/// goes after it exactly: held before the answer or come after it, what the
/// backlog has is dropped, a chunk across its end is cut there, and a query
/// past it is answered once, the backlog's never. The backlog's last escape,
/// left unfinished, is finished by what comes after. Input goes once the
/// modes are parsed, and no SESSION_MODES is asked.
#[gpui::test]
fn an_exact_reattach_takes_the_output_from_the_backlogs_end_once(cx: &mut TestAppContext) {
    let mut p = open_with(cx, Provider::Claude, LinkState::Starting, demo::attached_at("OLD SCREEN\x1b[c\r\n\x1b[?20", 7, 100, "\x1b[?1h"));
    p.view.update(p.cx, |v, cx| v.start(cx));
    // In the backlog; across its end, with the rest of its last escape; a
    // query past it.
    assert!(p.host.output_at("s1", b"OLD", Some(90)));
    assert!(p.host.output_at("s1", b"2004hA", Some(98)));
    assert!(p.host.output_at("s1", b"\x1b[c", Some(104)));
    p.cx.run_until_parked();
    p.settle();
    assert_eq!(p.phase(), Phase::Live(7));
    assert!(p.host.output_at("s1", b"B", Some(107)));
    p.settle();
    assert_eq!(p.screen()[0], "AB", "{:?}", p.screen());
    assert!(p.mode().contains(TermMode::BRACKETED_PASTE | TermMode::APP_CURSOR), "the modes, and the escape finished");
    assert_eq!(p.take_all(), vec![json!({ "id": "s1", "data": "\x1b[?6c", "reply": true })], "the live query, once");
    assert!(p.host.calls_of(cmd::SESSION_MODES).is_empty());
    assert_eq!(p.view.read_with(p.cx, |v, cx| v.waiting(cx)), None);
    p.cx.simulate_input("k");
    assert_eq!(p.take(), "k");

    // Again, with nothing held: the first chunk after the answer crosses the
    // backlog's end, and is cut there.
    p.host.answer(cmd::ENSURE_CLAUDE_SESSION, demo::attached_at("", 8, 200, ""));
    p.view.update(p.cx, |v, cx| v.start(cx));
    p.cx.run_until_parked();
    p.settle();
    assert!(p.host.output_at("s1", b"OLDNEW", Some(197)));
    p.settle();
    assert_eq!(p.screen()[0], "NEW", "{:?}", p.screen());
}

/// Output that starts past where the backlog ended means some was missed:
/// held before the answer, or come after it, the pane starts over and shows
/// none of it.
#[gpui::test]
fn output_missed_after_the_backlog_starts_the_session_over(cx: &mut TestAppContext) {
    let mut p = open_with(cx, Provider::Claude, LinkState::Up, demo::attached_at("OLD\r\n", 3, 50, ""));
    let starts = |p: &mut Pane| p.host.calls_of(cmd::ENSURE_CLAUDE_SESSION).len();
    assert_eq!((starts(&mut p), p.phase()), (1, Phase::Live(3)));
    p.view.update(p.cx, |v, cx| v.start(cx));
    assert!(p.host.output_at("s1", b"LOST", Some(60)));
    p.cx.run_until_parked();
    p.settle();
    assert_eq!((starts(&mut p), p.phase()), (3, Phase::Live(3)), "started over once, then attached");
    assert!(!p.shows("LOST"));
    assert!(p.host.output_at("s1", b"GAP", Some(70)));
    p.settle();
    assert_eq!(starts(&mut p), 4);
    assert!(p.host.output_at("s1", b"NEXT", Some(50)));
    p.settle();
    assert!(!p.shows("GAP") && p.shows("NEXT"), "{:?}", p.screen());
}

/// A reattach waits for the modes, saying so, and takes no key meanwhile,
/// not even to encode; one that does not get them in time, or gets an error,
/// starts over rather than take input without them; and one whose answer
/// comes after the pane has gone back to waiting for the core unlocks
/// nothing.
#[gpui::test]
fn a_reattach_is_bounded_and_a_late_answer_unlocks_nothing(cx: &mut TestAppContext) {
    let mut p = open_with(cx, Provider::Claude, LinkState::Starting, demo::attached("OLD\r\n", 4));
    p.host.defer(cmd::SESSION_MODES);
    let starts = |p: &mut Pane| p.host.calls_of(cmd::ENSURE_CLAUDE_SESSION).len();
    let said = |p: &mut Pane| p.view.read_with(p.cx, |v, cx| v.waiting(cx));
    p.core(LinkState::Up);
    assert_eq!((starts(&mut p), p.phase()), (1, Phase::Live(4)));
    assert_eq!(said(&mut p), Some("Reattaching to the session..."));
    p.cx.simulate_keystrokes("up a");
    p.cx.simulate_input("b");
    assert_eq!(p.take(), "", "no input before the modes");
    // Past the bound: started over. The first attach's answer, come late,
    // unlocks nothing.
    p.cx.executor().advance_clock(super::REATTACH_WAIT);
    p.cx.run_until_parked();
    assert_eq!(starts(&mut p), 2);
    assert!(p.host.resolve(cmd::SESSION_MODES, Ok(json!("\x1b[?2004h"))));
    p.cx.run_until_parked();
    p.settle();
    assert_eq!(said(&mut p), Some("Reattaching to the session..."));
    p.cx.simulate_input("x");
    assert_eq!(p.take(), "");
    // An error: started over too, never live with no modes.
    assert!(p.host.resolve(cmd::SESSION_MODES, Err("broken".into())));
    p.cx.run_until_parked();
    assert_eq!(starts(&mut p), 3);
    // The core goes away; the answer comes after.
    p.core(LinkState::Starting);
    assert_eq!(p.phase(), Phase::Waiting);
    assert!(p.host.resolve(cmd::SESSION_MODES, Ok(json!("\x1b[?2004h"))));
    p.cx.run_until_parked();
    p.settle();
    assert_eq!(p.phase(), Phase::Waiting);
    p.cx.simulate_input("c");
    assert_eq!(p.take(), "");
    assert!(!p.mode().contains(TermMode::BRACKETED_PASTE), "nothing of the late answer was parsed");
    // Up again: this one goes through.
    p.core(LinkState::Up);
    assert_eq!(starts(&mut p), 4);
    assert!(p.host.resolve(cmd::SESSION_MODES, Ok(json!("\x1b[?2004h"))));
    p.cx.run_until_parked();
    p.settle();
    assert_eq!(said(&mut p), None);
    p.cx.simulate_input("d");
    assert_eq!(p.take(), "d");
}

/// The core replaced under the pane, its `starting` folded into the `up`
/// after it in the inbox: a new core (pid) or the same one found again (a
/// newer revision) starts the session over, as the page's epoch does; the
/// same state taken again does not.
#[gpui::test]
fn a_core_replaced_between_two_states_starts_the_session_over(cx: &mut TestAppContext) {
    let mut p = open(cx);
    assert_eq!(p.phase(), Phase::Live(1));
    p.output(b"before");
    let starts = |p: &mut Pane| p.host.calls_of(cmd::ENSURE_CLAUDE_SESSION).len();
    assert_eq!(starts(&mut p), 1);
    // The same `up` again (the answer to `core_state` after the event).
    p.tell(event::CORE_STATE, link(LinkState::Up, 1));
    assert_eq!(starts(&mut p), 1);
    // Another core, nothing in between.
    p.host.answer(cmd::ENSURE_CLAUDE_SESSION, demo::attached("", 2));
    p.tell(event::CORE_STATE, up_as(5151, 3));
    p.settle();
    assert_eq!((starts(&mut p), p.phase()), (2, Phase::Live(2)));
    assert!(!p.shows("before"), "started over on a clean grid");
    // The same core found again: only its revision tells.
    p.host.answer(cmd::ENSURE_CLAUDE_SESSION, demo::attached("", 3));
    p.tell(event::CORE_STATE, up_as(5151, 6));
    assert_eq!((starts(&mut p), p.phase()), (3, Phase::Live(3)));
}

/// `pty://start-over` (a shutdown taken back closed every session): every
/// pane starts its session again, an ended one too.
#[gpui::test]
fn the_cores_start_over_starts_every_pane_again(cx: &mut TestAppContext) {
    let mut p = open(cx);
    p.tell(event::PTY_EXIT, json!({ "id": "s1", "code": 0, "generation": 1 }));
    assert_eq!(p.phase(), Phase::Ended);
    p.host.answer(cmd::ENSURE_CLAUDE_SESSION, demo::attached("", 2));
    p.tell(event::START_OVER, json!({ "why": "the shutdown was taken back" }));
    assert_eq!(p.phase(), Phase::Live(2));
    assert_eq!(p.host.calls_of(cmd::ENSURE_CLAUDE_SESSION).len(), 2);
    // While the core is not up, it waits for it.
    p.core(LinkState::Starting);
    p.tell(event::START_OVER, json!({ "why": "again" }));
    assert_eq!(p.phase(), Phase::Waiting);
}

#[gpui::test]
fn the_sessions_exit_and_refused_input_are_said_in_the_pane(cx: &mut TestAppContext) {
    let mut p = open(cx);
    p.tell(event::PTY_INPUT_REFUSED, json!({ "id": "s1", "waitingKb": 12 }));
    p.settle();
    assert!(p.shows("[keys not delivered: the session is not taking input, 12 KB waiting]"));
    // Another session's exit, or an older spawn's of this one, is not its own.
    p.tell(event::PTY_EXIT, json!({ "id": "other", "code": 0, "generation": 1 }));
    p.tell(event::PTY_EXIT, json!({ "id": "s1", "code": 0, "generation": 0 }));
    assert_eq!(p.phase(), Phase::Live(1));
    p.tell(event::PTY_EXIT, json!({ "id": "s1", "code": 0, "generation": 1 }));
    p.settle();
    assert_eq!(p.phase(), Phase::Ended);
    assert!(p.shows("[session exited]"));
    // Nothing goes to a session that is gone.
    p.cx.simulate_input("x");
    assert_eq!(p.take(), "");
}

#[gpui::test]
fn an_exit_heard_before_the_start_answered_ends_it_after_its_backlog(cx: &mut TestAppContext) {
    let mut p = open_with(cx, Provider::Codex, LinkState::Starting, demo::attached("bye\r\n", 2));
    p.view.update(p.cx, |v, cx| {
        v.start(cx);
        v.on_exit(2, cx);
    });
    p.cx.run_until_parked();
    p.settle();
    assert_eq!(p.phase(), Phase::Ended);
    assert!(p.shows("bye") && p.shows("[session exited]"));
}

#[gpui::test]
fn a_start_that_fails_is_said_unless_the_core_went_away(cx: &mut TestAppContext) {
    let mut p = open_with(cx, Provider::Claude, LinkState::Up, json!("not an answer"));
    p.settle();
    assert_eq!(p.phase(), Phase::Ended);
    assert!(p.screen().iter().any(|l| l.starts_with("[failed to start session:")), "{:?}", p.screen());
}

/// The core is found, replaced, found again: the pane waits and says so,
/// never that the core is not running, and starts once it is up.
#[gpui::test]
fn the_pane_waits_for_the_core_and_starts_again_in_the_next(cx: &mut TestAppContext) {
    let mut p = open_with(cx, Provider::Claude, LinkState::Starting, demo::attached("", 1));
    let said = |p: &mut Pane| p.view.read_with(p.cx, |v, cx| v.waiting(cx));
    assert!(said(&mut p).unwrap().starts_with("Waiting for Luna's core"));
    p.core(LinkState::Up);
    assert_eq!(p.phase(), Phase::Live(1));
    assert_eq!(said(&mut p), None);
    p.output(b"first run");
    // Replaced: it waits, and keys go nowhere meanwhile.
    p.core(LinkState::Starting);
    assert_eq!(p.phase(), Phase::Waiting);
    let replacing = said(&mut p).unwrap();
    assert!(replacing.contains("being replaced") && !replacing.contains("not running"), "{replacing}");
    p.cx.simulate_input("lost");
    assert_eq!(p.take(), "");
    p.host.answer(cmd::ENSURE_CLAUDE_SESSION, demo::attached("", 2));
    p.core(LinkState::Up);
    p.settle();
    assert_eq!(p.phase(), Phase::Live(2));
    assert_eq!(p.host.calls_of(cmd::ENSURE_CLAUDE_SESSION).len(), 2);
    assert!(!p.shows("first run"), "a start in the new core begins on a clean grid");
    // Down or of another build: it still waits.
    p.core(LinkState::Down);
    assert!(said(&mut p).unwrap().starts_with("Waiting for Luna's core"));
}

/// Focus follows the window's activation and the pane's own focus, as the
/// page's terminal does, each report a write of its own.
#[gpui::test]
fn focus_is_reported_to_a_program_that_asks(cx: &mut TestAppContext) {
    let mut p = open(cx);
    p.cx.deactivate_window();
    p.output(b"\x1b[?1004h");
    p.cx.update(|window, _| window.activate_window());
    p.cx.run_until_parked();
    p.cx.deactivate_window();
    p.cx.run_until_parked();
    p.cx.update(|window, _| window.activate_window());
    p.cx.run_until_parked();
    p.cx.update(|window, cx| window.blur(cx));
    p.cx.run_until_parked();
    let view = p.view.clone();
    p.cx.update(|window, cx| {
        let handle = view.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
    });
    p.cx.run_until_parked();
    let writes: Vec<Value> = p.host.sends_of(cmd::WRITE_SESSION);
    assert_eq!(writes.iter().map(|w| w["data"].as_str().unwrap()).collect::<Vec<_>>(), ["\x1b[I", "\x1b[O", "\x1b[I", "\x1b[O", "\x1b[I"]);
}

#[gpui::test]
fn color_queries_are_answered_from_the_theme(cx: &mut TestAppContext) {
    let mut p = open(cx);
    p.output(b"\x1b]11;?\x07");
    let reply = p.take();
    assert!(reply.contains("rgb:ffff/ffff/ffff"), "the light ground: {reply:?}");
}

/// Replies to the program's queries go as replies, which the core does not
/// count as a person typing; keys do not.
#[gpui::test]
fn replies_go_on_their_own_lane(cx: &mut TestAppContext) {
    let mut p = open(cx);
    p.output(b"\x1b[c\x1b]10;?\x07\x1b[14t");
    let replies = p.take_all();
    assert_eq!(replies.len(), 3, "{replies:?}");
    assert!(replies.iter().all(|r| r["reply"] == true), "{replies:?}");
    p.cx.simulate_input("k");
    assert_eq!(p.take_all(), vec![json!({ "id": "s1", "data": "k" })]);
}

/// AltGr comes as Ctrl+Alt: its text is typed, never taken for a control
/// key (AltGr+Space as ESC NUL).
#[gpui::test]
fn altgr_text_is_typed_not_sent_as_ctrl_alt(cx: &mut TestAppContext) {
    let mut p = open(cx);
    let altgr = Modifiers { control: true, alt: true, ..Modifiers::default() };
    for (key, text) in [("space", "\u{a0}"), ("e", "\u{20ac}")] {
        let keystroke = Keystroke { modifiers: altgr, key: key.into(), key_char: Some(text.into()) };
        p.cx.simulate_event(KeyDownEvent { keystroke, is_held: false, prefer_character_input: true });
        assert_eq!(p.take(), "", "{key}: left to the text input");
        // Which then types it.
        p.cx.simulate_input(text);
        assert_eq!(p.take(), text);
    }
}

/// A size the host refused is not taken for sent: it goes again a moment
/// later.
#[gpui::test]
fn a_refused_size_is_sent_again(cx: &mut TestAppContext) {
    let p = open(cx);
    let before = p.resizes().len();
    p.host.refuse(cmd::RESIZE_SESSION, 1);
    p.cx.simulate_resize(size(px(500.), px(300.)));
    p.cx.run_until_parked();
    assert_eq!(p.resizes().len(), before, "refused, so not sent");
    p.cx.executor().advance_clock(super::RESIZE_RETRY);
    p.cx.run_until_parked();
    let after = p.resizes();
    assert_eq!(after.len(), before + 1, "{after:?}");
    let g = p.view.read_with(p.cx, |v, _| v.geom());
    assert_eq!(after.last().unwrap()["cols"].as_u64(), Some(g.cols as u64));
}

/// Output past the pane's bound is dropped rather than held, and the pane
/// says how much in a line of its own.
///
/// The parser says what it dropped as soon as it has caught up, so one that
/// outpaces the attach's sending would say it between two chunks, and the
/// chunks after would scroll it off the screen. The parser is held on its own
/// thread, by a job that tells when it has started and waits to be let go,
/// until the whole attach is queued behind it: it then parses all of it
/// without a moment of being caught up before the end.
#[gpui::test]
fn output_past_the_bound_is_dropped_and_said(cx: &mut TestAppContext) {
    use std::sync::mpsc::channel;
    let mut p = open_with(cx, Provider::Claude, LinkState::Starting, demo::attached("", 1));
    p.view.update(p.cx, |v, cx| v.start(cx));
    let (held, holding) = channel();
    let (go, gate) = channel::<()>();
    p.view.read_with(p.cx, |p, _| {
        p.parser().mark(move || {
            let _ = held.send(());
            let _ = gate.recv();
        })
    });
    // Past its own look at what was dropped, which is nothing yet.
    holding.recv().expect("the parser took the job");
    let chunk = "x".repeat(64 << 10);
    for _ in 0..(super::parser::OUTPUT_MAX / chunk.len() + 4) {
        assert!(p.host.output("s1", chunk.as_bytes()), "still routed");
    }
    p.cx.run_until_parked();
    go.send(()).expect("the parser waits");
    p.settle();
    assert_eq!(p.phase(), Phase::Live(1));
    assert!(p.shows("[dropped 256 KB of output: the pane fell behind]"), "{:?}", p.screen());
    // Parsed, so the room is back.
    p.output(b"\r\nafter");
    assert!(p.shows("after"));
}

/// A synchronized update (2026) left open is drawn at its deadline, and at
/// once before the pane's own notice; one that buffers too much is drawn
/// without waiting for its end.
#[gpui::test]
fn an_unfinished_synchronized_update_does_not_hold_the_pane(cx: &mut TestAppContext) {
    let mut p = open(cx);
    p.output(b"\x1b[?2026hHALF");
    assert!(!p.shows("HALF"), "held while the frame is open");
    std::thread::sleep(std::time::Duration::from_millis(300));
    p.settle();
    assert!(p.shows("HALF"), "drawn at the deadline");
    p.output(b"\x1b[?2026h\r\nPART");
    p.tell(event::PTY_EXIT, json!({ "id": "s1", "code": 0, "generation": 1 }));
    p.settle();
    assert!(p.shows("PART") && p.shows("[session exited]"), "{:?}", p.screen());
}

#[gpui::test]
fn a_synchronized_update_past_its_bound_is_drawn_without_its_end(cx: &mut TestAppContext) {
    let mut p = open(cx);
    let mut big = b"\x1b[?2026h".to_vec();
    big.extend(std::iter::repeat_n(b'y', super::parser::SYNC_MAX + 8192));
    p.output(&big);
    assert!(p.shows("yyyy"), "drawn past the bound without its end");
}

#[gpui::test]
fn an_idle_pane_draws_nothing(cx: &mut TestAppContext) {
    let mut p = open(cx);
    p.output(b"$ ");
    let before = p.view.read_with(p.cx, |v, _| v.renders);
    p.cx.executor().advance_clock(std::time::Duration::from_secs(60));
    p.cx.run_until_parked();
    assert_eq!(p.view.read_with(p.cx, |v, _| v.renders), before, "no blink, no timer");
    p.output(b"x");
    assert!(p.view.read_with(p.cx, |v, _| v.renders) > before, "output draws");
}
