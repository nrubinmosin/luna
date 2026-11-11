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

/// Reads the transcript at `path` of a `provider` chat as the module says.
pub(super) fn read(path: &Path, provider: Provider) -> Result<Excerpted, String> {
    read_bounded(path, provider, HEAD_MAX, TAIL_MAX)
}

pub(super) fn read_bounded(path: &Path, provider: Provider, head_max: u64, tail_max: u64) -> Result<Excerpted, String> {
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
    let after = stamp(path);
    let mut out = Excerpted { cut_front, skipped, changed: after != Some(before), ..Default::default() };
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
        if let Some(m) = said(line).filter(|m| m.role == "user") {
            out.first = Some(Said { role: m.role, text: m.text, final_answer: false, at: v["timestamp"].as_str().map(str::to_owned) });
            break;
        }
    }
    let mut edits: Vec<String> = Vec::new();
    for line in &tail {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let at = v["timestamp"].as_str().map(str::to_owned);
        match provider {
            Provider::Claude => claude_artifacts(&v, &mut out, &mut edits),
            Provider::Codex => codex_artifacts(&v, &mut out, &mut edits),
        }
        if let Some(m) = said(line) {
            out.said.push(Said { role: m.role, text: m.text, final_answer: m.final_answer, at });
        }
    }
    out.edits = edits;
    Ok(out)
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
        let r = read(&p, Provider::Claude).unwrap();
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
        let r = read(&p, Provider::Codex).unwrap();
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
        let r = read_bounded(&p, Provider::Claude, 4 * 1024, LINE_MAX as u64 + 64 * 1024).unwrap();
        assert!(r.cut_front, "the tail did not reach the start");
        assert_eq!(r.first.as_ref().map(|s| s.text.as_str()), Some("The first task."), "the head still gives the task");
        assert_eq!(r.skipped, 1, "the long line skipped, not kept");
        assert!(r.said.len() < 2000 && r.said.last().is_some_and(|s| s.text == "The last ask."));
        assert!(r.said.iter().all(|s| s.text.len() < 1024));
        let _ = std::fs::remove_file(p);
    }
}
