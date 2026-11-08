//! Helpers the tests of several agents modules share.

use crate::agents::notifier::lines::notice_line;
use crate::agents::notifier::Settled;
use crate::agents::owed::Owed;
use crate::agents::persist::{take_up_chats, SavedChat, SavedChats, SavedSend, Ties};
use crate::agents::read::transcripts;
use crate::agents::registry::{reg, register, Known};
use crate::agents::spawn::SpawnTaken;
use crate::agents::wait::REPLY_GRACE_MS;
use crate::provider::Provider;
use crate::throttle::now_ms;
use std::sync::Mutex;

/// A helper announced as not done, its job over, with a transcript on disk
/// holding what `said` past `cursor`, the debt `caller` is owed.
pub(super) fn job_over_helper(id: &str, caller: &str, said: &str) -> (Known, Owed, std::path::PathBuf) {
    let none = serde_json::json!({});
    register(id, Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, false, none);
    crate::background::idle_for_test(id, Provider::Claude, 3, false);
    let path = std::env::temp_dir().join(format!("luna-{id}-{}.jsonl", std::process::id()));
    let before = "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"I am starting the job.\"}]}}\n";
    std::fs::write(&path, format!("{before}{said}")).unwrap();
    transcripts().lock().unwrap().insert(id.into(), path.clone());
    let o = Owed::pending(3, before.len() as u64);
    reg().lock().unwrap().owed.insert((caller.to_string(), id.to_string()), o);
    (reg().lock().unwrap().known.get(id).cloned().unwrap(), o, path)
}

/// The notice as the notifier would make it, past the grace for a reply
/// that is not coming.
pub(super) fn notice_for(caller: &str, id: &str, turns_ended: u32) -> (Option<String>, Settled) {
    let (k, mut o) = {
        let r = reg().lock().unwrap();
        (r.known.get(id).cloned().unwrap(), r.owed.get(&(caller.to_string(), id.to_string())).copied().unwrap())
    };
    o.held = Some((turns_ended, now_ms() - REPLY_GRACE_MS));
    notice_line(crate::pty::manager(), caller, "Your helper", id, &k, o).unwrap()
}

/// A chat of the person's as the last core left it: `caller` wrote to it and is
/// owed its answer from count `baseline` on, its transcript read from `cursor`,
/// and it had ended `baseline` turns.
pub(super) fn owed_chat(id: &str, caller: &str, baseline: u32, cursor: u64) -> SavedChat {
    SavedChat {
        id: id.into(),
        provider: Provider::Claude,
        folder: "C:\\p".into(),
        account_path: "C:\\acc\\anthropic\\one".into(),
        turns_ended: baseline,
        transcript: None,
        mid_turn: false,
        ties: Ties { owed: vec![(caller.into(), baseline, cursor)], seen: vec![(caller.into(), baseline)], ..Default::default() },
    }
}

pub(super) fn take_up(chats: Vec<SavedChat>, sends: Vec<SavedSend>, spawns: Vec<SpawnTaken>) {
    let saved = SavedChats { chats, sends, spawns, news: Vec::new() };
    take_up_chats(serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap());
}

/// `settle_spawns` and its like go over every spawn there is, a test's or
/// another's, as they do in a core: one test at a time.
pub(super) fn spawns_to_myself() -> std::sync::MutexGuard<'static, ()> {
    static ONE: Mutex<()> = Mutex::new(());
    ONE.lock().unwrap_or_else(|e| e.into_inner())
}

pub(super) fn spawn_taken(request: u64, caller: &str, helper: Option<&str>, resume: bool) -> SpawnTaken {
    SpawnTaken {
        request,
        caller: caller.into(),
        params: serde_json::from_value(serde_json::json!({ "prompt": "the whole job, word for word", "name": "the job" })).unwrap(),
        auto_delete: None,
        resume_on_restart: resume,
        budget_tokens: None,
        connectors: false,
        in_worktree: None,
        account_path: "C:\\acc\\anthropic\\one".into(),
        worktrees_before: Vec::new(),
        helper: helper.map(str::to_owned),
        owed_placed: false,
        left_by_core: false,
        settled: false,
        news: None,
    }
}

pub(super) fn tooled(id: &str) {
    register(id, Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, serde_json::json!({}));
}
