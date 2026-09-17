//! The models Codex offers this account, off the list it caches itself:
//! `<CODEX_HOME>/models_cache.json`, refreshed by the CLI whenever it runs.
//! Each entry names the reasoning levels the model takes, which is also what
//! `/model` inside Codex shows — so the new-chat dialog can offer the same
//! buttons instead of a blank box.

use serde::Serialize;
use std::path::Path;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CodexModel {
    pub slug: String,
    pub display_name: String,
    pub default_effort: Option<String>,
    pub efforts: Vec<String>,
}

pub fn read_models(account_path: &str) -> Vec<CodexModel> {
    let path = Path::new(account_path).join("models_cache.json");
    let Ok(text) = std::fs::read_to_string(&path) else { return vec![] };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { return vec![] };
    let Some(models) = v["models"].as_array() else { return vec![] };
    models
        .iter()
        .filter(|m| m["visibility"].as_str().unwrap_or("list") == "list")
        .filter_map(|m| {
            let slug = m["slug"].as_str()?.to_string();
            Some(CodexModel {
                display_name: m["display_name"].as_str().unwrap_or(&slug).to_string(),
                default_effort: m["default_reasoning_level"].as_str().map(str::to_owned),
                efforts: m["supported_reasoning_levels"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|l| l["effort"].as_str().map(str::to_owned)).collect())
                    .unwrap_or_default(),
                slug,
            })
        })
        .collect()
}

#[tauri::command]
pub fn codex_models(account_path: String) -> Vec<CodexModel> {
    if account_path.is_empty() {
        return vec![];
    }
    read_models(&account_path)
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
              {"slug":"gpt-6-astra","display_name":"GPT-6-Astra","default_reasoning_level":"medium",
               "supported_reasoning_levels":[{"effort":"low"},{"effort":"high"},{"effort":"ultra"}],"visibility":"list"},
              {"slug":"gpt-reserve","display_name":"GPT-Reserve","supported_reasoning_levels":[],"visibility":"hide"}
            ]}"#,
        )
        .unwrap();
        let m = read_models(&root.to_string_lossy());
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].slug, "gpt-6-astra");
        assert_eq!(m[0].display_name, "GPT-6-Astra");
        assert_eq!(m[0].default_effort.as_deref(), Some("medium"));
        assert_eq!(m[0].efforts, vec!["low", "high", "ultra"]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
