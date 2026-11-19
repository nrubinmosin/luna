//! What 0.19's WebView window kept in its Local Storage, read for the native
//! window's import (luna_window's import.rs makes its state of it). Chromium
//! keeps an origin's Local Storage in LevelDB, under
//! `<webview>/EBWebView/Default/Local Storage/leveldb` (paths.rs
//! `webview_dir`). The store is never opened where it is: its files are
//! copied, as they are, into a private folder of the window's own
//! (`<data>/window/import-<pid>-<random>`), the copy is opened and read, and
//! the copy goes; one a run before left behind goes at the next read
//! (`clean_up`). Opening a LevelDB recovers it and writes, and the store may
//! still be in use by a 0.19 window, which can write while it is copied: a
//! copy is taken only if the store's files are the same, by name, size and
//! time, before and after it, and one that does not open or read, or whose
//! listing or copy fails (a file the old window holds a moment, one it
//! removes as it compacts), is taken again, `ATTEMPTS` times in all.
//!
//! Chromium's layout (components/services/storage/dom_storage): a key
//! `_<origin>\0<key>` holds a value of that origin's, where `<key>` and the
//! value each start with a byte saying their encoding, 0 for UTF-16LE and 1
//! for Latin-1. `VERSION` and `META:<origin>` keys are its bookkeeping. Only
//! the app's own origins are read (`APP_ORIGINS`), never a dev server's, and
//! of those the one whose `luna.chats` is a list that imports (`pick`).

use rusty_leveldb::{LdbIterator, Options, DB};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The origins 0.19's page was served from: Windows serves the app as
/// `http://tauri.localhost` (`https://` where asked to), macOS and Linux as
/// `tauri://localhost`.
const APP_ORIGINS: &[&str] = &["http://tauri.localhost", "https://tauri.localhost", "tauri://localhost"];

/// How many copies are taken before the store is said not to read.
const ATTEMPTS: usize = 3;

/// Where the WebView window's Local Storage is; None where Luna keeps no
/// WebView profile of its own (off Windows the WebView's default place is
/// in the user's folder, which 0.19 never moved).
fn store() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        Some(crate::paths::webview_dir().join("EBWebView").join("Default").join("Local Storage").join("leveldb"))
    }
    #[cfg(not(windows))]
    None
}

/// The keys 0.19's page kept (`luna.*`), each as its text; None when no
/// WebView window ever kept anything here. The copy is made under the data
/// dir `data`.
pub(super) fn read_webview_storage(data: &Path) -> Result<Option<HashMap<String, String>>, String> {
    let Some(store) = store().filter(|s| s.is_dir()) else { return Ok(None) };
    read_copy(&store, &data.join(super::state_file::DIR)).map(Some)
}

/// The import's copies a run before left behind (one that ended while it
/// read), and the one builds before kept in the data dir itself. Only at
/// the first read of a run (native.rs `window_state`), never beside one.
pub(super) fn clean_up(data: &Path) {
    let mut gone: Vec<PathBuf> = vec![data.join("import-0.19")];
    if let Ok(entries) = fs::read_dir(data.join(super::state_file::DIR)) {
        gone.extend(entries.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("import-")).map(|e| e.path()));
    }
    for dir in gone.into_iter().filter(|d| d.is_dir()) {
        match fs::remove_dir_all(&dir) {
            Ok(()) => crate::log::info("window", &format!("removed the import's copy left at {}", dir.display())),
            Err(e) => crate::log::warn("window", &format!("the import's copy at {} stays: {e}", dir.display())),
        }
    }
}

/// `store` copied into a private folder of its own under `root`, read
/// there, and the copy removed.
pub(super) fn read_copy(store: &Path, root: &Path) -> Result<HashMap<String, String>, String> {
    let scratch = root.join(format!("import-{}-{}", std::process::id(), crate::secret::random_hex(4)));
    let read = crate::secret::private_dir(&scratch)
        .map_err(|e| format!("cannot make {}: {e}", scratch.display()))
        .and_then(|()| read_steady(store, &scratch));
    if let Err(e) = fs::remove_dir_all(&scratch) {
        crate::log::warn("window", &format!("the copy of 0.19's store stays at {}: {e}", scratch.display()));
    }
    read
}

/// A copy of `store` the store did not change under, read: taken again
/// where it changed, where listing or copying it failed, or where it does
/// not open or read. Why the last attempt failed, after `ATTEMPTS`.
fn read_steady(store: &Path, scratch: &Path) -> Result<HashMap<String, String>, String> {
    retried(|| {
        copy_steady(store, scratch)?;
        read(scratch)
    })
}

/// `attempt` until it succeeds, `ATTEMPTS` times at most: why the last
/// failed, where none did.
fn retried<T>(mut attempt: impl FnMut() -> Result<T, String>) -> Result<T, String> {
    let mut why = String::new();
    for _ in 0..ATTEMPTS {
        match attempt() {
            Ok(got) => return Ok(got),
            Err(e) => why = e,
        }
    }
    Err(why)
}

/// One copy of `store` into `scratch`, the same files listed before and
/// after it.
fn copy_steady(store: &Path, scratch: &Path) -> Result<(), String> {
    let before = listing(store)?;
    copy(store, scratch)?;
    if listing(store)? != before {
        return Err("the old window's store changed while it was copied".into());
    }
    Ok(())
}

/// The store's files, but its lock, by name, with their size and time.
fn listing(store: &Path) -> Result<Vec<(String, u64, Option<SystemTime>)>, String> {
    let entries = fs::read_dir(store).map_err(|e| format!("cannot list the old window's store: {e}"))?;
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("cannot list the old window's store: {e}"))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(meta) = entry.metadata() else { continue };
        if name == "LOCK" || !meta.is_file() {
            continue;
        }
        files.push((name, meta.len(), meta.modified().ok()));
    }
    files.sort();
    Ok(files)
}

/// The store's files into `to`, but its lock: a plain copy of each, and
/// nothing of an earlier copy left there.
fn copy(store: &Path, to: &Path) -> Result<(), String> {
    for old in fs::read_dir(to).map_err(|e| format!("cannot list {}: {e}", to.display()))?.flatten() {
        let _ = fs::remove_file(old.path());
    }
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

/// The `luna.*` keys of the copy at `dir`, of the app's own origin
/// (`pick`). None of its keys at all is an empty map: nothing to import.
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
        if !name.starts_with("luna.") || !APP_ORIGINS.contains(&origin.as_str()) {
            continue;
        }
        if let Some(text) = decode(&value) {
            by_origin.entry(origin).or_default().insert(name, text);
        }
    }
    Ok(pick(by_origin))
}

/// The keys of the app's origin the import is to take: the one whose
/// `luna.chats` is a list that imports (luna_window's `chat_list_size`),
/// not merely the first with a `luna.*` key; of two or more, the one with
/// the most chats, then folders, the first of `APP_ORIGINS` on a tie, and
/// the others logged, by their counts alone. With none that imports, the
/// first with a `luna.chats` at all (the import then says why it does not
/// read), else the first with any keys.
fn pick(mut by_origin: HashMap<String, HashMap<String, String>>) -> HashMap<String, String> {
    let lists: Vec<(&str, (usize, usize))> = APP_ORIGINS
        .iter()
        .filter_map(|o| Some((*o, luna_window::chat_list_size(by_origin.get(*o)?.get("luna.chats")?)?)))
        .collect();
    let best = lists.iter().fold(None::<(&str, (usize, usize))>, |best, &(o, size)| match best {
        Some((_, at)) if at >= size => best,
        _ => Some((o, size)),
    });
    if lists.len() > 1 {
        let all: Vec<String> = lists.iter().map(|(_, (chats, folders))| format!("{chats} chats in {folders} folders")).collect();
        let (chats, folders) = best.map_or((0, 0), |(_, size)| size);
        crate::log::warn(
            "window",
            &format!("0.19's store holds {} chat lists ({}); importing the one of {chats} chats in {folders} folders", lists.len(), all.join(", ")),
        );
    }
    let origin = best.map(|(o, _)| o).or_else(|| {
        let with = |key: Option<&str>| {
            APP_ORIGINS.iter().copied().find(|o| by_origin.get(*o).is_some_and(|keys| key.is_none_or(|k| keys.contains_key(k))))
        };
        with(Some("luna.chats")).or_else(|| with(None))
    });
    origin.and_then(|o| by_origin.remove(o)).unwrap_or_default()
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
        // A lone surrogate (text cut inside a pair) is the replacement
        // character, as the page itself read it, not the key lost.
        (0, utf16) if utf16.len() % 2 == 0 => {
            let units: Vec<u16> = utf16.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
            Some(String::from_utf16_lossy(&units))
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
        db.put(&key("http://tauri.localhost.evil", latin1("luna.chats")), &latin1("{}")).unwrap();
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
        assert_eq!(got.len(), 3, "the app's own origin alone: {got:?}");
        assert_eq!(got.get("luna.chats").map(String::as_str), Some(r#"{"state":{"folders":[{"path":"C:\\wörk"}]},"version":2}"#));
        assert_eq!(got.get("luna.sidebar").map(String::as_str), Some("off"));
        assert_eq!(got.get("luna.theme").map(String::as_str), Some("dark"));
        assert!(!got.contains_key("luna.gone"));
        assert!(!got.contains_key("other.key"));
        assert_eq!(fs::read_dir(&scratch).unwrap().count(), 0, "the copy goes");
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

    /// A store of other origins only (a dev server's) has nothing to import.
    #[test]
    fn only_the_apps_own_origin_is_read() {
        let tmp = scratch("origins");
        let src = tmp.join("leveldb");
        let mut db = DB::open(&src, Options::default()).unwrap();
        db.put(&key("http://localhost:1420", latin1("luna.chats")), &latin1("{}")).unwrap();
        db.put(&key("tauri://localhost", latin1("luna.theme")), &latin1("dark")).unwrap();
        db.flush().unwrap();
        db.close().unwrap();
        let got = read_copy(&src, &tmp.join("import")).unwrap();
        assert_eq!(got, HashMap::from([("luna.theme".to_string(), "dark".to_string())]));
        let _ = fs::remove_dir_all(&tmp);
    }

    /// A chat list of `chats` chats in one folder, as 0.19's page kept it.
    fn chat_list(chats: usize) -> String {
        let chats: Vec<serde_json::Value> = (0..chats)
            .map(|i| serde_json::json!({ "id": format!("c{i}"), "name": "x", "provider": "claude", "account": "work" }))
            .collect();
        serde_json::json!({ "state": { "folders": [{ "id": "f1", "path": "/repo", "chats": chats }] }, "version": 2 }).to_string()
    }

    /// The origin taken is the one whose chat list imports, not the first
    /// with any key; of two such, the longer list; with none, the one whose
    /// list is there (the import says why it does not read).
    #[test]
    fn the_origin_taken_is_the_one_with_a_chat_list_that_imports() {
        let keys = |pairs: &[(&str, String)]| pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect::<HashMap<_, _>>();
        let origins = |http: HashMap<String, String>, tauri: HashMap<String, String>| {
            HashMap::from([("http://tauri.localhost".to_string(), http), ("tauri://localhost".to_string(), tauri)])
        };
        // The first origin kept only a theme: the second's list is taken.
        let got = pick(origins(keys(&[("luna.theme", "dark".into())]), keys(&[("luna.chats", chat_list(2))])));
        assert_eq!(got.get("luna.chats"), Some(&chat_list(2)));
        // The first's list does not read: the second's, which does.
        let got = pick(origins(keys(&[("luna.chats", "{oops".into())]), keys(&[("luna.chats", chat_list(1))])));
        assert_eq!(got.get("luna.chats"), Some(&chat_list(1)));
        // Both read: the longer.
        let got = pick(origins(keys(&[("luna.chats", chat_list(1))]), keys(&[("luna.chats", chat_list(3))])));
        assert_eq!(got.get("luna.chats"), Some(&chat_list(3)));
        let got = pick(origins(keys(&[("luna.chats", chat_list(2))]), keys(&[("luna.chats", chat_list(2))])));
        assert_eq!(got.get("luna.chats"), Some(&chat_list(2)), "a tie goes to the first");
        // None reads: the one whose list is there, for the import to say why.
        let got = pick(origins(keys(&[("luna.theme", "dark".into())]), keys(&[("luna.chats", "{oops".into())])));
        assert_eq!(got.get("luna.chats").map(String::as_str), Some("{oops"));
        assert!(pick(HashMap::new()).is_empty());
    }

    /// A copy that fails for a moment (its listing, a file held) is taken
    /// again within the attempts; one that fails every time says why the
    /// last did.
    #[test]
    fn a_copy_that_fails_for_a_moment_is_taken_again() {
        let mut failed = 0;
        let got = retried(|| {
            failed += 1;
            if failed < ATTEMPTS { Err(format!("cannot list the old window's store: try {failed}")) } else { Ok(failed) }
        });
        assert_eq!(got, Ok(ATTEMPTS));
        let mut tries = 0;
        let got: Result<(), String> = retried(|| {
            tries += 1;
            Err(format!("try {tries}"))
        });
        assert_eq!((got, tries), (Err(format!("try {ATTEMPTS}")), ATTEMPTS));
        let tmp = scratch("listing");
        let err = read_copy(&tmp.join("no-store"), &tmp.join("import")).unwrap_err();
        assert!(err.contains("cannot list"), "{err}");
        let _ = fs::remove_dir_all(&tmp);
    }

    /// The copies a run before left behind go, and nothing else there.
    #[test]
    fn copies_left_behind_are_cleaned_up() {
        let data = scratch("clean");
        let dir = data.join(crate::native::state_file::DIR);
        for left in [dir.join("import-1-ab"), data.join("import-0.19")] {
            fs::create_dir_all(&left).unwrap();
            fs::write(left.join("000005.ldb"), b"x").unwrap();
        }
        fs::write(dir.join("chats.json"), b"{}").unwrap();
        clean_up(&data);
        assert!(!dir.join("import-1-ab").exists() && !data.join("import-0.19").exists());
        assert!(dir.join("chats.json").is_file());
        let _ = fs::remove_dir_all(&data);
    }

    /// A store that changes while it is copied is copied again; one that
    /// changes on every copy is said to.
    #[test]
    fn a_store_written_while_it_is_copied_is_copied_again() {
        let tmp = scratch("steady");
        let src = tmp.join("leveldb");
        store(&src);
        assert!(listing(&src).unwrap().iter().all(|(n, ..)| n != "LOCK"));
        assert_eq!(listing(&src).unwrap(), listing(&src).unwrap(), "a store at rest lists the same");
        let before = listing(&src).unwrap();
        fs::write(src.join("000099.log"), b"more").unwrap();
        assert_ne!(listing(&src).unwrap(), before, "a file more is a change");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn keys_and_values_decode_in_both_encodings() {
        assert_eq!(decode(&latin1("abc")).as_deref(), Some("abc"));
        assert_eq!(decode(&utf16("ab€")).as_deref(), Some("ab€"));
        assert_eq!(decode(&[0, 1]), None, "an odd UTF-16");
        assert_eq!(decode(&[0, 0x3d, 0xd8]).as_deref(), Some("\u{fffd}"), "a lone surrogate");
        assert_eq!(decode(&[7, b'x']), None);
        assert_eq!(storage_key(&key("o", latin1("k"))), Some(("o".into(), "k".into())));
        assert_eq!(storage_key(b"META:o"), None);
    }
}
