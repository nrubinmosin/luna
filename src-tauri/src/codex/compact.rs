//! Where a Codex session compacts its context on its own, as 0.171.0 works it
//! out for the `total` scope (core/src/session/context_window.rs at
//! rust-v0.171.0; docs/cli-compatibility.md): the configured or catalog
//! limit, at most 90% of the model's window and within its usable window.
//!
//! Read off the config stack with Luna's layer reader (config.rs
//! `compaction`) and the configured or account catalog (models.rs `catalog_window`).
//! The `body_after_prefix` scope counts against a prefill Luna does not see,
//! and a post-turn percentage compacts on another trigger: neither has a
//! figure here.

use crate::codex::{config::Compaction, models};
use std::path::{Path, PathBuf};

/// The share of the window a turn may fill where the catalog does not say.
const EFFECTIVE_PERCENT: u64 = 95;

/// What a session's limit is made of.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Inputs {
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
pub(crate) fn limit(i: &Inputs, usable: u64) -> u64 {
    let window = i.window.unwrap_or_else(|| (usable as f64 * 100.0 / i.effective_percent as f64).round() as u64);
    // Codex 0.171.0 core/src/session/context_window.rs:83-95, 105-110
    // caps the trigger and the reported remainder at the usable window.
    let most = (window - window.div_ceil(10)).min(usable);
    i.limit.map_or(most, |l| l.min(most))
}

pub(crate) fn account_catalog(account_path: &str) -> PathBuf {
    Path::new(account_path).join("models_cache.json")
}

/// The custom catalog replaces the default one when readable, even if it
/// omits this model (Codex 0.171.0 core/src/config/mod.rs:2376-2405, 4445).
pub(crate) fn inputs(config: &Compaction, account_catalog: &Path, model: &str) -> Option<Inputs> {
    let catalog =
        config.model_catalog.as_deref().and_then(models::read_catalog_file).or_else(|| models::read_catalog_file(account_catalog));
    let window = catalog.and_then(|bytes| models::catalog_window(&bytes, model)).unwrap_or_default();
    resolve(config, &window)
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
        assert_eq!(limit(&resolve(&set(2_000_000), &CatalogWindow::default()).unwrap(), 258_400), 258_400, "usable rollout cap");
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
            let cfg = crate::codex::config::compaction(&a, &f, true).unwrap();
            inputs(&cfg, &account_catalog(&a), "gpt-6.1-sol")
        };
        let i = estimate().unwrap();
        assert_eq!(limit(&i, 475_000), 300_000);
        std::fs::write(account.join("config.toml"), "model_catalog_json = \"custom.json\"\n").unwrap();
        assert_eq!(limit(&estimate().unwrap(), 475_000), 450_000, "unreadable custom catalog falls back to the cache");
        std::fs::write(
            account.join("custom.json"),
            r#"{"models":[{"slug":"gpt-6.1-sol","context_window":500000,"auto_compact_token_limit":200000}]}"#,
        )
        .unwrap();
        assert_eq!(limit(&estimate().unwrap(), 475_000), 200_000);
        let big = format!(
            r#"{{"models":[{{"slug":"gpt-6.1-sol","auto_compact_token_limit":12345}}]}}{}"#,
            " ".repeat(crate::boundary::SETTINGS_MAX as usize)
        );
        std::fs::write(account.join("custom.json"), big).unwrap();
        assert_eq!(limit(&estimate().unwrap(), 475_000), 450_000, "an oversized custom catalog is skipped");
        std::fs::write(account.join("config.toml"), "model_auto_compact_token_limit_scope = \"body_after_prefix\"\n").unwrap();
        assert_eq!(estimate(), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
