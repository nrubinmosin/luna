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
//!   state.json, state.lock      the record: current, the one to go back to, the versions
//!                               turned away, the canary (`state`)
//!   versions/<ver>/claude.exe   one directory per installed version
//!   versions/<ver>/luna-install.json   its completion record
//!   staging/<ver>-<random>/     a download in progress
//!   in-use/<dir>                held (shared) by each process started from <dir>, and by a
//!                               promotion until its flip (`in_use`)
//! <data>/codex-cli/
//!   versions/<ver>/bin/codex.exe (+ codex-path/, codex-resources/ from the package)
//!   (and the rest as for Claude Code)
//! ```
//!
//! Versions are never overwritten in place: Windows will not let a running
//! exe be replaced, and a session that is mid-turn keeps the old one open
//! for hours. A new version is fetched into `staging/`, held to what its
//! release says the package is, given its completion record last, and
//! renamed into `versions/` under a name nothing has (`<ver>`, or `<ver>~<n>`
//! while an older `<ver>` directory is still there). The record then names
//! it current and what it named before the one to go back to, in one write,
//! and old directories are swept whenever nothing names or holds them any
//! more; the one before stays, for the canary to roll back to when the new
//! one's first sessions fail.
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

pub(crate) mod authenticode;
mod canary;
mod choice;
mod in_use;
mod probe;
mod state;

#[cfg(test)]
pub(crate) use canary::cause_outside;
pub use canary::{
    session_awaits_start, session_ended, session_exited, session_running, session_start_through, session_stopped, Canary, LastWords,
};
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

/// What the record names current: the directory sessions spawn from and the
/// version in it.
struct Current {
    dir: String,
    version: String,
    /// False for a directory from before completion records (`state::Slot`).
    /// Sessions keep spawning it; the update pass does not count it as
    /// installed and fetches its version again beside it.
    recorded: bool,
}

/// A directory name out of the record, or one component of a package path.
/// Not a Windows device name either, on any system: `NUL`, `CON`, `COM1`
/// and the rest open the device, with any extension and in any case.
fn plain_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', ':']) && !device_name(name)
}

/// Whether Windows takes `name` for a device: its part before the first dot,
/// trailing spaces dropped, is one of the reserved names (the serial and
/// parallel ports also with a superscript digit).
fn device_name(name: &str) -> bool {
    let base = name.split('.').next().unwrap_or_default().trim_end_matches(' ').to_ascii_uppercase();
    if ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"].contains(&base.as_str()) {
        return true;
    }
    let port = base.strip_prefix("COM").or_else(|| base.strip_prefix("LPT"));
    port.is_some_and(|n| matches!(n, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "\u{b9}" | "\u{b2}" | "\u{b3}"))
}

/// A relative path out of a package: plain components only.
pub(crate) fn plain_rel(rel: &str) -> bool {
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

/// The record's slot for the folder `dir` under `versions/`, by its
/// completion record; for the migration of a tree from before records
/// (`unrecorded`), by the folder alone, named for its version.
fn slot_of(root: &Path, dir: &str, unrecorded: bool) -> Option<state::Slot> {
    if !plain_name(dir) {
        return None;
    }
    let path = root.join("versions").join(dir);
    match read_record(&path) {
        Ok(record) => Some(state::Slot { dir: dir.to_string(), version: record.version, recorded: true }),
        Err(_) if unrecorded && path.is_dir() => Some(state::Slot { dir: dir.to_string(), version: dir.to_string(), recorded: false }),
        Err(_) => None,
    }
}

fn current_in(root: &Path, bin_rel: &str) -> Option<Current> {
    current_or_why(root, bin_rel).ok()
}

/// Why the record gives nothing to start.
#[derive(Debug, PartialEq)]
enum NoCurrent {
    /// Nothing current yet: the first install has not finished.
    NotInstalled,
    /// A record that will not read, or names nothing runnable, and why.
    Damaged(String),
}

fn current_or_why(root: &Path, bin_rel: &str) -> Result<Current, NoCurrent> {
    current_of(root, bin_rel, state::read_in(root).map_err(NoCurrent::Damaged)?)
}

/// `current_or_why` for a record already read, so a caller that needs more
/// of it sees one state.
fn current_of(root: &Path, bin_rel: &str, record: state::State) -> Result<Current, NoCurrent> {
    let slot = record.current.ok_or(NoCurrent::NotInstalled)?;
    let name = slot.dir.as_str();
    if !plain_name(name) {
        return Err(NoCurrent::Damaged(format!("its record names {name:?} current, which is no version directory")));
    }
    let dir = root.join("versions").join(name);
    let damaged = |why: &str| NoCurrent::Damaged(format!("versions/{name}, which its record names current, {why}"));
    if !slot.recorded {
        return match dir.join(bin_rel).is_file() {
            true => Ok(Current { dir: slot.dir.clone(), version: slot.version, recorded: false }),
            false => Err(damaged(&format!("has no {bin_rel}"))),
        };
    }
    match read_record(&dir) {
        Ok(record) if record.version == slot.version && intact(&dir, &record, bin_rel) => {
            Ok(Current { dir: slot.dir.clone(), version: record.version, recorded: true })
        }
        Ok(_) => Err(damaged("is missing files its completion record lists, or has them at another size")),
        Err(e) => Err(damaged(&format!("has no completion record that reads ({e})"))),
    }
}

/// A recorded, intact directory of `version` under `versions/`: one a pass
/// left between its promotion and its flip.
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

/// The managed copy the record names current, for Settings to show; None
/// while there is none to start.
pub fn binary(provider: Provider) -> Option<PathBuf> {
    let src = source(provider);
    let root = root(provider);
    current_in(&root, src.bin_rel).map(|c| root.join("versions").join(c.dir).join(src.bin_rel))
}

/// What a session should spawn: the managed copy the record names current,
/// with a hold on its version directory that the caller keeps until the
/// process has exited, so the sweep leaves that directory be (`in_use`).
/// `session` is the chat or helper it is for, whose version
/// `luna_session_get` then shows.
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

/// How often a launch tries for its hold: the record moving under it, or the
/// lock file not to be had for a moment.
const HOLD_TRIES: u32 = 5;

/// The exe the record names current under `root`, its version, and a hold
/// on its directory taken before the record is read a second time: a flip
/// and a sweep between the two reads would have taken the directory the
/// first one named. An Err where nothing is current, saying whether nothing
/// has been installed yet or what is there is damaged; an Err rather than a
/// start without the hold: the sweep could later take the folder from under
/// the process. An Err too for a current version below `floor`: a Luna
/// release that raised its floor runs nothing older until the next pass
/// moves it.
fn launch_in(root: &Path, bin_rel: &str, floor: &str) -> Result<(PathBuf, String, InUse), String> {
    let mut why = String::new();
    for attempt in 0..HOLD_TRIES {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(20 * u64::from(attempt)));
        }
        // A record lost or garbled is rebuilt from the versions on disk
        // first: an intact install is not left unstartable for want of it.
        let c = match current_or_why(root, bin_rel) {
            Err(_) if attempt == 0 && recover_in(root, bin_rel, floor) => current_or_why(root, bin_rel),
            got => got,
        };
        let c = match c {
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
pub fn cli_status(provider: Provider) -> CliStatus {
    let mut s = state().lock().ok().and_then(|m| m.get(&provider).cloned()).unwrap_or_default();
    s.version = current_version(provider);
    s.path = binary(provider).map(|b| b.to_string_lossy().into_owned()).unwrap_or_default();
    s
}

/// A click on the status-bar field: check now, install if newer.
pub fn cli_update_now(provider: Provider) {
    std::thread::spawn(move || update(provider));
}

/// What a manual rollback did.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RolledBack {
    /// The version turned away, now marked as by a person.
    pub from: String,
    /// The version now current.
    pub to: String,
}

/// Settings → CLIs → Roll back: goes back to the version the record keeps
/// (`CliVersions::way_back`) by the path of the canary's rollback
/// (`roll_back_by`: only a whole install above the floor not turned away,
/// its files held to their sha256), and turns the version left away, by a
/// person. `from` is the version the person saw current: the rollback is
/// made only while it still is, so a click on a screen a pass has since moved
/// on from turns nothing away. The pass then does not take `from` again while
/// the pointer names it (`pass_in`), only a different release. An Err says
/// why nothing moved: the current version is not `from` any more, nothing is
/// current or kept to go back to, the one before is below the floor, turned
/// away, no longer whole or no longer matches its sha256, or the record would
/// not read or write. Running sessions keep their version.
pub fn cli_roll_back(provider: Provider, from: String) -> Result<RolledBack, String> {
    let done = roll_back_by_hand_in(&root(provider), source(provider).bin_rel, floor(provider), &from)?;
    crate::log::info("cli", &format!("{} rolled back by hand from {} to {}", provider.as_str(), done.from, done.to));
    // Read afresh inside the publish: a pass that has moved on since is not
    // shown the version rolled back to, and the path goes with the version.
    publish(provider, |s| {
        s.version = current_version(provider);
        s.path = binary(provider).map(|b| b.to_string_lossy().into_owned()).unwrap_or_default();
    });
    crate::emit::to_ui("cli://canary", serde_json::json!({ "provider": provider, "version": done.from }));
    Ok(done)
}

fn roll_back_by_hand_in(root: &Path, bin_rel: &str, floor: &str, from: &str) -> Result<RolledBack, String> {
    let to = roll_back_by(root, bin_rel, floor, from, "rolled back by hand", true).map_err(|e| e.to_string())?;
    Ok(RolledBack { from: from.to_string(), to })
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
    /// The version Roll back goes to, when the record keeps one.
    pub way_back: Option<WayBack>,
}

/// The version `cli_roll_back` goes back to.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WayBack {
    /// Its directory under `versions/`.
    pub dir: String,
    pub version: String,
    /// Why Roll back would be refused now, from what is cheap to see (the
    /// hashing is left to the command): None where it is open.
    pub refusal: Option<String>,
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
    let record = state::read_in(root).unwrap_or_default();
    let current = current_of(root, bin_rel, record.clone()).ok();
    let canary = record.canary.clone().filter(|c| current.as_ref().is_some_and(|cur| cur.version == c.version));
    let way_back = current.as_ref().and_then(|cur| way_back_of(root, bin_rel, floor, &record, &cur.version));
    let current = current.map(|c| c.dir);
    let previous = record.previous.as_ref().map(|p| p.dir.clone());
    let bad = record.bad_whys();
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
    CliVersions { provider: None, versions, floor: floor.to_string(), bad, canary, way_back }
}

/// The record's way back from `current`, and what already refuses it: the
/// checks of `roll_back_by` that cost no hashing.
fn way_back_of(root: &Path, bin_rel: &str, floor: &str, record: &state::State, current: &str) -> Option<WayBack> {
    let prev = record.previous.as_ref()?;
    let dir = root.join("versions").join(&prev.dir);
    let whole = read_record(&dir).map_err(|e| e.to_string()).and_then(|kept| {
        if kept.version != prev.version || !intact(&dir, &kept, bin_rel) {
            Err("no longer whole".to_string())
        } else if kept.files.iter().any(|f| f.sha256.is_none()) {
            Err("its record keeps no sha256 to check it against".to_string())
        } else {
            Ok(())
        }
    });
    let refusal = if prev.version == current {
        Some(format!("the version kept before it is {current} too"))
    } else if version_cmp(&prev.version, floor).is_lt() {
        Some(format!("{} is older than {floor}, the oldest this Luna runs", prev.version))
    } else if let Some(mark) = record.bad.get(&prev.version) {
        Some(format!("{} was turned away ({})", prev.version, mark.why))
    } else {
        whole.err().map(|e| format!("{}: {e}", prev.version))
    };
    Some(WayBack { dir: prev.dir.clone(), version: prev.version.clone(), refusal })
}

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
/// a mode that allows more when auto mode was unavailable. Codex 0.169.0 is
/// the `codex app-server` protocol Luna's helpers speak (experimental:
/// background/appserver.rs, docs/cli-compatibility.md), seen live on it.
pub(crate) fn floor(provider: Provider) -> &'static str {
    match provider {
        Provider::Claude => "2.1.325",
        Provider::Codex => "0.169.0",
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
/// version rolled back to, reused or taken back after a lost record: it has
/// sat on disk unwatched since it ran. Nothing but those files and the record
/// may be in its folder either: a file an older Luna's unpacker left there,
/// or one added since, would run unchecked. A record without a hash for each
/// file (Codex's, from before they were kept) cannot be held so and is
/// refused.
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
    let listed: std::collections::HashSet<&str> = record.files.iter().map(|f| f.path.as_str()).chain([RECORD]).collect();
    match unlisted(dir, "", &listed)? {
        Some(stray) => Err(format!("{stray:?} is in its folder but not in its record")),
        None => Ok(()),
    }
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

/// Orders two `sane_version`s as semver does: by their dotted numbers (each
/// part's leading digits; a missing part counts as 0), so 2.1.100 is above
/// 2.1.99; then a pre-release (`-alpha.1`) below the release it leads to,
/// two pre-releases by their dotted identifiers (numbers by value and below
/// words, words by their bytes, a longer list above its own start). Build
/// metadata (`+…`) does not count. Numbers are compared as digit strings, by
/// length once their leading zeros are gone and then digit by digit, so no
/// number is too long to order.
pub(crate) fn version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering::{Equal, Greater, Less};
    fn number(x: &str, y: &str) -> std::cmp::Ordering {
        let (x, y) = (x.trim_start_matches('0'), y.trim_start_matches('0'));
        x.len().cmp(&y.len()).then_with(|| x.cmp(y))
    }
    let digits = |p: &str| p.chars().take_while(|c| c.is_ascii_digit()).collect::<String>();
    let split = |v: &'_ str| -> (Vec<String>, Option<String>) {
        let v = v.split('+').next().unwrap_or(v);
        let (core, pre) = match v.split_once('-') {
            Some((core, pre)) => (core, Some(pre.to_string())),
            None => (v, None),
        };
        (core.split('.').map(digits).collect(), pre)
    };
    let ((a, a_pre), (b, b_pre)) = (split(a), split(b));
    let zero = String::new();
    let core = (0..a.len().max(b.len())).map(|i| number(a.get(i).unwrap_or(&zero), b.get(i).unwrap_or(&zero))).find(|o| o.is_ne());
    if let Some(o) = core {
        return o;
    }
    match (a_pre, b_pre) {
        (None, None) => Equal,
        (None, Some(_)) => Greater,
        (Some(_), None) => Less,
        (Some(a), Some(b)) => {
            let (a, b): (Vec<&str>, Vec<&str>) = (a.split('.').collect(), b.split('.').collect());
            let numeric = |x: &str| !x.is_empty() && x.bytes().all(|c| c.is_ascii_digit());
            let id = |x: &str, y: &str| match (numeric(x), numeric(y)) {
                (true, true) => number(x, y),
                (true, false) => Less,
                (false, true) => Greater,
                (false, false) => x.cmp(y),
            };
            a.iter().zip(&b).map(|(x, y)| id(x, y)).find(|o| o.is_ne()).unwrap_or(a.len().cmp(&b.len()))
        }
    }
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
/// pass sweeps if this one cannot. A scratch home not to be had is Luna's
/// fault, not the version's.
fn probe_in(root: &Path, provider: Provider, exe: &Path, version: &str) -> Result<(), probe::Failed> {
    let home = clean_staging(root).map_err(probe::Failed::Unasked)?.join(format!("probe-{}", crate::secret::random_hex(4)));
    fs::create_dir(&home).map_err(|e| probe::Failed::Unasked(format!("mkdir {}: {e}", home.display())))?;
    crate::log::info("cli", &format!("probing {} {version}", provider.as_str()));
    let probed = probe::probe(provider, exe, version, &home);
    let _ = remove_tree(&home);
    probed
}

/// The pass on one provider's tree once the release pointer has been asked
/// (`latest`, an error where it could not be read). The pointer read, every
/// mark of another version goes from the record (a tree from before the
/// record migrates in the same write); `choice::plan` names the version,
/// `update_in` makes it current. `probe` and `fetch` take the version they
/// are for.
fn pass_in(
    root: &Path,
    bin_rel: &str,
    floor: &str,
    latest: Result<&str, &str>,
    probe: &dyn Fn(&Path, &str) -> Result<(), probe::Failed>,
    fetch: &mut dyn FnMut(&str, &Path) -> Result<Package, String>,
) -> Result<Option<String>, String> {
    recover_in(root, bin_rel, floor);
    let record = state::change(root, |s| {
        if let Ok(named) = latest {
            s.bad.retain(|v, _| v == named);
        }
        s.clone()
    })?;
    // What runs, recorded or from before records alike: no step down from
    // either. One below the floor runs nothing, and holds nothing back.
    let current = current_in(root, bin_rel).filter(|c| version_cmp(&c.version, floor).is_ge());
    // A pass that installs nothing still sweeps: a version rolled away from
    // while a session held it goes once that session is over, though the
    // pointer has not moved.
    let version = match choice::plan(current.as_ref().map(|c| c.version.as_str()), floor, &record.bad_whys(), latest) {
        Ok(choice::Plan::Stay(why)) => {
            prune_in(root, bin_rel);
            return Ok(Some(why));
        }
        Ok(choice::Plan::Install(version)) => version,
        Err(e) => {
            prune_in(root, bin_rel);
            return Err(e);
        }
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
    probe: &'a dyn Fn(&Path) -> Result<(), probe::Failed>,
}

/// The update pass on one provider's tree once `choice::plan` has named its
/// version. Nothing to fetch when the record names a recorded install of it
/// current, or when one already in `versions/` still matches its record's
/// sha256; otherwise `fetch` stages a fresh one. Nothing below the floor is
/// fetched or made current. From its promotion to its flip the version is
/// held (`in_use`), so no sweep takes it meanwhile. One the probe finds wrong
/// is turned away; one it could not ask is left for the next pass. Then the
/// flip, one write of the record (`flip_in`), and the sweep. Returns the
/// directory now current and whether it moved.
fn update_in(
    root: &Path,
    bin_rel: &str,
    target: Target,
    fetch: &mut dyn FnMut(&Path) -> Result<Package, String>,
) -> Result<(String, bool), String> {
    let version = target.version;
    if let Some(c) = current_in(root, bin_rel).filter(|c| c.recorded && c.version == version) {
        prune_in(root, bin_rel);
        return Ok((c.dir, false));
    }
    at_or_above(target.floor, version)?;
    // One a pass left between its promotion and its flip has sat there
    // unwatched: held first, then held to every sha256 its record kept, or
    // fetched afresh beside it.
    let found = installed_in(root, bin_rel, version).and_then(|dir| in_use::hold(root, &dir).map(|held| (dir, held)));
    let found = found.filter(|(dir, _)| {
        let path = root.join("versions").join(dir);
        match read_record(&path).map_err(|e| e.to_string()).and_then(|record| reverify(&path, &record)) {
            Ok(()) => true,
            Err(e) => {
                crate::log::info("cli", &format!("fetching {version} again: versions/{dir} {e}"));
                false
            }
        }
    });
    let (dir, being_made_current) = match found {
        Some(found) => found,
        None => install_in(root, bin_rel, version, fetch)?,
    };
    // Whole and where it will run from, before anything runs it as a
    // session.
    match (target.probe)(&root.join("versions").join(&dir).join(bin_rel)) {
        Ok(()) => {}
        // Not offered again while the pointer names it; the sweep takes it.
        Err(probe::Failed::Wrong(e)) => {
            drop(being_made_current);
            let why = format!("failed the offline probe: {e}");
            state::change(root, |s| s.bad.insert(version.to_string(), state::Mark::new(short(&why), state::By::Probe)))?;
            step("sweep")?;
            prune_in(root, bin_rel);
            return Err(format!("{version} {why}"));
        }
        Err(probe::Failed::Unasked(e)) => return Err(format!("{version} could not be probed ({e}); the next pass tries again")),
    }
    if let Err(e) = flip_in(root, &dir, version) {
        drop(being_made_current);
        prune_in(root, bin_rel);
        return Err(e);
    }
    step("sweep")?;
    prune_in(root, bin_rel);
    drop(being_made_current);
    Ok((dir, true))
}

/// Words for a person to read: one line, kept short.
fn short(why: &str) -> String {
    why.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(300).collect()
}

#[cfg(test)]
thread_local! {
    /// For this test thread: the step of this name fails, as the process
    /// dying just before it would leave the tree.
    static FAIL_AT: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
}

/// A step of a change to the tree, about to be taken: `state`, the record's
/// one write; `sweep`, the sweep after it.
fn step(name: &'static str) -> Result<(), String> {
    #[cfg(test)]
    if FAIL_AT.with(std::cell::Cell::get) == Some(name) {
        return Err(format!("stopped before {name}, as a crash would"));
    }
    let _ = name;
    Ok(())
}

/// Makes the installed directory `dir`, of `version`, current: one write of
/// the record. What ran until now becomes the one to go back to, unless it
/// was on its way out itself (turned away, or its canary tripped): then the
/// one before it stays. A version not current before gets a fresh canary in
/// the same write, so no session on it starts unwatched. Refused for a
/// version turned away since the pass chose it (the core's rollback from it).
fn flip_in(root: &Path, dir: &str, version: &str) -> Result<(), String> {
    let slot = state::Slot { dir: dir.to_string(), version: version.to_string(), recorded: true };
    state::change(root, |s| {
        if let Some(mark) = s.bad.get(version) {
            return Err(format!("{version} was turned away meanwhile ({})", mark.why));
        }
        let old = s.current.take();
        let leaving = old
            .as_ref()
            .is_some_and(|o| s.bad.contains_key(&o.version) || s.canary.as_ref().is_some_and(|c| c.version == o.version && c.warned));
        if old.as_ref().is_none_or(|o| o.version != version) {
            s.canary = Some(canary::fresh(version));
        }
        match old {
            Some(o) if o.dir == dir => {}
            Some(o) if !leaving => s.previous = Some(o),
            _ => {}
        }
        s.current = Some(slot);
        Ok(())
    })?
}

#[cfg(test)]
thread_local! {
    /// For this test thread: called between the rollback's checks and its
    /// write, as another process's change would land there.
    static BEFORE_ROLLBACK_WRITE: std::cell::RefCell<Option<Box<dyn FnMut()>>> = const { std::cell::RefCell::new(None) };
}

/// Why a rollback was not made.
#[derive(Debug, PartialEq)]
pub(crate) enum NoRollback {
    /// The tree does not allow it (nothing to go back to, or nothing Luna
    /// can vouch for): noted with the canary, not tried again.
    Refused(String),
    /// The record could not be read or written: tried again at the next
    /// core's start.
    Failed(String),
    /// The version to roll back from is no longer the current one (a pass,
    /// or the person's own rollback, moved on): nothing to roll back, and
    /// nothing to announce.
    Moved(String),
}

impl std::fmt::Display for NoRollback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NoRollback::Refused(why) | NoRollback::Failed(why) | NoRollback::Moved(why) => f.write_str(why),
        }
    }
}

/// The canary's rollback (`canary`): `version`'s first sessions failed for
/// `why`. Returns the version now current.
fn roll_back(provider: Provider, root: &Path, version: &str, why: &str) -> Result<String, NoRollback> {
    let to = roll_back_in(root, source(provider).bin_rel, floor(provider), version, why)?;
    crate::log::info("cli", &format!("{} rolled back from {version} to {to}: {why}", provider.as_str()));
    Ok(to)
}

/// `roll_back_by` for the canary's rollback, not asked for by a person.
fn roll_back_in(root: &Path, bin_rel: &str, floor: &str, version: &str, why: &str) -> Result<String, NoRollback> {
    roll_back_by(root, bin_rel, floor, version, why, false)
}

/// Makes the version the record keeps to go back to current again in place
/// of `version`, and turns `version` away (marked as asked for by a person where `person`), in one write of the
/// record: only while `version` is still current (a pass may have moved on),
/// only to a whole install of another version, not below `floor` nor turned
/// away itself, once every file matches the sha256 its record kept (hashed
/// before the write, outside the lock; held to the record again in the write:
/// the same slots, the one before still not turned away). Afterwards nothing
/// is kept to go back to: the only version left is the one just turned away,
/// whose directory the sweep takes once nothing runs from it. So a version
/// gone back to whose first sessions fail too stays: the failures were then
/// not the version's.
fn roll_back_by(root: &Path, bin_rel: &str, floor: &str, version: &str, why: &str, person: bool) -> Result<String, NoRollback> {
    let refused = |why: String| NoRollback::Refused(why);
    let moved = |now: &str| NoRollback::Moved(format!("the current version changed (it is {now} now); look again"));
    let record = state::read_in(root).map_err(NoRollback::Failed)?;
    let current = record.current.ok_or_else(|| refused("nothing is current to roll back from".into()))?;
    if current.version != version {
        return Err(moved(&current.version));
    }
    let prev = record.previous.ok_or_else(|| refused("no version before it is kept".into()))?;
    let to = prev.version.clone();
    if to == version {
        return Err(refused(format!("the version kept before it is {version} too")));
    }
    if version_cmp(&to, floor).is_lt() {
        return Err(refused(format!("{to}, the one before, is older than {floor}, the oldest this Luna runs")));
    }
    if let Some(bad) = record.bad.get(&to) {
        return Err(refused(format!("{to}, the one before, was turned away too ({})", bad.why)));
    }
    let dir = root.join("versions").join(&prev.dir);
    let kept = read_record(&dir).map_err(|e| refused(format!("{to}, the one before: {e}")))?;
    if kept.version != to || !intact(&dir, &kept, bin_rel) {
        return Err(refused(format!("{to}, the one before, is no longer whole")));
    }
    reverify(&dir, &kept).map_err(|e| refused(format!("{to}, the one before: {e}")))?;
    #[cfg(test)]
    BEFORE_ROLLBACK_WRITE.with(|hook| hook.borrow_mut().as_mut().map(|f| f()));
    state::change(root, |s| {
        if s.current.as_ref().map(|c| &c.version) != Some(&current.version) {
            return Err(moved(s.current.as_ref().map_or("nothing", |c| c.version.as_str())));
        }
        if s.current.as_ref() != Some(&current) || s.previous.as_ref() != Some(&prev) {
            return Err(NoRollback::Failed("the versions changed while it was checked".to_string()));
        }
        if let Some(bad) = s.bad.get(&to) {
            return Err(refused(format!("{to}, the one before, was turned away meanwhile ({})", bad.why)));
        }
        s.bad.insert(version.to_string(), state::Mark { why: short(why), by: state::By::Rollback, person });
        s.canary = Some(canary::fresh(&to));
        s.current = Some(prev.clone());
        s.previous = None;
        Ok(())
    })
    .map_err(NoRollback::Failed)??;
    step("sweep").map_err(NoRollback::Failed)?;
    prune_in(root, bin_rel);
    Ok(to)
}

/// A rollback the record says is due and not done: the current version's
/// canary tripped, with nothing come of it (the core went away between the
/// trip and the rollback, or the rollback's write failed). The version, what
/// was seen of it, and what the rollback came to.
fn resume_in(provider: Provider, root: &Path, bin_rel: &str, floor: &str) -> Option<(String, String, Result<String, NoRollback>)> {
    let record = state::read_in(root).ok()?;
    let version = record.current?.version;
    let tripped = record.canary.filter(|c| c.version == version && c.warned && c.outcome.is_none())?;
    let seen = canary::seen_of(provider, &tripped);
    let rolled = roll_back_in(root, bin_rel, floor, &version, &format!("rolled back: {seen}"));
    Some((version, seen, rolled))
}

/// At the core's start, before any session: a lost record rebuilt from the
/// disk (`recover_in`), the canary's watches the last core held in memory
/// given back (`canary::release_orphans`), and each rollback the record says
/// is due made, the person told as for any.
pub fn finish_rollbacks() {
    for provider in Provider::ALL {
        let root = root(provider);
        let (bin_rel, floor) = (source(provider).bin_rel, floor(provider));
        recover_in(&root, bin_rel, floor);
        if let Err(e) = canary::release_orphans(&root) {
            crate::log::warn("cli", &format!("canary: {e}"));
        }
        let Some((version, seen, rolled)) = resume_in(provider, &root, bin_rel, floor) else { continue };
        canary::settle_rollback(provider, &root, &version, &seen, rolled);
    }
}

/// A tree with no record to go by (`state::lost_in`: lost, or not parsing)
/// rebuilt from what is on disk: the newest installed version at or above
/// the floor whose every file still matches the sha256 its completion
/// record kept made current, the next such version the one to go back to.
/// Hashed outside the lock, written only from what verified and only while
/// the record is still lost; where nothing verifies nothing is written, and
/// the pass goes on as on a fresh install. Marks and the canary are lost
/// with the record. Returns whether a record was written.
fn recover_in(root: &Path, bin_rel: &str, floor: &str) -> bool {
    if !state::lost_in(root) {
        return false;
    }
    let mut verified: Vec<state::Slot> = Vec::new();
    for (dir, record) in installed_all(root, bin_rel) {
        if verified.len() == 2 {
            break;
        }
        if version_cmp(&record.version, floor).is_lt() || verified.iter().any(|v| v.version == record.version) {
            continue;
        }
        match reverify(&root.join("versions").join(&dir), &record) {
            Ok(()) => verified.push(state::Slot { dir, version: record.version, recorded: true }),
            Err(e) => crate::log::info("cli", &format!("not taking versions/{dir} back: {e}")),
        }
    }
    let mut verified = verified.into_iter();
    let Some(current) = verified.next() else { return false };
    let rebuilt = state::State { current: Some(current), previous: verified.next(), ..Default::default() };
    let written = state::lock_in(root).and_then(|held| match state::lost_in(root) {
        true => held.save(&rebuilt).map(|()| true),
        false => Ok(false),
    });
    match written {
        Ok(true) => {
            crate::log::warn("cli", &format!("{}: its record was lost; rebuilt from the versions on disk", root.display()));
            true
        }
        Ok(false) => false,
        Err(e) => {
            crate::log::warn("cli", &format!("{}: {e}", root.display()));
            false
        }
    }
}

/// Fetches `version` into a fresh staging directory, holds it to what the
/// fetch vouched for, puts it and its record on disk, and renames it into
/// `versions/`, held (`in_use`) from before the rename, so no sweep takes it
/// before its flip. Returns the name it got there and the hold. Whatever
/// fails, the staged directory goes.
fn install_in(
    root: &Path,
    bin_rel: &str,
    version: &str,
    fetch: &mut dyn FnMut(&Path) -> Result<Package, String>,
) -> Result<(String, InUse), String> {
    let staging = clean_staging(root)?;
    let staged = staging.join(format!("{version}-{}", crate::secret::random_hex(4)));
    let versions = root.join("versions");
    fs::create_dir(&staged).map_err(|e| format!("mkdir {}: {e}", staged.display()))?;
    let done = fetch(&staged)
        .and_then(|package| validate(&staged, version, bin_rel, package))
        .and_then(|record| persist(&staged, &record))
        .and_then(|()| promote(root, &staged, version));
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
/// one, and nothing else is: the folder is promoted whole, so a file the
/// list leaves out (one an unpacker wrote under another name than the
/// listed) would be installed unchecked. The executable must be among them.
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
    let listed: std::collections::HashSet<&str> = files.iter().map(|f| f.path.as_str()).collect();
    if let Some(stray) = unlisted(dir, "", &listed)? {
        return Err(format!("{version}: {stray:?} is in the package's folder but not in the package"));
    }
    Ok(Record { version: version.into(), verified: package.verified, files })
}

/// The first entry under `dir` (its path below the staged root being
/// `prefix`) that is neither a folder nor a file `listed` names, exactly as
/// named on disk; a link or junction counts as one, and is not gone into.
fn unlisted(dir: &Path, prefix: &str, listed: &std::collections::HashSet<&str>) -> Result<Option<String>, String> {
    for entry in fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| format!("read {}: {e}", dir.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
        let meta = fs::symlink_metadata(entry.path()).map_err(|e| format!("read {rel}: {e}"))?;
        if meta.is_dir() && !is_link(&meta) {
            if let Some(stray) = unlisted(&entry.path(), &rel, listed)? {
                return Ok(Some(stray));
            }
        } else if is_link(&meta) || !meta.is_file() || !listed.contains(rel.as_str()) {
            return Ok(Some(rel));
        }
    }
    Ok(None)
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
/// The name is held (`in_use`) before the rename: the sweep, which takes a
/// folder only under its exclusive lock, cannot take the new one before the
/// flip makes the record name it.
fn promote(root: &Path, staged: &Path, version: &str) -> Result<(String, InUse), String> {
    let versions = root.join("versions");
    fs::create_dir_all(&versions).map_err(|e| format!("mkdir {}: {e}", versions.display()))?;
    for n in 0..1000u32 {
        let name = if n == 0 { version.to_string() } else { format!("{version}~{n}") };
        let target = versions.join(&name);
        if fs::symlink_metadata(&target).is_ok() {
            continue;
        }
        let held = in_use::hold(root, &name).ok_or_else(|| format!("cannot hold {name} in {}", root.join(in_use::DIR).display()))?;
        match fs::rename(staged, &target) {
            Ok(()) => return Ok((name, held)),
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

#[cfg(test)]
thread_local! {
    /// For this test thread: called with each folder the sweep is about to
    /// judge, before it takes the folder's hold.
    static BEFORE_SWEEP: std::cell::RefCell<Option<Box<dyn FnMut(&str)>>> = const { std::cell::RefCell::new(None) };
}

/// Drops every version directory but those the record names (current and
/// the one to go back to) and those a process of Luna's runs from or a
/// promotion holds (`in_use`); such a one goes on a later pass, once nothing
/// holds it. On Windows a running exe would not go anyway; on Unix the hold
/// is all that keeps a running session's directory, its helpers with it.
/// Each folder is judged only once its exclusive hold is had, by the record
/// read afresh under its lock then: a flip that named a folder after the
/// sweep began (and has let go of its promotion's hold) is seen. The folder
/// is then removed outside the record's lock: while the sweep holds it, no
/// write can come to name it, as a flip names only a folder it holds (which
/// waits on the sweep's hold) and a rollback only the one the record already
/// names. A tree with no record to go by (`state::lost_in`) is not swept at
/// all: what it would keep is unknown until it is rebuilt.
fn prune_in(root: &Path, bin_rel: &str) {
    // What the record names, read under its lock; None where it cannot be
    // read or there is none to go by.
    let named = || -> Option<Vec<String>> {
        let _held = state::lock_in(root).ok()?;
        match state::lost_in(root) {
            true => None,
            false => state::read_in(root).ok().map(|r| r.named()),
        }
    };
    let Some(keep) = named() else {
        crate::log::warn("cli", &format!("{}: no record to go by; not sweeping", root.display()));
        return;
    };
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
        #[cfg(test)]
        BEFORE_SWEEP.with(|hook| hook.borrow_mut().as_mut().map(|f| f(&name)));
        // Held until the directory is gone: nothing starts from it meanwhile.
        let Some(sweeping) = in_use::try_sweep(root, &name) else {
            crate::log::info("cli", &format!("keeping {name}: a process of Luna's runs from it"));
            continue;
        };
        if named().is_none_or(|now| now.contains(&name)) {
            continue;
        }
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
    fn passes(_: &Path) -> Result<(), probe::Failed> {
        Ok(())
    }

    /// The same, as `pass_in` takes it.
    fn passes_any(_: &Path, _: &str) -> Result<(), probe::Failed> {
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
        prune_in(root, BIN);
    }

    fn current_version_in(root: &Path) -> Option<String> {
        current_in(root, BIN).map(|c| c.version)
    }

    /// The versions the record turns away, with why.
    fn bad_of(root: &Path) -> std::collections::BTreeMap<String, String> {
        state::read_in(root).unwrap().bad_whys()
    }

    /// The folder the record keeps to go back to.
    fn previous_of(root: &Path) -> Option<String> {
        state::read_in(root).unwrap().previous.map(|p| p.dir)
    }

    /// Turns `version` away, as the core's rollback from it would.
    fn mark(root: &Path, version: &str, why: &str) {
        state::change(root, |s| s.bad.insert(version.into(), state::Mark::new(why, state::By::Rollback))).unwrap();
    }

    /// The current version's canary tripped, as `fail_in` leaves it before
    /// the rollback.
    fn trip(root: &Path) {
        state::change(root, |s| {
            let c = s.canary.as_mut().unwrap();
            (c.failures, c.watched, c.warned) = (vec!["a".into(), "b".into()], 2, true);
        })
        .unwrap();
    }

    /// Runs `f` with the step `at` failing on this thread, as the process
    /// dying just before it.
    fn cut_at<T>(at: &'static str, f: impl FnOnce() -> T) -> T {
        FAIL_AT.with(|s| s.set(Some(at)));
        let out = f();
        FAIL_AT.with(|s| s.set(None));
        out
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

    /// A Windows device name opens the device, not a file, whatever follows
    /// its first dot and in any case.
    #[test]
    fn a_device_name_is_no_plain_name() {
        for name in ["NUL", "nul", "Con.txt", "aux.tar.gz", "COM1", "com9.dll", "LPT3.exe", "COM\u{b9}", "prn .x", "CONIN$", "conout$.log"]
        {
            assert!(!plain_name(name), "{name}");
        }
        assert!(!plain_rel("codex-resources/voice/bin/NUL.dll"));
        for name in ["null", "console.exe", "COM0", "COM10", "LPT", "auxiliary.dll", "nul_", "x.nul", "codex.exe"] {
            assert!(plain_name(name), "{name}");
        }
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
        assert!(current_version_in(&root).is_none(), "nothing was activated");
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
        // A file the package does not list, beside the exe or below it: the
        // folder is promoted whole, so it would be installed unchecked.
        for stray in ["claude.dll", "sub/x.dll", "CLAUDE.EXE.dll"] {
            let refused = install_in(&root, "claude.exe", "2.0.0", &mut |dir: &Path| {
                fs::write(dir.join("claude.exe"), full).unwrap();
                write_files(dir, &[(stray, b"unchecked")]);
                Ok(package())
            });
            assert!(refused.unwrap_err().contains(&format!("{stray:?} is in the package's folder but not in the package")));
        }
        assert!(!root.join("versions").exists(), "nothing was promoted");
        assert_eq!(entries(&root.join("staging")), 0);
        let _ = remove_tree(&root);
    }

    #[test]
    fn a_whole_package_is_activated_with_its_record() {
        let root = scratch("whole");
        let (dir, flipped) = update_in(&root, BIN, pointer("1.2.0"), &mut whole).unwrap();
        assert_eq!((dir.as_str(), flipped), ("1.2.0", true));
        assert_eq!(state::read_in(&root).unwrap().current.unwrap().dir, "1.2.0");
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

        let (name, _held) = promote(&root, &staged, "3.0.0").unwrap();
        assert_eq!(name, "3.0.0~1");
        assert_eq!(snapshot(&running), before, "nothing in the running version changed");
        assert!(!staged.exists());
        assert_eq!(fs::read(root.join("versions").join(&name).join(BIN)).unwrap(), FILES[0].1);
        let _ = remove_tree(&root);
    }

    /// A record lost after install (a power cut before it reached the disk,
    /// on an older layout) must not let the bare exe pass: only a tree from
    /// before records runs a directory without one.
    #[test]
    fn a_version_installed_here_is_not_run_without_its_record() {
        let root = scratch("unrecorded");
        update_in(&root, BIN, pointer("1.2.0"), &mut whole).unwrap();
        fs::remove_file(root.join("versions").join("1.2.0").join(RECORD)).unwrap();
        assert!(current_in(&root, BIN).is_none());
        let _ = remove_tree(&root);

        // From before records: no `recorded` marker, and `current` names a
        // directory with its exe. Still run, but not taken for installed.
        let legacy = scratch("legacy");
        write_files(&legacy.join("versions").join("0.9.0"), &FILES[..1]);
        fs::write(legacy.join("current"), "0.9.0").unwrap();
        assert!(current_in(&legacy, BIN).is_some_and(|c| !c.recorded && c.version == "0.9.0"));
        assert!(installed_in(&legacy, BIN, "0.9.0").is_none());
        // With the marker, the same folder is an install whose record is gone.
        fs::write(legacy.join("recorded"), "x").unwrap();
        assert!(current_in(&legacy, BIN).is_none());
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
        assert_eq!(previous_of(&root).as_deref(), Some("1.0.0"));

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
        assert_eq!(entries(&root.join(in_use::DIR)), 4, "the lock files stay, unlocked, one per directory promoted (cli/in_use.rs)");
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
        let refused = launch_in(&root, BIN, "0").expect_err("refused");
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
        let refused = launch_in(&root, BIN, "1.5.0").expect_err("refused");
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
        let refused = launch_in(&root, BIN, "0").expect_err("nothing installed: refused, not the CLI on PATH");
        assert!(refused.contains("not started") && refused.contains("not installed this CLI"), "{refused}");
        update_in(&root, BIN, pointer("1.0.0"), &mut whole).unwrap();
        assert!(launch_in(&root, BIN, "0").is_ok());
        let damaged = |root: &Path| {
            let refused = launch_in(root, BIN, "0").expect_err("damaged: refused");
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
        // The record naming no version directory, then not reading at all.
        let name = |dir: &str| state::change(&root, |s| s.current.as_mut().unwrap().dir = dir.into()).unwrap();
        name("../elsewhere");
        assert!(damaged(&root).contains("no version directory"));
        name("1.0.0");
        assert!(launch_in(&root, BIN, "0").is_ok());
        // A record that does not parse, with nothing on disk that verifies:
        // refused as damaged, and nothing written over it.
        fs::write(root.join("versions").join("1.0.0").join(FILES[1].0), b"THE SANDBOX HELPER").unwrap();
        fs::write(root.join("state.json"), "{not json").unwrap();
        assert!(damaged(&root).contains("does not read"));
        assert_eq!(fs::read(root.join("state.json")).unwrap(), b"{not json");
        // With an install that verifies, the launch rebuilds it and starts.
        fs::write(root.join("versions").join("1.0.0").join(FILES[1].0), FILES[1].1).unwrap();
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
        // Pre-releases, as semver orders them: below their release.
        assert_eq!(version_cmp("0.170.0-alpha.1", "0.170.0"), Less);
        assert_eq!(version_cmp("0.170.0-alpha.1", "0.169.0"), Greater);
        assert_eq!(version_cmp("1.0.0-alpha.2", "1.0.0-alpha.10"), Less);
        assert_eq!(version_cmp("1.0.0-alpha", "1.0.0-alpha.1"), Less);
        assert_eq!(version_cmp("1.0.0-alpha.1", "1.0.0-beta"), Less);
        assert_eq!(version_cmp("1.0.0-1", "1.0.0-alpha"), Less, "a number below a word");
        assert_eq!(version_cmp("1.0.0-rc.1", "1.0.0-rc.1"), Equal);
        assert_eq!(version_cmp("1.0.0+build.5", "1.0.0"), Equal, "build metadata does not count");
        // Numbers past any machine word, ordered as numbers.
        let huge = "1.0.0-alpha.18446744073709551616";
        assert_eq!(version_cmp(huge, "1.0.0-alpha.18446744073709551615"), Greater);
        assert_eq!(version_cmp(huge, "1.0.0-alpha.beta"), Less, "still a number, below a word");
        assert_eq!(version_cmp("99999999999999999999999.0.0", "2.0.0"), Greater);
        assert_eq!(version_cmp("1.007.0", "1.7.0"), Equal, "leading zeros do not count");
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
            Err(probe::Failed::Wrong("`exec` no longer lists --json".to_string()))
        };
        let e = pass_in(&root, BIN, "0", Ok("2.0.0"), &refuse, &mut |_: &str, dir: &Path| hashed(dir)).unwrap_err();
        assert!(e.contains("2.0.0 failed the offline probe: `exec` no longer lists --json"), "{e}");
        assert_eq!(*probed.borrow(), [root.join("versions").join("2.0.0").join(BIN)], "the exe where it would run from");
        assert_eq!(current_version_in(&root).as_deref(), Some("1.0.0"));
        assert_eq!(dirs(&root), ["1.0.0"], "the refused version went");
        assert!(bad_of(&root)["2.0.0"].contains("failed the offline probe"));

        let stayed = pass_in(&root, BIN, "0", Ok("2.0.0"), &refuse, &mut |_: &str, _: &Path| panic!("fetched again")).unwrap();
        assert!(stayed.unwrap().contains("2.0.0, the newest release, was turned away (failed the offline probe"));
        assert_eq!(probed.borrow().len(), 1, "nor probed again");
        assert_eq!(pass_at(&root, "0", "2.1.0"), Ok(None), "a newer one is taken");
        assert!(bad_of(&root).is_empty(), "and what it passed is forgotten");
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
                std::thread::sleep(Duration::from_millis(100));
            }
        };
        let fail = |version: &str, ids: [&str; 2]| {
            for id in ids {
                canary::session_started(Provider::Codex, &root, version, id);
                session_ended(id, true, || LastWords::of_screen(String::new()));
            }
        };
        fail("2.0.0", ["rollback-a", "rollback-b"]);
        told_with("Codex rolled back to 1.0.0");
        assert_eq!(current_version_in(&root).as_deref(), Some("1.0.0"), "rolled back");
        assert!(bad_of(&root)["2.0.0"].starts_with("rolled back: 2 of the 2 sessions started on Codex 2.0.0"));
        assert_eq!(previous_of(&root), None, "nothing but the failed one to go back to");
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
        assert!(!bad_of(&root).contains_key("1.0.0"));

        // A newer release: taken, and the old turning away forgotten.
        assert_eq!(pass_at(&root, "0", "3.0.0"), Ok(None));
        assert_eq!(current_version_in(&root).as_deref(), Some("3.0.0"));
        assert!(bad_of(&root).is_empty());
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
        let back = |floor: &str, from: &str| roll_back_in(&root, BIN, floor, from, "test").map_err(|e| e.to_string());
        assert!(back("0", "1.5.0").unwrap_err().contains("it is 2.0.0 now); look again"));
        assert!(back("1.5.0", "2.0.0").unwrap_err().contains("1.0.0, the one before, is older than 1.5.0"));
        // A helper of 1.0.0 altered at its size: sizes pass, the hash does not.
        let helper = root.join("versions").join("1.0.0").join(FILES[1].0);
        fs::write(&helper, b"THE SANDBOX HELPER").unwrap();
        assert!(previous_of(&root).is_some(), "intact by size");
        assert!(back("0", "2.0.0").unwrap_err().contains("no longer matches its sha256"));
        fs::write(&helper, FILES[1].1).unwrap();
        // A file its record does not name, added beside the helper: it would
        // run unchecked.
        let stray = root.join("versions").join("1.0.0").join("codex-resources").join("added.dll");
        fs::write(&stray, b"unchecked").unwrap();
        let why = back("0", "2.0.0").unwrap_err();
        assert!(why.contains("\"codex-resources/added.dll\" is in its folder but not in its record"), "{why}");
        fs::remove_file(&stray).unwrap();
        mark(&root, "1.0.0", "by hand in 0.19");
        assert!(back("0", "2.0.0").unwrap_err().contains("was turned away too"));
        assert_eq!(current_version_in(&root).as_deref(), Some("2.0.0"), "each refused, nothing moved");
        assert!(!bad_of(&root).contains_key("2.0.0"));

        // A record from before every file had a sha256: nothing to vouch for
        // it by.
        let legacy = scratch("rollback-legacy");
        update_in(&legacy, BIN, pointer("1.0.0"), &mut whole).unwrap();
        update_in(&legacy, BIN, pointer("2.0.0"), &mut whole).unwrap();
        let path = legacy.join("versions").join("1.0.0").join(RECORD);
        let mut record: Record = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        record.files.iter_mut().for_each(|f| f.sha256 = None);
        fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        assert!(roll_back_in(&legacy, BIN, "0", "2.0.0", "test").unwrap_err().to_string().contains("keeps no sha256"));
        let _ = remove_tree(&root);
        let _ = remove_tree(&legacy);
    }

    /// Roll back goes to the version the record keeps, turns the one left
    /// away as by a person, and a pass with the pointer still on it does not
    /// take it again; only a different release moves on.
    #[test]
    fn a_rollback_by_hand_turns_the_version_away_until_the_pointer_moves() {
        let root = scratch("rollback-hand");
        let by_hand = |root: &Path, from: &str| roll_back_by_hand_in(root, BIN, "0", from);
        update_in(&root, BIN, pointer("1.0.0"), &mut whole).unwrap();
        update_in(&root, BIN, pointer("2.0.0"), &mut whole).unwrap();
        let shown = versions_in(&root, BIN, "0");
        let open = shown.way_back.unwrap();
        assert_eq!((open.version.as_str(), open.refusal), ("1.0.0", None));
        assert!(
            shown.versions.iter().any(|r| r.current && r.version == "2.0.0")
                && shown.versions.iter().any(|r| r.previous && r.dir == open.dir)
        );

        assert_eq!(by_hand(&root, "2.0.0"), Ok(RolledBack { from: "2.0.0".into(), to: "1.0.0".into() }));
        assert_eq!(current_version_in(&root).as_deref(), Some("1.0.0"));
        let record = state::read_in(&root).unwrap();
        let marked = &record.bad["2.0.0"];
        assert_eq!((marked.why.as_str(), marked.by, marked.person), ("rolled back by hand", state::By::Rollback, true));
        assert_eq!(record.previous, None);
        assert!(versions_in(&root, BIN, "0").way_back.is_none(), "nothing kept to go back to now");
        assert!(by_hand(&root, "1.0.0").unwrap_err().contains("no version before it is kept"));
        assert!(by_hand(&root, "2.0.0").unwrap_err().contains("look again"), "a second click on the old screen");

        // The record as a Luna before the person's mark reads it (edec40c4's
        // shapes: three origins, no `person`): the mark is kept, not the
        // record taken for lost.
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        enum OldBy {
            Probe,
            Rollback,
            Earlier,
        }
        #[derive(serde::Deserialize)]
        struct OldMark {
            why: String,
            by: OldBy,
        }
        #[derive(serde::Deserialize)]
        struct OldState {
            current: Option<state::Slot>,
            bad: std::collections::BTreeMap<String, OldMark>,
        }
        let old: OldState = serde_json::from_slice(&fs::read(root.join("state.json")).unwrap()).unwrap();
        assert_eq!(old.current.unwrap().version, "1.0.0");
        assert!(matches!(old.bad["2.0.0"].by, OldBy::Rollback) && old.bad["2.0.0"].why == "rolled back by hand");

        // The pointer still on 2.0.0: not fetched, not taken again.
        let stayed = pass_in(&root, BIN, "0", Ok("2.0.0"), &passes_any, &mut |_: &str, _: &Path| panic!("fetched")).unwrap();
        assert!(stayed.unwrap().contains("was turned away (rolled back by hand"));
        assert_eq!(current_version_in(&root).as_deref(), Some("1.0.0"));
        // A different release is taken, and the mark forgotten.
        assert_eq!(pass_at(&root, "0", "3.0.0"), Ok(None));
        assert_eq!(current_version_in(&root).as_deref(), Some("3.0.0"));
        assert!(bad_of(&root).is_empty());
        let _ = remove_tree(&root);
    }

    /// Each refusal leaves the tree as it was, with no mark, and Settings'
    /// way back already says what it can see of it.
    #[test]
    fn a_rollback_by_hand_is_refused_where_the_automatic_one_would_be() {
        let root = scratch("rollback-hand-refused");
        let from = |floor: &str, from: &str| roll_back_by_hand_in(&root, BIN, floor, from).unwrap_err();
        assert!(from("0", "1.0.0").contains("nothing is current"), "nothing installed");
        update_in(&root, BIN, pointer("1.0.0"), &mut whole).unwrap();
        assert!(from("0", "1.0.0").contains("no version before it is kept"));
        assert!(versions_in(&root, BIN, "0").way_back.is_none());
        update_in(&root, BIN, pointer("2.0.0"), &mut whole).unwrap();
        let by_hand = |floor: &str| from(floor, "2.0.0");
        let refusal = |floor: &str| versions_in(&root, BIN, floor).way_back.unwrap().refusal;

        // A click on what a pass has moved on from: not what the person saw.
        assert!(from("0", "1.0.0").contains("the current version changed (it is 2.0.0 now); look again"));
        assert!(from("0", "9.9.9").contains("look again"));

        assert!(by_hand("1.5.0").contains("1.0.0, the one before, is older than 1.5.0"));
        assert!(refusal("1.5.0").unwrap().contains("older than 1.5.0"));

        let helper = root.join("versions").join("1.0.0").join(FILES[1].0);
        fs::write(&helper, b"THE SANDBOX HELPER").unwrap();
        assert!(by_hand("0").contains("no longer matches its sha256"), "sizes pass, the hash does not");
        assert_eq!(refusal("0"), None, "only the hashing sees it");
        fs::write(&helper, FILES[1].1).unwrap();

        let path = root.join("versions").join("1.0.0").join(RECORD);
        let mut kept: Record = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let hashes: Vec<_> = kept.files.iter().map(|f| f.sha256.clone()).collect();
        kept.files.iter_mut().for_each(|f| f.sha256 = None);
        fs::write(&path, serde_json::to_vec(&kept).unwrap()).unwrap();
        assert!(by_hand("0").contains("keeps no sha256"));
        assert!(refusal("0").unwrap().contains("keeps no sha256"));
        kept.files.iter_mut().zip(hashes).for_each(|(f, h)| f.sha256 = h);
        fs::write(&path, serde_json::to_vec(&kept).unwrap()).unwrap();

        mark(&root, "1.0.0", "failed the offline probe");
        assert!(by_hand("0").contains("1.0.0, the one before, was turned away too"));
        assert!(refusal("0").unwrap().contains("was turned away"));

        assert_eq!(current_version_in(&root).as_deref(), Some("2.0.0"), "each refused, nothing moved");
        assert_eq!(previous_of(&root).as_deref(), Some("1.0.0"));
        assert!(!bad_of(&root).contains_key("2.0.0"));
        let _ = remove_tree(&root);
    }

    /// The version rolled away from while a session held it is swept by a
    /// later pass once the hold is let go, though the pointer still names it
    /// and so the pass installs nothing.
    #[test]
    fn a_version_held_when_rolled_away_from_is_swept_by_a_later_pass() {
        let root = scratch("rollback-held");
        update_in(&root, BIN, pointer("1.0.0"), &mut whole).unwrap();
        update_in(&root, BIN, pointer("2.0.0"), &mut whole).unwrap();
        let session = in_use::hold(&root, "2.0.0").unwrap();
        roll_back_by_hand_in(&root, BIN, "0", "2.0.0").unwrap();
        assert_eq!(dirs(&root), ["1.0.0", "2.0.0"], "held: spared");
        let stay = |root: &Path| pass_in(root, BIN, "0", Ok("2.0.0"), &passes_any, &mut |_: &str, _: &Path| panic!("fetched"));
        assert!(stay(&root).unwrap().is_some());
        assert_eq!(dirs(&root), ["1.0.0", "2.0.0"], "still held");
        drop(session);
        assert!(stay(&root).unwrap().is_some());
        assert_eq!(dirs(&root), ["1.0.0"], "swept with the pointer unchanged");
        // A pass that could not read the pointer sweeps too.
        update_in(&root, BIN, pointer("3.0.0"), &mut whole).unwrap();
        let session = in_use::hold(&root, "3.0.0").unwrap();
        roll_back_by_hand_in(&root, BIN, "0", "3.0.0").unwrap();
        assert_eq!(dirs(&root), ["1.0.0", "3.0.0"]);
        drop(session);
        let _ = pass_in(&root, BIN, "0", Err("offline"), &passes_any, &mut |_: &str, _: &Path| panic!("fetched"));
        assert_eq!(dirs(&root), ["1.0.0"]);
        let _ = remove_tree(&root);
    }

    /// The person's rollback got there before the canary's: the canary's
    /// finds the version no longer current, notes nothing and tells nobody
    /// that it failed or will be retried.
    #[test]
    fn a_rollback_by_hand_ahead_of_the_canarys_leaves_nothing_to_announce() {
        let root = scratch("rollback-hand-canary");
        update_in(&root, BIN, pointer("7.0.0"), &mut hashed).unwrap();
        update_in(&root, BIN, pointer("8.0.0"), &mut hashed).unwrap();
        trip(&root);
        roll_back_by_hand_in(&root, BIN, "0", "8.0.0").unwrap();
        let late = roll_back_in(&root, BIN, "0", "8.0.0", "rolled back: late");
        assert!(matches!(late, Err(NoRollback::Moved(_))), "{late:?}");
        canary::settle_rollback(Provider::Codex, &root, "8.0.0", "seen", late);
        let told = crate::emit::sent_for_test("ui://notify");
        assert!(!told.iter().any(|n| n["title"].as_str().is_some_and(|t| t.contains("8.0.0"))), "{told:?}");
        let record = state::read_in(&root).unwrap();
        assert_eq!(record.canary.as_ref().map(|c| (c.version.as_str(), c.outcome.clone())), Some(("7.0.0", None)));
        assert_eq!(resume_in(Provider::Codex, &root, BIN, "0").map(|(_, _, r)| r), None, "nothing due for 7.0.0's fresh canary");
        assert_eq!(current_version_in(&root).as_deref(), Some("7.0.0"));
        let _ = remove_tree(&root);
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

    /// A tree Luna 0.19 left (its `current`, a one-line `previous`, a bad mark
    /// by hand, its channel, pins, marks good and rising floor, a canary the
    /// person kept with `dismissed`) starts as it was, migrates into the record
    /// at the first pass, its files gone, and moves on to the newest: the bad
    /// mark holds while the pointer names that version, and nothing rolls back
    /// from the canary of then.
    #[test]
    fn a_tree_from_0_19_starts_and_moves_on_to_the_newest() {
        let root = scratch("from-0-19");
        update_in(&root, BIN, pointer("1.0.0"), &mut hashed).unwrap();
        update_in(&root, BIN, pointer("2.0.0"), &mut hashed).unwrap();
        fs::remove_file(root.join("state.json")).unwrap();
        let left = [
            ("current", "2.0.0"),
            ("previous", "1.0.0"),
            ("recorded", "versions here are installed with completion records\n"),
            ("bad.json", r#"{"2.5.0":"marked bad by hand"}"#),
            ("canary.json", r#"{"version":"2.0.0","watched":3,"failures":["a","b"],"warned":true,"dismissed":true}"#),
            ("canary.lock", ""),
            ("channel", "known-good"),
            ("pinned", "1.0.0"),
            ("good.json", r#"{"1.0.0":"marked good by hand"}"#),
            ("marks.lock", ""),
            ("floor", "9.9.9"),
            ("signed-floor", "9.9.9"),
        ];
        for (name, text) in left {
            fs::write(root.join(name), text).unwrap();
        }
        assert!(launch_in(&root, BIN, "0").is_ok_and(|(_, v, _)| v == "2.0.0"), "it runs before anything is written");
        assert_eq!(previous_of(&root).as_deref(), Some("1.0.0"));
        assert!(resume_in(Provider::Codex, &root, BIN, "0").is_none(), "no rollback from the canary of then");

        let stayed = pass_at(&root, floor(Provider::Codex), "2.5.0").unwrap();
        assert!(stayed.unwrap().contains("marked bad by hand"));
        for (name, _) in left {
            assert!(!root.join(name).exists(), "{name}");
        }
        let record = state::read_in(&root).unwrap();
        assert_eq!((record.current.unwrap().dir, record.previous.unwrap().dir, record.canary), ("2.0.0".into(), "1.0.0".into(), None));
        assert_eq!(record.bad["2.5.0"].by, state::By::Earlier);
        // Above 9.9.9's floor of then or not, the newest release is taken.
        assert_eq!(pass_at(&root, floor(Provider::Codex), "3.0.0"), Ok(None));
        assert_eq!(current_version_in(&root).as_deref(), Some("3.0.0"));
        assert!(bad_of(&root).is_empty(), "the pointer moved past the marked version");
        let _ = remove_tree(&root);
    }

    /// A new version gets a canary of its own; Settings shows it while that
    /// version is current, and a rollback watches the one gone back to.
    #[test]
    fn each_flip_to_another_version_starts_its_canary() {
        let root = scratch("canary");
        update_in(&root, BIN, pointer("1.0.0"), &mut hashed).unwrap();
        update_in(&root, BIN, pointer("2.0.0"), &mut hashed).unwrap();
        canary::session_started(Provider::Codex, &root, "2.0.0", "canary-flip-a");
        session_ended("canary-flip-a", true, || LastWords::of_screen(String::new()));
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

    /// A flip that dies before the record's write leaves the record as it
    /// was; one that dies between the write and the sweep leaves it as it was
    /// to be, the old folders still on disk. Either way what runs is whole,
    /// its way back is on disk, and the next pass ends where an uncut one
    /// would have.
    #[test]
    fn a_flip_cut_short_leaves_one_of_its_two_states_and_the_next_pass_converges() {
        for at in ["state", "sweep"] {
            let root = scratch(&format!("flip-cut-{at}"));
            update_in(&root, BIN, pointer("1.0.0"), &mut hashed).unwrap();
            update_in(&root, BIN, pointer("2.0.0"), &mut hashed).unwrap();
            let before = state::read_in(&root).unwrap();
            let cut = cut_at(at, || update_in(&root, BIN, pointer("3.0.0"), &mut hashed));
            assert!(cut.unwrap_err().contains(&format!("stopped before {at}")), "{at}");
            let after = state::read_in(&root).unwrap();
            let names = |s: &state::State| (s.current.clone().unwrap().dir, s.previous.clone().map(|p| p.dir));
            match at {
                "state" => assert_eq!(after, before, "nothing written"),
                _ => assert_eq!(names(&after), ("3.0.0".into(), Some("2.0.0".into())), "written whole"),
            }
            assert!(after.named().iter().all(|d| root.join("versions").join(d).join(BIN).is_file()), "{at}: all it names is there");
            assert!(launch_in(&root, BIN, "0").is_ok(), "{at}");
            update_in(&root, BIN, pointer("3.0.0"), &mut hashed).unwrap();
            assert_eq!(names(&state::read_in(&root).unwrap()), ("3.0.0".into(), Some("2.0.0".into())), "{at}");
            assert_eq!(dirs(&root), ["2.0.0", "3.0.0"], "{at}: swept");
            let _ = remove_tree(&root);
        }
    }

    /// A rollback that dies before its write leaves the trip recorded and
    /// nothing else, which the next core takes up (`resume_in`); one that
    /// dies between the write and the sweep leaves it done. Either way the
    /// failed version ends turned away and the one before current.
    #[test]
    fn a_rollback_cut_short_is_finished_or_done_and_the_next_core_converges() {
        for at in ["state", "sweep"] {
            let root = scratch(&format!("rollback-cut-{at}"));
            update_in(&root, BIN, pointer("1.0.0"), &mut hashed).unwrap();
            update_in(&root, BIN, pointer("2.0.0"), &mut hashed).unwrap();
            trip(&root);
            let cut = cut_at(at, || roll_back_in(&root, BIN, "0", "2.0.0", "rolled back: test").map_err(|e| e.to_string()));
            assert!(cut.unwrap_err().contains(&format!("stopped before {at}")), "{at}");
            let resumed = resume_in(Provider::Codex, &root, BIN, "0");
            match at {
                "state" => assert_eq!(resumed.map(|(v, _, r)| (v, r)), Some(("2.0.0".into(), Ok("1.0.0".into())))),
                _ => assert!(resumed.is_none(), "done already"),
            }
            assert_eq!(current_version_in(&root).as_deref(), Some("1.0.0"), "{at}");
            assert!(bad_of(&root).contains_key("2.0.0") && previous_of(&root).is_none(), "{at}");
            assert!(resume_in(Provider::Codex, &root, BIN, "0").is_none(), "{at}: finished");
            prune(&root);
            assert_eq!(dirs(&root), ["1.0.0"], "{at}");
            let _ = remove_tree(&root);
        }
    }

    /// A flip away from a version whose canary tripped (its rollback still
    /// to come) keeps the one before it to go back to: the failed version is
    /// never made the way back.
    #[test]
    fn a_flip_away_from_a_tripped_version_keeps_the_one_before_it() {
        let root = scratch("flip-from-tripped");
        update_in(&root, BIN, pointer("1.0.0"), &mut hashed).unwrap();
        update_in(&root, BIN, pointer("2.0.0"), &mut hashed).unwrap();
        trip(&root);
        update_in(&root, BIN, pointer("3.0.0"), &mut hashed).unwrap();
        assert_eq!(previous_of(&root).as_deref(), Some("1.0.0"));
        assert!(resume_in(Provider::Codex, &root, BIN, "0").is_none(), "nothing left to roll back from");
        assert_eq!(dirs(&root), ["1.0.0", "3.0.0"], "2.0.0 swept");
        let _ = remove_tree(&root);
    }

    /// The window's pass flipping to 3.0.0 and the core rolling back from a
    /// tripped 2.0.0 at once, on two threads, each under the record's lock:
    /// whichever goes first, the record ends with 3.0.0 current, 1.0.0 to go
    /// back to, and every folder it names on disk.
    #[test]
    fn a_pass_and_a_rollback_at_once_end_in_one_record() {
        for round in 0..8 {
            let root = scratch(&format!("race-{round}"));
            update_in(&root, BIN, pointer("1.0.0"), &mut hashed).unwrap();
            update_in(&root, BIN, pointer("2.0.0"), &mut hashed).unwrap();
            trip(&root);
            let (a, b) = (root.clone(), root.clone());
            let pass = std::thread::spawn(move || pass_at(&a, "0", "3.0.0"));
            let back = std::thread::spawn(move || roll_back_in(&b, BIN, "0", "2.0.0", "rolled back: race"));
            let (passed, _rolled) = (pass.join().unwrap(), back.join().unwrap());

            assert_eq!(passed, Ok(None), "round {round}");
            let record = state::read_in(&root).unwrap();
            assert_eq!(record.current.as_ref().map(|c| c.dir.as_str()), Some("3.0.0"), "round {round}");
            assert_eq!(record.previous.as_ref().map(|p| p.dir.as_str()), Some("1.0.0"), "round {round}");
            assert!(record.named().iter().all(|d| root.join("versions").join(d).join(BIN).is_file()), "round {round}");
            assert!(resume_in(Provider::Codex, &root, BIN, "0").is_none(), "round {round}");
            let _ = remove_tree(&root);
        }
    }

    /// The probe not run to its end (out of time, its scratch home not to
    /// be had) says nothing of the version: nothing turned away, and the
    /// next pass takes up the same install without fetching it again.
    #[test]
    fn a_probe_that_could_not_ask_turns_nothing_away() {
        let root = scratch("probe-unasked");
        update_in(&root, BIN, pointer("1.0.0"), &mut hashed).unwrap();
        let late = |_: &Path, _: &str| -> Result<(), probe::Failed> {
            Err(probe::Failed::Unasked("codex --version did not finish within 30s".into()))
        };
        let e = pass_in(&root, BIN, "0", Ok("2.0.0"), &late, &mut |_: &str, dir: &Path| hashed(dir)).unwrap_err();
        assert!(e.contains("2.0.0 could not be probed") && e.contains("the next pass tries again"), "{e}");
        assert!(bad_of(&root).is_empty());
        assert_eq!(pass_in(&root, BIN, "0", Ok("2.0.0"), &passes_any, &mut |_: &str, _: &Path| panic!("fetched again")), Ok(None));
        assert_eq!(current_version_in(&root).as_deref(), Some("2.0.0"));
        let _ = remove_tree(&root);
    }

    /// An install a pass left between its promotion and its flip is held to
    /// its record's sha256 before it is made current, and fetched afresh
    /// beside it where a file no longer matches.
    #[test]
    fn a_promoted_install_is_held_to_its_hashes_before_it_is_made_current() {
        let root = scratch("promoted");
        update_in(&root, BIN, pointer("1.0.0"), &mut hashed).unwrap();
        drop(install_in(&root, BIN, "2.0.0", &mut hashed).unwrap());
        let (dir, _) = update_in(&root, BIN, pointer("2.0.0"), &mut |_: &Path| panic!("fetched again")).unwrap();
        assert_eq!(dir, "2.0.0", "whole: taken as it is");

        let altered = scratch("promoted-altered");
        update_in(&altered, BIN, pointer("1.0.0"), &mut hashed).unwrap();
        drop(install_in(&altered, BIN, "2.0.0", &mut hashed).unwrap());
        fs::write(altered.join("versions").join("2.0.0").join(FILES[1].0), b"THE SANDBOX HELPER").unwrap();
        let fetched = Cell::new(0);
        let (dir, _) = update_in(&altered, BIN, pointer("2.0.0"), &mut |d: &Path| {
            fetched.set(fetched.get() + 1);
            hashed(d)
        })
        .unwrap();
        assert_eq!((dir.as_str(), fetched.get()), ("2.0.0~1", 1), "altered at its size: fetched afresh beside it");

        // A file its record does not name: the same.
        let added = scratch("promoted-added");
        update_in(&added, BIN, pointer("1.0.0"), &mut hashed).unwrap();
        drop(install_in(&added, BIN, "2.0.0", &mut hashed).unwrap());
        fs::write(added.join("versions").join("2.0.0").join("added.dll"), b"unchecked").unwrap();
        let (dir, _) = update_in(&added, BIN, pointer("2.0.0"), &mut hashed).unwrap();
        assert_eq!(dir, "2.0.0~1", "a stray file: fetched afresh beside it");
        let _ = remove_tree(&root);
        let _ = remove_tree(&altered);
        let _ = remove_tree(&added);
    }

    /// The core's rollback, and its sweep, while the window's pass probes a
    /// new version: the version being probed is held, so the sweep leaves it
    /// and the probe finds it whole.
    #[test]
    fn a_sweep_meanwhile_leaves_the_version_being_probed() {
        let root = scratch("probed-swept");
        update_in(&root, BIN, pointer("1.0.0"), &mut hashed).unwrap();
        update_in(&root, BIN, pointer("2.0.0"), &mut hashed).unwrap();
        let probe = |exe: &Path, _: &str| -> Result<(), probe::Failed> {
            roll_back_in(&root, BIN, "0", "2.0.0", "rolled back: in the core").unwrap();
            match exe.is_file() {
                true => Ok(()),
                false => Err(probe::Failed::Unasked("gone from under the probe".into())),
            }
        };
        assert_eq!(pass_in(&root, BIN, "0", Ok("3.0.0"), &probe, &mut |_: &str, dir: &Path| hashed(dir)), Ok(None));
        assert_eq!(current_version_in(&root).as_deref(), Some("3.0.0"));
        assert_eq!(previous_of(&root).as_deref(), Some("1.0.0"), "back to what the rollback left current");
        assert_eq!(bad_of(&root).keys().collect::<Vec<_>>(), ["2.0.0"], "until the next pointer read names another");
        let _ = remove_tree(&root);
    }

    /// The core turns away the very version the pass is about to make
    /// current, after the pass chose it: the flip, a change of the record
    /// under its lock, sees it and refuses.
    #[test]
    fn a_version_turned_away_meanwhile_is_not_made_current() {
        let root = scratch("meanwhile");
        update_in(&root, BIN, pointer("1.0.0"), &mut hashed).unwrap();
        let probe = |_: &Path, version: &str| -> Result<(), probe::Failed> {
            mark(&root, version, "rolled back in the core");
            Ok(())
        };
        let e = pass_in(&root, BIN, "0", Ok("2.0.0"), &probe, &mut |_: &str, dir: &Path| hashed(dir)).unwrap_err();
        assert!(e.contains("2.0.0 was turned away meanwhile (rolled back in the core)"), "{e}");
        assert_eq!(current_version_in(&root).as_deref(), Some("1.0.0"));
        assert_eq!(dirs(&root), ["1.0.0"], "and swept");
        let _ = remove_tree(&root);
    }

    /// A tree from before records runs its version without one: no step
    /// down from it either.
    #[test]
    fn no_step_down_from_a_version_from_before_records() {
        let root = scratch("legacy-step");
        write_files(&root.join("versions").join("3.0.0"), &FILES[..1]);
        fs::write(root.join("current"), "3.0.0").unwrap();
        let stayed = pass_at(&root, "0", "2.0.0").unwrap();
        assert!(stayed.unwrap().contains("older than 3.0.0, which runs here"));
        assert_eq!(pass_at(&root, "0", "3.0.0"), Ok(None), "its own version: fetched again beside it, recorded");
        assert!(current_in(&root, BIN).is_some_and(|c| c.recorded && c.dir == "3.0.0~1"));
        let _ = remove_tree(&root);
    }

    /// The review's case: the core's sweep after a rollback has read the
    /// record, the window then promotes 3.0.0, flips to it and lets go of its
    /// hold, all before the sweep reaches that folder. The sweep judges the
    /// folder by the record read once it holds it, and spares it.
    #[test]
    fn a_sweep_spares_a_folder_a_flip_named_after_it_began() {
        let root = scratch("sweep-recheck");
        update_in(&root, BIN, pointer("1.0.0"), &mut hashed).unwrap();
        drop(install_in(&root, BIN, "3.0.0", &mut hashed).unwrap());
        let flipper = root.clone();
        BEFORE_SWEEP.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |name: &str| {
                if name == "3.0.0" {
                    flip_in(&flipper, "3.0.0", "3.0.0").unwrap();
                }
            }))
        });
        prune(&root);
        BEFORE_SWEEP.with(|hook| *hook.borrow_mut() = None);
        assert_eq!(current_version_in(&root).as_deref(), Some("3.0.0"));
        assert_eq!(dirs(&root), ["1.0.0", "3.0.0"], "the folder now current was spared");
        assert!(launch_in(&root, BIN, "0").is_ok_and(|(_, v, _)| v == "3.0.0"));
        let _ = remove_tree(&root);
    }

    /// A record lost or garbled strands no install: the newest version on
    /// disk at or above the floor whose files still match their sha256 is
    /// made current again, the next one the way back, and nothing is swept
    /// while there is no record to go by. Marks go with the record.
    #[test]
    fn a_lost_or_garbled_record_is_rebuilt_from_the_versions_on_disk() {
        for garbled in [false, true] {
            let root = scratch(&format!("rebuild-{garbled}"));
            update_in(&root, BIN, pointer("1.0.0"), &mut hashed).unwrap();
            update_in(&root, BIN, pointer("2.0.0"), &mut hashed).unwrap();
            mark(&root, "2.5.0", "rolled back");
            match garbled {
                true => fs::write(root.join("state.json"), "{not json").unwrap(),
                false => fs::remove_file(root.join("state.json")).unwrap(),
            }
            prune(&root);
            assert_eq!(dirs(&root), ["1.0.0", "2.0.0"], "nothing swept without a record");
            // A launch rebuilds it, offline as it may be.
            assert!(launch_in(&root, BIN, "0").is_ok_and(|(_, v, _)| v == "2.0.0"), "{garbled}");
            assert_eq!(previous_of(&root).as_deref(), Some("1.0.0"));
            assert!(bad_of(&root).is_empty(), "marks are lost with the record");
            let _ = remove_tree(&root);
        }

        // A version whose files no longer match is not taken back; below the
        // floor neither.
        let root = scratch("rebuild-unverified");
        update_in(&root, BIN, pointer("1.0.0"), &mut hashed).unwrap();
        update_in(&root, BIN, pointer("2.0.0"), &mut hashed).unwrap();
        fs::remove_file(root.join("state.json")).unwrap();
        fs::write(root.join("versions").join("2.0.0").join(FILES[1].0), b"THE SANDBOX HELPER").unwrap();
        assert!(recover_in(&root, BIN, "0"));
        assert_eq!((current_version_in(&root).as_deref(), previous_of(&root)), (Some("1.0.0"), None));
        fs::remove_file(root.join("state.json")).unwrap();
        assert!(!recover_in(&root, BIN, "1.5.0"), "nothing at or above the floor verifies");
        assert!(!root.join("state.json").exists(), "and nothing is written");
        // Offline, nothing written either; online, the pass installs afresh.
        let offline = pass_in(&root, BIN, "1.5.0", Err("offline"), &passes_any, &mut |_: &str, _: &Path| panic!("fetched"));
        assert_eq!(offline, Err("offline".into()));
        assert!(!root.join("state.json").exists());
        assert_eq!(pass_at(&root, "1.5.0", "3.0.0"), Ok(None));
        assert_eq!(current_version_in(&root).as_deref(), Some("3.0.0"));
        let _ = remove_tree(&root);
    }

    /// The version the rollback is to go back to, turned away by the window
    /// while the core hashed it: the rollback's write sees it and refuses,
    /// nothing changed. A rollback whose write fails is not noted, and the
    /// next core's start makes it.
    #[test]
    fn a_rollback_holds_to_the_record_in_its_write_and_a_failed_write_is_tried_again() {
        let root = scratch("rollback-recheck");
        update_in(&root, BIN, pointer("1.0.0"), &mut hashed).unwrap();
        update_in(&root, BIN, pointer("2.0.0"), &mut hashed).unwrap();
        trip(&root);
        let marker = root.clone();
        let refused = {
            // Between the hash and the write: the window turns 1.0.0 away.
            BEFORE_ROLLBACK_WRITE
                .with(|hook| *hook.borrow_mut() = Some(Box::new(move || mark(&marker, "1.0.0", "failed the offline probe"))));
            let out = roll_back_in(&root, BIN, "0", "2.0.0", "rolled back: test");
            BEFORE_ROLLBACK_WRITE.with(|hook| *hook.borrow_mut() = None);
            out
        };
        assert!(matches!(refused, Err(NoRollback::Refused(ref why)) if why.contains("turned away meanwhile")), "{refused:?}");
        assert_eq!(current_version_in(&root).as_deref(), Some("2.0.0"));
        let _ = remove_tree(&root);

        let root = scratch("rollback-retry");
        update_in(&root, BIN, pointer("1.0.0"), &mut hashed).unwrap();
        update_in(&root, BIN, pointer("2.0.0"), &mut hashed).unwrap();
        trip(&root);
        let failed = cut_at("state", || roll_back_in(&root, BIN, "0", "2.0.0", "rolled back: test"));
        assert!(matches!(failed, Err(NoRollback::Failed(_))), "{failed:?}");
        canary::settle_rollback(Provider::Codex, &root, "2.0.0", "seen", failed);
        assert_eq!(state::read_in(&root).unwrap().canary.unwrap().outcome, None, "not noted");
        assert_eq!(resume_in(Provider::Codex, &root, BIN, "0").map(|(_, _, r)| r), Some(Ok("1.0.0".into())));
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
