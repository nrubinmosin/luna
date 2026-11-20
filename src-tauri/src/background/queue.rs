//! Text for a helper: queued for its next turn, joined to the one running, or
//! reserved for a batch until its senders are settled, and what never reached it.

use super::*;

/// A batch as one prompt, as the turn gets it.
pub(crate) fn joined(batch: &[Queued]) -> String {
    batch.iter().map(|q| q.text.as_str()).collect::<Vec<_>>().join("\n\n")
}

/// A batch goes as one line of stdin, named for its first message.
pub(super) fn stamp(batch: &mut [Queued]) {
    let line = batch.first().map(|q| q.id.clone());
    for q in batch {
        q.line = line.clone();
    }
}

/// A batch claimed for the next turn, its senders still to be asked
/// (`settle_batch`): what it holds, by id, and the text in it a CLI would read
/// as a command (`/`, `!`), by (id, sender, text), and the claim it is on.
/// A claim made since (the helper deleted and made again, a turn begun) has
/// taken the batch over; with its own claim still there and the batch moved
/// by a stop, the claim is the reservation's to give back.
pub(crate) struct Reservation {
    claim: u64,
    ids: Vec<String>,
    screen: Vec<(String, String, String)>,
    /// A `resume`'s batch, which runs as `resume_text` has it even with none
    /// of its text left; `Some(true)`: a helper that never named its
    /// conversation, which runs on its text alone.
    resume: Option<bool>,
}

pub(super) fn reservation_of(e: &Entry, resume: Option<bool>) -> Reservation {
    let claim = e.run.as_ref().map_or(0, |run| run.claim);
    let ids = e.in_flight.iter().map(|q| q.id.clone()).collect();
    let screen = e
        .in_flight
        .iter()
        .filter(|q| crate::agents::command_prefix(&q.text).is_some())
        .filter_map(|q| Some((q.id.clone(), q.from.clone()?, q.text.clone())))
        .collect();
    Reservation { claim, ids, screen, resume }
}

/// What is queued becomes the batch of the turn about to start, on a run the
/// caller has claimed; on disk as `in_flight` while its senders are asked.
pub(super) fn reserve(e: &mut Entry) -> Reservation {
    e.in_flight = std::mem::take(&mut e.queue);
    stamp(&mut e.in_flight);
    reservation_of(e, None)
}

/// The turn a settled batch starts.
pub(super) struct Batch {
    pub(super) text: String,
    pub(super) turns_ended: u32,
    pub(super) carried: usize,
}

/// Asks, with no lock held, whether each sender of text a CLI would read as a
/// command may still send it (agents/admission.rs `command_reach_now`): it may have been
/// deleted, or the helper moved to another chat, since it was taken. Then,
/// with the reservation still standing, drops what is refused and gives the
/// batch that is left; with none left, the next queued batch is asked about
/// the same way, or the claim is given back and no turn starts. A refusal is
/// told to its sender, or the helper's parent if the sender is gone, by a
/// notice; it is not kept as undelivered, which a resume would type in.
/// Luna's own text (no sender) was checked when it was made.
pub(super) fn settle_batch(id: &str, mut res: Reservation) -> Option<Batch> {
    enum Step {
        Run(Batch),
        Again(Reservation),
        Idle,
        Released,
        Lost,
    }
    loop {
        let refused: Vec<(String, String, String)> = res
            .screen
            .iter()
            .filter_map(|(qid, from, text)| {
                crate::agents::command_reach_now(from, id, text).err().map(|why| (qid.clone(), from.clone(), why))
            })
            .collect();
        // Whether the refused text was dropped here, and so is to be told of.
        let mut dropped = false;
        let step = {
            let mut r = lock();
            let ours = |e: &Entry| e.run.as_ref().is_some_and(|run| run.claim == res.claim && run.pid.is_none());
            let stopped =
                |e: &Entry| e.run.as_ref().is_some_and(|run| run.stopped) || !e.in_flight.iter().map(|q| &q.id).eq(res.ids.iter());
            match r.entries.get_mut(id) {
                // A stop came while its senders were asked: the batch went with
                // it, and the claim, which no process will take up now, goes
                // back, the turn it was for counted as ended (as `run_turn`
                // counts one that could not start). What was sent meanwhile,
                // queued behind that claim, is the next turn's, as after any
                // turn: reserved and asked about the same way. Not while a stop
                // is still ending what was left running, or the helper is being
                // opened as a chat or deleted (`held_back`): each takes the queue
                // up itself once the claim has gone.
                Some(e) if ours(e) && stopped(e) => {
                    e.run = None;
                    e.turns_ended += 1;
                    note(e, "stopped before its turn began");
                    if !e.queue.is_empty() && !e.ending_left && !e.held_back() {
                        claim(e);
                        Step::Again(reserve(e))
                    } else {
                        Step::Released
                    }
                }
                Some(e) if ours(e) => {
                    dropped = true;
                    e.in_flight.retain(|q| !refused.iter().any(|(qid, _, _)| *qid == q.id));
                    stamp(&mut e.in_flight);
                    let carried = e.in_flight.len();
                    match res.resume {
                        Some(false) => Step::Run(Batch { text: resume_text(&e.in_flight), turns_ended: e.turns_ended, carried }),
                        _ if carried > 0 => Step::Run(Batch { text: joined(&e.in_flight), turns_ended: e.turns_ended, carried }),
                        _ if !e.queue.is_empty() => Step::Again(reserve(e)),
                        _ => {
                            e.run = None;
                            Step::Idle
                        }
                    }
                }
                _ => Step::Lost,
            }
        };
        // Taken over or stopped meanwhile: nothing of it was dropped here.
        if dropped {
            for (_, from, why) in &refused {
                crate::log::warn("background", &format!("{}: dropped text queued by {from}: {why}", crate::agents::label(id)));
            }
            let told: Vec<(String, String)> = refused.into_iter().map(|(_, from, why)| (from, why)).collect();
            crate::agents::queued_refused(id, &told);
        }
        match step {
            Step::Run(batch) => return Some(batch),
            Step::Again(next) => res = next,
            Step::Idle | Step::Released => {
                reg().1.notify_all();
                changed();
                return None;
            }
            Step::Lost => return None,
        }
    }
}

/// Takes the helper's run for a turn about to start, under the same lock that
/// found it free: text that arrives before the process is up queues behind
/// it rather than starting a second process on the same conversation. A turn
/// beginning is activity: a power-off's quiet window starts over (the
/// activity lock is taken under the registry's, never the other way round).
/// Every claim but the first turn's (`start_on`) and one taken again for a
/// claim given back in the same hold has passed its line's places first
/// (places.rs `room_for_turn`).
pub(super) fn claim(e: &mut Entry) {
    e.run = Some(Run::claimed(None, crate::throttle::now_ms()));
    e.left_seen_out = false;
    e.waits_since = None;
    crate::activity::touch();
}

/// Where text sent to a helper went.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Placed {
    /// It starts a turn.
    Started,
    /// It waits for the turn after the one running.
    Queued,
    /// It is on the stdin of the turn running, which ends once that has answered
    /// it (Claude Code, at its next tool boundary), or of a lead's CLI kept up
    /// past its answer by its own background tasks (events.rs `listening`),
    /// which answers it in a turn of its own as it takes it in.
    Joined,
    /// It is queued for a turn that waits for a place on its line (places.rs):
    /// none is running, and the next to start is the one on it.
    Waits,
}

/// Onto the stdin of the turn running, if that still takes text and nothing
/// waits ahead of it: what came first goes first. `q` is back if it cannot go.
// The error is the caller's own message handed back whole, to be queued
// instead; boxing it would only move it to the heap and out again.
#[allow(clippy::result_large_err)]
fn join(e: &mut Entry, q: Queued) -> Result<(), Queued> {
    if !e.queue.is_empty() {
        return Err(q);
    }
    write_line(e, q)
}

/// `q` as a line on its stdin, if that is still open, kept in `fed` until
/// the CLI takes it in: Claude Code's stream-json message, or a Codex
/// server's `turn/steer` once its turn has an id and is not being stopped
/// (appserver.rs). `q` is back if it cannot go.
#[allow(clippy::result_large_err)]
pub(super) fn write_line(e: &mut Entry, mut q: Queued) -> Result<(), Queued> {
    let Some(run) = e.run.as_ref() else { return Err(q) };
    let Some(feed) = run.feed.as_ref() else { return Err(q) };
    let line = match &run.rpc {
        None => user_line(&q.id, &q.text),
        Some(rpc) => match (rpc.thread.as_deref(), rpc.steerable()) {
            (Some(thread), Some(turn)) => {
                q.sent = true;
                steer_line(thread, turn, &q)
            }
            _ => return Err(q),
        },
    };
    q.line = Some(q.id.clone());
    if feed.send(line).is_err() {
        q.line = None;
        return Err(q);
    }
    note(e, &format!("joined the turn: {}", one_line(&q.text)));
    e.fed.push(q);
    // At work on a turn again, as the CLI is once it has the line: nothing
    // closes this stdin under it before that turn's own `result`.
    if let Some(run) = e.run.as_mut().filter(|run| run.listening_since.is_some()) {
        run.after_result = false;
    }
    Ok(())
}

/// Puts text where it goes: on the turn running if it may `join` that, behind it,
/// or for the turn now claimed, if `room` (places.rs `room_for_turn`), else for
/// the turn that waits for a place. One with budget lines is `in_flight` as
/// queued text is by then, so they count as arrived when the CLI has it, not
/// before.
pub(super) fn place(e: &mut Entry, q: Queued, may_join: bool, room: bool) -> Placed {
    if e.run.is_some() || e.ending_left || e.held_back() {
        let q = if may_join {
            match join(e, q) {
                Ok(()) => return Placed::Joined,
                Err(q) => q,
            }
        } else {
            q
        };
        note(e, &format!("queued: {}", one_line(&q.text)));
        e.queue.push(q);
        return Placed::Queued;
    }
    // Behind what already waits for a place, in its order: `wake` starts it
    // with that.
    if !room || !e.queue.is_empty() {
        note(e, &format!("queued: {}", one_line(&q.text)));
        e.queue.push(q);
        wait_for_place(e);
        return Placed::Waits;
    }
    claim(e);
    // A Codex turn's text is kept until the server has it, as a batch's is:
    // what a core gone meanwhile, or a refused `turn/start`, hands back.
    if !q.budget.is_empty() || e.spec.provider == Provider::Codex {
        e.in_flight.push(q);
        stamp(&mut e.in_flight);
    }
    Placed::Started
}

/// `send_tracked` without the id, for the tests.
#[cfg(test)]
pub fn send(id: &str, text: &str, from: Option<&str>) -> Result<Placed, String> {
    send_tracked(id, text, from).map(|(placed, _)| placed)
}

/// Text for the helper: the next turn now, into the one running, or after it.
/// With the text's own id (`Queued::id`): what tells whether text queued went
/// onto the running process's stdin after all (events.rs `moved_onto_stdin_at`).
pub fn send_tracked(id: &str, text: &str, from: Option<&str>) -> Result<(Placed, String), String> {
    let q = Queued::new(from.map(str::to_owned), text.to_string(), Vec::new());
    let qid = q.id.clone();
    send_as(id, q, true).map(|(placed, _)| (placed, qid))
}

/// A notice with budget lines in it, or none. It was aimed at a helper found
/// idle, and if a turn has begun since, it waits for that turn's end: what it
/// reports is what the next turn is to act on, and it starts one by itself.
/// Or at a lead found taking notices on stdin while its own background tasks
/// run (events.rs `listening`), which it joins while it still does. With
/// where it went, whether it was queued for a stdin kept open so (events.rs
/// `keeps_listening`): one its CLI's next `result` takes it onto, within the
/// process running (events.rs `feed_queued`), unless that goes first.
pub fn send_marked(id: &str, text: &str, from: Option<&str>, budget: Vec<BudgetMark>) -> Result<(Placed, bool), String> {
    send_as(id, Queued::new(from.map(str::to_owned), text.to_string(), budget), false)
}

/// The text a helper may have queued behind its turn, in bytes, before more
/// is refused: each send is at most the hub's 4 MB, but how many come is up
/// to its callers, and every queued text is kept in memory and in
/// `helpers.json`, written whole at each change.
const QUEUED_MAX: usize = 16 * 1024 * 1024;

/// Whether `text` may join what waits for helper `id`: refused, with the
/// reason, past `QUEUED_MAX`. What waits is its queue and what joined its
/// running turn on stdin that its CLI has not taken in yet (`Entry::fed`,
/// let go as the CLI echoes it): a CLI that stopped reading its stdin would
/// otherwise have every send join a turn that never takes them. Text when
/// nothing waits always may.
pub(super) fn room_for(id: &str, e: &Entry, text: &str) -> Result<(), String> {
    let queued: usize = e.queue.iter().chain(&e.fed).map(|q| q.text.len()).sum();
    if queued > 0 && queued + text.len() > QUEUED_MAX {
        crate::log::warn("background", &format!("refused text for {id}: {} KB queued for it already", queued / 1024));
        return Err(format!(
            "helper {id} has {} KB of text queued already; send again once its turns have taken some of it",
            queued / 1024
        ));
    }
    Ok(())
}

fn send_as(id: &str, q: Queued, may_join: bool) -> Result<(Placed, bool), String> {
    let marked = !q.budget.is_empty();
    let text = q.text.clone();
    let (placed, kept_open) = {
        let mut r = lock();
        // The one check a turn passes for its place, in the hold that claims it.
        let room = room_for_turn(&r, id, q.from.is_none());
        let e = r.entries.get_mut(id).ok_or("no such helper")?;
        if !e.held.is_empty() {
            return Err(format!("helper {id} is waiting on a question or permission: answer it with luna_session_answer"));
        }
        room_for(id, e, &q.text)?;
        let may_join = may_join || listening(e);
        let placed = place(e, q, may_join, room);
        let kept_open = e.run.as_ref().is_some_and(|run| run.feed.is_some() && run.listening_since.is_some());
        (placed, placed == Placed::Queued && kept_open)
    };
    if placed != Placed::Started || marked {
        // At once: a core that goes hard (power, a crash) writes nothing more.
        save();
    }
    if placed == Placed::Started {
        if let Err(err) = run_turn(id, &text) {
            if let Some(e) = lock().entries.get_mut(id) {
                e.in_flight.retain(|q| q.budget.is_empty());
            }
            return Err(err);
        }
    }
    changed();
    Ok((placed, kept_open))
}

/// Where a helper's budget line stands with a background parent.
#[derive(Debug, PartialEq)]
pub enum MarkStands {
    /// In none of what it holds: never given, or dropped with the rest of a queue.
    Nothing,
    /// Queued, with a turn starting on it, or on the stdin of one running.
    Coming,
    /// Kept undelivered: it will not reach the parent by itself.
    Undelivered,
    /// Its CLI's stdin took it.
    Landed,
}

pub fn budget_mark(parent: &str, helper: &str) -> MarkStands {
    let r = lock();
    let Some(e) = r.entries.get(parent) else { return MarkStands::Nothing };
    let holds = |batch: &[Queued]| batch.iter().any(|q| q.budget.iter().any(|m| m.id == helper));
    if e.budgets_landed.iter().any(|h| h == helper) {
        MarkStands::Landed
    } else if holds(&e.queue) || holds(&e.in_flight) || holds(&e.fed) {
        MarkStands::Coming
    } else if holds(&e.undelivered) {
        MarkStands::Undelivered
    } else {
        MarkStands::Nothing
    }
}

/// The agent layer has noted that the line landed.
pub fn budget_mark_noted(parent: &str, helper: &str) {
    if let Some(e) = lock().entries.get_mut(parent) {
        e.budgets_landed.retain(|h| h != helper);
    }
}

/// Takes a helper's line out of what is kept undelivered, as its next notice has it,
/// and with it any message left with nothing but `opening`, the words notices open on.
pub fn drop_undelivered_mark(parent: &str, helper: &str, opening: &str) {
    let mut r = lock();
    let Some(e) = r.entries.get_mut(parent) else { return };
    for q in e.undelivered.iter_mut() {
        let (gone, kept): (Vec<BudgetMark>, Vec<BudgetMark>) = std::mem::take(&mut q.budget).into_iter().partition(|m| m.id == helper);
        q.budget = kept;
        for m in gone {
            q.text = q.text.replace(&m.line, "");
        }
    }
    e.undelivered.retain(|q| {
        let left = q.text.trim();
        !left.is_empty() && left != opening
    });
}

/// What never reached it that `caller` is to hear of (`Entry::undelivered`):
/// what it sent, and for its parent what no session did.
pub fn undelivered_for(id: &str, caller: &str, parent: bool) -> Vec<String> {
    let mine = |q: &Queued| q.from.as_deref().map_or(parent, |f| f == caller);
    let told = |q: &Queued| {
        if q.maybe_reached {
            format!("[This one MAY have reached it before Luna's core went: Luna cannot tell. Check what it did before sending it again.]\n\n{}", q.text)
        } else {
            q.text.clone()
        }
    };
    lock().entries.get(id).map(|e| e.undelivered.iter().filter(|q| mine(q)).map(told).collect()).unwrap_or_default()
}

/// Takes it off, `caller` having been told: `put_back` undoes that.
pub fn take_undelivered_for(id: &str, caller: &str, parent: bool) -> Vec<Queued> {
    let mut r = lock();
    let Some(e) = r.entries.get_mut(id) else { return Vec::new() };
    let (taken, left) =
        std::mem::take(&mut e.undelivered).into_iter().partition(|q: &Queued| q.from.as_deref().map_or(parent, |f| f == caller));
    e.undelivered = left;
    taken
}

pub fn put_back(id: &str, taken: Vec<Queued>) {
    if let Some(e) = lock().entries.get_mut(id) {
        let left = std::mem::take(&mut e.undelivered);
        e.undelivered = [taken, left].concat();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Text past what a helper may have queued is refused, saying why; into
    /// an empty queue any text goes.
    #[test]
    fn text_past_the_queue_bound_is_refused_with_the_reason() {
        let mut e = entry(Provider::Claude);
        assert!(room_for("bq-room", &e, &"x".repeat(QUEUED_MAX + 1)).is_ok(), "an empty queue takes it");
        e.queue.push("y".repeat(QUEUED_MAX - 1).as_str().into());
        assert!(room_for("bq-room", &e, "z").is_ok(), "up to the bound");
        e.queue.push("z".into());
        let refused = room_for("bq-room", &e, "w").unwrap_err();
        assert!(refused.contains("queued already"), "{refused}");
    }

    /// Text that joined a running turn on a stdin its CLI does not read
    /// counts until the CLI takes it: past the bound a send is refused, and
    /// once the CLI has taken it in there is room again.
    #[test]
    fn text_joined_on_a_stdin_never_read_counts_against_the_bound() {
        let mut e = entry(Provider::Claude);
        let _unread = feeding(&mut e);
        let big = "j".repeat(QUEUED_MAX / 2);
        for _ in 0..2 {
            room_for("bq-fed", &e, &big).unwrap();
            assert_eq!(place(&mut e, big.as_str().into(), true, true), Placed::Joined);
        }
        assert!(e.queue.is_empty() && e.fed.len() == 2);
        assert!(room_for("bq-fed", &e, "one more").unwrap_err().contains("queued already"));
        e.fed.clear();
        assert!(room_for("bq-fed", &e, "one more").is_ok(), "taken in: room again");
    }

    #[test]
    fn text_joins_a_turn_with_its_stdin_open_and_goes_first_come_first_served() {
        let mut e = entry(Provider::Claude);
        let lines = feeding(&mut e);
        assert_eq!(place(&mut e, "stop at the next commit".into(), true, true), Placed::Joined);
        let line: Value = serde_json::from_str(&lines.try_recv().unwrap()).unwrap();
        assert_eq!(line["message"]["content"].as_str(), Some("stop at the next commit"));
        assert_eq!(line["uuid"].as_str(), Some(e.fed[0].id.as_str()));
        assert_eq!(e.fed[0].line.as_deref(), Some(e.fed[0].id.as_str()));
        assert!(e.queue.is_empty());

        // Something waits already: it goes behind that, not past it.
        e.queue.push("cancel the commit".into());
        assert_eq!(place(&mut e, "and this".into(), true, true), Placed::Queued);
        assert_eq!(e.queue, ["cancel the commit", "and this"]);
        assert!(lines.try_recv().is_err());
    }

    /// A turn claimed has no stdin yet: what comes then queues, and what comes
    /// once it has one must not overtake that.
    #[test]
    fn what_comes_as_a_turn_starts_is_not_overtaken_once_it_has_started() {
        let mut e = entry(Provider::Claude);
        claim(&mut e);
        assert_eq!(place(&mut e, "do A".into(), true, true), Placed::Queued, "no stdin yet");
        let lines = feeding(&mut e);
        assert_eq!(place(&mut e, "cancel A".into(), true, true), Placed::Queued);
        assert_eq!(e.queue, ["do A", "cancel A"]);
        assert!(lines.try_recv().is_err() && e.fed.is_empty());
    }

    #[test]
    fn a_notice_a_codex_turn_not_yet_started_and_a_closed_stdin_all_queue() {
        let mut e = entry(Provider::Claude);
        let _lines = feeding(&mut e);
        assert_eq!(place(&mut e, "a notice".into(), false, true), Placed::Queued, "aimed at an idle helper");
        // A Codex turn whose `turn/start` has not answered has no id to steer.
        let mut e = entry(Provider::Codex);
        let _lines = feeding(&mut e);
        e.session = Some("t".into());
        e.run.as_mut().unwrap().rpc = Some(Rpc::new(Value::Null, "the task".into(), String::new(), "c".into()));
        assert_eq!(place(&mut e, "text".into(), true, true), Placed::Queued);
        let mut e = entry(Provider::Claude);
        let lines = feeding(&mut e);
        drop(lines);
        assert_eq!(place(&mut e, "its writer is gone".into(), true, true), Placed::Queued);
        assert!(e.fed.is_empty() && e.queue[0].line.is_none());
    }

    /// A stop puts the text by a moment before the last of what the CLI said is read.
    #[test]
    fn its_word_arriving_after_a_stop_takes_the_text_back_off_the_undelivered() {
        let mut e = entry(Provider::Claude);
        let _lines = feeding(&mut e);
        assert_eq!(place(&mut e, "joined".into(), true, true), Placed::Joined);
        let uuid = e.fed[0].id.clone();
        stop_for_luna(&mut e);
        assert_eq!(e.undelivered, ["joined"]);
        assert!(on_claude_event(&mut e, &json!({ "type": "user", "isReplay": true, "uuid": uuid })));
        assert!(e.undelivered.is_empty());
    }

    #[test]
    fn a_stop_for_lunas_sake_keeps_what_joined_and_counts_no_turn_for_it_and_a_kill_drops_it() {
        let mut e = entry(Provider::Claude);
        let _lines = feeding(&mut e);
        assert_eq!(place(&mut e, "joined".into(), true, true), Placed::Joined);
        e.queue.push("behind it".into());
        e.turns_ended = 3;
        stop_for_luna(&mut e);
        assert_eq!(e.undelivered, ["joined", "behind it"]);
        assert!(e.fed.is_empty() && e.run.as_ref().unwrap().feed.is_none());
        assert_eq!(e.turns_ended, 4, "the one for what was queued, none for what joined");

        let mut e = entry(Provider::Claude);
        let _lines = feeding(&mut e);
        assert_eq!(place(&mut e, "joined".into(), true, true), Placed::Joined);
        e.turns_ended = 3;
        stop(&mut e);
        assert!(e.fed.is_empty() && e.undelivered.is_empty());
        assert_eq!(e.turns_ended, 3, "its sender waits on this turn's end");
    }

    /// One try more, under the same id, and then back to who sent it: a CLI that
    /// keeps going before it takes anything does not loop.
    #[test]
    fn what_a_cli_went_without_starts_the_next_turn_once() {
        let mut e = entry(Provider::Claude);
        let _lines = feeding(&mut e);
        assert_eq!(place(&mut e, "joined".into(), true, true), Placed::Joined);
        let uuid = e.fed[0].id.clone();
        e.queue.push("behind it".into());
        assert_eq!(sort_out_untaken(&mut e), ["joined"]);
        assert_eq!(e.queue, ["joined", "behind it"], "in the order they came");
        assert!(e.fed.is_empty() && e.undelivered.is_empty());

        // The next process is started on it. Its pipe taking the line retires nothing.
        reserve(&mut e);
        assert_eq!(joined(&e.in_flight), "joined\n\nbehind it");
        assert_eq!(e.in_flight[0].id, uuid, "the id it had");
        assert_eq!(e.in_flight[1].line.as_deref(), Some(uuid.as_str()), "the batch goes by the first's");

        // That one goes too.
        assert!(sort_out_untaken(&mut e).is_empty());
        assert!(e.in_flight.is_empty() && e.queue.is_empty());
        assert_eq!(e.undelivered, ["joined", "behind it"]);
    }

    #[test]
    fn what_joined_behind_a_batch_never_taken_is_not_sent_past_it() {
        let mut e = entry(Provider::Claude);
        e.in_flight = vec!["the batch".into()];
        stamp(&mut e.in_flight);
        let _lines = feeding(&mut e);
        assert_eq!(place(&mut e, "joined".into(), true, true), Placed::Joined);
        assert!(sort_out_untaken(&mut e).is_empty());
        assert_eq!(e.undelivered, ["the batch", "joined"]);
        assert!(e.queue.is_empty());
    }

    #[test]
    fn a_turn_over_takes_up_what_missed_it_and_what_the_cli_took_is_done() {
        let mut e = entry(Provider::Claude);
        let _lines = feeding(&mut e);
        assert_eq!(place(&mut e, "joined".into(), true, true), Placed::Joined);
        lock().entries.insert("missed".into(), e);
        // No spawn gets far: the folder is not there.
        lock().entries.get_mut("missed").unwrap().spec.folder = "C:\\luna-no-such-folder".into();
        turn_over("missed", Some(1));
        let e = lock().entries.remove("missed").unwrap();
        assert_eq!(e.turns_ended, 2, "the turn, and the next that could not start");
        assert_eq!(e.undelivered, ["joined"], "not dropped with the turn that could not start");
    }

    #[test]
    fn stable_ids_cross_a_core_and_an_older_file_gets_them() {
        let mut e = entry(Provider::Claude);
        let _lines = feeding(&mut e);
        assert_eq!(place(&mut e, "joined".into(), true, true), Placed::Joined);
        let uuid = e.fed[0].id.clone();
        lock().entries.insert("carried".into(), e);
        let bytes = serde_json::to_vec(&snapshot().into_iter().filter(|s| s.id == "carried").collect::<Vec<Saved>>()).unwrap();
        lock().entries.remove("carried");
        let saved: Vec<Saved> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(saved[0].fed[0].id, uuid);
        assert_eq!(saved[0].fed[0].line.as_deref(), Some(uuid.as_str()));
        let e = taken_up(saved.into_iter().next().unwrap());
        assert_eq!(e.undelivered, ["joined"]);
        assert_eq!(e.turns_ended, 1, "the turn; none for what joined it");

        let old: Queued = serde_json::from_value(json!({ "from": null, "text": "older" })).unwrap();
        assert_eq!(old.id.len(), 36);
        assert!(old.line.is_none() && !old.maybe_reached);
    }

    #[test]
    fn queued_text_a_cli_reads_as_a_command_is_asked_about_again_when_its_turn_comes() {
        use crate::agents::{forget, news_for, register};
        let mode = |m: &str| serde_json::json!({ "permissionMode": m });
        register("bq-top", Provider::Claude, "C:/p", "C:/acc/one", None, true, mode("bypassPermissions"));
        register("bq-full", Provider::Claude, "C:/p", "C:/acc/one", Some("bq-top"), true, mode("bypassPermissions"));
        register("bq-kid", Provider::Claude, "C:/p", "C:/acc/one", Some("bq-full"), false, mode("default"));
        let queued = |from: &str, text: &str| Queued::new(Some(from.to_string()), text.to_string(), Vec::new());
        lock().entries.insert("bq-kid".into(), entry(Provider::Claude));
        // As a turn's end has it: claimed and reserved under the lock, its
        // senders asked with the lock let go.
        let reserved = |items: Vec<Queued>| {
            let mut r = lock();
            let e = r.entries.get_mut("bq-kid").unwrap();
            e.run = None;
            e.queue.extend(items);
            claim(e);
            reserve(e)
        };
        let take = |items: Vec<Queued>| settle_batch("bq-kid", reserved(items)).map(|b| b.text);

        // Taken while the helper was its sender's, and still so: it goes.
        assert_eq!(take(vec![queued("bq-full", "/project-command")]), Some("/project-command".into()));
        assert!(news_for("bq-full").is_empty());

        // Queued by a session the helper is not under (it was, until a move):
        // all of it refused, whatever mode the sender runs in, no turn, the
        // claim given back, nothing kept for a resume to type in.
        register("bq-other", Provider::Claude, "C:/p", "C:/acc/one", None, true, mode("bypassPermissions"));
        assert_eq!(take(vec![queued("bq-other", "/project-command"), queued("bq-other", "! make")]), None);
        {
            let r = lock();
            let e = &r.entries["bq-kid"];
            assert!(e.run.is_none() && e.in_flight.is_empty() && e.queue.is_empty() && e.undelivered.is_empty());
        }
        // The sender hears of each, as it was told the text was taken.
        let told = news_for("bq-other");
        assert_eq!(told.len(), 2, "{told:?}");
        assert!(told.iter().all(|l| l.contains("Text you sent to helper bq-kid did not go in")), "{told:?}");

        // Mixed: only what may go goes.
        assert_eq!(take(vec![queued("bq-other", "/project-command"), queued("bq-other", "plain words")]), Some("plain words".into()));
        forget("bq-other");

        // Its sender gone: the helper's parent now hears, without the text. Luna's own goes.
        forget("bq-full");
        assert_eq!(take(vec![queued("bq-full", "! whoami"), Queued::new(None, "a notice".into(), Vec::new())]), Some("a notice".into()));
        let top = news_for("bq-top");
        assert_eq!(top.len(), 1, "{top:?}");
        assert!(top[0].contains("bq-full, since gone") && !top[0].contains("whoami"), "{top:?}");

        // Stopped while its senders were asked: no turn, the claim given back,
        // nobody told of a drop that did not happen.
        let res = reserved(vec![queued("bq-gone", "/x")]);
        stop(lock().entries.get_mut("bq-kid").unwrap());
        assert!(settle_batch("bq-kid", res).is_none());
        assert!(lock().entries["bq-kid"].run.is_none(), "no processless claim left behind");
        assert_eq!(news_for("bq-top").len(), 1);

        lock().entries.remove("bq-kid");
        forget("bq-kid");
        forget("bq-top");
    }

    #[test]
    fn what_lands_between_a_batch_reserved_and_its_senders_asked_is_taken_into_account() {
        let id = "rw-kid";
        let fresh = || {
            let mut e = entry(Provider::Claude);
            e.queue.push(Queued::new(None, "first".into(), Vec::new()));
            lock().entries.insert(id.into(), e);
            let mut r = lock();
            let e = r.entries.get_mut(id).unwrap();
            claim(e);
            reserve(e)
        };

        // Killed meanwhile: the batch went with the stop, the claim goes back,
        // its turn counted as ended, and the next text is not stuck behind it.
        let res = fresh();
        let ended = lock().entries[id].turns_ended;
        stop(lock().entries.get_mut(id).unwrap());
        assert!(settle_batch(id, res).is_none());
        {
            let r = lock();
            let e = &r.entries[id];
            assert!(e.run.is_none(), "no processless claim left behind");
            assert_eq!(e.turns_ended, ended + 1);
        }

        // Killed, then sent text, which queues behind the stopped claim: that
        // turn counts as ended and the text starts the next, as after any turn.
        let res = fresh();
        let ended = lock().entries[id].turns_ended;
        stop(lock().entries.get_mut(id).unwrap());
        lock().entries.get_mut(id).unwrap().queue.push(Queued::new(None, "after the kill".into(), Vec::new()));
        let next = settle_batch(id, res).expect("the queued text starts a turn");
        assert_eq!(next.text, "after the kill");
        {
            let r = lock();
            let e = &r.entries[id];
            assert_eq!(e.turns_ended, ended + 1, "the stopped turn is counted, the new one not yet");
            assert!(e.run.as_ref().is_some_and(|run| run.pid.is_none() && !run.stopped), "claimed afresh for it");
            assert!(e.queue.is_empty() && e.in_flight.len() == 1);
        }
        // The same while the kill is still ending what was left running: the
        // claim goes back and the queue waits for the kill to take it up.
        let res = fresh();
        {
            let mut r = lock();
            let e = r.entries.get_mut(id).unwrap();
            stop(e);
            e.ending_left = true;
            e.queue.push(Queued::new(None, "after the kill".into(), Vec::new()));
        }
        assert!(settle_batch(id, res).is_none());
        {
            let r = lock();
            assert!(r.entries[id].run.is_none() && r.entries[id].queue.len() == 1);
        }

        // Text sent meanwhile is queued behind the claim: the batch runs, it waits.
        let res = fresh();
        lock().entries.get_mut(id).unwrap().queue.push(Queued::new(None, "later".into(), Vec::new()));
        assert_eq!(settle_batch(id, res).map(|b| b.text).as_deref(), Some("first"));
        assert_eq!(lock().entries[id].queue, ["later"]);

        // Deleted meanwhile: nothing to settle, nothing made again.
        let res = fresh();
        lock().entries.remove(id);
        assert!(settle_batch(id, res).is_none());
        assert!(!lock().entries.contains_key(id));

        // Deleted and made again, with a claim of its own: that one is not touched.
        let res = fresh();
        lock().entries.remove(id);
        let mut e = entry(Provider::Claude);
        claim(&mut e);
        lock().entries.insert(id.into(), e);
        assert!(settle_batch(id, res).is_none());
        assert!(lock().entries[id].run.is_some(), "the new claim stays");

        lock().entries.remove(id);
    }
}
