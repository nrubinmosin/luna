//! Where a new chat's model, effort and permission mode come from when nobody
//! has said otherwise: Claude Code's own settings files, read in the CLI's own
//! precedence order.
//!
//! Luna always spawns the CLI with `--model/--effort/--permission-mode`, so
//! whatever the settings say would otherwise be overridden on every session.
//! Reading them here is what makes those flags agree with the files instead of
//! fighting them — the new-chat dialog opens on these values, and only a
//! deliberate change in the dialog departs from them.
//!
//! The values come out in the CLI's own spelling and already settled: one
//! Luna has no equivalent for reads as nothing said, and nothing said is
//! Luna's stock choice (`source: stock`). The dialog, a spawn and
//! `luna_account_get` all take them from here, so what an agent is told it would
//! get is what it gets.
//!
//! Effort is the CLI's per model (2.1.322, `effort_for` below): `/effort`
//! writes `modelSettings.<model>.effortLevel`, and the account's top-level
//! `effortLevel` speaks only for older models. So it is resolved for each of
//! the account's models, and a model nothing names one for runs at its own
//! default, as the CLI would have it.

use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One resolved value and the file it came from, so the dialog can say why it
/// is showing what it is showing.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Setting {
    pub value: String,
    /// `account` | `project` | `project-local` | `managed` | `stock`; for an
    /// effort also `model`, the model's own default in its catalog.
    pub source: String,
    /// The key it was read from where that is not the field's plain one: an
    /// effort's `modelSettings.<model>.effortLevel`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

impl Setting {
    fn new(value: impl Into<String>, source: &str) -> Setting {
        Setting { value: value.into(), source: source.to_owned(), key: None }
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Defaults {
    pub model: Setting,
    /// The effort of `model`.
    pub effort: Setting,
    pub permission_mode: Setting,
    /// The effort each of the account's models starts on, by slug: what the
    /// dialog moves to when another model is picked.
    pub effort_by_model: BTreeMap<String, Setting>,
}

/// What a chat starts with when no file says anything Luna can use.
const STOCK_MODEL: &str = "opus";
const STOCK_EFFORT: &str = "high";
const STOCK_PERMISSION_MODE: &str = "bypassPermissions";

/// The levels a settings file may name (2.1.322's schema: `effortLevel` is one
/// of these or nothing; `max` and `ultracode` are a session's only).
const SETTINGS_EFFORTS: [&str; 4] = ["low", "medium", "high", "xhigh"];
const PERMISSION_MODES: [&str; 4] = ["default", "acceptEdits", "plan", "bypassPermissions"];

/// The models the account's top-level `effortLevel` still speaks for, by their
/// canonical names (2.1.322's list; any other model it reads as named by none).
const LEGACY_MODELS: [&str; 19] = [
    "claude-3-5-haiku",
    "claude-3-5-sonnet",
    "claude-3-7-sonnet",
    "claude-haiku-4-5",
    "claude-sonnet-4-0",
    "claude-sonnet-4-5",
    "claude-sonnet-4-6",
    "claude-sonnet-5",
    "claude-opus-4-0",
    "claude-opus-4-1",
    "claude-opus-4-5",
    "claude-opus-4-6",
    "claude-opus-4-7",
    "claude-opus-4-8",
    "claude-opus-5",
    "claude-fable-5",
    "claude-fable-5-1",
    "claude-mythos-5",
    "claude-mythos-5-1",
];

/// `model` as one of the account's models (claude/models.rs): a slug or full
/// id it lists is that model, by the slug Luna passes on. Otherwise an alias
/// (`opus`), a compound one (`opusplan`) or an id the list does not carry
/// (`claude-sonnet-5[1m]`) names its family plainly enough to match on.
fn model_in(models: &[crate::claude::models::ClaudeModel], raw: &str) -> Option<String> {
    let listed =
        models.iter().find(|m| m.slug.eq_ignore_ascii_case(raw) || m.model_id.as_deref().is_some_and(|id| id.eq_ignore_ascii_case(raw)));
    if let Some(m) = listed {
        return Some(m.slug.clone());
    }
    let v = raw.to_lowercase();
    let families = models.iter().map(|m| m.slug.as_str()).filter(|s| !s.contains('-'));
    families.chain(crate::claude::models::ALIASES).find(|a| v.contains(*a)).map(str::to_owned)
}

fn effort_level(raw: &str) -> Option<String> {
    SETTINGS_EFFORTS.iter().find(|e| **e == raw).map(|e| (*e).to_owned())
}

fn permission_mode(raw: &str) -> Option<String> {
    PERMISSION_MODES.iter().find(|m| **m == raw).map(|m| (*m).to_owned())
}

/// The value the files settled on, in the spelling Luna passes on, or the
/// stock one when they said nothing usable.
fn settle(said: Option<Setting>, usable: impl Fn(&str) -> Option<String>, stock: &str) -> Setting {
    said.and_then(|s| usable(&s.value).map(|value| Setting { value, ..s })).unwrap_or_else(|| Setting::new(stock, "stock"))
}

fn read_object(path: &Path) -> Option<Value> {
    let raw = std::fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&raw).ok()?;
    value.is_object().then_some(value)
}

/// The folder of the machine-wide files an administrator can put down, as
/// Claude Code 2.1.318 has it: fixed paths, no environment involved. Highest
/// precedence in the CLI, so its file is read last below.
pub(crate) fn managed_dir() -> PathBuf {
    PathBuf::from(if cfg!(windows) {
        r"C:\Program Files\ClaudeCode"
    } else if cfg!(target_os = "macos") {
        "/Library/Application Support/ClaudeCode"
    } else {
        "/etc/claude-code"
    })
}

fn take(source: &str, raw: Option<&Value>) -> Option<Setting> {
    let value = raw?.as_str()?.trim();
    if value.is_empty() {
        return None;
    }
    Some(Setting::new(value, source))
}

/// A model's name as the CLI keys its settings by (2.1.322 `Ej`): an alias or
/// slug the account lists stands for its full id, and the dated, `[1m]`,
/// Bedrock and Vertex spellings of an id are that id.
fn canonical(models: &[crate::claude::models::ClaudeModel], raw: &str) -> String {
    let v = raw.trim().to_lowercase();
    let mut v =
        models.iter().find(|m| m.slug.eq_ignore_ascii_case(&v)).and_then(|m| m.model_id.as_deref()).map_or(v.clone(), str::to_lowercase);
    for window in ["[1m]", "[2m]"] {
        if let Some(cut) = v.strip_suffix(window) {
            v = cut.to_owned();
        }
    }
    if let Some(at) = v.rfind('/') {
        v = v[at + 1..].to_owned();
    }
    if let Some(at) = v.find("anthropic.").filter(|_| v.contains("anthropic.claude-")) {
        v = v[at + "anthropic.".len()..].to_owned();
    }
    if let Some(at) = v.find('@') {
        v.truncate(at);
    }
    if let Some(at) = v.rfind("-v").filter(|at| v.len() > at + 2 && v[at + 2..].chars().all(|c| c.is_ascii_digit() || c == ':')) {
        v.truncate(at);
    }
    if let Some(at) = v.rfind('-').filter(|at| v.len() - at == 9 && v[at + 1..].chars().all(|c| c.is_ascii_digit())) {
        v.truncate(at);
    }
    v
}

/// Whether the account's top-level `effortLevel` speaks for a model: one of the
/// CLI's older models, or a name it cannot read as `claude-<family>-<version>`.
/// Not an alias, bare (`opus`) or compound (`opusplan`), left so where the
/// account's list has not named its id yet: the CLI reads it as the family's
/// current model, never an older one.
fn legacy_model(canonical: &str) -> bool {
    if !canonical.contains('-') && crate::claude::models::ALIASES.iter().any(|a| canonical.contains(a)) {
        return false;
    }
    let mut parts = canonical.split('-');
    let standard =
        parts.next() == Some("claude") && parts.next().is_some_and(|f| !f.is_empty() && f.chars().all(|c| c.is_ascii_lowercase())) && {
            let rest: Vec<&str> = parts.collect();
            !rest.is_empty() && rest.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
        };
    LEGACY_MODELS.contains(&canonical) || !standard
}

/// The models whose launch effort the CLI holds against the account's
/// top-level `effortLevel`, on an account first started before it noted its
/// first version (2.1.322 `He`): its `.claude.json` has no `firstStartVersion`.
fn pinned(account_path: &str) -> Vec<&'static str> {
    let Some(cfg) = read_object(&Path::new(account_path).join(".claude.json")) else { return Vec::new() };
    if cfg.get("firstStartVersion").is_some() {
        return Vec::new();
    }
    let unpinned = |key: &str| cfg.get(key).and_then(Value::as_bool) == Some(true);
    [
        ("unpinOpus47LaunchEffort", "claude-opus-4-7"),
        ("unpinOpus48LaunchEffort", "claude-opus-4-8"),
        ("unpinFable5LaunchEffort", "claude-fable-5"),
    ]
    .into_iter()
    .filter(|(key, _)| !unpinned(key))
    .map(|(_, model)| model)
    .collect()
}

/// One settings file's say on effort.
struct EffortFile {
    source: &'static str,
    /// Its top-level `effortLevel`, where it names a level.
    top: Option<String>,
    /// Its `modelSettings`: canonical name, the key as written, the level.
    by_model: Vec<(String, String, String)>,
}

impl EffortFile {
    fn read(source: &'static str, cfg: &Value, models: &[crate::claude::models::ClaudeModel]) -> EffortFile {
        let top = cfg.get("effortLevel").and_then(Value::as_str).and_then(effort_level);
        let mut by_model: Vec<(String, String, String)> = Vec::new();
        for (key, entry) in cfg.get("modelSettings").and_then(Value::as_object).into_iter().flatten() {
            let Some(level) = entry.get("effortLevel").and_then(Value::as_str).and_then(effort_level) else { continue };
            let name = canonical(models, key);
            // Two spellings of one model: the canonical one wins, else the first.
            match by_model.iter_mut().find(|(n, _, _)| *n == name) {
                Some(slot) if *key == name => *slot = (name, key.clone(), level),
                Some(_) => {}
                None => by_model.push((name, key.clone(), level)),
            }
        }
        EffortFile { source, top, by_model }
    }
}

/// The effort the settings files give a model, as Claude Code 2.1.322 resolves
/// it (`FZe`, `Yko`): from the highest file down, the first that names one for
/// the model in `modelSettings`, or names a top-level `effortLevel` and is not
/// the account's own; else the account's top-level one, for an older model
/// only and not one whose launch effort is held (`pinned`). None: nothing
/// does, and the model runs at its own default.
fn effort_for(files: &[EffortFile], pinned: &[&str], name: &str) -> Option<Setting> {
    for f in files.iter().rev() {
        if let Some((_, key, level)) = f.by_model.iter().find(|(n, _, _)| n == name) {
            return Some(Setting { key: Some(format!("modelSettings.{key}.effortLevel")), ..Setting::new(level.clone(), f.source) });
        }
        if let (Some(level), false) = (&f.top, f.source == "account") {
            return Some(Setting::new(level.clone(), f.source));
        }
    }
    let account = files.iter().find(|f| f.source == "account")?;
    let level = account.top.as_ref().filter(|_| legacy_model(name) && !pinned.contains(&name))?;
    Some(Setting::new(level.clone(), "account"))
}

/// What the files say of effort, and the account's models to resolve it for.
struct Efforts {
    files: Vec<EffortFile>,
    pinned: Vec<&'static str>,
    models: Vec<crate::claude::models::ClaudeModel>,
}

impl Efforts {
    /// The effort a chat on `model` (a slug, alias or id) starts on: what the
    /// files give it, else its own default where its catalog names one, else
    /// Luna's stock.
    fn of(&self, model: &str) -> Setting {
        let name = canonical(&self.models, model);
        effort_for(&self.files, &self.pinned, &name).unwrap_or_else(|| {
            let listed = self.models.iter().find(|m| canonical(&self.models, &m.slug) == name);
            listed
                .and_then(|m| m.default_effort.as_deref())
                .and_then(effort_level)
                .map_or_else(|| Setting::new(STOCK_EFFORT, "stock"), |level| Setting::new(level, "model"))
        })
    }
}

/// Model, effort and permission mode as the CLI would resolve them for this
/// account in this folder, and the efforts to resolve another model's by. A
/// missing file, an unreadable one and a key nobody set are all the same
/// answer: nothing said.
fn resolve(account_path: &str, folder: &str) -> (Defaults, Efforts) {
    // Lowest precedence first: every file that has something to say overwrites
    // what the one before it said. The account folder is the CLI's user scope
    // (it is the CLAUDE_CONFIG_DIR its sessions run with), then the project's
    // own two files, then the managed one.
    let mut files: Vec<(&'static str, PathBuf)> = Vec::new();
    if !account_path.is_empty() {
        files.push(("account", Path::new(account_path).join("settings.json")));
    }
    if !folder.is_empty() {
        let dot = Path::new(folder).join(".claude");
        files.push(("project", dot.join("settings.json")));
        files.push(("project-local", dot.join("settings.local.json")));
    }
    files.push(("managed", managed_dir().join("managed-settings.json")));

    // What the files name is held to the account's list; read off disk only,
    // the CLI is not asked for it here.
    let account_models = if account_path.is_empty() { Vec::new() } else { crate::claude::models::models_known(account_path) };
    let (mut model, mut mode) = (None, None);
    let mut effort_files = Vec::new();
    for (source, path) in files {
        let Some(cfg) = read_object(&path) else { continue };
        if let Some(s) = take(source, cfg.get("model")) {
            model = Some(s);
        }
        if let Some(s) = take(source, cfg.pointer("/permissions/defaultMode")) {
            mode = Some(s);
        }
        effort_files.push(EffortFile::read(source, &cfg, &account_models));
    }
    let efforts = Efforts {
        files: effort_files,
        pinned: if account_path.is_empty() { Vec::new() } else { pinned(account_path) },
        models: account_models,
    };
    let model = settle(model, |raw| model_in(&efforts.models, raw), STOCK_MODEL);
    let defaults = Defaults {
        effort: efforts.of(&model.value),
        effort_by_model: efforts.models.iter().map(|m| (m.slug.clone(), efforts.of(&m.slug))).collect(),
        model,
        permission_mode: settle(mode, permission_mode, STOCK_PERMISSION_MODE),
    };
    (defaults, efforts)
}

#[tauri::command]
pub fn claude_defaults(account_path: String, folder: String) -> Defaults {
    resolve(&account_path, &folder).0
}

/// The same as the flags a helper would be spawned with, for `luna_account_get`:
/// `effortByModel` is the effort it gets on each model it may ask for.
pub fn flags(account_path: &str, folder: &str) -> Value {
    let d = claude_defaults(account_path.to_owned(), folder.to_owned());
    let by_model: serde_json::Map<String, Value> = d.effort_by_model.into_iter().map(|(m, s)| (m, Value::String(s.value))).collect();
    serde_json::json!({
        "model": d.model.value,
        "effort": d.effort.value,
        "effortByModel": by_model,
        "permissionMode": d.permission_mode.value,
    })
}

/// The effort a helper on `model` starts on when its spawn names none: for a
/// model the account's list does not carry too, as its name reads.
pub fn effort_of(account_path: &str, folder: &str, model: &str) -> String {
    resolve(account_path, folder).1.of(model).value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::models::ClaudeModel;

    fn write(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn project_settings_win_over_the_account() {
        let root = std::env::temp_dir().join(format!("luna-defaults-{}", std::process::id()));
        let account = root.join("account");
        let folder = root.join("project");
        write(
            &account.join("settings.json"),
            r#"{ "model": "opus", "modelSettings": { "opus": { "effortLevel": "xhigh" } },
                 "permissions": { "defaultMode": "bypassPermissions" } }"#,
        );
        write(&folder.join(".claude").join("settings.json"), r#"{ "model": "sonnet" }"#);

        let d = claude_defaults(account.to_string_lossy().into_owned(), folder.to_string_lossy().into_owned());
        assert_eq!(d.model.value, "sonnet");
        assert_eq!(d.model.source, "project");
        // Untouched by the project file, so the account still answers for them.
        assert_eq!(
            d.effort_by_model["opus"],
            Setting { key: Some("modelSettings.opus.effortLevel".into()), ..Setting::new("xhigh", "account") }
        );
        assert_eq!(d.permission_mode.source, "account");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn values_come_out_settled_or_stock() {
        let root = std::env::temp_dir().join(format!("luna-defaults-settled-{}", std::process::id()));
        let account = root.join("account");
        write(
            &account.join("settings.json"),
            r#"{ "model": "claude-Sonnet-5-5[1m]", "effortLevel": "turbo",
                 "modelSettings": { "sonnet": { "effortLevel": "High" } },
                 "permissions": { "defaultMode": "dontAsk" } }"#,
        );
        let d = claude_defaults(account.to_string_lossy().into_owned(), String::new());
        // A managed file on the machine may have its own say over any of them.
        if d.model.source != "managed" {
            assert_eq!((d.model.value.as_str(), d.model.source.as_str()), ("sonnet", "account"), "a full id names its alias");
        }
        // Neither is one the CLI takes: nothing was said.
        if d.effort.source != "managed" {
            assert_eq!((d.effort.value.as_str(), d.effort.source.as_str()), (STOCK_EFFORT, "stock"));
        }
        if d.permission_mode.source != "managed" {
            assert_eq!(d.permission_mode.value, STOCK_PERMISSION_MODE);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn no_account_and_no_folder_reads_neither() {
        // The machine-wide file is still consulted — it is the one scope that
        // does not depend on either argument — so this asserts on the source
        // rather than on emptiness, which would fail on a managed machine.
        let d = claude_defaults(String::new(), String::new());
        for s in [d.model, d.effort, d.permission_mode] {
            assert!(s.source == "managed" || s.source == "stock", "{}", s.source);
        }
    }

    fn model(slug: &str, id: Option<&str>, default_effort: Option<&str>) -> ClaudeModel {
        ClaudeModel {
            slug: slug.into(),
            model_id: id.map(str::to_owned),
            display_name: slug.into(),
            default_effort: default_effort.map(str::to_owned),
            efforts: SETTINGS_EFFORTS.iter().map(|e| (*e).to_owned()).collect(),
            effort_hints: Default::default(),
            more: false,
        }
    }

    fn efforts(files: &[(&'static str, &str)], pinned: Vec<&'static str>) -> Efforts {
        let models = vec![
            model("opus", Some("claude-opus-5-5"), Some("medium")),
            model("sonnet", Some("claude-sonnet-5-5"), Some("high")),
            model("claude-opus-4-7", Some("claude-opus-4-7"), Some("xhigh")),
            model("claude-sonnet-4-6", None, Some("high")),
        ];
        let files = files.iter().map(|(source, body)| EffortFile::read(source, &serde_json::from_str(body).unwrap(), &models)).collect();
        Efforts { files, pinned, models }
    }

    /// As /effort left it on an account (2.1.322): the level under
    /// `modelSettings` for the model it was set on, the old top-level one
    /// beside it. That speaks for older models only; a current one nothing
    /// names runs at its own default.
    #[test]
    fn effort_is_the_clis_per_model_and_the_accounts_top_level_is_for_older_models() {
        let e = efforts(
            &[("account", r#"{ "effortLevel": "medium", "modelSettings": { "claude-opus-5-5": { "effortLevel": "high" } } }"#)],
            vec![],
        );
        let opus = e.of("opus");
        assert_eq!((opus.value.as_str(), opus.source.as_str()), ("high", "account"));
        assert_eq!(opus.key.as_deref(), Some("modelSettings.claude-opus-5-5.effortLevel"));
        assert_eq!(e.of("claude-opus-5-5-20261001[1m]"), opus, "its dated and [1m] spellings are the model");
        assert_eq!(e.of("sonnet"), Setting::new("high", "model"), "not an older model: its catalog's default");
        assert_eq!(e.of("claude-sonnet-4-6"), Setting::new("medium", "account"), "an older model: the account's top level");
        assert_eq!(e.of("claude-opus-4-7"), Setting::new("medium", "account"));
        // Held at its launch effort on an account older than `firstStartVersion`.
        let held = efforts(&[("account", r#"{ "effortLevel": "medium" }"#)], vec!["claude-opus-4-7"]);
        assert_eq!(held.of("claude-opus-4-7"), Setting::new("xhigh", "model"));
        // Nothing anywhere and no default in the catalog: Luna's stock.
        assert_eq!(efforts(&[], vec![]).of("claude-mythos-6"), Setting::new(STOCK_EFFORT, "stock"));
    }

    /// From the highest file down, the first with a word for the model: its own
    /// level, or a top-level one of a file that is not the account's.
    #[test]
    fn a_higher_files_top_level_effort_beats_a_lower_files_per_model_one() {
        let e = efforts(
            &[
                ("account", r#"{ "modelSettings": { "opus": { "effortLevel": "low" }, "sonnet": { "effortLevel": "low" } } }"#),
                ("project", r#"{ "effortLevel": "medium" }"#),
                ("project-local", r#"{ "modelSettings": { "claude-sonnet-5-5": { "effortLevel": "xhigh" } } }"#),
            ],
            vec![],
        );
        assert_eq!(e.of("opus"), Setting::new("medium", "project"));
        assert_eq!(e.of("sonnet").value, "xhigh");
        assert_eq!(e.of("sonnet").source, "project-local");
        // A project's top level speaks for every model, the newer ones too.
        assert_eq!(e.of("claude-mythos-6"), Setting::new("medium", "project"));
        // `max` is a session's only: a file naming it says nothing.
        let max = efforts(&[("managed", r#"{ "effortLevel": "max", "modelSettings": { "opus": { "effortLevel": "max" } } }"#)], vec![]);
        assert_eq!(max.of("opus"), Setting::new("medium", "model"));
    }

    #[test]
    fn two_spellings_of_one_model_in_a_file_keep_the_canonical_one() {
        let e = efforts(
            &[(
                "account",
                r#"{ "modelSettings": { "opus": { "effortLevel": "low" }, "claude-opus-5-5": { "effortLevel": "xhigh" }, "Claude-Opus-5-5[1m]": { "effortLevel": "medium" } } }"#,
            )],
            vec![],
        );
        assert_eq!(e.of("opus").value, "xhigh");
    }

    #[test]
    fn names_read_as_the_cli_keys_them() {
        let models = vec![model("opus", Some("claude-opus-5-5"), None)];
        for raw in [
            "opus",
            "claude-opus-5-5",
            "claude-opus-5-5-20261001",
            "claude-opus-5-5[1m]",
            "us.anthropic.claude-opus-5-5-v1:0",
            "claude-opus-5-5@20261001",
        ] {
            assert_eq!(canonical(&models, raw), "claude-opus-5-5", "{raw}");
        }
        assert!(legacy_model("claude-opus-4-7") && legacy_model("my-proxy-model") && !legacy_model("claude-opus-5-5"));
        assert!(!legacy_model("opus"), "a bare alias is the family's current model");
        assert!(!legacy_model("opusplan"), "and so is a compound one");
    }

    /// No catalog yet, only the aliases: the account's top-level level is not
    /// the current models' (2.1.322 keeps it for older ones), so an alias runs
    /// at Luna's stock rather than at it.
    #[test]
    fn with_no_catalog_an_alias_is_no_older_model() {
        let models: Vec<ClaudeModel> = crate::claude::models::ALIASES.iter().map(|a| model(a, None, None)).collect();
        let body = r#"{ "effortLevel": "medium", "modelSettings": { "claude-opus-5-5": { "effortLevel": "high" } } }"#;
        let files = vec![EffortFile::read("account", &serde_json::from_str(body).unwrap(), &models)];
        let e = Efforts { files, pinned: vec![], models };
        assert_eq!(e.of("opus"), Setting::new(STOCK_EFFORT, "stock"));
        assert_eq!(e.of("opusplan"), Setting::new(STOCK_EFFORT, "stock"), "a compound alias alike");
        assert_eq!(e.of("claude-opus-4-7"), Setting::new("medium", "account"), "an older model named by id still takes it");
    }

    #[test]
    fn an_account_older_than_its_first_start_version_holds_launch_efforts() {
        let root = std::env::temp_dir().join(format!("luna-defaults-pinned-{}", std::process::id()));
        write(&root.join(".claude.json"), r#"{ "unpinOpus48LaunchEffort": true }"#);
        assert_eq!(pinned(&root.to_string_lossy()), vec!["claude-opus-4-7", "claude-fable-5"]);
        write(&root.join(".claude.json"), r#"{ "firstStartVersion": "2.1.322" }"#);
        assert!(pinned(&root.to_string_lossy()).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
