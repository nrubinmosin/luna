//! The places a line has for its helpers at work (agents/spawn
//! `max_descendants`): one check, which every helper turn passes as it is
//! claimed, under this module's lock, the lock that claim is made under.
//!
//! A line is a session at the top (a person's chat, or a helper with no parent)
//! and every helper below it, by `Spec::parent`. A place is taken by a helper
//! with a turn claimed or running (`Entry::run`: its record's `running` or
//! `asking`), and by a spawn under way from the line (`promise_place`), whose
//! helper's first turn takes its place over (`start_on`). A helper between
//! turns takes none, whatever its turns left running. A Claude Code helper
//! whose CLI stays up past its answer for its background work still has its
//! run: it takes its place until that process goes, as its background
//! subagents are model work, and the turns its CLI starts by itself at a
//! task's end pass no check of Luna's. Its stdin stays open meanwhile only
//! while something that may ask runs (a subagent or a workflow; a lead's for
//! any task, for its notices: events.rs `keeps_listening`).
//!
//! A turn with no place free is not refused: its text stays queued, the helper
//! is marked as waiting (`Entry::waits_since`), and turns waiting start in that
//! order, line by line, as places free (`wake`). A turn on Luna's own text (a
//! notice, a wake: `Queued::from` none) is let start past the limit: a lead
//! whose helpers ask it something must not wait for a place they hold.

use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::SeqCst};

/// The places a line has, as last read from the settings (agents/spawn
/// `max_descendants`, which sets it at every read): read here under the lock,
/// where no file is.
static MOST: AtomicUsize = AtomicUsize::new(0);

/// The order turns waiting for a place start in.
static MARK: AtomicU64 = AtomicU64::new(1);

/// The places a line has, as the settings say (agents/spawn `max_descendants`).
pub fn set_most(n: usize) {
    MOST.store(n, SeqCst);
}

fn most() -> usize {
    match MOST.load(SeqCst) {
        // Before the agents' layer has read them (a core taking its helpers up):
        // read once here.
        0 => crate::agents::spawn::max_descendants(),
        n => n,
    }
}

/// One helper as its line counts it.
#[derive(Clone, Debug)]
struct Seat {
    parent: Option<String>,
    at_work: bool,
    waits_since: Option<u64>,
    /// What is queued for it holds Luna's own text (`lunas_own`).
    lunas: bool,
}

/// Every line's helpers and spawns under way at one look: what places are
/// counted from, here under the lock and by the agents' layer off it (`lines`).
#[derive(Clone, Debug, Default)]
pub struct Lines {
    seats: HashMap<String, Seat>,
    /// The session each spawn under way was asked by.
    promised: Vec<String>,
}

/// Where a line stands: its helpers at work, by id; its spawns under way; its
/// turns waiting for a place.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Taken {
    pub at_work: Vec<String>,
    pub promised: usize,
    pub waiting: usize,
}

impl Taken {
    /// The places in use: a session counted once, as its helper or as its spawn.
    pub fn in_use(&self) -> usize {
        self.at_work.len() + self.promised
    }
}

impl Lines {
    fn of(r: &Registry) -> Lines {
        let seats = r
            .entries
            .iter()
            .map(|(id, e)| {
                let seat = Seat {
                    parent: e.spec.parent.clone(),
                    at_work: e.run.is_some(),
                    waits_since: waiting_since(e),
                    lunas: lunas_own(&e.queue),
                };
                (id.clone(), seat)
            })
            .collect();
        Lines { seats, promised: r.promised.values().cloned().collect() }
    }

    fn parent(&self, id: &str) -> Option<&str> {
        self.seats.get(id)?.parent.as_deref()
    }

    /// The session at the top of `id`'s line: up its helpers' parents to the
    /// first that is no helper, or a helper with no parent.
    pub fn top(&self, id: &str) -> String {
        let mut cur = id;
        for _ in 0..16 {
            match self.parent(cur) {
                Some(p) => cur = p,
                None => break,
            }
        }
        cur.to_string()
    }

    /// Whether helper `id` is below `of`, any generation.
    pub fn below(&self, of: &str, id: &str) -> bool {
        let mut cur = id;
        for _ in 0..16 {
            match self.parent(cur) {
                Some(p) if p == of => return true,
                Some(p) => cur = p,
                None => return false,
            }
        }
        false
    }

    /// What takes places on the line `top` heads, and what waits for one.
    pub fn taken(&self, top: &str) -> Taken {
        let in_line = |id: &str| self.below(top, id);
        let mut at_work: Vec<String> = self.seats.iter().filter(|(id, s)| s.at_work && in_line(id)).map(|(id, _)| id.clone()).collect();
        at_work.sort();
        let promised = self.promised_under(top);
        let waiting = self.seats.iter().filter(|(id, s)| s.waits_since.is_some() && in_line(id)).count();
        Taken { at_work, promised, waiting }
    }

    /// The spawns under way asked by `of` or a helper below it.
    pub fn promised_under(&self, of: &str) -> usize {
        self.promised.iter().filter(|c| c.as_str() == of || self.below(of, c)).count()
    }

    /// Whether helper `id` may start a turn with `most` places on its line: the
    /// places in use there, it aside, and the turns that have waited longer
    /// than it for one, leave one free. A line's own top takes no place of it,
    /// and a turn on Luna's own text starts past the limit.
    pub fn room_for(&self, id: &str, most: usize) -> bool {
        let top = self.top(id);
        if top == id || self.seats.get(id).is_some_and(|s| s.lunas) {
            return true;
        }
        let taken = self.taken(&top);
        let others = taken.at_work.iter().filter(|a| *a != id).count();
        let mine = self.seats.get(id).and_then(|s| s.waits_since);
        let ahead = self
            .seats
            .iter()
            .filter(|(other, _)| *other != id && self.below(&top, other))
            .filter_map(|(_, s)| s.waits_since)
            .filter(|since| mine.is_none_or(|m| *since < m))
            .count();
        others + taken.promised + ahead < most
    }

    /// The helpers waiting for a place that may start now, longest waiting first.
    fn startable(&self, most: usize) -> Vec<String> {
        let mut waiting: Vec<(u64, &String)> = self.seats.iter().filter_map(|(id, s)| Some((s.waits_since?, id))).collect();
        waiting.sort();
        waiting.into_iter().filter(|(_, id)| self.room_for(id, most)).map(|(_, id)| id.clone()).collect()
    }
}

/// Since when helper `e` waits for a place, if it still has a turn to start:
/// text queued, or a turn a core's going cut short to go on with.
fn waiting_since(e: &Entry) -> Option<u64> {
    let has_turn = !e.queue.is_empty() || (e.cut_short && e.resumable);
    e.waits_since.filter(|_| e.run.is_none() && has_turn && !e.held_back() && !e.ending_left)
}

/// Every line as it stands now, for the agents' layer to count off its own lock.
pub fn lines() -> Lines {
    Lines::of(&lock())
}

/// Whether text in a batch is Luna's own (a notice, a wake), which starts its
/// turn past the limit.
pub(super) fn lunas_own<'a>(batch: impl IntoIterator<Item = &'a Queued>) -> bool {
    batch.into_iter().any(|q| q.from.is_none())
}

/// The check every helper turn passes as it is claimed: whether helper `id`
/// may start one now, on Luna's own text (`lunas_own`) or with a place free.
pub(super) fn room_for_turn(r: &Registry, id: &str, lunas: bool) -> bool {
    lunas || Lines::of(r).room_for(id, most())
}

/// Helper `e`'s next turn waits for a place: marked, once, with its place in
/// the order they start in.
pub(super) fn wait_for_place(e: &mut Entry) {
    if e.waits_since.is_none() {
        e.waits_since = Some(MARK.fetch_add(1, SeqCst));
        note(e, "its next turn waits for a place on its line: every place is taken by a turn at work");
    }
}

/// A place for a spawn under way from `caller` (agents/spawn `SpawnUnderWay`),
/// by its `request`, if its line has one with `most` places: counted with the
/// turns at work and the turns waiting there, as these are ahead of it.
/// Refused, where the line stands.
pub fn promise_place(caller: &str, request: u64, most: usize) -> Result<(), Taken> {
    let mut r = lock();
    let lines = Lines::of(&r);
    let taken = lines.taken(&lines.top(caller));
    if taken.in_use() + taken.waiting >= most {
        return Err(taken);
    }
    r.promised.insert(request, caller.to_string());
    Ok(())
}

/// The spawn `request` is through or gone: its place, if its helper's first
/// turn has not taken it over, is free.
pub fn unpromise(request: u64) {
    let freed = lock().promised.remove(&request).is_some();
    if freed {
        wake();
    }
}

/// A pass over the turns waiting for a place is running (`wake`), and whether
/// another is asked for meanwhile.
static PASSING: AtomicBool = AtomicBool::new(false);
static AGAIN: AtomicBool = AtomicBool::new(false);

/// Starts the turns waiting for a place that have one now, longest waiting
/// first, on a thread of its own: called whenever a helper changes (`changed`),
/// including a turn's end, a delete or a stop. A spawn giving back its place
/// or a settings change can also free a place.
pub fn wake() {
    if !lock().entries.values().any(|e| waiting_since(e).is_some()) {
        return;
    }
    AGAIN.store(true, SeqCst);
    if PASSING.swap(true, SeqCst) {
        return;
    }
    // A test's helpers have no CLI to start: what would start is asked of
    // `startable` there.
    if cfg!(test) {
        PASSING.store(false, SeqCst);
        return;
    }
    let spawned = std::thread::Builder::new().name("luna-places".into()).spawn(|| {
        let mut last = Vec::new();
        loop {
            AGAIN.store(false, SeqCst);
            let ids = Lines::of(&lock()).startable(most());
            // The same as the pass before started nothing: what is left waits
            // for the next change, and `go_on`'s own (`changed`) is none.
            if ids != last {
                for id in &ids {
                    // Asks again under the lock: one started before it may
                    // have taken the place.
                    go_on(id);
                }
                last = ids;
            }
            PASSING.store(false, SeqCst);
            if !AGAIN.load(SeqCst) || PASSING.swap(true, SeqCst) {
                break;
            }
        }
    });
    if spawned.is_err() {
        PASSING.store(false, SeqCst);
    }
}

/// Claims helper `id`'s next turn on what is queued for it, if it is free to
/// start one (no turn, nothing ending what its turns left, no hold) and its
/// line has a place (`room_for_turn`): what was sent meanwhile, or while it
/// waited for a place, is that turn's batch. With no place it waits for one.
pub(super) fn claim_queued(r: &mut Registry, id: &str) -> Option<Reservation> {
    let room = room_for_turn(r, id, false);
    let e = r.entries.get_mut(id)?;
    if e.run.is_some() || e.ending_left || e.held_back() || e.queue.is_empty() {
        return None;
    }
    if !room {
        wait_for_place(e);
        return None;
    }
    claim(e);
    Some(reserve(e))
}

/// What would start now, of the turns waiting, for the tests.
#[cfg(test)]
pub(crate) fn startable_for_test(most: usize) -> Vec<String> {
    Lines::of(&lock()).startable(most)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn helper(r: &mut Registry, id: &str, parent: &str, at_work: bool) {
        let mut e = entry(Provider::Claude);
        e.spec.parent = Some(parent.to_string());
        if at_work {
            claim(&mut e);
        }
        r.entries.insert(id.to_string(), e);
    }

    fn registry() -> Registry {
        Registry {
            entries: HashMap::new(),
            next_serial: 1,
            asked: HashMap::new(),
            closed: false,
            parent_changes: HashMap::new(),
            promised: HashMap::new(),
        }
    }

    /// Only a turn at work and a spawn under way take a place, each session
    /// once; a lead's helpers count on its chat's line; a helper between turns,
    /// with what its turns left running, takes none.
    #[test]
    fn a_line_counts_its_turns_at_work_and_its_spawns_under_way() {
        let mut r = registry();
        helper(&mut r, "lead", "chat", true);
        helper(&mut r, "a", "lead", true);
        helper(&mut r, "b", "lead", false);
        r.entries.get_mut("b").unwrap().left_running = true;
        helper(&mut r, "c", "chat", false);
        helper(&mut r, "other", "chat-2", true);
        r.promised.insert(7, "lead".into());
        let lines = Lines::of(&r);
        assert_eq!(lines.top("a"), "chat");
        assert_eq!(lines.taken("chat"), Taken { at_work: vec!["a".into(), "lead".into()], promised: 1, waiting: 0 });
        assert_eq!(lines.taken("chat").in_use(), 3);
        assert_eq!(lines.taken("chat-2").in_use(), 1, "another line's are its own");
        // With three places, the idle ones may not start; with four, one may.
        assert!(!lines.room_for("b", 3) && !lines.room_for("c", 3));
        assert!(lines.room_for("b", 4));
        // One at work asks for its next turn as its own ends: its place is its own.
        assert!(lines.room_for("a", 3));
        // The spawn's helper takes its place over: still three.
        r.promised.remove(&7);
        helper(&mut r, "new", "lead", true);
        assert_eq!(Lines::of(&r).taken("chat").in_use(), 3);
        // A helper heading its own line (no parent) takes no place of it.
        let mut e = entry(Provider::Claude);
        e.spec.parent = None;
        r.entries.insert("orphan".into(), e);
        assert!(Lines::of(&r).room_for("orphan", 1));
    }

    /// Turns waiting for a place start in the order they began to wait, and one
    /// that has not waited yet goes behind them.
    #[test]
    fn turns_waiting_for_a_place_start_in_order_as_places_free() {
        let mut r = registry();
        helper(&mut r, "busy-1", "chat", true);
        helper(&mut r, "busy-2", "chat", true);
        for id in ["w-late", "w-early", "fresh"] {
            helper(&mut r, id, "chat", false);
            r.entries.get_mut(id).unwrap().queue.push(Queued::new(Some("chat".into()), "go".into()));
        }
        r.entries.get_mut("w-early").unwrap().waits_since = Some(1);
        r.entries.get_mut("w-late").unwrap().waits_since = Some(2);
        // Two places, both taken: nothing starts.
        assert!(Lines::of(&r).startable(2).is_empty());
        assert!(!Lines::of(&r).room_for("w-early", 2));
        // One frees: the longest waiting, alone.
        r.entries.get_mut("busy-1").unwrap().run = None;
        assert_eq!(Lines::of(&r).startable(2), ["w-early"]);
        assert!(!Lines::of(&r).room_for("w-late", 2) && !Lines::of(&r).room_for("fresh", 2), "both behind it");
        // It starts, and the other one frees: the next in line.
        claim(r.entries.get_mut("w-early").unwrap());
        r.entries.get_mut("w-early").unwrap().waits_since = None;
        r.entries.get_mut("busy-2").unwrap().run = None;
        assert_eq!(Lines::of(&r).startable(2), ["w-late"]);
        // A spawn under way counts ahead of none of them, but takes its place.
        r.promised.insert(1, "chat".into());
        assert!(Lines::of(&r).startable(2).is_empty());
        // Held (being deleted or opened as a chat): it does not wait, and
        // holds up nobody.
        r.promised.clear();
        r.entries.get_mut("w-late").unwrap().deleting = true;
        assert!(Lines::of(&r).startable(2).is_empty());
        assert!(Lines::of(&r).room_for("fresh", 2));
    }

    /// A notice into a lead (its helper's answer, a question one asks it) is
    /// not held for a place its helpers hold: the lead would never hear them.
    #[test]
    fn a_notice_into_a_lead_starts_its_turn_past_the_limit() {
        let mut r = registry();
        helper(&mut r, "busy", "chat", true);
        helper(&mut r, "lead", "chat", false);
        assert!(!Lines::of(&r).room_for("lead", 1), "a session's text waits");
        let lead = r.entries.get_mut("lead").unwrap();
        lead.queue.push(Queued::new(Some("chat".into()), "and this".into()));
        lead.queue.push(Queued::new(None, "[Luna notice] your helper answered".into()));
        lead.waits_since = Some(5);
        assert!(Lines::of(&r).room_for("lead", 1));
        assert_eq!(Lines::of(&r).startable(1), ["lead"]);
        assert!(room_for_turn(&r, "lead", true));
    }

    /// The helpers of the tests below, in the one registry the gate reads.
    fn global(prefix: &str, busy: usize, idle: &[&str]) -> Vec<String> {
        let mut r = lock();
        let chat = format!("{prefix}-chat");
        let mut ids: Vec<String> = (0..busy).map(|i| format!("{prefix}-busy-{i}")).collect();
        for id in &ids {
            helper(&mut r, id, &chat, true);
        }
        for id in idle {
            helper(&mut r, id, &chat, false);
            ids.push(id.to_string());
        }
        ids
    }

    fn gone(ids: &[String]) {
        let mut r = lock();
        for id in ids {
            r.entries.remove(id);
        }
    }

    fn ours(prefix: &str, ids: Vec<String>) -> Vec<String> {
        ids.into_iter().filter(|id| id.starts_with(prefix)).collect()
    }

    /// Text for an idle helper on a full line is taken and queued, not refused;
    /// its turn starts, on all of it, once a turn at work ends.
    #[test]
    fn a_send_to_an_idle_helper_on_a_full_line_waits_for_a_place() {
        let most = most();
        let ids = global("pw", most, &["pw-idle", "pw-idle-2"]);
        assert_eq!(send("pw-idle", "go on", Some("pw-chat")), Ok(Placed::Waits));
        assert_eq!(send("pw-idle-2", "and you", Some("pw-chat")), Ok(Placed::Waits));
        assert_eq!(send("pw-idle", "and then this", Some("pw-chat")), Ok(Placed::Waits), "behind what waits");
        {
            let r = lock();
            let e = &r.entries["pw-idle"];
            assert!(e.run.is_none() && e.queue.len() == 2 && e.waits_since.is_some());
        }
        assert!(ours("pw-", startable_for_test(most)).is_empty());
        // A turn ends: the first to wait, alone, may start.
        lock().entries.get_mut("pw-busy-0").unwrap().run = None;
        assert_eq!(ours("pw-", startable_for_test(most)), ["pw-idle"]);
        // Started as `wake` starts it (`go_on`), on its whole queue.
        assert!(claim_queued(&mut lock(), "pw-idle").is_some());
        {
            let r = lock();
            let e = &r.entries["pw-idle"];
            assert!(e.run.is_some() && e.waits_since.is_none() && e.in_flight.len() == 2);
        }
        assert!(claim_queued(&mut lock(), "pw-idle-2").is_none(), "the line is full again");
        gone(&ids);
    }

    /// Turns claimed at once, from as many threads, take no more places than
    /// are free: each is counted in the hold that claims it.
    #[test]
    fn turns_claimed_at_once_take_no_more_places_than_are_free() {
        let most = most();
        let idle: Vec<String> = (0..6).map(|i| format!("pc-idle-{i}")).collect();
        let names: Vec<&str> = idle.iter().map(String::as_str).collect();
        let ids = global("pc", most - 2, &names);
        for id in &idle {
            lock().entries.get_mut(id).unwrap().queue.push(Queued::new(Some("pc-chat".into()), "go".into()));
        }
        let threads: Vec<_> = idle.iter().cloned().map(|id| std::thread::spawn(move || claim_queued(&mut lock(), &id).is_some())).collect();
        let started = threads.into_iter().map(|t| t.join().unwrap()).filter(|s| *s).count();
        assert_eq!(started, 2);
        let r = lock();
        assert_eq!(Lines::of(&r).taken("pc-chat").at_work.len(), most);
        assert_eq!(idle.iter().filter(|id| r.entries[*id].waits_since.is_some()).count(), 4, "the rest wait their turn");
        drop(r);
        gone(&ids);
    }

    /// A turn a core's going cut short goes on after a restart only with a
    /// place on its line, as the settings say then; it waits for one meanwhile.
    #[test]
    fn a_turn_cut_short_by_a_restart_resumes_only_with_a_place() {
        let most = most();
        let ids = global("pr", most, &["pr-cut"]);
        {
            let mut r = lock();
            let e = r.entries.get_mut("pr-cut").unwrap();
            e.cut_short = true;
            e.resumable = true;
            e.session = Some("s-1".into());
            e.undelivered.push(Queued::new(Some("pr-chat".into()), "the rest".into()));
        }
        resume("pr-cut");
        {
            let r = lock();
            let e = &r.entries["pr-cut"];
            assert!(e.run.is_none() && e.waits_since.is_some() && e.undelivered.len() == 1, "kept, waiting");
        }
        lock().entries.get_mut("pr-busy-0").unwrap().run = None;
        assert_eq!(ours("pr-", startable_for_test(most)), ["pr-cut"], "the next to go on");
        gone(&ids);
    }

    #[test]
    fn lunas_own_text_is_a_notice_or_a_wake() {
        let from = |f: Option<&str>| Queued::new(f.map(str::to_owned), "x".into());
        assert!(lunas_own(&[from(Some("lead")), from(None)]));
        assert!(!lunas_own(&[from(Some("lead"))]));
    }
}
