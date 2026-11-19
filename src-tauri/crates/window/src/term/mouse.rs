//! Mouse reporting, from xterm's "Mouse Tracking": which events a program
//! asked for (modes 1000 click, 1002 drag, 1003 any motion) and how they are
//! encoded (SGR, mode 1006; UTF-8, mode 1005; or the original X10-style
//! bytes, as far as one byte of text carries them).

use super::keys::Mods;
use alacritty_terminal::term::TermMode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
    /// No button: motion under mode 1003.
    None,
    WheelUp,
    WheelDown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Press,
    Release,
    Motion,
}

/// The program wants mouse events at all.
pub fn reporting(mode: TermMode) -> bool {
    mode.intersects(TermMode::MOUSE_MODE)
}

/// This event is one the active mode reports.
pub fn wants(mode: TermMode, action: Action, held: bool) -> bool {
    match action {
        Action::Press | Action::Release => mode.intersects(TermMode::MOUSE_MODE),
        Action::Motion if mode.contains(TermMode::MOUSE_MOTION) => true,
        Action::Motion => held && mode.contains(TermMode::MOUSE_DRAG),
    }
}

/// The furthest cell, 0-based, the X10 encoding reports: its coordinate
/// bytes are raw bytes, 32 + 1 + the cell, and the session takes text, which
/// would turn one past 127 into two bytes of UTF-8. Past it, a position is
/// reported at this cell, as xterm does at its own limit of 223.
pub const X10_LAST: usize = 127 - 33;

/// The furthest cell, 0-based, the UTF-8 encoding (mode 1005) reports: its
/// coordinates are characters, up to U+07FF.
pub const UTF8_LAST: usize = 0x7ff - 33;

/// The report for one event at 0-based cell (col, line), as the text the
/// session is sent: SGR (mode 1006) at any cell; UTF-8 (1005) and X10 with
/// the position held to `UTF8_LAST` and `X10_LAST`.
pub fn encode(mode: TermMode, button: Button, action: Action, mods: Mods, col: usize, line: usize) -> Option<String> {
    let mut code: u32 = match button {
        Button::Left => 0,
        Button::Middle => 1,
        Button::Right => 2,
        Button::None => 3,
        Button::WheelUp => 64,
        Button::WheelDown => 65,
    };
    if action == Action::Motion {
        code += 32;
    }
    code += 4 * mods.shift as u32 + 8 * mods.alt as u32 + 16 * mods.ctrl as u32;
    if mode.contains(TermMode::SGR_MOUSE) {
        let fin = if action == Action::Release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{code};{};{}{fin}", col + 1, line + 1));
    }
    // X10 style: a release is button 3, whichever was released.
    if action == Action::Release {
        code = (code & !3) | 3;
    }
    let last = if mode.contains(TermMode::UTF8_MOUSE) { UTF8_LAST } else { X10_LAST };
    let (x, y) = (col.min(last) as u32 + 1 + 32, line.min(last) as u32 + 1 + 32);
    [32 + code, x, y].into_iter().map(char::from_u32).collect::<Option<String>>().map(|tail| format!("\x1b[M{tail}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SGR: TermMode = TermMode::SGR_MOUSE.union(TermMode::MOUSE_REPORT_CLICK);

    #[test]
    fn sgr_press_release_and_wheel() {
        let none = Mods::NONE;
        assert_eq!(encode(SGR, Button::Left, Action::Press, none, 0, 0).unwrap(), "\x1b[<0;1;1M");
        assert_eq!(encode(SGR, Button::Left, Action::Release, none, 4, 2).unwrap(), "\x1b[<0;5;3m");
        assert_eq!(encode(SGR, Button::Right, Action::Press, none, 9, 9).unwrap(), "\x1b[<2;10;10M");
        assert_eq!(encode(SGR, Button::WheelUp, Action::Press, none, 0, 0).unwrap(), "\x1b[<64;1;1M");
        assert_eq!(encode(SGR, Button::WheelDown, Action::Press, none, 0, 0).unwrap(), "\x1b[<65;1;1M");
        let ctrl = Mods { ctrl: true, ..Mods::NONE };
        assert_eq!(encode(SGR, Button::Left, Action::Motion, ctrl, 1, 1).unwrap(), "\x1b[<48;2;2M");
    }

    #[test]
    fn x10_encoding() {
        let m = TermMode::MOUSE_REPORT_CLICK;
        assert_eq!(encode(m, Button::Left, Action::Press, Mods::NONE, 0, 0).unwrap(), "\x1b[M !!");
        assert_eq!(encode(m, Button::Left, Action::Release, Mods::NONE, 0, 0).unwrap(), "\x1b[M#!!");
        // The 95th column is the last a single byte of text reports; past it,
        // the report is held there, every byte of it still ASCII.
        assert_eq!(encode(m, Button::Left, Action::Press, Mods::NONE, 94, 0).unwrap(), "\x1b[M \x7f!");
        let far = encode(m, Button::Left, Action::Press, Mods::NONE, 100, 300).unwrap();
        assert_eq!(far, "\x1b[M \x7f\x7f");
        assert!(far.is_ascii());
        // Mode 1005: coordinates as UTF-8 characters, as far as U+07FF.
        let utf8 = m | TermMode::UTF8_MOUSE;
        assert_eq!(encode(utf8, Button::Left, Action::Press, Mods::NONE, 100, 0).unwrap(), "\x1b[M \u{85}!");
        assert_eq!(encode(utf8, Button::Left, Action::Press, Mods::NONE, 5000, 0).unwrap(), "\x1b[M \u{7ff}!");
    }

    #[test]
    fn modes_decide_what_is_reported() {
        let click = TermMode::MOUSE_REPORT_CLICK;
        let drag = TermMode::MOUSE_DRAG;
        let any = TermMode::MOUSE_MOTION;
        assert!(wants(click, Action::Press, false));
        assert!(!wants(click, Action::Motion, true));
        assert!(wants(drag, Action::Motion, true));
        assert!(!wants(drag, Action::Motion, false));
        assert!(wants(any, Action::Motion, false));
        assert!(!reporting(TermMode::empty()));
    }
}
