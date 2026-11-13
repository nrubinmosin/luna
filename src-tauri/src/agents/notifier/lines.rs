//! The notice lines: what a notice says of a session a caller is owed by,
//! settled or standing, and the turns notices start.

use crate::agents::ask::{ask_of, live};
use crate::agents::lost::{never_started_what, sentence, undelivered_for, undelivered_line, undelivered_text};
use crate::agents::notifier::{left_quiet, Settled, IDLE_JOB_MS};
use crate::agents::owed::requests_of;
use crate::agents::owed::{Owed, QUEUED_GRACE_MS};
use crate::agents::persist::{core_up_ms, returning};
use crate::agents::read::{failure_of, transcript_end, transcript_found, transcript_path};
use crate::agents::registry::{is_descendant, label, reg, waits_on_helpers, Known, Registry};
use crate::agents::said::{
    answer_file, answer_text, answer_where, begins, inline_answer, note_file, replies_since, report_re, report_span, said_since,
    says_status, Said, SHORT_MESSAGE,
};
use crate::agents::wait::REPLY_GRACE_MS;
use crate::provider::Provider;
use crate::throttle::now_ms;
use std::collections::HashMap;
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
/// delegates work its turns wait for news (`worth_waking`), and once they are
/// done Luna wakes it rather than leave it waiting (`wake_lead`).
pub(in crate::agents) fn is_lead(k: &Known) -> bool {
    k.tools && k.parent.is_some()
}

/// Whether what a turn wrote (`said`) is worth a notice that would otherwise
/// wait: of a lead whose delegates still work, a status word; in answer to
/// sends that asked for no reply (`no_reply`), a status word or a message of
/// SHORT_MESSAGE or more. Anything else is worth one.
pub(in crate::agents) fn worth_waking(said: &Said, lead: bool, no_reply: bool) -> bool {
    if !lead && !no_reply {
        return true;
    }
    said.iter().any(|m| says_status(m)) || (!lead && said.iter().any(|m| m.chars().count() >= SHORT_MESSAGE))
}

/// A lead Luna has told its delegates finished: at the `turnsEnded` their end
/// was seen at, where its transcript ended then, and when. Once per such end;
/// the turn it starts reaches the lead's callers (`NoticeTurn::wake`), and if
/// that turn ends without a word they hear the lead seems stuck.
#[derive(Clone, Copy)]
pub(in crate::agents) struct Woken {
    ended: u32,
    cursor: u64,
    at: u64,
}

/// How long a lead woken by Luna has to take that up before its callers hear
/// of its delegates' end as they would have: the wake waits, as any notice
/// does, for the lead to be between turns, and one that never goes in must
/// not keep them from hearing.
const WAKE_WAIT_MS: u64 = 10 * 60 * 1000;

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
        "{WAKE_HEAD}{named}. Their answers are in your conversation (your CLI's task notifications) or where they wrote them, and a helper's in luna_report_get. Nothing else wakes you for them: take them up and go on with what you were asked, or report where you stand."
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
    // Mid-switch to a fresh conversation of its own (handoff/lead.rs) it is at
    // work, and the fresh one hears where its delegates stand as it opens:
    // its callers wait for what that one says.
    if crate::background::switch_of(id).is_some() {
        return Wake::Waits;
    }
    let now = now_ms();
    let cursor = transcript_end(id);
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(w) = r.woken.get(id).filter(|w| w.ended == ended) {
        return if now.saturating_sub(w.at) < WAKE_WAIT_MS { Wake::Waits } else { Wake::Missed };
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

/// Lead `id` starts a fresh conversation of its own (handoff/lead.rs
/// `start`): a wake still queued for the one it leaves, or one given there, is
/// behind it. The fresh one's opening says where its delegates stand, and a
/// wake's cursor is into the transcript it leaves, so "seems stuck" would read
/// the fresh one's words from the wrong place. An end of its delegates still
/// unheard of wakes it again, in the fresh one. So is the share of its context
/// its notices ride with (usage.rs `context_rider`): the fresh one's comes
/// with the next reading once it is through.
pub(in crate::agents) fn lead_goes_fresh(id: &str) {
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let queued = r.news.len();
    r.news.retain(|(c, line)| !(c == id && is_wake(line)));
    let dropped = r.news.len() != queued;
    r.woken.remove(id);
    r.context_seen.remove(id);
    drop(r);
    if dropped {
        crate::agents::save_chats();
    }
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
            .keys()
            .filter(|(c, h)| c == id && is_descendant(&r, id, h))
            .map(|(_, h)| match r.names.get(h).map(|n| n.trim()).filter(|n| !n.is_empty()) {
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
    let since = now_ms().saturating_sub(at);
    let typed = |s: &str| pty.typed_ago_ms(s).is_some_and(|ago| ago < since);
    if !typed(caller) && !typed(id) {
        return true;
    }
    reg().lock().unwrap_or_else(|e| e.into_inner()).stopped.remove(&key);
    crate::log::info("agents", &format!("notices to {} about {} go on: a person has typed in", label(caller), label(id)));
    false
}

/// The notice for one session, or None while nothing has happened to it.
pub(in crate::agents) fn notice_line(
    pty: &crate::pty::PtyManager,
    caller: &str,
    who: &str,
    id: &str,
    k: &Known,
    o: Owed,
) -> Option<(Option<String>, Settled)> {
    // A move to another transcript since the debt was looked at, seen to by this
    // look or another's (`moved_to`), has it read from where it reads now.
    let mut o = o;
    transcript_path(pty, id);
    match reg().lock().unwrap_or_else(|e| e.into_inner()).owed.get(&(caller.to_string(), id.to_string())) {
        // Another debt by now (`debts_moved`): the next look goes by that.
        Some(now) if now.rev != o.rev => return None,
        Some(now) => o.cursor = now.cursor,
        None => {}
    }
    let ts = crate::activity::turn_state(id);
    let ended = ts.map(|t| t.turns_ended).unwrap_or(0);
    let turn = ts.map(|t| t.turn);
    let alive = live(pty, id);
    // A Claude Code chat ends a turn on a job it put in the background and takes
    // it up again in a turn of its own when the job is done (`claude -p` stays
    // in its turn until then, up to 3 hours for a helper's: background/
    // command.rs); for anything else that job's end is all there
    // will be to hear of.
    let left = if ts.is_some_and(|t| t.children_busy) && alive { left_quiet(id) } else { None };
    let background = ts.is_some_and(|t| t.children_busy) && alive;
    let waits = waits_on_helpers(id, caller);
    let between_turns = turn != Some(crate::activity::Turn::Busy);
    let real_turn = o.told_pending
        && real_turn_past(id, caller, o.baseline, ended, waits, || between_turns && news_for_parent(pty, caller, id, k, o.cursor));
    let now = verdict(&o, ended, turn, background || waits, real_turn);
    if now == Verdict::OnlyNotices {
        return Some((None, Settled::Skip(ended)));
    }
    if now == Verdict::AtWork && o.held.is_some() {
        return Some((None, Settled::Unhold));
    }
    // Long idle is no proof of an end — a client waiting on work done
    // elsewhere, a slow download — so the caller hears it once, and the debt
    // stays open for that end.
    if now == Verdict::AtWork && !o.told_idle {
        if let Some((quiet, names)) = left.as_ref().filter(|(quiet, _)| *quiet >= IDLE_JOB_MS) {
            return Some((
                Some(idle_line(who, *quiet, names, &background_calls(pty, id, k.provider))),
                Settled::Idle(now_ms().saturating_sub(quiet - IDLE_JOB_MS)),
            ));
        }
    }
    if now == Verdict::JobOver && alive {
        // Its processes come and go between samples, and a job's end may be
        // followed by the turn the CLI takes it up in.
        return match o.held {
            Some((n, at)) if n == ended && now_ms().saturating_sub(at) >= BACKGROUND_GRACE_MS => {
                // Its turns on notices may have said something since.
                let said = said_since(pty, id, k.provider, o.cursor);
                let failed = failure_of(pty, id, k.provider);
                let stopped = background_stopped(id, caller);
                // A core's going took the job, or its CLI cut it off: news, whatever was said.
                let plain = failed.is_none() && !o.job_went && stopped.is_empty();
                let how = if said.is_empty() && plain { Settled::NoNews(at) } else { Settled::Answered };
                // A lead with all its delegates done and not a word of it: it is
                // woken to go on, and its callers hear of the turn that brings.
                if matches!(how, Settled::NoNews(_)) && is_lead(k) && wake_lead(pty, id, ended) == Wake::Waits {
                    return None;
                }
                if o.no_reply && plain && !worth_waking(&said, false, true) {
                    return Some((None, Settled::Unasked));
                }
                let said = said.asked(requests_of(caller, id));
                let quoted = if failed.is_some() { String::new() } else { begins_re(&said) };
                let n = said.len();
                let messages = if n == 1 { "1 message".to_string() } else { format!("{n} messages") };
                // Short enough, what it said is the notice's, and no file.
                let said = match (n, inline_answer(&said)) {
                    (0, _) if !stopped.is_empty() => "it has said nothing since your last notice".to_string(),
                    (0, _) => "it has said nothing since your last notice, and takes no turn on that by itself".to_string(),
                    (_, Some(line)) if said.requests().is_empty() => format!("it wrote {messages} since your last notice: \"{line}\""),
                    (_, _) => format!("it wrote {messages} since your last notice{}; {}{quoted}", answering(&said), answer_file(id, &said)),
                };
                let failed = failed.map_or(String::new(), |why| format!(" Its last turn stopped on an error: {}", sentence(&why)));
                let since = if o.job_went { " that Luna's core has restarted" } else { "" };
                Some((Some(format!("{who} has nothing left running now{since}, and {said}.{stopped}{failed}")), how))
            }
            Some((n, _)) if n == ended => None,
            _ => Some((None, Settled::Hold(ended))),
        };
    }
    if now == Verdict::TurnOver {
        let cut = crate::background::cut_short(id) || o.cut_short;
        let failed = if cut { None } else { failure_of(pty, id, k.provider) };
        let replies = replies_since(pty, id, k.provider, o.cursor, cut || failed.is_some()).asked(requests_of(caller, id));
        let settle_ms = if background {
            BACKGROUND_GRACE_MS
        } else if replies.is_empty() {
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
        let lost = undelivered_for(id, caller, parent);
        let text = match (replies.is_empty(), lost.is_empty()) {
            (true, true) => None,
            (true, false) => Some(undelivered_text(&lost)),
            (false, true) => Some(answer_text(&replies)),
            (false, false) => Some(format!("{}\n\n---\n\n{}", answer_text(&replies), undelivered_text(&lost))),
        };
        // All it said, short enough for the notice itself: no file to read.
        // Not answers to several requests, which the file sets apart.
        let inline =
            if lost.is_empty() && failed.is_none() && !cut && replies.requests().is_empty() { inline_answer(&replies) } else { None };
        let file = text.as_deref().filter(|_| inline.is_none()).and_then(|t| note_file(id, t));
        let span = text.as_deref().and_then(|t| report_span(&replies, t));
        let reply = match replies.len() {
            0 if !transcript_found(pty, id) => ", but Luna cannot find its transcript, so whether it replied is unknown".to_string(),
            0 => " without a reply".to_string(),
            n if inline.is_some() => {
                let messages = if n == 1 { "1 message".to_string() } else { format!("{n} messages") };
                format!(" with {messages} since you asked: \"{}\"", inline.as_deref().unwrap_or_default())
            }
            1 => format!(" with 1 message since you asked; {}", answer_where(file.as_deref(), span)),
            n => format!(" with {n} messages since you asked{}; {}", answering(&replies), answer_where(file.as_deref(), span)),
        };
        if parent && crate::background::never_started(id) {
            // Before it is told: what its next turn will not go on in is told with this.
            crate::background::settle_worktree(id);
            let what = never_started_what(&lost, file.as_deref());
            return Some((
                Some(format!("{who} never got started: Luna's core restarted before its first turn got anywhere, so its prompt was lost and there is nothing to carry on from; {what}. luna_session_send starts it afresh.")),
                Settled::Answered,
            ));
        }
        let lost = undelivered_line(&lost, !replies.is_empty(), file.as_deref());
        // In every line that settles the debt: the next does not say it again.
        let stopped = background_stopped(id, caller);
        let unsure = if o.unsure {
            " Luna cannot tell whether your last message reached it: the core went as it was going in, and its transcript cannot be read. luna_message_list shows what it has."
        } else {
            ""
        };
        if cut {
            return Some((
                Some(format!("{who} did NOT finish its turn: Luna's core restarted while it worked and stopped it. It ended{reply}.{stopped}{lost}{unsure} luna_session_send has it carry on.")),
                Settled::Answered,
            ));
        }
        // Whatever else is going on: this turn did not get to its end.
        if let Some(why) = failed {
            let said = if replies.is_empty() { String::new() } else { format!(" It ended{reply}.") };
            return Some((
                Some(format!("{who} stopped on an error: {}{said}{stopped}{lost} luna_session_send has it carry on.", sentence(&why))),
                Settled::Answered,
            ));
        }
        let reply = if inline.is_some() { reply } else { format!("{reply}{}", begins_re(&replies)) };
        let lead = is_lead(k);
        if background || waits {
            if lead {
                note_delegates(pty, id, k.provider, left.as_ref().filter(|_| background).map(|(_, names)| names.as_str()));
            }
            // A lead's delegates at work are its normal state, and what it
            // writes meanwhile waits in its report for the notice that comes
            // once they are done; so does an answer to sends that asked for
            // none. A status word gets either through now, and the first turn
            // that answers a send of its caller's is no interim report.
            let held_as_lead = lead && !(o.by_send && !o.told_pending);
            if !worth_waking(&replies, held_as_lead, o.no_reply) {
                return Some((None, Settled::Skip(ended)));
            }
            // Said first and plainly: a weak model read "ended a turn" as
            // done and deleted the helper mid-job. Once per debt: a job can
            // run for days, and each notice costs the caller a turn.
            let agents = crate::activity::subagents(id);
            let what = match &left {
                Some((_, names)) if background => format!("a job it started still runs in the background ({names})"),
                _ if background && agents == 1 => "a subagent its CLI started still works in the background".to_string(),
                _ if background && agents > 1 => format!("{agents} subagents its CLI started still work in the background"),
                _ if background => "a job it started still runs in the background".to_string(),
                _ => "helpers of its own have yet to answer it".to_string(),
            };
            let line = if o.told_pending {
                format!("{who} ended another turn{reply}, but {what}. The next notice comes when that is over, or with a turn of its own.")
            } else if lead {
                format!("{who} ended a turn{reply}, and {what}. The next notice comes when that is over, or with another status word.")
            } else {
                format!("{who} is NOT done yet: {what}. It ended a turn{reply}. The next notice comes when that is over, or with a turn of its own.")
            };
            return Some((Some(line), Settled::Pending(ended, transcript_end(id))));
        }
        // Woken to go on (`wake_lead`) and not a word since: it is stuck, or
        // takes no notice of Luna's; once, as this settles the debt.
        if let Some(w) = woken_before(id, ended).filter(|_| lead) {
            if said_since(pty, id, k.provider, w.cursor).is_empty() {
                let earlier = if replies.is_empty() { String::new() } else { format!(" Its turns since you asked ended{reply}.") };
                return Some((
                    Some(format!("{who} seems stuck: its delegates had finished and Luna told it so, but it ended its turn on that without a word.{earlier}{lost}")),
                    Settled::Answered,
                ));
            }
        }
        if o.no_reply && lost.is_empty() && stopped.is_empty() && !o.unsure && !o.resumed && !worth_waking(&replies, false, true) {
            return Some((None, Settled::Unasked));
        }
        // Too short a turn for a notice of its own in between.
        let resumed = if o.resumed { " Luna's core stopped it on the way and it resumed automatically." } else { "" };
        return Some((Some(format!("{who} finished its turn{reply}.{stopped}{lost}{resumed}{unsure}")), Settled::Answered));
    }
    // A turn gone quiet is told once, and once more if it then answers and
    // goes quiet again only waiting on its background work: the first notice
    // said nothing of that.
    let answered = crate::background::turn_answered(id);
    let untold = if answered { o.told_background_wait != Some(ended) } else { o.told_quiet != Some(ended) };
    if turn == Some(crate::activity::Turn::Busy) && alive && untold {
        let waiting = crate::background::waits_on_background(id).filter(|_| answered);
        // What its job does counts too: a subagent's long command writes
        // nothing to the stream until it ends. Only so long: a watcher that
        // polls, a growing `tail -f` or a chatty dev server keeps its job busy
        // for good, and is what this notice is for.
        let job_quiet = waiting.as_ref().and_then(|w| w.job_quiet_ms);
        let quiet = turn_quiet_ms(pty, id).map(|q| match job_quiet {
            Some(j) if q < JOB_HOLDS_MS => q.min(j),
            _ => q,
        });
        if let Some(quiet) = quiet.filter(|q| *q >= QUIET_TURN_MS) {
            // Looked for only once the turn is that quiet: a deadline holds the
            // notice back, never brings it sooner.
            let at = now_ms();
            let due = due_ms(pty, id, k.provider);
            if due.is_none_or(|due| at >= due.saturating_add(PAST_DUE_MS)) {
                return Some(match waiting {
                    Some(w) => (
                        Some(background_wait_line(who, quiet, &background_calls(pty, id, k.provider), &w.procs)),
                        Settled::WaitsOnBackground(ended),
                    ),
                    None => (Some(quiet_line(who, quiet, due.map(|due| at - due))), Settled::Quiet(ended)),
                });
            }
        }
    }
    standing_line(pty, who, id, k, o)
}

/// What a notice says of a helper whose CLI stopped what its last turn left in
/// the background, out of time to wait for it (background/run.rs
/// `stops_background`), to a caller it owed an answer then that has not had
/// it yet; nothing for any other. Without it, the end of those subagents reads
/// as a job that finished and said nothing.
fn background_stopped(id: &str, caller: &str) -> &'static str {
    background_stopped_line(id, caller).unwrap_or("")
}

/// See `background_stopped`: the line, by what stopped the work.
pub(in crate::agents) fn background_stopped_line(id: &str, caller: &str) -> Option<&'static str> {
    if !crate::background::background_stopped_for(id, caller) {
        return None;
    }
    Some(match crate::background::background_stop_cause(id) {
        crate::background::StopCause::Ceiling => BACKGROUND_STOPPED,
        crate::background::StopCause::TurnEnd => SUBAGENTS_INTERRUPTED,
    })
}

/// See `background_stopped`; a notice that went in with it acknowledges it for
/// its caller (notifier `ack_background_stop`), as does a reply handed over by
/// a wait, read or report (owed.rs `hand_over`, `background_stopped_note`).
pub(in crate::agents) const BACKGROUND_STOPPED: &str = " Its CLI stopped the subagents and jobs it had left in the background, out of time to wait for them, not because they were done: their work may be unfinished and uncommitted. luna_session_send has it check what they left and carry on.";

/// The same for a Codex helper, whose `codex exec` interrupts the subagents
/// its turn spawned as the turn ends (codex::session `subagents_cut_at_turn_end`).
pub(in crate::agents) const SUBAGENTS_INTERRUPTED: &str = " Its CLI interrupted the subagents it had spawned as its turn ended, before they were done: their work may be unfinished and uncommitted. luna_session_send has it check what they left and carry on, waiting for any subagent it spawns before it ends its turn.";

/// Each line a notice may say a helper's background work was stopped with.
pub(in crate::agents) const STOPPED_LINES: [&str; 2] = [BACKGROUND_STOPPED, SUBAGENTS_INTERRUPTED];

/// A turn that has said nothing for this long, its caller hears of once: a
/// command that hangs looks like this and like nothing else from outside, and
/// a caller that ended its turn on "a notice comes when it is done" would
/// otherwise hear nothing until a person asks. A long build looks the same,
/// hence once per turn and not its end.
pub(in crate::agents) const QUIET_TURN_MS: u64 = 30 * 60 * 1000;

/// How long a helper's job at work holds back the notice that its answered
/// turn waits on its background work, its stream quiet all the while: past
/// this the notice goes, whatever the job does.
const JOB_HOLDS_MS: u64 = 90 * 60 * 1000;

/// A command its own timeout has yet to stop is no sign of a hang: a quiet turn
/// that waits on one is told of only this long past that timeout, by when the
/// CLI has ended the call and written so, or it hangs. Test lanes of 45 min
/// under a timeout of their own each woke their caller for nothing.
const PAST_DUE_MS: u64 = 5 * 60 * 1000;

fn quiet_line(who: &str, quiet_ms: u64, past_due_ms: Option<u64>) -> String {
    let quiet = quiet_ms / 60_000;
    match past_due_ms {
        Some(past) => format!(
            "{who} is mid-turn and has written nothing for {quiet} min, and the command it waits on is {} min past its own timeout, so it may hang; luna_message_list with screen: true shows where it is. Said once per turn; the usual notice comes when it ends.",
            past / 60_000
        ),
        None => format!(
            "{who} is mid-turn and has written nothing for {quiet} min: a command it runs may hang, or simply take long; luna_message_list with screen: true shows where it is. Said once per turn; the usual notice comes when it ends."
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
        "A subagent among them may still be at work, thinking between its steps, which shows nowhere; luna_message_list shows the answer, and luna_session_stop ends it all if it should not be running."
    } else {
        "luna_session_stop ends it now, luna_message_list shows the answer."
    };
    format!(
        "{who} has written its answer, but its turn is not over: its CLI waits on what the turn left in the background, and nothing has come of it for {} min. It waits on: {what}. Messages to it wait until that ends, or until Claude Code stops it 3 h after the answer. {stop} Said once per turn; the usual notice comes when it ends.",
        quiet_ms / 60_000
    )
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

/// All a session's turns left has sat idle (IDLE_JOB_MS): the processes
/// (`names`), and what the transcript says they were started to do (`calls`,
/// `background_calls`), which is what a caller needs to tell a stuck wait from
/// a job; a list of `timeout.exe, grep.exe, tail.exe` alone left it guessing.
fn idle_line(who: &str, quiet_ms: u64, names: &str, calls: &[String]) -> String {
    let waits = if calls.is_empty() { String::new() } else { format!(" Its transcript has them started as: {}.", calls.join("; ")) };
    format!(
        "{who} has only idle processes left, which have done nothing for {} min: {names}.{waits} They may be stuck or waiting on something; luna_session_stop ends them. You will still hear when they end.",
        quiet_ms / 60_000
    )
}

/// What a notice adds where an answer covers several of the caller's
/// requests (said.rs `Said::asked`): which, and that the file sets them apart.
fn answering(replies: &Said) -> String {
    let requests = replies.requests();
    if requests.is_empty() {
        return String::new();
    }
    let names: Vec<String> = requests.iter().map(|q| q.name()).collect();
    format!(", answering your {} (each its own section in the file, the latest first)", names.join(", "))
}

/// `begins`, saying which request the report it quotes answers where the
/// answer covers several.
fn begins_re(replies: &Said) -> String {
    let quoted = begins(replies);
    match report_re(replies).filter(|_| !replies.requests().is_empty() && !quoted.is_empty()) {
        Some(q) => quoted.replacen(". It begins:", &format!(". Its report, re {}, begins:", q.name()), 1),
        None => quoted,
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
        if o.lost {
            return Some((
                Some(format!("{who} has exited before Luna could type your message in: the user's draft held it until then.")),
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
    if o.resumed {
        return Some((
            Some(format!("{who} was cut short when Luna's core restarted and has resumed automatically, with anything queued for it. It works on; another notice comes when its turn ends.")),
            Settled::Resumed,
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
    use crate::agents::owed::{owe_from, Owed};
    use crate::agents::persist::{tie_back, ties_of, Ties};
    use crate::agents::read::transcripts;
    use crate::agents::registry::{parent_of, reg, register, waits_on_helpers};
    use crate::agents::said::said_since;
    use crate::agents::teardown::forget;
    use crate::agents::test_support::{job_over_helper, notice_for};
    use crate::agents::wait::{reply_text, REPLY_GRACE_MS};

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
        assert!(!fine.contains("the answer is in") && !fine.contains("It begins"), "{fine}");
        let failed = line_for("vd-failed");
        assert!(failed.contains("stopped on an error: rate_limit"), "{failed}");
        assert!(failed.contains("It ended with 1 message"), "what it said before is still named: {failed}");
        assert!(!failed.contains("It begins") && !failed.contains("All done"), "its words are no verdict on it: {failed}");
        for id in ["vd-fine", "vd-failed"] {
            forget(id);
            crate::activity::forget(id);
        }
        let _ = std::fs::remove_file(&path);
    }

    /// The loop of that night: two chats that wrote to each other, a stray
    /// process under each. Every notice starts a turn in the chat it goes into,
    /// and that turn's end is what the other is owed. Driven by the notifier's
    /// own decisions, with a turn counted the moment its notice goes in.
    #[test]
    fn two_chats_with_a_job_each_do_not_wake_each_other_forever() {
        use crate::activity::Turn;
        let (a, b) = ("pp-a", "pp-b");
        let key = |caller: &str, id: &str| (caller.to_string(), id.to_string());
        let mut turns: HashMap<&str, u32> = HashMap::from([(a, 1), (b, 1)]);
        owe_from(a, b, 0, 0);
        owe_from(b, a, 0, 0);
        let mut notices = 0;
        // One pass over both debts, with or without the jobs; returns how many
        // notices went.
        let pass = |job: bool, turns: &mut HashMap<&str, u32>| {
            let mut sent = 0;
            for (caller, id) in [(a, b), (b, a)] {
                let Some(o) = reg().lock().unwrap().owed.get(&key(caller, id)).copied() else { continue };
                let ended = turns[id];
                let v = verdict(&o, ended, Some(Turn::Idle), job, real_turn_past(id, caller, o.baseline, ended, false, || true));
                let settled = match v {
                    Verdict::OnlyNotices => {
                        reg().lock().unwrap().owed.get_mut(&key(caller, id)).unwrap().baseline = ended;
                        continue;
                    }
                    Verdict::TurnOver if job => Settled::Pending(ended, 0),
                    Verdict::TurnOver | Verdict::JobOver => Settled::Answered,
                    Verdict::Nothing | Verdict::AtWork => continue,
                };
                sent += 1;
                let mut r = reg().lock().unwrap();
                notice_turn(&mut r, caller, turns[caller] + 1, &[id.to_string()], false);
                match settled {
                    Settled::Pending(n, c) => {
                        r.owed.insert(key(caller, id), Owed::pending(n, c));
                    }
                    _ => {
                        r.owed.remove(&key(caller, id));
                    }
                }
                drop(r);
                *turns.get_mut(caller).unwrap() += 1;
            }
            sent
        };
        for _ in 0..500 {
            notices += pass(true, &mut turns);
        }
        assert_eq!(notices, 2, "\"NOT done yet\" once per debt, whatever the turns on those notices");
        let at_rest = turns.clone();
        for _ in 0..500 {
            notices += pass(true, &mut turns);
        }
        assert_eq!(turns, at_rest, "no notice, no turn: the chats are left at rest");
        // The jobs end: each is told once, and that is the end of it.
        for _ in 0..500 {
            notices += pass(false, &mut turns);
        }
        assert_eq!(notices, 4, "the job's end is news, once per debt");
        assert!(!reg().lock().unwrap().owed.contains_key(&key(a, b)));
        assert!(!reg().lock().unwrap().owed.contains_key(&key(b, a)));
        forget(a);
        forget(b);
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

        let Some((Some(text), Settled::Pending(2, cursor))) = line("ir-top", &format!("{notice}{}", said(&report))) else {
            panic!("its parent hears")
        };
        assert!(text.contains("ended another turn") && text.contains("helpers of its own have yet to answer it"), "{text}");
        assert!(text.contains("It begins: \"BLOCKED: Phase 1 is in"), "{text}");
        assert_eq!(cursor, std::fs::metadata(&path).unwrap().len(), "and what follows is what it hears next");
        let file = text.split("the answer is in ").nth(1).and_then(|t| t.split(". It begins").next()).unwrap();
        assert!(std::fs::read_to_string(file).unwrap().starts_with("BLOCKED: Phase 1 is in"));
        let _ = std::fs::remove_file(file);
        // A line or two is no news worth a turn: it comes with the answer.
        assert!(matches!(line("ir-top", &format!("{notice}{}", said("Noted; waiting on ir-low2."))), Some((None, Settled::Skip(2)))));
        // Nor is a long interim report with no status word: it rides with the next notice.
        let interim = format!("Phase 1 is in, phase 2 under way. {}", "Evidence. ".repeat(45));
        assert!(matches!(line("ir-top", &format!("{notice}{}", said(&interim))), Some((None, Settled::Skip(2)))));
        // A status word lower down is none: it opens the message or does not count.
        assert!(matches!(line("ir-top", &format!("{notice}{}", said("Notes.\nDONE: all of it"))), Some((None, Settled::Skip(2)))));
        // Only its parent: a session that wrote to it is told of its own turns alone.
        assert!(matches!(line("ir-peer", &format!("{notice}{}", said(&report))), Some((None, Settled::Skip(2)))));
        // A turn on a notice about anyone but its own helpers is no news for the parent.
        notice_turn(&mut reg().lock().unwrap(), "ir-mid", 2, &["ir-low".into(), "ir-peer".into()], false);
        reg().lock().unwrap().notice_turns.get_mut("ir-mid").unwrap().remove(0);
        assert!(matches!(line("ir-top", &format!("{notice}{}", said(&report))), Some((None, Settled::Skip(2)))));
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
        assert!(matches!(notice_line(pty, "nn-c", "Your helper", "nn-q", &known, stood), Some((Some(_), Settled::NoNews(_)))));
        let went = Owed { job_went: true, ..stood };
        let Some((Some(text), Settled::Answered)) = notice_line(pty, "nn-c", "Your helper", "nn-q", &known, went) else {
            panic!("a core took it")
        };
        assert!(text.contains("that Luna's core has restarted"), "{text}");
        // Held past its grace, a wait still gets it: the debt is open, on the one clock.
        let key = ("nn-c".to_string(), "nn-q".to_string());
        reg().lock().unwrap().owed.insert(key.clone(), stood);
        assert!(job_end_stood(&key, 3, true));
        // At work again: no line, and its end counts from nothing.
        crate::background::left_for_test("nn-q", &[("build.exe", 4242, 0)]);
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
            assert!(text.contains("luna_message_list with screen: true shows where it is"), "{text}");
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
        let Some((Some(text), Settled::Answered)) = notice_line(pty, "bs-c", "Your helper", "bs-s", &known, stood) else {
            panic!("its work cut off is news")
        };
        assert!(
            text.contains("nothing left running now, and it has said nothing since your last notice. Its CLI stopped the subagents"),
            "{text}"
        );
        assert!(text.contains("may be unfinished and uncommitted") && !text.contains("does not take that up"), "{text}");
        let turn = Owed { told_pending: false, baseline: 2, cursor: 0, ..o };
        let Some((Some(text), Settled::Answered)) = notice_line(pty, "bs-c", "Your helper", "bs-s", &known, turn) else {
            panic!("its turn is over")
        };
        assert!(text.starts_with("Your helper finished its turn") && text.contains("Its CLI stopped the subagents"), "{text}");
        // A core that goes before the notice is in: the next one still says it.
        crate::background::restart_for_test("bs-s");
        let Some((Some(text), Settled::Answered)) = notice_line(pty, "bs-c", "Your helper", "bs-s", &known, stood) else {
            panic!("still news after a restart")
        };
        assert!(text.contains("Its CLI stopped the subagents"), "{text}");
        // Who has had it is kept across cores too; once everyone it concerns has, it is done.
        crate::background::background_stop_told("bs-s", "bs-c", &["bs-c".to_string(), "bs-other".to_string()]);
        crate::background::restart_for_test("bs-s");
        let told = notice_line(pty, "bs-c", "Your helper", "bs-s", &known, stood).and_then(|(t, _)| t).unwrap_or_default();
        assert!(!told.contains("Its CLI stopped"), "said to this caller already: {told}");
        assert!(crate::background::background_stopped_for("bs-s", "bs-other"));
        crate::background::background_stop_told("bs-s", "bs-other", &["bs-other".to_string()]);
        assert!(!crate::background::background_stopped("bs-s"));
        crate::background::gone_for_test("bs-s");
        forget("bs-s");
        let _ = std::fs::remove_file(&path);
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
        assert!(text.contains("has written its answer, but its turn is not over"), "{text}");
        assert!(text.contains("It waits on: Bash `npm run dev`."), "{text}");
        let both = Owed { told_background_wait: Some(3), ..told };
        assert!(matches!(notice_line(pty, caller, "Your helper", id, &known, both), None | Some((None, _))), "and that once too");
        // Its job at work (a subagent's long command) is no quiet turn, though
        // the stream says nothing; all of it idle as long is.
        crate::background::watched_for_test(id, &[("claude.exe", 1, QUIET_TURN_MS + 60_000), ("cargo.exe", 2, 60_000)]);
        assert!(matches!(notice_line(pty, caller, "Your helper", id, &known, told), None | Some((None, _))), "its job works");
        crate::background::watched_for_test(id, &[("claude.exe", 1, QUIET_TURN_MS + 60_000), ("node.exe", 2, QUIET_TURN_MS + 60_000)]);
        let Some((Some(text), Settled::WaitsOnBackground(3))) = notice_line(pty, caller, "Your helper", id, &known, told) else {
            panic!("all of it idle")
        };
        assert!(text.contains("processes claude.exe, node.exe") && text.contains("luna_session_stop ends it now"), "{text}");
        // A watcher that keeps at it (a dev server's polling) holds the notice
        // back only so long.
        crate::background::watched_for_test(id, &[("claude.exe", 1, QUIET_TURN_MS + 60_000), ("node.exe", 2, 1_000)]);
        assert!(matches!(notice_line(pty, caller, "Your helper", id, &known, told), None | Some((None, _))), "its job works");
        crate::background::quiet_for_test(id, JOB_HOLDS_MS + 60_000);
        let Some((Some(text), Settled::WaitsOnBackground(3))) = notice_line(pty, caller, "Your helper", id, &known, told) else {
            panic!("a busy watcher past the bound")
        };
        assert!(text.contains("for 91 min. It waits on: Bash `npm run dev`; processes claude.exe, node.exe."), "{text}");
        crate::background::quiet_for_test(id, QUIET_TURN_MS + 60_000);
        // A subagent among what it waits on: said without calling it a hang.
        let agent = json!({ "type": "assistant", "timestamp": at, "message": { "content": [{ "type": "tool_use", "id": "t2", "name": "Agent",
            "input": { "description": "port the parser", "prompt": "…", "run_in_background": true } }] } });
        let launched = json!({ "type": "user", "message": { "content": [{ "type": "tool_result", "tool_use_id": "t2", "content": "…" }] },
            "toolUseResult": { "isAsync": true, "status": "async_launched", "agentId": "a1" } });
        std::fs::write(&path, format!("{call}\n{started}\n{agent}\n{launched}\n")).unwrap();
        // A subagent's long command at work holds it back.
        crate::background::watched_for_test(id, &[("claude.exe", 1, QUIET_TURN_MS + 60_000), ("cargo.exe", 2, 1_000)]);
        assert!(matches!(notice_line(pty, caller, "Your helper", id, &known, told), None | Some((None, _))), "the subagent works");
        crate::background::watched_for_test(id, &[("claude.exe", 1, QUIET_TURN_MS + 60_000), ("node.exe", 2, QUIET_TURN_MS + 60_000)]);
        let Some((Some(text), _)) = notice_line(pty, caller, "Your helper", id, &known, told) else { panic!("still news") };
        assert!(text.contains("subagent \"port the parser\"") && text.contains("may still be at work"), "{text}");
        assert!(!text.contains("luna_session_stop ends it now"), "{text}");
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
            Some((Some(line), Settled::Answered)) => line,
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
            let Some((Some(line), Settled::Answered)) = notice_line(pty, "ws-caller", "Your helper", id, &known, o) else {
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
        // The same transcript with nothing cut: a prompt last is a reply to come.
        helper("ws-open");
        assert!(line_of("ws-open", format!("{worked}{}", user("then the tests")), false).contains("without a reply"));
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
        assert!(matches!(how, Settled::Answered));
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
        assert!(matches!(how, Settled::Answered));
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
        assert!(matches!(how, Settled::Answered));
        assert!(line.contains("has nothing left running now that Luna's core has restarted"), "{line}");
        // And it comes back marked again from what this core saves.
        assert_eq!(ties_of("tp-helper").unwrap().told_pending, vec!["tp-caller".to_string()]);
        crate::background::gone_for_test("tp-helper");
        forget("tp-helper");
    }

    /// An assistant line of a Claude Code transcript.
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
        register(top, Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, serde_json::json!({}));
        register(id, Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some(top), true, serde_json::json!({ "background": true }));
        crate::background::idle_for_test(id, Provider::Claude, turns, false);
        if job {
            crate::background::left_for_test(id, &[("bash.exe", 4242, 0)]);
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
        let (top, id) = ("ln-top", "ln-lead");
        let interim = format!("Gate started, both checkers out; nothing for you yet. {}", "Detail. ".repeat(60));
        let (known, path) = lead(top, id, 1, true, &said_line(&interim));
        let pty = crate::pty::manager();
        let line = |o: Owed| notice_line(pty, top, "Your helper", id, &known, o);
        let first = held(Owed::from(0, 0), 1, BACKGROUND_GRACE_MS);
        assert!(matches!(line(first), Some((None, Settled::Skip(1)))), "no notice, and no NOT done yet: that is a lead's normal state");
        // As the notifier leaves it: told it is not done, what it said still to come.
        let waiting = Owed { rev: first.rev, ..Owed::pending(1, 0) };
        assert!(!matches!(line(waiting), Some((Some(_), _))), "nothing to say while the job runs");
        // A status word gets through at once, delegates or not, and is no NOT done yet.
        for word in ["DONE", "BLOCKED: the gate", "QUESTION which branch?", "FAILED:", "PLAN: three lanes", "**DONE**: all of it"] {
            std::fs::write(&path, said_line(&format!("{word}\nmore"))).unwrap();
            let Some((Some(text), Settled::Pending(1, _))) = line(first) else { panic!("{word} goes at once") };
            assert!(text.contains("ended a turn") && !text.contains("NOT done yet"), "{text}");
            assert!(text.contains("a job it started still runs in the background"), "{text}");
        }
        for not_one in ["done: lowercase", "DONEZO", "Status: DONE"] {
            std::fs::write(&path, said_line(not_one)).unwrap();
            assert!(matches!(line(first), Some((None, Settled::Skip(1)))), "{not_one}");
        }
        // Its parent asked it something: the answer is no interim report.
        std::fs::write(&path, said_line("Lane 2 of 3; the gate is still out.")).unwrap();
        let sent = Owed { by_send: true, ..first };
        let Some((Some(text), Settled::Pending(1, _))) = line(sent) else { panic!("an answer to a send goes") };
        assert!(text.contains("Lane 2 of 3") && !text.contains("NOT done yet"), "{text}");
        let quiet = Owed { no_reply: true, ..sent };
        assert!(matches!(line(quiet), Some((None, Settled::Skip(1)))), "unless the send asked for no reply");
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

    /// Sub-orchestrators of 13 November "has nothing left running now, and it
    /// has said nothing" seven times in a night, each woken by hand: Luna wakes
    /// the lead itself, once per such end, and its parent hears of the turn
    /// that brings, or that it seems stuck if that turn says nothing.
    #[test]
    fn a_lead_whose_delegates_finished_is_woken_once_and_its_parent_hears_of_what_came_of_it() {
        let (top, id) = ("wk-top", "wk-lead");
        let text = said_line("Waiting on the gate and both checkers.");
        let (known, path) = lead(top, id, 1, false, &text);
        reg().lock().unwrap().delegates.insert(id.into(), vec!["Bash `scripts/gate.ps1`".into(), "helper wk-low".into()]);
        let pty = crate::pty::manager();
        let line = |o: Owed| notice_line(pty, top, "Your helper", id, &known, o);
        let wakes = || reg().lock().unwrap().news.iter().filter(|(c, l)| c == id && is_wake(l)).count();
        let told = Owed::pending(1, text.len() as u64);
        reg().lock().unwrap().owed.insert((top.into(), id.into()), told);
        let over = held(told, 1, BACKGROUND_GRACE_MS);
        assert!(line(over).is_none(), "its parent hears nothing yet");
        assert_eq!(wakes(), 1, "the lead is woken");
        let wake = reg().lock().unwrap().news.iter().find(|(c, _)| c == id).map(|(_, l)| l.clone()).unwrap();
        assert!(wake.starts_with("Your delegates have finished: Bash `scripts/gate.ps1`; helper wk-low."), "{wake}");
        assert!(line(over).is_none());
        assert_eq!(wakes(), 1, "once per end");
        // Its turn on the wake ends without a word: stuck, said once (it settles the debt).
        notice_turn(&mut reg().lock().unwrap(), id, 2, &[], true);
        crate::background::idle_for_test(id, Provider::Claude, 2, false);
        let Some((Some(stuck), Settled::Answered)) = line(held(told, 2, REPLY_GRACE_MS)) else { panic!("its parent hears") };
        assert!(stuck.contains("seems stuck") && stuck.contains("without a word"), "{stuck}");
        // One that takes it up is heard of as any turn of its own.
        std::fs::write(&path, format!("{text}{}", said_line("DONE: gate green, both checks pass."))).unwrap();
        let Some((Some(done), Settled::Answered)) = line(held(told, 2, REPLY_GRACE_MS)) else { panic!("its turn on the wake") };
        assert!(done.contains("finished its turn") && done.contains("DONE: gate green"), "{done}");
        // Stopped by the breaker, it is not woken, and its parent hears as before.
        reg().lock().unwrap().news.retain(|(c, _)| c != id);
        reg().lock().unwrap().woken.remove(id);
        reg().lock().unwrap().stopped.insert((id.into(), id.into()), now_ms());
        crate::background::idle_for_test(id, Provider::Claude, 1, false);
        std::fs::write(&path, &text).unwrap();
        let Some((Some(plain), Settled::NoNews(_))) = line(over) else { panic!("as before") };
        assert!(plain.contains("has nothing left running now"), "{plain}");
        assert_eq!(wakes(), 0);
        reg().lock().unwrap().stopped.remove(&(id.to_string(), id.to_string()));
        // A wake not taken up in WAKE_WAIT_MS: its parent hears.
        reg().lock().unwrap().woken.insert(id.into(), Woken { ended: 1, cursor: 0, at: now_ms() - WAKE_WAIT_MS });
        assert!(matches!(line(over), Some((Some(_), Settled::NoNews(_)))));
        // A turn a wake started counts as the lead's own for every caller.
        assert!(real_turn_past(id, top, 1, 2, true, || false));
        for s in [id, top] {
            crate::background::gone_for_test(s);
            forget(s);
        }
        let _ = std::fs::remove_file(&path);
    }

    /// A lead mid-switch to a fresh conversation (handoff/lead.rs) is not
    /// woken: its callers wait for what the fresh one says. Once it starts, a
    /// wake queued or given in the one it leaves is dropped, and an end of its
    /// delegates still unheard of wakes it again.
    #[test]
    fn a_lead_switching_to_a_fresh_conversation_is_not_woken_and_leaves_its_wake_behind() {
        use crate::background::{LeadPhase, LeadSwitch};
        let (top, id) = ("wks-top", "wks-lead");
        let text = said_line("Waiting on the gate.");
        let (known, path) = lead(top, id, 1, false, &text);
        let pty = crate::pty::manager();
        let line = |o: Owed| notice_line(pty, top, "Your helper", id, &known, o);
        let wakes = || reg().lock().unwrap().news.iter().filter(|(c, l)| c == id && is_wake(l)).count();
        let told = Owed::pending(1, text.len() as u64);
        reg().lock().unwrap().owed.insert((top.into(), id.into()), told);
        let over = held(told, 1, BACKGROUND_GRACE_MS);
        // Its fresh conversation named and not yet recorded as through: it reads
        // between turns, and still no wake goes.
        let s = LeadSwitch {
            n: 1,
            copy: String::new(),
            sha256: String::new(),
            source: String::new(),
            phase: LeadPhase::Done,
            before: Some("old".into()),
            previous: None,
            opening_id: None,
            opened: Some("new".into()),
            from_records: false,
        };
        crate::background::set_switch_for_test(id, Some(s));
        assert!(line(over).is_none(), "its parent waits");
        assert_eq!(wakes(), 0, "not woken mid-switch");
        crate::background::set_switch_for_test(id, None);
        assert!(line(over).is_none());
        assert_eq!(wakes(), 1, "woken once it is through");
        // A fresh start leaves that wake behind, queued or given.
        lead_goes_fresh(id);
        assert_eq!(wakes(), 0, "dropped");
        assert!(!reg().lock().unwrap().woken.contains_key(id), "and no seems-stuck from the transcript left");
        assert!(line(over).is_none());
        assert_eq!(wakes(), 1, "its delegates' end wakes it again");
        for s in [id, top] {
            crate::background::gone_for_test(s);
            forget(s);
        }
        let _ = std::fs::remove_file(&path);
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
            std::fs::write(&path, said_line(r)).unwrap();
            let o = reg().lock().unwrap().owed.get(&(caller.to_string(), id.to_string())).copied().unwrap();
            notice_line(pty, caller, "Your helper", id, &known, held(o, 1, REPLY_GRACE_MS))
        };
        assert!(matches!(o("Understood."), Some((None, Settled::Unasked))), "no notice");
        assert!(matches!(o("DONE: rebased"), Some((Some(_), Settled::Answered))), "a status word");
        assert!(matches!(o(&"A long report. ".repeat(40)), Some((Some(_), Settled::Answered))), "a report");
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
        assert!(text.contains("answering your request 1, request 2 (requestKey \"pr-7772\")"), "{text}");
        assert!(text.contains("Its report, re request 2 (requestKey \"pr-7772\"), begins: \"VERDICT: GO"), "{text}");
        let file = text.split("the answer is in ").nth(1).and_then(|t| t.split(", its report first").next()).unwrap();
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

    /// Part 2 of the feedback: "only idle processes left: timeout.exe, grep.exe,
    /// tail.exe, 6 more", and nothing of what they waited on.
    #[test]
    fn an_idle_notice_names_what_the_transcript_started_them_as() {
        use serde_json::json;
        let (known, o, path) = job_over_helper("in-s", "in-c", "");
        let call = json!({ "type": "assistant", "timestamp": "2026-11-13T01:00:00Z", "message": { "content": [{ "type": "tool_use", "id": "t1", "name": "Bash",
            "input": { "command": "timeout 3600 tail -f lane.log | grep -m1 DONE", "run_in_background": true } }] } });
        let started = json!({ "type": "user", "message": { "content": [{ "type": "tool_result", "tool_use_id": "t1", "content": "…" }] },
            "toolUseResult": { "backgroundTaskId": "b1" } });
        std::fs::write(&path, format!("{call}\n{started}\n")).unwrap();
        crate::background::left_for_test("in-s", &[("timeout.exe", 1, IDLE_JOB_MS + 60_000), ("tail.exe", 2, IDLE_JOB_MS + 60_000)]);
        let Some((Some(text), Settled::Idle(_))) = notice_line(crate::pty::manager(), "in-c", "Your helper", "in-s", &known, o) else {
            panic!("idle")
        };
        assert!(text.contains("Its transcript has them started as: Bash `timeout 3600 tail -f lane.log | grep -m1 DONE`."), "{text}");
        crate::background::gone_for_test("in-s");
        forget("in-s");
        let _ = std::fs::remove_file(&path);
    }
}
