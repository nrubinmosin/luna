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
use serde::Serialize;
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
#[derive(Clone)]
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
    /// `luna_permission` for every Claude Code helper.
    pub token: Option<String>,
}

struct Run {
    pid: Option<u32>,
    /// When the turn started, for the window's "working 4 min".
    started_ms: u64,
    /// Holds the job the turn's process tree lives in, for as long as it runs.
    _job: Option<crate::procs::Job>,
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
}

/// Stops every turn in progress with its process tree and drops what was
/// queued; `close` also keeps any from starting after. Returns how many it
/// stopped.
pub fn stop_turns(close: bool) -> usize {
    let pids: Vec<u32> = {
        let mut r = lock();
        r.closed |= close;
        r.entries
            .values_mut()
            .filter_map(|e| {
                e.queue.clear();
                e.held = None;
                e.run.as_ref().and_then(|run| run.pid)
            })
            .collect()
    };
    reg().1.notify_all();
    let stopped = pids.len();
    for pid in pids {
        crate::pty::kill_tree(Some(pid));
    }
    stopped
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
    e.run = Some(Run { pid: None, started_ms: crate::throttle::now_ms(), _job: None });
}

/// Starts the next turn's process with `prompt` on its stdin, on a run
/// `claim` has taken; a turn that could not start gives the claim back.
/// Text queued behind it meanwhile would wait for a `turn_over` that never
/// comes: it fails with it, counted as a turn that ended on the error, so
/// whoever waits on it hears — rather than starting what would most likely
/// fail the same way.
fn run_turn(id: &str, prompt: &str) -> Result<(), String> {
    let started = spawn_turn(id, prompt);
    if let Err(err) = &started {
        if let Some(e) = lock().entries.get_mut(id) {
            if e.run.as_ref().is_some_and(|r| r.pid.is_none()) {
                e.run = None;
                if !e.queue.is_empty() {
                    e.queue.clear();
                    note(e, &format!("could not start the queued turn: {err}"));
                    e.error = Some(err.clone());
                    e.turns_ended += 1;
                }
            }
        }
        reg().1.notify_all();
        changed();
    }
    started
}

fn spawn_turn(id: &str, prompt: &str) -> Result<(), String> {
    let (args, dir, envs, provider) = {
        let r = lock();
        if r.closed {
            return Err("Luna is stopping".into());
        }
        let e = r.entries.get(id).ok_or("no such helper")?;
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
    let mut child = cmd.spawn().map_err(|e| {
        crate::log::error("background", &format!("could not start {id}: {e}"));
        format!("could not start {}: {e}", provider.as_str())
    })?;
    let pid = Some(child.id());
    let job = pid.and_then(crate::procs::adopt);
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    {
        let mut r = lock();
        let closed = r.closed;
        let Some(e) = r.entries.get_mut(id).filter(|_| !closed) else {
            drop(r);
            crate::pty::kill_tree(pid);
            let _ = child.kill();
            let _ = child.wait();
            return Err(if closed { "Luna is stopping" } else { "the helper was deleted" }.into());
        };
        e.run = Some(Run { pid, started_ms: crate::throttle::now_ms(), _job: job });
        e.error = None;
        note(e, &format!("› {}", one_line(prompt)));
    }
    crate::log::info("background", &format!("{id} turn started, pid {pid:?}: {} {}", provider.as_str(), args.join(" ")));
    // Written whole and closed: the CLI reads its prompt to the end of input.
    if let Some(mut stdin) = stdin {
        let prompt = prompt.to_string();
        std::thread::spawn(move || {
            let _ = stdin.write_all(prompt.as_bytes());
        });
    }
    // Both pipes are read to their end as bytes: `lines()` stops at the first
    // line that is not UTF-8 — a hook's output in the console's code page —
    // and a pipe nobody drains any more blocks the CLI once it fills.
    if let Some(err) = stderr {
        let id = id.to_string();
        std::thread::spawn(move || {
            for line in BufReader::new(err).split(b'\n').map_while(Result::ok) {
                let line = String::from_utf8_lossy(&line);
                let line = line.trim();
                if line.is_empty() || line.starts_with("Reading additional input") {
                    continue;
                }
                if let Some(e) = lock().entries.get_mut(&id) {
                    note(e, &format!("stderr: {}", one_line(line)));
                }
            }
        });
    }
    let id = id.to_string();
    std::thread::Builder::new()
        .name(format!("luna-bg-{id}"))
        .spawn(move || {
            if let Some(out) = stdout {
                for line in BufReader::new(out).split(b'\n').map_while(Result::ok) {
                    let Ok(v) = serde_json::from_slice::<Value>(&line) else { continue };
                    let mut r = lock();
                    let Some(e) = r.entries.get_mut(&id) else { break };
                    match provider {
                        Provider::Claude => on_claude_event(e, &v),
                        Provider::Codex => on_codex_event(e, &v),
                    }
                }
            }
            let code = child.wait().ok().and_then(|s| s.code());
            turn_over(&id, code);
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn on_claude_event(e: &mut Entry, v: &Value) {
    match (v["type"].as_str(), v["subtype"].as_str()) {
        (Some("system"), Some("init")) => {
            e.session = v["session_id"].as_str().map(str::to_owned).or(e.session.take());
            e.cwd = v["cwd"].as_str().map(str::to_owned).or(e.cwd.take());
        }
        (Some("assistant"), _) if v["parent_tool_use_id"].is_null() => {
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
            Some(other) if other != "reasoning" => note(e, other),
            _ => {}
        },
        Some("turn.failed") | Some("error") => {
            let why = v["error"]["message"].as_str().or(v["message"].as_str()).unwrap_or("error").to_string();
            note(e, &format!("error: {}", one_line(&why)));
            e.error = Some(why);
        }
        Some("turn.completed") => note(e, "turn done"),
        _ => {}
    }
}

/// The turn's process has exited: count the turn, let any held question go,
/// and start the turn the queue has waiting.
fn turn_over(id: &str, code: Option<i32>) {
    let next = {
        let mut r = lock();
        let Some(e) = r.entries.get_mut(id) else { return };
        e.run = None;
        e.turns_ended += 1;
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
        next
    };
    reg().1.notify_all();
    crate::log::info("background", &format!("{id} turn over, code {code:?}"));
    if let Some(text) = next {
        if let Err(err) = run_turn(id, &text) {
            if let Some(e) = lock().entries.get_mut(id) {
                note(e, &format!("could not start the queued turn: {err}"));
                e.error = Some(err);
            }
        }
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

/// What the power rules see of every helper.
pub fn activity() -> Vec<crate::activity::SessionActivity> {
    let ids: Vec<String> = lock().entries.keys().cloned().collect();
    ids.into_iter()
        .filter_map(|id| {
            let t = turn_state(&id)?;
            Some(crate::activity::SessionActivity::of_turn(&id, t.turn, t.busy))
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
        e.queue.clear();
        e.held = None;
        e.run.as_ref().and_then(|run| run.pid)
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

#[cfg(test)]
mod tests {
    use super::*;

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
        }
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
