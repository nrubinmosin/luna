use std::path::PathBuf;

const APP_DIR: &str = "luna";
/// The app shipped under this name before the rebrand.
const LEGACY_DIR: &str = "llm-desktop";

/// `LOCALAPPDATA\luna`, taking over the pre-rebrand folder the first time it
/// runs so an existing install keeps its attachments and log instead of
/// orphaning them under a name nothing reads any more.
pub fn local_dir() -> Option<PathBuf> {
    let base = dirs::data_local_dir().or_else(dirs::cache_dir)?;
    let dir = base.join(APP_DIR);
    if !dir.exists() {
        let legacy = base.join(LEGACY_DIR);
        if legacy.exists() {
            let _ = std::fs::rename(&legacy, &dir);
        }
    }
    Some(dir)
}
