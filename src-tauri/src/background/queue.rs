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
/// command may still send it (agents.rs `command_reach_now`): it may have been
/// deleted, run lower or lost the user's setting since it was taken. Then,
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
                // opened as a chat: each takes the queue up itself once the
                // claim has gone.
                Some(e) if ours(e) && stopped(e) => {
                    e.run = None;
                    e.turns_ended += 1;
                    note(e, "stopped before its turn began");
                    if !e.queue.is_empty() && !e.ending_left && !e.promoting {
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
/// it rather than starting a second process on the same conversation.
pub(super) fn claim(e: &mut Entry) {
    e.run = Some(Run::claimed(None, crate::throttle::now_ms()));
}

/// Where text sent to a helper went.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Placed {
    /// It starts a turn.
    Started,
    /// It waits for the turn after the one running.
    Queued,
    /// It is on the stdin of the turn running, which ends once that has answered
    /// it (Claude Code, at its next tool boundary).
    Joined,
}

/// Onto the stdin of the turn running, if that still takes text and nothing
/// waits ahead of it: what came first goes first. `q` is back if it cannot go.
// The error is the caller's own message handed back whole, to be queued
// instead; boxing it would only move it to the heap and out again.
#[allow(clippy::result_large_err)]
fn join(e: &mut Entry, mut q: Queued) -> Result<(), Queued> {
    if !e.queue.is_empty() {
        return Err(q);
    }
    let Some(feed) = e.run.as_ref().and_then(|run| run.feed.as_ref()) else { return Err(q) };
    q.line = Some(q.id.clone());
    if feed.send(user_line(&q.id, &q.text)).is_err() {
        q.line = None;
        return Err(q);
    }
    note(e, &format!("joined the turn: {}", one_line(&q.text)));
    e.fed.push(q);
    Ok(())
}

/// Puts text where it goes: on the turn running if it may `join` that, behind it,
/// or for the turn now claimed. One with budget lines is `in_flight` as queued text
/// is by then, so they count as arrived when the CLI has it, not before.
pub(super) fn place(e: &mut Entry, q: Queued, may_join: bool) -> Placed {
    if e.run.is_some() || e.ending_left || e.promoting {
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
    claim(e);
    if !q.budget.is_empty() {
        e.in_flight.push(q);
        stamp(&mut e.in_flight);
    }
    Placed::Started
}

/// Text for the helper: the next turn now, into the one running, or after it.
pub fn send(id: &str, text: &str, from: Option<&str>) -> Result<Placed, String> {
    send_as(id, Queued::new(from.map(str::to_owned), text.to_string(), Vec::new()), true)
}

/// A notice with budget lines in it, or none. It was aimed at a helper found
/// idle, and if a turn has begun since, it waits for that turn's end: what it
/// reports is what the next turn is to act on, and it starts one by itself.
/// Whether it was queued.
pub fn send_marked(id: &str, text: &str, from: Option<&str>, budget: Vec<BudgetMark>) -> Result<bool, String> {
    send_as(id, Queued::new(from.map(str::to_owned), text.to_string(), budget), false).map(|placed| placed != Placed::Started)
}

fn send_as(id: &str, q: Queued, may_join: bool) -> Result<Placed, String> {
    let marked = !q.budget.is_empty();
    let text = q.text.clone();
    let placed = {
        let mut r = lock();
        let e = r.entries.get_mut(id).ok_or("no such helper")?;
        if !e.held.is_empty() {
            return Err(format!("helper {id} is waiting on a question or permission: answer it with luna_answer"));
        }
        place(e, q, may_join)
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
    Ok(placed)
}

/// Where a helper's budget line stands with a background parent.
#[derive(Debug, PartialEq)]
pub enum MarkStands {
    /// In none of what it holds: never given, or dropped with the rest of a queue.
    Nothing,
    /// Queued, or with a turn starting on it.
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
    } else if holds(&e.queue) || holds(&e.in_flight) {
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

    #[test]
    fn text_joins_a_turn_with_its_stdin_open_and_goes_first_come_first_served() {
        let mut e = entry(Provider::Claude);
        let lines = feeding(&mut e);
        assert_eq!(place(&mut e, "stop at the next commit".into(), true), Placed::Joined);
        let line: Value = serde_json::from_str(&lines.try_recv().unwrap()).unwrap();
        assert_eq!(line["message"]["content"].as_str(), Some("stop at the next commit"));
        assert_eq!(line["uuid"].as_str(), Some(e.fed[0].id.as_str()));
        assert_eq!(e.fed[0].line.as_deref(), Some(e.fed[0].id.as_str()));
        assert!(e.queue.is_empty());

        // Something waits already: it goes behind that, not past it.
        e.queue.push("cancel the commit".into());
        assert_eq!(place(&mut e, "and this".into(), true), Placed::Queued);
        assert_eq!(e.queue, ["cancel the commit", "and this"]);
        assert!(lines.try_recv().is_err());
    }

    /// A turn claimed has no stdin yet: what comes then queues, and what comes
    /// once it has one must not overtake that.
    #[test]
    fn what_comes_as_a_turn_starts_is_not_overtaken_once_it_has_started() {
        let mut e = entry(Provider::Claude);
        claim(&mut e);
        assert_eq!(place(&mut e, "do A".into(), true), Placed::Queued, "no stdin yet");
        let lines = feeding(&mut e);
        assert_eq!(place(&mut e, "cancel A".into(), true), Placed::Queued);
        assert_eq!(e.queue, ["do A", "cancel A"]);
        assert!(lines.try_recv().is_err() && e.fed.is_empty());
    }

    #[test]
    fn a_notice_a_codex_turn_and_a_closed_stdin_all_queue() {
        let mut e = entry(Provider::Claude);
        let _lines = feeding(&mut e);
        assert_eq!(place(&mut e, "a notice".into(), false), Placed::Queued, "aimed at an idle helper");
        let mut e = entry(Provider::Codex);
        e.run = Some(Run::claimed(Some(1), 0));
        assert_eq!(place(&mut e, "text".into(), true), Placed::Queued);
        let mut e = entry(Provider::Claude);
        let lines = feeding(&mut e);
        drop(lines);
        assert_eq!(place(&mut e, "its writer is gone".into(), true), Placed::Queued);
        assert!(e.fed.is_empty() && e.queue[0].line.is_none());
    }

    /// A stop puts the text by a moment before the last of what the CLI said is read.
    #[test]
    fn its_word_arriving_after_a_stop_takes_the_text_back_off_the_undelivered() {
        let mut e = entry(Provider::Claude);
        let _lines = feeding(&mut e);
        assert_eq!(place(&mut e, "joined".into(), true), Placed::Joined);
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
        assert_eq!(place(&mut e, "joined".into(), true), Placed::Joined);
        e.queue.push("behind it".into());
        e.turns_ended = 3;
        stop_for_luna(&mut e);
        assert_eq!(e.undelivered, ["joined", "behind it"]);
        assert!(e.fed.is_empty() && e.run.as_ref().unwrap().feed.is_none());
        assert_eq!(e.turns_ended, 4, "the one for what was queued, none for what joined");

        let mut e = entry(Provider::Claude);
        let _lines = feeding(&mut e);
        assert_eq!(place(&mut e, "joined".into(), true), Placed::Joined);
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
        assert_eq!(place(&mut e, "joined".into(), true), Placed::Joined);
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
        assert_eq!(place(&mut e, "joined".into(), true), Placed::Joined);
        assert!(sort_out_untaken(&mut e).is_empty());
        assert_eq!(e.undelivered, ["the batch", "joined"]);
        assert!(e.queue.is_empty());
    }

    #[test]
    fn a_turn_over_takes_up_what_missed_it_and_what_the_cli_took_is_done() {
        let mut e = entry(Provider::Claude);
        let _lines = feeding(&mut e);
        assert_eq!(place(&mut e, "joined".into(), true), Placed::Joined);
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
        assert_eq!(place(&mut e, "joined".into(), true), Placed::Joined);
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

        // Taken while its sender was at full access, and still so: it goes.
        assert_eq!(take(vec![queued("bq-full", "/project-command")]), Some("/project-command".into()));
        assert!(news_for("bq-full").is_empty());

        // The sender runs lower by the time the turn starts: all of it refused,
        // no turn, the claim given back, nothing kept for a resume to type in.
        register("bq-full", Provider::Claude, "C:/p", "C:/acc/one", Some("bq-top"), true, mode("default"));
        assert_eq!(take(vec![queued("bq-full", "/project-command"), queued("bq-full", "! make")]), None);
        {
            let r = lock();
            let e = &r.entries["bq-kid"];
            assert!(e.run.is_none() && e.in_flight.is_empty() && e.queue.is_empty() && e.undelivered.is_empty());
        }
        // The sender hears of each, as it was told the text was taken.
        let told = news_for("bq-full");
        assert_eq!(told.len(), 2, "{told:?}");
        assert!(told.iter().all(|l| l.contains("Text you sent to helper bq-kid did not go in")), "{told:?}");

        // Mixed: only what may go goes.
        assert_eq!(take(vec![queued("bq-full", "/project-command"), queued("bq-full", "plain words")]), Some("plain words".into()));

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
