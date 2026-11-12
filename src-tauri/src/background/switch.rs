//! A lead's handoff to a fresh conversation of its own, as this module carries
//! it out (agents/handoff/lead.rs decides and records): the switch scheduled
//! by its call, the opening started on no conversation, the conversation its
//! CLI names taken as the new one, and a switch given up, before its start or
//! after a start that named none. A headless helper has no conversation to
//! clear: its next turn simply does not resume the last one.

use super::*;

/// Whether `id` is a lead: a helper spawned with Luna's tools, its role
/// (command.rs `role_of`).
pub fn is_lead(id: &str) -> bool {
    lock().entries.get(id).is_some_and(|e| e.spec.tools)
}

/// What lead.rs looks at, at a lead's turn's end and as it is called.
#[derive(Clone, Debug)]
pub struct LeadView {
    pub provider: Provider,
    pub session: Option<String>,
    pub switch: Option<LeadSwitch>,
    /// No turn runs or is claimed.
    pub idle: bool,
    /// Idle, with nothing waiting to go in, nothing held, nothing its turns
    /// left running, and nobody opening, deleting or stopping it.
    pub at_rest: bool,
    /// What its turns left running, as last sampled (`Entry::left`).
    pub left: Vec<String>,
    /// `codex exec` interrupted subagents of its last turn as that turn ended.
    pub cut_off: bool,
    /// Its last turn was stopped by `luna_session_stop` or the window.
    pub stopped: bool,
    pub error: Option<String>,
    /// Nothing says an earlier core's turn ended (`Entry::unsure`).
    pub unsure: bool,
    pub turns_ended: u32,
    /// It can write no file: Claude Code without its edit tools or in plan
    /// mode, Codex in a sandbox that writes nothing.
    pub unwritable: bool,
}

pub fn lead_view(id: &str) -> Option<LeadView> {
    let r = lock();
    let e = r.entries.get(id).filter(|e| e.spec.tools)?;
    let idle = e.run.is_none();
    let o = &e.observed;
    Some(LeadView {
        provider: e.spec.provider,
        session: e.session.clone(),
        switch: e.switch.clone(),
        idle,
        at_rest: idle
            && e.queue.is_empty()
            && e.in_flight.is_empty()
            && e.held.is_empty()
            && e.left.is_empty()
            && !e.left_running
            && !e.ending_left
            && !e.promoting
            && !e.deleting
            && !e.handing
            && !r.closed,
        left: e.left.iter().map(Leftover::label).collect(),
        cut_off: o.background_stopped && o.stopped_turn == e.turns_ended && o.stop_cause == StopCause::TurnEnd,
        stopped: e.error.as_deref() == Some(STOPPED_BY_STOP),
        error: e.error.clone(),
        unsure: e.unsure,
        turns_ended: e.turns_ended,
        unwritable: unwritable(&e.spec),
    })
}

/// Whether a helper run with `s` can write no file, its handoff included.
pub(super) fn unwritable(s: &Spec) -> bool {
    match s.provider {
        Provider::Claude => s.read_only || s.mode == "plan",
        Provider::Codex => !s.auto_review && !crate::codex::launch::writes_files(&s.mode),
    }
}

/// The switch on `id`, if one is under way.
pub fn switch_of(id: &str) -> Option<LeadSwitch> {
    lock().entries.get(id)?.switch.clone()
}

/// The leads with a switch on record, for a core taking them up.
pub fn switching() -> Vec<String> {
    lock().entries.iter().filter(|(_, e)| e.switch.is_some()).map(|(id, _)| id.clone()).collect()
}

/// Records `s` as `id`'s switch, its call answered: only on the conversation
/// that called (`s.before`), with none under way. From here what is sent to
/// it queues (`place`). Saved before it counts.
pub fn schedule_switch(id: &str, s: LeadSwitch) -> Result<(), String> {
    {
        let mut r = lock();
        let e = r.entries.get_mut(id).ok_or("no such helper")?;
        if let Some(under_way) = &e.switch {
            return Err(format!("handoff {} is under way already; end your turn", under_way.n));
        }
        if e.session != s.before {
            return Err("this helper went on in another conversation meanwhile; nothing will switch".into());
        }
        note(e, &format!("handoff {} scheduled from {}", s.n, s.source));
        e.switch = Some(s);
    }
    save();
    changed();
    Ok(())
}

/// Starts `id`'s fresh conversation on `opening`, for its switch `n`: from a
/// switch scheduled on its conversation, which it lets go of, or one a core
/// took up with no conversation yet named (`Fresh`), whose earlier opening
/// is dropped from what would go back to its senders. Only between turns,
/// with nobody opening, deleting or stopping it. No `--resume`, no `resume`:
/// the turn starts one (command.rs), in the worktree the first turn made.
/// Err with why, the switch left as it is for lead.rs to give up.
pub fn begin_fresh(id: &str, n: u32, opening: String, previous: Option<String>) -> Result<(), String> {
    let res = {
        let mut r = lock();
        if r.closed {
            return Err("Luna is stopping".into());
        }
        let e = r.entries.get_mut(id).ok_or("no such helper")?;
        if e.run.is_some() || e.ending_left || e.promoting || e.deleting || e.handing {
            return Err("it is busy (a turn, a stop, or being opened as a chat or deleted)".into());
        }
        let Some(s) = e.switch.as_mut().filter(|s| s.n == n && s.phase != LeadPhase::Done) else {
            return Err(format!("handoff {n} is no longer under way"));
        };
        match s.phase {
            LeadPhase::Scheduled => {
                s.before = e.session.take();
                s.previous = previous;
            }
            _ => {
                if let Some(old) = s.opening_id.take() {
                    e.undelivered.retain(|q| q.id != old);
                }
                e.session = None;
            }
        }
        let q = Queued::new(None, opening, Vec::new());
        s.phase = LeadPhase::Fresh;
        s.opening_id = Some(q.id.clone());
        e.opening = None;
        e.opening_id = None;
        note(e, &format!("handoff {n}: a fresh conversation starts on Luna's opening"));
        claim(e);
        e.in_flight = vec![q];
        stamp(&mut e.in_flight);
        reservation_of(e, None)
    };
    // On disk before the CLI has it, as any batch (`Entry::in_flight`).
    save();
    let Some(Batch { text, .. }) = settle_batch(id, res) else { return Err("its turn was stopped before it began".into()) };
    save();
    start_turn(id, &text)?;
    changed();
    Ok(())
}

/// The turn on `text`, on the run claimed for it. A test starts no CLI: the
/// claim stays, with the text as its batch, as a turn just begun has it.
fn start_turn(id: &str, text: &str) -> Result<(), String> {
    #[cfg(not(test))]
    return run_turn(id, text);
    #[cfg(test)]
    {
        if let Some(e) = lock().entries.get_mut(id).filter(|e| e.in_flight.is_empty()) {
            e.in_flight.push(Queued::new(None, text.to_string(), Vec::new()));
        }
        Ok(())
    }
}

/// `id`'s switch `n` given up before its fresh start: it stays on the
/// conversation that called, and what waited goes in. The switch, for lead.rs
/// to record.
pub fn drop_switch(id: &str, n: u32) -> Option<LeadSwitch> {
    let s = {
        let mut r = lock();
        let e = r.entries.get_mut(id)?;
        let s = e.switch.take_if(|s| s.n == n && s.phase == LeadPhase::Scheduled)?;
        note(e, &format!("handoff {n} given up: it goes on in the same conversation"));
        s
    };
    save();
    go_on(id);
    Some(s)
}

/// `id`'s switch `n` given up after a fresh start that named no conversation
/// (it could not start, or ended first): back on the conversation it left,
/// which nothing changed, and what waited goes in.
pub fn revert_switch(id: &str, n: u32) -> Option<LeadSwitch> {
    let s = {
        let mut r = lock();
        let e = r.entries.get_mut(id)?;
        if e.run.is_some() || e.session.is_some() {
            return None;
        }
        let s = e.switch.take_if(|s| s.n == n && s.phase == LeadPhase::Fresh)?;
        if let Some(opening) = &s.opening_id {
            e.undelivered.retain(|q| q.id != *opening);
            e.in_flight.retain(|q| q.id != *opening);
        }
        e.session = s.before.clone();
        e.opening = None;
        e.opening_id = None;
        note(e, &format!("handoff {n} given up: back on the conversation it left"));
        s
    };
    save();
    go_on(id);
    Some(s)
}

/// `id`'s switch `n` through: its record is lead.rs's now.
pub fn clear_switch(id: &str, n: u32) -> Option<LeadSwitch> {
    let s = {
        let mut r = lock();
        let e = r.entries.get_mut(id)?;
        e.switch.take_if(|s| s.n == n && s.phase == LeadPhase::Done)?
    };
    save();
    changed();
    Some(s)
}

/// A lead's fresh start has its conversation once its CLI names one other
/// than the one it left (`init`, `thread.started`).
pub(super) fn lead_named(e: &mut Entry) {
    let Some(sid) = e.session.clone() else { return };
    if let Some(s) = e.switch.as_mut().filter(|s| s.phase == LeadPhase::Fresh && s.before.as_deref() != Some(sid.as_str())) {
        s.phase = LeadPhase::Done;
        s.opened = Some(sid);
    }
}

/// Luna's own text for a lead as a turn of its own (no sender): the next turn
/// now if it is between turns, else queued, `first` ahead of what waits.
pub fn send_luna(id: &str, text: &str, first: bool) -> Result<Placed, String> {
    let q = Queued::new(None, text.to_string(), Vec::new());
    let placed = {
        let mut r = lock();
        let e = r.entries.get_mut(id).ok_or("no such helper")?;
        if first && (e.run.is_some() || e.held_back() || e.ending_left) {
            note(e, &format!("queued first: {}", one_line(text)));
            e.queue.insert(0, q);
            Placed::Queued
        } else {
            place(e, q, false)
        }
    };
    if placed != Placed::Started {
        save();
    } else {
        start_turn(id, text)?;
    }
    changed();
    Ok(placed)
}

/// What a helper's turns left that still runs, by name, as last sampled.
pub fn left_now(id: &str) -> Vec<String> {
    let left: Vec<Leftover> = lock().entries.get(id).map(|e| e.left.clone()).unwrap_or_default();
    left.iter().filter(|l| crate::procs::alive(l.pid)).map(Leftover::label).collect()
}

/// A lead's switch put in place for a test, as its call would.
#[cfg(test)]
pub fn set_switch_for_test(id: &str, s: Option<LeadSwitch>) {
    lock().entries.get_mut(id).unwrap().switch = s;
}

/// A lead between turns for a test, on `session`, its account and folder at
/// `dir` (where a test writes its transcript), in a worktree there if `cwd`.
#[cfg(test)]
pub fn test_lead(id: &str, provider: Provider, dir: &str, session: Option<&str>, cwd: Option<&str>) {
    let mut e = entry(provider);
    e.spec.tools = true;
    e.spec.mode = match provider {
        Provider::Claude => "default".into(),
        Provider::Codex => "workspace-write".into(),
    };
    e.spec.account_path = dir.into();
    e.spec.folder = dir.into();
    e.session = session.map(str::to_owned);
    e.cwd = cwd.map(str::to_owned);
    lock().entries.insert(id.to_string(), e);
}

/// A test's helper gone.
#[cfg(test)]
pub fn remove_for_test(id: &str) {
    lock().entries.remove(id);
}

/// Its CLI names conversation `session`, as an `init` does.
#[cfg(test)]
pub fn name_for_test(id: &str, session: &str) {
    let mut r = lock();
    let e = r.entries.get_mut(id).unwrap();
    e.session = Some(session.to_string());
    lead_named(e);
}

/// Spawned without its edit tools.
#[cfg(test)]
pub fn read_only_for_test(id: &str) {
    lock().entries.get_mut(id).unwrap().spec.read_only = true;
}

/// What a test's lead is, for its assertions: its conversation, its run
/// claimed, the batch of the turn begun, what is queued, what never went in.
#[cfg(test)]
pub fn lead_for_test(id: &str) -> (Option<String>, bool, Vec<String>, Vec<String>, Vec<String>) {
    let r = lock();
    let e = &r.entries[id];
    let texts = |q: &[Queued]| q.iter().map(|q| q.text.clone()).collect();
    (e.session.clone(), e.run.is_some(), texts(&e.in_flight), texts(&e.queue), texts(&e.undelivered))
}

/// A lead's turn over for a test, as `turn_over` leaves it: no run, the batch
/// taken in, what its turns left running as given, how it ended as given.
#[cfg(test)]
pub fn lead_turn_over_for_test(id: &str, left: &[u32], error: Option<&str>) {
    let mut r = lock();
    let e = r.entries.get_mut(id).unwrap();
    e.run = None;
    e.in_flight.clear();
    e.turns_ended += 1;
    e.error = error.map(str::to_owned);
    let now = crate::throttle::now_ms();
    e.left = left
        .iter()
        .map(|&pid| Leftover {
            pid,
            exe: "node".into(),
            started_ms: now,
            cpu_ms: 0,
            io_bytes: 0,
            read_ms: now,
            seen_ms: now,
            moved_ms: now,
        })
        .collect();
    e.left_running = !e.left.is_empty();
}

/// A core's going as a test has it for a lead: its turn cut short, with or
/// without a doubt that it ended, then what `helpers.json` keeps of it read
/// back.
#[cfg(test)]
pub fn lead_across_a_core_for_test(id: &str, unsure: bool) {
    {
        let mut r = lock();
        let e = r.entries.get_mut(id).unwrap();
        e.cut_short = true;
        e.tied = !unsure;
        e.unsure = unsure;
    }
    across_a_core_for_test(id);
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn a_switch(n: u32, phase: LeadPhase) -> LeadSwitch {
        LeadSwitch {
            n,
            copy: format!("C:\\data\\handoffs\\lead\\{n}.md"),
            sha256: "h".into(),
            source: "C:\\w\\handoff.md".into(),
            phase,
            before: Some("old".into()),
            previous: None,
            opening_id: None,
            opened: None,
            from_records: false,
        }
    }

    /// While a lead's handoff is under way, what is sent to it queues: never
    /// on the stdin of the turn that wrote it, nor a turn of its own before
    /// the switch.
    #[test]
    fn text_sent_mid_switch_queues_and_never_joins() {
        let mut e = entry(Provider::Claude);
        e.spec.tools = true;
        let _lines = feeding(&mut e);
        e.switch = Some(a_switch(1, LeadPhase::Scheduled));
        assert_eq!(place(&mut e, "from its helper".into(), true), Placed::Queued);
        assert!(e.fed.is_empty());
        e.run = None;
        assert!(e.held_back(), "between turns the switch is what starts next");
        assert_eq!(place(&mut e, "later".into(), true), Placed::Queued);
        assert!(e.run.is_none(), "nothing started on the conversation it leaves");
        e.switch.as_mut().unwrap().phase = LeadPhase::Done;
        assert!(!e.held_back(), "once the fresh conversation is named it goes on as ever");
    }

    /// The fresh conversation is the one its CLI names first, on either CLI;
    /// the one it left, named again, is not it.
    #[test]
    fn the_fresh_conversation_is_the_one_its_cli_names() {
        let mut e = entry(Provider::Claude);
        e.switch = Some(LeadSwitch { phase: LeadPhase::Fresh, ..a_switch(2, LeadPhase::Fresh) });
        on_claude_event(&mut e, &json!({ "type": "system", "subtype": "init", "session_id": "old", "cwd": "C:\\p" }));
        assert_eq!(e.switch.as_ref().unwrap().phase, LeadPhase::Fresh, "the old one is not the new one");
        e.session = None;
        on_claude_event(&mut e, &json!({ "type": "system", "subtype": "init", "session_id": "new", "cwd": "C:\\p" }));
        let s = e.switch.as_ref().unwrap();
        assert_eq!((s.phase, s.opened.as_deref()), (LeadPhase::Done, Some("new")));

        let mut e = entry(Provider::Codex);
        e.switch = Some(a_switch(3, LeadPhase::Fresh));
        on_codex_event(&mut e, &json!({ "type": "thread.started", "thread_id": "t-new" }));
        let s = e.switch.as_ref().unwrap();
        assert_eq!((s.phase, s.opened.as_deref()), (LeadPhase::Done, Some("t-new")));
        assert_eq!(e.session.as_deref(), Some("t-new"));
    }

    /// The switch is saved with the helper, and a core that takes it up has
    /// it in the same phase, on the same conversation.
    #[test]
    fn a_switch_comes_back_with_its_helper() {
        for phase in [LeadPhase::Scheduled, LeadPhase::Fresh, LeadPhase::Done] {
            let id = format!("sw-saved-{phase:?}");
            let mut e = entry(Provider::Codex);
            e.spec.tools = true;
            e.session = (phase != LeadPhase::Fresh).then(|| "t".to_string());
            e.switch = Some(a_switch(4, phase));
            lock().entries.insert(id.clone(), e);
            across_a_core_for_test(&id);
            assert_eq!(switch_of(&id).map(|s| s.phase), Some(phase));
            lock().entries.remove(&id);
        }
        // helpers.json from before switches reads as none.
        let bare: Saved = serde_json::from_value(serde_json::to_value(with_lines(true)).unwrap()).unwrap();
        assert!(bare.switch.is_none());
    }

    /// A switch given up before its start leaves the conversation as it was;
    /// one given up after a start that named nothing goes back to the one it
    /// left. Either way what waited goes on.
    #[test]
    fn a_switch_given_up_goes_back_to_the_conversation_it_left() {
        let mut e = entry(Provider::Claude);
        e.spec.tools = true;
        e.session = Some("old".into());
        e.switch = Some(a_switch(5, LeadPhase::Scheduled));
        lock().entries.insert("sw-drop".into(), e);
        assert!(drop_switch("sw-drop", 6).is_none(), "another switch's number");
        assert_eq!(drop_switch("sw-drop", 5).map(|s| s.n), Some(5));
        assert_eq!(session_id("sw-drop").as_deref(), Some("old"));
        lock().entries.remove("sw-drop");

        let mut e = entry(Provider::Codex);
        e.spec.tools = true;
        e.session = None;
        let mut s = a_switch(7, LeadPhase::Fresh);
        s.opening_id = Some("opening".into());
        e.undelivered = vec![{
            let mut q = Queued::from("the opening");
            q.id = "opening".into();
            q
        }];
        e.switch = Some(s);
        lock().entries.insert("sw-revert".into(), e);
        assert_eq!(revert_switch("sw-revert", 7).map(|s| s.n), Some(7));
        assert_eq!(session_id("sw-revert").as_deref(), Some("old"));
        assert!(lock().entries["sw-revert"].undelivered.is_empty(), "the opening is nobody's to be told of");
        lock().entries.remove("sw-revert");
    }

    #[test]
    fn a_lead_that_writes_no_file_is_told_from_one_that_does() {
        let mut s = entry(Provider::Claude).spec;
        s.mode = "default".into();
        assert!(!unwritable(&s));
        s.read_only = true;
        assert!(unwritable(&s));
        s.read_only = false;
        s.mode = "plan".into();
        assert!(unwritable(&s));
        let mut s = entry(Provider::Codex).spec;
        for (mode, none) in [("read-only", true), ("read-only-net", true), ("workspace-write", false), ("danger-full-access", false)] {
            s.mode = mode.into();
            assert_eq!(unwritable(&s), none, "{mode}");
        }
        s.mode = "read-only".into();
        s.auto_review = true;
        assert!(!unwritable(&s), "the reviewer's sandbox writes");
    }
}
