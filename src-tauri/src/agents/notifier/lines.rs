//! The notice lines: what a notice says of a session a caller is owed by,
//! settled or standing, and the turns notices start.

use crate::agents::ask::{ask_of, live};
use crate::agents::lost::{never_started_what, sentence, undelivered_for, undelivered_line, undelivered_text};
use crate::agents::notifier::{LineEvidence, Settled};
use crate::agents::owed::{requests_through, Owed, QUEUED_GRACE_MS};
use crate::agents::persist::{core_up_ms, returning};
use crate::agents::read::{failure_of, transcript_end, transcript_found, transcript_path};
use crate::agents::registry::{is_descendant, label, reg, waits_on_helpers, Known, Registry};
use crate::agents::said::{
    answer_file, answer_text, answer_where, begins, fresh_status, inline_answer, note_file, report_span, said_since, says_status,
    written_range, Request, Said, Status,
};
use crate::agents::wait::REPLY_GRACE_MS;
use crate::provider::Provider;
use crate::throttle::now_ms;
use std::collections::{HashMap, HashSet};

#[cfg(test)]
mod debt_model;
use std::sync::{Mutex, OnceLock};

/// A turn has ended with the session's processes still at work: give them
/// this long to settle before calling it a job left in the background. Right
/// after a turn they often move for a sample or two anyway.
pub(in crate::agents) const BACKGROUND_GRACE_MS: u64 = 20_000;

/// The most turn-end notices one caller gets about one session within
/// BREAKER_WINDOW_MS before Luna stops telling it: each costs a turn of the
/// caller, and two chats that keep answering each other's notices would go on
/// until a person came back. Real use never came near it (at most 3 in half an
/// hour per pair, luna.log 10-28 to 11-01); the loop of that night ran one a
/// minute per pair, for 13 hours.
pub(in crate::agents) const BREAKER_NOTICES: usize = 12;

pub(in crate::agents) const BREAKER_WINDOW_MS: u64 = 15 * 60 * 1000;

/// The notice turns kept per session (see `NoticeTurn`): the debts that ask
/// about them are never many turns behind.
const NOTICE_TURNS_KEPT: usize = 64;

/// A turn a Luna notice started. Told a session is not done yet, a caller is
/// not told again of a turn that only answers a notice: two chats with a job
/// each would wake each other for as long as the jobs run.
pub(in crate::agents) struct NoticeTurn {
    /// The `turnsEnded` it ends at.
    pub(in crate::agents) turn: u32,
    /// Those the notice told of that were the session's own helpers then.
    helpers: Vec<String>,
    /// It told of nobody else: the turn was the session at work on what its
    /// own helpers brought, and nobody's notice came back to it.
    only_helpers: bool,
    /// It woke a lead to its delegates' end (`wake_lead`): the lead going on
    /// with what it was asked, a turn its callers hear of as one of its own.
    wake: bool,
}

/// Where a debt stands, by what the session is doing now.
#[derive(Debug, PartialEq, Clone, Copy)]
pub(in crate::agents) enum Verdict {
    /// No news of a turn or a job: the session may still be on a prompt.
    Nothing,
    /// A turn the caller is to hear of ended.
    TurnOver,
    /// Told it is not done, and only turns on Luna's own notices ended since.
    OnlyNotices,
    /// Told it is not done, and it still is not.
    AtWork,
    /// Told it is not done, and what it had running has ended with no turn.
    JobOver,
}

/// `real_turn`: a turn past the baseline counts for this caller (`real_turn_past`).
pub(in crate::agents) fn verdict(o: &Owed, ended: u32, turn: Option<crate::activity::Turn>, job: bool, real_turn: bool) -> Verdict {
    use crate::activity::Turn;
    let past = ended > o.baseline && turn != Some(Turn::Busy);
    if !o.told_pending {
        return if past { Verdict::TurnOver } else { Verdict::Nothing };
    }
    match (past, real_turn) {
        (true, true) => Verdict::TurnOver,
        (true, false) => Verdict::OnlyNotices,
        _ if turn != Some(Turn::Idle) => Verdict::Nothing,
        _ if job => Verdict::AtWork,
        _ => Verdict::JobOver,
    }
}

/// Whether a turn of `id` past `after` and up to `ended` ended that `caller`,
/// told it is not done, is to hear of: one no Luna notice started; or, once
/// `id` has all its own helpers' answers (`waits`: not yet), one on a notice
/// from them, in which it goes on with what it was asked. Not when it is
/// `caller` who is that helper: that is its own notice coming back.
///
/// A turn Luna's wake started (`wake_lead`) counts as one of its own.
///
/// `upward`: `caller` is `id`'s parent and `id` has written a status word
/// since it last heard (`news_for_parent`). Then turns on its own helpers'
/// notices reach it while others are still out, every turn since being one of
/// those: what a helper makes of its helpers' answers goes up the tree it was
/// spawned in, and nothing goes back to a session whose notice
/// started a turn. Two chats that wrote to each other are no parent and helper
/// to each other; one that is the other's parent hears only of turns on
/// notices about its helper's own helpers, and the helper never of its parent.
pub(in crate::agents) fn real_turn_past(
    id: &str,
    caller: &str,
    after: u32,
    ended: u32,
    waits: bool,
    upward: impl FnOnce() -> bool,
) -> bool {
    let turns = after.saturating_add(1)..=ended;
    let (counts, on_helpers) = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let marked = r.notice_turns.get(id);
        let mark = |turn: u32| marked.and_then(|m| m.iter().find(|m| m.turn == turn));
        let counts = turns.clone().any(|turn| match mark(turn) {
            None => true,
            Some(m) => m.wake || (!waits && m.helpers.iter().any(|h| h != caller)),
        });
        (counts, turns.clone().all(|turn| mark(turn).is_some_and(|m| m.only_helpers && !m.helpers.is_empty())))
    };
    // Read off the lock: it looks at the transcript.
    counts || (on_helpers && !turns.is_empty() && upward())
}

/// Whether every turn of `id` past `after` and up to `ended` was one on a
/// notice about its own helpers (`notice_turn`), none a wake's.
fn turns_on_helpers(id: &str, after: u32, ended: u32) -> bool {
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let marked = r.notice_turns.get(id);
    let turns = after.saturating_add(1)..=ended;
    !turns.is_empty()
        && turns.into_iter().all(|turn| {
            marked.and_then(|m| m.iter().find(|m| m.turn == turn)).is_some_and(|m| !m.wake && m.only_helpers && !m.helpers.is_empty())
        })
}

/// Whether lead `id` has written nothing since its last turn held back while
/// its delegates worked (`Owed::held_to`), `said` being what it wrote since
/// the debt's cursor: what it wrote before then, interim words the caller
/// has not had yet, is no word on its delegates' end.
fn quiet_since_held(pty: &crate::pty::PtyManager, id: &str, k: &Known, o: &Owed, said: &Said) -> bool {
    match o.held_to {
        Some(at) => said_since(pty, id, k.provider, at).is_empty(),
        None => said.is_empty(),
    }
}

/// Whether lead `id`, its delegates all done, has said nothing of them in
/// its turns since: its last, ending at `ended` and on its own helpers'
/// notices, wrote nothing (`silent`); or every turn since its caller was told
/// it is not done was on those notices, and none wrote past `held_to`. What
/// it wrote before them waited on its delegates. Not whether a turn of it
/// was held back from the caller first: a helper that answers within the
/// reply grace of its lead's interim turn ends before the notifier has looked
/// at that turn, and the lead's turn on its notice is all that tells.
fn quiet_on_helpers(pty: &crate::pty::PtyManager, id: &str, k: &Known, o: &Owed, ended: u32, silent: bool) -> bool {
    if !turns_on_helpers(id, ended.saturating_sub(1), ended) {
        return false;
    }
    silent
        || ((o.told_pending || o.wake.skipped.is_some())
            && turns_on_helpers(id, o.notice_baseline(), ended)
            && o.held_to.is_some_and(|at| said_since(pty, id, k.provider, at).is_empty()))
}

/// For `real_turn_past`: `caller` is `id`'s parent, and `id`, a lead whose
/// own helpers are still out, has written a message since `cursor` that opens
/// on a status word (said.rs `says_status`): word that it is done, blocked or
/// asks, worth a turn of the parent's. Anything else it writes meanwhile, a
/// long interim report too, waits in its report for the next notice: four
/// leads writing "still working" woke one parent dozens of times a night.
pub(in crate::agents) fn news_for_parent(pty: &crate::pty::PtyManager, caller: &str, id: &str, k: &Known, cursor: u64) -> bool {
    k.parent.as_deref() == Some(caller) && said_since(pty, id, k.provider, cursor).iter().any(|m| says_status(m))
}

/// Whether a session is a lead: one another session spawned with Luna's tools,
/// able to start helpers of its own (`settings::Role::Lead`). While its
/// delegates work its interim turns wait for news, and once they are
/// done Luna wakes it rather than leave it waiting (`wake_lead`).
pub(in crate::agents) fn is_lead(k: &Known) -> bool {
    k.tools && k.parent.is_some()
}

/// A lead Luna has told its delegates finished: at the `turnsEnded` their end
/// was seen at, where its transcript ended then, and when. Once per such end;
/// the turn it starts reaches the lead's callers (`NoticeTurn::wake`), and if
/// that turn ends without a word they hear the lead seems stuck.
#[derive(Clone, Copy)]
pub(in crate::agents) struct Woken {
    pub(super) ended: u32,
    cursor: u64,
    at: u64,
}

/// How long a lead woken by Luna has to take that up before its callers hear
/// of its delegates' end as they would have: the wake waits, as any notice
/// does, for the lead to be between turns, and one that never goes in must
/// not keep them from hearing.
const WAKE_WAIT_MS: u64 = 10 * 60 * 1000;

impl Woken {
    pub(super) fn expired(self, now: u64) -> bool {
        now.saturating_sub(self.at) >= WAKE_WAIT_MS
    }
}

/// How a wake (`wake_line`) opens: what tells it from any other queued line
/// where its notice goes in (notifier `deliver_notice`).
pub(in crate::agents) const WAKE_HEAD: &str = "Your delegates have finished";

/// Whether a queued line is a lead's wake.
pub(in crate::agents) fn is_wake(line: &str) -> bool {
    line.starts_with(WAKE_HEAD)
}

fn wake_line(names: &[String]) -> String {
    let named = if names.is_empty() { String::new() } else { format!(": {}", names.join("; ")) };
    format!(
        "{WAKE_HEAD}{named}. Read their answers (task notifications, files or luna_report_get). This is your only wake; continue or report status."
    )
}

/// What `wake_lead` came to.
#[derive(Debug, PartialEq)]
enum Wake {
    /// Woken now, or earlier for this end and not yet past WAKE_WAIT_MS: its
    /// callers wait for the turn that brings.
    Waits,
    /// Not to be woken (the breaker has stopped it), or woken and it never
    /// took that up: its callers hear of the end as ever.
    Missed,
}

/// A lead whose delegates have all finished (its own helpers answered, what
/// its CLI ran in the background over) at `ended`, with no word from it since.
/// In the field such a lead waited on, its parent was told only that it had
/// nothing left running, and work stood still until a person sent "go on".
/// Luna types a wake into the lead itself instead, once per such end, as a
/// line no debt brings (`Registry::news`), counted by the breaker as a notice
/// from the lead to itself.
fn wake_lead(pty: &crate::pty::PtyManager, id: &str, ended: u32) -> Wake {
    if pair_stopped(pty, id, id) {
        return Wake::Missed;
    }
    let now = now_ms();
    let cursor = transcript_end(id);
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(w) = r.woken.get(id).filter(|w| w.ended == ended) {
        return if !w.expired(now) { Wake::Waits } else { Wake::Missed };
    }
    let names = r.delegates.remove(id).unwrap_or_default();
    r.news.push((id.to_string(), wake_line(&names)));
    r.woken.insert(id.to_string(), Woken { ended, cursor, at: now });
    let last = count_notice(&mut r, id, id, now);
    drop(r);
    // A queued line is kept across a core until it is told, as any is.
    crate::agents::save_chats();
    crate::log::info("agents", &format!("{} woken: its delegates finished and it has said nothing since", label(id)));
    if last {
        crate::log::warn("agents", &format!("stopped waking {}: {BREAKER_NOTICES} wakes within the breaker's window", label(id)));
    }
    Wake::Waits
}

fn wake_is_pending(id: &str, ended: u32) -> bool {
    reg().lock().unwrap_or_else(|e| e.into_inner()).woken.get(id).is_some_and(|w| w.ended == ended && !w.expired(now_ms()))
}

/// The wake of `id` before its turn ending at `ended`, if one stands: that
/// turn is its turn on the wake, or one after it.
fn woken_before(id: &str, ended: u32) -> Option<Woken> {
    reg().lock().unwrap_or_else(|e| e.into_inner()).woken.get(id).filter(|w| w.ended < ended).copied()
}

/// The most delegates a wake names; the rest are the lead's to know.
const DELEGATES_NAMED: usize = 8;

/// What lead `id` is seen waiting on now, kept for the wake that comes once
/// it is all over (`wake_lead`): its own helpers still to answer it, what its
/// CLI left in the background as its transcript names it, else the processes
/// left (`procs`).
fn note_delegates(pty: &crate::pty::PtyManager, id: &str, provider: Provider, procs: Option<&str>) {
    let mut names: Vec<String> = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        r.owed
            .iter()
            .filter(|((c, h), o)| c == id && o.active() && is_descendant(&r, id, h))
            .map(|((_, h), _)| match r.names.get(h).map(|n| n.trim()).filter(|n| !n.is_empty()) {
                Some(n) => format!("helper \"{n}\" ({h})"),
                None => format!("helper {h}"),
            })
            .collect()
    };
    names.sort();
    names.extend(background_calls(pty, id, provider));
    if names.is_empty() {
        names.extend(procs.map(|p| format!("processes {p}")));
    }
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let seen = r.delegates.entry(id.to_string()).or_default();
    for name in names {
        if seen.len() < DELEGATES_NAMED && !seen.contains(&name) {
            seen.push(name);
        }
    }
}

/// A notice about `about` went into `caller` and starts its turn ending at
/// `turn`.
/// `wake`: it carried `caller`'s own wake (`wake_lead`).
pub(in crate::agents) fn notice_turn(r: &mut Registry, caller: &str, turn: u32, about: &[String], wake: bool) {
    let helpers: Vec<String> = about.iter().filter(|a| is_descendant(r, caller, a)).cloned().collect();
    let only_helpers = helpers.len() == about.len();
    let turns = r.notice_turns.entry(caller.to_string()).or_default();
    turns.push(NoticeTurn { turn, helpers, only_helpers, wake });
    if turns.len() > NOTICE_TURNS_KEPT {
        turns.remove(0);
    }
}

/// Counts a notice to `caller` about `id`. Returns whether it was the last
/// BREAKER_NOTICES allows: the pair is stopped from here on.
pub(in crate::agents) fn count_notice(r: &mut Registry, caller: &str, id: &str, now: u64) -> bool {
    let key = (caller.to_string(), id.to_string());
    let times = r.notice_times.entry(key.clone()).or_default();
    times.retain(|at| now.saturating_sub(*at) < BREAKER_WINDOW_MS);
    times.push(now);
    let last = times.len() >= BREAKER_NOTICES;
    if last {
        r.notice_times.remove(&key);
        r.stopped.insert(key, now);
    }
    last
}

/// Whether the next notice to `caller` about `id` is the last the breaker lets
/// through, for it to say so.
pub(in crate::agents) fn last_notice(caller: &str, id: &str, now: u64) -> bool {
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    r.notice_times
        .get(&(caller.to_string(), id.to_string()))
        .is_some_and(|times| times.iter().filter(|at| now.saturating_sub(**at) < BREAKER_WINDOW_MS).count() + 1 >= BREAKER_NOTICES)
}

/// Whether Luna has stopped telling `caller` of `id`'s turns and jobs. A person
/// typing into either of them since is someone looking at it, and lifts that;
/// nothing a model does does, or two models answering each other would lift it
/// forever.
pub(in crate::agents) fn pair_stopped(pty: &crate::pty::PtyManager, caller: &str, id: &str) -> bool {
    let key = (caller.to_string(), id.to_string());
    let Some(at) = reg().lock().unwrap_or_else(|e| e.into_inner()).stopped.get(&key).copied() else { return false };
    if !breaker_lifted(pty, caller, id, at) {
        return true;
    }
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    r.stopped.remove(&key);
    r.failures_told.remove(&key);
    drop(r);
    crate::log::info("agents", &format!("notices to {} about {} go on: a person has typed in", label(caller), label(id)));
    false
}

pub(super) fn breaker_lifted(pty: &crate::pty::PtyManager, caller: &str, id: &str, at: u64) -> bool {
    let since = now_ms().saturating_sub(at);
    [caller, id].iter().any(|s| pty.typed_ago_ms(s).is_some_and(|ago| ago < since))
}

/// The notice for one session, or None while nothing has happened to it.
pub(super) fn render_notice(
    pty: &crate::pty::PtyManager,
    caller: &str,
    who: &str,
    id: &str,
    k: &Known,
    o: Owed,
    work: &mut WorkLook,
) -> Option<(Option<String>, Settled, LineEvidence)> {
    let mut evidence = LineEvidence::default();
    let (text, how) = notice_line_inner(pty, caller, who, id, k, o, &mut evidence, work)?;
    if matches!(how, Settled::Exited) {
        evidence.ended = crate::activity::turn_state(id).map_or(o.notice_baseline(), |t| t.turns_ended);
        evidence.requests = requests_through(caller, id, o, evidence.ended);
    }
    Some((text, how, evidence))
}

#[cfg(test)]
pub(in crate::agents) fn notice_line(
    pty: &crate::pty::PtyManager,
    caller: &str,
    who: &str,
    id: &str,
    k: &Known,
    o: Owed,
) -> Option<(Option<String>, Settled)> {
    render_notice(pty, caller, who, id, k, o, &mut WorkLook::default()).map(|(text, how, _)| (text, how))
}

#[allow(clippy::too_many_arguments)]
fn notice_line_inner(
    pty: &crate::pty::PtyManager,
    caller: &str,
    who: &str,
    id: &str,
    k: &Known,
    o: Owed,
    evidence: &mut LineEvidence,
    work: &mut WorkLook,
) -> Option<(Option<String>, Settled)> {
    transcript_path(pty, id);
    if reg()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .owed
        .get(&(caller.to_string(), id.to_string()))
        .is_some_and(|now| now.rev != o.rev || now.transcript_rev != o.transcript_rev)
    {
        return None;
    }
    let ts = crate::activity::turn_state(id);
    let ended = ts.map_or(0, |t| t.turns_ended);
    let turn = ts.map(|t| t.turn);
    let alive = live(pty, id);
    // A Claude Code chat ends a turn on a job it put in the background and takes
    // it up again in a turn of its own when the job is done (`claude -p` stays
    // in its turn until then, up to 3 hours for a helper's: background/
    // command.rs); for anything else that job's end is all there
    // will be to hear of.
    let background = ts.is_some_and(|t| t.children_busy) && alive;
    let left = if background { left_named(id) } else { None };
    let notice_baseline = o.notice_baseline();
    let between_turns = turn != Some(crate::activity::Turn::Busy);
    if o.wake.delivery_expired.is_some() {
        return None;
    }
    let cut = crate::background::cut_short(id) || o.cut_short;
    let unopened = between_turns && crate::background::unopened_turn(id, ended);
    let failed = (between_turns && (ended > notice_baseline || o.told_pending || o.wake.skipped.is_some()))
        .then(|| failure_of(pty, id, k.provider).or_else(|| crate::activity::limit_of(id)))
        .flatten();
    let covered_ended = ts.map_or_else(|| crate::activity::turns_counted(id).unwrap_or(0), |t| t.turns_ended);
    let covered_turn = o.active()
        || o.wake.plan_through.is_some_and(|plan| covered_ended > plan)
        || (cut || failed.is_some()) && covered_ended > notice_baseline && {
            let (words, _) = written_range(pty, id, k.provider, o.held_to.unwrap_or(o.cursor));
            let requests = requests_through(caller, id, o, covered_ended);
            words.asked_after(requests, None).last_turn_started_by_request()
        };
    let cut = cut && covered_turn;
    let failed = failed.filter(|_| covered_turn);
    let o = Owed { cut_short: o.cut_short && cut, ..o };
    if !o.active() && !o.watches_later_turns() && !cut && failed.is_none() {
        return None;
    }
    let standing = standing_line(pty, who, id, k, o);
    if o.resumed || o.job_went {
        if let Some((Some(_), Settled::Waiting | Settled::Asked(_))) = &standing {
            return standing;
        }
        let line = match (o.resumed, o.job_went) {
            (true, true) => "Luna's core restarted and cut off its turn and background work. It has resumed automatically.",
            (true, false) => "Luna's core restarted and cut off its turn. It has resumed automatically.",
            _ => "Luna's core restarted and cut off its background work.",
        };
        return Some((Some(format!("{who}: {line}")), Settled::Resumed));
    }
    if !alive && (standing.is_none() || !o.active() && o.watches_later_turns() || turn == Some(crate::activity::Turn::Waiting)) {
        return standing;
    }
    let standing = standing.filter(|(_, how)| !matches!(how, Settled::Exited));
    if standing.is_some() {
        return standing;
    }
    if unopened {
        let parent = k.parent.as_deref() == Some(caller);
        let (ids, texts): (Vec<_>, Vec<_>) = crate::background::undelivered_snapshot(id, caller, parent).into_iter().unzip();
        if !ids.is_empty() {
            let requests = requests_through(caller, id, o, ended);
            let file = note_file(id, &undelivered_text(&texts));
            let lost = undelivered_line(&texts, false, file.as_deref());
            let why = failed.as_deref().map(|why| format!(" {}", sentence(why))).unwrap_or_default();
            let (stopped, stopped_ids) = background_stopped_line(id, caller).unwrap_or_default();
            let how = failed_requests(o, ended, &requests, &ids, true);
            *evidence =
                LineEvidence { requests, lost: ids, parent, stopped: stopped_ids, ended, batch_failed: true, ..LineEvidence::default() };
            return Some((Some(format!("{who} stopped before opening its prompt.{why}{stopped}{lost} [send to resume]")), how));
        }
    }
    // The launch's error belonged to its batch. An earlier answer can now
    // go on its own, even if the failed launch was the last turn recorded.
    let failed = failed.filter(|_| !unopened);
    if turn == Some(crate::activity::Turn::Waiting) {
        return None;
    }
    let waits = waits_on_helpers(id, caller);
    let work_left = alive && (background || waits || !work.runs_nothing(pty, caller, id, ts));
    let job_debt = o.told_pending || o.wake.skipped.is_some();
    let real_turn = (job_debt || o.wake.silent.is_some()) && real_turn_past(id, caller, notice_baseline, ended, waits, || false);
    let mut now = verdict(
        &Owed { baseline: notice_baseline, told_pending: job_debt || o.wake.silent.is_some(), ..o },
        ended,
        turn,
        work_left,
        real_turn,
    );
    if !job_debt && now == Verdict::JobOver {
        now = Verdict::Nothing;
    }
    if between_turns && (alive && cut || failed.is_some() && matches!(now, Verdict::AtWork | Verdict::Nothing)) {
        now = Verdict::TurnOver;
    }
    if now == Verdict::OnlyNotices {
        let (said, read_to) = written_range(pty, id, k.provider, o.notice_cursor());
        if turn_changed(id, ended) {
            return None;
        }
        let lost = undelivered_for(id, caller, k.parent.as_deref() == Some(caller));
        if fresh_status(&said).is_some()
            || failed.is_some()
            || crate::background::stop_of(id).is_some()
            || o.cut_short
            || o.unsure
            || !lost.is_empty()
            || !background_stopped(id, caller).is_empty()
            || !transcript_found(pty, id)
            || !alive
        {
            now = Verdict::TurnOver;
        } else {
            // Keep an interim boundary only while work remains. If this
            // was the last turn on a dependency, its words can be the answer.
            return Some((None, Settled::Skip(ended, if work_left { read_to } else { o.held_to.unwrap_or(o.cursor) }, background)));
        }
    }
    if now == Verdict::AtWork && o.held.is_some() {
        if wake_is_pending(id, ended) {
            return None;
        }
        return Some((None, Settled::Unhold));
    }
    if now == Verdict::JobOver && alive {
        // Its processes come and go between samples, and a job's end may be
        // followed by the turn the CLI takes it up in.
        return match o.held {
            Some((n, at)) if n == ended && now_ms().saturating_sub(at) >= BACKGROUND_GRACE_MS => {
                // Its turns on notices may have said something since, one that
                // said nothing after them too.
                let (said, read_to) = notice_replies(pty, caller, id, k.provider, o, ended);
                if turn_changed(id, ended) {
                    return None;
                }
                let halted = crate::background::stop_of(id);
                let (stopped, stopped_ids) = background_stopped_line(id, caller).unwrap_or_default();
                // A core's going took the job, or its CLI cut it off: news, whatever was said.
                let parent = k.parent.as_deref() == Some(caller);
                let (lost_ids, lost): (Vec<_>, Vec<_>) = crate::background::undelivered_snapshot(id, caller, parent).into_iter().unzip();
                let found = transcript_found(pty, id);
                let plain = failed.is_none() && halted.is_none() && stopped.is_empty() && lost.is_empty() && !o.unsure && found;
                let status = fresh_status(&said);
                let (requests, plan, fyi) = said.notice_requests();
                let keeps_plan = fyi && o.wake.plan_through.is_some();
                let plan_covers = keeps_plan || plan_covers(o, ended);
                if plain && keeps_plan && status.is_none() && requests.iter().all(|q| q.taken) {
                    evidence.read_to = Some(read_to);
                    return Some((None, Settled::PlanFyi(ended, requests, !said.is_empty())));
                }
                // An internal wake still goes to a lead whose delegates finished.
                if plain
                    && is_lead(k)
                    && !plan_covers
                    && !plan
                    && had_delegates(id, o)
                    && quiet_since_held(pty, id, k, &o, &said)
                    && wake_lead(pty, id, ended) == Wake::Waits
                {
                    return None;
                }
                let said_since_hold = !quiet_since_held(pty, id, k, &o, &said);
                if plain && status.is_none() && (!said_since_hold || !can_stall(o, &requests, plan_covers)) {
                    evidence.read_to = Some(read_to);
                    return Some((None, silent_or_untaken(o, ended, &requests, !said.is_empty())));
                }
                let how = if plain {
                    turn_settlement(o, ended, read_to, &said, requests.clone(), status, plan || keeps_plan, false, false, false)
                } else {
                    failed_requests(o, ended, &requests, &lost_ids, false)
                };
                let quoted = if failed.is_some() { String::new() } else { begins_re(&said, o.told_pending) };
                let n = said.len();
                let messages = if n == 1 { "1 message".to_string() } else { format!("{n} messages") };
                // Short enough, what it said is the notice's, and no file.
                let said = match (n, inline_answer(&said)) {
                    (0, _) if !found => "Luna cannot find its transcript, so whether it replied is unknown".to_string(),
                    (0, _) if !stopped.is_empty() => "it has said nothing since your last notice".to_string(),
                    (0, _) => "it has said nothing since your last notice, and takes no turn on that by itself".to_string(),
                    (_, Some(line)) if said.requests().len() <= 1 => {
                        format!("it wrote {messages} since your last notice{}: \"{line}\"", answering(&said))
                    }
                    (_, _) => format!("it wrote {messages} since your last notice{}; {}{quoted}", answering(&said), answer_file(id, &said)),
                };
                let failed = match (failed, halted) {
                    (Some(why), _) => format!(" Its last turn stopped on an error: {}", sentence(&why)),
                    (None, Some(why)) => format!(" Its last turn was stopped: {}", sentence(&why)),
                    (None, None) => String::new(),
                };
                let lost = undelivered_line(&lost, n > 0, None);
                let unsure = unsure_line(o);
                *evidence = LineEvidence {
                    requests,
                    lost: lost_ids,
                    parent,
                    stopped: stopped_ids,
                    read_to: Some(read_to),
                    ended,
                    batch_failed: false,
                };
                Some((Some(format!("{who} has nothing left running now, and {said}.{stopped}{failed}{lost}{unsure}")), how))
            }
            Some((n, _)) if n == ended => None,
            _ => Some((None, Settled::Hold(ended))),
        };
    }
    if now == Verdict::TurnOver {
        let failed = failed.filter(|_| !cut);
        // Stopped by Luna (a stop, an interrupt): no error, and not finished.
        let halted = if cut { None } else { crate::background::stop_of(id) };
        // Read only new text for a notice. The tool readers also offer a
        // fallback for an old or moved cursor, which could repeat a status.
        let (replies, read_to) = notice_replies(pty, caller, id, k.provider, o, ended);
        if turn_changed(id, ended) {
            return None;
        }
        let status = fresh_status(&replies);
        let (requests, plan, fyi) = replies.notice_requests();
        let keeps_plan = fyi && o.wake.plan_through.is_some();
        let plan_covers = keeps_plan || plan_covers(o, ended);
        // The last lead turn may have made no reply even when the answer
        // includes interim text from earlier turns.
        let silent = replies.is_empty() || is_lead(k) && said_since(pty, id, k.provider, o.notice_cursor()).is_empty();
        let settle_ms = if background {
            BACKGROUND_GRACE_MS
        } else if silent {
            REPLY_GRACE_MS
        } else {
            0
        };
        // Asked mid-turn: the CLI may start a turn on the text right after.
        let settle_ms = if o.queued { settle_ms.max(QUEUED_GRACE_MS) } else { settle_ms };
        if settle_ms > 0 {
            match o.held {
                Some((n, at)) if n == ended => {
                    if now_ms().saturating_sub(at) < settle_ms {
                        return None;
                    }
                }
                _ => return Some((None, Settled::Hold(ended))),
            }
        }
        let parent = k.parent.as_deref() == Some(caller);
        let (lost_ids, lost): (Vec<_>, Vec<_>) = crate::background::undelivered_snapshot(id, caller, parent).into_iter().unzip();
        let (stopped, stopped_ids) = background_stopped_line(id, caller).unwrap_or_default();
        let found = transcript_found(pty, id);
        let never_started = parent && crate::background::never_started(id);
        let trouble =
            cut || halted.is_some() || failed.is_some() || never_started || !found || o.unsure || !lost.is_empty() || !stopped.is_empty();
        if !trouble && wake_is_pending(id, ended) {
            return None;
        }
        let failure = || failed_requests(o, ended, &requests, &lost_ids, false);
        let lead = is_lead(k);
        if !trouble && work_left {
            if lead && (background || waits) {
                note_delegates(pty, id, k.provider, left.as_deref().filter(|_| background));
            }
            if status.is_none() {
                return Some((None, Settled::Skip(ended, read_to, background)));
            }
        }
        if !trouble && !work_left {
            if keeps_plan && status.is_none() && requests.iter().all(|q| q.taken) {
                evidence.read_to = Some(read_to);
                return Some((None, Settled::PlanFyi(ended, requests, !replies.is_empty())));
            }
            if lead && !plan_covers && !plan && quiet_on_helpers(pty, id, k, &o, ended, silent) && wake_lead(pty, id, ended) == Wake::Waits
            {
                return None;
            }
            if status.is_none() && !can_stall(o, &requests, plan_covers) {
                if !alive {
                    return standing_line(pty, who, id, k, o);
                }
                evidence.read_to = Some(read_to);
                return Some((None, silent_or_untaken(o, ended, &requests, !replies.is_empty())));
            }
        }
        let text = match (replies.is_empty(), lost.is_empty()) {
            (true, true) => None,
            (true, false) => Some(undelivered_text(&lost)),
            (false, true) => Some(answer_text(&replies)),
            (false, false) => Some(format!("{}\n\n---\n\n{}", answer_text(&replies), undelivered_text(&lost))),
        };
        // All it said, short enough for the notice itself: no file to read.
        // Not answers to several requests, which the file sets apart.
        let inline =
            if lost.is_empty() && failed.is_none() && !cut && replies.requests().len() <= 1 { inline_answer(&replies) } else { None };
        let file = text.as_deref().filter(|_| inline.is_none()).and_then(|t| note_file(id, t));
        let span = report_span(&replies);
        *evidence = LineEvidence {
            requests: requests.clone(),
            lost: lost_ids.clone(),
            parent,
            stopped: stopped_ids,
            read_to: Some(read_to),
            ended,
            batch_failed: false,
        };
        let reply = match replies.len() {
            0 if !found => ", but Luna cannot find its transcript, so whether it replied is unknown".to_string(),
            0 => " without a reply".to_string(),
            n if inline.is_some() => {
                let messages = if n == 1 { "1 message".to_string() } else { format!("{n} messages") };
                format!(" with {messages} since you asked{}: \"{}\"", answering(&replies), inline.as_deref().unwrap_or_default())
            }
            n => {
                let messages = if n == 1 { "1 message".to_string() } else { format!("{n} messages") };
                format!(" with {messages} since you asked{}; {}", answering(&replies), answer_where(file.as_deref(), span))
            }
        };
        if never_started {
            let what = never_started_what(&lost, file.as_deref());
            return Some((
                Some(format!("{who} never got started: Luna's core restarted before its first turn got anywhere, so its prompt was lost and there is nothing to carry on from; {what}. luna_session_send starts it afresh.")),
                failure(),
            ));
        }
        let lost = undelivered_line(&lost, !replies.is_empty(), file.as_deref());
        let unsure = unsure_line(o);
        if cut {
            return Some((
                Some(format!("{who} did NOT finish its turn: Luna's core restarted mid-turn. It ended{reply}.{stopped}{lost}{unsure} [send to resume]")),
                failure(),
            ));
        }
        // Whatever else is going on: this turn did not get to its end.
        if let Some(why) = halted.filter(|_| failed.is_none()) {
            let said = if replies.is_empty() { String::new() } else { format!(" It ended{reply}.") };
            return Some((
                Some(format!("{who} did not finish its turn: {}{said}{stopped}{lost}{unsure} [send to resume]", sentence(&why))),
                failure(),
            ));
        }
        if let Some(why) = failed {
            let said = if replies.is_empty() { String::new() } else { format!(" It ended{reply}.") };
            return Some((
                Some(format!("{who} stopped on an error: {}{said}{stopped}{lost}{unsure} [send to resume]", sentence(&why))),
                failure(),
            ));
        }
        let reply = if inline.is_some() { reply } else { format!("{reply}{}", begins_re(&replies, o.told_pending)) };
        if !trouble && work_left {
            // Said first and plainly: a weak model read "ended a turn" as
            // done and deleted the helper mid-job. Once per debt: a job can
            // run for days, and each notice costs the caller a turn.
            let agents = crate::activity::subagents(id);
            let what = match &left {
                Some(names) if background => format!("[job running: {names}]"),
                _ if background && agents == 1 => "[1 subagent running]".to_string(),
                _ if background && agents > 1 => format!("[{agents} subagents running]"),
                _ if background => "[job running]".to_string(),
                _ => "[helpers pending]".to_string(),
            };
            let line = if o.told_pending {
                format!("{who} ended another turn{reply}. {what}")
            } else if lead {
                format!("{who} ended a turn{reply}. {what}")
            } else {
                format!("{who} is NOT done yet {what}. It ended a turn{reply}.")
            };
            return Some((
                Some(line),
                turn_settlement(
                    o,
                    ended,
                    read_to,
                    &replies,
                    requests,
                    status,
                    plan || keeps_plan,
                    work_left,
                    background,
                    background || waits,
                ),
            ));
        }
        // The internal wake has had its chance to get this lead moving.
        if lead && status.is_none() && (can_stall(o, &requests, plan_covers) || trouble) {
            if let Some(w) = woken_before(id, ended) {
                if said_since(pty, id, k.provider, w.cursor).is_empty() {
                    let earlier = if replies.is_empty() { String::new() } else { format!(" Its turns since you asked ended{reply}.") };
                    return Some((
                        Some(format!("{who} seems stuck: no reply to Luna's delegates-finished notice.{earlier}{stopped}{lost}{unsure}")),
                        if trouble {
                            failure()
                        } else {
                            turn_settlement(o, ended, read_to, &replies, requests, status, false, false, false, false)
                        },
                    ));
                }
            }
        }
        let idle = if parent && !trouble && alive && !work_left { NOTHING_RUNNING } else { "" };
        let how = if trouble {
            failure()
        } else {
            turn_settlement(o, ended, read_to, &replies, requests, status, plan || keeps_plan, false, false, false)
        };
        return Some((Some(format!("{who} finished its turn{reply}.{idle}{stopped}{lost}{unsure}")), how));
    }
    // Quiet conditions have their own allowance, separate from an idle stall.
    let answered = crate::background::turn_answered(id);
    let untold = if answered { o.told_background_wait != Some(ended) } else { o.told_quiet != Some(ended) };
    if turn == Some(crate::activity::Turn::Busy) && alive && untold {
        let waiting = crate::background::waits_on_background(id).filter(|_| answered);
        if let Some(quiet) = turn_quiet_ms(pty, id).filter(|q| *q >= QUIET_TURN_MS) {
            // Looked for only once the turn is that quiet: a deadline holds the
            // notice back, never brings it sooner.
            let at = now_ms();
            let due = due_ms(pty, id, k.provider);
            if due.is_none_or(|due| at >= due.saturating_add(PAST_DUE_MS)) {
                let target = if o.upto == 0 {
                    o.quiet_target()
                } else {
                    let (said, _) = written_range(pty, id, k.provider, o.cursor);
                    let requests = requests_through(caller, id, o, ended.saturating_add(1));
                    said.asked_after(requests, o.request_match)
                        .notice_requests()
                        .0
                        .into_iter()
                        .rev()
                        .find(|q| q.reply && q.taken)
                        .map(|q| q.n)
                };
                return Some(match waiting {
                    Some(w) => (
                        Some(background_wait_line(who, quiet, &background_calls(pty, id, k.provider), &w.procs)),
                        if o.upto == 0 { Settled::WaitsOnBackground(ended) } else { Settled::QuietFor(ended, target, true) },
                    ),
                    None => (
                        Some(quiet_line(who, quiet, due.map(|due| at - due))),
                        if o.upto == 0 { Settled::Quiet(ended) } else { Settled::QuietFor(ended, target, false) },
                    ),
                });
            }
        }
    }
    standing_line(pty, who, id, k, o)
}

fn notice_replies(pty: &crate::pty::PtyManager, caller: &str, id: &str, provider: Provider, o: Owed, ended: u32) -> (Said, u64) {
    let cursor = o.notice_cursor();
    let mut requests = requests_through(caller, id, o, ended);
    if cursor > o.cursor && requests.iter().any(|q| !q.taken && q.cursor < cursor) {
        // The text was reported before this request was accounted for.
        // Its prompt is still delivery evidence even if its words were read.
        let (earlier, _) = written_range(pty, id, provider, o.cursor);
        let placed = earlier.asked_after(requests.clone(), o.request_match).notice_requests().0;
        for (request, evidence) in requests.iter_mut().zip(placed) {
            request.taken |= evidence.taken;
        }
    }
    let (fresh, end) = written_range(pty, id, provider, cursor);
    (fresh.asked_after(requests, o.request_match), end)
}

fn failed_requests(o: Owed, ended: u32, requests: &[Request], lost: &[String], unopened: bool) -> Settled {
    if requests.is_empty() {
        return Settled::Failed;
    }
    let covered: Vec<_> = requests
        .iter()
        .filter(|q| {
            q.delivery.as_ref().is_some_and(|qid| lost.contains(qid))
                || !unopened && (q.taken || (q.n == 1 && q.baseline < ended && !q.queued && (q.delivery.is_none() || o.cut_short)))
        })
        .map(|q| q.n)
        .collect();
    if covered.len() == requests.len() {
        Settled::Failed
    } else {
        Settled::FailedRequests(ended, covered)
    }
}

fn turn_changed(id: &str, ended: u32) -> bool {
    crate::activity::turn_state(id).is_some_and(|t| t.turn != crate::activity::Turn::Idle || t.turns_ended != ended)
}

fn plan_covers(o: Owed, ended: u32) -> bool {
    o.wake.plan_through.is_some_and(|through| ended <= through)
}

fn can_stall(o: Owed, requests: &[Request], plan_covers: bool) -> bool {
    if o.no_reply || plan_covers {
        return false;
    }
    if requests.is_empty() {
        return o.legacy_can_stall();
    }
    requests.iter().any(|q| q.reply && q.taken && !q.told)
}

fn silent_or_untaken(o: Owed, ended: u32, requests: &[Request], unread: bool) -> Settled {
    if requests.iter().any(|q| !q.taken) {
        return if o.notice_baseline() < ended { Settled::UnusedTurn(ended) } else { Settled::Unhold };
    }
    Settled::Silent(ended, unread)
}

#[allow(clippy::too_many_arguments)]
fn turn_settlement(
    o: Owed,
    ended: u32,
    cursor: u64,
    said: &Said,
    mut requests: Vec<Request>,
    status: Option<Status>,
    plan: bool,
    work_left: bool,
    job: bool,
    pending: bool,
) -> Settled {
    let plan = plan && !o.no_reply;
    let untaken = requests.iter().any(|q| !q.taken);
    if !work_left && !plan && !untaken {
        return Settled::Answered;
    }
    let matched = requests.iter().rev().find(|q| q.taken).map(|q| q.n).or(o.request_match);
    if requests.is_empty() && pending && !plan {
        return Settled::Pending(ended, cursor, said.last_request().or(matched), status, job);
    }
    if !work_left && !plan {
        for request in &mut requests {
            request.told |= request.taken;
        }
    }
    Settled::Progress(super::Progress { ended, cursor, matched, requests, plan, work_left, job, pending })
}

fn had_delegates(id: &str, o: Owed) -> bool {
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    o.wake.job_running || r.delegates.contains_key(id) || r.woken.contains_key(id)
}

fn unsure_line(o: Owed) -> &'static str {
    if o.unsure {
        " Luna cannot tell whether your last message reached it: the core went as it was going in, and its transcript cannot be read. luna_message_list shows what it has."
    } else {
        ""
    }
}

/// What a parent's notice adds when its helper or lead ended a turn with
/// nothing left to go on with (`runs_nothing`). A lead ended its turn on
/// "Gate is running; waiting." with no gate started, and its parent, told only
/// that it finished its turn, read that as work under way for ten hours.
const NOTHING_RUNNING: &str = " [idle; send to resume]";

/// Whether session `id`, its turn state `ts`, is between turns with nothing
/// that would start its next turn or that it waits on: no job or subagent its turns left (`children_busy`); no answer
/// owed to it by anyone else, a helper of its own or a session it wrote to,
/// whose notice would wake it; no `luna_session_wait` of its out; nothing
/// queued, held or reserved for it; and no line waiting to go in, one queued
/// (`news`) or a spawn a core's going cut off. Usage and places news can
/// wake a top chat only. Each is read as it stands when the notice is made.
/// Only the work Luna sees counts.
#[derive(Default)]
pub(super) struct WorkLook {
    graph: Option<WorkGraph>,
}

struct WorkGraph {
    edges: HashMap<String, Vec<WorkEdge>>,
    upstream: HashMap<String, Vec<String>>,
    pending: HashSet<String>,
    queued: HashMap<String, u32>,
    sessions: HashMap<String, (Provider, Option<String>)>,
    turns: HashMap<String, Option<crate::activity::TurnState>>,
    working: HashMap<String, bool>,
    news: PeerNews,
}

struct WorkEdge {
    peer: String,
    owed: Owed,
    due: bool,
    can_go: Option<bool>,
}

impl WorkGraph {
    fn callers_of<'a>(upstream: &'a HashMap<String, Vec<String>>, id: &'a str) -> HashSet<&'a str> {
        let mut reached = HashSet::new();
        let mut callers = vec![id];
        while let Some(caller) = callers.pop() {
            if reached.insert(caller) {
                callers.extend(upstream.get(caller).into_iter().flatten().map(String::as_str));
            }
        }
        reached
    }

    fn sample(pty: &crate::pty::PtyManager) -> Self {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let pending = r
            .waits
            .keys()
            .chain(r.reserved.keys())
            .map(|(c, _)| c.clone())
            .chain(r.held.iter().map(|h| h.id.clone()))
            .chain(r.news.iter().map(|(c, _)| c.clone()))
            .chain(r.spawns.values().filter(|s| s.left_by_core || s.news.is_some()).map(|s| s.caller.clone()))
            .collect();
        let sessions = r.known.iter().map(|(id, k)| (id.clone(), (k.provider, k.parent.clone()))).collect();
        let debts: Vec<_> =
            r.owed.iter().filter(|(_, o)| o.active()).map(|(pair, o)| (pair.clone(), *o, r.stopped.get(pair).copied())).collect();
        drop(r);

        let mut graph = Self {
            edges: HashMap::new(),
            upstream: HashMap::new(),
            pending,
            queued: HashMap::new(),
            sessions,
            turns: HashMap::new(),
            working: HashMap::new(),
            news: PeerNews::default(),
        };
        for ((from, peer), owed, stopped) in debts {
            if owed.queued {
                let floor = graph.queued.entry(peer.clone()).or_default();
                *floor = (*floor).max(owed.baseline);
            }
            if owed.no_reply {
                continue;
            }
            let state = graph.turns.entry(peer.clone()).or_insert_with(|| crate::activity::turn_state(&peer));
            let due = (owed.resumed
                || owed.job_went
                || owed.cut_short
                || state.is_some_and(|t| t.turns_ended > owed.notice_baseline() || owed.told_pending))
                && stopped.is_none_or(|at| breaker_lifted(pty, &from, &peer, at));
            if due {
                graph.upstream.entry(peer.clone()).or_default().push(from.clone());
            }
            graph.edges.entry(from).or_default().push(WorkEdge { peer, owed, due, can_go: None });
        }
        graph
    }
}

impl WorkLook {
    pub(super) fn runs_nothing(
        &mut self,
        pty: &crate::pty::PtyManager,
        recipient: &str,
        id: &str,
        ts: Option<crate::activity::TurnState>,
    ) -> bool {
        let Some(ts) = ts.filter(|t| t.turn == crate::activity::Turn::Idle && !t.children_busy) else { return false };
        let graph = self.graph.get_or_insert_with(|| WorkGraph::sample(pty));
        let working = &mut graph.working;
        let mut has_work = |id: &str| *working.entry(id.to_owned()).or_insert_with(|| crate::background::has_work(id));
        if has_work(id) || graph.pending.contains(id) || graph.queued.get(id).is_some_and(|floor| ts.turns_ended <= *floor) {
            return false;
        }

        // A due reply inside the same idle cycle can go to another member.
        // A caller outside the cycle still waits for their real answer.
        let leads_back = WorkGraph::callers_of(&graph.upstream, id);
        let internal = leads_back.contains(recipient) && WorkGraph::callers_of(&graph.upstream, recipient).contains(id);
        let mut visited = HashSet::from([id.to_owned()]);
        let mut peers = vec![id.to_owned()];
        while let Some(caller) = peers.pop() {
            for edge in graph.edges.get_mut(&caller).into_iter().flatten() {
                let peer = edge.peer.as_str();
                if peer != id
                    && edge.due
                    && (!internal
                        || !leads_back.contains(peer)
                        || edge.owed.resumed
                        || edge.owed.job_went
                        || *edge.can_go.get_or_insert_with(|| {
                            let known = graph.sessions.get(peer);
                            let provider = known.map(|(provider, _)| *provider);
                            let parent = known.and_then(|(_, parent)| parent.as_deref()) == Some(caller.as_str());
                            graph.news.can_go(pty, &caller, peer, edge.owed, provider, parent)
                        }))
                {
                    return false;
                }
                if !visited.insert(peer.to_owned()) {
                    continue;
                }
                if graph.pending.contains(peer)
                    || has_work(peer)
                    || graph.turns[peer].is_some_and(|t| t.turn != crate::activity::Turn::Idle || t.children_busy)
                {
                    return false;
                }
                peers.push(peer.to_owned());
            }
        }
        true
    }
}

#[cfg(test)]
fn runs_nothing(id: &str, ts: Option<crate::activity::TurnState>) -> bool {
    WorkLook::default().runs_nothing(crate::pty::manager(), id, id, ts)
}

#[derive(Default)]
struct PeerNews {
    trouble: HashMap<String, bool>,
    status: HashMap<(String, u64), bool>,
}

impl PeerNews {
    /// A peer can report these even if it is still waiting for an answer itself.
    fn can_go(
        &mut self,
        pty: &crate::pty::PtyManager,
        caller: &str,
        peer: &str,
        owed: Owed,
        provider: Option<Provider>,
        parent: bool,
    ) -> bool {
        owed.cut_short
            || owed.unsure
            || *self.trouble.entry(peer.to_owned()).or_insert_with(|| {
                crate::background::cut_short(peer)
                    || crate::background::stop_of(peer).is_some()
                    || provider
                        .is_some_and(|provider| failure_of(pty, peer, provider).is_some() || crate::activity::limit_of(peer).is_some())
            })
            || !crate::background::untold_losses(peer, caller).is_empty()
            || !undelivered_for(peer, caller, parent).is_empty()
            || provider.is_some_and(|provider| {
                *self
                    .status
                    .entry((peer.to_owned(), owed.notice_cursor()))
                    .or_insert_with(|| fresh_status(&written_range(pty, peer, provider, owed.notice_cursor()).0).is_some())
            })
    }
}

/// What a notice says of a helper whose CLI stopped what its last turn left in
/// the background (background/run.rs `stops_background`), or whose turn's
/// process went with background subagents unfinished (background/events.rs
/// `turn_over`), to a caller it owed an answer then that has not had it yet;
/// nothing for any other. Without it, the end of those subagents reads as a
/// job that finished and said nothing.
fn background_stopped(id: &str, caller: &str) -> String {
    background_stopped_line(id, caller).map(|(line, _)| line).unwrap_or_default()
}

/// See `background_stopped`: a line for each loss `caller` has yet to hear
/// of, oldest first, each by its own cause, and the turns of those losses.
/// A notice that went in with a loss's line acknowledges that loss for its
/// caller (notifier `settle_delivered`); a reply of a wait, read or
/// report, the turns it carried (owed.rs `background_stopped_note`,
/// `hand_over_said`).
pub(in crate::agents) fn background_stopped_line(id: &str, caller: &str) -> Option<(String, Vec<u32>)> {
    let losses = crate::background::untold_losses(id, caller);
    if losses.is_empty() {
        return None;
    }
    Some((losses.iter().map(loss_line).collect(), losses.iter().map(|l| l.turn).collect()))
}

/// One loss's line: by what stopped the work and which subagents it took.
pub(in crate::agents) fn loss_line(l: &crate::background::Loss) -> String {
    use crate::background::{LostBy, StopCause};
    let kind = if l.cause == StopCause::TurnEnd { "subagent" } else { "background subagent" };
    let names = &l.names;
    match (l.cause, l.by) {
        // Saved by a core before subagents were counted.
        (StopCause::TurnEnd, _) if names.is_empty() => SUBAGENTS_INTERRUPTED.to_string(),
        (StopCause::Ceiling, _) if names.is_empty() => BACKGROUND_STOPPED.to_string(),
        // Luna stopped the turn, at its caller's word or for its own going:
        // that they were cut is all there is to say, nothing on how to work.
        (_, Some(LostBy::Stopped)) => format!(" {}.", subagents_lost(kind, names)),
        (StopCause::TurnEnd, _) => format!(" {}. {LOST_GOES_ON_CODEX}", subagents_lost(kind, names)),
        (StopCause::Ceiling, None) => {
            format!(" {}: its CLI stopped them, out of time to wait for them. {LOST_GOES_ON}", subagents_lost(kind, names))
        }
        (StopCause::Ceiling, Some(_)) => format!(" {}. {LOST_GOES_ON}", subagents_lost(kind, names)),
    }
}

/// "2 background subagents ("a", "b") ended with its turn without finishing;
/// their work is not done": said plainly, and never "finished" of them. A
/// lead's parent read "its delegates finished" as nothing left to wait for,
/// while the implementers its lead's CLI took with it had committed nothing.
/// Those with a name ("" for none) are named, the first LOST_NAMED of them.
fn subagents_lost(kind: &str, names: &[String]) -> String {
    let n = names.len();
    let mut quoted: Vec<String> = names.iter().filter(|d| !d.is_empty()).take(LOST_NAMED).map(|d| format!("\"{d}\"")).collect();
    if !quoted.is_empty() && n > quoted.len() {
        quoted.push(format!("{} more", n - quoted.len()));
    }
    let what = if n == 1 { format!("1 {kind}") } else { format!("{n} {kind}s") };
    let named = if quoted.is_empty() { String::new() } else { format!(" ({})", quoted.join(", ")) };
    format!("{what}{named} ended with its turn without finishing; their work is not done")
}

/// How many lost subagents a line names; the rest it counts.
const LOST_NAMED: usize = 5;

/// What a line on lost subagents has the caller do about them.
const LOST_GOES_ON: &str =
    "luna_session_send has it check what they left and carry on, with Luna helpers for work that must outlive its turn.";

/// The same for a Codex helper, whose subagents `wait_agent` can wait for.
const LOST_GOES_ON_CODEX: &str =
    "luna_session_send has it check what they left and carry on, waiting for any subagent it spawns before it ends its turn.";

/// Claude Code's own word that it stopped what the turn left, with no
/// subagent of it named.
pub(in crate::agents) const BACKGROUND_STOPPED: &str = " Its CLI stopped the subagents and jobs it had left in the background, out of time to wait for them, not because they were done: their work may be unfinished and uncommitted. luna_session_send has it check what they left and carry on.";

/// The same for a Codex helper, whose turn's server goes, and the subagents
/// its turn spawned with it, as the turn ends (codex::session `subagents_cut_at_turn_end`),
/// as a core that did not count them saved it.
pub(in crate::agents) const SUBAGENTS_INTERRUPTED: &str = " Its CLI interrupted the subagents it had spawned as its turn ended, before they were done: their work may be unfinished and uncommitted. luna_session_send has it check what they left and carry on, waiting for any subagent it spawns before it ends its turn.";

/// A turn that has said nothing for this long, its caller hears of once: a
/// command that hangs looks like this and like nothing else from outside, and
/// a caller that ended its turn on "a notice comes when it is done" would
/// otherwise hear nothing until a person asks. A long build looks the same,
/// hence once per turn and not its end.
pub(in crate::agents) const QUIET_TURN_MS: u64 = 30 * 60 * 1000;

/// A command its own timeout has yet to stop is no sign of a hang: a quiet turn
/// that waits on one is told of only this long past that timeout, by when the
/// CLI has ended the call and written so, or it hangs. Test lanes of 45 min
/// under a timeout of their own each woke their caller for nothing.
const PAST_DUE_MS: u64 = 5 * 60 * 1000;

fn quiet_line(who: &str, quiet_ms: u64, past_due_ms: Option<u64>) -> String {
    let quiet = quiet_ms / 60_000;
    match past_due_ms {
        Some(past) => format!(
            "{who} is mid-turn and has written nothing for {quiet} min, and the command it waits on is {} min past its own timeout, so it may hang. Check luna_message_list with screen: true.",
            past / 60_000
        ),
        None => format!(
            "{who} is mid-turn and has written nothing for {quiet} min: a command it runs may hang, or simply take long. Check luna_message_list with screen: true."
        ),
    }
}

/// A helper that has written its turn's answer and whose CLI is still up only
/// for what the turn left in the background (background `waits_on_background`):
/// what it waits on, as the model asked for it (`calls`, its transcript) and as
/// processes in its job (`procs`). Its turn, and every message to it, waits on
/// that until it ends or Claude Code stops it (background/command.rs
/// `BG_WAIT_CEILING_MS`): a watcher left behind looks like this and like nothing
/// else.
fn background_wait_line(who: &str, quiet_ms: u64, calls: &[String], procs: &[String]) -> String {
    let mut what: Vec<String> = calls.to_vec();
    if !procs.is_empty() {
        what.push(format!("processes {}", procs.join(", ")));
    }
    let what = if what.is_empty() { "nothing Luna can name".to_string() } else { what.join("; ") };
    // A subagent may think for long between its steps: no sign of a hang.
    let stop = if calls.iter().any(|c| c.starts_with("subagent ")) {
        "A subagent may still be thinking. Read with luna_message_list or end with luna_session_stop."
    } else {
        "Read with luna_message_list or end with luna_session_stop."
    };
    format!(
        "{who} answered but its turn waits on background work ({} min quiet): {what}. Sends queue until it ends (Claude Code stops it 3 h after the answer). {stop}",
        quiet_ms / 60_000
    )
}

/// What a session's turns left running, by name, for a notice: a helper's job's
/// (background/), a chat's as its record names it (activity/).
fn left_named(id: &str) -> Option<String> {
    if crate::background::has(id) {
        return crate::background::left_named(id);
    }
    Some(crate::activity::left_running(id).join(", ")).filter(|names| !names.is_empty())
}

/// What a session's turns left in the background and its transcript has not
/// seen the end of, as the model asked for it: Claude Code's background calls
/// (`claude::session::background_calls`), Codex's commands still running past
/// their call (`codex::session::background_calls`).
fn background_calls(pty: &crate::pty::PtyManager, id: &str, provider: Provider) -> Vec<String> {
    let Some(path) = transcript_path(pty, id) else { return Vec::new() };
    match provider {
        Provider::Claude => crate::claude::session::background_calls(&path),
        Provider::Codex => crate::codex::session::background_calls(&path),
    }
}

/// The number of requests covered and the latest request's identifier.
fn answering(replies: &Said) -> String {
    let requests = replies.requests();
    let Some(latest) = requests.last() else { return String::new() };
    let key = latest.key.as_deref().map(|key| format!(" ({})", key.escape_debug())).unwrap_or_default();
    if requests.len() == 1 {
        format!(", #{}{key}", latest.n)
    } else {
        format!(", {} requests, latest #{}{key}", requests.len(), latest.n)
    }
}

/// Mark previews whose request is unknown or differs from the header.
fn begins_re(replies: &Said, already_pending: bool) -> String {
    let quoted = begins(replies);
    let requests = replies.requests();
    if quoted.is_empty() || requests.len() <= 1 {
        return quoted;
    }
    match replies.last_request() {
        Some(n) if requests.last().is_some_and(|q| q.n != n) => format!(" #{n}{quoted}"),
        // Claude's reader skips native command records, so their absence
        // cannot tell us whether the reply belongs to the command. A pending
        // reply may also begin past its prompt, especially after an old save.
        None if !already_pending && !requests.iter().any(|q| crate::claude::session::command_record(&q.head)) => {
            format!(" [unmatched]{quoted}")
        }
        _ => quoted,
    }
}

/// When the commands a session's turn waits on are due, the latest of them:
/// read off its transcript (`claude::session::due_ms`) or rollout
/// (`codex::session::due_ms`). Asked every few seconds while a quiet turn's
/// notice waits on it, so a file is read again only once it has grown, which a
/// quiet one does not.
fn due_ms(pty: &crate::pty::PtyManager, id: &str, provider: Provider) -> Option<u64> {
    static READ: OnceLock<Mutex<HashMap<std::path::PathBuf, (u64, Option<u64>)>>> = OnceLock::new();
    let path = transcript_path(pty, id)?;
    let len = std::fs::metadata(&path).ok()?.len();
    let read = READ.get_or_init(Default::default);
    if let Some((was, due)) = read.lock().unwrap_or_else(|e| e.into_inner()).get(&path).copied() {
        if was == len {
            return due;
        }
    }
    let due = match provider {
        Provider::Claude => crate::claude::session::due_ms(&path),
        Provider::Codex => crate::codex::session::due_ms(&path),
    };
    let mut map = read.lock().unwrap_or_else(|e| e.into_inner());
    // A file per quiet turn looked at: few, but the app runs for weeks.
    if map.len() >= 64 {
        map.clear();
    }
    map.insert(path, (len, due));
    due
}

/// While a session's turn runs, how long nothing has come of it: a helper's
/// output (background/), a chat's transcript; None between turns.
fn turn_quiet_ms(pty: &crate::pty::PtyManager, id: &str) -> Option<u64> {
    let now = now_ms();
    if crate::background::has(id) {
        return crate::background::turn_quiet_ms(id, now);
    }
    let started = crate::activity::turn_started_ms(id)?;
    let written = transcript_path(pty, id)
        .and_then(|p| std::fs::metadata(p).ok()?.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as u64);
    Some(now.saturating_sub(written.max(started)))
}

/// What a notice has to say of where the session stands, whatever its turns
/// have come to: it has exited, or it holds on a prompt or a question. All a
/// pair the breaker has stopped still hears: a helper must never sit on a
/// prompt until it times out for the breaker's sake.
pub(in crate::agents) fn standing_line(
    pty: &crate::pty::PtyManager,
    who: &str,
    id: &str,
    k: &Known,
    o: Owed,
) -> Option<(Option<String>, Settled)> {
    let turn = crate::activity::turn_state(id).map(|t| t.turn);
    if !live(pty, id) {
        // The window has yet to start it, as it does a chat in a pane.
        if returning(k, core_up_ms()) {
            return None;
        }
        if o.cut_short {
            return Some((
                Some(format!("{who} did NOT finish its turn: Luna's core restarted while it worked, and it has not started again. It runs again once the user opens it.")),
                Settled::Exited,
            ));
        }
        return Some((
            Some(match failure_of(pty, id, k.provider) {
                Some(why) => format!("{who} has exited on an error: {}", sentence(&why)),
                None => format!("{who} has exited."),
            }),
            Settled::Exited,
        ));
    }
    let ask = ask_of(id);
    let held = turn == Some(crate::activity::Turn::Waiting) || ask.as_ref().is_some_and(|a| a.blocking && a.kind == "question");
    if held && !o.told_waiting {
        let line = match &ask {
            Some(a) => match &a.id {
                Some(ask_id) => format!("{who} {}. Answer with luna_session_answer, askId {ask_id}.", a.line()),
                None => format!("{who} {}. Answer with luna_session_answer.", a.line()),
            },
            None => format!("{who} is stopped on a prompt in its terminal that Luna cannot read; luna_message_list with screen: true shows it, and only the person can answer it."),
        };
        return Some((Some(line), Settled::Waiting));
    }
    // Codex's async question: the helper works on, the question waits.
    if let Some(a) = ask.filter(|a| !a.blocking) {
        let key = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            a.line().hash(&mut h);
            h.finish()
        };
        if o.told_ask != key {
            return Some((Some(format!("{who} {} It works on meanwhile; luna_session_answer answers it.", a.line())), Settled::Asked(key)));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::ask::ask_of;
    use crate::agents::lost::sentence;
    use crate::agents::notifier::{job_end_stood, Settled, NOTICE_TAG};
    use crate::agents::owed::{owe_from, requests_of, Owed};
    use crate::agents::persist::{tie_back, ties_of, Ties};
    use crate::agents::read::transcripts;
    use crate::agents::registry::{parent_of, reg, register, waits_on_helpers};
    use crate::agents::said::said_since;
    use crate::agents::teardown::forget;
    use crate::agents::test_support::{job_over_helper, notice_for};
    use crate::agents::wait::{reply_text, REPLY_GRACE_MS};

    #[test]
    fn a_preview_labels_earlier_and_unmatched_requests() {
        use crate::agents::read::Message;
        use crate::agents::said::{said_of, Request};
        let request = |n, head: &str| Request::new(n, None, head.into(), true);
        let message =
            |role: &str, text: &str| Message { role: role.into(), text: text.into(), after_cli: false, joined: false, final_answer: false };
        let preview = |prompt, requests| {
            begins_re(&said_of(vec![message("user", prompt), message("assistant", "DONE: first pass")]).asked(requests), false)
        };
        let requests = || vec![request(4, "review"), request(5, "merge")];
        assert_eq!(preview("review", requests()), " #4 \"DONE: first pass\"");
        assert_eq!(preview("unknown", requests()), " [unmatched] \"DONE: first pass\"");
        assert_eq!(preview("merge", requests()), " \"DONE: first pass\"");
        assert_eq!(preview("unknown", vec![request(5, "merge")]), " \"DONE: first pass\"");
        assert_eq!(preview("unknown", vec![request(4, "review"), request(5, "<command-name>/brief")]), " \"DONE: first pass\"");
        assert_eq!(preview("unknown", vec![]), " \"DONE: first pass\"");
    }

    #[test]
    fn a_pending_answer_keeps_its_request_past_the_prompt() {
        use crate::agents::notifier::settle_told;
        use crate::agents::owed::{owe_asked, shown, Asked};
        for delivery in ["notice", "wait", "restart"] {
            let [caller, id, child] = ["caller", "helper", "child"].map(|s| format!("pending-match-{delivery}-{s}"));
            let (caller, id, child) = (caller.as_str(), id.as_str(), child.as_str());
            let folder = "C:/p";
            let account = "C:/acc/anthropic/one";
            register(caller, Provider::Claude, folder, account, None, true, serde_json::json!({}));
            register(id, Provider::Claude, folder, account, Some(caller), false, serde_json::json!({ "background": true }));
            crate::background::idle_for_test(id, Provider::Claude, 3, false);
            let path = std::env::temp_dir().join(format!("luna-{id}-{}.jsonl", std::process::id()));
            let before = format!("{}{}", prompt_line("review the patch"), said_line("PLAN: Still reviewing."));
            std::fs::write(&path, &before).unwrap();
            transcripts().lock().unwrap().insert(id.into(), path.clone());
            owe_asked(caller, id, 2, 0, &Asked { text: "review the patch", key: None, reply: true });
            owe_asked(caller, id, 2, 0, &Asked { text: "check the other patch", key: None, reply: true });
            let key = (caller.to_string(), id.to_string());
            let debt = || reg().lock().unwrap().owed[&key];
            let pty = crate::pty::manager();
            let look = |o| {
                let known = reg().lock().unwrap().known[id].clone();
                notice_line(pty, caller, "Your helper", id, &known, held(o, 3, BACKGROUND_GRACE_MS))
            };
            if delivery == "wait" {
                register(child, Provider::Claude, folder, account, Some(id), false, serde_json::json!({}));
                owe_from(id, child, 0, 0);
                shown(caller, id, 3);
                forget(child);
            } else {
                crate::background::left_for_test(id, &[("gate", 22104)]);
                let first = debt();
                let Some((Some(line), how @ Settled::Progress(_))) = look(first) else { panic!("{delivery}: the first reply is pending") };
                settle_told(&mut reg().lock().unwrap(), key.clone(), how, &first);
                let file = line.split("Report ").nth(1).and_then(|tail| tail.split(":1–").next()).unwrap();
                let _ = std::fs::remove_file(file);
                if delivery != "restart" {
                    crate::background::left_for_test(id, &[]);
                }
            }
            if delivery == "restart" {
                let saved = serde_json::to_vec(&ties_of(id).unwrap()).unwrap();
                forget(id);
                tie_back(id, Provider::Claude, folder, account, serde_json::from_slice(&saved).unwrap());
                transcripts().lock().unwrap().insert(id.into(), path.clone());
                crate::background::idle_for_test(id, Provider::Claude, 3, false);
                let interrupted = debt();
                let Some((Some(_), how @ Settled::Resumed)) = look(interrupted) else { panic!("last core took the job") };
                settle_told(&mut reg().lock().unwrap(), key.clone(), how, &interrupted);
            }
            assert_eq!(debt().request_match, Some(1), "{delivery}");
            assert_eq!(debt().cursor, before.len() as u64, "{delivery}");
            let report = format!("DONE: Reviewed the patch. {}", "Review details. ".repeat(40));
            std::fs::write(&path, format!("{before}{}", said_line(&report))).unwrap();
            let Some((Some(line), Settled::Progress(_))) = look(debt()) else { panic!("{delivery}: job done") };
            assert!(line.contains("2 requests, latest #2") && line.contains(" #1 \"DONE: Reviewed"), "{delivery}: {line}");
            assert!(!line.contains("[unmatched]"), "{delivery}: {line}");
            let file = line.split("Report ").nth(1).and_then(|tail| tail.split(":1–").next()).unwrap();
            let _ = std::fs::remove_file(file);
            crate::background::gone_for_test(id);
            forget(id);
            forget(caller);
            let _ = std::fs::remove_file(&path);
        }
    }

    #[test]
    fn request_headers_count_and_name_only_the_latest() {
        use crate::agents::said::Request;
        let request = |n| Request::new(n, Some(format!("key-{n}")), String::new(), true);
        assert_eq!(answering(&Said::default()), "");
        assert_eq!(answering(&Said::default().asked(vec![request(25)])), ", #25 (key-25)");
        let requests = (5..=25).map(request).collect();
        assert_eq!(answering(&Said::default().asked(requests)), ", 21 requests, latest #25 (key-25)");
    }

    #[test]
    fn a_failed_turn_quotes_nothing_as_its_verdict() {
        use crate::activity::{on_hook, HookEvent};
        let path = std::env::temp_dir().join(format!("luna-verdict-{}.jsonl", std::process::id()));
        std::fs::write(
            &path,
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"All done and fixed.\"}]}}\n",
        )
        .unwrap();
        for (id, failed) in [("vd-fine", false), ("vd-failed", true)] {
            register(id, Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, false, serde_json::json!({}));
            transcripts().lock().unwrap().insert(id.into(), path.clone());
            on_hook(id, HookEvent::Busy);
            on_hook(id, if failed { HookEvent::Failed("rate_limit") } else { HookEvent::Idle });
        }
        let fine = line_for("vd-fine");
        // A line that short is the notice's own: no file, nothing to begin.
        assert!(fine.contains("finished its turn") && fine.contains("since you asked: \"All done and fixed.\""), "{fine}");
        assert!(!fine.contains("Report "), "{fine}");
        let failed = line_for("vd-failed");
        assert!(failed.contains("stopped on an error: rate_limit"), "{failed}");
        assert!(failed.contains("It ended with 1 message"), "what it said before is still named: {failed}");
        assert!(!failed.contains("All done"), "its words are no verdict on it: {failed}");
        for id in ["vd-fine", "vd-failed"] {
            forget(id);
            crate::activity::forget(id);
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn two_chats_with_a_job_each_do_not_wake_each_other_forever() {
        let (a, b) = ("pp-a", "pp-b");
        let register_chat = |id: &str| {
            register(id, Provider::Claude, "C:/p", "C:/acc/anthropic/one", None, true, serde_json::json!({}));
            crate::background::idle_for_test(id, Provider::Claude, 1, false);
            crate::background::left_for_test(id, &[("tail.exe", 22104)]);
            let path = std::env::temp_dir().join(format!("luna-{id}-{}.jsonl", std::process::id()));
            std::fs::write(&path, said_line("The job is still running.")).unwrap();
            transcripts().lock().unwrap().insert(id.into(), path.clone());
            (reg().lock().unwrap().known[id].clone(), path)
        };
        let (ka, pa) = register_chat(a);
        let (kb, pb) = register_chat(b);
        owe_from(a, b, 0, 0);
        owe_from(b, a, 0, 0);
        for (caller, id, known) in [(a, b, &kb), (b, a, &ka)] {
            assert!(matches!(look(caller, id, known), Some((None, Settled::Hold(1)))));
            assert!(matches!(look(caller, id, known), Some((None, Settled::Skip(1, ..)))));
            assert!(look(caller, id, known).is_none());
            // Even if another notice sends work back across the pair, a
            // plain reply while its background job runs cannot echo.
            notice_turn(&mut reg().lock().unwrap(), id, 2, &[caller.to_string()], false);
            crate::background::idle_for_test(id, Provider::Claude, 2, false);
            crate::background::left_for_test(id, &[("tail.exe", 22104)]);
            assert!(matches!(look(caller, id, known), Some((None, Settled::Skip(2, ..)))));
        }
        crate::background::left_for_test(a, &[]);
        crate::background::left_for_test(b, &[]);
        for (caller, id, known) in [(b, a, &ka), (a, b, &kb)] {
            assert!(matches!(look(caller, id, known), Some((None, Settled::Hold(2)))));
            assert!(matches!(look(caller, id, known), Some((None, Settled::Silent(..)))));
        }
        for (id, path) in [(a, pa), (b, pb)] {
            crate::background::gone_for_test(id);
            forget(id);
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn told_it_is_not_done_a_caller_hears_only_of_news() {
        use crate::activity::Turn::{Busy, Idle, Waiting};
        let told = Owed::pending(5, 0);
        assert_eq!(verdict(&Owed::from(5, 0), 6, Some(Idle), true, false), Verdict::TurnOver, "a first turn end is news, job or not");
        assert_eq!(verdict(&Owed::from(5, 0), 5, Some(Idle), false, false), Verdict::Nothing);
        assert_eq!(verdict(&told, 5, Some(Idle), true, false), Verdict::AtWork, "the job runs on");
        assert_eq!(verdict(&told, 5, Some(Idle), false, false), Verdict::JobOver, "it ended with no turn");
        assert_eq!(verdict(&told, 5, Some(Busy), false, false), Verdict::Nothing, "a turn runs: its end will say");
        assert_eq!(verdict(&told, 5, Some(Waiting), false, false), Verdict::Nothing, "on a prompt: its own notice");
        assert_eq!(verdict(&told, 6, Some(Idle), true, false), Verdict::OnlyNotices, "a turn on a notice is no news");
        assert_eq!(verdict(&told, 6, Some(Idle), true, true), Verdict::TurnOver, "a turn of its own is");
        assert_eq!(verdict(&told, 6, Some(Busy), true, true), Verdict::Nothing, "not before it is over");
    }

    #[test]
    fn a_turn_counts_for_a_caller_told_it_is_not_done_unless_a_notice_started_it() {
        let mut r = reg().lock().unwrap();
        notice_turn(&mut r, "rt-s", 4, &["rt-other".into()], false);
        drop(r);
        let past = |after, ended| real_turn_past("rt-s", "rt-c", after, ended, false, || false);
        assert!(!past(3, 4), "the one a notice started");
        assert!(past(3, 5), "and the one after it, which none did");
        assert!(past(2, 4), "or one before");
        assert!(!past(4, 4), "none past the count");
        // A notice that did not start a turn is not marked: at worst one notice more.
        assert!(real_turn_past("rt-unmarked", "rt-c", 0, 1, false, || false));
        forget("rt-s");
    }

    /// A top chat, a helper with tools, and the helper's own helper: the first
    /// answer of the middle one is "I have started it", and what it makes of
    /// its helper's answer is what the top chat asked for.
    #[test]
    fn a_turn_on_its_own_helpers_news_reaches_the_caller_above() {
        use crate::activity::Turn::Idle;
        let none = serde_json::json!({});
        register("tr-top", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone());
        register("tr-mid", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("tr-top"), true, none.clone());
        register("tr-low", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("tr-mid"), false, none.clone());
        register("tr-low2", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("tr-mid"), false, none);
        owe_from("tr-mid", "tr-low", 0, 0);
        assert!(waits_on_helpers("tr-mid", "tr-top"));
        assert!(!waits_on_helpers("tr-mid", "tr-low"), "not counting the one that would be told");
        assert!(!waits_on_helpers("tr-top", ""), "answers owed to it, not those owed by it");
        owe_from("tr-mid", "tr-low2", 0, 0);

        // Its first turn ended: told it is not done (`notice_line`), the debt goes on.
        let told = Owed::pending(1, 0);
        // The first helper's notice starts its turn 2, with the other still out.
        reg().lock().unwrap().owed.remove(&("tr-mid".to_string(), "tr-low".to_string()));
        notice_turn(&mut reg().lock().unwrap(), "tr-mid", 2, &["tr-low".into()], false);
        let waits = waits_on_helpers("tr-mid", "tr-top");
        assert!(waits);
        assert_eq!(
            verdict(&told, 2, Some(Idle), waits, real_turn_past("tr-mid", "tr-top", 1, 2, waits, || false)),
            Verdict::OnlyNotices,
            "quiet while one is out"
        );
        assert_eq!(
            verdict(&told, 2, Some(Idle), waits, real_turn_past("tr-mid", "tr-top", 1, 2, waits, || true)),
            Verdict::TurnOver,
            "unless what it made of that is news for its parent"
        );
        // The last one's notice starts its turn 3: this is the result.
        reg().lock().unwrap().owed.remove(&("tr-mid".to_string(), "tr-low2".to_string()));
        notice_turn(&mut reg().lock().unwrap(), "tr-mid", 3, &["tr-low2".into()], false);
        let waits = waits_on_helpers("tr-mid", "tr-top");
        assert!(!waits);
        let told = Owed::pending(2, 0);
        assert_eq!(
            verdict(&told, 3, Some(Idle), waits, real_turn_past("tr-mid", "tr-top", 2, 3, waits, || false)),
            Verdict::TurnOver,
            "it reaches the top"
        );
        // A notice about something else, or about the caller itself, does not count.
        notice_turn(&mut reg().lock().unwrap(), "tr-mid", 4, &["tr-stranger".into()], false);
        assert!(!real_turn_past("tr-mid", "tr-top", 3, 4, false, || false));
        assert!(!real_turn_past("tr-mid", "tr-top", 3, 4, false, || true), "news or not");
        // A notice about its helper and someone else: not all its own helpers' doing.
        notice_turn(&mut reg().lock().unwrap(), "tr-mid", 6, &["tr-low".into(), "tr-stranger".into()], false);
        assert!(!real_turn_past("tr-mid", "tr-top", 5, 6, true, || true));
        notice_turn(&mut reg().lock().unwrap(), "tr-low", 1, &["tr-mid".into()], false);
        assert!(!real_turn_past("tr-low", "tr-mid", 0, 1, false, || false), "not a helper of its own");
        notice_turn(&mut reg().lock().unwrap(), "tr-mid", 5, &["tr-low".into()], false);
        assert!(!real_turn_past("tr-mid", "tr-low", 4, 5, false, || false), "its own notice coming back");
        for id in ["tr-low", "tr-low2", "tr-mid", "tr-top"] {
            forget(id);
        }
    }

    /// The QA helper of 0.8.1: its helpers' notices start its turns, each ends
    /// on a report its parent needs now, and its parent heard none until the
    /// last helper had answered. Since 0.18 what it needs now opens on a status
    /// word: four leads' long "still working" reports woke one parent dozens
    /// of times a night.
    #[test]
    fn a_report_made_on_its_own_helpers_news_reaches_its_parent_once() {
        let none = serde_json::json!({});
        register("ir-top", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone());
        register(
            "ir-mid",
            Provider::Claude,
            "C:\\p",
            "C:\\acc\\anthropic\\one",
            Some("ir-top"),
            true,
            serde_json::json!({ "background": true }),
        );
        register("ir-low", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("ir-mid"), false, none.clone());
        register("ir-low2", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("ir-mid"), false, none.clone());
        register("ir-peer", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none);
        crate::background::idle_for_test("ir-mid", Provider::Claude, 2, false);
        owe_from("ir-mid", "ir-low2", 0, 0);
        let said = |t: &str| {
            format!("{}\n", serde_json::json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": t }] } }))
        };
        let notice = format!(
            "{}\n",
            serde_json::json!({ "type": "user", "message": { "role": "user", "content": format!("{NOTICE_TAG} Your helper ir-low finished its turn.") } })
        );
        let before = said("Started both helpers.");
        let path = std::env::temp_dir().join(format!("luna-ir-mid-{}.jsonl", std::process::id()));
        transcripts().lock().unwrap().insert("ir-mid".into(), path.clone());
        // Its turn 2 is on ir-low's notice, with ir-low2 still out.
        notice_turn(&mut reg().lock().unwrap(), "ir-mid", 2, &["ir-low".into()], false);
        let known = reg().lock().unwrap().known.get("ir-mid").cloned().unwrap();
        let pty = crate::pty::manager();
        let line = |caller: &str, transcript: &str| {
            std::fs::write(&path, format!("{before}{transcript}")).unwrap();
            notice_line(pty, caller, "Your helper", "ir-mid", &known, Owed::pending(1, before.len() as u64))
        };
        let report = format!("BLOCKED: Phase 1 is in: kill the core before 12:35 UTC. {}", "Evidence. ".repeat(45));

        let Some((Some(text), Settled::Pending(2, cursor, ..))) = line("ir-top", &format!("{notice}{}", said(&report))) else {
            panic!("its parent hears")
        };
        assert!(text.contains("ended another turn") && text.contains("[helpers pending]"), "{text}");
        assert!(text.contains(" \"BLOCKED: Phase 1 is in"), "{text}");
        assert_eq!(cursor, std::fs::metadata(&path).unwrap().len(), "and what follows is what it hears next");
        let file = text.split("Report ").nth(1).and_then(|t| t.split(":1–").next()).unwrap();
        assert!(std::fs::read_to_string(file).unwrap().starts_with("BLOCKED: Phase 1 is in"));
        let _ = std::fs::remove_file(file);
        // A line or two is no news worth a turn: it comes with the answer.
        assert!(matches!(line("ir-top", &format!("{notice}{}", said("Noted; waiting on ir-low2."))), Some((None, Settled::Skip(2, ..)))));
        // Nor is a long interim report with no status word: it rides with the next notice.
        let interim = format!("Phase 1 is in, phase 2 under way. {}", "Evidence. ".repeat(45));
        assert!(matches!(line("ir-top", &format!("{notice}{}", said(&interim))), Some((None, Settled::Skip(2, ..)))));
        // A status word lower down is none: it opens the message or does not count.
        assert!(matches!(line("ir-top", &format!("{notice}{}", said("Notes.\nDONE: all of it"))), Some((None, Settled::Skip(2, ..)))));
        // A fresh status is actionable for any owed caller.
        assert!(matches!(line("ir-peer", &format!("{notice}{}", said(&report))), Some((Some(_), Settled::Pending(2, ..)))));
        // The same applies to a turn on a mixed batch of notices.
        notice_turn(&mut reg().lock().unwrap(), "ir-mid", 2, &["ir-low".into(), "ir-peer".into()], false);
        reg().lock().unwrap().notice_turns.get_mut("ir-mid").unwrap().remove(0);
        assert!(matches!(line("ir-top", &format!("{notice}{}", said(&report))), Some((Some(_), Settled::Pending(2, ..)))));
        let _ = std::fs::remove_file(&path);
        crate::background::gone_for_test("ir-mid");
        for id in ["ir-low", "ir-low2", "ir-mid", "ir-top", "ir-peer"] {
            forget(id);
        }
    }

    /// The loop of that night, between a chat and its own helper that wrote to
    /// each other, every turn on a notice saying plenty: what goes up to the
    /// parent never comes back down.
    #[test]
    fn a_parent_and_its_helper_that_wrote_to_each_other_do_not_wake_each_other_forever() {
        use crate::activity::Turn;
        let (p, h) = ("pq-parent", "pq-helper");
        let none = serde_json::json!({});
        register(p, Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone());
        register(h, Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some(p), true, none);
        let key = |caller: &str, id: &str| (caller.to_string(), id.to_string());
        let mut turns: HashMap<&str, u32> = HashMap::from([(p, 1), (h, 1)]);
        owe_from(p, h, 0, 0);
        owe_from(h, p, 0, 0);
        let mut notices = 0;
        for _ in 0..500 {
            for (caller, id) in [(p, h), (h, p)] {
                let Some(o) = reg().lock().unwrap().owed.get(&key(caller, id)).copied() else { continue };
                let ended = turns[id];
                // Always news, wherever its parent is the one to hear it.
                let upward = || parent_of(id).as_deref() == Some(caller);
                let v = verdict(
                    &o,
                    ended,
                    Some(Turn::Idle),
                    true,
                    o.told_pending && real_turn_past(id, caller, o.baseline, ended, false, upward),
                );
                match v {
                    Verdict::OnlyNotices => reg().lock().unwrap().owed.get_mut(&key(caller, id)).unwrap().baseline = ended,
                    Verdict::TurnOver => {
                        notices += 1;
                        let mut r = reg().lock().unwrap();
                        notice_turn(&mut r, caller, turns[caller] + 1, &[id.to_string()], false);
                        r.owed.insert(key(caller, id), Owed::pending(ended, 0));
                        drop(r);
                        *turns.get_mut(caller).unwrap() += 1;
                    }
                    _ => {}
                }
            }
        }
        assert_eq!(notices, 2, "\"NOT done yet\" once each, and quiet after");
        forget(h);
        forget(p);
    }

    /// News does not wait: words since the last notice, or a core that took the
    /// job. And what happens to a held line's session replaces it.
    #[test]
    fn a_jobs_end_with_something_to_say_goes_at_once() {
        let said = "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"All green.\"}]}}\n";
        let (known, o, path) = job_over_helper("nn-s", "nn-c", said);
        let pty = crate::pty::manager();
        let stood = Owed { held: Some((3, now_ms() - BACKGROUND_GRACE_MS)), ..o };
        assert!(matches!(notice_line(pty, "nn-c", "Your helper", "nn-s", &known, stood), Some((Some(_), Settled::Answered))), "it wrote");
        let (known, o, path2) = job_over_helper("nn-q", "nn-c", "");
        let stood = Owed { held: Some((3, now_ms() - BACKGROUND_GRACE_MS)), ..o };
        assert!(matches!(notice_line(pty, "nn-c", "Your helper", "nn-q", &known, stood), Some((None, Settled::Silent(..)))));
        let went = Owed { job_went: true, ..stood };
        let Some((Some(text), Settled::Resumed)) = notice_line(pty, "nn-c", "Your helper", "nn-q", &known, went) else {
            panic!("a core took it")
        };
        assert!(text.contains("cut off its background work"), "{text}");
        // Held past its grace, a wait still gets it: the debt is open, on the one clock.
        let key = ("nn-c".to_string(), "nn-q".to_string());
        reg().lock().unwrap().owed.insert(key.clone(), stood);
        assert!(job_end_stood(&key, 3, true));
        // At work again: no line, and its end counts from nothing.
        crate::background::left_for_test("nn-q", &[("build.exe", 4242)]);
        assert!(matches!(notice_line(pty, "nn-c", "Your helper", "nn-q", &known, stood), Some((None, Settled::Unhold))));
        for id in ["nn-s", "nn-q"] {
            crate::background::gone_for_test(id);
            forget(id);
        }
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&path2);
    }

    /// 4 November: helpers ran test lanes of 45 minutes under a timeout of their
    /// own, and their callers were woken by each as a turn gone quiet. One that
    /// waits on a command hears of it 5 min past the command's own timeout,
    /// Codex's as Claude Code's; a command that has finished holds nothing back.
    #[test]
    fn a_quiet_turn_waits_for_the_timeout_of_its_command() {
        use serde_json::json;
        let min = 60_000;
        let pty = crate::pty::manager();
        for provider in Provider::ALL {
            let (caller, id) = (format!("dq-c-{}", provider.as_str()), format!("dq-s-{}", provider.as_str()));
            register(&id, provider, "C:\\p", "C:\\acc\\one", Some(caller.as_str()), false, json!({}));
            crate::background::idle_for_test(&id, provider, 3, false);
            let o = Owed::from(3, 0);
            reg().lock().unwrap().owed.insert((caller.clone(), id.clone()), o);
            let known = reg().lock().unwrap().known.get(&id).cloned().unwrap();
            // Quiet since a command with a timeout of 40 min started, which has
            // since come back when `done`.
            let quiet_on = |quiet: u64, done: bool| {
                let at = crate::background::iso(now_ms() - quiet).unwrap();
                let [call, back] = match provider {
                    Provider::Claude => [
                        json!({ "type": "assistant", "timestamp": at, "message": { "content": [{ "type": "tool_use", "id": "t1", "name": "Bash", "input": { "command": "./lane.sh", "timeout": 40 * min } }] } }),
                        json!({ "type": "user", "message": { "content": [{ "type": "tool_result", "tool_use_id": "t1", "content": "ok" }] } }),
                    ],
                    Provider::Codex => [
                        json!({ "timestamp": at, "type": "response_item", "payload": { "type": "custom_tool_call", "name": "exec", "call_id": "c1",
                            "input": format!("// @exec: {{\"yield_time_ms\": {}}}\ntext(await tools.exec_command({{cmd:\"./lane.sh\"}}));\n", 40 * min) } }),
                        json!({ "timestamp": at, "type": "response_item", "payload": { "type": "custom_tool_call_output", "call_id": "c1", "output": "Script completed" } }),
                    ],
                };
                let text = if done { format!("{call}\n{back}\n") } else { format!("{call}\n") };
                let path = std::env::temp_dir().join(format!("luna-{id}-{quiet}-{done}-{}.jsonl", std::process::id()));
                std::fs::write(&path, text).unwrap();
                transcripts().lock().unwrap().insert(id.clone(), path.clone());
                crate::background::quiet_for_test(&id, quiet);
                let said = notice_line(pty, &caller, "Your helper", &id, &known, o);
                let _ = std::fs::remove_file(&path);
                said
            };
            // Half an hour of nothing, its command 9 min short of its timeout.
            assert!(matches!(quiet_on(31 * min, false), None | Some((None, _))), "{id}: it is still within its time");
            let Some((Some(text), Settled::Quiet(3))) = quiet_on(45 * min + 30_000, false) else {
                panic!("{id}: 5 min past its timeout is news")
            };
            assert!(text.contains("written nothing for 45 min, and the command it waits on is 5 min past its own timeout"), "{text}");
            let Some((Some(text), Settled::Quiet(3))) = quiet_on(31 * min, true) else {
                panic!("{id}: a finished command holds nothing back")
            };
            assert!(text.contains("written nothing for 31 min: a command it runs may hang"), "{text}");
            assert!(text.contains("Check luna_message_list with screen: true"), "{text}");
            crate::background::gone_for_test(&id);
            forget(&id);
        }
    }

    /// Claude Code 2.1.322 stopped helpers' background subagents 10 min after
    /// their turns, and their parents heard that they had nothing left running
    /// and had said nothing. What its CLI said of that reaches the parent, in
    /// the job's notice and in the turn's.
    #[test]
    fn a_helper_whose_cli_stopped_its_background_work_says_so() {
        let pty = crate::pty::manager();
        let (known, o, path) = job_over_helper("bs-s", "bs-c", "");
        crate::background::background_stopped_for_test("bs-s", &["bs-c", "bs-other"]);
        let stood = Owed { held: Some((3, now_ms() - BACKGROUND_GRACE_MS)), ..o };
        let Some((Some(text), Settled::Failed)) = notice_line(pty, "bs-c", "Your helper", "bs-s", &known, stood) else {
            panic!("its work cut off is news")
        };
        assert!(
            text.contains("nothing left running now, and it has said nothing since your last notice. Its CLI stopped the subagents"),
            "{text}"
        );
        assert!(text.contains("may be unfinished and uncommitted") && !text.contains("does not take that up"), "{text}");
        let turn = held(Owed { told_pending: false, baseline: 2, cursor: 0, ..o }, 3, REPLY_GRACE_MS);
        let Some((Some(text), Settled::Failed)) = notice_line(pty, "bs-c", "Your helper", "bs-s", &known, turn) else {
            panic!("its turn is over")
        };
        assert!(text.starts_with("Your helper finished its turn") && text.contains("Its CLI stopped the subagents"), "{text}");
        // A core that goes before the notice is in: the next one still says it.
        crate::background::restart_for_test("bs-s");
        let Some((Some(text), Settled::Failed)) = notice_line(pty, "bs-c", "Your helper", "bs-s", &known, stood) else {
            panic!("still news after a restart")
        };
        assert!(text.contains("Its CLI stopped the subagents"), "{text}");
        // Who has had it is kept across cores too; once everyone it concerns has, it is done.
        crate::background::losses_told("bs-s", "bs-c", &[3]);
        crate::background::restart_for_test("bs-s");
        let told = notice_line(pty, "bs-c", "Your helper", "bs-s", &known, stood).and_then(|(t, _)| t).unwrap_or_default();
        assert!(!told.contains("Its CLI stopped"), "said to this caller already: {told}");
        assert!(crate::background::background_stopped_for("bs-s", "bs-other"));
        crate::background::losses_told("bs-s", "bs-other", &[3]);
        assert!(!crate::background::background_stopped("bs-s"));
        crate::background::gone_for_test("bs-s");
        forget("bs-s");
        let _ = std::fs::remove_file(&path);
    }

    /// A lead's turn started three background implementers, and its Claude
    /// Code process went with two of them at work. Its parent hears that
    /// plainly, by name, in the notice and in what a wait or a report hands
    /// it, and never that they finished.
    #[test]
    fn background_subagents_gone_with_the_turn_are_named_as_unfinished() {
        let (id, caller) = ("ls-s", "ls-c");
        let pty = crate::pty::manager();
        let (known, o, path) = job_over_helper(id, caller, "");
        crate::background::set_running_for_test(id);
        let event = |v: serde_json::Value| crate::background::claude_event_for_test(id, &v);
        for (task, description) in [("a1", "Implement slice A"), ("b2", "Implement slice B"), ("c3", "Implement slice C")] {
            event(serde_json::json!({ "type": "system", "subtype": "task_started", "task_id": task, "description": description,
                "subagent_type": "general-purpose", "is_backgrounded": true, "task_type": "local_agent" }));
        }
        event(serde_json::json!({ "type": "result", "subtype": "success", "duration_ms": 10 }));
        event(serde_json::json!({ "type": "system", "subtype": "task_notification", "task_id": "c3", "status": "completed" }));
        crate::background::exited_for_test(id, Some(0));

        let lost = "2 background subagents (\"Implement slice A\", \"Implement slice B\") ended with its turn without finishing; their work is not done. luna_session_send has it check what they left and carry on";
        let turn = Owed { told_pending: false, baseline: 3, cursor: 0, ..o };
        let turn = held(turn, 4, REPLY_GRACE_MS);
        let Some((Some(text), Settled::Failed, evidence)) =
            render_notice(pty, caller, "Your helper", id, &known, turn, &mut WorkLook::default())
        else {
            panic!("its turn is over")
        };
        assert!(text.starts_with("Your helper finished its turn") && text.contains(lost), "{text}");
        assert!(!text.contains("Implement slice C") && !text.contains(NOTHING_RUNNING), "{text}");
        // What a wait or a report hands over says it the same way.
        let note = crate::agents::owed::background_stopped_note(caller, id).0.expect("still to be said");
        assert!(note.starts_with(lost), "{note}");
        // The notice that said it is the caller's acknowledgment; one that did not is none.
        let settle = |evidence| {
            super::super::settle_delivered(&mut reg().lock().unwrap(), caller, id.to_string(), Settled::Failed, &turn, evidence, None)
        };
        settle(LineEvidence::default());
        assert!(crate::background::background_stopped_for(id, caller), "not said yet");
        reg().lock().unwrap().owed.insert((caller.to_string(), id.to_string()), turn);
        settle(evidence);
        assert!(!crate::background::background_stopped(id), "said to the one caller it concerns");

        // Killed, the turn's error comes first, and the subagents still go by name.
        let (known, o, _) = job_over_helper(id, caller, "");
        crate::background::set_running_for_test(id);
        event(serde_json::json!({ "type": "system", "subtype": "task_started", "task_id": "d4", "description": "Run the gate",
            "is_backgrounded": true, "task_type": "local_agent" }));
        crate::background::exited_for_test(id, None);
        let turn = Owed { told_pending: false, baseline: 3, cursor: 0, ..o };
        let turn = held(turn, 4, REPLY_GRACE_MS);
        let Some((Some(text), Settled::Failed)) = notice_line(pty, caller, "Your helper", id, &known, turn) else {
            panic!("its turn is over")
        };
        assert!(
            text.contains("stopped on an error")
                && text.contains("1 background subagent (\"Run the gate\") ended with its turn without finishing")
                && text.contains("Luna helpers for work that must outlive its turn"),
            "{text}"
        );

        // Stopped by Luna: that they were cut and their work is not done, and
        // nothing on how to work, which was no doing of the helper's.
        let (known, o, _) = job_over_helper(id, caller, "");
        crate::background::set_running_for_test(id);
        event(serde_json::json!({ "type": "system", "subtype": "task_started", "task_id": "e5", "description": "Write the docs",
            "is_backgrounded": true, "task_type": "local_agent" }));
        crate::background::luna_stopped_for_test(id);
        crate::background::exited_for_test(id, None);
        let turn = Owed { told_pending: false, baseline: 3, cursor: 0, ..o };
        let turn = held(turn, 4, REPLY_GRACE_MS);
        let Some((Some(text), Settled::Failed)) = notice_line(pty, caller, "Your helper", id, &known, turn) else {
            panic!("its turn is over")
        };
        let cut = "1 background subagent (\"Write the docs\") ended with its turn without finishing; their work is not done.";
        assert!(text.contains(cut) && !text.contains("Luna helpers") && !text.contains("waiting for any"), "{text}");
        crate::background::gone_for_test(id);
        forget(id);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn an_open_prompt_keeps_an_earlier_plain_turn_from_stalling() {
        let (id, caller) = ("open-prompt-helper", "open-prompt-caller");
        let (known, owed, path) = job_over_helper(id, caller, &said_line("Some progress."));
        let o = Owed { baseline: 2, told_pending: false, ..owed };
        crate::background::set_running_for_test(id);
        crate::background::hold_for_test(id);
        let pty = crate::pty::manager();
        let line = |o| notice_line(pty, caller, "Session", id, &known, o);
        assert!(matches!(line(o), Some((Some(_), Settled::Waiting))));
        assert!(line(Owed { told_waiting: true, ..o }).is_none(), "waiting is not idle");
        crate::background::idle_for_test(id, Provider::Claude, 4, false);
        assert!(matches!(line(o), Some((Some(_), Settled::Answered))), "the later idle can stall");
        crate::background::gone_for_test(id);
        forget(id);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_skipped_turn_survives_restart_with_its_answer_for_each_caller() {
        use crate::agents::owed::{owe_asked, Asked};
        let (id, parent, peer) = ("skip-restart-helper", "skip-restart-parent", "skip-restart-peer");
        let account = "C:/acc/anthropic/one";
        let pty = crate::pty::manager();
        let (known, path) = lead(parent, id, 1, true, &said_line("Intermediate evidence"));
        register(peer, Provider::Claude, "C:/p", account, None, true, serde_json::json!({}));
        for caller in [parent, peer] {
            owe_asked(caller, id, 0, 0, &Asked { text: "Investigate", key: None, reply: true });
            assert!(matches!(look(caller, id, &known), Some((None, Settled::Hold(1)))));
            assert!(matches!(look(caller, id, &known), Some((None, Settled::Skip(1, ..)))));
        }
        let saved = serde_json::to_vec(&ties_of(id).unwrap()).unwrap();
        forget(id);
        tie_back(id, Provider::Claude, "C:/p", account, serde_json::from_slice(&saved).unwrap());
        transcripts().lock().unwrap().insert(id.into(), path.clone());
        crate::background::idle_for_test(id, Provider::Claude, 2, false);
        std::fs::write(&path, [prompt_line("Investigate"), said_line("Intermediate evidence"), said_line("DONE: final result")].concat())
            .unwrap();
        for caller in [peer, parent] {
            let key = (caller.to_string(), id.to_string());
            let o = reg().lock().unwrap().owed[&key];
            assert_eq!((o.baseline, o.notice_baseline(), o.cursor), (0, 1, 0));
            let (Some(_), how @ Settled::Resumed) = notice_line(pty, caller, "Session", id, &known, o).unwrap() else {
                panic!("both callers hear what the core took")
            };
            super::super::settle_told(&mut reg().lock().unwrap(), key.clone(), how, &o);
            let o = reg().lock().unwrap().owed[&key];
            let (Some(line), _) = notice_line(pty, caller, "Session", id, &known, o).unwrap() else { panic!("both callers are owed") };
            assert!(line.contains("2 messages") && line.contains("DONE: final result"), "{line}");
            if let Some(file) = line.split("Report ").nth(1).and_then(|tail| tail.split(":1–").next()) {
                assert!(std::fs::read_to_string(file).unwrap().contains("Intermediate evidence"));
                let _ = std::fs::remove_file(file);
            } else {
                assert!(line.contains("Intermediate evidence"), "{line}");
            }
        }
        for id in [id, parent, peer] {
            crate::background::gone_for_test(id);
            forget(id);
        }
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn an_idle_plan_waits_through_an_fyi_and_reopens_for_an_independent_turn() {
        use crate::agents::owed::{owe_asked, owe_sent, Asked};
        let (id, caller) = ("plan-turn-helper", "plan-turn-caller");
        let initial = [prompt_line("Review changes"), said_line("PLAN: inspect the changes")].concat();
        let (known, path) = lead(caller, id, 3, false, &initial);
        owe_asked(caller, id, 2, 0, &Asked { text: "Review changes", key: None, reply: true });
        let key = (caller.to_string(), id.to_string());
        let pty = crate::pty::manager();
        let line = |o| notice_line(pty, caller, "Session", id, &known, o);
        let debt = || reg().lock().unwrap().owed[&key];
        let settle = |how, o| super::super::settle_told(&mut reg().lock().unwrap(), key.clone(), how, &o);
        let o = debt();
        let (Some(_), how @ Settled::Progress(_)) = line(o).unwrap() else { panic!("plan") };
        settle(how, o);
        let planned = debt();
        assert!(!planned.told_pending && !planned.active());
        assert!(line(planned).is_none());
        assert!(!reg().lock().unwrap().woken.contains_key(id));
        owe_sent(
            caller,
            id,
            3,
            (initial.len() as u64).into(),
            None,
            &Asked { text: "Heads up", key: None, reply: false },
            (false, None, true),
        );
        crate::background::quiet_for_test(id, 0);
        std::fs::write(&path, format!("{initial}{}", prompt_line("Heads up"))).unwrap();
        assert!(line(debt()).is_none(), "the FYI cannot reopen the plan");
        assert_eq!(debt().wake.plan_through, Some(3), "the delivery already marked the FYI taken");
        let news = [initial, prompt_line("Heads up"), said_line("Noted.")].concat();
        std::fs::write(&path, &news).unwrap();
        crate::background::idle_for_test(id, Provider::Claude, 4, false);
        let o = debt();
        let Some((None, how @ Settled::PlanFyi(..))) = line(o) else { panic!("the FYI stays readable without a stall") };
        settle(how, o);
        crate::background::quiet_for_test(id, 0);
        assert!(line(debt()).is_none(), "the independent turn has yet to end");
        std::fs::write(&path, format!("{news}{}{}", prompt_line("Go ahead"), said_line("Reviewed the changes."))).unwrap();
        crate::background::idle_for_test(id, Provider::Claude, 5, false);
        let (Some(text), Settled::Answered) = line(debt()).unwrap() else { panic!("later stall") };
        assert!(text.contains("finished its turn") && !text.contains("PLAN"), "{text}");
        crate::background::gone_for_test(id);
        forget(id);
        forget(caller);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_chat_that_exits_during_a_plan_continuation_is_reported() {
        let (id, caller) = ("plan-exit-chat", "plan-exit-caller");
        let initial = [prompt_line("Review changes"), said_line("PLAN: inspect the changes")].concat();
        let (mut known, path) = lead(caller, id, 1, false, &initial);
        known.parent = None;
        owe_from(caller, id, 0, 0);
        assert!(matches!(look(caller, id, &known), Some((Some(_), Settled::Progress(_)))));
        assert!(look(caller, id, &known).is_none());
        std::fs::write(&path, format!("{initial}{}", prompt_line("Go ahead"))).unwrap();
        crate::background::quiet_for_test(id, 0);
        // No notifier sample saw the independent turn run.
        crate::background::gone_for_test(id);
        let Some((Some(line), Settled::Exited)) = look(caller, id, &known) else { panic!("the exit must wake the caller") };
        assert!(line.contains("has exited"), "{line}");
        forget(id);
        forget(caller);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn status_before_the_prompt_does_not_cover_a_queued_or_joined_send() {
        use crate::agents::owed::{owe_asked, Asked};
        for (provider, queued) in [(Provider::Claude, true), (Provider::Claude, false), (Provider::Codex, true), (Provider::Codex, false)] {
            let id = format!("ask-{}-{queued}", provider.as_str());
            let caller = format!("caller-{}-{queued}", provider.as_str());
            let (id, caller) = (id.as_str(), caller.as_str());
            let key = (caller.to_string(), id.to_string());
            let (known, path) = lead_on(provider, caller, id, 1, false, "");
            let line_of = |role, text| match (provider, role) {
                (Provider::Claude, "user") => prompt_line(text),
                (Provider::Claude, _) => said_line(text),
                (Provider::Codex, _) => {
                    let kind = if role == "user" { "input_text" } else { "output_text" };
                    let v = serde_json::json!({ "type": "response_item", "payload": { "type": "message", "role": role, "content": [{ "type": kind, "text": text }] } });
                    format!("{v}\n")
                }
            };
            let ask = |text, reply| owe_asked(caller, id, 1, 0, &Asked { text, key: None, reply });
            ask("First task", true);
            ask("Next task", true);
            ask("Heads up", false);
            let first = [line_of("user", "First task"), line_of("assistant", "DONE: first task")].concat();
            let rest = [
                if provider == Provider::Claude {
                    format!(
                        "{}\n",
                        serde_json::json!({"type":"attachment","attachment":{
                            "type":"queued_command","prompt":"Next task","source_uuid":"joined-uuid"
                        }})
                    )
                } else {
                    line_of("user", "Next task")
                },
                line_of("assistant", "Here's the output."),
                line_of("user", "Heads up"),
                line_of("assistant", "OK."),
            ]
            .concat();
            let pty = crate::pty::manager();
            let line = |o, ended| notice_line(pty, caller, "Session", id, &known, held(o, ended, QUEUED_GRACE_MS));
            let debt = || reg().lock().unwrap().owed[&key];
            if queued {
                crate::background::idle_for_test(id, provider, 2, false);
                reg().lock().unwrap().owed.get_mut(&key).unwrap().queued = true;
                std::fs::write(&path, &first).unwrap();
                let old = debt();
                let Some((Some(_), how @ Settled::Progress(_))) = line(old, 2) else { panic!("first status") };
                super::super::settle_told(&mut reg().lock().unwrap(), key.clone(), how, &old);
                let remaining = requests_of(caller, id);
                assert_eq!(remaining.iter().map(|q| q.n).collect::<Vec<_>>(), [2, 3], "the first reply already arrived");
                assert!(!remaining[0].told);
                assert!(!remaining[0].taken);
            }
            std::fs::write(&path, format!("{first}{rest}")).unwrap();
            crate::background::idle_for_test(id, provider, if queued { 3 } else { 2 }, false);
            let Some((Some(text), Settled::Answered)) = line(debt(), if queued { 3 } else { 2 }) else { panic!("untagged answer") };
            assert!(text.contains("output") || text.contains("Report "), "{text}");
            crate::background::gone_for_test(id);
            forget(id);
            forget(caller);
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn an_earlier_failure_keeps_the_send_waiting_for_a_place() {
        use crate::agents::owed::{owe_asked, owe_sent, Asked};
        let (caller, id) = ("failure-queue-caller", "failure-queue-helper");
        let key = (caller.into(), id.into());
        let (known, path) = lead_on(Provider::Claude, caller, id, 1, false, "");
        owe_asked(caller, id, 0, 0, &Asked { text: "first request", reply: true, key: None });
        let first = prompt_line("first request");
        std::fs::write(&path, &first).unwrap();
        owe_sent(
            caller,
            id,
            1,
            (first.len() as u64).into(),
            None,
            &Asked { text: "waiting for a place", reply: true, key: None },
            (true, Some("queued-id".into()), false),
        );
        reg().lock().unwrap().owed.get_mut(&key).unwrap().cut_short = true;
        let pty = crate::pty::manager();
        let o = held(reg().lock().unwrap().owed[&key], 1, QUEUED_GRACE_MS.max(REPLY_GRACE_MS));
        let Some((Some(text), how)) = notice_line(pty, caller, "Session", id, &known, o) else { panic!("the first failure wakes") };
        assert!(text.contains("did NOT finish"), "{text}");
        super::super::settle_told(&mut reg().lock().unwrap(), key.clone(), how, &o);
        let left = reg().lock().unwrap().owed[&key];
        assert_eq!((left.baseline, left.cursor, left.queued), (1, first.len() as u64, true));
        let requests = requests_of(caller, id);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].n, 2);
        crate::background::gone_for_test(id);
        forget(id);
        forget(caller);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_lead_keeps_waiting_for_plan_approval_when_its_job_ends() {
        let (caller, id) = ("plan-job-caller", "plan-job-lead");
        let (known, path) = lead_on(Provider::Claude, caller, id, 1, false, "");
        let o = held(Owed::pending(1, 0), 1, BACKGROUND_GRACE_MS);
        let o = Owed { wake: super::super::super::owed::WakeState { plan_through: Some(1), job_running: true, ..o.wake }, ..o };
        let _ = notice_line(crate::pty::manager(), caller, "Lead", id, &known, o);
        assert!(!reg().lock().unwrap().woken.contains_key(id));
        crate::background::gone_for_test(id);
        forget(id);
        forget(caller);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn codex_status_wakes_an_fyi_caller_after_turn_end() {
        let (id, caller) = ("codex-status-helper", "codex-status-caller");
        register(id, Provider::Codex, "C:/p", "C:/acc/openai/one", Some(caller), false, serde_json::json!({}));
        crate::background::idle_for_test(id, Provider::Codex, 0, false);
        crate::background::quiet_for_test(id, 0);
        let path = std::env::temp_dir().join(format!("luna-{id}-{}.jsonl", std::process::id()));
        let message = r#"{"type":"response_item","payload":{"type":"message","role":"assistant","phase":"commentary","content":[{"type":"output_text","text":"BLOCKED: waiting for the deploy"}]}}"#;
        std::fs::write(&path, format!("{message}\n")).unwrap();
        transcripts().lock().unwrap().insert(id.into(), path.clone());
        let mut debt = Owed::from(0, 0);
        debt.no_reply = true;
        reg().lock().unwrap().owed.insert((caller.into(), id.into()), debt);
        let pty = crate::pty::manager();
        let known = reg().lock().unwrap().known[id].clone();
        let o = reg().lock().unwrap().owed[&(caller.into(), id.into())];
        assert!(notice_line(pty, caller, "Session", id, &known, o).is_none());
        crate::background::idle_for_test(id, Provider::Codex, 1, false);
        let ack = message.replace("BLOCKED: waiting for the deploy", &"Still waiting for the deploy. ".repeat(20));
        std::fs::write(&path, format!("{ack}\n")).unwrap();
        assert!(matches!(notice_line(pty, caller, "Session", id, &known, o), Some((None, Settled::Silent(1, true)))));
        std::fs::write(&path, format!("{message}\n")).unwrap();
        let (Some(text), Settled::Answered) = notice_line(pty, caller, "Session", id, &known, o).unwrap() else { panic!("ended report") };
        assert!(text.contains("BLOCKED"), "{text}");
        crate::background::gone_for_test(id);
        forget(id);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_slash_in_an_ordinary_request_does_not_hide_an_unmatched_reply() {
        let said = |head: &str| {
            Said::one_turn(vec!["Still investigating.".into()])
                .asked(vec![Request::new(1, None, "First request".into(), true), Request::new(2, None, head.into(), true)])
        };
        assert!(begins_re(&said("/tmp has logs"), false).contains("[unmatched]"));
        assert!(!begins_re(&said("<command-name>/model</command-name>"), false).contains("[unmatched]"));
    }

    /// A helper's turn that has answered but waits on a watcher it left: the
    /// quiet notice says so, and names the watcher. Before its result, the
    /// plain quiet notice.
    #[test]
    fn a_quiet_turn_waiting_on_its_background_names_what_it_waits_on() {
        use serde_json::json;
        let (caller, id) = ("bw-c", "bw-s");
        register(id, Provider::Claude, "C:\\p", "C:\\acc\\one", Some(caller), false, json!({}));
        crate::background::idle_for_test(id, Provider::Claude, 3, false);
        let o = Owed::from(3, 0);
        reg().lock().unwrap().owed.insert((caller.into(), id.into()), o);
        let known = reg().lock().unwrap().known.get(id).cloned().unwrap();
        let at = crate::background::iso(now_ms() - QUIET_TURN_MS - 60_000).unwrap();
        let call = json!({ "type": "assistant", "timestamp": at, "message": { "content": [{ "type": "tool_use", "id": "t1", "name": "Bash",
            "input": { "command": "npm run dev", "run_in_background": true } }] } });
        let started = json!({ "type": "user", "message": { "content": [{ "type": "tool_result", "tool_use_id": "t1", "content": "…" }] },
            "toolUseResult": { "backgroundTaskId": "b1" } });
        let path = std::env::temp_dir().join(format!("luna-{id}-{}.jsonl", std::process::id()));
        std::fs::write(&path, format!("{call}\n{started}\n")).unwrap();
        transcripts().lock().unwrap().insert(id.into(), path.clone());
        crate::background::quiet_for_test(id, QUIET_TURN_MS + 60_000);
        let pty = crate::pty::manager();
        let Some((Some(text), Settled::Quiet(3))) = notice_line(pty, caller, "Your helper", id, &known, o) else { panic!("quiet") };
        assert!(text.contains("written nothing for 31 min: a command it runs may hang"), "not answered yet: {text}");
        // The plain notice told for this turn; then the turn answers and waits
        // on its watcher: that is news of its own.
        let told = Owed { told_quiet: Some(3), ..o };
        assert!(matches!(notice_line(pty, caller, "Your helper", id, &known, told), None | Some((None, _))), "told once");
        crate::background::after_result_for_test(id);
        let Some((Some(text), Settled::WaitsOnBackground(3))) = notice_line(pty, caller, "Your helper", id, &known, told) else {
            panic!("waiting on its background is news")
        };
        assert!(text.contains("answered but its turn waits on background work"), "{text}");
        assert!(text.contains("31 min quiet): Bash `npm run dev`."), "{text}");
        let both = Owed { told_background_wait: Some(3), ..told };
        assert!(matches!(notice_line(pty, caller, "Your helper", id, &known, both), None | Some((None, _))), "and that once too");
        // What its job holds is named, the CLI aside; nothing of it is
        // measured, so its stream's quiet is the turn's.
        crate::background::watched_for_test(id, &[("claude.exe", 1), ("node.exe", 2)]);
        let Some((Some(text), Settled::WaitsOnBackground(3))) = notice_line(pty, caller, "Your helper", id, &known, told) else {
            panic!("its stream quiet")
        };
        assert!(text.contains("31 min quiet): Bash `npm run dev`; processes claude.exe, node.exe."), "{text}");
        assert!(text.contains("end with luna_session_stop"), "{text}");
        // A subagent among what it waits on: said without calling it a hang.
        let agent = json!({ "type": "assistant", "timestamp": at, "message": { "content": [{ "type": "tool_use", "id": "t2", "name": "Agent",
            "input": { "description": "port the parser", "prompt": "…", "run_in_background": true } }] } });
        let launched = json!({ "type": "user", "message": { "content": [{ "type": "tool_result", "tool_use_id": "t2", "content": "…" }] },
            "toolUseResult": { "isAsync": true, "status": "async_launched", "agentId": "a1" } });
        std::fs::write(&path, format!("{call}\n{started}\n{agent}\n{launched}\n")).unwrap();
        let Some((Some(text), _)) = notice_line(pty, caller, "Your helper", id, &known, told) else { panic!("still news") };
        assert!(text.contains("subagent \"port the parser\"") && text.contains("may still be thinking"), "{text}");
        crate::background::gone_for_test(id);
        forget(id);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_jobs_end_brings_what_was_said_on_notices_since() {
        let prompt = "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"[Luna notice]\"}}\n";
        let result = "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"Here are the real results.\"}]}}\n";
        let (known, o, path) = job_over_helper("jr-s", "jr-c", &format!("{prompt}{result}"));
        let pty = crate::pty::manager();
        // As `wait` puts it for `job_done`.
        assert_eq!(
            reply_text("jr-s", &said_since(pty, "jr-s", Provider::Claude, o.cursor), true).as_deref(),
            Some("Here are the real results.")
        );
        // The same as the notice brings: nothing is settled without its report.
        let mut stood = o;
        stood.held = Some((3, now_ms() - BACKGROUND_GRACE_MS));
        let Some((Some(line), Settled::Answered)) = notice_line(pty, "jr-c", "Your helper", "jr-s", &known, stood) else {
            panic!("the job's end is news")
        };
        assert!(line.contains("it wrote 1 message since your last notice"), "{line}");
        // Nothing since: nothing to hand over, not an old message.
        let (_, o2, path2) = job_over_helper("jr-q", "jr-c", "");
        assert_eq!(reply_text("jr-q", &said_since(pty, "jr-q", Provider::Claude, o2.cursor), true), None);
        for id in ["jr-s", "jr-q"] {
            crate::background::gone_for_test(id);
            forget(id);
        }
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&path2);
    }

    #[test]
    fn the_breaker_lets_so_many_notices_through_and_stops_the_pair() {
        let (caller, id) = ("br-a", "br-b");
        let pty = crate::pty::manager();
        let t0 = now_ms();
        for i in 0..BREAKER_NOTICES - 1 {
            assert!(!pair_stopped(pty, caller, id));
            assert!(!last_notice(caller, id, t0));
            assert!(!count_notice(&mut reg().lock().unwrap(), caller, id, t0), "notice {i}");
        }
        assert!(last_notice(caller, id, t0), "it says so as it goes");
        assert!(count_notice(&mut reg().lock().unwrap(), caller, id, t0));
        assert!(pair_stopped(pty, caller, id), "nothing more for this pair");
        assert!(!pair_stopped(pty, id, caller), "the other way round is a pair of its own");
        forget(caller);
        assert!(!pair_stopped(pty, caller, id), "gone with the chat");
    }

    #[test]
    fn a_stopped_pair_still_hears_of_prompts_and_exits_and_nothing_else() {
        let (known, o, path) = job_over_helper("bp-s", "bp-c", "");
        let pty = crate::pty::manager();
        let t0 = now_ms();
        for _ in 0..BREAKER_NOTICES {
            count_notice(&mut reg().lock().unwrap(), "bp-c", "bp-s", t0);
        }
        assert!(pair_stopped(pty, "bp-c", "bp-s"));
        // A job's end or a turn's, which is all the notifier would have for a
        // stopped pair: nothing.
        assert!(standing_line(pty, "Your helper", "bp-s", &known, o).is_none());
        // Held on a prompt, it is told, or the helper sits there until it times out.
        crate::background::in_turn_for_test("bp-s");
        let asks = std::thread::spawn(|| {
            crate::background::permission("bp-s", &serde_json::json!({ "tool_name": "Bash", "input": { "command": "ls" } }))
        });
        for _ in 0..200 {
            if ask_of("bp-s").is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let Some((Some(line), Settled::Waiting)) = standing_line(pty, "Your helper", "bp-s", &known, o) else {
            panic!("a prompt goes through")
        };
        assert!(line.contains("asks for permission to run Bash: ls") && line.contains("askId"), "{line}");
        crate::background::kill("bp-s");
        let _ = asks.join();
        // And gone is gone.
        crate::background::gone_for_test("bp-s");
        assert!(matches!(standing_line(pty, "Your helper", "bp-s", &known, o), Some((Some(_), Settled::Exited))));
        forget("bp-s");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_breaker_counts_only_what_falls_within_its_window() {
        let (caller, id) = ("bw-a", "bw-b");
        let t0 = now_ms();
        for _ in 0..BREAKER_NOTICES - 1 {
            assert!(!count_notice(&mut reg().lock().unwrap(), caller, id, t0));
        }
        // A notice now and then over a long job, as real use has it.
        let later = t0 + BREAKER_WINDOW_MS;
        assert!(!last_notice(caller, id, later), "the old ones have aged out");
        assert!(!count_notice(&mut reg().lock().unwrap(), caller, id, later));
        forget(caller);
    }

    /// `notice_line` on a session known only by its hooks, the debt past the
    /// grace it gives a turn with no reply yet.
    fn line_for(id: &str) -> String {
        let pty = crate::pty::manager();
        let known = reg().lock().unwrap().known.get(id).cloned().unwrap();
        let mut o = Owed::from(0, 0);
        o.held = Some((1, now_ms() - REPLY_GRACE_MS));
        match notice_line(pty, "its-caller", "Your helper", id, &known, o) {
            Some((Some(line), Settled::Answered | Settled::Failed)) => line,
            _ => panic!("a turn is over: there is something to say"),
        }
    }

    #[test]
    fn a_turn_that_failed_is_not_reported_as_finished() {
        use crate::activity::{on_hook, HookEvent};
        let none = serde_json::json!({});
        register("fl-limit", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, false, none.clone());
        on_hook("fl-limit", HookEvent::Busy);
        on_hook("fl-limit", HookEvent::Failed("rate_limit"));
        let line = line_for("fl-limit");
        assert!(line.starts_with("Your helper stopped on an error: rate_limit."), "{line}");
        assert!(!line.contains("finished"), "{line}");
        // No error known and nothing to read: that it is not known is what it says.
        register("fl-lost", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, false, none);
        on_hook("fl-lost", HookEvent::Busy);
        on_hook("fl-lost", HookEvent::Idle);
        let line = line_for("fl-lost");
        assert!(line.contains("cannot find its transcript"), "{line}");
        assert!(!line.contains("without a reply"), "{line}");
        forget("fl-limit");
        forget("fl-lost");
        crate::activity::forget("fl-limit");
        crate::activity::forget("fl-lost");
    }

    /// A turn that stopped short hands over what it said before that, whatever
    /// came after it; and a line the CLI put in itself (the end of a background
    /// task it takes up) is no prompt whose reply is still to come.
    #[test]
    fn what_a_turn_said_comes_with_its_notice_whatever_came_after() {
        use crate::activity::{on_hook, HookEvent};
        let user = |t: &str| format!("{}\n", serde_json::json!({ "type": "user", "message": { "role": "user", "content": t } }));
        let said = |t: &str| {
            format!("{}\n", serde_json::json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": t }] } }))
        };
        let task = format!(
            "{}\n",
            serde_json::json!({ "type": "user", "message": { "role": "user", "content": "<task-notification>\n<task-id>b1</task-id>\n</task-notification>" },
                "origin": { "kind": "task-notification" }, "promptSource": "system", "turnOrigin": "task_notification" })
        );
        let worked = format!("{}{}{}", user("fix it"), said("Read the code."), said("Half the fix is in."));
        let pty = crate::pty::manager();
        let line_of = |id: &str, transcript: String, cut: bool| -> String {
            let path = std::env::temp_dir().join(format!("luna-{id}-{}.jsonl", std::process::id()));
            std::fs::write(&path, transcript).unwrap();
            transcripts().lock().unwrap().insert(id.into(), path.clone());
            let known = reg().lock().unwrap().known.get(id).cloned().unwrap();
            let mut o = Owed::from(0, 0);
            o.cut_short = cut;
            o.held = Some((1, now_ms() - REPLY_GRACE_MS));
            let Some((Some(line), Settled::Answered | Settled::Failed)) = notice_line(pty, "ws-caller", "Your helper", id, &known, o)
            else {
                panic!("its turn is over")
            };
            let _ = std::fs::remove_file(&path);
            line
        };
        let helper = |id: &str| {
            register(id, Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, false, serde_json::json!({}));
            crate::background::idle_for_test(id, Provider::Claude, 1, false);
        };

        // Cut short on the way to what was queued behind its turn.
        helper("ws-cut");
        let line = line_of("ws-cut", format!("{worked}{}", user("then the tests")), true);
        assert!(line.contains("did NOT finish") && line.contains("It ended with 2 messages since you asked"), "{line}");
        // The same transcript with nothing cut, past the grace for a reply to
        // the prompt last: what it wrote before that prompt is still told.
        helper("ws-open");
        let line = line_of("ws-open", format!("{worked}{}", user("then the tests")), false);
        assert!(line.contains("finished its turn with 2 messages since you asked") && !line.contains("without a reply"), "{line}");
        // Claude Code taking up the end of its background task wrote that line itself.
        helper("ws-task");
        let line = line_of("ws-task", format!("{worked}{task}"), true);
        assert!(line.contains("It ended with 2 messages since you asked"), "{line}");
        let line = line_of("ws-task", format!("{worked}{task}"), false);
        assert!(line.contains("finished its turn with 2 messages since you asked"), "{line}");
        // Stopped on an error: what it said before that, and the error's own stop.
        register("ws-fail", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, false, serde_json::json!({}));
        on_hook("ws-fail", HookEvent::Busy);
        on_hook("ws-fail", HookEvent::Failed("rate_limit"));
        let line = line_of("ws-fail", format!("{worked}{}", user("then the tests")), false);
        assert!(line.starts_with("Your helper stopped on an error: rate_limit. It ended with 2 messages since you asked"), "{line}");
        for id in ["ws-cut", "ws-open", "ws-task", "ws-fail"] {
            crate::background::gone_for_test(id);
            forget(id);
        }
        crate::activity::forget("ws-fail");
        assert_eq!(sentence("pick a different model."), "pick a different model.");
        assert_eq!(sentence("rate_limit"), "rate_limit.");
        assert_eq!(sentence("cut short…"), "cut short…");
    }

    #[test]
    fn a_transcript_on_disk_with_no_reply_is_said_as_that() {
        use crate::activity::{on_hook, HookEvent};
        let path = std::env::temp_dir().join(format!("luna-no-reply-{}.jsonl", std::process::id()));
        std::fs::write(&path, "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"go\"}}\n").unwrap();
        register("nr-chat", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, false, serde_json::json!({}));
        transcripts().lock().unwrap().insert("nr-chat".into(), path.clone());
        on_hook("nr-chat", HookEvent::Busy);
        on_hook("nr-chat", HookEvent::Idle);
        let line = line_for("nr-chat");
        assert!(line.contains("finished its turn without a reply"), "{line}");
        forget("nr-chat");
        crate::activity::forget("nr-chat");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn text_queued_as_the_core_went_comes_back_with_the_notice() {
        let none = serde_json::json!({});
        register("ql-child", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("ql-parent"), false, none);
        crate::background::take_up_for_test("ql-child", "C:\\p", 3, &["first line\nsecond line", "and another"], None);
        owe_from("ql-parent", "ql-child", 3, 0);
        let (line, how) = notice_for("ql-parent", "ql-child", 5);
        let line = line.unwrap();
        assert!(matches!(how, Settled::Failed));
        assert!(line.contains("did NOT finish") && line.contains("Not delivered: 2 messages queued for it"), "{line}");
        let file = line.split(", in ").nth(1).and_then(|l| l.split(';').next()).unwrap();
        assert!(line.replace(file, "").len() < 360, "a notice is typed into a CLI: {line}");
        let held = std::fs::read_to_string(file).unwrap();
        let _ = std::fs::remove_file(file);
        assert!(held.starts_with("NOT DELIVERED: 2 messages"), "{held}");
        let (first, second) = (held.find("first line\nsecond line").unwrap(), held.find("and another").unwrap());
        assert!(first < second, "word for word, oldest first: {held}");

        crate::background::remove("ql-child");
        forget("ql-child");
    }

    #[test]
    fn a_helper_that_never_got_started_says_so_and_hands_back_its_prompt() {
        register("ns-child", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("ns-parent"), false, serde_json::json!({}));
        crate::background::take_up_for_test("ns-child", "C:\\p", 0, &["and a follow-up"], Some("do the whole job"));
        owe_from("ns-parent", "ns-child", 0, 0);
        let (line, how) = notice_for("ns-parent", "ns-child", 2);
        let line = line.unwrap();
        assert!(matches!(how, Settled::Failed));
        assert!(
            line.contains("never got started")
                && line.contains("its prompt was lost")
                && line.contains("luna_session_send starts it afresh"),
            "{line}"
        );
        assert!(line.contains("its prompt and 1 queued message are in "), "{line}");
        let file = line.split(" are in ").nth(1).and_then(|l| l.split(". luna_session_send").next()).unwrap();
        let held = std::fs::read_to_string(file).unwrap();
        let _ = std::fs::remove_file(file);
        assert!(held.find("do the whole job").unwrap() < held.find("and a follow-up").unwrap(), "{held}");
        // Still there to delete, which is what writes its end down.
        assert!(reg().lock().unwrap().known.contains_key("ns-child"));
        crate::background::remove("ns-child");
        forget("ns-child");
    }

    #[test]
    fn a_helper_told_it_was_not_done_is_heard_of_past_a_core_that_took_its_job() {
        crate::background::idle_for_test("tp-helper", Provider::Codex, 3, false);
        // As the last core saved it.
        let ties = Ties {
            parent: Some("tp-caller".into()),
            settings: serde_json::json!({ "background": true }),
            owed: vec![("tp-caller".into(), 3, 40)],
            told_pending: vec!["tp-caller".into()],
            ..Default::default()
        };
        let ties: Ties = serde_json::from_slice(&serde_json::to_vec(&ties).unwrap()).unwrap();
        tie_back("tp-helper", Provider::Codex, "C:\\p", "C:\\acc\\openai\\two", ties);
        let key = ("tp-caller".to_string(), "tp-helper".to_string());
        let o = reg().lock().unwrap().owed[&key];
        assert!(o.told_pending && o.job_went);
        // Not a turn in sight and nothing running: its job's end is all there will be.
        assert_eq!(verdict(&o, 3, Some(crate::activity::Turn::Idle), false, false), Verdict::JobOver);
        let (k, pty) = (reg().lock().unwrap().known["tp-helper"].clone(), crate::pty::manager());
        let held = Owed { held: Some((3, now_ms() - BACKGROUND_GRACE_MS)), ..o };
        let (line, how) = notice_line(pty, "tp-caller", "Your helper", "tp-helper", &k, held).unwrap();
        let line = line.unwrap();
        assert!(matches!(how, Settled::Resumed));
        assert!(line.contains("cut off its background work"), "{line}");
        // And it comes back marked again from what this core saves.
        assert_eq!(ties_of("tp-helper").unwrap().told_pending, vec!["tp-caller".to_string()]);
        crate::background::gone_for_test("tp-helper");
        forget("tp-helper");
    }

    /// An assistant line of a Claude Code transcript.
    #[test]
    fn restart_interruptions_are_told_once_even_with_a_live_delegate() {
        let (top, lead, child) = ("restart-fact-top", "restart-fact-lead", "restart-fact-child");
        let (known, path) = lead_on(Provider::Claude, top, lead, 1, false, &said_line("I am waiting."));
        crate::background::test_helper(child, "C:/acc/one", None, true);
        owe_from(lead, child, 0, 0);
        owe_from(top, lead, 0, 0);
        let key = (top.into(), lead.into());
        {
            let mut r = reg().lock().unwrap();
            let o = r.owed.get_mut(&key).unwrap();
            o.job_went = true;
            o.resumed = true;
        }
        let pty = crate::pty::manager();
        let o = reg().lock().unwrap().owed[&key];
        let result = notice_line(pty, top, "Your helper", lead, &known, o);
        let Some((Some(line), how @ Settled::Resumed)) = result else { panic!("untold interruption") };
        assert!(line.contains("cut off its turn and background work"));
        super::super::settle_told(&mut reg().lock().unwrap(), key.clone(), how, &o);
        let o = reg().lock().unwrap().owed[&key];
        assert!(o.active());
        assert!(!matches!(notice_line(pty, top, "Your helper", lead, &known, o), Some((Some(_), Settled::Resumed))));
        for id in [lead, child, top] {
            forget(id);
            crate::background::gone_for_test(id);
        }
        let _ = std::fs::remove_file(path);
    }

    fn said_line(text: &str) -> String {
        format!("{}\n", serde_json::json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": text }] } }))
    }

    /// A prompt line of a Claude Code transcript.
    fn prompt_line(text: &str) -> String {
        format!("{}\n", serde_json::json!({ "type": "user", "message": { "role": "user", "content": text } }))
    }

    /// A lead under `top`, a background helper at `turns` turns with a job
    /// left running or not, its transcript `text`.
    fn lead(top: &str, id: &str, turns: u32, job: bool, text: &str) -> (Known, std::path::PathBuf) {
        lead_on(Provider::Claude, top, id, turns, job, text)
    }

    /// `lead` on `provider`'s CLI, its transcript in that CLI's format.
    fn lead_on(provider: Provider, top: &str, id: &str, turns: u32, job: bool, text: &str) -> (Known, std::path::PathBuf) {
        let account = if provider == Provider::Claude { "C:\\acc\\anthropic\\one" } else { "C:\\acc\\openai\\one" };
        register(top, provider, "C:\\p", account, None, true, serde_json::json!({}));
        register(id, provider, "C:\\p", account, Some(top), true, serde_json::json!({ "background": true }));
        crate::background::idle_for_test(id, provider, turns, false);
        if job {
            crate::background::left_for_test(id, &[("bash.exe", 4242)]);
        }
        let path = std::env::temp_dir().join(format!("luna-{id}-{}.jsonl", std::process::id()));
        std::fs::write(&path, text).unwrap();
        transcripts().lock().unwrap().insert(id.into(), path.clone());
        (reg().lock().unwrap().known.get(id).cloned().unwrap(), path)
    }

    fn held(o: Owed, at: u32, grace: u64) -> Owed {
        Owed { held: Some((at, now_ms() - grace)), ..o }
    }

    /// The night of 13 November: four leads wrote "still working" and "NOT
    /// done yet" reached their parent each time. While a lead's delegates
    /// work its turns wait in its report; a status word gets one through; and
    /// once they are done its parent hears all it wrote since.
    #[test]
    fn a_leads_turns_wait_while_its_delegates_work_unless_a_status_word_opens_one() {
        let (top, id) = ("lw-top", "lw-lead");
        let interim = format!("Gate started, both checkers out; nothing for you yet. {}", "Detail. ".repeat(60));
        let (known, path) = lead(top, id, 1, true, &said_line(&interim));
        let pty = crate::pty::manager();
        let line = |o: Owed| notice_line(pty, top, "Your helper", id, &known, o);
        let first = held(Owed::from(0, 0), 1, BACKGROUND_GRACE_MS);
        assert!(matches!(line(first), Some((None, Settled::Skip(1, ..)))), "no notice, and no NOT done yet: that is a lead's normal state");
        // As the notifier leaves it: told it is not done, what it said still to come.
        let waiting = Owed { rev: first.rev, ..Owed::pending(1, 0) };
        assert!(!matches!(line(waiting), Some((Some(_), _))), "nothing to say while the job runs");
        // A status word gets through at once, delegates or not, and is no NOT done yet.
        for word in ["DONE", "BLOCKED: the gate", "QUESTION which branch?", "FAILED:", "PLAN: three lanes", "**DONE**: all of it"] {
            std::fs::write(&path, said_line(&format!("{word}\nmore"))).unwrap();
            let Some((Some(text), Settled::Pending(1, ..) | Settled::Progress(_))) = line(first) else { panic!("{word} goes at once") };
            assert!(text.contains("ended a turn") && !text.contains("NOT done yet"), "{text}");
            assert!(text.contains("[job running"), "{text}");
        }
        for not_one in ["done: lowercase", "DONEZO", "Status: DONE", "> DONE: quoted"] {
            std::fs::write(&path, said_line(not_one)).unwrap();
            assert!(matches!(line(first), Some((None, Settled::Skip(1, ..)))), "{not_one}");
        }
        // A send's first turn and an FYI both wait while work continues.
        std::fs::write(&path, said_line("Lane 2 of 3; the gate is still out.")).unwrap();
        assert!(matches!(line(first), Some((None, Settled::Skip(1, ..)))), "a sent task's first turn");
        let quiet = Owed { no_reply: true, ..first };
        assert!(matches!(line(quiet), Some((None, Settled::Skip(1, ..)))), "an FYI");
        // A prompt it stops on comes as ever.
        crate::background::set_running_for_test(id);
        crate::background::hold_for_test(id);
        assert!(matches!(line(waiting), Some((Some(_), Settled::Waiting))), "a permission is never held back");
        crate::background::idle_for_test(id, Provider::Claude, 1, false);
        // Its delegates done: everything it wrote since its parent last heard.
        std::fs::write(&path, said_line(&interim)).unwrap();
        let Some((Some(text), Settled::Answered)) = line(held(waiting, 1, BACKGROUND_GRACE_MS)) else {
            panic!("the end reaches the parent")
        };
        assert!(text.contains("has nothing left running now, and it wrote 1 message since your last notice"), "{text}");
        // So does a turn of its own once nothing of its is out.
        crate::background::idle_for_test(id, Provider::Claude, 2, false);
        let Some((Some(text), Settled::Answered)) = line(held(waiting, 2, REPLY_GRACE_MS)) else { panic!("a quiet subtree's turn end") };
        assert!(text.contains("finished its turn"), "{text}");
        // Exits come as ever.
        crate::background::gone_for_test(id);
        assert!(matches!(line(waiting), Some((Some(_), Settled::Exited))));
        for s in [id, top] {
            forget(s);
        }
        let _ = std::fs::remove_file(&path);
    }

    /// What the notifier does with what a look at `id` for `top` came to, as
    /// its loop and a notice's delivery do: a hold starts the grace (put back
    /// past it here), and the rest settles (`settle_told`). The look's result.
    fn look(top: &str, id: &str, known: &Known) -> Option<(Option<String>, Settled)> {
        let key = (top.to_string(), id.to_string());
        let o = reg().lock().unwrap().owed[&key];
        let said = notice_line(crate::pty::manager(), top, "Your helper", id, known, o);
        match &said {
            Some((None, Settled::Hold(n))) => {
                reg().lock().unwrap().owed.get_mut(&key).unwrap().held = Some((*n, now_ms() - BACKGROUND_GRACE_MS.max(REPLY_GRACE_MS)))
            }
            Some((_, how)) => crate::agents::notifier::settle_told(&mut reg().lock().unwrap(), key, how.clone(), &o),
            _ => {}
        }
        said
    }

    /// Live on 13 November: a lead wrote "Progress: the ping runs." and ended
    /// its turn with its job at work, then went quiet; once the job was over its
    /// parent was told it "wrote 2 messages", and the lead sat idle. What it
    /// wrote while its delegates worked waits for its parent and is no word on
    /// their end: Luna wakes the lead, once, and its parent hears of the turn
    /// that brings, all it wrote included, or that it seems stuck if that turn
    /// says nothing.
    #[test]
    fn a_lead_whose_delegates_finished_is_woken_once_and_its_parent_hears_of_what_came_of_it() {
        use crate::agents::owed::{owe_asked, Asked};
        let (top, id) = ("wk-top", "wk-lead");
        let interim = [prompt_line("Run the ping and report"), said_line("Progress: the ping runs.")].concat();
        // Its first turn ends on that, its job still at work.
        let (known, path) = lead(top, id, 1, true, &interim);
        owe_asked(top, id, 0, 0, &Asked { text: "Run the ping and report", key: None, reply: true });
        let wakes = || reg().lock().unwrap().news.iter().filter(|(c, l)| c == id && is_wake(l)).count();
        assert!(matches!(look(top, id, &known), Some((None, Settled::Hold(1)))));
        assert!(matches!(look(top, id, &known), Some((None, Settled::Skip(1, ..)))), "held back from its parent");
        assert!(look(top, id, &known).is_none_or(|(line, _)| line.is_none()), "nothing while the job runs");
        // The job ends; the lead says nothing more.
        crate::background::left_for_test(id, &[]);
        assert!(matches!(look(top, id, &known), Some((None, Settled::Hold(1)))));
        assert!(look(top, id, &known).is_none(), "its parent hears nothing yet");
        assert_eq!(wakes(), 1, "the lead is woken");
        let wake = reg().lock().unwrap().news.iter().find(|(c, _)| c == id).map(|(_, l)| l.clone()).unwrap();
        assert!(wake.starts_with("Your delegates have finished: processes bash.exe"), "{wake}");
        assert!(look(top, id, &known).is_none());
        assert_eq!(wakes(), 1, "once per end");
        // Its turn on the wake.
        reg().lock().unwrap().news.retain(|(c, _)| c != id);
        notice_turn(&mut reg().lock().unwrap(), id, 2, &[], true);
        crate::background::idle_for_test(id, Provider::Claude, 2, false);
        // One that takes it up is heard of, with what it wrote before.
        std::fs::write(&path, format!("{interim}{}", said_line("DONE: the ping ran clean."))).unwrap();
        let o = reg().lock().unwrap().owed[&(top.to_string(), id.to_string())];
        let Some((Some(done), Settled::Answered)) = notice_line(crate::pty::manager(), top, "Your helper", id, &known, o) else {
            panic!("its turn on the wake")
        };
        assert!(done.contains("finished its turn with 2 messages since you asked") && done.contains("DONE: the ping ran clean"), "{done}");
        // One that says nothing: stuck, said once, as it settles the debt,
        // with the background work its CLI cut off, which that settles too.
        std::fs::write(&path, &interim).unwrap();
        crate::background::background_stopped_for_test(id, &[top]);
        let Some((Some(stuck), Settled::Failed)) = look(top, id, &known) else { panic!("its parent hears") };
        assert!(stuck.contains("seems stuck") && stuck.contains("Progress: the ping runs."), "{stuck}");
        assert!(stuck.contains(BACKGROUND_STOPPED), "{stuck}");
        assert!(!reg().lock().unwrap().owed.contains_key(&(top.to_string(), id.to_string())), "settled");
        // Stopped by the breaker, it is not woken, and its parent hears as before.
        let told = Owed { held_to: Some(interim.len() as u64), ..Owed::pending(1, 0) };
        reg().lock().unwrap().owed.insert((top.into(), id.into()), told);
        reg().lock().unwrap().woken.remove(id);
        reg().lock().unwrap().stopped.insert((id.into(), id.into()), now_ms());
        crate::background::idle_for_test(id, Provider::Claude, 1, false);
        let over = held(told, 1, BACKGROUND_GRACE_MS);
        let line = |o: Owed| notice_line(crate::pty::manager(), top, "Your helper", id, &known, o);
        assert!(matches!(line(over), Some((None, Settled::Silent(..)))), "no word from the lead when the job ended");
        assert_eq!(wakes(), 0);
        reg().lock().unwrap().stopped.remove(&(id.to_string(), id.to_string()));
        // A wake not taken up in WAKE_WAIT_MS: its parent hears.
        reg().lock().unwrap().woken.insert(id.into(), Woken { ended: 1, cursor: 0, at: now_ms() - WAKE_WAIT_MS });
        assert!(matches!(line(over), Some((None, Settled::Silent(..)))));
        // A turn a wake started counts as the lead's own for every caller.
        assert!(real_turn_past(id, top, 1, 2, true, || false));
        reg().lock().unwrap().owed.remove(&(top.to_string(), id.to_string()));
        for s in [id, top] {
            crate::background::gone_for_test(s);
            forget(s);
        }
        let _ = std::fs::remove_file(&path);
    }

    /// A lead owed to its parent and to a peer that sent to it: the parent's
    /// look wakes it, and the peer, whose debt was never told it is not done,
    /// hears of the same end without one. That notice settling must leave the
    /// wake standing, or the parent's next look wakes the lead again for one
    /// end; told of the turn the wake brought, it is behind it.
    #[test]
    fn a_lead_owed_to_two_callers_is_woken_once_for_one_end() {
        let (id, parent, peer) = ("w2-lead", "w2-parent", "w2-peer");
        crate::background::idle_for_test(id, Provider::Claude, 2, false);
        reg().lock().unwrap().woken.insert(id.into(), Woken { ended: 2, cursor: 0, at: now_ms() });
        let wakes = || reg().lock().unwrap().news.iter().filter(|(c, l)| c == id && is_wake(l)).count();
        let settle = |caller: &str, ended: u32| {
            let told = Owed::from(1, 0);
            reg().lock().unwrap().owed.insert((caller.into(), id.into()), told);
            super::super::settle_delivered(
                &mut reg().lock().unwrap(),
                caller,
                id.into(),
                Settled::Answered,
                &told,
                LineEvidence::default(),
                Some(ended),
            );
        };
        settle(peer, 2);
        assert!(reg().lock().unwrap().woken.get(id).is_some_and(|w| w.ended == 2), "the wake stands");
        assert_eq!(wake_lead(crate::pty::manager(), id, 2), Wake::Waits);
        assert_eq!(wakes(), 0, "not woken again for the same end");
        settle(parent, 3);
        assert!(!reg().lock().unwrap().woken.contains_key(id), "its turn on the wake told: behind it");
        crate::background::gone_for_test(id);
        for s in [id, parent, peer] {
            forget(s);
        }
    }

    /// Live on 13 November: a Codex lead's helper answered, and the lead's
    /// turn on that notice said nothing; its parent heard it "finished its
    /// turn without a reply", and nothing went on. A lead whose own helpers
    /// have answered and that says nothing of them is woken as for a job's end.
    #[test]
    fn a_lead_silent_on_its_helpers_answers_is_woken() {
        use crate::agents::owed::{owe_asked, Asked};
        let (top, id, low) = ("wh-top", "wh-lead", "wh-low");
        let interim = [prompt_line("Get the branch reviewed"), said_line("Progress: the review is out.")].concat();
        let (known, path) = lead(top, id, 1, false, &interim);
        register(low, Provider::Codex, "C:\\p", "C:\\acc\\openai\\one", Some(id), false, serde_json::json!({ "background": true }));
        owe_asked(top, id, 0, 0, &Asked { text: "Get the branch reviewed", key: None, reply: true });
        owe_asked(id, low, 0, 0, &Asked { text: "Review the branch", key: None, reply: true });
        let wakes = || reg().lock().unwrap().news.iter().filter(|(c, l)| c == id && is_wake(l)).count();
        // Its first turn ends with its helper out: held back.
        assert!(matches!(look(top, id, &known), Some((None, Settled::Skip(1, ..)))), "held back from its parent");
        // The helper answers, a notice tells the lead, and its turn on that says nothing.
        let helper_key = (id.to_string(), low.to_string());
        let owed_low = reg().lock().unwrap().owed[&helper_key];
        crate::agents::notifier::settle_told(&mut reg().lock().unwrap(), helper_key, Settled::Answered, &owed_low);
        notice_turn(&mut reg().lock().unwrap(), id, 2, &[low.to_string()], false);
        crate::background::idle_for_test(id, Provider::Claude, 2, false);
        assert!(look(top, id, &known).is_none(), "its parent hears nothing yet");
        assert_eq!(wakes(), 1, "the lead is woken");
        let wake = reg().lock().unwrap().news.iter().find(|(c, _)| c == id).map(|(_, l)| l.clone()).unwrap();
        assert!(wake.contains("helper wh-low"), "{wake}");
        // Its turn on the wake takes them up: its parent hears of it.
        reg().lock().unwrap().news.retain(|(c, _)| c != id);
        notice_turn(&mut reg().lock().unwrap(), id, 3, &[], true);
        crate::background::idle_for_test(id, Provider::Claude, 3, false);
        std::fs::write(&path, format!("{interim}{}", said_line("DONE: reviewed, two fixes in."))).unwrap();
        let Some((Some(done), Settled::Answered)) = look(top, id, &known) else { panic!("its turn on the wake") };
        assert!(done.contains("finished its turn") && done.contains("DONE: reviewed"), "{done}");
        reg().lock().unwrap().woken.remove(id);
        for s in [low, id, top] {
            crate::background::gone_for_test(s);
            forget(s);
        }
        let _ = std::fs::remove_file(&path);
    }

    /// Live on 13 November (lead-d-codex): a Codex lead ended its interim
    /// turn ("Helper is running…") and its helper answered 2 s later, before
    /// the notifier had looked at that turn, so it was never held back from
    /// its parent. Its turn on the helper's notice said nothing; its parent
    /// heard it "finished its turn without a reply", the interim line
    /// dropped, and the lead was never woken. That timing is no matter: the
    /// lead is woken, and all it wrote reaches its parent, with the turn the
    /// wake brings or, the wake stopped, with the notice of its end. On
    /// either CLI's transcript.
    #[test]
    fn a_lead_whose_helper_answers_before_its_interim_turn_is_looked_at_is_woken_and_its_words_reach_its_parent() {
        use crate::agents::owed::{owe_asked, Asked};
        for provider in [Provider::Codex, Provider::Claude] {
            let line = |role: &str, text: &str| match provider {
                Provider::Claude if role == "user" => prompt_line(text),
                Provider::Claude => said_line(text),
                Provider::Codex => {
                    let kind = if role == "user" { "input_text" } else { "output_text" };
                    let v = serde_json::json!({ "type": "response_item", "payload": { "type": "message", "role": role, "content": [{ "type": kind, "text": text }] } });
                    format!("{v}\n")
                }
            };
            let tag = provider.as_str();
            let (top, id, low) = (format!("wg-{tag}-top"), format!("wg-{tag}-lead"), format!("wg-{tag}-low"));
            let (top, id, low) = (top.as_str(), id.as_str(), low.as_str());
            let spawned = "Spawn the helper, then end this turn on one line.";
            let interim = [line("user", spawned), line("assistant", "Helper is running, I will report when it is done.")].concat();
            let (known, path) = lead_on(provider, top, id, 1, false, &interim);
            register(low, Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some(id), false, serde_json::json!({ "background": true }));
            owe_asked(top, id, 0, 0, &Asked { text: spawned, key: None, reply: true });
            owe_asked(id, low, 0, 0, &Asked { text: "Reply HELPER-OK", key: None, reply: true });
            let wakes = || reg().lock().unwrap().news.iter().filter(|(c, l)| c == id && is_wake(l)).count();
            // The helper answers before the notifier looks at the lead for its
            // parent; the lead's turn on that notice says nothing.
            let helper_key = (id.to_string(), low.to_string());
            let owed_low = reg().lock().unwrap().owed[&helper_key];
            crate::agents::notifier::settle_told(&mut reg().lock().unwrap(), helper_key, Settled::Answered, &owed_low);
            notice_turn(&mut reg().lock().unwrap(), id, 2, &[low.to_string()], false);
            crate::background::idle_for_test(id, provider, 2, false);
            let notice = format!("{NOTICE_TAG} Your helper \"{low}\" finished its turn with 1 message since you asked, #1: \"HELPER-OK\"");
            let silent = format!("{interim}{}", line("user", &notice));
            std::fs::write(&path, &silent).unwrap();
            assert!(!reg().lock().unwrap().owed[&(top.to_string(), id.to_string())].told_pending, "{tag}: never held back");
            assert!(matches!(look(top, id, &known), Some((None, Settled::Hold(2)))), "{tag}: the grace for its last turn's reply");
            assert!(look(top, id, &known).is_none(), "{tag}: its parent hears nothing yet");
            assert_eq!(wakes(), 1, "{tag}: the lead is woken");
            assert!(look(top, id, &known).is_none());
            assert_eq!(wakes(), 1, "{tag}: once");
            // Its turn on the wake: its parent hears of it, the interim line included.
            reg().lock().unwrap().news.retain(|(c, _)| c != id);
            notice_turn(&mut reg().lock().unwrap(), id, 3, &[], true);
            crate::background::idle_for_test(id, provider, 3, false);
            let woken = [line("user", &wake_line(&[])), line("assistant", "FINAL: the helper answered HELPER-OK")].concat();
            std::fs::write(&path, format!("{silent}{woken}")).unwrap();
            let Some((Some(done), Settled::Answered)) = look(top, id, &known) else { panic!("{tag}: its turn on the wake") };
            assert!(
                done.contains("with 2 messages since you asked, #1: \"Helper is running") && done.contains("FINAL: the helper answered"),
                "{tag}: {done}"
            );
            // The breaker stopping the wake, its parent hears of its end with the interim line, never "without a reply".
            reg().lock().unwrap().woken.remove(id);
            reg().lock().unwrap().stopped.insert((id.into(), id.into()), now_ms());
            std::fs::write(&path, &silent).unwrap();
            crate::background::idle_for_test(id, provider, 2, false);
            let mut r = reg().lock().unwrap();
            let key = (top.into(), id.into());
            let upto = r.owed.get(&key).map_or(1, |o| o.upto);
            r.owed.insert(key, Owed { upto, ..Owed::from(0, 0) });
            drop(r);
            assert!(matches!(look(top, id, &known), Some((None, Settled::Hold(2)))));
            let Some((Some(plain), Settled::Answered)) = look(top, id, &known) else { panic!("{tag}: its parent hears") };
            assert!(plain.contains("finished its turn with 1 message since you asked, #1: \"Helper is running"), "{tag}: {plain}");
            assert!(!plain.contains("without a reply"), "{tag}: {plain}");
            assert_eq!(wakes(), 0, "{tag}");
            reg().lock().unwrap().stopped.remove(&(id.to_string(), id.to_string()));
            reg().lock().unwrap().notice_turns.remove(id);
            for s in [low, id, top] {
                crate::background::gone_for_test(s);
                forget(s);
            }
            let _ = std::fs::remove_file(&path);
        }
    }

    /// "Understood", "ok": a send that asked for no reply is answered in the
    /// report, and wakes its caller only for a status word or a report.
    #[test]
    fn an_answer_to_a_send_that_asked_for_no_reply_wakes_only_for_news() {
        use crate::agents::owed::{owe_asked, Asked};
        let (caller, id) = ("nr-c", "nr-s");
        register(caller, Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, serde_json::json!({}));
        register(id, Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some(caller), false, serde_json::json!({}));
        crate::background::idle_for_test(id, Provider::Claude, 1, false);
        let path = std::env::temp_dir().join(format!("luna-{id}-{}.jsonl", std::process::id()));
        transcripts().lock().unwrap().insert(id.into(), path.clone());
        let known = reg().lock().unwrap().known.get(id).cloned().unwrap();
        let pty = crate::pty::manager();
        let quiet = Asked { text: "FYI: main moved", key: None, reply: false };
        owe_asked(caller, id, 0, 0, &quiet);
        let o = |r: &str| {
            std::fs::write(&path, format!("{}{}{}", prompt_line("FYI: main moved"), prompt_line("and run the tests"), said_line(r)))
                .unwrap();
            let o = reg().lock().unwrap().owed.get(&(caller.to_string(), id.to_string())).copied().unwrap();
            notice_line(pty, caller, "Your helper", id, &known, held(o, 1, REPLY_GRACE_MS))
        };
        assert!(matches!(o("Understood."), Some((None, Settled::Silent(..)))), "no notice");
        assert!(matches!(o("DONE: rebased"), Some((Some(_), Settled::Answered))), "a status word");
        assert!(matches!(o(&"A long report. ".repeat(40)), Some((None, Settled::Silent(..)))), "length alone is not news");
        // Asked for a reply too, the debt wants one.
        owe_asked(caller, id, 0, 0, &Asked { text: "and run the tests", key: None, reply: true });
        assert!(matches!(o("Understood."), Some((Some(_), Settled::Answered))));
        crate::background::gone_for_test(id);
        for s in [id, caller] {
            forget(s);
        }
        let _ = std::fs::remove_file(&path);
    }

    /// 13 November: a notice opened on "VERDICT: GO", the answer to the second
    /// request; the first's "NO-GO" was on line 43 of the same file.
    #[test]
    fn an_answer_to_two_requests_says_which_is_whose() {
        use crate::agents::owed::{owe_asked, Asked};
        let (caller, id) = ("rq-c", "rq-s");
        register(caller, Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, serde_json::json!({}));
        register(id, Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some(caller), false, serde_json::json!({}));
        crate::background::idle_for_test(id, Provider::Claude, 2, false);
        owe_asked(caller, id, 0, 0, &Asked { text: "Review PR 7719 and give a verdict", key: None, reply: true });
        owe_asked(caller, id, 1, 0, &Asked { text: "Now review PR 7772", key: Some("pr-7772"), reply: true });
        let no_go = format!("VERDICT: NO-GO for 7719. {}", "Reason. ".repeat(60));
        let go = format!("VERDICT: GO for 7772. {}", "Reason. ".repeat(60));
        let transcript =
            [prompt_line("Review PR 7719 and give a verdict"), said_line(&no_go), prompt_line("Now review PR 7772"), said_line(&go)]
                .concat();
        let path = std::env::temp_dir().join(format!("luna-{id}-{}.jsonl", std::process::id()));
        std::fs::write(&path, transcript).unwrap();
        transcripts().lock().unwrap().insert(id.into(), path.clone());
        let (Some(text), Settled::Answered) = notice_for(caller, id, 2) else { panic!("answered") };
        assert!(text.contains("2 requests, latest #2 (pr-7772)"), "{text}");
        assert!(text.contains(" \"VERDICT: GO"), "{text}");
        let file = text.split("Report ").nth(1).and_then(|t| t.split(":1–").next()).unwrap();
        let whole = std::fs::read_to_string(file).unwrap();
        assert!(whole.starts_with("Re request 2 (requestKey \"pr-7772\"):\n\nVERDICT: GO"), "{whole}");
        assert!(whole.contains("===\n\nRe request 1:\n\nVERDICT: NO-GO"), "{whole}");
        let _ = std::fs::remove_file(file);
        crate::background::gone_for_test(id);
        for s in [id, caller] {
            forget(s);
        }
        let _ = std::fs::remove_file(&path);
    }

    /// Issue #77: a lead ended its turn on "Gate is running; waiting." with no
    /// gate started, and its parent read "finished its turn" as work going on.
    /// Its parent's notice says when nothing of its own runs, on either CLI's
    /// transcript, and not to a caller not its parent, nor where a line is
    /// coming that wakes it, nor where its CLI cut its background work off
    /// (`runs_nothing_is_false_for_whatever_would_start_the_session` has the
    /// rest of what stops it).
    #[test]
    fn a_turn_ended_with_nothing_running_says_so_to_its_parent() {
        for provider in [Provider::Claude, Provider::Codex] {
            let said = match provider {
                Provider::Claude => said_line("Gate is running for the final fixes; waiting."),
                Provider::Codex => format!(
                    "{}\n",
                    serde_json::json!({ "type": "response_item", "payload": { "type": "message", "role": "assistant",
                        "content": [{ "type": "output_text", "text": "Gate is running for the final fixes; waiting." }] } })
                ),
            };
            let tag = provider.as_str();
            let [top, id, other] = ["top", "lead", "other"].map(|s| format!("nx-{tag}-{s}"));
            let (top, id, other) = (top.as_str(), id.as_str(), other.as_str());
            let (known, path) = lead_on(provider, top, id, 1, false, &said);
            let pty = crate::pty::manager();
            let o = held(Owed::from(0, 0), 1, REPLY_GRACE_MS);
            let says = |caller: &str| match notice_line(pty, caller, "Your helper", id, &known, o) {
                Some((Some(line), _)) => Some(line),
                _ => None,
            };
            let plain = says(top).unwrap_or_else(|| panic!("{tag}: its turn is over"));
            assert!(plain.contains("finished its turn with 1 message since you asked: \"Gate is running"), "{tag}: {plain}");
            assert!(plain.contains(NOTHING_RUNNING), "{tag}: {plain}");
            // Only its parent is told to send to it.
            let to_other = says(other).unwrap_or_else(|| panic!("{tag}: a caller not its parent still hears"));
            assert!(!to_other.contains(NOTHING_RUNNING), "{tag}: {to_other}");
            // A delegate account's high usage only wakes the top chat.
            let (sub, account) = (format!("nx-{tag}-sub"), format!("nx{tag}"));
            register(&sub, Provider::Claude, "C:\\p", &format!("C:/acc/anthropic/{account}"), Some(id), false, serde_json::json!({}));
            crate::background::idle_for_test(&sub, Provider::Claude, 1, false);
            let usage = serde_json::json!({ "limits": [{ "label": "5h", "usedPct": 95.0 }] });
            reg().lock().unwrap().usage.insert(format!("claude/{account}"), usage);
            let line = says(top).unwrap_or_default();
            assert!(line.contains("finished its turn") && line.contains(NOTHING_RUNNING), "{tag}: {line}");
            reg().lock().unwrap().usage.remove(&format!("claude/{account}"));
            crate::background::gone_for_test(&sub);
            forget(&sub);
            // Its CLI cut its background work off: that line says how it goes on.
            crate::background::background_stopped_for_test(id, &[top]);
            let cut = says(top).unwrap_or_default();
            assert!(!cut.contains(NOTHING_RUNNING) && cut.contains(BACKGROUND_STOPPED), "{tag}: {cut}");
            crate::background::gone_for_test(id);
            for s in [id, top, other] {
                forget(s);
            }
            let _ = std::fs::remove_file(&path);
        }
    }

    /// `runs_nothing` itself, one branch at a time: each would keep the
    /// session going or start its next turn, and with it no sentence that
    /// nothing will. The notice's own verdict stops some of these earlier, so
    /// they are put to the check directly.
    #[test]
    fn idle_wait_cycles_resolve_when_their_sessions_are_idle() {
        use crate::activity::Turn;
        for size in [2, 3] {
            let ids: Vec<_> = (0..size).map(|i| format!("cycle-{size}-{i}")).collect();
            for (i, id) in ids.iter().enumerate() {
                crate::background::idle_for_test(id, Provider::Claude, 1, false);
                owe_from(id, &ids[(i + 1) % size], 0, 0);
            }
            let idle = |id| runs_nothing(id, crate::activity::turn_state(id));
            assert!(ids.iter().all(|id| idle(id)), "every idle member can make progress");
            crate::background::set_running_for_test(&ids[1]);
            assert_eq!(crate::activity::turn_state(&ids[1]).unwrap().turn, Turn::Busy);
            assert!(!idle(&ids[0]), "a turn in the cycle can still answer");
            crate::background::idle_for_test(&ids[1], Provider::Claude, 1, false);
            let delivery = super::super::TypingInto::claim(&ids[1]).unwrap();
            assert!(idle(&ids[0]), "delivery to a peer cannot suppress this stall");
            drop(delivery);
            for id in &ids {
                forget(id);
                crate::background::gone_for_test(id);
            }
        }
    }

    #[test]
    fn a_due_reply_from_a_peer_holds_an_interim_stall() {
        let (a, b, c) = ("peer-chain-a", "peer-chain-b", "peer-chain-c");
        for id in [a, b, c] {
            register(id, Provider::Claude, "C:/p", "C:/acc/one", None, true, serde_json::json!({}));
            crate::background::idle_for_test(id, Provider::Claude, 1, false);
        }
        owe_from(a, b, 0, 0);
        owe_from(b, c, 0, 0);
        owe_from(c, b, 1, 0);
        let path = std::env::temp_dir().join(format!("luna-{b}-{}.jsonl", std::process::id()));
        std::fs::write(&path, said_line("Waiting on C.")).unwrap();
        transcripts().lock().unwrap().insert(b.into(), path.clone());
        let (debt, known) = {
            let r = reg().lock().unwrap();
            (r.owed[&(a.into(), b.into())], r.known[b].clone())
        };
        let line = || notice_line(crate::pty::manager(), a, "Session B", b, &known, debt);
        assert!(!waits_on_helpers(b, a), "C is a peer chat");
        assert!(!runs_nothing(b, crate::activity::turn_state(b)), "C ended the turn it owes to B");
        assert!(matches!(line(), Some((None, Settled::Skip(1, ..)))), "A stays asleep while C's reply is due");
        let c_path = path.with_file_name(format!("luna-{c}-{}.jsonl", std::process::id()));
        std::fs::write(&c_path, said_line("C has the result.")).unwrap();
        transcripts().lock().unwrap().insert(c.into(), c_path.clone());
        owe_from(c, b, 0, 0);
        assert!(matches!(line(), Some((None, Settled::Skip(1, ..)))), "A is outside the due cycle");
        let c_debt = reg().lock().unwrap().owed[&(c.into(), b.into())];
        assert!(
            matches!(notice_line(crate::pty::manager(), c, "Session B", b, &known, c_debt), Some((Some(_), Settled::Answered))),
            "C can get B's plain reply within the cycle"
        );
        let pty = crate::pty::manager();
        let idle_for_c = |work: &mut WorkLook| work.runs_nothing(pty, c, b, crate::activity::turn_state(b));
        let mut work = WorkLook::default();
        assert!(idle_for_c(&mut work));
        // An explicit answer is also independent of B's reply if both are due.
        std::fs::write(&c_path, said_line("DONE: C has the result.")).unwrap();
        assert!(idle_for_c(&mut work), "the same tick reuses its peer's transcript check");
        assert!(!idle_for_c(&mut WorkLook::default()), "the next tick sees C's status");
        assert!(matches!(line(), Some((None, Settled::Skip(1, ..)))));
        reg().lock().unwrap().owed.remove(&(b.into(), c.into()));
        assert!(runs_nothing(b, crate::activity::turn_state(b)), "C's answer has been delivered");
        crate::background::idle_for_test(b, Provider::Claude, 2, false);
        std::fs::write(&path, said_line("Finished after hearing from C.")).unwrap();
        assert!(matches!(line(), Some((Some(line), Settled::Answered)) if line.contains("Finished after hearing from C")));
        for id in [a, b, c] {
            crate::background::gone_for_test(id);
            forget(id);
        }
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(c_path).unwrap();
    }

    #[test]
    fn an_inactive_debt_only_hears_failure_of_a_turn_it_still_covers() {
        use crate::agents::owed::{owe_asked, Asked};

        for cut in [false, true] {
            let caller = "covered-failure-caller";
            let id = if cut { "covered-cut-helper" } else { "covered-failure-helper" };
            register(id, Provider::Claude, "C:/p", "C:/acc/one", None, false, serde_json::json!({}));
            crate::background::idle_for_test(id, Provider::Claude, 1, false);
            let path = std::env::temp_dir().join(format!("luna-{id}-{}.jsonl", std::process::id()));
            std::fs::write(&path, prompt_line("Earlier request")).unwrap();
            transcripts().lock().unwrap().insert(id.into(), path.clone());
            owe_asked(caller, id, 0, 0, &Asked { text: "First caller's request", key: None, reply: false });
            let key = (caller.into(), id.into());
            {
                let mut r = reg().lock().unwrap();
                let o = r.owed.get_mut(&key).unwrap();
                o.wake.silent = Some(1);
                o.held = Some((2, 0));
            }
            if cut {
                crate::background::idle_for_test(id, Provider::Claude, 2, false);
                crate::background::cut_for_test(id);
            } else {
                crate::background::set_running_for_test(id);
                crate::background::exited_for_test(id, Some(1));
            }
            let known = reg().lock().unwrap().known[id].clone();
            let render = || {
                let o = reg().lock().unwrap().owed[&key];
                notice_line(crate::pty::manager(), caller, "Your helper", id, &known, o)
            };
            let turn = |prompt| {
                std::fs::write(&path, format!("{}{}{}", prompt_line("Earlier request"), said_line("Acknowledged."), prompt_line(prompt)))
                    .unwrap();
            };
            turn("A different caller's request");
            assert!(render().is_none(), "an unrelated failed or cut turn owes nothing");
            let expected = if cut { "did NOT finish" } else { "stopped on an error" };
            let text = || render().and_then(|(line, _)| line).unwrap_or_default();
            turn("First caller's request");
            let own = text();
            assert!(own.contains(expected), "cut {cut}: {own}, {:?}", crate::activity::turn_state(id));
            turn("Continue the existing plan");
            reg().lock().unwrap().owed.get_mut(&key).unwrap().wake.plan_through = Some(1);
            let plan = text();
            assert!(plan.contains(expected), "{plan}");
            crate::background::gone_for_test(id);
            forget(id);
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn runs_nothing_is_false_for_whatever_would_start_the_session() {
        use crate::activity::{Turn, TurnState};
        use crate::agents::held::Held;
        let (top, id, other) = ("rn-top", "rn-lead", "rn-other");
        let state = |turn, children_busy| Some(TurnState { turn, busy: turn == Turn::Busy, turns_ended: 1, children_busy });
        let idle = state(Turn::Idle, false);
        let account = "C:\\acc\\anthropic\\one";
        for s in [top, other] {
            register(s, Provider::Claude, "C:\\p", account, None, false, serde_json::json!({}));
        }
        register(id, Provider::Claude, "C:\\p", account, Some(top), true, serde_json::json!({ "background": true }));
        let fresh = || crate::background::idle_for_test(id, Provider::Claude, 1, false);
        let key = |c: &str| (id.to_string(), c.to_string());
        let stops = |what: &str| assert!(!runs_nothing(id, idle), "{what}");
        fresh();
        assert!(runs_nothing(id, idle), "nothing at all");
        assert!(!runs_nothing(id, None), "no state: not known to be idle");
        assert!(!runs_nothing(id, state(Turn::Busy, false)), "a turn runs");
        assert!(!runs_nothing(id, state(Turn::Idle, true)), "a job or subagent its turns left runs");
        // What its record in the background holds for it.
        crate::background::set_running_for_test(id);
        stops("a process of its own runs");
        fresh();
        crate::background::queue_for_test(id, "queued");
        stops("text is queued for its next turn");
        crate::background::start_queued_for_test(id);
        stops("text is in flight");
        fresh();
        crate::background::hold_for_test(id);
        stops("it holds on a permission");
        fresh();
        assert!(runs_nothing(id, idle));
        // What the registry holds for it.
        reg().lock().unwrap().held.push(Held {
            caller: other.into(),
            id: id.into(),
            text: "x".into(),
            since: 0,
            key: None,
            reply: true,
            going_in: None,
        });
        stops("text held back for a draft");
        reg().lock().unwrap().held.retain(|h| h.id != id);
        reg().lock().unwrap().waits.insert(key(other), 0);
        stops("a wait of its is out");
        reg().lock().unwrap().waits.remove(&key(other));
        reg().lock().unwrap().reserved.insert(key(other), 0);
        stops("a wait_many reserves a session's notice");
        reg().lock().unwrap().reserved.remove(&key(other));
        reg().lock().unwrap().news.push((id.into(), "a line for it".into()));
        stops("a line queued to go into it");
        reg().lock().unwrap().news.retain(|(c, _)| c != id);
        // Quota on a lead's line does not start the lead's next turn.
        let sub = "rn-sub";
        register(sub, Provider::Claude, "C:\\p", "C:/acc/anthropic/rn-acc", Some(id), false, serde_json::json!({}));
        crate::background::idle_for_test(sub, Provider::Claude, 1, false);
        assert!(runs_nothing(id, idle), "an idle helper of its own, nothing to say of it");
        owe_from(id, sub, 1, 0);
        assert!(waits_on_helpers(id, top));
        assert!(runs_nothing(id, idle), "its unanswered helper is idle too");
        let debt = reg().lock().unwrap().owed[&key(sub)];
        super::super::settle_told(&mut reg().lock().unwrap(), key(sub), Settled::Silent(1, true), &debt);
        assert!(!waits_on_helpers(id, top));
        assert!(runs_nothing(id, idle), "an unread silent answer holds no lead");
        let window = |pct: f64| serde_json::json!({ "limits": [{ "label": "5h", "usedPct": pct }] });
        reg().lock().unwrap().usage.insert("claude/rn-acc".into(), window(95.0));
        assert!(runs_nothing(id, idle), "only the top chat hears quota");
        reg().lock().unwrap().usage.insert("claude/rn-acc".into(), window(40.0));
        assert!(runs_nothing(id, idle), "a window below every level");
        reg().lock().unwrap().usage.remove("claude/rn-acc");
        crate::background::gone_for_test(sub);
        forget(sub);
        let mut cut = crate::agents::test_support::spawn_taken(9_177_001, id, None, false);
        cut.news = Some("your spawn went with a core".into());
        reg().lock().unwrap().spawns.insert(cut.request, cut);
        stops("a spawn a core's going cut off, to be told");
        reg().lock().unwrap().spawns.remove(&9_177_001);
        assert!(runs_nothing(id, idle));
        owe_from(id, other, 0, 0);
        assert!(runs_nothing(id, idle), "an idle peer owes an answer");
        reg().lock().unwrap().owed.remove(&key(other));
        owe_from(id, top, 0, 0);
        assert!(runs_nothing(id, idle), "its idle caller owes an answer");
        reg().lock().unwrap().owed.remove(&key(top));
        assert!(runs_nothing(id, idle));
        crate::background::gone_for_test(id);
        for s in [id, top, other] {
            forget(s);
        }
    }
}
