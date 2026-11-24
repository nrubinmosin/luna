//! How many tokens a session can still take in before its CLI compacts its
//! context on its own, for the pane's context meter: the threshold its CLI's
//! settings make of its window (claude/compact.rs, codex/compact.rs), less
//! what its last answer used. The settings are read with Luna's own readers,
//! never by asking the CLI. Codex keeps its launch's inputs. Claude Code
//! reloads settings, so its inputs are refreshed on change.
//!
//! Always an estimate: Luna counts a context by its last answer's usage,
//! and each CLI by its own count, which takes in more than that.

use crate::provider::Provider;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

/// Usage and its model for the compaction estimate.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    pub tokens: u64,
    pub model: String,
}

/// The start of a session a figure is reckoned for.
#[derive(Clone, Debug)]
pub struct Launch {
    pub provider: Provider,
    pub account_path: String,
    /// Where it works, whose project settings it reads.
    pub folder: String,
    /// Which start of the chat or delegate turn.
    pub start: u64,
    pub claude_window: Option<u32>,
    pub(crate) codex: Option<Arc<crate::codex::compact::Startup>>,
}

/// A Claude Code session's inputs, by session, launch, folder and model.
type Key = (String, u64, String, String, String);
type Seen = HashMap<Key, Arc<Read>>;

struct Read {
    stamp: u64,
    inputs: Option<crate::claude::compact::Inputs>,
    at: Instant,
}

fn seen() -> &'static Mutex<Seen> {
    static S: OnceLock<Mutex<Seen>> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// Past this many starts and models, the oldest read makes room.
const SEEN_CAP: usize = 512;

fn resolved(cache: &Mutex<Seen>, key: Key, stamp: u64, read: impl FnOnce() -> Read) -> Arc<Read> {
    let lock = || cache.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(known) = lock().get(&key).filter(|r| r.stamp == stamp) {
        return known.clone();
    }
    // Settings are resolved off the lock. If two readers miss together,
    // they share the first result for the same contents.
    let fresh = Arc::new(read());
    let mut map = lock();
    if let Some(known) = map.get(&key).filter(|r| r.stamp == stamp) {
        return known.clone();
    }
    map.insert(key, fresh.clone());
    if map.len() > SEEN_CAP {
        if let Some(oldest) = map.iter().min_by_key(|(_, r)| r.at).map(|(key, _)| key.clone()) {
            map.remove(&oldest);
        }
    }
    fresh
}

/// What session `id` on `model` can take in past `tokens`, its rollout or
/// transcript stating `window`; None where auto-compaction is off or its
/// threshold cannot be told.
pub fn left(id: &str, launch: &Launch, model: &str, tokens: u64, window: u64) -> Option<u64> {
    let limit = match launch.provider {
        Provider::Claude => crate::claude::compact::threshold(claude_inputs(id, launch, model).inputs.as_ref()?, window),
        Provider::Codex => launch.codex.as_ref()?.limit(model, window)?,
    };
    Some(limit.saturating_sub(tokens))
}

fn claude_inputs(id: &str, launch: &Launch, model: &str) -> Arc<Read> {
    use crate::claude::{compact, defaults::managed_dir};
    let Launch { account_path, folder, start, .. } = launch;
    // Claude Code 2.1.330's settingsChangeDetector notifies the running
    // app-state handler ("Settings changed from ..."), which rereads settings.
    // Hash the same bounded bytes we parse to catch same-length rewrites.
    let (files, policy) = compact::settings_files(account_path, folder, &managed_dir());
    let read = |path: &Path| crate::boundary::read_settings_file(path, crate::boundary::SETTINGS_MAX).ok().flatten();
    let files: Vec<_> = files.iter().map(|p| read(p)).collect();
    let policy: Vec<_> = policy.iter().map(|p| read(p)).collect();
    let legacy = read(&Path::new(account_path).join(".claude.json"));
    let mut hash = DefaultHasher::new();
    (&files, &policy, &legacy, launch.claude_window).hash(&mut hash);
    let stamp = hash.finish();
    let key = (id.to_string(), *start, account_path.clone(), folder.clone(), model.to_string());
    resolved(seen(), key, stamp, || {
        let parse = |raw: Option<Vec<u8>>| {
            let value: serde_json::Value = serde_json::from_slice(&raw?).ok()?;
            value.is_object().then_some(value)
        };
        let inputs = compact::inputs(
            account_path,
            files.into_iter().filter_map(parse).collect(),
            policy.into_iter().filter_map(parse),
            model,
            &|name| {
                if let Some(window) = launch.claude_window.filter(|_| name == compact::AUTO_COMPACT_WINDOW_ENV) {
                    return Some(window.to_string());
                }
                std::env::var(name).ok()
            },
            parse(legacy),
        );
        Read { stamp, inputs, at: Instant::now() }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch(account: &Path, start: u64) -> Launch {
        let path = account.to_string_lossy().into_owned();
        Launch { provider: Provider::Claude, account_path: path.clone(), folder: path, start, claude_window: None, codex: None }
    }

    #[test]
    fn a_file_appearing_or_changing_refreshes_the_same_start_and_model() {
        let account = std::env::temp_dir().join(format!("luna-to-compact-{}", std::process::id()));
        std::fs::create_dir_all(&account).unwrap();
        let settings = account.join("settings.json");
        let model = "claude-opus-5-5";
        let left_at = |start: u64, model: &str| left("tc-chat", &launch(&account, start), model, 100_000, 1_000_000);
        assert_eq!(left_at(1, model), Some(867_000), "a read before the file exists");
        std::fs::write(&settings, r#"{ "autoCompactWindow": 400000 }"#).unwrap();
        assert_eq!(left_at(1, model), Some(400_000 - 33_000 - 100_000));
        let before = settings.metadata().unwrap().modified().unwrap();
        std::fs::write(&settings, r#"{ "autoCompactWindow": 500000 }"#).unwrap();
        std::fs::File::options().write(true).open(&settings).unwrap().set_modified(before).unwrap();
        assert_eq!(left_at(1, model), Some(367_000), "same length and mtime");
        std::fs::write(&settings, r#"{ "autoCompactEnabled": false }"#).unwrap();
        assert_eq!(left_at(1, model), None, "the switch is read again");
        assert_eq!(left_at(1, "claude-sonnet-5"), None, "another model: read again");
        assert_eq!(left_at(2, model), None, "a new start: read again");
        std::fs::write(&settings, "{}").unwrap();
        assert_eq!(left("tc-chat", &launch(&account, 3), model, 990_000, 1_000_000), Some(0), "past it: none left");
        let _ = std::fs::remove_dir_all(&account);
    }

    #[test]
    fn a_claude_delegate_estimates_with_its_launch_window_below_file_env() {
        let project = crate::golden::project();
        let account = Path::new(&project);
        let settings = account.join("settings.json");
        std::fs::write(&settings, r#"{"autoCompactWindow": 500000}"#).unwrap();
        let mut delegate = launch(account, 1);
        delegate.claude_window = Some(400_000);
        let at = |start| left("tc-claude-delegate", start, "claude-opus-5-5", 50_000, 1_000_000);
        assert_eq!(at(&delegate), Some(317_000));
        assert_eq!(at(&launch(account, 2)), Some(417_000), "the chat has no window from Luna");
        std::fs::write(&settings, r#"{"env": {"CLAUDE_CODE_AUTO_COMPACT_WINDOW": "250k"}}"#).unwrap();
        assert_eq!(at(&delegate), Some(167_000), "the settings-file env wins");
    }

    #[test]
    fn codex_keeps_config_and_catalog_edits_for_the_next_launch() {
        crate::codex::config::use_empty_machine_layers_for_test();
        let account = std::env::temp_dir().join(format!("luna-to-compact-codex-{}", std::process::id()));
        std::fs::create_dir_all(&account).unwrap();
        let id = "tc-codex";
        let start = |start| {
            let mut l = launch(&account, start);
            l.provider = Provider::Codex;
            l.codex = crate::codex::compact::at_launch(id, &l.account_path, &l.folder);
            l
        };
        let tokens_left = |l: &Launch, model: &str| left(id, l, model, 0, 190_000);
        let catalog = account.join("custom.json");
        let config = account.join("config.toml");
        let path = toml_edit::Value::from(catalog.to_str().unwrap());
        std::fs::write(&config, format!("model_catalog_json = {path}\n")).unwrap();
        std::fs::write(
            &catalog,
            r#"{"models":[{"slug":"test-model","auto_compact_token_limit":50000},{"slug":"other-model","auto_compact_token_limit":60000}]}"#,
        )
        .unwrap();
        let first = start(1);
        std::fs::write(&config, format!("model_catalog_json = {path}\nmodel_auto_compact_token_limit = 40000\n")).unwrap();
        assert_eq!(tokens_left(&first, "test-model"), Some(50_000), "even before the first sample");
        assert_eq!(tokens_left(&start(2), "test-model"), Some(40_000));
        let mut delegate = start(3);
        delegate.codex = crate::codex::compact::at_launch_with(id, &delegate.account_path, &delegate.folder, None, Some(100_000));
        assert_eq!(tokens_left(&delegate, "test-model"), Some(100_000), "the launch limit wins over the account");
        std::fs::write(&catalog, "{").unwrap();
        assert_eq!(tokens_left(&first, "other-model"), Some(60_000), "the launch covers subsequent models");
        assert_eq!(tokens_left(&start(3), "test-model"), None);
        let _ = std::fs::remove_dir_all(&account);
    }

    #[test]
    fn only_the_oldest_read_is_evicted_and_a_racing_read_keeps_the_winner() {
        let cache = Mutex::new(Seen::new());
        let start = Instant::now();
        let key = |n: usize| (n.to_string(), 1, "test".into(), "test".into(), "test-model".into());
        let read = |n| Read { stamp: 1, inputs: None, at: start + std::time::Duration::from_millis(n as u64) };
        for n in 0..SEEN_CAP {
            resolved(&cache, key(n), 1, || read(n));
        }
        let latest_key = key(SEEN_CAP);
        let winner = resolved(&cache, latest_key.clone(), 1, || {
            resolved(&cache, latest_key, 1, || read(SEEN_CAP));
            read(SEEN_CAP + 1)
        });
        assert_eq!(winner.at, read(SEEN_CAP).at);
        let map = cache.into_inner().unwrap();
        assert_eq!(map.len(), SEEN_CAP);
        assert!(!map.contains_key(&key(0)));
        assert!(map.contains_key(&key(1)));
    }
}
