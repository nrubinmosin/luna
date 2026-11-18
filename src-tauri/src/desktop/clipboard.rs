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

/// An image from PNG bytes, its size read from the header.
pub fn image_from_png(png: Vec<u8>) -> Result<Image, String> {
    // The signature, then IHDR first: length, type, width, height.
    if png.len() < 24 || &png[..8] != b"\x89PNG\r\n\x1a\n" || &png[12..16] != b"IHDR" {
        return Err("not a PNG".into());
    }
    let width = u32::from_be_bytes(png[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(png[20..24].try_into().unwrap());
    Ok(Image { width, height, png })
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

/// CF_HDROP's bytes, a DROPFILES: its header, then at `pFiles` the paths,
/// each ended by a NUL and the list by another, in UTF-16 when `fWide`.
pub fn drop_files(bytes: &[u8]) -> Result<Vec<PathBuf>, String> {
    // pFiles (4), pt (8), fNC (4), fWide (4).
    if bytes.len() < 20 {
        return Err("a file list too short for its header".into());
    }
    let start = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
    let wide = u32::from_le_bytes(bytes[16..20].try_into().unwrap()) != 0;
    let list = bytes.get(start..).ok_or("a file list that starts past its end")?;
    let names: Vec<String> = if wide {
        let units: Vec<u16> = list.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
        units.split(|&u| u == 0).take_while(|s| !s.is_empty()).map(String::from_utf16_lossy).collect()
    } else {
        // The ANSI code page's; a path outside ASCII is lost to it anyway.
        list.split(|&b| b == 0).take_while(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).into_owned()).collect()
    };
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
    use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW,
        SetClipboardData,
    };
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};

    const CF_DIB: u32 = 8;
    const CF_UNICODETEXT: u32 = 13;
    const CF_HDROP: u32 = 15;
    const CF_DIBV5: u32 = 17;

    /// The clipboard, open for as long as this lives. Another program may
    /// hold it for a moment: a few tries before giving up.
    struct Open;

    impl Open {
        fn new() -> Result<Open, String> {
            let mut last = String::new();
            for _ in 0..10 {
                // SAFETY: no owner window; closed again by Drop on this thread.
                match unsafe { OpenClipboard(None) } {
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

    /// A format's bytes, if the clipboard has it. With the clipboard open.
    fn bytes(_open: &Open, format: u32) -> Option<Vec<u8>> {
        // SAFETY: the clipboard is open; the handle it gives is its own and
        // stays valid until it is closed, and is only read here, locked.
        unsafe {
            IsClipboardFormatAvailable(format).ok()?;
            let handle = GetClipboardData(format).ok()?;
            let global = HGLOBAL(handle.0);
            let size = GlobalSize(global);
            let at = GlobalLock(global) as *const u8;
            if at.is_null() {
                return None;
            }
            let copy = std::slice::from_raw_parts(at, size).to_vec();
            let _ = GlobalUnlock(global);
            Some(copy)
        }
    }

    pub fn read() -> Result<Clip, String> {
        let open = Open::new()?;
        if let Some(b) = bytes(&open, CF_HDROP) {
            let files = drop_files(&b)?;
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
            if let Some(b) = bytes(&open, format) {
                let (width, height, rgba) = dib_to_rgba(&b)?;
                return Ok(Clip::Image(Image { width, height, png: encode_png(width, height, &rgba)? }));
            }
        }
        Ok(Clip::Empty)
    }

    pub fn write_text(text: &str) -> Result<(), String> {
        let units: Vec<u16> = text.encode_utf16().chain([0]).collect();
        let _open = Open::new()?;
        // SAFETY: the clipboard is open on this thread. The memory is
        // filled under its lock and, once SetClipboardData takes it, the
        // system's; freed here only when it did not.
        unsafe {
            EmptyClipboard().map_err(|e| e.to_string())?;
            let global = GlobalAlloc(GMEM_MOVEABLE, units.len() * 2).map_err(|e| e.to_string())?;
            let at = GlobalLock(global) as *mut u16;
            if at.is_null() {
                let _ = GlobalFree(Some(global));
                return Err("could not lock the clipboard's memory".into());
            }
            std::ptr::copy_nonoverlapping(units.as_ptr(), at, units.len());
            let _ = GlobalUnlock(global);
            if let Err(e) = SetClipboardData(CF_UNICODETEXT, Some(HANDLE(global.0))) {
                let _ = GlobalFree(Some(global));
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
            if !files.is_empty() {
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

    #[test]
    fn a_file_list_gives_every_path_it_holds() {
        let names = [r"C:\Users\a\Pictures\shot 1.png", r"D:\Bücher\notes.md"];
        let got = drop_files(&dropfiles(&names, true)).unwrap();
        assert_eq!(got, names.iter().map(PathBuf::from).collect::<Vec<_>>());
        assert_eq!(drop_files(&dropfiles(&[r"C:\a.txt"], false)).unwrap(), [PathBuf::from(r"C:\a.txt")]);
        assert!(drop_files(&dropfiles(&[], true)).unwrap().is_empty());
        assert!(drop_files(&[0; 8]).is_err());
        let mut past = dropfiles(&["x"], true);
        past[0..4].copy_from_slice(&le32(4096));
        assert!(drop_files(&past).is_err());
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
