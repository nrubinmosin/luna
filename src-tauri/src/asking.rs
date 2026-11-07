//! A chat whose turn ended on a question to its user: shown as waiting, like a
//! permission prompt, though nothing holds the turn. Its last message ends on
//! the marker the user set (`waiting_marker` in settings), a line their own
//! reply format already writes.
//!
//! The user may clear it by hand (`dismiss_waiting`): it stays cleared for the
//! words of that turn, and the next turn that ends on the marker shows again.
//!
//! Only for the user's own chats: a helper asks its parent, who has its own
//! ways of hearing it (agents.rs). And only for the eye: for power.rs the turn
//! is over, and an armed shutdown goes ahead — a question nobody is there to
//! answer must not keep the PC on all night.

use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// The last waiting shown per chat, by the ending it was shown on (`ending`),
/// and whether the user cleared it.
struct Shown {
    on: u64,
    cleared: bool,
}

fn shown() -> &'static Mutex<HashMap<String, Shown>> {
    static S: OnceLock<Mutex<HashMap<String, Shown>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// What a waiting is shown on: the words a turn ended on and when they were
/// said, both from one read of the transcript or rollout. Either moving is a
/// new question; the time alone tells the same words said again a turn later.
fn ending(said: &str, at: Option<&str>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (said, at).hash(&mut h);
    h.finish()
}

/// Whether a session is the user's own chat rather than a helper. The dev
/// caller's spawns come up as top-level chats (bridge.ts), with nobody above
/// them to ask.
fn for_user(chat_id: &str) -> bool {
    crate::agents::parent_of(chat_id).is_none_or(|p| p == crate::agents::DEV_CALLER)
}

/// The chat is gone.
pub fn forget(chat_id: &str) {
    shown().lock().unwrap_or_else(|e| e.into_inner()).remove(chat_id);
}

/// Whether a message's last line holds the marker, when one is set.
fn marked_by(said: &str, marker: &str) -> bool {
    let marker = marker.trim();
    !marker.is_empty() && said.lines().rev().map(str::trim).find(|l| !l.is_empty()).is_some_and(|l| l.contains(marker))
}

/// A live chat's meta, as `session_meta` hands it on: a turn over that ended
/// on the marker reads `waiting`, unless the user cleared it.
pub fn overlay(chat_id: &str, meta: &mut crate::pty::SessionMeta) {
    let s = crate::settings::get();
    if s.waiting_marker_on {
        overlay_marked(chat_id, meta, &s.waiting_marker);
    }
}

/// `overlay` with the marker on and set to `marker`.
fn overlay_marked(chat_id: &str, meta: &mut crate::pty::SessionMeta, marker: &str) {
    let idle = meta.status.as_deref().is_none_or(|s| crate::activity::turn_of(s) == crate::activity::Turn::Idle);
    let busy = crate::activity::turn_state(chat_id).is_some_and(|t| t.turn != crate::activity::Turn::Idle);
    if !idle || busy || !for_user(chat_id) {
        return;
    }
    let Some(said) = meta.last_said.as_deref().filter(|said| marked_by(said, marker)) else { return };
    let on = ending(said, meta.last_said_at.as_deref());
    let mut shown = shown().lock().unwrap_or_else(|e| e.into_inner());
    if shown.get(chat_id).is_some_and(|s| s.cleared && s.on == on) {
        return;
    }
    if shown.get(chat_id).is_none_or(|s| s.on != on) {
        shown.insert(chat_id.to_string(), Shown { on, cleared: false });
    }
    meta.status = Some("waiting".into());
    meta.marked = Some(format!("{on:x}"));
}

/// The user says the chat is not waiting on them: cleared until a turn ends
/// on the marker again. Only the ending they saw (`key`, as `marked` gave it):
/// one a later look found, still on its way to the window, stands.
#[tauri::command]
pub fn dismiss_waiting(id: String, key: String) {
    if let Some(s) = shown().lock().unwrap_or_else(|e| e.into_inner()).get_mut(&id).filter(|s| format!("{:x}", s.on) == key) {
        s.cleared = true;
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

    const MARKER: &str = "AWAITING OPERATOR";

    /// The key a chat whose turn is over and ended on `said` at `at` reads
    /// waiting by, if it does.
    fn waits_at(chat_id: &str, said: &str, at: &str) -> Option<String> {
        let mut meta = crate::pty::SessionMeta {
            status: Some("idle".into()),
            last_said: Some(said.into()),
            last_said_at: Some(at.into()),
            ..Default::default()
        };
        overlay_marked(chat_id, &mut meta, MARKER);
        assert_eq!(meta.marked.is_some(), meta.status.as_deref() == Some("waiting"));
        meta.marked
    }

    fn waits(chat_id: &str, said: &str) -> bool {
        waits_at(chat_id, said, "t1").is_some()
    }

    #[test]
    fn a_cleared_waiting_stays_cleared_until_a_turn_ends_on_the_marker_again() {
        let id = "ask-cleared";
        let ship = "Ship it?\n\n`STATUS: AWAITING OPERATOR`";
        let which = "Which one?\n\n`STATUS: AWAITING OPERATOR`";
        let key = waits_at(id, ship, "t1").unwrap();
        dismiss_waiting(id.into(), key);
        assert!(waits_at(id, ship, "t1").is_none(), "the same words said at the same time");
        assert!(waits_at(id, ship, "t1").is_none(), "and on every look after");
        // The next turn ends on other words.
        let key = waits_at(id, which, "t2").unwrap();
        dismiss_waiting(id.into(), key.clone());
        assert!(waits_at(id, which, "t2").is_none());
        // Or on the same words, a turn later.
        assert!(waits_at(id, which, "t3").is_some());
        // A clear for what was shown before a later look found more clears
        // nothing of the newer one.
        dismiss_waiting(id.into(), key);
        assert!(waits_at(id, which, "t3").is_some());
        // Gone with the chat: nothing cleared stays behind.
        let key = waits_at(id, which, "t3").unwrap();
        dismiss_waiting(id.into(), key);
        forget(id);
        assert!(waits_at(id, which, "t3").is_some());
        forget(id);
    }

    #[test]
    fn only_the_users_own_chats_wait_on_them() {
        let (parent, helper) = ("ask-parent", "ask-helper");
        crate::agents::register(
            helper,
            crate::provider::Provider::Claude,
            "C:/p",
            "C:/acc/anthropic/one",
            Some(parent),
            false,
            serde_json::json!({}),
        );
        assert!(!waits(helper, "Go?\nAWAITING OPERATOR"), "a helper asks its parent");
        // Nothing was shown, so there is nothing to clear.
        dismiss_waiting(helper.into(), String::new());
        assert!(!shown().lock().unwrap().contains_key(helper));
        assert!(waits(parent, "Go?\nAWAITING OPERATOR"));
        crate::agents::forget(helper);
        forget(parent);
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
