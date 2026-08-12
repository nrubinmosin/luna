use serde_json::{Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

// Claude Code keeps per-project state in <CLAUDE_CONFIG_DIR>/.claude.json under
// a `projects` map keyed by the absolute cwd with forward slashes. Without a
// `hasTrustDialogAccepted` entry the CLI opens its trust prompt — and it refuses
// to do that under --worktree, which is why a fresh folder had to be opened once
// without isolation first.
fn config_file(account_path: &str) -> Result<PathBuf, String> {
    if account_path.is_empty() {
        return Err("No account selected".into());
    }
    Ok(Path::new(account_path).join(".claude.json"))
}

/// Normalises a Windows path the way the CLI stores it: forward slashes,
/// upper-case drive letter, no trailing separator.
fn project_key(folder: &str) -> String {
    let mut s = folder.replace('\\', "/");
    while s.len() > 1 && s.ends_with('/') && !s.ends_with(":/") {
        s.pop();
    }
    let mut chars: Vec<char> = s.chars().collect();
    if chars.len() >= 2 && chars[1] == ':' {
        chars[0] = chars[0].to_ascii_uppercase();
    }
    chars.into_iter().collect()
}

fn read_config(path: &Path) -> Value {
    fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Map::new()))
}

#[tauri::command]
pub fn folder_trusted(account_path: String, folder: String) -> Result<bool, String> {
    let cfg = read_config(&config_file(&account_path)?);
    Ok(cfg["projects"][project_key(&folder)]["hasTrustDialogAccepted"]
        .as_bool()
        .unwrap_or(false))
}

/// Marks the folder trusted for this account — the same bit the CLI's own trust
/// prompt writes. Leaves every other key in the config untouched.
#[tauri::command]
pub fn trust_folder(account_path: String, folder: String) -> Result<(), String> {
    let path = config_file(&account_path)?;
    let mut cfg = read_config(&path);

    let projects = cfg
        .as_object_mut()
        .ok_or("Malformed .claude.json")?
        .entry("projects")
        .or_insert_with(|| Value::Object(Map::new()));
    if !projects.is_object() {
        *projects = Value::Object(Map::new());
    }
    let entry = projects
        .as_object_mut()
        .unwrap()
        .entry(project_key(&folder))
        .or_insert_with(|| Value::Object(Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(Map::new());
    }
    entry.as_object_mut().unwrap().insert(
        "hasTrustDialogAccepted".into(),
        Value::Bool(true),
    );

    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    // Write via a sibling temp file so a crash mid-write cannot truncate the
    // account's whole config.
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(&cfg).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::project_key;

    #[test]
    fn normalises_windows_paths() {
        assert_eq!(project_key(r"e:\Projects\App"), "E:/Projects/App");
        assert_eq!(project_key(r"E:\Projects\App\"), "E:/Projects/App");
        assert_eq!(project_key("E:/Projects/App"), "E:/Projects/App");
        assert_eq!(project_key(r"C:\"), "C:/");
    }
}
