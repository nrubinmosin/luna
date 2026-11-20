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

/// `limit_at_in` without its time.
#[cfg(test)]
pub(crate) fn limit_in(text: &str) -> Option<&'static str> {
    limit_at_in(text).map(|(kind, _)| kind)
}

/// The limit a rollout's last turn was stopped by, in Luna's words
/// (`usage_limit`, `rate_limit`, `credits_depleted`), from the tail of its
/// text: an `error` event's `codex_error_info`, or the last `token_count`'s
/// `rate_limits.rate_limit_reached_type` (seen in 0.167.1's binary, docs/
/// cli-compatibility.md). Only what follows the last turn's start counts: a
/// limit an earlier turn met is over once a turn has run since. With it, the
/// timestamp of the line that says it: when the turn was stopped, as far as
/// the rollout tells.
pub(crate) fn limit_at_in(text: &str) -> Option<(&'static str, Option<String>)> {
    let mut reached: Option<Option<(&'static str, Option<String>)>> = None;
    for line in text.lines().rev() {
        if !line.contains("\"event_msg\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let p = &v["payload"];
        let at = || v["timestamp"].as_str().map(str::to_owned);
        match p["type"].as_str() {
            Some("task_started" | "turn_started") => break,
            // 0.169.0 also keeps the error a turn ended on in its end's
            // `error` (`TurnError`), with no `error` event left beside it.
            Some(kind @ ("error" | "stream_error" | "task_complete" | "turn_complete" | "turn_aborted")) => {
                let info = if kind.contains("error") { &p["codex_error_info"] } else { &p["error"]["codex_error_info"] };
                let kind = info.as_str().or_else(|| info.as_object().and_then(|o| o.keys().next()).map(String::as_str));
                if let Some(k) = kind.and_then(limit_word) {
                    return Some((k, at()));
                }
            }
            Some("token_count") if reached.is_none() && p["rate_limits"].is_object() => {
                reached = Some(p["rate_limits"]["rate_limit_reached_type"].as_str().and_then(limit_word).map(|k| (k, at())));
            }
            _ => {}
        }
    }
    reached.flatten()
}

/// `limit_at_in` of the last 256 KB of a rollout, read again only once the
/// file has changed: every live chat's limit is asked for every few seconds.
pub(crate) fn rollout_limit(path: &Path) -> Option<(&'static str, Option<String>)> {
    type Limit = Option<(&'static str, Option<String>)>;
    static SEEN: std::sync::LazyLock<crate::stamped::ByStamp<Limit>> = std::sync::LazyLock::new(|| crate::stamped::ByStamp::new(256));
    SEEN.get(path, |path| tail(path, 256 * 1024).map(|text| limit_at_in(&text))).flatten()
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

/// The limit an error message of Codex's says a turn stopped on, in Luna's
/// words (`limit_word`): a `codex exec --json` stream's `turn.failed` and
/// `error` events carry the message alone. Codex 0.169.0's
/// (protocol/src/error.rs), each beginning so: the errors it files as
/// `UsageLimitExceeded` — "You’ve hit your usage limit" (a curly apostrophe
/// or, as earlier versions wrote it, a straight one), "You hit your spend cap"
/// (a workspace's), "Quota exceeded.", "To use Codex with your ChatGPT plan,
/// upgrade to Plus" — "Your workspace is out of credits" (`*_credits_depleted`),
/// and `RateLimitExceeded`'s "rate limit exceeded:".
pub(crate) fn limit_of_message(message: &str) -> Option<&'static str> {
    const WORDS: [(&str, &str); 6] = [
        ("You've hit your usage limit", "usage_limit"),
        ("You hit your spend cap", "usage_limit"),
        ("Quota exceeded.", "usage_limit"),
        ("To use Codex with your ChatGPT plan, upgrade to Plus", "usage_limit"),
        ("Your workspace is out of credits", "credits_depleted"),
        ("rate limit exceeded:", "rate_limit"),
    ];
    let message = message.trim().replace('\u{2019}', "'");
    WORDS.iter().find(|(start, _)| message.starts_with(start)).map(|(_, word)| *word)
}
/// The thread's name, if it has one. Codex appends `{id, thread_name,
/// updated_at}` to `<CODEX_HOME>/session_index.jsonl` whenever a thread is
/// named, so the last line for an id is the current name. It names a thread
/// once on its own, from the opening prompt; a name that differs from that
/// first one is the user's `/rename`.
pub fn thread_title(account_path: &str, thread_id: &str) -> Option<CliTitle> {
    let names = thread_names(&Path::new(account_path).join("session_index.jsonl"));
    let (first, title) = names.get(thread_id)?;
    Some(CliTitle { renamed: title != first, title: title.clone() })
}

/// Each thread's first and last name in a `session_index.jsonl`, by id. The
/// file is read whole, and so read again only once it has changed: every
/// live chat's title is asked for every few seconds, and the index grows by
/// a line a thread.
fn thread_names(path: &Path) -> std::sync::Arc<HashMap<String, (String, String)>> {
    static SEEN: std::sync::LazyLock<crate::stamped::ByStamp<std::sync::Arc<HashMap<String, (String, String)>>>> =
        std::sync::LazyLock::new(|| crate::stamped::ByStamp::new(32));
    SEEN.get(path, |path| {
        let mut names: HashMap<String, (String, String)> = HashMap::new();
        // A read that failed keeps nothing (`ByStamp::get`).
        let text = std::fs::read_to_string(path).ok()?;
        for v in text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()) {
            let (Some(id), Some(name)) = (v["id"].as_str(), v["thread_name"].as_str().and_then(sane_title)) else { continue };
            match names.get_mut(id) {
                Some((_, last)) => *last = name,
                None => {
                    names.insert(id.to_string(), (name.clone(), name));
                }
            }
        }
        Some(std::sync::Arc::new(names))
    })
    .unwrap_or_default()
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

/// Read again only once the file has changed: every live chat's tail is
/// asked for every few seconds, and at idle none of them moves.
pub fn read_tail(path: &Path) -> Tail {
    static SEEN: std::sync::LazyLock<crate::stamped::ByStamp<Tail>> = std::sync::LazyLock::new(|| crate::stamped::ByStamp::new(256));
    SEEN.get(path, |path| tail(path, 128 * 1024).map(|text| tail_in(&text))).unwrap_or_default()
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
                let at = v["timestamp"].as_str().and_then(|t| t.parse::<jiff::Timestamp>().ok()).map(|t| t.as_millisecond().max(0) as u64);
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

/// The commands the conversation left running past their call, as the model
/// gave them, for an idle notice beside the processes it sees (notifier
/// lines.rs `idle_line`): `Shells` over the rollout's tail.
pub fn background_calls(path: &Path) -> Vec<String> {
    tail(path, crate::claude::session::DUE_TAIL).map(|t| background_calls_in(&t)).unwrap_or_default()
}

pub(super) fn background_calls_in(text: &str) -> Vec<String> {
    let mut shells = Shells::default();
    for line in text.lines() {
        shells.line(line);
    }
    shells.running().into_iter().map(|(_, name)| name).collect()
}

/// The shells a rollout says its turns left running, by Codex's session id
/// (its unified exec's process id), each as Codex 0.169.0 reports it, and
/// only off the output of a call that runs a command, matched to it by its
/// call id (an `exec_command` call, or a code-mode cell whose script calls
/// `exec_command`): nothing else shaped like a result starts one.
///
/// - the call's result says the process still runs: `exec_command`'s output
///   header `Process running with session ID N` (0.166+), or, in code mode
///   (every tool call through the `exec` tool's script), the result object the
///   script printed, `{"session_id":N,"wall_time_seconds":…}` with no
///   `exit_code`, among the cell's `custom_tool_call_output` texts;
/// - its end: an `item_completed` event of a `CommandExecution` item whose
///   `process_id` is N, written when the process exits, after the turn that
///   started it if it outlived it; or a `write_stdin` to N whose header says
///   `Process exited with code`.
///
/// A result printed by a call made before that end (a cell that went on past
/// its shell's exit) starts nothing; one made after it is a new process, the
/// id reused. Where a rollout never records an end (`ends_recorded` false:
/// 0.169.0's legacy persistence leaves the items out) the record bounds a
/// shell by its job instead (`activity`, `SHELL_UNSEEN_MS`).
///
/// Named by the command where the call gives one (`exec_command`'s `cmd`, a
/// cell with a single `exec_command` call whose `cmd` is a literal), else by
/// the session id. A result the script never printed is not seen here (the
/// session's job still holds the process).
///
/// It also keeps the last tool outputs, each with what its call ran
/// (`answered`): a permission a hook said is up for a command is answered
/// once that command's call has its output.
#[derive(Clone, Debug, Default)]
pub(crate) struct Shells {
    /// Running, in the order seen: session id, the command if known, and when
    /// the call that started it was made (its line's time, epoch ms; 0 for
    /// none that reads).
    running: Vec<(String, Option<String>, i64)>,
    /// Calls awaiting their output that may start a shell or write to one, by
    /// call id.
    asked: HashMap<String, Call>,
    /// Sessions whose end was written, with when (`seq`), the last
    /// `ENDED_KEPT`.
    ended: std::collections::VecDeque<(String, u64)>,
    /// Whether the rollout has recorded any process's end.
    ends_recorded: bool,
    /// The last `ANSWERED_KEPT` outputs of calls that ran a command: what the
    /// call ran (`Call::text`), and when the output was written (its line's
    /// time, epoch ms).
    answered: std::collections::VecDeque<(String, i64)>,
    /// Lines taken so far: the order of calls and ends.
    seq: u64,
}

/// A call that may start a shell or write to one.
#[derive(Clone, Debug, Default)]
struct Call {
    /// It runs a command (`exec_command`, or a cell that calls it): its output
    /// may say a shell runs.
    runs: bool,
    /// The command it runs, where it names one.
    cmd: Option<String>,
    /// The session it writes to (`write_stdin`).
    writes_to: Option<String>,
    /// What it runs as text, to match a permission's command against: its
    /// command, or a cell's script (its first `TEXT_KEPT` characters).
    text: String,
    /// When it was made: its order (`Shells::seq`) and its line's time.
    seq: u64,
    at_ms: i64,
}

/// How many ended sessions, and how many answered calls, `Shells` keeps; and
/// how much of a cell's script.
const ENDED_KEPT: usize = 256;
const ANSWERED_KEPT: usize = 32;
const TEXT_KEPT: usize = 8192;

impl Shells {
    /// Whether a rollout line may say anything of a shell, before it is parsed.
    pub(crate) fn wants(line: &str) -> bool {
        line.contains("exec_command") || line.contains("write_stdin") || line.contains("_call_output") || line.contains("CommandExecution")
    }

    /// One rollout line, its text.
    pub(crate) fn line(&mut self, line: &str) {
        if !Self::wants(line) {
            return;
        }
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            self.take(&v);
        }
    }

    /// One rollout line, parsed.
    pub(crate) fn take(&mut self, v: &Value) {
        self.seq += 1;
        let seq = self.seq;
        let at_ms = v["timestamp"].as_str().and_then(|t| t.parse::<jiff::Timestamp>().ok()).map(|t| t.as_millisecond());
        let payload = &v["payload"];
        match (v["type"].as_str(), payload["type"].as_str()) {
            (Some("response_item"), Some("function_call")) => {
                let Some(id) = payload["call_id"].as_str() else { return };
                let args = payload["arguments"].as_str().and_then(|a| serde_json::from_str::<Value>(a).ok()).unwrap_or(Value::Null);
                let at_ms = at_ms.unwrap_or(0);
                let call = match payload["name"].as_str() {
                    Some("exec_command") => {
                        let cmd = match &args["cmd"] {
                            Value::String(s) => Some(s.clone()),
                            Value::Array(words) => Some(words.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ")),
                            _ => None,
                        };
                        Call { runs: true, text: cmd.clone().unwrap_or_default(), cmd, writes_to: None, seq, at_ms }
                    }
                    Some("write_stdin") => Call { writes_to: session_of(&args["session_id"]), seq, at_ms, ..Default::default() },
                    _ => return,
                };
                self.asked.insert(id.to_string(), call);
            }
            (Some("response_item"), Some("custom_tool_call")) => {
                let (Some(id), Some(code)) = (payload["call_id"].as_str(), payload["input"].as_str()) else { return };
                if code.contains("exec_command") {
                    let cmd = cell_command(code);
                    let text = cmd.clone().unwrap_or_else(|| code.chars().take(TEXT_KEPT).collect());
                    self.asked.insert(id.to_string(), Call { runs: true, cmd, writes_to: None, text, seq, at_ms: at_ms.unwrap_or(0) });
                }
            }
            (Some("response_item"), Some("function_call_output" | "custom_tool_call_output")) => {
                // An output of no call seen here says nothing of a shell.
                let Some(call) = payload["call_id"].as_str().and_then(|id| self.asked.remove(id)) else { return };
                for text in output_texts(&payload["output"]) {
                    // A header, the lines before `Output:`, says how it stands.
                    let head: Vec<&str> = text.lines().take_while(|l| l.trim_end() != "Output:").collect();
                    if let Some(n) =
                        call.writes_to.as_deref().filter(|_| head.iter().any(|l| l.trim().starts_with("Process exited with code ")))
                    {
                        self.end(n);
                    }
                    if !call.runs {
                        continue;
                    }
                    if let Some(n) = head.iter().find_map(|l| l.trim().strip_prefix("Process running with session ID ")) {
                        self.started(n.trim(), &call);
                    }
                    // Code mode: a result object as the script printed it.
                    for n in running_results(&text) {
                        self.started(&n, &call);
                    }
                }
                if let (false, Some(at)) = (call.text.is_empty(), at_ms) {
                    if self.answered.len() == ANSWERED_KEPT {
                        self.answered.pop_front();
                    }
                    self.answered.push_back((call.text, at));
                }
            }
            (Some("event_msg"), Some("item_completed")) if payload["item"]["type"] == "CommandExecution" => {
                self.ends_recorded = true;
                if let Some(n) = session_of(&payload["item"]["process_id"]) {
                    self.end(&n);
                }
            }
            _ => {}
        }
    }

    /// A result of `call` says `session` runs.
    fn started(&mut self, session: &str, call: &Call) {
        let ended_since = self.ended.iter().rev().find(|(s, _)| s == session).is_some_and(|(_, at)| *at > call.seq);
        if session.is_empty() || ended_since {
            return;
        }
        self.running.retain(|(s, _, _)| s != session);
        self.running.push((session.to_string(), call.cmd.clone(), call.at_ms));
    }

    fn end(&mut self, session: &str) {
        self.running.retain(|(s, _, _)| s != session);
        if self.ended.len() == ENDED_KEPT {
            self.ended.pop_front();
        }
        self.ended.push_back((session.to_string(), self.seq));
    }

    /// What runs, by session id, each with its name.
    pub(crate) fn running(&self) -> Vec<(String, String)> {
        self.running_since().into_iter().map(|(id, name, _)| (id, name)).collect()
    }

    /// What runs, by session id, each with its name and when the call that
    /// started it was made (epoch ms, 0 for none that read).
    pub(crate) fn running_since(&self) -> Vec<(String, String, i64)> {
        let short = |s: &str| {
            let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
            if s.chars().count() > 160 {
                format!("{}…", s.chars().take(160).collect::<String>())
            } else {
                s
            }
        };
        self.running
            .iter()
            .map(|(id, cmd, at)| {
                let name = match cmd {
                    Some(cmd) => format!("exec_command `{}`", short(cmd)),
                    None => format!("exec_command session {id}"),
                };
                (id.clone(), name, *at)
            })
            .collect()
    }

    /// Whether the rollout has recorded any process's end: where it has not,
    /// it may never (0.169.0's legacy persistence).
    pub(crate) fn ends_recorded(&self) -> bool {
        self.ends_recorded
    }

    /// The last calls that ran a command and have their output, oldest
    /// first: what each ran, and when its output was written (epoch ms).
    pub(crate) fn answered(&self) -> Vec<(String, i64)> {
        self.answered.iter().cloned().collect()
    }
}

/// A session id as Codex writes it: a number, or a number as text.
fn session_of(v: &Value) -> Option<String> {
    v.as_u64().map(|n| n.to_string()).or_else(|| v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned))
}

/// The texts of a tool output: the output itself, or each text block of it.
fn output_texts(out: &Value) -> Vec<String> {
    match out.as_str() {
        Some(s) => vec![s.to_string()],
        None => out.as_array().into_iter().flatten().filter_map(|b| b["text"].as_str().map(str::to_owned)).collect(),
    }
}

/// The session ids of unified exec results still running in one text of a
/// code-mode cell's output: the text is a result object (or a list of them),
/// as `JSON.stringify` prints one, with a `session_id`, a `wall_time_seconds`
/// and no `exit_code`.
fn running_results(text: &str) -> Vec<String> {
    let text = text.trim();
    if !(text.starts_with('{') || text.starts_with('[')) || !text.contains("session_id") {
        return Vec::new();
    }
    let Ok(v) = serde_json::from_str::<Value>(text) else { return Vec::new() };
    let one = |r: &Value| {
        let running = r["wall_time_seconds"].is_number() && r.get("exit_code").is_none_or(Value::is_null);
        running.then(|| session_of(&r["session_id"])).flatten()
    };
    match &v {
        Value::Array(items) => items.iter().filter_map(one).collect(),
        _ => one(&v).into_iter().collect(),
    }
}

/// The command of a code-mode cell's one `exec_command` call, where its `cmd`
/// is a string literal: none for a cell with more than one, or a command made
/// at run time.
fn cell_command(code: &str) -> Option<String> {
    let mut calls = code.match_indices("exec_command(");
    let (at, _) = calls.next()?;
    if calls.next().is_some() {
        return None;
    }
    let rest = &code[at..];
    let key = rest.find("cmd")?;
    let value = rest[key + 3..].trim_start_matches(['"', '\'']).trim_start().strip_prefix(':')?.trim_start();
    let quote = value.chars().next().filter(|c| matches!(c, '"' | '\'' | '`'))?;
    let mut out = String::new();
    let mut chars = value[1..].chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                other => out.push(other),
            },
            c if c == quote => return Some(out),
            // A template with a placeholder is made at run time.
            '$' if quote == '`' => return None,
            c => out.push(c),
        }
    }
    None
}

/// Whether a Codex screen shows an approval dialog — a command to run, an edit
/// to make, a permission to grant. The rollout records none of them; each ends
/// in the same footer. Looked for only while the rollout says a turn runs, and
/// taken only where the chat's status hooks are not delivering
/// (`activity::fold_codex`): its `PermissionRequest` says it otherwise.
pub fn approval_on(screen: &str) -> bool {
    const MARKS: [&str; 3] = [
        "Press enter to confirm or esc to cancel",
        "Would you like to run the following command?",
        "Would you like to make the following edits?",
    ];
    MARKS.iter().any(|m| screen.contains(m))
}

/// `working` | `resting` off the rollout's own turn events: a turn begun
/// (`task_started`) and not yet over is working, whatever its screen does. An
/// approval dialog is not in the rollout, so such a turn reads working.
fn fold_status(working: Option<bool>) -> &'static str {
    if working == Some(true) {
        "working"
    } else {
        "resting"
    }
}

/// What a live session's rollout says of its turn, for its lifecycle record
/// (`activity`); None until the rollout is found, which comes with the first
/// turn.
pub fn rollout_read(live: &Live) -> Option<crate::activity::RolloutRead> {
    let path = rollout_for(live)?;
    // A file that will not open now says nothing of the turn: the scan kept
    // from the last read is no word on what came since.
    let len = match std::fs::File::open(&path).and_then(|f| f.metadata()) {
        Ok(m) => m.len(),
        Err(_) => return Some(crate::activity::RolloutRead { unreadable: true, ..Default::default() }),
    };
    let (scan, through) = tracked_fresh(live, path.clone())?;
    // Opened, then not read through (the reopen, a seek or a read failed):
    // the same.
    if !through {
        return Some(crate::activity::RolloutRead { unreadable: true, ..Default::default() });
    }
    let working = scan.working == Some(true);
    Some(crate::activity::RolloutRead {
        working,
        seen_turn: scan.working.is_some(),
        first_started_ms: scan.first_started_ms.and_then(|t| u64::try_from(t).ok()),
        len,
        unreadable: false,
        completed: scan.completed,
        // Only a question that holds the turn: an async one lets the model work on.
        asks: working && scan.ask.as_ref().is_some_and(|(_, a)| a.blocking),
        limit: (!working && scan.failed.is_some()).then(|| rollout_limit(&path).map(|(kind, _)| kind)).flatten(),
        failed: scan.failed,
        shells: scan.shells.running_since().into_iter().map(|(id, name, at)| (id, name, u64::try_from(at).unwrap_or(0))).collect(),
        shell_ends_recorded: scan.shells.ends_recorded(),
        answered: scan.shells.answered().into_iter().filter_map(|(text, at)| Some((text, u64::try_from(at).ok()?))).collect(),
        last_end_ms: scan.last_end_ms.and_then(|t| u64::try_from(t).ok()),
        turn_started_ms: scan.turn_started_ms.and_then(|t| u64::try_from(t).ok()),
    })
}

/// Where the session works, per its rollout: the folder it was started in,
/// or the worktree `--worktree` made for it.
pub fn rollout_cwd(live: &Live) -> Option<String> {
    first_cwd(&rollout_for(live)?)
}

/// The cwd a rollout's first line names, read again only once the file has
/// changed: the line runs to tens of KB with Codex's instructions.
fn first_cwd(path: &Path) -> Option<String> {
    static SEEN: std::sync::LazyLock<crate::stamped::ByStamp<Option<String>>> =
        std::sync::LazyLock::new(|| crate::stamped::ByStamp::new(256));
    // A first line that does not read (not there, half written) keeps nothing.
    SEEN.get(path, |path| first_line(path).map(|v| v["payload"]["cwd"].as_str().map(str::to_owned))).flatten()
}

/// Status, thread id, model, context, name and opening prompt of a live session.
/// None until the rollout file exists, which is a normal early answer.
pub fn meta(live: &Live) -> Option<SessionMeta> {
    let path = rollout_for(live)?;
    let name = path.file_name()?.to_string_lossy().into_owned();
    let (_, id) = parse_name(&name)?;
    let t = read_tail(&path);

    let scan = tracked(live);
    let status = Some(fold_status(scan.as_ref().and_then(|s| s.working)));
    let named = thread_title(live.account_path, &id);

    let mut m = SessionMeta {
        status: status.map(str::to_owned),
        cwd: first_cwd(&path).or_else(|| Some(live.cwd.to_string())),
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
/// last couple of days, with the line's timestamp. Newest by that timestamp:
/// a name carries when the thread started, and the file written to last may
/// be an older conversation resumed that has not said its limits since.
/// The files are read written-to-last first, and only until one was last
/// written before the newest snapshot found, which holds none newer.
pub fn latest_rate_limits(account_path: &str) -> Option<(Value, String)> {
    let today = local_today();
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
        let at = hit.1.parse::<jiff::Timestamp>().map_or(i64::MIN, |t| t.as_millisecond());
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
    let mut said = Conversation::default();
    let read = crate::pty::complete_lines(path, from, |line| said.line(line));
    (said.finish(|said| from > 0 && said_before(path, from, said)), read)
}

/// `messages_from` of a rollout's bytes in memory, from the top: the messages of
/// its complete lines, and the offset just past the last of them.
pub fn messages_in(bytes: &[u8]) -> (Vec<crate::agents::Message>, usize) {
    let mut said = Conversation::default();
    let read = crate::pty::complete_lines_in(bytes, |line| said.line(line));
    (said.finish(|_| false), read)
}

/// How far back a read from a cursor looks for a message the turn's end
/// repeats (`Conversation::finish`).
const LOOKBACK: u64 = 256 * 1024;

/// Whether the last turn before byte `from` of the rollout at `path` said
/// `said` (as `Conversation::same_words` has it): a read that opened at the
/// turn's end line, its message handed over in the read before.
fn said_before(path: &Path, from: u64, said: &str) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let start = from.saturating_sub(LOOKBACK);
    let mut window = Vec::new();
    let read = std::fs::File::open(path).and_then(|mut f| {
        f.seek(SeekFrom::Start(start))?;
        f.take(from - start).read_to_end(&mut window)
    });
    if read.is_err() {
        return false;
    }
    // A window that opens mid-file opens mid-line.
    let skip = if start > 0 { window.iter().position(|b| *b == b'\n').map_or(window.len(), |i| i + 1) } else { 0 };
    let mut earlier = Conversation::default();
    crate::pty::complete_lines_in(&window[skip..], |line| earlier.line(line));
    earlier.messages[earlier.turn_end.min(earlier.messages.len())..]
        .iter()
        .any(|m| m.role == "assistant" && Conversation::same_words(&m.text, said))
}

/// A rollout's conversation, one complete line at a time (`messages_from`).
/// Its messages come from the `response_item` lines; where they do not say
/// which is a turn's final answer (no `phase`, as before 0.167), or leave it
/// out, the turn's end does: `task_complete.last_agent_message` is Codex's own
/// word for what the turn ended saying.
#[derive(Default)]
pub(super) struct Conversation {
    pub(super) messages: Vec<crate::agents::Message>,
    /// Where the turn now running began among `messages`, if its start was
    /// read here: a read from a cursor may open in the middle of one.
    turn_from: Option<usize>,
    /// Where the last turn read here ended.
    turn_end: usize,
    /// Turns that ended in this read, begun before it, saying something none
    /// of the messages read of them does: where among `messages` (the end of
    /// what was read of the turn) and what (`finish`).
    orphans: Vec<(usize, String)>,
}

impl Conversation {
    pub(super) fn line(&mut self, line: &str) {
        let response = line.contains("\"response_item\"");
        // The many token counts and item events are not parsed for this.
        const ENDS: [&str; 5] = ["\"task_started\"", "\"turn_started\"", "\"task_complete\"", "\"turn_complete\"", "\"turn_aborted\""];
        if !response && !(line.contains("\"event_msg\"") && ENDS.iter().any(|e| line.contains(e))) {
            return;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { return };
        if response {
            self.messages.extend(message_of(&v));
            return;
        }
        let payload = &v["payload"];
        match payload["type"].as_str() {
            Some("task_started" | "turn_started") => self.turn_from = Some(self.messages.len()),
            Some("task_complete" | "turn_complete") => {
                if let Some(said) = payload["last_agent_message"].as_str().map(str::trim).filter(|s| !s.is_empty()) {
                    self.ended_saying(said);
                }
                self.turn_from = None;
                self.turn_end = self.messages.len();
            }
            Some("turn_aborted") => {
                self.turn_from = None;
                self.turn_end = self.messages.len();
            }
            _ => {}
        }
    }

    /// Whether two texts are the same words: Luna joins the blocks of a
    /// message with a newline where Codex's `last_agent_message` runs them
    /// together, so whitespace is not compared.
    fn same_words(a: &str, b: &str) -> bool {
        let mut a = a.chars().filter(|c| !c.is_whitespace());
        let mut b = b.chars().filter(|c| !c.is_whitespace());
        loop {
            match (a.next(), b.next()) {
                (None, None) => return true,
                (x, y) if x == y => {}
                _ => return false,
            }
        }
    }

    /// A turn ended with `said`. The assistant message of it that says that
    /// (`same_words`) is its final answer, `phase` or not. A turn read whole
    /// that has no assistant message at all in the rollout's `response_item`s
    /// gets it as its one: a report is never lost for the shape its message
    /// was written in. A turn begun before this read (a cursor inside it, or
    /// at its end line alone) may have had its message in the read before: it
    /// is only marked, or kept for `finish` to look back for.
    fn ended_saying(&mut self, said: &str) {
        let (from, whole) = (self.turn_from.unwrap_or(self.turn_end), self.turn_from.is_some());
        let from = from.min(self.messages.len());
        let turn = &mut self.messages[from..];
        if let Some(m) = turn.iter_mut().rev().find(|m| m.role == "assistant" && Self::same_words(&m.text, said)) {
            m.final_answer = true;
        } else if !whole {
            self.orphans.push((self.messages.len(), said.to_string()));
        } else if !turn.iter().any(|m| m.role == "assistant") {
            self.messages.push(crate::agents::Message {
                role: "assistant".into(),
                text: said.to_string(),
                after_cli: false,
                final_answer: true,
            });
        }
    }

    /// The messages read. Turns that ended in this read, begun before it, with
    /// no message of it that says what its end does, get it as theirs, unless
    /// `before` says the read before this one handed it over.
    pub(super) fn finish(mut self, before: impl Fn(&str) -> bool) -> Vec<crate::agents::Message> {
        for (at, said) in std::mem::take(&mut self.orphans).into_iter().rev() {
            if !before(&said) {
                let message = crate::agents::Message { role: "assistant".into(), text: said, after_cli: false, final_answer: true };
                self.messages.insert(at, message);
            }
        }
        self.messages
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The message one rollout line comes to on its own.
    fn message_in(line: &str) -> Option<crate::agents::Message> {
        let mut said = Conversation::default();
        said.line(line);
        said.finish(|_| false).into_iter().next()
    }

    #[test]
    fn commands_left_running_past_their_call_are_named_until_they_exit() {
        let call = |id: &str, name: &str, args: Value| {
            serde_json::json!({ "type": "response_item", "payload": { "type": "function_call", "name": name, "call_id": id, "arguments": args.to_string() } })
                .to_string()
        };
        let out = |id: &str, head: &str| {
            serde_json::json!({ "type": "response_item", "payload": { "type": "function_call_output", "call_id": id, "output": format!("Chunk ID: 1\nWall time: 10 seconds\n{head}\nOutput:\nProcess running with session ID 9 is not this") } })
                .to_string()
        };
        let lines = [
            call("c1", "exec_command", serde_json::json!({ "cmd": "pnpm test --watch" })),
            out("c1", "Process running with session ID 7"),
            call("c2", "exec_command", serde_json::json!({ "cmd": ["bash", "-lc", "sleep 1"] })),
            out("c2", "Process exited with code 0"),
            call("c3", "exec_command", serde_json::json!({ "cmd": "tail -f lane.log" })),
            out("c3", "Process running with session ID 8"),
        ];
        assert_eq!(background_calls_in(&lines.join("\n")), ["exec_command `pnpm test --watch`", "exec_command `tail -f lane.log`"]);
        // Written to and found exited: no longer left running.
        let done =
            [call("c4", "write_stdin", serde_json::json!({ "session_id": 7, "chars": "" })), out("c4", "Process exited with code 0")];
        let all = format!("{}\n{}", lines.join("\n"), done.join("\n"));
        assert_eq!(background_calls_in(&all), ["exec_command `tail -f lane.log`"]);
    }

    /// Code mode (0.169.0): a shell is running while the cell's printed
    /// result has its `session_id` and no `exit_code`, and ends with its
    /// `CommandExecution` item, written after the turn it outlived. The live
    /// fixture, read up to the line before that end and through it.
    #[test]
    fn a_code_mode_shell_runs_until_its_item_completes() {
        let bytes = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/transcripts/codex/code-mode.jsonl"));
        let text = std::str::from_utf8(bytes).unwrap();
        let end = text.find(r#""type":"CommandExecution","id":"exec-9e94ad1c"#).unwrap();
        let before = &text[..text[..end].rfind('\n').unwrap() + 1];
        let ping = ("27586".to_string(), "exec_command `ping -n 20 127.0.0.1`".to_string());
        assert_eq!(read_rollout(before.as_bytes(), 0).shells, std::slice::from_ref(&ping), "past its turn's end, still running");
        assert!(before.contains("\"task_complete\""), "the turn that started it is over");
        assert_eq!(background_calls_in(before), std::slice::from_ref(&ping.1));
        assert!(read_rollout(bytes, 0).shells.is_empty(), "its end written; the interrupted sleep never reported running");
        // Lines of an earlier process start nothing of this one's.
        assert!(read_rollout(before.as_bytes(), i64::MAX).shells.is_empty());
        // What each call that ran a command answered, by its output line's time.
        let mut shells = Shells::default();
        for line in text.lines() {
            shells.line(line);
        }
        let at = |t: &str| t.parse::<jiff::Timestamp>().unwrap().as_millisecond();
        assert!(shells.ends_recorded());
        assert_eq!(
            shells.answered(),
            [
                ("ping -n 20 127.0.0.1".to_string(), at("2026-11-19T23:37:17.788Z")),
                ("Start-Sleep -Seconds 40".to_string(), at("2026-11-19T23:38:00.526Z"))
            ]
        );
    }

    /// What a cell's script printed, read for shells still running.
    #[test]
    fn a_cells_printed_results_say_which_shells_run() {
        let out = |call: &str, texts: &[&str]| {
            let output: Vec<Value> = texts.iter().map(|t| serde_json::json!({ "type": "input_text", "text": t })).collect();
            serde_json::json!({ "type": "response_item", "payload": { "type": "custom_tool_call_output", "call_id": call, "output": output } })
                .to_string()
        };
        let cell = |call: &str, code: &str| {
            serde_json::json!({ "type": "response_item", "payload": { "type": "custom_tool_call", "name": "exec", "call_id": call, "input": code } })
                .to_string()
        };
        let done = |n: u64| {
            serde_json::json!({ "type": "event_msg", "payload": { "type": "item_completed", "item": { "type": "CommandExecution", "process_id": n.to_string(), "status": "completed" } } })
                .to_string()
        };
        let lines = [
            cell("a", "const r = await tools.exec_command({cmd: 'npm run dev', yield_time_ms: 1000});\ntext(JSON.stringify(r));"),
            out("a", &["Script completed\nWall time 1.2 seconds\nOutput:\n", r#"{"chunk_id":"1","wall_time_seconds":1.0,"session_id":7,"output":"ready"}"#]),
            // Two calls in one cell: no one command to name it by.
            cell("b", "const x = await tools.exec_command({cmd:\"a\"}); const y = await tools.exec_command({cmd:\"b\"});\ntext(JSON.stringify([x, y]));"),
            out("b", &[r#"[{"wall_time_seconds":1,"session_id":8,"output":""},{"wall_time_seconds":1,"exit_code":0,"output":""}]"#]),
            // Done within its call, or printed as text: nothing left running.
            cell("c", "text(JSON.stringify(await tools.exec_command({cmd:\"ls\"})));"),
            out("c", &[r#"{"wall_time_seconds":0.1,"exit_code":0,"session_id":null,"output":"a.txt"}"#, "session_id 9 is not a result"]),
            // The plain tool's header, from its call.
            serde_json::json!({ "type": "response_item", "payload": { "type": "function_call", "name": "exec_command", "call_id": "d", "arguments": "{\"cmd\":\"tail -f a.log\"}" } })
                .to_string(),
            out("d", &["Chunk ID: 2\nWall time: 1.0000 seconds\nProcess running with session ID 10\nOutput:\n"]),
            // An output of no call that runs a command, shaped like a result
            // or with the header: nothing.
            cell("g", "text(JSON.stringify({session_id: 11, wall_time_seconds: 1}));"),
            out("g", &[r#"{"wall_time_seconds":1,"session_id":11,"output":""}"#]),
            out("h", &["Process running with session ID 12\nOutput:\n"]),
        ];
        let mut shells = Shells::default();
        for l in &lines {
            shells.line(l);
        }
        let ids = |s: &Shells| s.running().into_iter().map(|(id, _)| id).collect::<Vec<_>>();
        assert_eq!(shells.running()[0].1, "exec_command `npm run dev`");
        assert_eq!(shells.running()[1].1, "exec_command session 8");
        assert_eq!(ids(&shells), ["7", "8", "10"]);
        shells.line(&done(8));
        assert_eq!(ids(&shells), ["7", "10"]);
        // A result printed after its end by a call made before it, a cell that
        // went on past it, starts nothing.
        shells.line(&cell("e", "const r = await tools.exec_command({cmd:\"sleep 1\"}); text(JSON.stringify(r));"));
        shells.line(&done(9));
        shells.line(&out("e", &[r#"{"wall_time_seconds":1,"session_id":9,"output":""}"#]));
        assert_eq!(ids(&shells), ["7", "10"]);
        // An id Codex gives again, to a call made after the end: a new process.
        shells.line(&cell("f", "const r = await tools.exec_command({cmd:\"npm run dev\"}); text(JSON.stringify(r));"));
        shells.line(&out("f", &[r#"{"wall_time_seconds":1,"session_id":8,"output":""}"#]));
        assert_eq!(ids(&shells), ["7", "10", "8"]);
    }

    /// 0.169.0 keeps the error a turn ended on in its `task_complete`, with
    /// no `error` event beside it: its `codex_error_info` names the limit.
    #[test]
    fn a_limit_in_a_turns_end_is_read() {
        let end = |info: &str| {
            format!(
                "{{\"timestamp\":\"t\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"task_started\"}}}}\n{{\"timestamp\":\"t\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"task_complete\",\"error\":{{\"message\":\"Quota exceeded.\",\"codex_error_info\":{info}}}}}}}"
            )
        };
        assert_eq!(limit_in(&end("\"usage_limit_exceeded\"")), Some("usage_limit"));
        assert_eq!(limit_in(&end("{\"rate_limit_exceeded\":{}}")), Some("rate_limit"));
        assert_eq!(limit_in(&end("\"server_overloaded\"")), None);
        assert_eq!(limit_in(&end("null")), None);
    }

    #[test]
    fn a_cells_one_command_is_read_from_its_literal() {
        assert_eq!(
            cell_command(r#"await tools.exec_command({cmd: "ping -n 2 \"x\"", yield_time_ms: 1})"#).as_deref(),
            Some(r#"ping -n 2 "x""#)
        );
        assert_eq!(cell_command(r#"await tools.exec_command({"cmd":"ls"})"#).as_deref(), Some("ls"));
        assert_eq!(cell_command("await tools.exec_command({ cmd : `tail -f a.log` })").as_deref(), Some("tail -f a.log"));
        assert_eq!(cell_command("await tools.exec_command({cmd: `run ${name}`})"), None, "made at run time");
        assert_eq!(cell_command("await tools.exec_command({cmd: c})"), None);
        assert_eq!(cell_command("await tools.exec_command({cmd: 'a'}); await tools.exec_command({cmd: 'b'})"), None);
        assert_eq!(cell_command("await tools.exec_command({cmd: \"unterminated"), None);
        assert_eq!(limit_of_message("You\u{2019}ve hit your usage limit."), Some("usage_limit"));
        assert_eq!(limit_of_message("rate limited"), None);
    }

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
        let start = at.parse::<jiff::Timestamp>().unwrap().as_millisecond() as u64;
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
