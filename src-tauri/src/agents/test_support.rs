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
use std::cell::RefCell;
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
        codex_trust: None,
        ties: Ties { owed: vec![(caller.into(), baseline, cursor)], seen: vec![(caller.into(), baseline)], ..Default::default() },
    }
}

pub(super) fn take_up(chats: Vec<SavedChat>, sends: Vec<SavedSend>, spawns: Vec<SpawnTaken>) {
    let saved = SavedChats { chats, sends, spawns, no_handoff: true, ..SavedChats::default() };
    take_up_chats(serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap());
}

thread_local! {
    static SWEEP_SESSIONS: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
}

/// A core sweeps the whole registry. A test sweeps only its fixtures,
/// since other tests may be partway through setting up theirs.
pub(super) fn sweep_only(ids: &[&'static str]) -> impl Drop {
    struct Scope {
        previous: Vec<&'static str>,
        thread: std::marker::PhantomData<*const ()>,
    }

    impl Drop for Scope {
        fn drop(&mut self) {
            SWEEP_SESSIONS.with_borrow_mut(|ids| *ids = std::mem::take(&mut self.previous));
        }
    }

    Scope { previous: SWEEP_SESSIONS.replace(ids.to_vec()), thread: std::marker::PhantomData }
}

pub(crate) fn sweep_includes(id: &str) -> bool {
    SWEEP_SESSIONS.with_borrow(|ids| ids.contains(&id))
}

pub(super) fn spawn_taken(request: u64, caller: &str, helper: Option<&str>, resume: bool) -> SpawnTaken {
    SpawnTaken {
        request,
        caller: caller.into(),
        params: serde_json::from_value(serde_json::json!({ "prompt": "the whole job, word for word", "name": "the job" })).unwrap(),
        auto_delete: None,
        resume_on_restart: resume,
        connectors: false,
        in_worktree: None,
        account_path: "C:\\acc\\anthropic\\one".into(),
        helper: helper.map(str::to_owned),
        owed_placed: false,
        left_by_core: false,
        settled: false,
        news: None,
    }
}

/// A place in production code a test may stop a call at (`stop_at`), to put
/// something of its own between two of the call's steps: the call stops at
/// `reached` with the same point and session, until the test lets it go.
#[allow(clippy::type_complexity)]
static STOPS: Mutex<Vec<(&'static str, String, std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>> = Mutex::new(Vec::new());

/// A test's hold on a call stopped at a point (`stop_at`).
pub(super) struct Stop {
    reached: std::sync::mpsc::Receiver<()>,
    go: std::sync::mpsc::Sender<()>,
}

impl Stop {
    /// Until the call is at the point.
    pub(super) fn wait(&self) {
        self.reached.recv_timeout(std::time::Duration::from_secs(20)).expect("the call never reached the point");
    }

    /// The call goes on (as it does when the hold is dropped).
    pub(super) fn go(self) {
        let _ = self.go.send(());
    }
}

/// The next call to reach `point` for session `who` stops there.
pub(super) fn stop_at(point: &'static str, who: &str) -> Stop {
    let (tell, reached) = std::sync::mpsc::channel();
    let (go, wait) = std::sync::mpsc::channel();
    STOPS.lock().unwrap().push((point, who.to_string(), tell, wait));
    Stop { reached, go }
}

/// Production code at `point` for session `who`: stops there if a test asked.
pub(super) fn reached(point: &str, who: &str) {
    let found = {
        let mut stops = STOPS.lock().unwrap_or_else(|e| e.into_inner());
        stops.iter().position(|(p, w, ..)| *p == point && w == who).map(|i| stops.remove(i))
    };
    if let Some((_, _, tell, wait)) = found {
        let _ = tell.send(());
        let _ = wait.recv();
    }
}
