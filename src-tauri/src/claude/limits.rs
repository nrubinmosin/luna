use super::oauth;
use crate::throttle::{self, now_ms};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::{Mutex, OnceLock};

const BETA_HEADER: &str = "oauth-2025-04-20";
const RATE_401: &str = "token rejected (401)";
pub const RATE_429: &str = "rate-limited";
/// The request never reached the server: no network, DNS down, the machine
/// just woke up. Says nothing about the account, so it is not logged.
const OFFLINE: &str = "offline";

/// How old the CLI's own cached usage may be before we bother the network.
const CACHE_FRESH_MS: u64 = 5 * 60 * 1000;
/// How long Luna's own last answer from the endpoint stands in for a new one.
/// The CLI's cache only moves while a session runs on the account, so an idle
/// account used to go to the network on every one-minute poll — and the
/// endpoint answered that with a 429 roughly every hour, per account, for as
/// long as Luna was open. Idle numbers barely move; a click on refresh still
/// asks right away.
const NETWORK_EVERY_MS: u64 = 10 * 60 * 1000;

/// The last `limits` the endpoint returned, per account path, and when.
fn last_answer() -> &'static Mutex<HashMap<String, (u64, Value)>> {
    static LAST: OnceLock<Mutex<HashMap<String, (u64, Value)>>> = OnceLock::new();
    LAST.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AccountLimits {
    /// Credentials are present and the refresh token has not expired. The
    /// access token expiring is routine — Luna or the CLI renews it — so it is
    /// not a sign-out and must not be reported as one.
    pub signed_in: bool,
    pub email: Option<String>,
    pub plan: Option<String>,

    /// False when no usage numbers are known at all, so the UI can show "—"
    /// rather than a row of confident-looking zeroes.
    pub have_usage: bool,
    /// The session window ("5 hours") and the week across all models, each
    /// None when the answer had no row for it: a window the server did not
    /// send is absent, not at 0%.
    pub h5: Option<f64>,
    pub week: Option<f64>,
    pub reset_h5: Option<String>,
    pub reset_week: Option<String>,
    /// Every weekly limit scoped to one model (Fable, Opus, ...), each with
    /// its own reset. One field used to hold them all, so the last row won.
    pub scoped: Vec<ScopedLimit>,
    /// Rows of a kind Luna does not know, under the server's own label, so a
    /// new or renamed window shows instead of vanishing.
    pub other: Vec<OtherLimit>,

    /// Where the numbers came from, and when they were taken.
    pub source: Option<String>,
    pub fetched_at_ms: Option<f64>,

    /// The stored access token has expired and could not be renewed just now.
    pub stale: bool,
    /// Why not, when the renewal was tried and failed.
    pub refresh_error: Option<String>,
    /// The usage endpoint throttled us. Any numbers above came from the cache.
    pub rate_limited: Option<u64>,
}

#[derive(Serialize, Default, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ScopedLimit {
    /// What the limit is scoped to, as the endpoint names it ("Fable").
    pub label: String,
    pub used: f64,
    pub resets_at: Option<String>,
}

#[derive(Serialize, Default, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OtherLimit {
    /// The server's meter kind, which Luna has no meaning for.
    pub kind: String,
    /// What the server calls the row: its scope's display name, its group,
    /// both, or failing those its kind.
    pub label: String,
    pub used: f64,
    pub resets_at: Option<String>,
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

struct Token {
    access: String,
    expired: bool,
}

fn read_token(account_path: &str) -> Result<Token, String> {
    let v =
        read_json(&Path::new(account_path).join(".credentials.json")).ok_or_else(|| "not logged in (no .credentials.json)".to_string())?;
    let oauth = &v["claudeAiOauth"];
    let access = oauth["accessToken"].as_str().map(str::to_owned).ok_or_else(|| "no access token in credentials".to_string())?;
    let expired = oauth["expiresAt"].as_u64().map(|ms| now_ms() >= ms).unwrap_or(false);
    Ok(Token { access, expired })
}

/// Being signed in is about the *refresh* token: while it lives, a new access
/// token can be minted without the user doing anything.
fn signed_in(account_path: &str) -> bool {
    read_json(&Path::new(account_path).join(".credentials.json"))
        .map(|v| {
            let o = &v["claudeAiOauth"];
            o["refreshToken"].is_string() && o["refreshTokenExpiresAt"].as_u64().map(|ms| now_ms() < ms).unwrap_or(true)
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

/// What the server calls a row of a kind Luna does not know: its scope's
/// display name and its group ("Fable · monthly"), whichever it has, else its
/// kind. A second row of the same name gets its kind too, so that each window
/// keeps a name of its own: the quota notices tell windows apart by it.
fn own_label(lim: &Value, kind: &str, taken: &[OtherLimit]) -> String {
    let scope = &lim["scope"];
    let name = scope["model"]["display_name"].as_str().or_else(|| scope["surface"]["display_name"].as_str()).filter(|n| !n.is_empty());
    let group = lim["group"].as_str().filter(|g| !g.is_empty());
    let label = match (name, group) {
        (Some(n), Some(g)) => format!("{n} · {g}"),
        (Some(one), None) | (None, Some(one)) => one.to_string(),
        (None, None) if kind.is_empty() => "limit".to_string(),
        (None, None) => kind.to_string(),
    };
    if !kind.is_empty() && taken.iter().any(|o| o.label == label) {
        format!("{label} ({kind})")
    } else {
        label
    }
}

/// Whether an unknown kind is yet to be logged: once a run, as every poll
/// brings it again.
fn kind_is_news(kind: &str) -> bool {
    static SAID: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
    SAID.lock().unwrap_or_else(|e| e.into_inner()).insert(kind.to_string())
}

/// Copies a `limits` array — the shape is identical in the API response and in
/// the CLI's on-disk cache — into the DTO. Each answer stands alone: a window
/// an earlier one had and this one lacks is gone, not kept from before.
fn apply_limits(out: &mut AccountLimits, limits: &Value) {
    let Some(arr) = limits.as_array() else { return };
    (out.h5, out.reset_h5, out.week, out.reset_week) = (None, None, None, None);
    out.scoped.clear();
    out.other.clear();
    for lim in arr {
        // A row without its share is not a window at 0%.
        let Some(pct) = lim["percent"].as_f64().map(|p| p / 100.0) else { continue };
        let reset = lim["resets_at"].as_str().map(str::to_owned);
        match lim["kind"].as_str().unwrap_or("") {
            "session" => {
                out.h5 = Some(pct);
                out.reset_h5 = reset;
            }
            "weekly_all" => {
                out.week = Some(pct);
                out.reset_week = reset;
            }
            "weekly_scoped" => {
                let scope = &lim["scope"];
                let label = scope["model"]["display_name"]
                    .as_str()
                    .or_else(|| scope["surface"]["display_name"].as_str())
                    .unwrap_or("model")
                    .to_string();
                out.scoped.push(ScopedLimit { label, used: pct, resets_at: reset });
            }
            kind => {
                let label = own_label(lim, kind, &out.other);
                if kind_is_news(kind) {
                    crate::log::info(
                        "limits",
                        &format!("the usage endpoint sent a row of kind {kind:?}, unknown to Luna; shown as {label:?}"),
                    );
                }
                out.other.push(OtherLimit { kind: kind.to_string(), label, used: pct, resets_at: reset });
            }
        }
    }
    out.have_usage = true;
}

fn get_json(url: &str, token: &str) -> Result<Value, String> {
    // Without these a stalled connection hangs the refresh indefinitely. The
    // read timeout is per read and the connect one per address, so only the
    // overall one bounds the request as a whole.
    let resp = ureq::builder()
        .timeout_connect(std::time::Duration::from_secs(5))
        .timeout_read(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(15))
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
                format!("{RATE_429}:{}", r.header("retry-after").and_then(|h| h.trim().parse::<u64>().ok()).unwrap_or(0))
            }
            ureq::Error::Transport(t) => format!("{OFFLINE}: {t}"),
            other => other.to_string(),
        })?;
    resp.into_json().map_err(|e| e.to_string())
}

/// Async so the blocking HTTP work leaves the main thread free. `force` is a
/// click on refresh: it looks past a fresh cache and a 429 cool-off.
#[tauri::command]
pub async fn account_limits(account_path: crate::boundary::AccountPath, force: Option<bool>) -> Result<AccountLimits, String> {
    let (account_path, force) = (String::from(account_path), force.unwrap_or(false));
    tauri::async_runtime::spawn_blocking(move || fetch_limits(&account_path, force)).await.map_err(|e| e.to_string())?
}

fn fetch_limits(account_path: &str, force: bool) -> Result<AccountLimits, String> {
    let mut out = AccountLimits { signed_in: signed_in(account_path), ..Default::default() };

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
                if !force && now_ms().saturating_sub(fetched) < CACHE_FRESH_MS {
                    return Ok(out);
                }
            }
        }
    }

    // Luna's own last answer, when it is newer than what the CLI cached.
    let remembered = last_answer().lock().ok().and_then(|m| m.get(account_path).cloned());
    if let Some((at, limits)) = remembered {
        if out.fetched_at_ms.is_none_or(|cached| (at as f64) > cached) {
            apply_limits(&mut out, &limits);
            out.source = Some("network".into());
            out.fetched_at_ms = Some(at as f64);
            if !force && now_ms().saturating_sub(at) < NETWORK_EVERY_MS {
                return Ok(out);
            }
        }
    }

    if !out.signed_in {
        return Ok(out);
    }

    // Inside a 429 cool-off: report the remaining hold instead of poking the
    // endpoint again. The frontend folds this into its own retry cadence.
    if !force {
        if let Some(secs) = throttle::remaining(account_path) {
            out.rate_limited = Some(secs);
            return Ok(out);
        }
    }

    let mut token = match read_token(account_path) {
        Ok(t) => t,
        // Cached numbers beat failing the whole read over a credentials hiccup.
        Err(e) if out.have_usage => {
            crate::log::warn("limits", &format!("{account_path}: {e}"));
            return Ok(out);
        }
        Err(e) => return Err(e),
    };
    if token.expired {
        match oauth::refresh(account_path, force) {
            oauth::Outcome::Fresh => token = read_token(account_path)?,
            // A CLI is renewing it right now; the next round has the result.
            oauth::Outcome::Busy => {}
            oauth::Outcome::Rejected => {
                out.signed_in = false;
                out.refresh_error = Some("sign-in expired — log in again".into());
                return Ok(out);
            }
            oauth::Outcome::Failed(e) => out.refresh_error = Some(e),
        }
        if token.expired {
            out.stale = true;
            return Ok(out);
        }
    }

    match get_json("https://api.anthropic.com/api/oauth/usage", &token.access) {
        Ok(usage) => {
            let at = now_ms();
            apply_limits(&mut out, &usage["limits"]);
            out.source = Some("network".into());
            out.fetched_at_ms = Some(at as f64);
            throttle::clear(account_path);
            if let Ok(mut m) = last_answer().lock() {
                m.insert(account_path.to_string(), (at, usage["limits"].clone()));
            }
        }
        Err(e) if e.starts_with(RATE_429) => {
            let secs: u64 = e.rsplit(':').next().and_then(|s| s.parse().ok()).unwrap_or(0);
            out.rate_limited = Some(throttle::hit("limits", account_path, secs));
        }
        // Every account fails at once, on every poll, until the network is
        // back; the row already says it has no fresh numbers.
        Err(e) if e.starts_with(OFFLINE) => {}
        Err(e) => {
            crate::log::warn("limits", &format!("usage failed for {account_path}: {e}"));
            if !out.have_usage {
                return Err(e);
            }
        }
    }

    Ok(out)
}

#[cfg(test)]
mod scoped_tests {
    use super::*;

    #[test]
    fn every_scoped_limit_is_kept_with_its_own_reset() {
        let limits: Value = serde_json::from_str(
            r#"[
              {"kind":"session","percent":12,"resets_at":"2026-10-15T05:00:00Z"},
              {"kind":"weekly_all","percent":41,"resets_at":"2026-10-19T21:00:00Z"},
              {"kind":"weekly_scoped","percent":63,"resets_at":"2026-10-19T21:00:00Z",
               "scope":{"model":{"display_name":"Fable"}}},
              {"kind":"weekly_scoped","percent":5,"resets_at":"2026-10-20T09:00:00Z",
               "scope":{"model":{"display_name":"Opus"}}}
            ]"#,
        )
        .unwrap();
        let mut out = AccountLimits::default();
        apply_limits(&mut out, &limits);
        assert!(out.have_usage);
        assert_eq!(out.h5, Some(0.12));
        assert_eq!(out.week, Some(0.41));
        assert_eq!(out.scoped.len(), 2);
        assert_eq!(out.scoped[0].label, "Fable");
        assert_eq!(out.scoped[0].used, 0.63);
        assert_eq!(out.scoped[1].label, "Opus");
        assert_eq!(out.scoped[1].resets_at.as_deref(), Some("2026-10-20T09:00:00Z"));
        assert!(out.other.is_empty());
    }

    /// The answer as the endpoint gave it on 2026-11-09: weekly rows, no
    /// `session` row.
    #[test]
    fn a_window_the_answer_lacks_is_absent_not_zero() {
        let limits: Value = serde_json::from_str(
            r#"[
              {"kind":"weekly_all","group":"weekly","percent":41,"resets_at":"2026-10-19T21:00:00Z"},
              {"kind":"weekly_scoped","group":"weekly","percent":63,"resets_at":"2026-10-19T21:00:00Z",
               "scope":{"model":{"display_name":"Fable"}}}
            ]"#,
        )
        .unwrap();
        let mut out = AccountLimits::default();
        apply_limits(&mut out, &limits);
        assert_eq!((out.h5, out.reset_h5.as_deref()), (None, None));
        assert_eq!(out.week, Some(0.41));
        let json = serde_json::to_value(&out).unwrap();
        assert_eq!(json["h5"], Value::Null, "{json}");

        // A session window an earlier answer had (the CLI's cache) is gone
        // once a newer answer lacks it.
        let mut out = AccountLimits::default();
        apply_limits(&mut out, &serde_json::json!([{ "kind": "session", "percent": 12, "resets_at": null }]));
        assert_eq!(out.h5, Some(0.12));
        apply_limits(&mut out, &limits);
        assert_eq!(out.h5, None);

        // A row without its share is no window either.
        apply_limits(&mut out, &serde_json::json!([{ "kind": "weekly_all", "resets_at": null }]));
        assert_eq!((out.week, out.have_usage), (None, true));
    }

    #[test]
    fn a_row_of_an_unknown_kind_is_kept_under_the_servers_label() {
        let limits: Value = serde_json::from_str(
            r#"[
              {"kind":"weekly_all","group":"weekly","percent":41,"resets_at":"2026-10-19T21:00:00Z"},
              {"kind":"monthly_all","group":"monthly","percent":7,"resets_at":"2026-11-30T00:00:00Z"},
              {"kind":"monthly_scoped","group":"monthly","percent":30,"resets_at":null,
               "scope":{"model":{"display_name":"Fable"}}},
              {"kind":"daily_surface","percent":2,"resets_at":null,"scope":{"surface":{"display_name":"Claude Code"}}},
              {"kind":"burst","percent":55,"resets_at":null},
              {"kind":"monthly_extra","group":"monthly","percent":1,"resets_at":null}
            ]"#,
        )
        .unwrap();
        let mut out = AccountLimits::default();
        apply_limits(&mut out, &limits);
        let rows: Vec<(&str, &str, f64)> = out.other.iter().map(|o| (o.kind.as_str(), o.label.as_str(), o.used)).collect();
        assert_eq!(
            rows,
            [
                ("monthly_all", "monthly", 0.07),
                ("monthly_scoped", "Fable · monthly", 0.3),
                ("daily_surface", "Claude Code", 0.02),
                ("burst", "burst", 0.55),
                ("monthly_extra", "monthly (monthly_extra)", 0.01),
            ]
        );
        assert_eq!(out.other[0].resets_at.as_deref(), Some("2026-11-30T00:00:00Z"));
        assert_eq!(out.week, Some(0.41), "the known rows as ever");
        assert_eq!(out.h5, None);
    }

    #[test]
    fn an_unknown_kind_is_logged_once() {
        assert!(kind_is_news("ul-invented-kind"));
        assert!(!kind_is_news("ul-invented-kind"), "every poll brings it again");
        assert!(kind_is_news("ul-another-kind"));
    }
}
