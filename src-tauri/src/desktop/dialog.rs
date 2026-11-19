//! The folder and file pickers, on `rfd`, the crate Tauri's dialog plugin is
//! built on. The page asks for a folder only (src/ipc/cli.ts `pickFolder`:
//! the new chat's folder, the accounts root, the library's folders, Codex's
//! trusted roots); a file picker is here for the window that will want one.
//!
//! The blocking calls run the system's modal dialog on the calling thread.
//! On macOS that has to be the main thread; on Windows any thread will do
//! (rfd initializes COM on it), so the window may run them off its loop.
//! The `_async` ones return a future the window can await from its loop on
//! either platform: rfd then puts the dialog where the platform wants it.
//! A `parent` makes the dialog modal to that window, as the page's was.

use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use std::path::{Path, PathBuf};

/// A window a dialog can be modal to: any with both handles, a GPUI window
/// among them.
pub trait Parent: HasWindowHandle + HasDisplayHandle {}

impl<T: HasWindowHandle + HasDisplayHandle + ?Sized> Parent for T {}

/// What to ask with.
#[derive(Clone, Debug, Default)]
pub struct Ask {
    pub title: Option<String>,
    /// The folder the dialog opens in; the nearest one of it that exists.
    pub start: Option<PathBuf>,
    /// For a file: the kinds offered, each with its extensions.
    pub filters: Vec<Filter>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filter {
    pub name: String,
    /// `md`, `.md` or `*.md` alike.
    pub extensions: Vec<String>,
}

/// The nearest folder at or above `p` that exists: a dialog given a missing
/// one opens wherever the system pleases.
pub fn start_dir(p: &Path) -> Option<PathBuf> {
    p.ancestors().find(|a| !a.as_os_str().is_empty() && a.is_dir()).map(Path::to_path_buf)
}

/// An extension as the dialog takes it: no `*.` or `.` before it.
fn bare(ext: &str) -> &str {
    ext.trim().trim_start_matches('*').trim_start_matches('.')
}

macro_rules! dressed {
    ($dialog:expr, $ask:expr, $parent:expr) => {{
        let mut d = $dialog;
        if let Some(title) = &$ask.title {
            d = d.set_title(title.clone());
        }
        if let Some(dir) = $ask.start.as_deref().and_then(start_dir) {
            d = d.set_directory(dir);
        }
        for f in &$ask.filters {
            let exts: Vec<&str> = f.extensions.iter().map(|e| bare(e)).filter(|e| !e.is_empty()).collect();
            d = d.add_filter(f.name.clone(), &exts);
        }
        if let Some(parent) = $parent {
            d = d.set_parent(parent);
        }
        d
    }};
}

/// Asks for a folder; `None` if the person cancels.
pub fn folder(ask: &Ask, parent: Option<&dyn Parent>) -> Option<PathBuf> {
    dressed!(rfd::FileDialog::new(), ask, parent).pick_folder()
}

/// Asks for a file to open; `None` if the person cancels.
pub fn file(ask: &Ask, parent: Option<&dyn Parent>) -> Option<PathBuf> {
    dressed!(rfd::FileDialog::new(), ask, parent).pick_file()
}

/// `folder`, awaited.
pub async fn folder_async(ask: &Ask, parent: Option<&dyn Parent>) -> Option<PathBuf> {
    let picked = dressed!(rfd::AsyncFileDialog::new(), ask, parent).pick_folder();
    picked.await.map(|h| h.path().to_path_buf())
}

/// `file`, awaited.
pub async fn file_async(ask: &Ask, parent: Option<&dyn Parent>) -> Option<PathBuf> {
    let picked = dressed!(rfd::AsyncFileDialog::new(), ask, parent).pick_file();
    picked.await.map(|h| h.path().to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_start_opens_at_the_nearest_folder_that_is_there() {
        let dir = std::env::temp_dir().join(format!("luna-dialog-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(start_dir(&dir.join("gone").join("deeper")), Some(dir.clone()));
        assert_eq!(start_dir(&dir), Some(dir.clone()));
        // A file starts at its folder.
        std::fs::write(dir.join("a.md"), "").unwrap();
        assert_eq!(start_dir(&dir.join("a.md")), Some(dir.clone()));
        assert_eq!(start_dir(Path::new("relative-and-missing")), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_extension_is_taken_however_it_is_written() {
        assert_eq!(bare("md"), "md");
        assert_eq!(bare(".md"), "md");
        assert_eq!(bare("*.md"), "md");
        assert_eq!(bare(" *.tar.gz "), "tar.gz");
    }
}
