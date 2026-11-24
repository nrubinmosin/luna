//! Helpers going: `luna_session_stop`, `luna_session_delete` and the person's delete, the
//! `autoDelete` helpers `reap` takes, and what `forget` does with a deleted
//! session's helpers and debts. Every helper runs in the background: a chat
//! the window runs is the person's, and goes only by their hand.

use crate::agents::ask::live;
use crate::agents::notifier::system::skills_result;
use crate::agents::notifier::usage::save_quota_told;
use crate::agents::read::{transcript_path, transcripts};
use crate::agents::registry::{authorize, descendants, label, named, persons_chat_in, reg, top_of, Known, Registry};
use crate::agents::said::replies_since;
use crate::throttle::now_ms;
use serde::Serialize;

/// An `autoDelete` helper goes once nobody is owed its answer and it has sat
/// idle this long since, nobody reading it either: room for a review of its
/// work and a follow-up first.
pub(super) const AUTO_DELETE_IDLE_MS: u64 = 30 * 60 * 1000;

/// An `autoDelete` helper's progress towards going.
#[derive(Default)]
pub(super) struct Auto {
    /// Since when it has been idle with nobody owed its answer.
    pub(super) idle_since: Option<u64>,
    /// Luna is deleting it now, so its end goes down as `auto-deleted`.
    pub(super) deleting: bool,
}

/// What a helper has at stake in a delete, as the person's Delete dialog lists it:
/// whether it is at work, which a parent's checkbox alone does not stop
/// (`delete_by_user`).
#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AtStake {
    pub id: String,
    /// What it is doing that a delete would cut off: its turn, a question it
    /// waits on, what its turns left running.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working: Option<String>,
}

/// What a helper is doing that a delete would cut off, if anything.
fn at_work(id: &str) -> Option<String> {
    let t = crate::background::turn_state(id)?;
    match t.turn {
        crate::activity::Turn::Waiting => Some("waits on a question or permission".into()),
        crate::activity::Turn::Busy => Some("in the middle of a turn".into()),
        crate::activity::Turn::Idle if t.children_busy => Some("what its turns left is still running".into()),
        crate::activity::Turn::Idle => None,
    }
}

/// `AtStake` of each helper named, for the Delete dialog: looked at on demand.
pub fn at_stake(ids: &[String]) -> Vec<AtStake> {
    ids.iter().filter(|id| crate::background::has(id)).map(|id| AtStake { id: id.clone(), working: at_work(id) }).collect()
}

/// The window's Delete on a background helper. A helper at work is refused
/// unless `stop_working` says to stop it: its own yes, which a parent chat's
/// checkbox is not. Held from the first look to the removal (`claim_delete`): no
/// turn starts in between, so one found idle is idle when it goes, and the
/// person's opening it as a chat waits.
///
/// `of_chat`: the chat whose Delete dialog listed it. Refused if its line hangs
/// from another chat by now, checked under the delete hold. If the chat was
/// already deleted and left the helper with no chat above it, the delete goes on.
pub fn delete_by_user(id: &str, stop_working: bool, of_chat: Option<&str>) -> Result<(), String> {
    let claim = crate::background::claim_delete(id)?;
    if !claim.held() {
        return Err("no such helper".into());
    }
    let known = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(chat) = of_chat {
            let top = top_of(&r, id);
            if top != chat && persons_chat_in(&r, &top) {
                return Err(format!(
                    "{} is no longer under chat {}: it answers to chat {} now; not deleted",
                    named(&r, id),
                    named(&r, chat),
                    named(&r, &top)
                ));
            }
        }
        r.known.get(id).cloned().ok_or("no such helper")?
    };
    if !stop_working {
        if let Some(what) = at_work(id) {
            return Err(format!("{} is still working ({what}); not deleted", label(id)));
        }
    }
    remove(id, known, &claim)?;
    Ok(())
}

/// What `kill` did beyond stopping a turn.
#[derive(Serialize, Debug, Default)]
pub struct Killed {
    pub ok: bool,
    /// Between turns: what its turns had left running, which is what there was
    /// to stop.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ended: Vec<String>,
}

/// Stops a helper's turn; it stays, and `luna_session_send` carries it on. Between turns it
/// has only what its turns left running to stop (`background::kill`).
pub fn kill(caller: &str, id: &str) -> Result<Killed, String> {
    authorize(caller, id)?;
    if !crate::background::has(id) {
        return Err(format!("session {id} is not running"));
    }
    let between_turns = crate::activity::turn_state(id).is_some_and(|t| !t.busy);
    // Stopped is not done with: what it was in the middle of is still there
    // to go on from, so it stays until it is deleted. Between turns it was in the
    // middle of nothing, and goes by itself as before.
    if !between_turns {
        reg().lock().unwrap_or_else(|e| e.into_inner()).auto.remove(id);
    }
    let ended = crate::background::kill(id);
    Ok(Killed { ok: true, ended })
}

/// Stops the helper and removes it. Its own helpers go on, passed up to its
/// parent (see `forget`). A worktree it worked in is left as it is: Luna made
/// none and removes none.
///
/// Held from the first look to the removal (`claim_delete`), and whose it is asked
/// again under that hold: a person's opening it as a chat either finished before
/// (it is theirs, and refused) or waits until this is through, never lands between
/// what this went by and what it removes.
pub fn delete(caller: &str, id: &str) -> Result<Removed, String> {
    authorize(caller, id)?;
    let claim = crate::background::claim_delete(id)?;
    let known = authorize(caller, id)?;
    remove(id, known, &claim)
}

/// What a delete answers: the helper is gone.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Removed {
    pub ok: bool,
}

/// Only under a delete's hold on the helper (`claim_delete`), and only while it
/// is still a helper: never a chat the person took over.
fn remove(id: &str, known: Known, _held: &crate::background::DeleteClaim) -> Result<Removed, String> {
    if reg().lock().unwrap_or_else(|e| e.into_inner()).taken_over.contains_key(id) {
        return Err(format!("{id} is the person's own chat now, not a helper to delete"));
    }
    let pty = crate::pty::manager();
    // What `luna_run_list` has left to go on once it is gone.
    let transcript = transcript_path(pty, id);
    let last = transcript.as_ref().and_then(|_| replies_since(pty, id, known.provider, 0, false).texts.pop());
    crate::agentlog::left(id, transcript.map(|p| p.to_string_lossy().into_owned()).as_deref(), last.as_deref());
    let who = label(id);
    crate::background::remove(id);
    forget(id);
    crate::activity::forget(id);
    crate::log::info("delete", &format!("helper {who}"));
    Ok(Removed { ok: true })
}

/// Deletes the `autoDelete` helpers that are done with: nobody is owed their
/// answer or waiting on them, nothing is owed to them, no helper of theirs is
/// still there, nothing their turns left is still in their job, and they have
/// sat idle — or exited — for AUTO_DELETE_IDLE_MS since. Its worktree stays,
/// with whatever work is in it.
pub(super) fn reap(pty: &crate::pty::PtyManager) {
    let ids: Vec<String> = reg().lock().unwrap_or_else(|e| e.into_inner()).auto.keys().cloned().collect();
    #[cfg(test)]
    let ids = ids.into_iter().filter(|id| crate::agents::sweep_includes(id)).collect();
    reap_these(pty, ids);
}

fn reap_these(pty: &crate::pty::PtyManager, ids: Vec<String>) {
    for id in ids {
        // Not active by its record: between turns with nothing left running,
        // however long that has done nothing.
        let idle = !live(pty, &id) || crate::activity::life(&id).is_some_and(|l| !l.active());
        // Gone, it would take with it what its own helpers are to answer it,
        // and what it would make of that.
        let below = descendants(&reg().lock().unwrap_or_else(|e| e.into_inner()), &id);
        let helpers_there = below.iter().any(|k| live(pty, k));
        let known = {
            let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
            let quiet = !helpers_there && !r.owed.iter().any(|((c, s), _)| *s == id || *c == id) && !r.waits.keys().any(|(_, s)| *s == id);
            let known = r.known.get(&id).cloned();
            let Some(a) = r.auto.get_mut(&id) else { continue };
            if !(quiet && idle) {
                a.idle_since = None;
                continue;
            }
            let since = *a.idle_since.get_or_insert(now_ms());
            if now_ms().saturating_sub(since) < AUTO_DELETE_IDLE_MS {
                continue;
            }
            a.deleting = true;
            known
        };
        // Held as any delete is (`delete`), and still `auto`'s under the hold: the
        // person opening it as a chat meanwhile took it off, or holds it now.
        let claim = crate::background::claim_delete(&id);
        let still = reg().lock().unwrap_or_else(|e| e.into_inner()).auto.get(&id).is_some_and(|a| a.deleting);
        let claim = match claim {
            Ok(claim) if still => claim,
            _ => {
                if let Some(a) = reg().lock().unwrap_or_else(|e| e.into_inner()).auto.get_mut(&id) {
                    a.deleting = false;
                }
                continue;
            }
        };
        match known.map(|k| remove(&id, k, &claim)) {
            Some(Ok(_)) => crate::log::info("agents", &format!("auto-deleted helper {}", label(&id))),
            Some(Err(e)) => {
                crate::log::warn("agents", &format!("could not auto-delete helper {}: {e}", label(&id)));
                reg().lock().unwrap_or_else(|e| e.into_inner()).auto.remove(&id);
            }
            None => {
                reg().lock().unwrap_or_else(|e| e.into_inner()).auto.remove(&id);
            }
        }
    }
}

/// Text queued for background helper `id` was dropped when its turn came, as
/// its sender (`from`, with why) may no longer send it there (background/queue.rs
/// `settle_batch`): the sender hears so in its next notice, having been told
/// the text was taken; with the sender gone, the helper's parent, without the
/// text, which was not its own.
pub fn queued_refused(id: &str, refused: &[(String, String)]) {
    if refused.is_empty() {
        return;
    }
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let parent = r.known.get(id).and_then(|k| k.parent.clone());
    for (from, why) in refused {
        let told = if r.known.contains_key(from) {
            (from.clone(), format!("Text you sent to helper {id} did not go in when its turn came, and will not: {why}"))
        } else if let Some(parent) = &parent {
            (
                parent.clone(),
                format!(
                    "Text that {from}, since gone, had queued for your helper {id} did not go in when its turn came, and \
                     will not: it started with a command to the CLI, which {from} may no longer send there."
                ),
            )
        } else {
            continue;
        };
        r.news.push(told);
    }
}

/// What applying the library's folder skills did on this delegate turn.
pub fn turn_folder_skills(id: &str, failure: Option<&str>) {
    let changed = skills_result(&mut reg().lock().unwrap_or_else(|e| e.into_inner()), id, failure);
    if changed {
        crate::agents::save_chats();
    }
}

/// The lines waiting for a caller's next notice.
#[cfg(test)]
pub fn news_for(caller: &str) -> Vec<String> {
    reg().lock().unwrap_or_else(|e| e.into_inner()).news.iter().filter(|(c, _)| c == caller).map(|(_, l)| l.clone()).collect()
}

/// A session is gone, however it was deleted: nothing here may keep it — not in
/// `luna_session_list`, not against a caller's helper limit, not as a live token,
/// and not as an answer owed to or by it.
///
/// Its helpers go on. Each passes to its own parent, whose it already was
/// through this one, for that one's tools to go on reaching it. With no parent
/// to pass to, it is an orphan: no agent has any claim on it, so it is the
/// window's to stop or delete, and, as no answer of it is awaited any more, it
/// goes by itself once it has been idle for AUTO_DELETE_IDLE_MS, rather than
/// hold its account and folder for good.
pub fn forget(id: &str) {
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let gone = r.known.get(id).cloned();
    let spawned = gone.as_ref().is_some_and(|k| k.parent.is_some() || k.orphan);
    let auto = r.auto.remove(id);
    r.known.remove(id);
    let (moved, change) = pass_up(&mut r, id, gone.and_then(|k| k.parent));
    r.tokens.retain(|_, chat| chat != id);
    r.seen.retain(|(who, chat), _| who != id && chat != id);
    r.delivery_floors.retain(|(who, chat), _| who != id && chat != id);
    r.owed.retain(|(who, chat), _| who != id && chat != id);
    r.answered.retain(|(who, chat), _| who != id && chat != id);
    r.spawns.retain(|_, s| s.caller != id);
    r.tasks.remove(id);
    r.taken_over.retain(|chat, from| chat != id && from != id);
    r.waits.retain(|(who, chat), _| who != id && chat != id);
    r.notified.remove(id);
    r.places_refused.remove(id);
    r.system_news.retain(|n| !n.about(id));
    r.hook_asks.remove(id);
    r.notice_turns.remove(id);
    r.woken.remove(id);
    r.delegates.remove(id);
    r.requests.retain(|(who, chat), _| who != id && chat != id);
    r.notice_times.retain(|(who, chat), _| who != id && chat != id);
    r.stopped.retain(|(who, chat), _| who != id && chat != id);
    r.exits.remove(id);
    let told = r.quota_told.len();
    r.quota_told.retain(|(caller, _, _), _| caller != id);
    let quota_now = r.quota_told.len() != told;
    r.receipts.retain(|(who, chat), _| who != id && chat != id);
    drop(r);
    if quota_now {
        save_quota_told();
    }
    transcripts().lock().unwrap_or_else(|e| e.into_inner()).remove(id);
    if spawned {
        crate::agentlog::ended(id, if auto.is_some_and(|a| a.deleting) { "auto-deleted" } else { "deleted" });
    }
    let handed = !moved.is_empty();
    // Off the registry's lock. A later pass-up may already have updated the helper.
    for (helper, parent) in moved {
        crate::background::set_parent(&helper, parent, change);
    }
    if handed {
        crate::background::changed();
    }
    // A core killed now must not bring it back.
    crate::background::save();
}

/// `id`'s helpers pass to `up`, or with no `up` stand alone (see `forget`).
/// Returns each with whom it answers to now, and the count of parent changes
/// this one is (`Registry::parent_changes`).
fn pass_up(r: &mut Registry, id: &str, up: Option<String>) -> (Vec<(String, Option<String>)>, u64) {
    r.parent_changes += 1;
    let change = r.parent_changes;
    let Registry { known, auto, .. } = r;
    let mut moved = Vec::new();
    for (kid, k) in known.iter_mut().filter(|(_, k)| k.parent.as_deref() == Some(id)) {
        k.parent = up.clone();
        k.orphan = up.is_none();
        if k.orphan {
            auto.entry(kid.clone()).or_default();
        }
        moved.push((kid.clone(), up.clone()));
    }
    (moved, change)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::owed::owe_from;
    use crate::agents::persist::{ties_of, Ties};
    use crate::agents::registry::{authorize, authorize_view, descendants, parent_of, reg, register};
    use crate::provider::Provider;

    #[test]
    fn an_auto_delete_helper_counts_idle_only_once_its_answer_is_out() {
        let _sweep = crate::agents::test_support::sweep_only(&["ad-child"]);
        let none = serde_json::json!({});
        register("ad-parent", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone());
        register("ad-child", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("ad-parent"), false, none);
        reg().lock().unwrap().auto.insert("ad-child".into(), Auto::default());
        let since = || reg().lock().unwrap().auto.get("ad-child").and_then(|a| a.idle_since);
        let pty = crate::pty::manager();
        owe_from("ad-parent", "ad-child", 0, 0);
        reap(pty);
        assert_eq!(since(), None, "its answer is still owed");
        reg().lock().unwrap().owed.remove(&("ad-parent".to_string(), "ad-child".to_string()));
        reap(pty);
        assert!(since().is_some(), "exited and nobody owed: the clock runs");
        owe_from("ad-parent", "ad-child", 1, 0);
        reap(pty);
        assert_eq!(since(), None, "a follow-up starts it over");
        reg().lock().unwrap().auto.remove("ad-child");
    }

    /// An `autoDelete` helper with something its turns left still in its job is
    /// active, however long that has done nothing: it is not reaped until that
    /// has gone, and then goes as any idle one does.
    #[test]
    fn an_auto_delete_helper_stays_while_what_it_left_is_there() {
        let _sweep = crate::agents::test_support::sweep_only(&["ai-s"]);
        register("ai-top", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, serde_json::json!({}));
        register(
            "ai-s",
            Provider::Claude,
            "C:\\p",
            "C:\\acc\\anthropic\\one",
            Some("ai-top"),
            false,
            serde_json::json!({ "background": true }),
        );
        crate::background::idle_for_test("ai-s", Provider::Claude, 1, false);
        reg().lock().unwrap().auto.insert("ai-s".into(), Auto::default());
        let since = || reg().lock().unwrap().auto.get("ai-s").and_then(|a| a.idle_since);
        let pty = crate::pty::manager();
        crate::background::left_for_test("ai-s", &[("tail.exe", 22104)]);
        reap(pty);
        assert_eq!(since(), None, "what it left keeps it");
        crate::background::left_for_test("ai-s", &[]);
        reap(pty);
        assert!(since().is_some(), "gone: the clock runs as for any idle helper");
        crate::background::gone_for_test("ai-s");
        forget("ai-s");
        forget("ai-top");
    }

    #[test]
    fn a_helper_with_helpers_or_answers_to_come_is_not_auto_deleted() {
        let _sweep = crate::agents::test_support::sweep_only(&["rq-mid"]);
        let none = serde_json::json!({});
        register("rq-top", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone());
        register("rq-mid", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("rq-top"), true, none.clone());
        register("rq-low", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("rq-mid"), false, none);
        reg().lock().unwrap().auto.insert("rq-mid".into(), Auto::default());
        let since = || reg().lock().unwrap().auto.get("rq-mid").and_then(|a| a.idle_since);
        let pty = crate::pty::manager();
        owe_from("rq-mid", "rq-low", 0, 0);
        reap(pty);
        assert_eq!(since(), None, "an answer is still owed to it");
        reg().lock().unwrap().owed.remove(&("rq-mid".to_string(), "rq-low".to_string()));
        reap(pty);
        assert!(since().is_some(), "nothing owed, and its helper has gone: the clock runs");
        for id in ["rq-low", "rq-mid", "rq-top"] {
            forget(id);
        }
    }

    #[test]
    fn a_deleted_chats_helpers_pass_to_its_parent_or_stand_alone() {
        let none = serde_json::json!({});
        let bg = serde_json::json!({ "background": true });
        register("og-top", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone());
        register("og-mid", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("og-top"), true, bg.clone());
        register("og-low", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("og-mid"), false, bg.clone());
        register("og-other", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone());
        owe_from("og-mid", "og-low", 0, 0);
        assert!(authorize("og-top", "og-low").is_ok());
        let (moved, _) = pass_up(&mut reg().lock().unwrap(), "og-nobody", Some("og-top".into()));
        assert!(moved.is_empty(), "nobody's helpers, nothing moved");

        // A helper in the middle goes: its helper is the top chat's directly,
        // which could reach it all along.
        forget("og-mid");
        assert_eq!(parent_of("og-low").as_deref(), Some("og-top"));
        assert!(authorize("og-top", "og-low").is_ok(), "kill and delete still reach it");
        assert!(list_ids("og-top").contains(&"og-low".to_string()), "and the list shows it");
        assert!(!reg().lock().unwrap().auto.contains_key("og-low"), "its parent's to delete, as it says");

        // The top chat goes: nobody is left above them.
        forget("og-top");
        assert_eq!(parent_of("og-low"), None);
        assert!(reg().lock().unwrap().known.get("og-low").unwrap().orphan);
        assert!(reg().lock().unwrap().auto.contains_key("og-low"), "it goes by itself once idle");
        assert!(authorize("og-other", "og-low").is_err(), "no other chat's agent has a claim on it");
        assert!(authorize_view("og-other", "og-low").is_ok(), "though it can read it, as any session");

        // Saved with it, for the core after this one.
        let ties = ties_of("og-low").unwrap();
        assert!(ties.orphan && ties.parent.is_none() && ties.auto_delete);
        let old: Ties = serde_json::from_value(serde_json::json!({
            "parent": null, "name": null, "settings": {}, "tools": false, "borrowed": false, "autoDelete": false, "owed": [], "seen": []
        }))
        .expect("what an older core saved still reads");
        assert!(!old.orphan);
        for id in ["og-low", "og-other"] {
            forget(id);
        }
    }

    fn list_ids(caller: &str) -> Vec<String> {
        descendants(&reg().lock().unwrap(), caller)
    }

    #[test]
    fn a_stop_between_turns_ends_what_is_left_and_leaves_it_to_go_by_itself() {
        let none = serde_json::json!({});
        register("ks-c", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone());
        register(
            "ks-s",
            Provider::Claude,
            "C:\\p",
            "C:\\acc\\anthropic\\one",
            Some("ks-c"),
            false,
            serde_json::json!({ "background": true }),
        );
        crate::background::idle_for_test("ks-s", Provider::Claude, 1, false);
        reg().lock().unwrap().auto.insert("ks-s".into(), Auto::default());
        let k = kill("ks-c", "ks-s").unwrap();
        assert!(k.ok && k.ended.is_empty());
        assert!(reg().lock().unwrap().auto.contains_key("ks-s"), "between turns it was in the middle of nothing");
        crate::background::in_turn_for_test("ks-s");
        kill("ks-c", "ks-s").unwrap();
        assert!(!reg().lock().unwrap().auto.contains_key("ks-s"), "stopped mid-turn, it stays for luna_session_send");
        crate::background::gone_for_test("ks-s");
        forget("ks-s");
        forget("ks-c");
    }

    /// A helper the person is opening as a chat is theirs to be: its parent can
    /// neither stop nor delete it meanwhile.
    #[test]
    fn a_helper_being_opened_by_the_person_is_not_its_parents_to_stop_or_delete() {
        for provider in [Provider::Claude, Provider::Codex] {
            let none = serde_json::json!({});
            register("po-c", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none);
            register("po-s", provider, "C:\\p", "C:\\acc\\anthropic\\one", Some("po-c"), false, serde_json::json!({ "background": true }));
            crate::background::idle_for_test("po-s", provider, 1, false);
            crate::background::promoting_for_test("po-s", true);
            assert!(kill("po-c", "po-s").err().unwrap().contains("the person is opening po-s"));
            assert!(delete("po-c", "po-s").err().unwrap().contains("the person is opening po-s"));
            assert!(crate::background::has("po-s"), "nothing was touched");
            crate::background::promoting_for_test("po-s", false);
            assert!(kill("po-c", "po-s").is_ok(), "not opened after all: its parent's again");
            crate::background::gone_for_test("po-s");
            forget("po-s");
            forget("po-c");
        }
    }

    /// A delete holds the helper from its first look to its removal, and an opening
    /// as a chat holds it the same way: neither lands inside the other, and no turn
    /// starts under either. A removal that finds the helper the person's chat by
    /// then refuses. Both providers alike.
    #[test]
    fn a_delete_and_an_opening_hold_each_other_off() {
        use crate::background::{background_promote, claim_delete, Placed};
        for provider in [Provider::Claude, Provider::Codex] {
            register("dh-c", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, serde_json::json!({}));
            register("dh-s", provider, "C:\\p", "C:\\acc\\anthropic\\one", Some("dh-c"), false, serde_json::json!({ "background": true }));
            crate::background::idle_for_test("dh-s", provider, 1, false);
            let claim = claim_delete("dh-s").unwrap();
            assert!(claim.held());
            assert!(background_promote("dh-s".into()).err().unwrap().contains("being deleted"));
            assert!(claim_delete("dh-s").err().unwrap().contains("being deleted already"));
            assert!(delete_by_user("dh-s", false, None).is_err(), "nor the person's at once");
            // Text sent meanwhile waits: it found the helper idle, and idle it stays.
            assert_eq!(crate::background::send("dh-s", "one more", Some("dh-c")).unwrap(), Placed::Queued);
            assert_eq!(at_work("dh-s"), None);
            crate::background::gone_for_test("dh-s");
            drop(claim);

            crate::background::idle_for_test("dh-s", provider, 1, false);
            crate::background::promoting_for_test("dh-s", true);
            assert!(claim_delete("dh-s").err().unwrap().contains("the person is opening dh-s"));
            assert!(delete_by_user("dh-s", true, None).err().unwrap().contains("the person is opening dh-s"));
            assert!(delete("dh-c", "dh-s").is_err());
            assert!(crate::background::has("dh-s"), "nothing was touched");

            // Handed over all the same: never removed as a helper.
            crate::agents::take_over("dh-s", || Some(Vec::new())).unwrap();
            crate::background::promoting_for_test("dh-s", false);
            let known = reg().lock().unwrap().known.get("dh-s").cloned().unwrap();
            let claim = claim_delete("dh-s").unwrap();
            assert!(remove("dh-s", known, &claim).err().unwrap().contains("the person's own chat"));
            assert!(reg().lock().unwrap().known.contains_key("dh-s"), "the chat is still there");
            crate::background::gone_for_test("dh-s");
            drop(claim);
            reg().lock().unwrap().news.retain(|(c, _)| c != "dh-c");
            forget("dh-s");
            forget("dh-c");
        }
    }

    /// The person's Delete never stops a helper at work on a parent chat's
    /// checkbox alone: that takes its own yes.
    #[test]
    fn the_persons_delete_spares_a_working_helper_unless_told() {
        for provider in [Provider::Claude, Provider::Codex] {
            register("ud-c", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, serde_json::json!({}));
            let bg = serde_json::json!({ "background": true });
            register("ud-s", provider, "C:\\p", "C:\\acc\\anthropic\\one", Some("ud-c"), false, bg);
            crate::background::idle_for_test("ud-s", provider, 1, false);
            crate::background::in_turn_for_test("ud-s");
            let stake = at_stake(&["ud-s".to_string(), "ud-nobody".to_string()]);
            assert_eq!(stake.len(), 1, "only a helper there is");
            assert_eq!(stake[0].working.as_deref(), Some("in the middle of a turn"));
            let e = delete_by_user("ud-s", false, Some("ud-c")).unwrap_err();
            assert!(e.contains("still working") && e.contains("not deleted"), "{e}");
            assert!(crate::background::has("ud-s"), "still there");
            // Waiting on a question and a job left running are at work too.
            crate::background::hold_for_test("ud-s");
            assert_eq!(at_stake(&["ud-s".to_string()])[0].working.as_deref(), Some("waits on a question or permission"));
            crate::background::idle_for_test("ud-s", provider, 1, true);
            assert!(delete_by_user("ud-s", false, Some("ud-c")).unwrap_err().contains("left is still running"));
            // Told to stop it, it goes.
            delete_by_user("ud-s", true, Some("ud-c")).unwrap();
            assert!(!crate::background::has("ud-s") && !reg().lock().unwrap().known.contains_key("ud-s"));
            forget("ud-c");
        }
    }
}
