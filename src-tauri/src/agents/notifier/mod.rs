//! The notifier: a loop that types a one-line notice into an idle caller once
//! what it is owed has come (an answer, a prompt, an exit), and the luna_notification_send
//! tool's desktop notification.

pub(super) mod lines;
pub(super) mod system;
pub(super) mod usage;

use crate::agents::ask::live;
use crate::agents::delivery::{puts_draft_aside, still_starting, type_text, TypingInto};
use crate::agents::lost::take_undelivered_ids;
use crate::agents::notifier::lines::{
    count_notice, is_wake, last_notice, notice_turn, pair_stopped, render_notice, BACKGROUND_GRACE_MS, BREAKER_NOTICES, BREAKER_WINDOW_MS,
};
use crate::agents::notifier::system::SystemNews;
use crate::agents::notifier::usage::{quota_file, quota_from, quota_news, save_quota_told, your_helper, QuotaNews};
use crate::agents::owed::Owed;
use crate::agents::persist::{core_up_ms, SESSION_RETURN_MS};
use crate::agents::registry::{chat_name, is_dev, is_persons_chat, label, reg, waits_on_helpers, Known, Registry};
use crate::agents::said::{Request, Status};
use crate::agents::spawn::settle::{settle_spawns, spawn_news};
use crate::agents::spawn::PlacesRefused;
use crate::agents::teardown::{reap_with_losses, unread_until, UNREAD_ANSWER_MS};
use crate::provider::Provider;
use crate::throttle::now_ms;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How often the notifier looks for answers nobody collected.
pub(super) const NOTIFY_EVERY: Duration = Duration::from_secs(2);

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
#[derive(Clone)]
pub(super) enum Settled {
    /// A turn past the one asked about ended.
    Answered,
    /// A terminal failure or loss.
    Failed,
    FailedRequests(u32, Vec<u32>),
    /// A turn ended, but the session left a job running in the background;
    /// owed on from this turn count and transcript end, for that job's end or
    /// a turn of its own. Keep the last prompt match across that cursor.
    Pending(u32, u64, Option<u32>, Option<Status>, bool),
    Progress(Progress),
    UnusedTurn(u32, u64),
    PlanFyi(u32, Vec<Request>, bool),
    /// A turn ended but is not settled — processes still at work, or no
    /// reply in the transcript yet: nothing to say, start the grace from now.
    Hold(u32),
    /// A job looked over and is at work again: the grace starts over.
    Unhold,
    /// An interim or notice-driven turn. Advance only the notifier's count.
    /// Keep this turn's words for the caller, and its end for the lead wake.
    Skip(u32, u64, bool),
    Waiting,
    /// It asks a question it does not wait on; told once per question.
    Asked(u64),
    /// A core's going stopped its turn and it went on by itself; told once.
    Resumed,
    /// A quiet turn or its background wait. Only the task's first stall wakes.
    Quiet(u32),
    WaitsOnBackground(u32),
    QuietFor(u32, Option<u32>, bool),
    /// The answer is settled quietly, with the cursor retained for reading.
    Silent(u32, bool),
    Exited,
}

#[derive(Clone)]
pub(super) struct Progress {
    ended: u32,
    cursor: u64,
    matched: Option<u32>,
    requests: Vec<Request>,
    plan: bool,
    work_left: bool,
    job: bool,
    pending: bool,
}

/// Whether a notice may go into `caller` now: its CLI is up and between turns
/// and nobody is typing into it. A draft in its composer is `type_text`'s to
/// put aside, or to keep the notice back for.
pub(in crate::agents) fn ready_for_notice(pty: &crate::pty::PtyManager, caller: &str) -> bool {
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

/// Whether a caller told a session is not done yet may have that session's job
/// to hear of: between turns, with nothing of its own left running. For the
/// holds that wait on news; helpers still to answer it keep `notice_line` from
/// saying so.
fn job_over(o: &Owed, ts: Option<crate::activity::TurnState>) -> bool {
    (o.told_pending || o.wake.skipped.is_some()) && ts.is_some_and(|t| t.turn == crate::activity::Turn::Idle && !t.children_busy)
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
    if pair_stopped(pty, caller, id) {
        return true;
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
        if (owed.told_pending || owed.wake.skipped.is_some()) && !job_ended {
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

fn prune_owed(
    owed: &mut HashMap<(String, String), Owed>,
    known: &HashMap<String, Known>,
    stopped: &HashMap<(String, String), u64>,
    up_ms: u64,
    now: u64,
    caller_in_scope: impl Fn(&str) -> bool,
) -> Vec<(String, String)> {
    let mut unreachable = Vec::new();
    owed.retain(|key, o| {
        let (caller, id) = key;
        if !caller_in_scope(caller) || stopped.contains_key(key) {
            return true;
        }
        if !known.contains_key(id) {
            return false;
        }
        if let Some(until) = o.wake.delivery_expired {
            return now < until;
        }
        if owed_stands(known, caller, id, up_ms) {
            return true;
        }
        let active = o.active();
        if active {
            unreachable.push((caller.clone(), id.clone()));
        }
        o.wake.delivery_expired = unread_until(o).filter(|until| now < *until);
        if o.wake.delivery_expired.is_none() && active && o.wake.unread_since.is_none() {
            // An idle turn can be visible before its last transcript line.
            // Keep the cursor available to the transcript observer.
            o.wake.delivery_expired = Some(now.saturating_add(UNREAD_ANSWER_MS));
            o.wake.reconciling = true;
        }
        o.wake.delivery_expired.is_some()
    });
    unreachable
}

/// Refusals normally ride. The top chat must also hear when it is the only
/// one that can free places, even if another helper keeps working for hours.
const RIDE_MAX_MS: u64 = 10 * 60 * 1000;

fn riders_go(company: bool, otherwise_owed: bool, refusal_age: Option<u64>) -> bool {
    company || refusal_age.is_some_and(|age| !otherwise_owed || age >= RIDE_MAX_MS)
}

fn can_take_refusal(pty: &crate::pty::PtyManager, caller: &str) -> bool {
    // Power closes its own admission gate for the final recheck. Eligibility
    // must still reflect whether the chat could take the line with it open.
    ready_for_notice(pty, caller)
        && (!pty.has_draft(caller)
            || (puts_draft_aside(pty.provider(caller)) && pty.composer(caller).is_some_and(|c| !c.text.is_empty() && !c.stashed)))
}

pub(crate) fn places_pending(sessions: &[crate::activity::SessionLife]) -> bool {
    let pending: Vec<_> = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        sessions
            .iter()
            .filter_map(|s| {
                let p = r.places_refused.get(&s.id)?;
                refusal_has_chat(&r, &s.id, p).then_some((s.id.as_str(), p.since.elapsed() < Duration::from_millis(RIDE_MAX_MS)))
            })
            .collect()
    };
    // A claim can be visible before the delivery thread enters power
    // admission. In that interval the refusal is still pending.
    pending.into_iter().any(|(caller, fresh)| fresh && can_take_refusal(crate::pty::manager(), caller) || notice_waits(caller))
}

fn refusal_has_chat(r: &Registry, caller: &str, refusal: &PlacesRefused) -> bool {
    r.known.get(caller).is_some_and(|k| k.is_persons_chat() && !k.waiting)
        && refusal.who.iter().any(|id| super::registry::top_chat(r, id).as_deref() == Some(caller))
}

fn rides(how: &Settled, owed: Owed) -> bool {
    let target = match how {
        Settled::Quiet(_) | Settled::WaitsOnBackground(_) => owed.quiet_target(),
        Settled::QuietFor(_, target, _) => *target,
        _ => return false,
    };
    target.is_none() || owed.wake.quiet_task == target
}

/// The longest a ready notice waits to batch with another session's answer.
pub(super) const COALESCE_MS: u64 = 20_000;

/// Whether a notice first ready at `first_ready` waits on at `now`: only while
/// something else the caller is owed is at work (`others_working`), and never
/// past COALESCE_MS. On the monotonic clock: a wall clock set back would hold
/// it longer.
pub(super) fn coalesce_holds(first_ready: Instant, now: Instant, others_working: bool) -> bool {
    others_working && now.saturating_duration_since(first_ready) < Duration::from_millis(COALESCE_MS)
}

/// Whether the notice put together for `caller` (its lines settling `settled`,
/// of the debts to `owed_by`) waits for company at `now`, keeping when it was
/// first ready in `since`; one that goes out takes its entry with it. Never one
/// that carries a question or a permission prompt: a helper stopped on it waits
/// on the caller, and so may the person behind a question.
fn hold_for_company(
    since: &mut HashMap<String, Instant>,
    caller: &str,
    now: Instant,
    settled: &[(String, Settled, Owed)],
    owed_by: &[&str],
    working: impl Fn(&str) -> bool,
) -> bool {
    let asks = settled.iter().any(|(_, how, _)| matches!(how, Settled::Waiting | Settled::Asked(_)));
    let others_working = owed_by.iter().any(|id| !settled.iter().any(|(s, ..)| s == id) && working(id));
    let first = *since.entry(caller.to_string()).or_insert(now);
    let holds = !asks && coalesce_holds(first, now, others_working);
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
    // When each caller's notice was first ready while another of its helpers
    // worked on (`hold_for_company`).
    let mut company_since: HashMap<String, Instant> = HashMap::new();
    loop {
        std::thread::sleep(NOTIFY_EVERY);
        let pty = crate::pty::manager();
        notify_once(
            pty,
            &mut idle_looks,
            &mut company_since,
            |_| true,
            |typing, caller, notice| {
                let typed = std::thread::Builder::new().name("luna-notice".into()).spawn(move || {
                    let _typing = typing;
                    deliver_notice(crate::pty::manager(), &caller, notice);
                });
                if let Err(e) = typed {
                    crate::log::warn("agents", &format!("could not start typing a notice: {e}"));
                }
            },
        );
    }
}

#[cfg(test)]
pub(super) fn tick_for_test(caller: &str) {
    let pty = crate::pty::manager();
    notify_once(
        pty,
        &mut HashMap::from([(caller.into(), NOTIFY_IDLE_LOOKS - 1)]),
        &mut HashMap::new(),
        |id| id == caller,
        |_, caller, notice| deliver_notice(pty, &caller, notice),
    );
}

#[cfg(test)]
pub(super) fn claim_refusal_for_test(caller: &str) -> TypingInto {
    let mut claim = None;
    notify_once(
        crate::pty::manager(),
        &mut HashMap::from([(caller.into(), NOTIFY_IDLE_LOOKS - 1)]),
        &mut HashMap::new(),
        |id| id == caller,
        |typing, _, notice| {
            assert!(notice.text.contains("No place was left"));
            claim = Some(typing);
        },
    );
    claim.expect("the notifier claimed the refusal before typing it")
}

fn notify_once(
    pty: &crate::pty::PtyManager,
    idle_looks: &mut HashMap<String, u32>,
    company_since: &mut HashMap<String, Instant>,
    caller_in_scope: impl Fn(&str) -> bool,
    deliver: impl FnMut(TypingInto, String, Notice),
) {
    notify_once_since_start(pty, idle_looks, company_since, core_up_ms(), caller_in_scope, deliver);
}

fn notify_once_since_start(
    pty: &crate::pty::PtyManager,
    idle_looks: &mut HashMap<String, u32>,
    company_since: &mut HashMap<String, Instant>,
    up_ms: u64,
    caller_in_scope: impl Fn(&str) -> bool,
    mut deliver: impl FnMut(TypingInto, String, Notice),
) {
    observe_unread(pty, &caller_in_scope);
    let loss_recipients = crate::background::loss_recipients();
    reap_with_losses(pty, &loss_recipients);
    // Owed by caller, each session with what is known of it; anything a
    // delete has made unknown is dropped on the way, and a caller yet to
    // come back has nothing to be told in.
    let mut by_caller: std::collections::BTreeMap<String, Vec<(String, Known, Owed, String)>> = Default::default();
    let mut loss_helpers: HashMap<String, Vec<(String, bool, String)>> = HashMap::new();
    let mut gave_up: Vec<(String, String)> = Vec::new();
    {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let Registry { owed, known, names, stopped, .. } = &mut *r;
        gave_up.extend(prune_owed(owed, known, stopped, up_ms, now_ms(), &caller_in_scope));
        for ((caller, id), o) in owed.iter() {
            if !caller_in_scope(caller) {
                continue;
            }
            let Some(k) = known.get(id) else { continue };
            if o.wake.delivery_expired.is_some()
                || (!o.active() && !o.watches_later_turns() && o.upto == 0)
                || known.get(caller).is_none_or(|k| k.waiting)
            {
                continue;
            }
            let who = notice_subject(id, k.parent.as_deref() == Some(caller), names.get(id));
            by_caller.entry(caller.clone()).or_default().push((id.clone(), k.clone(), *o, who));
        }
        for (caller, id) in loss_recipients {
            let Some(k) = known.get(&id) else { continue };
            if known.get(&caller).is_none_or(|k| k.waiting)
                || owed.get(&(caller.clone(), id.clone())).is_some_and(|o| o.wake.delivery_expired.is_some())
            {
                continue;
            }
            let parent = k.parent.as_deref() == Some(&caller);
            let who = notice_subject(&id, parent, names.get(&id));
            by_caller.entry(caller.clone()).or_default();
            loss_helpers.entry(caller).or_default().push((id, parent, who));
        }
        // A lead's stdin kept open past its answer for notices goes once it
        // has none to come (background/events.rs `keeps_listening`). Under
        // this lock, which goes before the background one: a debt made
        // meanwhile would not be in what it goes by.
        let owed_to =
            owed.iter().filter(|(_, o)| o.active()).map(|((caller, _), _)| caller.clone()).chain(loss_helpers.keys().cloned()).collect();
        crate::background::listen_only_while_owed(&owed_to, &caller_in_scope);
    }
    if !gave_up.is_empty() {
        crate::background::save();
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
    settle_spawns();
    let mut news: HashMap<String, Vec<String>> = HashMap::new();
    {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let Registry { news: queued, known, woken, .. } = &mut *r;
        // A caller a core restart has yet to bring back gets the time any
        // caller gets (CALLER_RETURN_MS): these lines are said once, and kept
        // across a core for that (`SavedChats::news`).
        // The dev caller has no terminal to be told in: the log has its lines,
        // as it has its spawns' (`spawn_news`).
        queued.retain(|(caller, line)| {
            if !caller_in_scope(caller) {
                return true;
            }
            if is_dev(caller) {
                crate::log::warn("agents", &format!("{caller} is not there to be told: {line}"));
            }
            let expired_wake = is_wake(line) && woken.get(caller).is_some_and(|w| w.expired(now_ms()));
            !expired_wake && !is_dev(caller) && (known.contains_key(caller) || up_ms < CALLER_RETURN_MS)
        });
        for (caller, line) in queued.iter().filter(|(caller, _)| known.get(caller).is_some_and(|k| !k.waiting)) {
            if !caller_in_scope(caller) {
                continue;
            }
            news.entry(caller.clone()).or_default().push(line.clone());
        }
    }
    for caller in news.keys() {
        by_caller.entry(caller.clone()).or_default();
    }
    let mut system_news = system::news(pty);
    for caller in system_news.keys() {
        by_caller.entry(caller.clone()).or_default();
    }
    let mut spawns = spawn_news(up_ms);
    for caller in spawns.keys() {
        by_caller.entry(caller.clone()).or_default();
    }
    // Sessions below a chat refused a place on its line: a line that rides
    // (`riders_go`), as only that chat can free the places.
    let mut places: HashMap<String, PlacesRefused> = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        r.places_refused.iter().filter(|(caller, _)| caller_in_scope(caller)).map(|(caller, p)| (caller.clone(), p.clone())).collect()
    };
    let expired: Vec<_> = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        places.iter().filter(|(caller, refusal)| !refusal_has_chat(&r, caller, refusal)).map(|(caller, _)| caller.clone()).collect()
    };
    let expired: Vec<_> = expired.into_iter().chain(places.keys().filter(|caller| !live(pty, caller)).cloned()).collect();
    if !expired.is_empty() {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        for caller in expired {
            if let Some(expired) = places.remove(&caller) {
                r.places_refused.retain(|id, current| *id != caller || current.since != expired.since);
            }
        }
    }
    for caller in places.keys() {
        by_caller.entry(caller.clone()).or_default();
    }
    idle_looks.retain(|c, _| by_caller.contains_key(c));
    company_since.retain(|c, _| by_caller.contains_key(c));
    reg().lock().unwrap_or_else(|e| e.into_inner()).notices_held.retain(|c, _| !caller_in_scope(c) || by_caller.contains_key(c));
    let mut deliveries = Vec::new();
    let mut work = lines::WorkLook::default();
    for (caller, items) in by_caller.into_iter().filter(|(caller, _)| caller_in_scope(caller)) {
        if notice_waits(&caller) {
            observe_without_delivery(pty, &caller, &items, &mut work);
            idle_looks.insert(caller, 0);
            continue;
        }
        let looks = idle_looks.entry(caller.clone()).or_default();
        // Or a lead kept up by its own background tasks, which takes the
        // notice on stdin then (background/events.rs `listening`).
        let ready = ready_for_notice(pty, &caller) || crate::background::takes_notices(&caller);
        *looks = if ready { *looks + 1 } else { 0 };
        if *looks < NOTIFY_IDLE_LOOKS {
            observe_without_delivery(pty, &caller, &items, &mut work);
            continue;
        }
        let mut lines = Vec::new();
        let mut settled = Vec::new();
        let mut riders = Vec::new();
        let mut evidence = HashMap::new();
        let mut waiting_on = std::collections::HashSet::new();
        for (id, k, o, who) in &items {
            if held_for_wait(pty, &caller, id, o) {
                waiting_on.insert(id.as_str());
                continue;
            }
            let said = render_notice(pty, &caller, who, id, k, *o, &mut work);
            #[cfg(test)]
            super::test_support::reached("notice rendered", &caller);
            match said {
                Some((Some(mut line), how, read)) => {
                    if let Some(suffix) = breaker_suffix(&caller, id, &how, now_ms()) {
                        line.push_str(suffix);
                    }
                    if rides(&how, *o) {
                        riders.push((line, id.clone(), how, *o, read));
                    } else {
                        lines.push(line);
                        settled.push((id.clone(), how, *o));
                        evidence.insert(id.clone(), read);
                    }
                }
                Some((None, how, read)) => settle_control(&caller, id, o, how, read.read_to),
                _ => {}
            }
        }
        let quota_lines = quota.remove(&caller).unwrap_or_default();
        lines.extend(quota_lines.iter().map(|n| n.line.clone()));
        let mut news_lines: Vec<String> = news.remove(&caller).unwrap_or_default();
        lines.extend(news_lines.iter().cloned());
        let system_lines = system_news.remove(&caller).unwrap_or_default();
        lines.extend(system_lines.iter().map(|(_, line)| line.clone()));
        let system_news = system_lines.into_iter().map(|(entry, _)| entry).collect();
        let spawn_lines = spawns.remove(&caller).unwrap_or_default();
        lines.extend(spawn_lines.iter().map(|(_, line)| line.clone()));
        let refused = places.remove(&caller).filter(|_| can_take_refusal(pty, &caller));
        let otherwise_owed = refused.is_some()
            && reg().lock().unwrap_or_else(|e| e.into_inner()).owed.iter().any(|((to, _), o)| *to == caller && o.active());
        let refusal_age = refused.as_ref().map(|p| p.since.elapsed().as_millis() as u64);
        if riders_go(!lines.is_empty(), otherwise_owed, refusal_age) {
            if let Some(line) = refused.map(|p| p.line()) {
                lines.push(line.clone());
                news_lines.push(line);
            }
            for (line, id, how, o, read) in riders {
                lines.push(line);
                evidence.insert(id.clone(), read);
                settled.push((id, how, o));
            }
        }
        let mut losses = Vec::new();
        for (id, parent, who) in loss_helpers.remove(&caller).unwrap_or_default() {
            if waiting_on.contains(id.as_str()) || breaker_blocks(pty, &caller, &id, &Settled::Failed) {
                continue;
            }
            let read = evidence.get(&id);
            let (stopped, turns) = if read.is_none_or(|read| read.stopped.is_empty()) {
                lines::background_stopped_line(&id, &caller).unwrap_or_default()
            } else {
                Default::default()
            };
            let (ids, texts): (Vec<_>, Vec<_>) = if read.is_none_or(|read| read.lost.is_empty()) {
                crate::background::undelivered_snapshot(&id, &caller, parent).into_iter().unzip()
            } else {
                (Vec::new(), Vec::new())
            };
            if !ids.is_empty() || !turns.is_empty() {
                let file = (!texts.is_empty()).then(|| super::said::note_file(&id, &super::lost::undelivered_text(&texts))).flatten();
                let lost = super::lost::undelivered_line(&texts, false, file.as_deref());
                let suffix = if settled.iter().any(|(other, how, _)| other == &id && counts_for_breaker(how)) {
                    ""
                } else {
                    breaker_suffix(&caller, &id, &Settled::Failed, now_ms()).unwrap_or_default()
                };
                lines.push(format!("{who}:{stopped}{lost}{suffix}"));
                // The fallback sends the caller to the queue to read the text.
                let ids = if file.is_some() { ids } else { Vec::new() };
                let mut requests = super::owed::requests_of(&caller, &id);
                requests.retain(|q| q.delivery.as_ref().is_some_and(|qid| ids.contains(qid)));
                losses.push((id, LineEvidence { lost: ids, parent, stopped: turns, requests, ..LineEvidence::default() }));
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
        // The start gate closed (an update's stop, a power-off being
        // decided): what it would say stays owed, for once it opens
        // (`crate::starts`). Asked again where it goes in (delivery.rs).
        if !crate::starts::open(&caller) {
            continue;
        }
        // Another session it is owed an answer by still at work: this notice
        // waits a few seconds for that one's (COALESCE_MS), so near-simultaneous
        // ends wake the caller once. Not a question or a prompt.
        let owed_by: Vec<&str> = items.iter().filter(|(_, _, o, _)| o.active()).map(|(id, ..)| id.as_str()).collect();
        let busy = |id: &str| crate::activity::turn_state(id).is_some_and(|t| t.turn == crate::activity::Turn::Busy);
        let now = Instant::now();
        // Taken before the hold looks, which lets go of it once the notice goes.
        let held_for = company_since.get(&caller).map(|first| now.saturating_duration_since(*first));
        if refusal_age.is_some_and(|age| age >= RIDE_MAX_MS) {
            company_since.remove(&caller);
        } else if hold_for_company(company_since, &caller, now, &settled, &owed_by, busy) {
            continue;
        }
        // Typing it waits up to SEND_TIMEOUT for the turn to start, off this
        // loop: one caller stuck on that is no reason to hold back the rest.
        let Some(typing) = TypingInto::claim(&caller) else { continue };
        idle_looks.insert(caller.clone(), 0);
        let text = format!("{NOTICE_TAG} {}", lines.join(" "));
        let notice = Notice { text, settled, quota_lines, spawn_lines, news_lines, system_news, held_for, evidence, losses };
        deliveries.push((typing, caller, notice));
    }
    // Finish the look before any of its replies can settle. Waking the first
    // caller in an idle cycle removes an edge the other callers still need
    // to see, or their wakes would depend on the order of this loop.
    for (typing, caller, notice) in deliveries {
        deliver(typing, caller, notice);
    }
}

fn notice_subject(id: &str, parent: bool, name: Option<&String>) -> String {
    let name = name.map(|n| n.trim()).filter(|n| !n.is_empty());
    match (parent, name) {
        (true, _) => your_helper(id, name),
        (false, Some(n)) => format!("Session \"{n}\" ({id}), which you wrote to,"),
        (false, None) => format!("Session {id}, which you wrote to,"),
    }
}

/// Observe answers before caller readiness and expiry can hide their debt.
fn observe_unread(pty: &crate::pty::PtyManager, caller_in_scope: impl Fn(&str) -> bool) {
    let now = now_ms();
    let debts: Vec<_> = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        r.owed
            .iter()
            .filter_map(|((caller, id), o)| {
                let k = r.known.get(id)?;
                let awaiting_unread = o.wake.delivery_expired.is_some_and(|until| now < until)
                    && o.wake.silent.is_none()
                    && o.wake.unread_since.is_none()
                    && o.wake.unread_to.is_none();
                let checks_growth = o.late_transcript_cursor().is_some();
                (caller_in_scope(caller) && (o.active() || o.watches_later_turns() || awaiting_unread || checks_growth))
                    .then(|| ((caller.clone(), id.clone()), *o, k.provider))
            })
            .collect()
    };
    let mut changed = false;
    for (key, o, provider) in debts {
        let id = &key.1;
        if pair_stopped(pty, &key.0, id) {
            continue;
        }
        let Some(turn) = crate::activity::turn_state(id) else { continue };
        if turn.turn == crate::activity::Turn::Busy {
            continue;
        }
        let late_status = (turn.turn == crate::activity::Turn::Idle)
            .then(|| o.late_transcript_cursor())
            .flatten()
            .filter(|(checked, _)| *checked <= turn.turns_ended)
            .and_then(|(checked, from)| lines::status_after(pty, id, provider, from, checked < turn.turns_ended));
        let end = if let Some(end) = late_status {
            end
        } else {
            if turn.turns_ended <= o.with_late_transcript(turn.turns_ended, || super::read::transcript_end(id)).notice_baseline() {
                continue;
            }
            let from = o.wake.unread_to.unwrap_or_else(|| o.notice_cursor()).max(o.cursor);
            let (words, end) = crate::agents::said::written_range(pty, id, provider, from);
            if words.is_empty() {
                continue;
            }
            end
        };
        if crate::activity::turn_state(id).is_none_or(|now| now.turn != turn.turn || now.turns_ended != turn.turns_ended) {
            continue;
        }
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        if r.stopped.contains_key(&key) {
            continue;
        }
        if let Some(current) = r.owed.get_mut(&key).filter(|now| {
            now.rev == o.rev
                && now.transcript_rev == o.transcript_rev
                && now.cursor == o.cursor
                && now.wake.unread_to == o.wake.unread_to
                && now.wake.unread_since == o.wake.unread_since
                && now.wake.late_check == o.wake.late_check
                && now.wake.reported_to == o.wake.reported_to
        }) {
            if late_status.is_some() {
                current.wake.silent = None;
                current.wake.skipped = None;
                current.wake.late_check = None;
                current.wake.late_status_to = late_status;
            }
            current.wake.unread_since.get_or_insert_with(now_ms);
            current.wake.unread_to = current.wake.unread_to.max(Some(end));
            current.wake.reconciling = false;
            if current.wake.delivery_expired.is_some() {
                current.wake.delivery_expired = unread_until(current);
            }
            changed = true;
        }
    }
    if changed {
        crate::background::save();
    }
}

/// Watch delegate completions and later plan turns even while delivery waits.
fn observe_without_delivery(
    pty: &crate::pty::PtyManager,
    caller: &str,
    items: &[(String, Known, Owed, String)],
    work: &mut lines::WorkLook,
) {
    for (id, known, owed, who) in items {
        if (owed.wake.plan_through.is_some() || (owed.active() && lines::is_lead(known))) && !pair_stopped(pty, caller, id) {
            if let Some((None, how @ (Settled::Hold(_) | Settled::Unhold | Settled::Skip(..) | Settled::PlanFyi(..)), read)) =
                render_notice(pty, caller, who, id, known, *owed, work)
            {
                settle_control(caller, id, owed, how, read.read_to);
            }
        }
    }
}

fn settle_control(caller: &str, id: &str, o: &Owed, how: Settled, read_to: Option<u64>) {
    let key = (caller.to_string(), id.to_string());
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    if r.stopped.contains_key(&key) {
        return;
    }
    let Some(current) = r.owed.get_mut(&key).filter(|now| now.rev == o.rev && now.transcript_rev == o.transcript_rev) else { return };
    match how {
        Settled::Hold(ended) => current.held = Some((ended, now_ms())),
        Settled::Unhold => current.held = None,
        Settled::Skip(..) | Settled::Silent(..) | Settled::UnusedTurn(..) | Settled::PlanFyi(..) => {
            if matches!(how, Settled::Silent(_, true) | Settled::PlanFyi(_, _, true)) {
                current.wake.unread_to = current.wake.unread_to.max(read_to);
            }
            if let (Some(to), Settled::Skip(ended, ..) | Settled::Silent(ended, _) | Settled::PlanFyi(ended, ..)) = (read_to, &how) {
                current.note_transcript_check(*ended, to);
            }
            settle_told(&mut r, key, how, o);
            drop(r);
            crate::background::save();
        }
        _ => {}
    }
}

/// Whether a notice for `caller` waits, whatever it says: the last one is
/// still going in, and starts a turn of its own.
pub(in crate::agents) fn notice_waits(caller: &str) -> bool {
    TypingInto::under_way(caller)
}

/// A debt a notice was put together on has become another since (`Owed::rev`), or
/// is gone (a wait took the answer, the person took the session over), and what it
/// says may no longer hold: nothing of it is typed, nothing is settled, and the next
/// look starts over. A debt taken since is left to `settle_told`.
fn debts_moved(caller: &str, told: &[(String, Settled, Owed)]) -> bool {
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    told.iter().any(|(id, how, o)| {
        r.owed.get(&(caller.to_string(), id.clone())).is_none_or(|now| {
            now.rev != o.rev
                || now.transcript_rev != o.transcript_rev
                || now.wake.late_status_to != o.wake.late_status_to
                || matches!(how, Settled::Resumed) && now.restart_rev != o.restart_rev
        })
    })
}

/// The one exclusion between text going into a session and what changes whose
/// it is: by session, those a notice is going into or telling of now (and
/// those held sends and sends are typing into, `going_into`), and those being
/// handed over to the person (agents/handover.rs). Either waits for the other,
/// so neither lands between a notice's last look at its debts and its text
/// going in.
#[derive(Default)]
struct Gate {
    telling: HashMap<String, usize>,
    handing: std::collections::HashSet<String>,
}

fn gate() -> &'static (std::sync::Mutex<Gate>, std::sync::Condvar) {
    static G: std::sync::OnceLock<(std::sync::Mutex<Gate>, std::sync::Condvar)> = std::sync::OnceLock::new();
    G.get_or_init(Default::default)
}

/// A notice's hold on its caller and the sessions it tells of, from its last
/// look at their debts until its text is in (`tell_of`), or text's hold on the
/// session it is typed into (`going_into`): a handover of any of them waits.
pub(in crate::agents) struct Telling(Vec<String>);

impl Drop for Telling {
    fn drop(&mut self) {
        let (lock, done) = gate();
        let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
        for id in &self.0 {
            if let Some(n) = g.telling.get_mut(id) {
                *n -= 1;
                if *n == 0 {
                    g.telling.remove(id);
                }
            }
        }
        done.notify_all();
    }
}

/// The hold a notice to `caller` about `told` goes in under, if it still may: none
/// of its sessions, its caller included, is being handed over, and its debts
/// are as it found them. All of it is looked at under the gate, so a handover
/// either waits for this notice or is in place before it, and it goes nowhere.
fn tell_of(
    caller: &str,
    told: &[(String, Settled, Owed)],
    system_news: &[SystemNews],
    losses: &[(String, LineEvidence)],
) -> Option<Telling> {
    let mut g = gate().0.lock().unwrap_or_else(|e| e.into_inner());
    let ids: Vec<_> = told
        .iter()
        .map(|(id, ..)| id.as_str())
        .chain(losses.iter().map(|(id, _)| id.as_str()))
        .chain(system_news.iter().flat_map(SystemNews::sessions))
        .chain([caller])
        .collect();
    if ids.iter().any(|&id| g.handing.contains(id)) || debts_moved(caller, told) {
        return None;
    }
    let ids: Vec<String> = ids.into_iter().map(str::to_string).collect();
    for id in &ids {
        *g.telling.entry(id.clone()).or_default() += 1;
    }
    Some(Telling(ids))
}

/// The hold text other than a notice goes into session `id` under (a held
/// send, a send), if it still may: nothing is handing `id` over, looked at
/// under the gate, so a handover started now waits for this text to be in.
pub(in crate::agents) fn going_into(id: &str) -> Option<Telling> {
    let mut g = gate().0.lock().unwrap_or_else(|e| e.into_inner());
    if g.handing.contains(id) {
        return None;
    }
    *g.telling.entry(id.to_string()).or_default() += 1;
    Some(Telling(vec![id.to_string()]))
}

/// A handover's hold on session `id` (`hand_off`), let go when dropped.
pub(in crate::agents) struct HandingOff(String);

impl Drop for HandingOff {
    fn drop(&mut self) {
        let (lock, done) = gate();
        lock.lock().unwrap_or_else(|e| e.into_inner()).handing.remove(&self.0);
        done.notify_all();
    }
}

/// How long a handover waits for a notice going in about its session: a notice's
/// typing gives up well within this (delivery.rs).
const HAND_OFF_WAIT: Duration = Duration::from_secs(120);

/// Holds session `id` for its handover: no notice about it starts going in from
/// here, and one going in now is waited for. Err if that one is not in within
/// HAND_OFF_WAIT; nothing has changed hands then, and the window says it again.
pub(in crate::agents) fn hand_off(id: &str) -> Result<HandingOff, String> {
    // Before the gate: the registry is taken under it only after it.
    let who = label(id);
    let (lock, done) = gate();
    let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
    if !g.handing.insert(id.to_string()) {
        return Err(format!("{who} is being handed over already"));
    }
    let deadline = Instant::now() + HAND_OFF_WAIT;
    while g.telling.contains_key(id) {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            g.handing.remove(id);
            done.notify_all();
            return Err(format!("a notice about {who} is still going in; it is handed over once that is through"));
        }
        g = done.wait_timeout(g, left).unwrap_or_else(|e| e.into_inner()).0;
    }
    Ok(HandingOff(id.to_string()))
}

/// What the breaker counts, and all it silences: notices a turn's or a job's
/// end brings. Not a prompt or a question, which hold until they are answered
/// and cannot loop by themselves, and a busy helper may ask many.
fn counts_for_breaker(how: &Settled) -> bool {
    matches!(
        how,
        Settled::Answered | Settled::Pending(..) | Settled::Progress(_) | Settled::Failed | Settled::FailedRequests(..) | Settled::Resumed
    )
}

fn failure(how: &Settled) -> bool {
    matches!(how, Settled::Failed | Settled::FailedRequests(..) | Settled::Resumed)
}

fn breaker_blocks(pty: &crate::pty::PtyManager, caller: &str, id: &str, how: &Settled) -> bool {
    if !counts_for_breaker(how) || !pair_stopped(pty, caller, id) {
        return false;
    }
    !failure(how) || reg().lock().unwrap_or_else(|e| e.into_inner()).failures_told.contains_key(&(caller.into(), id.into()))
}

/// What a notice tells of, for `deliver_notice` to settle once it is in.
#[derive(Default)]
struct Notice {
    text: String,
    /// The session, what became of it, the debt as it was announced.
    settled: Vec<(String, Settled, Owed)>,
    /// The usage windows it told of.
    quota_lines: Vec<QuotaNews>,
    /// The spawns cut off by a core going.
    spawn_lines: Vec<(u64, String)>,
    /// The queued lines.
    news_lines: Vec<String>,
    system_news: Vec<SystemNews>,
    /// How long it waited for company (`hold_for_company`), for the log: what
    /// a wider window of batching would have saved is read off there.
    held_for: Option<Duration>,
    evidence: HashMap<String, LineEvidence>,
    losses: Vec<(String, LineEvidence)>,
}

impl Notice {
    fn len(&self) -> usize {
        self.settled.len()
            + self.losses.len()
            + self.quota_lines.len()
            + self.spawn_lines.len()
            + self.news_lines.len()
            + self.system_news.len()
    }

    fn about(&self) -> String {
        self.settled
            .iter()
            .map(|(id, how, _)| format!("{} ({})", label(id), kind_of(how)))
            .chain(self.losses.iter().map(|(id, _)| format!("{} (losses)", label(id))))
            .chain(self.quota_lines.iter().map(|n| format!("{} at {}%", n.key.1, n.told.level)))
            .chain(self.spawn_lines.iter().map(|(request, _)| format!("spawn {request}")))
            .chain(self.news_lines.iter().map(|_| "news no debt brings".to_string()))
            .chain(self.system_news.iter().map(|_| "system news".to_string()))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// What a rendered line actually read and can acknowledge after delivery.
#[derive(Default)]
pub(super) struct LineEvidence {
    requests: Vec<Request>,
    lost: Vec<String>,
    parent: bool,
    stopped: Vec<u32>,
    read_to: Option<u64>,
    ended: u32,
    batch_failed: bool,
}

/// The name of what became of a session, for the log line of the notice that
/// tells of it.
fn kind_of(how: &Settled) -> &'static str {
    match how {
        Settled::Answered => "answered",
        Settled::Failed | Settled::FailedRequests(..) => "failed",
        Settled::Pending(..) => "pending",
        Settled::Progress(_) => "reported",
        Settled::UnusedTurn(..) => "unused-turn",
        Settled::PlanFyi(..) => "plan-fyi",
        Settled::Hold(_) => "hold",
        Settled::Unhold => "unhold",
        Settled::Skip(..) => "skip",
        Settled::Waiting => "waiting",
        Settled::Asked(_) => "asked",
        Settled::Resumed => "resumed",
        Settled::Quiet(_) | Settled::QuietFor(_, _, false) => "quiet",
        Settled::WaitsOnBackground(_) | Settled::QuietFor(_, _, true) => "waits-on-background",
        Settled::Silent(..) => "silent",
        Settled::Exited => "exited",
    }
}

/// Types a notice into `caller` and settles what it said (`Notice`).
fn deliver_notice(pty: &crate::pty::PtyManager, caller: &str, notice: Notice) {
    deliver_notice_with(pty, caller, notice, |text, before, baseline| {
        if crate::background::has(caller) {
            match crate::background::send_notice(caller, text) {
                Ok((placed, for_stdin)) => (
                    Ok(Some(true)),
                    baseline + 1 + u32::from(placed == crate::background::Placed::Queued),
                    placed == crate::background::Placed::Joined || for_stdin,
                ),
                Err(e) => (Err(e), 0, false),
            }
        } else {
            (type_text(pty, caller, text, before, baseline), baseline + 1, false)
        }
    });
}

fn deliver_notice_with(
    pty: &crate::pty::PtyManager,
    caller: &str,
    notice: Notice,
    send: impl FnOnce(&str, Option<crate::activity::Turn>, u32) -> (Result<Option<bool>, String>, u32, bool),
) {
    let Notice { ref text, ref settled, ref system_news, ref evidence, ref losses, .. } = notice;
    let before = crate::activity::turn_state(caller);
    let baseline = before.map(|t| t.turns_ended).unwrap_or(0);
    // Again here, where it goes in, and held from this check until it is in
    // (`Telling`): the look that put it together was before this thread, and a
    // debt it tells of may have gone since (a handover clears them,
    // agents/handover.rs) or become another, or a handover of one of its
    // sessions be under way. Then nothing of it goes in and nothing is settled;
    // the next look starts over from what stands.
    for (id, ..) in settled {
        crate::agents::read::transcript_path(pty, id);
    }
    let Some(telling) = tell_of(caller, settled, system_news, losses) else {
        crate::log::info("agents", &format!("notice to {} not typed: what it told of changed as it was about to go in", label(caller)));
        return;
    };
    #[cfg(test)]
    tests::between_check_and_typing(caller);
    if !system_news.is_empty() && !system::still_due(pty, caller, system_news) {
        crate::log::info("agents", &format!("notice to {} not typed: its system news changed before delivery", label(caller)));
        return;
    }
    if losses.iter().map(|(id, read)| (id, read)).chain(evidence.iter()).any(|(id, read)| !losses_still_due(id, caller, read)) {
        return;
    }
    // The turn it starts: the next, or for a background caller that has begun
    // one since the look, the one after that. A background caller whose turn
    // runs on and took it on stdin (`Placed::Joined`) answers it within that
    // turn, which is no turn on a notice: it is not marked as one. Nor is it
    // for a lead found taking notices so that its CLI has begun a turn of its
    // own since the look: queued, it goes onto that stdin at the turn's
    // `result` (background/events.rs `feed_queued`).
    let (typed, turn, joined) = send(text, before.map(|t| t.turn), baseline);
    // In, or not to be: a handover waiting on it goes ahead.
    drop(telling);
    let about = notice.about();
    if let Err(e) = &typed {
        crate::log::warn("agents", &format!("notice to {} about {about} failed: {e}", label(caller)));
    }
    apply_delivery(pty, caller, notice, typed, turn, joined);
}

fn apply_delivery(
    pty: &crate::pty::PtyManager,
    caller: &str,
    notice: Notice,
    typed: Result<Option<bool>, String>,
    turn: u32,
    joined: bool,
) {
    // A readable draft proves Enter missed. With no such evidence, a late
    // start and a missed Enter look alike. Honor the write, but only mark a
    // turn as started by this notice if we saw it begin.
    let typed = if matches!(typed, Ok(Some(false))) && pty.composer(caller).is_some_and(|c| !c.text.trim().is_empty()) {
        pty.set_draft(caller, notice.text.chars().count() as u64);
        Ok(None)
    } else {
        typed
    };
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    if matches!(typed, Ok(None)) {
        r.notices_held.insert(caller.to_string(), notice.len());
    } else {
        r.notices_held.remove(caller);
    }
    drop(r);
    if let Ok(Some(started)) = typed {
        finish_notice(pty, caller, notice, joined, started.then_some(turn));
    }
}

fn finish_notice(pty: &crate::pty::PtyManager, caller: &str, notice: Notice, joined: bool, turn: Option<u32>) {
    let about = notice.about();
    let Notice { settled, quota_lines, spawn_lines, news_lines, system_news, held_for, mut evidence, losses, .. } = notice;
    let held = held_for.map_or(String::new(), |d| format!("; held {} s for company", d.as_secs()));
    let into = if joined { "; on the stdin of its running process" } else { "" };
    if turn.is_some() {
        crate::log::info("agents", &format!("notice to {} about {about}{held}{into}", label(caller)));
    } else {
        crate::log::warn("agents", &format!("notice to {} about {about} has no confirmed turn{held}", label(caller)));
    }
    let mut stopped = Vec::new();
    // Read before the registry's lock: where each helper's turns stand.
    let ended: HashMap<String, u32> =
        settled.iter().filter_map(|(id, _, _)| Some((id.clone(), crate::activity::turn_state(id)?.turns_ended))).collect();
    for (id, ..) in &settled {
        crate::agents::read::transcript_path(pty, id);
    }
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(turn) = turn.filter(|_| !joined) {
        let told_of: Vec<String> = settled.iter().map(|(id, _, _)| id.clone()).chain(losses.iter().map(|(id, _)| id.clone())).collect();
        let wake = news_lines.iter().any(|line| is_wake(line));
        notice_turn(&mut r, caller, turn, &told_of, wake);
    }
    let told_debts = !settled.is_empty() || !losses.is_empty();
    let mut counted = std::collections::HashSet::new();
    for (id, how, told) in settled {
        if counts_for_breaker(&how) {
            counted.insert(id.clone());
            if count_delivery(&mut r, caller, &id, failure(&how)) {
                stopped.push(id.clone());
            }
        }
        let now_ended = ended.get(&id).copied();
        let read = evidence.remove(&id).unwrap_or_default();
        settle_delivered(&mut r, caller, id, how, &told, read, now_ended);
    }
    for (id, read) in losses {
        let key = (caller.to_string(), id.clone());
        let (lost, stopped_turns) = ack_losses(&r, &key, &read);
        settle_lost_requests(&mut r, &key, &lost);
        if (!lost.is_empty() || stopped_turns) && counted.insert(id.clone()) && count_delivery(&mut r, caller, &id, true) {
            stopped.push(id);
        }
    }
    let told_quota = !quota_lines.is_empty();
    for n in quota_lines {
        r.quota_told.insert(n.key, n.told);
    }
    r.news.retain(|(c, line)| !(c == caller && news_lines.contains(line)));
    system::told(&mut r, &system_news);
    places_told(&mut r, caller, &news_lines);
    // Told: not to be told again by a core after this one.
    let told_news = !news_lines.is_empty() || system_news.iter().any(SystemNews::survives_restart);
    let told_spawns = !spawn_lines.is_empty();
    for (request, _) in spawn_lines {
        r.spawns.remove(&request);
    }
    drop(r);
    if told_quota {
        save_quota_told();
    }
    if told_debts || told_news || told_spawns {
        crate::background::save();
    }
    for id in stopped {
        breaker_tripped(caller, &id);
    }
}

fn losses_still_due(id: &str, caller: &str, read: &LineEvidence) -> bool {
    (read.lost.is_empty()
        || crate::background::undelivered_snapshot(id, caller, read.parent).iter().filter(|(qid, _)| read.lost.contains(qid)).count()
            == read.lost.len())
        && (read.stopped.is_empty()
            || crate::background::untold_losses(id, caller).iter().filter(|loss| read.stopped.contains(&loss.turn)).count()
                == read.stopped.len())
}

fn count_delivery(r: &mut Registry, caller: &str, id: &str, failure: bool) -> bool {
    let now = now_ms();
    let key = (caller.into(), id.into());
    if r.stopped.contains_key(&key) {
        if failure {
            r.failures_told.insert(key, now);
        }
        return false;
    }
    if count_notice(r, caller, id, now) {
        system::breaker(r, caller, id, now);
        return true;
    }
    false
}

fn ack_losses(r: &Registry, key: &(String, String), read: &LineEvidence) -> (Vec<crate::background::Queued>, bool) {
    let (caller, id) = key;
    let mut lost = Vec::new();
    if !read.lost.is_empty() {
        let mut ids = read.lost.clone();
        if let Some(qs) = r.requests.get(key) {
            // A retry keeps its queue ID. Evidence of the old attempt must
            // leave the new attempt's loss (if any) available for its caller.
            ids.retain(|qid| {
                !read.requests.iter().any(|old| {
                    old.delivery.as_ref() == Some(qid) && qs.open.iter().any(|q| q.n == old.n && q.delivery_rev != old.delivery_rev)
                })
            });
        }
        lost = take_undelivered_ids(id, caller, read.parent, Some(&ids)).queued;
    }
    let stopped = !read.stopped.is_empty() && !crate::background::losses_told(id, caller, &read.stopped).is_empty();
    (lost, stopped)
}

fn settle_lost_requests(r: &mut Registry, key: &(String, String), lost: &[crate::background::Queued]) {
    let Some(now) = r.owed.get(key).copied().filter(|_| !lost.is_empty()) else { return };
    let covered: Vec<_> = r
        .requests
        .get(key)
        .into_iter()
        .flat_map(|qs| &qs.open)
        .filter(|q| q.delivery.as_ref().is_some_and(|qid| lost.iter().any(|q| &q.id == qid)))
        .map(|q| q.n)
        .collect();
    super::owed::take_requests(r, key, now, now, &covered, None);
}

/// The refusals for want of places a notice that went into `caller` told it of
/// (`news_lines`), gone with it. A refusal since the line was put together
/// makes another line, told next.
fn places_told(r: &mut Registry, caller: &str, news_lines: &[String]) {
    r.places_refused.retain(|c, p| !(c == caller && news_lines.iter().any(|queued| *queued == p.line())));
}

/// A notice that went into `caller` settles what it told of `id`.
fn settle_delivered(r: &mut Registry, caller: &str, id: String, how: Settled, told: &Owed, read: LineEvidence, ended: Option<u32>) {
    let key = (caller.to_string(), id.clone());
    let (lost, _) = ack_losses(r, &key, &read);
    let Some(current) = r.owed.get(&key).copied() else { return };
    if current.transcript_rev != told.transcript_rev {
        // A conversation switch invalidates the words and their offsets.
        // A queue ID says which send failed regardless of the conversation.
        settle_lost_requests(r, &key, &lost);
        return;
    }
    let answered = matches!(how, Settled::Answered | Settled::Failed | Settled::FailedRequests(..) | Settled::Exited);
    if current.delivery_rev != told.delivery_rev
        && (!answered || read.requests.is_empty())
        && !matches!(how, Settled::Resumed | Settled::Progress(_))
    {
        return;
    }
    if current.wake.late_status_to != told.wake.late_status_to && current.wake.late_status_to > read.read_to {
        return;
    }
    if let Some(to) = read.read_to {
        let wake = &mut r.owed.get_mut(&key).unwrap().wake;
        wake.reported_to = wake.reported_to.max(Some(to));
        if wake.late_status_to.is_some_and(|end| end <= to) {
            wake.late_status_to = None;
        }
        if told.notice_cursor() <= current.cursor && wake.unread_to.is_some_and(|end| end <= to) {
            wake.unread_since = None;
            wake.unread_to = None;
        }
    }
    // Told of its end: a wake before it is behind it (lines.rs `woken_before`).
    // Not one at the turns it has ended now (`ended`), which it has yet to take
    // up: another caller told of the same end without a wake would have the
    // next look wake it again.
    let behind = |w: &lines::Woken| read.read_to.map(|_| read.ended).or(ended).is_none_or(|e| w.ended < e);
    if matches!(how, Settled::Answered | Settled::Failed | Settled::Exited) && r.woken.get(&id).is_some_and(behind) {
        r.woken.remove(&id);
    }
    if answered && !read.requests.is_empty() {
        let covered: Vec<_> = r
            .requests
            .get(&key)
            .into_iter()
            .flat_map(|qs| &qs.open)
            .filter(|q| {
                read.requests.iter().any(|old| old.n == q.n && old.delivery_rev == q.delivery_rev && old.delivery == q.delivery)
                    && match &how {
                        Settled::FailedRequests(_, covered) => covered.contains(&q.n),
                        _ => true,
                    }
            })
            .map(|q| q.n)
            .collect();
        if !covered.is_empty() || matches!(how, Settled::FailedRequests(..)) {
            let current = r.owed[&key];
            super::owed::take_requests(r, &key, current, *told, &covered, (!read.batch_failed).then_some(read.ended));
        }
    } else {
        settle_told(r, key, how, told);
    }
}

/// What a notice that went in does to the debt it told of (`told`). Only to
/// that debt: typing the notice takes seconds, and the turn it starts may ask
/// again before they are over, a question that is owed on; and a `wait` may
/// have taken the answer meanwhile, which leaves nothing to carry on.
pub(super) fn settle_told(r: &mut Registry, key: (String, String), how: Settled, told: &Owed) {
    let Some(current) = r.owed.get(&key).copied() else { return };
    if current.transcript_rev != told.transcript_rev {
        return;
    }
    if matches!(how, Settled::Resumed) {
        if current.restart_rev == told.restart_rev && current.resumed == told.resumed && current.job_went == told.job_went {
            let o = r.owed.get_mut(&key).unwrap();
            o.resumed = false;
            o.job_went = false;
            o.wake.job_running = false;
        }
        return;
    }
    if let Settled::Progress(progress) = &how {
        mark_requests(r, &key, &progress.requests);
        if !progress.work_left && !progress.plan {
            // A reply can finish while another request is still queued. Its
            // outcome has reached the caller even though the pair stays open.
            let covered: Vec<_> = r
                .requests
                .get(&key)
                .into_iter()
                .flat_map(|qs| &qs.open)
                .filter(|q| {
                    progress
                        .requests
                        .iter()
                        .any(|said| said.taken && said.n == q.n && said.delivery_rev == q.delivery_rev && said.delivery == q.delivery)
                })
                .map(|q| q.n)
                .collect();
            if !covered.is_empty() {
                super::owed::take_requests(r, &key, current, *told, &covered, None);
            }
            if !r.owed.contains_key(&key) {
                return;
            }
        }
    }
    if current.delivery_rev != told.delivery_rev {
        return;
    }
    if let Settled::QuietFor(ended, target, background) = how {
        let o = r.owed.get_mut(&key).unwrap();
        if let Some(target) = target {
            o.wake.quiet_task = Some(target);
        }
        if background {
            o.told_background_wait = Some(ended);
        } else {
            o.told_quiet = Some(ended);
        }
        return;
    }
    if let Settled::FailedRequests(ended, requests) = how {
        super::owed::settle_requests(r, &key, *told, &requests, ended);
        return;
    }
    if current.rev != told.rev {
        // Told as it was before another ask took it in: that ask is what is left.
        if matches!(how, Settled::Answered | Settled::Failed | Settled::Exited) {
            if told.upto == 0 {
                super::owed::told_before(r, &key, told.rev);
            } else {
                let covered: Vec<_> =
                    r.requests.get(&key).into_iter().flat_map(|qs| &qs.open).filter(|q| q.n <= told.upto).map(|q| q.n).collect();
                super::owed::settle_requests(r, &key, *told, &covered, told.notice_baseline());
            }
        } else if let Settled::Progress(progress) = &how {
            let next_prompt = if told.upto == 0 {
                current.cursor_after(told.rev)
            } else {
                r.requests.get(&key).and_then(|qs| qs.open.iter().find(|q| q.n > told.upto)).map(|q| q.cursor)
            };
            if let Some(next_prompt) = next_prompt {
                let o = r.owed.get_mut(&key).unwrap();
                let cursor = progress.cursor.min(next_prompt);
                if progress.plan {
                    o.wake.plan_through = Some(progress.ended);
                }
                if cursor > o.cursor {
                    if o.wake.unread_since.is_none() {
                        o.cursor = cursor;
                    }
                    o.request_match = progress.matched;
                }
            }
        } else if matches!(how, Settled::Quiet(_) | Settled::WaitsOnBackground(_)) && current.quiet_target() == told.quiet_target() {
            r.owed.get_mut(&key).unwrap().wake.quiet_task = told.quiet_target();
        }
        return;
    }
    match how {
        Settled::Answered | Settled::Failed | Settled::Exited => {
            if current.wake.unread_since.is_some() {
                let o = r.owed.get_mut(&key).unwrap();
                settle_silent(o, o.notice_baseline(), false);
                o.cut_short = false;
                o.wake.plan_through = None;
            } else {
                r.owed.remove(&key);
            }
        }
        Settled::FailedRequests(..) | Settled::QuietFor(..) => unreachable!(),
        Settled::Waiting => {
            if let Some(o) = r.owed.get_mut(&key) {
                o.told_waiting = true;
            }
        }
        Settled::Pending(ended, cursor, request_match, status, job_running) => {
            let mut wake = told.wake;
            if let Some(status) = status {
                wake.reported(status, ended);
            }
            wake.job_running = job_running;
            wake.after = None;
            wake.skipped = None;
            wake.silent = None;
            let current_wake = current.wake;
            wake.unread_since = current_wake.unread_since;
            wake.unread_to = current_wake.unread_to;
            wake.reported_to = current_wake.reported_to;
            wake.late_status_to = current_wake.late_status_to;
            let cursor = if wake.unread_since.is_some() { current.cursor.min(cursor) } else { cursor };
            r.owed.insert(
                key,
                Owed { request_match, wake, no_reply: told.no_reply, transcript_rev: told.transcript_rev, ..Owed::pending(ended, cursor) },
            );
        }
        Settled::Progress(progress) => {
            let o = r.owed.get_mut(&key).unwrap();
            if o.wake.unread_since.is_none() {
                o.cursor = progress.cursor;
            }
            o.request_match = progress.matched;
            o.baseline = progress.ended;
            o.held = None;
            o.queued = progress.requests.iter().any(|q| !q.taken && q.queued);
            o.told_pending = progress.pending;
            o.wake.job_running = progress.job;
            o.wake.after = None;
            o.wake.skipped = (progress.work_left && !progress.pending).then_some(progress.ended);
            o.wake.unused_turn = None;
            o.wake.unused_to = None;
            o.wake.plan_through = progress.plan.then_some(progress.ended);
            o.wake.silent = (progress.plan && !progress.work_left && !progress.requests.iter().any(|q| !q.taken)).then_some(progress.ended);
        }
        Settled::PlanFyi(ended, requests, unread) => {
            mark_requests(r, &key, &requests);
            let o = r.owed.get_mut(&key).unwrap();
            o.wake.plan_through = Some(ended);
            o.told_pending = false;
            o.wake.skipped = None;
            settle_silent(o, ended, unread);
        }
        Settled::UnusedTurn(ended, checked_to) => {
            let o = r.owed.get_mut(&key).unwrap();
            o.wake.unused_turn = Some(ended);
            o.wake.unused_to = Some((ended, checked_to));
            o.wake.skipped = None;
            o.told_pending = false;
            o.wake.job_running = false;
            o.held = None;
        }
        Settled::Asked(ask) => {
            if let Some(o) = r.owed.get_mut(&key) {
                o.told_ask = ask;
            }
        }
        Settled::Resumed => unreachable!(),
        Settled::Quiet(ended) => {
            if let Some(o) = r.owed.get_mut(&key) {
                o.wake.quiet_task = o.quiet_target();
                o.told_quiet = Some(ended);
            }
        }
        Settled::WaitsOnBackground(ended) => {
            if let Some(o) = r.owed.get_mut(&key) {
                o.wake.quiet_task = o.quiet_target();
                o.told_background_wait = Some(ended);
            }
        }
        Settled::Skip(ended, cursor, job_running) => {
            let o = r.owed.get_mut(&key).unwrap();
            o.wake.skipped = Some(ended);
            o.wake.job_running = job_running;
            o.held = None;
            o.held_to = Some(cursor);
        }
        Settled::Silent(ended, unread) => settle_silent(r.owed.get_mut(&key).unwrap(), ended, unread),
        Settled::Hold(_) | Settled::Unhold => {}
    }
}

fn settle_silent(o: &mut Owed, ended: u32, unread: bool) {
    o.wake.silent = Some(ended);
    if o.wake.plan_through.is_some_and(|through| ended > through) {
        o.wake.plan_through = None;
    }
    if unread {
        o.wake.unread_since.get_or_insert_with(now_ms);
    }
    o.wake.job_running = false;
    o.held = None;
}

/// A notice carries evidence only for the prompts it read. A send that
/// merged during delivery has a different number and keeps its own state.
fn mark_requests(r: &mut Registry, key: &(String, String), snapshots: &[Request]) {
    if let Some(requests) = r.requests.get_mut(key) {
        for q in &mut requests.open {
            if let Some(snapshot) = snapshots
                .iter()
                .find(|snapshot| snapshot.n == q.n && snapshot.delivery_rev == q.delivery_rev && snapshot.delivery == q.delivery)
            {
                q.taken |= snapshot.taken;
                q.told |= snapshot.told;
            }
        }
    }
}

fn breaker_suffix(caller: &str, id: &str, how: &Settled, now: u64) -> Option<&'static str> {
    if !counts_for_breaker(how) || !last_notice(caller, id, now) {
        return None;
    }
    if is_persons_chat(caller) {
        return Some(" [turn notices stopped; asks still arrive; type in either session to resume]");
    }
    Some(" [turn notices stopped; luna_report_get]")
}

/// Logs the breaker trip and shows the desktop toast.
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
    use crate::agents::registry::{parent_of, reg, register};
    use crate::agents::teardown::forget;
    use crate::agents::test_support::job_over_helper;
    use crate::agents::wait::{Reserved, WaitOut};

    type Between = (String, Box<dyn FnOnce() + Send>);

    /// What a test has run in a notice's delivery to a caller, once, between its
    /// check and its text going in (`deliver_notice`).
    static BETWEEN: std::sync::Mutex<Vec<Between>> = std::sync::Mutex::new(Vec::new());

    pub(super) fn run_between(caller: &str, run: Box<dyn FnOnce() + Send>) {
        BETWEEN.lock().unwrap_or_else(|e| e.into_inner()).push((caller.to_string(), run));
    }

    pub(super) fn between_check_and_typing(caller: &str) {
        let hook = {
            let mut b = BETWEEN.lock().unwrap_or_else(|e| e.into_inner());
            b.iter().position(|(c, _)| c == caller).map(|i| b.remove(i).1)
        };
        if let Some(run) = hook {
            run();
        }
    }

    /// A handover started exactly between a notice's check and its text going in
    /// waits for that notice, which goes in as the helper's answer it was, and only
    /// then changes hands. A notice that finds a handover of its session under way
    /// does not go in. Both providers alike.
    #[test]
    fn a_handover_and_a_notice_going_in_wait_for_each_other() {
        for (provider, tag) in [(Provider::Claude, "cl"), (Provider::Codex, "cx")] {
            let (a, id, next) = (format!("hb-{tag}-caller"), format!("hb-{tag}-helper"), format!("hb-{tag}-next"));
            let bg = serde_json::json!({ "background": true });
            register(&a, Provider::Claude, "C:\\p", "C:\\acc\\one", None, true, serde_json::json!({}));
            register(&id, provider, "C:\\p", "C:\\acc\\one", Some(&a), false, bg.clone());
            register(&next, provider, "C:\\p", "C:\\acc\\one", Some(&a), false, bg);
            // The caller mid-turn: a notice for it queues, to be seen.
            crate::background::test_helper(&a, "C:\\acc", None, true);
            crate::background::idle_for_test(&id, provider, 1, false);
            let looked_up = |helper: &str| {
                owe_from(&a, helper, 0, 0);
                reg().lock().unwrap().owed[&(a.clone(), helper.to_string())]
            };
            let pty = crate::pty::manager();
            let text = format!("{NOTICE_TAG} {id} answered");

            let (sent, handover) = std::sync::mpsc::channel();
            let (helper, caller) = (id.clone(), a.clone());
            BETWEEN.lock().unwrap().push((
                a.clone(),
                Box::new(move || {
                    let taking = helper.clone();
                    let t = std::thread::spawn(move || crate::agents::take_over(&taking, || Some(Vec::new())).unwrap());
                    let until = Instant::now() + Duration::from_secs(5);
                    while !gate().0.lock().unwrap().handing.contains(&helper) {
                        assert!(Instant::now() < until, "the handover never took its hold");
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    std::thread::sleep(Duration::from_millis(200));
                    assert_eq!(parent_of(&helper).as_deref(), Some(caller.as_str()), "nothing changed hands under the notice");
                    sent.send(t).unwrap();
                }),
            ));
            deliver_notice(
                pty,
                &a,
                Notice { text: text.clone(), settled: vec![(id.clone(), Settled::Answered, looked_up(&id))], ..Notice::default() },
            );
            handover.recv().unwrap().join().unwrap();
            assert_eq!(crate::background::queued_for_test(&a), std::slice::from_ref(&text), "the notice went in whole");
            assert_eq!(parent_of(&id), None, "and the handover after it");

            // The other order: a handover of `next` under way, a notice about it waits
            // for nothing and goes nowhere.
            let held = hand_off(&next).unwrap();
            let other = format!("{NOTICE_TAG} {next} answered");
            deliver_notice(
                pty,
                &a,
                Notice { text: other, settled: vec![(next.clone(), Settled::Answered, looked_up(&next))], ..Notice::default() },
            );
            assert_eq!(crate::background::queued_for_test(&a), [text], "nothing more went in");
            drop(held);

            reg().lock().unwrap().news.retain(|(c, _)| *c != a);
            for s in [&id, &next, &a] {
                crate::background::gone_for_test(s);
                forget(s);
            }
        }
    }

    #[test]
    fn a_late_caller_loses_delivery_before_its_unread_hold() {
        let id = "gone-unread-helper";
        register(id, Provider::Claude, "C:/p", "C:/acc/one", None, false, serde_json::json!({}));
        let known = HashMap::from([(id.to_string(), reg().lock().unwrap().known[id].clone())]);
        let key = ("gone-unread-caller".into(), id.to_string());
        let mut silent = Owed::from(0, 0);
        silent.wake.silent = Some(1);
        silent.wake.plan_through = Some(1);
        silent.wake.unread_since = Some(now_ms());
        let mut owed = HashMap::from([(key.clone(), silent)]);
        prune_owed(&mut owed, &known, &HashMap::new(), CALLER_RETURN_MS - 1, now_ms(), |_| true);
        assert!(owed.contains_key(&key), "the caller can still come back");
        prune_owed(&mut owed, &known, &HashMap::new(), CALLER_RETURN_MS, now_ms(), |_| true);
        let expired = owed.get_mut(&key).expect("the unread answer still holds auto-delete");
        assert!(!expired.active() && !expired.watches_later_turns());
        assert!(crate::agents::teardown::unread_holds(expired, now_ms()));
        let read = expired.wake.unread_since.take().unwrap();
        prune_owed(&mut owed, &known, &HashMap::new(), CALLER_RETURN_MS, now_ms(), |_| true);
        let expired = owed.get_mut(&key).expect("a cancel can still restore the unread hold");
        expired.wake.unread_since = Some(read);
        expired.wake.delivery_expired = Some(now_ms());
        prune_owed(&mut owed, &known, &HashMap::new(), CALLER_RETURN_MS, now_ms(), |_| true);
        assert!(owed.is_empty(), "the unread hold expired");
        forget(id);
    }

    #[test]
    fn a_busy_parent_does_not_hold_back_its_leads_internal_wake() {
        let (top, lead) = ("busy-wake-top", "busy-wake-lead");
        let (mut known, o, path) = job_over_helper(lead, top, "");
        register(top, Provider::Claude, "C:/p", "C:/acc/one", None, true, serde_json::json!({}));
        crate::background::test_helper(top, "C:/acc/one", None, true);
        known.tools = true;
        known.parent = Some(top.into());
        let o = Owed { held: Some((3, now_ms() - BACKGROUND_GRACE_MS)), ..o };
        {
            let mut r = reg().lock().unwrap();
            r.known.insert(lead.into(), known);
            r.owed.insert((top.into(), lead.into()), o);
            r.delegates.insert(lead.into(), vec!["helper that finished".into()]);
        }
        let pty = crate::pty::manager();
        assert!(!ready_for_notice(pty, top));
        notify_once(pty, &mut HashMap::new(), &mut HashMap::new(), |caller| caller == top, |_, _, _| {});
        assert!(reg().lock().unwrap().news.iter().any(|(id, line)| id == lead && is_wake(line)));
        for id in [top, lead] {
            crate::background::gone_for_test(id);
            forget(id);
        }
        let _ = std::fs::remove_file(path);
    }

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

    /// What a turn left is named, and the debt waits for its end however long
    /// it does nothing: idle is no proof of an end.
    #[test]
    fn what_a_turn_left_is_named_and_its_end_is_the_news() {
        let (known, o, path) = job_over_helper("lj-s", "lj-c", "");
        let pty = crate::pty::manager();
        let line = |o: Owed| notice_line(pty, "lj-c", "Your helper", "lj-s", &known, o);
        // Its first turn's end, with the tail just left: not done, and what runs is named.
        crate::background::left_for_test("lj-s", &[("tail.exe", 22104)]);
        let mut first = Owed { rev: o.rev, ..Owed::from(2, o.cursor) };
        first.held = Some((3, now_ms() - BACKGROUND_GRACE_MS));
        assert!(matches!(line(first), Some((None, Settled::Skip(3, ..)))), "interim end stays quiet");
        // Told: still a job, nothing more to say, and no end for a wait either.
        let told = Owed { told_pending: true, ..o };
        assert!(matches!(line(told), None | Some((None, _))));
        assert!(!job_over(&told, crate::activity::turn_state("lj-s")));
        // Its end: that is the news the debt waited for.
        crate::background::left_for_test("lj-s", &[]);
        let mut ended = told;
        ended.held = Some((3, now_ms() - BACKGROUND_GRACE_MS));
        assert!(matches!(line(ended), Some((Some(_), Settled::Answered))), "the unanswered job end goes");
        ended.wake.reported(Status::Done, 3);
        assert!(matches!(line(ended), Some((None, Settled::Silent(3, false)))), "a told status leaves no stall or unread text");
        crate::background::gone_for_test("lj-s");
        forget("lj-s");
        let _ = std::fs::remove_file(&path);
    }

    /// Two helpers finishing seconds apart: the first's notice waits for the
    /// second's, up to COALESCE_MS, and goes alone past that or once nothing
    /// else of the caller's is at work.
    #[test]
    fn a_notice_waits_a_little_for_a_helper_about_to_finish() {
        let ready = Instant::now();
        let at = |s: u64| ready + Duration::from_secs(s);
        assert!(coalesce_holds(ready, at(0), true), "another helper of its mid-turn: it waits");
        assert!(coalesce_holds(ready, at(19), true));
        assert!(!coalesce_holds(ready, at(20), true), "never past COALESCE_MS");
        assert!(!coalesce_holds(ready, at(0), false), "nothing else at work: it goes at once");

        // What the look loop holds, as it puts a notice together: "a" answered,
        // "b" still at work.
        let owed = Owed::from(0, 0);
        let answered = vec![("a".to_string(), Settled::Answered, owed)];
        let owed_by = ["a", "b"];
        let b_works = |id: &str| id == "b";
        let mut since: HashMap<String, Instant> = HashMap::new();
        assert!(hold_for_company(&mut since, "c", at(0), &answered, &owed_by, b_works));
        assert!(hold_for_company(&mut since, "c", at(10), &answered, &owed_by, b_works), "{since:?}");
        assert_eq!(since.get("c"), Some(&at(0)), "the wait counts from the first look");
        assert!(!hold_for_company(&mut since, "c", at(21), &answered, &owed_by, b_works), "20 s at most");
        assert!(!since.contains_key("c"), "going out clears it");
        // "b" answered too meanwhile: both go now, in one notice.
        let both = vec![answered[0].clone(), ("b".to_string(), Settled::Answered, owed)];
        assert!(!hold_for_company(&mut since, "c", at(30), &both, &owed_by, b_works));
        // Nothing else at work: no wait.
        assert!(!hold_for_company(&mut since, "d", at(0), &answered, &owed_by, |_| false));
        assert!(!since.contains_key("d"));
        // A question or a permission prompt never waits, with company at work or not.
        for how in [Settled::Waiting, Settled::Asked(7)] {
            let asking = vec![("a".to_string(), how, owed)];
            assert!(!hold_for_company(&mut since, "e", at(0), &asking, &owed_by, b_works));
            let with_answer = vec![asking[0].clone(), ("x".to_string(), Settled::Answered, owed)];
            assert!(!hold_for_company(&mut since, "e", at(0), &with_answer, &["a", "x", "b"], b_works), "nor what goes with it");
        }
    }

    #[test]
    fn riders_use_an_existing_wake_and_only_actionable_refusals_can_go_alone() {
        assert!(riders_go(true, true, None));
        assert!(!riders_go(false, false, None));
        assert!(!riders_go(false, true, Some(RIDE_MAX_MS - 1)));
        assert!(riders_go(false, false, Some(0)));
        let mut o = Owed::pending(3, 0);
        assert!(!rides(&Settled::Resumed, o));
        assert!(riders_go(false, true, Some(RIDE_MAX_MS)));
        assert!(!rides(&Settled::Quiet(3), o));
        o.wake.quiet_task = o.quiet_target();
        assert!(rides(&Settled::Quiet(4), o));
        assert!(rides(&Settled::WaitsOnBackground(3), o));
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
        assert!(text.contains("mid-turn and has written nothing for 31 min") && text.contains("may hang"), "{text}");
        settle_told(&mut reg().lock().unwrap(), ("qt-c".to_string(), "qt-s".to_string()), Settled::Quiet(3), &o);
        let told = reg().lock().unwrap().owed.get(&("qt-c".to_string(), "qt-s".to_string())).copied().expect("still owed");
        assert_eq!(told.told_quiet, Some(3));
        assert!(matches!(line(told), None | Some((None, _))), "once per turn");
        assert!(!counts_for_breaker(&Settled::Quiet(3)), "no turn's end");
        crate::background::gone_for_test("qt-s");
        forget("qt-s");
    }

    #[test]
    fn only_a_send_wanting_a_reply_renews_the_quiet_allowance() {
        use crate::agents::owed::{owe_asked, Asked};
        let key = ("renew-stall-caller".to_string(), "renew-stall-helper".to_string());
        let (caller, id) = (&key.0, &key.1);
        let ask = |text, reply| owe_asked(caller, id, 0, 0, &Asked { text, key: None, reply });
        ask("Run the tests", true);
        let debt = || reg().lock().unwrap().owed[&key];
        let first = debt();
        assert!(!rides(&Settled::Quiet(0), first));
        settle_told(&mut reg().lock().unwrap(), key.clone(), Settled::Quiet(0), &first);
        assert!(rides(&Settled::WaitsOnBackground(0), debt()), "quiet conditions share one wake");
        ask("FYI", false);
        assert!(rides(&Settled::Quiet(1), debt()), "an FYI keeps the allowance spent");
        assert!(debt().legacy_can_stall(), "quiet does not spend the idle stall");
        ask("Try again", true);
        assert!(!rides(&Settled::Quiet(2), debt()), "a new task has its own allowance");
        forget(id);
        forget(caller);
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
        assert!(matches!(notice_line(pty, "jw-c", "Your helper", "jw-s", &known, stood), Some((Some(_), Settled::Answered))));
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

    /// The top chat hears of a refusal below it for want of places once, with
    /// the notice it went in; one that came while the line was being typed is
    /// told next.
    #[test]
    fn a_place_refusal_gets_a_wake_at_its_deadline_even_with_an_active_debt() {
        let (top, lead) = ("place-wake-top", "place-wake-lead");
        register(top, Provider::Claude, "C:/p", "C:/acc/one", None, true, serde_json::json!({}));
        register(lead, Provider::Claude, "C:/p", "C:/acc/one", Some(top), true, serde_json::json!({ "background": true }));
        let _terminal = crate::pty::notice_terminal(top, Provider::Claude, false);
        crate::background::test_helper(lead, "C:/acc/one", None, true);
        crate::background::quiet_for_test(lead, 0);
        owe_from(top, lead, 0, 0);
        let pty = crate::pty::manager();
        crate::agents::spawn::refused_a_place(&mut reg().lock().unwrap(), lead);
        let draft = || {
            let mut got = None;
            let mut ready = HashMap::from([(top.into(), NOTIFY_IDLE_LOOKS - 1)]);
            notify_once(
                pty,
                &mut ready,
                &mut HashMap::new(),
                |caller| caller == top,
                |_, caller, notice| {
                    if caller == top {
                        got = Some(notice.text);
                    }
                },
            );
            got
        };
        assert!(draft().is_none());
        reg().lock().unwrap().places_refused.get_mut(top).unwrap().since = Instant::now() - Duration::from_millis(RIDE_MAX_MS);
        assert!(draft().unwrap().contains("No place was left"));
        for id in [lead, top] {
            forget(id);
            crate::background::gone_for_test(id);
        }
    }

    #[test]
    fn a_restart_notice_and_its_rider_survive_a_draft_and_a_closed_write_gate() {
        let _alone = crate::agents::stops_for_test();
        let (caller, id) = ("resumed-draft-caller", "resumed-draft-helper");
        register(caller, Provider::Codex, ".", "acc", None, true, serde_json::json!({}));
        register(id, Provider::Claude, ".", "acc", Some(caller), true, serde_json::json!({"background":true}));
        let terminal = crate::pty::notice_terminal(caller, Provider::Codex, true);
        crate::background::idle_for_test(id, Provider::Claude, 0, false);
        owe_from(caller, id, 0, 0);
        let key = (caller.into(), id.into());
        let rider = "A queued line accompanying the restart.";
        {
            let mut r = reg().lock().unwrap();
            let o = r.owed.get_mut(&key).unwrap();
            o.resumed = true;
            o.job_went = true;
            r.news.push((caller.into(), rider.into()));
        }
        tick_for_test(caller);
        let pending = || {
            let r = reg().lock().unwrap();
            let o = r.owed[&key];
            o.active() && o.resumed && o.job_went && r.news.iter().any(|(c, line)| c == caller && line == rider)
        };
        assert!(pending());
        assert!(terminal.input.try_recv().is_err());
        crate::pty::manager().set_draft(caller, 0);
        crate::starts::under_update_for_test(caller);
        run_between(caller, Box::new(|| crate::starts::set_update(true)));
        tick_for_test(caller);
        crate::starts::set_update(false);
        assert!(pending());
        assert!(terminal.input.try_recv().is_err());
        tick_for_test(caller);
        let text: String = terminal.input.try_iter().collect();
        assert!(text.contains("resumed automatically") && text.contains(rider), "{text}");
        {
            let r = reg().lock().unwrap();
            let o = r.owed[&key];
            assert!(o.active() && !o.resumed && !o.job_went);
            assert!(!r.news.iter().any(|(c, _)| c == caller));
        }
        tick_for_test(caller);
        assert!(terminal.input.try_recv().is_err());
        crate::background::gone_for_test(id);
        forget(id);
        forget(caller);
        crate::activity::forget(caller);
    }

    #[test]
    fn a_failed_follow_up_launch_tells_only_of_its_own_batch() {
        use super::super::owed::{owe_sent, Asked};
        for (caller, id, provider) in [
            ("launch-claude-caller", "launch-claude-helper", Provider::Claude),
            ("launch-codex-caller", "launch-codex-helper", Provider::Codex),
        ] {
            register(caller, Provider::Claude, ".", "acc", None, true, serde_json::json!({}));
            register(id, provider, ".", "acc", Some(caller), false, serde_json::json!({"background":true}));
            let terminal = crate::pty::notice_terminal(caller, Provider::Claude, false);
            crate::background::idle_for_test(id, provider, 0, false);
            let path = std::env::temp_dir().join(format!("{id}-{}.jsonl", std::process::id()));
            let words = "Earlier result.";
            let line = match provider {
                Provider::Claude => serde_json::json!({
                    "type": "assistant", "message": {"content": [{"type": "text", "text": words}]}
                }),
                Provider::Codex => serde_json::json!({
                    "type": "response_item",
                    "payload": {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": words}]}
                }),
            };
            std::fs::write(&path, format!("{line}\n")).unwrap();
            crate::agents::read::transcripts().lock().unwrap().insert(id.into(), path.clone());
            let queued = crate::background::Queued::new(Some(caller.into()), "queued follow-up".into());
            for (baseline, text, queued, delivery) in
                [(0, "earlier prompt", false, None), (1, "queued follow-up", true, Some(queued.id.clone()))]
            {
                owe_sent(caller, id, baseline, 0.into(), None, &Asked { text, key: None, reply: true }, (queued, delivery, !queued));
            }
            crate::background::fail_next_launch_for_test(id, queued);
            tick_for_test(caller);
            let notice: String = terminal.input.try_iter().collect();
            assert!(notice.contains("Launch failed before the prompt opened") && notice.contains("Not delivered: 1 message"), "{notice}");
            assert!(!notice.contains(words), "{notice}");
            let unanswered = super::super::owed::requests_of(caller, id);
            assert_eq!(unanswered.iter().map(|q| q.n).collect::<Vec<_>>(), [1]);
            tick_for_test(caller);
            let earlier: String = terminal.input.try_iter().collect();
            assert!(earlier.contains(words) && !earlier.contains("opening its prompt"), "{earlier}");
            assert!(super::super::owed::requests_of(caller, id).is_empty());
            crate::background::gone_for_test(id);
            for s in [caller, id] {
                forget(s);
            }
            crate::activity::forget(caller);
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn ambiguous_enter_settles_restart_news_without_marking_or_blocking_the_next_notice() {
        let (caller, id, other) = ("ambiguous-caller", "ambiguous-helper", "ambiguous-other");
        let pty = crate::pty::manager();
        let _terminal = crate::pty::notice_terminal(caller, Provider::Claude, false);
        assert!(pty.composer(caller).is_none());
        register(caller, Provider::Claude, ".", "acc", None, true, serde_json::json!({}));
        for helper in [id, other] {
            register(helper, Provider::Claude, ".", "acc", Some(caller), false, serde_json::json!({}));
            crate::background::idle_for_test(helper, Provider::Claude, 0, false);
        }
        let restart = |helper: &str| {
            owe_from(caller, helper, 0, 0);
            let key = (caller.into(), helper.into());
            reg().lock().unwrap().owed.get_mut(&key).unwrap().resumed = true;
        };
        let tick = |expected: &str, result: bool| {
            let mut deliveries = 0;
            notify_once(
                pty,
                &mut HashMap::from([(caller.into(), NOTIFY_IDLE_LOOKS - 1)]),
                &mut HashMap::new(),
                |c| c == caller,
                |_, caller, notice| {
                    deliveries += 1;
                    assert!(notice.text.contains(expected), "{}", notice.text);
                    apply_delivery(pty, &caller, notice, Ok(Some(result)), 2, false);
                },
            );
            assert_eq!(deliveries, 1);
        };
        restart(id);
        tick(id, false);
        {
            let r = reg().lock().unwrap();
            assert!(!r.owed[&(caller.into(), id.into())].resumed);
            assert!(!r.notice_turns.contains_key(caller));
        }
        restart(other);
        tick(other, true);
        for helper in [id, other] {
            crate::background::gone_for_test(helper);
            forget(helper);
        }
        forget(caller);
        crate::activity::forget(caller);
    }

    #[test]
    fn a_queued_launch_loss_waits_for_publication_even_for_an_interrupt() {
        use crate::agents::test_support::stop_at;

        for (caller, id, provider, interrupt) in [
            ("publish-send-caller", "publish-send-helper", Provider::Codex, false),
            ("publish-interrupt-caller", "publish-interrupt-helper", Provider::Claude, true),
        ] {
            register(caller, Provider::Claude, ".", "acc", None, true, serde_json::json!({}));
            register(id, provider, ".", "acc", Some(caller), false, serde_json::json!({"background":true}));
            let terminal = crate::pty::notice_terminal(caller, Provider::Claude, false);
            crate::background::idle_for_test(id, provider, 0, false);
            crate::background::set_running_for_test(id);
            let stop = stop_at("send placed", caller);
            let sending = std::thread::spawn(move || crate::agents::send(caller, id, "Replace the work", interrupt, None, true));
            stop.wait();
            crate::background::fail_queued_launch_for_test(id);
            assert!(crate::background::undelivered_for(id, caller, true).is_empty());
            assert!(crate::background::take_undelivered_for(id, caller, true, None).is_empty());
            assert!(!crate::background::loss_recipients().contains(&(caller.into(), id.into())));
            tick_for_test(caller);
            assert!(terminal.input.try_recv().is_err());
            stop.go();
            sending.join().unwrap().unwrap();
            assert!(crate::background::loss_recipients().contains(&(caller.into(), id.into())));
            let lost = crate::background::undelivered_snapshot(id, caller, true);
            let requests = super::super::owed::requests_of(caller, id);
            assert_eq!(requests[0].delivery.as_deref(), Some(lost[0].0.as_str()));
            tick_for_test(caller);
            let text: String = terminal.input.try_iter().collect();
            assert!(text.contains("Not delivered: 1 message"), "{text}");
            assert!(super::super::owed::requests_of(caller, id).is_empty());
            tick_for_test(caller);
            assert!(terminal.input.try_recv().is_err());
            crate::background::gone_for_test(id);
            forget(id);
            forget(caller);
            crate::activity::forget(caller);
        }
    }

    #[test]
    fn a_loss_only_notice_keeps_the_text_until_its_report_can_be_written() {
        let (caller, id) = ("loss-report-caller", "loss-report-helper");
        register(caller, Provider::Claude, ".", "acc", None, true, serde_json::json!({}));
        register(id, Provider::Claude, ".", "acc", Some(caller), false, serde_json::json!({"background":true}));
        let terminal = crate::pty::notice_terminal(caller, Provider::Claude, false);
        crate::background::idle_for_test(id, Provider::Claude, 1, false);
        crate::background::lose_for_test(id, caller, "queued text");
        let payload = super::super::lost::undelivered_text(&["queued text".into()]);
        let path = super::super::said::note_file(id, &payload).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();

        tick_for_test(caller);
        std::fs::remove_dir(&path).unwrap();
        let fallback: String = terminal.input.try_iter().collect();
        assert!(fallback.contains("luna_message_list gives them"), "{fallback}");
        assert_eq!(crate::background::undelivered_for(id, caller, true), ["queued text"]);

        tick_for_test(caller);
        let notice: String = terminal.input.try_iter().collect();
        assert!(notice.contains(&path.display().to_string()), "{notice}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), payload);
        assert!(crate::background::undelivered_for(id, caller, true).is_empty());
        std::fs::remove_file(&path).unwrap();
        crate::background::gone_for_test(id);
        forget(id);
        forget(caller);
        crate::activity::forget(caller);
    }

    #[test]
    fn loss_only_notices_trip_the_breaker_and_leave_later_losses_owed() {
        let (caller, id) = ("loss-breaker-caller", "loss-breaker-helper");
        let pty = crate::pty::manager();
        let _terminal = crate::pty::notice_terminal(caller, Provider::Claude, false);
        register(caller, Provider::Claude, ".", "acc", None, true, serde_json::json!({}));
        register(id, Provider::Claude, ".", "acc", Some(caller), false, serde_json::json!({"background":true}));
        crate::background::idle_for_test(id, Provider::Claude, 0, false);
        let tick = || {
            let mut losses = 0;
            notify_once(
                pty,
                &mut HashMap::from([(caller.into(), NOTIFY_IDLE_LOOKS - 1)]),
                &mut HashMap::new(),
                |c| c == caller,
                |_, caller, notice| {
                    losses += notice.losses.len();
                    apply_delivery(pty, &caller, notice, Ok(Some(true)), 2, false);
                },
            );
            losses
        };
        for turn in 1..=BREAKER_NOTICES as u32 + 2 {
            crate::background::loss_for_test(id, turn, &[caller], &["delegate"]);
            let expected = usize::from(turn <= BREAKER_NOTICES as u32 + 1);
            assert_eq!(tick(), expected, "loss set {turn}");
            assert_eq!(tick(), 0, "the same set on the next tick");
        }
        assert!(pair_stopped(pty, caller, id));
        assert_eq!(crate::background::untold_losses(id, caller).len(), 1);
        crate::background::gone_for_test(id);
        forget(id);
        forget(caller);
        crate::activity::forget(caller);
    }

    #[test]
    fn an_answer_is_marked_unread_while_its_caller_is_away() {
        let (caller, id) = ("away-answer-caller", "away-answer-helper");
        let path = std::env::temp_dir().join(format!("luna-{id}-{}.jsonl", std::process::id()));
        let key = (caller.into(), id.into());
        let expire = || {
            let mut r = reg().lock().unwrap();
            let Registry { owed, known, .. } = &mut *r;
            prune_owed(owed, known, &HashMap::new(), CALLER_RETURN_MS, now_ms(), |other| other == caller);
        };
        for (expires_first, turn_ended) in [(false, true), (true, false), (true, true)] {
            for s in [caller, id] {
                register(s, Provider::Claude, ".", "acc", None, true, serde_json::json!({}));
            }
            owe_from(caller, id, 0, 0);
            reg().lock().unwrap().known.get_mut(caller).unwrap().waiting = true;
            let answer = serde_json::json!({
                "type": "assistant",
                "message": {"content": [{"type": "text", "text": "Here is the answer."}]}
            });
            std::fs::write(&path, answer.to_string()).unwrap();
            crate::agents::read::transcripts().lock().unwrap().insert(id.into(), path.clone());
            if expires_first {
                crate::background::idle_for_test(id, Provider::Claude, u32::from(turn_ended), false);
                if !turn_ended {
                    crate::background::set_running_for_test(id);
                }
                observe_unread(crate::pty::manager(), |c| c == caller);
                assert!(reg().lock().unwrap().owed[&key].wake.unread_since.is_none(), "the transcript line is incomplete");
                expire();
                let kept = reg().lock().unwrap().owed[&key];
                assert!(!kept.active() && crate::agents::teardown::unread_holds(&kept, now_ms()));
            }
            crate::background::idle_for_test(id, Provider::Claude, 1, false);
            std::fs::write(&path, format!("{answer}\n")).unwrap();
            crate::agents::read::transcripts().lock().unwrap().insert(id.into(), path.clone());
            tick_for_test(caller);
            expire();
            let kept = reg().lock().unwrap().owed[&key];
            assert!(!kept.active() && crate::agents::teardown::unread_holds(&kept, now_ms()));
            crate::background::gone_for_test(id);
            for s in [caller, id] {
                forget(s);
            }
            std::fs::remove_file(&path).unwrap();
        }
    }

    #[test]
    fn closing_a_chat_with_a_held_send_wakes_its_caller_and_keeps_unread_text() {
        let (caller, id) = ("held-closed-caller", "held-closed-chat");
        let _sweep = crate::agents::test_support::sweep_only(&[caller, id]);
        for chat in [caller, id] {
            register(chat, Provider::Codex, ".", "acc", None, true, serde_json::json!({}));
        }
        let terminal = crate::pty::notice_terminal(caller, Provider::Codex, false);
        owe_from(caller, id, 0, 0);
        let key = (caller.into(), id.into());
        let o = reg().lock().unwrap().owed[&key];
        settle_told(&mut reg().lock().unwrap(), key.clone(), Settled::Silent(1, true), &o);
        crate::agents::held::hold(caller, id, "A request that could not go in", None, true).unwrap();
        crate::agents::held::deliver_held(crate::pty::manager());
        tick_for_test(caller);
        let text: String = terminal.input.try_iter().collect();
        assert!(text.contains("exited before your held send went in"), "{text}");
        assert!(reg().lock().unwrap().owed[&key].wake.unread_since.is_some());
        tick_for_test(caller);
        assert!(terminal.input.try_recv().is_err());
        forget(id);
        forget(caller);
        crate::activity::forget(caller);
    }

    #[test]
    fn idle_cycles_deliver_once_to_each_chat_even_with_a_codex_draft() {
        for size in [2, 3] {
            let ids: Vec<_> = (0..size).map(|i| format!("delivery-cycle-{size}-{i}")).collect();
            let terminals: Vec<_> = ids
                .iter()
                .enumerate()
                .map(|(i, id)| {
                    register(id, Provider::Codex, ".", "acc", None, true, serde_json::json!({}));
                    crate::pty::notice_terminal(id, Provider::Codex, i + 1 == size)
                })
                .collect();
            for (i, id) in ids.iter().enumerate() {
                let peer = &ids[(i + 1) % size];
                let path = std::env::temp_dir().join(format!("luna-{peer}-{}.jsonl", std::process::id()));
                std::fs::write(
                    &path,
                    format!(
                        "{}\n",
                        serde_json::json!({
                            "type": "response_item", "payload": {"type":"message", "role":"assistant",
                            "content":[{"type":"output_text", "text":"Here is the result."}]}
                        })
                    ),
                )
                .unwrap();
                crate::agents::read::transcripts().lock().unwrap().insert(peer.clone(), path);
                owe_from(id, peer, 0, 0);
            }
            let pty = crate::pty::manager();
            let tick = || {
                let mut looks = ids.iter().map(|id| (id.clone(), NOTIFY_IDLE_LOOKS - 1)).collect();
                notify_once(
                    pty,
                    &mut looks,
                    &mut HashMap::new(),
                    |id| ids.iter().any(|s| s == id),
                    |_, caller, notice| deliver_notice(pty, &caller, notice),
                );
            };
            tick();
            for terminal in &terminals[..size - 1] {
                assert_eq!(terminal.input.try_iter().collect::<String>().matches(NOTICE_TAG).count(), 1);
            }
            assert!(terminals[size - 1].input.try_recv().is_err(), "the draft gets no keys");
            pty.set_draft(&ids[size - 1], 0);
            tick();
            assert_eq!(terminals[size - 1].input.try_iter().collect::<String>().matches(NOTICE_TAG).count(), 1);
            tick();
            for id in &ids {
                if let Some(path) = crate::agents::read::transcripts().lock().unwrap().get(id).cloned() {
                    std::fs::remove_file(path).unwrap();
                }
                forget(id);
                crate::activity::forget(id);
            }
            assert!(terminals.iter().all(|t| t.input.try_recv().is_err()), "one wake per answer");
        }
    }

    #[test]
    fn a_refusal_for_want_of_places_is_told_once_and_a_later_one_again() {
        let bg = serde_json::json!({ "background": true });
        register("pt-lead", Provider::Claude, "C:\\p", "C:\\acc\\one", Some("pt-top"), true, bg.clone());
        register("pt-other", Provider::Claude, "C:\\p", "C:\\acc\\one", Some("pt-lead"), true, bg);
        let refuse = |id: &str| crate::agents::spawn::refused_a_place(&mut reg().lock().unwrap(), id);
        let line = || reg().lock().unwrap().places_refused.get("pt-top").map(|p| p.line());
        refuse("pt-lead");
        let told = line().unwrap();
        assert!(told.contains("pt-lead"), "{told}");
        register("pt-top", Provider::Claude, "C:\\p", "C:\\acc\\one", None, true, serde_json::json!({}));
        // Another refused while that line went in: not what was told.
        refuse("pt-other");
        places_told(&mut reg().lock().unwrap(), "pt-top", std::slice::from_ref(&told));
        let next = line().expect("told next");
        assert!(next.contains("pt-lead, pt-other"), "{next}");
        assert!(!reg().lock().unwrap().places_refused.contains_key("pt-lead"));
        places_told(&mut reg().lock().unwrap(), "pt-other", std::slice::from_ref(&next));
        assert!(line().is_some(), "another chat's notice tells this one nothing");
        places_told(&mut reg().lock().unwrap(), "pt-top", &[next]);
        assert_eq!(line(), None, "told once");
        // A refusal after that is a new one.
        refuse("pt-lead");
        assert!(line().is_some());
        forget("pt-top");
        refuse("pt-other");
        assert!(!reg().lock().unwrap().places_refused.contains_key("pt-lead"), "the orphaned line has no chat");
        for id in ["pt-other", "pt-lead"] {
            forget(id);
        }
        assert_eq!(line(), None, "nothing left of a chat gone");
    }

    #[test]
    fn the_breaker_counts_turn_ends_and_not_prompts() {
        assert!(counts_for_breaker(&Settled::Answered));
        assert!(counts_for_breaker(&Settled::Pending(1, 0, None, None, false)));
        assert!(!counts_for_breaker(&Settled::Waiting), "a busy helper asks often, and holds until answered");
        assert!(!counts_for_breaker(&Settled::Asked(1)));
        assert!(!counts_for_breaker(&Settled::Exited));
    }

    #[test]
    fn a_queued_send_does_not_reset_the_current_turns_quiet_allowance() {
        use crate::agents::owed::{owe_sent, Asked};
        let (caller, id) = ("quiet-queue-caller", "quiet-queue-helper");
        let key = (caller.into(), id.into());
        register(id, Provider::Claude, "C:/p", "C:/acc/one", None, false, serde_json::json!({}));
        let first = Asked { text: "First task", key: None, reply: true };
        owe_sent(caller, id, 1, 0u64.into(), None, &first, (false, None, true));
        let o = reg().lock().unwrap().owed[&key];
        settle_told(&mut reg().lock().unwrap(), key.clone(), Settled::QuietFor(1, Some(1), false), &o);
        let later = Asked { text: "Queued task", key: None, reply: true };
        owe_sent(caller, id, 1, 40u64.into(), None, &later, (true, None, false));
        let current = reg().lock().unwrap().owed[&key];
        assert_eq!(current.told_quiet, Some(1));
        assert!(rides(&Settled::QuietFor(1, Some(1), true), current));
        assert!(!rides(&Settled::QuietFor(2, Some(2), false), current));
        forget(id);
        forget(caller);
    }

    #[test]
    fn a_new_notice_preserves_an_earlier_unread_report_restored_by_cancel() {
        let (caller, id) = ("unread-progress-caller", "unread-progress-helper");
        let key = (caller.into(), id.into());
        let mut r = Registry::default();
        let mut debt = Owed::from(0, 10);
        debt.wake.unread_since = Some(now_ms());
        debt.wake.unread_to = Some(20);
        debt.wake.reported_to = Some(30);
        r.owed.insert(key.clone(), debt);
        let how = Settled::Progress(Progress {
            ended: 1,
            cursor: 50,
            matched: None,
            requests: vec![],
            plan: true,
            work_left: true,
            job: false,
            pending: false,
        });
        let read = LineEvidence { read_to: Some(50), ended: 1, ..LineEvidence::default() };
        settle_delivered(&mut r, caller, id.into(), how, &debt, read, Some(1));
        let left = r.owed[&key];
        assert_eq!(left.cursor, 10, "a report read still starts at the unread answer");
        assert_eq!(left.notice_cursor(), 50, "the new notice's words already reached the caller");
        assert!(crate::agents::teardown::unread_holds(&left, now_ms()));
    }

    #[test]
    fn a_plan_remains_recorded_when_an_fyi_merges_during_delivery() {
        use crate::agents::owed::{owe_asked, requests_of, Asked};
        let (caller, id) = ("racing-plan-caller", "racing-plan-helper");
        let key = (caller.into(), id.into());
        owe_asked(caller, id, 0, 0, &Asked { text: "task", key: None, reply: true });
        let o = reg().lock().unwrap().owed[&key];
        let mut requests = requests_of(caller, id);
        requests[0].taken = true;
        let how = Settled::Progress(Progress {
            ended: 1,
            cursor: 50,
            matched: Some(1),
            requests,
            plan: true,
            work_left: true,
            job: true,
            pending: true,
        });
        owe_asked(caller, id, 1, 50, &Asked { text: "FYI", key: None, reply: false });
        settle_told(&mut reg().lock().unwrap(), key.clone(), how, &o);
        assert_eq!(reg().lock().unwrap().owed[&key].wake.plan_through, Some(1));
        let requests = requests_of(caller, id);
        assert!(requests[0].taken && !requests[1].taken);
        forget(id);
        forget(caller);
    }

    #[test]
    fn a_stopped_pair_can_hear_its_first_failure_only() {
        let (caller, id) = ("first-failure-caller", "first-failure-helper");
        let key = (caller.into(), id.into());
        let pty = crate::pty::manager();
        assert!(counts_for_breaker(&Settled::Failed));
        reg().lock().unwrap().stopped.insert(key.clone(), now_ms());
        assert!(!breaker_blocks(pty, caller, id, &Settled::Failed));
        assert!(breaker_blocks(pty, caller, id, &Settled::Answered));
        reg().lock().unwrap().failures_told.insert(key.clone(), now_ms());
        assert!(breaker_blocks(pty, caller, id, &Settled::Failed));
        reg().lock().unwrap().failures_told.insert(key, now_ms().saturating_sub(BREAKER_WINDOW_MS));
        assert!(breaker_blocks(pty, caller, id, &Settled::Failed));
        assert!(!breaker_blocks(pty, caller, id, &Settled::Waiting));
        forget(caller);
        forget(id);
    }

    #[test]
    fn a_breaker_trip_under_a_lead_is_queued_only_for_its_top_chat() {
        let bg = serde_json::json!({ "background": true });
        let account = "C:/acc/anthropic/one";
        let (top, lead, id) = ("bt-top", "bt-inner", "bt-helper");
        let _sweep = crate::agents::test_support::sweep_only(&[top, lead, id, "bt-lead"]);
        register("bt-lead", Provider::Claude, "C:/p", account, Some("bt-top"), true, bg.clone());
        register(lead, Provider::Claude, "C:/p", account, Some("bt-lead"), true, bg.clone());
        register(id, Provider::Claude, "C:/p", account, Some(lead), false, bg);
        crate::agents::registry::set_name(id, "Check the gate");
        crate::background::test_helper(lead, account, None, true);
        // Other notifier tests can sample the registry before this test
        // delivers. Put the debt in before leaving the lead listening.
        owe_from(lead, id, 0, 0);
        let lines = crate::background::listening_for_test(lead);
        crate::background::idle_for_test(id, Provider::Claude, 1, false);
        let now = now_ms();
        assert!(breaker_suffix(lead, id, &Settled::Answered, now).is_none());
        {
            let mut r = reg().lock().unwrap();
            for _ in 1..BREAKER_NOTICES {
                assert!(!count_notice(&mut r, lead, id, now));
            }
        }
        assert!(breaker_suffix(lead, id, &Settled::Waiting, now).is_none());
        let suffix = breaker_suffix(lead, id, &Settled::Answered, now).unwrap();
        assert_eq!(suffix, " [turn notices stopped; luna_report_get]");
        let owed = reg().lock().unwrap().owed[&(lead.to_string(), id.to_string())];
        let text = format!("{NOTICE_TAG} {id} answered.{suffix}");
        let pty = crate::pty::manager();
        deliver_notice(pty, lead, Notice { text, settled: vec![(id.into(), Settled::Answered, owed)], ..Notice::default() });
        assert!(lines.try_recv().unwrap().contains("turn notices stopped"));
        assert!(breaker_suffix(lead, id, &Settled::Answered, now).is_none(), "the trip has happened");
        let saved = crate::agents::persist::chats_now();
        assert!(!saved.system_news.iter().any(|n| n.about(id)), "the breaker resets on restart");
        assert!(!system::news(pty).contains_key(top), "the top has yet to register");
        register(top, Provider::Claude, "C:/p", account, None, true, serde_json::json!({}));
        let news = system::news(pty);
        assert!(news[top][0].1.contains("Check the gate"));
        assert!(!news.contains_key(lead));
        {
            let mut r = reg().lock().unwrap();
            let tripped = r.stopped[&(lead.into(), id.into())];
            system::breaker(&mut r, lead, id, tripped);
        }
        assert_eq!(system::news(pty)[top].len(), 1, "the same trip is queued once");
        let terminal = crate::pty::notice_terminal(top, Provider::Claude, false);
        let mut idle_looks = HashMap::from([(top.to_string(), NOTIFY_IDLE_LOOKS - 1)]);
        let mut drafted = None;
        notify_once(
            pty,
            &mut idle_looks,
            &mut HashMap::new(),
            |caller| caller == top,
            |_typing, caller, notice| {
                if caller == top {
                    drafted = Some(notice);
                }
            },
        );
        let notice = drafted.expect("the notifier delivers system news even to a chat owed nothing");
        assert!(notice.text.contains("Check the gate"));
        assert_eq!(notice.system_news.len(), 1);
        deliver_notice(pty, top, notice);
        assert!(terminal.input.try_iter().collect::<String>().contains("Check the gate"));
        assert!(!system::news(pty).contains_key(top), "a delivered breaker is removed");
        drop(terminal);
        crate::activity::forget(top);
        forget(top);
        system::breaker(&mut reg().lock().unwrap(), lead, id, now);
        assert!(!reg().lock().unwrap().system_news.iter().any(|n| n.about(id)));
        for session in [id, lead, "bt-lead"] {
            crate::background::gone_for_test(session);
            forget(session);
        }
    }

    #[test]
    fn system_news_built_before_a_handover_goes_to_the_new_top_chat() {
        let (chat, lead, helper) = ("ns-old-top", "ns-new-top", "ns-helper");
        let _sweep = crate::agents::test_support::sweep_only(&[chat, lead, helper]);
        let account = "C:/acc/anthropic/one";
        register(chat, Provider::Claude, "C:/p", account, None, true, serde_json::json!({}));
        let bg = serde_json::json!({ "background": true });
        register(lead, Provider::Claude, "C:/p", account, Some(chat), true, bg.clone());
        register(helper, Provider::Claude, "C:/p", account, Some(lead), false, bg);
        crate::agents::turn_folder_skills(lead, Some("access denied"));
        crate::agents::turn_folder_skills(helper, Some("disk full"));
        let pty = crate::pty::manager();
        let pending = system::news(pty).remove(chat).unwrap();
        let (entry, line) = pending.into_iter().find(|(entry, _)| entry.about(helper)).unwrap();
        crate::agents::take_over(lead, || Some(Vec::new())).unwrap();
        assert!(!reg().lock().unwrap().system_news.iter().any(|n| n.about(lead)));
        register(lead, Provider::Claude, "C:/p", account, None, true, serde_json::json!({}));
        crate::background::test_helper(chat, account, None, true);
        let lines = crate::background::listening_for_test(chat);
        deliver_notice(pty, chat, Notice { text: line, system_news: vec![entry], ..Notice::default() });
        assert!(lines.try_recv().is_err(), "the stale notice never goes into the former top chat");
        assert!(system::news(pty)[lead][0].1.contains("disk full"));
        crate::background::gone_for_test(chat);
        reg().lock().unwrap().news.retain(|(to, _)| to != chat);
        for id in [helper, lead, chat] {
            forget(id);
        }
    }

    /// A notice put together before a handover and going in after it, the gap
    /// between the notifier's look and its delivery thread: the debt it told of
    /// went with the handover, so nothing of it goes in. One whose debt stands
    /// goes in as ever. Both providers' helpers alike.
    #[test]
    fn a_notice_on_its_way_through_a_handover_does_not_go_in() {
        for (provider, tag) in [(Provider::Claude, "cl"), (Provider::Codex, "cx")] {
            let (a, id, other) = (format!("nh-{tag}-caller"), format!("nh-{tag}-helper"), format!("nh-{tag}-other"));
            let bg = serde_json::json!({ "background": true });
            register(&a, Provider::Claude, "C:\\p", "C:\\acc\\one", None, true, serde_json::json!({}));
            register(&id, provider, "C:\\p", "C:\\acc\\one", Some(&a), false, bg.clone());
            register(&other, provider, "C:\\p", "C:\\acc\\one", Some(&a), false, bg);
            // The caller mid-turn: a notice for it queues, to be seen.
            crate::background::test_helper(&a, "C:\\acc", None, true);
            crate::background::idle_for_test(&id, provider, 1, false);
            let looked_up = |helper: &str| {
                owe_from(&a, helper, 0, 0);
                reg().lock().unwrap().owed[&(a.clone(), helper.to_string())]
            };
            let told = looked_up(&id);
            assert!(!debts_moved(&a, &[(id.clone(), Settled::Answered, told)]), "as the notifier found it");
            crate::agents::take_over(&id, || Some(Vec::new())).unwrap();
            let pty = crate::pty::manager();
            let notice = |helper: &str, o: Owed| {
                let text = format!("{NOTICE_TAG} {helper} answered");
                deliver_notice(pty, &a, Notice { text, settled: vec![(helper.to_string(), Settled::Answered, o)], ..Notice::default() });
            };
            notice(&id, told);
            assert!(crate::background::queued_for_test(&a).is_empty(), "the handover's notice went nowhere");
            let other_told = looked_up(&other);
            notice(&other, other_told);
            assert_eq!(crate::background::queued_for_test(&a), [format!("{NOTICE_TAG} {other} answered")]);
            reg().lock().unwrap().news.retain(|(c, _)| *c != a);
            for s in [&id, &other, &a] {
                crate::background::gone_for_test(s);
                forget(s);
            }
        }
    }

    /// Issue #73: a lead whose CLI is kept up by its own background subagents
    /// past its answer hears of its helper then, on stdin, not once they end.
    /// The notice is answered within the lead's running turn, which is not
    /// marked as one on a notice: its parent hears of that turn as ever.
    #[test]
    fn a_lead_kept_up_by_its_own_background_work_hears_of_its_helper_on_stdin() {
        let (a, id) = ("ln-stdin-lead", "ln-stdin-helper");
        register(a, Provider::Claude, "C:\\p", "C:\\acc\\one", None, true, serde_json::json!({ "background": true }));
        register(id, Provider::Claude, "C:\\p", "C:\\acc\\one", Some(a), false, serde_json::json!({ "background": true }));
        crate::background::test_helper(a, "C:\\acc", None, true);
        let lines = crate::background::listening_for_test(a);
        crate::background::idle_for_test(id, Provider::Claude, 1, false);
        assert!(crate::background::takes_notices(a));
        owe_from(a, id, 0, 0);
        let key = (a.to_string(), id.to_string());
        let told = reg().lock().unwrap().owed[&key];
        let text = format!("{NOTICE_TAG} {id} answered");
        deliver_notice(
            crate::pty::manager(),
            a,
            Notice { text: text.clone(), settled: vec![(id.to_string(), Settled::Answered, told)], ..Notice::default() },
        );
        let line: serde_json::Value = serde_json::from_str(&lines.try_recv().expect("on its stdin")).unwrap();
        assert_eq!(line["message"]["content"].as_str(), Some(text.as_str()));
        assert!(crate::background::queued_for_test(a).is_empty(), "not held for its next process");
        assert!(!reg().lock().unwrap().owed.contains_key(&key), "told");
        assert!(!reg().lock().unwrap().notice_turns.contains_key(a), "no turn of its marked as one on a notice");
        assert!(!crate::background::takes_notices(a), "at work on it");
        for s in [id, a] {
            crate::background::gone_for_test(s);
            forget(s);
        }
    }

    /// Issue #73, review: the lead's CLI takes up a task's end in a turn of
    /// its own between the notifier's look and the notice going in. The notice
    /// waits for that turn, and so does a send behind it; both go onto stdin
    /// at the turn's `result`, in the order they came, and not behind the
    /// process for as long as its other tasks run. No turn is marked as one
    /// on the notice: it is answered within this process.
    #[test]
    fn a_notice_that_finds_the_leads_cli_at_a_turn_of_its_own_goes_in_at_its_result() {
        let (a, id) = ("lq-lead", "lq-helper");
        register(a, Provider::Claude, "C:\\p", "C:\\acc\\one", None, true, serde_json::json!({ "background": true }));
        register(id, Provider::Claude, "C:\\p", "C:\\acc\\one", Some(a), false, serde_json::json!({ "background": true }));
        crate::background::test_helper(a, "C:\\acc", None, true);
        let lines = crate::background::listening_for_test(a);
        crate::background::idle_for_test(id, Provider::Claude, 1, false);
        assert!(crate::background::takes_notices(a));
        owe_from(a, id, 0, 0);
        let key = (a.to_string(), id.to_string());
        let told = reg().lock().unwrap().owed[&key];
        let text = format!("{NOTICE_TAG} {id} answered");
        run_between(
            a,
            Box::new(move || {
                crate::background::claude_event_for_test(a, &serde_json::json!({ "type": "system", "subtype": "task_notification" }));
            }),
        );
        deliver_notice(
            crate::pty::manager(),
            a,
            Notice { text: text.clone(), settled: vec![(id.to_string(), Settled::Answered, told)], ..Notice::default() },
        );
        assert!(lines.try_recv().is_err(), "not into the CLI's own turn");
        assert_eq!(crate::background::queued_for_test(a), std::slice::from_ref(&text));
        assert!(!reg().lock().unwrap().notice_turns.contains_key(a), "no later turn of its marked as one on a notice");
        // Its parent's send comes behind the notice, not past it.
        assert_eq!(crate::background::send(a, "from its parent", None).unwrap(), crate::background::Placed::Queued);

        crate::background::claude_event_for_test(
            a,
            &serde_json::json!({ "type": "assistant", "message": { "model": "claude-haiku", "content": [] }, "parent_tool_use_id": null }),
        );
        crate::background::claude_event_for_test(a, &serde_json::json!({ "type": "result", "subtype": "success", "duration_ms": 10 }));
        let content =
            |line: String| serde_json::from_str::<serde_json::Value>(&line).unwrap()["message"]["content"].as_str().unwrap().to_string();
        assert_eq!(content(lines.try_recv().expect("the notice, on its stdin")), text);
        assert_eq!(content(lines.try_recv().expect("then the send")), "from its parent");
        assert!(crate::background::queued_for_test(a).is_empty());
        assert!(!crate::background::takes_notices(a), "at work on them");
        for s in [id, a] {
            crate::background::gone_for_test(s);
            forget(s);
        }
    }

    /// Review of issue #73: a parent's send queued behind a notice for a lead
    /// whose CLI is at a turn of its own goes onto its stdin with the notice
    /// at that turn's `result`, and is
    /// answered within that process. Its debt, put past the process as the
    /// text queued, is that process's from then: once it ends, the answer is
    /// told, with no process after it. Through the send itself, with its debt
    /// in before the move, with the move between the text queuing and its
    /// debt going in, and with the line taken in, answered and the process
    /// over in between as well (review of 0.18.4).
    #[test]
    fn a_send_moved_from_a_leads_queue_onto_its_stdin_is_owed_by_its_process() {
        use crate::agents::notifier::lines::{verdict, Verdict};
        use crate::agents::test_support::stop_at;
        let event = |id: &str, v: serde_json::Value| crate::background::claude_event_for_test(id, &v);
        for (race, over) in [(false, false), (true, false), (true, true)] {
            let (p, a) = match (race, over) {
                (false, _) => ("lm-parent", "lm-lead"),
                (true, false) => ("lmr-parent", "lmr-lead"),
                (true, true) => ("lmo-parent", "lmo-lead"),
            };
            register(p, Provider::Claude, "C:\\p", "C:\\acc\\one", None, true, serde_json::json!({}));
            register(a, Provider::Claude, "C:\\p", "C:\\acc\\one", Some(p), true, serde_json::json!({ "background": true }));
            crate::background::test_helper(a, "C:\\acc", None, true);
            let lines = crate::background::listening_for_test(a);
            // Its CLI takes up a task's end in a turn of its own, and a notice
            // that finds it so waits for that turn's end; the send behind it.
            event(a, serde_json::json!({ "type": "system", "subtype": "task_notification" }));
            let notice = crate::background::send_notice(a, "[Luna notice] a helper answered").unwrap();
            assert_eq!(notice, (crate::background::Placed::Queued, true));
            let ended = crate::activity::turn_state(a).unwrap().turns_ended;
            let at_result = || {
                event(
                    a,
                    serde_json::json!({ "type": "assistant", "message": { "model": "claude-haiku", "content": [] }, "parent_tool_use_id": null }),
                );
                event(a, serde_json::json!({ "type": "result", "subtype": "success", "duration_ms": 10 }));
            };
            let fed = || {
                let notice = lines.try_recv().expect("the notice, on its stdin");
                assert!(notice.contains("a helper answered"), "{notice}");
                let line = lines.try_recv().expect("then the send");
                assert!(line.contains("the build is green"), "{line}");
                [notice, line]
            };
            let key = (p.to_string(), a.to_string());
            if race {
                let stop = stop_at("send placed", p);
                let sending = std::thread::spawn(move || crate::agents::send(p, a, "the build is green", false, None, true));
                stop.wait();
                // Moved before its debt is in.
                at_result();
                let lines = fed();
                if over {
                    // And taken in and answered, its tasks over and the
                    // process gone, all before the debt is in.
                    for line in lines {
                        let line: serde_json::Value = serde_json::from_str(&line).unwrap();
                        event(a, serde_json::json!({ "type": "user", "isReplay": true, "uuid": line["uuid"] }));
                    }
                    crate::background::turn_over_for_test(a, Some(0));
                }
                stop.go();
                let sent = sending.join().unwrap().unwrap();
                assert!(sent.queued && sent.turns_ended == ended, "the count it is answered past: this process's");
            } else {
                let sent = crate::agents::send(p, a, "the build is green", false, None, true).unwrap();
                assert!(sent.queued && !sent.joined);
                assert_eq!(reg().lock().unwrap().owed[&key].baseline, ended + 1, "past this process, as queued");
                at_result();
                fed();
            }
            let o = reg().lock().unwrap().owed[&key];
            assert_eq!(o.baseline, ended, "race {race}, over {over}: owed by this process");
            assert_eq!(o.moved_from, Some(ended + 1), "for a wait going by the count the send returned first");
            // It takes the line in and answers; its tasks over, the process ends.
            if !over {
                crate::background::test_turn_ended(a);
            }
            let ts = crate::activity::turn_state(a).unwrap();
            assert_eq!(ts.turns_ended, ended + 1, "no process after it");
            assert_eq!(verdict(&o, ts.turns_ended, Some(ts.turn), false, true), Verdict::TurnOver, "race {race}, over {over}");
            reg().lock().unwrap().owed.remove(&key);
            crate::background::gone_for_test(a);
            for s in [a, p] {
                forget(s);
            }
        }
    }

    /// Live on 13 November: a chat sent `expectReply: false` a second before
    /// the notice of its helper's last answer had settled that debt. The send
    /// took the debt in, and the helper's "ok" came back as "answering your
    /// request 1, request 2", the first answer told again. Once the notice
    /// settles, what is left is the send's alone, and it asked for no reply.
    #[test]
    fn a_no_reply_send_crossing_a_notice_in_flight_wants_no_reply_and_repeats_nothing() {
        use crate::agents::owed::{owe_asked, requests_of, Asked};
        let (a, id) = ("nx-caller", "nx-helper");
        register(a, Provider::Claude, "C:\\p", "C:\\acc\\one", None, true, serde_json::json!({}));
        register(id, Provider::Claude, "C:\\p", "C:\\acc\\one", Some(a), false, serde_json::json!({ "background": true }));
        // The caller mid-turn: a notice for it queues, as one going in.
        crate::background::test_helper(a, "C:\\acc", None, true);
        crate::background::idle_for_test(id, Provider::Claude, 1, false);
        let prompt = |t: &str| format!("{}\n", serde_json::json!({ "type": "user", "message": { "role": "user", "content": t } }));
        let said = |t: &str| {
            format!("{}\n", serde_json::json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": t }] } }))
        };
        let first = format!("{}{}", prompt("Review PR 7719"), said("VERDICT: GO for 7719."));
        let path = std::env::temp_dir().join(format!("luna-{id}-{}.jsonl", std::process::id()));
        std::fs::write(&path, &first).unwrap();
        crate::agents::read::transcripts().lock().unwrap().insert(id.into(), path.clone());
        owe_asked(a, id, 0, 0, &Asked { text: "Review PR 7719", key: None, reply: true });
        let key = (a.to_string(), id.to_string());
        let told = reg().lock().unwrap().owed[&key];
        let k = reg().lock().unwrap().known.get(id).cloned().unwrap();
        let pty = crate::pty::manager();
        let Some((Some(line), Settled::Answered)) = notice_line(pty, a, "Your helper", id, &k, told) else { panic!("answered") };
        // The send comes in as the notice goes in, before it settles.
        let end = first.len() as u64;
        run_between(a, Box::new(move || owe_asked(a, id, 1, end, &Asked { text: "FYI: main moved", key: None, reply: false })));
        let text = format!("{NOTICE_TAG} {line}");
        deliver_notice(
            pty,
            a,
            Notice { text: text.clone(), settled: vec![(id.to_string(), Settled::Answered, told)], ..Notice::default() },
        );
        assert_eq!(crate::background::queued_for_test(a), [text], "the notice went in");
        let left = reg().lock().unwrap().owed[&key];
        assert!(left.no_reply && left.cursor == end && left.baseline == 1, "the send's own debt is what is left");
        assert_eq!(requests_of(a, id).iter().map(|q| q.n).collect::<Vec<_>>(), [2]);
        // The helper's "ok" to it: no notice, and the first answer not again.
        std::fs::write(&path, format!("{first}{}{}", prompt("FYI: main moved"), said("ok."))).unwrap();
        crate::background::idle_for_test(id, Provider::Claude, 2, false);
        assert!(matches!(notice_line(pty, a, "Your helper", id, &k, left), Some((None, Settled::Silent(..)))));
        reg().lock().unwrap().owed.remove(&key);
        for s in [id, a] {
            crate::background::gone_for_test(s);
            forget(s);
        }
        let _ = std::fs::remove_file(&path);
    }

    /// The same race a debt later: two requests merged before the notice
    /// was put together, so it tells of their debt, not the first one's. A
    /// third, wanting no reply, crosses it in flight; once it settles, what is
    /// left is the third's alone, from where it was asked and past the turns
    /// counted as it was, with what was decided of it as it came (`queued`).
    #[test]
    fn a_no_reply_send_crossing_a_notice_of_two_merged_requests_repeats_neither() {
        use crate::agents::owed::{owe_asked, owe_sent, requests_of, Asked};
        let (a, id) = ("nx2-caller", "nx2-helper");
        register(a, Provider::Claude, "C:\\p", "C:\\acc\\one", None, true, serde_json::json!({}));
        register(id, Provider::Claude, "C:\\p", "C:\\acc\\one", Some(a), false, serde_json::json!({ "background": true }));
        crate::background::test_helper(a, "C:\\acc", None, true);
        crate::background::idle_for_test(id, Provider::Claude, 2, false);
        let prompt = |t: &str| format!("{}\n", serde_json::json!({ "type": "user", "message": { "role": "user", "content": t } }));
        let said = |t: &str| {
            format!("{}\n", serde_json::json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": t }] } }))
        };
        let first = format!("{}{}{}{}", prompt("Review PR 7719"), said("Reviewing."), prompt("And 7720"), said("VERDICT: GO for both."));
        let path = std::env::temp_dir().join(format!("luna-{id}-{}.jsonl", std::process::id()));
        std::fs::write(&path, &first).unwrap();
        crate::agents::read::transcripts().lock().unwrap().insert(id.into(), path.clone());
        owe_asked(a, id, 0, 0, &Asked { text: "Review PR 7719", key: None, reply: true });
        owe_asked(a, id, 1, 40, &Asked { text: "And 7720", key: None, reply: true });
        let key = (a.to_string(), id.to_string());
        let told = reg().lock().unwrap().owed[&key];
        let k = reg().lock().unwrap().known.get(id).cloned().unwrap();
        let pty = crate::pty::manager();
        let Some((Some(line), Settled::Answered)) = notice_line(pty, a, "Your helper", id, &k, told) else { panic!("answered") };
        let end = first.len() as u64;
        run_between(
            a,
            Box::new(move || {
                owe_sent(a, id, 2, end.into(), None, &Asked { text: "FYI: main moved", key: None, reply: false }, (true, None, false));
            }),
        );
        let text = format!("{NOTICE_TAG} {line}");
        deliver_notice(
            pty,
            a,
            Notice { text: text.clone(), settled: vec![(id.to_string(), Settled::Answered, told)], ..Notice::default() },
        );
        let left = reg().lock().unwrap().owed[&key];
        assert!(left.no_reply && left.cursor == end && left.baseline == 2, "the third send's own debt is what is left");
        assert!(left.queued, "what was decided of it holds");
        assert_eq!(requests_of(a, id).iter().map(|q| q.n).collect::<Vec<_>>(), [3]);
        // Its turn begun, the queue's grace is behind it.
        let left = Owed { queued: false, ..left };
        reg().lock().unwrap().owed.insert(key.clone(), left);
        std::fs::write(&path, format!("{first}{}{}", prompt("FYI: main moved"), said("ok."))).unwrap();
        crate::background::idle_for_test(id, Provider::Claude, 3, false);
        assert!(matches!(notice_line(pty, a, "Your helper", id, &k, left), Some((None, Settled::Silent(..)))));
        reg().lock().unwrap().owed.remove(&key);
        for s in [id, a] {
            crate::background::gone_for_test(s);
            forget(s);
        }
        let _ = std::fs::remove_file(&path);
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

    /// A helper whose CLI cut its background work off, owed by two callers:
    /// each is to hear of it, and a line that does not say so (an exited
    /// helper's) acknowledges nothing.
    #[test]
    fn a_cut_off_is_told_to_every_caller_owed_and_only_by_a_line_that_says_so() {
        let id = "ack-s";
        let (a, b) = ("ack-a", "ack-b");
        crate::background::idle_for_test(id, Provider::Claude, 3, false);
        crate::background::background_stopped_for_test(id, &[a, b]);
        let key = |c: &str| (c.to_string(), id.to_string());
        let (oa, ob) = (Owed::from(2, 0), Owed::from(2, 0));
        reg().lock().unwrap().owed.insert(key(a), oa);
        reg().lock().unwrap().owed.insert(key(b), ob);
        let settle = |caller: &str, how: Settled, told: &Owed, stopped| {
            let read = LineEvidence { stopped, ..LineEvidence::default() };
            settle_delivered(&mut reg().lock().unwrap(), caller, id.to_string(), how, told, read, None);
        };
        settle(a, Settled::Exited, &oa, vec![]);
        assert!(crate::background::background_stopped_for(id, a), "an exited line did not say it");
        let oa = Owed::from(2, 0);
        reg().lock().unwrap().owed.insert(key(a), oa);
        settle(a, Settled::Answered, &oa, vec![3]);
        assert!(!crate::background::background_stopped_for(id, a), "the first has had it");
        assert!(crate::background::background_stopped_for(id, b), "the other, still owed, has not");
        settle(b, Settled::Answered, &ob, vec![3]);
        assert!(!crate::background::background_stopped(id), "everyone owed has had it");
        crate::background::gone_for_test(id);
    }

    #[test]
    fn a_loss_that_appears_after_rendering_is_not_acknowledged() {
        let (caller, id) = ("rendered-loss-caller", "rendered-loss-helper");
        register(caller, Provider::Codex, ".", "acc", None, true, serde_json::json!({}));
        register(id, Provider::Claude, ".", "acc", Some(caller), false, serde_json::json!({"background":true}));
        let terminal = crate::pty::notice_terminal(caller, Provider::Codex, false);
        crate::background::idle_for_test(id, Provider::Claude, 1, false);
        crate::background::lose_for_test(id, caller, "first queued text");
        crate::background::loss_for_test(id, 0, &[caller], &[]);
        owe_from(caller, id, 0, 0);
        reg().lock().unwrap().owed.get_mut(&(caller.into(), id.into())).unwrap().held = Some((1, 0));
        let pty = crate::pty::manager();
        let rendered = super::super::test_support::stop_at("notice rendered", caller);
        std::thread::scope(|scope| {
            scope.spawn(move || {
                rendered.wait();
                crate::background::lose_for_test(id, caller, "second queued text");
                crate::background::loss_for_test(id, 1, &[caller], &[]);
                rendered.go();
            });
            notify_once(
                pty,
                &mut HashMap::from([(caller.into(), NOTIFY_IDLE_LOOKS - 1)]),
                &mut HashMap::new(),
                |c| c == caller,
                |_, caller, notice| {
                    assert!(notice.text.contains("Not delivered: 1 message"), "{}", notice.text);
                    deliver_notice(pty, &caller, notice);
                },
            );
        });
        let first: String = terminal.input.try_iter().collect();
        assert!(first.contains("Not delivered: 1 message"), "{first}");
        assert!(!reg().lock().unwrap().owed.contains_key(&(caller.into(), id.into())));
        assert_eq!(crate::background::undelivered_for(id, caller, true), ["second queued text"]);
        assert_eq!(crate::background::untold_losses(id, caller).iter().map(|l| l.turn).collect::<Vec<_>>(), [1]);
        tick_for_test(caller);
        let second: String = terminal.input.try_iter().collect();
        assert!(second.contains("Not delivered: 1 message"), "{second}");
        assert!(crate::background::undelivered_for(id, caller, true).is_empty());
        assert!(crate::background::untold_losses(id, caller).is_empty());
        tick_for_test(caller);
        assert!(terminal.input.try_recv().is_err(), "each loss was delivered once");
        crate::background::gone_for_test(id);
        for s in [caller, id] {
            forget(s);
        }
        crate::activity::forget(caller);
    }
}
