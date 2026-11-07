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
        if let Err(why) = crate::secret::claim_private(&dir, uid) {
            eprintln!("luna: no data folder: {why}");
            std::process::exit(1);
        }
        dir
    }
    #[cfg(not(unix))]
    std::env::temp_dir().join(APP_DIR)
}

/// What Settings' checks say of the data dir's DACL (checks.rs).
#[cfg_attr(not(windows), allow(dead_code))]
pub struct Privacy {
    /// Private down to the last file, as a full pass last verified.
    pub private: bool,
    /// What the row adds: only Luna's own things were made private, what the
    /// last pass found, or why it failed.
    pub note: Option<String>,
}

/// What Luna keeps at the top of its data dir, its exe and uninstaller
/// aside: a folder holding only these (and those two) is Luna's own. A file
/// name may also be one being written through a temp file (`<name>.….tmp`).
#[cfg_attr(not(windows), allow(dead_code))]
const OWN_NAMES: &[&str] = &[
    "account-settings.lock",
    "agents",
    "agents.jsonl",
    "chat-ties.json",
    "claude-cli",
    "codex-cli",
    "core",
    "helpers.json",
    "hooks",
    "library",
    "library-backups",
    "library-blobs",
    "library-folders",
    "library-state.json",
    "library-state.json.lock",
    "luna.core.run",
    "luna.log",
    "luna.log.1",
    "luna.log.lock",
    "luna.run",
    "mcp",
    "media",
    "quota-told.json",
    "settings.json",
    "settings.json.lock",
    "tied-jobs",
    "webview",
];

/// Whether a top-level name in the data dir is Luna's: one of `OWN_NAMES`,
/// one of those being written through a temp file, `exe` or the uninstaller.
#[cfg_attr(not(windows), allow(dead_code))]
fn lunas_own(name: &str, exe: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower == exe.to_ascii_lowercase()
        || lower == "uninstall.exe"
        || lower.starts_with(".luna-write-probe-")
        || OWN_NAMES.iter().any(|own| lower == *own || (lower.starts_with(&format!("{own}.")) && lower.ends_with(".tmp")))
}

/// What `protect_data_dir` makes private: the data dir as a whole where it is
/// a folder of Luna's alone, otherwise each of Luna's own things in it, the
/// reason said.
#[cfg_attr(not(windows), allow(dead_code))]
enum Scope {
    Whole,
    Own(Vec<PathBuf>, String),
}

/// The data dir is Luna's alone when it is `local_dir`, which Luna names
/// itself, or an install of Luna's: its exe and uninstaller there, nothing
/// at the top that is not Luna's (`lunas_own`), no drive root, and no link
/// or junction at it or on the way to it. Anything else — a copy run from a
/// shared tools folder, say — keeps everything but Luna's own things as it
/// was: making the whole folder private would take others' files away from
/// them.
#[cfg_attr(not(windows), allow(dead_code))]
fn scope(dir: &Path) -> Scope {
    let own = |why: String| {
        let entries = std::fs::read_dir(dir).map(|r| r.flatten().map(|e| e.path()).collect::<Vec<_>>()).unwrap_or_default();
        let items = entries
            .into_iter()
            .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| OWN_NAMES.iter().any(|own| n.eq_ignore_ascii_case(own))))
            .collect();
        Scope::Own(items, why)
    };
    if local_dir().is_some_and(|l| same_dir(&l, dir)) {
        return Scope::Whole;
    }
    if dir.parent().is_none() {
        return own(format!("{} is the root of a drive", dir.display()));
    }
    if let Some(link) = dir.ancestors().find(|a| std::fs::symlink_metadata(a).is_ok_and(|m| m.file_type().is_symlink())) {
        return own(format!("{} is reached through a link or junction", link.display()));
    }
    let exe = std::env::current_exe().ok().and_then(|e| e.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_default();
    // The core runs from a copy of the exe (corelink.rs `core_exe`), named
    // for the window's: the exe beside the data is that name.
    let exe = exe.strip_suffix("-core.exe").map(|stem| format!("{stem}.exe")).unwrap_or(exe);
    if !dir.join(&exe).is_file() || !dir.join("uninstall.exe").is_file() {
        return own(format!("{} is not an installed Luna", dir.display()));
    }
    let names: Vec<String> =
        std::fs::read_dir(dir).map(|r| r.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
    let foreign: Vec<String> = names.into_iter().filter(|n| !lunas_own(n, &exe)).collect();
    if foreign.is_empty() {
        Scope::Whole
    } else {
        own(format!("{} also holds what is not Luna's: {}", dir.display(), foreign.join(", ")))
    }
}

/// Written in the data dir's `core/` once a full pass has verified every
/// target, with the scope it was for; until then each start makes one.
#[cfg_attr(not(windows), allow(dead_code))]
fn verified_marker() -> PathBuf {
    data_dir().join("core").join("private-verified")
}

/// What the last `protect_data_dir` of this run came to: what it found odd,
/// or why it failed.
#[cfg(windows)]
static LAST_PROTECT: std::sync::Mutex<Option<Result<Vec<String>, String>>> = std::sync::Mutex::new(None);

/// Windows: makes the data dir private (secret.rs `acl::protect_tree`), as
/// far as `scope` allows, and checks it held down to the last file. A
/// per-user install keeps everything there, beside the exe, and an install
/// folder outside Program Files takes the ACL of the folder it is in, which
/// on a second drive commonly lets every signed-in user read the sessions'
/// history and replace the exe. The installer protects a folder before it
/// puts anything in (windows/hooks.nsh); this is for a copy installed before
/// that, and for whatever has changed since. Verified at every start, the
/// pass made only where the walk finds something or `verified_marker` is
/// missing (`make_private`). Run as the core starts,
/// and from Settings' row. Elsewhere nothing to do: Unix has
/// `secret::private_dir`'s folders, and the data dir in the user's home.
pub fn protect_data_dir() -> Result<(), String> {
    #[cfg(windows)]
    {
        let dir = data_dir();
        let (targets, label) = match scope(&dir) {
            Scope::Whole => (vec![dir.clone()], "whole".to_string()),
            Scope::Own(items, why) => (items, format!("own: {why}")),
        };
        let done = make_private(&targets, &label, &verified_marker());
        match &done {
            Ok(Pass::Held) => {}
            Ok(Pass::Done(odd)) => {
                crate::log::info("app", &format!("made {} private to this user, SYSTEM and Administrators ({label})", dir.display()));
                for what in odd {
                    crate::log::warn("app", &format!("data folder: {what}"));
                }
            }
            Err(e) => crate::log::warn("app", e),
        }
        let result = done.as_ref().map(|_| ()).map_err(String::clone);
        let kept = done.map(|pass| match pass {
            Pass::Held => Vec::new(),
            Pass::Done(odd) => odd,
        });
        *LAST_PROTECT.lock().unwrap_or_else(|e| e.into_inner()) = Some(kept);
        result
    }
    #[cfg(not(windows))]
    Ok(())
}

/// What `make_private` came to: it held as the last pass left it, or a pass
/// was made now, with what it found odd.
#[cfg(windows)]
enum Pass {
    Held,
    Done(Vec<String>),
}

/// Verifies `targets` down to the last file, every time: the marker only
/// saves the pass that changes things, never the look. A marker can be
/// planted while the folder is open and made the user's by the installer's
/// takeover, and a junction beside it is never gone into by that takeover;
/// only the walk sees it. Where the marker says a pass for `label` held and
/// the walk finds nothing, that is all; otherwise the marker goes, a pass is
/// made (`protect_and_verify`), and the marker is written only after it.
#[cfg(windows)]
fn make_private(targets: &[PathBuf], label: &str, marker: &Path) -> Result<Pass, String> {
    let marked =
        std::fs::read_to_string(marker).is_ok_and(|said| said == label) && crate::secret::acl::owner_trusted(marker).unwrap_or(false);
    if marked {
        match verify_all(targets) {
            Ok(open) if open.is_empty() => return Ok(Pass::Held),
            Ok(open) => {
                crate::log::warn("app", &format!("data folder was marked private, but: {}; making it private again", open.join("; ")))
            }
            Err(e) => crate::log::warn("app", &format!("{e}; making the data folder private again")),
        }
    }
    let _ = std::fs::remove_file(marker);
    let odd = protect_and_verify(targets)?;
    if let Some(parent) = marker.parent() {
        crate::secret::private_dir(parent)
            .and_then(|()| crate::secret::write_private(marker, label.as_bytes()))
            .map_err(|e| format!("could not note the data folder as private: {e}"))?;
    }
    Ok(Pass::Done(odd))
}

/// What keeps any target from being private, `acl::verify` over each.
#[cfg(windows)]
fn verify_all(targets: &[PathBuf]) -> Result<Vec<String>, String> {
    let mut open = Vec::new();
    for t in targets {
        open.extend(crate::secret::acl::verify(t).map_err(|e| format!("could not check {}: {e}", t.display()))?);
    }
    Ok(open)
}

/// Every target protected, then every one verified: what was odd, or what
/// went wrong or is still open.
#[cfg(windows)]
fn protect_and_verify(targets: &[PathBuf]) -> Result<Vec<String>, String> {
    let mut odd = Vec::new();
    for t in targets {
        odd.extend(crate::secret::acl::protect_tree(t).map_err(|e| format!("could not make {} private: {e}", t.display()))?);
    }
    let open = verify_all(targets)?;
    if open.is_empty() {
        Ok(odd)
    } else {
        Err(format!("still not private: {}", open.join("; ")))
    }
}

/// The data dir's privacy, for Settings' checks: private only once this run's
/// `protect_data_dir` has verified it (red until then), None where there is
/// nothing to check (off Windows).
pub fn data_dir_privacy() -> Option<Privacy> {
    #[cfg(windows)]
    {
        let dir = data_dir();
        let (label, own) = match scope(&dir) {
            Scope::Whole => ("whole".to_string(), None),
            Scope::Own(_, why) => (format!("own: {why}"), Some(format!("Only Luna's own folders and files are made private: {why}."))),
        };
        let last = LAST_PROTECT.lock().unwrap_or_else(|e| e.into_inner());
        let private = matches!(&*last, Some(Ok(_))) && std::fs::read_to_string(verified_marker()).is_ok_and(|said| said == label);
        let last = match &*last {
            Some(Err(e)) => Some(e.clone()),
            Some(Ok(odd)) if !odd.is_empty() => Some(format!(
                "Found when it was made private, see the log: {}. A file someone else owned may have been changed by them; the next update reinstalls Luna's own.",
                odd.join("; ")
            )),
            _ => None,
        };
        let note = [own, last].into_iter().flatten().collect::<Vec<_>>().join(" ");
        Some(Privacy { private, note: (!note.is_empty()).then_some(note) })
    }
    #[cfg(not(windows))]
    None
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

    #[test]
    fn only_lunas_own_names_make_a_folder_lunas() {
        for own in ["luna.exe", "Uninstall.exe", "webview", "agents.jsonl", "claude-cli", "settings.json.1a2b3c4d.tmp", "luna.log.1"] {
            assert!(lunas_own(own, "luna.exe"), "{own}");
        }
        for other in ["notes.txt", "other.exe", "webview2", "settings.json.bak", "agents.jsonl.old", "luna-test.exe"] {
            assert!(!lunas_own(other, "luna.exe"), "{other}");
        }
    }

    /// In the test executables' folder under `test-build/`: what makes a
    /// folder a Luna install that may be made private whole.
    #[cfg(windows)]
    #[test]
    fn a_folder_is_made_private_whole_only_when_it_is_an_install_of_lunas_alone() {
        let exe = std::env::current_exe().unwrap().file_name().unwrap().to_string_lossy().into_owned();
        let base = data_dir().join(format!("scope-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let install = base.join("Luna");
        std::fs::create_dir_all(install.join("webview")).unwrap();
        for f in [exe.as_str(), "uninstall.exe", "settings.json"] {
            std::fs::write(install.join(f), "").unwrap();
        }
        assert!(matches!(scope(&install), Scope::Whole));
        std::fs::write(install.join("notes.txt"), "").unwrap();
        match scope(&install) {
            Scope::Own(items, why) => {
                assert!(why.contains("notes.txt"), "{why}");
                let names: Vec<String> = items.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
                assert!(names.contains(&"webview".to_string()) && names.contains(&"settings.json".to_string()), "{names:?}");
                assert!(!names.iter().any(|n| n == "notes.txt" || n == &exe), "{names:?}");
            }
            Scope::Whole => panic!("a folder with another's file is not Luna's alone"),
        }
        std::fs::remove_file(install.join("notes.txt")).unwrap();
        std::fs::remove_file(install.join("uninstall.exe")).unwrap();
        assert!(matches!(scope(&install), Scope::Own(_, why) if why.contains("not an installed Luna")));
        // Reached through a junction.
        std::fs::write(install.join("uninstall.exe"), "").unwrap();
        let link = base.join("via");
        let made = std::process::Command::new("cmd").args(["/d", "/c", "mklink", "/J"]).arg(&link).arg(&base).output().unwrap();
        assert!(made.status.success());
        assert!(matches!(scope(&link.join("Luna")), Scope::Own(_, why) if why.contains("link")));
        std::fs::remove_dir(&link).unwrap();
        assert!(matches!(scope(Path::new("C:\\")), Scope::Own(_, why) if why.contains("root of a drive")));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A marker planted while the folder was open, made the user's (as the
    /// installer's takeover would), beside a junction out: the walk still
    /// runs, finds the junction, and the pass made again fails on it, the
    /// marker gone. Without the junction the pass holds, and the next start
    /// only looks. Under `test-build/`, with the walk timed on a tree twice
    /// the real data dir's size (about 1,300 entries, most in the WebView's
    /// profile).
    #[cfg(windows)]
    #[test]
    fn a_planted_marker_does_not_keep_the_walk_from_running() {
        let root = data_dir().join(format!("marker-test-{}", std::process::id()));
        let outside = data_dir().join(format!("marker-out-{}", std::process::id()));
        for d in [&root, &outside] {
            let _ = std::fs::remove_dir_all(d);
        }
        std::fs::create_dir_all(root.join("core")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let marker = root.join("core").join("private-verified");
        std::fs::write(&marker, "whole").unwrap();
        let link = root.join("webview");
        assert!(std::process::Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&link)
            .arg(&outside)
            .output()
            .unwrap()
            .status
            .success());
        let targets = [root.clone()];

        let failed = make_private(&targets, "whole", &marker).err().expect("the junction keeps it from being private");
        assert!(failed.contains("webview") && failed.contains("link"), "{failed}");
        assert!(!marker.exists(), "no marker without a pass that held");
        assert!(make_private(&targets, "whole", &marker).is_err(), "and the next start tries again");

        std::fs::remove_dir(&link).unwrap();
        for d in 0..300 {
            let sub = root.join("webview").join(format!("d{}", d / 30)).join(format!("e{d}"));
            std::fs::create_dir_all(&sub).unwrap();
            for f in 0..9 {
                std::fs::write(sub.join(format!("f{f}")), "x").unwrap();
            }
        }
        assert!(matches!(make_private(&targets, "whole", &marker), Ok(Pass::Done(_))));
        assert!(marker.is_file());
        let started = std::time::Instant::now();
        assert!(matches!(make_private(&targets, "whole", &marker), Ok(Pass::Held)), "verified, nothing to do");
        println!("verify of {} entries: {:?}", 300 * 10 + 12, started.elapsed());
        for d in [&root, &outside] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn the_loose_key_takes_case_for_nothing_on_macos_too_and_separators_nowhere_more() {
        assert_eq!(path_key_loose("/p/Foo") == path_key_loose("/p/foo"), cfg!(any(windows, target_os = "macos")));
        assert_eq!(path_key_loose(r"/p\foo") == path_key_loose("/p/foo"), cfg!(windows));
        assert_eq!(path_key_loose("/p/foo/"), path_key_loose("/p/foo"));
    }
}
