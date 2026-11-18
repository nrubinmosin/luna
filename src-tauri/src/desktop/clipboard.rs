//! The clipboard, as the native window needs it: what a paste would put in a
//! terminal (`read`), and text to copy (`write_text`). The window types
//! files as their quoted paths and, for an image, presses the CLI's own
//! image-paste key, so the CLI reads the bitmap off the clipboard itself
//! (ADR-0030: no attachments store); `read` tells it which of the two it is.
//!
//! What one copy puts there often comes in several formats at once, and
//! `read` takes the first of: files (Explorer's CF_HDROP, Finder's file
//! URLs), text, an image. Text before an image: a spreadsheet's cells come
//! with a picture of themselves, a copied image seldom with text.
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
    Image(Image),
}

/// An image, as PNG.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub png: Vec<u8>,
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

/// The most pixels an image off the clipboard may have: 64 megapixels,
/// 256 MiB as RGBA. Above any screen's or screenshot's, and checked before
/// anything of the size is allocated.
pub const MAX_PIXELS: u64 = 64 << 20;

/// The most bytes of one format read off the clipboard: such an image as a
/// DIB, with room for its header.
pub const MAX_BYTES: usize = (MAX_PIXELS as usize) * 4 + (1 << 20);

/// An image from PNG bytes, its size read from the header.
pub fn image_from_png(png: Vec<u8>) -> Result<Image, String> {
    // The signature, then IHDR first: length, type, width, height.
    if png.len() < 24 || &png[..8] != b"\x89PNG\r\n\x1a\n" || &png[12..16] != b"IHDR" {
        return Err("not a PNG".into());
    }
    let width = u32::from_be_bytes(png[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(png[20..24].try_into().unwrap());
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(format!("an image of {width}x{height}, over {MAX_PIXELS} pixels"));
    }
    Ok(Image { width, height, png })
}

/// Whether a path off the clipboard may be typed into a CLI as a quoted
/// path: no `"`, which would end the quotes early, and no control character
/// (CR, LF, tab, NUL and the rest), which a terminal would act on. A list
/// with one such path is not taken as files at all.
pub fn typeable_path(path: &str) -> bool {
    !path.is_empty() && !path.contains('"') && !path.chars().any(char::is_control)
}

/// RGBA pixels, row by row from the top, as PNG.
pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(rgba).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    Ok(out)
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
/// decode and a path `typeable_path` refuses make it an error, and the
/// clipboard is read for its next format.
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
    let mut names = Vec::new();
    if wide {
        let units: Vec<u16> = list.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
        let mut rest = &units[..];
        loop {
            let end = rest.iter().position(|&u| u == 0).ok_or_else(unended)?;
            if end == 0 {
                break;
            }
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
            names.push(ansi(&rest[..end]).ok_or("a file name the code page does not decode")?);
            rest = &rest[end + 1..];
        }
    }
    if let Some(bad) = names.iter().find(|n| !typeable_path(n)) {
        return Err(format!("a file name with a quote or a control character: {bad:?}"));
    }
    Ok(names.into_iter().map(PathBuf::from).collect())
}

/// A packed DIB (CF_DIB, CF_DIBV5: a BITMAPINFOHEADER or BITMAPV5HEADER, its
/// masks or palette, then the rows) as RGBA rows from the top. 24- and
/// 32-bit, uncompressed or by bit fields: what screenshots and browsers put
/// there. A 32-bit DIB whose alpha is zero all over carries none and is
/// opaque.
pub fn dib_to_rgba(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let u32_at = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    let u16_at = |at: usize| bytes.get(at..at + 2).map(|b| u16::from_le_bytes(b.try_into().unwrap()));
    let short = || "a bitmap shorter than its header".to_string();
    let header = u32_at(0).ok_or_else(short)? as usize;
    if header < 40 {
        return Err(format!("a bitmap header of {header} bytes"));
    }
    let width = u32_at(4).ok_or_else(short)? as i32;
    let height = u32_at(8).ok_or_else(short)? as i32;
    let bits = u16_at(14).ok_or_else(short)?;
    let compression = u32_at(16).ok_or_else(short)?;
    let colors = u32_at(32).ok_or_else(short)? as usize;
    const BI_RGB: u32 = 0;
    const BI_BITFIELDS: u32 = 3;
    if width <= 0 || height == 0 || width > 1 << 15 || height.unsigned_abs() > 1 << 15 {
        return Err(format!("a bitmap of {width}x{height}"));
    }
    if width as u64 * u64::from(height.unsigned_abs()) > MAX_PIXELS {
        return Err(format!("a bitmap of {width}x{height}, over {MAX_PIXELS} pixels"));
    }
    let (w, h) = (width as usize, height.unsigned_abs() as usize);
    // Bottom-up unless the height is negative.
    let top_down = height < 0;
    let (masks, mut offset) = match (bits, compression) {
        (24, BI_RGB) => (None, header),
        (32, BI_RGB) => (Some([0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0xFF00_0000]), header),
        (32, BI_BITFIELDS) => {
            // The masks follow a 40-byte header; a V4 or V5 one holds them,
            // with the alpha mask after the three.
            let at = 40;
            let rgb = [u32_at(at).ok_or_else(short)?, u32_at(at + 4).ok_or_else(short)?, u32_at(at + 8).ok_or_else(short)?];
            let alpha = if header >= 56 { u32_at(at + 12).ok_or_else(short)? } else { 0 };
            (Some([rgb[0], rgb[1], rgb[2], alpha]), if header == 40 { header + 12 } else { header })
        }
        _ => return Err(format!("a {bits}-bit bitmap of compression {compression}")),
    };
    // A palette, which these depths need not have but may.
    offset += colors * 4;
    let stride = (w * bits as usize).div_ceil(32) * 4;
    let rows = bytes.get(offset..offset + stride * h).ok_or("a bitmap shorter than its rows")?;
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        let src = &rows[(if top_down { y } else { h - 1 - y }) * stride..][..stride];
        let dst = &mut out[y * w * 4..(y + 1) * w * 4];
        match masks {
            None => {
                for (d, s) in dst.as_chunks_mut::<4>().0.iter_mut().zip(src.as_chunks::<3>().0) {
                    *d = [s[2], s[1], s[0], 0xFF];
                }
            }
            Some(m) => {
                for (d, s) in dst.as_chunks_mut::<4>().0.iter_mut().zip(src.as_chunks::<4>().0) {
                    let px = u32::from_le_bytes(*s);
                    *d = [channel(px, m[0]), channel(px, m[1]), channel(px, m[2]), channel(px, m[3])];
                }
            }
        }
    }
    if masks.is_some_and(|m| m[3] == 0) || (masks.is_some() && out.as_chunks::<4>().0.iter().all(|p| p[3] == 0)) {
        for p in out.as_chunks_mut::<4>().0 {
            p[3] = 0xFF;
        }
    }
    Ok((w as u32, h as u32, out))
}

/// The channel `mask` picks out of `px`, scaled to 8 bits.
fn channel(px: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let width = (mask >> shift).count_ones();
    let v = (px & mask) >> shift;
    if width >= 8 {
        (v >> (width - 8)) as u8
    } else {
        // Scaled to the full range: 5 bits of 31 make 255.
        let max = (1u32 << width) - 1;
        ((v * 255 + max / 2) / max) as u8
    }
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
    /// does not parse, a bitmap of a kind not read here) gives way to the
    /// next.
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
        // Browsers put the image as a PNG too, its alpha kept.
        // SAFETY: a constant name.
        let png = unsafe { RegisterClipboardFormatW(w!("PNG")) };
        if png != 0 {
            if let Some(image) = bytes(&open, png).and_then(|b| image_from_png(b).ok()) {
                return Ok(Clip::Image(image));
            }
        }
        for format in [CF_DIBV5, CF_DIB] {
            let Some((width, height, rgba)) = bytes(&open, format).and_then(|b| dib_to_rgba(&b).ok()) else { continue };
            if let Ok(png) = encode_png(width, height, &rgba) {
                return Ok(Clip::Image(Image { width, height, png }));
            }
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
    use objc2::runtime::AnyObject;
    use objc2_app_kit::{
        NSBitmapImageFileType, NSBitmapImageRep, NSPasteboard, NSPasteboardTypeFileURL, NSPasteboardTypePNG, NSPasteboardTypeString,
        NSPasteboardTypeTIFF,
    };
    use objc2_foundation::{NSDictionary, NSString, NSURL};

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
            if let Some(items) = pb.pasteboardItems() {
                for i in 0..items.count() {
                    let item = items.objectAtIndex(i);
                    if let Some(url) = item.stringForType(NSPasteboardTypeFileURL) {
                        files.extend(file_path(&url.to_string()));
                    }
                }
            }
            // A path that cannot be typed as a quoted one: not files, then.
            if !files.is_empty() && files.iter().all(|f| f.to_str().is_some_and(typeable_path)) {
                return Ok(Clip::Files(files));
            }
            if let Some(text) = pb.stringForType(NSPasteboardTypeString) {
                let text = text.to_string();
                if !text.is_empty() {
                    return Ok(Clip::Text(text));
                }
            }
            if let Some(data) = pb.dataForType(NSPasteboardTypePNG) {
                return image_from_png(data.to_vec()).map(Clip::Image);
            }
            // A screenshot or an image app's copy is TIFF: made PNG by AppKit.
            if let Some(data) = pb.dataForType(NSPasteboardTypeTIFF) {
                let png = NSBitmapImageRep::imageRepWithData(&data).and_then(|rep| {
                    let props = NSDictionary::<NSString, AnyObject>::new();
                    rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &props)
                });
                if let Some(png) = png {
                    return image_from_png(png.to_vec()).map(Clip::Image);
                }
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

    /// A BITMAPINFOHEADER of `size` bytes for a `w`x`h` DIB.
    fn info_header(size: u32, w: i32, h: i32, bits: u16, compression: u32) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend(le32(size));
        b.extend(w.to_le_bytes());
        b.extend(h.to_le_bytes());
        b.extend(1u16.to_le_bytes());
        b.extend(bits.to_le_bytes());
        b.extend(le32(compression));
        b.extend([0; 20]);
        b.resize(size as usize, 0);
        b
    }

    #[test]
    fn a_24_bit_bottom_up_dib_comes_out_top_first_and_opaque() {
        // 2x2: bottom row blue, red; top row green, white. Rows pad to 8 bytes.
        let mut dib = info_header(40, 2, 2, 24, 0);
        dib.extend([0xFF, 0, 0, 0, 0, 0xFF, 0, 0]);
        dib.extend([0, 0xFF, 0, 0xFF, 0xFF, 0xFF, 0, 0]);
        let (w, h, rgba) = dib_to_rgba(&dib).unwrap();
        assert_eq!((w, h), (2, 2));
        assert_eq!(rgba, [0, 0xFF, 0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0xFF, 0xFF, 0xFF, 0, 0, 0xFF]);
    }

    #[test]
    fn a_32_bit_dib_keeps_its_alpha_unless_it_has_none() {
        // Top-down, one pixel half transparent red.
        let mut dib = info_header(40, 1, -1, 32, 0);
        dib.extend([0, 0, 0xFF, 0x80]);
        assert_eq!(dib_to_rgba(&dib).unwrap().2, [0xFF, 0, 0, 0x80]);
        // Alpha zero everywhere is a DIB that never set it.
        let mut dib = info_header(40, 1, -1, 32, 0);
        dib.extend([0, 0, 0xFF, 0]);
        assert_eq!(dib_to_rgba(&dib).unwrap().2, [0xFF, 0, 0, 0xFF]);
    }

    #[test]
    fn bit_fields_are_read_after_a_short_header_and_inside_a_v5_one() {
        // 40 bytes and three masks after: RGB565-like in 32 bits, here 8-8-8.
        let mut dib = info_header(40, 1, 1, 32, 3);
        dib.extend(le32(0x0000_00FF));
        dib.extend(le32(0x0000_FF00));
        dib.extend(le32(0x00FF_0000));
        dib.extend([0x11, 0x22, 0x33, 0x00]);
        assert_eq!(dib_to_rgba(&dib).unwrap().2, [0x11, 0x22, 0x33, 0xFF]);
        // A V5 header: masks at 40, alpha's among them.
        let mut v5 = info_header(124, 1, 1, 32, 3);
        v5[40..56].copy_from_slice(&[le32(0x00FF_0000), le32(0x0000_FF00), le32(0x0000_00FF), le32(0xFF00_0000)].concat());
        v5.extend([0x33, 0x22, 0x11, 0x40]);
        assert_eq!(dib_to_rgba(&v5).unwrap().2, [0x11, 0x22, 0x33, 0x40]);
    }

    #[test]
    fn a_dib_that_lies_about_its_size_is_refused() {
        let mut dib = info_header(40, 100, 100, 24, 0);
        dib.extend([0; 16]);
        assert!(dib_to_rgba(&dib).is_err());
        assert!(dib_to_rgba(&info_header(40, 1, 1, 8, 0)).is_err());
        assert!(dib_to_rgba(&[1, 2, 3]).is_err());
        assert!(dib_to_rgba(&info_header(40, 0, 1, 24, 0)).is_err());
    }

    #[test]
    fn narrow_channels_widen_to_full_range() {
        assert_eq!(channel(0x1F, 0x1F), 0xFF);
        assert_eq!(channel(0, 0x1F), 0);
        assert_eq!(channel(0x7C00, 0x7C00), 0xFF);
        assert_eq!(channel(0xFFFF, 0), 0);
    }

    #[test]
    fn an_image_round_trips_through_png_with_its_size() {
        let png = encode_png(2, 1, &[1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
        let image = image_from_png(png.clone()).unwrap();
        assert_eq!((image.width, image.height), (2, 1));
        let back = crate::desktop::tray::decode_png(&png).unwrap();
        assert_eq!(back.pixels, [1, 2, 3, 4, 5, 6, 7, 8]);
        assert!(image_from_png(b"GIF89a and more bytes than that".to_vec()).is_err());
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
            assert!(!typeable_path(bad), "{bad:?}");
            assert!(drop_files(&dropfiles(&[r"C:\fine.txt", bad], true), &latin1).is_err(), "{bad:?}");
        }
        assert!(typeable_path(r"C:\Users\a b\c'd (1).png"));
        assert!(!typeable_path(""));
    }

    #[test]
    fn an_image_too_large_is_refused_before_it_is_allocated() {
        // 32768 x 32768: within the old bounds, 4 GiB as RGBA.
        let mut dib = info_header(40, 1 << 15, 1 << 15, 32, 0);
        dib.extend([0; 64]);
        assert!(dib_to_rgba(&dib).unwrap_err().contains("pixels"));
        // At the limit it is the rows that are missing.
        assert!(dib_to_rgba(&info_header(40, 8192, 8192, 32, 0)).unwrap_err().contains("rows"));
        let mut png = encode_png(1, 1, &[0; 4]).unwrap();
        png[16..20].copy_from_slice(&65536u32.to_be_bytes());
        png[20..24].copy_from_slice(&65536u32.to_be_bytes());
        assert!(image_from_png(png).unwrap_err().contains("pixels"));
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
