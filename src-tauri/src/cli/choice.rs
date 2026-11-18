//! What a provider's update pass makes current: the newest version the CLI's
//! release pointer names, unless it is below this Luna's floor (`cli.rs`
//! `floor`), older than the version that runs here already, or one Luna has
//! turned away (ADR-0030).
//!
//! A version is turned away when the offline probe finds it wrong (`probe`:
//! its `--version`, a flag gone, a configuration refused), or when its first
//! sessions fail and Luna rolls back from it (`canary`). Why is kept at the
//! provider's root, written private like `current` and only under the switch
//! lock (`cli.rs` `switching`), which both the window's update pass and the
//! core's rollback take:
//!
//! ```text
//! bad.json   {"<version>": {"why": …, "by": "probe"|"rollback", "atMs": …}}
//! ```
//!
//! A mark holds until a version at or above it is current (a newer release
//! has been taken by then, and the pass never goes down to an older one), or
//! until it has stood for its time (`By::lasts`): a day for the probe's, a
//! week for a rollback's. There is no clearing a mark by hand, so that is
//! how a mark made in error ends without a newer release: a probe that
//! found fault with a sound version through a fault of Luna's own is tried
//! again, and a version rolled back from for failures that were not its own
//! is taken again, at the cost of two failed starts if they were.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

const BAD: &str = "bad.json";

/// What Luna 0.19 kept at a provider's root for the CLI channels, the "Use"
/// hold, the marks by hand and the floor that rose on this machine, none of
/// which 0.20 has. Removed at the first pass, each only where it is a plain
/// file; a `bad.json` of then is read as this one is, its marks by hand
/// holding like any until a newer version is current.
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

/// Who turned a version away.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum By {
    /// The offline probe found it wrong.
    Probe,
    /// Its first sessions failed and Luna rolled back from it.
    Rollback,
}

impl By {
    /// How long a mark holds, if no newer version is current first.
    fn lasts(self) -> u64 {
        const HOUR: u64 = 3_600_000;
        match self {
            By::Probe => 24 * HOUR,
            By::Rollback => 7 * 24 * HOUR,
        }
    }
}

/// One version turned away.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(untagged)]
pub(crate) enum Mark {
    #[serde(rename_all = "camelCase")]
    Dated { why: String, by: By, at_ms: u64 },
    /// As 0.19 wrote it, by hand or by its probe: why alone, held until a
    /// newer version is current.
    Plain(String),
}

impl Mark {
    pub(crate) fn why(&self) -> &str {
        match self {
            Mark::Dated { why, .. } | Mark::Plain(why) => why,
        }
    }

    fn holds_at(&self, now_ms: u64) -> bool {
        match self {
            Mark::Dated { by, at_ms, .. } => now_ms < at_ms.saturating_add(by.lasts()),
            Mark::Plain(_) => true,
        }
    }
}

/// Every mark in `bad.json`, in force or not. A file that does not parse is
/// taken as none, and said so in the log: it only ever adds refusals.
fn marks_in(root: &Path) -> BTreeMap<String, Mark> {
    let Ok(bytes) = fs::read(root.join(BAD)) else { return BTreeMap::new() };
    serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        crate::log::warn("cli", &format!("{}: {e}; taking it as no version turned away", root.join(BAD).display()));
        BTreeMap::new()
    })
}

/// The versions turned away as of now, each with why.
pub(crate) fn bad_in(root: &Path) -> BTreeMap<String, String> {
    bad_at(root, super::now_ms())
}

fn bad_at(root: &Path, now_ms: u64) -> BTreeMap<String, String> {
    marks_in(root).into_iter().filter(|(_, m)| m.holds_at(now_ms)).map(|(v, m)| (v, m.why().to_string())).collect()
}

fn write_bad(root: &Path, bad: &BTreeMap<String, Mark>) -> Result<(), String> {
    fs::create_dir_all(root).map_err(|e| format!("mkdir {}: {e}", root.display()))?;
    let bytes = serde_json::to_vec_pretty(bad).map_err(|e| e.to_string())?;
    crate::secret::write_private_synced(&root.join(BAD), &bytes).map_err(|e| format!("write {BAD}: {e}"))
}

/// Words for a person to read: one line, kept short.
fn short(why: &str) -> String {
    why.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(300).collect()
}

/// Turns `version` away, for `why`. A mark still in force stays as it is,
/// with its time: a rollback finished after a core went away does not
/// lengthen it.
pub(super) fn mark_bad_in(_held: &super::Switching, root: &Path, version: &str, by: By, why: &str) -> Result<(), String> {
    let version = super::sane_version(version, "a version turned away")?;
    let now_ms = super::now_ms();
    let mut bad = marks_in(root);
    if bad.get(&version).is_some_and(|m| m.holds_at(now_ms)) {
        return Ok(());
    }
    bad.insert(version, Mark::Dated { why: short(why), by, at_ms: now_ms });
    write_bad(root, &bad)
}

/// `version` has become current: what was turned away at or below it goes,
/// as nothing takes it again, and so does every mark past its time.
pub(super) fn forget_up_to_in(_held: &super::Switching, root: &Path, version: &str) -> Result<(), String> {
    let now_ms = super::now_ms();
    let mut bad = marks_in(root);
    let before = bad.len();
    bad.retain(|v, m| super::version_cmp(v, version).is_gt() && m.holds_at(now_ms));
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

/// The pass's decision from the version that runs here (`current`: at or
/// above the floor, recorded or from before records; None where there is
/// none such),
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
        mark_bad_in(&held, &root, "1.5.0", By::Rollback, "  rolled back:\n its first sessions failed  ").unwrap();
        mark_bad_in(&held, &root, "1.7.0", By::Probe, "`exec` no longer lists --json").unwrap();
        assert_eq!(bad_in(&root)["1.5.0"], "rolled back: its first sessions failed");
        assert!(mark_bad_in(&held, &root, "<html>", By::Probe, "x").is_err());
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

    /// With no clearing by hand, a mark made in error ends with its time: a
    /// probe's after a day, a rollback's after a week; one still in force is
    /// not made longer by being made again.
    #[test]
    fn a_mark_holds_for_its_time_and_no_longer() {
        let root = scratch("expiry");
        let held = super::super::switching(&root).unwrap();
        mark_bad_in(&held, &root, "1.5.0", By::Probe, "probe").unwrap();
        mark_bad_in(&held, &root, "1.6.0", By::Rollback, "rollback").unwrap();
        let at = match &marks_in(&root)["1.5.0"] {
            Mark::Dated { at_ms, .. } => *at_ms,
            m => panic!("{m:?}"),
        };
        const HOUR: u64 = 3_600_000;
        assert_eq!(bad_at(&root, at + 23 * HOUR).len(), 2);
        assert_eq!(bad_at(&root, at + 25 * HOUR).keys().collect::<Vec<_>>(), ["1.6.0"], "the probe's is tried again after a day");
        assert!(bad_at(&root, at + 8 * 24 * HOUR).is_empty(), "a rollback's after a week");
        mark_bad_in(&held, &root, "1.5.0", By::Probe, "again").unwrap();
        assert_eq!(marks_in(&root)["1.5.0"], Mark::Dated { why: "probe".into(), by: By::Probe, at_ms: at }, "kept as it was");
        // 0.19's plain marks hold until a newer version is current.
        fs::write(root.join(BAD), r#"{"1.5.0":"marked bad by hand"}"#).unwrap();
        assert_eq!(bad_at(&root, u64::MAX)["1.5.0"], "marked bad by hand");
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
