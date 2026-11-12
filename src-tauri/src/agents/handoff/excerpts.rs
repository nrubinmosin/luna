//! What a document Luna writes for a moved chat (document.rs) reads of the
//! chat's transcript, bounded in what it reads, not only in what it keeps:
//! the start of the file for the first prompt and the conversation's start,
//! and a window at its end for the turns that matter, each line no longer
//! than LINE_MAX (longer ones skipped and counted), the file's size and time
//! looked at before and after so a transcript that changed meanwhile is said
//! to have. Never `messages_from(0)`: both readers keep every message from
//! where they start, and one line can be as long as the CLI wrote it.
//!
//! Said lines are read by the CLIs' own readers (`said_line`), so what is
//! quoted is what `luna_message_list` would give, tool calls and their output
//! never. Beside them, from the same lines, what the document notes for
//! itself and is fragile by nature (shapes no CLI documents, left out
//! silently when they do not match): the last plan proposed (Claude Code's
//! `ExitPlanMode`, Codex's `update_plan`), Claude Code's last compaction
//! summary, and the paths its edit tools were called on.
//!
//! A document built after the chat was asked for its own leaves that ask out:
//! a turn opened by Luna's ask line (`Skip` says which), up to the next prompt, is the
//! handoff attempt, not the work, and neither its words nor what its tools
//! did are read. A window that starts inside a turn, its prompt before it,
//! is inside the ask's turn where the window's first line falls within the
//! move's ask, from its typing to its close (or still open).

use crate::provider::Provider;
use serde_json::Value;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// The most read from the start of a transcript.
pub(super) const HEAD_MAX: u64 = 512 * 1024;
/// The most read from its end.
pub(super) const TAIL_MAX: u64 = 16 * 1024 * 1024;
/// A line longer than this is skipped, and counted.
pub(super) const LINE_MAX: usize = 1024 * 1024;
/// The most edit paths kept.
const EDITS_MAX: usize = 100;
/// The most of a plan or a summary kept, in bytes.
pub(super) const PLAN_MAX: usize = 8 * 1024;
pub(super) const SUMMARY_MAX: usize = 16 * 1024;

/// One thing said, as the document quotes it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Said {
    pub role: String,
    pub text: String,
    pub final_answer: bool,
    /// When, as the line has it.
    pub at: Option<String>,
}

/// What was read of a transcript.
#[derive(Clone, Debug, Default)]
pub(super) struct Excerpted {
    /// The first thing a person said: the task as it began.
    pub first: Option<Said>,
    /// What was said in the window at the end, in order.
    pub said: Vec<Said>,
    /// The window did not reach back to the file's start: earlier turns are
    /// not in `said`.
    pub cut_front: bool,
    /// Lines skipped for their length.
    pub skipped: usize,
    /// The file changed while it was read.
    pub changed: bool,
    /// When the conversation began, as its first line has it.
    pub began: Option<String>,
    pub plan: Option<String>,
    pub summary: Option<String>,
    pub edits: Vec<String>,
    /// The skills the conversation loaded, the ask's turns left out.
    pub skills: Skills,
}

/// The skills a conversation loaded, as its transcript records them: Claude
/// Code's `Skill` tool calls, the blocks Codex puts a skill's text in
/// (`<skill>`, docs/cli-compatibility.md). Names only, as the CLI wrote them.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Skills {
    /// In order of first use, each once, at most SKILLS_MAX.
    pub names: Vec<String>,
    /// The transcript runs past what was read for them (SKILLS_READ_MAX).
    pub cut: bool,
}

/// The most of a transcript read, from its start, for the skills it loaded:
/// only lines that may name one are parsed, and a skill loaded past it is
/// said to be possible (`Skills::cut`).
pub(super) const SKILLS_READ_MAX: u64 = 256 * 1024 * 1024;
/// The most skill names kept, and the longest name.
const SKILLS_MAX: usize = 50;
const SKILL_NAME_MAX: usize = 128;

/// The turns a reading leaves out: those a prompt `opens` picks, and, for a
/// window that starts mid-turn, the move's last ask as (typed, closed) in ms.
#[derive(Clone, Copy)]
pub(super) struct Skip {
    pub opens: fn(&str) -> bool,
    pub ask: Option<(u64, Option<u64>)>,
}

impl Skip {
    /// Whether a window whose first line was written at `first` starts inside
    /// the ask's turn: written once the ask was typed and, where the ask has
    /// closed, no later than that. A line past the close may be a turn of the
    /// chat's own, whose prompt lies before the window too: it is read, as
    /// nothing of the chat's work is to go missing.
    fn starts_in_ask(&self, first: Option<u64>) -> bool {
        match (self.ask, first) {
            (Some((typed, closed)), Some(at)) => at >= typed && closed.is_none_or(|c| at <= c),
            _ => false,
        }
    }
}

/// A file's size and time: what tells it changed.
fn stamp(path: &Path) -> Option<(u64, Option<std::time::SystemTime>)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.len(), m.modified().ok()))
}

/// The complete lines of `bytes`, a chunk read from `start`: a first line cut
/// by the chunk's start left out unless it starts the file, the last left out
/// unless it ends it (`whole_end`). Over-long ones counted, not kept.
fn lines_of(bytes: &[u8], from_start: bool, whole_end: bool, skipped: &mut usize) -> Vec<String> {
    let mut parts: Vec<&[u8]> = bytes.split(|b| *b == b'\n').collect();
    if !from_start && !parts.is_empty() {
        parts.remove(0);
    }
    if !whole_end {
        parts.pop();
    }
    let mut out = Vec::with_capacity(parts.len());
    for p in parts {
        if p.len() > LINE_MAX {
            *skipped += 1;
            continue;
        }
        let line = String::from_utf8_lossy(p);
        let line = line.trim_end_matches('\r');
        if !line.trim().is_empty() {
            out.push(line.to_string());
        }
    }
    out
}

fn read_range(f: &mut std::fs::File, start: u64, len: u64) -> std::io::Result<Vec<u8>> {
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::with_capacity(len as usize);
    f.take(len).read_to_end(&mut buf)?;
    Ok(buf)
}

/// Reads the transcript at `path` of a `provider` chat as the module says,
/// the turns `skip` picks left out.
pub(super) fn read(path: &Path, provider: Provider, skip: Option<Skip>) -> Result<Excerpted, String> {
    read_bounded(path, provider, HEAD_MAX, TAIL_MAX, skip)
}

pub(super) fn read_bounded(path: &Path, provider: Provider, head_max: u64, tail_max: u64, skip: Option<Skip>) -> Result<Excerpted, String> {
    let before = stamp(path).ok_or_else(|| format!("{} cannot be read", path.display()))?;
    let len = before.0;
    let mut f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut skipped = 0;
    let (head, tail, cut_front) = if len <= tail_max {
        let all = read_range(&mut f, 0, len).map_err(|e| e.to_string())?;
        let lines = lines_of(&all, true, true, &mut skipped);
        (Vec::new(), lines, false)
    } else {
        let head = read_range(&mut f, 0, head_max.min(len)).map_err(|e| e.to_string())?;
        let head = lines_of(&head, true, false, &mut skipped);
        let start = len - tail_max;
        let tail = read_range(&mut f, start, tail_max).map_err(|e| e.to_string())?;
        let tail = lines_of(&tail, false, true, &mut skipped);
        (head, tail, true)
    };
    // The skills loaded before the window, read from the file's start; the
    // window's own come with its lines below, the ask's turns left out as the
    // rest of them are.
    let early = if cut_front { skills_within(&mut f, len - tail_max, provider).map_err(|e| e.to_string())? } else { Skills::default() };
    let after = stamp(path);
    let mut out = Excerpted { cut_front, skipped, changed: after != Some(before), skills: early, ..Default::default() };
    let said = |line: &str| match provider {
        Provider::Claude => crate::claude::session::said_line(line),
        Provider::Codex => crate::codex::session::said_line(line),
    };
    // The start: when it began, and the first thing a person said.
    for line in head.iter().chain(tail.iter().take(if cut_front { 0 } else { usize::MAX })) {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if out.began.is_none() {
            out.began = v["timestamp"].as_str().map(str::to_owned);
        }
        if let Some(m) = said(line).filter(|m| m.role == "user" && !skip.is_some_and(|s| (s.opens)(&m.text))) {
            out.first = Some(Said { role: m.role, text: m.text, final_answer: false, at: v["timestamp"].as_str().map(str::to_owned) });
            break;
        }
    }
    let mut edits: Vec<String> = Vec::new();
    // Inside a turn the skipped prompt opened: until the next prompt. A
    // window cut from the front may start inside the ask's turn, its prompt
    // before the window.
    let first_at = tail
        .iter()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find_map(|v| v["timestamp"].as_str().and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()))
        .map(|t| t.timestamp_millis().max(0) as u64);
    let mut skipping = cut_front && skip.is_some_and(|s| s.starts_in_ask(first_at));
    let mut command = None;
    for line in &tail {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let at = v["timestamp"].as_str().map(str::to_owned);
        let m = said(line);
        if let (Some(skip), Some(m)) = (skip, m.as_ref().filter(|m| m.role == "user")) {
            skipping = (skip.opens)(&m.text);
        }
        if skipping {
            continue;
        }
        match provider {
            Provider::Claude => claude_artifacts(&v, &mut out, &mut edits),
            Provider::Codex => codex_artifacts(&v, &mut out, &mut edits),
        }
        note_skills(&v, provider, &mut out.skills.names, &mut command);
        if let Some(m) = m {
            out.said.push(Said { role: m.role, text: m.text, final_answer: m.final_answer, at });
        }
    }
    out.edits = edits;
    Ok(out)
}

/// The skills the conversation in the transcript at `path` loaded, read from
/// its start (`skills_within`): what the next conversation of a handoff is
/// told it had (facts.rs `fact_block`).
pub(super) fn skills_in(path: &Path, provider: Provider) -> Result<Skills, String> {
    let mut f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let len = f.metadata().map_err(|e| format!("{}: {e}", path.display()))?.len();
    skills_within(&mut f, len, provider).map_err(|e| format!("{}: {e}", path.display()))
}

/// The skills named in the first `end` bytes of transcript `f`, read no
/// further than SKILLS_READ_MAX (`each_line`). Only a line that may name a
/// skill is parsed: a transcript is mostly lines that cannot.
fn skills_within(f: &mut std::fs::File, end: u64, provider: Provider) -> std::io::Result<Skills> {
    let mut out = Skills { cut: end > SKILLS_READ_MAX, ..Default::default() };
    let marks: &[&str] = match provider {
        Provider::Claude => &["\"Skill\"", "<command-name>", SKILL_TEXT],
        Provider::Codex => &["<skill"],
    };
    let mut command = None;
    each_line(f, end, |text| {
        if marks.iter().any(|m| text.contains(m)) {
            if let Ok(v) = serde_json::from_str::<Value>(text) {
                note_skills(&v, provider, &mut out.names, &mut command);
            }
        }
    })?;
    Ok(out)
}

/// Each line of the first `end` bytes of transcript `f`, read no further than
/// SKILLS_READ_MAX, a line at a time, each no longer than LINE_MAX (a longer
/// one passed over as it streams by, never held whole) and in UTF-8.
fn each_line(f: &mut std::fs::File, end: u64, mut each: impl FnMut(&str)) -> std::io::Result<()> {
    use std::io::BufRead;
    f.seek(SeekFrom::Start(0))?;
    let mut r = std::io::BufReader::with_capacity(64 * 1024, (&*f).take(end.min(SKILLS_READ_MAX)));
    let mut line: Vec<u8> = Vec::new();
    let mut over = false;
    let mut scan = |line: &[u8]| {
        if let Ok(text) = std::str::from_utf8(line) {
            each(text);
        }
    };
    loop {
        let buf = r.fill_buf()?;
        if buf.is_empty() {
            break;
        }
        let (part, ends) = match buf.iter().position(|b| *b == b'\n') {
            Some(i) => (&buf[..i], true),
            None => (buf, false),
        };
        if !over && line.len() + part.len() <= LINE_MAX {
            line.extend_from_slice(part);
        } else {
            over = true;
            line.clear();
        }
        let used = part.len() + usize::from(ends);
        r.consume(used);
        if ends {
            if !over {
                scan(&line);
            }
            line.clear();
            over = false;
        }
    }
    if !over && !line.is_empty() {
        scan(&line);
    }
    Ok(())
}

/// How Claude Code opens the text of a skill it loads (2.1.322, a skill's
/// `getPromptForCommand`), by the `Skill` tool or a person's `/name` alike.
const SKILL_TEXT: &str = "Base directory for this skill: ";

/// The skills transcript line `v` says were loaded, added to `names` as the
/// CLI wrote them, each once. Claude Code: an assistant line's `Skill` tool
/// call, `input.skill` its name; and a skill a person started by typing
/// `/name`, told by its two lines (`slash_skill`), `command` keeping the
/// first for the second; never a subagent's (`isSidechain`). Codex: a
/// skill's text as it puts it in a message, `<skill>` `<name>…</name>` …
/// `</skill>`, of either role: 0.167.0 recognizes it among the user's, and
/// 0.167.1's binary names it beside the developer's role
/// (docs/cli-compatibility.md).
fn note_skills(v: &Value, provider: Provider, names: &mut Vec<String>, command: &mut Option<(String, String)>) {
    let mut add = |name: &str| {
        let name: String = name.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
        let name = name.trim();
        if !name.is_empty() && names.len() < SKILLS_MAX && !names.iter().any(|n| n == name) {
            names.push(clip(name, SKILL_NAME_MAX).to_string());
        }
    };
    match provider {
        Provider::Claude => {
            if v["isSidechain"] == true {
                return;
            }
            if v["type"] == "user" {
                if let Some(name) = slash_skill(v, command) {
                    add(&name);
                }
                return;
            }
            if v["type"] != "assistant" {
                return;
            }
            let Some(blocks) = v["message"]["content"].as_array() else { return };
            for b in blocks.iter().filter(|b| b["type"] == "tool_use" && b["name"] == "Skill") {
                if let Some(name) = b["input"]["skill"].as_str() {
                    add(name);
                }
            }
        }
        Provider::Codex => {
            let p = &v["payload"];
            if v["type"] != "response_item" || p["type"] != "message" || !matches!(p["role"].as_str(), Some("user" | "developer")) {
                return;
            }
            let Some(blocks) = p["content"].as_array() else { return };
            for text in blocks.iter().filter_map(|b| b["text"].as_str()).map(str::trim) {
                let wrapped = text.strip_prefix("<skill").is_some_and(|r| r.starts_with(['>', ' '])) && text.ends_with("</skill>");
                let name = text.split_once("<name>").and_then(|(_, r)| r.split_once("</name>")).map(|(n, _)| n);
                if let Some(name) = name.filter(|_| wrapped) {
                    add(name);
                }
            }
        }
    }
}

/// A Claude Code user line's text: its content, or its text blocks joined.
fn user_text(v: &Value) -> String {
    let content = &v["message"]["content"];
    match content.as_str() {
        Some(s) => s.to_string(),
        None => content.as_array().into_iter().flatten().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"),
    }
}

/// A skill a person started by typing `/name` into Claude Code, as 2.1.322
/// writes one (read in its binary, `nt`): a line of the person's,
/// `<command-message>name</command-message>` then
/// `<command-name>/name</command-name>`, and after it the skill's text, a
/// line of the CLI's own (`isMeta`) opening with SKILL_TEXT whose
/// `parentUuid` is that line's uuid. The `Skill` tool's text names its call
/// (`sourceToolUseID`) instead and follows its result; a built-in such as
/// `/clear` or `/effort` writes its command line the other way round, its
/// name first, and is followed by its output (`<local-command-stdout>`),
/// never by a skill's text. A command line is kept in `command` until the
/// next one; the name, once its skill's text names it as the parent.
fn slash_skill(v: &Value, command: &mut Option<(String, String)>) -> Option<String> {
    let text = user_text(v);
    if v["isMeta"] != true {
        let named =
            text.trim_start().strip_prefix("<command-message>").and_then(|r| r.split_once("</command-message>")).and_then(|(_, r)| {
                let (name, _) = r.trim_start().strip_prefix("<command-name>/")?.split_once("</command-name>")?;
                Some(name.to_string())
            });
        if let (Some(name), Some(uuid)) = (named, v["uuid"].as_str()) {
            *command = Some((uuid.to_string(), name));
        }
        return None;
    }
    if !v["sourceToolUseID"].is_null() || !text.starts_with(SKILL_TEXT) {
        return None;
    }
    let parent = v["parentUuid"].as_str()?;
    command.take_if(|c| c.0 == parent).map(|(_, name)| name)
}

/// `text` cut to `max` bytes at a character's edge.
pub(super) fn clip(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn note_edit(edits: &mut Vec<String>, path: &str) {
    let path = path.trim();
    if !path.is_empty() && edits.len() < EDITS_MAX && !edits.iter().any(|e| e == path) {
        edits.push(path.to_string());
    }
}

/// Claude Code: the last `ExitPlanMode` plan, the last compaction summary,
/// and the paths its edit tools were called on.
fn claude_artifacts(v: &Value, out: &mut Excerpted, edits: &mut Vec<String>) {
    if v["isCompactSummary"] == true {
        if let Some(text) = crate::claude::session::content_text(&v["message"]["content"]) {
            out.summary = Some(clip(&text, SUMMARY_MAX).to_string());
        }
        return;
    }
    if v["type"] != "assistant" || v["isSidechain"] == true {
        return;
    }
    let Some(blocks) = v["message"]["content"].as_array() else { return };
    for b in blocks.iter().filter(|b| b["type"] == "tool_use") {
        match b["name"].as_str() {
            Some("ExitPlanMode") => {
                if let Some(plan) = b["input"]["plan"].as_str() {
                    out.plan = Some(clip(plan, PLAN_MAX).to_string());
                }
            }
            Some("Edit" | "Write" | "MultiEdit" | "NotebookEdit") => {
                if let Some(p) = b["input"]["file_path"].as_str().or_else(|| b["input"]["notebook_path"].as_str()) {
                    note_edit(edits, p);
                }
            }
            _ => {}
        }
    }
}

/// Codex: the last `update_plan`, and the files `apply_patch` names.
fn codex_artifacts(v: &Value, out: &mut Excerpted, edits: &mut Vec<String>) {
    if v["type"] != "response_item" {
        return;
    }
    let p = &v["payload"];
    let name = p["name"].as_str();
    // A function call's arguments are a JSON string; a custom tool's input
    // is the patch as text.
    let input = p["arguments"].as_str().or_else(|| p["input"].as_str()).unwrap_or("");
    match (p["type"].as_str(), name) {
        (Some("function_call"), Some("update_plan")) => {
            let Ok(args) = serde_json::from_str::<Value>(input) else { return };
            let Some(steps) = args["plan"].as_array() else { return };
            let mut text = String::new();
            if let Some(why) = args["explanation"].as_str().filter(|s| !s.trim().is_empty()) {
                text.push_str(why.trim());
                text.push('\n');
            }
            for s in steps {
                let (Some(step), status) = (s["step"].as_str(), s["status"].as_str().unwrap_or("?")) else { continue };
                text.push_str(&format!("- [{status}] {step}\n"));
            }
            if !text.is_empty() {
                out.plan = Some(clip(&text, PLAN_MAX).to_string());
            }
        }
        (Some("function_call" | "custom_tool_call"), Some("apply_patch")) => {
            // The patch's own text, or a JSON `input` field holding it.
            let patch = serde_json::from_str::<Value>(input).ok().and_then(|a| a["input"].as_str().map(str::to_owned));
            for line in patch.as_deref().unwrap_or(input).lines() {
                for head in ["*** Update File: ", "*** Add File: ", "*** Delete File: ", "*** Move to: "] {
                    if let Some(path) = line.strip_prefix(head) {
                        note_edit(edits, path);
                    }
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    pub(in crate::agents::handoff) fn claude_line(role: &str, text: &str, at: &str, end_turn: bool) -> String {
        if role == "user" {
            serde_json::json!({ "type": "user", "timestamp": at, "message": { "role": "user", "content": text } }).to_string()
        } else {
            let stop = if end_turn { "end_turn" } else { "tool_use" };
            serde_json::json!({ "type": "assistant", "timestamp": at,
                "message": { "model": "m", "stop_reason": stop, "content": [{ "type": "text", "text": text }] } })
            .to_string()
        }
    }

    fn file(tag: &str, text: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("luna-excerpts-{tag}-{}.jsonl", std::process::id()));
        std::fs::write(&p, text).unwrap();
        p
    }

    #[test]
    fn a_claude_transcript_gives_what_was_said_and_what_it_did_but_no_tool_output() {
        let lines = [
            claude_line("user", "Fix the build.", "2026-11-10T10:00:00Z", false),
            serde_json::json!({ "type": "assistant", "timestamp": "2026-11-10T10:00:05Z", "message": { "model": "m", "stop_reason": "tool_use",
                "content": [{ "type": "tool_use", "name": "Edit", "input": { "file_path": "src/a.rs", "old_string": "x" } },
                            { "type": "tool_use", "name": "ExitPlanMode", "input": { "plan": "1. Fix a.rs\n2. Test" } }] } })
            .to_string(),
            serde_json::json!({ "type": "user", "timestamp": "2026-11-10T10:00:06Z", "message": { "role": "user",
                "content": [{ "type": "tool_result", "content": "SECRET TOOL OUTPUT" }] } })
            .to_string(),
            serde_json::json!({ "type": "user", "isCompactSummary": true, "message": { "role": "user", "content": "Summary: we fixed a.rs" } })
                .to_string(),
            claude_line("assistant", "Done: the build is green.", "2026-11-10T10:01:00Z", true),
        ];
        let p = file("claude", &(lines.join("\n") + "\n"));
        let r = read(&p, Provider::Claude, None).unwrap();
        assert_eq!(r.first.as_ref().map(|s| s.text.as_str()), Some("Fix the build."));
        assert_eq!(r.began.as_deref(), Some("2026-11-10T10:00:00Z"));
        let said: Vec<(&str, &str, bool)> = r.said.iter().map(|s| (s.role.as_str(), s.text.as_str(), s.final_answer)).collect();
        assert_eq!(said, [("user", "Fix the build.", false), ("assistant", "Done: the build is green.", true)]);
        assert!(!format!("{r:?}").contains("SECRET TOOL OUTPUT"));
        assert_eq!(r.plan.as_deref(), Some("1. Fix a.rs\n2. Test"));
        assert_eq!(r.summary.as_deref(), Some("Summary: we fixed a.rs"));
        assert_eq!(r.edits, ["src/a.rs"]);
        assert!(!r.cut_front && !r.changed && r.skipped == 0);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn a_codex_rollout_gives_its_plan_and_patched_files() {
        let msg = |role: &str, kind: &str, text: &str, phase: Option<&str>| {
            let mut p = serde_json::json!({ "type": "message", "role": role, "content": [{ "type": kind, "text": text }] });
            if let Some(ph) = phase {
                p["phase"] = serde_json::json!(ph);
            }
            serde_json::json!({ "timestamp": "2026-11-10T10:00:00Z", "type": "response_item", "payload": p }).to_string()
        };
        let plan = serde_json::json!({ "explanation": "Two steps", "plan": [{ "step": "Patch", "status": "completed" }, { "step": "Test", "status": "in_progress" }] });
        let lines = [
            msg("user", "input_text", "<environment_context>cwd</environment_context>", None),
            msg("user", "input_text", "Add a flag.", None),
            serde_json::json!({ "type": "response_item", "payload": { "type": "function_call", "name": "update_plan", "arguments": plan.to_string() } }).to_string(),
            serde_json::json!({ "type": "response_item", "payload": { "type": "custom_tool_call", "name": "apply_patch",
                "input": "*** Begin Patch\n*** Update File: src/main.rs\n@@\n-a\n+b\n*** Add File: docs/x.md\n*** End Patch" } })
            .to_string(),
            msg("assistant", "output_text", "Added.", Some("final_answer")),
        ];
        let p = file("codex", &lines.join("\n"));
        let r = read(&p, Provider::Codex, None).unwrap();
        assert_eq!(r.first.as_ref().map(|s| s.text.as_str()), Some("Add a flag."));
        assert_eq!(r.plan.as_deref(), Some("Two steps\n- [completed] Patch\n- [in_progress] Test\n"));
        assert_eq!(r.edits, ["src/main.rs", "docs/x.md"]);
        assert!(r.said.last().is_some_and(|s| s.final_answer && s.text == "Added."));
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn reading_is_bounded_at_both_ends_and_says_what_it_skipped() {
        let mut text = claude_line("user", "The first task.", "2026-11-01T00:00:00Z", false) + "\n";
        for i in 0..2000 {
            text.push_str(&claude_line("assistant", &format!("filler {i} {}", "x".repeat(200)), "2026-11-01T00:00:01Z", true));
            text.push('\n');
        }
        // One line longer than the cap.
        text.push_str(&claude_line("assistant", &"y".repeat(LINE_MAX + 10), "2026-11-01T00:00:02Z", true));
        text.push('\n');
        text.push_str(&claude_line("user", "The last ask.", "2026-11-01T00:00:03Z", false));
        text.push('\n');
        let p = file("bounded", &text);
        let r = read_bounded(&p, Provider::Claude, 4 * 1024, LINE_MAX as u64 + 64 * 1024, None).unwrap();
        assert!(r.cut_front, "the tail did not reach the start");
        assert_eq!(r.first.as_ref().map(|s| s.text.as_str()), Some("The first task."), "the head still gives the task");
        assert_eq!(r.skipped, 1, "the long line skipped, not kept");
        assert!(r.said.len() < 2000 && r.said.last().is_some_and(|s| s.text == "The last ask."));
        assert!(r.said.iter().all(|s| s.text.len() < 1024));
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn the_turn_an_ask_opened_is_left_out_to_the_next_prompt() {
        let ask = "[Luna move m1 request, not typed by the user] The person is moving this chat.";
        let lines = [
            claude_line("user", "Fix the build.", "2026-11-10T10:00:00Z", false),
            claude_line("assistant", "Fixed.", "2026-11-10T10:01:00Z", true),
            claude_line("user", ask, "2026-11-10T10:02:00Z", false),
            serde_json::json!({ "type": "assistant", "timestamp": "2026-11-10T10:02:05Z", "message": { "model": "m", "stop_reason": "tool_use",
                "content": [{ "type": "tool_use", "name": "Write", "input": { "file_path": "drafts/move-m1-ab.md" } }] } })
            .to_string(),
            claude_line("assistant", "The document is written.", "2026-11-10T10:03:00Z", true),
            claude_line("user", "Now the tests.", "2026-11-10T10:30:00Z", false),
            claude_line("assistant", "Green.", "2026-11-10T10:31:00Z", true),
        ];
        let p = file("skip", &(lines.join("\n") + "\n"));
        let r = read(&p, Provider::Claude, Some(Skip { opens: |t: &str| t.starts_with("[Luna move m1 request"), ask: None })).unwrap();
        let said: Vec<&str> = r.said.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(said, ["Fix the build.", "Fixed.", "Now the tests.", "Green."]);
        assert!(r.edits.is_empty(), "what the ask's turn wrote is not the work: {:?}", r.edits);
        let whole = read(&p, Provider::Claude, None).unwrap();
        assert!(whole.said.iter().any(|s| s.text.starts_with("[Luna move m1 request")));
        assert_eq!(whole.edits, ["drafts/move-m1-ab.md"]);
        let _ = std::fs::remove_file(p);
    }

    /// The ask's turn longer than the window: its prompt is before it, its
    /// narration and edits inside, and they are still left out.
    #[test]
    fn a_window_that_starts_inside_the_asks_turn_leaves_it_out() {
        let ask = "[Luna move m1 request, not typed by the user] The person is moving this chat.";
        let mut text = claude_line("user", "Fix the build.", "2026-11-10T10:00:00Z", false) + "\n";
        text.push_str(&claude_line("user", ask, "2026-11-10T10:02:00Z", false));
        text.push('\n');
        for i in 0..400 {
            text.push_str(&claude_line("assistant", &format!("writing {i} {}", "x".repeat(200)), "2026-11-10T10:02:30Z", false));
            text.push('\n');
        }
        text.push_str(
            &serde_json::json!({ "type": "assistant", "timestamp": "2026-11-10T10:03:00Z", "message": { "model": "m", "stop_reason": "tool_use",
                "content": [{ "type": "tool_use", "name": "Write", "input": { "file_path": "drafts/move-m1-ab.md" } }] } })
            .to_string(),
        );
        text.push('\n');
        let p = file("skip-mid", &text);
        let typed = chrono::DateTime::parse_from_rfc3339("2026-11-10T10:02:00Z").unwrap().timestamp_millis() as u64;
        let opens: fn(&str) -> bool = |t| t.starts_with("[Luna move m1 request");
        let read_with = |ask| read_bounded(&p, Provider::Claude, 4 * 1024, 32 * 1024, Some(Skip { opens, ask })).unwrap();
        let r = read_with(Some((typed, None)));
        assert!(r.cut_front && !r.said.iter().any(|s| s.text.contains(ask)), "the ask's prompt is before the window");
        assert!(r.said.is_empty() && r.edits.is_empty(), "the ask's turn is not the work: {:?} {:?}", r.said.len(), r.edits);
        // Closed before the window's first line: that may be a turn of the
        // chat's own, and is read.
        let r = read_with(Some((typed, Some(typed + 1_000))));
        assert!(!r.said.is_empty() && r.edits == ["drafts/move-m1-ab.md"]);
        // No ask on record: read as ever.
        assert_eq!(read_with(None).edits, ["drafts/move-m1-ab.md"]);
        let _ = std::fs::remove_file(p);
    }

    /// A Claude Code assistant line calling `Skill`, as 2.1.322 writes one.
    fn skill_call(names: &[&str], sidechain: bool) -> String {
        let blocks: Vec<Value> = names
            .iter()
            .enumerate()
            .map(|(i, n)| serde_json::json!({ "type": "tool_use", "id": format!("toolu_{i}"), "name": "Skill", "input": { "skill": n }, "caller": { "type": "direct" } }))
            .collect();
        serde_json::json!({ "type": "assistant", "isSidechain": sidechain, "timestamp": "2026-11-10T10:00:05Z",
            "message": { "model": "m", "stop_reason": "tool_use", "content": blocks } })
        .to_string()
    }

    /// A Codex message carrying a skill's text, as its `<skill>` block is
    /// spelled (0.167.1 `ext/skills/src/fragments.rs`).
    fn skill_block(role: &str, name: &str) -> String {
        let text = format!("<skill>\n<name>{name}</name>\n<path>C:\\acc\\skills\\{name}\\SKILL.md</path>\n# {name}\nsteps\n</skill>");
        serde_json::json!({ "timestamp": "2026-11-10T10:00:00Z", "type": "response_item",
            "payload": { "type": "message", "role": role, "content": [{ "type": "input_text", "text": text }] } })
        .to_string()
    }

    #[test]
    fn the_skills_a_claude_code_conversation_loaded_come_from_its_skill_calls() {
        let lines = [
            claude_line("user", "Release it, and mind the Skill tool.", "2026-11-10T10:00:00Z", false),
            skill_call(&["release"], false),
            // The same call as Claude Code also logs it: no assistant line.
            serde_json::json!({ "type": "api-request-blob", "message": { "content": [
                { "type": "tool_use", "name": "Skill", "input": { "skill": "blob-only" } }] } })
            .to_string(),
            skill_call(&["ui-check", "release"], false),
            // A subagent's own, not the conversation's.
            skill_call(&["sidechain-only"], true),
            serde_json::json!({ "type": "assistant", "message": { "content": [
                { "type": "tool_use", "name": "Read", "input": { "skill": "not-a-skill-call" } }] } })
            .to_string(),
            claude_line("assistant", "Released.", "2026-11-10T10:01:00Z", true),
        ];
        let p = file("skills-claude", &(lines.join("\n") + "\n"));
        let s = skills_in(&p, Provider::Claude).unwrap();
        assert_eq!(s, Skills { names: vec!["release".into(), "ui-check".into()], cut: false }, "in order of first use, each once");
        assert_eq!(read(&p, Provider::Claude, None).unwrap().skills, s, "the move's reading finds the same");
        assert!(skills_in(&p.with_extension("gone"), Provider::Claude).is_err());
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn the_skills_a_codex_conversation_loaded_come_from_its_skill_blocks() {
        let msg = |role: &str, text: &str| {
            serde_json::json!({ "type": "response_item", "payload": { "type": "message", "role": role,
                "content": [{ "type": "input_text", "text": text }] } })
            .to_string()
        };
        let lines = [
            // The list of skills it may load is no skill loaded.
            msg("developer", "<skills_instructions>\n## Skills\n- review: (file: C:\\s\\SKILL.md)\n</skills_instructions>"),
            msg("user", "Use $review, then <skill>s are done."),
            skill_block("developer", "review"),
            skill_block("user", "release"),
            skill_block("developer", "review"),
            msg("assistant", "<skill>\n<name>said-not-loaded</name>\n</skill>"),
            // What a tool printed is no message.
            serde_json::json!({ "type": "response_item", "payload": { "type": "function_call_output",
                "output": "<skill>\n<name>printed</name>\n</skill>" } })
            .to_string(),
        ];
        let p = file("skills-codex", &(lines.join("\n") + "\n"));
        let s = skills_in(&p, Provider::Codex).unwrap();
        assert_eq!(s.names, ["review", "release"]);
        assert_eq!(read(&p, Provider::Codex, None).unwrap().skills.names, ["review", "release"]);
        let _ = std::fs::remove_file(p);
    }

    /// A Claude Code user line as 2.1.322 writes one: `meta` the CLI's own,
    /// `parent` the line before's uuid, `tool` the call a skill's text answers.
    fn user(uuid: &str, parent: &str, meta: bool, tool: Option<&str>, content: Value) -> String {
        let mut v = serde_json::json!({ "type": "user", "uuid": uuid, "parentUuid": parent, "isSidechain": false,
            "message": { "role": "user", "content": content } });
        if meta {
            v["isMeta"] = serde_json::json!(true);
        }
        if let Some(t) = tool {
            v["sourceToolUseID"] = serde_json::json!(t);
        }
        v.to_string()
    }

    fn skill_text(dir: &str) -> Value {
        serde_json::json!([{ "type": "text", "text": format!("Base directory for this skill: {dir}\n\n# Steps") }])
    }

    #[test]
    fn a_skill_a_person_started_with_a_slash_is_one_its_conversation_loaded() {
        let command = |name: &str, args: &str| {
            Value::String(format!(
                "<command-message>{name}</command-message>\n<command-name>/{name}</command-name>\n<command-args>{args}</command-args>"
            ))
        };
        let lines = [
            // Built-ins, as 2.1.322 wrote them in this machine's transcripts:
            // the name first, then their output, never a skill's text.
            user("u1", "u0", false, None, Value::String("<command-name>/effort</command-name>\n            <command-message>effort</command-message>\n            <command-args>high</command-args>".into())),
            user("u2", "u1", false, None, Value::String("<local-command-stdout>Set effort level to high</local-command-stdout>".into())),
            // A skill the person started: its line, then its text naming it as the parent.
            user("u3", "u2", false, None, command("release", "0.18.0")),
            user("u4", "u3", true, None, skill_text("E:\\p\\.claude\\skills\\release")),
            // A command of the person's whose text is no skill's.
            user("u5", "u4", false, None, command("deploy", "")),
            user("u6", "u5", true, None, serde_json::json!([{ "type": "text", "text": "Deploy it." }])),
            // A skill's text that is no child of a command line, and one the
            // Skill tool's call brought (counted by that call, not here).
            user("u7", "u6", true, None, skill_text("E:\\p\\.claude\\skills\\stray")),
            user("u8", "u7", true, Some("toolu_1"), skill_text("E:\\p\\.claude\\skills\\by-tool")),
            // A person's prompt that only quotes the shape.
            user("u9", "u8", false, None, Value::String("<command-message>quoted</command-message> is how it looks".into())),
            user("u10", "u9", true, None, skill_text("E:\\p\\.claude\\skills\\quoted")),
            command_then_skill_by_subagent(),
        ];
        let p = file("skills-slash", &(lines.join("\n") + "\n"));
        assert_eq!(skills_in(&p, Provider::Claude).unwrap().names, ["release"]);
        assert_eq!(read(&p, Provider::Claude, None).unwrap().skills.names, ["release"], "the move's reading finds the same");
        let _ = std::fs::remove_file(p);
    }

    /// A subagent's own `/name` and skill text: not the conversation's.
    fn command_then_skill_by_subagent() -> String {
        let mut a: Value = serde_json::from_str(&user(
            "s1",
            "u10",
            false,
            None,
            Value::String("<command-message>sub</command-message>\n<command-name>/sub</command-name>".into()),
        ))
        .unwrap();
        a["isSidechain"] = serde_json::json!(true);
        let mut b: Value = serde_json::from_str(&user("s2", "s1", true, None, skill_text("E:\\p\\sub"))).unwrap();
        b["isSidechain"] = serde_json::json!(true);
        format!("{a}\n{b}")
    }

    /// Read a line at a time, an over-long one passed over; before the move's
    /// window from the file's start, within it as the rest of it is read, the
    /// ask's turn left out.
    #[test]
    fn the_skills_are_read_bounded_and_the_asks_turn_is_left_out() {
        let ask = "[Luna move m1 request, not typed by the user] The person is moving this chat.";
        let mut text = claude_line("user", "Fix the build.", "2026-11-10T10:00:00Z", false) + "\n";
        text.push_str(&(skill_call(&["early"], false) + "\n"));
        // Longer than a line may be: its skill is not read.
        let zs = "z".repeat(LINE_MAX);
        text.push_str(&(skill_call(&[zs.as_str()], false) + "\n"));
        for i in 0..400 {
            text.push_str(&(claude_line("assistant", &format!("work {i} {}", "x".repeat(200)), "2026-11-10T10:01:00Z", true) + "\n"));
        }
        text.push_str(&(skill_call(&["late"], false) + "\n"));
        text.push_str(&(claude_line("user", ask, "2026-11-10T10:02:00Z", false) + "\n"));
        text.push_str(&(skill_call(&["for-the-ask"], false) + "\n"));
        let p = file("skills-bounded", &text);
        assert_eq!(skills_in(&p, Provider::Claude).unwrap().names, ["early", "late", "for-the-ask"]);
        let opens: fn(&str) -> bool = |t| t.starts_with("[Luna move m1 request");
        let r = read_bounded(&p, Provider::Claude, 4 * 1024, 32 * 1024, Some(Skip { opens, ask: None })).unwrap();
        assert!(r.cut_front);
        assert_eq!(r.skills.names, ["early", "late"], "before the window and in it, the ask's turn left out");
        // A transcript past what is read for them says so.
        let mut f = std::fs::File::open(&p).unwrap();
        let past = skills_within(&mut f, SKILLS_READ_MAX + 1, Provider::Claude).unwrap();
        assert!(past.cut && past.names.len() == 3);
        let _ = std::fs::remove_file(p);
    }
}
