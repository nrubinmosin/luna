//! Where a Claude Code session compacts its context on its own, as 2.1.330
//! works it out: from the window its settings and environment give the
//! model, the output it holds back, and a percentage that may lower it
//! (`wS`, `rne`, `SY`, `rm` in its bundle; docs/cli-compatibility.md).
//!
//! Read off the same files the CLI reads, with Luna's settings reader
//! (defaults.rs `read_settings`). What the CLI is served (a model's tuned
//! default window, an experiment's) is no file: a window nothing here names
//! is the model's own, which makes the figure an estimate.

use crate::claude::defaults::canonical;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The least window the CLI takes from a setting or its variable (`uH`).
const MIN_WINDOW: u64 = 100_000;
/// The most output a turn holds back from the window (`iQr`).
const OUTPUT_RESERVE: u64 = 20_000;
/// How far below the effective window it compacts (`_Gn`).
const BELOW: u64 = 13_000;

/// What a session's threshold is made of, but the model's window: that comes
/// with each usage sample (`threshold`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Inputs {
    /// The window its variable or settings name, None for the model's own.
    window: Option<u64>,
    /// The output held back from it.
    reserve: u64,
    /// `CLAUDE_AUTOCOMPACT_PCT_OVERRIDE`, where it takes.
    pct: Option<f64>,
}

/// The tokens at which a session on a model with `model_window` compacts.
pub(crate) fn threshold(i: &Inputs, model_window: u64) -> u64 {
    let window = i.window.map_or(model_window, |w| w.min(model_window));
    let effective = window.saturating_sub(i.reserve);
    let at = effective.saturating_sub(BELOW);
    match i.pct {
        Some(p) => at.min((effective as f64 * (p / 100.0)).floor() as u64),
        None => at,
    }
}

/// The settings layers lowest first, as the CLI ascends them: the
/// account's (its user scope, the `CLAUDE_CONFIG_DIR` it runs with), the
/// project's two, then the managed files. Luna's own `--settings` sits between
/// the project and managed settings and names none of the keys read here (launch.rs
/// `claude_settings`).
pub(crate) fn settings_files(account_path: &str, folder: &str, managed: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut files = vec![Path::new(account_path).join("settings.json")];
    if !folder.is_empty() {
        let dot = Path::new(folder).join(".claude");
        files.extend([dot.join("settings.json"), dot.join("settings.local.json")]);
    }
    let mut policy = vec![managed.join("managed-settings.json")];
    // 2.1.330 loads the base first, then sorts visible JSON drop-ins by name.
    let mut drop_ins: Vec<_> = std::fs::read_dir(drop_in_dir(managed))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let kind = entry.file_type().ok()?;
            let name = entry.file_name().into_string().ok()?;
            ((kind.is_file() || kind.is_symlink()) && name.ends_with(".json") && !name.starts_with('.')).then_some((name, entry.path()))
        })
        .collect();
    drop_ins.sort_by(|(a, _), (b, _)| a.encode_utf16().cmp(b.encode_utf16()));
    policy.extend(drop_ins.into_iter().map(|(_, path)| path));
    (files, policy)
}

fn drop_in_dir(managed: &Path) -> PathBuf {
    managed.join("managed-settings.d")
}

/// The inputs of a session running `model`, with its inherited `env`; None
/// where auto-compaction is off.
pub(crate) fn inputs(
    account_path: &str,
    mut layers: Vec<Value>,
    policy: impl IntoIterator<Item = Value>,
    model: &str,
    env: &dyn Fn(&str) -> Option<String>,
    legacy: Option<Value>,
) -> Option<Inputs> {
    let mut managed = Value::Null;
    for layer in policy {
        merge_policy(&mut managed, layer);
    }
    // In 2.1.330 the managed base and drop-ins reach Kxn as one policy
    // layer, so a per-model value survives a global value in a drop-in.
    layers.push(managed);
    let models = crate::claude::models::models_known(account_path);
    let legacy = || legacy.as_ref()?.get("autoCompactEnabled")?.as_bool();
    resolve(&layers, &|raw| canonical(&models, raw), &canonical(&models, model), env, &legacy)
}

fn merge_policy(base: &mut Value, over: Value) {
    match (base, over) {
        (Value::Object(base), Value::Object(over)) => {
            for (key, value) in over {
                merge_policy(base.entry(key).or_insert(Value::Null), value);
            }
        }
        (base, over) => *base = over,
    }
}

/// `inputs` over settings already read, lowest first: `key` spells a model
/// as the CLI keys its settings by, `legacy` is the account's
/// `.claude.json` word on auto-compaction.
fn resolve(
    layers: &[Value],
    key: &dyn Fn(&str) -> String,
    model: &str,
    env: &dyn Fn(&str) -> Option<String>,
    legacy: &dyn Fn() -> Option<bool>,
) -> Option<Inputs> {
    // A settings file's `env` is set over what the CLI inherited, the
    // highest file's last.
    let var = |name: &str| {
        let set = layers.iter().rev().find_map(|l| l.get("env")?.get(name).and_then(env_value));
        set.or_else(|| env(name))
    };
    if truthy(var("DISABLE_COMPACT")) || truthy(var("DISABLE_AUTO_COMPACT")) {
        return None;
    }
    // The highest file that says, else the account's state from before the
    // setting moved into the files (only its `false` differs from the stock).
    let enabled = layers.iter().rev().find_map(|l| l.get("autoCompactEnabled")?.as_bool()).or_else(legacy).unwrap_or(true);
    if !enabled {
        return None;
    }
    let window = var(AUTO_COMPACT_WINDOW_ENV).and_then(|v| env_window(&v)).or_else(|| settings_window(layers, key, model));
    // The small fixed limits from kw and the built-in model table in 2.1.330.
    let model_reserve = match model {
        "claude-3-haiku" | "claude-3-opus" => 4096,
        "claude-3-sonnet" | "claude-3-5-haiku" | "claude-3-5-sonnet" => 8192,
        _ => OUTPUT_RESERVE,
    };
    let reserve = var("CLAUDE_CODE_MAX_OUTPUT_TOKENS")
        .and_then(|v| grouped_int(&v))
        .filter(|n| *n > 0.0)
        .map_or(model_reserve, |n| (n as u64).min(model_reserve));
    let pct = var("CLAUDE_AUTOCOMPACT_PCT_OVERRIDE").and_then(|v| js_float(&v)).filter(|p| *p > 0.0 && *p <= 100.0);
    Some(Inputs { window, reserve, pct })
}

/// A window a setting names for `model` (`Kxn`): from the lowest file up,
/// one's top-level `autoCompactWindow` replaces what was said before it,
/// per-model entries and all, while its `modelSettings.<model>` entries are
/// laid over those below; within one file an entry for the model wins over
/// its top-level value. `auto`: the model's own.
fn settings_window(layers: &[Value], key: &dyn Fn(&str) -> String, model: &str) -> Option<u64> {
    #[derive(Clone, Copy)]
    enum Said {
        Auto,
        Tokens(u64),
    }
    let mut top: Option<Said> = None;
    let mut by_model: HashMap<String, Said> = HashMap::new();
    for layer in layers {
        let mut here: HashMap<String, Said> = HashMap::new();
        for (name, entry) in layer.get("modelSettings").and_then(Value::as_object).into_iter().flatten() {
            let said = match entry.get("autoCompactWindow") {
                Some(Value::String(s)) if s == "auto" => Said::Auto,
                Some(v) => match window_setting(v) {
                    Some(n) => Said::Tokens(n),
                    None => continue,
                },
                None => continue,
            };
            // Two spellings of one model: the canonical one wins, else the first.
            let canonical = key(name);
            if *name == canonical || !here.contains_key(&canonical) {
                here.insert(canonical, said);
            }
        }
        match layer.get("autoCompactWindow").and_then(window_setting) {
            Some(n) => (top, by_model) = (Some(Said::Tokens(n)), here),
            None => by_model.extend(here),
        }
    }
    match by_model.get(model).copied().or(top)? {
        Said::Tokens(n) => Some(n),
        Said::Auto => None,
    }
}

/// A setting's window: a whole number of tokens, at least `MIN_WINDOW`; any
/// other the CLI drops as unset.
fn window_setting(v: &Value) -> Option<u64> {
    let n = v.as_f64().filter(|n| n.fract() == 0.0 && *n >= MIN_WINDOW as f64)?;
    Some(n as u64)
}

pub(crate) const AUTO_COMPACT_WINDOW_ENV: &str = "CLAUDE_CODE_AUTO_COMPACT_WINDOW";

/// `CLAUDE_CODE_AUTO_COMPACT_WINDOW` as the CLI reads it (`sBe`, `gGn`):
/// tokens (including grouped integers), or a number with `k` or `M`; one below the least is the least,
/// one that is no size is ignored.
fn env_window(raw: &str) -> Option<u64> {
    let v = raw.trim();
    let scale = match v.chars().last().map(|c| c.to_ascii_lowercase()) {
        Some('m') => Some(1e6),
        Some('k') => Some(1e3),
        _ => None,
    };
    let n = match scale {
        Some(scale) => js_float(v)? * scale,
        None => grouped_int(v)?,
    };
    if !n.is_finite() || n < 1.0 || n.round() > 9_007_199_254_740_991.0 {
        return None;
    }
    Some((n.round() as u64).max(MIN_WINDOW))
}

/// Claude's `Ch` recognizes whole scientific notation and consistently
/// grouped thousands before falling back to parseInt.
fn grouped_int(v: &str) -> Option<f64> {
    let v = v.trim();
    if v.encode_utf16().count() <= 32 {
        if v.contains(['e', 'E']) {
            if let Ok(n) = v.parse::<f64>() {
                return (n.fract() == 0.0).then_some(n);
            }
        }
        let digits = v.strip_prefix(['+', '-']).unwrap_or(v);
        let separators = ['_', ',', '\u{00a0}', '\u{202f}', ' '];
        if let Some(sep) = digits.chars().find(|c| separators.contains(c)) {
            let mut groups = digits.split(sep);
            let first = groups.next().unwrap_or_default();
            if (1..=3).contains(&first.len())
                && first.bytes().all(|b| b.is_ascii_digit())
                && groups.all(|g| g.len() == 3 && g.bytes().all(|b| b.is_ascii_digit()))
            {
                return v.replace(sep, "").parse().ok();
            }
        }
    }
    js_int(v)
}

/// A settings file's `env` value as the variable it sets.
fn env_value(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// A switch the CLI reads as on (`Fe`).
fn truthy(v: Option<String>) -> bool {
    v.is_some_and(|v| ["1", "true", "yes", "on"].contains(&v.trim().to_ascii_lowercase().as_str()))
}

/// JavaScript's `parseInt(v, 10)`: the whole number a string starts with.
fn js_int(v: &str) -> Option<f64> {
    let v = v.trim_start();
    let (sign, digits) = match v.as_bytes().first() {
        Some(b'-') => (-1.0, &v[1..]),
        Some(b'+') => (1.0, &v[1..]),
        _ => (1.0, v),
    };
    let end = digits.bytes().take_while(u8::is_ascii_digit).count();
    digits[..end].parse::<f64>().ok().map(|n| sign * n)
}

/// JavaScript's `parseFloat`: the decimal number a string starts with.
fn js_float(v: &str) -> Option<f64> {
    let v = v.trim_start();
    let b = v.as_bytes();
    let mut end = usize::from(matches!(b.first(), Some(b'-' | b'+')));
    let int = b[end..].iter().take_while(|c| c.is_ascii_digit()).count();
    end += int;
    let mut frac = 0;
    if b.get(end) == Some(&b'.') {
        frac = b[end + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
        if int + frac > 0 {
            end += 1 + frac;
        }
    }
    if int + frac == 0 {
        return None;
    }
    if matches!(b.get(end), Some(b'e' | b'E')) {
        let mut at = end + 1;
        at += usize::from(matches!(b.get(at), Some(b'-' | b'+')));
        let exp = b[at..].iter().take_while(|c| c.is_ascii_digit()).count();
        if exp > 0 {
            end = at + exp;
        }
    }
    v[..end].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::defaults::read_settings;
    use serde_json::json;

    const MODEL: &str = "claude-opus-5-5";

    fn same(raw: &str) -> String {
        raw.to_lowercase()
    }

    fn of(layers: &[Value], env: &[(&str, &str)]) -> Option<Inputs> {
        let env: HashMap<String, String> = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        resolve(layers, &same, MODEL, &|name| env.get(name).cloned(), &|| None)
    }

    fn at(layers: &[Value], env: &[(&str, &str)], model_window: u64) -> Option<u64> {
        of(layers, env).map(|i| threshold(&i, model_window))
    }

    #[test]
    fn nothing_said_compacts_13k_under_the_window_less_the_output_reserve() {
        assert_eq!(at(&[], &[], 1_000_000), Some(1_000_000 - 20_000 - 13_000));
        assert_eq!(at(&[], &[], 200_000), Some(167_000));
    }

    #[test]
    fn the_variable_names_the_window_over_the_settings_and_below_the_model() {
        let settings = [json!({ "autoCompactWindow": 400_000 })];
        assert_eq!(at(&settings, &[("CLAUDE_CODE_AUTO_COMPACT_WINDOW", "300k")], 1_000_000), Some(300_000 - 33_000));
        assert_eq!(at(&settings, &[("CLAUDE_CODE_AUTO_COMPACT_WINDOW", "250000")], 1_000_000), Some(250_000 - 33_000));
        assert_eq!(
            at(&settings, &[("CLAUDE_CODE_AUTO_COMPACT_WINDOW", "1.5M")], 1_000_000),
            Some(1_000_000 - 33_000),
            "the model's at most"
        );
        assert_eq!(at(&settings, &[("CLAUDE_CODE_AUTO_COMPACT_WINDOW", "50000")], 1_000_000), Some(100_000 - 33_000), "the least it takes");
        assert_eq!(
            at(&settings, &[("CLAUDE_CODE_AUTO_COMPACT_WINDOW", "lots")], 1_000_000),
            Some(400_000 - 33_000),
            "no size: the settings'"
        );
        // A settings file's `env` sets it as well, over what was inherited.
        let env_block = [json!({ "env": { "CLAUDE_CODE_AUTO_COMPACT_WINDOW": "200k" } })];
        assert_eq!(at(&env_block, &[("CLAUDE_CODE_AUTO_COMPACT_WINDOW", "600k")], 1_000_000), Some(200_000 - 33_000));
    }

    #[test]
    fn a_files_entry_for_the_model_wins_in_it_and_a_higher_top_level_one_clears_those_below() {
        let user = json!({ "autoCompactWindow": 500_000, "modelSettings": { MODEL: { "autoCompactWindow": 300_000 } } });
        assert_eq!(at(std::slice::from_ref(&user), &[], 1_000_000), Some(300_000 - 33_000), "per model first in a file");
        let project = json!({ "modelSettings": { "claude-sonnet-5": { "autoCompactWindow": 200_000 } } });
        assert_eq!(at(&[user.clone(), project], &[], 1_000_000), Some(300_000 - 33_000), "another model's entry above leaves it");
        let local = json!({ "autoCompactWindow": 700_000 });
        assert_eq!(at(&[user.clone(), local], &[], 1_000_000), Some(700_000 - 33_000), "a top-level one above clears it");
        let auto = json!({ "modelSettings": { MODEL: { "autoCompactWindow": "auto" } } });
        assert_eq!(at(&[user, auto], &[], 1_000_000), Some(1_000_000 - 33_000), "auto: the model's own");
        let small = json!({ "autoCompactWindow": 50_000 });
        assert_eq!(at(&[small], &[], 1_000_000), Some(1_000_000 - 33_000), "below the least: unset");
    }

    #[test]
    fn the_percentage_lowers_the_threshold_and_never_raises_it() {
        let pct = |p: &str| at(&[], &[("CLAUDE_AUTOCOMPACT_PCT_OVERRIDE", p)], 200_000);
        assert_eq!(pct("50"), Some(90_000), "half of the effective 180k");
        assert_eq!(pct("35"), Some(62_999), "divide the percentage before multiplying");
        assert_eq!(pct("99"), Some(167_000), "13k under it stays the most");
        assert_eq!(pct("0"), Some(167_000), "out of range: none");
        assert_eq!(pct("150"), Some(167_000));
    }

    #[test]
    fn a_lower_output_cap_holds_back_less() {
        assert_eq!(at(&[], &[("CLAUDE_CODE_MAX_OUTPUT_TOKENS", "8000")], 200_000), Some(200_000 - 8_000 - 13_000));
        assert_eq!(at(&[], &[("CLAUDE_CODE_MAX_OUTPUT_TOKENS", "8_000")], 200_000), Some(200_000 - 8_000 - 13_000));
        assert_eq!(at(&[], &[("CLAUDE_CODE_MAX_OUTPUT_TOKENS", "64000")], 200_000), Some(167_000));
        assert_eq!(at(&[], &[("CLAUDE_CODE_MAX_OUTPUT_TOKENS", "x")], 200_000), Some(167_000));
    }

    #[test]
    fn legacy_models_reserve_only_the_output_they_can_make() {
        let at = |model, limit: &str| {
            let inputs =
                resolve(&[], &same, model, &|name| (name == "CLAUDE_CODE_MAX_OUTPUT_TOKENS").then(|| limit.to_owned()), &|| None).unwrap();
            threshold(&inputs, 200_000)
        };
        assert_eq!(at("claude-3-haiku", "64000"), 182_904);
        assert_eq!(at("claude-3-haiku", "2048"), 184_952);
        assert_eq!(at("claude-3-5-haiku", ""), 178_808);
    }

    #[test]
    fn the_window_parser_takes_grouped_and_scientific_integers() {
        for value in ["250_000", "250,000", "250 000", "250\u{00a0}000", "250\u{202f}000", "+2.5e5"] {
            assert_eq!(env_window(value), Some(250_000), "{value}");
        }
        assert_eq!(env_window("1_250,000"), Some(MIN_WINDOW), "mixed separators fall back to parseInt");
        assert_eq!(env_window("2.5e0"), None, "scientific notation must give an integer");
    }

    #[test]
    fn auto_compaction_off_has_no_threshold() {
        assert_eq!(of(&[], &[("DISABLE_AUTO_COMPACT", "1")]), None);
        assert_eq!(of(&[], &[("DISABLE_COMPACT", "true")]), None);
        assert!(of(&[], &[("DISABLE_AUTO_COMPACT", "0")]).is_some());
        assert_eq!(of(&[json!({ "env": { "DISABLE_AUTO_COMPACT": "1" } })], &[]), None, "a file's env");
        assert_eq!(of(&[json!({ "autoCompactEnabled": false })], &[]), None);
        let on_above = [json!({ "autoCompactEnabled": false }), json!({ "autoCompactEnabled": true })];
        assert!(of(&on_above, &[]).is_some(), "the highest file says");
        let off_before = resolve(&[], &same, MODEL, &|_| None, &|| Some(false));
        assert_eq!(off_before, None, "the account's state from before the setting");
        let said_since = resolve(&[json!({ "autoCompactEnabled": true })], &same, MODEL, &|_| None, &|| Some(false));
        assert!(said_since.is_some(), "a file over it");
    }

    #[test]
    fn the_files_are_read_where_the_cli_reads_them() {
        let root = std::env::temp_dir().join(format!("luna-compact-claude-{}", std::process::id()));
        let (account, folder) = (root.join("account"), root.join("project"));
        let managed = root.join("managed");
        std::fs::create_dir_all(folder.join(".claude")).unwrap();
        std::fs::create_dir_all(&account).unwrap();
        std::fs::write(account.join("settings.json"), r#"{ "autoCompactWindow": 500000 }"#).unwrap();
        std::fs::write(
            folder.join(".claude").join("settings.local.json"),
            r#"{ "modelSettings": { "claude-opus-5-5": { "autoCompactWindow": 400000 } } }"#,
        )
        .unwrap();
        let (a, f) = (account.to_string_lossy().into_owned(), folder.to_string_lossy().into_owned());
        let legacy = account.join(".claude.json");
        let get = |model| {
            let (layers, policy) = settings_files(&a, &f, &managed);
            inputs(
                &a,
                layers.iter().filter_map(|file| read_settings(file)).collect(),
                policy.iter().filter_map(|file| read_settings(file)),
                model,
                &|_| None,
                read_settings(&legacy),
            )
        };
        let i = get("claude-opus-5-5-20261101").unwrap();
        assert_eq!(threshold(&i, 1_000_000), 400_000 - 33_000, "a dated id is its model");
        std::fs::write(account.join(".claude.json"), r#"{ "autoCompactEnabled": false }"#).unwrap();
        assert_eq!(get(MODEL), None);
        // The oversized legacy file does not set the flag, even with valid JSON.
        let big = format!(r#"{{ "autoCompactEnabled": false }}{}"#, " ".repeat(crate::boundary::SETTINGS_MAX as usize));
        std::fs::write(&legacy, big).unwrap();
        assert!(get(MODEL).is_some());

        let drops = drop_in_dir(&managed);
        std::fs::create_dir_all(&drops).unwrap();
        std::fs::write(managed.join("managed-settings.json"), r#"{"autoCompactWindow":310000,"autoCompactEnabled":false}"#).unwrap();
        std::fs::write(drops.join("30-window.json"), r#"{"env":{"DISABLE_COMPACT":"0"},"autoCompactWindow":250000}"#).unwrap();
        std::fs::write(drops.join("10-disabled.json"), r#"{"env":{"DISABLE_COMPACT":"1"},"autoCompactEnabled":true}"#).unwrap();
        std::fs::write(drops.join(".hidden.json"), r#"{"autoCompactEnabled":false}"#).unwrap();
        std::fs::write(drops.join("99-ignored.JSON"), r#"{"autoCompactEnabled":false}"#).unwrap();
        assert_eq!(get(MODEL).map(|i| threshold(&i, 1_000_000)), Some(217_000));
        std::fs::write(managed.join("managed-settings.json"), r#"{"modelSettings":{"claude-opus-5-5":{"autoCompactWindow":250000}}}"#)
            .unwrap();
        std::fs::write(drops.join("30-window.json"), r#"{"env":{"DISABLE_COMPACT":"0"},"autoCompactWindow":400000}"#).unwrap();
        assert_eq!(get(MODEL).map(|i| threshold(&i, 1_000_000)), Some(250_000 - 33_000));
        std::fs::remove_file(drops.join("30-window.json")).unwrap();
        assert_eq!(get(MODEL), None, "a drop-in can disable compaction");
        let _ = std::fs::remove_dir_all(&root);
    }
}
