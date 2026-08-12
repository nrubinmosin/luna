use serde::Serialize;
use serde_json::Value;
use std::path::Path;

const BETA_HEADER: &str = "oauth-2025-04-20";

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AccountLimits {
    pub h5: f64,
    pub week: f64,
    pub model: f64,
    pub reset_h5: Option<String>,
    pub reset_week: Option<String>,
    pub reset_model: Option<String>,
    pub plan: Option<String>,
    pub email: Option<String>,
}

fn read_token(account_path: &str) -> Result<String, String> {
    let creds_path = Path::new(account_path).join(".credentials.json");
    let raw = std::fs::read_to_string(&creds_path)
        .map_err(|_| "not logged in (no .credentials.json)".to_string())?;
    let v: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    v["claudeAiOauth"]["accessToken"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "no access token in credentials".into())
}

fn get_json(url: &str, token: &str) -> Result<Value, String> {
    let resp = ureq::get(url)
        .set("Authorization", &format!("Bearer {token}"))
        .set("anthropic-beta", BETA_HEADER)
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(401, _) => "token expired — run a session to refresh".into(),
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

#[tauri::command]
pub fn account_limits(account_path: String) -> Result<AccountLimits, String> {
    let token = read_token(&account_path)?;
    let usage = get_json("https://api.anthropic.com/api/oauth/usage", &token)?;

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

    // Plan/email are cosmetic — ignore profile failures rather than lose the bars.
    if let Ok(profile) = get_json("https://api.anthropic.com/api/oauth/profile", &token) {
        out.plan = plan_label(&profile);
        out.email = profile["account"]["email"].as_str().map(str::to_owned);
    }

    Ok(out)
}
