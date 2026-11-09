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
//!   current                     text file: the version directory sessions spawn from
//!   previous                    text file: the one `current` named before, kept to go back to
//!   versions/<ver>/claude.exe   one directory per installed version
//!   versions/<ver>/luna-install.json   its completion record
//!   staging/<ver>-<random>/     a download in progress
//! <data>/codex-cli/
//!   current
//!   versions/<ver>/bin/codex.exe (+ codex-path/, codex-resources/ from the package)
//!   versions/<ver>/luna-install.json
//!   staging/
//!   in-use/<dir>                held (shared) by each process started from <dir> (`in_use`)
//! ```
//!
//! Versions are never overwritten in place: Windows will not let a running
//! exe be replaced, and a session that is mid-turn keeps the old one open
//! for hours. A new version is fetched into `staging/`, held to what its
//! release says the package is, given its completion record last, and
//! renamed into `versions/` under a name nothing has (`<ver>`, or `<ver>~<n>`
//! while an older `<ver>` directory is still there). `current` then flips to
//! it, `previous` to what it named before, and old directories are swept
//! whenever nothing runs from them any more; the previous one stays, to go
//! back to by hand ("Use" in Settings).
//! A directory counts as installed by its record alone, so a crash
//! mid-download or mid-unpack leaves nothing that passes for one.
//!
//! This module is the machinery — layout, download, checksum, status, the
//! periodic check. Where a version comes from is each provider's `Source`
//! (`claude::cli`, `codex::cli`).

mod canary;
mod choice;
mod in_use;
mod known_good;
mod probe;

pub use canary::{session_ended, Canary};
pub use choice::Channel;
pub use in_use::{session_version, InUse};
#[cfg(test)]
pub(crate) use probe::covers as probe_covers;

use crate::provider::Provider;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);
const EVENT: &str = "cli://status";

/// Where a provider's releases come from and how one is put on disk.
pub struct Source {
    /// Directory under `<data>` that holds this provider's tree.
    pub root_name: &'static str,
    /// The executable, relative to a version directory, `/`-separated.
    pub bin_rel: &'static str,
    /// What to spawn from PATH while there is no managed copy.
    pub path_fallback: &'static str,
    pub latest: fn(&ureq::Agent) -> Result<String, String>,
    /// Puts `version` into `dir`, an empty staging directory, and says what
    /// it put there. What it leaves on failure goes with the directory.
    pub fetch: fn(&ureq::Agent, &str, &Path, &mut dyn FnMut(u64, Option<u64>)) -> Result<Package, String>,
}

/// What a fetch vouches for, from the release's own account of it: every
/// file of the package. `validate` holds the staged directory to it.
pub struct Package {
    pub files: Vec<PackageFile>,
    /// How the source checked what it fetched, kept in the record.
    pub verified: String,
}

pub struct PackageFile {
    /// Relative to the version directory, `/`-separated.
    pub rel: String,
    pub size: u64,
    /// Where the release publishes one for this very file.
    pub sha256: Option<String>,
}

/// The completion record: written into the staged directory after
/// everything else, before the rename that makes it a version.
const RECORD: &str = "luna-install.json";

/// At a provider's root once this code has installed there, before it first
/// points `current` at anything: from then on a version without its record
/// is never run (`current_in`). A tree without it is from before records,
/// whose `current` was only ever moved to a finished install.
const RECORDED: &str = "recorded";

/// At a provider's root: the directory `current` named before its last flip.
const PREVIOUS: &str = "previous";

#[derive(Serialize, Deserialize)]
pub(crate) struct Record {
    version: String,
    verified: String,
    files: Vec<RecordFile>,
}

#[derive(Serialize, Deserialize)]
struct RecordFile {
    path: String,
    size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sha256: Option<String>,
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
    /// The newest version the CLI's own release channel offers, once a check
    /// has come back.
    pub latest: Option<String>,
    /// Which versions the update pass installs.
    pub channel: Channel,
    pub got: u64,
    pub total: Option<u64>,
    pub error: Option<String>,
    /// Why the last pass left `current` where it was, when that was on
    /// purpose rather than for want of anything newer.
    pub note: Option<String>,
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
                    channel: choice::channel_in(&root(p)),
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
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

// ---------------------------------------------------------------- layout --

pub fn root(provider: Provider) -> PathBuf {
    crate::paths::data_dir().join(source(provider).root_name)
}

/// What `current` names: the directory sessions spawn from and the version
/// in it.
struct Current {
    dir: String,
    version: String,
    /// False for a directory from before completion records, which `current`
    /// was only ever pointed at once its install had finished. Sessions keep
    /// spawning it; the update pass does not count it as installed and
    /// fetches its version again beside it.
    recorded: bool,
}

/// A directory name out of `current`, or one component of a package path.
fn plain_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', ':'])
}

/// A relative path out of a package: plain components only.
fn plain_rel(rel: &str) -> bool {
    rel.split('/').all(plain_name)
}

fn read_record(dir: &Path) -> std::io::Result<Record> {
    let bytes = fs::read(dir.join(RECORD))?;
    serde_json::from_slice(&bytes).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// Every file the record lists is there, a file and not a link, at its size,
/// the executable among them. Sizes only: this runs for every spawn, and the
/// hashes were checked before the record was written.
fn intact(dir: &Path, record: &Record, bin_rel: &str) -> bool {
    record.files.iter().any(|f| f.path == bin_rel)
        && record.files.iter().all(|f| {
            plain_rel(&f.path) && fs::symlink_metadata(dir.join(&f.path)).is_ok_and(|m| m.is_file() && !is_link(&m) && m.len() == f.size)
        })
}

fn current_in(root: &Path, bin_rel: &str) -> Option<Current> {
    let name = fs::read_to_string(root.join("current")).ok()?;
    let name = name.trim();
    if !plain_name(name) {
        return None;
    }
    let dir = root.join("versions").join(name);
    match read_record(&dir) {
        Ok(record) => intact(&dir, &record, bin_rel).then(|| Current { dir: name.into(), version: record.version, recorded: true }),
        // Without its record, a directory is run only in a tree this code has
        // never installed into: there it can only be from before records. In
        // one it has, it is an install whose record is gone, which proves
        // nothing about the files beside it.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && fs::symlink_metadata(root.join(RECORDED)).is_err() => {
            dir.join(bin_rel).is_file().then(|| Current { dir: name.into(), version: name.into(), recorded: false })
        }
        Err(_) => None,
    }
}

/// A recorded, intact directory of `version` under `versions/`: one a crash
/// left between its promotion and the flip of `current`.
fn installed_in(root: &Path, bin_rel: &str, version: &str) -> Option<String> {
    let suffixed = format!("{version}~");
    fs::read_dir(root.join("versions")).ok()?.flatten().find_map(|entry| {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name != version && !name.starts_with(&suffixed) {
            return None;
        }
        let meta = fs::symlink_metadata(entry.path()).ok()?;
        if is_link(&meta) || !meta.is_dir() {
            return None;
        }
        let record = read_record(&entry.path()).ok()?;
        (record.version == version && intact(&entry.path(), &record, bin_rel)).then_some(name)
    })
}

pub(crate) fn current_version(provider: Provider) -> Option<String> {
    current_in(&root(provider), source(provider).bin_rel).map(|c| c.version)
}

/// What a session should spawn. Falls back to PATH so a fresh install that
/// has not finished its first download still works for anyone who has the
/// CLI installed the usual way.
pub fn binary(provider: Provider) -> PathBuf {
    let src = source(provider);
    let root = root(provider);
    match current_in(&root, src.bin_rel) {
        Some(c) => root.join("versions").join(c.dir).join(src.bin_rel),
        None => PathBuf::from(src.path_fallback),
    }
}

/// `binary`, for a process that is about to start: with a hold on its
/// version directory that the caller keeps until the process has exited, so
/// the sweep leaves that directory be (`in_use`). `session` is the chat or
/// helper it is for, whose version `luna_session_get` then shows. The bare
/// name on PATH, unheld, only while nothing is current; a managed copy that
/// cannot be held is not started at all.
pub fn launch(provider: Provider, session: Option<&str>) -> Result<(PathBuf, Option<InUse>), String> {
    let src = source(provider);
    let root = root(provider);
    match launch_in(&root, src.bin_rel, min_safe(provider))? {
        Some((path, version, held)) => {
            if let Some(session) = session {
                in_use::note_session(session, provider, &version);
                canary::session_started(provider, &root, &version, session);
            }
            Ok((path, Some(held)))
        }
        None => Ok((PathBuf::from(src.path_fallback), None)),
    }
}

/// How often a launch tries for its hold: `current` moving under it, or the
/// lock file not to be had for a moment.
const HOLD_TRIES: u32 = 5;

/// The exe `current` names under `root`, its version, and a hold on its
/// directory taken before `current` is read a second time: a flip and a
/// sweep between the two reads would have taken the directory the first one
/// named. None where nothing is current. An Err rather than a start without
/// the hold: the sweep could later take the folder from under the process.
/// An Err too, never the CLI on PATH, for a current version below `min_safe`:
/// a Luna release that raised its minimum runs nothing older, whatever
/// `current` or a version held by hand says, until the next pass moves it.
fn launch_in(root: &Path, bin_rel: &str, min_safe: &str) -> Result<Option<(PathBuf, String, InUse)>, String> {
    let mut why = String::new();
    for attempt in 0..HOLD_TRIES {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(20 * u64::from(attempt)));
        }
        let Some(c) = current_in(root, bin_rel) else { return Ok(None) };
        if version_cmp(&c.version, min_safe).is_lt() {
            return Err(format!(
                "not started: version {} is older than {min_safe}, the oldest this Luna lets run; Settings → CLIs installs a newer one",
                c.version
            ));
        }
        let Some(held) = in_use::hold(root, &c.dir) else {
            why = format!("cannot hold {} in {} (a lock file there cannot be had)", c.dir, root.join(in_use::DIR).display());
            continue;
        };
        if current_in(root, bin_rel).is_some_and(|again| again.dir == c.dir) {
            return Ok(Some((root.join("versions").join(&c.dir).join(bin_rel), c.version, held)));
        }
        why = "the current version kept changing while it was read".into();
    }
    Err(format!("not started: {why}"))
}

// ---------------------------------------------------------------- status --

fn publish(provider: Provider, f: impl FnOnce(&mut CliStatus)) {
    let snapshot = {
        let Ok(mut map) = state().lock() else { return };
        let Some(s) = map.get_mut(&provider) else { return };
        f(s);
        s.clone()
    };
    crate::emit::to_ui(EVENT, snapshot);
}

#[tauri::command]
pub fn cli_status(provider: Provider) -> CliStatus {
    state().lock().ok().and_then(|m| m.get(&provider).cloned()).unwrap_or_default()
}

/// A click on the status-bar field: check now, install if newer.
#[tauri::command]
pub fn cli_update_now(provider: Provider) {
    std::thread::spawn(move || update(provider));
}

/// One installed version, as Settings → CLIs lists it.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VersionRow {
    /// Its directory under `versions/`: what "Use" names.
    pub dir: String,
    pub version: String,
    pub current: bool,
    pub previous: bool,
    /// A process of Luna's runs from it now.
    pub in_use: bool,
    /// On this Luna's shipped list.
    pub known_good: bool,
    /// Marked bad, and why.
    pub bad: Option<String>,
}

/// A provider's installed versions and what each is to Luna.
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct CliVersions {
    pub provider: Option<Provider>,
    pub channel: Channel,
    /// Newest first.
    pub versions: Vec<VersionRow>,
    /// The shipped list, installed or not.
    pub known_good: Vec<String>,
    /// Nothing older runs.
    pub min_safe: String,
    /// The newest version that has run here: nothing older is downloaded.
    pub floor: String,
    /// The directory "Use" holds `current` at, while it does.
    pub pinned: Option<String>,
    /// Every version marked bad, installed or not, and why.
    pub bad: std::collections::BTreeMap<String, String>,
    /// The watch over the current version's first sessions, while it is the
    /// current one's.
    pub canary: Option<Canary>,
}

/// Every recorded, intact install under `root`, newest first.
fn installed_all(root: &Path, bin_rel: &str) -> Vec<(String, Record)> {
    let Ok(entries) = fs::read_dir(root.join("versions")) else { return Vec::new() };
    let mut out: Vec<(String, Record)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let meta = fs::symlink_metadata(entry.path()).ok()?;
            if !plain_name(&name) || is_link(&meta) || !meta.is_dir() {
                return None;
            }
            let record = read_record(&entry.path()).ok()?;
            intact(&entry.path(), &record, bin_rel).then_some((name, record))
        })
        .collect();
    out.sort_by(|a, b| version_cmp(&b.1.version, &a.1.version).then_with(|| b.0.cmp(&a.0)));
    out
}

fn versions_in(root: &Path, bin_rel: &str, known: &known_good::Entry) -> CliVersions {
    let current = current_in(root, bin_rel);
    let canary = canary::read_in(root).filter(|c| current.as_ref().is_some_and(|cur| cur.version == c.version));
    let current = current.map(|c| c.dir);
    let previous = previous_in(root, bin_rel);
    let bad = choice::bad_in(root);
    let versions = installed_all(root, bin_rel)
        .into_iter()
        .map(|(dir, record)| VersionRow {
            current: current.as_deref() == Some(dir.as_str()),
            previous: previous.as_deref() == Some(dir.as_str()),
            in_use: in_use::busy(root, &dir),
            known_good: known.known_good.contains(&record.version),
            bad: bad.get(&record.version).cloned(),
            version: record.version,
            dir,
        })
        .collect();
    CliVersions {
        provider: None,
        channel: choice::channel_in(root),
        versions,
        known_good: known.known_good.clone(),
        min_safe: known.min_safe.clone(),
        floor: floor_in(root, &known.min_safe),
        pinned: choice::pinned_in(root),
        bad,
        canary,
    }
}

/// "Keep it" on a canary warning: the badge goes, the version stays.
#[tauri::command]
pub fn cli_canary_dismiss(provider: Provider) -> Result<(), String> {
    canary::dismiss_in(&root(provider))
}

#[tauri::command]
pub fn cli_versions(provider: Provider) -> CliVersions {
    let src = source(provider);
    CliVersions { provider: Some(provider), ..versions_in(&root(provider), src.bin_rel, known_good::entry(provider)) }
}

/// The channel a provider follows from now on; a version held by hand is let
/// go, and a pass runs at once.
#[tauri::command]
pub fn cli_set_channel(provider: Provider, channel: Channel) -> Result<(), String> {
    choice::set_channel_in(&root(provider), channel)?;
    crate::log::info("cli", &format!("{} follows the {channel:?} channel", provider.as_str()));
    publish(provider, |s| s.channel = channel);
    std::thread::spawn(move || update(provider));
    Ok(())
}

/// "Mark bad": no pass installs `version` or makes it current again. One
/// that is current stays until something else is: on the known-good channel
/// the pass that runs now moves to the next version on the list, and on
/// either "Use" goes back by hand.
#[tauri::command]
pub fn cli_mark_bad(provider: Provider, version: String, why: Option<String>) -> Result<(), String> {
    choice::mark_bad_in(&root(provider), &version, why.as_deref().unwrap_or(""))?;
    crate::log::info("cli", &format!("{} {version} marked bad: {}", provider.as_str(), why.unwrap_or_default()));
    std::thread::spawn(move || update(provider));
    Ok(())
}

/// "Use" in Settings: makes the installed directory `dir` current and holds
/// it there, the update pass leaving it be until a channel is chosen again.
/// Running sessions keep what they run; new ones start on it.
#[tauri::command]
pub async fn cli_use_version(provider: Provider, dir: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || use_version(provider, &dir)).await.map_err(|e| e.to_string())?
}

fn use_version(provider: Provider, dir: &str) -> Result<(), String> {
    let busy = &BUSY[provider.index()];
    if busy.swap(true, Ordering::SeqCst) {
        return Err("an update of this CLI is under way; try again once it is done".into());
    }
    let src = source(provider);
    let root = root(provider);
    let result = use_version_in(&root, src.bin_rel, min_safe(provider), dir).and_then(|v| choice::pin_in(&root, dir).map(|()| v));
    busy.store(false, Ordering::SeqCst);
    match &result {
        Ok(version) => crate::log::info("cli", &format!("{} {version} is now current by hand (versions/{dir})", provider.as_str())),
        Err(e) => crate::log::warn("cli", &format!("{} use {dir}: {e}", provider.as_str())),
    }
    publish(provider, |s| {
        s.version = current_version(provider);
        s.path = binary(provider).to_string_lossy().into_owned();
    });
    result.map(|_| ())
}

/// Makes the installed directory `dir` current, as the update pass would,
/// and returns its version. Below the floor too, the choice being the
/// person's rather than the channel's: once every file is held again to the
/// sha256 its record kept, and never below `min_safe`.
fn use_version_in(root: &Path, bin_rel: &str, min_safe: &str, dir: &str) -> Result<String, String> {
    // The floor as it is, before it decides how far the version is checked:
    // a floor left below what is current would let an older version pass
    // as above it, and the flip would then raise it to that one only.
    reconcile_in(root, bin_rel)?;
    let Some((_, record)) = installed_all(root, bin_rel).into_iter().find(|(d, _)| d == dir) else {
        return Err(format!("{dir} is not an installed version"));
    };
    if version_cmp(&record.version, min_safe).is_lt() {
        return Err(format!("{} is older than {min_safe}, the oldest this Luna lets run", record.version));
    }
    // Held to its record again whichever way it goes: a version gone back to
    // has sat on disk unwatched since it ran. Below the floor every file
    // must have a sha256 to be held to; above it, a record from before every
    // file got one (Codex's) is held to what it has.
    let floor = floor_in(root, min_safe);
    let below = version_cmp(&record.version, &floor).is_lt();
    reverify(&root.join("versions").join(dir), &record, below).map_err(|e| match below {
        true => format!("{} is below {floor}, and {e}", record.version),
        false => format!("{}: {e}", record.version),
    })?;
    mark_recorded(root)?;
    flip_in(root, bin_rel, dir)?;
    prune_in(root, bin_rel);
    Ok(record.version)
}

// ----------------------------------------------------------------- floor --

/// At a provider's root: the highest version that has been current there.
/// Nothing below it is downloaded, nor taken from the channel's pointer,
/// which is not signed: a release server in other hands could otherwise
/// point back at an older release with its known flaws. It rises when a
/// version becomes current (`flip_in`), not when one is downloaded, so a
/// version fetched and then refused does not shut out the one before it.
const FLOOR: &str = "floor";
/// Claude Code's floor from before, raised as each signed manifest was
/// accepted: still honored, never written.
const LEGACY_FLOOR: &str = "signed-floor";

/// The oldest version of each CLI this Luna lets run at all (`known_good`).
pub(crate) fn min_safe(provider: Provider) -> &'static str {
    known_good::min_safe(provider)
}

/// The floor under `root`: the highest of `min_safe` and what the floor
/// files say.
pub(crate) fn floor_in(root: &Path, min_safe: &str) -> String {
    [FLOOR, LEGACY_FLOOR]
        .iter()
        .filter_map(|f| fs::read_to_string(root.join(f)).ok().and_then(|t| sane_version(&t, f).ok()))
        .fold(min_safe.to_string(), |high, v| if version_cmp(&v, &high).is_gt() { v } else { high })
}

/// Refuses `version` below the floor under `root`.
pub(crate) fn at_or_above_floor_in(root: &Path, min_safe: &str, version: &str) -> Result<(), String> {
    let floor = floor_in(root, min_safe);
    if version_cmp(version, &floor).is_lt() {
        return Err(format!("refusing {version}: older than {floor}, the newest version this Luna has run (or the oldest it takes)"));
    }
    Ok(())
}

/// Moves the floor up to `version`, never down.
fn raise_floor_in(root: &Path, version: &str) -> Result<(), String> {
    let kept = fs::read_to_string(root.join(FLOOR)).ok().and_then(|t| sane_version(&t, FLOOR).ok());
    if kept.is_some_and(|k| version_cmp(version, &k).is_le()) {
        return Ok(());
    }
    crate::secret::write_private_synced(&root.join(FLOOR), version.as_bytes()).map_err(|e| format!("write {FLOOR}: {e}"))
}

/// Every file of an install held again to the sha256 its record kept, for a
/// version gone back to. `all`: below the floor, where nothing newer vouches
/// for it, a record without a hash for each file cannot be held so and is
/// refused; otherwise such a file is passed over.
fn reverify(dir: &Path, record: &Record, all: bool) -> Result<(), String> {
    for f in &record.files {
        let Some(want) = &f.sha256 else {
            if all {
                return Err(format!("its record keeps no sha256 for {} to check it against", f.path));
            }
            continue;
        };
        let path = dir.join(&f.path);
        let meta = fs::symlink_metadata(&path).map_err(|e| format!("{} is missing: {e}", f.path))?;
        if is_link(&meta) || !meta.is_file() {
            return Err(format!("{} is not a file", f.path));
        }
        let got = sha256_of(&path)?;
        if !got.eq_ignore_ascii_case(want) {
            return Err(format!("{} no longer matches its sha256", f.path));
        }
    }
    Ok(())
}

/// Startup: one update pass right away, then one every few hours. The first
/// pass is what installs the CLIs on a fresh machine, so it is not delayed.
pub fn refresh_periodically() {
    for provider in Provider::ALL {
        std::thread::spawn(move || loop {
            update(provider);
            std::thread::sleep(CHECK_EVERY);
        });
    }
}

// ---------------------------------------------------------------- update --

pub fn http() -> ureq::Agent {
    ureq::builder().timeout_connect(Duration::from_secs(10)).timeout_read(Duration::from_secs(60)).build()
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

/// Orders two `sane_version`s by their dotted numbers (each part's leading
/// digits; a missing part counts as 0), so 2.1.100 is above 2.1.99.
pub(crate) fn version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let parts = |v: &str| -> Vec<u64> {
        v.split('.').map(|p| p.chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap_or(0)).collect()
    };
    let (a, b) = (parts(a), parts(b));
    (0..a.len().max(b.len()))
        .map(|i| a.get(i).unwrap_or(&0).cmp(b.get(i).unwrap_or(&0)))
        .find(|o| o.is_ne())
        .unwrap_or(std::cmp::Ordering::Equal)
}

/// Streams `url` into `part`, hashing as it goes, and returns the sha256 hex.
/// The caller compares it and decides what the file becomes.
pub fn download_to(agent: &ureq::Agent, url: &str, part: &Path, progress: &mut dyn FnMut(u64, Option<u64>)) -> Result<String, String> {
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

fn update(provider: Provider) {
    let busy = &BUSY[provider.index()];
    if busy.swap(true, Ordering::SeqCst) {
        return;
    }
    let result = update_inner(provider);
    busy.store(false, Ordering::SeqCst);
    publish(provider, |s| {
        s.checked_at_ms = Some(now_ms());
        s.got = 0;
        s.total = None;
        match &result {
            Ok(note) => {
                s.phase = "idle".into();
                s.error = None;
                s.note = note.clone();
            }
            Err(e) => {
                s.phase = "error".into();
                s.error = Some(e.clone());
                s.note = None;
            }
        }
        s.version = current_version(provider);
        s.path = binary(provider).to_string_lossy().into_owned();
    });
    match result {
        Err(e) => crate::log::warn("cli", &format!("{} update failed: {e}", provider.as_str())),
        Ok(Some(note)) => crate::log::info("cli", &format!("{}: {note}", provider.as_str())),
        Ok(None) => {}
    }
}

/// One pass: what the channel names, made current. Ok with a note where
/// `current` was left where it is on purpose.
fn update_inner(provider: Provider) -> Result<Option<String>, String> {
    let root = root(provider);
    let channel = choice::channel_in(&root);
    publish(provider, |s| {
        s.phase = "checking".into();
        s.error = None;
        s.channel = channel;
    });
    if let Some(why) = held_for_elevation() {
        return Ok(Some(why));
    }
    let src = source(provider);
    // Before any way out of the pass, a version held by hand among them.
    reconcile_in(&root, src.bin_rel)?;
    let agent = http();
    // Asked on either channel: on the known-good one it is news for the
    // status line, and only `latest` fails without it.
    let latest = (src.latest)(&agent);
    if let Ok(l) = &latest {
        publish(provider, |s| s.latest = Some(l.clone()));
    }
    let min = min_safe(provider);
    let pinned = choice::pinned_in(&root).and_then(|dir| match installed_all(&root, src.bin_rel).into_iter().find(|(d, _)| *d == dir) {
        Some((_, record)) if version_cmp(&record.version, min).is_ge() => Some(record.version),
        // Held at a version that is gone, or that this Luna no longer lets
        // run (its minimum rose): nothing left to hold.
        _ => {
            let _ = choice::unpin_in(&root);
            None
        }
    });
    // A current version below the minimum runs no session (`launch_in`):
    // the pass moves off it whatever would otherwise keep it.
    let unsafe_now = current_in(&root, src.bin_rel).is_some_and(|c| version_cmp(&c.version, min).is_lt());
    let bad = choice::bad_in(&root);
    let latest = latest.as_ref().map(String::as_str).map_err(String::as_str);
    let (version, chosen) = match choice::plan(channel, pinned.as_deref(), known_good::entry(provider), &bad, latest, unsafe_now)? {
        choice::Plan::Stay(why) => return Ok(Some(why)),
        choice::Plan::Install { version, chosen } => (version, chosen),
    };
    // Marks a version that fails it bad, with why: no pass offers it again.
    // A probe refused for elevation says nothing of the version.
    let probe = |exe: &Path| {
        probe_in(&root, provider, exe, &version).inspect_err(|e| {
            if crate::elevation::refusal().is_some() {
                return;
            }
            if let Err(m) = choice::mark_bad_in(&root, &version, &format!("failed the offline probe: {e}")) {
                crate::log::warn("cli", &m);
            }
        })
    };
    let target = Target { version: &version, min_safe: min, chosen, probe: &probe };
    if let Some(why) = held_below_floor(&root, src.bin_rel, target).filter(|_| !unsafe_now) {
        return Ok(Some(why));
    }

    let mut progress = |got: u64, total: Option<u64>| {
        publish(provider, |s| {
            s.got = got;
            s.total = total;
        });
    };
    let mut fetch = |dir: &Path| {
        crate::log::info("cli", &format!("downloading {} {version}", provider.as_str()));
        publish(provider, |s| {
            s.phase = "downloading".into();
            s.got = 0;
            s.total = None;
        });
        (src.fetch)(&agent, &version, dir, &mut progress)
    };
    let (dir, flipped) = update_in(&root, src.bin_rel, target, &mut fetch)?;
    if flipped {
        crate::log::info("cli", &format!("{} {version} is now current (versions/{dir})", provider.as_str()));
    }
    Ok(None)
}

/// Why the pass changes nothing in a process that runs as administrator: a
/// new version would have to pass the offline probe, which runs the CLI, and
/// nothing of Luna's runs a CLI elevated (elevation.rs, F11). Nothing is
/// fetched or marked bad meanwhile; the next pass of a Luna not run so goes on.
fn held_for_elevation() -> Option<String> {
    crate::elevation::refusal().map(|why| format!("no CLI update while Luna runs as administrator: {why}"))
}

/// `probe::probe` of `exe` in a scratch home under `staging/`, which the next
/// pass sweeps if this one cannot.
fn probe_in(root: &Path, provider: Provider, exe: &Path, version: &str) -> Result<(), String> {
    let home = clean_staging(root)?.join(format!("probe-{}", crate::secret::random_hex(4)));
    fs::create_dir(&home).map_err(|e| format!("mkdir {}: {e}", home.display()))?;
    crate::log::info("cli", &format!("probing {} {version}", provider.as_str()));
    let probed = probe::probe(provider, exe, version, &home);
    let _ = remove_tree(&home);
    probed
}

/// What an update pass is to make current.
#[derive(Clone, Copy)]
struct Target<'a> {
    version: &'a str,
    /// Nothing older is taken however it is named.
    min_safe: &'a str,
    /// Named by this Luna's own list (the known-good channel), not by the
    /// channel's pointer off the network: it may be gone back to below the
    /// floor while it is installed, as "Use" may.
    chosen: bool,
    /// The offline probe (`probe`) of the exe at the path given, which a
    /// version must pass before it becomes current.
    probe: &'a dyn Fn(&Path) -> Result<(), String>,
}

/// Why the pass leaves `current` be rather than take `target`: a version of
/// the shipped list below the floor that is no longer installed is not
/// downloaded again, so a Luna whose CLI had moved on past its list stays
/// where it is until a newer list comes with a Luna update.
fn held_below_floor(root: &Path, bin_rel: &str, target: Target) -> Option<String> {
    let floor = floor_in(root, target.min_safe);
    let below = target.chosen && version_cmp(target.version, &floor).is_lt();
    (below && installed_in(root, bin_rel, target.version).is_none()).then(|| {
        format!(
            "{}, the newest known-good version, is older than {floor}, which has run here, and is no longer installed: staying until a Luna update brings a newer one",
            target.version
        )
    })
}

/// The update pass on one provider's tree once the channel has named its
/// version. Nothing to fetch when `current` holds a recorded install of it,
/// or when one is already in `versions/`; otherwise `fetch` stages a fresh
/// one. A version below the floor is refused when the channel's pointer
/// named it (the pointer is not signed) and never downloaded; one from the
/// shipped list may be gone back to while installed, once its files match
/// their recorded sha256 again. Then the flip and the sweep. Returns the
/// directory now current and whether `current` moved.
fn update_in(
    root: &Path,
    bin_rel: &str,
    target: Target,
    fetch: &mut dyn FnMut(&Path) -> Result<Package, String>,
) -> Result<(String, bool), String> {
    let version = target.version;
    reconcile_in(root, bin_rel)?;
    if let Some(c) = current_in(root, bin_rel).filter(|c| c.recorded && c.version == version) {
        // `current` names a recorded install, so this code installed here:
        // a marker missing is one a crash or a cleanup took after the fact.
        mark_recorded(root)?;
        prune_in(root, bin_rel);
        return Ok((c.dir, false));
    }
    if version_cmp(version, target.min_safe).is_lt() {
        return Err(format!("refusing {version}: older than {}, the oldest this Luna lets run", target.min_safe));
    }
    // Before the fetch, and for an install already there alike: an older
    // recorded version left in `versions/` would otherwise be flipped back
    // to with no fetch.
    let below = at_or_above_floor_in(root, target.min_safe, version);
    if !target.chosen {
        below.clone()?;
    }
    let dir = match (installed_in(root, bin_rel, version), below) {
        (Some(dir), Ok(())) => dir,
        (Some(dir), Err(_)) => {
            let record = read_record(&root.join("versions").join(&dir)).map_err(|e| format!("{dir}: {e}"))?;
            reverify(&root.join("versions").join(&dir), &record, true).map_err(|e| format!("{version}: {e}"))?;
            dir
        }
        (None, Err(e)) => return Err(e),
        (None, Ok(())) => install_in(root, bin_rel, version, fetch)?,
    };
    // Whole and where it will run from, before anything runs it as a
    // session. One that fails stays out of `current`, and the sweep takes it.
    if let Err(e) = (target.probe)(&root.join("versions").join(&dir).join(bin_rel)) {
        prune_in(root, bin_rel);
        return Err(format!("{version} failed the offline probe: {e}"));
    }
    mark_recorded(root)?;
    flip_in(root, bin_rel, &dir)?;
    prune_in(root, bin_rel);
    Ok((dir, true))
}

/// Points `current` at `dir`, an installed directory, and `previous` at what
/// `current` named before. The flip is the last step of an install, so a
/// crash anywhere before it leaves `current` pointing at a directory that is
/// known to be complete; `previous` is written first, so a crash between the
/// two leaves both naming the old one.
/// Through a temp file: the core reads `current` for every spawn, and a read
/// between a plain write's truncate and its bytes would find no version and
/// spawn whatever is on PATH. Private as it is made, under a random name: in
/// a data dir open to others a `current` of theirs would pick the binary
/// every session runs. Synced, so a power cut leaves the old name or the new
/// one. Then the floor rises to the version, now that it runs.
fn flip_in(root: &Path, bin_rel: &str, dir: &str) -> Result<(), String> {
    let record = read_record(&root.join("versions").join(dir)).map_err(|e| format!("{dir}: {e}"))?;
    let old = current_in(root, bin_rel);
    if let Some(old) = old.as_ref().filter(|c| c.dir != dir) {
        crate::secret::write_private_synced(&root.join(PREVIOUS), old.dir.as_bytes()).map_err(|e| format!("write {PREVIOUS}: {e}"))?;
    }
    // Everything that goes with the switch is on disk before `current`
    // names the version, so a crash or a failed write anywhere leaves the
    // old version current under a floor that is, at worst, already the new
    // one's: never the new version current under the old floor, which the
    // unsigned pointer could then undercut. `reconcile_in` mends a tree
    // from before this order.
    raise_floor_in(root, &record.version)?;
    // Another version from now on: its first sessions are watched afresh.
    if old.is_none_or(|o| o.version != record.version) {
        canary::reset_in(root, &record.version)?;
    }
    crate::secret::write_private_synced(&root.join("current"), dir.as_bytes()).map_err(|e| format!("write current: {e}"))
}

/// The floor up to the current version, before a pass takes anything: a
/// switch cut short by a crash (or made before the floor rose with the
/// switch) can have left the floor below what is current.
fn reconcile_in(root: &Path, bin_rel: &str) -> Result<(), String> {
    match current_in(root, bin_rel).filter(|c| c.recorded) {
        Some(c) => raise_floor_in(root, &c.version),
        None => Ok(()),
    }
}

/// The directory `previous` names, while it is still a whole install.
fn previous_in(root: &Path, bin_rel: &str) -> Option<String> {
    let name = fs::read_to_string(root.join(PREVIOUS)).ok()?;
    let name = name.trim();
    if !plain_name(name) {
        return None;
    }
    let dir = root.join("versions").join(name);
    let record = read_record(&dir).ok()?;
    intact(&dir, &record, bin_rel).then(|| name.to_string())
}

/// Writes the `RECORDED` marker where it is not there yet.
fn mark_recorded(root: &Path) -> Result<(), String> {
    if fs::symlink_metadata(root.join(RECORDED)).is_ok() {
        return Ok(());
    }
    crate::secret::write_private_synced(&root.join(RECORDED), b"versions here are installed with completion records\n")
        .map_err(|e| format!("write {RECORDED}: {e}"))
}

/// Fetches `version` into a fresh staging directory, holds it to what the
/// fetch vouched for, puts it and its record on disk, and renames it into
/// `versions/`. Returns the name it got there. Whatever fails, the staged
/// directory goes.
fn install_in(
    root: &Path,
    bin_rel: &str,
    version: &str,
    fetch: &mut dyn FnMut(&Path) -> Result<Package, String>,
) -> Result<String, String> {
    let staging = clean_staging(root)?;
    let staged = staging.join(format!("{version}-{}", crate::secret::random_hex(4)));
    let versions = root.join("versions");
    fs::create_dir(&staged).map_err(|e| format!("mkdir {}: {e}", staged.display()))?;
    let done = fetch(&staged)
        .and_then(|package| validate(&staged, version, bin_rel, package))
        .and_then(|record| persist(&staged, &record))
        .and_then(|()| promote(&staged, &versions, version));
    let done = done.and_then(|name| {
        // The rename made durable before `current` names its result. Where
        // the platform cannot (Windows, `secret::sync_dir`), a rename a power
        // cut undoes leaves the staged copy, which the next pass sweeps and
        // fetches again; one that stays finds its files already on disk.
        crate::secret::sync_dir(&versions)
            .and_then(|()| crate::secret::sync_dir(&staging))
            .map_err(|e| format!("sync {}: {e}", versions.display()))?;
        Ok(name)
    });
    if done.is_err() && fs::symlink_metadata(&staged).is_ok() {
        if let Err(e) = remove_tree(&staged) {
            crate::log::info("cli", &format!("leaving {} for the next pass: {e}", staged.display()));
        }
    }
    done
}

/// Puts the validated package on disk before anything can name it: every
/// file flushed, then the record written and flushed, then (on Unix) every
/// directory below, so a rename that survives a power cut finds whole
/// files, never names and sizes over contents still in the cache.
fn persist(staged: &Path, record: &Record) -> Result<(), String> {
    for f in &record.files {
        let path = staged.join(&f.path);
        sync_file(&path).map_err(|e| format!("sync {}: {e}", path.display()))?;
    }
    let json = serde_json::to_vec_pretty(record).map_err(|e| e.to_string())?;
    // A plain new file in a directory made for this install: nothing reads it
    // before the rename, so no temp name is needed to keep it whole.
    let path = staged.join(RECORD);
    let mut file = crate::secret::create_private(&path).map_err(|e| format!("write {RECORD}: {e}"))?;
    file.write_all(&json).and_then(|()| file.sync_all()).map_err(|e| format!("write {RECORD}: {e}"))?;
    drop(file);
    sync_dirs(staged).map_err(|e| format!("sync {}: {e}", staged.display()))
}

/// Flushes a file's data to disk. Windows flushes only through a handle open
/// for writing (`FlushFileBuffers`), Unix through any; a file unpacked
/// read-only cannot be opened for writing there.
fn sync_file(path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    let file = fs::OpenOptions::new().write(true).open(path)?;
    #[cfg(not(windows))]
    let file = fs::File::open(path)?;
    file.sync_all()
}

/// `secret::sync_dir` on `dir` and every directory below it, links not
/// followed; nothing to do where a directory cannot be flushed.
fn sync_dirs(dir: &Path) -> std::io::Result<()> {
    if !cfg!(unix) {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let meta = fs::symlink_metadata(&path)?;
        if meta.is_dir() && !is_link(&meta) {
            sync_dirs(&path)?;
        }
    }
    crate::secret::sync_dir(dir)
}

/// Holds the staged directory to the package: every file it names is there,
/// a regular file at its size, and at its sha256 where the release publishes
/// one. The executable must be among them.
pub(crate) fn validate(dir: &Path, version: &str, bin_rel: &str, package: Package) -> Result<Record, String> {
    if !package.files.iter().any(|f| f.rel == bin_rel) {
        return Err(format!("{version}: the package has no {bin_rel}"));
    }
    let mut files = Vec::with_capacity(package.files.len());
    for f in package.files {
        if !plain_rel(&f.rel) || f.rel == RECORD {
            return Err(format!("{version}: unexpected file {:?} in the package", f.rel));
        }
        let path = dir.join(&f.rel);
        let meta = fs::symlink_metadata(&path).map_err(|e| format!("{version}: {} is missing: {e}", f.rel))?;
        if is_link(&meta) || !meta.is_file() {
            return Err(format!("{version}: {} is not a file", f.rel));
        }
        if meta.len() != f.size {
            return Err(format!("{version}: {} is {} bytes, the package says {}", f.rel, meta.len(), f.size));
        }
        // Every file's sha256 goes into the record, the release's or, where
        // it publishes none (Codex's files inside its tarball), the one it
        // has now, held to the release's sizes and its tarball's checksum:
        // what going back to the version later checks it against again
        // (`reverify`).
        let got = sha256_of(&path)?;
        if let Some(want) = &f.sha256 {
            if !got.eq_ignore_ascii_case(want) {
                return Err(format!("{version}: checksum mismatch for {}: expected {want}, got {got}", f.rel));
            }
        }
        files.push(RecordFile { path: f.rel, size: f.size, sha256: Some(got) });
    }
    Ok(Record { version: version.into(), verified: package.verified, files })
}

fn sha256_of(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).map_err(|e| format!("read {}: {e}", path.display()))?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// Renames the staged directory into `versions/` under a name nothing has:
/// `<ver>`, or `<ver>~<n>` while an older directory of that version is still
/// there — one a running session may hold, which is left exactly as it is.
/// One rename on one volume, so the directory appears whole or not at all.
fn promote(staged: &Path, versions: &Path, version: &str) -> Result<String, String> {
    fs::create_dir_all(versions).map_err(|e| format!("mkdir {}: {e}", versions.display()))?;
    for n in 0..1000u32 {
        let name = if n == 0 { version.to_string() } else { format!("{version}~{n}") };
        let target = versions.join(&name);
        if fs::symlink_metadata(&target).is_ok() {
            continue;
        }
        match fs::rename(staged, &target) {
            Ok(()) => return Ok(name),
            // Taken meanwhile: Windows refuses a directory rename onto
            // anything, Unix onto a directory with anything in it.
            Err(_) if fs::symlink_metadata(&target).is_ok() => continue,
            Err(e) => return Err(format!("move {} into {}: {e}", staged.display(), target.display())),
        }
    }
    Err(format!("no free directory name for {version} in {}", versions.display()))
}

/// `<root>/staging`, a real directory emptied of what earlier passes left
/// there (a crash mid-download, a staged copy that would not go). Links and
/// junctions are removed as themselves, never gone into: a planted one must
/// not get Luna deleting outside its CLI tree.
fn clean_staging(root: &Path) -> Result<PathBuf, String> {
    let staging = root.join("staging");
    match fs::symlink_metadata(&staging) {
        Ok(meta) if is_link(&meta) || !meta.is_dir() => {
            remove_tree(&staging).map_err(|e| format!("remove {}: {e}", staging.display()))?;
        }
        Ok(_) => {
            for entry in fs::read_dir(&staging).map_err(|e| format!("read {}: {e}", staging.display()))?.flatten() {
                if let Err(e) = remove_tree(&entry.path()) {
                    crate::log::info("cli", &format!("leaving {}: {e}", entry.path().display()));
                }
            }
        }
        Err(_) => {}
    }
    fs::create_dir_all(&staging).map_err(|e| format!("mkdir {}: {e}", staging.display()))?;
    Ok(staging)
}

/// A link or junction, whatever std calls it: a junction is a reparse point
/// too.
fn is_link(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    meta.file_type().is_symlink()
}

/// No link or junction on the way from `root` down to `path`, `path`
/// included, so a delete there stays in Luna's tree. `root` is the boundary:
/// the data dir's own ancestry is paths.rs's to check. A component that is
/// not there ends the walk: nothing below it can be reached.
fn link_free_below(root: &Path, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(root) else { return false };
    let mut at = root.to_path_buf();
    for part in rel.components() {
        at.push(part);
        match fs::symlink_metadata(&at) {
            Ok(meta) if is_link(&meta) => return false,
            Ok(_) => {}
            Err(e) => return e.kind() == std::io::ErrorKind::NotFound,
        }
    }
    true
}

/// Removes `path` and what is below it without following a link or junction
/// anywhere: one is removed as itself. That `path`'s own ancestors are no
/// links is the caller's to know (`link_free_below`, or a directory it has
/// just checked or made).
fn remove_tree(path: &Path) -> std::io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if is_link(&meta) {
        // A directory link or junction goes with remove_dir on Windows; a
        // file link, and any link on Unix, with remove_file.
        return fs::remove_dir(path).or_else(|_| fs::remove_file(path));
    }
    if !meta.is_dir() {
        return fs::remove_file(path);
    }
    for entry in fs::read_dir(path)? {
        remove_tree(&entry?.path())?;
    }
    fs::remove_dir(path)
}

/// Drops every version directory but the current one, the previous one and
/// those a process of Luna's runs from (`in_use`); such a one goes on a later
/// pass, once nothing runs from it. On Windows a running exe would not go
/// anyway; on Unix the hold is all that keeps a running session's directory,
/// its helpers with it.
fn prune_in(root: &Path, bin_rel: &str) {
    let keep: Vec<String> = current_in(root, bin_rel).map(|c| c.dir).into_iter().chain(previous_in(root, bin_rel)).collect();
    let versions = root.join("versions");
    if !link_free_below(root, &versions) {
        crate::log::warn("cli", &format!("{} is a link or junction; not sweeping through it", versions.display()));
        return;
    }
    let Ok(entries) = fs::read_dir(&versions) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if keep.contains(&name) {
            continue;
        }
        // Held until the directory is gone: nothing starts from it meanwhile.
        let Some(sweeping) = in_use::try_sweep(root, &name) else {
            crate::log::info("cli", &format!("keeping {name}: a process of Luna's runs from it"));
            continue;
        };
        let path = entry.path();
        sweep(root, bin_rel, &path);
        if fs::symlink_metadata(&path).is_err() {
            sweeping.done();
        }
    }
    // A hold's file whose directory went another way (a crash mid-sweep).
    for entry in fs::read_dir(root.join("in-use")).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if plain_name(&name) && fs::symlink_metadata(versions.join(&name)).is_err() {
            if let Some(sweeping) = in_use::try_sweep(root, &name) {
                sweeping.done();
            }
        }
    }
}

/// Removes one version directory, its exe first.
fn sweep(root: &Path, bin_rel: &str, path: &Path) {
    // A link anywhere on the way to the exe (the version directory, its
    // `bin/`) is none of Luna's making: the directory goes by `remove_tree`,
    // which takes a link as itself, and nothing is deleted through one.
    if !link_free_below(root, &path.join(bin_rel)) {
        if let Err(e) = remove_tree(path) {
            crate::log::info("cli", &format!("keeping {}: {e}", path.display()));
        }
        return;
    }
    // The exe first: Windows refuses it while something runs it, and the
    // directory then stays whole. Removed from the top, it would lose the
    // files beside the exe (Codex's codex-path/, codex-resources/) under a
    // process started outside Luna's count before failing on the exe itself.
    match fs::remove_file(path.join(bin_rel)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            crate::log::info("cli", &format!("keeping {}: {e}", path.display()));
            return;
        }
        _ => {}
    }
    if let Err(e) = remove_tree(path) {
        crate::log::info("cli", &format!("keeping {}: {e}", path.display()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    const BIN: &str = "bin/codex.exe";
    /// A package as Codex ships one: the exe and a helper beside it.
    const FILES: [(&str, &[u8]); 2] = [(BIN, b"the codex exe"), ("codex-resources/codex-command-runner.exe", b"the sandbox helper")];

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("luna-cli-{tag}-{}", std::process::id()));
        let _ = remove_tree(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_files(dir: &Path, files: &[(&str, &[u8])]) {
        for (rel, bytes) in files {
            let path = dir.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
    }

    /// A fetch that writes the whole package and vouches for it by size.
    fn whole(dir: &Path) -> Result<Package, String> {
        write_files(dir, &FILES);
        let files = FILES.iter().map(|(rel, b)| PackageFile { rel: rel.to_string(), size: b.len() as u64, sha256: None }).collect();
        Ok(Package { files, verified: "test".into() })
    }

    /// A probe every version passes.
    fn passes(_: &Path) -> Result<(), String> {
        Ok(())
    }

    /// What the channel's pointer names, with no minimum of its own.
    fn pointer(version: &str) -> Target<'_> {
        Target { version, min_safe: "0", chosen: false, probe: &passes }
    }

    /// What this Luna's list names, `known` from it.
    fn chosen(version: &str) -> Target<'_> {
        Target { version, min_safe: "0", chosen: true, probe: &passes }
    }

    fn list(known: &[&str]) -> known_good::Entry {
        known_good::Entry { min_safe: "0".into(), known_good: known.iter().map(|v| v.to_string()).collect() }
    }

    /// `whole`, vouched for by sha256 as well, as Claude Code's manifest does.
    fn hashed(dir: &Path) -> Result<Package, String> {
        write_files(dir, &FILES);
        let files = FILES
            .iter()
            .map(|(rel, b)| PackageFile { rel: rel.to_string(), size: b.len() as u64, sha256: Some(format!("{:x}", Sha256::digest(b))) })
            .collect();
        Ok(Package { files, verified: "test".into() })
    }

    fn entries(dir: &Path) -> usize {
        fs::read_dir(dir).map(|d| d.count()).unwrap_or(0)
    }

    /// Every file below `dir` with its bytes, in order.
    fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for entry in fs::read_dir(&d).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    out.push((path.clone(), fs::read(&path).unwrap()));
                }
            }
        }
        out.sort();
        out
    }

    fn link_dir(link: &Path, target: &Path) {
        #[cfg(windows)]
        {
            let made = std::process::Command::new("cmd").args(["/d", "/c", "mklink", "/J"]).arg(link).arg(target).output().unwrap();
            assert!(made.status.success());
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).unwrap();
    }

    #[test]
    fn rejects_html_and_accepts_versions() {
        assert!(sane_version("2.1.252", "x").is_ok());
        assert!(sane_version("0.154.0\n", "x").is_ok());
        assert!(sane_version("<html>", "x").is_err());
        assert!(sane_version("", "x").is_err());
    }

    /// The defect: the exe on disk was taken for a finished install, so an
    /// unpack cut short after its entry was activated without the helpers.
    #[test]
    fn an_unpack_cut_short_after_the_exe_is_not_activated_and_is_fetched_again() {
        let root = scratch("cut");
        // What a crash mid-unpack leaves: a staged copy with only the exe,
        // and (from before staging) a version directory with only the exe.
        write_files(&root.join("staging").join("1.0.0-dead"), &FILES[..1]);
        write_files(&root.join("versions").join("1.0.0"), &FILES[..1]);
        assert!(installed_in(&root, BIN, "1.0.0").is_none());

        // Cut short again: the exe written, then the unpack fails.
        let cut = update_in(&root, BIN, pointer("1.0.0"), &mut |dir: &Path| {
            write_files(dir, &FILES[..1]);
            Err("unpack: unexpected end of file".into())
        });
        assert!(cut.is_err());
        assert!(!root.join("current").exists(), "nothing was activated");
        assert_eq!(entries(&root.join("staging")), 0, "the staged copies are gone");

        let fetched = Cell::new(0);
        let (dir, flipped) = update_in(&root, BIN, pointer("1.0.0"), &mut |dir: &Path| {
            fetched.set(fetched.get() + 1);
            whole(dir)
        })
        .unwrap();
        assert_eq!(fetched.get(), 1, "fetched again, not taken from the half-unpacked directory");
        assert!(flipped);
        let current = current_in(&root, BIN).unwrap();
        assert!(current.recorded && current.version == "1.0.0" && current.dir == dir);
        assert!(root.join("versions").join(&dir).join(FILES[1].0).is_file());
        let _ = remove_tree(&root);
    }

    #[test]
    fn a_truncated_or_altered_exe_is_rejected() {
        let root = scratch("short");
        let full: &[u8] = b"the claude exe, every byte of it";
        let package = || Package {
            files: vec![PackageFile {
                rel: "claude.exe".into(),
                size: full.len() as u64,
                sha256: Some(format!("{:x}", Sha256::digest(full))),
            }],
            verified: "test".into(),
        };
        let short = install_in(&root, "claude.exe", "2.0.0", &mut |dir: &Path| {
            fs::write(dir.join("claude.exe"), &full[..10]).unwrap();
            Ok(package())
        });
        assert!(short.unwrap_err().contains("bytes"));
        let altered = install_in(&root, "claude.exe", "2.0.0", &mut |dir: &Path| {
            fs::write(dir.join("claude.exe"), b"the claude exe, every BYTE of it").unwrap();
            Ok(package())
        });
        assert!(altered.unwrap_err().contains("checksum mismatch"));
        assert!(!root.join("versions").exists(), "nothing was promoted");
        assert_eq!(entries(&root.join("staging")), 0);
        let _ = remove_tree(&root);
    }

    #[test]
    fn a_whole_package_is_activated_with_its_record() {
        let root = scratch("whole");
        let (dir, flipped) = update_in(&root, BIN, pointer("1.2.0"), &mut whole).unwrap();
        assert_eq!((dir.as_str(), flipped), ("1.2.0", true));
        assert_eq!(fs::read_to_string(root.join("current")).unwrap(), "1.2.0");
        let record = read_record(&root.join("versions").join("1.2.0")).unwrap();
        assert_eq!(record.version, "1.2.0");
        assert_eq!(record.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), [FILES[0].0, FILES[1].0]);
        assert!(current_in(&root, BIN).is_some_and(|c| c.recorded && c.version == "1.2.0"));
        assert_eq!(entries(&root.join("staging")), 0);

        // The next pass finds it installed and fetches nothing.
        let (_, flipped) = update_in(&root, BIN, pointer("1.2.0"), &mut |_: &Path| panic!("fetched again")).unwrap();
        assert!(!flipped);
        // A file the record names, cut short afterwards: no longer installed.
        fs::write(root.join("versions").join("1.2.0").join(FILES[1].0), b"cut").unwrap();
        assert!(current_in(&root, BIN).is_none());
        let _ = remove_tree(&root);
    }

    #[test]
    fn promotion_leaves_a_running_version_alone() {
        let root = scratch("running");
        // 3.0.0 as a session runs it, installed before records: the pass
        // fetches 3.0.0 again and must put it beside this one.
        let running = root.join("versions").join("3.0.0");
        write_files(&running, &[(BIN, b"old exe"), ("codex-path/rg.exe", b"old rg")]);
        fs::write(root.join("current"), "3.0.0").unwrap();
        let before = snapshot(&running);
        let staged = root.join("staging").join("3.0.0-new");
        whole(&staged).unwrap();

        let name = promote(&staged, &root.join("versions"), "3.0.0").unwrap();
        assert_eq!(name, "3.0.0~1");
        assert_eq!(snapshot(&running), before, "nothing in the running version changed");
        assert!(!staged.exists());
        assert_eq!(fs::read(root.join("versions").join(&name).join(BIN)).unwrap(), FILES[0].1);
        let _ = remove_tree(&root);
    }

    /// A record lost after install (a power cut before it reached the disk,
    /// on an older layout) must not let the bare exe pass: only a tree this
    /// code never installed into runs a directory without one.
    #[test]
    fn a_version_installed_here_is_not_run_without_its_record() {
        let root = scratch("unrecorded");
        update_in(&root, BIN, pointer("1.2.0"), &mut whole).unwrap();
        assert!(root.join(RECORDED).is_file(), "the tree is marked before `current` names anything");
        // A marker lost after the fact comes back on the next pass, even one
        // with nothing to fetch.
        fs::remove_file(root.join(RECORDED)).unwrap();
        let (_, flipped) = update_in(&root, BIN, pointer("1.2.0"), &mut |_: &Path| panic!("fetched again")).unwrap();
        assert!(!flipped && root.join(RECORDED).is_file());
        fs::remove_file(root.join("versions").join("1.2.0").join(RECORD)).unwrap();
        assert!(current_in(&root, BIN).is_none());
        let _ = remove_tree(&root);

        // From before records: no marker, and `current` names a directory
        // with its exe. Still run, but not taken for installed.
        let legacy = scratch("legacy");
        write_files(&legacy.join("versions").join("0.9.0"), &FILES[..1]);
        fs::write(legacy.join("current"), "0.9.0").unwrap();
        assert!(current_in(&legacy, BIN).is_some_and(|c| !c.recorded && c.version == "0.9.0"));
        assert!(installed_in(&legacy, BIN, "0.9.0").is_none());
        let _ = remove_tree(&legacy);
    }

    /// prune removes an old version's exe first; with `bin/` a junction out,
    /// that remove would have gone through it.
    #[test]
    fn prune_deletes_nothing_through_a_junction_below_a_version() {
        let root = scratch("prune-link");
        let outside = scratch("prune-link-outside");
        write_files(&outside, &[("codex.exe", b"not Luna's")]);
        update_in(&root, BIN, pointer("2.0.0"), &mut whole).unwrap();
        let old = root.join("versions").join("1.0.0");
        fs::create_dir_all(&old).unwrap();
        link_dir(&old.join("bin"), &outside);

        prune_in(&root, BIN);
        assert!(outside.join("codex.exe").is_file(), "the exe outside the tree survived");
        assert!(!old.exists(), "the old version went, its junction as itself");
        assert!(current_in(&root, BIN).is_some_and(|c| c.version == "2.0.0"));
        let _ = remove_tree(&root);
        let _ = remove_tree(&outside);
    }

    fn dirs(root: &Path) -> Vec<String> {
        let mut out: Vec<String> =
            fs::read_dir(root.join("versions")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        out.sort();
        out
    }

    /// The defect: every pass kept `current` alone, so nothing was left to go
    /// back to, and on Unix a running session's directory went from under it.
    #[test]
    fn the_previous_version_and_those_in_use_survive_the_sweep() {
        let root = scratch("keep");
        update_in(&root, BIN, pointer("1.0.0"), &mut whole).unwrap();
        update_in(&root, BIN, pointer("2.0.0"), &mut whole).unwrap();
        assert_eq!(dirs(&root), ["1.0.0", "2.0.0"]);
        assert_eq!(previous_in(&root, BIN).as_deref(), Some("1.0.0"));

        // A session starts on 2.0.0 and runs on through two more updates.
        let (exe, version, held) = launch_in(&root, BIN, "0").unwrap().unwrap();
        assert_eq!((exe, version.as_str()), (root.join("versions").join("2.0.0").join(BIN), "2.0.0"));
        update_in(&root, BIN, pointer("3.0.0"), &mut whole).unwrap();
        update_in(&root, BIN, pointer("4.0.0"), &mut whole).unwrap();
        assert_eq!(dirs(&root), ["2.0.0", "3.0.0", "4.0.0"], "1.0.0 went; 2.0.0 runs, 3.0.0 is the previous one");
        assert!(root.join("versions").join("2.0.0").join(FILES[1].0).is_file(), "its helpers too");
        let rows = versions_in(&root, BIN, &list(&[])).versions;
        assert_eq!(
            rows.iter().map(|r| (r.version.as_str(), r.current, r.previous, r.in_use)).collect::<Vec<_>>(),
            [("4.0.0", true, false, false), ("3.0.0", false, true, false), ("2.0.0", false, false, true)]
        );

        // Once it has exited, the next pass takes it.
        drop(held);
        prune_in(&root, BIN);
        assert_eq!(dirs(&root), ["3.0.0", "4.0.0"]);
        assert_eq!(entries(&root.join(in_use::DIR)), 0, "the lock files went with their directories");
        let _ = remove_tree(&root);
    }

    /// A start the sweep could not see would let it take the folder from under
    /// the process: one whose hold cannot be had is refused, never started
    /// unheld; with nothing current it is PATH's, which no sweep touches.
    #[test]
    fn a_managed_copy_that_cannot_be_held_is_not_started() {
        let root = scratch("unheld");
        assert!(launch_in(&root, BIN, "0").unwrap().is_none(), "nothing current: the CLI on PATH");
        update_in(&root, BIN, pointer("1.0.0"), &mut whole).unwrap();
        // The locks' folder taken by a file: no lock can be had there.
        remove_tree(&root.join(in_use::DIR)).ok();
        fs::write(root.join(in_use::DIR), b"not a folder").unwrap();
        let refused = launch_in(&root, BIN, "0").err().expect("refused");
        assert!(refused.contains("not started: cannot hold 1.0.0"), "{refused}");
        // Held again once the lock can be had.
        fs::remove_file(root.join(in_use::DIR)).unwrap();
        assert!(launch_in(&root, BIN, "0").unwrap().is_some());
        let _ = remove_tree(&root);
    }

    /// A Luna release that raised the minimum: the current version below it
    /// starts nothing, held by hand or not, and the CLI on PATH does not stand
    /// in for it.
    #[test]
    fn a_current_version_below_the_minimum_starts_nothing() {
        let root = scratch("unsafe");
        update_in(&root, BIN, pointer("1.0.0"), &mut hashed).unwrap();
        use_version_in(&root, BIN, "0", "1.0.0").unwrap();
        choice::pin_in(&root, "1.0.0").unwrap();
        let refused = launch_in(&root, BIN, "1.5.0").err().expect("refused");
        assert!(refused.contains("1.0.0 is older than 1.5.0"), "{refused}");
        assert!(launch_in(&root, BIN, "1.0.0").unwrap().is_some(), "at the minimum it runs");
        let _ = remove_tree(&root);
    }

    #[test]
    fn use_goes_back_to_an_installed_version_and_only_to_one() {
        let root = scratch("use");
        update_in(&root, BIN, pointer("1.0.0"), &mut hashed).unwrap();
        update_in(&root, BIN, pointer("2.0.0"), &mut hashed).unwrap();
        assert_eq!(use_version_in(&root, BIN, "0", "1.0.0").unwrap(), "1.0.0");
        assert!(current_in(&root, BIN).is_some_and(|c| c.dir == "1.0.0"));
        assert_eq!(previous_in(&root, BIN).as_deref(), Some("2.0.0"), "and the way forward again is kept");
        // Not installed, not a plain name, or no longer whole: refused, and
        // `current` stays.
        for dir in ["3.0.0", "..", "../1.0.0"] {
            assert!(use_version_in(&root, BIN, "0", dir).is_err(), "{dir}");
        }
        fs::write(root.join("versions").join("2.0.0").join(FILES[1].0), b"cut").unwrap();
        assert!(use_version_in(&root, BIN, "0", "2.0.0").unwrap_err().contains("not an installed version"));
        assert!(current_in(&root, BIN).is_some_and(|c| c.dir == "1.0.0"));
        let _ = remove_tree(&root);
    }

    #[test]
    fn versions_order_by_their_numbers() {
        use std::cmp::Ordering::*;
        assert_eq!(version_cmp("2.1.100", "2.1.99"), Greater);
        assert_eq!(version_cmp("2.1.319", "2.1.319"), Equal);
        assert_eq!(version_cmp("2.1", "2.1.0"), Equal);
        assert_eq!(version_cmp("2.0.999", "2.1.0"), Less);
        assert_eq!(version_cmp("10.0.0", "9.9.9"), Greater);
    }

    /// The defect: the floor rose as each manifest was taken, before its
    /// version ever ran, so a download refused afterwards shut out the
    /// version before it.
    #[test]
    fn the_floor_rises_when_a_version_becomes_current_not_when_it_is_fetched() {
        let root = scratch("floor");
        update_in(&root, BIN, pointer("1.0.0"), &mut hashed).unwrap();
        assert_eq!(floor_in(&root, "0"), "1.0.0");
        // 2.0.0 fetched, then refused: the floor stays.
        let refused = update_in(&root, BIN, pointer("2.0.0"), &mut |dir: &Path| {
            write_files(dir, &FILES[..1]);
            whole(dir).map(|mut p| {
                p.files[0].size += 1;
                p
            })
        });
        assert!(refused.is_err());
        assert_eq!(floor_in(&root, "0"), "1.0.0");
        update_in(&root, BIN, pointer("2.0.0"), &mut hashed).unwrap();
        assert_eq!(floor_in(&root, "0"), "2.0.0");

        // The pointer may not name 1.0.0 again, installed as it is...
        let refused = update_in(&root, BIN, pointer("1.0.0"), &mut |_: &Path| panic!("fetched")).unwrap_err();
        assert!(refused.contains("refusing 1.0.0: older than 2.0.0"), "{refused}");
        // ...nor anything else below the floor, fetched or not.
        assert!(update_in(&root, BIN, pointer("1.5.0"), &mut |_: &Path| panic!("fetched")).is_err());
        assert!(current_in(&root, BIN).is_some_and(|c| c.version == "2.0.0"));
        let _ = remove_tree(&root);
    }

    #[test]
    fn going_back_below_the_floor_rechecks_every_file_and_stops_at_the_minimum() {
        let root = scratch("below");
        update_in(&root, BIN, pointer("1.0.0"), &mut hashed).unwrap();
        update_in(&root, BIN, pointer("2.0.0"), &mut hashed).unwrap();
        // Back by hand: allowed, once every file matches its sha256 again;
        // the floor stays where it was.
        assert_eq!(use_version_in(&root, BIN, "0", "1.0.0").unwrap(), "1.0.0");
        assert_eq!(floor_in(&root, "0"), "2.0.0");
        // A file altered at its size since: refused.
        use_version_in(&root, BIN, "0", "2.0.0").unwrap();
        fs::write(root.join("versions").join("1.0.0").join(BIN), b"THE CODEX EXE").unwrap();
        assert!(use_version_in(&root, BIN, "0", "1.0.0").unwrap_err().contains("no longer matches its sha256"));
        // Below the minimum: refused whatever its files.
        update_in(&root, BIN, pointer("3.0.0"), &mut hashed).unwrap();
        assert!(use_version_in(&root, BIN, "2.5.0", "2.0.0").unwrap_err().contains("older than 2.5.0"));
        let _ = remove_tree(&root);
    }

    /// Codex's release vouches for its tarball, not the files in it: each
    /// file's sha256 as unpacked goes into the record, for "Use" to hold the
    /// version to again. Before, its records kept sizes alone.
    #[test]
    fn every_file_is_recorded_with_its_sha256_and_held_to_it_on_the_way_back() {
        let root = scratch("hashes");
        update_in(&root, BIN, pointer("1.0.0"), &mut whole).unwrap();
        let record = read_record(&root.join("versions").join("1.0.0")).unwrap();
        for (f, (_, bytes)) in record.files.iter().zip(FILES) {
            assert_eq!(f.sha256.as_deref(), Some(format!("{:x}", Sha256::digest(bytes)).as_str()), "{}", f.path);
        }
        update_in(&root, BIN, pointer("2.0.0"), &mut whole).unwrap();
        // A helper altered at its size: sizes pass, the hash does not.
        fs::write(root.join("versions").join("1.0.0").join(FILES[1].0), b"THE SANDBOX HELPER").unwrap();
        assert!(current_in(&root, BIN).is_some(), "intact by size");
        assert!(use_version_in(&root, BIN, "0", "1.0.0").unwrap_err().contains("no longer matches its sha256"));

        // A record from before, with sizes alone: not gone back to below the
        // floor, where nothing else vouches for it...
        let legacy = scratch("hashes-legacy");
        update_in(&legacy, BIN, pointer("1.0.0"), &mut whole).unwrap();
        update_in(&legacy, BIN, pointer("2.0.0"), &mut whole).unwrap();
        for dir in ["1.0.0", "2.0.0"] {
            let path = legacy.join("versions").join(dir).join(RECORD);
            let mut record: Record = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            record.files.iter_mut().for_each(|f| f.sha256 = None);
            fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        }
        assert!(use_version_in(&legacy, BIN, "0", "1.0.0").unwrap_err().contains("keeps no sha256"));
        assert!(current_in(&legacy, BIN).is_some_and(|c| c.version == "2.0.0"));
        // ...while one at the floor is held to what it has.
        assert_eq!(use_version_in(&legacy, BIN, "0", "2.0.0").unwrap(), "2.0.0");
        let _ = remove_tree(&root);
        let _ = remove_tree(&legacy);
    }

    /// The known-good channel after `latest` had moved on: back to the
    /// listed version while it is installed, and never a download below the
    /// floor; Settings sees the list, the marks and the hold.
    #[test]
    fn the_known_good_channel_goes_back_only_to_what_is_installed() {
        let root = scratch("known");
        update_in(&root, BIN, chosen("1.0.0"), &mut hashed).unwrap();
        update_in(&root, BIN, pointer("2.0.0"), &mut hashed).unwrap();
        assert!(held_below_floor(&root, BIN, chosen("1.0.0")).is_none(), "1.0.0 is still installed");
        let (dir, flipped) = update_in(&root, BIN, chosen("1.0.0"), &mut |_: &Path| panic!("fetched")).unwrap();
        assert_eq!((dir.as_str(), flipped), ("1.0.0", true));
        assert_eq!(floor_in(&root, "0"), "2.0.0", "the floor stays");

        // Listed but never installed, below the floor: held, not fetched.
        let why = held_below_floor(&root, BIN, chosen("1.5.0")).unwrap();
        assert!(why.contains("older than 2.0.0"), "{why}");
        assert!(update_in(&root, BIN, chosen("1.5.0"), &mut |_: &Path| panic!("fetched")).is_err());
        // Above it, a listed version is fetched like any.
        update_in(&root, BIN, chosen("3.0.0"), &mut hashed).unwrap();

        choice::mark_bad_in(&root, "2.0.0", "hangs").unwrap();
        choice::pin_in(&root, "3.0.0").unwrap();
        let v = versions_in(&root, BIN, &list(&["1.0.0", "3.0.0"]));
        let rows: Vec<_> = v.versions.iter().map(|r| (r.version.as_str(), r.known_good, r.bad.as_deref())).collect();
        assert_eq!(rows, [("3.0.0", true, None), ("1.0.0", true, None)], "2.0.0, neither current nor previous, went");
        assert_eq!((v.floor.as_str(), v.pinned.as_deref(), v.bad.len()), ("3.0.0", Some("3.0.0"), 1));
        let _ = remove_tree(&root);
    }

    /// A version that fails the offline probe never becomes current, and
    /// the sweep takes it; the one before runs on.
    #[test]
    fn a_version_that_fails_the_probe_is_not_made_current() {
        let root = scratch("probe");
        update_in(&root, BIN, pointer("1.0.0"), &mut hashed).unwrap();
        let probed = std::cell::RefCell::new(Vec::new());
        let refuse = |exe: &Path| {
            probed.borrow_mut().push(exe.to_path_buf());
            Err("`exec` no longer lists --json".to_string())
        };
        let target = Target { probe: &refuse, ..pointer("2.0.0") };
        let e = update_in(&root, BIN, target, &mut hashed).unwrap_err();
        assert!(e.contains("2.0.0 failed the offline probe: `exec` no longer lists --json"), "{e}");
        assert_eq!(*probed.borrow(), [root.join("versions").join("2.0.0").join(BIN)], "the exe where it would run from");
        assert!(current_in(&root, BIN).is_some_and(|c| c.version == "1.0.0"));
        assert_eq!(dirs(&root), ["1.0.0"], "the refused version went");
        assert_eq!(floor_in(&root, "0"), "1.0.0");
        let _ = remove_tree(&root);
    }

    /// A new version gets a canary of its own; Settings shows it while that
    /// version is current, and a rollback by hand watches the one gone back to.
    #[test]
    fn each_flip_to_another_version_starts_its_canary() {
        let root = scratch("canary");
        update_in(&root, BIN, pointer("1.0.0"), &mut hashed).unwrap();
        update_in(&root, BIN, pointer("2.0.0"), &mut hashed).unwrap();
        canary::session_started(Provider::Codex, &root, "2.0.0", "canary-flip-a");
        canary::session_started(Provider::Codex, &root, "2.0.0", "canary-flip-b");
        session_ended("canary-flip-a", true);
        session_ended("canary-flip-b", true);
        let shown = versions_in(&root, BIN, &list(&[])).canary.unwrap();
        assert_eq!((shown.version.as_str(), shown.warned), ("2.0.0", true));
        // The same version again (nothing flipped): the warning stands.
        update_in(&root, BIN, pointer("2.0.0"), &mut |_: &Path| panic!("fetched")).unwrap();
        assert!(versions_in(&root, BIN, &list(&[])).canary.unwrap().warned);
        // Rolled back: the old version is watched, the warning is the past.
        use_version_in(&root, BIN, "0", "1.0.0").unwrap();
        let shown = versions_in(&root, BIN, &list(&[])).canary.unwrap();
        assert_eq!((shown.version.as_str(), shown.watched, shown.warned), ("1.0.0", 0, false));
        let _ = remove_tree(&root);
    }

    /// F11: the pass holds off in an elevated window rather than probe, and
    /// says why.
    #[test]
    fn an_elevated_window_holds_the_update_pass() {
        // As the OS has it (the Linux check runs as root), then refused here.
        assert_eq!(held_for_elevation().is_some(), crate::elevation::refusal().is_some());
        crate::elevation::refuse_here(Some("runs as administrator"));
        let (held, why) = (held_for_elevation(), crate::elevation::refusal().unwrap());
        crate::elevation::refuse_here(None);
        assert!(held.is_some_and(|w| w.contains("as administrator") && w.contains(why)));
    }

    /// The switch writes the floor (and the canary) before `current`: a
    /// write of `current` that fails leaves the old version named under the
    /// new floor, never the new one under the old floor.
    #[test]
    fn the_floor_is_on_disk_before_current_names_the_version() {
        let root = scratch("journal");
        update_in(&root, BIN, pointer("1.0.0"), &mut hashed).unwrap();
        update_in(&root, BIN, pointer("2.0.0"), &mut hashed).unwrap();
        use_version_in(&root, BIN, "0", "1.0.0").unwrap();
        // `current` cannot be replaced: a folder with something in it stands
        // at its name, so the rename onto it fails.
        fs::remove_file(root.join("current")).unwrap();
        fs::create_dir(root.join("current")).unwrap();
        fs::write(root.join("current").join("keep"), b"x").unwrap();
        let refused = flip_in(&root, BIN, "2.0.0");
        assert!(refused.unwrap_err().contains("write current"));
        assert_eq!(floor_in(&root, "0"), "2.0.0", "the floor went first");
        assert_eq!(canary::read_in(&root).unwrap().version, "2.0.0", "and the canary");
        let _ = remove_tree(&root);
    }

    /// A tree a crash left with the new version current under the old floor
    /// (the order before): the next pass mends the floor before it takes
    /// anything, so the pointer cannot name a version below what runs.
    #[test]
    fn a_pass_mends_a_floor_left_below_the_current_version() {
        let root = scratch("reconcile");
        update_in(&root, BIN, pointer("1.0.0"), &mut hashed).unwrap();
        update_in(&root, BIN, pointer("2.0.0"), &mut hashed).unwrap();
        // As a crash between `current` and the floor left it.
        fs::write(root.join(FLOOR), "1.0.0").unwrap();
        let refused = update_in(&root, BIN, pointer("1.5.0"), &mut |_: &Path| panic!("fetched")).unwrap_err();
        assert!(refused.contains("refusing 1.5.0: older than 2.0.0"), "{refused}");
        fs::write(root.join(FLOOR), "1.0.0").unwrap();
        // The pass with nothing to do mends it too.
        update_in(&root, BIN, pointer("2.0.0"), &mut |_: &Path| panic!("fetched")).unwrap();
        assert_eq!(fs::read_to_string(root.join(FLOOR)).unwrap(), "2.0.0");
        let _ = remove_tree(&root);
    }

    /// The review's case: 3.0.0 current, the floor left at 1.0.0, 2.0.0
    /// installed with a record from before every file had a sha256. "Use
    /// 2.0.0" took it for above the floor, passed its files over and set the
    /// floor to 2.0.0, as if 3.0.0 had never run.
    #[test]
    fn use_mends_a_floor_left_below_the_current_version_first() {
        let root = scratch("use-floor");
        update_in(&root, BIN, pointer("1.0.0"), &mut hashed).unwrap();
        update_in(&root, BIN, pointer("2.0.0"), &mut hashed).unwrap();
        let path = root.join("versions").join("2.0.0").join(RECORD);
        let mut record: Record = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        record.files.iter_mut().for_each(|f| f.sha256 = None);
        fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        update_in(&root, BIN, pointer("3.0.0"), &mut hashed).unwrap();
        fs::write(root.join(FLOOR), "1.0.0").unwrap();

        let refused = use_version_in(&root, BIN, "0", "2.0.0").unwrap_err();
        assert!(refused.contains("below 3.0.0") && refused.contains("keeps no sha256"), "{refused}");
        assert_eq!(fs::read_to_string(root.join(FLOOR)).unwrap(), "3.0.0", "3.0.0 ran, and the floor says so");
        assert!(current_in(&root, BIN).is_some_and(|c| c.version == "3.0.0"));
        let _ = remove_tree(&root);
    }

    #[test]
    fn a_fresh_tree_takes_nothing_below_the_minimum() {
        let root = scratch("fresh-floor");
        assert_eq!(floor_in(&root, "2.1.319"), "2.1.319");
        assert!(at_or_above_floor_in(&root, "2.1.319", "2.1.300").is_err());
        at_or_above_floor_in(&root, "2.1.319", "2.2.0").unwrap();
        // Floor files below the minimum do not lower it; the old one still counts.
        fs::write(root.join(FLOOR), "1.0.0").unwrap();
        assert_eq!(floor_in(&root, "2.1.319"), "2.1.319");
        fs::write(root.join(LEGACY_FLOOR), "2.1.330").unwrap();
        assert_eq!(floor_in(&root, "2.1.319"), "2.1.330");
        let _ = remove_tree(&root);
    }

    #[test]
    fn stale_staging_is_swept_without_following_links() {
        let root = scratch("stale");
        let outside = scratch("stale-outside");
        fs::write(outside.join("keep.txt"), "not Luna's").unwrap();
        // A stale staged copy with a junction out inside it, and one that is
        // a junction itself.
        let dead = root.join("staging").join("1.0.0-dead");
        write_files(&dead, &FILES[..1]);
        link_dir(&dead.join("out"), &outside);
        link_dir(&root.join("staging").join("1.0.0-link"), &outside);
        clean_staging(&root).unwrap();
        assert_eq!(entries(&root.join("staging")), 0);
        assert!(outside.join("keep.txt").is_file(), "the junctions were not gone into");

        // The staging directory itself a junction out.
        remove_tree(&root.join("staging")).unwrap();
        link_dir(&root.join("staging"), &outside);
        let staging = clean_staging(&root).unwrap();
        assert!(fs::symlink_metadata(&staging).is_ok_and(|m| m.is_dir() && !is_link(&m)));
        assert!(outside.join("keep.txt").is_file());
        let _ = remove_tree(&root);
        let _ = remove_tree(&outside);
    }
}
