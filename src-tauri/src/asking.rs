//! A chat whose turn ended on a question to its user: shown as waiting, like a
//! permission prompt, though nothing holds the turn. Two ways to tell:
//!
//! - the session said so with `luna_waiting` (mcp.rs), which stands until
//!   the next prompt a person gives it — a Luna notice typed in is none;
//! - its last message ends on the marker the user set (`waiting_marker` in
//!   settings), a line their own reply format already writes.
//!
//! Only for the user's own chats: a helper asks its parent, who has its own
//! ways of hearing it (agents.rs). And only for the eye: for power.rs the turn
//! is over, and an armed shutdown goes ahead — a question nobody is there to
//! answer must not keep the PC on all night.

use crate::throttle::now_ms;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// The longest question kept: what a toast and a tooltip can show.
const QUESTION_MAX: usize = 500;

struct Declared {
    question: String,
    at_ms: u64,
}

fn declared() -> &'static Mutex<HashMap<String, Declared>> {
    static D: OnceLock<Mutex<HashMap<String, Declared>>> = OnceLock::new();
    D.get_or_init(Default::default)
}

/// `luna_waiting`: the caller ends its turn on this question (None takes it
/// back). Refused for a helper, whose questions go to its parent.
pub fn declare(chat_id: &str, question: Option<&str>) -> Result<serde_json::Value, String> {
    if crate::agents::parent_of(chat_id).is_some() {
        return Err("only the user's own chats wait on them: your parent gets your reply as it is".into());
    }
    let mut d = declared().lock().unwrap_or_else(|e| e.into_inner());
    match question.map(str::trim).filter(|q| !q.is_empty()) {
        Some(q) => {
            let question: String = q.chars().take(QUESTION_MAX).collect();
            d.insert(chat_id.to_string(), Declared { question, at_ms: now_ms() });
            Ok(serde_json::json!({ "waiting": true }))
        }
        None => {
            d.remove(chat_id);
            Ok(serde_json::json!({ "waiting": false }))
        }
    }
}

/// A prompt reached the chat at `at_ms`: a question put before it has had its
/// answer, or been passed over, either way the user has spoken.
pub fn prompted_at(chat_id: &str, at_ms: u64) {
    let mut d = declared().lock().unwrap_or_else(|e| e.into_inner());
    if d.get(chat_id).is_some_and(|q| q.at_ms <= at_ms) {
        d.remove(chat_id);
    }
}

/// Text Luna typed in on its own, not the user's answer.
pub fn is_notice(text: &str) -> bool {
    text.trim_start().starts_with(crate::agents::NOTICE_TAG)
}

/// A Claude Code hook (hub.rs): `UserPromptSubmit` is the next prompt. A body
/// cut short by the hub's limit is a long prompt, and Luna's notices are short.
pub fn on_hook(chat_id: &str, body: &str) {
    match serde_json::from_str::<serde_json::Value>(body) {
        Ok(v) if v["hook_event_name"] == "UserPromptSubmit" => {
            if !is_notice(v["prompt"].as_str().unwrap_or("")) {
                prompted_at(chat_id, now_ms());
            }
        }
        Ok(_) => {}
        Err(_) if body.contains("\"UserPromptSubmit\"") => prompted_at(chat_id, now_ms()),
        Err(_) => {}
    }
}

/// The chat is gone.
pub fn forget(chat_id: &str) {
    declared().lock().unwrap_or_else(|e| e.into_inner()).remove(chat_id);
}

fn question_of(chat_id: &str) -> Option<String> {
    declared().lock().unwrap_or_else(|e| e.into_inner()).get(chat_id).map(|d| d.question.clone())
}

/// Whether a message's last line holds the marker, when one is set.
fn marked_by(said: &str, marker: &str) -> bool {
    let marker = marker.trim();
    !marker.is_empty() && said.lines().rev().map(str::trim).find(|l| !l.is_empty()).is_some_and(|l| l.contains(marker))
}

/// A live chat's meta, as `session_meta` hands it on: a turn over and ended on
/// a question reads `waiting`, with the question where one was given.
pub fn overlay(chat_id: &str, meta: &mut crate::pty::SessionMeta) {
    let idle = meta.status.as_deref().map_or(true, |s| crate::activity::turn_of(s) == crate::activity::Turn::Idle);
    let busy = crate::activity::turn_state(chat_id).is_some_and(|t| t.turn != crate::activity::Turn::Idle);
    if !idle || busy || crate::agents::parent_of(chat_id).is_some() {
        return;
    }
    if let Some(q) = question_of(chat_id) {
        meta.status = Some("waiting".into());
        meta.question = Some(q);
        return;
    }
    let s = crate::settings::get();
    if s.waiting_marker_on && meta.last_said.as_deref().is_some_and(|said| marked_by(said, &s.waiting_marker)) {
        meta.status = Some("waiting".into());
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Marker {
    pub on: bool,
    pub text: String,
}

#[tauri::command]
pub fn waiting_marker() -> Marker {
    let s = crate::settings::get();
    Marker { on: s.waiting_marker_on, text: s.waiting_marker }
}

#[tauri::command]
pub fn set_waiting_marker(on: bool, text: String) -> Result<Marker, String> {
    let text = text.trim().to_string();
    crate::settings::update(|s| {
        s.waiting_marker_on = on;
        s.waiting_marker = text;
    })?;
    Ok(waiting_marker())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_question_stands_until_the_user_speaks() {
        let id = "ask-stands";
        declared().lock().unwrap().insert(id.into(), Declared { question: "Which one?".into(), at_ms: 1_000 });
        // The prompt that started the turn came before the question.
        prompted_at(id, 900);
        assert_eq!(question_of(id).as_deref(), Some("Which one?"));
        // A notice typed in is no answer.
        on_hook(id, &serde_json::json!({ "hook_event_name": "UserPromptSubmit", "prompt": format!("{} Your helper finished.", crate::agents::NOTICE_TAG) }).to_string());
        assert!(question_of(id).is_some());
        // Nor is the turn's own tool coming back.
        on_hook(id, r#"{"hook_event_name":"PostToolUse","tool_name":"mcp__luna__luna_waiting"}"#);
        assert!(question_of(id).is_some());
        on_hook(id, r#"{"hook_event_name":"UserPromptSubmit","prompt":"the second"}"#);
        assert!(question_of(id).is_none());
    }

    #[test]
    fn a_prompt_too_long_for_the_hub_still_answers() {
        let id = "ask-long";
        declared().lock().unwrap().insert(id.into(), Declared { question: "Go?".into(), at_ms: 0 });
        on_hook(id, r#"{"hook_event_name":"UserPromptSubmit","prompt":"a very long paste cut sh"#);
        assert!(question_of(id).is_none());
    }

    #[test]
    fn the_marker_is_looked_for_on_the_last_line() {
        let report = "`QUESTIONS`\n1. Ship it?\n\n`STATUS: AWAITING OPERATOR · QUESTIONS: 1`\n";
        assert!(marked_by(report, "AWAITING OPERATOR"));
        // Quoted along the way, it says nothing of how the message ends.
        assert!(!marked_by("The marker `STATUS: AWAITING OPERATOR` would do.\n\n`STATUS: WORKING`", "AWAITING OPERATOR"));
        assert!(!marked_by(report, "  "), "no marker set");
        assert!(!marked_by("", "AWAITING OPERATOR"));
    }
}
