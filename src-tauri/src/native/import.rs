//! What 0.19's WebView window kept in its Local Storage, read for the native
//! window's import (luna_window's import.rs makes its state of it). Chromium
//! keeps an origin's Local Storage in LevelDB, under
//! `<profile>/EBWebView/Default/Local Storage/leveldb`. Which profile is read
//! is narrow on purpose (`sources`, `read_old`): the data dir's, where every
//! 0.6.32 to 0.19 kept it; only where that store is surely not there, and
//! only for an installed Luna (`installed`), the ones Tauri kept per user
//! under `LOCALAPPDATA` before. A store reached through a link or junction,
//! or one that cannot be looked at, is an error, never taken for absent
//! (`store_in`). The store is never opened where it is: its files are
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
//! of those the one 0.19 used (`pick`).

use rusty_leveldb::{LdbIterator, Options, DB};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The origins 0.19's page was served from, the one it wrote to first.
/// Tauri 2 serves the app on Windows as `http://tauri.localhost`, which
/// every Luna on Windows used (none set `useHttpsScheme`); as
/// `https://tauri.localhost` where asked to (Tauri 1's default); and on
/// macOS and Linux as `tauri://localhost`.
const APP_ORIGINS: &[&str] = &["http://tauri.localhost", "https://tauri.localhost", "tauri://localhost"];

/// How many copies are taken before the store is said not to read.
const ATTEMPTS: usize = 3;

/// The folder of a data dir the WebView window kept its profile in from
/// 0.6.32 on (paths.rs `webview_dir`).
#[cfg_attr(not(windows), allow(dead_code))]
const PROFILE: &str = "webview";

/// The folders from a profile down to its Local Storage's store.
const STORE: &[&str] = &["EBWebView", "Default", "Local Storage", "leveldb"];

/// The identifiers Tauri kept the WebView's profile under, in
/// `LOCALAPPDATA\<identifier>` (its `app_local_data_dir`), before 0.6.32
/// moved it into the data dir, newest first: `rm.luna` in 0.6.31,
/// `ai.sprouter.luna` up to 0.6.30 (the app's identifier then).
#[cfg_attr(not(windows), allow(dead_code))]
const IDENTIFIERS: &[&str] = &[crate::identity::RELEASE.identifier, "ai.sprouter.luna"];
/// The test build's: it never reads the real Luna's.
#[cfg_attr(not(windows), allow(dead_code))]
const TEST_IDENTIFIERS: &[&str] = &[crate::identity::TEST.identifier, "ai.sprouter.luna.test"];

/// The product the installer installs this build as (identity.rs): its
/// folder in `LOCALAPPDATA` and its key under `HKCU\Software\luna`, where
/// the installer keeps the folder it chose.
#[cfg_attr(not(windows), allow(dead_code))]
const PRODUCT: &str = crate::identity::RELEASE.product;
#[cfg_attr(not(windows), allow(dead_code))]
const TEST_PRODUCT: &str = crate::identity::TEST.product;

/// Where the import looks: the data dir's profile, then, only where its
/// store is surely not there, the `legacy` ones in order.
#[derive(Debug, PartialEq)]
struct Sources {
    current: PathBuf,
    legacy: Vec<PathBuf>,
}

/// The profiles of the data dir `data`, and, for an `installed` Luna, those
/// under `app_local` (`LOCALAPPDATA`) by `ids`. Those are per user, not per
/// install: a copy run from anywhere else (a portable one, the test build)
/// would find the installed Luna's there and import its list. They hold a
/// list only for an install last started on 0.6.31 or before, which never
/// had one in the data dir: the WebView window of 0.6.32 to 0.19 moved them
/// there as it started, and nothing moves them now.
#[cfg_attr(not(windows), allow(dead_code))]
fn sources(data: &Path, installed: bool, app_local: Option<&Path>, ids: &[&str]) -> Sources {
    let legacy = match app_local {
        Some(base) if installed => ids.iter().map(|id| base.join(id)).collect(),
        _ => Vec::new(),
    };
    Sources { current: data.join(PROFILE), legacy }
}

/// `sources` for this build; none where Luna keeps no WebView profile of its
/// own (off Windows the WebView's default place is in the user's folder,
/// which 0.19 never moved).
fn sources_here(data: &Path) -> Option<Sources> {
    #[cfg(windows)]
    {
        let ids = if crate::paths::is_test() { TEST_IDENTIFIERS } else { IDENTIFIERS };
        let app_local = dirs::data_local_dir();
        Some(sources(data, installed(app_local.as_deref()), app_local.as_deref(), ids))
    }
    #[cfg(not(windows))]
    {
        let _ = data;
        None
    }
}

/// Whether this exe is an installed Luna: its folder is the one the
/// installer puts this build in for this user (the folder it keeps under
/// `HKCU\Software\luna\<product>`, else `LOCALAPPDATA\<product>`, as
/// installer.nsi chooses), with the uninstaller beside it.
#[cfg(windows)]
fn installed(app_local: Option<&Path>) -> bool {
    let product = if crate::paths::is_test() { TEST_PRODUCT } else { PRODUCT };
    let Some(exe_dir) = crate::paths::exe_dir() else { return false };
    let Some(install) = registered_install(product).or_else(|| app_local.map(|l| l.join(product))) else { return false };
    is_install(&exe_dir, &install)
}

/// Whether `dir` is `install`, with the installer's uninstaller in it: a
/// copy run from anywhere else is no install, whatever it finds there.
#[cfg_attr(not(windows), allow(dead_code))]
fn is_install(dir: &Path, install: &Path) -> bool {
    crate::paths::same_dir(dir, install) && dir.join("uninstall.exe").is_file()
}

/// The folder the installer last installed `product` in for this user, as
/// it keeps it (installer.nsi's `MANUPRODUCTKEY`, Tauri's template's too).
#[cfg(windows)]
fn registered_install(product: &str) -> Option<PathBuf> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};
    let key = HSTRING::from(format!("Software\\luna\\{product}"));
    let mut bytes = 0u32;
    // SAFETY: `key` outlives the call; with no buffer it only sizes the value.
    unsafe { RegGetValueW(HKEY_CURRENT_USER, &key, PCWSTR::null(), RRF_RT_REG_SZ, None, None, Some(&mut bytes)) }.ok().ok()?;
    let mut text = vec![0u16; (bytes as usize).div_ceil(2)];
    // SAFETY: `text` holds `bytes` bytes, as the call is told.
    unsafe { RegGetValueW(HKEY_CURRENT_USER, &key, PCWSTR::null(), RRF_RT_REG_SZ, None, Some(text.as_mut_ptr().cast()), Some(&mut bytes)) }
        .ok()
        .ok()?;
    text.truncate(bytes as usize / 2);
    let text = String::from_utf16_lossy(&text).trim_end_matches('\0').to_owned();
    (!text.is_empty()).then(|| PathBuf::from(text))
}

/// The keys 0.19's page kept (`luna.*`), each as its text; None when no
/// WebView window ever kept anything here. The copy is made under the data
/// dir `data`.
pub(super) fn read_webview_storage(data: &Path) -> Result<Option<HashMap<String, String>>, String> {
    let Some(sources) = sources_here(data) else { return Ok(None) };
    let read = read_old(&sources, &data.join(super::state_file::DIR))?;
    Ok(read.map(|(store, keys)| {
        crate::log::info("window", &format!("the import takes 0.19's store at {}", store.display()));
        keys
    }))
}

/// The store the import takes, read from a copy under `root`, and where it
/// is: the data dir's where it is there at all, whatever it holds; only
/// where it is surely not there, the first legacy one that holds a
/// `luna.chats`, else the first that is there. A store that cannot be looked
/// at or read is an error, never taken for absent, so an older store is not
/// taken over it; but once a legacy store has been read, an older one's
/// failure is logged and let go of: it cannot be the list last used.
fn read_old(sources: &Sources, root: &Path) -> Result<Option<(PathBuf, HashMap<String, String>)>, String> {
    let read = |profile: &Path| -> Result<Option<(PathBuf, HashMap<String, String>)>, String> {
        let Some(store) = store_in(profile)? else { return Ok(None) };
        crate::log::info("window", &format!("reading 0.19's store at {}", store.display()));
        let keys = read_copy(&store, root)?;
        Ok(Some((store, keys)))
    };
    if let Some(found) = read(&sources.current)? {
        return Ok(Some(found));
    }
    let mut first = None;
    for profile in &sources.legacy {
        match read(profile) {
            Ok(None) => {}
            Ok(Some(found)) if found.1.contains_key("luna.chats") => return Ok(Some(found)),
            Ok(Some(found)) => {
                first.get_or_insert(found);
            }
            Err(e) if first.is_some() => {
                crate::log::warn("window", &format!("an older store of 0.19's at {} is let go of: {e}", profile.display()))
            }
            Err(e) => return Err(e),
        }
    }
    Ok(first)
}

/// The store under the profile `profile`: None where it is surely not there
/// (the profile, or a folder on the way down, is not found). Where any of
/// them cannot be looked at, is no folder, or is a link or junction (which
/// could lead to another profile, another user's), an error: looked at
/// without following one.
fn store_in(profile: &Path) -> Result<Option<PathBuf>, String> {
    let mut at = profile.to_path_buf();
    for part in std::iter::once("").chain(STORE.iter().copied()) {
        if !part.is_empty() {
            at.push(part);
        }
        match fs::symlink_metadata(&at) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("cannot look at {}: {e}", at.display())),
            Ok(meta) if is_link(&meta) => return Err(format!("{} is a link or junction, which the import does not follow", at.display())),
            Ok(meta) if !meta.is_dir() => return Err(format!("{} is not a folder", at.display())),
            Ok(_) => {}
        }
    }
    Ok(Some(at))
}

/// Whether `meta`, of a path not followed, is a link: on Windows any
/// reparse point (a junction, a symbolic link, a mount point).
fn is_link(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
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
    let mut by_origin: ByOrigin = HashMap::new();
    let (mut key, mut value) = (Vec::new(), Vec::new());
    while it.advance() {
        if !it.current(&mut key, &mut value) {
            continue;
        }
        let Some((origin, name)) = storage_key(&key) else { continue };
        if !name.starts_with("luna.") || !APP_ORIGINS.contains(&origin.as_str()) {
            continue;
        }
        by_origin.entry(origin).or_default().insert(name, decode(&value));
    }
    pick(by_origin)
}

/// The `luna.*` values of each origin, None for one that does not decode.
type ByOrigin = HashMap<String, HashMap<String, Option<String>>>;

/// The keys of the app's origin the import is to take: the first of
/// `APP_ORIGINS` that holds a `luna.chats` key, the one 0.19 wrote, whatever
/// another holds (a fuller list there is an older one, not the list last
/// used). The origin is chosen before any value is decoded: where its list
/// does not decode, that is an error for the import to say why, never a
/// reason to take another origin's. With no list, the first origin with any
/// keys (a value of it that does not decode is left out). Where others hold
/// a list of another size, each list's size is logged (luna_window's
/// `chat_list_size`).
fn pick(mut by_origin: ByOrigin) -> Result<HashMap<String, String>, String> {
    let lists: Vec<(&str, Option<(usize, usize)>)> = APP_ORIGINS
        .iter()
        .filter_map(|o| Some((*o, by_origin.get(*o)?.get("luna.chats")?)))
        .map(|(o, text)| (o, text.as_deref().and_then(luna_window::chat_list_size)))
        .collect();
    if lists.iter().any(|(_, size)| *size != lists[0].1) {
        let size =
            |size: &Option<(usize, usize)>| size.map_or("a list that does not import".into(), |(c, f)| format!("{c} chats in {f} folders"));
        let all: Vec<String> = lists.iter().map(|(o, s)| format!("{o}: {}", size(s))).collect();
        crate::log::warn(
            "window",
            &format!("0.19's store holds chat lists of more than one origin ({}); importing {}'s", all.join(", "), lists[0].0),
        );
    }
    let origin = lists.first().map(|(o, _)| *o).or_else(|| APP_ORIGINS.iter().copied().find(|o| by_origin.contains_key(*o)));
    let Some(keys) = origin.and_then(|o| by_origin.remove(o)) else { return Ok(HashMap::new()) };
    if matches!(keys.get("luna.chats"), Some(None)) {
        return Err(format!("0.19's chat list at {} is not stored in an encoding the import reads", origin.unwrap_or_default()));
    }
    Ok(keys.into_iter().filter_map(|(k, v)| Some((k, v?))).collect())
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

    /// The list of the origin 0.19 wrote to (`http://tauri.localhost`) is
    /// taken over a fuller one of another, an older one; where it does not
    /// read, it is still the one taken, for the import to say why. Another
    /// origin's list is taken only where the first kept none.
    #[test]
    fn the_origin_taken_is_the_one_0_19_wrote_to() {
        let keys = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| (k.to_string(), Some(v.to_string()))).collect::<HashMap<_, _>>();
        let origins = |http: HashMap<String, Option<String>>, tauri: HashMap<String, Option<String>>| {
            HashMap::from([("http://tauri.localhost".to_string(), http), ("tauri://localhost".to_string(), tauri)])
        };
        let (one, two, three) = (chat_list(1), chat_list(2), chat_list(3));
        let got = pick(origins(keys(&[("luna.chats", &one)]), keys(&[("luna.chats", &three)]))).unwrap();
        assert_eq!(got.get("luna.chats"), Some(&one), "not the fuller list");
        let got = pick(origins(keys(&[("luna.chats", "{oops")]), keys(&[("luna.chats", &one)]))).unwrap();
        assert_eq!(got.get("luna.chats").map(String::as_str), Some("{oops"), "not passed over where it does not read");
        let got = pick(origins(keys(&[("luna.theme", "dark")]), keys(&[("luna.chats", &two)]))).unwrap();
        assert_eq!(got.get("luna.chats"), Some(&two), "the first kept no list");
        let got = pick(origins(keys(&[("luna.theme", "dark")]), keys(&[("luna.sidebar", "off")]))).unwrap();
        assert_eq!(got.get("luna.theme").map(String::as_str), Some("dark"), "no list anywhere: the first with keys");
        assert!(pick(HashMap::new()).unwrap().is_empty());
        // A list that does not decode is the one 0.19 wrote to all the same.
        let broken = HashMap::from([("luna.chats".to_string(), None), ("luna.theme".to_string(), Some("dark".to_string()))]);
        let err = pick(origins(broken, keys(&[("luna.chats", &one)]))).unwrap_err();
        assert!(err.contains("http://tauri.localhost"), "{err}");
        // With no list there, a value that does not decode is only left out.
        let theme = HashMap::from([("luna.theme".to_string(), Some("dark".to_string())), ("luna.x".to_string(), None)]);
        assert_eq!(pick(origins(theme, HashMap::new())).unwrap().len(), 1);
    }

    /// Through the store itself: a list stored in an encoding the import does
    /// not read (a byte that is neither of Chromium's two) in the origin 0.19
    /// wrote to is an error, not the other origin's list.
    #[test]
    fn a_list_that_does_not_decode_is_not_replaced_by_another_origins() {
        let tmp = scratch("undecodable");
        let src = tmp.join("leveldb");
        let mut db = DB::open(&src, Options::default()).unwrap();
        db.put(&key("http://tauri.localhost", latin1("luna.chats")), &[2, b'{', b'}']).unwrap();
        db.put(&key("tauri://localhost", latin1("luna.chats")), &latin1(&chat_list(1))).unwrap();
        db.flush().unwrap();
        db.close().unwrap();
        let err = read_copy(&src, &tmp.join("import")).unwrap_err();
        assert!(err.contains("http://tauri.localhost") && err.contains("encoding"), "{err}");
        let _ = fs::remove_dir_all(&tmp);
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

    /// The store under the profile `profile`, made with `chats` (as `kept`).
    fn kept_in(profile: &Path, chats: Option<usize>) -> PathBuf {
        let store = STORE.iter().fold(profile.to_path_buf(), |p, part| p.join(part));
        fs::create_dir_all(&store).unwrap();
        kept(&store, chats);
        store
    }

    /// A store at `store` that does not open.
    fn broken(store: &Path) {
        let _ = fs::remove_dir_all(store);
        fs::create_dir_all(store).unwrap();
        fs::write(store.join("CURRENT"), b"MANIFEST-000009\n").unwrap();
    }

    /// The sources of an installed Luna whose data dir and `LOCALAPPDATA`
    /// are under `tmp`.
    fn installed_sources(tmp: &Path) -> Sources {
        sources(&tmp.join("install"), true, Some(&tmp.join("appdata")), IDENTIFIERS)
    }

    /// `at`, a link or junction to the folder `to`.
    fn link(to: &Path, at: &Path) {
        #[cfg(windows)]
        {
            // A junction needs no privilege, as a symbolic link does.
            let made = std::process::Command::new("cmd").arg("/C").arg("mklink").arg("/J").arg(at).arg(to).output().unwrap();
            assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stderr));
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(to, at).unwrap();
    }

    #[test]
    fn the_sources_are_the_data_dirs_profile_then_an_installs_legacy_ones() {
        let (data, base) = (Path::new("data"), Path::new("appdata"));
        assert_eq!(
            sources(data, true, Some(base), IDENTIFIERS),
            Sources { current: data.join("webview"), legacy: vec![base.join("rm.luna"), base.join("ai.sprouter.luna")] }
        );
        assert_eq!(
            sources(data, true, Some(base), TEST_IDENTIFIERS).legacy,
            [base.join("rm.luna.test"), base.join("ai.sprouter.luna.test")]
        );
        // A copy that is not the install looks in its own data dir alone.
        assert_eq!(sources(data, false, Some(base), IDENTIFIERS), Sources { current: data.join("webview"), legacy: Vec::new() });
        assert!(sources(data, true, None, IDENTIFIERS).legacy.is_empty());
    }

    /// The installer's folder with its uninstaller is an install; a copy
    /// elsewhere, or that folder without one, is not.
    #[test]
    fn only_the_install_folder_with_its_uninstaller_is_an_install() {
        let tmp = scratch("install");
        let (install, portable) = (tmp.join("Luna"), tmp.join("portable"));
        fs::create_dir_all(&install).unwrap();
        fs::create_dir_all(&portable).unwrap();
        assert!(!is_install(&install, &install), "no uninstaller");
        fs::write(install.join("uninstall.exe"), b"").unwrap();
        fs::write(portable.join("uninstall.exe"), b"").unwrap();
        assert!(is_install(&install, &install));
        assert!(!is_install(&portable, &install), "a copy elsewhere");
        let _ = fs::remove_dir_all(&tmp);
    }

    /// A portable copy reads no legacy store, though one holds a list.
    #[test]
    fn a_copy_that_is_not_the_install_reads_no_legacy_store() {
        let tmp = scratch("portable");
        let (data, base) = (tmp.join("portable"), tmp.join("appdata"));
        let rm = kept_in(&base.join("rm.luna"), Some(3));
        assert_eq!(read_old(&sources(&data, false, Some(&base), IDENTIFIERS), &tmp.join("import")), Ok(None));
        let (store, keys) = read_old(&sources(&data, true, Some(&base), IDENTIFIERS), &tmp.join("import")).unwrap().unwrap();
        assert_eq!((store, keys.get("luna.chats")), (rm, Some(&chat_list(3))), "the install's reads it");
        let _ = fs::remove_dir_all(&tmp);
    }

    /// The data dir's store is taken wherever it is there, with a list
    /// shorter than a legacy one's or none at all, and no store is touched.
    #[test]
    fn the_data_dirs_store_is_taken_over_any_legacy_one() {
        let tmp = scratch("current");
        let s = installed_sources(&tmp);
        let legacy: Vec<PathBuf> = s.legacy.iter().map(|p| kept_in(p, Some(4))).collect();
        let current = kept_in(&s.current, Some(1));
        let all = [&current, &legacy[0], &legacy[1]];
        let listed: Vec<_> = all.iter().map(|p| listing(p).unwrap()).collect();
        let (store, keys) = read_old(&s, &tmp.join("import")).unwrap().unwrap();
        assert_eq!((store, keys.get("luna.chats")), (current.clone(), Some(&chat_list(1))));
        assert_eq!(all.iter().map(|p| listing(p).unwrap()).collect::<Vec<_>>(), listed, "no store is touched");
        fs::remove_dir_all(&s.current).unwrap();
        kept_in(&s.current, None);
        let (store, keys) = read_old(&s, &tmp.join("import")).unwrap().unwrap();
        assert_eq!((store, keys.get("luna.chats")), (current, None), "a theme alone: still the data dir's");
        let _ = fs::remove_dir_all(&tmp);
    }

    /// With the data dir's store not found, the legacy ones in order: the
    /// first with a list, past one with a theme alone; once one has been
    /// read, an older one that does not is let go of.
    #[test]
    fn with_no_store_in_the_data_dir_the_legacy_ones_are_read_in_order() {
        let tmp = scratch("legacy");
        let s = installed_sources(&tmp);
        // The profile is there, its store is not: not found all the same.
        fs::create_dir_all(s.current.join("EBWebView")).unwrap();
        let rm = kept_in(&s.legacy[0], Some(2));
        let ai = kept_in(&s.legacy[1], Some(5));
        assert_eq!(read_old(&s, &tmp.join("import")).unwrap().unwrap().0, rm);
        fs::remove_dir_all(&s.legacy[0]).unwrap();
        let rm = kept_in(&s.legacy[0], None);
        assert_eq!(read_old(&s, &tmp.join("import")).unwrap().unwrap().0, ai, "a theme alone is passed over");
        broken(&ai);
        let (store, keys) = read_old(&s, &tmp.join("import")).unwrap().unwrap();
        assert_eq!((store, keys.len()), (rm, 1), "the broken older store does not stop the import");
        let none = scratch("legacy-none");
        assert_eq!(read_old(&installed_sources(&none), &none.join("import")), Ok(None), "no store anywhere");
        let _ = fs::remove_dir_all(&tmp);
        let _ = fs::remove_dir_all(&none);
    }

    /// A store that is there and does not read, or a folder on the way to it
    /// that is not one, is an error, never taken for absent: no older store
    /// is read in its place.
    #[test]
    fn a_store_that_cannot_be_read_is_an_error_not_absent() {
        let tmp = scratch("unread");
        let s = installed_sources(&tmp);
        kept_in(&s.legacy[1], Some(5));
        broken(&STORE.iter().fold(s.current.clone(), |p, part| p.join(part)));
        let err = read_old(&s, &tmp.join("import")).unwrap_err();
        assert!(err.contains("does not open"), "{err}");
        fs::remove_dir_all(&s.current).unwrap();
        fs::write(&s.current, b"").unwrap();
        let err = read_old(&s, &tmp.join("import")).unwrap_err();
        assert!(err.contains("is not a folder"), "{err}");
        fs::remove_file(&s.current).unwrap();
        // A newer legacy store that does not read stops the search too.
        broken(&STORE.iter().fold(s.legacy[0].clone(), |p, part| p.join(part)));
        let err = read_old(&s, &tmp.join("import")).unwrap_err();
        assert!(err.contains("does not open"), "{err}");
        let _ = fs::remove_dir_all(&tmp);
    }

    /// A profile that is a link or junction, or one with a link on the way
    /// down to its store, is refused, not followed: it could lead to
    /// another profile, another user's.
    #[test]
    fn a_store_reached_through_a_link_is_refused() {
        for (case, at) in [("profile", 0), ("below", 2), ("legacy", 0)] {
            let tmp = scratch(&format!("link-{case}"));
            let elsewhere = tmp.join("elsewhere");
            kept_in(&elsewhere, Some(3));
            let s = installed_sources(&tmp);
            let profile = if case == "legacy" { &s.legacy[0] } else { &s.current };
            let place = STORE[..at].iter().fold(profile.clone(), |p, part| p.join(part));
            fs::create_dir_all(place.parent().unwrap()).unwrap();
            link(&STORE[..at].iter().fold(elsewhere.clone(), |p, part| p.join(part)), &place);
            let err = read_old(&s, &tmp.join("import")).unwrap_err();
            assert!(err.contains("link or junction"), "{case}: {err}");
            let _ = fs::remove_dir_all(&tmp);
        }
    }

    /// The identifiers looked under are the ones the builds have now, first,
    /// and the install's folder is named for the builds' products.
    #[test]
    fn the_identifiers_and_products_are_the_builds_own() {
        use crate::identity::{RELEASE, TEST};
        assert_eq!((IDENTIFIERS[0], TEST_IDENTIFIERS[0]), (RELEASE.identifier, TEST.identifier));
        assert_eq!((PRODUCT, TEST_PRODUCT), (RELEASE.product, TEST.product));
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
