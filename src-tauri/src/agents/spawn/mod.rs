//! `luna_spawn`: a request to the window for a visible chat, or a background
//! helper run here, its account, folder and privilege settled first. A spawn
//! under way is on disk until a helper comes of it (`SpawnUnderWay`).

pub(crate) mod account;
pub(super) mod background;
pub(super) mod worktree;

use crate::agents::ask::{ask_of, Ask};
use crate::agents::delivery::{await_start, screen_of, spill, still_starting, unknown_command};
use crate::agents::owed::{owe_from, shown};
use crate::agents::persist::Opening;
use crate::agents::privilege::{command_reach, plain_text, reaches_luna, settle_mode, settle_role, Privilege};
use crate::agents::registry::{chat_name, depth_of, descendants, is_descendant, label, reg, set_name, top_of, Registry, DEV_CALLER};
use crate::agents::said::{first_words, TASK_CHARS};
use crate::agents::spawn::account::{allowed_account_dirs, list_provider, pick_from_list, signed_in, AccountChoice};
use crate::agents::spawn::background::spawn_background;
use crate::agents::spawn::worktree::{caller_worktree, helper_worktree_name, neighbors_in, worktree_of, Neighbor, Worktree};
use crate::agents::spawn_failed_late;
use crate::agents::teardown::Auto;
use crate::provider::Provider;
use crate::throttle::now_ms;
use serde::{Deserialize, Serialize};
use std::sync::{Condvar, Mutex};

/// How long the frontend gets to make the chat and start its session.
const SPAWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);

/// The most a caller may spawn under itself, all generations counted.
const MAX_DESCENDANTS: usize = 8;

/// A child of a child may exist; a child of that may not have tools.
const MAX_TOOL_DEPTH: usize = 1;

/// How long a fresh session gets to start working on its opening prompt
/// before `spawn` answers without that assurance. Codex brings up its MCP
/// servers first, which can take a while; a session still not busy after
/// this is on a notice, a picker or an error, and the answer carries its
/// screen so the caller can see which — or has not drawn at all yet, and the
/// answer says `starting` instead.
const START_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// The most warning lines a spawn passes on from the helper's screen.
const MAX_WARNINGS: usize = 5;

/// What an agent asks for. Everything but the prompt may be left to the
/// defaults the frontend would apply to a chat made by hand.
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
    /// Luna's to act on (see `reap`); the frontend never sees it.
    #[serde(skip_serializing)]
    pub auto_delete: Option<bool>,
    /// A background helper a core's going stopped mid-turn starts its next
    /// turn by itself under the core after it (`background::resume_stopped`).
    /// Luna's to act on, kept in the settings it runs with.
    #[serde(skip_serializing)]
    pub resume_on_restart: Option<bool>,
    /// Context tokens past which its parent hears, once (`budget_news`). Luna's
    /// to act on, kept in the settings it runs with.
    #[serde(skip_serializing)]
    pub budget_tokens: Option<u64>,
    /// A chat the user sees, run by the window. Without it the helper runs
    /// in the background (background.rs) and the frontend never hears of it.
    #[serde(skip_serializing)]
    pub visible: Option<bool>,
    /// Without the edit tools (Claude Code); in a read-only sandbox (Codex).
    pub read_only: Option<bool>,
    /// The agent it runs as: Claude Code's `--agent`.
    pub agent: Option<String>,
}

/// `account` in a spawn: a name, or names to choose from (`pick_from_list`).
/// By the time the frontend hears of it, the one name taken.
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

/// The request as the frontend receives it.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpawnRequest {
    request_id: u64,
    parent_id: String,
    /// A worktree the helper is to run in without owning it: its caller's.
    /// The chat is filed under `folder`, the worktree's project.
    pub(super) in_worktree: Option<String>,
    /// Codex: run the project's hooks unasked, as the folder would start a
    /// chat of the user's (codex/hooks.rs). Not the helper's to choose.
    pub(super) hooks_unasked: bool,
    #[serde(flatten)]
    pub(super) params: SpawnParams,
}

#[derive(Default)]
pub(super) struct Pending {
    done: Option<Result<String, String>>,
}

/// A `luna_spawn` taken on, in `chat-ties.json` until a helper has come of it and
/// its caller is owed by that: a core going before then leaves the caller an error
/// from the call, or nothing at all, and this is what the next core tells it from.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SpawnTaken {
    pub(super) request: u64,
    pub(super) caller: String,
    /// What was asked, its account and folder settled.
    pub(super) params: SpawnParams,
    /// What `params` does not write, the window having no word on them.
    pub(super) auto_delete: Option<bool>,
    pub(super) resume_on_restart: bool,
    #[serde(default)]
    pub(super) budget_tokens: Option<u64>,
    pub(super) visible: bool,
    pub(super) in_worktree: Option<String>,
    pub(super) account_path: String,
    /// With `worktree`: the worktrees there were as it was taken on.
    pub(super) worktrees_before: Vec<String>,
    /// What the window was asked for, until a session has come of it (`agent_spawns`).
    /// Not saved: it means a request of this core's, and one a core left is settled.
    #[serde(skip)]
    pub(super) asked: Option<SpawnRequest>,
    /// The session it came to, as soon as there is one (`spawn_is`).
    pub(super) helper: Option<String>,
    /// That session owes its caller by now, whatever has become of the debt since,
    /// and no core after this is to make it owe again (`take_up_spawns`).
    #[serde(default)]
    pub(super) owed_placed: bool,
    /// Read from what the last core left: no call waits on it any more.
    #[serde(skip)]
    pub(super) left_by_core: bool,
    /// Given up on, as far as starting goes: a session the window is still trying
    /// to start for it is refused (`spawn_is`), so what its caller is told stays true.
    #[serde(skip)]
    pub(super) settled: bool,
    /// What its caller is to be told, once worked out (`settle_spawns`).
    #[serde(skip)]
    pub(super) news: Option<String>,
    /// Asked for again and not answered yet, so the later request stands for it and
    /// this has only its word to say: not saved, as two on disk would start two.
    #[serde(skip)]
    pub(super) retried: bool,
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
            visible: params.visible == Some(true),
            in_worktree: in_worktree.map(str::to_owned),
            account_path: account_path.to_string(),
            worktrees_before,
            asked: None,
            helper: None,
            owed_placed: false,
            left_by_core: false,
            settled: false,
            news: None,
            retried: false,
        };
        let request = {
            let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
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

/// How many sessions count against the cap of `caller`'s top chat: all under it,
/// every generation, and the spawns of any of them under way with no session yet,
/// `except` aside (the one asked for again).
fn under_cap(r: &Registry, caller: &str, except: Option<u64>) -> usize {
    let top = top_of(r, caller);
    let coming = r
        .spawns
        .values()
        .filter(|s| s.helper.is_none() && !s.settled && Some(s.request) != except)
        .filter(|s| s.caller == top || is_descendant(r, &top, &s.caller))
        .count();
    descendants(r, &top).len() + coming
}

/// Whether `caller` may spawn one more (MAX_DESCENDANTS).
fn cap_room(r: &Registry, caller: &str, except: Option<u64>) -> Result<(), String> {
    if under_cap(r, caller, except) < MAX_DESCENDANTS {
        return Ok(());
    }
    Err(if top_of(r, caller) == caller {
        format!("you already have {MAX_DESCENDANTS} sessions under you, spawns under way counted; delete some first")
    } else {
        format!(
            "the chat at the top of your line already has {MAX_DESCENDANTS} sessions under it, yours and the other helpers' \
             counted; delete some first"
        )
    })
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
    /// True when the session was seen working on the prompt (or already done
    /// with it) within START_TIMEOUT. False means the prompt has not been
    /// taken up: look at `screen`.
    pub started: bool,
    /// The terminal as it stands, only when `started` is false and the CLI
    /// has drawn something.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screen: Option<String>,
    /// What it stopped on instead, when Luna can read it: a dialog of the
    /// CLI's own, which `luna_answer` answers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ask: Option<Ask>,
    /// See `still_starting`: not stuck, just not up yet.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub starting: bool,
    /// What the CLI flagged on its screen as it came up — a model the
    /// organization does not allow and the one used instead — which the
    /// settings above, being what it was started with, cannot say.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// Other live sessions working in the same directory, the caller aside:
    /// they share its files and its git index, commits and all.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub same_folder: Vec<Neighbor>,
    /// The worktree it works in and its branch, once the CLI has made it.
    #[serde(flatten)]
    pub worktree: Worktree,
    /// Asked for a worktree of its own and the CLI has not said where yet:
    /// `luna_list` names it once it has.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub worktree_pending: bool,
    /// With `account` as a list: the one taken, and why not the ones before.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_choice: Option<AccountChoice>,
}

/// What a CLI marks as warnings on its screen (⚠), each once, put back on one
/// line: a helper no pane shows has a narrow terminal, and the CLI wraps the
/// rest of a warning onto indented lines below it.
fn warnings_of(pty: &crate::pty::PtyManager, id: &str) -> Vec<String> {
    warnings_in(&pty.screen(id).unwrap_or_default())
}

fn warnings_in(screen: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut lines = screen.lines().peekable();
    while let Some(line) = lines.next() {
        let line = line.trim();
        if !line.starts_with('⚠') {
            continue;
        }
        let mut warning = line.to_string();
        while let Some(more) = lines.peek().filter(|l| l.starts_with("  ") && !l.trim().is_empty()) {
            warning.push(' ');
            warning.push_str(more.trim());
            lines.next();
        }
        if !out.contains(&warning) && out.len() < MAX_WARNINGS {
            out.push(warning);
        }
    }
    out
}

/// Asks the frontend to make the chat and start its session; blocks until it
/// answers or the timeout passes.
pub fn spawn(caller: &str, params: SpawnParams) -> Result<Spawned, String> {
    spawn_with(caller, params, None, &mut false)
}

/// `spawn`. `replaces`: the record of the spawn this asks for again. `still_coming`:
/// set when the error is only that the window has not answered in time, the request
/// standing for as long as it may yet (`agent_spawned`): no failure to act on.
pub(super) fn spawn_with(caller: &str, mut params: SpawnParams, replaces: Option<u64>, still_coming: &mut bool) -> Result<Spawned, String> {
    if params.prompt.trim().is_empty() {
        return Err("prompt is required".into());
    }
    // A prompt kept for a resume is typed in later (`resume_words`).
    plain_text(&params.prompt)?;
    if params.budget_tokens == Some(0) {
        return Err("budgetTokens must be above 0".into());
    }
    // Off the registry lock: this reads the CLI's registry from disk. A
    // helper works where its caller does, worktree and all.
    let worktree = if params.folder.is_none() { caller_worktree(caller) } else { None };
    let (taken, slot, in_worktree, account_choice) = {
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
        // step behind it (activity.rs `sample` has the same care).
        if params.folder.is_none() {
            params.folder = Some(worktree.unwrap_or_else(|| me.folder.clone()));
        }
        // A folder inside a CLI-made worktree files the chat under the
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
                    format!("no {} account named {name:?} is available to agents; luna_list shows the ones that are", provider.as_str())
                }
                None => format!("no {} account is available to agents", provider.as_str()),
            });
        };
        params.account = Some(AccountAsk::One(account.name.clone()));
        // The dev caller stands in for the user (`register_dev_caller`): no cap.
        let above = caller == DEV_CALLER || crate::settings::get().helpers_above_caller;
        let run_in = in_worktree.clone().or_else(|| params.folder.clone()).unwrap_or_default();
        settle_mode(&mut params, provider, (me.provider, &me.settings), &account.path, &run_in, above)?;
        // A helper's CLI takes its opening prompt as it takes text sent later,
        // a command among it (`command_reach`); it is its caller's own.
        let approver = (!above).then(|| Privilege::of_caller(me.provider, &me.settings));
        command_reach(&params.prompt, caller == DEV_CALLER, true, approver)?;
        let account_choice = picked.map(|(_, choice)| choice);
        let taken = SpawnUnderWay::begin(caller, &params, &account.path, in_worktree.as_deref(), replaces)?;
        if !params.visible.unwrap_or(false) {
            return spawn_background(caller, params, account.path, in_worktree, taken.0).map(|s| Spawned { account_choice, ..s });
        }
        let slot = std::sync::Arc::new((Mutex::new(Pending::default()), Condvar::new()));
        reg().lock().unwrap_or_else(|e| e.into_inner()).pending.insert(taken.0, slot.clone());
        (taken, slot, in_worktree, account_choice)
    };
    let request_id = taken.0;

    let borrowed = in_worktree.is_some();
    let auto_delete = params.auto_delete.unwrap_or(false);
    let own_worktree = params.worktree.unwrap_or(false);
    let folder = params.folder.clone().unwrap_or_default();
    // Where it will work: a worktree of its own shares nothing.
    let run_dir = (!own_worktree).then(|| in_worktree.clone().unwrap_or_else(|| folder.clone()));
    let (provider, account, prompt) =
        (params.provider.clone().unwrap_or_default(), params.account_name().unwrap_or_default().to_string(), params.prompt.clone());
    // A command line holds some 32 thousand characters on Windows.
    params.prompt = if provider == Provider::Codex.as_str() {
        spill(&crate::pty::codex_helper_prompt(caller, &params.prompt))
    } else {
        spill(&params.prompt)
    };
    let hooks_unasked = provider == Provider::Codex.as_str() && crate::codex::hooks::unasked_in(&folder);
    let asking = SpawnRequest { request_id, parent_id: caller.to_string(), in_worktree, hooks_unasked, params };
    if let Some(s) = reg().lock().unwrap_or_else(|e| e.into_inner()).spawns.get_mut(&request_id) {
        s.asked = Some(asking.clone());
    }
    crate::emit::to_ui("agent://spawn", asking);

    let (lock, cv) = &*slot;
    let mut p = lock.lock().unwrap_or_else(|e| e.into_inner());
    let deadline = std::time::Instant::now() + SPAWN_TIMEOUT;
    while p.done.is_none() {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            break;
        }
        let (guard, _) = cv.wait_timeout(p, left).unwrap_or_else(|e| e.into_inner());
        p = guard;
    }
    let mut outcome = p.done.take();
    drop(p);
    {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        r.pending.remove(&request_id);
        if outcome.is_none() {
            // The frontend may still make the chat (a slow trust write, a
            // worktree): `agent_spawned` then hands it to the caller.
            r.abandoned.insert(request_id, (caller.to_string(), borrowed));
        }
    }
    if outcome.is_none() {
        // An answer that landed between the timeout and the line above.
        outcome = lock.lock().unwrap_or_else(|e| e.into_inner()).done.take();
        if outcome.is_some() {
            reg().lock().unwrap_or_else(|e| e.into_inner()).abandoned.remove(&request_id);
        }
    }
    let id = match outcome {
        Some(Ok(id)) => {
            if borrowed {
                reg().lock().unwrap_or_else(|e| e.into_inner()).borrowed.insert(id.clone());
            }
            id
        }
        Some(Err(e)) => return Err(e),
        None => {
            // Still under way, for as long as the window may answer it.
            std::mem::forget(taken);
            *still_coming = true;
            return Err(format!(
                "the app did not start the session within {}s. It may still come up: luna_list will show it as yours \
                 and Luna will tell you when it answers, so check luna_list before spawning again. Helpers are made by \
                 Luna's window: if it is closed, they start once the user opens it",
                SPAWN_TIMEOUT.as_secs()
            ));
        }
    };
    // The frontend has registered the session by now (ensure_* ran before it
    // answered), so its settings are known; whether it took the prompt is
    // what the wait below finds out.
    let settings =
        reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(&id).map(|k| k.settings.clone()).unwrap_or(serde_json::Value::Null);
    if auto_delete {
        reg().lock().unwrap_or_else(|e| e.into_inner()).auto.insert(id.clone(), Auto::default());
    }
    crate::agentlog::spawned(&crate::agentlog::Spawn {
        id: &id,
        parent: caller,
        parent_name: chat_name(caller),
        folder: &folder,
        worktree: own_worktree,
        provider: &provider,
        account: &account,
        settings: &settings,
        name: chat_name(&id),
        prompt: &prompt,
        auto_delete,
        background: false,
    });
    let pty = crate::pty::manager();
    let started = await_start(pty, &id, &prompt, 0, START_TIMEOUT);
    let starting = !started && still_starting(pty, &id);
    if starting {
        crate::log::info(
            "agents",
            &format!("session {} spawned by {} has drawn nothing within {START_TIMEOUT:?}; still starting", label(&id), label(caller)),
        );
    } else if !started && unknown_command(pty, &id, &prompt) {
        crate::log::info(
            "agents",
            &format!("session {} spawned by {} does not know the command its prompt opens with", label(&id), label(caller)),
        );
    } else if !started {
        crate::log::warn(
            "agents",
            &format!("session {} spawned by {} did not start on its prompt within {START_TIMEOUT:?}", label(&id), label(caller)),
        );
    }
    let screen = (!started && !starting).then(|| screen_of(pty, &id)).flatten();
    let ask = screen.is_some().then(|| ask_of(&id)).flatten();
    // A screen handed over says it all already.
    let warnings = if screen.is_none() { warnings_of(pty, &id) } else { Vec::new() };
    let same_folder = run_dir.map(|dir| neighbors_in(&dir, &[caller, &id])).unwrap_or_default();
    shown(caller, &id, 0);
    owe_from(caller, &id, 0, 0);
    let worktree = worktree_of(pty, &id);
    let worktree_pending = own_worktree && worktree.path.is_none();
    Ok(Spawned {
        id,
        turns_ended: 0,
        settings,
        started,
        screen,
        ask,
        starting,
        warnings,
        same_folder,
        worktree,
        worktree_pending,
        account_choice,
    })
}

/// The session a spawn came to, as soon as it is registered and before its CLI is
/// started: what the spawn asked of it that the window does not carry goes on it now,
/// as a core may go before the spawn gets any further, and from here on
/// `take_up_spawns` finds it. Refused for a spawn given up on (`settle_spawns`), or
/// asked for again since, whose caller has been or is being told so: the session
/// goes unstarted and the window drops its row, rather than a second helper run or
/// what was said prove false.
pub fn spawn_is(request: u64, id: &str) -> Result<(), String> {
    {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let Some(s) = r.spawns.get_mut(&request).filter(|s| !s.settled) else {
            r.known.remove(id);
            r.tokens.retain(|_, chat| chat != id);
            return Err("Luna's core has since given up on this spawn".into());
        };
        s.helper = Some(id.to_string());
        let (resumes, borrowed, auto) = (s.resume_on_restart, s.in_worktree.is_some(), s.auto_delete == Some(true));
        let budget = s.budget_tokens;
        // A background helper keeps its own (`Entry::opening`).
        let opening = s.visible.then(|| s.params.prompt.clone());
        let own_worktree = s.params.worktree == Some(true);
        let codex = s.params.provider.as_deref() == Some(Provider::Codex.as_str());
        let name = (own_worktree && !codex && s.visible).then(|| helper_worktree_name(id));
        let before = (own_worktree && codex).then(|| s.worktrees_before.clone());
        let task = first_words(&s.params.prompt, TASK_CHARS);
        if borrowed {
            r.borrowed.insert(id.to_string());
        }
        if auto {
            r.auto.entry(id.to_string()).or_default();
        }
        if resumes {
            if let Some(k) = r.known.get_mut(id) {
                k.settings["resumeOnRestart"] = serde_json::json!(true);
            }
        }
        if let (Some(budget), Some(k)) = (budget, r.known.get_mut(id)) {
            k.settings["budgetTokens"] = serde_json::json!(budget);
        }
        if let Some(text) = opening {
            r.openings.insert(id.to_string(), Opening { text, floor: 0, lost: false, at_ms: now_ms() });
        }
        if let Some(task) = task {
            r.tasks.insert(id.to_string(), task);
        }
        if let Some(name) = name {
            r.worktree_names.insert(id.to_string(), name);
        }
        if let Some(before) = before {
            r.worktrees_before.insert(id.to_string(), before);
        }
    }
    crate::background::save();
    Ok(())
}

/// A visible helper's CLI is starting on no prompt of its own. With a conversation
/// to go back to (`conversation`) that holds what it was asked and the CLI knows
/// where it works. With none, its first turn having gone with a core, nothing has
/// what it was asked but `Opening`, and its caller is to get that back.
pub fn chat_starting(id: &str, conversation: bool) {
    let count = crate::activity::turns_counted(id).unwrap_or(0);
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    if conversation {
        r.worktrees_before.remove(id);
    } else if let Some(o) = r.openings.get_mut(id).filter(|o| o.floor >= 1 && count <= o.floor) {
        o.lost = true;
    }
}

/// What the window has been asked to start and nothing has come of yet. An event
/// reaches only a page that is listening: `agent://spawn` sent with no window, or
/// while one is still loading, is gone, and the spawn times out on a window that is
/// there. So the page asks for these as it comes up and at every core, and takes each
/// once (`bridge.ts`). The oldest first, in the order they were asked.
#[tauri::command]
pub fn agent_spawns() -> Vec<SpawnRequest> {
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let mut asked: Vec<SpawnRequest> =
        r.spawns.values().filter(|s| s.helper.is_none() && !s.settled).filter_map(|s| s.asked.clone()).collect();
    asked.sort_by_key(|s| s.request_id);
    asked
}

/// The frontend's answer to `agent://spawn`.
#[tauri::command]
pub fn agent_spawned(request_id: u64, chat_id: Option<String>, name: Option<String>, error: Option<String>) {
    if let (Some(id), Some(n)) = (&chat_id, &name) {
        set_name(id, n);
    }
    // The answer goes in under the registry lock, which `spawn` takes to
    // give up on it: either it is in before the caller stops waiting, or the
    // request is found abandoned here.
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let Some(slot) = r.pending.get(&request_id).cloned() else {
        let caller = r.abandoned.remove(&request_id);
        if let (Some((_, true)), Some(id)) = (&caller, &chat_id) {
            r.borrowed.insert(id.clone());
        }
        // One the last core had made a session of owes already (`take_up_spawns`),
        // and what it said is not to be started over.
        let owed = chat_id.as_ref().zip(caller.as_ref()).is_some_and(|(id, (c, _))| r.owed.contains_key(&(c.clone(), id.clone())));
        // A failure of one the last core left is still news to its caller, who has
        // heard nothing: `settle_spawns` tells it. One that had timed out was told it
        // may yet come up, and is told now that it did not.
        let mut failed = None;
        if chat_id.is_some() {
            r.spawns.remove(&request_id);
        } else if let Some(s) = r.spawns.get_mut(&request_id).filter(|s| !s.left_by_core && !s.settled) {
            s.settled = true;
            failed = Some(s.clone());
        }
        drop(r);
        if let Some(s) = failed {
            let why = error.clone().unwrap_or_else(|| "spawn failed".into());
            let _ = std::thread::Builder::new().name("luna-spawn-failed".into()).spawn(move || spawn_failed_late(s, &why));
        }
        // Too late for the caller's spawn, not for the caller: the session
        // registered as its child, so `luna_list` shows it, and the answer
        // to its prompt is owed like any other.
        if let (Some((caller, _)), Some(id)) = (caller, chat_id) {
            crate::log::warn("agents", &format!("session {} for {} came up after its spawn timed out", label(&id), label(&caller)));
            if !owed {
                shown(&caller, &id, 0);
                owe_from(&caller, &id, 0, 0);
            }
        }
        crate::background::save();
        return;
    };
    let (lock, cv) = &*slot;
    let mut p = lock.lock().unwrap_or_else(|e| e.into_inner());
    p.done = Some(match (chat_id, error) {
        (Some(id), _) => Ok(id),
        (None, Some(e)) => Err(e),
        (None, None) => Err("spawn failed".into()),
    });
    cv.notify_all();
    drop(p);
    drop(r);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::persist::{chats_now, ties_in, SESSION_RETURN_MS};
    use crate::agents::registry::{reg, register, resumes_on_restart};
    use crate::agents::teardown::forget;
    use crate::agents::tests::{spawn_taken, spawns_to_myself, take_up};
    use crate::agents::{settle_spawns, spawn_news};

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
        let p = SpawnParams {
            account: Some(AccountAsk::One("a".into())),
            read_only: Some(true),
            agent: Some("checker".into()),
            ..Default::default()
        };
        let out = serde_json::to_value(&p).unwrap();
        assert_eq!(out["account"], "a", "the frontend reads a string");
        assert_eq!(out["readOnly"], true);
        assert_eq!(out["agent"], "checker");
    }

    #[test]
    fn a_wrapped_warning_comes_back_whole() {
        let screen = " ▐▛███▛█   Claude Code\n\n⚠ Model \"haiku\" is restricted by your organization's\n  settings. Using claude-sonnet-5-5 instead.\n\n● Unknown command: /x\n";
        assert_eq!(
            warnings_in(screen),
            ["⚠ Model \"haiku\" is restricted by your organization's settings. Using claude-sonnet-5-5 instead."]
        );
        assert!(warnings_in("● all fine\n").is_empty());
    }

    #[test]
    fn a_visible_helper_may_resume_on_restart_and_the_window_never_hears_of_it() {
        let p: SpawnParams =
            serde_json::from_value(serde_json::json!({ "prompt": "go", "visible": true, "resumeOnRestart": true })).unwrap();
        assert!(spawn("nobody", p).err().unwrap().contains("not a registered session"));
        let p: SpawnParams = serde_json::from_value(serde_json::json!({ "prompt": "go", "resumeOnRestart": true })).unwrap();
        assert_eq!(p.resume_on_restart, Some(true));
        assert!(serde_json::to_value(&p).unwrap().get("resumeOnRestart").is_none(), "Luna's to act on, not the window's");
    }

    #[test]
    fn a_spawn_is_on_disk_for_as_long_as_it_is_under_way() {
        let params: SpawnParams =
            serde_json::from_value(serde_json::json!({ "prompt": "go", "visible": true, "resumeOnRestart": true, "autoDelete": true }))
                .unwrap();
        let saved = |request: u64| chats_now().spawns.into_iter().find(|s| s.request == request);
        let under_way = SpawnUnderWay::begin("uw-caller", &params, "C:\\acc\\anthropic\\one", Some("C:\\p\\wt"), None).unwrap();
        let request = under_way.0;
        let s = saved(request).unwrap();
        assert_eq!((s.caller.as_str(), s.params.prompt.as_str(), s.in_worktree.as_deref()), ("uw-caller", "go", Some("C:\\p\\wt")));
        assert!(s.resume_on_restart && s.visible && s.auto_delete == Some(true), "what the window never hears of");
        // Its session registers, and what the window does not carry goes on it.
        register("uw-helper", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("uw-caller"), false, serde_json::json!({}));
        spawn_is(request, "uw-helper").unwrap();
        assert_eq!(saved(request).unwrap().helper.as_deref(), Some("uw-helper"));
        assert!(resumes_on_restart("uw-helper"));
        {
            let r = reg().lock().unwrap();
            assert!(r.borrowed.contains("uw-helper") && r.auto.contains_key("uw-helper"));
            assert_eq!(ties_in(&r, "uw-helper").unwrap().opening.as_deref(), Some("go"), "as it was asked");
            assert_eq!(ties_in(&r, "uw-helper").unwrap().worktrees_before, None, "no worktree was asked for");
        }
        drop(under_way);
        assert!(saved(request).is_none(), "seen through, nothing is left to tell");
        forget("uw-helper");
    }

    #[test]
    fn what_the_window_was_asked_stays_there_to_ask_for_until_a_session_comes_of_it() {
        let _one = spawns_to_myself();
        let pending =
            || agent_spawns().into_iter().map(|s| s.request_id).filter(|r| (9_000_500..9_000_600).contains(r)).collect::<Vec<u64>>();
        let put = |request: u64| {
            let mut s = spawn_taken(request, "as-caller", None, false);
            s.asked = Some(SpawnRequest {
                request_id: request,
                parent_id: "as-caller".into(),
                in_worktree: None,
                hooks_unasked: false,
                params: s.params.clone(),
            });
            reg().lock().unwrap().spawns.insert(request, s);
        };
        put(9_000_502);
        put(9_000_501);
        // A background spawn asks the window nothing.
        reg().lock().unwrap().spawns.insert(9_000_503, spawn_taken(9_000_503, "as-caller", None, false));
        assert_eq!(pending(), [9_000_501, 9_000_502], "in the order they were asked");
        let wire = serde_json::to_value(agent_spawns().into_iter().find(|s| s.request_id == 9_000_501).unwrap()).unwrap();
        assert_eq!(
            (wire["requestId"].as_u64(), wire["parentId"].as_str(), wire["prompt"].as_str()),
            (Some(9_000_501), Some("as-caller"), Some("the whole job, word for word")),
            "as the event carries it"
        );

        // Its session registers: the window has it, whatever comes of the page.
        register("as-helper", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("as-caller"), false, serde_json::json!({}));
        spawn_is(9_000_501, "as-helper").unwrap();
        assert_eq!(pending(), [9_000_502]);
        // Given up on: a session would be refused, and its caller has been told.
        reg().lock().unwrap().spawns.get_mut(&9_000_502).unwrap().settled = true;
        assert!(pending().is_empty());
        // A core after this one settles it from the file, and asks for nothing.
        let saved = serde_json::to_vec(&reg().lock().unwrap().spawns[&9_000_501]).unwrap();
        let back: SpawnTaken = serde_json::from_slice(&saved).unwrap();
        assert!(back.asked.is_none());
        // The record gone, so is the asking.
        put(9_000_504);
        assert_eq!(pending(), [9_000_504]);
        drop(SpawnUnderWay(9_000_504));
        assert!(pending().is_empty());
        forget("as-helper");
        reg().lock().unwrap().spawns.retain(|request, _| !(9_000_500..9_000_600).contains(request));
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
        for id in ids.iter().map(String::as_str).chain(["cap-top"]) {
            forget(id);
        }
    }

    #[test]
    fn a_spawn_keeps_what_was_asked_with_or_without_the_flag_and_a_background_helper_its_own_way() {
        let spawned = |ask: serde_json::Value, id: &str| {
            let params: SpawnParams = serde_json::from_value(ask).unwrap();
            let taken = SpawnUnderWay::begin("ka-caller", &params, "C:\\no\\such\\account", None, None).unwrap();
            register(id, Provider::Codex, "C:\\no\\such\\folder", "C:\\no\\such\\account", Some("ka-caller"), false, serde_json::json!({}));
            spawn_is(taken.0, id).unwrap();
            let r = reg().lock().unwrap();
            ties_in(&r, id).unwrap()
        };
        // Claude Code's worktree goes by a name Luna gave it.
        let ties = spawned(
            serde_json::json!({ "prompt": "go", "visible": true, "worktree": true, "folder": "C:\\no\\such\\folder" }),
            "ka-visible",
        );
        assert_eq!(ties.opening.as_deref(), Some("go"));
        assert_eq!(ties.worktree_name.as_deref(), Some("luna-ka-visible"));
        assert_eq!(ties.worktrees_before, None);
        // Codex takes no name: what is new since is all that says what it began.
        let ties = spawned(
            serde_json::json!({ "prompt": "go", "visible": true, "worktree": true, "provider": "codex", "folder": "C:\\no\\such\\folder" }),
            "ka-codex",
        );
        assert_eq!(ties.worktree_name, None);
        assert_eq!(ties.worktrees_before, Some(Vec::new()), "none yet is a word too: all that come after may be its");
        let ties = spawned(serde_json::json!({ "prompt": "go", "worktree": true, "folder": "C:\\no\\such\\folder" }), "ka-background");
        assert_eq!(ties.opening, None, "`Entry::opening` has that");
        assert_eq!(ties.worktree_name, None, "its `Spec` has that");
        forget("ka-visible");
        forget("ka-codex");
        forget("ka-background");
    }

    #[test]
    fn the_window_finishing_a_spawn_with_the_next_core_owes_its_caller() {
        let _one = spawns_to_myself();
        register("wf-caller", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, serde_json::json!({}));
        take_up(
            Vec::new(),
            Vec::new(),
            vec![spawn_taken(9_000_200, "wf-caller", None, false), spawn_taken(9_000_201, "wf-caller", None, false)],
        );
        register("wf-helper", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("wf-caller"), false, serde_json::json!({}));
        agent_spawned(9_000_200, Some("wf-helper".into()), Some("named".into()), None);
        {
            let r = reg().lock().unwrap();
            assert!(!r.spawns.contains_key(&9_000_200), "seen through");
            assert!(r.owed.contains_key(&("wf-caller".to_string(), "wf-helper".to_string())));
        }
        // One it gives up on: nobody has told the caller yet.
        agent_spawned(9_000_201, None, None, Some("the core stopped".into()));
        assert!(reg().lock().unwrap().spawns.contains_key(&9_000_201));
        forget("wf-helper");
        forget("wf-caller");
    }

    #[test]
    fn a_session_starting_for_a_spawn_given_up_on_is_refused_so_what_was_said_stays_true() {
        let _one = spawns_to_myself();
        register("gu-caller", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, serde_json::json!({}));
        take_up(
            Vec::new(),
            Vec::new(),
            vec![spawn_taken(9_000_400, "gu-caller", None, false), spawn_taken(9_000_401, "gu-caller", None, false)],
        );
        // Started as it is given up on: decided under one lock, whichever comes first.
        register("gu-first", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("gu-caller"), false, serde_json::json!({}));
        spawn_is(9_000_400, "gu-first").unwrap();
        settle_spawns(SESSION_RETURN_MS);
        assert!(!reg().lock().unwrap().spawns.contains_key(&9_000_400), "before: that session is what came of it");
        register("gu-late", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("gu-caller"), false, serde_json::json!({}));
        assert!(spawn_is(9_000_401, "gu-late").is_err(), "after: it does not start");
        assert!(!reg().lock().unwrap().known.contains_key("gu-late"), "and leaves nothing behind");
        assert!(reg().lock().unwrap().spawns[&9_000_401].helper.is_none());
        // A request nothing stands for any more is refused the same.
        register("gu-none", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("gu-caller"), false, serde_json::json!({}));
        assert!(spawn_is(1, "gu-none").is_err());
        forget("gu-first");
        forget("gu-caller");
    }

    #[test]
    fn a_retry_still_coming_is_not_saved_beside_its_own_request_and_a_late_failure_is_told() {
        let _one = spawns_to_myself();
        register("rc-caller", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, serde_json::json!({}));
        let word = SpawnTaken {
            news: Some("asked again".into()),
            settled: true,
            retried: true,
            ..spawn_taken(9_000_500, "rc-caller", None, true)
        };
        reg().lock().unwrap().spawns.insert(9_000_500, word);
        assert!(!chats_now().spawns.iter().any(|s| s.request == 9_000_500), "two on disk would start two");
        assert!(spawn_news(1000)["rc-caller"].iter().any(|(request, _)| *request == 9_000_500), "its word is still said");
        // A spawn the caller was told may yet come up, and the window says it did not.
        let timed_out = spawn_taken(9_000_501, "rc-caller", None, false);
        {
            let mut r = reg().lock().unwrap();
            r.abandoned.insert(9_000_501, ("rc-caller".into(), false));
            r.spawns.insert(9_000_501, timed_out);
        }
        agent_spawned(9_000_501, None, None, Some("the folder is gone".into()));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let news = loop {
            if let Some(news) = reg().lock().unwrap().spawns[&9_000_501].news.clone() {
                break news;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        assert!(news.contains("did not come up after all: the folder is gone") && news.contains("Its prompt is in "), "{news}");
        let file = news.split("Its prompt is in ").nth(1).and_then(|l| l.split("; spawn it again").next()).unwrap();
        let _ = std::fs::remove_file(file);
        forget("rc-caller");
    }

    #[test]
    fn a_budget_is_lunas_and_a_spawn_of_nothing_is_refused() {
        let p: SpawnParams = serde_json::from_value(serde_json::json!({ "prompt": "go", "budgetTokens": 50000 })).unwrap();
        assert_eq!(p.budget_tokens, Some(50000));
        assert!(serde_json::to_value(&p).unwrap().get("budgetTokens").is_none(), "the window never hears of it");
        let p: SpawnParams = serde_json::from_value(serde_json::json!({ "prompt": "go", "budgetTokens": 0 })).unwrap();
        assert_eq!(spawn("nobody", p).err().unwrap(), "budgetTokens must be above 0");
        // A spawn a core cut off is asked for again with its budget.
        let s = SpawnTaken { budget_tokens: Some(9), ..spawn_taken(9001, "b-caller", None, true) };
        let back: SpawnTaken = serde_json::from_slice(&serde_json::to_vec(&s).unwrap()).unwrap();
        assert_eq!(back.budget_tokens, Some(9));
    }

    #[test]
    fn a_spawns_prompt_is_plain_text_before_anything_else() {
        let p = SpawnParams { prompt: "go\x1b[201~\ry".into(), ..Default::default() };
        assert!(spawn("nobody", p).err().unwrap().contains("control character"));
    }
}
