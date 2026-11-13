//! News of usage: an account's window past 90% and 95% (`quota_news`), and a
//! helper's context past its level (`budget_news`).

use crate::agents::ask::live;
use crate::agents::notifier::NOTICE_TAG;
use crate::agents::read::transcript_path;
use crate::agents::registry::{label, reg, Registry, DEV_CALLER};
use crate::background::MarkStands;
use crate::provider::Provider;
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
    at.and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()).is_some_and(|t| (t.timestamp_millis() as u64) < now)
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

/// A usage window a caller is yet to hear of.
pub(in crate::agents) struct QuotaNews {
    pub(in crate::agents) key: (String, String, String),
    pub(in crate::agents) told: QuotaTold,
    pub(in crate::agents) line: String,
}

/// What each caller is yet to hear of the usage windows of the accounts its
/// live helpers run on, all generations counted, as `account_usage` last showed
/// them. It is told only once the notice has gone in (`quota_told`); what has
/// reset or come down is settled here.
pub(in crate::agents) fn quota_news(pty: &crate::pty::PtyManager) -> HashMap<String, Vec<QuotaNews>> {
    // Every helper's account, under every session above it.
    type Window = (String, f64, Option<String>);
    let (windows, helpers): (HashMap<String, Vec<Window>>, Vec<(String, String, String)>) = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let windows: HashMap<String, Vec<Window>> = r
            .usage
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
            .collect();
        // Nothing near a level and nothing told: the usual case, every look.
        if r.quota_told.is_empty() && !windows.values().flatten().any(|w| quota_level(w.1) > 0) {
            return HashMap::new();
        }
        let mut helpers = Vec::new();
        for (id, k) in &r.known {
            let account = format!("{}/{}", k.provider.as_str(), k.account);
            let mut above = k.parent.clone();
            while let Some(caller) = above {
                above = r.known.get(&caller).and_then(|p| p.parent.clone());
                if caller != DEV_CALLER && r.known.contains_key(&caller) {
                    helpers.push((caller, account.clone(), id.clone()));
                }
            }
        }
        (windows, helpers)
    };
    let live_ones: Vec<(String, String)> = helpers.into_iter().filter(|(_, _, id)| live(pty, id)).map(|(c, a, _)| (c, a)).collect();
    let mut news: HashMap<String, Vec<QuotaNews>> = HashMap::new();
    let now = now_ms();
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let before = quota_bytes(&r.quota_told);
    let mut pairs: Vec<&(String, String)> = live_ones.iter().collect();
    pairs.sort();
    pairs.dedup();
    for (caller, account) in pairs {
        for (label, pct, resets_at) in windows.get(account).into_iter().flatten() {
            let key = (caller.clone(), account.clone(), label.clone());
            let (level, say) = quota_step(r.quota_told.get(&key), *pct, resets_at.as_deref(), now);
            let told = QuotaTold { level, resets_at: resets_at.clone() };
            if say {
                let resets = resets_at.as_deref().map(|t| format!(" (resets {t})")).unwrap_or_default();
                let line = format!("Account {account}, which your helpers run on, has used {pct:.0}% of its \"{label}\" window{resets}.");
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

/// A helper's context past its level that its parent is yet to hear of on its
/// own (`ALARM_PCT`).
pub(in crate::agents) struct BudgetNews {
    pub(in crate::agents) id: String,
    /// When it was passed: the line rides from then (`riders_go`).
    pub(in crate::agents) since: u64,
    pub(in crate::agents) line: String,
}

/// Past this share of its window, or its own level if that is higher, a
/// helper's context is news of its own: a notice that rides, told once. Below
/// it and past its level, it is said only beside other news of the helper
/// (`context_rider`): a reviewer at 60% is worth a word, not a turn.
pub(in crate::agents) const ALARM_PCT: u64 = 85;

/// The share of its window past which notices about a helper say where its
/// context stands, if any: its spawn's `budgetPct` (0: never); else its
/// deprecated `budgetTokens`, as the share of `window` they are; else the
/// person's setting (`default`, 0: never).
fn level_of(settings: &serde_json::Value, default: u64, window: u64) -> Option<u64> {
    match (settings["budgetPct"].as_u64(), settings["budgetTokens"].as_u64()) {
        (Some(pct), _) => Some(pct).filter(|p| *p > 0),
        (None, Some(tokens)) if window > 0 => Some((tokens.saturating_mul(100)).div_ceil(window).clamp(1, 100)),
        (None, Some(_)) => None,
        (None, None) => Some(default).filter(|p| *p > 0),
    }
}

/// Whether `context` is past `level` percent of its window: by its tokens, as
/// its rounded `pct` would call 50.4% past 50.
fn past(context: &crate::background::Context, level: u64) -> bool {
    context.tokens.saturating_mul(100) > level.saturating_mul(context.window)
}

/// The first reading past the alarm level, kept as read, as `budgetBreach`: a
/// compaction may take the helper back under it before its parent can be told.
fn budget_breach(context: &crate::background::Context, now: u64) -> serde_json::Value {
    serde_json::json!({ "at": now, "tokens": context.tokens, "window": context.window, "pct": context.pct })
}

fn budget_line(who: &str, breach: &serde_json::Value, rotate: bool) -> String {
    let rotate = if rotate {
        " A fresh helper briefed on what is settled would have room; this one may be losing what it read early on."
    } else {
        ""
    };
    format!(
        "{who} has used {}% of its context window ({} of {} tokens). Nothing was stopped.{rotate}",
        breach["pct"], breach["tokens"], breach["window"]
    )
}

/// Whether helper `id` hands its conversation off by itself as its context
/// fills, so that a notice about its context has no rotation to advise: a
/// lead while auto-handoff for leads is on (handoff `lead_hands_off`).
fn hands_off_by_itself(id: &str) -> bool {
    crate::agents::handoff::lead_hands_off(id)
}

/// " (context N%)", the rider every notice about helper `id` carries while its
/// context is past its level (`budget_news`), or nothing.
pub(in crate::agents) fn context_rider(id: &str) -> String {
    reg().lock().unwrap_or_else(|e| e.into_inner()).context_seen.get(id).map_or(String::new(), |pct| format!(" (context {pct}%)"))
}

/// One watched helper, as `budget_news` takes it off the registry.
struct Budgeted {
    id: String,
    provider: Provider,
    settings: serde_json::Value,
    breach: Option<serde_json::Value>,
    told: bool,
    parent: Option<String>,
    name: Option<String>,
}

/// Where helpers' contexts stand, by what their transcripts said last: each one
/// past its level (`level_of`) is in `context_seen` for the rider, and what the
/// parents of those past the alarm level are yet to hear, by parent. Where that
/// stands is kept in the helper's settings, so a core started later goes on
/// from there: `budgetBreach` once one is seen, `budgetTold` once the line has
/// reached the parent. Between the two a background parent's own record says
/// where the line is (`background::budget_mark`), it being marked in the text
/// itself. Told once. Transcripts are read only when `read`. Also says whether
/// where any stands moved, to be saved at once.
pub(in crate::agents) fn budget_news(pty: &crate::pty::PtyManager, read: bool) -> (HashMap<String, Vec<BudgetNews>>, bool) {
    let default = u64::from(crate::settings::get().helper_context_pct.min(100));
    let watched: Vec<Budgeted> = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        r.known
            .iter()
            .filter(|(_, k)| k.parent.is_some())
            .map(|(id, k)| {
                let parent = k.parent.clone().filter(|p| p != DEV_CALLER && r.known.contains_key(p));
                let name = r.names.get(id).map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
                let some = |key: &str| k.settings.get(key).filter(|v| !v.is_null()).cloned();
                Budgeted {
                    id: id.clone(),
                    provider: k.provider,
                    settings: k.settings.clone(),
                    breach: some("budgetBreach"),
                    told: k.settings["budgetTold"] == true,
                    parent,
                    name,
                }
            })
            .collect()
    };
    let mut news: HashMap<String, Vec<BudgetNews>> = HashMap::new();
    let mut moved = false;
    for w in watched {
        let mut breach = w.breach;
        if read {
            // Off the registry lock: this reads the session's transcript.
            let context = transcript_path(pty, &w.id).and_then(|path| crate::background::Context::of(&path, w.provider));
            let level = context.as_ref().and_then(|c| level_of(&w.settings, default, c.window));
            let over = context.as_ref().zip(level).filter(|(c, level)| past(c, *level));
            {
                let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
                match over {
                    Some((c, _)) => r.context_seen.insert(w.id.clone(), c.pct),
                    None => r.context_seen.remove(&w.id),
                };
            }
            if let Some((c, level)) = over.filter(|(c, level)| breach.is_none() && !w.told && past(c, (*level).max(ALARM_PCT))) {
                crate::log::warn("agents", &format!("{} has used {}% of its context, past {}%", label(&w.id), c.pct, level.max(ALARM_PCT)));
                let b = budget_breach(c, now_ms());
                set_budget_state(&w.id, "budgetBreach", Some(b.clone()));
                breach = Some(b);
                moved = true;
            }
        }
        let Some(breach) = breach.filter(|_| !w.told) else { continue };
        let Some(parent) = w.parent else { continue };
        match crate::background::budget_mark(&parent, &w.id) {
            MarkStands::Coming => continue,
            MarkStands::Landed => {
                set_budget_state(&w.id, "budgetTold", Some(serde_json::json!(true)));
                crate::background::budget_mark_noted(&parent, &w.id);
                moved = true;
                continue;
            }
            // Kept as undelivered it would be told of as well: this notice has it.
            MarkStands::Undelivered => {
                crate::background::drop_undelivered_mark(&parent, &w.id, NOTICE_TAG);
                moved = true;
            }
            MarkStands::Nothing => {}
        }
        let line = budget_line(&your_helper(&w.id, w.name.as_deref()), &breach, !hands_off_by_itself(&w.id));
        news.entry(parent).or_default().push(BudgetNews { id: w.id, since: breach["at"].as_u64().unwrap_or(0), line });
    }
    (news, moved)
}

pub(in crate::agents) fn set_budget_state(id: &str, key: &str, value: Option<serde_json::Value>) {
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    if let (Some(k), Some(v)) = (r.known.get_mut(id), value) {
        k.settings[key] = v;
    }
}

/// Whether a helper's parent is yet to hear it passed its budget, and is there to:
/// the notice has no debt to it, so this is all that keeps `reap` from taking the
/// breach with the helper.
pub(in crate::agents) fn budget_owed(r: &Registry, id: &str) -> bool {
    r.known.get(id).is_some_and(|k| {
        k.settings["budgetBreach"].is_object()
            && k.settings["budgetTold"] != true
            && k.parent.as_deref().is_some_and(|p| p != DEV_CALLER && r.known.contains_key(p))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::notifier::NOTICE_TAG;
    use crate::agents::persist::{tie_back, ties_of, Ties};
    use crate::agents::read::transcripts;
    use crate::agents::registry::{reg, register, settings_of, DEV_CALLER};
    use crate::agents::teardown::forget;
    use crate::agents::test_support::tooled;

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
    fn quota_news_goes_to_whoever_a_live_helper_runs_under() {
        let none = serde_json::json!({});
        let acc = "C:/acc/anthropic/qn-hot";
        register("qn-top", Provider::Claude, "C:/p", "C:/acc/anthropic/qn-cold", None, true, none.clone());
        register("qn-mid", Provider::Claude, "C:/p", "C:/acc/anthropic/qn-cold", Some("qn-top"), true, none.clone());
        register("qn-low", Provider::Claude, "C:/p", acc, Some("qn-mid"), false, none.clone());
        register("qn-dead", Provider::Claude, "C:/p", "C:/acc/anthropic/qn-idle", Some("qn-top"), false, none);
        crate::background::test_helper("qn-low", acc, None, false);
        let pty = crate::pty::manager();
        let limits = |pct: u8| serde_json::json!({ "limits": [{ "label": "week", "usedPct": pct, "resetsAt": null }] });
        let set = |pct: u8| {
            let mut r = reg().lock().unwrap();
            r.usage.insert("claude/qn-hot".into(), limits(pct));
            r.usage.insert("claude/qn-idle".into(), limits(99));
        };
        set(92);
        let news = quota_news(pty);
        for caller in ["qn-top", "qn-mid"] {
            let lines = &news[caller];
            let hot: Vec<&QuotaNews> = lines.iter().filter(|n| n.key.1 == "claude/qn-hot").collect();
            assert_eq!(hot.len(), 1, "{caller}");
            assert!(hot[0].line.contains("has used 92% of its \"week\" window"), "{}", hot[0].line);
        }
        assert!(!news.values().flatten().any(|n| n.key.1 == "claude/qn-idle"), "no live helper of its runs there");
        assert!(!news.contains_key("qn-low"), "a session does not hear of its own account");

        // Not told until the notice has gone in: asked again until it has.
        assert!(quota_news(pty).contains_key("qn-top"));
        for n in news.into_values().flatten() {
            reg().lock().unwrap().quota_told.insert(n.key, n.told);
        }
        assert!(!quota_news(pty).contains_key("qn-top"), "once");
        set(96);
        assert_eq!(quota_news(pty)["qn-top"][0].told.level, 95, "the next level");
        forget("qn-top");
        forget("qn-mid");
        assert!(!reg().lock().unwrap().quota_told.keys().any(|(c, _, _)| c == "qn-top" || c == "qn-mid"));
        crate::background::remove("qn-low");
        forget("qn-low");
        forget("qn-dead");
        let mut r = reg().lock().unwrap();
        r.usage.remove("claude/qn-hot");
        r.usage.remove("claude/qn-idle");
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

    /// Writes a transcript or rollout whose last answer has `tokens` in the window.
    fn write_usage(path: &std::path::Path, provider: Provider, tokens: u64) {
        let line = match provider {
            Provider::Claude => {
                serde_json::json!({ "type": "assistant", "message": { "model": "claude-sonnet-5-5", "usage": { "input_tokens": tokens }, "content": [] } })
            }
            Provider::Codex => serde_json::json!({ "timestamp": "t", "type": "event_msg", "payload": { "type": "token_count", "info": {
                "total_token_usage": { "total_tokens": tokens }, "last_token_usage": { "input_tokens": tokens, "total_tokens": tokens }, "model_context_window": 258400 } } }),
        };
        std::fs::write(path, format!("{line}\n")).unwrap();
    }

    #[test]
    fn a_helpers_level_is_its_own_its_tokens_share_or_the_setting() {
        use serde_json::json;
        assert_eq!(level_of(&json!({ "budgetPct": 70 }), 50, 1000), Some(70));
        assert_eq!(level_of(&json!({ "budgetPct": 0 }), 50, 1000), None, "0: never");
        assert_eq!(level_of(&json!({ "budgetTokens": 129_200 }), 50, 258_400), Some(50), "deprecated tokens, as a share");
        assert_eq!(level_of(&json!({ "budgetTokens": 1 }), 50, 258_400), Some(1), "a share, never none");
        assert_eq!(level_of(&json!({ "budgetPct": 30, "budgetTokens": 1 }), 50, 1000), Some(30), "the share wins");
        assert_eq!(level_of(&json!({}), 50, 1000), Some(50), "the person's setting");
        assert_eq!(level_of(&json!({}), 0, 1000), None, "which may be off");
        let c = |tokens| crate::background::Context { pct: tokens / 10, tokens, window: 1000 };
        assert!(!past(&c(500), 50), "at it, not past it");
        assert!(past(&c(501), 50));
    }

    #[test]
    fn a_context_line_says_where_it_stands_and_advises_a_fresh_helper_unless_it_hands_off_itself() {
        let b = budget_breach(&crate::background::Context { pct: 90, tokens: 900_000, window: 1_000_000 }, 5);
        assert_eq!(b, serde_json::json!({ "at": 5, "tokens": 900_000, "window": 1_000_000, "pct": 90 }));
        assert_eq!(
            budget_line(&your_helper("kid", Some("Tests")), &b, false),
            "Your helper \"Tests\" (kid) has used 90% of its context window (900000 of 1000000 tokens). Nothing was stopped."
        );
        assert!(budget_line(&your_helper("kid", None), &b, true)
            .ends_with("A fresh helper briefed on what is settled would have room; this one may be losing what it read early on."));
        assert!(!hands_off_by_itself("kid"), "no such lead");
    }

    /// A lead hands off by itself while auto-handoff for leads is on: its
    /// parent is advised no rotation then, and is again with it off. A helper
    /// without Luna's tools never hands off.
    #[test]
    fn a_lead_hands_off_by_itself_while_auto_handoff_for_leads_is_on() {
        use crate::settings::LeadHandoff;
        let (lead, plain) = ("hb-lead", "hb-plain");
        crate::background::test_lead(lead, Provider::Claude, "C:\\p", None, None);
        crate::background::idle_for_test(plain, Provider::Claude, 0, false);
        crate::agents::handoff::lead_settings_for_test(Some(LeadHandoff { on: true, threshold: 60 }));
        assert!(hands_off_by_itself(lead));
        assert!(!hands_off_by_itself(plain), "no Luna tools");
        crate::agents::handoff::lead_settings_for_test(Some(LeadHandoff { on: false, threshold: 60 }));
        assert!(!hands_off_by_itself(lead), "off");
        crate::agents::handoff::lead_settings_for_test(None);
        crate::background::remove_for_test(lead);
        crate::background::gone_for_test(plain);
    }

    #[test]
    fn a_context_past_its_level_rides_and_past_the_alarm_is_told_once_through_a_compaction_and_a_core() {
        let _budgets = budgets_to_myself();
        let pty = crate::pty::manager();
        let dir = std::env::temp_dir().join(format!("luna-budget-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (parent, kid, provider) in [("bb-p1", "bb-kid1", Provider::Claude), ("bb-p2", "bb-kid2", Provider::Codex)] {
            tooled(parent);
            let news_of = |read: bool| budget_news(pty, read).0.remove(parent).unwrap_or_default();
            let path = dir.join(format!("{kid}.jsonl"));
            write_usage(&path, provider, 1);
            let window = crate::background::Context::of(&path, provider).unwrap().window;
            let at = |pct: u64| window * pct / 100;
            transcripts().lock().unwrap().insert(kid.to_string(), path.clone());
            register(kid, provider, "C:\\p", "C:\\acc\\one", Some(parent), false, serde_json::json!({ "budgetPct": 50 }));
            write_usage(&path, provider, at(40));
            assert!(news_of(true).is_empty(), "{kid}: under it");
            assert_eq!(context_rider(kid), "", "{kid}");
            write_usage(&path, provider, at(60));
            assert!(news_of(true).is_empty(), "{kid}: past its level, a word on its notices and no notice of its own");
            assert_eq!(context_rider(kid), " (context 60%)", "{kid}");
            assert!(settings_of(kid).get("budgetBreach").is_none());
            write_usage(&path, provider, at(90));
            assert!(news_of(false).is_empty(), "{kid}: not a look that reads");
            let first = news_of(true);
            assert_eq!(first.len(), 1, "{kid}: past the alarm, news of its own");
            assert!(first[0].line.contains("has used 90% of its context window"), "{}", first[0].line);

            // Its parent busy, the helper compacts: what was read stands, and the rider goes.
            write_usage(&path, provider, at(10));
            let again = news_of(true);
            assert_eq!(again.len(), 1, "{kid}: still owed");
            assert_eq!(again[0].line, first[0].line);
            assert_eq!(again[0].since, first[0].since, "and its time");
            assert_eq!(context_rider(kid), "");

            // A core goes before the parent is told: its ties carry it over.
            let ties = serde_json::to_vec(&ties_of(kid).unwrap()).unwrap();
            reg().lock().unwrap().known.remove(kid);
            let ties: Ties = serde_json::from_slice(&ties).unwrap();
            tie_back(kid, provider, "C:\\p", "C:\\acc\\one", ties);
            assert_eq!(news_of(true)[0].line, first[0].line, "{kid}: after a core");
            assert_eq!(settings_of(kid)["budgetPct"], 50, "{kid}: its level too");

            // Typed into its parent: told, and a second crossing says nothing of its own.
            set_budget_state(kid, "budgetTold", Some(serde_json::json!(true)));
            write_usage(&path, provider, at(95));
            assert!(news_of(true).is_empty(), "{kid}: told once");
            assert_eq!(context_rider(kid), " (context 95%)", "{kid}: the rider goes on");
            forget(kid);
            forget(parent);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_budget_with_no_parent_to_tell_is_kept_for_when_there_is_one() {
        let _budgets = budgets_to_myself();
        let pty = crate::pty::manager();
        let breach = serde_json::json!({ "at": 1, "tokens": 5, "window": 10, "pct": 50 });
        let settings = serde_json::json!({ "budgetTokens": 1, "budgetBreach": breach });
        register("bn-kid", Provider::Claude, "C:\\p", "C:\\acc\\one", Some("bn-gone"), false, settings.clone());
        assert!(!budget_news(pty, false).0.values().flatten().any(|n| n.id == "bn-kid"));
        assert_eq!(settings_of("bn-kid")["budgetBreach"], breach);
        register("bn-dev", Provider::Claude, "C:\\p", "C:\\acc\\one", Some(DEV_CALLER), false, settings);
        assert!(!budget_news(pty, false).0.values().flatten().any(|n| n.id == "bn-dev"), "a dev caller takes no notices");
        forget("bn-kid");
        forget("bn-dev");
    }

    /// `budget_news` goes over every helper there is, a test's or not.
    fn budgets_to_myself() -> std::sync::MutexGuard<'static, ()> {
        static ONE: Mutex<()> = Mutex::new(());
        ONE.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A background parent, its helper past a budget, and what `budget_news` has for it.
    fn budget_pair(parent: &str, kid: &str) -> (String, crate::background::BudgetMark) {
        let breach = serde_json::json!({ "at": 1, "tokens": 5, "window": 10, "pct": 50 });
        crate::background::idle_for_test(parent, Provider::Claude, 1, false);
        tooled(parent);
        register(
            kid,
            Provider::Claude,
            "C:\\p",
            "C:\\acc\\one",
            Some(parent),
            false,
            serde_json::json!({ "budgetTokens": 1, "budgetBreach": breach }),
        );
        let line = budget_now(parent).expect("a line").clone();
        (format!("{NOTICE_TAG} {line}"), crate::background::BudgetMark { id: kid.to_string(), line })
    }

    fn budget_now(parent: &str) -> Option<String> {
        budget_news(crate::pty::manager(), false).0.remove(parent).unwrap_or_default().pop().map(|n| n.line)
    }

    fn budget_pair_gone(parent: &str, kid: &str) {
        forget(kid);
        forget(parent);
    }

    #[test]
    fn a_budget_line_to_an_idle_parent_counts_only_once_its_stdin_took_it() {
        let _budgets = budgets_to_myself();
        let (parent, kid) = ("bi-parent", "bi-kid");
        let (notice, mark) = budget_pair(parent, kid);
        assert!(!crate::background::place_for_test(parent, &notice, mark.clone()), "idle: a turn is claimed for it");
        assert_eq!(budget_now(parent), None, "on its way, not said again");
        assert_ne!(settings_of(kid)["budgetTold"], true, "and not told yet");
        // The CLI went before it took anything: still owed.
        crate::background::turn_ended_for_test(parent);
        assert_eq!(budget_now(parent).as_deref(), Some(mark.line.as_str()));
        assert_ne!(settings_of(kid)["budgetTold"], true);
        // Its stdin took it.
        assert!(!crate::background::place_for_test(parent, &notice, mark));
        crate::background::stdin_took_for_test(parent);
        assert_eq!(budget_now(parent), None);
        assert_eq!(settings_of(kid)["budgetTold"], true);
        budget_pair_gone(parent, kid);
    }

    #[test]
    fn a_budget_line_behind_a_parents_turn_is_owed_until_a_turn_takes_it_in() {
        let _budgets = budgets_to_myself();
        let (parent, kid) = ("bq-parent", "bq-kid");
        let (notice, mark) = budget_pair(parent, kid);
        crate::background::set_running_for_test(parent);
        assert!(crate::background::place_for_test(parent, &notice, mark.clone()), "behind its turn");
        assert_eq!(budget_now(parent), None, "not typed twice");
        // A turn started on it, not yet taken in.
        crate::background::start_queued_for_test(parent);
        assert_eq!(budget_now(parent), None);
        assert_ne!(settings_of(kid)["budgetTold"], true);
        crate::background::stdin_took_for_test(parent);
        assert_eq!(budget_now(parent), None);
        assert_eq!(settings_of(kid)["budgetTold"], true);
        budget_pair_gone(parent, kid);
    }

    #[test]
    fn a_parent_killed_with_a_budget_line_queued_has_not_heard_it() {
        let _budgets = budgets_to_myself();
        let (parent, kid) = ("bk-parent", "bk-kid");
        let (notice, mark) = budget_pair(parent, kid);
        crate::background::set_running_for_test(parent);
        assert!(crate::background::place_for_test(parent, &notice, mark.clone()));
        crate::background::killed_for_test(parent);
        assert_eq!(budget_now(parent).as_deref(), Some(mark.line.as_str()), "owed, and told over");
        assert_ne!(settings_of(kid)["budgetTold"], true);
        budget_pair_gone(parent, kid);
    }

    #[test]
    fn a_core_going_with_a_budget_line_queued_neither_repeats_it_nor_loses_it() {
        let _budgets = budgets_to_myself();
        // The enqueue and what says a line is on its way are one thing on disk.
        let (parent, kid) = ("bc-parent", "bc-kid");
        let (notice, mark) = budget_pair(parent, kid);
        crate::background::set_running_for_test(parent);
        assert!(crate::background::place_for_test(parent, &notice, mark.clone()));
        crate::background::across_a_core_for_test(parent);
        // A parent that goes on by itself takes the old text: nothing is said again.
        crate::background::resume_for_test(parent);
        assert_eq!(budget_now(parent), None, "resumed with it: not told twice");
        crate::background::stdin_took_for_test(parent);
        assert_eq!(budget_now(parent), None);
        assert_eq!(settings_of(kid)["budgetTold"], true);
        budget_pair_gone(parent, kid);

        // One that does not has it taken back out, and told over, once.
        let (parent, kid) = ("bd-parent", "bd-kid");
        let (notice, mark) = budget_pair(parent, kid);
        crate::background::set_running_for_test(parent);
        assert!(crate::background::place_for_test(parent, &format!("{notice} And more."), mark.clone()));
        crate::background::across_a_core_for_test(parent);
        assert_eq!(budget_now(parent).as_deref(), Some(mark.line.as_str()));
        let kept = crate::background::undelivered_for(parent, "x", true);
        assert_eq!(kept.len(), 1, "what else it said stays");
        assert!(!kept[0].contains(&mark.line), "{kept:?}");
        assert_ne!(settings_of(kid)["budgetTold"], true);
        budget_pair_gone(parent, kid);

        // Landed before the core went: that stands.
        let (parent, kid) = ("be-parent", "be-kid");
        let (notice, mark) = budget_pair(parent, kid);
        assert!(!crate::background::place_for_test(parent, &notice, mark));
        crate::background::stdin_took_for_test(parent);
        crate::background::across_a_core_for_test(parent);
        assert_eq!(budget_now(parent), None);
        assert_eq!(settings_of(kid)["budgetTold"], true);
        budget_pair_gone(parent, kid);
    }
}
