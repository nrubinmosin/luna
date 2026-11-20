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
//!
//! A refresh token the server turns down ends the login, and the CLI says so
//! in the file: it blanks the tokens, if the file still holds the one turned
//! down. Luna does the same under the same locks, so every process of Luna's
//! and every later start reads the account as signed out from the file alone.
//! The locks get no heartbeat here, so a refusal counts only if it came back
//! well inside their stale window: past it a CLI may have taken them and
//! rotated the token, and the refusal may be of a token already replaced.

use crate::http::{Agent, Error, Timeouts};
use crate::throttle::now_ms;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
/// What the CLI asks for when the credentials do not list their scopes.
const SCOPES: &str = "user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload user:plugins";
/// proper-lockfile's `stale`, as the CLI configures it.
const LOCK_STALE_MS: u128 = 60_000;
/// The whole exchange, from the name lookup to the last byte.
const EXCHANGE_DEADLINE: Duration = Duration::from_secs(20);
/// How long after reaching for the locks a refusal may still sign the account
/// out: half their stale window, so they are certainly still ours. A refusal
/// later than that is a failed refresh, tried again.
const SIGN_OUT_WITHIN: Duration = Duration::from_secs(30);
/// After a failed exchange the poller leaves the account alone this long; a
/// click on refresh does not wait.
const RETRY_MS: u64 = 5 * 60 * 1000;

pub enum Outcome {
    /// The access token on disk is live now — ours, or a CLI's that beat us.
    Fresh,
    /// A CLI holds the lock: it is renewing the token this very moment.
    Busy,
    /// The server refused the refresh token and its tokens on disk are now
    /// blank, or there was none to send. Only a new login helps. `unsaved`
    /// says why the file could not be blanked, when it could not: then only
    /// this process knows, and it does not post that token again.
    Rejected {
        unsaved: Option<String>,
    },
    Failed(String),
}

fn creds_path(account_path: &str) -> PathBuf {
    Path::new(account_path).join(".credentials.json")
}

fn read(account_path: &str) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(creds_path(account_path)).ok()?).ok()
}

/// The stored refresh token, or None when it is absent or blank. Claude Code
/// leaves `refreshToken: ""` behind once the server has turned the token
/// down, and a blank one can only be turned down again: either way only a
/// new login helps. A token with text in it is used as it is, untrimmed.
fn refresh_token(oauth: &Value) -> Option<&str> {
    oauth["refreshToken"].as_str().filter(|t| !t.trim().is_empty())
}

/// Whether a `claudeAiOauth` is a login: a refresh token (`refresh_token`)
/// not past its `refreshTokenExpiresAt`. While it lives, a new access token
/// can be minted without the user doing anything. The panel's row and the
/// helpers' account choice both go by this.
pub(crate) fn signed_in(oauth: &Value) -> bool {
    refresh_token(oauth).is_some() && oauth["refreshTokenExpiresAt"].as_u64().map(|ms| now_ms() < ms).unwrap_or(true)
}

fn expired(oauth: &Value) -> bool {
    oauth["expiresAt"].as_u64().map(|ms| now_ms() >= ms).unwrap_or(false)
}

/// Accounts whose last exchange failed: until when to leave them alone, and
/// why it failed — what the row keeps showing meanwhile.
fn held_until() -> &'static Mutex<HashMap<String, (u64, String)>> {
    static H: OnceLock<Mutex<HashMap<String, (u64, String)>>> = OnceLock::new();
    H.get_or_init(Default::default)
}

/// Per account, the refresh token the server refused whose file could not be
/// blanked, and why not. The file still reads signed in, so without this the
/// poller would post the same dead token every few minutes for as long as
/// Luna runs.
fn unsaved() -> &'static Mutex<HashMap<String, (String, String)>> {
    static U: OnceLock<Mutex<HashMap<String, (String, String)>>> = OnceLock::new();
    U.get_or_init(Default::default)
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
    /// `time_limit`: the server said the refresh token ran out, which the CLI
    /// records as `loginEnded: "time_limit"`.
    Rejected {
        time_limit: bool,
    },
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
    let t = Timeouts { connect: Duration::from_secs(5), read: Duration::from_secs(15), total: Some(EXCHANGE_DEADLINE) };
    let resp = Agent::new(t).map_err(Exchange::Failed)?.post(TOKEN_URL).json(&body).call();
    match resp {
        Ok(r) => r.json().map_err(Exchange::Failed),
        Err(Error::Status(code, _, r)) => Err(refusal(code, &r.json().unwrap_or(Value::Null))),
        Err(e) => Err(Exchange::Failed(e.to_string())),
    }
}

/// What an error answer from the token endpoint means, by the CLI's own test
/// before it gives a refresh token up (2.1.325's `G5`, `Ms`, `XTa`): a 400 or
/// 401 whose `error` (a string, or an object's `type`) is `invalid_grant` —
/// the token is invalid, expired or revoked (RFC 6749 §5.2); it ran out when
/// the top-level `error_description` says "Refresh token expired", the only
/// place the CLI looks for that. The exception is an account on hold, which
/// the server tells with "account_on_hold": a new login does not lift that,
/// so it is not called a sign-out. The CLI looks for the hold only in the
/// top-level `error_description` (`_er`); Luna looks in an object's
/// `message` too, since a sign-out cannot be taken back and a hold missed
/// the other way is only a refresh tried again. Anything else — a 429, a
/// 5xx, a 400 of another kind — may pass, and is tried again later.
fn refusal(code: u16, body: &Value) -> Exchange {
    let err = body["error"].as_str().or_else(|| body["error"]["type"].as_str()).unwrap_or("");
    let description = body["error_description"].as_str();
    let on_hold = [description, body["error"]["message"].as_str()].contains(&Some("account_on_hold"));
    if matches!(code, 400 | 401) && err == "invalid_grant" && !on_hold {
        return Exchange::Rejected { time_limit: description == Some("Refresh token expired") };
    }
    let why = if on_hold { "account on hold" } else { err };
    Exchange::Failed(format!("{code} {why}").trim().to_string())
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

/// The login turned down: the tokens of `claudeAiOauth` blanked as Claude
/// Code blanks them on `invalid_grant` (`refreshToken: ""`, `accessToken:
/// ""`, `expiresAt: 0`, and `loginEnded: "time_limit"` when the token ran out,
/// else no `loginEnded`), every other field kept. Only while the file still
/// holds `refused`: a newer token there is a login made since, and the
/// refusal was of one gone stale. The caller holds the CLI's locks. Ok(true)
/// once blanked, Ok(false) when the file holds another token. By rename only,
/// never in place: there is no new token here to save at any cost, and a
/// file left half rewritten would take every other field with it.
fn sign_out(account_path: &str, refused: &str, time_limit: bool) -> Result<bool, String> {
    let mut cur = read(account_path).ok_or("credentials vanished mid-refresh")?;
    let Some(oauth) = cur["claudeAiOauth"].as_object_mut().filter(|o| o.get("refreshToken").and_then(Value::as_str) == Some(refused))
    else {
        return Ok(false);
    };
    oauth.insert("refreshToken".into(), "".into());
    oauth.insert("accessToken".into(), "".into());
    oauth.insert("expiresAt".into(), 0.into());
    if time_limit {
        oauth.insert("loginEnded".into(), "time_limit".into());
    } else {
        oauth.shift_remove("loginEnded");
    }
    let path = creds_path(account_path);
    let tmp = temp_beside(&path);
    stage(&tmp, &serde_json::to_string(&cur).map_err(|e| e.to_string())?)?;
    rename_over(&tmp, &path)?;
    Ok(true)
}

/// Temp file and rename, so a CLI reading at the same instant sees the old
/// file or the new one and never half of either. The temp file is the
/// owner's alone from the call that creates it (`secret::create_private`),
/// under a random name nobody can have ready. A rename can lose to a reader
/// holding the file open (Windows); a write in place is the last resort,
/// because the tokens it carries are the only copy there is once the old
/// refresh token has rotated away, and it tightens the file first or writes
/// nothing.
fn write(account_path: &str, text: &str) -> Result<(), String> {
    let path = creds_path(account_path);
    write_via(&path, &temp_beside(&path), text)
}

fn temp_beside(path: &Path) -> PathBuf {
    path.with_extension(format!("json.{}.luna-tmp", crate::secret::random_hex(8)))
}

/// `text` in a new private file at `tmp`, which is removed on a failure only
/// once this call has made it: a file someone else has there is not ours to
/// delete.
fn stage(tmp: &Path, text: &str) -> Result<(), String> {
    use std::io::Write;
    let mut file = crate::secret::create_private(tmp).map_err(|e| e.to_string())?;
    let wrote = file.write_all(text.as_bytes());
    drop(file);
    wrote.map_err(|e| {
        let _ = std::fs::remove_file(tmp);
        e.to_string()
    })
}

#[cfg(test)]
thread_local! {
    /// For this test thread: every rename onto the file is refused, as a
    /// reader holding it open refuses it on Windows.
    static NO_RENAME: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The staged `tmp` renamed over `path`, tried three times; on a failure
/// `tmp` is gone and `path` is as it was.
fn rename_over(tmp: &Path, path: &Path) -> Result<(), String> {
    let mut why = String::new();
    for attempt in 0..3 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(100));
        }
        #[cfg(test)]
        if NO_RENAME.with(std::cell::Cell::get) {
            why = "rename refused (test)".into();
            continue;
        }
        match std::fs::rename(tmp, path) {
            Ok(()) => return Ok(()),
            Err(e) => why = e.to_string(),
        }
    }
    let _ = std::fs::remove_file(tmp);
    Err(why)
}

/// `write` through `tmp`.
fn write_via(path: &Path, tmp: &Path, text: &str) -> Result<(), String> {
    use std::io::Write;
    stage(tmp, text)?;
    if rename_over(tmp, path).is_ok() {
        return Ok(());
    }
    let mut file = std::fs::OpenOptions::new().write(true).open(path).map_err(|e| e.to_string())?;
    crate::secret::restrict(&file).map_err(|e| e.to_string())?;
    file.set_len(0).and_then(|()| file.write_all(text.as_bytes())).map_err(|e| e.to_string())
}

/// Renews the account's access token if it has expired. `force` skips the
/// hold after a recent failure — it is a person asking, not the poller.
pub fn refresh(account_path: &str, force: bool) -> Outcome {
    if !crate::token_use::allowed("a token renewal", account_path) {
        return Outcome::Failed("token renewal switched off in this test run".into());
    }
    refresh_with(account_path, force, SIGN_OUT_WITHIN, exchange)
}

/// `refresh`, with the server's side as `exchange` and `SIGN_OUT_WITHIN` as
/// `within`, so the tests can stand in for both.
fn refresh_with(
    account_path: &str,
    force: bool,
    within: Duration,
    exchange: impl FnOnce(&Value, &str) -> Result<Value, Exchange>,
) -> Outcome {
    let Some(v) = read(account_path) else { return Outcome::Failed("no credentials".into()) };
    // No token to post is the same as one turned down, and asks nothing.
    let Some(rt) = refresh_token(&v["claudeAiOauth"]) else { return Outcome::Rejected { unsaved: None } };
    // Turned down before in this process, the file left saying otherwise.
    if let Some((_, why)) = unsaved().lock().unwrap().get(account_path).filter(|(refused, _)| refused == rt) {
        return Outcome::Rejected { unsaved: Some(why.clone()) };
    }
    if !force {
        if let Some((until, why)) = held_until().lock().unwrap().get(account_path) {
            if now_ms() < *until {
                return Outcome::Failed(why.clone());
            }
        }
    }

    // Before the locks are taken, so it never reads younger than they are.
    let reached = Instant::now();
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
    // A CLI may have given the token up meanwhile, and blanked it.
    let Some(rt) = refresh_token(oauth).map(str::to_owned) else { return Outcome::Rejected { unsaved: None } };

    let fail = |e: String| {
        crate::log::warn("oauth", &format!("token refresh failed for {account_path}: {e}"));
        held_until().lock().unwrap().insert(account_path.to_string(), (now_ms() + RETRY_MS, e.clone()));
        Outcome::Failed(e)
    };
    let resp = match exchange(oauth, &rt) {
        Ok(r) => r,
        // The locks may have lapsed meanwhile, and a CLI taken them and
        // rotated the token: the refusal may be of one already replaced, its
        // successor not yet on disk, so the file is not touched.
        Err(Exchange::Rejected { .. }) if reached.elapsed() >= within => {
            return fail(format!(
                "sign-in refused {} s after taking the locks, too late to be sure no CLI renewed it meanwhile",
                reached.elapsed().as_secs()
            ));
        }
        Err(Exchange::Rejected { time_limit }) => {
            crate::log::warn("oauth", &format!("refresh token rejected for {account_path} — needs a new login"));
            held_until().lock().unwrap().remove(account_path);
            return match sign_out(account_path, &rt, time_limit) {
                Ok(true) => Outcome::Rejected { unsaved: None },
                // A login, or a CLI past the locks, wrote the file meanwhile:
                // the account is whatever the file now says.
                Ok(false) if read(account_path).is_some_and(|v| signed_in(&v["claudeAiOauth"])) => Outcome::Fresh,
                Ok(false) => Outcome::Rejected { unsaved: None },
                // The file keeps saying signed in; this process at least
                // knows better, and posts the token no more.
                Err(e) => {
                    let why = format!(".credentials.json could not be updated: {e}");
                    crate::log::warn("oauth", &format!("{account_path}: {why}"));
                    unsaved().lock().unwrap().insert(account_path.to_string(), (rt, why.clone()));
                    Outcome::Rejected { unsaved: Some(why) }
                }
            };
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

/// A refresh of the account's expired token that the server turns down, run
/// through `refresh` with only the server stood in for, for the tests of the
/// modules that read what it leaves on disk. True once the file is blanked.
#[cfg(test)]
pub(crate) fn refused_for_test(account_path: &str) -> bool {
    let refuse = |_: &Value, _: &str| Err(Exchange::Rejected { time_limit: false });
    matches!(refresh_with(account_path, true, SIGN_OUT_WITHIN, refuse), Outcome::Rejected { unsaved: None })
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

    /// Error bodies in the RFC 6749 §5.2 shape the token endpoint answers in,
    /// and the object shape the CLI reads `error` from as well.
    #[test]
    fn only_a_bad_grant_is_a_sign_out() {
        let rejected = |code, body: Value| matches!(refusal(code, &body), Exchange::Rejected { .. });
        let failed = |code, body: Value| match refusal(code, &body) {
            Exchange::Failed(why) => why,
            Exchange::Rejected { .. } => panic!("{code} {body} taken as a sign-out"),
        };
        let grant = json!({ "error": "invalid_grant", "error_description": "Refresh token not found or invalid" });
        assert!(rejected(400, grant.clone()));
        assert!(matches!(refusal(400, &grant), Exchange::Rejected { time_limit: false }));
        let ran_out = json!({ "error": "invalid_grant", "error_description": "Refresh token expired" });
        assert!(matches!(refusal(400, &ran_out), Exchange::Rejected { time_limit: true }));
        assert!(rejected(401, grant.clone()));
        // The object shape: a grant refused, but "ran out" is read only where
        // the CLI reads it, the top-level `error_description`.
        let object = json!({ "error": { "type": "invalid_grant", "message": "Refresh token expired" } });
        assert!(matches!(refusal(400, &object), Exchange::Rejected { time_limit: false }));

        // The same words under another status are no verdict on the token.
        assert_eq!(failed(500, grant.clone()), "500 invalid_grant");
        assert_eq!(failed(429, grant), "429 invalid_grant");
        // A request the server could not read, not a token it turned down.
        let unread = json!({ "error": "invalid_request", "error_description": "refresh_token is required" });
        assert_eq!(failed(400, unread), "400 invalid_request");
        assert_eq!(failed(401, json!({ "error": "invalid_client" })), "401 invalid_client");
        assert_eq!(failed(502, Value::Null), "502");
        // A new login does not lift a hold, so the row must not ask for one.
        let hold = json!({ "error": "invalid_grant", "error_description": "account_on_hold" });
        assert_eq!(failed(400, hold), "400 account on hold");
        let hold = json!({ "error": { "type": "invalid_grant", "message": "account_on_hold" } });
        assert_eq!(failed(401, hold), "401 account on hold");
    }

    /// What Claude Code leaves after the server turned its refresh token
    /// down, as #76 found it: nothing to post, so no request and no lock.
    /// The lock is held as a CLI would hold it: `refresh` reaching for it
    /// would answer Busy, and the exchange comes only after it.
    #[test]
    fn a_blank_or_missing_refresh_token_is_rejected_without_asking() {
        let root = std::env::temp_dir().join(format!("luna-oauth-blank-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".oauth_refresh.lock")).unwrap();
        let acct = root.to_string_lossy().into_owned();
        let with = |oauth: &Value| std::fs::write(creds_path(&acct), json!({ "claudeAiOauth": oauth }).to_string()).unwrap();
        // The seam holds: a token to post goes for the lock and finds it taken.
        with(&json!({ "accessToken": "old", "refreshToken": "rt-live", "expiresAt": 1 }));
        assert!(matches!(refresh(&acct, true), Outcome::Busy));
        // The message names the case, not the JSON, which CodeQL reads as a logged token.
        for (case, oauth) in [
            ("blank", json!({ "accessToken": "", "refreshToken": "", "expiresAt": 0 })),
            ("whitespace", json!({ "accessToken": "", "refreshToken": " \t", "expiresAt": 0 })),
            ("missing", json!({ "accessToken": "old", "expiresAt": 1 })),
        ] {
            with(&oauth);
            assert!(matches!(refresh(&acct, true), Outcome::Rejected { unsaved: None }), "{case} refresh token");
        }
        assert!(root.join(".oauth_refresh.lock").is_dir(), "the CLI's lock is left as it was");
        assert_eq!(refresh_token(&json!({ "refreshToken": "rt-1" })), Some("rt-1"));
        assert_eq!(refresh_token(&json!({ "refreshToken": 7 })), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A refusal blanks the tokens as Claude Code 2.1.325 does (`Kjo`): only
    /// over the refused token, every other field kept, `loginEnded` said only
    /// of a token that ran out. A newer token saved meanwhile is left alone.
    #[test]
    fn a_refused_token_still_on_disk_is_blanked_and_nothing_else() {
        let root = std::env::temp_dir().join(format!("luna-oauth-refused-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let acct = root.to_string_lossy().into_owned();
        let path = creds_path(&acct);
        let stored = |rt: &str| {
            json!({
                "claudeAiOauth": {
                    "accessToken": "at", "refreshToken": rt, "expiresAt": 1,
                    "refreshTokenExpiresAt": 99999999999999u64, "scopes": ["user:inference"],
                    "subscriptionType": "max", "rateLimitTier": "default_claude_max_20x", "loginEnded": "earlier"
                },
                "mcpOAuth": { "srv": { "accessToken": "mcp" } }
            })
        };
        let save = |v: &Value| std::fs::write(&path, v.to_string()).unwrap();
        let now = || serde_json::from_str::<Value>(&std::fs::read_to_string(&path).unwrap()).unwrap();

        // A CLI saved a newer token since the refused one was read: untouched.
        save(&stored("rt-newer"));
        let before = std::fs::read(&path).unwrap();
        assert!(!sign_out(&acct, "rt-refused", false).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(signed_in(&now()["claudeAiOauth"]));

        save(&stored("rt-refused"));
        assert!(sign_out(&acct, "rt-refused", false).unwrap());
        let mut want = stored("");
        let o = want["claudeAiOauth"].as_object_mut().unwrap();
        o.insert("accessToken".into(), "".into());
        o.insert("expiresAt".into(), 0.into());
        o.shift_remove("loginEnded");
        assert_eq!(now(), want);
        assert!(!signed_in(&now()["claudeAiOauth"]));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        // Once blank, nothing more to sign out, and a later refresh asks nobody.
        assert!(!sign_out(&acct, "rt-refused", false).unwrap());
        assert!(matches!(refresh(&acct, true), Outcome::Rejected { unsaved: None }));

        save(&stored("rt-refused"));
        assert!(sign_out(&acct, "rt-refused", true).unwrap());
        assert_eq!(now()["claudeAiOauth"]["loginEnded"], "time_limit");
        let left: Vec<_> = std::fs::read_dir(&root).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(left.len(), 1, "no temp file left: {left:?}");

        // A rename refused is a sign-out refused: never written in place.
        save(&stored("rt-refused"));
        let before = std::fs::read(&path).unwrap();
        NO_RENAME.with(|c| c.set(true));
        let out = sign_out(&acct, "rt-refused", false);
        NO_RENAME.with(|c| c.set(false));
        assert!(out.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let left: Vec<_> = std::fs::read_dir(&root).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(left.len(), 1, "no temp file left: {left:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `refresh`'s own refusal arm, only the server stood in for: a refusal
    /// blanks the file under the locks; a token saved meanwhile is left
    /// alone; a file that cannot be replaced is left as it was and the token
    /// is not posted again; a refusal too late to be sure of the locks
    /// touches nothing.
    #[test]
    fn a_refusal_through_refresh_signs_out_only_while_sure() {
        let root = std::env::temp_dir().join(format!("luna-oauth-arm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let fresh = |name: &str| {
            let dir = root.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            let acct = dir.to_string_lossy().into_owned();
            let v = json!({
                "claudeAiOauth": { "accessToken": "at", "refreshToken": "rt-old", "expiresAt": 1, "scopes": ["user:inference"] },
                "mcpOAuth": { "srv": { "accessToken": "mcp" } }
            });
            std::fs::write(creds_path(&acct), v.to_string()).unwrap();
            acct
        };
        let bytes = |acct: &str| std::fs::read(creds_path(acct)).unwrap();
        let files = |acct: &str| std::fs::read_dir(acct).unwrap().flatten().count();
        let unlocked = |acct: &str| !Path::new(acct).join(".oauth_refresh.lock").exists() && lock(acct).unwrap().is_some();
        let refuse = |_: &Value, rt: &str| {
            assert_eq!(rt, "rt-old");
            Err(Exchange::Rejected { time_limit: true })
        };
        let posted = |_: &Value, rt: &str| -> Result<Value, Exchange> { panic!("{rt} posted again") };

        let acct = fresh("refused");
        assert!(matches!(refresh_with(&acct, false, SIGN_OUT_WITHIN, refuse), Outcome::Rejected { unsaved: None }));
        let v = read(&acct).unwrap();
        let o = &v["claudeAiOauth"];
        assert_eq!([&o["refreshToken"], &o["accessToken"], &o["expiresAt"]], [&json!(""), &json!(""), &json!(0)]);
        assert_eq!(o["loginEnded"], "time_limit");
        assert_eq!(v["mcpOAuth"]["srv"]["accessToken"], "mcp");
        assert!(unlocked(&acct));
        assert!(matches!(refresh_with(&acct, false, SIGN_OUT_WITHIN, posted), Outcome::Rejected { unsaved: None }));

        // A login saved a newer token while the exchange ran.
        let acct = fresh("newer");
        let newer = json!({ "claudeAiOauth": { "accessToken": "at-new", "refreshToken": "rt-new", "expiresAt": now_ms() + 3_600_000 } });
        let refused_after_a_login = |_: &Value, _: &str| {
            std::fs::write(creds_path(&acct), newer.to_string()).unwrap();
            Err(Exchange::Rejected { time_limit: false })
        };
        assert!(matches!(refresh_with(&acct, false, SIGN_OUT_WITHIN, refused_after_a_login), Outcome::Fresh));
        assert_eq!(read(&acct).unwrap(), newer);

        let acct = fresh("unwritable");
        let before = bytes(&acct);
        NO_RENAME.with(|c| c.set(true));
        let out = refresh_with(&acct, false, SIGN_OUT_WITHIN, refuse);
        NO_RENAME.with(|c| c.set(false));
        assert!(matches!(&out, Outcome::Rejected { unsaved: Some(why) } if why.contains("could not be updated")));
        assert_eq!(bytes(&acct), before);
        assert_eq!(files(&acct), 1, "no temp file left");
        assert!(unlocked(&acct));
        // Neither the poller nor a click posts it again.
        for force in [false, true] {
            assert!(matches!(refresh_with(&acct, force, SIGN_OUT_WITHIN, posted), Outcome::Rejected { unsaved: Some(_) }));
        }

        let acct = fresh("slow");
        let before = bytes(&acct);
        let slow = |_: &Value, _: &str| {
            std::thread::sleep(Duration::from_millis(80));
            Err(Exchange::Rejected { time_limit: false })
        };
        let out = refresh_with(&acct, false, Duration::from_millis(40), slow);
        assert!(matches!(&out, Outcome::Failed(why) if why.contains("too late")));
        assert_eq!(bytes(&acct), before);
        assert!(signed_in(&read(&acct).unwrap()["claudeAiOauth"]));
        assert!(unlocked(&acct));
        // A passing failure: held off for the poller, tried again on a click.
        assert!(matches!(refresh_with(&acct, false, SIGN_OUT_WITHIN, posted), Outcome::Failed(_)));
        assert!(matches!(refresh_with(&acct, true, SIGN_OUT_WITHIN, refuse), Outcome::Rejected { unsaved: None }));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn renewed_tokens_replace_the_file_whole_and_are_the_owners_alone() {
        let root = std::env::temp_dir().join(format!("luna-oauth-write-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let acct = root.to_string_lossy().into_owned();
        let path = creds_path(&acct);
        std::fs::write(&path, "old").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // As a careless copy may have left it.
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        write(&acct, "new").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        let left: Vec<_> = std::fs::read_dir(&root).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(left.len(), 1, "no temp file left: {left:?}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        // A temp name someone else holds: refused, their file and ours untouched.
        let theirs = root.join("planted.luna-tmp");
        std::fs::write(&theirs, "theirs").unwrap();
        assert!(write_via(&path, &theirs, "newer").is_err());
        assert_eq!(std::fs::read_to_string(&theirs).unwrap(), "theirs");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        // A renewal whose rename is refused is written in place: its tokens
        // are the only copy there is.
        NO_RENAME.with(|c| c.set(true));
        let wrote = write(&acct, "in place");
        NO_RENAME.with(|c| c.set(false));
        wrote.unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "in place");
        let _ = std::fs::remove_dir_all(&root);
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
