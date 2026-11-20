//! The window's one font, Cascadia Mono, carried in the exe so every platform
//! draws the same letters: Windows 11 ships the face with Terminal, macOS and
//! Linux do not. The static Regular and Bold of Microsoft's release 2407.24
//! (SIL Open Font License 1.1, `assets/fonts/OFL.txt`); their SHA-256 sums are
//! in docs/development.md, "The window's font".

use gpui::App;
use std::borrow::Cow;

/// The family every text in the window asks for.
pub const FAMILY: &str = "Cascadia Mono";

const REGULAR: &[u8] = include_bytes!("../assets/fonts/CascadiaMono-Regular.ttf");
const BOLD: &[u8] = include_bytes!("../assets/fonts/CascadiaMono-Bold.ttf");

/// Hands both faces to gpui's text system, before any text is laid out.
pub fn register(cx: &mut App) -> Result<(), String> {
    cx.text_system().add_fonts(vec![Cow::Borrowed(REGULAR), Cow::Borrowed(BOLD)]).map_err(|e| format!("{e:#}"))
}
