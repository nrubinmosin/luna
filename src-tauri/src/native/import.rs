//! What 0.19's WebView window kept in its Local Storage, read for the native
//! window's import (luna_window's import.rs makes its state of it). Chromium
//! keeps an origin's Local Storage in LevelDB, under
//! `<profile>/EBWebView/Default/Local Storage/leveldb`; the profile was in
//! more than one place over 0.x, and the newest that holds a chat list is
//! read (`stores`, `read_newest`). The store is never opened where it is: its files are
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

/// The folder of a data dir the WebView window kept its profile in from
/// 0.6.32 on (paths.rs `webview_dir`).
#[cfg_attr(not(windows), allow(dead_code))]
const PROFILE: &str = "webview";

/// The identifiers Tauri kept the WebView's profile under, in
/// `LOCALAPPDATA\<identifier>` (its `app_local_data_dir`), before 0.6.32
/// moved it into the data dir, newest first: `rm.luna` in 0.6.31,
/// `ai.sprouter.luna` up to 0.6.30 (tauri.conf.json's history).
const IDENTIFIERS: &[&str] = &["rm.luna", "ai.sprouter.luna"];
/// The test build's (tauri.test.conf.json's): it never reads the real Luna's.
const TEST_IDENTIFIERS: &[&str] = &["rm.luna.test", "ai.sprouter.luna.test"];

#[cfg_attr(not(windows), allow(dead_code))]
fn identifiers() -> &'static [&'static str] {
    if crate::paths::is_test() { TEST_IDENTIFIERS } else { IDENTIFIERS }
}

/// Every place an earlier 0.x may have left the WebView window's Local
/// Storage, newest first: the data dir's profile (0.6.32 to 0.19); the one
/// in the local data dir `local`, where that is not the data dir (an
/// install whose folder Luna could not write to when the earlier version
/// ran); then the ones under `app_local` (`LOCALAPPDATA`) by `ids`, which
/// paths.rs `adopt_webview_profile` moves only when the WebView window
/// starts, so an install updated past 0.6.31 without starting it, or one
/// from 0.6.30 or before (whose profile no version moved), still has them.
#[cfg_attr(not(windows), allow(dead_code))]
fn stores(data: &Path, local: Option<&Path>, app_local: Option<&Path>, ids: &[&str]) -> Vec<PathBuf> {
    let mut profiles = vec![data.join(PROFILE)];
    profiles.extend(local.filter(|l| !crate::paths::same_dir(l, data)).map(|l| l.join(PROFILE)));
    profiles.extend(app_local.into_iter().flat_map(|base| ids.iter().map(move |id| base.join(id))));
    profiles.into_iter().map(|p| p.join("EBWebView").join("Default").join("Local Storage").join("leveldb")).collect()
}

/// `stores` for this build; none where Luna keeps no WebView profile of its
/// own (off Windows the WebView's default place is in the user's folder,
/// which 0.19 never moved).
fn stores_here(data: &Path) -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        stores(data, crate::paths::local_dir().as_deref(), dirs::data_local_dir().as_deref(), identifiers())
    }
    #[cfg(not(windows))]
    {
        let _ = data;
        Vec::new()
    }
}

/// The keys 0.19's page kept (`luna.*`), each as its text; None when no
/// WebView window ever kept anything here. The copy is made under the data
/// dir `data`.
pub(super) fn read_webview_storage(data: &Path) -> Result<Option<HashMap<String, String>>, String> {
    let read = read_newest(&stores_here(data), &data.join(super::state_file::DIR))?;
    Ok(read.map(|(store, keys)| {
        crate::log::info("window", &format!("the import takes 0.19's store at {}", store.display()));
        keys
    }))
}

/// The first of `stores` that is there and holds a `luna.chats`, read from
/// a copy under `root`, and where it is; with none that holds one, the first
/// that is there. One that does not read stops the search: an older store
/// is not taken over the one last used, and the import says why.
fn read_newest(stores: &[PathBuf], root: &Path) -> Result<Option<(PathBuf, HashMap<String, String>)>, String> {
    let mut first = None;
    for store in stores.iter().filter(|s| s.is_dir()) {
        crate::log::info("window", &format!("reading 0.19's store at {}", store.display()));
        let keys = read_copy(store, root)?;
        if keys.contains_key("luna.chats") {
            return Ok(Some((store.clone(), keys)));
        }
        first.get_or_insert((store.clone(), keys));
    }
    Ok(first)
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
    let lists: Vec<(&str, (usize, usize))> =
        APP_ORIGINS.iter().filter_map(|o| Some((*o, luna_window::chat_list_size(by_origin.get(*o)?.get("luna.chats")?)?))).collect();
    let best = lists.iter().fold(None::<(&str, (usize, usize))>, |best, &(o, size)| match best {
        Some((_, at)) if at >= size => best,
        _ => Some((o, size)),
    });
    if lists.len() > 1 {
        let all: Vec<String> = lists.iter().map(|(_, (chats, folders))| format!("{chats} chats in {folders} folders")).collect();
        let (chats, folders) = best.map_or((0, 0), |(_, size)| size);
        crate::log::warn(
            "window",
            &format!(
                "0.19's store holds {} chat lists ({}); importing the one of {chats} chats in {folders} folders",
                lists.len(),
                all.join(", ")
            ),
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
            if failed < ATTEMPTS {
                Err(format!("cannot list the old window's store: try {failed}"))
            } else {
                Ok(failed)
            }
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

    /// A store holding only `luna.theme`, or also a chat list of `chats`.
    fn kept(store: &Path, chats: Option<usize>) {
        let mut db = DB::open(store, Options::default()).unwrap();
        let tauri = "http://tauri.localhost";
        db.put(&key(tauri, latin1("luna.theme")), &latin1("dark")).unwrap();
        if let Some(chats) = chats {
            db.put(&key(tauri, latin1("luna.chats")), &latin1(&chat_list(chats))).unwrap();
        }
        db.flush().unwrap();
        db.close().unwrap();
    }

    /// A data dir, a local data dir and a `LOCALAPPDATA` of their own under
    /// `tmp`, and the places `stores` finds in them.
    fn places(tmp: &Path) -> Vec<PathBuf> {
        let (data, local, app_local) = (tmp.join("install"), tmp.join("local"), tmp.join("appdata"));
        fs::create_dir_all(&data).unwrap();
        stores(&data, Some(&local), Some(&app_local), IDENTIFIERS)
    }

    #[test]
    fn the_stores_are_looked_for_newest_first() {
        let tmp = scratch("order");
        let leveldb = |p: PathBuf| p.join("EBWebView").join("Default").join("Local Storage").join("leveldb");
        assert_eq!(
            places(&tmp),
            vec![
                leveldb(tmp.join("install").join("webview")),
                leveldb(tmp.join("local").join("webview")),
                leveldb(tmp.join("appdata").join("rm.luna")),
                leveldb(tmp.join("appdata").join("ai.sprouter.luna")),
            ]
        );
        // The local data dir that is the data dir is looked in once; no
        // LOCALAPPDATA, no place there.
        let data = tmp.join("install");
        assert_eq!(stores(&data, Some(&data), None, IDENTIFIERS), vec![leveldb(data.join("webview"))]);
        assert_eq!(stores(&data, None, Some(&tmp), TEST_IDENTIFIERS)[1], leveldb(tmp.join("rm.luna.test")));
        let _ = fs::remove_dir_all(&tmp);
    }

    /// Each place is read where it is the only one there, and is said.
    #[test]
    fn a_store_in_any_of_the_places_is_read() {
        for at in 0..4 {
            let tmp = scratch(&format!("place{at}"));
            let places = places(&tmp);
            kept(&places[at], Some(at + 1));
            let (store, keys) = read_newest(&places, &tmp.join("import")).unwrap().unwrap();
            assert_eq!(store, places[at]);
            assert_eq!(keys.get("luna.chats"), Some(&chat_list(at + 1)));
            let _ = fs::remove_dir_all(&tmp);
        }
        let tmp = scratch("none");
        assert_eq!(read_newest(&places(&tmp), &tmp.join("import")), Ok(None), "no store anywhere");
        let _ = fs::remove_dir_all(&tmp);
    }

    /// Of stores in more than one place, the newest with a chat list; one
    /// without (a theme alone) is passed over, and with none that has one,
    /// the newest there. One that does not read is said, not passed over
    /// for an older one, and every store stays as it was.
    #[test]
    fn the_newest_store_with_a_chat_list_is_taken() {
        let tmp = scratch("newest");
        let places = places(&tmp);
        for (at, place) in places.iter().enumerate() {
            kept(place, Some(at + 1));
        }
        let listed: Vec<_> = places.iter().map(|p| listing(p).unwrap()).collect();
        let (store, keys) = read_newest(&places, &tmp.join("import")).unwrap().unwrap();
        assert_eq!((store, keys.get("luna.chats")), (places[0].clone(), Some(&chat_list(1))));
        assert_eq!(places.iter().map(|p| listing(p).unwrap()).collect::<Vec<_>>(), listed, "no store is touched");

        fs::remove_dir_all(&places[0]).unwrap();
        kept(&places[0], None);
        let (store, _) = read_newest(&places, &tmp.join("import")).unwrap().unwrap();
        assert_eq!(store, places[1], "a theme alone is passed over");

        let without = &places[..1];
        let (store, keys) = read_newest(without, &tmp.join("import")).unwrap().unwrap();
        assert_eq!((store, keys.len()), (places[0].clone(), 1), "with no chat list anywhere, the newest there");

        fs::remove_dir_all(&places[0]).unwrap();
        fs::create_dir_all(&places[0]).unwrap();
        fs::write(places[0].join("CURRENT"), b"MANIFEST-000009\n").unwrap();
        let err = read_newest(&places, &tmp.join("import")).unwrap_err();
        assert!(err.contains("does not open"), "{err}");
        let _ = fs::remove_dir_all(&tmp);
    }

    /// The identifiers looked under are the ones the builds have now, first.
    #[test]
    fn the_identifiers_start_with_the_builds_own() {
        let id = |conf: &str| serde_json::from_str::<serde_json::Value>(conf).unwrap()["identifier"].as_str().unwrap().to_string();
        assert_eq!(id(include_str!("../../tauri.conf.json")), IDENTIFIERS[0]);
        assert_eq!(id(include_str!("../../tauri.test.conf.json")), TEST_IDENTIFIERS[0]);
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
