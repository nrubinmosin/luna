//! The notifier: a loop that types a one-line notice into an idle caller once
//! what it is owed has come (an answer, a prompt, an exit), and the luna_notify
//! tool's desktop notification.

pub(super) mod lines;
pub(super) mod usage;

use crate::agents::ask::live;
use crate::agents::delivery::{still_starting, type_text, TypingInto, DRAFT_ALERT_MS};
use crate::agents::lost::{take_undelivered, worktrees_left_line};
use crate::agents::notifier::lines::{
    count_notice, last_notice, notice_line, notice_turn, pair_stopped, standing_line, BACKGROUND_GRACE_MS, BREAKER_NOTICES,
    BREAKER_WINDOW_MS,
};
use crate::agents::notifier::usage::{
    budget_news, quota_file, quota_from, quota_news, save_quota_told, your_helper, BudgetNews, QuotaNews,
};
use crate::agents::owed::Owed;
use crate::agents::persist::{core_up_ms, SESSION_RETURN_MS};
use crate::agents::registry::{chat_name, label, parent_of, reg, waits_on_helpers, Known, Registry};
use crate::agents::spawn::settle::{settle_spawns, spawn_news};
use crate::agents::teardown::reap;
use crate::provider::Provider;
use crate::throttle::now_ms;
use std::collections::HashMap;

/// How often the notifier looks for answers nobody collected.
pub(super) const NOTIFY_EVERY: std::time::Duration = std::time::Duration::from_secs(2);

/// Looks in a row the caller has to be found idle before a notice goes in:
/// a turn that has just ended may still be followed by the one it queued.
pub(super) const NOTIFY_IDLE_LOOKS: u32 = 2;

/// A person typed into the caller this recently: hold the notice, it would
/// land in the middle of what they are writing.
const TYPING_QUIET_MS: u64 = 20_000;

/// A `wait` out for an answer hands it over itself, so the notifier holds its
/// notice — this long past the answer at most, for a client that has dropped
/// the call it moved to the background.
pub(super) const WAIT_HOLD_MS: u64 = 10_000;

/// How long into a core's life an answer stays owed to a caller it has not
/// met. What a helper is owed comes back with the helper, before anything
/// else (`tie_back`), and the chat it is owed to only when the window starts
/// that chat's CLI again, seconds later; but a caller that never comes back
/// must not keep a helper from `reap` for good.
pub(super) const CALLER_RETURN_MS: u64 = 10 * 60 * 1000;

/// How every notice opens, so a model tells it from what the user types.
pub const NOTICE_TAG: &str = "[Luna notice, not typed by the user]";

/// How many of the notifier's looks go by between reads of the transcripts of
/// helpers with a budget: finding a visible one's goes to the disk.
const BUDGET_EVERY_LOOKS: u32 = 5;

/// The shortest gap between two notifications from one session: a model in
/// a loop must not bury the desktop in them.
const NOTIFY_GAP_MS: u64 = 10_000;

/// A desktop notification from `caller`, under its own chat's name so it
/// cannot pass for another; a click on it opens that chat.
pub fn notify(caller: &str, text: &str) -> Result<(), String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("text is empty".into());
    }
    let provider = {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let now = now_ms();
        if let Some(last) = r.notified.get(caller) {
            let left = NOTIFY_GAP_MS.saturating_sub(now.saturating_sub(*last));
            if left > 0 {
                return Err(format!("not shown: one notification per {} s, try again in {} s", NOTIFY_GAP_MS / 1000, left.div_ceil(1000)));
            }
        }
        r.notified.insert(caller.to_string(), now);
        r.known.get(caller).map(|k| k.provider)
    };
    let who = match provider {
        Some(Provider::Codex) => "Codex",
        _ => "Claude",
    };
    let chat = chat_name(caller).unwrap_or_else(|| caller.to_string());
    // A toast shows a few lines at most; the rest would only be cut.
    let body: String = text.chars().take(300).collect();
    crate::power::notify_chat(caller, &format!("“{chat}” · {who}"), &body);
    Ok(())
}

/// What became of a session the caller is owed an answer by.
pub(super) enum Settled {
    /// A turn past the one asked about ended.
    Answered,
    /// A turn ended, but the session left a job running in the background;
    /// owed on from this turn count and transcript end, for that job's end or
    /// a turn of its own.
    Pending(u32, u64),
    /// A turn ended but is not settled — processes still at work, or no
    /// reply in the transcript yet: nothing to say, start the grace from now.
    Hold(u32),
    /// A job looked over and is at work again: the grace starts over.
    Unhold,
    /// Told it is not done, and all that ended since were turns on Luna's own
    /// notices: nothing to say, and owed on from this turn count, what they
    /// said included.
    Skip(u32),
    Waiting,
    /// It asks a question it does not wait on; told once per question.
    Asked(u64),
    /// A core's going stopped its turn and it went on by itself; told once.
    Resumed,
    /// All its job left has sat idle for IDLE_JOB_MS, past that since this
    /// moment; told once, and owed on. It rides (`riders_go`).
    Idle(u64),
    /// Its job's end, seen at this moment, with no word from it and nothing
    /// else to tell: settles as `Answered`, and rides (`riders_go`).
    NoNews(u64),
    /// Its turn at this count has gone quiet (QUIET_TURN_MS); told once per
    /// turn, and owed on.
    Quiet(u32),
    Exited,
}

/// Whether a notice may go into `caller` now: its CLI is up and between turns
/// and nobody is typing into it. A draft in its composer is `type_text`'s to
/// put aside, or to keep the notice back for.
fn ready_for_notice(pty: &crate::pty::PtyManager, caller: &str) -> bool {
    // A background caller takes it as its next turn, between turns.
    if crate::background::has(caller) {
        return crate::activity::turn_state(caller).is_some_and(|t| t.turn == crate::activity::Turn::Idle);
    }
    pty.alive(caller)
        && crate::activity::turn_state(caller).is_some_and(|t| t.turn == crate::activity::Turn::Idle)
        && !still_starting(pty, caller)
        && pty.typed_ago_ms(caller).is_none_or(|ms| ms >= TYPING_QUIET_MS)
        && drawn_for_notice(pty, caller)
}

/// A CLI just started has drawn something, and may not be taking text yet: its
/// prompt says it is. For its first minute only: a prompt Luna cannot read (a mode
/// with a mark of its own, a CLI that draws it differently) must not silence notices.
fn drawn_for_notice(pty: &crate::pty::PtyManager, id: &str) -> bool {
    let long_up = pty.probe(id).is_none_or(|p| (now_ms() as u128).saturating_sub(p.spawned_at_ms) >= SESSION_RETURN_MS as u128);
    long_up || pty.at_prompt(id) || pty.has_draft(id)
}

/// Whether any of these owed answers has come in, without saying it yet.
fn anything_to_say(pty: &crate::pty::PtyManager, items: &[(String, Known, Owed, String)]) -> bool {
    items.iter().any(|(id, _, o, _)| {
        let ts = crate::activity::turn_state(id);
        let ended = ts.map(|t| t.turns_ended).unwrap_or(0);
        o.resumed || (ended > o.baseline && ts.map(|t| t.turn) != Some(crate::activity::Turn::Busy)) || job_over(o, ts) || !live(pty, id)
    })
}

/// Whether a caller told a session is not done yet may have that session's job
/// to hear of: between turns, with nothing of its own left running. For the
/// holds that wait on news; helpers still to answer it keep `notice_line` from
/// saying so.
fn job_over(o: &Owed, ts: Option<crate::activity::TurnState>) -> bool {
    o.told_pending && ts.is_some_and(|t| t.turn == crate::activity::Turn::Idle && !t.children_busy)
}

/// What a session's turns left running and has done nothing (no CPU, no I/O:
/// `left_quiet`) for this long, its caller hears of once (`Settled::Idle`): that
/// may be a job that is stuck, or one waiting on something, which its caller can
/// end. Not its end, which doing nothing is no proof of: the debt stays open. A
/// real job's quiet stretch is minutes (a link, a test on a timeout).
pub(super) const IDLE_JOB_MS: u64 = 30 * 60 * 1000;

/// Whether anything a session's turns left still holds the machine
/// (`procs::Leftover::holds`): with nothing that does, it is no job at work for
/// `reap` either.
pub(super) fn left_holds(id: &str) -> bool {
    if crate::background::has(id) {
        return crate::background::left_holds(id);
    }
    crate::activity::children_hold(id)
}

/// Between turns, how long all a session's turns left running has done nothing,
/// and their names: a helper's job's (background/), a chat's under its CLI
/// (activity/). None with nothing left.
pub(super) fn left_quiet(id: &str) -> Option<(u64, String)> {
    if crate::background::has(id) {
        return crate::background::left_quiet(id);
    }
    crate::activity::left_quiet(id)
}

/// For a `wait`: whether a job's end (`over`) has stood BACKGROUND_GRACE_MS, by
/// the debt's own clock (`Owed::held`), the one the notifier goes by too: with
/// a clock each, the notice could go out while the wait still counted, and the
/// caller hear of it twice. A job at work again starts it over. With no debt
/// left to clock (a notice has just settled it), there is nothing to hold for.
pub(super) fn job_end_stood(key: &(String, String), turns_ended: u32, over: bool) -> bool {
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let Some(o) = r.owed.get_mut(key) else { return over };
    if !over {
        o.held = None;
        return false;
    }
    match o.held {
        Some((n, at)) if n == turns_ended => now_ms().saturating_sub(at) >= BACKGROUND_GRACE_MS,
        _ => {
            o.held = Some((turns_ended, now_ms()));
            false
        }
    }
}

/// Whether a `wait` the caller has out on `id` is to hand this answer over
/// rather than a notice: while nothing has come in, and for WAIT_HOLD_MS
/// after something did — the wait answers within a second or two, so one
/// still out by then belongs to a client that has let the call go. A job's end
/// comes in only once it has stood its grace, on the clock the wait goes by
/// too (`job_end_stood`), which this starts for a wait that is not counting.
pub(super) fn held_for_wait(pty: &crate::pty::PtyManager, caller: &str, id: &str, o: &Owed) -> bool {
    let key = (caller.to_string(), id.to_string());
    {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        if r.reserved.contains_key(&key) {
            return true;
        }
        if !r.waits.contains_key(&key) {
            return false;
        }
    }
    let ts = crate::activity::turn_state(id);
    let turn = ts.map(|t| t.turn);
    let alive = live(pty, id);
    let ended = ts.map_or(0, |t| t.turns_ended);
    // A prompt it stopped on is news too: `wait(turn_done)` hands that back.
    let other_news =
        (ended > o.baseline && turn != Some(crate::activity::Turn::Busy)) || turn == Some(crate::activity::Turn::Waiting) || !alive;
    let job_ended = alive && job_over(o, ts) && !waits_on_helpers(id, caller);
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let Some(owed) = r.owed.get_mut(&key) else { return true };
    let job_news = job_ended && {
        let since = match owed.held {
            Some((n, at)) if n == ended => at,
            _ => {
                let at = now_ms();
                owed.held = Some((ended, at));
                at
            }
        };
        now_ms().saturating_sub(since) >= BACKGROUND_GRACE_MS
    };
    if !other_news && !job_news {
        // Taken back by a turn begun since (a queued send): the wait gets
        // its whole hold again for the news that turn brings.
        owed.ready_at = None;
        // A job at work again: its end starts its grace over.
        if owed.told_pending && !job_ended {
            owed.held = None;
        }
        return true;
    }
    let at = *owed.ready_at.get_or_insert(now_ms());
    now_ms().saturating_sub(at) < WAIT_HOLD_MS
}

/// Whether what `caller` is owed by `id` still stands `up_ms` into this core's
/// life: a delete takes the session or the caller from `known`, but a caller
/// not started under this core yet may be one on its way back
/// (CALLER_RETURN_MS).
pub(super) fn owed_stands(known: &HashMap<String, Known>, caller: &str, id: &str, up_ms: u64) -> bool {
    known.contains_key(id) && (known.get(caller).is_some_and(|k| !k.waiting) || up_ms < CALLER_RETURN_MS)
}

/// The longest a line with no news in it waits for a notice to ride with: a
/// caller may be waiting on that very job while another helper runs for days.
const RIDE_MAX_MS: u64 = 10 * 60 * 1000;

/// Since when a line that is no reason to wake its caller (`Settled::NoNews`,
/// `Settled::Idle`) has stood, if it is one.
fn rides_since(how: &Settled) -> Option<u64> {
    match how {
        Settled::NoNews(since) | Settled::Idle(since) => Some(*since),
        _ => None,
    }
}

/// Whether such lines, the oldest standing since `oldest`, go out now. A wake
/// costs a caller a read of its whole context, so they go with a notice it is
/// getting anyway (`company`). Alone they go once all it is owed is such lines
/// (`only_riders`), or with nothing anywhere at work (`busy` 0), so no notice is
/// on its way: all that lets a power-off start counting (power/rule.rs `advance`), which
/// is not to go ahead of a line held here. That is this gate's word only: a line
/// past it can still be held back, as any notice can, by a draft in its caller's
/// composer or a turn it is on, and a power-off does not wait for that. Their debts
/// stay open until then, for a `luna_wait` and against `reap`, and what happens to
/// the session meanwhile replaces the line. A budget line rides the same way, with
/// no debt to it.
pub(crate) fn riders_go(company: bool, only_riders: bool, oldest: u64, now: u64, busy: usize) -> bool {
    company || only_riders || busy == 0 || now.saturating_sub(oldest) >= RIDE_MAX_MS
}

/// The longest a notice ready for an idle caller waits for another helper of
/// the caller's that is mid-turn, so that two finishing seconds apart come as
/// one notice, and one turn of the caller's, rather than two. Short beside
/// RIDE_MAX_MS: an answer is news, and a caller may be waiting on it.
pub(super) const COALESCE_MS: u64 = 20_000;
// Within what a line with no news waits for company.
const _: () = assert!(COALESCE_MS < RIDE_MAX_MS);

/// Whether a notice first ready at `first_ready` waits on at `now`: only while
/// something else the caller is owed is at work (`others_working`), and never
/// past COALESCE_MS.
pub(super) fn coalesce_holds(first_ready: u64, now: u64, others_working: bool) -> bool {
    others_working && now.saturating_sub(first_ready) < COALESCE_MS
}

/// `coalesce_holds` for `caller` at `now`, keeping when its notice was first
/// ready in `since`; a notice that goes out takes its entry with it.
fn hold_for_company(since: &mut HashMap<String, u64>, caller: &str, now: u64, others_working: bool) -> bool {
    let first = *since.entry(caller.to_string()).or_insert(now);
    let holds = coalesce_holds(first, now, others_working);
    if !holds {
        since.remove(caller);
    }
    holds
}

/// Runs for the life of the app: every few seconds, for each caller owed an
/// answer, checks whether one came and tells the caller once it is idle; and
/// deletes the `autoDelete` helpers that are done (see `reap`).
pub(super) fn notifier() {
    if let Ok(bytes) = std::fs::read(quota_file()) {
        reg().lock().unwrap_or_else(|e| e.into_inner()).quota_told = quota_from(&bytes, now_ms());
    }
    let mut idle_looks: HashMap<String, u32> = HashMap::new();
    // Callers whose notice a draft is holding back: since when, and whether
    // the desktop has been told.
    let mut drafts: HashMap<String, (u64, bool)> = HashMap::new();
    // When each caller's notice was first ready while another of its helpers
    // worked on (`hold_for_company`).
    let mut company_since: HashMap<String, u64> = HashMap::new();
    let mut looks_at_budgets = 0;
    loop {
        std::thread::sleep(NOTIFY_EVERY);
        let pty = crate::pty::manager();
        reap(pty);
        // Owed by caller, each session with what is known of it; anything a
        // delete has made unknown is dropped on the way, and a caller yet to
        // come back has nothing to be told in.
        let mut by_caller: std::collections::BTreeMap<String, Vec<(String, Known, Owed, String)>> = Default::default();
        let mut gave_up: Vec<(String, String)> = Vec::new();
        {
            let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
            let Registry { owed, known, names, .. } = &mut *r;
            let up_ms = core_up_ms();
            owed.retain(|(caller, id), _| {
                let stands = owed_stands(known, caller, id, up_ms);
                if !stands && known.contains_key(id) {
                    gave_up.push((caller.clone(), id.clone()));
                }
                stands
            });
            for ((caller, id), o) in owed.iter() {
                let Some(k) = known.get(id) else { continue };
                if known.get(caller).is_none_or(|k| k.waiting) {
                    continue;
                }
                let parent = k.parent.as_deref() == Some(caller.as_str());
                let name = names.get(id).map(|n| n.trim()).filter(|n| !n.is_empty());
                let who = match (parent, name) {
                    (true, _) => your_helper(id, name),
                    (false, Some(n)) => format!("Session \"{n}\" ({id}), which you wrote to,"),
                    (false, None) => format!("Session {id}, which you wrote to,"),
                };
                by_caller.entry(caller.clone()).or_default().push((id.clone(), k.clone(), *o, who));
            }
        }
        for (caller, id) in gave_up {
            crate::log::warn(
                "agents",
                &format!(
                    "{caller} is owed the answer of {} but has not come back in {} min; no notice will go",
                    label(&id),
                    CALLER_RETURN_MS / 60_000
                ),
            );
        }
        // A caller owed nothing still hears of the quota its helpers run on,
        // and of a spawn of its own that a core's going cut off.
        let mut quota = quota_news(pty);
        for caller in quota.keys() {
            by_caller.entry(caller.clone()).or_default();
        }
        looks_at_budgets = (looks_at_budgets + 1) % BUDGET_EVERY_LOOKS;
        let (mut budget, budgets_moved) = budget_news(pty, looks_at_budgets == 0);
        if budgets_moved {
            crate::background::save();
        }
        for caller in budget.keys() {
            by_caller.entry(caller.clone()).or_default();
        }
        settle_spawns(core_up_ms());
        let mut news: HashMap<String, Vec<String>> = HashMap::new();
        {
            let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
            let Registry { news: queued, known, .. } = &mut *r;
            queued.retain(|(caller, _)| known.contains_key(caller));
            for (caller, line) in queued.iter() {
                news.entry(caller.clone()).or_default().push(line.clone());
            }
        }
        for caller in news.keys() {
            by_caller.entry(caller.clone()).or_default();
        }
        let mut spawns = spawn_news(core_up_ms());
        for caller in spawns.keys() {
            by_caller.entry(caller.clone()).or_default();
        }
        idle_looks.retain(|c, _| by_caller.contains_key(c));
        drafts.retain(|c, _| by_caller.contains_key(c));
        company_since.retain(|c, _| by_caller.contains_key(c));
        reg().lock().unwrap_or_else(|e| e.into_inner()).notices_held.retain(|c, _| by_caller.contains_key(c));
        for (caller, items) in by_caller {
            // The last notice is still going in: it starts a turn of its own.
            if TypingInto::under_way(&caller) {
                idle_looks.insert(caller, 0);
                continue;
            }
            if pty.has_draft(&caller) && pty.alive(&caller) && anything_to_say(pty, &items) {
                let (since, told) = drafts.entry(caller.clone()).or_insert((now_ms(), false));
                if !*told && now_ms().saturating_sub(*since) >= DRAFT_ALERT_MS {
                    *told = true;
                    let chat = chat_name(&caller).unwrap_or_else(|| caller.clone());
                    crate::power::notify_chat(
                        &caller,
                        "A helper has answered",
                        &format!("\"{chat}\" has an unsent draft, so Luna is holding the helper's notice until you send or clear it."),
                    );
                }
            } else {
                drafts.remove(&caller);
            }
            let looks = idle_looks.entry(caller.clone()).or_default();
            *looks = if ready_for_notice(pty, &caller) { *looks + 1 } else { 0 };
            if *looks < NOTIFY_IDLE_LOOKS {
                continue;
            }
            let mut lines = Vec::new();
            let mut settled = Vec::new();
            let mut riders = Vec::new();
            for (id, k, o, who) in &items {
                if held_for_wait(pty, &caller, id, o) {
                    continue;
                }
                let said = if pair_stopped(pty, &caller, id) {
                    standing_line(pty, &caller, who, id, k, *o)
                } else {
                    notice_line(pty, &caller, who, id, k, *o)
                };
                match said {
                    Some((Some(mut line), how)) => {
                        // Told once, with whatever is told first.
                        if k.parent.as_deref() == Some(caller.as_str()) {
                            line.push_str(&worktrees_left_line(id));
                        }
                        if counts_for_breaker(&how) && last_notice(&caller, id, now_ms()) {
                            line.push_str(&format!(
                                " Luna stops telling you of its turns and jobs now: {BREAKER_NOTICES} notices in {} minutes look like chats waking each other. Its prompts and questions still come, luna_wait and luna_read still reach it, and a person typing into either chat lifts this.",
                                BREAKER_WINDOW_MS / 60_000
                            ));
                        }
                        if rides_since(&how).is_some() {
                            riders.push((line, id.clone(), how, *o));
                        } else {
                            lines.push(line);
                            settled.push((id.clone(), how, *o));
                        }
                    }
                    Some((None, Settled::Hold(ended))) => {
                        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(o) = r.owed.get_mut(&(caller.clone(), id.clone())) {
                            o.held = Some((ended, now_ms()));
                        }
                    }
                    Some((None, Settled::Unhold)) => {
                        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(o) = r.owed.get_mut(&(caller.clone(), id.clone())) {
                            o.held = None;
                        }
                    }
                    Some((None, Settled::Skip(ended))) => {
                        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(o) = r.owed.get_mut(&(caller.clone(), id.clone())).filter(|now| now.rev == o.rev) {
                            o.baseline = ended;
                            o.held = None;
                        }
                    }
                    _ => {}
                }
            }
            let quota_lines = quota.remove(&caller).unwrap_or_default();
            lines.extend(quota_lines.iter().map(|n| n.line.clone()));
            let news_lines = news.remove(&caller).unwrap_or_default();
            lines.extend(news_lines.iter().cloned());
            let spawn_lines = spawns.remove(&caller).unwrap_or_default();
            lines.extend(spawn_lines.iter().map(|(_, line)| line.clone()));
            let breaches = budget.remove(&caller).unwrap_or_default();
            let mut budget_lines = Vec::new();
            if let Some(oldest) = riders.iter().filter_map(|(_, _, how, _)| rides_since(how)).chain(breaches.iter().map(|n| n.since)).min()
            {
                if riders_go(!lines.is_empty(), riders.len() == items.len(), oldest, now_ms(), crate::activity::summary().busy) {
                    for (line, id, how, o) in riders {
                        lines.push(line);
                        settled.push((id, how, o));
                    }
                    lines.extend(breaches.iter().map(|n| n.line.clone()));
                    budget_lines = breaches;
                }
            }
            if lines.is_empty() {
                reg().lock().unwrap_or_else(|e| e.into_inner()).notices_held.remove(&caller);
                company_since.remove(&caller);
                continue;
            }
            if debts_moved(&caller, &settled) {
                continue;
            }
            // Another session it is owed an answer by still at work: this notice
            // waits a few seconds for that one's (COALESCE_MS), so near-simultaneous
            // ends wake the caller once.
            let others_working = items.iter().any(|(id, ..)| {
                !settled.iter().any(|(s, ..)| s == id)
                    && crate::activity::turn_state(id).is_some_and(|t| t.turn == crate::activity::Turn::Busy)
            });
            if hold_for_company(&mut company_since, &caller, now_ms(), others_working) {
                continue;
            }
            // Typing it waits up to SEND_TIMEOUT for the turn to start, off this
            // loop: one caller stuck on that is no reason to hold back the rest.
            let Some(typing) = TypingInto::claim(&caller) else { continue };
            idle_looks.insert(caller.clone(), 0);
            let text = format!("{NOTICE_TAG} {}", lines.join(" "));
            let typed = std::thread::Builder::new().name("luna-notice".into()).spawn(move || {
                let _typing = typing;
                deliver_notice(crate::pty::manager(), &caller, &text, settled, quota_lines, spawn_lines, news_lines, budget_lines);
            });
            if let Err(e) = typed {
                crate::log::warn("agents", &format!("could not start typing a notice: {e}"));
            }
        }
    }
}

/// A debt a notice was put together on has become another since (`Owed::rev`), and
/// what it says may no longer hold: nothing of it is typed, nothing is settled, and
/// the next look starts over. A debt taken since is left to `settle_told`.
fn debts_moved(caller: &str, told: &[(String, Settled, Owed)]) -> bool {
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    told.iter().any(|(id, _, o)| r.owed.get(&(caller.to_string(), id.clone())).is_some_and(|now| now.rev != o.rev))
}

/// What the breaker counts, and all it silences: notices a turn's or a job's
/// end brings. Not a prompt or a question, which hold until they are answered
/// and cannot loop by themselves, and a busy helper may ask many.
fn counts_for_breaker(how: &Settled) -> bool {
    matches!(how, Settled::Answered | Settled::NoNews(_) | Settled::Pending(..))
}

/// Types a notice into `caller` and settles what it said (`settled`: the
/// session, what became of it, the debt as it was announced; `quota_lines`: the
/// usage windows it told of; `spawn_lines`: the spawns cut off by a core going;
/// `budget_lines`: the budgets passed).
#[allow(clippy::too_many_arguments)]
fn deliver_notice(
    pty: &crate::pty::PtyManager,
    caller: &str,
    text: &str,
    settled: Vec<(String, Settled, Owed)>,
    quota_lines: Vec<QuotaNews>,
    spawn_lines: Vec<(u64, String)>,
    news_lines: Vec<String>,
    budget_lines: Vec<BudgetNews>,
) {
    let before = crate::activity::turn_state(caller);
    let baseline = before.map(|t| t.turns_ended).unwrap_or(0);
    // The turn it starts: the next, or for a background caller that has begun
    // one since the look, the one after that.
    let background = crate::background::has(caller);
    let (typed, turn) = if background {
        // A background parent's own record says when these have arrived (`budget_news`).
        let marks = budget_lines.iter().map(|n| crate::background::BudgetMark { id: n.id.clone(), line: n.line.clone() }).collect();
        match crate::background::send_marked(caller, text, None, marks) {
            Ok(queued) => (Ok(Some(true)), baseline + 1 + u32::from(queued)),
            Err(e) => (Err(e), 0),
        }
    } else {
        (type_text(pty, caller, text, before.map(|t| t.turn), baseline), baseline + 1)
    };
    // Typed and no turn begun, with text still in the composer: its Enter went
    // astray, and it never reached the model. What it told of stays owed, as it
    // is for text held back, and the text stands as a draft: the next notice
    // waits on it as on a person's, and its reader may send it themselves.
    // Where the composer will not read, a turn begun late cannot be told from
    // text a dialog took: settled, as ever.
    let stuck = matches!(typed, Ok(Some(false))) && pty.composer(caller).is_some_and(|c| !c.text.trim().is_empty());
    let typed = if stuck {
        crate::log::warn(
            "agents",
            &format!("notice to {} sits in its composer, no turn begun: kept as a draft, still owed", label(caller)),
        );
        pty.set_draft(caller, text.chars().count() as u64);
        Ok(None)
    } else {
        typed
    };
    let about = settled
        .iter()
        .map(|(id, _, _)| label(id))
        .chain(quota_lines.iter().map(|n| format!("{} at {}%", n.key.1, n.told.level)))
        .chain(spawn_lines.iter().map(|(request, _)| format!("spawn {request}")))
        .chain(news_lines.iter().map(|_| "an auto-delete".to_string()))
        .chain(budget_lines.iter().map(|n| format!("the budget of {}", label(&n.id))))
        .collect::<Vec<_>>()
        .join(", ");
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    match typed {
        Ok(None) => {
            r.notices_held
                .insert(caller.to_string(), settled.len() + quota_lines.len() + spawn_lines.len() + news_lines.len() + budget_lines.len());
        }
        _ => {
            r.notices_held.remove(caller);
        }
    }
    drop(r);
    match typed {
        Ok(None) => {}
        Ok(Some(started)) => {
            if started {
                crate::log::info("agents", &format!("notice to {} about {about}", label(caller)));
            } else {
                crate::log::warn("agents", &format!("notice to {} about {about} did not start a turn", label(caller)));
            }
            // Its words on what never reached them went with this notice.
            let told_lost: Vec<String> = settled
                .iter()
                .filter(|(_, how, _)| matches!(how, Settled::Answered | Settled::NoNews(_) | Settled::Exited))
                .map(|(id, _, _)| id.clone())
                .collect();
            let told_left: Vec<String> =
                settled.iter().map(|(id, _, _)| id.clone()).filter(|id| parent_of(id).as_deref() == Some(caller)).collect();
            let mut stopped = Vec::new();
            let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
            for id in &told_left {
                r.worktrees_left.remove(id);
            }
            // A turn it did not start stays unmarked: at worst the caller hears
            // of a turn once too often, never misses one.
            if started {
                let told_of: Vec<String> = settled.iter().map(|(id, _, _)| id.clone()).collect();
                notice_turn(&mut r, caller, turn, &told_of);
            }
            for (id, how, told) in settled {
                if counts_for_breaker(&how) && count_notice(&mut r, caller, &id, now_ms()) {
                    stopped.push(id.clone());
                }
                settle_told(&mut r, (caller.to_string(), id), how, &told);
            }
            let told_quota = !quota_lines.is_empty();
            for n in quota_lines {
                r.quota_told.insert(n.key, n.told);
            }
            r.news.retain(|(c, line)| !(c == caller && news_lines.contains(line)));
            let told_spawns = !spawn_lines.is_empty();
            for (request, _) in spawn_lines {
                r.spawns.remove(&request);
            }
            let told_budgets = !background && !budget_lines.is_empty();
            for n in budget_lines.iter().filter(|_| !background) {
                if let Some(k) = r.known.get_mut(&n.id) {
                    k.settings["budgetTold"] = serde_json::json!(true);
                }
            }
            drop(r);
            if told_quota {
                save_quota_told();
            }
            for id in &told_lost {
                take_undelivered(id, caller, parent_of(id).as_deref() == Some(caller));
            }
            if !told_lost.is_empty() || !told_left.is_empty() || told_spawns || told_budgets {
                crate::background::save();
            }
            for id in stopped {
                breaker_tripped(caller, &id);
            }
        }
        Err(e) => crate::log::warn("agents", &format!("notice to {} about {about} failed: {e}", label(caller))),
    }
}

/// What a notice that went in does to the debt it told of (`told`). Only to
/// that debt: typing the notice takes seconds, and the turn it starts may ask
/// again before they are over, a question that is owed on; and a `wait` may
/// have taken the answer meanwhile, which leaves nothing to carry on.
pub(super) fn settle_told(r: &mut Registry, key: (String, String), how: Settled, told: &Owed) {
    if r.owed.get(&key).map(|o| o.rev) != Some(told.rev) {
        return;
    }
    match how {
        Settled::Answered | Settled::NoNews(_) | Settled::Exited => {
            r.owed.remove(&key);
        }
        Settled::Waiting => {
            if let Some(o) = r.owed.get_mut(&key) {
                o.told_waiting = true;
            }
        }
        Settled::Pending(ended, cursor) => {
            r.owed.insert(key, Owed::pending(ended, cursor));
        }
        Settled::Asked(ask) => {
            if let Some(o) = r.owed.get_mut(&key) {
                o.told_ask = ask;
            }
        }
        Settled::Resumed => {
            if let Some(o) = r.owed.get_mut(&key) {
                o.resumed = false;
            }
        }
        Settled::Idle(_) => {
            if let Some(o) = r.owed.get_mut(&key) {
                o.told_idle = true;
            }
        }
        Settled::Quiet(ended) => {
            if let Some(o) = r.owed.get_mut(&key) {
                o.told_quiet = Some(ended);
            }
        }
        Settled::Hold(_) | Settled::Unhold | Settled::Skip(_) => {}
    }
}

/// Says that the breaker has stopped a pair: in the log, and once on the
/// desktop, as nothing else will tell the person why these chats went quiet.
fn breaker_tripped(caller: &str, id: &str) {
    crate::log::warn(
        "agents",
        &format!(
            "stopped telling {} of the turns and jobs of {}: {BREAKER_NOTICES} notices within {} minutes; a person typing into either lifts this",
            label(caller),
            label(id),
            BREAKER_WINDOW_MS / 60_000
        ),
    );
    let chat = chat_name(caller).unwrap_or_else(|| caller.to_string());
    let other = chat_name(id).unwrap_or_else(|| id.to_string());
    crate::power::notify_chat(
        caller,
        "Luna stopped a notice loop",
        &format!("\"{chat}\" and \"{other}\" kept waking each other, so Luna has stopped telling the first when the second's turns end. Type into either chat to let it go on."),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::notifier::lines::{notice_line, BACKGROUND_GRACE_MS, QUIET_TURN_MS};
    use crate::agents::owed::{owe_from, retried, Owed};
    use crate::agents::registry::{reg, register};
    use crate::agents::teardown::forget;
    use crate::agents::test_support::job_over_helper;
    use crate::agents::wait::{Reserved, WaitOut};

    #[test]
    fn a_wait_out_holds_the_notice_for_a_while() {
        let none = serde_json::json!({});
        register("w-parent", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone());
        register("w-child", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("w-parent"), false, none);
        owe_from("w-parent", "w-child", 0, 0);
        let pty = crate::pty::manager();
        let key = ("w-parent".to_string(), "w-child".to_string());
        let o = || reg().lock().unwrap().owed.get(&key).copied().unwrap();
        assert!(!held_for_wait(pty, "w-parent", "w-child", &o()), "no wait out, nothing held");
        {
            let _out = WaitOut::start("w-parent", "w-child");
            // Not running, so there is news: the wait has WAIT_HOLD_MS to say it.
            assert!(held_for_wait(pty, "w-parent", "w-child", &o()));
            reg().lock().unwrap().owed.get_mut(&key).unwrap().ready_at = Some(now_ms() - WAIT_HOLD_MS - 1);
            assert!(!held_for_wait(pty, "w-parent", "w-child", &o()), "a wait that sat on news is let go");
        }
        assert!(!reg().lock().unwrap().waits.contains_key(&key), "the count goes with the call");
        {
            // A wait on several holds it however long the others take.
            let _reserved = Reserved::start("w-parent", "w-child");
            assert!(held_for_wait(pty, "w-parent", "w-child", &o()));
        }
        assert!(!held_for_wait(pty, "w-parent", "w-child", &o()), "and no longer");
    }

    /// The night of 0.8.1: a `tail -F` its own tool left running after its turn
    /// kept a helper "not done" for 14 hours, its caller told nothing. Idle is
    /// no proof of an end: told once, and the debt waits for the end itself.
    #[test]
    fn what_a_turn_left_is_named_and_an_idle_one_is_told_of_once() {
        let (known, o, path) = job_over_helper("lj-s", "lj-c", "");
        let pty = crate::pty::manager();
        let line = |o: Owed| notice_line(pty, "lj-c", "Your helper", "lj-s", &known, o);
        // Its first turn's end, with the tail just left: not done, and what runs is named.
        crate::background::left_for_test("lj-s", &[("tail.exe", 22104, 0)]);
        let mut first = Owed { rev: o.rev, ..Owed::from(2, o.cursor) };
        first.held = Some((3, now_ms() - BACKGROUND_GRACE_MS));
        let Some((Some(text), Settled::Pending(3, _))) = line(first) else { panic!("not done yet") };
        assert!(text.contains("NOT done yet: a job it started still runs in the background (tail.exe (pid 22104))"), "{text}");
        // Quiet for a while, but not for long: still a job, nothing to say.
        crate::background::left_for_test("lj-s", &[("tail.exe", 22104, IDLE_JOB_MS - 60_000)]);
        assert!(matches!(line(o), None | Some((None, _))));
        // Half an hour of nothing: said once, how to end it with it, and still owed.
        crate::background::left_for_test("lj-s", &[("tail.exe", 22104, IDLE_JOB_MS + 60_000)]);
        let Some((Some(text), how @ Settled::Idle(_))) = line(o) else { panic!("its idle job is told of") };
        assert!(rides_since(&how).is_some_and(|since| now_ms() - since < 2 * 60_000), "and rides, from when it passed the mark");
        assert!(text.contains("only idle processes left, which have done nothing for 31 min: tail.exe (pid 22104)"), "{text}");
        assert!(text.contains("luna_kill ends them") && text.contains("You will still hear when they end"), "{text}");
        settle_told(&mut reg().lock().unwrap(), ("lj-c".to_string(), "lj-s".to_string()), how, &o);
        let told = reg().lock().unwrap().owed.get(&("lj-c".to_string(), "lj-s".to_string())).copied().expect("the debt stays open");
        assert!(told.told_idle && told.told_pending);
        assert!(matches!(line(told), None | Some((None, _))), "said once");
        assert!(!job_over(&told, crate::activity::turn_state("lj-s")), "and no end for a wait either");
        // Its end: that is the news the debt waited for.
        crate::background::left_for_test("lj-s", &[]);
        let mut ended = told;
        ended.held = Some((3, now_ms() - BACKGROUND_GRACE_MS));
        let Some((Some(text), Settled::NoNews(_))) = line(ended) else { panic!("the job's end") };
        assert!(text.contains("has nothing left running now"), "{text}");
        crate::background::gone_for_test("lj-s");
        forget("lj-s");
        let _ = std::fs::remove_file(&path);
    }

    /// Two helpers finishing seconds apart: the first's notice waits for the
    /// second's, up to COALESCE_MS, and goes alone past that or once nothing
    /// else of the caller's is at work.
    #[test]
    fn a_notice_waits_a_little_for_a_helper_about_to_finish() {
        let ready = 1_000_000;
        let at = |s: u64| ready + s * 1000;
        assert!(coalesce_holds(ready, at(0), true), "another helper of its mid-turn: it waits");
        assert!(coalesce_holds(ready, at(19), true));
        assert!(!coalesce_holds(ready, at(20), true), "never past COALESCE_MS");
        assert!(!coalesce_holds(ready, at(0), false), "nothing else at work: it goes at once");
        // A clock that went back holds no longer than it would have.
        assert!(coalesce_holds(ready, ready - 5_000, true));
        // The look loop's own bookkeeping: the first look a line is ready starts
        // the wait, a later one keeps it, delivery or nothing ready clears it.
        let mut since: HashMap<String, u64> = HashMap::new();
        assert!(hold_for_company(&mut since, "c", at(0), true));
        assert!(hold_for_company(&mut since, "c", at(10), true), "{since:?}");
        assert_eq!(since.get("c"), Some(&at(0)), "the wait counts from the first look");
        assert!(!hold_for_company(&mut since, "c", at(21), true));
        assert!(!since.contains_key("c"), "going out clears it");
        assert!(!hold_for_company(&mut since, "d", at(0), false));
        assert!(!since.contains_key("d"));
    }

    /// A job's end with no word from it, or an idle job, is told of with the
    /// next notice; on its own only once nothing else can bring one.
    #[test]
    fn a_line_with_no_news_rides_with_the_next_notice() {
        let now = 100 * RIDE_MAX_MS;
        let fresh = now - 30_000;
        assert!(!riders_go(false, false, fresh, now, 1), "another helper at work: it waits for that one's notice");
        assert!(riders_go(true, false, fresh, now, 1), "a notice is going in anyway");
        assert!(riders_go(false, true, fresh, now, 1), "nothing else owed: this is the last it will hear");
        assert!(!riders_go(false, false, now - RIDE_MAX_MS + 1, now, 1));
        assert!(riders_go(false, false, now - RIDE_MAX_MS, now, 1), "it may be waiting on this very job");
        // An open debt that can bring nothing (a stopped pair, a chat at rest), and
        // all is idle: nothing is coming.
        assert!(riders_go(false, false, fresh, now, 0));
        // The line settles a debt as it always did; only going out does.
        let key = ("rd-c".to_string(), "rd-s".to_string());
        let o = Owed::pending(3, 0);
        reg().lock().unwrap().owed.insert(key.clone(), o);
        assert!(counts_for_breaker(&Settled::NoNews(1)) && !counts_for_breaker(&Settled::Idle(1)));
        settle_told(&mut reg().lock().unwrap(), key.clone(), Settled::Idle(1), &o);
        assert!(reg().lock().unwrap().owed.get(&key).is_some_and(|o| o.told_idle), "owed on");
        settle_told(&mut reg().lock().unwrap(), key.clone(), Settled::NoNews(1), &o);
        assert!(!reg().lock().unwrap().owed.contains_key(&key), "settled");
    }

    /// 2 November: a helper's pwsh hung for 25 minutes mid-turn on a process a
    /// failed test had left, and nothing told its caller, who had ended its turn
    /// on "the notice comes when it is done". A quiet turn is told of once.
    #[test]
    fn a_turn_gone_quiet_is_told_of_once() {
        let none = serde_json::json!({});
        register("qt-s", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("qt-c"), false, none);
        crate::background::idle_for_test("qt-s", Provider::Claude, 3, false);
        let o = Owed::from(3, 0);
        reg().lock().unwrap().owed.insert(("qt-c".to_string(), "qt-s".to_string()), o);
        let known = reg().lock().unwrap().known.get("qt-s").cloned().unwrap();
        let pty = crate::pty::manager();
        let line = |o: Owed| notice_line(pty, "qt-c", "Your helper", "qt-s", &known, o);
        // At work and writing: nothing to say.
        crate::background::quiet_for_test("qt-s", QUIET_TURN_MS - 60_000);
        assert!(matches!(line(o), None | Some((None, _))));
        // Half an hour of nothing: said once for this turn, and still owed.
        crate::background::quiet_for_test("qt-s", QUIET_TURN_MS + 60_000);
        let Some((Some(text), Settled::Quiet(3))) = line(o) else { panic!("a quiet turn is news") };
        assert!(text.contains("mid-turn and has written nothing for 31 min") && text.contains("luna_kill stops the turn"), "{text}");
        settle_told(&mut reg().lock().unwrap(), ("qt-c".to_string(), "qt-s".to_string()), Settled::Quiet(3), &o);
        let told = reg().lock().unwrap().owed.get(&("qt-c".to_string(), "qt-s".to_string())).copied().expect("still owed");
        assert_eq!(told.told_quiet, Some(3));
        assert!(matches!(line(told), None | Some((None, _))), "once per turn");
        assert!(!counts_for_breaker(&Settled::Quiet(3)), "no turn's end");
        crate::background::gone_for_test("qt-s");
        forget("qt-s");
    }

    #[test]
    fn a_jobs_end_goes_to_a_wait_that_is_out_and_not_to_a_notice_too() {
        let (known, o, path) = job_over_helper("jw-s", "jw-c", "");
        let key = ("jw-c".to_string(), "jw-s".to_string());
        let pty = crate::pty::manager();
        let debt = || reg().lock().unwrap().owed.get(&key).copied().unwrap();
        let out = WaitOut::start("jw-c", "jw-s");
        // The one clock: whoever looks first starts it, the other reads it.
        assert!(held_for_wait(pty, "jw-c", "jw-s", &o), "the wait has it reserved");
        let started = debt().held.expect("the hold starts the clock");
        assert!(!job_end_stood(&key, 3, true), "the wait goes by the same clock");
        assert_eq!(debt().held, Some(started), "and does not start its own");
        assert_eq!(debt().ready_at, None, "not news yet");
        // Past where a notice used to give up on the wait, still reserved.
        reg().lock().unwrap().owed.get_mut(&key).unwrap().held = Some((3, now_ms() - BACKGROUND_GRACE_MS + 5_000));
        assert!(held_for_wait(pty, "jw-c", "jw-s", &debt()), "through the whole grace");
        assert!(!job_end_stood(&key, 3, true));
        // The grace is over: the wait hands it over now, with its hold to do it in.
        reg().lock().unwrap().owed.get_mut(&key).unwrap().held = Some((3, now_ms() - BACKGROUND_GRACE_MS));
        assert!(job_end_stood(&key, 3, true), "the wait's moment");
        assert!(held_for_wait(pty, "jw-c", "jw-s", &debt()), "and the notice still holds for it");
        assert!(debt().ready_at.is_some(), "from here on it is WAIT_HOLD_MS");
        // A wait that sat on it past that has been let go: the notice is free.
        reg().lock().unwrap().owed.get_mut(&key).unwrap().ready_at = Some(now_ms() - WAIT_HOLD_MS - 1);
        assert!(!held_for_wait(pty, "jw-c", "jw-s", &debt()));
        // With the wait gone the notice goes by the clock the wait left running.
        drop(out);
        let stood = debt();
        assert!(!held_for_wait(pty, "jw-c", "jw-s", &stood));
        assert!(matches!(notice_line(pty, "jw-c", "Your helper", "jw-s", &known, stood), Some((Some(_), Settled::NoNews(_)))));
        // A job back at work starts it over.
        assert!(!job_end_stood(&key, 3, false));
        assert_eq!(debt().held, None);
        assert!(!job_end_stood(&key, 3, true), "from nothing");
        // A notice settled it first: the wait is not left counting for ever.
        reg().lock().unwrap().owed.remove(&key);
        assert!(job_end_stood(&key, 3, true));
        crate::background::gone_for_test("jw-s");
        forget("jw-s");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_breaker_counts_turn_ends_and_not_prompts() {
        assert!(counts_for_breaker(&Settled::Answered));
        assert!(counts_for_breaker(&Settled::Pending(1, 0)));
        assert!(!counts_for_breaker(&Settled::Waiting), "a busy helper asks often, and holds until answered");
        assert!(!counts_for_breaker(&Settled::Asked(1)));
        assert!(!counts_for_breaker(&Settled::Exited));
    }

    #[test]
    fn a_budget_line_rides_like_any_line_with_no_news_in_it() {
        let now = 1_000_000;
        let fresh = now - 1000;
        assert!(!riders_go(false, false, fresh, now, 1), "owed another helper's answer, with work going on: it waits for that");
        assert!(riders_go(false, true, fresh, now, 1), "owed nothing else");
        assert!(riders_go(true, false, fresh, now, 1), "a notice is going in anyway");
        assert!(riders_go(false, false, now - RIDE_MAX_MS, now, 1), "ten minutes at most");
    }

    /// A look taken before a retry reads the turn that missed as the answer.
    #[test]
    fn a_notice_looked_up_before_a_retry_neither_goes_out_nor_settles() {
        let (a, id) = ("rn-sender", "rn-helper");
        let none = serde_json::json!({});
        register(a, Provider::Claude, "C:\\p", "C:\\acc\\one", None, true, none.clone());
        register(id, Provider::Claude, "C:\\p", "C:\\acc\\one", Some(a), false, none);
        crate::background::idle_for_test(id, Provider::Claude, 1, false);
        owe_from(a, id, 0, 0);
        let key = (a.to_string(), id.to_string());
        let (looked_up, k) = {
            let r = reg().lock().unwrap();
            (r.owed.get(&key).copied().unwrap(), r.known.get(id).cloned().unwrap())
        };
        retried(id, &[a.to_string()], 1);
        let told = [(id.to_string(), Settled::Answered, looked_up)];
        assert!(debts_moved(a, &told), "caught before it is typed");
        assert!(notice_line(crate::pty::manager(), a, "Your helper", id, &k, looked_up).is_none(), "and before it is put together");
        settle_told(&mut reg().lock().unwrap(), key.clone(), Settled::Answered, &looked_up);
        assert_eq!(reg().lock().unwrap().owed.get(&key).map(|o| o.baseline), Some(1), "still owed, past the turn that missed");
        crate::background::gone_for_test(id);
        forget(id);
        forget(a);
    }
}
