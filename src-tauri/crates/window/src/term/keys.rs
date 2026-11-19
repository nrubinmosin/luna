//! Key encoding for the session's PTY, written from the xterm
//! control-sequence reference ("PC-Style Function Keys", "Alt and Meta keys",
//! DECCKM/DECKPAM): what bytes a key press sends, given the modifiers and the
//! terminal's modes, and the paste framing.
//!
//! Plain printable keys are not encoded here: they reach the terminal as text
//! through gpui's InputHandler (WM_CHAR, IME commits, dead keys, AltGr), so
//! `encode` returns None for them and the key event is left to propagate.
//! What the pane does with a key before it gets here (copy, paste, the
//! scrollback) is the view's (mod.rs `key_down`).

/// The modifiers that change what a key sends.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

impl Mods {
    pub const NONE: Mods = Mods { shift: false, alt: false, ctrl: false };

    /// xterm's modifier parameter: 1 + shift(1) + alt(2) + ctrl(4).
    fn param(self) -> u8 {
        1 + self.shift as u8 + 2 * self.alt as u8 + 4 * self.ctrl as u8
    }

    fn any(self) -> bool {
        self.shift || self.alt || self.ctrl
    }
}

/// The terminal modes key encoding depends on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modes {
    /// DECCKM (CSI ? 1 h): cursor keys send SS3 instead of CSI.
    pub app_cursor: bool,
    /// DECKPAM (ESC =): the keypad sends SS3 sequences.
    pub app_keypad: bool,
    /// LNM (CSI 20 h): Enter sends CR LF.
    pub newline: bool,
}

/// One key press as gpui reports it: `key` is gpui's key name ("a", "enter",
/// "f5", "up"), `key_char` the character it would type, if any.
pub struct Key<'a> {
    pub key: &'a str,
    pub key_char: Option<&'a str>,
    pub mods: Mods,
    /// The platform wants this key as text (AltGr, dead keys on Windows).
    pub prefer_text: bool,
}

/// The bytes the key sends, or None when it is text for the InputHandler (or
/// nothing a terminal sends).
pub fn encode(k: &Key, modes: Modes) -> Option<Vec<u8>> {
    let m = k.mods;
    // Text the platform wants typed (AltGr's characters, AltGr+Space's on a
    // layout that has one, a dead key's) before any named key: AltGr comes
    // as Ctrl+Alt, which would make AltGr+Space ESC NUL.
    if k.prefer_text && k.key_char.is_some_and(|t| !t.is_empty()) {
        return None;
    }
    if let Some(seq) = alt_arrow(k.key, m) {
        return Some(seq.to_vec());
    }
    if let Some(seq) = named(k.key, m, modes) {
        return Some(seq);
    }
    if k.prefer_text {
        return None;
    }
    // A character key: gpui names it by its unshifted character.
    let mut chars = k.key.chars();
    let (Some(base), None) = (chars.next(), chars.next()) else {
        return None;
    };
    if m.ctrl {
        let c = ctrl_byte(base, m.shift)?;
        let mut out = Vec::with_capacity(2);
        if m.alt {
            out.push(0x1b);
        }
        out.push(c);
        return Some(out);
    }
    if m.alt {
        // Alt sends ESC before what the key types (xterm's metaSendsEscape).
        let text = match k.key_char {
            Some(t) if !t.is_empty() => t.to_string(),
            _ if m.shift => base.to_uppercase().to_string(),
            _ => base.to_string(),
        };
        let mut out = vec![0x1b];
        out.extend_from_slice(text.as_bytes());
        return Some(out);
    }
    None
}

/// Alt+arrow alone as the WebView window's terminal sends it (xterm.js up to
/// 5, kept by src/features/panes/terminal/keys.ts): as Ctrl+arrow, so
/// Alt+Left and Alt+Right move a word in the CLI's input; on macOS ESC b and
/// ESC f, the same moves there, and Alt+Up/Down as xterm has them.
fn alt_arrow(key: &str, m: Mods) -> Option<&'static [u8]> {
    if m != (Mods { alt: true, ..Mods::NONE }) {
        return None;
    }
    let mac = cfg!(target_os = "macos");
    Some(match key {
        "left" if mac => b"\x1bb",
        "right" if mac => b"\x1bf",
        _ if mac => return None,
        "left" => b"\x1b[1;5D",
        "right" => b"\x1b[1;5C",
        "up" => b"\x1b[1;5A",
        "down" => b"\x1b[1;5B",
        _ => return None,
    })
}

/// Ctrl+key as a C0 control byte, as xterm maps it.
fn ctrl_byte(c: char, shift: bool) -> Option<u8> {
    Some(match c {
        'a'..='z' => c as u8 - b'a' + 1,
        'A'..='Z' => c as u8 - b'A' + 1,
        '@' | '2' | ' ' => 0x00,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' => 0x1e,
        '_' | '7' | '/' => 0x1f,
        '-' if shift => 0x1f,
        '8' | '?' => 0x7f,
        _ => return None,
    })
}

fn csi(params: &str, fin: char) -> Vec<u8> {
    format!("\x1b[{params}{fin}").into_bytes()
}

/// CSI 1 ; m X with modifiers, else SS3 X (application) or CSI X (normal).
fn cursor_key(fin: char, m: Mods, app: bool) -> Vec<u8> {
    if m.any() {
        csi(&format!("1;{}", m.param()), fin)
    } else if app {
        format!("\x1bO{fin}").into_bytes()
    } else {
        csi("", fin)
    }
}

/// CSI n ~ or CSI n ; m ~.
fn tilde(n: u8, m: Mods) -> Vec<u8> {
    if m.any() { csi(&format!("{n};{}", m.param()), '~') } else { csi(&n.to_string(), '~') }
}

fn with_alt(m: Mods, bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + 1);
    if m.alt {
        out.push(0x1b);
    }
    out.extend_from_slice(bytes);
    out
}

fn named(key: &str, m: Mods, modes: Modes) -> Option<Vec<u8>> {
    Some(match key {
        "enter" => with_alt(m, if modes.newline { b"\r\n" } else { b"\r" }),
        "tab" if m.shift => csi("", 'Z'),
        "tab" => with_alt(m, b"\t"),
        // DEL, as xterm and every modern terminal send; Ctrl+Backspace is BS.
        "backspace" if m.ctrl => with_alt(m, b"\x08"),
        "backspace" => with_alt(m, b"\x7f"),
        "escape" => with_alt(m, b"\x1b"),
        "space" if m.ctrl => with_alt(m, b"\x00"),
        "space" if m.alt => b"\x1b ".to_vec(),
        "up" => cursor_key('A', m, modes.app_cursor),
        "down" => cursor_key('B', m, modes.app_cursor),
        "right" => cursor_key('C', m, modes.app_cursor),
        "left" => cursor_key('D', m, modes.app_cursor),
        "home" => cursor_key('H', m, modes.app_cursor),
        "end" => cursor_key('F', m, modes.app_cursor),
        "insert" => tilde(2, m),
        "delete" => tilde(3, m),
        "pageup" => tilde(5, m),
        "pagedown" => tilde(6, m),
        // F1-F4 are SS3 P-S unmodified, CSI 1 ; m P-S modified.
        "f1" => cursor_key('P', m, true),
        "f2" => cursor_key('Q', m, true),
        "f3" => cursor_key('R', m, true),
        "f4" => cursor_key('S', m, true),
        "f5" => tilde(15, m),
        "f6" => tilde(17, m),
        "f7" => tilde(18, m),
        "f8" => tilde(19, m),
        "f9" => tilde(20, m),
        "f10" => tilde(21, m),
        "f11" => tilde(23, m),
        "f12" => tilde(24, m),
        k if k.starts_with("numpad") => keypad(&k["numpad".len()..], m, modes)?,
        _ => return None,
    })
}

/// The keypad (DECKPAM): SS3 p..y, SS3 M (Enter), SS3 k/m/j/o/n for + - * / .
/// in application mode, the plain characters otherwise. gpui-pre 0.3.12's
/// Keystroke does not tell keypad keys from the main ones on any platform, so
/// nothing produces "numpad*" names yet: this is the encoder half, ready for a
/// platform layer that reports them.
fn keypad(name: &str, m: Mods, modes: Modes) -> Option<Vec<u8>> {
    let (app, plain): (char, &str) = match name {
        "0" => ('p', "0"),
        "1" => ('q', "1"),
        "2" => ('r', "2"),
        "3" => ('s', "3"),
        "4" => ('t', "4"),
        "5" => ('u', "5"),
        "6" => ('v', "6"),
        "7" => ('w', "7"),
        "8" => ('x', "8"),
        "9" => ('y', "9"),
        "enter" => ('M', "\r"),
        "add" => ('k', "+"),
        "subtract" => ('m', "-"),
        "multiply" => ('j', "*"),
        "divide" => ('o', "/"),
        "decimal" => ('n', "."),
        _ => return None,
    };
    if modes.app_keypad {
        Some(if m.any() { format!("\x1bO{}{app}", m.param()).into_bytes() } else { format!("\x1bO{app}").into_bytes() })
    } else {
        Some(with_alt(m, plain.as_bytes()))
    }
}

/// Text pasted into the terminal: line ends become CR (what Enter sends), and
/// with bracketed paste (mode 2004) on, it is framed by CSI 200~ / CSI 201~.
/// In both modes every control character but tab and the line ends is
/// dropped, C0 and C1 alike: an ESC would end the paste early and type keys
/// of its own (so would CSI's C1 form, U+009B), an ETX is the end of a paste
/// to some shells (alacritty 0.17.0 filters both, event.rs `paste`), and a
/// ^C or ^D in a paste would act as the key.
pub fn paste(text: &str, bracketed: bool) -> String {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    let clean: String = text.chars().filter(|&c| !pasted_control(c)).collect();
    if bracketed { format!("\x1b[200~{clean}\x1b[201~") } else { clean }
}

/// A character a paste leaves out: C0 but tab and CR, and C1.
fn pasted_control(c: char) -> bool {
    matches!(c, '\0'..='\x1f' | '\u{80}'..='\u{9f}') && !matches!(c, '\t' | '\r')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: &str, mods: Mods) -> Option<Vec<u8>> {
        enc(key, None, mods, Modes::default())
    }

    fn enc(key: &str, key_char: Option<&str>, mods: Mods, modes: Modes) -> Option<Vec<u8>> {
        encode(&Key { key, key_char, mods, prefer_text: false }, modes)
    }

    const SHIFT: Mods = Mods { shift: true, alt: false, ctrl: false };
    const ALT: Mods = Mods { shift: false, alt: true, ctrl: false };
    const CTRL: Mods = Mods { shift: false, alt: false, ctrl: true };
    const CTRL_SHIFT: Mods = Mods { shift: true, alt: false, ctrl: true };
    const CTRL_ALT: Mods = Mods { shift: false, alt: true, ctrl: true };
    const ALL: Mods = Mods { shift: true, alt: true, ctrl: true };

    fn s(b: &[u8]) -> Option<Vec<u8>> {
        Some(b.to_vec())
    }

    #[test]
    fn plain_printable_keys_are_left_to_text_input() {
        assert_eq!(enc("a", Some("a"), Mods::NONE, Modes::default()), None);
        assert_eq!(enc("a", Some("A"), SHIFT, Modes::default()), None);
        assert_eq!(enc("1", Some("!"), SHIFT, Modes::default()), None);
        // AltGr on Windows arrives as ctrl+alt with prefer_text.
        let altgr = Key { key: "q", key_char: Some("@"), mods: CTRL_ALT, prefer_text: true };
        assert_eq!(encode(&altgr, Modes::default()), None);
        // AltGr+Space on a layout that types a no-break space there: the
        // text, never Ctrl+Alt+Space's ESC NUL.
        let altgr_space = Key { key: "space", key_char: Some("\u{a0}"), mods: CTRL_ALT, prefer_text: true };
        assert_eq!(encode(&altgr_space, Modes::default()), None);
        // Ctrl+Alt+Space that types nothing is still ESC NUL.
        assert_eq!(key("space", CTRL_ALT), s(b"\x1b\x00"));
    }

    #[test]
    fn editing_keys() {
        assert_eq!(key("enter", Mods::NONE), s(b"\r"));
        assert_eq!(key("enter", ALT), s(b"\x1b\r"));
        let lnm = Modes { newline: true, ..Default::default() };
        assert_eq!(enc("enter", None, Mods::NONE, lnm), s(b"\r\n"));
        assert_eq!(key("tab", Mods::NONE), s(b"\t"));
        assert_eq!(key("tab", SHIFT), s(b"\x1b[Z"));
        assert_eq!(key("backspace", Mods::NONE), s(b"\x7f"));
        assert_eq!(key("backspace", CTRL), s(b"\x08"));
        assert_eq!(key("backspace", ALT), s(b"\x1b\x7f"));
        assert_eq!(key("escape", Mods::NONE), s(b"\x1b"));
        assert_eq!(key("space", CTRL), s(b"\x00"));
        assert_eq!(key("space", ALT), s(b"\x1b "));
        assert_eq!(key("space", Mods::NONE), None);
    }

    #[test]
    fn cursor_keys_follow_decckm() {
        let app = Modes { app_cursor: true, ..Default::default() };
        for (k, f) in [("up", 'A'), ("down", 'B'), ("right", 'C'), ("left", 'D'), ("home", 'H'), ("end", 'F')] {
            assert_eq!(key(k, Mods::NONE), Some(format!("\x1b[{f}").into_bytes()), "{k}");
            assert_eq!(enc(k, None, Mods::NONE, app), Some(format!("\x1bO{f}").into_bytes()), "{k} app");
            // Modified cursor keys are CSI 1 ; m X in either mode.
            assert_eq!(enc(k, None, CTRL, app), Some(format!("\x1b[1;5{f}").into_bytes()), "{k} ctrl app");
        }
    }

    #[test]
    fn xterm_modifier_parameters() {
        assert_eq!(key("up", SHIFT), s(b"\x1b[1;2A"));
        assert_eq!(key("home", ALT), s(b"\x1b[1;3H"));
        assert_eq!(key("up", Mods { shift: true, alt: true, ctrl: false }), s(b"\x1b[1;4A"));
        assert_eq!(key("up", CTRL), s(b"\x1b[1;5A"));
        assert_eq!(key("up", CTRL_SHIFT), s(b"\x1b[1;6A"));
        assert_eq!(key("up", CTRL_ALT), s(b"\x1b[1;7A"));
        assert_eq!(key("up", ALL), s(b"\x1b[1;8A"));
        assert_eq!(key("delete", Mods::NONE), s(b"\x1b[3~"));
        assert_eq!(key("delete", CTRL), s(b"\x1b[3;5~"));
        assert_eq!(key("insert", Mods::NONE), s(b"\x1b[2~"));
        assert_eq!(key("pageup", Mods::NONE), s(b"\x1b[5~"));
        assert_eq!(key("pagedown", ALT), s(b"\x1b[6;3~"));
    }

    #[test]
    fn function_keys() {
        assert_eq!(key("f1", Mods::NONE), s(b"\x1bOP"));
        assert_eq!(key("f4", Mods::NONE), s(b"\x1bOS"));
        assert_eq!(key("f1", SHIFT), s(b"\x1b[1;2P"));
        assert_eq!(key("f4", CTRL_SHIFT), s(b"\x1b[1;6S"));
        let tildes = [("f5", 15), ("f6", 17), ("f7", 18), ("f8", 19), ("f9", 20), ("f10", 21), ("f11", 23), ("f12", 24)];
        for (k, n) in tildes {
            assert_eq!(key(k, Mods::NONE), Some(format!("\x1b[{n}~").into_bytes()), "{k}");
            assert_eq!(key(k, CTRL), Some(format!("\x1b[{n};5~").into_bytes()), "{k} ctrl");
        }
    }

    #[test]
    fn ctrl_and_alt_characters() {
        assert_eq!(key("a", CTRL), s(b"\x01"));
        assert_eq!(key("c", CTRL), s(b"\x03"));
        assert_eq!(key("z", CTRL), s(b"\x1a"));
        assert_eq!(key("z", CTRL_SHIFT), s(b"\x1a"));
        assert_eq!(key("[", CTRL), s(b"\x1b"));
        assert_eq!(key("\\", CTRL), s(b"\x1c"));
        assert_eq!(key("]", CTRL), s(b"\x1d"));
        assert_eq!(key("6", CTRL_SHIFT), s(b"\x1e"));
        assert_eq!(key("-", CTRL_SHIFT), s(b"\x1f"));
        assert_eq!(key("/", CTRL), s(b"\x1f"));
        assert_eq!(key("2", CTRL), s(b"\x00"));
        assert_eq!(key("8", CTRL), s(b"\x7f"));
        assert_eq!(key("a", CTRL_ALT), s(b"\x1b\x01"));
        assert_eq!(enc("b", Some("b"), ALT, Modes::default()), s(b"\x1bb"));
        assert_eq!(enc("b", Some("B"), Mods { shift: true, alt: true, ctrl: false }, Modes::default()), s(b"\x1bB"));
        assert_eq!(enc("b", None, Mods { shift: true, alt: true, ctrl: false }, Modes::default()), s(b"\x1bB"));
        assert_eq!(key(",", CTRL), None);
    }

    #[test]
    fn keypad_follows_deckpam() {
        let app = Modes { app_keypad: true, ..Default::default() };
        assert_eq!(key("numpad5", Mods::NONE), s(b"5"));
        assert_eq!(enc("numpad5", None, Mods::NONE, app), s(b"\x1bOu"));
        assert_eq!(enc("numpadenter", None, Mods::NONE, app), s(b"\x1bOM"));
        assert_eq!(key("numpadenter", Mods::NONE), s(b"\r"));
        assert_eq!(enc("numpadadd", None, SHIFT, app), s(b"\x1bO2k"));
    }

    #[test]
    fn paste_framing() {
        assert_eq!(paste("a\nb", false), "a\rb");
        assert_eq!(paste("a\r\nb\x1b[201~x", true), "\x1b[200~a\rb[201~x\x1b[201~");
        assert_eq!(paste("a\x03b", true), "\x1b[200~ab\x1b[201~");
        // Unframed too: no ESC to type keys, no ^C or ^D to act as them.
        assert_eq!(paste("a\x03b\x1b[2J\x04", false), "ab[2J");
        // Tab and the line ends stay; the rest of C0 and all of C1 go.
        assert_eq!(paste("a\tb\x00\x07\x08\x7f\u{85}\u{9b}31m\u{a0}c", false), "a\tb\x7f31m\u{a0}c");
        assert_eq!(paste("\u{9b}201~x", true), "\x1b[200~201~x\x1b[201~");
    }

    #[test]
    fn alt_arrows_are_the_pages() {
        let mac = cfg!(target_os = "macos");
        assert_eq!(key("left", ALT), s(if mac { b"\x1bb" } else { b"\x1b[1;5D" }));
        assert_eq!(key("right", ALT), s(if mac { b"\x1bf" } else { b"\x1b[1;5C" }));
        assert_eq!(key("up", ALT), s(if mac { b"\x1b[1;3A" } else { b"\x1b[1;5A" }));
        // With another modifier, xterm's own.
        assert_eq!(key("left", Mods { shift: true, alt: true, ctrl: false }), s(b"\x1b[1;4D"));
    }
}
