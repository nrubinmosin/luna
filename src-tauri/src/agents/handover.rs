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
pub fn take_over(id: &str, take_record: impl FnOnce() -> Option<Vec<crate::background::Queued>>) {
    let Some(taken) = change_hands(id) else {
        take_record();
        return;
    };
    let unsent = take_record().unwrap_or_default();
    tell(id, &taken, unsent);
    crate::agentlog::ended(id, "taken over by the person");
    crate::log::warn(
        "agents",
        &format!(
            "{} taken over by the person as their own chat; {} no longer has it",
            label(id),
            taken.parent.as_deref().unwrap_or("nobody")
        ),
    );
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

/// Whose helper `id` was before the person took it over, while this core knows:
/// what a call of that parent's is refused with.
pub(super) fn taken_from(id: &str) -> Option<String> {
    reg().lock().unwrap_or_else(|e| e.into_inner()).taken_over.get(id).cloned()
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
            take_over(&id, || Some(queued));

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
            assert_eq!(taken_from(&id).as_deref(), Some(parent.as_str()));

            reg().lock().unwrap().news.retain(|(c, _)| *c != parent && *c != peer);
            for s in [&id, &parent, &peer] {
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
        });
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
        take_over("tk-mid", || Some(Vec::new()));
        assert_eq!(parent_of("tk-low").as_deref(), Some("tk-mid"));
        assert!(authorize("tk-parent", "tk-low").is_err(), "its former parent reaches them no more");
        assert!(reg().lock().unwrap().owed.contains_key(&("tk-mid".to_string(), "tk-low".to_string())));
        reg().lock().unwrap().news.retain(|(c, _)| c != "tk-parent");
        for id in ["tk-low", "tk-mid", "tk-parent"] {
            forget(id);
        }
    }
}
