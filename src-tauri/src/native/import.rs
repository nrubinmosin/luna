//! What 0.19's WebView window kept in its Local Storage, read once for the
//! native window's first start (luna_window's import.rs makes its state of
//! it). Chromium keeps an origin's Local Storage in LevelDB, under
//! `<data>/webview/EBWebView/Default/Local Storage/leveldb`. The store is
//! never opened where it is: its files are copied, as they are, into a
//! folder of Luna's own in the data dir, the copy is opened and read, and
//! the copy goes. Opening a LevelDB recovers it and writes, and the store may
//! still be in use by a 0.19 window.
//!
//! Chromium's layout (components/services/storage/dom_storage): a key
//! `_<origin>\0<key>` holds a value of that origin's, where `<key>` and the
//! value each start with a byte saying their encoding, 0 for UTF-16LE and 1
//! for Latin-1. `VERSION` and `META:<origin>` keys are its bookkeeping.

use rusty_leveldb::{LdbIterator, Options, DB};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Where the WebView window's Local Storage is.
fn store() -> PathBuf {
    crate::paths::data_dir().join("webview").join("EBWebView").join("Default").join("Local Storage").join("leveldb")
}

/// The keys 0.19's page kept (`luna.*`), each as its text; None when no
/// WebView window ever kept anything here.
pub(super) fn read_webview_storage() -> Result<Option<HashMap<String, String>>, String> {
    let store = store();
    if !store.is_dir() {
        return Ok(None);
    }
    read_copy(&store, &crate::paths::data_dir().join("import-0.19")).map(Some)
}

/// `store` copied into `scratch`, read there, and the copy removed.
pub(super) fn read_copy(store: &Path, scratch: &Path) -> Result<HashMap<String, String>, String> {
    let _ = fs::remove_dir_all(scratch);
    let read = copy(store, scratch).and_then(|()| read(scratch));
    if let Err(e) = fs::remove_dir_all(scratch) {
        crate::log::warn("window", &format!("the copy of 0.19's store stays at {}: {e}", scratch.display()));
    }
    read
}

/// The store's files into `to`, but its lock: a plain copy of each.
fn copy(store: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|e| format!("cannot make {}: {e}", to.display()))?;
    let entries = fs::read_dir(store).map_err(|e| format!("cannot list the old window's store: {e}"))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("cannot list the old window's store: {e}"))?;
        let name = entry.file_name();
        if name == "LOCK" || !entry.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        fs::copy(entry.path(), to.join(&name))
            .map_err(|e| format!("cannot copy {} of the old window's store: {e}", name.to_string_lossy()))?;
    }
    Ok(())
}

/// The `luna.*` keys of the copy at `dir`, of the origin that has
/// `luna.chats` (Tauri's own, `tauri.localhost`, before any other).
fn read(dir: &Path) -> Result<HashMap<String, String>, String> {
    let options = Options { create_if_missing: false, ..Options::default() };
    let mut db = DB::open(dir, options).map_err(|e| format!("the old window's store does not open: {e}"))?;
    let mut it = db.new_iter().map_err(|e| format!("the old window's store does not read: {e}"))?;
    let mut by_origin: HashMap<String, HashMap<String, String>> = HashMap::new();
    let (mut key, mut value) = (Vec::new(), Vec::new());
    while it.advance() {
        if !it.current(&mut key, &mut value) {
            continue;
        }
        let Some((origin, name)) = storage_key(&key) else { continue };
        if !name.starts_with("luna.") {
            continue;
        }
        if let Some(text) = decode(&value) {
            by_origin.entry(origin).or_default().insert(name, text);
        }
    }
    let mut origins: Vec<String> = by_origin.iter().filter(|(_, keys)| keys.contains_key("luna.chats")).map(|(o, _)| o.clone()).collect();
    origins.sort_by_key(|o| !o.contains("tauri.localhost"));
    match origins.first() {
        Some(origin) => Ok(by_origin.remove(origin).unwrap_or_default()),
        None => Ok(by_origin.into_values().next().unwrap_or_default()),
    }
}

/// A Local Storage key: its origin and its own key.
fn storage_key(raw: &[u8]) -> Option<(String, String)> {
    let rest = raw.strip_prefix(b"_")?;
    let at = rest.iter().position(|b| *b == 0)?;
    let origin = String::from_utf8(rest[..at].to_vec()).ok()?;
    Some((origin, decode(&rest[at + 1..])?))
}

/// A key or a value as Chromium stores it: its encoding's byte, then the
/// text.
fn decode(raw: &[u8]) -> Option<String> {
    match raw.split_first()? {
        (0, utf16) if utf16.len() % 2 == 0 => {
            let units: Vec<u16> = utf16.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
            String::from_utf16(&units).ok()
        }
        (1, latin1) => Some(latin1.iter().map(|b| char::from(*b)).collect()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(s: &str) -> Vec<u8> {
        std::iter::once(0).chain(s.encode_utf16().flat_map(u16::to_le_bytes)).collect()
    }

    fn latin1(s: &str) -> Vec<u8> {
        std::iter::once(1).chain(s.chars().map(|c| c as u8)).collect()
    }

    /// A folder of the test's own under the system's temp.
    fn scratch(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("luna-import-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn key(origin: &str, k: Vec<u8>) -> Vec<u8> {
        [b"_".to_vec(), origin.as_bytes().to_vec(), vec![0], k].concat()
    }

    /// A store in Chromium's layout, written by LevelDB itself: two origins,
    /// keys and values in both encodings, bookkeeping, a key written over
    /// and one deleted.
    fn store(dir: &Path) {
        let mut db = DB::open(dir, Options::default()).unwrap();
        let tauri = "http://tauri.localhost";
        db.put(b"VERSION", b"1").unwrap();
        db.put(format!("META:{tauri}").as_bytes(), b"\x08\x01").unwrap();
        db.put(&key(tauri, latin1("luna.chats")), &utf16(r#"{"state":{"folders":[]},"version":1}"#)).unwrap();
        db.put(&key(tauri, latin1("luna.chats")), &utf16(r#"{"state":{"folders":[{"path":"C:\\wörk"}]},"version":2}"#)).unwrap();
        db.put(&key(tauri, latin1("luna.sidebar")), &latin1("off")).unwrap();
        db.put(&key(tauri, utf16("luna.theme")), &latin1("dark")).unwrap();
        db.put(&key(tauri, latin1("other.key")), &latin1("x")).unwrap();
        db.put(&key(tauri, latin1("luna.gone")), &latin1("x")).unwrap();
        db.delete(&key(tauri, latin1("luna.gone"))).unwrap();
        db.put(&key("http://localhost:1420", latin1("luna.chats")), &latin1("{}")).unwrap();
        db.flush().unwrap();
        db.close().unwrap();
    }

    #[test]
    fn the_copy_is_read_and_goes_and_the_store_stays_as_it_was() {
        let tmp = scratch("copy");
        let (src, scratch) = (tmp.join("leveldb"), tmp.join("import"));
        store(&src);
        fs::write(src.join("LOCK"), b"").unwrap();
        let before: Vec<(String, Vec<u8>)> = {
            let mut files: Vec<_> = fs::read_dir(&src)
                .unwrap()
                .map(|e| e.unwrap())
                .map(|e| (e.file_name().to_string_lossy().into_owned(), fs::read(e.path()).unwrap()))
                .collect();
            files.sort();
            files
        };
        let got = read_copy(&src, &scratch).unwrap();
        assert_eq!(got.get("luna.chats").map(String::as_str), Some(r#"{"state":{"folders":[{"path":"C:\\wörk"}]},"version":2}"#));
        assert_eq!(got.get("luna.sidebar").map(String::as_str), Some("off"));
        assert_eq!(got.get("luna.theme").map(String::as_str), Some("dark"));
        assert!(!got.contains_key("luna.gone"));
        assert!(!got.contains_key("other.key"));
        assert!(!scratch.exists(), "the copy goes");
        let mut after: Vec<_> = fs::read_dir(&src)
            .unwrap()
            .map(|e| e.unwrap())
            .map(|e| (e.file_name().to_string_lossy().into_owned(), fs::read(e.path()).unwrap()))
            .collect();
        after.sort();
        assert_eq!(before, after, "the store is not touched");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn a_store_that_does_not_open_says_why() {
        let tmp = scratch("broken");
        let src = tmp.join("leveldb");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("CURRENT"), b"MANIFEST-000009\n").unwrap();
        let err = read_copy(&src, &tmp.join("import")).unwrap_err();
        assert!(err.contains("does not open"), "{err}");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn keys_and_values_decode_in_both_encodings() {
        assert_eq!(decode(&latin1("abc")).as_deref(), Some("abc"));
        assert_eq!(decode(&utf16("ab€")).as_deref(), Some("ab€"));
        assert_eq!(decode(&[0, 1]), None, "an odd UTF-16");
        assert_eq!(decode(&[7, b'x']), None);
        assert_eq!(storage_key(&key("o", latin1("k"))), Some(("o".into(), "k".into())));
        assert_eq!(storage_key(b"META:o"), None);
    }
}
