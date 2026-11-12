//! Answers owed. A caller that spawned a session or typed into it is owed the
//! turn that answers it (`Owed`), until a tool response shows it one past the
//! question (`shown`, `hand_over`) or a notice tells it.

use crate::agents::lost::{put_undelivered_back, take_undelivered};
use crate::agents::read::transcript_end;
use crate::agents::registry::{parent_of, reg, waits_on_helpers_in, Registry, DEV_CALLER};
use crate::throttle::now_ms;

/// Text typed into a chat's terminal mid-turn is the CLI's to place: into
/// the turn that runs, or a turn of its own right after it. A turn end
/// counts as the answer to it once no turn has started this long after —
/// more than one activity sample.
pub(super) const QUEUED_GRACE_MS: u64 = 8_000;

#[derive(Clone, Copy)]
pub(super) struct Owed {
    /// Turns over when the caller asked; the answer ends a later one.
    pub(super) baseline: u32,
    /// The caller has been told the session is stopped on a prompt.
    pub(super) told_waiting: bool,
    /// The question the caller was last told the session asks beside its
    /// work, by hash (0: none).
    pub(super) told_ask: u64,
    /// The turn count at which the answer was first seen in but held back,
    /// and when: processes still at work (BACKGROUND_GRACE_MS), no reply yet
    /// (REPLY_GRACE_MS), a turn yet to start on queued text (QUEUED_GRACE_MS).
    pub(super) held: Option<(u32, u64)>,
    /// When the notifier first found the answer in while a `wait` was out
    /// for it (see WAIT_HOLD_MS).
    pub(super) ready_at: Option<u64>,
    /// Where the session's transcript ended when the caller asked (the first
    /// of its questions still unanswered): what it said from there on is the
    /// answer, however many turns it took.
    pub(super) cursor: u64,
    /// Asked by text typed into a chat's terminal mid-turn (see
    /// QUEUED_GRACE_MS).
    pub(super) queued: bool,
    /// Asked by text Luna held (`Held`) and typed in later: the `turnsEnded`
    /// the caller got with `held` is older than the question.
    pub(super) late: bool,
    /// The session exited with the caller's text still held, never typed in.
    pub(super) lost: bool,
    /// A core's going stopped its turn and it has gone on by itself
    /// (`resumeOnRestart`); the caller has yet to hear.
    pub(super) resumed: bool,
    /// A chat's turn asked about went with a core (`restore_chats`): it did
    /// not finish, and the caller hears so.
    pub(super) cut_short: bool,
    /// Told it is not done under a core since gone: on Windows what it had running
    /// went with that core, and its own helpers' answers come as ever.
    pub(super) job_went: bool,
    /// Text was going in as a core went, and nothing can tell whether it arrived
    /// (`went_in`): not typed again, and the caller is told.
    pub(super) unsure: bool,
    /// The caller has been told the session is not done yet. It hears again
    /// when that ends or a turn no notice started does, not of every turn; a
    /// question asked since is a new debt.
    pub(super) told_pending: bool,
    /// The caller has been told all its job left has sat idle (IDLE_JOB_MS):
    /// once per debt, which stays open for the job's end.
    pub(super) told_idle: bool,
    /// The turn count during which the caller was told the turn has gone quiet
    /// (QUIET_TURN_MS): once per turn.
    pub(super) told_quiet: Option<u32>,
    /// The same for a helper's turn that has answered and only waits on its
    /// background work (lines.rs `background_wait_line`): told apart, so a
    /// plain quiet notice earlier in the turn does not stand for it.
    pub(super) told_background_wait: Option<u32>,
    /// Which debt this is, whatever its numbers: asked again with nothing
    /// moved since, a new one has the same baseline and cursor as the old, and
    /// what was decided about the old is not to touch it.
    pub(super) rev: u64,
}

fn next_rev() -> u64 {
    static NEXT_REV: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT_REV.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl Owed {
    pub(super) fn from(baseline: u32, cursor: u64) -> Owed {
        Owed {
            rev: next_rev(),
            baseline,
            told_waiting: false,
            told_ask: 0,
            held: None,
            ready_at: None,
            cursor,
            queued: false,
            late: false,
            lost: false,
            resumed: false,
            cut_short: false,
            job_went: false,
            unsure: false,
            told_pending: false,
            told_idle: false,
            told_quiet: None,
            told_background_wait: None,
        }
    }

    /// What is left of a debt once the caller has heard it is not done yet.
    pub(super) fn pending(baseline: u32, cursor: u64) -> Owed {
        Owed { told_pending: true, ..Owed::from(baseline, cursor) }
    }
}

/// `caller` has been handed the answer `id` gave by `turns_ended`: a turn past
/// what it asked about settles what it was owed. It has seen that count too
/// (see `seen`).
pub(super) fn shown(caller: &str, id: &str, turns_ended: u32) {
    let key = (caller.to_string(), id.to_string());
    let goes_on = take_debt(&mut reg().lock().unwrap_or_else(|e| e.into_inner()), &key, turns_ended, None);
    owed_on(key, turns_ended, goes_on);
    seen(caller, id, turns_ended);
}

/// Takes what `caller` is owed by `id` if it has been handed the answer: a turn
/// past what it asked about, or, `job_debt` being this very debt, the end of a
/// job it was told of. In one hold of the lock, so that a question asked a
/// moment later is a debt that stands. Returns whether what the session goes on
/// to say is owed on (`owed_on`).
fn take_debt(r: &mut Registry, key: &(String, String), turns_ended: u32, job_debt: Option<u64>) -> bool {
    let Some(o) = r.owed.get(key) else { return false };
    if turns_ended <= o.baseline && job_debt != Some(o.rev) {
        return false;
    }
    r.owed.remove(key);
    waits_on_helpers_in(r, &key.1, &key.0)
}

/// What a session says once its own helpers have answered it is what it was
/// asked for: still owed, past what the caller has now, as a notice's "not
/// done" leaves it. Off the lock, as this reads the transcript's length, and so
/// behind a question asked meanwhile, whose debt covers that too.
fn owed_on(key: (String, String), turns_ended: u32, goes_on: bool) {
    owed_on_for(key, turns_ended, goes_on, None);
}

/// `owed_on` by a hand-over of the debt of rev `taken`: the one put in is said
/// to the receipt that stands for that debt in the same hold (`Receipt::put_in`),
/// so a hand-over coming in next finds them tied. Returns the one put in, by
/// `rev`: none when a question since stands there already.
fn owed_on_for(key: (String, String), turns_ended: u32, goes_on: bool, taken: Option<u64>) -> Option<u64> {
    if !goes_on {
        return None;
    }
    let cursor = transcript_end(&key.1);
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let r = &mut *r;
    let rev = match r.owed.entry(key.clone()) {
        std::collections::hash_map::Entry::Vacant(v) => v.insert(Owed::pending(turns_ended, cursor)).rev,
        std::collections::hash_map::Entry::Occupied(_) => return None,
    };
    if let Some(x) = r.receipts.get_mut(&key).filter(|x| taken.is_some() && x.stands_for.map(|(o, _)| o.rev) == taken) {
        x.put_in = Some(rev);
    }
    Some(rev)
}

/// The latest answer handed over to a caller about a session (`hand_over`).
#[derive(Default)]
pub(super) struct Receipt {
    n: u64,
    /// The debt it stands for, and since when: the one it took, or, taking none,
    /// the one a response just before it took, which that response's cancel may
    /// still come for (mcp.rs LATE_CANCEL) and find this one in its place.
    stands_for: Option<(Owed, std::time::Instant)>,
    /// The debt of "not done yet" a hand-over put in that one's place
    /// (`owed_on`), by `rev`: what an undo takes back out.
    put_in: Option<u64>,
}

/// `shown`, for a tool response the client may yet drop unread (a cancel
/// that crosses it): returns what puts the debt and the count seen back as
/// they were. Whatever a later call has recorded meanwhile stands, and a later
/// call that handed the answer over too has delivered it: this one's response
/// being dropped then owes nothing.
pub(super) fn hand_over(caller: &str, id: &str, turns_ended: u32, job_debt: Option<u64>) -> impl FnOnce() + Send + 'static {
    let undo = hand_over_of(caller, id, turns_ended, job_debt, None);
    move || {
        if let Some(undo) = undo {
            undo();
        }
    }
}

/// `hand_over` of the debt of revision `rev` alone, checked and taken under one
/// hold of the lock: None, and nothing done, when the caller's debt there is
/// another by now (a question asked since), which `hand_over` would take.
pub(super) fn hand_over_rev(caller: &str, id: &str, turns_ended: u32, rev: u64) -> Option<impl FnOnce() + Send + 'static> {
    hand_over_of(caller, id, turns_ended, None, Some(rev))
}

fn hand_over_of(
    caller: &str,
    id: &str,
    turns_ended: u32,
    job_debt: Option<u64>,
    expected: Option<u64>,
) -> Option<impl FnOnce() + Send + 'static> {
    let key = (caller.to_string(), id.to_string());
    let (taken, seen_before, receipt, goes_on, stop) = {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        if expected.is_some_and(|rev| r.owed.get(&key).map(|o| o.rev) != Some(rev)) {
            return None;
        }
        let (now, seen_before) = (r.owed.get(&key).copied(), r.seen.get(&key).copied());
        let receipt = r.receipts.entry(key.clone()).or_default();
        receipt.n += 1;
        // Two responses on one answer, both dropped: the earlier one's undo
        // finds this one in its place and leaves the debt to it, so this one
        // puts back what that one took — and not what that one put in its
        // place, which is all this one finds.
        let recent = receipt.stands_for.is_some_and(|(_, at)| at.elapsed() < crate::mcp::LATE_CANCEL);
        let inherits = recent && now.is_none_or(|o| receipt.put_in == Some(o.rev));
        if !inherits {
            receipt.stands_for = now.map(|o| (o, std::time::Instant::now()));
            receipt.put_in = None;
        }
        let (n, taken) = (receipt.n, receipt.stands_for.map(|(o, _)| o.rev));
        // Said in the reply by the same rule (`background_stopped_note`), before the debt goes.
        let stop = now.is_some() && crate::background::background_stopped_for(id, caller);
        let goes_on = take_debt(&mut r, &key, turns_ended, job_debt);
        let stop = stop.then(|| callers_on(&r, id));
        (taken, seen_before, n, goes_on, stop)
    };
    owed_on_for(key.clone(), turns_ended, goes_on, taken);
    seen(caller, id, turns_ended);
    // What never reached it goes with this response alone: any later one no
    // longer finds it, so the receipt below says nothing of it.
    let parent = parent_of(id).as_deref() == Some(caller);
    let taken = take_undelivered(id, caller, parent);
    let stop_told = stop.and_then(|owed| crate::background::background_stop_told(id, caller, &owed));
    let id = id.to_string();
    Some(move || {
        put_undelivered_back(&id, taken);
        // Whatever the receipt below says: a later reply found the caller told
        // and did not say it again.
        if let Some(turn) = stop_told {
            crate::background::background_stop_untold(&id, &key.0, turn);
        }
        crate::background::save();
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        // As the receipt has them now: a stand-in put in after this response
        // came by is tied to it there (`owed_on_for`).
        let Some((owed, put_in)) = r.receipts.get(&key).filter(|x| x.n == receipt).map(|x| (x.stands_for.map(|(o, _)| o), x.put_in)) else {
            return;
        };
        if let Some(o) = owed {
            match r.owed.get_mut(&key) {
                // What was put in the place of the debt it stands for
                // (`owed_on`) goes, and that debt is back as it was.
                Some(now) if Some(now.rev) == put_in => *now = o,
                Some(now) => now.cursor = now.cursor.min(o.cursor),
                None => {
                    r.owed.insert(key.clone(), o);
                }
            }
        }
        if r.seen.get(&key) == Some(&turns_ended) {
            match seen_before {
                Some(n) => r.seen.insert(key, n),
                None => r.seen.remove(&key),
            };
        }
    })
}

/// The callers `id` owes an answer, whatever turn each is owed.
pub(super) fn callers_on(r: &Registry, id: &str) -> Vec<String> {
    r.owed.keys().filter(|(_, s)| s == id).map(|(c, _)| c.clone()).collect()
}

/// `callers_on`, for the moment helper `id`'s CLI says it stopped its
/// background work: who is to hear of that (background `stop_owed_to`).
pub fn callers_owed(id: &str) -> Vec<String> {
    callers_on(&reg().lock().unwrap_or_else(|e| e.into_inner()), id)
}

/// What a reply handed over to `caller` (a wait, read or report) says of
/// helper `id`'s CLI having stopped its background work, while it owes
/// `caller` an answer and that is still to be said to it (notifier lines.rs
/// `background_stopped_line`); `hand_over` acknowledges it by the same rule.
pub(super) fn background_stopped_note(caller: &str, id: &str) -> Option<String> {
    reg().lock().unwrap_or_else(|e| e.into_inner()).owed.get(&(caller.to_string(), id.to_string()))?;
    crate::agents::notifier::lines::background_stopped_line(id, caller).map(|line| line.trim().to_string())
}

/// Records the turn count a tool answer showed `caller` of `id`, settling
/// nothing: the default `afterTurn` of its next `wait` once nothing is owed.
pub(super) fn seen(caller: &str, id: &str, turns_ended: u32) {
    reg().lock().unwrap_or_else(|e| e.into_inner()).seen.insert((caller.to_string(), id.to_string()), turns_ended);
    looked(id);
}

/// Somebody has looked at `id`, so an `autoDelete` helper counts its idle
/// time from now.
pub(super) fn looked(id: &str) {
    if let Some(a) = reg().lock().unwrap_or_else(|e| e.into_inner()).auto.get_mut(id) {
        a.idle_since = None;
    }
}

/// `caller` has asked `id` something; the answer ends a turn past `baseline`
/// and is what the session says from `cursor` on, where its transcript ended
/// before the question went in (0 for a session just spawned, whose whole
/// transcript answers its opening prompt). Asked again before an earlier
/// answer reached it, the caller is owed that one too: the cursor stays the
/// earlier one. The dev caller has no terminal to be told in.
pub(super) fn owe_from(caller: &str, id: &str, baseline: u32, cursor: u64) {
    owe(caller, id, baseline, cursor, None);
}

/// `owe_from` for text a send put into a helper's queue, `handovers` being the
/// count (handover.rs `handovers_of`) taken before it went in: none if the person
/// has taken the helper over since, which took that text back out and told its
/// sender, so nothing of the chat it is now is owed. False then.
pub(super) fn owe_from_at(caller: &str, id: &str, baseline: u32, cursor: u64, handovers: u64) -> bool {
    owe(caller, id, baseline, cursor, Some(handovers))
}

fn owe(caller: &str, id: &str, baseline: u32, cursor: u64, handovers: Option<u64>) -> bool {
    if caller == DEV_CALLER {
        return true;
    }
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    if handovers.is_some_and(|n| r.handovers.get(id).copied().unwrap_or(0) != n) {
        return false;
    }
    let key = (caller.to_string(), id.to_string());
    // A new turn answers this one; that the last was cut short has yet to be
    // said all the same.
    let (cursor, resumed) = r.owed.get(&key).map_or((cursor, false), |o| (o.cursor.min(cursor), o.resumed || o.cut_short));
    // Text can miss its turn before the send that placed it gets here (`retried`).
    let floor = r.delivery_floors.get(&key).copied().unwrap_or(0);
    r.owed.insert(key, Owed { resumed, ..Owed::from(baseline.max(floor), cursor) });
    drop(r);
    // A spawn's or a question's: a core killed now must not forget either.
    crate::background::save();
    true
}

/// The last count `caller` was shown for `id`, if any.
pub(super) fn last_shown(caller: &str, id: &str) -> Option<u32> {
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    r.seen.get(&(caller.to_string(), id.to_string())).copied()
}

/// A helper goes on by itself after a core's going stopped its turn: whoever
/// is owed its answer is owed the end of the turn about to start, past the
/// `turns_ended` counted so far, and has yet to hear of it. The cursor stays:
/// the answer is all it has said since they asked.
pub fn resumed(id: &str, turns_ended: u32) {
    resumed_in(&mut reg().lock().unwrap_or_else(|e| e.into_inner()), id, turns_ended);
}

pub(super) fn resumed_in(r: &mut Registry, id: &str, turns_ended: u32) {
    for (_, o) in r.owed.iter_mut().filter(|((_, s), _)| s == id) {
        o.baseline = o.baseline.max(turns_ended);
        o.resumed = true;
        o.cut_short = false;
    }
}

/// Text that joined a background helper's turn missed it, its CLI going before
/// taking it in, and starts the next turn instead: the turn counted `turns_ended`
/// did not answer `callers`, who heard it would. Their answer lies past that count,
/// for the debt and for a wait, which may be going by an `afterTurn` from before
/// this. Nothing else about the helper changed, so nothing else is said of it. A
/// debt moved is a new one (`rev`): a notice looked up before this told of the turn
/// that missed, and neither goes out nor settles anything.
pub fn retried(id: &str, callers: &[String], turns_ended: u32) {
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    for caller in callers {
        let key = (caller.clone(), id.to_string());
        if let Some(o) = r.owed.get_mut(&key) {
            o.baseline = o.baseline.max(turns_ended);
            o.held = None;
            o.rev = next_rev();
        }
        let floor = r.delivery_floors.entry(key).or_default();
        *floor = (*floor).max(turns_ended);
    }
}

/// Whether a turn end at `turns_ended` answers a question typed in mid-turn
/// (`Owed::queued`) by now: a turn past the one it was typed into has ended,
/// or this end has stood QUEUED_GRACE_MS with no turn begun on the text.
/// Notes when the end was first seen, for the next look.
pub(super) fn queue_settled(key: &(String, String), o: Owed, turns_ended: u32) -> bool {
    if turns_ended >= o.baseline.saturating_add(2) {
        return true;
    }
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let Some(owed) = r.owed.get_mut(key) else { return false };
    match owed.held {
        Some((n, at)) if n == turns_ended => now_ms().saturating_sub(at) >= QUEUED_GRACE_MS,
        _ => {
            owed.held = Some((turns_ended, now_ms()));
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::notifier::{settle_told, Settled};
    use crate::agents::registry::{reg, register, resumes_on_restart, DEV_CALLER};
    use crate::agents::teardown::forget;
    use crate::agents::test_support::notice_for;
    use crate::agents::wait::wait;
    use crate::provider::Provider;

    #[test]
    fn an_answer_is_owed_until_the_caller_sees_a_later_turn() {
        let none = serde_json::json!({});
        register("o-parent", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone());
        register("o-child", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("o-parent"), false, none);
        let owed = || reg().lock().unwrap().owed.get(&("o-parent".to_string(), "o-child".to_string())).map(|o| o.baseline);
        owe_from("o-parent", "o-child", 1, 0);
        shown("o-parent", "o-child", 1);
        assert_eq!(owed(), Some(1), "the send itself shows the count it went in at");
        shown("o-parent", "o-child", 2);
        assert_eq!(owed(), None, "a wait or read past it settles the answer");
        owe_from(DEV_CALLER, "o-child", 0, 0);
        assert!(!reg().lock().unwrap().owed.contains_key(&(DEV_CALLER.to_string(), "o-child".to_string())));
    }

    #[test]
    fn a_chat_is_in_the_background_while_its_helpers_owe_it_an_answer() {
        use crate::activity::in_background;
        let none = serde_json::json!({});
        let acc = "C:\\acc\\anthropic\\one";
        register("ib-top", Provider::Claude, "C:\\p", acc, None, true, none.clone());
        register("ib-mid", Provider::Claude, "C:\\p", acc, Some("ib-top"), true, none.clone());
        register("ib-low", Provider::Claude, "C:\\p", acc, Some("ib-mid"), false, none.clone());
        register("ib-other", Provider::Claude, "C:\\p", acc, None, true, none.clone());
        register("ib-theirs", Provider::Claude, "C:\\p", acc, Some("ib-other"), false, none);
        let settle = |caller: &str, helper: &str| reg().lock().unwrap().owed.remove(&(caller.to_string(), helper.to_string()));

        assert!(!in_background("ib-top"), "nothing owed yet");
        owe_from("ib-top", "ib-mid", 0, 0);
        assert!(in_background("ib-top"), "by its helper");
        settle("ib-top", "ib-mid");
        owe_from("ib-top", "ib-low", 0, 0);
        assert!(in_background("ib-top"), "by its helper's helper");
        settle("ib-top", "ib-low");
        assert!(!in_background("ib-top"), "answered");

        owe_from("ib-other", "ib-theirs", 0, 0);
        assert!(!in_background("ib-top"), "a helper owed to another chat is none of its");
        assert!(in_background("ib-other"));
        for id in ["ib-low", "ib-mid", "ib-top", "ib-theirs", "ib-other"] {
            forget(id);
        }
    }

    #[test]
    fn asked_again_the_earlier_answer_is_still_owed() {
        let key = ("m-parent".to_string(), "m-child".to_string());
        let owed = || reg().lock().unwrap().owed.get(&key).map(|o| (o.baseline, o.cursor));
        owe_from("m-parent", "m-child", 1, 100);
        owe_from("m-parent", "m-child", 2, 300);
        assert_eq!(owed(), Some((2, 100)), "the second answer ends a later turn; both are read from the first question on");
        reg().lock().unwrap().owed.remove(&key);
        owe_from("m-parent", "m-child", 3, 500);
        assert_eq!(owed(), Some((3, 500)), "once collected, a new question starts where it went in");
    }

    #[test]
    fn a_hand_over_undone_puts_back_what_it_took() {
        let key = ("h-parent".to_string(), "h-child".to_string());
        owe_from("h-parent", "h-child", 1, 40);
        seen("h-parent", "h-child", 1);
        let undo = hand_over("h-parent", "h-child", 2, None);
        assert!(!reg().lock().unwrap().owed.contains_key(&key), "handed over");
        assert_eq!(last_shown("h-parent", "h-child"), Some(2));
        undo();
        assert_eq!(reg().lock().unwrap().owed.get(&key).map(|o| (o.baseline, o.cursor)), Some((1, 40)));
        assert_eq!(last_shown("h-parent", "h-child"), Some(1));
        // A question asked in between stands, owing the dropped answer too.
        let undo = hand_over("h-parent", "h-child", 2, None);
        owe_from("h-parent", "h-child", 2, 90);
        seen("h-parent", "h-child", 2);
        undo();
        assert_eq!(reg().lock().unwrap().owed.get(&key).map(|o| (o.baseline, o.cursor)), Some((2, 40)));
        reg().lock().unwrap().owed.remove(&key);
    }

    /// A wait, read or report that hands an answer over says that the helper's
    /// CLI stopped its background work, and has said it to that caller alone;
    /// a reply that never reached it has not, whatever came of the others' and
    /// of a stop since.
    #[test]
    fn a_reply_handed_over_carries_a_cut_off_and_a_cancel_takes_that_back() {
        use crate::background::{background_stopped, background_stopped_for};
        let (id, a, b) = ("hb-child", "hb-parent", "hb-peer");
        crate::background::idle_for_test(id, Provider::Claude, 3, false);
        crate::background::background_stopped_for_test(id, &[a, b]);
        owe_from(a, id, 2, 0);
        owe_from(b, id, 2, 0);
        let note = background_stopped_note(a, id).expect("said while it holds");
        assert!(note.starts_with("Its CLI stopped the subagents") && note.contains("uncommitted"), "{note}");
        let undo_a = hand_over(a, id, 3, None);
        assert_eq!(background_stopped_note(a, id), None, "said to the parent");
        assert!(background_stopped_for(id, b), "not yet to the other caller");
        let undo_b = hand_over(b, id, 3, None);
        assert!(!background_stopped(id), "both have had it");
        // Both replies dropped, the parent's first: each undo takes back its
        // own caller alone, never the other's as it stood then.
        undo_a();
        assert!(background_stopped_for(id, a) && !background_stopped_for(id, b), "the peer's reply still stands");
        undo_b();
        assert!(background_stopped_for(id, a), "the peer's undo leaves the parent untold");
        assert!(background_stopped_for(id, b));
        // Told to both again; the parent's reply is dropped only after a stop
        // at the end of a later turn has been told to it: that one stands.
        let undo_a = hand_over(a, id, 3, None);
        drop(hand_over(b, id, 3, None));
        assert!(!background_stopped(id));
        crate::background::idle_for_test(id, Provider::Claude, 4, false);
        crate::background::background_stopped_for_test(id, &[a, b]);
        owe_from(a, id, 3, 0);
        owe_from(b, id, 3, 0);
        drop(hand_over(a, id, 4, None));
        undo_a();
        assert!(!background_stopped_for(id, a), "told of the later stop, which the undo leaves alone");
        assert!(background_stopped_for(id, b), "and the later stop is still to be told to the peer");
        crate::background::gone_for_test(id);
        reg().lock().unwrap().owed.retain(|(_, s), _| s != id);
    }

    /// `codex exec` 0.167.2 interrupted the subagent a Codex helper's turn
    /// spawned as that turn ended, and exited 0. Its parent hears so in the
    /// words for that, with the answer; a caller who asked after it does not.
    #[test]
    fn a_codex_helpers_subagent_cut_off_at_its_turn_end_is_told() {
        let (id, parent, later) = ("cx-child", "cx-parent", "cx-later");
        let root = std::env::temp_dir().join(format!("luna-cx-cut-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let t0 = now_ms() - 60_000;
        let (thread, kid) = (format!("cxp{t0}"), format!("cxk{t0}"));
        crate::codex::session::cut_off_pair_for_test(&root, t0, &thread, &kid, true);
        crate::background::codex_in_turn_for_test(id, &root.to_string_lossy(), &thread, t0);
        owe_from(parent, id, 0, 0);
        crate::background::turn_over_for_test(id, Some(0));
        let note = background_stopped_note(parent, id).expect("the parent is told");
        assert!(note.starts_with("Its CLI interrupted the subagents it had spawned as its turn ended"), "{note}");
        owe_from(later, id, 1, 0);
        assert_eq!(background_stopped_note(later, id), None, "it asked after that turn");
        drop(hand_over(parent, id, 1, None));
        assert!(!crate::background::background_stopped(id), "everyone it concerned has had it");
        crate::background::gone_for_test(id);
        reg().lock().unwrap().owed.retain(|(_, s), _| s != id);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_dropped_response_does_not_take_back_what_a_later_call_delivered() {
        let key = ("hr-parent".to_string(), "hr-child".to_string());
        owe_from("hr-parent", "hr-child", 1, 40);
        let dropped = hand_over("hr-parent", "hr-child", 2, None);
        let read = hand_over("hr-parent", "hr-child", 2, None);
        dropped();
        assert!(!reg().lock().unwrap().owed.contains_key(&key), "the later read delivered it");
        assert_eq!(last_shown("hr-parent", "hr-child"), Some(2));
        read();
        forget("hr-parent");
        forget("hr-child");
    }

    /// Two responses on one answer, both dropped, in either order: the earlier
    /// one's undo leaves the debt to the later, which puts back what the earlier
    /// took, so the answer is still owed.
    #[test]
    fn two_responses_dropped_leave_the_answer_owed() {
        for (pair, earlier_first) in [("hd-a", true), ("hd-b", false)] {
            let (caller, child) = (format!("{pair}-parent"), format!("{pair}-child"));
            let key = (caller.clone(), child.clone());
            owe_from(&caller, &child, 1, 40);
            let earlier = hand_over(&caller, &child, 2, None);
            let later = hand_over(&caller, &child, 2, None);
            if earlier_first {
                earlier();
                later();
            } else {
                later();
                earlier();
            }
            assert_eq!(reg().lock().unwrap().owed.get(&key).map(|o| (o.baseline, o.cursor)), Some((1, 40)), "neither reached it");
            reg().lock().unwrap().owed.remove(&key);
            forget(&caller);
            forget(&child);
        }
    }

    #[test]
    fn a_queued_question_settles_after_a_later_turn_or_the_grace() {
        let key = ("q-parent".to_string(), "q-child".to_string());
        owe_from("q-parent", "q-child", 4, 0);
        let o = || reg().lock().unwrap().owed.get(&key).copied().unwrap();
        assert!(!queue_settled(&key, o(), 5), "the turn it was typed into may be followed by its own");
        assert!(!queue_settled(&key, o(), 5), "not yet");
        reg().lock().unwrap().owed.get_mut(&key).unwrap().held = Some((5, now_ms() - QUEUED_GRACE_MS));
        assert!(queue_settled(&key, o(), 5), "no turn began on it in the grace");
        assert!(queue_settled(&key, o(), 6), "a turn past the running one ended");
        reg().lock().unwrap().owed.remove(&key);
    }

    #[test]
    fn a_question_asked_since_makes_a_new_debt() {
        owe_from("nd-a", "nd-b", 1, 10);
        reg().lock().unwrap().owed.insert(("nd-a".into(), "nd-b".into()), Owed::pending(2, 20));
        owe_from("nd-a", "nd-b", 3, 30);
        let o = reg().lock().unwrap().owed.remove(&("nd-a".to_string(), "nd-b".to_string())).unwrap();
        assert!(!o.told_pending, "this one may be told it is not done, once");
        assert_eq!((o.baseline, o.cursor), (3, 20));
    }

    #[test]
    fn only_a_job_ending_or_a_later_turn_settles_a_caller_told_it_is_not_done() {
        let key = ("js-a".to_string(), "js-b".to_string());
        let owed = || reg().lock().unwrap().owed.get(&key).copied();
        let told = Owed::pending(3, 0);
        reg().lock().unwrap().owed.insert(key.clone(), told);
        // A wait with an `afterTurn` from before the notice hands over a turn
        // the caller was told of: the job's end is still to come.
        drop(hand_over("js-a", "js-b", 3, None));
        assert!(owed().is_some_and(|o| o.told_pending), "still owed");
        drop(hand_over("js-a", "js-b", 3, Some(told.rev)));
        assert!(owed().is_none(), "the job's end, with no turn past it");
        reg().lock().unwrap().owed.insert(key.clone(), Owed::pending(3, 0));
        drop(hand_over("js-a", "js-b", 4, None));
        assert!(owed().is_none(), "a later turn, as ever");
    }

    #[test]
    fn a_question_asked_meanwhile_outlives_what_was_decided_about_the_last_one() {
        let key = ("rv-a".to_string(), "rv-b".to_string());
        let owed = || reg().lock().unwrap().owed.get(&key).copied();
        // A wait saw the job end on the debt it knew; the caller has asked again.
        let old = Owed::pending(3, 0);
        reg().lock().unwrap().owed.insert(key.clone(), old);
        owe_from("rv-a", "rv-b", 3, 0);
        let new = owed().unwrap();
        assert_ne!(new.rev, old.rev, "the same numbers, another debt");
        drop(hand_over("rv-a", "rv-b", 3, Some(old.rev)));
        assert_eq!(owed().map(|o| o.rev), Some(new.rev), "the new question stands");
        // A notice about the old one went in meanwhile: nor does that take it.
        settle_told(&mut reg().lock().unwrap(), key.clone(), Settled::Answered, &old);
        assert_eq!(owed().map(|o| o.rev), Some(new.rev));
        settle_told(&mut reg().lock().unwrap(), key.clone(), Settled::Pending(9, 9), &old);
        assert_eq!(owed().map(|o| (o.rev, o.baseline)), Some((new.rev, 3)));
        settle_told(&mut reg().lock().unwrap(), key.clone(), Settled::Waiting, &old);
        assert!(!owed().unwrap().told_waiting);
        // The one it told of it settles, and a debt a wait took stays gone.
        settle_told(&mut reg().lock().unwrap(), key.clone(), Settled::Answered, &new);
        assert!(owed().is_none());
        settle_told(&mut reg().lock().unwrap(), key.clone(), Settled::Pending(9, 9), &new);
        assert!(owed().is_none(), "a notice does not bring back what a wait handed over");
    }

    #[test]
    fn taking_a_debt_and_owing_on_leave_a_new_question_alone() {
        let none = serde_json::json!({});
        register("tk-top", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone());
        register("tk-mid", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("tk-top"), true, none.clone());
        register("tk-low", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("tk-mid"), false, none);
        let key = ("tk-top".to_string(), "tk-mid".to_string());
        owe_from("tk-mid", "tk-low", 0, 0);
        owe_from("tk-top", "tk-mid", 0, 0);
        let goes_on = take_debt(&mut reg().lock().unwrap(), &key, 1, None);
        assert!(goes_on, "its own helper has yet to answer it");
        assert!(!reg().lock().unwrap().owed.contains_key(&key), "taken in the one hold of the lock");
        // A question between the taking and the owing on is the debt from here.
        owe_from("tk-top", "tk-mid", 1, 50);
        let asked = reg().lock().unwrap().owed.get(&key).copied().unwrap();
        owed_on(key.clone(), 1, goes_on);
        let now = reg().lock().unwrap().owed.get(&key).copied().unwrap();
        assert_eq!((now.rev, now.told_pending), (asked.rev, false));
        for id in ["tk-low", "tk-mid", "tk-top"] {
            forget(id);
        }
    }

    /// A hand-over that found its session waiting on its own helpers put a debt
    /// of "not done yet" in place of the one it took: dropped, the one it took is
    /// back as it was, not that one with its cursor moved.
    #[test]
    fn a_hand_over_undone_puts_back_the_debt_it_took_not_the_one_it_owed_on() {
        let none = serde_json::json!({});
        register("uo-top", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone());
        register("uo-mid", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("uo-top"), true, none.clone());
        register("uo-low", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("uo-mid"), false, none);
        let key = ("uo-top".to_string(), "uo-mid".to_string());
        owe_from("uo-mid", "uo-low", 0, 0);
        owe_from("uo-top", "uo-mid", 0, 30);
        let asked = reg().lock().unwrap().owed.get(&key).copied().unwrap();
        let undo = hand_over("uo-top", "uo-mid", 1, None);
        let put_in = reg().lock().unwrap().owed.get(&key).copied().unwrap();
        assert!(put_in.told_pending && put_in.rev != asked.rev, "owed on, as its helper has yet to answer it");
        undo();
        let back = reg().lock().unwrap().owed.get(&key).copied().unwrap();
        assert_eq!((back.rev, back.baseline, back.cursor, back.told_pending), (asked.rev, 0, 30, false));
        // Two on it, both dropped: the later found only what the earlier put in.
        let earlier = hand_over("uo-top", "uo-mid", 1, None);
        let later = hand_over("uo-top", "uo-mid", 1, None);
        earlier();
        later();
        let back = reg().lock().unwrap().owed.get(&key).copied().unwrap();
        assert_eq!((back.rev, back.baseline, back.cursor, back.told_pending), (asked.rev, 0, 30, false), "the one asked, as it was");
        for id in ["uo-low", "uo-mid", "uo-top"] {
            forget(id);
        }
    }

    #[test]
    fn a_helper_that_resumed_owes_the_end_of_that_turn_and_says_so_once() {
        let none = serde_json::json!({ "resumeOnRestart": true });
        register("rs-child", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("rs-parent"), false, none);
        assert!(resumes_on_restart("rs-child"));
        assert!(!resumes_on_restart("nobody"));
        crate::background::take_up_for_test("rs-child", "C:\\p", 3, &[], None);
        owe_from("rs-parent", "rs-child", 3, 40);
        // The stopped turn is counted (4); the answer ends the one started past it.
        resumed("rs-child", 4);
        let key = ("rs-parent".to_string(), "rs-child".to_string());
        let owed = || reg().lock().unwrap().owed.get(&key).copied().unwrap();
        assert_eq!((owed().baseline, owed().cursor), (4, 40), "all it has said since asked, up to the end of the new turn");
        let (line, how) = notice_for("rs-parent", "rs-child", 4);
        assert!(matches!(how, Settled::Resumed));
        assert!(line.unwrap().contains("resumed automatically"));
        owe_from("rs-parent", "rs-child", 4, 90);
        assert!(owed().resumed && owed().cursor == 40, "asked again before the notice went, it still has to go");
        crate::background::remove("rs-child");
        forget("rs-child");
    }

    /// Text that missed the turn it joined: its sender's answer lies past that
    /// turn, for a wait going by an `afterTurn` from before that too, and it hears
    /// nothing of a restart that did not happen.
    #[test]
    fn text_that_missed_its_turn_moves_its_senders_answer_and_only_that() {
        let (a, b, id) = ("rt-sender", "rt-other", "rt-helper");
        register(a, Provider::Claude, "C:\\p", "C:\\acc\\one", None, true, serde_json::json!({}));
        crate::background::test_helper(a, "C:\\acc\\one", None, false);
        crate::background::idle_for_test(id, Provider::Claude, 1, false);
        register(id, Provider::Claude, "C:\\p", "C:\\acc\\one", Some(a), false, serde_json::json!({}));
        owe_from(a, id, 0, 7);
        owe_from(b, id, 0, 7);
        retried(id, &[a.to_string()], 1);
        let owed = |who: &str| reg().lock().unwrap().owed.get(&(who.to_string(), id.to_string())).copied().unwrap();
        assert_eq!(owed(a).baseline, 1);
        assert!(!owed(a).resumed && !owed(a).cut_short, "no core went");
        assert_eq!(owed(a).cursor, 7, "all it says from where it was asked is the answer");
        assert_eq!(owed(b).baseline, 0, "another's question stands as it was");

        // It went on to stop on a permission: that is no end of the turn it asked about.
        crate::background::in_turn_for_test(id);
        crate::background::hold_for_test(id);
        let w = wait(a, id, "turn_done", 2, Some(0), None, &crate::mcp::Call::new()).unwrap();
        assert_eq!(w.outcome, "waiting", "not `turn_done`, though the count is past the 0 it asked for");
        assert_eq!(owed(a).baseline, 1, "and still owed");

        // The turn it did start ends, and that is the answer.
        crate::background::turn_ended_for_test(id);
        let w = wait(a, id, "turn_done", 2, Some(0), None, &crate::mcp::Call::new()).unwrap();
        assert_eq!(w.outcome, "turn_done");
        forget(id);
        crate::background::gone_for_test(id);
        crate::background::gone_for_test(a);
        forget(a);
    }

    /// A send can be held up after placing its text, for as long as a save takes:
    /// the CLI goes meanwhile, and the debt comes in behind `retried`.
    #[test]
    fn a_debt_recorded_after_a_retry_lies_past_it() {
        let (a, id) = ("rd-sender", "rd-helper");
        register(a, Provider::Claude, "C:\\p", "C:\\acc\\one", None, true, serde_json::json!({}));
        retried(id, &[a.to_string()], 1);
        owe_from(a, id, 0, 7);
        let key = (a.to_string(), id.to_string());
        assert_eq!(reg().lock().unwrap().owed.get(&key).unwrap().baseline, 1);
        forget(id);
        forget(a);
    }

    /// A chat counts on from the helper it was: a floor and a debt from then go by
    /// the same count as the turns it has from now on.
    #[test]
    fn a_helper_opened_as_a_chat_keeps_its_turn_count() {
        let (a, id) = ("pc-sender", "pc-helper");
        let none = serde_json::json!({});
        register(a, Provider::Claude, "C:\\p", "C:\\acc\\one", None, true, none.clone());
        register(id, Provider::Claude, "C:\\p", "C:\\acc\\one", Some(a), false, none);
        crate::background::idle_for_test(id, Provider::Claude, 9, false);
        retried(id, &[a.to_string()], 9);
        crate::background::background_promote(id.to_string()).unwrap();
        assert_eq!(crate::background::background_promoted(id.to_string(), None), Ok(true));
        assert!(!crate::background::has(id));
        assert_eq!(crate::activity::turns_counted(id), Some(9), "not 0, which a floor of 9 would wait past for nine turns");
        crate::activity::between_turns_for_test(id);
        assert_eq!(crate::activity::turn_state(id).map(|t| t.turns_ended), Some(9));
        forget(id);
        forget(a);
    }
}
