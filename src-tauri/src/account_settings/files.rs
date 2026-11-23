//! An account's settings file, found, checked and written.

use super::*;

pub(super) fn config_file(provider: Provider, account_path: &str) -> Result<PathBuf, String> {
    match provider {
        Provider::Claude => claude_file(account_path),
        Provider::Codex => crate::codex::config::account_file(account_path),
    }
}

fn claude_file(account_path: &str) -> Result<PathBuf, String> {
    if account_path.is_empty() {
        return Err("No account selected".into());
    }
    Ok(Path::new(account_path).join("settings.json"))
}

/// Writes via a sibling temp file so a crash mid-write cannot truncate the file.
pub(super) fn write_text(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    // A temp file of this write's own: two writers sharing one could rename
    // the other's half-written bytes into place (the start-up sync runs on a
    // thread of its own beside the window's commands).
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}-{}.tmp", std::process::id(), SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, text).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })?;
    // A rename refused (the CLI holding the file) leaves no temp file behind.
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })
}

/// Whether `text` is a settings file this provider can read: a JSON object
/// for Claude Code, TOML for Codex. The message says where it breaks, and
/// for TOML nothing of what is there (codex/config.rs `toml_unparsed`): it
/// goes to the log, and the parent's file may be a link to one that is no
/// config.
pub(super) fn check_config(provider: Provider, text: &str) -> Result<(), String> {
    match provider {
        Provider::Claude => match serde_json::from_str::<Value>(text) {
            Ok(v) if v.is_object() => Ok(()),
            Ok(_) => Err("settings.json has to be one JSON object: { … }".into()),
            Err(e) => Err(format!("Not valid JSON: {e}")),
        },
        Provider::Codex => text.parse::<DocumentMut>().map(|_| ()).map_err(|e| crate::codex::config::toml_unparsed(text, &e)),
    }
}
