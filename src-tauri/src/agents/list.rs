//! `luna_session_list` and `luna_session_get`, `luna_account_list` and
//! `luna_account_get`: the compact rows a caller polls, and one item whole.

use crate::agents::ask::{state_of, Ask};
use crate::agents::read::transcript_path;
use crate::agents::registry::{descendants, is_descendant, reg, scope_all, Known, DEV_CALLER};
use crate::agents::spawn::account::allowed_account_dirs;
use crate::agents::spawn::background::CLAUDE_MODELS;
use crate::agents::spawn::worktree::{checkout_of, Worktree};
use crate::agents::teardown::AUTO_DELETE_IDLE_MS;
use crate::provider::Provider;
use crate::throttle::now_ms;
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRow {
    pub id: String,
    pub name: String,
    /// The window group, 1-based as the title bar counts (I = 1).
    pub group: Option<u32>,
    pub provider: Provider,
    pub account: String,
    pub parent: Option<String>,
    /// See `state_of`.
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ask: Option<Ask>,
    pub turns_ended: u32,
    /// The flags it runs with (see `Known::settings`).
    pub settings: serde_json::Value,
    #[serde(flatten)]
    pub worktree: Worktree,
    /// The first line of its spawn's prompt: none for a session nobody spawned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// An `autoDelete` helper idle with nobody owed its answer: the minutes
    /// left before it goes, unless it is read or written to first.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deletes_in_min: Option<u64>,
    #[serde(flatten)]
    pub signs: crate::background::Signs,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listing {
    pub me: String,
    pub sessions: Vec<SessionRow>,
    /// Of the sessions asked for by id, those Luna does not know.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unknown: Vec<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AccountRow {
    pub provider: Provider,
    pub name: String,
    /// The settings a helper gets here unless the spawn says otherwise, for
    /// the caller's folder: the flags as `spawn` echoes them.
    pub defaults: serde_json::Value,
    /// The models a helper may ask for here: Claude Code's aliases (a full
    /// id works too), Codex's catalog with each model's name and levels. Only
    /// when asked (`models: true`): Codex's catalog is most of what a listing
    /// weighs, and a caller lists far more often than it picks a model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<serde_json::Value>>,
    /// Its usage as the window last showed it (see `account_usage`); none
    /// before the window has said.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<serde_json::Value>,
}

/// The models a helper may ask for on the account at `path`.
fn models_of(provider: Provider, path: &str) -> Vec<serde_json::Value> {
    match provider {
        Provider::Claude => CLAUDE_MODELS.iter().map(|m| serde_json::json!({ "id": m })).collect(),
        Provider::Codex => crate::codex::models::models_known(path)
            .into_iter()
            .map(|m| serde_json::json!({ "id": m.slug, "name": m.display_name, "efforts": m.efforts, "defaultEffort": m.default_effort }))
            .collect(),
    }
}

/// What `luna_session_get` says of a chat the window runs beyond its state: its turn's
/// start from the activity tracker, and from its transcript when the CLI last
/// wrote there — the screen is no measure, a spinner paints through a hung
/// call — the model and how full its context is.
fn chat_signs(pty: &crate::pty::PtyManager, id: &str, provider: Provider) -> crate::background::Signs {
    let turn_started_at = crate::activity::turn_started_ms(id).and_then(crate::background::iso);
    let Some(path) = transcript_path(pty, id) else {
        return crate::background::Signs { turn_started_at, ..Default::default() };
    };
    let last_activity_at = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|d| crate::background::iso(d.as_millis() as u64));
    let resolved_model = match provider {
        Provider::Claude => crate::claude::session::last_model(&path),
        // The rollout names the model a turn is to run on before asking it:
        // the one that answered is the last with tokens spent after it.
        Provider::Codex => crate::codex::session::answered_model(&path),
    };
    let context = crate::background::Context::of(&path, provider);
    let left_running = crate::background::shown(&crate::activity::left_running(id), now_ms());
    crate::background::Signs { turn_started_at, last_activity_at, resolved_model, context, left_running }
}

/// The caller's own, or the sessions `ids` names, which may be any: what a row
/// says is what `sessions` and `read` already show of them. Of a session not
/// the caller's, what it was asked and is asking (`task`, `ask`) only with
/// `all` (`scope: "all"`): a listing of any session is metadata. `accounts` is
/// the costly part of a listing, in time and in what it adds to the caller's
/// context, and `models` the costliest part of that.
pub fn list(caller: &str, ids: Option<&[String]>, all: bool) -> Listing {
    let mut unknown = Vec::new();
    let rows: Vec<(String, Known, String, Option<u32>, bool)> = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let wanted = match ids {
            None => descendants(&r, caller),
            Some(ids) => {
                let mut wanted: Vec<String> = Vec::new();
                for id in ids {
                    if wanted.contains(id) || unknown.contains(id) {
                        continue;
                    }
                    if r.known.contains_key(id) && (id != DEV_CALLER || caller == DEV_CALLER) {
                        wanted.push(id.clone());
                    } else {
                        unknown.push(id.clone());
                    }
                }
                wanted
            }
        };
        wanted
            .into_iter()
            .filter_map(|id| {
                let name = r.names.get(&id).cloned().unwrap_or_default();
                let group = r.groups.get(&id).map(|g| g + 1);
                let told = all || id == caller || is_descendant(&r, caller, &id);
                r.known.get(&id).cloned().map(|k| (id, k, name, group, told))
            })
            .collect()
    };
    let pty = crate::pty::manager();
    let sessions = rows
        .into_iter()
        .map(|(id, k, name, group, told)| {
            let (deletes_in_min, task) = {
                let r = reg().lock().unwrap_or_else(|e| e.into_inner());
                let left = r.auto.get(&id).and_then(|a| a.idle_since);
                (
                    left.map(|since| AUTO_DELETE_IDLE_MS.saturating_sub(now_ms().saturating_sub(since)).div_ceil(60_000)),
                    r.tasks.get(&id).cloned(),
                )
            };
            let ts = crate::activity::turn_state(&id);
            // A listing is not a reading: it does not move what the caller
            // is taken to have seen of a session's turns.
            let turns_ended = ts.map(|t| t.turns_ended).unwrap_or(0);
            let (state, ask) = state_of(&id);
            let (ask, task) = if told { (ask, task) } else { (None, None) };
            let worktree = checkout_of(pty, &id, &k);
            let signs = crate::background::signs(&id).unwrap_or_else(|| chat_signs(pty, &id, k.provider));
            SessionRow {
                state,
                ask,
                turns_ended,
                id,
                name,
                group,
                provider: k.provider,
                account: k.account,
                parent: k.parent,
                settings: k.settings,
                worktree,
                task,
                deletes_in_min,
                signs,
            }
        })
        .collect();
    Listing { me: caller.to_string(), sessions, unknown }
}

/// What `luna_session_list` is asked: whose sessions, which of them, how many.
/// Each filter is applied before anything is read off a session but the
/// registry: the state, the one filter that needs a look, comes after the rest.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SessionQuery {
    /// `mine` (the default): the caller's helpers and theirs; `all`: every session.
    pub scope: Option<String>,
    /// Only these, of any session: a row says no more of another's than `scope` lets it.
    pub ids: Option<Vec<String>>,
    pub state: Option<Vec<String>>,
    pub provider: Option<String>,
    pub account: Option<String>,
    /// A window group, 1-based as the title bar counts (I = 1), or `mine`.
    pub group: Option<serde_json::Value>,
    /// Text in the name, folder, account or id, any case.
    pub q: Option<String>,
    pub limit: Option<usize>,
    /// Where the page before ended (`nextCursor`).
    pub cursor: Option<String>,
}

/// How many rows a page holds unless asked, and at most.
const PAGE: usize = 50;
const PAGE_MAX: usize = 200;

/// One row of `luna_session_list`: what is on hand without reading a transcript
/// or asking git, some 60-80 tokens. `luna_session_get` has the rest.
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub provider: Provider,
    pub account: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// See `state_of`.
    pub state: &'static str,
    /// What it waits on, by id, for `luna_session_answer`: a background helper's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ask_id: Option<String>,
    pub turns_ended: u32,
    /// The first line of its spawn's prompt (`TASK_CHARS`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<String>,
    /// An `autoDelete` helper idle with nobody owed its answer: minutes left.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deletes_in_min: Option<u64>,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SessionPage {
    pub me: String,
    pub sessions: Vec<SessionSummary>,
    /// More past this page: pass it as `cursor`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    /// Of the sessions asked for by id, those Luna does not know.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unknown: Vec<String>,
}

/// The 1-based group `group` asks for, `mine` resolved against the caller's.
fn group_asked(group: Option<&serde_json::Value>, mine: Option<u32>) -> Result<Option<u32>, String> {
    let Some(v) = group else { return Ok(None) };
    if v.as_str().is_some_and(|s| s.trim().eq_ignore_ascii_case("mine")) {
        return mine.map(Some).ok_or_else(|| "your session is in no window group, so \"mine\" matches nothing".to_string());
    }
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
        .filter(|g| *g >= 1)
        .map(|g| Some(g as u32))
        .ok_or_else(|| format!("group {v} is neither a number from 1 nor \"mine\""))
}

/// When a session last did anything, as cheaply as that is known: a helper's
/// last event, a chat's transcript's last write.
fn last_activity(pty: &crate::pty::PtyManager, id: &str) -> Option<String> {
    if crate::background::has(id) {
        return crate::background::last_activity_ms(id).and_then(crate::background::iso);
    }
    let path = transcript_path(pty, id)?;
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
    crate::background::iso(modified.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as u64)
}

/// `luna_session_list`: the caller's sessions, or with `scope: "all"` every one,
/// or those `ids` names, as compact rows a page at a time. Of a session not the
/// caller's, what it was asked and is asking (`task`, `askId`) only with `all`.
pub fn session_list(caller: &str, q: &SessionQuery) -> Result<SessionPage, String> {
    let all = scope_all(q.scope.as_deref())?;
    let limit = q.limit.unwrap_or(PAGE).clamp(1, PAGE_MAX);
    let from: usize = match q.cursor.as_deref() {
        None | Some("") => 0,
        Some(c) => c.parse().map_err(|_| format!("cursor {c:?} is not one a page gave; leave it out to start over"))?,
    };
    let text = q.q.as_deref().map(str::trim).filter(|t| !t.is_empty()).map(str::to_lowercase);
    let mut unknown = Vec::new();
    type Row = (String, Known, String, bool, Option<String>, Option<u64>);
    let rows: Vec<Row> = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let group = group_asked(q.group.as_ref(), r.groups.get(caller).map(|g| g + 1))?;
        let visible = |id: &str| id != DEV_CALLER || caller == DEV_CALLER;
        let mut ids: Vec<String> = match &q.ids {
            Some(asked) => {
                let mut out: Vec<String> = Vec::new();
                for id in asked {
                    if out.contains(id) || unknown.contains(id) {
                        continue;
                    }
                    if r.known.contains_key(id) && visible(id) {
                        out.push(id.clone());
                    } else {
                        unknown.push(id.clone());
                    }
                }
                out
            }
            None if all => r.known.keys().filter(|id| visible(id)).cloned().collect(),
            None => descendants(&r, caller),
        };
        if q.ids.is_none() {
            ids.sort();
        }
        ids.into_iter()
            .filter_map(|id| {
                let k = r.known.get(&id)?.clone();
                let name = r.names.get(&id).cloned().unwrap_or_default();
                let keep = q.provider.as_deref().is_none_or(|p| k.provider.as_str() == p)
                    && q.account.as_deref().is_none_or(|a| k.account == a)
                    && group.is_none_or(|g| r.groups.get(&id).map(|x| x + 1) == Some(g))
                    && text.as_deref().is_none_or(|t| [&name, &k.folder, &k.account, &id].iter().any(|f| f.to_lowercase().contains(t)));
                if !keep {
                    return None;
                }
                let told = all || id == caller || is_descendant(&r, caller, &id);
                let task = r.tasks.get(&id).cloned().filter(|_| told);
                let left = r.auto.get(&id).and_then(|a| a.idle_since);
                Some((id, k, name, told, task, left))
            })
            .collect()
    };
    let pty = crate::pty::manager();
    let wanted = q.state.as_ref().filter(|s| !s.is_empty());
    let mut matched = rows.into_iter().filter_map(|(id, k, name, told, task, left)| {
        let (state, ask) = state_of(&id);
        if wanted.is_some_and(|w| !w.iter().any(|s| s == state)) {
            return None;
        }
        Some((id, k, name, told, task, left, state, ask))
    });
    let mut sessions = Vec::new();
    for (id, k, name, told, task, left, state, ask) in matched.by_ref().skip(from).take(limit) {
        sessions.push(SessionSummary {
            ask_id: ask.and_then(|a| a.id).filter(|_| told),
            turns_ended: crate::activity::turn_state(&id).map(|t| t.turns_ended).unwrap_or(0),
            last_activity_at: last_activity(pty, &id),
            deletes_in_min: left.map(|since| AUTO_DELETE_IDLE_MS.saturating_sub(now_ms().saturating_sub(since)).div_ceil(60_000)),
            name,
            provider: k.provider,
            account: k.account,
            parent: k.parent,
            state,
            task,
            id,
        });
    }
    let next_cursor = matched.next().is_some().then(|| (from + sessions.len()).to_string());
    Ok(SessionPage { me: caller.to_string(), sessions, next_cursor, unknown })
}

/// `luna_session_get`: one session whole, its row as `list` gives it (settings,
/// worktree, context, leftRunning and the rest) with where it lives on disk.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDetail {
    #[serde(flatten)]
    pub row: SessionRow,
    pub folder: String,
    /// Where the CLI runs: the worktree, for one in a worktree.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// The transcript (Claude Code) or rollout (Codex) file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript: Option<String>,
    /// The CLI's own id for the conversation (`--resume`, `codex resume`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

pub fn session_get(caller: &str, id: &str, all: bool) -> Result<SessionDetail, String> {
    let mut listing = list(caller, Some(&[id.to_string()]), all);
    let row = listing.sessions.pop().ok_or_else(|| format!("unknown session {id}; luna_session_list lists them"))?;
    let folder = reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(id).map(|k| k.folder.clone()).unwrap_or_default();
    let pty = crate::pty::manager();
    let cwd = pty.probe(id).map(|p| p.cwd).or_else(|| crate::background::working_dir(id)).filter(|c| !crate::pty::same_dir(c, &folder));
    let transcript = transcript_path(pty, id).map(|p| p.to_string_lossy().into_owned());
    let session_id = crate::background::session_id(id).or_else(|| pty.probe(id).as_ref().and_then(disk_meta).and_then(|m| m.session_id));
    Ok(SessionDetail { row, folder, cwd, transcript, session_id })
}

/// One row of `luna_account_list`: an account agents may spawn on, and its
/// usage as the window last showed it.
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AccountSummary {
    pub provider: Provider,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<serde_json::Value>,
}

/// `luna_account_list`: every account folder an agent may use, `provider`'s alone
/// if given.
pub fn account_list(provider: Option<&str>) -> Vec<AccountSummary> {
    let usage = reg().lock().unwrap_or_else(|e| e.into_inner()).usage.clone();
    allowed_account_dirs()
        .into_iter()
        .filter(|a| provider.is_none_or(|p| a.provider.as_str() == p))
        .map(|a| AccountSummary {
            usage: usage.get(&format!("{}/{}", a.provider.as_str(), a.name)).cloned(),
            provider: a.provider,
            name: a.name,
        })
        .collect()
}

/// `luna_account_get`: one account with what a helper gets on it in `folder`
/// (default: the caller's), its usage and the models it may ask for.
pub fn account_get(caller: &str, name: &str, provider: Option<&str>, folder: Option<&str>) -> Result<AccountRow, String> {
    let found: Vec<_> =
        allowed_account_dirs().into_iter().filter(|a| a.name == name && provider.is_none_or(|p| a.provider.as_str() == p)).collect();
    if found.len() > 1 {
        return Err(format!("both providers have an account {name:?}: say which with provider"));
    }
    let Some(a) = found.into_iter().next() else {
        return Err(format!("no account {name:?} agents may use; luna_account_list lists them"));
    };
    let folder = match folder {
        Some(f) => f.to_string(),
        None => reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(caller).map(|k| k.folder.clone()).unwrap_or_default(),
    };
    let defaults = match a.provider {
        Provider::Claude => crate::claude::defaults::flags(&a.path, &folder),
        Provider::Codex => crate::codex::defaults::flags(&a.path, &folder),
    };
    let usage = reg().lock().unwrap_or_else(|e| e.into_inner()).usage.get(&format!("{}/{}", a.provider.as_str(), a.name)).cloned();
    Ok(AccountRow { models: Some(models_of(a.provider, &a.path)), provider: a.provider, name: a.name, defaults, usage })
}

/// What the CLI has on disk about a live session: its cwd, ids and title.
pub(super) fn disk_meta(p: &crate::pty::ActivityProbe) -> Option<crate::pty::SessionMeta> {
    match p.provider {
        Provider::Claude => crate::claude::session::meta(p.pid, p.resume.as_deref(), &p.cwd, p.spawned_at_ms, &p.account_path),
        Provider::Codex => crate::codex::session::meta(&crate::codex::session::Live {
            chat_id: &p.id,
            cwd: &p.cwd,
            spawned_at_ms: p.spawned_at_ms,
            resume: p.resume.as_deref(),
            account_path: &p.account_path,
            last_output_ms: p.last_output_ms,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::registry::{reg, register};

    #[test]
    fn an_account_row_has_models_only_when_asked() {
        let row =
            |models| AccountRow { provider: Provider::Codex, name: "one".into(), defaults: serde_json::json!({}), models, usage: None };
        assert_eq!(serde_json::to_value(row(None)).unwrap(), serde_json::json!({ "provider": "codex", "name": "one", "defaults": {} }));
        let with = serde_json::to_value(row(Some(vec![serde_json::json!({ "id": "gpt-5.5" })]))).unwrap();
        assert_eq!(with["models"], serde_json::json!([{ "id": "gpt-5.5" }]));
    }

    #[test]
    fn session_list_filters_by_group_and_text_and_pages() {
        let none = serde_json::json!({});
        register("grp-me", Provider::Claude, "C:\\work\\luna", "C:\\acc\\anthropic\\one", None, true, none.clone());
        register("grp-peer", Provider::Codex, "C:\\work\\asuri", "C:\\acc\\openai\\two", None, false, none.clone());
        register("grp-far", Provider::Claude, "C:\\work\\luna", "C:\\acc\\anthropic\\one", None, false, none.clone());
        register("grp-loose", Provider::Claude, "C:\\work\\luna", "C:\\acc\\anthropic\\one", None, false, none.clone());
        for i in 0..3 {
            register(
                &format!("grp-kid-{i}"),
                Provider::Claude,
                "C:\\work\\luna",
                "C:\\acc\\anthropic\\one",
                Some("grp-me"),
                false,
                none.clone(),
            );
        }
        // Added to what is there and not said as `chat_names` says it: the window's
        // whole list, which would let go of every other test's waiting chat.
        {
            let mut r = reg().lock().unwrap();
            r.names
                .extend([("grp-me", "builder"), ("grp-peer", "Tester"), ("grp-far", "docs")].map(|(k, v)| (k.to_string(), v.to_string())));
            r.groups.extend([("grp-me", 1), ("grp-peer", 1), ("grp-far", 0)].map(|(k, v)| (k.to_string(), v)));
        }
        let ids = |caller: &str, q: SessionQuery| -> Vec<String> {
            session_list(caller, &q).unwrap().sessions.into_iter().map(|s| s.id).filter(|id| id.starts_with("grp-")).collect()
        };
        let all = |q: SessionQuery| SessionQuery { scope: Some("all".into()), ..q };
        // Its own by default; every session with scope all.
        assert_eq!(ids("grp-me", SessionQuery::default()), ["grp-kid-0", "grp-kid-1", "grp-kid-2"]);
        // Groups go out 1-based, as the title bar counts them.
        let by = |g: serde_json::Value| all(SessionQuery { group: Some(g), ..Default::default() });
        assert_eq!(ids("grp-me", by(serde_json::json!("mine"))), ["grp-me", "grp-peer"]);
        assert_eq!(ids("grp-me", by(serde_json::json!(1))), ["grp-far"]);
        assert_eq!(ids("grp-me", by(serde_json::json!("2"))), ["grp-me", "grp-peer"]);
        assert!(session_list("grp-loose", &by(serde_json::json!("mine"))).is_err(), "a session in no group has no mine");
        assert!(session_list("grp-me", &by(serde_json::json!(0))).is_err());
        let q = |text: &str| all(SessionQuery { q: Some(text.into()), ..Default::default() });
        assert_eq!(ids("grp-me", q("tester")), ["grp-peer"], "name, any case");
        assert_eq!(ids("grp-me", q("asuri")), ["grp-peer"], "folder");
        let codex = all(SessionQuery { provider: Some("codex".into()), q: Some("grp-".into()), ..Default::default() });
        assert_eq!(ids("grp-me", codex), ["grp-peer"]);
        // A page at a time, the cursor going on where the last ended.
        let page = |cursor: Option<&str>| {
            session_list("grp-me", &SessionQuery { limit: Some(2), cursor: cursor.map(str::to_owned), ..Default::default() }).unwrap()
        };
        let first = page(None);
        assert_eq!(first.sessions.len(), 2);
        let next = first.next_cursor.clone().expect("one more");
        let second = page(Some(&next));
        assert_eq!(second.sessions.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["grp-kid-2"]);
        assert!(second.next_cursor.is_none());
        assert!(session_list("grp-me", &SessionQuery { cursor: Some("x".into()), ..Default::default() }).is_err());
        // A row is small: what it costs a caller that polls.
        let row = serde_json::to_string(&first.sessions[0]).unwrap();
        assert!(row.len() < 320, "{} chars: {row}", row.len());
        // Named, any session, and unknown ones said.
        let named =
            session_list("grp-me", &SessionQuery { ids: Some(vec!["grp-far".into(), "grp-gone".into()]), ..Default::default() }).unwrap();
        assert_eq!(named.sessions.len(), 1);
        assert_eq!(named.unknown, ["grp-gone"]);
        for i in 0..3 {
            crate::agents::teardown::forget(&format!("grp-kid-{i}"));
        }
    }

    #[test]
    fn session_get_is_one_session_whole() {
        let none = serde_json::json!({});
        register("sg-me", Provider::Claude, "C:\\work\\luna", "C:\\acc\\anthropic\\one", None, true, none.clone());
        register(
            "sg-kid",
            Provider::Claude,
            "C:\\work\\luna",
            "C:\\acc\\anthropic\\one",
            Some("sg-me"),
            false,
            serde_json::json!({ "model": "m" }),
        );
        let got = serde_json::to_value(session_get("sg-me", "sg-kid", false).unwrap()).unwrap();
        assert_eq!((got["id"].as_str(), got["folder"].as_str()), (Some("sg-kid"), Some("C:\\work\\luna")));
        assert_eq!(got["settings"]["model"], "m");
        assert!(session_get("sg-me", "sg-nobody", false).err().unwrap().contains("luna_session_list"));
        crate::agents::teardown::forget("sg-kid");
        crate::agents::teardown::forget("sg-me");
    }
}
