//! What a Claude Code session leaves on disk, and how Luna reads it back:
//! the live registry at `<config>/sessions/<pid>.json` (name, status, cwd,
//! session id) and the transcript at `<config>/projects/<cwd>/<sid>.jsonl`
//! (context usage, opening prompt). All of it is free to read — no tokens.

use crate::pty::{sane_title, SessionMeta};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

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
fn transcript_path(account_path: &str, cwd: &str, session_id: &str) -> PathBuf {
    Path::new(account_path)
        .join("projects")
        .join(encode_project_dir(cwd))
        .join(format!("{session_id}.jsonl"))
}

fn read_context(account_path: &str, cwd: &str, session_id: &str) -> Option<ContextRead> {
    let path = transcript_path(account_path, cwd, session_id);
    let text = crate::pty::tail(&path, 128 * 1024)?;
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
                    window: super::models::context_window(model),
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
        // `isMeta` marks a turn the CLI injected rather than one the user
        // typed — a hook's instructions, a resumed-session preamble. One of
        // those titled a chat "A session-scoped Stop hook is now active…".
        if v["type"] != "user"
            || v["isSidechain"].as_bool().unwrap_or(false)
            || v["isMeta"].as_bool().unwrap_or(false)
        {
            continue;
        }
        let Some(text) = content_text(&v["message"]["content"]) else { continue };
        if let Some(t) = crate::pty::title_from_prompt(&text) {
            return Some(t);
        }
    }
    None
}

fn norm_path(p: &str) -> String {
    p.replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

fn parse_session_file(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Name, status, cwd, session id, context and opening prompt of a live session,
/// off the registry entry the CLI writes a moment after the pty starts — so a
/// missing entry is a normal early answer, not a failure.
pub fn meta(pid: Option<u32>, cwd: &str, spawned_at_ms: u128, account_path: &str) -> Option<SessionMeta> {
    let dir = Path::new(account_path).join("sessions");
    let v = registry_entry(&dir, pid, cwd, spawned_at_ms)?;
    let mut m = SessionMeta {
        name: v["name"].as_str().and_then(sane_title),
        status: v["status"].as_str().map(str::to_owned),
        cwd: v["cwd"].as_str().map(str::to_owned),
        session_id: v["sessionId"].as_str().map(str::to_owned),
        name_source: v["nameSource"].as_str().map(str::to_owned),
        ..Default::default()
    };
    if let (Some(scwd), Some(sid)) = (m.cwd.clone(), m.session_id.clone()) {
        if let Some(c) = read_context(account_path, &scwd, &sid) {
            m.context_tokens = Some(c.tokens);
            m.context_window = Some(c.window);
            m.context = Some((c.tokens / c.window).min(1.0));
        }
        m.first_prompt = cached_first_prompt(account_path, &scwd, &sid);
    }
    Some(m)
}

/// The cwd the CLI reports for a live pty session — under `--worktree` that
/// is the worktree, not the folder the session was spawned in.
pub fn session_cwd(pid: Option<u32>, cwd: &str, spawned_at_ms: u128, account_path: &str) -> Option<String> {
    let dir = Path::new(account_path).join("sessions");
    let v = registry_entry(&dir, pid, cwd, spawned_at_ms)?;
    v["cwd"].as_str().map(str::to_owned)
}

/// Every cwd the CLI currently has a session in, per this account's registry.
pub fn live_cwds(account_path: &str) -> Vec<String> {
    let dir = Path::new(account_path).join("sessions");
    let Ok(entries) = std::fs::read_dir(&dir) else { return vec![] };
    entries
        .flatten()
        .filter_map(|e| parse_session_file(&e.path()))
        .filter_map(|v| v["cwd"].as_str().map(str::to_owned))
        .collect()
}

/// Finds the CLI's registry entry for a pty session, by pid when that matches
/// and otherwise by cwd + start time (launcher shims give the pty a different
/// pid than the CLI process that writes the registry).
fn registry_entry(
    dir: &Path,
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
