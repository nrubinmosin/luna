use std::path::{Path, PathBuf};

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

/// `LOCALAPPDATA\luna` (`Application Support/luna` on macOS): the data dir
/// when the exe's own cannot be. None for the test build, which must not
/// touch the real Luna's.
pub fn local_dir() -> Option<PathBuf> {
    if is_test() {
        return None;
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

/// Where Luna keeps everything of its own: settings, the log, attachments,
/// the WebView's profile, the CLIs it downloads. Beside the exe when that
/// directory is writable — the portable build is meant to carry everything on
/// its own drive, and the installer puts Luna under LOCALAPPDATA — and in
/// `local_dir` otherwise, since an app installed into Program Files cannot
/// write there. Never beside the exe on macOS: it sits inside the signed app
/// bundle, which an update replaces whole. Decided once: a directory that
/// flips between the two mid-run would lose track of its own files.
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

fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".luna-write-probe-{}", std::process::id()));
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// Whether two paths name one directory, whatever their spelling: on Windows
/// `LOCALAPPDATA\luna` is the installed Luna's own `LOCALAPPDATA\Luna`.
pub fn same_dir(a: &Path, b: &Path) -> bool {
    matches!((std::fs::canonicalize(a), std::fs::canonicalize(b)), (Ok(a), Ok(b)) if a == b)
}

/// The WebView's profile: localStorage, which holds the chats and the
/// layout, and the browser's caches. Windows only. Elsewhere the exe's folder
/// is an app bundle or a system path, so the data dir is in the user's folder
/// anyway and the WebView's own default place there is as good.
#[cfg(windows)]
pub fn webview_dir() -> PathBuf {
    data_dir().join("webview")
}

/// Brings the chats and the layout over from the profile WebView2 kept in its
/// default place, `old` (`LOCALAPPDATA\<identifier>`), and removes that
/// profile: its caches are only caches. Before the window is made, which
/// opens the profile in `webview_dir` for good.
#[cfg(windows)]
pub fn adopt_webview_profile(old: &Path) {
    if !old.exists() || same_dir(old, &webview_dir()) {
        return;
    }
    let storage = Path::new("EBWebView").join("Default").join("Local Storage");
    let (from, to) = (old.join(&storage), webview_dir().join(&storage));
    if from.exists() && !to.exists() {
        if let Err(e) = copy_dir(&from, &to) {
            // Left where it is for the next launch to try again.
            crate::log::warn("app", &format!("could not move the WebView's storage from {}: {e}", old.display()));
            let _ = std::fs::remove_dir_all(&to);
            return;
        }
        crate::log::info("app", &format!("moved the WebView's storage from {}", old.display()));
    }
    // A WebView2 process of the previous run can still hold a file or two;
    // whatever is left goes on a later launch.
    let _ = std::fs::remove_dir_all(old);
}

#[cfg(windows)]
fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}
