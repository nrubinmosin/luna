//! The pane on GPUI's test platform, over a fake host: every way into the
//! session (keys, Ctrl+C both ways, paste with bracketed paste off and on,
//! files pasted and dropped, a bitmap, the mouse, the wheel, the IME, the
//! size), the start and the reattach, the exit and the refused input, and the
//! wait for the core. Driven through gpui's own dispatch and the input
//! handler the element registers; asserts what the host was sent and what
//! the grid shows. Output goes through the real seam: the fake host hands it
//! to the pane's sink, which its parser thread parses; the test waits for the
//! parse (`settle`) before it lets the window's thread run, so the wake from
//! that thread is always there to take (gpui's test scheduler is told to
//! expect a thread of the pane's own: `allow_parking`).

use super::paste::{Provider, Quoting, quote};
use super::{Phase, SessionSpec, TermPane};
use crate::demo::{self, FakeHost};
use crate::dto::{CoreState, LinkState, ThemePref};
use crate::host::{Mailbox, cmd, event, mailbox};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::TermMode;
use gpui::{
    Bounds, ClipboardEntry, ClipboardItem, ElementInputHandler, Entity, ExternalPaths, FileDropEvent, Focusable, Image, ImageFormat,
    InputHandler, Modifiers, MouseButton, ScrollDelta, ScrollEvent, TestAppContext, VisualTestContext, point, px, size,
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
        let all = self.host.sends_of(cmd::WRITE_SESSION);
        let new = all[self.seen..].iter().map(|a| a["data"].as_str().unwrap().to_owned()).collect();
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

    fn clipboard(&mut self, item: ClipboardItem) {
        self.cx.update(|_, cx| cx.write_to_clipboard(item));
    }

    fn clipboard_text(&mut self) -> Option<String> {
        self.cx.update(|_, cx| cx.read_from_clipboard().and_then(|i| i.text()))
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
    p.clipboard(ClipboardItem::new_string("echo hi\x1b[201~\nrm -rf x\x03\r\n".into()));
    // Mode 2004 off: the text as it is, line ends as CR.
    p.cx.simulate_keystrokes("ctrl-v");
    assert_eq!(p.take(), "echo hi\x1b[201~\rrm -rf x\x03\r");
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
    p.clipboard(ClipboardItem { entries: vec![ClipboardEntry::ExternalPaths(ExternalPaths(paths.clone().into()))] });
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

#[gpui::test]
fn a_bitmap_presses_the_clis_own_image_key(cx: &mut TestAppContext) {
    for (provider, key) in [(Provider::Claude, "\x1bv"), (Provider::Codex, "\x16")] {
        let mut p = open_with(cx, provider, LinkState::Up, demo::attached("", 1));
        p.clipboard(ClipboardItem { entries: vec![ClipboardEntry::Image(Image::from_bytes(ImageFormat::Png, vec![0x89, 0x50]))] });
        p.cx.simulate_keystrokes("ctrl-v");
        assert_eq!(p.take(), key, "{provider:?}");
        // With text beside it, the text is what is pasted.
        p.clipboard(ClipboardItem {
            entries: vec![
                ClipboardEntry::Image(Image::from_bytes(ImageFormat::Png, vec![0x89])),
                ClipboardEntry::String(gpui::ClipboardString::new("cells".into())),
            ],
        });
        p.cx.simulate_keystrokes("ctrl-v");
        assert_eq!(p.take(), "cells");
    }
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
/// and the session's modes put back, then the output that came meanwhile,
/// and the CLI asked to repaint (a row less, and back).
#[gpui::test]
fn a_reattach_replays_the_backlog_before_the_live_output(cx: &mut TestAppContext) {
    let mut p = open_with(cx, Provider::Claude, LinkState::Starting, demo::attached("OLD SCREEN\x1b[c\r\n", 7));
    p.host.answer(cmd::SESSION_MODES, json!("\x1b[?2004h\x1b[?1h"));
    // The live output arrives after the start is asked and before its
    // answer is taken.
    p.view.update(p.cx, |v, cx| v.start(cx));
    assert!(p.host.output("s1", b"LIVE"));
    p.cx.run_until_parked();
    p.settle();
    assert_eq!(p.phase(), Phase::Live(7));
    assert_eq!(p.host.calls_of(cmd::SESSION_MODES), vec![json!({ "id": "s1" })]);
    let screen = p.screen();
    assert_eq!(screen[0], "LIVE", "the backlog was shown and wiped before the live output: {screen:?}");
    assert!(!p.shows("OLD SCREEN"));
    assert_eq!(p.take(), "", "the backlog's query was answered when it was new");
    assert!(p.mode().contains(TermMode::BRACKETED_PASTE | TermMode::APP_CURSOR), "the session's modes are back");
    let rows: Vec<u64> = p.resizes().iter().map(|r| r["rows"].as_u64().unwrap()).collect();
    let n = rows.len();
    assert!(n >= 2 && rows[n - 2] + 1 == rows[n - 1], "a row less, then back: {rows:?}");
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
