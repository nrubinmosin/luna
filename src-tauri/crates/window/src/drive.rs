//! The test build's driver (`LUNA_TEST_DRIVE`, src/native.rs): a file a smoke
//! script appends lines to, which the window reads every 200 ms and acts on,
//! through gpui's own dispatch, as if the keys had been pressed in it. Keys
//! with modifiers cannot be sent to a window by message (gpui reads the
//! modifiers off the keyboard's state), and global input is off limits, so a
//! smoke run types this way. Compiled into the test build alone (`luna_test`);
//! the polling runs only while a file is named. A line runs once it is whole,
//! its newline written: one read while it is being appended waits for it.
//!
//! - `keys ctrl-s escape`: keystrokes, in gpui's notation.
//! - `text some words`: each character typed.
//! - `close`: the window closes, as its close button would.
//! - the panels' own (panels/mod.rs `drive`): `panel library`, `select skill
//!   review`, `save`, `setting sound off`, `cli-check claude`, ...

use gpui::{AnyWindowHandle, App, AsyncApp, Keystroke, Modifiers, OperatingSystem};
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

pub fn start(path: PathBuf, window: AnyWindowHandle, cx: &mut App) {
    cx.spawn(async move |cx| {
        let mut done = 0;
        loop {
            cx.background_executor().timer(EVERY).await;
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            let whole = text.rfind('\n').map_or("", |end| &text[..end + 1]);
            let lines: Vec<String> = whole.lines().map(str::to_owned).collect();
            for line in lines.iter().skip(done) {
                if !run(line, window, cx) {
                    return;
                }
            }
            done = lines.len();
        }
    })
    .detach();
}

/// One line; false once the window is gone.
fn run(line: &str, window: AnyWindowHandle, cx: &mut AsyncApp) -> bool {
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
    window
        .update(cx, |_, window, cx| match verb {
            "close" => window.remove_window(),
            "keys" | "text" => {
                for k in keys {
                    window.dispatch_keystroke(k, cx);
                }
            }
            _ => {
                if !crate::panels::drive(verb, rest.trim(), window, cx) {
                    log::warn!("drive: {line}: nothing does that");
                }
            }
        })
        .is_ok()
}
