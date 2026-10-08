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
//! killing and deleting stay with the lineage. Children outlive their parent;
//! the UI marks them orphaned.
//!
//! A helper does not report back on its own the way a subagent does: its
//! answer sits in its own transcript until someone waits for it or reads it.
//! A caller that asked something (spawn, send) and ended its turn without
//! collecting the answer would sit idle beside a helper that finished long
//! ago, so the notifier here types a one-line notice into the caller once it
//! is idle: the helper answered, stopped on a prompt, or exited.

use crate::provider::Provider;
use crate::throttle::now_ms;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Condvar, Mutex, OnceLock};
use tauri::Manager;

/// How long the frontend gets to make the chat and start its session.
const SPAWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);
/// The most a caller may spawn under itself, all generations counted.
const MAX_DESCENDANTS: usize = 8;
/// A child of a child may exist; a child of that may not have tools.
const MAX_TOOL_DEPTH: usize = 1;
/// Enter, sent after the text so the TUI sees a paste and then a keypress.
/// Both CLIs treat bytes arriving in a quick burst as one paste, and an Enter
/// inside the burst as a newline in it: at 80 ms Codex 0.157 left the text
/// sitting in its composer. Well past the burst window, then.
const ENTER_GAP: std::time::Duration = std::time::Duration::from_millis(400);
/// If no turn has begun this long after Enter, Enter is pressed once more:
/// on a composer still holding the text it submits; on an empty one it is a
/// no-op in both CLIs.
const ENTER_RETRY: std::time::Duration = std::time::Duration::from_secs(3);
/// How long a fresh session gets to start working on its opening prompt
/// before `spawn` answers without that assurance. Codex brings up its MCP
/// servers first, which can take a while; a session still not busy after
/// this is on a notice, a picker or an error, and the answer carries its
/// screen so the caller can see which — or has not drawn at all yet, and the
/// answer says `starting` instead.
const START_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// The same for text typed into a running session: a turn should begin
/// within a few samples of Enter.
const SEND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(12);
/// The activity sampler runs every 5 s; the probes here look between samples
/// so the answer comes as soon as the verdict is in.
const POLL: std::time::Duration = std::time::Duration::from_millis(500);
/// The rendered screen as an agent gets it: the last lines of the terminal,
/// enough to read a dialog, not a transcript.
const SCREEN_LINES: usize = 40;
/// How often the notifier looks for answers nobody collected.
const NOTIFY_EVERY: std::time::Duration = std::time::Duration::from_secs(2);
/// Looks in a row the caller has to be found idle before a notice goes in:
/// a turn that has just ended may still be followed by the one it queued.
const NOTIFY_IDLE_LOOKS: u32 = 2;
/// A person typed into the caller this recently: hold the notice, it would
/// land in the middle of what they are writing.
const TYPING_QUIET_MS: u64 = 20_000;
/// A notice held this long by an unsent draft in the caller is announced on
/// the desktop instead, once: the draft may be forgotten, the notice is not.
const DRAFT_ALERT_MS: u64 = 60_000;
/// How much of a reply a notice quotes; the rest is a `luna_read` away.
const NOTICE_QUOTE: usize = 400;
/// A turn has ended with the session's processes still at work: give them
/// this long to settle before calling it a job left in the background. Right
/// after a turn they often move for a sample or two anyway.
const BACKGROUND_GRACE_MS: u64 = 20_000;
/// After a caller answers a prompt, how long the prompt it answered may still
/// read as open (the sampler runs every 5 s).
const ANSWERED_MS: u64 = 8_000;
/// Claude Code fires its Stop hook before the turn's last message reaches the
/// transcript: a turn seen over with no reply yet gets this long for one.
const REPLY_GRACE_MS: u64 = 5_000;

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct Message {
    pub role: String,
    pub text: String,
}

/// A session the agent layer knows: every one started with tools, and every
/// one an agent spawned.
#[derive(Clone)]
struct Known {
    provider: Provider,
    folder: String,
    account_path: String,
    account: String,
    parent: Option<String>,
    /// What the session was started with, as the CLI's flags spell it:
    /// model, effort and permission mode (Claude) or approval and sandbox
    /// (Codex). Echoed by `list` and `spawn`.
    settings: serde_json::Value,
}

/// What an agent asks for. Everything but the prompt may be left to the
/// defaults the frontend would apply to a chat made by hand.
#[derive(Clone, Serialize, Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct SpawnParams {
    pub provider: Option<String>,
    pub account: Option<String>,
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
}

/// The request as the frontend receives it.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SpawnRequest {
    request_id: u64,
    parent_id: String,
    #[serde(flatten)]
    params: SpawnParams,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Deleted {
    id: String,
}

#[derive(Default)]
struct Pending {
    done: Option<Result<String, String>>,
}

struct Registry {
    tokens: HashMap<String, String>,
    known: HashMap<String, Known>,
    pending: HashMap<u64, std::sync::Arc<(Mutex<Pending>, Condvar)>>,
    next_request: u64,
    /// The `turnsEnded` each caller was last shown for each session, by
    /// (caller, session). `wait(turn_done)` without `afterTurn` waits for a
    /// turn past that — the one the caller has not seen — rather than past
    /// the count at the moment of the call, which a fast reply had already
    /// moved on: `spawn`, a two-second answer, `wait` → timeout.
    seen: HashMap<(String, String), u32>,
    /// The chat rows' names by chat id, as the frontend last sent them. Every
    /// chat's, not only the known ones: a name may arrive before its session
    /// registers.
    names: HashMap<String, String>,
    /// When each caller last typed into a session that was stopped on a
    /// prompt, by (caller, session): the answer to the prompt. For a few
    /// seconds after it the prompt may still look open, and `wait` must not
    /// hand it back as if it were a new one.
    answered: HashMap<(String, String), u64>,
    /// Answers owed, by (caller, session): the caller spawned the session or
    /// typed into it, and has not been shown a turn past `baseline` since.
    /// The notifier tells the caller when the answer comes.
    owed: HashMap<(String, String), Owed>,
}

#[derive(Clone, Copy)]
struct Owed {
    /// Turns over when the caller asked; the answer ends a later one.
    baseline: u32,
    /// The caller has been told the session is stopped on a prompt.
    told_waiting: bool,
    /// The turn count at which the answer was seen with processes still at
    /// work, and when (see BACKGROUND_GRACE_MS).
    held: Option<(u32, u64)>,
}

fn reg() -> &'static Mutex<Registry> {
    static R: OnceLock<Mutex<Registry>> = OnceLock::new();
    R.get_or_init(|| {
        Mutex::new(Registry {
            tokens: HashMap::new(),
            known: HashMap::new(),
            pending: HashMap::new(),
            next_request: 1,
            seen: HashMap::new(),
            names: HashMap::new(),
            answered: HashMap::new(),
            owed: HashMap::new(),
        })
    })
}

/// Records what a tool answer told `caller` about `id`'s turn count. A turn
/// past what the caller asked for settles the answer it was owed.
fn shown(caller: &str, id: &str, turns_ended: u32) {
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let key = (caller.to_string(), id.to_string());
    if r.owed.get(&key).is_some_and(|o| turns_ended > o.baseline) {
        r.owed.remove(&key);
    }
    r.seen.insert(key, turns_ended);
}

/// `caller` has asked `id` something; the answer ends a turn past `baseline`.
/// The dev caller has no terminal to be told in.
fn owe(caller: &str, id: &str, baseline: u32) {
    if caller == DEV_CALLER {
        return;
    }
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    r.owed.insert((caller.to_string(), id.to_string()), Owed { baseline, told_waiting: false, held: None });
}

/// The last count `caller` was shown for `id`, if any.
fn last_shown(caller: &str, id: &str) -> Option<u32> {
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    r.seen.get(&(caller.to_string(), id.to_string())).copied()
}

static APP: OnceLock<tauri::AppHandle> = OnceLock::new();

pub fn init(app: tauri::AppHandle) {
    if APP.set(app).is_ok() {
        std::thread::Builder::new().name("luna-notifier".into()).spawn(notifier).expect("spawn notifier thread");
    }
}

fn app() -> Option<&'static tauri::AppHandle> {
    APP.get()
}

/// A session is starting. Returns the bearer token for its MCP config when it
/// gets tools. Called at every spawn, so a resumed session is re-registered
/// with the same lineage and a fresh token.
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
    let account = std::path::Path::new(account_path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let previous = r.known.get(id).cloned();
    r.known.insert(
        id.to_string(),
        Known {
            provider,
            folder: folder.to_string(),
            account_path: account_path.to_string(),
            account,
            parent: parent.map(str::to_owned).or_else(|| previous.as_ref().and_then(|k| k.parent.clone())),
            settings,
        },
    );
    // One token per session: a respawn invalidates the last one.
    r.tokens.retain(|_, chat| chat != id);
    if !tools {
        return None;
    }
    let token = mint(id);
    r.tokens.insert(token.clone(), id.to_string());
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
    r.tokens.insert(token.trim().to_string(), DEV_CALLER.to_string());
    let url = crate::hub::mcp_url().unwrap_or_default();
    crate::log::warn("agents", &format!("dev caller registered for {folder} ({account_path}); MCP at {url}"));
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

fn mint(id: &str) -> String {
    use sha2::{Digest, Sha256};
    use std::sync::atomic::{AtomicU64, Ordering};
    // Two spawns in the same millisecond must not share a token.
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    let mut h = Sha256::new();
    h.update(id.as_bytes());
    h.update(now_ms().to_le_bytes());
    h.update(std::process::id().to_le_bytes());
    h.update(SERIAL.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    let slot = 0u8;
    h.update((&slot as *const u8 as usize).to_le_bytes());
    h.finalize().iter().take(20).map(|b| format!("{b:02x}")).collect()
}

/// The session behind a bearer token.
pub fn caller_of(token: &str) -> Option<String> {
    reg().lock().unwrap_or_else(|e| e.into_inner()).tokens.get(token).cloned()
}

fn depth_of(r: &Registry, id: &str) -> usize {
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

fn is_descendant(r: &Registry, of: &str, id: &str) -> bool {
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

fn descendants(r: &Registry, of: &str) -> Vec<String> {
    r.known.keys().filter(|id| is_descendant(r, of, id)).cloned().collect()
}

/// The target must be something the caller spawned, directly or through a
/// child. Anything else — the user's own chats, a sibling — is out of reach.
fn authorize(caller: &str, target: &str) -> Result<Known, String> {
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    if !is_descendant(&r, caller, target) {
        return Err(format!("session {target} is not one you spawned"));
    }
    r.known.get(target).cloned().ok_or_else(|| format!("unknown session {target}"))
}

/// Reading a session and waiting on it reach any session but the caller's
/// own: the transcript is on disk for anyone `luna_sessions` names it to, and
/// a caller that typed into a session has to be able to collect the answer.
fn authorize_view(caller: &str, target: &str) -> Result<Known, String> {
    if caller == target {
        return Err("that is your own session".into());
    }
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    r.known.get(target).cloned().ok_or_else(|| format!("unknown session {target}; luna_sessions lists them"))
}

/// Text may go to any session Luna runs, the user's own chats included —
/// that is how one session passes a message to another — but not back into
/// the caller's own composer.
fn authorize_send(caller: &str, target: &str) -> Result<Known, String> {
    if caller == target {
        return Err("that is your own session".into());
    }
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    r.known.get(target).cloned().ok_or_else(|| format!("unknown session {target}; luna_sessions lists them"))
}

// ------------------------------------------------------------------ tools --

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRow {
    pub id: String,
    pub name: String,
    pub provider: Provider,
    pub account: String,
    pub parent: Option<String>,
    pub alive: bool,
    pub turn: Option<crate::activity::Turn>,
    pub busy: bool,
    pub turns_ended: u32,
    /// The flags it runs with (see `Known::settings`).
    pub settings: serde_json::Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listing {
    pub me: String,
    pub accounts: Vec<AccountRow>,
    pub sessions: Vec<SessionRow>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AccountRow {
    pub provider: Provider,
    pub name: String,
}

/// Accounts an agent may spawn on: every account folder on disk, minus the
/// ones the user switched off for agents.
pub fn allowed_accounts() -> Vec<AccountRow> {
    let blocked = crate::settings::get().agent_blocked_accounts;
    crate::accounts::list_accounts()
        .unwrap_or_default()
        .into_iter()
        .filter(|a| !blocked.iter().any(|b| b == &format!("{}/{}", a.provider.as_str(), a.name)))
        .map(|a| AccountRow { provider: a.provider, name: a.name })
        .collect()
}

pub fn list(caller: &str) -> Listing {
    let rows: Vec<(String, Known, String)> = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        descendants(&r, caller)
            .into_iter()
            .filter_map(|id| {
                let name = r.names.get(&id).cloned().unwrap_or_default();
                r.known.get(&id).cloned().map(|k| (id, k, name))
            })
            .collect()
    };
    let pty = app().map(|a| a.state::<crate::pty::PtyManager>());
    let sessions = rows
        .into_iter()
        .map(|(id, k, name)| {
            let ts = crate::activity::turn_state(&id);
            // A listing is not a reading: it does not move what the caller
            // is taken to have seen of a session's turns.
            let turns_ended = ts.map(|t| t.turns_ended).unwrap_or(0);
            SessionRow {
                alive: pty.as_ref().map(|p| p.alive(&id)).unwrap_or(false),
                turn: ts.map(|t| t.turn),
                busy: ts.map(|t| t.busy).unwrap_or(false),
                turns_ended,
                id,
                name,
                provider: k.provider,
                account: k.account,
                parent: k.parent,
                settings: k.settings,
            }
        })
        .collect();
    Listing { me: caller.to_string(), accounts: allowed_accounts(), sessions }
}

/// One row of `sessions`: any chat Luna has run a session for since it
/// started, whoever started it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnySession {
    pub id: String,
    pub name: String,
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
    pub turn: Option<crate::activity::Turn>,
    pub busy: bool,
    pub settings: serde_json::Value,
}

/// Every session Luna runs, for a caller that has to find another one — to
/// write to it, or to read its transcript off the disk.
pub fn sessions(caller: &str) -> Vec<AnySession> {
    let rows: Vec<(String, Known, String, bool)> = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        r.known
            .iter()
            .filter(|(id, _)| id.as_str() != DEV_CALLER || caller == DEV_CALLER)
            .map(|(id, k)| {
                let name = r.names.get(id).cloned().unwrap_or_default();
                (id.clone(), k.clone(), name, is_descendant(&r, caller, id))
            })
            .collect()
    };
    let pty = app().map(|a| a.state::<crate::pty::PtyManager>());
    let mut out: Vec<AnySession> = rows
        .into_iter()
        .map(|(id, k, name, yours)| {
            let probe = pty.as_ref().and_then(|p| p.probe(&id));
            let meta = probe.as_ref().and_then(disk_meta);
            let transcript = pty.as_ref().and_then(|p| transcript_path(p, &id)).map(|p| p.to_string_lossy().into_owned());
            // The row's name, else what the CLI calls the conversation.
            let name = Some(name)
                .filter(|n| !n.trim().is_empty())
                .or_else(|| meta.as_ref().and_then(|m| m.title.clone().or_else(|| m.first_prompt.clone())))
                .unwrap_or_default();
            let ts = crate::activity::turn_state(&id);
            AnySession {
                you: id == caller,
                yours,
                alive: probe.is_some(),
                turn: ts.map(|t| t.turn),
                busy: ts.map(|t| t.busy).unwrap_or(false),
                pid: probe.as_ref().and_then(|p| p.pid),
                cwd: meta.as_ref().and_then(|m| m.cwd.clone()).or_else(|| probe.as_ref().map(|p| p.cwd.clone())),
                session_id: meta.and_then(|m| m.session_id),
                transcript,
                id,
                name,
                provider: k.provider,
                account: k.account,
                account_path: k.account_path,
                folder: k.folder,
                parent: k.parent,
                settings: k.settings,
            }
        })
        .collect();
    out.sort_by(|a, b| b.alive.cmp(&a.alive).then_with(|| (&a.account, &a.name).cmp(&(&b.account, &b.name))));
    out
}

/// What the CLI has on disk about a live session: its cwd, ids and title.
fn disk_meta(p: &crate::pty::ActivityProbe) -> Option<crate::pty::SessionMeta> {
    match p.provider {
        Provider::Claude => crate::claude::session::meta(p.pid, &p.cwd, p.spawned_at_ms, &p.account_path),
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
    /// See `still_starting`: not stuck, just not up yet.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub starting: bool,
}

/// The last lines of a session's terminal, for a caller that has to see
/// what a helper is stuck on.
fn screen_of(pty: &crate::pty::PtyManager, id: &str) -> Option<String> {
    let full = pty.screen(id)?;
    let lines: Vec<&str> = full.lines().collect();
    let from = lines.len().saturating_sub(SCREEN_LINES);
    Some(lines[from..].join("\n"))
}

/// True once the session has begun a turn — or finished one — since
/// `after_turn` turns had ended.
fn has_started(id: &str, after_turn: u32) -> bool {
    use crate::activity::Turn;
    // A permission prompt is inside a turn: one that comes up before the
    // first sample has seen the session busy is still a start.
    crate::activity::turn_state(id)
        .map(|t| matches!(t.turn, Turn::Busy | Turn::Waiting) || t.turns_ended > after_turn)
        .unwrap_or(false)
}

/// True while the CLI has not drawn anything yet: alive, no turn begun or
/// over, and a blank terminal. Claude Code syncs the org's skills and plugins
/// and makes its worktree before it draws a thing — minutes on a bad day,
/// with the session looking dead all along. The opening prompt is a CLI
/// argument and waits for it intact; text typed now would be lost.
fn still_starting(pty: &crate::pty::PtyManager, id: &str) -> bool {
    pty.alive(id) && !has_started(id, 0) && pty.screen(id).is_some_and(|s| s.trim().is_empty())
}

/// Polls until the session starts working (see `has_started`), it exits, or
/// `timeout` passes. Returns whether it started.
fn await_start(pty: &crate::pty::PtyManager, id: &str, after_turn: u32, timeout: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if has_started(id, after_turn) {
            return true;
        }
        if !pty.alive(id) || std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(POLL);
    }
}

/// Asks the frontend to make the chat and start its session; blocks until it
/// answers or the timeout passes.
pub fn spawn(caller: &str, mut params: SpawnParams) -> Result<Spawned, String> {
    if params.prompt.trim().is_empty() {
        return Err("prompt is required".into());
    }
    let (request_id, slot) = {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let me = r.known.get(caller).cloned().ok_or("caller is not a registered session")?;
        if descendants(&r, caller).len() >= MAX_DESCENDANTS {
            return Err(format!("you already have {MAX_DESCENDANTS} sessions; delete some first"));
        }
        if params.tools.unwrap_or(false) && depth_of(&r, caller) >= MAX_TOOL_DEPTH {
            return Err("a session this deep cannot spawn sessions with tools of their own".into());
        }
        if params.folder.is_none() {
            params.folder = Some(me.folder.clone());
        }
        let provider = match params.provider.as_deref() {
            None => me.provider,
            Some("claude") => Provider::Claude,
            Some("codex") => Provider::Codex,
            Some(other) => return Err(format!("unknown provider {other}; use claude or codex")),
        };
        params.provider = Some(provider.as_str().to_string());
        let account = params.account.clone().unwrap_or_else(|| if provider == me.provider { me.account.clone() } else { String::new() });
        let allowed = allowed_accounts();
        let pick = if account.is_empty() {
            allowed.iter().find(|a| a.provider == provider).map(|a| a.name.clone())
        } else {
            allowed.iter().find(|a| a.provider == provider && a.name == account).map(|a| a.name.clone())
        };
        let Some(account) = pick else {
            return Err(format!(
                "no {} account named {account:?} is available to agents; luna_list shows the ones that are",
                provider.as_str()
            ));
        };
        params.account = Some(account);
        let id = r.next_request;
        r.next_request += 1;
        let slot = std::sync::Arc::new((Mutex::new(Pending::default()), Condvar::new()));
        r.pending.insert(id, slot.clone());
        (id, slot)
    };

    let Some(app) = app() else { return Err("app not ready".into()) };
    crate::emit::to_ui(app, "agent://spawn", SpawnRequest { request_id, parent_id: caller.to_string(), params });

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
    let outcome = p.done.take();
    drop(p);
    reg().lock().unwrap_or_else(|e| e.into_inner()).pending.remove(&request_id);
    let id = match outcome {
        Some(Ok(id)) => id,
        Some(Err(e)) => return Err(e),
        None => return Err("the app did not start the session in time".into()),
    };
    // The frontend has registered the session by now (ensure_* ran before it
    // answered), so its settings are known; whether it took the prompt is
    // what the wait below finds out.
    let settings = reg()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .known
        .get(&id)
        .map(|k| k.settings.clone())
        .unwrap_or(serde_json::Value::Null);
    let pty = app.state::<crate::pty::PtyManager>();
    let started = await_start(&pty, &id, 0, START_TIMEOUT);
    let starting = !started && still_starting(&pty, &id);
    if starting {
        crate::log::info("agents", &format!("session {id} spawned by {caller} has drawn nothing within {START_TIMEOUT:?}; still starting"));
    } else if !started {
        crate::log::warn("agents", &format!("session {id} spawned by {caller} did not start on its prompt within {START_TIMEOUT:?}"));
    }
    let screen = (!started && !starting).then(|| screen_of(&pty, &id)).flatten();
    shown(caller, &id, 0);
    owe(caller, &id, 0);
    Ok(Spawned { id, turns_ended: 0, settings, started, screen, starting })
}

/// The frontend's answer to `agent://spawn`.
#[tauri::command]
pub fn agent_spawned(request_id: u64, chat_id: Option<String>, name: Option<String>, error: Option<String>) {
    let slot = reg().lock().unwrap_or_else(|e| e.into_inner()).pending.get(&request_id).cloned();
    if let (Some(id), Some(n)) = (&chat_id, &name) {
        set_name(id, n);
    }
    let Some(slot) = slot else { return };
    let (lock, cv) = &*slot;
    let mut p = lock.lock().unwrap_or_else(|e| e.into_inner());
    p.done = Some(match (chat_id, error) {
        (Some(id), _) => Ok(id),
        (None, Some(e)) => Err(e),
        (None, None) => Err("spawn failed".into()),
    });
    cv.notify_all();
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sent {
    pub ok: bool,
    /// The turn state when the text went in. `busy` means the CLI has queued
    /// the text behind the turn in flight, and `started` says nothing new.
    pub turn_before: Option<crate::activity::Turn>,
    /// A turn began within SEND_TIMEOUT of Enter. False on an idle session
    /// means the text did not become a prompt: `screen` shows where it went.
    pub started: bool,
    /// Turns over before this text went in — the `afterTurn` for the `wait`
    /// that collects the reply.
    pub turns_ended: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screen: Option<String>,
}

/// Types text into a session and presses Enter after it; returns whether a
/// turn began.
fn type_in(
    pty: &crate::pty::PtyManager,
    id: &str,
    text: &str,
    turn_before: Option<crate::activity::Turn>,
    baseline: u32,
) -> Result<bool, String> {
    pty.write(id, text.as_bytes())?;
    std::thread::sleep(ENTER_GAP);
    pty.write(id, b"\r")?;
    // Mid-turn the text is queued by the CLI and nothing here can tell when
    // it is taken up; only an idle session is expected to start now.
    Ok(if turn_before == Some(crate::activity::Turn::Busy) {
        true
    } else if await_start(pty, id, baseline, ENTER_RETRY) {
        true
    } else {
        pty.write(id, b"\r")?;
        await_start(pty, id, baseline, SEND_TIMEOUT.saturating_sub(ENTER_RETRY))
    })
}

/// Types text into the session, presses Enter after it, and reports whether
/// a turn began.
pub fn send(caller: &str, id: &str, text: &str) -> Result<Sent, String> {
    authorize_send(caller, id)?;
    let app = app().ok_or("app not ready")?;
    let pty = app.state::<crate::pty::PtyManager>();
    if !pty.alive(id) {
        return Err(format!("session {id} is not running"));
    }
    if still_starting(&pty, id) {
        return Err(format!(
            "session {id} has not come up yet (its terminal is still blank), so typed text would be lost;              an opening prompt from luna_spawn is held and runs once it is up — luna_wait(turn_done) for the reply"
        ));
    }
    let before = crate::activity::turn_state(id);
    let turn_before = before.map(|t| t.turn);
    let baseline = before.map(|t| t.turns_ended).unwrap_or(0);
    let started = type_in(&pty, id, text, turn_before, baseline)?;
    if turn_before == Some(crate::activity::Turn::Waiting) {
        reg().lock().unwrap_or_else(|e| e.into_inner()).answered.insert((caller.to_string(), id.to_string()), now_ms());
    }
    let screen = (!started).then(|| screen_of(&pty, id)).flatten();
    // The count before the text went in: the reply to it ends the next
    // turn, which the caller has not seen even if it is already over.
    shown(caller, id, baseline);
    owe(caller, id, baseline);
    Ok(Sent { ok: true, turn_before, started, turns_ended: baseline, screen })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reading {
    pub messages: Vec<Message>,
    /// Pass back as `since` to read only what came after.
    pub cursor: u64,
    pub turn: Option<crate::activity::Turn>,
    pub busy: bool,
    pub turns_ended: u32,
    pub alive: bool,
    /// The terminal's last lines, when asked for (`screen: true`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screen: Option<String>,
    /// See `still_starting`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub starting: bool,
}

fn transcript_path(pty: &crate::pty::PtyManager, id: &str) -> Option<std::path::PathBuf> {
    let p = pty.probe(id)?;
    match p.provider {
        Provider::Claude => crate::claude::session::transcript_of(p.pid, &p.cwd, p.spawned_at_ms, &p.account_path),
        Provider::Codex => crate::codex::session::rollout_for(&crate::codex::session::Live {
            chat_id: &p.id,
            cwd: &p.cwd,
            spawned_at_ms: p.spawned_at_ms,
            resume: p.resume.as_deref(),
            account_path: &p.account_path,
            last_output_ms: p.last_output_ms,
        }),
    }
}

/// The conversation of a session from `since` (a cursor from an earlier read;
/// None = from the start), trimmed to the last `last` messages when asked.
pub fn read(
    caller: &str,
    id: &str,
    since: Option<u64>,
    last: Option<usize>,
    with_tools: bool,
    with_screen: bool,
) -> Result<Reading, String> {
    let known = authorize_view(caller, id)?;
    let app = app().ok_or("app not ready")?;
    let pty = app.state::<crate::pty::PtyManager>();
    let (mut messages, cursor) = match transcript_path(&pty, id) {
        Some(path) => match known.provider {
            Provider::Claude => crate::claude::session::messages_from(&path, since.unwrap_or(0), with_tools),
            Provider::Codex => crate::codex::session::messages_from(&path, since.unwrap_or(0)),
        },
        None => (Vec::new(), since.unwrap_or(0)),
    };
    if let Some(n) = last {
        let n = n.max(1);
        if messages.len() > n {
            messages.drain(..messages.len() - n);
        }
    }
    let ts = crate::activity::turn_state(id);
    let turns_ended = ts.map(|t| t.turns_ended).unwrap_or(0);
    shown(caller, id, turns_ended);
    Ok(Reading {
        messages,
        cursor,
        turn: ts.map(|t| t.turn),
        busy: ts.map(|t| t.busy).unwrap_or(false),
        turns_ended,
        alive: pty.alive(id),
        screen: with_screen.then(|| screen_of(&pty, id)).flatten(),
        starting: still_starting(&pty, id),
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Waited {
    pub outcome: &'static str,
    pub turn: Option<crate::activity::Turn>,
    pub turns_ended: u32,
    pub alive: bool,
    /// On `turn_done`: what the session said last, so no `read` is needed.
    pub reply: Option<String>,
    /// On `timeout`, `waiting` and a turn that ended without a reply: the
    /// terminal's last lines, so the caller sees what the session is on — a
    /// permission prompt, a question, an error, a notice nobody dismissed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screen: Option<String>,
    /// On `timeout`: see `still_starting`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub starting: bool,
}

/// Blocks until the session reaches `until`, or `timeout_s` passes:
/// `turn_done` — a turn ended after `after_turn` (default: the count the
/// caller was last shown, so the reply to a prompt it just sent counts even
/// if it landed before this call, and a turn it has already read does not —
/// or the turn stopped on a prompt, which nothing but an answer moves on);
/// `waiting` — it stopped for a permission; `exit` — the process ended.
pub fn wait(caller: &str, id: &str, until: &str, timeout_s: u64, after_turn: Option<u32>) -> Result<Waited, String> {
    authorize_view(caller, id)?;
    let app = app().ok_or("app not ready")?;
    let pty = app.state::<crate::pty::PtyManager>();
    let baseline = after_turn
        .or_else(|| last_shown(caller, id))
        .unwrap_or_else(|| crate::activity::turn_state(id).map(|t| t.turns_ended).unwrap_or(0));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_s.clamp(1, 1800));
    let answered_at = reg().lock().unwrap_or_else(|e| e.into_inner()).answered.get(&(caller.to_string(), id.to_string())).copied();
    loop {
        let alive = pty.alive(id);
        let ts = crate::activity::turn_state(id);
        let turns_ended = ts.map(|t| t.turns_ended).unwrap_or(0);
        let turn = ts.map(|t| t.turn);
        let ended = turns_ended > baseline && turn != Some(crate::activity::Turn::Busy);
        // A turn stopped on a prompt will not end by itself: `turn_done`
        // hands it back too, unless it is the prompt the caller has just
        // answered and the sampler has not caught up with yet.
        let asking = turn == Some(crate::activity::Turn::Waiting)
            && answered_at.map_or(true, |at| now_ms().saturating_sub(at) >= ANSWERED_MS);
        let done = match until {
            "exit" => !alive,
            "waiting" => turn == Some(crate::activity::Turn::Waiting) || !alive,
            _ => ended || asking || !alive,
        };
        if done {
            let outcome = if !alive {
                "exit"
            } else if until == "waiting" || (asking && !ended) {
                "waiting"
            } else {
                "turn_done"
            };
            // The last message, if it is the session's: one that is still
            // the prompt means the turn ended on an error, which the screen
            // shows and the transcript does not.
            let reply_now = || {
                read(caller, id, None, Some(1), false, false)
                    .ok()
                    .and_then(|r| r.messages.into_iter().last().filter(|m| m.role == "assistant").map(|m| m.text))
            };
            let mut reply = (outcome == "turn_done").then(reply_now).flatten();
            let late = std::time::Instant::now() + std::time::Duration::from_millis(REPLY_GRACE_MS);
            while outcome == "turn_done" && reply.is_none() && std::time::Instant::now() < late {
                std::thread::sleep(POLL);
                reply = reply_now();
            }
            let bare = outcome == "turn_done" && reply.is_none();
            let screen = (outcome == "waiting" || bare).then(|| screen_of(&pty, id)).flatten();
            shown(caller, id, turns_ended);
            return Ok(Waited { outcome, turn, turns_ended, alive, reply, screen, starting: false });
        }
        if std::time::Instant::now() >= deadline {
            let screen = screen_of(&pty, id);
            let starting = still_starting(&pty, id);
            shown(caller, id, turns_ended);
            return Ok(Waited { outcome: "timeout", turn, turns_ended, alive, reply: None, screen, starting });
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

/// Stops the session; the chat row stays and can be resumed by hand.
pub fn kill(caller: &str, id: &str) -> Result<(), String> {
    authorize(caller, id)?;
    let app = app().ok_or("app not ready")?;
    let pty = app.state::<crate::pty::PtyManager>();
    tauri::async_runtime::block_on(pty.kill(id));
    Ok(())
}

/// Stops the session, removes its attachments and — when asked — the
/// worktree it ran in, then tells the frontend to drop the row. A session's
/// own children are left where they are, orphaned.
pub fn delete(caller: &str, id: &str, drop_worktree: bool) -> Result<Option<String>, String> {
    let known = authorize(caller, id)?;
    let app = app().ok_or("app not ready")?;
    let pty = app.state::<crate::pty::PtyManager>();
    // A Codex helper in a worktree knows the worktree as its folder.
    let folder = crate::worktree::project_of(&known.folder).unwrap_or(known.folder);
    let worktree = tauri::async_runtime::block_on(pty.delete(id, &folder, &known.account_path, None, drop_worktree))?;
    {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        r.known.remove(id);
        r.tokens.retain(|_, chat| chat != id);
        r.seen.retain(|(_, chat), _| chat != id);
        r.owed.retain(|(who, chat), _| who != id && chat != id);
        r.answered.retain(|(who, chat), _| who != id && chat != id);
    }
    crate::emit::to_ui(app, "agent://deleted", Deleted { id: id.to_string() });
    Ok(worktree)
}

// --------------------------------------------------------------- notifier --

/// What became of a session the caller is owed an answer by.
enum Settled {
    /// A turn past the one asked about ended.
    Answered,
    /// A turn ended, but the session left a job running in the background;
    /// owed again from this turn count, for the turn that job will end in.
    Pending(u32),
    /// A turn ended but is not settled — processes still at work, or no
    /// reply in the transcript yet: nothing to say, start the grace from now.
    Hold(u32),
    Waiting,
    Exited,
}

/// What the session said after the last prompt, read straight off its
/// transcript; None when the prompt is the last word (the turn ended on an
/// error, an interruption).
fn last_reply(pty: &crate::pty::PtyManager, id: &str, provider: Provider) -> Option<String> {
    let path = transcript_path(pty, id)?;
    let (messages, _) = match provider {
        Provider::Claude => crate::claude::session::messages_from(&path, 0, false),
        Provider::Codex => crate::codex::session::messages_from(&path, 0),
    };
    messages.into_iter().last().filter(|m| m.role == "assistant").map(|m| m.text)
}

/// One line for a notice: whitespace folded (a newline would submit the
/// notice early, or turn it into a paste the CLI shows as a placeholder) and
/// the length capped.
fn quote(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= NOTICE_QUOTE {
        return flat;
    }
    let cut: String = flat.chars().take(NOTICE_QUOTE).collect();
    format!("{cut}…")
}

/// The notice for one session, or None while nothing has happened to it.
fn notice_line(
    pty: &crate::pty::PtyManager,
    who: &str,
    id: &str,
    k: &Known,
    o: Owed,
) -> Option<(Option<String>, Settled)> {
    let ts = crate::activity::turn_state(id);
    let ended = ts.map(|t| t.turns_ended).unwrap_or(0);
    let turn = ts.map(|t| t.turn);
    if ended > o.baseline && turn != Some(crate::activity::Turn::Busy) {
        // Claude Code ends a turn on a job it put in the background and
        // takes it up again in a turn of its own when the job is done.
        let background = ts.is_some_and(|t| t.children_busy) && pty.alive(id);
        let reply = last_reply(pty, id, k.provider);
        let settle_ms = if background {
            BACKGROUND_GRACE_MS
        } else if reply.is_none() {
            REPLY_GRACE_MS
        } else {
            0
        };
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
        let reply = reply
            .map(|r| format!(" and said: \"{}\"", quote(&r)))
            .unwrap_or_else(|| " without a reply (an error or an interruption; luna_read with screen: true shows it)".into());
        if background {
            return Some((
                // Said first and plainly: a weak model read "ended a turn" as
                // done and deleted the helper mid-job.
                Some(format!("{who} is NOT done yet: a job it started still runs in the background. It ended a turn{reply}. Wait for the next notice, which comes when it finishes.")),
                Settled::Pending(ended),
            ));
        }
        return Some((Some(format!("{who} finished its turn{reply}.")), Settled::Answered));
    }
    if !pty.alive(id) {
        return Some((Some(format!("{who} has exited.")), Settled::Exited));
    }
    if turn == Some(crate::activity::Turn::Waiting) && !o.told_waiting {
        return Some((
            Some(format!("{who} is stopped on a prompt in its terminal; luna_read with screen: true shows it.")),
            Settled::Waiting,
        ));
    }
    None
}

/// Whether a notice may go into `caller` now: its CLI is up and between turns,
/// nobody is typing into it, and nothing typed waits unsent in its composer
/// (a notice and its Enter would send the draft along with it).
fn ready_for_notice(pty: &crate::pty::PtyManager, caller: &str) -> bool {
    pty.alive(caller)
        && crate::activity::turn_state(caller).is_some_and(|t| t.turn == crate::activity::Turn::Idle)
        && !still_starting(pty, caller)
        && pty.typed_ago_ms(caller).map_or(true, |ms| ms >= TYPING_QUIET_MS)
        && !pty.has_draft(caller)
}

/// Whether any of these owed answers has come in, without saying it yet.
fn anything_to_say(pty: &crate::pty::PtyManager, items: &[(String, Known, Owed, String)]) -> bool {
    items.iter().any(|(id, _, o, _)| {
        let ts = crate::activity::turn_state(id);
        let ended = ts.map(|t| t.turns_ended).unwrap_or(0);
        (ended > o.baseline && ts.map(|t| t.turn) != Some(crate::activity::Turn::Busy)) || !pty.alive(id)
    })
}

/// Runs for the life of the app: every few seconds, for each caller owed an
/// answer, checks whether one came and tells the caller once it is idle.
fn notifier() {
    let mut idle_looks: HashMap<String, u32> = HashMap::new();
    // Callers whose notice a draft is holding back: since when, and whether
    // the desktop has been told.
    let mut drafts: HashMap<String, (u64, bool)> = HashMap::new();
    loop {
        std::thread::sleep(NOTIFY_EVERY);
        let Some(app) = app() else { continue };
        let pty = app.state::<crate::pty::PtyManager>();
        // Owed by caller, each session with what is known of it; anything a
        // delete has made unknown is dropped on the way.
        let mut by_caller: std::collections::BTreeMap<String, Vec<(String, Known, Owed, String)>> = Default::default();
        {
            let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
            let Registry { owed, known, names, .. } = &mut *r;
            owed.retain(|(caller, id), _| known.contains_key(caller) && known.contains_key(id));
            for ((caller, id), o) in owed.iter() {
                let Some(k) = known.get(id) else { continue };
                let parent = k.parent.as_deref() == Some(caller.as_str());
                let name = names.get(id).map(|n| n.trim()).filter(|n| !n.is_empty());
                let who = match (parent, name) {
                    (true, Some(n)) => format!("Your helper \"{n}\" ({id})"),
                    (true, None) => format!("Your helper {id}"),
                    (false, Some(n)) => format!("Session \"{n}\" ({id}), which you wrote to,"),
                    (false, None) => format!("Session {id}, which you wrote to,"),
                };
                by_caller.entry(caller.clone()).or_default().push((id.clone(), k.clone(), *o, who));
            }
        }
        idle_looks.retain(|c, _| by_caller.contains_key(c));
        drafts.retain(|c, _| by_caller.contains_key(c));
        for (caller, items) in by_caller {
            if pty.has_draft(&caller) && pty.alive(&caller) && anything_to_say(&pty, &items) {
                let (since, told) = drafts.entry(caller.clone()).or_insert((now_ms(), false));
                if !*told && now_ms().saturating_sub(*since) >= DRAFT_ALERT_MS {
                    *told = true;
                    let chat = chat_name(&caller).unwrap_or_else(|| caller.clone());
                    crate::power::notify(
                        app,
                        "A helper has answered",
                        &format!("\"{chat}\" has an unsent draft, so Luna is holding the helper's notice until you send or clear it."),
                    );
                }
            } else {
                drafts.remove(&caller);
            }
            let looks = idle_looks.entry(caller.clone()).or_default();
            *looks = if ready_for_notice(&pty, &caller) { *looks + 1 } else { 0 };
            if *looks < NOTIFY_IDLE_LOOKS {
                continue;
            }
            let mut lines = Vec::new();
            let mut settled = Vec::new();
            for (id, k, o, who) in &items {
                match notice_line(&pty, who, id, k, *o) {
                    Some((Some(line), how)) => {
                        lines.push(line);
                        settled.push((id.clone(), how));
                    }
                    Some((None, Settled::Hold(ended))) => {
                        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(o) = r.owed.get_mut(&(caller.clone(), id.clone())) {
                            o.held = Some((ended, now_ms()));
                        }
                    }
                    _ => {}
                }
            }
            if lines.is_empty() {
                continue;
            }
            let text = format!("[Luna notice, not typed by the user] {} (luna_read gives the full text)", lines.join(" "));
            let before = crate::activity::turn_state(&caller);
            let baseline = before.map(|t| t.turns_ended).unwrap_or(0);
            match type_in(&pty, &caller, &text, before.map(|t| t.turn), baseline) {
                Ok(started) => {
                    if !started {
                        crate::log::warn("agents", &format!("notice to {caller} did not start a turn"));
                    }
                    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
                    for (id, how) in settled {
                        let key = (caller.clone(), id);
                        match how {
                            Settled::Answered | Settled::Exited => {
                                r.owed.remove(&key);
                            }
                            Settled::Waiting => {
                                if let Some(o) = r.owed.get_mut(&key) {
                                    o.told_waiting = true;
                                }
                            }
                            Settled::Pending(ended) => {
                                r.owed.insert(key, Owed { baseline: ended, told_waiting: false, held: None });
                            }
                            Settled::Hold(_) => {}
                        }
                    }
                }
                Err(e) => crate::log::warn("agents", &format!("notice to {caller} failed: {e}")),
            }
            idle_looks.insert(caller, 0);
        }
    }
}

// --------------------------------------------------------------- commands --

/// Every chat's name, sent whenever one changes: what `luna_sessions` calls
/// the user's own chats, which the core otherwise knows only by id.
#[tauri::command]
pub fn chat_names(names: HashMap<String, String>) {
    reg().lock().unwrap_or_else(|e| e.into_inner()).names = names;
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
        assert!(authorize_send("a", "x").is_ok(), "text may go to the user's own chats");
        assert!(authorize_send("b", "a").is_ok(), "and upwards");
        assert!(authorize_send("a", "a").is_err(), "but not into its own composer");
        assert!(authorize_send("a", "nobody").is_err());
        assert!(authorize_view("b", "a").is_ok(), "a session written to can be read and waited on");
        assert!(authorize_view("a", "a").is_err(), "but not one's own");
        let r = reg().lock().unwrap();
        assert_eq!(depth_of(&r, "c"), 2);
        let mut d = descendants(&r, "a");
        d.sort();
        assert_eq!(d, vec!["b", "c"]);
    }

    #[test]
    fn an_answer_is_owed_until_the_caller_sees_a_later_turn() {
        let none = serde_json::json!({});
        register("o-parent", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone());
        register("o-child", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("o-parent"), false, none);
        let owed = || reg().lock().unwrap().owed.get(&("o-parent".to_string(), "o-child".to_string())).map(|o| o.baseline);
        owe("o-parent", "o-child", 1);
        shown("o-parent", "o-child", 1);
        assert_eq!(owed(), Some(1), "the send itself shows the count it went in at");
        shown("o-parent", "o-child", 2);
        assert_eq!(owed(), None, "a wait or read past it settles the answer");
        owe(DEV_CALLER, "o-child", 0);
        assert!(reg().lock().unwrap().owed.get(&(DEV_CALLER.to_string(), "o-child".to_string())).is_none());
        assert_eq!(quote("a\n\nb   c"), "a b c");
        assert!(quote(&"x".repeat(1000)).chars().count() == NOTICE_QUOTE + 1);
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
}
