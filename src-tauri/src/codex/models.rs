//! The models Codex offers an account, with the reasoning levels each takes —
//! what `/model` inside Codex shows, so the new-chat dialog can offer the same
//! buttons instead of a box to type a slug into.
//!
//! The list comes in Codex's own order, by `priority`: the first model is the
//! one Codex runs when nothing names another.
//!
//! The list is always Codex's own, from wherever it can be had soonest:
//! 1. `<CODEX_HOME>/models_cache.json`, which the CLI keeps up to date
//!    whenever it runs on the account;
//! 2. the same file of another Codex account — the catalogue is the CLI's, not
//!    the account's — for an account that has never run;
//! 3. `codex debug models` on the account, which fetches the catalogue and
//!    refreshes the cache file as it does;
//! 4. `codex debug models --bundled`, the catalogue built into the binary,
//!    for when there is no network.

use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::provider::Provider;

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CodexModel {
    pub slug: String,
    pub display_name: String,
    pub default_effort: Option<String>,
    pub efforts: Vec<String>,
    /// What Codex says each level is for, by level.
    pub effort_hints: BTreeMap<String, String>,
}

/// The catalogue JSON as both the cache file and `debug models` write it.
fn parse(text: &str) -> Vec<CodexModel> {
    // `debug models` may print a warning ahead of the JSON.
    let Some(start) = text.find('{') else { return vec![] };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text[start..]) else { return vec![] };
    let Some(models) = v["models"].as_array() else { return vec![] };
    let mut listed: Vec<_> = models
        .iter()
        .filter(|m| m["visibility"].as_str().unwrap_or("list") == "list")
        .filter_map(|m| {
            let slug = m["slug"].as_str()?.to_string();
            let levels = m["supported_reasoning_levels"].as_array();
            let model = CodexModel {
                display_name: m["display_name"].as_str().unwrap_or(&slug).to_string(),
                default_effort: m["default_reasoning_level"].as_str().map(str::to_owned),
                efforts: levels
                    .map(|a| a.iter().filter_map(|l| l["effort"].as_str().map(str::to_owned)).collect())
                    .unwrap_or_default(),
                effort_hints: levels
                    .map(|a| {
                        a.iter()
                            .filter_map(|l| Some((l["effort"].as_str()?.to_owned(), l["description"].as_str()?.to_owned())))
                            .collect()
                    })
                    .unwrap_or_default(),
                slug,
            };
            Some((m["priority"].as_i64().unwrap_or(i64::MAX), model))
        })
        .collect();
    // Stable, so models of equal priority keep the catalogue's order.
    listed.sort_by_key(|(priority, _)| *priority);
    listed.into_iter().map(|(_, m)| m).collect()
}

pub fn read_models(account_path: &str) -> Vec<CodexModel> {
    std::fs::read_to_string(Path::new(account_path).join("models_cache.json"))
        .map(|t| parse(&t))
        .unwrap_or_default()
}

/// The freshest cache file among the other Codex accounts.
fn from_other_accounts(account_path: &str) -> Vec<CodexModel> {
    let Ok(accounts) = crate::accounts::list_accounts() else { return vec![] };
    let mut caches: Vec<_> = accounts
        .into_iter()
        .filter(|a| a.provider == Provider::Codex && a.path != account_path)
        .filter_map(|a| {
            let file = Path::new(&a.path).join("models_cache.json");
            let modified = std::fs::metadata(&file).and_then(|m| m.modified()).ok()?;
            Some((modified, a.path))
        })
        .collect();
    caches.sort_by(|a, b| b.0.cmp(&a.0));
    caches.into_iter().map(|(_, p)| read_models(&p)).find(|m| !m.is_empty()).unwrap_or_default()
}

/// `codex debug models`, bounded: a hung network must not hold the dialog.
fn from_cli(account_path: &str, bundled: bool) -> Vec<CodexModel> {
    let mut cmd = Command::new(crate::cli::binary(Provider::Codex));
    cmd.args(["debug", "models"]);
    if bundled {
        cmd.arg("--bundled");
    }
    cmd.env("CODEX_HOME", account_path).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let Ok(mut child) = cmd.spawn() else { return vec![] };
    let stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        if let Some(mut s) = stdout {
            let _ = std::io::Read::read_to_string(&mut s, &mut out);
        }
        out
    });
    let deadline = Instant::now() + Duration::from_secs(if bundled { 10 } else { 20 });
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                crate::log::warn("codex-models", &format!("`codex debug models{}` timed out", if bundled { " --bundled" } else { "" }));
                return vec![];
            }
        }
    }
    parse(&reader.join().unwrap_or_default())
}

/// What the CLI said per account, for half an hour: asking it costs a process
/// and, without `--bundled`, a request.
fn asked() -> &'static Mutex<HashMap<String, (Instant, Vec<CodexModel>)>> {
    static A: OnceLock<Mutex<HashMap<String, (Instant, Vec<CodexModel>)>>> = OnceLock::new();
    A.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The list as far as it can be had without running the CLI: the account's
/// cache file, what the CLI said lately, another account's cache file.
pub fn models_known(account_path: &str) -> Vec<CodexModel> {
    let own = read_models(account_path);
    if !own.is_empty() {
        return own;
    }
    if let Some((at, m)) = asked().lock().ok().and_then(|a| a.get(account_path).cloned()) {
        if at.elapsed() < Duration::from_secs(30 * 60) && !m.is_empty() {
            return m;
        }
    }
    from_other_accounts(account_path)
}

pub fn models_for(account_path: &str) -> Vec<CodexModel> {
    let mut found = models_known(account_path);
    if !found.is_empty() {
        return found;
    }
    found = from_cli(account_path, false);
    if found.is_empty() {
        found = from_cli(account_path, true);
    }
    if !found.is_empty() {
        if let Ok(mut a) = asked().lock() {
            a.insert(account_path.to_string(), (Instant::now(), found.clone()));
        }
    }
    found
}

/// Async and off to a blocking thread: the CLI fallbacks take seconds, and a
/// sync command would run them on the window's own thread.
#[tauri::command]
pub async fn codex_models(account_path: String) -> Vec<CodexModel> {
    if account_path.is_empty() {
        return vec![];
    }
    tauri::async_runtime::spawn_blocking(move || models_for(&account_path)).await.unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_visible_models_with_their_levels() {
        let root = std::env::temp_dir().join(format!("luna-codex-models-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("models_cache.json"),
            r#"{"models":[
              {"slug":"gpt-6-sol","display_name":"GPT-6-Sol","priority":4,"visibility":"list"},
              {"slug":"gpt-6-astra","display_name":"GPT-6-Astra","default_reasoning_level":"medium","priority":2,
               "supported_reasoning_levels":[{"effort":"low","description":"Fast"},{"effort":"high"},{"effort":"ultra"}],"visibility":"list"},
              {"slug":"gpt-reserve","display_name":"GPT-Reserve","supported_reasoning_levels":[],"visibility":"hide","priority":1}
            ]}"#,
        )
        .unwrap();
        let m = read_models(&root.to_string_lossy());
        assert_eq!(m.len(), 2, "hidden models stay out");
        assert_eq!(m[0].slug, "gpt-6-astra", "lowest priority first");
        assert_eq!(m[1].slug, "gpt-6-sol");
        assert_eq!(m[0].display_name, "GPT-6-Astra");
        assert_eq!(m[0].default_effort.as_deref(), Some("medium"));
        assert_eq!(m[0].efforts, vec!["low", "high", "ultra"]);
        assert_eq!(m[0].effort_hints.get("low").map(String::as_str), Some("Fast"));
        assert!(m[0].effort_hints.get("high").is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn debug_models_output_parses_past_a_leading_warning() {
        let out = "WARNING: proceeding, even though we could not create PATH aliases\n\
                   {\"models\":[{\"slug\":\"gpt-6-sol\",\"display_name\":\"GPT-6-Sol\",\
                   \"supported_reasoning_levels\":[{\"effort\":\"minimal\"},{\"effort\":\"max\"}],\"visibility\":\"list\"}]}";
        let m = parse(out);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].slug, "gpt-6-sol");
        assert_eq!(m[0].efforts, vec!["minimal", "max"]);
    }
}
