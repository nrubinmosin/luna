//! Where a Codex session compacts its context on its own, as 0.171.0 works it
//! out for the `total` scope (core/src/session/context_window.rs at
//! rust-v0.171.0; docs/cli-compatibility.md): the configured or catalog
//! limit, at most 90% of the model's window and within its usable window.
//!
//! Read off the config stack with Luna's layer reader (config.rs
//! `compaction`) and the configured or account catalog (models.rs `catalog_windows`).
//! The `body_after_prefix` scope counts against a prefill Luna does not see,
//! and a post-turn percentage compacts on another trigger: neither has a
//! figure here.

use crate::codex::{config, config::Compaction, models};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The share of the window a turn may fill where the catalog does not say.
const EFFECTIVE_PERCENT: u64 = 95;

/// What a session's limit is made of.
#[derive(Clone, Debug, PartialEq)]
struct Inputs {
    /// The model's window, where the config or the catalog says it.
    window: Option<u64>,
    /// The limit they name.
    limit: Option<u64>,
    /// The share of the window the rollout's `model_context_window` is.
    effective_percent: u64,
}

/// The tokens at which a session compacts, `usable` the window its rollout
/// states (the model's, less what a turn may not fill): the window from that
/// where nothing else said it.
fn limit(i: &Inputs, usable: u64) -> u64 {
    let window = i.window.unwrap_or_else(|| (usable as f64 * 100.0 / i.effective_percent as f64).round() as u64);
    // Codex computes the hard cap from the same window as the threshold.
    let full = i.window.map_or(usable, |w| (u128::from(w) * u128::from(i.effective_percent) / 100) as u64);
    let most = (window - window.div_ceil(10)).min(full);
    i.limit.map_or(most, |l| l.min(most))
}

fn account_catalog(account_path: &str) -> PathBuf {
    Path::new(account_path).join("models_cache.json")
}

/// Codex 0.171 retains explicit model overrides across turns and model
/// switches (core/src/session/step_settings.rs, `ModelInfoOverrides`).
/// Keep one launch's inputs with the process, covering every model it may use.
#[derive(Debug)]
pub(crate) struct Startup {
    config: Compaction,
    catalog: HashMap<String, models::CatalogWindow>,
    bundled_0171: bool,
}

pub(crate) fn at_launch(id: &str, account: &str, folder: &str) -> Option<Arc<Startup>> {
    let config = config::compaction(account, folder, crate::agents::codex_chat_trust_key(id, account, folder).as_deref()).ok()?;
    let version = crate::cli::session_version(id);
    Startup::read(config, &account_catalog(account), version.as_deref()).map(Arc::new)
}

impl Startup {
    fn read(config: Compaction, account_catalog: &Path, version: Option<&str>) -> Option<Self> {
        let read = |path: &Path| models::catalog_windows(&models::read_catalog_file(path)?);
        let (catalog, bundled_0171) = match config.model_catalog.as_deref() {
            // An explicit catalog replaces the default. An unreadable one
            // leaves us without its limits, even when the cache is readable.
            Some(path) => (read(&local_catalog_path(path)?).filter(|models| !models.is_empty())?, false),
            None => (read(account_catalog).unwrap_or_default(), version == Some("0.171.0")),
        };
        Some(Self { config, catalog, bundled_0171 })
    }

    pub(crate) fn limit(&self, model: &str, usable: u64) -> Option<u64> {
        let bundled;
        let catalog = match self.catalog.get(model) {
            Some(catalog) => catalog,
            None => {
                // A configured window needs catalog metadata when available.
                // The rollout's usable window does not establish a catalog max.
                bundled = if self.config.model_catalog.is_none() && self.config.context_window.is_some() {
                    self.bundled_0171.then(|| bundled_window(model))?
                } else {
                    models::CatalogWindow::default()
                };
                &bundled
            }
        };
        resolve(&self.config, catalog).map(|i| limit(&i, usable))
    }
}

/// Validate the spelling before any filesystem lookup. Windows accepts both
/// separators, including in share and device prefixes.
fn local_catalog_path(path: &Path) -> Option<PathBuf> {
    let raw = path.to_str()?;
    if raw.chars().take(2).filter(|c| matches!(c, '/' | '\\')).count() == 2 {
        return None;
    }
    let path = crate::boundary::absolute(raw).ok()?;
    #[cfg(windows)]
    if !path.file_name().and_then(|name| name.to_str()).is_some_and(crate::cli::plain_rel) {
        return None;
    }
    Some(path)
}

/// Window metadata in rust-v0.171.0, models-manager/models.json. For an
/// absent slug Codex synthesizes metadata with no max or explicit limit.
fn bundled_window(model: &str) -> models::CatalogWindow {
    let (window, max) = match model {
        "gpt-6.2-sol" | "gpt-6.1-sol" | "gpt-6.1-terra" => (500_000, 872_000),
        "gpt-6.2-astra"
        | "gpt-6-astra"
        | "gpt-6-sol"
        | "gpt-6-luna"
        | "gpt-5.6-sol"
        | "gpt-5.6-terra"
        | "gpt-5.6-luna"
        | "gpt-daybreak-blue-latest"
        | "codex-auto-review" => (272_000, 872_000),
        "gpt-daybreak-red-latest" => (372_000, 372_000),
        "gpt-5.5" => (272_000, 272_000),
        _ => return models::CatalogWindow::default(),
    };
    models::CatalogWindow { context_window: Some(window), max_context_window: Some(max), ..Default::default() }
}

fn resolve(config: &Compaction, catalog: &models::CatalogWindow) -> Option<Inputs> {
    if config.scope.as_deref().is_some_and(|s| s != "total") || config.post_turn_pct.is_some_and(|p| p > 0.0) {
        return None;
    }
    let positive = |n: Option<i64>| n.filter(|n| *n > 0).map(|n| n as u64);
    // A configured window replaces the catalog's, up to the most it takes.
    let window = positive(config.context_window)
        .map(|w| catalog.max_context_window.map_or(w, |max| w.min(max)))
        .or(catalog.context_window)
        .or(catalog.max_context_window);
    Some(Inputs {
        window,
        limit: positive(config.token_limit).or(catalog.auto_compact_token_limit),
        effective_percent: catalog.effective_percent.unwrap_or(EFFECTIVE_PERCENT),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codex::config::Compaction;
    use crate::codex::models::CatalogWindow;

    fn catalog(window: u64) -> CatalogWindow {
        CatalogWindow { context_window: Some(window), max_context_window: Some(872_000), effective_percent: Some(95), ..Default::default() }
    }

    #[test]
    fn nothing_set_compacts_at_90_percent_of_the_catalogs_window() {
        let i = resolve(&Compaction::default(), &catalog(272_000)).unwrap();
        assert_eq!(limit(&i, 258_400), 244_800);
        let i = resolve(&Compaction::default(), &catalog(500_000)).unwrap();
        assert_eq!(limit(&i, 475_000), 450_000);
    }

    #[test]
    fn without_a_catalog_the_window_is_the_rollouts_less_its_share() {
        let i = resolve(&Compaction::default(), &CatalogWindow::default()).unwrap();
        assert_eq!(limit(&i, 258_400), 244_800);
    }

    #[test]
    fn a_configured_limit_counts_up_to_90_percent_of_the_window() {
        let set = |limit: i64| Compaction { token_limit: Some(limit), ..Default::default() };
        assert_eq!(limit(&resolve(&set(200_000), &catalog(272_000)).unwrap(), 258_400), 200_000);
        assert_eq!(limit(&resolve(&set(260_000), &catalog(272_000)).unwrap(), 258_400), 244_800, "0.9 of the window at most");
        let in_catalog = CatalogWindow { auto_compact_token_limit: Some(150_000), ..catalog(272_000) };
        assert_eq!(limit(&resolve(&Compaction::default(), &in_catalog).unwrap(), 258_400), 150_000, "the catalog's");
        assert_eq!(limit(&resolve(&set(180_000), &in_catalog).unwrap(), 258_400), 180_000, "the config's over it");
    }

    #[test]
    fn a_configured_window_replaces_the_catalogs_up_to_its_most() {
        let set = |window: i64| Compaction { context_window: Some(window), ..Default::default() };
        assert_eq!(limit(&resolve(&set(400_000), &catalog(272_000)).unwrap(), 380_000), 360_000);
        assert_eq!(limit(&resolve(&set(2_000_000), &catalog(272_000)).unwrap(), 828_400), 784_800, "the catalog's most");
        let lower = CatalogWindow { effective_percent: Some(80), ..catalog(272_000) };
        assert_eq!(limit(&resolve(&set(400_000), &lower).unwrap(), 258_400), 320_000, "full-window cap from the configured window");
    }

    #[test]
    fn without_a_cache_the_configured_window_is_capped_by_the_bundled_catalog() {
        let cache = std::env::temp_dir().join(format!("luna-absent-models-cache-{}", std::process::id()));
        let config = Compaction { context_window: Some(2_000_000), ..Default::default() };
        let start = Startup::read(config, &cache, Some("0.171.0")).unwrap();
        assert_eq!(start.limit("gpt-5.5", 258_400), Some(244_800));
        // Same stock window, different catalog max. The rollout cannot tell them apart.
        assert_eq!(start.limit("gpt-6-luna", 258_400), Some(784_800));
    }

    #[test]
    fn a_scope_or_trigger_it_cannot_reckon_with_has_no_limit() {
        let body = Compaction { scope: Some("body_after_prefix".into()), ..Default::default() };
        assert_eq!(resolve(&body, &catalog(272_000)), None);
        let total = Compaction { scope: Some("total".into()), ..Default::default() };
        assert!(resolve(&total, &catalog(272_000)).is_some());
        let post_turn = Compaction { post_turn_pct: Some(80.0), ..Default::default() };
        assert_eq!(resolve(&post_turn, &catalog(272_000)), None);
        let off = Compaction { post_turn_pct: Some(0.0), ..Default::default() };
        assert!(resolve(&off, &catalog(272_000)).is_some(), "0: no post-turn trigger");
    }

    #[test]
    fn the_config_stack_and_the_catalog_are_read_where_codex_reads_them() {
        let root = std::env::temp_dir().join(format!("luna-compact-codex-{}", std::process::id()));
        let (account, folder) = (root.join("account"), root.join("project"));
        std::fs::create_dir_all(&account).unwrap();
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(account.join("config.toml"), "model_auto_compact_token_limit = 300000\n").unwrap();
        std::fs::write(
            account.join("models_cache.json"),
            r#"{"models":[{"slug":"gpt-6.1-sol","context_window":500000,"max_context_window":872000,"effective_context_window_percent":95}]}"#,
        )
        .unwrap();
        let (a, f) = (account.to_string_lossy().into_owned(), folder.to_string_lossy().into_owned());
        let estimate = || {
            let cfg = config::compaction(&a, &f, None).unwrap();
            Startup::read(cfg, &account_catalog(&a), Some("0.171.0"))?.limit("gpt-6.1-sol", 475_000)
        };
        assert_eq!(estimate(), Some(300_000));
        let custom = account.join("custom.json");
        let set_catalog = |path: &str| {
            let path = toml_edit::Value::from(path);
            std::fs::write(account.join("config.toml"), format!("model_catalog_json = {path}\n")).unwrap();
        };
        set_catalog("custom.json");
        std::fs::write(&custom, r#"{"models":[{"slug":"gpt-6.1-sol","auto_compact_token_limit":200000}]}"#).unwrap();
        assert_eq!(estimate(), None, "a relative path is refused even beside the config");
        std::fs::remove_file(&custom).unwrap();
        set_catalog(custom.to_str().unwrap());
        assert_eq!(estimate(), None, "an unreadable custom catalog suppresses the estimate");
        std::fs::write(&custom, r#"{"models":[{"slug":"gpt-6.1-sol","context_window":500000,"auto_compact_token_limit":200000}]}"#)
            .unwrap();
        assert_eq!(estimate(), Some(200_000));
        let big = format!(
            r#"{{"models":[{{"slug":"gpt-6.1-sol","auto_compact_token_limit":12345}}]}}{}"#,
            " ".repeat(crate::boundary::SETTINGS_MAX as usize)
        );
        std::fs::write(&custom, big).unwrap();
        assert_eq!(estimate(), None, "an oversized custom catalog suppresses the estimate");
        std::fs::write(account.join("config.toml"), "model_auto_compact_token_limit_scope = \"body_after_prefix\"\n").unwrap();
        assert_eq!(estimate(), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn catalog_paths_refuse_share_and_device_prefixes_before_a_read() {
        for raw in
            [r"\\server\share\models.json", "//server/share/models.json", r"\\?\C:\models.json", r"\\.\PhysicalDrive0", r"/\server\share"]
        {
            assert!(local_catalog_path(Path::new(raw)).is_none(), "{raw}");
        }
        #[cfg(windows)]
        assert!(local_catalog_path(Path::new(r"C:\catalogs\NUL.json")).is_none());
    }

    #[test]
    fn the_project_catalog_needs_codex_trust() {
        let root = std::env::temp_dir().join(format!("luna-compact-trust-{}", std::process::id()));
        let (account, project) = (root.join("account"), root.join("project"));
        std::fs::create_dir_all(&account).unwrap();
        std::fs::create_dir_all(project.join(".codex")).unwrap();
        std::fs::write(project.join(".codex/config.toml"), "model_catalog_json = 'project.json'\n").unwrap();
        let (account, project) = (account.to_str().unwrap(), project.to_str().unwrap());
        let config = || config::compaction(account, project, None).unwrap().model_catalog;
        assert_eq!(config(), None, "undecided");
        let key = toml_edit::Value::from(project);
        let file = Path::new(account).join("config.toml");
        std::fs::write(&file, format!("[projects.{key}]\ntrust_level = 'untrusted'\n")).unwrap();
        assert_eq!(config(), None, "untrusted");
        std::fs::write(&file, format!("[projects.{key}]\ntrust_level = 'trusted'\n")).unwrap();
        assert_eq!(config(), Some(PathBuf::from("project.json")), "trusted");
        let _ = std::fs::remove_dir_all(&root);
    }
}
