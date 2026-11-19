//! What a provider's tree is, in one file: `<root>/state.json`. The version
//! sessions start from and the one to go back to, the versions turned away,
//! and the watch over the current version's first sessions all change
//! together, so they are one record, and every change of it is one atomic
//! write (`secret::write_private_synced`: a temp file in the same folder,
//! flushed, renamed over the old) made under `state.lock` by whichever
//! process makes it: the window's update pass, the core's canary and its
//! rollback (ADR-0030). A crash leaves the record as it was or as it was
//! to be, never between; the only change that takes two writes, a canary
//! that trips and the rollback it calls for, is read back from the record
//! by the next core (`cli.rs` `finish_rollbacks`).
//!
//! A reader that only looks (a launch, Settings) reads the file without the
//! lock: the rename that replaces it is atomic where the platform lets it
//! happen (on Windows a scanner holding the file can refuse it: the change
//! then fails, and is made again by the next one). Flushed before the rename,
//! and on Unix the folder after it; Windows cannot flush a folder, so there a
//! power cut may undo a recent change. Version folders are only deleted by
//! the sweep (`cli.rs` `prune_in`), which spares every folder the record
//! names, read afresh once the folder is held, and every one an in-use hold
//! covers.
//!
//! Before this file, Luna 0.19's tree was several: `current`, `previous`,
//! `bad.json`, `canary.json` and the rest. The first change made to a tree
//! without `state.json` reads them in and removes them (`legacy_in`). A
//! record lost or garbled since is rebuilt from the versions on disk whose
//! files still match their sha256 (`cli.rs` `recover_in`); its marks are
//! lost with it.

use super::canary::Canary;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const FILE: &str = "state.json";
const LOCK: &str = "state.lock";

/// A version folder the record names.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Slot {
    /// Its name under `versions/`.
    pub(crate) dir: String,
    pub(crate) version: String,
    /// False only for a folder from before completion records (`cli.rs`
    /// `RECORD`), found so by the migration: it is run as long as its exe is
    /// there, and the update pass fetches its version again beside it.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub(crate) recorded: bool,
}

fn yes() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

/// Who turned a version away.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum By {
    /// The offline probe found it wrong.
    Probe,
    /// Its first sessions failed and Luna rolled back from it.
    Rollback,
    /// A person rolled back from it (`cli.rs` `cli_roll_back`).
    Person,
    /// Marked before this file, by a person (Luna 0.19's "Mark bad") or its
    /// probe of then.
    Earlier,
}

/// One version turned away. It holds while the release pointer names it:
/// the pass drops every mark of another version as it reads the pointer
/// (`choice::plan`'s caller), so a release is tried at most once.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Mark {
    pub(crate) why: String,
    pub(crate) by: By,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct State {
    /// The folder sessions start from; None until the first install.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) current: Option<Slot>,
    /// The folder to go back to from `current`, while there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) previous: Option<Slot>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) bad: BTreeMap<String, Mark>,
    /// The watch over `current`'s first sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) canary: Option<Canary>,
}

impl State {
    /// The versions turned away, each with why.
    pub(crate) fn bad_whys(&self) -> BTreeMap<String, String> {
        self.bad.iter().map(|(v, m)| (v.clone(), m.why.clone())).collect()
    }

    /// The folders the record names: none of them is swept.
    pub(crate) fn named(&self) -> Vec<String> {
        self.current.iter().chain(&self.previous).map(|s| s.dir.clone()).collect()
    }
}

/// The record as it is on disk, for a reader that only looks. A tree with
/// no record yet but 0.19's files is read as they would migrate; one with
/// neither is a tree with no record (`lost_in`). A record that does not parse
/// is an Err, which a launch refuses as damaged until it is rebuilt.
pub(crate) fn read_in(root: &Path) -> Result<State, String> {
    match fs::read(root.join(FILE)) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("its {FILE} does not read: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(legacy_in(root).0),
        Err(e) => Err(format!("its {FILE} will not read: {e}")),
    }
}

/// Whether the tree has no record to go by: `state.json` missing with no
/// 0.19 files to migrate, or there but not parsing. Such a tree is rebuilt
/// from what is on disk (`cli.rs` `recover_in`).
pub(crate) fn lost_in(root: &Path) -> bool {
    match fs::read(root.join(FILE)) {
        Ok(bytes) => serde_json::from_slice::<State>(&bytes).is_err(),
        Err(_) => legacy_in(root).1.is_empty(),
    }
}

/// `state.lock`, held: the one change of the record under way.
pub(crate) struct Held {
    _lock: fs::File,
    root: PathBuf,
}

/// Takes `state.lock`. Held only for a read and a write of the record,
/// never across a download, a probe, a hash or a sweep.
pub(crate) fn lock_in(root: &Path) -> Result<Held, String> {
    fs::create_dir_all(root).map_err(|e| format!("mkdir {}: {e}", root.display()))?;
    let lock = crate::secret::open_private(&root.join(LOCK)).map_err(|e| format!("{LOCK}: {e}"))?;
    lock.lock().map_err(|e| format!("{LOCK}: {e}"))?;
    Ok(Held { _lock: lock, root: root.to_path_buf() })
}

impl Held {
    /// The record, and whether it must be written whatever changes: one
    /// migrated from 0.19's files, where it is not there yet. One that does
    /// not parse reads as empty and is not written back unless a change
    /// makes something of it (the pass's flip): an empty record is never
    /// written over it for nothing (`cli.rs` `recover_in` rebuilds it).
    pub(crate) fn load(&self) -> (State, bool) {
        match fs::read(self.root.join(FILE)) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(state) => (state, false),
                Err(e) => {
                    crate::log::warn("cli", &format!("{}: {e}", self.root.join(FILE).display()));
                    (State::default(), false)
                }
            },
            Err(_) => {
                let (state, there) = legacy_in(&self.root);
                (state, !there.is_empty())
            }
        }
    }

    /// Writes `state` in one atomic step, then removes the files from before
    /// it, now read in.
    pub(crate) fn save(&self, state: &State) -> Result<(), String> {
        super::step("state")?;
        let bytes = serde_json::to_vec_pretty(state).map_err(|e| e.to_string())?;
        crate::secret::write_private_synced(&self.root.join(FILE), &bytes).map_err(|e| format!("write {FILE}: {e}"))?;
        for name in legacy_in(&self.root).1 {
            let path = self.root.join(name);
            if let Err(e) = fs::remove_file(&path) {
                crate::log::info("cli", &format!("leaving {}: {e}", path.display()));
            }
        }
        Ok(())
    }
}

/// One change of the record: read under the lock, changed by `f`, written
/// back in one step where it changed.
pub(crate) fn change<T>(root: &Path, f: impl FnOnce(&mut State) -> T) -> Result<T, String> {
    let held = lock_in(root)?;
    let (mut state, must) = held.load();
    let before = state.clone();
    let out = f(&mut state);
    if state != before || must {
        held.save(&state)?;
    }
    Ok(out)
}

/// The files of Luna 0.19's tree, by what they held:
/// - `current` and `previous`, each a folder's name on one line; `recorded`;
/// - `bad.json`, `{"<version>": "why"}`, marks by hand or by its probe;
/// - `canary.json`, `canary.lock`: a canary of then is not carried over, so
///   nothing rolls back from what a person kept (its `dismissed`), and the
///   version current is taken as through its watch (0.19 never rolled back
///   on its own, so no rollback of then is half made);
/// - `channel`, `pinned`, `good.json`, `marks.lock`, `floor`, `signed-floor`,
///   which nothing reads any more.
const LEGACY: [&str; 12] = [
    "current",
    "previous",
    "recorded",
    "bad.json",
    "canary.json",
    "canary.lock",
    "channel",
    "pinned",
    "good.json",
    "marks.lock",
    "floor",
    "signed-floor",
];

/// What 0.19's files say, and which of them are there (plain files only: a
/// folder at one of the names is none of Luna's).
fn legacy_in(root: &Path) -> (State, Vec<&'static str>) {
    let there: Vec<&'static str> =
        LEGACY.iter().copied().filter(|n| fs::symlink_metadata(root.join(n)).is_ok_and(|m| m.is_file())).collect();
    if there.is_empty() {
        return (State::default(), there);
    }
    let read = |name: &str| fs::read_to_string(root.join(name)).ok().map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    let current = read("current").and_then(|d| super::slot_of(root, &d, !there.contains(&"recorded")));
    let previous = read("previous").and_then(|d| super::slot_of(root, &d, false));
    let previous = previous.filter(|p| current.as_ref().is_none_or(|c| c.dir != p.dir));
    let bad = read("bad.json")
        .and_then(|t| serde_json::from_str::<BTreeMap<String, String>>(&t).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|(v, _)| super::sane_version(v, "bad.json").is_ok())
        .map(|(v, why)| (v, Mark { why, by: By::Earlier }))
        .collect();
    (State { current, previous, bad, canary: None }, there)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("luna-cli-state-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_change_writes_the_record_whole_and_only_when_it_changed() {
        let root = scratch("change");
        assert_eq!(read_in(&root).unwrap(), State::default(), "nothing installed");
        change(&root, |_| ()).unwrap();
        assert!(!root.join(FILE).exists(), "nothing changed, nothing written");
        change(&root, |s| s.bad.insert("1.0.0".into(), Mark { why: "x".into(), by: By::Probe })).unwrap();
        assert_eq!(read_in(&root).unwrap().bad["1.0.0"].by, By::Probe);
        fs::write(root.join(FILE), "{not json").unwrap();
        assert!(read_in(&root).unwrap_err().contains("does not read"), "a launch refuses it");
        assert!(lost_in(&root));
        change(&root, |s| s.bad.clear()).unwrap();
        assert!(read_in(&root).is_err(), "no empty record written over it for nothing");
        change(&root, |s| s.bad.insert("2.0.0".into(), Mark { why: "y".into(), by: By::Probe })).unwrap();
        assert_eq!(read_in(&root).unwrap().bad.len(), 1, "a change that makes something of it writes it");
        assert!(!lost_in(&root));
        let _ = fs::remove_dir_all(&root);
    }
}
