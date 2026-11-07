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
mod owed;
mod persist;
pub(crate) mod privilege;
mod read;
mod registry;
mod resume;
mod said;
mod teardown;
mod wait;

pub use answer::answer;
pub use ask::{on_hook, resolve_option, state_of, Ask, AskQuestion};
pub use delivery::{held_count, notes_dir, send};
pub use list::{list, sessions, SessionFilter};
pub use lost::undelivered_left;
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
#[cfg(test)]
pub use teardown::news_for;
pub use teardown::{delete, delete_by_user, forget, kill, queued_refused};
pub use wait::{wait, wait_many};

use crate::background::MarkStands;
use crate::provider::Provider;
use crate::throttle::now_ms;
use ask::{ask_of, live};
use delivery::{await_start, held_sender, screen_of, spill, still_starting, type_text, unknown_command, TypingInto, DRAFT_ALERT_MS, POLL};
use list::disk_meta;
use lost::{
    first_turn_lost, left_words, never_started_what, sentence, take_undelivered, undelivered_for, undelivered_line, undelivered_text,
    worktrees_left_line,
};
use owed::{owe_from, shown, Owed, QUEUED_GRACE_MS};
use persist::{core_up_ms, returning, Opening, SESSION_RETURN_MS};
use privilege::{command_reach, plain_text, reaches_luna, settle_mode, settle_role, Privilege};
use read::{failure_of, transcript_end, transcript_found, transcript_path};
use registry::{depth_of, descendants, is_descendant, issue_token, reg, set_name, top_of, waits_on_helpers, Known, Registry};
use resume::resume_pending;
use said::{answer_file, answer_text, answer_where, begins, first_words, note_file, replies_since, said_since, SHORT_MESSAGE, TASK_CHARS};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Condvar, Mutex, OnceLock};
use teardown::{reap, Auto};
use wait::REPLY_GRACE_MS;

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

/// How long a background spawn in a worktree of its own waits for the CLI
/// to say where it made it.
const WORKTREE_WAIT: std::time::Duration = std::time::Duration::from_secs(20);

/// How often the notifier looks for answers nobody collected.
const NOTIFY_EVERY: std::time::Duration = std::time::Duration::from_secs(2);

/// Looks in a row the caller has to be found idle before a notice goes in:
/// a turn that has just ended may still be followed by the one it queued.
const NOTIFY_IDLE_LOOKS: u32 = 2;

/// A person typed into the caller this recently: hold the notice, it would
/// land in the middle of what they are writing.
const TYPING_QUIET_MS: u64 = 20_000;

/// A turn has ended with the session's processes still at work: give them
/// this long to settle before calling it a job left in the background. Right
/// after a turn they often move for a sample or two anyway.
const BACKGROUND_GRACE_MS: u64 = 20_000;

/// A `wait` out for an answer hands it over itself, so the notifier holds its
/// notice — this long past the answer at most, for a client that has dropped
/// the call it moved to the background.
const WAIT_HOLD_MS: u64 = 10_000;

/// How long into a core's life an answer stays owed to a caller it has not
/// met. What a helper is owed comes back with the helper, before anything
/// else (`tie_back`), and the chat it is owed to only when the window starts
/// that chat's CLI again, seconds later; but a caller that never comes back
/// must not keep a helper from `reap` for good.
const CALLER_RETURN_MS: u64 = 10 * 60 * 1000;

/// The most turn-end notices one caller gets about one session within
/// BREAKER_WINDOW_MS before Luna stops telling it: each costs a turn of the
/// caller, and two chats that keep answering each other's notices would go on
/// until a person came back. Real use never came near it (at most 3 in half an
/// hour per pair, luna.log 10-28 to 11-01); the loop of that night ran one a
/// minute per pair, for 13 hours.
const BREAKER_NOTICES: usize = 12;

const BREAKER_WINDOW_MS: u64 = 15 * 60 * 1000;

/// The notice turns kept per session (see `NoticeTurn`): the debts that ask
/// about them are never many turns behind.
const NOTICE_TURNS_KEPT: usize = 64;

/// How every notice opens, so a model tells it from what the user types.
pub const NOTICE_TAG: &str = "[Luna notice, not typed by the user]";

/// The most warning lines a spawn passes on from the helper's screen.
const MAX_WARNINGS: usize = 5;

/// The percentages of a usage window a caller hears of, once each per window.
const QUOTA_LEVELS: [u8; 2] = [90, 95];

/// How many of the notifier's looks go by between reads of the transcripts of
/// helpers with a budget: finding a visible one's goes to the disk.
const BUDGET_EVERY_LOOKS: u32 = 5;

/// A window this far below the level it was told at has reset, whatever its
/// reset time says; a point or two is rounding.
const QUOTA_SLACK: f64 = 5.0;

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
    fn account_name(&self) -> Option<&str> {
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
    in_worktree: Option<String>,
    /// Codex: run the project's hooks unasked, as the folder would start a
    /// chat of the user's (codex/hooks.rs). Not the helper's to choose.
    hooks_unasked: bool,
    #[serde(flatten)]
    params: SpawnParams,
}

#[derive(Default)]
struct Pending {
    done: Option<Result<String, String>>,
}

/// A `luna_spawn` taken on, in `chat-ties.json` until a helper has come of it and
/// its caller is owed by that: a core going before then leaves the caller an error
/// from the call, or nothing at all, and this is what the next core tells it from.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpawnTaken {
    request: u64,
    caller: String,
    /// What was asked, its account and folder settled.
    params: SpawnParams,
    /// What `params` does not write, the window having no word on them.
    auto_delete: Option<bool>,
    resume_on_restart: bool,
    #[serde(default)]
    budget_tokens: Option<u64>,
    visible: bool,
    in_worktree: Option<String>,
    account_path: String,
    /// With `worktree`: the worktrees there were as it was taken on.
    worktrees_before: Vec<String>,
    /// What the window was asked for, until a session has come of it (`agent_spawns`).
    /// Not saved: it means a request of this core's, and one a core left is settled.
    #[serde(skip)]
    asked: Option<SpawnRequest>,
    /// The session it came to, as soon as there is one (`spawn_is`).
    helper: Option<String>,
    /// That session owes its caller by now, whatever has become of the debt since,
    /// and no core after this is to make it owe again (`take_up_spawns`).
    #[serde(default)]
    owed_placed: bool,
    /// Read from what the last core left: no call waits on it any more.
    #[serde(skip)]
    left_by_core: bool,
    /// Given up on, as far as starting goes: a session the window is still trying
    /// to start for it is refused (`spawn_is`), so what its caller is told stays true.
    #[serde(skip)]
    settled: bool,
    /// What its caller is to be told, once worked out (`settle_spawns`).
    #[serde(skip)]
    news: Option<String>,
    /// Asked for again and not answered yet, so the later request stands for it and
    /// this has only its word to say: not saved, as two on disk would start two.
    #[serde(skip)]
    retried: bool,
}

/// A spawn's record for as long as its call runs, however that ends.
struct SpawnUnderWay(pub(super) u64);

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

/// A turn a Luna notice started. Told a session is not done yet, a caller is
/// not told again of a turn that only answers a notice: two chats with a job
/// each would wake each other for as long as the jobs run.
struct NoticeTurn {
    /// The `turnsEnded` it ends at.
    turn: u32,
    /// Those the notice told of that were the session's own helpers then.
    helpers: Vec<String>,
    /// It told of nobody else: the turn was the session at work on what its
    /// own helpers brought, and nobody's notice came back to it.
    only_helpers: bool,
}

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

/// The same with each account's folder.
fn allowed_account_dirs() -> Vec<crate::accounts::AccountInfo> {
    let blocked = crate::settings::get().agent_blocked_accounts;
    crate::accounts::list_accounts()
        .unwrap_or_default()
        .into_iter()
        .filter(|a| !blocked.iter().any(|b| b == &format!("{}/{}", a.provider.as_str(), a.name)))
        .collect()
}

/// Whether an account folder holds a login its CLI can start on: Claude
/// Code's OAuth refresh token, or Codex's ChatGPT tokens or API key. A
/// helper sent to an account nobody signed in to sits on a login screen.
fn signed_in(provider: Provider, account_path: &str) -> bool {
    let read = |file: &str| {
        std::fs::read_to_string(std::path::Path::new(account_path).join(file))
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
    };
    match provider {
        Provider::Claude => read(".credentials.json").is_some_and(|v| v["claudeAiOauth"]["refreshToken"].is_string()),
        Provider::Codex => {
            read("auth.json").is_some_and(|v| v["tokens"].is_object() || v["OPENAI_API_KEY"].as_str().is_some_and(|k| !k.is_empty()))
        }
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

/// How a list of accounts came down to one.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AccountChoice {
    pub account: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<SkippedAccount>,
    /// Why this one: the first with room, and how full it is; the full list's
    /// best; or the first signed in, its usage not known yet.
    pub why: String,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SkippedAccount {
    pub account: String,
    pub why: String,
}

/// Where a session in a worktree works: the path, and the branch and commit
/// there.
#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Worktree {
    #[serde(rename = "worktree", skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
}

/// Where a session's CLI works now, as far as Luna can say.
fn working_dir_of(pty: &crate::pty::PtyManager, id: &str) -> Option<String> {
    if crate::background::has(id) {
        crate::background::made_worktree(id).or_else(|| crate::background::working_dir(id))
    } else {
        pty.probe(id).and_then(|p| p.working_dir())
    }
}

/// The worktree a session works in — the one it made or the one it was
/// handed — with its branch; nothing for one in a plain folder. Its root,
/// wherever in it the CLI says it is now.
fn worktree_of(pty: &crate::pty::PtyManager, id: &str) -> Worktree {
    let Some((dir, _)) = working_dir_of(pty, id).and_then(|d| crate::worktree::root_of(&d)) else {
        return Worktree::default();
    };
    let head = crate::worktree::head_at(&dir).unwrap_or_default();
    Worktree { branch: head.branch, sha: head.sha, path: Some(dir) }
}

/// `worktree_of`, and for a session in a plain folder the branch and commit there:
/// what a caller running several needs to tell where each of them has got to.
fn checkout_of(pty: &crate::pty::PtyManager, id: &str, k: &Known) -> Worktree {
    let mut worktree = worktree_of(pty, id);
    if worktree.path.is_none() {
        let head = crate::worktree::head_of(&working_dir_of(pty, id).unwrap_or_else(|| k.folder.clone())).unwrap_or_default();
        worktree.branch = head.branch;
        worktree.sha = head.sha;
    }
    worktree
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Neighbor {
    pub id: String,
    pub name: String,
    pub busy: bool,
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

/// Live sessions other than `skip` that run in `dir`, by what the CLI says
/// its cwd is: the project folder for a chat in it, the worktree for one in
/// a worktree.
fn neighbors_in(dir: &str, skip: &[&str]) -> Vec<Neighbor> {
    let ids: Vec<String> = reg().lock().unwrap_or_else(|e| e.into_inner()).known.keys().cloned().collect();
    let pty = crate::pty::manager();
    let mut out = Vec::new();
    for id in ids.into_iter().filter(|id| !skip.contains(&id.as_str())) {
        let cwd = match (crate::background::working_dir(&id), pty.probe(&id)) {
            (Some(dir), _) => dir,
            (None, Some(probe)) => disk_meta(&probe).and_then(|m| m.cwd).unwrap_or_else(|| probe.cwd.clone()),
            (None, None) => continue,
        };
        if crate::pty::same_dir(&cwd, dir) {
            out.push(Neighbor {
                busy: crate::activity::turn_state(&id).is_some_and(|t| t.busy),
                name: chat_name(&id).unwrap_or_default(),
                id,
            });
        }
    }
    out
}

/// The worktree a caller started with `--worktree` runs in. It registers
/// with the project folder, the only one known at spawn, and the CLI makes
/// the worktree itself; what it leaves on disk says where that went.
fn caller_worktree(caller: &str) -> Option<String> {
    let folder = reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(caller)?.folder.clone();
    let cwd = match crate::background::working_dir(caller) {
        Some(dir) => dir,
        None => crate::pty::manager().probe(caller)?.working_dir()?,
    };
    // Its root: the caller's shell may be in a folder of it.
    let (root, _) = crate::worktree::root_of(&cwd)?;
    crate::worktree::is_worktree_of(&folder, &root).then_some(root)
}

/// At this share of a usage window an account counts as spent when a spawn
/// lists several: usage is as old as the window's last fetch and a helper's
/// turn can use up what is left, stopping it on its limit mid-work.
const ACCOUNT_SPENT_PCT: f64 = 95.0;

/// The windows of an account's usage (as `account_usage` keeps it) that are
/// still running, with the percent used: one past its reset says nothing any
/// more, usage being only as fresh as the window's last fetch. None when it
/// has no figures yet.
fn usage_windows(usage: Option<&serde_json::Value>, now_ms: u64) -> Option<Vec<(String, f64)>> {
    let limits = usage?["limits"].as_array().filter(|l| !l.is_empty())?;
    let over = |l: &serde_json::Value| {
        l["resetsAt"]
            .as_str()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .is_some_and(|t| t.timestamp_millis() <= now_ms as i64)
    };
    Some(
        limits
            .iter()
            .filter(|l| !over(l))
            .filter_map(|l| Some((l["label"].as_str().unwrap_or("usage").to_string(), l["usedPct"].as_f64()?)))
            .collect(),
    )
}

/// The provider a list of accounts runs on: one that has every name agents may
/// use at all, the caller's first. An account name can exist under both.
fn list_provider(
    names: &[String],
    allowed: &[crate::accounts::AccountInfo],
    asked: Option<Provider>,
    mine: Provider,
) -> Result<Provider, String> {
    let known: Vec<&String> = names.iter().filter(|n| allowed.iter().any(|a| a.name == **n)).collect();
    let other = if mine == Provider::Claude { Provider::Codex } else { Provider::Claude };
    let order = asked.map_or(vec![mine, other], |p| vec![p]);
    order.into_iter().find(|p| known.iter().all(|n| allowed.iter().any(|a| a.provider == *p && a.name == **n))).ok_or_else(|| match asked {
        Some(p) => format!("the accounts in the list must all be {} accounts", p.as_str()),
        None => "the accounts in the list must all be of one provider".to_string(),
    })
}

/// The first account of the list that is there, signed in and has room in all
/// its windows; failing that, of those signed in, the one with the most room.
/// Unknown usage is no reason to pass over an account the caller put first.
fn pick_from_list(
    names: &[String],
    provider: Provider,
    allowed: &[crate::accounts::AccountInfo],
    usage: &HashMap<String, serde_json::Value>,
    signed_in: impl Fn(&crate::accounts::AccountInfo) -> bool,
    now_ms: u64,
) -> Result<(crate::accounts::AccountInfo, AccountChoice), String> {
    let mut skipped: Vec<SkippedAccount> = Vec::new();
    // Signed in, over the line: what is left if nothing qualifies.
    let mut spent: Vec<(&crate::accounts::AccountInfo, f64)> = Vec::new();
    for name in names {
        let mut skip = |why: String| skipped.push(SkippedAccount { account: name.clone(), why });
        let Some(a) = allowed.iter().find(|a| a.provider == provider && a.name == *name) else {
            skip(format!("no {} account of that name is available to agents", provider.as_str()));
            continue;
        };
        if !signed_in(a) {
            skip("not signed in".into());
            continue;
        }
        let windows = usage_windows(usage.get(&format!("{}/{}", provider.as_str(), a.name)), now_ms);
        let fullest = windows.iter().flatten().max_by(|x, y| x.1.total_cmp(&y.1));
        match fullest {
            Some((label, pct)) if *pct >= ACCOUNT_SPENT_PCT => {
                skip(format!("{label} at {pct:.0}%"));
                spent.push((a, *pct));
            }
            _ => {
                let why = match (&windows, fullest) {
                    (None, _) => "the first in the list signed in; its usage is not known yet".to_string(),
                    (Some(_), None) => "the first in the list with room: its windows have reset since Luna last read them".to_string(),
                    (Some(_), Some((label, pct))) => {
                        format!("the first in the list with room in every window (under {ACCOUNT_SPENT_PCT:.0}%); the fullest, {label}, is at {pct:.0}%")
                    }
                };
                return Ok((a.clone(), AccountChoice { account: a.name.clone(), skipped, why }));
            }
        }
    }
    if let Some((a, pct)) = spent.iter().min_by(|x, y| x.1.total_cmp(&y.1)) {
        skipped.retain(|s| s.account != a.name);
        let why = format!(
            "every signed-in account in the list is at {ACCOUNT_SPENT_PCT:.0}% or more of a window; this one has the most room, at {pct:.0}%"
        );
        return Ok(((*a).clone(), AccountChoice { account: a.name.clone(), skipped, why }));
    }
    if names.is_empty() {
        return Err("the account list is empty".into());
    }
    let why: Vec<String> = skipped.iter().map(|s| format!("{}: {}", s.account, s.why)).collect();
    Err(format!("no account in the list can take a helper ({}); luna_list shows the ones that can", why.join("; ")))
}

/// Asks the frontend to make the chat and start its session; blocks until it
/// answers or the timeout passes.
pub fn spawn(caller: &str, params: SpawnParams) -> Result<Spawned, String> {
    spawn_with(caller, params, None, &mut false)
}

/// `spawn`. `replaces`: the record of the spawn this asks for again. `still_coming`:
/// set when the error is only that the window has not answered in time, the request
/// standing for as long as it may yet (`agent_spawned`): no failure to act on.
fn spawn_with(caller: &str, mut params: SpawnParams, replaces: Option<u64>, still_coming: &mut bool) -> Result<Spawned, String> {
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

/// Claude Code's model aliases, as the new-chat dialog lists them.
pub const CLAUDE_MODELS: [&str; 4] = ["fable", "opus", "sonnet", "haiku"];

const CLAUDE_EFFORTS: [&str; 6] = ["low", "medium", "high", "xhigh", "max", "ultracode"];

const CLAUDE_MODES: [&str; 4] = ["default", "acceptEdits", "plan", "bypassPermissions"];

/// Given and not blank.
fn asked(v: &Option<String>) -> Option<&str> {
    v.as_deref().map(str::trim).filter(|v| !v.is_empty())
}

/// The settings a background helper runs with: what the request asks for
/// over the account's defaults, refused in the same words the window uses
/// for a visible one (agents/bridge.ts).
fn background_spec(params: &SpawnParams, provider: Provider, account_path: &str, folder: &str) -> Result<crate::background::Spec, String> {
    let worktree = params.worktree.unwrap_or(false);
    let tools = params.tools.unwrap_or(false);
    match provider {
        Provider::Claude => {
            if asked(&params.approval).is_some() || asked(&params.sandbox).is_some() {
                return Err("approval and sandbox are Codex settings; Claude takes permissionMode".into());
            }
            let d = crate::claude::defaults::flags(account_path, folder);
            let model = match asked(&params.model) {
                Some(m) if CLAUDE_MODELS.iter().any(|a| m.to_lowercase().contains(a)) => Some(m.to_string()),
                Some(m) => {
                    return Err(format!(
                        "unknown Claude model \"{m}\"; use {} or a full id such as claude-opus-5",
                        CLAUDE_MODELS.join(", ")
                    ))
                }
                None => d["model"].as_str().map(str::to_owned),
            };
            let effort = match asked(&params.effort) {
                Some(e) if CLAUDE_EFFORTS.contains(&e) => e.to_string(),
                Some(e) => return Err(format!("unknown Claude effort \"{e}\"; use {}", CLAUDE_EFFORTS.join(", "))),
                None => d["effort"].as_str().unwrap_or("high").to_string(),
            };
            let mode = match asked(&params.permission_mode) {
                Some(p) if CLAUDE_MODES.contains(&p) => p.to_string(),
                Some(p) => return Err(format!("unknown permission mode \"{p}\"; use {}", CLAUDE_MODES.join(", "))),
                None => d["permissionMode"].as_str().unwrap_or("default").to_string(),
            };
            Ok(crate::background::Spec {
                provider,
                account_path: account_path.to_string(),
                folder: folder.to_string(),
                worktree,
                worktree_name: None,
                model,
                effort,
                mode,
                auto_review: false,
                tools,
                hooks_unasked: false,
                read_only: params.read_only.unwrap_or(false),
                agent: params.agent.clone(),
                parent: None,
                token: None,
            })
        }
        Provider::Codex => {
            use crate::pty::{AUTO_REVIEW, AUTO_REVIEW_SANDBOX};
            if asked(&params.permission_mode).is_some() {
                return Err("permissionMode is a Claude setting; Codex takes approval and sandbox".into());
            }
            // Nobody is there to ask, but a reviewer model can answer.
            let auto_review = match asked(&params.approval) {
                None | Some("never") => false,
                Some(AUTO_REVIEW) => true,
                Some(_) => {
                    return Err(format!(
                        "a background Codex helper has nobody to ask for approval; leave approval out, \
                         use {AUTO_REVIEW}, or spawn it with visible: true"
                    ))
                }
            };
            let d = crate::codex::defaults::flags(account_path, folder);
            let sandbox = match asked(&params.sandbox) {
                Some(s) if auto_review && s != AUTO_REVIEW_SANDBOX => {
                    return Err(format!("{AUTO_REVIEW} runs in {AUTO_REVIEW_SANDBOX} only; leave sandbox out"))
                }
                Some(s) if ["read-only", crate::pty::READ_ONLY_NET, "workspace-write", "danger-full-access"].contains(&s) => s.to_string(),
                Some(s) => {
                    return Err(format!("unknown sandbox \"{s}\"; use read-only, read-only-net, workspace-write, danger-full-access"))
                }
                None if auto_review => AUTO_REVIEW_SANDBOX.to_string(),
                None => d["sandbox"].as_str().unwrap_or("read-only").to_string(),
            };
            let models = crate::codex::models::models_known(account_path);
            let model = asked(&params.model).map(str::to_owned).or_else(|| d["model"].as_str().map(str::to_owned));
            let listed = model.as_deref().and_then(|m| models.iter().find(|x| x.slug == m));
            if let (Some(m), None, false) = (asked(&params.model), listed, models.is_empty()) {
                let all: Vec<&str> = models.iter().map(|x| x.slug.as_str()).collect();
                return Err(format!("this Codex account does not list model \"{m}\"; it has {}", all.join(", ")));
            }
            // As in the dialog: a model asked for brings its own default level.
            let effort = match (asked(&params.effort), listed) {
                (Some(e), Some(l)) if !l.efforts.is_empty() && !l.efforts.iter().any(|x| x == e) => {
                    return Err(format!("{} takes effort {}, not \"{e}\"", l.slug, l.efforts.join(", ")))
                }
                (Some(e), _) => e.to_string(),
                (None, Some(l)) if asked(&params.model).is_some() && l.default_effort.is_some() => {
                    l.default_effort.clone().unwrap_or_default()
                }
                (None, _) => d["effort"].as_str().unwrap_or("medium").to_string(),
            };
            Ok(crate::background::Spec {
                provider,
                account_path: account_path.to_string(),
                folder: folder.to_string(),
                worktree,
                worktree_name: None,
                model,
                effort,
                mode: sandbox,
                auto_review,
                tools,
                hooks_unasked: false,
                read_only: false,
                agent: None,
                parent: None,
                token: None,
            })
        }
    }
}

/// `spawn` for a helper with no row: started here, in the core, on the
/// settings `background_spec` works out.
fn spawn_background(
    caller: &str,
    params: SpawnParams,
    account_path: String,
    in_worktree: Option<String>,
    request: u64,
) -> Result<Spawned, String> {
    let provider = if params.provider.as_deref() == Some("codex") { Provider::Codex } else { Provider::Claude };
    let project = params.folder.clone().unwrap_or_default();
    let own_worktree = params.worktree.unwrap_or(false);
    // A helper handed its caller's worktree works in it and never owns it.
    let run_in = in_worktree.clone().unwrap_or_else(|| project.clone());
    let mut spec = background_spec(&params, provider, &account_path, &run_in)?;
    // As its project folder would start a chat of the user's: a helper does
    // not choose this for itself.
    spec.hooks_unasked = provider == Provider::Codex && crate::codex::hooks::unasked_in(&project);
    spec.parent = Some(caller.to_string());
    let id = crate::background::mint_id();
    spec.worktree_name = (own_worktree && provider == Provider::Claude).then(|| helper_worktree_name(&id));
    let mut settings = match provider {
        Provider::Claude => crate::pty::with_role(
            serde_json::json!({
                "model": spec.model, "effort": spec.effort, "permissionMode": spec.mode, "worktree": own_worktree, "background": true,
            }),
            spec.read_only,
            spec.agent.as_deref(),
        ),
        Provider::Codex => serde_json::json!({
            "model": spec.model, "effort": spec.effort,
            "approval": if spec.auto_review { crate::pty::AUTO_REVIEW } else { "never" }, "sandbox": spec.mode,
            "hooksUnasked": spec.hooks_unasked, "background": true,
        }),
    };
    if params.resume_on_restart == Some(true) {
        settings["resumeOnRestart"] = serde_json::json!(true);
    }
    if let Some(budget) = params.budget_tokens {
        settings["budgetTokens"] = serde_json::json!(budget);
    }
    let tools = spec.tools;
    let auto_delete = params.auto_delete.unwrap_or(true);
    let token = register(&id, provider, &run_in, &account_path, Some(caller), tools, settings.clone());
    // Claude Code asks its permissions of Luna, tools or not.
    spec.token = token.or_else(|| (provider == Provider::Claude).then(|| issue_token(&id)));
    let name = params
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_owned)
        .or_else(|| crate::pty::title_from_prompt(&params.prompt))
        .unwrap_or_else(|| "helper".into());
    set_name(&id, &name);
    {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        if in_worktree.is_some() {
            r.borrowed.insert(id.clone());
        }
        // Nobody sees it to delete it: it goes once its answer is out,
        // unless its caller means to delete it itself.
        if auto_delete {
            r.auto.insert(id.clone(), Auto::default());
        }
    }
    if let Err(e) = spawn_is(request, &id) {
        forget(&id);
        return Err(e);
    }
    if let Err(e) = crate::background::start(&id, spec, &params.prompt) {
        forget(&id);
        return Err(e);
    }
    crate::agentlog::spawned(&crate::agentlog::Spawn {
        id: &id,
        parent: caller,
        parent_name: chat_name(caller),
        folder: &project,
        worktree: own_worktree,
        provider: provider.as_str(),
        account: params.account_name().unwrap_or(""),
        settings: &settings,
        name: Some(name),
        prompt: &params.prompt,
        auto_delete,
        background: true,
    });
    let same_folder = if own_worktree { Vec::new() } else { neighbors_in(&run_in, &[caller, &id]) };
    shown(caller, &id, 0);
    owe_from(caller, &id, 0, 0);
    // The CLI makes its worktree as the first turn starts: worth a few
    // seconds, so the caller need not go looking for it.
    let pty = crate::pty::manager();
    let mut worktree = Worktree::default();
    if own_worktree {
        let deadline = std::time::Instant::now() + WORKTREE_WAIT;
        loop {
            worktree = worktree_of(pty, &id);
            if worktree.path.is_some() || std::time::Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(POLL);
        }
    }
    let worktree_pending = own_worktree && worktree.path.is_none();
    Ok(Spawned {
        id,
        turns_ended: 0,
        settings,
        started: true,
        screen: None,
        ask: None,
        starting: false,
        warnings: Vec::new(),
        same_folder,
        worktree,
        worktree_pending,
        account_choice: None,
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

/// What Luna names the worktree of a Claude Code helper it spawns, and so knows to be
/// that helper's: nothing else goes by this name, where a worktree merely new since the
/// spawn may be anybody's. Claude Code takes it by `--worktree=<name>`, makes it if it
/// is not there and goes back in if it is. Codex takes no name.
pub fn helper_worktree_name(id: &str) -> String {
    format!("luna-{id}")
}

/// The name a visible helper's CLI is to ask for, starting on no prompt of its own:
/// in a conversation the one it has, afresh the one it is still to go by. None for a
/// session Luna named none for: its CLI names its own, as a chat of a person's does.
pub fn worktree_for_start(id: &str, folder: &str, account_path: &str, conversation: bool) -> Option<String> {
    let name = reg().lock().unwrap_or_else(|e| e.into_inner()).worktree_names.get(id).cloned()?;
    if conversation {
        return Some(name);
    }
    let name = name_to_go_on(id, &name, folder, account_path);
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    match &name {
        Some(name) => r.worktree_names.insert(id.to_string(), name.clone()),
        None => r.worktree_names.remove(id),
    };
    name
}

/// What a helper that has no conversation asks for, its worktree named `name`: that
/// name if nothing is there, as the CLI then makes it, or if what is there is still as
/// the CLI made it with nobody in it (`worktree::as_made`), as going back in is then the
/// same as starting in a new one. Anything else is left alone, its parent hears of it
/// (`worktrees_left_line`), and the helper takes the next name. None past a few of those:
/// the CLI names its own.
pub fn name_to_go_on(id: &str, name: &str, folder: &str, account_path: &str) -> Option<String> {
    let mut busy = crate::pty::live_cwds();
    busy.extend(crate::worktree::claude_sessions_running(account_path));
    let mut left = Vec::new();
    let mut chosen = None;
    for n in 1..=9 {
        let candidate = if n == 1 { name.to_string() } else { format!("{name}-{n}") };
        let path = crate::worktree::named_path(folder, &candidate);
        let there = std::path::Path::new(&path).exists();
        if there && (!crate::worktree::as_made(folder, &path) || crate::worktree::someone_in(&path, &busy)) {
            left.push(path);
            continue;
        }
        if there {
            crate::log::warn("agents", &format!("{} goes back into {path}, which a core cut it off in", label(id)));
        }
        chosen = Some(candidate);
        break;
    }
    leave_worktrees(id, left);
    chosen.filter(|n| crate::worktree::name_ok(n))
}

/// A Codex helper starting afresh: what is new of Codex's since it was spawned may be
/// what a core cut it off making, and its parent hears. It is not gone back into, as
/// nothing says whose it is: Codex takes no name, and says where it works only with a
/// thread, which is then a conversation to resume. Ahead of what this start makes.
pub fn report_codex_worktrees(id: &str, folder: &str, account_path: &str) {
    let Some(before) = reg().lock().unwrap_or_else(|e| e.into_inner()).worktrees_before.remove(id) else { return };
    leave_worktrees(id, crate::worktree::codex_new_since(folder, account_path, &before, &crate::pty::live_cwds()));
}

/// `id`'s CLI is not back to go on in any of it: its parent hears of what it began, and
/// opened after that it makes its own.
fn leave_half_made(id: &str, k: &Known) {
    if k.provider == Provider::Codex {
        return report_codex_worktrees(id, &k.folder, &k.account_path);
    }
    let Some(name) = reg().lock().unwrap_or_else(|e| e.into_inner()).worktree_names.get(id).cloned() else { return };
    let path = crate::worktree::named_path(&k.folder, &name);
    if std::path::Path::new(&path).exists() {
        reg().lock().unwrap_or_else(|e| e.into_inner()).worktree_names.insert(id.to_string(), format!("{name}-2"));
        leave_worktrees(id, vec![path]);
    }
}

fn leave_worktrees(id: &str, left: Vec<String>) {
    if left.is_empty() {
        return;
    }
    crate::log::warn("agents", &format!("{} leaves what a core cut it off in: {}", label(id), left.join(", ")));
    {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let told = r.worktrees_left.entry(id.to_string()).or_default();
        for path in left {
            if !told.contains(&path) {
                told.push(path);
            }
        }
    }
    crate::background::save();
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

/// The shortest gap between two notifications from one session: a model in
/// a loop must not bury the desktop in them.
const NOTIFY_GAP_MS: u64 = 10_000;

/// A desktop notification from `caller`, under its own chat's name so it
/// cannot pass for another; a click on it opens that chat.
pub fn notify(caller: &str, text: &str) -> Result<(), String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("text is empty".into());
    }
    let provider = {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let now = now_ms();
        if let Some(last) = r.notified.get(caller) {
            let left = NOTIFY_GAP_MS.saturating_sub(now.saturating_sub(*last));
            if left > 0 {
                return Err(format!("not shown: one notification per {} s, try again in {} s", NOTIFY_GAP_MS / 1000, left.div_ceil(1000)));
            }
        }
        r.notified.insert(caller.to_string(), now);
        r.known.get(caller).map(|k| k.provider)
    };
    let who = match provider {
        Some(Provider::Codex) => "Codex",
        _ => "Claude",
    };
    let chat = chat_name(caller).unwrap_or_else(|| caller.to_string());
    // A toast shows a few lines at most; the rest would only be cut.
    let body: String = text.chars().take(300).collect();
    crate::power::notify_chat(caller, &format!("“{chat}” · {who}"), &body);
    Ok(())
}

/// What became of a session the caller is owed an answer by.
enum Settled {
    /// A turn past the one asked about ended.
    Answered,
    /// A turn ended, but the session left a job running in the background;
    /// owed on from this turn count and transcript end, for that job's end or
    /// a turn of its own.
    Pending(u32, u64),
    /// A turn ended but is not settled — processes still at work, or no
    /// reply in the transcript yet: nothing to say, start the grace from now.
    Hold(u32),
    /// A job looked over and is at work again: the grace starts over.
    Unhold,
    /// Told it is not done, and all that ended since were turns on Luna's own
    /// notices: nothing to say, and owed on from this turn count, what they
    /// said included.
    Skip(u32),
    Waiting,
    /// It asks a question it does not wait on; told once per question.
    Asked(u64),
    /// A core's going stopped its turn and it went on by itself; told once.
    Resumed,
    /// All its job left has sat idle for IDLE_JOB_MS, past that since this
    /// moment; told once, and owed on. It rides (`riders_go`).
    Idle(u64),
    /// Its job's end, seen at this moment, with no word from it and nothing
    /// else to tell: settles as `Answered`, and rides (`riders_go`).
    NoNews(u64),
    /// Its turn at this count has gone quiet (QUIET_TURN_MS); told once per
    /// turn, and owed on.
    Quiet(u32),
    Exited,
}

/// Where a debt stands, by what the session is doing now.
#[derive(Debug, PartialEq, Clone, Copy)]
enum Verdict {
    /// No news of a turn or a job: the session may still be on a prompt.
    Nothing,
    /// A turn the caller is to hear of ended.
    TurnOver,
    /// Told it is not done, and only turns on Luna's own notices ended since.
    OnlyNotices,
    /// Told it is not done, and it still is not.
    AtWork,
    /// Told it is not done, and what it had running has ended with no turn.
    JobOver,
}

/// `real_turn`: a turn past the baseline counts for this caller (`real_turn_past`).
fn verdict(o: &Owed, ended: u32, turn: Option<crate::activity::Turn>, job: bool, real_turn: bool) -> Verdict {
    use crate::activity::Turn;
    let past = ended > o.baseline && turn != Some(Turn::Busy);
    if !o.told_pending {
        return if past { Verdict::TurnOver } else { Verdict::Nothing };
    }
    match (past, real_turn) {
        (true, true) => Verdict::TurnOver,
        (true, false) => Verdict::OnlyNotices,
        _ if turn != Some(Turn::Idle) => Verdict::Nothing,
        _ if job => Verdict::AtWork,
        _ => Verdict::JobOver,
    }
}

/// Whether a turn of `id` past `after` and up to `ended` ended that `caller`,
/// told it is not done, is to hear of: one no Luna notice started; or, once
/// `id` has all its own helpers' answers (`waits`: not yet), one on a notice
/// from them, in which it goes on with what it was asked. Not when it is
/// `caller` who is that helper: that is its own notice coming back.
///
/// `upward`: `caller` is `id`'s parent and `id` has written more than a line
/// or two since it last heard (`news_for_parent`). Then turns on its own
/// helpers' notices reach it while others are still out, every turn since
/// being one of those: what a helper makes of its helpers' answers goes up the
/// tree it was spawned in, and nothing goes back to a session whose notice
/// started a turn. Two chats that wrote to each other are no parent and helper
/// to each other; one that is the other's parent hears only of turns on
/// notices about its helper's own helpers, and the helper never of its parent.
fn real_turn_past(id: &str, caller: &str, after: u32, ended: u32, waits: bool, upward: impl FnOnce() -> bool) -> bool {
    let turns = after.saturating_add(1)..=ended;
    let (counts, on_helpers) = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let marked = r.notice_turns.get(id);
        let mark = |turn: u32| marked.and_then(|m| m.iter().find(|m| m.turn == turn));
        let counts = turns.clone().any(|turn| match mark(turn) {
            None => true,
            Some(m) => !waits && m.helpers.iter().any(|h| h != caller),
        });
        (counts, turns.clone().all(|turn| mark(turn).is_some_and(|m| m.only_helpers && !m.helpers.is_empty())))
    };
    // Read off the lock: it looks at the transcript.
    counts || (on_helpers && !turns.is_empty() && upward())
}

/// For `real_turn_past`: `caller` is `id`'s parent, and `id` has written a
/// message since `cursor` that is more than a line or two (SHORT_MESSAGE): a
/// report worth a turn of the parent's, not "noted, waiting on the rest",
/// which comes with the answer all the same.
fn news_for_parent(pty: &crate::pty::PtyManager, caller: &str, id: &str, k: &Known, cursor: u64) -> bool {
    k.parent.as_deref() == Some(caller) && said_since(pty, id, k.provider, cursor).iter().any(|m| m.chars().count() >= SHORT_MESSAGE)
}

/// A notice about `about` went into `caller` and starts its turn ending at
/// `turn`.
fn notice_turn(r: &mut Registry, caller: &str, turn: u32, about: &[String]) {
    let helpers: Vec<String> = about.iter().filter(|a| is_descendant(r, caller, a)).cloned().collect();
    let only_helpers = helpers.len() == about.len();
    let turns = r.notice_turns.entry(caller.to_string()).or_default();
    turns.push(NoticeTurn { turn, helpers, only_helpers });
    if turns.len() > NOTICE_TURNS_KEPT {
        turns.remove(0);
    }
}

/// Counts a notice to `caller` about `id`. Returns whether it was the last
/// BREAKER_NOTICES allows: the pair is stopped from here on.
fn count_notice(r: &mut Registry, caller: &str, id: &str, now: u64) -> bool {
    let key = (caller.to_string(), id.to_string());
    let times = r.notice_times.entry(key.clone()).or_default();
    times.retain(|at| now.saturating_sub(*at) < BREAKER_WINDOW_MS);
    times.push(now);
    let last = times.len() >= BREAKER_NOTICES;
    if last {
        r.notice_times.remove(&key);
        r.stopped.insert(key, now);
    }
    last
}

/// Whether the next notice to `caller` about `id` is the last the breaker lets
/// through, for it to say so.
fn last_notice(caller: &str, id: &str, now: u64) -> bool {
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    r.notice_times
        .get(&(caller.to_string(), id.to_string()))
        .is_some_and(|times| times.iter().filter(|at| now.saturating_sub(**at) < BREAKER_WINDOW_MS).count() + 1 >= BREAKER_NOTICES)
}

/// Whether Luna has stopped telling `caller` of `id`'s turns and jobs. A person
/// typing into either of them since is someone looking at it, and lifts that;
/// nothing a model does does, or two models answering each other would lift it
/// forever.
fn pair_stopped(pty: &crate::pty::PtyManager, caller: &str, id: &str) -> bool {
    let key = (caller.to_string(), id.to_string());
    let Some(at) = reg().lock().unwrap_or_else(|e| e.into_inner()).stopped.get(&key).copied() else { return false };
    let since = now_ms().saturating_sub(at);
    let typed = |s: &str| pty.typed_ago_ms(s).is_some_and(|ago| ago < since);
    if !typed(caller) && !typed(id) {
        return true;
    }
    reg().lock().unwrap_or_else(|e| e.into_inner()).stopped.remove(&key);
    crate::log::info("agents", &format!("notices to {} about {} go on: a person has typed in", label(caller), label(id)));
    false
}

/// The notice for one session, or None while nothing has happened to it.
fn notice_line(pty: &crate::pty::PtyManager, caller: &str, who: &str, id: &str, k: &Known, o: Owed) -> Option<(Option<String>, Settled)> {
    // A move to another transcript since the debt was looked at, seen to by this
    // look or another's (`moved_to`), has it read from where it reads now.
    let mut o = o;
    transcript_path(pty, id);
    match reg().lock().unwrap_or_else(|e| e.into_inner()).owed.get(&(caller.to_string(), id.to_string())) {
        // Another debt by now (`debts_moved`): the next look goes by that.
        Some(now) if now.rev != o.rev => return None,
        Some(now) => o.cursor = now.cursor,
        None => {}
    }
    let ts = crate::activity::turn_state(id);
    let ended = ts.map(|t| t.turns_ended).unwrap_or(0);
    let turn = ts.map(|t| t.turn);
    let alive = live(pty, id);
    // A Claude Code chat ends a turn on a job it put in the background and takes
    // it up again in a turn of its own when the job is done (`claude -p` stays
    // in its turn until then); for anything else that job's end is all there
    // will be to hear of.
    let left = if ts.is_some_and(|t| t.children_busy) && alive { left_quiet(id) } else { None };
    let background = ts.is_some_and(|t| t.children_busy) && alive;
    let waits = waits_on_helpers(id, caller);
    let between_turns = turn != Some(crate::activity::Turn::Busy);
    let real_turn = o.told_pending
        && real_turn_past(id, caller, o.baseline, ended, waits, || between_turns && news_for_parent(pty, caller, id, k, o.cursor));
    let now = verdict(&o, ended, turn, background || waits, real_turn);
    if now == Verdict::OnlyNotices {
        return Some((None, Settled::Skip(ended)));
    }
    if now == Verdict::AtWork && o.held.is_some() {
        return Some((None, Settled::Unhold));
    }
    // Long idle is no proof of an end — a client waiting on work done
    // elsewhere, a slow download — so the caller hears it once, and the debt
    // stays open for that end.
    if now == Verdict::AtWork && !o.told_idle {
        if let Some((quiet, names)) = left.as_ref().filter(|(quiet, _)| *quiet >= IDLE_JOB_MS) {
            return Some((
                Some(format!(
                    "{who} has only idle processes left, which have done nothing for {} min: {names}. They may be stuck or waiting on something; luna_kill ends them. You will still hear when they end.",
                    quiet / 60_000
                )),
                Settled::Idle(now_ms().saturating_sub(quiet - IDLE_JOB_MS)),
            ));
        }
    }
    if now == Verdict::JobOver && alive {
        // Its processes come and go between samples, and a job's end may be
        // followed by the turn the CLI takes it up in.
        return match o.held {
            Some((n, at)) if n == ended && now_ms().saturating_sub(at) >= BACKGROUND_GRACE_MS => {
                // Its turns on notices may have said something since.
                let said = said_since(pty, id, k.provider, o.cursor);
                let failed = failure_of(pty, id, k.provider);
                // A core's going took the job: news, whatever was said.
                let how = if said.is_empty() && failed.is_none() && !o.job_went { Settled::NoNews(at) } else { Settled::Answered };
                let quoted = if failed.is_some() { String::new() } else { begins(&said) };
                let said = match said.len() {
                    0 => "it has said nothing since your last notice. It does not take that up by itself; luna_send asks it how it went"
                        .to_string(),
                    1 => format!("it wrote 1 message since your last notice; {}{quoted}", answer_file(id, &said)),
                    n => format!("it wrote {n} messages since your last notice; {}{quoted}", answer_file(id, &said)),
                };
                let failed = failed.map_or(String::new(), |why| format!(" Its last turn stopped on an error: {}", sentence(&why)));
                let since = if o.job_went { " that Luna's core has restarted" } else { "" };
                Some((Some(format!("{who} has nothing left running now{since}, and {said}.{failed}")), how))
            }
            Some((n, _)) if n == ended => None,
            _ => Some((None, Settled::Hold(ended))),
        };
    }
    // Going on by itself any moment: what it said so far is no end of anything.
    if now == Verdict::TurnOver && !resume_pending(id) {
        let cut = crate::background::cut_short(id) || o.cut_short;
        let failed = if cut { None } else { failure_of(pty, id, k.provider) };
        let replies = replies_since(pty, id, k.provider, o.cursor, cut || failed.is_some());
        let settle_ms = if background {
            BACKGROUND_GRACE_MS
        } else if replies.is_empty() {
            REPLY_GRACE_MS
        } else {
            0
        };
        // Asked mid-turn: the CLI may start a turn on the text right after.
        let settle_ms = if o.queued { settle_ms.max(QUEUED_GRACE_MS) } else { settle_ms };
        if settle_ms > 0 {
            match o.held {
                Some((n, at)) if n == ended => {
                    if now_ms().saturating_sub(at) < settle_ms {
                        return None;
                    }
                }
                _ => return Some((None, Settled::Hold(ended))),
            }
        }
        let parent = k.parent.as_deref() == Some(caller);
        let lost = undelivered_for(id, caller, parent);
        let file = match (replies.is_empty(), lost.is_empty()) {
            (true, true) => None,
            (true, false) => note_file(id, &undelivered_text(&lost)),
            (false, true) => note_file(id, &answer_text(&replies)),
            (false, false) => note_file(id, &format!("{}\n\n---\n\n{}", answer_text(&replies), undelivered_text(&lost))),
        };
        let reply = match replies.len() {
            0 if !transcript_found(pty, id) => {
                ", but Luna cannot find its transcript, so whether it replied is unknown (luna_read with screen: true shows what there is)"
                    .to_string()
            }
            0 => " without a reply (luna_read with screen: true shows why, if anything does)".to_string(),
            1 => format!(" with 1 message since you asked; {}", answer_where(file.as_deref())),
            n => format!(" with {n} messages since you asked; {}", answer_where(file.as_deref())),
        };
        if parent && (crate::background::never_started(id) || (o.cut_short && first_turn_lost(pty, id))) {
            // Before it is told: what its next turn will not go on in is told with this.
            crate::background::settle_worktree(id);
            let what = never_started_what(&lost, file.as_deref());
            return Some((
                Some(format!("{who} never got started: Luna's core restarted before its first turn got anywhere, so its prompt was lost and there is nothing to carry on from; {what}. luna_send starts it afresh.")),
                Settled::Answered,
            ));
        }
        let lost = undelivered_line(&lost, !replies.is_empty(), file.as_deref());
        let unsure = if o.unsure {
            " Luna cannot tell whether your last message reached it: the core went as it was going in, and its transcript cannot be read. luna_read shows what it has."
        } else {
            ""
        };
        if cut {
            return Some((
                Some(format!("{who} did NOT finish its turn: Luna's core restarted while it worked and stopped it. It ended{reply}.{lost}{unsure} luna_send has it carry on.")),
                Settled::Answered,
            ));
        }
        // Whatever else is going on: this turn did not get to its end.
        if let Some(why) = failed {
            let said = if replies.is_empty() { String::new() } else { format!(" It ended{reply}.") };
            return Some((
                Some(format!("{who} stopped on an error: {}{said}{lost} luna_send has it carry on.", sentence(&why))),
                Settled::Answered,
            ));
        }
        let reply = format!("{reply}{}", begins(&replies));
        if background || waits {
            // Said first and plainly: a weak model read "ended a turn" as
            // done and deleted the helper mid-job. Once per debt: a job can
            // run for days, and each notice costs the caller a turn.
            let agents = crate::activity::subagents(id);
            let what = match &left {
                Some((_, names)) if background => format!("a job it started still runs in the background ({names})"),
                _ if background && agents == 1 => "a subagent its CLI started still works in the background".to_string(),
                _ if background && agents > 1 => format!("{agents} subagents its CLI started still work in the background"),
                _ if background => "a job it started still runs in the background".to_string(),
                _ => "helpers of its own have yet to answer it".to_string(),
            };
            let line = if o.told_pending {
                format!("{who} ended another turn{reply}, but {what}. The next notice comes when that is over, or with a turn of its own.")
            } else {
                format!("{who} is NOT done yet: {what}. It ended a turn{reply}. The next notice comes when that is over, or with a turn of its own.")
            };
            return Some((Some(line), Settled::Pending(ended, transcript_end(id))));
        }
        // Too short a turn for a notice of its own in between.
        let resumed = if o.resumed { " Luna's core stopped it on the way and it resumed automatically." } else { "" };
        return Some((Some(format!("{who} finished its turn{reply}.{lost}{resumed}{unsure}")), Settled::Answered));
    }
    if turn == Some(crate::activity::Turn::Busy) && alive && o.told_quiet != Some(ended) {
        if let Some(quiet) = turn_quiet_ms(pty, id).filter(|q| *q >= QUIET_TURN_MS) {
            // Looked for only once the turn is that quiet: a deadline holds the
            // notice back, never brings it sooner.
            let at = now_ms();
            let due = due_ms(pty, id, k.provider);
            if due.is_none_or(|due| at >= due.saturating_add(PAST_DUE_MS)) {
                return Some((Some(quiet_line(who, quiet, due.map(|due| at - due))), Settled::Quiet(ended)));
            }
        }
    }
    standing_line(pty, caller, who, id, k, o)
}

/// A turn that has said nothing for this long, its caller hears of once: a
/// command that hangs looks like this and like nothing else from outside, and
/// a caller that ended its turn on "a notice comes when it is done" would
/// otherwise hear nothing until a person asks. A long build looks the same,
/// hence once per turn and not its end.
const QUIET_TURN_MS: u64 = 30 * 60 * 1000;

/// A command its own timeout has yet to stop is no sign of a hang: a quiet turn
/// that waits on one is told of only this long past that timeout, by when the
/// CLI has ended the call and written so, or it hangs. Test lanes of 45 min
/// under a timeout of their own each woke their caller for nothing.
const PAST_DUE_MS: u64 = 5 * 60 * 1000;

fn quiet_line(who: &str, quiet_ms: u64, past_due_ms: Option<u64>) -> String {
    let quiet = quiet_ms / 60_000;
    match past_due_ms {
        Some(past) => format!(
            "{who} is mid-turn and has written nothing for {quiet} min, and the command it waits on is {} min past its own timeout, so it may hang; luna_read with screen: true shows where it is, luna_kill stops the turn. Said once per turn; the usual notice comes when it ends.",
            past / 60_000
        ),
        None => format!(
            "{who} is mid-turn and has written nothing for {quiet} min. A command it runs may hang, or simply take long; luna_read with screen: true shows where it is, luna_kill stops the turn. Said once per turn; the usual notice comes when it ends."
        ),
    }
}

/// When the commands a session's turn waits on are due, the latest of them:
/// read off its transcript (`claude::session::due_ms`) or rollout
/// (`codex::session::due_ms`). Asked every few seconds while a quiet turn's
/// notice waits on it, so a file is read again only once it has grown, which a
/// quiet one does not.
fn due_ms(pty: &crate::pty::PtyManager, id: &str, provider: Provider) -> Option<u64> {
    static READ: OnceLock<Mutex<HashMap<std::path::PathBuf, (u64, Option<u64>)>>> = OnceLock::new();
    let path = transcript_path(pty, id)?;
    let len = std::fs::metadata(&path).ok()?.len();
    let read = READ.get_or_init(Default::default);
    if let Some((was, due)) = read.lock().unwrap_or_else(|e| e.into_inner()).get(&path).copied() {
        if was == len {
            return due;
        }
    }
    let due = match provider {
        Provider::Claude => crate::claude::session::due_ms(&path),
        Provider::Codex => crate::codex::session::due_ms(&path),
    };
    let mut map = read.lock().unwrap_or_else(|e| e.into_inner());
    // A file per quiet turn looked at: few, but the app runs for weeks.
    if map.len() >= 64 {
        map.clear();
    }
    map.insert(path, (len, due));
    due
}

/// While a session's turn runs, how long nothing has come of it: a helper's
/// output (background.rs), a chat's transcript; None between turns.
fn turn_quiet_ms(pty: &crate::pty::PtyManager, id: &str) -> Option<u64> {
    let now = now_ms();
    if crate::background::has(id) {
        return crate::background::turn_quiet_ms(id, now);
    }
    let started = crate::activity::turn_started_ms(id)?;
    let written = transcript_path(pty, id)
        .and_then(|p| std::fs::metadata(p).ok()?.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as u64);
    Some(now.saturating_sub(written.max(started)))
}

/// What a notice has to say of where the session stands, whatever its turns
/// have come to: it has exited, or it holds on a prompt or a question. All a
/// pair the breaker has stopped still hears: a helper must never sit on a
/// prompt until it times out for the breaker's sake.
fn standing_line(pty: &crate::pty::PtyManager, caller: &str, who: &str, id: &str, k: &Known, o: Owed) -> Option<(Option<String>, Settled)> {
    let turn = crate::activity::turn_state(id).map(|t| t.turn);
    if !live(pty, id) {
        // The window has yet to start it, as it does a chat in a pane.
        if returning(k, core_up_ms()) {
            return None;
        }
        if o.cut_short && k.parent.as_deref() == Some(caller) && first_turn_lost(pty, id) {
            leave_half_made(id, k);
            let lost = undelivered_for(id, caller, true);
            let file = note_file(id, &undelivered_text(&lost));
            let what = never_started_what(&lost, file.as_deref());
            // No `luna_send` for one that is not running. Opened by a person, it
            // would start with nothing to go on.
            return Some((
                Some(format!("{who} never got started: Luna's core restarted before its first turn got anywhere, and its CLI has not started again, so there is nothing to carry on from; {what}. Spawn it again if it still matters, and luna_delete this one.")),
                Settled::Exited,
            ));
        }
        if o.cut_short {
            return Some((
                Some(format!("{who} did NOT finish its turn: Luna's core restarted while it worked, and it has not started again. It runs again once the user opens it.")),
                Settled::Exited,
            ));
        }
        if o.lost {
            return Some((
                Some(format!("{who} has exited before Luna could type your message in: the user's draft held it until then.")),
                Settled::Exited,
            ));
        }
        return Some((
            Some(match failure_of(pty, id, k.provider) {
                Some(why) => format!("{who} has exited on an error: {}", sentence(&why)),
                None => format!("{who} has exited."),
            }),
            Settled::Exited,
        ));
    }
    if o.resumed && !resume_pending(id) {
        return Some((
            Some(format!("{who} was cut short when Luna's core restarted and has resumed automatically, with anything queued for it. It works on; another notice comes when its turn ends.")),
            Settled::Resumed,
        ));
    }
    let ask = ask_of(id);
    let held = turn == Some(crate::activity::Turn::Waiting) || ask.as_ref().is_some_and(|a| a.blocking && a.kind == "question");
    if held && !o.told_waiting {
        let line = match &ask {
            Some(a) => match &a.id {
                Some(ask_id) => format!("{who} {}. Answer with luna_answer, askId {ask_id}.", a.line()),
                None => format!("{who} {}. Answer with luna_answer.", a.line()),
            },
            None => format!(
                "{who} is stopped on a prompt in its terminal; luna_read with screen: true shows it, luna_answer with keys answers it."
            ),
        };
        return Some((Some(line), Settled::Waiting));
    }
    // Codex's async question: the helper works on, the question waits.
    if let Some(a) = ask.filter(|a| !a.blocking) {
        let key = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            a.line().hash(&mut h);
            h.finish()
        };
        if o.told_ask != key {
            return Some((Some(format!("{who} {} It works on meanwhile; luna_answer answers it.", a.line())), Settled::Asked(key)));
        }
    }
    None
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

/// Whether a notice may go into `caller` now: its CLI is up and between turns
/// and nobody is typing into it. A draft in its composer is `type_text`'s to
/// put aside, or to keep the notice back for.
fn ready_for_notice(pty: &crate::pty::PtyManager, caller: &str) -> bool {
    // A background caller takes it as its next turn, between turns.
    if crate::background::has(caller) {
        return crate::activity::turn_state(caller).is_some_and(|t| t.turn == crate::activity::Turn::Idle);
    }
    pty.alive(caller)
        && crate::activity::turn_state(caller).is_some_and(|t| t.turn == crate::activity::Turn::Idle)
        && !still_starting(pty, caller)
        && pty.typed_ago_ms(caller).is_none_or(|ms| ms >= TYPING_QUIET_MS)
        && drawn_for_notice(pty, caller)
}

/// A CLI just started has drawn something, and may not be taking text yet: its
/// prompt says it is. For its first minute only: a prompt Luna cannot read (a mode
/// with a mark of its own, a CLI that draws it differently) must not silence notices.
fn drawn_for_notice(pty: &crate::pty::PtyManager, id: &str) -> bool {
    let long_up = pty.probe(id).is_none_or(|p| (now_ms() as u128).saturating_sub(p.spawned_at_ms) >= SESSION_RETURN_MS as u128);
    long_up || pty.at_prompt(id) || pty.has_draft(id)
}

/// Whether any of these owed answers has come in, without saying it yet.
fn anything_to_say(pty: &crate::pty::PtyManager, items: &[(String, Known, Owed, String)]) -> bool {
    items.iter().any(|(id, _, o, _)| {
        let ts = crate::activity::turn_state(id);
        let ended = ts.map(|t| t.turns_ended).unwrap_or(0);
        o.resumed || (ended > o.baseline && ts.map(|t| t.turn) != Some(crate::activity::Turn::Busy)) || job_over(o, ts) || !live(pty, id)
    })
}

/// Whether a caller told a session is not done yet may have that session's job
/// to hear of: between turns, with nothing of its own left running. For the
/// holds that wait on news; helpers still to answer it keep `notice_line` from
/// saying so.
fn job_over(o: &Owed, ts: Option<crate::activity::TurnState>) -> bool {
    o.told_pending && ts.is_some_and(|t| t.turn == crate::activity::Turn::Idle && !t.children_busy)
}

/// What a session's turns left running and has done nothing (no CPU, no I/O:
/// `left_quiet`) for this long, its caller hears of once (`Settled::Idle`): that
/// may be a job that is stuck, or one waiting on something, which its caller can
/// end. Not its end, which doing nothing is no proof of: the debt stays open. A
/// real job's quiet stretch is minutes (a link, a test on a timeout).
const IDLE_JOB_MS: u64 = 30 * 60 * 1000;

/// Whether anything a session's turns left still holds the machine
/// (`procs::Leftover::holds`): with nothing that does, it is no job at work for
/// `reap` either.
fn left_holds(id: &str) -> bool {
    if crate::background::has(id) {
        return crate::background::left_holds(id);
    }
    crate::activity::children_hold(id)
}

/// Between turns, how long all a session's turns left running has done nothing,
/// and their names: a helper's job's (background.rs), a chat's under its CLI
/// (activity.rs). None with nothing left.
fn left_quiet(id: &str) -> Option<(u64, String)> {
    if crate::background::has(id) {
        return crate::background::left_quiet(id);
    }
    crate::activity::left_quiet(id)
}

/// For a `wait`: whether a job's end (`over`) has stood BACKGROUND_GRACE_MS, by
/// the debt's own clock (`Owed::held`), the one the notifier goes by too: with
/// a clock each, the notice could go out while the wait still counted, and the
/// caller hear of it twice. A job at work again starts it over. With no debt
/// left to clock (a notice has just settled it), there is nothing to hold for.
fn job_end_stood(key: &(String, String), turns_ended: u32, over: bool) -> bool {
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let Some(o) = r.owed.get_mut(key) else { return over };
    if !over {
        o.held = None;
        return false;
    }
    match o.held {
        Some((n, at)) if n == turns_ended => now_ms().saturating_sub(at) >= BACKGROUND_GRACE_MS,
        _ => {
            o.held = Some((turns_ended, now_ms()));
            false
        }
    }
}

/// Whether a `wait` the caller has out on `id` is to hand this answer over
/// rather than a notice: while nothing has come in, and for WAIT_HOLD_MS
/// after something did — the wait answers within a second or two, so one
/// still out by then belongs to a client that has let the call go. A job's end
/// comes in only once it has stood its grace, on the clock the wait goes by
/// too (`job_end_stood`), which this starts for a wait that is not counting.
fn held_for_wait(pty: &crate::pty::PtyManager, caller: &str, id: &str, o: &Owed) -> bool {
    let key = (caller.to_string(), id.to_string());
    {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        if r.reserved.contains_key(&key) {
            return true;
        }
        if !r.waits.contains_key(&key) {
            return false;
        }
    }
    let ts = crate::activity::turn_state(id);
    let turn = ts.map(|t| t.turn);
    let alive = live(pty, id);
    let ended = ts.map_or(0, |t| t.turns_ended);
    // A prompt it stopped on is news too: `wait(turn_done)` hands that back.
    let other_news =
        (ended > o.baseline && turn != Some(crate::activity::Turn::Busy)) || turn == Some(crate::activity::Turn::Waiting) || !alive;
    let job_ended = alive && job_over(o, ts) && !waits_on_helpers(id, caller);
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let Some(owed) = r.owed.get_mut(&key) else { return true };
    let job_news = job_ended && {
        let since = match owed.held {
            Some((n, at)) if n == ended => at,
            _ => {
                let at = now_ms();
                owed.held = Some((ended, at));
                at
            }
        };
        now_ms().saturating_sub(since) >= BACKGROUND_GRACE_MS
    };
    if !other_news && !job_news {
        // Taken back by a turn begun since (a queued send): the wait gets
        // its whole hold again for the news that turn brings.
        owed.ready_at = None;
        // A job at work again: its end starts its grace over.
        if owed.told_pending && !job_ended {
            owed.held = None;
        }
        return true;
    }
    let at = *owed.ready_at.get_or_insert(now_ms());
    now_ms().saturating_sub(at) < WAIT_HOLD_MS
}

/// Whether what `caller` is owed by `id` still stands `up_ms` into this core's
/// life: a delete takes the session or the caller from `known`, but a caller
/// not started under this core yet may be one on its way back
/// (CALLER_RETURN_MS).
fn owed_stands(known: &HashMap<String, Known>, caller: &str, id: &str, up_ms: u64) -> bool {
    known.contains_key(id) && (known.get(caller).is_some_and(|k| !k.waiting) || up_ms < CALLER_RETURN_MS)
}

/// How a spawn is named to its caller, who may have several: as it was named,
/// else by its prompt.
fn spawn_called(s: &SpawnTaken) -> String {
    let name = s.params.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).map(str::to_owned);
    match name.or_else(|| crate::pty::title_from_prompt(&s.params.prompt)) {
        Some(name) => format!("\"{name}\""),
        None => format!("number {}", s.request),
    }
}

/// What a notice says of a worktree this spawn had begun that nothing came of. It
/// is not removed: it may hold what a person would want, and nothing has claimed
/// it. A worktree another chat made meanwhile that has no session running in it
/// reads the same; it is only named.
fn worktrees_left(s: &SpawnTaken) -> String {
    let (Some(true), Some(folder)) = (s.params.worktree, s.params.folder.as_deref()) else { return String::new() };
    left_words(&crate::worktree::left_since(
        crate::worktree::made_for(folder, &s.account_path),
        &s.worktrees_before,
        &crate::pty::live_cwds(),
    ))
}

/// The notice of a spawn nothing came of. It says why the call itself failed: the
/// caller's CLI went with the same core, and all it has of that call is an error.
fn never_completed(s: &SpawnTaken, why: Option<&str>, left: &str) -> String {
    let prompt = match note_file(&format!("spawn-{}", s.request), &s.params.prompt) {
        Some(path) => format!("Its prompt is in {}", path.display()),
        None => "Its prompt was not kept".to_string(),
    };
    let why = why.map_or(String::new(), |w| format!(" (started afresh, it failed too: {w})"));
    format!(
        "Your luna_spawn {} never completed: Luna's core restarted while it was being made, which is also why that call ended in an error or with no answer. No helper came of it{why}. {prompt}; spawn it again if it still matters.{left}",
        spawn_called(s),
    )
}

/// Asks for it again as it was asked (`resumeOnRestart`), as `spawn` would have it
/// had it not settled anything yet, and leaves what its caller is to hear. Blocks
/// as a spawn does.
fn spawn_afresh(s: SpawnTaken) {
    let mut params = s.params.clone();
    params.visible = Some(s.visible);
    params.auto_delete = s.auto_delete;
    params.resume_on_restart = Some(true);
    params.budget_tokens = s.budget_tokens;
    if let Some(worktree) = &s.in_worktree {
        params.folder = Some(worktree.clone());
    }
    // Before this one can make a worktree of its own, which is no leftover.
    let left = worktrees_left(&s);
    let mut still_coming = false;
    let made = spawn_with(&s.caller, params, Some(s.request), &mut still_coming);
    let told = match &made {
        Ok(made) => {
            crate::log::warn(
                "agents",
                &format!("{}'s spawn {} started afresh as {} after a core cut it off", label(&s.caller), s.request, label(&made.id)),
            );
            format!(
                "Your luna_spawn {} was cut off when Luna's core restarted, which is also why that call ended in an error or with no answer. As it asked with resumeOnRestart, Luna started it afresh as {}: do not spawn it again, and its answer comes as a notice.{left}",
                spawn_called(&s),
                made.id,
            )
        }
        // The later request stands and the window may yet answer it, so this is no
        // failure: told to spawn again, the caller would have two.
        Err(why) if still_coming => {
            crate::log::warn("agents", &format!("{}'s spawn {} asked for afresh is not answered yet: {why}", label(&s.caller), s.request));
            format!(
                "Your luna_spawn {} was cut off when Luna's core restarted, which is also why that call ended in an error or with no answer. As it asked with resumeOnRestart, Luna asked for it afresh, and the window has not started it yet. It may still come up: luna_list will show it and its answer comes as a notice, or a notice says it failed. Do not spawn it again.{left}",
                spawn_called(&s),
            )
        }
        Err(why) => {
            crate::log::warn(
                "agents",
                &format!("{}'s spawn {} could not start afresh after a core cut it off: {why}", label(&s.caller), s.request),
            );
            never_completed(&s, Some(why), &left)
        }
    };
    let helper = made.ok().map(|made| made.id);
    let owed_placed = helper.is_some();
    let told = SpawnTaken { news: Some(told), helper, owed_placed, settled: true, retried: still_coming, ..s };
    reg().lock().unwrap_or_else(|e| e.into_inner()).spawns.insert(told.request, told);
    crate::background::save();
}

/// A spawn that had timed out, its caller told it may yet come up, and the window
/// says it did not.
fn spawn_failed_late(s: SpawnTaken, why: &str) {
    crate::log::warn("agents", &format!("{}'s spawn {} failed after it timed out: {why}", label(&s.caller), s.request));
    let prompt = match note_file(&format!("spawn-{}", s.request), &s.params.prompt) {
        Some(path) => format!("Its prompt is in {}", path.display()),
        None => "Its prompt was not kept".to_string(),
    };
    let news = format!(
        "Your luna_spawn {}, which had not started in time, did not come up after all: {} No helper came of it. {prompt}; spawn it again if it still matters.{}",
        spawn_called(&s),
        sentence(why),
        worktrees_left(&s)
    );
    if let Some(left) = reg().lock().unwrap_or_else(|e| e.into_inner()).spawns.get_mut(&s.request) {
        left.news = Some(news);
    }
    crate::background::save();
}

/// What the spawns the last core left have come to, once a window has had the time
/// to finish any it was part way through (`SESSION_RETURN_MS`, as for a chat coming
/// back): a session known by then is one that came of it, and answers as any other
/// (`take_up_spawns`, and a row since deleted is no longer known); for the rest the
/// caller is told, or with `resumeOnRestart` it is started afresh and then told.
/// What it came to is decided under the lock that gives it up (`SpawnTaken::settled`):
/// past that a session the window is still starting for it is refused (`spawn_is`),
/// and before it that session is the one that came of it.
fn settle_spawns(up_ms: u64) {
    if up_ms < SESSION_RETURN_MS {
        return;
    }
    let due: Vec<(SpawnTaken, bool)> = {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let Registry { spawns, known, abandoned, .. } = &mut *r;
        let mut due = Vec::new();
        for s in spawns.values_mut().filter(|s| s.left_by_core && !s.settled) {
            let made = s.helper.as_ref().is_some_and(|h| known.contains_key(h));
            s.settled = true;
            if !made && s.resume_on_restart {
                abandoned.remove(&s.request);
            }
            due.push((s.clone(), made));
        }
        for (s, made) in &due {
            if *made {
                spawns.remove(&s.request);
            }
        }
        due
    };
    for (s, made) in due {
        if made {
            crate::log::info(
                "agents",
                &format!("{}'s spawn {} had come to {} when a core went", label(&s.caller), s.request, s.helper.as_deref().unwrap_or("")),
            );
            crate::background::save();
        } else if s.resume_on_restart {
            let _ = std::thread::Builder::new().name("luna-spawn-again".into()).spawn(move || spawn_afresh(s));
        } else {
            let news = never_completed(&s, None, &worktrees_left(&s));
            crate::log::warn("agents", &format!("{}'s spawn {} went with a core: {}", label(&s.caller), s.request, news));
            if let Some(left) = reg().lock().unwrap_or_else(|e| e.into_inner()).spawns.get_mut(&s.request) {
                left.news = Some(news);
            }
        }
    }
}

/// What each caller is to hear of its spawns (`settle_spawns`), by caller: the
/// request and the line, once it is there to hear it. One that has not come back
/// in the time a caller gets (CALLER_RETURN_MS), or the dev caller with no terminal
/// to be told in, goes to the log, which names the file.
fn spawn_news(up_ms: u64) -> HashMap<String, Vec<(u64, String)>> {
    let mut news: HashMap<String, Vec<(u64, String)>> = HashMap::new();
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let Registry { spawns, known, .. } = &mut *r;
    let mut gave_up = Vec::new();
    let mut told: Vec<&SpawnTaken> = spawns.values().filter(|s| s.news.is_some()).collect();
    told.sort_by_key(|s| s.request);
    for s in told {
        let line = s.news.clone().unwrap_or_default();
        match known.get(&s.caller) {
            Some(k) if !k.waiting && s.caller != DEV_CALLER => news.entry(s.caller.clone()).or_default().push((s.request, line)),
            Some(k) if k.waiting && up_ms < CALLER_RETURN_MS => {}
            _ => gave_up.push((s.request, s.caller.clone(), line)),
        }
    }
    for (request, caller, line) in gave_up {
        spawns.remove(&request);
        crate::log::warn("agents", &format!("{caller} is not there to be told: {line}"));
    }
    news
}

/// The longest a line with no news in it waits for a notice to ride with: a
/// caller may be waiting on that very job while another helper runs for days.
const RIDE_MAX_MS: u64 = 10 * 60 * 1000;

/// Since when a line that is no reason to wake its caller (`Settled::NoNews`,
/// `Settled::Idle`) has stood, if it is one.
fn rides_since(how: &Settled) -> Option<u64> {
    match how {
        Settled::NoNews(since) | Settled::Idle(since) => Some(*since),
        _ => None,
    }
}

/// Whether such lines, the oldest standing since `oldest`, go out now. A wake
/// costs a caller a read of its whole context, so they go with a notice it is
/// getting anyway (`company`). Alone they go once all it is owed is such lines
/// (`only_riders`), or with nothing anywhere at work (`busy` 0), so no notice is
/// on its way: all that lets a power-off start counting (power.rs `advance`), which
/// is not to go ahead of a line held here. That is this gate's word only: a line
/// past it can still be held back, as any notice can, by a draft in its caller's
/// composer or a turn it is on, and a power-off does not wait for that. Their debts
/// stay open until then, for a `luna_wait` and against `reap`, and what happens to
/// the session meanwhile replaces the line. A budget line rides the same way, with
/// no debt to it.
pub(crate) fn riders_go(company: bool, only_riders: bool, oldest: u64, now: u64, busy: usize) -> bool {
    company || only_riders || busy == 0 || now.saturating_sub(oldest) >= RIDE_MAX_MS
}

/// Runs for the life of the app: every few seconds, for each caller owed an
/// answer, checks whether one came and tells the caller once it is idle; and
/// deletes the `autoDelete` helpers that are done (see `reap`).
fn notifier() {
    if let Ok(bytes) = std::fs::read(quota_file()) {
        reg().lock().unwrap_or_else(|e| e.into_inner()).quota_told = quota_from(&bytes, now_ms());
    }
    let mut idle_looks: HashMap<String, u32> = HashMap::new();
    // Callers whose notice a draft is holding back: since when, and whether
    // the desktop has been told.
    let mut drafts: HashMap<String, (u64, bool)> = HashMap::new();
    let mut looks_at_budgets = 0;
    loop {
        std::thread::sleep(NOTIFY_EVERY);
        let pty = crate::pty::manager();
        reap(pty);
        // Owed by caller, each session with what is known of it; anything a
        // delete has made unknown is dropped on the way, and a caller yet to
        // come back has nothing to be told in.
        let mut by_caller: std::collections::BTreeMap<String, Vec<(String, Known, Owed, String)>> = Default::default();
        let mut gave_up: Vec<(String, String)> = Vec::new();
        {
            let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
            let Registry { owed, known, names, .. } = &mut *r;
            let up_ms = core_up_ms();
            owed.retain(|(caller, id), _| {
                let stands = owed_stands(known, caller, id, up_ms);
                if !stands && known.contains_key(id) {
                    gave_up.push((caller.clone(), id.clone()));
                }
                stands
            });
            for ((caller, id), o) in owed.iter() {
                let Some(k) = known.get(id) else { continue };
                if known.get(caller).is_none_or(|k| k.waiting) {
                    continue;
                }
                let parent = k.parent.as_deref() == Some(caller.as_str());
                let name = names.get(id).map(|n| n.trim()).filter(|n| !n.is_empty());
                let who = match (parent, name) {
                    (true, _) => your_helper(id, name),
                    (false, Some(n)) => format!("Session \"{n}\" ({id}), which you wrote to,"),
                    (false, None) => format!("Session {id}, which you wrote to,"),
                };
                by_caller.entry(caller.clone()).or_default().push((id.clone(), k.clone(), *o, who));
            }
        }
        for (caller, id) in gave_up {
            crate::log::warn(
                "agents",
                &format!(
                    "{caller} is owed the answer of {} but has not come back in {} min; no notice will go",
                    label(&id),
                    CALLER_RETURN_MS / 60_000
                ),
            );
        }
        // A caller owed nothing still hears of the quota its helpers run on,
        // and of a spawn of its own that a core's going cut off.
        let mut quota = quota_news(pty);
        for caller in quota.keys() {
            by_caller.entry(caller.clone()).or_default();
        }
        looks_at_budgets = (looks_at_budgets + 1) % BUDGET_EVERY_LOOKS;
        let (mut budget, budgets_moved) = budget_news(pty, looks_at_budgets == 0);
        if budgets_moved {
            crate::background::save();
        }
        for caller in budget.keys() {
            by_caller.entry(caller.clone()).or_default();
        }
        settle_spawns(core_up_ms());
        let mut news: HashMap<String, Vec<String>> = HashMap::new();
        {
            let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
            let Registry { news: queued, known, .. } = &mut *r;
            queued.retain(|(caller, _)| known.contains_key(caller));
            for (caller, line) in queued.iter() {
                news.entry(caller.clone()).or_default().push(line.clone());
            }
        }
        for caller in news.keys() {
            by_caller.entry(caller.clone()).or_default();
        }
        let mut spawns = spawn_news(core_up_ms());
        for caller in spawns.keys() {
            by_caller.entry(caller.clone()).or_default();
        }
        idle_looks.retain(|c, _| by_caller.contains_key(c));
        drafts.retain(|c, _| by_caller.contains_key(c));
        reg().lock().unwrap_or_else(|e| e.into_inner()).notices_held.retain(|c, _| by_caller.contains_key(c));
        for (caller, items) in by_caller {
            // The last notice is still going in: it starts a turn of its own.
            if TypingInto::under_way(&caller) {
                idle_looks.insert(caller, 0);
                continue;
            }
            if pty.has_draft(&caller) && pty.alive(&caller) && anything_to_say(pty, &items) {
                let (since, told) = drafts.entry(caller.clone()).or_insert((now_ms(), false));
                if !*told && now_ms().saturating_sub(*since) >= DRAFT_ALERT_MS {
                    *told = true;
                    let chat = chat_name(&caller).unwrap_or_else(|| caller.clone());
                    crate::power::notify_chat(
                        &caller,
                        "A helper has answered",
                        &format!("\"{chat}\" has an unsent draft, so Luna is holding the helper's notice until you send or clear it."),
                    );
                }
            } else {
                drafts.remove(&caller);
            }
            let looks = idle_looks.entry(caller.clone()).or_default();
            *looks = if ready_for_notice(pty, &caller) { *looks + 1 } else { 0 };
            if *looks < NOTIFY_IDLE_LOOKS {
                continue;
            }
            let mut lines = Vec::new();
            let mut settled = Vec::new();
            let mut riders = Vec::new();
            for (id, k, o, who) in &items {
                if held_for_wait(pty, &caller, id, o) {
                    continue;
                }
                let said = if pair_stopped(pty, &caller, id) {
                    standing_line(pty, &caller, who, id, k, *o)
                } else {
                    notice_line(pty, &caller, who, id, k, *o)
                };
                match said {
                    Some((Some(mut line), how)) => {
                        // Told once, with whatever is told first.
                        if k.parent.as_deref() == Some(caller.as_str()) {
                            line.push_str(&worktrees_left_line(id));
                        }
                        if counts_for_breaker(&how) && last_notice(&caller, id, now_ms()) {
                            line.push_str(&format!(
                                " Luna stops telling you of its turns and jobs now: {BREAKER_NOTICES} notices in {} minutes look like chats waking each other. Its prompts and questions still come, luna_wait and luna_read still reach it, and a person typing into either chat lifts this.",
                                BREAKER_WINDOW_MS / 60_000
                            ));
                        }
                        if rides_since(&how).is_some() {
                            riders.push((line, id.clone(), how, *o));
                        } else {
                            lines.push(line);
                            settled.push((id.clone(), how, *o));
                        }
                    }
                    Some((None, Settled::Hold(ended))) => {
                        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(o) = r.owed.get_mut(&(caller.clone(), id.clone())) {
                            o.held = Some((ended, now_ms()));
                        }
                    }
                    Some((None, Settled::Unhold)) => {
                        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(o) = r.owed.get_mut(&(caller.clone(), id.clone())) {
                            o.held = None;
                        }
                    }
                    Some((None, Settled::Skip(ended))) => {
                        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(o) = r.owed.get_mut(&(caller.clone(), id.clone())).filter(|now| now.rev == o.rev) {
                            o.baseline = ended;
                            o.held = None;
                        }
                    }
                    _ => {}
                }
            }
            let quota_lines = quota.remove(&caller).unwrap_or_default();
            lines.extend(quota_lines.iter().map(|n| n.line.clone()));
            let news_lines = news.remove(&caller).unwrap_or_default();
            lines.extend(news_lines.iter().cloned());
            let spawn_lines = spawns.remove(&caller).unwrap_or_default();
            lines.extend(spawn_lines.iter().map(|(_, line)| line.clone()));
            let breaches = budget.remove(&caller).unwrap_or_default();
            let mut budget_lines = Vec::new();
            if let Some(oldest) = riders.iter().filter_map(|(_, _, how, _)| rides_since(how)).chain(breaches.iter().map(|n| n.since)).min()
            {
                if riders_go(!lines.is_empty(), riders.len() == items.len(), oldest, now_ms(), crate::activity::summary().busy) {
                    for (line, id, how, o) in riders {
                        lines.push(line);
                        settled.push((id, how, o));
                    }
                    lines.extend(breaches.iter().map(|n| n.line.clone()));
                    budget_lines = breaches;
                }
            }
            if lines.is_empty() {
                reg().lock().unwrap_or_else(|e| e.into_inner()).notices_held.remove(&caller);
                continue;
            }
            if debts_moved(&caller, &settled) {
                continue;
            }
            // Typing it waits up to SEND_TIMEOUT for the turn to start, off this
            // loop: one caller stuck on that is no reason to hold back the rest.
            let Some(typing) = TypingInto::claim(&caller) else { continue };
            idle_looks.insert(caller.clone(), 0);
            let text = format!("{NOTICE_TAG} {}", lines.join(" "));
            let typed = std::thread::Builder::new().name("luna-notice".into()).spawn(move || {
                let _typing = typing;
                deliver_notice(crate::pty::manager(), &caller, &text, settled, quota_lines, spawn_lines, news_lines, budget_lines);
            });
            if let Err(e) = typed {
                crate::log::warn("agents", &format!("could not start typing a notice: {e}"));
            }
        }
    }
}

/// A debt a notice was put together on has become another since (`Owed::rev`), and
/// what it says may no longer hold: nothing of it is typed, nothing is settled, and
/// the next look starts over. A debt taken since is left to `settle_told`.
fn debts_moved(caller: &str, told: &[(String, Settled, Owed)]) -> bool {
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    told.iter().any(|(id, _, o)| r.owed.get(&(caller.to_string(), id.clone())).is_some_and(|now| now.rev != o.rev))
}

/// What the breaker counts, and all it silences: notices a turn's or a job's
/// end brings. Not a prompt or a question, which hold until they are answered
/// and cannot loop by themselves, and a busy helper may ask many.
fn counts_for_breaker(how: &Settled) -> bool {
    matches!(how, Settled::Answered | Settled::NoNews(_) | Settled::Pending(..))
}

/// Types a notice into `caller` and settles what it said (`settled`: the
/// session, what became of it, the debt as it was announced; `quota_lines`: the
/// usage windows it told of; `spawn_lines`: the spawns cut off by a core going;
/// `budget_lines`: the budgets passed).
#[allow(clippy::too_many_arguments)]
fn deliver_notice(
    pty: &crate::pty::PtyManager,
    caller: &str,
    text: &str,
    settled: Vec<(String, Settled, Owed)>,
    quota_lines: Vec<QuotaNews>,
    spawn_lines: Vec<(u64, String)>,
    news_lines: Vec<String>,
    budget_lines: Vec<BudgetNews>,
) {
    let before = crate::activity::turn_state(caller);
    let baseline = before.map(|t| t.turns_ended).unwrap_or(0);
    // The turn it starts: the next, or for a background caller that has begun
    // one since the look, the one after that.
    let background = crate::background::has(caller);
    let (typed, turn) = if background {
        // A background parent's own record says when these have arrived (`budget_news`).
        let marks = budget_lines.iter().map(|n| crate::background::BudgetMark { id: n.id.clone(), line: n.line.clone() }).collect();
        match crate::background::send_marked(caller, text, None, marks) {
            Ok(queued) => (Ok(Some(true)), baseline + 1 + u32::from(queued)),
            Err(e) => (Err(e), 0),
        }
    } else {
        (type_text(pty, caller, text, before.map(|t| t.turn), baseline), baseline + 1)
    };
    // Typed and no turn begun, with text still in the composer: its Enter went
    // astray, and it never reached the model. What it told of stays owed, as it
    // is for text held back, and the text stands as a draft: the next notice
    // waits on it as on a person's, and its reader may send it themselves.
    // Where the composer will not read, a turn begun late cannot be told from
    // text a dialog took: settled, as ever.
    let stuck = matches!(typed, Ok(Some(false))) && pty.composer(caller).is_some_and(|c| !c.text.trim().is_empty());
    let typed = if stuck {
        crate::log::warn(
            "agents",
            &format!("notice to {} sits in its composer, no turn begun: kept as a draft, still owed", label(caller)),
        );
        pty.set_draft(caller, text.chars().count() as u64);
        Ok(None)
    } else {
        typed
    };
    let about = settled
        .iter()
        .map(|(id, _, _)| label(id))
        .chain(quota_lines.iter().map(|n| format!("{} at {}%", n.key.1, n.told.level)))
        .chain(spawn_lines.iter().map(|(request, _)| format!("spawn {request}")))
        .chain(news_lines.iter().map(|_| "an auto-delete".to_string()))
        .chain(budget_lines.iter().map(|n| format!("the budget of {}", label(&n.id))))
        .collect::<Vec<_>>()
        .join(", ");
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    match typed {
        Ok(None) => {
            r.notices_held
                .insert(caller.to_string(), settled.len() + quota_lines.len() + spawn_lines.len() + news_lines.len() + budget_lines.len());
        }
        _ => {
            r.notices_held.remove(caller);
        }
    }
    drop(r);
    match typed {
        Ok(None) => {}
        Ok(Some(started)) => {
            if started {
                crate::log::info("agents", &format!("notice to {} about {about}", label(caller)));
            } else {
                crate::log::warn("agents", &format!("notice to {} about {about} did not start a turn", label(caller)));
            }
            // Its words on what never reached them went with this notice.
            let told_lost: Vec<String> = settled
                .iter()
                .filter(|(_, how, _)| matches!(how, Settled::Answered | Settled::NoNews(_) | Settled::Exited))
                .map(|(id, _, _)| id.clone())
                .collect();
            let told_left: Vec<String> =
                settled.iter().map(|(id, _, _)| id.clone()).filter(|id| parent_of(id).as_deref() == Some(caller)).collect();
            let mut stopped = Vec::new();
            let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
            for id in &told_left {
                r.worktrees_left.remove(id);
            }
            // A turn it did not start stays unmarked: at worst the caller hears
            // of a turn once too often, never misses one.
            if started {
                let told_of: Vec<String> = settled.iter().map(|(id, _, _)| id.clone()).collect();
                notice_turn(&mut r, caller, turn, &told_of);
            }
            for (id, how, told) in settled {
                if counts_for_breaker(&how) && count_notice(&mut r, caller, &id, now_ms()) {
                    stopped.push(id.clone());
                }
                settle_told(&mut r, (caller.to_string(), id), how, &told);
            }
            let told_quota = !quota_lines.is_empty();
            for n in quota_lines {
                r.quota_told.insert(n.key, n.told);
            }
            r.news.retain(|(c, line)| !(c == caller && news_lines.contains(line)));
            let told_spawns = !spawn_lines.is_empty();
            for (request, _) in spawn_lines {
                r.spawns.remove(&request);
            }
            let told_budgets = !background && !budget_lines.is_empty();
            for n in budget_lines.iter().filter(|_| !background) {
                if let Some(k) = r.known.get_mut(&n.id) {
                    k.settings["budgetTold"] = serde_json::json!(true);
                }
            }
            drop(r);
            if told_quota {
                save_quota_told();
            }
            for id in &told_lost {
                take_undelivered(id, caller, parent_of(id).as_deref() == Some(caller));
            }
            if !told_lost.is_empty() || !told_left.is_empty() || told_spawns || told_budgets {
                crate::background::save();
            }
            for id in stopped {
                breaker_tripped(caller, &id);
            }
        }
        Err(e) => crate::log::warn("agents", &format!("notice to {} about {about} failed: {e}", label(caller))),
    }
}

/// What a notice that went in does to the debt it told of (`told`). Only to
/// that debt: typing the notice takes seconds, and the turn it starts may ask
/// again before they are over, a question that is owed on; and a `wait` may
/// have taken the answer meanwhile, which leaves nothing to carry on.
fn settle_told(r: &mut Registry, key: (String, String), how: Settled, told: &Owed) {
    if r.owed.get(&key).map(|o| o.rev) != Some(told.rev) {
        return;
    }
    match how {
        Settled::Answered | Settled::NoNews(_) | Settled::Exited => {
            r.owed.remove(&key);
        }
        Settled::Waiting => {
            if let Some(o) = r.owed.get_mut(&key) {
                o.told_waiting = true;
            }
        }
        Settled::Pending(ended, cursor) => {
            r.owed.insert(key, Owed::pending(ended, cursor));
        }
        Settled::Asked(ask) => {
            if let Some(o) = r.owed.get_mut(&key) {
                o.told_ask = ask;
            }
        }
        Settled::Resumed => {
            if let Some(o) = r.owed.get_mut(&key) {
                o.resumed = false;
            }
        }
        Settled::Idle(_) => {
            if let Some(o) = r.owed.get_mut(&key) {
                o.told_idle = true;
            }
        }
        Settled::Quiet(ended) => {
            if let Some(o) = r.owed.get_mut(&key) {
                o.told_quiet = Some(ended);
            }
        }
        Settled::Hold(_) | Settled::Unhold | Settled::Skip(_) => {}
    }
}

/// Says that the breaker has stopped a pair: in the log, and once on the
/// desktop, as nothing else will tell the person why these chats went quiet.
fn breaker_tripped(caller: &str, id: &str) {
    crate::log::warn(
        "agents",
        &format!(
            "stopped telling {} of the turns and jobs of {}: {BREAKER_NOTICES} notices within {} minutes; a person typing into either lifts this",
            label(caller),
            label(id),
            BREAKER_WINDOW_MS / 60_000
        ),
    );
    let chat = chat_name(caller).unwrap_or_else(|| caller.to_string());
    let other = chat_name(id).unwrap_or_else(|| id.to_string());
    crate::power::notify_chat(
        caller,
        "Luna stopped a notice loop",
        &format!("\"{chat}\" and \"{other}\" kept waking each other, so Luna has stopped telling the first when the second's turns end. Type into either chat to let it go on."),
    );
}

/// Each account's usage as the window shows it, by `provider/name`, sent
/// whenever it changes: `luna_list` passes it on, so an agent picks an
/// account by the numbers the user sees — and the core asks no usage
/// endpoint of its own, which would only draw their 429s sooner.
#[tauri::command]
pub fn account_usage(usage: HashMap<String, serde_json::Value>) {
    reg().lock().unwrap_or_else(|e| e.into_inner()).usage = usage;
}

/// Account names the user has switched off for agents, as `provider/name`.
#[tauri::command]
pub fn agent_blocked_accounts() -> Vec<String> {
    crate::settings::get().agent_blocked_accounts
}

#[tauri::command]
pub fn set_agent_account(provider: Provider, name: String, allowed: bool) -> Result<Vec<String>, String> {
    let key = format!("{}/{}", provider.as_str(), name);
    crate::settings::update(|s| {
        s.agent_blocked_accounts.retain(|k| k != &key);
        if !allowed {
            s.agent_blocked_accounts.push(key.clone());
        }
    })?;
    Ok(crate::settings::get().agent_blocked_accounts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::ask::ask_of;
    use crate::agents::lost::{sentence, undelivered_note, worktrees_left_line};
    use crate::agents::owed::{hand_over, owe_from, retried, Owed};
    use crate::agents::persist::{
        chats_now, take_up_chats, take_up_spawns, tie_back, ties_in, ties_of, SavedChat, SavedChats, SavedSend, Ties, SESSION_RETURN_MS,
    };
    use crate::agents::read::transcripts;
    use crate::agents::registry::{parent_of, reg, register, resumes_on_restart, settings_of, waits_on_helpers, Known, DEV_CALLER};
    use crate::agents::said::said_since;
    use crate::agents::teardown::forget;
    use crate::agents::wait::{reply_text, Reserved, WaitOut, REPLY_GRACE_MS};

    fn account(provider: Provider, name: &str) -> crate::accounts::AccountInfo {
        crate::accounts::AccountInfo { provider, name: name.into(), path: format!("C:\\acc\\{name}") }
    }

    /// One window per figure, resetting far ahead.
    fn used(pairs: &[(&str, &[(&str, f64)])]) -> HashMap<String, serde_json::Value> {
        pairs
            .iter()
            .map(|(name, windows)| {
                let limits: Vec<_> = windows
                    .iter()
                    .map(|(label, pct)| serde_json::json!({ "label": label, "usedPct": pct, "resetsAt": "2999-01-01T00:00:00Z" }))
                    .collect();
                (format!("claude/{name}"), serde_json::json!({ "limits": limits }))
            })
            .collect()
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn a_list_takes_the_first_with_room_and_says_what_it_passed_over() {
        let allowed = [account(Provider::Claude, "a"), account(Provider::Claude, "b"), account(Provider::Claude, "c")];
        let usage = used(&[("a", &[("5 hours", 97.0), ("week", 10.0)]), ("b", &[("5 hours", 94.0)]), ("c", &[("5 hours", 1.0)])]);
        let (taken, choice) = pick_from_list(&names(&["a", "b", "c"]), Provider::Claude, &allowed, &usage, |_| true, 0).unwrap();
        assert_eq!(taken.name, "b", "under the line, and before c");
        assert_eq!(choice.account, "b");
        assert_eq!(choice.skipped, [SkippedAccount { account: "a".into(), why: "5 hours at 97%".into() }]);
        assert_eq!(choice.why, "the first in the list with room in every window (under 95%); the fullest, 5 hours, is at 94%");
    }

    #[test]
    fn a_list_passes_over_who_is_not_signed_in_or_not_allowed() {
        let allowed = [account(Provider::Claude, "out"), account(Provider::Claude, "in")];
        let (taken, choice) =
            pick_from_list(&names(&["blocked", "out", "in"]), Provider::Claude, &allowed, &HashMap::new(), |a| a.name == "in", 0).unwrap();
        assert_eq!(taken.name, "in");
        assert_eq!(choice.skipped.len(), 2);
        assert_eq!(choice.skipped[1].why, "not signed in");
        assert_eq!(choice.why, "the first in the list signed in; its usage is not known yet");
    }

    #[test]
    fn a_full_list_comes_down_to_the_most_room() {
        let allowed = [account(Provider::Claude, "a"), account(Provider::Claude, "b"), account(Provider::Claude, "c")];
        let usage = used(&[("a", &[("week", 100.0)]), ("b", &[("week", 96.0), ("5 hours", 20.0)]), ("c", &[("week", 99.0)])]);
        let (taken, choice) = pick_from_list(&names(&["a", "b", "c"]), Provider::Claude, &allowed, &usage, |_| true, 0).unwrap();
        assert_eq!(taken.name, "b");
        assert_eq!(choice.skipped.iter().map(|s| s.account.as_str()).collect::<Vec<_>>(), ["a", "c"]);
        assert!(choice.why.contains("the most room, at 96%"), "{}", choice.why);
    }

    #[test]
    fn a_list_with_nobody_to_take_it_is_an_error() {
        let allowed = [account(Provider::Claude, "a")];
        let e = pick_from_list(&names(&["a", "gone"]), Provider::Claude, &allowed, &HashMap::new(), |_| false, 0).err().unwrap();
        assert!(e.contains("a: not signed in") && e.contains("gone: no claude account"), "{e}");
        assert!(pick_from_list(&[], Provider::Claude, &allowed, &HashMap::new(), |_| true, 0).err().unwrap().contains("empty"));
    }

    #[test]
    fn a_window_past_its_reset_no_longer_counts() {
        let allowed = [account(Provider::Claude, "a")];
        let usage: HashMap<String, serde_json::Value> = [(
            "claude/a".to_string(),
            serde_json::json!({ "limits": [{ "label": "5 hours", "usedPct": 100, "resetsAt": "2026-01-01T00:00:00+00:00" }] }),
        )]
        .into();
        let before = chrono::DateTime::parse_from_rfc3339("2025-12-31T00:00:00Z").unwrap().timestamp_millis() as u64;
        let after = chrono::DateTime::parse_from_rfc3339("2026-01-02T00:00:00Z").unwrap().timestamp_millis() as u64;
        let pick = |now| pick_from_list(&names(&["a"]), Provider::Claude, &allowed, &usage, |_| true, now).unwrap().1;
        assert!(pick(before).why.contains("the most room, at 100%"), "spent, and the only one");
        assert!(pick(after).why.contains("reset since"), "the figure is from before the reset");
    }

    #[test]
    fn a_list_is_of_one_provider() {
        let allowed = [
            account(Provider::Claude, "c"),
            account(Provider::Codex, "x"),
            account(Provider::Claude, "both"),
            account(Provider::Codex, "both"),
        ];
        let of = |list: &[&str], asked| list_provider(&names(list), &allowed, asked, Provider::Claude);
        assert_eq!(of(&["c", "both"], None), Ok(Provider::Claude));
        assert_eq!(of(&["x", "both"], None), Ok(Provider::Codex), "the names bring their provider");
        assert_eq!(of(&["both"], None), Ok(Provider::Claude), "the caller's first");
        assert_eq!(of(&["c", "unknown"], None), Ok(Provider::Claude), "an unknown one is skipped later, with a reason");
        assert!(of(&["c", "x"], None).is_err());
        assert!(of(&["c"], Some(Provider::Codex)).is_err());
        assert_eq!(of(&["both", "x"], Some(Provider::Codex)), Ok(Provider::Codex));
    }

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
    fn read_only_net_goes_with_never_and_not_with_auto_review() {
        let spec = |approval: &str| {
            let p = SpawnParams { sandbox: Some("read-only-net".into()), approval: Some(approval.into()), ..Default::default() };
            background_spec(&p, Provider::Codex, "", "")
        };
        assert_eq!(spec("never").ok().unwrap().mode, "read-only-net");
        assert!(spec("auto-review").err().unwrap().contains("workspace-write only"), "the reviewer's sandbox is fixed");
    }

    #[test]
    fn a_worktree_still_being_made_says_so() {
        let spawned = |worktree: Worktree, worktree_pending: bool| {
            serde_json::to_value(Spawned {
                account_choice: None,
                id: "w".into(),
                turns_ended: 0,
                settings: serde_json::Value::Null,
                started: true,
                screen: None,
                ask: None,
                starting: false,
                warnings: Vec::new(),
                same_folder: Vec::new(),
                worktree,
                worktree_pending,
            })
            .unwrap()
        };
        let pending = spawned(Worktree::default(), true);
        assert_eq!(pending["worktreePending"], true);
        assert!(pending.get("worktree").is_none());
        let made = spawned(Worktree { path: Some("C:\\w".into()), ..Default::default() }, false);
        assert_eq!(made["worktree"], "C:\\w");
        assert!(made.get("worktreePending").is_none());
    }

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

    #[test]
    fn a_failed_turn_quotes_nothing_as_its_verdict() {
        use crate::activity::{on_hook, HookEvent};
        let path = std::env::temp_dir().join(format!("luna-verdict-{}.jsonl", std::process::id()));
        std::fs::write(
            &path,
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"All done and fixed.\"}]}}\n",
        )
        .unwrap();
        for (id, failed) in [("vd-fine", false), ("vd-failed", true)] {
            register(id, Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, false, serde_json::json!({}));
            transcripts().lock().unwrap().insert(id.into(), path.clone());
            on_hook(id, HookEvent::Busy);
            on_hook(id, if failed { HookEvent::Failed("rate_limit") } else { HookEvent::Idle });
        }
        let fine = line_for("vd-fine");
        assert!(fine.contains("finished its turn") && fine.contains("It begins: \"All done and fixed.\""), "{fine}");
        let failed = line_for("vd-failed");
        assert!(failed.contains("stopped on an error: rate_limit"), "{failed}");
        assert!(failed.contains("It ended with 1 message"), "what it said before is still named: {failed}");
        assert!(!failed.contains("It begins") && !failed.contains("All done"), "its words are no verdict on it: {failed}");
        for id in ["vd-fine", "vd-failed"] {
            forget(id);
            crate::activity::forget(id);
        }
        let _ = std::fs::remove_file(&path);
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
    fn a_wait_out_holds_the_notice_for_a_while() {
        let none = serde_json::json!({});
        register("w-parent", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone());
        register("w-child", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("w-parent"), false, none);
        owe_from("w-parent", "w-child", 0, 0);
        let pty = crate::pty::manager();
        let key = ("w-parent".to_string(), "w-child".to_string());
        let o = || reg().lock().unwrap().owed.get(&key).copied().unwrap();
        assert!(!held_for_wait(pty, "w-parent", "w-child", &o()), "no wait out, nothing held");
        {
            let _out = WaitOut::start("w-parent", "w-child");
            // Not running, so there is news: the wait has WAIT_HOLD_MS to say it.
            assert!(held_for_wait(pty, "w-parent", "w-child", &o()));
            reg().lock().unwrap().owed.get_mut(&key).unwrap().ready_at = Some(now_ms() - WAIT_HOLD_MS - 1);
            assert!(!held_for_wait(pty, "w-parent", "w-child", &o()), "a wait that sat on news is let go");
        }
        assert!(!reg().lock().unwrap().waits.contains_key(&key), "the count goes with the call");
        {
            // A wait on several holds it however long the others take.
            let _reserved = Reserved::start("w-parent", "w-child");
            assert!(held_for_wait(pty, "w-parent", "w-child", &o()));
        }
        assert!(!held_for_wait(pty, "w-parent", "w-child", &o()), "and no longer");
    }

    /// The loop of that night: two chats that wrote to each other, a stray
    /// process under each. Every notice starts a turn in the chat it goes into,
    /// and that turn's end is what the other is owed. Driven by the notifier's
    /// own decisions, with a turn counted the moment its notice goes in.
    #[test]
    fn two_chats_with_a_job_each_do_not_wake_each_other_forever() {
        use crate::activity::Turn;
        let (a, b) = ("pp-a", "pp-b");
        let key = |caller: &str, id: &str| (caller.to_string(), id.to_string());
        let mut turns: HashMap<&str, u32> = HashMap::from([(a, 1), (b, 1)]);
        owe_from(a, b, 0, 0);
        owe_from(b, a, 0, 0);
        let mut notices = 0;
        // One pass over both debts, with or without the jobs; returns how many
        // notices went.
        let pass = |job: bool, turns: &mut HashMap<&str, u32>| {
            let mut sent = 0;
            for (caller, id) in [(a, b), (b, a)] {
                let Some(o) = reg().lock().unwrap().owed.get(&key(caller, id)).copied() else { continue };
                let ended = turns[id];
                let v = verdict(&o, ended, Some(Turn::Idle), job, real_turn_past(id, caller, o.baseline, ended, false, || true));
                let settled = match v {
                    Verdict::OnlyNotices => {
                        reg().lock().unwrap().owed.get_mut(&key(caller, id)).unwrap().baseline = ended;
                        continue;
                    }
                    Verdict::TurnOver if job => Settled::Pending(ended, 0),
                    Verdict::TurnOver | Verdict::JobOver => Settled::Answered,
                    Verdict::Nothing | Verdict::AtWork => continue,
                };
                sent += 1;
                let mut r = reg().lock().unwrap();
                notice_turn(&mut r, caller, turns[caller] + 1, &[id.to_string()]);
                match settled {
                    Settled::Pending(n, c) => {
                        r.owed.insert(key(caller, id), Owed::pending(n, c));
                    }
                    _ => {
                        r.owed.remove(&key(caller, id));
                    }
                }
                drop(r);
                *turns.get_mut(caller).unwrap() += 1;
            }
            sent
        };
        for _ in 0..500 {
            notices += pass(true, &mut turns);
        }
        assert_eq!(notices, 2, "\"NOT done yet\" once per debt, whatever the turns on those notices");
        let at_rest = turns.clone();
        for _ in 0..500 {
            notices += pass(true, &mut turns);
        }
        assert_eq!(turns, at_rest, "no notice, no turn: the chats are left at rest");
        // The jobs end: each is told once, and that is the end of it.
        for _ in 0..500 {
            notices += pass(false, &mut turns);
        }
        assert_eq!(notices, 4, "the job's end is news, once per debt");
        assert!(!reg().lock().unwrap().owed.contains_key(&key(a, b)));
        assert!(!reg().lock().unwrap().owed.contains_key(&key(b, a)));
        forget(a);
        forget(b);
    }

    #[test]
    fn told_it_is_not_done_a_caller_hears_only_of_news() {
        use crate::activity::Turn::{Busy, Idle, Waiting};
        let told = Owed::pending(5, 0);
        assert_eq!(verdict(&Owed::from(5, 0), 6, Some(Idle), true, false), Verdict::TurnOver, "a first turn end is news, job or not");
        assert_eq!(verdict(&Owed::from(5, 0), 5, Some(Idle), false, false), Verdict::Nothing);
        assert_eq!(verdict(&told, 5, Some(Idle), true, false), Verdict::AtWork, "the job runs on");
        assert_eq!(verdict(&told, 5, Some(Idle), false, false), Verdict::JobOver, "it ended with no turn");
        assert_eq!(verdict(&told, 5, Some(Busy), false, false), Verdict::Nothing, "a turn runs: its end will say");
        assert_eq!(verdict(&told, 5, Some(Waiting), false, false), Verdict::Nothing, "on a prompt: its own notice");
        assert_eq!(verdict(&told, 6, Some(Idle), true, false), Verdict::OnlyNotices, "a turn on a notice is no news");
        assert_eq!(verdict(&told, 6, Some(Idle), true, true), Verdict::TurnOver, "a turn of its own is");
        assert_eq!(verdict(&told, 6, Some(Busy), true, true), Verdict::Nothing, "not before it is over");
    }

    #[test]
    fn a_turn_counts_for_a_caller_told_it_is_not_done_unless_a_notice_started_it() {
        let mut r = reg().lock().unwrap();
        notice_turn(&mut r, "rt-s", 4, &["rt-other".into()]);
        drop(r);
        let past = |after, ended| real_turn_past("rt-s", "rt-c", after, ended, false, || false);
        assert!(!past(3, 4), "the one a notice started");
        assert!(past(3, 5), "and the one after it, which none did");
        assert!(past(2, 4), "or one before");
        assert!(!past(4, 4), "none past the count");
        // A notice that did not start a turn is not marked: at worst one notice more.
        assert!(real_turn_past("rt-unmarked", "rt-c", 0, 1, false, || false));
        forget("rt-s");
    }

    /// A top chat, a helper with tools, and the helper's own helper: the first
    /// answer of the middle one is "I have started it", and what it makes of
    /// its helper's answer is what the top chat asked for.
    #[test]
    fn a_turn_on_its_own_helpers_news_reaches_the_caller_above() {
        use crate::activity::Turn::Idle;
        let none = serde_json::json!({});
        register("tr-top", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone());
        register("tr-mid", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("tr-top"), true, none.clone());
        register("tr-low", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("tr-mid"), false, none.clone());
        register("tr-low2", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("tr-mid"), false, none);
        owe_from("tr-mid", "tr-low", 0, 0);
        assert!(waits_on_helpers("tr-mid", "tr-top"));
        assert!(!waits_on_helpers("tr-mid", "tr-low"), "not counting the one that would be told");
        assert!(!waits_on_helpers("tr-top", ""), "answers owed to it, not those owed by it");
        owe_from("tr-mid", "tr-low2", 0, 0);

        // Its first turn ended: told it is not done (`notice_line`), the debt goes on.
        let told = Owed::pending(1, 0);
        // The first helper's notice starts its turn 2, with the other still out.
        reg().lock().unwrap().owed.remove(&("tr-mid".to_string(), "tr-low".to_string()));
        notice_turn(&mut reg().lock().unwrap(), "tr-mid", 2, &["tr-low".into()]);
        let waits = waits_on_helpers("tr-mid", "tr-top");
        assert!(waits);
        assert_eq!(
            verdict(&told, 2, Some(Idle), waits, real_turn_past("tr-mid", "tr-top", 1, 2, waits, || false)),
            Verdict::OnlyNotices,
            "quiet while one is out"
        );
        assert_eq!(
            verdict(&told, 2, Some(Idle), waits, real_turn_past("tr-mid", "tr-top", 1, 2, waits, || true)),
            Verdict::TurnOver,
            "unless what it made of that is news for its parent"
        );
        // The last one's notice starts its turn 3: this is the result.
        reg().lock().unwrap().owed.remove(&("tr-mid".to_string(), "tr-low2".to_string()));
        notice_turn(&mut reg().lock().unwrap(), "tr-mid", 3, &["tr-low2".into()]);
        let waits = waits_on_helpers("tr-mid", "tr-top");
        assert!(!waits);
        let told = Owed::pending(2, 0);
        assert_eq!(
            verdict(&told, 3, Some(Idle), waits, real_turn_past("tr-mid", "tr-top", 2, 3, waits, || false)),
            Verdict::TurnOver,
            "it reaches the top"
        );
        // A notice about something else, or about the caller itself, does not count.
        notice_turn(&mut reg().lock().unwrap(), "tr-mid", 4, &["tr-stranger".into()]);
        assert!(!real_turn_past("tr-mid", "tr-top", 3, 4, false, || false));
        assert!(!real_turn_past("tr-mid", "tr-top", 3, 4, false, || true), "news or not");
        // A notice about its helper and someone else: not all its own helpers' doing.
        notice_turn(&mut reg().lock().unwrap(), "tr-mid", 6, &["tr-low".into(), "tr-stranger".into()]);
        assert!(!real_turn_past("tr-mid", "tr-top", 5, 6, true, || true));
        notice_turn(&mut reg().lock().unwrap(), "tr-low", 1, &["tr-mid".into()]);
        assert!(!real_turn_past("tr-low", "tr-mid", 0, 1, false, || false), "not a helper of its own");
        notice_turn(&mut reg().lock().unwrap(), "tr-mid", 5, &["tr-low".into()]);
        assert!(!real_turn_past("tr-mid", "tr-low", 4, 5, false, || false), "its own notice coming back");
        for id in ["tr-low", "tr-low2", "tr-mid", "tr-top"] {
            forget(id);
        }
    }

    /// The QA helper of 0.8.1: its helpers' notices start its turns, each ends
    /// on a report its parent needs now, and its parent heard none until the
    /// last helper had answered.
    #[test]
    fn a_report_made_on_its_own_helpers_news_reaches_its_parent_once() {
        let none = serde_json::json!({});
        register("ir-top", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone());
        register(
            "ir-mid",
            Provider::Claude,
            "C:\\p",
            "C:\\acc\\anthropic\\one",
            Some("ir-top"),
            true,
            serde_json::json!({ "background": true }),
        );
        register("ir-low", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("ir-mid"), false, none.clone());
        register("ir-low2", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("ir-mid"), false, none.clone());
        register("ir-peer", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none);
        crate::background::idle_for_test("ir-mid", Provider::Claude, 2, false);
        owe_from("ir-mid", "ir-low2", 0, 0);
        let said = |t: &str| {
            format!("{}\n", serde_json::json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": t }] } }))
        };
        let notice = format!(
            "{}\n",
            serde_json::json!({ "type": "user", "message": { "role": "user", "content": format!("{NOTICE_TAG} Your helper ir-low finished its turn.") } })
        );
        let before = said("Started both helpers.");
        let path = std::env::temp_dir().join(format!("luna-ir-mid-{}.jsonl", std::process::id()));
        transcripts().lock().unwrap().insert("ir-mid".into(), path.clone());
        // Its turn 2 is on ir-low's notice, with ir-low2 still out.
        notice_turn(&mut reg().lock().unwrap(), "ir-mid", 2, &["ir-low".into()]);
        let known = reg().lock().unwrap().known.get("ir-mid").cloned().unwrap();
        let pty = crate::pty::manager();
        let line = |caller: &str, transcript: &str| {
            std::fs::write(&path, format!("{before}{transcript}")).unwrap();
            notice_line(pty, caller, "Your helper", "ir-mid", &known, Owed::pending(1, before.len() as u64))
        };
        let report = format!("Phase 1 is in: kill the core before 12:35 UTC. {}", "Evidence. ".repeat(45));

        let Some((Some(text), Settled::Pending(2, cursor))) = line("ir-top", &format!("{notice}{}", said(&report))) else {
            panic!("its parent hears")
        };
        assert!(text.contains("ended another turn") && text.contains("helpers of its own have yet to answer it"), "{text}");
        assert!(text.contains("It begins: \"Phase 1 is in"), "{text}");
        assert_eq!(cursor, std::fs::metadata(&path).unwrap().len(), "and what follows is what it hears next");
        let file = text.split("the answer is in ").nth(1).and_then(|t| t.split(", its report first").next()).unwrap();
        assert!(std::fs::read_to_string(file).unwrap().starts_with("Phase 1 is in"));
        let _ = std::fs::remove_file(file);
        // A line or two is no news worth a turn: it comes with the answer.
        assert!(matches!(line("ir-top", &format!("{notice}{}", said("Noted; waiting on ir-low2."))), Some((None, Settled::Skip(2)))));
        // Only its parent: a session that wrote to it is told of its own turns alone.
        assert!(matches!(line("ir-peer", &format!("{notice}{}", said(&report))), Some((None, Settled::Skip(2)))));
        // A turn on a notice about anyone but its own helpers is no news for the parent.
        notice_turn(&mut reg().lock().unwrap(), "ir-mid", 2, &["ir-low".into(), "ir-peer".into()]);
        reg().lock().unwrap().notice_turns.get_mut("ir-mid").unwrap().remove(0);
        assert!(matches!(line("ir-top", &format!("{notice}{}", said(&report))), Some((None, Settled::Skip(2)))));
        let _ = std::fs::remove_file(&path);
        crate::background::gone_for_test("ir-mid");
        for id in ["ir-low", "ir-low2", "ir-mid", "ir-top", "ir-peer"] {
            forget(id);
        }
    }

    /// The loop of that night, between a chat and its own helper that wrote to
    /// each other, every turn on a notice saying plenty: what goes up to the
    /// parent never comes back down.
    #[test]
    fn a_parent_and_its_helper_that_wrote_to_each_other_do_not_wake_each_other_forever() {
        use crate::activity::Turn;
        let (p, h) = ("pq-parent", "pq-helper");
        let none = serde_json::json!({});
        register(p, Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone());
        register(h, Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some(p), true, none);
        let key = |caller: &str, id: &str| (caller.to_string(), id.to_string());
        let mut turns: HashMap<&str, u32> = HashMap::from([(p, 1), (h, 1)]);
        owe_from(p, h, 0, 0);
        owe_from(h, p, 0, 0);
        let mut notices = 0;
        for _ in 0..500 {
            for (caller, id) in [(p, h), (h, p)] {
                let Some(o) = reg().lock().unwrap().owed.get(&key(caller, id)).copied() else { continue };
                let ended = turns[id];
                // Always news, wherever its parent is the one to hear it.
                let upward = || parent_of(id).as_deref() == Some(caller);
                let v = verdict(
                    &o,
                    ended,
                    Some(Turn::Idle),
                    true,
                    o.told_pending && real_turn_past(id, caller, o.baseline, ended, false, upward),
                );
                match v {
                    Verdict::OnlyNotices => reg().lock().unwrap().owed.get_mut(&key(caller, id)).unwrap().baseline = ended,
                    Verdict::TurnOver => {
                        notices += 1;
                        let mut r = reg().lock().unwrap();
                        notice_turn(&mut r, caller, turns[caller] + 1, &[id.to_string()]);
                        r.owed.insert(key(caller, id), Owed::pending(ended, 0));
                        drop(r);
                        *turns.get_mut(caller).unwrap() += 1;
                    }
                    _ => {}
                }
            }
        }
        assert_eq!(notices, 2, "\"NOT done yet\" once each, and quiet after");
        forget(h);
        forget(p);
    }

    /// A helper announced as not done, its job over, with a transcript on disk
    /// holding what `said` past `cursor`, the debt `caller` is owed.
    fn job_over_helper(id: &str, caller: &str, said: &str) -> (Known, Owed, std::path::PathBuf) {
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

    /// The night of 0.8.1: a `tail -F` its own tool left running after its turn
    /// kept a helper "not done" for 14 hours, its caller told nothing. Idle is
    /// no proof of an end: told once, and the debt waits for the end itself.
    #[test]
    fn what_a_turn_left_is_named_and_an_idle_one_is_told_of_once() {
        let (known, o, path) = job_over_helper("lj-s", "lj-c", "");
        let pty = crate::pty::manager();
        let line = |o: Owed| notice_line(pty, "lj-c", "Your helper", "lj-s", &known, o);
        // Its first turn's end, with the tail just left: not done, and what runs is named.
        crate::background::left_for_test("lj-s", &[("tail.exe", 22104, 0)]);
        let mut first = Owed { rev: o.rev, ..Owed::from(2, o.cursor) };
        first.held = Some((3, now_ms() - BACKGROUND_GRACE_MS));
        let Some((Some(text), Settled::Pending(3, _))) = line(first) else { panic!("not done yet") };
        assert!(text.contains("NOT done yet: a job it started still runs in the background (tail.exe (pid 22104))"), "{text}");
        // Quiet for a while, but not for long: still a job, nothing to say.
        crate::background::left_for_test("lj-s", &[("tail.exe", 22104, IDLE_JOB_MS - 60_000)]);
        assert!(matches!(line(o), None | Some((None, _))));
        // Half an hour of nothing: said once, how to end it with it, and still owed.
        crate::background::left_for_test("lj-s", &[("tail.exe", 22104, IDLE_JOB_MS + 60_000)]);
        let Some((Some(text), how @ Settled::Idle(_))) = line(o) else { panic!("its idle job is told of") };
        assert!(rides_since(&how).is_some_and(|since| now_ms() - since < 2 * 60_000), "and rides, from when it passed the mark");
        assert!(text.contains("only idle processes left, which have done nothing for 31 min: tail.exe (pid 22104)"), "{text}");
        assert!(text.contains("luna_kill ends them") && text.contains("You will still hear when they end"), "{text}");
        settle_told(&mut reg().lock().unwrap(), ("lj-c".to_string(), "lj-s".to_string()), how, &o);
        let told = reg().lock().unwrap().owed.get(&("lj-c".to_string(), "lj-s".to_string())).copied().expect("the debt stays open");
        assert!(told.told_idle && told.told_pending);
        assert!(matches!(line(told), None | Some((None, _))), "said once");
        assert!(!job_over(&told, crate::activity::turn_state("lj-s")), "and no end for a wait either");
        // Its end: that is the news the debt waited for.
        crate::background::left_for_test("lj-s", &[]);
        let mut ended = told;
        ended.held = Some((3, now_ms() - BACKGROUND_GRACE_MS));
        let Some((Some(text), Settled::NoNews(_))) = line(ended) else { panic!("the job's end") };
        assert!(text.contains("has nothing left running now"), "{text}");
        crate::background::gone_for_test("lj-s");
        forget("lj-s");
        let _ = std::fs::remove_file(&path);
    }

    /// A job's end with no word from it, or an idle job, is told of with the
    /// next notice; on its own only once nothing else can bring one.
    #[test]
    fn a_line_with_no_news_rides_with_the_next_notice() {
        let now = 100 * RIDE_MAX_MS;
        let fresh = now - 30_000;
        assert!(!riders_go(false, false, fresh, now, 1), "another helper at work: it waits for that one's notice");
        assert!(riders_go(true, false, fresh, now, 1), "a notice is going in anyway");
        assert!(riders_go(false, true, fresh, now, 1), "nothing else owed: this is the last it will hear");
        assert!(!riders_go(false, false, now - RIDE_MAX_MS + 1, now, 1));
        assert!(riders_go(false, false, now - RIDE_MAX_MS, now, 1), "it may be waiting on this very job");
        // An open debt that can bring nothing (a stopped pair, a chat at rest), and
        // all is idle: nothing is coming.
        assert!(riders_go(false, false, fresh, now, 0));
        // The line settles a debt as it always did; only going out does.
        let key = ("rd-c".to_string(), "rd-s".to_string());
        let o = Owed::pending(3, 0);
        reg().lock().unwrap().owed.insert(key.clone(), o);
        assert!(counts_for_breaker(&Settled::NoNews(1)) && !counts_for_breaker(&Settled::Idle(1)));
        settle_told(&mut reg().lock().unwrap(), key.clone(), Settled::Idle(1), &o);
        assert!(reg().lock().unwrap().owed.get(&key).is_some_and(|o| o.told_idle), "owed on");
        settle_told(&mut reg().lock().unwrap(), key.clone(), Settled::NoNews(1), &o);
        assert!(!reg().lock().unwrap().owed.contains_key(&key), "settled");
    }

    /// News does not wait: words since the last notice, or a core that took the
    /// job. And what happens to a held line's session replaces it.
    #[test]
    fn a_jobs_end_with_something_to_say_goes_at_once() {
        let said = "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"All green.\"}]}}\n";
        let (known, o, path) = job_over_helper("nn-s", "nn-c", said);
        let pty = crate::pty::manager();
        let stood = Owed { held: Some((3, now_ms() - BACKGROUND_GRACE_MS)), ..o };
        assert!(matches!(notice_line(pty, "nn-c", "Your helper", "nn-s", &known, stood), Some((Some(_), Settled::Answered))), "it wrote");
        let (known, o, path2) = job_over_helper("nn-q", "nn-c", "");
        let stood = Owed { held: Some((3, now_ms() - BACKGROUND_GRACE_MS)), ..o };
        assert!(matches!(notice_line(pty, "nn-c", "Your helper", "nn-q", &known, stood), Some((Some(_), Settled::NoNews(_)))));
        let went = Owed { job_went: true, ..stood };
        let Some((Some(text), Settled::Answered)) = notice_line(pty, "nn-c", "Your helper", "nn-q", &known, went) else {
            panic!("a core took it")
        };
        assert!(text.contains("that Luna's core has restarted"), "{text}");
        // Held past its grace, a wait still gets it: the debt is open, on the one clock.
        let key = ("nn-c".to_string(), "nn-q".to_string());
        reg().lock().unwrap().owed.insert(key.clone(), stood);
        assert!(job_end_stood(&key, 3, true));
        // At work again: no line, and its end counts from nothing.
        crate::background::left_for_test("nn-q", &[("build.exe", 4242, 0)]);
        assert!(matches!(notice_line(pty, "nn-c", "Your helper", "nn-q", &known, stood), Some((None, Settled::Unhold))));
        for id in ["nn-s", "nn-q"] {
            crate::background::gone_for_test(id);
            forget(id);
        }
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&path2);
    }

    /// 2 November: a helper's pwsh hung for 25 minutes mid-turn on a process a
    /// failed test had left, and nothing told its caller, who had ended its turn
    /// on "the notice comes when it is done". A quiet turn is told of once.
    #[test]
    fn a_turn_gone_quiet_is_told_of_once() {
        let none = serde_json::json!({});
        register("qt-s", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("qt-c"), false, none);
        crate::background::idle_for_test("qt-s", Provider::Claude, 3, false);
        let o = Owed::from(3, 0);
        reg().lock().unwrap().owed.insert(("qt-c".to_string(), "qt-s".to_string()), o);
        let known = reg().lock().unwrap().known.get("qt-s").cloned().unwrap();
        let pty = crate::pty::manager();
        let line = |o: Owed| notice_line(pty, "qt-c", "Your helper", "qt-s", &known, o);
        // At work and writing: nothing to say.
        crate::background::quiet_for_test("qt-s", QUIET_TURN_MS - 60_000);
        assert!(matches!(line(o), None | Some((None, _))));
        // Half an hour of nothing: said once for this turn, and still owed.
        crate::background::quiet_for_test("qt-s", QUIET_TURN_MS + 60_000);
        let Some((Some(text), Settled::Quiet(3))) = line(o) else { panic!("a quiet turn is news") };
        assert!(text.contains("mid-turn and has written nothing for 31 min") && text.contains("luna_kill stops the turn"), "{text}");
        settle_told(&mut reg().lock().unwrap(), ("qt-c".to_string(), "qt-s".to_string()), Settled::Quiet(3), &o);
        let told = reg().lock().unwrap().owed.get(&("qt-c".to_string(), "qt-s".to_string())).copied().expect("still owed");
        assert_eq!(told.told_quiet, Some(3));
        assert!(matches!(line(told), None | Some((None, _))), "once per turn");
        assert!(!counts_for_breaker(&Settled::Quiet(3)), "no turn's end");
        crate::background::gone_for_test("qt-s");
        forget("qt-s");
    }

    /// 4 November: helpers ran test lanes of 45 minutes under a timeout of their
    /// own, and their callers were woken by each as a turn gone quiet. One that
    /// waits on a command hears of it 5 min past the command's own timeout,
    /// Codex's as Claude Code's; a command that has finished holds nothing back.
    #[test]
    fn a_quiet_turn_waits_for_the_timeout_of_its_command() {
        use serde_json::json;
        let min = 60_000;
        let pty = crate::pty::manager();
        for provider in Provider::ALL {
            let (caller, id) = (format!("dq-c-{}", provider.as_str()), format!("dq-s-{}", provider.as_str()));
            register(&id, provider, "C:\\p", "C:\\acc\\one", Some(caller.as_str()), false, json!({}));
            crate::background::idle_for_test(&id, provider, 3, false);
            let o = Owed::from(3, 0);
            reg().lock().unwrap().owed.insert((caller.clone(), id.clone()), o);
            let known = reg().lock().unwrap().known.get(&id).cloned().unwrap();
            // Quiet since a command with a timeout of 40 min started, which has
            // since come back when `done`.
            let quiet_on = |quiet: u64, done: bool| {
                let at = crate::background::iso(now_ms() - quiet).unwrap();
                let [call, back] = match provider {
                    Provider::Claude => [
                        json!({ "type": "assistant", "timestamp": at, "message": { "content": [{ "type": "tool_use", "id": "t1", "name": "Bash", "input": { "command": "./lane.sh", "timeout": 40 * min } }] } }),
                        json!({ "type": "user", "message": { "content": [{ "type": "tool_result", "tool_use_id": "t1", "content": "ok" }] } }),
                    ],
                    Provider::Codex => [
                        json!({ "timestamp": at, "type": "response_item", "payload": { "type": "custom_tool_call", "name": "exec", "call_id": "c1",
                            "input": format!("// @exec: {{\"yield_time_ms\": {}}}\ntext(await tools.exec_command({{cmd:\"./lane.sh\"}}));\n", 40 * min) } }),
                        json!({ "timestamp": at, "type": "response_item", "payload": { "type": "custom_tool_call_output", "call_id": "c1", "output": "Script completed" } }),
                    ],
                };
                let text = if done { format!("{call}\n{back}\n") } else { format!("{call}\n") };
                let path = std::env::temp_dir().join(format!("luna-{id}-{quiet}-{done}-{}.jsonl", std::process::id()));
                std::fs::write(&path, text).unwrap();
                transcripts().lock().unwrap().insert(id.clone(), path.clone());
                crate::background::quiet_for_test(&id, quiet);
                let said = notice_line(pty, &caller, "Your helper", &id, &known, o);
                let _ = std::fs::remove_file(&path);
                said
            };
            // Half an hour of nothing, its command 9 min short of its timeout.
            assert!(matches!(quiet_on(31 * min, false), None | Some((None, _))), "{id}: it is still within its time");
            let Some((Some(text), Settled::Quiet(3))) = quiet_on(45 * min + 30_000, false) else {
                panic!("{id}: 5 min past its timeout is news")
            };
            assert!(text.contains("written nothing for 45 min, and the command it waits on is 5 min past its own timeout"), "{text}");
            let Some((Some(text), Settled::Quiet(3))) = quiet_on(31 * min, true) else {
                panic!("{id}: a finished command holds nothing back")
            };
            assert!(text.contains("written nothing for 31 min. A command it runs may hang"), "{text}");
            crate::background::gone_for_test(&id);
            forget(&id);
        }
    }

    #[test]
    fn a_jobs_end_brings_what_was_said_on_notices_since() {
        let prompt = "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"[Luna notice]\"}}\n";
        let result = "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"Here are the real results.\"}]}}\n";
        let (known, o, path) = job_over_helper("jr-s", "jr-c", &format!("{prompt}{result}"));
        let pty = crate::pty::manager();
        // As `wait` puts it for `job_done`.
        assert_eq!(
            reply_text("jr-s", &said_since(pty, "jr-s", Provider::Claude, o.cursor), true).as_deref(),
            Some("Here are the real results.")
        );
        // The same as the notice brings: nothing is settled without its report.
        let mut stood = o;
        stood.held = Some((3, now_ms() - BACKGROUND_GRACE_MS));
        let Some((Some(line), Settled::Answered)) = notice_line(pty, "jr-c", "Your helper", "jr-s", &known, stood) else {
            panic!("the job's end is news")
        };
        assert!(line.contains("it wrote 1 message since your last notice"), "{line}");
        // Nothing since: nothing to hand over, not an old message.
        let (_, o2, path2) = job_over_helper("jr-q", "jr-c", "");
        assert_eq!(reply_text("jr-q", &said_since(pty, "jr-q", Provider::Claude, o2.cursor), true), None);
        for id in ["jr-s", "jr-q"] {
            crate::background::gone_for_test(id);
            forget(id);
        }
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&path2);
    }

    #[test]
    fn a_jobs_end_goes_to_a_wait_that_is_out_and_not_to_a_notice_too() {
        let (known, o, path) = job_over_helper("jw-s", "jw-c", "");
        let key = ("jw-c".to_string(), "jw-s".to_string());
        let pty = crate::pty::manager();
        let debt = || reg().lock().unwrap().owed.get(&key).copied().unwrap();
        let out = WaitOut::start("jw-c", "jw-s");
        // The one clock: whoever looks first starts it, the other reads it.
        assert!(held_for_wait(pty, "jw-c", "jw-s", &o), "the wait has it reserved");
        let started = debt().held.expect("the hold starts the clock");
        assert!(!job_end_stood(&key, 3, true), "the wait goes by the same clock");
        assert_eq!(debt().held, Some(started), "and does not start its own");
        assert_eq!(debt().ready_at, None, "not news yet");
        // Past where a notice used to give up on the wait, still reserved.
        reg().lock().unwrap().owed.get_mut(&key).unwrap().held = Some((3, now_ms() - BACKGROUND_GRACE_MS + 5_000));
        assert!(held_for_wait(pty, "jw-c", "jw-s", &debt()), "through the whole grace");
        assert!(!job_end_stood(&key, 3, true));
        // The grace is over: the wait hands it over now, with its hold to do it in.
        reg().lock().unwrap().owed.get_mut(&key).unwrap().held = Some((3, now_ms() - BACKGROUND_GRACE_MS));
        assert!(job_end_stood(&key, 3, true), "the wait's moment");
        assert!(held_for_wait(pty, "jw-c", "jw-s", &debt()), "and the notice still holds for it");
        assert!(debt().ready_at.is_some(), "from here on it is WAIT_HOLD_MS");
        // A wait that sat on it past that has been let go: the notice is free.
        reg().lock().unwrap().owed.get_mut(&key).unwrap().ready_at = Some(now_ms() - WAIT_HOLD_MS - 1);
        assert!(!held_for_wait(pty, "jw-c", "jw-s", &debt()));
        // With the wait gone the notice goes by the clock the wait left running.
        drop(out);
        let stood = debt();
        assert!(!held_for_wait(pty, "jw-c", "jw-s", &stood));
        assert!(matches!(notice_line(pty, "jw-c", "Your helper", "jw-s", &known, stood), Some((Some(_), Settled::NoNews(_)))));
        // A job back at work starts it over.
        assert!(!job_end_stood(&key, 3, false));
        assert_eq!(debt().held, None);
        assert!(!job_end_stood(&key, 3, true), "from nothing");
        // A notice settled it first: the wait is not left counting for ever.
        reg().lock().unwrap().owed.remove(&key);
        assert!(job_end_stood(&key, 3, true));
        crate::background::gone_for_test("jw-s");
        forget("jw-s");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_breaker_lets_so_many_notices_through_and_stops_the_pair() {
        let (caller, id) = ("br-a", "br-b");
        let pty = crate::pty::manager();
        let t0 = now_ms();
        for i in 0..BREAKER_NOTICES - 1 {
            assert!(!pair_stopped(pty, caller, id));
            assert!(!last_notice(caller, id, t0));
            assert!(!count_notice(&mut reg().lock().unwrap(), caller, id, t0), "notice {i}");
        }
        assert!(last_notice(caller, id, t0), "it says so as it goes");
        assert!(count_notice(&mut reg().lock().unwrap(), caller, id, t0));
        assert!(pair_stopped(pty, caller, id), "nothing more for this pair");
        assert!(!pair_stopped(pty, id, caller), "the other way round is a pair of its own");
        forget(caller);
        assert!(!pair_stopped(pty, caller, id), "gone with the chat");
    }

    #[test]
    fn a_stopped_pair_still_hears_of_prompts_and_exits_and_nothing_else() {
        let (known, o, path) = job_over_helper("bp-s", "bp-c", "");
        let pty = crate::pty::manager();
        let t0 = now_ms();
        for _ in 0..BREAKER_NOTICES {
            count_notice(&mut reg().lock().unwrap(), "bp-c", "bp-s", t0);
        }
        assert!(pair_stopped(pty, "bp-c", "bp-s"));
        // A job's end or a turn's, which is all the notifier would have for a
        // stopped pair: nothing.
        assert!(standing_line(pty, "bp-c", "Your helper", "bp-s", &known, o).is_none());
        // Held on a prompt, it is told, or the helper sits there until it times out.
        crate::background::in_turn_for_test("bp-s");
        let asks = std::thread::spawn(|| {
            crate::background::permission("bp-s", &serde_json::json!({ "tool_name": "Bash", "input": { "command": "ls" } }))
        });
        for _ in 0..200 {
            if ask_of("bp-s").is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let Some((Some(line), Settled::Waiting)) = standing_line(pty, "bp-c", "Your helper", "bp-s", &known, o) else {
            panic!("a prompt goes through")
        };
        assert!(line.contains("asks for permission to run Bash: ls") && line.contains("askId"), "{line}");
        crate::background::kill("bp-s");
        let _ = asks.join();
        // And gone is gone.
        crate::background::gone_for_test("bp-s");
        assert!(matches!(standing_line(pty, "bp-c", "Your helper", "bp-s", &known, o), Some((Some(_), Settled::Exited))));
        forget("bp-s");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_breaker_counts_only_what_falls_within_its_window() {
        let (caller, id) = ("bw-a", "bw-b");
        let t0 = now_ms();
        for _ in 0..BREAKER_NOTICES - 1 {
            assert!(!count_notice(&mut reg().lock().unwrap(), caller, id, t0));
        }
        // A notice now and then over a long job, as real use has it.
        let later = t0 + BREAKER_WINDOW_MS;
        assert!(!last_notice(caller, id, later), "the old ones have aged out");
        assert!(!count_notice(&mut reg().lock().unwrap(), caller, id, later));
        forget(caller);
    }

    #[test]
    fn the_breaker_counts_turn_ends_and_not_prompts() {
        assert!(counts_for_breaker(&Settled::Answered));
        assert!(counts_for_breaker(&Settled::Pending(1, 0)));
        assert!(!counts_for_breaker(&Settled::Waiting), "a busy helper asks often, and holds until answered");
        assert!(!counts_for_breaker(&Settled::Asked(1)));
        assert!(!counts_for_breaker(&Settled::Exited));
    }

    /// `notice_line` on a session known only by its hooks, the debt past the
    /// grace it gives a turn with no reply yet.
    fn line_for(id: &str) -> String {
        let pty = crate::pty::manager();
        let known = reg().lock().unwrap().known.get(id).cloned().unwrap();
        let mut o = Owed::from(0, 0);
        o.held = Some((1, now_ms() - REPLY_GRACE_MS));
        match notice_line(pty, "its-caller", "Your helper", id, &known, o) {
            Some((Some(line), Settled::Answered)) => line,
            _ => panic!("a turn is over: there is something to say"),
        }
    }

    #[test]
    fn a_turn_that_failed_is_not_reported_as_finished() {
        use crate::activity::{on_hook, HookEvent};
        let none = serde_json::json!({});
        register("fl-limit", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, false, none.clone());
        on_hook("fl-limit", HookEvent::Busy);
        on_hook("fl-limit", HookEvent::Failed("rate_limit"));
        let line = line_for("fl-limit");
        assert!(line.starts_with("Your helper stopped on an error: rate_limit."), "{line}");
        assert!(!line.contains("finished"), "{line}");
        // No error known and nothing to read: that it is not known is what it says.
        register("fl-lost", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, false, none);
        on_hook("fl-lost", HookEvent::Busy);
        on_hook("fl-lost", HookEvent::Idle);
        let line = line_for("fl-lost");
        assert!(line.contains("cannot find its transcript"), "{line}");
        assert!(!line.contains("without a reply"), "{line}");
        forget("fl-limit");
        forget("fl-lost");
        crate::activity::forget("fl-limit");
        crate::activity::forget("fl-lost");
    }

    /// A turn that stopped short hands over what it said before that, whatever
    /// came after it; and a line the CLI put in itself (the end of a background
    /// task it takes up) is no prompt whose reply is still to come.
    #[test]
    fn what_a_turn_said_comes_with_its_notice_whatever_came_after() {
        use crate::activity::{on_hook, HookEvent};
        let user = |t: &str| format!("{}\n", serde_json::json!({ "type": "user", "message": { "role": "user", "content": t } }));
        let said = |t: &str| {
            format!("{}\n", serde_json::json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": t }] } }))
        };
        let task = format!(
            "{}\n",
            serde_json::json!({ "type": "user", "message": { "role": "user", "content": "<task-notification>\n<task-id>b1</task-id>\n</task-notification>" },
                "origin": { "kind": "task-notification" }, "promptSource": "system", "turnOrigin": "task_notification" })
        );
        let worked = format!("{}{}{}", user("fix it"), said("Read the code."), said("Half the fix is in."));
        let pty = crate::pty::manager();
        let line_of = |id: &str, transcript: String, cut: bool| -> String {
            let path = std::env::temp_dir().join(format!("luna-{id}-{}.jsonl", std::process::id()));
            std::fs::write(&path, transcript).unwrap();
            transcripts().lock().unwrap().insert(id.into(), path.clone());
            let known = reg().lock().unwrap().known.get(id).cloned().unwrap();
            let mut o = Owed::from(0, 0);
            o.cut_short = cut;
            o.held = Some((1, now_ms() - REPLY_GRACE_MS));
            let Some((Some(line), Settled::Answered)) = notice_line(pty, "ws-caller", "Your helper", id, &known, o) else {
                panic!("its turn is over")
            };
            let _ = std::fs::remove_file(&path);
            line
        };
        let helper = |id: &str| {
            register(id, Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, false, serde_json::json!({}));
            crate::background::idle_for_test(id, Provider::Claude, 1, false);
        };

        // Cut short on the way to what was queued behind its turn.
        helper("ws-cut");
        let line = line_of("ws-cut", format!("{worked}{}", user("then the tests")), true);
        assert!(line.contains("did NOT finish") && line.contains("It ended with 2 messages since you asked"), "{line}");
        // The same transcript with nothing cut: a prompt last is a reply to come.
        helper("ws-open");
        assert!(line_of("ws-open", format!("{worked}{}", user("then the tests")), false).contains("without a reply"));
        // Claude Code taking up the end of its background task wrote that line itself.
        helper("ws-task");
        let line = line_of("ws-task", format!("{worked}{task}"), true);
        assert!(line.contains("It ended with 2 messages since you asked"), "{line}");
        let line = line_of("ws-task", format!("{worked}{task}"), false);
        assert!(line.contains("finished its turn with 2 messages since you asked"), "{line}");
        // Stopped on an error: what it said before that, and the error's own stop.
        register("ws-fail", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, false, serde_json::json!({}));
        on_hook("ws-fail", HookEvent::Busy);
        on_hook("ws-fail", HookEvent::Failed("rate_limit"));
        let line = line_of("ws-fail", format!("{worked}{}", user("then the tests")), false);
        assert!(line.starts_with("Your helper stopped on an error: rate_limit. It ended with 2 messages since you asked"), "{line}");
        for id in ["ws-cut", "ws-open", "ws-task", "ws-fail"] {
            crate::background::gone_for_test(id);
            forget(id);
        }
        crate::activity::forget("ws-fail");
        assert_eq!(sentence("pick a different model."), "pick a different model.");
        assert_eq!(sentence("rate_limit"), "rate_limit.");
        assert_eq!(sentence("cut short…"), "cut short…");
    }

    #[test]
    fn a_transcript_on_disk_with_no_reply_is_said_as_that() {
        use crate::activity::{on_hook, HookEvent};
        let path = std::env::temp_dir().join(format!("luna-no-reply-{}.jsonl", std::process::id()));
        std::fs::write(&path, "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"go\"}}\n").unwrap();
        register("nr-chat", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, false, serde_json::json!({}));
        transcripts().lock().unwrap().insert("nr-chat".into(), path.clone());
        on_hook("nr-chat", HookEvent::Busy);
        on_hook("nr-chat", HookEvent::Idle);
        let line = line_for("nr-chat");
        assert!(line.contains("finished its turn without a reply"), "{line}");
        forget("nr-chat");
        crate::activity::forget("nr-chat");
        let _ = std::fs::remove_file(&path);
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

    #[test]
    fn text_queued_as_the_core_went_comes_back_with_the_notice() {
        let none = serde_json::json!({});
        register("ql-child", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("ql-parent"), false, none);
        crate::background::take_up_for_test("ql-child", "C:\\p", 3, &["first line\nsecond line", "and another"], None);
        owe_from("ql-parent", "ql-child", 3, 0);
        let (line, how) = notice_for("ql-parent", "ql-child", 5);
        let line = line.unwrap();
        assert!(matches!(how, Settled::Answered));
        assert!(line.contains("did NOT finish") && line.contains("Not delivered: 2 messages queued for it"), "{line}");
        let file = line.split(", in ").nth(1).and_then(|l| l.split(';').next()).unwrap();
        assert!(line.replace(file, "").len() < 360, "a notice is typed into a CLI: {line}");
        let held = std::fs::read_to_string(file).unwrap();
        let _ = std::fs::remove_file(file);
        assert!(held.starts_with("NOT DELIVERED: 2 messages"), "{held}");
        let (first, second) = (held.find("first line\nsecond line").unwrap(), held.find("and another").unwrap());
        assert!(first < second, "word for word, oldest first: {held}");

        crate::background::remove("ql-child");
        forget("ql-child");
    }

    #[test]
    fn a_helper_that_never_got_started_says_so_and_hands_back_its_prompt() {
        register("ns-child", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("ns-parent"), false, serde_json::json!({}));
        crate::background::take_up_for_test("ns-child", "C:\\p", 0, &["and a follow-up"], Some("do the whole job"));
        owe_from("ns-parent", "ns-child", 0, 0);
        let (line, how) = notice_for("ns-parent", "ns-child", 2);
        let line = line.unwrap();
        assert!(matches!(how, Settled::Answered));
        assert!(
            line.contains("never got started") && line.contains("its prompt was lost") && line.contains("luna_send starts it afresh"),
            "{line}"
        );
        assert!(line.contains("its prompt and 1 queued message are in "), "{line}");
        let file = line.split(" are in ").nth(1).and_then(|l| l.split(". luna_send").next()).unwrap();
        let held = std::fs::read_to_string(file).unwrap();
        let _ = std::fs::remove_file(file);
        assert!(held.find("do the whole job").unwrap() < held.find("and a follow-up").unwrap(), "{held}");
        // Still there to delete, which is what writes its end down.
        assert!(reg().lock().unwrap().known.contains_key("ns-child"));
        crate::background::remove("ns-child");
        forget("ns-child");
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

    /// A throwaway repository with worktrees made as Claude Code makes them.
    fn repo_with(tag: &str, worktrees: &[&str]) -> Option<(std::path::PathBuf, String, Vec<String>)> {
        let root = std::env::temp_dir().join(format!("luna-rw-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).ok()?;
        // As a user's temp folder may be: git's own say of it is in long names.
        let root = crate::worktree::short_spelling(&root);
        let repo = root.to_string_lossy().into_owned();
        let git = |dir: &str, args: &[&str]| {
            let mut c = std::process::Command::new("git");
            c.arg("-C").arg(dir).args(["-c", "user.email=t@t", "-c", "user.name=t"]).args(args);
            c.output().is_ok_and(|o| o.status.success())
        };
        if !git(&repo, &["init", "-q"]) || !git(&repo, &["commit", "-q", "--allow-empty", "-m", "i"]) {
            return None;
        }
        let mut made = Vec::new();
        for name in worktrees {
            let wt = crate::worktree::named_path(&repo, name);
            if !git(&repo, &["worktree", "add", "-q", "-b", &format!("worktree-{name}"), &wt]) {
                return None;
            }
            made.push(wt);
        }
        Some((root, repo, made))
    }

    /// A visible helper its name was given, as `spawn_is` gives it.
    fn named_helper(id: &str, repo: &str, name: &str) {
        register(id, Provider::Claude, repo, "C:\\no\\such\\account", Some("rw-parent"), false, serde_json::json!({}));
        reg().lock().unwrap().worktree_names.insert(id.to_string(), name.to_string());
    }

    #[test]
    fn a_fresh_start_goes_back_only_into_the_worktree_luna_named_and_only_as_made() {
        let Some((root, repo, made)) = repo_with("reuse", &["luna-a", "luna-b", "someone-elses"]) else { return };
        let (a, b, elses) = (&made[0], &made[1], &made[2]);
        let start = |id: &str| worktree_for_start(id, &repo, "C:\\no\\such\\account", false);

        // Its own, as made. The clean one beside it, new as it is, is nobody's to take.
        named_helper("rw-a", &repo, "luna-a");
        assert_eq!(start("rw-a").as_deref(), Some("luna-a"));
        assert_eq!(worktrees_left_line("rw-a"), "");
        // Not there: Claude Code makes it by that name.
        named_helper("rw-new", &repo, "luna-new");
        assert_eq!(start("rw-new").as_deref(), Some("luna-new"));
        // Named none, a person's chat: its CLI names its own, and nothing is taken.
        register("rw-none", Provider::Claude, &repo, "C:\\no\\such\\account", None, false, serde_json::json!({}));
        assert_eq!(start("rw-none"), None);

        // Something in it: left alone, told of, and the next name from here on.
        std::fs::write(std::path::Path::new(b).join("half-done.txt"), "work").unwrap();
        named_helper("rw-b", &repo, "luna-b");
        assert_eq!(start("rw-b").as_deref(), Some("luna-b-2"));
        let said = worktrees_left_line("rw-b");
        assert!(
            said.contains("worktreeLeft") && said.contains(b.as_str()) && !said.contains(elses.as_str()) && !said.contains(a.as_str()),
            "{said}"
        );
        assert_eq!(reg().lock().unwrap().worktree_names["rw-b"], "luna-b-2");
        assert_eq!(start("rw-b").as_deref(), Some("luna-b-2"));
        assert_eq!(worktrees_left_line("rw-b"), said, "the one it left is not told of twice");
        // In a conversation it is its own, whatever is in it.
        named_helper("rw-talk", &repo, "luna-b");
        assert_eq!(worktree_for_start("rw-talk", &repo, "C:\\no\\such\\account", true).as_deref(), Some("luna-b"));
        // A name that would be a flag never goes out as one.
        named_helper("rw-flag", &repo, "--dangerously-skip-permissions");
        assert_eq!(start("rw-flag"), None);

        // Told once: with what is told first, or with a read; a cancel brings it back.
        let note = undelivered_note("rw-parent", "rw-b").unwrap();
        assert!(note.starts_with("worktreeLeft") && note.contains(b.as_str()), "{note}");
        assert!(undelivered_note("rw-nobody", "rw-b").is_none());
        let undo = hand_over("rw-parent", "rw-b", 1, None);
        assert_eq!(worktrees_left_line("rw-b"), "");
        undo();
        assert!(worktrees_left_line("rw-b").contains(b.as_str()));

        // Written down, it comes back with the helper.
        let ties = {
            let r = reg().lock().unwrap();
            ties_in(&r, "rw-b").unwrap()
        };
        assert_eq!(ties.worktrees_left, std::slice::from_ref(b));
        assert_eq!(ties.worktree_name.as_deref(), Some("luna-b-2"));
        let mut chat = cut_short_chat("rw-back", "rw-parent", false, true, false);
        chat.ties.worktree_name = Some("luna-back".into());
        chat.ties.worktrees_left = vec!["b".into()];
        take_up(vec![chat], Vec::new(), Vec::new());
        {
            let r = reg().lock().unwrap();
            assert_eq!(r.worktree_names["rw-back"], "luna-back");
            assert_eq!(r.worktrees_left["rw-back"], ["b"]);
        }
        for id in ["rw-a", "rw-new", "rw-none", "rw-b", "rw-talk", "rw-flag", "rw-back"] {
            forget(id);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_helper_not_back_has_what_luna_named_told_of_and_then_makes_another() {
        let Some((root, repo, made)) = repo_with("notback", &["luna-gone"]) else { return };
        named_helper("nb-chat", &repo, "luna-gone");
        let k = reg().lock().unwrap().known["nb-chat"].clone();
        leave_half_made("nb-chat", &k);
        assert!(worktrees_left_line("nb-chat").contains(made[0].as_str()));
        assert_eq!(reg().lock().unwrap().worktree_names["nb-chat"], "luna-gone-2");
        forget("nb-chat");
        let _ = std::fs::remove_dir_all(&root);
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

    #[test]
    fn what_a_spawn_came_to_decides_what_its_caller_is_told() {
        let _one = spawns_to_myself();
        // One that had come to a session the last core saved: nothing to say of the
        // spawn, and that session owes its caller as if the call had got there.
        register("ws-caller", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, serde_json::json!({}));
        let mut made = cut_short_chat("ws-made", "ws-caller", true, false, false);
        made.turns_ended = 0;
        made.ties.owed.clear();
        made.ties.opening = Some("the opening".into());
        // Asked for again by a request above any the file holds.
        let lost = spawn_taken(9_000_001, "ws-caller", None, false);
        take_up(vec![made], Vec::new(), vec![spawn_taken(9_000_000, "ws-caller", Some("ws-made"), true), lost]);
        assert!(reg().lock().unwrap().next_request > 9_000_001);
        take_up_spawns();
        {
            let r = reg().lock().unwrap();
            let o = r.owed[&("ws-caller".to_string(), "ws-made".to_string())];
            assert!(o.cut_short && o.baseline == 0 && o.cursor == 0);
            assert_eq!(r.resumes["ws-made"].opening.as_deref(), Some("the opening"));
        }
        assert_eq!(crate::activity::turns_counted("ws-made"), Some(1), "its turn went with the core");
        // Nothing before the window has had its chance.
        settle_spawns(SESSION_RETURN_MS - 1);
        assert!(reg().lock().unwrap().spawns[&9_000_001].news.is_none());
        settle_spawns(SESSION_RETURN_MS);
        {
            let r = reg().lock().unwrap();
            assert!(!r.spawns.contains_key(&9_000_000), "a helper came of it");
            let news = r.spawns[&9_000_001].news.clone().unwrap();
            assert!(news.contains("\"the job\"") && news.contains("never completed") && news.contains("core restarted"), "{news}");
            assert!(news.contains("ended in an error"), "it says what became of the call: {news}");
            assert!(!news.contains("worktreeLeft"), "{news}");
            let file = news.split("Its prompt is in ").nth(1).and_then(|l| l.split("; spawn it again").next()).unwrap();
            assert_eq!(std::fs::read_to_string(file).unwrap(), "the whole job, word for word");
            let _ = std::fs::remove_file(file);
        }
        // Told only once it is back, and what was told goes.
        let news = spawn_news(1000);
        assert_eq!(news["ws-caller"].len(), 1);
        reg().lock().unwrap().known.get_mut("ws-caller").unwrap().waiting = true;
        assert!(!spawn_news(1000).contains_key("ws-caller"), "not started yet");
        assert!(reg().lock().unwrap().spawns.contains_key(&9_000_001));
        assert!(!spawn_news(CALLER_RETURN_MS).contains_key("ws-caller"));
        assert!(!reg().lock().unwrap().spawns.contains_key(&9_000_001), "not back in time: to the log");
        forget("ws-made");
        forget("ws-caller");
    }

    #[test]
    fn a_row_the_window_dropped_makes_it_a_spawn_nothing_came_of() {
        let _one = spawns_to_myself();
        register("wd-caller", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, serde_json::json!({}));
        let mut made = cut_short_chat("wd-gone", "wd-caller", false, false, false);
        made.ties.owed.clear();
        take_up(vec![made], Vec::new(), vec![spawn_taken(9_000_100, "wd-caller", Some("wd-gone"), false)]);
        take_up_spawns();
        // `chat_names` finds no row for it: the caller's debt goes with it.
        forget("wd-gone");
        assert!(!reg().lock().unwrap().owed.contains_key(&("wd-caller".to_string(), "wd-gone".to_string())));
        settle_spawns(SESSION_RETURN_MS);
        let news = reg().lock().unwrap().spawns[&9_000_100].news.clone().unwrap();
        assert!(news.contains("never completed"), "{news}");
        forget("wd-caller");
        assert!(!reg().lock().unwrap().spawns.contains_key(&9_000_100));
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
    fn a_helper_told_it_was_not_done_is_heard_of_past_a_core_that_took_its_job() {
        crate::background::idle_for_test("tp-helper", Provider::Codex, 3, false);
        // As the last core saved it.
        let ties = Ties {
            parent: Some("tp-caller".into()),
            settings: serde_json::json!({ "background": true }),
            owed: vec![("tp-caller".into(), 3, 40)],
            told_pending: vec!["tp-caller".into()],
            ..Default::default()
        };
        let ties: Ties = serde_json::from_slice(&serde_json::to_vec(&ties).unwrap()).unwrap();
        tie_back("tp-helper", Provider::Codex, "C:\\p", "C:\\acc\\openai\\two", ties);
        let key = ("tp-caller".to_string(), "tp-helper".to_string());
        let o = reg().lock().unwrap().owed[&key];
        assert!(o.told_pending && o.job_went);
        // Not a turn in sight and nothing running: its job's end is all there will be.
        assert_eq!(verdict(&o, 3, Some(crate::activity::Turn::Idle), false, false), Verdict::JobOver);
        let (k, pty) = (reg().lock().unwrap().known["tp-helper"].clone(), crate::pty::manager());
        let held = Owed { held: Some((3, now_ms() - BACKGROUND_GRACE_MS)), ..o };
        let (line, how) = notice_line(pty, "tp-caller", "Your helper", "tp-helper", &k, held).unwrap();
        let line = line.unwrap();
        assert!(matches!(how, Settled::Answered));
        assert!(line.contains("has nothing left running now that Luna's core has restarted"), "{line}");
        // And it comes back marked again from what this core saves.
        assert_eq!(ties_of("tp-helper").unwrap().told_pending, vec!["tp-caller".to_string()]);
        crate::background::gone_for_test("tp-helper");
        forget("tp-helper");
    }

    #[test]
    fn a_spawn_with_the_flag_is_started_afresh_and_says_so_or_why_not() {
        let _one = spawns_to_myself();
        // No such caller to spawn for: as a spawn would fail, and the caller told.
        let s = SpawnTaken { left_by_core: true, ..spawn_taken(9_000_300, "af-nobody", None, true) };
        spawn_afresh(s);
        let r = reg().lock().unwrap();
        let news = r.spawns[&9_000_300].news.clone().unwrap();
        assert!(news.contains("never completed") && news.contains("started afresh, it failed too"), "{news}");
        drop(r);
        reg().lock().unwrap().spawns.remove(&9_000_300);
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

    #[test]
    fn a_budget_line_rides_like_any_line_with_no_news_in_it() {
        let now = 1_000_000;
        let fresh = now - 1000;
        assert!(!riders_go(false, false, fresh, now, 1), "owed another helper's answer, with work going on: it waits for that");
        assert!(riders_go(false, true, fresh, now, 1), "owed nothing else");
        assert!(riders_go(true, false, fresh, now, 1), "a notice is going in anyway");
        assert!(riders_go(false, false, now - RIDE_MAX_MS, now, 1), "ten minutes at most");
    }

    pub(super) fn tooled(id: &str) {
        register(id, Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, serde_json::json!({}));
    }

    /// A look taken before a retry reads the turn that missed as the answer.
    #[test]
    fn a_notice_looked_up_before_a_retry_neither_goes_out_nor_settles() {
        let (a, id) = ("rn-sender", "rn-helper");
        let none = serde_json::json!({});
        register(a, Provider::Claude, "C:\\p", "C:\\acc\\one", None, true, none.clone());
        register(id, Provider::Claude, "C:\\p", "C:\\acc\\one", Some(a), false, none);
        crate::background::idle_for_test(id, Provider::Claude, 1, false);
        owe_from(a, id, 0, 0);
        let key = (a.to_string(), id.to_string());
        let (looked_up, k) = {
            let r = reg().lock().unwrap();
            (r.owed.get(&key).copied().unwrap(), r.known.get(id).cloned().unwrap())
        };
        retried(id, &[a.to_string()], 1);
        let told = [(id.to_string(), Settled::Answered, looked_up)];
        assert!(debts_moved(a, &told), "caught before it is typed");
        assert!(notice_line(crate::pty::manager(), a, "Your helper", id, &k, looked_up).is_none(), "and before it is put together");
        settle_told(&mut reg().lock().unwrap(), key.clone(), Settled::Answered, &looked_up);
        assert_eq!(reg().lock().unwrap().owed.get(&key).map(|o| o.baseline), Some(1), "still owed, past the turn that missed");
        crate::background::gone_for_test(id);
        forget(id);
        forget(a);
    }

    #[test]
    fn a_spawns_prompt_is_plain_text_before_anything_else() {
        let p = SpawnParams { prompt: "go\x1b[201~\ry".into(), ..Default::default() };
        assert!(spawn("nobody", p).err().unwrap().contains("control character"));
    }
}
