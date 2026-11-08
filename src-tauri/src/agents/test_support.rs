//! Helpers the tests of several agents modules share.

use crate::agents::notifier::lines::notice_line;
use crate::agents::notifier::Settled;
use crate::agents::owed::Owed;
use crate::agents::persist::{take_up_chats, SavedChat, SavedChats, SavedSend, Ties};
use crate::agents::read::transcripts;
use crate::agents::registry::{reg, register, Known};
use crate::agents::spawn::{chat_starting, SpawnTaken};
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

/// A chat as the last core left it: spawned by `parent`, its turn cut short
/// with `flag` or not, and owed to `parent` from count 3 on.
pub(super) fn cut_short_chat(id: &str, parent: &str, flag: bool, mid_turn: bool, already: bool) -> SavedChat {
    let settings = if flag { serde_json::json!({ "resumeOnRestart": true }) } else { serde_json::json!({}) };
    SavedChat {
        id: id.into(),
        provider: Provider::Claude,
        folder: "C:\\p".into(),
        account_path: "C:\\acc\\anthropic\\one".into(),
        turns_ended: 3,
        transcript: None,
        mid_turn,
        ties: Ties {
            parent: Some(parent.into()),
            settings,
            owed: vec![(parent.into(), 3, 40)],
            seen: vec![(parent.into(), 3)],
            cut_short: if already { vec![parent.into()] } else { Vec::new() },
            ..Default::default()
        },
    }
}

pub(super) fn take_up(chats: Vec<SavedChat>, sends: Vec<SavedSend>, spawns: Vec<SpawnTaken>) {
    take_up_chats(serde_json::from_slice(&serde_json::to_vec(&SavedChats { chats, sends, spawns }).unwrap()).unwrap());
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
        visible: true,
        in_worktree: None,
        account_path: "C:\\acc\\anthropic\\one".into(),
        worktrees_before: Vec::new(),
        asked: None,
        helper: helper.map(str::to_owned),
        owed_placed: false,
        left_by_core: false,
        settled: false,
        news: None,
        retried: false,
    }
}

/// A visible helper as the last core left it, its first turn having gone with
/// it: asked `opening`, owed to `parent`, with `held` from that parent waiting.
pub(super) fn first_turn_cut(id: &str, parent: &str, provider: Provider, opening: &str, held: &[&str]) {
    let mut chat = cut_short_chat(id, parent, false, true, false);
    chat.provider = provider;
    chat.turns_ended = 0;
    chat.ties.owed = vec![(parent.into(), 0, 0)];
    chat.ties.seen = Vec::new();
    chat.ties.opening = Some(opening.into());
    let sends = held.iter().map(|text| SavedSend { caller: parent.into(), id: id.into(), text: (*text).into(), going_in: None }).collect();
    take_up(vec![chat], sends, Vec::new());
}

/// The window starts it: no longer waiting for that.
pub(super) fn back(id: &str, parent: &str, provider: Provider) {
    register(id, provider, "C:\\p", "C:\\acc\\anthropic\\one", Some(parent), false, serde_json::json!({}));
}

/// A caller a wait is not let go by, and a helper that lost its first turn, back
/// afresh with `held` from it waiting.
pub(super) fn lost_helper(caller: &str, id: &str, dir: &str, held: &[&str]) {
    register(caller, Provider::Claude, dir, dir, None, true, serde_json::json!({}));
    crate::background::test_helper(caller, dir, None, false);
    first_turn_cut(id, caller, Provider::Claude, "the whole job", held);
    back(id, caller, Provider::Claude);
    chat_starting(id, false);
}

pub(super) fn tooled(id: &str) {
    register(id, Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, serde_json::json!({}));
}
