//! Sessions run by sessions. A chat started with Luna's MCP server attached
//! can spawn other chats — another model, another account — send them text,
//! read what they answered, wait for them, and kill or delete them, worktree
//! included. This module is the registry behind those tools: who is who,
//! who may touch whom, and the round trips to the UI that owns the chat list.
//!
//! The core does not know chats, only pty sessions (see ARCHITECTURE.md), so
//! a spawn is a request to the frontend — `agent://spawn` out, `agent_spawned`
//! back — and a delete tells the frontend to drop the row after the session
//! and its worktree are gone. Lineage lives here: every session registered
//! with a parent is that parent's, and a caller may only reach its own
//! descendants — except to list every session, type a message into one and
//! read or wait on its answer, which is how sessions talk to each other; only
//! killing and deleting stay with the lineage. Children outlive their parent:
//! they pass to its parent, or with none stand alone, the window's to manage
//! (see `forget`).
//!
//! A helper does not report back on its own the way a subagent does: its
//! answer sits in its own transcript until someone waits for it or reads it.
//! A caller that asked something (spawn, send) and ended its turn without
//! collecting the answer would sit idle beside a helper that finished long
//! ago, so the notifier here types a one-line notice into the caller once it
//! is idle: the helper answered, stopped on a prompt, or exited.

mod answer;
mod ask;
mod delivery;
mod list;
mod lost;
mod notifier;
mod owed;
mod persist;
pub(crate) mod privilege;
mod read;
mod registry;
mod resume;
mod said;
pub(crate) mod spawn;
mod teardown;
mod wait;

pub use answer::answer;
pub use ask::{on_hook, resolve_option, state_of, Ask, AskQuestion};
pub use delivery::{held_count, notes_dir, send};
pub use list::{list, sessions, SessionFilter};
pub use lost::undelivered_left;
#[cfg(test)]
pub(crate) use notifier::riders_go;
pub use notifier::{notify, NOTICE_TAG};
pub use owed::{resumed, retried};
pub use persist::{conversation_of, freeze_chats, restore_chats, save_chats, take_up_spawns, tie_back, ties_of, Ties};
pub(crate) use privilege::{command_prefix, command_reach_now};
pub use read::{read, Message};
pub(crate) use registry::mint;
pub use registry::{
    caller_of, chat_name, chat_names, has_tools, label, parent_of, parent_on_start, promoted, register, register_dev_caller,
    resumes_on_restart, session_exited, settings_of, DEV_CALLER,
};
pub use resume::agent_resumes;
pub use spawn::account::account_usage;
pub use spawn::worktree::{helper_worktree_name, name_to_go_on, report_codex_worktrees, worktree_for_start};
pub use spawn::{agent_spawned, agent_spawns, chat_starting, spawn, spawn_is, SpawnParams};
#[cfg(test)]
pub use teardown::news_for;
pub use teardown::{delete, delete_by_user, forget, kill, queued_refused};
pub use wait::{wait, wait_many};

use crate::background::MarkStands;
use crate::provider::Provider;
use crate::throttle::now_ms;
use ask::{ask_of, live};
use delivery::held_sender;
use lost::{first_turn_lost, never_started_what, sentence, undelivered_for, undelivered_line, undelivered_text};
use notifier::{left_quiet, notifier, Settled, IDLE_JOB_MS};
use owed::{Owed, QUEUED_GRACE_MS};
use persist::{core_up_ms, returning};
use read::{failure_of, transcript_end, transcript_found, transcript_path};
use registry::{is_descendant, reg, waits_on_helpers, Known, Registry};
use resume::resume_pending;
use said::{answer_file, answer_text, answer_where, begins, note_file, replies_since, said_since, SHORT_MESSAGE};
use spawn::worktree::leave_half_made;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use wait::REPLY_GRACE_MS;

/// A turn has ended with the session's processes still at work: give them
/// this long to settle before calling it a job left in the background. Right
/// after a turn they often move for a sample or two anyway.
const BACKGROUND_GRACE_MS: u64 = 20_000;

/// The most turn-end notices one caller gets about one session within
/// BREAKER_WINDOW_MS before Luna stops telling it: each costs a turn of the
/// caller, and two chats that keep answering each other's notices would go on
/// until a person came back. Real use never came near it (at most 3 in half an
/// hour per pair, luna.log 10-28 to 11-01); the loop of that night ran one a
/// minute per pair, for 13 hours.
const BREAKER_NOTICES: usize = 12;

const BREAKER_WINDOW_MS: u64 = 15 * 60 * 1000;

/// The notice turns kept per session (see `NoticeTurn`): the debts that ask
/// about them are never many turns behind.
const NOTICE_TURNS_KEPT: usize = 64;

/// The percentages of a usage window a caller hears of, once each per window.
const QUOTA_LEVELS: [u8; 2] = [90, 95];

/// A window this far below the level it was told at has reset, whatever its
/// reset time says; a point or two is rounding.
const QUOTA_SLACK: f64 = 5.0;

/// A turn a Luna notice started. Told a session is not done yet, a caller is
/// not told again of a turn that only answers a notice: two chats with a job
/// each would wake each other for as long as the jobs run.
struct NoticeTurn {
    /// The `turnsEnded` it ends at.
    turn: u32,
    /// Those the notice told of that were the session's own helpers then.
    helpers: Vec<String>,
    /// It told of nobody else: the turn was the session at work on what its
    /// own helpers brought, and nobody's notice came back to it.
    only_helpers: bool,
}

/// The last a caller heard of an account's usage window.
#[derive(Clone, Debug, PartialEq)]
struct QuotaTold {
    level: u8,
    resets_at: Option<String>,
}

/// Starts the notifier and what types in the sends held for people, once.
pub fn init() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        std::thread::Builder::new().name("luna-notifier".into()).spawn(notifier).expect("spawn notifier thread");
        std::thread::Builder::new().name("luna-held".into()).spawn(held_sender).expect("spawn held sender thread");
    });
}

/// Where a debt stands, by what the session is doing now.
#[derive(Debug, PartialEq, Clone, Copy)]
enum Verdict {
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
fn verdict(o: &Owed, ended: u32, turn: Option<crate::activity::Turn>, job: bool, real_turn: bool) -> Verdict {
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
/// `upward`: `caller` is `id`'s parent and `id` has written more than a line
/// or two since it last heard (`news_for_parent`). Then turns on its own
/// helpers' notices reach it while others are still out, every turn since
/// being one of those: what a helper makes of its helpers' answers goes up the
/// tree it was spawned in, and nothing goes back to a session whose notice
/// started a turn. Two chats that wrote to each other are no parent and helper
/// to each other; one that is the other's parent hears only of turns on
/// notices about its helper's own helpers, and the helper never of its parent.
fn real_turn_past(id: &str, caller: &str, after: u32, ended: u32, waits: bool, upward: impl FnOnce() -> bool) -> bool {
    let turns = after.saturating_add(1)..=ended;
    let (counts, on_helpers) = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let marked = r.notice_turns.get(id);
        let mark = |turn: u32| marked.and_then(|m| m.iter().find(|m| m.turn == turn));
        let counts = turns.clone().any(|turn| match mark(turn) {
            None => true,
            Some(m) => !waits && m.helpers.iter().any(|h| h != caller),
        });
        (counts, turns.clone().all(|turn| mark(turn).is_some_and(|m| m.only_helpers && !m.helpers.is_empty())))
    };
    // Read off the lock: it looks at the transcript.
    counts || (on_helpers && !turns.is_empty() && upward())
}

/// For `real_turn_past`: `caller` is `id`'s parent, and `id` has written a
/// message since `cursor` that is more than a line or two (SHORT_MESSAGE): a
/// report worth a turn of the parent's, not "noted, waiting on the rest",
/// which comes with the answer all the same.
fn news_for_parent(pty: &crate::pty::PtyManager, caller: &str, id: &str, k: &Known, cursor: u64) -> bool {
    k.parent.as_deref() == Some(caller) && said_since(pty, id, k.provider, cursor).iter().any(|m| m.chars().count() >= SHORT_MESSAGE)
}

/// A notice about `about` went into `caller` and starts its turn ending at
/// `turn`.
fn notice_turn(r: &mut Registry, caller: &str, turn: u32, about: &[String]) {
    let helpers: Vec<String> = about.iter().filter(|a| is_descendant(r, caller, a)).cloned().collect();
    let only_helpers = helpers.len() == about.len();
    let turns = r.notice_turns.entry(caller.to_string()).or_default();
    turns.push(NoticeTurn { turn, helpers, only_helpers });
    if turns.len() > NOTICE_TURNS_KEPT {
        turns.remove(0);
    }
}

/// Counts a notice to `caller` about `id`. Returns whether it was the last
/// BREAKER_NOTICES allows: the pair is stopped from here on.
fn count_notice(r: &mut Registry, caller: &str, id: &str, now: u64) -> bool {
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
fn last_notice(caller: &str, id: &str, now: u64) -> bool {
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    r.notice_times
        .get(&(caller.to_string(), id.to_string()))
        .is_some_and(|times| times.iter().filter(|at| now.saturating_sub(**at) < BREAKER_WINDOW_MS).count() + 1 >= BREAKER_NOTICES)
}

/// Whether Luna has stopped telling `caller` of `id`'s turns and jobs. A person
/// typing into either of them since is someone looking at it, and lifts that;
/// nothing a model does does, or two models answering each other would lift it
/// forever.
fn pair_stopped(pty: &crate::pty::PtyManager, caller: &str, id: &str) -> bool {
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
fn notice_line(pty: &crate::pty::PtyManager, caller: &str, who: &str, id: &str, k: &Known, o: Owed) -> Option<(Option<String>, Settled)> {
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
    // in its turn until then); for anything else that job's end is all there
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
                Some(format!(
                    "{who} has only idle processes left, which have done nothing for {} min: {names}. They may be stuck or waiting on something; luna_kill ends them. You will still hear when they end.",
                    quiet / 60_000
                )),
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
                // A core's going took the job: news, whatever was said.
                let how = if said.is_empty() && failed.is_none() && !o.job_went { Settled::NoNews(at) } else { Settled::Answered };
                let quoted = if failed.is_some() { String::new() } else { begins(&said) };
                let said = match said.len() {
                    0 => "it has said nothing since your last notice. It does not take that up by itself; luna_send asks it how it went"
                        .to_string(),
                    1 => format!("it wrote 1 message since your last notice; {}{quoted}", answer_file(id, &said)),
                    n => format!("it wrote {n} messages since your last notice; {}{quoted}", answer_file(id, &said)),
                };
                let failed = failed.map_or(String::new(), |why| format!(" Its last turn stopped on an error: {}", sentence(&why)));
                let since = if o.job_went { " that Luna's core has restarted" } else { "" };
                Some((Some(format!("{who} has nothing left running now{since}, and {said}.{failed}")), how))
            }
            Some((n, _)) if n == ended => None,
            _ => Some((None, Settled::Hold(ended))),
        };
    }
    // Going on by itself any moment: what it said so far is no end of anything.
    if now == Verdict::TurnOver && !resume_pending(id) {
        let cut = crate::background::cut_short(id) || o.cut_short;
        let failed = if cut { None } else { failure_of(pty, id, k.provider) };
        let replies = replies_since(pty, id, k.provider, o.cursor, cut || failed.is_some());
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
        let file = match (replies.is_empty(), lost.is_empty()) {
            (true, true) => None,
            (true, false) => note_file(id, &undelivered_text(&lost)),
            (false, true) => note_file(id, &answer_text(&replies)),
            (false, false) => note_file(id, &format!("{}\n\n---\n\n{}", answer_text(&replies), undelivered_text(&lost))),
        };
        let reply = match replies.len() {
            0 if !transcript_found(pty, id) => {
                ", but Luna cannot find its transcript, so whether it replied is unknown (luna_read with screen: true shows what there is)"
                    .to_string()
            }
            0 => " without a reply (luna_read with screen: true shows why, if anything does)".to_string(),
            1 => format!(" with 1 message since you asked; {}", answer_where(file.as_deref())),
            n => format!(" with {n} messages since you asked; {}", answer_where(file.as_deref())),
        };
        if parent && (crate::background::never_started(id) || (o.cut_short && first_turn_lost(pty, id))) {
            // Before it is told: what its next turn will not go on in is told with this.
            crate::background::settle_worktree(id);
            let what = never_started_what(&lost, file.as_deref());
            return Some((
                Some(format!("{who} never got started: Luna's core restarted before its first turn got anywhere, so its prompt was lost and there is nothing to carry on from; {what}. luna_send starts it afresh.")),
                Settled::Answered,
            ));
        }
        let lost = undelivered_line(&lost, !replies.is_empty(), file.as_deref());
        let unsure = if o.unsure {
            " Luna cannot tell whether your last message reached it: the core went as it was going in, and its transcript cannot be read. luna_read shows what it has."
        } else {
            ""
        };
        if cut {
            return Some((
                Some(format!("{who} did NOT finish its turn: Luna's core restarted while it worked and stopped it. It ended{reply}.{lost}{unsure} luna_send has it carry on.")),
                Settled::Answered,
            ));
        }
        // Whatever else is going on: this turn did not get to its end.
        if let Some(why) = failed {
            let said = if replies.is_empty() { String::new() } else { format!(" It ended{reply}.") };
            return Some((
                Some(format!("{who} stopped on an error: {}{said}{lost} luna_send has it carry on.", sentence(&why))),
                Settled::Answered,
            ));
        }
        let reply = format!("{reply}{}", begins(&replies));
        if background || waits {
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
            } else {
                format!("{who} is NOT done yet: {what}. It ended a turn{reply}. The next notice comes when that is over, or with a turn of its own.")
            };
            return Some((Some(line), Settled::Pending(ended, transcript_end(id))));
        }
        // Too short a turn for a notice of its own in between.
        let resumed = if o.resumed { " Luna's core stopped it on the way and it resumed automatically." } else { "" };
        return Some((Some(format!("{who} finished its turn{reply}.{lost}{resumed}{unsure}")), Settled::Answered));
    }
    if turn == Some(crate::activity::Turn::Busy) && alive && o.told_quiet != Some(ended) {
        if let Some(quiet) = turn_quiet_ms(pty, id).filter(|q| *q >= QUIET_TURN_MS) {
            // Looked for only once the turn is that quiet: a deadline holds the
            // notice back, never brings it sooner.
            let at = now_ms();
            let due = due_ms(pty, id, k.provider);
            if due.is_none_or(|due| at >= due.saturating_add(PAST_DUE_MS)) {
                return Some((Some(quiet_line(who, quiet, due.map(|due| at - due))), Settled::Quiet(ended)));
            }
        }
    }
    standing_line(pty, caller, who, id, k, o)
}

/// A turn that has said nothing for this long, its caller hears of once: a
/// command that hangs looks like this and like nothing else from outside, and
/// a caller that ended its turn on "a notice comes when it is done" would
/// otherwise hear nothing until a person asks. A long build looks the same,
/// hence once per turn and not its end.
const QUIET_TURN_MS: u64 = 30 * 60 * 1000;

/// A command its own timeout has yet to stop is no sign of a hang: a quiet turn
/// that waits on one is told of only this long past that timeout, by when the
/// CLI has ended the call and written so, or it hangs. Test lanes of 45 min
/// under a timeout of their own each woke their caller for nothing.
const PAST_DUE_MS: u64 = 5 * 60 * 1000;

fn quiet_line(who: &str, quiet_ms: u64, past_due_ms: Option<u64>) -> String {
    let quiet = quiet_ms / 60_000;
    match past_due_ms {
        Some(past) => format!(
            "{who} is mid-turn and has written nothing for {quiet} min, and the command it waits on is {} min past its own timeout, so it may hang; luna_read with screen: true shows where it is, luna_kill stops the turn. Said once per turn; the usual notice comes when it ends.",
            past / 60_000
        ),
        None => format!(
            "{who} is mid-turn and has written nothing for {quiet} min. A command it runs may hang, or simply take long; luna_read with screen: true shows where it is, luna_kill stops the turn. Said once per turn; the usual notice comes when it ends."
        ),
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
/// output (background.rs), a chat's transcript; None between turns.
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
fn standing_line(pty: &crate::pty::PtyManager, caller: &str, who: &str, id: &str, k: &Known, o: Owed) -> Option<(Option<String>, Settled)> {
    let turn = crate::activity::turn_state(id).map(|t| t.turn);
    if !live(pty, id) {
        // The window has yet to start it, as it does a chat in a pane.
        if returning(k, core_up_ms()) {
            return None;
        }
        if o.cut_short && k.parent.as_deref() == Some(caller) && first_turn_lost(pty, id) {
            leave_half_made(id, k);
            let lost = undelivered_for(id, caller, true);
            let file = note_file(id, &undelivered_text(&lost));
            let what = never_started_what(&lost, file.as_deref());
            // No `luna_send` for one that is not running. Opened by a person, it
            // would start with nothing to go on.
            return Some((
                Some(format!("{who} never got started: Luna's core restarted before its first turn got anywhere, and its CLI has not started again, so there is nothing to carry on from; {what}. Spawn it again if it still matters, and luna_delete this one.")),
                Settled::Exited,
            ));
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
    if o.resumed && !resume_pending(id) {
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
                Some(ask_id) => format!("{who} {}. Answer with luna_answer, askId {ask_id}.", a.line()),
                None => format!("{who} {}. Answer with luna_answer.", a.line()),
            },
            None => format!(
                "{who} is stopped on a prompt in its terminal; luna_read with screen: true shows it, luna_answer with keys answers it."
            ),
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
            return Some((Some(format!("{who} {} It works on meanwhile; luna_answer answers it.", a.line())), Settled::Asked(key)));
        }
    }
    None
}

/// The level of `QUOTA_LEVELS` a window at `pct` percent has reached, 0 if none.
fn quota_level(pct: f64) -> u8 {
    QUOTA_LEVELS.iter().copied().filter(|l| pct >= f64::from(*l)).max().unwrap_or(0)
}

/// Whether an instant has passed; one that does not read has not.
fn is_past(at: Option<&str>, now: u64) -> bool {
    at.and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()).is_some_and(|t| (t.timestamp_millis() as u64) < now)
}

/// Where a usage window stands against what the caller was last told of it:
/// the level to remember (0: nothing), and whether that is news. Numbers of a
/// window already over are stale, and say nothing until new ones come.
fn quota_step(told: Option<&QuotaTold>, pct: f64, resets_at: Option<&str>, now: u64) -> (u8, bool) {
    if is_past(resets_at, now) {
        return (0, false);
    }
    let reset = told.is_some_and(|t| {
        pct + QUOTA_SLACK < f64::from(t.level) || (t.resets_at.as_deref() != resets_at && is_past(t.resets_at.as_deref(), now))
    });
    let had = if reset { 0 } else { told.map_or(0, |t| t.level) };
    let level = quota_level(pct);
    (level.max(had), level > had)
}

fn quota_file() -> std::path::PathBuf {
    crate::paths::data_dir().join("quota-told.json")
}

/// `quota_told` as its file keeps it: a core started later is not to tell a
/// caller again what this one already did.
fn quota_bytes(told: &HashMap<(String, String, String), QuotaTold>) -> Vec<u8> {
    let mut rows: Vec<(&String, &String, &String, u8, &Option<String>)> =
        told.iter().map(|((c, a, l), t)| (c, a, l, t.level, &t.resets_at)).collect();
    rows.sort();
    serde_json::to_vec(&rows).unwrap_or_default()
}

/// The file's rows, but for windows over since they were written.
fn quota_from(bytes: &[u8], now: u64) -> HashMap<(String, String, String), QuotaTold> {
    let rows: Vec<(String, String, String, u8, Option<String>)> = serde_json::from_slice(bytes).unwrap_or_default();
    rows.into_iter()
        .filter(|row| !is_past(row.4.as_deref(), now))
        .map(|(c, a, l, level, resets_at)| ((c, a, l), QuotaTold { level, resets_at }))
        .collect()
}

/// Writes it out as it is now, called with the registry's lock let go. One
/// save at a time, each taking its own look: two threads that each took a
/// snapshot under the lock and saved it after would race on the one temp file,
/// and the older could land last, a warning told since told again after a restart.
fn save_quota_told() {
    static SAVING: Mutex<()> = Mutex::new(());
    let _saving = SAVING.lock().unwrap_or_else(|e| e.into_inner());
    let bytes = quota_bytes(&reg().lock().unwrap_or_else(|e| e.into_inner()).quota_told);
    let path = quota_file();
    if let Err(e) = crate::secret::write_private(&path, &bytes) {
        crate::log::warn("agents", &format!("could not save {}: {e}", path.display()));
    }
}

/// A usage window a caller is yet to hear of.
struct QuotaNews {
    key: (String, String, String),
    told: QuotaTold,
    line: String,
}

/// What each caller is yet to hear of the usage windows of the accounts its
/// live helpers run on, all generations counted, as `account_usage` last showed
/// them. It is told only once the notice has gone in (`quota_told`); what has
/// reset or come down is settled here.
fn quota_news(pty: &crate::pty::PtyManager) -> HashMap<String, Vec<QuotaNews>> {
    // Every helper's account, under every session above it.
    type Window = (String, f64, Option<String>);
    let (windows, helpers): (HashMap<String, Vec<Window>>, Vec<(String, String, String)>) = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let windows: HashMap<String, Vec<Window>> = r
            .usage
            .iter()
            .map(|(account, u)| {
                let limits = u["limits"].as_array().into_iter().flatten();
                let w: Vec<Window> = limits
                    .filter_map(|l| {
                        let resets = l["resetsAt"].as_str().map(str::to_owned);
                        Some((l["label"].as_str()?.to_string(), l["usedPct"].as_f64()?, resets))
                    })
                    .collect();
                (account.clone(), w)
            })
            .collect();
        // Nothing near a level and nothing told: the usual case, every look.
        if r.quota_told.is_empty() && !windows.values().flatten().any(|w| quota_level(w.1) > 0) {
            return HashMap::new();
        }
        let mut helpers = Vec::new();
        for (id, k) in &r.known {
            let account = format!("{}/{}", k.provider.as_str(), k.account);
            let mut above = k.parent.clone();
            while let Some(caller) = above {
                above = r.known.get(&caller).and_then(|p| p.parent.clone());
                if caller != DEV_CALLER && r.known.contains_key(&caller) {
                    helpers.push((caller, account.clone(), id.clone()));
                }
            }
        }
        (windows, helpers)
    };
    let live_ones: Vec<(String, String)> = helpers.into_iter().filter(|(_, _, id)| live(pty, id)).map(|(c, a, _)| (c, a)).collect();
    let mut news: HashMap<String, Vec<QuotaNews>> = HashMap::new();
    let now = now_ms();
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let before = quota_bytes(&r.quota_told);
    let mut pairs: Vec<&(String, String)> = live_ones.iter().collect();
    pairs.sort();
    pairs.dedup();
    for (caller, account) in pairs {
        for (label, pct, resets_at) in windows.get(account).into_iter().flatten() {
            let key = (caller.clone(), account.clone(), label.clone());
            let (level, say) = quota_step(r.quota_told.get(&key), *pct, resets_at.as_deref(), now);
            let told = QuotaTold { level, resets_at: resets_at.clone() };
            if say {
                let resets = resets_at.as_deref().map(|t| format!(" (resets {t})")).unwrap_or_default();
                let line = format!("Account {account}, which your helpers run on, has used {pct:.0}% of its \"{label}\" window{resets}.");
                news.entry(caller.clone()).or_default().push(QuotaNews { key, told, line });
            } else if level == 0 {
                r.quota_told.remove(&key);
            } else {
                r.quota_told.insert(key, told);
            }
        }
    }
    let after = quota_bytes(&r.quota_told);
    drop(r);
    if after != before {
        save_quota_told();
    }
    news
}

/// `Your helper "name" (id)`: what a notice calls one to its parent.
fn your_helper(id: &str, name: Option<&str>) -> String {
    match name {
        Some(n) => format!("Your helper \"{n}\" ({id})"),
        None => format!("Your helper {id}"),
    }
}

/// A budget passed that the helper's parent is yet to hear of.
struct BudgetNews {
    id: String,
    /// When it was passed: the line rides from then (`riders_go`).
    since: u64,
    line: String,
}

/// The first reading past a budget, kept as read, as `budgetBreach`: a compaction
/// may take the helper back under it before its parent can be told. At its budget
/// it has not passed it.
fn budget_breach(budget: u64, context: &crate::background::Context, now: u64) -> Option<serde_json::Value> {
    (context.tokens > budget)
        .then(|| serde_json::json!({ "at": now, "tokens": context.tokens, "window": context.window, "pct": context.pct }))
}

fn budget_line(who: &str, budget: u64, breach: &serde_json::Value) -> String {
    format!(
        "{who} has passed its budget of {budget} context tokens: {} of its {} window ({}%). Nothing was stopped.",
        breach["tokens"], breach["window"], breach["pct"]
    )
}

/// One watched helper, as `budget_news` takes it off the registry.
struct Budgeted {
    id: String,
    provider: Provider,
    budget: u64,
    breach: Option<serde_json::Value>,
    parent: Option<String>,
    name: Option<String>,
}

/// What the parents of helpers with a budget are yet to hear, by parent. Where it
/// stands is kept in the helper's settings, so a core started later goes on from
/// there: `budgetBreach` once one is seen, `budgetTold` once the line has reached the
/// parent. Between the two a background parent's own record says where the line is
/// (`background::budget_mark`), it being marked in the text itself. Told once: a helper
/// told of is not watched again. A transcript is read only when `read`. Also says
/// whether where any stands moved, to be saved at once.
fn budget_news(pty: &crate::pty::PtyManager, read: bool) -> (HashMap<String, Vec<BudgetNews>>, bool) {
    let watched: Vec<Budgeted> = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        r.known
            .iter()
            .filter(|(_, k)| k.settings["budgetTold"] != true)
            .filter_map(|(id, k)| {
                let budget = k.settings["budgetTokens"].as_u64()?;
                let parent = k.parent.clone().filter(|p| p != DEV_CALLER && r.known.contains_key(p));
                let name = r.names.get(id).map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
                let some = |key: &str| k.settings.get(key).filter(|v| !v.is_null()).cloned();
                Some(Budgeted { id: id.clone(), provider: k.provider, budget, breach: some("budgetBreach"), parent, name })
            })
            .collect()
    };
    let mut news: HashMap<String, Vec<BudgetNews>> = HashMap::new();
    let mut moved = false;
    for w in watched {
        let mut breach = w.breach;
        if breach.is_none() && read {
            // Off the registry lock: this reads the session's transcript.
            let context = transcript_path(pty, &w.id).and_then(|path| crate::background::Context::of(&path, w.provider));
            breach = context.and_then(|c| budget_breach(w.budget, &c, now_ms()));
            if let Some(b) = &breach {
                crate::log::warn(
                    "agents",
                    &format!("{} has passed its budget of {} context tokens: {}", label(&w.id), w.budget, b["tokens"]),
                );
                set_budget_state(&w.id, "budgetBreach", Some(b.clone()));
                moved = true;
            }
        }
        let Some(breach) = breach else { continue };
        let Some(parent) = w.parent else { continue };
        match crate::background::budget_mark(&parent, &w.id) {
            MarkStands::Coming => continue,
            MarkStands::Landed => {
                set_budget_state(&w.id, "budgetTold", Some(serde_json::json!(true)));
                crate::background::budget_mark_noted(&parent, &w.id);
                moved = true;
                continue;
            }
            // Kept as undelivered it would be told of as well: this notice has it.
            MarkStands::Undelivered => {
                crate::background::drop_undelivered_mark(&parent, &w.id, NOTICE_TAG);
                moved = true;
            }
            MarkStands::Nothing => {}
        }
        let line = budget_line(&your_helper(&w.id, w.name.as_deref()), w.budget, &breach);
        news.entry(parent).or_default().push(BudgetNews { id: w.id, since: breach["at"].as_u64().unwrap_or(0), line });
    }
    (news, moved)
}

fn set_budget_state(id: &str, key: &str, value: Option<serde_json::Value>) {
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    if let (Some(k), Some(v)) = (r.known.get_mut(id), value) {
        k.settings[key] = v;
    }
}

/// Whether a helper's parent is yet to hear it passed its budget, and is there to:
/// the notice has no debt to it, so this is all that keeps `reap` from taking the
/// breach with the helper.
fn budget_owed(r: &Registry, id: &str) -> bool {
    r.known.get(id).is_some_and(|k| {
        k.settings["budgetBreach"].is_object()
            && k.settings["budgetTold"] != true
            && k.parent.as_deref().is_some_and(|p| p != DEV_CALLER && r.known.contains_key(p))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::ask::ask_of;
    use crate::agents::lost::sentence;
    use crate::agents::notifier::{job_end_stood, Settled, NOTICE_TAG};
    use crate::agents::owed::{owe_from, Owed};
    use crate::agents::persist::{take_up_chats, tie_back, ties_of, SavedChat, SavedChats, SavedSend, Ties};
    use crate::agents::read::transcripts;
    use crate::agents::registry::{parent_of, reg, register, settings_of, waits_on_helpers, Known, DEV_CALLER};
    use crate::agents::said::said_since;
    use crate::agents::spawn::{chat_starting, SpawnTaken};
    use crate::agents::teardown::forget;
    use crate::agents::wait::{reply_text, REPLY_GRACE_MS};

    #[test]
    fn a_window_is_told_once_per_level_until_it_resets() {
        let now = now_ms();
        let later = crate::background::iso(now + 3_600_000).unwrap();
        let later = Some(later.as_str());
        let told = |level: u8| QuotaTold { level, resets_at: later.map(str::to_owned) };

        assert_eq!(quota_step(None, 89.0, later, now), (0, false));
        assert_eq!(quota_step(None, 90.0, later, now), (90, true));
        assert_eq!(quota_step(Some(&told(90)), 91.0, later, now), (90, false), "once per level");
        assert_eq!(quota_step(Some(&told(90)), 95.0, later, now), (95, true));
        assert_eq!(quota_step(Some(&told(95)), 99.0, later, now), (95, false));
        assert_eq!(quota_step(None, 97.0, later, now), (95, true), "a jump past both is one line");
        assert_eq!(quota_step(Some(&told(95)), 93.0, later, now), (95, false), "a point or two down is rounding");
        assert_eq!(quota_step(Some(&told(90)), 3.0, later, now), (0, false), "reset: forgotten");

        // A new window that is already high, seen only by its reset time.
        let gone = crate::background::iso(now - 60_000).unwrap();
        let old = QuotaTold { level: 95, resets_at: Some(gone.clone()) };
        assert_eq!(quota_step(Some(&old), 96.0, later, now), (95, true), "told again for the new window");
        // Numbers of a window already over say nothing.
        assert_eq!(quota_step(Some(&old), 96.0, Some(&gone), now), (0, false));
        assert_eq!(quota_step(None, 96.0, Some(&gone), now), (0, false));
        // A reset time moving about within a window is not a new one.
        let drift = crate::background::iso(now + 3_700_000).unwrap();
        assert_eq!(quota_step(Some(&told(95)), 96.0, Some(&drift), now), (95, false));
    }

    #[test]
    fn quota_news_goes_to_whoever_a_live_helper_runs_under() {
        let none = serde_json::json!({});
        let acc = "C:/acc/anthropic/qn-hot";
        register("qn-top", Provider::Claude, "C:/p", "C:/acc/anthropic/qn-cold", None, true, none.clone());
        register("qn-mid", Provider::Claude, "C:/p", "C:/acc/anthropic/qn-cold", Some("qn-top"), true, none.clone());
        register("qn-low", Provider::Claude, "C:/p", acc, Some("qn-mid"), false, none.clone());
        register("qn-dead", Provider::Claude, "C:/p", "C:/acc/anthropic/qn-idle", Some("qn-top"), false, none);
        crate::background::test_helper("qn-low", acc, None, false);
        let pty = crate::pty::manager();
        let limits = |pct: u8| serde_json::json!({ "limits": [{ "label": "week", "usedPct": pct, "resetsAt": null }] });
        let set = |pct: u8| {
            let mut r = reg().lock().unwrap();
            r.usage.insert("claude/qn-hot".into(), limits(pct));
            r.usage.insert("claude/qn-idle".into(), limits(99));
        };
        set(92);
        let news = quota_news(pty);
        for caller in ["qn-top", "qn-mid"] {
            let lines = &news[caller];
            let hot: Vec<&QuotaNews> = lines.iter().filter(|n| n.key.1 == "claude/qn-hot").collect();
            assert_eq!(hot.len(), 1, "{caller}");
            assert!(hot[0].line.contains("has used 92% of its \"week\" window"), "{}", hot[0].line);
        }
        assert!(!news.values().flatten().any(|n| n.key.1 == "claude/qn-idle"), "no live helper of its runs there");
        assert!(!news.contains_key("qn-low"), "a session does not hear of its own account");

        // Not told until the notice has gone in: asked again until it has.
        assert!(quota_news(pty).contains_key("qn-top"));
        for n in news.into_values().flatten() {
            reg().lock().unwrap().quota_told.insert(n.key, n.told);
        }
        assert!(!quota_news(pty).contains_key("qn-top"), "once");
        set(96);
        assert_eq!(quota_news(pty)["qn-top"][0].told.level, 95, "the next level");
        forget("qn-top");
        forget("qn-mid");
        assert!(!reg().lock().unwrap().quota_told.keys().any(|(c, _, _)| c == "qn-top" || c == "qn-mid"));
        crate::background::remove("qn-low");
        forget("qn-low");
        forget("qn-dead");
        let mut r = reg().lock().unwrap();
        r.usage.remove("claude/qn-hot");
        r.usage.remove("claude/qn-idle");
    }

    #[test]
    fn what_a_caller_was_told_of_quota_outlives_the_core() {
        let now = now_ms();
        let later = crate::background::iso(now + 3_600_000);
        let gone = crate::background::iso(now - 60_000);
        let mut told = HashMap::new();
        let key = |l: &str| ("c".to_string(), "claude/a".to_string(), l.to_string());
        told.insert(key("week"), QuotaTold { level: 95, resets_at: later.clone() });
        told.insert(key("5 hours"), QuotaTold { level: 90, resets_at: gone });
        let back = quota_from(&quota_bytes(&told), now);
        assert_eq!(back.len(), 1, "a window over since is forgotten");
        assert_eq!(back[&key("week")], QuotaTold { level: 95, resets_at: later.clone() });
        let (level, say) = quota_step(back.get(&key("week")), 96.0, later.as_deref(), now);
        assert_eq!((level, say), (95, false), "not told again");
        assert!(quota_from(b"not json", now).is_empty());
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
        assert!(fine.contains("finished its turn") && fine.contains("It begins: \"All done and fixed.\""), "{fine}");
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
                notice_turn(&mut r, caller, turns[caller] + 1, &[id.to_string()]);
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
        notice_turn(&mut r, "rt-s", 4, &["rt-other".into()]);
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
        notice_turn(&mut reg().lock().unwrap(), "tr-mid", 2, &["tr-low".into()]);
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
        notice_turn(&mut reg().lock().unwrap(), "tr-mid", 3, &["tr-low2".into()]);
        let waits = waits_on_helpers("tr-mid", "tr-top");
        assert!(!waits);
        let told = Owed::pending(2, 0);
        assert_eq!(
            verdict(&told, 3, Some(Idle), waits, real_turn_past("tr-mid", "tr-top", 2, 3, waits, || false)),
            Verdict::TurnOver,
            "it reaches the top"
        );
        // A notice about something else, or about the caller itself, does not count.
        notice_turn(&mut reg().lock().unwrap(), "tr-mid", 4, &["tr-stranger".into()]);
        assert!(!real_turn_past("tr-mid", "tr-top", 3, 4, false, || false));
        assert!(!real_turn_past("tr-mid", "tr-top", 3, 4, false, || true), "news or not");
        // A notice about its helper and someone else: not all its own helpers' doing.
        notice_turn(&mut reg().lock().unwrap(), "tr-mid", 6, &["tr-low".into(), "tr-stranger".into()]);
        assert!(!real_turn_past("tr-mid", "tr-top", 5, 6, true, || true));
        notice_turn(&mut reg().lock().unwrap(), "tr-low", 1, &["tr-mid".into()]);
        assert!(!real_turn_past("tr-low", "tr-mid", 0, 1, false, || false), "not a helper of its own");
        notice_turn(&mut reg().lock().unwrap(), "tr-mid", 5, &["tr-low".into()]);
        assert!(!real_turn_past("tr-mid", "tr-low", 4, 5, false, || false), "its own notice coming back");
        for id in ["tr-low", "tr-low2", "tr-mid", "tr-top"] {
            forget(id);
        }
    }

    /// The QA helper of 0.8.1: its helpers' notices start its turns, each ends
    /// on a report its parent needs now, and its parent heard none until the
    /// last helper had answered.
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
        notice_turn(&mut reg().lock().unwrap(), "ir-mid", 2, &["ir-low".into()]);
        let known = reg().lock().unwrap().known.get("ir-mid").cloned().unwrap();
        let pty = crate::pty::manager();
        let line = |caller: &str, transcript: &str| {
            std::fs::write(&path, format!("{before}{transcript}")).unwrap();
            notice_line(pty, caller, "Your helper", "ir-mid", &known, Owed::pending(1, before.len() as u64))
        };
        let report = format!("Phase 1 is in: kill the core before 12:35 UTC. {}", "Evidence. ".repeat(45));

        let Some((Some(text), Settled::Pending(2, cursor))) = line("ir-top", &format!("{notice}{}", said(&report))) else {
            panic!("its parent hears")
        };
        assert!(text.contains("ended another turn") && text.contains("helpers of its own have yet to answer it"), "{text}");
        assert!(text.contains("It begins: \"Phase 1 is in"), "{text}");
        assert_eq!(cursor, std::fs::metadata(&path).unwrap().len(), "and what follows is what it hears next");
        let file = text.split("the answer is in ").nth(1).and_then(|t| t.split(", its report first").next()).unwrap();
        assert!(std::fs::read_to_string(file).unwrap().starts_with("Phase 1 is in"));
        let _ = std::fs::remove_file(file);
        // A line or two is no news worth a turn: it comes with the answer.
        assert!(matches!(line("ir-top", &format!("{notice}{}", said("Noted; waiting on ir-low2."))), Some((None, Settled::Skip(2)))));
        // Only its parent: a session that wrote to it is told of its own turns alone.
        assert!(matches!(line("ir-peer", &format!("{notice}{}", said(&report))), Some((None, Settled::Skip(2)))));
        // A turn on a notice about anyone but its own helpers is no news for the parent.
        notice_turn(&mut reg().lock().unwrap(), "ir-mid", 2, &["ir-low".into(), "ir-peer".into()]);
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
                        notice_turn(&mut r, caller, turns[caller] + 1, &[id.to_string()]);
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
            assert!(text.contains("written nothing for 31 min. A command it runs may hang"), "{text}");
            crate::background::gone_for_test(&id);
            forget(&id);
        }
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
        assert!(standing_line(pty, "bp-c", "Your helper", "bp-s", &known, o).is_none());
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
        let Some((Some(line), Settled::Waiting)) = standing_line(pty, "bp-c", "Your helper", "bp-s", &known, o) else {
            panic!("a prompt goes through")
        };
        assert!(line.contains("asks for permission to run Bash: ls") && line.contains("askId"), "{line}");
        crate::background::kill("bp-s");
        let _ = asks.join();
        // And gone is gone.
        crate::background::gone_for_test("bp-s");
        assert!(matches!(standing_line(pty, "bp-c", "Your helper", "bp-s", &known, o), Some((Some(_), Settled::Exited))));
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
            line.contains("never got started") && line.contains("its prompt was lost") && line.contains("luna_send starts it afresh"),
            "{line}"
        );
        assert!(line.contains("its prompt and 1 queued message are in "), "{line}");
        let file = line.split(" are in ").nth(1).and_then(|l| l.split(". luna_send").next()).unwrap();
        let held = std::fs::read_to_string(file).unwrap();
        let _ = std::fs::remove_file(file);
        assert!(held.find("do the whole job").unwrap() < held.find("and a follow-up").unwrap(), "{held}");
        // Still there to delete, which is what writes its end down.
        assert!(reg().lock().unwrap().known.contains_key("ns-child"));
        crate::background::remove("ns-child");
        forget("ns-child");
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
        let sends =
            held.iter().map(|text| SavedSend { caller: parent.into(), id: id.into(), text: (*text).into(), going_in: None }).collect();
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

    fn context(tokens: u64) -> crate::background::Context {
        crate::background::Context { pct: 21, tokens, window: 1_000_000 }
    }

    #[test]
    fn a_budget_is_passed_only_past_it_and_its_line_says_so() {
        assert_eq!(budget_breach(200_000, &context(200_000), 5), None, "at it, not past it");
        let b = budget_breach(200_000, &context(200_001), 5).unwrap();
        assert_eq!(b, serde_json::json!({ "at": 5, "tokens": 200_001, "window": 1_000_000, "pct": 21 }));
        assert_eq!(
            budget_line(&your_helper("kid", Some("Tests")), 200_000, &b),
            "Your helper \"Tests\" (kid) has passed its budget of 200000 context tokens: 200001 of its 1000000 window (21%). Nothing was stopped."
        );
        assert!(budget_line(&your_helper("kid", None), 1, &b).starts_with("Your helper kid has passed"));
    }

    /// Writes a transcript or rollout whose last answer has `tokens` in the window.
    fn write_usage(path: &std::path::Path, provider: Provider, tokens: u64) {
        let line = match provider {
            Provider::Claude => {
                serde_json::json!({ "type": "assistant", "message": { "model": "claude-sonnet-5-5", "usage": { "input_tokens": tokens }, "content": [] } })
            }
            Provider::Codex => serde_json::json!({ "timestamp": "t", "type": "event_msg", "payload": { "type": "token_count", "info": {
                "total_token_usage": { "total_tokens": tokens }, "last_token_usage": { "input_tokens": tokens, "total_tokens": tokens }, "model_context_window": 258400 } } }),
        };
        std::fs::write(path, format!("{line}\n")).unwrap();
    }

    #[test]
    fn a_budget_breach_stays_owed_through_a_compaction_and_a_core_and_is_told_once() {
        let _budgets = budgets_to_myself();
        let pty = crate::pty::manager();
        let dir = std::env::temp_dir().join(format!("luna-budget-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (parent, kid, provider) in [("bb-p1", "bb-kid1", Provider::Claude), ("bb-p2", "bb-kid2", Provider::Codex)] {
            tooled(parent);
            let news_of = |read: bool| budget_news(pty, read).0.remove(parent).unwrap_or_default();
            let path = dir.join(format!("{kid}.jsonl"));
            write_usage(&path, provider, 900);
            transcripts().lock().unwrap().insert(kid.to_string(), path.clone());
            let start = serde_json::json!({ "budgetTokens": 1000 });
            register(kid, provider, "C:\\p", "C:\\acc\\one", Some(parent), false, start);
            assert!(news_of(true).is_empty(), "{kid}: under it");
            assert!(settings_of(kid).get("budgetBreach").is_none());
            write_usage(&path, provider, 1500);
            assert!(news_of(false).is_empty(), "{kid}: not a look that reads");
            let first = news_of(true);
            assert_eq!(first.len(), 1, "{kid}");
            assert!(first[0].line.contains("has passed its budget of 1000 context tokens: 1500 of its"), "{}", first[0].line);
            let breach = settings_of(kid)["budgetBreach"].clone();
            assert_eq!(breach["tokens"], 1500);

            // Its parent busy, the helper compacts: what was read stands.
            write_usage(&path, provider, 100);
            let again = news_of(true);
            assert_eq!(again.len(), 1, "{kid}: still owed");
            assert_eq!(again[0].line, first[0].line);
            assert_eq!(again[0].since, first[0].since, "and its time");

            // A core goes before the parent is told: its ties carry it over.
            let ties = serde_json::to_vec(&ties_of(kid).unwrap()).unwrap();
            reg().lock().unwrap().known.remove(kid);
            let ties: Ties = serde_json::from_slice(&ties).unwrap();
            tie_back(kid, provider, "C:\\p", "C:\\acc\\one", ties);
            assert_eq!(news_of(true)[0].line, first[0].line, "{kid}: after a core");

            // Typed into its parent: told, not watched, and a second crossing says nothing.
            set_budget_state(kid, "budgetTold", Some(serde_json::json!(true)));
            write_usage(&path, provider, 2000);
            assert!(news_of(true).is_empty(), "{kid}: told once");
            assert_eq!(settings_of(kid)["budgetTold"], true);
            forget(kid);
            forget(parent);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_budget_with_no_parent_to_tell_is_kept_for_when_there_is_one() {
        let _budgets = budgets_to_myself();
        let pty = crate::pty::manager();
        let breach = serde_json::json!({ "at": 1, "tokens": 5, "window": 10, "pct": 50 });
        let settings = serde_json::json!({ "budgetTokens": 1, "budgetBreach": breach });
        register("bn-kid", Provider::Claude, "C:\\p", "C:\\acc\\one", Some("bn-gone"), false, settings.clone());
        assert!(!budget_news(pty, false).0.values().flatten().any(|n| n.id == "bn-kid"));
        assert_eq!(settings_of("bn-kid")["budgetBreach"], breach);
        register("bn-dev", Provider::Claude, "C:\\p", "C:\\acc\\one", Some(DEV_CALLER), false, settings);
        assert!(!budget_news(pty, false).0.values().flatten().any(|n| n.id == "bn-dev"), "a dev caller takes no notices");
        forget("bn-kid");
        forget("bn-dev");
    }

    /// `budget_news` goes over every helper there is, a test's or not.
    fn budgets_to_myself() -> std::sync::MutexGuard<'static, ()> {
        static ONE: Mutex<()> = Mutex::new(());
        ONE.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A background parent, its helper past a budget, and what `budget_news` has for it.
    fn budget_pair(parent: &str, kid: &str) -> (String, crate::background::BudgetMark) {
        let breach = serde_json::json!({ "at": 1, "tokens": 5, "window": 10, "pct": 50 });
        crate::background::idle_for_test(parent, Provider::Claude, 1, false);
        tooled(parent);
        register(
            kid,
            Provider::Claude,
            "C:\\p",
            "C:\\acc\\one",
            Some(parent),
            false,
            serde_json::json!({ "budgetTokens": 1, "budgetBreach": breach }),
        );
        let line = budget_now(parent).expect("a line").clone();
        (format!("{NOTICE_TAG} {line}"), crate::background::BudgetMark { id: kid.to_string(), line })
    }

    fn budget_now(parent: &str) -> Option<String> {
        budget_news(crate::pty::manager(), false).0.remove(parent).unwrap_or_default().pop().map(|n| n.line)
    }

    fn budget_pair_gone(parent: &str, kid: &str) {
        forget(kid);
        forget(parent);
    }

    #[test]
    fn a_budget_line_to_an_idle_parent_counts_only_once_its_stdin_took_it() {
        let _budgets = budgets_to_myself();
        let (parent, kid) = ("bi-parent", "bi-kid");
        let (notice, mark) = budget_pair(parent, kid);
        assert!(!crate::background::place_for_test(parent, &notice, mark.clone()), "idle: a turn is claimed for it");
        assert_eq!(budget_now(parent), None, "on its way, not said again");
        assert_ne!(settings_of(kid)["budgetTold"], true, "and not told yet");
        // The CLI went before it took anything: still owed.
        crate::background::turn_ended_for_test(parent);
        assert_eq!(budget_now(parent).as_deref(), Some(mark.line.as_str()));
        assert_ne!(settings_of(kid)["budgetTold"], true);
        // Its stdin took it.
        assert!(!crate::background::place_for_test(parent, &notice, mark));
        crate::background::stdin_took_for_test(parent);
        assert_eq!(budget_now(parent), None);
        assert_eq!(settings_of(kid)["budgetTold"], true);
        budget_pair_gone(parent, kid);
    }

    #[test]
    fn a_budget_line_behind_a_parents_turn_is_owed_until_a_turn_takes_it_in() {
        let _budgets = budgets_to_myself();
        let (parent, kid) = ("bq-parent", "bq-kid");
        let (notice, mark) = budget_pair(parent, kid);
        crate::background::set_running_for_test(parent);
        assert!(crate::background::place_for_test(parent, &notice, mark.clone()), "behind its turn");
        assert_eq!(budget_now(parent), None, "not typed twice");
        // A turn started on it, not yet taken in.
        crate::background::start_queued_for_test(parent);
        assert_eq!(budget_now(parent), None);
        assert_ne!(settings_of(kid)["budgetTold"], true);
        crate::background::stdin_took_for_test(parent);
        assert_eq!(budget_now(parent), None);
        assert_eq!(settings_of(kid)["budgetTold"], true);
        budget_pair_gone(parent, kid);
    }

    #[test]
    fn a_parent_killed_with_a_budget_line_queued_has_not_heard_it() {
        let _budgets = budgets_to_myself();
        let (parent, kid) = ("bk-parent", "bk-kid");
        let (notice, mark) = budget_pair(parent, kid);
        crate::background::set_running_for_test(parent);
        assert!(crate::background::place_for_test(parent, &notice, mark.clone()));
        crate::background::killed_for_test(parent);
        assert_eq!(budget_now(parent).as_deref(), Some(mark.line.as_str()), "owed, and told over");
        assert_ne!(settings_of(kid)["budgetTold"], true);
        budget_pair_gone(parent, kid);
    }

    #[test]
    fn a_core_going_with_a_budget_line_queued_neither_repeats_it_nor_loses_it() {
        let _budgets = budgets_to_myself();
        // The enqueue and what says a line is on its way are one thing on disk.
        let (parent, kid) = ("bc-parent", "bc-kid");
        let (notice, mark) = budget_pair(parent, kid);
        crate::background::set_running_for_test(parent);
        assert!(crate::background::place_for_test(parent, &notice, mark.clone()));
        crate::background::across_a_core_for_test(parent);
        // A parent that goes on by itself takes the old text: nothing is said again.
        crate::background::resume_for_test(parent);
        assert_eq!(budget_now(parent), None, "resumed with it: not told twice");
        crate::background::stdin_took_for_test(parent);
        assert_eq!(budget_now(parent), None);
        assert_eq!(settings_of(kid)["budgetTold"], true);
        budget_pair_gone(parent, kid);

        // One that does not has it taken back out, and told over, once.
        let (parent, kid) = ("bd-parent", "bd-kid");
        let (notice, mark) = budget_pair(parent, kid);
        crate::background::set_running_for_test(parent);
        assert!(crate::background::place_for_test(parent, &format!("{notice} And more."), mark.clone()));
        crate::background::across_a_core_for_test(parent);
        assert_eq!(budget_now(parent).as_deref(), Some(mark.line.as_str()));
        let kept = crate::background::undelivered_for(parent, "x", true);
        assert_eq!(kept.len(), 1, "what else it said stays");
        assert!(!kept[0].contains(&mark.line), "{kept:?}");
        assert_ne!(settings_of(kid)["budgetTold"], true);
        budget_pair_gone(parent, kid);

        // Landed before the core went: that stands.
        let (parent, kid) = ("be-parent", "be-kid");
        let (notice, mark) = budget_pair(parent, kid);
        assert!(!crate::background::place_for_test(parent, &notice, mark));
        crate::background::stdin_took_for_test(parent);
        crate::background::across_a_core_for_test(parent);
        assert_eq!(budget_now(parent), None);
        assert_eq!(settings_of(kid)["budgetTold"], true);
        budget_pair_gone(parent, kid);
    }

    pub(super) fn tooled(id: &str) {
        register(id, Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, serde_json::json!({}));
    }
}
