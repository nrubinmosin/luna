//! `luna_list` and `luna_sessions`: the rows a caller sees of the sessions it
//! may reach, and of every session there is.

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accounts: Option<Vec<AccountRow>>,
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

/// Accounts an agent may spawn on — every account folder on disk, minus the
/// ones the user switched off for agents — with what a helper would get in
/// `folder` on each.
fn account_rows(folder: &str, with_models: bool) -> Vec<AccountRow> {
    let usage = reg().lock().unwrap_or_else(|e| e.into_inner()).usage.clone();
    allowed_account_dirs()
        .into_iter()
        .map(|a| {
            let defaults = match a.provider {
                Provider::Claude => crate::claude::defaults::flags(&a.path, folder),
                Provider::Codex => crate::codex::defaults::flags(&a.path, folder),
            };
            let usage = usage.get(&format!("{}/{}", a.provider.as_str(), a.name)).cloned();
            let models = with_models.then(|| models_of(a.provider, &a.path));
            AccountRow { provider: a.provider, name: a.name, defaults, models, usage }
        })
        .collect()
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

/// What `luna_list` says of a visible session beyond its state: its turn's
/// start from the activity tracker, and from its transcript when the CLI last
/// wrote there — the screen is no measure, a spinner paints through a hung
/// call — the model and how full its context is.
fn visible_signs(pty: &crate::pty::PtyManager, id: &str, provider: Provider) -> crate::background::Signs {
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
pub fn list(caller: &str, ids: Option<&[String]>, accounts: bool, models: bool, all: bool) -> Listing {
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
            let signs = crate::background::signs(&id).unwrap_or_else(|| visible_signs(pty, &id, k.provider));
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
    let accounts = accounts.then(|| {
        let folder = reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(caller).map(|k| k.folder.clone()).unwrap_or_default();
        account_rows(&folder, models)
    });
    Listing { me: caller.to_string(), accounts, sessions, unknown }
}

/// One row of `sessions`: any chat Luna has run a session for since it
/// started, whoever started it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnySession {
    pub id: String,
    pub name: String,
    /// The window group, 1-based as the title bar counts (I = 1).
    pub group: Option<u32>,
    pub provider: Provider,
    pub account: String,
    /// The account directory — `CLAUDE_CONFIG_DIR` / `CODEX_HOME` of the
    /// session, where its registry and transcripts live.
    pub account_path: String,
    /// The chat's project folder.
    pub folder: String,
    /// Where the CLI actually runs: the worktree, for a chat in one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// The CLI's own id for the conversation (`--resume` / `codex resume`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// The transcript (Claude Code) or rollout (Codex) file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    pub parent: Option<String>,
    /// The caller itself.
    pub you: bool,
    /// Spawned by the caller (directly or through a child): every tool
    /// reaches it, not only `luna_send`.
    pub yours: bool,
    pub alive: bool,
    /// See `state_of`.
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ask: Option<Ask>,
    /// Runs with no terminal and no row (background/).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub background: bool,
    pub settings: serde_json::Value,
}

/// Which sessions `sessions` returns: a window group, `mine` for the
/// caller's own, and text to look for in the name, folder, account or id.
#[derive(Deserialize, Default)]
pub struct SessionFilter {
    #[serde(default)]
    pub group: Option<serde_json::Value>,
    #[serde(default)]
    pub q: Option<String>,
    /// Of sessions not the caller's, their `ask`, and with `history` their
    /// spawns, only when `all` (`registry::scope_all`).
    #[serde(default)]
    pub scope: Option<String>,
    /// Past spawns instead (agentlog.rs), deleted helpers included.
    #[serde(default)]
    pub history: Option<bool>,
}

impl SessionFilter {
    /// The 1-based group asked for, `mine` resolved against the caller's.
    fn group(&self, mine: Option<u32>) -> Result<Option<u32>, String> {
        let Some(v) = &self.group else { return Ok(None) };
        if v.as_str().is_some_and(|s| s.trim().eq_ignore_ascii_case("mine")) {
            return mine.map(Some).ok_or_else(|| "your session is in no window group, so \"mine\" matches nothing".to_string());
        }
        v.as_u64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
            .filter(|g| *g >= 1)
            .map(|g| Some(g as u32))
            .ok_or_else(|| format!("group {v} is neither a number from 1 nor \"mine\""))
    }
}

/// Whether a row passes a filter whose group is already resolved.
fn keeps(row: &AnySession, group: Option<u32>, q: Option<&str>) -> bool {
    if group.is_some_and(|g| row.group != Some(g)) {
        return false;
    }
    let Some(q) = q.map(str::trim).filter(|q| !q.is_empty()) else { return true };
    let q = q.to_lowercase();
    [&row.name, &row.folder, &row.account, &row.id].iter().any(|f| f.to_lowercase().contains(&q))
}

/// Every session Luna runs, for a caller that has to find another one — to
/// write to it, or to read its transcript off the disk.
pub fn sessions(caller: &str, filter: &SessionFilter) -> Result<Vec<AnySession>, String> {
    let all = scope_all(filter.scope.as_deref())?;
    type Row = (String, Known, String, Option<u32>, bool);
    let (rows, mine): (Vec<Row>, Option<u32>) = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let rows = r
            .known
            .iter()
            .filter(|(id, _)| id.as_str() != DEV_CALLER || caller == DEV_CALLER)
            .map(|(id, k)| {
                let name = r.names.get(id).cloned().unwrap_or_default();
                let group = r.groups.get(id).map(|g| g + 1);
                (id.clone(), k.clone(), name, group, is_descendant(&r, caller, id))
            })
            .collect();
        (rows, r.groups.get(caller).map(|g| g + 1))
    };
    let group = filter.group(mine)?;
    let pty = Some(crate::pty::manager());
    let mut out: Vec<AnySession> = rows
        .into_iter()
        .map(|(id, k, name, group, yours)| {
            let probe = pty.as_ref().and_then(|p| p.probe(&id));
            let meta = probe.as_ref().and_then(disk_meta);
            let transcript = pty.as_ref().and_then(|p| transcript_path(p, &id)).map(|p| p.to_string_lossy().into_owned());
            // The row's name, else what the CLI calls the conversation.
            let name = Some(name)
                .filter(|n| !n.trim().is_empty())
                .or_else(|| meta.as_ref().and_then(|m| m.title.clone().or_else(|| m.first_prompt.clone())))
                .unwrap_or_default();
            let background = crate::background::has(&id);
            let (state, ask) = state_of(&id);
            // What it waits on, its question's words or a command up for leave,
            // is its own: listed for the caller's alone, unless told all.
            let ask = ask.filter(|_| all || yours || id == caller);
            AnySession {
                you: id == caller,
                yours,
                alive: probe.is_some() || background,
                state,
                ask,
                background,
                pid: probe.as_ref().and_then(|p| p.pid),
                cwd: meta
                    .as_ref()
                    .and_then(|m| m.cwd.clone())
                    .or_else(|| probe.as_ref().map(|p| p.cwd.clone()))
                    .or_else(|| crate::background::working_dir(&id)),
                session_id: meta.and_then(|m| m.session_id).or_else(|| crate::background::session_id(&id)),
                transcript,
                id,
                name,
                group,
                provider: k.provider,
                account: k.account,
                account_path: k.account_path,
                folder: k.folder,
                parent: k.parent,
                settings: k.settings,
            }
        })
        .collect();
    out.retain(|row| keeps(row, group, filter.q.as_deref()));
    out.sort_by(|a, b| b.alive.cmp(&a.alive).then_with(|| (&a.account, &a.name).cmp(&(&b.account, &b.name))));
    Ok(out)
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
    fn sessions_filter_by_group_and_text() {
        let none = serde_json::json!({});
        register("grp-me", Provider::Claude, "C:\\work\\luna", "C:\\acc\\anthropic\\one", None, true, none.clone());
        register("grp-peer", Provider::Codex, "C:\\work\\asuri", "C:\\acc\\openai\\two", None, false, none.clone());
        register("grp-far", Provider::Claude, "C:\\work\\luna", "C:\\acc\\anthropic\\one", None, false, none.clone());
        register("grp-loose", Provider::Claude, "C:\\work\\luna", "C:\\acc\\anthropic\\one", None, false, none);
        // Added to what is there and not said as `chat_names` says it: the window's
        // whole list, which would let go of every other test's waiting chat.
        {
            let mut r = reg().lock().unwrap();
            r.names
                .extend([("grp-me", "builder"), ("grp-peer", "Tester"), ("grp-far", "docs")].map(|(k, v)| (k.to_string(), v.to_string())));
            r.groups.extend([("grp-me", 1), ("grp-peer", 1), ("grp-far", 0)].map(|(k, v)| (k.to_string(), v)));
        }

        let ids = |caller: &str, f: SessionFilter| -> Vec<String> {
            let mut v: Vec<String> = sessions(caller, &f).unwrap().into_iter().map(|s| s.id).filter(|id| id.starts_with("grp-")).collect();
            v.sort();
            v
        };
        let by = |g: serde_json::Value| SessionFilter { group: Some(g), q: None, history: None, scope: None };
        // Groups go out 1-based, as the title bar counts them.
        assert_eq!(ids("grp-me", by(serde_json::json!("mine"))), ["grp-me", "grp-peer"]);
        assert_eq!(ids("grp-me", by(serde_json::json!(1))), ["grp-far"]);
        assert_eq!(ids("grp-me", by(serde_json::json!("2"))), ["grp-me", "grp-peer"]);
        assert!(sessions("grp-loose", &by(serde_json::json!("mine"))).is_err(), "a session in no group has no mine");
        assert!(sessions("grp-me", &by(serde_json::json!(0))).is_err());
        let q = |text: &str| SessionFilter { group: None, q: Some(text.into()), history: None, scope: None };
        assert_eq!(ids("grp-me", q("tester")), ["grp-peer"], "name, any case");
        assert_eq!(ids("grp-me", q("asuri")), ["grp-peer"], "folder");
        assert_eq!(
            ids("grp-me", SessionFilter { group: Some(serde_json::json!("mine")), q: Some("build".into()), history: None, scope: None }),
            ["grp-me"]
        );
    }
}
