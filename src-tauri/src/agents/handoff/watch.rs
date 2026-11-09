//! The threshold: at each turn's end of a chat with auto-handoff on, how full
//! its context is (`Context::of`, as its pane's meter reads it), and once past
//! the threshold one notice, through the notifier, asking for a handoff; a
//! second, firmer one near where the CLI would compact on its own. Each once
//! per conversation: a new one (the handoff's, or a person's own `/clear`)
//! starts over. A conversation a handoff began is asked only once it has grown
//! past where it began: one that starts above the threshold (a small window, a
//! long handoff) would otherwise be asked to hand off again at its first turn's
//! end, and the next one too. Nothing is switched from here.
//!
//! A conversation whose context falls well under the threshold after a notice
//! (`rearm_below`) is asked again from the start, a handoff not having come:
//! its CLI compacted it, or it went on in a larger window (a model with more
//! context). Its tokens tell which: a compaction takes them away, a larger
//! window does not, and only a compaction is counted for the window. Both CLIs
//! keep the conversation's id through their own compaction
//! (docs/cli-compatibility.md); one that did not would read as a new
//! conversation, which starts over all the same.

use super::store::Told;

/// How many points a conversation a handoff began grows before it is asked.
pub(super) const GROWTH: u64 = 5;

/// How far under its threshold a conversation told of it falls before it is
/// asked again: this many points, or half the threshold where that is less.
/// Only a compaction or a larger window takes a conversation's share back
/// down, and either takes it far: a reading a point or two under is neither.
pub(super) const REARM: u64 = 15;

/// Where the firmer notice goes: well past the threshold, and short of where
/// the CLIs compact by themselves.
pub(super) fn firm_at(threshold: u8) -> u8 {
    threshold.saturating_add(10).clamp(85, 95)
}

/// The share of its context under which a conversation told of its threshold
/// is asked again from the start.
pub(super) fn rearm_below(threshold: u8) -> u64 {
    let t = u64::from(threshold);
    t - (t / 2).min(REARM)
}

/// How far `pct` has come: 0 short of the threshold, or within GROWTH of the
/// `floor` a handoff's conversation began at; 1 past it; 2 past the firm mark.
pub(super) fn level(pct: u64, threshold: u8, floor: u64) -> u8 {
    if floor > 0 && pct < floor + GROWTH {
        0
    } else if pct >= u64::from(firm_at(threshold)) {
        2
    } else if pct >= u64::from(threshold) {
        1
    } else {
        0
    }
}

/// What `conversation`, at `pct` of its context and `tokens` now, has been
/// told: what was kept for it, or for one not looked at before nothing yet,
/// and its floor where a handoff began it (`fresh`). One told of `threshold`
/// and now well under it is told nothing from here on, its floor gone, what it
/// was asked kept for the window; counted as compacted where it has fewer
/// tokens than at its last notice (`compacted`), not where only its window
/// grew, nor where that count is not known (a record from before it was kept).
pub(super) fn told_now(told: Option<&Told>, conversation: &str, pct: u64, tokens: u64, fresh: bool, threshold: u8) -> Told {
    match told.filter(|t| t.conversation == conversation) {
        Some(t) if t.level > 0 && pct < rearm_below(threshold) => {
            let compacted = t.compacted.saturating_add(u32::from(compacted(t, tokens)));
            Told { level: 0, floor: 0, compacted, ..t.clone() }
        }
        Some(t) => t.clone(),
        None => Told {
            conversation: conversation.to_string(),
            level: 0,
            floor: if fresh { pct.max(1) } else { 0 },
            asked: Vec::new(),
            compacted: 0,
            tokens: 0,
        },
    }
}

/// Whether a conversation told at `told.tokens`, at `tokens` now, lost context
/// to a compaction: fewer tokens than at its last notice, by a tenth at least
/// (a reading's own wobble is less).
pub(super) fn compacted(told: &Told, tokens: u64) -> bool {
    told.tokens > 0 && tokens < told.tokens - told.tokens / 10
}

/// Whether a chat at `level` is yet to hear of it: a level higher than its
/// conversation was told of. One jump past both is one notice, the firm one.
pub(super) fn due(told: &Told, level: u8) -> bool {
    level > told.level
}

#[cfg(test)]
mod tests {
    use super::*;

    fn told(conversation: &str, level: u8) -> Told {
        Told { conversation: conversation.into(), level, floor: 0, asked: Vec::new(), compacted: 0, tokens: 0 }
    }

    #[test]
    fn a_chat_hears_once_at_its_threshold_and_once_near_the_limit_per_conversation() {
        assert_eq!((firm_at(70), firm_at(30), firm_at(90), firm_at(1)), (85, 85, 95, 85));
        assert_eq!((level(69, 70, 0), level(70, 70, 0), level(84, 70, 0), level(85, 70, 0), level(100, 70, 0)), (0, 1, 1, 2, 2));
        let now = |kept: Option<&Told>, conversation| told_now(kept, conversation, 72, 144_000, false, 70);
        assert!(!due(&now(None, "a"), 0));
        assert!(due(&now(None, "a"), 1));
        assert!(!due(&now(Some(&told("a", 1)), "a"), 1), "once");
        assert!(due(&now(Some(&told("a", 1)), "a"), 2), "then the firm one");
        assert!(!due(&now(Some(&told("a", 2)), "a"), 2));
        assert!(!due(&now(Some(&told("a", 2)), "a"), 1), "a reading a little under says nothing");
        assert!(due(&now(Some(&told("a", 2)), "b"), 1), "a new conversation starts over");
        assert!(due(&now(None, "a"), 2), "a jump past both: the firm one alone");
    }

    #[test]
    fn a_conversation_a_handoff_began_is_asked_only_once_it_has_grown() {
        // Begun at 12% under a threshold of 10: not asked at its first turn's end.
        let fresh = told_now(None, "next", 12, 24_000, true, 10);
        assert_eq!(fresh.floor, 12);
        assert_eq!(level(12, 10, fresh.floor), 0);
        assert_eq!(level(16, 10, fresh.floor), 0);
        assert_eq!(level(17, 10, fresh.floor), 1, "grown past where it began");
        assert_eq!(level(90, 10, fresh.floor), 2);
        // Kept for that conversation from then on; a person's own new one has none.
        assert_eq!(told_now(Some(&fresh), "next", 30, 60_000, false, 10).floor, 12);
        assert_eq!(told_now(Some(&fresh), "theirs", 30, 60_000, false, 10).floor, 0);
        assert_eq!(level(11, 10, told_now(None, "theirs", 11, 22_000, false, 10).floor), 1);
    }

    #[test]
    fn a_conversation_compacted_before_a_handoff_is_asked_again() {
        assert_eq!((rearm_below(70), rearm_below(90), rearm_below(20), rearm_below(10), rearm_below(1)), (55, 75, 10, 5, 1));
        // Asked at 71% and 86% of a 200K window, 172K tokens at the last ask;
        // then the CLI compacts it to 18% (36K) within the same conversation
        // id: told nothing from here on, the asks kept, a compaction counted.
        let asked = Told { asked: vec![71, 86], floor: 0, tokens: 172_000, ..told("a", 2) };
        let after = told_now(Some(&asked), "a", 18, 36_000, false, 70);
        assert_eq!((after.level, after.compacted, after.asked.clone()), (0, 1, vec![71, 86]));
        assert!(!due(&after, level(18, 70, after.floor)));
        assert!(due(&after, level(72, 70, after.floor)), "past the threshold again: asked again");
        // Not yet well under: still the conversation told of it.
        assert_eq!(told_now(Some(&asked), "a", 60, 120_000, false, 70), asked);
        // Never asked: a drop is nothing to re-arm.
        let quiet = told("a", 0);
        assert_eq!(told_now(Some(&quiet), "a", 5, 10_000, false, 70), quiet);
        // A floor the handoff gave it goes with the compaction.
        let high = Told { floor: 40, ..told("a", 1) };
        assert_eq!(told_now(Some(&high), "a", 4, 8_000, false, 10).floor, 0);
        // A CLI that named the compacted conversation anew: a new one, asked again too.
        let renamed = told_now(Some(&asked), "b", 18, 36_000, false, 70);
        assert!(renamed.asked.is_empty() && due(&renamed, level(72, 70, renamed.floor)));
    }

    #[test]
    fn a_larger_window_is_asked_again_but_is_no_compaction() {
        // Asked at 71% of a 200K window (142K tokens); the chat goes on on a
        // model with a 1M window: 15%, the same tokens and more.
        let asked = Told { asked: vec![71], tokens: 142_000, ..told("a", 1) };
        let wider = told_now(Some(&asked), "a", 15, 150_000, false, 70);
        assert_eq!((wider.level, wider.compacted, wider.asked.clone()), (0, 0, vec![71]), "asked again, nothing compacted");
        assert!(due(&wider, level(70, 70, wider.floor)), "asked again at the threshold of the new window");
        // A reading a little lower is no compaction either; a tenth and more is.
        assert!(!compacted(&asked, 135_000) && compacted(&asked, 120_000));
        // From before the count was kept: the cause is not known, and not counted.
        let old = Told { asked: vec![71], ..told("a", 1) };
        assert_eq!(told_now(Some(&old), "a", 15, 30_000, false, 70).compacted, 0);
    }
}
