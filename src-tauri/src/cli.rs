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
//!   previous                    text file: the one `current` named before, kept to roll back to
//!   versions/<ver>/claude.exe   one directory per installed version
//!   versions/<ver>/luna-install.json   its completion record
//!   staging/<ver>-<random>/     a download in progress
//!   bad.json                    versions turned away (`choice`)
//!   canary.json, canary.lock    the watch over a new version's first sessions (`canary`)
//!   switch.lock                 held while `current`, `previous`, `bad.json` or `versions/` change (`switching`)
//! <data>/codex-cli/
//!   current
//!   versions/<ver>/bin/codex.exe (+ codex-path/, codex-resources/ from the package)
//!   versions/<ver>/luna-install.json
//!   staging/
//!   in-use/<dir>                held (shared) by each process started from <dir> (`in_use`)
//!   (and the rest as for Claude Code)
//! ```
//!
//! Versions are never overwritten in place: Windows will not let a running
//! exe be replaced, and a session that is mid-turn keeps the old one open
//! for hours. A new version is fetched into `staging/`, held to what its
//! release says the package is, given its completion record last, and
//! renamed into `versions/` under a name nothing has (`<ver>`, or `<ver>~<n>`
//! while an older `<ver>` directory is still there). `current` then flips to
//! it, `previous` to what it named before, and old directories are swept
//! whenever nothing runs from them any more; the previous one stays, for the
//! canary to roll back to when the new one's first sessions fail.
//! A directory counts as installed by its record alone, so a crash
//! mid-download or mid-unpack leaves nothing that passes for one.
//!
//! Which version: always the newest the release pointer names, never below
//! the floor compiled into this Luna (`floor`), never a step down from what
//! runs, never one turned away (`choice`, ADR-0030).
//!
//! This module is the machinery — layout, download, checksum, status, the
//! periodic check, the rollback. Where a version comes from is each
//! provider's `Source` (`claude::cli`, `codex::cli`).

mod canary;
mod choice;
mod in_use;
mod probe;

#[cfg(test)]
pub(crate) use canary::cause_outside;
pub use canary::{session_ended, session_exited, session_stopped, Canary};
pub use in_use::{session_version, InUse};
#[cfg(test)]
pub(crate) use probe::covers as probe_covers;

use crate::http;
use crate::provider::Provider;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
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
    pub latest: fn(&http::Agent) -> Result<String, String>,
    /// Puts `version` into `dir`, an empty staging directory, and says what
    /// it put there. What it leaves on failure goes with the directory.
    pub fetch: fn(&http::Agent, &str, &Path, &mut dyn FnMut(u64, Option<u64>)) -> Result<Package, String>,
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
    /// Where that binary lives; empty while there is no managed copy to start.
    pub path: String,
    /// The newest version the CLI's own release channel offers, once a check
    /// has come back.
    pub latest: Option<String>,
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
                    version: current_version(p),
                    path: binary(p).map(|b| b.to_string_lossy().into_owned()).unwrap_or_default(),
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
    current_or_why(root, bin_rel).ok()
}

/// Why `current` gives nothing to start.
#[derive(Debug, PartialEq)]
enum NoCurrent {
    /// No `current` yet: the first install has not finished.
    NotInstalled,
    /// A `current` that names nothing runnable, and why.
    Damaged(String),
}

fn current_or_why(root: &Path, bin_rel: &str) -> Result<Current, NoCurrent> {
    let name = match fs::read_to_string(root.join("current")) {
        Ok(name) => name,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(NoCurrent::NotInstalled),
        Err(e) => return Err(NoCurrent::Damaged(format!("its current will not read: {e}"))),
    };
    let name = name.trim();
    if !plain_name(name) {
        return Err(NoCurrent::Damaged(format!("its current names {name:?}, which is no version directory")));
    }
    let dir = root.join("versions").join(name);
    let damaged = |why: &str| NoCurrent::Damaged(format!("versions/{name}, which current names, {why}"));
    match read_record(&dir) {
        Ok(record) if intact(&dir, &record, bin_rel) => Ok(Current { dir: name.into(), version: record.version, recorded: true }),
        Ok(_) => Err(damaged("is missing files its record lists, or has them at another size")),
        // Without its record, a directory is run only in a tree this code has
        // never installed into: there it can only be from before records. In
        // one it has, it is an install whose record is gone, which proves
        // nothing about the files beside it.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && fs::symlink_metadata(root.join(RECORDED)).is_err() => {
            match dir.join(bin_rel).is_file() {
                true => Ok(Current { dir: name.into(), version: name.into(), recorded: false }),
                false => Err(damaged(&format!("has no {bin_rel}"))),
            }
        }
        Err(e) => Err(damaged(&format!("has no completion record that reads ({e})"))),
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

/// The managed copy `current` names, for Settings to show; None while there
/// is none to start.
pub fn binary(provider: Provider) -> Option<PathBuf> {
    let src = source(provider);
    let root = root(provider);
    current_in(&root, src.bin_rel).map(|c| root.join("versions").join(c.dir).join(src.bin_rel))
}

/// What a session should spawn: the managed copy `current` names, with a hold
/// on its version directory that the caller keeps until the process has
/// exited, so the sweep leaves that directory be (`in_use`). `session` is the
/// chat or helper it is for, whose version `luna_session_get` then shows.
///
/// Nothing else is started, the CLI on PATH included: only a managed copy has
/// been through the checks a version passes before it runs (its signature or
/// checksums, the probe, the floor), and a bare name
/// is found wherever the process's search looks first. Luna installs its copy
/// at its first start, so only that first download is waited for. The live
/// suite's test executables, which keep no managed copy, take the one its
/// script names (`live_cli`).
pub fn launch(provider: Provider, session: Option<&str>) -> Result<(PathBuf, Option<InUse>), String> {
    let src = source(provider);
    let root = root(provider);
    #[cfg(test)]
    if let Some(exe) = live_cli(provider).filter(|_| current_in(&root, src.bin_rel).is_none()) {
        return Ok((exe, None));
    }
    let (path, version, held) = launch_in(&root, src.bin_rel, floor(provider))?;
    if let Some(session) = session {
        in_use::note_session(session, provider, &version);
        canary::session_started(provider, &root, &version, session);
    }
    Ok((path, Some(held)))
}

/// The CLI scripts/live-tests.ps1 resolved for the live suite, by its full
/// path: `LUNA_LIVE_CLAUDE_CLI` or `LUNA_LIVE_CODEX_CLI`. Test executables
/// only; a shipped Luna starts nothing but its managed copy.
#[cfg(test)]
pub fn live_cli(provider: Provider) -> Option<PathBuf> {
    let var = match provider {
        Provider::Claude => "LUNA_LIVE_CLAUDE_CLI",
        Provider::Codex => "LUNA_LIVE_CODEX_CLI",
    };
    std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_absolute())
}

/// How often a launch tries for its hold: `current` moving under it, or the
/// lock file not to be had for a moment.
const HOLD_TRIES: u32 = 5;

/// The exe `current` names under `root`, its version, and a hold on its
/// directory taken before `current` is read a second time: a flip and a
/// sweep between the two reads would have taken the directory the first one
/// named. An Err where nothing is current, saying whether nothing has been
/// installed yet or what is there is damaged; an Err rather than a start
/// without the hold: the sweep could later take the folder from under the
/// process. An Err too for a current version below `floor`: a Luna release
/// that raised its floor runs nothing older, whatever `current` says, until
/// the next pass moves it.
fn launch_in(root: &Path, bin_rel: &str, floor: &str) -> Result<(PathBuf, String, InUse), String> {
    let mut why = String::new();
    for attempt in 0..HOLD_TRIES {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(20 * u64::from(attempt)));
        }
        let c = match current_or_why(root, bin_rel) {
            Ok(c) => c,
            Err(NoCurrent::NotInstalled) => {
                return Err(format!(
                    "not started: Luna has not installed this CLI in {} yet; it downloads it at its start, and Settings → CLIs \
                     shows how far it has got",
                    root.display()
                ))
            }
            Err(NoCurrent::Damaged(why)) => {
                return Err(format!(
                    "not started: Luna's copy of this CLI in {} is damaged: {why}; Settings → CLIs installs it again",
                    root.display()
                ))
            }
        };
        if version_cmp(&c.version, floor).is_lt() {
            return Err(format!(
                "not started: version {} is older than {floor}, the oldest this Luna lets run; Settings → CLIs installs a newer one",
                c.version
            ));
        }
        let Some(held) = in_use::hold(root, &c.dir) else {
            why = format!("cannot hold {} in {} (a lock file there cannot be had)", c.dir, root.join(in_use::DIR).display());
            continue;
        };
        if current_in(root, bin_rel).is_some_and(|again| again.dir == c.dir) {
            return Ok((root.join("versions").join(&c.dir).join(bin_rel), c.version, held));
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

/// The status as this process last published it, with the version read
/// afresh: the core's rollback moves `current` without this process's pass.
#[tauri::command]
pub fn cli_status(provider: Provider) -> CliStatus {
    let mut s = state().lock().ok().and_then(|m| m.get(&provider).cloned()).unwrap_or_default();
    s.version = current_version(provider);
    s.path = binary(provider).map(|b| b.to_string_lossy().into_owned()).unwrap_or_default();
    s
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
    /// Its directory under `versions/`.
    pub dir: String,
    pub version: String,
    pub current: bool,
    pub previous: bool,
    /// A process of Luna's runs from it now.
    pub in_use: bool,
    /// Older than the floor: left from before a Luna release raised it, and
    /// neither run nor rolled back to.
    pub below_floor: bool,
    /// Turned away, and why.
    pub bad: Option<String>,
}

/// A provider's installed versions and what each is to Luna.
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct CliVersions {
    pub provider: Option<Provider>,
    /// Newest first.
    pub versions: Vec<VersionRow>,
    /// Nothing older runs (`floor`).
    pub floor: String,
    /// Every version turned away, installed or not, and why.
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

fn versions_in(root: &Path, bin_rel: &str, floor: &str) -> CliVersions {
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
            below_floor: version_cmp(&record.version, floor).is_lt(),
            bad: bad.get(&record.version).cloned(),
            version: record.version,
            dir,
        })
        .collect();
    CliVersions { provider: None, versions, floor: floor.to_string(), bad, canary }
}

#[tauri::command(async)]
pub fn cli_versions(provider: Provider) -> CliVersions {
    let src = source(provider);
    CliVersions { provider: Some(provider), ..versions_in(&root(provider), src.bin_rel, floor(provider)) }
}

// ----------------------------------------------------------------- floor --

/// The oldest version of each CLI this Luna release downloads, makes current,
/// rolls back to or starts. It rises only here, in a Luna release, when an
/// older version is found unsafe: nothing on this machine moves it
/// (ADR-0030). The release pointer is not signed, so this is what keeps a
/// pointer in other hands from bringing back a release with known flaws.
/// Claude Code 2.1.325 fixed a security issue in how zstd-compressed
/// responses are decoded, and some compound Bash commands running without
/// asking or under an allow rule for another program; before 2.1.323,
/// `--permission-mode auto` (which Luna spawns and inherits) could start in
/// a mode that allows more when auto mode was unavailable. Codex 0.167.0 is
/// the oldest the live suite and the sandbox checks ran on
/// (docs/cli-compatibility.md).
pub(crate) fn floor(provider: Provider) -> &'static str {
    match provider {
        Provider::Claude => "2.1.325",
        Provider::Codex => "0.167.0",
    }
}

/// Refuses `version` below `floor`.
pub(crate) fn at_or_above(floor: &str, version: &str) -> Result<(), String> {
    if version_cmp(version, floor).is_lt() {
        return Err(format!("refusing {version}: older than {floor}, the oldest this Luna lets run"));
    }
    Ok(())
}

/// Every file of an install held again to the sha256 its record kept, for a
/// version rolled back to: it has sat on disk unwatched since it ran. A
/// record without a hash for each file (Codex's, from before they were kept)
/// cannot be held so and is refused.
fn reverify(dir: &Path, record: &Record) -> Result<(), String> {
    for f in &record.files {
        let Some(want) = &f.sha256 else {
            return Err(format!("its record keeps no sha256 for {} to check it against", f.path));
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

/// For the version lookups and the downloads: no deadline on the whole, a
/// download of hundreds of MB taking as long as the line needs.
pub fn http() -> Result<http::Agent, String> {
    http::Agent::new(http::Timeouts { connect: Duration::from_secs(10), read: Duration::from_secs(60), total: None })
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
pub fn download_to(agent: &http::Agent, url: &str, part: &Path, progress: &mut dyn FnMut(u64, Option<u64>)) -> Result<String, String> {
    let resp = agent.get(url).call().map_err(|e| format!("download: {e}"))?;
    let total = resp.header("Content-Length").and_then(|s| s.parse::<u64>().ok());
    progress(0, total);

    let mut file = fs::File::create(part).map_err(|e| format!("create {}: {e}", part.display()))?;
    let mut hasher = Sha256::new();
    let mut got: u64 = 0;
    let mut last_report = std::time::Instant::now();
    let mut write_err = None;
    resp.each_piece(|piece| {
        if let Err(e) = file.write_all(piece) {
            write_err = Some(format!("write: {e}"));
            return Err(String::new());
        }
        hasher.update(piece);
        got += piece.len() as u64;
        // A 100MB file comes in thousands of pieces; a progress event every
        // 200ms is plenty for a progress label.
        if last_report.elapsed() > Duration::from_millis(200) {
            last_report = std::time::Instant::now();
            progress(got, total);
        }
        Ok(())
    })
    .map_err(|e| write_err.take().unwrap_or_else(|| format!("download: {e}")))?;
    file.flush().map_err(|e| e.to_string())?;
    drop(file);
    Ok(format!("{:x}", hasher.finalize()))
}

/// A pass asked for while one runs (a click on "Check now" meanwhile, say),
/// for that one to run again once it is done rather than be lost.
static AGAIN: [AtomicBool; 2] = [AtomicBool::new(false), AtomicBool::new(false)];

fn update(provider: Provider) {
    let busy = &BUSY[provider.index()];
    let again = &AGAIN[provider.index()];
    again.store(true, Ordering::SeqCst);
    if busy.swap(true, Ordering::SeqCst) {
        return;
    }
    loop {
        again.store(false, Ordering::SeqCst);
        pass(provider);
        busy.store(false, Ordering::SeqCst);
        // Asked again meanwhile, and nobody else took the pass up since.
        if !again.load(Ordering::SeqCst) || busy.swap(true, Ordering::SeqCst) {
            break;
        }
    }
}

fn pass(provider: Provider) {
    let result = update_inner(provider);
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
        s.path = binary(provider).map(|b| b.to_string_lossy().into_owned()).unwrap_or_default();
    });
    match result {
        Err(e) => crate::log::warn("cli", &format!("{} update failed: {e}", provider.as_str())),
        Ok(Some(note)) => crate::log::info("cli", &format!("{}: {note}", provider.as_str())),
        Ok(None) => {}
    }
}

/// One pass: what the release pointer names, made current. Ok with a note
/// where `current` was left where it is on purpose.
fn update_inner(provider: Provider) -> Result<Option<String>, String> {
    let root = root(provider);
    publish(provider, |s| {
        s.phase = "checking".into();
        s.error = None;
    });
    if let Some(why) = held_for_elevation() {
        return Ok(Some(why));
    }
    choice::forget_0_19(&root);
    let src = source(provider);
    // A client that cannot be made fails the lookup and the download as an
    // unreachable server would.
    let agent = http().map_err(|e| format!("no HTTP client: {e}"));
    let latest = agent.as_ref().map_err(String::clone).and_then(|a| (src.latest)(a));
    if let Ok(l) = &latest {
        publish(provider, |s| s.latest = Some(l.clone()));
    }
    let probe = |exe: &Path, version: &str| probe_in(&root, provider, exe, version);
    let mut progress = |got: u64, total: Option<u64>| {
        publish(provider, |s| {
            s.got = got;
            s.total = total;
        });
    };
    let mut fetch = |version: &str, dir: &Path| {
        crate::log::info("cli", &format!("downloading {} {version}", provider.as_str()));
        publish(provider, |s| {
            s.phase = "downloading".into();
            s.got = 0;
            s.total = None;
        });
        let agent = agent.as_ref().map_err(String::clone)?;
        (src.fetch)(agent, version, dir, &mut progress)
    };
    let latest = latest.as_ref().map(String::as_str).map_err(String::as_str);
    pass_in(&root, src.bin_rel, floor(provider), latest, &probe, &mut fetch)
}

/// Why the pass changes nothing in a process that runs as administrator: a
/// new version would have to pass the offline probe, which runs the CLI, and
/// nothing of Luna's runs a CLI elevated (elevation.rs, F11). Nothing is
/// fetched or turned away meanwhile; the next pass of a Luna not run so goes on.
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

/// The pass on one provider's tree once the release pointer has been asked
/// (`latest`, an error where it could not be read): `choice::plan` names the
/// version, `update_in` makes it current. `probe` and `fetch` take the
/// version they are for.
fn pass_in(
    root: &Path,
    bin_rel: &str,
    floor: &str,
    latest: Result<&str, &str>,
    probe: &dyn Fn(&Path, &str) -> Result<(), String>,
    fetch: &mut dyn FnMut(&str, &Path) -> Result<Package, String>,
) -> Result<Option<String>, String> {
    let current = current_in(root, bin_rel).filter(|c| c.recorded && version_cmp(&c.version, floor).is_ge());
    let version = match choice::plan(current.as_ref().map(|c| c.version.as_str()), floor, &choice::bad_in(root), latest)? {
        choice::Plan::Stay(why) => return Ok(Some(why)),
        choice::Plan::Install(version) => version,
    };
    let probe = |exe: &Path| probe(exe, &version);
    let target = Target { version: &version, floor, probe: &probe };
    let (dir, flipped) = update_in(root, bin_rel, target, &mut |dir: &Path| fetch(&version, dir))?;
    if flipped {
        crate::log::info("cli", &format!("{version} is now current ({})", root.join("versions").join(dir).display()));
    }
    Ok(None)
}

/// What an update pass is to make current.
#[derive(Clone, Copy)]
struct Target<'a> {
    version: &'a str,
    /// Nothing older is taken however it is named.
    floor: &'a str,
    /// The offline probe (`probe`) of the exe at the path given, which a
    /// version must pass before it becomes current.
    probe: &'a dyn Fn(&Path) -> Result<(), String>,
}

/// The update pass on one provider's tree once `choice::plan` has named its
/// version. Nothing to fetch when `current` holds a recorded install of it,
/// or when one is already in `versions/`; otherwise `fetch` stages a fresh
/// one. Nothing below the floor is fetched or made current. One that fails
/// the offline probe is turned away. Then the flip and the sweep, under the
/// switch lock. Returns the directory now current and whether `current`
/// moved.
fn update_in(
    root: &Path,
    bin_rel: &str,
    target: Target,
    fetch: &mut dyn FnMut(&Path) -> Result<Package, String>,
) -> Result<(String, bool), String> {
    let version = target.version;
    if let Some(c) = current_in(root, bin_rel).filter(|c| c.recorded && c.version == version) {
        // `current` names a recorded install, so this code installed here:
        // a marker missing is one a crash or a cleanup took after the fact.
        let held = switching(root)?;
        mark_recorded(root)?;
        prune_in(&held, root, bin_rel);
        return Ok((c.dir, false));
    }
    at_or_above(target.floor, version)?;
    let dir = match installed_in(root, bin_rel, version) {
        Some(dir) => dir,
        None => install_in(root, bin_rel, version, fetch)?,
    };
    // Whole and where it will run from, before anything runs it as a
    // session. One that fails stays out of `current`, is not offered again
    // until a newer one is out, and the sweep takes it.
    if let Err(e) = (target.probe)(&root.join("versions").join(&dir).join(bin_rel)) {
        let held = switching(root)?;
        if let Err(m) = choice::mark_bad_in(&held, root, version, &format!("failed the offline probe: {e}")) {
            crate::log::warn("cli", &m);
        }
        prune_in(&held, root, bin_rel);
        return Err(format!("{version} failed the offline probe: {e}"));
    }
    let held = switching(root)?;
    // The core may have rolled back from this very version since the plan
    // was made.
    if let Some(why) = choice::bad_in(root).get(version) {
        prune_in(&held, root, bin_rel);
        return Err(format!("{version} was turned away meanwhile ({why})"));
    }
    mark_recorded(root)?;
    flip_in(&held, root, bin_rel, &dir)?;
    prune_in(&held, root, bin_rel);
    Ok((dir, true))
}

/// At a provider's root: held by whoever changes `current`, `previous`,
/// `bad.json` or what is in `versions/` (`switching`).
const SWITCH_LOCK: &str = "switch.lock";

/// The switch lock, held: the window's update pass and the core's rollback
/// are two processes, so it is a file lock, and each takes it for a flip, a
/// turning away or a sweep, never for a download or the probe. Taken while
/// no other lock of the tree is held; the canary's may be taken under it.
struct Switching {
    _lock: fs::File,
}

fn switching(root: &Path) -> Result<Switching, String> {
    fs::create_dir_all(root).map_err(|e| format!("mkdir {}: {e}", root.display()))?;
    let lock = crate::secret::open_private(&root.join(SWITCH_LOCK)).map_err(|e| format!("{SWITCH_LOCK}: {e}"))?;
    lock.lock().map_err(|e| format!("{SWITCH_LOCK}: {e}"))?;
    Ok(Switching { _lock: lock })
}

/// Points `current` at `dir`, an installed directory, and `previous` at what
/// `current` named before. The flip is the last step of an install, so a
/// crash anywhere before it leaves `current` pointing at a directory that is
/// known to be complete; `previous` is written first, so a crash between the
/// two leaves both naming the old one.
/// Through a temp file: the core reads `current` for every spawn, and a read
/// between a plain write's truncate and its bytes would find no version and
/// refuse the start. Private as it is made, under a random name: in
/// a data dir open to others a `current` of theirs would pick the binary
/// every session runs. Synced, so a power cut leaves the old name or the new
/// one. Then what was turned away at or below the version goes.
fn flip_in(held: &Switching, root: &Path, bin_rel: &str, dir: &str) -> Result<(), String> {
    let record = read_record(&root.join("versions").join(dir)).map_err(|e| format!("{dir}: {e}"))?;
    let old = current_in(root, bin_rel);
    if let Some(old) = old.as_ref().filter(|c| c.dir != dir) {
        crate::secret::write_private_synced(&root.join(PREVIOUS), old.dir.as_bytes()).map_err(|e| format!("write {PREVIOUS}: {e}"))?;
    }
    // Another version from now on: its first sessions are watched afresh,
    // and the watch is on disk before `current` names the version, so no
    // session on it starts unwatched.
    if old.is_none_or(|o| o.version != record.version) {
        canary::reset_in(root, &record.version)?;
    }
    crate::secret::write_private_synced(&root.join("current"), dir.as_bytes()).map_err(|e| format!("write current: {e}"))?;
    if let Err(e) = choice::forget_up_to_in(held, root, &record.version) {
        crate::log::warn("cli", &format!("{}: {e}", root.display()));
    }
    Ok(())
}

/// The canary's rollback (`canary`): `version`'s first sessions failed for
/// `why`. Returns the version now current.
fn roll_back(provider: Provider, root: &Path, version: &str, why: &str) -> Result<String, String> {
    let to = roll_back_in(root, source(provider).bin_rel, floor(provider), version, why)?;
    crate::log::info("cli", &format!("{} rolled back from {version} to {to}: {why}", provider.as_str()));
    Ok(to)
}

/// Makes the version `previous` names current again in place of `version`,
/// and turns `version` away until a newer one is out. Only while `version`
/// is still what is current (a pass may have moved on meanwhile), and only
/// to a whole install of another version, not below `floor` nor turned away
/// itself, once every file matches the sha256 its record kept. `previous`
/// goes after: the only version left to go back to is the one just turned
/// away, whose directory the sweep takes once nothing runs from it. So a
/// version gone back to whose first sessions fail too stays: the failures
/// were then not the version's.
fn roll_back_in(root: &Path, bin_rel: &str, floor: &str, version: &str, why: &str) -> Result<String, String> {
    let held = switching(root)?;
    let current = current_in(root, bin_rel).ok_or("nothing is current to roll back from")?;
    if current.version != version {
        return Err(format!("{} is current by now", current.version));
    }
    let prev = previous_in(root, bin_rel).ok_or("no version before it is kept")?;
    let dir = root.join("versions").join(&prev);
    let record = read_record(&dir).map_err(|e| format!("{prev}: {e}"))?;
    let to = record.version.clone();
    if to == version {
        return Err(format!("the version kept before it is {version} too"));
    }
    if version_cmp(&to, floor).is_lt() {
        return Err(format!("{to}, the one before, is older than {floor}, the oldest this Luna runs"));
    }
    if let Some(bad) = choice::bad_in(root).get(&to) {
        return Err(format!("{to}, the one before, was turned away too ({bad})"));
    }
    reverify(&dir, &record).map_err(|e| format!("{to}, the one before: {e}"))?;
    mark_recorded(root)?;
    // The flip before the mark: a crash between leaves the version gone back
    // to current and the failed one offered again, whose canary then trips
    // again, rather than the failed one current and never moved off.
    flip_in(&held, root, bin_rel, &prev)?;
    if let Err(e) = choice::mark_bad_in(&held, root, version, why) {
        crate::log::warn("cli", &format!("rolled back from {version}, but could not turn it away: {e}"));
    }
    match fs::remove_file(root.join(PREVIOUS)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => crate::log::info("cli", &format!("keeping {PREVIOUS}: {e}")),
        _ => {}
    }
    prune_in(&held, root, bin_rel);
    Ok(to)
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
/// its helpers with it. Under the switch lock, so `current` and `previous`
/// cannot move between their reading and the sweep.
fn prune_in(_held: &Switching, root: &Path, bin_rel: &str) {
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

    /// Codex's exe, as its `Source` names it: the canary's rollback finds the
    /// tree by it.
    #[cfg(windows)]
    const BIN: &str = "bin/codex.exe";
    #[cfg(not(windows))]
    const BIN: &str = "bin/codex";
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

    /// The same, as `pass_in` takes it.
    fn passes_any(_: &Path, _: &str) -> Result<(), String> {
        Ok(())
    }

    /// What the release pointer names, with no floor of its own.
    fn pointer(version: &str) -> Target<'_> {
        Target { version, floor: "0", probe: &passes }
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

    /// A pass with the pointer at `latest`, fetching `hashed`.
    fn pass_at(root: &Path, floor: &str, latest: &str) -> Result<Option<String>, String> {
        pass_in(root, BIN, floor, Ok(latest), &passes_any, &mut |_: &str, dir: &Path| hashed(dir))
    }

    fn entries(dir: &Path) -> usize {
        fs::read_dir(dir).map(|d| d.count()).unwrap_or(0)
    }

    fn prune(root: &Path) {
        prune_in(&switching(root).unwrap(), root, BIN);
    }

    fn current_version_in(root: &Path) -> Option<String> {
        current_in(root, BIN).map(|c| c.version)
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
        write_files(&outside, &[(BIN.rsplit('/').next().unwrap(), b"not Luna's")]);
        update_in(&root, BIN, pointer("2.0.0"), &mut whole).unwrap();
        let old = root.join("versions").join("1.0.0");
        fs::create_dir_all(&old).unwrap();
        link_dir(&old.join("bin"), &outside);

        prune(&root);
        assert_eq!(entries(&outside), 1, "the exe outside the tree survived");
        assert!(!old.exists(), "the old version went, its junction as itself");
        assert_eq!(current_version_in(&root).as_deref(), Some("2.0.0"));
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
        let (exe, version, held) = launch_in(&root, BIN, "0").unwrap();
        assert_eq!((exe, version.as_str()), (root.join("versions").join("2.0.0").join(BIN), "2.0.0"));
        update_in(&root, BIN, pointer("3.0.0"), &mut whole).unwrap();
        update_in(&root, BIN, pointer("4.0.0"), &mut whole).unwrap();
        assert_eq!(dirs(&root), ["2.0.0", "3.0.0", "4.0.0"], "1.0.0 went; 2.0.0 runs, 3.0.0 is the previous one");
        assert!(root.join("versions").join("2.0.0").join(FILES[1].0).is_file(), "its helpers too");
        let rows = versions_in(&root, BIN, "0").versions;
        assert_eq!(
            rows.iter().map(|r| (r.version.as_str(), r.current, r.previous, r.in_use)).collect::<Vec<_>>(),
            [("4.0.0", true, false, false), ("3.0.0", false, true, false), ("2.0.0", false, false, true)]
        );

        // Once it has exited, the next pass takes it.
        drop(held);
        prune(&root);
        assert_eq!(dirs(&root), ["3.0.0", "4.0.0"]);
        assert_eq!(entries(&root.join(in_use::DIR)), 2, "the lock files stay, unlocked (cli/in_use.rs)");
        let _ = remove_tree(&root);
    }

    /// A start the sweep could not see would let it take the folder from under
    /// the process: one whose hold cannot be had is refused, never started
    /// unheld.
    #[test]
    fn a_managed_copy_that_cannot_be_held_is_not_started() {
        let root = scratch("unheld");
        update_in(&root, BIN, pointer("1.0.0"), &mut whole).unwrap();
        // The locks' folder taken by a file: no lock can be had there.
        remove_tree(&root.join(in_use::DIR)).ok();
        fs::write(root.join(in_use::DIR), b"not a folder").unwrap();
        let refused = launch_in(&root, BIN, "0").err().expect("refused");
        assert!(refused.contains("not started: cannot hold 1.0.0"), "{refused}");
        // Held again once the lock can be had.
        fs::remove_file(root.join(in_use::DIR)).unwrap();
        assert!(launch_in(&root, BIN, "0").is_ok());
        let _ = remove_tree(&root);
    }

    /// A Luna release that raised the floor: the current version below it
    /// starts nothing, and the CLI on PATH does not stand in for it; the next
    /// pass takes the newest release whatever ran before.
    #[test]
    fn a_current_version_below_the_floor_starts_nothing_until_a_pass_moves_it() {
        let root = scratch("unsafe");
        update_in(&root, BIN, pointer("1.0.0"), &mut hashed).unwrap();
        let refused = launch_in(&root, BIN, "1.5.0").err().expect("refused");
        assert!(refused.contains("1.0.0 is older than 1.5.0"), "{refused}");
        assert!(launch_in(&root, BIN, "1.0.0").is_ok(), "at the floor it runs");
        assert_eq!(pass_at(&root, "1.5.0", "1.6.0"), Ok(None));
        assert!(launch_in(&root, BIN, "1.5.0").is_ok_and(|(_, v, _)| v == "1.6.0"));
        let _ = remove_tree(&root);
    }

    /// Nothing but a managed copy that passed its checks is started: not the
    /// CLI on PATH while none is installed yet, and not a damaged one, each
    /// refused with what it is.
    #[test]
    fn nothing_installed_or_a_damaged_copy_starts_nothing() {
        let root = scratch("damaged");
        let refused = launch_in(&root, BIN, "0").err().expect("nothing installed: refused, not the CLI on PATH");
        assert!(refused.contains("not started") && refused.contains("not installed this CLI"), "{refused}");
        update_in(&root, BIN, pointer("1.0.0"), &mut whole).unwrap();
        assert!(launch_in(&root, BIN, "0").is_ok());
        let damaged = |root: &Path| {
            let refused = launch_in(root, BIN, "0").err().expect("damaged: refused");
            assert!(refused.contains("not started") && refused.contains("is damaged"), "{refused}");
            refused
        };
        // The exe gone from under its record.
        let exe = root.join("versions").join("1.0.0").join(BIN);
        fs::remove_file(&exe).unwrap();
        assert!(damaged(&root).contains("missing files"));
        fs::write(&exe, FILES[0].1).unwrap();
        // The record gone, in a tree this code installed into.
        let record = root.join("versions").join("1.0.0").join(RECORD);
        let kept = fs::read(&record).unwrap();
        fs::remove_file(&record).unwrap();
        assert!(damaged(&root).contains("completion record"));
        fs::write(&record, kept).unwrap();
        // `current` naming no version directory.
        fs::write(root.join("current"), "../elsewhere").unwrap();
        assert!(damaged(&root).contains("no version directory"));
        fs::write(root.join("current"), "1.0.0").unwrap();
        assert!(launch_in(&root, BIN, "0").is_ok());
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

    /// Floors a security fix set; a Luna release may raise them, never lower
    /// them (`floor`, docs/cli-compatibility.md).
    #[test]
    fn the_floors_stay_at_or_above_their_security_fixes() {
        assert!(version_cmp(floor(Provider::Claude), "2.1.325").is_ge(), "{}", floor(Provider::Claude));
        assert!(version_cmp(floor(Provider::Codex), "0.167.0").is_ge(), "{}", floor(Provider::Codex));
        for p in Provider::ALL {
            sane_version(floor(p), "floor").unwrap();
        }
    }

    /// Always the newest the pointer names, at or above the floor: fetched
    /// once, never a step down, nothing below the floor fetched however the
    /// pointer names it.
    #[test]
    fn the_pass_takes_the_newest_release_above_the_floor_and_never_steps_down() {
        let root = scratch("newest");
        assert_eq!(pass_at(&root, "1.0.0", "1.2.0"), Ok(None));
        assert_eq!(pass_at(&root, "1.0.0", "2.0.0"), Ok(None));
        assert_eq!(current_version_in(&root).as_deref(), Some("2.0.0"));
        // The pointer back at 1.2.0, installed as it is: no step down.
        let stayed = pass_in(&root, BIN, "1.0.0", Ok("1.2.0"), &passes_any, &mut |_: &str, _: &Path| panic!("fetched")).unwrap();
        assert!(stayed.unwrap().contains("older than 2.0.0, which runs here"));
        // Below the floor, on a fresh tree too: nothing fetched.
        let fresh = scratch("newest-fresh");
        let stayed = pass_in(&fresh, BIN, "1.0.0", Ok("0.9.0"), &passes_any, &mut |_: &str, _: &Path| panic!("fetched")).unwrap();
        assert!(stayed.unwrap().contains("older than 1.0.0, the oldest this Luna runs"));
        let refused = update_in(&fresh, BIN, Target { floor: "1.0.0", ..pointer("0.9.0") }, &mut |_: &Path| panic!("fetched")).unwrap_err();
        assert!(refused.contains("refusing 0.9.0: older than 1.0.0"), "{refused}");
        // No pointer: an error, and `current` stays.
        assert_eq!(
            pass_in(&root, BIN, "1.0.0", Err("offline"), &passes_any, &mut |_: &str, _: &Path| panic!("fetched")),
            Err("offline".into())
        );
        assert_eq!(current_version_in(&root).as_deref(), Some("2.0.0"));
        let _ = remove_tree(&root);
        let _ = remove_tree(&fresh);
    }

    /// A version left installed from before the floor rose says so.
    #[test]
    fn a_version_below_the_floor_is_flagged() {
        let root = scratch("flagged");
        update_in(&root, BIN, pointer("1.0.0"), &mut hashed).unwrap();
        update_in(&root, BIN, pointer("2.0.0"), &mut hashed).unwrap();
        let rows = versions_in(&root, BIN, "1.5.0").versions;
        assert_eq!(
            rows.iter().map(|r| (r.version.as_str(), r.previous, r.below_floor)).collect::<Vec<_>>(),
            [("2.0.0", false, false), ("1.0.0", true, true)]
        );
        let _ = remove_tree(&root);
    }

    /// A version that fails the offline probe never becomes current, the
    /// sweep takes it, and it is not fetched again until a newer one is out.
    #[test]
    fn a_version_that_fails_the_probe_is_not_made_current_nor_tried_again() {
        let root = scratch("probe");
        update_in(&root, BIN, pointer("1.0.0"), &mut hashed).unwrap();
        let probed = std::cell::RefCell::new(Vec::new());
        let refuse = |exe: &Path, _: &str| {
            probed.borrow_mut().push(exe.to_path_buf());
            Err("`exec` no longer lists --json".to_string())
        };
        let e = pass_in(&root, BIN, "0", Ok("2.0.0"), &refuse, &mut |_: &str, dir: &Path| hashed(dir)).unwrap_err();
        assert!(e.contains("2.0.0 failed the offline probe: `exec` no longer lists --json"), "{e}");
        assert_eq!(*probed.borrow(), [root.join("versions").join("2.0.0").join(BIN)], "the exe where it would run from");
        assert_eq!(current_version_in(&root).as_deref(), Some("1.0.0"));
        assert_eq!(dirs(&root), ["1.0.0"], "the refused version went");
        assert!(choice::bad_in(&root)["2.0.0"].contains("failed the offline probe"));

        let stayed = pass_in(&root, BIN, "0", Ok("2.0.0"), &refuse, &mut |_: &str, _: &Path| panic!("fetched again")).unwrap();
        assert!(stayed.unwrap().contains("2.0.0, the newest release, was turned away (failed the offline probe"));
        assert_eq!(probed.borrow().len(), 1, "nor probed again");
        assert_eq!(pass_at(&root, "0", "2.1.0"), Ok(None), "a newer one is taken");
        assert!(choice::bad_in(&root).is_empty(), "and what it passed is forgotten");
        let _ = remove_tree(&root);
    }

    /// The canary's whole way: two early failures among a new version's
    /// first sessions roll it back to the version before, on their own; the
    /// failed one is not taken again until a newer one is out; a version gone
    /// back to whose sessions fail too stays, with nothing to go back to.
    #[test]
    fn early_failures_of_a_new_version_roll_it_back_on_their_own() {
        assert_eq!(BIN, source(Provider::Codex).bin_rel);
        let root = scratch("rollback");
        assert_eq!(pass_at(&root, "0", "1.0.0"), Ok(None));
        assert_eq!(pass_at(&root, "0", "2.0.0"), Ok(None));
        // The rollback runs on a thread of its own: done once its notice is out.
        let told_with = |title: &str| {
            let deadline = std::time::Instant::now() + Duration::from_secs(20);
            loop {
                let told = crate::emit::sent_for_test("ui://notify");
                if let Some(n) = told.into_iter().find(|n| n["title"] == title) {
                    return n;
                }
                assert!(std::time::Instant::now() < deadline, "no notice {title:?}");
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        let fail = |version: &str, ids: [&str; 2]| {
            for id in ids {
                canary::session_started(Provider::Codex, &root, version, id);
                session_ended(id, true, String::new);
            }
        };
        fail("2.0.0", ["rollback-a", "rollback-b"]);
        told_with("Codex rolled back to 1.0.0");
        assert_eq!(current_version_in(&root).as_deref(), Some("1.0.0"), "rolled back");
        assert!(choice::bad_in(&root)["2.0.0"].starts_with("rolled back: 2 of the 2 sessions started on Codex 2.0.0"));
        assert!(!root.join(PREVIOUS).exists(), "nothing but the failed one to go back to");
        assert_eq!(dirs(&root), ["1.0.0"], "and that one is swept, nothing running from it");
        assert!(launch_in(&root, BIN, "0").is_ok_and(|(_, v, _)| v == "1.0.0"));

        // The pointer still at 2.0.0: not taken again.
        let stayed = pass_in(&root, BIN, "0", Ok("2.0.0"), &passes_any, &mut |_: &str, _: &Path| panic!("fetched")).unwrap();
        assert!(stayed.unwrap().contains("was turned away (rolled back"));

        // 1.0.0's first sessions fail too: nothing to go back to, so it stays.
        fail("1.0.0", ["rollback-c", "rollback-d"]);
        let told = told_with("Codex 1.0.0 may be broken");
        assert!(told["body"].as_str().unwrap().contains("Luna could not roll back: no version before it is kept"), "{told}");
        assert_eq!(current_version_in(&root).as_deref(), Some("1.0.0"));
        assert!(!choice::bad_in(&root).contains_key("1.0.0"));

        // A newer release: taken, and the old turning away forgotten.
        assert_eq!(pass_at(&root, "0", "3.0.0"), Ok(None));
        assert_eq!(current_version_in(&root).as_deref(), Some("3.0.0"));
        assert!(choice::bad_in(&root).is_empty());
        let _ = remove_tree(&root);
    }

    /// The rollback goes only from the version still current, only to a
    /// whole install of another version at or above the floor that was not
    /// turned away, and only once its files match their sha256 again.
    #[test]
    fn a_rollback_goes_only_to_a_kept_version_it_can_vouch_for() {
        let root = scratch("rollback-guard");
        update_in(&root, BIN, pointer("1.0.0"), &mut whole).unwrap();
        update_in(&root, BIN, pointer("2.0.0"), &mut whole).unwrap();
        let back = |floor: &str, from: &str| roll_back_in(&root, BIN, floor, from, "test");
        assert!(back("0", "1.5.0").unwrap_err().contains("2.0.0 is current by now"));
        assert!(back("1.5.0", "2.0.0").unwrap_err().contains("1.0.0, the one before, is older than 1.5.0"));
        // A helper of 1.0.0 altered at its size: sizes pass, the hash does not.
        let helper = root.join("versions").join("1.0.0").join(FILES[1].0);
        fs::write(&helper, b"THE SANDBOX HELPER").unwrap();
        assert!(previous_in(&root, BIN).is_some(), "intact by size");
        assert!(back("0", "2.0.0").unwrap_err().contains("no longer matches its sha256"));
        fs::write(&helper, FILES[1].1).unwrap();
        {
            let held = switching(&root).unwrap();
            choice::mark_bad_in(&held, &root, "1.0.0", "by hand in 0.19").unwrap();
        }
        assert!(back("0", "2.0.0").unwrap_err().contains("was turned away too"));
        assert_eq!(current_version_in(&root).as_deref(), Some("2.0.0"), "each refused, nothing moved");
        assert!(!choice::bad_in(&root).contains_key("2.0.0"));

        // A record from before every file had a sha256: nothing to vouch for
        // it by.
        let legacy = scratch("rollback-legacy");
        update_in(&legacy, BIN, pointer("1.0.0"), &mut whole).unwrap();
        update_in(&legacy, BIN, pointer("2.0.0"), &mut whole).unwrap();
        let path = legacy.join("versions").join("1.0.0").join(RECORD);
        let mut record: Record = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        record.files.iter_mut().for_each(|f| f.sha256 = None);
        fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        assert!(roll_back_in(&legacy, BIN, "0", "2.0.0", "test").unwrap_err().contains("keeps no sha256"));
        let _ = remove_tree(&root);
        let _ = remove_tree(&legacy);
    }

    /// Codex's release vouches for its tarball, not the files in it: each
    /// file's sha256 as unpacked goes into the record, for a rollback to hold
    /// the version to again.
    #[test]
    fn every_file_is_recorded_with_its_sha256() {
        let root = scratch("hashes");
        update_in(&root, BIN, pointer("1.0.0"), &mut whole).unwrap();
        let record = read_record(&root.join("versions").join("1.0.0")).unwrap();
        for (f, (_, bytes)) in record.files.iter().zip(FILES) {
            assert_eq!(f.sha256.as_deref(), Some(format!("{:x}", Sha256::digest(bytes)).as_str()), "{}", f.path);
        }
        let _ = remove_tree(&root);
    }

    /// What Luna 0.19 left at the root (its channel, a version held by hand,
    /// marks good, the floor that rose here, the canary's `dismissed`)
    /// neither breaks the first pass nor holds it: the leftovers go, a bad
    /// mark still counts until a newer version is current.
    #[test]
    fn a_tree_from_0_19_starts_and_moves_on_to_the_newest() {
        let root = scratch("from-0-19");
        update_in(&root, BIN, pointer("1.0.0"), &mut hashed).unwrap();
        update_in(&root, BIN, pointer("2.0.0"), &mut hashed).unwrap();
        for (name, text) in [
            ("channel", "known-good"),
            ("pinned", "1.0.0"),
            ("good.json", r#"{"1.0.0":"marked good by hand"}"#),
            ("marks.lock", ""),
            ("floor", "9.9.9"),
            ("signed-floor", "9.9.9"),
            ("bad.json", r#"{"2.5.0":"marked bad by hand"}"#),
        ] {
            fs::write(root.join(name), text).unwrap();
        }
        let canary = read_to_string_or_panic(&root.join("canary.json")).replace("\"warned\"", "\"dismissed\":true,\"warned\"");
        fs::write(root.join("canary.json"), canary).unwrap();
        assert!(canary::read_in(&root).is_some(), "0.19's canary.json reads");
        assert!(launch_in(&root, BIN, "0").is_ok_and(|(_, v, _)| v == "2.0.0"));

        choice::forget_0_19(&root);
        for name in ["channel", "pinned", "good.json", "marks.lock", "floor", "signed-floor"] {
            assert!(!root.join(name).exists(), "{name}");
        }
        let stayed = pass_at(&root, floor(Provider::Codex), "2.5.0").unwrap();
        assert!(stayed.unwrap().contains("marked bad by hand"));
        // Above 9.9.9's floor of then or not, the newest release is taken.
        assert_eq!(pass_at(&root, floor(Provider::Codex), "3.0.0"), Ok(None));
        assert_eq!(current_version_in(&root).as_deref(), Some("3.0.0"));
        assert!(choice::bad_in(&root).is_empty());
        let _ = remove_tree(&root);
    }

    fn read_to_string_or_panic(path: &Path) -> String {
        fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    /// A new version gets a canary of its own; Settings shows it while that
    /// version is current, and a rollback watches the one gone back to.
    #[test]
    fn each_flip_to_another_version_starts_its_canary() {
        let root = scratch("canary");
        update_in(&root, BIN, pointer("1.0.0"), &mut hashed).unwrap();
        update_in(&root, BIN, pointer("2.0.0"), &mut hashed).unwrap();
        canary::session_started(Provider::Codex, &root, "2.0.0", "canary-flip-a");
        session_ended("canary-flip-a", true, String::new);
        let shown = versions_in(&root, BIN, "0").canary.unwrap();
        assert_eq!((shown.version.as_str(), shown.failures.len()), ("2.0.0", 1));
        // The same version again (nothing flipped): the count stands.
        update_in(&root, BIN, pointer("2.0.0"), &mut |_: &Path| panic!("fetched")).unwrap();
        assert_eq!(versions_in(&root, BIN, "0").canary.unwrap().failures.len(), 1);
        // Rolled back: the old version is watched afresh.
        roll_back_in(&root, BIN, "0", "2.0.0", "test").unwrap();
        let shown = versions_in(&root, BIN, "0").canary.unwrap();
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

    /// The switch writes the canary before `current`: a write of `current`
    /// that fails leaves the old version named, and no session of the new
    /// one starts unwatched.
    #[test]
    fn the_canary_is_on_disk_before_current_names_the_version() {
        let root = scratch("journal");
        update_in(&root, BIN, pointer("1.0.0"), &mut hashed).unwrap();
        let dir = install_in(&root, BIN, "2.0.0", &mut hashed).unwrap();
        // `current` cannot be replaced: a folder with something in it stands
        // at its name, so the rename onto it fails.
        fs::remove_file(root.join("current")).unwrap();
        fs::create_dir(root.join("current")).unwrap();
        fs::write(root.join("current").join("keep"), b"x").unwrap();
        let refused = flip_in(&switching(&root).unwrap(), &root, BIN, &dir);
        assert!(refused.unwrap_err().contains("write current"));
        assert_eq!(canary::read_in(&root).unwrap().version, "2.0.0", "the canary went first");
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
