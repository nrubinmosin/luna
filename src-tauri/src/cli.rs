//! Luna's own copy of the Claude Code CLI.
//!
//! The app used to run whatever `claude` was on PATH, which tied every Luna
//! install to a global CLI living under the user profile — and to that CLI's
//! own updater writing into `~/.local/bin`. This module keeps a private copy
//! under Luna's data dir instead, fetched straight from the same release
//! bucket the official installer uses, so the whole thing (app, accounts,
//! CLI) can sit on one drive and move with it.
//!
//! Layout:
//!
//! ```text
//! <data>/claude-cli/
//!   current                   text file: the version sessions should spawn
//!   versions/<ver>/claude.exe one directory per downloaded version
//! ```
//!
//! Versions are never overwritten in place: Windows will not let a running
//! exe be replaced, and a session that is mid-turn keeps the old one open
//! for hours. A new version lands in its own directory, `current` flips to
//! it, and old directories are swept whenever nothing holds them any more.
//!
//! The CLI's own auto-updater is switched off for sessions Luna spawns
//! (`DISABLE_AUTOUPDATER=1`), so this is the only thing that updates it.

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tauri::{AppHandle, Emitter};

const RELEASES: &str = "https://downloads.claude.ai/claude-code-releases";
/// `latest` or `stable` — the same two channels the official installer takes.
const CHANNEL: &str = "latest";
const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);
const EVENT: &str = "cli://status";

#[cfg(windows)]
const BIN_NAME: &str = "claude.exe";
#[cfg(not(windows))]
const BIN_NAME: &str = "claude";

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct CliStatus {
    /// `idle` | `checking` | `downloading` | `error`.
    pub phase: String,
    /// The version sessions spawn right now; None until the first download.
    pub version: Option<String>,
    /// Where that binary lives — `claude` on PATH while there is no managed copy.
    pub path: String,
    /// The newest version the bucket offers, once a check has come back.
    pub latest: Option<String>,
    pub got: u64,
    pub total: Option<u64>,
    pub error: Option<String>,
    pub checked_at_ms: Option<u64>,
}

fn state() -> &'static Mutex<CliStatus> {
    static S: OnceLock<Mutex<CliStatus>> = OnceLock::new();
    S.get_or_init(|| {
        Mutex::new(CliStatus {
            phase: "idle".into(),
            version: current_version(),
            path: binary().to_string_lossy().into_owned(),
            ..Default::default()
        })
    })
}

/// One update at a time: the periodic check and a click on the status-bar
/// field must not start two downloads of the same 100MB.
static BUSY: AtomicBool = AtomicBool::new(false);

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------- layout --

pub fn root() -> PathBuf {
    crate::paths::data_dir().join("claude-cli")
}

fn versions_dir() -> PathBuf {
    root().join("versions")
}

fn version_binary(version: &str) -> PathBuf {
    versions_dir().join(version).join(BIN_NAME)
}

fn current_file() -> PathBuf {
    root().join("current")
}

fn current_version() -> Option<String> {
    let v = fs::read_to_string(current_file()).ok()?;
    let v = v.trim().to_string();
    (!v.is_empty() && version_binary(&v).is_file()).then_some(v)
}

/// What `ensure_session` should spawn. Falls back to PATH so a fresh install
/// that has not finished its first download still works for anyone who has
/// the CLI installed the usual way.
pub fn binary() -> PathBuf {
    match current_version() {
        Some(v) => version_binary(&v),
        None => PathBuf::from("claude"),
    }
}

fn platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "aarch64") => "win32-arm64",
        ("windows", _) => "win32-x64",
        ("macos", "aarch64") => "darwin-arm64",
        ("macos", _) => "darwin-x64",
        ("linux", "aarch64") => "linux-arm64",
        _ => "linux-x64",
    }
}

// ---------------------------------------------------------------- status --

fn publish(app: Option<&AppHandle>, f: impl FnOnce(&mut CliStatus)) {
    let snapshot = {
        let Ok(mut s) = state().lock() else { return };
        f(&mut s);
        s.clone()
    };
    if let Some(app) = app {
        if !crate::pty::SHUTTING_DOWN.load(Ordering::SeqCst) {
            let _ = app.emit(EVENT, snapshot);
        }
    }
}

#[tauri::command]
pub fn cli_status() -> CliStatus {
    state().lock().map(|s| s.clone()).unwrap_or_default()
}

/// A click on the status-bar field: check now, install if newer.
#[tauri::command]
pub fn cli_update_now(app: AppHandle) {
    std::thread::spawn(move || update(&app));
}

/// Startup: one update pass right away, then one every few hours. The first
/// pass is what installs the CLI on a fresh machine, so it is not delayed.
pub fn refresh_periodically(app: AppHandle) {
    std::thread::spawn(move || loop {
        update(&app);
        std::thread::sleep(CHECK_EVERY);
    });
}

// ---------------------------------------------------------------- update --

fn http() -> ureq::Agent {
    ureq::builder()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(60))
        .build()
}

fn latest_version(agent: &ureq::Agent) -> Result<String, String> {
    let text = agent
        .get(&format!("{RELEASES}/{CHANNEL}"))
        .call()
        .map_err(|e| format!("version lookup: {e}"))?
        .into_string()
        .map_err(|e| e.to_string())?;
    let v = text.trim().to_string();
    // An HTML error page must not become a directory name.
    let ok = v.split('.').take(3).all(|p| p.chars().take_while(|c| c.is_ascii_digit()).count() > 0)
        && v.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    if !ok || v.len() > 64 {
        return Err(format!("unexpected version text from {CHANNEL}: {:?}", v.chars().take(40).collect::<String>()));
    }
    Ok(v)
}

fn expected_checksum(agent: &ureq::Agent, version: &str) -> Result<String, String> {
    let manifest: serde_json::Value = agent
        .get(&format!("{RELEASES}/{version}/manifest.json"))
        .call()
        .map_err(|e| format!("manifest: {e}"))?
        .into_json()
        .map_err(|e| format!("manifest: {e}"))?;
    manifest["platforms"][platform()]["checksum"]
        .as_str()
        .map(|s| s.to_ascii_lowercase())
        .ok_or_else(|| format!("platform {} not in manifest for {version}", platform()))
}

/// Downloads `version` into its own directory, verifying the sha256 from the
/// manifest before the file gets its real name. Progress goes to the UI.
fn download(app: &AppHandle, agent: &ureq::Agent, version: &str) -> Result<(), String> {
    let checksum = expected_checksum(agent, version)?;
    let dest = version_binary(version);
    let dir = dest.parent().unwrap();
    fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    let part = dir.join(format!("{BIN_NAME}.part"));

    let resp = agent
        .get(&format!("{RELEASES}/{version}/{}/{BIN_NAME}", platform()))
        .call()
        .map_err(|e| format!("download: {e}"))?;
    let total = resp.header("Content-Length").and_then(|s| s.parse::<u64>().ok());
    publish(Some(app), |s| {
        s.phase = "downloading".into();
        s.got = 0;
        s.total = total;
    });

    let mut reader = resp.into_reader();
    let mut file = fs::File::create(&part).map_err(|e| format!("create {}: {e}", part.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    let mut got: u64 = 0;
    let mut last_report = std::time::Instant::now();
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("download: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| format!("write: {e}"))?;
        hasher.update(&buf[..n]);
        got += n as u64;
        // A 100MB file at 256KB per chunk is 400 events; one every 200ms is
        // plenty for a progress label.
        if last_report.elapsed() > Duration::from_millis(200) {
            last_report = std::time::Instant::now();
            publish(Some(app), |s| s.got = got);
        }
    }
    file.flush().map_err(|e| e.to_string())?;
    drop(file);

    let actual = format!("{:x}", hasher.finalize());
    if actual != checksum {
        let _ = fs::remove_file(&part);
        return Err(format!("checksum mismatch for {version}: expected {checksum}, got {actual}"));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&part, fs::Permissions::from_mode(0o755));
    }
    fs::rename(&part, &dest).map_err(|e| format!("rename: {e}"))?;
    Ok(())
}

fn update(app: &AppHandle) {
    if BUSY.swap(true, Ordering::SeqCst) {
        return;
    }
    let result = update_inner(app);
    BUSY.store(false, Ordering::SeqCst);
    publish(Some(app), |s| {
        s.checked_at_ms = Some(now_ms());
        s.got = 0;
        s.total = None;
        match &result {
            Ok(()) => {
                s.phase = "idle".into();
                s.error = None;
            }
            Err(e) => {
                s.phase = "error".into();
                s.error = Some(e.clone());
            }
        }
        s.version = current_version();
        s.path = binary().to_string_lossy().into_owned();
    });
    if let Err(e) = result {
        crate::log::warn("cli", &format!("update failed: {e}"));
    }
}

fn update_inner(app: &AppHandle) -> Result<(), String> {
    publish(Some(app), |s| {
        s.phase = "checking".into();
        s.error = None;
    });
    let agent = http();
    let latest = latest_version(&agent)?;
    publish(Some(app), |s| s.latest = Some(latest.clone()));

    if current_version().as_deref() == Some(latest.as_str()) {
        prune();
        return Ok(());
    }

    if !version_binary(&latest).is_file() {
        crate::log::info("cli", &format!("downloading claude {latest} ({})", platform()));
        download(app, &agent, &latest)?;
    }

    // The flip is the last step, so a crash anywhere above leaves `current`
    // pointing at a binary that is known to be complete.
    fs::create_dir_all(root()).map_err(|e| e.to_string())?;
    fs::write(current_file(), &latest).map_err(|e| format!("write current: {e}"))?;
    crate::log::info("cli", &format!("claude {latest} is now current"));
    prune();
    Ok(())
}

/// Drops every version directory except the current one. A directory whose
/// exe a session still runs will not go on Windows; that is fine — it goes
/// on the next pass after the session ends.
fn prune() {
    let keep = current_version();
    let Ok(entries) = fs::read_dir(versions_dir()) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if Some(name.as_str()) == keep.as_deref() {
            continue;
        }
        let path = entry.path();
        if let Err(e) = remove_version_dir(&path) {
            crate::log::info("cli", &format!("keeping {}: {e}", path.display()));
        }
    }
}

fn remove_version_dir(path: &Path) -> std::io::Result<()> {
    fs::remove_dir_all(path)
}
