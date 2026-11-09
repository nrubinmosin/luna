//! Which account a spawn runs on: the one named, or the first of a list with
//! room in its usage windows (`pick_from_list`), among those agents may use.

use crate::agents::registry::reg;
use crate::provider::Provider;
use serde::Serialize;
use std::collections::HashMap;

/// The same with each account's folder.
pub(in crate::agents) fn allowed_account_dirs() -> Vec<crate::accounts::AccountInfo> {
    let blocked = crate::settings::get().agent_blocked_accounts;
    crate::accounts::list_accounts()
        .unwrap_or_default()
        .into_iter()
        .filter(|a| !blocked.iter().any(|b| b == &format!("{}/{}", a.provider.as_str(), a.name)))
        .collect()
}

/// Whether an account folder holds a login its CLI can start on: Claude
/// Code's OAuth refresh token, or Codex's ChatGPT tokens or API key. A
/// helper sent to an account nobody signed in to sits on a login screen.
pub(in crate::agents) fn signed_in(provider: Provider, account_path: &str) -> bool {
    let read = |file: &str| {
        std::fs::read_to_string(std::path::Path::new(account_path).join(file))
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
    };
    match provider {
        Provider::Claude => read(".credentials.json").is_some_and(|v| v["claudeAiOauth"]["refreshToken"].is_string()),
        Provider::Codex => {
            read("auth.json").is_some_and(|v| v["tokens"].is_object() || v["OPENAI_API_KEY"].as_str().is_some_and(|k| !k.is_empty()))
        }
    }
}

/// How a list of accounts came down to one.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AccountChoice {
    pub account: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<SkippedAccount>,
    /// Why this one: the first with room, and how full it is; the full list's
    /// best; or the first signed in, its usage not known yet.
    pub why: String,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SkippedAccount {
    pub account: String,
    pub why: String,
}

/// At this share of a usage window an account counts as spent when a spawn
/// lists several: usage is as old as the window's last fetch and a helper's
/// turn can use up what is left, stopping it on its limit mid-work.
const ACCOUNT_SPENT_PCT: f64 = 95.0;

/// The windows of an account's usage (as `account_usage` keeps it) that are
/// still running, with the percent used: one past its reset says nothing any
/// more, usage being only as fresh as the window's last fetch. None when it
/// has no figures yet.
fn usage_windows(usage: Option<&serde_json::Value>, now_ms: u64) -> Option<Vec<(String, f64)>> {
    let limits = usage?["limits"].as_array().filter(|l| !l.is_empty())?;
    let over = |l: &serde_json::Value| {
        l["resetsAt"]
            .as_str()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .is_some_and(|t| t.timestamp_millis() <= now_ms as i64)
    };
    Some(
        limits
            .iter()
            .filter(|l| !over(l))
            .filter_map(|l| Some((l["label"].as_str().unwrap_or("usage").to_string(), l["usedPct"].as_f64()?)))
            .collect(),
    )
}

/// The provider a list of accounts runs on: one that has every name agents may
/// use at all, the caller's first. An account name can exist under both.
pub(in crate::agents) fn list_provider(
    names: &[String],
    allowed: &[crate::accounts::AccountInfo],
    asked: Option<Provider>,
    mine: Provider,
) -> Result<Provider, String> {
    let known: Vec<&String> = names.iter().filter(|n| allowed.iter().any(|a| a.name == **n)).collect();
    let other = if mine == Provider::Claude { Provider::Codex } else { Provider::Claude };
    let order = asked.map_or(vec![mine, other], |p| vec![p]);
    order.into_iter().find(|p| known.iter().all(|n| allowed.iter().any(|a| a.provider == *p && a.name == **n))).ok_or_else(|| match asked {
        Some(p) => format!("the accounts in the list must all be {} accounts", p.as_str()),
        None => "the accounts in the list must all be of one provider".to_string(),
    })
}

/// The first account of the list that is there, signed in and has room in all
/// its windows; failing that, of those signed in, the one with the most room.
/// Unknown usage is no reason to pass over an account the caller put first.
/// An account is judged by the windows its provider reported, whatever they
/// are called: one it did not send counts neither as room nor as full, and
/// an account with none at all is taken like one whose usage is unknown.
pub(in crate::agents) fn pick_from_list(
    names: &[String],
    provider: Provider,
    allowed: &[crate::accounts::AccountInfo],
    usage: &HashMap<String, serde_json::Value>,
    signed_in: impl Fn(&crate::accounts::AccountInfo) -> bool,
    now_ms: u64,
) -> Result<(crate::accounts::AccountInfo, AccountChoice), String> {
    let mut skipped: Vec<SkippedAccount> = Vec::new();
    // Signed in, over the line: what is left if nothing qualifies.
    let mut spent: Vec<(&crate::accounts::AccountInfo, f64)> = Vec::new();
    for name in names {
        let mut skip = |why: String| skipped.push(SkippedAccount { account: name.clone(), why });
        let Some(a) = allowed.iter().find(|a| a.provider == provider && a.name == *name) else {
            skip(format!("no {} account of that name is available to agents", provider.as_str()));
            continue;
        };
        if !signed_in(a) {
            skip("not signed in".into());
            continue;
        }
        let windows = usage_windows(usage.get(&format!("{}/{}", provider.as_str(), a.name)), now_ms);
        let fullest = windows.iter().flatten().max_by(|x, y| x.1.total_cmp(&y.1));
        match fullest {
            Some((label, pct)) if *pct >= ACCOUNT_SPENT_PCT => {
                skip(format!("{label} at {pct:.0}%"));
                spent.push((a, *pct));
            }
            _ => {
                let why = match (&windows, fullest) {
                    (None, _) => "the first in the list signed in; its usage is not known yet".to_string(),
                    (Some(_), None) => "the first in the list with room: its windows have reset since Luna last read them".to_string(),
                    (Some(_), Some((label, pct))) => {
                        format!("the first in the list with room in every window (under {ACCOUNT_SPENT_PCT:.0}%); the fullest, {label}, is at {pct:.0}%")
                    }
                };
                return Ok((a.clone(), AccountChoice { account: a.name.clone(), skipped, why }));
            }
        }
    }
    if let Some((a, pct)) = spent.iter().min_by(|x, y| x.1.total_cmp(&y.1)) {
        skipped.retain(|s| s.account != a.name);
        let why = format!(
            "every signed-in account in the list is at {ACCOUNT_SPENT_PCT:.0}% or more of a window; this one has the most room, at {pct:.0}%"
        );
        return Ok(((*a).clone(), AccountChoice { account: a.name.clone(), skipped, why }));
    }
    if names.is_empty() {
        return Err("the account list is empty".into());
    }
    let why: Vec<String> = skipped.iter().map(|s| format!("{}: {}", s.account, s.why)).collect();
    Err(format!("no account in the list can take a helper ({}); luna_account_list shows the ones that can", why.join("; ")))
}

/// Each account's usage as the window shows it, by `provider/name`, sent
/// whenever it changes: `luna_account_list` passes it on, so an agent picks an
/// account by the numbers the user sees — and the core asks no usage
/// endpoint of its own, which would only draw their 429s sooner.
#[tauri::command]
pub fn account_usage(usage: HashMap<String, serde_json::Value>) {
    reg().lock().unwrap_or_else(|e| e.into_inner()).usage = usage;
}

/// Account names the user has switched off for agents, as `provider/name`.
#[tauri::command]
pub fn agent_blocked_accounts() -> Vec<String> {
    crate::settings::get().agent_blocked_accounts
}

#[tauri::command]
pub fn set_agent_account(provider: Provider, name: String, allowed: bool) -> Result<Vec<String>, String> {
    let key = format!("{}/{}", provider.as_str(), name);
    crate::settings::update(|s| {
        s.agent_blocked_accounts.retain(|k| k != &key);
        if !allowed {
            s.agent_blocked_accounts.push(key.clone());
        }
    })?;
    Ok(crate::settings::get().agent_blocked_accounts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(provider: Provider, name: &str) -> crate::accounts::AccountInfo {
        crate::accounts::AccountInfo { provider, name: name.into(), path: format!("C:\\acc\\{name}") }
    }

    /// One window per figure, resetting far ahead.
    fn used(pairs: &[(&str, &[(&str, f64)])]) -> HashMap<String, serde_json::Value> {
        pairs
            .iter()
            .map(|(name, windows)| {
                let limits: Vec<_> = windows
                    .iter()
                    .map(|(label, pct)| serde_json::json!({ "label": label, "usedPct": pct, "resetsAt": "2999-01-01T00:00:00Z" }))
                    .collect();
                (format!("claude/{name}"), serde_json::json!({ "limits": limits }))
            })
            .collect()
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn a_list_takes_the_first_with_room_and_says_what_it_passed_over() {
        let allowed = [account(Provider::Claude, "a"), account(Provider::Claude, "b"), account(Provider::Claude, "c")];
        let usage = used(&[("a", &[("5 hours", 97.0), ("week", 10.0)]), ("b", &[("5 hours", 94.0)]), ("c", &[("5 hours", 1.0)])]);
        let (taken, choice) = pick_from_list(&names(&["a", "b", "c"]), Provider::Claude, &allowed, &usage, |_| true, 0).unwrap();
        assert_eq!(taken.name, "b", "under the line, and before c");
        assert_eq!(choice.account, "b");
        assert_eq!(choice.skipped, [SkippedAccount { account: "a".into(), why: "5 hours at 97%".into() }]);
        assert_eq!(choice.why, "the first in the list with room in every window (under 95%); the fullest, 5 hours, is at 94%");
    }

    #[test]
    fn a_window_the_provider_did_not_send_counts_for_nothing() {
        let allowed = [account(Provider::Claude, "a"), account(Provider::Claude, "b"), account(Provider::Claude, "c")];
        // No "5 hours" anywhere, as the endpoint answered on 2026-11-09, and a
        // window of a kind Luna does not know counting like any other.
        let usage = used(&[("a", &[("week", 96.0)]), ("b", &[("week", 20.0), ("monthly", 97.0)]), ("c", &[("week", 30.0)])]);
        let (taken, choice) = pick_from_list(&names(&["a", "b", "c"]), Provider::Claude, &allowed, &usage, |_| true, 0).unwrap();
        assert_eq!(taken.name, "c");
        let skipped: Vec<(&str, &str)> = choice.skipped.iter().map(|s| (s.account.as_str(), s.why.as_str())).collect();
        assert_eq!(skipped, [("a", "week at 96%"), ("b", "monthly at 97%")]);
        assert_eq!(choice.why, "the first in the list with room in every window (under 95%); the fullest, week, is at 30%");

        // Every one spent: the most room is read off the windows each has.
        let usage = used(&[("a", &[("week", 99.0)]), ("b", &[("monthly", 96.0)])]);
        let (taken, choice) = pick_from_list(&names(&["a", "b"]), Provider::Claude, &allowed, &usage, |_| true, 0).unwrap();
        assert_eq!(taken.name, "b");
        assert!(choice.why.contains("the most room, at 96%"), "{}", choice.why);

        // None sent at all: taken in its turn, as usage not known.
        let usage = used(&[("a", &[])]);
        let (taken, choice) = pick_from_list(&names(&["a", "b"]), Provider::Claude, &allowed, &usage, |_| true, 0).unwrap();
        assert_eq!(taken.name, "a");
        assert_eq!(choice.why, "the first in the list signed in; its usage is not known yet");
    }

    #[test]
    fn a_list_passes_over_who_is_not_signed_in_or_not_allowed() {
        let allowed = [account(Provider::Claude, "out"), account(Provider::Claude, "in")];
        let (taken, choice) =
            pick_from_list(&names(&["blocked", "out", "in"]), Provider::Claude, &allowed, &HashMap::new(), |a| a.name == "in", 0).unwrap();
        assert_eq!(taken.name, "in");
        assert_eq!(choice.skipped.len(), 2);
        assert_eq!(choice.skipped[1].why, "not signed in");
        assert_eq!(choice.why, "the first in the list signed in; its usage is not known yet");
    }

    #[test]
    fn a_full_list_comes_down_to_the_most_room() {
        let allowed = [account(Provider::Claude, "a"), account(Provider::Claude, "b"), account(Provider::Claude, "c")];
        let usage = used(&[("a", &[("week", 100.0)]), ("b", &[("week", 96.0), ("5 hours", 20.0)]), ("c", &[("week", 99.0)])]);
        let (taken, choice) = pick_from_list(&names(&["a", "b", "c"]), Provider::Claude, &allowed, &usage, |_| true, 0).unwrap();
        assert_eq!(taken.name, "b");
        assert_eq!(choice.skipped.iter().map(|s| s.account.as_str()).collect::<Vec<_>>(), ["a", "c"]);
        assert!(choice.why.contains("the most room, at 96%"), "{}", choice.why);
    }

    #[test]
    fn a_list_with_nobody_to_take_it_is_an_error() {
        let allowed = [account(Provider::Claude, "a")];
        let e = pick_from_list(&names(&["a", "gone"]), Provider::Claude, &allowed, &HashMap::new(), |_| false, 0).err().unwrap();
        assert!(e.contains("a: not signed in") && e.contains("gone: no claude account"), "{e}");
        assert!(pick_from_list(&[], Provider::Claude, &allowed, &HashMap::new(), |_| true, 0).err().unwrap().contains("empty"));
    }

    #[test]
    fn a_window_past_its_reset_no_longer_counts() {
        let allowed = [account(Provider::Claude, "a")];
        let usage: HashMap<String, serde_json::Value> = [(
            "claude/a".to_string(),
            serde_json::json!({ "limits": [{ "label": "5 hours", "usedPct": 100, "resetsAt": "2026-01-01T00:00:00+00:00" }] }),
        )]
        .into();
        let before = chrono::DateTime::parse_from_rfc3339("2025-12-31T00:00:00Z").unwrap().timestamp_millis() as u64;
        let after = chrono::DateTime::parse_from_rfc3339("2026-01-02T00:00:00Z").unwrap().timestamp_millis() as u64;
        let pick = |now| pick_from_list(&names(&["a"]), Provider::Claude, &allowed, &usage, |_| true, now).unwrap().1;
        assert!(pick(before).why.contains("the most room, at 100%"), "spent, and the only one");
        assert!(pick(after).why.contains("reset since"), "the figure is from before the reset");
    }

    #[test]
    fn a_list_is_of_one_provider() {
        let allowed = [
            account(Provider::Claude, "c"),
            account(Provider::Codex, "x"),
            account(Provider::Claude, "both"),
            account(Provider::Codex, "both"),
        ];
        let of = |list: &[&str], asked| list_provider(&names(list), &allowed, asked, Provider::Claude);
        assert_eq!(of(&["c", "both"], None), Ok(Provider::Claude));
        assert_eq!(of(&["x", "both"], None), Ok(Provider::Codex), "the names bring their provider");
        assert_eq!(of(&["both"], None), Ok(Provider::Claude), "the caller's first");
        assert_eq!(of(&["c", "unknown"], None), Ok(Provider::Claude), "an unknown one is skipped later, with a reason");
        assert!(of(&["c", "x"], None).is_err());
        assert!(of(&["c"], Some(Provider::Codex)).is_err());
        assert_eq!(of(&["both", "x"], Some(Provider::Codex)), Ok(Provider::Codex));
    }
}
