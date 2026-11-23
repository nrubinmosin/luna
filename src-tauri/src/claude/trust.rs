use super::config::{config_file, edit, read_config, Missing};
use crate::boundary::{AbsPath, AccountPath};
use serde_json::{Map, Value};

// Claude Code keeps per-project state in <CLAUDE_CONFIG_DIR>/.claude.json under
// a `projects` map keyed by the absolute cwd with forward slashes. Without a
// `hasTrustDialogAccepted` entry the CLI opens its trust prompt — and it refuses
// to do that under --worktree, which is why a fresh folder had to be opened once
// without isolation first. The file is written through config.rs `edit`.

/// Normalizes a Windows path the way the CLI stores it: forward slashes,
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

/// The page's paths are held to an account folder of Luna's and a plain
/// absolute folder (boundary.rs) before the account's config is touched.
pub fn claude_folder_trusted(account_path: AccountPath, folder: AbsPath) -> Result<bool, String> {
    let (account_path, folder) = (String::from(account_path), String::from(folder));
    let cfg = read_config(&config_file(&account_path)?);
    Ok(cfg["projects"][project_key(&folder)]["hasTrustDialogAccepted"].as_bool().unwrap_or(false))
}

/// Marks the folder trusted for this account when the person asks in the
/// new-chat dialog. Sets the same bit as the CLI's own trust prompt under
/// Claude Code's config lock.
pub fn claude_trust_folder(account_path: AccountPath, folder: AbsPath) -> Result<(), String> {
    trust_folder(&String::from(account_path), &String::from(folder))
}

pub(crate) fn trust_folder(account_path: &str, folder: &str) -> Result<(), String> {
    // A model ask's CLI rewriting the file meanwhile would undo the mark:
    // the few seconds of one are waited out (claude/models.rs).
    edit(account_path, crate::claude::models::ASK_WAIT, Missing::Create, |cfg| {
        let projects = cfg.entry("projects").or_insert_with(|| Value::Object(Map::new()));
        if !projects.is_object() {
            *projects = Value::Object(Map::new());
        }
        let entry = projects.as_object_mut().unwrap().entry(project_key(folder)).or_insert_with(|| Value::Object(Map::new()));
        if !entry.is_object() {
            *entry = Value::Object(Map::new());
        }
        entry.as_object_mut().unwrap().insert("hasTrustDialogAccepted".into(), Value::Bool(true));
        true
    })?;
    Ok(())
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
