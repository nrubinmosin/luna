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
/// read from the top. A read from a cursor gives what one from the top gives
/// from there: a CLI prompt the lines before it end on still makes what answers
/// it open a turn (`after_cli`). The cursor stays a byte offset its callers can
/// hold; the state at it comes from the read that handed it back, or failing
/// that is read back from the file within REBUILD_SCAN.
pub fn messages_from(path: &Path, from: u64, with_tools: bool) -> (Vec<crate::agents::Message>, u64) {
    let from = if std::fs::metadata(path).is_ok_and(|m| from > m.len()) { 0 } else { from };
    let pending = from > 0 && carried(path, from, with_tools).unwrap_or_else(|| cli_prompt_before(path, from, with_tools));
    let mut said = Conversation::resumed(with_tools, pending);
    let read = crate::pty::complete_lines(path, from, |line| said.line(line));
    carry(path, read, with_tools, said.cli_prompt);
    (said.messages, read)
}

/// `messages_from` of a file's bytes, `from` an offset into them.
pub fn messages_in(bytes: &[u8], from: usize, with_tools: bool) -> (Vec<crate::agents::Message>, usize) {
    let from = if from > bytes.len() { 0 } else { from };
    let pending = cli_prompt_back(from as u64, with_tools, |start, buf| {
        buf.copy_from_slice(&bytes[start as usize..start as usize + buf.len()]);
        true
    });
    let mut said = Conversation::resumed(with_tools, pending);
    let read = crate::pty::complete_lines_in(&bytes[from..], |line| said.line(line));
    (said.messages, from + read)
}

/// How much of a file the state at a cursor is read back from, in chunks of
/// REBUILD_CHUNK: at most REBUILD_SCAN bytes, and through no line longer than
/// REBUILD_LINE. A CLI prompt further back, or behind such a line, counts as
/// answered, and what answers it opens no turn of its own. So a read from a
/// cursor no read handed back costs this much I/O and memory at most, however
/// much the tool results since have grown the file.
const REBUILD_CHUNK: usize = 64 * 1024;
const REBUILD_SCAN: usize = 8 * 1024 * 1024;
const REBUILD_LINE: usize = 1024 * 1024;

/// What a read ended on, by file, cursor handed back and `with_tools`: whether
/// a CLI prompt was left open there, and the bytes just before the cursor, so a
/// file written anew under it is not taken for the one read. The read that goes
/// on from that cursor carries the state instead of reading it back, which a
/// caller polling a session does every few seconds.
type Carried = HashMap<(PathBuf, u64, bool), (Vec<u8>, bool)>;

fn carried_states() -> &'static Mutex<Carried> {
    static STATES: std::sync::OnceLock<Mutex<Carried>> = std::sync::OnceLock::new();
    STATES.get_or_init(Default::default)
}

/// The last bytes before `at`, which a cursor's state goes with.
fn before_cursor(path: &Path, at: u64) -> Option<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let start = at.saturating_sub(32);
    let mut bytes = vec![0; (at - start) as usize];
    let mut f = std::fs::File::open(path).ok()?;
    f.seek(SeekFrom::Start(start)).ok()?;
    f.read_exact(&mut bytes).ok()?;
    Some(bytes)
}

fn carried(path: &Path, from: u64, with_tools: bool) -> Option<bool> {
    let states = carried_states().lock().unwrap_or_else(|e| e.into_inner());
    let (before, pending) = states.get(&(path.to_path_buf(), from, with_tools))?;
    (before_cursor(path, from).as_ref() == Some(before)).then_some(*pending)
}

fn carry(path: &Path, at: u64, with_tools: bool, pending: bool) {
    let Some(before) = before_cursor(path, at) else { return };
    let mut states = carried_states().lock().unwrap_or_else(|e| e.into_inner());
    if states.len() >= CACHE_CAP {
        states.clear();
    }
    states.insert((path.to_path_buf(), at, with_tools), (before, pending));
}

/// Whether the file up to `from` ends on a prompt the CLI put in itself with
/// nothing said since (`cli_prompt_back`).
fn cli_prompt_before(path: &Path, from: u64, with_tools: bool) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    cli_prompt_back(from, with_tools, |start, buf| f.seek(SeekFrom::Start(start)).is_ok() && f.read_exact(buf).is_ok())
}

/// Whether the bytes before `from` end on a prompt the CLI put in itself with
/// nothing said since, read backwards a chunk at a time by `read_at(start,
/// buf)`, which fills `buf` from `start` or says it cannot. The line that
/// decides is nearly always the last one or close to it; the bounds are
/// REBUILD_SCAN and REBUILD_LINE.
fn cli_prompt_back(from: u64, with_tools: bool, mut read_at: impl FnMut(u64, &mut [u8]) -> bool) -> bool {
    // The front of what has been read: a line whose start is further back.
    let mut carried: Vec<u8> = Vec::new();
    let mut at = from;
    loop {
        let start = at.saturating_sub(REBUILD_CHUNK as u64);
        let mut bytes = vec![0; (at - start) as usize];
        if !read_at(start, &mut bytes) {
            return false;
        }
        bytes.extend_from_slice(&carried);
        // The lines this chunk completes: from its first newline on, or all
        // of it at the top of the file.
        let (front, whole) = match (start, bytes.iter().position(|b| *b == b'\n')) {
            (0, _) => (&[][..], &bytes[..]),
            (_, Some(i)) => (&bytes[..i], &bytes[i + 1..]),
            (_, None) => (&bytes[..], &[][..]),
        };
        if let Some(pending) = cli_prompt_at_end(&String::from_utf8_lossy(whole), with_tools) {
            return pending;
        }
        if start == 0 || front.len() > REBUILD_LINE || from - start >= REBUILD_SCAN as u64 {
            return false;
        }
        carried = front.to_vec();
        at = start;
    }
}

/// See `cli_prompt_before`, of a text: Some(whether its last turn line is a
/// prompt the CLI put in itself), None where it has no turn line.
fn cli_prompt_at_end(text: &str, with_tools: bool) -> Option<bool> {
    text.lines().rev().find_map(|line| match message_of(line, with_tools)? {
        Line::Injected => Some(true),
        Line::Said(_) => Some(false),
    })
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
        Self::resumed(with_tools, false)
    }

    /// One taken up where the lines before it left a CLI prompt open, or not.
    fn resumed(with_tools: bool, cli_prompt: bool) -> Self {
        Conversation { with_tools, cli_prompt, messages: Vec::new() }
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

    /// A file of `size` bytes of tool results after a CLI prompt, as a long
    /// tool run leaves one: lines of `line` bytes, no turn line among them.
    fn prompt_then_tool_results(path: &Path, size: usize, line: usize) -> u64 {
        use std::io::Write;
        let notice = r#"{"type":"user","origin":{"kind":"task-notification"},"message":{"role":"user","content":"<task-notification>\n<status>completed</status>\n</task-notification>"}}"#;
        let result = format!(
            r#"{{"type":"user","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"t1","content":"{}"}}]}}}}"#,
            "x".repeat(line.saturating_sub(100))
        );
        let mut out = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
        writeln!(out, "{notice}").unwrap();
        for _ in 0..size / (result.len() + 1) {
            writeln!(out, "{result}").unwrap();
        }
        out.flush().unwrap();
        std::fs::metadata(path).unwrap().len()
    }

    /// The state at a cursor is read back from the file in fixed chunks, at
    /// most REBUILD_SCAN bytes and no line longer than REBUILD_LINE, however
    /// large the file: a CLI prompt further back counts as answered.
    #[test]
    fn rebuilding_a_cursors_state_reads_a_bounded_tail_of_a_large_file() {
        use std::io::{Read, Seek, SeekFrom};
        let path = std::env::temp_dir().join(format!("luna-claude-rebuild-{}.jsonl", std::process::id()));
        let read_back = |from: u64| {
            let mut f = std::fs::File::open(&path).unwrap();
            let (mut bytes, mut peak) = (0, 0);
            let started = std::time::Instant::now();
            let pending = cli_prompt_back(from, false, |start, buf| {
                bytes += buf.len();
                peak = peak.max(buf.len());
                f.seek(SeekFrom::Start(start)).is_ok() && f.read_exact(buf).is_ok()
            });
            (pending, bytes, peak, started.elapsed())
        };
        // 200 MB of 64 KiB tool results behind the cursor.
        let end = prompt_then_tool_results(&path, 200 << 20, 64 << 10);
        let (pending, bytes, peak, took) = read_back(end);
        eprintln!("200 MB file: read {bytes} bytes, {peak} at most at once, in {took:?}");
        assert!(!pending, "too far back: answered");
        assert!(bytes <= REBUILD_SCAN + REBUILD_CHUNK && peak <= REBUILD_CHUNK, "{bytes} bytes read, {peak} at once");
        // One line of 4 MiB behind the cursor: given up on at REBUILD_LINE.
        let end = prompt_then_tool_results(&path, 8 << 20, 4 << 20);
        let (pending, bytes, _, _) = read_back(end);
        assert!(!pending && bytes <= REBUILD_LINE + 2 * REBUILD_CHUNK, "{bytes} bytes read");
        // Within the bounds the prompt is found, across chunks and lines.
        let end = prompt_then_tool_results(&path, 2 << 20, 300 << 10);
        assert!(read_back(end).0);
        let started = std::time::Instant::now();
        let answer = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Green."}]}}"#;
        std::io::Write::write_all(&mut std::fs::OpenOptions::new().append(true).open(&path).unwrap(), format!("{answer}\n").as_bytes())
            .unwrap();
        let (said, _) = messages_from(&path, end, false);
        assert!(said[0].after_cli && started.elapsed() < std::time::Duration::from_secs(10));
        let _ = std::fs::remove_file(&path);
    }

    /// Where a read stopped, the state it ended on goes on with the read from
    /// its cursor, however far back the prompt is: only a cursor no read
    /// handed back, or one into a file since written anew, is read back.
    #[test]
    fn a_read_carries_its_state_to_the_next_read_from_its_cursor() {
        let path = std::env::temp_dir().join(format!("luna-claude-carry-{}.jsonl", std::process::id()));
        let elsewhere = path.with_extension("copy.jsonl");
        prompt_then_tool_results(&path, REBUILD_SCAN * 2, 64 << 10);
        let (said, cursor) = messages_from(&path, 0, false);
        assert!(said.is_empty());
        let answer = br#"{"type":"assistant","message":{"content":[{"type":"text","text":"Green."}]}}"#;
        let grown = [std::fs::read(&path).unwrap(), answer.to_vec(), b"\n".to_vec()].concat();
        std::fs::write(&path, &grown).unwrap();
        std::fs::write(&elsewhere, &grown).unwrap();
        assert!(messages_from(&path, cursor, false).0[0].after_cli, "carried from the read that handed the cursor back");
        assert!(!messages_from(&elsewhere, cursor, false).0[0].after_cli, "read back: further than REBUILD_SCAN");
        // Written anew under the same cursor: not the file that read ended on.
        let mut anew = grown.clone();
        anew[cursor as usize - 2] = b' ';
        std::fs::write(&path, &anew).unwrap();
        assert!(!messages_from(&path, cursor, false).0[0].after_cli);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&elsewhere);
    }

    /// As a caller waiting on a session sees it: the CLI's own prompt lands in
    /// one read and what answers it in the next, past lines that are no turn
    /// (a system prompt's snapshot, 300 KB), and it still opens a turn.
    #[test]
    fn the_answer_to_the_clis_own_prompt_opens_a_turn_in_a_later_read() {
        let path = std::env::temp_dir().join(format!("luna-claude-after-cli-later-{}.jsonl", std::process::id()));
        let notice = r#"{"type":"user","origin":{"kind":"task-notification"},"message":{"role":"user","content":"<task-notification>\n<status>completed</status>\n</task-notification>"}}"#;
        let snapshot =
            format!(r#"{{"type":"attachment","attachment":{{"type":"prompt_snapshot","systemPrompt":["{}"]}}}}"#, "x".repeat(300 * 1024));
        let answer = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Green."}]}}"#;
        std::fs::write(&path, format!("{notice}\n{snapshot}\n")).unwrap();
        let (first, cursor) = messages_from(&path, 0, false);
        assert!(first.is_empty());
        std::fs::write(&path, format!("{notice}\n{snapshot}\n{answer}\n")).unwrap();
        let (later, _) = messages_from(&path, cursor, false);
        let _ = std::fs::remove_file(&path);
        assert_eq!(later, vec![crate::agents::Message { role: "assistant".into(), text: "Green.".into(), after_cli: true }]);
        // Said since: the prompt is answered, and a read after that starts afresh.
        let bytes = format!("{notice}\n{answer}\n{answer}\n");
        let after_first = notice.len() + answer.len() + 2;
        assert!(!messages_in(bytes.as_bytes(), after_first, false).0[0].after_cli);
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
