//! What a Claude Code session leaves on disk, and how Luna reads it back:
//! the live registry at `<config>/sessions/<pid>.json` (name, status, cwd,
//! session id) and the transcript at `<config>/projects/<cwd>/<sid>.jsonl`
//! (context usage, the titles the CLI gave it, opening prompt). All of it is
//! free to read — no tokens.

use crate::paths::path_key_loose as norm_path;
use crate::pty::{sane_title, CliTitle, SessionMeta};
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

#[derive(Clone, Debug, PartialEq)]
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

/// The model of the last answer in a transcript: the id the CLI really used,
/// whatever alias it was started on. Read with the rest of its `Ending`: the
/// first turn's `prompt_snapshot` after it fills any short tail.
pub fn last_model(path: &Path) -> Option<String> {
    ending(path).model
}

/// The context the last answer in a transcript used, as (tokens, window): what
/// the pane's meter shows, read with the rest of its `Ending`.
pub fn last_context(path: &Path) -> Option<(f64, f64)> {
    ending(path).context.map(|c| (c.tokens, c.window))
}

/// See `last_model`: the CLI's own stand-in for a message no model wrote
/// reads `<synthetic>`.
fn model_in(text: &str) -> Option<String> {
    text.lines().rev().filter(|l| l.contains("\"model\"")).find_map(|line| {
        let v = serde_json::from_str::<serde_json::Value>(line).ok()?;
        if v["isSidechain"].as_bool().unwrap_or(false) {
            return None;
        }
        v["message"]["model"].as_str().filter(|m| !m.starts_with('<')).map(str::to_owned)
    })
}

/// The titles the CLI keeps in the transcript. Both are re-appended with the
/// session's other metadata as the transcript grows, so the last of each is
/// the current one and sits near the end.
#[derive(Default, Clone)]
struct Titles {
    /// `/rename`.
    custom: Option<String>,
    /// What the CLI generated from the opening of the conversation — the same
    /// title it puts on the terminal tab. Never a plan's handle.
    ai: Option<String>,
}

/// Accepting a plan makes the CLI write a second `ai-title`: the plan's
/// kebab-case handle (`fix-account-status-migration`), echoed by an
/// `agent-name` line. It is not a title anyone would give the chat, and in
/// every transcript on disk it is the only thing that ever replaced the first.
fn is_handle(s: &str) -> bool {
    s.contains('-') && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn titles_in(text: &str) -> Titles {
    let mut t = Titles::default();
    // The `agent-name` follows its `ai-title`, so reading backwards it is
    // known by the time the title it rules out comes up.
    let mut handles: Vec<String> = Vec::new();
    for line in text.lines().rev() {
        if t.custom.is_some() && t.ai.is_some() {
            break;
        }
        if !line.contains("-title\"") && !line.contains("\"agent-name\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        match v["type"].as_str() {
            Some("agent-name") => handles.extend(v["agentName"].as_str().map(str::to_owned)),
            Some("custom-title") if t.custom.is_none() => t.custom = v["customTitle"].as_str().and_then(sane_title),
            Some("ai-title") if t.ai.is_none() => {
                t.ai = v["aiTitle"]
                    .as_str()
                    .and_then(sane_title)
                    .filter(|s| !is_handle(s) && !handles.contains(s))
            }
            _ => {}
        }
    }
    t
}

fn title_cache() -> &'static Mutex<HashMap<String, Titles>> {
    static CACHE: std::sync::OnceLock<Mutex<HashMap<String, Titles>>> = std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// The titles of a live session. A long tool run can push the last title out
/// of the tail for a while, so the titles seen before are kept per session and
/// the tail only updates them. The first look reads the whole transcript: a
/// long session Luna has just restarted on may hold no title in its tail, and
/// the opening prompt would take the chat's name until one came round again.
fn titles_of(session_id: &str, path: &Path, tail: Option<&str>) -> Titles {
    let first_look = !title_cache().lock().unwrap().contains_key(session_id);
    let seen = if first_look {
        std::fs::read_to_string(path).ok().map(|s| titles_in(&s))
    } else {
        tail.map(titles_in)
    };
    remembered_titles(session_id, seen.unwrap_or_default())
}

/// How many conversations the per-conversation caches hold before starting
/// over: each `/clear` and resume is a new one, and the app runs for weeks.
const CACHE_CAP: usize = 512;

fn remembered_titles(session_id: &str, seen: Titles) -> Titles {
    let mut map = title_cache().lock().unwrap();
    if map.len() >= CACHE_CAP && !map.contains_key(session_id) {
        map.clear();
    }
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
pub fn saved_title(account_path: &str, session_id: &str) -> Option<CliTitle> {
    let path = find_transcript(account_path, session_id)?;
    let mut t = crate::pty::tail(&path, 256 * 1024).map(|s| titles_in(&s)).unwrap_or_default();
    if t.custom.is_none() && t.ai.is_none() {
        // Asked once per chat per run, so the whole file is affordable when
        // a session ended with its last title further back than the tail.
        t = titles_in(&std::fs::read_to_string(&path).ok()?);
    }
    match (t.custom, t.ai) {
        (Some(title), _) => Some(CliTitle { title, renamed: true }),
        (None, Some(title)) => Some(CliTitle { title, renamed: false }),
        (None, None) => None,
    }
}

/// A session's transcript by its id alone, in whichever project folder it is.
/// None for a session that never got one: Claude Code writes it with the
/// first message, so a chat closed before anything was typed has an id in
/// the registry and nothing on disk to resume.
pub fn find_transcript(account_path: &str, session_id: &str) -> Option<PathBuf> {
    if session_id.is_empty() || !session_id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return None;
    }
    let file = format!("{session_id}.jsonl");
    std::fs::read_dir(Path::new(account_path).join("projects"))
        .ok()?
        .flatten()
        .map(|e| e.path().join(&file))
        .find(|p| p.is_file())
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
            if map.len() >= CACHE_CAP {
                map.clear();
            }
            map.insert(session_id.to_string(), found.clone());
        }
    }
    found
}

/// The session's opening prompt, trimmed to a title — what the chat is called
/// for the few seconds before the CLI's own title lands in the transcript.
fn read_first_prompt(account_path: &str, cwd: &str, session_id: &str) -> Option<String> {
    use std::io::{BufRead, BufReader};
    let path = located_transcript(account_path, cwd, session_id);
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

fn parse_session_file(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Name, status, cwd, session id, context, title and opening prompt of a live
/// session, off the registry entry the CLI writes a moment after the pty
/// starts — so a missing entry is a normal early answer, not a failure.
pub fn meta(pid: Option<u32>, resume: Option<&str>, cwd: &str, spawned_at_ms: u128, account_path: &str) -> Option<SessionMeta> {
    let dir = Path::new(account_path).join("sessions");
    let v = registry_entry(&dir, pid, resume, cwd, spawned_at_ms)?;
    let mut m = SessionMeta {
        name: v["name"].as_str().and_then(sane_title),
        status: v["status"].as_str().map(str::to_owned),
        cwd: v["cwd"].as_str().map(str::to_owned),
        session_id: v["sessionId"].as_str().map(str::to_owned),
        name_source: v["nameSource"].as_str().map(str::to_owned),
        ..Default::default()
    };
    if let (Some(scwd), Some(sid)) = (m.cwd.clone(), m.session_id.clone()) {
        let path = located_transcript(account_path, &scwd, &sid);
        let text = crate::pty::tail(&path, 128 * 1024);
        let end = ending(&path);
        if let Some(c) = end.context {
            m.context_tokens = Some(c.tokens);
            m.context_window = Some(c.window);
            m.context = Some((c.tokens / c.window).min(1.0));
        }
        let titles = titles_of(&sid, &path, text.as_deref());
        // The registry name is the cwd folder when `derived` — a worktree's
        // random codename — and a kebab-case handle when `auto`, neither of
        // them a title; a rename there is as good as one in the transcript.
        let renamed = titles
            .custom
            .or_else(|| m.name.clone().filter(|_| m.name_source.as_deref() == Some("user")));
        m.title_renamed = renamed.is_some();
        m.title = renamed.or(titles.ai);
        m.first_prompt = cached_first_prompt(account_path, &scwd, &sid);
        m.last_said = end.last_said;
        m.last_said_at = end.last_said_at;
    }
    Some(m)
}

/// What the end of a transcript says of the session: the context its last
/// answer used and the model that wrote it, the text the model last wrote
/// since the last prompt — what a turn that is over ended on — and the API
/// error that turn ended on, if it did.
#[derive(Clone, Default)]
struct Ending {
    context: Option<ContextRead>,
    last_said: Option<String>,
    /// When those words were written, by their line's timestamp.
    last_said_at: Option<String>,
    model: Option<String>,
    api_error: Option<serde_json::Value>,
}

/// The transcript's `Ending`, read back from the end in growing pieces: after
/// a turn Claude Code writes lines of 100 KB and more — the first turn's whole
/// system prompt (an attachment, `prompt_snapshot`, 150 KB), `api-request-shape`
/// and `-blob` lines — which a short tail is all of. Kept per file length and
/// change time, as it is asked for every few seconds and changes only when the
/// file grows.
fn ending(path: &Path) -> Ending {
    type Stamp = (u64, Option<std::time::SystemTime>);
    static SEEN: std::sync::OnceLock<Mutex<HashMap<PathBuf, (Stamp, Ending)>>> = std::sync::OnceLock::new();
    let Ok(meta) = std::fs::metadata(path) else { return Ending::default() };
    let (len, stamp) = (meta.len(), (meta.len(), meta.modified().ok()));
    let seen = SEEN.get_or_init(Default::default);
    if let Some((at, end)) = seen.lock().unwrap_or_else(|e| e.into_inner()).get(path) {
        if *at == stamp {
            return end.clone();
        }
    }
    let mut end = Ending::default();
    let (mut said_known, mut error_known) = (false, false);
    for max in [128 * 1024, 1024 * 1024, 8 * 1024 * 1024] {
        let Some(text) = crate::pty::tail(path, max) else { break };
        if end.context.is_none() {
            end.context = context_in(&text);
        }
        if end.model.is_none() {
            end.model = model_in(&text);
        }
        if !said_known {
            if let Some(said) = last_said_in(&text) {
                (end.last_said, end.last_said_at) = said.map_or((None, None), |(said, at)| (Some(said), at));
                said_known = true;
            }
        }
        if !error_known {
            if let Some(error) = api_error_line(&text) {
                end.api_error = error;
                error_known = true;
            }
        }
        if (end.context.is_some() && end.model.is_some() && said_known && error_known) || len <= max {
            break;
        }
    }
    let mut map = seen.lock().unwrap_or_else(|e| e.into_inner());
    if map.len() >= CACHE_CAP {
        map.clear();
    }
    map.insert(path.to_path_buf(), (stamp, end.clone()));
    end
}

/// See `Ending`: Some(what was said, if anything) once the text reaches
/// back to the model's last words or to a prompt, None if it does not.
fn last_said_in(text: &str) -> Option<Option<(String, Option<String>)>> {
    for line in text.lines().rev() {
        if !line.contains("\"assistant\"") && !line.contains("\"user\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        if v["isSidechain"].as_bool() == Some(true) || v["isMeta"].as_bool() == Some(true) {
            continue;
        }
        let content = &v["message"]["content"];
        match v["type"].as_str() {
            Some("assistant") => {
                if v["isApiErrorMessage"].as_bool() == Some(true) || v["message"]["model"] == "<synthetic>" {
                    return Some(None);
                }
                let said: Vec<&str> = content.as_array().into_iter().flatten().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect();
                if !said.is_empty() {
                    return Some(Some((said.join("\n"), v["timestamp"].as_str().map(str::to_owned))));
                }
            }
            // A tool's result is the turn going on; anything else a prompt,
            // with nothing said since.
            Some("user") if content.as_array().is_some_and(|c| c.iter().any(|b| b["type"] == "tool_result")) => {}
            Some("user") => return Some(None),
            _ => {}
        }
    }
    None
}

/// Just the registry's status word for a live session — one small file, no
/// transcript — for the activity sampler, which asks every few seconds; with
/// when the CLI last set it (`statusUpdatedAt`, ms), where it says.
pub fn status(pid: Option<u32>, resume: Option<&str>, cwd: &str, spawned_at_ms: u128, account_path: &str) -> Option<(String, Option<u64>)> {
    let dir = Path::new(account_path).join("sessions");
    let v = registry_entry(&dir, pid, resume, cwd, spawned_at_ms)?;
    Some((v["status"].as_str()?.to_owned(), v["statusUpdatedAt"].as_u64()))
}

/// The cwd the CLI reports for a live pty session — under `--worktree` that
/// is the worktree, not the folder the session was spawned in.
pub fn session_cwd(pid: Option<u32>, resume: Option<&str>, cwd: &str, spawned_at_ms: u128, account_path: &str) -> Option<String> {
    let dir = Path::new(account_path).join("sessions");
    let v = registry_entry(&dir, pid, resume, cwd, spawned_at_ms)?;
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

/// `live_cwds`, of the entries whose process is still there: one killed leaves its
/// entry behind.
pub fn running_cwds(account_path: &str) -> Vec<String> {
    let dir = Path::new(account_path).join("sessions");
    let Ok(entries) = std::fs::read_dir(&dir) else { return vec![] };
    entries
        .flatten()
        .filter_map(|e| parse_session_file(&e.path()))
        .filter(|v| v["pid"].as_u64().is_some_and(|pid| crate::procs::alive(pid as u32)))
        .filter_map(|v| v["cwd"].as_str().map(str::to_owned))
        .collect()
}

/// The transcripts begun at or after `since_ms` by sessions that started in one of
/// these folders, each of which has a folder of its own under the account.
pub fn transcripts_since(account_path: &str, cwds: &[String], since_ms: u64) -> Vec<PathBuf> {
    let since = std::time::UNIX_EPOCH + std::time::Duration::from_millis(since_ms);
    let began = |p: &Path| std::fs::metadata(p).ok().and_then(|m| m.created().or_else(|_| m.modified()).ok()).is_some_and(|t| t >= since);
    cwds.iter()
        .filter_map(|cwd| std::fs::read_dir(Path::new(account_path).join("projects").join(encode_project_dir(cwd))).ok())
        .flat_map(|dir| dir.flatten().map(|e| e.path()).collect::<Vec<_>>())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl") && began(p))
        .collect()
}

/// Finds the CLI's registry entry for a pty session: by pid when that matches;
/// else, for a resumed one, by the session it was told to resume; else by cwd +
/// start time. Launcher shims give the pty a different pid than the CLI process
/// that writes the registry, and the entry comes a moment after the pty. Going
/// by those last two, anything of Claude Code's in the folder would look like
/// this chat: only a terminal session counts, as `entrypoint` says. `claude -p`,
/// a background helper, says `sdk-cli` (its `kind` is "interactive" all the
/// same); the SDKs `sdk-ts` and `sdk-py`, an editor's its own.
fn registry_entry(
    dir: &Path,
    pid: Option<u32>,
    resume: Option<&str>,
    cwd: &str,
    spawned_at_ms: u128,
) -> Option<serde_json::Value> {
    if let Some(pid) = pid {
        if let Some(v) = parse_session_file(&dir.join(format!("{pid}.json"))) {
            return Some(v);
        }
    }
    let entries: Vec<serde_json::Value> =
        std::fs::read_dir(dir).ok()?.flatten().filter_map(|entry| parse_session_file(&entry.path())).collect();
    if let Some(v) = resume.and_then(|sid| entries.iter().find(|v| v["sessionId"] == sid)) {
        return Some(v.clone());
    }

    // The folder itself or one under it (a `--worktree` session reports its
    // worktree), not a sibling that shares the prefix: `luna` is not `luna-site`.
    let want = norm_path(cwd);
    let under = format!("{want}{}", crate::paths::KEY_SEP);
    let mut best: Option<(u64, serde_json::Value)> = None;
    for v in entries {
        if v["entrypoint"] != "cli" {
            continue;
        }
        let scwd = norm_path(v["cwd"].as_str().unwrap_or(""));
        let started = v["startedAt"].as_u64().unwrap_or(0);
        if (scwd == want || scwd.starts_with(&under)) && (started as u128) + 5000 >= spawned_at_ms {
            let updated = v["updatedAt"].as_u64().unwrap_or(started);
            if best.as_ref().map(|(u, _)| updated > *u).unwrap_or(true) {
                best = Some((updated, v));
            }
        }
    }
    best.map(|(_, v)| v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_terminal_sessions_registry_entry_can_be_a_chats() {
        let dir = std::env::temp_dir().join(format!("luna-claude-registry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let entry = |pid: u32, sid: &str, entrypoint: Option<&str>, updated: u64| {
            let mut v = serde_json::json!({ "pid": pid, "sessionId": sid, "cwd": "E:\\p", "startedAt": 2000, "updatedAt": updated, "kind": "interactive" });
            if let Some(e) = entrypoint {
                v["entrypoint"] = e.into();
            }
            std::fs::write(dir.join(format!("{pid}.json")), v.to_string()).unwrap();
        };
        let found = |pid: u32, resume: Option<&str>| {
            registry_entry(&dir, Some(pid), resume, "E:\\p", 1000).map(|v| v["sessionId"].as_str().unwrap_or("").to_string())
        };
        // Each newer than any chat's, so each would win going by the time.
        for (pid, entrypoint) in [(101, Some("sdk-cli")), (102, Some("sdk-py")), (103, Some("sdk-ts")), (104, Some("claude-vscode")), (105, None)] {
            entry(pid, &format!("other-{pid}"), entrypoint, 9000);
        }
        assert_eq!(found(999, None), None, "none of them is this chat, whose entry is yet to come");
        entry(333, "a-chat", Some("cli"), 3000);
        assert_eq!(found(999, None).as_deref(), Some("a-chat"), "the shim's case still finds a terminal session's");
        assert_eq!(found(101, None).as_deref(), Some("other-101"), "its own pid is an exact answer");

        // A helper opened as a chat, behind a shim: what it resumes is exact,
        // over a newer chat in the folder.
        entry(444, "promoted", Some("cli"), 3500);
        entry(555, "newer-chat", Some("cli"), 8000);
        assert_eq!(found(999, None).as_deref(), Some("newer-chat"));
        assert_eq!(found(999, Some("promoted")).as_deref(), Some("promoted"));
        assert_eq!(found(999, Some("gone")).as_deref(), Some("newer-chat"), "a resume that took another id goes by the rest");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_model_is_the_main_loop_s_last() {
        let path = std::env::temp_dir().join(format!("luna-last-model-{}.jsonl", std::process::id()));
        std::fs::write(
            &path,
            concat!(
                r#"{"type":"assistant","message":{"model":"claude-opus-5-5","content":[]}}"#, "\n",
                r#"{"type":"assistant","message":{"model":"claude-haiku-5-5","content":[]}}"#, "\n",
                r#"{"type":"assistant","isSidechain":true,"message":{"model":"claude-sonnet-5-5","content":[]}}"#, "\n",
                r#"{"type":"assistant","message":{"model":"<synthetic>","content":[]}}"#, "\n",
                r#"{"type":"user","message":{"content":"the word \"model\" in passing"}}"#, "\n",
            ),
        )
        .unwrap();
        assert_eq!(last_model(&path).as_deref(), Some("claude-haiku-5-5"));
        let _ = std::fs::remove_file(&path);
        assert_eq!(last_model(&path), None, "no file");
    }

    #[test]
    fn a_turn_that_ended_on_a_usage_limit() {
        let limited = concat!(
            r#"{"type":"user","message":{"role":"user","content":"go on"}}"#, "\n",
            r#"{"type":"assistant","isApiErrorMessage":true,"error":"rate_limit","message":{"content":[{"type":"text","text":"You've hit your session limit"}]}}"#, "\n",
            r#"{"type":"system","subtype":"turn_duration"}"#, "\n",
        );
        assert_eq!(api_error_in(limited), Some("rate_limit"));
        // A prompt typed after the error is a new turn, not the stall.
        let retried = format!("{limited}{}\n", r#"{"type":"user","message":{"role":"user","content":"continue"}}"#);
        assert_eq!(api_error_in(&retried), None);
        // A sidechain's error is a subagent's, not the session's.
        let side = r#"{"type":"assistant","isSidechain":true,"isApiErrorMessage":true,"error":"rate_limit"}"#;
        assert_eq!(api_error_in(&format!("{}\n{side}\n", r#"{"type":"assistant","message":{}}"#)), None);
    }

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
    fn a_plan_handle_is_not_a_title() {
        // As a transcript has it after a plan is accepted.
        let text = concat!(
            r#"{"type":"ai-title","aiTitle":"Issue #5244","sessionId":"s"}"#, "\n",
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"ExitPlanMode"}]}}"#, "\n",
            r#"{"type":"ai-title","aiTitle":"fix-connector-logging-and-model-picker","sessionId":"s"}"#, "\n",
            r#"{"type":"agent-name","agentName":"fix-connector-logging-and-model-picker","sessionId":"s"}"#, "\n",
        );
        assert_eq!(titles_in(text).ai.as_deref(), Some("Issue #5244"));
        // The first title already out of the tail: nothing beats the handle.
        let tail = text.lines().skip(1).collect::<Vec<_>>().join("\n");
        assert_eq!(titles_in(&tail).ai, None);
        assert!(!is_handle("Fix login-flow"));
        assert!(!is_handle("Онборд нового юзера"));
    }

    #[test]
    fn keeps_a_title_the_tail_no_longer_shows() {
        let sid = format!("test-{}", std::process::id());
        let seen = Titles { ai: Some("Issue 4489".into()), custom: None };
        assert_eq!(remembered_titles(&sid, seen).ai.as_deref(), Some("Issue 4489"));
        assert_eq!(remembered_titles(&sid, Titles::default()).ai.as_deref(), Some("Issue 4489"));
    }

    #[test]
    fn the_first_look_reads_past_the_tail() {
        let sid = format!("first-look-{}", std::process::id());
        let path = std::env::temp_dir().join(format!("{sid}.jsonl"));
        std::fs::write(&path, r#"{"type":"ai-title","aiTitle":"Chat titles","sessionId":"s"}"#).unwrap();
        // A tail with no title in it, as a long tool run leaves one.
        assert_eq!(titles_of(&sid, &path, Some("{}")).ai.as_deref(), Some("Chat titles"));
        assert_eq!(titles_of(&sid, &path, Some("{}")).ai.as_deref(), Some("Chat titles"));
        let _ = std::fs::remove_file(&path);
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
            saved_title(&acct, "28c18d5b-64d5-40a8-a1ec-fb55573139d9"),
            Some(CliTitle { title: "Онборд нового юзера".into(), renamed: false })
        );
        assert_eq!(saved_title(&acct, "../../etc"), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// The API error the last turn of a live session ended on — a usage limit, a
/// lost login — when the conversation's last word is such an error. That is
/// how a session is seen to have stopped on one when no `StopFailure` hook
/// was heard: one that has sat on the limit dialog since before Luna started.
pub fn last_api_error(pid: Option<u32>, resume: Option<&str>, cwd: &str, spawned_at_ms: u128, account_path: &str) -> Option<&'static str> {
    let path = transcript_of(pid, resume, cwd, spawned_at_ms, account_path)?;
    let v = ending(&path).api_error?;
    Some(crate::activity::api_error_kind(v["error"].as_str().unwrap_or("")))
}

/// See `last_api_error`, of a text alone.
#[cfg(test)]
fn api_error_in(text: &str) -> Option<&'static str> {
    api_error_line(text).flatten().map(|v| crate::activity::api_error_kind(v["error"].as_str().unwrap_or("")))
}

/// The last user or assistant line decides: Some(the error line, if it is
/// one), or None when the text does not reach back to either.
fn api_error_line(text: &str) -> Option<Option<serde_json::Value>> {
    for line in text.lines().rev() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        if v["isSidechain"].as_bool() == Some(true) {
            continue;
        }
        match v["type"].as_str() {
            Some("assistant") if v["isApiErrorMessage"].as_bool() == Some(true) => return Some(Some(v)),
            Some("assistant") | Some("user") => return Some(None),
            _ => {}
        }
    }
    None
}

/// What a caller is told of the API error a transcript's last turn ended on,
/// when it did: the CLI's kind of error and the words it shows for it.
pub fn api_error_of(path: &Path) -> Option<String> {
    let v = ending(path).api_error?;
    let kind = crate::activity::api_error_kind(v["error"].as_str().unwrap_or(""));
    Some(match content_text(&v["message"]["content"]).filter(|t| !t.trim().is_empty()) {
        Some(said) => format!("{kind}: {said}"),
        None => kind.to_string(),
    })
}

/// The transcript of a session Luna knows by its id and cwd rather than
/// through the registry — a background helper's, which `claude -p` names in
/// its first event.
pub fn transcript_file(account_path: &str, cwd: &str, session_id: &str) -> PathBuf {
    located_transcript(account_path, cwd, session_id)
}

/// Where a session's transcript is: under its cwd, or, where that has none, in
/// whichever project folder holds it. A session resumed from another folder
/// keeps writing to the one it started in, and its cwd's folder holds only its
/// tool results. Not there yet (no message so far), it is still the file named
/// by the cwd. Where one was found is kept, as a transcript does not move and
/// the session's title and context are asked for every few seconds per pane.
fn located_transcript(account_path: &str, cwd: &str, session_id: &str) -> PathBuf {
    static ELSEWHERE: std::sync::OnceLock<Mutex<HashMap<String, PathBuf>>> = std::sync::OnceLock::new();
    let path = transcript_path(account_path, cwd, session_id);
    if path.is_file() {
        return path;
    }
    let elsewhere = ELSEWHERE.get_or_init(Default::default);
    let key = format!("{account_path}\n{session_id}");
    if let Some(found) = elsewhere.lock().unwrap_or_else(|e| e.into_inner()).get(&key).filter(|p| p.is_file()) {
        return found.clone();
    }
    let Some(found) = find_transcript(account_path, session_id) else { return path };
    let mut map = elsewhere.lock().unwrap_or_else(|e| e.into_inner());
    if map.len() >= CACHE_CAP {
        map.clear();
    }
    map.insert(key, found.clone());
    found
}

/// The tool call the conversation stands on:the last one the model made that
/// no result has come back for, as (name, input). `AskUserQuestion` waits on
/// a person by its nature; any other tool waits on one only while a
/// permission prompt is up — otherwise it is simply running.
pub fn pending_tool(path: &Path) -> Option<(String, serde_json::Value)> {
    pending_tool_in(&crate::pty::tail(path, 256 * 1024)?)
}

fn pending_tool_in(text: &str) -> Option<(String, serde_json::Value)> {
    let mut open: Vec<(String, String, serde_json::Value)> = Vec::new();
    for line in text.lines() {
        if !line.contains("\"assistant\"") && !line.contains("\"user\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        if v["isSidechain"].as_bool() == Some(true) || v["isMeta"].as_bool() == Some(true) {
            continue;
        }
        let content = &v["message"]["content"];
        match v["type"].as_str() {
            Some("assistant") => {
                for b in content.as_array().into_iter().flatten().filter(|b| b["type"] == "tool_use") {
                    let id = b["id"].as_str().unwrap_or("").to_string();
                    open.push((id, b["name"].as_str().unwrap_or("").to_string(), b["input"].clone()));
                }
            }
            Some("user") => {
                let results: Vec<&str> = content
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|b| b["type"] == "tool_result")
                    .filter_map(|b| b["tool_use_id"].as_str())
                    .collect();
                if results.is_empty() {
                    // A new prompt: whatever was left open went with the turn.
                    open.clear();
                } else {
                    open.retain(|(id, _, _)| !results.contains(&id.as_str()));
                }
            }
            _ => {}
        }
    }
    open.pop().map(|(_, name, input)| (name, input))
}

/// When the commands the conversation waits on are due, the latest of them, in
/// ms since the epoch: a `Bash` or `PowerShell` call by its `timeout` (2 min
/// without one), a `Monitor` by its `timeout_ms` (5 min) or the one its result
/// says it got, from the line that made it. One in the foreground is waited on
/// until its result. One with `run_in_background`, and a `Monitor`, come back at
/// once and run on until a task notification with a status names them (a
/// Monitor's events have none): `claude -p` holds its turn for them. A
/// background job with no `timeout` of its own runs as long as the CLI's flags
/// say, which is no deadline to read.
pub fn due_ms(path: &Path) -> Option<u64> {
    due_in(&crate::pty::tail(path, DUE_TAIL)?)
}

/// How far back the calls a quiet turn waits on are looked for: far enough to
/// reach past the large tool results that may have come after a long job's
/// start, and cheap, as it is read only once a turn has gone quiet.
pub(crate) const DUE_TAIL: u64 = 4 * 1024 * 1024;

fn due_in(text: &str) -> Option<u64> {
    struct Call {
        id: String,
        background: bool,
        /// The job a background call started, as its notifications name it.
        task: Option<String>,
        at: u64,
        due: u64,
    }
    let ms = |v: &serde_json::Value| v.as_f64().filter(|t| *t > 0.0).map(|t| t as u64);
    let mut open: Vec<Call> = Vec::new();
    for line in text.lines() {
        if !line.contains("\"assistant\"") && !line.contains("\"user\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        if v["isSidechain"].as_bool() == Some(true) || v["isMeta"].as_bool() == Some(true) {
            continue;
        }
        let content = &v["message"]["content"];
        match v["type"].as_str() {
            Some("assistant") => {
                let Some(at) = v["timestamp"].as_str().and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()) else { continue };
                let at = at.timestamp_millis().max(0) as u64;
                for b in content.as_array().into_iter().flatten().filter(|b| b["type"] == "tool_use") {
                    let input = &b["input"];
                    let (background, timeout) = match b["name"].as_str() {
                        Some("Bash" | "PowerShell") if input["run_in_background"] == true => (true, ms(&input["timeout"])),
                        Some("Bash" | "PowerShell") => (false, Some(ms(&input["timeout"]).unwrap_or(120_000))),
                        Some("Monitor") if input["persistent"] != true => (true, Some(ms(&input["timeout_ms"]).unwrap_or(300_000))),
                        _ => continue,
                    };
                    if let Some(timeout) = timeout {
                        let id = b["id"].as_str().unwrap_or("").to_string();
                        open.push(Call { id, background, task: None, at, due: at + timeout });
                    }
                }
            }
            Some("user") => {
                let results: Vec<&str> = content
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|b| b["type"] == "tool_result")
                    .filter_map(|b| b["tool_use_id"].as_str())
                    .collect();
                if !results.is_empty() {
                    // A background call whose result names no job started none. A
                    // Monitor's says the timeout it got, which a session may cap.
                    let made = &v["toolUseResult"];
                    let task = made["backgroundTaskId"].as_str().or(made["taskId"].as_str());
                    open.retain_mut(|c| {
                        if !results.contains(&c.id.as_str()) {
                            return true;
                        }
                        // A foreground call moved to the background, by hand or
                        // on its timeout, names its job too, and runs on as one.
                        c.task = task.map(str::to_owned);
                        c.background |= c.task.is_some();
                        if let Some(timeout) = ms(&made["timeoutMs"]) {
                            c.due = c.at + timeout;
                        }
                        c.task.is_some()
                    });
                    continue;
                }
                let said = content_text(content).unwrap_or_default();
                if said.contains("<task-notification>") {
                    if said.contains("<status>") {
                        let named = |c: &Call| {
                            said.contains(&format!("<tool-use-id>{}</tool-use-id>", c.id))
                                || c.task.as_ref().is_some_and(|t| said.contains(&format!("<task-id>{t}</task-id>")))
                        };
                        open.retain(|c| !named(c));
                    }
                } else {
                    // A new prompt: what ran in the foreground went with the turn.
                    open.retain(|c| c.background);
                }
            }
            _ => {}
        }
    }
    open.iter().map(|c| c.due).max()
}

/// The transcript file of a live session, once the registry knows it.
pub fn transcript_of(pid: Option<u32>, resume: Option<&str>, cwd: &str, spawned_at_ms: u128, account_path: &str) -> Option<PathBuf> {
    let dir = Path::new(account_path).join("sessions");
    let v = registry_entry(&dir, pid, resume, cwd, spawned_at_ms)?;
    let scwd = v["cwd"].as_str()?;
    let sid = v["sessionId"].as_str()?;
    Some(located_transcript(account_path, scwd, sid))
}

/// The conversation as another agent reads it: user and assistant turns from
/// byte offset `from` on, tool calls folded to one line each when asked for
/// and left out otherwise. Returns the messages and the offset to continue
/// from: the end of the last complete line, so a line still being written is
/// read whole next time rather than skipped. Subagent sidechains, CLI-injected
/// turns (`injected`) and the lines an API error writes in the model's place
/// ("You've hit your session limit", which `api_error_of` reports) are skipped:
/// neither the model nor whoever drives the session said any of them. A cursor
/// past the end is one into a file
/// since cut short or written anew, where it would land mid-line: the file is
/// read from the top.
pub fn messages_from(path: &Path, from: u64, with_tools: bool) -> (Vec<crate::agents::Message>, u64) {
    let from = if std::fs::metadata(path).is_ok_and(|m| from > m.len()) { 0 } else { from };
    let mut out = Vec::new();
    // A prompt the CLI put in itself is left out, but what answers it opens a
    // turn of its own (`Message::after_cli`).
    let mut cli_prompt = false;
    let read = crate::pty::complete_lines(path, from, |line| match message_of(line, with_tools) {
        Some(Line::Said(mut m)) => {
            if m.role == "assistant" {
                m.after_cli = std::mem::take(&mut cli_prompt);
            } else {
                cli_prompt = false;
            }
            out.push(m);
        }
        Some(Line::Injected) => cli_prompt = true,
        None => {}
    });
    (out, read)
}

/// Whether the conversation has taken in the stdin line `uuid`: a line that
/// opened a turn is a user line under that uuid, one taken in at a tool boundary
/// an attachment naming it. Its `queue-operation` lines are no sign: what is only
/// queued goes with the process. None if the file cannot be read.
pub fn took_line(path: &Path, uuid: &str) -> Option<bool> {
    std::fs::File::open(path).ok()?;
    let mut found = false;
    crate::pty::complete_lines(path, 0, |line| {
        if found || !line.contains(uuid) {
            return;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { return };
        let attached = &v["attachment"];
        found = (v["type"] == "user" && v["uuid"] == uuid) || (v["type"] == "attachment" && attached["type"] == "queued_command" && attached["source_uuid"] == uuid);
    });
    Some(found)
}

/// A transcript line that is a turn of the conversation.
enum Line {
    Said(crate::agents::Message),
    /// A prompt the CLI put in itself (`injected`).
    Injected,
}

/// One transcript line as a turn of the conversation, if it is one.
fn message_of(line: &str, with_tools: bool) -> Option<Line> {
    let v = serde_json::from_str::<serde_json::Value>(line).ok()?;
    if v["isSidechain"].as_bool().unwrap_or(false)
        || v["isMeta"].as_bool().unwrap_or(false)
        || v["isApiErrorMessage"].as_bool().unwrap_or(false)
    {
        return None;
    }
    let role = match v["type"].as_str() {
        Some("user") => "user",
        // The CLI's own stand-in reads `<synthetic>` ("No response requested."
        // on a resume after an interrupt).
        Some("assistant") if v["message"]["model"] == "<synthetic>" => return None,
        Some("assistant") => "assistant",
        _ => return None,
    };
    let content = &v["message"]["content"];
    let mut text = String::new();
    if let Some(s) = content.as_str() {
        text.push_str(s);
    } else if let Some(blocks) = content.as_array() {
        for b in blocks {
            match b["type"].as_str() {
                Some("text") => {
                    if let Some(t) = b["text"].as_str() {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(t);
                    }
                }
                Some("tool_use") if with_tools => {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(&format!("[tool: {}]", b["name"].as_str().unwrap_or("?")));
                }
                // tool_result blocks are the user side of a tool call:
                // bulk, and already reflected in what the assistant did.
                _ => {}
            }
        }
    }
    let text = text.trim().to_string();
    if text.is_empty() || text.starts_with("<local-command") || text.starts_with("<command-name") {
        return None;
    }
    if role == "user" && injected(&v, &text) {
        return Some(Line::Injected);
    }
    Some(Line::Said(crate::agents::Message { role: role.to_string(), text, after_cli: false }))
}

/// A user line Claude Code wrote itself, not one it was given: the end of a
/// background task it takes up in a turn of its own (`<task-notification>`),
/// a wake-up it scheduled, a compaction's summary, the marker an interrupt
/// leaves. Nobody asked anything there, so it is no prompt whose reply is
/// still to come. 2.1.316 says where a prompt came from (`turnOrigin`, these
/// being the ones it counts as no person's, and `promptSource`), and that word
/// stands: a person may paste a `<task-notification>` of their own. Its
/// interrupt marker says nothing of where it came from, nor does an older CLI's
/// line: those go by the text.
fn injected(v: &serde_json::Value, text: &str) -> bool {
    const OWN: [&str; 5] = ["scheduled", "task_notification", "auto_continuation", "host_synthetic", "system"];
    if v["isCompactSummary"].as_bool() == Some(true) || v["isVisibleInTranscriptOnly"].as_bool() == Some(true) || v["origin"]["kind"] == "task-notification" {
        return true;
    }
    if let Some(origin) = v["turnOrigin"].as_str() {
        return OWN.contains(&origin);
    }
    if let Some(source) = v["promptSource"].as_str() {
        return source == "system";
    }
    text.starts_with("<task-notification>") || text.starts_with("[Request interrupted by user")
}

#[cfg(test)]
mod reading_tests {
    use super::*;

    /// As Claude Code 2.1.314 writes a turn the API refused.
    const API_ERROR: &str = r#"{"parentUuid":"p","isSidechain":false,"type":"assistant","uuid":"u","message":{"id":"m","model":"<synthetic>","role":"assistant","content":[{"type":"text","text":"Invalid API key · Fix external API key"}]},"error":"authentication_failed","isApiErrorMessage":true,"apiErrorStatus":401}"#;

    #[test]
    fn an_api_error_is_no_message_and_says_why() {
        let path = std::env::temp_dir().join(format!("luna-claude-api-error-{}.jsonl", std::process::id()));
        let prompt = r#"{"type":"user","message":{"role":"user","content":"go on"}}"#;
        let said = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Starting."}]}}"#;
        std::fs::write(&path, format!("{prompt}\n{said}\n{API_ERROR}\n")).unwrap();
        let (msgs, _) = messages_from(&path, 0, false);
        let texts: Vec<&str> = msgs.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(texts, ["go on", "Starting."], "what the model said, and not what the CLI put in its place");
        assert_eq!(api_error_of(&path).as_deref(), Some("authentication_failed: Invalid API key · Fix external API key"));
        // A prompt since: that turn is another one.
        std::fs::write(&path, format!("{prompt}\n{API_ERROR}\n{prompt}\n")).unwrap();
        assert_eq!(api_error_of(&path), None);
        // Words of the model's after the error: it went on.
        std::fs::write(&path, format!("{API_ERROR}\n{said}\n")).unwrap();
        assert_eq!(api_error_of(&path), None);
        let _ = std::fs::remove_file(&path);
    }

    /// As Claude Code 2.1.316 writes what it puts in itself: a background task's
    /// end, which it takes up in a turn of its own, and the stand-in reply it
    /// writes on a resume after an interrupt.
    #[test]
    fn what_the_cli_puts_in_itself_is_nobodys_message() {
        let path = std::env::temp_dir().join(format!("luna-claude-injected-{}.jsonl", std::process::id()));
        let lines = [
            r#"{"type":"user","message":{"role":"user","content":"build it"},"promptSource":"sdk","turnOrigin":"sdk"}"#,
            r#"{"type":"assistant","message":{"model":"claude-opus-5-5","content":[{"type":"text","text":"Started the build in the background."}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":"<task-notification>\n<task-id>b1</task-id>\n<status>completed</status>\n</task-notification>"},"origin":{"kind":"task-notification","producer":"session-task"},"promptSource":"system","turnOrigin":"task_notification"}"#,
            r#"{"type":"assistant","message":{"model":"claude-opus-5-5","content":[{"type":"text","text":"The build passed."}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":"<task-notification>\n<task-id>b2</task-id>\n</task-notification>"}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"[Request interrupted by user]"}]}}"#,
            r#"{"type":"assistant","message":{"model":"<synthetic>","content":[{"type":"text","text":"No response requested."}]},"isApiErrorMessage":false}"#,
            r#"{"type":"user","message":{"role":"user","content":"This session is being continued from a previous conversation."},"isCompactSummary":true,"isVisibleInTranscriptOnly":true}"#,
        ];
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        let (msgs, _) = messages_from(&path, 0, false);
        let got: Vec<(&str, &str)> = msgs.iter().map(|m| (m.role.as_str(), m.text.as_str())).collect();
        assert_eq!(got, [("user", "build it"), ("assistant", "Started the build in the background."), ("assistant", "The build passed.")]);
        // A person's paste that reads like one: where the line says it came from wins.
        let pasted = [
            r#"{"type":"user","message":{"role":"user","content":"<task-notification> is what I saw, why?"},"promptSource":"typed","turnOrigin":"human"}"#,
            r#"{"type":"user","message":{"role":"user","content":"[Request interrupted by user] showed up, is that bad?"},"promptSource":"sdk"}"#,
        ];
        std::fs::write(&path, pasted.join("\n") + "\n").unwrap();
        assert_eq!(messages_from(&path, 0, false).0.len(), 2, "both are a person's");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_transcript_is_found_wherever_the_session_keeps_it() {
        let root = std::env::temp_dir().join(format!("luna-claude-located-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let acct = root.to_string_lossy().into_owned();
        let sid = "41448537-aaaa-4bbb-8ccc-0123456789ab";
        let sessions = root.join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(sessions.join("77.json"), format!(r#"{{"cwd":"E:\\p\\.claude\\worktrees\\w","sessionId":"{sid}"}}"#)).unwrap();
        let expected = transcript_path(&acct, "E:\\p\\.claude\\worktrees\\w", sid);
        // Nothing yet: the file it is going to write.
        assert_eq!(transcript_of(Some(77), None, "E:\\p", 0, &acct), Some(expected.clone()));
        // Resumed from another folder: its cwd's folder holds tool results only.
        std::fs::create_dir_all(expected.parent().unwrap().join("tool-results")).unwrap();
        let elsewhere = root.join("projects").join("E--p-app").join(format!("{sid}.jsonl"));
        std::fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
        std::fs::write(&elsewhere, "{}\n").unwrap();
        assert_eq!(transcript_of(Some(77), None, "E:\\p", 0, &acct), Some(elsewhere.clone()));
        assert_eq!(transcript_file(&acct, "E:\\p\\.claude\\worktrees\\w", sid), elsewhere);
        // And the chat's title and context are read from there too.
        std::fs::write(
            &elsewhere,
            concat!(
                r#"{"type":"user","message":{"role":"user","content":"look into the login bug"}}"#, "\n",
                r#"{"type":"ai-title","aiTitle":"Login bug","sessionId":"s"}"#, "\n",
                r#"{"type":"assistant","message":{"model":"claude-opus-5","usage":{"input_tokens":1000,"output_tokens":5},"content":[{"type":"text","text":"ok"}]}}"#, "\n",
            ),
        )
        .unwrap();
        let m = meta(Some(77), None, "E:\\p", 0, &acct).unwrap();
        assert_eq!(m.title.as_deref(), Some("Login bug"));
        assert_eq!(m.context_tokens, Some(1000.0));
        assert_eq!(m.first_prompt.as_deref(), Some("look into the login bug"));
        // Where the cwd says, once there is one there.
        std::fs::write(&expected, "{}\n").unwrap();
        assert_eq!(transcript_of(Some(77), None, "E:\\p", 0, &acct), Some(expected));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_pending_tool_is_the_last_one_without_a_result() {
        let ask = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t2","name":"AskUserQuestion","input":{"questions":[{"question":"Which colour?","options":[{"label":"Red"},{"label":"Blue"}],"multiSelect":false}]}}]}}"#;
        let ran = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}"#;
        let ran_done = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"a"}]}}"#;
        let text = [ran, ran_done, ask].join("\n");
        let (name, input) = pending_tool_in(&text).unwrap();
        assert_eq!(name, "AskUserQuestion");
        let a = crate::agents::Ask::from_claude_questions(&input).unwrap();
        assert_eq!((a.questions[0].text.as_str(), a.questions[0].options.len()), ("Which colour?", 2));
        let answered = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t2","content":"Blue"}]}}"#;
        assert_eq!(pending_tool_in(&[text.as_str(), answered].join("\n")), None);
        // A prompt typed after it: the turn it stood in is over.
        let prompt = r#"{"type":"user","message":{"role":"user","content":"never mind"}}"#;
        assert_eq!(pending_tool_in(&[ran, prompt].join("\n")), None);
        assert_eq!(pending_tool_in(ran).map(|(n, _)| n).as_deref(), Some("Bash"));
    }

    /// As Claude Code 2.1.318 writes its commands: a background one's result
    /// names the job (`backgroundTaskId`, a Monitor's `taskId`), and the task
    /// notification of its end has a status, a Monitor's events none.
    #[test]
    fn a_command_is_due_by_its_own_timeout() {
        let at = "2026-11-04T15:10:00.000Z";
        let start = chrono::DateTime::parse_from_rfc3339(at).unwrap().timestamp_millis() as u64;
        let call = |id: &str, name: &str, input: &str| {
            format!(r#"{{"type":"assistant","timestamp":"{at}","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"{name}","input":{input}}}]}}}}"#)
        };
        let result = |id: &str, made: &str| format!(r#"{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"{id}","content":"…"}}]}},"toolUseResult":{made}}}"#);
        let notice = |body: &str| {
            let text = serde_json::to_string(&format!("<task-notification>\n{body}\n</task-notification>")).unwrap();
            format!(r#"{{"type":"user","message":{{"role":"user","content":{text}}},"origin":{{"kind":"task-notification"}}}}"#)
        };
        let due = |lines: &[&str]| due_in(&lines.join("\n")).map(|d| d - start);
        let min = 60_000;

        let lane = call("t1", "Bash", r#"{"command":"./lane.sh","timeout":2400000}"#);
        assert_eq!(due(&[&lane]), Some(40 * min));
        assert_eq!(due(&[&lane, &result("t1", "{}")]), None, "its result ends it");
        assert_eq!(due(&[&call("t1", "PowerShell", r#"{"command":"ls"}"#)]), Some(2 * min), "the CLI's own default");
        let prompt = r#"{"type":"user","message":{"role":"user","content":"never mind"}}"#;
        assert_eq!(due(&[&lane, prompt]), None, "it went with its turn");
        assert_eq!(due(&[&call("t1", "mcp__x__run", r#"{"timeout":9000000}"#)]), None, "not a command of the CLI's");

        let job = call("t2", "Bash", r#"{"command":"./lane.sh","run_in_background":true,"timeout":3000000}"#);
        let started = result("t2", r#"{"stdout":"","backgroundTaskId":"b7"}"#);
        assert_eq!(due(&[&lane, &job, &started]), Some(50 * min), "the latest; a job runs on past its result");
        assert_eq!(due(&[&job, &started, prompt]), Some(50 * min), "and past its turn");
        assert_eq!(due(&[&job, &result("t2", r#"{"stdout":"","stderr":"refused"}"#)]), None, "one refused never ran");
        let ended = notice("<task-id>b7</task-id>\n<tool-use-id>t2</tool-use-id>\n<status>completed</status>");
        assert_eq!(due(&[&job, &started, &ended]), None);
        let open_ended = call("t2", "Bash", r#"{"command":"./lane.sh","run_in_background":true}"#);
        assert_eq!(due(&[&open_ended]), None, "no deadline of its own to read");
        // Moved to the background after it started: its result names a job,
        // which runs on with the deadline it was given.
        let moved = result("t1", r#"{"stdout":"","backgroundTaskId":"b9"}"#);
        assert_eq!(due(&[&lane, &moved, prompt]), Some(40 * min));
        let moved_over = notice("<task-id>b9</task-id>\n<status>completed</status>");
        assert_eq!(due(&[&lane, &moved, &moved_over]), None);

        let watch = call("t3", "Monitor", r#"{"command":"tail -f lane.log","timeout_ms":3600000}"#);
        assert_eq!(due(&[&watch]), Some(60 * min));
        let armed = result("t3", r#"{"taskId":"m4","timeoutMs":1800000,"persistent":false}"#);
        assert_eq!(due(&[&watch, &armed]), Some(30 * min), "as capped");
        let event = notice("<task-id>m4</task-id>\n<summary>Monitor event</summary>\n<event>STEP build exit=0</event>");
        assert_eq!(due(&[&watch, &armed, &event]), Some(30 * min), "an event is no end");
        let over = notice("<task-id>m4</task-id>\n<status>completed</status>");
        assert_eq!(due(&[&watch, &armed, &event, &over]), None);
        assert_eq!(due(&[&call("t3", "Monitor", r#"{"command":"tail -f x","persistent":true}"#)]), None);
    }

    #[test]
    fn the_cursor_stops_before_a_line_still_being_written() {
        let path = std::env::temp_dir().join(format!("luna-claude-read-{}.jsonl", std::process::id()));
        let first = r#"{"type":"user","message":{"role":"user","content":"ping"}}"#;
        let half = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"po"#;
        std::fs::write(&path, format!("{first}\n{half}")).unwrap();
        let (msgs, cursor) = messages_from(&path, 0, false);
        assert_eq!(msgs, vec![crate::agents::Message { role: "user".into(), text: "ping".into(), after_cli: false }]);
        assert_eq!(cursor, first.len() as u64 + 1, "the half line is not consumed");
        // Finished: the next read from the cursor has it whole.
        std::fs::write(&path, format!("{first}\n{half}ng\"}}]}}}}\n")).unwrap();
        let (msgs, next) = messages_from(&path, cursor, false);
        assert_eq!(msgs, vec![crate::agents::Message { role: "assistant".into(), text: "pong".into(), after_cli: false }]);
        assert_eq!(next, std::fs::metadata(&path).unwrap().len());
        // Written anew, shorter: a cursor past its end reads it from the top.
        std::fs::write(&path, format!("{first}\n")).unwrap();
        let (msgs, _) = messages_from(&path, next, false);
        assert_eq!(msgs, vec![crate::agents::Message { role: "user".into(), text: "ping".into(), after_cli: false }]);
        let _ = std::fs::remove_file(&path);
    }

    /// A background task's end Claude Code takes up is left out, and what
    /// answers it starts a turn of its own.
    #[test]
    fn what_answers_the_clis_own_prompt_opens_a_turn() {
        let path = std::env::temp_dir().join(format!("luna-claude-after-cli-{}.jsonl", std::process::id()));
        let lines = [
            r#"{"type":"user","message":{"role":"user","content":"verify it"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"The lane runs."}]}}"#,
            r#"{"type":"user","origin":{"kind":"task-notification"},"message":{"role":"user","content":"<task-notification>\n<status>completed</status>\n</task-notification>"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Green."}]}}"#,
        ];
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        let (msgs, _) = messages_from(&path, 0, false);
        let _ = std::fs::remove_file(&path);
        let seen: Vec<(&str, &str, bool)> = msgs.iter().map(|m| (m.role.as_str(), m.text.as_str(), m.after_cli)).collect();
        assert_eq!(seen, [("user", "verify it", false), ("assistant", "The lane runs.", false), ("assistant", "Green.", true)]);
    }

    #[test]
    fn a_turn_ends_on_its_last_words() {
        let prompt = r#"{"type":"user","message":{"role":"user","content":"which one?"}}"#;
        let said = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Pick one.\nSTATUS: AWAITING OPERATOR"}]}}"#;
        let tool = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"mcp__luna__luna_notify","input":{"text":"done"}}]}}"#;
        let result = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}"#;
        let title = r#"{"type":"ai-title","aiTitle":"x"}"#;
        let said_of = |text: &str| last_said_in(text).map(|said| said.map(|(said, _)| said));
        assert_eq!(said_of(&[prompt, said, tool, result, title].join("\n")), Some(Some("Pick one.\nSTATUS: AWAITING OPERATOR".into())));
        assert_eq!(said_of(&[said, prompt].join("\n")), Some(None), "a new prompt: nothing said to it yet");
        assert_eq!(said_of(&[prompt, said, API_ERROR].join("\n")), Some(None), "the API's words, not the model's");
        assert_eq!(said_of(title), None, "not far enough back to tell");

        // As Claude Code 2.1.3xx ends a turn: its system prompt, written after
        // the last words and the usage they came with, outgrows a short tail.
        let path = std::env::temp_dir().join(format!("luna-claude-last-said-{}.jsonl", std::process::id()));
        let answer = r#"{"type":"assistant","message":{"model":"claude-sonnet-5-5","usage":{"input_tokens":10,"cache_read_input_tokens":20000},"content":[{"type":"text","text":"Pick one.\nSTATUS: AWAITING OPERATOR"}]}}"#;
        let snapshot = format!(r#"{{"type":"attachment","attachment":{{"type":"prompt_snapshot","systemPrompt":["{}"]}}}}"#, "x".repeat(300 * 1024));
        std::fs::write(&path, [prompt, answer, &snapshot, title].join("\n") + "\n").unwrap();
        let end = ending(&path);
        assert_eq!(end.last_said.as_deref(), Some("Pick one.\nSTATUS: AWAITING OPERATOR"));
        assert_eq!(end.context.map(|c| c.tokens), Some(20010.0));
        assert_eq!(last_model(&path).as_deref(), Some("claude-sonnet-5-5"), "the model, past the snapshot too");
        assert!(end.api_error.is_none());
        let _ = std::fs::remove_file(&path);

        // A first turn that ended on an API error has its snapshot after it all the same.
        let path = std::env::temp_dir().join(format!("luna-claude-error-snapshot-{}.jsonl", std::process::id()));
        std::fs::write(&path, [prompt, API_ERROR, &snapshot, title].join("\n") + "\n").unwrap();
        let said = api_error_of(&path).expect("the error, past the snapshot");
        assert!(said.ends_with("Invalid API key · Fix external API key"), "{said}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_line_counts_as_taken_in_by_a_user_line_or_its_attachment_not_by_the_queue() {
        let dir = std::env::temp_dir().join(format!("luna-took-line-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.jsonl");
        let lines = [
            r#"{"type":"queue-operation","operation":"enqueue","content":"only queued 11111111"}"#,
            r#"{"type":"user","uuid":"22222222","message":{"role":"user","content":"it opened a turn"}}"#,
            r#"{"type":"attachment","attachment":{"type":"queued_command","prompt":"folded","source_uuid":"33333333"}}"#,
            r#"{"type":"assistant","uuid":"44444444","message":{"content":[{"type":"text","text":"says 11111111"}]}}"#,
        ];
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        assert_eq!(took_line(&path, "11111111"), Some(false));
        assert_eq!(took_line(&path, "22222222"), Some(true));
        assert_eq!(took_line(&path, "33333333"), Some(true));
        assert_eq!(took_line(&path, "44444444"), Some(false), "not a prompt");
        assert_eq!(took_line(&dir.join("gone.jsonl"), "22222222"), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
