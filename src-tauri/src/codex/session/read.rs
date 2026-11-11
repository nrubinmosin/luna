//! What is read of a rollout on demand: the opening prompt, the conversation for
//! another agent, the tail and last words, the model, when a call is due, and a
//! live session's status and meta.

use super::*;

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

pub(super) fn read_first_prompt(path: &Path) -> Option<String> {
    use std::io::BufReader;
    let f = std::fs::File::open(path).ok()?;
    first_prompt_in(crate::pty::read_lossy_lines(BufReader::new(f)))
}

/// `read_first_prompt` of the rollout's lines (`read_lossy_lines`).
pub(super) fn first_prompt_in(lines: impl Iterator<Item = impl AsRef<str>>) -> Option<String> {
    for line in lines.take(200) {
        let line = line.as_ref();
        if !line.contains("\"message\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
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

/// What Codex puts into the conversation in a user message that no person
/// wrote, each block of it wrapped in one of these tags, open and close (the
/// opening one may carry attributes). As Codex 0.167.0 tells such a block
/// from a prompt (`CONTEXTUAL_USER_FRAGMENT_MATCHERS`,
/// codex-rs/core/src/context/contextual_user_message.rs at rust-v0.167.0),
/// tag names in any case: the environment, a skill's text, a `!` shell
/// command and its output, a turn's interruption, a subagent's word, plugin
/// suggestions, a message-board notice, internal and goal context, a hook's
/// prompt; and `<user_instructions>`, which older versions wrapped an
/// `AGENTS.md` in. Anything else a prompt opens with, `<task>` say, is the
/// person's.
const INJECTED_TAGS: [&str; 11] = [
    "environment_context",
    "user_instructions",
    "skill",
    "user_shell_command",
    "turn_aborted",
    "subagent_notification",
    "recommended_plugins",
    "agent_message_board_notification",
    "codex_internal_context",
    "goal_context",
    "hook_prompt",
];

/// The same 0.167.0 matchers for what is not a tag: the `AGENTS.md` Codex reads
/// (`# AGENTS.md instructions for <dir>` ... `</INSTRUCTIONS>`, taken here by its
/// heading alone), and three warnings Codex once wrote in the user's place and
/// still recognizes in old rollouts.
const INJECTED_OPENINGS: [&str; 4] = [
    "# AGENTS.md instructions",
    "Warning: apply_patch was requested via ",
    "Warning: The maximum number of unified exec processes you can keep open is",
    "Warning: Your account was flagged for potentially high-risk cyber activity",
];

/// Whether a block of a user message is one Codex injected (`INJECTED_TAGS`,
/// `INJECTED_OPENINGS`, and `<external_KEY>` ... `</external_KEY>`, extra context
/// a client hands it), rather than words a person or an agent sent.
pub(super) fn injected(block: &str) -> bool {
    let t = block.trim();
    let starts = |prefix: &str| t.get(..prefix.len()).is_some_and(|head| head.eq_ignore_ascii_case(prefix));
    let ends = |suffix: &str| {
        let tail = t.len().checked_sub(suffix.len()).and_then(|at| t.get(at..));
        tail.is_some_and(|tail| tail.eq_ignore_ascii_case(suffix))
    };
    if INJECTED_OPENINGS.iter().any(|o| starts(o)) {
        return true;
    }
    let wrapped = |tag: &str| {
        let open = format!("<{tag}");
        starts(&open) && t[open.len()..].starts_with(['>', ' ']) && ends(&format!("</{tag}>"))
    };
    if INJECTED_TAGS.iter().any(|tag| wrapped(tag)) {
        return true;
    }
    let key = t.strip_prefix("<external_").and_then(|rest| rest.split_once('>')).map(|(key, _)| key);
    key.is_some_and(|key| !key.is_empty() && ends(&format!("</external_{key}>")))
}

/// A rollout line as one turn of the conversation, if it is one: a
/// `response_item` of type `message` with a user or assistant role. Developer
/// messages (skills, agent-mode preambles) and what Codex injects as a user
/// message (`injected`: `AGENTS.md`, `<environment_context>` and the like)
/// are not conversation and are left out, the way `luna_message_list` leaves out
/// Claude Code's meta turns.
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
        if !matches!(block["type"].as_str(), Some("input_text") | Some("output_text") | Some("text")) {
            continue;
        }
        // Codex tells its own blocks one by one, a message of the person's
        // possibly carrying one beside their words.
        if role == "user" && injected(t) {
            continue;
        }
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(t);
    }
    // A helper's prompt as its caller wrote it, without Luna's note on it.
    let text = if role == "user" { crate::pty::without_note(&text) } else { &text }.trim().to_string();
    if text.is_empty() {
        return None;
    }
    // 0.167.0 classes an assistant message as commentary, a partial answer or the
    // turn's terminal one (`MessagePhase`, protocol/src/models.rs).
    let final_answer = role == "assistant" && v["payload"]["phase"] == "final_answer";
    Some(crate::agents::Message { role: role.to_string(), text, after_cli: false, final_answer })
}

/// One rollout line as something said in the conversation, if it is one
/// (`message_of`): what a moved chat's document quotes
/// (agents/handoff/excerpts.rs).
pub(crate) fn said_line(line: &str) -> Option<crate::agents::Message> {
    message_of(&serde_json::from_str::<Value>(line).ok()?)
}
/// The limit a rollout's last turn was stopped by, in Luna's words
/// (`usage_limit`, `rate_limit`, `credits_depleted`), from the tail of its
/// text: an `error` event's `codex_error_info`, or the last `token_count`'s
/// `rate_limits.rate_limit_reached_type` (seen in 0.167.1's binary, docs/
/// cli-compatibility.md). Only what follows the last turn's start counts: a
/// limit an earlier turn met is over once a turn has run since.
pub(crate) fn limit_in(text: &str) -> Option<&'static str> {
    let mut reached: Option<Option<&'static str>> = None;
    for line in text.lines().rev() {
        if !line.contains("\"event_msg\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let p = &v["payload"];
        match p["type"].as_str() {
            Some("task_started" | "turn_started") => break,
            Some("error" | "stream_error") => {
                let info = &p["codex_error_info"];
                let kind = info.as_str().or_else(|| info.as_object().and_then(|o| o.keys().next()).map(String::as_str));
                if let Some(k) = kind.and_then(limit_word) {
                    return Some(k);
                }
            }
            Some("token_count") if reached.is_none() && p["rate_limits"].is_object() => {
                reached = Some(p["rate_limits"]["rate_limit_reached_type"].as_str().and_then(limit_word));
            }
            _ => {}
        }
    }
    reached.flatten()
}

/// A Codex limit's name as Luna words it, if it is one that stops a turn.
fn limit_word(kind: &str) -> Option<&'static str> {
    match kind {
        "usage_limit_exceeded" => Some("usage_limit"),
        "rate_limit_exceeded" | "rate_limit_reached" => Some("rate_limit"),
        k if k.ends_with("_credits_depleted") => Some("credits_depleted"),
        k if k.ends_with("usage_limit_reached") => Some("usage_limit"),
        _ => None,
    }
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
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Tail {
    pub model: Option<String>,
    pub context_tokens: Option<f64>,
    pub context_window: Option<f64>,
    /// The last `rate_limits` snapshot and the timestamp of its line.
    pub rate_limits: Option<(Value, String)>,
}

pub fn read_tail(path: &Path) -> Tail {
    tail(path, 128 * 1024).map(|text| tail_in(&text)).unwrap_or_default()
}

/// See `read_tail`, of the tail's text.
pub(super) fn tail_in(text: &str) -> Tail {
    let mut out = Tail::default();
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
            Some("event_msg") => {
                if let Some("token_count") = payload["type"].as_str() {
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
                        out.rate_limits = Some((payload["rate_limits"].clone(), v["timestamp"].as_str().unwrap_or("").to_string()));
                    }
                }
            }
            _ => {}
        }
        if out.model.is_some() && out.context_tokens.is_some() && out.rate_limits.is_some() {
            break;
        }
    }
    out
}

/// The model a rollout's last turn ran on: what a `model_reroute` moved it to,
/// else what its `turn_context` names. A turn with much output pushes those
/// lines out of the tail; the whole file is read only then.
pub fn last_model(path: &Path) -> Option<String> {
    tail(path, 128 * 1024)
        .and_then(|t| last_model_in(&t))
        .or_else(|| std::fs::read(path).ok().and_then(|b| last_model_in(&String::from_utf8_lossy(&b))))
}

/// See `last_model`, of a text.
pub(super) fn last_model_in(text: &str) -> Option<String> {
    text.lines().rev().filter(|l| l.contains("\"turn_context\"") || l.contains("\"model_reroute\"")).find_map(|line| {
        let v = serde_json::from_str::<Value>(line).ok()?;
        let payload = &v["payload"];
        payload["to_model"].as_str().or(payload["model"].as_str()).map(str::to_owned)
    })
}

/// The model of the last turn that was answered: the latest `turn_context` (or
/// reroute) after which the thread's running total of tokens grew. The rollout
/// names a turn's model before asking it, so the latest alone may be one that
/// never answered — not there, or not allowed — and a `token_count` is written
/// again unchanged (rate limits renewed), which is no answer either. The tail
/// does not say what the total stood at before its first count, so that count
/// says nothing there; failing the tail, the whole file is read from nothing.
pub fn answered_model(path: &Path) -> Option<String> {
    tail(path, 128 * 1024)
        .and_then(|t| answered_model_in(&t, None))
        .or_else(|| std::fs::read(path).ok().and_then(|b| answered_model_in(&String::from_utf8_lossy(&b), Some(0))))
}

/// See `answered_model`, of a text, the total standing at `from` before it
/// (None: not known).
pub(super) fn answered_model_in(text: &str, from: Option<u64>) -> Option<String> {
    let (mut asked, mut answered, mut total) = (None::<String>, None::<String>, from);
    for line in text.lines() {
        let model = line.contains("\"turn_context\"") || line.contains("\"model_reroute\"");
        if !model && !line.contains("\"token_count\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let payload = &v["payload"];
        if model {
            asked = payload["to_model"].as_str().or(payload["model"].as_str()).map(str::to_owned).or(asked);
            continue;
        }
        let usage = &payload["info"]["total_token_usage"];
        if !usage.is_object() {
            continue;
        }
        // Saturating: two counts the rollout makes up may overflow the sum.
        let now = usage["input_tokens"].as_u64().unwrap_or(0).saturating_add(usage["output_tokens"].as_u64().unwrap_or(0));
        if total.is_some_and(|before| now > before) && asked.is_some() {
            answered = asked.clone();
        }
        total = Some(now);
    }
    answered
}

/// When the calls the turn waits on are due, the latest of them, in ms since
/// the epoch: each by the bound its own arguments set, from the line that made
/// it until its output. In 0.166 `shell`, `shell_command` and a
/// `local_shell_call` take `timeout_ms`; `exec_command` and `write_stdin` come
/// back by `yield_time_ms` (or `timeout_ms`, run to completion); code mode's
/// `exec` cell by the `yield_time_ms` of its `// @exec:` pragma, and `wait` on
/// one by its own; `sleep` by `duration_ms`, `wait_agent` by `timeout_ms`. Their
/// defaults, seconds long, are left out: next to half an hour they change nothing.
pub fn due_ms(path: &Path) -> Option<u64> {
    due_in(&tail(path, crate::claude::session::DUE_TAIL)?)
}

pub(super) fn due_in(text: &str) -> Option<u64> {
    // A bound past any clock (`as` saturates) is a deadline never reached, so
    // the sums with it saturate too rather than wrap round to one long past.
    let ms = |v: &Value| v.as_f64().filter(|t| *t > 0.0).map(|t| t as u64);
    // A call is back by the first of its bounds.
    let bound = |args: &Value| ["timeout_ms", "yield_time_ms", "duration_ms"].iter().filter_map(|k| ms(&args[*k])).min();
    let mut open: Vec<(String, u64)> = Vec::new();
    for line in text.lines() {
        let turn = line.contains("\"task_started\"") || line.contains("\"turn_started\"");
        if !turn && !line.contains("_call") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let payload = &v["payload"];
        match (v["type"].as_str(), payload["type"].as_str()) {
            // What an earlier turn left open is not what this one waits on.
            (Some("event_msg"), Some("task_started" | "turn_started")) => open.clear(),
            (Some("response_item"), Some(kind @ ("function_call" | "custom_tool_call" | "local_shell_call"))) => {
                let by = match kind {
                    "custom_tool_call" => payload["input"]
                        .as_str()
                        .and_then(|code| code.lines().next()?.trim().strip_prefix("// @exec:").map(str::to_owned))
                        .and_then(|pragma| serde_json::from_str::<Value>(&pragma).ok())
                        .and_then(|pragma| bound(&pragma)),
                    "local_shell_call" => bound(&payload["action"]),
                    _ => payload["arguments"].as_str().and_then(|a| serde_json::from_str::<Value>(a).ok()).and_then(|a| bound(&a)),
                };
                let at = v["timestamp"]
                    .as_str()
                    .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                    .map(|t| t.timestamp_millis().max(0) as u64);
                if let (Some(by), Some(at), Some(id)) = (by, at, payload["call_id"].as_str()) {
                    open.push((id.to_string(), at.saturating_add(by)));
                }
            }
            (Some("response_item"), Some("function_call_output" | "custom_tool_call_output")) => {
                open.retain(|(id, _)| payload["call_id"].as_str() != Some(id.as_str()));
            }
            _ => {}
        }
    }
    open.iter().map(|(_, due)| *due).max()
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

    let scan = tracked(live);
    let status = Some(fold_status(scan.as_ref().and_then(|s| s.working), live.last_output_ms));
    let named = thread_title(live.account_path, &id);

    let mut m = SessionMeta {
        status: status.map(str::to_owned),
        cwd: first_line(&path).and_then(|v| v["payload"]["cwd"].as_str().map(str::to_owned)).or_else(|| Some(live.cwd.to_string())),
        title_renamed: named.as_ref().is_some_and(|n| n.renamed),
        title: named.map(|n| n.title),
        session_id: Some(id),
        model: t.model,
        first_prompt: cached_first_prompt(&path).and_then(|s| sane_title(&s)),
        last_said_at: scan.as_ref().and_then(|s| s.last_said_at.clone()),
        last_said: scan.and_then(|s| s.last_said),
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
/// last couple of days, with the line's timestamp. Newest by that timestamp:
/// a name carries when the thread started, and the file written to last may
/// be an older conversation resumed that has not said its limits since.
/// The files are read written-to-last first, and only until one was last
/// written before the newest snapshot found, which holds none newer.
pub fn latest_rate_limits(account_path: &str) -> Option<(Value, String)> {
    let today = Local::now().date_naive();
    let ms = |t: std::time::SystemTime| t.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
    let mut files: Vec<(i64, PathBuf)> = rollouts_on(account_path, days_back(today, 2))
        .into_iter()
        .map(|(_, _, p)| (std::fs::metadata(&p).and_then(|m| m.modified()).map_or(0, ms), p))
        .collect();
    files.sort_by_key(|e| std::cmp::Reverse(e.0));
    let mut best: Option<(i64, (Value, String))> = None;
    for (modified, path) in files {
        if best.as_ref().is_some_and(|(at, _)| *at >= modified) {
            break;
        }
        let Some(hit) = read_tail(&path).rate_limits else { continue };
        let at = chrono::DateTime::parse_from_rfc3339(&hit.1).map_or(i64::MIN, |t| t.timestamp_millis());
        if best.as_ref().is_none_or(|(b, _)| at > *b) {
            best = Some((at, hit));
        }
    }
    best.map(|(_, hit)| hit)
}

/// The conversation as another agent reads it: user and assistant messages
/// from byte offset `from` on. Returns the messages and the offset to
/// continue from: the end of the last complete line, so a line Codex is
/// still writing is read whole next time rather than skipped. A cursor past
/// the end is one into a file since cut short or written anew, where it would
/// land mid-line: the file is read from the top.
pub fn messages_from(path: &Path, from: u64) -> (Vec<crate::agents::Message>, u64) {
    let from = if past_end(path, from) { 0 } else { from };
    let mut out = Vec::new();
    let read = crate::pty::complete_lines(path, from, |line| out.extend(message_in(line)));
    (out, read)
}

/// `messages_from` of a rollout's bytes in memory, from the top: the messages of
/// its complete lines, and the offset just past the last of them.
pub fn messages_in(bytes: &[u8]) -> (Vec<crate::agents::Message>, usize) {
    let mut out = Vec::new();
    let read = crate::pty::complete_lines_in(bytes, |line| out.extend(message_in(line)));
    (out, read)
}

/// One complete line of a rollout as `messages_from` reads it.
pub(super) fn message_in(line: &str) -> Option<crate::agents::Message> {
    if !line.contains("\"response_item\"") {
        return None;
    }
    message_of(&serde_json::from_str::<Value>(line).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rollout_names_its_model_and_context() {
        let path = std::env::temp_dir().join(format!("luna-rollout-model-{}.jsonl", std::process::id()));
        // As `codex exec` 0.166.0 wrote them, cut to the fields read here. The
        // last `token_count` is the one that counts; a later one with no
        // `info` (rate limits alone) changes nothing.
        let head = concat!(
            r#"{"timestamp":"t","type":"turn_context","payload":{"model":"gpt-6.1-sol","effort":"low"}}"#,
            "\n",
            r#"{"timestamp":"t","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":15515,"cached_input_tokens":12416,"output_tokens":5},"last_token_usage":{"total_tokens":15520}}}}"#,
            "\n",
            r#"{"timestamp":"t","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":32377,"cached_input_tokens":27776,"output_tokens":10},"last_token_usage":{"total_tokens":16867}}}}"#,
            "\n",
            r#"{"timestamp":"t","type":"event_msg","payload":{"type":"token_count","info":null,"rate_limits":{"primary":null}}}"#,
            "\n",
        );
        std::fs::write(&path, head).unwrap();
        assert_eq!(last_model(&path).as_deref(), Some("gpt-6.1-sol"));
        assert_eq!(answered_model(&path).as_deref(), Some("gpt-6.1-sol"));
        // A turn on a model that never answered: named, and no more.
        let asked = r#"{"timestamp":"t","type":"turn_context","payload":{"model":"gpt-0-missing","effort":"low"}}"#;
        std::fs::write(&path, format!("{head}{asked}\n")).unwrap();
        assert_eq!(last_model(&path).as_deref(), Some("gpt-0-missing"));
        assert_eq!(answered_model(&path).as_deref(), Some("gpt-6.1-sol"), "the last one that answered");
        // A count written again unchanged as the next turn starts is no answer.
        let again = r#"{"timestamp":"t","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":32377,"cached_input_tokens":27776,"output_tokens":10},"last_token_usage":{"total_tokens":16867}}}}"#;
        std::fs::write(&path, format!("{head}{asked}\n{again}\n")).unwrap();
        assert_eq!(answered_model(&path).as_deref(), Some("gpt-6.1-sol"), "unchanged: no answer of the new one");
        std::fs::write(&path, format!("{asked}\n")).unwrap();
        assert_eq!(answered_model(&path), None, "none has yet");
        std::fs::write(&path, head).unwrap();
        let tail = read_tail(&path);
        assert_eq!((tail.model.as_deref(), tail.context_tokens), (Some("gpt-6.1-sol"), Some(16867.0)));

        // A turn whose output pushed the line out of the tail.
        let filler = format!("{{\"type\":\"response_item\",\"payload\":\"{}\"}}\n", "x".repeat(200 * 1024));
        std::fs::write(&path, format!("{head}{filler}")).unwrap();
        assert_eq!(last_model(&path).as_deref(), Some("gpt-6.1-sol"), "read from the whole file");

        // A reroute stands for its turn; the next turn's context comes after it.
        let rerouted = r#"{"timestamp":"t","type":"event_msg","payload":{"type":"model_reroute","from_model":"gpt-6.1-sol","to_model":"gpt-other","reason":"x"}}"#;
        std::fs::write(&path, format!("{head}{rerouted}\n")).unwrap();
        assert_eq!(last_model(&path).as_deref(), Some("gpt-other"));
        std::fs::write(
            &path,
            format!("{head}{rerouted}\n{}\n", r#"{"timestamp":"t","type":"turn_context","payload":{"model":"gpt-6.1-sol"}}"#),
        )
        .unwrap();
        assert_eq!(last_model(&path).as_deref(), Some("gpt-6.1-sol"));
        let _ = std::fs::remove_file(&path);
    }

    /// A line that is not UTF-8 (a Latin-1 byte in a prompt) neither ends the
    /// search for the opening prompt nor is passed over: it reads as the
    /// conversation reads it.
    #[test]
    fn a_line_not_utf8_reads_lossily_in_the_search_for_the_opening_prompt() {
        let path = std::env::temp_dir().join(format!("luna-codex-latin1-{}.jsonl", std::process::id()));
        let developer = b"{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"developer\",\"content\":[{\"type\":\"input_text\",\"text\":\"caf\xe9\"}]}}\n";
        let prompt = b"{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"fix the caf\xe9 page\"}]}}\n";
        let bytes = [&developer[..], &prompt[..]].concat();
        std::fs::write(&path, &bytes).unwrap();
        let want = Some("fix the caf\u{fffd} page".to_string());
        assert_eq!(read_first_prompt(&path), want);
        assert_eq!(read_rollout(&bytes, 0).first_prompt, want);
        assert_eq!(messages_from(&path, 0).0[0].text, "fix the caf\u{fffd} page", "as the conversation has it");
        let _ = std::fs::remove_file(&path);
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
    fn reads_the_latest_thread_name() {
        let root = std::env::temp_dir().join(format!("luna-codex-names-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("session_index.jsonl"),
            concat!(
                r#"{"id":"abc","thread_name":"First name","updated_at":"2026-09-17T10:00:00Z"}"#,
                "\n",
                r#"{"id":"other","thread_name":"Not ours","updated_at":"2026-09-17T10:01:00Z"}"#,
                "\n",
                r#"{"id":"abc","thread_name":"Renamed","updated_at":"2026-09-17T10:02:00Z"}"#,
                "\n",
            ),
        )
        .unwrap();
        let acct = root.to_string_lossy();
        assert_eq!(thread_title(&acct, "abc"), Some(CliTitle { title: "Renamed".into(), renamed: true }));
        assert_eq!(thread_title(&acct, "other"), Some(CliTitle { title: "Not ours".into(), renamed: false }));
        assert_eq!(thread_title(&acct, "missing"), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A user message as a rollout holds it, its blocks as given.
    fn user_line(blocks: &[&str]) -> String {
        let content: Vec<Value> = blocks.iter().map(|t| serde_json::json!({ "type": "input_text", "text": t })).collect();
        serde_json::json!({ "type": "response_item", "payload": { "type": "message", "role": "user", "content": content } }).to_string()
    }

    #[test]
    fn a_final_answer_is_marked_as_codex_marks_it() {
        let said = |phase: Option<&str>| {
            let mut payload =
                serde_json::json!({ "type": "message", "role": "assistant", "content": [{ "type": "output_text", "text": "Done." }] });
            if let Some(p) = phase {
                payload["phase"] = p.into();
            }
            message_in(&serde_json::json!({ "type": "response_item", "payload": payload }).to_string()).unwrap()
        };
        assert!(said(Some("final_answer")).final_answer);
        assert!(!said(Some("commentary")).final_answer);
        assert!(!said(None).final_answer, "an older Codex's: no word either way");
    }

    #[test]
    fn a_prompt_in_tags_of_its_own_is_a_prompt() {
        let text = |line: String| message_in(&line).map(|m| m.text);
        // Prompts a person or an agent wrote, whatever they open with.
        assert_eq!(text(user_line(&["<task>Fix the build</task>"])).as_deref(), Some("<task>Fix the build</task>"));
        assert_eq!(text(user_line(&["<b>bold</b> claim"])).as_deref(), Some("<b>bold</b> claim"));
        assert_eq!(text(user_line(&["<skill>s"])).as_deref(), Some("<skill>s"), "not closed: not Codex's");
        let other = "<skills_instructions>x</skills_instructions>";
        assert_eq!(text(user_line(&[other])).as_deref(), Some(other), "a tag of Codex's own, but not one it puts in a user's place");
        // What Codex 0.167.0 puts there itself, as it spells it.
        for block in [
            "<environment_context>\n  <cwd>E:\\p</cwd>\n</environment_context>",
            "<ENVIRONMENT_CONTEXT><cwd>E:\\p</cwd></Environment_Context>",
            "# AGENTS.md instructions for E:\\p\n\n<INSTRUCTIONS>\nrules\n</INSTRUCTIONS>",
            "<user_instructions>\nrules\n</user_instructions>",
            "<skill>\n<name>review</name>\n<path>p</path>\nbody\n</skill>",
            "<user_shell_command>\n<command>ls</command>\n</user_shell_command>",
            "<turn_aborted>\nThe user interrupted the previous turn on purpose.\n</turn_aborted>",
            "<subagent_notification>{}</subagent_notification>",
            "<recommended_plugins>\n- x\n</recommended_plugins>",
            "<agent_message_board_notification>x</agent_message_board_notification>",
            "<codex_internal_context source=\"goal\">x</codex_internal_context>",
            "<goal_context>x</goal_context>",
            "<hook_prompt hook_run_id=\"r1\">go on</hook_prompt>",
            "<external_ticket>ABC-1</external_ticket>",
            "Warning: The maximum number of unified exec processes you can keep open is 8.",
        ] {
            assert_eq!(text(user_line(&[block])), None, "{block}");
        }
        // Beside a person's words, only Codex's block goes.
        let mixed = user_line(&["<environment_context><cwd>E:\\p</cwd></environment_context>", "<task>Review it</task>"]);
        assert_eq!(text(mixed).as_deref(), Some("<task>Review it</task>"));
    }

    /// What a turn ended saying, until the next one starts.
    #[test]
    fn reads_the_last_words() {
        let dir = std::env::temp_dir().join(format!("luna-rollout-said-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("r.jsonl");
        let event =
            |at: &str, payload: Value| format!("{}\n", serde_json::json!({ "timestamp": at, "type": "event_msg", "payload": payload }));
        let lines = [
            event("2026-11-03T10:00:00.000Z", serde_json::json!({ "type": "task_started" })),
            event("2026-11-03T10:00:00.100Z", serde_json::json!({ "type": "user_message", "message": "which way?" })),
            event(
                "2026-11-03T10:00:09.000Z",
                serde_json::json!({ "type": "task_complete", "last_agent_message": "A or B?\nSTATUS: AWAITING OPERATOR" }),
            ),
        ];
        std::fs::write(&path, lines.concat()).unwrap();
        let mut scan = Scan::default();
        scan_rollout(&path, 0, 0, &mut scan);
        assert_eq!(scan.last_said.as_deref(), Some("A or B?\nSTATUS: AWAITING OPERATOR"));
        // The next turn is on: nothing said to it yet.
        let more = event("2026-11-03T10:06:00.000Z", serde_json::json!({ "type": "task_started" }));
        std::fs::write(&path, format!("{}{more}", lines.concat())).unwrap();
        scan_rollout(&path, 0, 0, &mut scan);
        assert_eq!(scan.last_said, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// As 0.166 writes its calls: code mode's `exec` cell with the pragma on its
    /// first line, `wait` on it, and the function tools of a session without it.
    #[test]
    fn a_call_is_due_by_its_own_bound() {
        let at = "2026-11-04T13:15:19.533Z";
        let start = chrono::DateTime::parse_from_rfc3339(at).unwrap().timestamp_millis() as u64;
        let item = |payload: Value| serde_json::json!({ "timestamp": at, "type": "response_item", "payload": payload }).to_string();
        let call = |id: &str, name: &str, args: Value| {
            item(serde_json::json!({ "type": "function_call", "name": name, "arguments": args.to_string(), "call_id": id }))
        };
        let cell = |id: &str, code: &str| {
            item(serde_json::json!({ "type": "custom_tool_call", "status": "completed", "name": "exec", "input": code, "call_id": id }))
        };
        let output = |kind: &str, id: &str| item(serde_json::json!({ "type": kind, "call_id": id, "output": "Script completed" }));
        let due = |lines: &[&str]| due_in(&lines.join("\n")).map(|d| d - start);
        let min = 60_000;

        let lane = cell(
            "c1",
            "// @exec: {\"yield_time_ms\": 2400000, \"max_output_tokens\": 1000}\ntext(await tools.exec_command({cmd:\"./lane.sh\"}));\n",
        );
        assert_eq!(due(&[&lane]), Some(40 * min));
        assert_eq!(due(&[&lane, &output("custom_tool_call_output", "c1")]), None, "its output ends it");
        assert_eq!(due(&[&cell("c1", "text(await tools.exec_command({cmd:\"ls\"}));\n")]), None, "no pragma: the default");
        let waits = call("c2", "wait", serde_json::json!({ "cell_id": "18", "yield_time_ms": 3000000 }));
        assert_eq!(due(&[&lane, &waits]), Some(50 * min), "the latest");
        assert_eq!(due(&[&waits, &output("function_call_output", "c2")]), None);
        assert_eq!(
            due(&[&call("c3", "shell_command", serde_json::json!({ "command": "./lane.sh", "timeout_ms": 2400000 }))]),
            Some(40 * min)
        );
        assert_eq!(due(&[&call("c3", "sleep", serde_json::json!({ "duration_ms": 1800000 }))]), Some(30 * min));
        let both = serde_json::json!({ "cmd": "./lane.sh", "yield_time_ms": 600000, "timeout_ms": 2400000 });
        assert_eq!(due(&[&call("c3", "exec_command", both)]), Some(10 * min), "back by the first of its bounds");
        let shell = item(
            serde_json::json!({ "type": "local_shell_call", "call_id": "c4", "status": "in_progress", "action": { "type": "exec", "command": ["./lane.sh"], "timeout_ms": 2400000 } }),
        );
        assert_eq!(due(&[&shell]), Some(40 * min));
        let next = r#"{"timestamp":"t","type":"event_msg","payload":{"type":"task_started"}}"#;
        assert_eq!(due(&[&lane, next]), None, "an earlier turn's call");
    }

    /// Found by the codex_rollout fuzz target: a bound past any clock, added
    /// to its call's start, overflowed (a panic with overflow checks, a
    /// deadline long past without them), and so did two token counts past
    /// u64 in a total.
    #[test]
    fn numbers_past_any_clock_or_count_saturate() {
        let call = r#"{"timestamp":"2026-11-04T13:15:19.533Z","type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"./lane.sh\",\"yield_time_ms\":9223372036854775807599999}","call_id":"c3"}}"#;
        assert_eq!(due_in(call), Some(u64::MAX), "due never, not before the call");
        let asked = r#"{"timestamp":"t","type":"turn_context","payload":{"model":"gpt-6.1-sol"}}"#;
        let counted = r#"{"timestamp":"t","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":18446744073709551615,"output_tokens":1}}}}"#;
        assert_eq!(answered_model_in(&[asked, counted].join("\n"), Some(0)).as_deref(), Some("gpt-6.1-sol"));
        assert!(read_rollout([call, asked, counted].join("\n").as_bytes(), 0).due_ms.is_some());
    }
}

#[cfg(test)]
mod limit_tests {
    use super::*;

    #[test]
    fn a_limit_is_read_from_the_last_turn_only() {
        let ev = |p: Value| serde_json::json!({ "timestamp": "t", "type": "event_msg", "payload": p }).to_string();
        let started = ev(serde_json::json!({ "type": "task_started" }));
        let count = |reached: Value| {
            ev(
                serde_json::json!({ "type": "token_count", "rate_limits": { "primary": { "used_percent": 100.0 }, "rate_limit_reached_type": reached } }),
            )
        };
        let error =
            |info: Value| ev(serde_json::json!({ "type": "error", "message": "You've hit your usage limit.", "codex_error_info": info }));
        let rollout = |lines: &[String]| lines.join("\n");
        assert_eq!(limit_in(&rollout(&[started.clone(), error(serde_json::json!("usage_limit_exceeded"))])), Some("usage_limit"));
        assert_eq!(limit_in(&rollout(&[started.clone(), error(serde_json::json!({ "usage_limit_exceeded": {} }))])), Some("usage_limit"));
        assert_eq!(
            limit_in(&rollout(&[started.clone(), count(serde_json::json!("workspace_member_usage_limit_reached"))])),
            Some("usage_limit")
        );
        assert_eq!(limit_in(&rollout(&[started.clone(), count(serde_json::json!("member_credits_depleted"))])), Some("credits_depleted"));
        assert_eq!(limit_in(&rollout(&[started.clone(), count(Value::Null)])), None, "as a live rollout has it");
        // Met before, and a turn has run since: over.
        assert_eq!(limit_in(&rollout(&[error(serde_json::json!("usage_limit_exceeded")), started.clone(), count(Value::Null)])), None);
        // Another error is no limit.
        assert_eq!(limit_in(&rollout(&[started, error(serde_json::json!("context_window_exceeded"))])), None);
    }
}
