//! `luna_session_spawn`: a helper run in the background here, in the core, its account,
//! folder and role settled first. A spawn under way is on disk until a
//! helper comes of it (`SpawnUnderWay`).

pub(crate) mod account;
pub(super) mod background;
pub(super) mod folder;
mod role;
pub(super) mod settle;

use crate::agents::admission::{plain_text, reaches_luna};
use crate::agents::registry::{depth_of, descendants, is_descendant, reg, top_of, Registry, DEV_CALLER};
use crate::agents::said::{first_words, TASK_CHARS};
use crate::agents::spawn::account::{allowed_account_dirs, list_provider, pick_from_list, signed_in, AccountChoice};
#[cfg(test)]
pub(crate) use crate::agents::spawn::background::background_spec_for_test;
use crate::agents::spawn::background::spawn_background;
use crate::agents::spawn::folder::{caller_worktree, Neighbor};
use crate::agents::spawn::role::settle_role;
use crate::provider::Provider;
use crate::throttle::now_ms;
use serde::{Deserialize, Serialize};

/// How many sessions under a line's top chat may be at work at once, all
/// generations counted, unless the person sets otherwise
/// (`Settings::max_helpers`), and the range they may set it in: each place is
/// a turn running, its CLI process and its tokens. A value past the range, in
/// a file written by hand, is held to it where it is read.
pub(crate) const HELPERS_DEFAULT: u32 = 16;
pub(crate) const HELPERS_RANGE: std::ops::RangeInclusive<u32> = 1..=64;

/// The most sessions one line may have at work under its top chat, as the
/// person set it, held to `HELPERS_RANGE` whatever the file says. Reads the
/// settings file's stamp: called before the registry is held.
pub(in crate::agents) fn max_descendants() -> usize {
    crate::settings::get().max_helpers.clamp(*HELPERS_RANGE.start(), *HELPERS_RANGE.end()) as usize
}

/// Whether a session's lifecycle record (`activity::Life`, ADR-0031) has it
/// take a place: a turn running or held on a question, or a state not known
/// (no record yet, or one no source has spoken for), as unknown is active
/// everywhere else. A turn over takes none, whatever it left running beside it
/// (shells, crons, a job that would not list): no turn of its CLI runs, and
/// what it left is the power rule's to weigh, not the line's.
pub(in crate::agents) fn takes_a_place(life: Option<&crate::activity::Life>) -> bool {
    life.is_none_or(|l| l.phase != crate::activity::Phase::Ended)
}

/// Which sessions take a place (`takes_a_place`), each read off its record
/// before the registry is held, as every reader of a record does: a record is
/// behind the helpers' lock and the activity lock. A session not read (one
/// registered since) takes one.
#[derive(Default)]
pub(in crate::agents) struct AtWork(std::collections::HashMap<String, bool>);

impl AtWork {
    /// Every registered session's record, as read now.
    pub(in crate::agents) fn now() -> AtWork {
        let ids: Vec<String> = reg().lock().unwrap_or_else(|e| e.into_inner()).known.keys().cloned().collect();
        AtWork(
            ids.into_iter()
                .map(|id| {
                    let place = takes_a_place(crate::activity::life(&id).as_ref());
                    (id, place)
                })
                .collect(),
        )
    }

    fn takes_a_place(&self, id: &str) -> bool {
        self.0.get(id).copied().unwrap_or(true)
    }

    /// A read with these sessions in these phases, the rest not read.
    #[cfg(test)]
    pub(in crate::agents) fn of(phases: &[(&str, crate::activity::Phase)]) -> AtWork {
        let at = |phase| takes_a_place(Some(&crate::activity::Life { phase, ..Default::default() }));
        AtWork(phases.iter().map(|(id, phase)| (id.to_string(), at(*phase))).collect())
    }
}

/// How long a send that sets an idle session to work holds a place for it
/// (`Registry::set_to_work`) before its record alone decides: long enough for
/// the turn it starts to show in a record read after it, which one read before
/// it (`AtWork`) cannot.
pub(in crate::agents) const SET_TO_WORK_MS: u64 = 60_000;

/// A child of a child may exist; a child of that may not have tools.
const MAX_TOOL_DEPTH: usize = 1;

/// Whether a session `depth` below its top chat, with Luna's `tools` or
/// without, is one a spawn could have made there: one with tools only as a
/// chat's own helper, and none below a helper's helper, as only a session with
/// tools spawns. What a move (reparent.rs) may not go past.
pub(in crate::agents) fn spawnable_at(depth: usize, tools: bool) -> bool {
    depth <= MAX_TOOL_DEPTH + usize::from(!tools)
}

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
    /// Deprecated for `budget_pct`: context tokens, read as the share of its
    /// window they are once the window is known (`budget_news`). Kept in the
    /// settings it runs with, as records saved before `budgetPct` have it.
    #[serde(skip_serializing)]
    pub budget_tokens: Option<u64>,
    /// The share of its context window past which notices about it say where
    /// it stands (`budget_news`), 0 for never; none: the person's setting
    /// (`Settings::helper_context_pct`). Kept in the settings it runs with.
    #[serde(skip_serializing)]
    pub budget_pct: Option<u32>,
    /// The `requestKey` the spawn came with, which notices name its answer by
    /// (said.rs `Request`); the retry itself is the dispatcher's.
    #[serde(skip_serializing)]
    pub request_key: Option<String>,
    /// Without the edit tools (Claude Code); in a read-only sandbox (Codex).
    pub read_only: Option<bool>,
    /// The agent it runs as: Claude Code's `--agent`.
    pub agent: Option<String>,
    /// Keeps the MCP servers and connectors its account has (claude.ai's,
    /// Codex's apps), which a helper otherwise starts without.
    #[serde(skip_serializing)]
    pub connectors: Option<bool>,
    /// What the helper and every session under it are to keep to, added word
    /// for word to what its caller was given (`Constraint`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constraints: Option<String>,
}

/// One spawn's `constraints`, and the session whose spawn gave them. A
/// session's are its caller's, then its own spawn's: set once at the spawn,
/// never changed, and passed down whole to every session it starts. Text,
/// not enforcement: a session is told them as the last of its role layers
/// (library/role.rs), at every start and resume, and the mode it runs in
/// is what it was.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct Constraint {
    pub from: String,
    pub text: String,
}

/// The most one spawn's constraints may hold, and a whole chain's, in bytes:
/// all of it goes into every turn of every session under it, and on a Codex
/// command line (library/role.rs `CODEX_ROLE_MAX`).
pub const CONSTRAINT_MAX: usize = 4 * 1024;
pub const CONSTRAINTS_MAX: usize = 16 * 1024;

/// The constraints a session carries, as its registry settings keep them
/// (`"constraints"`); none where it has none or they will not read.
pub fn constraints_of(settings: &serde_json::Value) -> Vec<Constraint> {
    serde_json::from_value(settings["constraints"].clone()).unwrap_or_default()
}

/// The chain a spawn by `caller` gives its helper: the caller's own, then
/// what the spawn adds, checked and held to the caps.
pub(super) fn constraints_for(caller: &str, carried: Vec<Constraint>, added: Option<&str>) -> Result<Vec<Constraint>, String> {
    let mut chain = carried;
    if let Some(text) = added.map(str::trim).filter(|t| !t.is_empty()) {
        plain_text(text).map_err(|e| format!("constraints: {e}"))?;
        if text.len() > CONSTRAINT_MAX {
            return Err(format!("constraints come to {} bytes, over the {CONSTRAINT_MAX} one spawn may add", text.len()));
        }
        chain.push(Constraint { from: caller.to_string(), text: text.to_string() });
    }
    let total: usize = chain.iter().map(|c| c.text.len()).sum();
    if total > CONSTRAINTS_MAX {
        return Err(format!(
            "constraints come to {total} bytes with those the caller carries, over the {CONSTRAINTS_MAX} a chain may hold"
        ));
    }
    Ok(chain)
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
    pub(super) budget_pct: Option<u32>,
    #[serde(default)]
    pub(super) connectors: bool,
    pub(super) in_worktree: Option<String>,
    pub(super) account_path: String,
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
        let most = max_descendants();
        let work = AtWork::now();
        let taken = SpawnTaken {
            request: 0,
            caller: caller.to_string(),
            params: params.clone(),
            auto_delete: params.auto_delete,
            resume_on_restart: params.resume_on_restart == Some(true),
            budget_tokens: params.budget_tokens,
            budget_pct: params.budget_pct,
            connectors: params.connectors == Some(true),
            in_worktree: in_worktree.map(str::to_owned),
            account_path: account_path.to_string(),
            helper: None,
            owed_placed: false,
            left_by_core: false,
            settled: false,
            news: None,
        };
        let request = {
            let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
            if let Err(e) = cap_room(&r, &work, caller, replaces, most) {
                refused_a_place(&mut r, caller);
                return Err(e);
            }
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

/// Whether session `id` takes a place: at work by its record (`AtWork`), or
/// set to work by a send within `SET_TO_WORK_MS`; never an `autoDelete` helper
/// whose answer has reached its caller and that only waits out its time to go
/// (`Auto::idle_since`).
pub(in crate::agents) fn takes_place(r: &Registry, work: &AtWork, id: &str) -> bool {
    if r.auto.get(id).is_some_and(|a| a.released && !a.deleting) {
        return false;
    }
    work.takes_a_place(id) || r.set_to_work.get(id).is_some_and(|&at| now_ms().saturating_sub(at) < SET_TO_WORK_MS)
}

/// The sessions that take places on `caller`'s line (`takes_place`), by id:
/// those under its top chat, every generation, with a turn running or held on
/// a question or a state not known, none between turns; and how many spawns
/// of any of them are under way with no session yet, `except` aside (the one
/// asked for again).
pub(in crate::agents) fn under_cap(r: &Registry, work: &AtWork, caller: &str, except: Option<u64>) -> (Vec<String>, usize) {
    let top = top_of(r, caller);
    let coming = r
        .spawns
        .values()
        .filter(|s| s.helper.is_none() && !s.settled && Some(s.request) != except)
        .filter(|s| s.caller == top || is_descendant(r, &top, &s.caller))
        .count();
    let mut counted: Vec<String> = descendants(r, &top).into_iter().filter(|id| takes_place(r, work, id)).collect();
    counted.sort();
    (counted, coming)
}

/// Whether `caller` may spawn one more of the `most` its line may have at work
/// (`max_descendants`); refused, it hears which sessions take the places, and
/// how it can free one itself.
fn cap_room(r: &Registry, work: &AtWork, caller: &str, except: Option<u64>, most: usize) -> Result<(), String> {
    let (counted, coming) = under_cap(r, work, caller, except);
    if counted.len() + coming < most {
        return Ok(());
    }
    let coming = if coming > 0 { format!(", and {coming} spawn(s) under way") } else { String::new() };
    let top = top_of(r, caller);
    let full = if top == caller {
        format!("you already have {most} sessions at work under you: {}{coming}", counted.join(", "))
    } else {
        format!("your line already has {most} sessions at work under its top chat, {top}{coming}")
    };
    Err(format!("{full}. {AT_WORK}. {}", free_first(r, caller, &counted)))
}

/// What takes a place, as every refusal for want of one says it.
pub(in crate::agents) const AT_WORK: &str =
    "Only a session with a turn running or waiting on a question takes a place; one between turns takes none";

/// What a caller refused a place can do: of the sessions that take the places
/// (`counted`), those under it are the only ones it can free, by waiting for a
/// turn to end or stopping it. One below the top of its line hears that the
/// rest are its top chat's to free, which is told (`refused_a_place`).
pub(in crate::agents) fn free_first(r: &Registry, caller: &str, counted: &[String]) -> String {
    let own: Vec<&str> = counted.iter().filter(|id| is_descendant(r, caller, id)).map(String::as_str).collect();
    let own = if own.is_empty() { "none of them is under you".to_string() } else { format!("yours at work: {}", own.join(", ")) };
    let free = "wait for one of yours to end its turn (luna_session_wait), or stop one whose work you no longer need \
                (luna_session_stop), then try again";
    let top = top_of(r, caller);
    if top == caller {
        return format!("{free} ({own})");
    }
    format!("{free} ({own}). The rest are not yours to free; {top}, at the top of your line, is told")
}

/// A session below the top of its line refused a place: its top chat, the only
/// one that can free every place, hears of it once with its next notice, or
/// alone within a while (notifier `riders_go`). Refusals until then join the
/// one line; one after it was told starts another.
pub(in crate::agents) fn refused_a_place(r: &mut Registry, caller: &str) {
    let top = top_of(r, caller);
    if top == caller || top == DEV_CALLER {
        return;
    }
    let p = r.places_refused.entry(top).or_insert_with(|| PlacesRefused { since: now_ms(), who: Vec::new() });
    if !p.who.iter().any(|w| w == caller) {
        p.who.push(caller.to_string());
    }
}

/// Sessions below a top chat refused a place since it was last told, and since
/// when (`refused_a_place`).
#[derive(Clone, Debug)]
pub(in crate::agents) struct PlacesRefused {
    pub(in crate::agents) since: u64,
    pub(in crate::agents) who: Vec<String>,
}

impl PlacesRefused {
    /// The line its top chat's notice carries.
    pub(in crate::agents) fn line(&self) -> String {
        format!(
            "No place was left on your line for a helper of {} (luna_session_spawn or a send refused: every place is \
             taken by a session at work). Only you can free any of them: wait for a turn under you to end, or stop \
             one whose work is no longer needed (luna_session_stop).",
            self.who.join(", ")
        )
    }
}

/// Where a caller's line stands against its cap: the places in use under its
/// top chat — the sessions at work there, every generation, and every spawn
/// under way, counted as the cap counts them (`under_cap`); not the helpers
/// between turns — of how many there are, and how many of those are under the
/// caller itself, the only ones it can free.
#[derive(Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Places {
    pub in_use: usize,
    pub of: usize,
    pub under_you: usize,
}

pub(in crate::agents) fn places_in(r: &Registry, work: &AtWork, caller: &str, most: usize) -> Places {
    let (counted, coming) = under_cap(r, work, caller, None);
    let mine = |id: &str| id == caller || is_descendant(r, caller, id);
    let coming_mine = r.spawns.values().filter(|s| s.helper.is_none() && !s.settled && mine(&s.caller)).count();
    Places {
        in_use: counted.len() + coming,
        of: most,
        under_you: counted.iter().filter(|id| is_descendant(r, caller, id)).count() + coming_mine,
    }
}

/// `places_in` for `caller` now.
pub(in crate::agents) fn places(caller: &str) -> Places {
    let most = max_descendants();
    let work = AtWork::now();
    places_in(&reg().lock().unwrap_or_else(|e| e.into_inner()), &work, caller, most)
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
    /// With `account` as a list: the one taken, and why not the ones before.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_choice: Option<AccountChoice>,
    /// Where the caller's line stands against its cap, this helper counted.
    pub places: Places,
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
    if params.budget_pct.is_some_and(|pct| pct > 100) {
        return Err("budgetPct is a share of the context window: 0 (never) to 100".into());
    }
    // Off the registry lock: this reads the CLI's registry from disk. A
    // helper works where its caller does, in the worktree its CLI made for it
    // as well.
    let worktree = if params.folder.is_none() { caller_worktree(caller) } else { None };
    let most = max_descendants();
    let work = AtWork::now();
    let me = {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let me = r.known.get(caller).cloned().ok_or("caller is not a registered session")?;
        // Early, before folders and logins are read; `begin` counts again.
        if let Err(e) = cap_room(&r, &work, caller, replaces, most) {
            refused_a_place(&mut r, caller);
            return Err(e);
        }
        if params.tools.unwrap_or(false) && depth_of(&r, caller) >= MAX_TOOL_DEPTH {
            return Err("a session this deep cannot spawn sessions with tools of their own".into());
        }
        me
    };
    // What the caller carries, then what this spawn adds: nothing a caller
    // asks takes one off (`Constraint`).
    let constraints = constraints_for(caller, constraints_of(&me.settings), params.constraints.as_deref())?;
    // Off the registry lock: what follows reads folders and login files,
    // and every Claude Code session's hook waits on this lock with its next
    // step behind it (activity/sample.rs `sample` has the same care).
    if params.folder.is_none() {
        params.folder = Some(worktree.unwrap_or_else(|| me.folder.clone()));
    }
    // A folder inside a CLI-made worktree files the helper under the
    // worktree's project, like the chat it came from; it runs in that
    // worktree.
    let mut in_worktree = None;
    if let Some(project) = params.folder.as_deref().and_then(crate::checkout::project_of) {
        in_worktree = params.folder.replace(project);
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
    let run_in = in_worktree.clone().or_else(|| params.folder.clone()).unwrap_or_default();
    // A folder the helper's CLI could not start in (pty/folder.rs), said to
    // the caller now rather than as a first turn that fails.
    if let Some(why) = crate::pty::folder_refusal(&run_in, false) {
        return Err(why);
    }

    let account_choice = picked.map(|(_, choice)| choice);
    let taken = SpawnUnderWay::begin(caller, &params, &account.path, in_worktree.as_deref(), replaces)?;
    // Counted with its helper in, as `spawn_is` has it by now.
    spawn_background(caller, params, account.path, in_worktree, taken.0, constraints).map(|s| Spawned {
        account_choice,
        places: places(caller),
        ..s
    })
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
        if let Some(task) = task {
            r.tasks.insert(id.to_string(), task);
        }
    }
    crate::background::save();
    Ok(())
}

/// The helpers' settings the window shows: how many places a line has
/// (`max_descendants`), the range it may be set in, and the context share its
/// notices speak past.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HelperSettings {
    pub max_helpers: u32,
    pub min: u32,
    pub max: u32,
    /// `Settings::helper_context_pct`, 0 for off.
    pub helper_context_pct: u32,
}

/// The share of a helper's context window past which notices about it say
/// where it stands, unless the person sets otherwise: a reviewer with a small
/// window filled to 89% before its caller thought to rotate it.
pub const CONTEXT_PCT_DEFAULT: u32 = 50;

#[tauri::command(async)]
pub fn helper_settings() -> HelperSettings {
    let s = crate::settings::get();
    HelperSettings {
        max_helpers: s.max_helpers.clamp(*HELPERS_RANGE.start(), *HELPERS_RANGE.end()),
        min: *HELPERS_RANGE.start(),
        max: *HELPERS_RANGE.end(),
        helper_context_pct: s.helper_context_pct.min(100),
    }
}

/// Only what notices say: nothing a helper may do changes.
#[tauri::command]
pub fn set_helper_context_pct(pct: u32) -> Result<HelperSettings, String> {
    let pct = pct.min(100);
    crate::settings::update(|s| s.helper_context_pct = pct)?;
    crate::log::info("agents", &format!("helpers' context told of past: {pct}%"));
    Ok(helper_settings())
}

/// More places are more processes under one chat, each its CLI and its tokens.
#[tauri::command]
pub fn set_max_helpers(n: u32) -> Result<HelperSettings, String> {
    let n = n.clamp(*HELPERS_RANGE.start(), *HELPERS_RANGE.end());
    crate::settings::update(|s| s.max_helpers = n)?;
    crate::log::info("agents", &format!("places on a line: {n}"));
    Ok(helper_settings())
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
            "request": 7, "caller": "c", "params": { "prompt": "go", "visible": true, "worktree": true }, "autoDelete": null,
            "resumeOnRestart": false, "visible": true, "inWorktree": null, "accountPath": "a",
            "worktreesBefore": ["/p/.claude/worktrees/a"], "helper": null
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
        // As many as the default has places, but one: none of them read yet,
        // all of them at work.
        let ids: Vec<String> = std::iter::once("cap-tools".to_string()).chain((0..14).map(|i| format!("cap-kid-{i}"))).collect();
        one("cap-top", None);
        for id in &ids {
            one(id, Some("cap-top"));
        }
        let most = HELPERS_DEFAULT as usize;
        let room = |caller: &str, except: Option<u64>| cap_room(&reg().lock().unwrap(), &AtWork::default(), caller, except, most).is_ok();
        assert!(room("cap-top", None) && room("cap-tools", None), "fifteen under it");
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
        let e = cap_room(&reg().lock().unwrap(), &AtWork::default(), "cap-top", None, most).unwrap_err();
        assert!(e.contains("cap-grandkid") && e.contains("cap-kid-0") && e.contains("16 sessions at work"), "{e}");
        assert!(e.contains("luna_session_wait") && e.contains("luna_session_stop") && e.contains("between turns takes none"), "{e}");
        // One whose answer reached its caller and that waits out autoDelete holds none.
        let auto = |released| crate::agents::teardown::Auto { idle_since: Some(1), released, ..Default::default() };
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

    /// Below the top of its line a caller is told the places it holds and can
    /// free, nobody else's, and the top chat hears once that it was refused.
    #[test]
    fn a_lead_refused_a_place_names_only_its_own_and_its_top_chat_is_told_once() {
        let one = |id: &str, parent: Option<&str>| {
            register(id, Provider::Claude, "C:\\no\\such\\folder", "C:\\no\\such\\account", parent, false, serde_json::json!({}));
        };
        one("pl-top", None);
        for lead in ["pl-lead-a", "pl-lead-b"] {
            one(lead, Some("pl-top"));
        }
        one("pl-a1", Some("pl-lead-a"));
        one("pl-b1", Some("pl-lead-b"));
        one("pl-b2", Some("pl-lead-b"));
        // Five in use of six, none of them read yet: one under lead A, two under lead B.
        let places = |caller: &str| places_in(&reg().lock().unwrap(), &AtWork::default(), caller, 6);
        assert_eq!(places("pl-lead-a"), Places { in_use: 5, of: 6, under_you: 1 });
        one("pl-a2", Some("pl-lead-a"));
        assert_eq!(places("pl-lead-a"), Places { in_use: 6, of: 6, under_you: 2 });
        assert_eq!(places("pl-top"), Places { in_use: 6, of: 6, under_you: 6 }, "all under the top");
        let row = serde_json::to_value(Places { in_use: 6, of: 6, under_you: 2 }).unwrap();
        assert_eq!(row, serde_json::json!({ "inUse": 6, "of": 6, "underYou": 2 }));

        let refusal = |caller: &str| cap_room(&reg().lock().unwrap(), &AtWork::default(), caller, None, 6).unwrap_err();
        let e = refusal("pl-lead-a");
        assert!(e.contains("yours at work: pl-a1, pl-a2") && e.contains("pl-top"), "{e}");
        assert!(!e.contains("pl-b1") && !e.contains("pl-lead-b"), "another's are not its to free: {e}");
        let e = refusal("pl-b1");
        assert!(e.contains("none of them is under you"), "{e}");
        // The top chat hears of each refused below it, in one line, once.
        {
            let mut r = reg().lock().unwrap();
            refused_a_place(&mut r, "pl-lead-a");
            refused_a_place(&mut r, "pl-lead-a");
            refused_a_place(&mut r, "pl-b1");
            refused_a_place(&mut r, "pl-top");
        }
        let told = reg().lock().unwrap().places_refused.get("pl-top").cloned().unwrap();
        assert_eq!(told.who, ["pl-lead-a", "pl-b1"], "a refusal of its own is not news to it");
        assert!(told.line().contains("pl-lead-a, pl-b1") && told.line().contains("luna_session_stop"), "{}", told.line());
        reg().lock().unwrap().places_refused.remove("pl-top");
        for id in ["pl-a1", "pl-a2", "pl-b1", "pl-b2", "pl-lead-a", "pl-lead-b", "pl-top"] {
            forget(id);
        }
    }

    #[test]
    fn the_places_a_person_sets_are_held_to_their_range() {
        assert_eq!(HELPERS_DEFAULT, crate::settings::Settings::default().max_helpers);
        assert!(HELPERS_RANGE.contains(&HELPERS_DEFAULT));
        let read: crate::settings::Settings = serde_json::from_str(r#"{"maxHelpers": 0}"#).unwrap();
        assert_eq!(read.max_helpers.clamp(*HELPERS_RANGE.start(), *HELPERS_RANGE.end()), 1, "never none");
        let read: crate::settings::Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(read.max_helpers, 16, "a file that never set it");
        assert_eq!(HELPERS_RANGE, 1..=64);
        // A file written before the settings for helpers' modes went still reads.
        let read: crate::settings::Settings =
            serde_json::from_str(r#"{"helperMode": "account", "helpersAboveCaller": true, "maxHelpers": 3}"#).unwrap();
        assert_eq!(read.max_helpers, 3);
        // One set past the old ceiling of 32 by hand keeps its number; one past 64 is held to it.
        let held = |file: &str| {
            let read: crate::settings::Settings = serde_json::from_str(file).unwrap();
            read.max_helpers.clamp(*HELPERS_RANGE.start(), *HELPERS_RANGE.end())
        };
        assert_eq!(held(r#"{"maxHelpers": 40}"#), 40);
        assert_eq!(held(r#"{"maxHelpers": 64}"#), 64);
        assert_eq!(held(r#"{"maxHelpers": 500}"#), 64);
    }

    /// A place is a turn at work: running, held on a question, or not known;
    /// never a turn over, whatever it left running beside it.
    #[test]
    fn only_a_session_at_work_takes_a_place() {
        use crate::activity::{Background, Life, Phase};
        let life = |phase, background| Life { phase, background, ..Default::default() };
        let shells = Background { shells: 2, crons: 1, ..Default::default() };
        assert!(takes_a_place(Some(&life(Phase::Running, Background::default()))));
        assert!(takes_a_place(Some(&life(Phase::Asking, Background::default()))));
        assert!(takes_a_place(Some(&life(Phase::Unknown, Background::default()))), "not known counts");
        assert!(takes_a_place(None), "no record yet counts");
        assert!(!takes_a_place(Some(&life(Phase::Ended, Background::default()))));
        assert!(!takes_a_place(Some(&life(Phase::Ended, shells))), "what a turn left running takes none");
        let unlisted = Background { unlisted: true, ..Default::default() };
        assert!(!takes_a_place(Some(&life(Phase::Ended, unlisted))), "nor a job that would not list");

        // On a line: each by its phase, one not read as at work, spawns under way too.
        let one = |id: &str, parent: Option<&str>| {
            register(id, Provider::Claude, "C:\\no\\such\\folder", "C:\\no\\such\\account", parent, false, serde_json::json!({}));
        };
        one("ph-top", None);
        one("ph-lead", Some("ph-top"));
        for id in ["ph-run", "ph-ask", "ph-unknown", "ph-idle", "ph-left", "ph-unread"] {
            one(id, Some("ph-lead"));
        }
        let work = AtWork::of(&[
            ("ph-lead", Phase::Ended),
            ("ph-run", Phase::Running),
            ("ph-ask", Phase::Asking),
            ("ph-unknown", Phase::Unknown),
            ("ph-idle", Phase::Ended),
            ("ph-left", Phase::Ended),
        ]);
        let (counted, coming) = under_cap(&reg().lock().unwrap(), &work, "ph-lead", None);
        assert_eq!(counted, ["ph-ask", "ph-run", "ph-unknown", "ph-unread"]);
        assert_eq!(coming, 0);
        let places = places_in(&reg().lock().unwrap(), &work, "ph-lead", 5);
        assert_eq!(places, Places { in_use: 4, of: 5, under_you: 4 });
        assert!(cap_room(&reg().lock().unwrap(), &work, "ph-lead", None, 5).is_ok(), "one left");
        assert!(cap_room(&reg().lock().unwrap(), &work, "ph-lead", None, 4).is_err());
        // An idle helper set to work by a send takes one at once, whatever was read.
        reg().lock().unwrap().set_to_work.insert("ph-idle".into(), now_ms());
        assert_eq!(places_in(&reg().lock().unwrap(), &work, "ph-lead", 5).in_use, 5);
        // Once that has had its time, its record decides again.
        reg().lock().unwrap().set_to_work.insert("ph-idle".into(), now_ms() - SET_TO_WORK_MS);
        assert_eq!(places_in(&reg().lock().unwrap(), &work, "ph-lead", 5).in_use, 4);
        for id in ["ph-run", "ph-ask", "ph-unknown", "ph-idle", "ph-left", "ph-unread", "ph-lead", "ph-top"] {
            forget(id);
        }
        assert!(!reg().lock().unwrap().set_to_work.contains_key("ph-idle"), "gone with its session");
    }

    /// Idle helpers take no place, so a caller may have more of them than places
    /// and send to all at once: a send that would start a turn past the limit is
    /// refused, and one to a helper at work is not.
    #[test]
    fn a_send_that_would_start_a_turn_past_the_limit_is_refused() {
        use crate::agents::registry::authorize_send;
        let one = |id: &str, parent: Option<&str>| {
            register(id, Provider::Claude, "C:\\no\\such\\folder", "C:\\no\\such\\account", parent, false, serde_json::json!({}));
        };
        one("sl-top", None);
        // As many at work as the default has places, but one (none read yet), and two idle.
        let busy: Vec<String> = (0..HELPERS_DEFAULT - 1).map(|i| format!("sl-busy-{i}")).collect();
        for id in &busy {
            one(id, Some("sl-top"));
        }
        for id in ["sl-idle-1", "sl-idle-2"] {
            one(id, Some("sl-top"));
            crate::background::idle_for_test(id, Provider::Claude, 1, false);
        }
        assert!(authorize_send("sl-top", "sl-idle-1").is_ok(), "the last place");
        assert!(reg().lock().unwrap().set_to_work.contains_key("sl-idle-1"), "held for its turn");
        let e = authorize_send("sl-top", "sl-idle-2").err().unwrap();
        assert!(e.contains("between turns") && e.contains("nothing was queued") && e.contains("16 places"), "{e}");
        assert!(e.contains("yours at work") && e.contains("sl-idle-1"), "the one just set to work holds its place: {e}");
        assert!(!reg().lock().unwrap().set_to_work.contains_key("sl-idle-2"));
        // Text into one at work joins its turn, or waits for its next, in its place.
        assert!(authorize_send("sl-top", "sl-busy-0").is_ok());
        assert!(authorize_send("sl-top", "sl-idle-1").is_ok(), "set to work already");
        // A spawn finds the line as full.
        let work = AtWork::now();
        assert!(cap_room(&reg().lock().unwrap(), &work, "sl-top", None, HELPERS_DEFAULT as usize).is_err());
        for id in busy.iter().map(String::as_str).chain(["sl-idle-1", "sl-idle-2", "sl-top"]) {
            crate::background::gone_for_test(id);
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
    fn constraints_add_to_the_callers_and_none_is_ever_taken_off() {
        let one = |from: &str, text: &str| Constraint { from: from.into(), text: text.into() };
        // A chat of the person's carries none: the first spawn's are the chain.
        let lead = constraints_for("chat", Vec::new(), Some("  Touch nothing in src/.\n")).unwrap();
        assert_eq!(lead, [one("chat", "Touch nothing in src/.")]);
        // Its helper inherits them whole, its own after them.
        let helper = constraints_for("lead", lead.clone(), Some("No network.")).unwrap();
        assert_eq!(helper, [one("chat", "Touch nothing in src/."), one("lead", "No network.")]);
        // A spawn that adds nothing passes them on as they are; none it asks for removes any.
        assert_eq!(constraints_for("lead", lead.clone(), None).unwrap(), lead);
        assert_eq!(constraints_for("lead", lead.clone(), Some("  ")).unwrap(), lead);
        // Kept as the registry keeps them, and read back from there by a spawn of its own.
        let settings = serde_json::json!({ "model": "x", "constraints": helper });
        assert_eq!(constraints_of(&settings), helper);
        assert!(constraints_of(&serde_json::json!({})).is_empty(), "a session without any");
        assert!(constraints_of(&serde_json::json!({ "constraints": "loose text" })).is_empty(), "none it cannot read");
    }

    #[test]
    fn constraints_are_plain_text_within_their_caps() {
        assert!(constraints_for("c", Vec::new(), Some("ok\x1b[201~")).unwrap_err().contains("control character"));
        assert!(constraints_for("c", Vec::new(), Some("line one\nline two\ttabbed")).is_ok(), "lines and tabs are text");
        let big = "a".repeat(CONSTRAINT_MAX + 1);
        assert!(constraints_for("c", Vec::new(), Some(&big)).unwrap_err().contains("one spawn may add"));
        let full: Vec<Constraint> = (0..4).map(|i| Constraint { from: format!("s{i}"), text: "a".repeat(CONSTRAINT_MAX) }).collect();
        assert!(constraints_for("c", full.clone(), None).is_ok(), "16 KB in all is the most");
        assert!(constraints_for("c", full, Some("one more")).unwrap_err().contains("a chain may hold"));
    }

    #[test]
    fn a_callers_constraints_reach_its_helper_and_the_person_taking_it_over_drops_them() {
        let chain = vec![Constraint { from: "ct-top".into(), text: "No network.".into() }];
        register("ct-lead", Provider::Claude, "C:\\p", "C:\\acc", Some("ct-top"), true, serde_json::json!({ "constraints": chain }));
        let carried = constraints_of(&crate::agents::registry::settings_of("ct-lead"));
        assert_eq!(carried, chain, "what a spawn of its own starts from");
        // Registered again at its next start, as every turn's does: kept.
        register("ct-lead", Provider::Claude, "C:\\p", "C:\\acc", Some("ct-top"), true, serde_json::json!({}));
        assert_eq!(constraints_of(&crate::agents::registry::settings_of("ct-lead")), chain);
        // The person opens it as their own chat: a chat of the person's carries none.
        crate::agents::take_over("ct-lead", || None).unwrap();
        assert!(constraints_of(&crate::agents::registry::settings_of("ct-lead")).is_empty());
        forget("ct-lead");
    }

    #[test]
    fn a_spawn_record_of_before_constraints_still_reads() {
        let p: SpawnParams = serde_json::from_value(serde_json::json!({ "prompt": "go" })).unwrap();
        assert_eq!(p.constraints, None);
        let p: SpawnParams = serde_json::from_value(serde_json::json!({ "prompt": "go", "constraints": "No network." })).unwrap();
        assert_eq!(serde_json::to_value(&p).unwrap()["constraints"], "No network.", "kept with a spawn under way");
        let spec: crate::background::Spec = serde_json::from_value(serde_json::json!({
            "provider": "claude", "accountPath": "a", "folder": "f", "worktree": false, "model": null, "effort": "low",
            "mode": "default", "autoReview": false, "tools": true, "hooksUnasked": false
        }))
        .expect("a helpers.json of before still reads");
        assert!(spec.constraints.is_empty() && spec.tools, "with none, a lead all the same");
    }

    #[test]
    fn a_spawns_prompt_is_plain_text_before_anything_else() {
        let p = SpawnParams { prompt: "go\x1b[201~\ry".into(), ..Default::default() };
        assert!(spawn("nobody", p).err().unwrap().contains("control character"));
    }
}
