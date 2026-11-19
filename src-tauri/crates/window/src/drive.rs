//! The test build's driver (`LUNA_TEST_DRIVE`, src/native.rs): a file a smoke
//! script appends lines to, which the window reads every 200 ms and acts on,
//! through gpui's own dispatch, as if the keys had been pressed in it. Keys
//! with modifiers cannot be sent to a window by message (gpui reads the
//! modifiers off the keyboard's state), and global input is off limits, so a
//! smoke run types this way. Compiled into the test build alone (`luna_test`);
//! the polling runs only while a file is named. A line runs once it is whole,
//! its newline written: one read while it is being appended waits for it.
//!
//! - `keys ctrl-c enter shift-pageup`: keystrokes, in gpui's notation.
//! - `text some words`: each character typed.
//! - `focus 2`: the keyboard into the terminal of the board's second pane.
//! - `row 3`: a click on the third chat of the list (the numbers Ctrl+digit
//!   counts by), as on its row.
//! - `put 2 3`: the list's third chat into pane 2, as a pane's menu does;
//!   `swap 1 2`, `unseat 1` likewise; `toboard`: the peek's To board.
//! - `dump`: the board's state into Luna's log, for the script to read, with
//!   what has the keyboard (the peek's frame, a terminal, the window).
//! - `close`: the window closes, as its close button would.

use crate::root::LunaWindow;
use gpui::{AnyWindowHandle, App, AsyncApp, Entity, Keystroke, Modifiers, OperatingSystem};
use std::path::PathBuf;
use std::time::Duration;

/// How often the file is read.
const EVERY: Duration = Duration::from_millis(200);

/// The keystrokes' notation is this platform's.
const OS: OperatingSystem = if cfg!(target_os = "macos") {
    OperatingSystem::Mac
} else if cfg!(windows) {
    OperatingSystem::Windows
} else {
    OperatingSystem::Linux
};

pub fn start(path: PathBuf, window: AnyWindowHandle, view: Entity<LunaWindow>, cx: &mut App) {
    cx.spawn(async move |cx| {
        let mut done = 0;
        loop {
            cx.background_executor().timer(EVERY).await;
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            let whole = text.rfind('\n').map_or("", |end| &text[..end + 1]);
            let lines: Vec<String> = whole.lines().map(str::to_owned).collect();
            for line in lines.iter().skip(done) {
                if !run(line, window, &view, cx) {
                    return;
                }
            }
            done = lines.len();
        }
    })
    .detach();
}

/// One line; false once the window is gone.
fn run(line: &str, window: AnyWindowHandle, view: &Entity<LunaWindow>, cx: &mut AsyncApp) -> bool {
    let (verb, rest) = line.split_once(' ').unwrap_or((line, ""));
    let keys: Vec<Keystroke> = match verb {
        "keys" => rest
            .split_whitespace()
            .filter_map(|k| match Keystroke::parse_with_os(k, OS) {
                Ok(k) => Some(k),
                Err(e) => {
                    log::warn!("drive: {k}: {e}");
                    None
                }
            })
            .collect(),
        "text" => rest
            .chars()
            .map(|c| {
                let key = if c == ' ' { "space".to_owned() } else { c.to_string() };
                Keystroke { modifiers: Modifiers::default(), key, key_char: Some(c.to_string()) }
            })
            .collect(),
        _ => Vec::new(),
    };
    let view = view.clone();
    window
        .update(cx, |_, window, cx| match verb {
            "focus" => {
                let n = rest.trim().parse::<usize>().unwrap_or(1).saturating_sub(1);
                let board = view.read(cx).board.clone();
                if let Some(pane) = board.read(cx).terminal_in(n, cx) {
                    pane.update(cx, |p, cx| p.focus(window, cx));
                }
            }
            "row" | "put" | "swap" | "unseat" | "toboard" | "dump" => {
                let ws = view.read(cx).models().workspace.clone();
                let board = view.read(cx).board.read(cx);
                let keyboard = if board.peek_focus.is_focused(window) {
                    "peek"
                } else if board.terminal_has_keyboard(window, cx) {
                    "terminal"
                } else {
                    "window"
                };
                let nums: Vec<usize> = rest.split_whitespace().filter_map(|n| n.parse().ok()).collect();
                let listed = |ws: &crate::workspace::Workspace, n: usize| ws.state.numbered(ws.state.board).get(n.wrapping_sub(1)).cloned();
                ws.update(cx, |w, cx| match (verb, nums.as_slice()) {
                    ("row", [n]) => {
                        if let Some(id) = listed(w, *n) {
                            w.show_chat(&id, cx);
                        }
                    }
                    ("put", [pane, n]) => {
                        if let Some(id) = listed(w, *n) {
                            w.put(pane.wrapping_sub(1), &id, cx);
                        }
                    }
                    ("swap", [a, b]) => w.swap(a.wrapping_sub(1), b.wrapping_sub(1), cx),
                    ("unseat", [pane]) => w.unseat(pane.wrapping_sub(1), cx),
                    ("toboard", []) => {
                        if let Some(id) = w.state.peeked_chat().map(str::to_owned) {
                            w.to_board(&id, cx);
                        }
                    }
                    ("dump", []) => {
                        let s = &w.state;
                        let seats: Vec<Option<&str>> = (0..s.layout()).map(|i| s.seat(i)).collect();
                        log::warn!(
                            "drive: board {} layout {} seats {:?} peek {:?} keyboard {keyboard} active pane {} chats {} folders {} statuses {:?}",
                            s.board + 1,
                            s.layout(),
                            seats,
                            s.peeked_chat(),
                            s.active_pane + 1,
                            s.chats().count(),
                            s.folders.len(),
                            s.chats().map(|c| c.status.word()).collect::<Vec<_>>()
                        );
                    }
                    _ => log::warn!("drive: {line}: not understood"),
                });
            }
            "close" => window.remove_window(),
            _ => {
                for k in keys {
                    window.dispatch_keystroke(k, cx);
                }
            }
        })
        .is_ok()
}
