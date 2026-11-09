//! Text held for a chat of the person's: a `luna_session_send` that found someone typing
//! there, or a draft Luna could not put aside, typed in once the way is clear
//! (`deliver_held`), and text a core went in the middle of typing, found or not
//! in what the chat was told (`reconcile_held`).

use crate::agents::ask::{ask_of, live};
use crate::agents::delivery::{put_in, spill_sent, still_starting, TypingInto, SEND_TYPING_MS};
use crate::agents::notifier::NOTIFY_EVERY;
use crate::agents::owed::{owe_from, seen, Owed};
use crate::agents::persist::{core_up_ms, returning};
use crate::agents::read::{transcript_end, transcript_path, Message};
use crate::agents::registry::{chat_name, label, reg, Registry, DEV_CALLER};
use crate::agents::said::messages_at;
use crate::throttle::now_ms;
use serde::{Deserialize, Serialize};

/// A send held this long by an unsent draft is announced on the desktop, once:
/// the draft may be forgotten, the text is not. A notice the same.
pub(super) const DRAFT_ALERT_MS: u64 = 60_000;

/// A `send` waiting for a person to send or clear what they typed.
pub(super) struct Held {
    pub(super) caller: String,
    pub(super) id: String,
    pub(super) text: String,
    pub(super) since: u64,
    /// The desktop has been told (DRAFT_ALERT_MS).
    pub(super) told: bool,
    /// On its way in: found so by the next core, a core went during that.
    pub(super) going_in: Option<GoingIn>,
}

/// How a core after this one tells whether text being typed arrived: the CLI went
/// with the core, so what is in its conversation is what it took.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GoingIn {
    /// The start of it, words only, to look for in what the session was told.
    pub(super) needle: String,
    /// Where the transcript ended before it went in.
    pub(super) cursor: u64,
}

/// How much of a text is looked for: enough to tell it, in case its tail changed.
pub(super) const NEEDLE: usize = 200;

/// Text with its white space as one space each.
pub(super) fn words(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What to look for of `text` when `typed` is what goes in: itself, or if it went
/// in with more or went to a file (`spill`), what was typed.
pub(super) fn needle_of(typed: &str, text: &str) -> String {
    let (typed, text) = (words(typed), words(text));
    (if typed.contains(&text) { text } else { typed }).chars().take(NEEDLE).collect()
}

/// Puts a caller's text by for the held sender to type in (`deliver_held`).
pub(super) fn hold(caller: &str, id: &str, text: &str) {
    crate::log::info(
        "agents",
        &format!("send from {} to {} held: the user is typing or has a draft, or other text is going in", label(caller), label(id)),
    );
    reg().lock().unwrap_or_else(|e| e.into_inner()).held.push(Held {
        caller: caller.to_string(),
        id: id.to_string(),
        text: text.to_string(),
        since: now_ms(),
        told: false,
        going_in: None,
    });
    // "Do not send it again" goes back with the result: it must hold.
    crate::background::save();
}

/// What Luna holds for a chat: sends waiting to go into it, and notices a
/// draft in it keeps back. For the chat row.
pub fn held_count(id: &str) -> usize {
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    r.held.iter().filter(|h| h.id == id).count() + r.notices_held.get(id).copied().unwrap_or(0)
}

/// Looks for held sends to type in, on a thread of its own: typing one waits
/// for its turn to start, which is not to keep the notices back.
pub(super) fn held_sender() {
    loop {
        std::thread::sleep(NOTIFY_EVERY);
        deliver_held(crate::pty::manager());
    }
}

/// Types in the sends held for people at their keys (`hold`), each once its
/// session is clear, in the order they came; tells the caller of a session
/// that went before its text could, and the desktop of text a draft has held
/// for long.
pub(super) fn deliver_held(pty: &crate::pty::PtyManager) {
    // What a core went in the middle of typing, its session back.
    let marked: std::collections::HashSet<String> =
        reg().lock().unwrap_or_else(|e| e.into_inner()).held.iter().filter(|h| h.going_in.is_some()).map(|h| h.id.clone()).collect();
    for id in marked {
        if pty.alive(&id) {
            reconcile_held(pty, &id);
        }
    }
    let held: Vec<(String, String, String, u64, bool)> = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        r.held.iter().map(|h| (h.caller.clone(), h.id.clone(), h.text.clone(), h.since, h.told)).collect()
    };
    let mut stuck: std::collections::HashSet<String> = Default::default();
    for (caller, id, text, since, told) in held {
        let drop_it = |r: &mut Registry| {
            if let Some(i) = r.held.iter().position(|h| h.caller == caller && h.id == id && h.since == since) {
                r.held.remove(i);
            }
        };
        if !live(pty, &id) {
            // Kept across a core: it goes in once the window has started it.
            if reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(&id).is_some_and(|k| returning(k, core_up_ms())) {
                continue;
            }
            drop_it(&mut reg().lock().unwrap_or_else(|e| e.into_inner()));
            if caller != DEV_CALLER {
                let ended = crate::activity::turn_state(&id).map_or(0, |t| t.turns_ended);
                let mut o = Owed::from(ended, transcript_end(&id));
                o.lost = true;
                reg().lock().unwrap_or_else(|e| e.into_inner()).owed.insert((caller.clone(), id.clone()), o);
            }
            continue;
        }
        // One at a time per session, so they go in the order they came.
        if stuck.contains(&id) {
            continue;
        }
        // A handoff under way: into the conversation it opens, once that has
        // its prompt (agents/handoff).
        let blocked = crate::agents::handoff::holds_text(&id)
            || ask_of(&id).is_some_and(|a| a.blocking)
            || still_starting(pty, &id)
            || pty.typed_ago_ms(&id).is_some_and(|ms| ms < SEND_TYPING_MS);
        let before = crate::activity::turn_state(&id);
        let baseline = before.map(|t| t.turns_ended).unwrap_or(0);
        let typing = if blocked { None } else { TypingInto::claim(&id) };
        // Under the notifier's exclusion for the session, looked at again
        // there: a handoff scheduled since the look above waits for this
        // text, or this text for the conversation it opens.
        let going = typing.as_ref().and_then(|_| crate::agents::notifier::going_into(&id));
        let typed = if going.is_some() {
            let going = spill_sent(&caller, &id, &text);
            mark_going_in(&id, &going, transcript_end(&id), Some((caller.as_str(), since)));
            put_in(&caller, &id, &going, before, baseline)
        } else {
            Ok(None)
        };
        if !matches!(typed, Ok(Some(_))) {
            unmark_going_in(&id);
        }
        drop(going);
        drop(typing);
        match typed {
            Ok(Some(sent)) => {
                let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
                drop_it(&mut r);
                if let Some(o) = r.owed.get_mut(&(caller.clone(), id.clone())) {
                    o.late = true;
                }
                drop(r);
                // Or a core killed now would type it in again.
                crate::background::save();
                crate::log::info(
                    "agents",
                    &format!("held send from {} typed into {} (started {})", label(&caller), label(&id), sent.started),
                );
            }
            Ok(None) => {
                stuck.insert(id.clone());
                if !told && pty.has_draft(&id) && now_ms().saturating_sub(since) >= DRAFT_ALERT_MS {
                    if let Some(h) =
                        reg().lock().unwrap_or_else(|e| e.into_inner()).held.iter_mut().find(|h| h.id == id && h.since == since)
                    {
                        h.told = true;
                    }
                    let chat = chat_name(&id).unwrap_or_else(|| id.clone());
                    let from = chat_name(&caller).unwrap_or_else(|| caller.clone());
                    crate::power::notify_chat(
                        &id,
                        "A message is waiting",
                        &format!(
                            "\"{chat}\" has an unsent draft, so Luna is holding a message from \"{from}\" until you send or clear it."
                        ),
                    );
                }
            }
            Err(e) => {
                stuck.insert(id.clone());
                crate::log::warn("agents", &format!("held send from {} to {} failed: {e}", label(&caller), label(&id)));
            }
        }
    }
}

/// Whether text `caller` sent to `id` is still held.
pub(super) fn is_held(caller: &str, id: &str) -> bool {
    reg().lock().unwrap_or_else(|e| e.into_inner()).held.iter().any(|h| h.caller == caller && h.id == id)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::notifier::Settled;
    use crate::agents::persist::{chats_now, SavedSend};
    use crate::agents::registry::register;
    use crate::agents::teardown::forget;
    use crate::agents::test_support::notice_for;
    use crate::provider::Provider;

    #[test]
    fn text_going_in_is_found_in_what_the_session_was_told() {
        let m = |role: &str, text: &str| Message { role: role.into(), text: text.into(), after_cli: false, final_answer: false };
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
        mark_going_in("gi-chat", "first and second", 80, None);
        assert!(marks().iter().all(|m| m.as_ref().is_some_and(|g| g.cursor == 80)), "all of them");
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
        register("cs-chat", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, false, serde_json::json!({}));
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
