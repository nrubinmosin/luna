use std::path::{Path, PathBuf};

const APP_DIR: &str = "luna";

/// The build scripts/test-build.ps1 makes. It runs beside the Luna in use, so it keeps
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
/// directory is writable — Luna is meant to carry everything on the drive it
/// was installed to, which the installer lets the user choose — and in
/// `local_dir` otherwise, since an app installed into Program Files cannot
/// write there. Never beside the exe on macOS: it sits inside the signed app
/// bundle, which an update replaces whole. Decided once: a directory that
/// flips between the two mid-run would lose track of its own files.
pub fn data_dir() -> PathBuf {
    DATA_DIR
        .get_or_init(|| {
            if let Some(dir) = exe_dir() {
                if is_test() || (!cfg!(target_os = "macos") && writable(&dir)) {
                    return dir;
                }
            }
            local_dir().unwrap_or_else(last_resort_dir)
        })
        .clone()
}

/// The data dir when there is no local data folder (no `HOME` to find one
/// by): the system's temp folder, the one place left. On Windows that is the
/// user's own (`LOCALAPPDATA\Temp`). On Unix it is `/tmp`, shared by every
/// user and predictable, where another could make Luna's folder first and
/// read or plant what Luna keeps there: so it is `luna-<uid>`, made `0700`,
/// and used only if it is a real folder of this user's that nobody else can
/// get into. Otherwise Luna stops with the reason rather than run on a
/// folder someone else holds; a temp folder of one's own is still better
/// than refusing to start where there is no home.
fn last_resort_dir() -> PathBuf {
    #[cfg(unix)]
    {
        // SAFETY: geteuid has no preconditions and cannot fail.
        let uid = unsafe { libc::geteuid() };
        let dir = std::env::temp_dir().join(format!("{APP_DIR}-{uid}"));
        if let Err(why) = claim_private(&dir, uid) {
            eprintln!("luna: no data folder: {why}");
            std::process::exit(1);
        }
        dir
    }
    #[cfg(not(unix))]
    std::env::temp_dir().join(APP_DIR)
}

/// Makes `dir` `0700` if it is not there, and checks that it is a folder
/// (not a link) owned by `uid` with no access for anyone else: one this user
/// made before is tightened, one someone else made is refused.
#[cfg(unix)]
fn claim_private(dir: &Path, uid: u32) -> Result<(), String> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(format!("cannot create {}: {e}", dir.display())),
    }
    let meta = std::fs::symlink_metadata(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    if !meta.file_type().is_dir() {
        return Err(format!("{} is not a folder (a link or a file is there)", dir.display()));
    }
    if meta.uid() != uid {
        return Err(format!("{} belongs to another user (uid {})", dir.display(), meta.uid()));
    }
    if meta.mode() & 0o077 != 0 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("cannot make {} private: {e}", dir.display()))?;
    }
    Ok(())
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

/// What `path_key` leaves between the parts of a path.
pub const KEY_SEP: char = if cfg!(windows) { '\\' } else { '/' };

/// What two spellings of a path compare by where taking them for one place would
/// be the mistake: nothing is deleted on a likeness. Only Windows has them the
/// same in any case and either separator; on Linux `/p/Foo` and `/p/foo` are two
/// folders and a backslash is part of a name. So is macOS taken at its word: APFS
/// is usually case-insensitive but may not be, and there two spellings of one
/// folder only leave a worktree on disk, where one taken for another deletes it.
pub fn path_key(p: &str) -> String {
    if cfg!(windows) {
        p.replace('/', "\\").trim_end_matches('\\').to_lowercase()
    } else {
        p.trim_end_matches('/').to_string()
    }
}

/// `path_key` for where it is missing a likeness that would be the mistake: a
/// live session not seen in a worktree leaves that worktree to be deleted under
/// it, and a session not recognized as a chat's leaves the chat without it. On
/// macOS the spelling a chat was given and the one its CLI reports differ only
/// where the volume does not care, as it usually does not.
pub fn path_key_loose(p: &str) -> String {
    if cfg!(target_os = "macos") {
        path_key(p).to_lowercase()
    } else {
        path_key(p)
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_in_another_case_or_with_other_separators_is_the_same_only_on_windows() {
        assert_eq!(path_key("/p/Foo") == path_key("/p/foo"), cfg!(windows));
        assert_eq!(path_key(r"/p\foo") == path_key("/p/foo"), cfg!(windows));
        assert_eq!(path_key("/p/foo/"), path_key("/p/foo"), "a trailing separator says nothing");
    }

    #[cfg(unix)]
    #[test]
    fn the_temp_fallback_is_this_users_alone_or_refused() {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::temp_dir().join(format!("luna-paths-claim-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        // SAFETY: geteuid has no preconditions.
        let uid = unsafe { libc::geteuid() };
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;

        let fresh = base.join("fresh");
        claim_private(&fresh, uid).unwrap();
        assert_eq!(mode(&fresh), 0o700);
        let open = base.join("open");
        std::fs::create_dir(&open).unwrap();
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o777)).unwrap();
        claim_private(&open, uid).unwrap();
        assert_eq!(mode(&open), 0o700, "one of this user's own is tightened");

        // Another user's, as a folder made first by someone else reads.
        assert!(claim_private(&fresh, uid.wrapping_add(1)).unwrap_err().contains("another user"));
        let link = base.join("link");
        std::os::unix::fs::symlink(&fresh, &link).unwrap();
        assert!(claim_private(&link, uid).unwrap_err().contains("not a folder"));
        let file = base.join("file");
        std::fs::write(&file, "").unwrap();
        assert!(claim_private(&file, uid).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn the_loose_key_takes_case_for_nothing_on_macos_too_and_separators_nowhere_more() {
        assert_eq!(path_key_loose("/p/Foo") == path_key_loose("/p/foo"), cfg!(any(windows, target_os = "macos")));
        assert_eq!(path_key_loose(r"/p\foo") == path_key_loose("/p/foo"), cfg!(windows));
        assert_eq!(path_key_loose("/p/foo/"), path_key_loose("/p/foo"));
    }
}
