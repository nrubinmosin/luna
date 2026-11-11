//! `luna_session_spawn`: a helper run in the background here, in the core, its account,
//! folder and privilege settled first. A spawn under way is on disk until a
//! helper comes of it (`SpawnUnderWay`).

pub(crate) mod account;
pub(super) mod background;
pub(super) mod settle;
pub(super) mod worktree;

use crate::agents::privilege::{command_reach, plain_text, reaches_luna, settle_mode, settle_role, Privilege};
use crate::agents::registry::{depth_of, descendants, is_descendant, reg, top_of, Registry, DEV_CALLER};
use crate::agents::said::{first_words, TASK_CHARS};
use crate::agents::spawn::account::{allowed_account_dirs, list_provider, pick_from_list, signed_in, AccountChoice};
use crate::agents::spawn::background::spawn_background;
use crate::agents::spawn::worktree::{caller_worktree, Neighbor, Worktree};
use crate::provider::Provider;
use crate::throttle::now_ms;
use serde::{Deserialize, Serialize};

/// The most a caller may spawn under itself, all generations counted.
pub(in crate::agents) const MAX_DESCENDANTS: usize = 8;

/// A child of a child may exist; a child of that may not have tools.
const MAX_TOOL_DEPTH: usize = 1;

/// What an agent asks for. Everything but the prompt may be left to the
/// account's defaults.
#[derive(Clone, Serialize, Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct SpawnParams {
    pub provider: Option<String>,
    pub account: Option<AccountAsk>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub permission_mode: Option<String>,
    pub approval: Option<String>,
    pub sandbox: Option<String>,
    pub folder: Option<String>,
    pub worktree: Option<bool>,
    pub prompt: String,
    pub name: Option<String>,
    pub tools: Option<bool>,
    /// Deleted once its answer is out (see `reap`). This and the others not
    /// written with the record of a spawn under way are written beside it
    /// (`SpawnTaken`), as every chat-ties.json has had them.
    #[serde(skip_serializing)]
    pub auto_delete: Option<bool>,
    /// A background helper a core's going stopped mid-turn starts its next
    /// turn by itself under the core after it (`background::resume_stopped`).
    /// Kept in the settings it runs with.
    #[serde(skip_serializing)]
    pub resume_on_restart: Option<bool>,
    /// Context tokens past which its parent hears, once (`budget_news`). Kept
    /// in the settings it runs with.
    #[serde(skip_serializing)]
    pub budget_tokens: Option<u64>,
    /// Without the edit tools (Claude Code); in a read-only sandbox (Codex).
    pub read_only: Option<bool>,
    /// The agent it runs as: Claude Code's `--agent`.
    pub agent: Option<String>,
    /// Keeps the MCP servers and connectors its account has (claude.ai's,
    /// Codex's apps), which a helper otherwise starts without.
    #[serde(skip_serializing)]
    pub connectors: Option<bool>,
}

/// `account` in a spawn: a name, or names to choose from (`pick_from_list`).
/// Once `spawn` has settled it, the one name taken.
#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(untagged)]
pub enum AccountAsk {
    One(String),
    List(Vec<String>),
}

impl SpawnParams {
    /// The account's name, once `spawn` has settled it.
    pub(super) fn account_name(&self) -> Option<&str> {
        match &self.account {
            Some(AccountAsk::One(name)) => Some(name),
            _ => None,
        }
    }
}

/// A `luna_session_spawn` taken on, in `chat-ties.json` until a helper has come of it and
/// its caller is owed by that: a core going before then leaves the caller an error
/// from the call, or nothing at all, and this is what the next core tells it from.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SpawnTaken {
    pub(super) request: u64,
    pub(super) caller: String,
    /// What was asked, its account and folder settled.
    pub(super) params: SpawnParams,
    /// What `params` does not write (`SpawnParams::auto_delete`).
    pub(super) auto_delete: Option<bool>,
    pub(super) resume_on_restart: bool,
    #[serde(default)]
    pub(super) budget_tokens: Option<u64>,
    #[serde(default)]
    pub(super) connectors: bool,
    pub(super) in_worktree: Option<String>,
    pub(super) account_path: String,
    /// With `worktree`: the worktrees there were as it was taken on.
    pub(super) worktrees_before: Vec<String>,
    /// The session it came to, as soon as there is one (`spawn_is`).
    pub(super) helper: Option<String>,
    /// That session owes its caller by now, whatever has become of the debt since,
    /// and no core after this is to make it owe again (`take_up_spawns`).
    #[serde(default)]
    pub(super) owed_placed: bool,
    /// Read from what the last core left: no call waits on it any more.
    #[serde(skip)]
    pub(super) left_by_core: bool,
    /// Given up on, as far as starting goes: a helper still being made for it is
    /// refused (`spawn_is`), so what its caller is told stays true.
    #[serde(skip)]
    pub(super) settled: bool,
    /// What its caller is to be told, once worked out (`settle_spawns`).
    #[serde(skip)]
    pub(super) news: Option<String>,
}

/// A spawn's record for as long as its call runs, however that ends.
pub(super) struct SpawnUnderWay(pub(super) u64);

impl SpawnUnderWay {
    /// On disk before anything is made. `replaces`: the record of the spawn this
    /// asks for again, gone in the same hold, as two would start two after a restart.
    /// Refused past the cap, counted in that same hold (`under_cap`): two spawns at
    /// once cannot both take the last place.
    pub(super) fn begin(
        caller: &str,
        params: &SpawnParams,
        account_path: &str,
        in_worktree: Option<&str>,
        replaces: Option<u64>,
    ) -> Result<SpawnUnderWay, String> {
        let worktrees_before = match (params.worktree, params.folder.as_deref()) {
            (Some(true), Some(folder)) => crate::worktree::made_for(folder, account_path),
            _ => Vec::new(),
        };
        let taken = SpawnTaken {
            request: 0,
            caller: caller.to_string(),
            params: params.clone(),
            auto_delete: params.auto_delete,
            resume_on_restart: params.resume_on_restart == Some(true),
            budget_tokens: params.budget_tokens,
            connectors: params.connectors == Some(true),
            in_worktree: in_worktree.map(str::to_owned),
            account_path: account_path.to_string(),
            worktrees_before,
            helper: None,
            owed_placed: false,
            left_by_core: false,
            settled: false,
            news: None,
        };
        let request = {
            let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
            // A chat stopping for a move into another account looks at its
            // helpers and then stops: nothing of its may start between the two
            // (agents/handoff/moving.rs `stopping`), so the mark is looked at
            // under the same lock the spawn is recorded under.
            let top = top_of(&r, caller);
            if crate::agents::handoff::stopping_for_move(&top) {
                return Err(format!("{top} is being stopped for a move to another account: no helper starts under it now"));
            }
            cap_room(&r, caller, replaces)?;
            let request = r.next_request;
            r.next_request += 1;
            if let Some(old) = replaces {
                r.spawns.remove(&old);
            }
            r.spawns.insert(request, SpawnTaken { request, ..taken });
            request
        };
        crate::background::save();
        Ok(SpawnUnderWay(request))
    }
}

/// The sessions that count against the cap of `caller`'s top chat, by id: all
/// under it, every generation, but an `autoDelete` helper whose answer has
/// reached its caller and that only waits out its time to go (`Auto::idle_since`);
/// and how many spawns of any of them are under way with no session yet,
/// `except` aside (the one asked for again).
pub(in crate::agents) fn under_cap(r: &Registry, caller: &str, except: Option<u64>) -> (Vec<String>, usize) {
    let top = top_of(r, caller);
    let coming = r
        .spawns
        .values()
        .filter(|s| s.helper.is_none() && !s.settled && Some(s.request) != except)
        .filter(|s| s.caller == top || is_descendant(r, &top, &s.caller))
        .count();
    let done = |id: &String| r.auto.get(id).is_some_and(|a| a.released && !a.deleting);
    let mut counted: Vec<String> = descendants(r, &top).into_iter().filter(|id| !done(id)).collect();
    counted.sort();
    (counted, coming)
}

/// Whether `caller` may spawn one more (MAX_DESCENDANTS); refused, it hears which
/// sessions take the places, to delete what it is done with.
fn cap_room(r: &Registry, caller: &str, except: Option<u64>) -> Result<(), String> {
    let (counted, coming) = under_cap(r, caller, except);
    if counted.len() + coming < MAX_DESCENDANTS {
        return Ok(());
    }
    let coming = if coming > 0 { format!(", and {coming} spawn(s) under way") } else { String::new() };
    let whose = if top_of(r, caller) == caller { "you" } else { "the chat at the top of your line" };
    Err(format!(
        "{whose} already {} {MAX_DESCENDANTS} sessions under {}: {}{coming}. Helpers whose answer reached you and that wait \
         out autoDelete do not count; delete those you are done with",
        if whose == "you" { "have" } else { "has" },
        if whose == "you" { "you" } else { "it" },
        counted.join(", ")
    ))
}

impl Drop for SpawnUnderWay {
    fn drop(&mut self) {
        reg().lock().unwrap_or_else(|e| e.into_inner()).spawns.remove(&self.0);
        crate::background::save();
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Spawned {
    pub id: String,
    /// Pass to `wait` as `afterTurn`: the reply to the prompt ends turn 1.
    pub turns_ended: u32,
    /// The flags the session runs with, as resolved from the request and the
    /// account's defaults.
    pub settings: serde_json::Value,
    /// Other live sessions working in the same directory, the caller aside:
    /// they share its files and its git index, commits and all.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub same_folder: Vec<Neighbor>,
    /// The worktree it works in and its branch, once the CLI has made it.
    #[serde(flatten)]
    pub worktree: Worktree,
    /// Asked for a worktree of its own and the CLI has not said where yet:
    /// `luna_session_list` names it once it has.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub worktree_pending: bool,
    /// A Codex helper spawned without connectors: the MCP servers it keeps
    /// all the same, which Codex cannot turn off for a run (codex/config.rs
    /// `servers_left_on`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub mcp_left_on: Vec<String>,
    /// With `account` as a list: the one taken, and why not the ones before.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_choice: Option<AccountChoice>,
}

/// Starts a helper in the background on what was asked; returns once its first
/// turn is under way.
pub fn spawn(caller: &str, params: SpawnParams) -> Result<Spawned, String> {
    spawn_with(caller, params, None)
}

/// `spawn`. `replaces`: the record of the spawn this asks for again.
pub(super) fn spawn_with(caller: &str, mut params: SpawnParams, replaces: Option<u64>) -> Result<Spawned, String> {
    if params.prompt.trim().is_empty() {
        return Err("prompt is required".into());
    }
    // Kept on disk with the spawn, and read back as it was asked: never as keys.
    plain_text(&params.prompt)?;
    if params.budget_tokens == Some(0) {
        return Err("budgetTokens must be above 0".into());
    }
    // Off the registry lock: this reads the CLI's registry from disk. A
    // helper works where its caller does, worktree and all.
    let worktree = if params.folder.is_none() { caller_worktree(caller) } else { None };
    let me = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let me = r.known.get(caller).cloned().ok_or("caller is not a registered session")?;
        // Early, before folders and logins are read; `begin` counts again.
        cap_room(&r, caller, replaces)?;
        if params.tools.unwrap_or(false) && depth_of(&r, caller) >= MAX_TOOL_DEPTH {
            return Err("a session this deep cannot spawn sessions with tools of their own".into());
        }
        me
    };
    // Off the registry lock: what follows reads folders and login files,
    // and every Claude Code session's hook waits on this lock with its next
    // step behind it (activity/sample.rs `sample` has the same care).
    if params.folder.is_none() {
        params.folder = Some(worktree.unwrap_or_else(|| me.folder.clone()));
    }
    // A folder inside a CLI-made worktree files the helper under the
    // worktree's project, like the chat it came from; it runs in that
    // worktree unless it asked for one of its own.
    let mut in_worktree = None;
    if let Some(project) = params.folder.as_deref().and_then(crate::worktree::project_of) {
        let wt = params.folder.replace(project);
        if !params.worktree.unwrap_or(false) {
            in_worktree = wt;
        }
    }
    for dir in params.folder.iter().chain(in_worktree.iter()).filter(|d| !d.trim().is_empty()) {
        if reaches_luna(dir) {
            return Err(format!(
                "{dir} holds or is inside Luna's own folder (its settings, logs and the CLIs it runs): no helper works there"
            ));
        }
    }
    let allowed = allowed_account_dirs();
    let (one, list) = match params.account.take() {
        Some(AccountAsk::One(name)) => (Some(name), None),
        Some(AccountAsk::List(names)) => (None, Some(names)),
        None => (None, None),
    };
    let provider = match params.provider.as_deref() {
        // An account runs one provider only: one named that the
        // caller's provider has no account of brings its own.
        None => match one.as_deref() {
            Some(name) if !allowed.iter().any(|a| a.provider == me.provider && a.name == name) => {
                allowed.iter().find(|a| a.name == name).map_or(me.provider, |a| a.provider)
            }
            _ => me.provider,
        },
        Some("claude") => Provider::Claude,
        Some("codex") => Provider::Codex,
        Some(other) => return Err(format!("unknown provider {other}; use claude or codex")),
    };
    let provider = match &list {
        Some(names) => list_provider(names, &allowed, params.provider.is_some().then_some(provider), me.provider)?,
        None => provider,
    };
    settle_role(&mut params, provider)?;
    params.provider = Some(provider.as_str().to_string());
    let picked = match &list {
        Some(names) => {
            // A copy, and off the lock: choosing reads login files.
            let usage = reg().lock().unwrap_or_else(|e| e.into_inner()).usage.clone();
            Some(pick_from_list(names, provider, &allowed, &usage, |a| signed_in(provider, &a.path), now_ms())?)
        }
        None => None,
    };
    let mut ours = allowed.iter().filter(|a| a.provider == provider);
    let pick = match (&picked, one.as_deref()) {
        (Some((a, _)), _) => Some(a),
        (None, Some(name)) => ours.find(|a| a.name == name),
        // The caller's own account, else one somebody signed in to, else
        // any: the user said which accounts agents may use, not which
        // of them works.
        (None, None) => allowed
            .iter()
            .find(|a| a.provider == provider && provider == me.provider && a.name == me.account)
            .or_else(|| allowed.iter().find(|a| a.provider == provider && signed_in(provider, &a.path)))
            .or_else(|| ours.next()),
    };
    let Some(account) = pick.cloned() else {
        return Err(match one.as_deref() {
            Some(name) => {
                format!("no {} account named {name:?} is available to agents; luna_account_list shows the ones that are", provider.as_str())
            }
            None => format!("no {} account is available to agents", provider.as_str()),
        });
    };
    params.account = Some(AccountAsk::One(account.name.clone()));
    // The dev caller stands in for the user (`register_dev_caller`): no cap.
    let above = caller == DEV_CALLER || crate::settings::get().helpers_above_caller;
    let run_in = in_worktree.clone().or_else(|| params.folder.clone()).unwrap_or_default();
    // A folder the helper's CLI could not start in (pty/folder.rs), said to
    // the caller now rather than as a first turn that fails.
    if let Some(why) = crate::pty::folder_refusal(provider, &run_in, &account.path, params.worktree.unwrap_or(false)) {
        return Err(why);
    }
    settle_mode(&mut params, provider, (me.provider, &me.settings), &account.path, &run_in, above)?;
    // A helper's CLI takes its opening prompt as it takes text sent later,
    // a command among it (`command_reach`); it is its caller's own.
    let approver = (!above).then(|| Privilege::of_caller(me.provider, &me.settings));
    command_reach(&params.prompt, caller == DEV_CALLER, true, approver)?;
    let account_choice = picked.map(|(_, choice)| choice);
    let taken = SpawnUnderWay::begin(caller, &params, &account.path, in_worktree.as_deref(), replaces)?;
    spawn_background(caller, params, account.path, in_worktree, taken.0).map(|s| Spawned { account_choice, ..s })
}

/// The helper a spawn came to, as soon as it is registered and before its CLI is
/// started: what the spawn asked of it that `register` does not carry goes on it
/// now, as a core may go before the spawn gets any further, and from here on
/// `take_up_spawns` finds it. Refused for a spawn given up on (`settle_spawns`),
/// whose caller has been or is being told so: the helper goes unstarted rather
/// than a second one run or what was said prove false.
pub(super) fn spawn_is(request: u64, id: &str) -> Result<(), String> {
    {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let Some(s) = r.spawns.get_mut(&request).filter(|s| !s.settled) else {
            r.known.remove(id);
            r.tokens.retain(|_, chat| chat != id);
            return Err("Luna's core has since given up on this spawn".into());
        };
        s.helper = Some(id.to_string());
        let task = first_words(&s.params.prompt, TASK_CHARS);
        // Codex names no worktree: what is new since is all that tells what a core
        // may have cut it off making (`report_codex_worktrees`).
        let codex = s.params.provider.as_deref() == Some(Provider::Codex.as_str());
        let before = (s.params.worktree == Some(true) && codex).then(|| s.worktrees_before.clone());
        if let Some(task) = task {
            r.tasks.insert(id.to_string(), task);
        }
        if let Some(before) = before {
            r.worktrees_before.insert(id.to_string(), before);
        }
    }
    crate::background::save();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::persist::{chats_now, ties_in};
    use crate::agents::registry::{reg, register};
    use crate::agents::teardown::forget;

    #[test]
    fn account_comes_as_a_name_or_a_list_and_goes_out_as_a_name() {
        let p: SpawnParams =
            serde_json::from_value(serde_json::json!({ "prompt": "x", "account": "a", "readOnly": true, "agent": "checker" })).unwrap();
        assert_eq!(p.account_name(), Some("a"));
        assert_eq!(p.read_only, Some(true));
        assert_eq!(p.agent.as_deref(), Some("checker"));
        let p: SpawnParams = serde_json::from_value(serde_json::json!({ "prompt": "x", "account": ["a", "b"] })).unwrap();
        assert!(matches!(p.account, Some(AccountAsk::List(ref l)) if l.len() == 2));
        assert_eq!(p.account_name(), None, "not settled yet");
        let p = SpawnParams { account: Some(AccountAsk::One("a".into())), ..Default::default() };
        let out = serde_json::to_value(&p).unwrap();
        assert_eq!(out["account"], "a", "a record on disk reads a string");
    }

    #[test]
    fn a_spawn_is_on_disk_for_as_long_as_it_is_under_way() {
        let params: SpawnParams =
            serde_json::from_value(serde_json::json!({ "prompt": "go", "resumeOnRestart": true, "autoDelete": true })).unwrap();
        let saved = |request: u64| chats_now().spawns.into_iter().find(|s| s.request == request);
        let under_way = SpawnUnderWay::begin("uw-caller", &params, "C:\\acc\\anthropic\\one", Some("C:\\p\\wt"), None).unwrap();
        let request = under_way.0;
        let s = saved(request).unwrap();
        assert_eq!((s.caller.as_str(), s.params.prompt.as_str(), s.in_worktree.as_deref()), ("uw-caller", "go", Some("C:\\p\\wt")));
        assert!(s.resume_on_restart && s.auto_delete == Some(true));
        // Its helper registers, and its task goes on it.
        register("uw-helper", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("uw-caller"), false, serde_json::json!({}));
        spawn_is(request, "uw-helper").unwrap();
        assert_eq!(saved(request).unwrap().helper.as_deref(), Some("uw-helper"));
        assert_eq!(ties_in(&reg().lock().unwrap(), "uw-helper").unwrap().task.as_deref(), Some("go"));
        drop(under_way);
        assert!(saved(request).is_none(), "seen through, nothing is left to tell");
        // A record a core of an earlier version wrote, a visible spawn's marks and
        // all, still reads.
        let old = serde_json::json!({
            "request": 7, "caller": "c", "params": { "prompt": "go", "visible": true }, "autoDelete": null,
            "resumeOnRestart": false, "visible": true, "inWorktree": null, "accountPath": "a", "worktreesBefore": [], "helper": null
        });
        let back: SpawnTaken = serde_json::from_value(old).expect("an older chat-ties.json still reads");
        assert_eq!(back.params.prompt, "go");
        forget("uw-helper");
    }

    /// The cap is the top chat's, every generation and every spawn under way
    /// counted, in the hold that records a spawn.
    #[test]
    fn the_cap_counts_all_under_the_top_chat_and_the_spawns_under_way() {
        let one = |id: &str, parent: Option<&str>| {
            register(id, Provider::Claude, "C:\\no\\such\\folder", "C:\\no\\such\\account", parent, false, serde_json::json!({}));
        };
        let ids: Vec<String> = std::iter::once("cap-tools".to_string()).chain((0..6).map(|i| format!("cap-kid-{i}"))).collect();
        one("cap-top", None);
        for id in &ids {
            one(id, Some("cap-top"));
        }
        let room = |caller: &str, except: Option<u64>| cap_room(&reg().lock().unwrap(), caller, except).is_ok();
        assert!(room("cap-top", None) && room("cap-tools", None), "seven under it");
        // A helper's own helper takes the last place, whoever asks next.
        one("cap-grandkid", Some("cap-tools"));
        assert!(!room("cap-top", None));
        assert!(!room("cap-tools", None), "its helpers count against the top chat");
        forget("cap-grandkid");
        // A spawn under way holds its place until it has a session.
        let params: SpawnParams = serde_json::from_value(serde_json::json!({ "prompt": "go" })).unwrap();
        let taken = SpawnUnderWay::begin("cap-tools", &params, "C:\\no\\such\\account", None, None).unwrap();
        assert!(SpawnUnderWay::begin("cap-top", &params, "C:\\no\\such\\account", None, None).is_err(), "the last place is taken");
        assert!(room("cap-top", Some(taken.0)), "asked for again, it stands in for itself");
        drop(taken);
        assert!(room("cap-top", None), "gone with its call");
        // Full again: the refusal names who holds the places.
        one("cap-grandkid", Some("cap-tools"));
        let e = cap_room(&reg().lock().unwrap(), "cap-top", None).unwrap_err();
        assert!(e.contains("cap-grandkid") && e.contains("cap-kid-0") && e.contains("delete those"), "{e}");
        // One whose answer reached its caller and that waits out autoDelete holds none.
        let auto = |released| crate::agents::teardown::Auto { idle_since: Some(1), deleting: false, released };
        reg().lock().unwrap().auto.insert("cap-kid-0".into(), auto(true));
        assert!(room("cap-top", None), "a finished helper is no place taken");
        // At work again, or not autoDelete, it counts.
        reg().lock().unwrap().auto.insert("cap-kid-0".into(), auto(false));
        assert!(!room("cap-top", None));
        reg().lock().unwrap().auto.remove("cap-kid-0");
        forget("cap-grandkid");
        for id in ids.iter().map(String::as_str).chain(["cap-top"]) {
            forget(id);
        }
    }

    #[test]
    fn a_session_starting_for_a_spawn_given_up_on_is_refused_so_what_was_said_stays_true() {
        let params: SpawnParams = serde_json::from_value(serde_json::json!({ "prompt": "go" })).unwrap();
        let under_way = SpawnUnderWay::begin("gu-caller", &params, "C:\\acc\\anthropic\\one", None, None).unwrap();
        reg().lock().unwrap().spawns.get_mut(&under_way.0).unwrap().settled = true;
        register("gu-late", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("gu-caller"), false, serde_json::json!({}));
        assert!(spawn_is(under_way.0, "gu-late").is_err(), "it does not start");
        assert!(!reg().lock().unwrap().known.contains_key("gu-late"), "and leaves nothing behind");
        assert!(!reg().lock().unwrap().tasks.contains_key("gu-late"));
        // A request nothing stands for any more is refused the same.
        register("gu-none", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("gu-caller"), false, serde_json::json!({}));
        assert!(spawn_is(1, "gu-none").is_err());
    }

    #[test]
    fn a_budget_is_lunas_and_a_spawn_of_nothing_is_refused() {
        let p: SpawnParams = serde_json::from_value(serde_json::json!({ "prompt": "go", "budgetTokens": 50000 })).unwrap();
        assert_eq!(p.budget_tokens, Some(50000));
        let p: SpawnParams = serde_json::from_value(serde_json::json!({ "prompt": "go", "budgetTokens": 0 })).unwrap();
        assert_eq!(spawn("nobody", p).err().unwrap(), "budgetTokens must be above 0");
    }

    #[test]
    fn a_spawns_prompt_is_plain_text_before_anything_else() {
        let p = SpawnParams { prompt: "go\x1b[201~\ry".into(), ..Default::default() };
        assert!(spawn("nobody", p).err().unwrap().contains("control character"));
    }
}
