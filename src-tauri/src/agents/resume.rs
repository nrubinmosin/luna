//! Carrying on after a core's going: visible helpers spawned with
//! `resumeOnRestart` take up their turn again (`resume_chat`), and text that was
//! going in as the core went is found, or not, in what the session was told.

use crate::agents::ask::ask_of;
use crate::agents::delivery::{needle_of, spill, type_held, words, GoingIn, TypingInto};
use crate::agents::owed::{owe_from, resumed_in, seen};
use crate::agents::persist::{core_up_ms, SESSION_RETURN_MS};
use crate::agents::privilege::{command_prefix, command_reach_now, plain_text};
use crate::agents::read::{transcript_end, transcript_path, Message};
use crate::agents::registry::{label, reg};
use crate::agents::said::messages_at;
use crate::agents::{CALLER_RETURN_MS, NOTIFY_IDLE_LOOKS};
use crate::provider::Provider;

/// A visible helper spawned with `resumeOnRestart` that a core's going stopped
/// mid-turn, until the text that carries it on has gone in or will not.
pub(super) struct ChatResume {
    /// What it was first asked: with no conversation to carry on (the CLI came
    /// back afresh), this is what it gets.
    pub(super) opening: Option<String>,
    /// Looks in a row that found it up, drawn and between turns.
    pub(super) looks: u32,
}

/// Whether Luna means to carry this chat on by itself and has not yet: the text
/// held for it goes in with that (`resume_chat`), and what it said so far is no
/// answer while the rest is on its way.
pub(super) fn resume_pending(id: &str) -> bool {
    reg().lock().unwrap_or_else(|e| e.into_inner()).resumes.contains_key(id)
}

/// It is off: the caller hears the plain "did NOT finish" (`notice_line`), and
/// what was held for it goes in as any held text does.
pub(super) fn call_off_resume(id: &str, why: &str) {
    if reg().lock().unwrap_or_else(|e| e.into_inner()).resumes.remove(id).is_some() {
        crate::log::warn("agents", &format!("{}: not carried on by itself after a core stopped its turn: {why}", label(id)));
    }
}

/// The chats the window is to start for this (`resume_chats`): a chat's CLI
/// otherwise starts as a pane attaches, and a helper usually sits in none.
#[tauri::command]
pub fn agent_resumes() -> Vec<String> {
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    r.resumes.keys().filter(|id| r.known.get(*id).is_some_and(|k| k.waiting)).cloned().collect()
}

/// Where a chat to carry on stands: Ok(true) ready now, Ok(false) not yet, Err
/// why it is off. `ready`: its CLI stands at an empty prompt, between turns with
/// nothing up. Given as long as a chat is to come back at all, then as
/// long as a caller is: Claude Code can take minutes to draw.
fn resume_look(waiting: bool, up_ms: u64, alive: bool, typed: bool, ready: bool) -> Result<bool, &'static str> {
    if waiting {
        return if up_ms < SESSION_RETURN_MS { Ok(false) } else { Err("the window has not started it") };
    }
    if !alive {
        return Err("its CLI did not stay up");
    }
    if typed {
        return Err("a person has typed into it");
    }
    if up_ms >= CALLER_RETURN_MS {
        return Err("its CLI has not come to its prompt");
    }
    Ok(ready)
}

/// One look at every chat to carry on, typing it in where it has stood ready for
/// two looks in a row (`NOTIFY_IDLE_LOOKS`, as for a notice).
pub(super) fn resume_chats(pty: &crate::pty::PtyManager) {
    let ids: Vec<String> = reg().lock().unwrap_or_else(|e| e.into_inner()).resumes.keys().cloned().collect();
    for id in ids {
        let waiting = reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(&id).is_none_or(|k| k.waiting);
        let alive = pty.alive(&id);
        let ready = alive
            && crate::activity::turn_state(&id).is_some_and(|t| t.turn == crate::activity::Turn::Idle)
            && pty.at_prompt(&id)
            && !ask_of(&id).is_some_and(|a| a.blocking);
        match resume_look(waiting, core_up_ms(), alive, pty.typed_ago_ms(&id).is_some(), ready) {
            Err(why) => call_off_resume(&id, why),
            Ok(ready) => {
                let looks = {
                    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
                    let Some(res) = r.resumes.get_mut(&id) else { continue };
                    res.looks = if ready { res.looks + 1 } else { 0 };
                    res.looks
                };
                if looks >= NOTIFY_IDLE_LOOKS {
                    resume_chat(pty, &id);
                }
            }
        }
    }
}

/// What a chat is typed to go on: with a conversation (`resuming`), why it stopped
/// and what never reached it, as a background helper is told; with none, its
/// opening prompt and that. None with neither to go on.
fn resume_words(resuming: bool, opening: Option<String>, batch: &[crate::background::Queued]) -> Option<String> {
    if resuming {
        return Some(crate::background::resume_text(batch));
    }
    let first = crate::background::Queued::new(None, opening?, Vec::new());
    Some(crate::background::joined(&[std::slice::from_ref(&first), batch].concat()))
}

/// Whether text a core went in the middle of typing is in the conversation the CLI
/// came back on: what is there it took, and what is not it did not, the CLI that
/// was to take it having gone with the core. A new conversation has none of it.
#[derive(Debug, PartialEq)]
pub(super) enum WentIn {
    Yes,
    No,
    CannotTell,
}

fn went_in(pty: &crate::pty::PtyManager, id: &str, g: &GoingIn) -> WentIn {
    let Some(p) = pty.probe(id) else { return WentIn::CannotTell };
    if p.resume.is_none() {
        return WentIn::No;
    }
    let Some(path) = transcript_path(pty, id).filter(|path| path.is_file()) else { return WentIn::CannotTell };
    said_to(&messages_at(&path, p.provider, g.cursor), &g.needle)
}

/// Whether anyone told the session this, in what a transcript holds.
pub(super) fn said_to(messages: &[Message], needle: &str) -> WentIn {
    if messages.iter().any(|m| m.role == "user" && words(&m.text).contains(needle)) {
        WentIn::Yes
    } else {
        WentIn::No
    }
}

/// Text found on its way in (`Held::going_in`), its session back: what arrived is
/// not typed again, its sender owed the end of a turn that went with the core.
/// What did not goes in as any held text. What cannot be told is not typed again
/// either, and the sender is told: twice is what an agent cannot take back.
pub(super) fn reconcile_held(pty: &crate::pty::PtyManager, id: &str) {
    let marked: Vec<(String, String, GoingIn)> = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        r.held.iter().filter(|h| h.id == id).filter_map(|h| Some((h.caller.clone(), h.text.clone(), h.going_in.clone()?))).collect()
    };
    if marked.is_empty() {
        return;
    }
    for (caller, text, g) in marked {
        let went = went_in(pty, id, &g);
        {
            let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
            let at = r.held.iter().position(|h| h.id == id && h.caller == caller && h.text == text && h.going_in.is_some());
            if let Some(at) = at {
                if went == WentIn::No {
                    r.held[at].going_in = None;
                } else {
                    r.held.remove(at);
                }
            }
        }
        crate::log::warn("agents", &format!("text from {} to {} was going in as a core went: {went:?}", label(&caller), label(id)));
        if went != WentIn::No {
            owe_cut_short(&caller, id, g.cursor, went == WentIn::CannotTell);
        }
    }
    crate::background::save();
}

/// `caller`'s text went in and its turn went with a core: as `take_up_chats` has it
/// for a debt known as the core went, found out only now.
fn owe_cut_short(caller: &str, id: &str, cursor: u64, unsure: bool) {
    let count = crate::activity::turns_counted(id).unwrap_or(0);
    seen(caller, id, count);
    owe_from(caller, id, count, cursor);
    if let Some(o) = reg().lock().unwrap_or_else(|e| e.into_inner()).owed.get_mut(&(caller.to_string(), id.to_string())) {
        o.cut_short = true;
        o.resumed = false;
        o.late = true;
        o.unsure = unsure;
    }
    crate::activity::count_at_least(id, count.saturating_add(1));
}

/// Marks what is held for `id` as on its way in, as `typed`, on disk before it goes:
/// a core gone before it is taken off leaves the next one to tell (`reconcile_held`).
/// With `only` just that one, `Held::since` naming it.
pub(super) fn mark_going_in(id: &str, typed: &str, cursor: u64, only: Option<(&str, u64)>) {
    {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let mut one = only.is_some();
        for h in r.held.iter_mut().filter(|h| h.id == id) {
            if let Some((caller, since)) = only {
                if h.caller != caller || h.since != since || !one {
                    continue;
                }
                one = false;
            }
            h.going_in = Some(GoingIn { needle: needle_of(typed, &h.text), cursor });
        }
    }
    crate::background::save();
}

/// It did not go in after all.
pub(super) fn unmark_going_in(id: &str) {
    for h in reg().lock().unwrap_or_else(|e| e.into_inner()).held.iter_mut().filter(|h| h.id == id) {
        h.going_in = None;
    }
}

/// Types in the text that carries a chat on, with what was held for it. A person
/// or a `send` that has got there first has taken it off the list by now (as
/// `resume` finds under its lock), and any `send` after this finds the turn it
/// starts and queues behind it. Its callers are owed the end of that turn, and told
/// it is running; a text that does not go in leaves them the plain notice.
fn resume_chat(pty: &crate::pty::PtyManager, id: &str) {
    let Some(typing) = TypingInto::claim(id) else { return };
    // No key of a person's lands from here to the Enter, so what this looks at now
    // is what the text goes in on. What changed since the last look is looked at again.
    let Some(hold) = pty.hold_keys(id) else { return };
    if pty.typed_ago_ms(id).is_some() {
        return call_off_resume(id, "a person has typed into it");
    }
    let before = crate::activity::turn_state(id);
    if before.map(|t| t.turn) != Some(crate::activity::Turn::Idle) || !pty.at_prompt(id) {
        if let Some(res) = reg().lock().unwrap_or_else(|e| e.into_inner()).resumes.get_mut(id) {
            res.looks = 0;
        }
        return;
    }
    reconcile_held(pty, id);
    let resuming = pty.probe(id).is_some_and(|p| p.resume.is_some());
    let (opening, batch) = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let Some(res) = r.resumes.get(id) else { return };
        let batch: Vec<crate::background::Queued> = r
            .held
            .iter()
            .filter(|h| h.id == id)
            .map(|h| crate::background::Queued::new(Some(h.caller.clone()), h.text.clone(), Vec::new()))
            .collect();
        // As its first start would have had it: Codex has nowhere else to hear its note.
        let noted = r.known.get(id).filter(|k| k.provider == Provider::Codex).and_then(|k| k.parent.clone());
        let opening = res
            .opening
            .as_deref()
            .map(|text| noted.as_deref().map_or(text.to_string(), |parent| crate::pty::codex_helper_prompt(parent, text)));
        (opening, batch)
    };
    let Some(text) = resume_words(resuming, opening, &batch) else {
        return call_off_resume(id, "it came back with no conversation, and its prompt was not kept");
    };
    // Counted once `reconcile_held` has had its say.
    let count = crate::activity::turn_state(id).map_or(0, |t| t.turns_ended);
    let cursor = transcript_end(id);
    // Kept on disk across cores: read back, it is checked as it was when taken.
    if plain_text(&text).is_err() {
        return call_off_resume(id, "its kept text holds control characters, which would go in as keys");
    }
    // Text a CLI would take as a command goes in only if everyone whose words
    // it carries may type one there now: its parent's opening prompt, and what
    // was held for it.
    if command_prefix(&text).is_some() {
        let senders: Vec<String> = {
            let r = reg().lock().unwrap_or_else(|e| e.into_inner());
            let parent = r.known.get(id).and_then(|k| k.parent.clone());
            parent.into_iter().chain(r.held.iter().filter(|h| h.id == id).map(|h| h.caller.clone())).collect()
        };
        if senders.is_empty() || senders.iter().any(|s| command_reach_now(s, id, &text).is_err()) {
            return call_off_resume(id, "its kept text starts with a command to the CLI, which its sender may not type there");
        }
    }
    let typed = spill(&text);
    mark_going_in(id, &typed, cursor, None);
    let started = match type_held(pty, id, &typed, before.map(|t| t.turn), count, hold) {
        Ok(Some(started)) => started,
        Ok(None) => {
            unmark_going_in(id);
            return call_off_resume(id, "a person's draft is in the way");
        }
        Err(e) => {
            unmark_going_in(id);
            return call_off_resume(id, &e);
        }
    };
    // Each sender's text went in as `deliver_held` would have typed it.
    let mut senders: Vec<&str> = batch.iter().filter_map(|q| q.from.as_deref()).collect();
    senders.dedup();
    for sender in senders {
        seen(sender, id, count);
        owe_from(sender, id, count, cursor);
        if let Some(o) = reg().lock().unwrap_or_else(|e| e.into_inner()).owed.get_mut(&(sender.to_string(), id.to_string())) {
            o.late = true;
        }
    }
    {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let mut typed_in = batch.len();
        r.held.retain(|h| {
            let went = h.id == id && typed_in > 0;
            typed_in -= usize::from(went);
            !went
        });
        // In one hold with its leaving the list, or the notifier could tell the
        // plain notice in between.
        if started {
            resumed_in(&mut r, id, count);
        }
        r.resumes.remove(id);
        // Given what it was asked just now, if it had no conversation.
        if let Some(o) = r.openings.get_mut(id) {
            o.lost = false;
        }
    }
    drop(typing);
    crate::background::save();
    if started {
        crate::log::warn(
            "agents",
            &format!("{}: carried on by itself after a core stopped its turn, {} held message(s) with it", label(id), batch.len()),
        );
    } else {
        crate::log::warn("agents", &format!("{}: Luna typed in what carries it on and no turn began", label(id)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::delivery::{needle_of, GoingIn, Held, NEEDLE};
    use crate::agents::owed::{resumed, Owed};
    use crate::agents::persist::{chats_now, ties_in, SavedSend, SESSION_RETURN_MS};
    use crate::agents::read::Message;
    use crate::agents::registry::{reg, register, resumes_on_restart};
    use crate::agents::teardown::forget;
    use crate::agents::tests::{cut_short_chat, notice_for, take_up};
    use crate::agents::{notice_line, Settled, CALLER_RETURN_MS, NOTICE_TAG};

    #[test]
    fn a_visible_helper_cut_short_with_the_flag_waits_to_go_on_and_says_nothing_yet() {
        take_up(vec![cut_short_chat("vr-chat", "vr-parent", true, true, false)], Vec::new(), Vec::new());
        assert!(resume_pending("vr-chat"));
        assert!(agent_resumes().contains(&"vr-chat".to_string()), "the window is asked to start it");
        // Its turn counts as ended, but it is no answer while it may yet go on.
        let (k, o) = {
            let r = reg().lock().unwrap();
            (r.known["vr-chat"].clone(), r.owed[&("vr-parent".to_string(), "vr-chat".to_string())])
        };
        assert!(o.cut_short);
        assert_eq!(crate::activity::turns_counted("vr-chat"), Some(4));
        let pty = crate::pty::manager();
        assert!(notice_line(pty, "vr-parent", "Your helper", "vr-chat", &k, Owed { held: Some((4, 0)), ..o }).is_none());
        // Told to start, it is no longer the window's to start, and keeps its flag.
        register(
            "vr-chat",
            Provider::Claude,
            "C:\\p",
            "C:\\acc\\anthropic\\one",
            Some("vr-parent"),
            false,
            serde_json::json!({ "model": "sonnet" }),
        );
        assert!(!agent_resumes().contains(&"vr-chat".to_string()));
        assert!(resumes_on_restart("vr-chat"), "the window's settings do not carry it");
        // Typed in: both notices are owed, the first now.
        resumed("vr-chat", 4);
        reg().lock().unwrap().resumes.remove("vr-chat");
        let o = reg().lock().unwrap().owed[&("vr-parent".to_string(), "vr-chat".to_string())];
        assert!(o.resumed && !o.cut_short && o.baseline == 4 && o.cursor == 40, "all it said since it was asked, past the new turn");
        forget("vr-chat");
        assert!(!resume_pending("vr-chat"));
    }

    #[test]
    fn called_off_a_helper_gets_the_plain_notice_and_is_not_carried_on_by_the_next_core() {
        take_up(vec![cut_short_chat("co-chat", "co-parent", true, true, false)], Vec::new(), Vec::new());
        call_off_resume("co-chat", "a person has typed into it");
        assert!(!resume_pending("co-chat"));
        let o = reg().lock().unwrap().owed[&("co-parent".to_string(), "co-chat".to_string())];
        assert!(o.cut_short && !o.resumed, "the plain did NOT finish");
        // A core going before that notice: the mark comes back, no new turn with it.
        let saved = {
            let r = reg().lock().unwrap();
            ties_in(&r, "co-chat").unwrap()
        };
        assert_eq!(saved.cut_short, vec!["co-parent".to_string()]);
        forget("co-chat");
        take_up(vec![cut_short_chat("co-chat", "co-parent", true, false, true)], Vec::new(), Vec::new());
        assert!(!resume_pending("co-chat"), "called off stays off");
        forget("co-chat");
        // Without the flag, or nothing cut short, nothing to carry on.
        take_up(vec![cut_short_chat("co-plain", "co-parent", false, true, false)], Vec::new(), Vec::new());
        assert!(!resume_pending("co-plain"));
        forget("co-plain");
        let mut done = cut_short_chat("co-done", "co-parent", true, false, false);
        done.turns_ended = 4;
        take_up(vec![done], Vec::new(), Vec::new());
        assert!(!resume_pending("co-done"), "its answer was in as the core went");
        forget("co-done");
        // A turn running that nobody is owed the end of goes on all the same.
        let mut own = cut_short_chat("co-own", "co-parent", true, true, false);
        own.ties.owed.clear();
        take_up(vec![own], Vec::new(), Vec::new());
        assert!(resume_pending("co-own"));
        forget("co-own");
    }

    #[test]
    fn a_chat_goes_on_only_up_drawn_and_untouched_and_gives_up_in_time() {
        let up = 1000;
        assert_eq!(resume_look(true, up, false, false, false), Ok(false), "the window has yet to start it");
        assert!(resume_look(true, SESSION_RETURN_MS, false, false, false).is_err(), "and did not");
        assert_eq!(resume_look(false, up, true, false, false), Ok(false), "up, not drawn or not idle");
        assert_eq!(resume_look(false, up, true, false, true), Ok(true));
        assert!(resume_look(false, up, true, true, true).is_err(), "a person typed: never");
        assert!(resume_look(false, up, false, false, false).is_err(), "its CLI went");
        assert_eq!(resume_look(false, CALLER_RETURN_MS - 1, true, false, false), Ok(false), "a CLI can take minutes to draw");
        assert!(resume_look(false, CALLER_RETURN_MS, true, false, false).is_err());
    }

    #[test]
    fn a_chat_is_told_why_with_what_was_held_or_given_its_prompt_with_no_conversation() {
        let batch = [crate::background::Queued::new(Some("x".into()), "the held text".into(), Vec::new())];
        let told = resume_words(true, Some("the prompt".into()), &batch).unwrap();
        assert!(
            told.starts_with(NOTICE_TAG) && told.contains("core restarted") && told.contains("Carry on") && told.contains("the held text"),
            "{told}"
        );
        assert!(!told.contains("the prompt"), "its conversation has that");
        assert_eq!(
            resume_words(false, Some("the prompt".into()), &batch).unwrap(),
            "the prompt\n\nthe held text",
            "a first turn, on what it was given"
        );
        assert_eq!(resume_words(false, None, &batch), None, "nothing to go on from");
    }

    #[test]
    fn text_going_in_is_found_in_what_the_session_was_told() {
        let m = |role: &str, text: &str| Message { role: role.into(), text: text.into(), after_cli: false };
        let needle = needle_of("[tag] carry on\n\nThese messages:\n\nthe held   text", "the held\ntext");
        assert_eq!(needle, "the held text", "words, however they are laid out");
        let told = [m("assistant", "the held text"), m("user", "[tag] carry on\n\nthe held\n text and more")];
        assert_eq!(said_to(&told, &needle), WentIn::Yes);
        assert_eq!(said_to(&told[..1], &needle), WentIn::No, "said by the session itself is not told to it");
        assert_eq!(said_to(&[], &needle), WentIn::No);
        // A text long enough to go to a file is looked for as what was typed.
        let spilled = "This message is 9000 characters long, so Luna put it in a file: x.md.";
        assert_eq!(needle_of(spilled, &"long ".repeat(2000)), spilled);
        assert_eq!(needle_of(&"a ".repeat(500), &"a ".repeat(500)).chars().count(), NEEDLE);
    }

    #[test]
    fn a_held_text_on_its_way_in_comes_back_marked_and_only_it_is_marked() {
        let held = |caller: &str, text: &str, since| Held {
            caller: caller.into(),
            id: "gi-chat".into(),
            text: text.into(),
            since,
            told: false,
            going_in: None,
        };
        reg().lock().unwrap().held.extend([held("gi-a", "first", 1), held("gi-b", "second", 2)]);
        mark_going_in("gi-chat", "first", 77, Some(("gi-a", 1)));
        let marks = || chats_now().sends.into_iter().filter(|s| s.id == "gi-chat").map(|s| s.going_in).collect::<Vec<_>>();
        assert_eq!(marks(), [Some(GoingIn { needle: "first".into(), cursor: 77 }), None]);
        mark_going_in("gi-chat", "carry on with first and second", 80, None);
        assert!(marks().iter().all(|m| m.as_ref().is_some_and(|g| g.cursor == 80)), "a resume's batch");
        // As a file written so reads back under the next core.
        let saved: SavedSend =
            serde_json::from_str(&serde_json::to_string(&chats_now().sends.into_iter().find(|s| s.id == "gi-chat").unwrap()).unwrap())
                .unwrap();
        assert_eq!(saved.going_in.as_ref().map(|g| g.needle.as_str()), Some("first"));
        unmark_going_in("gi-chat");
        assert_eq!(marks(), [None, None]);
        let old: SavedSend = serde_json::from_str(r#"{"caller":"x","id":"y","text":"z"}"#).unwrap();
        assert_eq!(old.going_in, None, "a file an older core left");
        reg().lock().unwrap().held.retain(|h| h.id != "gi-chat");
    }

    #[test]
    fn a_text_that_went_in_as_the_core_went_owes_a_turn_cut_short() {
        register("cs-chat", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("cs-caller"), false, serde_json::json!({}));
        crate::activity::keep_count("cs-chat", 3);
        // Back under this core, with a track to count on.
        crate::activity::on_hook("cs-chat", crate::activity::HookEvent::Gone);
        owe_cut_short("cs-caller", "cs-chat", 55, true);
        let o = reg().lock().unwrap().owed[&("cs-caller".to_string(), "cs-chat".to_string())];
        assert!(o.cut_short && o.late && o.unsure && !o.resumed && o.baseline == 3 && o.cursor == 55);
        assert_eq!(crate::activity::turns_counted("cs-chat"), Some(4), "so its end reads as one");
        let (line, how) = notice_for("cs-caller", "cs-chat", 4);
        let line = line.unwrap();
        assert!(matches!(how, Settled::Answered));
        assert!(line.contains("did NOT finish") && line.contains("cannot tell whether your last message reached it"), "{line}");
        forget("cs-chat");
        crate::activity::forget("cs-chat");
    }
}
