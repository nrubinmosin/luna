//! What a Codex session leaves on disk, and how Luna reads it back.
//!
//! Codex has no live registry; it writes one rollout file per thread at
//! `<CODEX_HOME>/sessions/YYYY/MM/DD/rollout-<local ts>-<uuid>.jsonl`, one
//! JSON object per line: `{"timestamp","type","payload"}`. The first line is
//! `session_meta` (thread id, cwd); `turn_context` lines carry the model;
//! `event_msg` lines mark the turn — `task_started` / `task_complete` since
//! 0.157, `turn_started` / `turn_complete` / `turn_aborted` before — and carry
//! `token_count` (context usage plus rate limits); `response_item` lines of
//! type `message` are the conversation itself, `role` user / assistant /
//! developer with `input_text` / `output_text` blocks. All of it is free to
//! read — no tokens.
//!
//! Which file is ours is the one thing to work out: a fresh session gets the
//! newest rollout whose cwd matches and that appeared after the spawn, and a
//! resumed one is found by the uuid in its name. Either way the answer is
//! remembered per chat, so two chats in one folder never share a file.

use crate::pty::{sane_title, tail, title_from_prompt, CliTitle, SessionMeta};
use chrono::{Datelike, Local, NaiveDate, NaiveDateTime, TimeZone, Timelike};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// A turn is in flight but the screen has not changed for this long: Codex
/// paints its spinner continuously while it works, so a still screen mid-turn
/// is an approval prompt (which the rollout does not record).
const STILL_MS: u64 = 3000;

pub struct Live<'a> {
    pub chat_id: &'a str,
    pub cwd: &'a str,
    pub spawned_at_ms: u128,
    /// The thread id the session was resumed with, if any.
    pub resume: Option<&'a str>,
    pub account_path: &'a str,
    pub last_output_ms: u64,
}

fn sessions_dir(account_path: &str) -> PathBuf {
    Path::new(account_path).join("sessions")
}

fn norm_path(p: &str) -> String {
    p.replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

/// `rollout-2026-09-17T14-03-22-<uuid>.jsonl` → (local time, uuid). A name
/// with a `_<rollout id>` suffix (after a `/revert`) still names the thread.
pub fn parse_name(file_name: &str) -> Option<(NaiveDateTime, String)> {
    let rest = file_name.strip_prefix("rollout-")?.strip_suffix(".jsonl")?;
    // "YYYY-MM-DDTHH-MM-SS" is 19 chars, then '-' then the uuid. Any file
    // in the folder is looked at, so the cut must land on a char boundary.
    if rest.len() < 21 || !rest.is_char_boundary(19) {
        return None;
    }
    let (ts, id) = rest.split_at(19);
    let ts = NaiveDateTime::parse_from_str(ts, "%Y-%m-%dT%H-%M-%S").ok()?;
    let id = id.strip_prefix('-')?;
    let id = id.split('_').next()?.to_string();
    (!id.is_empty()).then_some((ts, id))
}

fn first_line(path: &Path) -> Option<Value> {
    use std::io::{BufRead, BufReader};
    let f = std::fs::File::open(path).ok()?;
    let mut line = String::new();
    BufReader::new(f).read_line(&mut line).ok()?;
    serde_json::from_str(&line).ok()
}

fn day_dir(account_path: &str, day: NaiveDate) -> PathBuf {
    sessions_dir(account_path)
        .join(day.year().to_string())
        .join(format!("{:02}", day.month()))
        .join(format!("{:02}", day.day()))
}

/// Rollout files written on the given days, newest first.
fn rollouts_on(account_path: &str, days: impl Iterator<Item = NaiveDate>) -> Vec<(NaiveDateTime, String, PathBuf)> {
    let mut out = vec![];
    for day in days {
        let Ok(entries) = std::fs::read_dir(day_dir(account_path, day)) else { continue };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if let Some((ts, id)) = parse_name(&name) {
                out.push((ts, id, e.path()));
            }
        }
    }
    out.sort_by(|a, b| b.0.cmp(&a.0));
    out
}

fn days_back(from: NaiveDate, n: usize) -> impl Iterator<Item = NaiveDate> {
    (0..n as i64).filter_map(move |i| from.checked_sub_days(chrono::Days::new(i as u64)))
}

/// The rollout of a thread, by id, wherever it was written. Walks day
/// directories newest first and stops at the first hit; a thread months old
/// costs a longer walk once, after which the answer is cached.
fn rollout_by_id(account_path: &str, id: &str) -> Option<PathBuf> {
    let root = sessions_dir(account_path);
    let mut years: Vec<PathBuf> = std::fs::read_dir(&root).ok()?.flatten().map(|e| e.path()).collect();
    years.sort();
    for y in years.into_iter().rev() {
        let Ok(months) = std::fs::read_dir(&y) else { continue };
        let mut months: Vec<PathBuf> = months.flatten().map(|e| e.path()).collect();
        months.sort();
        for m in months.into_iter().rev() {
            let Ok(days) = std::fs::read_dir(&m) else { continue };
            let mut days: Vec<PathBuf> = days.flatten().map(|e| e.path()).collect();
            days.sort();
            for d in days.into_iter().rev() {
                let Ok(files) = std::fs::read_dir(&d) else { continue };
                for f in files.flatten() {
                    let name = f.file_name().to_string_lossy().into_owned();
                    if parse_name(&name).map(|(_, i)| i == id).unwrap_or(false) {
                        return Some(f.path());
                    }
                }
            }
        }
    }
    None
}

/// The rollout of a thread no live pty session claims — a background
/// helper's, whose `codex exec` names the thread on stdout. Remembered once
/// found, as a thread's file never moves.
pub fn rollout_of_thread(account_path: &str, id: &str) -> Option<PathBuf> {
    static FOUND: OnceLock<Mutex<HashMap<String, PathBuf>>> = OnceLock::new();
    let found = FOUND.get_or_init(Default::default);
    if let Some(p) = found.lock().ok()?.get(id).filter(|p| p.is_file()) {
        return Some(p.clone());
    }
    let path = rollout_by_id(account_path, id)?;
    found.lock().ok()?.insert(id.to_string(), path.clone());
    Some(path)
}

/// Where a thread works, per its rollout: a `--worktree` one names the
/// checkout Codex made.
pub fn thread_cwd(account_path: &str, id: &str) -> Option<String> {
    first_line(&rollout_of_thread(account_path, id)?)?["payload"]["cwd"].as_str().map(str::to_owned)
}

fn claims() -> &'static Mutex<HashMap<String, PathBuf>> {
    static C: OnceLock<Mutex<HashMap<String, PathBuf>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// Forget a chat's rollout, e.g. when its session is respawned.
pub fn release(chat_id: &str) {
    if let Ok(mut m) = claims().lock() {
        m.remove(chat_id);
    }
    if let Ok(mut m) = turn_counts().lock() {
        m.remove(chat_id);
    }
}

/// What the turn tracker of each chat has read of its rollout, and where it
/// stopped.
struct TurnCount {
    path: PathBuf,
    /// Bytes consumed: the end of the last complete line.
    offset: u64,
    scan: Scan,
}

/// The rollout read so far, as the tracker keeps it.
#[derive(Default, Clone)]
struct Scan {
    /// Turns ended since the process was spawned.
    completed: u32,
    /// Whether the last turn event read started a turn that has not ended;
    /// None before any.
    working: Option<bool>,
    /// The question the model last put to the user that nothing has answered
    /// yet, with the call id it came in.
    ask: Option<(String, crate::agents::Ask)>,
}

fn turn_counts() -> &'static Mutex<HashMap<String, TurnCount>> {
    static C: OnceLock<Mutex<HashMap<String, TurnCount>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// The rollout read on from where the last look stopped. Counted off the
/// file rather than off sampled state, so a turn that starts and ends between
/// two samples of the activity tracker still counts — a one-word answer takes
/// two seconds, the sampler looks every five — and read whole rather than by
/// its tail, which a long turn outgrows: a megabyte of tool output after
/// `task_started` and the tail read the session as resting. None until the
/// rollout exists, i.e. before the first turn.
fn tracked(live: &Live) -> Option<Scan> {
    let path = rollout_for(live)?;
    let mut counts = turn_counts().lock().ok()?;
    let entry = counts
        .entry(live.chat_id.to_string())
        .or_insert_with(|| TurnCount { path: path.clone(), offset: 0, scan: Scan::default() });
    if entry.path != path {
        *entry = TurnCount { path: path.clone(), offset: 0, scan: Scan::default() };
    }
    entry.offset = scan_rollout(&path, entry.offset, live.spawned_at_ms as i64, &mut entry.scan);
    Some(entry.scan.clone())
}

/// How many turns this process of the session has finished, per its rollout:
/// one per `task_complete` (`turn_complete` / `turn_aborted` before 0.157)
/// written since it was spawned — a resumed thread's rollout holds the turns
/// of every earlier run too.
pub fn turns_completed(live: &Live) -> Option<u32> {
    tracked(live).map(|s| s.completed)
}

/// The question the session has put to the user and nobody has answered:
/// `request_user_input`, which holds the turn until it is answered, or
/// `request_user_input_async`, which lets the model work on and is open until
/// the user next says something.
pub fn pending_ask(live: &Live) -> Option<crate::agents::Ask> {
    tracked(live)?.ask.map(|(_, a)| a)
}

/// Reads the complete lines from byte `from` on into `scan` and returns the
/// offset just past the last of them. Turn ends count only when written at or
/// after `since_ms`; a line whose timestamp does not read is taken as new.
fn scan_rollout(path: &Path, from: u64, since_ms: i64, scan: &mut Scan) -> u64 {
    crate::pty::complete_lines(path, from, |line| {
        let event = line.contains("\"event_msg\"");
        if !event && !line.contains("request_user_input") && !line.contains("\"function_call_output\"") {
            return;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { return };
        let payload = &v["payload"];
        match (v["type"].as_str(), payload["type"].as_str()) {
            (Some("event_msg"), Some("task_started" | "turn_started")) => scan.working = Some(true),
            (Some("event_msg"), Some("task_complete" | "turn_complete" | "turn_aborted" | "task_aborted")) => {
                scan.working = Some(false);
                // A blocking question goes with the turn that asked it.
                if scan.ask.as_ref().is_some_and(|(_, a)| a.blocking) {
                    scan.ask = None;
                }
                let new = v["timestamp"]
                    .as_str()
                    .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                    .map_or(true, |t| t.timestamp_millis() >= since_ms);
                if new {
                    scan.completed += 1;
                }
            }
            // Whatever the user says next is the answer, typed or picked:
            // `user_message` before 0.163, a completed `UserMessage` item since.
            (Some("event_msg"), Some("user_message")) => scan.ask = None,
            (Some("event_msg"), Some("item_completed")) if payload["item"]["type"] == "UserMessage" => scan.ask = None,
            (Some("response_item"), Some("function_call")) => {
                let blocking = match payload["name"].as_str() {
                    Some("request_user_input") => true,
                    Some("request_user_input_async") => false,
                    _ => return,
                };
                let args = payload["arguments"].as_str().and_then(|a| serde_json::from_str::<Value>(a).ok());
                if let Some(ask) = args.as_ref().and_then(|a| ask_of(a, blocking)) {
                    scan.ask = Some((payload["call_id"].as_str().unwrap_or("").to_string(), ask));
                }
            }
            (Some("response_item"), Some("function_call_output")) => {
                let answered = scan.ask.as_ref().is_some_and(|(id, a)| a.blocking && payload["call_id"].as_str() == Some(id));
                if answered {
                    scan.ask = None;
                }
            }
            _ => {}
        }
    })
}

/// `{"questions":[…]}` as Codex's question tools take it: each with its text
/// under `question` (`title` in the async one) and the suggested answers
/// under `options`.
fn ask_of(args: &Value, blocking: bool) -> Option<crate::agents::Ask> {
    let questions: Vec<crate::agents::AskQuestion> = args["questions"]
        .as_array()?
        .iter()
        .filter_map(|q| {
            let text = q["question"].as_str().or(q["title"].as_str())?.trim().to_string();
            let options = q["options"]
                .as_array()
                .map(|o| o.iter().filter_map(|o| o["label"].as_str().or(o.as_str()).map(str::to_owned)).collect())
                .unwrap_or_default();
            (!text.is_empty()).then_some(crate::agents::AskQuestion { text, options, multi: false, list_at: None })
        })
        .collect();
    (!questions.is_empty()).then(|| crate::agents::Ask::question(questions, blocking))
}

/// The rollout file this live session writes to, once it exists.
pub fn rollout_for(live: &Live) -> Option<PathBuf> {
    if let Ok(m) = claims().lock() {
        if let Some(p) = m.get(live.chat_id) {
            if p.is_file() {
                return Some(p.clone());
            }
        }
    }

    // A resumed thread is found by its id, which says whose file it is.
    if let Some(id) = live.resume {
        let found = rollout_by_id(live.account_path, id)?;
        if let Ok(mut m) = claims().lock() {
            m.insert(live.chat_id.to_string(), found.clone());
        }
        return Some(found);
    }

    let spawned = Local
        .timestamp_millis_opt(live.spawned_at_ms as i64)
        .single()
        .map(|t| t.naive_local())?;
    // Codex writes the file right after start; allow the clock to disagree
    // by a few seconds either way.
    let not_before = spawned - chrono::Duration::seconds(5);
    let taken: Vec<PathBuf> = claims()
        .lock()
        .map(|m| m.iter().filter(|(k, _)| *k != live.chat_id).map(|(_, v)| v.clone()).collect())
        .unwrap_or_default();
    let want = norm_path(live.cwd);
    // Under `--worktree` the rollout names the checkout Codex made, which
    // points back at the folder the session was started in.
    let ours = |cwd: &str| {
        norm_path(cwd) == want
            || (crate::worktree::codex_shaped(cwd)
                && crate::worktree::project_of(cwd).is_some_and(|p| norm_path(&p) == want))
    };
    let today = Local::now().date_naive();
    let mut candidates: Vec<(NaiveDateTime, PathBuf)> = rollouts_on(live.account_path, days_back(today, 2))
        .into_iter()
        .filter(|(ts, _, p)| *ts >= not_before && !taken.contains(p))
        .filter(|(_, _, p)| {
            first_line(p).and_then(|v| v["payload"]["cwd"].as_str().map(ours)).unwrap_or(false)
        })
        .map(|(ts, _, p)| (ts, p))
        .collect();
    // Earliest after the spawn: the one that appeared when this session
    // started, not one a later chat in the same folder made. A file named in
    // the second the session started or later goes first: one from just
    // before it is a chat started a moment earlier that has not claimed its
    // own yet.
    let started = spawned.with_nanosecond(0).unwrap_or(spawned);
    candidates.sort_by_key(|(ts, _)| (*ts < started, *ts));
    let (_, found) = candidates.into_iter().next()?;

    // A guessed rollout is one chat's. The look above ran without the lock,
    // and another chat may have claimed the same file since: the first claim
    // stands, and this chat looks again next time.
    let mut m = claims().lock().ok()?;
    if m.iter().any(|(k, v)| *k != live.chat_id && *v == found) {
        return None;
    }
    m.insert(live.chat_id.to_string(), found.clone());
    Some(found)
}

/// The opening prompt never changes, but session_meta asks every few seconds
/// per pane — so read it once and remember the answer.
fn cached_first_prompt(path: &Path) -> Option<String> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Option<String>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Ok(map) = cache.lock() {
        if let Some(hit) = map.get(path) {
            return hit.clone();
        }
    }
    let found = read_first_prompt(path);
    if found.is_some() {
        if let Ok(mut map) = cache.lock() {
            // Each rollout is a conversation; the app runs for weeks.
            if map.len() >= 512 {
                map.clear();
            }
            map.insert(path.to_path_buf(), found.clone());
        }
    }
    found
}

fn read_first_prompt(path: &Path) -> Option<String> {
    use std::io::{BufRead, BufReader};
    let f = std::fs::File::open(path).ok()?;
    for line in BufReader::new(f).lines().take(200).map_while(Result::ok) {
        if !line.contains("\"message\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        let Some(m) = message_of(&v) else { continue };
        if m.role != "user" {
            continue;
        }
        if let Some(t) = title_from_prompt(&m.text) {
            return Some(t);
        }
    }
    None
}

/// A rollout line as one turn of the conversation, if it is one: a
/// `response_item` of type `message` with a user or assistant role. Developer
/// messages (skills, agent-mode preambles) and the context Codex injects as
/// a user turn — `AGENTS.md`, `<environment_context>` — are not conversation
/// and are left out, the way `luna_read` leaves out Claude Code's meta turns.
fn message_of(v: &Value) -> Option<crate::agents::Message> {
    if v["type"] != "response_item" || v["payload"]["type"] != "message" {
        return None;
    }
    let role = match v["payload"]["role"].as_str()? {
        "user" => "user",
        "assistant" => "assistant",
        _ => return None,
    };
    let mut text = String::new();
    for block in v["payload"]["content"].as_array()? {
        let Some(t) = block["text"].as_str() else { continue };
        if matches!(block["type"].as_str(), Some("input_text") | Some("output_text") | Some("text")) {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(t);
        }
    }
    let text = text.trim().to_string();
    if text.is_empty() || (role == "user" && (text.starts_with('<') || text.starts_with("# AGENTS.md"))) {
        return None;
    }
    Some(crate::agents::Message { role: role.to_string(), text })
}

/// The thread's name, if it has one. Codex appends `{id, thread_name,
/// updated_at}` to `<CODEX_HOME>/session_index.jsonl` whenever a thread is
/// named, so the last line for an id is the current name. It names a thread
/// once on its own, from the opening prompt; a name that differs from that
/// first one is the user's `/rename`.
pub fn thread_title(account_path: &str, thread_id: &str) -> Option<CliTitle> {
    let text = std::fs::read_to_string(Path::new(account_path).join("session_index.jsonl")).ok()?;
    let names: Vec<String> = text
        .lines()
        .filter(|l| l.contains(thread_id))
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| v["id"] == thread_id)
        .filter_map(|v| v["thread_name"].as_str().and_then(sane_title))
        .collect();
    let title = names.last()?.clone();
    Some(CliTitle { renamed: title != names[0], title })
}

/// What the tail of a rollout says right now.
#[derive(Default)]
pub struct Tail {
    pub model: Option<String>,
    pub context_tokens: Option<f64>,
    pub context_window: Option<f64>,
    /// The last `rate_limits` snapshot and the timestamp of its line.
    pub rate_limits: Option<(Value, String)>,
}

pub fn read_tail(path: &Path) -> Tail {
    let mut out = Tail::default();
    let Some(text) = tail(path, 128 * 1024) else { return out };
    // The first line of a tail read is usually a fragment; a parse failure
    // just skips it.
    for line in text.lines().rev() {
        let interesting = line.contains("\"event_msg\"") || line.contains("\"turn_context\"");
        if !interesting {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let payload = &v["payload"];
        match v["type"].as_str() {
            Some("turn_context") => {
                if out.model.is_none() {
                    out.model = payload["model"].as_str().map(str::to_owned);
                }
            }
            Some("event_msg") => match payload["type"].as_str() {
                Some("token_count") => {
                    if out.context_tokens.is_none() {
                        let info = &payload["info"];
                        if info.is_object() {
                            let last = &info["last_token_usage"];
                            let total = last["total_tokens"].as_f64().unwrap_or(0.0);
                            let reasoning = last["reasoning_output_tokens"].as_f64().unwrap_or(0.0);
                            let tokens = (total - reasoning).max(0.0);
                            if tokens > 0.0 {
                                out.context_tokens = Some(tokens);
                                out.context_window = info["model_context_window"].as_f64();
                            }
                        }
                    }
                    if out.rate_limits.is_none() && payload["rate_limits"].is_object() {
                        out.rate_limits = Some((
                            payload["rate_limits"].clone(),
                            v["timestamp"].as_str().unwrap_or("").to_string(),
                        ));
                    }
                }
                _ => {}
            },
            _ => {}
        }
        if out.model.is_some() && out.context_tokens.is_some() && out.rate_limits.is_some() {
            break;
        }
    }
    out
}

/// Whether a Codex screen shows an approval dialog — a command to run, an edit
/// to make, a permission to grant. The rollout records none of them, and the
/// screen does not go quiet under one (the quiet-screen guess below misses
/// it), but each ends in the same footer.
pub fn asks(screen: &str) -> bool {
    const MARKS: [&str; 3] = [
        "Press enter to confirm or esc to cancel",
        "Would you like to run the following command?",
        "Would you like to make the following edits?",
    ];
    MARKS.iter().any(|m| screen.contains(m))
}

/// `working` | `waiting` | `resting` off the rollout's turn events and the
/// screen.
fn fold_status(working: Option<bool>, last_output_ms: u64) -> &'static str {
    match working {
        Some(true) => {
            let still = crate::throttle::now_ms().saturating_sub(last_output_ms) > STILL_MS;
            if still {
                "waiting"
            } else {
                "working"
            }
        }
        _ => "resting",
    }
}

/// Just the status of a live session, for the activity sampler.
pub fn status(live: &Live) -> Option<String> {
    Some(fold_status(tracked(live)?.working, live.last_output_ms).to_string())
}

/// Where the session works, per its rollout: the folder it was started in,
/// or the worktree `--worktree` made for it.
pub fn rollout_cwd(live: &Live) -> Option<String> {
    first_line(&rollout_for(live)?)?["payload"]["cwd"].as_str().map(str::to_owned)
}

/// Status, thread id, model, context, name and opening prompt of a live session.
/// None until the rollout file exists, which is a normal early answer.
pub fn meta(live: &Live) -> Option<SessionMeta> {
    let path = rollout_for(live)?;
    let name = path.file_name()?.to_string_lossy().into_owned();
    let (_, id) = parse_name(&name)?;
    let t = read_tail(&path);

    let status = Some(fold_status(tracked(live).and_then(|s| s.working), live.last_output_ms));
    let named = thread_title(live.account_path, &id);

    let mut m = SessionMeta {
        status: status.map(str::to_owned),
        cwd: first_line(&path)
            .and_then(|v| v["payload"]["cwd"].as_str().map(str::to_owned))
            .or_else(|| Some(live.cwd.to_string())),
        title_renamed: named.as_ref().is_some_and(|n| n.renamed),
        title: named.map(|n| n.title),
        session_id: Some(id),
        model: t.model,
        first_prompt: cached_first_prompt(&path).and_then(|s| sane_title(&s)),
        ..Default::default()
    };
    if let (Some(tokens), Some(window)) = (t.context_tokens, t.context_window) {
        if window > 0.0 {
            m.context_tokens = Some(tokens);
            m.context_window = Some(window);
            m.context = Some((tokens / window).min(1.0));
        }
    }
    Some(m)
}

/// The newest rate-limit snapshot any rollout of this account recorded in the
/// last couple of days, with the line's timestamp. Looked for in the files
/// written to last first: a name carries when the thread started, and a chat
/// started yesterday that is still running has newer numbers than a helper
/// that started and ended an hour ago.
pub fn latest_rate_limits(account_path: &str) -> Option<(Value, String)> {
    let today = Local::now().date_naive();
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = rollouts_on(account_path, days_back(today, 2))
        .into_iter()
        .map(|(_, _, p)| (std::fs::metadata(&p).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH), p))
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, path) in files {
        if let Some(hit) = read_tail(&path).rate_limits {
            return Some(hit);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rollout_names() {
        let (ts, id) = parse_name("rollout-2026-09-17T14-03-22-0192a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b.jsonl").unwrap();
        assert_eq!(ts.to_string(), "2026-09-17 14:03:22");
        assert_eq!(id, "0192a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b");
        let (_, id) = parse_name("rollout-2026-09-17T14-03-22-abc_def.jsonl").unwrap();
        assert_eq!(id, "abc");
        assert!(parse_name("notes.txt").is_none());
        assert!(parse_name("rollout-garbage.jsonl").is_none());
    }

    #[test]
    fn reads_the_tail() {
        let root = std::env::temp_dir().join(format!("luna-codex-tail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("rollout-2026-09-17T10-00-00-abc.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"timestamp":"2026-09-17T10:00:00.000Z","type":"session_meta","payload":{"id":"abc","cwd":"E:\\p"}}"#, "\n",
                r#"{"timestamp":"2026-09-17T10:00:01.000Z","type":"turn_context","payload":{"model":"gpt-5-codex","cwd":"E:\\p"}}"#, "\n",
                r#"{"timestamp":"2026-09-17T10:00:02.000Z","type":"event_msg","payload":{"type":"user_message","message":"  fix   the build\nplease "}}"#, "\n",
                r#"{"timestamp":"2026-09-17T10:00:02.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"  fix   the build\nplease "}]}}"#, "\n",
                r#"{"timestamp":"2026-09-17T10:00:02.000Z","type":"event_msg","payload":{"type":"turn_started","turn_id":"t1"}}"#, "\n",
                r#"{"timestamp":"2026-09-17T10:00:09.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":9000},"last_token_usage":{"input_tokens":4000,"output_tokens":600,"reasoning_output_tokens":500,"total_tokens":4600},"model_context_window":258400},"rate_limits":{"primary":{"used_percent":12.5,"window_minutes":300,"resets_at":1789000000},"secondary":null,"plan_type":"plus"}}}"#, "\n",
                r#"{"timestamp":"2026-09-17T10:00:10.000Z","type":"event_msg","payload":{"type":"turn_complete","turn_id":"t1"}}"#, "\n",
            ),
        )
        .unwrap();

        let t = read_tail(&path);
        assert_eq!(t.model.as_deref(), Some("gpt-5-codex"));
        assert_eq!(t.context_tokens, Some(4100.0));
        assert_eq!(t.context_window, Some(258400.0));
        let (rl, ts) = t.rate_limits.unwrap();
        assert_eq!(rl["primary"]["used_percent"], 12.5);
        assert_eq!(ts, "2026-09-17T10:00:09.000Z");
        assert_eq!(read_first_prompt(&path).as_deref(), Some("fix the build please"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn two_chats_in_one_folder_get_a_rollout_each() {
        let root = std::env::temp_dir().join(format!("luna-codex-claims-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let acct = root.to_string_lossy().into_owned();
        let t0 = crate::throttle::now_ms() - 60_000;
        // A rollout appears a second after its chat starts; chat b starts two
        // seconds after chat a.
        let write = |at_ms: u64, uuid: &str| {
            let ts = Local.timestamp_millis_opt(at_ms as i64).single().unwrap().naive_local();
            let dir = day_dir(&acct, ts.date());
            std::fs::create_dir_all(&dir).unwrap();
            let name = format!("rollout-{}-{uuid}.jsonl", ts.format("%Y-%m-%dT%H-%M-%S"));
            std::fs::write(dir.join(name), format!("{}\n", r#"{"type":"session_meta","payload":{"cwd":"E:\\p"}}"#)).unwrap();
        };
        write(t0 + 1_000, "aaaa");
        write(t0 + 3_000, "bbbb");
        let (a, b, c) = (format!("claim-a-{t0}"), format!("claim-b-{t0}"), format!("claim-c-{t0}"));
        let live = |chat: &str, at_ms: u64| {
            rollout_for(&Live { chat_id: chat, cwd: "E:\\p", spawned_at_ms: at_ms as u128, resume: None, account_path: &acct, last_output_ms: 0 })
                .and_then(|p| parse_name(&p.file_name()?.to_string_lossy()).map(|(_, id)| id))
        };
        // b looks first: the file of the chat started just before it is not b's.
        assert_eq!(live(b.as_str(), t0 + 2_000).as_deref(), Some("bbbb"));
        assert_eq!(live(a.as_str(), t0).as_deref(), Some("aaaa"));
        // Both taken: a third chat in the folder gets neither.
        assert_eq!(live(c.as_str(), t0), None);
        for chat in [a, b, c] {
            release(&chat);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reads_the_latest_thread_name() {
        let root = std::env::temp_dir().join(format!("luna-codex-names-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("session_index.jsonl"),
            concat!(
                r#"{"id":"abc","thread_name":"First name","updated_at":"2026-09-17T10:00:00Z"}"#, "\n",
                r#"{"id":"other","thread_name":"Not ours","updated_at":"2026-09-17T10:01:00Z"}"#, "\n",
                r#"{"id":"abc","thread_name":"Renamed","updated_at":"2026-09-17T10:02:00Z"}"#, "\n",
            ),
        )
        .unwrap();
        let acct = root.to_string_lossy();
        assert_eq!(thread_title(&acct, "abc"), Some(CliTitle { title: "Renamed".into(), renamed: true }));
        assert_eq!(thread_title(&acct, "other"), Some(CliTitle { title: "Not ours".into(), renamed: false }));
        assert_eq!(thread_title(&acct, "missing"), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// The conversation as another agent reads it: user and assistant messages
/// from byte offset `from` on. Returns the messages and the offset to
/// continue from: the end of the last complete line, so a line Codex is
/// still writing is read whole next time rather than skipped.
pub fn messages_from(path: &Path, from: u64) -> (Vec<crate::agents::Message>, u64) {
    let mut out = Vec::new();
    let read = crate::pty::complete_lines(path, from, |line| {
        if !line.contains("\"response_item\"") {
            return;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { return };
        if let Some(m) = message_of(&v) {
            out.push(m);
        }
    });
    (out, read)
}

#[cfg(test)]
mod format_tests {
    use super::*;

    /// The 0.157 rollout: `task_started` / `task_complete` for the turn, and
    /// the conversation as `response_item` messages with roles.
    #[test]
    fn reads_the_current_rollout_format() {
        let root = std::env::temp_dir().join(format!("luna-codex-fmt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("rollout-2026-09-26T15-43-59-abc.jsonl");
        let lines = [
            r#"{"timestamp":"t","type":"session_meta","payload":{"id":"abc","cwd":"E:\\p"}}"#,
            r#"{"timestamp":"t","type":"event_msg","payload":{"type":"task_started","turn_id":"t1"}}"#,
            r#"{"timestamp":"t","type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"<skills_instructions>…"}]}}"#,
            r##"{"timestamp":"t","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"# AGENTS.md instructions for E:\\p\n\nrules"}]}}"##,
            r#"{"timestamp":"t","type":"turn_context","payload":{"turn_id":"t1","cwd":"E:\\p","model":"gpt-6-astra"}}"#,
            r#"{"timestamp":"t","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Reply with the single word READY."}]}}"#,
            r#"{"timestamp":"t","type":"event_msg","payload":{"type":"item_completed","item":{"type":"UserMessage","text":"Reply with the single word READY."}}}"#,
            r#"{"timestamp":"t","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"READY"}]}}"#,
            r#"{"timestamp":"t","type":"event_msg","payload":{"type":"task_complete","turn_id":"t1","last_agent_message":"READY"}}"#,
        ];
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();

        let t = read_tail(&path);
        assert_eq!(t.model.as_deref(), Some("gpt-6-astra"));
        assert_eq!(read_first_prompt(&path).as_deref(), Some("Reply with the single word READY."));

        let (msgs, cursor) = messages_from(&path, 0);
        let got: Vec<(&str, &str)> = msgs.iter().map(|m| (m.role.as_str(), m.text.as_str())).collect();
        assert_eq!(got, vec![("user", "Reply with the single word READY."), ("assistant", "READY")]);
        assert_eq!(cursor, std::fs::metadata(&path).unwrap().len());
        let (more, _) = messages_from(&path, cursor);
        assert!(more.is_empty(), "nothing new after the cursor");
        // A reply still being written is not read, nor stepped over.
        let reply = r#"{"timestamp":"t","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"DONE"}]}}"#;
        let whole = lines.join("\n") + "\n";
        std::fs::write(&path, format!("{whole}{}", &reply[..40])).unwrap();
        assert_eq!(messages_from(&path, cursor), (vec![], cursor));
        std::fs::write(&path, format!("{whole}{reply}\n")).unwrap();
        let (more, _) = messages_from(&path, cursor);
        assert_eq!(more, vec![crate::agents::Message { role: "assistant".into(), text: "DONE".into() }]);
        std::fs::write(&path, &whole).unwrap();

        // Turn ends are counted off complete lines only, and incrementally.
        let mut s = Scan::default();
        let consumed = scan_rollout(&path, 0, 0, &mut s);
        assert_eq!((s.completed, s.working), (1, Some(false)));
        assert_eq!(consumed, std::fs::metadata(&path).unwrap().len());
        let mut grown = std::fs::read_to_string(&path).unwrap();
        grown.push_str(r#"{"timestamp":"t","type":"event_msg","payload":{"type":"task_complete","turn_id":"t2"}}"#);
        std::fs::write(&path, &grown).unwrap();
        assert_eq!(scan_rollout(&path, consumed, 0, &mut s), consumed, "a line without its newline is not there yet");
        assert_eq!(s.completed, 1);
        grown.push('\n');
        std::fs::write(&path, &grown).unwrap();
        scan_rollout(&path, consumed, 0, &mut s);
        assert_eq!(s.completed, 2);

        // A resumed thread: the turns of an earlier run are not this one's.
        let resumed = [
            r#"{"timestamp":"2026-09-26T15:44:00.000Z","type":"event_msg","payload":{"type":"task_complete","turn_id":"t1"}}"#,
            r#"{"timestamp":"2026-09-26T16:10:00.500Z","type":"event_msg","payload":{"type":"task_complete","turn_id":"t2"}}"#,
        ];
        std::fs::write(&path, resumed.join("\n") + "\n").unwrap();
        let spawned = chrono::DateTime::parse_from_rfc3339("2026-09-26T16:00:00Z").unwrap().timestamp_millis();
        let count = |since: i64| {
            let mut s = Scan::default();
            scan_rollout(&path, 0, since, &mut s);
            s.completed
        };
        assert_eq!(count(spawned), 1);
        assert_eq!(count(0), 2);

        // Mid-turn, however much the turn has written since it started: a
        // megabyte of tool output still reads as working.
        let filler = format!(
            r#"{{"timestamp":"t","type":"response_item","payload":{{"type":"function_call_output","call_id":"c","output":"{}"}}}}"#,
            "x".repeat(4096)
        );
        let long: Vec<String> = lines[..2].iter().map(|l| l.to_string()).chain(std::iter::repeat(filler).take(300)).collect();
        std::fs::write(&path, long.join("\n") + "\n").unwrap();
        let mut s = Scan::default();
        scan_rollout(&path, 0, 0, &mut s);
        assert_eq!(s.working, Some(true));

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Codex 0.163 asks through `request_user_input_async` (the turn goes on)
    /// and `request_user_input` (the turn waits for the answer).
    #[test]
    fn a_question_is_open_until_it_is_answered() {
        let root = std::env::temp_dir().join(format!("luna-codex-ask-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("rollout-2026-10-22T04-51-02-abc.jsonl");
        let write = |lines: &[&str]| std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        let ask = |lines: &[&str]| {
            write(lines);
            let mut s = Scan::default();
            scan_rollout(&path, 0, 0, &mut s);
            s.ask.map(|(_, a)| a)
        };
        let started = r#"{"timestamp":"t","type":"event_msg","payload":{"type":"task_started"}}"#;
        let async_q = r#"{"timestamp":"t","type":"response_item","payload":{"type":"function_call","name":"request_user_input_async","arguments":"{\"questions\":[{\"title\":\"Can you paste CLAUDE.md here?\"}]}","call_id":"call_a"}}"#;
        let accepted = r#"{"timestamp":"t","type":"response_item","payload":{"type":"function_call_output","call_id":"call_a","output":"{\"accepted\":true}"}}"#;
        let done = r#"{"timestamp":"t","type":"event_msg","payload":{"type":"task_complete"}}"#;
        let user = r#"{"timestamp":"t","type":"event_msg","payload":{"type":"user_message","message":"here it is"}}"#;

        let a = ask(&[started, async_q, accepted, done]).expect("an async question outlives its turn");
        assert!(!a.blocking);
        assert_eq!(a.questions[0].text, "Can you paste CLAUDE.md here?");
        assert!(ask(&[started, async_q, accepted, done, user]).is_none(), "the user's next word answers it");
        let item = r#"{"timestamp":"t","type":"event_msg","payload":{"type":"item_completed","item":{"type":"UserMessage","id":"u"}}}"#;
        assert!(ask(&[started, async_q, accepted, item]).is_none(), "as 0.163 writes it");
        let plain = r#"{"timestamp":"t","type":"response_item","payload":{"type":"function_call","name":"request_user_input_async","arguments":"{\"questions\":[{\"title\":\"Size?\",\"options\":[\"Small\",\"Large\"]}]}","call_id":"c"}}"#;
        assert_eq!(ask(&[started, plain]).unwrap().questions[0].options, ["Small", "Large"], "options as bare strings");

        let blocking = r#"{"timestamp":"t","type":"response_item","payload":{"type":"function_call","name":"request_user_input","arguments":"{\"questions\":[{\"id\":\"colour\",\"header\":\"Colour\",\"question\":\"Which colour?\",\"options\":[{\"label\":\"Red (Recommended)\",\"description\":\"\"},{\"label\":\"Blue\",\"description\":\"\"}]}]}","call_id":"call_b"}}"#;
        let answered = r#"{"timestamp":"t","type":"response_item","payload":{"type":"function_call_output","call_id":"call_b","output":"{\"answers\":{}}"}}"#;
        let a = ask(&[started, blocking]).expect("a blocking question");
        assert!(a.blocking);
        assert_eq!(a.questions[0].options, ["Red (Recommended)", "Blue"]);
        assert!(ask(&[started, blocking, answered]).is_none(), "its output is the answer");
        assert!(ask(&[started, blocking, done]).is_none(), "it goes with the turn");

        let _ = std::fs::remove_dir_all(&root);
    }
}
