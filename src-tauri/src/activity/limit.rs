//! Whether the usage limit a chat stopped on still holds: what a move's
//! window goes by to offer the document Luna builds from records first
//! (agents/handoff/moving.rs), since a chat stopped on a limit cannot write
//! its own.
//!
//! The limit holds until every limit that counts against the chat (the
//! windows a reading scopes to one model, or to none) is known to have
//! lifted: read after the stop with no window full, or with every window it
//! showed full past its reset. A limit read only before the stop and not
//! full says nothing of now, and one the reading lacks is not known to have
//! lifted: one with no window that counts says nothing. A reading is what
//! Luna knows without a request (the CLI's cache or Luna's last answer for
//! Claude Code; Luna's last answer and each limit's newest rollout snapshot
//! for Codex, the chat's own rollout among them, each window as of its own):
//! the window's own polls keep it fresh. With no reading, or no reset to go
//! by, it holds.
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
    /// The model the window counts alone, as its source names it (Claude
    /// Code's "Opus", a Codex limit's model): None for one every model on
    /// the account counts against.
    pub model: Option<String>,
    /// When it was read, where that is not the reading's own `at`: a Codex
    /// limit whose newest snapshot is older than another's.
    pub at: Option<u64>,
}

/// An account's usage as last known, and when it was taken.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UsageReading {
    pub at: Option<u64>,
    pub windows: Vec<UsageWindow>,
}

impl UsageReading {
    /// The windows that hold a chat on `model`: every unscoped one, and a
    /// scoped one whose model is `model`'s. A full Opus week does not keep a
    /// Sonnet chat. Where Luna cannot tell (the chat's model unknown, its
    /// CLI's default), every window counts, as a limit wrongly lifted would
    /// have the chat asked for nothing.
    pub fn for_model(&self, model: Option<&str>) -> UsageReading {
        let windows = self.windows.iter().filter(|w| applies(w.model.as_deref(), model)).cloned().collect();
        UsageReading { at: self.at, windows }
    }
}

/// Whether a window scoped to `scope` counts against a chat on `model`:
/// the scope's name, letters and digits alone and in any case, within the
/// model's ("Opus" in "claude-opus-4-1", "GPT-5 Codex Spark" in
/// "gpt-5-codex-spark").
fn applies(scope: Option<&str>, model: Option<&str>) -> bool {
    let bare = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect::<String>();
    match (scope.map(bare).filter(|s| !s.is_empty()), model.map(bare).filter(|m| !m.is_empty())) {
        (Some(scope), Some(model)) => model.contains(&scope),
        _ => true,
    }
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

/// When `chat`'s limit `kind` stopped it: what its source says (`told`), or,
/// where it says nothing, when it was first seen. The source's word always
/// stands: one earlier than the first sight is that stop's own, one later is
/// a new stop of the same kind after a recovery no look saw, which must not
/// keep the older stop's time.
fn stop_of(chat: &str, kind: &'static str, told: Option<u64>, now: u64) -> u64 {
    let mut s = stops().lock().unwrap_or_else(|e| e.into_inner());
    let since = match (told, s.get(chat)) {
        (Some(t), _) => t,
        (None, Some((k, seen))) if *k == kind => *seen,
        (None, _) => now,
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
    let probe = crate::pty::manager().probe(chat);
    let account = probe.as_ref().map(|p| p.account_path.clone()).unwrap_or_default();
    let reading = match provider {
        Provider::Claude => crate::claude::limits::known_reading(&account),
        // The chat's own rollout too, wherever it falls among the account's:
        // it holds the snapshot of the limit the chat runs against.
        Provider::Codex => {
            let own = probe.as_ref().and_then(|p| crate::codex::session::rollout_for(&codex_live(p)));
            crate::codex::limits::known_reading(&account, own.as_deref())
        }
    };
    let model = crate::agents::stopped_model_of(chat, provider);
    in_force(kind, since, reading.map(|r| r.for_model(model.as_deref())).as_ref(), now)
}

/// The ISO instant `at`, in ms.
fn ms_of(at: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(at).ok().map(|t| t.timestamp_millis().max(0) as u64)
}

/// The limit `kind` that stopped a chat at `since`, judged on `reading` at
/// `now`: the fact if it still holds.
pub(crate) fn in_force(kind: &str, since: u64, reading: Option<&UsageReading>, now: u64) -> Option<LimitFact> {
    let full: Vec<&UsageWindow> = reading.map(|r| r.windows.iter().filter(|w| w.used >= FULL).collect()).unwrap_or_default();
    if let Some(r) = reading.filter(|r| !r.windows.is_empty()) {
        // Each limit by its windows, the model they count alone naming it.
        let mut limits: Vec<(Option<&str>, Vec<&UsageWindow>)> = Vec::new();
        for w in &r.windows {
            match limits.iter_mut().find(|(m, _)| *m == w.model.as_deref()) {
                Some((_, ws)) => ws.push(w),
                None => limits.push((w.model.as_deref(), vec![w])),
            }
        }
        // A limit lifted: every window read after the stop, nothing full;
        // or every window it showed full past its reset, by its own clock.
        // One read only before the stop and not full may be the one that
        // stopped the chat, filled since: not known, so the chat is held.
        let after = |w: &UsageWindow| w.at.or(r.at).is_some_and(|at| at > since);
        let past_reset = |w: &&UsageWindow| w.resets_at.as_deref().and_then(ms_of).is_some_and(|t| now >= t);
        let lifted = |ws: &Vec<&UsageWindow>| {
            let full: Vec<&&UsageWindow> = ws.iter().filter(|w| w.used >= FULL).collect();
            if full.is_empty() {
                ws.iter().all(|w| after(w))
            } else {
                full.into_iter().all(past_reset)
            }
        };
        if limits.iter().all(|(_, ws)| lifted(ws)) {
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
        UsageWindow { label: label.into(), used, resets_at: resets_at.map(str::to_owned), model: None, at: None }
    }

    /// A limit is lifted only by a reading of the windows that count: one
    /// with none of them (a Spark chat's snapshot, for a chat on another
    /// model) says nothing, and a window read before the stop, beside one
    /// read after, may be the one that stopped it.
    #[test]
    fn a_window_not_read_since_the_stop_is_not_taken_for_lifted() {
        let spark =
            |used, at| UsageWindow { model: Some("GPT-5 Codex Spark".into()), at: Some(at), ..window("Spark · 5 hours", used, None) };
        let general = |used, at| UsageWindow { at: Some(at), ..window("5 hours", used, None) };
        let reading = |windows: Vec<UsageWindow>| UsageReading { at: windows.iter().filter_map(|w| w.at).max(), windows };
        let judged = |r: &UsageReading, model| in_force("usage_limit", STOP, Some(&r.for_model(model)), STOP + 61_000);
        // Only another model's limit read since: nothing counts, it holds.
        let only_spark = reading(vec![spark(0.1, STOP + 60_000)]);
        assert!(judged(&only_spark, Some("gpt-5-codex")).is_some());
        // The general limit full before the stop, Spark read after: holds.
        let both = reading(vec![general(1.0, STOP - 1_000), spark(0.1, STOP + 60_000)]);
        assert!(judged(&both, Some("gpt-5-codex")).is_some() && judged(&both, Some("gpt-5-codex-spark")).is_some());
        // A full Spark window from before, the general one read after: a Spark chat stays held.
        let spark_full = reading(vec![spark(1.0, STOP - 1_000), general(0.1, STOP + 60_000)]);
        assert!(judged(&spark_full, Some("gpt-5-codex-spark")).is_some());
        assert!(judged(&spark_full, Some("gpt-5-codex")).is_none(), "its own limit read since, not full");
        // Spark read before the stop and not full: no word on it for a Spark chat.
        let spark_old = reading(vec![spark(0.5, STOP - 1_000), general(0.1, STOP + 60_000)]);
        assert!(judged(&spark_old, Some("gpt-5-codex-spark")).is_some());
        // Both read since, neither full: lifted.
        let fresh = reading(vec![spark(0.5, STOP + 50_000), general(0.1, STOP + 60_000)]);
        assert!(judged(&fresh, Some("gpt-5-codex-spark")).is_none());
        assert!(in_force("usage_limit", STOP, Some(&UsageReading { at: Some(STOP + 1), windows: vec![] }), STOP + 2).is_some());
    }

    /// A limit that counts against the chat, read only before the stop and
    /// not full, says nothing of now: the general limit's full window past
    /// its reset lifts a chat on the general model, not a Spark chat whose own
    /// limit has had no reading since.
    #[test]
    fn a_limit_with_no_reading_since_the_stop_holds_whatever_another_says() {
        let spark =
            UsageWindow { model: Some("GPT-5 Codex Spark".into()), at: Some(STOP - 1_000), ..window("Spark · 5 hours", 0.9, None) };
        let general = UsageWindow { at: Some(STOP - 1_000), ..window("5 hours", 1.0, Some("2027-01-15T09:00:00Z")) };
        let reading = UsageReading { at: Some(STOP - 1_000), windows: vec![general, spark] };
        let after_reset = ms_of("2027-01-15T09:00:01Z").unwrap();
        let judged = |model| in_force("usage_limit", STOP, Some(&reading.for_model(model)), after_reset);
        assert!(judged(Some("gpt-5-codex")).is_none(), "its full window has reset");
        assert!(judged(Some("gpt-5-codex-spark")).is_some(), "Spark not read since: unknown, held");
        // Spark read since, not full: lifted for it too.
        let mut fresh = reading.clone();
        fresh.windows[1].at = Some(STOP + 1_000);
        assert!(in_force("usage_limit", STOP, Some(&fresh.for_model(Some("gpt-5-codex-spark"))), after_reset).is_none());
    }

    /// A Sonnet chat stopped by the session window, which has reset: an
    /// Opus-only week still full does not keep it, but keeps an Opus chat,
    /// and one on a model Luna does not know.
    #[test]
    fn a_window_scoped_to_another_model_does_not_hold_a_chat() {
        let opus_week = UsageWindow { model: Some("Opus".into()), ..window("week, Opus", 1.0, Some("2027-03-05T00:00:00Z")) };
        let reading = UsageReading { at: Some(STOP + 60_000), windows: vec![window("5 hours", 0.03, None), opus_week] };
        let judged = |model| in_force("rate_limit", STOP, Some(&reading.for_model(model)), STOP + 61_000);
        assert_eq!(judged(Some("claude-sonnet-4-5")), None, "the window it stopped on has reset");
        assert_eq!(judged(Some("sonnet")), None, "an alias names it too");
        assert_eq!(judged(Some("claude-opus-4-1")).and_then(|f| f.window).as_deref(), Some("week, Opus"));
        assert_eq!(judged(Some("opus")).and_then(|f| f.window).as_deref(), Some("week, Opus"));
        assert!(judged(None).is_some(), "no model known: every window counts");
        // A Codex limit of its own model, by its display name.
        let spark = UsageWindow { model: Some("GPT-5 Codex Spark".into()), ..window("GPT-5 Codex Spark · 5 hours", 1.0, None) };
        let codex = UsageReading { at: Some(STOP + 60_000), windows: vec![window("5 hours", 0.1, None), spark] };
        assert!(in_force("usage_limit", STOP, Some(&codex.for_model(Some("gpt-5-codex"))), STOP + 61_000).is_none());
        assert!(in_force("usage_limit", STOP, Some(&codex.for_model(Some("gpt-5-codex-spark"))), STOP + 61_000).is_some());
    }

    /// A Codex chat switched to Spark whose first Spark turn hit Spark's full
    /// limit is judged by Spark, the model of the turn that stopped, not by
    /// the general model that answered last: a fresh general reading does
    /// not lift it. A Claude Code chat's failed request names no model, so
    /// its settings' model is taken, and with none every window counts.
    #[test]
    fn a_limit_is_judged_by_the_model_of_the_turn_it_stopped() {
        let path = std::env::temp_dir().join(format!("luna-limit-model-{}.jsonl", std::process::id()));
        let lines = [
            r#"{"timestamp":"t","type":"turn_context","payload":{"model":"gpt-5-codex","effort":"low"}}"#,
            r#"{"timestamp":"t","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"output_tokens":5}}}}"#,
            r#"{"timestamp":"t","type":"turn_context","payload":{"model":"gpt-5-codex-spark","effort":"low"}}"#,
            r#"{"timestamp":"t","type":"event_msg","payload":{"type":"error","codex_error_info":"usage_limit_exceeded"}}"#,
        ];
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        assert_eq!(crate::codex::session::answered_model(&path).as_deref(), Some("gpt-5-codex"), "the one that answered last");
        let model = crate::agents::stopped_model(Provider::Codex, Some(&path), Some("gpt-5-codex".into()));
        assert_eq!(model.as_deref(), Some("gpt-5-codex-spark"));
        let spark = UsageWindow {
            model: Some("GPT-5 Codex Spark".into()),
            at: Some(STOP - 1_000),
            ..window("Spark · 5 hours", 1.0, Some("2027-03-01T00:00:00Z"))
        };
        let general = UsageWindow { at: Some(STOP + 60_000), ..window("5 hours", 0.1, None) };
        let reading = UsageReading { at: Some(STOP + 60_000), windows: vec![spark, general] };
        assert!(in_force("usage_limit", STOP, Some(&reading.for_model(model.as_deref())), STOP + 61_000).is_some());
        assert!(in_force("usage_limit", STOP, Some(&reading.for_model(Some("gpt-5-codex"))), STOP + 61_000).is_none());
        // No turn named in the rollout: the settings' model, else none.
        assert_eq!(crate::agents::stopped_model(Provider::Codex, None, Some("m".into())).as_deref(), Some("m"));
        assert_eq!(crate::agents::stopped_model(Provider::Codex, None, None), None);
        // Claude Code: its transcript is not asked.
        assert_eq!(crate::agents::stopped_model(Provider::Claude, Some(&path), Some("opus".into())).as_deref(), Some("opus"));
        assert_eq!(crate::agents::stopped_model(Provider::Claude, Some(&path), None), None);
        let _ = std::fs::remove_file(&path);
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
    fn the_stop_is_its_sources_word_and_a_new_kind_starts_over() {
        assert_eq!(stop_of("lim-a", "rate_limit", None, 5_000), 5_000, "first seen");
        assert_eq!(stop_of("lim-a", "rate_limit", None, 9_000), 5_000, "kept");
        assert_eq!(stop_of("lim-a", "rate_limit", Some(3_000), 9_000), 3_000, "its source says earlier");
        assert_eq!(stop_of("lim-a", "rate_limit", None, 9_500), 3_000, "kept as the source said");
        assert_eq!(stop_of("lim-a", "usage_limit", None, 9_000), 9_000, "another limit");
    }

    /// Limit A, a recovery no look saw, then limit B of the same kind: B's
    /// stop is its own, so a reading taken between the two lifts nothing.
    #[test]
    fn a_second_stop_of_the_same_kind_does_not_keep_the_firsts_time() {
        assert_eq!(stop_of("lim-b", "rate_limit", Some(3_000), 4_000), 3_000, "limit A");
        assert_eq!(stop_of("lim-b", "rate_limit", Some(10_000), 11_000), 10_000, "limit B, by its own source");
        let between = UsageReading { at: Some(6_000), windows: vec![window("5 hours", 0.4, None)] };
        assert!(in_force("rate_limit", 10_000, Some(&between), 11_000).is_some(), "a reading from before B is no word on it");
        assert!(in_force("rate_limit", 3_000, Some(&between), 11_000).is_none(), "as A's stop it would have lifted");
    }
}
