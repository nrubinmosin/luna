//! What a Claude Code session leaves on disk, and how Luna reads it back:
//! the live registry at `<config>/sessions/<pid>.json` (name, status, cwd,
//! session id) and the transcript at `<config>/projects/<cwd>/<sid>.jsonl`
//! (context usage, the titles the CLI gave it, opening prompt). All of it is
//! free to read — no tokens.

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

fn transcript_path(account_path: &str, cwd: &str, session_id: &str) -> PathBuf {
    Path::new(account_path)
        .join("projects")
        .join(encode_project_dir(cwd))
        .join(format!("{session_id}.jsonl"))
}

// Free context reading: the last assistant message in the session transcript
// carries cumulative input-side token usage. Returns raw tokens; the fraction
// is derived from it.
fn context_in(text: &str) -> Option<ContextRead> {
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

/// The titles the CLI keeps in the transcript. Both are re-appended with the
/// session's other metadata as the transcript grows, so the last of each is
/// the current one and sits near the end.
#[derive(Default, Clone)]
struct Titles {
    /// `/rename`.
    custom: Option<String>,
    /// What the CLI generated from the opening of the conversation — the same
    /// title it puts on the terminal tab.
    ai: Option<String>,
}

fn titles_in(text: &str) -> Titles {
    let mut t = Titles::default();
    for line in text.lines().rev() {
        if t.custom.is_some() && t.ai.is_some() {
            break;
        }
        if !line.contains("-title\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        match v["type"].as_str() {
            Some("custom-title") if t.custom.is_none() => t.custom = v["customTitle"].as_str().and_then(sane_title),
            Some("ai-title") if t.ai.is_none() => t.ai = v["aiTitle"].as_str().and_then(sane_title),
            _ => {}
        }
    }
    t
}

/// A long tool run can push the last title out of the tail for a while, so
/// the titles seen before are kept per session and the tail only updates them.
fn remembered_titles(session_id: &str, seen: Titles) -> Titles {
    static CACHE: std::sync::OnceLock<Mutex<HashMap<String, Titles>>> = std::sync::OnceLock::new();
    let mut map = CACHE.get_or_init(Default::default).lock().unwrap();
    let t = map.entry(session_id.to_string()).or_default();
    if seen.custom.is_some() {
        t.custom = seen.custom;
    }
    if seen.ai.is_some() {
        t.ai = seen.ai;
    }
    t.clone()
}

/// The CLI's title for a session that is not running — a chat restored from
/// an earlier run, which session_meta cannot see. The transcript is found by
/// name: the folder it sits in comes from a cwd that, for a worktree session,
/// is long gone.
pub fn saved_title(account_path: &str, session_id: &str) -> Option<String> {
    if session_id.is_empty() || !session_id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return None;
    }
    let file = format!("{session_id}.jsonl");
    let path = std::fs::read_dir(Path::new(account_path).join("projects"))
        .ok()?
        .flatten()
        .map(|e| e.path().join(&file))
        .find(|p| p.is_file())?;
    let mut t = crate::pty::tail(&path, 256 * 1024).map(|s| titles_in(&s)).unwrap_or_default();
    if t.custom.is_none() && t.ai.is_none() {
        // Asked once per chat per run, so the whole file is affordable when
        // a session ended with its last title further back than the tail.
        t = titles_in(&std::fs::read_to_string(&path).ok()?);
    }
    t.custom.or(t.ai)
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

/// The session's opening prompt, trimmed to a title — what the chat is called
/// for the few seconds before the CLI's own title lands in the transcript.
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

/// Name, status, cwd, session id, context, title and opening prompt of a live
/// session, off the registry entry the CLI writes a moment after the pty
/// starts — so a missing entry is a normal early answer, not a failure.
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
        let text = crate::pty::tail(&transcript_path(account_path, &scwd, &sid), 128 * 1024);
        if let Some(c) = text.as_deref().and_then(context_in) {
            m.context_tokens = Some(c.tokens);
            m.context_window = Some(c.window);
            m.context = Some((c.tokens / c.window).min(1.0));
        }
        let titles = remembered_titles(&sid, text.as_deref().map(titles_in).unwrap_or_default());
        // The registry name is the cwd folder when `derived` — a worktree's
        // random codename — and a kebab-case handle when `auto`; a rename
        // there is as good as one in the transcript.
        let registry = |source: &str| m.name.clone().filter(|_| m.name_source.as_deref() == Some(source));
        m.title = titles.custom.or_else(|| registry("user")).or(titles.ai).or_else(|| registry("auto"));
        m.first_prompt = cached_first_prompt(account_path, &scwd, &sid);
    }
    Some(m)
}

/// Just the registry's status word for a live session — one small file, no
/// transcript — for the activity sampler, which asks every few seconds.
pub fn status(pid: Option<u32>, cwd: &str, spawned_at_ms: u128, account_path: &str) -> Option<String> {
    let dir = Path::new(account_path).join("sessions");
    let v = registry_entry(&dir, pid, cwd, spawned_at_ms)?;
    v["status"].as_str().map(str::to_owned)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_latest_titles_from_the_tail() {
        let text = concat!(
            r#"{"type":"ai-title","aiTitle":"First guess","sessionId":"s"}"#, "\n",
            r#"{"type":"user","message":{"role":"user","content":"mention \"type\":\"ai-title\" in passing"}}"#, "\n",
            r#"{"type":"ai-title","aiTitle":"Token refresh and chat titles","sessionId":"s"}"#, "\n",
            r#"{"type":"last-prompt","lastPrompt":"…","sessionId":"s"}"#, "\n",
        );
        let t = titles_in(text);
        assert_eq!(t.ai.as_deref(), Some("Token refresh and chat titles"));
        assert_eq!(t.custom, None);

        let renamed = format!("{text}{}\n", r#"{"type":"custom-title","customTitle":"Luna auth","sessionId":"s"}"#);
        let t = titles_in(&renamed);
        assert_eq!(t.custom.as_deref(), Some("Luna auth"));
        assert_eq!(t.ai.as_deref(), Some("Token refresh and chat titles"));
    }

    #[test]
    fn keeps_a_title_the_tail_no_longer_shows() {
        let sid = format!("test-{}", std::process::id());
        let seen = Titles { ai: Some("Issue 4489".into()), custom: None };
        assert_eq!(remembered_titles(&sid, seen).ai.as_deref(), Some("Issue 4489"));
        assert_eq!(remembered_titles(&sid, Titles::default()).ai.as_deref(), Some("Issue 4489"));
    }

    #[test]
    fn finds_a_saved_title_by_session_id() {
        let root = std::env::temp_dir().join(format!("luna-claude-title-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("projects").join("E--p--claude-worktrees-gone");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("28c18d5b-64d5-40a8-a1ec-fb55573139d9.jsonl"),
            r#"{"type":"ai-title","aiTitle":"Онборд нового юзера","sessionId":"28c18d5b-64d5-40a8-a1ec-fb55573139d9"}"#,
        )
        .unwrap();
        let acct = root.to_string_lossy();
        assert_eq!(
            saved_title(&acct, "28c18d5b-64d5-40a8-a1ec-fb55573139d9").as_deref(),
            Some("Онборд нового юзера")
        );
        assert_eq!(saved_title(&acct, "../../etc"), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
