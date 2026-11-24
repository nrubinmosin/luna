//! News of an account's quota at 90% and 95%.

use crate::agents::ask::live;
use crate::agents::registry::{persons_chat_in, reg, top_chat, Registry};
use crate::throttle::now_ms;
use std::collections::HashMap;
use std::sync::Mutex;

/// The percentages of a usage window a caller hears of, once each per window.
const QUOTA_LEVELS: [u8; 2] = [90, 95];

/// A window this far below the level it was told at has reset, whatever its
/// reset time says; a point or two is rounding.
const QUOTA_SLACK: f64 = 5.0;

/// The last a caller heard of an account's usage window.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::agents) struct QuotaTold {
    pub(in crate::agents) level: u8,
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

/// Where a usage window stands against what the caller was last told of it:
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

pub(in crate::agents) fn quota_file() -> std::path::PathBuf {
    crate::paths::data_dir().join("quota-told.json")
}

/// `quota_told` as its file keeps it: a core started later is not to tell a
/// caller again what this one already did.
fn quota_bytes(told: &HashMap<(String, String, String), QuotaTold>) -> Vec<u8> {
    let mut rows: Vec<(&String, &String, &String, u8, &Option<String>)> =
        told.iter().map(|((c, a, l), t)| (c, a, l, t.level, &t.resets_at)).collect();
    rows.sort();
    serde_json::to_vec(&rows).unwrap_or_default()
}

/// The file's rows, but for windows over since they were written.
pub(in crate::agents) fn quota_from(bytes: &[u8], now: u64) -> HashMap<(String, String, String), QuotaTold> {
    let rows: Vec<(String, String, String, u8, Option<String>)> = serde_json::from_slice(bytes).unwrap_or_default();
    rows.into_iter()
        .filter(|row| !is_past(row.4.as_deref(), now))
        .map(|(c, a, l, level, resets_at)| ((c, a, l), QuotaTold { level, resets_at }))
        .collect()
}

/// Writes it out as it is now, called with the registry's lock let go. One
/// save at a time, each taking its own look: two threads that each took a
/// snapshot under the lock and saved it after would race on the one temp file,
/// and the older could land last, a warning told since told again after a restart.
pub(in crate::agents) fn save_quota_told() {
    static SAVING: Mutex<()> = Mutex::new(());
    let _saving = SAVING.lock().unwrap_or_else(|e| e.into_inner());
    let bytes = quota_bytes(&reg().lock().unwrap_or_else(|e| e.into_inner()).quota_told);
    let path = quota_file();
    if let Err(e) = crate::secret::write_private(&path, &bytes) {
        crate::log::warn("agents", &format!("could not save {}: {e}", path.display()));
    }
}

/// A usage window: its label, the share used, when it resets.
type Window = (String, f64, Option<String>);

/// Each account's usage windows, as `account_usage` last showed them.
fn windows_of(r: &Registry) -> HashMap<String, Vec<Window>> {
    r.usage
        .iter()
        .map(|(account, u)| {
            let limits = u["limits"].as_array().into_iter().flatten();
            let w: Vec<Window> = limits
                .filter_map(|l| {
                    let resets = l["resetsAt"].as_str().map(str::to_owned);
                    Some((l["label"].as_str()?.to_string(), l["usedPct"].as_f64()?, resets))
                })
                .collect();
            (account.clone(), w)
        })
        .collect()
}

/// Every helper's account under its top chat: (chat, `provider/name`, helper).
fn accounts_below(r: &Registry) -> Vec<(String, String, String)> {
    r.known
        .iter()
        .filter_map(|(id, k)| {
            k.parent.as_ref()?;
            let chat = top_chat(r, id)?;
            if !r.known.contains_key(&chat) {
                return None;
            }
            Some((chat, format!("{}/{}", k.provider.as_str(), k.account), id.clone()))
        })
        .collect()
}

/// A usage window a caller is yet to hear of.
pub(in crate::agents) struct QuotaNews {
    pub(in crate::agents) key: (String, String, String),
    pub(in crate::agents) told: QuotaTold,
    pub(in crate::agents) line: String,
}

/// What each top chat is yet to hear of the usage windows of the accounts its
/// live helpers run on, all generations counted, as `account_usage` last showed
/// them. It is told only once the notice has gone in (`quota_told`); what has
/// reset or come down is settled here.
pub(in crate::agents) fn quota_news(pty: &crate::pty::PtyManager) -> HashMap<String, Vec<QuotaNews>> {
    let (windows, helpers) = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let windows = windows_of(&r);
        // Nothing near a level and nothing told: the usual case, every look.
        if r.quota_told.is_empty() && !windows.values().flatten().any(|w| quota_level(w.1) > 0) {
            return HashMap::new();
        }
        (windows, accounts_below(&r))
    };
    let live_ones: Vec<(String, String)> = helpers.into_iter().filter(|(_, _, id)| live(pty, id)).map(|(c, a, _)| (c, a)).collect();
    let mut news: HashMap<String, Vec<QuotaNews>> = HashMap::new();
    let now = now_ms();
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let before = quota_bytes(&r.quota_told);
    // Older cores also told leads. Keep unknown IDs in case a chat has yet to
    // register, but drop rows for known delegates.
    let mut told = std::mem::take(&mut r.quota_told);
    told.retain(|(caller, ..), _| {
        #[cfg(test)]
        if !crate::agents::sweep_includes(caller) {
            return true;
        }
        !r.known.contains_key(caller) || persons_chat_in(&r, caller)
    });
    r.quota_told = told;
    let mut pairs: Vec<&(String, String)> = live_ones.iter().collect();
    pairs.sort();
    pairs.dedup();
    for (caller, account) in pairs {
        #[cfg(test)]
        if !crate::agents::sweep_includes(caller) {
            continue;
        }
        for (label, pct, resets_at) in windows.get(account).into_iter().flatten() {
            let key = (caller.clone(), account.clone(), label.clone());
            let (level, say) = quota_step(r.quota_told.get(&key), *pct, resets_at.as_deref(), now);
            let told = QuotaTold { level, resets_at: resets_at.clone() };
            if say {
                let resets = resets_at.as_deref().map(|t| format!(" (resets {t})")).unwrap_or_default();
                let line = format!("Delegate account {account}: {pct:.0}% of \"{label}\" window used{resets}.");
                news.entry(caller.clone()).or_default().push(QuotaNews { key, told, line });
            } else if level == 0 {
                r.quota_told.remove(&key);
            } else {
                r.quota_told.insert(key, told);
            }
        }
    }
    let after = quota_bytes(&r.quota_told);
    drop(r);
    if after != before {
        save_quota_told();
    }
    news
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
    use crate::agents::registry::register;
    use crate::agents::teardown::forget;
    use crate::provider::Provider;

    #[test]
    fn a_window_is_told_once_per_level_until_it_resets() {
        let now = now_ms();
        let later = crate::background::iso(now + 3_600_000).unwrap();
        let later = Some(later.as_str());
        let told = |level: u8| QuotaTold { level, resets_at: later.map(str::to_owned) };

        assert_eq!(quota_step(None, 89.0, later, now), (0, false));
        assert_eq!(quota_step(None, 90.0, later, now), (90, true));
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

    #[test]
    fn only_the_top_chat_hears_quota_for_live_delegates_at_every_depth() {
        let _sweep = crate::agents::test_support::sweep_only(&["qn-top", "qn-mid", "qn-lead", "qn-low", "qn-peer", "qn-dead"]);
        let none = serde_json::json!({});
        let bg = serde_json::json!({ "background": true });
        let acc = "C:/acc/anthropic/qn-hot";
        let top_acc = "C:/acc/anthropic/qn-own";
        register("qn-mid", Provider::Claude, "C:/p", "C:/acc/anthropic/qn-cold", Some("qn-top"), true, bg.clone());
        register("qn-lead", Provider::Claude, "C:/p", "C:/acc/anthropic/qn-inner", Some("qn-mid"), true, bg.clone());
        register("qn-low", Provider::Claude, "C:/p", acc, Some("qn-lead"), false, bg.clone());
        register("qn-peer", Provider::Claude, "C:/p", acc, Some("qn-mid"), false, bg.clone());
        register("qn-dead", Provider::Claude, "C:/p", "C:/acc/anthropic/qn-idle", Some("qn-top"), false, bg);
        for id in ["qn-low", "qn-peer"] {
            crate::background::test_helper(id, acc, None, false);
        }
        let pty = crate::pty::manager();
        let limits = |pct: u8| serde_json::json!({ "limits": [{ "label": "week", "usedPct": pct, "resetsAt": null }] });
        let set = |pct: u8| {
            let mut r = reg().lock().unwrap();
            r.usage.insert("claude/qn-hot".into(), limits(pct));
            r.usage.insert("claude/qn-idle".into(), limits(99));
            r.usage.insert("claude/qn-own".into(), limits(99));
        };
        set(92);
        assert!(!quota_news(pty).values().flatten().any(|n| n.key.1 == "claude/qn-hot"), "the top is missing");

        register("qn-top", Provider::Claude, "C:/p", top_acc, None, true, none);
        let old = ("qn-mid".into(), "claude/qn-hot".into(), "week".into());
        reg().lock().unwrap().quota_told.insert(old, QuotaTold { level: 95, resets_at: None });

        // A known chat without a live terminal has news pending until it returns.
        assert!(!live(pty, "qn-top"));
        let mut news = quota_news(pty);
        for delegate in ["qn-mid", "qn-lead", "qn-low", "qn-peer"] {
            assert!(!news.contains_key(delegate));
        }
        let lines = &news["qn-top"];
        assert_eq!(lines.len(), 1, "one shared delegate account, excluding the chat's own and the dead delegate's");
        assert_eq!(lines[0].key.1, "claude/qn-hot");
        assert!(lines[0].line.contains("92% of \"week\" window used"), "{}", lines[0].line);
        assert!(!reg().lock().unwrap().quota_told.keys().any(|(c, _, _)| c == "qn-mid"));

        // Not told until the notice has gone in: asked again until it has.
        assert!(quota_news(pty).contains_key("qn-top"));
        for n in news.remove("qn-top").unwrap() {
            reg().lock().unwrap().quota_told.insert(n.key, n.told);
        }
        assert!(!quota_news(pty).contains_key("qn-top"), "once");

        set(96);
        assert_eq!(quota_news(pty)["qn-top"][0].told.level, 95, "the next level");
        forget("qn-top");
        assert!(reg().lock().unwrap().known["qn-mid"].orphan);
        assert!(!quota_news(pty).values().flatten().any(|n| n.key.1 == "claude/qn-hot"), "the orphaned line has no chat");
        assert!(!reg().lock().unwrap().quota_told.keys().any(|(c, _, _)| c == "qn-top" || c == "qn-mid"));
        for id in ["qn-low", "qn-peer"] {
            crate::background::remove(id);
            forget(id);
        }
        for id in ["qn-dead", "qn-lead", "qn-mid"] {
            forget(id);
        }
        let mut r = reg().lock().unwrap();
        r.usage.remove("claude/qn-hot");
        r.usage.remove("claude/qn-idle");
        r.usage.remove("claude/qn-own");
    }

    #[test]
    fn what_a_caller_was_told_of_quota_outlives_the_core() {
        let now = now_ms();
        let later = crate::background::iso(now + 3_600_000);
        let gone = crate::background::iso(now - 60_000);
        let mut told = HashMap::new();
        let key = |l: &str| ("c".to_string(), "claude/a".to_string(), l.to_string());
        told.insert(key("week"), QuotaTold { level: 95, resets_at: later.clone() });
        told.insert(key("5 hours"), QuotaTold { level: 90, resets_at: gone });
        let back = quota_from(&quota_bytes(&told), now);
        assert_eq!(back.len(), 1, "a window over since is forgotten");
        assert_eq!(back[&key("week")], QuotaTold { level: 95, resets_at: later.clone() });
        let (level, say) = quota_step(back.get(&key("week")), 96.0, later.as_deref(), now);
        assert_eq!((level, say), (95, false), "not told again");
        assert!(quota_from(b"not json", now).is_empty());
    }
}
