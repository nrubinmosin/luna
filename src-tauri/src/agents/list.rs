//! `luna_session_list` and `luna_session_get`, `luna_account_list` and
//! `luna_account_get`: the compact rows a caller polls, and one item whole.

use crate::agents::admission::refuse_luna;
use crate::agents::ask::state_of;
use crate::agents::read::transcript_path;
use crate::agents::registry::{descendants, is_descendant, is_dev, is_self, persons_chat_in, reg, scope_all, Registry};
use crate::agents::spawn::account::allowed_account_dirs;
use crate::agents::spawn::places;
use crate::agents::teardown::AUTO_DELETE_IDLE_MS;
use crate::boundary::AbsPath;
use crate::provider::Provider;
use crate::throttle::now_ms;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AccountRow {
    pub provider: Provider,
    pub name: String,
    /// The settings a helper gets here unless the spawn says otherwise, for
    /// the caller's folder: the flags as `spawn` echoes them.
    pub defaults: serde_json::Value,
    /// The models a helper may ask for here, each with its name and levels:
    /// the account's own list for Claude Code (a full id works too; the four
    /// aliases where Luna knows none yet), Codex's catalog. Only
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
        Provider::Claude => crate::claude::models::models_known(path)
            .into_iter()
            .map(|m| {
                serde_json::json!({ "id": m.slug, "name": m.display_name, "efforts": m.efforts, "defaultEffort": m.default_effort, "more": m.more })
            })
            .collect(),
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
    let folder = pty.probe(id).and_then(|p| p.working_dir());
    let launch = pty.compaction_launch(id, folder);
    let context = crate::background::Context::read(&path, id, provider, launch.as_ref()).map(|(context, _)| context);
    let left_running = crate::activity::left_running(id);
    crate::background::Signs { turn_started_at, last_activity_at, resolved_model, context, left_running, ..Default::default() }
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
/// or asking git. `luna_session_get` has the rest.
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// See `state_of`.
    pub state: &'static str,
    /// What it waits on, by id, for `luna_session_answer`: a background helper's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ask_id: Option<String>,
    pub turns_ended: u32,
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

/// A helper names only itself and the sessions under it (`persons_chat_in`):
/// asked for others by id, it is refused, naming them.
fn ids_in_reach(r: &Registry, caller: &str, ids: &[String]) -> Result<(), String> {
    if persons_chat_in(r, caller) {
        return Ok(());
    }
    let past: Vec<&str> = ids
        .iter()
        .filter(|id| !is_self(r, caller, id) && r.known.contains_key(*id) && !is_descendant(r, caller, id))
        .map(String::as_str)
        .collect();
    if past.is_empty() {
        return Ok(());
    }
    Err(format!("{} not under you: a helper looks only at itself and its own helpers and theirs", past.join(", ")))
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

/// List the caller's sessions, every session with scope "all", or the requested ids.
pub fn session_list(caller: &str, q: &SessionQuery) -> Result<SessionPage, String> {
    let all = scope_all(caller, q.scope.as_deref())?;
    let limit = q.limit.unwrap_or(PAGE).clamp(1, PAGE_MAX);
    let from: usize = match q.cursor.as_deref() {
        None | Some("") => 0,
        Some(c) => c.parse().map_err(|_| format!("cursor {c:?} is not one a page gave; leave it out to start over"))?,
    };
    let text = q.q.as_deref().map(str::trim).filter(|t| !t.is_empty()).map(str::to_lowercase);
    let mut unknown = Vec::new();
    let rows: Vec<_> = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let group = group_asked(q.group.as_ref(), r.groups.get(caller).map(|g| g + 1))?;
        ids_in_reach(&r, caller, q.ids.as_deref().unwrap_or_default())?;
        let visible = |id: &str| !is_dev(id) || is_dev(caller);
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
                let k = r.known.get(&id)?;
                let name = r.names.get(&id).cloned().unwrap_or_default();
                let keep = q.provider.as_deref().is_none_or(|p| k.provider.as_str() == p)
                    && q.account.as_deref().is_none_or(|a| k.account == a)
                    && group.is_none_or(|g| r.groups.get(&id).map(|x| x + 1) == Some(g))
                    && text.as_deref().is_none_or(|t| [&name, &k.folder, &k.account, &id].iter().any(|f| f.to_lowercase().contains(t)));
                if !keep {
                    return None;
                }
                let told = all || is_self(&r, caller, &id) || is_descendant(&r, caller, &id);
                Some((id, name, told))
            })
            .collect()
    };
    let wanted = q.state.as_ref().filter(|s| !s.is_empty());
    let mut matched = rows.into_iter().filter_map(|(id, name, told)| {
        let (state, ask) = state_of(&id);
        if wanted.is_some_and(|w| !w.iter().any(|s| s == state)) {
            return None;
        }
        Some((id, name, told, state, ask))
    });
    let mut sessions = Vec::new();
    for (id, name, told, state, ask) in matched.by_ref().skip(from).take(limit) {
        sessions.push(SessionSummary {
            ask_id: ask.and_then(|a| a.id).filter(|_| told),
            turns_ended: crate::activity::turn_state(&id).map(|t| t.turns_ended).unwrap_or(0),
            name,
            state,
            id,
        });
    }
    let next_cursor = matched.next().is_some().then(|| (from + sessions.len()).to_string());
    Ok(SessionPage { me: caller.to_string(), sessions, next_cursor, unknown })
}

/// The session's identity, turn and ask. Read extra groups only when requested.
pub fn session_get(caller: &str, id: &str, all: bool, include: &[String]) -> Result<Map<String, Value>, String> {
    let (known, name, told, task, group, left) = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        ids_in_reach(&r, caller, &[id.to_string()])?;
        let known = r
            .known
            .get(id)
            .filter(|_| !is_dev(id) || is_dev(caller))
            .ok_or_else(|| format!("unknown session {id}; luna_session_list lists them"))?;
        let told = all || is_self(&r, caller, id) || is_descendant(&r, caller, id);
        let task = r.tasks.get(id).filter(|_| told).cloned();
        let group = r.groups.get(id).map(|g| g + 1);
        let left = r.auto.get(id).and_then(|a| a.idle_since);
        (known.clone(), r.names.get(id).cloned().unwrap_or_default(), told, task, group, left)
    };
    let (state, ask) = state_of(id);
    let turns_ended = crate::activity::turn_state(id).map_or(0, |t| t.turns_ended);
    let mut out = Map::from_iter([
        ("id".into(), json!(id)),
        ("name".into(), json!(name)),
        ("provider".into(), json!(known.provider)),
        ("account".into(), json!(known.account)),
        ("state".into(), json!(state)),
        ("turnsEnded".into(), json!(turns_ended)),
    ]);
    if let Some(parent) = known.parent {
        out.insert("parent".into(), json!(parent));
    }
    if let Some(ask) = ask.filter(|_| told) {
        out.insert("ask".into(), json!(ask));
    }
    if let Some(task) = task {
        out.insert("task".into(), json!(task));
    }
    let wants = |group| include.iter().any(|g| g == group);
    if wants("places") {
        out.insert("places".into(), json!(places(caller)));
    }
    if wants("settings") {
        out.insert("settings".into(), known.settings);
    }
    let pty = crate::pty::manager();
    if wants("paths") {
        let cwd =
            pty.probe(id).map(|p| p.cwd).or_else(|| crate::background::working_dir(id)).filter(|c| !crate::pty::same_dir(c, &known.folder));
        out.insert("folder".into(), json!(known.folder));
        if let Some(cwd) = cwd {
            out.insert("cwd".into(), json!(cwd));
        }
        if let Some(path) = transcript_path(pty, id) {
            out.insert("transcript".into(), json!(path.to_string_lossy()));
        }
    }
    if wants("diagnostics") {
        let signs = crate::background::signs(id).unwrap_or_else(|| chat_signs(pty, id, known.provider));
        if let Ok(Value::Object(fields)) = serde_json::to_value(signs) {
            out.extend(fields);
        }
        out.insert("group".into(), json!(group));
        if let Some(since) = left {
            out.insert("deletesInMin".into(), json!(AUTO_DELETE_IDLE_MS.saturating_sub(now_ms().saturating_sub(since)).div_ceil(60_000)));
        }
        if let Some(session_id) =
            crate::background::session_id(id).or_else(|| pty.probe(id).as_ref().and_then(disk_meta).and_then(|m| m.session_id))
        {
            out.insert("sessionId".into(), json!(session_id));
        }
        if let Some(version) = crate::cli::session_version(id) {
            out.insert("cliVersion".into(), json!(version));
        }
    }
    Ok(out)
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
/// (default: the caller's), its usage and the models it may ask for. The
/// folder, named or the caller's own, is refused where a spawn would refuse
/// it, in or above Luna's own, before any account or settings file is read.
pub fn account_get(caller: &str, name: &str, provider: Option<&str>, folder: Option<AbsPath>) -> Result<AccountRow, String> {
    let folder = match folder {
        Some(f) => String::from(f),
        None => reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(caller).map(|k| k.folder.clone()).unwrap_or_default(),
    };
    // A blank one (a caller registered with none) reads no project.
    let folder = if folder.trim().is_empty() {
        String::new()
    } else {
        refuse_luna(&folder)?;
        folder
    };
    let found: Vec<_> =
        allowed_account_dirs().into_iter().filter(|a| a.name == name && provider.is_none_or(|p| a.provider.as_str() == p)).collect();
    if found.len() > 1 {
        return Err(format!("both providers have an account {name:?}: say which with provider"));
    }
    let Some(a) = found.into_iter().next() else {
        return Err(format!("no account {name:?} agents may use; luna_account_list lists them"));
    };
    Ok(account_row(a, &folder))
}

fn account_row(a: crate::accounts::AccountInfo, folder: &str) -> AccountRow {
    let defaults = match a.provider {
        Provider::Claude => crate::claude::defaults::flags(&a.path, folder),
        Provider::Codex => crate::codex::defaults::helper_defaults(&a.path, folder).0,
    };
    let usage = reg().lock().unwrap_or_else(|e| e.into_inner()).usage.get(&format!("{}/{}", a.provider.as_str(), a.name)).cloned();
    AccountRow { models: Some(models_of(a.provider, &a.path)), provider: a.provider, name: a.name, defaults, usage }
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
    fn account_get_reports_which_default_a_helper_would_get() {
        crate::codex::config::use_empty_machine_layers_for_test();
        let root = std::env::temp_dir().join(format!("luna-account-mode-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let defaults = |provider| {
            account_row(crate::accounts::AccountInfo { provider, name: "one".into(), path: root.to_string_lossy().into_owned() }, "")
                .defaults
        };
        let codex = defaults(Provider::Codex);
        assert_eq!(codex["modeFrom"], "luna");
        assert_eq!(codex["approval"], "auto-review");
        assert_eq!(codex["sandbox"], "workspace-write");
        std::fs::write(root.join("config.toml"), "default_permissions = \":workspace\"\n").unwrap();
        let codex = defaults(Provider::Codex);
        assert_eq!(codex["modeFrom"], "cli");
        assert_eq!(codex["approval"], "codex");
        assert_eq!(codex["sandbox"], "codex");
        if !crate::claude::defaults::managed_mode_for_test() {
            let claude = defaults(Provider::Claude);
            assert_eq!(claude["modeFrom"], "luna");
            assert_eq!(claude["permissionMode"], "auto");
            std::fs::write(root.join("settings.json"), r#"{ "permissions": { "defaultMode": "plan" } }"#).unwrap();
            let claude = defaults(Provider::Claude);
            assert_eq!(claude["modeFrom"], "cli");
            assert_eq!(claude["permissionMode"], "plan");
        }
        let _ = std::fs::remove_dir_all(&root);
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
        assert_eq!(json!(first.sessions[0]), json!({ "id": "grp-kid-0", "state": "exited", "turnsEnded": 0 }));
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
    fn session_get_has_only_the_groups_requested() {
        let none = serde_json::json!({});
        register("sg-me", Provider::Claude, "C:\\work\\luna", "C:/acc/anthropic/one", None, true, none.clone());
        register(
            "sg-kid",
            Provider::Claude,
            "C:\\work\\luna",
            "C:/acc/anthropic/one",
            Some("sg-me"),
            false,
            serde_json::json!({ "model": "m", "modeFrom": "luna" }),
        );
        let get = |groups: &[&str]| {
            json!(session_get("sg-me", "sg-kid", false, &groups.iter().map(|g| g.to_string()).collect::<Vec<_>>()).unwrap())
        };
        assert_eq!(
            get(&[]),
            json!({
                "id": "sg-kid", "name": "", "provider": "claude", "account": "one",
                "state": "exited", "turnsEnded": 0, "parent": "sg-me"
            })
        );
        assert_eq!(get(&["paths"])["folder"], "C:\\work\\luna");
        assert_eq!(get(&["settings"])["settings"], json!({ "model": "m", "modeFrom": "luna" }));
        assert!(get(&["diagnostics"]).get("turnStartedAt").is_some());
        assert!(get(&["places"])["places"]["of"].is_number());
        assert!(get(&["paths"]).get("settings").is_none());
        let combined = get(&["settings", "paths"]);
        assert_eq!(combined["settings"]["model"], "m");
        assert_eq!(combined["folder"], "C:\\work\\luna");
        assert!(combined.get("turnStartedAt").is_none() && combined.get("places").is_none());
        assert!(session_get("sg-me", "sg-nobody", false, &[]).err().unwrap().contains("luna_session_list"));
        crate::agents::teardown::forget("sg-kid");
        crate::agents::teardown::forget("sg-me");
    }
}
