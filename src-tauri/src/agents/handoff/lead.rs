//! Auto-handoff for leads: a helper spawned with Luna's tools runs a long
//! campaign, and its delegates' notices fill its context. While the setting
//! is on (`Settings::lead_handoff`) it is listed `luna_session_handoff`, and
//! at a turn's end past its threshold, at rest, Luna asks it for a handoff as
//! a turn of its own; one that writes no file gets Luna's document from its
//! records instead (document.rs). The switch is background/switch.rs's: at the
//! end of the turn that called, its next turn starts no conversation it had,
//! on Luna's opening with the copy and the facts, in the worktree it works in;
//! the conversation its CLI names then is the new one. Its id, token, role
//! layers, constraints, helpers and what they owe it stay as they are: they
//! are keyed by its id, and every turn's command rebuilds the rest
//! (background/command.rs). Its parent hears of it with its next notice.
//!
//! What is kept: the asks, what each conversation was told and the handoffs
//! through or given up in `handoffs.json` under the lead's id, as a chat's;
//! the switch itself with the helper (`helpers.json`, switch.rs `LeadSwitch`),
//! in the write that also holds its conversation and its queue.

use super::document::{Inputs, Stopped};
use super::{copies_of, drafts_of, drafts_refused, facts, file, lead_settings, served, store, watch, NOT_OFFERED};
use crate::agents::registry::{chat_name, label, reg};
use crate::background::{LeadPhase, LeadSwitch, LeadView};
use crate::provider::Provider;
use crate::throttle::now_ms;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::Path;

/// Whether `id` is a lead listed the handoff's tool: auto-handoff for leads
/// on, and a helper with Luna's tools. A helper without them never is.
pub(super) fn listed_to(id: &str) -> bool {
    lead_settings().on && crate::background::is_lead(id)
}

/// What a lead hears calling the tool unasked: nobody asked, and who will.
pub(super) const NOT_ASKED: &str = "nobody asked this session for a handoff, so luna_session_handoff does nothing now. Luna \
     asks a lead for one itself once its context passes the threshold set for leads; the tool answers then.";

/// Whether `id` is asked in the conversation it is on now: the threshold's
/// ask, bound as it opened to the conversation whose context passed it.
fn asked_now(id: &str) -> Result<(), &'static str> {
    let asked = store::records().get(id).and_then(|r| r.asked.clone());
    let now = crate::background::session_id(id);
    match asked {
        Some(a) if a.conversation.is_some() && a.conversation == now => Ok(()),
        _ => Err(NOT_ASKED),
    }
}

/// `luna_session_handoff` without `file`, from a lead: how, where, and the
/// instructions in effect, kept as what it was served.
pub(super) fn instructions(caller: &str) -> Result<String, String> {
    if !listed_to(caller) {
        return Err(NOT_OFFERED.into());
    }
    if let Some(s) = crate::background::switch_of(caller) {
        return Err(format!("handoff {} is under way already; end your turn", s.n));
    }
    asked_now(caller)?;
    let r = served::resolved_for(caller, "handoff's");
    // Its drafts folder, made private at its turn's start (`drafts_at_start`,
    // background/command.rs `handoff_dir`); none where it could not be, nor
    // to Codex, whose `exec resume` is given no folder to write in: its
    // workspace, then.
    let codex = crate::background::lead_view(caller).is_some_and(|v| v.provider == Provider::Codex);
    let suggested = match drafts_refused(caller) {
        Some(_) => None,
        None if codex => None,
        None => Some(served::new_draft(&drafts_of(caller)?)?),
    };
    let suggested = suggested.map(|p| p.to_string_lossy().into_owned());
    Ok(facts::lead_how_to(&served::source_words(&r), &r.sha256, r.body(), suggested.as_deref()))
}

/// `luna_session_handoff` with `file`, from a lead asked for it: the file
/// checked and copied as a chat's (file.rs), and the switch scheduled for the
/// end of this turn, on the conversation that called. Refused while what its
/// turns left still runs: a fresh conversation would know nothing of it.
pub(super) fn handoff(caller: &str, file: &str) -> Result<Value, String> {
    asked_now(caller)?;
    let v = crate::background::lead_view(caller).ok_or(NOT_OFFERED)?;
    if let Some(s) = &v.switch {
        return Err(format!("handoff {} is under way already; end your turn", s.n));
    }
    let left = crate::background::left_now(caller);
    if !left.is_empty() {
        return Err(format!(
            "not now: what your turns left is still running ({}), and it cannot cross to a new conversation. Let it finish or \
             stop it, then call luna_session_handoff again",
            left.join(", ")
        ));
    }
    let before = v.session.clone();
    let dir = copies_of(caller)?;
    let free = file::next_free(&dir);
    let (n, served) = reserve(caller, free)?;
    let unreserve = || {
        if let Some(rec) = store::records().get_mut(caller).filter(|r| r.reserved == Some(n)) {
            rec.reserved = None;
        }
    };
    let snap = match file::snapshot(caller, file, &crate::paths::data_dir(), &dir, n) {
        Ok(snap) => snap,
        Err(e) => {
            unreserve();
            return Err(e);
        }
    };
    let give_up = |e: String| {
        unreserve();
        let _ = std::fs::remove_file(&snap.path);
        Err(e)
    };
    let missing = if served::checks_sections(served.as_ref()) { facts::missing_sections(&snap.text) } else { Vec::new() };
    // The ask still open, of this very conversation, and the number still the
    // call's: nothing changed while the file was copied.
    let held = store::change(caller, |rec| {
        if rec.reserved != Some(n) {
            return false;
        }
        rec.reserved = None;
        rec.asked.as_ref().is_some_and(|a| a.conversation.is_some() && a.conversation == before)
    });
    match held {
        Ok(true) => {}
        Ok(false) => return give_up("the ask for this handoff closed while the file was copied; nothing will switch".into()),
        Err(e) => return give_up(format!("the handoff could not be saved, so nothing will switch: {e}")),
    }
    // Owed answers read on from where they are: once the fresh conversation is
    // named, read.rs `moved_to` takes them to its start, as for a chat's.
    let previous = crate::agents::read::transcript_path(crate::pty::manager(), caller).map(|p| p.to_string_lossy().into_owned());
    let snapshot = snap.path.to_string_lossy().into_owned();
    let switch = LeadSwitch {
        n,
        copy: snapshot.clone(),
        sha256: snap.sha256.clone(),
        source: snap.source.clone(),
        phase: LeadPhase::Scheduled,
        before,
        previous,
        opening_id: None,
        opened: None,
        from_records: false,
    };
    if let Err(e) = crate::background::schedule_switch(caller, switch) {
        return give_up(e);
    }
    let thin = if missing.is_empty() { String::new() } else { format!("; no heading for {}", missing.join(", ")) };
    crate::log::info(
        "handoff",
        &format!("{} scheduled handoff {n} from {} ({} bytes copied, sha256 {}){thin}", label(caller), snap.source, snap.size, snap.sha256),
    );
    let mut answer = json!({
        "scheduled": true,
        "n": n,
        "snapshot": snapshot,
        "sha256": snap.sha256,
        "next": "End your turn now. As it ends Luna starts this session's next conversation with your handoff and the facts \
                 it knows; what is sent to you meanwhile waits for it. If it cannot, a notice says why.",
    });
    if !missing.is_empty() {
        answer["missing"] = json!(missing);
        answer["note"] = json!(
            "Your file has no heading for these sections of Luna's checklist. It goes as it is; if one of them matters to the \
             next conversation, say it in your last words before you end your turn: the conversation before stays readable."
        );
    }
    Ok(answer)
}

/// The next handoff number of `lead`'s, past any copy on disk (`free`),
/// reserved for one call at a time; with what it was served to write it by.
fn reserve(lead: &str, free: u32) -> Result<(u32, Option<served::Served>), String> {
    let mut r = store::records();
    let rec = r.entry(lead.to_string()).or_default();
    if rec.reserved.is_some() {
        return Err("a handoff of this session is being scheduled already; end your turn".into());
    }
    let n = (rec.last + 1).max(free);
    rec.last = n;
    rec.reserved = Some(n);
    Ok((n, rec.served.clone()))
}

/// At the end of each turn of a lead's (background/events.rs `turn_over`):
/// the switch that turn scheduled started, or given up; one whose fresh start
/// named no conversation given up; one through recorded; then, at rest, its
/// context held to the threshold.
pub fn lead_turn_ended(id: &str) {
    let Some(v) = crate::background::lead_view(id) else { return };
    match v.switch.clone() {
        Some(s) if s.phase == LeadPhase::Done => through(id, &s, v.provider),
        // Its fresh conversation's first turn, begun at once on what waited.
        Some(_) if !v.idle => return,
        Some(s) if s.phase == LeadPhase::Fresh => {
            if v.session.is_none() {
                let why = v.error.clone().unwrap_or_else(|| "its CLI ended before it named a conversation".into());
                gave_up(id, &s, &why, true);
            }
            return;
        }
        Some(s) => {
            switch_now(id, &s, &v);
            return;
        }
        None => {}
    }
    measure(id);
}

/// A lead a core took up with its switch on record (background/persist.rs
/// `resume_stopped`): through, recorded; scheduled or started with no
/// conversation named, started on a fresh opening, the facts as of now,
/// whatever `resumeOnRestart` says. Where nothing says the last core's turn
/// ended, a second turn could run beside it: given up, back on the
/// conversation it left.
pub fn lead_taken_up(id: &str) {
    let Some(v) = crate::background::lead_view(id) else { return };
    let Some(s) = v.switch.clone() else { return };
    match s.phase {
        LeadPhase::Done => through(id, &s, v.provider),
        _ if !v.idle => {}
        _ if v.unsure => {
            let why = "Luna's core restarted, and nothing says the turn before it had ended";
            gave_up(id, &s, why, s.phase == LeadPhase::Fresh);
        }
        _ => start(id, &s, &v),
    }
}

/// `s`, scheduled in the turn that just ended, started: only where the turn
/// was not stopped and nothing of its CLI's own is left at work, which would
/// run on with no conversation to know of it.
fn switch_now(id: &str, s: &LeadSwitch, v: &LeadView) {
    let why = if v.stopped {
        Some("its turn was stopped (luna_session_stop or the window)".to_string())
    } else if !v.left.is_empty() {
        Some(format!("what its turns left is still running ({})", v.left.join(", ")))
    } else if v.cut_off {
        Some("codex exec interrupted the subagents of its turn as it ended, their work maybe unfinished".to_string())
    } else {
        None
    };
    match why {
        Some(why) => gave_up(id, s, &why, false),
        None => start(id, s, v),
    }
}

/// The fresh conversation started on Luna's opening: the copy checked first
/// (`copy_holds`), the facts as of now.
fn start(id: &str, s: &LeadSwitch, v: &LeadView) {
    if let Err(why) = copy_holds(id, s) {
        return gave_up(id, s, &format!("its copy cannot be trusted: {why}"), s.phase == LeadPhase::Fresh);
    }
    // The transcript it leaves, for the opening and for where owed answers
    // read from until the new one is named (read.rs `moved_to`).
    let previous = match s.phase {
        LeadPhase::Scheduled => crate::agents::read::transcript_path(crate::pty::manager(), id).map(|p| p.to_string_lossy().into_owned()),
        _ => s.previous.clone(),
    };
    let text = opening(id, s, previous.as_deref(), v.provider);
    match crate::background::begin_fresh(id, s.n, text, previous) {
        Ok(()) => {
            // A wake for the conversation it left, and where that one's context
            // stood, are behind it (notifier lines.rs).
            crate::agents::notifier::lines::lead_goes_fresh(id);
            crate::log::info("handoff", &format!("{} handoff {}: a fresh conversation started from {}", label(id), s.n, s.copy))
        }
        Err(why) => {
            // Never started, or started on no conversation: either way none
            // was named, and it goes back to the one it left.
            let s = crate::background::switch_of(id).unwrap_or_else(|| s.clone());
            gave_up(id, &s, &format!("its fresh conversation could not start: {why}"), s.phase == LeadPhase::Fresh);
        }
    }
}

/// Whether `s`'s copy is still the one Luna made: right in the lead's folder
/// of copies, a private file of this user's opened as itself, its SHA-256 the
/// one on record (as mod.rs `copy_holds` for a chat's).
fn copy_holds(id: &str, s: &LeadSwitch) -> Result<(), String> {
    let dir = copies_of(id)?;
    let path = Path::new(&s.copy);
    if path.parent() != Some(dir.as_path()) {
        return Err(format!("{} is not in this session's folder of copies", path.display()));
    }
    let bytes = crate::secret::read_private(path, file::MAX_BYTES)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("{} is gone", path.display()))?;
    let sha256: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    if sha256 != s.sha256 {
        return Err(format!("{} is not the file Luna copied (its SHA-256 differs)", path.display()));
    }
    Ok(())
}

/// The opening of `s`'s conversation: the copy, the facts as of now, and the
/// conversation before. Past what one prompt holds, the facts go in a file
/// beside the copy (mod.rs `continuation`, for a chat).
fn opening(id: &str, s: &LeadSwitch, previous: Option<&str>, provider: Provider) -> String {
    let facts = facts::fact_block(&super::gather(id, previous, provider));
    let name = chat_name(id);
    let prompt = |facts: &str| facts::lead_prompt(s.n, id, name.as_deref(), &s.copy, &s.sha256, &s.source, previous, facts, s.from_records);
    let whole = prompt(&facts);
    if whole.chars().count() <= 6000 {
        return whole;
    }
    let written = copies_of(id).and_then(|dir| {
        let path = dir.join(format!("{}-facts.md", s.n));
        crate::secret::write_private(&path, facts.as_bytes()).map(|()| path).map_err(|e| e.to_string())
    });
    let pointer = match written {
        Ok(path) => format!("What Luna knows now is in {}: read it after the handoff.", path.display()),
        Err(_) => facts.chars().take(4000).collect(),
    };
    prompt(&pointer)
}

/// `s` given up, `started` after a fresh start that named no conversation:
/// back where it was (switch.rs), kept on record, and the lead told. Before
/// its start the ask stays open, so it may call again once what stood in the
/// way is over; after, the ask closes, so a start that keeps failing is not
/// asked over and over.
fn gave_up(id: &str, s: &LeadSwitch, why: &str, started: bool) {
    let back = match started {
        true => crate::background::revert_switch(id, s.n),
        false => crate::background::drop_switch(id, s.n),
    };
    if back.is_none() {
        return;
    }
    let provider = crate::background::lead_view(id).map_or(Provider::Claude, |v| v.provider);
    let recorded = store::change(id, |r| {
        record_past(r, s, provider, Some(why.to_string()));
        if started || s.from_records {
            r.asked = None;
        }
        r.asked.is_some()
    });
    let asked = match recorded {
        Ok(asked) => asked,
        Err(e) => {
            crate::log::warn("handoff", &format!("{}: handoff {} could not be saved as given up: {e}", label(id), s.n));
            false
        }
    };
    crate::log::warn("handoff", &format!("{} gave up handoff {}: {why}", label(id), s.n));
    let line = format!("{} {}", crate::agents::NOTICE_TAG, facts::lead_failed_line(s.n, why, &s.copy, started, asked));
    if let Err(e) = crate::background::send_luna(id, &line, false) {
        crate::log::warn("handoff", &format!("{}: could not tell it its handoff was given up: {e}", label(id)));
    }
}

/// `s` through: the fresh conversation named. Kept on record as a chat's
/// handoff is (mod.rs `close`), the ask it answered closed, the conversation
/// it began given its floor at its first look (`Record::fresh`), and its
/// parent told with its next notice.
fn through(id: &str, s: &LeadSwitch, provider: Provider) {
    if crate::background::clear_switch(id, s.n).is_none() {
        return;
    }
    let gone = store::change(id, |r| {
        let gone = record_past(r, s, provider, None);
        r.fresh = true;
        r.served = None;
        r.asked = None;
        if let Some(t) = r.told.as_mut() {
            t.shares.clear();
            t.compacted = 0;
        }
        gone
    });
    match gone {
        Ok(gone) => {
            if let Ok(dir) = copies_of(id) {
                for p in gone.iter().filter(|p| Path::new(&p.snapshot).parent() == Some(dir.as_path())) {
                    let _ = std::fs::remove_file(&p.snapshot);
                    let _ = std::fs::remove_file(dir.join(format!("{}-facts.md", p.n)));
                }
            }
        }
        Err(e) => crate::log::warn("handoff", &format!("{}: handoff {} could not be saved as through: {e}", label(id), s.n)),
    }
    crate::log::info(
        "handoff",
        &format!("{} went through handoff {}: conversation {}", label(id), s.n, s.opened.as_deref().unwrap_or("?")),
    );
    handed_off(id, s.n);
}

/// `s` as a past handoff of record `r`, the last KEPT kept; those dropped
/// handed back for their copies to go.
fn record_past(r: &mut store::Record, s: &LeadSwitch, provider: Provider, why: Option<String>) -> Vec<store::Past> {
    r.past.push(store::Past {
        n: s.n,
        at: now_ms(),
        provider,
        snapshot: s.copy.clone(),
        sha256: s.sha256.clone(),
        previous: s.previous.clone(),
        done: why.is_none(),
        why,
        instructions: r.served.clone(),
        moved: false,
    });
    let over = r.past.len().saturating_sub(store::KEPT);
    r.past.drain(..over).collect()
}

/// The leads under a session that went on in a fresh conversation since it
/// was last told, and since when: a line riding its next notice (notifier
/// `riders_go`), as a refused place does (spawn/ `PlacesRefused`).
#[derive(Clone, Debug)]
pub(in crate::agents) struct HandedOff {
    pub(in crate::agents) since: u64,
    pub(in crate::agents) who: Vec<(String, u32)>,
}

impl HandedOff {
    pub(in crate::agents) fn line(&self) -> String {
        facts::handed_off_line(&self.who)
    }
}

/// `id`'s handoff `n` is through: its parent, if it has one, hears of it with
/// its next notice.
fn handed_off(id: &str, n: u32) {
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let Some(parent) = r.known.get(id).and_then(|k| k.parent.clone()) else { return };
    let h = r.handed_off.entry(parent).or_insert_with(|| HandedOff { since: now_ms(), who: Vec::new() });
    h.who.retain(|(lead, _)| lead != id);
    h.who.push((id.to_string(), n));
}

/// `id`'s context held to the threshold for leads at a turn's end, at rest:
/// past it, the ask opened, bound to the conversation it is on, and queued as
/// a turn of Luna's own, past the firm mark ahead of anything queued; for a
/// lead that writes no file, Luna's own document instead. A conversation a
/// handoff began is asked only past its floor (`watch::level`). Off, the
/// threshold's ask still open closes.
fn measure(id: &str) {
    let l = lead_settings();
    if !l.on {
        let _ = store::change(id, |r| {
            if r.asked.as_ref().is_some_and(|a| a.kind == store::AskKind::Auto) {
                r.asked = None;
            }
        });
        return;
    }
    let Some(v) = crate::background::lead_view(id).filter(|v| v.at_rest && v.switch.is_none()) else { return };
    let Some(session) = v.session.clone() else { return };
    let Some(path) = crate::background::transcript(id) else { return };
    let Some(ctx) = crate::background::Context::of(&path, v.provider) else { return };
    let (told, fresh) = store::records().get(id).map(|r| (r.told.clone(), r.fresh)).unwrap_or_default();
    // A conversation a handoff began has its floor where its first turn
    // ended: this look, the first at its end.
    let floor_at = fresh.then_some(ctx.pct);
    let now = watch::told_now(told.as_ref(), &session, ctx.pct, ctx.tokens, floor_at, l.threshold);
    let level = watch::level(ctx.pct, l.threshold, now.floor);
    if !watch::due(&now, level) && now.level == 2 && unanswered(id, &session, v.turns_ended) {
        crate::log::info("handoff", &format!("{} left its firm ask unanswered: handoff from Luna's records", label(id)));
        return from_records(id, &v, now, &session);
    }
    if !watch::due(&now, level) {
        if told.as_ref() != Some(&now) {
            if let Err(e) = store::change(id, |r| super::kept_told(r, now)) {
                crate::log::warn("handoff", &format!("{}: where its context stands could not be saved: {e}", label(id)));
            }
        }
        return;
    }
    let firm = level == 2;
    let inline = firm && crate::library::handoff_instructions().source == crate::library::Source::Luna;
    let mut shares = now.shares.clone();
    shares.push(ctx.pct);
    let told = store::Told { level, shares, tokens: ctx.tokens, ..now };
    if v.unwritable {
        return from_records(id, &v, told, &session);
    }
    let line = facts::notice(ctx.pct, l.threshold, firm, inline);
    let asked = store::Asked {
        conversation: Some(session),
        notice: Some(super::sha256_of(&line)),
        ..store::Asked::new(store::AskKind::Auto, now_ms(), Some(v.turns_ended))
    };
    if let Err(e) = store::change(id, |r| {
        super::told_of(r, told, inline);
        r.asked = Some(asked);
    }) {
        crate::log::warn("handoff", &format!("{}: the ask could not be saved: {e}", label(id)));
        return;
    }
    let text = format!("{} {line}", crate::agents::NOTICE_TAG);
    match crate::background::send_luna(id, &text, firm) {
        Ok(_) => {
            crate::log::info("handoff", &format!("{} is at {}% of its context: asked for a handoff (level {level})", label(id), ctx.pct))
        }
        Err(e) => crate::log::warn("handoff", &format!("{}: its ask for a handoff did not go in: {e}", label(id))),
    }
}

/// Whether `id`'s ask, Luna's own on `session`, went unanswered: a turn of its
/// has ended since it was opened (the ask's own, started at once at rest),
/// with no switch scheduled and none given up since. A lead whose write is
/// refused unasked (a mode or reviewer that lets it through nowhere) would
/// otherwise run on to its CLI's own compaction.
fn unanswered(id: &str, session: &str, turns_ended: u32) -> bool {
    let r = store::records();
    let Some(rec) = r.get(id) else { return false };
    rec.asked.as_ref().is_some_and(|a| {
        a.kind == store::AskKind::Auto
            && a.conversation.as_deref() == Some(session)
            && a.turns.is_some_and(|t| turns_ended > t)
            && rec.past.last().is_none_or(|p| p.at < a.at)
    })
}

/// A lead that writes no file past its threshold, or that left the firm ask
/// unanswered: Luna writes its handoff from its records (document.rs, as for
/// a move from records), copies it as the next number, and starts its fresh
/// conversation on it at once, the lead being at rest. Its conversation's
/// told level is kept first and the ask closed, so one whose document will
/// not build is not built again at every turn's end.
fn from_records(id: &str, v: &LeadView, told: store::Told, session: &str) {
    if let Err(e) = store::change(id, |r| {
        super::told_of(r, told, false);
        if r.asked.as_ref().is_some_and(|a| a.kind == store::AskKind::Auto) {
            r.asked = None;
        }
    }) {
        crate::log::warn("handoff", &format!("{}: where its context stands could not be saved: {e}", label(id)));
        return;
    }
    let made = written_from_records(id, v);
    let (n, path, sha256) = match made {
        Ok(made) => made,
        Err(why) => {
            crate::log::warn("handoff", &format!("{}: Luna's handoff document from its records was not made: {why}", label(id)));
            return;
        }
    };
    let previous = crate::agents::read::transcript_path(crate::pty::manager(), id).map(|p| p.to_string_lossy().into_owned());
    let s = LeadSwitch {
        n,
        copy: path,
        sha256,
        source: "Luna's document from its records".into(),
        phase: LeadPhase::Scheduled,
        before: Some(session.to_string()),
        previous,
        opening_id: None,
        opened: None,
        from_records: true,
    };
    if let Err(e) = crate::background::schedule_switch(id, s.clone()) {
        crate::log::warn("handoff", &format!("{}: handoff {n} from records not scheduled: {e}", label(id)));
        return;
    }
    crate::log::info("handoff", &format!("{} writes no file: handoff {n} from Luna's records", label(id)));
    if let Some(v) = crate::background::lead_view(id) {
        start(id, &s, &v);
    }
}

/// Luna's document for `id` written and copied as its next handoff, private
/// in its folder of copies: its number, path and SHA-256.
fn written_from_records(id: &str, v: &LeadView) -> Result<(u32, String, String), String> {
    let dir = copies_of(id)?;
    let free = file::next_free(&dir);
    let (n, _) = reserve(id, free)?;
    let unreserve = || {
        if let Some(rec) = store::records().get_mut(id).filter(|r| r.reserved == Some(n)) {
            rec.reserved = None;
        }
    };
    let (known_account, folder, settings) = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        r.known.get(id).map(|k| (k.account.clone(), k.folder.clone(), k.settings.clone())).unwrap_or_default()
    };
    let folder = crate::background::working_dir(id).unwrap_or(folder);
    let (helpers, _) = super::moving::helpers_of(id);
    let inputs = Inputs {
        provider: v.provider,
        chat: id.to_string(),
        name: chat_name(id),
        from_account: known_account.clone(),
        to_account: known_account,
        folder,
        settings,
        note: String::new(),
        stopped: Stopped::Finished,
        transcript: Err(String::new()),
        last_handoff: super::moving::last_handoff(id),
        git: Default::default(),
        helpers,
        at: crate::background::iso(now_ms()).unwrap_or_default(),
        boundary: crate::secret::random_hex(6),
        chat_text: None,
        lead: true,
    };
    let transcript = crate::background::transcript(id);
    let built = match super::moving::build_within(inputs, transcript, None, super::moving::BUILD_WAIT) {
        Ok(built) => built,
        Err(e) => {
            unreserve();
            return Err(e);
        }
    };
    let path = dir.join(format!("{n}.md"));
    let written = crate::secret::write_private_synced(&path, built.text.as_bytes()).map_err(|e| format!("{}: {e}", path.display()));
    unreserve();
    written?;
    Ok((n, path.to_string_lossy().into_owned(), super::sha256_of(&built.text)))
}

#[cfg(test)]
#[path = "lead_tests.rs"]
mod tests;
