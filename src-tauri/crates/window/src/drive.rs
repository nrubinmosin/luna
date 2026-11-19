//! The test build's driver (`LUNA_TEST_DRIVE`, src/native.rs): a file a smoke
//! script appends lines to, which the window reads every 200 ms and acts on,
//! through gpui's own dispatch, as if the keys had been pressed in it. Keys
//! with modifiers cannot be sent to a window by message (gpui reads the
//! modifiers off the keyboard's state), and global input is off limits, so a
//! smoke run types this way. Never on in any other build; the polling runs
//! only while it is.
//!
//! - `keys ctrl-c enter shift-pageup`: keystrokes, in gpui's notation.
//! - `text some words`: each character typed.
//! - `focus 2`: the keyboard into the board's second terminal.
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
            let lines: Vec<String> = text.lines().map(str::to_owned).collect();
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
                if let Some(pane) = view.read(cx).board.read(cx).panes.get(n).cloned() {
                    pane.update(cx, |p, cx| p.focus(window, cx));
                }
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
