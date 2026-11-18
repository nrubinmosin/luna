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
//! lock: the rename that replaces it is atomic. Version folders are only
//! deleted by the sweep (`cli.rs` `prune_in`), which spares every folder the
//! record names and every one an in-use hold covers.
//!
//! Before this file, the tree was several (Luna 0.19 and the builds that
//! led to 0.20): `current`, `previous`, `bad.json`, `canary.json` and the
//! rest. The first change made to a tree without `state.json` reads them in
//! and removes them (`migrate`).

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
/// no record yet but files from before it is read as they would migrate; one
/// with neither is a tree nothing has been installed into. A record that
/// does not parse is an Err, which a launch refuses as damaged and the next
/// change replaces.
pub(crate) fn read_in(root: &Path) -> Result<State, String> {
    match fs::read(root.join(FILE)) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("its {FILE} does not read: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(legacy_in(root).0),
        Err(e) => Err(format!("its {FILE} will not read: {e}")),
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
    /// migrated from the files before it, where it is not there yet, or one
    /// that does not parse, which starts over empty (said in the log): the
    /// next pass installs the newest version again, and only marks are lost,
    /// which only ever refused.
    pub(crate) fn load(&self) -> (State, bool) {
        match fs::read(self.root.join(FILE)) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(state) => (state, false),
                Err(e) => {
                    crate::log::warn("cli", &format!("{}: {e}; starting it over", self.root.join(FILE).display()));
                    (State::default(), true)
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

/// The files of the trees before `state.json`, by what they held:
/// - `current`, `previous` (one line, or two: the folder to go back to and the
///   folder the flip that wrote it made current), `recorded`;
/// - `bad.json`, `{"<version>": "why"}` or `{"<version>": {"why", "by", …}}`;
/// - `canary.json`, `canary.lock`, `switch.lock`: a canary of then is not
///   carried over, so nothing rolls back from what a person kept (0.19's
///   `dismissed`), and the version current is taken as through its watch;
/// - Luna 0.19's `channel`, `pinned`, `good.json`, `marks.lock`, `floor`,
///   `signed-floor`, which nothing reads any more.
const LEGACY: [&str; 13] = [
    "current",
    "previous",
    "recorded",
    "bad.json",
    "canary.json",
    "canary.lock",
    "switch.lock",
    "channel",
    "pinned",
    "good.json",
    "marks.lock",
    "floor",
    "signed-floor",
];

/// What the files before `state.json` say, and which of them are there (plain
/// files only: a folder at one of the names is none of Luna's).
fn legacy_in(root: &Path) -> (State, Vec<&'static str>) {
    let there: Vec<&'static str> =
        LEGACY.iter().copied().filter(|n| fs::symlink_metadata(root.join(n)).is_ok_and(|m| m.is_file())).collect();
    if there.is_empty() {
        return (State::default(), there);
    }
    let read = |name: &str| fs::read_to_string(root.join(name)).ok();
    let was_recorded = there.contains(&"recorded");
    let current_dir = read("current").map(|t| t.trim().to_string()).filter(|d| !d.is_empty());
    let current = current_dir.as_deref().and_then(|d| super::slot_of(root, d, !was_recorded));
    let previous = read("previous").and_then(|text| {
        let mut lines = text.lines().map(str::trim);
        let first = lines.next().unwrap_or("").to_string();
        // A two-line `previous` whose second line is not `current`'s folder
        // was left by a flip cut short after `current`: the folder before is
        // that second line.
        let dir = match lines.next().filter(|of| !of.is_empty()) {
            Some(of) if current_dir.as_deref() != Some(of) => of.to_string(),
            _ => first,
        };
        super::slot_of(root, &dir, false)
    });
    let previous = previous.filter(|p| current.as_ref().is_none_or(|c| c.dir != p.dir));
    let bad = read("bad.json")
        .and_then(|t| serde_json::from_str::<BTreeMap<String, serde_json::Value>>(&t).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|(v, _)| super::sane_version(v, "bad.json").is_ok())
        .map(|(v, m)| {
            let why = m.as_str().or(m["why"].as_str()).unwrap_or("turned away").to_string();
            let by = match m["by"].as_str() {
                Some("probe") => By::Probe,
                Some("rollback") => By::Rollback,
                _ => By::Earlier,
            };
            (v, Mark { why, by })
        })
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
        change(&root, |s| s.bad.clear()).unwrap();
        assert_eq!(read_in(&root).unwrap(), State::default(), "the next change starts it over");
        let _ = fs::remove_dir_all(&root);
    }
}
