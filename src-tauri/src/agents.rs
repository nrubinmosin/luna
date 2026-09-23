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
const ENTER_GAP: std::time::Duration = std::time::Duration::from_millis(80);

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
}

fn reg() -> &'static Mutex<Registry> {
    static R: OnceLock<Mutex<Registry>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(Registry { tokens: HashMap::new(), known: HashMap::new(), pending: HashMap::new(), next_request: 1 }))
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
            SessionRow {
                alive: pty.as_ref().map(|p| p.alive(&id)).unwrap_or(false),
                turn: ts.map(|t| t.turn),
                busy: ts.map(|t| t.busy).unwrap_or(false),
                turns_ended: ts.map(|t| t.turns_ended).unwrap_or(0),
                id,
                name: k.name,
                provider: k.provider,
                account: k.account,
                parent: k.parent,
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
    match outcome {
        Some(Ok(id)) => Ok(Spawned { id, turns_ended: 0 }),
        Some(Err(e)) => Err(e),
        None => Err("the app did not start the session in time".into()),
    }
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

/// Types text into the session and presses Enter after it.
pub fn send(caller: &str, id: &str, text: &str) -> Result<(), String> {
    authorize(caller, id)?;
    let app = app().ok_or("app not ready")?;
    let pty = app.state::<crate::pty::PtyManager>();
    if !pty.alive(id) {
        return Err(format!("session {id} is not running"));
    }
    pty.write(id, text.as_bytes())?;
    std::thread::sleep(ENTER_GAP);
    pty.write(id, b"\r")
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
pub fn read(caller: &str, id: &str, since: Option<u64>, last: Option<usize>, with_tools: bool) -> Result<Reading, String> {
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
    Ok(Reading {
        messages,
        cursor,
        turn: ts.map(|t| t.turn),
        busy: ts.map(|t| t.busy).unwrap_or(false),
        turns_ended: ts.map(|t| t.turns_ended).unwrap_or(0),
        alive: pty.alive(id),
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
}

/// Blocks until the session reaches `until`, or `timeout_s` passes:
/// `turn_done` — a turn ended after `after_turn` (default: the count when
/// the call was made, so a turn already over does not count twice);
/// `waiting` — it stopped for a permission; `exit` — the process ended.
pub fn wait(caller: &str, id: &str, until: &str, timeout_s: u64, after_turn: Option<u32>) -> Result<Waited, String> {
    authorize(caller, id)?;
    let app = app().ok_or("app not ready")?;
    let pty = app.state::<crate::pty::PtyManager>();
    let baseline = after_turn.unwrap_or_else(|| crate::activity::turn_state(id).map(|t| t.turns_ended).unwrap_or(0));
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
                .then(|| read(caller, id, None, Some(1), false).ok())
                .flatten()
                .and_then(|r| r.messages.into_iter().rev().find(|m| m.role == "assistant").map(|m| m.text));
            return Ok(Waited { outcome, turn, turns_ended, alive, reply });
        }
        if std::time::Instant::now() >= deadline {
            return Ok(Waited { outcome: "timeout", turn, turns_ended, alive, reply: None });
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
        let a = register("a", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true).expect("token");
        assert_eq!(caller_of(&a).as_deref(), Some("a"));
        register("b", Provider::Codex, "C:\\p", "C:\\acc\\openai\\two", Some("a"), false);
        register("c", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("b"), false);
        register("x", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, false);
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
        let t1 = register("r", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true).unwrap();
        let t2 = register("r", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true).unwrap();
        assert_ne!(t1, t2);
        assert_eq!(caller_of(&t1), None);
        assert_eq!(caller_of(&t2).as_deref(), Some("r"));
    }
}
