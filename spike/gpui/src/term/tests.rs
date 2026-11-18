// The pane in gpui's test platform (TestAppContext / VisualTestContext, the
// test-support feature): a TermView on a fake PTY (Source::Sink), driven
// through gpui's own dispatch (simulate_keystrokes, simulate_input, actions)
// and its InputHandler (ElementInputHandler, the same object the element
// registers with window.handle_input during paint, that the platform's IME
// calls). Asserts the bytes that reach the fake PTY.
use std::sync::{Arc, Mutex};

use gpui::{
    Bounds, ClipboardItem, ElementInputHandler, Entity, InputHandler, Modifiers, MouseButton, TestAppContext,
    VisualTestContext, point, px,
};

use super::{Options, PasteText, Source, TermView, init};

fn pane(cx: &mut TestAppContext) -> (Entity<TermView>, &mut VisualTestContext, Arc<Mutex<Vec<u8>>>) {
    cx.update(init);
    let sink = Arc::new(Mutex::new(Vec::new()));
    let s = sink.clone();
    let (view, vcx) = cx.add_window_view(move |window, cx| {
        TermView::new(Source::Sink(s), Options { font_size: px(14.), ..Default::default() }, window, cx)
    });
    vcx.run_until_parked();
    (view, vcx, sink)
}

fn take(sink: &Arc<Mutex<Vec<u8>>>) -> Vec<u8> {
    std::mem::take(&mut *sink.lock().unwrap())
}

#[gpui::test]
fn ime_composition_is_marked_then_committed(cx: &mut TestAppContext) {
    let (view, vcx, sink) = pane(cx);
    // Put the cursor at column 3, so the candidate window has somewhere to go.
    view.update(vcx, |v, cx| v.feed_bytes(b"abc", cx));
    vcx.simulate_input("x");
    assert_eq!(take(&sink), b"x", "plain typing reaches the PTY through the registered InputHandler");

    vcx.update(|window, cx| {
        let mut ime = ElementInputHandler::new(Bounds::default(), view.clone());
        ime.replace_and_mark_text_in_range(None, "に", Some(1..1), window, cx);
        assert_eq!(ime.marked_text_range(window, cx), Some(0..1));
        ime.replace_and_mark_text_in_range(None, "日本", Some(2..2), window, cx);
        assert_eq!(ime.marked_text_range(window, cx), Some(0..2));
        assert_eq!(ime.selected_text_range(false, window, cx).map(|s| s.range), Some(2..2));
        let mut adjusted = None;
        assert_eq!(ime.text_for_range(1..9, &mut adjusted, window, cx).as_deref(), Some("本"));
        assert_eq!(adjusted, Some(1..2));
    });
    assert_eq!(view.read_with(vcx, |v, _| v.marked_text().map(str::to_string)), Some("日本".into()));
    assert!(take(&sink).is_empty(), "nothing is sent while composing");

    // The candidate window: the cursor's cell, then one cell per column of
    // composed text before the range.
    vcx.run_until_parked();
    let geom = view.read_with(vcx, |v, _| v.geom.get());
    assert_eq!(geom.cursor, Some((3, 0)));
    vcx.update(|window, cx| {
        let mut ime = ElementInputHandler::new(Bounds::default(), view.clone());
        let at0 = ime.bounds_for_range(0..0, window, cx).unwrap();
        assert_eq!(at0.origin.x, geom.origin.x + geom.cell_w * 3.0);
        assert_eq!(at0.size.height, geom.line_h);
        let at1 = ime.bounds_for_range(1..1, window, cx).unwrap();
        assert_eq!(at1.origin.x, geom.origin.x + geom.cell_w * 5.0, "日 is two columns");

        // Commit.
        ime.replace_text_in_range(None, "日本語", window, cx);
        assert_eq!(ime.marked_text_range(window, cx), None);
    });
    assert_eq!(String::from_utf8(take(&sink)).unwrap(), "日本語");

    // UTF-16 offsets: an emoji is two units.
    vcx.update(|window, cx| {
        let mut ime = ElementInputHandler::new(Bounds::default(), view.clone());
        ime.replace_and_mark_text_in_range(None, "😀a", Some(1..3), window, cx);
        assert_eq!(ime.marked_text_range(window, cx), Some(0..3));
        assert_eq!(ime.selected_text_range(false, window, cx).map(|s| s.range), Some(1..3));
        // Ending the composition keeps the text: it is sent.
        ime.unmark_text(window, cx);
        assert_eq!(ime.marked_text_range(window, cx), None);
        // An empty composition cancels.
        ime.replace_and_mark_text_in_range(None, "か", None, window, cx);
        ime.replace_and_mark_text_in_range(None, "", None, window, cx);
        assert_eq!(ime.marked_text_range(window, cx), None);
    });
    assert_eq!(String::from_utf8(take(&sink)).unwrap(), "😀a");
}

#[gpui::test]
fn paste_is_bracketed_when_the_app_asks(cx: &mut TestAppContext) {
    let (view, vcx, sink) = pane(cx);
    let text = "echo hi\x1b[201~\nrm -rf x\r\n";
    vcx.update(|_, cx| cx.write_to_clipboard(ClipboardItem::new_string(text.into())));

    // Mode 2004 off: the text as is, line ends as CR.
    vcx.simulate_keystrokes("ctrl-shift-v");
    assert_eq!(take(&sink), b"echo hi\x1b[201~\rrm -rf x\r");

    // Mode 2004 on: framed, every ESC removed, so the paste cannot end early.
    view.update(vcx, |v, cx| v.feed_bytes(b"\x1b[?2004h", cx));
    vcx.simulate_keystrokes("ctrl-shift-v");
    assert_eq!(take(&sink), b"\x1b[200~echo hi[201~\rrm -rf x\r\x1b[201~");
    vcx.dispatch_action(PasteText);
    assert_eq!(take(&sink), b"\x1b[200~echo hi[201~\rrm -rf x\r\x1b[201~");

    view.update(vcx, |v, cx| v.feed_bytes(b"\x1b[?2004l", cx));
    vcx.simulate_keystrokes("shift-insert");
    assert_eq!(take(&sink), b"echo hi\x1b[201~\rrm -rf x\r");
}

#[gpui::test]
fn keys_through_gpui_dispatch(cx: &mut TestAppContext) {
    let (view, vcx, sink) = pane(cx);
    vcx.simulate_keystrokes("up enter ctrl-c tab shift-tab backspace escape f5 ctrl-left alt-b");
    assert_eq!(take(&sink), b"\x1b[A\r\x03\t\x1b[Z\x7f\x1b\x1b[15~\x1b[1;5D\x1bb");

    // DECCKM on: cursor keys switch to SS3, modified ones stay CSI.
    view.update(vcx, |v, cx| v.feed_bytes(b"\x1b[?1h", cx));
    vcx.simulate_keystrokes("up home shift-up");
    assert_eq!(take(&sink), b"\x1bOA\x1bOH\x1b[1;2A");
    view.update(vcx, |v, cx| v.feed_bytes(b"\x1b[?1l", cx));
    vcx.simulate_keystrokes("down");
    assert_eq!(take(&sink), b"\x1b[B");

    // Printable keys go through the InputHandler as text.
    vcx.simulate_keystrokes("a shift-b 1");
    assert_eq!(take(&sink), b"aB1");

    // Shift+PageUp scrolls the scrollback and sends nothing.
    view.update(vcx, |v, cx| v.feed_bytes("line\r\n".repeat(200).as_bytes(), cx));
    vcx.simulate_keystrokes("shift-pageup");
    assert!(take(&sink).is_empty());
    assert!(view.read_with(vcx, |v, _| v.term.lock().grid().display_offset()) > 0);
    // Typing jumps back to the bottom.
    vcx.simulate_input("z");
    assert_eq!(take(&sink), b"z");
    assert_eq!(view.read_with(vcx, |v, _| v.term.lock().grid().display_offset()), 0);
}

#[gpui::test]
fn bracketed_paste_drops_etx(cx: &mut TestAppContext) {
    let (view, vcx, sink) = pane(cx);
    vcx.update(|_, cx| cx.write_to_clipboard(ClipboardItem::new_string("a\x03b\x1bc".into())));
    view.update(vcx, |v, cx| v.feed_bytes(b"\x1b[?2004h", cx));
    vcx.simulate_keystrokes("ctrl-shift-v");
    assert_eq!(take(&sink), b"\x1b[200~abc\x1b[201~");
}

#[gpui::test]
fn overlapping_mouse_presses_release_their_own_button(cx: &mut TestAppContext) {
    let (view, vcx, sink) = pane(cx);
    // SGR reporting of presses and drags.
    view.update(vcx, |v, cx| v.feed_bytes(b"\x1b[?1002h\x1b[?1006h", cx));
    let g = view.read_with(vcx, |v, _| v.geom.get());
    let at = |col: f32, row: f32| point(g.origin.x + g.cell_w * (col + 0.5), g.origin.y + g.line_h * (row + 0.5));
    let none = Modifiers::default();
    vcx.simulate_mouse_down(at(0.0, 0.0), MouseButton::Left, none);
    vcx.simulate_mouse_down(at(1.0, 0.0), MouseButton::Right, none);
    vcx.simulate_mouse_up(at(2.0, 0.0), MouseButton::Left, none);
    vcx.simulate_mouse_up(at(3.0, 1.0), MouseButton::Right, none);
    assert_eq!(
        String::from_utf8(take(&sink)).unwrap(),
        "\x1b[<0;1;1M\x1b[<2;2;1M\x1b[<0;3;1m\x1b[<2;4;2m",
        "each release names the button released"
    );
    // A release of a button that was never pressed reports nothing.
    vcx.simulate_mouse_up(at(0.0, 0.0), MouseButton::Middle, none);
    assert!(take(&sink).is_empty());
}

#[gpui::test]
fn ime_text_for_range_widens_a_split_surrogate_pair(cx: &mut TestAppContext) {
    let (view, vcx, _sink) = pane(cx);
    vcx.update(|window, cx| {
        let mut ime = ElementInputHandler::new(Bounds::default(), view.clone());
        // "a😀b": a is 0..1, 😀 1..3 (two UTF-16 units), b 3..4.
        ime.replace_and_mark_text_in_range(None, "a😀b", None, window, cx);
        let mut adjusted = None;
        // Starting inside the pair: the whole emoji, and the range says so.
        assert_eq!(ime.text_for_range(2..4, &mut adjusted, window, cx).as_deref(), Some("😀b"));
        assert_eq!(adjusted, Some(1..4));
        // Ending inside the pair takes the emoji in too.
        assert_eq!(ime.text_for_range(0..2, &mut adjusted, window, cx).as_deref(), Some("a😀"));
        assert_eq!(adjusted, Some(0..3));
        // Whole characters come back as asked.
        assert_eq!(ime.text_for_range(1..3, &mut adjusted, window, cx).as_deref(), Some("😀"));
        assert_eq!(adjusted, Some(1..3));
        assert_eq!(ime.text_for_range(3..99, &mut adjusted, window, cx).as_deref(), Some("b"));
        assert_eq!(adjusted, Some(3..4));
    });
}

#[test]
fn pty_sizes_are_exact_device_pixels() {
    let g = super::element::Geom { cols: 97, rows: 29, cell_w: px(8.203), line_h: px(17.0), scale: 1.5, ..Default::default() };
    let ws = g.window_size();
    // 8.203 * 1.5 = 12.3 -> 12 and 17 * 1.5 = 25.5 -> 26, not the logical 8 x 17.
    assert_eq!((ws.num_cols, ws.num_lines, ws.cell_width, ws.cell_height), (97, 29, 12, 26));
}

#[test]
fn term_stats_stay_apart_from_the_editor() {
    let shared = std::path::PathBuf::from("stats.json");
    let mut args = super::TermArgs::default();
    assert_eq!(args.stats_path(Some(&shared), false), Some(shared.clone()));
    assert_eq!(args.stats_path(Some(&shared), true), None, "the editor writes --stats");
    args.stats = Some("term.json".into());
    assert_eq!(args.stats_path(Some(&shared), true), Some("term.json".into()));
}
