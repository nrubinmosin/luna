//! The native window's chat list on disk: `<data>/window/chats.json`, the
//! only copy of the person's chats, boards and sidebar since 0.19's Local
//! Storage was imported. Everything that can lose it is answered here:
//!
//! - It has a file of its own. The window's place (desktop/window_state.rs)
//!   is `<data>/window-state.json`, where builds before this one kept the
//!   chat list too, so a resize could write geometry over the chats. A
//!   file of either is told by its shape (`shape`), never taken for the
//!   other: a chat list found in the place's file is moved here, synced,
//!   before the place's file goes (`rescue`), and the place's writer leaves
//!   alone a file that still holds one.
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
//!   file.
//!
//! The import of 0.19's list (import.rs) is asked for only where there is no
//! list here, or one whose import failed and was left empty.

use luna_window::state::WindowState;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

/// The window's folder in the data dir (paths.rs `OWN_NAMES`): the chat
/// list, the copies kept of it, the import's scratch copies.
pub(crate) const DIR: &str = "window";
const FILE: &str = "chats.json";
/// The window's place (desktop/window_state.rs `path`), in the data dir.
const PLACE: &str = "window-state.json";
/// The largest chat list read: far past any real one, short of a file that
/// is not one being read whole.
const MAX: u64 = 64 << 20;

/// The chat list's file in the data dir `data`.
pub(crate) fn file(data: &Path) -> PathBuf {
    data.join(DIR).join(FILE)
}

/// What a JSON file of the window's holds, by its shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shape {
    /// The chat list: an object with folders or boards.
    Chats,
    /// The window's place: an object with bounds.
    Place,
    Other,
}

pub(crate) fn shape(v: &Value) -> Shape {
    let Some(o) = v.as_object() else { return Shape::Other };
    if o.contains_key("folders") || o.contains_key("boards") {
        Shape::Chats
    } else if o.contains_key("bounds") {
        Shape::Place
    } else {
        Shape::Other
    }
}

/// Whether `bytes` are a chat list (desktop/window_state.rs asks, before it
/// writes the place over a file).
pub(crate) fn holds_chats(bytes: &[u8]) -> bool {
    serde_json::from_slice::<Value>(bytes).is_ok_and(|v| shape(&v) == Shape::Chats)
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

/// The window's folder, made private where it is not there yet.
fn make_dir(dir: &Path) -> io::Result<()> {
    if dir.is_dir() {
        Ok(())
    } else {
        crate::secret::private_dir(dir)
    }
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
        Ok(None) => match read(&data.join(PLACE)) {
            Ok(Some(bytes)) if holds_chats(&bytes) => match rescue(data, &bytes) {
                Ok(()) => bytes,
                Err(e) => return json!({ "readError": format!("the chat list in {PLACE} could not be moved to {}: {e}", file.display()) }),
            },
            Ok(_) => return with_import(json!({ "saved": null }), import),
            // It may hold the list of an earlier build: not to be passed by.
            Err(e) => return json!({ "readError": format!("{} could not be read: {e}", data.join(PLACE).display()) }),
        },
        Err(e) => return json!({ "readError": format!("{}: {e}", file.display()) }),
    };
    let value = match serde_json::from_slice::<Value>(&bytes) {
        Ok(v) if shape(&v) == Shape::Chats => v,
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
        answer = with_import(answer, import);
    }
    answer
}

/// `answer` with 0.19's list, or why it could not be read, for the window
/// to import.
fn with_import(mut answer: Value, import: impl FnOnce() -> Result<Option<HashMap<String, String>>, String>) -> Value {
    match import() {
        Ok(Some(old)) => {
            crate::log::info("window", &format!("read 0.19's Local Storage for the import: {} keys", old.len()));
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

/// The chat list a build before this one kept in the place's file, moved to
/// its own: written there synced, then the place's file goes, whose writer
/// would otherwise not write over it.
fn rescue(data: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = data.join(DIR);
    make_dir(&dir)?;
    crate::secret::write_private_synced(&dir.join(FILE), bytes)?;
    crate::log::warn("window", &format!("moved the chat list from {PLACE} to {DIR}/{FILE}"));
    if let Err(e) = std::fs::remove_file(data.join(PLACE)) {
        crate::log::warn("window", &format!("{PLACE} stays, the chat list in it now a copy: {e}"));
    }
    Ok(())
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
        crate::desktop::window_state::save_to(&data.join(PLACE), &serde_json::from_str(&place()).unwrap()).unwrap();
        assert_eq!(chats_of(&load(&data, no_import)), 3);
        assert!(crate::desktop::window_state::load_from(&data.join(PLACE)).is_some());
        // A fresh machine with a place kept and no list: imported.
        let data = scratch("place-only");
        std::fs::write(data.join(PLACE), place()).unwrap();
        let answer = load(&data, old);
        assert!(answer["saved"].is_null());
        assert!(answer["old"].is_object(), "{answer}");
        let _ = std::fs::remove_dir_all(&data);
    }

    /// A chat list an earlier build kept in the place's file is moved to its
    /// own before the place's writer may write there; until then, that
    /// writer leaves it alone.
    #[test]
    fn a_chat_list_in_the_places_file_is_moved_not_lost() {
        let data = scratch("rescue");
        std::fs::write(data.join(PLACE), list(2)).unwrap();
        // The place first: refused, the list stays.
        let place: crate::desktop::window_state::WindowState = serde_json::from_str(&place()).unwrap();
        assert!(crate::desktop::window_state::save_to(&data.join(PLACE), &place).is_err());
        assert!(holds_chats(&std::fs::read(data.join(PLACE)).unwrap()));
        // Then the list is read, and moved.
        assert_eq!(chats_of(&load(&data, no_import)), 2);
        assert!(file(&data).is_file());
        assert!(!data.join(PLACE).exists());
        crate::desktop::window_state::save_to(&data.join(PLACE), &place).unwrap();
        assert_eq!(chats_of(&load(&data, no_import)), 2, "the place written after reads as no list");
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
}
