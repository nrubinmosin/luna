use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Pasted / dropped attachments are copied here so the CLI has a stable
/// absolute path to read, independent of wherever the original lived.
fn media_root(chat_id: &str) -> Result<PathBuf, String> {
    let base = dirs::data_local_dir()
        .or_else(dirs::cache_dir)
        .ok_or("No local data directory")?;
    Ok(base.join("llm-desktop").join("media").join(sanitize(chat_id)))
}

fn sanitize(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').to_string();
    if trimmed.is_empty() {
        "file".into()
    } else {
        // Windows caps a path component at 255; leave room for the stamp prefix.
        trimmed.chars().take(120).collect()
    }
}

/// `data` is base64 — a raw JS byte array would cross the IPC bridge as a JSON
/// list of numbers, roughly 4x the payload for a screenshot.
#[tauri::command]
pub fn save_media(chat_id: String, name: String, data: String) -> Result<String, String> {
    let bytes = STANDARD.decode(data.as_bytes()).map_err(|e| e.to_string())?;
    if bytes.is_empty() {
        return Err("Empty file".into());
    }
    let dir = media_root(&chat_id)?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let file = sanitize(&name);

    // Collisions only happen within the same millisecond; walk a suffix.
    let mut path = dir.join(format!("{stamp}-{file}"));
    let mut n = 1;
    while path.exists() {
        path = dir.join(format!("{stamp}-{n}-{file}"));
        n += 1;
    }

    fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

/// Drops everything a chat accumulated; called when the chat is deleted.
#[tauri::command]
pub fn clear_media(chat_id: String) -> Result<(), String> {
    let dir = media_root(&chat_id)?;
    if dir.exists() {
        fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    }
    Ok(())
}
