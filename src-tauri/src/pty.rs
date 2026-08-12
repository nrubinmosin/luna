use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, PtySize};
use serde::Serialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, State};

const SCROLLBACK_CAP: usize = 2 * 1024 * 1024;

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
    writer: Box<dyn Write + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    scrollback: Arc<Mutex<Vec<u8>>>,
    alive: Arc<AtomicBool>,
    pid: Option<u32>,
    cwd: String,
    spawned_at_ms: u128,
}

#[derive(Default)]
pub struct PtyManager {
    sessions: Mutex<HashMap<String, Session>>,
}

pub type PtyState<'a> = State<'a, PtyManager>;

#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub fn ensure_session(
    app: AppHandle,
    state: PtyState,
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
            let buf = s.scrollback.lock().unwrap();
            return Ok(String::from_utf8_lossy(&buf).into_owned());
        }
        sessions.remove(&id);
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

    let mut cmd = CommandBuilder::new("claude");
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

    let child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
    drop(pair.slave);

    let pid = child.process_id();
    let spawned_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let killer = child.clone_killer();
    let mut child = child;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;
    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;

    let scrollback = Arc::new(Mutex::new(Vec::new()));
    let alive = Arc::new(AtomicBool::new(true));

    {
        let app = app.clone();
        let id = id.clone();
        let scrollback = Arc::clone(&scrollback);
        let alive = Arc::clone(&alive);
        std::thread::spawn(move || {
            let mut chunk = [0u8; 8192];
            // Carry bytes of a UTF-8 sequence split across reads.
            let mut carry: Vec<u8> = Vec::new();
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        {
                            let mut sb = scrollback.lock().unwrap();
                            sb.extend_from_slice(&chunk[..n]);
                            let len = sb.len();
                            if len > SCROLLBACK_CAP {
                                sb.drain(..len - SCROLLBACK_CAP);
                            }
                        }
                        carry.extend_from_slice(&chunk[..n]);
                        let valid_to = match std::str::from_utf8(&carry) {
                            Ok(_) => carry.len(),
                            Err(e) => e.valid_up_to(),
                        };
                        if valid_to > 0 {
                            let text = unsafe { std::str::from_utf8_unchecked(&carry[..valid_to]) };
                            let _ = app.emit("pty://output", PtyOutput { id: &id, data: text });
                        }
                        carry.drain(..valid_to);
                        if carry.len() > 4 {
                            carry.clear(); // not a split sequence, just invalid bytes
                        }
                    }
                }
            }
            alive.store(false, Ordering::SeqCst);
            let code = child.wait().ok().map(|st| st.exit_code());
            let _ = app.emit("pty://exit", PtyExit { id: &id, code });
        });
    }

    sessions.insert(
        id,
        Session {
            master: pair.master,
            writer,
            killer,
            scrollback,
            alive,
            pid,
            cwd: folder,
            spawned_at_ms,
        },
    );

    Ok(String::new())
}

#[tauri::command]
pub fn write_session(state: PtyState, id: String, data: String) -> Result<(), String> {
    let mut sessions = state.sessions.lock().unwrap();
    let s = sessions.get_mut(&id).ok_or("no such session")?;
    s.writer.write_all(data.as_bytes()).map_err(|e| e.to_string())
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

#[tauri::command]
pub fn kill_session(state: PtyState, id: String) -> Result<(), String> {
    let mut sessions = state.sessions.lock().unwrap();
    if let Some(mut s) = sessions.remove(&id) {
        let _ = s.killer.kill();
    }
    Ok(())
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SessionMeta {
    pub name: Option<String>,
    pub status: Option<String>,
    pub cwd: Option<String>,
    pub session_id: Option<String>,
    pub context: Option<f64>,
    pub context_tokens: Option<f64>,
}

// Claude Code encodes a project cwd into a transcript folder name by replacing
// every non-alphanumeric character with '-'.
fn encode_project_dir(p: &str) -> String {
    p.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

const CONTEXT_WINDOW: f64 = 200_000.0;

// Free context reading: the last assistant message in the session transcript
// carries cumulative input-side token usage. Returns raw tokens; the fraction
// is derived from it.
fn read_context_tokens(account_path: &str, cwd: &str, session_id: &str) -> Option<f64> {
    use std::io::{Read, Seek, SeekFrom};
    let path = std::path::Path::new(account_path)
        .join("projects")
        .join(encode_project_dir(cwd))
        .join(format!("{session_id}.jsonl"));
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
                return Some(total);
            }
        }
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
#[tauri::command]
pub fn session_meta(state: PtyState, id: String, account_path: String) -> Result<SessionMeta, String> {
    let (pid, cwd, spawned_at_ms) = {
        let sessions = state.sessions.lock().unwrap();
        let s = sessions.get(&id).ok_or("no such session")?;
        if !s.alive.load(Ordering::SeqCst) {
            return Err("session exited".into());
        }
        (s.pid, s.cwd.clone(), s.spawned_at_ms)
    };

    let dir = std::path::Path::new(&account_path).join("sessions");
    let extract = |v: &serde_json::Value| {
        let mut m = SessionMeta {
            name: v["name"].as_str().map(str::to_owned),
            status: v["status"].as_str().map(str::to_owned),
            cwd: v["cwd"].as_str().map(str::to_owned),
            session_id: v["sessionId"].as_str().map(str::to_owned),
            context: None,
            context_tokens: None,
        };
        if let (Some(scwd), Some(sid)) = (m.cwd.as_deref(), m.session_id.as_deref()) {
            m.context_tokens = read_context_tokens(&account_path, scwd, sid);
            m.context = m.context_tokens.map(|t| (t / CONTEXT_WINDOW).min(1.0));
        }
        m
    };

    if let Some(pid) = pid {
        let f = dir.join(format!("{pid}.json"));
        if let Some(v) = parse_session_file(&f) {
            return Ok(extract(&v));
        }
    }

    // The CLI process pid may differ from the pty child (launcher shims);
    // fall back to matching by cwd (chat folder or its worktree) + start time.
    let want = norm_path(&cwd);
    let mut best: Option<(u64, serde_json::Value)> = None;
    for entry in std::fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten() {
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
    best.map(|(_, v)| extract(&v)).ok_or_else(|| "no session file".into())
}

#[tauri::command]
pub fn session_alive(state: PtyState, id: String) -> bool {
    let sessions = state.sessions.lock().unwrap();
    sessions
        .get(&id)
        .map(|s| s.alive.load(Ordering::SeqCst))
        .unwrap_or(false)
}
