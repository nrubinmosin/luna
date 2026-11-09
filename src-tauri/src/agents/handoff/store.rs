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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub asked: Vec<u64>,
    /// How often its CLI compacted it after a notice, no handoff between
    /// (watch.rs `told_now`).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub compacted: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
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
        #[cfg(not(test))]
        let loaded = std::fs::read(file()).map(|b| from_bytes(&b, crate::throttle::now_ms())).unwrap_or_default();
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
    }
    r
}

/// Writes the records out as they are, called with their lock held, on disk
/// (flushed, the folder too where it can be) before it returns.
#[cfg(not(test))]
fn save(r: &Records) -> Result<(), String> {
    let path = file();
    let bytes = serde_json::to_vec_pretty(r).map_err(|e| format!("could not put the handoffs into words: {e}"))?;
    std::fs::create_dir_all(super::root())
        .and_then(|()| crate::secret::write_private_synced(&path, &bytes))
        .map_err(|e| format!("could not save {}: {e}", path.display()))
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
        let back = from_bytes(&serde_json::to_vec(&saved).unwrap(), 900);
        assert_eq!(back["reset"].reserved, None, "a number being copied under is never saved");
        for chat in ["reset", "prompt", "again"] {
            assert_eq!(back[chat].switch.as_ref().unwrap().phase, Phase::Recovering { since: 900 }, "{chat}");
        }
        let waiting = back["waiting"].switch.as_ref().unwrap();
        assert_eq!((waiting.phase, waiting.scheduled_at, waiting.attempts), (Phase::Scheduled, 900, 1));
        assert!(from_bytes(b"not json", 1).is_empty());
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
