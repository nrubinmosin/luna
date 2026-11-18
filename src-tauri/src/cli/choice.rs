//! What a provider's update pass makes current: the newest version the CLI's
//! release pointer names, unless it is below this Luna's floor (`cli.rs`
//! `floor`), older than the version that runs here already, or one Luna has
//! turned away (ADR-0030).
//!
//! A version is turned away when the offline probe finds it wrong (`probe`:
//! its `--version`, a flag gone, a configuration refused), or when its first
//! sessions fail and Luna rolls back from it (`canary`). The mark is kept in
//! the tree's record (`cli/state.rs`) for as long as the pointer names that
//! version: the pass drops every other as it reads the pointer, so a release
//! is tried once, and taken again only if the pointer moves away and comes
//! back to it.

use std::collections::BTreeMap;

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
}
