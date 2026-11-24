//! `luna_session_spawn`: a helper run in the background here, in the core, its account,
//! folder and role settled first. A spawn under way is on disk until a
//! helper comes of it (`SpawnUnderWay`).

pub(crate) mod account;
pub(super) mod background;
pub(super) mod folder;
mod role;
pub(super) mod settle;

use crate::agents::admission::{plain_text, refuse_luna};
use crate::agents::registry::{depth_of, is_dev, reg, stands, top_of, Registry, TOKEN_GONE};
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
/// settings file's stamp: called before any lock is held. The helpers' gate
/// reads it from there (background/places.rs `set_most`).
pub(crate) fn max_descendants() -> usize {
    let most = crate::settings::get().max_helpers.clamp(*HELPERS_RANGE.start(), *HELPERS_RANGE.end()) as usize;
    crate::background::set_most(most);
    most
}

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
    /// A place on its caller's line is promised to it first (background/places.rs
    /// `promise_place`), counted in the hold every helper turn is claimed under,
    /// with the turns at work and those waiting for a place there: two spawns at
    /// once cannot both take the last place, nor a spawn one a waiting turn is
    /// owed. Refused, nothing is recorded.
    pub(super) fn begin(
        caller: &str,
        params: &SpawnParams,
        account_path: &str,
        in_worktree: Option<&str>,
        replaces: Option<u64>,
    ) -> Result<SpawnUnderWay, String> {
        let most = max_descendants();
        let taken = SpawnTaken {
            request: 0,
            caller: caller.to_string(),
            params: params.clone(),
            auto_delete: params.auto_delete,
            resume_on_restart: params.resume_on_restart == Some(true),
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
            let request = r.next_request;
            r.next_request += 1;
            request
        };
        if crate::background::promise_place(caller, request, most).is_err() {
            let why = refusal(caller, &crate::background::lines(), most);
            refused_a_place(&mut reg().lock().unwrap_or_else(|e| e.into_inner()), caller);
            return Err(why);
        }
        {
            let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
            if let Some(old) = replaces {
                r.spawns.remove(&old);
            }
            r.spawns.insert(request, SpawnTaken { request, ..taken });
        }
        crate::background::save();
        Ok(SpawnUnderWay(request))
    }
}

/// Whether `caller` may spawn one more of the `most` its line may have at work
/// (`max_descendants`), as its line stands now: the early look, before folders
/// and logins are read; `begin` counts again where it promises the place.
fn room_now(caller: &str, most: usize) -> Result<(), String> {
    let lines = crate::background::lines();
    let taken = lines.taken(&lines.top(caller));
    if taken.in_use() + taken.waiting < most {
        return Ok(());
    }
    Err(refusal(caller, &lines, most))
}

/// What a caller refused a place for a spawn is told: what takes the places
/// on its line, and how it can free one itself.
fn refusal(caller: &str, lines: &crate::background::Lines, most: usize) -> String {
    let top = lines.top(caller);
    let taken = lines.taken(&top);
    let mut more = Vec::new();
    if taken.promised > 0 {
        more.push(format!("{} spawn(s) under way", taken.promised));
    }
    if taken.waiting > 0 {
        more.push(format!("{} turn(s) waiting for a place", taken.waiting));
    }
    let more = if more.is_empty() { String::new() } else { format!(", and {}", more.join(" and ")) };
    let full = if top == caller {
        let at_work = if taken.at_work.is_empty() { "none".to_string() } else { taken.at_work.join(", ") };
        format!("all {most} places under you are taken: at work {at_work}{more}")
    } else {
        format!("all {most} places on your line, under its top chat {top}, are taken: {} at work{more}", taken.at_work.len())
    };
    format!("{full}. {AT_WORK}. {}", free_first(caller, lines, &taken.at_work))
}

/// What takes a place, as a refusal for want of one says it.
const AT_WORK: &str = "A place is taken by a helper with a turn running or waiting on a question, and by a spawn under \
                       way; a helper between turns takes none, and text sent to one waits for a place rather than being \
                       refused";

/// What a caller refused a place can do: of the helpers at work on its line,
/// those under it are the only ones it can free, by waiting for a turn to end
/// or stopping it. One below the top of its line hears that the rest are its
/// top chat's to free, which is told (`refused_a_place`).
fn free_first(caller: &str, lines: &crate::background::Lines, at_work: &[String]) -> String {
    let own: Vec<&str> = at_work.iter().filter(|id| lines.below(caller, id)).map(String::as_str).collect();
    let own = if own.is_empty() { "none of them is under you".to_string() } else { format!("yours at work: {}", own.join(", ")) };
    let free = "wait for one of yours to end its turn (luna_session_wait), or stop one whose work you no longer need \
                (luna_session_stop), then try again";
    let top = lines.top(caller);
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
    if top == caller || is_dev(&top) {
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
            "No place was left on your line for a helper of {} (luna_session_spawn refused: every place is taken by a \
             helper at work or a spawn under way). Only you can free any of them: wait for a turn under you to end, or \
             stop one whose work is no longer needed (luna_session_stop).",
            self.who.join(", ")
        )
    }
}

/// Where a caller's line stands against its cap (background/places.rs): the
/// places in use under its top chat — its helpers at work, every generation,
/// and its spawns under way, each session once; not the helpers between turns —
/// of how many there are, how many of those are under the caller itself, the
/// only ones it can free, and how many turns wait for a place.
#[derive(Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Places {
    pub in_use: usize,
    pub of: usize,
    pub under_you: usize,
    #[serde(skip_serializing_if = "is_zero")]
    pub waiting: usize,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

pub(in crate::agents) fn places_in(lines: &crate::background::Lines, caller: &str, most: usize) -> Places {
    let taken = lines.taken(&lines.top(caller));
    Places {
        in_use: taken.in_use(),
        of: most,
        under_you: taken.at_work.iter().filter(|id| lines.below(caller, id)).count() + lines.promised_under(caller),
        waiting: taken.waiting,
    }
}

/// `places_in` for `caller` now.
pub(in crate::agents) fn places(caller: &str) -> Places {
    let most = max_descendants();
    places_in(&crate::background::lines(), caller, most)
}

impl Drop for SpawnUnderWay {
    fn drop(&mut self) {
        reg().lock().unwrap_or_else(|e| e.into_inner()).spawns.remove(&self.0);
        // Its helper's first turn has its place by now, or there is none to keep.
        crate::background::unpromise(self.0);
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
    // A blank folder is none named, as `luna_account_get` has it.
    params.folder = params.folder.take().filter(|f| !f.trim().is_empty());
    // Off the registry lock: this reads the CLI's registry from disk. A
    // helper works where its caller does, in the worktree its CLI made for it
    // as well.
    let worktree = if params.folder.is_none() { caller_worktree(caller) } else { None };
    let most = max_descendants();
    // Early, before folders and logins are read, and off the registry's lock,
    // as the helpers' is taken for it; `begin` counts again.
    let room = room_now(caller, most);
    let me = {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let me = r.known.get(caller).cloned().ok_or("caller is not a registered session")?;
        // Its depth and constraints, read below, change as a helper is handed
        // over to the person: read only while the call's token stands for it.
        if !stands(&r, caller) {
            return Err(TOKEN_GONE.into());
        }
        if let Err(e) = room {
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
    // Luna's own folder is refused before anything in it, its `.git` among
    // them, is read. A blank one (a caller registered with none) holds all.
    if let Some(dir) = params.folder.as_deref().filter(|d| !d.trim().is_empty()) {
        refuse_luna(dir)?;
    }
    // A folder inside a CLI-made worktree files the helper under the
    // worktree's project, like the chat it came from; it runs in that
    // worktree. The project its `.git` file leads to is never a share the
    // worktree is not on (`checkout::repo_of_linked`), and is refused in
    // Luna's folder in turn.
    let mut in_worktree = None;
    if let Some(project) = params.folder.as_deref().and_then(crate::checkout::project_of) {
        refuse_luna(&project)?;
        in_worktree = params.folder.replace(project);
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
/// (`max_descendants`) and the range it may be set in.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HelperSettings {
    pub max_helpers: u32,
    pub min: u32,
    pub max: u32,
    pub delegate_auto_compact_tokens: u32,
}

pub fn helper_settings() -> HelperSettings {
    let s = crate::settings::get();
    HelperSettings {
        max_helpers: s.max_helpers.clamp(*HELPERS_RANGE.start(), *HELPERS_RANGE.end()),
        min: *HELPERS_RANGE.start(),
        max: *HELPERS_RANGE.end(),
        delegate_auto_compact_tokens: crate::settings::delegate_auto_compact_tokens(s.delegate_auto_compact_tokens).unwrap_or(0),
    }
}

pub fn set_delegate_auto_compact_tokens(tokens: u32) -> Result<HelperSettings, String> {
    let tokens = crate::settings::delegate_auto_compact_tokens(tokens).unwrap_or(0);
    crate::settings::update(|s| s.delegate_auto_compact_tokens = tokens)?;
    Ok(helper_settings())
}

/// More places are more processes under one chat, each its CLI and its tokens.
pub fn set_max_helpers(n: u32) -> Result<HelperSettings, String> {
    let n = n.clamp(*HELPERS_RANGE.start(), *HELPERS_RANGE.end());
    crate::settings::update(|s| s.max_helpers = n)?;
    crate::log::info("agents", &format!("places on a line: {n}"));
    // More of them: turns waiting for one start now.
    crate::background::set_most(n as usize);
    crate::background::wake();
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

    /// A spawn takes a place on its caller's line as it is taken on, counted
    /// in the hold helper turns are claimed under with the helpers at work and
    /// the turns waiting there, and is refused past them, told what takes the
    /// places and how to free one. Helpers between turns take none, and each
    /// session counts once.
    #[test]
    fn a_spawn_is_refused_past_the_places_its_line_has_at_work() {
        let one = |id: &str, parent: &str, at_work: bool| {
            register(id, Provider::Claude, "C:\\p", "C:\\acc", Some(parent), false, serde_json::json!({}));
            crate::background::idle_for_test(id, Provider::Claude, 1, false);
            crate::background::set_parent_for_test(id, Some(parent.to_string()));
            if at_work {
                crate::background::in_turn_for_test(id);
            }
        };
        register("sp-top", Provider::Claude, "C:\\p", "C:\\acc", None, true, serde_json::json!({}));
        let most = max_descendants();
        one("sp-lead", "sp-top", true);
        let busy: Vec<String> = (0..most - 3).map(|i| format!("sp-busy-{i:02}")).collect();
        for id in &busy {
            one(id, "sp-lead", true);
        }
        // As many again between turns: none of them takes a place.
        let idle: Vec<String> = (0..most).map(|i| format!("sp-idle-{i:02}")).collect();
        for id in &idle {
            one(id, "sp-top", false);
        }
        assert_eq!(places("sp-top"), Places { in_use: most - 2, of: most, under_you: most - 2, waiting: 0 });
        assert_eq!(places("sp-lead").under_you, most - 3, "its own");

        let params: SpawnParams = serde_json::from_value(serde_json::json!({ "prompt": "go" })).unwrap();
        let begin = |caller: &str| SpawnUnderWay::begin(caller, &params, "C:\\acc", None, None);
        let first = begin("sp-lead").unwrap();
        let second = begin("sp-top").unwrap();
        assert_eq!(places("sp-top").in_use, most, "a spawn under way, counted once");
        assert_eq!(places("sp-lead").under_you, most - 2, "its own spawn too");
        let e = begin("sp-top").err().unwrap();
        assert!(e.contains(&format!("all {most} places under you are taken")) && e.contains("sp-busy-00"), "{e}");
        assert!(e.contains("2 spawn(s) under way") && e.contains("between turns takes none") && e.contains("luna_session_wait"), "{e}");
        // Below the top it hears its own, and that its top chat is told.
        let e = begin("sp-lead").err().unwrap();
        assert!(e.contains("yours at work: sp-busy-00") && e.contains("sp-top, at the top of your line, is told"), "{e}");
        assert_eq!(reg().lock().unwrap().places_refused["sp-top"].who, ["sp-lead"]);

        // Text for an idle helper is taken, and waits for a place.
        assert_eq!(crate::background::send("sp-idle-00", "go on", Some("sp-top")), Ok(crate::background::Placed::Waits));
        // A spawn gone frees its place; the turn waiting is ahead of a new one.
        drop(first);
        assert_eq!(places("sp-top"), Places { in_use: most - 1, of: most, under_you: most - 1, waiting: 1 });
        let e = begin("sp-top").err().unwrap();
        assert!(e.contains("1 turn(s) waiting for a place"), "{e}");

        drop(second);
        reg().lock().unwrap().places_refused.remove("sp-top");
        for id in busy.iter().chain(&idle).map(String::as_str).chain(["sp-lead", "sp-top"]) {
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
