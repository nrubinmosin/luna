//! The keys the window keeps for itself, with the meanings the page's
//! keymap gave them (src/app/keymap.ts). They are bound in the window's own
//! key context, an ancestor of every terminal's, so they reach the window
//! whatever has the keyboard; a terminal's own bindings (Ctrl+C, Ctrl+V)
//! are deeper and win where they overlap, which here they never do. A key
//! that means nothing at the moment (Ctrl+3 on a board of two panes, Esc
//! with nothing up over the board) goes on to the terminal as typed.
//!
//! - Ctrl+N: a new chat; Ctrl+Shift+N: a quick chat (the dialogs' seams).
//! - Ctrl+Shift+1 to 4: the layout.
//! - Ctrl+1 to 4: the keyboard into that pane; with one pane, the chat of
//!   that number in the list.
//! - Ctrl+Shift+W: the next chat waiting for the person.
//! - Ctrl+Tab: the next board. Ctrl+Shift+Tab is Claude Code's own.
//! - Esc: what is up over the board goes back.

use gpui::{App, KeyBinding, actions};

/// The key context of the window's own bindings (root.rs).
pub const CONTEXT: &str = "LunaWindow";

actions!(luna, [NewChat, QuickChat, Layout1, Layout2, Layout3, Layout4, Pane1, Pane2, Pane3, Pane4, NextWaiting, NextBoard, ClosePeek]);

pub fn bind(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-n", NewChat, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-n", QuickChat, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-1", Layout1, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-2", Layout2, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-3", Layout3, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-4", Layout4, Some(CONTEXT)),
        KeyBinding::new("ctrl-1", Pane1, Some(CONTEXT)),
        KeyBinding::new("ctrl-2", Pane2, Some(CONTEXT)),
        KeyBinding::new("ctrl-3", Pane3, Some(CONTEXT)),
        KeyBinding::new("ctrl-4", Pane4, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-w", NextWaiting, Some(CONTEXT)),
        KeyBinding::new("ctrl-tab", NextBoard, Some(CONTEXT)),
        KeyBinding::new("escape", ClosePeek, Some(CONTEXT)),
    ]);
}
