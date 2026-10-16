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

static LOCAL_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// The window's local dir, handed to the core for the same reason as
/// `set_data_dir`: the test build keeps it beside its exe, and the core's exe
/// is a copy elsewhere.
pub fn set_local_dir(dir: PathBuf) {
    let _ = LOCAL_DIR.set(dir);
}

/// `LOCALAPPDATA\luna` — where the log and pasted attachments live.
pub fn local_dir() -> Option<PathBuf> {
    if let Some(dir) = LOCAL_DIR.get() {
        return Some(dir.clone());
    }
    if is_test() {
        return exe_dir();
    }
    let base = dirs::data_local_dir().or_else(dirs::cache_dir)?;
    Some(base.join(APP_DIR))
}

static DATA_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// The window's data dir, handed to the core, which runs from a copy of the
/// exe and would otherwise decide on a folder of its own. Before the first
/// `data_dir`, or it has no effect.
pub fn set_data_dir(dir: PathBuf) {
    let _ = DATA_DIR.set(dir);
}

/// Where Luna keeps what it downloads for itself (the Claude CLI). Beside the
/// exe when that directory is writable — the portable build is meant to carry
/// everything on its own drive — and under LOCALAPPDATA otherwise, since an
/// installed app cannot write into Program Files. Never beside the exe on
/// macOS: it sits inside the signed app bundle, which an update replaces
/// whole. Decided once: a directory that flips between the two mid-run would
/// lose track of its own files.
pub fn data_dir() -> PathBuf {
    DATA_DIR.get_or_init(|| {
        if let Some(dir) = exe_dir() {
            if is_test() || (!cfg!(target_os = "macos") && writable(&dir)) {
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
