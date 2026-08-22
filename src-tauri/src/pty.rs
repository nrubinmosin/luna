use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, PtySize};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
// Only the Windows kill path shells out; on other targets this is dead weight
// and warns about itself on every build.
#[cfg(windows)]
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, State};

const SCROLLBACK_CAP: usize = 2 * 1024 * 1024;
/// Let the buffer run this far past the cap before trimming, so the trim costs
/// one pass per slack rather than one per read.
const SCROLLBACK_SLACK: usize = 256 * 1024;
/// How long output is allowed to pile up before it is sent to the UI — one
/// frame at 60Hz, short enough to feel immediate and long enough that a
/// repainting TUI does not turn into hundreds of events per second.
const FRAME: std::time::Duration = std::time::Duration::from_millis(16);

/// Set once the app has been asked to exit. Sessions outlive the window by
/// design (quit only comes from the tray), so their reader threads are still
/// streaming when the event loop starts tearing down — and an emit landing on
/// a destroyed tao loop is a panic ("cannot move state from Destroyed"), not
/// an Err. Checking the flag narrows that window to the emit already in
/// flight; it cannot close it completely, but the panic it guards against
/// fired at shutdown, where the process is going away regardless.
pub static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);

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

struct Session {
    master: Box<dyn portable_pty::MasterPty + Send>,
    /// Input is handed to a writer thread instead of being written inline.
    /// ConPTY stops accepting bytes while the child is not draining them, and a
    /// write that blocks used to do so while holding the session map — freezing
    /// every other chat's IPC behind one busy pane, for hundreds of ms at a
    /// time. A channel also keeps keystrokes in the order they were typed,
    /// which handing each write its own lock would not.
    writer_tx: std::sync::mpsc::Sender<Vec<u8>>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    scrollback: Arc<Mutex<VecDeque<u8>>>,
    alive: Arc<AtomicBool>,
    pid: Option<u32>,
    cwd: String,
    spawned_at_ms: u128,
    /// Kept so a session that has lost its chat row can still be described:
    /// its title and status live in this account's registry.
    account_path: String,
}

#[derive(Default)]
pub struct PtyManager {
    sessions: Mutex<HashMap<String, Session>>,
}

pub type PtyState<'a> = State<'a, PtyManager>;

#[allow(clippy::too_many_arguments)]
#[tauri::command]
// Creating a session means `git worktree add` plus a process spawn — seconds of
// blocking work that must not run on the main thread, where it would stall
// every other chat's IPC. The sessions lock is held for the whole body, so
// concurrent calls still serialise and cannot double-spawn one chat.
pub async fn ensure_session(
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
) -> Result<String, String> {
    let mut sessions = state.sessions.lock().unwrap();

    if let Some(s) = sessions.get(&id) {
        if s.alive.load(Ordering::SeqCst) {
            let mut buf = s.scrollback.lock().unwrap();
            return Ok(String::from_utf8_lossy(buf.make_contiguous()).into_owned());
        }
        sessions.remove(&id);
    }

    // Spawning without one would silently fall back to the default ~/.claude
    // config: wrong account, and first-run onboarding in every pane.
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

    // Luna's own copy of the CLI (see cli.rs), or `claude` on PATH until the
    // first download has landed.
    let mut cmd = CommandBuilder::new(crate::cli::binary());
    cmd.cwd(&folder);
    cmd.args(["--model", &model]);
    cmd.args(["--effort", &effort]);
    cmd.args(["--permission-mode", &permission_mode]);
    if let Some(session_id) = &resume {
        cmd.args(["--resume", session_id]);
    }
    if worktree {
        cmd.arg("--worktree");
    }
    if !account_path.is_empty() {
        cmd.env("CLAUDE_CONFIG_DIR", &account_path);
    }
    cmd.env("TERM", "xterm-256color");
    // Luna updates the CLI itself; the CLI's own updater would install a
    // second copy under the user profile that nothing here ever runs.
    cmd.env("DISABLE_AUTOUPDATER", "1");

    let child = pair.slave.spawn_command(cmd).map_err(|e| {
        crate::log::error("pty", &format!("spawn failed for {id} in {folder}: {e}"));
        e.to_string()
    })?;
    drop(pair.slave);

    let pid = child.process_id();
    let spawned_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let killer = child.clone_killer();
    let mut child = child;
    let mut writer = pair.master.take_writer().map_err(|e| e.to_string())?;
    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;

    // Drains on its own thread and ends when the session is dropped, which
    // drops the sender with it.
    let (writer_tx, writer_rx) = std::sync::mpsc::channel::<Vec<u8>>();
    {
        let id = id.clone();
        std::thread::spawn(move || {
            for chunk in writer_rx {
                if let Err(e) = writer.write_all(&chunk).and_then(|()| writer.flush()) {
                    crate::log::warn("pty", &format!("write to {id} failed: {e}"));
                    break;
                }
            }
        });
    }

    let scrollback = Arc::new(Mutex::new(VecDeque::new()));
    let alive = Arc::new(AtomicBool::new(true));

    {
        let app = app.clone();
        let id = id.clone();
        let scrollback = Arc::clone(&scrollback);
        let alive = Arc::clone(&alive);

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
                if valid_to > 0 && !SHUTTING_DOWN.load(Ordering::SeqCst) {
                    let text = unsafe { std::str::from_utf8_unchecked(&carry[..valid_to]) };
                    let _ = app.emit("pty://output", PtyOutput { id: &id, data: text });
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
            let code = child.wait().ok().map(|st| st.exit_code());
            crate::log::info("pty", &format!("session {id} exited, code {code:?}"));
            if !SHUTTING_DOWN.load(Ordering::SeqCst) {
                let _ = app.emit("pty://exit", PtyExit { id: &id, code });
            }
        });
    }

    crate::log::info(
        "pty",
        &format!("spawned {id} pid {pid:?} model {model} perm {permission_mode} worktree {worktree} in {folder}"),
    );

    sessions.insert(
        id,
        Session {
            master: pair.master,
            writer_tx,
            killer,
            scrollback,
            alive,
            pid,
            cwd: folder,
            spawned_at_ms,
            account_path,
        },
    );

    Ok(String::new())
}

#[tauri::command]
pub fn write_session(state: PtyState, id: String, data: String) -> Result<(), String> {
    let sessions = state.sessions.lock().unwrap();
    let s = sessions.get(&id).ok_or("no such session")?;
    // Queueing, so the command returns at once however busy the pty is.
    s.writer_tx
        .send(data.into_bytes())
        .map_err(|_| "session writer is gone".to_string())
}

#[tauri::command]
pub fn resize_session(state: PtyState, id: String, cols: u16, rows: u16) -> Result<(), String> {
    let sessions = state.sessions.lock().unwrap();
    let s = sessions.get(&id).ok_or("no such session")?;
    s.master
        .resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| e.to_string())
}

// The pty child is `claude.exe`; everything it spawned (node, MCP servers)
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

#[tauri::command]
pub fn kill_session(state: PtyState, id: String) -> Result<(), String> {
    let mut sessions = state.sessions.lock().unwrap();
    if let Some(mut s) = sessions.remove(&id) {
        kill_tree(s.pid);
        let _ = s.killer.kill();
    }
    Ok(())
}

/// Tears a chat down in one shot: resolve where the session actually lives,
/// kill it, drop its worktree (and the branch the CLI made for it) and its
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
    // Resolve before killing: once the process is gone its registry entry goes too.
    let resolved = worktree_path.filter(|p| !p.is_empty()).or_else(|| {
        let (pid, cwd, spawned_at_ms) = {
            let sessions = state.sessions.lock().unwrap();
            let s = sessions.get(&id)?;
            (s.pid, s.cwd.clone(), s.spawned_at_ms)
        };
        let dir = std::path::Path::new(&account_path).join("sessions");
        let v = registry_entry(&dir, pid, &cwd, spawned_at_ms)?;
        let scwd = v["cwd"].as_str()?.to_string();
        crate::worktree::is_worktree_of(&folder, &scwd).then_some(scwd)
    });

    {
        let mut sessions = state.sessions.lock().unwrap();
        if let Some(mut s) = sessions.remove(&id) {
            kill_tree(s.pid);
            let _ = s.killer.kill();
        }
    }

    let _ = crate::media::clear_media(id.clone());
    crate::log::info(
        "delete",
        &format!("chat {id}, worktree {resolved:?}, dropping {drop_worktree}"),
    );

    match &resolved {
        Some(wt) if drop_worktree => {
            crate::worktree::remove_worktree_now(folder, wt.clone()).map(|_| resolved.clone())
        }
        Some(_) => Ok(resolved),
        None => Ok(None),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrphanSession {
    /// The chat id the session was spawned under. Re-creating a chat row with
    /// this exact id is what reattaches it, scrollback included.
    pub id: String,
    pub pid: Option<u32>,
    pub cwd: String,
    pub account_path: String,
    pub title: Option<String>,
    pub status: Option<String>,
}

/// Sessions this app is still running that no chat claims any more.
///
/// A chat row can go missing while its session keeps working — archived by a
/// misclick, dropped by a half-written state restore — and there is then no
/// way to reach the session from the UI at all: it holds an account's tokens
/// and answers to nobody. The frontend passes the chat ids it knows, and
/// anything alive outside that set comes back here.
///
/// Only this app's own ptys are considered. The CLI registers its subagents
/// and shells in the same account registry, so walking that instead would
/// report every busy chat's children as orphans.
#[tauri::command]
pub async fn orphan_sessions(
    state: PtyState<'_>,
    known: Vec<String>,
) -> Result<Vec<OrphanSession>, String> {
    let candidates: Vec<(String, Option<u32>, String, String, u128)> = {
        let sessions = state.sessions.lock().unwrap();
        sessions
            .iter()
            .filter(|(id, s)| !known.contains(id) && s.alive.load(Ordering::SeqCst))
            .map(|(id, s)| {
                (id.clone(), s.pid, s.cwd.clone(), s.account_path.clone(), s.spawned_at_ms)
            })
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
            .map(|(id, pid, cwd, account_path, spawned_at_ms)| {
                let meta = meta_from_disk(pid, cwd.clone(), spawned_at_ms, account_path.clone());
                OrphanSession {
                    id,
                    pid,
                    cwd,
                    account_path,
                    title: meta
                        .as_ref()
                        .and_then(|m| m.first_prompt.clone().or_else(|| m.name.clone())),
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
    pub status: Option<String>,
    pub cwd: Option<String>,
    pub session_id: Option<String>,
    /// "auto" (AI-titled), "user" (renamed by hand) or "derived" (just the cwd
    /// folder name, which for a worktree session is meaningless noise).
    pub name_source: Option<String>,
    pub context: Option<f64>,
    pub context_tokens: Option<f64>,
    /// Window the percentage was computed against, so the UI can show what it
    /// assumed instead of silently pinning a longer session at 100%.
    pub context_window: Option<f64>,
    /// First real prompt of the session, used as a chat title: the registry's
    /// own name is always `derived` (the cwd folder) in practice.
    pub first_prompt: Option<String>,
}

// Claude Code encodes a project cwd into a transcript folder name by replacing
// every non-alphanumeric character with '-'.
fn encode_project_dir(p: &str) -> String {
    p.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

struct ContextRead {
    tokens: f64,
    window: f64,
}

// Free context reading: the last assistant message in the session transcript
// carries cumulative input-side token usage. Returns raw tokens; the fraction
// is derived from it.
fn transcript_path(account_path: &str, cwd: &str, session_id: &str) -> std::path::PathBuf {
    std::path::Path::new(account_path)
        .join("projects")
        .join(encode_project_dir(cwd))
        .join(format!("{session_id}.jsonl"))
}

fn read_context(account_path: &str, cwd: &str, session_id: &str) -> Option<ContextRead> {
    use std::io::{Read, Seek, SeekFrom};
    let path = transcript_path(account_path, cwd, session_id);
    let mut f = std::fs::File::open(&path).ok()?;
    let len = f.metadata().ok()?.len();
    let take = len.min(128 * 1024);
    f.seek(SeekFrom::End(-(take as i64))).ok()?;
    let mut bytes = Vec::with_capacity(take as usize);
    f.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    for line in text.lines().rev() {
        if !line.contains("\"usage\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        // Subagent turns are interleaved into the same transcript but run in
        // their own context window — their usage says nothing about this chat.
        if v["isSidechain"].as_bool().unwrap_or(false) {
            continue;
        }
        let u = &v["message"]["usage"];
        if u.is_object() {
            // Input side only: output tokens of a turn become input of the
            // next one, so counting them here would double-count.
            let total = u["input_tokens"].as_f64().unwrap_or(0.0)
                + u["cache_read_input_tokens"].as_f64().unwrap_or(0.0)
                + u["cache_creation_input_tokens"].as_f64().unwrap_or(0.0);
            if total > 0.0 {
                let model = v["message"]["model"].as_str().unwrap_or("");
                return Some(ContextRead {
                    tokens: total,
                    window: crate::models::context_window(model),
                });
            }
        }
    }
    None
}

/// Text of a transcript content field, which is either a bare string or an
/// array of blocks.
fn content_text(content: &serde_json::Value) -> Option<String> {
    if let Some(t) = content.as_str() {
        return Some(t.to_string());
    }
    let blocks = content.as_array()?;
    for b in blocks {
        if b["type"] == "text" {
            if let Some(t) = b["text"].as_str() {
                return Some(t.to_string());
            }
        }
    }
    None
}

/// The opening prompt of a session never changes, but session_meta asks for it
/// every few seconds per pane — so read the file once and remember the answer.
fn cached_first_prompt(account_path: &str, cwd: &str, session_id: &str) -> Option<String> {
    static CACHE: std::sync::OnceLock<Mutex<HashMap<String, Option<String>>>> =
        std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);

    if let Ok(map) = cache.lock() {
        if let Some(hit) = map.get(session_id) {
            return hit.clone();
        }
    }
    let found = read_first_prompt(account_path, cwd, session_id);
    // A miss is worth caching too, but only once the transcript exists: before
    // the first turn there is genuinely nothing to read yet.
    if found.is_some() {
        if let Ok(mut map) = cache.lock() {
            map.insert(session_id.to_string(), found.clone());
        }
    }
    found
}

/// The session's opening prompt, trimmed to a title. Sessions are always named
/// `derived` in the registry — i.e. after the cwd, which for a worktree run is
/// a random codename — so the first thing the user actually said is a far
/// better label for the chat.
fn read_first_prompt(account_path: &str, cwd: &str, session_id: &str) -> Option<String> {
    use std::io::{BufRead, BufReader};
    let path = transcript_path(account_path, cwd, session_id);
    let f = std::fs::File::open(&path).ok()?;
    // The opening prompt is near the top; no need to walk a 700KB transcript.
    for line in BufReader::new(f).lines().take(80).map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
        if v["type"] != "user" || v["isSidechain"].as_bool().unwrap_or(false) {
            continue;
        }
        let Some(text) = content_text(&v["message"]["content"]) else { continue };
        let text = text.trim();
        // Slash commands, replayed tool output and system reminders are not titles.
        if text.is_empty() || text.starts_with('<') || text.starts_with('/') {
            continue;
        }
        let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut title: String = flat.chars().take(48).collect();
        if flat.chars().count() > 48 {
            title.push('…');
        }
        return Some(title);
    }
    None
}

fn norm_path(p: &str) -> String {
    p.replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

fn parse_session_file(path: &std::path::Path) -> Option<serde_json::Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

// Claude Code maintains a live registry at <config>/sessions/<pid>.json with
// the AI-derived session name and current status — free to read, no tokens.
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
    let (pid, cwd, spawned_at_ms) = {
        let sessions = state.sessions.lock().unwrap();
        let Some(s) = sessions.get(&id) else {
            return Ok(None);
        };
        if !s.alive.load(Ordering::SeqCst) {
            return Ok(None);
        }
        (s.pid, s.cwd.clone(), s.spawned_at_ms)
    };

    tauri::async_runtime::spawn_blocking(move || meta_from_disk(pid, cwd, spawned_at_ms, account_path))
        .await
        .map_err(|e| e.to_string())
}

fn meta_from_disk(
    pid: Option<u32>,
    cwd: String,
    spawned_at_ms: u128,
    account_path: String,
) -> Option<SessionMeta> {
    let dir = std::path::Path::new(&account_path).join("sessions");
    let extract = |v: &serde_json::Value| {
        let mut m = SessionMeta {
            name: v["name"].as_str().map(str::to_owned),
            status: v["status"].as_str().map(str::to_owned),
            cwd: v["cwd"].as_str().map(str::to_owned),
            session_id: v["sessionId"].as_str().map(str::to_owned),
            name_source: v["nameSource"].as_str().map(str::to_owned),
            context: None,
            context_tokens: None,
            context_window: None,
            first_prompt: None,
        };
        if let (Some(scwd), Some(sid)) = (m.cwd.as_deref(), m.session_id.as_deref()) {
            if let Some(c) = read_context(&account_path, scwd, sid) {
                m.context_tokens = Some(c.tokens);
                m.context_window = Some(c.window);
                m.context = Some((c.tokens / c.window).min(1.0));
            }
            m.first_prompt = cached_first_prompt(&account_path, scwd, sid);
        }
        m
    };

    // The CLI writes its registry entry a moment after the pty starts, so a
    // missing file is a normal early answer, not a failure.
    registry_entry(&dir, pid, &cwd, spawned_at_ms).map(|v| extract(&v))
}

/// Finds the CLI's registry entry for a pty session, by pid when that matches
/// and otherwise by cwd + start time (launcher shims give the pty a different
/// pid than the CLI process that writes the registry).
fn registry_entry(
    dir: &std::path::Path,
    pid: Option<u32>,
    cwd: &str,
    spawned_at_ms: u128,
) -> Option<serde_json::Value> {
    if let Some(pid) = pid {
        if let Some(v) = parse_session_file(&dir.join(format!("{pid}.json"))) {
            return Some(v);
        }
    }

    let want = norm_path(cwd);
    let mut best: Option<(u64, serde_json::Value)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        if let Some(v) = parse_session_file(&entry.path()) {
            let scwd = norm_path(v["cwd"].as_str().unwrap_or(""));
            let started = v["startedAt"].as_u64().unwrap_or(0);
            if scwd.starts_with(&want) && (started as u128) + 5000 >= spawned_at_ms {
                let updated = v["updatedAt"].as_u64().unwrap_or(started);
                if best.as_ref().map(|(u, _)| updated > *u).unwrap_or(true) {
                    best = Some((updated, v));
                }
            }
        }
    }
    best.map(|(_, v)| v)
}

#[tauri::command]
pub fn session_alive(state: PtyState, id: String) -> bool {
    let sessions = state.sessions.lock().unwrap();
    sessions
        .get(&id)
        .map(|s| s.alive.load(Ordering::SeqCst))
        .unwrap_or(false)
}
