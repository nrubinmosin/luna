//! What is kept of each chat's handoffs, in `<data>/handoffs/handoffs.json`:
//! the number last given out, what the chat was told of its context and in
//! which conversation, the instructions it was last served, the handoff under
//! way with its phase, and those through or given up. Saved at every change,
//! under the lock it is changed under, and on disk before the change counts
//! (`change`): what is typed into a chat next goes by it, and a core started
//! later goes on from there (`from_bytes`).

use super::phase::{Phase, Switch};
use super::served::Served;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, OnceLock};

/// How many past handoffs a chat keeps on record, snapshots with them.
pub(super) const KEPT: usize = 20;

/// The threshold notices a chat has had in one conversation: 1 the first, 2
/// the firmer one; and for one a handoff began, the share of its context it
/// began at (watch.rs `level`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct Told {
    pub conversation: String,
    pub level: u8,
    #[serde(default)]
    pub floor: u64,
    /// The share of its context at each notice it was sent, compactions
    /// between them or not: what the window shows while no handoff follows.
    /// Saved as `asked` by earlier cores, a name `Record::asked` (the ask open)
    /// now has.
    #[serde(default, alias = "asked", skip_serializing_if = "Vec::is_empty")]
    pub shares: Vec<u64>,
    /// How often its CLI compacted it after a notice, no handoff between
    /// (watch.rs `told_now`).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub compacted: u32,
    /// Its context in tokens at the last notice: what tells a compaction,
    /// which takes tokens away, from a larger window, which does not.
    #[serde(default, skip_serializing_if = "is_none_u64")]
    pub tokens: u64,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

fn is_none_u64(n: &u64) -> bool {
    *n == 0
}

/// Who asked a chat for a handoff: `luna_session_handoff` answers only while
/// one of them (or a move's ask, moving.rs) stands open.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(in crate::agents) enum AskKind {
    /// Auto-handoff's threshold, by its notice.
    Auto,
    /// The person, by the pane's Hand off now.
    Manual,
}

/// An ask for a handoff standing open on a chat.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(in crate::agents) struct Asked {
    pub kind: AskKind,
    /// When it was opened.
    pub at: u64,
    /// The chat's ended turns then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turns: Option<u32>,
    /// A person's ask: when Luna typed its request into the chat, from which
    /// its time runs (mod.rs `MANUAL_ASK_MS`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub typed: Option<u64>,
    /// The conversation it asks, as `phase::Before::conversation` names one:
    /// the threshold's, the one whose context passed it; the person's, the one
    /// its request went into. The ask holds only there (mod.rs `ask_holds`):
    /// the person's own `/clear` or `/new` starts a conversation nobody asked.
    /// None until that conversation is known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<String>,
}

impl Asked {
    pub fn new(kind: AskKind, at: u64, turns: Option<u32>) -> Asked {
        Asked { kind, at, turns, typed: None, conversation: None }
    }
}

/// A handoff through, or given up.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct Past {
    pub n: u32,
    /// When it ended.
    pub at: u64,
    pub provider: crate::provider::Provider,
    pub snapshot: String,
    pub sha256: String,
    /// The transcript of the conversation it left.
    pub previous: Option<String>,
    pub done: bool,
    /// Why it was given up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// The instructions the chat was served to write it by; none when it
    /// went by the notices alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<Served>,
    /// The document a move to another account opened this chat with
    /// (moving.rs), not a handoff of its own.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub moved: bool,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct Record {
    /// The last number given out: the next handoff is one more.
    #[serde(default)]
    pub last: u32,
    #[serde(default)]
    pub told: Option<Told>,
    #[serde(default)]
    pub switch: Option<Switch>,
    /// A handoff went through, and the conversation it began is yet to be
    /// looked at: its first reading is its floor.
    #[serde(default)]
    pub fresh: bool,
    #[serde(default)]
    pub past: Vec<Past>,
    /// The instructions `luna_session_handoff` last served the chat, for the
    /// handoff it writes by them; let go once one goes through.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub served: Option<Served>,
    /// The number a call is copying its file under, its I/O done outside
    /// the lock: one at a time per chat. Never saved: a core that stops in
    /// the middle leaves no copy anyone schedules from.
    #[serde(skip)]
    pub reserved: Option<u32>,
    /// This chat's move to another account, under way or the last one ended
    /// (moving.rs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moving: Option<super::moving::Move>,
    /// The move that opened this chat, for a chat that is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moved_from: Option<super::moving::MovedFrom>,
    /// The Claude Code worktree a move put this chat in with others: its
    /// project and name (moving.rs `worktree_key`). No two chats with the
    /// same one run at once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_key: Option<String>,
    /// When the last ask for a move's document ended unanswered (given up,
    /// canceled), and which: a file the chat sends after it is that ask's
    /// late answer, never a handoff (moving.rs `asked_file`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask_closed: Option<(String, u64)>,
    /// The chat's ended turns when that ask ended: its next turn, the
    /// person's or a handoff's, ends the window (moving.rs `late_answer`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask_closed_turns: Option<u32>,
    /// The ask for a handoff open on this chat, the threshold's or the
    /// person's: closed as a handoff goes through, or as it lapses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked: Option<Asked>,
    /// The failure lines queued with a handoff's continuation in them, each
    /// with the conversation it was made for: the notifier types one only
    /// while the chat is still on that conversation (mod.rs `as_told`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub continued: Vec<Continued>,
}

/// A failure line carrying the continuation (facts.rs `failed_line`), by its
/// hash: the conversation Luna's reset opened, the only one it may go into,
/// and the short line that takes its place anywhere else.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct Continued {
    pub line_sha256: String,
    pub conversation: Option<String>,
    pub short: String,
}

pub(super) type Records = BTreeMap<String, Record>;

#[cfg(not(test))]
fn file() -> std::path::PathBuf {
    super::root().join("handoffs.json")
}

/// Every chat's record, behind one lock; read from disk at first use. A
/// test's run starts empty and writes nothing: tests share the data dir, and a
/// record a test left would stand in another's way.
pub(super) fn records() -> MutexGuard<'static, Records> {
    static_records().lock().unwrap_or_else(|e| e.into_inner())
}

fn static_records() -> &'static Mutex<Records> {
    static R: OnceLock<Mutex<Records>> = OnceLock::new();
    R.get_or_init(|| {
        // Read only from a folder that is Luna's (`private_chain`): one a link
        // leads elsewhere holds no record of Luna's.
        #[cfg(not(test))]
        let loaded = match super::private_chain(&super::root(), &[]) {
            Ok(_) => load(&file(), crate::throttle::now_ms()),
            Err(e) => {
                crate::log::warn("handoff", &format!("handoffs are not read: {e}"));
                Records::new()
            }
        };
        #[cfg(test)]
        let loaded = Records::new();
        Mutex::new(loaded)
    })
}

/// The records if nobody holds them now: for the test that the file is read
/// without them.
#[cfg(test)]
pub(super) fn try_records() -> Option<MutexGuard<'static, Records>> {
    static_records().try_lock().ok()
}

/// What a core saved, as this one takes it up: a switch it was in the middle
/// of is looked at again before anything is typed (`Phase::Recovering`), and
/// one that was only scheduled waits its full time again from now.
pub(super) fn from_bytes(bytes: &[u8], now: u64) -> Records {
    let mut r: Records = serde_json::from_slice(bytes).unwrap_or_default();
    for rec in r.values_mut() {
        if let Some(s) = rec.switch.as_mut() {
            match s.phase {
                Phase::Resetting { .. } | Phase::Prompting { .. } | Phase::Recovering { .. } => {
                    s.phase = Phase::Recovering { since: now };
                }
                Phase::Scheduled => s.scheduled_at = now,
            }
        }
        if let Some(m) = rec.moving.as_mut() {
            super::moving::taken_up(m, now);
        }
        rearmed_unasked(rec);
    }
    r
}

/// A conversation told of its threshold with no ask open and no handoff
/// under way is told nothing yet, so the threshold asks it again at its next
/// look if it is still past it: once the person's ask that took the
/// threshold's place closes with no handoff (mod.rs `lapsed`), a move ends
/// with the chat not stopped (moving.rs `end_where`), or auto-handoff is turned on again
/// (mod.rs `set_auto_handoff`, which looks as soon as the chat is at rest),
/// the threshold not noticing again at a level
/// it told of. A record from before asks were kept
/// (the notice itself let the chat call) has none, and would otherwise be
/// refused and never noticed again at that level. Not migrated into an open
/// ask: such a record cannot be told from one whose ask closed since (the
/// person's lapsed in its place), and an ask the chat would answer now must
/// have been typed into the conversation it binds (`Asked::conversation`),
/// which the notice, gone long ago, cannot vouch for. A notice more after a
/// restart is the smaller harm: it goes only to a chat past its threshold
/// that nothing asks.
pub(super) fn rearmed_unasked(rec: &mut Record) {
    if rec.asked.is_some() || rec.switch.is_some() {
        return;
    }
    if let Some(t) = rec.told.as_mut().filter(|t| t.level > 0) {
        t.level = 0;
    }
}

/// The most the record may hold as it is read back: KEPT handoffs a chat,
/// for many chats, is far less.
const MAX_BYTES: u64 = 16 * 1024 * 1024;

/// What a core saved at `path`, as this one takes it up (`from_bytes`): read
/// only as a private file of this user's, opened as itself
/// (`secret::read_private`). A link there, or a file others could have
/// written, is no record of Luna's: none is taken up, and the log says why;
/// the next save puts Luna's own in its place (`write`).
fn load(path: &std::path::Path, now: u64) -> Records {
    match crate::secret::read_private(path, MAX_BYTES) {
        Ok(Some(bytes)) => from_bytes(&bytes, now),
        Ok(None) => Records::new(),
        Err(e) => {
            crate::log::warn("handoff", &format!("handoffs are not read: {e}"));
            Records::new()
        }
    }
}

/// The records written to `path`, on disk (flushed, the folder too where it
/// can be) before it returns: a new private file renamed into place
/// (`secret::write_private_synced`), which takes the place of a link at the
/// name, never writing through it.
fn write(path: &std::path::Path, r: &Records) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(r).map_err(|e| format!("could not put the handoffs into words: {e}"))?;
    crate::secret::write_private_synced(path, &bytes).map_err(|e| format!("could not save {}: {e}", path.display()))
}

/// Writes the records out as they are, called with their lock held, on disk
/// before it returns.
#[cfg(not(test))]
fn save(r: &Records) -> Result<(), String> {
    // Its folder checked first (`private_chain`): a record written through a
    // link would be another user's to forge.
    super::private_chain(&super::root(), &[])?;
    write(&file(), r)
}

/// A test's records stay in memory; a chat a test names here fails to save,
/// as a full disk would.
#[cfg(test)]
fn save_for(_: &Records, chat: &str) -> Result<(), String> {
    if failing().lock().unwrap_or_else(|e| e.into_inner()).contains(chat) {
        return Err("could not save handoffs.json: the disk is full".into());
    }
    Ok(())
}

#[cfg(not(test))]
fn save_for(r: &Records, _: &str) -> Result<(), String> {
    save(r)
}

#[cfg(test)]
pub(super) fn failing() -> &'static Mutex<std::collections::HashSet<String>> {
    static F: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    F.get_or_init(Default::default)
}

/// Changes `chat`'s record and saves them all. A save that fails puts the
/// record back as it was and says so: nothing goes by a change that is not on
/// disk.
pub(super) fn change<T>(chat: &str, f: impl FnOnce(&mut Record) -> T) -> Result<T, String> {
    let mut r = records();
    let before = r.get(chat).cloned();
    let out = f(r.entry(chat.to_string()).or_default());
    if let Err(e) = save_for(&r, chat) {
        match before {
            Some(b) => {
                r.insert(chat.to_string(), b);
            }
            None => {
                r.remove(chat);
            }
        }
        return Err(e);
    }
    Ok(out)
}

/// `chat`'s record gone, saved; what is in memory goes whatever the disk says.
pub(super) fn remove(chat: &str) -> Result<(), String> {
    let mut r = records();
    if r.remove(chat).is_none() {
        return Ok(());
    }
    save_for(&r, chat)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::handoff::phase::Before;
    use crate::provider::Provider;

    #[test]
    fn a_core_taking_up_a_switch_looks_before_it_types() {
        let s = |phase| Switch {
            n: 1,
            provider: Provider::Claude,
            snapshot: "s".into(),
            sha256: "h".into(),
            source: "f".into(),
            scheduled_at: 5,
            phase,
            before: Before::default(),
            attempts: 1,
            instructions: None,
            opened: Some("new".into()),
        };
        let mut saved = Records::new();
        for (chat, phase) in [
            ("reset", Phase::Resetting { at: 7 }),
            ("prompt", Phase::Prompting { at: 7 }),
            ("again", Phase::Recovering { since: 7 }),
            ("waiting", Phase::Scheduled),
        ] {
            saved.insert(chat.into(), Record { last: 1, switch: Some(s(phase)), ..Default::default() });
        }
        saved.get_mut("reset").unwrap().reserved = Some(2);
        // A move's preview nobody confirmed before the core went.
        let shown = super::super::moving::tests::a_move(super::super::moving::MovePhase::Shown { at: 3 });
        saved.insert("moving".into(), Record { moving: Some(shown), ..Default::default() });
        let back = from_bytes(&serde_json::to_vec(&saved).unwrap(), 900);
        assert_eq!(back["reset"].reserved, None, "a number being copied under is never saved");
        for chat in ["reset", "prompt", "again"] {
            assert_eq!(back[chat].switch.as_ref().unwrap().phase, Phase::Recovering { since: 900 }, "{chat}");
            assert_eq!(back[chat].switch.as_ref().unwrap().opened.as_deref(), Some("new"), "where the reset went, kept");
        }
        let waiting = back["waiting"].switch.as_ref().unwrap();
        assert_eq!((waiting.phase, waiting.scheduled_at, waiting.attempts), (Phase::Scheduled, 900, 1));
        let gave_up = back["moving"].moving.as_ref().unwrap();
        assert!(matches!(&gave_up.phase, super::super::moving::MovePhase::Failed { at: 900, .. }), "never sent unconfirmed");
        assert!(from_bytes(b"not json", 1).is_empty());
    }

    /// A conversation a core told of its threshold, with no ask kept for it
    /// (a record from before asks were kept), is told nothing yet, so the
    /// threshold asks it again; one with an ask or a handoff under way keeps
    /// what it was told.
    #[test]
    fn a_conversation_told_with_no_ask_kept_is_asked_again() {
        let told = |level| Some(Told { conversation: "c".into(), level, floor: 3, shares: vec![71], compacted: 0, tokens: 9 });
        let mut saved = Records::new();
        saved.insert("old".into(), Record { told: told(2), ..Default::default() });
        let open = Asked::new(AskKind::Auto, 1, None);
        saved.insert("asked".into(), Record { told: told(1), asked: Some(open), ..Default::default() });
        // As a record from before asks were kept is written.
        let mut bytes = serde_json::to_value(&saved).unwrap();
        bytes["old"].as_object_mut().unwrap().remove("asked");
        let back = from_bytes(&serde_json::to_vec(&bytes).unwrap(), 9);
        let old = back["old"].told.as_ref().unwrap();
        assert_eq!((old.level, old.floor, old.shares.clone()), (0, 3, vec![71]), "asked again, the rest kept");
        assert!(super::super::watch::due(old, 1), "the threshold's next look notices it");
        assert_eq!(back["asked"].told, told(1), "an ask open: the notice stands");
        assert_eq!(back["asked"].asked.as_ref().map(|a| a.kind), Some(AskKind::Auto));
    }

    /// What a conversation was asked at, saved under its old name, reads back.
    #[test]
    fn told_shares_saved_as_asked_read_back() {
        let old = r#"{"c":{"told":{"conversation":"c","level":1,"asked":[71,86]}}}"#;
        let back = from_bytes(old.as_bytes(), 9);
        assert_eq!(back["c"].told.as_ref().map(|t| t.shares.clone()), Some(vec![71, 86]));
        assert!(serde_json::to_string(&back).unwrap().contains(r#""shares":[71,86]"#));
    }

    /// `handoffs.json` planted as a link, to a folder (a junction on Windows)
    /// or a file someone else holds, is not read and not written through: a
    /// save takes the name back for Luna's own file, or fails, and what the
    /// link led to is left as it was. A file others may write is not read.
    #[test]
    fn a_store_planted_as_a_link_is_neither_read_nor_written_through() {
        let dir = std::env::temp_dir().join(format!("luna-handoff-store-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let forged = serde_json::to_vec(&Records::from([("c1".to_string(), Record { last: 7, ..Default::default() })])).unwrap();
        let mine = Records::from([("c2".to_string(), Record { last: 1, ..Default::default() })]);
        // A folder link at the name.
        let theirs = dir.join("theirs");
        std::fs::create_dir_all(&theirs).unwrap();
        std::fs::write(theirs.join("handoffs.json"), &forged).unwrap();
        let at = dir.join("handoffs.json");
        assert!(super::super::file::tests::dir_link(&theirs, &at));
        assert!(load(&at, 1).is_empty(), "not read");
        let _ = write(&at, &mine);
        assert_eq!(std::fs::read(theirs.join("handoffs.json")).unwrap(), forged, "nothing written through it");
        assert_eq!(std::fs::read_dir(&theirs).unwrap().count(), 1);
        // A file link at the name, where this machine lets a test make one.
        let file_at = dir.join("store.json");
        let target = dir.join("theirs.json");
        std::fs::write(&target, &forged).unwrap();
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(&target, &file_at).is_ok();
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&target, &file_at).is_ok();
        if made {
            assert!(load(&file_at, 1).is_empty(), "not read");
            write(&file_at, &mine).unwrap();
            assert_eq!(std::fs::read(&target).unwrap(), forged, "what it led to is as it was");
            assert!(!std::fs::symlink_metadata(&file_at).unwrap().file_type().is_symlink(), "the name is Luna's file now");
            assert_eq!(load(&file_at, 1)["c2"].last, 1);
        }
        // A plain file others may write: not read.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let open = dir.join("open.json");
            std::fs::write(&open, &forged).unwrap();
            std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(load(&open, 1).is_empty());
        }
        // Luna's own, written and read back.
        let own = dir.join("own.json");
        write(&own, &mine).unwrap();
        assert_eq!(load(&own, 1)["c2"].last, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_change_that_cannot_be_saved_is_undone_and_said() {
        change("sv-ok", |r| r.last = 4).unwrap();
        failing().lock().unwrap().insert("sv-ok".into());
        let e = change("sv-ok", |r| r.last = 5).unwrap_err();
        assert!(e.contains("disk is full"), "{e}");
        assert_eq!(records()["sv-ok"].last, 4, "put back as it was");
        failing().lock().unwrap().insert("sv-new".into());
        assert!(change("sv-new", |r| r.last = 1).is_err());
        assert!(!records().contains_key("sv-new"), "a record that never saved is not there");
        failing().lock().unwrap().retain(|c| !c.starts_with("sv-"));
        remove("sv-ok").unwrap();
    }
}
