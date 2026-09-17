//! Rate limits of a ChatGPT subscription, the way Codex's own `/status` gets
//! them: `GET chatgpt.com/backend-api/wham/usage` with the access token from
//! `<CODEX_HOME>/auth.json`. Zero tokens. When the network or the token is
//! not there, the last `token_count` line of a rollout has the same numbers
//! as of the last turn.

use crate::throttle::{self, now_ms};
use base64::Engine;
use serde::Serialize;
use serde_json::Value;
use std::path::Path;

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
pub const RATE_429: &str = "rate-limited";

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct LimitWindow {
    /// `primary` | `secondary` | the metered feature of an additional limit.
    pub id: String,
    /// "5 hours", "week", or the limit's own name.
    pub label: String,
    /// 0..1
    pub used: f64,
    pub window_minutes: Option<i64>,
    /// ISO instant.
    pub resets_at: Option<String>,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CodexLimits {
    /// `auth.json` holds ChatGPT tokens. The access token expiring is routine
    /// — Codex renews it from the refresh token on next use.
    pub signed_in: bool,
    pub email: Option<String>,
    pub plan: Option<String>,

    pub have_usage: bool,
    pub windows: Vec<LimitWindow>,

    /// `network` | `rollout`, and when the numbers were taken.
    pub source: Option<String>,
    pub fetched_at_ms: Option<f64>,

    /// The stored access token has expired; Codex refreshes it on next use.
    pub stale: bool,
    /// The usage endpoint throttled us; numbers above are the last known.
    pub rate_limited: Option<u64>,
}

struct Auth {
    access: String,
    account_id: Option<String>,
    email: Option<String>,
    plan: Option<String>,
    expired: bool,
}

/// Codex rewrites `auth.json` in place (truncate + write, no rename), so a
/// reader can catch it half-written; a short retry rides that out.
fn read_auth_json(account_path: &str) -> Option<Value> {
    let path = Path::new(account_path).join("auth.json");
    for attempt in 0..3 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let Ok(text) = std::fs::read_to_string(&path) else { return None };
        if let Ok(v) = serde_json::from_str::<Value>(&text) {
            return Some(v);
        }
    }
    None
}

/// The claims of a JWT, unverified — this is our own token, read for the
/// email and plan it carries, not for trust.
fn jwt_claims(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn plan_label(plan_type: &str) -> Option<String> {
    let p = match plan_type.trim() {
        "" | "unknown" => return None,
        "plus" => "Plus",
        "pro" => "Pro",
        "prolite" => "Pro Lite",
        "team" => "Team",
        "business" => "Business",
        "enterprise" => "Enterprise",
        "free" => "Free",
        "go" => "Go",
        "edu" => "Edu",
        other => return Some(other.replace('_', " ")),
    };
    Some(p.into())
}

fn read_auth(account_path: &str) -> Option<Auth> {
    let v = read_auth_json(account_path)?;
    let tokens = &v["tokens"];
    let access = tokens["access_token"].as_str()?.to_string();
    let claims = tokens["id_token"].as_str().and_then(jwt_claims).unwrap_or(Value::Null);
    let auth_ns = &claims["https://api.openai.com/auth"];
    let expired = jwt_claims(&access)
        .and_then(|c| c["exp"].as_u64())
        .map(|exp| now_ms() / 1000 >= exp)
        .unwrap_or(false);
    Some(Auth {
        access,
        account_id: tokens["account_id"]
            .as_str()
            .or_else(|| auth_ns["chatgpt_account_id"].as_str())
            .map(str::to_owned),
        email: claims["email"]
            .as_str()
            .or_else(|| claims["https://api.openai.com/profile"]["email"].as_str())
            .map(str::to_owned),
        plan: auth_ns["chatgpt_plan_type"].as_str().and_then(plan_label),
        expired,
    })
}

pub fn window_label(minutes: Option<i64>) -> String {
    match minutes {
        Some(m) if m == 300 => "5 hours".into(),
        Some(m) if m == 10080 => "week".into(),
        Some(m) if m > 0 && m % 1440 == 0 => format!("{} days", m / 1440),
        Some(m) if m > 0 && m % 60 == 0 => format!("{} hours", m / 60),
        Some(m) if m > 0 => format!("{m} min"),
        _ => "limit".into(),
    }
}

fn iso(secs: Option<i64>) -> Option<String> {
    chrono::DateTime::from_timestamp(secs?, 0).map(|t| t.to_rfc3339())
}

/// One window of the `wham/usage` response.
fn window_from_api(id: &str, label: Option<&str>, w: &Value) -> Option<LimitWindow> {
    if !w.is_object() {
        return None;
    }
    let secs = w["limit_window_seconds"].as_i64();
    let minutes = secs.filter(|s| *s > 0).map(|s| (s + 59) / 60);
    Some(LimitWindow {
        id: id.into(),
        label: label.map(str::to_owned).unwrap_or_else(|| window_label(minutes)),
        used: w["used_percent"].as_f64().unwrap_or(0.0) / 100.0,
        window_minutes: minutes,
        resets_at: iso(w["reset_at"].as_i64()),
    })
}

/// Windows and plan out of a `wham/usage` response body.
pub fn windows_from_api(body: &Value) -> (Vec<LimitWindow>, Option<String>) {
    let mut out = vec![];
    let rl = &body["rate_limit"];
    out.extend(window_from_api("primary", None, &rl["primary_window"]));
    out.extend(window_from_api("secondary", None, &rl["secondary_window"]));
    if let Some(extra) = body["additional_rate_limits"].as_array() {
        for lim in extra {
            let feature = lim["metered_feature"].as_str().unwrap_or("extra");
            let name = lim["limit_name"]
                .as_str()
                .or_else(|| lim["normal_model_slug"].as_str())
                .unwrap_or(feature);
            let r = &lim["rate_limit"];
            if let Some(mut w) = window_from_api(feature, Some(name), &r["primary_window"]) {
                let wl = window_label(w.window_minutes);
                w.label = format!("{name} · {wl}");
                out.push(w);
            }
            if let Some(mut w) = window_from_api(&format!("{feature}:secondary"), Some(name), &r["secondary_window"]) {
                let wl = window_label(w.window_minutes);
                w.label = format!("{name} · {wl}");
                out.push(w);
            }
        }
    }
    (out, body["plan_type"].as_str().and_then(plan_label))
}

/// Windows and plan out of a rollout's `rate_limits` snapshot.
pub fn windows_from_rollout(snapshot: &Value) -> (Vec<LimitWindow>, Option<String>) {
    let mut out = vec![];
    for (id, key) in [("primary", "primary"), ("secondary", "secondary")] {
        let w = &snapshot[key];
        if !w.is_object() {
            continue;
        }
        let minutes = w["window_minutes"].as_i64();
        out.push(LimitWindow {
            id: id.into(),
            label: window_label(minutes),
            used: w["used_percent"].as_f64().unwrap_or(0.0) / 100.0,
            window_minutes: minutes,
            resets_at: iso(w["resets_at"].as_i64()),
        });
    }
    (out, snapshot["plan_type"].as_str().and_then(plan_label))
}

fn get_usage(auth: &Auth) -> Result<Value, String> {
    let mut req = ureq::builder()
        .timeout_connect(std::time::Duration::from_secs(5))
        .timeout_read(std::time::Duration::from_secs(10))
        .build()
        .get(USAGE_URL)
        .set("Authorization", &format!("Bearer {}", auth.access))
        .set("User-Agent", "codex_cli_rs")
        .set("originator", "codex_cli_rs")
        .set("Accept", "application/json");
    if let Some(id) = &auth.account_id {
        req = req.set("ChatGPT-Account-Id", id);
    }
    let resp = req.call().map_err(|e| match e {
        ureq::Error::Status(401, _) => "token expired — run a session to refresh".to_string(),
        ureq::Error::Status(429, r) => format!(
            "{RATE_429}:{}",
            r.header("retry-after").and_then(|h| h.trim().parse::<u64>().ok()).unwrap_or(0)
        ),
        other => other.to_string(),
    })?;
    resp.into_json().map_err(|e| e.to_string())
}

fn from_rollout(out: &mut CodexLimits, account_path: &str) {
    let Some((snapshot, ts)) = super::session::latest_rate_limits(account_path) else { return };
    let (windows, plan) = windows_from_rollout(&snapshot);
    if windows.is_empty() {
        return;
    }
    out.windows = windows;
    out.have_usage = true;
    out.source = Some("rollout".into());
    out.fetched_at_ms = chrono::DateTime::parse_from_rfc3339(&ts)
        .ok()
        .map(|t| t.timestamp_millis() as f64);
    if out.plan.is_none() {
        out.plan = plan;
    }
}

/// Async so the blocking HTTP work leaves the main thread free.
#[tauri::command]
pub async fn codex_limits(account_path: String) -> Result<CodexLimits, String> {
    tauri::async_runtime::spawn_blocking(move || fetch_limits(&account_path))
        .await
        .map_err(|e| e.to_string())?
}

fn fetch_limits(account_path: &str) -> Result<CodexLimits, String> {
    let mut out = CodexLimits::default();
    let Some(auth) = read_auth(account_path) else {
        return Ok(out);
    };
    out.signed_in = true;
    out.email = auth.email.clone();
    out.plan = auth.plan.clone();

    if auth.expired {
        out.stale = true;
        from_rollout(&mut out, account_path);
        return Ok(out);
    }

    if let Some(secs) = throttle::remaining(account_path) {
        out.rate_limited = Some(secs);
        from_rollout(&mut out, account_path);
        return Ok(out);
    }

    match get_usage(&auth) {
        Ok(body) => {
            let (windows, plan) = windows_from_api(&body);
            if plan.is_some() {
                out.plan = plan;
            }
            out.windows = windows;
            out.have_usage = !out.windows.is_empty();
            out.source = Some("network".into());
            out.fetched_at_ms = Some(now_ms() as f64);
            throttle::clear(account_path);
        }
        Err(e) if e.starts_with(RATE_429) => {
            let secs: u64 = e.rsplit(':').next().and_then(|s| s.parse().ok()).unwrap_or(0);
            out.rate_limited = Some(throttle::hit("codex-limits", account_path, secs));
            from_rollout(&mut out, account_path);
        }
        Err(e) => {
            crate::log::warn("codex-limits", &format!("usage failed for {account_path}: {e}"));
            from_rollout(&mut out, account_path);
            if !out.have_usage {
                return Err(e);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_the_usage_response() {
        let body: Value = serde_json::from_str(
            r#"{"plan_type":"plus",
                "rate_limit":{"allowed":true,"limit_reached":false,
                  "primary_window":{"used_percent":37,"limit_window_seconds":604800,"reset_after_seconds":1000,"reset_at":1789000000},
                  "secondary_window":null},
                "additional_rate_limits":[{"limit_name":"GPT-5 Codex Spark","metered_feature":"codex_spark",
                  "rate_limit":{"primary_window":{"used_percent":5,"limit_window_seconds":18000,"reset_at":1789001000}}}]}"#,
        )
        .unwrap();
        let (w, plan) = windows_from_api(&body);
        assert_eq!(plan.as_deref(), Some("Plus"));
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].id, "primary");
        assert_eq!(w[0].label, "week");
        assert!((w[0].used - 0.37).abs() < 1e-9);
        assert_eq!(w[0].window_minutes, Some(10080));
        assert!(w[0].resets_at.as_deref().unwrap().starts_with("2026-"));
        assert_eq!(w[1].id, "codex_spark");
        assert_eq!(w[1].label, "GPT-5 Codex Spark · 5 hours");
    }

    #[test]
    fn maps_a_rollout_snapshot() {
        let snap: Value = serde_json::from_str(
            r#"{"primary":{"used_percent":12.5,"window_minutes":300,"resets_at":1789000000},"secondary":{"used_percent":40,"window_minutes":10080,"resets_at":null},"plan_type":"pro"}"#,
        )
        .unwrap();
        let (w, plan) = windows_from_rollout(&snap);
        assert_eq!(plan.as_deref(), Some("Pro"));
        assert_eq!(w[0].label, "5 hours");
        assert_eq!(w[1].label, "week");
        assert_eq!(w[1].resets_at, None);
    }

    #[test]
    fn decodes_jwt_claims() {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"email":"a@b.c","exp":1700000000,"https://api.openai.com/auth":{"chatgpt_plan_type":"plus"}}"#);
        let token = format!("hdr.{payload}.sig");
        let c = jwt_claims(&token).unwrap();
        assert_eq!(c["email"], "a@b.c");
        assert_eq!(c["https://api.openai.com/auth"]["chatgpt_plan_type"], "plus");
        assert!(jwt_claims("nonsense").is_none());
    }

    #[test]
    fn labels_windows() {
        assert_eq!(window_label(Some(300)), "5 hours");
        assert_eq!(window_label(Some(10080)), "week");
        assert_eq!(window_label(Some(2880)), "2 days");
        assert_eq!(window_label(Some(90)), "90 min");
        assert_eq!(window_label(None), "limit");
    }
}
