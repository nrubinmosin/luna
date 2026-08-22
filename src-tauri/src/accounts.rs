use serde::Serialize;
use std::fs;
use std::path::PathBuf;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AccountInfo {
    pub name: String,
    pub path: String,
}

/// The folder the user picked in settings, or `Documents/claude-accounts`.
pub fn accounts_root() -> Result<PathBuf, String> {
    if let Some(p) = crate::settings::get().accounts_root {
        return Ok(PathBuf::from(p));
    }
    let docs = dirs::document_dir().ok_or("Documents directory not found")?;
    Ok(docs.join("claude-accounts"))
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 64 {
        return Err("Account name must be 1..64 characters".into());
    }
    if name
        .chars()
        .any(|c| matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control())
    {
        return Err("Account name contains invalid characters".into());
    }
    if name == "." || name == ".." {
        return Err("Invalid account name".into());
    }
    Ok(())
}

#[tauri::command]
pub fn list_accounts() -> Result<Vec<AccountInfo>, String> {
    let root = accounts_root()?;
    if !root.exists() {
        return Ok(vec![]);
    }
    let mut out = vec![];
    for entry in fs::read_dir(&root).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            out.push(AccountInfo {
                name: entry.file_name().to_string_lossy().into_owned(),
                path: entry.path().to_string_lossy().into_owned(),
            });
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(out)
}

#[tauri::command]
pub fn create_account(name: String) -> Result<AccountInfo, String> {
    validate_name(&name)?;
    let dir = accounts_root()?.join(&name);
    if dir.exists() {
        return Err(format!("Account \"{name}\" already exists"));
    }
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(AccountInfo {
        name,
        path: dir.to_string_lossy().into_owned(),
    })
}

#[tauri::command]
pub fn delete_account(name: String) -> Result<(), String> {
    validate_name(&name)?;
    let dir = accounts_root()?.join(&name);
    if !dir.exists() {
        return Ok(());
    }
    fs::remove_dir_all(&dir).map_err(|e| e.to_string())
}
