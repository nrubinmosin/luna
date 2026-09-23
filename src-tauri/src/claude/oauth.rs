//! Renewing an expired access token the way Claude Code itself does, so an
//! account nobody has opened for a day still shows its limits.
//!
//! The access token lives about eight hours, and Luna used to wait for a
//! session to renew it — the row said "waiting for token refresh" until then.
//! The exchange is the CLI's own: `POST platform.claude.com/v1/oauth/token`
//! with the refresh token, its client id and scopes.
//!
//! Refresh tokens rotate, so doing this beside a running CLI is only safe on
//! the CLI's terms. It takes two mkdir locks (proper-lockfile, stale after
//! 60 s): `<config>/.oauth_refresh.lock` and the older `<config>.lock`; it
//! re-reads the file inside them, and writes only if the refresh token on disk
//! is still the one it posted. Luna does the same three things. A CLI that
//! comes to refresh after us finds a different access token on disk and
//! adopts it — that is its own "race resolved" path.

use crate::throttle::now_ms;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
/// What the CLI asks for when the credentials do not list their scopes.
const SCOPES: &str =
    "user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload user:plugins";
/// proper-lockfile's `stale`, as the CLI configures it.
const LOCK_STALE_MS: u128 = 60_000;
/// After a failed exchange the poller leaves the account alone this long; a
/// click on refresh does not wait.
const RETRY_MS: u64 = 5 * 60 * 1000;

pub enum Outcome {
    /// The access token on disk is live now — ours, or a CLI's that beat us.
    Fresh,
    /// A CLI holds the lock: it is renewing the token this very moment.
    Busy,
    /// The server refused the refresh token. Only a new login helps.
    Rejected,
    Failed(String),
}

fn creds_path(account_path: &str) -> PathBuf {
    Path::new(account_path).join(".credentials.json")
}

fn read(account_path: &str) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(creds_path(account_path)).ok()?).ok()
}

fn expired(oauth: &Value) -> bool {
    oauth["expiresAt"].as_u64().map(|ms| now_ms() >= ms).unwrap_or(false)
}

/// Refresh tokens the server turned down. Asking again with one only earns
/// the same answer; a new login writes a different token and clears this.
fn dead() -> &'static Mutex<HashSet<String>> {
    static D: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    D.get_or_init(Default::default)
}

/// Accounts whose last exchange failed: until when to leave them alone, and
/// why it failed — what the row keeps showing meanwhile.
fn held_until() -> &'static Mutex<HashMap<String, (u64, String)>> {
    static H: OnceLock<Mutex<HashMap<String, (u64, String)>>> = OnceLock::new();
    H.get_or_init(Default::default)
}

/// The lock directories we hold, removed on drop in reverse order.
struct Lock(Vec<PathBuf>);

impl Drop for Lock {
    fn drop(&mut self) {
        for p in self.0.iter().rev() {
            let _ = std::fs::remove_dir(p);
        }
    }
}

/// One proper-lockfile lock: mkdir, and a directory older than the stale
/// window was left by a process that died holding it.
fn take(path: &Path) -> Result<bool, String> {
    match std::fs::create_dir(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let stale = std::fs::metadata(path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .map(|age| age.as_millis() >= LOCK_STALE_MS)
                .unwrap_or(false);
            if !stale {
                return Ok(false);
            }
            let _ = std::fs::remove_dir(path);
            Ok(std::fs::create_dir(path).is_ok())
        }
        Err(e) => Err(format!("lock {}: {e}", path.display())),
    }
}

/// Both of the CLI's locks, in its order; None while a CLI holds either.
fn lock(account_path: &str) -> Result<Option<Lock>, String> {
    let dir = Path::new(account_path);
    // The older lock sits beside the folder, named after its real path.
    let mut legacy = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf()).into_os_string();
    legacy.push(".lock");
    let mut held = Lock(vec![]);
    for p in [dir.join(".oauth_refresh.lock"), PathBuf::from(legacy)] {
        if !take(&p)? {
            return Ok(None);
        }
        held.0.push(p);
    }
    Ok(Some(held))
}

enum Exchange {
    Rejected,
    Failed(String),
}

fn exchange(oauth: &Value, refresh_token: &str) -> Result<Value, Exchange> {
    let scope = oauth["scopes"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" "))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| SCOPES.to_string());
    let body = json!({
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
        "client_id": oauth["clientId"].as_str().unwrap_or(CLIENT_ID),
        "scope": scope,
    });
    let resp = ureq::builder()
        .timeout_connect(std::time::Duration::from_secs(5))
        .timeout_read(std::time::Duration::from_secs(15))
        .build()
        .post(TOKEN_URL)
        .send_json(body);
    match resp {
        Ok(r) => r.into_json().map_err(|e| Exchange::Failed(e.to_string())),
        Err(ureq::Error::Status(code, r)) => {
            let body: Value = r.into_json().unwrap_or(Value::Null);
            let err = body["error"].as_str().unwrap_or("");
            if err == "invalid_grant" {
                Err(Exchange::Rejected)
            } else {
                Err(Exchange::Failed(format!("{code} {err}").trim().to_string()))
            }
        }
        Err(e) => Err(Exchange::Failed(e.to_string())),
    }
}

/// The stored `claudeAiOauth` with the new tokens folded in. Fields the
/// response leaves out keep their stored values, as the CLI's merge does.
fn renewed(oauth: &Value, resp: &Value) -> Option<Value> {
    let access = resp["access_token"].as_str()?;
    let expires_in = resp["expires_in"].as_u64()?;
    let now = now_ms();
    let mut next = oauth.clone();
    let obj = next.as_object_mut()?;
    obj.insert("accessToken".into(), access.into());
    obj.insert("expiresAt".into(), (now + expires_in * 1000).into());
    if let Some(rt) = resp["refresh_token"].as_str() {
        obj.insert("refreshToken".into(), rt.into());
    }
    if let Some(s) = resp["refresh_token_expires_in"].as_u64() {
        obj.insert("refreshTokenExpiresAt".into(), (now + s * 1000).into());
    }
    let scopes: Vec<&str> = resp["scope"].as_str().unwrap_or("").split(' ').filter(|s| !s.is_empty()).collect();
    if !scopes.is_empty() {
        obj.insert("scopes".into(), scopes.into());
    }
    Some(next)
}

/// Temp file and rename, so a CLI reading at the same instant sees the old
/// file or the new one and never half of either. A rename can lose to a
/// reader holding the file open; a plain write is the last resort, because
/// the tokens it carries are the only copy there is once the old refresh
/// token has rotated away.
fn write(account_path: &str, text: &str) -> Result<(), String> {
    let path = creds_path(account_path);
    let tmp = path.with_extension("json.luna-tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    for attempt in 0..3 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        if std::fs::rename(&tmp, &path).is_ok() {
            return Ok(());
        }
    }
    let _ = std::fs::remove_file(&tmp);
    std::fs::write(&path, text).map_err(|e| e.to_string())
}

/// Renews the account's access token if it has expired. `force` skips the
/// hold after a recent failure — it is a person asking, not the poller.
pub fn refresh(account_path: &str, force: bool) -> Outcome {
    let Some(v) = read(account_path) else { return Outcome::Failed("no credentials".into()) };
    let Some(rt) = v["claudeAiOauth"]["refreshToken"].as_str() else {
        return Outcome::Failed("no refresh token".into());
    };
    if dead().lock().unwrap().contains(rt) {
        return Outcome::Rejected;
    }
    if !force {
        if let Some((until, why)) = held_until().lock().unwrap().get(account_path) {
            if now_ms() < *until {
                return Outcome::Failed(why.clone());
            }
        }
    }

    let _lock = match lock(account_path) {
        Ok(Some(l)) => l,
        Ok(None) => return Outcome::Busy,
        Err(e) => return Outcome::Failed(e),
    };
    // Again under the lock: a CLI may have renewed it while we were deciding.
    let Some(v) = read(account_path) else { return Outcome::Failed("no credentials".into()) };
    let oauth = &v["claudeAiOauth"];
    if !expired(oauth) {
        return Outcome::Fresh;
    }
    let Some(rt) = oauth["refreshToken"].as_str().map(str::to_owned) else {
        return Outcome::Failed("no refresh token".into());
    };

    let fail = |e: String| {
        crate::log::warn("oauth", &format!("token refresh failed for {account_path}: {e}"));
        held_until().lock().unwrap().insert(account_path.to_string(), (now_ms() + RETRY_MS, e.clone()));
        Outcome::Failed(e)
    };
    let resp = match exchange(oauth, &rt) {
        Ok(r) => r,
        Err(Exchange::Rejected) => {
            crate::log::warn("oauth", &format!("refresh token rejected for {account_path} — needs a new login"));
            dead().lock().unwrap().insert(rt);
            return Outcome::Rejected;
        }
        Err(Exchange::Failed(e)) => return fail(e),
    };
    let Some(next) = renewed(oauth, &resp) else { return fail("unexpected token response".into()) };

    // Compare and swap: only over the refresh token we posted.
    let Some(mut cur) = read(account_path) else { return fail("credentials vanished mid-refresh".into()) };
    if cur["claudeAiOauth"]["refreshToken"].as_str() != Some(rt.as_str()) {
        return Outcome::Fresh;
    }
    cur["claudeAiOauth"] = next;
    let text = match serde_json::to_string(&cur) {
        Ok(t) => t,
        Err(e) => return fail(e.to_string()),
    };
    if let Err(e) = write(account_path, &text) {
        return fail(format!("could not save the new token: {e}"));
    }
    held_until().lock().unwrap().remove(account_path);
    crate::log::info("oauth", &format!("renewed the access token for {account_path}"));
    Outcome::Fresh
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_the_response_into_the_stored_tokens() {
        let stored = json!({
            "accessToken": "old", "refreshToken": "rt-old", "expiresAt": 1,
            "refreshTokenExpiresAt": 5, "scopes": ["user:inference"],
            "subscriptionType": "max", "rateLimitTier": "default_claude_max_20x"
        });
        let resp = json!({
            "access_token": "new", "refresh_token": "rt-new", "expires_in": 28800,
            "scope": "user:inference user:profile"
        });
        let next = renewed(&stored, &resp).unwrap();
        assert_eq!(next["accessToken"], "new");
        assert_eq!(next["refreshToken"], "rt-new");
        assert!(next["expiresAt"].as_u64().unwrap() > now_ms());
        // Not in the response, so kept.
        assert_eq!(next["refreshTokenExpiresAt"], 5);
        assert_eq!(next["subscriptionType"], "max");
        assert_eq!(next["rateLimitTier"], "default_claude_max_20x");
        assert_eq!(next["scopes"], json!(["user:inference", "user:profile"]));

        // A response that does not rotate the refresh token keeps ours.
        let next = renewed(&stored, &json!({"access_token": "a", "expires_in": 60})).unwrap();
        assert_eq!(next["refreshToken"], "rt-old");
        assert_eq!(next["scopes"], json!(["user:inference"]));
        assert!(renewed(&stored, &json!({"expires_in": 60})).is_none());
    }

    #[test]
    fn a_held_lock_is_busy_and_a_stale_one_is_taken() {
        let root = std::env::temp_dir().join(format!("luna-oauth-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let acct = root.join("acct");
        std::fs::create_dir_all(&acct).unwrap();
        let acct = acct.to_string_lossy().into_owned();

        let first = lock(&acct).unwrap();
        assert!(first.is_some());
        assert!(lock(&acct).unwrap().is_none());
        drop(first);
        assert!(!Path::new(&acct).join(".oauth_refresh.lock").exists());

        // A fresh lock of the CLI's alone is enough to stand aside for.
        std::fs::create_dir(Path::new(&acct).join(".oauth_refresh.lock")).unwrap();
        assert!(lock(&acct).unwrap().is_none());
        std::fs::remove_dir(Path::new(&acct).join(".oauth_refresh.lock")).unwrap();
        assert!(lock(&acct).unwrap().is_some());

        let _ = std::fs::remove_dir_all(&root);
    }
}
