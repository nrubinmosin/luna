//! The person moves a helper, with the helpers under it, to another chat
//! (ADR-0028): from the helper's menu in the window ("Move to chat…"), or from
//! the "Move to B?" its row offers once chat B was refused a write into it
//! (`asked_to_move`). No agent may: it is a window command, not a tool.
//!
//! Only the helper's parent changes, so everything under it follows; ids,
//! worktrees, settings and constraints stay. What its old chat was owed by it
//! and the helpers under it is closed with one line to that chat, and its
//! later calls on them are refused naming the new one (`moved_error`); its
//! waits without `scope: "all"` end with that refusal at their next look, its
//! waits with it go on. The new chat is owed the answer the helper is at work
//! on, if it is. Text queued for it still goes in: Claude Code joins it to a
//! turn running or takes it next, Codex at its next turn, and either's next
//! turn is told its new chat in its prompt (`move_note`).
//!
//! The order is what keeps it whole: the notifier's hold on every session
//! that moves (`hand_off`), so a notice about one going in is in before the
//! debts change and none starts until they have; whose the helpers are held
//! alone (`ownership_changing`), so a send or an answer checked against the old
//! chat has left its debt before, or is checked against the new one after;
//! the helpers held between turns (`claim_move`), so the next turn starts
//! under its new parent; and the checks and the change in one hold of the
//! registry, where a spawn counts its places.

use crate::agents::notifier::hand_off;
use crate::agents::owed::{owe_from, seen};
use crate::agents::privilege::{helpers_above_caller, holds_over};
use crate::agents::read::transcript_end;
use crate::agents::registry::{
    descendants, is_descendant, label, named, ownership_changing, persons_chat_in, reg, top_of, Registry, DEV_CALLER,
};
use crate::agents::spawn::{max_descendants, under_cap};
use crate::throttle::now_ms;
use serde::{Deserialize, Serialize};

/// The person's last move of a helper to another chat, kept in its ties.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Moved {
    /// The chat it was under; None for one that had no chat above it.
    pub from: Option<String>,
    pub to: String,
    pub at_ms: u64,
    /// A turn of its own has been told (`move_note`).
    #[serde(default)]
    pub told: bool,
}

/// A chat of the person's that was refused a write into a helper not under it
/// (registry.rs `WriteRefused::NotAbove`), and when it last was.
#[derive(Clone, Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Suggested {
    pub chat: String,
    pub at_ms: u64,
}

/// Chat `chat` asked to write into helper `id` and was refused for not being
/// above it: the window may offer to move `id` there. Only for a person's chat;
/// a helper refused has no such way. Whether it is new, for the rows to be told.
pub(super) fn asked_to_move(r: &mut Registry, chat: &str, id: &str) -> bool {
    if chat == DEV_CALLER || !persons_chat_in(r, chat) || !r.known.contains_key(id) {
        return false;
    }
    let now = now_ms();
    let asked = r.suggested.entry(id.to_string()).or_default();
    match asked.iter_mut().find(|s| s.chat == chat) {
        Some(s) => {
            s.at_ms = now;
            false
        }
        None => {
            asked.push(Suggested { chat: chat.to_string(), at_ms: now });
            true
        }
    }
}

/// The helper moved away from `caller` that `target` is, or is under, if the
/// person moved one: `caller` was its chat, and is not above it now.
pub(super) fn moved_away(r: &Registry, caller: &str, target: &str) -> Option<String> {
    if is_descendant(r, caller, target) {
        return None;
    }
    let mut cur = target.to_string();
    for _ in 0..16 {
        if r.moved.get(&cur).is_some_and(|m| m.from.as_deref() == Some(caller)) {
            return Some(cur);
        }
        cur = r.known.get(&cur)?.parent.clone()?;
    }
    None
}

/// What a former chat is told of a call on `id`, which went with `root` when the
/// person moved it: who has it now, and what is still open to it.
pub(super) fn moved_error(r: &Registry, id: &str, root: &str) -> String {
    let to = r.moved.get(root).map_or_else(|| "another chat".to_string(), |m| format!("chat {}", named(r, &m.to)));
    let what = if root == id { format!("the person moved {id}") } else { format!("the person moved {root}, and {id} with it,") };
    format!(
        "{what} to {to} in Luna's window: it is no longer yours to send to, answer, stop or delete, and nothing more is \
         owed by it. You may still read it and wait on it, with scope: \"all\""
    )
}

/// The line a moved helper's next turn is given (background/command.rs
/// `stdin_with`), until a turn has had it (`move_told`).
pub fn move_note(id: &str) -> Option<String> {
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let m = r.moved.get(id).filter(|m| !m.told)?;
    let from = match &m.from {
        Some(from) => format!("from chat {}", named(&r, from)),
        None => "from no chat (the one above you had gone)".to_string(),
    };
    Some(format!(
        "The person moved you, in Luna's window, {from} to chat {}: you answer to that chat now. Your answers, \
         reports and questions go to it, and only it and the sessions above you send to you.",
        named(&r, &m.to)
    ))
}

/// A turn started with `move_note` in its prompt: none after it is given it again.
pub fn move_told(id: &str) {
    if let Some(m) = reg().lock().unwrap_or_else(|e| e.into_inner()).moved.get_mut(id) {
        m.told = true;
    }
    crate::background::save();
}

/// What a helper's row says of whose it is (background/rows.rs `Row`).
#[derive(Default)]
pub struct Ownership {
    /// The chat at the top of its line; None with no chat above it.
    pub top: Option<String>,
    /// The person's chats that wait on it now without being above it.
    pub watched_by: Vec<String>,
    pub moved: Option<Moved>,
    /// The chats refused a write into it that it could be moved to.
    pub suggested: Vec<Suggested>,
}

pub fn ownership_of(id: &str) -> Ownership {
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let Some(k) = r.known.get(id) else { return Ownership::default() };
    let top = k.parent.as_ref().map(|_| top_of(&r, id));
    let mut watched_by: Vec<String> = r
        .waits
        .keys()
        .filter(|(c, s)| s == id && c != DEV_CALLER && persons_chat_in(&r, c) && !is_descendant(&r, c, id))
        .map(|(c, _)| c.clone())
        .collect();
    watched_by.sort();
    watched_by.dedup();
    let suggested = r
        .suggested
        .get(id)
        .map(|asked| asked.iter().filter(|s| r.known.contains_key(&s.chat) && !is_descendant(&r, &s.chat, id)).cloned().collect())
        .unwrap_or_default();
    Ownership { top, watched_by, moved: r.moved.get(id).cloned(), suggested }
}

/// What the person's move did (`move_helper`).
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct HelperMoved {
    pub id: String,
    pub from: Option<String>,
    pub to: String,
    /// Every session that moved: the helper and those under it.
    pub sessions: Vec<String>,
    /// The `autoDelete` its chat's going had switched on is off again.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub auto_delete_off: bool,
    /// It was at work (a turn, text queued, a question held): the new chat is
    /// owed its answer.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub owed: bool,
}

/// Why helper `id` cannot be moved at all: Ok with its chat now (None: none)
/// and every session that would move, it first.
fn check_source(r: &Registry, id: &str) -> Result<(Option<String>, Vec<String>), String> {
    let k = r.known.get(id).ok_or_else(|| format!("no helper {id}"))?;
    if persons_chat_in(r, id) {
        return Err(format!("{} is a chat of the person's, not a helper", named(r, id)));
    }
    if let Some(parent) = k.parent.as_deref().filter(|p| !persons_chat_in(r, p)) {
        let top = top_of(r, id);
        let mut root = id.to_string();
        while let Some(p) = r.known.get(&root).and_then(|k| k.parent.clone()).filter(|p| *p != top) {
            root = p;
        }
        return Err(format!(
            "{id} is a helper of {}, itself a helper under chat {}: only a chat's own helper moves, with the helpers \
             under it. Move {root} and {id} goes with it",
            named(r, parent),
            named(r, &top)
        ));
    }
    if r.auto.get(id).is_some_and(|a| a.deleting) {
        return Err(format!("{id} is being deleted (autoDelete, idle since its answer); not moved"));
    }
    let mut sessions = descendants(r, id);
    sessions.sort();
    sessions.insert(0, id.to_string());
    Ok((k.parent.clone(), sessions))
}

/// Why `to` is no chat to move `id`'s line to: the refusals in the order the
/// window shows them. `most`: the places a line may have; `above`: the person
/// lets helpers run above their caller.
fn check_target(r: &Registry, id: &str, from: Option<&str>, sessions: &[String], to: &str, most: usize, above: bool) -> Result<(), String> {
    if sessions.iter().any(|s| s == to) {
        return Err(format!("{} is {id} or a helper under it: a helper does not move into its own line", named(r, to)));
    }
    let Some(k) = r.known.get(to).filter(|_| to != DEV_CALLER) else {
        return Err(format!("chat {} is not running under Luna now: open it first", named(r, to)));
    };
    if !persons_chat_in(r, to) {
        return Err(format!("{} is a lead or a helper, not one of the person's chats: a helper moves only to a chat", named(r, to)));
    }
    if from == Some(to) {
        return Err(format!("{id} is a helper of chat {} already", named(r, to)));
    }
    if !k.tools {
        return Err(format!(
            "chat {} runs without Luna's tools, so it could not send to {id}, wait on it or answer it: turn Luna's tools \
             on for it first",
            named(r, to)
        ));
    }
    if !above {
        for s in sessions {
            let Some(h) = r.known.get(s) else { continue };
            if let Err((runs, has)) = holds_over((k.provider, &k.settings), (h.provider, &h.settings)) {
                return Err(format!(
                    "{s} runs at {runs}, above chat {} ({has}): a helper may not run with more privilege than the chat \
                     above it. Only the person can let helpers go above their caller, in Luna's settings",
                    named(r, to)
                ));
            }
        }
    }
    let (counted, coming) = under_cap(r, to, None);
    // A helper whose answer is out and that only waits out autoDelete holds no
    // place, unless the move switches that autoDelete off.
    let placed = |s: &String| !r.auto.get(s).is_some_and(|a| a.released && !a.deleting && !a.by_pass_up);
    let moving = sessions.iter().filter(|s| placed(s)).count();
    let spawning = r.spawns.values().filter(|s| s.helper.is_none() && !s.settled && sessions.contains(&s.caller)).count();
    let used = counted.len() + coming;
    if used + moving + spawning > most {
        let theirs = if counted.is_empty() { String::new() } else { format!(": {}", counted.join(", ")) };
        let theirs = if coming > 0 { format!("{theirs}, and {coming} spawn(s) under way") } else { theirs };
        let brings = if spawning > 0 { format!("{moving}, and {spawning} spawn(s) under way") } else { moving.to_string() };
        return Err(format!(
            "chat {} has {used} of its {most} places in use{theirs}, and {id} would bring {brings}: not moved. Delete \
             what that chat is done with, or raise the number of places in Luna's settings",
            named(r, to)
        ));
    }
    Ok(())
}

/// The person moves helper `id`, with every helper under it, to chat `to`. Err,
/// with nothing changed, naming why: `id` held back, not a chat's own helper or
/// one with none; `to` inside its line, no chat of the person's, without Luna's
/// tools, below one of them in privilege, or without the places they take.
pub fn move_helper(id: &str, to: &str) -> Result<HelperMoved, String> {
    // Read before the registry is held: the settings are a file of their own.
    let (most, above) = (max_descendants(), helpers_above_caller());
    let (_, first) = check_source(&reg().lock().unwrap_or_else(|e| e.into_inner()), id)?;
    let mut holds = Vec::new();
    for s in &first {
        holds.push(hand_off(s)?);
    }
    let _owners = ownership_changing();
    let claim = crate::background::claim_move(&first)?;
    // Off the registry's lock: the transcript is read, the helper's record locked.
    let (cursor, work) = (transcript_end(id), crate::background::has_work(id));
    let baseline = crate::activity::turn_state(id).map_or(0, |t| t.turns_ended);
    let (from, sessions, auto_off, news) = {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let (from, sessions) = check_source(&r, id)?;
        check_target(&r, id, from.as_deref(), &sessions, to, most, above)?;
        let (auto_off, news) = change(&mut r, id, from.as_deref(), &sessions, to);
        (from, sessions, auto_off, news)
    };
    crate::background::set_parent(id, Some(to.to_string()));
    let owed = work && to != DEV_CALLER;
    if owed {
        owe_from(to, id, baseline, cursor);
        seen(to, id, baseline);
    }
    if let Some((from, line)) = news {
        let queued = crate::background::queued_from(id, &from);
        let queued = match queued {
            0 => String::new(),
            1 => " The message you had queued for it still goes in, at its next turn if not this one.".to_string(),
            n => format!(" The {n} messages you had queued for it still go in, at its next turn if not this one."),
        };
        reg().lock().unwrap_or_else(|e| e.into_inner()).news.push((from, format!("{line}{queued}")));
    }
    drop(claim);
    drop(holds);
    crate::background::save();
    crate::background::changed();
    crate::log::warn(
        "agents",
        &format!(
            "the person moved {} with {} under it from {} to {}{}",
            label(id),
            sessions.len() - 1,
            from.as_deref().unwrap_or("no chat"),
            label(to),
            if auto_off { "; its autoDelete from its chat's going is off" } else { "" }
        ),
    );
    Ok(HelperMoved { id: id.to_string(), from, to: to.to_string(), sessions, auto_delete_off: auto_off, owed })
}

/// The one hold in which `id`'s line changes chat. Returns whether an
/// `autoDelete` `pass_up` had switched on went off, and the old chat with the
/// line it is to hear, if it had one.
fn change(r: &mut Registry, id: &str, from: Option<&str>, sessions: &[String], to: &str) -> (bool, Option<(String, String)>) {
    if let Some(k) = r.known.get_mut(id) {
        k.parent = Some(to.to_string());
        k.orphan = false;
    }
    // Asked for by its spawner it stays; switched on by its chat's going, the
    // chat it answers to now is there to delete it.
    let mut auto_off = false;
    for s in sessions {
        if r.auto.get(s).is_some_and(|a| a.by_pass_up) {
            r.auto.remove(s);
            auto_off = true;
        }
    }
    r.moved.insert(id.to_string(), Moved { from: from.map(str::to_owned), to: to.to_string(), at_ms: now_ms(), told: false });
    r.suggested.remove(id);
    for s in sessions {
        if let Some(asked) = r.suggested.get_mut(s) {
            asked.retain(|a| a.chat != to);
        }
    }
    let Some(from) = from else { return (auto_off, None) };
    // What the old chat had of them: nothing more is owed, and nothing a cancel
    // could put back (owed.rs `hand_over`).
    let theirs = |key: &(String, String)| key.0 == from && sessions.contains(&key.1);
    r.owed.retain(|k, _| !theirs(k));
    r.requests.retain(|k, _| !theirs(k));
    r.receipts.retain(|k, _| !theirs(k));
    r.seen.retain(|k, _| !theirs(k));
    r.delivery_floors.retain(|k, _| !theirs(k));
    r.answered.retain(|k, _| !theirs(k));
    r.notice_times.retain(|k, _| !theirs(k));
    r.stopped.retain(|k, _| !theirs(k));
    let name = r.names.get(id).filter(|n| !n.trim().is_empty()).map_or(String::new(), |n| format!(" \"{n}\""));
    let under = match sessions.len() - 1 {
        0 => String::new(),
        1 => " with the helper under it".to_string(),
        n => format!(" with the {n} helpers under it"),
    };
    let line = format!(
        "The person moved your helper {id}{name}{under} to chat {} in Luna's window: it answers to that chat now, no \
         longer yours to send to, answer, stop or delete, and nothing more is owed to you by it. You may still read it \
         and wait on it, with scope: \"all\".",
        named(r, to)
    );
    (auto_off, Some((from.to_string(), line)))
}

/// A chat the window may offer to move a helper to, and why not, if not.
#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MoveTarget {
    pub id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

/// Where helper `id` may move, for its menu: `blocked` says why it cannot move
/// at all; else each of the person's chats, with why not where it may not.
#[derive(Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct MoveTargets {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked: Option<String>,
    pub targets: Vec<MoveTarget>,
}

pub fn move_targets(id: &str) -> MoveTargets {
    let (most, above) = (max_descendants(), helpers_above_caller());
    let first = check_source(&reg().lock().unwrap_or_else(|e| e.into_inner()), id).map(|(_, sessions)| sessions);
    let held = first.as_ref().ok().and_then(|s| crate::background::move_held_back(s));
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let (from, sessions) = match check_source(&r, id) {
        Ok(found) => found,
        Err(why) => return MoveTargets { blocked: Some(why), targets: Vec::new() },
    };
    // Every chat of the person's: the ones running, and the rows the window
    // has named that are not (a helper's own name aside).
    let mut chats: Vec<String> = r.known.keys().filter(|c| persons_chat_in(&r, c) && *c != DEV_CALLER).cloned().collect();
    chats.extend(r.names.keys().filter(|c| !r.known.contains_key(*c)).cloned());
    chats.retain(|c| !sessions.contains(c));
    chats.sort();
    chats.dedup();
    let targets = chats
        .into_iter()
        .map(|to| {
            let why = check_target(&r, id, from.as_deref(), &sessions, &to, most, above).err();
            MoveTarget { ok: why.is_none(), why, id: to }
        })
        .collect();
    MoveTargets { blocked: held, targets }
}

#[cfg(test)]
#[path = "reparent_tests.rs"]
mod tests;
