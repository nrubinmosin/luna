//! Context-window sizes, read from the Models API rather than guessed.
//!
//! Hardcoding them went wrong twice: the table drifts as models ship, and a
//! wrong window silently multiplies every context percentage in the UI. The
//! endpoint is account metadata — no inference, no tokens — so this costs one
//! request every few hours.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

const REFRESH_MS: u64 = 6 * 3600 * 1000;

type Cache = (u64, HashMap<String, f64>);

fn cache() -> &'static Mutex<Cache> {
    static C: OnceLock<Mutex<Cache>> = OnceLock::new();
    C.get_or_init(|| Mutex::new((0, HashMap::new())))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Window for a model id as recorded in a transcript, e.g. `claude-opus-5`.
pub fn context_window(model: &str) -> f64 {
    if let Ok(c) = cache().lock() {
        let map = &c.1;
        if let Some(w) = map.get(model) {
            return *w;
        }
        // Transcript ids and API ids can differ by a date suffix in either
        // direction (`claude-haiku-4-5` vs `claude-haiku-4-5-20251001`).
        if let Some((_, w)) = map
            .iter()
            .find(|(id, _)| id.starts_with(model) || model.starts_with(id.as_str()))
        {
            return *w;
        }
    }
    fallback(model)
}

/// Used until the first fetch lands, and if the endpoint is unreachable.
/// Values as published on 2026-08-13: everything current is a megatoken except
/// Haiku, and Opus was 200k before the 4.6 generation.
fn fallback(model: &str) -> f64 {
    let m = model.to_ascii_lowercase();
    if m.contains("[1m]") {
        return 1_000_000.0;
    }
    if m.contains("haiku") {
        return 200_000.0;
    }
    if let Some((major, minor)) = version_after(&m, "opus") {
        return if (major, minor) >= (4, 6) { 1_000_000.0 } else { 200_000.0 };
    }
    if m.contains("sonnet") || m.contains("fable") || m.contains("mythos") {
        return 1_000_000.0;
    }
    200_000.0
}

/// `claude-opus-4-6` → (4, 6); `claude-opus-5` → (5, 0). None for the legacy
/// `claude-3-5-sonnet-<date>` shape, where the digits after the family name are
/// a date rather than a version.
fn version_after(model: &str, family: &str) -> Option<(u32, u32)> {
    let rest = model.split_once(family)?.1.trim_start_matches('-');
    let mut parts = rest.split('-');
    let major: u32 = parts.next()?.parse().ok()?;
    if major > 50 {
        return None;
    }
    let minor: u32 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    Some((major, minor))
}

fn read_token(account_path: &str) -> Option<String> {
    let raw = std::fs::read_to_string(std::path::Path::new(account_path).join(".credentials.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let oauth = &v["claudeAiOauth"];
    if oauth["expiresAt"].as_u64().map(|ms| now_ms() >= ms).unwrap_or(false) {
        return None;
    }
    oauth["accessToken"].as_str().map(str::to_owned)
}

fn fetch(token: &str) -> Option<HashMap<String, f64>> {
    let resp = ureq::builder()
        .timeout_connect(std::time::Duration::from_secs(5))
        .timeout_read(std::time::Duration::from_secs(10))
        .build()
        .get("https://api.anthropic.com/v1/models?limit=100")
        .set("Authorization", &format!("Bearer {token}"))
        .set("anthropic-version", "2023-06-01")
        .set("anthropic-beta", "oauth-2025-04-20")
        .call()
        .ok()?;
    let body: serde_json::Value = resp.into_json().ok()?;

    let mut out = HashMap::new();
    for m in body["data"].as_array()? {
        if let (Some(id), Some(window)) = (m["id"].as_str(), m["max_input_tokens"].as_f64()) {
            if window > 0.0 {
                out.insert(id.to_string(), window);
            }
        }
    }
    (!out.is_empty()).then_some(out)
}

/// Refreshes the table in the background — never on the path that renders a
/// pane. Any signed-in account can answer; the windows are not per-account.
pub fn refresh_soon() {
    std::thread::spawn(|| loop {
        let due = cache().lock().map(|c| now_ms().saturating_sub(c.0) > REFRESH_MS).unwrap_or(false);
        if due {
            let token = crate::accounts::list_accounts()
                .unwrap_or_default()
                .into_iter()
                .find_map(|a| read_token(&a.path));

            if let Some(map) = token.as_deref().and_then(fetch) {
                crate::log::info("models", &format!("context windows refreshed for {} models", map.len()));
                if let Ok(mut c) = cache().lock() {
                    *c = (now_ms(), map);
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(600));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_matches_the_published_windows() {
        assert_eq!(fallback("claude-opus-5"), 1_000_000.0);
        assert_eq!(fallback("claude-opus-4-8"), 1_000_000.0);
        assert_eq!(fallback("claude-opus-4-6"), 1_000_000.0);
        assert_eq!(fallback("claude-opus-4-5-20251101"), 200_000.0);
        assert_eq!(fallback("claude-sonnet-5"), 1_000_000.0);
        assert_eq!(fallback("claude-sonnet-4-6"), 1_000_000.0);
        assert_eq!(fallback("claude-fable-5"), 1_000_000.0);
        assert_eq!(fallback("claude-haiku-4-5-20251001"), 200_000.0);
    }

    #[test]
    fn a_date_is_not_a_version() {
        assert_eq!(version_after("claude-3-5-sonnet-20241022", "sonnet"), None);
        assert_eq!(version_after("claude-opus-4-6", "opus"), Some((4, 6)));
        assert_eq!(version_after("claude-opus-5", "opus"), Some((5, 0)));
    }
}
