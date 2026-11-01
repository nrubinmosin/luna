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
/// How long the CLI's last events get to be read once its process has gone, for
/// a pipe that something it left running still holds open.
const LAST_WORDS: std::time::Duration = std::time::Duration::from_secs(2);

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
                // Read to its end as bytes, as stderr below is, on a thread of
                // its own: the turn is over when the CLI is, and a job it left
                // running can have inherited this pipe and hold it open for as
                // long as it runs. Nothing it might write there is the CLI's.
                let over = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                let (drained, read_out) = std::sync::mpsc::channel::<()>();
                if let Some(out) = stdout {
                    let (helper, over) = (id.clone(), over.clone());
                    let read = std::thread::Builder::new().name(format!("luna-bg-out-{id}")).spawn(move || {
                        let _drained = drained;
                        for line in BufReader::new(out).split(b'\n').map_while(Result::ok) {
                            if over.load(std::sync::atomic::Ordering::SeqCst) {
                                continue;
                            }
                            let Ok(v) = serde_json::from_slice::<Value>(&line) else { continue };
                            let mut r = lock();
                            let Some(e) = r.entries.get_mut(&helper) else { break };
                            match provider {
                                Provider::Claude => on_claude_event(e, &v),
                                Provider::Codex => on_codex_event(e, &v),
                            }
                        }
                    });
                    if let Err(e) = read {
                        crate::log::warn("background", &format!("cannot read what {} says: {e}", crate::agents::label(&id)));
                    }
                }
                let code = child.wait().ok().and_then(|s| s.code());
                // What it wrote before it went is in the pipe by now.
                let _ = read_out.recv_timeout(LAST_WORDS);
                over.store(true, std::sync::atomic::Ordering::SeqCst);
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
            // Codex says its retries the same way ("Reconnecting... 2/5"): a
            // turn that gets through after them has not failed.
            if !why.starts_with("Reconnecting") {
                e.error = Some(why);
            }
        }
        Some("turn.completed") => {
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
    let next = {
        let mut r = lock();
        let Some(e) = r.entries.get_mut(id) else { return };
        e.run = None;
        e.turns_ended += 1;
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
        next
    };
    reg().1.notify_all();
    crate::log::info("background", &format!("{} turn over, code {code:?}", crate::agents::label(id)));
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
pub fn answer(id: &str, answers: &[String], allow: Option<bool>, note_text: Option<&str>) -> Result<(), String> {
    let mut r = lock();
    let e = r.entries.get_mut(id).ok_or("no such helper")?;
    let h = e.held.iter_mut().find(|h| h.answer.is_none()).ok_or_else(|| format!("helper {id} is not waiting on anything"))?;
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
    let rows: Vec<(String, Spec, u64, Option<String>, Option<String>, u32, bool, bool, bool)> = lock()
        .entries
        .iter()
        .map(|(id, e)| {
            let queued = !e.queue.is_empty();
            (id.clone(), e.spec.clone(), e.started_ms, e.session.clone(), e.cwd.clone(), e.turns_ended, e.run.is_some(), queued, e.cut_short)
        })
        .collect();
    // The agent layer's lock after this one is let go: it is the one taken
    // first where both are held.
    let mut saved: Vec<Saved> = rows
        .into_iter()
        .map(|(id, spec, started_ms, session, cwd, turns_ended, running, queued, cut_short)| Saved {
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
            held: VecDeque::new(),
            error: None,
            cut_short: false,
            job: None,
            left_running: false,
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
        assert_eq!(super::ask("two-asks").unwrap().tool.as_deref(), Some("Bash"), "the one that came first");
        assert_eq!(turn_state("two-asks").unwrap().turn, crate::activity::Turn::Waiting);
        answer("two-asks", &[], Some(true), None).unwrap();
        reg().1.notify_all();
        assert_eq!(first.join().unwrap()["behavior"], "allow", "the second did not cost it its answer");
        assert_eq!(super::ask("two-asks").unwrap().tool.as_deref(), Some("Edit"), "and then the next");
        assert_eq!(turn_state("two-asks").unwrap().turn, crate::activity::Turn::Waiting, "still held on that one");
        answer("two-asks", &[], Some(false), Some("not that")).unwrap();
        let no = second.join().unwrap();
        assert_eq!((no["behavior"].as_str(), no["message"].as_str()), (Some("deny"), Some("not that")));
        assert_eq!(super::ask("two-asks"), None);
        assert_eq!(turn_state("two-asks").unwrap().turn, crate::activity::Turn::Busy);
        lock().entries.remove("two-asks");
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
