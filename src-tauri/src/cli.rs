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
//!   versions/<ver>/claude.exe   one directory per installed version
//!   versions/<ver>/luna-install.json   its completion record
//!   staging/<ver>-<random>/     a download in progress
//! <data>/codex-cli/
//!   current
//!   versions/<ver>/bin/codex.exe (+ codex-path/, codex-resources/ from the package)
//!   versions/<ver>/luna-install.json
//!   staging/
//! ```
//!
//! Versions are never overwritten in place: Windows will not let a running
//! exe be replaced, and a session that is mid-turn keeps the old one open
//! for hours. A new version is fetched into `staging/`, held to what its
//! release says the package is, given its completion record last, and
//! renamed into `versions/` under a name nothing has (`<ver>`, or `<ver>~<n>`
//! while an older `<ver>` directory is still there). `current` then flips to
//! it, and old directories are swept whenever nothing holds them any more.
//! A directory counts as installed by its record alone, so a crash
//! mid-download or mid-unpack leaves nothing that passes for one.
//!
//! This module is the machinery — layout, download, checksum, status, the
//! periodic check. Where a version comes from is each provider's `Source`
//! (`claude::cli`, `codex::cli`).

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

fn update_inner(provider: Provider) -> Result<(), String> {
    publish(provider, |s| {
        s.phase = "checking".into();
        s.error = None;
    });
    let src = source(provider);
    let agent = http();
    let latest = (src.latest)(&agent)?;
    publish(provider, |s| s.latest = Some(latest.clone()));

    let mut progress = |got: u64, total: Option<u64>| {
        publish(provider, |s| {
            s.got = got;
            s.total = total;
        });
    };
    let mut fetch = |dir: &Path| {
        crate::log::info("cli", &format!("downloading {} {latest}", provider.as_str()));
        publish(provider, |s| {
            s.phase = "downloading".into();
            s.got = 0;
            s.total = None;
        });
        (src.fetch)(&agent, &latest, dir, &mut progress)
    };
    let (dir, flipped) = update_in(&root(provider), src.bin_rel, &latest, &mut fetch)?;
    if flipped {
        crate::log::info("cli", &format!("{} {latest} is now current (versions/{dir})", provider.as_str()));
    }
    Ok(())
}

/// The update pass on one provider's tree once the channel has named
/// `latest`. Nothing to fetch when `current` holds a recorded install of it,
/// or when one is already in `versions/`; otherwise `fetch` stages a fresh
/// one. Then the flip and the sweep. Returns the directory now current and
/// whether `current` moved.
fn update_in(
    root: &Path,
    bin_rel: &str,
    latest: &str,
    fetch: &mut dyn FnMut(&Path) -> Result<Package, String>,
) -> Result<(String, bool), String> {
    if let Some(c) = current_in(root, bin_rel).filter(|c| c.recorded && c.version == latest) {
        // `current` names a recorded install, so this code installed here:
        // a marker missing is one a crash or a cleanup took after the fact.
        mark_recorded(root)?;
        prune_in(root, bin_rel);
        return Ok((c.dir, false));
    }
    let dir = match installed_in(root, bin_rel, latest) {
        Some(dir) => dir,
        None => install_in(root, bin_rel, latest, fetch)?,
    };
    mark_recorded(root)?;
    // The flip is the last step, so a crash anywhere above leaves `current`
    // pointing at a directory that is known to be complete.
    // Through a temp file: the core reads `current` for every spawn, and a
    // read between a plain write's truncate and its bytes would find no
    // version and spawn whatever is on PATH. Private as it is made, under a
    // random name: in a data dir open to others a `current` of theirs would
    // pick the binary every session runs. Synced, so a power cut leaves the
    // old name or the new one.
    crate::secret::write_private_synced(&root.join("current"), dir.as_bytes()).map_err(|e| format!("write current: {e}"))?;
    prune_in(root, bin_rel);
    Ok((dir, true))
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
        if let Some(want) = &f.sha256 {
            let got = sha256_of(&path)?;
            if !got.eq_ignore_ascii_case(want) {
                return Err(format!("{version}: checksum mismatch for {}: expected {want}, got {got}", f.rel));
            }
        }
        files.push(RecordFile { path: f.rel, size: f.size, sha256: f.sha256.map(|h| h.to_ascii_lowercase()) });
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

/// Drops every version directory except the current one. A directory whose
/// exe a session still runs will not go on Windows; that is fine — it goes
/// on the next pass after the session ends.
fn prune_in(root: &Path, bin_rel: &str) {
    let keep = current_in(root, bin_rel).map(|c| c.dir);
    let versions = root.join("versions");
    if !link_free_below(root, &versions) {
        crate::log::warn("cli", &format!("{} is a link or junction; not sweeping through it", versions.display()));
        return;
    }
    let Ok(entries) = fs::read_dir(&versions) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if Some(name.as_str()) == keep.as_deref() {
            continue;
        }
        let path = entry.path();
        // A link anywhere on the way to the exe (the version directory, its
        // `bin/`) is none of Luna's making: the directory goes by
        // `remove_tree`, which takes a link as itself, and nothing is
        // deleted through one.
        if !link_free_below(root, &path.join(bin_rel)) {
            if let Err(e) = remove_tree(&path) {
                crate::log::info("cli", &format!("keeping {}: {e}", path.display()));
            }
            continue;
        }
        // The exe first: Windows refuses it while a session runs it, and the
        // directory then stays whole. Removed from the top, it would lose the
        // files beside the exe (Codex's codex-path/, codex-resources/) under
        // the running session before failing on the exe itself.
        match fs::remove_file(path.join(bin_rel)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                crate::log::info("cli", &format!("keeping {}: {e}", path.display()));
                continue;
            }
            _ => {}
        }
        if let Err(e) = remove_tree(&path) {
            crate::log::info("cli", &format!("keeping {}: {e}", path.display()));
        }
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
        let cut = update_in(&root, BIN, "1.0.0", &mut |dir: &Path| {
            write_files(dir, &FILES[..1]);
            Err("unpack: unexpected end of file".into())
        });
        assert!(cut.is_err());
        assert!(!root.join("current").exists(), "nothing was activated");
        assert_eq!(entries(&root.join("staging")), 0, "the staged copies are gone");

        let fetched = Cell::new(0);
        let (dir, flipped) = update_in(&root, BIN, "1.0.0", &mut |dir: &Path| {
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
        let (dir, flipped) = update_in(&root, BIN, "1.2.0", &mut whole).unwrap();
        assert_eq!((dir.as_str(), flipped), ("1.2.0", true));
        assert_eq!(fs::read_to_string(root.join("current")).unwrap(), "1.2.0");
        let record = read_record(&root.join("versions").join("1.2.0")).unwrap();
        assert_eq!(record.version, "1.2.0");
        assert_eq!(record.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), [FILES[0].0, FILES[1].0]);
        assert!(current_in(&root, BIN).is_some_and(|c| c.recorded && c.version == "1.2.0"));
        assert_eq!(entries(&root.join("staging")), 0);

        // The next pass finds it installed and fetches nothing.
        let (_, flipped) = update_in(&root, BIN, "1.2.0", &mut |_: &Path| panic!("fetched again")).unwrap();
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
        update_in(&root, BIN, "1.2.0", &mut whole).unwrap();
        assert!(root.join(RECORDED).is_file(), "the tree is marked before `current` names anything");
        // A marker lost after the fact comes back on the next pass, even one
        // with nothing to fetch.
        fs::remove_file(root.join(RECORDED)).unwrap();
        let (_, flipped) = update_in(&root, BIN, "1.2.0", &mut |_: &Path| panic!("fetched again")).unwrap();
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
        update_in(&root, BIN, "2.0.0", &mut whole).unwrap();
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
