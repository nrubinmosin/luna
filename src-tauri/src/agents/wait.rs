//! `luna_wait`: one session or several, until a turn ends, a prompt holds it,
//! it exits or the time is up, with the answer handed over on the way out.

use crate::agents::ask::{ask_of, live, state_of, Ask};
use crate::agents::delivery::{is_held, screen_of, POLL};
use crate::agents::lost::{lost_answer, still_all_lost, undelivered_note};
use crate::agents::owed::{hand_over, last_shown, looked, Owed, QUEUED_GRACE_MS};
use crate::agents::read::{failure_of, transcript_found, transcript_path, MAX_TEXT};
use crate::agents::registry::{authorize_view, reg, waits_on_helpers, DEV_CALLER};
use crate::agents::resume::resume_pending;
use crate::agents::said::{answer_file, answer_text, replies_since, said_since, Said};
use crate::agents::{job_end_stood, news_for_parent, real_turn_past, verdict, Verdict};
use crate::provider::Provider;
use crate::throttle::now_ms;
use serde::Serialize;

/// After a caller answers a prompt, how long the prompt it answered may still
/// read as open (the sampler runs every 5 s).
const ANSWERED_MS: u64 = 8_000;

/// Claude Code fires its Stop hook before the turn's last message reaches the
/// transcript: a turn seen over with no reply yet gets this long for one.
pub(super) const REPLY_GRACE_MS: u64 = 5_000;

/// How long a Codex caller's turn has to look over before a `wait` it made
/// counts as let go (see `let_go`): more than one activity sample.
const LET_GO_MS: u64 = 8_000;

/// How long a wait on any of several goes on after the first has finished, so
/// that those done at the same moment come too: longer than a wait's look.
const SAME_LOOK: std::time::Duration = std::time::Duration::from_millis(700);

/// What a `wait` that was let go says.
pub(super) const LET_GO: &str = "this wait was let go before it ended; the answer stays owed and comes as a notice";

/// A `wait` by `caller` on a session, counted for as long as it runs.
pub(super) struct WaitOut((String, String));

impl WaitOut {
    pub(super) fn start(caller: &str, id: &str) -> WaitOut {
        let key = (caller.to_string(), id.to_string());
        *reg().lock().unwrap_or_else(|e| e.into_inner()).waits.entry(key.clone()).or_default() += 1;
        WaitOut(key)
    }
}

impl Drop for WaitOut {
    fn drop(&mut self) {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = r.waits.get_mut(&self.0) {
            *n -= 1;
            if *n == 0 {
                r.waits.remove(&self.0);
            }
        }
    }
}

/// A `wait_many` out on a session, for as long as it runs. Its answer stays owed
/// until the whole call has its response, so that a core going in between
/// leaves it owed in `helpers.json`; this is what keeps the notice from telling
/// it meanwhile.
pub(super) struct Reserved((String, String));

impl Reserved {
    pub(super) fn start(caller: &str, id: &str) -> Reserved {
        let key = (caller.to_string(), id.to_string());
        *reg().lock().unwrap_or_else(|e| e.into_inner()).reserved.entry(key.clone()).or_default() += 1;
        Reserved(key)
    }
}

impl Drop for Reserved {
    fn drop(&mut self) {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = r.reserved.get_mut(&self.0) {
            *n -= 1;
            if *n == 0 {
                r.reserved.remove(&self.0);
            }
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Waited {
    pub outcome: &'static str,
    /// See `state_of`.
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ask: Option<Ask>,
    pub turns_ended: u32,
    /// On `turn_done`, and on `exit` after such a turn: all the session said
    /// since the caller asked it something, as `reply_of` puts it, so no
    /// `read` is needed. On `job_done`: what it said since it was announced as
    /// not done, the same as that notice would bring.
    pub reply: Option<String>,
    /// The turn did not end by itself: it stopped on this error (`failure_of`),
    /// and `reply` is what it said before that, if anything.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed: Option<String>,
    /// Luna found no transcript, so a null `reply` says nothing about whether
    /// it replied.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub transcript_missing: bool,
    /// On `timeout`, `waiting`, a turn that ended on an error or without a
    /// reply: the terminal's last lines, so the caller sees what the session
    /// is on — a permission prompt, a question, an error, a notice nobody
    /// dismissed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screen: Option<String>,
    /// See `undelivered_note`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub undelivered: Option<String>,
    /// On `job_done`, the debt that end settles (`take_debt`), for whichever
    /// hands it over: this `wait`, or a `wait_many` once it has its response.
    #[serde(skip)]
    pub(super) job_debt: Option<u64>,
}

/// Blocks until the session reaches `until`, or `timeout_s` passes:
/// `turn_done` — a turn ended after `after_turn` (default: the count the
/// caller's question still unanswered went in at, else the count it was
/// last shown, so the reply to a prompt it just sent counts even if it
/// landed before this call, and a turn it has already read does not — or
/// the turn stopped on a prompt, which nothing but an answer moves on, or,
/// told it is not done, the job it left running ended, outcome `job_done`);
/// `waiting` — it stopped for a permission; `exit` — the process ended.
/// A wait its caller let go (`call` canceled, or see `let_go`) returns an
/// error and hands nothing over: the answer stays owed, for the notice.
pub fn wait(
    caller: &str,
    id: &str,
    until: &str,
    timeout_s: u64,
    after_turn: Option<u32>,
    call: &crate::mcp::Call,
) -> Result<Waited, String> {
    wait_for(caller, id, until, timeout_s, after_turn, call, true).map(|(waited, _)| waited)
}

/// A session's turn state and the delivery floor that goes with it, on every look:
/// text may miss its turn during a wait. In this order: `retried` has set the floor
/// before a helper can read as anything but busy (`background::turn_over`), so a
/// state past that turn is never paired with a floor from before it.
fn state_then_floor(
    key: &(String, String),
    state: impl FnOnce() -> Option<crate::activity::TurnState>,
) -> (Option<crate::activity::TurnState>, u32) {
    let ts = state();
    let floor = reg().lock().unwrap_or_else(|e| e.into_inner()).delivery_floors.get(key).copied();
    (ts, floor.unwrap_or(0))
}

/// The wait itself, and whether what it returns answers what was asked. With
/// `alone` that answer is handed over here, the reply cut to fit a result; a
/// `wait_many` hands over and cuts for all its sessions at once, when it has
/// its response (`alone` false).
fn wait_for(
    caller: &str,
    id: &str,
    until: &str,
    timeout_s: u64,
    after_turn: Option<u32>,
    call: &crate::mcp::Call,
    alone: bool,
) -> Result<(Waited, bool), String> {
    if !matches!(until, "turn_done" | "waiting" | "exit") {
        return Err(format!("unknown until {until:?}; use turn_done, waiting or exit"));
    }
    let known = authorize_view(caller, id)?;
    let _out = WaitOut::start(caller, id);
    let key = (caller.to_string(), id.to_string());
    let holding = let_go(caller);
    let gone = || call.canceled() || holding() == Holding::No;
    let gone_err = || -> Result<(Waited, bool), String> { Err(LET_GO.into()) };
    let pty = crate::pty::manager();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_s.clamp(1, 1800));
    // A first turn lost is all there is to wait for, and what Luna holds of the
    // caller's stays for it (`deliver_held`), so nothing below would ever see it through.
    // An `exit` waits for the session itself.
    let lost = || (until == "turn_done" || !live(pty, id)) && still_all_lost(pty, caller, id) && holding() != Holding::Unsure;
    // Text Luna holds for a person's draft goes in first; the turn to wait
    // for is the one it starts, past whatever the person sent meanwhile. So does
    // what carries it on after a core stopped its turn.
    while is_held(caller, id) || resume_pending(id) {
        if gone() {
            return gone_err();
        }
        if lost() {
            return lost_answer(caller, id, call, alone);
        }
        if std::time::Instant::now() >= deadline {
            let (state, ask) = state_of(id);
            let turns_ended = crate::activity::turn_state(id).map_or(0, |t| t.turns_ended);
            let screen = screen_of(pty, id);
            return Ok((
                Waited {
                    outcome: "timeout",
                    state,
                    ask,
                    turns_ended,
                    reply: None,
                    failed: None,
                    transcript_missing: false,
                    screen,
                    undelivered: None,
                    job_debt: None,
                },
                false,
            ));
        }
        std::thread::sleep(POLL);
    }
    let now = crate::activity::turn_state(id).map(|t| t.turns_ended);
    let (owed_since, answered_at, queued, late, told_debt) = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let owed = r.owed.get(&key);
        (
            owed.map(|o| o.baseline),
            r.answered.get(&key).copied(),
            owed.is_some_and(|o| o.queued),
            owed.is_some_and(|o| o.late),
            owed.filter(|o| o.told_pending).map(|o| o.rev),
        )
    };
    let after_turn = if late { None } else { after_turn };
    // Told it is not done, a wait is for what a notice would bring next: the
    // job's end or a turn no notice started. A count asked for is that count.
    let told_debt = told_debt.filter(|_| after_turn.is_none());
    let told_pending = told_debt.is_some();
    // A count shown since the question (a read of part of the answer, a
    // timeout) does not make the answer seen.
    let mut baseline = match (after_turn, owed_since.or_else(|| last_shown(caller, id)), now) {
        (Some(n), _, _) => n,
        // A count the session is behind has been left over from before it
        // was respawned; waiting past it would sit out the whole timeout.
        (None, Some(seen), Some(now)) => seen.min(now),
        (None, Some(seen), None) => seen,
        (None, None, now) => now.unwrap_or(0),
    };
    let mut looks = 0u32;
    // The turn count a turn end was first seen at, and when (QUEUED_GRACE_MS).
    let mut quiet: Option<(u32, u64)> = None;
    loop {
        if gone() {
            return gone_err();
        }
        if lost() {
            return lost_answer(caller, id, call, alone);
        }
        let alive = live(pty, id);
        let (ts, floor) = state_then_floor(&key, || crate::activity::turn_state(id));
        baseline = baseline.max(floor);
        let turns_ended = ts.map(|t| t.turns_ended).unwrap_or(0);
        let turn = ts.map(|t| t.turn);
        let mut ended = turns_ended > baseline && turn != Some(crate::activity::Turn::Busy);
        // A job's end that has stood its grace (`job_end_stood`).
        let mut job_done = false;
        if told_pending {
            let waits = waits_on_helpers(id, caller);
            let job = (ts.is_some_and(|t| t.children_busy) && alive) || waits;
            // What it has said since the caller last heard, as a notice would go by.
            let upward = || {
                let cursor = reg().lock().unwrap_or_else(|e| e.into_inner()).owed.get(&key).map(|o| o.cursor);
                turn != Some(crate::activity::Turn::Busy) && cursor.is_some_and(|at| news_for_parent(pty, caller, id, &known, at))
            };
            let now = verdict(
                &Owed::pending(baseline, 0),
                turns_ended,
                turn,
                job,
                real_turn_past(id, caller, baseline, turns_ended, waits, upward),
            );
            if now == Verdict::OnlyNotices {
                baseline = turns_ended;
            }
            ended = now == Verdict::TurnOver;
            job_done = job_end_stood(&key, turns_ended, now == Verdict::JobOver && alive);
        }
        if !ended {
            quiet = None;
        } else if queued {
            let since = match quiet {
                Some((n, at)) if n == turns_ended => at,
                _ => {
                    let at = now_ms();
                    quiet = Some((turns_ended, at));
                    at
                }
            };
            ended = now_ms().saturating_sub(since) >= QUEUED_GRACE_MS;
        }
        // A question in the transcript holds the turn whatever the turn
        // word says; read for it every few looks, not every one.
        looks += 1;
        let question = looks % 4 == 1 && ask_of(id).is_some_and(|a| a.blocking && a.kind == "question");
        // A turn stopped on a prompt will not end by itself: `turn_done`
        // hands it back too, unless it is the prompt the caller has just
        // answered and the sampler has not caught up with yet.
        let asking = (turn == Some(crate::activity::Turn::Waiting) || question)
            && answered_at.is_none_or(|at| now_ms().saturating_sub(at) >= ANSWERED_MS);
        let done = match until {
            "exit" => !alive,
            "waiting" => turn == Some(crate::activity::Turn::Waiting) || question || !alive,
            _ => ended || job_done || asking || !alive,
        };
        if done && holding() != Holding::Unsure {
            let outcome = if !alive {
                "exit"
            } else if until == "waiting" || (asking && !ended && !job_done) {
                "waiting"
            } else if job_done && !ended {
                "job_done"
            } else {
                "turn_done"
            };
            // A turn it answered in before it exited is handed over too.
            let says = outcome == "turn_done" || (outcome == "exit" && turns_ended > baseline);
            // A job's end is what the caller was owed, with no turn to wait
            // for the words of: what it said since is there already.
            let answers = says || outcome == "job_done";
            // Read from where the caller's question found the transcript, once
            // a move to another is seen to (`moved_to`).
            transcript_path(pty, id);
            let (asked_at, debt_cut) = {
                let r = reg().lock().unwrap_or_else(|e| e.into_inner());
                r.owed.get(&key).map_or((None, false), |o| (Some(o.cursor), o.cut_short))
            };
            // A core's going stopped the turn (the helper's word or the debt's, a
            // chat brought back): as the notice says it, and with what it said.
            let cut = says && (crate::background::cut_short(id) || debt_cut);
            let failure = || {
                (answers || outcome == "exit")
                    .then(|| failure_of(pty, id, known.provider))
                    .flatten()
                    .or_else(|| cut.then(|| "it did NOT finish its turn: Luna's core restarted while it worked and stopped it".to_string()))
            };
            let mut failed = failure();
            // A turn that stopped on an error has said all it will.
            let reply_now = |stopped: bool| reply_of(pty, id, known.provider, asked_at, alone, stopped);
            let mut reply = if outcome == "job_done" {
                asked_at.and_then(|at| reply_text(id, &said_since(pty, id, known.provider, at), alone))
            } else if says {
                reply_now(failed.is_some())
            } else {
                None
            };
            let late = std::time::Instant::now() + std::time::Duration::from_millis(REPLY_GRACE_MS);
            while says && reply.is_none() && std::time::Instant::now() < late && !gone() {
                std::thread::sleep(POLL);
                failed = failed.or_else(failure);
                reply = reply_now(failed.is_some());
            }
            // In doubt again after the grace, it is the notice's to bring.
            if gone() || (answers && holding() == Holding::Unsure) {
                return gone_err();
            }
            let failed = failed.or_else(failure);
            let transcript_missing = says && reply.is_none() && !transcript_found(pty, id);
            let bare = outcome == "turn_done" && (reply.is_none() || failed.is_some());
            let screen = (outcome == "waiting" || bare).then(|| screen_of(pty, id)).flatten();
            // Ahead of the hand-over, which takes it off.
            let undelivered = if answers { undelivered_note(caller, id) } else { None };
            let job_debt = told_debt.filter(|_| outcome == "job_done");
            if !answers {
                // Nothing was handed over: whatever is owed stays owed.
                looked(id);
            } else if alone && !call.commit(hand_over(caller, id, turns_ended, job_debt)) {
                // A cancel since the last look: it stays owed after all.
                return gone_err();
            }
            let (state, ask) = state_of(id);
            return Ok((
                Waited { outcome, state, ask, turns_ended, reply, failed, transcript_missing, screen, undelivered, job_debt },
                answers,
            ));
        }
        if std::time::Instant::now() >= deadline {
            let screen = screen_of(pty, id);
            // Nothing was handed over: whatever is owed stays owed, and the
            // next wait still waits for it.
            looked(id);
            let (state, ask) = state_of(id);
            return Ok((
                Waited {
                    outcome: "timeout",
                    state,
                    ask,
                    turns_ended,
                    reply: None,
                    failed: None,
                    transcript_missing: false,
                    screen,
                    undelivered: None,
                    job_debt: None,
                },
                false,
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

/// What a `wait` on several sessions says of one of them.
#[derive(Serialize)]
pub struct WaitedOne {
    pub id: String,
    #[serde(flatten)]
    pub waited: Waited,
}

#[derive(Serialize)]
pub struct WaitedMany {
    /// `finished`, or `timeout` when the time ran out first.
    pub outcome: &'static str,
    /// The sessions that got to where they were waited for, in the order
    /// asked for: what they say is in `results`, handed over as by one wait.
    pub finished: Vec<String>,
    pub results: Vec<WaitedOne>,
}

/// `wait` on several sessions at once, each by a `wait` of its own in a thread
/// of its own, so that what holds a notice back, what a reply covers and what
/// settles a debt are one wait's for each. `all` has them go on until each has
/// finished or the time is up; otherwise the first to finish stops the others,
/// which come back `pending` with whatever they are owed still owed.
///
/// Nothing is handed over until the response is: a session that has finished
/// stays owed, reserved from the notice (`Reserved`), so a cancel, a caller found
/// to have let the call go (`let_go`, which any of the waits may be the one to
/// see) or a core going meanwhile leaves its answer for the notice to bring.
/// Once the response is made they are all handed over together, and a cancel
/// crossing it undoes them together.
pub fn wait_many(
    caller: &str,
    ids: &[String],
    all: bool,
    until: &str,
    timeout_s: u64,
    call: &crate::mcp::Call,
) -> Result<WaitedMany, String> {
    if !matches!(until, "turn_done" | "waiting" | "exit") {
        return Err(format!("unknown until {until:?}; use turn_done, waiting or exit"));
    }
    let mut unique: Vec<&str> = Vec::new();
    for id in ids {
        if !unique.contains(&id.as_str()) {
            unique.push(id);
        }
    }
    if unique.is_empty() {
        return Err("ids is empty".into());
    }
    for id in &unique {
        authorize_view(caller, id)?;
    }
    let _reserved: Vec<Reserved> = unique.iter().map(|id| Reserved::start(caller, id)).collect();
    let calls: Vec<crate::mcp::Call> = unique.iter().map(|_| crate::mcp::Call::new()).collect();
    let mut got: Vec<Option<Result<(Waited, bool), String>>> = unique.iter().map(|_| None).collect();
    let mut let_go_seen = false;
    let mut no_thread: Option<String> = None;
    std::thread::scope(|scope| {
        let (tx, rx) = std::sync::mpsc::channel();
        for (i, id) in unique.iter().copied().enumerate() {
            let (tx, one) = (tx.clone(), &calls[i]);
            let thread = std::thread::Builder::new().name(format!("luna-wait-{id}")).spawn_scoped(scope, move || {
                let _ = tx.send((i, wait_for(caller, id, until, timeout_s, None, one, false)));
            });
            if let Err(e) = thread {
                no_thread = Some(e.to_string());
            }
        }
        drop(tx);
        let mut stopped = no_thread.is_some();
        if stopped {
            calls.iter().for_each(|c| c.cancel());
        }
        // When the first of `any` came.
        let mut first_at: Option<std::time::Instant> = None;
        loop {
            match rx.recv_timeout(std::time::Duration::from_millis(200)) {
                Ok((i, r)) => {
                    // Not stopped by this call: the caller has let it go.
                    let_go_seen |= !stopped && r.as_ref().is_err_and(|e| e == LET_GO);
                    got[i] = Some(r);
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
            if !all && first_at.is_none() && got.iter().flatten().any(|r| r.as_ref().is_ok_and(|(w, _)| w.outcome != "timeout")) {
                first_at = Some(std::time::Instant::now());
            }
            let enough = first_at.is_some_and(|at| at.elapsed() >= SAME_LOOK);
            if !stopped && (enough || let_go_seen || call.canceled()) {
                stopped = true;
                calls.iter().for_each(|c| c.cancel());
            }
        }
    });
    if let Some(e) = no_thread {
        return Err(e);
    }
    if let_go_seen || call.canceled() {
        return Err(LET_GO.into());
    }
    // (session, the count it answered by, the debt a job's end settles), for
    // the hand-over below.
    let mut answers: Vec<(String, u32, Option<u64>)> = Vec::new();
    let mut results: Vec<WaitedOne> = unique
        .iter()
        .copied()
        .zip(got)
        .map(|(id, r)| {
            let waited = match r {
                Some(Ok((w, answered))) => {
                    if answered {
                        answers.push((id.to_string(), w.turns_ended, w.job_debt));
                    }
                    w
                }
                other => {
                    // Stopped for another's answer, nothing handed over. Any
                    // other error is a session gone since it was looked up.
                    let stopped = matches!(&other, Some(Err(e)) if e == LET_GO);
                    let (state, ask) = state_of(id);
                    let turns_ended = crate::activity::turn_state(id).map_or(0, |t| t.turns_ended);
                    looked(id);
                    Waited {
                        outcome: if stopped { "pending" } else { "exit" },
                        state,
                        ask,
                        turns_ended,
                        reply: None,
                        failed: None,
                        transcript_missing: false,
                        screen: None,
                        undelivered: None,
                        job_debt: None,
                    }
                }
            };
            WaitedOne { id: id.to_string(), waited }
        })
        .collect();
    share_replies(&mut results);
    let finished: Vec<String> =
        results.iter().filter(|r| !matches!(r.waited.outcome, "timeout" | "pending")).map(|r| r.id.clone()).collect();
    let outcome = if !finished.is_empty() && (!all || finished.len() == results.len()) { "finished" } else { "timeout" };
    let undo: Vec<_> = answers.iter().map(|(id, turns_ended, job_debt)| hand_over(caller, id, *turns_ended, *job_debt)).collect();
    if !call.commit(move || undo.into_iter().for_each(|u| u())) {
        return Err(LET_GO.into());
    }
    Ok(WaitedMany { outcome, finished, results })
}

/// Holds several replies, whole as `reply_of` gives them to a `wait_many`, to
/// what one tool result carries (MAX_TEXT): the short ones whole, the rest cut to
/// an even share of what is left, each naming a file with the whole of it, as a
/// reply cut by itself does.
fn share_replies(results: &mut [WaitedOne]) {
    let mut order: Vec<usize> = (0..results.len()).filter(|&i| results[i].waited.reply.is_some()).collect();
    let length = |r: &WaitedOne| r.waited.reply.as_ref().map_or(0, |t| t.chars().count());
    order.sort_by_key(|&i| length(&results[i]));
    let mut left = MAX_TEXT;
    let mut others = order.len();
    for i in order {
        let share = left / others;
        others -= 1;
        let n = length(&results[i]);
        if n <= share {
            left -= n;
            continue;
        }
        left -= share;
        let r = &mut results[i];
        if let Some(whole) = r.waited.reply.take() {
            let head: String = whole.chars().take(share).collect();
            r.waited.reply = Some(format!(
                "{head}\n\n[cut at {share} characters, several sessions sharing this result; {}]",
                answer_file(&r.id, &Said::one_turn(vec![whole]))
            ));
        }
    }
}

/// Whether the call a `wait` by `caller` answers has been let go without a
/// cancel saying so: the caller is gone, or — a Codex caller, held in the
/// tool call for as long as it lasts — the turn that made the call is over
/// (an interrupt) and no other has begun, for LET_GO_MS on end: a sample
/// read just before the call's turn began, and published after it, shows
/// the turn before over at a count past the one the call found; the next
/// sample finds the caller busy in this one. Until then it is `Unsure`.
/// Claude Code moves a long call to the background and may end its turn
/// while the call runs on, and sends a cancel when it drops one.
fn let_go(caller: &str) -> impl Fn() -> Holding {
    let caller = caller.to_string();
    let codex = reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(&caller).is_some_and(|k| k.provider == Provider::Codex);
    let turns = crate::activity::turn_state(&caller).map(|t| t.turns_ended);
    // Since when the caller's turn has looked over.
    let over_since = std::cell::Cell::new(None::<u64>);
    move || {
        if caller == DEV_CALLER {
            return Holding::Yes;
        }
        if !live(crate::pty::manager(), &caller) {
            return Holding::No;
        }
        let over = codex
            && turns.is_some_and(|n| {
                crate::activity::turn_state(&caller).is_some_and(|t| t.turns_ended > n && t.turn == crate::activity::Turn::Idle)
            });
        if !over {
            over_since.set(None);
            return Holding::Yes;
        }
        let since = over_since.get().unwrap_or_else(now_ms);
        over_since.set(Some(since));
        if now_ms().saturating_sub(since) >= LET_GO_MS {
            Holding::No
        } else {
            Holding::Unsure
        }
    }
}

/// Whether a caller still holds the `wait` it made (see `let_go`).
#[derive(Clone, Copy, PartialEq)]
enum Holding {
    Yes,
    /// It may have let it go: nothing is handed over until that is known.
    Unsure,
    No,
}

/// What a `wait` hands back: all the session said past `asked_at`, where
/// the caller's question found its transcript, report first (`answer_text`);
/// with no question out, its last message. None when it said nothing, or the
/// prompt is the last word of a turn that has not `stopped` short
/// (`own_words`). `cut` as in `reply_text`.
fn reply_of(pty: &crate::pty::PtyManager, id: &str, provider: Provider, asked_at: Option<u64>, cut: bool, stopped: bool) -> Option<String> {
    let replies = match asked_at {
        Some(at) => replies_since(pty, id, provider, at, stopped),
        None => Said::one_turn(replies_since(pty, id, provider, 0, false).texts.pop().into_iter().collect()),
    };
    reply_text(id, &replies, cut)
}

/// Messages as one reply in a tool result; None for no messages. `cut` keeps it
/// to what one result carries, naming a file with the whole; a `wait_many`
/// shares that out itself (`share_replies`).
pub(super) fn reply_text(id: &str, replies: &Said, cut: bool) -> Option<String> {
    if replies.is_empty() {
        return None;
    }
    let text = answer_text(replies);
    if !cut || text.chars().count() <= MAX_TEXT {
        return Some(text);
    }
    let head: String = text.chars().take(MAX_TEXT).collect();
    Some(format!("{head}\n\n[cut at {MAX_TEXT} characters; {}]", answer_file(id, replies)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::owed::{hand_over, last_shown, owe_from, retried, shown, Owed};
    use crate::agents::read::{transcripts, MAX_TEXT};
    use crate::agents::registry::{reg, register};
    use crate::agents::teardown::forget;
    use crate::agents::{held_for_wait, ties_of, BACKGROUND_GRACE_MS, WAIT_HOLD_MS};

    /// A caller and two background helpers of its, one whose answer is on
    /// disk and owed, the other in the middle of a turn.
    fn several(prefix: &str) -> (String, String, String, std::path::PathBuf) {
        let none = serde_json::json!({});
        let dir = std::env::temp_dir().join(format!("luna-wait-many-{prefix}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let account = dir.to_string_lossy().into_owned();
        let (caller, done, busy) = (format!("{prefix}-caller"), format!("{prefix}-done"), format!("{prefix}-busy"));
        register(&caller, Provider::Claude, &account, &account, None, true, none.clone());
        crate::background::test_helper(&caller, &account, None, false);
        for (id, sid, working) in [(&done, "sid-done", false), (&busy, "sid-busy", true)] {
            register(id, Provider::Claude, &account, &account, Some(&caller), false, none.clone());
            crate::background::test_helper(id, &account, Some(sid), working);
            let path = crate::claude::session::transcript_file(&account, &account, sid);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, format!("{}\n", serde_json::json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": format!("answer of {sid}") }] } }))).unwrap();
            owe_from(&caller, id, 0, 0);
        }
        crate::background::test_turn_ended(&done);
        (caller, done, busy, dir)
    }

    fn leave(ids: &[&String], dir: &std::path::Path) {
        for id in ids {
            crate::background::remove(id);
            forget(id);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    fn owed_to(caller: &str, id: &str) -> bool {
        reg().lock().unwrap().owed.contains_key(&(caller.to_string(), id.to_string()))
    }

    #[test]
    fn waiting_on_any_hands_over_the_first_and_leaves_the_rest_owed() {
        let (caller, done, busy, dir) = several("any");
        let call = crate::mcp::Call::new();
        let w = wait_many(&caller, &[done.clone(), busy.clone(), done.clone()], false, "turn_done", 20, &call).unwrap();
        assert_eq!(w.outcome, "finished");
        assert_eq!(w.finished, std::slice::from_ref(&done));
        assert_eq!(w.results.len(), 2, "an id asked for twice is one");
        assert_eq!(w.results[0].waited.outcome, "turn_done");
        assert_eq!(w.results[0].waited.reply.as_deref(), Some("answer of sid-done"));
        assert_eq!(w.results[1].waited.outcome, "pending");
        assert_eq!(w.results[1].waited.reply, None);
        assert_eq!(w.results[1].waited.state, "working");
        assert!(!owed_to(&caller, &done), "handed over: no notice follows");
        assert!(owed_to(&caller, &busy), "the other is still owed");
        let v = serde_json::to_value(&w).unwrap();
        assert_eq!(v["results"][0]["id"], serde_json::json!(done));
        assert_eq!(v["results"][0]["turnsEnded"], 1, "the shape one wait returns");

        // Two done at once: neither is passed over for the other.
        crate::background::test_turn_ended(&busy);
        owe_from(&caller, &done, 1, 0);
        crate::background::test_helper(&done, &dir.to_string_lossy(), Some("sid-done"), false);
        crate::background::test_turn_ended(&done);
        crate::background::test_turn_ended(&done);
        let call = crate::mcp::Call::new();
        let w = wait_many(&caller, &[done.clone(), busy.clone()], false, "turn_done", 20, &call).unwrap();
        assert_eq!(w.finished, [done.clone(), busy.clone()]);
        assert!(!owed_to(&caller, &done) && !owed_to(&caller, &busy));
        leave(&[&caller, &done, &busy], &dir);
    }

    #[test]
    fn waiting_on_all_goes_on_until_each_has_finished_or_the_time_is_up() {
        let (caller, done, busy, dir) = several("all");
        let ids = [done.clone(), busy.clone()];

        // While the other runs, what has come is still owed, as a core going
        // now would save it, and held from the notice.
        let call = std::sync::Arc::new(crate::mcp::Call::new());
        let waiting = {
            let (caller, ids, call) = (caller.clone(), ids.clone(), call.clone());
            std::thread::spawn(move || wait_many(&caller, &ids, true, "turn_done", 3, &call).is_ok())
        };
        std::thread::sleep(std::time::Duration::from_millis(1500));
        assert!(owed_to(&caller, &done));
        assert_eq!(ties_of(&done).unwrap().owed.len(), 1, "saved with the helper");
        assert!(last_shown(&caller, &done) != Some(1), "not seen yet either");
        let o = reg().lock().unwrap().owed.get(&(caller.clone(), done.clone())).copied().unwrap();
        reg().lock().unwrap().owed.get_mut(&(caller.clone(), done.clone())).unwrap().ready_at = Some(now_ms() - WAIT_HOLD_MS - 1);
        assert!(held_for_wait(crate::pty::manager(), &caller, &done, &o), "past the hold of a wait on one");
        assert!(waiting.join().unwrap());
        assert!(!owed_to(&caller, &done), "handed over with the response");
        reg().lock().unwrap().seen.remove(&(caller.clone(), done.clone()));
        crate::background::test_helper(&done, &dir.to_string_lossy(), Some("sid-done"), false);
        crate::background::test_turn_ended(&done);
        owe_from(&caller, &done, 0, 0);

        // The time runs out on one: what came is handed over, the other is
        // a timeout with its screen, still owed.
        let call = crate::mcp::Call::new();
        let w = wait_many(&caller, &ids, true, "turn_done", 1, &call).unwrap();
        assert_eq!(w.outcome, "timeout");
        assert_eq!(w.finished, std::slice::from_ref(&done));
        assert_eq!(w.results[0].waited.reply.as_deref(), Some("answer of sid-done"));
        assert_eq!(w.results[1].waited.outcome, "timeout");
        assert!(!owed_to(&caller, &done));
        assert!(owed_to(&caller, &busy));

        // The other ends a little later: the one already seen does not count
        // again, so the wait is for the turn still running.
        let (end, id) = (busy.clone(), busy.clone());
        crate::background::test_helper(&id, &dir.to_string_lossy(), Some("sid-busy"), true);
        let ender = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(1500));
            crate::background::test_turn_ended(&end);
        });
        crate::background::test_helper(&done, &dir.to_string_lossy(), Some("sid-done"), false);
        crate::background::test_turn_ended(&done);
        reg().lock().unwrap().seen.remove(&(caller.clone(), done.clone()));
        owe_from(&caller, &done, 0, 0);
        let call = crate::mcp::Call::new();
        let w = wait_many(&caller, &ids, true, "turn_done", 20, &call).unwrap();
        ender.join().unwrap();
        assert_eq!(w.outcome, "finished");
        assert_eq!(w.finished, ids);
        assert!(w.results.iter().all(|r| r.waited.outcome == "turn_done" && r.waited.reply.is_some()));
        assert!(!owed_to(&caller, &done) && !owed_to(&caller, &busy));
        leave(&[&caller, &done, &busy], &dir);
    }

    #[test]
    fn a_wait_on_several_that_is_cancelled_hands_nothing_over() {
        // Before it answers: the hand-over made while another ran is undone.
        let (caller, done, busy, dir) = several("cxl");
        let ids = [done.clone(), busy.clone()];
        let call = std::sync::Arc::new(crate::mcp::Call::new());
        let canceler = call.clone();
        let canceling = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(1500));
            canceler.cancel();
        });
        assert!(wait_many(&caller, &ids, true, "turn_done", 20, &call).is_err());
        canceling.join().unwrap();
        assert!(owed_to(&caller, &done), "still owed, for the notice");
        assert!(owed_to(&caller, &busy));
        assert!(last_shown(&caller, &done).is_none());

        // Crossing the response: the same, after it has been handed over.
        let call = crate::mcp::Call::new();
        let w = wait_many(&caller, &ids, false, "turn_done", 20, &call).unwrap();
        assert_eq!(w.finished, std::slice::from_ref(&done));
        assert!(!owed_to(&caller, &done));
        call.cancel();
        assert!(owed_to(&caller, &done), "dropped unread: owed again");

        // Another call collects it during the wait, which is then canceled:
        // nothing comes back, as nothing was the wait's to take.
        let canceled = std::sync::Arc::new(crate::mcp::Call::new());
        let waiting = {
            let (caller, ids, call) = (caller.clone(), ids.clone(), canceled.clone());
            std::thread::spawn(move || wait_many(&caller, &ids, true, "turn_done", 20, &call).is_err())
        };
        std::thread::sleep(std::time::Duration::from_millis(1200));
        let _read = hand_over(&caller, &done, 1, None);
        canceled.cancel();
        assert!(waiting.join().unwrap());
        assert!(!owed_to(&caller, &done), "the read delivered it");
        leave(&[&caller, &done, &busy], &dir);
    }

    #[test]
    fn a_long_answer_is_cut_once_and_its_file_has_the_whole() {
        let (caller, done, busy, dir) = several("long");
        let account = dir.to_string_lossy().into_owned();
        let long = "z".repeat(MAX_TEXT + 500);
        let path = crate::claude::session::transcript_file(&account, &account, "sid-done");
        std::fs::write(
            &path,
            format!("{}\n", serde_json::json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": long }] } })),
        )
        .unwrap();
        crate::background::test_turn_ended(&busy);
        let call = crate::mcp::Call::new();
        let w = wait_many(&caller, std::slice::from_ref(&done), false, "turn_done", 20, &call).unwrap();
        let reply = w.results[0].waited.reply.as_ref().unwrap();
        assert_eq!(reply.matches("[cut at").count(), 1, "once");
        let file = reply.rsplit("the answer is in ").next().unwrap().trim_end_matches(", its report first]");
        assert_eq!(std::fs::read_to_string(file).unwrap(), long, "the whole of it");
        let _ = std::fs::remove_file(file);
        leave(&[&caller, &done, &busy], &dir);
    }

    #[test]
    fn a_wait_on_several_is_refused_whole() {
        let none = serde_json::json!({});
        register("mw-me", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none);
        let call = crate::mcp::Call::new();
        assert!(wait_many("mw-me", &[], false, "turn_done", 1, &call).err().unwrap().contains("empty"));
        assert!(wait_many("mw-me", &["nobody".into()], false, "turn_done", 1, &call).err().unwrap().contains("unknown session"));
        assert!(wait_many("mw-me", &["mw-me".into()], false, "turn_done", 1, &call).err().unwrap().contains("your own"));
        assert!(wait_many("mw-me", &["x".into()], false, "soon", 1, &call).err().unwrap().contains("unknown until"));
        forget("mw-me");
    }

    #[test]
    fn several_long_replies_share_one_result() {
        let one = |id: &str, reply: Option<String>| WaitedOne {
            id: id.into(),
            waited: Waited {
                outcome: "turn_done",
                state: "idle",
                ask: None,
                turns_ended: 1,
                reply,
                failed: None,
                transcript_missing: false,
                screen: None,
                undelivered: None,
                job_debt: None,
            },
        };
        let mut results = [
            one("s-short", Some("short".into())),
            one("s-none", None),
            one("s-long", Some("x".repeat(MAX_TEXT))),
            one("s-long2", Some("y".repeat(MAX_TEXT))),
        ];
        share_replies(&mut results);
        assert_eq!(results[0].waited.reply.as_deref(), Some("short"), "room left for the short one whole");
        assert_eq!(results[1].waited.reply, None);
        for r in &results[2..] {
            let text = r.waited.reply.as_ref().unwrap();
            assert!(text.contains("several sessions sharing this result"), "{}", &text[text.len() - 80..]);
            let file = text.rsplit("the answer is in ").next().unwrap().trim_end_matches(", its report first]");
            assert_eq!(std::fs::read_to_string(file).unwrap().chars().count(), MAX_TEXT, "the whole of it");
            let _ = std::fs::remove_file(file);
        }
        let total: usize = results.iter().filter_map(|r| r.waited.reply.as_ref()).map(|t| t.chars().count()).sum();
        assert!(total < MAX_TEXT + 1000, "{total}");

        let mut small = [one("s-a", Some("a".repeat(100))), one("s-b", Some("b".repeat(100)))];
        share_replies(&mut small);
        assert_eq!(small[0].waited.reply.as_ref().unwrap().len(), 100, "what fits is left alone");
    }

    #[test]
    fn a_wait_on_several_hands_a_jobs_end_over_with_its_report() {
        let (caller, done, busy, dir) = several("jd");
        let account = dir.to_string_lossy().into_owned();
        let path = crate::claude::session::transcript_file(&account, &account, "sid-done");
        let before = "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"Started the job.\"}]}}\n";
        let after = "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"The real results.\"}]}}\n";
        std::fs::write(&path, format!("{before}{after}")).unwrap();
        // Told it is not done, its job over, the grace stood: `busy` still works.
        let key = (caller.clone(), done.clone());
        let turns = crate::activity::turn_state(&done).map_or(0, |t| t.turns_ended);
        let told = Owed::pending(turns, before.len() as u64);
        {
            let mut r = reg().lock().unwrap();
            r.owed.insert(key.clone(), told);
            r.owed.get_mut(&key).unwrap().held = Some((turns, now_ms() - BACKGROUND_GRACE_MS));
        }
        let call = crate::mcp::Call::new();
        let w = wait_many(&caller, &[done.clone(), busy.clone()], false, "turn_done", 20, &call).unwrap();
        assert_eq!(w.finished, std::slice::from_ref(&done));
        let one = &w.results[0].waited;
        assert_eq!(one.outcome, "job_done");
        assert_eq!(one.reply.as_deref(), Some("The real results."), "what it said since, not an old message");
        assert!(!owed_to(&caller, &done), "the job's end settles the debt it ended");
        assert_eq!(w.results[1].waited.outcome, "pending");
        leave(&[&caller, &done, &busy], &dir);
    }

    #[test]
    fn a_wait_that_takes_a_first_answer_leaves_the_rest_owed() {
        let none = serde_json::json!({});
        register("cw-top", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone());
        register("cw-mid", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("cw-top"), true, none.clone());
        register("cw-low", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("cw-mid"), false, none);
        let key = ("cw-top".to_string(), "cw-mid".to_string());
        owe_from("cw-top", "cw-mid", 0, 0);
        owe_from("cw-mid", "cw-low", 0, 0);
        shown("cw-top", "cw-mid", 1);
        let o = reg().lock().unwrap().owed.get(&key).copied().expect("what it makes of its helper's answer is still owed");
        assert!(o.told_pending);
        assert_eq!(o.baseline, 1);
        // With no helper out, a turn past the question settles it, as ever.
        reg().lock().unwrap().owed.remove(&("cw-mid".to_string(), "cw-low".to_string()));
        shown("cw-top", "cw-mid", 2);
        assert!(!reg().lock().unwrap().owed.contains_key(&key));
        for id in ["cw-low", "cw-mid", "cw-top"] {
            forget(id);
        }
    }

    /// A wait on a helper a restart cut off hands over what it said, as its notice
    /// would, and says it did not finish: with no error to go by and a prompt as
    /// the last word, it once handed back nothing and settled the debt.
    #[test]
    fn a_wait_on_a_turn_a_restart_cut_short_keeps_what_it_said() {
        let none = serde_json::json!({});
        let dir = std::env::temp_dir().join(format!("luna-wait-cut-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let account = dir.to_string_lossy().into_owned();
        register("wc-caller", Provider::Claude, &account, &account, None, true, none.clone());
        crate::background::test_helper("wc-caller", &account, None, false);
        register("wc-helper", Provider::Claude, &account, &account, Some("wc-caller"), false, none);
        crate::background::test_helper("wc-helper", &account, None, false);
        let line = |role: &str, t: &str| match role {
            "user" => format!("{}\n", serde_json::json!({ "type": "user", "message": { "role": "user", "content": t } })),
            _ => format!("{}\n", serde_json::json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": t }] } })),
        };
        let path = dir.join("wc-helper.jsonl");
        std::fs::write(
            &path,
            format!("{}{}{}", line("user", "fix it"), line("assistant", "Half the fix is in."), line("user", "then the tests")),
        )
        .unwrap();
        transcripts().lock().unwrap().insert("wc-helper".into(), path);
        owe_from("wc-caller", "wc-helper", 0, 0);
        crate::background::test_turn_ended("wc-helper");
        crate::background::cut_for_test("wc-helper");
        let call = crate::mcp::Call::new();
        let w = wait("wc-caller", "wc-helper", "turn_done", 10, None, &call).unwrap();
        assert_eq!(w.outcome, "turn_done");
        assert_eq!(w.reply.as_deref(), Some("Half the fix is in."));
        assert!(w.failed.as_deref().is_some_and(|f| f.contains("did NOT finish")), "{:?}", w.failed);
        for id in ["wc-helper", "wc-caller"] {
            crate::background::remove(id);
            forget(id);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The retry lands in the middle of a wait's look: it stops on a permission at a
    /// count past `afterTurn`, which reads as a turn done with a floor from before it.
    #[test]
    fn a_wait_pairs_a_turn_state_with_the_floor_of_that_moment() {
        use crate::activity::{Turn, TurnState};
        let (a, id) = ("rs-sender", "rs-helper");
        let key = (a.to_string(), id.to_string());
        let (ts, floor) = state_then_floor(&key, || {
            retried(id, &[a.to_string()], 1);
            Some(TurnState { turn: Turn::Waiting, busy: true, turns_ended: 1, children_busy: false })
        });
        assert_eq!(ts.map(|t| t.turns_ended), Some(1));
        assert_eq!(floor, 1);
        forget(id);
    }
}
