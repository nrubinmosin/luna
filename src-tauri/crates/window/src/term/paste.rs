//! What a paste or a drop types into a session, and the clipboard it reads.
//!
//! - Text goes in as text, framed for bracketed paste when the program asked
//!   for it (keys.rs `paste`).
//! - Files, pasted or dropped, go in as their paths, each quoted for the
//!   session's CLI on this OS, space-separated, through the same paste: the
//!   CLIs attach an image whose path is pasted. A path that cannot be typed
//!   whole (not Unicode, or a control character in it) is left out.
//! - A bitmap with no text presses the CLI's own image-paste key, which reads
//!   the clipboard itself (docs/cli-compatibility.md).
//!
//! The clipboard is a trait: the system's through gpui, or, in the test build
//! only, a file standing in for it, so a smoke run never touches the
//! clipboard of the person at the machine.

use gpui::{App, ClipboardEntry, ClipboardItem};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The CLI a session runs, for what only it decides.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Provider {
    Claude,
    Codex,
}

impl Provider {
    /// The key the CLI takes an image from the clipboard on, as it reaches a
    /// PTY: Claude Code's Alt+V, Codex's Ctrl+V (the page's `pasteImageKey`).
    pub fn image_key(self) -> &'static str {
        match self {
            Provider::Claude => "\x1bv",
            Provider::Codex => "\x16",
        }
    }
}

/// What the clipboard holds, as a paste takes it.
#[derive(Clone, Debug, PartialEq)]
pub enum Clip {
    Text(String),
    Files(Vec<PathBuf>),
    /// A bitmap and no text.
    Image,
}

/// What a paste reads and a copy writes.
pub trait Clipboard: 'static {
    fn read(&self, cx: &mut App) -> Option<Clip>;
    fn write_text(&self, text: String, cx: &mut App);
}

/// The system's clipboard, through gpui's platform.
pub struct SystemClipboard;

impl Clipboard for SystemClipboard {
    fn read(&self, cx: &mut App) -> Option<Clip> {
        cx.read_from_clipboard().and_then(|item| clip_of(&item))
    }

    fn write_text(&self, text: String, cx: &mut App) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }
}

/// What a clipboard item pastes as: files before text (Explorer's copy
/// carries both), text before a bitmap (a spreadsheet's copy carries both,
/// and the text is what was meant), and nothing for an empty one.
pub fn clip_of(item: &ClipboardItem) -> Option<Clip> {
    let entries = item.entries();
    let files: Vec<PathBuf> = entries
        .iter()
        .filter_map(|e| match e {
            ClipboardEntry::ExternalPaths(p) => Some(p.paths().to_vec()),
            _ => None,
        })
        .flatten()
        .collect();
    if !files.is_empty() {
        return Some(Clip::Files(files));
    }
    let text: String = entries
        .iter()
        .filter_map(|e| match e {
            ClipboardEntry::String(s) => Some(s.text().as_str()),
            _ => None,
        })
        .collect();
    if !text.is_empty() {
        return Some(Clip::Text(text));
    }
    entries.iter().any(|e| matches!(e, ClipboardEntry::Image(_))).then_some(Clip::Image)
}

/// The test build's stand-in (`LUNA_TEST_CLIPBOARD`, src/native.rs): a JSON
/// file a smoke script writes what is "on the clipboard" into —
/// `{"text": "..."}`, `{"files": ["..."]}` or `{"image": true}` — and a copy
/// writes `{"text": "..."}` into.
pub struct FileClipboard(pub PathBuf);

#[derive(Default, Deserialize, Serialize)]
#[serde(default)]
struct FileClip {
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    files: Vec<PathBuf>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    image: bool,
}

impl Clipboard for FileClipboard {
    fn read(&self, _cx: &mut App) -> Option<Clip> {
        let clip: FileClip = serde_json::from_slice(&std::fs::read(&self.0).ok()?).ok()?;
        if !clip.files.is_empty() {
            Some(Clip::Files(clip.files))
        } else if let Some(text) = clip.text.filter(|t| !t.is_empty()) {
            Some(Clip::Text(text))
        } else {
            clip.image.then_some(Clip::Image)
        }
    }

    fn write_text(&self, text: String, _cx: &mut App) {
        let clip = FileClip { text: Some(text), ..Default::default() };
        let written =
            serde_json::to_vec(&clip).map_err(|e| e.to_string()).and_then(|b| std::fs::write(&self.0, b).map_err(|e| e.to_string()));
        if let Err(e) = written {
            log::warn!("the test clipboard {}: {e}", self.0.display());
        }
    }
}

/// How a path is quoted for the CLI it is typed into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quoting {
    /// In double quotes, which no Windows path holds: what the page types.
    Windows,
    /// Bare when it has nothing a shell would split or expand, else in
    /// single quotes, a single quote as `'\''`: both CLIs read a pasted path
    /// either way.
    Posix,
}

impl Quoting {
    pub const HERE: Quoting = if cfg!(windows) { Quoting::Windows } else { Quoting::Posix };
}

/// One path as typed, or None when it cannot be typed whole.
pub fn quote(path: &Path, quoting: Quoting) -> Option<String> {
    let s = path.to_str()?;
    if s.is_empty() || s.chars().any(char::is_control) {
        return None;
    }
    Some(match quoting {
        Quoting::Windows if s.contains('"') => return None,
        Quoting::Windows => format!("\"{s}\""),
        Quoting::Posix if s.chars().all(|c| c.is_alphanumeric() || "_-./+@%:,=".contains(c)) => s.to_owned(),
        Quoting::Posix => format!("'{}'", s.replace('\'', "'\\''")),
    })
}

/// The paths as typed, space-separated; None when none of them can be.
pub fn paths_text(paths: &[PathBuf], quoting: Quoting) -> Option<String> {
    let typed: Vec<String> = paths
        .iter()
        .filter_map(|p| {
            let q = quote(p, quoting);
            if q.is_none() {
                log::warn!("a path that cannot be typed was left out: {}", p.display());
            }
            q
        })
        .collect();
    (!typed.is_empty()).then(|| typed.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{ExternalPaths, Image, ImageFormat};

    #[test]
    fn paths_are_quoted_for_each_os() {
        let p = |s: &str| PathBuf::from(s);
        assert_eq!(quote(&p(r"C:\Users\a b\shot.png"), Quoting::Windows).as_deref(), Some(r#""C:\Users\a b\shot.png""#));
        assert_eq!(quote(&p("/tmp/plain-file_1.png"), Quoting::Posix).as_deref(), Some("/tmp/plain-file_1.png"));
        assert_eq!(quote(&p("/tmp/a b.png"), Quoting::Posix).as_deref(), Some("'/tmp/a b.png'"));
        assert_eq!(quote(&p("/tmp/it's $HOME"), Quoting::Posix).as_deref(), Some(r"'/tmp/it'\''s $HOME'"));
        // What cannot be typed whole is left out, never typed in part.
        assert_eq!(quote(&p("/tmp/a\nb"), Quoting::Posix), None);
        assert_eq!(quote(&p("x\"y"), Quoting::Windows), None);
        let both = [p("/a b"), p("/c\rd"), p("/e")];
        assert_eq!(paths_text(&both, Quoting::Posix).as_deref(), Some("'/a b' /e"));
        assert_eq!(paths_text(&[p("/c\rd")], Quoting::Posix), None);
    }

    #[test]
    fn files_before_text_before_a_bitmap() {
        let image = ClipboardEntry::Image(Image::from_bytes(ImageFormat::Png, vec![1, 2, 3]));
        let files = ClipboardEntry::ExternalPaths(ExternalPaths(vec![PathBuf::from("/a.png")].into()));
        let text = ClipboardEntry::String(gpui::ClipboardString::new("hi".into()));
        let item = |entries: Vec<ClipboardEntry>| ClipboardItem { entries };
        assert_eq!(clip_of(&item(vec![text.clone(), files.clone()])), Some(Clip::Files(vec![PathBuf::from("/a.png")])));
        assert_eq!(clip_of(&item(vec![image.clone(), text.clone()])), Some(Clip::Text("hi".into())));
        assert_eq!(clip_of(&item(vec![image])), Some(Clip::Image));
        assert_eq!(clip_of(&item(vec![])), None);
    }

    #[test]
    fn each_cli_has_its_image_key() {
        assert_eq!(Provider::Claude.image_key(), "\x1bv");
        assert_eq!(Provider::Codex.image_key(), "\x16");
    }

    #[test]
    fn the_test_clipboard_is_a_file() {
        let dir = std::env::temp_dir().join(format!("luna-window-clip-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("clipboard.json");
        let clip = FileClipboard(file.clone());
        let app = gpui::TestAppContext::single();
        app.update(|cx| {
            assert_eq!(clip.read(cx), None, "no file, nothing on it");
            std::fs::write(&file, r#"{"files":["/x y.png"]}"#).unwrap();
            assert_eq!(clip.read(cx), Some(Clip::Files(vec![PathBuf::from("/x y.png")])));
            std::fs::write(&file, r#"{"image":true}"#).unwrap();
            assert_eq!(clip.read(cx), Some(Clip::Image));
            clip.write_text("copied".into(), cx);
            assert_eq!(clip.read(cx), Some(Clip::Text("copied".into())));
        });
        let _ = std::fs::remove_dir_all(dir);
    }
}
