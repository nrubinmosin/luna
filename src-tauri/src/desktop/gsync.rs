//! G-SYNC off for Luna's own exe, in NVIDIA's driver profiles.
//!
//! With G-SYNC on for windowed apps, or Luna's window covering its monitor,
//! the monitor's refresh follows Luna's presents. Luna draws only when
//! something changes, so the refresh falls to the panel's VRR floor and
//! everything on that monitor stutters. No DXGI, Direct3D or NVAPI call lets
//! a window opt out at run time. The driver's own way is its profile
//! database: browsers, Explorer, Office and Windows Terminal are left out by
//! the profiles NVIDIA ships for their exes, which set "G-SYNC - Application
//! State" (`VRR_APP_OVERRIDE`, 0x10A879CF) to Force Off (1).
//!
//! So the window, as it starts on Windows with an NVIDIA driver, makes a
//! profile of Luna's own, once, and never touches one it did not make: a
//! profile the driver resolves the exe to by its bare name (NVIDIA ships one
//! named "Luna" for any `luna.exe`, a game's) applies to every exe of that
//! name, so a change there would reach them all. Luna's profile has a name
//! no one else's has (`PREFIX` and a random tail), holds the running exe by
//! its full path, which the driver matches before a bare name, and sets that
//! one setting to Force Off. Luna's application entries carry a marker of
//! their own, random, in their friendly name (`Entry::lunas`), and are
//! otherwise bare (no launcher, folder file or command line): a profile is
//! taken for Luna's only while it is not predefined and holds such an entry
//! of Luna's for a path Luna recorded, and only such entries are ever taken
//! for Luna's, whatever their path. Before saving, the session is asked
//! again which profile the exe's full path resolves to; anything but Luna's
//! entry, and nothing is saved. The driver takes no entry, a full path's
//! included, for an exe whose name a profile has as a bare name
//! (`NVAPI_EXECUTABLE_ALREADY_IN_USE` from `NvAPI_DRS_CreateApplication`,
//! whatever the entry's launcher, file in the folder or command line; seen
//! on driver 621.24): with NVIDIA's "Luna" there, a `luna.exe` gets no
//! profile of Luna's. Where the profile that holds the name is the one the
//! driver applies to the exe, the window asks the person whether to set
//! Force Off in that profile instead (`Outcome::Ask`), saying it reaches
//! every exe of that name; else the refusal is said (`refused_entry`).
//!
//! On a yes (`turn_off`) that one setting is set in that profile, and only
//! where the exe still resolves to it by its bare name and the setting is
//! not Force Off already: no entry, no other setting, no profile made or
//! removed. What it was before, the profile's own value or the driver's
//! (a predefined value, or none and the global profile's), goes in
//! `<data>/nvidia-setting.json` (`SetRecord`), written ahead of the save
//! and confirmed after it, under the same lock and the same rules as the
//! record of Luna's profile. Luna then never sets it there again, so a
//! later choice in the NVIDIA Control Panel stands. The undo puts it back
//! only while it still has Luna's value: the driver's default restored
//! (`NvAPI_DRS_RestoreProfileDefaultSetting`) or the person's earlier
//! value set again; a value changed since is left as it is.
//!
//! What Luna made is in `<data>/nvidia-profile.json` (`Record`), written
//! ahead of every save and confirmed after it, under a lock
//! (`nvidia-profile.lock`) that the window's start and the uninstaller's
//! undo both hold, so neither reads the driver's database while the other
//! changes it. A start that finds a save still pending reads the database
//! to learn whether it went through; one that finds no record, where the
//! database has Luna's entry for the exe (the record's rename undone by a
//! power cut), takes that profile back. With the record confirmed for the
//! running exe nothing is written again, so a later choice in the NVIDIA
//! Control Panel stands; an exe that moved (a portable copy) gets its new
//! path added to the same profile. The uninstaller's `luna --nvidia-undo`
//! (not an update's) removes Luna's marked entries from that profile and
//! from the one the exe resolves to, the profile whole where it holds only
//! what Luna put there, and says so where it cannot tell it removed all of
//! them. A record is acted on only where it is a private file in the data
//! folder owned by this user's own SID, neither it nor the folder a link
//! (`FileStore`), of this build's flavor, naming a profile of Luna's naming;
//! a Luna run as administrator leaves the profiles alone. Where the driver
//! refuses (it may ask for
//! administrator rights, `NVAPI_INVALID_USER_PRIVILEGE`) or anything else
//! fails, the window logs one line, naming the NVAPI call and its status
//! (`Fault`), and shows a one-line hint pointing to the user guide's Fixed
//! Refresh setting; every call a session made, with its status, is logged
//! too (`Trail`). The test build only reads, unless
//! `LUNA_TEST_NVIDIA=write` (a live test's), and then under a name of its
//! own.
//!
//! NVAPI is the driver's library, loaded at run time from System32
//! (`nvapi64.dll`, through its one export, `nvapi_QueryInterface`), no SDK
//! linked: the interface ids, status values and structure layouts below are
//! those of NVIDIA's MIT-licensed headers (github.com/NVIDIA/nvapi:
//! nvapi.h, nvapi_interface.h, nvapi_lite_common.h, NvApiDriverSettings.h).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub use nvapi::Fault;

/// "G-SYNC - Application State" (`VRR_APP_OVERRIDE_ID`, "G-SYNC").
pub const VRR_APP_OVERRIDE: u32 = 0x10A8_79CF;
/// Its "Force Off" (`VRR_APP_OVERRIDE_FORCE_OFF`).
pub const FORCE_OFF: u32 = 1;
/// The start of the name of the profile Luna makes; a space and eight hex
/// digits, random, follow, so neither NVIDIA's profiles, a person's nor
/// another copy of Luna's share it. Only characters seen in names the
/// driver takes: driver 621.24 refuses a comma (`NvAPI_DRS_CreateProfile`,
/// `NVAPI_ERROR`).
const PREFIX: &str = "Luna native window (G-SYNC off)";
/// The release's earlier prefix. The driver never let a profile of this
/// name be made (the comma), so it is here for records alone: one naming it
/// is still of Luna's naming, for the undo and a pending save to read.
const COMMA_PREFIX: &str = "Luna (native window, G-SYNC off)";
/// The test build's, so that its live test and a Luna in use never act on
/// each other's.
const TEST_PREFIX: &str = "Luna test build (G-SYNC off)";
/// What Luna made, in its data folder, for the undo.
const RECORD: &str = "nvidia-profile.json";
/// What Luna set in a profile not of its making, on the person's yes.
const SET_RECORD: &str = "nvidia-setting.json";
/// Held while the record and the driver's database are read and changed.
const LOCK: &str = "nvidia-profile.lock";
/// Far above any record Luna writes (a few hundred bytes).
const RECORD_MAX: u64 = 64 << 10;
/// The longest name NVAPI takes: `NvAPI_UnicodeString` less its terminator.
const NAME_MAX: usize = 2047;
/// How long the undo waits for a window's start to let go of the lock.
const UNDO_WAIT: Duration = Duration::from_secs(30);

fn prefix(test: bool) -> &'static str {
    if test {
        TEST_PREFIX
    } else {
        PREFIX
    }
}

/// A new name for Luna's profile, and the marker of its entries.
pub fn new_profile(test: bool) -> (String, String) {
    (format!("{} {}", prefix(test), crate::secret::random_hex(4)), crate::secret::random_hex(8))
}

fn hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Whether `name` is one `new_profile` makes, or a release's under
/// `COMMA_PREFIX`.
fn ours(name: &str, test: bool) -> bool {
    let named = |prefix: &str| name.strip_prefix(prefix).and_then(|tail| tail.strip_prefix(' ')).is_some_and(|tail| hex(tail, 8));
    named(prefix(test)) || (!test && named(COMMA_PREFIX))
}

/// Two application names for one exe: the driver keeps them in its own
/// case and with either slash.
pub fn same_app(a: &str, b: &str) -> bool {
    let key = |s: &str| s.replace('\\', "/").to_lowercase();
    key(a) == key(b)
}

/// An exe's full path as nvapi.h writes one (`c:/Folder1/Folder2/App.exe`).
/// None where NVAPI would be asked about another path than the exe's: one
/// that is no Unicode (an unpaired surrogate, which a lossy conversion would
/// change) or longer than an `NvAPI_UnicodeString` holds (which would cut it).
pub fn app_path(exe: &Path) -> Option<String> {
    let path = exe.to_str()?.replace('\\', "/");
    (path.encode_utf16().count() <= NAME_MAX).then_some(path)
}

/// An application entry of a profile, as NVAPI gives it
/// (`NVDRS_APPLICATION_V4`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub app: String,
    pub friendly: String,
    pub predefined: bool,
    pub launcher: String,
    pub file_in_folder: String,
    /// `isMetro`, `isCommandLine`.
    pub flags: u32,
    pub command_line: String,
}

impl Entry {
    /// The entry Luna makes for `app`: its marker in the friendly name, the
    /// rest bare.
    pub fn lunas(app: &str, marker: &str) -> Entry {
        Entry {
            app: app.to_owned(),
            friendly: format!("Luna ({marker})"),
            predefined: false,
            launcher: String::new(),
            file_in_folder: String::new(),
            flags: 0,
            command_line: String::new(),
        }
    }

    /// The marker of an entry of Luna's making, whatever its path: one of
    /// its shape, no field of it but these two set.
    fn marker(&self) -> Option<&str> {
        let marker = self.friendly.strip_prefix("Luna (")?.strip_suffix(')')?;
        let bare = !self.predefined
            && self.launcher.is_empty()
            && self.file_in_folder.is_empty()
            && self.flags == 0
            && self.command_line.is_empty();
        (bare && hex(marker, 16)).then_some(marker)
    }

    /// Whether this is the entry Luna made for `app` with `marker`.
    fn is_lunas(&self, app: &str, marker: &str) -> bool {
        same_app(&self.app, app) && self.marker() == Some(marker)
    }
}

/// What Luna made in the driver's profiles (`<data>/nvidia-profile.json`).
/// The record 0.20.1's development builds wrote before the marker (only
/// test builds ran that code, and they only read) does not parse, and is
/// refused like any other: there is none to carry over.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Record {
    /// The profile Luna made (`new_profile`).
    pub profile: String,
    /// The marker of Luna's entries in it (`Entry::lunas`).
    pub marker: String,
    /// Written by the test build: only it acts on it, and it on no other.
    pub test: bool,
    /// Whether the profile is known to be in the driver's database.
    pub created: bool,
    /// The exes, by full path, known to be in it by Luna's doing.
    pub apps: Vec<String>,
    /// The exe about to be saved into it (with `created` false, the profile
    /// and its setting too): written before the save, folded into the above
    /// after it.
    pub pending: Option<String>,
}

impl Record {
    /// Everything Luna may have put in the profile.
    fn entries(&self) -> impl Iterator<Item = &String> {
        self.apps.iter().chain(&self.pending)
    }

    /// Whether `held` is the profile this record names, Luna's own: not
    /// predefined, with an entry of Luna's making in it (its marker,
    /// whatever its path).
    fn owns(&self, held: &Held) -> bool {
        !held.predefined && held.apps.iter().any(|e| e.marker() == Some(self.marker.as_str()))
    }
}

/// What Luna set, on the person's yes, in the profile that holds the exe's
/// name (`<data>/nvidia-setting.json`): `VRR_APP_OVERRIDE` at Force Off
/// there, and what it was before, for the undo.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetRecord {
    /// The profile, by its name.
    pub profile: String,
    /// Its own value before; None where the driver's applied there (the
    /// profile's predefined value, or none and the global profile's).
    pub previous: Option<u32>,
    /// Written by the test build: only it acts on it, and it on no other.
    pub test: bool,
    /// Written before the save, false once it went through.
    pub pending: bool,
}

/// Why the setting's `record` is not acted on, for a build of the `test`
/// flavor.
pub fn set_refusal(record: &SetRecord, test: bool) -> Option<String> {
    if record.test != test {
        let by = if record.test { "the test build" } else { "a release" };
        return Some(format!("{SET_RECORD} was written by {by}"));
    }
    if record.profile.is_empty() || record.profile.encode_utf16().count() > NAME_MAX {
        return Some(format!("{SET_RECORD} names no profile NVAPI takes"));
    }
    if record.previous == Some(FORCE_OFF) {
        return Some(format!("{SET_RECORD} says the setting was Force Off before Luna set it"));
    }
    None
}

/// Why `record` is not acted on, for a build of the `test` flavor.
pub fn refusal(record: &Record, test: bool) -> Option<String> {
    if record.test != test {
        let by = if record.test { "the test build" } else { "a release" };
        return Some(format!("{RECORD} was written by {by}"));
    }
    if !ours(&record.profile, test) || !hex(&record.marker, 16) {
        return Some(format!("{RECORD} names a profile Luna does not make (\"{}\")", record.profile));
    }
    if !record.created && record.pending.is_none() {
        return Some(format!("{RECORD} names neither a profile made nor one being made"));
    }
    if record.entries().any(|app| app.encode_utf16().count() > NAME_MAX) {
        return Some(format!("{RECORD} names a path longer than NVAPI takes"));
    }
    None
}

/// Where the record is kept.
pub trait Store {
    /// The record, None where there is none; why not, where one is there
    /// but may not be read.
    fn load(&self) -> Result<Option<Record>, String>;
    fn save(&self, record: &Record) -> Result<(), String>;
    fn clear(&self) -> Result<(), String>;
    /// The same for `SetRecord`.
    fn load_set(&self) -> Result<Option<SetRecord>, String>;
    fn save_set(&self, record: &SetRecord) -> Result<(), String>;
    fn clear_set(&self) -> Result<(), String>;
}

/// The records in the data folder `0`. Acted on only where the folder is no
/// link or junction (one would bring a record from elsewhere) and the
/// record a private file of one name this user's own SID holds, opened
/// without following a link (`secret::read_own_private`); written on disk
/// before it is in place, as the save it precedes is.
pub struct FileStore(pub PathBuf);

impl FileStore {
    pub fn in_data(data: &Path) -> FileStore {
        FileStore(data.to_path_buf())
    }

    fn path(&self) -> PathBuf {
        self.0.join(RECORD)
    }

    fn set_path(&self) -> PathBuf {
        self.0.join(SET_RECORD)
    }

    fn read<T: serde::de::DeserializeOwned>(&self, name: &str) -> Result<Option<T>, String> {
        self.folder()?;
        let Some(bytes) = crate::secret::read_own_private(&self.0.join(name), RECORD_MAX).map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        serde_json::from_slice(&bytes).map(Some).map_err(|e| format!("{name} does not read ({e})"))
    }

    fn write<T: Serialize>(&self, name: &str, record: &T) -> Result<(), String> {
        self.folder()?;
        let bytes = serde_json::to_vec_pretty(record).map_err(|e| e.to_string())?;
        crate::secret::write_private_synced(&self.0.join(name), &bytes).map_err(|e| format!("{name} was not written ({e})"))
    }

    fn remove(&self, name: &str) -> Result<(), String> {
        self.folder()?;
        match std::fs::remove_file(self.0.join(name)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("{name} was not removed ({e})")),
            _ => Ok(()),
        }
    }

    /// The folder, as `FileStore` takes it.
    fn folder(&self) -> Result<(), String> {
        match std::fs::symlink_metadata(&self.0) {
            Ok(meta) if crate::boundary::is_link(&meta) => {
                Err(format!("{} is a link or junction: {RECORD} is not read there", self.0.display()))
            }
            Ok(meta) if !meta.is_dir() => Err(format!("{} is not a folder", self.0.display())),
            Ok(_) => Ok(()),
            Err(e) => Err(format!("{} does not read ({e})", self.0.display())),
        }
    }

    /// The lock the window's start and the undo hold over the record and
    /// the driver's database, waited for up to `wait` (None: as long as it
    /// takes). Released when the file is closed, a process that died
    /// included. A lock file that `release` removed while this waited for
    /// it is not the lock any more: it is opened afresh (`is_at`).
    pub fn lock(&self, wait: Option<Duration>) -> Result<std::fs::File, String> {
        self.folder()?;
        let path = self.0.join(LOCK);
        let until = wait.map(|wait| Instant::now() + wait);
        let more = || until.is_none_or(|until| Instant::now() < until);
        let pause = || std::thread::sleep(Duration::from_millis(100));
        loop {
            let file = match std::fs::File::options().create(true).truncate(false).write(true).open(&path) {
                Ok(file) => file,
                // A `release` has it open to delete it, for a moment.
                Err(e) if released_now(&e) && more() => {
                    pause();
                    continue;
                }
                Err(e) => return Err(format!("{LOCK} does not open ({e})")),
            };
            if until.is_none() {
                file.lock().map_err(|e| format!("{LOCK} is not taken ({e})"))?;
            } else {
                match file.try_lock() {
                    Ok(()) => {}
                    Err(std::fs::TryLockError::WouldBlock) if more() => {
                        pause();
                        continue;
                    }
                    Err(std::fs::TryLockError::WouldBlock) => return Err("another Luna is changing its NVIDIA profile".into()),
                    Err(std::fs::TryLockError::Error(e)) => return Err(format!("{LOCK} is not taken ({e})")),
                }
            }
            if is_at(&file, &path) {
                return Ok(file);
            }
        }
    }

    /// Lets go of `lock`, and removes the lock file where nothing is left
    /// for it to guard: neither record (and `keep` false: the undo keeps it where
    /// it may have left something), and no other process with it open,
    /// waiting for its turn. On Windows the file is opened to be deleted as
    /// it closes, with no sharing, which fails while any other handle is
    /// open; on Unix it is unlinked under the lock, and a waiter that then
    /// takes it finds it gone (`is_at`) and opens a new one.
    pub fn release(&self, lock: std::fs::File, keep: bool) {
        let path = self.0.join(LOCK);
        let unguarded = !keep && !self.path().exists() && !self.set_path().exists();
        #[cfg(windows)]
        {
            drop(lock);
            if unguarded {
                use std::os::windows::fs::OpenOptionsExt;
                const DELETE: u32 = 0x0001_0000;
                const FILE_FLAG_DELETE_ON_CLOSE: u32 = 0x0400_0000;
                let _ = std::fs::OpenOptions::new().access_mode(DELETE).share_mode(0).custom_flags(FILE_FLAG_DELETE_ON_CLOSE).open(&path);
            }
        }
        #[cfg(not(windows))]
        {
            if unguarded {
                let _ = std::fs::remove_file(&path);
            }
            drop(lock);
        }
    }
}

/// Whether an open of the lock file failed only because a `release` has it
/// open to delete it (Windows' sharing violation).
fn released_now(e: &std::io::Error) -> bool {
    cfg!(windows) && e.raw_os_error() == Some(32)
}

/// Whether the lock file open on `file` is still the one at `path`: on Unix
/// a `release` may have unlinked it while this waited for its lock. On
/// Windows a file open here cannot be deleted (`release` opens it with no
/// sharing).
fn is_at(file: &std::fs::File, path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        matches!((file.metadata(), std::fs::metadata(path)), (Ok(a), Ok(b)) if a.dev() == b.dev() && a.ino() == b.ino())
    }
    #[cfg(not(unix))]
    {
        let _ = (file, path);
        true
    }
}

impl Store for FileStore {
    fn load(&self) -> Result<Option<Record>, String> {
        self.read(RECORD)
    }

    fn save(&self, record: &Record) -> Result<(), String> {
        self.write(RECORD, record)
    }

    fn clear(&self) -> Result<(), String> {
        self.remove(RECORD)
    }

    fn load_set(&self) -> Result<Option<SetRecord>, String> {
        self.read(SET_RECORD)
    }

    fn save_set(&self, record: &SetRecord) -> Result<(), String> {
        self.write(SET_RECORD, record)
    }

    fn clear_set(&self) -> Result<(), String> {
        self.remove(SET_RECORD)
    }
}

/// The profile the driver applies to an exe, as `Drs::resolve` finds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub profile: String,
    pub predefined: bool,
    /// The profile's application entry that matched: the exe's full path,
    /// or a bare name.
    pub entry: Entry,
    /// The setting as the driver applies it to the exe: the profile's own
    /// or inherited (the global or base profile's); None when nothing sets
    /// it.
    pub value: Option<u32>,
}

/// What a profile holds, for the undo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Held {
    pub predefined: bool,
    pub apps: Vec<Entry>,
    /// Whether `apps` is all of them: false where the enumeration stopped
    /// short of the count the profile gives.
    pub complete: bool,
    /// How many settings the profile has of its own.
    pub settings: u32,
    /// Its own value of `VRR_APP_OVERRIDE`.
    pub own_value: Option<u32>,
}

/// The driver's settings database, in a session: what is changed stays in
/// the session until `save`. NVAPI's (`nvapi::Session`) on Windows, a fake
/// in the tests.
pub trait Drs {
    type Profile: Copy;
    /// The profile the driver applies to the exe at `app`, a full path;
    /// None when no profile has it.
    fn resolve(&self, app: &str) -> Result<Option<Resolved>, Fault>;
    fn find_profile(&self, name: &str) -> Result<Option<Self::Profile>, Fault>;
    fn held(&self, profile: Self::Profile) -> Result<Held, Fault>;
    fn create_profile(&self, name: &str) -> Result<Self::Profile, Fault>;
    fn add_app(&self, profile: Self::Profile, entry: &Entry) -> Result<(), Fault>;
    /// The entry `entry` is, whole (`NvAPI_DRS_DeleteApplicationEx`), so no
    /// other one a bare name might also pick.
    fn delete_app(&self, profile: Self::Profile, entry: &Entry) -> Result<(), Fault>;
    /// `VRR_APP_OVERRIDE` at `value`, as the profile's own.
    fn set(&self, profile: Self::Profile, value: u32) -> Result<(), Fault>;
    /// `VRR_APP_OVERRIDE` at Force Off, as the profile's own.
    fn set_off(&self, profile: Self::Profile) -> Result<(), Fault> {
        self.set(profile, FORCE_OFF)
    }
    /// `VRR_APP_OVERRIDE` back to what the driver gives the profile: its
    /// predefined value, or none (`NvAPI_DRS_RestoreProfileDefaultSetting`).
    fn restore_default(&self, profile: Self::Profile) -> Result<(), Fault>;
    fn delete_setting(&self, profile: Self::Profile) -> Result<(), Fault>;
    fn delete_profile(&self, profile: Self::Profile) -> Result<(), Fault>;
    /// The session's changes, into the driver's database.
    fn save(&self) -> Result<(), Fault>;
}

/// How `ensure` ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// No NVIDIA driver (or no NVIDIA GPU driving a display): nothing to do.
    NoNvidia,
    /// Force Off already for the exe, in this profile, of no one's making
    /// that Luna knows; nothing written.
    Off(String),
    /// The exe has an entry of its full path in this profile, not Luna's: a
    /// person's choice, left as it is.
    Theirs(String),
    /// Luna's own profile, made now with the exe in it.
    Made(String),
    /// The exe's path added now to Luna's own profile (an exe that moved).
    Added(String),
    /// Luna set it earlier, in this profile; not set again.
    Kept(String),
    /// The profile Luna made is no longer there, or no longer Luna's; not
    /// made again.
    Gone(String),
    /// The driver takes no entry of Luna's, as this profile, which it
    /// applies to the exe, holds the exe's name `exe` (NVIDIA's "Luna" holds
    /// `luna.exe`): the person is asked whether to set Force Off there
    /// (`turn_off`). Nothing written.
    Ask { profile: String, exe: String },
    /// Set off in this profile, not Luna's, on the person's yes
    /// (`SetRecord`), now or earlier; not set again.
    SetIn(String),
    /// Not off, and the test build, which only reads, left it.
    ReadOnly,
    /// Not looked at: the window runs as administrator, and why.
    Elevated(String),
    /// Not off, and why.
    NotOff(String),
}

/// The window's one-line hint for `Outcome::NotOff`.
pub fn hint(why: &str) -> String {
    format!(
        "NVIDIA G-SYNC could not be turned off for Luna ({why}). If the screen stutters while Luna is open, set luna.exe to \
         Fixed Refresh: the user guide's \"The window and panes\", G-SYNC."
    )
}

/// Whether this build may write the driver's profiles: a release, or the
/// test build with `LUNA_TEST_NVIDIA=write` (a live test's).
pub fn may_write() -> bool {
    !crate::paths::is_test() || std::env::var("LUNA_TEST_NVIDIA").is_ok_and(|v| v == "write")
}

/// `ensure`'s work over `drs` and `store`, the lock held: `app` the running
/// exe's full path, `write` false only reads (the record aside, which a
/// recovery rewrites), `test` the build's flavor, `fresh` the name and
/// marker for a profile made now.
pub fn ensure_in<D: Drs>(drs: &D, store: &dyn Store, app: &str, write: bool, test: bool, fresh: &(String, String)) -> Outcome {
    let record = match store.load() {
        Ok(Some(record)) => match refusal(&record, test) {
            Some(why) => return Outcome::NotOff(format!("{why}; left as it is")),
            None => Some(record),
        },
        Ok(None) => None,
        Err(why) => return Outcome::NotOff(format!("{why}; left as it is")),
    };
    let record = match record {
        Some(record) if record.pending.is_some() => match recover(drs, store, record) {
            Ok(record) => record,
            Err(why) => return Outcome::NotOff(why),
        },
        record => record,
    };
    let set = match set_record(drs, store, test) {
        Ok(set) => set,
        Err(why) => return Outcome::NotOff(why),
    };
    if let Some(set) = &set {
        // Set there on the person's yes: neither set nor asked again.
        match drs.resolve(app) {
            Ok(Some(found)) if found.profile == set.profile => return Outcome::SetIn(found.profile),
            Ok(_) => {}
            Err(fault) => return Outcome::NotOff(nvapi::why(fault)),
        }
    }
    let done = match record {
        Some(record) => add(drs, store, record, app, write),
        None => make(drs, store, app, write, test, fresh),
    };
    match (done, set) {
        // One setting of Luna's in another profile is all Luna keeps track
        // of: the person is not asked about a second.
        (Ok(Outcome::Ask { profile, .. }), Some(set)) => Outcome::NotOff(format!(
            "the NVIDIA profile \"{profile}\" has this exe's name, and Luna turned G-SYNC off in the profile \"{}\" before; \
             nothing changed",
            set.profile
        )),
        (done, _) => done.unwrap_or_else(Outcome::NotOff),
    }
}

/// The setting's record, None where there is none; why not, where it is
/// not acted on. One whose save was left pending is settled by what the
/// database holds: Force Off as the profile's own value there means the
/// save went through.
fn set_record<D: Drs>(drs: &D, store: &dyn Store, test: bool) -> Result<Option<SetRecord>, String> {
    let Some(mut record) = store.load_set()? else { return Ok(None) };
    if let Some(why) = set_refusal(&record, test) {
        return Err(format!("{why}; left as it is"));
    }
    let held = held(drs, &record.profile)?;
    if !record.pending {
        if held.is_none() {
            // The profile is gone (a driver's clean install, or one that
            // no longer ships it): nothing is left for the undo.
            store.clear_set()?;
            return Ok(None);
        }
        return Ok(Some(record));
    }
    let saved = held.is_some_and(|(_, held)| held.own_value == Some(FORCE_OFF));
    if !saved {
        store.clear_set()?;
        return Ok(None);
    }
    record.pending = false;
    store.save_set(&record)?;
    Ok(Some(record))
}

/// What the profile `name` holds, None where there is none.
fn held<D: Drs>(drs: &D, name: &str) -> Result<Option<(D::Profile, Held)>, String> {
    match drs.find_profile(name).map_err(nvapi::why)? {
        Some(profile) => Ok(Some((profile, drs.held(profile).map_err(nvapi::why)?))),
        None => Ok(None),
    }
}

/// A record whose save was left pending (the window ended between the
/// record and the save, or before the confirmation), settled by what the
/// driver's database holds: Luna's entry for that exe there means the save
/// went through. A profile of that name without it is not Luna's.
fn recover<D: Drs>(drs: &D, store: &dyn Store, mut record: Record) -> Result<Option<Record>, String> {
    let Some(app) = record.pending.take() else { return Ok(Some(record)) };
    let saved =
        held(drs, &record.profile)?.is_some_and(|(_, held)| !held.predefined && held.apps.iter().any(|e| e.is_lunas(&app, &record.marker)));
    if saved {
        record.created = true;
        if !record.apps.iter().any(|a| same_app(a, &app)) {
            record.apps.push(app);
        }
    } else if !record.created {
        // Nothing of Luna's is there: the profile was never saved.
        store.clear()?;
        return Ok(None);
    }
    store.save(&record)?;
    Ok(Some(record))
}

/// No record: Luna's profile made, with the exe and the setting in it,
/// unless the exe is off already or a profile has it by its full path. One
/// of Luna's own that has it, the record of which was lost, is taken back.
fn make<D: Drs>(drs: &D, store: &dyn Store, app: &str, write: bool, test: bool, fresh: &(String, String)) -> Result<Outcome, String> {
    let found = drs.resolve(app).map_err(nvapi::why)?;
    match &found {
        Some(found) if same_app(&found.entry.app, app) => return take_back(drs, store, found.clone(), test),
        Some(found) if found.value == Some(FORCE_OFF) => return Ok(Outcome::Off(found.profile.clone())),
        _ => {}
    }
    if !write {
        return Ok(Outcome::ReadOnly);
    }
    let (name, marker) = fresh;
    let profile = drs.create_profile(name).map_err(nvapi::why)?;
    if let Err(fault) = drs.add_app(profile, &Entry::lunas(app, marker)) {
        return refused(fault, app, found.as_ref());
    }
    drs.set_off(profile).map_err(nvapi::why)?;
    applies(drs, app, name, marker, true)?;
    let mut record =
        Record { profile: name.clone(), marker: marker.clone(), test, created: false, apps: Vec::new(), pending: Some(app.to_owned()) };
    store.save(&record).map_err(|why| format!("{why}; nothing saved"))?;
    if let Err(fault) = drs.save() {
        let _ = store.clear();
        return Err(nvapi::why(fault));
    }
    record.created = true;
    record.apps = record.pending.take().into_iter().collect();
    confirm(store, &record);
    Ok(Outcome::Made(record.profile))
}

/// The profile `found` names the exe in by its full path, with no record:
/// Luna's own taken back (a record whose write a power cut undid), where it
/// is of Luna's naming, not predefined, and the entry of Luna's making;
/// anyone else's left as it is.
fn take_back<D: Drs>(drs: &D, store: &dyn Store, found: Resolved, test: bool) -> Result<Outcome, String> {
    let Some(marker) = found.entry.marker().filter(|_| ours(&found.profile, test) && !found.predefined) else {
        return Ok(Outcome::Theirs(found.profile));
    };
    let Some((_, held)) = held(drs, &found.profile)? else { return Ok(Outcome::Theirs(found.profile)) };
    let apps = held.apps.iter().filter(|e| e.marker() == Some(marker)).map(|e| e.app.clone()).collect();
    let record = Record { profile: found.profile, marker: marker.to_owned(), test, created: true, apps, pending: None };
    confirm(store, &record);
    Ok(Outcome::Kept(record.profile))
}

/// A record confirmed: nothing, where it has the exe; else the exe's path
/// added to Luna's profile, its setting as it now is.
fn add<D: Drs>(drs: &D, store: &dyn Store, mut record: Record, app: &str, write: bool) -> Result<Outcome, String> {
    if record.apps.iter().any(|a| same_app(a, app)) {
        return Ok(Outcome::Kept(record.profile));
    }
    let Some((profile, held)) = held(drs, &record.profile)? else { return Ok(Outcome::Gone(record.profile)) };
    if !record.owns(&held) {
        // A profile of that name that is not Luna's (made by someone in its
        // place, or predefined): never written.
        return Ok(Outcome::Gone(record.profile));
    }
    let found = drs.resolve(app).map_err(nvapi::why)?;
    if let Some(found) = found.as_ref().filter(|found| same_app(&found.entry.app, app)) {
        if found.profile != record.profile || !found.entry.is_lunas(app, &record.marker) {
            return Ok(Outcome::Theirs(found.profile.clone()));
        }
        // Luna's entry, by a save whose record was lost.
        record.apps.push(app.to_owned());
        confirm(store, &record);
        return Ok(Outcome::Kept(record.profile));
    }
    if !write {
        return Ok(Outcome::ReadOnly);
    }
    if let Err(fault) = drs.add_app(profile, &Entry::lunas(app, &record.marker)) {
        return refused(fault, app, found.as_ref());
    }
    applies(drs, app, &record.profile, &record.marker, false)?;
    record.pending = Some(app.to_owned());
    store.save(&record).map_err(|why| format!("{why}; nothing saved"))?;
    if let Err(fault) = drs.save() {
        record.pending = None;
        let _ = store.save(&record);
        return Err(nvapi::why(fault));
    }
    record.apps.extend(record.pending.take());
    confirm(store, &record);
    Ok(Outcome::Added(record.profile))
}

/// The driver took no entry of Luna's for the exe at `app`: the person is
/// asked (`Outcome::Ask`) where the profile that holds the exe's name is the
/// one the exe resolves to (`found`), by that name; else why
/// (`refused_entry`).
fn refused(fault: Fault, app: &str, found: Option<&Resolved>) -> Result<Outcome, String> {
    match found.filter(|found| fault.status == nvapi::EXECUTABLE_ALREADY_IN_USE && same_app(&found.entry.app, exe_name(app))) {
        Some(found) => Ok(Outcome::Ask { profile: found.profile.clone(), exe: found.entry.app.clone() }),
        None => Err(refused_entry(fault, app, found)),
    }
}

/// The exe's file name, from its full path.
fn exe_name(app: &str) -> &str {
    app.rsplit(['/', '\\']).next().unwrap_or(app)
}

/// Why the driver took no entry of Luna's for the exe at `app`. It refuses
/// an entry, a full path's too, whose exe name a profile already has as a
/// bare name (`NVAPI_EXECUTABLE_ALREADY_IN_USE`), whatever else Luna's entry
/// sets; NVIDIA's "Luna" has the bare `luna.exe`. `found`, the profile the
/// exe resolved to, is named only where it matched by that bare name.
fn refused_entry(fault: Fault, app: &str, found: Option<&Resolved>) -> String {
    if fault.status != nvapi::EXECUTABLE_ALREADY_IN_USE {
        return nvapi::why(fault);
    }
    let exe = exe_name(app);
    let holder = found
        .filter(|found| same_app(&found.entry.app, exe))
        .map_or("another NVIDIA profile".to_owned(), |found| format!("the NVIDIA profile \"{}\"", found.profile));
    format!("{holder} has this exe's name, and the driver gives no other profile an entry of that name ({fault}); nothing saved")
}

/// Whether the session, before its save, resolves `app` to Luna's entry in
/// the profile `name` (and, for one made now, with the setting off): the
/// driver's own answer to which entry wins, so a profile it would not apply
/// is never saved.
fn applies<D: Drs>(drs: &D, app: &str, name: &str, marker: &str, off: bool) -> Result<(), String> {
    match drs.resolve(app).map_err(nvapi::why)? {
        Some(found) if found.profile == name && found.entry.is_lunas(app, marker) && (!off || found.value == Some(FORCE_OFF)) => Ok(()),
        found => Err(format!(
            "the NVIDIA driver would apply {} to this exe rather than Luna's own; nothing saved",
            found.map_or("no profile".to_owned(), |f| format!("the profile \"{}\"", f.profile))
        )),
    }
}

/// The record after its save. Where it does not write, the pending one
/// stays, and the next start settles it from the database.
fn confirm(store: &dyn Store, record: &Record) {
    if let Err(why) = store.save(record) {
        crate::log::warn("app", &format!("{why}: the next start reads the NVIDIA profiles to settle it"));
    }
}

/// `turn_off`'s work over `drs` and `store`, the lock held: Force Off set in
/// the profile `profile`, on the person's yes to `Outcome::Ask`. Only where
/// the exe at `app` still resolves to that profile by its bare name (the
/// window's word is not taken for which profile), and only that setting,
/// only where it is not Force Off already; what it was is recorded ahead of
/// the save, and the session asked again before it.
pub fn turn_off_in<D: Drs>(drs: &D, store: &dyn Store, app: &str, profile: &str, test: bool) -> Result<Outcome, String> {
    if let Some(record) = set_record(drs, store, test)? {
        if record.profile == profile {
            return Ok(Outcome::SetIn(record.profile));
        }
        return Err(format!("Luna set it in the profile \"{}\" already; nothing changed", record.profile));
    }
    let found = drs.resolve(app).map_err(nvapi::why)?;
    let Some(found) = found.filter(|found| found.profile == profile && same_app(&found.entry.app, exe_name(app))) else {
        return Err(format!("the NVIDIA driver no longer applies the profile \"{profile}\" to this exe by its name; nothing changed"));
    };
    if found.value == Some(FORCE_OFF) {
        return Ok(Outcome::Off(found.profile));
    }
    let (handle, held) = held(drs, profile)?.ok_or_else(|| format!("the profile \"{profile}\" is gone; nothing changed"))?;
    drs.set_off(handle).map_err(nvapi::why)?;
    match drs.resolve(app).map_err(nvapi::why)? {
        Some(now) if now.profile == profile && now.value == Some(FORCE_OFF) => {}
        _ => return Err(format!("the NVIDIA driver would not apply Force Off from the profile \"{profile}\" to this exe; nothing saved")),
    }
    let mut record = SetRecord { profile: profile.to_owned(), previous: held.own_value, test, pending: true };
    store.save_set(&record).map_err(|why| format!("{why}; nothing saved"))?;
    if let Err(fault) = drs.save() {
        let _ = store.clear_set();
        return Err(nvapi::why(fault));
    }
    record.pending = false;
    if let Err(why) = store.save_set(&record) {
        crate::log::warn("app", &format!("{why}: the next start reads the NVIDIA profiles to settle it"));
    }
    Ok(Outcome::SetIn(record.profile))
}

/// What the undo removes from one of Luna's profiles, holding `held`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UndoPlan {
    /// The whole profile: it holds only Luna's entries and setting.
    pub profile: bool,
    /// Else these entries of Luna's, whole, as the profile has them.
    pub apps: Vec<Entry>,
    /// And the setting, which Luna set and no one changed, where no one
    /// else's exe is there for it to serve.
    pub setting: bool,
}

/// What the undo removes: only entries of Luna's making, by their marker
/// (`Entry::marker`), whatever their path, so one a record lost track of
/// goes too; the whole profile only where it is known to hold nothing else,
/// an enumeration cut short being no proof.
pub fn undo_plan(marker: &str, held: &Held) -> UndoPlan {
    let apps: Vec<Entry> = held.apps.iter().filter(|e| e.marker() == Some(marker)).cloned().collect();
    let others = held.apps.len() > apps.len() || !held.complete;
    let only_setting = held.settings == 0 || (held.settings == 1 && held.own_value.is_some());
    if !others && only_setting {
        return UndoPlan { profile: true, ..UndoPlan::default() };
    }
    UndoPlan { profile: false, apps, setting: !others && held.own_value == Some(FORCE_OFF) }
}

/// How the undo ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Undone {
    /// Nothing of Luna's there (no record and no profile of Luna's for the
    /// exe, or the profile gone or no longer Luna's).
    Nothing,
    /// Removed, and nothing of Luna's found left: the plan for each profile.
    Done(Vec<UndoPlan>),
    /// The setting Luna set in this profile on the person's yes put back.
    Restored(String),
    /// Removed what it could, but something of Luna's may be left, and why.
    LeftBehind(String),
    /// The record is not acted on, and why.
    Refused(String),
    /// The driver refused or failed.
    Failed(Fault),
}

/// Luna's profiles the undo is to clear, by name and marker: the record's,
/// and the one the exe at `app` resolves to where that is Luna's (of Luna's
/// naming, not predefined, the entry of Luna's making), which a record may
/// not know of (its rename undone by a power cut after a save).
fn targets<D: Drs>(drs: &D, record: Option<&Record>, app: Option<&str>, test: bool) -> Result<Vec<(String, String)>, Fault> {
    let mut targets: Vec<(String, String)> = record.map(|r| (r.profile.clone(), r.marker.clone())).into_iter().collect();
    if let Some(found) = app.map(|app| drs.resolve(app)).transpose()?.flatten() {
        if let Some(marker) = found.entry.marker().filter(|_| ours(&found.profile, test) && !found.predefined) {
            if !targets.iter().any(|(name, _)| *name == found.profile) {
                targets.push((found.profile.clone(), marker.to_owned()));
            }
        }
    }
    Ok(targets)
}

/// Why something of Luna's may be left in the profiles `targets` name, or
/// for the exe at `app`, as the session now has them; None where nothing
/// is.
fn left<D: Drs>(drs: &D, targets: &[(String, String)], app: Option<&str>, test: bool) -> Result<Option<String>, Fault> {
    for (name, marker) in targets {
        let Some(profile) = drs.find_profile(name)? else { continue };
        let held = drs.held(profile)?;
        if held.predefined {
            continue;
        }
        if !held.complete {
            return Ok(Some(format!("the profile \"{name}\" has more entries than NVAPI listed")));
        }
        if held.apps.iter().any(|e| e.marker() == Some(marker)) {
            return Ok(Some(format!("Luna's entries are still in the profile \"{name}\"")));
        }
    }
    let still = app.map(|app| drs.resolve(app)).transpose()?.flatten();
    if let Some(found) = still.filter(|f| f.entry.marker().is_some() && ours(&f.profile, test) && !f.predefined) {
        return Ok(Some(format!("this exe still resolves to Luna's profile \"{}\"", found.profile)));
    }
    Ok(None)
}

/// `undo`'s work over `drs` and `store`, the lock held, `app` the exe's own
/// full path (None where NVAPI cannot be asked about it). Luna's entries go
/// from the record's profile and the exe's (`targets`); the session is read
/// again before the record goes, and anything of Luna's found left is said
/// (`LeftBehind`), the record kept for another try.
pub fn undo_in<D: Drs>(drs: &D, store: &dyn Store, app: Option<&str>, test: bool) -> Undone {
    let record = match store.load() {
        Ok(record) => record,
        Err(why) => return Undone::Refused(why),
    };
    if let Some(why) = record.as_ref().and_then(|r| refusal(r, test)) {
        return Undone::Refused(why);
    }
    let run = || -> Result<Undone, Fault> {
        let targets = targets(drs, record.as_ref(), app, test)?;
        let mut plans = Vec::new();
        for (name, marker) in &targets {
            let Some(profile) = drs.find_profile(name)? else { continue };
            let held = drs.held(profile)?;
            if held.predefined || !held.apps.iter().any(|e| e.marker() == Some(marker.as_str())) {
                // Never one of NVIDIA's, nor one someone made under that name.
                continue;
            }
            let plan = undo_plan(marker, &held);
            if plan.profile {
                drs.delete_profile(profile)?;
            } else {
                for entry in &plan.apps {
                    drs.delete_app(profile, entry)?;
                }
                if plan.setting {
                    drs.delete_setting(profile)?;
                }
            }
            plans.push(plan);
        }
        if plans.iter().any(|plan| *plan != UndoPlan::default()) {
            drs.save()?;
        }
        if let Some(why) = left(drs, &targets, app, test)? {
            return Ok(Undone::LeftBehind(why));
        }
        Ok(if plans.is_empty() { Undone::Nothing } else { Undone::Done(plans) })
    };
    let done = match run() {
        Ok(Undone::LeftBehind(why)) => return Undone::LeftBehind(why),
        Ok(done) => done,
        Err(fault) => return Undone::Failed(fault),
    };
    if record.is_some() {
        if let Err(why) = store.clear() {
            // What it named is gone all the same; a record left behind finds
            // nothing to do the next time.
            crate::log::warn("app", &format!("luna --nvidia-undo: {why}; the profile is undone"));
        }
    }
    done
}

/// `undo`'s work on the setting Luna set on the person's yes
/// (`SetRecord`), in the same session: put back only while the profile
/// still has Luna's value, Force Off as its own; the driver's default
/// restored where that was what applied (in a profile of a person's, which
/// has no predefined value, the profile's own value removed), else the
/// person's earlier value set again. Nothing else in the profile is touched. The record goes once
/// nothing of Luna's is left there; kept where it is refused or fails.
pub fn undo_set_in<D: Drs>(drs: &D, store: &dyn Store, test: bool) -> Undone {
    let record = match store.load_set() {
        Ok(Some(record)) => record,
        Ok(None) => return Undone::Nothing,
        Err(why) => return Undone::Refused(why),
    };
    if let Some(why) = set_refusal(&record, test) {
        return Undone::Refused(why);
    }
    let run = || -> Result<Undone, Fault> {
        let Some(profile) = drs.find_profile(&record.profile)? else { return Ok(Undone::Nothing) };
        // Changed since (or a pending save that did not go through): the
        // person's, or the driver's, as it is.
        let held = drs.held(profile)?;
        if held.own_value != Some(FORCE_OFF) {
            return Ok(Undone::Nothing);
        }
        match record.previous {
            None if held.predefined => drs.restore_default(profile)?,
            None => drs.delete_setting(profile)?,
            Some(value) => drs.set(profile, value)?,
        }
        drs.save()?;
        if drs.held(profile)?.own_value == Some(FORCE_OFF) {
            return Ok(Undone::LeftBehind(format!("G-SYNC is still off in the profile \"{}\"", record.profile)));
        }
        Ok(Undone::Restored(record.profile.clone()))
    };
    match run() {
        Ok(Undone::LeftBehind(why)) => Undone::LeftBehind(why),
        Ok(done) => {
            if let Err(why) = store.clear_set() {
                crate::log::warn("app", &format!("luna --nvidia-undo: {why}; the setting is undone"));
            }
            done
        }
        Err(fault) => Undone::Failed(fault),
    }
}

/// The one of two undos' ends the uninstaller says: a failure first, then
/// something left, then a refusal.
fn worse(a: Undone, b: Undone) -> Undone {
    let rank = |u: &Undone| match u {
        Undone::Failed(_) => 3,
        Undone::LeftBehind(_) => 2,
        Undone::Refused(_) => 1,
        Undone::Nothing | Undone::Done(_) | Undone::Restored(_) => 0,
    };
    if rank(&b) > rank(&a) {
        b
    } else {
        a
    }
}

/// The setting at Force Off for the running exe (module docs). `write`
/// false only reads (the test build). Blocks: NVAPI loads and parses the
/// driver's database, which takes a moment, and an undo under way holds
/// the lock. An elevated window leaves NVIDIA's profiles alone: the record
/// it would write would be Administrators', which Luna does not read
/// (`FileStore`), and its data folder may not be the one Luna has when it
/// runs as the person.
#[cfg(windows)]
pub fn ensure(data: &Path, write: bool) -> Outcome {
    if let Some(why) = crate::elevation::refusal() {
        return Outcome::Elevated(why.to_owned());
    }
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => return Outcome::NotOff(format!("the exe's path did not read: {e}")),
    };
    let Some(app) = app_path(&exe) else {
        return Outcome::NotOff("the exe's path is no Unicode or longer than NVAPI takes; NVAPI is not asked about another".into());
    };
    let store = FileStore::in_data(data);
    let lock = match store.lock(None) {
        Ok(lock) => lock,
        Err(why) => return Outcome::NotOff(why),
    };
    let outcome = (|| {
        let Some(api) = nvapi::Api::load() else { return Outcome::NoNvidia };
        let session = match api.session() {
            Ok(s) => s,
            Err(fault) if nvapi::no_nvidia(fault.status) => return Outcome::NoNvidia,
            Err(fault) => return Outcome::NotOff(nvapi::why(fault)),
        };
        let test = crate::paths::is_test();
        let outcome = ensure_in(&session, &store, &app, write, test, &new_profile(test));
        crate::log::info("window", &format!("NVIDIA profiles for {app}: {}", session.trail.text()));
        outcome
    })();
    // With no record (nothing made, or nothing to make), no lock file stays.
    store.release(lock, false);
    outcome
}

#[cfg(not(windows))]
pub fn ensure(_data: &Path, _write: bool) -> Outcome {
    Outcome::NoNvidia
}

/// The person's yes to `Outcome::Ask`: Force Off set in the profile
/// `profile` (`turn_off_in`), under the lock, as `ensure` writes. Refused in
/// the test build that only reads, and as administrator.
#[cfg(windows)]
pub fn turn_off(data: &Path, profile: &str) -> Result<Outcome, String> {
    if !may_write() {
        return Err("the test build leaves NVIDIA's profiles as they are".into());
    }
    if let Some(why) = crate::elevation::refusal() {
        return Err(why.to_owned());
    }
    let exe = std::env::current_exe().map_err(|e| format!("the exe's path did not read: {e}"))?;
    let app = app_path(&exe).ok_or("the exe's path is no Unicode or longer than NVAPI takes")?;
    let store = FileStore::in_data(data);
    let lock = store.lock(None)?;
    let done = (|| {
        let api = nvapi::Api::load().ok_or("no NVIDIA driver")?;
        let session = api.session().map_err(nvapi::why)?;
        let done = turn_off_in(&session, &store, &app, profile, crate::paths::is_test());
        crate::log::info("window", &format!("NVIDIA profiles for {app}, G-SYNC off in \"{profile}\": {}", session.trail.text()));
        done
    })();
    store.release(lock, false);
    done
}

#[cfg(not(windows))]
pub fn turn_off(_data: &Path, _profile: &str) -> Result<Outcome, String> {
    Err("NVIDIA's profiles are Windows'".into())
}

/// `--nvidia-undo`'s exit codes (windows/hooks.nsh): the driver asks for
/// administrator rights; the record is refused, the lock not had or the
/// undo run as administrator; something of Luna's may be left; anything
/// else that failed.
pub const UNDO_ELEVATION: i32 = 2;
pub const UNDO_REFUSED: i32 = 3;
pub const UNDO_LEFT: i32 = 4;
pub const UNDO_FAILED: i32 = 1;

/// `luna --nvidia-undo`, for the uninstaller once the window and the core
/// are gone: removes what Luna made and puts back what it set, and the
/// records. Where it cannot, or
/// cannot tell it removed all of it, one line on stdout, which the
/// uninstaller shows, and an exit code of the above.
pub fn undo_from_args() -> bool {
    if std::env::args().nth(1).as_deref() != Some("--nvidia-undo") {
        return false;
    }
    let stays = "NVIDIA Control Panel, Manage 3D settings, Program Settings undoes it.";
    let (code, line) = match undo(&crate::paths::data_dir()) {
        Undone::Nothing | Undone::Done(_) | Undone::Restored(_) => (0, None),
        Undone::Refused(why) => (UNDO_REFUSED, Some(format!("Luna's NVIDIA settings were left as they are: {why}."))),
        Undone::LeftBehind(why) => (UNDO_LEFT, Some(format!("Luna's G-SYNC change may be left in NVIDIA's settings: {why}. {stays}"))),
        Undone::Failed(fault) => (
            if fault.status == nvapi::INVALID_USER_PRIVILEGE { UNDO_ELEVATION } else { UNDO_FAILED },
            Some(format!("Luna's G-SYNC change stays in NVIDIA's settings: {}. {stays}", nvapi::why(fault))),
        ),
    };
    if let Some(line) = line {
        crate::log::warn("app", &format!("luna --nvidia-undo: {line}"));
        use std::io::Write;
        let _ = writeln!(std::io::stdout(), "{line}");
        std::process::exit(code);
    }
    true
}

/// The undo, under the lock: the record read first, and the driver's
/// database loaded only after it, so the database read is never older than
/// the record (a window's start saves both under the same lock). With no
/// record it still looks for Luna's profile by the exe's own path. Not as
/// administrator: the uninstaller runs as the person (`RequestExecutionLevel
/// user`), and an elevated run may find another data folder than the
/// window's (`paths::data_dir` takes the exe's folder where the process may
/// write there).
#[cfg(windows)]
fn undo(data: &Path) -> Undone {
    if crate::elevation::refusal().is_some() {
        return Undone::Refused("the undo runs as administrator; uninstall Luna without administrator rights".into());
    }
    let store = FileStore::in_data(data);
    let lock = match store.lock(Some(UNDO_WAIT)) {
        Ok(lock) => lock,
        Err(why) => return Undone::Refused(why),
    };
    let undone = (|| {
        // Each undo refuses on its own record; with neither readable,
        // NVAPI is not loaded at all.
        if let (Err(why), Err(_)) = (store.load(), store.load_set()) {
            return Undone::Refused(why);
        }
        let app = std::env::current_exe().ok().and_then(|exe| app_path(&exe));
        let Some(api) = nvapi::Api::load() else { return Undone::Nothing };
        let test = crate::paths::is_test();
        let undone = match api.session() {
            Ok(session) => {
                // A profile undo that failed part way leaves the session with
                // what it did: not saved by the setting's undo, which waits
                // for another try, its record kept.
                let profile = undo_in(&session, &store, app.as_deref(), test);
                let undone = match profile {
                    Undone::Failed(_) => profile,
                    profile => worse(profile, undo_set_in(&session, &store, test)),
                };
                crate::log::info("app", &format!("luna --nvidia-undo: {}", session.trail.text()));
                undone
            }
            Err(fault) if nvapi::no_nvidia(fault.status) => Undone::Nothing,
            Err(fault) => Undone::Failed(fault),
        };
        undone
    })();
    // Once all of it is gone, the lock file goes too: nothing is left for
    // it to guard.
    store.release(lock, !matches!(undone, Undone::Nothing | Undone::Done(_) | Undone::Restored(_)));
    undone
}

#[cfg(not(windows))]
fn undo(_data: &Path) -> Undone {
    let _ = UNDO_WAIT;
    Undone::Nothing
}

/// NVAPI's types as its headers lay them out, and on Windows the calls.
mod nvapi {
    pub type Status = i32;
    pub const OK: Status = 0;
    pub const ERROR: Status = -1;
    pub const LIBRARY_NOT_FOUND: Status = -2;
    pub const NVIDIA_DEVICE_NOT_FOUND: Status = -6;
    pub const END_ENUMERATION: Status = -7;
    pub const INVALID_USER_PRIVILEGE: Status = -137;
    pub const SETTING_NOT_FOUND: Status = -160;
    pub const PROFILE_NOT_FOUND: Status = -163;
    pub const PROFILE_NAME_IN_USE: Status = -164;
    pub const EXECUTABLE_NOT_FOUND: Status = -166;
    pub const EXECUTABLE_ALREADY_IN_USE: Status = -167;

    /// `NvAPI_UnicodeString`: `NVAPI_UNICODE_STRING_MAX` (2048) UTF-16 units.
    pub type UnicodeString = [u16; 2048];

    /// `NVDRS_SETTING_V1`, `#pragma pack(4)`: its two value unions are each
    /// at most an `NVDRS_BINARY_SETTING` (a length and 4096 bytes), held
    /// here as 1025 words, the first of which is a DWORD setting's value.
    #[repr(C)]
    pub struct Setting {
        pub version: u32,
        pub name: UnicodeString,
        pub id: u32,
        /// `NVDRS_SETTING_TYPE`: 0 a DWORD.
        pub kind: u32,
        /// `NVDRS_SETTING_LOCATION`: 0 this profile, else global, base or
        /// default.
        pub location: u32,
        pub is_current_predefined: u32,
        pub is_predefined_valid: u32,
        pub predefined: [u32; 1025],
        pub current: [u32; 1025],
    }

    /// `NVDRS_APPLICATION_V4`.
    #[repr(C)]
    pub struct Application {
        pub version: u32,
        pub is_predefined: u32,
        pub app_name: UnicodeString,
        pub friendly_name: UnicodeString,
        pub launcher: UnicodeString,
        pub file_in_folder: UnicodeString,
        /// `isMetro:1`, `isCommandLine:1`, reserved.
        pub flags: u32,
        pub command_line: UnicodeString,
    }

    /// `NVDRS_PROFILE_V1`.
    #[repr(C)]
    pub struct Profile {
        pub version: u32,
        pub name: UnicodeString,
        pub gpu_support: u32,
        pub is_predefined: u32,
        pub num_apps: u32,
        pub num_settings: u32,
    }

    /// `MAKE_NVAPI_VERSION(type, ver)`: the structure's size and its version.
    const fn version<T>(ver: u32) -> u32 {
        size_of::<T>() as u32 | (ver << 16)
    }
    pub const SETTING_VER: u32 = version::<Setting>(1);
    pub const APPLICATION_VER: u32 = version::<Application>(4);
    pub const PROFILE_VER: u32 = version::<Profile>(1);

    /// A text as an `NvAPI_UnicodeString`: cut to fit, always terminated.
    pub fn unicode(text: &str) -> Box<UnicodeString> {
        let mut out = Box::new([0u16; 2048]);
        for (slot, unit) in out.iter_mut().take(2047).zip(text.encode_utf16()) {
            *slot = unit;
        }
        out
    }

    pub fn text(s: &UnicodeString) -> String {
        let end = s.iter().position(|&u| u == 0).unwrap_or(s.len());
        String::from_utf16_lossy(&s[..end])
    }

    /// A zeroed structure on the heap: they are 4 to 20 KB each.
    ///
    /// Safety: `T` is one of the plain-data structures above, for which all
    /// zeros is a valid value.
    pub unsafe fn zeroed<T>() -> Box<T> {
        // SAFETY: as the caller promises.
        unsafe { Box::new(std::mem::zeroed()) }
    }

    /// A call that did not answer `OK`: which, and what it answered.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Fault {
        /// The NVAPI function, by its name in nvapi.h.
        pub call: &'static str,
        pub status: Status,
    }

    impl Fault {
        pub fn at(call: &'static str, status: Status) -> Fault {
            Fault { call, status }
        }
    }

    impl std::fmt::Display for Fault {
        /// `NvAPI_DRS_CreateApplication: NVAPI_EXECUTABLE_ALREADY_IN_USE (-167)`.
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}: {} ({})", self.call, name(self.status), self.status)
        }
    }

    /// The calls a session made and what each answered, in order, a run of
    /// one call answering the same counted once (`×n`): for the log, so a
    /// failure there says which step it came at and what came before it.
    #[derive(Default)]
    pub struct Trail(std::cell::RefCell<Vec<(&'static str, Status, u32)>>);

    impl Trail {
        pub fn note(&self, call: &'static str, status: Status) {
            let mut steps = self.0.borrow_mut();
            match steps.last_mut() {
                Some((c, s, n)) if *c == call && *s == status => *n += 1,
                _ => steps.push((call, status, 1)),
            }
        }

        /// `status` noted, and as a `Fault` where it is not `OK`.
        pub fn check(&self, call: &'static str, status: Status) -> Result<(), Fault> {
            self.note(call, status);
            if status == OK {
                Ok(())
            } else {
                Err(Fault::at(call, status))
            }
        }

        /// `NvAPI_Initialize NVAPI_OK, NvAPI_DRS_GetProfileInfo NVAPI_OK ×3,
        /// NvAPI_DRS_CreateApplication NVAPI_EXECUTABLE_ALREADY_IN_USE (-167)`.
        pub fn text(&self) -> String {
            let steps = self.0.borrow();
            let step = |(call, status, n): &(&str, Status, u32)| {
                let status = match *status {
                    OK => name(OK).to_owned(),
                    s => format!("{} ({s})", name(s)),
                };
                match n {
                    1 => format!("{call} {status}"),
                    n => format!("{call} {status} ×{n}"),
                }
            };
            steps.iter().map(step).collect::<Vec<_>>().join(", ")
        }
    }

    /// A status's name in nvapi_lite_common.h (`NVAPI_` and the rest), for
    /// those the calls here may answer; the driver's `NvAPI_GetErrorMessage`
    /// gives these names on 621.24.
    pub fn name(status: Status) -> &'static str {
        match status {
            OK => "NVAPI_OK",
            ERROR => "NVAPI_ERROR",
            LIBRARY_NOT_FOUND => "NVAPI_LIBRARY_NOT_FOUND",
            -3 => "NVAPI_NO_IMPLEMENTATION",
            -4 => "NVAPI_API_NOT_INITIALIZED",
            -5 => "NVAPI_INVALID_ARGUMENT",
            NVIDIA_DEVICE_NOT_FOUND => "NVAPI_NVIDIA_DEVICE_NOT_FOUND",
            END_ENUMERATION => "NVAPI_END_ENUMERATION",
            -8 => "NVAPI_INVALID_HANDLE",
            -9 => "NVAPI_INCOMPATIBLE_STRUCT_VERSION",
            -10 => "NVAPI_HANDLE_INVALIDATED",
            -14 => "NVAPI_INVALID_POINTER",
            INVALID_USER_PRIVILEGE => "NVAPI_INVALID_USER_PRIVILEGE",
            SETTING_NOT_FOUND => "NVAPI_SETTING_NOT_FOUND",
            -161 => "NVAPI_SETTING_SIZE_TOO_LARGE",
            -162 => "NVAPI_TOO_MANY_SETTINGS_IN_PROFILE",
            PROFILE_NOT_FOUND => "NVAPI_PROFILE_NOT_FOUND",
            PROFILE_NAME_IN_USE => "NVAPI_PROFILE_NAME_IN_USE",
            -165 => "NVAPI_PROFILE_NAME_EMPTY",
            EXECUTABLE_NOT_FOUND => "NVAPI_EXECUTABLE_NOT_FOUND",
            EXECUTABLE_ALREADY_IN_USE => "NVAPI_EXECUTABLE_ALREADY_IN_USE",
            -168 => "NVAPI_DATATYPE_MISMATCH",
            -169 => "NVAPI_PROFILE_REMOVED",
            _ => "an NVAPI status",
        }
    }

    /// NVAPI's answer when there is no NVIDIA driver to talk to.
    pub fn no_nvidia(status: Status) -> bool {
        matches!(status, LIBRARY_NOT_FOUND | NVIDIA_DEVICE_NOT_FOUND)
    }

    /// Why, in words, for the log and the hint: the call and its status
    /// always named, so a report says which step failed.
    pub fn why(fault: Fault) -> String {
        match fault.status {
            INVALID_USER_PRIVILEGE => format!("the NVIDIA driver asks for administrator rights to change its profiles ({fault})"),
            _ => format!("{fault}"),
        }
    }

    #[cfg(windows)]
    pub use calls::*;

    #[cfg(windows)]
    mod calls {
        use super::*;
        use std::ffi::c_void;
        use windows::core::{s, w};
        use windows::Win32::Foundation::{FreeLibrary, HMODULE};
        use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};

        type Handle = *mut c_void;
        type QueryInterface = unsafe extern "C" fn(u32) -> *mut c_void;
        type NoArgs = unsafe extern "C" fn() -> Status;
        type CreateSession = unsafe extern "C" fn(*mut Handle) -> Status;
        type OnSession = unsafe extern "C" fn(Handle) -> Status;
        type FindApplicationByName = unsafe extern "C" fn(Handle, *const u16, *mut Handle, *mut Application) -> Status;
        type FindProfileByName = unsafe extern "C" fn(Handle, *const u16, *mut Handle) -> Status;
        type CreateProfile = unsafe extern "C" fn(Handle, *mut Profile, *mut Handle) -> Status;
        type OnProfile = unsafe extern "C" fn(Handle, Handle) -> Status;
        type CreateApplication = unsafe extern "C" fn(Handle, Handle, *mut Application) -> Status;
        type GetSetting = unsafe extern "C" fn(Handle, Handle, u32, *mut Setting) -> Status;
        type SetSetting = unsafe extern "C" fn(Handle, Handle, *mut Setting) -> Status;
        type OnSetting = unsafe extern "C" fn(Handle, Handle, u32) -> Status;
        type GetProfileInfo = unsafe extern "C" fn(Handle, Handle, *mut Profile) -> Status;
        type EnumApplications = unsafe extern "C" fn(Handle, Handle, u32, *mut u32, *mut Application) -> Status;
        type DeleteApplicationEx = unsafe extern "C" fn(Handle, Handle, *mut Application) -> Status;

        /// The calls Luna makes, by their ids in nvapi_interface.h.
        pub struct Api {
            module: HMODULE,
            /// What `NvAPI_Initialize` answered.
            init: Status,
            unload: NoArgs,
            create_session: CreateSession,
            destroy_session: OnSession,
            load_settings: OnSession,
            save_settings: OnSession,
            find_application_by_name: FindApplicationByName,
            find_profile_by_name: FindProfileByName,
            create_profile: CreateProfile,
            delete_profile: OnProfile,
            create_application: CreateApplication,
            get_setting: GetSetting,
            set_setting: SetSetting,
            delete_profile_setting: OnSetting,
            restore_profile_default_setting: OnSetting,
            get_profile_info: GetProfileInfo,
            enum_applications: EnumApplications,
            delete_application_ex: DeleteApplicationEx,
        }

        impl Api {
            /// The driver's library, from System32 only (no other folder's
            /// DLL of that name), initialized; None where there is none or it
            /// lacks a call Luna makes.
            pub fn load() -> Option<Api> {
                // SAFETY: a library loaded by its name from System32; the
                // export is NVAPI's documented entry point, and every id is
                // cast to the signature nvapi.h gives that function.
                unsafe {
                    let module = LoadLibraryExW(w!("nvapi64.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32).ok()?;
                    let Some(query) = GetProcAddress(module, s!("nvapi_QueryInterface")) else {
                        let _ = FreeLibrary(module);
                        return None;
                    };
                    let query = std::mem::transmute::<unsafe extern "system" fn() -> isize, QueryInterface>(query);
                    macro_rules! get {
                        ($t:ty, $id:expr) => {{
                            let f = query($id);
                            if f.is_null() {
                                let _ = FreeLibrary(module);
                                return None;
                            }
                            std::mem::transmute::<*mut c_void, $t>(f)
                        }};
                    }
                    let initialize = get!(NoArgs, 0x0150_E828);
                    let mut api = Api {
                        module,
                        init: NVIDIA_DEVICE_NOT_FOUND,
                        unload: get!(NoArgs, 0xD22B_DD7E),
                        create_session: get!(CreateSession, 0x0694_D52E),
                        destroy_session: get!(OnSession, 0xDAD9_CFF8),
                        load_settings: get!(OnSession, 0x375D_BD6B),
                        save_settings: get!(OnSession, 0xFCBC_7E14),
                        find_application_by_name: get!(FindApplicationByName, 0xEEE5_66B2),
                        find_profile_by_name: get!(FindProfileByName, 0x7E4A_9A0B),
                        create_profile: get!(CreateProfile, 0xCC17_6068),
                        delete_profile: get!(OnProfile, 0x1709_3206),
                        create_application: get!(CreateApplication, 0x4347_A9DE),
                        get_setting: get!(GetSetting, 0x73BF_8338),
                        set_setting: get!(SetSetting, 0x577D_D202),
                        delete_profile_setting: get!(OnSetting, 0xE4A2_6362),
                        restore_profile_default_setting: get!(OnSetting, 0x53F0_381E),
                        get_profile_info: get!(GetProfileInfo, 0x61CD_6FD6),
                        enum_applications: get!(EnumApplications, 0x7FA2_173A),
                        delete_application_ex: get!(DeleteApplicationEx, 0xC5EA_85A1),
                    };
                    api.init = initialize();
                    Some(api)
                }
            }

            /// A session with the driver's settings loaded.
            pub fn session(&self) -> Result<Session<'_>, Fault> {
                let trail = Trail::default();
                trail.check("NvAPI_Initialize", self.init)?;
                let mut handle: Handle = std::ptr::null_mut();
                // SAFETY: NVAPI writes the session's handle.
                trail.check("NvAPI_DRS_CreateSession", unsafe { (self.create_session)(&mut handle) })?;
                let session = Session { api: self, handle, trail };
                // SAFETY: the session just made.
                session.trail.check("NvAPI_DRS_LoadSettings", unsafe { (self.load_settings)(handle) })?;
                Ok(session)
            }
        }

        impl Drop for Api {
            fn drop(&mut self) {
                // SAFETY: paired with the initialize in `load`; the library
                // is freed after, no call of it left.
                unsafe {
                    if self.init == OK {
                        (self.unload)();
                    }
                    let _ = FreeLibrary(self.module);
                }
            }
        }

        pub struct Session<'a> {
            api: &'a Api,
            handle: Handle,
            /// Every call made in it, for the log.
            pub trail: Trail,
        }

        impl Drop for Session<'_> {
            fn drop(&mut self) {
                // SAFETY: the session `api.session` made, destroyed once.
                unsafe { (self.api.destroy_session)(self.handle) };
            }
        }

        use crate::desktop::gsync::{Drs, Entry, Held, Resolved, VRR_APP_OVERRIDE};

        /// The most entries of one profile read: no profile of Luna's comes
        /// near it, and one cut short there is taken as not all read.
        const APPS_MAX: u32 = 4096;

        /// An application as `Entry` has it.
        fn entry(app: &Application) -> Entry {
            Entry {
                app: text(&app.app_name),
                friendly: text(&app.friendly_name),
                predefined: app.is_predefined != 0,
                launcher: text(&app.launcher),
                file_in_folder: text(&app.file_in_folder),
                // isMetro and isCommandLine; the reserved bits are NVAPI's.
                flags: app.flags & 0b11,
                command_line: text(&app.command_line),
            }
        }

        impl Session<'_> {
            /// The setting as it applies in `profile`, and the profile's own
            /// user value of it.
            fn setting(&self, profile: Handle) -> Result<(Option<u32>, Option<u32>), Fault> {
                // SAFETY: a zeroed setting with its version, which NVAPI fills in.
                let mut setting = unsafe { zeroed::<Setting>() };
                setting.version = SETTING_VER;
                // SAFETY: a profile handle of this session.
                let status = unsafe { (self.api.get_setting)(self.handle, profile, VRR_APP_OVERRIDE, &mut *setting) };
                if status == SETTING_NOT_FOUND {
                    self.trail.note("NvAPI_DRS_GetSetting", status);
                    return Ok((None, None));
                }
                self.trail.check("NvAPI_DRS_GetSetting", status)?;
                if setting.kind != 0 {
                    return Ok((None, None));
                }
                let value = setting.current[0];
                let own = setting.location == 0 && setting.is_current_predefined == 0;
                Ok((Some(value), own.then_some(value)))
            }

            fn info(&self, profile: Handle) -> Result<Box<Profile>, Fault> {
                // SAFETY: a zeroed profile with its version, which NVAPI fills in.
                let mut info = unsafe { zeroed::<Profile>() };
                info.version = PROFILE_VER;
                // SAFETY: a profile handle of this session.
                self.trail.check("NvAPI_DRS_GetProfileInfo", unsafe { (self.api.get_profile_info)(self.handle, profile, &mut *info) })?;
                Ok(info)
            }

            /// The profile's application entries, one at a time: each is
            /// 20 KB.
            fn apps(&self, profile: Handle, count: u32) -> Result<Vec<Entry>, Fault> {
                let mut apps = Vec::new();
                for index in 0..count.min(APPS_MAX) {
                    // SAFETY: a zeroed V4 application with its version, which
                    // NVAPI fills in.
                    let mut app = unsafe { zeroed::<Application>() };
                    app.version = APPLICATION_VER;
                    let mut n = 1u32;
                    // SAFETY: room for the one application `n` says.
                    match unsafe { (self.api.enum_applications)(self.handle, profile, index, &mut n, &mut *app) } {
                        END_ENUMERATION => {
                            self.trail.note("NvAPI_DRS_EnumApplications", END_ENUMERATION);
                            break;
                        }
                        status => self.trail.check("NvAPI_DRS_EnumApplications", status)?,
                    }
                    if n == 0 {
                        break;
                    }
                    apps.push(entry(&app));
                }
                Ok(apps)
            }
        }

        impl Drs for Session<'_> {
            type Profile = Handle;

            /// Asked by the full path, as nvapi.h recommends: the answer is
            /// then the profile the driver applies to the exe at that path.
            fn resolve(&self, app: &str) -> Result<Option<Resolved>, Fault> {
                let name = unicode(app);
                let mut profile: Handle = std::ptr::null_mut();
                // SAFETY: a zeroed V4 application with its version, which
                // NVAPI fills in.
                let mut found = unsafe { zeroed::<Application>() };
                found.version = APPLICATION_VER;
                // SAFETY: the name is a full NvAPI_UnicodeString; NVAPI
                // writes the profile's handle and the application.
                let status = unsafe { (self.api.find_application_by_name)(self.handle, name.as_ptr(), &mut profile, &mut *found) };
                if status == EXECUTABLE_NOT_FOUND {
                    self.trail.note("NvAPI_DRS_FindApplicationByName", status);
                    return Ok(None);
                }
                self.trail.check("NvAPI_DRS_FindApplicationByName", status)?;
                let info = self.info(profile)?;
                let (value, _) = self.setting(profile)?;
                Ok(Some(Resolved { profile: text(&info.name), predefined: info.is_predefined != 0, entry: entry(&found), value }))
            }

            fn find_profile(&self, name: &str) -> Result<Option<Handle>, Fault> {
                let name = unicode(name);
                let mut profile: Handle = std::ptr::null_mut();
                // SAFETY: the name is a full NvAPI_UnicodeString.
                match unsafe { (self.api.find_profile_by_name)(self.handle, name.as_ptr(), &mut profile) } {
                    PROFILE_NOT_FOUND => {
                        self.trail.note("NvAPI_DRS_FindProfileByName", PROFILE_NOT_FOUND);
                        Ok(None)
                    }
                    status => self.trail.check("NvAPI_DRS_FindProfileByName", status).map(|()| Some(profile)),
                }
            }

            fn held(&self, profile: Handle) -> Result<Held, Fault> {
                let info = self.info(profile)?;
                let (_, own_value) = self.setting(profile)?;
                let apps = self.apps(profile, info.num_apps)?;
                Ok(Held {
                    predefined: info.is_predefined != 0,
                    complete: apps.len() as u64 == u64::from(info.num_apps),
                    apps,
                    settings: info.num_settings,
                    own_value,
                })
            }

            fn create_profile(&self, name: &str) -> Result<Handle, Fault> {
                // SAFETY: a zeroed profile with its version and name.
                let mut info = unsafe { zeroed::<Profile>() };
                info.version = PROFILE_VER;
                info.name = *unicode(name);
                let mut profile: Handle = std::ptr::null_mut();
                // SAFETY: NVAPI writes the new profile's handle.
                self.trail.check("NvAPI_DRS_CreateProfile", unsafe { (self.api.create_profile)(self.handle, &mut *info, &mut profile) })?;
                Ok(profile)
            }

            /// `entry`'s names; the rest of it is Luna's bare entry, zeroed.
            fn add_app(&self, profile: Handle, entry: &Entry) -> Result<(), Fault> {
                // SAFETY: a zeroed V4 application with its version and names.
                let mut app = unsafe { zeroed::<Application>() };
                app.version = APPLICATION_VER;
                app.app_name = *unicode(&entry.app);
                app.friendly_name = *unicode(&entry.friendly);
                // SAFETY: a profile handle of this session.
                self.trail.check("NvAPI_DRS_CreateApplication", unsafe { (self.api.create_application)(self.handle, profile, &mut *app) })
            }

            /// Every field of the entry as it was read, the V4 layout
            /// `NvAPI_DRS_DeleteApplicationEx` takes (`NVDRS_APPLICATION` is
            /// `NVDRS_APPLICATION_V4` in nvapi.h).
            fn delete_app(&self, profile: Handle, entry: &Entry) -> Result<(), Fault> {
                // SAFETY: a zeroed V4 application with its version, then
                // every field of `entry`.
                let mut app = unsafe { zeroed::<Application>() };
                app.version = APPLICATION_VER;
                app.is_predefined = u32::from(entry.predefined);
                app.app_name = *unicode(&entry.app);
                app.friendly_name = *unicode(&entry.friendly);
                app.launcher = *unicode(&entry.launcher);
                app.file_in_folder = *unicode(&entry.file_in_folder);
                app.flags = entry.flags;
                app.command_line = *unicode(&entry.command_line);
                // SAFETY: a profile handle of this session.
                self.trail
                    .check("NvAPI_DRS_DeleteApplicationEx", unsafe { (self.api.delete_application_ex)(self.handle, profile, &mut *app) })
            }

            fn set(&self, profile: Handle, value: u32) -> Result<(), Fault> {
                // SAFETY: a zeroed setting with its version, id, type and value.
                let mut setting = unsafe { zeroed::<Setting>() };
                setting.version = SETTING_VER;
                setting.id = VRR_APP_OVERRIDE;
                setting.current[0] = value;
                // SAFETY: a profile handle of this session.
                self.trail.check("NvAPI_DRS_SetSetting", unsafe { (self.api.set_setting)(self.handle, profile, &mut *setting) })
            }

            fn restore_default(&self, profile: Handle) -> Result<(), Fault> {
                // SAFETY: a profile handle of this session.
                self.trail.check("NvAPI_DRS_RestoreProfileDefaultSetting", unsafe {
                    (self.api.restore_profile_default_setting)(self.handle, profile, VRR_APP_OVERRIDE)
                })
            }

            fn delete_setting(&self, profile: Handle) -> Result<(), Fault> {
                // SAFETY: a profile handle of this session.
                self.trail.check("NvAPI_DRS_DeleteProfileSetting", unsafe {
                    (self.api.delete_profile_setting)(self.handle, profile, VRR_APP_OVERRIDE)
                })
            }

            fn delete_profile(&self, profile: Handle) -> Result<(), Fault> {
                // SAFETY: a profile handle of this session.
                self.trail.check("NvAPI_DRS_DeleteProfile", unsafe { (self.api.delete_profile)(self.handle, profile) })
            }

            fn save(&self) -> Result<(), Fault> {
                // SAFETY: this session.
                self.trail.check("NvAPI_DRS_SaveSettings", unsafe { (self.api.save_settings)(self.handle) })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    const A: &str = "C:/Apps/Luna/luna.exe";
    const B: &str = "D:/Portable/Luna/luna.exe";
    const NAME: &str = "Luna native window (G-SYNC off) 0a1b2c3d";
    /// A name under the release's earlier prefix, which the driver refuses.
    const COMMA_NAME: &str = "Luna (native window, G-SYNC off) 0a1b2c3d";
    const MARKER: &str = "00112233445566ff";
    const OTHER_SETTING: u32 = 0x1057_EB71;

    fn fresh() -> (String, String) {
        (NAME.into(), MARKER.into())
    }

    /// Luna's entry for `app`.
    fn ours_for(app: &str) -> Entry {
        Entry::lunas(app, MARKER)
    }

    /// An entry a person added (the NVIDIA Control Panel names it by the
    /// exe's description).
    fn theirs(app: &str) -> Entry {
        Entry { friendly: "Some app".into(), ..Entry::lunas(app, MARKER) }
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct P {
        name: String,
        predefined: bool,
        apps: Vec<Entry>,
        /// (id, value), the profile's own.
        settings: Vec<(u32, u32)>,
        /// (id, value), NVIDIA's predefined values, which its own replace.
        defaults: Vec<(u32, u32)>,
    }

    fn p(name: &str, predefined: bool, apps: &[Entry], settings: &[(u32, u32)]) -> P {
        P { name: name.into(), predefined, apps: apps.to_vec(), settings: settings.to_vec(), defaults: Vec::new() }
    }

    /// Luna's own profile, holding Luna's entries for `apps`.
    fn lunas(apps: &[&str], settings: &[(u32, u32)]) -> P {
        p(NAME, false, &apps.iter().map(|a| ours_for(a)).collect::<Vec<_>>(), settings)
    }

    /// NVIDIA's "Luna" profile, for any `luna.exe` by its bare name (a
    /// game's, in driver 621.24): the driver takes no other entry of that
    /// name.
    fn nvidias() -> P {
        let mut entry = theirs("luna.exe");
        entry.predefined = true;
        p("Luna", true, &[entry], &[])
    }

    /// A profile NVIDIA ships for another exe, by its bare name: Luna's
    /// profile is made beside it, and it is left as it is.
    fn game() -> P {
        let mut entry = theirs("game.exe");
        entry.predefined = true;
        p("Some Game", true, &[entry], &[])
    }

    type Journal = Rc<RefCell<Vec<String>>>;

    /// The driver's database: what is saved, and a session's copy of it.
    struct Fake {
        saved: RefCell<Vec<Option<P>>>,
        session: RefCell<Vec<Option<P>>>,
        /// What the global profile sets for `VRR_APP_OVERRIDE`.
        global: Option<u32>,
        /// A driver that would not resolve an exe by its full path.
        misses_paths: bool,
        /// A driver that would apply this profile to any exe.
        prefers: Option<&'static str>,
        /// Enumerations that stop short of the profile's count.
        cut_short: bool,
        save_fails: Option<nvapi::Status>,
        saves: Cell<u32>,
        journal: Journal,
    }

    impl Fake {
        fn new(profiles: Vec<P>, journal: &Journal) -> Fake {
            let db: Vec<_> = profiles.into_iter().map(Some).collect();
            Fake {
                saved: RefCell::new(db.clone()),
                session: RefCell::new(db),
                global: None,
                misses_paths: false,
                prefers: None,
                cut_short: false,
                save_fails: None,
                saves: Cell::new(0),
                journal: journal.clone(),
            }
        }

        /// The next start's session, on what was saved.
        fn reopen(&self) -> Fake {
            let db = self.saved.borrow().clone();
            Fake {
                saved: RefCell::new(db.clone()),
                session: RefCell::new(db),
                global: self.global,
                misses_paths: self.misses_paths,
                prefers: self.prefers,
                cut_short: self.cut_short,
                save_fails: self.save_fails,
                saves: Cell::new(0),
                journal: self.journal.clone(),
            }
        }

        fn saved(&self, name: &str) -> Option<P> {
            self.saved.borrow().iter().flatten().find(|p| p.name == name).cloned()
        }

        fn profiles(&self) -> Vec<P> {
            self.saved.borrow().iter().flatten().cloned().collect()
        }

        /// Changes the saved database, as a person in the NVIDIA Control
        /// Panel would.
        fn edit(&self, name: &str, f: impl FnOnce(&mut P)) {
            let mut db = self.saved.borrow_mut();
            f(db.iter_mut().flatten().find(|p| p.name == name).unwrap());
            *self.session.borrow_mut() = db.clone();
        }

        fn with(&self, i: usize, f: impl FnOnce(&mut P)) -> Result<(), Fault> {
            match self.session.borrow_mut().get_mut(i) {
                Some(Some(p)) => {
                    f(p);
                    Ok(())
                }
                _ => Err(Fault::at("with", nvapi::PROFILE_NOT_FOUND)),
            }
        }
    }

    fn file_name(app: &str) -> &str {
        app.rsplit('/').next().unwrap_or(app)
    }

    impl Drs for Fake {
        type Profile = usize;

        fn resolve(&self, app: &str) -> Result<Option<Resolved>, Fault> {
            let db = self.session.borrow();
            let full = db.iter().flatten().find_map(|p| p.apps.iter().find(|e| same_app(&e.app, app)).map(|e| (p, e)));
            // A bare name with a file in the folder matches only where that
            // file is there, which in these tests it never is.
            let bare = db.iter().flatten().find_map(|p| {
                let bare = |e: &&Entry| !e.app.contains('/') && e.file_in_folder.is_empty() && same_app(&e.app, file_name(app));
                p.apps.iter().find(bare).map(|e| (p, e))
            });
            let preferred =
                self.prefers.and_then(|name| db.iter().flatten().find(|p| p.name == name)).and_then(|p| p.apps.first().map(|e| (p, e)));
            let hit = preferred.or(if self.misses_paths { bare } else { full.or(bare) });
            Ok(hit.map(|(p, e)| Resolved {
                profile: p.name.clone(),
                predefined: p.predefined,
                entry: e.clone(),
                value: [&p.settings, &p.defaults]
                    .into_iter()
                    .find_map(|s| s.iter().find(|(id, _)| *id == VRR_APP_OVERRIDE).map(|(_, v)| *v))
                    .or(self.global),
            }))
        }

        fn find_profile(&self, name: &str) -> Result<Option<usize>, Fault> {
            Ok(self.session.borrow().iter().position(|p| p.as_ref().is_some_and(|p| p.name == name)))
        }

        fn held(&self, i: usize) -> Result<Held, Fault> {
            let db = self.session.borrow();
            let p = db[i].as_ref().ok_or(Fault::at("NvAPI_DRS_GetProfileInfo", nvapi::PROFILE_NOT_FOUND))?;
            let apps: Vec<Entry> = if self.cut_short { p.apps.iter().take(1).cloned().collect() } else { p.apps.clone() };
            Ok(Held {
                predefined: p.predefined,
                complete: apps.len() == p.apps.len(),
                apps,
                settings: p.settings.len() as u32,
                own_value: p.settings.iter().find(|(id, _)| *id == VRR_APP_OVERRIDE).map(|(_, v)| *v),
            })
        }

        /// As driver 621.24 does: refused where the name has a comma, or
        /// where a profile has the name already.
        fn create_profile(&self, name: &str) -> Result<usize, Fault> {
            if name.contains(',') {
                return Err(Fault::at("NvAPI_DRS_CreateProfile", nvapi::ERROR));
            }
            if self.find_profile(name)?.is_some() {
                return Err(Fault::at("NvAPI_DRS_CreateProfile", nvapi::PROFILE_NAME_IN_USE));
            }
            let mut db = self.session.borrow_mut();
            db.push(Some(p(name, false, &[], &[])));
            Ok(db.len() - 1)
        }

        /// As driver 621.24 does: refused where a profile has the same path,
        /// or the exe's name as a bare name, whatever else the entry holds.
        fn add_app(&self, i: usize, entry: &Entry) -> Result<(), Fault> {
            let taken = |e: &Entry| same_app(&e.app, &entry.app) || (!e.app.contains('/') && same_app(&e.app, file_name(&entry.app)));
            if self.session.borrow().iter().flatten().any(|p| p.apps.iter().any(taken)) {
                return Err(Fault::at("NvAPI_DRS_CreateApplication", nvapi::EXECUTABLE_ALREADY_IN_USE));
            }
            self.with(i, |p| p.apps.push(entry.clone()))
        }

        fn delete_app(&self, i: usize, entry: &Entry) -> Result<(), Fault> {
            self.with(i, |p| p.apps.retain(|e| e != entry))
        }

        fn set(&self, i: usize, value: u32) -> Result<(), Fault> {
            self.with(i, |p| {
                p.settings.retain(|(id, _)| *id != VRR_APP_OVERRIDE);
                p.settings.push((VRR_APP_OVERRIDE, value));
            })
        }

        /// As driver 621.24 does for a setting the profile has no
        /// predefined value of: the profile's own goes, and with none there
        /// the call is refused.
        fn restore_default(&self, i: usize) -> Result<(), Fault> {
            let mut had = false;
            self.with(i, |p| {
                had = p.settings.iter().any(|(id, _)| *id == VRR_APP_OVERRIDE);
                p.settings.retain(|(id, _)| *id != VRR_APP_OVERRIDE);
            })?;
            if had {
                Ok(())
            } else {
                Err(Fault::at("NvAPI_DRS_RestoreProfileDefaultSetting", nvapi::SETTING_NOT_FOUND))
            }
        }

        fn delete_setting(&self, i: usize) -> Result<(), Fault> {
            self.with(i, |p| p.settings.retain(|(id, _)| *id != VRR_APP_OVERRIDE))
        }

        fn delete_profile(&self, i: usize) -> Result<(), Fault> {
            self.with(i, |_| ())?;
            self.session.borrow_mut()[i] = None;
            Ok(())
        }

        fn save(&self) -> Result<(), Fault> {
            self.journal.borrow_mut().push("save".into());
            if let Some(status) = self.save_fails {
                return Err(Fault::at("NvAPI_DRS_SaveSettings", status));
            }
            *self.saved.borrow_mut() = self.session.borrow().clone();
            self.saves.set(self.saves.get() + 1);
            Ok(())
        }
    }

    struct Mem {
        record: RefCell<Option<Record>>,
        set: RefCell<Option<SetRecord>>,
        refuse: Option<String>,
        clear_fails: bool,
        journal: Journal,
    }

    impl Mem {
        fn new(record: Option<Record>, journal: &Journal) -> Mem {
            Mem { record: RefCell::new(record), set: RefCell::new(None), refuse: None, clear_fails: false, journal: journal.clone() }
        }
        fn get(&self) -> Option<Record> {
            self.record.borrow().clone()
        }
        fn get_set(&self) -> Option<SetRecord> {
            self.set.borrow().clone()
        }
    }

    impl Store for Mem {
        fn load(&self) -> Result<Option<Record>, String> {
            match &self.refuse {
                Some(why) => Err(why.clone()),
                None => Ok(self.get()),
            }
        }
        fn save(&self, record: &Record) -> Result<(), String> {
            let what = if record.pending.is_some() { "record pending" } else { "record confirmed" };
            self.journal.borrow_mut().push(what.into());
            *self.record.borrow_mut() = Some(record.clone());
            Ok(())
        }
        fn clear(&self) -> Result<(), String> {
            if self.clear_fails {
                return Err("in use".into());
            }
            self.journal.borrow_mut().push("record cleared".into());
            *self.record.borrow_mut() = None;
            Ok(())
        }
        fn load_set(&self) -> Result<Option<SetRecord>, String> {
            match &self.refuse {
                Some(why) => Err(why.clone()),
                None => Ok(self.get_set()),
            }
        }
        fn save_set(&self, record: &SetRecord) -> Result<(), String> {
            let what = if record.pending { "setting pending" } else { "setting confirmed" };
            self.journal.borrow_mut().push(what.into());
            *self.set.borrow_mut() = Some(record.clone());
            Ok(())
        }
        fn clear_set(&self) -> Result<(), String> {
            if self.clear_fails {
                return Err("in use".into());
            }
            self.journal.borrow_mut().push("setting cleared".into());
            *self.set.borrow_mut() = None;
            Ok(())
        }
    }

    fn confirmed(apps: &[&str]) -> Record {
        Record {
            profile: NAME.into(),
            marker: MARKER.into(),
            test: false,
            created: true,
            apps: apps.iter().map(|a| a.to_string()).collect(),
            pending: None,
        }
    }

    fn ensure(drs: &Fake, store: &Mem, app: &str) -> Outcome {
        ensure_in(drs, store, app, true, false, &fresh())
    }

    #[test]
    fn lunas_own_profile_is_made_and_others_left_as_they_are() {
        let journal = Journal::default();
        let drs = Fake::new(vec![game()], &journal);
        let store = Mem::new(None, &journal);
        assert_eq!(ensure(&drs, &store, A), Outcome::Made(NAME.into()));
        assert_eq!(drs.saved("Some Game"), Some(game()));
        assert_eq!(drs.saved(NAME), Some(lunas(&[A], &[(VRR_APP_OVERRIDE, FORCE_OFF)])));
        assert_eq!(store.get(), Some(confirmed(&[A])));
        // Ahead of the save, confirmed after it.
        assert_eq!(*journal.borrow(), ["record pending", "save", "record confirmed"]);
        // The driver now applies Luna's profile to this exe, and only to it.
        assert_eq!(drs.reopen().resolve(A).unwrap().unwrap().profile, NAME);
        assert_eq!(drs.reopen().resolve("E:/Games/Luna/luna.exe").unwrap(), None);
        assert_eq!(drs.reopen().resolve("E:/Games/Game/game.exe").unwrap().unwrap().profile, "Some Game");
    }

    /// Seen on driver 621.24: with NVIDIA's "Luna" holding `luna.exe` by its
    /// bare name, `NvAPI_DRS_CreateApplication` refuses Luna's entry for the
    /// exe's full path (`NVAPI_EXECUTABLE_ALREADY_IN_USE`). The exe resolves
    /// to that profile by that name, so the person is asked; nothing saved,
    /// no record, NVIDIA's profile as it was. Where the profile that has the
    /// name is not the one the exe resolves to, the refusal is said with the
    /// call and its status. The call's name here is the fake's: these tests
    /// hold how a `Fault` is carried and said, and the names
    /// `nvapi::Session` gives its calls are checked live (the log's trail,
    /// docs/architecture/desktop-services.md, "Seen on").
    #[test]
    fn an_exe_name_nvidia_holds_gets_no_entry_and_the_person_is_asked() {
        let journal = Journal::default();
        let drs = Fake::new(vec![nvidias()], &journal);
        let store = Mem::new(None, &journal);
        assert_eq!(ensure(&drs, &store, A), ask());
        assert_eq!((drs.saves.get(), drs.profiles(), store.get(), store.get_set()), (0, vec![nvidias()], None, None));
        assert!(journal.borrow().is_empty());
        // Held by a bare name with a file in the folder that is not there:
        // the exe resolves to no profile, and the driver still refuses.
        let mut held = theirs("luna.exe");
        held.file_in_folder = "LunaGame_Data".into();
        let drs = Fake::new(vec![p("Luna", true, &[held.clone()], &[])], &journal);
        let Outcome::NotOff(why) = ensure(&drs, &store, A) else { panic!("not refused") };
        assert!(why.starts_with("another NVIDIA profile has this exe's name"), "{why}");
        assert!(why.contains("(NvAPI_DRS_CreateApplication: NVAPI_EXECUTABLE_ALREADY_IN_USE (-167))"), "{why}");
        assert_eq!(drs.saves.get(), 0);
        // An exe that moved, after a driver that brought NVIDIA's "Luna":
        // not added, the record as it was, and the person asked.
        let drs = Fake::new(vec![nvidias(), lunas(&[A], &[(VRR_APP_OVERRIDE, FORCE_OFF)])], &journal);
        let store = Mem::new(Some(confirmed(&[A])), &journal);
        assert_eq!(ensure(&drs, &store, B), ask());
        assert_eq!((drs.saves.get(), store.get()), (0, Some(confirmed(&[A]))));
    }

    fn ask() -> Outcome {
        Outcome::Ask { profile: "Luna".into(), exe: "luna.exe".into() }
    }

    /// NVIDIA's "Luna" with `VRR_APP_OVERRIDE` of its own at `value`.
    fn nvidias_at(value: u32) -> P {
        P { settings: vec![(VRR_APP_OVERRIDE, value)], ..nvidias() }
    }

    fn set_record(previous: Option<u32>, pending: bool) -> SetRecord {
        SetRecord { profile: "Luna".into(), previous, test: false, pending }
    }

    /// On the person's yes, the one setting in the profile that holds the
    /// exe's name, recorded ahead of the save with what it was; never set or
    /// asked again, so a later choice stands; and the undo restores the
    /// driver's default, leaving NVIDIA's profile as it shipped.
    #[test]
    fn a_yes_sets_only_that_setting_in_the_profile_that_holds_the_name() {
        let journal = Journal::default();
        let drs = Fake::new(vec![game(), nvidias()], &journal);
        let store = Mem::new(None, &journal);
        // Asked in the start's session, set in the yes's own.
        assert_eq!(ensure(&drs.reopen(), &store, A), ask());
        assert_eq!(turn_off_in(&drs, &store, A, "Luna", false), Ok(Outcome::SetIn("Luna".into())));
        assert_eq!(drs.profiles(), [game(), nvidias_at(FORCE_OFF)]);
        assert_eq!(store.get_set(), Some(set_record(None, false)));
        assert_eq!(store.get(), None);
        assert_eq!(*journal.borrow(), ["setting pending", "save", "setting confirmed"]);
        // The next start: set, so neither set nor asked again.
        let next = drs.reopen();
        assert_eq!(ensure(&next, &store, A), Outcome::SetIn("Luna".into()));
        // The person turned G-SYNC back on there: their choice stands.
        next.edit("Luna", |p| p.settings = vec![(VRR_APP_OVERRIDE, 0)]);
        assert_eq!(ensure(&next, &store, A), Outcome::SetIn("Luna".into()));
        assert_eq!(next.saves.get(), 0);
        // The undo leaves the person's value, and the record goes.
        assert_eq!(undo_set_in(&next, &store, false), Undone::Nothing);
        assert_eq!((next.saved("Luna"), store.get_set()), (Some(nvidias_at(0)), None));

        // Left as Luna set it: the driver's default restored, NVIDIA's
        // profile whole as it was.
        let drs = Fake::new(vec![game(), nvidias()], &journal);
        let store = Mem::new(None, &journal);
        turn_off_in(&drs, &store, A, "Luna", false).unwrap();
        assert_eq!(undo_set_in(&drs.reopen(), &store, false), Undone::Restored("Luna".into()));
        assert_eq!(store.get_set(), None);
        let drs = Fake::new(vec![game(), nvidias_at(FORCE_OFF)], &journal);
        let store = Mem::new(None, &journal);
        *store.set.borrow_mut() = Some(set_record(None, false));
        assert_eq!(undo_set_in(&drs, &store, false), Undone::Restored("Luna".into()));
        assert_eq!(drs.profiles(), [game(), nvidias()]);
    }

    /// A value of the person's own there before Luna's: set again by the
    /// undo, not the driver's default.
    #[test]
    fn the_undo_puts_back_a_value_the_person_had_set() {
        let journal = Journal::default();
        let drs = Fake::new(vec![nvidias_at(0)], &journal);
        let store = Mem::new(None, &journal);
        assert_eq!(ensure(&drs.reopen(), &store, A), ask());
        turn_off_in(&drs, &store, A, "Luna", false).unwrap();
        assert_eq!(store.get_set(), Some(set_record(Some(0), false)));
        assert_eq!(undo_set_in(&drs, &store, false), Undone::Restored("Luna".into()));
        assert_eq!(drs.profiles(), [nvidias_at(0)]);
        // Removed by a clean install of the driver: nothing to do.
        let drs = Fake::new(vec![game()], &journal);
        *store.set.borrow_mut() = Some(set_record(None, false));
        assert_eq!(undo_set_in(&drs, &store, false), Undone::Nothing);
        assert_eq!((drs.saves.get(), store.get_set()), (0, None));
    }

    /// The window's word is not taken for which profile: only the one the
    /// exe resolves to by its bare name, only where the setting is not off
    /// already; a refused save leaves neither the setting nor a record.
    #[test]
    fn a_yes_is_refused_for_any_other_profile_or_a_refused_save() {
        let journal = Journal::default();
        let drs = Fake::new(vec![game(), nvidias()], &journal);
        let store = Mem::new(None, &journal);
        for other in ["Some Game", NAME, "luna"] {
            let why = turn_off_in(&drs, &store, A, other, false).unwrap_err();
            assert!(why.contains("no longer applies") && why.contains("nothing changed"), "{why}");
        }
        // By its full path in a person's profile: not the name's holder.
        let mine = p("Mine", false, &[theirs(A)], &[]);
        let drs2 = Fake::new(vec![nvidias(), mine.clone()], &journal);
        assert!(turn_off_in(&drs2, &store, A, "Mine", false).is_err());
        assert_eq!((drs.saves.get(), drs2.saves.get(), store.get_set()), (0, 0, None));
        // Off there already: nothing written.
        let drs = Fake::new(vec![nvidias_at(FORCE_OFF)], &journal);
        assert_eq!(ensure(&drs, &store, A), Outcome::Off("Luna".into()));
        assert_eq!(turn_off_in(&drs, &store, A, "Luna", false), Ok(Outcome::Off("Luna".into())));
        assert_eq!((drs.saves.get(), store.get_set()), (0, None));
        // The driver refuses the save.
        let mut drs = Fake::new(vec![nvidias()], &journal);
        drs.save_fails = Some(nvapi::INVALID_USER_PRIVILEGE);
        assert!(turn_off_in(&drs, &store, A, "Luna", false).unwrap_err().contains("administrator"));
        assert_eq!((drs.profiles(), store.get_set()), (vec![nvidias()], None));
        // A driver that would apply another profile once it is set.
        let mut drs = Fake::new(vec![game(), nvidias()], &journal);
        drs.prefers = Some("Some Game");
        assert!(turn_off_in(&drs, &store, A, "Some Game", false).is_err());
        assert_eq!((drs.saves.get(), store.get_set()), (0, None));
    }

    /// A start that ended between the record and the save, or before its
    /// confirmation, is settled by what the database holds, as the profile's
    /// record is; and a record of the other flavor is not acted on.
    #[test]
    fn a_pending_or_foreign_setting_record_is_settled_or_refused() {
        let journal = Journal::default();
        let saved = Fake::new(vec![nvidias_at(FORCE_OFF)], &journal);
        let store = Mem::new(None, &journal);
        *store.set.borrow_mut() = Some(set_record(None, true));
        assert_eq!(ensure(&saved, &store, A), Outcome::SetIn("Luna".into()));
        assert_eq!(store.get_set(), Some(set_record(None, false)));
        let unsaved = Fake::new(vec![nvidias()], &journal);
        *store.set.borrow_mut() = Some(set_record(None, true));
        assert_eq!(ensure(&unsaved, &store, A), ask());
        assert_eq!(store.get_set(), None);
        // The undo of a pending one that went through puts it back.
        *store.set.borrow_mut() = Some(set_record(None, true));
        assert_eq!(undo_set_in(&saved, &store, false), Undone::Restored("Luna".into()));
        // The test build's in a release, a release's in the test build, or
        // one that says the setting was off before.
        for (record, test) in [
            (SetRecord { test: true, ..set_record(None, false) }, false),
            (set_record(None, false), true),
            (set_record(Some(FORCE_OFF), false), false),
            (SetRecord { profile: String::new(), ..set_record(None, false) }, false),
        ] {
            let drs = Fake::new(vec![nvidias_at(FORCE_OFF)], &journal);
            let store = Mem::new(None, &journal);
            *store.set.borrow_mut() = Some(record.clone());
            assert!(matches!(ensure_in(&drs, &store, A, true, test, &fresh()), Outcome::NotOff(_)), "{record:?}");
            assert!(turn_off_in(&drs, &store, A, "Luna", test).is_err(), "{record:?}");
            assert!(matches!(undo_set_in(&drs, &store, test), Undone::Refused(_)), "{record:?}");
            assert_eq!((drs.profiles(), store.get_set()), (vec![nvidias_at(FORCE_OFF)], Some(record)));
        }
    }

    /// A value NVIDIA's profile ships for the setting: the driver's default
    /// that the undo brings back (`NvAPI_DRS_RestoreProfileDefaultSetting`);
    /// a profile of a person's holding the name has none, and Luna's value
    /// is removed from it.
    #[test]
    fn the_undo_brings_back_nvidias_value_or_none() {
        let journal = Journal::default();
        let shipped = P { defaults: vec![(VRR_APP_OVERRIDE, 2)], ..nvidias() };
        let drs = Fake::new(vec![shipped.clone()], &journal);
        let store = Mem::new(None, &journal);
        assert_eq!(ensure(&drs.reopen(), &store, A), ask());
        assert_eq!(turn_off_in(&drs, &store, A, "Luna", false), Ok(Outcome::SetIn("Luna".into())));
        assert_eq!(store.get_set(), Some(set_record(None, false)), "NVIDIA's value is no value of the person's");
        assert_eq!(drs.resolve(A).unwrap().unwrap().value, Some(FORCE_OFF));
        assert_eq!(undo_set_in(&drs, &store, false), Undone::Restored("Luna".into()));
        assert_eq!(drs.profiles(), [shipped]);
        assert_eq!(drs.resolve(A).unwrap().unwrap().value, Some(2));
        // A person's own profile with the bare name.
        let mine = p("Mine", false, &[theirs("luna.exe")], &[]);
        let drs = Fake::new(vec![mine.clone()], &journal);
        assert_eq!(ensure(&drs.reopen(), &store, A), Outcome::Ask { profile: "Mine".into(), exe: "luna.exe".into() });
        turn_off_in(&drs, &store, A, "Mine", false).unwrap();
        assert_eq!(undo_set_in(&drs, &store, false), Undone::Restored("Mine".into()));
        assert_eq!(drs.profiles(), [mine]);
    }

    /// The record of a profile that is gone is let go of, so a later yes is
    /// not refused for it; while one stands for a profile still there that
    /// the exe no longer resolves to, the person is not asked about another.
    #[test]
    fn a_setting_record_for_a_profile_gone_or_left_behind() {
        let journal = Journal::default();
        let store = Mem::new(None, &journal);
        *store.set.borrow_mut() = Some(SetRecord { profile: "Old".into(), ..set_record(None, false) });
        let drs = Fake::new(vec![nvidias()], &journal);
        assert_eq!(ensure(&drs.reopen(), &store, A), ask());
        assert_eq!(store.get_set(), None);
        assert_eq!(turn_off_in(&drs, &store, A, "Luna", false), Ok(Outcome::SetIn("Luna".into())));
        // Still there, the exe now resolving to another that has its name.
        *store.set.borrow_mut() = Some(SetRecord { profile: "Old".into(), ..set_record(None, false) });
        let drs = Fake::new(vec![nvidias(), p("Old", false, &[], &[(VRR_APP_OVERRIDE, FORCE_OFF)])], &journal);
        let Outcome::NotOff(why) = ensure(&drs.reopen(), &store, A) else { panic!("asked") };
        assert!(why.contains("\"Old\" before"), "{why}");
        assert!(turn_off_in(&drs, &store, A, "Luna", false).is_err());
        assert_eq!(drs.saves.get(), 0);
    }

    #[test]
    fn the_uninstaller_says_the_worse_of_the_two_undos() {
        let failed = Undone::Failed(Fault::at("NvAPI_DRS_SaveSettings", -1));
        assert_eq!(worse(Undone::Done(vec![]), failed.clone()), failed);
        assert_eq!(worse(Undone::LeftBehind("x".into()), Undone::Refused("y".into())), Undone::LeftBehind("x".into()));
        assert_eq!(worse(Undone::Nothing, Undone::Restored("Luna".into())), Undone::Nothing);
        let text = serde_json::to_string(&set_record(Some(0), false)).unwrap();
        assert_eq!(text, r#"{"profile":"Luna","previous":0,"test":false,"pending":false}"#);
        assert!(serde_json::from_str::<SetRecord>(r#"{"profile":"Luna","previous":null,"test":false}"#).is_err());
    }

    #[test]
    fn lunas_entry_carries_its_marker_and_nothing_else() {
        let entry = Entry::lunas(A, MARKER);
        assert_eq!(entry.friendly, format!("Luna ({MARKER})"));
        assert_eq!(entry.marker(), Some(MARKER));
        assert!(entry.is_lunas(r"c:\apps\luna\LUNA.EXE", MARKER) && !entry.is_lunas(A, "ffffffffffffffff"));
        // A launcher, a file in the folder, a command line, a flag, another
        // name or a predefined one: no longer the entry Luna makes.
        let not = [
            Entry { launcher: "x.exe".into(), ..entry.clone() },
            Entry { file_in_folder: "x.dll".into(), ..entry.clone() },
            Entry { command_line: "-x".into(), ..entry.clone() },
            Entry { flags: 2, ..entry.clone() },
            Entry { predefined: true, ..entry.clone() },
            theirs(A),
            Entry { friendly: "Luna (0011)".into(), ..entry.clone() },
        ];
        for e in not {
            assert_eq!(e.marker(), None, "{e:?}");
        }
    }

    #[test]
    fn a_profile_the_driver_would_not_apply_is_never_saved() {
        let journal = Journal::default();
        let mut drs = Fake::new(vec![game()], &journal);
        drs.misses_paths = true;
        let store = Mem::new(None, &journal);
        assert!(matches!(ensure(&drs, &store, A), Outcome::NotOff(why) if why.contains("apply no profile to this exe")));
        assert_eq!(drs.saves.get(), 0);
        assert_eq!(drs.profiles(), [game()]);
        assert_eq!(store.get(), None);
        // One that would apply another profile, G-SYNC not off there.
        let mut drs = Fake::new(vec![game()], &journal);
        drs.prefers = Some("Some Game");
        assert!(matches!(ensure(&drs, &store, A), Outcome::NotOff(why) if why.contains("apply the profile \"Some Game\" to this exe")));
        assert_eq!((drs.saves.get(), drs.profiles(), store.get()), (0, vec![game()], None));
    }

    #[test]
    fn nothing_is_written_where_it_is_off_already_or_a_profile_of_its_own_has_the_exe() {
        let journal = Journal::default();
        let mut drs = Fake::new(vec![nvidias()], &journal);
        drs.global = Some(FORCE_OFF);
        let store = Mem::new(None, &journal);
        assert_eq!(ensure(&drs, &store, A), Outcome::Off("Luna".into()));
        // A person's profile with the exe's full path, G-SYNC on there.
        let mine = p("Mine", false, &[theirs(r"c:\apps\luna\LUNA.exe")], &[(VRR_APP_OVERRIDE, 0)]);
        let drs = Fake::new(vec![nvidias(), mine], &journal);
        assert_eq!(ensure(&drs, &store, A), Outcome::Theirs("Mine".into()));
        // Of Luna's naming, but the entry a person's: still theirs.
        let named = p(NAME, false, &[theirs(A)], &[(VRR_APP_OVERRIDE, 0)]);
        let drs = Fake::new(vec![game(), named], &journal);
        assert_eq!(ensure(&drs, &store, A), Outcome::Theirs(NAME.into()));
        assert_eq!(drs.saves.get(), 0);
        assert_eq!(store.get(), None);
        assert!(journal.borrow().is_empty());
    }

    #[test]
    fn lunas_profile_without_its_record_is_taken_back() {
        // The record's rename undone by a power cut after the driver saved,
        // the person's later choice in it: taken back as it is, not set
        // again, and the undo then knows it.
        let journal = Journal::default();
        let drs = Fake::new(vec![game(), lunas(&[A, B], &[(VRR_APP_OVERRIDE, 0)])], &journal);
        let store = Mem::new(None, &journal);
        assert_eq!(ensure(&drs, &store, A), Outcome::Kept(NAME.into()));
        assert_eq!(store.get(), Some(confirmed(&[A, B])));
        assert_eq!(drs.saves.get(), 0);
        assert!(matches!(undo_in(&drs, &store, None, false), Undone::Done(p) if p.len() == 1 && p[0].profile));
        assert_eq!(drs.profiles(), [game()]);
        // Not a predefined one, nor one not of Luna's naming.
        for profile in [p(NAME, true, &[ours_for(A)], &[]), p("Mine", false, &[ours_for(A)], &[])] {
            let drs = Fake::new(vec![profile.clone()], &journal);
            let store = Mem::new(None, &journal);
            assert_eq!(ensure(&drs, &store, A), Outcome::Theirs(profile.name.clone()));
            assert_eq!(store.get(), None);
        }
    }

    #[test]
    fn the_test_build_only_reads_unless_told() {
        let journal = Journal::default();
        let drs = Fake::new(vec![game()], &journal);
        let store = Mem::new(None, &journal);
        assert_eq!(ensure_in(&drs, &store, A, false, true, &new_profile(true)), Outcome::ReadOnly);
        assert_eq!(drs.saves.get(), 0);
        let made = new_profile(true);
        assert!(made.0.starts_with(TEST_PREFIX) && ours(&made.0, true) && !ours(&made.0, false) && hex(&made.1, 16));
        assert_eq!(ensure_in(&drs, &store, A, true, true, &made), Outcome::Made(made.0.clone()));
        assert!(store.get().unwrap().test);
    }

    /// Seen on driver 621.24: `NvAPI_DRS_CreateProfile` answers
    /// `NVAPI_ERROR` for a name with a comma, as the release's earlier
    /// prefix had. A name `new_profile` makes has none, in either flavor,
    /// and the profile is made; the earlier name is refused before anything
    /// is saved.
    #[test]
    fn a_new_profile_name_has_no_comma_and_the_driver_takes_it() {
        for test in [false, true] {
            let made = new_profile(test);
            assert!(!made.0.contains(',') && ours(&made.0, test), "{}", made.0);
            let journal = Journal::default();
            let drs = Fake::new(vec![game()], &journal);
            let store = Mem::new(None, &journal);
            assert_eq!(ensure_in(&drs, &store, A, true, test, &made), Outcome::Made(made.0.clone()));
        }
        let journal = Journal::default();
        let drs = Fake::new(vec![game()], &journal);
        let store = Mem::new(None, &journal);
        let refused = ensure_in(&drs, &store, A, true, false, &(COMMA_NAME.into(), MARKER.into()));
        assert!(matches!(&refused, Outcome::NotOff(why) if why.contains("NVAPI_ERROR (-1)")), "{refused:?}");
        assert_eq!((drs.saves.get(), drs.profiles()), (0, vec![game()]));
    }

    /// A record naming a profile under the release's earlier prefix is still
    /// of Luna's naming, for a release and not for the test build.
    #[test]
    fn a_record_under_the_comma_prefix_is_still_lunas() {
        assert!(ours(COMMA_NAME, false) && !ours(COMMA_NAME, true));
        assert!(!ours(&format!("{COMMA_PREFIX} 0a1b2c3"), false));
        let record = Record { profile: COMMA_NAME.into(), ..confirmed(&[A]) };
        assert_eq!(refusal(&record, false), None);
        assert!(refusal(&Record { test: true, ..record.clone() }, true).is_some());
        let journal = Journal::default();
        let drs = Fake::new(vec![game()], &journal);
        let store = Mem::new(Some(record), &journal);
        assert!(!matches!(undo_in(&drs, &store, None, false), Undone::Refused(_)));
    }

    #[test]
    fn it_is_set_once_and_a_later_choice_stands() {
        let journal = Journal::default();
        let drs = Fake::new(vec![game()], &journal);
        let store = Mem::new(None, &journal);
        ensure(&drs, &store, A);
        // The person chose G-SYNC on for Luna in the NVIDIA Control Panel.
        drs.edit(NAME, |p| p.settings = vec![(VRR_APP_OVERRIDE, 0)]);
        let next = drs.reopen();
        assert_eq!(ensure(&next, &store, A), Outcome::Kept(NAME.into()));
        assert_eq!(next.saves.get(), 0);
        assert_eq!(next.saved(NAME).unwrap().settings, [(VRR_APP_OVERRIDE, 0)]);
        // Removed by the person, or with a driver's clean install: not made
        // again.
        let gone = Fake::new(vec![game()], &journal);
        assert_eq!(ensure(&gone, &Mem::new(Some(confirmed(&[A])), &journal), B), Outcome::Gone(NAME.into()));
        assert_eq!(gone.saves.get(), 0);
    }

    #[test]
    fn an_exe_that_moved_is_added_and_the_undo_cleans_both() {
        let journal = Journal::default();
        let drs = Fake::new(vec![game(), lunas(&[A], &[(VRR_APP_OVERRIDE, 0)])], &journal);
        let store = Mem::new(Some(confirmed(&[A])), &journal);
        assert_eq!(ensure(&drs, &store, B), Outcome::Added(NAME.into()));
        // The person's value stands for the new path too.
        assert_eq!(drs.saved(NAME), Some(lunas(&[A, B], &[(VRR_APP_OVERRIDE, 0)])));
        assert_eq!(store.get(), Some(confirmed(&[A, B])));
        assert_eq!(*journal.borrow(), ["record pending", "save", "record confirmed"]);
        let next = drs.reopen();
        assert_eq!(undo_in(&next, &store, None, false), Undone::Done(vec![UndoPlan { profile: true, ..UndoPlan::default() }]));
        assert_eq!(next.profiles(), [game()]);
        assert_eq!(store.get(), None);
    }

    #[test]
    fn a_profile_of_the_recorded_name_that_is_not_lunas_is_never_acted_on() {
        let journal = Journal::default();
        // Luna's profile removed and another made under its name: empty, or
        // with an exe of the person's; or a predefined one of that name.
        let replacements = [p(NAME, false, &[], &[]), p(NAME, false, &[theirs("C:/x.exe")], &[(VRR_APP_OVERRIDE, 0)]), {
            let mut q = lunas(&[A], &[]);
            q.predefined = true;
            q
        }];
        for replaced in replacements {
            let drs = Fake::new(vec![game(), replaced.clone()], &journal);
            // The moved exe is not saved into it.
            let store = Mem::new(Some(confirmed(&[A])), &journal);
            assert_eq!(ensure(&drs, &store, B), Outcome::Gone(NAME.into()), "{replaced:?}");
            // The undo removes nothing of it.
            assert_eq!(undo_in(&drs, &store, None, false), Undone::Nothing, "{replaced:?}");
            assert_eq!(drs.profiles(), [game(), replaced.clone()]);
            // A pending save is not taken to have gone through into it.
            let pending = Record { created: false, apps: vec![], pending: Some(A.into()), ..confirmed(&[]) };
            let store = Mem::new(Some(pending), &journal);
            assert_eq!(recover(&drs, &store, store.get().unwrap()), Ok(None), "{replaced:?}");
            assert_eq!(drs.saves.get(), 0);
        }
    }

    #[test]
    fn an_entry_a_person_added_to_lunas_profile_stays_theirs() {
        let journal = Journal::default();
        // The moved exe added by the person, before Luna's next start.
        let drs = Fake::new(vec![game(), p(NAME, false, &[ours_for(A), theirs(B)], &[(VRR_APP_OVERRIDE, FORCE_OFF)])], &journal);
        let store = Mem::new(Some(confirmed(&[A])), &journal);
        assert_eq!(ensure(&drs, &store, B), Outcome::Theirs(NAME.into()));
        assert_eq!(store.get(), Some(confirmed(&[A])));
        // Not claimed by a recovery either.
        let pending = Record { pending: Some(B.into()), ..confirmed(&[A]) };
        assert_eq!(recover(&drs, &Mem::new(None, &journal), pending), Ok(Some(confirmed(&[A]))));
        // The undo takes out Luna's entry only, and leaves the setting to
        // the person's exe.
        assert_eq!(
            undo_in(&drs, &store, None, false),
            Undone::Done(vec![UndoPlan { profile: false, apps: vec![ours_for(A)], setting: false }])
        );
        assert_eq!(drs.saved(NAME), Some(p(NAME, false, &[theirs(B)], &[(VRR_APP_OVERRIDE, FORCE_OFF)])));
        // Even recorded as Luna's, an entry changed since (a launcher, a
        // command line) is no longer what Luna made.
        let changed = Entry { command_line: "-x".into(), ..ours_for(B) };
        let drs = Fake::new(vec![p(NAME, false, &[ours_for(A), changed.clone()], &[(VRR_APP_OVERRIDE, FORCE_OFF)])], &journal);
        let store = Mem::new(Some(confirmed(&[A, B])), &journal);
        assert!(matches!(undo_in(&drs, &store, None, false), Undone::Done(p) if p.len() == 1 && !p[0].profile));
        assert_eq!(drs.saved(NAME).unwrap().apps, [changed]);
    }

    #[test]
    fn an_enumeration_cut_short_never_removes_the_profile_and_says_so() {
        let journal = Journal::default();
        let mut drs = Fake::new(vec![lunas(&[A, B], &[(VRR_APP_OVERRIDE, FORCE_OFF)])], &journal);
        drs.cut_short = true;
        let store = Mem::new(Some(confirmed(&[A, B])), &journal);
        // What it read of Luna's goes; that it cannot see the rest is said,
        // and the record stays for another try.
        assert!(matches!(undo_in(&drs, &store, Some(A), false), Undone::LeftBehind(_)));
        assert_eq!(drs.saved(NAME), Some(lunas(&[B], &[(VRR_APP_OVERRIDE, FORCE_OFF)])));
        assert_eq!(store.get(), Some(confirmed(&[A, B])));
        // Three, two of them past what is listed: that the list is short is
        // what is said.
        let mut drs = Fake::new(vec![lunas(&[A, B, "E:/Third/luna.exe"], &[])], &journal);
        drs.cut_short = true;
        assert!(matches!(undo_in(&drs, &store, None, false), Undone::LeftBehind(why) if why.contains("more entries")));
    }

    #[test]
    fn the_undo_finds_lunas_entries_the_record_lost_track_of() {
        let journal = Journal::default();
        // The record's rename undone by a power cut after the save, then an
        // uninstall before the next start: no record, found by the exe.
        let drs = Fake::new(vec![game(), lunas(&[A], &[(VRR_APP_OVERRIDE, FORCE_OFF)])], &journal);
        let store = Mem::new(None, &journal);
        assert_eq!(undo_in(&drs, &store, Some(A), false), Undone::Done(vec![UndoPlan { profile: true, ..UndoPlan::default() }]));
        assert_eq!(drs.profiles(), [game()]);
        // An older record after a move (B saved, the record of it lost):
        // both of Luna's entries go, by their marker, whatever their path.
        let drs = Fake::new(vec![game(), lunas(&[A, B], &[(VRR_APP_OVERRIDE, FORCE_OFF)])], &journal);
        let store = Mem::new(Some(confirmed(&[A])), &journal);
        assert_eq!(undo_in(&drs, &store, Some(B), false), Undone::Done(vec![UndoPlan { profile: true, ..UndoPlan::default() }]));
        assert_eq!(drs.profiles(), [game()]);
        assert_eq!(store.get(), None);
        // The record's profile and another of Luna's the exe resolves to.
        let other = p(&format!("{PREFIX} ffff0000"), false, &[Entry::lunas(B, "ffffffffffffffff")], &[]);
        let drs = Fake::new(vec![game(), lunas(&[A], &[]), other], &journal);
        let store = Mem::new(Some(confirmed(&[A])), &journal);
        assert!(matches!(undo_in(&drs, &store, Some(B), false), Undone::Done(p) if p.len() == 2));
        assert_eq!(drs.profiles(), [game()]);
        // Nothing of Luna's: nothing done, and a person's profile for the
        // exe or NVIDIA's left alone.
        let mine = p("Mine", false, &[theirs(A)], &[(VRR_APP_OVERRIDE, 0)]);
        let drs = Fake::new(vec![game(), mine.clone()], &journal);
        assert_eq!(undo_in(&drs, &Mem::new(None, &journal), Some(A), false), Undone::Nothing);
        assert_eq!(undo_in(&drs, &Mem::new(None, &journal), Some("E:/Games/luna.exe"), false), Undone::Nothing);
        assert_eq!(drs.profiles(), [game(), mine]);
        assert_eq!(drs.saves.get(), 0);
    }

    #[test]
    fn a_pending_record_is_settled_by_what_the_database_holds() {
        let journal = Journal::default();
        let pending = |created: bool, apps: &[&str], app: &str| Record { created, pending: Some(app.into()), ..confirmed(apps) };
        // Ended before the save: nothing is there, so the record goes and
        // the profile is made afresh, under a new name.
        let drs = Fake::new(vec![game()], &journal);
        let store = Mem::new(Some(pending(false, &[], A)), &journal);
        let other = (format!("{PREFIX} ffff0000"), "ffffffffffffffff".to_owned());
        assert_eq!(ensure_in(&drs, &store, A, true, false, &other), Outcome::Made(other.0.clone()));
        assert_eq!(drs.profiles().len(), 2);
        // Ended after the save: confirmed, and not set again.
        let drs = Fake::new(vec![game(), lunas(&[A], &[(VRR_APP_OVERRIDE, FORCE_OFF)])], &journal);
        let store = Mem::new(Some(pending(false, &[], A)), &journal);
        assert_eq!(ensure(&drs, &store, A), Outcome::Kept(NAME.into()));
        assert_eq!(store.get(), Some(confirmed(&[A])));
        assert_eq!(drs.saves.get(), 0);
        // A moved exe's path not saved: dropped, then added.
        let drs = Fake::new(vec![game(), lunas(&[A], &[(VRR_APP_OVERRIDE, FORCE_OFF)])], &journal);
        let store = Mem::new(Some(pending(true, &[A], B)), &journal);
        assert_eq!(ensure(&drs, &store, B), Outcome::Added(NAME.into()));
        assert_eq!(store.get(), Some(confirmed(&[A, B])));
        // Even the read-only test build settles the record, writing no profile.
        let drs = Fake::new(vec![game(), lunas(&[A, B], &[(VRR_APP_OVERRIDE, FORCE_OFF)])], &journal);
        let store = Mem::new(Some(pending(true, &[A], B)), &journal);
        assert_eq!(ensure_in(&drs, &store, B, false, false, &fresh()), Outcome::Kept(NAME.into()));
        assert_eq!(store.get(), Some(confirmed(&[A, B])));
        // And the undo counts a pending entry of Luna's as Luna's.
        let drs = Fake::new(vec![game(), lunas(&[A], &[(VRR_APP_OVERRIDE, FORCE_OFF)])], &journal);
        let store = Mem::new(Some(pending(false, &[], A)), &journal);
        assert!(matches!(undo_in(&drs, &store, None, false), Undone::Done(p) if p.len() == 1 && p[0].profile));
        assert_eq!(drs.profiles(), [game()]);
    }

    #[test]
    fn a_refused_save_leaves_neither_profile_nor_record() {
        let journal = Journal::default();
        let mut drs = Fake::new(vec![game()], &journal);
        drs.save_fails = Some(nvapi::INVALID_USER_PRIVILEGE);
        let store = Mem::new(None, &journal);
        assert!(matches!(ensure(&drs, &store, A), Outcome::NotOff(why) if why.contains("administrator")));
        assert_eq!(drs.profiles(), [game()]);
        assert_eq!(store.get(), None);
        // A path to add, refused: the record as it was.
        let mut drs = Fake::new(vec![game(), lunas(&[A], &[(VRR_APP_OVERRIDE, FORCE_OFF)])], &journal);
        drs.save_fails = Some(nvapi::INVALID_USER_PRIVILEGE);
        let store = Mem::new(Some(confirmed(&[A])), &journal);
        assert!(matches!(ensure(&drs, &store, B), Outcome::NotOff(_)));
        assert_eq!(store.get(), Some(confirmed(&[A])));
    }

    #[test]
    fn a_foreign_record_is_not_acted_on() {
        let journal = Journal::default();
        let profile = lunas(&[A], &[(VRR_APP_OVERRIDE, FORCE_OFF)]);
        let cases = [
            // A release's, in the test build (and back).
            (confirmed(&[A]), true),
            (Record { test: true, ..confirmed(&[A]) }, false),
            // A profile Luna does not make: NVIDIA's own, or a person's.
            (Record { profile: "Luna".into(), ..confirmed(&["luna.exe"]) }, false),
            (Record { profile: format!("{PREFIX} mine"), ..confirmed(&[A]) }, false),
            (Record { marker: "luna".into(), ..confirmed(&[A]) }, false),
            (Record { created: false, ..confirmed(&[A]) }, false),
            (Record { apps: vec!["x".repeat(NAME_MAX + 1)], ..confirmed(&[]) }, false),
        ];
        for (record, test) in cases {
            let drs = Fake::new(vec![game(), profile.clone()], &journal);
            let store = Mem::new(Some(record.clone()), &journal);
            assert!(matches!(ensure_in(&drs, &store, B, true, test, &fresh()), Outcome::NotOff(_)), "{record:?}");
            assert!(matches!(undo_in(&drs, &store, None, test), Undone::Refused(_)), "{record:?}");
            assert_eq!(drs.profiles(), [game(), profile.clone()]);
            assert_eq!(store.get(), Some(record));
        }
        // One the store will not read (not this user's, a link).
        let drs = Fake::new(vec![game(), profile.clone()], &journal);
        let store = Mem { refuse: Some("not this user's alone".into()), ..Mem::new(None, &journal) };
        assert!(matches!(ensure(&drs, &store, A), Outcome::NotOff(why) if why.contains("not this user's")));
        assert!(matches!(undo_in(&drs, &store, None, false), Undone::Refused(_)));
        assert_eq!(drs.saves.get(), 0);
    }

    #[test]
    fn the_undo_leaves_what_a_person_added() {
        let record = confirmed(&[A]);
        let held =
            |apps: &[Entry], settings: u32, own_value| Held { predefined: false, apps: apps.to_vec(), complete: true, settings, own_value };
        let whole = UndoPlan { profile: true, ..UndoPlan::default() };
        assert_eq!(undo_plan(MARKER, &held(&[ours_for(A)], 1, Some(FORCE_OFF))), whole);
        // Luna's setting changed by the person: still only Luna's.
        assert_eq!(undo_plan(MARKER, &held(&[ours_for("c:/apps/luna/LUNA.EXE")], 1, Some(0))), whole);
        // A setting of the person's: Luna's entry and setting go.
        assert_eq!(
            undo_plan(MARKER, &held(&[ours_for(A)], 2, Some(FORCE_OFF))),
            UndoPlan { profile: false, apps: vec![ours_for(A)], setting: true }
        );
        // ... but not a value the person chose.
        assert_eq!(
            undo_plan(MARKER, &held(&[ours_for(A)], 2, Some(0))),
            UndoPlan { profile: false, apps: vec![ours_for(A)], setting: false }
        );
        // An exe of the person's: Luna's entry goes, the setting serves theirs.
        assert_eq!(
            undo_plan(MARKER, &held(&[ours_for(A), theirs("C:/x.exe")], 1, Some(FORCE_OFF))),
            UndoPlan { profile: false, apps: vec![ours_for(A)], setting: false }
        );

        let journal = Journal::default();
        let drs = Fake::new(vec![game(), lunas(&[A], &[(VRR_APP_OVERRIDE, FORCE_OFF), (OTHER_SETTING, 7)])], &journal);
        let store = Mem::new(Some(record.clone()), &journal);
        assert!(matches!(undo_in(&drs, &store, None, false), Undone::Done(_)));
        assert_eq!(drs.saved(NAME), Some(p(NAME, false, &[], &[(OTHER_SETTING, 7)])));
        assert_eq!(store.get(), None);
        // Nothing left is nothing to do.
        let drs = Fake::new(vec![game()], &journal);
        let store = Mem::new(Some(record), &journal);
        assert_eq!(undo_in(&drs, &store, None, false), Undone::Nothing);
        assert_eq!(store.get(), None);
        // A refusal keeps the record for another try.
        let mut drs = Fake::new(vec![game(), lunas(&[A], &[(VRR_APP_OVERRIDE, FORCE_OFF)])], &journal);
        drs.save_fails = Some(nvapi::INVALID_USER_PRIVILEGE);
        let store = Mem::new(Some(confirmed(&[A])), &journal);
        assert_eq!(undo_in(&drs, &store, None, false), Undone::Failed(Fault::at("NvAPI_DRS_SaveSettings", nvapi::INVALID_USER_PRIVILEGE)));
        assert_eq!(store.get(), Some(confirmed(&[A])));
    }

    #[test]
    fn a_record_that_will_not_go_does_not_unsay_the_undo() {
        let journal = Journal::default();
        let drs = Fake::new(vec![game(), lunas(&[A], &[(VRR_APP_OVERRIDE, FORCE_OFF)])], &journal);
        let store = Mem { clear_fails: true, ..Mem::new(Some(confirmed(&[A])), &journal) };
        assert_eq!(undo_in(&drs, &store, None, false), Undone::Done(vec![UndoPlan { profile: true, ..UndoPlan::default() }]));
        assert_eq!(drs.profiles(), [game()]);
        // The record left finds nothing the next time.
        assert_eq!(undo_in(&drs.reopen(), &Mem::new(store.get(), &journal), None, false), Undone::Nothing);
    }

    #[test]
    fn the_record_reads_as_written() {
        let record = Record { pending: Some(B.into()), ..confirmed(&[A]) };
        let text = serde_json::to_string(&record).unwrap();
        assert_eq!(
            text,
            r#"{"profile":"Luna native window (G-SYNC off) 0a1b2c3d","marker":"00112233445566ff","test":false,"created":true,"apps":["C:/Apps/Luna/luna.exe"],"pending":"D:/Portable/Luna/luna.exe"}"#
        );
        assert_eq!(serde_json::from_str::<Record>(&text).unwrap(), record);
        // Neither shape 0.20.1's development builds wrote is taken for it.
        assert!(serde_json::from_str::<Record>(r#"{"profile":"Luna","own":false,"previous":4}"#).is_err());
        assert!(serde_json::from_str::<Record>(
            r#"{"profile":"Luna (native window, G-SYNC off) 0a1b2c3d","test":false,"created":true,"apps":[],"pending":null}"#
        )
        .is_err());
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("luna-gsync-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Whether what this process makes is the user's own (`secret::own_owner`):
    /// not where Windows gives an elevated admin's files to Administrators.
    fn makes_its_own() -> bool {
        #[cfg(windows)]
        let own = crate::secret::acl::maker_sid().unwrap() == crate::secret::acl::user_sid().unwrap();
        #[cfg(not(windows))]
        let own = true;
        own
    }

    #[test]
    fn the_file_store_keeps_the_record_private() {
        let dir = scratch("store");
        let store = FileStore::in_data(&dir);
        assert_eq!(store.load(), Ok(None));
        store.save(&confirmed(&[A])).unwrap();
        if makes_its_own() {
            assert_eq!(store.load(), Ok(Some(confirmed(&[A]))));
        } else {
            // An elevated admin's run (CI's Windows runner) makes its files
            // Administrators', not the user's: its own record is not read.
            assert!(store.load().unwrap_err().contains("owned by"));
        }
        store.clear().unwrap();
        assert_eq!(store.load(), Ok(None));
        store.clear().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_lock_is_one_holders_at_a_time() {
        let dir = scratch("lock");
        let store = FileStore::in_data(&dir);
        let held = store.lock(None).unwrap();
        assert!(store.lock(Some(Duration::ZERO)).unwrap_err().contains("another Luna"));
        drop(held);
        assert!(store.lock(Some(Duration::ZERO)).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_lock_file_goes_once_nothing_is_left_for_it_to_guard() {
        let dir = scratch("release");
        let store = FileStore::in_data(&dir);
        let at = dir.join(LOCK);
        // No record (a start that made nothing, an undo that removed all).
        store.release(store.lock(None).unwrap(), false);
        assert!(!at.exists());
        // An undo that may have left something, or a record: kept.
        store.release(store.lock(None).unwrap(), true);
        assert!(at.exists());
        store.save(&confirmed(&[A])).unwrap();
        store.release(store.lock(None).unwrap(), false);
        assert!(at.exists());
        store.clear().unwrap();
        // Another holder waiting for it as it is let go: that one still
        // gets a lock that is the file at its path (on Windows the file is
        // not removed from under it; on Unix it opens a fresh one).
        let held = store.lock(None).unwrap();
        let waiter = {
            let dir = dir.clone();
            std::thread::spawn(move || {
                let store = FileStore::in_data(&dir);
                let lock = store.lock(Some(Duration::from_secs(10))).unwrap();
                assert!(is_at(&lock, &dir.join(LOCK)));
                store.release(lock, false);
            })
        };
        std::thread::sleep(Duration::from_millis(300));
        store.release(held, false);
        waiter.join().unwrap();
        assert!(!at.exists(), "the last to let go removes it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A data folder that is a link or junction would bring a record from
    /// wherever it leads.
    #[test]
    fn a_linked_data_folder_is_refused() {
        let dir = scratch("linked");
        let real = dir.join("real");
        std::fs::create_dir_all(&real).unwrap();
        FileStore::in_data(&real).save(&confirmed(&[A])).unwrap();
        let link = dir.join("link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &link).unwrap();
        #[cfg(windows)]
        {
            // A junction, which needs no rights a symlink does.
            let made = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&link).arg(&real).output().unwrap();
            assert!(made.status.success(), "{made:?}");
        }
        let store = FileStore::in_data(&link);
        assert!(store.load().unwrap_err().contains("link or junction"));
        assert!(store.save(&confirmed(&[A])).is_err() && store.clear().is_err() && store.lock(None).is_err());
        #[cfg(windows)]
        let _ = std::fs::remove_dir(&link);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// One others may read or write, or a link to one, is not Luna's word.
    #[cfg(unix)]
    #[test]
    fn a_record_not_this_users_alone_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("loose");
        let store = FileStore::in_data(&dir);
        store.save(&confirmed(&[A])).unwrap();
        std::fs::set_permissions(store.path(), std::fs::Permissions::from_mode(0o666)).unwrap();
        assert!(store.load().is_err());
        let elsewhere = dir.join("elsewhere.json");
        std::fs::rename(store.path(), &elsewhere).unwrap();
        std::fs::set_permissions(&elsewhere, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::os::unix::fs::symlink(&elsewhere, store.path()).unwrap();
        assert!(store.load().is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The same on Windows, by its DACL: Everyone let in to read it.
    #[cfg(windows)]
    #[test]
    fn a_record_others_may_open_is_refused() {
        let dir = scratch("dacl");
        let store = FileStore::in_data(&dir);
        store.save(&confirmed(&[A])).unwrap();
        assert_eq!(store.load().is_ok(), makes_its_own());
        let granted = std::process::Command::new("icacls").arg(store.path()).args(["/grant", "*S-1-1-0:R"]).output().unwrap();
        assert!(granted.status.success(), "{granted:?}");
        assert!(store.load().unwrap_err().contains("not this user's alone"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn apps_are_one_whatever_their_case_and_slashes() {
        assert!(same_app("C:/Apps/Luna/luna.exe", r"c:\apps\luna\LUNA.EXE"));
        assert!(!same_app("C:/Apps/Luna/luna.exe", "luna.exe"));
        assert!(ours(NAME, false) && !ours("Luna", false) && !ours(&format!("{PREFIX} 0a1b2c3"), false));
    }

    #[test]
    fn an_exe_is_named_by_its_full_path_with_forward_slashes_or_not_at_all() {
        assert_eq!(app_path(Path::new(r"C:\Users\a\Luna\luna.exe")).as_deref(), Some("C:/Users/a/Luna/luna.exe"));
        // Too long for an NvAPI_UnicodeString: NVAPI would be asked about a
        // path cut short.
        let long = format!("C:/{}/luna.exe", "x".repeat(NAME_MAX));
        assert_eq!(app_path(Path::new(&long)), None);
        assert!(app_path(Path::new(&format!("C:/{}", "x".repeat(NAME_MAX - 3)))).is_some());
        // An unpaired surrogate, which a lossy conversion would change.
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStringExt;
            let units: Vec<u16> = "C:/a".encode_utf16().chain([0xD800]).chain("/luna.exe".encode_utf16()).collect();
            assert_eq!(app_path(Path::new(&std::ffi::OsString::from_wide(&units))), None);
        }
    }

    /// Luna's uninstaller removes the profile only after its running check
    /// closed the window, and with the core stopped (windows/hooks.nsh).
    #[test]
    fn the_uninstaller_undoes_the_profile_once_the_window_and_the_core_are_gone() {
        let installer = include_str!("../../windows/installer.nsi");
        let section = &installer[installer.find("Section Uninstall").expect("the uninstall section")..];
        let checked = section.find("Call un.LunaCheckIfRunning").expect("the running check");
        let stopped = section.find("!insertmacro NSIS_HOOK_PREUNINSTALL").expect("the core's stop");
        let undone = section.find("Call un.LunaNvidiaUndo").expect("the undo");
        assert!(checked < stopped && stopped < undone, "the window closed, then the core stopped, then the undo");
        assert!(undone < section.find("Delete \"$INSTDIR\\${MAINBINARYNAME}.exe\"").expect("the exe removed"));
        let hooks = include_str!("../../windows/hooks.nsh");
        let undo = &hooks[hooks.find("Function un.LunaNvidiaUndo").expect("the undo's function")..];
        let undo = &undo[..undo.find("FunctionEnd").unwrap()];
        let run = undo.find("--nvidia-undo").expect("it runs the exe");
        for known in ["$LunaCoreStopped", "$LunaAppClosed"] {
            assert!(undo.find(known).is_some_and(|at| at < run), "{known} is checked first");
        }
        let check = &installer[installer.find("Function ${UN}LunaCheckIfRunning").expect("the check")..];
        assert!(check.contains("StrCpy $LunaAppClosed 1"), "the check says when the window is closed");
        assert!(hooks.contains("StrCpy $LunaCoreStopped 1"), "the core's stop says when it is stopped");
    }

    /// The layouts and versions nvapi.h gives (`MAKE_NVAPI_VERSION`): a
    /// size the driver does not expect is refused, or worse, misread.
    #[test]
    fn the_structures_are_nvapis_size() {
        assert_eq!(size_of::<nvapi::Setting>(), 12320);
        assert_eq!(size_of::<nvapi::Application>(), 20492);
        assert_eq!(size_of::<nvapi::Profile>(), 4116);
        assert_eq!(nvapi::SETTING_VER, 0x0001_3020);
        assert_eq!(nvapi::APPLICATION_VER, 0x0004_500C);
        assert_eq!(nvapi::PROFILE_VER, 0x0001_1014);
    }

    #[test]
    fn a_name_fits_an_nvapi_string_terminated() {
        let s = nvapi::unicode(r"C:\Luna\luna.exe");
        assert_eq!(nvapi::text(&s), r"C:\Luna\luna.exe");
        let long = "x".repeat(5000);
        let s = nvapi::unicode(&long);
        assert_eq!(s[2047], 0);
        assert_eq!(nvapi::text(&s).len(), 2047);
    }

    /// Read only: which profile this machine's NVIDIA driver applies to the
    /// exe `LUNA_GSYNC_PROBE` names (a full path), and what that profile
    /// holds. Run by hand on a Windows PC with an NVIDIA driver
    /// (`--ignored`); it changes nothing.
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn probe_the_drivers_profile_for_an_exe() {
        let exe = std::env::var("LUNA_GSYNC_PROBE").expect("LUNA_GSYNC_PROBE names an exe");
        let api = nvapi::Api::load().expect("nvapi64.dll in System32");
        let session = api.session().expect("a session on the driver's profiles");
        let found = session.resolve(&app_path(Path::new(&exe)).expect("a path NVAPI takes")).expect("the lookup answers");
        println!("{exe}: {found:?}");
        if let Some(found) = found {
            let profile = session.find_profile(&found.profile).expect("the lookup answers").expect("the profile");
            println!("{}: {:?}", found.profile, session.held(profile));
        }
    }

    #[test]
    fn no_driver_is_no_hint() {
        assert!(nvapi::no_nvidia(nvapi::LIBRARY_NOT_FOUND) && nvapi::no_nvidia(nvapi::NVIDIA_DEVICE_NOT_FOUND));
        assert!(!nvapi::no_nvidia(nvapi::INVALID_USER_PRIVILEGE));
        let why = nvapi::why(Fault::at("NvAPI_DRS_SaveSettings", nvapi::INVALID_USER_PRIVILEGE));
        assert!(why.contains("administrator") && why.contains("NvAPI_DRS_SaveSettings: NVAPI_INVALID_USER_PRIVILEGE (-137)"), "{why}");
        assert!(hint("x").contains("Fixed Refresh"));
    }

    /// A failure names its call and status, and the log has every step
    /// before it, a run of one call counted once.
    #[test]
    fn a_failure_names_its_step_and_status() {
        let fault = Fault::at("NvAPI_DRS_LoadSettings", -1);
        assert_eq!(nvapi::why(fault), "NvAPI_DRS_LoadSettings: NVAPI_ERROR (-1)");
        assert_eq!(nvapi::name(-9999), "an NVAPI status");
        let unnamed = nvapi::Trail::default();
        unnamed.note("NvAPI_DRS_LoadSettings", -104);
        assert_eq!(unnamed.text(), "NvAPI_DRS_LoadSettings an NVAPI status (-104)");
        let trail = nvapi::Trail::default();
        trail.note("NvAPI_Initialize", nvapi::OK);
        for _ in 0..3 {
            trail.note("NvAPI_DRS_EnumApplications", nvapi::OK);
        }
        trail.note("NvAPI_DRS_EnumApplications", nvapi::END_ENUMERATION);
        assert_eq!(
            trail.check("NvAPI_DRS_CreateApplication", nvapi::EXECUTABLE_ALREADY_IN_USE),
            Err(Fault::at("NvAPI_DRS_CreateApplication", -167))
        );
        assert_eq!(trail.check("NvAPI_DRS_SetSetting", nvapi::OK), Ok(()));
        assert_eq!(
            trail.text(),
            "NvAPI_Initialize NVAPI_OK, NvAPI_DRS_EnumApplications NVAPI_OK ×3, NvAPI_DRS_EnumApplications NVAPI_END_ENUMERATION (-7), \
             NvAPI_DRS_CreateApplication NVAPI_EXECUTABLE_ALREADY_IN_USE (-167), NvAPI_DRS_SetSetting NVAPI_OK"
        );
    }
}
