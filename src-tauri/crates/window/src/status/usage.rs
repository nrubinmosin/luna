//! The accounts' usage as the bar shows it: each account's windows (5 hours,
//! week, a model's week), folded from `account_limits` and `codex_limits` as
//! the page's accounts store folds them, how long until each resets, the
//! cadence the polling keeps (never faster than the page's), and how much of
//! each account fits in the room the bar has.

use super::dto::{ClaudeLimits, CodexLimits};
use crate::term::paste::Provider;
use std::time::Duration;

/// How long between one account's ask and the next in a round: fired
/// together, the requests reach the usage endpoint at once and the second
/// account lives 429'd (the page's own 2 to 4 seconds).
pub const GAP: Duration = Duration::from_millis(3000);
/// The wait between rounds when every account has numbers.
const HEALTHY_S: u64 = 60;
/// After any 429: the endpoint sends no retry-after, so sit it out.
const THROTTLED_S: u64 = 180;
/// The wait after an unhappy round, a rung more each time, the last one
/// kept: a fixed short retry turned one 429 into a loop that kept the
/// endpoint throttled for good.
const BACKOFF_S: [u64; 6] = [30, 60, 120, 300, 600, 900];

/// Where an account's numbers stand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standing {
    /// Not asked yet.
    Loading,
    Ready,
    /// Its token expired and could not be renewed just now: the numbers are
    /// the last known.
    Stale,
    /// The endpoint answered 429.
    Throttled,
    /// The ask failed.
    Error,
}

/// One window of an account: which, how full, when it resets.
#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    pub label: String,
    /// The bar's key for it: `5H`, `W`, or a limit's initial (`F`).
    pub short: String,
    /// 0 to 1.
    pub used: f64,
    /// ISO 8601, as the endpoint sent it.
    pub reset_at: Option<String>,
}

/// One account with its usage.
#[derive(Clone, Debug, PartialEq)]
pub struct Usage {
    pub provider: Provider,
    pub name: String,
    pub path: String,
    pub plan: Option<String>,
    pub signed_in: bool,
    pub have_usage: bool,
    pub windows: Vec<Window>,
    pub sync: Standing,
    /// When the numbers were taken, epoch ms, as the endpoint's answer says.
    pub fetched_at_ms: Option<f64>,
}

/// One answer, folded.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Round {
    pub plan: Option<String>,
    pub signed_in: bool,
    pub have_usage: bool,
    pub stale: bool,
    pub rate_limited: Option<u64>,
    pub fetched_at_ms: Option<f64>,
    /// None when the answer had no usable numbers.
    pub windows: Option<Vec<Window>>,
}

/// The bar's key for a window: `5H`, `W`, or a limit's initial (`F` for
/// Fable); Codex names its extra limits `<name> · <window>`, which get the
/// initial and the window (`G5H`, `GW`).
fn short_of(label: &str) -> String {
    let (name, window) = label.split_once(" · ").unwrap_or((label, ""));
    let key = match window {
        "5 hours" => "5H",
        "week" => "W",
        _ => "",
    };
    format!("{}{key}", name.trim().chars().next().map_or('?', |c| c.to_ascii_uppercase()))
}

impl Round {
    pub fn claude(l: ClaudeLimits) -> Round {
        // Only the windows the answer had: one the server did not send is not
        // a window at 0%.
        let mut windows = Vec::new();
        if let Some(used) = l.h5 {
            windows.push(Window { label: "5 hours".into(), short: "5H".into(), used, reset_at: l.reset_h5.clone() });
        }
        if let Some(used) = l.week {
            windows.push(Window { label: "week".into(), short: "W".into(), used, reset_at: l.reset_week.clone() });
        }
        for s in &l.scoped {
            windows.push(Window {
                label: format!("{} · week", s.label),
                short: short_of(&s.label),
                used: s.used,
                reset_at: s.resets_at.clone(),
            });
        }
        for o in &l.other {
            windows.push(Window { label: o.label.clone(), short: short_of(&o.label), used: o.used, reset_at: o.resets_at.clone() });
        }
        Round {
            plan: l.plan,
            signed_in: l.signed_in,
            have_usage: l.have_usage,
            stale: l.stale,
            rate_limited: l.rate_limited,
            fetched_at_ms: l.fetched_at_ms,
            windows: l.have_usage.then_some(windows),
        }
    }

    pub fn codex(l: CodexLimits) -> Round {
        let windows = l
            .windows
            .iter()
            .map(|w| Window {
                label: w.label.clone(),
                short: match w.label.as_str() {
                    "5 hours" => "5H".into(),
                    "week" => "W".into(),
                    other => short_of(other),
                },
                used: w.used,
                reset_at: w.resets_at.clone(),
            })
            .collect();
        Round {
            plan: l.plan,
            signed_in: l.signed_in,
            have_usage: l.have_usage,
            stale: l.stale,
            rate_limited: l.rate_limited,
            fetched_at_ms: l.fetched_at_ms,
            windows: l.have_usage.then_some(windows),
        }
    }
}

impl Usage {
    pub fn new(provider: Provider, name: String, path: String) -> Usage {
        Usage {
            provider,
            name,
            path,
            plan: None,
            signed_in: false,
            have_usage: false,
            windows: Vec::new(),
            sync: Standing::Loading,
            fetched_at_ms: None,
        }
    }

    /// The fullest of its windows: what the dot goes red for.
    pub fn worst(&self) -> f64 {
        self.windows.iter().map(|w| w.used).fold(0., f64::max)
    }

    /// An answer taken in (`None`: the ask failed). Identity always applies:
    /// it comes off disk and is right even when the usage endpoint is not
    /// reachable. A round that brought no numbers keeps the last real ones
    /// rather than wipe the meters for a minute. True when anything shown
    /// changed: the time the numbers were taken is not shown, and a round
    /// that moves only that draws nothing.
    pub fn apply(&mut self, round: Option<Round>) -> bool {
        let before = self.clone();
        match round {
            None => self.sync = Standing::Error,
            Some(r) => {
                self.plan = r.plan;
                self.signed_in = r.signed_in;
                let sync = if r.rate_limited.is_some() {
                    Some(Standing::Throttled)
                } else if r.stale {
                    Some(Standing::Stale)
                } else {
                    None
                };
                match r.windows {
                    Some(windows) => {
                        self.have_usage = true;
                        self.windows = windows;
                        self.fetched_at_ms = r.fetched_at_ms;
                        self.sync = sync.unwrap_or(Standing::Ready);
                    }
                    None => self.sync = sync.unwrap_or(Standing::Error),
                }
            }
        }
        Usage { fetched_at_ms: self.fetched_at_ms, ..before } != *self
    }

    /// No numbers to show and none on the way: what drives the retry cadence
    /// (an account on cached numbers is serviceable).
    pub fn unhappy(&self) -> bool {
        self.sync != Standing::Ready && !self.have_usage
    }
}

/// How long to wait before the next round, as the page's accounts store
/// does: a minute when every account has numbers, a rung of `BACKOFF_S` more
/// each unhappy round, never less than the endpoint's own word nor, after a
/// 429, three minutes.
#[derive(Debug, Default)]
pub struct Cadence {
    step: usize,
}

impl Cadence {
    pub fn next(&mut self, accounts: &[Usage], server_wait: u64) -> Duration {
        let unhappy = accounts.iter().any(Usage::unhappy);
        let throttled = if accounts.iter().any(|a| a.sync == Standing::Throttled) { THROTTLED_S } else { 0 };
        let base = if unhappy {
            let rung = BACKOFF_S[self.step.min(BACKOFF_S.len() - 1)];
            self.step += 1;
            rung
        } else {
            self.step = 0;
            HEALTHY_S
        };
        Duration::from_secs(base.max(server_wait).max(throttled))
    }
}

// ------------------------------------------------------------------- time

/// Days from 1970-01-01 to a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The civil date of a day count from 1970-01-01 (the inverse of the above).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Epoch milliseconds as an ISO 8601 instant in UTC, to the second.
pub fn iso(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// An ISO 8601 / RFC 3339 instant (`2026-08-17T21:00:00+00:00`, a `Z`, a
/// fraction of a second) as epoch milliseconds; None for anything else.
pub fn parse_instant(s: &str) -> Option<i64> {
    let b = s.trim().as_bytes();
    let num = |from: usize, len: usize| -> Option<i64> {
        let digits = b.get(from..from + len)?;
        digits.iter().all(u8::is_ascii_digit).then(|| digits.iter().fold(0i64, |n, d| n * 10 + i64::from(d - b'0')))
    };
    if b.len() < 19 || !matches!(b[4], b'-') || !matches!(b[7], b'-') || !matches!(b[10], b'T' | b't' | b' ') {
        return None;
    }
    let (y, mo, d, h, mi, sec) = (num(0, 4)?, num(5, 2)?, num(8, 2)?, num(11, 2)?, num(14, 2)?, num(17, 2)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let mut at = 19;
    let mut ms = 0;
    if b.get(at) == Some(&b'.') {
        at += 1;
        let start = at;
        while b.get(at).is_some_and(u8::is_ascii_digit) {
            at += 1;
        }
        let frac = &s.trim()[start..at];
        ms = frac.chars().chain("000".chars()).take(3).fold(0i64, |n, c| n * 10 + i64::from(c.to_digit(10).unwrap_or(0)));
    }
    let offset = match b.get(at) {
        None | Some(b'Z' | b'z') => 0,
        Some(&sign @ (b'+' | b'-')) => {
            let (oh, om) = (num(at + 1, 2)?, if b.get(at + 3) == Some(&b':') { num(at + 4, 2)? } else { num(at + 3, 2)? });
            (oh * 60 + om) * if sign == b'+' { 1 } else { -1 }
        }
        _ => return None,
    };
    let days = days_from_civil(y, mo, d);
    Some(((days * 24 + h) * 60 + mi - offset) * 60_000 + sec * 1000 + ms)
}

/// How long until `iso`, as the page's `fmtReset` says it: `now`, `42m`,
/// `3h 5m`, `3h`, `4d`; a dash for none.
pub fn fmt_reset(iso: Option<&str>, now_ms: u64) -> String {
    let Some(at) = iso.and_then(parse_instant) else { return "—".into() };
    let ms = at - now_ms as i64;
    if ms <= 0 {
        return "now".into();
    }
    // Rounded to the minute, as the page rounds.
    let m = (ms + 30_000) / 60_000;
    if m < 60 {
        return format!("{m}m");
    }
    let h = m / 60;
    if h < 48 {
        return if m % 60 != 0 { format!("{h}h {}m", m % 60) } else { format!("{h}h") };
    }
    format!("{}d", (h as f64 / 24.).round() as i64)
}

// -------------------------------------------------------------------- fit

/// How much of each account the bar can afford, most first: everything; no
/// countdowns; no meters (key and number); only the fullest window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Fit {
    Full,
    NoReset,
    NoMeter,
    Worst,
}

impl Fit {
    pub const ALL: [Fit; 4] = [Fit::Full, Fit::NoReset, Fit::NoMeter, Fit::Worst];
}

/// A character of the window's font (Cascadia Mono at 12 px: 0.586 em),
/// rounded up so an estimate errs toward giving detail up.
pub const CELL: f32 = 7.1;
/// A window's meter, in pixels.
pub const METER_W: f32 = 30.;
/// An account's own furniture: dot, provider tag, and the gaps between them.
const FURNITURE: f32 = 8. + 4. + 20. + 4.;

/// The windows an account shows at `fit`.
pub fn shown(a: &Usage, fit: Fit) -> Vec<&Window> {
    if !a.signed_in || !a.have_usage {
        return Vec::new();
    }
    if fit == Fit::Worst {
        let worst = a.worst();
        return a.windows.iter().filter(|w| w.used == worst).take(1).collect();
    }
    a.windows.iter().collect()
}

/// A window's percent as the bar writes it.
pub fn percent(w: &Window) -> String {
    format!("{}%", (w.used * 100.).round() as i64)
}

/// About how wide an account's glance is at `fit`, in pixels.
pub fn width_of(a: &Usage, fit: Fit, now_ms: u64) -> f32 {
    let mut w = FURNITURE + a.name.chars().count() as f32 * CELL;
    let windows = shown(a, fit);
    if windows.is_empty() {
        // "signed out", "…" or a dash.
        return w + 4. + 10. * CELL;
    }
    for win in windows {
        w += 8. + (win.short.chars().count() as f32 + percent(win).chars().count() as f32) * CELL;
        if fit < Fit::NoMeter {
            w += 4. + METER_W;
        }
        if fit == Fit::Full {
            w += 4. + fmt_reset(win.reset_at.as_deref(), now_ms).chars().count() as f32 * CELL;
        }
    }
    w
}

/// The most detail every account fits in `room` pixels with, the gaps
/// between them counted: the last step when none does.
pub fn fit_in(accounts: &[Usage], room: f32, now_ms: u64) -> Fit {
    for fit in Fit::ALL {
        let total: f32 = accounts.iter().map(|a| width_of(a, fit, now_ms)).sum::<f32>() + 12. * accounts.len().saturating_sub(1) as f32;
        if total <= room {
            return fit;
        }
    }
    Fit::Worst
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(iso: &str) -> u64 {
        parse_instant(iso).unwrap() as u64
    }

    #[test]
    fn instants_parse_as_iso_and_rfc_3339() {
        assert_eq!(parse_instant("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_instant("2026-08-17T21:00:00+00:00"), Some(1_787_000_400_000));
        // A zone moves it, a fraction of a second is kept to the millisecond.
        assert_eq!(parse_instant("2026-08-17T23:00:00+02:00"), parse_instant("2026-08-17T21:00:00Z"));
        assert_eq!(parse_instant("2026-08-17T21:00:00.25Z"), Some(1_787_000_400_250));
        assert_eq!(parse_instant("2024-02-29T00:00:00Z"), Some(1_709_164_800_000), "a leap day");
        // And back.
        for ms in [0, 1_709_164_800_000, 1_787_000_400_000, 4_102_444_799_000] {
            assert_eq!(parse_instant(&iso(ms)), Some(ms), "{}", iso(ms));
        }
        assert_eq!(iso(1_787_000_400_999), "2026-08-17T21:00:00Z");
        for bad in ["", "tomorrow", "2026-13-01T00:00:00Z", "2026-08-17", "2026-08-17T25:00:00Z", "2026-08-17T21:00:00+x"] {
            assert_eq!(parse_instant(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_reset_reads_as_the_page_says_it() {
        let now = at("2026-08-17T12:00:00Z");
        let say = |iso: &str| fmt_reset(Some(iso), now);
        assert_eq!(say("2026-08-17T12:42:00Z"), "42m");
        assert_eq!(say("2026-08-17T15:05:00Z"), "3h 5m");
        assert_eq!(say("2026-08-17T15:00:00Z"), "3h");
        assert_eq!(say("2026-08-21T12:00:00Z"), "4d");
        assert_eq!(say("2026-08-17T11:00:00Z"), "now");
        assert_eq!(fmt_reset(None, now), "—");
        assert_eq!(fmt_reset(Some("soon"), now), "—");
    }

    fn claude(h5: Option<f64>, have: bool) -> ClaudeLimits {
        ClaudeLimits {
            signed_in: true,
            plan: Some("Max".into()),
            have_usage: have,
            h5,
            week: Some(0.2),
            reset_h5: Some("2026-08-17T15:00:00Z".into()),
            scoped: vec![super::super::dto::ExtraLimit { label: "Fable".into(), used: 0.4, ..Default::default() }],
            ..Default::default()
        }
    }

    #[test]
    fn claudes_answer_folds_into_its_windows() {
        let r = Round::claude(claude(Some(0.5), true));
        let keys: Vec<_> = r.windows.as_ref().unwrap().iter().map(|w| w.short.as_str()).collect();
        assert_eq!(keys, ["5H", "W", "F"]);
        // A window the answer lacks is absent, not at 0%.
        let r = Round::claude(claude(None, true));
        let keys: Vec<_> = r.windows.as_ref().unwrap().iter().map(|w| w.short.as_str()).collect();
        assert_eq!(keys, ["W", "F"]);
        assert_eq!(Round::claude(claude(Some(0.5), false)).windows, None);
        assert_eq!(short_of("Codex Spark · 5 hours"), "C5H");
        assert_eq!(short_of("g · week"), "GW");
        assert_eq!(short_of(""), "?");
    }

    #[test]
    fn a_round_with_no_numbers_keeps_the_last_real_ones() {
        let mut a = Usage::new(Provider::Claude, "work".into(), "p".into());
        assert!(a.unhappy(), "not asked yet");
        assert!(a.apply(Some(Round::claude(claude(Some(0.5), true)))));
        assert_eq!((a.sync, a.have_usage, a.windows.len(), a.unhappy()), (Standing::Ready, true, 3, false));
        // Throttled with nothing new: the meters stay, only the state moves.
        let throttled = Round { rate_limited: Some(60), signed_in: true, ..Default::default() };
        assert!(a.apply(Some(throttled)));
        assert_eq!((a.sync, a.windows.len()), (Standing::Throttled, 3));
        assert!(!a.unhappy(), "on kept numbers it is serviceable");
        // The same answer twice changes nothing shown.
        assert!(a.apply(None));
        assert_eq!((a.sync, a.windows.len()), (Standing::Error, 3));
        assert!(!a.apply(None));
        // The same numbers taken again later: nothing shown moved.
        let mut again = claude(Some(0.5), true);
        again.fetched_at_ms = Some(1.0);
        assert!(a.apply(Some(Round::claude(again.clone()))), "back from the error");
        again.fetched_at_ms = Some(60_001.0);
        assert!(!a.apply(Some(Round::claude(again))));
        assert_eq!(a.fetched_at_ms, Some(60_001.0), "kept, for the agents' account list");
    }

    #[test]
    fn the_cadence_is_never_faster_than_the_pages() {
        let mut ready = Usage::new(Provider::Claude, "a".into(), "p".into());
        ready.apply(Some(Round::claude(claude(Some(0.1), true))));
        let lost = Usage::new(Provider::Codex, "b".into(), "q".into());
        let mut c = Cadence::default();
        let secs = |d: Duration| d.as_secs();
        assert_eq!(secs(c.next(std::slice::from_ref(&ready), 0)), 60);
        // Unhappy rounds climb the ladder to its last rung and stay there.
        let all = [ready.clone(), lost];
        let ladder: Vec<u64> = (0..8).map(|_| secs(c.next(&all, 0))).collect();
        assert_eq!(ladder, [30, 60, 120, 300, 600, 900, 900, 900]);
        // A happy round starts it over; the server's word and a 429 stretch it.
        assert_eq!(secs(c.next(std::slice::from_ref(&ready), 0)), 60);
        assert_eq!(secs(c.next(std::slice::from_ref(&ready), 400)), 400);
        ready.apply(Some(Round { rate_limited: Some(1), signed_in: true, ..Default::default() }));
        assert_eq!(secs(c.next(std::slice::from_ref(&ready), 0)), 180);
    }

    #[test]
    fn the_bar_gives_up_detail_a_step_at_a_time() {
        let now = at("2026-08-17T12:00:00Z");
        let mut a = Usage::new(Provider::Claude, "work".into(), "p".into());
        a.apply(Some(Round::claude(claude(Some(0.5), true))));
        let mut b = a.clone();
        b.name = "personal".into();
        let both = [a.clone(), b];
        let widths: Vec<f32> = Fit::ALL.iter().map(|f| width_of(&a, *f, now)).collect();
        assert!(widths.windows(2).all(|w| w[0] > w[1]), "each step is narrower: {widths:?}");
        assert_eq!(fit_in(&both, 4000., now), Fit::Full);
        assert_eq!(fit_in(&both, 1., now), Fit::Worst);
        // Wide enough for everything but the countdowns.
        let room = both.iter().map(|x| width_of(x, Fit::NoReset, now)).sum::<f32>() + 12.;
        assert_eq!(fit_in(&both, room, now), Fit::NoReset);
        // The last step shows the fullest window alone.
        assert_eq!(shown(&a, Fit::Worst).len(), 1);
        assert_eq!(shown(&a, Fit::Worst)[0].short, "5H");
    }
}
