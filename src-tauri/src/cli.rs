//! Luna's own copies of the CLIs it drives.
//!
//! The app used to run whatever `claude` was on PATH, which tied every Luna
//! install to a global CLI living under the user profile — and to that CLI's
//! own updater writing into `~/.local/bin`. Each provider keeps a private copy
//! under Luna's data dir instead, fetched straight from its release channel,
//! so the whole thing (app, accounts, CLIs) can sit on one drive and move
//! with it.
//!
//! Layout, one tree per provider:
//!
//! ```text
//! <data>/claude-cli/
//!   current                     text file: the version sessions should spawn
//!   versions/<ver>/claude.exe   one directory per downloaded version
//! <data>/codex-cli/
//!   current
//!   versions/<ver>/bin/codex.exe (+ codex-path/, codex-resources/ from the package)
//! ```
//!
//! Versions are never overwritten in place: Windows will not let a running
//! exe be replaced, and a session that is mid-turn keeps the old one open
//! for hours. A new version lands in its own directory, `current` flips to
//! it, and old directories are swept whenever nothing holds them any more.
//!
//! This module is the machinery — layout, download, checksum, status, the
//! periodic check. Where a version comes from is each provider's `Source`
//! (`claude::cli`, `codex::cli`).

use crate::provider::Provider;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tauri::AppHandle;

const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);
const EVENT: &str = "cli://status";

/// Where a provider's releases come from and how one is put on disk.
pub struct Source {
    /// Directory under `<data>` that holds this provider's tree.
    pub root_name: &'static str,
    /// The executable, relative to a version directory.
    pub bin_rel: &'static str,
    /// What to spawn from PATH while there is no managed copy.
    pub path_fallback: &'static str,
    pub latest: fn(&ureq::Agent) -> Result<String, String>,
    /// Puts `version` into `dir`, complete and verified, or fails and leaves
    /// nothing that could pass for a finished install.
    pub fetch: fn(&ureq::Agent, &str, &Path, &mut dyn FnMut(u64, Option<u64>)) -> Result<(), String>,
}

fn source(provider: Provider) -> &'static Source {
    match provider {
        Provider::Claude => &crate::claude::cli::SOURCE,
        Provider::Codex => &crate::codex::cli::SOURCE,
    }
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct CliStatus {
    pub provider: Option<Provider>,
    /// `idle` | `checking` | `downloading` | `error`.
    pub phase: String,
    /// The version sessions spawn right now; None until the first download.
    pub version: Option<String>,
    /// Where that binary lives — the bare name on PATH while there is no managed copy.
    pub path: String,
    /// The newest version the channel offers, once a check has come back.
    pub latest: Option<String>,
    pub got: u64,
    pub total: Option<u64>,
    pub error: Option<String>,
    pub checked_at_ms: Option<u64>,
}

fn state() -> &'static Mutex<HashMap<Provider, CliStatus>> {
    static S: OnceLock<Mutex<HashMap<Provider, CliStatus>>> = OnceLock::new();
    S.get_or_init(|| {
        let mut map = HashMap::new();
        for p in Provider::ALL {
            map.insert(
                p,
                CliStatus {
                    provider: Some(p),
                    phase: "idle".into(),
                    version: current_version(p),
                    path: binary(p).to_string_lossy().into_owned(),
                    ..Default::default()
                },
            );
        }
        Mutex::new(map)
    })
}

/// One update per provider at a time: the periodic check and a click on the
/// status-bar field must not start two downloads of the same 100MB.
static BUSY: [AtomicBool; 2] = [AtomicBool::new(false), AtomicBool::new(false)];

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------- layout --

pub fn root(provider: Provider) -> PathBuf {
    crate::paths::data_dir().join(source(provider).root_name)
}

fn versions_dir(provider: Provider) -> PathBuf {
    root(provider).join("versions")
}

fn version_dir(provider: Provider, version: &str) -> PathBuf {
    versions_dir(provider).join(version)
}

fn version_binary(provider: Provider, version: &str) -> PathBuf {
    version_dir(provider, version).join(source(provider).bin_rel)
}

fn current_file(provider: Provider) -> PathBuf {
    root(provider).join("current")
}

fn current_version(provider: Provider) -> Option<String> {
    let v = fs::read_to_string(current_file(provider)).ok()?;
    let v = v.trim().to_string();
    (!v.is_empty() && version_binary(provider, &v).is_file()).then_some(v)
}

/// What a session should spawn. Falls back to PATH so a fresh install that
/// has not finished its first download still works for anyone who has the
/// CLI installed the usual way.
pub fn binary(provider: Provider) -> PathBuf {
    match current_version(provider) {
        Some(v) => version_binary(provider, &v),
        None => PathBuf::from(source(provider).path_fallback),
    }
}

// ---------------------------------------------------------------- status --

fn publish(app: Option<&AppHandle>, provider: Provider, f: impl FnOnce(&mut CliStatus)) {
    let snapshot = {
        let Ok(mut map) = state().lock() else { return };
        let Some(s) = map.get_mut(&provider) else { return };
        f(s);
        s.clone()
    };
    if let Some(app) = app {
        crate::emit::to_ui(app, EVENT, snapshot);
    }
}

#[tauri::command]
pub fn cli_status(provider: Provider) -> CliStatus {
    state()
        .lock()
        .ok()
        .and_then(|m| m.get(&provider).cloned())
        .unwrap_or_default()
}

/// A click on the status-bar field: check now, install if newer.
#[tauri::command]
pub fn cli_update_now(app: AppHandle, provider: Provider) {
    std::thread::spawn(move || update(&app, provider));
}

/// Startup: one update pass right away, then one every few hours. The first
/// pass is what installs the CLIs on a fresh machine, so it is not delayed.
pub fn refresh_periodically(app: AppHandle) {
    for provider in Provider::ALL {
        let app = app.clone();
        std::thread::spawn(move || loop {
            update(&app, provider);
            std::thread::sleep(CHECK_EVERY);
        });
    }
}

// ---------------------------------------------------------------- update --

pub fn http() -> ureq::Agent {
    ureq::builder()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(60))
        .build()
}

/// A version string that came off the network and is about to become a
/// directory name: digits-and-dots, nothing an HTML error page could pass for.
pub fn sane_version(v: &str, what: &str) -> Result<String, String> {
    let v = v.trim().to_string();
    let ok = v.split('.').take(3).all(|p| p.chars().take_while(|c| c.is_ascii_digit()).count() > 0)
        && v.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    if !ok || v.len() > 64 {
        return Err(format!("unexpected version text from {what}: {:?}", v.chars().take(40).collect::<String>()));
    }
    Ok(v)
}

/// Streams `url` into `part`, hashing as it goes, and returns the sha256 hex.
/// The caller compares it and decides what the file becomes.
pub fn download_to(
    agent: &ureq::Agent,
    url: &str,
    part: &Path,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<String, String> {
    let resp = agent.get(url).call().map_err(|e| format!("download: {e}"))?;
    let total = resp.header("Content-Length").and_then(|s| s.parse::<u64>().ok());
    progress(0, total);

    let mut reader = resp.into_reader();
    let mut file = fs::File::create(part).map_err(|e| format!("create {}: {e}", part.display()))?;
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
            progress(got, total);
        }
    }
    file.flush().map_err(|e| e.to_string())?;
    drop(file);
    Ok(format!("{:x}", hasher.finalize()))
}

fn update(app: &AppHandle, provider: Provider) {
    let busy = &BUSY[provider.index()];
    if busy.swap(true, Ordering::SeqCst) {
        return;
    }
    let result = update_inner(app, provider);
    busy.store(false, Ordering::SeqCst);
    publish(Some(app), provider, |s| {
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
        s.version = current_version(provider);
        s.path = binary(provider).to_string_lossy().into_owned();
    });
    if let Err(e) = result {
        crate::log::warn("cli", &format!("{} update failed: {e}", provider.as_str()));
    }
}

fn update_inner(app: &AppHandle, provider: Provider) -> Result<(), String> {
    publish(Some(app), provider, |s| {
        s.phase = "checking".into();
        s.error = None;
    });
    let src = source(provider);
    let agent = http();
    let latest = (src.latest)(&agent)?;
    publish(Some(app), provider, |s| s.latest = Some(latest.clone()));

    if current_version(provider).as_deref() == Some(latest.as_str()) {
        prune(provider);
        return Ok(());
    }

    if !version_binary(provider, &latest).is_file() {
        crate::log::info("cli", &format!("downloading {} {latest}", provider.as_str()));
        let dir = version_dir(provider, &latest);
        // A half-extracted directory from an earlier crash must not pass for
        // a finished one on the next attempt.
        if dir.exists() {
            let _ = fs::remove_dir_all(&dir);
        }
        fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
        publish(Some(app), provider, |s| {
            s.phase = "downloading".into();
            s.got = 0;
            s.total = None;
        });
        let mut progress = |got: u64, total: Option<u64>| {
            publish(Some(app), provider, |s| {
                s.got = got;
                s.total = total;
            });
        };
        if let Err(e) = (src.fetch)(&agent, &latest, &dir, &mut progress) {
            let _ = fs::remove_dir_all(&dir);
            return Err(e);
        }
        if !version_binary(provider, &latest).is_file() {
            let _ = fs::remove_dir_all(&dir);
            return Err(format!("{latest} downloaded but {} is missing", src.bin_rel));
        }
    }

    // The flip is the last step, so a crash anywhere above leaves `current`
    // pointing at a binary that is known to be complete.
    fs::create_dir_all(root(provider)).map_err(|e| e.to_string())?;
    fs::write(current_file(provider), &latest).map_err(|e| format!("write current: {e}"))?;
    crate::log::info("cli", &format!("{} {latest} is now current", provider.as_str()));
    prune(provider);
    Ok(())
}

/// Drops every version directory except the current one. A directory whose
/// exe a session still runs will not go on Windows; that is fine — it goes
/// on the next pass after the session ends.
fn prune(provider: Provider) {
    let keep = current_version(provider);
    let Ok(entries) = fs::read_dir(versions_dir(provider)) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if Some(name.as_str()) == keep.as_deref() {
            continue;
        }
        let path = entry.path();
        if let Err(e) = fs::remove_dir_all(&path) {
            crate::log::info("cli", &format!("keeping {}: {e}", path.display()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::sane_version;

    #[test]
    fn rejects_html_and_accepts_versions() {
        assert!(sane_version("2.1.252", "x").is_ok());
        assert!(sane_version("0.154.0\n", "x").is_ok());
        assert!(sane_version("<html>", "x").is_err());
        assert!(sane_version("", "x").is_err());
    }
}
