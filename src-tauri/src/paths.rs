use std::path::PathBuf;

const APP_DIR: &str = "luna";

/// `LOCALAPPDATA\luna` — where the log and pasted attachments live.
pub fn local_dir() -> Option<PathBuf> {
    let base = dirs::data_local_dir().or_else(dirs::cache_dir)?;
    Some(base.join(APP_DIR))
}
