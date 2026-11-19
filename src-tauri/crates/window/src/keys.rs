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
//! A digit held is not enough: the keystroke must be that key's own, by the
//! name the layout gives it with Shift or without (`key_names`), as gpui
//! names the keystroke from it. Its key code is not on the keystroke gpui
//! hands over, so a Ctrl+/ typed while 1 is held would otherwise be taken
//! for Ctrl+1, and the terminal would lose its control byte.

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
/// keypad: a digit's key that is down (`down`, a virtual key's state as the
/// keystroke came) and whose name is the keystroke's (`names`, what the
/// layout calls that key, with Shift and without), whatever character that
/// is. None for any other: a key typed while a digit is held is that key's.
#[cfg(any(windows, test))]
pub(crate) fn digit_key(ks: &Keystroke, down: impl Fn(u16) -> bool, names: impl Fn(u16) -> Vec<String>) -> Option<DigitKey> {
    let m = &ks.modifiers;
    if !m.control || m.alt || m.platform || m.function {
        return None;
    }
    let pressed = |vk: u16| down(vk) && names(vk).iter().any(|name| *name == ks.key);
    let n = (1..=4u16).find(|n| pressed(VK_0 + n) || pressed(VK_NUMPAD0 + n))?;
    let n = usize::from(n);
    Some(if down(VK_SHIFT) { DigitKey::Layout(n) } else { DigitKey::Pane(n) })
}

/// Takes Ctrl+digit by the physical key before the bindings see it
/// (`digit_key`), for keys typed in the main window. One that means
/// nothing here goes on, to the bindings and the terminal. `down` reads a
/// virtual key's state and `names` what the layout calls a key:
/// `GetKeyState` and `key_names` on Windows, fakes in the tests.
#[cfg(any(windows, test))]
pub(crate) fn watch_digit_row(
    models: Models,
    down: impl Fn(u16) -> bool + 'static,
    names: impl Fn(u16) -> Vec<String> + 'static,
    cx: &mut App,
) {
    cx.intercept_keystrokes(move |event, window, cx| {
        let home = &models.home;
        if !(home.is_focused(window) || home.contains_focused(window, cx)) {
            return;
        }
        match digit_key(&event.keystroke, &down, &names) {
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

/// What the keyboard layout in use calls the key `vk`, as gpui names a
/// keystroke from it (vendor/gpui-pre-windows keyboard.rs): its character
/// without Shift, lowercased, and with Shift; a keypad digit is its digit.
/// Asked of the window thread's layout, without changing the keyboard's
/// state (a dead key stays as it was).
#[cfg(windows)]
pub(crate) fn key_names(vk: u16) -> Vec<String> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetKeyboardLayout, MAPVK_VK_TO_CHAR, MAPVK_VK_TO_VSC_EX, MapVirtualKeyExW, ToUnicodeEx,
    };
    if (VK_NUMPAD0..=VK_NUMPAD0 + 9).contains(&vk) {
        return vec![(vk - VK_NUMPAD0).to_string()];
    }
    let mut names = Vec::new();
    // SAFETY: these take any virtual key and the calling thread's layout;
    // ToUnicodeEx writes only into the buffer it is given, and with flag
    // 0x4 leaves the keyboard's state alone.
    unsafe {
        let layout = GetKeyboardLayout(0);
        let plain = MapVirtualKeyExW(u32::from(vk), MAPVK_VK_TO_CHAR, Some(layout)) & 0xFFFF;
        if let Some(c) = char::from_u32(plain).filter(|_| plain != 0) {
            names.push(c.to_lowercase().to_string());
        }
        let scan = MapVirtualKeyExW(u32::from(vk), MAPVK_VK_TO_VSC_EX, Some(layout));
        for shift in [false, true] {
            let mut state = [0u8; 256];
            if shift {
                state[usize::from(VK_SHIFT)] = 0x80;
            }
            let mut buf = [0u16; 8];
            let n = ToUnicodeEx(u32::from(vk), scan, &state, &mut buf, 0x4, Some(layout));
            if n != 0 {
                let len = usize::try_from(n.unsigned_abs()).unwrap_or(0).min(buf.len());
                names.push(String::from_utf16_lossy(&buf[..len.max(1)]));
            }
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ks(s: &'static str) -> Keystroke {
        Keystroke::parse(s).unwrap()
    }

    /// What US QWERTY and French AZERTY call the digit row's first four
    /// keys, without Shift and with it; the keypad's are their digits.
    fn qwerty(vk: u16) -> Vec<String> {
        let names = match vk {
            0x31 => ["1", "!"],
            0x32 => ["2", "@"],
            0x33 => ["3", "#"],
            0x34 => ["4", "$"],
            0x61..=0x64 => return vec![(vk - VK_NUMPAD0).to_string()],
            _ => return Vec::new(),
        };
        names.iter().map(|n| n.to_string()).collect()
    }

    fn azerty(vk: u16) -> Vec<String> {
        let names = match vk {
            0x31 => ["&", "1"],
            0x32 => ["é", "2"],
            0x33 => ["\"", "3"],
            0x34 => ["'", "4"],
            _ => return qwerty(vk),
        };
        names.iter().map(|n| n.to_string()).collect()
    }

    /// Each layout's name for the digit row's keys, by the key down.
    #[test]
    fn a_digit_is_its_key_whatever_the_layout_calls_it() {
        let one = |vk: u16| move |k: u16| k == vk;
        let one_shift = |vk: u16| move |k: u16| k == vk || k == VK_SHIFT;
        // US QWERTY: Ctrl+1, and Ctrl+Shift+1 named by its `!`.
        assert_eq!(digit_key(&ks("ctrl-1"), one(0x31), qwerty), Some(DigitKey::Pane(1)));
        assert_eq!(digit_key(&ks("ctrl-!"), one_shift(0x31), qwerty), Some(DigitKey::Layout(1)));
        // French AZERTY: the key is `&`, and with Shift `1`.
        assert_eq!(digit_key(&ks("ctrl-&"), one(0x31), azerty), Some(DigitKey::Pane(1)));
        assert_eq!(digit_key(&ks("ctrl-1"), one_shift(0x31), azerty), Some(DigitKey::Layout(1)));
        assert_eq!(digit_key(&ks("ctrl-'"), one(0x34), azerty), Some(DigitKey::Pane(4)));
        // The keypad's.
        assert_eq!(digit_key(&ks("ctrl-3"), one(0x63), qwerty), Some(DigitKey::Pane(3)));
        // Not ours: 5 and up, no Ctrl, AltGr (Ctrl+Alt), a letter typed
        // while a digit is held, nothing down.
        assert_eq!(digit_key(&ks("ctrl-5"), one(0x35), qwerty), None);
        assert_eq!(digit_key(&ks("1"), one(0x31), qwerty), None);
        assert_eq!(digit_key(&ks("ctrl-alt-1"), one(0x31), qwerty), None);
        assert_eq!(digit_key(&ks("ctrl-n"), one(0x31), qwerty), None);
        assert_eq!(digit_key(&ks("ctrl-1"), |_| false, qwerty), None);
    }

    /// A key typed while a digit is held is that key's, not the digit's:
    /// Ctrl+/ with 1 down is the terminal's; of two digits down, the one
    /// whose name the keystroke has is the one pressed.
    #[test]
    fn a_digit_held_does_not_take_another_keys_keystroke() {
        let held = |vks: &'static [u16]| move |k: u16| vks.contains(&k);
        assert_eq!(digit_key(&ks("ctrl-/"), held(&[0x31]), qwerty), None);
        assert_eq!(digit_key(&ks("ctrl-shift-/"), held(&[0x10, 0x31]), qwerty), None);
        assert_eq!(digit_key(&ks("ctrl-2"), held(&[0x31, 0x32]), qwerty), Some(DigitKey::Pane(2)));
        assert_eq!(digit_key(&ks("ctrl-é"), held(&[0x31, 0x32]), azerty), Some(DigitKey::Pane(2)));
        // 1 held on AZERTY, and the key that types `1` on QWERTY's keypad.
        assert_eq!(digit_key(&ks("ctrl-1"), held(&[0x31, 0x61]), azerty), Some(DigitKey::Pane(1)));
    }
}
