//! What a paste or a drop types into a session. The clipboard is the host's
//! (desktop.rs `Desktop::read_clipboard`), which says what is on it: files
//! before text, text before an image.
//!
//! - Text goes in as text, framed for bracketed paste when the program asked
//!   for it (keys.rs `paste`).
//! - Files, pasted or dropped, go in as their paths, each quoted for the
//!   session's CLI on this OS, space-separated, through the same paste: the
//!   CLIs attach an image whose path is pasted. A path that cannot be typed
//!   whole (not Unicode, or a control character in it) is left out.
//! - An image presses the CLI's own image-paste key, which reads the
//!   clipboard itself (docs/cli-compatibility.md).

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
    fn each_cli_has_its_image_key() {
        assert_eq!(Provider::Claude.image_key(), "\x1bv");
        assert_eq!(Provider::Codex.image_key(), "\x16");
    }
}
