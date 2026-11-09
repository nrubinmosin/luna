//! The threshold: at each turn's end of a chat with auto-handoff on, how full
//! its context is (`Context::of`, as its pane's meter reads it), and once past
//! the threshold one notice, through the notifier, asking for a handoff; a
//! second, firmer one near where the CLI would compact on its own. Each once
//! per conversation: a new one (the handoff's, or a person's own `/clear`)
//! starts over. A conversation a handoff began is asked only once it has grown
//! past where it began: one that starts above the threshold (a small window, a
//! long handoff) would otherwise be asked to hand off again at its first turn's
//! end, and the next one too. Nothing is switched from here.

use super::store::Told;

/// How many points a conversation a handoff began grows before it is asked.
pub(super) const GROWTH: u64 = 5;

/// Where the firmer notice goes: well past the threshold, and short of where
/// the CLIs compact by themselves.
pub(super) fn firm_at(threshold: u8) -> u8 {
    threshold.saturating_add(10).clamp(85, 95)
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

/// What `conversation`, at `pct` now, has been told: what was kept for it, or
/// for one not looked at before nothing yet, and its floor where a handoff
/// began it (`fresh`).
pub(super) fn told_now(told: Option<&Told>, conversation: &str, pct: u64, fresh: bool) -> Told {
    match told.filter(|t| t.conversation == conversation) {
        Some(t) => t.clone(),
        None => Told { conversation: conversation.to_string(), level: 0, floor: if fresh { pct.max(1) } else { 0 } },
    }
}

/// Whether a chat at `level` is yet to hear of it: a level higher than its
/// conversation was told of. One jump past both is one notice, the firm one.
pub(super) fn due(told: &Told, level: u8) -> bool {
    level > told.level
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chat_hears_once_at_its_threshold_and_once_near_the_limit_per_conversation() {
        assert_eq!((firm_at(70), firm_at(30), firm_at(90), firm_at(1)), (85, 85, 95, 85));
        assert_eq!((level(69, 70, 0), level(70, 70, 0), level(84, 70, 0), level(85, 70, 0), level(100, 70, 0)), (0, 1, 1, 2, 2));
        let told = |level| Told { conversation: "a".into(), level, floor: 0 };
        let now = |kept: Option<&Told>, conversation| told_now(kept, conversation, 40, false);
        assert!(!due(&now(None, "a"), 0));
        assert!(due(&now(None, "a"), 1));
        assert!(!due(&now(Some(&told(1)), "a"), 1), "once");
        assert!(due(&now(Some(&told(1)), "a"), 2), "then the firm one");
        assert!(!due(&now(Some(&told(2)), "a"), 2));
        assert!(!due(&now(Some(&told(2)), "a"), 1), "a compaction taking it back under says nothing");
        assert!(due(&now(Some(&told(2)), "b"), 1), "a new conversation starts over");
        assert!(due(&now(None, "a"), 2), "a jump past both: the firm one alone");
    }

    #[test]
    fn a_conversation_a_handoff_began_is_asked_only_once_it_has_grown() {
        // Begun at 12% under a threshold of 10: not asked at its first turn's end.
        let fresh = told_now(None, "next", 12, true);
        assert_eq!(fresh.floor, 12);
        assert_eq!(level(12, 10, fresh.floor), 0);
        assert_eq!(level(16, 10, fresh.floor), 0);
        assert_eq!(level(17, 10, fresh.floor), 1, "grown past where it began");
        assert_eq!(level(90, 10, fresh.floor), 2);
        // Kept for that conversation from then on; a person's own new one has none.
        assert_eq!(told_now(Some(&fresh), "next", 30, false).floor, 12);
        assert_eq!(told_now(Some(&fresh), "theirs", 30, false).floor, 0);
        assert_eq!(level(11, 10, told_now(None, "theirs", 11, false).floor), 1);
    }
}
