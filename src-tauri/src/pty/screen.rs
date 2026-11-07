//! What a session's output shows: the screen replayed through a terminal
//! emulator, the composer on it and a person's draft there, the terminal
//! modes and window title the CLI set, and the key events Codex on Windows
//! takes typing as. With what a chat is described by (`SessionMeta`).

use super::start::without_note;
use super::{manager, now_ms, PtyManager};
use crate::provider::Provider;
use serde::Serialize;
use std::sync::atomic::Ordering;
use std::sync::Arc;

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SessionMeta {
    pub name: Option<String>,
    /// Claude Code's own vocabulary as the registry has it, or for Codex
    /// `working` | `waiting` | `resting`.
    pub status: Option<String>,
    pub cwd: Option<String>,
    pub session_id: Option<String>,
    /// "auto" (AI-titled), "user" (renamed by hand) or "derived" (just the cwd
    /// folder name, which for a worktree session is meaningless noise).
    pub name_source: Option<String>,
    /// The model the session is actually running, where the CLI records it
    /// (Codex's `turn_context`); None where the chat setting is the truth.
    pub model: Option<String>,
    pub context: Option<f64>,
    pub context_tokens: Option<f64>,
    /// Window the percentage was computed against, so the UI can show what it
    /// assumed instead of silently pinning a longer session at 100%.
    pub context_window: Option<f64>,
    /// The CLI's own title for the session: a rename, else the one it
    /// generated (Claude Code's `ai-title`, the same it puts on the terminal
    /// tab), else None.
    pub title: Option<String>,
    /// The title is a rename the user gave it in the CLI, not one the CLI
    /// came up with.
    pub title_renamed: bool,
    /// First real prompt of the session — the chat's title until the CLI
    /// comes up with one.
    pub first_prompt: Option<String>,
    /// A person left something typed and not sent in its composer.
    pub draft: bool,
    /// Text Luna holds for it until that draft is sent or cleared: sends
    /// from other sessions, notices to it (agents.rs `held_count`).
    pub held: usize,
    /// `waiting` comes from the marker its last message ends on (asking.rs),
    /// not from a prompt of its CLI's: the user may clear it by hand, by this
    /// key, which names the ending it was shown on.
    pub marked: Option<String>,
    /// The model's last message of a turn that is over, for the waiting
    /// marker; not sent to the window.
    #[serde(skip)]
    pub last_said: Option<String>,
    /// When it was said, as the same read of the transcript has it: the same
    /// words a turn later are another ending.
    #[serde(skip)]
    pub last_said_at: Option<String>,
}

/// A session's title as its CLI has it, and whether the user typed it.
#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CliTitle {
    pub title: String,
    pub renamed: bool,
}

/// A title has to be something a person can read. Both sources have handed us
/// strings that were not: a registry name made of the terminal's own cursor
/// and mouse reports, which is what a chat row read as for its first minutes.
/// Rejecting them here keeps that out of every place a title is shown.
pub fn sane_title(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty() && !t.chars().any(char::is_control)).then(|| t.to_string())
}

/// A prompt trimmed to a title: whitespace flattened, 48 characters. Slash
/// commands, replayed tool output and system reminders are not titles.
pub fn title_from_prompt(text: &str) -> Option<String> {
    let text = without_note(text).trim();
    if text.is_empty() || text.starts_with('<') || text.starts_with('/') {
        return None;
    }
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut title: String = flat.chars().take(48).collect();
    if flat.chars().count() > 48 {
        title.push('…');
    }
    sane_title(&title)
}

/// Replays raw pty output through a terminal emulator and returns the visible
/// screen as text, blank trailing lines dropped. The scrollback is capped
/// (SCROLLBACK_CAP), so a very long session starts mid-stream; a fullscreen
/// TUI repaints everything on each frame, so that costs nothing visible.
pub fn render_screen(bytes: &[u8], rows: u16, cols: u16) -> String {
    let mut parser = vt100::Parser::new(rows.max(1), cols.max(1), 0);
    parser.process(bytes);
    let contents = parser.screen().contents();
    let mut lines: Vec<&str> = contents.lines().map(str::trim_end).collect();
    while lines.last().map(|l| l.is_empty()).unwrap_or(false) {
        lines.pop();
    }
    lines.join("\n")
}

/// Codex reads key presses, not text. ConPTY makes a key press out of each
/// character it is given, on the console's keyboard layout, and a character
/// that layout has no key for reached Codex as nothing: on a Russian layout
/// `[ ] { } < > ' ` ~ @ # $ ^ & |` and typographic quotes and dashes all
/// vanished, from a person's typing and from `luna_send` alike. Given as
/// win32-input-mode key events (ConPTY is opened with WIN32_INPUT_MODE), every
/// character arrives as itself whatever the layout. Ctrl+<letter> is the same
/// story: ConPTY hands Codex the control byte and Codex asks the layout which
// lang-check: allow
/// letter that key is, so on a Russian one Ctrl+C came in as Ctrl+С — no
/// shortcut of its: a selection was dropped instead of copied. Sent as the
/// Latin letter with Ctrl held, it is Ctrl+C on any layout. The control bytes
/// of keys of their own (Backspace, Tab, Enter) and escape sequences — arrows,
/// focus and mouse reports, a cursor report — go through as they are.
pub(super) fn as_key_events(bytes: &[u8]) -> Vec<u8> {
    use std::fmt::Write as _;
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(bytes.len() * 24);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            out.push(c);
            match chars.next() {
                // CSI: parameters and intermediates up to the final byte.
                Some('[') => {
                    out.push('[');
                    for n in chars.by_ref() {
                        out.push(n);
                        if ('\x40'..='\x7e').contains(&n) {
                            break;
                        }
                    }
                }
                // SS3 (application cursor keys, F1-F4): one more character.
                Some('O') => {
                    out.push('O');
                    if let Some(n) = chars.next() {
                        out.push(n);
                    }
                }
                // Alt with a key.
                Some(n) => out.push(n),
                None => {}
            }
        } else if let Some(letter) = ctrl_letter(c) {
            // Vk;Sc;Uc;Kd;Cs;Rc with LEFT_CTRL_PRESSED. The real virtual key,
            // so a console still in processed input mode takes Ctrl+C as one.
            let vk = letter.to_ascii_uppercase() as u32;
            let uc = letter as u32;
            let _ = write!(out, "\x1b[{vk};0;{uc};1;8;1_\x1b[{vk};0;{uc};0;8;1_");
        } else if c < ' ' || c == '\x7f' {
            out.push(c);
        } else {
            let mut units = [0u16; 2];
            let units = c.encode_utf16(&mut units);
            // Vk;Sc;Uc;Kd;Cs;Rc — down, then up. The halves of a surrogate
            // pair go down only: Codex took the release of the first half for
            // the end of the character and dropped it.
            let pair = units.len() == 2;
            for u in units.iter() {
                let _ = write!(out, "\x1b[0;0;{u};1;0;1_");
                if !pair {
                    let _ = write!(out, "\x1b[0;0;{u};0;0;1_");
                }
            }
        }
    }
    out.into_bytes()
}

/// The letter a terminal's Ctrl+<letter> byte stands for, except the bytes that
/// are keys of their own: Backspace, Tab, Ctrl+J and Enter.
fn ctrl_letter(c: char) -> Option<char> {
    match c {
        '\x08' | '\t' | '\n' | '\r' => None,
        '\x01'..='\x1a' => Some((b'a' + c as u8 - 1) as char),
        _ => None,
    }
}

/// Set in the draft state when the last character typed was a backslash.
const DRAFT_BACKSLASH: u64 = 1 << 63;
/// How long after a key the screen may still not show it: until then the
/// keys' word on a draft stands (see `has_draft`).
const DRAFT_SETTLE_MS: u64 = 1_500;

/// What a CLI's composer holds, read off its screen.
#[derive(Debug, PartialEq)]
pub struct Composer {
    /// What is typed in it, blank lines and the edges trimmed; empty when
    /// nothing is. A placeholder, drawn dim, is not text.
    pub text: String,
    /// Claude Code holds a draft put aside with Ctrl+S, and says so above
    /// the composer until it is back: another Ctrl+S on text would replace
    /// it.
    pub stashed: bool,
}

/// The composer on a session's screen, read where the cursor is: both CLIs
/// put the terminal's cursor at the caret. Its first row starts with the
/// prompt mark — Claude Code's `❯` under a rule, Codex's `›`, which it draws
/// dim on the prompts in the conversation and not on its composer — and it
/// runs down to Claude Code's rule under it or to Codex's first blank row.
/// None with the cursor hidden or anywhere else: a dialog, a picker, a mode
/// with a mark of its own (`!`), a screen not drawn yet.
pub fn read_composer(bytes: &[u8], rows: u16, cols: u16, provider: Provider) -> Option<Composer> {
    let mut parser = vt100::Parser::new(rows.max(1), cols.max(1), 0);
    parser.process(bytes);
    let screen = parser.screen();
    if screen.hide_cursor() {
        return None;
    }
    let (cursor, _) = screen.cursor_position();
    let first = |r: u16| screen.cell(r, 0).map_or("", |c| c.contents());
    let rule = |r: u16| first(r) == "─";
    let blank = |r: u16| (0..cols).all(|c| screen.cell(r, c).is_none_or(|x| x.contents().trim().is_empty()));
    let mark = match provider {
        Provider::Claude => "❯",
        Provider::Codex => "›",
    };
    let top = (0..=cursor).rev().find(|&r| first(r) == mark)?;
    let end = match provider {
        Provider::Claude => {
            if top == 0 || !rule(top - 1) {
                return None;
            }
            // A pane too short for the rule under it ends with the composer.
            (top + 1..rows).find(|&r| rule(r)).unwrap_or(rows)
        }
        Provider::Codex => {
            if screen.cell(top, 0).is_some_and(|c| c.dim()) {
                return None;
            }
            (top + 1..rows).find(|&r| blank(r)).unwrap_or(rows)
        }
    };
    if cursor >= end {
        return None;
    }
    let lines: Vec<String> = (top..end)
        .map(|r| {
            let row: String = (2..cols)
                .filter_map(|c| screen.cell(r, c))
                .filter(|c| !c.dim() && !c.is_wide_continuation())
                .map(|c| if c.has_contents() { c.contents() } else { " " })
                .collect();
            row.trim_end().to_string()
        })
        .collect();
    let stashed = provider == Provider::Claude
        && (top.saturating_sub(3)..top.saturating_sub(1)).any(|r| {
            let row: String =
                (0..cols).filter_map(|c| screen.cell(r, c)).map(|c| if c.has_contents() { c.contents() } else { " " }).collect();
            // The mark that stays, or the word the stash first flashes.
            row.trim_end().ends_with("› stashed") || row.contains("stashed · ")
        });
    Some(Composer { text: lines.join("\n").trim().to_string(), stashed })
}

/// Whether the screen shows the CLI at its prompt with nothing in it, for a chat
/// nobody may be looking at. `read_composer` goes by the cursor, and that says
/// nothing here: Claude Code asks to be told of the focus (`?1004`) and until it
/// has it keeps the terminal's cursor hidden, drawing a caret of its own, as
/// in a helper no pane has ever shown. So its composer is also the last `❯` on
/// screen, under a rule and with a rule or the screen's end below it, nothing but
/// a dim placeholder after it. A dialog's `❯` picks a line with words on it.
pub fn at_empty_prompt(bytes: &[u8], rows: u16, cols: u16, provider: Provider) -> bool {
    if let Some(c) = read_composer(bytes, rows, cols, provider) {
        return c.text.is_empty();
    }
    if provider != Provider::Claude {
        return false;
    }
    let mut parser = vt100::Parser::new(rows.max(1), cols.max(1), 0);
    parser.process(bytes);
    let screen = parser.screen();
    let first = |r: u16| screen.cell(r, 0).map_or("", |c| c.contents());
    let rule = |r: u16| first(r) == "─";
    let Some(top) = (0..rows).rev().find(|&r| first(r) == "❯") else { return false };
    let empty = (1..cols).filter_map(|c| screen.cell(top, c)).all(|c| c.dim() || c.contents().trim().is_empty());
    empty && top > 0 && rule(top - 1) && (top + 1 == rows || rule(top + 1))
}

/// The composer's length after a person's keys, roughly: characters add,
/// Backspace takes one off, Enter, Ctrl+C and Ctrl+U empty it — but not an
/// Enter right after a backslash, which is how a Claude Code prompt gets its
/// next line — and the sequences a terminal sends for other keys (arrows,
/// focus, mouse) do nothing, except a bracketed paste, whose text counts.
/// Edits it cannot follow (a word deleted with Ctrl+W, a selection cut) leave
/// it high, which only holds a notice until the next Enter. The top bit
/// carries the backslash across writes, one key arriving per write.
pub(super) fn draft_after(state: u64, data: &str) -> u64 {
    let mut n = state & !DRAFT_BACKSLASH;
    let mut backslash = state & DRAFT_BACKSLASH != 0;
    let mut chars = data.chars().peekable();
    while let Some(c) = chars.next() {
        let was_backslash = std::mem::replace(&mut backslash, c == '\\');
        match c {
            '\r' if was_backslash => n += 1,
            '\r' | '\x03' | '\x15' => n = 0,
            '\x7f' | '\x08' => n = n.saturating_sub(1),
            // A sequence types nothing — the paste markers included; the
            // text between them is counted as it comes.
            '\x1b' => {
                if chars.peek() == Some(&'[') {
                    chars.next();
                    for p in chars.by_ref() {
                        if ('\x40'..='\x7e').contains(&p) {
                            break;
                        }
                    }
                } else if chars.next() == Some('O') {
                    // SS3: arrows in application mode, F1-F4.
                    chars.next();
                }
            }
            c if c < ' ' => {}
            _ => n += 1,
        }
    }
    n | if backslash { DRAFT_BACKSLASH } else { 0 }
}

/// Where the cursor stands once `bytes` have been drawn at this size, zero-based
/// (row, col).
pub(super) fn cursor_at(bytes: &[u8], rows: u16, cols: u16) -> (u16, u16) {
    let mut parser = vt100::Parser::new(rows.max(1), cols.max(1), 0);
    parser.process(bytes);
    parser.screen().cursor_position()
}

/// The DEC private modes worth carrying over to a pane that attaches late:
/// cursor visibility, cursor-key mode, mouse reporting, focus reporting and
/// bracketed paste. A CLI switches these once, at start, and never again —
/// so they are long gone from the capped scrollback, and a reattaching pane
/// resets its terminal anyway (see terminals.ts). Without them the pane drew
/// its own cursor wherever the CLI last wrote, on top of the one the CLI
/// draws in its input box, and the stray one wandered the screen with every
/// repaint. The alternate screen (1049) is left out on purpose: the pane
/// repaints onto the normal screen after the reset. So is synchronized output
/// (2026), which would freeze the pane if it were replayed mid-frame.
const KEPT_MODES: &[u16] = &[1, 25, 1000, 1002, 1003, 1004, 1006, 2004];

/// The longest window title kept (`Modes::title`): a thread id is 36.
const TITLE_MAX: usize = 256;

/// The thread id a Codex title holds (`CODEX_TITLE`), or as much of it as the
/// title gives: Codex 0.166 cuts each item to 29 characters and an ellipsis,
/// whatever the width, which leaves 25 of its 32 hex digits — no two threads
/// share them. The first run shaped like one, hex in groups of 8-4-4-4-12, at
/// least into the fifth group; the shortest is told apart where it is used.
fn thread_in_title(title: &str) -> Option<&str> {
    const SHORTEST: usize = 25;
    let b = title.as_bytes();
    let fits = |j: usize, c: u8| if matches!(j, 8 | 13 | 18 | 23) { c == b'-' } else { c.is_ascii_hexdigit() };
    (0..b.len()).find_map(|i| {
        if i > 0 && (b[i - 1].is_ascii_hexdigit() || b[i - 1] == b'-') {
            return None;
        }
        let len = b[i..].iter().take(36).enumerate().take_while(|(j, c)| fits(*j, **c)).count();
        let ends = b.get(i + len).is_none_or(|c| !c.is_ascii_hexdigit() && *c != b'-');
        (len >= SHORTEST && ends).then(|| &title[i..i + len])
    })
}

/// The last state of each of `KEPT_MODES`, read off the output as it streams,
/// so the scrollback cap cannot take it away, and the window title the CLI last
/// set (OSC 0 or 2), which no pane shows. The scanner keeps its place between
/// reads, so a sequence split across two of them still counts.
#[derive(Default)]
pub(super) struct Modes {
    pub(super) on: std::collections::BTreeMap<u16, bool>,
    /// 0 ground, 1 after ESC, 2 after ESC [, 3 in the parameters of ESC [ ?,
    /// 4 in the number of ESC ], 5 in the text of a title.
    state: u8,
    params: Vec<u8>,
    title: Option<String>,
}

impl Modes {
    pub(super) fn feed(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.state = match (self.state, b) {
                // BEL ends a title, as does ST (ESC \), whose ESC ends it here.
                (5, 0x07 | 0x1b) => {
                    self.title = Some(String::from_utf8_lossy(&self.params).into_owned());
                    if b == 0x1b {
                        1
                    } else {
                        0
                    }
                }
                (5, _) => {
                    if self.params.len() < TITLE_MAX {
                        self.params.push(b);
                    }
                    5
                }
                (_, 0x1b) => 1,
                (1, b'[') => 2,
                (1, b']') => {
                    self.params.clear();
                    4
                }
                (4, b'0'..=b'9') if self.params.len() < 4 => {
                    self.params.push(b);
                    4
                }
                (4, b';') if matches!(self.params.as_slice(), b"0" | b"2") => {
                    self.params.clear();
                    5
                }
                (2, b'?') => {
                    self.params.clear();
                    3
                }
                (3, b'0'..=b'9' | b';') if self.params.len() < 64 => {
                    self.params.push(b);
                    3
                }
                (3, b'h' | b'l') => {
                    self.apply(b == b'h');
                    0
                }
                _ => 0,
            };
        }
    }

    fn apply(&mut self, set: bool) {
        for p in self.params.split(|&c| c == b';') {
            let n = std::str::from_utf8(p).ok().and_then(|p| p.parse::<u16>().ok());
            if let Some(n) = n.filter(|n| KEPT_MODES.contains(n)) {
                self.on.insert(n, set);
            }
        }
    }

    fn replay(&self) -> String {
        self.on.iter().map(|(n, set)| format!("\x1b[?{n}{}", if *set { 'h' } else { 'l' })).collect()
    }
}

/// The escapes that put a pane's terminal back in the modes the session's CLI
/// switched — written after a reattaching pane resets (see `Modes`). Empty for
/// a session that is not there.
#[tauri::command]
pub fn session_modes(id: String) -> String {
    let state = manager();
    let modes = {
        let sessions = state.sessions.lock().unwrap();
        match sessions.get(&id) {
            Some(s) => Arc::clone(&s.modes),
            None => return String::new(),
        }
    };
    let replay = modes.lock().unwrap_or_else(|e| e.into_inner()).replay();
    replay
}

impl PtyManager {
    /// Whether a person has left something typed and not sent in the
    /// session's composer. The keys say whether there may be (`draft_after`);
    /// the screen, where the composer can be read, says whether there is: a
    /// draft cleared in a way the keys do not tell (Esc Esc, a word deleted)
    /// or one of nothing but spaces is none, and the count starts over. Keys
    /// just typed may not be drawn yet, so the screen is asked only once
    /// they have settled. A person who has stashed a draft in Claude Code
    /// (Ctrl+S) gets it back from the CLI with the next prompt, theirs or
    /// Luna's, with no key that says so: while one may be stashed, an empty
    /// count is asked about too.
    pub fn has_draft(&self, id: &str) -> bool {
        let (draft, seen, stashing, n, key) = {
            let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
            let Some(s) = sessions.get(id) else { return false };
            let n = s.draft.load(Ordering::Relaxed);
            let stashing = s.stashing.load(Ordering::Relaxed);
            let typed = n & !DRAFT_BACKSLASH > 0;
            if !typed && !stashing {
                return false;
            }
            if typed && now_ms().saturating_sub(s.typed_ms.load(Ordering::Relaxed)) < DRAFT_SETTLE_MS {
                return true;
            }
            let key = n.rotate_left(32) ^ s.last_output_ms.load(Ordering::Relaxed) ^ stashing as u64;
            (Arc::clone(&s.draft), Arc::clone(&s.draft_seen), Arc::clone(&s.stashing), n, key)
        };
        let cached = *seen.lock().unwrap_or_else(|e| e.into_inner());
        if cached.0 == key {
            return cached.1;
        }
        let there = match self.composer(id) {
            Some(c) if c.text.is_empty() => {
                // Unless a key came in since.
                let _ = draft.compare_exchange(n, 0, Ordering::Relaxed, Ordering::Relaxed);
                if !c.stashed {
                    stashing.store(false, Ordering::Relaxed);
                }
                false
            }
            Some(c) => {
                // A stashed draft back in place counts as typed.
                let _ = draft.compare_exchange(n, n.max(c.text.chars().count() as u64), Ordering::Relaxed, Ordering::Relaxed);
                true
            }
            None => n & !DRAFT_BACKSLASH > 0,
        };
        *seen.lock().unwrap_or_else(|e| e.into_inner()) = (key, there);
        there
    }

    /// The draft count as the keys have it (see `draft_after`), to be put
    /// back with `set_draft` once a draft Luna stashed is back in place.
    pub fn draft_count(&self, id: &str) -> u64 {
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions.get(id).map_or(0, |s| s.draft.load(Ordering::Relaxed))
    }

    pub fn set_draft(&self, id: &str, n: u64) {
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = sessions.get(id) {
            s.draft.store(n, Ordering::Relaxed);
        }
    }

    /// The session's composer as its screen shows it, None where the screen
    /// does not show one (see `read_composer`).
    pub fn composer(&self, id: &str) -> Option<Composer> {
        let (provider, scrollback, size) = {
            let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
            let s = sessions.get(id)?;
            (s.provider, Arc::clone(&s.scrollback), Arc::clone(&s.size))
        };
        let bytes: Vec<u8> = scrollback.lock().unwrap_or_else(|e| e.into_inner()).make_contiguous().to_vec();
        let (rows, cols) = *size.lock().unwrap_or_else(|e| e.into_inner());
        read_composer(&bytes, rows, cols, provider)
    }

    /// The thread a Codex session's TUI says it is on now (`CODEX_TITLE`), by
    /// its id or the start of it (`thread_in_title`); None before it says, for
    /// a session gone, and for Claude Code.
    pub fn codex_thread(&self, id: &str) -> Option<String> {
        let modes = {
            let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
            let s = sessions.get(id).filter(|s| s.provider == Provider::Codex)?;
            Arc::clone(&s.modes)
        };
        let m = modes.lock().unwrap_or_else(|e| e.into_inner());
        thread_in_title(m.title.as_deref()?).map(str::to_owned)
    }

    /// Whether its CLI stands at an empty prompt (see `at_empty_prompt`).
    pub fn at_prompt(&self, id: &str) -> bool {
        let found = {
            let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
            sessions.get(id).map(|s| (s.provider, Arc::clone(&s.scrollback), Arc::clone(&s.size)))
        };
        let Some((provider, scrollback, size)) = found else { return false };
        let bytes: Vec<u8> = scrollback.lock().unwrap_or_else(|e| e.into_inner()).make_contiguous().to_vec();
        let (rows, cols) = *size.lock().unwrap_or_else(|e| e.into_inner());
        at_empty_prompt(&bytes, rows, cols, provider)
    }

    /// What the session's terminal shows right now: the scrollback replayed
    /// through a terminal emulator at the pty's size, as plain text, rows
    /// joined by newlines. This is how an agent sees what its helper is
    /// stuck on — a first-run notice, a picker, an error the transcript will
    /// never carry — and what a person would see in the pane.
    pub fn screen(&self, id: &str) -> Option<String> {
        let (scrollback, size) = {
            let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
            let s = sessions.get(id)?;
            (Arc::clone(&s.scrollback), Arc::clone(&s.size))
        };
        let bytes: Vec<u8> = {
            let mut sb = scrollback.lock().unwrap_or_else(|e| e.into_inner());
            sb.make_contiguous().to_vec()
        };
        let (rows, cols) = *size.lock().unwrap_or_else(|e| e.into_inner());
        Some(render_screen(&bytes, rows, cols))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_survive_split_reads_and_keep_only_the_last_word() {
        let mut m = Modes::default();
        m.feed(b"\x1b[?25l\x1b[?20");
        m.feed(b"04h\x1b[?1049h\x1b[?2026h\x1b[?1000;1006h");
        m.feed(b"text\x1b[?1000l\x1b[31m\x1b[?");
        m.feed(b"1004h");
        assert_eq!(m.replay(), "\x1b[?25l\x1b[?1000l\x1b[?1004h\x1b[?1006h\x1b[?2004h");
    }

    #[test]
    fn the_title_is_read_across_reads_whichever_way_it_ends() {
        let mut m = Modes::default();
        assert_eq!(m.title, None);
        m.feed(b"\x1b[?25l\x1b]0;01a1a632-e9d4-79");
        assert_eq!(m.title, None, "not over yet");
        m.feed(b"13-af6d-a936925dd16c\x07text");
        assert_eq!(m.title.as_deref(), Some("01a1a632-e9d4-7913-af6d-a936925dd16c"));
        assert_eq!(thread_in_title(m.title.as_deref().unwrap()), Some("01a1a632-e9d4-7913-af6d-a936925dd16c"));
        // ST ends one too, and what follows it is read as ever.
        m.feed(b"\x1b]2;work \xe2\x80\x94 01a1a631-dc9f-7170-a47e-388412a20b13\x1b\\\x1b[?2004h");
        assert_eq!(thread_in_title(m.title.as_deref().unwrap()), Some("01a1a631-dc9f-7170-a47e-388412a20b13"));
        assert!(m.replay().contains("\x1b[?2004h"));
        // Another OSC (a hyperlink, Codex's run state) is no title.
        m.feed(b"\x1b]8;;https://x\x07link\x1b]8;;\x07\x1b]7501;state=idle:app=codex\x07");
        assert_eq!(thread_in_title(m.title.as_deref().unwrap()), Some("01a1a631-dc9f-7170-a47e-388412a20b13"));
        // As Codex 0.166 draws it under ConPTY: cut to 29 and an ellipsis, alone
        // or beside another item.
        assert_eq!(thread_in_title("01a1a697-6e85-7be0-b03c-583f7..."), Some("01a1a697-6e85-7be0-b03c-583f7"));
        assert_eq!(thread_in_title("01a1a698-30e8-7c22-86a3-0d5ef... | Ready"), Some("01a1a698-30e8-7c22-86a3-0d5ef"));
        assert_eq!(thread_in_title("codex"), None);
        assert_eq!(thread_in_title("E:\\Programs\\Luna\\codex-cli\\versions\\0.166.0\\bin\\codex.exe"), None);
        assert_eq!(thread_in_title("01a1a631-dc9f-7170-a47e-"), None, "too little of one to tell");
        assert_eq!(thread_in_title("01a1a631-dc9f-7170-a47e-3..."), Some("01a1a631-dc9f-7170-a47e-3"), "enough");
        assert_eq!(
            thread_in_title("x01a1a631-dc9f-7170-a47e-388412a20b13"),
            Some("01a1a631-dc9f-7170-a47e-388412a20b13"),
            "a letter not hex before it"
        );
        assert_eq!(thread_in_title("f01a1a631-dc9f-7170-a47e-388412a20b13"), None, "a run that is not shaped like one");
    }

    #[test]
    fn codex_gets_characters_as_key_events_and_sequences_as_they_are() {
        let k = |u: u32| format!("\x1b[0;0;{u};1;0;1_\x1b[0;0;{u};0;0;1_");
        assert_eq!(String::from_utf8(as_key_events(b"[")).unwrap(), k(91));
        // Enter, an arrow, a focus report, a cursor report, Alt+x: untouched.
        let raw = "\r\x1b[A\x1b[I\x1b[12;3R\x1bx\x1bOP\x7f";
        assert_eq!(String::from_utf8(as_key_events(raw.as_bytes())).unwrap(), raw);
        // Outside the basic plane: both halves of the surrogate pair, pressed.
        let down = |u: u32| format!("\x1b[0;0;{u};1;0;1_");
        assert_eq!(String::from_utf8(as_key_events("😀".as_bytes())).unwrap(), format!("{}{}", down(0xD83D), down(0xDE00)));
        // lang-check: allow
        assert_eq!(String::from_utf8(as_key_events("a\rя".as_bytes())).unwrap(), format!("{}\r{}", k(97), k(0x44F)));
        // Ctrl+C and Ctrl+T by their Latin letters, whatever the layout.
        let ctrl = |vk: u32, u: u32| format!("\x1b[{vk};0;{u};1;8;1_\x1b[{vk};0;{u};0;8;1_");
        assert_eq!(String::from_utf8(as_key_events(b"\x03\x14")).unwrap(), format!("{}{}", ctrl(67, 99), ctrl(84, 116)));
        // Backspace, Tab, Ctrl+J and Enter stay keys of their own.
        assert_eq!(String::from_utf8(as_key_events(b"\x08\t\n\r")).unwrap(), "\x08\t\n\r");
    }

    /// A screen drawn the way the CLIs draw theirs: text put at rows and
    /// columns (1-based, as the escapes count), the cursor left at the end.
    fn drawn(parts: &[(u16, u16, &str)], cursor: (u16, u16)) -> Vec<u8> {
        let mut out = String::from("\x1b[2J");
        for (r, c, text) in parts {
            out.push_str(&format!("\x1b[{r};{c}H{text}"));
        }
        out.push_str(&format!("\x1b[{};{}H", cursor.0, cursor.1));
        out.into_bytes()
    }

    #[test]
    fn claude_codes_composer_is_read_between_its_rules() {
        let rule = "─".repeat(40);
        let read = |parts: &[(u16, u16, &str)], cursor| read_composer(&drawn(parts, cursor), 10, 40, Provider::Claude);
        let empty = read(&[(1, 1, "❯ Reply with just OK."), (6, 1, &rule), (7, 1, "❯ "), (8, 1, &rule)], (7, 3));
        assert_eq!(empty, Some(Composer { text: String::new(), stashed: false }));
        // A lone space is no draft either.
        let space = read(&[(6, 1, &rule), (7, 1, "❯  "), (8, 1, &rule)], (7, 4));
        assert_eq!(space.map(|c| c.text), Some(String::new()));
        let typed = read(&[(5, 1, &rule), (6, 1, "❯ line2"), (7, 1, "  third[Pasted text #1 +29 lines]"), (8, 1, &rule)], (7, 34));
        assert_eq!(typed.map(|c| c.text).as_deref(), Some("line2\nthird[Pasted text #1 +29 lines]"));
        let short = read(&[(9, 1, &rule), (10, 1, "❯ my draft")], (10, 11));
        assert_eq!(short.map(|c| c.text).as_deref(), Some("my draft"), "no room left for the rule under it");
        let stashed = read(&[(5, 30, "› stashed"), (6, 1, &rule), (7, 1, "❯ "), (8, 1, &rule)], (7, 3));
        assert_eq!(stashed, Some(Composer { text: String::new(), stashed: true }));
        // A prompt in the conversation, with no rule over it: a dialog has
        // the cursor, not the composer.
        assert_eq!(read(&[(2, 1, "❯ Reply with just OK."), (4, 1, "Do you trust this folder?")], (5, 1)), None);
        let hidden = [drawn(&[(6, 1, &rule), (7, 1, "❯ hi"), (8, 1, &rule)], (7, 5)), b"\x1b[?25l".to_vec()].concat();
        assert_eq!(read_composer(&hidden, 10, 40, Provider::Claude), None);
    }

    #[test]
    fn an_empty_prompt_is_told_with_the_cursor_hidden_as_in_a_helper_with_no_pane() {
        let rule = "─".repeat(40);
        let hidden = |parts: &[(u16, u16, &str)]| [b"\x1b[?25l".to_vec(), drawn(parts, (1, 1))].concat();
        let at = |bytes: &[u8]| at_empty_prompt(bytes, 10, 40, Provider::Claude);
        // What Claude Code 2.1.316 draws: a no-break space, its own caret in reverse.
        let caret = "❯\u{a0}\x1b[7m \x1b[27m\x1b[K";
        assert!(at(&hidden(&[(1, 1, "❯ say hello"), (6, 1, &rule), (7, 1, caret), (8, 1, &rule)])));
        assert!(
            at(&hidden(&[(6, 1, &rule), (7, 1, "❯ \x1b[2mTry something\x1b[22m"), (8, 1, &rule)])),
            "a dim placeholder is nothing typed"
        );
        assert!(at(&hidden(&[(9, 1, &rule), (10, 1, caret)])), "no room left for the rule under it");
        assert!(!at(&hidden(&[(6, 1, &rule), (7, 1, "❯ a draft"), (8, 1, &rule)])), "text in it");
        assert!(!at(&hidden(&[(3, 1, "Use this API key?"), (5, 1, "❯ No (recommended)"), (6, 1, "  Yes")])), "a dialog's choice");
        assert!(!at(&hidden(&[(1, 1, "❯ say hello"), (3, 1, "working")])), "a prompt in the conversation, with no composer under it");
        assert!(!at(b""), "not drawn yet");
        // The cursor where a pane has it: the composer as `read_composer` has it.
        assert!(at(&drawn(&[(6, 1, &rule), (7, 1, "❯ "), (8, 1, &rule)], (7, 3))));
        // Codex has one way of showing it, the cursor at its caret.
        assert!(at_empty_prompt(&drawn(&[(6, 1, "› "), (8, 1, "? for shortcuts")], (6, 3)), 10, 40, Provider::Codex));
        assert!(!at_empty_prompt(&drawn(&[(6, 1, "› a draft"), (8, 1, "? for shortcuts")], (6, 10)), 10, 40, Provider::Codex));
        assert!(
            !at_empty_prompt(&hidden(&[(6, 1, "› "), (8, 1, "? for shortcuts")]), 10, 40, Provider::Codex),
            "not by the cursor, not at all"
        );
    }

    #[test]
    fn codexs_composer_leaves_its_dim_placeholder_out() {
        let read = |parts: &[(u16, u16, &str)], cursor| read_composer(&drawn(parts, cursor), 10, 60, Provider::Codex);
        let history = (2, 1, "\x1b[2m› \x1b[22mReply with just OK.");
        let footer = (9, 1, "  GPT-6.1-Sol low · C:\\src");
        let empty = read(&[history, (6, 1, "› \x1b[2mAsk Codex to do anything\x1b[22m"), footer], (6, 3));
        assert_eq!(empty, Some(Composer { text: String::new(), stashed: false }));
        let typed = read(&[history, (6, 1, "› hello world"), (7, 1, "  second"), footer], (7, 9));
        assert_eq!(typed.map(|c| c.text).as_deref(), Some("hello world\nsecond"));
        // The cursor on a prompt in the conversation, drawn dim: not a composer.
        assert_eq!(read(&[history, footer], (2, 3)), None);
    }

    #[test]
    fn a_draft_is_what_was_typed_since_enter() {
        assert_eq!(draft_after(0, "abc"), 3);
        assert_eq!(draft_after(3, "\x7f"), 2);
        assert_eq!(draft_after(2, "\r"), 0);
        assert_eq!(draft_after(0, "\x1b[A\x1b[I\x1b[<35;1;14M\x1bOP"), 0, "keys that type nothing");
        assert_eq!(draft_after(0, "\x1b[200~hi\x1b[201~"), 2, "a paste is text");
        assert_eq!(draft_after(5, "\x03"), 0);
        assert_eq!(draft_after(5, "\x15"), 0);
        // lang-check: allow
        assert_eq!(draft_after(0, "привет"), 6);
        assert_eq!(draft_after(1, "\x7f\x7f\x7f"), 0);
        // A backslash and Enter, one key per write: a second line, not a send.
        let s = draft_after(draft_after(0, "ab\\"), "\r");
        assert_eq!(s & !DRAFT_BACKSLASH, 4);
        assert_eq!(draft_after(draft_after(s, "c"), "\r"), 0);
    }

    #[test]
    fn the_answer_is_where_the_cursor_stands() {
        assert_eq!(cursor_at(b"", 30, 100), (0, 0));
        assert_eq!(cursor_at(b"hello\r\nab", 30, 100), (1, 2));
        assert_eq!(cursor_at(b"\x1b[5;7H", 30, 100), (4, 6));
    }
}
