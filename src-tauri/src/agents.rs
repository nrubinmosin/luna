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
//! descendants. Children outlive their parent; the UI marks them orphaned.

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
/// screen so the caller can see which.
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
    name: String,
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
        })
    })
}

/// Records what a tool answer told `caller` about `id`'s turn count.
fn shown(caller: &str, id: &str, turns_ended: u32) {
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    r.seen.insert((caller.to_string(), id.to_string()), turns_ended);
}

/// The last count `caller` was shown for `id`, if any.
fn last_shown(caller: &str, id: &str) -> Option<u32> {
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    r.seen.get(&(caller.to_string(), id.to_string())).copied()
}

static APP: OnceLock<tauri::AppHandle> = OnceLock::new();

pub fn init(app: tauri::AppHandle) {
    let _ = APP.set(app);
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
            name: previous.map(|k| k.name).unwrap_or_default(),
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

/// The frontend's name for a chat, for `list` — set when it spawns one.
pub fn set_name(id: &str, name: &str) {
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(k) = r.known.get_mut(id) {
        k.name = name.to_string();
    }
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
    let rows: Vec<(String, Known)> = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        descendants(&r, caller).into_iter().filter_map(|id| r.known.get(&id).cloned().map(|k| (id, k))).collect()
    };
    let pty = app().map(|a| a.state::<crate::pty::PtyManager>());
    let sessions = rows
        .into_iter()
        .map(|(id, k)| {
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
                name: k.name,
                provider: k.provider,
                account: k.account,
                parent: k.parent,
                settings: k.settings,
            }
        })
        .collect();
    Listing { me: caller.to_string(), accounts: allowed_accounts(), sessions }
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
    /// The terminal as it stands, only when `started` is false.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screen: Option<String>,
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
    crate::activity::turn_state(id)
        .map(|t| t.turn == crate::activity::Turn::Busy || t.turns_ended > after_turn)
        .unwrap_or(false)
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
    if !started {
        crate::log::warn("agents", &format!("session {id} spawned by {caller} did not start on its prompt within {START_TIMEOUT:?}"));
    }
    let screen = (!started).then(|| screen_of(&pty, &id)).flatten();
    shown(caller, &id, 0);
    Ok(Spawned { id, turns_ended: 0, settings, started, screen })
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

/// Types text into the session, presses Enter after it, and reports whether
/// a turn began.
pub fn send(caller: &str, id: &str, text: &str) -> Result<Sent, String> {
    authorize(caller, id)?;
    let app = app().ok_or("app not ready")?;
    let pty = app.state::<crate::pty::PtyManager>();
    if !pty.alive(id) {
        return Err(format!("session {id} is not running"));
    }
    let before = crate::activity::turn_state(id);
    let turn_before = before.map(|t| t.turn);
    let baseline = before.map(|t| t.turns_ended).unwrap_or(0);
    pty.write(id, text.as_bytes())?;
    std::thread::sleep(ENTER_GAP);
    pty.write(id, b"\r")?;
    // Mid-turn the text is queued by the CLI and nothing here can tell when
    // it is taken up; only an idle session is expected to start now.
    let started = if turn_before == Some(crate::activity::Turn::Busy) {
        true
    } else if await_start(&pty, id, baseline, ENTER_RETRY) {
        true
    } else {
        pty.write(id, b"\r")?;
        await_start(&pty, id, baseline, SEND_TIMEOUT.saturating_sub(ENTER_RETRY))
    };
    let screen = (!started).then(|| screen_of(&pty, id)).flatten();
    // The count before the text went in: the reply to it ends the next
    // turn, which the caller has not seen even if it is already over.
    shown(caller, id, baseline);
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
    let known = authorize(caller, id)?;
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
    /// On `timeout` and `waiting`: the terminal's last lines, so the caller
    /// sees what the session is on — a permission prompt, a question, a
    /// notice nobody dismissed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screen: Option<String>,
}

/// Blocks until the session reaches `until`, or `timeout_s` passes:
/// `turn_done` — a turn ended after `after_turn` (default: the count the
/// caller was last shown, so the reply to a prompt it just sent counts even
/// if it landed before this call, and a turn it has already read does not);
/// `waiting` — it stopped for a permission; `exit` — the process ended.
pub fn wait(caller: &str, id: &str, until: &str, timeout_s: u64, after_turn: Option<u32>) -> Result<Waited, String> {
    authorize(caller, id)?;
    let app = app().ok_or("app not ready")?;
    let pty = app.state::<crate::pty::PtyManager>();
    let baseline = after_turn
        .or_else(|| last_shown(caller, id))
        .unwrap_or_else(|| crate::activity::turn_state(id).map(|t| t.turns_ended).unwrap_or(0));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_s.clamp(1, 1800));
    loop {
        let alive = pty.alive(id);
        let ts = crate::activity::turn_state(id);
        let turns_ended = ts.map(|t| t.turns_ended).unwrap_or(0);
        let turn = ts.map(|t| t.turn);
        let done = match until {
            "exit" => !alive,
            "waiting" => turn == Some(crate::activity::Turn::Waiting) || !alive,
            _ => (turns_ended > baseline && turn != Some(crate::activity::Turn::Busy)) || !alive,
        };
        if done {
            let outcome = if !alive {
                "exit"
            } else if until == "waiting" {
                "waiting"
            } else {
                "turn_done"
            };
            let reply = (outcome == "turn_done")
                .then(|| read(caller, id, None, Some(1), false, false).ok())
                .flatten()
                .and_then(|r| r.messages.into_iter().rev().find(|m| m.role == "assistant").map(|m| m.text));
            let screen = (outcome == "waiting").then(|| screen_of(&pty, id)).flatten();
            shown(caller, id, turns_ended);
            return Ok(Waited { outcome, turn, turns_ended, alive, reply, screen });
        }
        if std::time::Instant::now() >= deadline {
            let screen = screen_of(&pty, id);
            shown(caller, id, turns_ended);
            return Ok(Waited { outcome: "timeout", turn, turns_ended, alive, reply: None, screen });
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
    let worktree = tauri::async_runtime::block_on(pty.delete(id, &known.folder, &known.account_path, None, drop_worktree))?;
    {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        r.known.remove(id);
        r.tokens.retain(|_, chat| chat != id);
        r.seen.retain(|(_, chat), _| chat != id);
    }
    crate::emit::to_ui(app, "agent://deleted", Deleted { id: id.to_string() });
    Ok(worktree)
}

// --------------------------------------------------------------- commands --

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
        let r = reg().lock().unwrap();
        assert_eq!(depth_of(&r, "c"), 2);
        let mut d = descendants(&r, "a");
        d.sort();
        assert_eq!(d, vec!["b", "c"]);
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
