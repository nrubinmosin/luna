//! What a provider's update pass makes current: the newest version the CLI's
//! release pointer names, unless it is below this Luna's floor (`cli.rs`
//! `floor`), older than the version that runs here already, or one Luna has
//! turned away (ADR-0030).
//!
//! A version is turned away when it fails the offline probe (`probe`), or
//! when its first sessions fail and Luna rolls back from it (`canary`). Why
//! is kept at the provider's root, written private like `current` and only
//! under the switch lock (`cli.rs` `switching`), which both the window's
//! update pass and the core's rollback take:
//!
//! ```text
//! bad.json   {"<version>": "why"}: not taken while the pointer names it
//! ```
//!
//! An entry goes once a version at or above it is current: a newer release
//! has been published and taken by then, and the pass never goes down to an
//! older one.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

const BAD: &str = "bad.json";

/// What Luna 0.19 kept at a provider's root for the CLI channels, the "Use"
/// hold, the marks by hand and the floor that rose on this machine, none of
/// which 0.20 has. Removed at the first pass, each only where it is a plain
/// file; a `bad.json` of then is read as this one is, its marks by hand
/// counting like a rollback's.
const LEFT_BY_0_19: [&str; 6] = ["channel", "good.json", "marks.lock", "pinned", "floor", "signed-floor"];

/// Removes what 0.19 left (`LEFT_BY_0_19`). One that will not go stays,
/// read by nothing.
pub(super) fn forget_0_19(root: &Path) {
    for name in LEFT_BY_0_19 {
        let path = root.join(name);
        if !fs::symlink_metadata(&path).is_ok_and(|m| m.is_file()) {
            continue;
        }
        match fs::remove_file(&path) {
            Ok(()) => crate::log::info("cli", &format!("removed {}, left by Luna 0.19", path.display())),
            Err(e) => crate::log::info("cli", &format!("leaving {}: {e}", path.display())),
        }
    }
}

/// The versions turned away, each with why. A file that does not parse is
/// taken as none, and said so in the log: it only ever adds refusals.
pub(crate) fn bad_in(root: &Path) -> BTreeMap<String, String> {
    let Ok(bytes) = fs::read(root.join(BAD)) else { return BTreeMap::new() };
    serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        crate::log::warn("cli", &format!("{}: {e}; taking it as no version turned away", root.join(BAD).display()));
        BTreeMap::new()
    })
}

fn write_bad(root: &Path, bad: &BTreeMap<String, String>) -> Result<(), String> {
    fs::create_dir_all(root).map_err(|e| format!("mkdir {}: {e}", root.display()))?;
    let bytes = serde_json::to_vec_pretty(bad).map_err(|e| e.to_string())?;
    crate::secret::write_private_synced(&root.join(BAD), &bytes).map_err(|e| format!("write {BAD}: {e}"))
}

/// Words for a person to read: one line, kept short.
fn short(why: &str) -> String {
    why.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(300).collect()
}

/// Turns `version` away, for `why`, until a newer one is current.
pub(super) fn mark_bad_in(_held: &super::Switching, root: &Path, version: &str, why: &str) -> Result<(), String> {
    let version = super::sane_version(version, "a version turned away")?;
    let mut bad = bad_in(root);
    bad.insert(version, short(why));
    write_bad(root, &bad)
}

/// `version` has become current: what was turned away at or below it goes,
/// as nothing takes it again.
pub(super) fn forget_up_to_in(_held: &super::Switching, root: &Path, version: &str) -> Result<(), String> {
    let mut bad = bad_in(root);
    let before = bad.len();
    bad.retain(|v, _| super::version_cmp(v, version).is_gt());
    if bad.len() == before {
        return Ok(());
    }
    write_bad(root, &bad)
}

/// What the update pass does.
#[derive(Debug, PartialEq)]
pub(super) enum Plan {
    /// Make `version` current, fetching it if it is not installed.
    Install(String),
    /// Leave `current` as it is, for the reason given.
    Stay(String),
}

/// The pass's decision from the version that runs here (`current`: a
/// recorded install at or above the floor, None where there is none such),
/// the floor, the versions turned away and what the release pointer said
/// (an error where it could not be read). The pointer is not signed: what it
/// names is held to the floor, and the pass never moves below what runs.
pub(super) fn plan(current: Option<&str>, floor: &str, bad: &BTreeMap<String, String>, latest: Result<&str, &str>) -> Result<Plan, String> {
    let latest = latest.map_err(str::to_string)?;
    let staying = match current {
        Some(c) => format!("staying on {c}"),
        None => "nothing runs until a newer one is out".into(),
    };
    if super::version_cmp(latest, floor).is_lt() {
        return Ok(Plan::Stay(format!("{latest}, the newest release, is older than {floor}, the oldest this Luna runs; {staying}")));
    }
    if let Some(why) = bad.get(latest) {
        return Ok(Plan::Stay(format!("{latest}, the newest release, was turned away ({why}); {staying} until a newer one is out")));
    }
    match current {
        Some(c) if super::version_cmp(latest, c).is_lt() => {
            Ok(Plan::Stay(format!("the release pointer names {latest}, older than {c}, which runs here; staying on {c}")))
        }
        _ => Ok(Plan::Install(latest.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("luna-cli-choice-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn stays(p: Result<Plan, String>, says: &str) {
        match p {
            Ok(Plan::Stay(why)) => assert!(why.contains(says), "{why}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_newest_release_at_or_above_the_floor_is_taken() {
        let none = BTreeMap::new();
        let take = |v: &str| -> Result<Plan, String> { Ok(Plan::Install(v.into())) };
        assert_eq!(plan(None, "1.0.0", &none, Ok("1.5.0")), take("1.5.0"), "a first install");
        assert_eq!(plan(Some("1.2.0"), "1.0.0", &none, Ok("1.5.0")), take("1.5.0"));
        assert_eq!(plan(Some("1.5.0"), "1.0.0", &none, Ok("1.5.0")), take("1.5.0"), "the one that runs: nothing to fetch");
        assert_eq!(plan(None, "1.0.0", &none, Ok("1.0.0")), take("1.0.0"), "the floor itself");
        assert_eq!(plan(Some("1.2.0"), "1.0.0", &none, Err("offline")).unwrap_err(), "offline");
        stays(plan(Some("1.2.0"), "1.0.0", &none, Ok("0.9.0")), "older than 1.0.0, the oldest this Luna runs; staying on 1.2.0");
        stays(plan(None, "1.0.0", &none, Ok("0.9.0")), "nothing runs");
        // The pointer going back below what runs: no step down.
        stays(plan(Some("1.2.0"), "1.0.0", &none, Ok("1.1.0")), "older than 1.2.0, which runs here");
    }

    /// A version rolled back from, or failed by the probe, is not taken while
    /// the pointer names it; a newer one is, and one between too.
    #[test]
    fn a_version_turned_away_is_skipped_until_a_newer_one_is_out() {
        let bad = BTreeMap::from([("1.5.0".to_string(), "rolled back: its first sessions failed".to_string())]);
        stays(plan(Some("1.2.0"), "1.0.0", &bad, Ok("1.5.0")), "1.5.0, the newest release, was turned away (rolled back");
        assert_eq!(plan(Some("1.2.0"), "1.0.0", &bad, Ok("1.6.0")), Ok(Plan::Install("1.6.0".into())));
        assert_eq!(plan(Some("1.2.0"), "1.0.0", &bad, Ok("1.4.0")), Ok(Plan::Install("1.4.0".into())), "the vendor took 1.5.0 back");
    }

    #[test]
    fn what_was_turned_away_goes_once_a_version_at_or_above_it_is_current() {
        let root = scratch("forget");
        let held = super::super::switching(&root).unwrap();
        mark_bad_in(&held, &root, "1.5.0", "  rolled back:\n its first sessions failed  ").unwrap();
        mark_bad_in(&held, &root, "1.7.0", "failed the offline probe").unwrap();
        assert_eq!(bad_in(&root)["1.5.0"], "rolled back: its first sessions failed");
        assert!(mark_bad_in(&held, &root, "<html>", "x").is_err());
        forget_up_to_in(&held, &root, "1.2.0").unwrap();
        assert_eq!(bad_in(&root).len(), 2, "a rollback to an older one keeps both");
        forget_up_to_in(&held, &root, "1.6.0").unwrap();
        assert_eq!(bad_in(&root).keys().collect::<Vec<_>>(), ["1.7.0"]);
        forget_up_to_in(&held, &root, "1.7.0").unwrap();
        assert!(bad_in(&root).is_empty());
        fs::write(root.join(BAD), "{not json").unwrap();
        assert!(bad_in(&root).is_empty(), "a garbled file turns nothing away");
        drop(held);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn what_0_19_left_is_removed_and_its_bad_marks_still_count() {
        let root = scratch("left");
        for name in LEFT_BY_0_19 {
            fs::write(root.join(name), "x").unwrap();
        }
        fs::write(root.join(BAD), r#"{"1.5.0":"marked bad by hand"}"#).unwrap();
        forget_0_19(&root);
        assert!(LEFT_BY_0_19.iter().all(|n| !root.join(n).exists()));
        assert_eq!(bad_in(&root)["1.5.0"], "marked bad by hand");
        // A folder at one of the names is none of 0.19's files: left alone.
        fs::create_dir(root.join("pinned")).unwrap();
        forget_0_19(&root);
        assert!(root.join("pinned").is_dir());
        let _ = fs::remove_dir_all(&root);
    }
}
