use serde::Serialize;
use serde_json::Value;
use std::path::Path;

const BETA_HEADER: &str = "oauth-2025-04-20";
const RATE_401: &str = "token expired — run a session to refresh";
pub const RATE_429: &str = "rate-limited";

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AccountLimits {
    /// The stored access token has expired. The CLI refreshes it the moment a
    /// session runs, so this is a "wait a few seconds", not a "logged out".
    pub stale: bool,
    /// The usage endpoint throttled us. Seconds it asked us to wait, 0 if it
    /// gave no hint. The numbers below are then meaningless — keep the old ones.
    pub rate_limited: Option<u64>,
    pub h5: f64,
    pub week: f64,
    pub model: f64,
    pub reset_h5: Option<String>,
    pub reset_week: Option<String>,
    pub reset_model: Option<String>,
    pub plan: Option<String>,
    pub email: Option<String>,
}

struct Token {
    access: String,
    expired: bool,
}

fn read_token(account_path: &str) -> Result<Token, String> {
    let creds_path = Path::new(account_path).join(".credentials.json");
    let raw = std::fs::read_to_string(&creds_path)
        .map_err(|_| "not logged in (no .credentials.json)".to_string())?;
    let v: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    let oauth = &v["claudeAiOauth"];
    let access = oauth["accessToken"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "no access token in credentials".to_string())?;

    // Spending two doomed HTTP round-trips on a token we can see is expired is
    // what made a restart look like a logout.
    let expired = oauth["expiresAt"]
        .as_u64()
        .map(|ms| {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            now >= ms
        })
        .unwrap_or(false);

    Ok(Token { access, expired })
}

/// Seconds the server asked us to wait, if it did.
fn retry_after(resp: &ureq::Response) -> Option<u64> {
    resp.header("retry-after")?.trim().parse().ok()
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
            ureq::Error::Status(429, r) => {
                format!("{RATE_429}:{}", retry_after(&r).unwrap_or(0))
            }
            other => other.to_string(),
        })?;
    resp.into_json().map_err(|e| e.to_string())
}

fn plan_label(profile: &Value) -> Option<String> {
    let tier = profile["organization"]["rate_limit_tier"].as_str().unwrap_or("");
    if let Some(mult) = tier.strip_prefix("default_claude_max_") {
        return Some(format!("Max {}", mult.replace('x', "×")));
    }
    if profile["account"]["has_claude_max"].as_bool() == Some(true) {
        return Some("Max".into());
    }
    if profile["account"]["has_claude_pro"].as_bool() == Some(true) {
        return Some("Pro".into());
    }
    profile["organization"]["organization_type"].as_str().map(str::to_owned)
}

/// Async so the blocking HTTP work leaves the main thread free: these calls
/// used to run inline with `ensure_session`, so restoring several sessions at
/// startup waited on every account's usage request first.
#[tauri::command]
pub async fn account_limits(account_path: String) -> Result<AccountLimits, String> {
    tauri::async_runtime::spawn_blocking(move || fetch_limits(&account_path))
        .await
        .map_err(|e| e.to_string())?
}

fn fetch_limits(account_path: &str) -> Result<AccountLimits, String> {
    let token = read_token(account_path)?;
    if token.expired {
        return Ok(AccountLimits { stale: true, ..Default::default() });
    }
    let token = token.access;
    let usage = match get_json("https://api.anthropic.com/api/oauth/usage", &token) {
        Ok(v) => v,
        Err(e) if e.starts_with(RATE_429) => {
            let secs = e.rsplit(':').next().and_then(|s| s.parse().ok()).unwrap_or(0);
            return Ok(AccountLimits { rate_limited: Some(secs), ..Default::default() });
        }
        Err(e) => return Err(e),
    };

    let mut out = AccountLimits::default();

    if let Some(limits) = usage["limits"].as_array() {
        for lim in limits {
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
    }

    // Plan/email are cosmetic and effectively static — cache them so a usage
    // poll costs one request rather than two.
    if let Some((plan, email)) = cached_profile(account_path, &token) {
        out.plan = plan;
        out.email = email;
    }

    Ok(out)
}

type ProfileCache = std::collections::HashMap<String, (Option<String>, Option<String>)>;
static PROFILE: std::sync::OnceLock<std::sync::Mutex<ProfileCache>> = std::sync::OnceLock::new();

fn cached_profile(account_path: &str, token: &str) -> Option<(Option<String>, Option<String>)> {
    let cache = PROFILE.get_or_init(Default::default);
    if let Some(hit) = cache.lock().ok()?.get(account_path) {
        return Some(hit.clone());
    }
    // Failures here are not worth surfacing: the usage bars matter, the plan label does not.
    let profile = get_json("https://api.anthropic.com/api/oauth/profile", token).ok()?;
    let entry = (
        plan_label(&profile),
        profile["account"]["email"].as_str().map(str::to_owned),
    );
    cache.lock().ok()?.insert(account_path.to_string(), entry.clone());
    Some(entry)
}
