//! How many tokens a session can still take in before its CLI compacts its
//! context on its own (`Context::to_compact`): the threshold its CLI's
//! settings make of its window (claude/compact.rs, codex/compact.rs), less
//! what its last answer used. The settings are read with Luna's own readers,
//! never by asking the CLI. A start and model keep their inputs while the
//! files behind them stay the same. Each usage sample is then a subtraction.
//!
//! Always an estimate: Luna counts a context by its last answer's usage,
//! and each CLI by its own count, which takes in more than that.

use crate::background::{Context, ToCompact};
use crate::provider::Provider;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

/// The start of a session a figure is reckoned for.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Launch {
    pub provider: Provider,
    pub account_path: String,
    /// Where it works, whose project settings it reads.
    pub folder: String,
    /// A helper's run, its folder trusted for it (codex/config.rs
    /// `loaded_layers`).
    pub helper: bool,
    /// Which start of it: a chat's spawn, a helper's turn.
    pub start: u64,
}

impl Launch {
    /// The launch of session `id` while it is one: a helper's latest turn, a
    /// chat's running CLI. None for a chat whose CLI is not running: what a
    /// later start reads may differ.
    pub fn of(pty: &crate::pty::PtyManager, id: &str) -> Option<Launch> {
        if crate::background::has(id) {
            return crate::background::launch(id);
        }
        let p = pty.probe(id)?;
        let folder = p.working_dir().unwrap_or(p.cwd);
        Some(Launch { provider: p.provider, account_path: p.account_path, folder, helper: false, start: p.spawned_at_ms as u64 })
    }
}

enum Inputs {
    Claude(crate::claude::compact::Inputs),
    Codex(crate::codex::compact::Inputs),
}

/// Sessions' inputs, by session, the start they were read for and model.
type Key = (String, Launch, String);
type Seen = HashMap<Key, Arc<Read>>;

struct Read {
    files: Vec<(PathBuf, Option<crate::stamped::Stamp>)>,
    inputs: Option<Inputs>,
    at: Instant,
}

impl Read {
    fn track(&mut self, path: PathBuf) {
        let stamp = crate::stamped::stamp(&path);
        self.files.push((path, stamp));
    }

    fn current(&self) -> bool {
        self.files.iter().all(|(path, stamp)| crate::stamped::stamp(path) == *stamp)
    }
}

fn seen() -> &'static Mutex<Seen> {
    static S: OnceLock<Mutex<Seen>> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// Past this many starts and models, the oldest read makes room.
const SEEN_CAP: usize = 512;

fn resolved(cache: &Mutex<Seen>, key: Key, read: impl FnOnce() -> Read) -> Arc<Read> {
    let lock = || cache.lock().unwrap_or_else(|e| e.into_inner());
    let known = lock().get(&key).cloned();
    if let Some(known) = known {
        if known.current() {
            return known;
        }
        let mut map = lock();
        if map.get(&key).is_some_and(|entry| Arc::ptr_eq(entry, &known)) {
            map.remove(&key);
        }
    }
    // Files are read with the cache let go. If two readers miss together,
    // the first result kept for this key is the one they both use.
    let fresh = Arc::new(read());
    let mut map = lock();
    let entry = map.entry(key).or_insert(fresh).clone();
    if map.len() > SEEN_CAP {
        if let Some(oldest) = map.iter().min_by_key(|(_, r)| r.at).map(|(key, _)| key.clone()) {
            map.remove(&oldest);
        }
    }
    entry
}

/// The context with its tokens to auto-compaction, where `launch` says
/// whose they are.
pub fn context_of(id: &str, launch: Option<&Launch>, path: &Path, provider: Provider) -> Option<Context> {
    let (mut context, model) = Context::of_with_model(path, provider)?;
    if let Some(launch) = launch.filter(|l| l.provider == provider) {
        context.to_compact = model.and_then(|m| left(id, launch, &m, context.tokens, context.window));
    }
    Some(context)
}

/// What session `id` on `model` can take in past `tokens`, its rollout or
/// transcript stating `window`; None where auto-compaction is off or its
/// threshold cannot be told.
pub fn left(id: &str, launch: &Launch, model: &str, tokens: u64, window: u64) -> Option<ToCompact> {
    let key = (id.to_string(), launch.clone(), model.to_string());
    let read = resolved(seen(), key, || read_inputs(launch, model));
    let limit = match read.inputs.as_ref()? {
        Inputs::Claude(i) => crate::claude::compact::threshold(i, window),
        Inputs::Codex(i) => crate::codex::compact::limit(i, window),
    };
    Some(ToCompact { tokens: limit.saturating_sub(tokens), estimated: true })
}

fn read_inputs(launch: &Launch, model: &str) -> Read {
    let Launch { provider, account_path, folder, helper, .. } = launch;
    let mut read = Read { files: Vec::new(), inputs: None, at: Instant::now() };
    read.inputs = match provider {
        // Its CLI's environment is the core's, which starts it with none of
        // the variables read here.
        Provider::Claude => {
            use crate::claude::{compact, defaults::managed_dir};
            let managed = managed_dir();
            read.track(compact::drop_in_dir(&managed));
            read.track(compact::legacy_file(account_path));
            let files = compact::settings_files(account_path, folder, &managed);
            for file in &files {
                read.track(file.clone());
            }
            compact::inputs(account_path, &files, model, &|name| std::env::var(name).ok()).map(Inputs::Claude)
        }
        Provider::Codex => {
            use crate::codex::{compact, config};
            for path in config::compaction_files(account_path, folder) {
                read.track(path);
            }
            let account_catalog = compact::account_catalog(account_path);
            read.track(account_catalog.clone());
            config::compaction(account_path, folder, *helper).ok().and_then(|settings| {
                if let Some(path) = &settings.model_catalog {
                    read.track(path.clone());
                }
                compact::inputs(&settings, &account_catalog, model).map(Inputs::Codex)
            })
        }
    };
    read
}

/// "~Nk": a figure to auto-compaction as a notice or a tooltip says it, in
/// thousands rounded down, so it never says more room than there is.
pub fn thousands(tokens: u64) -> String {
    format!("~{}k", tokens / 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch(account: &Path, start: u64) -> Launch {
        let path = account.to_string_lossy().into_owned();
        Launch { provider: Provider::Claude, account_path: path.clone(), folder: path, helper: false, start }
    }

    #[test]
    fn a_file_appearing_or_changing_refreshes_the_same_start_and_model() {
        let account = std::env::temp_dir().join(format!("luna-to-compact-{}", std::process::id()));
        std::fs::create_dir_all(&account).unwrap();
        let settings = account.join("settings.json");
        let model = "claude-opus-5-5";
        let left_at = |start: u64, model: &str| left("tc-chat", &launch(&account, start), model, 100_000, 1_000_000);
        assert_eq!(left_at(1, model).map(|t| t.tokens), Some(867_000), "a read before the file exists");
        std::fs::write(&settings, r#"{ "autoCompactWindow": 400000 }"#).unwrap();
        assert_eq!(left_at(1, model), Some(ToCompact { tokens: 400_000 - 33_000 - 100_000, estimated: true }));
        let before = settings.metadata().unwrap().modified().unwrap();
        std::fs::write(&settings, r#"{ "autoCompactWindow": 500000 }"#).unwrap();
        std::fs::File::options().write(true).open(&settings).unwrap().set_modified(before - std::time::Duration::from_secs(5)).unwrap();
        assert_eq!(left_at(1, model).map(|t| t.tokens), Some(367_000), "same length, changed mtime");
        std::fs::write(&settings, r#"{ "autoCompactEnabled": false }"#).unwrap();
        assert_eq!(left_at(1, model), None, "the switch is read again");
        assert_eq!(left_at(1, "claude-sonnet-5"), None, "another model: read again");
        assert_eq!(left_at(2, model), None, "a new start: read again");
        std::fs::write(&settings, "{}").unwrap();
        assert_eq!(left("tc-chat", &launch(&account, 3), model, 990_000, 1_000_000).map(|t| t.tokens), Some(0), "past it: none left");
        let _ = std::fs::remove_dir_all(&account);
    }

    #[test]
    fn a_custom_catalog_and_the_config_that_points_to_it_can_change() {
        let account = std::env::temp_dir().join(format!("luna-to-compact-codex-{}", std::process::id()));
        std::fs::create_dir_all(&account).unwrap();
        let mut launch = launch(&account, 1);
        launch.provider = Provider::Codex;
        let left = || left("tc-codex", &launch, "test-model", 0, 190_000).unwrap().tokens;
        assert_eq!(left(), 180_000);
        std::fs::write(account.join("config.toml"), r#"model_catalog_json = "custom.json""#).unwrap();
        assert_eq!(left(), 180_000);
        let catalog = account.join("custom.json");
        std::fs::write(&catalog, r#"{"models":[{"slug":"test-model","auto_compact_token_limit":50000}]}"#).unwrap();
        assert_eq!(left(), 50_000);
        std::fs::remove_file(&catalog).unwrap();
        assert_eq!(left(), 180_000);
        let _ = std::fs::remove_dir_all(&account);
    }

    #[test]
    fn only_the_oldest_read_is_evicted_and_a_racing_read_keeps_the_winner() {
        let cache = Mutex::new(Seen::new());
        let start = Instant::now();
        let key = |n: usize| (n.to_string(), launch(Path::new("test"), 1), "test-model".into());
        let read = |n| Read { files: vec![], inputs: None, at: start + std::time::Duration::from_millis(n as u64) };
        for n in 0..SEEN_CAP {
            resolved(&cache, key(n), || read(n));
        }
        let latest_key = key(SEEN_CAP);
        let winner = resolved(&cache, latest_key.clone(), || {
            resolved(&cache, latest_key, || read(SEEN_CAP));
            read(SEEN_CAP + 1)
        });
        assert_eq!(winner.at, read(SEEN_CAP).at);
        let map = cache.into_inner().unwrap();
        assert_eq!(map.len(), SEEN_CAP);
        assert!(!map.contains_key(&key(0)));
        assert!(map.contains_key(&key(1)));
    }

    #[test]
    fn a_figure_is_said_in_thousands_rounded_down() {
        assert_eq!(thousands(267_999), "~267k");
        assert_eq!(thousands(999), "~0k");
    }
}
