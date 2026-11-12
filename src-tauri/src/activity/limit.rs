//! Whether the usage limit a chat stopped on still holds: what a move's
//! window goes by to offer the document Luna builds from records first
//! (agents/handoff/moving.rs), since a chat stopped on a limit cannot write
//! its own.
//!
//! The limit holds from the stop until a reading of the account's usage
//! taken after it shows no window full, or until every window a reading
//! showed full has passed its reset. A reading is what Luna knows without a
//! request (the CLI's cache or Luna's last answer for Claude Code, the newest
//! rollout or Luna's last answer for Codex): the window's own polls keep it
//! fresh. With no reading, or no reset to go by, it holds.
//!
//! When the stop was is taken from its source where that says (Claude Code's
//! `StopFailure` hook, the Codex rollout line), else from when Luna first saw
//! the limit, kept here in memory: a core restart sees it anew, which only
//! makes an older reading count as before the stop.

use super::*;

/// A window at or past this share is full: what stops a turn.
const FULL: f64 = 0.995;

/// One window of an account's usage, as a reading has it.
#[derive(Clone, Debug, PartialEq)]
pub struct UsageWindow {
    /// "5 hours", "week", or the window's own name.
    pub label: String,
    /// 0..1
    pub used: f64,
    /// ISO instant.
    pub resets_at: Option<String>,
}

/// An account's usage as last known, and when it was taken.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UsageReading {
    pub at: Option<u64>,
    pub windows: Vec<UsageWindow>,
}

/// A usage limit that still holds a chat, as the window shows it: its kind,
/// since when, and the window that is full with its reset, by the reading
/// it was judged on and how old that was.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LimitFact {
    pub kind: String,
    pub since: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reading_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reading_age_ms: Option<u64>,
}

/// Each chat's limit as first seen: its kind and when it stopped the chat.
fn stops() -> &'static Mutex<HashMap<String, (&'static str, u64)>> {
    static S: OnceLock<Mutex<HashMap<String, (&'static str, u64)>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// When `chat`'s limit `kind` stopped it: what its source says (`told`), or
/// when it was first seen, the earlier of the two once both are known.
fn stop_of(chat: &str, kind: &'static str, told: Option<u64>, now: u64) -> u64 {
    let mut s = stops().lock().unwrap_or_else(|e| e.into_inner());
    let since = match s.get(chat) {
        Some((k, seen)) if *k == kind => told.map_or(*seen, |t| t.min(*seen)),
        _ => told.unwrap_or(now),
    };
    s.insert(chat.to_string(), (kind, since));
    since
}

/// The usage limit that still holds `chat`, a `provider` chat, if one does
/// (as the module says); None for one not stopped on a limit, or not running.
pub fn limit_in_force(chat: &str, provider: Provider) -> Option<LimitFact> {
    let Some((kind, told)) = limit_at(chat, provider) else {
        stops().lock().unwrap_or_else(|e| e.into_inner()).remove(chat);
        return None;
    };
    let now = now_ms();
    let since = stop_of(chat, kind, told, now);
    let account = crate::pty::manager().probe(chat).map(|p| p.account_path).unwrap_or_default();
    let reading = match provider {
        Provider::Claude => crate::claude::limits::known_reading(&account),
        Provider::Codex => crate::codex::limits::known_reading(&account),
    };
    in_force(kind, since, reading.as_ref(), now)
}

/// The ISO instant `at`, in ms.
fn ms_of(at: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(at).ok().map(|t| t.timestamp_millis().max(0) as u64)
}

/// The limit `kind` that stopped a chat at `since`, judged on `reading` at
/// `now`: the fact if it still holds.
pub(crate) fn in_force(kind: &str, since: u64, reading: Option<&UsageReading>, now: u64) -> Option<LimitFact> {
    let full: Vec<&UsageWindow> = reading.map(|r| r.windows.iter().filter(|w| w.used >= FULL).collect()).unwrap_or_default();
    if let Some(r) = reading {
        // Read after the stop with nothing full: the limit has lifted.
        if r.at.is_some_and(|at| at > since) && full.is_empty() {
            return None;
        }
        // Every full window past its reset, by its own clock.
        if !full.is_empty() && full.iter().all(|w| w.resets_at.as_deref().and_then(ms_of).is_some_and(|t| now >= t)) {
            return None;
        }
    }
    // The window that holds longest: a full one with the latest reset (none
    // known being the latest), else the one most used.
    let shown =
        full.iter().copied().max_by_key(|w| w.resets_at.as_deref().and_then(ms_of).unwrap_or(u64::MAX)).or_else(|| {
            reading.and_then(|r| r.windows.iter().max_by(|a, b| a.used.partial_cmp(&b.used).unwrap_or(std::cmp::Ordering::Equal)))
        });
    let reading_at = reading.and_then(|r| r.at);
    Some(LimitFact {
        kind: kind.to_string(),
        since,
        window: shown.map(|w| w.label.clone()),
        used: shown.map(|w| w.used),
        resets_at: shown.and_then(|w| w.resets_at.clone()),
        reading_at,
        reading_age_ms: reading_at.map(|at| now.saturating_sub(at)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(label: &str, used: f64, resets_at: Option<&str>) -> UsageWindow {
        UsageWindow { label: label.into(), used, resets_at: resets_at.map(str::to_owned) }
    }

    /// 2027-01-15T08:00:00Z.
    const STOP: u64 = 1_800_000_000_000;

    #[test]
    fn a_limit_holds_from_the_stop_until_a_newer_reading_or_its_reset_says_otherwise() {
        let full = |at| UsageReading { at: Some(at), windows: vec![window("5 hours", 1.0, Some("2027-03-01T00:00:00Z"))] };
        // Right after the stop: the reading that saw the window fill holds it.
        let f = in_force("rate_limit", STOP, Some(&full(STOP - 1_000)), STOP + 1_000).expect("in force");
        assert_eq!((f.window.as_deref(), f.used, f.resets_at.as_deref()), (Some("5 hours"), Some(1.0), Some("2027-03-01T00:00:00Z")));
        assert_eq!((f.since, f.reading_at, f.reading_age_ms), (STOP, Some(STOP - 1_000), Some(2_000)));
        // A newer reading shows the window reset: lifted.
        let reset = UsageReading { at: Some(STOP + 60_000), windows: vec![window("5 hours", 0.02, None)] };
        assert_eq!(in_force("rate_limit", STOP, Some(&reset), STOP + 61_000), None);
        // A reading from before the stop with nothing full is no word on it.
        let before = UsageReading { at: Some(STOP - 60_000), windows: vec![window("5 hours", 0.9, None)] };
        assert!(in_force("rate_limit", STOP, Some(&before), STOP + 1_000).is_some());
        // The full window's reset has passed, whatever the reading's age.
        let reset_at = ms_of("2027-03-01T00:00:00Z").unwrap();
        assert_eq!(in_force("rate_limit", STOP, Some(&full(STOP - 1_000)), reset_at), None);
        // Still full in a newer reading: holds.
        assert!(in_force("rate_limit", STOP, Some(&full(STOP + 60_000)), STOP + 61_000).is_some());
    }

    #[test]
    fn with_no_reset_data_a_limit_holds() {
        let f = in_force("usage_limit", STOP, None, STOP + 10 * 24 * 60 * 60_000).expect("no reading: in force");
        assert_eq!((f.window, f.reading_at), (None, None));
        let no_reset = UsageReading { at: Some(STOP - 1), windows: vec![window("week", 1.0, None)] };
        assert!(in_force("usage_limit", STOP, Some(&no_reset), u64::MAX / 2).is_some(), "a full window with no reset");
        // A reading of unknown age is not after the stop.
        let unknown = UsageReading { at: None, windows: vec![window("week", 0.1, None)] };
        assert!(in_force("usage_limit", STOP, Some(&unknown), STOP + 1).is_some());
        // Two full windows: the one that resets last is shown, and both must pass.
        let two = UsageReading {
            at: Some(STOP - 1),
            windows: vec![window("5 hours", 1.0, Some("2027-03-01T00:00:00Z")), window("week", 1.0, Some("2027-03-05T00:00:00Z"))],
        };
        let between = ms_of("2027-03-02T00:00:00Z").unwrap();
        assert_eq!(in_force("usage_limit", STOP, Some(&two), between).and_then(|f| f.window).as_deref(), Some("week"));
    }

    #[test]
    fn the_stop_is_the_earliest_word_of_it_and_a_new_kind_starts_over() {
        assert_eq!(stop_of("lim-a", "rate_limit", None, 5_000), 5_000, "first seen");
        assert_eq!(stop_of("lim-a", "rate_limit", None, 9_000), 5_000, "kept");
        assert_eq!(stop_of("lim-a", "rate_limit", Some(3_000), 9_000), 3_000, "its source says earlier");
        assert_eq!(stop_of("lim-a", "usage_limit", None, 9_000), 9_000, "another limit");
    }
}
