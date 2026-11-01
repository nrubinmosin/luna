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
    /// When it last printed anything.
    #[serde(skip)]
    event_ms: u64,
    /// Codex has said this turn runs on another model than it asked for.
    #[serde(skip)]
    rerouted: bool,
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
    held: Option<Held>,
    /// The CLI could not run or quit on an error before saying anything.
    error: Option<String>,
    /// Its last turn was stopped by the core going, not ended by the CLI:
    /// what the caller is told of it says so.
    cut_short: bool,
    /// The job every turn's process is put in, the first one's made for it:
    /// what a turn leaves running is still the helper's between turns, holds
    /// the machine as a chat's would, and is ended with the helper.
    job: Option<std::sync::Arc<crate::procs::Job>>,
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
    e.held = None;
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
        held: None,
        error: None,
        cut_short: false,
        job: None,
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

fn spawn_turn(id: &str, prompt: &str) -> Result<(), String> {
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
                // Read to its end as bytes, as stderr below is.
                if let Some(out) = stdout {
                    for line in BufReader::new(out).split(b'\n').map_while(Result::ok) {
                        let Ok(v) = serde_json::from_slice::<Value>(&line) else { continue };
                        let mut r = lock();
                        let Some(e) = r.entries.get_mut(&id) else { break };
                        e.observed.event_ms = crate::throttle::now_ms();
                        match provider {
                            Provider::Claude => on_claude_event(e, &v),
                            Provider::Codex => on_codex_event(e, &v),
                        }
                    }
                }
                let code = child.wait().ok().and_then(|s| s.code());
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
        let n = |k: &str| u[k].as_u64().unwrap_or(0);
        u.is_object().then(|| Usage {
            input: n("input_tokens"),
            output: n("output_tokens"),
            cache_read: n("cache_read_input_tokens"),
            cache_write: n("cache_creation_input_tokens"),
            cost_usd: running_cost.map(|c| usd((c - spent.cost_usd.unwrap_or(0.0)).max(0.0))),
        })
    });
    let Some(turn) = turn else { return };
    let mut running = Usage::of_claude_models(&v["modelUsage"]).unwrap_or_default();
    running.cost_usd = running_cost.or(running.cost_usd).map(usd);
    e.observed.spend = Spend { last_turn: Some(turn), session: spent.plus(turn).at_least(running) };
}

/// What `turn.completed` says: its usage is the thread's running total, resumes
/// included, so the turn is what it rose by, measured from the total saved with
/// the helper across a core. A helper past its first turn with no total to
/// measure from (saved before Luna kept count) gets none for this turn: the
/// figure is the thread's, not the turn's.
fn codex_spend(e: &mut Entry, usage: &Value) {
    if !usage.is_object() {
        return;
    }
    let total = Usage::of_codex(usage);
    let spend = e.observed.spend;
    let known = spend.session != Usage::default() || e.turns_ended == 0;
    let last_turn = if known { Some(total.since(spend.session)) } else { spend.last_turn };
    e.observed.spend = Spend { last_turn, session: total.at_least(spend.session) };
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
            e.error = Some(why);
        }
        Some("turn.completed") => {
            codex_spend(e, &v["usage"]);
            note(e, "turn done");
        }
        _ => {}
    }
}

/// The turn's process has exited: count the turn, let any held question go,
/// and start the turn the queue has waiting.
fn turn_over(id: &str, code: Option<i32>) {
    let (next, thread) = {
        let mut r = lock();
        let Some(e) = r.entries.get_mut(id) else { return };
        e.run = None;
        e.turns_ended += 1;
        e.observed.turn_ended_ms = crate::throttle::now_ms();
        e.held = None;
        if code.is_some_and(|c| c != 0) && e.error.is_none() {
            e.error = Some(format!("{} exited with code {}", e.spec.provider.as_str(), code.unwrap_or(-1)));
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
        let rerouted = std::mem::take(&mut e.observed.rerouted);
        let thread = e
            .session
            .clone()
            .filter(|_| e.spec.provider == Provider::Codex && !rerouted)
            .map(|t| (e.spec.account_path.clone(), t));
        (next, thread)
    };
    reg().1.notify_all();
    crate::log::info("background", &format!("{} turn over, code {code:?}", crate::agents::label(id)));
    // Codex's events name no model but a reroute's, which stands for its turn;
    // otherwise its rollout says what the turn ran on, read with the lock let go.
    let model = thread.and_then(|(account, t)| crate::codex::session::last_model(&crate::codex::session::rollout_of_thread(&account, &t)?));
    if let (Some(model), Some(e)) = (model, lock().entries.get_mut(id)) {
        e.observed.model = Some(model);
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

fn one_line(text: &str) -> String {
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
    let turn = match (&e.run, &e.held) {
        (Some(_), Some(_)) => Turn::Waiting,
        (Some(_), None) => Turn::Busy,
        _ => Turn::Idle,
    };
    Some(crate::activity::TurnState { turn, busy: e.run.is_some(), turns_ended: e.turns_ended, children_busy: false })
}

/// What the power rules see of every helper: its turn, and between turns
/// what its turns left running, by the same rule as a chat's (activity.rs):
/// it holds the machine for as long as it lives.
pub fn activity() -> Vec<crate::activity::SessionActivity> {
    let ids: Vec<(String, Option<std::sync::Arc<crate::procs::Job>>)> = lock()
        .entries
        .iter()
        .map(|(id, e)| (id.clone(), e.job.clone().filter(|_| e.run.is_none())))
        .collect();
    ids.into_iter()
        .filter_map(|(id, job)| {
            let t = turn_state(&id)?;
            let mut a = crate::activity::SessionActivity::of_turn(&id, t.turn, t.busy);
            if let Some(job) = job {
                let left = crate::procs::left_running(&job);
                if !left.is_empty() {
                    a.busy = true;
                    a.procs = left;
                }
            }
            Some(a)
        })
        .collect()
}

/// Text for the helper: the next turn now, or after the one running.
/// Returns whether it was queued.
pub fn send(id: &str, text: &str) -> Result<bool, String> {
    let queued = {
        let mut r = lock();
        let e = r.entries.get_mut(id).ok_or("no such helper")?;
        if e.held.is_some() {
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
        stop(e)
    };
    reg().1.notify_all();
    crate::pty::kill_tree(pid);
}

/// Stops the turn in progress and starts the next on `text`, with whatever
/// was queued for after it going first: the conversation goes on, the work
/// the turn was doing does not. Returns whether a turn was stopped.
pub fn interrupt(id: &str, text: &str) -> Result<bool, String> {
    let (pid, queued) = {
        let mut r = lock();
        let e = r.entries.get_mut(id).ok_or("no such helper")?;
        let queued = std::mem::take(&mut e.queue);
        e.held = None;
        (e.run.as_ref().and_then(|run| run.pid), queued)
    };
    let stopped = pid.is_some();
    if stopped {
        reg().1.notify_all();
        crate::pty::kill_tree(pid);
    }
    let text = queued.into_iter().chain([text.to_string()]).collect::<Vec<_>>().join("\n\n");
    {
        // The turn is over once its reader has seen the process go, and the
        // run is then free or taken by a turn queued before the kill.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        let (m, cv) = reg();
        let mut r = m.lock().unwrap_or_else(|e| e.into_inner());
        while stopped && r.entries.get(id).and_then(|e| e.run.as_ref()).is_some_and(|run| run.pid == pid) {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return Err(format!("helper {id} did not stop within 15 s"));
            }
            r = cv.wait_timeout(r, left).unwrap_or_else(|e| e.into_inner()).0;
        }
        let e = r.entries.get_mut(id).ok_or("no such helper")?;
        note(e, "interrupted");
        // A turn something else started meanwhile: the text goes after it.
        if e.run.is_some() {
            e.queue.push(text);
            drop(r);
            changed();
            return Ok(stopped);
        }
        claim(e);
    }
    run_turn(id, &text)?;
    changed();
    Ok(stopped)
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
    lock().entries.get(id)?.held.as_ref().map(|h| h.ask.clone())
}

/// `luna_permission`: the CLI asks before a tool runs, or carries the model's
/// `AskUserQuestion`. Held until `luna_answer`, the turn's end or the timeout;
/// the answer is the JSON the CLI takes back.
pub fn permission(id: &str, args: &Value) -> Value {
    let tool = args["tool_name"].as_str().unwrap_or("").to_string();
    let input = args["input"].clone();
    let ask = if tool == "AskUserQuestion" {
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
        note(e, &format!("waits: {}", ask.line()));
        e.held = Some(Held { serial, ask, input, answer: None });
        serial
    };
    changed();
    let deadline = std::time::Instant::now() + ASK_TIMEOUT;
    let (m, cv) = reg();
    let mut r = m.lock().unwrap_or_else(|e| e.into_inner());
    let answer = loop {
        match r.entries.get_mut(id).map(|e| (e.run.is_some(), e.held.as_mut())) {
            Some((true, Some(h))) if h.serial == serial => {
                if let Some(a) = h.answer.take() {
                    break Some(a);
                }
            }
            // Killed, deleted, or its turn is gone: nobody will answer.
            _ => break None,
        }
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            break None;
        }
        r = cv.wait_timeout(r, left).unwrap_or_else(|e| e.into_inner()).0;
    };
    if let Some(e) = r.entries.get_mut(id) {
        if e.held.as_ref().is_some_and(|h| h.serial == serial) {
            e.held = None;
        }
    }
    drop(r);
    changed();
    answer.unwrap_or_else(|| json!({ "behavior": "deny", "message": "Nobody answered in time; go on without it or ask in your reply." }))
}

/// The agent's answer to what the helper holds: `answers` for a question
/// (an option's number or label, or words of its own, one per question;
/// several numbers apart by commas where more than one may be picked),
/// `allow` for a permission, `note` as the reason for a refusal.
pub fn answer(id: &str, answers: &[String], allow: Option<bool>, note_text: Option<&str>) -> Result<(), String> {
    let mut r = lock();
    let e = r.entries.get_mut(id).ok_or("no such helper")?;
    let h = e.held.as_mut().ok_or_else(|| format!("helper {id} is not waiting on anything"))?;
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
pub fn background_delete(id: String) -> Result<(), String> {
    crate::agents::delete_by_user(&id)
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
        held: None,
        error: None,
        cut_short: s.cut_short || s.running,
        job: None,
        observed: s.observed,
    };
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
        held: None,
        error: None,
        cut_short: false,
        job: None,
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
        on_codex_event(&mut e, &event(CODEX_TURN_1));
        assert_eq!(e.observed.spend.last_turn, Some(Usage { input: 3099, output: 5, cache_read: 12416, cache_write: 0, cost_usd: None }));
        on_codex_event(&mut e, &event(CODEX_TURN_2));
        let s = e.observed.spend;
        assert_eq!(s.last_turn, Some(Usage { input: 1502, output: 5, cache_read: 15360, cache_write: 0, cost_usd: None }), "as the rollout's last_token_usage has it");
        assert_eq!(s.session, Usage { input: 4601, output: 10, cache_read: 27776, cache_write: 0, cost_usd: None });
        assert!(!serde_json::to_string(&s).unwrap().contains("costUsd"), "Codex states no cost");
    }

    #[test]
    fn a_codex_total_is_measured_from_what_came_back_with_the_helper() {
        // The first turn after a core restart: its total runs on from the saved one.
        let mut e = entry(Provider::Codex);
        e.turns_ended = 1;
        on_codex_event(&mut e, &event(CODEX_TURN_1));
        let saved = serde_json::to_value(&e.observed).unwrap();

        let mut e = entry(Provider::Codex);
        e.turns_ended = 1;
        e.observed = serde_json::from_value(saved).unwrap();
        on_codex_event(&mut e, &event(CODEX_TURN_2));
        assert_eq!(e.observed.spend.last_turn.map(|t| (t.input, t.cache_read, t.output)), Some((1502, 15360, 5)));
        assert_eq!(e.observed.spend.session.output, 10);

        // Nothing saved and past its first turn: the total is no turn's.
        let mut e = entry(Provider::Codex);
        e.turns_ended = 3;
        on_codex_event(&mut e, &event(CODEX_TURN_2));
        assert_eq!(e.observed.spend.last_turn, None);
        assert_eq!(e.observed.spend.session.input, 4601);
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

    fn entry(provider: Provider) -> Entry {
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
            held: None,
            error: None,
            cut_short: false,
            job: None,
            observed: Observed::default(),
        }
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
