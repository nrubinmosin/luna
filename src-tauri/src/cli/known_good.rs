//! The versions a Luna release vouches for, and the oldest it lets run at
//! all: `src-tauri/cli-known-good.json`, compiled in. A version goes on the
//! list once the gate and the live suite have passed on it (the cli-upgrade
//! skill, docs/cli-compatibility.md); `minSafe` rises when a version below it
//! is found unsafe, and a Luna patch release is how that reaches users. No
//! feed of its own: the list comes with the signed, attested Luna release,
//! and nothing off the network can add to it (ADR-0019).

use crate::provider::Provider;
use serde::Deserialize;
use std::sync::OnceLock;

const SHIPPED: &str = include_str!("../../cli-known-good.json");

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Entry {
    /// Nothing older is installed, run or gone back to, by hand or not.
    pub(crate) min_safe: String,
    /// Checked by the gate and the live suite before a release took them.
    pub(crate) known_good: Vec<String>,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct Shipped {
    claude: Entry,
    codex: Entry,
}

fn parse(text: &str) -> Result<Shipped, String> {
    let shipped: Shipped = serde_json::from_str(text).map_err(|e| format!("cli-known-good.json: {e}"))?;
    for (name, e) in [("claude", &shipped.claude), ("codex", &shipped.codex)] {
        super::sane_version(&e.min_safe, "cli-known-good.json")?;
        for v in &e.known_good {
            super::sane_version(v, "cli-known-good.json")?;
            if super::version_cmp(v, &e.min_safe).is_lt() {
                return Err(format!("cli-known-good.json: {name} {v} is below its minSafe {}", e.min_safe));
            }
        }
    }
    Ok(shipped)
}

fn shipped() -> &'static Shipped {
    static S: OnceLock<Shipped> = OnceLock::new();
    // A test holds the shipped file to `parse`; a release cannot carry one it
    // refuses.
    S.get_or_init(|| parse(SHIPPED).expect("the shipped cli-known-good.json"))
}

pub(crate) fn entry(provider: Provider) -> &'static Entry {
    match provider {
        Provider::Claude => &shipped().claude,
        Provider::Codex => &shipped().codex,
    }
}

pub(crate) fn min_safe(provider: Provider) -> &'static str {
    &entry(provider).min_safe
}

/// What the known-good channel installs: the newest version that the list
/// or `good` names (those marked good on this machine, held to the minimum
/// by `choice::good_in`) and that is not marked bad.
pub(crate) fn pick<'a>(list: &'a Entry, good: impl Iterator<Item = &'a String>, bad: &dyn Fn(&str) -> bool) -> Option<&'a str> {
    list.known_good.iter().chain(good).filter(|v| !bad(v)).max_by(|a, b| super::version_cmp(a, b)).map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_list_parses_and_holds_to_its_minimums() {
        let shipped = parse(SHIPPED).unwrap();
        for e in [&shipped.claude, &shipped.codex] {
            assert!(!e.known_good.is_empty(), "the known-good channel needs a version to install");
        }
        assert!(parse(r#"{"claude":{"minSafe":"2.0.0","knownGood":["1.9.0"]},"codex":{"minSafe":"0.1.0","knownGood":[]}}"#)
            .unwrap_err()
            .contains("below its minSafe"));
        assert!(parse(r#"{"claude":{"minSafe":"<html>","knownGood":[]},"codex":{"minSafe":"0.1.0","knownGood":[]}}"#).is_err());
        assert!(parse(r#"{"claude":{"minSafe":"1.0.0","knownGood":[],"feed":"x"},"codex":{"minSafe":"0.1.0","knownGood":[]}}"#).is_err());
    }

    /// Minimums a security fix set; a list may raise them, never lower them.
    /// Claude Code 2.1.323: before it, `--permission-mode auto` (which Luna
    /// spawns and inherits) could start in a mode that allows more when auto
    /// mode was unavailable (docs/cli-compatibility.md).
    #[test]
    fn the_shipped_minimums_stay_at_or_above_their_security_fixes() {
        let shipped = parse(SHIPPED).unwrap();
        assert!(super::super::version_cmp(&shipped.claude.min_safe, "2.1.323").is_ge(), "{}", shipped.claude.min_safe);
        assert!(super::super::version_cmp(&shipped.codex.min_safe, "0.167.0").is_ge(), "{}", shipped.codex.min_safe);
    }

    #[test]
    fn the_newest_known_good_version_not_marked_bad_is_picked() {
        let e = Entry { min_safe: "1.0.0".into(), known_good: vec!["1.2.0".into(), "1.10.0".into(), "1.9.0".into()] };
        assert_eq!(pick(&e, std::iter::empty(), &|_| false), Some("1.10.0"));
        assert_eq!(pick(&e, std::iter::empty(), &|v| v == "1.10.0"), Some("1.9.0"));
        assert_eq!(pick(&e, std::iter::empty(), &|_| true), None);
    }
}
