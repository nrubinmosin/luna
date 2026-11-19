//! The person moves a helper, with the helpers under it, to another chat
//! (ADR-0028): from the "Move to chat…" link on its row in the window's helper
//! list, or from the "Move to B?" its row offers once chat B was refused a
//! write into it or a helper under it (`asked_to_move`). No agent may: it is a
//! window command, not a tool.
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
//! alone (`ownership_changing`), so a send, an answer or a spawn checked
//! against the old chat has left its debt before, or is checked against the
//! new one after; the helpers held between turns (`claim_move`), so the next
//! turn starts under its new parent and none is deleted meanwhile; and the
//! checks, the change and the new chat's debt in one hold of the registry,
//! where a spawn counts its places and a chat's going is decided. The line is
//! looked at again in that hold: one that has changed since the holds were
//! taken (a helper spawned under it, one gone) is held again as it is now.

use crate::agents::notifier::hand_off;
use crate::agents::owed::owe_in;
use crate::agents::read::transcript_end;
use crate::agents::registry::{
    depth_of, descendants, is_descendant, label, named, ownership_changing, persons_chat_in, reg, top_of, Registry, DEV_CALLER,
};
use crate::agents::spawn::{max_descendants, spawnable_at, takes_place, under_cap, AtWork};
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
    /// A turn's model has answered a prompt that told it of this move
    /// (`move_note`, `move_told`).
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
/// above it: the window may offer to move it there, which is to move the
/// helper its line moves with (`movable_root`), `id` itself or the one above
/// it under the chat. Only for a person's chat; a helper refused has no such
/// way. Whether it is new, for the rows to be told.
pub(super) fn asked_to_move(r: &mut Registry, chat: &str, id: &str) -> bool {
    if chat == DEV_CALLER || !persons_chat_in(r, chat) || !r.known.contains_key(id) {
        return false;
    }
    let root = movable_root(r, id);
    let now = now_ms();
    let asked = r.suggested.entry(root).or_default();
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

/// The helper the person moves for `id` to move: `id` if it is a chat's own
/// helper or has no chat above it, else the helper above it that is, which
/// takes it along (`check_source`).
pub(super) fn movable_root(r: &Registry, id: &str) -> String {
    let mut root = id.to_string();
    for _ in 0..16 {
        match r.known.get(&root).and_then(|k| k.parent.clone()) {
            Some(p) if !persons_chat_in(r, &p) => root = p,
            _ => break,
        }
    }
    root
}

/// The helper moved away from `caller` that `target` is, or is under, if the
/// person moved one: `caller` was its chat, is not above it now, and the chat
/// it was moved to still is. One whose new chat has gone since hangs from no
/// chat, or from whichever the person moved it to after; its former chat is
/// told that, not of the move.
pub(super) fn moved_away(r: &Registry, caller: &str, target: &str) -> Option<String> {
    if is_descendant(r, caller, target) {
        return None;
    }
    let mut cur = target.to_string();
    for _ in 0..16 {
        if r.moved.get(&cur).is_some_and(|m| m.from.as_deref() == Some(caller) && is_descendant(r, &m.to, &cur)) {
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

/// What a moved helper's next turn is told of its move (`move_note`).
pub struct MoveNote {
    /// The line its prompt is given (background/command.rs `stdin_with`).
    pub text: String,
    /// The move it tells of, as it stood: what `move_told` marks, if it is
    /// still the last.
    pub of: Moved,
}

/// The note a moved helper's next turn is given, until a turn's model has
/// answered a prompt that carried it (`move_told`). Whom it answers to is its
/// parent now, not the move's chat: that chat may have gone since (`forget`),
/// leaving it with the one above that chat or with none.
pub fn move_note(id: &str) -> Option<MoveNote> {
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let m = r.moved.get(id).filter(|m| !m.told)?;
    let parent = r.known.get(id)?.parent.as_deref();
    let from = match &m.from {
        Some(from) => format!("from chat {}", named(&r, from)),
        None => "from no chat (the one above you had gone)".to_string(),
    };
    let to = named(&r, &m.to);
    let now = match parent {
        Some(p) if p == m.to => format!(
            "to chat {to}: you answer to that chat now. Your answers, reports and questions go to it, and only it and \
             the sessions above you send to you."
        ),
        Some(p) => format!(
            "to chat {to}, which has gone since: you answer to {} now. Your answers, reports and questions go to it, \
             and only it and the sessions above you send to you.",
            named(&r, p)
        ),
        None => format!(
            "to chat {to}, which has gone since: no chat is above you now, so no agent sends to you or hears your \
             answers. The person sees your work in Luna's window."
        ),
    };
    Some(MoveNote { text: format!("The person moved you, in Luna's window, {from} {now}"), of: m.clone() })
}

/// A turn's model has answered a prompt that told it of move `of`: no turn
/// after is told it again. A move made since, which that prompt did not tell
/// of, stays to be told.
pub fn move_told(id: &str, of: &Moved) {
    {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        match r.moved.get_mut(id) {
            Some(m) if m == of => m.told = true,
            _ => return,
        }
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
    // A line whose top is a helper (its chat deleted, the helper above it
    // left alone) hangs from no chat. A top not running under Luna is no
    // helper either: every helper is a session it knows.
    let top = k.parent.as_ref().map(|_| top_of(&r, id)).filter(|t| persons_chat_in(&r, t) || !r.known.contains_key(t));
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
        let under = if persons_chat_in(r, &top) { format!("under chat {}", named(r, &top)) } else { "with no chat above it".into() };
        return Err(format!(
            "{id} is a helper of {}, itself a helper {under}: only a chat's own helper moves, with the helpers under it. \
             Move {} and {id} goes with it",
            named(r, parent),
            movable_root(r, id)
        ));
    }
    let mut sessions = descendants(r, id);
    sessions.sort();
    sessions.insert(0, id.to_string());
    // Taken for deletion by autoDelete: going, and not to be moved under it.
    if let Some(s) = sessions.iter().find(|s| r.auto.get(*s).is_some_and(|a| a.deleting)) {
        return Err(format!("{s} is being deleted (autoDelete, idle since its answer); not moved"));
    }
    Ok((k.parent.clone(), sessions))
}

/// Why `to` is no chat to move `id`'s line to: the refusals in the order the
/// window shows them. `most`: the places a line may have; `work`: which
/// sessions are at work, read before the registry was held. The modes the CLIs
/// run in are not compared: what runs in a helper is what its spawn asked for.
fn check_target(
    r: &Registry,
    work: &AtWork,
    id: &str,
    from: Option<&str>,
    sessions: &[String],
    to: &str,
    most: usize,
) -> Result<(), String> {
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
    // No deeper than a spawn could make it under that chat: a line that grew
    // with no chat above it (a lead left alone, spawning leads) does not fit.
    // The one nearest the top is named: what is under it is why the rest are.
    let base = depth_of(r, id);
    let too_deep = sessions
        .iter()
        .filter_map(|s| {
            let tools = r.known.get(s)?.tools;
            let depth = depth_of(r, s) - base + 1;
            (!spawnable_at(depth, tools)).then_some((s, depth, tools))
        })
        .min_by_key(|(_, depth, _)| *depth);
    if let Some((s, depth, tools)) = too_deep {
        let what = if tools { "a helper with Luna's tools" } else { "a helper" };
        return Err(format!(
            "{s}, {what}, would be {depth} levels below chat {}, deeper than a spawn could put it there (a helper's \
             helper has no tools of its own, and nothing is below it): not moved",
            named(r, to)
        ));
    }

    let (counted, coming) = under_cap(r, work, to, None);
    // Only those at work take places there, as on any line: one between turns
    // takes none, whatever the move does to its autoDelete.
    let moving = sessions.iter().filter(|s| takes_place(r, work, s)).count();
    let spawning = r.spawns.values().filter(|s| s.helper.is_none() && !s.settled && sessions.contains(&s.caller)).count();
    let used = counted.len() + coming;
    if used + moving + spawning > most {
        let theirs = if counted.is_empty() { String::new() } else { format!(": {}", counted.join(", ")) };
        let theirs = if coming > 0 { format!("{theirs}, and {coming} spawn(s) under way") } else { theirs };
        let brings = if spawning > 0 { format!("{moving}, and {spawning} spawn(s) under way") } else { moving.to_string() };
        return Err(format!(
            "chat {} has {used} of its {most} places in use{theirs}, and {id} would bring {brings} at work: not moved. \
             Wait for turns there to end, stop what that chat no longer needs, or raise the number of places in Luna's \
             settings",
            named(r, to)
        ));
    }
    Ok(())
}

/// How many times a move takes its holds again on a line that changed under
/// them (`move_line`) before it gives up.
const LINE_TRIES: usize = 3;

/// The person moves helper `id`, with every helper under it, to chat `to`. Err,
/// with nothing changed, naming why: `id` held back, not a chat's own helper or
/// one with none; `to` inside its line, no chat of the person's, without Luna's
/// tools, or without the places they take.
pub fn move_helper(id: &str, to: &str) -> Result<HelperMoved, String> {
    // Read before the registry is held: the settings are a file of their own.
    let most = max_descendants();
    for _ in 0..LINE_TRIES {
        let line = check_source(&reg().lock().unwrap_or_else(|e| e.into_inner()), id)?;
        let Some(moved) = move_line(id, to, &line, most)? else { continue };
        crate::background::save();
        crate::background::changed();
        crate::log::warn(
            "agents",
            &format!(
                "the person moved {} with {} under it from {} to {}{}",
                label(id),
                moved.sessions.len() - 1,
                moved.from.as_deref().unwrap_or("no chat"),
                label(to),
                if moved.auto_delete_off { "; its autoDelete from its chat's going is off" } else { "" }
            ),
        );
        return Ok(moved);
    }
    Err(format!("the helpers under {id} kept changing as it was being moved (spawned or deleted meanwhile); not moved. Try again"))
}

/// One try at the move, holding `line` (its chat and its sessions, `id` first)
/// as it was looked at before the holds. Ok(None), with nothing changed: the
/// line is another by the time the registry is held, and the holds are not on
/// all of it (a helper spawned under it meanwhile, which a delete may hold).
fn move_line(id: &str, to: &str, line: &(Option<String>, Vec<String>), most: usize) -> Result<Option<HelperMoved>, String> {
    let (from_then, first) = line;
    let mut holds = Vec::new();
    for s in first {
        holds.push(hand_off(s)?);
    }
    // Read before the writes into helpers wait on the move: the transcript is a
    // file. Taken early, what the new chat is owed may begin a little before the
    // move, never after what the helper is at work on.
    let cursor = transcript_end(id);
    #[cfg(test)]
    super::test_support::reached("move looked", id);
    let owners = ownership_changing();
    let mut claim = match crate::background::claim_move(first) {
        Ok(claim) => claim,
        Err(why) => {
            drop(owners);
            return Err(why);
        }
    };
    // In one look, with no turn able to start: whether it is at work and the
    // turns it has ended, which the new chat's debt counts from.
    let (work, baseline) = crate::background::work_and_count(id).unwrap_or_default();
    // What the old chat still has queued for it, which it hears of; its sends
    // wait on `owners` meanwhile.
    let queued = from_then.as_deref().map_or(0, |from| crate::background::queued_from(id, from));
    // Which sessions take places, the moved ones read with none of their turns
    // able to start.
    let at_work = AtWork::now();
    #[cfg(test)]
    super::test_support::reached("move held", id);
    let changed = (|| {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let (from, sessions) = check_source(&r, id)?;
        if sessions != *first {
            return Ok(None);
        }
        check_target(&r, &at_work, id, from.as_deref(), &sessions, to, most)?;
        let auto_off = change(&mut r, id, from.as_deref(), &sessions, to, if from == *from_then { queued } else { 0 });
        // The answer it is at work on is the new chat's, owed in this hold: the
        // chat's going (`forget`) is decided under it too, and takes the debt
        // with it rather than leave it owed to nobody.
        let owed = work && to != DEV_CALLER;
        if owed {
            owe_in(&mut r, to, id, baseline, cursor, None, None);
            r.seen.insert((to.to_string(), id.to_string()), baseline);
            if let Some(a) = r.auto.get_mut(id) {
                a.idle_since = None;
            }
        }
        let moved = HelperMoved { id: id.to_string(), from, to: to.to_string(), sessions, auto_delete_off: auto_off, owed };
        Ok(Some((moved, r.parent_changes)))
    })();
    #[cfg(test)]
    super::test_support::reached("move changed", id);
    // Whose they are is settled: the writes waiting on it go on, and the
    // claim's release below, which may start a turn (`go_on`), comes after.
    drop(owners);
    if let Ok(Some((_, change))) = &changed {
        // The parent its next turn is started with, before the claim lets one
        // start. A chat's going (`forget`) passes it on in the registry and
        // writes it here too, after its own hold of the registry: whichever of
        // the two writes last, the one that changed the registry last stands.
        crate::background::set_parent(id, Some(to.to_string()), *change);
        claim.moved();
    }
    drop(claim);
    drop(holds);
    changed.map(|done| done.map(|(moved, _)| moved))
}

/// The one hold in which `id`'s line changes chat; `queued`: how many messages
/// the old chat has queued for it. Returns whether an `autoDelete` `pass_up` had
/// switched on went off.
fn change(r: &mut Registry, id: &str, from: Option<&str>, sessions: &[String], to: &str, queued: usize) -> bool {
    r.parent_changes += 1;
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
    let Some(from) = from else { return auto_off };
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
    let queued = match queued {
        0 => String::new(),
        1 => " The message you had queued for it still goes in, at its next turn if not this one.".to_string(),
        n => format!(" The {n} messages you had queued for it still go in, at its next turn if not this one."),
    };
    let line = format!(
        "The person moved your helper {id}{name}{under} to chat {} in Luna's window: it answers to that chat now, no \
         longer yours to send to, answer, stop or delete, and nothing more is owed to you by it. You may still read it \
         and wait on it, with scope: \"all\".{queued}",
        named(r, to)
    );
    r.news.push((from.to_string(), line));
    auto_off
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

/// Where helper `id` may move, for the window's Move dialog: `blocked` says
/// why it cannot move at all; else each of the person's chats, with why not
/// where it may not.
#[derive(Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct MoveTargets {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked: Option<String>,
    pub targets: Vec<MoveTarget>,
}

pub fn move_targets(id: &str) -> MoveTargets {
    let most = max_descendants();
    let first = check_source(&reg().lock().unwrap_or_else(|e| e.into_inner()), id).map(|(_, sessions)| sessions);
    let held = first.as_ref().ok().and_then(|s| crate::background::move_held_back(s));
    let work = AtWork::now();
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
            let why = check_target(&r, &work, id, from.as_deref(), &sessions, &to, most).err();
            MoveTarget { ok: why.is_none(), why, id: to }
        })
        .collect();
    MoveTargets { blocked: held, targets }
}

#[cfg(test)]
#[path = "reparent_tests.rs"]
mod tests;
