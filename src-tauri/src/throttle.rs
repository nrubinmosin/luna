//! Per-account cool-off after a usage endpoint answers 429.
//!
//! Both providers' usage endpoints throttle an eager poller, and neither says
//! for how long (Anthropic sends retry-after 0). Observed behaviour is that
//! coming back a few minutes later just earns the next 429, so a throttled
//! account sits out a full cool-off without a request; each 429 in a row
//! doubles the wait up to a cap, which is both gentler on the endpoint and
//! what turned hundreds of log lines a day into a handful. The UI keeps the
//! last real numbers meanwhile.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

const COOLOFF_MS: u64 = 15 * 60 * 1000;
const MAX_MS: u64 = 60 * 60 * 1000;

#[derive(Clone, Copy)]
struct Throttle {
    until_ms: u64,
    cool_ms: u64,
}

fn map() -> &'static Mutex<HashMap<String, Throttle>> {
    static MAP: OnceLock<Mutex<HashMap<String, Throttle>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Seconds left of a hold on this key, if one is in force.
pub fn remaining(key: &str) -> Option<u64> {
    let t = *map().lock().unwrap().get(key)?;
    let now = now_ms();
    (now < t.until_ms).then(|| ((t.until_ms - now) / 1000).max(1))
}

/// An answer means the sentence is served; the next 429 starts over short.
pub fn clear(key: &str) {
    map().lock().unwrap().remove(key);
}

/// Records a 429. Honours a real retry-after if one ever appears, but never
/// comes back sooner than the cool-off. Returns the hold in seconds.
pub fn hit(tag: &str, key: &str, retry_after_secs: u64) -> u64 {
    let mut m = map().lock().unwrap();
    let previous = m.get(key).map(|t| t.cool_ms);
    let cool_ms =
        (retry_after_secs * 1000).max(previous.map_or(COOLOFF_MS, |c| (c * 2).min(MAX_MS)));
    m.insert(key.to_string(), Throttle { until_ms: now_ms() + cool_ms, cool_ms });
    drop(m);
    // Only while the hold is still growing: at the cap this repeats every
    // hour for as long as the account is throttled, and that is the flood
    // the escalation exists to stop.
    if previous != Some(cool_ms) {
        crate::log::warn(
            tag,
            &format!("429 for {key}, retry-after {retry_after_secs}s — cooling off {}s", cool_ms / 1000),
        );
    }
    cool_ms / 1000
}
