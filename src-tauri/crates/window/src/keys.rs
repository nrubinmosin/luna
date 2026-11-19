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
//! - Esc: what is up over the board goes back, while the peek's frame has
//!   the keyboard; once its terminal has it, Esc is the CLI's (root.rs).
//!   Shift+Esc closes the peek whoever has the keyboard.
//!
//! The digits go by the physical key, as the page's `e.code` did, on any
//! layout. gpui on Windows names a key by the character the layout gives
//! it (vendor/gpui-pre-windows events.rs `parse_normal_key`): the digit
//! row's first key is `1` on US QWERTY but `&` on French AZERTY, and with
//! Shift it is named by its shifted character and the Shift dropped, so
//! Ctrl+Shift+1 comes as `ctrl-!` on QWERTY (which `ctrl-shift-1` never
//! matches) and as `ctrl-1` on AZERTY (which would be Ctrl+1); the keypad's
//! 1 comes as `1`. So on Windows `watch_digit_row` looks before the
//! bindings at which key is down (`GetKeyState`: VK_1 to VK_4, the same on
//! every layout, or the keypad's VK_NUMPAD1 to 4) and whether Shift is,
//! and decides; the bindings stay for every other platform and the tests.

#[cfg(any(windows, test))]
use crate::models::Models;
#[cfg(any(windows, test))]
use gpui::Keystroke;
use gpui::{App, KeyBinding, actions};

/// The key context of the window's own bindings (root.rs).
pub const CONTEXT: &str = "LunaWindow";

actions!(
    luna,
    [NewChat, QuickChat, Layout1, Layout2, Layout3, Layout4, Pane1, Pane2, Pane3, Pane4, NextWaiting, NextBoard, ClosePeek, ForceClosePeek]
);

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
        KeyBinding::new("shift-escape", ForceClosePeek, Some(CONTEXT)),
    ]);
}

/// The virtual keys `digit_key` looks at (Win32's numbering, the same on
/// every layout). Only Windows names keys so (and the tests).
#[cfg(any(windows, test))]
const VK_SHIFT: u16 = 0x10;
#[cfg(any(windows, test))]
const VK_0: u16 = 0x30;
#[cfg(any(windows, test))]
const VK_NUMPAD0: u16 = 0x60;

/// What a Ctrl+digit is by the key pressed.
#[cfg(any(windows, test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DigitKey {
    Pane(usize),
    Layout(usize),
}

/// `ks` as Ctrl+1 to 4 (or Ctrl+Shift+1 to 4) of the digit row or the
/// keypad, by the key that is down (`down`, a virtual key's state as the
/// keystroke came): whatever character the layout names it by, which must
/// be one character and no letter (a letter typed while a digit is held is
/// that letter's). None for any other.
#[cfg(any(windows, test))]
pub(crate) fn digit_key(ks: &Keystroke, down: impl Fn(u16) -> bool) -> Option<DigitKey> {
    let m = &ks.modifiers;
    if !m.control || m.alt || m.platform || m.function {
        return None;
    }
    let mut chars = ks.key.chars();
    if !matches!((chars.next(), chars.next()), (Some(c), None) if !c.is_alphabetic()) {
        return None;
    }
    let n = (1..=4u16).find(|n| down(VK_0 + n) || down(VK_NUMPAD0 + n))?;
    let n = usize::from(n);
    Some(if down(VK_SHIFT) { DigitKey::Layout(n) } else { DigitKey::Pane(n) })
}

/// Takes Ctrl+digit by the physical key before the bindings see it
/// (`digit_key`), for keys typed in the main window. One that means
/// nothing here goes on, to the bindings and the terminal. `down` reads a
/// virtual key's state: `GetKeyState` on Windows, a fake in the tests.
#[cfg(any(windows, test))]
pub(crate) fn watch_digit_row(models: Models, down: impl Fn(u16) -> bool + 'static, cx: &mut App) {
    cx.intercept_keystrokes(move |event, window, cx| {
        let home = &models.home;
        if !(home.is_focused(window) || home.contains_focused(window, cx)) {
            return;
        }
        match digit_key(&event.keystroke, &down) {
            Some(DigitKey::Layout(n)) => {
                models.workspace.update(cx, |w, cx| w.set_layout(n, cx));
                cx.stop_propagation();
            }
            // A digit that means nothing here goes on.
            Some(DigitKey::Pane(n)) if models.workspace.update(cx, |w, cx| w.digit(n, cx)) => cx.stop_propagation(),
            Some(DigitKey::Pane(_)) | None => {}
        }
    })
    .detach();
}

/// A virtual key's state as the keystroke being handled came (`GetKeyState`
/// is in step with the window's messages, not the keyboard's now).
#[cfg(windows)]
pub(crate) fn key_is_down(vk: u16) -> bool {
    // SAFETY: GetKeyState takes any virtual key and has no preconditions.
    unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState(i32::from(vk)) < 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ks(s: &'static str) -> Keystroke {
        Keystroke::parse(s).unwrap()
    }

    /// Each layout's name for the digit row's keys, by the key down.
    #[test]
    fn a_digit_is_its_key_whatever_the_layout_calls_it() {
        let one = |vk: u16| move |k: u16| k == vk;
        let one_shift = |vk: u16| move |k: u16| k == vk || k == VK_SHIFT;
        // US QWERTY: Ctrl+1, and Ctrl+Shift+1 named by its `!`.
        assert_eq!(digit_key(&ks("ctrl-1"), one(0x31)), Some(DigitKey::Pane(1)));
        assert_eq!(digit_key(&ks("ctrl-!"), one_shift(0x31)), Some(DigitKey::Layout(1)));
        // French AZERTY: the key is `&`, and with Shift `1`.
        assert_eq!(digit_key(&ks("ctrl-&"), one(0x31)), Some(DigitKey::Pane(1)));
        assert_eq!(digit_key(&ks("ctrl-1"), one_shift(0x31)), Some(DigitKey::Layout(1)));
        assert_eq!(digit_key(&ks("ctrl-'"), one(0x34)), Some(DigitKey::Pane(4)));
        // The keypad's.
        assert_eq!(digit_key(&ks("ctrl-3"), one(0x63)), Some(DigitKey::Pane(3)));
        // Not ours: 5 and up, no Ctrl, AltGr (Ctrl+Alt), a letter typed
        // while a digit is held, nothing down.
        assert_eq!(digit_key(&ks("ctrl-5"), one(0x35)), None);
        assert_eq!(digit_key(&ks("1"), one(0x31)), None);
        assert_eq!(digit_key(&ks("ctrl-alt-1"), one(0x31)), None);
        assert_eq!(digit_key(&ks("ctrl-n"), one(0x31)), None);
        assert_eq!(digit_key(&ks("ctrl-1"), |_| false), None);
    }
}
