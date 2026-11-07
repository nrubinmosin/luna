//! Sessions run by sessions. A chat started with Luna's MCP server attached
//! can spawn other chats — another model, another account — send them text,
//! read what they answered, wait for them, and kill or delete them, worktree
//! included. This module is the registry behind those tools: who is who,
//! who may touch whom, and the round trips to the UI that owns the chat list.
//!
//! The core does not know chats, only pty sessions (see ARCHITECTURE.md), so
//! a spawn is a request to the frontend — `agent://spawn` out, `agent_spawned`
//! back — and a delete tells the frontend to drop the row after the session
//! and its worktree are gone. Lineage lives here: every session registered
//! with a parent is that parent's, and a caller may only reach its own
//! descendants — except to list every session, type a message into one and
//! read or wait on its answer, which is how sessions talk to each other; only
//! killing and deleting stay with the lineage. Children outlive their parent:
//! they pass to its parent, or with none stand alone, the window's to manage
//! (see `forget`).
//!
//! A helper does not report back on its own the way a subagent does: its
//! answer sits in its own transcript until someone waits for it or reads it.
//! A caller that asked something (spawn, send) and ended its turn without
//! collecting the answer would sit idle beside a helper that finished long
//! ago, so the notifier here types a one-line notice into the caller once it
//! is idle: the helper answered, stopped on a prompt, or exited.

mod answer;
mod ask;
mod delivery;
mod list;
mod lost;
mod notifier;
mod owed;
mod persist;
pub(crate) mod privilege;
mod read;
mod registry;
mod resume;
mod said;
pub(crate) mod spawn;
mod teardown;
mod wait;

pub use answer::answer;
pub use ask::{on_hook, resolve_option, state_of, Ask, AskQuestion};
pub use delivery::{held_count, notes_dir, send};
pub use list::{list, sessions, SessionFilter};
pub use lost::undelivered_left;
#[cfg(test)]
pub(crate) use notifier::riders_go;
pub use notifier::{notify, NOTICE_TAG};
pub use owed::{resumed, retried};
pub use persist::{conversation_of, freeze_chats, restore_chats, save_chats, take_up_spawns, tie_back, ties_of, Ties};
pub(crate) use privilege::{command_prefix, command_reach_now};
pub use read::{read, Message};
pub(crate) use registry::mint;
pub use registry::{
    caller_of, chat_name, chat_names, has_tools, label, parent_of, parent_on_start, promoted, register, register_dev_caller,
    resumes_on_restart, session_exited, settings_of, DEV_CALLER,
};
pub use resume::agent_resumes;
pub use spawn::account::account_usage;
pub use spawn::worktree::{helper_worktree_name, name_to_go_on, report_codex_worktrees, worktree_for_start};
pub use spawn::{agent_spawned, agent_spawns, chat_starting, spawn, spawn_is, SpawnParams};
#[cfg(test)]
pub use teardown::news_for;
pub use teardown::{delete, delete_by_user, forget, kill, queued_refused};
pub use wait::{wait, wait_many};

use crate::background::MarkStands;
use crate::provider::Provider;
use crate::throttle::now_ms;
use ask::live;
use delivery::held_sender;
use notifier::notifier;
use read::transcript_path;
use registry::{reg, Registry};
use std::collections::HashMap;
use std::sync::Mutex;

/// The percentages of a usage window a caller hears of, once each per window.
const QUOTA_LEVELS: [u8; 2] = [90, 95];

/// A window this far below the level it was told at has reset, whatever its
/// reset time says; a point or two is rounding.
const QUOTA_SLACK: f64 = 5.0;

/// The last a caller heard of an account's usage window.
#[derive(Clone, Debug, PartialEq)]
struct QuotaTold {
    level: u8,
    resets_at: Option<String>,
}

/// Starts the notifier and what types in the sends held for people, once.
pub fn init() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        std::thread::Builder::new().name("luna-notifier".into()).spawn(notifier).expect("spawn notifier thread");
        std::thread::Builder::new().name("luna-held".into()).spawn(held_sender).expect("spawn held sender thread");
    });
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

fn quota_file() -> std::path::PathBuf {
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
fn quota_from(bytes: &[u8], now: u64) -> HashMap<(String, String, String), QuotaTold> {
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
fn save_quota_told() {
    static SAVING: Mutex<()> = Mutex::new(());
    let _saving = SAVING.lock().unwrap_or_else(|e| e.into_inner());
    let bytes = quota_bytes(&reg().lock().unwrap_or_else(|e| e.into_inner()).quota_told);
    let path = quota_file();
    if let Err(e) = crate::secret::write_private(&path, &bytes) {
        crate::log::warn("agents", &format!("could not save {}: {e}", path.display()));
    }
}

/// A usage window a caller is yet to hear of.
struct QuotaNews {
    key: (String, String, String),
    told: QuotaTold,
    line: String,
}

/// What each caller is yet to hear of the usage windows of the accounts its
/// live helpers run on, all generations counted, as `account_usage` last showed
/// them. It is told only once the notice has gone in (`quota_told`); what has
/// reset or come down is settled here.
fn quota_news(pty: &crate::pty::PtyManager) -> HashMap<String, Vec<QuotaNews>> {
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
fn your_helper(id: &str, name: Option<&str>) -> String {
    match name {
        Some(n) => format!("Your helper \"{n}\" ({id})"),
        None => format!("Your helper {id}"),
    }
}

/// A budget passed that the helper's parent is yet to hear of.
struct BudgetNews {
    id: String,
    /// When it was passed: the line rides from then (`riders_go`).
    since: u64,
    line: String,
}

/// The first reading past a budget, kept as read, as `budgetBreach`: a compaction
/// may take the helper back under it before its parent can be told. At its budget
/// it has not passed it.
fn budget_breach(budget: u64, context: &crate::background::Context, now: u64) -> Option<serde_json::Value> {
    (context.tokens > budget)
        .then(|| serde_json::json!({ "at": now, "tokens": context.tokens, "window": context.window, "pct": context.pct }))
}

fn budget_line(who: &str, budget: u64, breach: &serde_json::Value) -> String {
    format!(
        "{who} has passed its budget of {budget} context tokens: {} of its {} window ({}%). Nothing was stopped.",
        breach["tokens"], breach["window"], breach["pct"]
    )
}

/// One watched helper, as `budget_news` takes it off the registry.
struct Budgeted {
    id: String,
    provider: Provider,
    budget: u64,
    breach: Option<serde_json::Value>,
    parent: Option<String>,
    name: Option<String>,
}

/// What the parents of helpers with a budget are yet to hear, by parent. Where it
/// stands is kept in the helper's settings, so a core started later goes on from
/// there: `budgetBreach` once one is seen, `budgetTold` once the line has reached the
/// parent. Between the two a background parent's own record says where the line is
/// (`background::budget_mark`), it being marked in the text itself. Told once: a helper
/// told of is not watched again. A transcript is read only when `read`. Also says
/// whether where any stands moved, to be saved at once.
fn budget_news(pty: &crate::pty::PtyManager, read: bool) -> (HashMap<String, Vec<BudgetNews>>, bool) {
    let watched: Vec<Budgeted> = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        r.known
            .iter()
            .filter(|(_, k)| k.settings["budgetTold"] != true)
            .filter_map(|(id, k)| {
                let budget = k.settings["budgetTokens"].as_u64()?;
                let parent = k.parent.clone().filter(|p| p != DEV_CALLER && r.known.contains_key(p));
                let name = r.names.get(id).map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
                let some = |key: &str| k.settings.get(key).filter(|v| !v.is_null()).cloned();
                Some(Budgeted { id: id.clone(), provider: k.provider, budget, breach: some("budgetBreach"), parent, name })
            })
            .collect()
    };
    let mut news: HashMap<String, Vec<BudgetNews>> = HashMap::new();
    let mut moved = false;
    for w in watched {
        let mut breach = w.breach;
        if breach.is_none() && read {
            // Off the registry lock: this reads the session's transcript.
            let context = transcript_path(pty, &w.id).and_then(|path| crate::background::Context::of(&path, w.provider));
            breach = context.and_then(|c| budget_breach(w.budget, &c, now_ms()));
            if let Some(b) = &breach {
                crate::log::warn(
                    "agents",
                    &format!("{} has passed its budget of {} context tokens: {}", label(&w.id), w.budget, b["tokens"]),
                );
                set_budget_state(&w.id, "budgetBreach", Some(b.clone()));
                moved = true;
            }
        }
        let Some(breach) = breach else { continue };
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
        let line = budget_line(&your_helper(&w.id, w.name.as_deref()), w.budget, &breach);
        news.entry(parent).or_default().push(BudgetNews { id: w.id, since: breach["at"].as_u64().unwrap_or(0), line });
    }
    (news, moved)
}

fn set_budget_state(id: &str, key: &str, value: Option<serde_json::Value>) {
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    if let (Some(k), Some(v)) = (r.known.get_mut(id), value) {
        k.settings[key] = v;
    }
}

/// Whether a helper's parent is yet to hear it passed its budget, and is there to:
/// the notice has no debt to it, so this is all that keeps `reap` from taking the
/// breach with the helper.
fn budget_owed(r: &Registry, id: &str) -> bool {
    r.known.get(id).is_some_and(|k| {
        k.settings["budgetBreach"].is_object()
            && k.settings["budgetTold"] != true
            && k.parent.as_deref().is_some_and(|p| p != DEV_CALLER && r.known.contains_key(p))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::notifier::lines::notice_line;
    use crate::agents::notifier::{Settled, NOTICE_TAG};
    use crate::agents::owed::Owed;
    use crate::agents::persist::{take_up_chats, tie_back, ties_of, SavedChat, SavedChats, SavedSend, Ties};
    use crate::agents::read::transcripts;
    use crate::agents::registry::{reg, register, settings_of, Known, DEV_CALLER};
    use crate::agents::spawn::{chat_starting, SpawnTaken};
    use crate::agents::teardown::forget;
    use crate::agents::wait::REPLY_GRACE_MS;

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

    /// A helper announced as not done, its job over, with a transcript on disk
    /// holding what `said` past `cursor`, the debt `caller` is owed.
    pub(super) fn job_over_helper(id: &str, caller: &str, said: &str) -> (Known, Owed, std::path::PathBuf) {
        let none = serde_json::json!({});
        register(id, Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, false, none);
        crate::background::idle_for_test(id, Provider::Claude, 3, false);
        let path = std::env::temp_dir().join(format!("luna-{id}-{}.jsonl", std::process::id()));
        let before = "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"I am starting the job.\"}]}}\n";
        std::fs::write(&path, format!("{before}{said}")).unwrap();
        transcripts().lock().unwrap().insert(id.into(), path.clone());
        let o = Owed::pending(3, before.len() as u64);
        reg().lock().unwrap().owed.insert((caller.to_string(), id.to_string()), o);
        (reg().lock().unwrap().known.get(id).cloned().unwrap(), o, path)
    }

    /// The notice as the notifier would make it, past the grace for a reply
    /// that is not coming.
    pub(super) fn notice_for(caller: &str, id: &str, turns_ended: u32) -> (Option<String>, Settled) {
        let (k, mut o) = {
            let r = reg().lock().unwrap();
            (r.known.get(id).cloned().unwrap(), r.owed.get(&(caller.to_string(), id.to_string())).copied().unwrap())
        };
        o.held = Some((turns_ended, now_ms() - REPLY_GRACE_MS));
        notice_line(crate::pty::manager(), caller, "Your helper", id, &k, o).unwrap()
    }

    /// A chat as the last core left it: spawned by `parent`, its turn cut short
    /// with `flag` or not, and owed to `parent` from count 3 on.
    pub(super) fn cut_short_chat(id: &str, parent: &str, flag: bool, mid_turn: bool, already: bool) -> SavedChat {
        let settings = if flag { serde_json::json!({ "resumeOnRestart": true }) } else { serde_json::json!({}) };
        SavedChat {
            id: id.into(),
            provider: Provider::Claude,
            folder: "C:\\p".into(),
            account_path: "C:\\acc\\anthropic\\one".into(),
            turns_ended: 3,
            transcript: None,
            mid_turn,
            ties: Ties {
                parent: Some(parent.into()),
                settings,
                owed: vec![(parent.into(), 3, 40)],
                seen: vec![(parent.into(), 3)],
                cut_short: if already { vec![parent.into()] } else { Vec::new() },
                ..Default::default()
            },
        }
    }

    pub(super) fn take_up(chats: Vec<SavedChat>, sends: Vec<SavedSend>, spawns: Vec<SpawnTaken>) {
        take_up_chats(serde_json::from_slice(&serde_json::to_vec(&SavedChats { chats, sends, spawns }).unwrap()).unwrap());
    }

    /// `settle_spawns` and its like go over every spawn there is, a test's or
    /// another's, as they do in a core: one test at a time.
    pub(super) fn spawns_to_myself() -> std::sync::MutexGuard<'static, ()> {
        static ONE: Mutex<()> = Mutex::new(());
        ONE.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(super) fn spawn_taken(request: u64, caller: &str, helper: Option<&str>, resume: bool) -> SpawnTaken {
        SpawnTaken {
            request,
            caller: caller.into(),
            params: serde_json::from_value(serde_json::json!({ "prompt": "the whole job, word for word", "name": "the job" })).unwrap(),
            auto_delete: None,
            resume_on_restart: resume,
            budget_tokens: None,
            visible: true,
            in_worktree: None,
            account_path: "C:\\acc\\anthropic\\one".into(),
            worktrees_before: Vec::new(),
            asked: None,
            helper: helper.map(str::to_owned),
            owed_placed: false,
            left_by_core: false,
            settled: false,
            news: None,
            retried: false,
        }
    }

    /// A visible helper as the last core left it, its first turn having gone with
    /// it: asked `opening`, owed to `parent`, with `held` from that parent waiting.
    pub(super) fn first_turn_cut(id: &str, parent: &str, provider: Provider, opening: &str, held: &[&str]) {
        let mut chat = cut_short_chat(id, parent, false, true, false);
        chat.provider = provider;
        chat.turns_ended = 0;
        chat.ties.owed = vec![(parent.into(), 0, 0)];
        chat.ties.seen = Vec::new();
        chat.ties.opening = Some(opening.into());
        let sends =
            held.iter().map(|text| SavedSend { caller: parent.into(), id: id.into(), text: (*text).into(), going_in: None }).collect();
        take_up(vec![chat], sends, Vec::new());
    }

    /// The window starts it: no longer waiting for that.
    pub(super) fn back(id: &str, parent: &str, provider: Provider) {
        register(id, provider, "C:\\p", "C:\\acc\\anthropic\\one", Some(parent), false, serde_json::json!({}));
    }

    /// A caller a wait is not let go by, and a helper that lost its first turn, back
    /// afresh with `held` from it waiting.
    pub(super) fn lost_helper(caller: &str, id: &str, dir: &str, held: &[&str]) {
        register(caller, Provider::Claude, dir, dir, None, true, serde_json::json!({}));
        crate::background::test_helper(caller, dir, None, false);
        first_turn_cut(id, caller, Provider::Claude, "the whole job", held);
        back(id, caller, Provider::Claude);
        chat_starting(id, false);
    }

    fn context(tokens: u64) -> crate::background::Context {
        crate::background::Context { pct: 21, tokens, window: 1_000_000 }
    }

    #[test]
    fn a_budget_is_passed_only_past_it_and_its_line_says_so() {
        assert_eq!(budget_breach(200_000, &context(200_000), 5), None, "at it, not past it");
        let b = budget_breach(200_000, &context(200_001), 5).unwrap();
        assert_eq!(b, serde_json::json!({ "at": 5, "tokens": 200_001, "window": 1_000_000, "pct": 21 }));
        assert_eq!(
            budget_line(&your_helper("kid", Some("Tests")), 200_000, &b),
            "Your helper \"Tests\" (kid) has passed its budget of 200000 context tokens: 200001 of its 1000000 window (21%). Nothing was stopped."
        );
        assert!(budget_line(&your_helper("kid", None), 1, &b).starts_with("Your helper kid has passed"));
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
    fn a_budget_breach_stays_owed_through_a_compaction_and_a_core_and_is_told_once() {
        let _budgets = budgets_to_myself();
        let pty = crate::pty::manager();
        let dir = std::env::temp_dir().join(format!("luna-budget-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (parent, kid, provider) in [("bb-p1", "bb-kid1", Provider::Claude), ("bb-p2", "bb-kid2", Provider::Codex)] {
            tooled(parent);
            let news_of = |read: bool| budget_news(pty, read).0.remove(parent).unwrap_or_default();
            let path = dir.join(format!("{kid}.jsonl"));
            write_usage(&path, provider, 900);
            transcripts().lock().unwrap().insert(kid.to_string(), path.clone());
            let start = serde_json::json!({ "budgetTokens": 1000 });
            register(kid, provider, "C:\\p", "C:\\acc\\one", Some(parent), false, start);
            assert!(news_of(true).is_empty(), "{kid}: under it");
            assert!(settings_of(kid).get("budgetBreach").is_none());
            write_usage(&path, provider, 1500);
            assert!(news_of(false).is_empty(), "{kid}: not a look that reads");
            let first = news_of(true);
            assert_eq!(first.len(), 1, "{kid}");
            assert!(first[0].line.contains("has passed its budget of 1000 context tokens: 1500 of its"), "{}", first[0].line);
            let breach = settings_of(kid)["budgetBreach"].clone();
            assert_eq!(breach["tokens"], 1500);

            // Its parent busy, the helper compacts: what was read stands.
            write_usage(&path, provider, 100);
            let again = news_of(true);
            assert_eq!(again.len(), 1, "{kid}: still owed");
            assert_eq!(again[0].line, first[0].line);
            assert_eq!(again[0].since, first[0].since, "and its time");

            // A core goes before the parent is told: its ties carry it over.
            let ties = serde_json::to_vec(&ties_of(kid).unwrap()).unwrap();
            reg().lock().unwrap().known.remove(kid);
            let ties: Ties = serde_json::from_slice(&ties).unwrap();
            tie_back(kid, provider, "C:\\p", "C:\\acc\\one", ties);
            assert_eq!(news_of(true)[0].line, first[0].line, "{kid}: after a core");

            // Typed into its parent: told, not watched, and a second crossing says nothing.
            set_budget_state(kid, "budgetTold", Some(serde_json::json!(true)));
            write_usage(&path, provider, 2000);
            assert!(news_of(true).is_empty(), "{kid}: told once");
            assert_eq!(settings_of(kid)["budgetTold"], true);
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

    pub(super) fn tooled(id: &str) {
        register(id, Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, serde_json::json!({}));
    }
}
