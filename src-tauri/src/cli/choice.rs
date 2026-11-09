//! What a provider's update pass is to make current: the channel it follows,
//! the versions marked bad, and a version held by hand. Each is a small file
//! at the provider's root, written private like `current`:
//!
//! ```text
//! channel    `known-good` (also when there is no file) or `latest`
//! bad.json   {"<version>": "why"}: never installed or made current by a pass
//! pinned     the directory "Use" made current: the pass leaves `current` be
//! ```

use super::known_good::Entry;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

const CHANNEL: &str = "channel";
const BAD: &str = "bad.json";
const PINNED: &str = "pinned";

/// Which versions the update pass installs.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub enum Channel {
    /// The newest version on this Luna's shipped list (`known_good`).
    #[default]
    KnownGood,
    /// Whatever the CLI's own release channel names, once it passes the
    /// offline probe.
    Latest,
}

impl Channel {
    fn as_file(self) -> &'static str {
        match self {
            Channel::KnownGood => "known-good",
            Channel::Latest => "latest",
        }
    }
}

fn write(root: &Path, name: &str, bytes: &[u8]) -> Result<(), String> {
    fs::create_dir_all(root).map_err(|e| format!("mkdir {}: {e}", root.display()))?;
    crate::secret::write_private_synced(&root.join(name), bytes).map_err(|e| format!("write {name}: {e}"))
}

pub(super) fn channel_in(root: &Path) -> Channel {
    match fs::read_to_string(root.join(CHANNEL)).map(|t| t.trim().to_string()).as_deref() {
        Ok("latest") => Channel::Latest,
        _ => Channel::KnownGood,
    }
}

/// Follows `channel` from now on, and lets go of a version held by hand.
pub(super) fn set_channel_in(root: &Path, channel: Channel) -> Result<(), String> {
    write(root, CHANNEL, channel.as_file().as_bytes())?;
    unpin_in(root)
}

/// The versions marked bad, each with why. A file that does not parse is
/// taken as none, and said so in the log: it only ever adds refusals.
pub(super) fn bad_in(root: &Path) -> BTreeMap<String, String> {
    let Ok(bytes) = fs::read(root.join(BAD)) else { return BTreeMap::new() };
    serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        crate::log::warn("cli", &format!("{}: {e}; taking no version for bad", root.join(BAD).display()));
        BTreeMap::new()
    })
}

pub(super) fn mark_bad_in(root: &Path, version: &str, why: &str) -> Result<(), String> {
    let version = super::sane_version(version, "a version marked bad")?;
    let mut bad = bad_in(root);
    // A reason is for a person to read in Settings: one line, kept short.
    let why: String = why.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(300).collect();
    bad.insert(version, if why.is_empty() { "marked bad by hand".into() } else { why });
    write(root, BAD, &serde_json::to_vec_pretty(&bad).map_err(|e| e.to_string())?)
}

/// The directory "Use" made current, while that file is there.
pub(super) fn pinned_in(root: &Path) -> Option<String> {
    let name = fs::read_to_string(root.join(PINNED)).ok()?;
    Some(name.trim().to_string()).filter(|n| super::plain_name(n))
}

pub(super) fn pin_in(root: &Path, dir: &str) -> Result<(), String> {
    write(root, PINNED, dir.as_bytes())
}

pub(super) fn unpin_in(root: &Path) -> Result<(), String> {
    match fs::remove_file(root.join(PINNED)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("remove {PINNED}: {e}")),
        _ => Ok(()),
    }
}

/// What the update pass does.
#[derive(Debug, PartialEq)]
pub(super) enum Plan {
    /// Make `version` current, fetching it if it is not installed.
    /// `chosen`: named by this Luna's own list, not the network's pointer.
    Install { version: String, chosen: bool },
    /// Leave `current` as it is, for the reason given.
    Stay(String),
}

/// The pass's decision from the channel, a version held by hand (its
/// version), the shipped list, the bad marks and what the release channel's
/// pointer said (an error where it could not be read). `unsafe_now`: the
/// current version is below this Luna's minimum and runs nothing, so nothing
/// keeps the pass on it — not a hold by hand, not a `latest` that is bad or
/// cannot be read; the shipped list's version is taken instead.
pub(super) fn plan(
    channel: Channel,
    pinned: Option<&str>,
    list: &Entry,
    bad: &BTreeMap<String, String>,
    latest: Result<&str, &str>,
    unsafe_now: bool,
) -> Result<Plan, String> {
    let known_good = || {
        super::known_good::pick(list, &|v| bad.contains_key(v))
            .map(|v| Plan::Install { version: v.to_string(), chosen: true })
            .ok_or_else(|| "every known-good version is marked bad".to_string())
    };
    match (pinned, channel) {
        (Some(version), _) if !unsafe_now => Ok(Plan::Stay(format!("held at {version} by hand; choose a channel to follow it again"))),
        (_, Channel::KnownGood) => known_good(),
        (_, Channel::Latest) => match latest {
            Ok(latest) if !bad.contains_key(latest) => Ok(Plan::Install { version: latest.to_string(), chosen: false }),
            _ if unsafe_now => known_good(),
            Ok(latest) => Ok(Plan::Stay(format!("{latest}, the newest release, is marked bad"))),
            Err(e) => Err(e.to_string()),
        },
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

    fn list() -> Entry {
        Entry { min_safe: "1.0.0".into(), known_good: vec!["1.1.0".into(), "1.2.0".into()] }
    }

    #[test]
    fn the_channel_is_known_good_until_latest_is_chosen() {
        let root = scratch("channel");
        assert_eq!(channel_in(&root), Channel::KnownGood);
        pin_in(&root, "1.1.0").unwrap();
        set_channel_in(&root, Channel::Latest).unwrap();
        assert_eq!(channel_in(&root), Channel::Latest);
        assert_eq!(pinned_in(&root), None, "choosing a channel lets go of the version held by hand");
        fs::write(root.join(CHANNEL), "nightly").unwrap();
        assert_eq!(channel_in(&root), Channel::KnownGood);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn each_channel_picks_its_version_and_skips_the_bad() {
        let none = BTreeMap::new();
        let known = plan(Channel::KnownGood, None, &list(), &none, Err("offline"), false).unwrap();
        assert_eq!(known, Plan::Install { version: "1.2.0".into(), chosen: true }, "the pointer is not needed");
        let latest = plan(Channel::Latest, None, &list(), &none, Ok("1.5.0"), false).unwrap();
        assert_eq!(latest, Plan::Install { version: "1.5.0".into(), chosen: false });
        assert_eq!(plan(Channel::Latest, None, &list(), &none, Err("offline"), false).unwrap_err(), "offline");

        let bad = BTreeMap::from([("1.2.0".to_string(), "x".to_string()), ("1.5.0".to_string(), "y".to_string())]);
        assert_eq!(
            plan(Channel::KnownGood, None, &list(), &bad, Ok("1.5.0"), false).unwrap(),
            Plan::Install { version: "1.1.0".into(), chosen: true }
        );
        assert!(
            matches!(plan(Channel::Latest, None, &list(), &bad, Ok("1.5.0"), false).unwrap(), Plan::Stay(why) if why.contains("marked bad"))
        );
        let all = BTreeMap::from([("1.1.0".to_string(), "x".to_string()), ("1.2.0".to_string(), "x".to_string())]);
        assert!(plan(Channel::KnownGood, None, &list(), &all, Ok("1.5.0"), false).is_err());
        // Held by hand: nothing moves, whatever the channel says.
        assert!(matches!(plan(Channel::Latest, Some("1.1.0"), &list(), &none, Ok("1.5.0"), false).unwrap(), Plan::Stay(_)));
    }

    /// The current version is below the minimum and runs nothing: neither a
    /// hold by hand nor a bad or unreadable `latest` keeps the pass on it.
    #[test]
    fn nothing_keeps_the_pass_on_a_version_below_the_minimum() {
        let none = BTreeMap::new();
        let moved = |p: Result<Plan, String>| p.unwrap();
        assert_eq!(
            moved(plan(Channel::Latest, Some("0.9.0"), &list(), &none, Ok("1.5.0"), true)),
            Plan::Install { version: "1.5.0".into(), chosen: false }
        );
        let bad = BTreeMap::from([("1.5.0".to_string(), "x".to_string())]);
        let known = Plan::Install { version: "1.2.0".into(), chosen: true };
        assert_eq!(moved(plan(Channel::Latest, None, &list(), &bad, Ok("1.5.0"), true)), known);
        assert_eq!(moved(plan(Channel::Latest, None, &list(), &none, Err("offline"), true)), known);
        assert_eq!(moved(plan(Channel::KnownGood, Some("0.9.0"), &list(), &none, Err("offline"), true)), known);
    }

    #[test]
    fn a_bad_mark_keeps_its_reason_short_and_a_garbled_file_marks_nothing() {
        let root = scratch("bad");
        mark_bad_in(&root, "1.2.0", "  hangs\n on start  ").unwrap();
        mark_bad_in(&root, "1.3.0", "").unwrap();
        let bad = bad_in(&root);
        assert_eq!(bad["1.2.0"], "hangs on start");
        assert_eq!(bad["1.3.0"], "marked bad by hand");
        assert!(mark_bad_in(&root, "<html>", "x").is_err());
        fs::write(root.join(BAD), "{not json").unwrap();
        assert!(bad_in(&root).is_empty());
        let _ = fs::remove_dir_all(&root);
    }
}
