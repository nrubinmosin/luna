use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

const BETA_HEADER: &str = "oauth-2025-04-20";
const RATE_401: &str = "token expired — run a session to refresh";
pub const RATE_429: &str = "rate-limited";

/// How old the CLI's own cached usage may be before we bother the network.
const CACHE_FRESH_MS: u64 = 5 * 60 * 1000;

/// The usage endpoint answers a throttled client with 429 and retry-after 0,
/// i.e. it never says how long the sentence is — and observed behaviour is
/// that coming back a few minutes later just earns the next 429. Sit out a
/// full cool-off after one, without a request and without another log line;
/// the UI keeps the last real numbers (and the CLI's cache keeps refreshing
/// them whenever a session runs) so nothing on screen goes dark meanwhile.
const THROTTLE_COOLOFF_MS: u64 = 15 * 60 * 1000;

/// Per-account "leave the endpoint alone until" deadline, ms since the epoch.
fn throttled_until() -> &'static Mutex<HashMap<String, u64>> {
    static MAP: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AccountLimits {
    /// Credentials are present and the refresh token has not expired. The
    /// access token expiring is routine — the CLI renews it — so it is not a
    /// sign-out and must not be reported as one.
    pub signed_in: bool,
    pub email: Option<String>,
    pub plan: Option<String>,

    /// False when no usage numbers are known at all, so the UI can show "—"
    /// rather than a row of confident-looking zeroes.
    pub have_usage: bool,
    pub h5: f64,
    pub week: f64,
    pub model: f64,
    pub reset_h5: Option<String>,
    pub reset_week: Option<String>,
    pub reset_model: Option<String>,

    /// Where the numbers came from, and when they were taken.
    pub source: Option<String>,
    pub fetched_at_ms: Option<f64>,

    /// The stored access token has expired; the CLI refreshes it on next use.
    pub stale: bool,
    /// The usage endpoint throttled us. Any numbers above came from the cache.
    pub rate_limited: Option<u64>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

struct Token {
    access: String,
    expired: bool,
}

fn read_token(account_path: &str) -> Result<Token, String> {
    let v = read_json(&Path::new(account_path).join(".credentials.json"))
        .ok_or_else(|| "not logged in (no .credentials.json)".to_string())?;
    let oauth = &v["claudeAiOauth"];
    let access = oauth["accessToken"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "no access token in credentials".to_string())?;
    let expired = oauth["expiresAt"].as_u64().map(|ms| now_ms() >= ms).unwrap_or(false);
    Ok(Token { access, expired })
}

/// Being signed in is about the *refresh* token: while it lives, the CLI can
/// mint a new access token without the user doing anything.
fn signed_in(account_path: &str) -> bool {
    read_json(&Path::new(account_path).join(".credentials.json"))
        .map(|v| {
            let o = &v["claudeAiOauth"];
            o["refreshToken"].is_string()
                && o["refreshTokenExpiresAt"].as_u64().map(|ms| now_ms() < ms).unwrap_or(true)
        })
        .unwrap_or(false)
}

fn plan_from_tier(tier: &str) -> Option<String> {
    if let Some(mult) = tier.strip_prefix("default_claude_max_") {
        return Some(format!("Max {}", mult.replace('x', "×")));
    }
    match tier {
        "" => None,
        "default_claude_pro" => Some("Pro".into()),
        other => Some(other.to_string()),
    }
}

/// Copies a `limits` array — the shape is identical in the API response and in
/// the CLI's on-disk cache — into the DTO.
fn apply_limits(out: &mut AccountLimits, limits: &Value) {
    let Some(arr) = limits.as_array() else { return };
    for lim in arr {
        let pct = lim["percent"].as_f64().unwrap_or(0.0) / 100.0;
        let reset = lim["resets_at"].as_str().map(str::to_owned);
        match lim["kind"].as_str().unwrap_or("") {
            "session" => {
                out.h5 = pct;
                out.reset_h5 = reset;
            }
            "weekly_all" => {
                out.week = pct;
                out.reset_week = reset;
            }
            "weekly_scoped" => {
                out.model = pct;
                out.reset_model = reset;
            }
            _ => {}
        }
    }
    out.have_usage = true;
}

fn get_json(url: &str, token: &str) -> Result<Value, String> {
    // Without these a stalled connection hangs the refresh indefinitely.
    let resp = ureq::builder()
        .timeout_connect(std::time::Duration::from_secs(5))
        .timeout_read(std::time::Duration::from_secs(10))
        .build()
        .get(url)
        .set("Authorization", &format!("Bearer {token}"))
        .set("anthropic-beta", BETA_HEADER)
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(401, _) => RATE_401.to_string(),
            // Polling usage too eagerly gets the endpoint throttled; the caller
            // turns this into a backoff rather than another immediate retry.
            ureq::Error::Status(429, r) => format!(
                "{RATE_429}:{}",
                r.header("retry-after").and_then(|h| h.trim().parse::<u64>().ok()).unwrap_or(0)
            ),
            other => other.to_string(),
        })?;
    resp.into_json().map_err(|e| e.to_string())
}

/// Async so the blocking HTTP work leaves the main thread free.
#[tauri::command]
pub async fn account_limits(account_path: String) -> Result<AccountLimits, String> {
    tauri::async_runtime::spawn_blocking(move || fetch_limits(&account_path))
        .await
        .map_err(|e| e.to_string())?
}

fn fetch_limits(account_path: &str) -> Result<AccountLimits, String> {
    let mut out = AccountLimits {
        signed_in: signed_in(account_path),
        ..Default::default()
    };

    // Identity and plan come from the config the CLI wrote at login: no
    // request, and still correct when the network is unavailable.
    if let Some(cfg) = read_json(&Path::new(account_path).join(".claude.json")) {
        let acc = &cfg["oauthAccount"];
        out.email = acc["emailAddress"].as_str().map(str::to_owned);
        out.plan = plan_from_tier(acc["organizationRateLimitTier"].as_str().unwrap_or(""));

        // The CLI refreshes this cache whenever a session runs — which is
        // exactly when the numbers change. Reading it costs no request, so it
        // cannot be throttled, and it survives the usage endpoint being down.
        let cached = &cfg["cachedUsageUtilization"];
        if let Some(fetched) = cached["fetchedAtMs"].as_u64() {
            apply_limits(&mut out, &cached["utilization"]["limits"]);
            if out.have_usage {
                out.source = Some("cache".into());
                out.fetched_at_ms = Some(fetched as f64);
                if now_ms().saturating_sub(fetched) < CACHE_FRESH_MS {
                    return Ok(out);
                }
            }
        }
    }

    if !out.signed_in {
        return Ok(out);
    }

    // Inside a 429 cool-off: report the remaining hold instead of poking the
    // endpoint again. The frontend folds this into its own retry cadence.
    if let Some(&until) = throttled_until().lock().unwrap().get(account_path) {
        let now = now_ms();
        if now < until {
            out.rate_limited = Some(((until - now) / 1000).max(1));
            return Ok(out);
        }
    }

    let token = match read_token(account_path) {
        Ok(t) => t,
        // Cached numbers beat failing the whole read over a credentials hiccup.
        Err(e) if out.have_usage => {
            crate::log::warn("limits", &format!("{account_path}: {e}"));
            return Ok(out);
        }
        Err(e) => return Err(e),
    };
    if token.expired {
        out.stale = true;
        return Ok(out);
    }

    match get_json("https://api.anthropic.com/api/oauth/usage", &token.access) {
        Ok(usage) => {
            apply_limits(&mut out, &usage["limits"]);
            out.source = Some("network".into());
            out.fetched_at_ms = Some(now_ms() as f64);
        }
        Err(e) if e.starts_with(RATE_429) => {
            let secs: u64 = e.rsplit(':').next().and_then(|s| s.parse().ok()).unwrap_or(0);
            // Honour a real retry-after if one ever appears, but never come
            // back sooner than the cool-off — retry-after 0 is what turned
            // this warning into hundreds of log lines a day.
            let cool_ms = (secs * 1000).max(THROTTLE_COOLOFF_MS);
            throttled_until()
                .lock()
                .unwrap()
                .insert(account_path.to_string(), now_ms() + cool_ms);
            crate::log::warn(
                "limits",
                &format!("429 for {account_path}, retry-after {secs}s — cooling off {}s", cool_ms / 1000),
            );
            out.rate_limited = Some(cool_ms / 1000);
        }
        Err(e) => {
            crate::log::warn("limits", &format!("usage failed for {account_path}: {e}"));
            if !out.have_usage {
                return Err(e);
            }
        }
    }

    Ok(out)
}
