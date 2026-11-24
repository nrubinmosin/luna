//! Seeded histories with an oracle at the terminal boundary. The histories
//! specify which input causes each wake and which requests it can cover. No
//! notice verdict, rendered evidence, or settlement is used as an expectation.

use super::super::{deliver_notice_with, notify_once_since_start, Notice, TypingInto, NOTIFY_IDLE_LOOKS};
use crate::agents::owed::{owe_from, owe_sent, requests_of, resumed, retry_deliveries, Asked};
use crate::agents::persist::{restore_answers, ties_in};
use crate::agents::read::{move_transcript_for_test, transcripts};
use crate::agents::registry::{reg, register};
use crate::agents::teardown::forget;
use crate::provider::Provider;
use crate::throttle::{at_time_for_test, now_ms};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const TOP: &str = "oracle-top";
const WORKER: &str = "oracle-worker";
const PEER: &str = "oracle-peer";
const DELEGATE: &str = "oracle-delegate";
const SESSIONS: [&str; 4] = [TOP, WORKER, PEER, DELEGATE];
const GRACE: u64 = 25_000;

#[derive(Clone, Copy)]
enum WriteResult {
    Started,
    Refused,
    Held,
    Ambiguous,
}

struct Seed(u64);

impl Seed {
    fn pick(&mut self, limit: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % limit
    }

    fn coin(&mut self) -> bool {
        self.pick(2) == 0
    }
}

struct Fixture {
    dir: PathBuf,
    now: u64,
    up: u64,
    provider: Provider,
}

impl Fixture {
    fn new(dir: PathBuf, provider: Provider) -> Self {
        Self { dir, now: now_ms(), up: 0, provider }
    }

    fn path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.jsonl"))
    }

    fn session(&self, id: &str, parent: Option<&str>, lead: bool) {
        register(id, self.provider, ".", "oracle-account", parent, lead, serde_json::json!({"background": parent.is_some()}));
        self.idle(id, 0, false);
        let path = self.path(id);
        fs::write(&path, "").unwrap();
        transcripts().lock().unwrap().insert(id.into(), path);
    }

    fn idle(&self, id: &str, ends: u32, job: bool) {
        crate::background::idle_for_test(id, self.provider, ends, job);
    }

    fn append(&self, id: &str, role: &str, words: &str, joined: bool) {
        let line = match (self.provider, role, joined) {
            (Provider::Claude, "user", true) => serde_json::json!({
                "type": "attachment", "attachment": {
                    "type": "queued_command", "prompt": words, "source_uuid": "joined-prompt"
                }
            }),
            (Provider::Claude, "user", false) => serde_json::json!({"type": "user", "message": {"content": words}}),
            (Provider::Claude, ..) => serde_json::json!({
                "type": "assistant", "message": {"content": [{"type": "text", "text": words}]}
            }),
            (Provider::Codex, ..) => serde_json::json!({
                "type": "response_item", "payload": {"type": "message", "role": role, "content": [{
                    "type": if role == "user" { "input_text" } else { "output_text" }, "text": words
                }]}
            }),
        };
        let path = &transcripts().lock().unwrap()[id];
        writeln!(OpenOptions::new().append(true).open(path).unwrap(), "{line}").unwrap();
    }

    fn send(&self, caller: &str, id: &str, baseline: u32, words: &str, reply: bool, queued: bool) -> crate::background::Queued {
        let q = crate::background::Queued::new(Some(caller.into()), words.into());
        let cursor = crate::agents::read::send_cursor(id);
        assert!(owe_sent(
            caller,
            id,
            baseline,
            cursor,
            None,
            &Asked { text: words, key: None, reply },
            (queued, Some(q.id.clone()), false)
        ));
        q
    }

    fn advance(&mut self, ms: u64) {
        self.now += ms;
        self.up += ms;
    }

    fn look(&self) -> Vec<(TypingInto, String, Notice)> {
        let mut drafts = Vec::new();
        at_time_for_test(self.now, || {
            notify_once_since_start(
                crate::pty::manager(),
                &mut SESSIONS.into_iter().map(|id| (id.into(), NOTIFY_IDLE_LOOKS - 1)).collect(),
                &mut Default::default(),
                self.up,
                |id| SESSIONS.contains(&id),
                |typing, caller, notice| drafts.push((typing, caller, notice)),
            );
        });
        drafts
    }

    fn deliver(&self, drafts: Vec<(TypingInto, String, Notice)>, result: WriteResult) -> Vec<(String, String)> {
        self.deliver_while(drafts, result, || {})
    }

    fn deliver_while(
        &self,
        drafts: Vec<(TypingInto, String, Notice)>,
        result: WriteResult,
        mut while_typing: impl FnMut(),
    ) -> Vec<(String, String)> {
        let mut writes = Vec::new();
        at_time_for_test(self.now, || {
            for (_typing, caller, notice) in drafts {
                deliver_notice_with(crate::pty::manager(), &caller, notice, |text, _, baseline| {
                    writes.push((caller.clone(), text.into()));
                    while_typing();
                    let typed = match result {
                        WriteResult::Started => Ok(Some(true)),
                        WriteResult::Refused => Err("start gate refused the write".into()),
                        WriteResult::Held => Ok(None),
                        WriteResult::Ambiguous => Ok(Some(false)),
                    };
                    (typed, baseline + 1, false)
                });
            }
        });
        writes
    }

    fn tick(&self, result: WriteResult) -> Vec<(String, String)> {
        self.deliver(self.look(), result)
    }

    #[track_caller]
    fn checkpoint(&mut self, expected: &[(&str, &[&str])]) {
        let mut writes = self.tick(WriteResult::Started);
        self.advance(GRACE);
        writes.extend(self.tick(WriteResult::Started));
        expect(&writes, expected);
    }

    fn requests(&self, caller: &str, id: &str, expected: &[u32]) {
        let open: Vec<_> = requests_of(caller, id).iter().filter(|q| !q.told).map(|q| q.n).collect();
        assert_eq!(open, expected, "{caller} -> {id}");
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for id in SESSIONS.into_iter().rev() {
            crate::background::gone_for_test(id);
            forget(id);
            crate::activity::forget(id);
        }
    }
}

#[track_caller]
fn expect(writes: &[(String, String)], expected: &[(&str, &[&str])]) {
    assert_eq!(writes.len(), expected.len(), "expected {expected:?}, wrote {writes:?}");
    for ((caller, text), (target, causes)) in writes.iter().zip(expected) {
        assert_eq!(caller, target, "{writes:?}");
        assert!(text.starts_with("[Luna notice, not typed by the user]"), "{text}");
        assert!(causes.iter().all(|cause| text.contains(cause)), "expected {causes:?} in {text}");
    }
}

fn ordinary(f: &mut Fixture, rng: &mut Seed) {
    f.session(TOP, None, true);
    f.session(WORKER, Some(TOP), false);
    let wants_reply = rng.coin();
    let status = rng.coin();
    let words = if status { "DONE: completed" } else { "Ordinary result." };
    f.send(TOP, WORKER, 0, "First task", wants_reply, false);
    f.append(WORKER, "user", "First task", false);
    at_time_for_test(f.now, || crate::background::in_turn_for_test(WORKER));
    f.checkpoint(&[]);
    if rng.coin() {
        f.send(TOP, WORKER, 0, "Heads up", false, false);
        f.append(WORKER, "user", "Heads up", true);
    }
    f.append(WORKER, "assistant", words, false);
    f.idle(WORKER, 1, false);
    let causes: &[&str] = if wants_reply || status { &["finished its turn"] } else { &[] };
    if causes.is_empty() {
        f.checkpoint(&[]);
    } else {
        for _ in 0..rng.pick(4) {
            let result = if rng.coin() { WriteResult::Refused } else { WriteResult::Held };
            expect(&f.tick(result), &[(TOP, causes)]);
        }
        let result = if rng.coin() { WriteResult::Started } else { WriteResult::Ambiguous };
        expect(&f.tick(result), &[(TOP, causes)]);
        f.requests(TOP, WORKER, &[]);
    }
    // Liveness above is checked while still on the first completed turn.
    // Another turn with no new task never buys another idle stall.
    f.checkpoint(&[]);
    f.idle(WORKER, 2, false);
    f.append(WORKER, "assistant", "Unrelated commentary.", false);
    f.checkpoint(&[]);
}

fn queued_or_joined(f: &mut Fixture, rng: &mut Seed) {
    f.session(TOP, None, true);
    f.session(WORKER, Some(TOP), false);
    let queued = rng.coin();
    f.send(TOP, WORKER, 0, "First task", true, false);
    f.append(WORKER, "user", "First task", false);
    f.send(TOP, WORKER, u32::from(queued), "Next task", true, queued);
    f.append(WORKER, "assistant", "DONE: first result", false);
    if !queued {
        f.append(WORKER, "user", "Next task", true);
        f.append(WORKER, "assistant", "Second result.", false);
    }
    f.idle(WORKER, 1, false);
    let visible = if queued { "DONE: first result" } else { "Second result." };
    f.checkpoint(&[(TOP, &[visible])]);
    if queued {
        f.requests(TOP, WORKER, &[2]);
        f.checkpoint(&[]); // An unopened queued prompt is no new reason to wake.
        f.append(WORKER, "user", "Next task", false);
        f.append(WORKER, "assistant", "Second result.", false);
        f.idle(WORKER, 2, false);
        f.checkpoint(&[(TOP, &["Second result."])]);
    }
    f.requests(TOP, WORKER, &[]);
    f.checkpoint(&[]);
}

fn plan(f: &mut Fixture, rng: &mut Seed) {
    f.session(TOP, None, true);
    f.session(WORKER, Some(TOP), true);
    f.send(TOP, WORKER, 0, "Propose a plan", true, false);
    f.append(WORKER, "user", "Propose a plan", false);
    f.append(WORKER, "assistant", "PLAN: inspect then build", false);
    f.idle(WORKER, 1, false);
    f.checkpoint(&[(TOP, &["PLAN: inspect then build"])]);
    f.send(TOP, WORKER, 1, "Heads up", false, false);
    f.append(WORKER, "user", "Heads up", false);
    f.append(WORKER, "assistant", "Acknowledged.", false);
    f.idle(WORKER, 2, false);
    f.checkpoint(&[]);
    if rng.coin() {
        f.send(TOP, WORKER, 2, "Approved. Continue.", true, false);
        f.append(WORKER, "user", "Approved. Continue.", false);
        f.append(WORKER, "assistant", "Result after approval.", false);
        f.idle(WORKER, 3, false);
        f.checkpoint(&[(TOP, &["finished its turn"])]);
    } else {
        f.append(WORKER, "assistant", "DONE: plan no longer needed", false);
        f.idle(WORKER, 3, false);
        f.checkpoint(&[(TOP, &["finished its turn"])]);
    }
    f.checkpoint(&[]);
}

fn lead_and_job(f: &mut Fixture, rng: &mut Seed) {
    f.session(TOP, None, true);
    f.session(WORKER, Some(TOP), true);
    f.session(DELEGATE, Some(WORKER), false);
    let plan = rng.coin();
    let pending = !plan && rng.coin();
    if pending {
        owe_from(TOP, WORKER, 0, 0);
    } else {
        f.send(TOP, WORKER, 0, "Run the job", true, false);
    }
    f.append(WORKER, "user", "Run the job", false);
    let delegate = rng.coin();
    if delegate {
        owe_from(WORKER, DELEGATE, 0, 0);
        at_time_for_test(f.now, || crate::background::in_turn_for_test(DELEGATE));
    }
    let words = if plan {
        "PLAN: run the build"
    } else if pending {
        "BLOCKED: waiting for the build"
    } else {
        "The job is running."
    };
    f.append(WORKER, "assistant", words, false);
    f.idle(WORKER, 1, true);
    if plan || pending {
        f.checkpoint(&[(TOP, &[words, "job running"])]);
    } else {
        f.checkpoint(&[]);
    }
    f.checkpoint(&[]);
    if delegate {
        f.append(DELEGATE, "assistant", "DONE: delegate result", false);
        f.idle(DELEGATE, 1, false);
        f.checkpoint(&[(WORKER, &["delegate result"])]);
        // This was a turn on its delegate's notice, with no word on the end.
        f.idle(WORKER, 2, true);
        f.checkpoint(&[]);
    }
    crate::background::left_for_test(WORKER, &[]);
    if plan {
        f.checkpoint(&[]);
        f.checkpoint(&[]);
    } else {
        f.checkpoint(&[]); // The first look at job end holds, then queues the wake.
        f.checkpoint(&[(WORKER, &["Your delegates have finished"])]);
        f.checkpoint(&[]);
        let ended = if delegate { 3 } else { 2 };
        f.idle(WORKER, ended, false);
        if rng.coin() {
            f.append(WORKER, "assistant", "DONE: job result", false);
            f.checkpoint(&[(TOP, &["finished its turn"])]);
        } else if pending {
            // The prior BLOCKED already used the task's idle-stall allowance.
            f.checkpoint(&[]);
        } else {
            f.checkpoint(&[(TOP, &["seems stuck"])]);
        }
        f.checkpoint(&[]);
    }
}

fn peer_cycle(f: &mut Fixture, rng: &mut Seed) {
    let peers = if rng.coin() { &SESSIONS[..2] } else { &SESSIONS[..3] };
    for &id in peers {
        f.session(id, None, true);
        f.append(id, "assistant", "A plain peer result.", false);
        f.idle(id, 1, false);
    }
    for (i, &id) in peers.iter().enumerate() {
        owe_from(id, peers[(i + 1) % peers.len()], 0, 0);
    }
    let mut expected: Vec<(&str, &[&str])> = peers.iter().map(|&id| (id, &["A plain peer result."][..])).collect();
    expected.sort_unstable_by_key(|&(id, _)| id);
    f.checkpoint(&expected);
    // The notices themselves ended a turn, but bring no new task to answer.
    for &id in peers {
        f.idle(id, 2, false);
    }
    f.checkpoint(&[]);
}

fn quiet_and_refusal(f: &mut Fixture, rng: &mut Seed) {
    f.session(TOP, None, true);
    f.session(WORKER, Some(TOP), true);
    f.send(TOP, WORKER, 0, "Run the job", true, false);
    f.append(WORKER, "user", "Run the job", false);
    at_time_for_test(f.now, || crate::background::quiet_for_test(WORKER, 29 * 60_000));
    f.checkpoint(&[]);
    f.advance(60_000);
    f.checkpoint(&[(TOP, &["mid-turn and has written nothing"])]);
    f.checkpoint(&[]);
    // A new turn on the same task gets no separate quiet wake. It can ride
    // the refusal once that has a reason to go by itself.
    f.idle(WORKER, 1, true);
    f.checkpoint(&[]);
    at_time_for_test(f.now, || crate::background::quiet_for_test(WORKER, 31 * 60_000));
    f.checkpoint(&[]);

    crate::agents::spawn::refused_a_place(&mut reg().lock().unwrap(), WORKER);
    let top = crate::activity::SessionLife::of(TOP, crate::activity::life(TOP).unwrap());
    let pending = || super::super::places_pending(std::slice::from_ref(&top));
    assert!(pending());
    f.checkpoint(&[]); // A fresh refusal rides while the caller has active work.
    let aged = rng.coin();
    if aged {
        reg().lock().unwrap().places_refused.get_mut(TOP).unwrap().since = Instant::now() - Duration::from_secs(601);
        assert!(!pending());
        f.checkpoint(&[(TOP, &["No place was left", "mid-turn and has written nothing"])]);
    }
    f.append(WORKER, "assistant", "DONE: job result", false);
    f.idle(WORKER, 2, false);
    let causes: &[&str] = if aged { &["DONE: job result"] } else { &["DONE: job result", "No place was left"] };
    f.checkpoint(&[(TOP, causes)]);
    assert!(!pending());
    f.checkpoint(&[]);
}

fn changed_delivery(f: &mut Fixture, rng: &mut Seed) {
    f.session(TOP, None, true);
    f.session(WORKER, Some(TOP), false);
    let q = f.send(TOP, WORKER, 0, "Do the work", true, false);
    f.append(WORKER, "user", "Do the work", false);
    f.append(WORKER, "assistant", "DONE: original attempt", false);
    f.idle(WORKER, 1, false);
    let draft = f.look();
    assert_eq!(draft.len(), 1);
    if rng.coin() {
        retry_deliveries(WORKER, &[q], 1);
        expect(&f.deliver(draft, WriteResult::Started), &[]);
        f.checkpoint(&[]);
        f.append(WORKER, "user", "Do the work", false);
        f.append(WORKER, "assistant", "DONE: retry succeeded", false);
        f.idle(WORKER, 2, false);
        f.checkpoint(&[(TOP, &["retry succeeded"])]);
    } else {
        let new = f.dir.join("destination.jsonl");
        fs::write(&new, "").unwrap();
        move_transcript_for_test(WORKER, new);
        expect(&f.deliver(draft, WriteResult::Started), &[]);
        f.checkpoint(&[]);
        f.append(WORKER, "user", "Do the work", false);
        f.append(WORKER, "assistant", "DONE: destination result", false);
        f.checkpoint(&[(TOP, &["destination result"])]);
    }
    f.requests(TOP, WORKER, &[]);
    f.checkpoint(&[]);
}

fn restart_and_caller_return(f: &mut Fixture, rng: &mut Seed) {
    f.session(TOP, None, true);
    f.session(WORKER, Some(TOP), false);
    f.send(TOP, WORKER, 0, "Finish the work", true, false);
    f.append(WORKER, "user", "Finish the work", false);
    let resume = rng.coin();
    let lost_job = rng.coin();
    let key = (TOP.into(), WORKER.into());
    if resume {
        resumed(WORKER, 0);
    } else if !lost_job {
        f.idle(WORKER, 1, false);
        crate::background::cut_for_test(WORKER);
    }
    if lost_job {
        reg().lock().unwrap().owed.get_mut(&key).unwrap().job_went = true;
    }
    if rng.coin() {
        let mut r = reg().lock().unwrap();
        let saved = serde_json::to_vec(&ties_in(&r, WORKER).unwrap()).unwrap();
        r.owed.remove(&key);
        r.requests.remove(&key);
        restore_answers(&mut r, WORKER, &mut serde_json::from_slice(&saved).unwrap());
    }
    let returns = rng.coin();
    reg().lock().unwrap().known.get_mut(TOP).unwrap().waiting = true;
    f.advance(if returns { 8 * 60_000 } else { 10 * 60_000 });
    f.checkpoint(&[]);
    reg().lock().unwrap().known.get_mut(TOP).unwrap().waiting = false;
    if returns {
        f.checkpoint(&[(TOP, &["core restarted"])]);
    } else {
        f.checkpoint(&[]);
    }
    if resume || lost_job {
        f.append(WORKER, "assistant", "DONE: work after restart", false);
        f.idle(WORKER, 1, false);
        if returns {
            f.checkpoint(&[(TOP, &["work after restart"])]);
        } else {
            f.checkpoint(&[]);
            assert!(reg().lock().unwrap().owed[&key].wake.unread_since.is_some(), "report remains readable after expiry");
        }
    }
    f.checkpoint(&[]);
}

fn held_and_collected(f: &mut Fixture, rng: &mut Seed) {
    f.session(TOP, None, true);
    f.session(WORKER, Some(TOP), false);
    crate::agents::held::hold(TOP, WORKER, "A held request", None, true).unwrap();
    f.checkpoint(&[]);
    crate::background::gone_for_test(WORKER);
    crate::agents::held::deliver_held(crate::pty::manager());
    f.checkpoint(&[(TOP, &["exited before your held send went in"])]);
    f.idle(WORKER, 0, false);
    f.send(TOP, WORKER, 0, "A collected request", true, false);
    f.append(WORKER, "user", "A collected request", false);
    f.append(WORKER, "assistant", "DONE: collected result", false);
    f.idle(WORKER, 1, false);
    let call = crate::mcp::Call::new();
    if rng.coin() {
        let result = crate::agents::read::report_get(TOP, WORKER, None, true, &call).unwrap();
        assert!(result.acked);
    } else {
        let result = crate::agents::wait::wait(TOP, WORKER, "turn_done", 1, Some(0), None, &call).unwrap();
        assert_eq!(result.reply.as_deref(), Some("DONE: collected result"));
    }
    f.checkpoint(&[]);
    if rng.coin() {
        call.cancel();
        f.checkpoint(&[(TOP, &["collected result"])]);
    }
    f.checkpoint(&[]);
}

fn overlapping_requests(f: &mut Fixture, rng: &mut Seed) {
    f.session(TOP, None, true);
    f.session(WORKER, Some(TOP), false);
    f.send(TOP, WORKER, 0, "Task one", true, false);
    f.append(WORKER, "user", "Task one", false);
    let mut expected_reply = true;
    let count = 3 + rng.pick(5) as u32;
    for turn in 1..=count {
        let explicit_status = rng.coin();
        f.append(WORKER, "assistant", if explicit_status { "DONE: completed" } else { "Ordinary result." }, false);
        f.idle(WORKER, turn, false);
        if expected_reply || explicit_status {
            let draft = f.look();
            assert_eq!(draft.len(), 1, "the current turn must wake before another turn begins");
            let next_reply = rng.coin();
            let next = format!("Task after turn {turn}");
            let result = if rng.coin() { WriteResult::Started } else { WriteResult::Ambiguous };
            let mut sent = false;
            let writes = f.deliver_while(draft, result, || {
                if turn < count {
                    f.send(TOP, WORKER, turn, &next, next_reply, false);
                    sent = true;
                }
            });
            expect(&writes, &[(TOP, &["finished its turn"])]);
            if turn < count {
                assert!(sent);
                f.requests(TOP, WORKER, &[turn + 1]);
                f.checkpoint(&[]);
                f.append(WORKER, "user", &next, false);
            }
            expected_reply = next_reply;
        } else {
            f.checkpoint(&[]);
            if turn < count {
                let next = format!("Task after turn {turn}");
                expected_reply = rng.coin();
                f.send(TOP, WORKER, turn, &next, expected_reply, false);
                f.append(WORKER, "user", &next, false);
            }
        }
        f.checkpoint(&[]);
    }
}

fn failed_batch(f: &mut Fixture, rng: &mut Seed) {
    f.session(TOP, None, true);
    f.session(WORKER, Some(TOP), false);
    f.send(TOP, WORKER, 0, "First task", true, false);
    f.append(WORKER, "user", "First task", false);
    let later = f.send(TOP, WORKER, 1, "Next task", rng.coin(), true);
    f.append(WORKER, "assistant", "First result.", false);
    crate::background::fail_next_launch_for_test(WORKER, later);
    let draft = f.look();
    assert_eq!(draft.len(), 1);
    let result = if rng.coin() { WriteResult::Started } else { WriteResult::Ambiguous };
    let move_while_typing = rng.coin();
    let writes = f.deliver_while(draft, result, || {
        if move_while_typing {
            let new = f.dir.join("destination.jsonl");
            fs::write(&new, "").unwrap();
            move_transcript_for_test(WORKER, new);
            f.append(WORKER, "user", "First task", false);
            f.append(WORKER, "assistant", "First result.", false);
        }
    });
    expect(&writes, &[(TOP, &["before opening its prompt", "Not delivered: 1 message"])]);
    f.requests(TOP, WORKER, &[1]);
    f.checkpoint(&[(TOP, &["First result."])]);
    f.requests(TOP, WORKER, &[]);
    f.checkpoint(&[]);
}

#[test]
fn seeded_debt_interleavings() {
    let _sweep = crate::agents::test_support::sweep_only(&SESSIONS);
    let sequences = std::env::var("LUNA_DEBT_SEQUENCES").ok().map(|n| n.parse().expect("sequence count")).unwrap_or(512);
    let dir = std::env::temp_dir().join(format!("luna-notice-oracle-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let cases = [
        ordinary,
        queued_or_joined,
        plan,
        lead_and_job,
        peer_cycle,
        quiet_and_refusal,
        changed_delivery,
        restart_and_caller_return,
        held_and_collected,
        overlapping_requests,
        failed_batch,
    ];
    let mut failed = vec![false; cases.len()];
    for index in 0..sequences {
        let case = (index % cases.len() as u64) as usize;
        if failed[case] {
            continue;
        }
        let seed = 0x5337_c243_d089_61e1 ^ index.wrapping_mul(0x9e37_79b9_7f4a_7c15);
        let mut rng = Seed(seed);
        let provider = if rng.coin() { Provider::Claude } else { Provider::Codex };
        let mut fixture = Fixture::new(dir.clone(), provider);
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cases[case](&mut fixture, &mut rng))).is_err() {
            eprintln!("notifier oracle case {case}, sequence {index}, seed {seed:#018x}, provider {provider:?}");
            failed[case] = true;
        }
    }
    fs::remove_dir_all(dir).unwrap();
    assert!(!failed.into_iter().any(|failed| failed));
}
