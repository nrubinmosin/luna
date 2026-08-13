use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};


/// Pasted / dropped attachments are copied here so the CLI has a stable
/// absolute path to read, independent of wherever the original lived.
fn media_root(chat_id: &str) -> Result<PathBuf, String> {
    let base = crate::paths::local_dir().ok_or("No local data directory")?;
    Ok(base.join("media").join(sanitize(chat_id)))
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

/// Attachments are only cleared when their chat is deleted, so a long-lived
/// chat would hoard every screenshot ever pasted into it. Two ceilings, and
/// both are needed: age catches the chat nobody ever deletes, size catches an
/// afternoon of pasting full-screen grabs into one that is still open — a
/// single 4K screenshot runs to a couple of megabytes, and the age limit would
/// let a fortnight of them pile up untouched.
const KEEP_DAYS: u64 = 14;
const MAX_BYTES: u64 = 256 * 1024 * 1024;

struct Attachment {
    path: PathBuf,
    modified: SystemTime,
    len: u64,
}

fn sweep() -> Result<usize, String> {
    let base = crate::paths::local_dir()
        .ok_or("No local data directory")?
        .join("media");
    if !base.exists() {
        return Ok(0);
    }
    let max_age = std::time::Duration::from_secs(KEEP_DAYS * 24 * 3600);
    let mut kept: Vec<Attachment> = Vec::new();
    let mut removed = 0usize;
    let mut freed = 0u64;

    for chat_dir in fs::read_dir(&base).map_err(|e| e.to_string())?.flatten() {
        let Ok(entries) = fs::read_dir(chat_dir.path()) else { continue };
        for f in entries.flatten() {
            let Ok(meta) = f.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let modified = meta.modified().unwrap_or_else(|_| SystemTime::now());
            let stale = modified.elapsed().map(|age| age > max_age).unwrap_or(false);
            if stale {
                if fs::remove_file(f.path()).is_ok() {
                    removed += 1;
                    freed += meta.len();
                }
            } else {
                kept.push(Attachment { path: f.path(), modified, len: meta.len() });
            }
        }
    }

    // Oldest first, until what is left fits under the ceiling.
    let mut total: u64 = kept.iter().map(|a| a.len).sum();
    if total > MAX_BYTES {
        kept.sort_by_key(|a| a.modified);
        for a in &kept {
            if total <= MAX_BYTES {
                break;
            }
            if fs::remove_file(&a.path).is_ok() {
                total -= a.len;
                removed += 1;
                freed += a.len;
            }
        }
    }

    // Tidy up the chat folders that nothing is left in. Not recursive on
    // purpose: a folder with files still in it must survive.
    if let Ok(dirs) = fs::read_dir(&base) {
        for chat_dir in dirs.flatten() {
            let _ = fs::remove_dir(chat_dir.path());
        }
    }

    if removed > 0 {
        crate::log::info(
            "media",
            &format!("pruned {removed} attachment(s), {}KB freed", freed / 1024),
        );
    }
    Ok(removed)
}

#[tauri::command]
pub fn prune_media() -> Result<usize, String> {
    sweep()
}

/// Sweeps at startup and every few hours after it. Closing the window only
/// hides the app, so a session can run for weeks — a startup-only sweep would
/// never fire again on exactly the installs that accumulate the most.
pub fn prune_periodically() {
    std::thread::spawn(|| loop {
        let _ = sweep();
        std::thread::sleep(std::time::Duration::from_secs(6 * 3600));
    });
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
