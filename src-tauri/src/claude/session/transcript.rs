//! Where a session's transcript is — `<config>/projects/<cwd>/<sid>.jsonl`, or
//! wherever its id is found — and the conversation read from it for another
//! agent.

use super::*;

// Claude Code encodes a project cwd into a transcript folder name by replacing
// every non-alphanumeric character with '-'.
fn encode_project_dir(p: &str) -> String {
    p.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

fn transcript_path(account_path: &str, cwd: &str, session_id: &str) -> PathBuf {
    Path::new(account_path).join("projects").join(encode_project_dir(cwd)).join(format!("{session_id}.jsonl"))
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
    std::fs::read_dir(Path::new(account_path).join("projects")).ok()?.flatten().map(|e| e.path().join(&file)).find(|p| p.is_file())
}

/// Text of a transcript content field, which is either a bare string or an
/// array of blocks.
pub(super) fn content_text(content: &serde_json::Value) -> Option<String> {
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
pub(super) fn located_transcript(account_path: &str, cwd: &str, session_id: &str) -> PathBuf {
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
    let mut said = Conversation::new(with_tools);
    let read = crate::pty::complete_lines(path, from, |line| said.line(line));
    (said.messages, read)
}

/// The conversation `messages_from` reads, taken in a line at a time.
pub(super) struct Conversation {
    with_tools: bool,
    /// A prompt the CLI put in itself is left out, but what answers it opens
    /// a turn of its own (`Message::after_cli`).
    cli_prompt: bool,
    pub(super) messages: Vec<crate::agents::Message>,
}

impl Conversation {
    pub(super) fn new(with_tools: bool) -> Self {
        Conversation { with_tools, cli_prompt: false, messages: Vec::new() }
    }

    pub(super) fn line(&mut self, line: &str) {
        match message_of(line, self.with_tools) {
            Some(Line::Said(mut m)) => {
                if m.role == "assistant" {
                    m.after_cli = std::mem::take(&mut self.cli_prompt);
                } else {
                    self.cli_prompt = false;
                }
                self.messages.push(m);
            }
            Some(Line::Injected) => self.cli_prompt = true,
            None => {}
        }
    }
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
        found = (v["type"] == "user" && v["uuid"] == uuid)
            || (v["type"] == "attachment" && attached["type"] == "queued_command" && attached["source_uuid"] == uuid);
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
    if v["isCompactSummary"].as_bool() == Some(true)
        || v["isVisibleInTranscriptOnly"].as_bool() == Some(true)
        || v["origin"]["kind"] == "task-notification"
    {
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
mod tests {
    use super::*;

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
