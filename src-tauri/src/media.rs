use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;

/// Where 0.19's window copied pasted and dropped attachments, so the CLI had a
/// stable absolute path to read. Nothing is added now; what is there is
/// swept and cleared with its chat.
fn media_root(chat_id: &str) -> PathBuf {
    crate::paths::data_dir().join("media").join(crate::paths::id_file_name(chat_id))
}

/// Attachments are only cleared when their chat is deleted, so a long-lived
/// chat would hoard every screenshot ever pasted into it. Two ceilings, and
/// both are needed: age catches the chat nobody ever deletes, size catches an
/// afternoon of pasting full-screen grabs into one that is still open — a
/// single 4K screenshot runs to a couple of megabytes, and the age limit would
/// let two weeks of them pile up untouched.
const KEEP_DAYS: u64 = 14;
const MAX_BYTES: u64 = 256 * 1024 * 1024;

struct Attachment {
    path: PathBuf,
    modified: SystemTime,
    len: u64,
}

fn sweep() -> Result<usize, String> {
    let base = crate::paths::data_dir().join("media");
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
        crate::log::info("media", &format!("pruned {removed} attachment(s), {}KB freed", freed / 1024));
    }
    Ok(removed)
}

/// Sweeps at startup and every few hours after it. Closing the window only
/// hides the app, so a session can run for weeks — a startup-only sweep would
/// never fire again on exactly the installs that accumulate the most.
pub fn prune_periodically() {
    drop_old_root();
    std::thread::spawn(|| loop {
        let _ = sweep();
        std::thread::sleep(std::time::Duration::from_secs(6 * 3600));
    });
}

/// Attachments used to go under `LOCALAPPDATA\luna` whatever the data dir.
/// The CLI read each one when its prompt went, and none lives past two
/// weeks, so the ones still there are removed rather than moved — along
/// with the folder, if nothing else is in it. Not when that folder is the
/// data dir itself, as it is for an installed Luna.
fn drop_old_root() {
    let Some(old) = crate::paths::local_dir() else { return };
    if !old.join("media").exists() || crate::paths::same_dir(&old, &crate::paths::data_dir()) {
        return;
    }
    if fs::remove_dir_all(old.join("media")).is_ok() {
        crate::log::info("media", &format!("removed the attachments left in {}", old.display()));
    }
    let _ = fs::remove_dir(&old);
}

/// Drops everything a chat accumulated; called when the chat is deleted.
pub fn clear_media(chat_id: String) -> Result<(), String> {
    let dir = media_root(&chat_id);
    if dir.exists() {
        fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    }
    Ok(())
}
