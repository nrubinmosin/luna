//! The threshold: at each turn's end of a chat with auto-handoff on, how full
//! its context is (`Context::of`, as its pane's meter reads it), and once past
//! the threshold one notice, through the notifier, asking for a handoff; a
//! second, firmer one near where the CLI would compact on its own. Each once
//! per conversation: a new one (the handoff's, or a person's own `/clear`)
//! starts over. Nothing is switched from here.

use super::store::Told;

/// Where the firmer notice goes: well past the threshold, and short of where
/// the CLIs compact by themselves.
pub(super) fn firm_at(threshold: u8) -> u8 {
    threshold.saturating_add(10).clamp(85, 95)
}

/// How far `pct` has come: 0 short of the threshold, 1 past it, 2 past the
/// firm mark.
pub(super) fn level(pct: u64, threshold: u8) -> u8 {
    if pct >= u64::from(firm_at(threshold)) {
        2
    } else if pct >= u64::from(threshold) {
        1
    } else {
        0
    }
}

/// Whether a chat at `level` in `conversation` is yet to hear of it: a level
/// higher than this conversation was told of. One jump past both is one
/// notice, the firm one.
pub(super) fn due(told: Option<&Told>, conversation: &str, level: u8) -> bool {
    let had = told.filter(|t| t.conversation == conversation).map_or(0, |t| t.level);
    level > had
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chat_hears_once_at_its_threshold_and_once_near_the_limit_per_conversation() {
        assert_eq!((firm_at(70), firm_at(30), firm_at(90), firm_at(1)), (85, 85, 95, 85));
        assert_eq!((level(69, 70), level(70, 70), level(84, 70), level(85, 70), level(100, 70)), (0, 1, 1, 2, 2));
        let told = |level| Told { conversation: "a".into(), level };
        assert!(!due(None, "a", 0));
        assert!(due(None, "a", 1));
        assert!(!due(Some(&told(1)), "a", 1), "once");
        assert!(due(Some(&told(1)), "a", 2), "then the firm one");
        assert!(!due(Some(&told(2)), "a", 2));
        assert!(!due(Some(&told(2)), "a", 1), "a compaction taking it back under says nothing");
        assert!(due(Some(&told(2)), "b", 1), "a new conversation starts over");
        assert!(due(None, "a", 2), "a jump past both: the firm one alone");
    }
}
