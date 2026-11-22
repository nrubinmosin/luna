//! The native window's chat list on disk: `<data>/window/chats.json`, the
//! only copy of the person's chats, boards and sidebar since 0.19's Local
//! Storage was imported. Everything that can lose it is answered here:
//!
//! - It has a file of its own, which nothing else writes: the window's
//!   place is `<data>/window-state.json` (desktop/window_state.rs), and
//!   neither is ever read as the other.
//! - A file that is there and cannot be read (shared, denied) is no list:
//!   the window is told (`readError`), imports nothing and writes nothing
//!   over it, and asks again for Retry or on the next start.
//! - A file that is no JSON, or of no shape this build knows, is put aside
//!   under a name of its own (`chats.unreadable-<stamp>.json`) before the
//!   window starts empty. One that reads but not all of it (another
//!   version, a chat of a CLI this build does not know; luna_window's
//!   `WindowState::read`) is copied as it was (`chats.kept-<stamp>.json`)
//!   before the window, which reads what it can, writes over it.
//! - It is written synced (`secret::write_private_synced`): a power cut
//!   leaves the old file or the new one whole, never the name on a zeroed
//!   file; the data dir is synced once per run before the first write is
//!   acknowledged, whoever made the window's folder in it (`sync_parent_once`:
//!   on Unix the single instance's lock makes it at start, so the writer
//!   often finds it there and would never have synced its entry).
//!
//! The import of 0.19's list (import.rs) is asked for only where there is no
//! list here, or one whose import failed and was left empty.

use luna_window::state::WindowState;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The window's folder in the data dir (paths.rs `OWN_NAMES`): the chat
/// list, the copies kept of it, the import's scratch copies.
pub(crate) const DIR: &str = "window";
const FILE: &str = "chats.json";
/// The largest chat list read: far past any real one, short of a file that
/// is not one being read whole.
const MAX: u64 = 64 << 20;

/// The chat list's file in the data dir `data`.
fn file(data: &Path) -> PathBuf {
    data.join(DIR).join(FILE)
}

/// Whether `v` is a chat list: an object with folders or boards.
fn is_chat_list(v: &Value) -> bool {
    v.as_object().is_some_and(|o| o.contains_key("folders") || o.contains_key("boards"))
}

/// A file's bytes; None where there is none.
fn read(path: &Path) -> io::Result<Option<Vec<u8>>> {
    use std::io::Read;
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut bytes = Vec::new();
    file.take(MAX + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX {
        return Err(io::Error::other(format!("over {MAX} bytes")));
    }
    Ok(Some(bytes))
}

/// The window's folder, made private where it is not there yet, and then
/// synced into its parent: a file synced into a folder whose own entry is
/// not could still be lost with the folder after a power cut (Unix; see
/// `secret::sync_dir`). A folder that is there is synced into its parent too,
/// once per run (`sync_parent_once`): the single instance made it at start
/// (desktop/single.rs) and synced nothing, and a run that finds it there
/// could not tell that from one that was synced.
fn make_dir(dir: &Path) -> io::Result<()> {
    if !dir.is_dir() {
        crate::secret::private_dir(dir)?;
    }
    sync_parent_once(dir)
}

/// The parents already synced this run.
static SYNCED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Syncs the folder `dir` is in, the first time this run it is asked of that
/// folder and until it succeeds: its entry for `dir` is what a power cut
/// could take with the chat list inside. Windows has no call for a folder
/// (`secret::sync_dir`): NTFS journals the creation of a folder, so it is
/// whole or absent after a crash, and nothing is left to do here.
fn sync_parent_once(dir: &Path) -> io::Result<()> {
    let Some(parent) = dir.parent() else { return Ok(()) };
    let mut synced = SYNCED.lock().unwrap_or_else(|e| e.into_inner());
    if synced.iter().any(|p| p == parent) {
        return Ok(());
    }
    crate::secret::sync_dir(parent)?;
    synced.push(parent.to_owned());
    Ok(())
}

/// A name in `dir` for `what` (`kept`, `unreadable`) no file has yet.
fn fresh_name(dir: &Path, what: &str) -> PathBuf {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    dir.join(format!("chats.{what}-{secs}-{}.json", crate::secret::random_hex(3)))
}

/// `window_state`'s answer (luna_window's `cmd::WINDOW_STATE`) for the data
/// dir `data`; `import` reads 0.19's store, and is called only where the
/// list is to be imported.
pub(super) fn load(data: &Path, import: impl FnOnce() -> Result<Option<HashMap<String, String>>, String>) -> Value {
    let dir = data.join(DIR);
    let file = dir.join(FILE);
    let bytes = match read(&file) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return with_import(&dir, json!({ "saved": null }), import),
        Err(e) => return json!({ "readError": format!("{}: {e}", file.display()) }),
    };
    let value = match serde_json::from_slice::<Value>(&bytes) {
        Ok(v) if is_chat_list(&v) => v,
        read => {
            let why = read.map_or_else(|e| e.to_string(), |_| "it holds no chat list".into());
            let aside = fresh_name(&dir, "unreadable");
            return match std::fs::rename(&file, &aside) {
                Ok(()) => {
                    crate::log::warn("window", &format!("the chat list does not read ({why}), put aside as {}", aside.display()));
                    json!({ "saved": null, "aside": aside.display().to_string() })
                }
                Err(e) => json!({ "readError": format!("{} does not read ({why}) and could not be put aside: {e}", file.display()) }),
            };
        }
    };
    let Some((state, lost)) = WindowState::read(&value) else {
        return json!({ "readError": format!("{} holds no chat list", file.display()) });
    };
    let mut answer = json!({ "saved": value });
    if lost.any() {
        match keep(&dir, &bytes) {
            Ok(kept) => {
                crate::log::warn("window", &format!("the chat list did not all read ({}), kept as {}", lost.words(), kept.display()));
                answer["kept"] = kept.display().to_string().into();
            }
            // Not to be written over unseen: the window waits for a read.
            Err(e) => return json!({ "readError": format!("{} did not all read and could not be copied aside: {e}", file.display()) }),
        }
    }
    if state.import_retryable() {
        answer = with_import(&dir, answer, import);
    } else if luna_window::reads_old_prefs_again(&value) {
        // A list 0.20.0 imported 0.19's into, which kept no theme of it:
        // the store is read once more for the window's choices alone, until
        // the window has written the list again (without `notify`).
        match import() {
            Ok(Some(old)) => answer["oldPrefs"] = json!(old),
            Ok(None) => {}
            Err(why) => crate::log::warn("window", &format!("0.19's Local Storage could not be read again for its choices: {why}")),
        }
    }
    answer
}

/// `answer` with 0.19's list, or why it could not be read, for the window
/// to import. Where the import would leave part of the list out
/// (`luna_window::import_lost`), the list as it was is copied into `dir`
/// first (`oldKept`), as a file of the window's own that did not all read
/// is; failing that, the import is not offered (`oldError`), so nothing is
/// written over what could not be kept.
fn with_import(dir: &Path, mut answer: Value, import: impl FnOnce() -> Result<Option<HashMap<String, String>>, String>) -> Value {
    match import() {
        Ok(Some(old)) => {
            crate::log::info("window", &format!("read 0.19's Local Storage for the import: {} keys", old.len()));
            if let (Some(lost), Some(list)) = (luna_window::import_lost(&old), old.get("luna.chats")) {
                match make_dir(dir).and_then(|()| keep(dir, list.as_bytes())) {
                    Ok(kept) => {
                        crate::log::warn(
                            "window",
                            &format!("0.19's chat list did not all read ({}), kept as {}", lost.words(), kept.display()),
                        );
                        answer["oldKept"] = kept.display().to_string().into();
                    }
                    Err(e) => {
                        let why = format!("0.19's chat list did not all read and could not be copied aside: {e}");
                        crate::log::warn("window", &why);
                        answer["oldError"] = why.into();
                        return answer;
                    }
                }
            }
            answer["old"] = json!(old);
        }
        Ok(None) => {}
        Err(why) => {
            crate::log::warn("window", &format!("0.19's Local Storage could not be read: {why}"));
            answer["oldError"] = why.into();
        }
    }
    answer
}

/// A copy of `bytes` kept in `dir`, where it stays; one kept before with
/// the same bytes is that copy.
fn keep(dir: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
    let same = std::fs::read_dir(dir)?.flatten().map(|e| e.path()).find(|p| {
        p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("chats.kept-") && n.ends_with(".json"))
            && std::fs::read(p).is_ok_and(|b| b == bytes)
    });
    if let Some(path) = same {
        return Ok(path);
    }
    let path = fresh_name(dir, "kept");
    sync_parent_once(dir)?;
    crate::secret::write_private_synced(&path, bytes)?;
    Ok(path)
}

/// Writes the chat list, synced.
pub(super) fn write(data: &Path, text: &str) -> io::Result<()> {
    make_dir(&data.join(DIR))?;
    crate::secret::write_private_synced(&file(data), text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("luna-state-file-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn list(chats: usize) -> String {
        let chats: Vec<Value> = (0..chats)
            .map(|i| json!({ "id": format!("c{i}"), "name": "x", "provider": "claude", "account": "work", "effort": "" }))
            .collect();
        let state = json!({ "version": 1, "folders": [{ "id": "f1", "path": "/repo", "open": true, "chats": chats }], "boards": [] });
        let (state, _) = WindowState::read(&state).unwrap();
        state.to_text()
    }

    fn place() -> String {
        json!({ "bounds": { "x": 10.0, "y": 10.0, "width": 800.0, "height": 600.0 }, "maximized": false }).to_string()
    }

    fn no_import() -> Result<Option<HashMap<String, String>>, String> {
        panic!("nothing to import here")
    }

    fn old() -> Result<Option<HashMap<String, String>>, String> {
        Ok(Some(HashMap::from([("luna.chats".to_string(), "{}".to_string())])))
    }

    fn chats_of(answer: &Value) -> usize {
        WindowState::read(&answer["saved"]).unwrap().0.chats().count()
    }

    /// The place's writer and the chat list's never take each other's file,
    /// in either order: the list written, then the place, reads back as the
    /// list; a place's file alone is no list, and the import runs.
    #[test]
    fn the_chat_list_and_the_place_keep_files_of_their_own() {
        let data = scratch("own");
        write(&data, &list(3)).unwrap();
        let place_file = data.join("window-state.json");
        crate::desktop::window_state::save_to(&place_file, &serde_json::from_str(&place()).unwrap()).unwrap();
        assert_eq!(chats_of(&load(&data, no_import)), 3);
        assert!(crate::desktop::window_state::load_from(&place_file).is_some());
        // A fresh machine with a place kept and no list: imported.
        let data = scratch("place-only");
        std::fs::write(data.join("window-state.json"), place()).unwrap();
        let answer = load(&data, old);
        assert!(answer["saved"].is_null());
        assert!(answer["old"].is_object(), "{answer}");
        let _ = std::fs::remove_dir_all(&data);
    }

    /// The data dir is synced before the first write of a run is
    /// acknowledged, though the window's folder was there already (the
    /// single instance makes it at start on Unix), and once: a parent whose
    /// sync failed is tried again by the next write.
    #[test]
    fn the_data_dir_is_synced_once_before_the_first_write_even_where_the_folder_was_there() {
        let data = scratch("sync");
        std::fs::create_dir_all(data.join(DIR)).unwrap();
        let synced = |data: &Path| SYNCED.lock().unwrap().iter().any(|p| p == data);
        assert!(!synced(&data));
        write(&data, &list(1)).unwrap();
        assert!(synced(&data));
        write(&data, &list(2)).unwrap();
        assert_eq!(SYNCED.lock().unwrap().iter().filter(|p| **p == data).count(), 1);
        // A copy kept before any write syncs it as well.
        let data = scratch("sync-kept");
        std::fs::create_dir_all(data.join(DIR)).unwrap();
        keep(&data.join(DIR), b"{}").unwrap();
        assert!(synced(&data));
        let _ = std::fs::remove_dir_all(&data);
    }

    /// A file that is there and cannot be read is no list: nothing is
    /// imported, and the answer says why.
    #[test]
    fn a_file_that_cannot_be_read_is_not_an_empty_list() {
        let data = scratch("unread");
        // A folder in the file's place: there, and not readable as a file.
        std::fs::create_dir_all(file(&data)).unwrap();
        let answer = load(&data, no_import);
        assert!(answer["readError"].is_string(), "{answer}");
        assert!(answer.get("saved").is_none());
        assert!(file(&data).is_dir(), "left as it was");
        let _ = std::fs::remove_dir_all(&data);
    }

    /// One that is no JSON, or no chat list, is put aside, never deleted;
    /// nothing is imported again.
    #[test]
    fn a_file_that_does_not_read_is_put_aside() {
        let data = scratch("aside");
        for text in ["{not json", r#"{"bounds":{"x":0}}"#] {
            write(&data, text).unwrap();
            let answer = load(&data, no_import);
            let aside = PathBuf::from(answer["aside"].as_str().unwrap());
            assert_eq!(std::fs::read_to_string(&aside).unwrap(), text);
            assert!(answer["saved"].is_null() && answer.get("old").is_none());
            assert!(!file(&data).exists());
        }
        let _ = std::fs::remove_dir_all(&data);
    }

    /// One that reads only in part is copied as it was, once however often
    /// it is read so, and the window gets it to read what it can.
    #[test]
    fn a_file_that_does_not_all_read_is_kept_as_it_was() {
        let data = scratch("kept");
        let mut v: Value = serde_json::from_str(&list(2)).unwrap();
        v["version"] = 9.into();
        v["folders"][0]["chats"][1]["provider"] = "gemini".into();
        let text = v.to_string();
        write(&data, &text).unwrap();
        let answer = load(&data, no_import);
        let kept = PathBuf::from(answer["kept"].as_str().unwrap());
        assert_eq!(std::fs::read_to_string(&kept).unwrap(), text);
        assert_eq!(chats_of(&answer), 1);
        assert_eq!(load(&data, no_import)["kept"], answer["kept"], "the same copy");
        // A list that reads whole keeps nothing.
        write(&data, &list(2)).unwrap();
        assert!(load(&data, no_import).get("kept").is_none());
        let _ = std::fs::remove_dir_all(&data);
    }

    /// A 0.19 list the import would take only in part is copied as it was
    /// before the window gets it; one that reads whole, or is too old to
    /// read, keeps nothing.
    #[test]
    fn a_0_19_list_that_does_not_all_read_is_kept_as_it_was() {
        let data = scratch("old-kept");
        let lossy = json!({ "state": { "folders": [{ "id": "f1", "chats": [] }] }, "version": 2 }).to_string();
        let answer = load(&data, || Ok(Some(HashMap::from([("luna.chats".to_string(), lossy.clone())]))));
        let kept = PathBuf::from(answer["oldKept"].as_str().unwrap());
        assert_eq!(std::fs::read_to_string(&kept).unwrap(), lossy);
        assert_eq!(answer["old"]["luna.chats"], lossy.as_str());
        for whole in [r#"{"state":{"folders":[]},"version":2}"#, r#"{"state":{"folders":[{"id":"f1"}]},"version":0}"#] {
            let answer = load(&data, || Ok(Some(HashMap::from([("luna.chats".to_string(), whole.to_string())]))));
            assert!(answer.get("oldKept").is_none() && answer["old"].is_object(), "{whole}: {answer}");
        }
        let _ = std::fs::remove_dir_all(&data);
    }

    /// An import that failed and left the list empty is asked for again;
    /// one whose list has chats is not.
    #[test]
    fn a_failed_import_left_empty_is_tried_again() {
        let data = scratch("retry");
        let failed =
            json!({ "version": 1, "folders": [], "boards": [], "imported": { "ok": false, "chats": 0, "folders": 0, "why": "x" } });
        write(&data, &failed.to_string()).unwrap();
        assert!(load(&data, old)["old"].is_object());
        let mut filled: Value = serde_json::from_str(&list(1)).unwrap();
        filled["imported"] = failed["imported"].clone();
        write(&data, &filled.to_string()).unwrap();
        assert!(load(&data, no_import).get("old").is_none());
        let _ = std::fs::remove_dir_all(&data);
    }

    /// A list 0.20.0 wrote after importing 0.19's (it still has `notify`)
    /// has the store read again for its choices, as `oldPrefs`, never as a
    /// list to import; one this build wrote, or one that imported nothing,
    /// does not.
    #[test]
    fn a_list_0_20_0_imported_has_0_19s_store_read_again_for_its_choices() {
        let data = scratch("prefs-again");
        let mut by_0_20_0: Value = serde_json::from_str(&list(1)).unwrap();
        by_0_20_0["imported"] = json!({ "ok": true, "chats": 1, "folders": 1 });
        by_0_20_0["notify"] = json!({ "sound": false });
        write(&data, &by_0_20_0.to_string()).unwrap();
        let answer = load(&data, old);
        assert!(answer["oldPrefs"].is_object() && answer.get("old").is_none(), "{answer}");
        let mut by_0_20_1 = by_0_20_0.clone();
        by_0_20_1.as_object_mut().unwrap().remove("notify");
        write(&data, &by_0_20_1.to_string()).unwrap();
        assert!(load(&data, no_import).get("oldPrefs").is_none());
        let mut none = by_0_20_0;
        none.as_object_mut().unwrap().remove("imported");
        write(&data, &none.to_string()).unwrap();
        assert!(load(&data, no_import).get("oldPrefs").is_none());
        let _ = std::fs::remove_dir_all(&data);
    }
}
