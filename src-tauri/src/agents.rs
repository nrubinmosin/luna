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
/// The same for `send`, which refuses rather than waits: the agent can try
/// again, and a person who stopped typing this long ago has likely paused.
const SEND_TYPING_MS: u64 = 5_000;
/// A notice held this long by an unsent draft in the caller is announced on
/// the desktop instead, once: the draft may be forgotten, the notice is not.
const DRAFT_ALERT_MS: u64 = 60_000;
/// How much of a reply a notice quotes: enough for most reports whole, so
/// the caller need not read each one back; the rest is a `luna_read` away.
const NOTICE_QUOTE: usize = 3000;
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
/// A `wait` out for an answer hands it over itself, so the notifier holds its
/// notice — this long past the answer at most, for a client that has dropped
/// the call it moved to the background.
const WAIT_HOLD_MS: u64 = 10_000;
/// An `autoDelete` helper goes once nobody is owed its answer and it has sat
/// idle this long since: room for a follow-up first.
const AUTO_DELETE_IDLE_MS: u64 = 10 * 60 * 1000;
/// How every notice opens, so a model tells it from what the user types.
pub const NOTICE_TAG: &str = "[Luna notice, not typed by the user]";
/// The most warning lines a spawn passes on from the helper's screen.
const MAX_WARNINGS: usize = 5;

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct Message {
    pub role: String,
    pub text: String,
}

/// What a session waits on a person for: a question its model put (Claude
/// Code's `AskUserQuestion`, Codex's `request_user_input`), or leave to run a
/// tool. `luna_answer` answers either.
#[derive(Clone, Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Ask {
    /// `question` or `permission`.
    pub kind: &'static str,
    /// The turn holds until it is answered. Codex's async question does not
    /// hold it: the model works on beside it.
    pub blocking: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub questions: Vec<AskQuestion>,
    /// A permission's tool, and what it would do with it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Clone, Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AskQuestion {
    pub text: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    /// More than one option may be picked.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub multi: bool,
}

impl Ask {
    pub fn question(questions: Vec<AskQuestion>, blocking: bool) -> Ask {
        Ask { kind: "question", blocking, questions, tool: None, detail: None }
    }

    pub fn permission(tool: Option<String>, detail: Option<String>) -> Ask {
        Ask { kind: "permission", blocking: true, questions: Vec::new(), tool, detail }
    }

    /// Claude Code's `AskUserQuestion` input: `questions[]` with `question`,
    /// `options[].label` and `multiSelect`.
    pub fn from_claude_questions(input: &serde_json::Value) -> Option<Ask> {
        let questions: Vec<AskQuestion> = input["questions"]
            .as_array()?
            .iter()
            .filter_map(|q| {
                let text = q["question"].as_str()?.trim().to_string();
                let options =
                    q["options"].as_array().map(|o| o.iter().filter_map(|o| o["label"].as_str().map(str::to_owned)).collect()).unwrap_or_default();
                Some(AskQuestion { text, options, multi: q["multiSelect"].as_bool().unwrap_or(false) })
            })
            .collect();
        (!questions.is_empty()).then(|| Ask::question(questions, true))
    }

    /// A Claude Code tool call held for permission: the tool, and the one
    /// field of its input that says what it would do.
    pub fn from_claude_tool(name: &str, input: &serde_json::Value) -> Ask {
        let detail = ["command", "file_path", "path", "url", "pattern", "description"]
            .iter()
            .find_map(|k| input[*k].as_str())
            .map(str::to_owned)
            .or_else(|| input.is_object().then(|| input.to_string()))
            .map(|d| d.chars().take(400).collect());
        Ask::permission(Some(name.to_string()), detail)
    }

    /// One line for a notice: the questions with their options numbered.
    pub fn line(&self) -> String {
        if self.kind == "permission" {
            let what = [self.tool.as_deref(), self.detail.as_deref()].into_iter().flatten().collect::<Vec<_>>().join(": ");
            return if what.is_empty() { "asks for permission".into() } else { format!("asks for permission to run {what}") };
        }
        let qs: Vec<String> = self
            .questions
            .iter()
            .map(|q| {
                let opts: Vec<String> = q.options.iter().enumerate().map(|(i, o)| format!("{}. {o}", i + 1)).collect();
                if opts.is_empty() {
                    format!("\"{}\"", q.text)
                } else {
                    format!("\"{}\" ({})", q.text, opts.join(", "))
                }
            })
            .collect();
        format!("asks: {}", qs.join(" "))
    }
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
    /// Luna's tools were given to it: `tools/list` offers them.
    tools: bool,
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
    /// Luna's to act on (see `reap`); the frontend never sees it.
    #[serde(skip_serializing)]
    pub auto_delete: Option<bool>,
    /// A chat the user sees, run by the window. Without it the helper runs
    /// in the background (background.rs) and the frontend never hears of it.
    #[serde(skip_serializing)]
    pub visible: Option<bool>,
}

/// The request as the frontend receives it.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SpawnRequest {
    request_id: u64,
    parent_id: String,
    /// A worktree the helper is to run in without owning it: its caller's.
    /// The chat is filed under `folder`, the worktree's project.
    in_worktree: Option<String>,
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
    /// Spawns whose caller stopped waiting (SPAWN_TIMEOUT), by request id:
    /// the caller to hand the chat to should the frontend still make it.
    abandoned: HashMap<u64, (String, bool)>,
    next_request: u64,
    /// Helpers that run in a worktree they were handed rather than made:
    /// deleting one never drops that worktree, whose work is its caller's.
    borrowed: std::collections::HashSet<String>,
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
    /// The window group each chat row sits in, 0-based, sent with the names:
    /// sessions the user put side by side in one group are the ones meant to
    /// work things out between them.
    groups: HashMap<String, u32>,
    /// When each caller last typed into a session that was stopped on a
    /// prompt, by (caller, session): the answer to the prompt. For a few
    /// seconds after it the prompt may still look open, and `wait` must not
    /// hand it back as if it were a new one.
    answered: HashMap<(String, String), u64>,
    /// Answers owed, by (caller, session): the caller spawned the session or
    /// typed into it, and has not been shown a turn past `baseline` since.
    /// The notifier tells the caller when the answer comes.
    owed: HashMap<(String, String), Owed>,
    /// `wait` calls in flight, by (caller, session).
    waits: HashMap<(String, String), u32>,
    /// Helpers spawned with `autoDelete`, by id.
    auto: HashMap<String, Auto>,
    /// Each account's usage as the window last showed it, by `provider/name`.
    usage: HashMap<String, serde_json::Value>,
    /// When each caller last raised a desktop notification (`notify`).
    notified: HashMap<String, u64>,
    /// The tool a Claude Code session's hooks last said is up for an answer
    /// (`AskUserQuestion`, or one held for permission), with its input.
    hook_asks: HashMap<String, (String, serde_json::Value)>,
}

/// An `autoDelete` helper's progress towards going.
#[derive(Default)]
struct Auto {
    /// Since when it has been idle with nobody owed its answer.
    idle_since: Option<u64>,
    /// Luna is deleting it now, so its end goes down as `auto-deleted`.
    deleting: bool,
}

#[derive(Clone, Copy)]
struct Owed {
    /// Turns over when the caller asked; the answer ends a later one.
    baseline: u32,
    /// The caller has been told the session is stopped on a prompt.
    told_waiting: bool,
    /// The question the caller was last told the session asks beside its
    /// work, by hash (0: none).
    told_ask: u64,
    /// The turn count at which the answer was seen with processes still at
    /// work, and when (see BACKGROUND_GRACE_MS).
    held: Option<(u32, u64)>,
    /// When the notifier first found the answer in while a `wait` was out
    /// for it (see WAIT_HOLD_MS).
    ready_at: Option<u64>,
}

impl Owed {
    fn from(baseline: u32) -> Owed {
        Owed { baseline, told_waiting: false, told_ask: 0, held: None, ready_at: None }
    }
}

fn reg() -> &'static Mutex<Registry> {
    static R: OnceLock<Mutex<Registry>> = OnceLock::new();
    R.get_or_init(|| {
        Mutex::new(Registry {
            tokens: HashMap::new(),
            known: HashMap::new(),
            pending: HashMap::new(),
            abandoned: HashMap::new(),
            borrowed: Default::default(),
            next_request: 1,
            seen: HashMap::new(),
            names: HashMap::new(),
            groups: HashMap::new(),
            answered: HashMap::new(),
            owed: HashMap::new(),
            waits: HashMap::new(),
            auto: HashMap::new(),
            usage: HashMap::new(),
            notified: HashMap::new(),
            hook_asks: HashMap::new(),
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
    r.owed.insert((caller.to_string(), id.to_string()), Owed::from(baseline));
}

/// A `wait` by `caller` on a session, counted for as long as it runs.
struct WaitOut((String, String));

impl WaitOut {
    fn start(caller: &str, id: &str) -> WaitOut {
        let key = (caller.to_string(), id.to_string());
        *reg().lock().unwrap_or_else(|e| e.into_inner()).waits.entry(key.clone()).or_default() += 1;
        WaitOut(key)
    }
}

impl Drop for WaitOut {
    fn drop(&mut self) {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = r.waits.get_mut(&self.0) {
            *n -= 1;
            if *n == 0 {
                r.waits.remove(&self.0);
            }
        }
    }
}

/// The last count `caller` was shown for `id`, if any.
fn last_shown(caller: &str, id: &str) -> Option<u32> {
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    r.seen.get(&(caller.to_string(), id.to_string())).copied()
}

/// Starts the notifier, once.
pub fn init() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        std::thread::Builder::new().name("luna-notifier".into()).spawn(notifier).expect("spawn notifier thread");
    });
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
            tools,
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

/// A token for a session registered without tools: a background Claude Code
/// helper takes its permission prompts through Luna's MCP server all the
/// same, and `tools/list` offers it that one tool only.
fn issue_token(id: &str) -> String {
    let token = mint(id);
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    r.tokens.retain(|_, chat| chat != id);
    r.tokens.insert(token.clone(), id.to_string());
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

/// A background helper has become a chat the window runs: no longer one
/// Luna deletes on its own, and the window registers it again when its
/// session starts.
pub fn promoted(id: &str) {
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    r.auto.remove(id);
    if let Some(k) = r.known.get_mut(id) {
        if let Some(o) = k.settings.as_object_mut() {
            o.remove("background");
        }
    }
}

/// The window's Delete on a background helper: as its agent would, the
/// worktree it made going too unless it was handed one.
pub fn delete_by_user(id: &str) -> Result<(), String> {
    let known = reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(id).cloned().ok_or("no such helper")?;
    remove(id, known, true).map(|_| ())
}

/// A Claude Code hook (hub.rs): what it puts up for an answer, kept until
/// the tool runs or the turn is over.
pub fn on_hook(chat: &str, body: &str) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else { return };
    let tool = v["tool_name"].as_str().unwrap_or("");
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    match v["hook_event_name"].as_str() {
        Some("PermissionRequest") => {
            r.hook_asks.insert(chat.to_string(), (tool.to_string(), v["tool_input"].clone()));
        }
        Some("PreToolUse") if tool == "AskUserQuestion" => {
            r.hook_asks.insert(chat.to_string(), (tool.to_string(), v["tool_input"].clone()));
        }
        Some("PostToolUse" | "PostToolUseFailure" | "Stop" | "StopFailure" | "UserPromptSubmit" | "SessionEnd") => {
            r.hook_asks.remove(chat);
        }
        _ => {}
    }
}

/// Whether the session is there to talk to:a pty session that runs, or a
/// background helper, which is there between its turns too.
fn live(pty: &crate::pty::PtyManager, id: &str) -> bool {
    crate::background::has(id) || pty.alive(id)
}

/// What a session waits on a person for, if anything: a background helper's
/// held call; a Claude Code `AskUserQuestion` with no answer yet, or the tool
/// a permission prompt is up for; a Codex question tool not yet answered, or
/// the approval its screen shows.
pub fn ask_of(id: &str) -> Option<Ask> {
    if crate::background::has(id) {
        return crate::background::ask(id);
    }
    let pty = crate::pty::manager();
    let p = pty.probe(id)?;
    let turn = crate::activity::turn_state(id).map(|t| t.turn);
    let waiting = turn == Some(crate::activity::Turn::Waiting);
    match p.provider {
        Provider::Claude => {
            // The hooks say it whole; the transcript has a tool call held for
            // permission only sometimes, and a question not before its answer.
            let hooked = reg().lock().unwrap_or_else(|e| e.into_inner()).hook_asks.get(id).cloned();
            let (name, input) = hooked
                .filter(|_| turn.is_some_and(|t| t != crate::activity::Turn::Idle))
                .or_else(|| waiting.then(|| crate::claude::session::pending_tool(&transcript_path(&pty, id)?)).flatten())?;
            if name == "AskUserQuestion" {
                Ask::from_claude_questions(&input)
            } else {
                waiting.then(|| Ask::from_claude_tool(&name, &input))
            }
        }
        Provider::Codex => crate::codex::session::pending_ask(&crate::codex::session::Live {
            chat_id: &p.id,
            cwd: &p.cwd,
            spawned_at_ms: p.spawned_at_ms,
            resume: p.resume.as_deref(),
            account_path: &p.account_path,
            last_output_ms: p.last_output_ms,
        })
        .or_else(|| waiting.then(|| Ask::permission(None, pty.screen(id).as_deref().and_then(codex_command_on)))),
    }
}

/// The command a Codex approval dialog shows, on its `$ ` line.
fn codex_command_on(screen: &str) -> Option<String> {
    screen.lines().map(str::trim).find_map(|l| l.strip_prefix("$ ")).map(|c| c.trim().to_string()).filter(|c| !c.is_empty())
}

/// One word for where a session stands, and what it asks if it asks:
/// `working`, `waiting` (a permission), `question` (a question that holds its
/// turn), `background` (its turn is over, a job it started still runs),
/// `idle`, `starting` (its CLI has not drawn yet), `exited`.
pub fn state_of(id: &str) -> (&'static str, Option<Ask>) {
    use crate::activity::Turn;
    let pty = crate::pty::manager();
    if !live(&pty, id) {
        return ("exited", None);
    }
    if still_starting(&pty, id) {
        return ("starting", None);
    }
    let ask = ask_of(id);
    let ts = crate::activity::turn_state(id);
    let state = match (ts.map(|t| t.turn), &ask) {
        (_, Some(a)) if a.blocking && a.kind == "question" => "question",
        (Some(Turn::Waiting), _) => "waiting",
        (Some(Turn::Busy), _) => "working",
        _ if ts.is_some_and(|t| t.children_busy) => "background",
        _ => "idle",
    };
    (state, ask)
}

/// An answer to one question as the CLI is to get it: an option's number
/// (several, apart by commas, where more than one may be picked) or label
/// becomes that option's label; anything else is the answer in its own words.
pub fn resolve_option(q: &AskQuestion, answer: &str) -> String {
    let pick = |part: &str| -> Option<String> {
        let part = part.trim();
        if let Ok(n) = part.parse::<usize>() {
            return q.options.get(n.checked_sub(1)?).cloned();
        }
        q.options.iter().find(|o| o.eq_ignore_ascii_case(part)).cloned()
    };
    if let Some(o) = pick(answer) {
        return o;
    }
    if q.multi {
        let parts: Option<Vec<String>> = answer.split(',').map(pick).collect();
        if let Some(p) = parts.filter(|p| !p.is_empty()) {
            return p.join(", ");
        }
    }
    answer.trim().to_string()
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
    /// The window group, 1-based as the title bar counts (I = 1).
    pub group: Option<u32>,
    pub provider: Provider,
    pub account: String,
    pub parent: Option<String>,
    /// See `state_of`.
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ask: Option<Ask>,
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
    /// The settings a helper gets here unless the spawn says otherwise, for
    /// the caller's folder: the flags as `spawn` echoes them.
    pub defaults: serde_json::Value,
    /// The models a helper may ask for here: Claude Code's aliases (a full
    /// id works too), Codex's catalogue with each model's name and levels.
    pub models: Vec<serde_json::Value>,
    /// Its usage as the window last showed it (see `account_usage`); none
    /// before the window has said.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<serde_json::Value>,
}

/// Accounts an agent may spawn on — every account folder on disk, minus the
/// ones the user switched off for agents — with what a helper would get in
/// `folder` on each.
fn account_rows(folder: &str) -> Vec<AccountRow> {
    let usage = reg().lock().unwrap_or_else(|e| e.into_inner()).usage.clone();
    allowed_account_dirs()
        .into_iter()
        .map(|a| {
            let defaults = match a.provider {
                Provider::Claude => crate::claude::defaults::flags(&a.path, folder),
                Provider::Codex => crate::codex::defaults::flags(&a.path, folder),
            };
            let usage = usage.get(&format!("{}/{}", a.provider.as_str(), a.name)).cloned();
            let models = match a.provider {
                Provider::Claude => CLAUDE_MODELS.iter().map(|m| serde_json::json!({ "id": m })).collect(),
                Provider::Codex => crate::codex::models::models_known(&a.path)
                    .into_iter()
                    .map(|m| {
                        serde_json::json!({ "id": m.slug, "name": m.display_name, "efforts": m.efforts, "defaultEffort": m.default_effort })
                    })
                    .collect(),
            };
            AccountRow { provider: a.provider, name: a.name, defaults, models, usage }
        })
        .collect()
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
        Provider::Codex => read("auth.json").is_some_and(|v| {
            v["tokens"].is_object() || v["OPENAI_API_KEY"].as_str().is_some_and(|k| !k.is_empty())
        }),
    }
}

pub fn list(caller: &str) -> Listing {
    let rows: Vec<(String, Known, String, Option<u32>)> = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        descendants(&r, caller)
            .into_iter()
            .filter_map(|id| {
                let name = r.names.get(&id).cloned().unwrap_or_default();
                let group = r.groups.get(&id).map(|g| g + 1);
                r.known.get(&id).cloned().map(|k| (id, k, name, group))
            })
            .collect()
    };
    let sessions = rows
        .into_iter()
        .map(|(id, k, name, group)| {
            let ts = crate::activity::turn_state(&id);
            // A listing is not a reading: it does not move what the caller
            // is taken to have seen of a session's turns.
            let turns_ended = ts.map(|t| t.turns_ended).unwrap_or(0);
            let (state, ask) = state_of(&id);
            SessionRow {
                state,
                ask,
                turns_ended,
                id,
                name,
                group,
                provider: k.provider,
                account: k.account,
                parent: k.parent,
                settings: k.settings,
            }
        })
        .collect();
    let folder = reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(caller).map(|k| k.folder.clone()).unwrap_or_default();
    Listing { me: caller.to_string(), accounts: account_rows(&folder), sessions }
}

/// One row of `sessions`: any chat Luna has run a session for since it
/// started, whoever started it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnySession {
    pub id: String,
    pub name: String,
    /// The window group, 1-based as the title bar counts (I = 1).
    pub group: Option<u32>,
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
    /// See `state_of`.
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ask: Option<Ask>,
    /// Runs with no terminal and no row (background.rs).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub background: bool,
    pub settings: serde_json::Value,
}

/// Which sessions `sessions` returns: a window group, `mine` for the
/// caller's own, and text to look for in the name, folder, account or id.
#[derive(Deserialize, Default)]
pub struct SessionFilter {
    #[serde(default)]
    pub group: Option<serde_json::Value>,
    #[serde(default)]
    pub q: Option<String>,
    /// Past spawns instead (agentlog.rs), deleted helpers included.
    #[serde(default)]
    pub history: Option<bool>,
}

impl SessionFilter {
    /// The 1-based group asked for, `mine` resolved against the caller's.
    fn group(&self, mine: Option<u32>) -> Result<Option<u32>, String> {
        let Some(v) = &self.group else { return Ok(None) };
        if v.as_str().is_some_and(|s| s.trim().eq_ignore_ascii_case("mine")) {
            return mine.map(Some).ok_or_else(|| "your session is in no window group, so \"mine\" matches nothing".to_string());
        }
        v.as_u64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
            .filter(|g| *g >= 1)
            .map(|g| Some(g as u32))
            .ok_or_else(|| format!("group {v} is neither a number from 1 nor \"mine\""))
    }
}

/// Whether a row passes a filter whose group is already resolved.
fn keeps(row: &AnySession, group: Option<u32>, q: Option<&str>) -> bool {
    if group.is_some_and(|g| row.group != Some(g)) {
        return false;
    }
    let Some(q) = q.map(str::trim).filter(|q| !q.is_empty()) else { return true };
    let q = q.to_lowercase();
    [&row.name, &row.folder, &row.account, &row.id].iter().any(|f| f.to_lowercase().contains(&q))
}

/// Every session Luna runs, for a caller that has to find another one — to
/// write to it, or to read its transcript off the disk.
pub fn sessions(caller: &str, filter: &SessionFilter) -> Result<Vec<AnySession>, String> {
    type Row = (String, Known, String, Option<u32>, bool);
    let (rows, mine): (Vec<Row>, Option<u32>) = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let rows = r
            .known
            .iter()
            .filter(|(id, _)| id.as_str() != DEV_CALLER || caller == DEV_CALLER)
            .map(|(id, k)| {
                let name = r.names.get(id).cloned().unwrap_or_default();
                let group = r.groups.get(id).map(|g| g + 1);
                (id.clone(), k.clone(), name, group, is_descendant(&r, caller, id))
            })
            .collect();
        (rows, r.groups.get(caller).map(|g| g + 1))
    };
    let group = filter.group(mine)?;
    let pty = Some(crate::pty::manager());
    let mut out: Vec<AnySession> = rows
        .into_iter()
        .map(|(id, k, name, group, yours)| {
            let probe = pty.as_ref().and_then(|p| p.probe(&id));
            let meta = probe.as_ref().and_then(disk_meta);
            let transcript = pty.as_ref().and_then(|p| transcript_path(p, &id)).map(|p| p.to_string_lossy().into_owned());
            // The row's name, else what the CLI calls the conversation.
            let name = Some(name)
                .filter(|n| !n.trim().is_empty())
                .or_else(|| meta.as_ref().and_then(|m| m.title.clone().or_else(|| m.first_prompt.clone())))
                .unwrap_or_default();
            let background = crate::background::has(&id);
            let (state, ask) = state_of(&id);
            AnySession {
                you: id == caller,
                yours,
                alive: probe.is_some() || background,
                state,
                ask,
                background,
                pid: probe.as_ref().and_then(|p| p.pid),
                cwd: meta
                    .as_ref()
                    .and_then(|m| m.cwd.clone())
                    .or_else(|| probe.as_ref().map(|p| p.cwd.clone()))
                    .or_else(|| crate::background::working_dir(&id)),
                session_id: meta.and_then(|m| m.session_id).or_else(|| crate::background::session_id(&id)),
                transcript,
                id,
                name,
                group,
                provider: k.provider,
                account: k.account,
                account_path: k.account_path,
                folder: k.folder,
                parent: k.parent,
                settings: k.settings,
            }
        })
        .collect();
    out.retain(|row| keeps(row, group, filter.q.as_deref()));
    out.sort_by(|a, b| b.alive.cmp(&a.alive).then_with(|| (&a.account, &a.name).cmp(&(&b.account, &b.name))));
    Ok(out)
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
    /// What the CLI flagged on its screen as it came up — a model the
    /// organization does not allow and the one used instead — which the
    /// settings above, being what it was started with, cannot say.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// Other live sessions working in the same directory, the caller aside:
    /// they share its files and its git index, commits and all.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub same_folder: Vec<Neighbour>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Neighbour {
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
fn neighbours_in(dir: &str, skip: &[&str]) -> Vec<Neighbour> {
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
            out.push(Neighbour {
                busy: crate::activity::turn_state(&id).is_some_and(|t| t.busy),
                name: chat_name(&id).unwrap_or_default(),
                id,
            });
        }
    }
    out
}

/// The screen says the CLI did not know the slash command `text` opens with.
/// Claude Code answers an unknown one on the spot and starts no turn, so
/// there is no start to wait out.
fn unknown_command(pty: &crate::pty::PtyManager, id: &str, text: &str) -> bool {
    let Some(cmd) = text.trim_start().strip_prefix('/').and_then(|rest| rest.split_whitespace().next()) else {
        return false;
    };
    pty.screen(id).is_some_and(|s| s.contains(&format!("Unknown command: /{cmd}")))
}

/// The last lines of a session's terminal, for a caller that has to see
/// what a helper is stuck on.
fn screen_of(pty: &crate::pty::PtyManager, id: &str) -> Option<String> {
    // A helper with no terminal has what it did instead.
    if crate::background::has(id) {
        return crate::background::screen(id, SCREEN_LINES);
    }
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
    !crate::background::has(id) && pty.alive(id) && !has_started(id, 0) && pty.screen(id).is_some_and(|s| s.trim().is_empty())
}

/// Polls until the session starts working on `text` (see `has_started`),
/// turns it down as an unknown command, exits, or `timeout` passes. Returns
/// whether it started.
fn await_start(
    pty: &crate::pty::PtyManager,
    id: &str,
    text: &str,
    after_turn: u32,
    timeout: std::time::Duration,
) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if has_started(id, after_turn) {
            return true;
        }
        if !pty.alive(id) || unknown_command(pty, id, text) || std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(POLL);
    }
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
    crate::worktree::is_worktree_of(&folder, &cwd).then_some(cwd)
}

/// Asks the frontend to make the chat and start its session; blocks until it
/// answers or the timeout passes.
pub fn spawn(caller: &str, mut params: SpawnParams) -> Result<Spawned, String> {
    if params.prompt.trim().is_empty() {
        return Err("prompt is required".into());
    }
    // Off the registry lock: this reads the CLI's registry from disk. A
    // helper works where its caller does, worktree and all.
    let worktree = if params.folder.is_none() { caller_worktree(caller) } else { None };
    let (request_id, slot, in_worktree) = {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let me = r.known.get(caller).cloned().ok_or("caller is not a registered session")?;
        if descendants(&r, caller).len() >= MAX_DESCENDANTS {
            return Err(format!("you already have {MAX_DESCENDANTS} sessions; delete some first"));
        }
        if params.tools.unwrap_or(false) && depth_of(&r, caller) >= MAX_TOOL_DEPTH {
            return Err("a session this deep cannot spawn sessions with tools of their own".into());
        }
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
        let provider = match params.provider.as_deref() {
            None => me.provider,
            Some("claude") => Provider::Claude,
            Some("codex") => Provider::Codex,
            Some(other) => return Err(format!("unknown provider {other}; use claude or codex")),
        };
        params.provider = Some(provider.as_str().to_string());
        let allowed = allowed_account_dirs();
        let mut ours = allowed.iter().filter(|a| a.provider == provider);
        let pick = match params.account.as_deref() {
            Some(name) => ours.find(|a| a.name == name),
            // The caller's own account, else one somebody signed in to, else
            // any: the user said which accounts agents may use, not which
            // of them works.
            None => allowed
                .iter()
                .find(|a| a.provider == provider && provider == me.provider && a.name == me.account)
                .or_else(|| allowed.iter().find(|a| a.provider == provider && signed_in(provider, &a.path)))
                .or_else(|| ours.next()),
        };
        let Some(account) = pick.cloned() else {
            return Err(match params.account.as_deref() {
                Some(name) => format!(
                    "no {} account named {name:?} is available to agents; luna_list shows the ones that are",
                    provider.as_str()
                ),
                None => format!("no {} account is available to agents", provider.as_str()),
            });
        };
        params.account = Some(account.name.clone());
        if !params.visible.unwrap_or(false) {
            drop(r);
            return spawn_background(caller, params, account.path, in_worktree);
        }
        let id = r.next_request;
        r.next_request += 1;
        let slot = std::sync::Arc::new((Mutex::new(Pending::default()), Condvar::new()));
        r.pending.insert(id, slot.clone());
        (id, slot, in_worktree)
    };

    let borrowed = in_worktree.is_some();
    let auto_delete = params.auto_delete.unwrap_or(false);
    let own_worktree = params.worktree.unwrap_or(false);
    let folder = params.folder.clone().unwrap_or_default();
    // Where it will work: a worktree of its own shares nothing.
    let run_dir = (!own_worktree).then(|| in_worktree.clone().unwrap_or_else(|| folder.clone()));
    let (provider, account, prompt) =
        (params.provider.clone().unwrap_or_default(), params.account.clone().unwrap_or_default(), params.prompt.clone());
    // A command line holds some 32 thousand characters on Windows.
    params.prompt = spill(&params.prompt);
    crate::emit::to_ui("agent://spawn", SpawnRequest { request_id, parent_id: caller.to_string(), in_worktree, params });

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
            return Err(format!(
                "the app did not start the session within {}s. It may still come up: luna_list will show it as yours \
                 and Luna will tell you when it answers, so check luna_list before spawning again. Helpers are made by \
                 Luna's window: if it is closed, they start once the user opens it",
                SPAWN_TIMEOUT.as_secs()
            ))
        }
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
    let started = await_start(&pty, &id, &prompt, 0, START_TIMEOUT);
    let starting = !started && still_starting(&pty, &id);
    if starting {
        crate::log::info("agents", &format!("session {id} spawned by {caller} has drawn nothing within {START_TIMEOUT:?}; still starting"));
    } else if !started && unknown_command(&pty, &id, &prompt) {
        crate::log::info("agents", &format!("session {id} spawned by {caller} does not know the command its prompt opens with"));
    } else if !started {
        crate::log::warn("agents", &format!("session {id} spawned by {caller} did not start on its prompt within {START_TIMEOUT:?}"));
    }
    let screen = (!started && !starting).then(|| screen_of(&pty, &id)).flatten();
    // A screen handed over says it all already.
    let warnings = if screen.is_none() { warnings_of(&pty, &id) } else { Vec::new() };
    let same_folder = run_dir.map(|dir| neighbours_in(&dir, &[caller, &id])).unwrap_or_default();
    shown(caller, &id, 0);
    owe(caller, &id, 0);
    Ok(Spawned { id, turns_ended: 0, settings, started, screen, starting, warnings, same_folder })
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
                model,
                effort,
                mode,
                tools,
                token: None,
            })
        }
        Provider::Codex => {
            if asked(&params.permission_mode).is_some() {
                return Err("permissionMode is a Claude setting; Codex takes approval and sandbox".into());
            }
            if asked(&params.approval).is_some_and(|a| a != "never") {
                return Err("a background Codex helper has nobody to ask for approval; leave approval out, \
                            or spawn it with visible: true"
                    .into());
            }
            let d = crate::codex::defaults::flags(account_path, folder);
            let sandbox = match asked(&params.sandbox) {
                Some(s) if ["read-only", "workspace-write", "danger-full-access"].contains(&s) => s.to_string(),
                Some(s) => return Err(format!("unknown sandbox \"{s}\"; use read-only, workspace-write, danger-full-access")),
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
                (None, Some(l)) if asked(&params.model).is_some() && l.default_effort.is_some() => l.default_effort.clone().unwrap_or_default(),
                (None, _) => d["effort"].as_str().unwrap_or("medium").to_string(),
            };
            Ok(crate::background::Spec {
                provider,
                account_path: account_path.to_string(),
                folder: folder.to_string(),
                worktree,
                model,
                effort,
                mode: sandbox,
                tools,
                token: None,
            })
        }
    }
}

/// `spawn` for a helper with no row: started here, in the core, on the
/// settings `background_spec` works out.
fn spawn_background(caller: &str, params: SpawnParams, account_path: String, in_worktree: Option<String>) -> Result<Spawned, String> {
    let provider = if params.provider.as_deref() == Some("codex") { Provider::Codex } else { Provider::Claude };
    let project = params.folder.clone().unwrap_or_default();
    let own_worktree = params.worktree.unwrap_or(false);
    // A helper handed its caller's worktree works in it and never owns it.
    let run_in = in_worktree.clone().unwrap_or_else(|| project.clone());
    let mut spec = background_spec(&params, provider, &account_path, &run_in)?;
    let id = crate::background::mint_id();
    let settings = match provider {
        Provider::Claude => serde_json::json!({
            "model": spec.model, "effort": spec.effort, "permissionMode": spec.mode, "worktree": own_worktree, "background": true,
        }),
        Provider::Codex => serde_json::json!({
            "model": spec.model, "effort": spec.effort, "approval": "never", "sandbox": spec.mode, "background": true,
        }),
    };
    let tools = spec.tools;
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
        // Nobody sees it to delete it: it goes once its answer is out.
        r.auto.insert(id.clone(), Auto::default());
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
        account: params.account.as_deref().unwrap_or(""),
        settings: &settings,
        name: Some(name),
        prompt: &params.prompt,
        auto_delete: true,
        background: true,
    });
    let same_folder = if own_worktree { Vec::new() } else { neighbours_in(&run_in, &[caller, &id]) };
    shown(caller, &id, 0);
    owe(caller, &id, 0);
    Ok(Spawned { id, turns_ended: 0, settings, started: true, screen: None, starting: false, warnings: Vec::new(), same_folder })
}

/// Past this many characters a prompt or message goes to a visible session
/// as a file it is told to read: a command line holds some 32 thousand on
/// Windows, and a Codex composer takes typed text one key event at a time.
const LONG_TEXT: usize = 8000;

/// Where long prompts and messages are put for the sessions they are for.
/// A Claude Code session an agent spawned gets it as an `--add-dir`.
pub fn notes_dir() -> std::path::PathBuf {
    crate::paths::data_dir().join("agents")
}

/// `text` as it is to be typed or passed: itself, or past LONG_TEXT a line
/// that points at a file holding it.
fn spill(text: &str) -> String {
    let n = text.chars().count();
    if n <= LONG_TEXT {
        return text.to_string();
    }
    let dir = notes_dir();
    let path = dir.join(format!("{}.md", crate::background::mint_id()));
    if std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, text)).is_err() {
        return text.to_string();
    }
    format!(
        "This message is {n} characters long, so Luna put it in a file: {}. Read the whole file first and take it as this message.",
        path.display()
    )
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
        drop(r);
        // Too late for the caller's spawn, not for the caller: the session
        // registered as its child, so `luna_list` shows it, and the answer
        // to its prompt is owed like any other.
        if let (Some((caller, _)), Some(id)) = (caller, chat_id) {
            crate::log::warn("agents", &format!("session {id} for {caller} came up after its spawn timed out"));
            shown(&caller, &id, 0);
            owe(&caller, &id, 0);
        }
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sent {
    pub ok: bool,
    /// Where the session stood when the text went in (see `state_of`).
    pub state_before: &'static str,
    /// It was working: the text waits for the turn in flight to end and is
    /// taken up after it, and `started` says nothing yet.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub queued: bool,
    /// A turn began on the text within SEND_TIMEOUT. False on a session that
    /// was not working means the text did not become a prompt: `screen`
    /// shows where it went.
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
    } else if await_start(pty, id, text, baseline, ENTER_RETRY) {
        true
    } else if pty.write(id, b"\r").is_err() {
        // The text went in and the session is gone since — `/exit` does
        // exactly that. It was taken; there is just no turn to wait for.
        true
    } else {
        await_start(pty, id, text, baseline, SEND_TIMEOUT.saturating_sub(ENTER_RETRY))
    })
}

/// Types text into the session, presses Enter after it, and reports whether
/// a turn began.
pub fn send(caller: &str, id: &str, text: &str) -> Result<Sent, String> {
    authorize_send(caller, id)?;
    let pty = crate::pty::manager();
    if !live(&pty, id) {
        return Err(format!("session {id} is not running"));
    }
    let (state_before, ask) = state_of(id);
    if let Some(a) = ask.as_ref().filter(|a| a.blocking) {
        return Err(format!(
            "session {id} is on a {}: typed text would land in its dialog. Answer it with luna_answer",
            a.kind
        ));
    }
    let before = crate::activity::turn_state(id);
    let baseline = before.map(|t| t.turns_ended).unwrap_or(0);
    if crate::background::has(id) {
        let queued = crate::background::send(id, text)?;
        shown(caller, id, baseline);
        owe(caller, id, baseline);
        return Ok(Sent { ok: true, state_before, queued, started: !queued, turns_ended: baseline, screen: None });
    }
    if still_starting(&pty, id) {
        return Err(format!(
            "session {id} has not come up yet (its terminal is still blank), so typed text would be lost; \
             an opening prompt from luna_spawn is held and runs once it is up — luna_wait(turn_done) for the reply"
        ));
    }
    // Text typed now would go out with what a person has in the composer, or
    // land in the middle of what they are typing (see `ready_for_notice`).
    if pty.has_draft(id) {
        return Err(format!(
            "session {id} has something the user typed and has not sent yet; your text would be sent along with it. \
             Try again later, or ask the user"
        ));
    }
    if pty.typed_ago_ms(id).is_some_and(|ms| ms < SEND_TYPING_MS) {
        return Err(format!("the user is typing in session {id}; try again in a little while"));
    }
    let turn_before = before.map(|t| t.turn);
    let queued = turn_before == Some(crate::activity::Turn::Busy);
    let started = type_in(&pty, id, &spill(text), turn_before, baseline)? && !queued;
    if turn_before == Some(crate::activity::Turn::Waiting) {
        reg().lock().unwrap_or_else(|e| e.into_inner()).answered.insert((caller.to_string(), id.to_string()), now_ms());
    }
    let screen = (!started && !queued).then(|| screen_of(&pty, id)).flatten();
    // The count before the text went in: the reply to it ends the next
    // turn, which the caller has not seen even if it is already over.
    shown(caller, id, baseline);
    owe(caller, id, baseline);
    Ok(Sent { ok: true, state_before, queued, started, turns_ended: baseline, screen })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Answered {
    pub ok: bool,
    /// Where it stands now (see `state_of`): a dialog that is still up, or
    /// the next question of several, shows here and in `screen`.
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ask: Option<Ask>,
    /// The `afterTurn` for the `wait` that collects the reply.
    pub turns_ended: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screen: Option<String>,
}

/// One press for a terminal: text as typed, or a key's escape sequence.
#[derive(Debug, PartialEq)]
enum Key {
    Text(String),
    Seq(&'static str),
}

/// `{enter}`, `{esc}`, `{down}` … between literal text, as `luna_answer`'s
/// `keys` take them.
fn parse_keys(spec: &str) -> Result<Vec<Key>, String> {
    let mut out = Vec::new();
    let mut rest = spec;
    while !rest.is_empty() {
        let Some(open) = rest.find('{') else {
            out.push(Key::Text(rest.to_string()));
            break;
        };
        if open > 0 {
            out.push(Key::Text(rest[..open].to_string()));
        }
        let close = rest[open..].find('}').ok_or("a { without its }")? + open;
        let seq = match rest[open + 1..close].trim().to_lowercase().as_str() {
            "enter" => "\r",
            "esc" => "\x1b",
            "tab" => "\t",
            "shift+tab" => "\x1b[Z",
            "up" => "\x1b[A",
            "down" => "\x1b[B",
            "right" => "\x1b[C",
            "left" => "\x1b[D",
            "shift+left" => "\x1b[1;2D",
            "shift+right" => "\x1b[1;2C",
            "space" => " ",
            "backspace" => "\x7f",
            "ctrl+c" => "\x03",
            other => return Err(format!("unknown key {{{other}}}")),
        };
        out.push(Key::Seq(seq));
        rest = &rest[close + 1..];
    }
    Ok(out)
}

/// The presses that answer `ask` in a CLI's terminal: an option is reached
/// with the arrows from the first one and taken with Enter; words of its own
/// go in the free-text row that follows the options; a permission is
/// Enter on its first choice (yes) or Esc (no, with the note typed after as
/// what to do instead).
fn presses_for(provider: Provider, ask: &Ask, answers: &[String], allow: Option<bool>, note: Option<&str>) -> Result<Vec<Key>, String> {
    let mut keys = Vec::new();
    if ask.kind == "permission" {
        match allow {
            Some(true) => keys.push(match provider {
                Provider::Claude => Key::Seq("\r"),
                Provider::Codex => Key::Text("y".into()),
            }),
            Some(false) => {
                keys.push(Key::Seq("\x1b"));
                if let Some(n) = note.map(str::trim).filter(|n| !n.is_empty()) {
                    keys.push(Key::Text(n.to_string()));
                    keys.push(Key::Seq("\r"));
                }
            }
            None => return Err("this is a permission: answer with allow true or false".into()),
        }
        return Ok(keys);
    }
    if answers.len() != ask.questions.len() {
        return Err(format!("{} question(s), {} answer(s): give one answer per question", ask.questions.len(), answers.len()));
    }
    for (q, a) in ask.questions.iter().zip(answers) {
        let picked = resolve_option(q, a);
        match q.options.iter().position(|o| *o == picked) {
            Some(i) => keys.extend(std::iter::repeat_with(|| Key::Seq("\x1b[B")).take(i)),
            None => {
                keys.extend(std::iter::repeat_with(|| Key::Seq("\x1b[B")).take(q.options.len()));
                keys.push(Key::Text(picked));
            }
        }
        keys.push(Key::Seq("\r"));
    }
    // Several questions end on a review of the answers, to submit.
    if ask.questions.len() > 1 && provider == Provider::Claude {
        keys.push(Key::Seq("\r"));
    }
    Ok(keys)
}

/// Writes presses one at a time, far enough apart that the CLI takes each
/// as a key and not a burst of paste (see ENTER_GAP).
fn press(pty: &crate::pty::PtyManager, id: &str, keys: &[Key]) -> Result<(), String> {
    for k in keys {
        match k {
            Key::Text(t) => {
                pty.write(id, t.as_bytes())?;
                std::thread::sleep(ENTER_GAP);
            }
            Key::Seq(s) => {
                pty.write(id, s.as_bytes())?;
                std::thread::sleep(KEY_GAP);
            }
        }
    }
    Ok(())
}

/// Between two keys pressed for an answer.
const KEY_GAP: std::time::Duration = std::time::Duration::from_millis(150);
/// How long after an answer its effect is looked at.
const ANSWER_SETTLE: std::time::Duration = std::time::Duration::from_millis(1500);

/// `luna_answer`: answers what the session waits on — a question or a
/// permission (`ask_of`), or with `keys`, any dialog on its screen.
pub fn answer(
    caller: &str,
    id: &str,
    answers: &[String],
    allow: Option<bool>,
    note: Option<&str>,
    keys: Option<&str>,
) -> Result<Answered, String> {
    let known = authorize_send(caller, id)?;
    let pty = crate::pty::manager();
    if !live(&pty, id) {
        return Err(format!("session {id} is not running"));
    }
    let baseline = crate::activity::turn_state(id).map(|t| t.turns_ended).unwrap_or(0);
    let ask = ask_of(id);
    if crate::background::has(id) {
        if keys.is_some() {
            return Err("a background helper has no terminal: answer with answers or allow".into());
        }
        crate::background::answer(id, answers, allow, note)?;
    } else {
        if pty.has_draft(id) || pty.typed_ago_ms(id).is_some_and(|ms| ms < SEND_TYPING_MS) {
            return Err(format!("the user is typing in session {id}; try again in a little while"));
        }
        match (keys, &ask) {
            (Some(k), _) => press(&pty, id, &parse_keys(k)?)?,
            // An async question holds no dialog: the answer is a message.
            (None, Some(a)) if !a.blocking => {
                let text: Vec<String> =
                    a.questions.iter().zip(answers).map(|(q, ans)| format!("\"{}\": {}", q.text, resolve_option(q, ans))).collect();
                if text.is_empty() {
                    return Err("give one answer per question".into());
                }
                let before = crate::activity::turn_state(id);
                type_in(&pty, id, &format!("Answers to your question: {}", text.join("; ")), before.map(|t| t.turn), baseline)?;
            }
            (None, Some(a)) => press(&pty, id, &presses_for(known.provider, a, answers, allow, note)?)?,
            (None, None) => {
                return Err(format!(
                    "session {id} is not on a question or permission Luna can read; luna_read with screen: true shows its \
                     terminal, and keys answers any dialog"
                ))
            }
        }
        reg().lock().unwrap_or_else(|e| e.into_inner()).answered.insert((caller.to_string(), id.to_string()), now_ms());
    }
    std::thread::sleep(ANSWER_SETTLE);
    shown(caller, id, baseline);
    owe(caller, id, baseline);
    let (state, ask) = state_of(id);
    let screen = (!crate::background::has(id)).then(|| screen_of(&pty, id)).flatten();
    Ok(Answered { ok: true, state, ask, turns_ended: baseline, screen })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reading {
    pub messages: Vec<Message>,
    /// Pass back as `since` to read only what came after.
    pub cursor: u64,
    /// See `state_of`.
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ask: Option<Ask>,
    pub turns_ended: u32,
    /// The terminal's last lines, when asked for (`screen: true`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screen: Option<String>,
}

/// The last transcript found for each session: Claude Code drops the
/// registry entry it is found through when it exits, and an exited helper's
/// last reply is still asked for.
fn transcripts() -> &'static Mutex<HashMap<String, std::path::PathBuf>> {
    static T: OnceLock<Mutex<HashMap<String, std::path::PathBuf>>> = OnceLock::new();
    T.get_or_init(Default::default)
}

fn transcript_path(pty: &crate::pty::PtyManager, id: &str) -> Option<std::path::PathBuf> {
    let found = find_transcript(pty, id);
    let mut known = transcripts().lock().unwrap_or_else(|e| e.into_inner());
    match found {
        Some(path) => {
            known.insert(id.to_string(), path.clone());
            Some(path)
        }
        None => known.get(id).cloned(),
    }
}

fn find_transcript(pty: &crate::pty::PtyManager, id: &str) -> Option<std::path::PathBuf> {
    if crate::background::has(id) {
        return crate::background::transcript(id);
    }
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
    let pty = crate::pty::manager();
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
    let (state, ask) = state_of(id);
    shown(caller, id, turns_ended);
    Ok(Reading {
        messages,
        cursor,
        state,
        ask,
        turns_ended,
        screen: with_screen.then(|| screen_of(&pty, id)).flatten(),
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Waited {
    pub outcome: &'static str,
    /// See `state_of`.
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ask: Option<Ask>,
    pub turns_ended: u32,
    /// On `turn_done`: what the session said last, so no `read` is needed.
    pub reply: Option<String>,
    /// On `timeout`, `waiting` and a turn that ended without a reply: the
    /// terminal's last lines, so the caller sees what the session is on — a
    /// permission prompt, a question, an error, a notice nobody dismissed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screen: Option<String>,
}

/// Blocks until the session reaches `until`, or `timeout_s` passes:
/// `turn_done` — a turn ended after `after_turn` (default: the count the
/// caller was last shown, so the reply to a prompt it just sent counts even
/// if it landed before this call, and a turn it has already read does not —
/// or the turn stopped on a prompt, which nothing but an answer moves on);
/// `waiting` — it stopped for a permission; `exit` — the process ended.
pub fn wait(caller: &str, id: &str, until: &str, timeout_s: u64, after_turn: Option<u32>) -> Result<Waited, String> {
    if !matches!(until, "turn_done" | "waiting" | "exit") {
        return Err(format!("unknown until {until:?}; use turn_done, waiting or exit"));
    }
    authorize_view(caller, id)?;
    let _out = WaitOut::start(caller, id);
    let pty = crate::pty::manager();
    let now = crate::activity::turn_state(id).map(|t| t.turns_ended);
    let baseline = match (after_turn, last_shown(caller, id), now) {
        (Some(n), _, _) => n,
        // A count the session is behind has been left over from before it
        // was respawned; waiting past it would sit out the whole timeout.
        (None, Some(seen), Some(now)) => seen.min(now),
        (None, Some(seen), None) => seen,
        (None, None, now) => now.unwrap_or(0),
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_s.clamp(1, 1800));
    let answered_at = reg().lock().unwrap_or_else(|e| e.into_inner()).answered.get(&(caller.to_string(), id.to_string())).copied();
    let mut looks = 0u32;
    loop {
        let alive = live(&pty, id);
        let ts = crate::activity::turn_state(id);
        let turns_ended = ts.map(|t| t.turns_ended).unwrap_or(0);
        let turn = ts.map(|t| t.turn);
        let ended = turns_ended > baseline && turn != Some(crate::activity::Turn::Busy);
        // A question in the transcript holds the turn whatever the turn
        // word says; read for it every few looks, not every one.
        looks += 1;
        let question = looks % 4 == 1 && ask_of(id).is_some_and(|a| a.blocking && a.kind == "question");
        // A turn stopped on a prompt will not end by itself: `turn_done`
        // hands it back too, unless it is the prompt the caller has just
        // answered and the sampler has not caught up with yet.
        let asking = (turn == Some(crate::activity::Turn::Waiting) || question)
            && answered_at.map_or(true, |at| now_ms().saturating_sub(at) >= ANSWERED_MS);
        let done = match until {
            "exit" => !alive,
            "waiting" => turn == Some(crate::activity::Turn::Waiting) || question || !alive,
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
            let (state, ask) = state_of(id);
            return Ok(Waited { outcome, state, ask, turns_ended, reply, screen });
        }
        if std::time::Instant::now() >= deadline {
            let screen = screen_of(&pty, id);
            shown(caller, id, turns_ended);
            let (state, ask) = state_of(id);
            return Ok(Waited { outcome: "timeout", state, ask, turns_ended, reply: None, screen });
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

/// Stops the session; the chat row stays and can be resumed by hand.
pub fn kill(caller: &str, id: &str) -> Result<(), String> {
    authorize(caller, id)?;
    if crate::background::has(id) {
        crate::background::kill(id);
        return Ok(());
    }
    let pty = crate::pty::manager();
    tauri::async_runtime::block_on(pty.kill(id));
    Ok(())
}

/// Stops the session, removes its attachments and — when asked — the
/// worktree it made (never one it was handed), then tells the frontend to
/// drop the row. Returns the worktree removed, if any. A session's own
/// children are left where they are, orphaned.
pub fn delete(caller: &str, id: &str, drop_worktree: bool) -> Result<Option<String>, String> {
    let known = authorize(caller, id)?;
    remove(id, known, drop_worktree)
}

fn remove(id: &str, known: Known, drop_worktree: bool) -> Result<Option<String>, String> {
    let pty = crate::pty::manager();
    // A helper resumed in a worktree knows the worktree as its folder.
    let folder = crate::worktree::project_of(&known.folder).unwrap_or(known.folder);
    let drop_worktree = drop_worktree && !reg().lock().unwrap_or_else(|e| e.into_inner()).borrowed.contains(id);
    if crate::background::has(id) {
        let made = crate::background::remove(id);
        forget(id);
        crate::activity::forget(id);
        crate::log::info("delete", &format!("background helper {id}, worktree {made:?}, dropping {drop_worktree}"));
        return match made {
            Some(wt) if drop_worktree => crate::worktree::remove_worktree_now(folder, wt.clone()).map(|_| Some(wt)),
            _ => Ok(None),
        };
    }
    // PtyManager::delete forgets the session here too (see `forget`).
    let worktree = tauri::async_runtime::block_on(pty.delete(id, &folder, None, drop_worktree))?;
    crate::emit::to_ui("agent://deleted", Deleted { id: id.to_string() });
    // Without the drop, `delete` names the worktree that stays.
    Ok(worktree.filter(|_| drop_worktree))
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

/// Deletes the `autoDelete` helpers that are done with: nobody is owed their
/// answer or waiting on them, and they have sat idle — or exited — for
/// AUTO_DELETE_IDLE_MS since. One a person has typed into is theirs now and
/// stays. Its worktree stays too, with whatever work is in it.
fn reap(pty: &crate::pty::PtyManager) {
    let ids: Vec<String> = reg().lock().unwrap_or_else(|e| e.into_inner()).auto.keys().cloned().collect();
    for id in ids {
        if pty.typed_ago_ms(&id).is_some() {
            reg().lock().unwrap_or_else(|e| e.into_inner()).auto.remove(&id);
            continue;
        }
        // Between turns with no job left running; what its terminal last drew
        // and when does not matter here.
        let idle = !live(pty, &id)
            || crate::activity::turn_state(&id)
                .is_some_and(|t| t.turn == crate::activity::Turn::Idle && !t.children_busy);
        let known = {
            let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
            let quiet = !r.owed.keys().any(|(_, s)| *s == id) && !r.waits.keys().any(|(_, s)| *s == id);
            let known = r.known.get(&id).cloned();
            let Some(a) = r.auto.get_mut(&id) else { continue };
            if !(quiet && idle) {
                a.idle_since = None;
                continue;
            }
            let since = *a.idle_since.get_or_insert(now_ms());
            if now_ms().saturating_sub(since) < AUTO_DELETE_IDLE_MS {
                continue;
            }
            a.deleting = true;
            known
        };
        match known.map(|k| remove(&id, k, false)) {
            Some(Ok(_)) => crate::log::info("agents", &format!("auto-deleted helper {id}")),
            Some(Err(e)) => {
                crate::log::warn("agents", &format!("could not auto-delete helper {id}: {e}"));
                reg().lock().unwrap_or_else(|e| e.into_inner()).auto.remove(&id);
            }
            None => {
                reg().lock().unwrap_or_else(|e| e.into_inner()).auto.remove(&id);
            }
        }
    }
}

/// A chat is gone, however it was deleted: nothing here may keep it — not in
/// `luna_sessions`, not against a caller's helper limit, not as a live token,
/// and not as an answer owed to or by it.
pub fn forget(id: &str) {
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let spawned = r.known.get(id).is_some_and(|k| k.parent.is_some());
    let auto = r.auto.remove(id);
    r.known.remove(id);
    r.tokens.retain(|_, chat| chat != id);
    r.seen.retain(|(_, chat), _| chat != id);
    r.owed.retain(|(who, chat), _| who != id && chat != id);
    r.answered.retain(|(who, chat), _| who != id && chat != id);
    r.abandoned.retain(|_, (caller, _)| caller != id);
    r.borrowed.remove(id);
    r.waits.retain(|(who, chat), _| who != id && chat != id);
    r.notified.remove(id);
    r.hook_asks.remove(id);
    drop(r);
    transcripts().lock().unwrap_or_else(|e| e.into_inner()).remove(id);
    if spawned {
        crate::agentlog::ended(id, if auto.is_some_and(|a| a.deleting) { "auto-deleted" } else { "deleted" });
    }
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
    /// It asks a question it does not wait on; told once per question.
    Asked(u64),
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
/// the length capped, saying so where it is.
fn quote(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= NOTICE_QUOTE {
        return flat;
    }
    let cut: String = flat.chars().take(NOTICE_QUOTE).collect();
    format!("{cut}… [cut; luna_read gives the full text]")
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
    if !live(pty, id) {
        return Some((Some(format!("{who} has exited.")), Settled::Exited));
    }
    let ask = ask_of(id);
    let held = turn == Some(crate::activity::Turn::Waiting) || ask.as_ref().is_some_and(|a| a.blocking && a.kind == "question");
    if held && !o.told_waiting {
        let line = match &ask {
            Some(a) => format!("{who} {}. Answer with luna_answer.", a.line()),
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
            return Some((
                Some(format!("{who} {} It works on meanwhile; luna_answer answers it.", a.line())),
                Settled::Asked(key),
            ));
        }
    }
    None
}

/// Whether a notice may go into `caller` now: its CLI is up and between turns,
/// nobody is typing into it, and nothing typed waits unsent in its composer
/// (a notice and its Enter would send the draft along with it).
fn ready_for_notice(pty: &crate::pty::PtyManager, caller: &str) -> bool {
    // A background caller takes it as its next turn, between turns.
    if crate::background::has(caller) {
        return crate::activity::turn_state(caller).is_some_and(|t| t.turn == crate::activity::Turn::Idle);
    }
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
        (ended > o.baseline && ts.map(|t| t.turn) != Some(crate::activity::Turn::Busy)) || !live(pty, id)
    })
}

/// Whether a `wait` the caller has out on `id` is to hand this answer over
/// rather than a notice: while nothing has come in, and for WAIT_HOLD_MS
/// after something did — the wait answers within a second or two, so one
/// still out by then belongs to a client that has let the call go.
fn held_for_wait(pty: &crate::pty::PtyManager, caller: &str, id: &str, o: &Owed) -> bool {
    let key = (caller.to_string(), id.to_string());
    if !reg().lock().unwrap_or_else(|e| e.into_inner()).waits.contains_key(&key) {
        return false;
    }
    let ts = crate::activity::turn_state(id);
    let turn = ts.map(|t| t.turn);
    // A prompt it stopped on is news too: `wait(turn_done)` hands that back.
    let news = (ts.is_some_and(|t| t.turns_ended > o.baseline) && turn != Some(crate::activity::Turn::Busy))
        || turn == Some(crate::activity::Turn::Waiting)
        || !live(pty, id);
    if !news {
        return true;
    }
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let Some(owed) = r.owed.get_mut(&key) else { return true };
    let at = *owed.ready_at.get_or_insert(now_ms());
    now_ms().saturating_sub(at) < WAIT_HOLD_MS
}

/// Runs for the life of the app: every few seconds, for each caller owed an
/// answer, checks whether one came and tells the caller once it is idle; and
/// deletes the `autoDelete` helpers that are done (see `reap`).
fn notifier() {
    let mut idle_looks: HashMap<String, u32> = HashMap::new();
    // Callers whose notice a draft is holding back: since when, and whether
    // the desktop has been told.
    let mut drafts: HashMap<String, (u64, bool)> = HashMap::new();
    loop {
        std::thread::sleep(NOTIFY_EVERY);
        let pty = crate::pty::manager();
        reap(&pty);
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
            *looks = if ready_for_notice(&pty, &caller) { *looks + 1 } else { 0 };
            if *looks < NOTIFY_IDLE_LOOKS {
                continue;
            }
            let mut lines = Vec::new();
            let mut settled = Vec::new();
            for (id, k, o, who) in &items {
                if held_for_wait(&pty, &caller, id, o) {
                    continue;
                }
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
            let text = format!("{NOTICE_TAG} {}", lines.join(" "));
            let before = crate::activity::turn_state(&caller);
            let baseline = before.map(|t| t.turns_ended).unwrap_or(0);
            let typed = if crate::background::has(&caller) {
                crate::background::send(&caller, &text).map(|_| true)
            } else {
                type_in(&pty, &caller, &text, before.map(|t| t.turn), baseline)
            };
            match typed {
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
                                r.owed.insert(key, Owed::from(ended));
                            }
                            Settled::Asked(ask) => {
                                if let Some(o) = r.owed.get_mut(&key) {
                                    o.told_ask = ask;
                                }
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

/// Every chat's name and window group, sent whenever one changes: what
/// `luna_sessions` calls the user's own chats and where it puts them, which
/// the core otherwise knows only by id.
#[tauri::command]
pub fn chat_names(names: HashMap<String, String>, groups: Option<HashMap<String, u32>>) {
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    r.names = names;
    if let Some(groups) = groups {
        r.groups = groups;
    }
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

    #[test]
    fn sessions_filter_by_group_and_text() {
        let none = serde_json::json!({});
        register("grp-me", Provider::Claude, "C:\\work\\luna", "C:\\acc\\anthropic\\one", None, true, none.clone());
        register("grp-peer", Provider::Codex, "C:\\work\\asuri", "C:\\acc\\openai\\two", None, false, none.clone());
        register("grp-far", Provider::Claude, "C:\\work\\luna", "C:\\acc\\anthropic\\one", None, false, none.clone());
        register("grp-loose", Provider::Claude, "C:\\work\\luna", "C:\\acc\\anthropic\\one", None, false, none);
        let names = [("grp-me", "builder"), ("grp-peer", "Tester"), ("grp-far", "docs")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let groups = [("grp-me", 1), ("grp-peer", 1), ("grp-far", 0)].into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        chat_names(names, Some(groups));

        let ids = |caller: &str, f: SessionFilter| -> Vec<String> {
            let mut v: Vec<String> =
                sessions(caller, &f).unwrap().into_iter().map(|s| s.id).filter(|id| id.starts_with("grp-")).collect();
            v.sort();
            v
        };
        let by = |g: serde_json::Value| SessionFilter { group: Some(g), q: None, history: None };
        // Groups go out 1-based, as the title bar counts them.
        assert_eq!(ids("grp-me", by(serde_json::json!("mine"))), ["grp-me", "grp-peer"]);
        assert_eq!(ids("grp-me", by(serde_json::json!(1))), ["grp-far"]);
        assert_eq!(ids("grp-me", by(serde_json::json!("2"))), ["grp-me", "grp-peer"]);
        assert!(sessions("grp-loose", &by(serde_json::json!("mine"))).is_err(), "a session in no group has no mine");
        assert!(sessions("grp-me", &by(serde_json::json!(0))).is_err());
        let q = |text: &str| SessionFilter { group: None, q: Some(text.into()), history: None };
        assert_eq!(ids("grp-me", q("tester")), ["grp-peer"], "name, any case");
        assert_eq!(ids("grp-me", q("asuri")), ["grp-peer"], "folder");
        assert_eq!(ids("grp-me", SessionFilter { group: Some(serde_json::json!("mine")), q: Some("build".into()), history: None }), ["grp-me"]);
    }

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
        assert_eq!(quote(&"x".repeat(NOTICE_QUOTE)), "x".repeat(NOTICE_QUOTE));
        assert!(quote(&"x".repeat(NOTICE_QUOTE + 1)).ends_with("… [cut; luna_read gives the full text]"));
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
        owe("w-parent", "w-child", 0);
        let pty = crate::pty::manager();
        let key = ("w-parent".to_string(), "w-child".to_string());
        let o = || reg().lock().unwrap().owed.get(&key).copied().unwrap();
        assert!(!held_for_wait(&pty, "w-parent", "w-child", &o()), "no wait out, nothing held");
        {
            let _out = WaitOut::start("w-parent", "w-child");
            // Not running, so there is news: the wait has WAIT_HOLD_MS to say it.
            assert!(held_for_wait(&pty, "w-parent", "w-child", &o()));
            reg().lock().unwrap().owed.get_mut(&key).unwrap().ready_at = Some(now_ms() - WAIT_HOLD_MS - 1);
            assert!(!held_for_wait(&pty, "w-parent", "w-child", &o()), "a wait that sat on news is let go");
        }
        assert!(!reg().lock().unwrap().waits.contains_key(&key), "the count goes with the call");
    }

    #[test]
    fn an_auto_delete_helper_counts_idle_only_once_its_answer_is_out() {
        let none = serde_json::json!({});
        register("ad-parent", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone());
        register("ad-child", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("ad-parent"), false, none);
        reg().lock().unwrap().auto.insert("ad-child".into(), Auto::default());
        let since = || reg().lock().unwrap().auto.get("ad-child").and_then(|a| a.idle_since);
        let pty = crate::pty::manager();
        owe("ad-parent", "ad-child", 0);
        reap(&pty);
        assert_eq!(since(), None, "its answer is still owed");
        reg().lock().unwrap().owed.remove(&("ad-parent".to_string(), "ad-child".to_string()));
        reap(&pty);
        assert!(since().is_some(), "exited and nobody owed: the clock runs");
        owe("ad-parent", "ad-child", 1);
        reap(&pty);
        assert_eq!(since(), None, "a follow-up starts it over");
        reg().lock().unwrap().auto.remove("ad-child");
    }

    #[test]
    fn keys_read_as_presses() {
        assert_eq!(
            parse_keys("2{enter}").unwrap(),
            [Key::Text("2".into()), Key::Seq("\r")]
        );
        assert_eq!(parse_keys("{Down}{down}{ENTER}").unwrap(), [Key::Seq("\x1b[B"), Key::Seq("\x1b[B"), Key::Seq("\r")]);
        assert!(parse_keys("{f13}").is_err());
        assert!(parse_keys("{enter").is_err());
    }

    #[test]
    fn an_answer_picks_by_number_label_or_words() {
        let q = AskQuestion { text: "Which?".into(), options: vec!["Red".into(), "Blue".into()], multi: false };
        assert_eq!(resolve_option(&q, "2"), "Blue");
        assert_eq!(resolve_option(&q, "red"), "Red");
        assert_eq!(resolve_option(&q, "9"), "9", "out of range is words");
        assert_eq!(resolve_option(&q, " green "), "green");
        let multi = AskQuestion { multi: true, ..q.clone() };
        assert_eq!(resolve_option(&multi, "1, 2"), "Red, Blue");

        let ask = Ask::question(vec![q], true);
        let keys = presses_for(Provider::Claude, &ask, &["2".into()], None, None).unwrap();
        assert_eq!(keys, [Key::Seq("\x1b[B"), Key::Seq("\r")]);
        let own = presses_for(Provider::Claude, &ask, &["green".into()], None, None).unwrap();
        assert_eq!(own, [Key::Seq("\x1b[B"), Key::Seq("\x1b[B"), Key::Text("green".into()), Key::Seq("\r")]);
        assert!(presses_for(Provider::Claude, &ask, &[], None, None).is_err());
        let perm = Ask::permission(Some("Bash".into()), Some("ls".into()));
        assert_eq!(presses_for(Provider::Codex, &perm, &[], Some(true), None).unwrap(), [Key::Text("y".into())]);
        assert_eq!(
            presses_for(Provider::Claude, &perm, &[], Some(false), Some("use dir")).unwrap(),
            [Key::Seq("\x1b"), Key::Text("use dir".into()), Key::Seq("\r")]
        );
        assert_eq!(perm.line(), "asks for permission to run Bash: ls");
        let codex = "  Would you like to run the following command?\n\n  $ Set-Content -LiteralPath made.txt -Value hi\n\n› 1. Yes, proceed (y)";
        assert_eq!(codex_command_on(codex).as_deref(), Some("Set-Content -LiteralPath made.txt -Value hi"));
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
