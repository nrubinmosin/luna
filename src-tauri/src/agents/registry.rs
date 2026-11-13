//! The registry every agent tool works on: one `Registry` behind one lock
//! (`reg`), and what it says of a session — who it is (`Known`), what the window
//! calls it, which token is its, whose it is and whom it may reach.

use crate::agents::handover::taken_over_error;
use crate::agents::held::Held;
use crate::agents::notifier::lines::{NoticeTurn, Woken};
use crate::agents::notifier::usage::QuotaTold;
use crate::agents::owed::{Owed, Receipt, Requests};
use crate::agents::persist::CHATS_FROZEN;
use crate::agents::spawn::{free_first, max_descendants, refused_a_place, under_cap, PlacesRefused, SpawnTaken};
use crate::agents::teardown::{forget, Auto};
use crate::provider::Provider;
use crate::throttle::now_ms;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// A session the agent layer knows: every one started with tools, and every
/// one an agent spawned.
#[derive(Clone)]
pub(super) struct Known {
    pub(super) provider: Provider,
    pub(super) folder: String,
    pub(super) account_path: String,
    pub(super) account: String,
    pub(super) parent: Option<String>,
    /// What the session was started with, as the CLI's flags spell it:
    /// model, effort and permission mode (Claude) or approval and sandbox
    /// (Codex). Echoed by `list` and `spawn`.
    pub(super) settings: serde_json::Value,
    /// Luna's tools were given to it: `tools/list` offers them.
    pub(super) tools: bool,
    /// Known from what the last core saved (`restore_chats`): its CLI has not
    /// started under this one yet.
    pub(super) waiting: bool,
    /// Spawned by a chat since deleted, with no parent left to hand it to (see
    /// `forget`): no agent reaches it, only the window.
    pub(super) orphan: bool,
}

/// The chat at the top of `id`'s lineage, whose sessions the cap counts.
pub(super) fn top_of(r: &Registry, id: &str) -> String {
    let mut cur = id.to_string();
    for _ in 0..16 {
        match r.known.get(&cur).and_then(|k| k.parent.clone()) {
            Some(p) => cur = p,
            None => break,
        }
    }
    cur
}

pub(super) struct Registry {
    /// Sessions by the SHA-256 of their bearer token (`token_key`).
    pub(super) tokens: HashMap<[u8; 32], String>,
    pub(super) known: HashMap<String, Known>,
    pub(super) next_request: u64,
    /// Spawns taken on and not yet seen through, by request id.
    pub(super) spawns: HashMap<u64, SpawnTaken>,
    /// What each helper was spawned to do, one line of it (`first_words`), by id.
    /// Kept in its ties: `agents.jsonl` is cut, and a helper can outlive its line.
    pub(super) tasks: HashMap<String, String>,
    /// Helpers the person took over as their own chats, by id, with the parent
    /// each was taken from (handover.rs): what that parent's calls are refused with.
    pub(super) taken_over: HashMap<String, String>,
    /// How many times each session's ownership has changed hands, counted once
    /// what was queued for it has been taken back out (handover.rs): a send that
    /// put text in before that owes nothing of the person's chat after it
    /// (owed.rs `owe_from_at`).
    pub(super) handovers: HashMap<String, u64>,
    /// A Codex helper to make a worktree of its own, by id, until its CLI has said
    /// where: the worktrees there were as it was spawned. Codex takes no name, so one
    /// new since is all that tells what a core may have cut it off making.
    pub(super) worktrees_before: HashMap<String, Vec<String>>,
    /// What a helper began and is not going on in, by id, until its parent has heard
    /// (`worktrees_left_line`).
    pub(super) worktrees_left: HashMap<String, Vec<String>>,
    /// Past which turn count a helper's answer to a caller's text lies, the text
    /// having missed the turn it joined (`retried`): above any count the caller holds.
    pub(super) delivery_floors: HashMap<(String, String), u32>,
    /// Helpers that run in a worktree they were handed rather than made:
    /// deleting one never drops that worktree, whose work is its caller's.
    pub(super) borrowed: std::collections::HashSet<String>,
    /// The `turnsEnded` each caller was last shown for each session, by
    /// (caller, session). `wait(turn_done)` without `afterTurn` waits for a
    /// turn past that — the one the caller has not seen — rather than past
    /// the count at the moment of the call, which a fast reply had already
    /// moved on: `spawn`, a two-second answer, `wait` → timeout.
    pub(super) seen: HashMap<(String, String), u32>,
    /// The chat rows' names by chat id, as the frontend last sent them. Every
    /// chat's, not only the known ones: a name may arrive before its session
    /// registers.
    pub(super) names: HashMap<String, String>,
    /// The window group each chat row sits in, 0-based, sent with the names:
    /// sessions the user put side by side in one group are the ones meant to
    /// work things out between them.
    pub(super) groups: HashMap<String, u32>,
    /// When each caller last typed into a session that was stopped on a
    /// prompt, by (caller, session): the answer to the prompt. For a few
    /// seconds after it the prompt may still look open, and `wait` must not
    /// hand it back as if it were a new one.
    pub(super) answered: HashMap<(String, String), u64>,
    /// Answers owed, by (caller, session): the caller spawned the session or
    /// typed into it, and has not been shown a turn past `baseline` since.
    /// The notifier tells the caller when the answer comes.
    pub(super) owed: HashMap<(String, String), Owed>,
    /// Each caller's requests to each session, by (caller, session) (owed.rs
    /// `Requests`): which one an answer is to.
    pub(super) requests: HashMap<(String, String), Requests>,
    /// `wait` calls in flight, by (caller, session).
    pub(super) waits: HashMap<(String, String), u32>,
    /// Sessions a `wait_many` in flight will answer for, by (caller, session):
    /// their notices wait for it, however long the others take.
    pub(super) reserved: HashMap<(String, String), u32>,
    /// Answers handed over so far, by (caller, session), the latest being the
    /// only one a cancel may take back (see `hand_over`).
    pub(super) receipts: HashMap<(String, String), Receipt>,
    /// Helpers spawned with `autoDelete`, by id.
    pub(super) auto: HashMap<String, Auto>,
    /// Each account's usage as the window last showed it, by `provider/name`.
    pub(super) usage: HashMap<String, serde_json::Value>,
    /// When each caller last raised a desktop notification (`notify`).
    pub(super) notified: HashMap<String, u64>,
    /// The tools a Claude Code session's hooks say are up for an answer
    /// (`AskUserQuestion`, or one held for permission), oldest first, each
    /// with its input and the subagent that asks, if one does: subagents at
    /// work side by side may each hold one.
    pub(super) hook_asks: HashMap<String, Vec<(String, serde_json::Value, Option<String>)>>,
    /// Text sent to the person's chats where they were typing or had a draft
    /// Luna could not put aside, in the order it came: typed in once the way is
    /// clear (see `deliver_held`).
    pub(super) held: Vec<Held>,
    /// Notices a caller's draft holds back, by caller, for the chat row.
    pub(super) notices_held: HashMap<String, usize>,
    /// The turns Luna's notices started, by the session they went into.
    pub(super) notice_turns: HashMap<String, Vec<NoticeTurn>>,
    /// Leads Luna has told their delegates finished (lines.rs `wake_lead`), by id.
    pub(super) woken: HashMap<String, Woken>,
    /// What each lead was last seen waiting on (lines.rs `note_delegates`):
    /// what its wake names.
    pub(super) delegates: HashMap<String, Vec<String>>,
    /// The share of its context window each helper past its level was last
    /// read at (notifier usage.rs `budget_news`): the rider on its notices.
    pub(super) context_seen: HashMap<String, u64>,
    /// When each turn-end notice went, by (caller, session), within
    /// BREAKER_WINDOW_MS.
    pub(super) notice_times: HashMap<(String, String), Vec<u64>>,
    /// The pairs Luna has stopped telling (BREAKER_NOTICES), and since when.
    pub(super) stopped: HashMap<(String, String), u64>,
    /// How each pty session last exited, by id, until it runs again.
    pub(super) exits: HashMap<String, Option<u32>>,
    /// How far each caller has been told a usage window has gone, by (caller,
    /// `provider/name`, the window's label).
    pub(super) quota_told: HashMap<(String, String, String), QuotaTold>,
    /// Lines for a caller's next notice that no debt brings: by caller.
    pub(super) news: Vec<(String, String)>,
    /// Sessions refused a place on a line since its top chat was last told, by
    /// that chat (spawn/ `refused_a_place`): a line that rides with its notices.
    pub(super) places_refused: HashMap<String, PlacesRefused>,
    /// Leads under a session that went on in a fresh conversation since it
    /// was last told, by that session (handoff/lead.rs `handed_off`): a line
    /// that rides with its notices.
    pub(super) handed_off: HashMap<String, crate::agents::handoff::HandedOff>,
    /// What Luna applied to each session's latest start, a helper's latest
    /// turn (applied.rs), with the account and folder it ran on: what the
    /// window's "What Luna applied" shows.
    pub(super) applied: HashMap<String, Launched>,
}

/// A session's latest start, as `applied` keeps it.
pub(super) struct Launched {
    applied: Vec<crate::applied::Applied>,
    account_path: String,
    folder: String,
}

pub(super) fn reg() -> &'static Mutex<Registry> {
    static R: OnceLock<Mutex<Registry>> = OnceLock::new();
    let r = R.get_or_init(|| {
        Mutex::new(Registry {
            tokens: HashMap::new(),
            known: HashMap::new(),
            borrowed: Default::default(),
            // Above any request a core gone since may have left on disk.
            next_request: now_ms(),
            spawns: HashMap::new(),
            tasks: HashMap::new(),
            taken_over: HashMap::new(),
            handovers: HashMap::new(),
            worktrees_before: HashMap::new(),
            worktrees_left: HashMap::new(),
            delivery_floors: HashMap::new(),
            seen: HashMap::new(),
            names: HashMap::new(),
            groups: HashMap::new(),
            answered: HashMap::new(),
            owed: HashMap::new(),
            requests: HashMap::new(),
            waits: HashMap::new(),
            reserved: HashMap::new(),
            receipts: HashMap::new(),
            auto: HashMap::new(),
            usage: HashMap::new(),
            notified: HashMap::new(),
            hook_asks: HashMap::new(),
            held: Vec::new(),
            notices_held: HashMap::new(),
            notice_turns: HashMap::new(),
            woken: HashMap::new(),
            delegates: HashMap::new(),
            context_seen: HashMap::new(),
            notice_times: HashMap::new(),
            stopped: HashMap::new(),
            exits: HashMap::new(),
            quota_told: HashMap::new(),
            news: Vec::new(),
            places_refused: HashMap::new(),
            handed_off: HashMap::new(),
            applied: HashMap::new(),
        })
    });
    // Tests assert with this held and all of them share it: one that fails is
    // that test's failure, not that of every test after it.
    #[cfg(test)]
    r.clear_poison();
    r
}

pub(super) fn account_name(account_path: &str) -> String {
    std::path::Path::new(account_path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Whose a session is: the core's record (`pass_up`, kept across cores), and only
/// with none the one given (a helper being made), never for an orphan.
fn parent_by_record(record: Option<&Known>, given: Option<&str>) -> Option<String> {
    let orphan = record.is_some_and(|k| k.orphan);
    record.and_then(|k| k.parent.clone()).or_else(|| given.filter(|_| !orphan).map(str::to_owned))
}

/// A session is starting. Returns the bearer token for its MCP config when it
/// gets tools. Called at every start of its CLI, so a session going on is
/// registered again with the same lineage and a fresh token. `parent` is the
/// spawning session's for a helper being made; a chat the window starts is the
/// person's own and has none.
pub fn register(
    id: &str,
    provider: Provider,
    folder: &str,
    account_path: &str,
    parent: Option<&str>,
    tools: bool,
    settings: serde_json::Value,
) -> Option<String> {
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let account = account_name(account_path);
    let previous = r.known.get(id).cloned();
    r.exits.remove(id);
    // Whose it is since it last ran, if anything has changed that (`forget`).
    let orphan = previous.as_ref().is_some_and(|k| k.orphan);
    let parent = parent_by_record(previous.as_ref(), parent);
    // The window's settings are the flags it starts a CLI with: this one is the
    // core's, kept across every start.
    let mut settings = settings;
    if let Some(k) = &previous {
        // And what a Codex helper taken over still carries (handover.rs).
        for key in CORE_SETTINGS.into_iter().chain([crate::agents::handover::STARTED_WITH]) {
            if let Some(v) = k.settings.get(key) {
                settings[key] = v.clone();
            }
        }
    }
    // A session running again: what the chats' ties say is news once more.
    CHATS_FROZEN.store(false, std::sync::atomic::Ordering::SeqCst);
    r.known.insert(
        id.to_string(),
        Known {
            provider,
            folder: folder.to_string(),
            account_path: account_path.to_string(),
            account,
            parent,
            settings,
            tools,
            waiting: false,
            orphan,
        },
    );
    // One token per session: a respawn invalidates the last one.
    r.tokens.retain(|_, chat| chat != id);
    // Its CLI lists its tools afresh as it starts (agents/handoff).
    crate::agents::handoff::unlisted(id);
    if !tools {
        return None;
    }
    let token = mint();
    r.tokens.insert(token_key(&token), id.to_string());
    Some(token)
}

/// A caller that is not a session: for driving the agent layer from outside
/// — a test against a build, `curl` against the MCP endpoint. Registered only
/// when `LUNA_DEV_TOKEN` is set in the environment, with `LUNA_DEV_FOLDER`
/// as its project folder and `LUNA_DEV_ACCOUNT` as its account directory;
/// spawns it asks for come up as top-level chats, since it has no row of its
/// own. Never set for a Luna a person uses.
pub fn register_dev_caller() {
    let Ok(token) = std::env::var("LUNA_DEV_TOKEN") else { return };
    let folder = std::env::var("LUNA_DEV_FOLDER").unwrap_or_default();
    let account_path = std::env::var("LUNA_DEV_ACCOUNT").unwrap_or_default();
    if token.trim().is_empty() || folder.is_empty() || account_path.is_empty() {
        crate::log::warn("agents", "LUNA_DEV_TOKEN needs LUNA_DEV_FOLDER and LUNA_DEV_ACCOUNT too; dev caller off");
        return;
    }
    let provider = if account_path.replace('\\', "/").contains("/openai/") { Provider::Codex } else { Provider::Claude };
    register(DEV_CALLER, provider, &folder, &account_path, None, false, serde_json::json!({}));
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    r.tokens.insert(token_key(token.trim()), DEV_CALLER.to_string());
    let url = crate::hub::mcp_url().unwrap_or_default();
    crate::log::warn("agents", &format!("dev caller registered for {folder} ({account_path}); MCP at {url}"));
}

/// What Luna applied to `id`'s start, on `account_path` in `folder`: kept
/// until its next start, or its turn's for a helper, replaces it.
pub fn set_applied(id: &str, applied: Vec<crate::applied::Applied>, account_path: &str, folder: &str) {
    let launched = Launched { applied, account_path: account_path.to_string(), folder: folder.to_string() };
    reg().lock().unwrap_or_else(|e| e.into_inner()).applied.insert(id.to_string(), launched);
}

/// What `set_applied` keeps of `id`: what was applied, the account, the folder.
pub fn applied_of(id: &str) -> Option<(Vec<crate::applied::Applied>, String, String)> {
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    r.applied.get(id).map(|l| (l.applied.clone(), l.account_path.clone(), l.folder.clone()))
}

/// A pty session's process has gone, with this exit code if it left one.
pub fn session_exited(id: &str, code: Option<u32>) {
    reg().lock().unwrap_or_else(|e| e.into_inner()).exits.insert(id.to_string(), code);
}

/// The id of the dev caller (see `register_dev_caller`).
pub const DEV_CALLER: &str = "dev";

/// The frontend's name for a chat — set when it spawns one, ahead of the next
/// `chat_names`.
pub fn set_name(id: &str, name: &str) {
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    r.names.insert(id.to_string(), name.to_string());
}

/// A chat's name as the user sees it, if the frontend has sent one.
pub fn chat_name(id: &str) -> Option<String> {
    reg().lock().unwrap_or_else(|e| e.into_inner()).names.get(id).cloned().filter(|n| !n.trim().is_empty())
}

/// The first line of what a helper was spawned on (spawn/ `TASK_CHARS`), for
/// the window's list of a chat's helpers.
pub fn task_of(id: &str) -> Option<String> {
    reg().lock().unwrap_or_else(|e| e.into_inner()).tasks.get(id).cloned()
}

#[cfg(test)]
pub fn set_task_for_test(id: &str, task: &str) {
    reg().lock().unwrap_or_else(|e| e.into_inner()).tasks.insert(id.to_string(), task.to_string());
}

/// A session as the log names it: the id with the chat's name, which is what
/// is left to go by once the session is deleted.
pub fn label(id: &str) -> String {
    match chat_name(id) {
        Some(name) => format!("{id} \"{name}\""),
        None => id.to_string(),
    }
}

/// A bearer token: 160 bits from the OS's CSPRNG, as 40 hex characters.
pub(crate) fn mint() -> String {
    crate::secret::random_hex(20)
}

/// What the registry keys a token by. A map lookup compares the presented
/// token against a stored one byte by byte and stops at the first difference;
/// over digests, how far a guess got says nothing about the token itself.
fn token_key(token: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(token.as_bytes()).into()
}

/// The session behind a bearer token.
pub fn caller_of(token: &str) -> Option<String> {
    reg().lock().unwrap_or_else(|e| e.into_inner()).tokens.get(&token_key(token)).cloned()
}

/// A token for a session registered without tools: a background Claude Code
/// helper takes its permission prompts through Luna's MCP server all the
/// same, and `tools/list` offers it that one tool only.
pub(super) fn issue_token(id: &str) -> String {
    let token = mint();
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    r.tokens.retain(|_, chat| chat != id);
    r.tokens.insert(token_key(&token), id.to_string());
    token
}

/// Whether a session was given Luna's tools (the dev caller always has them).
pub fn has_tools(id: &str) -> bool {
    id == DEV_CALLER || reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(id).is_some_and(|k| k.tools)
}

pub fn parent_of(id: &str) -> Option<String> {
    reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(id)?.parent.clone()
}

pub fn settings_of(id: &str) -> serde_json::Value {
    reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(id).map(|k| k.settings.clone()).unwrap_or_default()
}

/// What a helper's settings hold that its CLI is not started with: `register` keeps
/// them across every start, and ties across cores. `resumeOnRestart`, a budget with
/// where it stands (`budget_news`), `connectors` (`Spec::connectors`) and
/// `constraints` (agents/spawn `Constraint`), which a spawn of its own passes on.
const CORE_SETTINGS: [&str; 7] =
    ["resumeOnRestart", "budgetTokens", "budgetPct", "budgetBreach", "budgetTold", "connectors", "constraints"];

/// Whether a helper was spawned with `resumeOnRestart`.
pub fn resumes_on_restart(id: &str) -> bool {
    reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(id).is_some_and(|k| k.settings["resumeOnRestart"] == true)
}

/// Whether `id` has helpers of its own still to answer it (`except` aside): it
/// is not through with what it was asked until it has their answers.
pub(super) fn waits_on_helpers(id: &str, except: &str) -> bool {
    waits_on_helpers_in(&reg().lock().unwrap_or_else(|e| e.into_inner()), id, except)
}

/// Whether a chat is owed an answer by its helpers, or its helpers' helpers:
/// work of its own going on beside its turn (`activity::in_background`).
pub fn owes_answer(id: &str) -> bool {
    waits_on_helpers(id, "")
}

pub(super) fn waits_on_helpers_in(r: &Registry, id: &str, except: &str) -> bool {
    r.owed.keys().any(|(caller, helper)| caller == id && helper != except && is_descendant(r, id, helper))
}

pub(super) fn depth_of(r: &Registry, id: &str) -> usize {
    let mut d = 0;
    let mut cur = id.to_string();
    while let Some(p) = r.known.get(&cur).and_then(|k| k.parent.clone()) {
        d += 1;
        cur = p;
        if d > 16 {
            break;
        }
    }
    d
}

pub(super) fn is_descendant(r: &Registry, of: &str, id: &str) -> bool {
    let mut cur = id.to_string();
    for _ in 0..16 {
        match r.known.get(&cur).and_then(|k| k.parent.clone()) {
            Some(p) if p == of => return true,
            Some(p) => cur = p,
            None => return false,
        }
    }
    false
}

pub(super) fn descendants(r: &Registry, of: &str) -> Vec<String> {
    r.known.keys().filter(|id| is_descendant(r, of, id)).cloned().collect()
}

/// The target must be something the caller spawned, directly or through a
/// child. Anything else — the user's own chats, a sibling — is out of reach.
pub(super) fn authorize(caller: &str, target: &str) -> Result<Known, String> {
    if crate::background::promoting(target) {
        return Err(format!("the person is opening {target} as a chat of their own; it is theirs once it opens"));
    }
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    if r.taken_over.get(target).is_some_and(|from| from == caller) {
        return Err(taken_over_error(target));
    }
    if !is_descendant(&r, caller, target) {
        return Err(format!("session {target} is not one you spawned"));
    }
    r.known.get(target).cloned().ok_or_else(|| format!("unknown session {target}"))
}

/// Reading a session and waiting on it reach any session but the caller's
/// own for a person's chat: the transcript is on disk for anyone
/// `luna_session_list` names it to, and a caller that typed into a session has
/// to be able to collect the answer. A helper reaches only what is under it
/// (`persons_chat_in`).
pub(super) fn authorize_view(caller: &str, target: &str) -> Result<Known, String> {
    if caller == target {
        return Err("that is your own session".into());
    }
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let known = r.known.get(target).cloned().ok_or_else(|| format!("unknown session {target}; luna_session_list lists them"))?;
    beneath(&r, caller, target, "read or wait on")?;
    Ok(known)
}

/// Whether `id` is a person's chat, or the dev caller standing in for the
/// person: the only callers whose reach goes past the sessions under them
/// (`scope: "all"`), and that may notify the person or arm the PC's power. A
/// helper (one with a parent, or run in the background), an orphan and a
/// session Luna does not know reach only their own: a prompt injected into a
/// helper deep in a tree stays in that tree.
pub(super) fn persons_chat_in(r: &Registry, id: &str) -> bool {
    id == DEV_CALLER || r.known.get(id).is_some_and(|k| k.parent.is_none() && !k.orphan && k.settings["background"] != true)
}

/// `persons_chat_in`, for the tools.
pub fn is_persons_chat(id: &str) -> bool {
    persons_chat_in(&reg().lock().unwrap_or_else(|e| e.into_inner()), id)
}

/// What a helper hears when it reaches past the sessions under it.
fn not_under_you(id: &str, verb: &str) -> String {
    format!(
        "session {id} is not under you: a helper can {verb} only its own helpers and theirs. What is for anyone else goes \
         in your reply to the session that spawned you"
    )
}

/// A helper's reach: `target` is under it. A person's chat goes by its scope
/// (`in_scope`), checked before this.
fn beneath(r: &Registry, caller: &str, target: &str, verb: &str) -> Result<(), String> {
    if persons_chat_in(r, caller) || is_descendant(r, caller, target) {
        return Ok(());
    }
    // One the person took from it says so, as `in_scope` does.
    if r.taken_over.get(target).is_some_and(|from| from == caller) {
        return Err(taken_over_error(target));
    }
    Err(not_under_you(target, verb))
}

/// Whether `id` is the caller's: spawned by it, directly or through a helper.
pub(super) fn is_mine(caller: &str, id: &str) -> bool {
    is_descendant(&reg().lock().unwrap_or_else(|e| e.into_inner()), caller, id)
}

/// Whether a call's `scope` says `all`: past the caller's own (`mine`, the
/// default). Only a person's chat may say it (`persons_chat_in`).
pub fn scope_all(caller: &str, scope: Option<&str>) -> Result<bool, String> {
    scope_all_in(&reg().lock().unwrap_or_else(|e| e.into_inner()), caller, scope)
}

pub(super) fn scope_all_in(r: &Registry, caller: &str, scope: Option<&str>) -> Result<bool, String> {
    match scope {
        None | Some("mine") => Ok(false),
        Some("all") if persons_chat_in(r, caller) => Ok(true),
        Some("all") => Err(SCOPE_ALL_REFUSED.into()),
        Some(other) => Err(format!("unknown scope {other:?}; use mine (the default) or all")),
    }
}

/// A helper's `scope: "all"`, refused in every tool.
pub(super) const SCOPE_ALL_REFUSED: &str =
    "scope \"all\" is for a person's chats: a helper reaches only the sessions under it (its own helpers and theirs). Leave scope out";

/// Whether `id` is the caller or one of its own: what `scope` `mine` reaches.
pub fn own_or_self(caller: &str, id: &str) -> bool {
    caller == id || is_mine(caller, id)
}

/// Whether this core knows session `id`: whose it is then goes by its owner now.
pub fn is_known(id: &str) -> bool {
    reg().lock().unwrap_or_else(|e| e.into_inner()).known.contains_key(id)
}

/// What a read, a wait or a send may reach, by its `scope`: the caller's own
/// helpers and theirs, unless it says `all`. Text read off or typed into a
/// session no one asked it to touch is the reach a prompt injection wants, so
/// going past its own takes an argument that says so, and only a person's chat
/// may say it (`scope_all_in`). An unknown session, or the caller itself, is
/// left to the tool's own refusal, which says more.
pub fn in_scope(caller: &str, id: &str, scope: Option<&str>, verb: &str) -> Result<(), String> {
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let all = scope_all_in(&r, caller, scope)?;
    if all || caller == id || !r.known.contains_key(id) || is_descendant(&r, caller, id) {
        return Ok(());
    }
    if r.taken_over.get(id).is_some_and(|from| from == caller) {
        return Err(taken_over_error(id));
    }
    if !persons_chat_in(&r, caller) {
        return Err(not_under_you(id, verb));
    }
    Err(format!(
        "session {id} is not one you spawned; to {verb} a session that is not yours (the user's own chats, another's \
         helpers) pass scope: \"all\""
    ))
}

/// Text from a person's chat may go to any session Luna runs, the user's own
/// chats included — that is how one session passes a message to another — but
/// not back into the caller's own composer. A helper's goes only to the
/// sessions under it (`beneath`).
pub(super) fn authorize_send(caller: &str, target: &str) -> Result<Known, String> {
    if caller == target {
        return Err("that is your own session".into());
    }
    // Read before the registry is held: the settings are a file of their own.
    let most = max_descendants();
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    // `reap` has let the lock go to remove it: text taken now would start a
    // turn that goes with it, and a question nobody hears the end of.
    if r.auto.get(target).is_some_and(|a| a.deleting) {
        return Err(format!("{target} is being deleted (autoDelete, idle since its answer); spawn a helper again for more"));
    }
    let known = r.known.get(target).cloned().ok_or_else(|| format!("unknown session {target}; luna_session_list lists them"))?;
    beneath(&r, caller, target, "send to or answer")?;
    // A helper that only waits out autoDelete holds no place under its chat's
    // cap (`under_cap`); text sets it to work, and in a place again: one there
    // must be, taken here under the lock the cap is counted under.
    if r.auto.get(target).is_some_and(|a| a.released) {
        let (counted, coming) = under_cap(&r, target, None);
        if counted.len() + coming >= most {
            refused_a_place(&mut r, caller);
            return Err(format!(
                "{target} has given its answer and holds no place among the {most} its line may have at work; they are \
                 taken, so text that would set it to work again is refused: {}",
                free_first(&r, caller, &counted)
            ));
        }
    }
    // Text on its way starts the idle count over now, under the lock `reap`
    // decides under: it cannot be taken for deletion while this is delivered.
    if let Some(a) = r.auto.get_mut(target) {
        a.idle_since = None;
        a.released = false;
    }
    Ok(known)
}

/// Every chat's name and window group, sent whenever one changes: what
/// `luna_session_list` calls the user's own chats and where it puts them, which
/// the core otherwise knows only by id.
#[tauri::command]
pub fn chat_names(mut names: HashMap<String, String>, groups: Option<HashMap<String, u32>>) {
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    // A row deleted while no core ran told nobody.
    let gone = gone_from_the_window(&r, &names);
    // A background helper has no row to be named by: it keeps the name it
    // was spawned with (`set_name`) until the window runs it as a chat.
    for (id, name) in &r.names {
        if !names.contains_key(id) && r.known.get(id).is_some_and(|k| k.settings["background"] == true) {
            names.insert(id.clone(), name.clone());
        }
    }
    r.names = names;
    if let Some(groups) = groups {
        r.groups = groups;
    }
    drop(r);
    for id in gone {
        crate::log::info("agents", &format!("{id} has no row any more; letting go of what the last core kept of it"));
        forget(&id);
        crate::activity::forget(&id);
    }
}

/// The sessions kept from the last core (`Known::waiting`) that the window, now
/// it has said which chats it has, does not have.
pub(super) fn gone_from_the_window(r: &Registry, names: &HashMap<String, String>) -> Vec<String> {
    if names.is_empty() {
        return Vec::new();
    }
    r.known.iter().filter(|(id, k)| k.waiting && !names.contains_key(*id)).map(|(id, _)| id.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::teardown::forget;

    #[test]
    fn lineage_and_reach() {
        let none = serde_json::json!({});
        let a = register("a", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone()).expect("token");
        assert_eq!(caller_of(&a).as_deref(), Some("a"));
        register("b", Provider::Codex, "C:\\p", "C:\\acc\\openai\\two", Some("a"), false, none.clone());
        register("c", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("b"), false, none.clone());
        register("x", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, false, none);
        assert!(authorize("a", "b").is_ok());
        assert!(authorize("a", "c").is_ok(), "grandchildren are reachable");
        assert!(authorize("b", "a").is_err(), "not upwards");
        assert!(authorize("a", "x").is_err(), "not the user's own chats");
        assert!(authorize("a", "a").is_err(), "not itself");
        assert!(authorize_send("a", "x").is_ok(), "a person's chat's text may go to the user's own chats");
        assert!(authorize_send("a", "a").is_err(), "but not into its own composer");
        assert!(authorize_send("a", "nobody").is_err());
        assert!(authorize_view("a", "x").is_ok(), "a session written to can be read and waited on");
        assert!(authorize_view("a", "a").is_err(), "but not one's own");
        // A helper reaches only what is under it: not upwards, not sideways.
        assert!(authorize_send("b", "c").is_ok() && authorize_view("b", "c").is_ok(), "its own helper");
        for target in ["a", "x"] {
            assert!(authorize_send("b", target).err().unwrap().contains("not under you"), "{target}");
            assert!(authorize_view("b", target).err().unwrap().contains("not under you"), "{target}");
        }
        let r = reg().lock().unwrap();
        assert_eq!(depth_of(&r, "c"), 2);
        let mut d = descendants(&r, "a");
        d.sort();
        assert_eq!(d, vec!["b", "c"]);
    }

    #[test]
    fn a_dormant_helper_sent_to_takes_a_place_under_the_cap_or_is_refused() {
        let (account, none) = ("C:\\acc\\anthropic\\one", serde_json::json!({}));
        register("dc-top", Provider::Claude, "C:\\p", account, None, true, none.clone());
        let kids: Vec<String> = (0..10).map(|i| format!("dc-kid-{i}")).collect();
        for id in &kids {
            register(id, Provider::Claude, "C:\\p", account, Some("dc-top"), false, none.clone());
            // Its answer delivered, waiting out autoDelete: no place taken.
            reg().lock().unwrap().auto.insert(id.clone(), Auto { idle_since: Some(1), deleting: false, released: true });
        }
        // Each sent to takes a place again, as long as there is one.
        for id in &kids[..8] {
            assert!(authorize_send("dc-top", id).is_ok(), "{id}");
        }
        for id in &kids[8..] {
            let e = authorize_send("dc-top", id).err().unwrap();
            assert!(e.contains("delete one first") && e.contains("dc-kid-0"), "{e}");
            assert!(reg().lock().unwrap().auto[id].released, "refused, it stays as it was");
        }
        // A read starts its autoDelete clock over, and sets no work going: the
        // place is still to be taken when text does.
        crate::agents::owed::looked(&kids[9]);
        assert!(reg().lock().unwrap().auto[&kids[9]].idle_since.is_none());
        let e = authorize_send("dc-top", &kids[9]).err().unwrap();
        assert!(e.contains("delete one first"), "read first, then sent to: still counted: {e}");
        for id in kids.iter().map(String::as_str).chain(["dc-top"]) {
            reg().lock().unwrap().auto.remove(id);
            forget(id);
        }
    }

    #[test]
    fn reaching_past_ones_own_takes_scope_all() {
        let (account, none) = ("C:\\acc\\anthropic\\one", serde_json::json!({}));
        register("sc-me", Provider::Claude, "C:\\p", account, None, true, none.clone());
        register("sc-kid", Provider::Claude, "C:\\p", account, Some("sc-me"), true, none.clone());
        register("sc-grand", Provider::Codex, "C:\\p", "C:\\acc\\openai\\two", Some("sc-kid"), false, none.clone());
        register("sc-chat", Provider::Claude, "C:\\p", account, None, false, none.clone());
        register("sc-theirs", Provider::Codex, "C:\\p", "C:\\acc\\openai\\two", Some("sc-chat"), false, none);
        // Its own, by default or said.
        assert_eq!(in_scope("sc-me", "sc-kid", None, "read"), Ok(()));
        assert_eq!(in_scope("sc-me", "sc-grand", Some("mine"), "read"), Ok(()));
        // From a person's chat, another chat and another chat's helper: only when told.
        for id in ["sc-chat", "sc-theirs"] {
            let e = in_scope("sc-me", id, None, "send to").unwrap_err();
            assert!(e.contains("not one you spawned") && e.contains("scope: \"all\""), "{id}: {e}");
            assert!(in_scope("sc-me", id, Some("mine"), "read").is_err());
            assert_eq!(in_scope("sc-me", id, Some("all"), "read"), Ok(()));
        }
        // A helper: its own, and nothing past it, told or not, in every verb.
        assert_eq!(in_scope("sc-kid", "sc-grand", None, "read"), Ok(()));
        for id in ["sc-me", "sc-chat", "sc-theirs"] {
            for verb in ["read", "wait on", "send to", "answer"] {
                let e = in_scope("sc-kid", id, None, verb).unwrap_err();
                assert!(e.contains("not under you") && !e.contains("scope: \"all\""), "{id}: {e}");
                assert_eq!(in_scope("sc-kid", id, Some("all"), verb).unwrap_err(), SCOPE_ALL_REFUSED);
            }
        }
        assert_eq!(scope_all("sc-kid", Some("all")).unwrap_err(), SCOPE_ALL_REFUSED, "nor even of its own");
        assert_eq!(scope_all("sc-me", Some("all")), Ok(true));
        assert_eq!(scope_all(DEV_CALLER, Some("all")), Ok(true), "the dev caller stands in for the person");
        // A session run in the background with no parent left is no person's chat.
        register("sc-alone", Provider::Claude, "C:\\p", account, None, true, serde_json::json!({ "background": true }));
        assert!(!is_persons_chat("sc-alone") && is_persons_chat("sc-me") && !is_persons_chat("sc-kid"));
        assert!(!is_persons_chat("sc-nobody"), "nor is one Luna does not know");
        forget("sc-alone");
        // An unknown session and itself are the tool's to refuse, in its words.
        assert_eq!(in_scope("sc-me", "sc-nobody", None, "read"), Ok(()));
        assert_eq!(in_scope("sc-me", "sc-me", None, "read"), Ok(()));
        assert!(in_scope("sc-me", "sc-kid", Some("everything"), "read").unwrap_err().contains("unknown scope"));
        assert!(own_or_self("sc-me", "sc-me") && own_or_self("sc-me", "sc-grand") && !own_or_self("sc-me", "sc-theirs"));
        for id in ["sc-grand", "sc-kid", "sc-theirs", "sc-chat", "sc-me"] {
            forget(id);
        }
    }

    #[test]
    fn a_helper_is_told_of_the_parent_it_passes_to() {
        let (account, none) = ("C:\\acc\\anthropic\\one", serde_json::json!({}));
        let background = serde_json::json!({ "background": true });
        register("tp-grand", Provider::Claude, "C:\\p", account, None, true, none.clone());
        register("tp-parent", Provider::Claude, "C:\\p", account, Some("tp-grand"), true, background.clone());
        register("tp-kid", Provider::Claude, "C:\\p", account, Some("tp-parent"), false, background);
        crate::background::take_up_for_test("tp-kid", "C:\\p", 1, &[], None);
        crate::background::set_parent("tp-kid", Some("tp-parent".into()));

        forget("tp-parent");
        assert_eq!(crate::background::told_parent_for_test("tp-kid").as_deref(), Some("tp-grand"), "a background helper's next turn");
        // Registered again, as a core taking it up does, it keeps the record's.
        register("tp-kid", Provider::Claude, "C:\\p", account, Some("tp-parent"), false, serde_json::json!({ "background": true }));
        assert_eq!(parent_of("tp-kid").as_deref(), Some("tp-grand"));

        forget("tp-grand");
        assert_eq!(crate::background::told_parent_for_test("tp-kid"), None, "nobody left to answer to");
        register("tp-kid", Provider::Claude, "C:\\p", account, Some("tp-parent"), false, serde_json::json!({ "background": true }));
        assert_eq!(parent_of("tp-kid"), None, "an orphan stays one");
        crate::background::remove("tp-kid");
        forget("tp-kid");
    }

    #[test]
    fn a_respawn_replaces_the_token() {
        let none = serde_json::json!({});
        let t1 = register("r", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone()).unwrap();
        let t2 = register("r", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none).unwrap();
        assert_ne!(t1, t2);
        assert_eq!(caller_of(&t1), None);
        assert_eq!(caller_of(&t2).as_deref(), Some("r"));
    }

    #[test]
    fn where_a_budget_stands_outlives_every_start_of_its_cli() {
        let settings = serde_json::json!({ "model": "m", "budgetTokens": 7, "budgetBreach": { "at": 1 }, "budgetTold": true });
        register("bs-kid", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("bs-p"), false, settings);
        // The window starts its CLI again, on flags that say nothing of it.
        register("bs-kid", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("bs-p"), false, serde_json::json!({ "model": "m2" }));
        let s = settings_of("bs-kid");
        assert_eq!(
            (&s["model"], &s["budgetTokens"], &s["budgetTold"]),
            (&serde_json::json!("m2"), &serde_json::json!(7), &serde_json::json!(true))
        );
        assert_eq!(s["budgetBreach"]["at"], 1);
        forget("bs-kid");
    }
}
