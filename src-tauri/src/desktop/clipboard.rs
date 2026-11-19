//! The clipboard, as the native window needs it: what a paste would put in a
//! terminal (`read`), and text to copy (`write_text`). The window types
//! files as their quoted paths and, for an image, presses the CLI's own
//! image-paste key, so the CLI reads the bitmap off the clipboard itself
//! (ADR-0030: no attachments store); `read` tells it which of the two it is.
//!
//! What one copy puts there often comes in several formats at once, and
//! `read` takes the first of: files (Explorer's CF_HDROP, Finder's file
//! URLs), text, an image. Text before an image: a spreadsheet's cells come
//! with a picture of themselves, a copied image seldom with text. An image
//! is never read here, only looked for among the formats on offer: the CLI
//! reads it.
//!
//! Windows reads the formats itself, through the parsers below that tests
//! hold to hand-made bytes; macOS asks NSPasteboard. Linux, which Luna does
//! not support, has no clipboard here. Never call this from a test: the
//! clipboard is the person's.

use std::path::PathBuf;

/// What is on the clipboard, by what a paste does with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Clip {
    Empty,
    Text(String),
    Files(Vec<PathBuf>),
    /// An image format is on offer (a bitmap, a PNG, a TIFF), not read.
    Image,
}

/// What is on the clipboard now.
pub fn read() -> Result<Clip, String> {
    #[cfg(windows)]
    return win::read();
    #[cfg(target_os = "macos")]
    return mac::read();
    #[cfg(not(any(windows, target_os = "macos")))]
    Err("no clipboard on this platform".into())
}

/// Puts `text` on the clipboard, in place of what was there.
pub fn write_text(text: &str) -> Result<(), String> {
    #[cfg(windows)]
    return win::write_text(text);
    #[cfg(target_os = "macos")]
    return mac::write_text(text);
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = text;
        Err("no clipboard on this platform".into())
    }
}

/// The most bytes of one format read off the clipboard (text, a file list):
/// far past anything a person pastes into a terminal.
pub const MAX_BYTES: usize = 64 << 20;

/// The most files one paste takes as files: past this many, the list is
/// not taken as files, and the clipboard is read for its next format.
pub const MAX_FILES: usize = 1024;

/// The longest path, in UTF-16 units, a paste takes as a file's: the
/// longest Windows has.
pub const MAX_PATH_UNITS: usize = 32_767;

/// Whether a path off the clipboard may be typed into a CLI whole, quoted
/// as luna_window types it where it was copied (its paste.rs `quote`): no
/// control character (CR, LF, tab, NUL, C1 and the rest), which a terminal
/// would act on, and on Windows, whose paths go in double quotes, no `"`,
/// which would end them early; POSIX's single quotes hold one. A list with
/// one such path is not taken as files at all.
pub fn typeable_path(path: &str, windows: bool) -> bool {
    !path.is_empty() && !(windows && path.contains('"')) && !path.chars().any(char::is_control)
}

/// CF_UNICODETEXT's bytes: UTF-16LE up to its NUL.
pub fn utf16_text(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).take_while(|&u| u != 0).collect();
    String::from_utf16_lossy(&units)
}

/// The size of DROPFILES' header: pFiles (4), pt (8), fNC (4), fWide (4).
const DROPFILES_SIZE: usize = 20;

/// CF_HDROP's bytes, a DROPFILES: its header, then at `pFiles`, past the
/// header, the paths, each ended by a NUL and the list by another, in UTF-16
/// when `fWide`, else in the system's ANSI code page, which `ansi` decodes.
/// A list that runs off its end before its closing NUL, a name that does not
/// decode, a path `typeable_path` refuses, more than `MAX_FILES` paths and
/// one longer than `MAX_PATH_UNITS` make it an error, and the clipboard is
/// read for its next format.
pub fn drop_files(bytes: &[u8], ansi: &dyn Fn(&[u8]) -> Option<String>) -> Result<Vec<PathBuf>, String> {
    if bytes.len() < DROPFILES_SIZE {
        return Err("a file list too short for its header".into());
    }
    let start = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
    if start < DROPFILES_SIZE {
        return Err(format!("a file list that starts inside its header (at {start})"));
    }
    let wide = u32::from_le_bytes(bytes[16..20].try_into().unwrap()) != 0;
    let list = bytes.get(start..).ok_or("a file list that starts past its end")?;
    let unended = || "a file list without its closing NUL".to_string();
    // Room for one more name of `units`, before it is decoded.
    let take = |names: &Vec<String>, units: usize| {
        if names.len() >= MAX_FILES {
            return Err(format!("a file list of more than {MAX_FILES} files"));
        }
        if units > MAX_PATH_UNITS {
            return Err(format!("a file name longer than {MAX_PATH_UNITS} units"));
        }
        Ok(())
    };
    let mut names = Vec::new();
    if wide {
        let units: Vec<u16> = list.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
        let mut rest = &units[..];
        loop {
            let end = rest.iter().position(|&u| u == 0).ok_or_else(unended)?;
            if end == 0 {
                break;
            }
            take(&names, end)?;
            names.push(String::from_utf16(&rest[..end]).map_err(|_| "a file name that is not UTF-16".to_string())?);
            rest = &rest[end + 1..];
        }
    } else {
        let mut rest = list;
        loop {
            let end = rest.iter().position(|&b| b == 0).ok_or_else(unended)?;
            if end == 0 {
                break;
            }
            // A code page's byte or two makes one UTF-16 unit at most.
            take(&names, end.div_ceil(2))?;
            let name = ansi(&rest[..end]).ok_or("a file name the code page does not decode")?;
            take(&names, name.encode_utf16().count())?;
            names.push(name);
            rest = &rest[end + 1..];
        }
    }
    if let Some(bad) = names.iter().find(|n| !typeable_path(n, true)) {
        return Err(format!("a file name with a quote or a control character: {bad:?}"));
    }
    Ok(names.into_iter().map(PathBuf::from).collect())
}

/// A `file://` URL's path, percent-decoded: what a pasteboard's file URL
/// names when macOS cannot resolve it itself.
pub fn file_url_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    if !rest.starts_with('/') {
        return None;
    }
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3).filter(|h| h.iter().all(u8::is_ascii_hexdigit))?;
            out.push(u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok().map(PathBuf::from)
}

#[cfg(windows)]
mod win {
    use super::*;
    use windows::core::w;
    use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
    use windows::Win32::Globalization::{MultiByteToWideChar, CP_ACP, MB_ERR_INVALID_CHARS};
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW,
        SetClipboardData,
    };
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};
    use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, HWND_MESSAGE, WINDOW_EX_STYLE, WINDOW_STYLE};

    const CF_DIB: u32 = 8;
    const CF_UNICODETEXT: u32 = 13;
    const CF_HDROP: u32 = 15;
    const CF_DIBV5: u32 = 17;

    /// The clipboard, open for as long as this lives. Another program may
    /// hold it for a moment: a few tries before giving up.
    struct Open;

    impl Open {
        /// Opened for `owner`: none to read; a window of this thread's to
        /// write (`Owner`).
        fn new(owner: Option<HWND>) -> Result<Open, String> {
            let mut last = String::new();
            for _ in 0..10 {
                // SAFETY: a window of this thread's or none; closed again by
                // Drop on this thread.
                match unsafe { OpenClipboard(owner) } {
                    Ok(()) => return Ok(Open),
                    Err(e) => last = e.to_string(),
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(format!("the clipboard is held by another program: {last}"))
        }
    }

    impl Drop for Open {
        fn drop(&mut self) {
            // SAFETY: opened by this thread in `Open::new`.
            let _ = unsafe { CloseClipboard() };
        }
    }

    /// A message-only window that owns the clipboard while text is put
    /// there. Opened with no owner, EmptyClipboard leaves the clipboard
    /// owned by none, and SetClipboardData may then fail (OpenClipboard's
    /// remarks). Destroyed once the clipboard is closed; what was set stays,
    /// as none of it is rendered late.
    struct Owner(HWND);

    impl Owner {
        fn new() -> Result<Owner, String> {
            // SAFETY: a window of the system's STATIC class, under
            // HWND_MESSAGE, destroyed by Drop on this thread.
            let made = unsafe {
                CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!(""), WINDOW_STYLE(0), 0, 0, 0, 0, Some(HWND_MESSAGE), None, None, None)
            };
            made.map(Owner).map_err(|e| format!("no window to own the clipboard: {e}"))
        }
    }

    impl Drop for Owner {
        fn drop(&mut self) {
            // SAFETY: made by this thread in `Owner::new`.
            let _ = unsafe { DestroyWindow(self.0) };
        }
    }

    /// A format's bytes, if the clipboard has it and they are no more than
    /// `MAX_BYTES`. With the clipboard open.
    fn bytes(_open: &Open, format: u32) -> Option<Vec<u8>> {
        // SAFETY: the clipboard is open; the handle it gives is its own and
        // stays valid until it is closed, and is only read here, locked.
        unsafe {
            IsClipboardFormatAvailable(format).ok()?;
            let handle = GetClipboardData(format).ok()?;
            let global = HGLOBAL(handle.0);
            let size = GlobalSize(global);
            if size > MAX_BYTES {
                return None;
            }
            let at = GlobalLock(global) as *const u8;
            if at.is_null() {
                return None;
            }
            let copy = std::slice::from_raw_parts(at, size).to_vec();
            let _ = GlobalUnlock(global);
            Some(copy)
        }
    }

    /// Bytes in the system's ANSI code page, decoded; None where they do not
    /// decode.
    fn ansi(bytes: &[u8]) -> Option<String> {
        // SAFETY: the input is a slice; the output buffer is of the size the
        // first call gave.
        unsafe {
            let n = MultiByteToWideChar(CP_ACP, MB_ERR_INVALID_CHARS, bytes, None);
            if n <= 0 {
                return None;
            }
            let mut wide = vec![0u16; n as usize];
            let n = MultiByteToWideChar(CP_ACP, MB_ERR_INVALID_CHARS, bytes, Some(&mut wide));
            if n <= 0 {
                return None;
            }
            String::from_utf16(&wide[..n as usize]).ok()
        }
    }

    /// The first format that reads; one that does not (a file list that
    /// does not parse) gives way to the next. An image is only looked for:
    /// a bitmap (CF_DIB, CF_DIBV5, and CF_BITMAP, which Windows makes of
    /// them and they of it) or a browser's PNG on offer, none of it read.
    pub fn read() -> Result<Clip, String> {
        let open = Open::new(None)?;
        if let Some(files) = bytes(&open, CF_HDROP).and_then(|b| drop_files(&b, &ansi).ok()) {
            if !files.is_empty() {
                return Ok(Clip::Files(files));
            }
        }
        if let Some(b) = bytes(&open, CF_UNICODETEXT) {
            let text = utf16_text(&b);
            if !text.is_empty() {
                return Ok(Clip::Text(text));
            }
        }
        // SAFETY: a constant name.
        let png = unsafe { RegisterClipboardFormatW(w!("PNG")) };
        let offered = |format: u32| {
            // SAFETY: asks about a format; reads nothing of it.
            format != 0 && unsafe { IsClipboardFormatAvailable(format) }.is_ok()
        };
        if [CF_DIBV5, CF_DIB, png].into_iter().any(offered) {
            return Ok(Clip::Image);
        }
        Ok(Clip::Empty)
    }

    pub fn write_text(text: &str) -> Result<(), String> {
        let units: Vec<u16> = text.encode_utf16().chain([0]).collect();
        // Made and filled before the clipboard is touched: a failure here
        // leaves what is there as it was.
        // SAFETY: the memory is filled under its lock; freed here unless
        // SetClipboardData takes it, after which it is the system's.
        let global = unsafe {
            let global = GlobalAlloc(GMEM_MOVEABLE, units.len() * 2).map_err(|e| e.to_string())?;
            let at = GlobalLock(global) as *mut u16;
            if at.is_null() {
                let _ = GlobalFree(Some(global));
                return Err("could not lock the clipboard's memory".into());
            }
            std::ptr::copy_nonoverlapping(units.as_ptr(), at, units.len());
            let _ = GlobalUnlock(global);
            global
        };
        let free = || {
            // SAFETY: not taken by the clipboard.
            let _ = unsafe { GlobalFree(Some(global)) };
        };
        let owner = match Owner::new() {
            Ok(owner) => owner,
            Err(e) => {
                free();
                return Err(e);
            }
        };
        // Closed before the owner goes (dropped in reverse order).
        let _open = match Open::new(Some(owner.0)) {
            Ok(open) => open,
            Err(e) => {
                free();
                return Err(e);
            }
        };
        // SAFETY: the clipboard is open on this thread, owned by `owner`.
        unsafe {
            if let Err(e) = EmptyClipboard() {
                free();
                return Err(e.to_string());
            }
            if let Err(e) = SetClipboardData(CF_UNICODETEXT, Some(HANDLE(global.0))) {
                free();
                return Err(e.to_string());
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeFileURL, NSPasteboardTypePNG, NSPasteboardTypeString, NSPasteboardTypeTIFF};
    use objc2_foundation::{NSString, NSURL};

    /// A file URL's path. Finder's are file reference URLs
    /// (`file:///.file/id=…`), which only NSURL resolves.
    fn file_path(url: &str) -> Option<PathBuf> {
        // SAFETY: plain Foundation calls on objects made and held here.
        #[allow(unused_unsafe)]
        let resolved = unsafe {
            NSURL::URLWithString(&NSString::from_str(url)).and_then(|u| u.filePathURL()).and_then(|u| u.path()).map(|p| p.to_string())
        };
        resolved.map(PathBuf::from).or_else(|| file_url_path(url))
    }

    pub fn read() -> Result<Clip, String> {
        // SAFETY: the general pasteboard, read on the calling thread; the
        // type names are AppKit's constants.
        #[allow(unused_unsafe)]
        unsafe {
            let pb = NSPasteboard::generalPasteboard();
            let mut files = Vec::new();
            // More items than a paste takes as files: not files, then.
            if let Some(items) = pb.pasteboardItems().filter(|items| items.count() <= MAX_FILES) {
                for i in 0..items.count() {
                    let item = items.objectAtIndex(i);
                    if let Some(url) = item.stringForType(NSPasteboardTypeFileURL) {
                        files.extend(file_path(&url.to_string()));
                    }
                }
            }
            // A path that cannot be typed as a quoted one, or longer than any
            // a paste takes: not files, then.
            let fits = |f: &PathBuf| f.to_str().is_some_and(|f| typeable_path(f, false) && f.encode_utf16().count() <= MAX_PATH_UNITS);
            if !files.is_empty() && files.iter().all(fits) {
                return Ok(Clip::Files(files));
            }
            if let Some(text) = pb.stringForType(NSPasteboardTypeString) {
                let text = text.to_string();
                if !text.is_empty() {
                    return Ok(Clip::Text(text));
                }
            }
            // A PNG, or the TIFF a screenshot or an image app's copy is: on
            // offer, none of it read.
            let image =
                pb.types().is_some_and(|types| types.containsObject(NSPasteboardTypePNG) || types.containsObject(NSPasteboardTypeTIFF));
            if image {
                return Ok(Clip::Image);
            }
            Ok(Clip::Empty)
        }
    }

    pub fn write_text(text: &str) -> Result<(), String> {
        // SAFETY: as in `read`.
        #[allow(unused_unsafe)]
        unsafe {
            let pb = NSPasteboard::generalPasteboard();
            pb.clearContents();
            if pb.setString_forType(&NSString::from_str(text), NSPasteboardTypeString) {
                Ok(())
            } else {
                Err("the pasteboard refused the text".into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn le32(v: u32) -> [u8; 4] {
        v.to_le_bytes()
    }

    /// A DROPFILES of `names`, wide or not.
    fn dropfiles(names: &[&str], wide: bool) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend(le32(20));
        b.extend([0; 12]);
        b.extend(le32(wide as u32));
        for n in names {
            if wide {
                b.extend(n.encode_utf16().chain([0]).flat_map(u16::to_le_bytes));
            } else {
                b.extend(n.bytes().chain([0]));
            }
        }
        b.extend(if wide { vec![0, 0] } else { vec![0] });
        b
    }

    /// The ANSI code page as Windows-1252 has it, as far as the tests go:
    /// Latin-1 for the bytes they use.
    fn latin1(bytes: &[u8]) -> Option<String> {
        Some(bytes.iter().map(|&b| char::from(b)).collect())
    }

    #[test]
    fn a_file_list_gives_every_path_it_holds() {
        let names = [r"C:\Users\a\Pictures\shot 1.png", r"D:\Bücher\notes.md"];
        let got = drop_files(&dropfiles(&names, true), &latin1).unwrap();
        assert_eq!(got, names.iter().map(PathBuf::from).collect::<Vec<_>>());
        assert_eq!(drop_files(&dropfiles(&[r"C:\a.txt"], false), &latin1).unwrap(), [PathBuf::from(r"C:\a.txt")]);
        assert!(drop_files(&dropfiles(&[], true), &latin1).unwrap().is_empty());
        // ANSI names go through the code page, not as UTF-8.
        let mut ansi = dropfiles(&[], false);
        ansi.truncate(20);
        ansi.extend(b"C:\\B\xfccher\0\0");
        assert_eq!(drop_files(&ansi, &latin1).unwrap(), [PathBuf::from(r"C:\Bücher")]);
    }

    #[test]
    fn a_file_list_that_breaks_its_own_layout_is_refused() {
        assert!(drop_files(&[0; 8], &latin1).is_err());
        let mut past = dropfiles(&["x"], true);
        past[0..4].copy_from_slice(&le32(4096));
        assert!(drop_files(&past, &latin1).is_err());
        // pFiles inside the header: the header read as names.
        let mut inside = dropfiles(&["x"], true);
        inside[0..4].copy_from_slice(&le32(4));
        assert!(drop_files(&inside, &latin1).unwrap_err().contains("inside its header"));
        // No closing NUL: the last name, or the list, runs off the end.
        let mut open = dropfiles(&[r"C:\a"], true);
        open.truncate(open.len() - 2);
        assert!(drop_files(&open, &latin1).unwrap_err().contains("closing NUL"));
        open.truncate(open.len() - 2);
        assert!(drop_files(&open, &latin1).unwrap_err().contains("closing NUL"));
        let mut open = dropfiles(&[r"C:\a"], false);
        open.truncate(open.len() - 2);
        assert!(drop_files(&open, &latin1).is_err());
        // A lone surrogate is no path.
        let mut odd = dropfiles(&[], true);
        odd.truncate(20);
        odd.extend([0x00, 0xD8, 0, 0, 0, 0]);
        assert!(drop_files(&odd, &latin1).is_err());
    }

    #[test]
    fn a_path_that_cannot_be_typed_in_quotes_is_no_file() {
        for bad in ["C:\\a\"b.txt", "C:\\a\rb", "C:\\a\nb", "C:\\a\tb", "C:\\a\u{1b}[2J", "C:\\a\u{85}b"] {
            assert!(!typeable_path(bad, true), "{bad:?}");
            assert!(drop_files(&dropfiles(&[r"C:\fine.txt", bad], true), &latin1).is_err(), "{bad:?}");
        }
        assert!(typeable_path(r"C:\Users\a b\c'd (1).png", true));
        assert!(!typeable_path("", true));
        // POSIX's single quotes hold a double quote; a control character is
        // still no path.
        assert!(typeable_path("/Users/a/say \"hi\".png", false));
        assert!(!typeable_path("/Users/a/x\ny.png", false));
        assert!(!typeable_path("", false));
    }

    #[test]
    fn a_file_list_past_the_caps_is_no_files() {
        let names: Vec<String> = (0..MAX_FILES).map(|i| format!(r"C:\f{i}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        assert_eq!(drop_files(&dropfiles(&refs, true), &latin1).unwrap().len(), MAX_FILES);
        let mut more = refs.clone();
        more.push(r"C:\one-too-many");
        for wide in [true, false] {
            assert!(drop_files(&dropfiles(&more, wide), &latin1).unwrap_err().contains("more than"), "wide: {wide}");
        }
        // One path at the longest Windows has, and one unit over it.
        let longest = format!(r"C:\{}", "a".repeat(MAX_PATH_UNITS - 3));
        assert_eq!(drop_files(&dropfiles(&[&longest], true), &latin1).unwrap().len(), 1);
        let over = format!("{longest}a");
        for wide in [true, false] {
            assert!(drop_files(&dropfiles(&[&over], wide), &latin1).unwrap_err().contains("longer than"), "wide: {wide}");
        }
    }

    #[test]
    fn text_stops_at_its_nul() {
        let bytes: Vec<u8> = "héllo\0junk".encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(utf16_text(&bytes), "héllo");
        assert_eq!(utf16_text(&[]), "");
    }

    #[test]
    fn a_file_url_names_its_path_decoded() {
        assert_eq!(file_url_path("file:///Users/a/My%20Shot.png"), Some(PathBuf::from("/Users/a/My Shot.png")));
        assert_eq!(file_url_path("file://localhost/tmp/x"), Some(PathBuf::from("/tmp/x")));
        assert_eq!(file_url_path("file:///tmp/%C3%A9t%C3%A9"), Some(PathBuf::from("/tmp/été")));
        assert_eq!(file_url_path("https://example.com/x"), None);
        assert_eq!(file_url_path("file:///bad%zz"), None);
        assert_eq!(file_url_path("file:///cut%2"), None);
    }
}
