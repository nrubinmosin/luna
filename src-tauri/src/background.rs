//! Helpers with no terminal and no row: what `luna_spawn` starts unless it is
//! asked for a visible chat. Each turn is one run of the CLI in its headless
//! mode — `claude -p --output-format stream-json`, `codex exec --json` — with
//! the prompt on stdin, the events on stdout, and the next turn a resume of
//! the same conversation. Whether it works is whether that process runs;
//! what it said is the turn's last message. No screen to read, no keys to
//! press, and no window needed to start one.
//!
//! A Claude Code helper is never left to ask the air: its permission prompts
//! and `AskUserQuestion` come to Luna through `--permission-prompt-tool`
//! (`luna_permission`), which holds the CLI's call until the agent that
//! spawned it answers with `luna_answer`. Codex's headless mode has no one to
//! ask: a question it has comes in its reply, and the answer is the next turn.
//!
//! The conversation itself lands on disk as with any session — Claude Code's
//! transcript, Codex's rollout — so `luna_read` reads it there.

use crate::agents::Ask;
use crate::provider::Provider;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::sync::{Condvar, Mutex, OnceLock};

/// Lines of what a helper did kept for `luna_read(screen)`: its messages,
/// commands and errors, one line each.
const LOG_LINES: usize = 200;
/// How much of a message one log line keeps.
const LOG_CHARS: usize = 300;
/// How long a held permission or question waits for `luna_answer` before the
/// CLI is told nobody answered. Under the MCP call timeout the CLIs are given
/// (pty.rs `MCP_TOOL_TIMEOUT_MS`).
const ASK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// What a helper is started with, every turn the same.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Spec {
    pub provider: Provider,
    pub account_path: String,
    /// The project folder, or the worktree it was handed to run in.
    pub folder: String,
    /// A worktree of its own, made by the CLI on the first turn.
    pub worktree: bool,
    pub model: Option<String>,
    pub effort: String,
    /// Claude: the permission mode. Codex: the sandbox.
    pub mode: String,
    /// Codex: `--approve-for-me`, a reviewer model answering what it would ask.
    pub auto_review: bool,
    pub tools: bool,
    /// Codex: run the project's hooks without their trust, which `exec` has
    /// no way to ask for (codex/hooks.rs).
    pub hooks_unasked: bool,
    /// The bearer token of its MCP config: Luna's tools when `tools`, and
    /// `luna_permission` for every Claude Code helper. Never saved: a core
    /// started later issues its own.
    #[serde(skip)]
    pub token: Option<String>,
}

/// Tokens and cost of a stretch of a helper's work, as `luna_list` gives them.
/// `input` is what no cache served, on both CLIs: Claude Code counts it so,
/// Codex's own `input_tokens` has the cached ones in it.
#[derive(Clone, Copy, Default, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    /// Claude Code's estimate; Codex states no cost.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

/// Floats summed turn after turn, without their noise.
fn usd(x: f64) -> f64 {
    (x * 1e8).round() / 1e8
}

impl Usage {
    fn plus(self, o: Usage) -> Usage {
        Usage {
            input: self.input.saturating_add(o.input),
            output: self.output.saturating_add(o.output),
            cache_read: self.cache_read.saturating_add(o.cache_read),
            cache_write: self.cache_write.saturating_add(o.cache_write),
            cost_usd: match (self.cost_usd, o.cost_usd) {
                (None, None) => None,
                (a, b) => Some(usd(a.unwrap_or(0.0) + b.unwrap_or(0.0))),
            },
        }
    }

    /// What `self` has on top of `o`, field by field.
    fn since(self, o: Usage) -> Usage {
        Usage {
            input: self.input.saturating_sub(o.input),
            output: self.output.saturating_sub(o.output),
            cache_read: self.cache_read.saturating_sub(o.cache_read),
            cache_write: self.cache_write.saturating_sub(o.cache_write),
            cost_usd: None,
        }
    }

    /// The larger of the two, field by field.
    fn at_least(self, o: Usage) -> Usage {
        Usage {
            input: self.input.max(o.input),
            output: self.output.max(o.output),
            cache_read: self.cache_read.max(o.cache_read),
            cache_write: self.cache_write.max(o.cache_write),
            cost_usd: match (self.cost_usd, o.cost_usd) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (a, b) => a.or(b),
            },
        }
    }

    /// Claude Code's usage object, on a message or a `result`.
    fn of_claude(u: &Value) -> Usage {
        let n = |k: &str| u[k].as_u64().unwrap_or(0);
        Usage {
            input: n("input_tokens"),
            output: n("output_tokens"),
            cache_read: n("cache_read_input_tokens"),
            cache_write: n("cache_creation_input_tokens"),
            cost_usd: None,
        }
    }

    /// Claude Code's per-model figures (`modelUsage` and its kin), all models
    /// together. None when the event does not have the field.
    fn of_claude_models(models: &Value) -> Option<Usage> {
        let n = |m: &Value, k: &str| m[k].as_u64().unwrap_or(0);
        Some(models.as_object()?.values().fold(Usage::default(), |sum, m| {
            sum.plus(Usage {
                input: n(m, "inputTokens"),
                output: n(m, "outputTokens"),
                cache_read: n(m, "cacheReadInputTokens"),
                cache_write: n(m, "cacheCreationInputTokens"),
                cost_usd: m["costUSD"].as_f64(),
            })
        }))
    }

    /// Codex's usage object, which `exec --json` and the rollout share.
    pub fn of_codex(u: &Value) -> Usage {
        let n = |k: &str| u[k].as_u64().unwrap_or(0);
        Usage {
            input: n("input_tokens").saturating_sub(n("cached_input_tokens")),
            output: n("output_tokens"),
            cache_read: n("cached_input_tokens"),
            cache_write: n("cache_write_input_tokens"),
            cost_usd: None,
        }
    }
}

/// What a session has spent, for `luna_list`.
#[derive(Clone, Copy, Default, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Spend {
    /// The last turn that stated any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_turn: Option<Usage>,
    pub session: Usage,
}

/// What a session's own events and files show beyond its state, as `luna_list`
/// gives it: each row carries one of these whatever runs the session.
#[derive(Serialize, Default, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Signs {
    /// When the turn on now began; null between turns.
    pub turn_started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<String>,
    /// The full model id the CLI used, whatever alias it was given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spend: Option<Spend>,
}

/// An instant as the rest of `luna_list` writes one.
pub fn iso(ms: u64) -> Option<String> {
    let t = chrono::DateTime::<chrono::Utc>::from_timestamp((ms / 1000) as i64, 0)?;
    Some(t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

/// What a helper's turns have shown of it. All but the last event's moment
/// is saved with it: spend must outlive a core, and a moment rewritten with
/// every event would rewrite the file every SAVE_EVERY of every turn.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Observed {
    spend: Spend,
    model: Option<String>,
    /// When its last turn ended: the last it did, for a core started since.
    turn_ended_ms: u64,
    /// Claude Code: what the turn on now has shown on its messages. A `result`
    /// takes its place, and a turn killed first has none: the CLI states it
    /// nowhere, even in the running totals of the next one.
    unreported: Usage,
    /// When it last printed anything.
    #[serde(skip)]
    event_ms: u64,
    /// Codex has said this turn runs on another model than it asked for.
    #[serde(skip)]
    rerouted: bool,
    /// `unreported` by message: the CLI writes a message once per block, with
    /// the same usage on each.
    #[serde(skip)]
    by_message: HashMap<String, Usage>,
    /// Codex: where the thread's running total stood as this turn started, if
    /// that could be told. It counts what a turn killed before it had spent.
    #[serde(skip)]
    turn_base: Option<Usage>,
    /// Codex: `turn.completed` has stated this turn.
    #[serde(skip)]
    turn_stated: bool,
    /// Which turn this is, for what is read of the last one once it is over.
    #[serde(skip)]
    generation: u64,
}

impl Observed {
    /// A turn's process is up.
    fn begin_turn(&mut self, base: Option<Usage>) {
        self.generation += 1;
        self.rerouted = false;
        self.turn_stated = false;
        self.by_message.clear();
        self.turn_base = base;
        // What a turn killed before it spent, now that it is known.
        if let Some(base) = base {
            self.spend.session = self.spend.session.at_least(base);
        }
    }

    /// A Claude Code turn has gone without a `result`: what its messages showed
    /// is all there will be of it, tokens with no cost.
    fn settle_unreported(&mut self) {
        self.by_message.clear();
        let seen = std::mem::take(&mut self.unreported);
        if seen != Usage::default() {
            self.spend = Spend { last_turn: Some(seen), session: self.spend.session.plus(seen) };
        }
    }

    /// What the rollout said of a Codex turn once it was over, unless another
    /// has begun since: its model, and for a turn gone with no `turn.completed`
    /// the running total, which holds what it spent up to then.
    fn apply_rollout(&mut self, generation: u64, model: Option<String>, total: Option<Usage>) {
        if self.generation != generation {
            return;
        }
        if model.is_some() {
            self.model = model;
        }
        let Some(total) = total else { return };
        self.spend.session = self.spend.session.at_least(total);
        match self.turn_base {
            Some(base) if total.since(base) != Usage::default() => self.spend.last_turn = Some(total.since(base)),
            Some(_) => {}
            None => self.spend.last_turn = None,
        }
    }
}

struct Run {
    pid: Option<u32>,
    /// When the turn started, for the window's "working 4 min".
    started_ms: u64,
    /// A stop came while the turn was claimed but had no process yet: it
    /// has no pid to kill, so `spawn_turn` does not start it, or kills what
    /// it just started.
    stopped: bool,
}

/// A permission or question held for an answer.
struct Held {
    serial: u64,
    ask: Ask,
    /// The tool input as the CLI sent it, returned with the answer.
    input: Value,
    answer: Option<Value>,
}

struct Entry {
    spec: Spec,
    /// When it was spawned.
    started_ms: u64,
    /// Claude Code's session id / Codex's thread id, once the first turn has
    /// said it.
    session: Option<String>,
    /// Where the CLI says it works: the worktree it made, if it made one.
    cwd: Option<String>,
    run: Option<Run>,
    turns_ended: u32,
    /// Text sent while a turn ran, for the turn after it.
    queue: Vec<String>,
    log: VecDeque<String>,
    /// What it is held on, oldest first: parallel subagents and tool calls ask
    /// at once, and the first still unanswered is the one shown.
    held: VecDeque<Held>,
    /// The CLI could not run or quit on an error before saying anything.
    error: Option<String>,
    /// Its last turn was stopped by the core going, not ended by the CLI:
    /// what the caller is told of it says so.
    cut_short: bool,
    /// The job every turn's process is put in, the first one's made for it:
    /// what a turn leaves running is still the helper's between turns, holds
    /// the machine as a chat's would, and is ended with the helper.
    job: Option<std::sync::Arc<crate::procs::Job>>,
    /// Its turns left something running in that job, when last looked: at the
    /// end of its turn and at every activity sample since. Reading a job takes
    /// a process snapshot, too much for every `turn_state`, and not under the
    /// registry lock.
    left_running: bool,
    observed: Observed,
}

struct Registry {
    entries: HashMap<String, Entry>,
    next_serial: u64,
    /// The core is leaving (`stop_all`): no turn starts from here on.
    closed: bool,
}

fn reg() -> &'static (Mutex<Registry>, Condvar) {
    static R: OnceLock<(Mutex<Registry>, Condvar)> = OnceLock::new();
    R.get_or_init(|| (Mutex::new(Registry { entries: HashMap::new(), next_serial: 1, closed: false }), Condvar::new()))
}

fn lock() -> std::sync::MutexGuard<'static, Registry> {
    reg().0.lock().unwrap_or_else(|e| e.into_inner())
}

/// A chat id like the window's own (chats.store `newId`), so a helper opened
/// as a chat later looks like any other.
pub fn mint_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let base36 = |mut n: u64| {
        let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
        let mut s = Vec::new();
        loop {
            s.push(digits[(n % 36) as usize]);
            n /= 36;
            if n == 0 {
                break;
            }
        }
        s.reverse();
        String::from_utf8(s).unwrap_or_default()
    };
    format!("c{}{}", base36(crate::throttle::now_ms()), base36(SEQ.fetch_add(1, Ordering::Relaxed) + 1000))
}

pub fn has(id: &str) -> bool {
    lock().entries.contains_key(id)
}

/// How many helpers are in a turn: what keeps the core, as a running
/// session does. Between turns a helper has no process to keep.
pub fn running() -> usize {
    lock().entries.values().filter(|e| e.run.is_some()).count()
}

/// Whether a helper runs on this account folder, between turns too: its next
/// turn starts there.
pub fn uses_account(account_path: &str) -> bool {
    lock().entries.values().any(|e| crate::pty::same_dir(&e.spec.account_path, account_path))
}

/// Every folder the helpers other than `except` work in, between turns too:
/// the one each was sent to, and the worktree it made once the CLI has said
/// where — Codex says it only in its rollout, read here off the registry.
pub fn working_dirs(except: Option<&str>) -> Vec<String> {
    let found: Vec<(String, Option<String>, Provider, String, Option<String>)> = lock()
        .entries
        .iter()
        .filter(|(id, _)| Some(id.as_str()) != except)
        .map(|(_, e)| (e.spec.folder.clone(), e.cwd.clone(), e.spec.provider, e.spec.account_path.clone(), e.session.clone()))
        .collect();
    found
        .into_iter()
        .flat_map(|(folder, cwd, provider, account, session)| {
            let cwd = cwd.or_else(|| match provider {
                Provider::Codex => crate::codex::session::thread_cwd(&account, session.as_deref()?),
                Provider::Claude => None,
            });
            [Some(folder), cwd].into_iter().flatten()
        })
        .collect()
}

/// The core is leaving: every turn in progress is stopped with its process
/// tree, and none starts after.
pub fn stop_all() {
    stop_turns(true);
    save();
    // A core that takes the helpers up again has no job of theirs to see
    // what is left by: it goes with this one.
    let jobs: Vec<(String, std::sync::Arc<crate::procs::Job>)> =
        lock().entries.iter().filter_map(|(id, e)| Some((id.clone(), e.job.clone()?))).collect();
    for (id, job) in jobs {
        end_leftovers(&id, Some(&job));
    }
}

/// Ends what a helper's turns left running, once the helper is done with.
fn end_leftovers(id: &str, job: Option<&crate::procs::Job>) {
    let Some(job) = job else { return };
    let ended = crate::procs::end_leftovers(job, None);
    if !ended.is_empty() {
        crate::log::info("background", &format!("{}: ended what its turns left running: {}", crate::agents::label(id), ended.join(", ")));
    }
}

/// Stops every turn in progress with its process tree and drops what was
/// queued; `close` also keeps any from starting after. A turn claimed but not
/// yet started is stopped too: it does not start. Returns how many it
/// stopped.
pub fn stop_turns(close: bool) -> usize {
    let (pids, claimed): (Vec<u32>, usize) = {
        let mut r = lock();
        r.closed |= close;
        let mut pids = Vec::new();
        let mut claimed = 0;
        for e in r.entries.values_mut() {
            // Stopped for Luna's sake, not ended: its caller hears so.
            e.cut_short |= e.run.is_some();
            match stop(e) {
                Some(pid) => pids.push(pid),
                None if e.run.is_some() => claimed += 1,
                None => {}
            }
        }
        (pids, claimed)
    };
    reg().1.notify_all();
    let stopped = pids.len() + claimed;
    for pid in pids {
        crate::pty::kill_tree(Some(pid));
    }
    stopped
}

/// Takes a helper's turn off, under the lock: drops what was queued, counted
/// as a turn that ended (whoever queued it waits on its end, which will not
/// come now), lets a held question go, and marks a claimed turn with no
/// process yet stopped. Returns the pid left to kill.
fn stop(e: &mut Entry) -> Option<u32> {
    if !e.queue.is_empty() {
        e.queue.clear();
        e.turns_ended += 1;
    }
    e.held.clear();
    let run = e.run.as_mut()?;
    if run.pid.is_none() {
        run.stopped = true;
    }
    run.pid
}

/// Starts a helper on its opening prompt.
pub fn start(id: &str, spec: Spec, prompt: &str) -> Result<(), String> {
    let mut e = Entry {
        spec,
        started_ms: crate::throttle::now_ms(),
        session: None,
        cwd: None,
        run: None,
        turns_ended: 0,
        queue: Vec::new(),
        log: VecDeque::new(),
        held: VecDeque::new(),
        error: None,
        cut_short: false,
        job: None,
        left_running: false,
        observed: Observed::default(),
    };
    claim(&mut e);
    lock().entries.insert(id.to_string(), e);
    if let Err(e) = run_turn(id, prompt) {
        lock().entries.remove(id);
        changed();
        return Err(e);
    }
    changed();
    Ok(())
}

/// The arguments and working directory of the next turn.
fn command_for(e: &Entry, id: &str) -> (Vec<String>, String, Vec<(&'static str, String)>) {
    let s = &e.spec;
    let mut args: Vec<String> = Vec::new();
    let mut envs: Vec<(&'static str, String)> = Vec::new();
    // A worktree the first turn made, while it is there; Claude Code may
    // drop a clean one on exit, and is then asked for it again by name.
    let made = e.cwd.clone().filter(|c| !crate::pty::same_dir(c, &s.folder));
    let mut dir = s.folder.clone();
    match s.provider {
        Provider::Claude => {
            args.extend(["-p", "--output-format", "stream-json", "--verbose"].map(String::from));
            if let Some(m) = &s.model {
                args.extend(["--model".into(), m.clone()]);
            }
            args.extend(["--effort".into(), s.effort.clone(), "--permission-mode".into(), s.mode.clone()]);
            match (&e.session, &made) {
                (Some(sid), Some(wt)) if std::path::Path::new(wt).is_dir() => {
                    dir = wt.clone();
                    args.extend(["--resume".into(), sid.clone()]);
                }
                (Some(sid), Some(wt)) => {
                    let name = std::path::Path::new(wt).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    args.extend(["--worktree".into(), name, "--resume".into(), sid.clone()]);
                }
                (Some(sid), None) => args.extend(["--resume".into(), sid.clone()]),
                (None, _) if s.worktree => args.push("--worktree".into()),
                (None, _) => {}
            }
            if let Some(mcp) = s.token.as_deref().and_then(|t| crate::pty::claude_mcp_file(id, t)) {
                args.extend(["--mcp-config".into(), mcp.to_string_lossy().into_owned()]);
                args.extend(["--permission-prompt-tool".into(), "mcp__luna__luna_permission".into()]);
                // The CLI's to call, not the model's: hidden from its tools,
                // and the prompts still come through.
                args.extend(["--disallowedTools".into(), "mcp__luna__luna_permission".into()]);
                if s.tools {
                    args.extend(["--allowedTools".into(), "mcp__luna".into()]);
                }
            }
            // A helper with tools has helpers of its own to hear from: what a
            // notice names is a file there, to read without asking.
            if s.tools {
                let notes = crate::agents::notes_dir();
                if std::fs::create_dir_all(&notes).is_ok() {
                    args.extend(["--add-dir".into(), notes.to_string_lossy().into_owned()]);
                }
            }
            envs.push(("CLAUDE_CONFIG_DIR", s.account_path.clone()));
            envs.push(("DISABLE_AUTOUPDATER", "1".into()));
        }
        Provider::Codex => {
            args.push("exec".into());
            match &e.session {
                Some(thread) => {
                    args.extend(["resume".into(), thread.clone()]);
                    if let Some(wt) = made.filter(|w| std::path::Path::new(w).is_dir()) {
                        dir = wt;
                    }
                }
                None => {
                    args.extend(["-C".into(), s.folder.clone()]);
                    if s.worktree {
                        args.push("--worktree".into());
                    }
                }
            }
            args.extend(["--json".into(), "--skip-git-repo-check".into()]);
            // `exec` cannot ask for the folder's trust and goes without the
            // project's config and hooks if it lacks it; a helper is where it
            // was sent, trusted for the run (codex/trust.rs).
            args.extend(crate::codex::trust::trust_override(&s.folder));
            if let Some(m) = &s.model {
                args.extend(["--model".into(), m.clone()]);
            }
            args.extend(["-c".into(), format!("model_reasoning_effort=\"{}\"", s.effort)]);
            if s.hooks_unasked {
                args.push(crate::pty::HOOKS_UNASKED.into());
            }
            if s.auto_review {
                args.push("--approve-for-me".into());
            } else {
                // `exec` asks nobody: the sandbox is what holds it.
                args.extend(["-c".into(), "approval_policy=\"never\"".into()]);
                args.extend(["-c".into(), format!("sandbox_mode=\"{}\"", s.mode)]);
            }
            if let Some(sb) = crate::pty::codex_windows_sandbox(&s.mode) {
                args.extend(["-c".into(), sb]);
            }
            if let Some(path) = crate::pty::codex_sandbox_path(&s.mode) {
                envs.push(("PATH", path));
            }
            if let Some(token) = s.token.clone().filter(|_| s.tools) {
                args.extend(crate::pty::codex_mcp_args());
                envs.push(("LUNA_MCP_TOKEN", token));
            }
            envs.push(("CODEX_HOME", s.account_path.clone()));
            // The prompt comes on stdin; Claude Code reads it there unasked.
            args.push("-".into());
        }
    }
    (args, dir, envs)
}

/// Takes the helper's run for a turn about to start, under the same lock that
/// found it free: text that arrives before the process is up queues behind
/// it rather than starting a second process on the same conversation.
fn claim(e: &mut Entry) {
    e.run = Some(Run { pid: None, started_ms: crate::throttle::now_ms(), stopped: false });
}

/// Starts the next turn's process with `prompt` on its stdin, on a run
/// `claim` has taken; a turn that could not start gives the claim back,
/// counted as a turn that ended on the error: whoever queued it, or queued
/// behind it, counts on its end, and `turn_over` never comes for it. Text
/// queued behind it fails with it, counted as the turn after, so whoever
/// waits on it hears — rather than starting what would most likely fail the
/// same way.
fn run_turn(id: &str, prompt: &str) -> Result<(), String> {
    let started = spawn_turn(id, prompt);
    if let Err(err) = &started {
        if let Some(e) = lock().entries.get_mut(id) {
            if e.run.as_ref().is_some_and(|r| r.pid.is_none()) {
                e.run = None;
                note(e, &format!("could not start the turn: {err}"));
                e.error = Some(err.clone());
                e.turns_ended += 1;
                if !e.queue.is_empty() {
                    e.queue.clear();
                    note(e, "dropped what was queued behind it");
                    e.turns_ended += 1;
                }
            }
        }
        reg().1.notify_all();
        changed();
    }
    started
}

/// Why a turn claimed before a stop does not run.
const STOPPED: &str = "it was stopped before it started";
/// Once the CLI's process has gone, a reader blocked on its pipe this long has
/// read all it wrote: the pipe is empty, and what holds it open is something it
/// left running, which may do so for days.
const DRAINED_AFTER: std::time::Duration = std::time::Duration::from_millis(500);
/// The most a turn's end waits on a reader that is not there yet (a stalled
/// disk behind the registry lock, a job writing to the pipe without end).
const DRAIN_AT_MOST: std::time::Duration = std::time::Duration::from_secs(30);

/// How the reader of a turn's stdout stands, for the thread that sees the turn
/// out: the turn is over when the CLI is, not when the pipe closes, as a job it
/// left running can have inherited the pipe. What the CLI wrote before it went
/// is still to be read by then, and a blocked read is the only sign that all
/// of it has been.
#[derive(Default)]
struct OutReader {
    /// In a read that has not returned.
    reading: std::sync::atomic::AtomicBool,
    reads: std::sync::atomic::AtomicU64,
    /// The turn is over: what comes down the pipe from here is not the CLI's.
    over: std::sync::atomic::AtomicBool,
}

impl OutReader {
    /// Reads `out` to its end as bytes, as stderr is, handing each line that
    /// holds JSON to `on_event`, which says whether to go on.
    fn read_events(&self, mut out: impl std::io::Read, mut on_event: impl FnMut(&Value) -> bool) {
        use std::sync::atomic::Ordering::SeqCst;
        let mut buf = vec![0u8; 64 * 1024];
        let mut line = Vec::new();
        loop {
            self.reading.store(true, SeqCst);
            let n = out.read(&mut buf).unwrap_or(0);
            self.reading.store(false, SeqCst);
            self.reads.fetch_add(1, SeqCst);
            if n == 0 {
                break;
            }
            for chunk in buf[..n].split_inclusive(|b| *b == b'\n') {
                line.extend_from_slice(chunk);
                if chunk.last() == Some(&b'\n') {
                    if serde_json::from_slice::<Value>(&line).is_ok_and(|v| !on_event(&v)) {
                        return;
                    }
                    line.clear();
                }
            }
        }
        // The last line, should it have come with no line end.
        if let Ok(v) = serde_json::from_slice::<Value>(&line) {
            on_event(&v);
        }
    }

    /// Waits for what the CLI wrote before it went to have been read: for the
    /// reader to end (`read_out` closes with it), or to sit in a read with
    /// nothing coming, however long it took to get through the rest.
    fn await_last_words(&self, read_out: &std::sync::mpsc::Receiver<()>, drained_after: std::time::Duration, at_most: std::time::Duration) {
        use std::sync::atomic::Ordering::SeqCst;
        let start = std::time::Instant::now();
        let (mut reads, mut since) = (self.reads.load(SeqCst), start);
        while start.elapsed() < at_most {
            if !matches!(read_out.recv_timeout(std::time::Duration::from_millis(20)), Err(std::sync::mpsc::RecvTimeoutError::Timeout)) {
                return;
            }
            let now = self.reads.load(SeqCst);
            if now != reads {
                (reads, since) = (now, std::time::Instant::now());
            }
            if self.reading.load(SeqCst) && since.elapsed() >= drained_after {
                return;
            }
        }
    }
}

/// One event off a turn's stdout, into its helper. The flag is looked at under
/// the lock: `turn_over` takes it after the flag is up and the next turn starts
/// after that, so an event of this turn can land neither after its end nor in
/// the next one. Whether to go on reading: not once the helper is gone.
fn take_event(id: &str, provider: Provider, reader: &OutReader, v: &Value) -> bool {
    let mut r = lock();
    if reader.over.load(std::sync::atomic::Ordering::SeqCst) {
        return true;
    }
    let Some(e) = r.entries.get_mut(id) else { return false };
    e.observed.event_ms = crate::throttle::now_ms();
    match provider {
        Provider::Claude => on_claude_event(e, v),
        Provider::Codex => on_codex_event(e, v),
    }
    true
}

/// Where Codex's running total stands as a turn starts, off the rollout: the
/// total saved when the last turn ended misses whatever a turn killed since
/// spent, which the rollout has and the next `turn.completed` will include.
/// None when that cannot be told, or for Claude Code.
fn turn_base(id: &str) -> Option<Usage> {
    let (account, thread) = {
        let r = lock();
        let e = r.entries.get(id)?;
        if e.spec.provider != Provider::Codex {
            return None;
        }
        let Some(thread) = e.session.clone() else { return Some(Usage::default()) };
        (e.spec.account_path.clone(), thread)
    };
    let path = crate::codex::session::rollout_of_thread(&account, &thread)?;
    // A rollout with no count yet: its thread has spent nothing.
    Some(crate::codex::session::last_total_usage(&path).map_or(Usage::default(), |u| Usage::of_codex(&u)))
}

fn spawn_turn(id: &str, prompt: &str) -> Result<(), String> {
    let base = turn_base(id);
    let (args, dir, envs, provider) = {
        let r = lock();
        if r.closed {
            return Err("Luna is stopping".into());
        }
        let e = r.entries.get(id).ok_or("no such helper")?;
        if e.run.as_ref().is_some_and(|run| run.stopped) {
            return Err(STOPPED.into());
        }
        let (args, dir, envs) = command_for(e, id);
        (args, dir, envs, e.spec.provider)
    };
    if !std::path::Path::new(&dir).is_dir() {
        return Err(format!("{dir} does not exist"));
    }
    let mut cmd = std::process::Command::new(crate::cli::binary(provider));
    cmd.args(&args).current_dir(&dir);
    crate::pty::clean_env(&mut cmd);
    for (k, v) in &envs {
        cmd.env(k, v);
    }
    cmd.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    // A group of its own, as a pty gives its CLI: what kills a turn
    // (`pty::kill_tree`) takes the group whose id is the CLI's pid.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let thread_err = |e: std::io::Error| format!("could not start {}: {e}", provider.as_str());
    // The thread that sees the turn out is there before the process: one
    // that could not be made after it would leave the process running with
    // nobody to reap it or end its turn. It is handed the process once the
    // turn is recorded, and nothing if the turn does not start.
    let (hand, take) = std::sync::mpsc::channel::<(std::process::Child, Option<std::process::ChildStdout>)>();
    {
        let id = id.to_string();
        std::thread::Builder::new()
            .name(format!("luna-bg-{id}"))
            .spawn(move || {
                let Ok((mut child, stdout)) = take.recv() else { return };
                // On a thread of its own (see `OutReader`).
                let reader = std::sync::Arc::new(OutReader::default());
                let (drained, read_out) = std::sync::mpsc::channel::<()>();
                if let Some(out) = stdout {
                    let (helper, on_it) = (id.clone(), reader.clone());
                    let read = std::thread::Builder::new().name(format!("luna-bg-out-{id}")).spawn(move || {
                        let _drained = drained;
                        on_it.read_events(out, |v| take_event(&helper, provider, &on_it, v));
                    });
                    if let Err(e) = read {
                        crate::log::warn("background", &format!("cannot read what {} says: {e}", crate::agents::label(&id)));
                    }
                }
                let code = child.wait().ok().and_then(|s| s.code());
                reader.await_last_words(&read_out, DRAINED_AFTER, DRAIN_AT_MOST);
                reader.over.store(true, std::sync::atomic::Ordering::SeqCst);
                turn_over(&id, code);
            })
            .map_err(thread_err)?;
    }
    let mut child = cmd.spawn().map_err(|e| {
        crate::log::error("background", &format!("could not start {}: {e}", crate::agents::label(id)));
        format!("could not start {}: {e}", provider.as_str())
    })?;
    let pid = Some(child.id());
    // Into the helper's job before it has had time to start anything.
    if let Some(pid) = pid {
        if let Some(e) = lock().entries.get_mut(id) {
            match &e.job {
                Some(job) => {
                    crate::procs::join(job, pid);
                }
                None => e.job = crate::procs::adopt(pid).map(std::sync::Arc::new),
            }
        }
    }
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    // Written whole and closed: the CLI reads its prompt to the end of input.
    if let Some(mut stdin) = stdin {
        let prompt = prompt.to_string();
        if let Err(e) = std::thread::Builder::new().spawn(move || {
            let _ = stdin.write_all(prompt.as_bytes());
        }) {
            abandon(child, pid);
            return Err(thread_err(e));
        }
    }
    // Both pipes are read to their end as bytes: `lines()` stops at the first
    // line that is not UTF-8 — a hook's output in the console's code page —
    // and a pipe nobody drains any more blocks the CLI once it fills.
    if let Some(err) = stderr {
        let id = id.to_string();
        let drained = std::thread::Builder::new().spawn(move || {
            for line in BufReader::new(err).split(b'\n').map_while(Result::ok) {
                let line = String::from_utf8_lossy(&line);
                let line = line.trim();
                if line.is_empty() || line.starts_with("Reading additional input") {
                    continue;
                }
                if let Some(e) = lock().entries.get_mut(&id) {
                    e.observed.event_ms = crate::throttle::now_ms();
                    note(e, &format!("stderr: {}", one_line(line)));
                }
            }
        });
        if let Err(e) = drained {
            abandon(child, pid);
            return Err(thread_err(e));
        }
    }
    {
        let mut r = lock();
        let closed = r.closed;
        // A stop that came while the process was starting found no pid to
        // kill: the process goes here instead.
        let refused = match r.entries.get_mut(id) {
            _ if closed => Some("Luna is stopping"),
            None => Some("the helper was deleted"),
            Some(e) if e.run.as_ref().is_some_and(|run| run.stopped) => Some(STOPPED),
            Some(e) => {
                e.run = Some(Run { pid, started_ms: crate::throttle::now_ms(), stopped: false });
                e.observed.begin_turn(base);
                e.error = None;
                e.cut_short = false;
                note(e, &format!("› {}", one_line(prompt)));
                None
            }
        };
        if let Some(why) = refused {
            drop(r);
            abandon(child, pid);
            return Err(why.into());
        }
    }
    crate::log::info("background", &format!("{} turn started, pid {pid:?}: {} {}", crate::agents::label(id), provider.as_str(), args.join(" ")));
    // The reader waits on the other end; should it be gone, the turn still
    // ends here rather than with nobody to say so.
    if let Err(std::sync::mpsc::SendError((child, _))) = hand.send((child, stdout)) {
        abandon(child, pid);
        turn_over(id, None);
    }
    Ok(())
}

/// Kills a turn's process that is not to run, its tree with it, and reaps it.
fn abandon(mut child: std::process::Child, pid: Option<u32>) {
    crate::pty::kill_tree(pid);
    let _ = child.kill();
    let _ = child.wait();
}

fn on_claude_event(e: &mut Entry, v: &Value) {
    if v["type"] == "assistant" {
        claude_message_usage(&mut e.observed, &v["message"]);
    }
    match (v["type"].as_str(), v["subtype"].as_str()) {
        (Some("system"), Some("init")) => {
            e.session = v["session_id"].as_str().map(str::to_owned).or(e.session.take());
            e.cwd = v["cwd"].as_str().map(str::to_owned).or(e.cwd.take());
            e.observed.model = v["model"].as_str().map(str::to_owned).or(e.observed.model.take());
        }
        (Some("assistant"), _) if v["parent_tool_use_id"].is_null() => {
            // A fallback mid-turn shows here, not in `init`. The CLI's own
            // stand-in for a message no model wrote reads `<synthetic>`.
            if let Some(m) = v["message"]["model"].as_str().filter(|m| !m.starts_with('<')) {
                e.observed.model = Some(m.to_string());
            }
            for b in v["message"]["content"].as_array().into_iter().flatten() {
                match b["type"].as_str() {
                    Some("text") => note(e, &format!("said: {}", one_line(b["text"].as_str().unwrap_or("")))),
                    Some("tool_use") => {
                        let a = Ask::from_claude_tool(b["name"].as_str().unwrap_or("?"), &b["input"]);
                        note(e, &format!("{}: {}", a.tool.unwrap_or_default(), one_line(&a.detail.unwrap_or_default())));
                    }
                    _ => {}
                }
            }
        }
        (Some("result"), sub) => {
            claude_spend(e, v);
            let ok = sub == Some("success") && v["is_error"].as_bool() != Some(true);
            let secs = v["duration_ms"].as_f64().unwrap_or(0.0) / 1000.0;
            if ok {
                note(e, &format!("turn done in {secs:.0}s"));
            } else {
                let why = v["result"].as_str().or(sub).unwrap_or("error").to_string();
                note(e, &format!("turn failed: {}", one_line(&why)));
                e.error = Some(why);
            }
        }
        _ => {}
    }
}

/// What a `result` says the turn spent. Its `total_cost_usd` and `modelUsage`
/// run on from whatever the transcript saved, over a resume too, and `usage` is
/// the main loop's alone, subagents left out: the turn is
/// `model_usage_since_last_result`, summed for the session as the CLI's schema
/// tells a reader to. Without that field (a CLI that does not write it, an error
/// result) the turn is `usage`, its cost what the running total rose by. The
/// running totals stand as a floor: they hold what a turn killed before its
/// result left in the transcript, and what ran before Luna kept count.
fn claude_spend(e: &mut Entry, v: &Value) {
    let running_cost = v["total_cost_usd"].as_f64();
    let spent = e.observed.spend.session;
    let turn = Usage::of_claude_models(&v["model_usage_since_last_result"]).or_else(|| {
        let u = &v["usage"];
        u.is_object().then(|| Usage {
            cost_usd: running_cost.map(|c| usd((c - spent.cost_usd.unwrap_or(0.0)).max(0.0))),
            ..Usage::of_claude(u)
        })
    });
    let Some(mut turn) = turn else { return };
    // An error result states zeros where its turn did spend: the messages stand.
    if (Usage { cost_usd: None, ..turn }) == Usage::default() {
        turn = Usage { cost_usd: turn.cost_usd, ..e.observed.unreported };
    }
    e.observed.unreported = Usage::default();
    e.observed.by_message.clear();
    let mut running = Usage::of_claude_models(&v["modelUsage"]).unwrap_or_default();
    running.cost_usd = running_cost.or(running.cost_usd).map(usd);
    e.observed.spend = Spend { last_turn: Some(turn), session: spent.plus(turn).at_least(running) };
}

/// One assistant message of a Claude Code turn, for `unreported`.
fn claude_message_usage(o: &mut Observed, message: &Value) {
    let (Some(id), true) = (message["id"].as_str(), message["usage"].is_object()) else { return };
    let was = o.by_message.get(id).copied().unwrap_or_default();
    let now = Usage::of_claude(&message["usage"]).at_least(was);
    o.by_message.insert(id.to_string(), now);
    o.unreported = o.unreported.since(was).plus(now);
}

/// What `turn.completed` says: its usage is the thread's running total, resumes
/// included, so the turn is what it rose by from where the total stood as it
/// started. Not knowing that, there is no telling: the total saved when the last
/// turn ended would charge this one with whatever a turn killed since spent.
fn codex_spend(e: &mut Entry, usage: &Value) {
    if !usage.is_object() {
        return;
    }
    let total = Usage::of_codex(usage);
    let o = &mut e.observed;
    o.turn_stated = true;
    o.spend = Spend { last_turn: o.turn_base.map(|base| total.since(base)), session: total.at_least(o.spend.session) };
}

/// The model Codex says it moved a turn to, from the one error item that is no
/// error: `model rerouted: <from> -> <to> (<reason>)`.
fn rerouted_to(message: &str) -> Option<&str> {
    let (_, moved) = message.split_once("model rerouted: ")?;
    let (_, to) = moved.split_once(" -> ")?;
    let to = to.split(" (").next()?.trim();
    (!to.is_empty()).then_some(to)
}

fn on_codex_event(e: &mut Entry, v: &Value) {
    let item = &v["item"];
    match v["type"].as_str() {
        Some("thread.started") => e.session = v["thread_id"].as_str().map(str::to_owned).or(e.session.take()),
        Some("item.completed") => match item["type"].as_str() {
            Some("agent_message") => note(e, &format!("said: {}", one_line(item["text"].as_str().unwrap_or("")))),
            Some("command_execution") => note(
                e,
                &format!(
                    "ran: {} → {}",
                    one_line(item["command"].as_str().unwrap_or("")),
                    item["exit_code"].as_i64().map(|c| c.to_string()).unwrap_or_else(|| "?".into())
                ),
            ),
            Some("error") if item["message"].as_str().and_then(rerouted_to).is_some() => {
                let message = item["message"].as_str().unwrap_or("");
                e.observed.model = rerouted_to(message).map(str::to_owned);
                e.observed.rerouted = true;
                note(e, &one_line(message));
            }
            Some(other) if other != "reasoning" => note(e, other),
            _ => {}
        },
        Some("turn.failed") | Some("error") => {
            let why = v["error"]["message"].as_str().or(v["message"].as_str()).unwrap_or("error").to_string();
            note(e, &format!("error: {}", one_line(&why)));
            // Codex says its retries the same way ("Reconnecting... 2/5"): a
            // turn that gets through after them has not failed.
            if !why.starts_with("Reconnecting") {
                e.error = Some(why);
            }
        }
        Some("turn.completed") => {
            codex_spend(e, &v["usage"]);
            note(e, "turn done");
            e.error = None;
        }
        _ => {}
    }
}

/// The turn's process has exited: count the turn, let any held question go,
/// and start the turn the queue has waiting.
fn turn_over(id: &str, code: Option<i32>) {
    // Before the turn reads as over: whoever is owed its answer must not hear
    // "finished" ahead of the job it left running.
    let job = lock().entries.get(id).and_then(|e| e.job.clone());
    let left_running = job.is_some_and(|job| !crate::procs::left_running(&job).is_empty());
    let (next, thread, generation, stated, rerouted) = {
        let mut r = lock();
        let Some(e) = r.entries.get_mut(id) else { return };
        e.run = None;
        e.turns_ended += 1;
        e.observed.turn_ended_ms = crate::throttle::now_ms();
        e.observed.settle_unreported();
        e.held.clear();
        e.left_running = left_running;
        if code != Some(0) && e.error.is_none() {
            e.error = Some(match code {
                Some(c) => format!("{} exited with code {c}", e.spec.provider.as_str()),
                // A signal on Unix: nothing says which but that it was not its own doing.
                None => format!("{} was killed", e.spec.provider.as_str()),
            });
        }
        // Codex says where it works only in its rollout.
        if e.spec.provider == Provider::Codex && e.cwd.is_none() {
            if let Some(thread) = &e.session {
                e.cwd = crate::codex::session::thread_cwd(&e.spec.account_path, thread);
            }
        }
        let next = (!e.queue.is_empty()).then(|| std::mem::take(&mut e.queue).join("\n\n"));
        if next.is_some() {
            claim(e);
        }
        let thread = e.session.clone().filter(|_| e.spec.provider == Provider::Codex).map(|t| (e.spec.account_path.clone(), t));
        (next, thread, e.observed.generation, e.observed.turn_stated, std::mem::take(&mut e.observed.rerouted))
    };
    reg().1.notify_all();
    crate::log::info("background", &format!("{} turn over, code {code:?}", crate::agents::label(id)));
    // Codex's events name no model but a reroute's, which stands for its turn;
    // otherwise its rollout says what the turn ran on, read with the lock let go.
    if let Some(rollout) = thread.and_then(|(account, t)| crate::codex::session::rollout_of_thread(&account, &t)) {
        let model = if rerouted { None } else { crate::codex::session::last_model(&rollout) };
        let total = if stated { None } else { crate::codex::session::last_total_usage(&rollout).map(|u| Usage::of_codex(&u)) };
        if let Some(e) = lock().entries.get_mut(id) {
            e.observed.apply_rollout(generation, model, total);
        }
    }
    // One that cannot start is recorded as ended by `run_turn`.
    if let Some(text) = next {
        let _ = run_turn(id, &text);
    }
    changed();
}

fn note(e: &mut Entry, line: &str) {
    if e.log.len() >= LOG_LINES {
        e.log.pop_front();
    }
    e.log.push_back(line.to_string());
}

pub fn one_line(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= LOG_CHARS {
        return flat;
    }
    format!("{}…", flat.chars().take(LOG_CHARS).collect::<String>())
}

/// The helper's turn as the agent layer counts it: busy while a turn's
/// process runs, waiting while a question or permission is held.
pub fn turn_state(id: &str) -> Option<crate::activity::TurnState> {
    use crate::activity::Turn;
    let r = lock();
    let e = r.entries.get(id)?;
    let turn = match &e.run {
        Some(_) if !e.held.is_empty() => Turn::Waiting,
        Some(_) => Turn::Busy,
        None => Turn::Idle,
    };
    Some(crate::activity::TurnState {
        turn,
        busy: e.run.is_some(),
        turns_ended: e.turns_ended,
        children_busy: e.run.is_none() && e.left_running,
    })
}

/// What the power rules see of every helper: its turn, and between turns
/// what its turns left running, by the same rule as a chat's (activity.rs):
/// it holds the machine for as long as it lives. Noted for `turn_state`, which
/// is what tells its caller and `reap` that a job still runs.
pub fn activity() -> Vec<crate::activity::SessionActivity> {
    let ids: Vec<(String, Option<std::sync::Arc<crate::procs::Job>>)> = lock()
        .entries
        .iter()
        .map(|(id, e)| (id.clone(), e.job.clone().filter(|_| e.run.is_none())))
        .collect();
    let mut moved = false;
    let out = ids
        .into_iter()
        .filter_map(|(id, job)| {
            let left = job.map(|job| crate::procs::left_running(&job));
            if let Some(left) = &left {
                if let Some(e) = lock().entries.get_mut(&id).filter(|e| e.run.is_none()) {
                    moved |= e.left_running != !left.is_empty();
                    e.left_running = !left.is_empty();
                }
            }
            let t = turn_state(&id)?;
            let mut a = crate::activity::SessionActivity::of_turn(&id, t.turn, t.busy);
            if let Some(left) = left.filter(|left| !left.is_empty()) {
                a.busy = true;
                a.procs = left;
            }
            Some(a)
        })
        .collect();
    if moved {
        changed();
    }
    out
}

/// Text for the helper: the next turn now, or after the one running.
/// Returns whether it was queued.
pub fn send(id: &str, text: &str) -> Result<bool, String> {
    let queued = {
        let mut r = lock();
        let e = r.entries.get_mut(id).ok_or("no such helper")?;
        if !e.held.is_empty() {
            return Err(format!("helper {id} is waiting on a question or permission: answer it with luna_answer"));
        }
        if e.run.is_some() {
            e.queue.push(text.to_string());
            note(e, &format!("queued: {}", one_line(text)));
            true
        } else {
            claim(e);
            false
        }
    };
    if !queued {
        run_turn(id, text)?;
    }
    changed();
    Ok(queued)
}

/// Stops the turn in progress; the conversation stays for the next one.
pub fn kill(id: &str) {
    let pid = {
        let mut r = lock();
        let Some(e) = r.entries.get_mut(id) else { return };
        // Whoever is owed the turn hears it was stopped, not an exit code.
        if e.run.is_some() {
            e.error = Some("it was stopped, by luna_kill or from the window".into());
        }
        stop(e)
    };
    reg().1.notify_all();
    crate::pty::kill_tree(pid);
}

/// Why its last turn stopped short, if it did.
pub fn last_error(id: &str) -> Option<String> {
    lock().entries.get(id)?.error.clone()
}

/// How an `interrupt` went.
#[derive(Debug, PartialEq, Clone, Copy)]
pub struct Interrupted {
    /// The turn it was on was stopped, and the text starts the next.
    pub stopped: bool,
    /// The text waits behind a turn that goes on: one that would not stop in
    /// time, or that has no process to stop yet. It ends a turn after that one.
    pub queued: bool,
}

/// How long a turn claimed a moment ago gets to have a process to stop.
const CLAIM_WAIT: std::time::Duration = std::time::Duration::from_secs(3);
/// How long a stopped turn's process gets to read as gone.
const STOP_WAIT: std::time::Duration = std::time::Duration::from_secs(15);

/// Stops the turn in progress and starts the next on `text`, behind whatever
/// was queued for after it: the conversation goes on, the work the turn was
/// doing does not. All of it waits in the queue, which the turn's end takes up
/// whole, so nobody's text hangs on how the stop goes: what would not stop
/// still has it all behind it.
pub fn interrupt(id: &str, text: &str) -> Result<Interrupted, String> {
    let free = {
        let mut r = lock();
        let e = r.entries.get_mut(id).ok_or("no such helper")?;
        e.held.clear();
        let free = e.run.is_none();
        if free {
            claim(e);
        } else {
            note(e, "interrupted");
            e.queue.push(text.to_string());
        }
        free
    };
    reg().1.notify_all();
    if free {
        run_turn(id, text)?;
        changed();
        return Ok(Interrupted { stopped: false, queued: false });
    }
    let queued = Interrupted { stopped: false, queued: true };
    let deadline = std::time::Instant::now() + CLAIM_WAIT;
    let pid = loop {
        {
            let r = lock();
            let e = r.entries.get(id).ok_or("no such helper")?;
            // Taken up already: that turn ended by itself, and this is the next.
            if e.queue.is_empty() {
                return Ok(queued);
            }
            match e.run.as_ref().map(|run| run.pid) {
                Some(Some(pid)) => break pid,
                Some(None) if std::time::Instant::now() < deadline => {}
                _ => return Ok(queued),
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    crate::pty::kill_tree(Some(pid));
    // The turn is over once its reader has seen the process go.
    let deadline = std::time::Instant::now() + STOP_WAIT;
    let (m, cv) = reg();
    let mut r = m.lock().unwrap_or_else(|e| e.into_inner());
    while r.entries.get(id).and_then(|e| e.run.as_ref()).is_some_and(|run| run.pid == Some(pid)) {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            drop(r);
            changed();
            return Ok(queued);
        }
        r = cv.wait_timeout(r, left).unwrap_or_else(|e| e.into_inner()).0;
    }
    drop(r);
    changed();
    Ok(Interrupted { stopped: true, queued: false })
}

/// The worktree it made, once the CLI has said where: Claude Code as its
/// first turn starts, Codex in its rollout.
pub fn made_worktree(id: &str) -> Option<String> {
    let (cwd, folder, provider, account, session) = {
        let r = lock();
        let e = r.entries.get(id)?;
        if !e.spec.worktree {
            return None;
        }
        (e.cwd.clone(), e.spec.folder.clone(), e.spec.provider, e.spec.account_path.clone(), e.session.clone())
    };
    let cwd = cwd.or_else(|| match provider {
        Provider::Codex => crate::codex::session::thread_cwd(&account, session.as_deref()?),
        Provider::Claude => None,
    })?;
    (!crate::pty::same_dir(&cwd, &folder)).then_some(cwd)
}

/// Stops it and forgets it. Returns the worktree it made, if any.
pub fn remove(id: &str) -> Option<String> {
    kill(id);
    let e = lock().entries.remove(id);
    reg().1.notify_all();
    let _ = std::fs::remove_file(crate::paths::data_dir().join("mcp").join(format!("{id}.json")));
    changed();
    let e = e?;
    end_leftovers(id, e.job.as_deref());
    e.cwd.filter(|c| !crate::pty::same_dir(c, &e.spec.folder) && crate::worktree::is_worktree_of(&e.spec.folder, c))
}

/// The last lines of what it did, in place of a screen.
pub fn screen(id: &str, lines: usize) -> Option<String> {
    let r = lock();
    let e = r.entries.get(id)?;
    let from = e.log.len().saturating_sub(lines);
    let mut out: Vec<String> = e.log.iter().skip(from).cloned().collect();
    if let Some(err) = &e.error {
        out.push(format!("last error: {}", one_line(err)));
    }
    Some(out.join("\n"))
}

/// Its transcript (Claude Code) or rollout (Codex), once the first turn has
/// named its conversation.
pub fn transcript(id: &str) -> Option<std::path::PathBuf> {
    let (provider, account, session, cwd, folder) = {
        let r = lock();
        let e = r.entries.get(id)?;
        (e.spec.provider, e.spec.account_path.clone(), e.session.clone()?, e.cwd.clone(), e.spec.folder.clone())
    };
    match provider {
        Provider::Claude => Some(crate::claude::session::transcript_file(&account, cwd.as_deref().unwrap_or(&folder), &session)),
        Provider::Codex => crate::codex::session::rollout_of_thread(&account, &session),
    }
}

/// The conversation's id as the CLI knows it.
pub fn session_id(id: &str) -> Option<String> {
    lock().entries.get(id)?.session.clone()
}

/// Where it works: the worktree its first turn made, or its folder.
pub fn working_dir(id: &str) -> Option<String> {
    let r = lock();
    let e = r.entries.get(id)?;
    Some(e.cwd.clone().unwrap_or_else(|| e.spec.folder.clone()))
}

pub fn ask(id: &str) -> Option<Ask> {
    lock().entries.get(id)?.held.iter().find(|h| h.answer.is_none()).map(|h| h.ask.clone())
}

/// `luna_permission`: the CLI asks before a tool runs, or carries the model's
/// `AskUserQuestion`. Held until `luna_answer`, the turn's end or the timeout;
/// the answer is the JSON the CLI takes back.
pub fn permission(id: &str, args: &Value) -> Value {
    let tool = args["tool_name"].as_str().unwrap_or("").to_string();
    let input = args["input"].clone();
    let mut ask = if tool == "AskUserQuestion" {
        Ask::from_claude_questions(&input).unwrap_or_else(|| Ask::from_claude_tool(&tool, &input))
    } else {
        Ask::from_claude_tool(&tool, &input)
    };
    let serial = {
        let mut r = lock();
        let serial = r.next_serial;
        r.next_serial += 1;
        let Some(e) = r.entries.get_mut(id) else {
            return json!({ "behavior": "deny", "message": "Luna does not know this session." });
        };
        ask.id = Some(format!("a{serial}"));
        note(e, &format!("waits: {}", ask.line()));
        e.held.push_back(Held { serial, ask, input, answer: None });
        serial
    };
    changed();
    let deadline = std::time::Instant::now() + ASK_TIMEOUT;
    let (m, cv) = reg();
    let mut r = m.lock().unwrap_or_else(|e| e.into_inner());
    let answer = loop {
        let held = r.entries.get_mut(id).filter(|e| e.run.is_some()).and_then(|e| e.held.iter_mut().find(|h| h.serial == serial));
        match held {
            Some(h) => {
                if let Some(a) = h.answer.take() {
                    break Some(a);
                }
            }
            // Killed, deleted, or its turn is gone: nobody will answer.
            None => break None,
        }
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            break None;
        }
        r = cv.wait_timeout(r, left).unwrap_or_else(|e| e.into_inner()).0;
    };
    if let Some(e) = r.entries.get_mut(id) {
        e.held.retain(|h| h.serial != serial);
    }
    drop(r);
    changed();
    answer.unwrap_or_else(|| json!({ "behavior": "deny", "message": "Nobody answered in time; go on without it or ask in your reply." }))
}

/// The agent's answer to what the helper holds: `answers` for a question
/// (an option's number or label, or words of its own, one per question;
/// several numbers apart by commas where more than one may be picked),
/// `allow` for a permission, `note` as the reason for a refusal.
///
/// `ask_id` says which of what it holds this answers: two responders who saw
/// the same ask must not have the slower one's `allow` land on the next. One no
/// longer open is refused. Without it the answer goes through only when a
/// single ask is held, as it did when there could be only one: with several,
/// nothing says it was written on the first.
pub fn answer(id: &str, ask_id: Option<&str>, answers: &[String], allow: Option<bool>, note_text: Option<&str>) -> Result<(), String> {
    let mut r = lock();
    let e = r.entries.get_mut(id).ok_or("no such helper")?;
    let mut open = e.held.iter_mut().filter(|h| h.answer.is_none());
    let h = match ask_id {
        Some(want) => open.find(|h| h.ask.id.as_deref() == Some(want)).ok_or_else(|| {
            format!("ask {want} of helper {id} is no longer open: it was answered or went with its turn. Its `ask` in luna_list says what it holds now")
        })?,
        None => {
            let first = open.next().ok_or_else(|| format!("helper {id} is not waiting on anything"))?;
            if open.next().is_some() {
                return Err(format!(
                    "helper {id} holds several asks at once: say which with askId. The first is {}: it {}",
                    first.ask.id.as_deref().unwrap_or("?"),
                    first.ask.line()
                ));
            }
            first
        }
    };
    let reply = build_answer(&h.ask, &h.input, answers, allow, note_text)?;
    h.answer = Some(reply);
    drop(r);
    reg().1.notify_all();
    Ok(())
}

fn build_answer(ask: &Ask, input: &Value, answers: &[String], allow: Option<bool>, note_text: Option<&str>) -> Result<Value, String> {
    if ask.kind == "permission" {
        return match allow {
            Some(true) => Ok(json!({ "behavior": "allow", "updatedInput": input })),
            Some(false) => Ok(json!({ "behavior": "deny", "message": note_text.unwrap_or("The user declined.") })),
            None => Err("this is a permission: answer with allow true or false".into()),
        };
    }
    if allow == Some(false) {
        return Ok(json!({ "behavior": "deny", "message": note_text.unwrap_or("The user declined to answer.") }));
    }
    if answers.len() != ask.questions.len() {
        return Err(format!("{} question(s), {} answer(s): give one answer per question", ask.questions.len(), answers.len()));
    }
    let mut picked = serde_json::Map::new();
    for (q, a) in ask.questions.iter().zip(answers) {
        picked.insert(q.text.clone(), Value::String(crate::agents::resolve_option(q, a)));
    }
    let mut updated = input.clone();
    if let Some(o) = updated.as_object_mut() {
        o.insert("answers".into(), Value::Object(picked));
    }
    Ok(json!({ "behavior": "allow", "updatedInput": updated }))
}

/// A helper as the window lists it under its parent.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub id: String,
    pub parent: Option<String>,
    pub name: String,
    pub provider: Provider,
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ask: Option<Ask>,
    /// The account folder's name.
    pub account: String,
    pub model: Option<String>,
    pub effort: String,
    pub worktree: bool,
    pub started_ms: u64,
    /// While a turn runs, when it started.
    pub turn_started_ms: Option<u64>,
}

/// Every helper, for the window.
#[tauri::command]
pub fn background_helpers() -> Vec<Row> {
    let ids: Vec<(String, Spec, u64, Option<u64>)> = lock()
        .entries
        .iter()
        .map(|(id, e)| (id.clone(), e.spec.clone(), e.started_ms, e.run.as_ref().map(|r| r.started_ms)))
        .collect();
    ids.into_iter()
        .map(|(id, spec, started_ms, turn_started_ms)| Row {
            parent: crate::agents::parent_of(&id),
            name: crate::agents::chat_name(&id).unwrap_or_default(),
            state: crate::agents::state_of(&id).0,
            ask: ask(&id),
            provider: spec.provider,
            account: std::path::Path::new(&spec.account_path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            model: spec.model,
            effort: spec.effort,
            worktree: spec.worktree,
            started_ms,
            turn_started_ms,
            id,
        })
        .collect()
}

/// Tells the window the list has changed.
pub fn changed() {
    crate::emit::to_ui("agent://background", background_helpers());
}

/// What `luna_list` says of a helper beyond its state.
pub fn signs(id: &str) -> Option<Signs> {
    let r = lock();
    let e = r.entries.get(id)?;
    let o = &e.observed;
    let turn_started = e.run.as_ref().map(|run| run.started_ms);
    let last = [o.event_ms, o.turn_ended_ms, turn_started.unwrap_or(0)].into_iter().max().filter(|ms| *ms > 0);
    Some(Signs {
        turn_started_at: turn_started.and_then(iso),
        last_activity_at: last.and_then(iso),
        // A Codex rollout that would not read: what it was asked to run on.
        resolved_model: o.model.clone().or_else(|| e.spec.model.clone().filter(|_| e.spec.provider == Provider::Codex && e.turns_ended > 0)),
        spend: (o.spend != Spend::default()).then_some(o.spend),
    })
}

/// What the window needs to make a helper a chat of its own.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Promoted {
    pub id: String,
    pub provider: Provider,
    pub account_path: String,
    pub folder: String,
    pub worktree_path: Option<String>,
    pub session_id: Option<String>,
    pub parent: Option<String>,
    pub name: Option<String>,
    pub settings: Value,
    pub tools: bool,
}

/// Hands a helper over to a chat row: from here on the window runs it, by
/// resuming its conversation in a terminal like any chat's. Not while a turn
/// runs — the conversation would be in two processes at once.
#[tauri::command]
pub fn background_promote(id: String) -> Result<(), String> {
    let e = {
        let mut r = lock();
        let e = r.entries.get(&id).ok_or("no such helper")?;
        if e.run.is_some() {
            return Err("it is in the middle of a turn; open it once the turn is over".into());
        }
        r.entries.remove(&id).ok_or("no such helper")?
    };
    let worktree_path = e.cwd.clone().filter(|c| !crate::pty::same_dir(c, &e.spec.folder));
    let promoted = Promoted {
        provider: e.spec.provider,
        account_path: e.spec.account_path.clone(),
        folder: crate::worktree::project_of(&e.spec.folder).unwrap_or_else(|| e.spec.folder.clone()),
        worktree_path: worktree_path.or_else(|| crate::worktree::project_of(&e.spec.folder).map(|_| e.spec.folder.clone())),
        session_id: e.session.clone(),
        parent: crate::agents::parent_of(&id),
        name: crate::agents::chat_name(&id),
        settings: crate::agents::settings_of(&id),
        tools: e.spec.tools,
        id: id.clone(),
    };
    crate::agents::promoted(&id);
    crate::emit::to_ui("agent://promote", promoted);
    changed();
    Ok(())
}

#[tauri::command]
pub fn background_kill(id: String) {
    kill(&id);
}

#[tauri::command]
pub fn background_delete(id: String, drop_worktree: Option<bool>) -> Result<(), String> {
    crate::agents::delete_by_user(&id, drop_worktree.unwrap_or(true))
}

// ------------------------------------------------------- across cores --

/// How often the helpers are written out, when they have changed.
const SAVE_EVERY: std::time::Duration = std::time::Duration::from_secs(2);

/// A helper as `<data>/helpers.json` keeps it, for the core started after
/// this one: between turns a helper is nothing but this and its
/// conversation, which the CLI keeps on disk. What it was told and said is
/// not here, only where to find it.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Saved {
    id: String,
    spec: Spec,
    started_ms: u64,
    session: Option<String>,
    cwd: Option<String>,
    turns_ended: u32,
    /// A turn ran as this was written: should the core go before the next
    /// write, it went mid-turn and stopped it.
    running: bool,
    /// Text was queued behind that turn, which goes with it.
    queued: bool,
    cut_short: bool,
    ties: crate::agents::Ties,
    #[serde(default)]
    observed: Observed,
}

fn saved_file() -> std::path::PathBuf {
    crate::paths::data_dir().join("helpers.json")
}

/// Set once `restore` has read the file: before it, an empty registry is
/// not to be written over what the last core left.
static RESTORED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Writes the helpers out if they changed since the last write.
fn save() {
    use std::sync::atomic::Ordering;
    static LAST: Mutex<Vec<u8>> = Mutex::new(Vec::new());
    if !RESTORED.load(Ordering::SeqCst) {
        return;
    }
    let rows: Vec<(String, Spec, u64, Option<String>, Option<String>, u32, bool, bool, bool, Observed)> = lock()
        .entries
        .iter()
        .map(|(id, e)| {
            let queued = !e.queue.is_empty();
            (id.clone(), e.spec.clone(), e.started_ms, e.session.clone(), e.cwd.clone(), e.turns_ended, e.run.is_some(), queued, e.cut_short, e.observed.clone())
        })
        .collect();
    // The agent layer's lock after this one is let go: it is the one taken
    // first where both are held.
    let mut saved: Vec<Saved> = rows
        .into_iter()
        .map(|(id, spec, started_ms, session, cwd, turns_ended, running, queued, cut_short, observed)| Saved {
            ties: crate::agents::ties_of(&id).unwrap_or_default(),
            id,
            spec,
            started_ms,
            session,
            cwd,
            turns_ended,
            running,
            queued,
            cut_short,
            observed,
        })
        .collect();
    saved.sort_by_key(|s| s.started_ms);
    let Ok(bytes) = serde_json::to_vec_pretty(&saved) else { return };
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if *last == bytes {
        return;
    }
    let path = saved_file();
    let tmp = path.with_extension("json.tmp");
    match std::fs::write(&tmp, &bytes).and_then(|()| std::fs::rename(&tmp, &path)) {
        Ok(()) => *last = bytes,
        Err(e) => crate::log::warn("background", &format!("could not save the helpers to {}: {e}", path.display())),
    }
}

/// Takes up the helpers the last core left, and keeps them saved from here
/// on. A turn that core was in the middle of is over, stopped with it; the
/// caller owed its answer is told so. One whose first turn never named its
/// conversation has nothing to resume, and is let go.
pub fn restore() {
    let saved: Vec<Saved> = match std::fs::read(saved_file()) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            crate::log::warn("background", &format!("could not read helpers.json: {e}"));
            Vec::new()
        }),
        Err(_) => Vec::new(),
    };
    let mut back = Vec::new();
    for mut s in saved {
        if s.session.is_none() {
            crate::log::warn("background", &format!("let go of helper {}: its first turn never named its conversation", s.id));
            continue;
        }
        let ties = std::mem::take(&mut s.ties);
        let token = crate::agents::tie_back(&s.id, s.spec.provider, &s.spec.folder, &s.spec.account_path, ties);
        let (id, running) = (s.id.clone(), s.running);
        let mut e = taken_up(s);
        e.spec.token = token;
        lock().entries.insert(id.clone(), e);
        back.push(format!("{}{}", crate::agents::label(&id), if running { " (stopped mid-turn)" } else { "" }));
    }
    RESTORED.store(true, std::sync::atomic::Ordering::SeqCst);
    if !back.is_empty() {
        crate::log::warn("background", &format!("took up {} helper(s) the last core left: {}", back.len(), back.join(", ")));
        changed();
    }
    save();
    let _ = std::thread::Builder::new().name("luna-bg-save".into()).spawn(|| loop {
        std::thread::sleep(SAVE_EVERY);
        save();
    });
}

/// A saved helper as a registry entry, between turns: a turn that ran went
/// with the old core and counts as ended, and so does the text queued
/// behind it, which went too (as `stop` counts them).
fn taken_up(s: Saved) -> Entry {
    let mut e = Entry {
        spec: s.spec,
        started_ms: s.started_ms,
        session: s.session,
        cwd: s.cwd,
        run: None,
        turns_ended: s.turns_ended + u32::from(s.running) + u32::from(s.queued),
        queue: Vec::new(),
        log: VecDeque::new(),
        held: VecDeque::new(),
        error: None,
        cut_short: s.cut_short || s.running,
        job: None,
        left_running: false,
        observed: s.observed,
    };
    e.observed.settle_unreported();
    if s.running {
        note(&mut e, "Luna's core went mid-turn and stopped the turn");
        e.error = Some("Luna's core restarted mid-turn and stopped the turn".into());
    }
    if s.queued {
        note(&mut e, "dropped what was queued behind it");
    }
    note(&mut e, "taken up by a new core");
    e
}

/// Whether its last turn was stopped by the core going rather than ended.
pub fn cut_short(id: &str) -> bool {
    lock().entries.get(id).is_some_and(|e| e.cut_short)
}

/// For the agent layer's tests: a helper as it stands between turns, there to
/// talk to with no process behind it.
#[cfg(test)]
pub(crate) fn idle_for_test(id: &str, provider: Provider, turns_ended: u32, left_running: bool) {
    let mut e = tests::entry(provider);
    e.turns_ended = turns_ended;
    e.left_running = left_running;
    lock().entries.insert(id.to_string(), e);
}

#[cfg(test)]
pub(crate) fn gone_for_test(id: &str) {
    lock().entries.remove(id);
}

/// The helper in a turn with no process, so nothing real can be killed.
#[cfg(test)]
pub(crate) fn in_turn_for_test(id: &str) {
    if let Some(e) = lock().entries.get_mut(id) {
        claim(e);
    }
}

/// A helper as `agents`'s tests need one, with no process behind it: in a
/// turn or between turns, on a conversation they can write the transcript of.
#[cfg(test)]
pub fn test_helper(id: &str, account_path: &str, session: Option<&str>, busy: bool) {
    let e = Entry {
        spec: Spec {
            provider: Provider::Claude,
            account_path: account_path.into(),
            folder: account_path.into(),
            worktree: false,
            model: None,
            effort: "low".into(),
            mode: "default".into(),
            auto_review: false,
            tools: false,
            hooks_unasked: false,
            token: None,
        },
        started_ms: 0,
        session: session.map(str::to_owned),
        cwd: None,
        run: busy.then(|| Run { pid: None, started_ms: 1, stopped: false }),
        turns_ended: 0,
        queue: Vec::new(),
        log: VecDeque::new(),
        held: VecDeque::new(),
        error: None,
        cut_short: false,
        job: None,
        left_running: false,
        observed: Observed::default(),
    };
    lock().entries.insert(id.to_string(), e);
}

/// A test helper's turn has ended, its process never having been.
#[cfg(test)]
pub fn test_turn_ended(id: &str) {
    let mut r = lock();
    let e = r.entries.get_mut(id).expect("a test helper");
    e.run = None;
    e.turns_ended += 1;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two turns of `claude -p --output-format stream-json --verbose` 2.1.316,
    /// the second a `--resume`, cut to the fields read here.
    const CLAUDE_TURN_1: &str = r#"{"type":"result","subtype":"success","is_error":false,"total_cost_usd":0.00289842,
        "usage":{"input_tokens":2,"cache_creation_input_tokens":13583,"cache_read_input_tokens":16562,"output_tokens":32},
        "modelUsage":{"claude-haiku-5-5":{"inputTokens":2,"outputTokens":32,"cacheReadInputTokens":16562,"cacheCreationInputTokens":13583,"costUSD":0.00289842}},
        "model_usage_since_last_result":{"claude-haiku-5-5":{"inputTokens":2,"outputTokens":32,"cacheReadInputTokens":16562,"cacheCreationInputTokens":13583,"costUSD":0.00289842}}}"#;
    const CLAUDE_TURN_2: &str = r#"{"type":"result","subtype":"success","is_error":false,"total_cost_usd":0.00329787,
        "usage":{"input_tokens":2,"cache_creation_input_tokens":479,"cache_read_input_tokens":30145,"output_tokens":4},
        "modelUsage":{"claude-haiku-5-5":{"inputTokens":4,"outputTokens":36,"cacheReadInputTokens":46707,"cacheCreationInputTokens":14062,"costUSD":0.00329787}},
        "model_usage_since_last_result":{"claude-haiku-5-5":{"inputTokens":2,"outputTokens":4,"cacheReadInputTokens":30145,"cacheCreationInputTokens":479,"costUSD":0.00039945000000000003}}}"#;
    /// The same of `codex exec --json` 0.166.0: `usage` runs on over a resume.
    const CODEX_TURN_1: &str = r#"{"type":"turn.completed","usage":{"input_tokens":15515,"cached_input_tokens":12416,"cache_write_input_tokens":0,"output_tokens":5,"reasoning_output_tokens":0}}"#;
    const CODEX_TURN_2: &str = r#"{"type":"turn.completed","usage":{"input_tokens":32377,"cached_input_tokens":27776,"cache_write_input_tokens":0,"output_tokens":10,"reasoning_output_tokens":0}}"#;

    fn event(line: &str) -> Value {
        serde_json::from_str(line).unwrap()
    }

    #[test]
    fn a_claude_turn_is_what_it_added_and_the_session_what_the_cli_says() {
        let mut e = entry(Provider::Claude);
        on_claude_event(&mut e, &event(CLAUDE_TURN_1));
        on_claude_event(&mut e, &event(CLAUDE_TURN_2));
        let s = e.observed.spend;
        assert_eq!(s.last_turn, Some(Usage { input: 2, output: 4, cache_read: 30145, cache_write: 479, cost_usd: Some(0.00039945) }));
        assert_eq!(s.session, Usage { input: 4, output: 36, cache_read: 46707, cache_write: 14062, cost_usd: Some(0.00329787) });
    }

    #[test]
    fn a_claude_result_without_its_turn_figures_falls_back_on_the_running_total() {
        let mut e = entry(Provider::Claude);
        on_claude_event(&mut e, &event(CLAUDE_TURN_1));
        let mut v = event(CLAUDE_TURN_2);
        v.as_object_mut().unwrap().remove("model_usage_since_last_result");
        on_claude_event(&mut e, &v);
        let s = e.observed.spend;
        let turn = s.last_turn.unwrap();
        assert_eq!((turn.input, turn.output, turn.cache_read, turn.cache_write), (2, 4, 30145, 479));
        assert_eq!(turn.cost_usd, Some(0.00039945), "what the running total rose by");
        assert_eq!(s.session.cost_usd, Some(0.00329787));

        // A resumed session Luna kept no count of: the CLI's total stands.
        let mut e = entry(Provider::Claude);
        on_claude_event(&mut e, &event(CLAUDE_TURN_2));
        assert_eq!(e.observed.spend.session.output, 36);
        assert_eq!(e.observed.spend.last_turn.unwrap().output, 4);

        // An event with no figures at all leaves them be.
        let before = e.observed.spend;
        on_claude_event(&mut e, &json!({ "type": "result", "subtype": "error_during_execution" }));
        assert_eq!(e.observed.spend, before);
    }

    #[test]
    fn a_codex_turn_is_what_the_running_total_rose_by() {
        let mut e = entry(Provider::Codex);
        e.observed.begin_turn(Some(Usage::default()));
        on_codex_event(&mut e, &event(CODEX_TURN_1));
        assert_eq!(e.observed.spend.last_turn, Some(Usage { input: 3099, output: 5, cache_read: 12416, cache_write: 0, cost_usd: None }));
        let base = e.observed.spend.session;
        e.observed.begin_turn(Some(base));
        on_codex_event(&mut e, &event(CODEX_TURN_2));
        let s = e.observed.spend;
        assert_eq!(s.last_turn, Some(Usage { input: 1502, output: 5, cache_read: 15360, cache_write: 0, cost_usd: None }), "as the rollout's last_token_usage has it");
        assert_eq!(s.session, Usage { input: 4601, output: 10, cache_read: 27776, cache_write: 0, cost_usd: None });
        assert!(!serde_json::to_string(&s).unwrap().contains("costUsd"), "Codex states no cost");
    }

    /// A turn of `codex exec resume` killed mid-way, as 0.166.0 left it: no
    /// `turn.completed`, the rollout's total on, and the next turn's total
    /// holding what the killed one spent.
    fn codex_total(input: u64, cached: u64, output: u64) -> Usage {
        Usage::of_codex(&json!({ "input_tokens": input, "cached_input_tokens": cached, "output_tokens": output }))
    }

    #[test]
    fn a_killed_codex_turn_is_its_own_not_the_next_ones() {
        let first = codex_total(16369, 12416, 5);
        let killed = codex_total(32789, 28672, 50);
        let mut e = entry(Provider::Codex);
        e.observed.begin_turn(Some(Usage::default()));
        on_codex_event(&mut e, &event(r#"{"type":"turn.completed","usage":{"input_tokens":16369,"cached_input_tokens":12416,"output_tokens":5}}"#));
        assert_eq!(e.observed.spend.session, first);

        // Killed: the rollout read once it is over counts it there and then.
        e.observed.begin_turn(Some(first));
        let generation = e.observed.generation;
        e.observed.apply_rollout(generation, None, Some(killed));
        assert_eq!(e.observed.spend.session, killed);
        assert_eq!(e.observed.spend.last_turn, Some(killed.since(first)));

        // The next turn is charged with what it spent itself.
        e.observed.begin_turn(Some(killed));
        on_codex_event(&mut e, &event(r#"{"type":"turn.completed","usage":{"input_tokens":50089,"cached_input_tokens":45312,"output_tokens":55}}"#));
        let turn = e.observed.spend.last_turn.unwrap();
        assert_eq!((turn.input, turn.cache_read, turn.output), (660, 16640, 5));
        assert_eq!(e.observed.spend.session, codex_total(50089, 45312, 55));

        // A core gone mid-turn: nothing read as it ended, the base has it.
        let mut e = entry(Provider::Codex);
        e.observed.spend.session = first;
        e.observed.begin_turn(Some(killed));
        assert_eq!(e.observed.spend.session, killed, "known as the turn starts");

        // Where the total stood cannot be told: no figure, not a wrong one.
        let mut e = entry(Provider::Codex);
        e.observed.spend = Spend { last_turn: Some(first), session: first };
        e.observed.begin_turn(None);
        on_codex_event(&mut e, &event(r#"{"type":"turn.completed","usage":{"input_tokens":50089,"cached_input_tokens":45312,"output_tokens":55}}"#));
        assert_eq!(e.observed.spend.last_turn, None);
        assert_eq!(e.observed.spend.session.output, 55);

        // A killed turn that spent nothing leaves the last one's figure.
        let mut e = entry(Provider::Codex);
        e.observed.spend = Spend { last_turn: Some(first), session: first };
        e.observed.begin_turn(Some(first));
        let generation = e.observed.generation;
        e.observed.apply_rollout(generation, None, Some(first));
        assert_eq!(e.observed.spend.last_turn, Some(first));
    }

    #[test]
    fn what_is_read_of_a_turn_gone_by_does_not_touch_the_next() {
        let mut o = Observed::default();
        o.begin_turn(None);
        let generation = o.generation;
        o.begin_turn(None);
        o.model = Some("rerouted-to".into());
        o.rerouted = true;
        o.apply_rollout(generation, Some("asked-for".into()), Some(codex_total(5, 0, 5)));
        assert_eq!(o.model.as_deref(), Some("rerouted-to"));
        assert!(o.rerouted, "its own lookup is its own to skip");
        assert_eq!(o.spend, Spend::default());
    }

    /// A Claude Code turn killed mid-way, as 2.1.316 left it: its message shown
    /// with usage, no `result`, and the next `modelUsage` without it.
    #[test]
    fn a_killed_claude_turn_is_counted_from_its_messages() {
        let message = |id: &str| json!({ "type": "assistant", "parent_tool_use_id": null, "message": { "id": id, "content": [],
            "usage": { "input_tokens": 2, "output_tokens": 16, "cache_read_input_tokens": 21390, "cache_creation_input_tokens": 7160 } } });
        let mut e = entry(Provider::Claude);
        e.observed.begin_turn(None);
        on_claude_event(&mut e, &message("msg_1"));
        on_claude_event(&mut e, &message("msg_1"));
        assert_eq!(e.observed.unreported.output, 16, "one message, however many blocks");
        on_claude_event(&mut e, &message("msg_2"));
        assert_eq!(e.observed.unreported.output, 32);

        // It ends with a result: that is the figure, nothing added.
        on_claude_event(&mut e, &event(CLAUDE_TURN_1));
        assert_eq!(e.observed.unreported, Usage::default());
        e.observed.settle_unreported();
        assert_eq!(e.observed.spend.session.output, 32, "the running total's 32, by chance the result's too");
        assert_eq!(e.observed.spend.session.input, 2);

        // Killed: its tokens count, and the next result adds to them.
        e.observed.begin_turn(None);
        on_claude_event(&mut e, &message("msg_3"));
        e.observed.settle_unreported();
        let s = e.observed.spend;
        assert_eq!(s.last_turn, Some(Usage { input: 2, output: 16, cache_read: 21390, cache_write: 7160, cost_usd: None }));
        assert_eq!((s.session.input, s.session.output), (4, 48));
        assert_eq!(s.session.cost_usd, Some(0.00289842), "its cost is stated nowhere");
        e.observed.begin_turn(None);
        on_claude_event(&mut e, &event(CLAUDE_TURN_2));
        let s = e.observed.spend;
        assert_eq!((s.session.input, s.session.output), (6, 52), "above the running total, which never had it");
        assert_eq!(s.session.cost_usd, Some(0.00329787));
    }

    #[test]
    fn a_failed_result_with_zeros_leaves_what_the_messages_showed() {
        let mut e = entry(Provider::Claude);
        on_claude_event(&mut e, &json!({ "type": "assistant", "parent_tool_use_id": null, "message": { "id": "m", "content": [],
            "usage": { "input_tokens": 3, "output_tokens": 7 } } }));
        on_claude_event(&mut e, &json!({ "type": "result", "subtype": "error_during_execution", "is_error": true, "total_cost_usd": 0,
            "usage": { "input_tokens": 0, "output_tokens": 0 }, "modelUsage": {} }));
        assert_eq!((e.observed.spend.session.input, e.observed.spend.session.output), (3, 7));
        assert_eq!(e.observed.unreported, Usage::default(), "not counted again");
    }

    #[test]
    fn a_core_gone_mid_turn_counts_what_its_messages_showed() {
        let spec = serde_json::to_value(&entry(Provider::Claude).spec).unwrap();
        let mut observed = Observed::default();
        observed.unreported.output = 9;
        let bytes = serde_json::to_value(&observed).unwrap();
        let e = taken_up(Saved {
            id: "c1".into(),
            spec: serde_json::from_value(spec).unwrap(),
            started_ms: 5,
            session: Some("sid".into()),
            cwd: None,
            turns_ended: 3,
            running: true,
            queued: false,
            cut_short: false,
            ties: Default::default(),
            observed: serde_json::from_value(bytes).unwrap(),
        });
        assert_eq!(e.observed.spend.session.output, 9);
        assert_eq!(e.observed.unreported, Usage::default());
    }

    #[test]
    fn a_reroute_names_the_model_of_its_turn_and_the_rollout_the_next() {
        assert_eq!(rerouted_to("model rerouted: gpt-a -> gpt-b (high risk)"), Some("gpt-b"));
        assert_eq!(rerouted_to("model rerouted: gpt-a -> gpt-b"), Some("gpt-b"));
        assert_eq!(rerouted_to("the model is overloaded"), None);

        let dir = std::env::temp_dir().join(format!("luna-reroute-{}", std::process::id()));
        let day = dir.join("sessions").join("2026").join("11").join("01");
        std::fs::create_dir_all(&day).unwrap();
        std::fs::write(
            day.join("rollout-2026-11-01T10-31-15-thread-reroute.jsonl"),
            format!("{}\n", r#"{"timestamp":"t","type":"turn_context","payload":{"model":"gpt-a"}}"#),
        )
        .unwrap();
        let mut h = entry(Provider::Codex);
        h.spec.account_path = dir.to_string_lossy().into_owned();
        h.spec.model = Some("gpt-asked".into());
        h.session = Some("thread-reroute".into());
        lock().entries.insert("reroute-1".into(), h);
        let model = || lock().entries.get("reroute-1").unwrap().observed.model.clone();

        assert_eq!(signs("reroute-1").unwrap().resolved_model, None, "before its first turn");
        turn_over("reroute-1", Some(0));
        assert_eq!(model().as_deref(), Some("gpt-a"), "the rollout, with no reroute");
        on_codex_event(
            lock().entries.get_mut("reroute-1").unwrap(),
            &json!({ "type": "item.completed", "item": { "id": "item_1", "type": "error", "message": "model rerouted: gpt-a -> gpt-b (high risk)" } }),
        );
        assert_eq!(model().as_deref(), Some("gpt-b"));
        turn_over("reroute-1", Some(0));
        assert_eq!(model().as_deref(), Some("gpt-b"), "the rollout does not take it back");
        turn_over("reroute-1", Some(0));
        assert_eq!(model().as_deref(), Some("gpt-a"), "the next turn asks again");
        assert!(lock().entries.get("reroute-1").unwrap().error.is_none(), "no failure");

        // A rollout that does not read: what it was asked to run on.
        {
            let mut r = lock();
            let e = r.entries.get_mut("reroute-1").unwrap();
            e.observed.model = None;
        }
        assert_eq!(signs("reroute-1").unwrap().resolved_model.as_deref(), Some("gpt-asked"));
        lock().entries.remove("reroute-1");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_model_is_the_one_the_cli_says_it_used() {
        let mut e = entry(Provider::Claude);
        on_claude_event(&mut e, &json!({ "type": "system", "subtype": "init", "session_id": "s", "model": "claude-haiku-5-5" }));
        assert_eq!(e.observed.model.as_deref(), Some("claude-haiku-5-5"));
        let said = |model: &str, parent: Value| json!({ "type": "assistant", "parent_tool_use_id": parent, "message": { "model": model, "content": [] } });
        on_claude_event(&mut e, &said("<synthetic>", Value::Null));
        assert_eq!(e.observed.model.as_deref(), Some("claude-haiku-5-5"), "no model wrote that");
        on_claude_event(&mut e, &said("claude-sonnet-5-5", json!("toolu_1")));
        assert_eq!(e.observed.model.as_deref(), Some("claude-haiku-5-5"), "a subagent's model is its own");
        on_claude_event(&mut e, &said("claude-sonnet-5-5", Value::Null));
        assert_eq!(e.observed.model.as_deref(), Some("claude-sonnet-5-5"), "a fallback shows");
    }

    #[test]
    fn spend_comes_back_with_the_helper_and_the_last_event_only_as_the_turn_end() {
        let mut o = Observed { model: Some("m".into()), turn_ended_ms: 5, event_ms: 9, ..Default::default() };
        o.spend.session.output = 7;
        let json = serde_json::to_value(&o).unwrap();
        assert!(json.get("eventMs").is_none(), "not written with every event");
        let back: Observed = serde_json::from_value(json).unwrap();
        assert_eq!((back.spend.session.output, back.model.as_deref(), back.turn_ended_ms, back.event_ms), (7, Some("m"), 5, 0));
        // helpers.json from before these fields reads as nothing spent.
        let bare: Observed = serde_json::from_value(json!({})).unwrap();
        assert_eq!(bare.spend, Spend::default());
    }

    #[test]
    fn signs_say_when_the_turn_began_and_what_it_was_last_seen_doing() {
        test_helper("signs-1", "C:\\acc", Some("s"), false);
        let s = signs("signs-1").unwrap();
        assert_eq!((s.turn_started_at.clone(), s.last_activity_at.clone(), s.spend), (None, None, None), "nothing known yet");
        let text = serde_json::to_string(&s).unwrap();
        assert_eq!(text, r#"{"turnStartedAt":null}"#, "null between turns, the rest left out");
        {
            let mut r = lock();
            let e = r.entries.get_mut("signs-1").unwrap();
            e.run = Some(Run { pid: None, started_ms: 1_793_000_000_000, stopped: false });
            e.observed.event_ms = 1_793_000_060_000;
        }
        let s = signs("signs-1").unwrap();
        assert_eq!(s.turn_started_at.as_deref(), iso(1_793_000_000_000).as_deref());
        assert_eq!(s.last_activity_at.as_deref(), iso(1_793_000_060_000).as_deref());
        assert!(s.turn_started_at.unwrap().ends_with('Z'));
        lock().entries.remove("signs-1");
    }

    pub(super) fn entry(provider: Provider) -> Entry {
        Entry {
            spec: Spec {
                provider,
                account_path: "C:\\acc".into(),
                folder: "C:\\p".into(),
                worktree: false,
                model: Some("haiku".into()),
                effort: "low".into(),
                mode: "read-only".into(),
                auto_review: false,
                tools: false,
                hooks_unasked: false,
                token: None,
            },
            started_ms: 0,
            session: None,
            cwd: None,
            run: None,
            turns_ended: 0,
            queue: Vec::new(),
            log: VecDeque::new(),
            held: VecDeque::new(),
            error: None,
            cut_short: false,
            job: None,
            left_running: false,
            observed: Observed::default(),
        }
    }

    /// An entry in a turn with no process yet, so nothing real can be killed.
    fn in_claimed_turn(id: &str) {
        let mut e = entry(Provider::Claude);
        claim(&mut e);
        lock().entries.insert(id.into(), e);
    }

    #[test]
    fn two_prompts_at_once_are_both_held_and_answered_in_order() {
        in_claimed_turn("two-asks");
        let ask = |tool: &'static str| {
            std::thread::spawn(move || permission("two-asks", &json!({ "tool_name": tool, "input": { "command": tool } })))
        };
        let held = || lock().entries.get("two-asks").unwrap().held.len();
        let until = |n: usize| {
            for _ in 0..200 {
                if held() == n {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            panic!("never {n} held");
        };
        let first = ask("Bash");
        until(1);
        let second = ask("Edit");
        until(2);
        let asked = super::ask("two-asks").unwrap();
        assert_eq!(asked.tool.as_deref(), Some("Bash"), "the one that came first");
        assert_eq!(turn_state("two-asks").unwrap().turn, crate::activity::Turn::Waiting);
        let stale = asked.id.clone().unwrap();
        answer("two-asks", Some(&stale), &[], Some(true), None).unwrap();
        reg().1.notify_all();
        assert_eq!(first.join().unwrap()["behavior"], "allow", "the second did not cost it its answer");
        let next = super::ask("two-asks").unwrap();
        assert_eq!(next.tool.as_deref(), Some("Edit"), "and then the next");
        assert_ne!(next.id, asked.id);
        assert_eq!(turn_state("two-asks").unwrap().turn, crate::activity::Turn::Waiting, "still held on that one");
        // A second responder who also saw the first, its answer late.
        let late = answer("two-asks", Some(&stale), &[], Some(true), None).unwrap_err();
        assert!(late.contains("no longer open"), "{late}");
        assert_eq!(super::ask("two-asks").unwrap().id, next.id, "the next is untouched");
        answer("two-asks", None, &[], Some(false), Some("not that")).unwrap();
        let no = second.join().unwrap();
        assert_eq!((no["behavior"].as_str(), no["message"].as_str()), (Some("deny"), Some("not that")));
        assert_eq!(super::ask("two-asks"), None);
        assert_eq!(turn_state("two-asks").unwrap().turn, crate::activity::Turn::Busy);
        lock().entries.remove("two-asks");
    }

    #[test]
    fn with_several_held_an_answer_says_which() {
        in_claimed_turn("which-ask");
        let ask = |tool: &'static str| {
            std::thread::spawn(move || permission("which-ask", &json!({ "tool_name": tool, "input": {} })))
        };
        let first = ask("Bash");
        for _ in 0..200 {
            if lock().entries.get("which-ask").unwrap().held.len() == 1 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let second = ask("Edit");
        for _ in 0..200 {
            if lock().entries.get("which-ask").unwrap().held.len() == 2 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let unsaid = answer("which-ask", None, &[], Some(true), None).unwrap_err();
        assert!(unsaid.contains("askId") && unsaid.contains("Bash"), "says how, and what is first: {unsaid}");
        assert!(lock().entries.get("which-ask").unwrap().held.iter().all(|h| h.answer.is_none()), "and answers nothing");
        // One named out of turn is the one it means.
        let last = lock().entries.get("which-ask").unwrap().held.back().unwrap().ask.id.clone().unwrap();
        answer("which-ask", Some(&last), &[], Some(false), None).unwrap();
        reg().1.notify_all();
        assert_eq!(second.join().unwrap()["behavior"], "deny");
        assert_eq!(super::ask("which-ask").unwrap().tool.as_deref(), Some("Bash"));
        // A single ask is answered with no id, as before.
        answer("which-ask", None, &[], Some(true), None).unwrap();
        reg().1.notify_all();
        assert_eq!(first.join().unwrap()["behavior"], "allow");
        assert!(answer("which-ask", None, &[], Some(true), None).unwrap_err().contains("not waiting"));
        lock().entries.remove("which-ask");
    }

    #[test]
    fn an_ask_says_its_id_to_whoever_is_to_answer_it() {
        in_claimed_turn("id-ask");
        let held = std::thread::spawn(|| permission("id-ask", &json!({ "tool_name": "Bash", "input": { "command": "ls" } })));
        for _ in 0..200 {
            if super::ask("id-ask").is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let shown = serde_json::to_value(super::ask("id-ask").unwrap()).unwrap();
        assert!(shown["id"].as_str().is_some_and(|i| !i.is_empty()), "{shown}");
        kill("id-ask");
        let _ = held.join();
        lock().entries.remove("id-ask");
        assert!(serde_json::to_value(Ask::permission(None, None)).unwrap().get("id").is_none(), "a terminal's has none");
    }

    #[test]
    fn a_kill_lets_every_held_prompt_go() {
        in_claimed_turn("kill-asks");
        let ask = |tool: &'static str| {
            std::thread::spawn(move || permission("kill-asks", &json!({ "tool_name": tool, "input": {} })))
        };
        let (a, b) = (ask("Bash"), ask("Edit"));
        for _ in 0..200 {
            if lock().entries.get("kill-asks").unwrap().held.len() == 2 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        kill("kill-asks");
        assert_eq!(a.join().unwrap()["behavior"], "deny");
        assert_eq!(b.join().unwrap()["behavior"], "deny");
        lock().entries.remove("kill-asks");
    }

    #[test]
    fn an_interrupt_keeps_the_queue_and_counts_a_turn_with_no_process_yet() {
        in_claimed_turn("int-claimed");
        lock().entries.get_mut("int-claimed").unwrap().queue.push("from another caller".into());
        // No process turns up to stop: the text waits behind the turn, which
        // it ends a turn after, and nothing is lost.
        let i = interrupt("int-claimed", "from this one").unwrap();
        assert_eq!(i, Interrupted { stopped: false, queued: true });
        assert_eq!(lock().entries.get("int-claimed").unwrap().queue, ["from another caller", "from this one"]);
        // When that turn is over, they go together, in the order they came,
        // and an empty queue in the meantime says the text has been taken up.
        let text = std::mem::take(&mut lock().entries.get_mut("int-claimed").unwrap().queue).join("\n\n");
        assert_eq!(text, "from another caller\n\nfrom this one");
        lock().entries.remove("int-claimed");
    }

    #[test]
    fn notes_are_open_to_a_helper_with_tools() {
        let mut e = entry(Provider::Claude);
        let has_dir = |e: &Entry| command_for(e, "x").0.iter().any(|a| a == "--add-dir");
        assert!(!has_dir(&e), "one with no tools gets no notices");
        e.spec.tools = true;
        assert!(has_dir(&e));
        let (args, _, _) = command_for(&e, "x");
        let at = args.iter().position(|a| a == "--add-dir").unwrap();
        assert_eq!(args[at + 1], crate::agents::notes_dir().to_string_lossy());
        let mut codex = entry(Provider::Codex);
        codex.spec.tools = true;
        assert!(!has_dir(&codex), "Codex's sandboxes read anywhere");
    }

    #[test]
    fn a_job_left_running_reads_as_one_between_turns_only() {
        let mut e = entry(Provider::Claude);
        e.turns_ended = 1;
        e.left_running = true;
        lock().entries.insert("left-job".into(), e);
        let children_busy = || turn_state("left-job").unwrap().children_busy;
        assert!(children_busy(), "between turns, what its turn left is a job");
        lock().entries.get_mut("left-job").unwrap().run = Some(Run { pid: Some(1), started_ms: 0, stopped: false });
        assert!(!children_busy(), "in a turn it is simply working");
        lock().entries.remove("left-job");
    }

    #[test]
    fn a_helper_saved_mid_turn_comes_back_between_turns_with_that_turn_ended() {
        let mut spec = entry(Provider::Claude).spec;
        spec.token = Some("secret".into());
        let json = serde_json::to_value(&spec).unwrap();
        assert!(json.get("token").is_none(), "a token is never saved");
        let saved = |running, queued| Saved {
            id: "c1".into(),
            spec: serde_json::from_value(json.clone()).unwrap(),
            started_ms: 5,
            session: Some("sid".into()),
            cwd: None,
            turns_ended: 3,
            running,
            queued,
            cut_short: false,
            ties: Default::default(),
            observed: Default::default(),
        };

        let e = taken_up(saved(true, true));
        assert!(e.run.is_none() && e.spec.token.is_none());
        assert_eq!(e.turns_ended, 5, "the turn stopped and the text queued behind it");
        assert!(e.cut_short && e.error.is_some());
        assert_eq!(e.session.as_deref(), Some("sid"));

        let e = taken_up(saved(false, false));
        assert_eq!(e.turns_ended, 3);
        assert!(!e.cut_short && e.error.is_none());
    }

    #[test]
    fn a_turn_resumes_the_conversation_the_first_one_named() {
        let mut e = entry(Provider::Codex);
        let (args, dir, _) = command_for(&e, "x");
        assert_eq!(args[..3], ["exec", "-C", "C:\\p"]);
        assert_eq!(args.last().map(String::as_str), Some("-"), "the prompt comes on stdin");
        assert!(args.contains(&"sandbox_mode=\"read-only\"".to_string()));
        assert_eq!(dir, "C:\\p");
        on_codex_event(&mut e, &json!({ "type": "thread.started", "thread_id": "t-1" }));
        let (args, _, _) = command_for(&e, "x");
        assert_eq!(args[..3], ["exec", "resume", "t-1"]);

        let mut e = entry(Provider::Claude);
        on_claude_event(&mut e, &json!({ "type": "system", "subtype": "init", "session_id": "s-1", "cwd": "C:\\p" }));
        let (args, _, envs) = command_for(&e, "x");
        assert_eq!(args[..2], ["-p", "--output-format"]);
        let at = args.iter().position(|a| a == "--resume").unwrap();
        assert_eq!(args[at + 1], "s-1");
        assert!(envs.iter().any(|(k, v)| *k == "CLAUDE_CONFIG_DIR" && v == "C:\\acc"));
    }

    #[test]
    fn auto_review_is_codex_own_flag_without_a_policy_or_sandbox() {
        let mut e = entry(Provider::Codex);
        e.spec.mode = "workspace-write".into();
        e.spec.auto_review = true;
        let (args, _, _) = command_for(&e, "x");
        assert!(args.contains(&"--approve-for-me".to_string()));
        assert!(!args.iter().any(|a| a.starts_with("approval_policy=") || a.starts_with("sandbox_mode=")), "Codex refuses them beside it");
    }

    #[test]
    fn hooks_run_unasked_only_when_the_folder_says_so() {
        let mut e = entry(Provider::Codex);
        let (args, _, _) = command_for(&e, "x");
        assert!(!args.contains(&crate::pty::HOOKS_UNASKED.to_string()));
        e.spec.hooks_unasked = true;
        on_codex_event(&mut e, &json!({ "type": "thread.started", "thread_id": "t-1" }));
        let (args, _, _) = command_for(&e, "x");
        assert_eq!(args[..3], ["exec", "resume", "t-1"]);
        assert!(args.contains(&crate::pty::HOOKS_UNASKED.to_string()), "every turn, resumes too");
        assert!(args.iter().any(|a| a.starts_with("projects={") && a.contains("trust_level")), "trusted for the run");
    }

    #[test]
    fn a_failed_turn_says_why() {
        let mut e = entry(Provider::Claude);
        on_claude_event(&mut e, &json!({ "type": "result", "subtype": "error_during_execution", "is_error": true, "result": "rate limited" }));
        assert_eq!(e.error.as_deref(), Some("rate limited"));
        let mut e = entry(Provider::Codex);
        on_codex_event(&mut e, &json!({ "type": "turn.failed", "error": { "message": "boom" } }));
        assert_eq!(e.error.as_deref(), Some("boom"));
        assert!(e.log.back().unwrap().contains("boom"));
    }

    /// A pipe its writer has left: what was written, then a read that blocks
    /// for as long as something else holds it open, as a stray job does.
    struct Pipe(std::sync::mpsc::Receiver<Vec<u8>>);

    impl std::io::Read for Pipe {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let Ok(bytes) = self.0.recv() else { return Ok(0) };
            buf[..bytes.len()].copy_from_slice(&bytes);
            Ok(bytes.len())
        }
    }

    /// Reads `lines` off such a pipe, each event taking `each` to handle, and
    /// returns once `await_last_words` does: what was handled by then.
    fn drained_with_a_stray(lines: &[&str], each: std::time::Duration) -> Vec<String> {
        use std::sync::{Arc, Mutex};
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        for line in lines {
            tx.send(format!("{line}\n").into_bytes()).unwrap();
        }
        let reader = Arc::new(OutReader::default());
        let got = Arc::new(Mutex::new(Vec::new()));
        let (drained, read_out) = std::sync::mpsc::channel::<()>();
        let (on_it, into) = (reader.clone(), got.clone());
        std::thread::spawn(move || {
            let _drained = drained;
            on_it.read_events(Pipe(rx), |v| {
                std::thread::sleep(each);
                into.lock().unwrap().push(v["type"].as_str().unwrap_or("").to_string());
                true
            });
        });
        reader.await_last_words(&read_out, std::time::Duration::from_millis(150), std::time::Duration::from_secs(20));
        let seen = got.lock().unwrap().clone();
        drop(tx); // Only now does the stray let go.
        seen
    }

    #[test]
    fn a_turns_last_events_are_read_with_a_stray_holding_the_pipe() {
        let events = [r#"{"type":"assistant"}"#, "not json", r#"{"type":"result"}"#];
        let started = std::time::Instant::now();
        assert_eq!(drained_with_a_stray(&events, std::time::Duration::ZERO), ["assistant", "result"]);
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "and the stray is not waited out");
    }

    #[test]
    fn however_slowly_the_last_events_are_taken_none_is_left_behind() {
        // Each behind a stalled lock: more than a turn's end used to give them.
        let events = [r#"{"type":"one"}"#, r#"{"type":"two"}"#, r#"{"type":"result"}"#];
        assert_eq!(drained_with_a_stray(&events, std::time::Duration::from_millis(900)), ["one", "two", "result"]);
    }

    #[test]
    fn a_pipe_that_closes_ends_the_wait_at_once_and_a_last_line_counts_without_its_end() {
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        tx.send(br#"{"type":"a"}"#.to_vec()).unwrap();
        tx.send(b"\n{\"type\":\"b\"}\n{\"ty".to_vec()).unwrap();
        tx.send(br#"pe":"c"}"#.to_vec()).unwrap();
        drop(tx);
        let reader = OutReader::default();
        let mut got = Vec::new();
        reader.read_events(Pipe(rx), |v| {
            got.push(v["type"].as_str().unwrap_or("").to_string());
            true
        });
        assert_eq!(got, ["a", "b", "c"], "split across reads, and the last with no line end");
        let (drained, read_out) = std::sync::mpsc::channel::<()>();
        drop(drained);
        let started = std::time::Instant::now();
        reader.await_last_words(&read_out, std::time::Duration::from_secs(9), std::time::Duration::from_secs(9));
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn an_event_of_a_turn_over_lands_in_nothing() {
        let id = "ev-after";
        lock().entries.insert(id.into(), entry(Provider::Codex));
        let reader = OutReader::default();
        let failed = json!({ "type": "turn.failed", "error": { "message": "boom" } });
        assert!(take_event(id, Provider::Codex, &reader, &failed));
        assert_eq!(last_error(id).as_deref(), Some("boom"));
        lock().entries.get_mut(id).unwrap().error = None;
        // The turn is over and the next may be on: this one's words stay out of it.
        reader.over.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(take_event(id, Provider::Codex, &reader, &failed), "it reads on, to keep the pipe drained");
        assert_eq!(last_error(id), None);
        lock().entries.remove(id);
        let reader = OutReader::default();
        assert!(!take_event(id, Provider::Codex, &reader, &failed), "the helper is gone: no more to read for");
    }

    #[test]
    fn a_retry_codex_got_through_is_no_failure() {
        // As `codex exec --json` 0.166 words them on a connection it retries.
        let retry = json!({ "type": "error", "message": "Reconnecting... 2/5 (unexpected status 401 Unauthorized)" });
        let mut e = entry(Provider::Codex);
        on_codex_event(&mut e, &retry);
        assert_eq!(e.error, None, "said in its log, and not a failure");
        assert!(e.log.back().unwrap().contains("Reconnecting"));
        on_codex_event(&mut e, &json!({ "type": "turn.completed" }));
        assert_eq!(e.error, None);
        // One that does not read as a retry, followed by a turn that completes.
        on_codex_event(&mut e, &json!({ "type": "error", "message": "stream hiccup" }));
        assert_eq!(e.error.as_deref(), Some("stream hiccup"));
        on_codex_event(&mut e, &json!({ "type": "turn.completed" }));
        assert_eq!(e.error, None, "the turn got through after all");
        // The retries run out.
        on_codex_event(&mut e, &retry);
        on_codex_event(&mut e, &json!({ "type": "error", "message": "unexpected status 401 Unauthorized" }));
        on_codex_event(&mut e, &json!({ "type": "turn.failed", "error": { "message": "unexpected status 401 Unauthorized" } }));
        assert_eq!(e.error.as_deref(), Some("unexpected status 401 Unauthorized"));
    }

    #[test]
    fn a_turn_killed_or_crashed_ends_on_an_error() {
        let run = |e: &mut Entry| e.run = Some(Run { pid: Some(1), started_ms: 0, stopped: false });
        for (id, code, why) in [
            ("end-crash", Some(1), "codex exited with code 1"),
            ("end-signal", None, "codex was killed"),
        ] {
            let mut e = entry(Provider::Codex);
            run(&mut e);
            lock().entries.insert(id.into(), e);
            turn_over(id, code);
            assert_eq!(last_error(id).as_deref(), Some(why));
            assert_eq!(turn_state(id).unwrap().turns_ended, 1, "it still ends the turn");
            lock().entries.remove(id);
        }
        // Stopped on purpose, it says so rather than the code that leaves.
        let mut e = entry(Provider::Claude);
        run(&mut e);
        e.run.as_mut().unwrap().pid = None;
        lock().entries.insert("end-killed".into(), e);
        kill("end-killed");
        turn_over("end-killed", Some(1));
        assert_eq!(last_error("end-killed").as_deref(), Some("it was stopped, by luna_kill or from the window"));
        lock().entries.remove("end-killed");
        // A clean exit says nothing.
        let mut e = entry(Provider::Claude);
        run(&mut e);
        lock().entries.insert("end-clean".into(), e);
        turn_over("end-clean", Some(0));
        assert_eq!(last_error("end-clean"), None);
        lock().entries.remove("end-clean");
    }

    #[test]
    fn an_answer_is_shaped_as_the_cli_takes_it() {
        let input = json!({ "questions": [{ "question": "Which colour?", "options": [{ "label": "Red" }, { "label": "Blue" }] }] });
        let ask = Ask::from_claude_questions(&input).unwrap();
        let a = build_answer(&ask, &input, &["2".into()], None, None).unwrap();
        assert_eq!(a["behavior"], "allow");
        assert_eq!(a["updatedInput"]["answers"]["Which colour?"], "Blue");
        assert_eq!(a["updatedInput"]["questions"], input["questions"], "the input goes back with the answers added");
        assert!(build_answer(&ask, &input, &[], None, None).is_err(), "one answer per question");
        let own = build_answer(&ask, &input, &["green, actually".into()], None, None).unwrap();
        assert_eq!(own["updatedInput"]["answers"]["Which colour?"], "green, actually");

        let cmd = json!({ "command": "rm -rf build" });
        let perm = Ask::from_claude_tool("Bash", &cmd);
        assert_eq!(build_answer(&perm, &cmd, &[], Some(true), None).unwrap(), json!({ "behavior": "allow", "updatedInput": cmd }));
        let no = build_answer(&perm, &cmd, &[], Some(false), Some("not now")).unwrap();
        assert_eq!((no["behavior"].as_str(), no["message"].as_str()), (Some("deny"), Some("not now")));
        assert!(build_answer(&perm, &cmd, &[], None, None).is_err());
    }
}
