//! Sessions going: `luna_kill`, `luna_delete` and the user's delete, the
//! `autoDelete` helpers `reap` takes, and what `forget` does with a deleted
//! chat's helpers and debts.

use crate::agents::ask::live;
use crate::agents::read::{transcript_path, transcripts};
use crate::agents::registry::{authorize, descendants, label, reg, Known, Registry};
use crate::agents::said::replies_since;
use crate::agents::spawn::worktree::worktree_of;
use crate::agents::{budget_owed, left_holds, left_quiet, save_quota_told};
use crate::throttle::now_ms;
use serde::Serialize;

/// An `autoDelete` helper goes once nobody is owed its answer and it has sat
/// idle this long since, nobody reading it either: room for a review of its
/// work and a follow-up first.
pub(super) const AUTO_DELETE_IDLE_MS: u64 = 30 * 60 * 1000;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Deleted {
    id: String,
}

/// A visible chat has passed to its deleted parent's parent (`forget`). The
/// window keeps a chat's parent and names it at every spawn, a core started
/// later included, so the row has to hear.
#[derive(Clone, Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct Reparented {
    pub(super) id: String,
    pub(super) parent: String,
}

/// An `autoDelete` helper's progress towards going.
#[derive(Default)]
pub(super) struct Auto {
    /// Since when it has been idle with nobody owed its answer.
    pub(super) idle_since: Option<u64>,
    /// Luna is deleting it now, so its end goes down as `auto-deleted`.
    pub(super) deleting: bool,
}

/// The window's Delete on a background helper: as its agent would, the
/// worktree it made going too, unless it was handed one or the person said to
/// keep it.
pub fn delete_by_user(id: &str, drop_worktree: bool) -> Result<(), String> {
    let known = reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(id).cloned().ok_or("no such helper")?;
    match remove(id, known, drop_worktree, None)?.worktree_error {
        Some(why) => Err(why),
        None => Ok(()),
    }
}

/// What `kill` did beyond stopping a turn.
#[derive(Serialize, Debug, Default)]
pub struct Killed {
    pub ok: bool,
    /// A background helper between turns: what its turns had left running,
    /// which is what there was to stop.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ended: Vec<String>,
}

/// Stops the session; the chat row stays and can be resumed by hand. A
/// background helper between turns has only what its turns left running to
/// stop (`background::kill`).
pub fn kill(caller: &str, id: &str) -> Result<Killed, String> {
    authorize(caller, id)?;
    let between_turns = crate::background::has(id) && crate::activity::turn_state(id).is_some_and(|t| !t.busy);
    // Stopped is not done with: what it was in the middle of is still there
    // to go on from, so it stays until it is deleted. Between turns it was in the
    // middle of nothing, and goes by itself as before.
    if !between_turns {
        reg().lock().unwrap_or_else(|e| e.into_inner()).auto.remove(id);
    }
    if crate::background::has(id) {
        let ended = crate::background::kill(id);
        return Ok(Killed { ok: true, ended });
    }
    let pty = crate::pty::manager();
    tauri::async_runtime::block_on(pty.kill(id));
    Ok(Killed { ok: true, ..Default::default() })
}

/// Stops the session, removes its attachments and — when asked — the
/// worktree it made (never one it was handed), then tells the frontend to
/// drop the row. Returns the worktree removed, if any. A session's own
/// children go on, passed up to its parent (see `forget`).
///
/// A person's Delete has its dialog to warn of the work in a worktree; an
/// agent's refuses while there is any, unless `force`. Nothing has been touched
/// by the time it does, the session still running if it was.
pub fn delete(caller: &str, id: &str, drop_worktree: bool, force: bool) -> Result<Removed, String> {
    let known = authorize(caller, id)?;
    if drop_worktree && !force {
        let folder = crate::worktree::project_of(&known.folder).unwrap_or_else(|| known.folder.clone());
        if let Some(wt) = worktree_to_drop(id, &folder) {
            if let Some(work) = crate::worktree::unsaved_work(&folder, &wt) {
                return Err(format!(
                    "not deleted: worktree {wt} holds {work}, which removing it would lose (files git ignores do not count, and would go too). \
                     Save what matters (commit and merge, or copy it), or call luna_delete without dropWorktree to keep the worktree, \
                     or with force: true to remove it anyway"
                ));
            }
            // Clean as it stands; a session still at it may yet change that. It
            // is stopped first and looked at again.
            stop_for_good(id);
            if let Some(work) = crate::worktree::unsaved_work(&folder, &wt) {
                return remove(id, known, false, None)
                    .map(|_| Removed::left(Some(wt), format!("kept: it came to hold {work} as the session was stopped")));
            }
            return remove(id, known, true, Some(wt));
        }
    }
    remove(id, known, drop_worktree, None)
}

/// Stops a session and what it has running, its row kept.
fn stop_for_good(id: &str) {
    if crate::background::has(id) {
        crate::background::stop_for_good(id);
    } else {
        tauri::async_runtime::block_on(crate::pty::manager().kill(id));
    }
}

/// The worktree `remove` would take with the session, as it would judge it:
/// one the session made, that it was not handed and no other session is in.
fn worktree_to_drop(id: &str, folder: &str) -> Option<String> {
    if reg().lock().unwrap_or_else(|e| e.into_inner()).borrowed.contains(id) {
        return None;
    }
    let pty = crate::pty::manager();
    let wt = if crate::background::has(id) {
        crate::background::removable_worktree(id)
    } else {
        let probe = pty.probe(id)?;
        let cwd = probe.working_dir().unwrap_or(probe.cwd);
        crate::worktree::is_worktree_of(folder, &cwd).then_some(cwd)
    }?;
    (!pty.worktree_shared(id, &wt)).then_some(wt)
}

/// What a delete did with the session's worktree. The session is gone either
/// way: a worktree that would not go (a file held open) is named, for the
/// caller to deal with, not an error that a retry could only answer with "not
/// one you spawned".
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Removed {
    pub ok: bool,
    pub worktree_removed: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_left: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_error: Option<String>,
    /// What went with a worktree without counting as work, said when one goes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<&'static str>,
}

/// Files git ignores do not stop a delete: build output would stop every one.
const IGNORED_GO: &str = "files git ignores went with it";

impl Removed {
    /// `ignored`: whether the worktree held files git ignores, as looked at
    /// before it went (`worktree::ignored_in`); not known, the note is said.
    fn of(worktree_removed: Option<String>, ignored: Option<bool>) -> Removed {
        let note = (worktree_removed.is_some() && ignored != Some(false)).then_some(IGNORED_GO);
        Removed { ok: true, worktree_removed, worktree_left: None, worktree_error: None, note }
    }

    fn left(worktree: Option<String>, why: String) -> Removed {
        crate::log::warn("delete", &format!("the session is gone, its worktree is not: {why}"));
        Removed { ok: true, worktree_removed: None, worktree_left: worktree, worktree_error: Some(why), note: None }
    }
}

/// `worktree`: the one to drop, where the session may no longer say itself.
fn remove(id: &str, known: Known, drop_worktree: bool, worktree: Option<String>) -> Result<Removed, String> {
    let pty = crate::pty::manager();
    // What `luna_sessions` history has left to go on once it is gone.
    if known.parent.is_some() || known.orphan {
        let transcript = transcript_path(pty, id);
        let last = transcript.as_ref().and_then(|_| replies_since(pty, id, known.provider, 0, false).texts.pop());
        crate::agentlog::left(
            id,
            transcript.map(|p| p.to_string_lossy().into_owned()).as_deref(),
            worktree_of(pty, id).path.as_deref(),
            last.as_deref(),
        );
    }
    // A helper resumed in a worktree knows the worktree as its folder.
    let folder = crate::worktree::project_of(&known.folder).unwrap_or(known.folder);
    let drop_worktree = drop_worktree && !reg().lock().unwrap_or_else(|e| e.into_inner()).borrowed.contains(id);
    if crate::background::has(id) {
        let who = label(id);
        let made = crate::background::remove(id);
        forget(id);
        crate::activity::forget(id);
        crate::log::info("delete", &format!("background helper {who}, worktree {made:?}, dropping {drop_worktree}"));
        return Ok(match made {
            // As for a chat (`PtyManager::delete`): a worktree another
            // session or helper is in stays, its work with it.
            Some(wt) if drop_worktree && pty.worktree_shared(id, &wt) => {
                crate::log::warn("delete", &format!("kept worktree {wt}: another session is in it"));
                Removed::of(None, None)
            }
            Some(wt) if drop_worktree => {
                let ignored = crate::worktree::ignored_in(&wt);
                match crate::worktree::remove_worktree_now(folder, wt.clone()) {
                    Ok(()) => Removed::of(Some(wt), ignored),
                    Err(why) => Removed::left(Some(wt), why),
                }
            }
            _ => Removed::of(None, None),
        });
    }
    // Asked before it goes: its process is what says where it works.
    let at_stake = if drop_worktree { worktree.clone().or_else(|| worktree_to_drop(id, &folder)) } else { None };
    let ignored = at_stake.as_deref().and_then(crate::worktree::ignored_in);
    // PtyManager::delete forgets the session here too (see `forget`).
    let deleted = tauri::async_runtime::block_on(pty.delete(id, &folder, worktree, drop_worktree));
    crate::emit::to_ui("agent://deleted", Deleted { id: id.to_string() });
    Ok(match deleted {
        // Without the drop, `delete` names the worktree that stays; what was
        // looked at is of the worktree that went only if that is the one.
        Ok(worktree) => {
            let gone = worktree.filter(|_| drop_worktree);
            let ignored = ignored.filter(|_| gone.as_deref().zip(at_stake.as_deref()).is_some_and(|(g, a)| crate::pty::same_dir(g, a)));
            Removed::of(gone, ignored)
        }
        Err(why) => Removed::left(at_stake, why),
    })
}

/// Deletes the `autoDelete` helpers that are done with: nobody is owed their
/// answer or waiting on them, nothing is owed to them, no helper of theirs is
/// still there, and they have sat idle — or exited — for AUTO_DELETE_IDLE_MS
/// since. Idle with processes left is idle once none of those holds the machine
/// any more (`left_holds`); they go with it. One a person has typed into is
/// theirs now and stays. Its worktree stays too, with whatever work is in it.
pub(super) fn reap(pty: &crate::pty::PtyManager) {
    let ids: Vec<String> = reg().lock().unwrap_or_else(|e| e.into_inner()).auto.keys().cloned().collect();
    reap_these(pty, ids);
}

fn reap_these(pty: &crate::pty::PtyManager, ids: Vec<String>) {
    for id in ids {
        if pty.typed_ago_ms(&id).is_some() {
            reg().lock().unwrap_or_else(|e| e.into_inner()).auto.remove(&id);
            continue;
        }
        // Between turns with no job left running; what its terminal last drew
        // and when does not matter here. Or with only what has done nothing
        // past LEFTOVER_HOLD_MS left: it holds nothing any more, it has had its
        // caller told it sat idle, and it goes with the helper (`remove` ends
        // what is left in its job), the debt for its end settled by one line.
        let ts = crate::activity::turn_state(&id);
        let between = ts.is_some_and(|t| t.turn == crate::activity::Turn::Idle);
        let only_idle = between && ts.is_some_and(|t| t.children_busy) && !left_holds(&id);
        let idle = !live(pty, &id) || (between && (ts.is_some_and(|t| !t.children_busy) || only_idle));
        // Gone, it would take with it what its own helpers are to answer it,
        // and what it would make of that.
        let below = descendants(&reg().lock().unwrap_or_else(|e| e.into_inner()), &id);
        let helpers_there = below.iter().any(|k| live(pty, k));
        let known = {
            let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
            let quiet = !helpers_there
                && !r.owed.iter().any(|((c, s), o)| (*s == id && !(only_idle && o.told_pending)) || *c == id)
                && !r.waits.keys().any(|(_, s)| *s == id)
                && !budget_owed(&r, &id);
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
        let left = only_idle.then(|| left_quiet(&id)).flatten();
        // Whoever was told it was not done yet, before its removal forgets them.
        let owed: Vec<String> = if left.is_some() {
            reg().lock().unwrap_or_else(|e| e.into_inner()).owed.keys().filter(|(_, s)| *s == id).map(|(c, _)| c.clone()).collect()
        } else {
            Vec::new()
        };
        match known.map(|k| (k.parent.clone(), remove(&id, k, false, None))) {
            Some((parent, Ok(_))) => {
                crate::log::info("agents", &format!("auto-deleted helper {}", label(&id)));
                if let Some((quiet, names)) = left {
                    reaped_with_leftovers(&id, owed, parent, quiet, &names);
                }
            }
            Some((_, Err(e))) => {
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
/// its sender (`from`, with why) may no longer send it there (background.rs
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

/// The lines waiting for a caller's next notice.
#[cfg(test)]
pub fn news_for(caller: &str) -> Vec<String> {
    reg().lock().unwrap_or_else(|e| e.into_inner()).news.iter().filter(|(c, _)| c == caller).map(|(_, l)| l.clone()).collect()
}

/// An `autoDelete` helper went with only idle leftovers: whoever was owed the
/// end of its job (`owed`, as it stood before the helper was removed) is told,
/// in one line, and owed nothing more; with nobody owed, its parent.
fn reaped_with_leftovers(id: &str, owed: Vec<String>, parent: Option<String>, quiet_ms: u64, names: &str) {
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    r.owed.retain(|(_, s), _| s != id);
    let line = format!(
        "Your helper {id} was auto-deleted: its turn was long over and what it had left running had done nothing for {} min: {names}, which went with it.",
        quiet_ms / 60_000
    );
    let to = if owed.is_empty() { parent.into_iter().collect() } else { owed };
    for caller in to {
        r.news.push((caller, line.clone()));
    }
}

/// A chat is gone, however it was deleted: nothing here may keep it — not in
/// `luna_sessions`, not against a caller's helper limit, not as a live token,
/// and not as an answer owed to or by it.
///
/// Its helpers go on. Each passes to its own parent, whose it already was
/// through this one, for that one's tools to go on reaching it. With no parent
/// to pass to, it is an orphan: no agent has any claim on it, so it is the
/// window's to stop or delete, and a background one, which no row shows and no
/// answer of which is awaited any more, goes by itself once it has been idle
/// for AUTO_DELETE_IDLE_MS, rather than hold its account and folder for good.
pub fn forget(id: &str) {
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let gone = r.known.get(id).cloned();
    let spawned = gone.as_ref().is_some_and(|k| k.parent.is_some() || k.orphan);
    let auto = r.auto.remove(id);
    r.known.remove(id);
    let (handed, passed, moved) = pass_up(&mut r, id, gone.and_then(|k| k.parent));
    r.tokens.retain(|_, chat| chat != id);
    r.seen.retain(|(who, chat), _| who != id && chat != id);
    r.delivery_floors.retain(|(who, chat), _| who != id && chat != id);
    r.owed.retain(|(who, chat), _| who != id && chat != id);
    r.answered.retain(|(who, chat), _| who != id && chat != id);
    r.abandoned.retain(|_, (caller, _)| caller != id);
    r.spawns.retain(|_, s| s.caller != id);
    r.resumes.remove(id);
    r.openings.remove(id);
    r.tasks.remove(id);
    r.worktree_names.remove(id);
    r.worktrees_before.remove(id);
    r.worktrees_left.remove(id);
    r.borrowed.remove(id);
    r.waits.retain(|(who, chat), _| who != id && chat != id);
    r.notified.remove(id);
    r.hook_asks.remove(id);
    r.notice_turns.remove(id);
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
    for chat in passed {
        crate::emit::to_ui("agent://reparent", chat);
    }
    for (helper, parent) in moved {
        crate::background::set_parent(&helper, parent);
    }
    if handed {
        crate::background::changed();
    }
    // A core killed now must not bring it back.
    crate::background::save();
}

/// `id`'s helpers pass to `up`, or with no `up` stand alone (see `forget`).
/// Returns whether it had any, the visible ones that have a new parent, and the
/// background ones with who they answer to now. A visible one with none keeps the
/// parent its row names: its row says it is an orphan by that, and in a core
/// started later a parent nobody knows reaches it no better.
fn pass_up(r: &mut Registry, id: &str, up: Option<String>) -> (bool, Vec<Reparented>, Vec<(String, Option<String>)>) {
    let Registry { known, auto, .. } = r;
    let mut handed = false;
    let mut passed = Vec::new();
    let mut moved = Vec::new();
    for (kid, k) in known.iter_mut().filter(|(_, k)| k.parent.as_deref() == Some(id)) {
        k.parent = up.clone();
        k.orphan = up.is_none();
        let background = k.settings["background"] == true;
        if k.orphan && background {
            auto.entry(kid.clone()).or_default();
        }
        if let (Some(parent), false) = (&up, background) {
            passed.push(Reparented { id: kid.clone(), parent: parent.clone() });
        }
        if background {
            moved.push((kid.clone(), up.clone()));
        }
        handed = true;
    }
    (handed, passed, moved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::owed::{owe_from, Owed};
    use crate::agents::persist::{ties_of, Ties};
    use crate::agents::registry::{authorize, authorize_view, descendants, parent_of, reg, register};
    use crate::agents::set_budget_state;
    use crate::agents::tests::tooled;
    use crate::provider::Provider;
    use std::sync::Mutex;

    #[test]
    fn an_auto_delete_helper_counts_idle_only_once_its_answer_is_out() {
        let _reaping = reaps_to_myself();
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

    /// An `autoDelete` helper kept forever by a `tail -F` that does nothing: once
    /// all it left has sat idle past its grace (LEFTOVER_HOLD_MS, its caller told
    /// of it long before), it goes as any idle one does, and that with it, the
    /// debt for its end settled by one line; a leftover at work still keeps it.
    #[test]
    fn an_auto_delete_helper_with_only_long_idle_leftovers_goes_and_says_so() {
        let _reaping = reaps_to_myself();
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
        let key = ("ai-top".to_string(), "ai-s".to_string());
        let mut told = Owed::pending(1, 0);
        told.told_idle = true;
        reg().lock().unwrap().owed.insert(key.clone(), told);
        let since = || reg().lock().unwrap().auto.get("ai-s").and_then(|a| a.idle_since);
        let pty = crate::pty::manager();
        let left = |quiet_ms: u64| crate::background::left_for_test("ai-s", &[("tail.exe", 22104, quiet_ms)]);
        left(0);
        reap(pty);
        assert_eq!(since(), None, "a leftover at work keeps it");
        left(crate::procs::LEFTOVER_HOLD_MS - 60_000);
        reap(pty);
        assert_eq!(since(), None, "idle, but still within its grace: the job may be waiting on something");
        left(crate::procs::LEFTOVER_HOLD_MS + 60_000);
        reap(pty);
        assert!(since().is_some(), "past its grace: the clock runs as for any idle helper, its open debt no bar");
        left(0);
        reap(pty);
        assert_eq!(since(), None, "at work again: it starts over");
        // Its time come: it goes, and whoever was owed its job's end hears so.
        left(crate::procs::LEFTOVER_HOLD_MS + 60_000);
        reap(pty);
        reg().lock().unwrap().auto.get_mut("ai-s").unwrap().idle_since = Some(now_ms() - AUTO_DELETE_IDLE_MS - 1);
        reap(pty);
        assert!(!crate::background::has("ai-s"), "auto-deleted");
        let r = reg().lock().unwrap();
        assert!(!r.owed.contains_key(&key), "owed nothing more");
        let said: Vec<&String> = r.news.iter().filter(|(c, _)| c == "ai-top").map(|(_, l)| l).collect();
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(said[0].contains("auto-deleted") && said[0].contains("tail.exe (pid 22104), which went with it"), "{}", said[0]);
        drop(r);
        reg().lock().unwrap().news.retain(|(c, _)| c != "ai-top");
        forget("ai-s");
        forget("ai-top");
    }

    #[test]
    fn a_helper_with_helpers_or_answers_to_come_is_not_auto_deleted() {
        let _reaping = reaps_to_myself();
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
        register("og-vis", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("og-top"), false, none.clone());
        register("og-other", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone());
        owe_from("og-mid", "og-low", 0, 0);
        assert!(authorize("og-top", "og-low").is_ok());
        // What the window is told, for a visible chat whose parent goes.
        register("rp-top", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone());
        register("rp-mid", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("rp-top"), true, none.clone());
        register("rp-vis", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("rp-mid"), false, none.clone());
        register("rp-bg", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("rp-mid"), false, bg.clone());
        let (handed, passed, moved) = pass_up(&mut reg().lock().unwrap(), "rp-mid", Some("rp-top".into()));
        assert!(handed);
        assert_eq!(moved, [("rp-bg".to_string(), Some("rp-top".to_string()))], "a background one hears as its next turn starts");
        assert_eq!(passed, [Reparented { id: "rp-vis".into(), parent: "rp-top".into() }], "a background one has no row to tell");
        // As a core started later finds it: the row now names the one it passed to.
        reg().lock().unwrap().known.remove("rp-vis");
        register("rp-vis", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("rp-top"), false, none.clone());
        assert!(authorize("rp-top", "rp-vis").is_ok(), "still within reach");
        let (_, passed, _) = pass_up(&mut reg().lock().unwrap(), "rp-top", None);
        assert!(passed.is_empty(), "with no one to pass to, the row keeps saying whose it was");
        for id in ["rp-bg", "rp-vis", "rp-mid", "rp-top"] {
            forget(id);
        }

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
        assert!(reg().lock().unwrap().auto.contains_key("og-low"), "a background one goes by itself once idle");
        assert!(reg().lock().unwrap().known.get("og-vis").unwrap().orphan);
        assert!(!reg().lock().unwrap().auto.contains_key("og-vis"), "a chat with a row is the person's to delete");
        assert!(authorize("og-other", "og-low").is_err(), "no other chat's agent has a claim on it");
        assert!(authorize_view("og-other", "og-low").is_ok(), "though it can read it, as any session");

        // Running again, the window names the parent it was spawned by; that stands no more.
        register("og-vis", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("og-top"), false, none.clone());
        assert_eq!(parent_of("og-vis"), None);
        // And saved with it, for the core after this one.
        let ties = ties_of("og-low").unwrap();
        assert!(ties.orphan && ties.parent.is_none() && ties.auto_delete);
        let old: Ties = serde_json::from_value(serde_json::json!({
            "parent": null, "name": null, "settings": {}, "tools": false, "borrowed": false, "autoDelete": false, "owed": [], "seen": []
        }))
        .expect("what an older core saved still reads");
        assert!(!old.orphan);
        for id in ["og-low", "og-vis", "og-other"] {
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
        assert!(!reg().lock().unwrap().auto.contains_key("ks-s"), "stopped mid-turn, it stays for luna_send");
        crate::background::gone_for_test("ks-s");
        forget("ks-s");
        forget("ks-c");
    }

    /// `reap` goes over every `autoDelete` helper there is, a test's or not, and in
    /// Luna it has one caller: two at once would both delete one.
    fn reaps_to_myself() -> std::sync::MutexGuard<'static, ()> {
        static ONE: Mutex<()> = Mutex::new(());
        ONE.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn auto_delete_keeps_a_helper_until_its_parent_has_heard_of_its_budget() {
        let pty = crate::pty::manager();
        let (parent, kid) = ("br-parent", "br-kid");
        tooled(parent);
        let breach = serde_json::json!({ "at": 1, "tokens": 5, "window": 10, "pct": 50 });
        register(
            kid,
            Provider::Claude,
            "C:\\p",
            "C:\\acc\\one",
            Some(parent),
            false,
            serde_json::json!({ "budgetTokens": 1, "budgetBreach": breach }),
        );
        let long_idle = || Auto { idle_since: Some(now_ms() - AUTO_DELETE_IDLE_MS - 1), deleting: false };
        reg().lock().unwrap().auto.insert(kid.to_string(), long_idle());
        reap_these(pty, vec![kid.to_string()]);
        assert!(reg().lock().unwrap().known.contains_key(kid), "the breach would go with it");
        assert_eq!(reg().lock().unwrap().auto.get(kid).unwrap().idle_since, None);
        set_budget_state(kid, "budgetTold", Some(serde_json::json!(true)));
        reg().lock().unwrap().auto.insert(kid.to_string(), long_idle());
        reap_these(pty, vec![kid.to_string()]);
        assert!(!reg().lock().unwrap().known.contains_key(kid), "told: it goes");
        forget(parent);
    }
}
