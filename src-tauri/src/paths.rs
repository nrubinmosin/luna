use std::path::PathBuf;

const APP_DIR: &str = "luna";

/// The build test-build.ps1 makes. It runs beside the Luna in use, so it keeps
/// everything — log, attachments, settings, downloaded CLIs — next to its exe
/// in the repo's ignored `test-build/`, and shares none of it.
pub fn is_test() -> bool {
    env!("LUNA_FLAVOR") == "test"
}

fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf()))
}

/// `LOCALAPPDATA\luna` — where the log and pasted attachments live.
pub fn local_dir() -> Option<PathBuf> {
    if is_test() {
        return exe_dir();
    }
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
        if let Some(dir) = exe_dir() {
            if is_test() || writable(&dir) {
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
