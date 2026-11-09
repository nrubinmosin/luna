//! What is kept of each chat's handoffs, in `<data>/handoffs/handoffs.json`:
//! the number last given out, what the chat was told of its context and in
//! which conversation, the handoff under way with its phase, and those through
//! or given up. Saved at every change, under the lock it is changed under, so
//! a core started later goes on from where this one stopped (`from_bytes`).

use super::phase::{Phase, Switch};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, OnceLock};

/// How many past handoffs a chat keeps on record, snapshots with them.
pub(super) const KEPT: usize = 20;

/// The threshold notices a chat has had in one conversation: 1 the first, 2
/// the firmer one.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct Told {
    pub conversation: String,
    pub level: u8,
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
    #[serde(default)]
    pub past: Vec<Past>,
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
    static R: OnceLock<Mutex<Records>> = OnceLock::new();
    let m = R.get_or_init(|| {
        #[cfg(not(test))]
        let loaded = std::fs::read(file()).map(|b| from_bytes(&b, crate::throttle::now_ms())).unwrap_or_default();
        #[cfg(test)]
        let loaded = Records::new();
        Mutex::new(loaded)
    });
    m.lock().unwrap_or_else(|e| e.into_inner())
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

/// Writes the records out as they are, called with their lock held.
pub(super) fn save(r: &Records) {
    #[cfg(test)]
    let _ = r;
    #[cfg(not(test))]
    {
        let path = file();
        let bytes = match serde_json::to_vec_pretty(r) {
            Ok(b) => b,
            Err(e) => return crate::log::warn("handoff", &format!("could not put the handoffs into words: {e}")),
        };
        let saved = std::fs::create_dir_all(super::root()).and_then(|()| crate::secret::write_private(&path, &bytes));
        if let Err(e) = saved {
            crate::log::warn("handoff", &format!("could not save {}: {e}", path.display()));
        }
    }
}

/// Changes `chat`'s record and saves them all.
pub(super) fn change<T>(chat: &str, f: impl FnOnce(&mut Record) -> T) -> T {
    let mut r = records();
    let out = f(r.entry(chat.to_string()).or_default());
    save(&r);
    out
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
        let back = from_bytes(&serde_json::to_vec(&saved).unwrap(), 900);
        for chat in ["reset", "prompt", "again"] {
            assert_eq!(back[chat].switch.as_ref().unwrap().phase, Phase::Recovering { since: 900 }, "{chat}");
        }
        let waiting = back["waiting"].switch.as_ref().unwrap();
        assert_eq!((waiting.phase, waiting.scheduled_at, waiting.attempts), (Phase::Scheduled, 900, 1));
        assert!(from_bytes(b"not json", 1).is_empty());
    }
}
