//! Helpers across cores: what is written to `<data>/helpers.json`, read back by the
//! next core and taken up again.

use super::*;

/// How often the helpers are written out, when they have changed.
const SAVE_EVERY: std::time::Duration = std::time::Duration::from_secs(2);

/// A helper as `<data>/helpers.json` keeps it, for the core started after
/// this one: between turns a helper is nothing but this and its
/// conversation, which the CLI keeps on disk. What it was told and said is
/// not here, only where to find it.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Saved {
    pub(super) id: String,
    pub(super) spec: Spec,
    pub(super) started_ms: u64,
    pub(super) session: Option<String>,
    pub(super) cwd: Option<String>,
    pub(super) turns_ended: u32,
    /// A turn ran as this was written: should the core go before the next
    /// write, it went mid-turn and stopped it.
    pub(super) running: bool,
    /// Text was queued behind that turn, which goes with it.
    pub(super) queued: bool,
    /// That text, word for word, for its sender to send again (a file an
    /// older core left has only `queued`).
    #[serde(default)]
    pub(super) queue: Vec<Queued>,
    /// See `Entry::in_flight`.
    #[serde(default)]
    pub(super) in_flight: Vec<Queued>,
    /// See `Entry::fed`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) fed: Vec<Queued>,
    /// See `Entry::budgets_landed`.
    #[serde(default)]
    pub(super) budgets_landed: Vec<String>,
    /// See `Entry::undelivered`.
    #[serde(default)]
    pub(super) undelivered: Vec<Queued>,
    /// See `Entry::tied`.
    #[serde(default)]
    pub(super) tied: bool,
    /// See `Entry::unsure`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) unsure: bool,
    /// See `Entry::opening`: written only while no turn has named the
    /// conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) opening: Option<String>,
    /// See `Entry::opening_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) opening_id: Option<String>,
    /// See `Entry::promoting`, since when: an opening the window had not settled
    /// as the core went. The next core holds the helper as this one did, with no
    /// turn starting or resuming, until the window says how its row came out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) promoting_since: Option<u64>,
    pub(super) cut_short: bool,
    pub(super) ties: crate::agents::Ties,
    #[serde(default)]
    pub(super) observed: Observed,
}

fn saved_file() -> std::path::PathBuf {
    crate::paths::data_dir().join("helpers.json")
}

/// Set once `restore` has read the file: before it, an empty registry is
/// not to be written over what the last core left.
static RESTORED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Writes the helpers out if they changed since the last write: every
/// SAVE_EVERY, and at once on what a core killed now must not undo (a spawn, a
/// delete, a question asked). Not to be called with the registry's lock or the
/// agent layer's held.
pub fn save() {
    use std::sync::atomic::Ordering;
    static LAST: Mutex<Vec<u8>> = Mutex::new(Vec::new());
    if !RESTORED.load(Ordering::SeqCst) {
        return;
    }
    // Held from before the reading: a save that read the registry ahead of a
    // delete and wrote behind that delete's own save would bring the helper
    // back, for a core killed before the next one.
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    // The chats' ties go by the same beat, in a file of their own.
    crate::agents::save_chats();
    let Ok(bytes) = serde_json::to_vec_pretty(&snapshot()) else { return };
    if *last == bytes {
        return;
    }
    let path = saved_file();
    match crate::secret::write_private(&path, &bytes) {
        Ok(()) => *last = bytes,
        Err(e) => crate::log::warn("background", &format!("could not save the helpers to {}: {e}", path.display())),
    }
}

/// The registry as `helpers.json` keeps it.
pub(super) fn snapshot() -> Vec<Saved> {
    let mut saved: Vec<Saved> = lock()
        .entries
        .iter()
        .map(|(id, e)| Saved {
            id: id.clone(),
            spec: e.spec.clone(),
            started_ms: e.started_ms,
            session: e.session.clone(),
            cwd: e.cwd.clone(),
            turns_ended: e.turns_ended,
            running: e.run.is_some(),
            queued: !e.queue.is_empty(),
            queue: e.queue.clone(),
            in_flight: e.in_flight.clone(),
            fed: e.fed.clone(),
            budgets_landed: e.budgets_landed.clone(),
            undelivered: e.undelivered.clone(),
            tied: e.tied,
            unsure: e.unsure,
            opening: e.opening.clone().filter(|_| e.session.is_none()),
            opening_id: e.opening_id.clone().filter(|_| e.session.is_none()),
            promoting_since: e.promoting.then_some(e.promoting_since),
            cut_short: e.cut_short,
            observed: e.observed.clone(),
            ties: Default::default(),
        })
        .collect();
    // The agent layer's lock after this one is let go: it is the one taken
    // first where both are held.
    for s in &mut saved {
        s.ties = crate::agents::ties_of(&s.id).unwrap_or_default();
        // Its CLI has said where it works.
        if s.session.is_some() || s.cwd.is_some() {
            s.ties.worktrees_before = None;
        }
    }
    saved.sort_by_key(|s| s.started_ms);
    saved
}

/// Takes up the helpers the last core left, and keeps them saved from here
/// on. A turn that core was in the middle of is over, stopped with it; the
/// caller owed its answer is told so, and given back what was queued behind
/// it. One spawned with `resumeOnRestart` goes on by itself (`resume_stopped`).
/// One stopped before any turn named its conversation is taken up all the
/// same: its next turn starts one, and what it was started on goes back to the
/// caller with the rest, or to that next turn if it resumes by itself.
pub fn restore() {
    let saved: Vec<Saved> = match std::fs::read(saved_file()) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            crate::log::warn("background", &format!("could not read helpers.json: {e}"));
            Vec::new()
        }),
        Err(_) => Vec::new(),
    };
    let mut back = Vec::new();
    let last_ties = crate::procs::end_last_ties();
    for mut s in saved {
        s.tied = s.tied && last_ties.held(&s.id);
        s.unsure = s.unsure && !last_ties.confirmed(&s.id);
        let (account, session) = (s.spec.account_path.clone(), s.session.clone());
        settle_lines(&mut s, &|line| {
            let path = crate::claude::session::find_transcript(&account, session.as_deref()?)?;
            crate::claude::session::took_line(&path, line)
        });
        let ties = std::mem::take(&mut s.ties);
        // The agent layer's record of whose it is (`Known::parent`) is the one.
        s.spec.parent = ties.parent.clone();
        let token = crate::agents::tie_back(&s.id, s.spec.provider, &s.spec.folder, &s.spec.account_path, ties);
        let (id, running) = (s.id.clone(), s.running);
        let mut e = taken_up(s);
        e.spec.token = token;
        lock().entries.insert(id.clone(), e);
        back.push(format!("{}{}", crate::agents::label(&id), if running { " (stopped mid-turn)" } else { "" }));
    }
    RESTORED.store(true, std::sync::atomic::Ordering::SeqCst);
    if !back.is_empty() {
        crate::log::warn("background", &format!("took up {} helper(s) the last core left: {}", back.len(), back.join(", ")));
        changed();
    }
    save();
    let _ = std::thread::Builder::new().name("luna-bg-save".into()).spawn(|| loop {
        std::thread::sleep(SAVE_EVERY);
        save();
    });
}

/// The helpers a core took up stopped mid-turn that were spawned with
/// `resumeOnRestart`: each starts its next turn by itself. Off this thread,
/// and only once the hub listens: a turn's MCP config names its port.
pub fn resume_stopped() {
    let (ids, unsure): (Vec<(String, bool)>, Vec<(String, bool)>) = lock()
        .entries
        .iter()
        .filter(|(_, e)| e.cut_short && e.run.is_none())
        .map(|(id, e)| (id.clone(), e.resumable))
        .partition(|(_, sure)| *sure);
    for (id, _) in unsure.iter().filter(|(id, _)| crate::agents::resumes_on_restart(id)) {
        crate::log::warn(
            "background",
            &format!("{}: not resumed: nothing says the last core's turn ended, and a second could overlap it", crate::agents::label(id)),
        );
    }
    let ids: Vec<String> = ids.into_iter().map(|(id, _)| id).collect();
    let ids: Vec<String> = ids.into_iter().filter(|id| crate::agents::resumes_on_restart(id)).collect();
    if ids.is_empty() {
        return;
    }
    let _ = std::thread::Builder::new().name("luna-bg-resume".into()).spawn(move || {
        for id in ids {
            resume(&id);
        }
    });
}

/// Starts the turn that carries on from one a core's going stopped. Its
/// callers are owed the end of that turn, and told it is running. One that
/// cannot start ends as `run_turn` records it and leaves the helper cut short
/// with its text kept: the caller's notice is then the one for a helper that
/// did not go on.
pub(super) fn resume(id: &str) {
    let reserved = {
        let mut r = lock();
        let Some(e) = r.entries.get_mut(id) else { return };
        // Something got there first. No conversation to go on with and no
        // words of its first turn (a file from an older core): nothing to run.
        let unnamed = e.session.is_none();
        // What may have reached it stays with its sender to decide on.
        let (doubtful, sure): (Vec<Queued>, Vec<Queued>) = std::mem::take(&mut e.undelivered).into_iter().partition(|q| q.maybe_reached);
        // Or a turn it was sent meanwhile has gone on already, and been told.
        // Or it is being opened as a chat, which takes the conversation up.
        if e.run.is_some() || e.held_back() || !e.cut_short || !e.resumable || (unnamed && sure.is_empty()) {
            e.undelivered = [sure, doubtful].concat();
            return;
        }
        claim(e);
        // This batch only: whatever is put by after this goes on being kept.
        e.in_flight = sure;
        e.undelivered = doubtful;
        stamp(&mut e.in_flight);
        // Never started, it is started now, on what it was given as it was.
        reservation_of(e, Some(unnamed))
    };
    // Its senders asked again, as for any batch (`settle_batch`).
    let Some(Batch { text, turns_ended: count, carried }) = settle_batch(id, reserved) else { return };
    save();
    crate::agents::resumed(id, count);
    match run_turn(id, &text) {
        Ok(()) => {
            if let Some(e) = lock().entries.get_mut(id) {
                note(e, "resumed by itself after a core stopped its turn");
            }
            crate::log::warn(
                "background",
                &format!(
                    "{}: resumed by itself after a core stopped its turn, {carried} undelivered message(s) with it",
                    crate::agents::label(id)
                ),
            );
        }
        Err(why) => {
            if let Some(e) = lock().entries.get_mut(id) {
                let batch = std::mem::take(&mut e.in_flight);
                let later = std::mem::take(&mut e.undelivered);
                e.undelivered = [batch, later].concat();
            }
            crate::log::warn("background", &format!("{}: could not resume by itself: {why}", crate::agents::label(id)));
        }
    }
    changed();
}

/// What a resumed helper is told: why its turn stopped, and what was queued
/// for it, word for word, as it would have come.
pub(crate) fn resume_text(queued: &[Queued]) -> String {
    let mut text = format!(
        "{} Luna's core restarted and stopped your last turn before it finished. Carry on where you left off.",
        crate::agents::NOTICE_TAG
    );
    if !queued.is_empty() {
        text.push_str("\n\nThese messages were sent to you while you worked and never reached you, oldest first:");
        for message in queued {
            text.push_str("\n\n");
            text.push_str(&message.text);
        }
    }
    text
}

/// A turn no core stopped ended only if its tie saw to it (`Entry::tied`).
fn turn_known_over(s: &Saved) -> bool {
    !s.unsure && (!s.running || s.cut_short || s.tied)
}

/// What the last core had on a Claude Code helper's stdin and heard nothing more
/// of, settled by what the conversation holds, before any of it is told of or
/// sent again: `reached` says whether a line is there, None if that cannot be
/// read. In it, the model has it, and it is off the books; not in it, with the
/// turn known to be over, it never got there; otherwise nobody can say, and its
/// sender is told that (`Queued::maybe_reached`) rather than it going out twice.
fn settle_lines(s: &mut Saved, reached: &dyn Fn(&str) -> Option<bool>) {
    if s.spec.provider != Provider::Claude {
        return;
    }
    let over = turn_known_over(s);
    settle_stores([&mut s.in_flight, &mut s.fed, &mut s.undelivered], &mut s.budgets_landed, over, reached);
}

/// `settle_lines` over the stores themselves, for a saved helper and for one of
/// this core's alike. Codex has no such lines: its stdin taking a batch is all
/// there is to go by, and what it took is off the books by then.
fn settle_stores(stores: [&mut Vec<Queued>; 3], budgets_landed: &mut Vec<String>, over: bool, reached: &dyn Fn(&str) -> Option<bool>) {
    let mut known: HashMap<String, Option<bool>> = HashMap::new();
    let mut landed: Vec<String> = Vec::new();
    for store in stores {
        store.retain_mut(|q| {
            let Some(line) = q.line.take() else { return true };
            match *known.entry(line).or_insert_with_key(|line| reached(line)) {
                Some(true) => {
                    landed.extend(q.budget.iter().map(|m| m.id.clone()));
                    false
                }
                Some(false) if over => true,
                _ => {
                    q.maybe_reached = true;
                    true
                }
            }
        });
    }
    budgets_landed.extend(landed);
}

/// What a helper stopped with no conversation yet was started on, as a message,
/// if nothing else holds it. A batch it was started on is kept already, with what
/// is known of its fate, and a second copy of it would go out as new. It goes under
/// the id it had. By its words in a file from before ids.
fn lone_opening(s: &mut Saved, stopped: bool) -> Option<Queued> {
    let text = s.opening.take().filter(|_| stopped && s.session.is_none())?;
    let id = s.opening_id.take();
    let kept = match &id {
        Some(id) => [&s.in_flight, &s.fed, &s.undelivered].iter().any(|store| store.iter().any(|q| q.id == *id)),
        None => !s.in_flight.is_empty() && joined(&s.in_flight) == text,
    };
    if kept {
        return None;
    }
    let mut q = Queued::new(None, text, Vec::new());
    if let Some(id) = id {
        q.id = id;
    }
    Some(q)
}

/// `taken_up` for a helper that stays with this core: it stopped the turn itself
/// (`stop_all`) and the process is reaped, so the turn is over and the helper may
/// resume — unless an older core left it in doubt (`unsure`), which only its next
/// turn settles. First what a Claude Code turn had on stdin is settled against
/// the conversation, as `restore` does with `settle_lines`: `reached` says of a
/// line whether the transcript holds it, read off the registry's lock
/// (`stopped_lines`), and what it holds is not sent again. One with no
/// conversation yet is owed its opening prompt first, as `lone_opening` gives it
/// to a new core, unless what was on its way holds it already: in this core every
/// opening has its id (run.rs `note_opening`), and what was on its way is in
/// `undelivered` by now (`stop_for_luna`).
pub(super) fn settle_stopped(e: &mut Entry, reached: &dyn Fn(&str) -> Option<bool>) {
    if e.run.is_some() || !e.cut_short {
        return;
    }
    e.resumable = !e.unsure;
    if e.spec.provider == Provider::Claude {
        settle_stores([&mut e.in_flight, &mut e.fed, &mut e.undelivered], &mut e.budgets_landed, !e.unsure, reached);
    }
    if e.session.is_some() {
        return;
    }
    let Some(text) = e.opening.take() else { return };
    let id = e.opening_id.take();
    let kept = match &id {
        Some(id) => e.undelivered.iter().any(|q| q.id == *id),
        None => e.undelivered.iter().any(|q| q.text == text),
    };
    if !kept {
        let mut q = Queued::new(None, text, Vec::new());
        if let Some(id) = id {
            q.id = id;
        }
        e.undelivered.insert(0, q);
    }
}

/// A saved helper as a registry entry, between turns: a turn that ran went
/// with the old core and counts as ended, and so does the text queued
/// behind it, which never reached the CLI (as `stop` counts them) and is kept
/// for its sender.
pub(super) fn taken_up(mut s: Saved) -> Entry {
    let stopped = s.running || s.cut_short;
    let resumable = turn_known_over(&s);
    let first: Vec<Queued> = lone_opening(&mut s, stopped).into_iter().collect();
    let undelivered = [first, s.undelivered, s.in_flight, s.fed, s.queue].concat();
    let cwd = s.cwd.map(|c| home_of(&s.spec, c));
    let mut e = Entry {
        spec: s.spec,
        started_ms: s.started_ms,
        session: s.session,
        cwd,
        run: None,
        turns_ended: s.turns_ended + u32::from(s.running) + u32::from(s.queued),
        queue: Vec::new(),
        in_flight: Vec::new(),
        fed: Vec::new(),
        log: VecDeque::new(),
        held: VecDeque::new(),
        error: None,
        cut_short: stopped,
        undelivered,
        budgets_landed: s.budgets_landed,
        tied: false,
        resumable,
        unsure: !resumable,
        opening: None,
        opening_id: None,
        job: None,
        left_running: false,
        left: Vec::new(),
        ending_left: false,
        promoting: s.promoting_since.is_some(),
        promoting_since: s.promoting_since.unwrap_or(0),
        deleting: false,
        observed: s.observed,
    };
    if s.running {
        note(&mut e, "Luna's core went mid-turn and stopped the turn");
        e.error = Some("Luna's core restarted mid-turn and stopped the turn".into());
    }
    if s.queued {
        note(&mut e, "what was queued behind it never reached it");
    }
    note(&mut e, "taken up by a new core");
    e
}

/// Where a helper works, from a cwd an older core saved after the CLI's shell
/// had moved it: the worktree it made and is in, else the folder it was sent
/// to if it is in that.
pub(super) fn home_of(spec: &Spec, cwd: String) -> String {
    if let Some((root, _)) = crate::worktree::root_of(&cwd).filter(|_| spec.worktree) {
        return root;
    }
    let inside = std::path::Path::new(&cwd).ancestors().any(|dir| crate::pty::same_dir(&dir.to_string_lossy(), &spec.folder));
    if inside {
        spec.folder.clone()
    } else {
        cwd
    }
}

/// Whether its last turn was stopped by the core going rather than ended.
pub fn cut_short(id: &str) -> bool {
    lock().entries.get(id).is_some_and(|e| e.cut_short)
}

/// Whether a core's going stopped it before any turn named its conversation:
/// nothing it did can be carried on from.
pub fn never_started(id: &str) -> bool {
    lock().entries.get(id).is_some_and(|e| e.cut_short && e.session.is_none())
}

/// A helper a core's going stopped before its CLI said where it works: what its next
/// turn does of the worktree that turn began, and what its parent hears of it. Claude
/// Code goes back into the one Luna named if that is still as made (`agents::
/// name_to_go_on`), and Codex makes another, naming none. A first turn no core cut off
/// is no business of this: it has not been stopped.
pub fn settle_worktree(id: &str) {
    let (provider, folder, account, name) = {
        let r = lock();
        let Some(e) = r.entries.get(id).filter(|e| e.cut_short && e.session.is_none() && e.spec.worktree) else { return };
        (e.spec.provider, e.spec.folder.clone(), e.spec.account_path.clone(), e.spec.worktree_name.clone())
    };
    match (provider, name) {
        (Provider::Codex, _) => crate::agents::report_codex_worktrees(id, &folder, &account),
        (Provider::Claude, None) => {}
        (Provider::Claude, Some(name)) => {
            let name = crate::agents::name_to_go_on(id, &name, &folder, &account);
            if let Some(e) = lock().entries.get_mut(id) {
                e.spec.worktree_name = name;
            }
            save();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A helper as an older core saved it, with the tokens and cost it counted
    /// then: it comes back all the same, with what is still kept.
    #[test]
    fn a_helper_saved_with_what_it_spent_comes_back() {
        let saved = json!([{
            "id": "c1", "spec": serde_json::to_value(&entry(Provider::Claude).spec).unwrap(), "startedMs": 5,
            "session": "sid", "turnsEnded": 3, "running": false, "queued": false, "cutShort": false,
            "ties": serde_json::to_value(crate::agents::Ties::default()).unwrap(),
            "observed": {
                "spend": {
                    "lastTurn": { "input": 2, "output": 4, "cacheRead": 30145, "cacheWrite": 479, "costUsd": 0.00039945 },
                    "session": { "input": 4, "output": 36, "cacheRead": 46707, "cacheWrite": 14062, "costUsd": 0.00329787 }
                },
                "model": "claude-haiku-5-5", "turnEndedMs": 7,
                "unreported": { "input": 0, "output": 9, "cacheRead": 0, "cacheWrite": 0 }, "answered": true
            }
        }]);
        let saved: Vec<Saved> = serde_json::from_value(saved).unwrap();
        let e = taken_up(saved.into_iter().next().unwrap());
        assert_eq!(
            (e.turns_ended, e.observed.model.as_deref(), e.observed.turn_ended_ms, e.observed.answered),
            (3, Some("claude-haiku-5-5"), 7, true)
        );
        let json = serde_json::to_value(&e.observed).unwrap();
        assert!(json.get("spend").is_none() && json.get("unreported").is_none(), "and is written without them");
    }

    #[test]
    fn a_helper_saved_mid_turn_comes_back_between_turns_with_that_turn_ended() {
        let mut spec = entry(Provider::Claude).spec;
        spec.token = Some("secret".into());
        let json = serde_json::to_value(&spec).unwrap();
        assert!(json.get("token").is_none(), "a token is never saved");
        let saved = |running, queued| Saved {
            id: "c1".into(),
            spec: serde_json::from_value(json.clone()).unwrap(),
            started_ms: 5,
            session: Some("sid".into()),
            cwd: None,
            turns_ended: 3,
            running,
            queued,
            queue: if queued { vec!["first".into(), "second".into()] } else { Vec::new() },
            in_flight: Vec::new(),
            fed: Vec::new(),
            undelivered: Vec::new(),
            budgets_landed: Vec::new(),
            tied: false,
            unsure: false,
            opening: None,
            opening_id: None,
            promoting_since: None,
            cut_short: false,
            ties: Default::default(),
            observed: Default::default(),
        };

        let e = taken_up(saved(true, true));
        assert!(e.run.is_none() && e.spec.token.is_none());
        assert_eq!(e.turns_ended, 5, "the turn stopped and the text queued behind it");
        assert!(e.cut_short && e.error.is_some());
        assert_eq!(e.session.as_deref(), Some("sid"));
        assert_eq!(e.undelivered, ["first", "second"], "kept word for word, none of it queued for a turn");
        assert!(e.queue.is_empty());

        let e = taken_up(saved(false, false));
        assert_eq!(e.turns_ended, 3);
        assert!(!e.cut_short && e.error.is_none());

        // A batch a turn was claimed for and never got comes back in its place.
        let e = taken_up(Saved { in_flight: vec!["flying".into()], ..saved(true, true) });
        assert_eq!(e.undelivered, ["flying", "first", "second"]);

        // Being opened as a chat as the core went: held as it was, no turn and no
        // resume, until the window settles the row it may have made; through the file.
        let mut opening = taken_up(Saved { cut_short: true, tied: true, ..saved(false, false) });
        opening.promoting = true;
        opening.promoting_since = 42;
        lock().entries.insert("opening-across".into(), opening);
        let on_disk = snapshot().into_iter().find(|s| s.id == "opening-across").unwrap();
        assert_eq!(on_disk.promoting_since, Some(42));
        lock().entries.insert("opening-across".into(), taken_up(on_disk));
        assert!(lock().entries["opening-across"].promoting && lock().entries["opening-across"].promoting_since == 42);
        resume("opening-across");
        assert!(lock().entries["opening-across"].run.is_none(), "not resumed while it is opened");
        lock().entries.remove("opening-across");

        // A turn no core stopped is over only if the OS saw to it.
        assert!(!taken_up(saved(true, false)).resumable, "no tie: its CLI may be running yet");
        assert!(taken_up(Saved { tied: true, ..saved(true, false) }).resumable);
        assert!(taken_up(Saved { cut_short: true, ..saved(true, false) }).resumable, "the last core stopped it itself");
        assert!(taken_up(saved(false, false)).resumable);

        // Not known to have ended stays so through the next core, which reads the
        // `cut_short` this one saved as if that core had stopped it, until its next
        // turn or a later look says.
        let first = taken_up(saved(true, false));
        assert!(first.unsure && first.cut_short && first.run.is_none());
        lock().entries.insert("unsure-across".into(), first);
        let on_disk = snapshot().into_iter().find(|s| s.id == "unsure-across").unwrap();
        lock().entries.remove("unsure-across");
        assert!(on_disk.unsure && on_disk.cut_short && !on_disk.running && !on_disk.tied);
        let second = taken_up(on_disk);
        assert!(!second.resumable && second.unsure, "as it was");
        let cleared = taken_up(Saved { unsure: false, cut_short: true, ..saved(false, false) });
        assert!(cleared.resumable && !cleared.unsure);
        assert!(
            !serde_json::to_value(saved(false, false)).unwrap().as_object().unwrap().contains_key("unsure"),
            "and nothing in the file otherwise"
        );
    }

    #[test]
    fn a_resume_nobody_can_vouch_for_is_not_tried_and_one_takes_only_its_batch() {
        take_up_for_test("unsure", "C:\\luna-no-such-folder", 3, &["queued"], None);
        lock().entries.get_mut("unsure").unwrap().resumable = false;
        resume("unsure");
        let e = lock().entries.remove("unsure").unwrap();
        assert_eq!(e.turns_ended, 5, "no turn tried");
        assert_eq!(e.undelivered, ["queued"]);

        // What is put by while a turn is being started stays, behind what failed to go.
        take_up_for_test("batch", "C:\\luna-no-such-folder", 3, &["queued"], None);
        resume("batch");
        lock().entries.get_mut("batch").unwrap().undelivered.push("later".into());
        put_back("batch", vec!["queued again".into()]);
        let e = lock().entries.remove("batch").unwrap();
        assert_eq!(e.undelivered, ["queued again", "queued", "later"]);
    }

    #[test]
    fn a_file_an_older_core_left_still_reads() {
        let spec = serde_json::to_value(&entry(Provider::Codex).spec).unwrap();
        let s: Saved = serde_json::from_value(json!({
            "id": "c1", "spec": spec, "startedMs": 5, "session": "t", "cwd": null, "turnsEnded": 3,
            "running": true, "queued": true, "cutShort": false, "ties": {
                "parent": null, "name": null, "settings": {}, "tools": false, "borrowed": false, "autoDelete": false, "owed": [], "seen": [],
            },
        }))
        .unwrap();
        let e = taken_up(s);
        assert_eq!(e.turns_ended, 5, "what it queued is counted though its words are gone");
        assert!(e.undelivered.is_empty());
    }

    #[test]
    fn what_a_stopped_turn_had_queued_is_kept_and_counted() {
        let mut e = entry(Provider::Codex);
        e.run = Some(Run::claimed(None, 0));
        e.queue = vec!["one".into(), "two".into()];
        stop_for_luna(&mut e);
        assert_eq!(e.undelivered, ["one", "two"]);
        assert!(e.queue.is_empty() && e.cut_short);
        assert_eq!(e.turns_ended, 1, "whoever queued it waits on an end that will not come");
    }

    #[test]
    fn a_kill_drops_what_was_queued_as_it_did() {
        let mut e = entry(Provider::Codex);
        e.run = Some(Run::claimed(None, 0));
        e.queue = vec!["one".into()];
        assert_eq!(stop(&mut e), None);
        assert!(e.queue.is_empty() && e.undelivered.is_empty() && !e.cut_short, "the agent's own stop, not Luna's");
    }

    #[test]
    fn a_helper_stopped_before_it_named_its_conversation_keeps_what_it_was_given() {
        // Saved as such only while no turn has named it.
        let mut e = entry(Provider::Claude);
        e.opening = Some("the prompt".into());
        lock().entries.insert("unnamed".into(), e);
        let saved = |id: &str| snapshot().into_iter().find(|s| s.id == id).unwrap();
        assert_eq!(saved("unnamed").opening.as_deref(), Some("the prompt"));
        on_claude_event(lock().entries.get_mut("unnamed").unwrap(), &json!({ "type": "system", "subtype": "init", "session_id": "s-1" }));
        assert_eq!(saved("unnamed").opening, None, "the conversation holds it now");
        lock().entries.remove("unnamed");

        take_up_for_test("never", "C:\\luna-no-such-folder", 0, &["then this"], Some("the prompt"));
        assert!(never_started("never"));
        assert_eq!(undelivered_for("never", "its parent", true), ["the prompt", "then this"], "what it was started on goes first");
        // A first turn again: no conversation to resume, its words as they were.
        let (args, _, _, _) = command_for(lock().entries.get("never").unwrap(), "never").unwrap();
        assert!(!args.contains(&"--resume".to_string()));
        resume("never");
        let e = lock().entries.remove("never").unwrap();
        assert_eq!(e.undelivered, ["the prompt", "then this"], "it could not start: kept");
        assert!(e.cut_short);

        // Stopped by nothing: a first turn that failed by itself has lost nothing to the core.
        let s = serde_json::from_value::<Saved>(json!({
            "id": "c1", "spec": serde_json::to_value(&entry(Provider::Claude).spec).unwrap(), "startedMs": 5, "session": null, "cwd": null,
            "turnsEnded": 1, "running": false, "queued": false, "opening": "the prompt", "cutShort": false, "ties": {
                "parent": null, "name": null, "settings": {}, "tools": false, "borrowed": false, "autoDelete": false, "owed": [], "seen": [],
            },
        }))
        .unwrap();
        let e = taken_up(s);
        assert!(e.undelivered.is_empty() && !e.cut_short);
    }

    #[test]
    fn a_resumed_turn_is_told_why_and_given_what_was_queued() {
        let text = resume_text(&[]);
        assert!(text.starts_with(crate::agents::NOTICE_TAG) && text.contains("Carry on where you left off"));
        assert!(!text.contains("never reached you"));
        let text = resume_text(&["fix the test\nand push".into(), "also the docs".into()]);
        let (first, second) = (text.find("fix the test\nand push").unwrap(), text.find("also the docs").unwrap());
        assert!(text.contains("never reached you") && first < second, "word for word, oldest first: {text}");
    }

    #[test]
    fn a_resume_finds_a_turn_sent_meanwhile_has_gone_on_already() {
        take_up_for_test("resume-late", "C:\\luna-no-such-folder", 3, &[], None);
        lock().entries.get_mut("resume-late").unwrap().cut_short = false;
        resume("resume-late");
        let e = lock().entries.remove("resume-late").unwrap();
        assert_eq!(e.turns_ended, 4, "no second turn tried");
    }

    #[test]
    fn a_resume_that_cannot_start_leaves_the_helper_cut_short_with_its_text() {
        take_up_for_test("resume-fail", "C:\\luna-no-such-folder", 3, &["queued"], None);
        resume("resume-fail");
        let e = lock().entries.remove("resume-fail").unwrap();
        assert!(e.run.is_none() && e.cut_short && e.error.is_some());
        assert_eq!(e.undelivered, ["queued"], "still the caller's to resend");
        assert_eq!(e.turns_ended, 6, "the stopped turn, what was queued behind it, the turn that could not start");
    }

    #[test]
    fn a_core_after_this_runs_a_helper_as_it_was_spawned() {
        // Through the file: what `save` writes and `restore` reads.
        let through_a_core = |mut e: Entry, session: Option<&str>| {
            e.session = session.map(str::to_owned);
            let spec: Spec = serde_json::from_slice(&serde_json::to_vec(&e.spec).unwrap()).unwrap();
            taken_up(Saved {
                id: "x".into(),
                spec,
                started_ms: 0,
                session: e.session.clone(),
                cwd: None,
                turns_ended: 1,
                running: true,
                queued: false,
                queue: Vec::new(),
                in_flight: Vec::new(),
                fed: Vec::new(),
                undelivered: Vec::new(),
                budgets_landed: Vec::new(),
                tied: true,
                unsure: false,
                opening: Some("the prompt".into()),
                opening_id: None,
                promoting_since: None,
                cut_short: false,
                observed: Default::default(),
                ties: Default::default(),
            })
        };
        let mut claude = entry(Provider::Claude);
        claude.spec.read_only = true;
        claude.spec.agent = Some("checker".into());
        claude.spec.parent = Some("p-1".into());
        let mut codex = entry(Provider::Codex);
        codex.spec.mode = crate::codex::launch::READ_ONLY_NET.into();
        codex.spec.parent = Some("p-1".into());
        // One that goes on and one that never got started alike.
        for session in [Some("s-1"), None] {
            let e = through_a_core(entry_like(&claude), session);
            let (args, _, envs, _) = command_for(&e, "x").unwrap();
            assert_eq!(value_of(&args, "--agent"), Some("checker"));
            assert_eq!(value_of(&args, "--disallowedTools"), Some("Edit,Write,NotebookEdit"));
            assert_eq!(value_of(&args, "--resume"), session);
            assert!(value_of(&args, "--append-system-prompt").unwrap().contains("by session p-1"));
            assert!(envs.iter().any(|(k, v)| *k == "LUNA_PARENT_ID" && v == "p-1"));

            let e = through_a_core(entry_like(&codex), session);
            let (args, _, envs, _) = command_for(&e, "x").unwrap();
            assert!(args.iter().any(|a| a.starts_with("default_permissions=\"luna-")), "{args:?}");
            assert_eq!(args.get(1).map(String::as_str) == Some("resume"), session.is_some());
            assert!(envs.iter().any(|(k, v)| *k == "LUNA_PARENT_ID" && v == "p-1"));
            // The caller gets back what it asked, the note going on as that turn starts.
            if session.is_none() {
                assert_eq!(e.undelivered, ["the prompt"]);
                assert_eq!(stdin_text(&e, &e.undelivered[0].text).matches("A note from Luna").count(), 1);
            }
        }
    }

    /// In the transcript it is the model's, whatever Luna had time to note: not
    /// given back, not sent again.
    #[test]
    fn a_core_gone_before_noting_the_clis_word_goes_by_the_transcript() {
        let mut s = with_lines(true);
        let mark = BudgetMark { id: "kid".into(), line: "over budget".into() };
        let mut flying = pending("batch", "started on");
        flying.budget = vec![mark];
        s.in_flight = vec![flying, Queued { line: Some("batch".into()), ..pending("second", "with it") }];
        s.fed = vec![pending("folded", "taken in"), pending("missed", "not taken in")];
        settle_lines(&mut s, &|line| Some(line != "missed"));
        assert!(s.in_flight.is_empty(), "a batch goes by its line");
        assert_eq!(s.fed, ["not taken in"]);
        assert!(!s.fed[0].maybe_reached, "the turn is over and it is not there: it never got there");
        assert!(s.fed[0].line.is_none());
        assert_eq!(s.budgets_landed, ["kid"]);
        let e = taken_up(s);
        assert_eq!(e.undelivered, ["not taken in"]);
    }

    #[test]
    fn what_cannot_be_established_is_said_so_and_not_sent_again() {
        // The transcript cannot be read.
        let mut s = with_lines(true);
        s.fed = vec![pending("a", "joined")];
        settle_lines(&mut s, &|_| None);
        assert!(s.fed[0].maybe_reached);
        // Or nothing says the turn is over, so the CLI may yet take it.
        let mut s = with_lines(false);
        s.in_flight = vec![pending("b", "flying")];
        settle_lines(&mut s, &|_| Some(false));
        assert!(s.in_flight[0].maybe_reached);

        let mut e = taken_up(s);
        e.resumable = true;
        e.undelivered.push(Queued::new(Some("x".into()), "plainly lost".into(), Vec::new()));
        e.session = Some("sid".into());
        e.spec.folder = "C:\\luna-no-such-folder".into();
        lock().entries.insert("doubt".into(), e);
        let told = undelivered_for("doubt", "x", false);
        assert!(told[0].contains("MAY have reached it") && told[0].ends_with("flying"), "{told:?}");
        assert_eq!(told[1], "plainly lost");
        resume("doubt");
        let e = lock().entries.remove("doubt").unwrap();
        assert_eq!(e.undelivered.iter().filter(|q| q.maybe_reached).count(), 1, "still its sender's to decide");
        assert!(e.undelivered.iter().filter(|q| q.maybe_reached).all(|q| q.text == "flying"));
    }

    /// What a stop put by, and a core's save, go the same way.
    #[test]
    fn what_a_stop_put_by_on_stdin_is_settled_too_and_what_never_went_is_not() {
        let mut s = with_lines(true);
        s.running = false;
        s.cut_short = true;
        s.undelivered = vec![pending("told", "went in"), "never on stdin".into()];
        settle_lines(&mut s, &|line| {
            assert_eq!(line, "told");
            Some(true)
        });
        assert_eq!(s.undelivered, ["never on stdin"]);
    }

    #[test]
    fn a_codex_helpers_text_is_left_as_it_was() {
        let mut s = with_lines(true);
        s.spec.provider = Provider::Codex;
        s.in_flight = vec![pending("a", "flying")];
        settle_lines(&mut s, &|_| panic!("Codex has no such lines"));
        assert!(!s.in_flight[0].maybe_reached);
    }

    /// Started on a tracked batch with no conversation named yet, and the core goes:
    /// one message, with what is known of it, and no sure copy for a resume to send.
    #[test]
    fn a_tracked_opening_is_not_rebuilt_as_a_fresh_copy() {
        let mut s = unnamed("do A", Some("a-id"));
        s.in_flight = vec![pending("a-id", "do A")];
        settle_lines(&mut s, &|_| None);
        let mut e = taken_up(s);
        assert_eq!(e.undelivered, ["do A"]);
        assert!(e.undelivered[0].maybe_reached, "the one there is says so");

        e.resumable = true;
        e.spec.folder = "C:\\luna-no-such-folder".into();
        let before = e.turns_ended;
        lock().entries.insert("one-copy".into(), e);
        resume("one-copy");
        let e = lock().entries.remove("one-copy").unwrap();
        assert!(e.run.is_none() && e.turns_ended == before, "nothing sure to go on with: a turn tried would have counted");
        assert_eq!(e.undelivered.len(), 1);

        // The same when a stop has put it by.
        let mut s = unnamed("do A", Some("a-id"));
        s.undelivered = vec![pending("a-id", "do A")];
        assert_eq!(taken_up(s).undelivered, ["do A"]);
        // And in a file from before ids, by its words.
        let mut s = unnamed("do A", None);
        s.in_flight = vec!["do A".into()];
        assert_eq!(taken_up(s).undelivered, ["do A"]);
    }

    #[test]
    fn an_opening_nothing_holds_comes_back_under_the_id_it_had() {
        let e = taken_up(unnamed("do A", Some("a-id")));
        assert_eq!(e.undelivered, ["do A"]);
        assert_eq!(e.undelivered[0].id, "a-id");
        assert!(!e.undelivered[0].maybe_reached, "as ever: it goes on by itself");

        // A batch claimed behind an opening that failed is another message.
        let mut s = unnamed("do A", Some("a-id"));
        s.in_flight = vec![pending("b-id", "then B")];
        assert_eq!(taken_up(s).undelivered, ["do A", "then B"]);
    }
}
