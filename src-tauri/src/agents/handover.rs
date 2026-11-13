//! A helper the person opened as a chat (background/rows.rs `background_promoted`)
//! is handed over to them for good: it becomes their own chat, nobody's helper.
//! Its parent loses every claim on it — no stopping, deleting, sending, waiting or
//! reading it as its own — and is owed nothing more by it. Text still queued for
//! it never goes in: the chat is the person's now, and their next prompt is
//! theirs. Whoever is affected hears so once, in a line that outlives a core.
//!
//! The order is what keeps it whole: its ownership changes in one hold of the
//! registry while the helper's record still stands and starts no turn
//! (`promoting`), so no call of its parent's lands in between; only then is the
//! record taken, with whatever was queued for it up to that moment, and the
//! lines put together from that.

use crate::agents::lost::undelivered_text;
use crate::agents::notifier::hand_off;
use crate::agents::persist::HELPER_SETTINGS;
use crate::agents::registry::{chat_name, label, reg};
use crate::agents::said::note_file;
use std::collections::BTreeMap;

/// What the hold that hands it over found, for the lines told after it.
struct TakenOver {
    parent: Option<String>,
    /// Every session that was owed an answer by it, its parent among them.
    owed: Vec<String>,
}

/// Hands helper `id` over to the person. `take_record` takes the helper's record
/// once its ownership has changed, and gives back what was queued for it and never
/// reached it (None: it is gone already, deleted meanwhile, and nothing is left to
/// tell). Its parent, and every other session that wrote to it, hears once.
///
/// All of it under the notifier's hold on `id` (`hand_off`): a notice about it on
/// its way into a caller is in before anything changes, and none starts after.
/// Err, with nothing changed and `take_record` not called, if such a notice will
/// not finish going in.
pub fn take_over(id: &str, take_record: impl FnOnce() -> Option<Vec<crate::background::Queued>>) -> Result<(), String> {
    let _held = hand_off(id)?;
    let Some(taken) = change_hands(id) else {
        take_record();
        return Ok(());
    };
    let unsent = take_record().unwrap_or_default();
    drained(id);
    tell(id, &taken, unsent);
    crate::agentlog::ended(id, crate::agentlog::TAKEN_OVER);
    crate::log::warn(
        "agents",
        &format!(
            "{} taken over by the person as their own chat; {} no longer has it",
            label(id),
            taken.parent.as_deref().unwrap_or("nobody")
        ),
    );
    Ok(())
}

/// The one hold in which `id` stops being anyone's helper. None if it is not
/// known (deleted meanwhile).
fn change_hands(id: &str) -> Option<TakenOver> {
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let k = r.known.get_mut(id)?;
    let parent = k.parent.take();
    k.orphan = false;
    if let Some(o) = k.settings.as_object_mut() {
        for key in HELPER_SETTINGS {
            o.remove(key);
        }
    }
    // Its answers owed: nobody's to wait for any more, whoever asked.
    let mut owed: Vec<String> = r.owed.keys().filter(|(_, s)| s == id).map(|(c, _)| c.clone()).collect();
    owed.sort();
    r.owed.retain(|(_, s), _| s != id);
    r.seen.retain(|(_, s), _| s != id);
    r.receipts.retain(|(_, s), _| s != id);
    r.delivery_floors.retain(|(_, s), _| s != id);
    r.answered.retain(|(_, s), _| s != id);
    r.notice_times.retain(|(_, s), _| s != id);
    r.stopped.retain(|(_, s), _| s != id);
    // What only a helper has: nothing deletes it now but the person.
    r.auto.remove(id);
    r.borrowed.remove(id);
    r.worktrees_before.remove(id);
    r.worktrees_left.remove(id);
    if let Some(p) = &parent {
        r.taken_over.insert(id.to_string(), p.clone());
    }
    Some(TakenOver { parent, owed })
}

/// Its queue taken back out: no text can go in for it as a helper any more. A send
/// that put text in before this registers its debt after it, or has since the
/// hold above: the count moves on (`handovers`), which such a send checks its debt
/// against (owed.rs `owe_from_at`), and what is owed by it goes again, once more.
/// A notice on its way about it goes nowhere: it is checked against its debts as
/// it goes in (notifier `deliver_notice`), and they are gone.
fn drained(id: &str) {
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    *r.handovers.entry(id.to_string()).or_default() += 1;
    r.owed.retain(|(_, s), _| s != id);
    r.seen.retain(|(_, s), _| s != id);
    r.delivery_floors.retain(|(_, s), _| s != id);
}

/// The count `drained` keeps for `id`, as a send that puts text in for it takes
/// it before it does.
pub(super) fn handovers_of(id: &str) -> u64 {
    reg().lock().unwrap_or_else(|e| e.into_inner()).handovers.get(id).copied().unwrap_or(0)
}

/// The line each one hears: its parent that the helper is the person's now, the
/// others that wrote to it that nothing more is owed by it; and each, of the text
/// it had queued that never reached it, where that text is. Text of a session no
/// longer there goes to a file the log names, as any that never reached a helper.
fn tell(id: &str, taken: &TakenOver, unsent: Vec<crate::background::Queued>) {
    let name = chat_name(id).map_or(String::new(), |n| format!(" \"{n}\""));
    let mut by_sender: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut nobodys = Vec::new();
    for q in unsent {
        // Luna's own notice to it, or its opening prompt: its parent's to hear
        // of, as what never reached it from nobody in particular; a notice no one.
        let from = q.from.clone().or_else(|| taken.parent.clone().filter(|_| !q.text.starts_with(crate::agents::NOTICE_TAG)));
        match from {
            Some(from) => by_sender.entry(from).or_default().push(q.text),
            None => nobodys.push(q),
        }
    }
    crate::agents::undelivered_left(id, &nobodys);
    let mut told: Vec<String> = taken.parent.iter().cloned().collect();
    told.extend(taken.owed.iter().filter(|c| !told.contains(c)).cloned().collect::<Vec<_>>());
    told.extend(by_sender.keys().filter(|c| !told.contains(c)).cloned().collect::<Vec<_>>());
    let mut lines = Vec::new();
    {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        for caller in told {
            let dropped = by_sender.remove(&caller).unwrap_or_default();
            if !r.known.contains_key(&caller) {
                if !dropped.is_empty() {
                    let lost: Vec<crate::background::Queued> =
                        dropped.into_iter().map(|t| crate::background::Queued::new(Some(caller.clone()), t, Vec::new())).collect();
                    lines.push((None, lost));
                }
                continue;
            }
            let opening = if taken.parent.as_deref() == Some(caller.as_str()) {
                format!(
                    "The person took over your helper {id}{name} as their own chat: it is theirs now, no longer yours to send to, \
                     wait on, read, stop or delete, and nothing more is owed by it."
                )
            } else {
                format!(
                    "Session {id}{name}, which you wrote to, was taken over by the person as their own chat: nothing more is owed by it."
                )
            };
            lines.push((
                Some((caller, opening)),
                dropped.into_iter().map(|t| crate::background::Queued::new(None, t, Vec::new())).collect(),
            ));
        }
    }
    let mut news = Vec::new();
    for (to, dropped) in lines {
        let Some((caller, opening)) = to else {
            crate::agents::undelivered_left(id, &dropped);
            continue;
        };
        news.push((caller, format!("{opening}{}", dropped_line(id, &dropped))));
    }
    reg().lock().unwrap_or_else(|e| e.into_inner()).news.extend(news);
    crate::background::save();
}

/// What a line says of a caller's text that never reached the helper: the file it
/// is in, read with the line.
fn dropped_line(id: &str, dropped: &[crate::background::Queued]) -> String {
    if dropped.is_empty() {
        return String::new();
    }
    let texts: Vec<String> = dropped.iter().map(|q| q.text.clone()).collect();
    let n = if texts.len() == 1 { "1 message".to_string() } else { format!("{} messages", texts.len()) };
    match note_file(id, &undelivered_text(&texts)) {
        Some(path) => format!(" Never delivered, and it will not be: {n} you had queued for it, word for word in {}", path.display()),
        None => format!(" Never delivered, and it will not be: {n} you had queued for it."),
    }
}

/// An opening of helper `id` as a chat that the window never settled has gone back
/// to its parent (background/rows.rs `lapse_openings`), which could neither stop nor
/// delete it meanwhile: the parent hears that it is its helper again, as it was.
pub fn opening_lapsed(id: &str) {
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let Some(parent) = r.known.get(id).and_then(|k| k.parent.clone()) else { return };
    let name = r.names.get(id).map_or(String::new(), |n| format!(" \"{n}\""));
    r.news.push((
        parent,
        format!(
            "The person began opening your helper {id}{name} as a chat of their own, and it did not open: it is yours as \
             before, to send to, wait on, stop or delete."
        ),
    ));
    drop(r);
    crate::background::save();
}

/// The refusal a former parent gets.
pub(super) fn taken_over_error(id: &str) -> String {
    format!("the person took {id} over as their own chat: it is no longer yours, and nothing more is owed by it")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::owed::owe_from;
    use crate::agents::registry::{authorize, in_scope, parent_of, register};
    use crate::agents::teardown::{forget, news_for, Auto};
    use crate::provider::Provider;

    fn helper(id: &str, parent: &str, provider: Provider) {
        let account = if provider == Provider::Codex { "C:\\acc\\openai\\two" } else { "C:\\acc\\anthropic\\one" };
        let settings =
            serde_json::json!({ "model": "m", "background": true, "resumeOnRestart": true, "budgetTokens": 5, "connectors": true });
        register(id, provider, "C:\\p", account, Some(parent), false, settings);
    }

    /// Both providers alike: the parent's claim, its debt and its reach end in
    /// the one hold, it hears once, and so does a session that had written to
    /// the helper; what either had queued is in a file their line names.
    #[test]
    fn a_helper_taken_over_is_the_persons_and_its_parent_is_told_once() {
        for (provider, tag) in [(Provider::Claude, "cl"), (Provider::Codex, "cx")] {
            let (parent, peer, id) = (format!("to-{tag}-parent"), format!("to-{tag}-peer"), format!("to-{tag}-helper"));
            let none = serde_json::json!({});
            register(&parent, Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone());
            register(&peer, Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none);
            helper(&id, &parent, provider);
            crate::agents::registry::set_name(&id, "the job");
            reg().lock().unwrap().auto.insert(id.clone(), Auto::default());
            owe_from(&parent, &id, 1, 10);
            owe_from(&peer, &id, 1, 10);
            assert!(authorize(&parent, &id).is_ok());

            let queued = vec![
                crate::background::Queued::new(Some(parent.clone()), "and then this".into(), Vec::new()),
                crate::background::Queued::new(Some(peer.clone()), "the peer's words".into(), Vec::new()),
            ];
            take_over(&id, || Some(queued)).unwrap();

            assert_eq!(parent_of(&id), None);
            let e = authorize(&parent, &id).err().unwrap();
            assert!(e.contains("the person took") && e.contains("no longer yours"), "kill and delete: {e}");
            for verb in ["send to", "read", "wait on"] {
                let e = in_scope(&parent, &id, None, verb).unwrap_err();
                assert!(e.contains("no longer yours"), "{verb}: {e}");
            }
            assert!(authorize(&peer, &id).err().unwrap().contains("not one you spawned"), "another is told as ever");
            {
                let r = reg().lock().unwrap();
                assert!(!r.owed.keys().any(|(_, s)| *s == id), "nothing more is owed by it, to anyone");
                assert!(!r.auto.contains_key(&id), "nothing deletes it now but the person");
                let k = &r.known[&id];
                assert_eq!(k.settings, serde_json::json!({ "model": "m" }), "a chat has none of a helper's settings");
            }
            let to_parent = news_for(&parent);
            assert_eq!(to_parent.len(), 1, "{to_parent:?}");
            let line = &to_parent[0];
            assert!(
                line.contains(&format!("The person took over your helper {id} \"the job\"")) && line.contains("nothing more is owed"),
                "{line}"
            );
            let file = line.split("word for word in ").nth(1).unwrap();
            let held = std::fs::read_to_string(file).unwrap();
            assert!(held.contains("and then this") && !held.contains("the peer's words"), "{held}");
            let _ = std::fs::remove_file(file);
            let to_peer = news_for(&peer);
            assert_eq!(to_peer.len(), 1);
            assert!(to_peer[0].contains("which you wrote to, was taken over by the person"), "{}", to_peer[0]);
            let file = to_peer[0].split("word for word in ").nth(1).unwrap();
            assert!(std::fs::read_to_string(file).unwrap().contains("the peer's words"));
            let _ = std::fs::remove_file(file);
            // What a call of its parent's is refused with.
            assert_eq!(reg().lock().unwrap().taken_over.get(&id), Some(&parent));

            reg().lock().unwrap().news.retain(|(c, _)| *c != parent && *c != peer);
            for s in [&id, &parent, &peer] {
                forget(s);
            }
        }
    }

    /// A send whose text went into the queue just before the person took the
    /// helper over, and gets to its debt only after: the handover took the text
    /// back out and told its sender, and the person's chat owes it nothing. Its
    /// debt from before, if any, is gone with the others.
    #[test]
    fn a_send_caught_by_a_takeover_owes_nothing_of_the_persons_chat() {
        use crate::agents::owed::owe_from_at;
        for (provider, tag) in [(Provider::Claude, "cl"), (Provider::Codex, "cx")] {
            let (parent, id) = (format!("sc-{tag}-parent"), format!("sc-{tag}-helper"));
            register(&parent, Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, serde_json::json!({}));
            helper(&id, &parent, provider);
            crate::background::idle_for_test(&id, provider, 1, false);
            let before = handovers_of(&id);
            crate::background::promoting_for_test(&id, true);
            let placed = crate::background::send(&id, "one more thing", Some(&parent)).unwrap();
            assert_eq!(placed, crate::background::Placed::Queued, "held while it opens");
            // A debt it had registered by then, which the handover clears.
            let asked = crate::agents::owed::Asked { text: "one more thing", key: None, reply: true };
            owe_from_at(&parent, &id, 1, 0, before, &asked);
            take_over(&id, || Some(Vec::new())).unwrap();
            assert!(!owe_from_at(&parent, &id, 2, 0, before, &asked), "the send learns it was taken over");
            assert!(!reg().lock().unwrap().owed.contains_key(&(parent.clone(), id.clone())), "and nothing is owed");
            // Text sent to the person's chat from now on is owed as any chat's.
            assert!(owe_from_at(&parent, &id, 2, 0, handovers_of(&id), &asked));
            reg().lock().unwrap().news.retain(|(c, _)| *c != parent);
            crate::background::gone_for_test(&id);
            forget(&id);
            forget(&parent);
        }
    }

    /// The window's word that the row is made, said again because its answer was
    /// lost: nothing is handed over twice, and what the person's chat came to be
    /// owed in between stands. Its former parent hears once.
    #[test]
    fn a_handover_said_again_changes_nothing() {
        for (provider, tag) in [(Provider::Claude, "cl"), (Provider::Codex, "cx")] {
            let (parent, peer, id) = (format!("ha-{tag}-parent"), format!("ha-{tag}-peer"), format!("ha-{tag}-helper"));
            let none = serde_json::json!({});
            register(&parent, Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone());
            register(&peer, Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none);
            helper(&id, &parent, provider);
            crate::background::idle_for_test(&id, provider, 1, false);
            crate::background::background_promote(id.clone()).unwrap();
            assert_eq!(crate::background::background_promoted(id.clone(), None), Ok(true));
            // The person's chat goes on: another session asks it something.
            owe_from(&peer, &id, 2, 0);
            assert_eq!(crate::background::background_promoted(id.clone(), None), Ok(true), "the retry is answered as the first");
            assert!(reg().lock().unwrap().owed.contains_key(&(peer.clone(), id.clone())), "what it is owed since stands");
            assert_eq!(handovers_of(&id), 1);
            assert_eq!(news_for(&parent).len(), 1, "told once");
            reg().lock().unwrap().news.retain(|(c, _)| *c != parent);
            for s in [&id, &peer, &parent] {
                forget(s);
            }
        }
    }

    /// Deleted before the person's row was made: nothing to hand over, nobody told.
    #[test]
    fn a_helper_gone_meanwhile_is_nothing_to_hand_over() {
        register("tg-parent", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, serde_json::json!({}));
        let mut took = false;
        take_over("tg-gone", || {
            took = true;
            None
        })
        .unwrap();
        assert!(took, "its record is still let go of");
        assert!(news_for("tg-parent").is_empty());
        forget("tg-parent");
    }

    /// The helper's own helpers stay its: the person's chat owes them nothing
    /// new, and their answers still come to it.
    #[test]
    fn a_helpers_own_helpers_stay_with_it() {
        let none = serde_json::json!({});
        register("tk-parent", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none);
        helper("tk-mid", "tk-parent", Provider::Claude);
        helper("tk-low", "tk-mid", Provider::Codex);
        owe_from("tk-mid", "tk-low", 0, 0);
        take_over("tk-mid", || Some(Vec::new())).unwrap();
        assert_eq!(parent_of("tk-low").as_deref(), Some("tk-mid"));
        assert!(authorize("tk-parent", "tk-low").is_err(), "its former parent reaches them no more");
        assert!(reg().lock().unwrap().owed.contains_key(&("tk-mid".to_string(), "tk-low".to_string())));
        reg().lock().unwrap().news.retain(|(c, _)| c != "tk-parent");
        for id in ["tk-low", "tk-mid", "tk-parent"] {
            forget(id);
        }
    }
}
