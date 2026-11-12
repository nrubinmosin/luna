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
    /// The model it counts alone, where its scope names one (`model_scope`):
    /// what a usage limit's hold on a chat goes by, not shown.
    #[serde(skip)]
    pub model: Option<String>,
}

#[derive(Serialize, Default, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OtherLimit {
    /// The server's meter kind, which Luna has no meaning for.
    pub kind: String,
    /// What the server calls the row, with its kind (`own_label`).
    pub label: String,
    pub used: f64,
    pub resets_at: Option<String>,
    /// As `ScopedLimit::model`.
    #[serde(skip)]
    pub model: Option<String>,
}

/// The model a row counts alone, by its scope's model name; None for a row
/// scoped to no model or to a surface, which every model counts against.
fn model_scope(lim: &Value) -> Option<String> {
    lim["scope"]["model"]["display_name"].as_str().filter(|n| !n.trim().is_empty()).map(str::to_owned)
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

/// What a row of a kind Luna does not know is called: the server's name for
/// it (its scope's display name and its group, "Fable · monthly", whichever it
/// has) with its kind beside it, "monthly (monthly_all)", else its kind alone.
/// Made of the row alone, never of the rows around it: the quota notices know
/// a window by its label, so it must stay the same from one answer to the
/// next whatever else comes and goes, and never be a known window's.
fn own_label(lim: &Value, kind: &str) -> String {
    let scope = &lim["scope"];
    let name = scope["model"]["display_name"].as_str().or_else(|| scope["surface"]["display_name"].as_str()).filter(|n| !n.is_empty());
    let group = lim["group"].as_str().filter(|g| !g.is_empty());
    let kind = if kind.is_empty() { "no kind" } else { kind };
    match (name, group) {
        (Some(n), Some(g)) => format!("{n} · {g} ({kind})"),
        (Some(one), None) | (None, Some(one)) => format!("{one} ({kind})"),
        // A kind alone could be the name a known window goes by.
        (None, None) if kind == "5 hours" || kind == "week" || kind.ends_with(" · week") => format!("{kind} ({kind})"),
        (None, None) => kind.to_string(),
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
                out.scoped.push(ScopedLimit { label, used: pct, resets_at: reset, model: model_scope(lim) });
            }
            kind => {
                let label = own_label(lim, kind);
                if kind_is_news(kind) {
                    crate::log::info(
                        "limits",
                        &format!("the usage endpoint sent a row of kind {kind:?}, unknown to Luna; shown as {label:?}"),
                    );
                }
                out.other.push(OtherLimit { kind: kind.to_string(), label, used: pct, resets_at: reset, model: model_scope(lim) });
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

/// The newest usage known without a request, with when it was taken, where it
/// came from and how long it stands in for a new one: the CLI's cache in its
/// `.claude.json` (`cfg`), or Luna's own last answer from the endpoint.
fn newest_known(cfg: Option<&Value>, account_path: &str) -> Option<(u64, Value, &'static str, u64)> {
    // The CLI refreshes its cache whenever a session runs — which is exactly
    // when the numbers change. Reading it costs no request, so it cannot be
    // throttled, and it survives the usage endpoint being down.
    let cached = cfg.and_then(|cfg| {
        let c = &cfg["cachedUsageUtilization"];
        Some((c["fetchedAtMs"].as_u64()?, c["utilization"]["limits"].clone()))
    });
    let remembered = last_answer().lock().ok().and_then(|m| m.get(account_path).cloned());
    // One without rows to read says nothing about the windows.
    let with_rows = |known: Option<(u64, Value)>| known.filter(|(_, limits)| limits.is_array());
    match (with_rows(cached), with_rows(remembered)) {
        (Some((at, limits)), Some((said_at, _))) if said_at <= at => Some((at, limits, "cache", CACHE_FRESH_MS)),
        (_, Some((at, limits))) => Some((at, limits, "network", NETWORK_EVERY_MS)),
        (Some((at, limits)), None) => Some((at, limits, "cache", CACHE_FRESH_MS)),
        (None, None) => None,
    }
}

/// The account's usage as last known, asking nothing of the network: what a
/// usage limit's hold on a chat is judged on (activity `limit_in_force`).
pub(crate) fn known_reading(account_path: &str) -> Option<crate::activity::UsageReading> {
    use crate::activity::UsageWindow;
    let cfg = read_json(&Path::new(account_path).join(".claude.json"));
    let (at, limits, _, _) = newest_known(cfg.as_ref(), account_path)?;
    let mut out = AccountLimits::default();
    apply_limits(&mut out, &limits);
    let mut windows = Vec::new();
    if let Some(used) = out.h5 {
        windows.push(UsageWindow { label: "5 hours".into(), used, resets_at: out.reset_h5.clone(), model: None, at: None });
    }
    if let Some(used) = out.week {
        windows.push(UsageWindow { label: "week".into(), used, resets_at: out.reset_week.clone(), model: None, at: None });
    }
    // A window scoped to a model holds only a chat on it (`UsageReading::for_model`).
    for s in &out.scoped {
        windows.push(UsageWindow {
            label: format!("week, {}", s.label),
            used: s.used,
            resets_at: s.resets_at.clone(),
            model: s.model.clone(),
            at: None,
        });
    }
    for o in &out.other {
        windows.push(UsageWindow {
            label: o.label.clone(),
            used: o.used,
            resets_at: o.resets_at.clone(),
            model: o.model.clone(),
            at: None,
        });
    }
    Some(crate::activity::UsageReading { at: Some(at), windows })
}

fn fetch_limits(account_path: &str, force: bool) -> Result<AccountLimits, String> {
    let mut out = AccountLimits { signed_in: signed_in(account_path), ..Default::default() };

    // Identity and plan come from the config the CLI wrote at login: no
    // request, and still correct when the network is unavailable.
    let cfg = read_json(&Path::new(account_path).join(".claude.json"));
    if let Some(cfg) = &cfg {
        let acc = &cfg["oauthAccount"];
        out.email = acc["emailAddress"].as_str().map(str::to_owned);
        out.plan = plan_from_tier(acc["organizationRateLimitTier"].as_str().unwrap_or(""));
    }

    // The newer of the CLI's cache and Luna's own last answer stands, and
    // stands alone: each is the whole picture, so the older one would bring
    // back a window the newer one no longer has.
    if let Some((at, limits, source, fresh_for)) = newest_known(cfg.as_ref(), account_path) {
        apply_limits(&mut out, &limits);
        out.source = Some(source.into());
        out.fetched_at_ms = Some(at as f64);
        if !force && now_ms().saturating_sub(at) < fresh_for {
            return Ok(out);
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
        assert_eq!((out.scoped[0].model.as_deref(), out.scoped[1].model.as_deref()), (Some("Fable"), Some("Opus")));
        assert!(serde_json::to_value(&out.scoped[0]).unwrap().get("model").is_none(), "not shown: the label says it");
    }

    /// The reading a limit's hold goes by keeps each row's model, and none for
    /// a row scoped to a surface, which every model counts against.
    #[test]
    fn a_reading_keeps_each_windows_model_scope() {
        let limits = serde_json::json!([
            { "kind": "session", "percent": 100, "resets_at": null },
            { "kind": "weekly_scoped", "percent": 100, "resets_at": null, "scope": { "model": { "display_name": "Opus" } } },
            { "kind": "weekly_scoped", "percent": 10, "resets_at": null, "scope": { "surface": { "display_name": "Claude Code" } } },
            { "kind": "monthly_scoped", "group": "monthly", "percent": 30, "resets_at": null, "scope": { "model": { "display_name": "Fable" } } },
        ]);
        let (dir, path) = account_with_cache("scoped-reading", 1_000, limits);
        let r = known_reading(&path).unwrap();
        let scopes: Vec<(&str, Option<&str>)> = r.windows.iter().map(|w| (w.label.as_str(), w.model.as_deref())).collect();
        assert_eq!(
            scopes,
            [
                ("5 hours", None),
                ("week, Opus", Some("Opus")),
                ("week, Claude Code", None),
                ("Fable · monthly (monthly_scoped)", Some("Fable")),
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
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
                ("monthly_all", "monthly (monthly_all)", 0.07),
                ("monthly_scoped", "Fable · monthly (monthly_scoped)", 0.3),
                ("daily_surface", "Claude Code (daily_surface)", 0.02),
                ("burst", "burst", 0.55),
                ("monthly_extra", "monthly (monthly_extra)", 0.01),
            ]
        );
        assert_eq!(out.other[0].resets_at.as_deref(), Some("2026-11-30T00:00:00Z"));
        assert_eq!(out.week, Some(0.41), "the known rows as ever");
        assert_eq!(out.h5, None);
    }

    /// The quota notices know a window by its label: two windows under one
    /// would take turns clearing and raising each other's warning.
    #[test]
    fn an_unknown_row_never_goes_by_a_known_windows_name() {
        let limits = serde_json::json!([
            { "kind": "weekly_all", "group": "weekly", "percent": 20, "resets_at": null },
            { "kind": "weekly_extra", "group": "week", "percent": 96, "resets_at": null },
            { "kind": "week", "percent": 50, "resets_at": null },
            { "kind": "session_extra", "group": "5 hours", "percent": 1, "resets_at": null },
            { "kind": "scoped_extra", "group": "week", "percent": 3, "resets_at": null, "scope": { "model": { "display_name": "Fable" } } },
        ]);
        let mut out = AccountLimits::default();
        apply_limits(&mut out, &limits);
        let labels: Vec<&str> = out.other.iter().map(|o| o.label.as_str()).collect();
        assert_eq!(labels, ["week (weekly_extra)", "week (week)", "5 hours (session_extra)", "Fable · week (scoped_extra)"]);
        // The names the window gives the known rows (accounts.store.ts).
        for known in ["5 hours", "week", "Fable · week"] {
            assert!(!labels.contains(&known), "{known}");
        }
    }

    /// A window's label is its own: the same whatever comes and goes around it.
    #[test]
    fn an_unknown_rows_label_does_not_hang_on_its_neighbors() {
        let a = serde_json::json!({ "kind": "monthly_a", "group": "monthly", "percent": 90, "resets_at": null });
        let b = serde_json::json!({ "kind": "monthly_b", "group": "monthly", "percent": 10, "resets_at": null });
        let label_of_b = |rows: Value| {
            let mut out = AccountLimits::default();
            apply_limits(&mut out, &rows);
            out.other.iter().find(|o| o.kind == "monthly_b").unwrap().label.clone()
        };
        let together = label_of_b(serde_json::json!([a, b]));
        assert_eq!(together, "monthly (monthly_b)");
        assert_eq!(label_of_b(serde_json::json!([b, a])), together, "the other order");
        assert_eq!(label_of_b(serde_json::json!([b])), together, "alone");
        let mut out = AccountLimits::default();
        apply_limits(&mut out, &serde_json::json!([a, b]));
        assert_ne!(out.other[0].label, out.other[1].label);
    }

    #[test]
    fn an_unknown_kind_is_logged_once() {
        assert!(kind_is_news("ul-invented-kind"));
        assert!(!kind_is_news("ul-invented-kind"), "every poll brings it again");
        assert!(kind_is_news("ul-another-kind"));
    }

    /// An account folder with the CLI's cache in `.claude.json`, taken `ago`
    /// ms before now, and no credentials: nothing here may reach the network.
    fn account_with_cache(tag: &str, ago: u64, limits: Value) -> (std::path::PathBuf, String) {
        let dir = std::env::temp_dir().join(format!("luna-limits-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = serde_json::json!({ "cachedUsageUtilization": { "fetchedAtMs": now_ms() - ago, "utilization": { "limits": limits } } });
        std::fs::write(dir.join(".claude.json"), cfg.to_string()).unwrap();
        let path = dir.to_string_lossy().into_owned();
        (dir, path)
    }

    #[test]
    fn an_older_cli_cache_does_not_bring_back_a_window_a_newer_answer_lacks() {
        let with_session = serde_json::json!([
            { "kind": "session", "percent": 12, "resets_at": null },
            { "kind": "weekly_all", "percent": 40, "resets_at": null },
        ]);
        let without = serde_json::json!([{ "kind": "weekly_all", "percent": 41, "resets_at": null }]);

        // The CLI cached a minute ago, still fresh; a click on refresh has
        // since brought an answer with no session window. The next poll:
        let (dir, path) = account_with_cache("newer-answer", 60_000, with_session.clone());
        last_answer().lock().unwrap().insert(path.clone(), (now_ms() - 10_000, without.clone()));
        let out = fetch_limits(&path, false).unwrap();
        assert_eq!((out.h5, out.week, out.source.as_deref()), (None, Some(0.41), Some("network")));
        last_answer().lock().unwrap().remove(&path);
        let _ = std::fs::remove_dir_all(&dir);

        // The other way round the cache is the newer, and stands.
        let (dir, path) = account_with_cache("newer-cache", 10_000, with_session);
        last_answer().lock().unwrap().insert(path.clone(), (now_ms() - 60_000, without));
        let out = fetch_limits(&path, false).unwrap();
        assert_eq!((out.h5, out.week, out.source.as_deref()), (Some(0.12), Some(0.4), Some("cache")));
        last_answer().lock().unwrap().remove(&path);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
