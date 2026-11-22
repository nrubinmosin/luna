//! Where a Codex session compacts its context on its own, as 0.171.0 works it
//! out for the `total` scope (core/src/session/context_window.rs at
//! rust-v0.171.0; docs/cli-compatibility.md): the configured or catalog
//! limit, at most 90% of the model's window.
//!
//! Read off the config stack with Luna's layer reader (config.rs
//! `compaction`) and the account's catalog (models.rs `catalog_window`).
//! The `body_after_prefix` scope counts against a prefill Luna does not see,
//! and a post-turn percentage compacts on another trigger: neither has a
//! figure here.

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
    let most = window * 9 / 10;
    i.limit.map_or(most, |l| l.min(most))
}

/// The inputs of a session on `account_path` in `folder` running `model`
/// (`helper`: a helper's run); None where its config will not read or sets
/// what this cannot reckon with.
pub(crate) fn inputs(account_path: &str, folder: &str, helper: bool, model: &str) -> Option<Inputs> {
    let config = crate::codex::config::compaction(account_path, folder, helper).ok()?;
    resolve(&config, &crate::codex::models::catalog_window(account_path, model).unwrap_or_default())
}

fn resolve(config: &crate::codex::config::Compaction, catalog: &crate::codex::models::CatalogWindow) -> Option<Inputs> {
    if config.scope.as_deref().is_some_and(|s| s != "total") || config.post_turn_pct.is_some_and(|p| p > 0.0) {
        return None;
    }
    let positive = |n: Option<i64>| n.filter(|n| *n > 0).map(|n| n as u64);
    // A configured window replaces the catalog's, up to the most it takes.
    let window = positive(config.context_window).map(|w| catalog.max_context_window.map_or(w, |max| w.min(max))).or(catalog.context_window);
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
        let i = inputs(&a, &f, true, "gpt-6.1-sol").unwrap();
        assert_eq!(limit(&i, 475_000), 300_000);
        std::fs::write(account.join("config.toml"), "model_auto_compact_token_limit_scope = \"body_after_prefix\"\n").unwrap();
        assert_eq!(inputs(&a, &f, true, "gpt-6.1-sol"), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
