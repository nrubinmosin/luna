use std::path::PathBuf;

const APP_DIR: &str = "luna";

/// `LOCALAPPDATA\luna` — where the log and pasted attachments live.
pub fn local_dir() -> Option<PathBuf> {
    let base = dirs::data_local_dir().or_else(dirs::cache_dir)?;
    Some(base.join(APP_DIR))
}

/// Where Luna keeps what it downloads for itself (the Claude CLI). Beside the
/// exe when that directory is writable — the portable build is meant to carry
/// everything on its own drive — and under LOCALAPPDATA otherwise, since an
/// installed app cannot write into Program Files. Decided once: a directory
/// that flips between the two mid-run would lose track of its own files.
pub fn data_dir() -> PathBuf {
    static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let beside_exe = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()));
        if let Some(dir) = beside_exe {
            if writable(&dir) {
                return dir;
            }
        }
        local_dir().unwrap_or_else(|| std::env::temp_dir().join(APP_DIR))
    })
    .clone()
}

fn writable(dir: &std::path::Path) -> bool {
    let probe = dir.join(format!(".luna-write-probe-{}", std::process::id()));
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}
