use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

// Codex would otherwise honor the account's saved remote-control preference.
pub(crate) const LOCAL_APP_SERVER: (&str, &str) = ("CODEX_INTERNAL_APP_SERVER_REMOTE_CONTROL_DISABLED", "1");
pub(crate) const NO_STARTUP_PLUGINS: &str = "features.plugins=false";
pub(crate) const READ_ID: &str = "launch-config";
pub(crate) const MAX_FRAME: usize = 8 * 1024 * 1024;
const INVALID: &str = "Codex returned an unexpected config/read result";

pub(crate) fn request(cwd: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": READ_ID, "method": "config/read", "params": {"cwd": cwd, "includeLayers": true}})
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
enum Source {
    PackagedDefaults { file: PathBuf },
    System { file: PathBuf },
    User { file: PathBuf, profile: Option<String> },
    Project { dot_codex_folder: PathBuf },
    SessionFlags,
    Mdm { domain: String, key: String },
    EnterpriseManaged { id: String, name: String },
    LegacyManagedConfigTomlFromFile { file: PathBuf },
    LegacyManagedConfigTomlFromMdm,
}

impl Source {
    fn above_overrides(&self) -> bool {
        matches!(self, Self::LegacyManagedConfigTomlFromFile { .. } | Self::LegacyManagedConfigTomlFromMdm)
    }

    fn label(&self) -> PathBuf {
        match self {
            Self::PackagedDefaults { file }
            | Self::System { file }
            | Self::User { file, .. }
            | Self::LegacyManagedConfigTomlFromFile { file } => file.clone(),
            Self::Project { dot_codex_folder } => dot_codex_folder.join("config.toml"),
            Self::SessionFlags => "Codex launch overrides".into(),
            Self::Mdm { domain, key } => format!("MDM ({domain}, {key})").into(),
            Self::EnterpriseManaged { id, name } => format!("enterprise config ({name}, {id})").into(),
            Self::LegacyManagedConfigTomlFromMdm => "legacy MDM config".into(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Layer {
    name: Source,
    version: String,
    config: Map<String, Value>,
    disabled_reason: Option<String>,
}

/// Only kept for the duration of launch planning. Layer values can contain
/// secrets and must never be logged or copied into a launch record.
#[derive(Deserialize)]
pub(crate) struct Snapshot {
    pub(crate) config: Map<String, Value>,
    layers: Vec<Layer>,
}

#[derive(Debug, Default)]
pub(crate) struct HelperMcp {
    pub servers: Vec<String>,
    pub plugins: Vec<String>,
}

impl Snapshot {
    fn loaded(&self) -> impl DoubleEndedIterator<Item = &Layer> {
        self.layers.iter().filter(|layer| layer.disabled_reason.is_none())
    }

    pub(crate) fn response(response: &Value) -> Result<Self, String> {
        if response.get("error").is_some() {
            return Err(read_error(response["error"]["message"].as_str().unwrap_or_default()));
        }
        let this: Self = Self::deserialize(&response["result"]).map_err(|_| INVALID)?;
        if !this.layers.iter().any(|layer| matches!(layer.name, Source::User { profile: None, .. }))
            || this
                .layers
                .iter()
                .any(|layer| layer.version.is_empty() || (layer.disabled_reason.is_some() && !matches!(layer.name, Source::Project { .. })))
        {
            return Err(INVALID.into());
        }
        for config in std::iter::once(&this.config).chain(this.layers.iter().map(|layer| &layer.config)) {
            for table in ["mcp_servers", "plugins", "apps"] {
                if let Some(entries) = config.get(table).filter(|value| !value.is_null()) {
                    let entries = entries.as_object().ok_or(INVALID)?;
                    if entries.values().any(|entry| !entry.is_object()) {
                        return Err(INVALID.into());
                    }
                }
            }
            for table in ["windows", "features"] {
                if config.get(table).is_some_and(|value| !value.is_null() && !value.is_object()) {
                    return Err(INVALID.into());
                }
            }
        }
        this.compaction()?;
        Ok(this)
    }

    /// The server override stays in force through config/read. Put back the
    /// layer's feature choice in the request that opens an admitted helper.
    pub(crate) fn thread_plugins(&self) -> bool {
        self.loaded()
            .filter(|layer| !matches!(layer.name, Source::SessionFlags))
            .find_map(|layer| layer.config.get("features")?.get("plugins")?.as_bool())
            .unwrap_or(true)
    }

    pub(crate) fn helper_mcp(&self) -> Result<HelperMcp, String> {
        let mut servers = BTreeSet::new();
        let mut plugins = BTreeSet::new();
        let library = format!("@{}", crate::library::CODEX_MARKET);
        for layer in self.loaded() {
            let config = &layer.config;
            let apps = config.contains_key("apps") || config.get("features").and_then(|features| features.get("apps")).is_some();
            if layer.name.above_overrides() && (config.contains_key("mcp_servers") || config.contains_key("plugins") || apps) {
                return Err(format!(
                    "the helper is not started: {} sets MCP servers, plugins or apps above the launch overrides; spawn with connectors: true",
                    layer.name.label().display()
                ));
            }
            if let Some(entries) = config.get("mcp_servers").and_then(Value::as_object) {
                servers.extend(entries.keys().cloned());
            }
            if let Some(entries) = config.get("plugins").and_then(Value::as_object) {
                plugins.extend(entries.keys().filter(|name| *name != "_default" && !name.ends_with(&library)).cloned());
            }
        }
        Ok(HelperMcp { servers: servers.into_iter().collect(), plugins: plugins.into_iter().collect() })
    }

    pub(crate) fn own_windows_sandbox(&self) -> bool {
        self.layers.iter().any(|layer| {
            matches!(layer.name, Source::User { profile: None, .. })
                && layer.config.get("windows").and_then(|windows| windows.get("sandbox")).is_some()
        })
    }

    pub(crate) fn developer_instructions(&self) -> super::config::StackInstructions {
        let unsure = self
            .layers
            .iter()
            .filter(|layer| layer.name.above_overrides() && layer.config.contains_key("developer_instructions"))
            .map(|layer| format!("{} sets developer_instructions above the launch overrides", layer.name.label().display()))
            .collect();
        let value = self
            .layers
            .iter()
            .filter(|layer| layer.disabled_reason.is_none() && !layer.name.above_overrides())
            .find_map(|layer| layer.config.get("developer_instructions")?.as_str().map(str::to_owned));
        super::config::StackInstructions { value, unsure }
    }

    pub(crate) fn prompt_for_spawn(&self, approval: Option<&str>, account: &str) -> bool {
        use super::config::{cloud_approval_is_absent, requirements_clear};
        if approval != Some("never") || !requirements_clear() || !cloud_approval_is_absent(Path::new(account)) {
            return true;
        }
        self.loaded().any(|layer| {
            let config = &layer.config;
            config.get("cli_auth_credentials_store").is_some_and(|value| value.as_str() != Some("file"))
                || config.contains_key("config_server")
                || (layer.name.above_overrides() && ["approval_policy", "approvals_reviewer"].iter().any(|key| config.contains_key(*key)))
        })
    }

    pub(crate) fn compaction(&self) -> Result<super::config::Compaction, String> {
        fn field<T: serde::de::DeserializeOwned>(config: &Map<String, Value>, name: &str) -> Result<Option<T>, String> {
            config
                .get(name)
                .filter(|value| !value.is_null())
                .map(|value| serde_json::from_value(value.clone()).map_err(|_| INVALID.to_string()))
                .transpose()
        }
        Ok(super::config::Compaction {
            context_window: field(&self.config, "model_context_window")?,
            token_limit: field(&self.config, "model_auto_compact_token_limit")?,
            model_catalog: field(&self.config, "model_catalog_json")?,
            scope: field(&self.config, "model_auto_compact_token_limit_scope")?,
            post_turn_pct: field(&self.config, "model_post_turn_compact_threshold_percent")?,
        })
    }
}

fn read_error(message: &str) -> String {
    let detail = message.strip_prefix("failed to read configuration layers: ").unwrap_or(message);
    let first = detail.lines().next().unwrap_or_default();
    let location = if let Some((file, at)) =
        first.strip_prefix("Error parsing project config file ").and_then(|text| text.split_once(": TOML parse error at line "))
    {
        at.split_once(", column ").map(|(line, column)| (file, line, column))
    } else {
        first.split_once(".toml:").and_then(|(file, rest)| {
            let (line, rest) = rest.split_once(':')?;
            let (column, _) = rest.split_once(':')?;
            Some((&detail[..file.len() + 5], line, column))
        })
    };
    if let Some((file, line, column)) = location {
        if Path::new(file).is_absolute() && !file.chars().any(char::is_control) {
            if let (Ok(line), Ok(column)) = (line.parse::<usize>(), column.parse::<usize>()) {
                return format!("{file} does not parse as TOML (line {line}, column {column})");
            }
        }
    }
    match without_excerpts(message).lines().next().map(str::trim).filter(|line| !line.is_empty()) {
        Some(reason) => format!("Codex could not read its launch configuration: {reason}"),
        None => "Codex could not read its launch configuration".into(),
    }
}

/// Codex's diagnostic without the source lines it quotes: a config that
/// will not parse comes back as its file, line and column and what is wrong
/// there, then the line itself in a gutter ("2 | key = value", with "|" and
/// caret lines about it; 0.171.0, its TOML errors). The file may be a link
/// to one that is no config, a linked worktree's main checkout's say, and
/// what the turn failed for goes to its parent and the log: the gutter
/// lines go, the rest stays.
pub(crate) fn without_excerpts(why: &str) -> String {
    let quoted = |line: &str| {
        let rest = line.trim_start().trim_start_matches(|c: char| c.is_ascii_digit());
        rest.trim_start().starts_with('|')
    };
    why.lines().filter(|line| !quoted(line)).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    fn item(value: &toml_edit::Item) -> Value {
        if let Some(table) = value.as_table_like() {
            return Value::Object(table.iter().map(|(key, value)| (key.to_owned(), item(value))).collect());
        }
        if let Some(array) = value.as_array() {
            return Value::Array(array.iter().map(|value| item(&toml_edit::Item::Value(value.clone()))).collect());
        }
        if let Some(value) = value.as_str() {
            return json!(value);
        }
        if let Some(value) = value.as_integer() {
            return json!(value);
        }
        if let Some(value) = value.as_float() {
            return json!(value);
        }
        value.as_bool().map_or(Value::Null, |value| json!(value))
    }

    /// Adapter for launch goldens that already set up TOML fixtures.
    pub(crate) fn fixture_for(account: &str, cwd: &str, flags: &[String]) -> Value {
        use crate::codex::{config, trust};
        let doc = match config::read(&Path::new(account).join("config.toml")) {
            Ok(doc) => doc,
            Err(_) => return json!({"error": {"message": "invalid TOML"}}),
        };
        let mut merged = item(doc.as_item());
        let mut layers = vec![json!({
            "name": {"type": "user", "file": Path::new(account).join("config.toml"), "profile": null},
            "version": "account-fixture", "config": merged,
        })];
        if let Ok((root, files)) = config::project_root_and_layers(account, cwd) {
            let key = (!flags.is_empty()).then(|| trust::override_key(cwd));
            let decisions = trust::Decisions::of(account, cwd, key.as_deref());
            for file in files {
                let dir = config::layer_dir(&file);
                let decision = decisions.layer_decision(&dir, &root.to_string_lossy(), cwd);
                let trusted = decision == Some(true);
                let doc = match config::read(&file) {
                    Ok(doc) => doc,
                    Err(_) if !trusted => continue,
                    Err(_) => return json!({"error": {"message": "invalid TOML"}}),
                };
                let data = item(doc.as_item());
                let mut layer = json!({
                    "name": {"type": "project", "dotCodexFolder": file.parent().unwrap()},
                    "version": "project-fixture", "config": data,
                });
                if trusted {
                    merged.as_object_mut().unwrap().extend(data.as_object().unwrap().clone());
                } else {
                    layer["disabledReason"] = json!("project is not trusted");
                }
                layers.insert(0, layer);
            }
        }
        for file in config::managed_layers() {
            if let Ok(contents) = config::read(&file) {
                let data = item(contents.as_item());
                merged.as_object_mut().unwrap().extend(data.as_object().unwrap().clone());
                layers.insert(
                    0,
                    json!({
                        "name": {"type": "legacyManagedConfigTomlFromFile", "file": file},
                        "version": "managed-fixture", "config": data,
                    }),
                );
            }
        }
        json!({"id": READ_ID, "result": {"config": merged, "layers": layers}})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn captured(key: &str) -> Value {
        let captures: Value = serde_json::from_str(include_str!("../background/testdata/codex-0.171.0-config-read.json")).unwrap();
        captures[key].clone()
    }

    #[test]
    fn a_fresh_account_has_an_empty_user_layer() {
        let result = json!({
            "config": {"mcp_servers": {}},
            "layers": [{"name":{"type":"user", "file":"config.toml", "profile":null}, "config":{}, "version":"empty"}]
        });
        assert!(Snapshot::response(&json!({"result": result})).is_ok());
        assert!(Snapshot::response(&json!({"result": {"config": {}, "layers": []}})).is_err());
    }

    #[test]
    fn the_admitted_thread_restores_the_loaded_plugin_feature_choice() {
        let mut result = captured("loaded");
        let read = |result: &Value| Snapshot::response(&json!({"result": result})).unwrap().thread_plugins();
        assert!(read(&result));
        result["layers"]
            .as_array_mut()
            .unwrap()
            .insert(0, json!({"name":{"type":"sessionFlags"}, "config":{"features":{"plugins":false}}, "version":"startup"}));
        assert!(read(&result));
        let user = result["layers"].as_array_mut().unwrap().iter_mut().find(|layer| layer["name"]["type"] == "user").unwrap();
        user["config"]["features"]["plugins"] = json!(false);
        assert!(!read(&result));
    }

    #[test]
    fn the_conflicting_trust_spellings_follow_the_layers_codex_actually_loaded() {
        let result = captured("conflictingTrust");
        let projects = result["layers"][1]["config"]["projects"].as_object().unwrap();
        assert_eq!(projects.len(), 2);
        let response = json!({"result": result});
        let snapshot = Snapshot::response(&response).unwrap();
        assert!(snapshot.layers[0].disabled_reason.is_some());
        assert!(snapshot.layers[0].config["mcp_servers"]["team.docs"].is_object());
        assert!(snapshot.helper_mcp().unwrap().servers.is_empty());
    }

    #[test]
    fn suppression_uses_loaded_layers_and_policy_above_overrides_refuses_it() {
        let mut result = captured("loaded");
        let response = json!({"result": result});
        let snapshot = Snapshot::response(&response).unwrap();
        let mcp = snapshot.helper_mcp().unwrap();
        assert!(mcp.servers.iter().any(|name| name == "team.docs"));
        assert!(mcp.plugins.iter().any(|name| name.contains('@')));
        assert!(snapshot.own_windows_sandbox());
        assert!(snapshot.compaction().unwrap().token_limit.is_some());

        for (kind, config) in [
            ("legacyManagedConfigTomlFromMdm", json!({"features": {"apps": true}})),
            ("legacyManagedConfigTomlFromMdm", json!({"mcp_servers": {"managed": {"url": "http://localhost"}}})),
            ("legacyManagedConfigTomlFromMdm", json!({"plugins": {"plugin@corp": {"enabled": true}}})),
        ] {
            let mut managed = result.clone();
            let name = json!({"type": kind});
            managed["layers"].as_array_mut().unwrap().insert(0, json!({"name": name, "version": "managed", "config": config}));
            let snapshot = Snapshot::response(&json!({"result": managed})).unwrap();
            assert!(snapshot.helper_mcp().unwrap_err().contains("above the launch overrides"));
        }
        result["layers"][0]["name"]["type"] = json!("unknownLayer");
        assert_eq!(Snapshot::response(&json!({"result": result})).err().as_deref(), Some(INVALID));
    }

    #[test]
    fn a_config_read_error_names_the_file_and_position_without_its_text() {
        let cwd = if cfg!(windows) { r"C:\repo\worktrees\new" } else { "/repo/worktrees/new" };
        let file = Path::new(cwd).join(".codex").join("config.toml");
        let error = format!(
            "failed to read configuration layers: Error parsing project config file {}: TOML parse error at line 3, column 8\n3 | PRIVATE TEXT",
            file.display()
        );
        let response = json!({"id": READ_ID, "error": {"message": error}});
        assert_eq!(Snapshot::response(&response).err(), Some(format!("{} does not parse as TOML (line 3, column 8)", file.display())));
    }
}
