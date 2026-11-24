//! Drive the debt transitions without a CLI, disk transcript, or wall-clock
//! waits. The oracle tracks requests by identity and turn, independently of
//! the aggregate debt, transcript cursor, and receipt used by Luna.

use super::*;
use crate::activity::Turn;
use crate::agents::notifier::{prune_owed, settle_delivered, settle_told, CALLER_RETURN_MS};
use crate::agents::owed::{owe_in, restore_requests, take_requests, Asked};
use crate::agents::persist::{restore_answers, ties_in, Ties};
use crate::agents::read::Message;
use crate::agents::said::{said_of, status_word};
use crate::agents::teardown::{unread_holds, UNREAD_ANSWER_MS};

const CALLERS: [&str; 3] = ["model-caller-a", "model-caller-b", "model-caller-c"];
const HELPERS: [&str; 3] = ["model-helper-a", "model-helper-b", "model-helper-c"];

#[derive(Clone, Copy, Debug)]
enum Action {
    Send(bool, bool), // reply, queued if busy
    Start,
    End(Option<&'static str>),
    Fail,
    Exit,
    Look,
    Delivered,
    Refused,
    Ambiguous,
    Wait(bool, bool), // partial, response may still be canceled
    Cancel,
    Read,
    Reload,
    Handover,
    Forget,
    Away,
}

#[derive(Clone, Copy, Debug)]
struct Event(usize, usize, Action);

#[derive(Clone, Copy, PartialEq, Debug)]
enum Fate {
    Queued,
    Running,
    Ready,
    Receipt,
    Told,
    Gone,
}

#[derive(Debug)]
struct Ask {
    number: u32,
    fate: Fate,
}

struct Draft {
    debt: Owed,
    how: Settled,
    read: LineEvidence,
    covered: Vec<u32>,
    status_words: Vec<usize>,
}

struct Receipt {
    before: Owed,
    removed: Vec<Request>,
    ids: Vec<u32>,
    unread: Option<u64>,
}

#[derive(Default)]
struct Lane {
    requests: Vec<Ask>,
    draft: Option<Draft>,
    receipt: Option<Receipt>,
    statuses: HashSet<usize>,
    unread: Option<u64>,
}

#[derive(Default)]
struct Helper {
    turns: u32,
    running: bool,
    exited: bool,
    failure: Option<u32>,
    transcript: Vec<Message>,
}

struct Machine {
    r: Registry,
    lanes: [[Lane; 3]; 3],
    helpers: [Helper; 3],
    now: u64,
    count: usize,
}

fn known() -> Known {
    Known {
        provider: Provider::Claude,
        folder: String::new(),
        account_path: String::new(),
        account: String::new(),
        parent: None,
        settings: serde_json::Value::Null,
        codex_trust: None,
        tools: true,
        waiting: false,
        orphan: false,
    }
}

fn message(role: &str, text: String, joined: bool) -> Message {
    Message { role: role.into(), text, joined, after_cli: false, final_answer: role == "assistant" }
}

impl Machine {
    fn new(count: usize) -> Self {
        let mut r = Registry::default();
        r.known.extend(CALLERS[..count].iter().chain(&HELPERS[..count]).map(|id| ((*id).into(), known())));
        Self { r, lanes: Default::default(), helpers: Default::default(), now: now_ms(), count }
    }

    fn send(&mut self, c: usize, h: usize, reply: bool, queue: bool) {
        let caller = CALLERS[c];
        let id = HELPERS[h];
        let key = (caller.into(), id.into());
        let s = &mut self.helpers[h];
        let queued = queue && s.running;
        let baseline = s.turns + u32::from(queued);
        let from = s.transcript.len() as u64;
        let n = self.r.requests.get(&key).map_or(1, |q| q.made + 1);
        let text = format!("Request {c}-{h}-{n}.");
        assert!(owe_in(&mut self.r, caller, id, baseline, from, None, Some(&Asked { text: &text, key: None, reply })));
        let q = self.r.requests.get_mut(&key).unwrap().open.last_mut().unwrap();
        q.queued = queued;
        q.taken = !queued;
        self.r.owed.get_mut(&key).unwrap().queued = queued || self.r.requests[&key].open.iter().any(|q| q.queued);
        if reply {
            self.lanes[c][h].requests.push(Ask { number: n, fate: if queued { Fate::Queued } else { Fate::Running } });
        }
        if !queued {
            s.transcript.push(message("user", text, s.running));
            s.running = true;
        }
    }

    fn start(&mut self, h: usize) {
        let s = &mut self.helpers[h];
        if s.running || s.exited {
            return;
        }
        for (c, caller) in CALLERS[..self.count].iter().enumerate() {
            let key = ((*caller).into(), HELPERS[h].into());
            let lane = &mut self.lanes[c][h];
            for ask in lane.requests.iter_mut().filter(|q| q.fate == Fate::Queued) {
                ask.fate = Fate::Running;
            }
            if let Some(qs) = self.r.requests.get_mut(&key).filter(|_| self.r.owed.get(&key).is_some_and(|o| o.active())) {
                for q in qs.open.iter_mut().filter(|q| q.queued && !q.taken) {
                    s.transcript.push(message("user", q.head.clone(), s.running));
                    s.running = true;
                    q.taken = true;
                }
            }
        }
        s.running = true;
    }

    fn end(&mut self, h: usize, text: Option<&str>, failure: bool) {
        let s = &mut self.helpers[h];
        if !s.running {
            return;
        }
        s.running = false;
        s.turns += 1;
        s.failure = failure.then_some(s.turns);
        if let Some(text) = text {
            s.transcript.push(message("assistant", format!("{text} (turn {})", s.turns), false));
        }
        for lanes in &mut self.lanes {
            for ask in lanes[h].requests.iter_mut().filter(|q| q.fate == Fate::Running) {
                ask.fate = Fate::Ready;
            }
        }
    }

    fn look(&mut self, c: usize, h: usize) {
        let lane = &mut self.lanes[c][h];
        if lane.draft.is_some() {
            return;
        }
        let key = (CALLERS[c].into(), HELPERS[h].into());
        let Some(debt) = self.r.owed.get(&key).copied().filter(|o| o.active()) else { return };
        let s = &self.helpers[h];
        if !s.exited
            && verdict(
                &Owed { baseline: debt.notice_baseline(), ..debt },
                s.turns,
                Some(if s.running { Turn::Busy } else { Turn::Idle }),
                false,
                true,
            ) != Verdict::TurnOver
        {
            return;
        }
        let from = debt.notice_cursor() as usize;
        let messages = s.transcript.get(from..).unwrap_or(&[]);
        let fresh = said_of(messages.to_vec());
        let status = fresh_status(&fresh);
        let qs = self.r.requests[&key].open.clone();
        let said = fresh.asked_after(qs, debt.request_match);
        let (requests, plan, _) = said.notice_requests();
        let to = s.transcript.len() as u64;
        let how = if s.exited {
            Settled::Exited
        } else if s.failure.is_some_and(|n| n > debt.notice_baseline()) {
            failed_requests(debt, s.turns, &requests, &[], false)
        } else if status.is_none() && !can_stall(debt, &requests, plan) {
            let unread = !said.is_empty();
            let how = silent_or_untaken(debt, s.turns, &requests, unread);
            if unread && matches!(how, Settled::Silent(..)) {
                lane.unread = Some(self.now);
            }
            settle_told(&mut self.r, key, how, &debt);
            if unread {
                let key = (CALLERS[c].into(), HELPERS[h].into());
                if let Some(left) = self.r.owed.get_mut(&key).filter(|o| !o.active()) {
                    left.wake.unread_to = Some(to);
                }
            }
            return;
        } else {
            turn_settlement(debt, s.turns, to, &said, requests.clone(), status, plan, false, false, false)
        };
        // These are the request outcomes the rendered notice will deliver.
        let covered = requests
            .iter()
            .filter(|q| {
                q.reply
                    && match &how {
                        Settled::Answered | Settled::Failed | Settled::Exited => true,
                        Settled::FailedRequests(_, nums) => nums.contains(&q.n),
                        Settled::Progress(p) => p.requests.iter().any(|reported| reported.n == q.n && reported.told) && q.taken,
                        _ => false,
                    }
            })
            .map(|q| q.n)
            .collect();
        let status_words = messages
            .iter()
            .enumerate()
            .filter(|(_, m)| m.role == "assistant" && status_word(&m.text).is_some())
            .map(|(i, _)| from + i)
            .collect();
        let read = LineEvidence { requests, read_to: Some(to), ended: s.turns, ..LineEvidence::default() };
        lane.draft = Some(Draft { debt, how, read, covered, status_words });
    }

    fn deliver(&mut self, c: usize, h: usize) -> Result<(), String> {
        let lane = &mut self.lanes[c][h];
        let Some(draft) = lane.draft.take() else { return Ok(()) };
        for n in &draft.covered {
            let ask = lane.requests.iter_mut().find(|q| q.number == *n).unwrap();
            if !(ask.fate == Fate::Ready || matches!(draft.how, Settled::Exited) && ask.fate == Fate::Queued) {
                return Err(format!("delivery of request {n} in state {:?}", ask.fate));
            }
            ask.fate = Fate::Told;
        }
        for i in draft.status_words {
            if !lane.statuses.insert(i) {
                return Err(format!("status word at {i} delivered twice"));
            }
        }
        settle_delivered(&mut self.r, CALLERS[c], HELPERS[h].into(), draft.how, &draft.debt, draft.read, Some(self.helpers[h].turns));
        lane.unread = None;
        Ok(())
    }

    fn collect(&mut self, c: usize, h: usize, partial: bool, cancelable: bool, read: bool) {
        let key = (CALLERS[c].into(), HELPERS[h].into());
        let s = &self.helpers[h];
        let lane = &mut self.lanes[c][h];
        if s.running || lane.draft.is_some() || lane.receipt.is_some() {
            return;
        }
        let Some(debt) = self.r.owed.get(&key).copied() else { return };
        if read && !s.transcript.get(debt.cursor as usize..).is_some_and(|part| part.iter().any(|m| m.role == "assistant")) {
            return;
        }
        if !debt.active() {
            if read {
                self.r.owed.get_mut(&key).unwrap().wake.unread_since = None;
                lane.unread = None;
            }
            return;
        }
        let taken: Vec<_> = self.r.requests[&key]
            .open
            .iter()
            .filter(|q| q.taken && q.baseline < s.turns && !q.told)
            .take(if partial { 1 } else { usize::MAX })
            .map(|q| q.n)
            .collect();
        if taken.is_empty() {
            return;
        }
        let mut ids = Vec::new();
        for ask in lane.requests.iter_mut().filter(|q| q.fate == Fate::Ready && taken.contains(&q.number)) {
            ask.fate = if cancelable { Fate::Receipt } else { Fate::Told };
            ids.push(ask.number);
        }
        let removed = take_requests(&mut self.r, &key, debt, debt, &taken, Some(s.turns));
        // The tool response includes the report from the debt's cursor.
        let unread = lane.unread.take();
        if cancelable {
            lane.receipt = Some(Receipt { before: debt, removed, ids, unread });
        }
    }

    fn cancel(&mut self, c: usize, h: usize) {
        let lane = &mut self.lanes[c][h];
        let Some(Receipt { before, removed, ids, unread }) = lane.receipt.take() else { return };
        let key = (CALLERS[c].into(), HELPERS[h].into());
        let current = self.r.owed.get(&key).copied();
        let debt = self.r.owed.entry(key.clone()).or_insert(before);
        let qs = self.r.requests.get_mut(&key).unwrap();
        if current.is_none() {
            qs.open.clear();
        }
        restore_requests(debt, before, qs, removed);
        lane.unread = lane.unread.or(unread);
        for ask in lane.requests.iter_mut().filter(|q| ids.contains(&q.number)) {
            ask.fate = Fate::Ready;
        }
    }

    fn abandon(&mut self, c: usize, h: usize, keep_report: bool) {
        let lane = &mut self.lanes[c][h];
        lane.draft = None;
        lane.receipt = None;
        for ask in lane.requests.iter_mut().filter(|q| q.fate != Fate::Told) {
            ask.fate = Fate::Gone;
        }
        if !keep_report {
            let key = (CALLERS[c].into(), HELPERS[h].into());
            self.r.owed.remove(&key);
            self.r.requests.remove(&key);
            lane.unread = None;
            // Numbering starts afresh after ownership changes. Status words
            // still have distinct offsets in the helper's transcript.
            lane.requests.clear();
        }
    }

    fn step(&mut self, Event(c, h, action): Event) -> Result<(), String> {
        self.now += 1;
        match action {
            Action::Send(reply, queue) if self.r.known.get(CALLERS[c]).is_some_and(|k| !k.waiting) && !self.helpers[h].exited => {
                self.send(c, h, reply, queue);
            }
            Action::Start => self.start(h),
            Action::End(text) => self.end(h, text, false),
            Action::Fail => self.end(h, None, true),
            Action::Exit => {
                self.end(h, None, false);
                self.helpers[h].exited = true;
            }
            Action::Look => self.look(c, h),
            Action::Delivered => self.deliver(c, h)?,
            Action::Refused => self.lanes[c][h].draft = None,
            Action::Ambiguous => {}
            Action::Wait(partial, cancelable) => self.collect(c, h, partial, cancelable, false),
            Action::Cancel => self.cancel(c, h),
            Action::Read => self.collect(c, h, false, false, true),
            Action::Reload => {
                let id = HELPERS[h];
                let ties = serde_json::to_vec(&ties_in(&self.r, id).unwrap()).unwrap();
                self.r.owed.retain(|(_, to), _| to != id);
                self.r.requests.retain(|(_, to), _| to != id);
                restore_answers(&mut self.r, id, &mut serde_json::from_slice::<Ties>(&ties).unwrap());
                for lanes in &mut self.lanes {
                    let lane = &mut lanes[h];
                    lane.draft = None;
                    lane.receipt = None;
                    for ask in lane.requests.iter_mut().filter(|q| q.fate == Fate::Receipt) {
                        ask.fate = Fate::Told;
                    }
                }
            }
            Action::Handover => {
                for c in 0..self.count {
                    // The handover response ends the old ownership's requests.
                    self.abandon(c, h, false);
                }
                *self.r.handovers.entry(HELPERS[h].into()).or_default() += 1;
            }
            Action::Forget => {
                self.r.known.remove(CALLERS[c]);
                for h in 0..self.count {
                    self.abandon(c, h, false);
                }
            }
            Action::Away => {
                if let Some(k) = self.r.known.get_mut(CALLERS[c]) {
                    k.waiting = true;
                }
                prune_owed(&mut self.r.owed, &self.r.known, CALLER_RETURN_MS, self.now, |id| id == CALLERS[c]);
                for h in 0..self.count {
                    self.abandon(c, h, true);
                }
            }
            _ => {}
        }
        self.check()
    }

    fn check(&self) -> Result<(), String> {
        for (c, caller) in CALLERS[..self.count].iter().enumerate() {
            for (h, helper) in HELPERS[..self.count].iter().enumerate() {
                let key = ((*caller).into(), (*helper).into());
                let lane = &self.lanes[c][h];
                let actual = self.r.owed.get(&key);
                let open: Vec<_> = self
                    .r
                    .requests
                    .get(&key)
                    .into_iter()
                    .flat_map(|qs| &qs.open)
                    .filter(|q| q.reply && !q.told && actual.is_some_and(|o| o.active()))
                    .map(|q| q.n)
                    .collect();
                let pending: Vec<_> = lane
                    .requests
                    .iter()
                    .filter(|q| matches!(q.fate, Fate::Queued | Fate::Running | Fate::Ready))
                    .map(|q| q.number)
                    .collect();
                if open != pending {
                    return Err(format!("{caller}/{helper}: open {open:?}, expected {pending:?}"));
                }
                if !pending.is_empty() && actual.is_some_and(|o| o.no_reply) {
                    return Err(format!("{caller}/{helper}: FYI suppressed a reply"));
                }
                if lane.unread.is_some_and(|at| self.now < at + UNREAD_ANSWER_MS) && actual.is_none_or(|o| !unread_holds(o, self.now)) {
                    return Err(format!("{caller}/{helper}: unread report lost"));
                }
            }
        }
        Ok(())
    }
}

fn replay(count: usize, events: &[Event]) -> Result<(), String> {
    let mut machine = Machine::new(count);
    for &event in events {
        machine.step(event)?;
    }
    // Give every accepted input its turn and drain notices. If everyone is
    // idle, no reply-wanted request may need another unrelated turn to wake.
    for (h, helper) in HELPERS[..count].iter().enumerate() {
        machine.end(h, Some("Result"), false);
        machine.start(h);
        machine.end(h, Some("Result"), false);
        for (c, caller) in CALLERS[..count].iter().enumerate() {
            machine.deliver(c, h)?;
            machine.look(c, h);
            machine.deliver(c, h)?;
            if machine.lanes[c][h].requests.iter().any(|q| matches!(q.fate, Fate::Queued | Fate::Running | Fate::Ready)) {
                return Err(format!("all idle but {caller}/{helper} still awaits a reply"));
            }
        }
    }
    machine.check()
}

#[test]
fn seeded_debt_interleavings() {
    let actions = [
        Action::Send(true, false),
        Action::Send(true, false),
        Action::Send(true, true),
        Action::Send(false, false),
        Action::Send(false, true),
        Action::Start,
        Action::End(None),
        Action::End(Some("Plain result")),
        Action::End(Some("DONE: result")),
        Action::End(Some("BLOCKED: no result")),
        Action::Fail,
        Action::Exit,
        Action::Look,
        Action::Look,
        Action::Delivered,
        Action::Refused,
        Action::Ambiguous,
        Action::Wait(true, true),
        Action::Wait(false, true),
        Action::Wait(false, false),
        Action::Cancel,
        Action::Read,
        Action::Reload,
        Action::Handover,
        Action::Forget,
        Action::Away,
    ];
    let sequences = std::env::var("LUNA_DEBT_SEQUENCES").ok().map(|n| n.parse().expect("sequence count")).unwrap_or(512);
    let regressions = [0x7073_7bee_0ba8_55a5, 0x92e0_3411_6c3a_9abf, 0xf931_dab7_0283_2a9d, 0x3b84_61af_1687_bb28];
    let seeds = (0..sequences).map(|index| 0x5337_c243_d089_61e1_u64 ^ (index as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15));
    for seed in regressions.into_iter().chain(seeds) {
        let mut state = seed;
        let mut next = |limit: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as usize % limit
        };
        let count = 2 + next(2);
        let mut events: Vec<_> = (0..40).map(|_| Event(next(count), next(count), actions[next(actions.len())])).collect();
        if let Err(mut why) = replay(count, &events) {
            // Greedily delete until no one event can be removed while
            // retaining a failure. A shorter witness is easier to debug.
            let mut i = 0;
            while i < events.len() {
                let removed = events.remove(i);
                if let Err(found) = replay(count, &events) {
                    why = found;
                    i = 0;
                } else {
                    events.insert(i, removed);
                    i += 1;
                }
            }
            panic!("seed {seed:#018x}: {why}; events {events:?}");
        }
    }
}
