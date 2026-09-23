use crate::provider::Provider;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, PtySize};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
// Only the Windows kill path shells out; on other targets this is dead weight
// and warns about itself on every build.
#[cfg(windows)]
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, State};

const SCROLLBACK_CAP: usize = 2 * 1024 * 1024;
/// Let the buffer run this far past the cap before trimming, so the trim costs
/// one pass per slack rather than one per read.
const SCROLLBACK_SLACK: usize = 256 * 1024;
/// How long output is allowed to pile up before it is sent to the UI — one
/// frame at 60Hz, short enough to feel immediate and long enough that a
/// repainting TUI does not turn into hundreds of events per second.
const FRAME: std::time::Duration = std::time::Duration::from_millis(16);

// Output and exit events go out through `emit::to_ui`, which holds them
// against the event loop tearing down at quit — see that module.

#[derive(Serialize, Clone)]
struct PtyOutput<'a> {
    id: &'a str,
    data: &'a str,
}

#[derive(Serialize, Clone)]
struct PtyExit<'a> {
    id: &'a str,
    code: Option<u32>,
}

/// What the UI sends a session, in the order it sent it.
enum Input {
    Bytes(Vec<u8>),
    Resize(PtySize),
}

struct Session {
    provider: Provider,
    /// Input is handed to a writer thread instead of being written inline.
    /// ConPTY stops accepting bytes while the child is not draining them, and a
    /// write that blocks used to do so while holding the session map — freezing
    /// every other chat's IPC behind one busy pane, for hundreds of ms at a
    /// time. A channel also keeps keystrokes in the order they were typed,
    /// which handing each write its own lock would not.
    ///
    /// Resizes travel the same channel, for the same reason: ConPTY's resize
    /// is synchronous and stalls like a write while the child is busy, and it
    /// used to run on the main thread under the session map — the log showed
    /// every pane's keystrokes queued for over a second behind one pane's
    /// window drag. The writer thread owns the master end, so the resize
    /// happens there, after the bytes that preceded it and before the ones
    /// that follow.
    input_tx: std::sync::mpsc::Sender<Input>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    /// For asking whether the process is still there without blocking on it;
    /// the reader thread shares it to collect the exit code.
    child: Arc<Mutex<Box<dyn portable_pty::Child + Send + Sync>>>,
    scrollback: Arc<Mutex<VecDeque<u8>>>,
    alive: Arc<AtomicBool>,
    /// When the child last wrote anything — Codex has no live status file, so
    /// a quiet screen mid-turn is how an approval prompt is told apart from
    /// work in progress.
    last_output_ms: Arc<AtomicU64>,
    pid: Option<u32>,
    cwd: String,
    spawned_at_ms: u128,
    /// The session id this was resumed with, if any: Codex is found on disk
    /// by it.
    resume: Option<String>,
    /// Kept so a session that has lost its chat row can still be described:
    /// its title and status live under this account.
    account_path: String,
    /// The job the CLI was put in at spawn, for listing what it has running
    /// under it (procs.rs). None where Windows refused or off Windows.
    job: Option<Arc<crate::procs::Job>>,
}

#[derive(Default)]
pub struct PtyManager {
    sessions: Mutex<HashMap<String, Session>>,
    /// One lock per chat id, held for as long as that chat is being spawned.
    /// It replaces holding `sessions` across the spawn: two calls for the same
    /// chat still cannot race into two processes, while a chat starting up no
    /// longer blocks every other pane's keystrokes.
    spawning: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

pub type PtyState<'a> = State<'a, PtyManager>;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Everything a spawn needs that is not the provider's business to work out.
struct Launch {
    provider: Provider,
    id: String,
    /// Where the process starts. For a Codex worktree chat this is the
    /// worktree itself; Claude Code gets the project folder and `--worktree`.
    cwd: String,
    account_path: String,
    args: Vec<String>,
    envs: Vec<(&'static str, String)>,
    resume: Option<String>,
    describe: String,
}

// Creating a session means `git worktree add` plus a process spawn — seconds of
// blocking work that must not run on the main thread, where it would stall
// every other chat's IPC. It used to hold the session map for that whole
// stretch to keep two calls from double-spawning one chat, which put every
// other pane's write_session behind it: the log shows keystrokes taking well
// over a second whenever a chat was starting. The per-chat gate below buys the
// same guarantee without the map.
async fn spawn(app: AppHandle, state: PtyState<'_>, launch: Launch) -> Result<String, String> {
    let Launch { provider, id, cwd, account_path, args, envs, resume, describe } = launch;
    let gate = {
        let mut gates = state.spawning.lock().unwrap();
        // Nobody holds a gate whose only reference is the map's own, so this
        // keeps the table to the chats actually starting right now.
        gates.retain(|_, g| Arc::strong_count(g) > 1);
        Arc::clone(gates.entry(id.clone()).or_default())
    };
    let _spawning = gate.lock().unwrap();

    // Under the gate: whoever waited here may find the session already spawned.
    {
        let mut sessions = state.sessions.lock().unwrap();
        if let Some(s) = sessions.get(&id) {
            if s.alive.load(Ordering::SeqCst) {
                let mut buf = s.scrollback.lock().unwrap();
                return Ok(String::from_utf8_lossy(buf.make_contiguous()).into_owned());
            }
            sessions.remove(&id);
        }
    }

    // Spawning without one would silently fall back to the CLI's default
    // config dir under the user profile: wrong account, and first-run
    // onboarding in every pane.
    if account_path.is_empty() {
        return Err("no account config dir for this chat".into());
    }

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 30,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| e.to_string())?;

    // Luna's own copy of the CLI (see cli.rs), or the bare name on PATH until
    // the first download has landed.
    let mut cmd = CommandBuilder::new(crate::cli::binary(provider));
    cmd.cwd(&cwd);
    cmd.args(&args);
    for (k, v) in &envs {
        cmd.env(k, v);
    }
    cmd.env("TERM", "xterm-256color");

    let child = pair.slave.spawn_command(cmd).map_err(|e| {
        crate::log::error("pty", &format!("spawn failed for {id} in {cwd}: {e}"));
        e.to_string()
    })?;
    drop(pair.slave);

    let pid = child.process_id();
    let spawned_at_ms = now_ms() as u128;
    // Into a job before it has had time to spawn anything, so every child it
    // ever starts is listed with it.
    let job = pid.and_then(crate::procs::adopt).map(Arc::new);
    let killer = child.clone_killer();
    let child = Arc::new(Mutex::new(child));
    let mut writer = pair.master.take_writer().map_err(|e| e.to_string())?;
    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;

    // A respawn is a new process and, for Codex, a new rollout file.
    if provider == Provider::Codex {
        crate::codex::session::release(&id);
    }

    // Drains on its own thread and ends when the session is dropped, which
    // drops the sender with it — and the master end, which the thread owns,
    // goes down with the thread, exactly when the session used to take it.
    let (input_tx, input_rx) = std::sync::mpsc::channel::<Input>();
    {
        let id = id.clone();
        let master = pair.master;
        std::thread::spawn(move || {
            // A broken pipe stays broken: say so once, then keep the master
            // alive for the session's own teardown rather than closing the pty
            // out from under a child that may still be on its way out.
            let mut writable = true;
            for msg in input_rx {
                match msg {
                    Input::Bytes(chunk) if writable => {
                        if let Err(e) = writer.write_all(&chunk).and_then(|()| writer.flush()) {
                            crate::log::warn("pty", &format!("write to {id} failed: {e}"));
                            writable = false;
                        }
                    }
                    Input::Bytes(_) => {}
                    Input::Resize(size) => {
                        if let Err(e) = master.resize(size) {
                            crate::log::warn("pty", &format!("resize of {id} failed: {e}"));
                        }
                    }
                }
            }
        });
    }

    let scrollback = Arc::new(Mutex::new(VecDeque::new()));
    let alive = Arc::new(AtomicBool::new(true));
    let last_output_ms = Arc::new(AtomicU64::new(now_ms()));

    {
        let app = app.clone();
        let id = id.clone();
        let scrollback = Arc::clone(&scrollback);
        let alive = Arc::clone(&alive);
        let last_output_ms = Arc::clone(&last_output_ms);
        let child = Arc::clone(&child);

        // The reader hands bytes to an emitter thread instead of emitting them
        // itself. A repainting TUI produces a steady stream of small reads, and
        // one event per read means one IPC message plus one JSON payload per
        // read, per chat, all landing on the webview's single thread — the app
        // got slower with every busy chat. The emitter coalesces whatever
        // arrives inside a frame into one event.
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let emitter = std::thread::spawn(move || {
            // Carry bytes of a UTF-8 sequence split across reads.
            let mut carry: Vec<u8> = Vec::new();
            while let Ok(first) = rx.recv() {
                carry.extend_from_slice(&first);
                // Keep collecting for one frame, so a burst becomes one event.
                let deadline = std::time::Instant::now() + FRAME;
                loop {
                    let left = deadline.saturating_duration_since(std::time::Instant::now());
                    if left.is_zero() {
                        break;
                    }
                    match rx.recv_timeout(left) {
                        Ok(more) => carry.extend_from_slice(&more),
                        Err(_) => break,
                    }
                }

                let valid_to = match std::str::from_utf8(&carry) {
                    Ok(_) => carry.len(),
                    Err(e) => e.valid_up_to(),
                };
                if valid_to > 0 {
                    let text = unsafe { std::str::from_utf8_unchecked(&carry[..valid_to]) };
                    crate::emit::to_ui(&app, "pty://output", PtyOutput { id: &id, data: text });
                }
                carry.drain(..valid_to);
                if carry.len() > 4 {
                    carry.clear(); // not a split sequence, just invalid bytes
                }
            }
            (app, id)
        });

        std::thread::spawn(move || {
            let mut chunk = [0u8; 8192];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        last_output_ms.store(now_ms(), Ordering::Relaxed);
                        {
                            let mut sb = scrollback.lock().unwrap();
                            sb.extend(chunk[..n].iter().copied());
                            // Trimming to the cap on every read used to move the
                            // whole two megabytes each time; a deque drops from
                            // the front without touching the rest, and trimming
                            // in one go per overshoot keeps it off the hot path.
                            if sb.len() > SCROLLBACK_CAP + SCROLLBACK_SLACK {
                                let excess = sb.len() - SCROLLBACK_CAP;
                                sb.drain(..excess);
                            }
                        }
                        if tx.send(chunk[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
            alive.store(false, Ordering::SeqCst);
            // Closing the channel ends the emitter; join it so the last output
            // of the session is on its way before the exit event goes out.
            drop(tx);
            let Ok((app, id)) = emitter.join() else { return };
            let code = child
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .wait()
                .ok()
                .map(|st| st.exit_code());
            crate::log::info("pty", &format!("session {id} exited, code {code:?}"));
            crate::emit::to_ui(&app, "pty://exit", PtyExit { id: &id, code });
        });
    }

    crate::log::info(
        "pty",
        &format!("spawned {id} ({}) pid {pid:?} {describe} in {cwd}", provider.as_str()),
    );

    state.sessions.lock().unwrap().insert(
        id,
        Session {
            provider,
            input_tx,
            killer,
            child,
            scrollback,
            alive,
            last_output_ms,
            pid,
            cwd,
            spawned_at_ms,
            resume,
            account_path,
            job,
        },
    );

    Ok(String::new())
}

/// The hooks a Claude Code session is started with: each event posts its
/// stdin to Luna's loopback listener (hub.rs) and prints nothing — a hook's
/// stdout is added to the model's context, silence is free. Written to a file
/// under the data dir rather than passed inline, so the command line stays
/// readable and quoting is not a concern. None while the listener is off.
fn claude_hooks_file(chat_id: &str) -> Option<std::path::PathBuf> {
    let url = crate::hub::hook_url(chat_id)?;
    // curl.exe by name: on Windows `curl` in PowerShell is an alias for
    // Invoke-WebRequest. Its stdin — the hook's JSON — goes up as the body;
    // the 204 answer has no body, so nothing comes back to print.
    let command = format!("curl.exe -s -m 3 -X POST --data-binary @- {url}");
    let hook = |timeout: u32| {
        serde_json::json!([{ "hooks": [{ "type": "command", "command": command, "timeout": timeout }] }])
    };
    let settings = serde_json::json!({
        "hooks": {
            "UserPromptSubmit": hook(5),
            "PostToolUse": hook(5),
            "Stop": hook(5),
            "Notification": hook(5),
            "SessionEnd": hook(5),
        }
    });
    let dir = crate::paths::data_dir().join("hooks");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{chat_id}.json"));
    std::fs::write(&path, serde_json::to_string_pretty(&settings).ok()?).ok()?;
    Some(path)
}

/// The MCP config a tooled Claude Code session starts with: Luna's server on
/// the hub listener, the session's own bearer token in the header. A file
/// per chat under the data dir, like the hooks.
fn claude_mcp_file(chat_id: &str, token: &str) -> Option<std::path::PathBuf> {
    let url = crate::hub::mcp_url()?;
    let config = serde_json::json!({
        "mcpServers": {
            "luna": {
                "type": "http",
                "url": url,
                "headers": { "Authorization": format!("Bearer {token}") }
            }
        }
    });
    let dir = crate::paths::data_dir().join("mcp");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{chat_id}.json"));
    std::fs::write(&path, serde_json::to_string_pretty(&config).ok()?).ok()?;
    Some(path)
}

/// What the activity sampler needs of a live session (activity.rs).
pub struct ActivityProbe {
    pub id: String,
    pub provider: Provider,
    pub pid: Option<u32>,
    pub cwd: String,
    pub spawned_at_ms: u128,
    pub resume: Option<String>,
    pub account_path: String,
    pub last_output_ms: u64,
    pub job: Option<Arc<crate::procs::Job>>,
}

fn activity_probe(id: &str, s: &Session) -> ActivityProbe {
    ActivityProbe {
        id: id.to_string(),
        provider: s.provider,
        pid: s.pid,
        cwd: s.cwd.clone(),
        spawned_at_ms: s.spawned_at_ms,
        resume: s.resume.clone(),
        account_path: s.account_path.clone(),
        last_output_ms: s.last_output_ms.load(Ordering::Relaxed),
        job: s.job.clone(),
    }
}

impl PtyManager {
    /// Every live session, copied out from under the map.
    pub fn activity_probes(&self) -> Vec<ActivityProbe> {
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions
            .iter()
            .filter(|(_, s)| s.alive.load(Ordering::SeqCst))
            .map(|(id, s)| activity_probe(id, s))
            .collect()
    }

    /// One live session, or None.
    pub fn probe(&self, id: &str) -> Option<ActivityProbe> {
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions.get(id).filter(|s| s.alive.load(Ordering::SeqCst)).map(|s| activity_probe(id, s))
    }

    pub fn alive(&self, id: &str) -> bool {
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions.get(id).map(|s| s.alive.load(Ordering::SeqCst)).unwrap_or(false)
    }

    /// Queues bytes for the session's writer thread.
    pub fn write(&self, id: &str, data: &[u8]) -> Result<(), String> {
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        let s = sessions.get(id).ok_or("no such session")?;
        s.input_tx.send(Input::Bytes(data.to_vec())).map_err(|_| "session writer is gone".to_string())
    }

    /// Asks the session to quit and kills it if it does not.
    pub async fn kill(&self, id: &str) {
        take_and_shut_down(self, id).await;
    }

    /// Tears a chat's session down: resolve its worktree, quit the process,
    /// drop attachments and the hooks file, and the worktree only if asked.
    /// Returns the worktree path either way.
    pub async fn delete(
        &self,
        id: &str,
        folder: &str,
        account_path: &str,
        worktree_path: Option<String>,
        drop_worktree: bool,
    ) -> Result<Option<String>, String> {
        // Resolve before killing: once the process is gone its registry entry goes too.
        let resolved = worktree_path.filter(|p| !p.is_empty()).or_else(|| {
            let (provider, pid, cwd, spawned_at_ms) = {
                let sessions = self.sessions.lock().unwrap();
                let s = sessions.get(id)?;
                (s.provider, s.pid, s.cwd.clone(), s.spawned_at_ms)
            };
            let scwd = match provider {
                // Claude Code moves into the worktree itself; only the registry
                // knows where.
                Provider::Claude => crate::claude::session::session_cwd(pid, &cwd, spawned_at_ms, account_path)?,
                // A Codex session runs where Luna started it.
                Provider::Codex => cwd,
            };
            crate::worktree::is_worktree_of(folder, &scwd).then_some(scwd)
        });

        take_and_shut_down(self, id).await;

        let _ = crate::media::clear_media(id.to_string());
        for sub in ["hooks", "mcp"] {
            let _ = std::fs::remove_file(crate::paths::data_dir().join(sub).join(format!("{id}.json")));
        }
        crate::log::info("delete", &format!("chat {id}, worktree {resolved:?}, dropping {drop_worktree}"));

        match &resolved {
            Some(wt) if drop_worktree => {
                crate::worktree::remove_worktree_now(folder.to_string(), wt.clone()).map(|_| resolved.clone())
            }
            Some(_) => Ok(resolved),
            None => Ok(None),
        }
    }

    /// Asks every live session to quit, each with `grace` to do it on its
    /// own, and returns how many there were. Blocks; for the shutdown path.
    pub fn shut_down_all(&self, grace: std::time::Duration) -> usize {
        let taken: Vec<Session> = {
            let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
            let ids: Vec<String> = sessions.iter().filter(|(_, s)| s.alive.load(Ordering::SeqCst)).map(|(id, _)| id.clone()).collect();
            ids.into_iter().filter_map(|id| sessions.remove(&id)).collect()
        };
        let n = taken.len();
        let handles: Vec<_> = taken.into_iter().map(|s| std::thread::spawn(move || shut_down(s, grace))).collect();
        for h in handles {
            let _ = h.join();
        }
        n
    }
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn ensure_claude_session(
    app: AppHandle,
    state: PtyState<'_>,
    id: String,
    folder: String,
    account_path: String,
    model: String,
    effort: String,
    permission_mode: String,
    worktree: bool,
    resume: Option<String>,
    // Luna's MCP tools (agents.rs): off unless the chat asked.
    tools: Option<bool>,
    // The chat that spawned this one, if an agent did.
    parent: Option<String>,
    // An opening prompt, for a chat an agent starts.
    prompt: Option<String>,
) -> Result<String, String> {
    let mut args = vec![
        "--model".to_string(),
        model.clone(),
        "--effort".to_string(),
        effort,
        "--permission-mode".to_string(),
        permission_mode.clone(),
    ];
    if let Some(session_id) = &resume {
        args.push("--resume".into());
        args.push(session_id.clone());
    }
    if worktree {
        args.push("--worktree".into());
    }
    // Rewritten at every spawn: the listener's port is new with every Luna run.
    if let Some(hooks) = claude_hooks_file(&id) {
        args.push("--settings".into());
        args.push(hooks.to_string_lossy().into_owned());
    }
    let tools = tools.unwrap_or(false);
    let token = crate::agents::register(&id, Provider::Claude, &folder, &account_path, parent.as_deref(), tools);
    if let Some(mcp) = token.as_deref().and_then(|t| claude_mcp_file(&id, t)) {
        args.push("--mcp-config".into());
        args.push(mcp.to_string_lossy().into_owned());
    }
    if let Some(p) = prompt.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        args.push(p.to_string());
    }
    let envs = vec![
        ("CLAUDE_CONFIG_DIR", account_path.clone()),
        // Luna updates the CLI itself; the CLI's own updater would install a
        // second copy under the user profile that nothing here ever runs.
        ("DISABLE_AUTOUPDATER", "1".to_string()),
    ];
    spawn(
        app,
        state,
        Launch {
            provider: Provider::Claude,
            id,
            cwd: folder,
            account_path,
            args,
            envs,
            resume,
            describe: format!("model {model} perm {permission_mode} worktree {worktree}"),
        },
    )
    .await
}

/// A Codex session. `folder` is where it runs — for a worktree chat, the
/// worktree Luna made. `login` runs `codex login` instead of a chat.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn ensure_codex_session(
    app: AppHandle,
    state: PtyState<'_>,
    id: String,
    folder: String,
    account_path: String,
    model: Option<String>,
    effort: String,
    approval: String,
    sandbox: String,
    resume: Option<String>,
    login: bool,
    tools: Option<bool>,
    parent: Option<String>,
    prompt: Option<String>,
) -> Result<String, String> {
    let mut args: Vec<String> = vec![];
    let mut envs = vec![("CODEX_HOME", account_path.clone())];
    let describe;
    if login {
        args.push("login".into());
        describe = "login".to_string();
    } else {
        if let Some(session_id) = &resume {
            args.push("resume".into());
            args.push(session_id.clone());
        }
        args.push("-C".into());
        args.push(folder.clone());
        if let Some(m) = model.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
            args.push("--model".into());
            args.push(m.to_string());
        }
        args.push("-c".into());
        args.push(format!("model_reasoning_effort=\"{effort}\""));
        // The pair Codex itself spells as one flag.
        if approval == "never" && sandbox == "danger-full-access" {
            args.push("--dangerously-bypass-approvals-and-sandbox".into());
        } else {
            args.push("--ask-for-approval".into());
            args.push(approval.clone());
            args.push("--sandbox".into());
            args.push(sandbox.clone());
        }
        // Luna's MCP server as a config override, the token through the
        // environment: neither touches the account's config.toml.
        let tools = tools.unwrap_or(false);
        let token = crate::agents::register(&id, Provider::Codex, &folder, &account_path, parent.as_deref(), tools);
        if let (Some(token), Some(url)) = (token, crate::hub::mcp_url()) {
            args.push("-c".into());
            args.push(format!("mcp_servers.luna.url=\"{url}\""));
            args.push("-c".into());
            args.push("mcp_servers.luna.bearer_token_env_var=\"LUNA_MCP_TOKEN\"".into());
            envs.push(("LUNA_MCP_TOKEN", token));
        }
        if let Some(p) = prompt.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
            args.push(p.to_string());
        }
        describe = format!(
            "model {} effort {effort} approval {approval} sandbox {sandbox}",
            model.as_deref().unwrap_or("default")
        );
    }
    spawn(
        app,
        state,
        Launch {
            provider: Provider::Codex,
            id,
            cwd: folder,
            account_path,
            args,
            envs,
            resume: if login { None } else { resume },
            describe,
        },
    )
    .await
}

#[tauri::command]
pub fn write_session(state: PtyState, id: String, data: String) -> Result<(), String> {
    let sessions = state.sessions.lock().unwrap();
    let s = sessions.get(&id).ok_or("no such session")?;
    // Queueing, so the command returns at once however busy the pty is.
    s.input_tx
        .send(Input::Bytes(data.into_bytes()))
        .map_err(|_| "session writer is gone".to_string())
}

/// Queued behind the keystrokes that preceded it and applied on the writer
/// thread. A failure there is logged rather than returned: by the time it
/// happens the caller has moved on, and it never looked at the result anyway.
#[tauri::command]
pub fn resize_session(state: PtyState, id: String, cols: u16, rows: u16) -> Result<(), String> {
    let sessions = state.sessions.lock().unwrap();
    let s = sessions.get(&id).ok_or("no such session")?;
    s.input_tx
        .send(Input::Resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        }))
        .map_err(|_| "session writer is gone".to_string())
}

// The pty child is the CLI; everything it spawned (node, MCP servers, shells)
// is a grandchild and survives a plain kill. Sweep the tree first, then kill
// the child directly as a backstop.
fn kill_tree(pid: Option<u32>) {
    #[cfg(windows)]
    if let Some(pid) = pid {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .output();
    }
    #[cfg(not(windows))]
    let _ = pid;
}

/// How long a session gets to leave on its own before it is taken down.
const GRACE: std::time::Duration = std::time::Duration::from_millis(1500);
/// Ctrl+C is one keystroke to the CLI and two in a row mean quit; one write
/// carrying both lands in a single input tick, where the second cannot see
/// the state the first set. Space them the way a hand would.
const CTRL_C_GAP: std::time::Duration = std::time::Duration::from_millis(150);

/// Asks the CLI to quit, and kills it only if it does not.
///
/// `taskkill /F` ends the process without its exit hooks, and the CLI keeps
/// state that only those hooks clean up. A fullscreen launch records itself as
/// pending until it has been healthy for ten seconds; a launch that dies with
/// the record still there costs the next launch on that account its fullscreen
/// renderer ("didn't finish starting last time"), and two of them turn the
/// renderer off. Deleting a chat you had just made, or closing the login
/// window, did exactly that. So first the keystrokes a user would type —
/// Ctrl+C, and again when it asks — and the hard kill only for a process that
/// is past listening. Codex reads Ctrl+C the same way: interrupt, then quit.
///
/// Blocks for up to `grace`: callers run it off the main thread.
fn shut_down(mut s: Session, grace: std::time::Duration) {
    let exited = || {
        s.child
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .try_wait()
            .map(|status| status.is_some())
            .unwrap_or(true)
    };
    let deadline = std::time::Instant::now() + grace;
    // A turn in flight takes one Ctrl+C to interrupt before the next two ask
    // for and confirm the exit.
    for _ in 0..3 {
        if exited() || s.input_tx.send(Input::Bytes(b"\x03".to_vec())).is_err() {
            break;
        }
        std::thread::sleep(CTRL_C_GAP);
    }
    while !exited() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    if !exited() {
        crate::log::warn("pty", &format!("pid {:?} did not quit on Ctrl+C within {grace:?}; killing it", s.pid));
    }
    // Either way: the CLI's own children (shells, MCP servers) do not follow it
    // out, and a taskkill on a pid that has just exited is a no-op.
    kill_tree(s.pid);
    let _ = s.killer.kill();
}

/// Takes the session out of the map first and shuts it down after: the
/// shutdown waits on the process, and waiting under the map would put every
/// other chat's keystrokes behind it.
async fn take_and_shut_down(state: &PtyManager, id: &str) {
    let session = state.sessions.lock().unwrap().remove(id);
    if let Some(s) = session {
        let _ = tauri::async_runtime::spawn_blocking(move || shut_down(s, GRACE)).await;
    }
}

#[tauri::command]
pub async fn kill_session(state: PtyState<'_>, id: String) -> Result<(), String> {
    take_and_shut_down(&state, &id).await;
    Ok(())
}

/// Tears a chat down in one shot: resolve where the session actually lives,
/// kill it, drop its worktree (and the branch made for it) and its
/// attachments. Doing this in one command removes the race the UI had — it
/// used to rely on a 4s poll having already reported the worktree path.
#[tauri::command]
// Async for the same reason as the worktree commands: removing a worktree can
// take seconds, and on the main thread every keystroke in every other chat
// waits behind it.
pub async fn delete_session(
    state: PtyState<'_>,
    id: String,
    folder: String,
    account_path: String,
    worktree_path: Option<String>,
    // Off by default at the call site: the worktree may hold work that exists
    // nowhere else, and a deleted chat is a cheaper mistake than a deleted
    // branch. Returns the path either way, so the caller can say where the
    // kept worktree is.
    drop_worktree: bool,
) -> Result<Option<String>, String> {
    state.delete(&id, &folder, &account_path, worktree_path, drop_worktree).await
}

/// The cwd of every live session this app runs. A running session must never
/// have its worktree swept, even if the UI has not yet learned where it lives.
pub fn live_cwds(state: &PtyState<'_>) -> Vec<String> {
    let sessions = state.sessions.lock().unwrap();
    sessions
        .values()
        .filter(|s| s.alive.load(Ordering::SeqCst))
        .map(|s| s.cwd.clone())
        .collect()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrphanSession {
    /// The chat id the session was spawned under. Re-creating a chat row with
    /// this exact id is what reattaches it, scrollback included.
    pub id: String,
    pub provider: Provider,
    pub pid: Option<u32>,
    pub cwd: String,
    pub account_path: String,
    pub title: Option<String>,
    pub status: Option<String>,
}

/// Sessions this app is still running that no chat claims any more.
///
/// A chat row can go missing while its session keeps working — dropped by a
/// half-written state restore, say — and there is then no
/// way to reach the session from the UI at all: it holds an account's tokens
/// and answers to nobody. The frontend passes the chat ids it knows, and
/// anything alive outside that set comes back here.
///
/// Only this app's own ptys are considered. Claude Code registers its
/// subagents and shells in the same account registry, so walking that instead
/// would report every busy chat's children as orphans.
#[tauri::command]
pub async fn orphan_sessions(
    state: PtyState<'_>,
    known: Vec<String>,
) -> Result<Vec<OrphanSession>, String> {
    let candidates: Vec<Probe> = {
        let sessions = state.sessions.lock().unwrap();
        sessions
            .iter()
            .filter(|(id, s)| !known.contains(id) && s.alive.load(Ordering::SeqCst))
            .map(|(id, s)| probe_of(id, s))
            .collect()
    };
    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    // Naming one means reading the head of its transcript, so do it off the
    // main thread like session_meta does.
    tauri::async_runtime::spawn_blocking(move || {
        candidates
            .into_iter()
            .map(|p| {
                let meta = meta_from_disk(&p);
                OrphanSession {
                    id: p.id,
                    provider: p.provider,
                    pid: p.pid,
                    cwd: p.cwd,
                    account_path: p.account_path,
                    title: meta.as_ref().and_then(|m| {
                        m.title.clone().or_else(|| m.first_prompt.clone()).or_else(|| m.name.clone())
                    }),
                    status: meta.and_then(|m| m.status),
                }
            })
            .collect()
    })
    .await
    .map_err(|e| e.to_string())
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SessionMeta {
    pub name: Option<String>,
    /// Claude Code's own vocabulary as the registry has it, or for Codex
    /// `working` | `waiting` | `resting`.
    pub status: Option<String>,
    pub cwd: Option<String>,
    pub session_id: Option<String>,
    /// "auto" (AI-titled), "user" (renamed by hand) or "derived" (just the cwd
    /// folder name, which for a worktree session is meaningless noise).
    pub name_source: Option<String>,
    /// The model the session is actually running, where the CLI records it
    /// (Codex's `turn_context`); None where the chat setting is the truth.
    pub model: Option<String>,
    pub context: Option<f64>,
    pub context_tokens: Option<f64>,
    /// Window the percentage was computed against, so the UI can show what it
    /// assumed instead of silently pinning a longer session at 100%.
    pub context_window: Option<f64>,
    /// The CLI's own title for the session: a rename, else the one it
    /// generated (Claude Code's `ai-title`, the same it puts on the terminal
    /// tab), else None.
    pub title: Option<String>,
    /// First real prompt of the session — the chat's title until the CLI
    /// comes up with one.
    pub first_prompt: Option<String>,
}

/// A title has to be something a person can read. Both sources have handed us
/// strings that were not: a registry name made of the terminal's own cursor
/// and mouse reports, which is what a chat row read as for its first minutes.
/// Rejecting them here keeps that out of every place a title is shown.
pub fn sane_title(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty() && !t.chars().any(char::is_control)).then(|| t.to_string())
}

/// A prompt trimmed to a title: whitespace flattened, 48 characters. Slash
/// commands, replayed tool output and system reminders are not titles.
pub fn title_from_prompt(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() || text.starts_with('<') || text.starts_with('/') {
        return None;
    }
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut title: String = flat.chars().take(48).collect();
    if flat.chars().count() > 48 {
        title.push('…');
    }
    sane_title(&title)
}

/// The last `max` bytes of a file as text, or None if it cannot be read.
pub fn tail(path: &std::path::Path, max: u64) -> Option<String> {
    use std::io::{Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let take = len.min(max);
    f.seek(SeekFrom::End(-(take as i64))).ok()?;
    let mut bytes = Vec::with_capacity(take as usize);
    f.read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// What a session-meta read needs, copied out from under the map.
struct Probe {
    id: String,
    provider: Provider,
    pid: Option<u32>,
    cwd: String,
    spawned_at_ms: u128,
    resume: Option<String>,
    account_path: String,
    last_output_ms: u64,
}

fn probe_of(id: &str, s: &Session) -> Probe {
    Probe {
        id: id.to_string(),
        provider: s.provider,
        pid: s.pid,
        cwd: s.cwd.clone(),
        spawned_at_ms: s.spawned_at_ms,
        resume: s.resume.clone(),
        account_path: s.account_path.clone(),
        last_output_ms: s.last_output_ms.load(Ordering::Relaxed),
    }
}

fn meta_from_disk(p: &Probe) -> Option<SessionMeta> {
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

// Name, status and context of a live session, read off what its CLI leaves on
// disk — free, no tokens.
//
// Async on purpose: this runs every few seconds for every open pane and reads
// the tail of a transcript that grows into the megabytes. As a sync command it
// would do that on the main thread, i.e. with the window frozen.
#[tauri::command]
//
// A chat with no live session answers None rather than failing: the watcher
// polls every chat every few seconds, resting ones included, so treating an
// absent session as an error meant thousands of warnings a day in the log —
// enough to push everything worth reading out of it.
pub async fn session_meta(
    state: PtyState<'_>,
    id: String,
    account_path: String,
) -> Result<Option<SessionMeta>, String> {
    let probe = {
        let sessions = state.sessions.lock().unwrap();
        let Some(s) = sessions.get(&id) else {
            return Ok(None);
        };
        if !s.alive.load(Ordering::SeqCst) {
            return Ok(None);
        }
        let mut p = probe_of(&id, s);
        // The caller's idea of the account wins, as it always has: the chat
        // row is the source of truth for which folder a chat belongs to.
        if !account_path.is_empty() {
            p.account_path = account_path;
        }
        p
    };

    tauri::async_runtime::spawn_blocking(move || meta_from_disk(&probe))
        .await
        .map_err(|e| e.to_string())
}

/// The CLI's title for a session that is not running, by its id — how a chat
/// restored from an earlier run picks up the title its CLI gave it.
#[tauri::command]
pub async fn saved_title(provider: Provider, account_path: String, session_id: String) -> Option<String> {
    tauri::async_runtime::spawn_blocking(move || match provider {
        Provider::Claude => crate::claude::session::saved_title(&account_path, &session_id),
        Provider::Codex => crate::codex::session::thread_name(&account_path, &session_id),
    })
    .await
    .ok()
    .flatten()
}

#[tauri::command]
pub fn session_alive(state: PtyState, id: String) -> bool {
    let sessions = state.sessions.lock().unwrap();
    sessions
        .get(&id)
        .map(|s| s.alive.load(Ordering::SeqCst))
        .unwrap_or(false)
}
