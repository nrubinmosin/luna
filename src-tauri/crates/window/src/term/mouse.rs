//! Mouse reporting, from xterm's "Mouse Tracking": which events a program
//! asked for (modes 1000 click, 1002 drag, 1003 any motion) and how they are
//! encoded (SGR, mode 1006, or the original X10-style bytes).

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

/// The report for one event at 0-based cell (col, line), as the text the
/// session is sent; None when the X10 encoding cannot express the position
/// (past column or row 223). X10's coordinate bytes past 127 go as the
/// characters of those code points, as xterm.js sends them.
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
    let (x, y) = (col as u32 + 1 + 32, line as u32 + 1 + 32);
    if x > 255 || y > 255 {
        return None;
    }
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
        // Past 95 columns a coordinate is a character above ASCII.
        assert_eq!(encode(m, Button::Left, Action::Press, Mods::NONE, 100, 0).unwrap(), "\x1b[M \u{85}!");
        assert_eq!(encode(m, Button::Left, Action::Press, Mods::NONE, 300, 0), None);
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
