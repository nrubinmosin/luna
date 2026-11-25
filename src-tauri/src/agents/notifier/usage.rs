//! News of an account's quota at 90% and 95%.

use crate::agents::ask::live;
use crate::agents::registry::{reg, top_chat, Registry};
use crate::throttle::now_ms;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// The percentages that can make news, once each per account window.
const QUOTA_LEVELS: [u8; 2] = [90, 95];

/// A window this far below the level it reached has reset, whatever its
/// reset time says; a point or two is rounding.
const QUOTA_SLACK: f64 = 5.0;

/// The highest level reached in an account's current usage window.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::agents) struct QuotaTold {
    level: u8,
    resets_at: Option<String>,
}

/// The level of `QUOTA_LEVELS` a window at `pct` percent has reached, 0 if none.
fn quota_level(pct: f64) -> u8 {
    QUOTA_LEVELS.iter().copied().filter(|l| pct >= f64::from(*l)).max().unwrap_or(0)
}

/// Whether an instant has passed; one that does not read has not.
fn is_past(at: Option<&str>, now: u64) -> bool {
    at.and_then(|t| t.parse::<jiff::Timestamp>().ok()).is_some_and(|t| (t.as_millisecond() as u64) < now)
}

/// Where a usage window stands against the last level it reached:
/// the level to remember (0: nothing), and whether that is news. Numbers of a
/// window already over are stale, and say nothing until new ones come.
fn quota_step(told: Option<&QuotaTold>, pct: f64, resets_at: Option<&str>, now: u64) -> (u8, bool) {
    if is_past(resets_at, now) {
        return (0, false);
    }
    let reset = told.is_some_and(|t| {
        pct + QUOTA_SLACK < f64::from(t.level) || (t.resets_at.as_deref() != resets_at && is_past(t.resets_at.as_deref(), now))
    });
    let had = if reset { 0 } else { told.map_or(0, |t| t.level) };
    let level = quota_level(pct);
    (level.max(had), level > had)
}

/// An account window as saved beside the notices it produced.
pub(in crate::agents) type QuotaRow = (String, String, u8, Option<String>);

pub(in crate::agents) fn quota_rows(told: &HashMap<(String, String), QuotaTold>) -> Vec<QuotaRow> {
    let mut rows: Vec<_> = told.iter().map(|((a, l), t)| (a.clone(), l.clone(), t.level, t.resets_at.clone())).collect();
    rows.sort();
    rows
}

pub(in crate::agents) fn quota_from(rows: Vec<QuotaRow>, now: u64) -> HashMap<(String, String), QuotaTold> {
    rows.into_iter()
        .filter(|(_, _, _, resets_at)| !is_past(resets_at.as_deref(), now))
        .map(|(account, label, level, resets_at)| ((account, label), QuotaTold { level, resets_at }))
        .collect()
}

/// Keeps the highest surviving level across the old per-chat rows.
pub(in crate::agents) fn legacy_quota_rows(bytes: &[u8]) -> Vec<QuotaRow> {
    type LegacyRow = (String, String, String, u8, Option<String>);
    let rows: Vec<LegacyRow> = serde_json::from_slice(bytes).unwrap_or_default();
    // For each account and label, the highest level is inserted last by
    // quota_from, after it has discarded rows whose windows are over.
    let mut rows: Vec<_> = rows.into_iter().map(|(_, a, l, level, resets_at)| (a, l, level, resets_at)).collect();
    rows.sort();
    rows
}

pub(super) fn quota_news(pty: &crate::pty::PtyManager) {
    let changed = {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        queue_quota_news(&mut r, now_ms(), |id| live(pty, id))
    };
    if changed {
        crate::agents::save_chats();
    }
}

/// Remember crossings even with nobody to tell. Freeze the recipients here
/// so a chat can take the notice when ready and later helpers add no recipients.
fn queue_quota_news(r: &mut Registry, now: u64, is_live: impl Fn(&str) -> bool) -> bool {
    let mut changed = false;
    let mut crossings = BTreeMap::<_, Vec<_>>::new();
    for (account, usage) in &r.usage {
        for window in usage["limits"].as_array().into_iter().flatten() {
            let (Some(label), Some(pct)) = (window["label"].as_str(), window["usedPct"].as_f64()) else { continue };
            let resets_at = window["resetsAt"].as_str();
            let key = (account.clone(), label.to_owned());
            let old = r.quota_told.get(&key);
            let (level, crossed) = quota_step(old, pct, resets_at, now);
            if level == 0 {
                changed |= r.quota_told.remove(&key).is_some();
            } else if old.is_none_or(|old| old.level != level || old.resets_at.as_deref() != resets_at) {
                r.quota_told.insert(key, QuotaTold { level, resets_at: resets_at.map(str::to_owned) });
                changed = true;
            }
            if crossed {
                let resets = resets_at.map(|t| format!(", resets {t}")).unwrap_or_default();
                let line = format!(
                    "Account {account} crossed {level}% of its \"{label}\" window ({pct:.0}% used{resets}); your helpers run on it."
                );
                crossings.entry(account).or_default().push(line);
            }
        }
    }
    if crossings.is_empty() {
        return changed;
    }

    let mut chats_by_account = HashMap::<_, BTreeSet<_>>::new();
    for (id, k) in &r.known {
        if k.parent.is_none() {
            continue;
        }
        let account = format!("{}/{}", k.provider.as_str(), k.account);
        if !crossings.contains_key(&account) {
            continue;
        }
        let Some(chat) = top_chat(r, id).filter(|chat| r.known.contains_key(chat) && is_live(id)) else { continue };
        chats_by_account.entry(account).or_default().insert(chat);
    }

    for (account, lines) in crossings {
        if let Some(chats) = chats_by_account.get(account) {
            r.news.extend(chats.iter().flat_map(|chat| lines.iter().map(move |line| (chat.clone(), line.clone()))));
        }
    }
    changed
}

/// `Your helper "name" (id)`: what a notice calls one to its parent.
pub(in crate::agents) fn your_helper(id: &str, name: Option<&str>) -> String {
    match name {
        Some(n) => format!("Your helper \"{n}\" ({id})"),
        None => format!("Your helper {id}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::persist::SavedChats;
    use crate::agents::registry::Known;
    use crate::provider::Provider;

    #[test]
    fn a_window_is_told_once_per_level_until_it_resets() {
        let now = now_ms();
        let later = crate::background::iso(now + 3_600_000).unwrap();
        let later = Some(later.as_str());
        let told = |level: u8| QuotaTold { level, resets_at: later.map(str::to_owned) };

        assert_eq!(quota_step(None, 89.0, later, now), (0, false));
        assert_eq!(quota_step(None, 90.0, later, now), (90, true));
        assert_eq!(quota_step(None, 93.0, later, now), (90, true), "first look above a level counts");
        assert_eq!(quota_step(Some(&told(90)), 91.0, later, now), (90, false), "once per level");
        assert_eq!(quota_step(Some(&told(90)), 95.0, later, now), (95, true));
        assert_eq!(quota_step(Some(&told(95)), 99.0, later, now), (95, false));
        assert_eq!(quota_step(None, 97.0, later, now), (95, true), "a jump past both is one line");
        assert_eq!(quota_step(Some(&told(95)), 93.0, later, now), (95, false), "a point or two down is rounding");
        assert_eq!(quota_step(Some(&told(90)), 3.0, later, now), (0, false), "reset: forgotten");

        // A new window that is already high, seen only by its reset time.
        let gone = crate::background::iso(now - 60_000).unwrap();
        let old = QuotaTold { level: 95, resets_at: Some(gone.clone()) };
        assert_eq!(quota_step(Some(&old), 96.0, later, now), (95, true), "told again for the new window");
        // Numbers of a window already over say nothing.
        assert_eq!(quota_step(Some(&old), 96.0, Some(&gone), now), (0, false));
        assert_eq!(quota_step(None, 96.0, Some(&gone), now), (0, false));
        // A reset time moving about within a window is not a new one.
        let drift = crate::background::iso(now + 3_700_000).unwrap();
        assert_eq!(quota_step(Some(&told(95)), 96.0, Some(&drift), now), (95, false));
    }

    fn session(provider: Provider, account: &str, parent: Option<&str>) -> Known {
        Known {
            provider,
            folder: "C:/p".into(),
            account_path: format!("C:/acc/{}/{account}", provider.as_str()),
            account: account.into(),
            parent: parent.map(str::to_owned),
            settings: serde_json::json!({ "background": parent.is_some() }),
            codex_trust: None,
            tools: true,
            waiting: false,
            orphan: false,
        }
    }

    fn limits(pct: u8, resets_at: Option<&str>) -> serde_json::Value {
        serde_json::json!({ "limits": [{ "label": "week", "usedPct": pct, "resetsAt": resets_at }] })
    }

    #[test]
    fn a_crossing_goes_once_to_each_top_chat_with_live_helpers_at_any_depth() {
        let now = now_ms();
        let reset = crate::background::iso(now + 3_600_000).unwrap();
        let known = HashMap::from([
            ("top".into(), session(Provider::Claude, "own", None)),
            ("mid".into(), session(Provider::Claude, "cold", Some("top"))),
            ("lead".into(), session(Provider::Claude, "cold", Some("mid"))),
            ("deep".into(), session(Provider::Claude, "hot", Some("lead"))),
            ("peer".into(), session(Provider::Claude, "hot", Some("mid"))),
            ("other".into(), session(Provider::Codex, "own", None)),
            ("other-helper".into(), session(Provider::Claude, "hot", Some("other"))),
            ("codex-helper".into(), session(Provider::Codex, "hot", Some("other"))),
            ("late".into(), session(Provider::Claude, "own", None)),
            ("late-helper".into(), session(Provider::Claude, "hot", Some("late"))),
            ("dead".into(), session(Provider::Claude, "idle", Some("top"))),
            ("no-top".into(), session(Provider::Claude, "hot", Some("missing"))),
        ]);
        let mut r = Registry { known, ..Registry::default() };
        for (account, pct) in [("claude/hot", 93), ("claude/idle", 99), ("claude/own", 99), ("codex/hot", 88)] {
            r.usage.insert(account.into(), limits(pct, Some(&reset)));
        }
        let at_crossing = |id: &str| ["deep", "peer", "other-helper", "codex-helper", "no-top"].contains(&id);
        assert!(queue_quota_news(&mut r, now, at_crossing));
        let line = format!("Account claude/hot crossed 90% of its \"week\" window (93% used, resets {reset}); your helpers run on it.");
        assert_eq!(r.news, vec![("other".into(), line.clone()), ("top".into(), line.clone())]);

        // A busy chat keeps its queued notice. A newly live helper or a top
        // chat that registers later has no claim on an earlier crossing.
        r.known.insert("missing".into(), session(Provider::Claude, "own", None));
        assert!(!queue_quota_news(&mut r, now, |_| true));
        assert_eq!(r.news, vec![("other".into(), line.clone()), ("top".into(), line)]);
        r.news.clear();

        r.usage.insert("claude/hot".into(), limits(96, Some(&reset)));
        r.usage.insert("codex/hot".into(), limits(95, None));
        assert!(queue_quota_news(&mut r, now, |id| at_crossing(id) || id == "late-helper"));
        let claude = format!("Account claude/hot crossed 95% of its \"week\" window (96% used, resets {reset}); your helpers run on it.");
        let codex = "Account codex/hot crossed 95% of its \"week\" window (95% used); your helpers run on it.";
        assert_eq!(
            r.news,
            vec![
                ("late".into(), claude.clone()),
                ("missing".into(), claude.clone()),
                ("other".into(), claude.clone()),
                ("top".into(), claude),
                ("other".into(), codex.into()),
            ]
        );
    }

    #[test]
    fn a_level_reached_with_no_recipients_stays_seen_until_reset() {
        let now = now_ms();
        let mut r = Registry::default();
        r.usage.insert("claude/idle".into(), limits(93, None));
        assert!(queue_quota_news(&mut r, now, |_| false));
        let key = ("claude/idle".into(), "week".into());
        assert_eq!(r.quota_told[&key].level, 90);

        r.known.insert("top".into(), session(Provider::Claude, "own", None));
        r.known.insert("helper".into(), session(Provider::Claude, "idle", Some("top")));
        assert!(!queue_quota_news(&mut r, now, |_| true));
        assert!(r.news.is_empty());

        r.usage.insert("claude/idle".into(), limits(5, None));
        assert!(queue_quota_news(&mut r, now, |_| false));
        assert!(!r.quota_told.contains_key(&key));
        r.usage.insert("claude/idle".into(), limits(93, None));
        assert!(queue_quota_news(&mut r, now, |_| true));
        assert_eq!(r.news.len(), 1);
        assert_eq!(r.news[0].0, "top");
    }

    #[test]
    fn account_window_levels_survive_a_core_restart() {
        let now = now_ms();
        let later = crate::background::iso(now + 3_600_000);
        let gone = crate::background::iso(now - 60_000);
        let key = |l: &str| ("claude/a".to_string(), l.to_string());
        let told = HashMap::from([
            (key("week"), QuotaTold { level: 95, resets_at: later.clone() }),
            (key("5 hours"), QuotaTold { level: 90, resets_at: gone }),
        ]);
        let news = vec![("top".into(), "Account claude/a crossed 95%".into())];
        let saved = SavedChats { quota_told: Some(quota_rows(&told)), news: news.clone(), ..SavedChats::default() };
        let restored: SavedChats = serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
        let back = quota_from(restored.quota_told.unwrap(), now);
        assert_eq!(back.len(), 1, "a window over since is forgotten");
        assert_eq!(back[&key("week")], told[&key("week")]);
        assert_eq!(quota_step(back.get(&key("week")), 96.0, later.as_deref(), now), (95, false));
        assert_eq!(restored.news, news, "queued recipients survive with the crossing");
    }

    #[test]
    fn legacy_chat_rows_keep_the_highest_level_in_an_unexpired_window() {
        let now = now_ms();
        let later = crate::background::iso(now + 3_600_000);
        let gone = crate::background::iso(now - 60_000);
        let rows = serde_json::json!([
            ["chat-a", "claude/a", "week", 95, later],
            ["chat-b", "claude/a", "week", 90, later],
            ["chat-a", "codex/a", "week", 90, later],
            ["chat-b", "codex/a", "week", 95, gone],
            ["chat-c", "claude/a", "5 hours", 95, gone],
        ]);
        let back = quota_from(legacy_quota_rows(&serde_json::to_vec(&rows).unwrap()), now);
        assert_eq!(back.len(), 2);
        assert_eq!(back[&("claude/a".into(), "week".into())].level, 95);
        assert_eq!(back[&("codex/a".into(), "week".into())].level, 90);
        assert!(legacy_quota_rows(b"not json").is_empty());
    }
}
