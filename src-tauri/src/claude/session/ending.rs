//! What the end of a transcript says of the session: the context and model of
//! the last answer, the words it ended on, the API error a turn ended on, the
//! tool call it stands on, and when what it waits on is due.

use super::*;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ContextRead {
    pub(super) tokens: f64,
    pub(super) window: f64,
}

// Free context reading: the last assistant message in the session transcript
// carries cumulative input-side token usage. Returns raw tokens; the fraction
// is derived from it.
fn context_in(text: &str) -> Option<ContextRead> {
    usage_in(text).map(|(tokens, model)| ContextRead { tokens, window: crate::claude::models::context_window(&model) })
}

/// See `context_in`: the tokens, and the model whose window they fill.
pub(super) fn usage_in(text: &str) -> Option<(f64, String)> {
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
                return Some((total, v["message"]["model"].as_str().unwrap_or("").to_string()));
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
pub(super) fn model_in(text: &str) -> Option<String> {
    text.lines().rev().filter(|l| l.contains("\"model\"")).find_map(|line| {
        let v = serde_json::from_str::<serde_json::Value>(line).ok()?;
        if v["isSidechain"].as_bool().unwrap_or(false) {
            return None;
        }
        v["message"]["model"].as_str().filter(|m| !m.starts_with('<')).map(str::to_owned)
    })
}

/// What the end of a transcript says of the session: the context its last
/// answer used and the model that wrote it, the text the model last wrote
/// since the last prompt — what a turn that is over ended on — and the API
/// error that turn ended on, if it did.
#[derive(Clone, Default)]
pub(super) struct Ending {
    pub(super) context: Option<ContextRead>,
    pub(super) last_said: Option<String>,
    /// When those words were written, by their line's timestamp.
    pub(super) last_said_at: Option<String>,
    model: Option<String>,
    pub(super) api_error: Option<serde_json::Value>,
}

/// The transcript's `Ending`, read back from the end in growing pieces: after
/// a turn Claude Code writes lines of 100 KB and more — the first turn's whole
/// system prompt (an attachment, `prompt_snapshot`, 150 KB), `api-request-shape`
/// and `-blob` lines — which a short tail is all of. Kept per file length and
/// change time, as it is asked for every few seconds and changes only when the
/// file grows.
pub(super) fn ending(path: &Path) -> Ending {
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
pub(super) fn last_said_in(text: &str) -> Option<Option<(String, Option<String>)>> {
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
                let said: Vec<&str> =
                    content.as_array().into_iter().flatten().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect();
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

/// See `last_api_error`, of a text alone.
#[cfg(test)]
fn api_error_in(text: &str) -> Option<&'static str> {
    api_error_line(text).flatten().map(|v| crate::activity::api_error_kind(v["error"].as_str().unwrap_or("")))
}

/// The last user or assistant line decides: Some(the error line, if it is
/// one), or None when the text does not reach back to either.
pub(super) fn api_error_line(text: &str) -> Option<Option<serde_json::Value>> {
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
    Some(api_error_text(&ending(path).api_error?))
}

/// See `api_error_of`, of the error line.
pub(super) fn api_error_text(v: &serde_json::Value) -> String {
    let kind = crate::activity::api_error_kind(v["error"].as_str().unwrap_or(""));
    match content_text(&v["message"]["content"]).filter(|t| !t.trim().is_empty()) {
        Some(said) => format!("{kind}: {said}"),
        None => kind.to_string(),
    }
}

/// The tool call the conversation stands on:the last one the model made that
/// no result has come back for, as (name, input). `AskUserQuestion` waits on
/// a person by its nature; any other tool waits on one only while a
/// permission prompt is up — otherwise it is simply running.
pub fn pending_tool(path: &Path) -> Option<(String, serde_json::Value)> {
    pending_tool_in(&crate::pty::tail(path, 256 * 1024)?)
}

pub(super) fn pending_tool_in(text: &str) -> Option<(String, serde_json::Value)> {
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

pub(super) fn due_in(text: &str) -> Option<u64> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_model_is_the_main_loop_s_last() {
        let path = std::env::temp_dir().join(format!("luna-last-model-{}.jsonl", std::process::id()));
        std::fs::write(
            &path,
            concat!(
                r#"{"type":"assistant","message":{"model":"claude-opus-5-5","content":[]}}"#,
                "\n",
                r#"{"type":"assistant","message":{"model":"claude-haiku-5-5","content":[]}}"#,
                "\n",
                r#"{"type":"assistant","isSidechain":true,"message":{"model":"claude-sonnet-5-5","content":[]}}"#,
                "\n",
                r#"{"type":"assistant","message":{"model":"<synthetic>","content":[]}}"#,
                "\n",
                r#"{"type":"user","message":{"content":"the word \"model\" in passing"}}"#,
                "\n",
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
            r#"{"type":"user","message":{"role":"user","content":"go on"}}"#,
            "\n",
            r#"{"type":"assistant","isApiErrorMessage":true,"error":"rate_limit","message":{"content":[{"type":"text","text":"You've hit your session limit"}]}}"#,
            "\n",
            r#"{"type":"system","subtype":"turn_duration"}"#,
            "\n",
        );
        assert_eq!(api_error_in(limited), Some("rate_limit"));
        // A prompt typed after the error is a new turn, not the stall.
        let retried = format!("{limited}{}\n", r#"{"type":"user","message":{"role":"user","content":"continue"}}"#);
        assert_eq!(api_error_in(&retried), None);
        // A sidechain's error is a subagent's, not the session's.
        let side = r#"{"type":"assistant","isSidechain":true,"isApiErrorMessage":true,"error":"rate_limit"}"#;
        assert_eq!(api_error_in(&format!("{}\n{side}\n", r#"{"type":"assistant","message":{}}"#)), None);
    }

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

    #[test]
    fn the_pending_tool_is_the_last_one_without_a_result() {
        let ask = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t2","name":"AskUserQuestion","input":{"questions":[{"question":"Which color?","options":[{"label":"Red"},{"label":"Blue"}],"multiSelect":false}]}}]}}"#;
        let ran = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}"#;
        let ran_done = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"a"}]}}"#;
        let text = [ran, ran_done, ask].join("\n");
        let (name, input) = pending_tool_in(&text).unwrap();
        assert_eq!(name, "AskUserQuestion");
        let a = crate::agents::Ask::from_claude_questions(&input).unwrap();
        assert_eq!((a.questions[0].text.as_str(), a.questions[0].options.len()), ("Which color?", 2));
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
            format!(
                r#"{{"type":"assistant","timestamp":"{at}","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"{name}","input":{input}}}]}}}}"#
            )
        };
        let result = |id: &str, made: &str| {
            format!(
                r#"{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"{id}","content":"…"}}]}},"toolUseResult":{made}}}"#
            )
        };
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
        let snapshot =
            format!(r#"{{"type":"attachment","attachment":{{"type":"prompt_snapshot","systemPrompt":["{}"]}}}}"#, "x".repeat(300 * 1024));
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
}
