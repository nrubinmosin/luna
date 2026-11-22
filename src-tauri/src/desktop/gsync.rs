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
//! entry, and nothing is saved.
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
//! (not an update's) removes that profile while it holds only what Luna put
//! there, else only Luna's entries. A record is acted on only where it is a
//! private file of this process's own owner in the data folder, neither it
//! nor the folder a link (`FileStore`), of this build's flavor, naming a
//! profile of Luna's naming. Where the driver refuses (it may ask for
//! administrator rights, `NVAPI_INVALID_USER_PRIVILEGE`) or anything else
//! fails, the window logs one line and shows a one-line hint pointing to the
//! user guide's Fixed Refresh setting. The test build only reads, unless
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
use std::time::Duration;

pub use nvapi::Status;

/// "G-SYNC - Application State" (`VRR_APP_OVERRIDE_ID`, "G-SYNC").
pub const VRR_APP_OVERRIDE: u32 = 0x10A8_79CF;
/// Its "Force Off" (`VRR_APP_OVERRIDE_FORCE_OFF`).
pub const FORCE_OFF: u32 = 1;
/// The start of the name of the profile Luna makes; a space and eight hex
/// digits, random, follow, so neither NVIDIA's profiles, a person's nor
/// another copy of Luna's share it.
const PREFIX: &str = "Luna (native window, G-SYNC off)";
/// The test build's, so that its live test and a Luna in use never act on
/// each other's.
const TEST_PREFIX: &str = "Luna test build (G-SYNC off)";
/// What Luna made, in its data folder, for the undo.
const RECORD: &str = "nvidia-profile.json";
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

/// Whether `name` is one `new_profile` makes.
fn ours(name: &str, test: bool) -> bool {
    name.strip_prefix(prefix(test)).and_then(|tail| tail.strip_prefix(' ')).is_some_and(|tail| hex(tail, 8))
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

    /// Whether `entry` is one Luna made, for a path this record has.
    fn made(&self, entry: &Entry) -> bool {
        self.entries().any(|app| entry.is_lunas(app, &self.marker))
    }

    /// Whether `held` is the profile this record names, Luna's own: not
    /// predefined, with an entry of Luna's making in it.
    fn owns(&self, held: &Held) -> bool {
        !held.predefined && held.apps.iter().any(|e| self.made(e))
    }
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
}

/// The record in the data folder `0`. Acted on only where the folder is no
/// link or junction (one would bring a record from elsewhere) and the
/// record a private file of one name this process's owner holds, opened
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
    /// included.
    pub fn lock(&self, wait: Option<Duration>) -> Result<std::fs::File, String> {
        self.folder()?;
        let file = std::fs::File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.0.join(LOCK))
            .map_err(|e| format!("{LOCK} does not open ({e})"))?;
        let Some(wait) = wait else {
            file.lock().map_err(|e| format!("{LOCK} is not taken ({e})"))?;
            return Ok(file);
        };
        let until = std::time::Instant::now() + wait;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(file),
                Err(std::fs::TryLockError::WouldBlock) if std::time::Instant::now() < until => {
                    std::thread::sleep(Duration::from_millis(100))
                }
                Err(std::fs::TryLockError::WouldBlock) => return Err("another Luna is changing its NVIDIA profile".into()),
                Err(std::fs::TryLockError::Error(e)) => return Err(format!("{LOCK} is not taken ({e})")),
            }
        }
    }
}

impl Store for FileStore {
    fn load(&self) -> Result<Option<Record>, String> {
        self.folder()?;
        let Some(bytes) = crate::secret::read_own_private(&self.path(), RECORD_MAX).map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        serde_json::from_slice(&bytes).map(Some).map_err(|e| format!("{RECORD} does not read ({e})"))
    }

    fn save(&self, record: &Record) -> Result<(), String> {
        self.folder()?;
        let bytes = serde_json::to_vec_pretty(record).map_err(|e| e.to_string())?;
        crate::secret::write_private_synced(&self.path(), &bytes).map_err(|e| format!("{RECORD} was not written ({e})"))
    }

    fn clear(&self) -> Result<(), String> {
        self.folder()?;
        match std::fs::remove_file(self.path()) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("{RECORD} was not removed ({e})")),
            _ => Ok(()),
        }
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
    fn resolve(&self, app: &str) -> Result<Option<Resolved>, Status>;
    fn find_profile(&self, name: &str) -> Result<Option<Self::Profile>, Status>;
    fn held(&self, profile: Self::Profile) -> Result<Held, Status>;
    fn create_profile(&self, name: &str) -> Result<Self::Profile, Status>;
    fn add_app(&self, profile: Self::Profile, entry: &Entry) -> Result<(), Status>;
    fn delete_app(&self, profile: Self::Profile, app: &str) -> Result<(), Status>;
    /// `VRR_APP_OVERRIDE` at Force Off, as the profile's own.
    fn set_off(&self, profile: Self::Profile) -> Result<(), Status>;
    fn delete_setting(&self, profile: Self::Profile) -> Result<(), Status>;
    fn delete_profile(&self, profile: Self::Profile) -> Result<(), Status>;
    /// The session's changes, into the driver's database.
    fn save(&self) -> Result<(), Status>;
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
    /// Not off, and the test build, which only reads, left it.
    ReadOnly,
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
    let done = match record {
        Some(record) => add(drs, store, record, app, write),
        None => make(drs, store, app, write, test, fresh),
    };
    done.unwrap_or_else(Outcome::NotOff)
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
    match drs.resolve(app).map_err(nvapi::why)? {
        Some(found) if same_app(&found.entry.app, app) => return take_back(drs, store, found, test),
        Some(found) if found.value == Some(FORCE_OFF) => return Ok(Outcome::Off(found.profile)),
        _ => {}
    }
    if !write {
        return Ok(Outcome::ReadOnly);
    }
    let (name, marker) = fresh;
    let profile = drs.create_profile(name).map_err(nvapi::why)?;
    drs.add_app(profile, &Entry::lunas(app, marker)).map_err(nvapi::why)?;
    drs.set_off(profile).map_err(nvapi::why)?;
    applies(drs, app, name, marker, true)?;
    let mut record =
        Record { profile: name.clone(), marker: marker.clone(), test, created: false, apps: Vec::new(), pending: Some(app.to_owned()) };
    store.save(&record).map_err(|why| format!("{why}; nothing saved"))?;
    if let Err(status) = drs.save() {
        let _ = store.clear();
        return Err(nvapi::why(status));
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
    if let Some(found) = drs.resolve(app).map_err(nvapi::why)? {
        if same_app(&found.entry.app, app) {
            if found.profile != record.profile || !found.entry.is_lunas(app, &record.marker) {
                return Ok(Outcome::Theirs(found.profile));
            }
            // Luna's entry, by a save whose record was lost.
            record.apps.push(app.to_owned());
            confirm(store, &record);
            return Ok(Outcome::Kept(record.profile));
        }
    }
    if !write {
        return Ok(Outcome::ReadOnly);
    }
    drs.add_app(profile, &Entry::lunas(app, &record.marker)).map_err(nvapi::why)?;
    applies(drs, app, &record.profile, &record.marker, false)?;
    record.pending = Some(app.to_owned());
    store.save(&record).map_err(|why| format!("{why}; nothing saved"))?;
    if let Err(status) = drs.save() {
        record.pending = None;
        let _ = store.save(&record);
        return Err(nvapi::why(status));
    }
    record.apps.extend(record.pending.take());
    confirm(store, &record);
    Ok(Outcome::Added(record.profile))
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

/// What the undo removes from Luna's profile, holding `held`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UndoPlan {
    /// The whole profile: it holds only Luna's entries and setting.
    pub profile: bool,
    /// Else these entries of Luna's, as the profile names them.
    pub apps: Vec<String>,
    /// And the setting, which Luna set and no one changed, where no one
    /// else's exe is there for it to serve.
    pub setting: bool,
}

/// What the undo removes: only entries of Luna's making (`Record::made`);
/// the whole profile only where it is known to hold nothing else, an
/// enumeration cut short being no proof.
pub fn undo_plan(record: &Record, held: &Held) -> UndoPlan {
    let apps: Vec<String> = held.apps.iter().filter(|e| record.made(e)).map(|e| e.app.clone()).collect();
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
    /// Nothing of Luna's left there (no record, or its profile gone or no
    /// longer Luna's).
    Nothing,
    Done(UndoPlan),
    /// The record is not acted on, and why.
    Refused(String),
    /// The driver refused or failed.
    Failed(Status),
}

/// `undo`'s work over `drs` and `store`, the lock held. The record is
/// removed once what it names is gone; kept where the driver refused, for
/// another try.
pub fn undo_in<D: Drs>(drs: &D, store: &dyn Store, test: bool) -> Undone {
    let record = match store.load() {
        Ok(Some(record)) => record,
        Ok(None) => return Undone::Nothing,
        Err(why) => return Undone::Refused(why),
    };
    if let Some(why) = refusal(&record, test) {
        return Undone::Refused(why);
    }
    let run = || -> Result<Undone, Status> {
        let Some(profile) = drs.find_profile(&record.profile)? else { return Ok(Undone::Nothing) };
        let held = drs.held(profile)?;
        if !record.owns(&held) {
            // Never one of NVIDIA's, nor one someone made under that name.
            return Ok(Undone::Nothing);
        }
        let plan = undo_plan(&record, &held);
        if plan.profile {
            drs.delete_profile(profile)?;
        } else {
            for app in &plan.apps {
                drs.delete_app(profile, app)?;
            }
            if plan.setting {
                drs.delete_setting(profile)?;
            }
        }
        if plan != UndoPlan::default() {
            drs.save()?;
        }
        Ok(Undone::Done(plan))
    };
    let done = match run() {
        Ok(done) => done,
        Err(status) => return Undone::Failed(status),
    };
    if let Err(why) = store.clear() {
        // What it named is gone all the same; a record left behind finds
        // nothing to do the next time.
        crate::log::warn("app", &format!("luna --nvidia-undo: {why}; the profile is undone"));
    }
    done
}

/// The setting at Force Off for the running exe (module docs). `write`
/// false only reads (the test build). Blocks: NVAPI loads and parses the
/// driver's database, which takes a moment, and an undo under way holds
/// the lock.
#[cfg(windows)]
pub fn ensure(data: &Path, write: bool) -> Outcome {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => return Outcome::NotOff(format!("the exe's path did not read: {e}")),
    };
    let Some(app) = app_path(&exe) else {
        return Outcome::NotOff("the exe's path is no Unicode or longer than NVAPI takes; NVAPI is not asked about another".into());
    };
    let store = FileStore::in_data(data);
    let _lock = match store.lock(None) {
        Ok(lock) => lock,
        Err(why) => return Outcome::NotOff(why),
    };
    let Some(api) = nvapi::Api::load() else { return Outcome::NoNvidia };
    let session = match api.session() {
        Ok(s) => s,
        Err(status) if nvapi::no_nvidia(status) => return Outcome::NoNvidia,
        Err(status) => return Outcome::NotOff(nvapi::why(status)),
    };
    let test = crate::paths::is_test();
    ensure_in(&session, &store, &app, write, test, &new_profile(test))
}

#[cfg(not(windows))]
pub fn ensure(_data: &Path, _write: bool) -> Outcome {
    Outcome::NoNvidia
}

/// `--nvidia-undo`'s exit codes (windows/hooks.nsh): the driver asks for
/// administrator rights; the record is refused or the lock not had;
/// anything else that failed.
pub const UNDO_ELEVATION: i32 = 2;
pub const UNDO_REFUSED: i32 = 3;
pub const UNDO_FAILED: i32 = 1;

/// `luna --nvidia-undo`, for the uninstaller once the window and the core
/// are gone: removes what the record says Luna made, and the record. Where
/// it cannot, one line on stdout, which the uninstaller shows, and an exit
/// code of the above.
pub fn undo_from_args() -> bool {
    if std::env::args().nth(1).as_deref() != Some("--nvidia-undo") {
        return false;
    }
    let (code, line) = match undo(&crate::paths::data_dir()) {
        Undone::Nothing | Undone::Done(_) => (0, None),
        Undone::Refused(why) => (UNDO_REFUSED, Some(format!("Luna's NVIDIA profile was left as it is: {why}."))),
        Undone::Failed(status) => (
            if status == nvapi::INVALID_USER_PRIVILEGE { UNDO_ELEVATION } else { UNDO_FAILED },
            Some(format!(
                "Luna's G-SYNC profile stays in NVIDIA's settings: {}. Without luna.exe it does nothing; NVIDIA Control Panel, \
                 Manage 3D settings, Program Settings removes it.",
                nvapi::why(status)
            )),
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
/// the record (a window's start saves both under the same lock).
#[cfg(windows)]
fn undo(data: &Path) -> Undone {
    let store = FileStore::in_data(data);
    let _lock = match store.lock(Some(UNDO_WAIT)) {
        Ok(lock) => lock,
        Err(why) => return Undone::Refused(why),
    };
    match store.load() {
        Ok(None) => return Undone::Nothing,
        Err(why) => return Undone::Refused(why),
        Ok(Some(_)) => {}
    }
    let Some(api) = nvapi::Api::load() else { return Undone::Nothing };
    let undone = match api.session() {
        Ok(session) => undo_in(&session, &store, crate::paths::is_test()),
        Err(status) if nvapi::no_nvidia(status) => Undone::Nothing,
        Err(status) => Undone::Failed(status),
    };
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
    pub const LIBRARY_NOT_FOUND: Status = -2;
    pub const NVIDIA_DEVICE_NOT_FOUND: Status = -6;
    pub const END_ENUMERATION: Status = -7;
    pub const INVALID_USER_PRIVILEGE: Status = -137;
    pub const SETTING_NOT_FOUND: Status = -160;
    pub const PROFILE_NOT_FOUND: Status = -163;
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

    /// NVAPI's answer when there is no NVIDIA driver to talk to.
    pub fn no_nvidia(status: Status) -> bool {
        matches!(status, LIBRARY_NOT_FOUND | NVIDIA_DEVICE_NOT_FOUND)
    }

    /// Why, in words, for the log and the hint.
    pub fn why(status: Status) -> String {
        match status {
            INVALID_USER_PRIVILEGE => "the NVIDIA driver asks for administrator rights to change its profiles".into(),
            EXECUTABLE_ALREADY_IN_USE => "another NVIDIA profile has this exe by its full path".into(),
            s => format!("NVAPI error {s}"),
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
        type DeleteProfileSetting = unsafe extern "C" fn(Handle, Handle, u32) -> Status;
        type GetProfileInfo = unsafe extern "C" fn(Handle, Handle, *mut Profile) -> Status;
        type EnumApplications = unsafe extern "C" fn(Handle, Handle, u32, *mut u32, *mut Application) -> Status;
        type DeleteApplication = unsafe extern "C" fn(Handle, Handle, *const u16) -> Status;

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
            delete_profile_setting: DeleteProfileSetting,
            get_profile_info: GetProfileInfo,
            enum_applications: EnumApplications,
            delete_application: DeleteApplication,
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
                        delete_profile_setting: get!(DeleteProfileSetting, 0xE4A2_6362),
                        get_profile_info: get!(GetProfileInfo, 0x61CD_6FD6),
                        enum_applications: get!(EnumApplications, 0x7FA2_173A),
                        delete_application: get!(DeleteApplication, 0x2C69_4BC6),
                    };
                    api.init = initialize();
                    Some(api)
                }
            }

            /// A session with the driver's settings loaded.
            pub fn session(&self) -> Result<Session<'_>, Status> {
                check(self.init)?;
                let mut handle: Handle = std::ptr::null_mut();
                // SAFETY: NVAPI writes the session's handle.
                check(unsafe { (self.create_session)(&mut handle) })?;
                let session = Session { api: self, handle };
                // SAFETY: the session just made.
                check(unsafe { (self.load_settings)(handle) })?;
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

        fn check(status: Status) -> Result<(), Status> {
            if status == OK {
                Ok(())
            } else {
                Err(status)
            }
        }

        pub struct Session<'a> {
            api: &'a Api,
            handle: Handle,
        }

        impl Drop for Session<'_> {
            fn drop(&mut self) {
                // SAFETY: the session `api.session` made, destroyed once.
                unsafe { (self.api.destroy_session)(self.handle) };
            }
        }

        use crate::desktop::gsync::{Drs, Entry, Held, Resolved, FORCE_OFF, VRR_APP_OVERRIDE};

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
            fn setting(&self, profile: Handle) -> Result<(Option<u32>, Option<u32>), Status> {
                // SAFETY: a zeroed setting with its version, which NVAPI fills in.
                let mut setting = unsafe { zeroed::<Setting>() };
                setting.version = SETTING_VER;
                // SAFETY: a profile handle of this session.
                let status = unsafe { (self.api.get_setting)(self.handle, profile, VRR_APP_OVERRIDE, &mut *setting) };
                if status == SETTING_NOT_FOUND {
                    return Ok((None, None));
                }
                check(status)?;
                if setting.kind != 0 {
                    return Ok((None, None));
                }
                let value = setting.current[0];
                let own = setting.location == 0 && setting.is_current_predefined == 0;
                Ok((Some(value), own.then_some(value)))
            }

            fn info(&self, profile: Handle) -> Result<Box<Profile>, Status> {
                // SAFETY: a zeroed profile with its version, which NVAPI fills in.
                let mut info = unsafe { zeroed::<Profile>() };
                info.version = PROFILE_VER;
                // SAFETY: a profile handle of this session.
                check(unsafe { (self.api.get_profile_info)(self.handle, profile, &mut *info) })?;
                Ok(info)
            }

            /// The profile's application entries, one at a time: each is
            /// 20 KB.
            fn apps(&self, profile: Handle, count: u32) -> Result<Vec<Entry>, Status> {
                let mut apps = Vec::new();
                for index in 0..count.min(APPS_MAX) {
                    // SAFETY: a zeroed V4 application with its version, which
                    // NVAPI fills in.
                    let mut app = unsafe { zeroed::<Application>() };
                    app.version = APPLICATION_VER;
                    let mut n = 1u32;
                    // SAFETY: room for the one application `n` says.
                    match unsafe { (self.api.enum_applications)(self.handle, profile, index, &mut n, &mut *app) } {
                        END_ENUMERATION => break,
                        status => check(status)?,
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
            fn resolve(&self, app: &str) -> Result<Option<Resolved>, Status> {
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
                    return Ok(None);
                }
                check(status)?;
                let info = self.info(profile)?;
                let (value, _) = self.setting(profile)?;
                Ok(Some(Resolved { profile: text(&info.name), predefined: info.is_predefined != 0, entry: entry(&found), value }))
            }

            fn find_profile(&self, name: &str) -> Result<Option<Handle>, Status> {
                let name = unicode(name);
                let mut profile: Handle = std::ptr::null_mut();
                // SAFETY: the name is a full NvAPI_UnicodeString.
                match unsafe { (self.api.find_profile_by_name)(self.handle, name.as_ptr(), &mut profile) } {
                    PROFILE_NOT_FOUND => Ok(None),
                    status => check(status).map(|()| Some(profile)),
                }
            }

            fn held(&self, profile: Handle) -> Result<Held, Status> {
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

            fn create_profile(&self, name: &str) -> Result<Handle, Status> {
                // SAFETY: a zeroed profile with its version and name.
                let mut info = unsafe { zeroed::<Profile>() };
                info.version = PROFILE_VER;
                info.name = *unicode(name);
                let mut profile: Handle = std::ptr::null_mut();
                // SAFETY: NVAPI writes the new profile's handle.
                check(unsafe { (self.api.create_profile)(self.handle, &mut *info, &mut profile) })?;
                Ok(profile)
            }

            /// `entry`'s names; the rest of it is Luna's bare entry, zeroed.
            fn add_app(&self, profile: Handle, entry: &Entry) -> Result<(), Status> {
                // SAFETY: a zeroed V4 application with its version and names.
                let mut app = unsafe { zeroed::<Application>() };
                app.version = APPLICATION_VER;
                app.app_name = *unicode(&entry.app);
                app.friendly_name = *unicode(&entry.friendly);
                // SAFETY: a profile handle of this session.
                check(unsafe { (self.api.create_application)(self.handle, profile, &mut *app) })
            }

            fn delete_app(&self, profile: Handle, app: &str) -> Result<(), Status> {
                let name = unicode(app);
                // SAFETY: a profile handle of this session; the name is a
                // full NvAPI_UnicodeString.
                check(unsafe { (self.api.delete_application)(self.handle, profile, name.as_ptr()) })
            }

            fn set_off(&self, profile: Handle) -> Result<(), Status> {
                // SAFETY: a zeroed setting with its version, id, type and value.
                let mut setting = unsafe { zeroed::<Setting>() };
                setting.version = SETTING_VER;
                setting.id = VRR_APP_OVERRIDE;
                setting.current[0] = FORCE_OFF;
                // SAFETY: a profile handle of this session.
                check(unsafe { (self.api.set_setting)(self.handle, profile, &mut *setting) })
            }

            fn delete_setting(&self, profile: Handle) -> Result<(), Status> {
                // SAFETY: a profile handle of this session.
                check(unsafe { (self.api.delete_profile_setting)(self.handle, profile, VRR_APP_OVERRIDE) })
            }

            fn delete_profile(&self, profile: Handle) -> Result<(), Status> {
                // SAFETY: a profile handle of this session.
                check(unsafe { (self.api.delete_profile)(self.handle, profile) })
            }

            fn save(&self) -> Result<(), Status> {
                // SAFETY: this session.
                check(unsafe { (self.api.save_settings)(self.handle) })
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
    const NAME: &str = "Luna (native window, G-SYNC off) 0a1b2c3d";
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
    }

    fn p(name: &str, predefined: bool, apps: &[Entry], settings: &[(u32, u32)]) -> P {
        P { name: name.into(), predefined, apps: apps.to_vec(), settings: settings.to_vec() }
    }

    /// Luna's own profile, holding Luna's entries for `apps`.
    fn lunas(apps: &[&str], settings: &[(u32, u32)]) -> P {
        p(NAME, false, &apps.iter().map(|a| ours_for(a)).collect::<Vec<_>>(), settings)
    }

    /// NVIDIA's "Luna" profile, for any `luna.exe` by its bare name.
    fn nvidias() -> P {
        let mut entry = theirs("luna.exe");
        entry.predefined = true;
        p("Luna", true, &[entry], &[])
    }

    type Journal = Rc<RefCell<Vec<String>>>;

    /// The driver's database: what is saved, and a session's copy of it.
    struct Fake {
        saved: RefCell<Vec<Option<P>>>,
        session: RefCell<Vec<Option<P>>>,
        /// What the global profile sets for `VRR_APP_OVERRIDE`.
        global: Option<u32>,
        /// A driver that would match a bare name before a full path.
        bare_first: bool,
        /// Enumerations that stop short of the profile's count.
        cut_short: bool,
        save_fails: Option<Status>,
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
                bare_first: false,
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
                bare_first: self.bare_first,
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

        fn with(&self, i: usize, f: impl FnOnce(&mut P)) -> Result<(), Status> {
            match self.session.borrow_mut().get_mut(i) {
                Some(Some(p)) => {
                    f(p);
                    Ok(())
                }
                _ => Err(nvapi::PROFILE_NOT_FOUND),
            }
        }
    }

    fn file_name(app: &str) -> &str {
        app.rsplit('/').next().unwrap_or(app)
    }

    impl Drs for Fake {
        type Profile = usize;

        fn resolve(&self, app: &str) -> Result<Option<Resolved>, Status> {
            let db = self.session.borrow();
            let full = db.iter().flatten().find_map(|p| p.apps.iter().find(|e| same_app(&e.app, app)).map(|e| (p, e)));
            let bare = db
                .iter()
                .flatten()
                .find_map(|p| p.apps.iter().find(|e| !e.app.contains('/') && same_app(&e.app, file_name(app))).map(|e| (p, e)));
            let hit = if self.bare_first { bare.or(full) } else { full.or(bare) };
            Ok(hit.map(|(p, e)| Resolved {
                profile: p.name.clone(),
                predefined: p.predefined,
                entry: e.clone(),
                value: p.settings.iter().find(|(id, _)| *id == VRR_APP_OVERRIDE).map(|(_, v)| *v).or(self.global),
            }))
        }

        fn find_profile(&self, name: &str) -> Result<Option<usize>, Status> {
            Ok(self.session.borrow().iter().position(|p| p.as_ref().is_some_and(|p| p.name == name)))
        }

        fn held(&self, i: usize) -> Result<Held, Status> {
            let db = self.session.borrow();
            let p = db[i].as_ref().ok_or(nvapi::PROFILE_NOT_FOUND)?;
            let apps: Vec<Entry> = if self.cut_short { p.apps.iter().take(1).cloned().collect() } else { p.apps.clone() };
            Ok(Held {
                predefined: p.predefined,
                complete: apps.len() == p.apps.len(),
                apps,
                settings: p.settings.len() as u32,
                own_value: p.settings.iter().find(|(id, _)| *id == VRR_APP_OVERRIDE).map(|(_, v)| *v),
            })
        }

        fn create_profile(&self, name: &str) -> Result<usize, Status> {
            if self.find_profile(name)?.is_some() {
                return Err(-164);
            }
            let mut db = self.session.borrow_mut();
            db.push(Some(p(name, false, &[], &[])));
            Ok(db.len() - 1)
        }

        fn add_app(&self, i: usize, entry: &Entry) -> Result<(), Status> {
            if self.session.borrow().iter().flatten().any(|p| p.apps.iter().any(|e| same_app(&e.app, &entry.app))) {
                return Err(nvapi::EXECUTABLE_ALREADY_IN_USE);
            }
            self.with(i, |p| p.apps.push(entry.clone()))
        }

        fn delete_app(&self, i: usize, app: &str) -> Result<(), Status> {
            self.with(i, |p| p.apps.retain(|e| e.app != app))
        }

        fn set_off(&self, i: usize) -> Result<(), Status> {
            self.with(i, |p| {
                p.settings.retain(|(id, _)| *id != VRR_APP_OVERRIDE);
                p.settings.push((VRR_APP_OVERRIDE, FORCE_OFF));
            })
        }

        fn delete_setting(&self, i: usize) -> Result<(), Status> {
            self.with(i, |p| p.settings.retain(|(id, _)| *id != VRR_APP_OVERRIDE))
        }

        fn delete_profile(&self, i: usize) -> Result<(), Status> {
            self.with(i, |_| ())?;
            self.session.borrow_mut()[i] = None;
            Ok(())
        }

        fn save(&self) -> Result<(), Status> {
            self.journal.borrow_mut().push("save".into());
            if let Some(status) = self.save_fails {
                return Err(status);
            }
            *self.saved.borrow_mut() = self.session.borrow().clone();
            self.saves.set(self.saves.get() + 1);
            Ok(())
        }
    }

    struct Mem {
        record: RefCell<Option<Record>>,
        refuse: Option<String>,
        clear_fails: bool,
        journal: Journal,
    }

    impl Mem {
        fn new(record: Option<Record>, journal: &Journal) -> Mem {
            Mem { record: RefCell::new(record), refuse: None, clear_fails: false, journal: journal.clone() }
        }
        fn get(&self) -> Option<Record> {
            self.record.borrow().clone()
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
    fn lunas_own_profile_is_made_and_nvidias_left_as_it_is() {
        let journal = Journal::default();
        let drs = Fake::new(vec![nvidias()], &journal);
        let store = Mem::new(None, &journal);
        assert_eq!(ensure(&drs, &store, A), Outcome::Made(NAME.into()));
        assert_eq!(drs.saved("Luna"), Some(nvidias()));
        assert_eq!(drs.saved(NAME), Some(lunas(&[A], &[(VRR_APP_OVERRIDE, FORCE_OFF)])));
        assert_eq!(store.get(), Some(confirmed(&[A])));
        // Ahead of the save, confirmed after it.
        assert_eq!(*journal.borrow(), ["record pending", "save", "record confirmed"]);
        // The driver now applies Luna's profile to this exe, and only to it.
        assert_eq!(drs.reopen().resolve(A).unwrap().unwrap().profile, NAME);
        assert_eq!(drs.reopen().resolve("E:/Games/Luna/luna.exe").unwrap().unwrap().profile, "Luna");
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
        let mut drs = Fake::new(vec![nvidias()], &journal);
        drs.bare_first = true;
        let store = Mem::new(None, &journal);
        assert!(matches!(ensure(&drs, &store, A), Outcome::NotOff(why) if why.contains("the profile \"Luna\"")));
        assert_eq!(drs.saves.get(), 0);
        assert_eq!(drs.profiles(), [nvidias()]);
        assert_eq!(store.get(), None);
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
        let drs = Fake::new(vec![nvidias(), named], &journal);
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
        let drs = Fake::new(vec![nvidias(), lunas(&[A, B], &[(VRR_APP_OVERRIDE, 0)])], &journal);
        let store = Mem::new(None, &journal);
        assert_eq!(ensure(&drs, &store, A), Outcome::Kept(NAME.into()));
        assert_eq!(store.get(), Some(confirmed(&[A, B])));
        assert_eq!(drs.saves.get(), 0);
        assert!(matches!(undo_in(&drs, &store, false), Undone::Done(UndoPlan { profile: true, .. })));
        assert_eq!(drs.profiles(), [nvidias()]);
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
        let drs = Fake::new(vec![nvidias()], &journal);
        let store = Mem::new(None, &journal);
        assert_eq!(ensure_in(&drs, &store, A, false, true, &new_profile(true)), Outcome::ReadOnly);
        assert_eq!(drs.saves.get(), 0);
        let made = new_profile(true);
        assert!(made.0.starts_with(TEST_PREFIX) && ours(&made.0, true) && !ours(&made.0, false) && hex(&made.1, 16));
        assert_eq!(ensure_in(&drs, &store, A, true, true, &made), Outcome::Made(made.0.clone()));
        assert!(store.get().unwrap().test);
    }

    #[test]
    fn it_is_set_once_and_a_later_choice_stands() {
        let journal = Journal::default();
        let drs = Fake::new(vec![nvidias()], &journal);
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
        let gone = Fake::new(vec![nvidias()], &journal);
        assert_eq!(ensure(&gone, &Mem::new(Some(confirmed(&[A])), &journal), B), Outcome::Gone(NAME.into()));
        assert_eq!(gone.saves.get(), 0);
    }

    #[test]
    fn an_exe_that_moved_is_added_and_the_undo_cleans_both() {
        let journal = Journal::default();
        let drs = Fake::new(vec![nvidias(), lunas(&[A], &[(VRR_APP_OVERRIDE, 0)])], &journal);
        let store = Mem::new(Some(confirmed(&[A])), &journal);
        assert_eq!(ensure(&drs, &store, B), Outcome::Added(NAME.into()));
        // The person's value stands for the new path too.
        assert_eq!(drs.saved(NAME), Some(lunas(&[A, B], &[(VRR_APP_OVERRIDE, 0)])));
        assert_eq!(store.get(), Some(confirmed(&[A, B])));
        assert_eq!(*journal.borrow(), ["record pending", "save", "record confirmed"]);
        let next = drs.reopen();
        assert_eq!(undo_in(&next, &store, false), Undone::Done(UndoPlan { profile: true, ..UndoPlan::default() }));
        assert_eq!(next.profiles(), [nvidias()]);
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
            let drs = Fake::new(vec![nvidias(), replaced.clone()], &journal);
            // The moved exe is not saved into it.
            let store = Mem::new(Some(confirmed(&[A])), &journal);
            assert_eq!(ensure(&drs, &store, B), Outcome::Gone(NAME.into()), "{replaced:?}");
            // The undo removes nothing of it.
            assert_eq!(undo_in(&drs, &store, false), Undone::Nothing, "{replaced:?}");
            assert_eq!(drs.profiles(), [nvidias(), replaced.clone()]);
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
        let drs = Fake::new(vec![nvidias(), p(NAME, false, &[ours_for(A), theirs(B)], &[(VRR_APP_OVERRIDE, FORCE_OFF)])], &journal);
        let store = Mem::new(Some(confirmed(&[A])), &journal);
        assert_eq!(ensure(&drs, &store, B), Outcome::Theirs(NAME.into()));
        assert_eq!(store.get(), Some(confirmed(&[A])));
        // Not claimed by a recovery either.
        let pending = Record { pending: Some(B.into()), ..confirmed(&[A]) };
        assert_eq!(recover(&drs, &Mem::new(None, &journal), pending), Ok(Some(confirmed(&[A]))));
        // The undo takes out Luna's entry only, and leaves the setting to
        // the person's exe.
        assert_eq!(undo_in(&drs, &store, false), Undone::Done(UndoPlan { profile: false, apps: vec![A.into()], setting: false }));
        assert_eq!(drs.saved(NAME), Some(p(NAME, false, &[theirs(B)], &[(VRR_APP_OVERRIDE, FORCE_OFF)])));
        // Even recorded as Luna's, an entry changed since (a launcher, a
        // command line) is no longer what Luna made.
        let changed = Entry { command_line: "-x".into(), ..ours_for(B) };
        let drs = Fake::new(vec![p(NAME, false, &[ours_for(A), changed.clone()], &[(VRR_APP_OVERRIDE, FORCE_OFF)])], &journal);
        let store = Mem::new(Some(confirmed(&[A, B])), &journal);
        assert!(matches!(undo_in(&drs, &store, false), Undone::Done(UndoPlan { profile: false, .. })));
        assert_eq!(drs.saved(NAME).unwrap().apps, [changed]);
    }

    #[test]
    fn an_enumeration_cut_short_never_removes_the_profile() {
        let journal = Journal::default();
        let mut drs = Fake::new(vec![lunas(&[A, B], &[(VRR_APP_OVERRIDE, FORCE_OFF)])], &journal);
        drs.cut_short = true;
        let store = Mem::new(Some(confirmed(&[A, B])), &journal);
        assert_eq!(undo_in(&drs, &store, false), Undone::Done(UndoPlan { profile: false, apps: vec![A.into()], setting: false }));
        assert_eq!(drs.saved(NAME), Some(lunas(&[B], &[(VRR_APP_OVERRIDE, FORCE_OFF)])));
    }

    #[test]
    fn a_pending_record_is_settled_by_what_the_database_holds() {
        let journal = Journal::default();
        let pending = |created: bool, apps: &[&str], app: &str| Record { created, pending: Some(app.into()), ..confirmed(apps) };
        // Ended before the save: nothing is there, so the record goes and
        // the profile is made afresh, under a new name.
        let drs = Fake::new(vec![nvidias()], &journal);
        let store = Mem::new(Some(pending(false, &[], A)), &journal);
        let other = ("Luna (native window, G-SYNC off) ffff0000".to_owned(), "ffffffffffffffff".to_owned());
        assert_eq!(ensure_in(&drs, &store, A, true, false, &other), Outcome::Made(other.0.clone()));
        assert_eq!(drs.profiles().len(), 2);
        // Ended after the save: confirmed, and not set again.
        let drs = Fake::new(vec![nvidias(), lunas(&[A], &[(VRR_APP_OVERRIDE, FORCE_OFF)])], &journal);
        let store = Mem::new(Some(pending(false, &[], A)), &journal);
        assert_eq!(ensure(&drs, &store, A), Outcome::Kept(NAME.into()));
        assert_eq!(store.get(), Some(confirmed(&[A])));
        assert_eq!(drs.saves.get(), 0);
        // A moved exe's path not saved: dropped, then added.
        let drs = Fake::new(vec![nvidias(), lunas(&[A], &[(VRR_APP_OVERRIDE, FORCE_OFF)])], &journal);
        let store = Mem::new(Some(pending(true, &[A], B)), &journal);
        assert_eq!(ensure(&drs, &store, B), Outcome::Added(NAME.into()));
        assert_eq!(store.get(), Some(confirmed(&[A, B])));
        // Even the read-only test build settles the record, writing no profile.
        let drs = Fake::new(vec![nvidias(), lunas(&[A, B], &[(VRR_APP_OVERRIDE, FORCE_OFF)])], &journal);
        let store = Mem::new(Some(pending(true, &[A], B)), &journal);
        assert_eq!(ensure_in(&drs, &store, B, false, false, &fresh()), Outcome::Kept(NAME.into()));
        assert_eq!(store.get(), Some(confirmed(&[A, B])));
        // And the undo counts a pending entry of Luna's as Luna's.
        let drs = Fake::new(vec![nvidias(), lunas(&[A], &[(VRR_APP_OVERRIDE, FORCE_OFF)])], &journal);
        let store = Mem::new(Some(pending(false, &[], A)), &journal);
        assert!(matches!(undo_in(&drs, &store, false), Undone::Done(UndoPlan { profile: true, .. })));
        assert_eq!(drs.profiles(), [nvidias()]);
    }

    #[test]
    fn a_refused_save_leaves_neither_profile_nor_record() {
        let journal = Journal::default();
        let mut drs = Fake::new(vec![nvidias()], &journal);
        drs.save_fails = Some(nvapi::INVALID_USER_PRIVILEGE);
        let store = Mem::new(None, &journal);
        assert!(matches!(ensure(&drs, &store, A), Outcome::NotOff(why) if why.contains("administrator")));
        assert_eq!(drs.profiles(), [nvidias()]);
        assert_eq!(store.get(), None);
        // A path to add, refused: the record as it was.
        let mut drs = Fake::new(vec![nvidias(), lunas(&[A], &[(VRR_APP_OVERRIDE, FORCE_OFF)])], &journal);
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
            let drs = Fake::new(vec![nvidias(), profile.clone()], &journal);
            let store = Mem::new(Some(record.clone()), &journal);
            assert!(matches!(ensure_in(&drs, &store, B, true, test, &fresh()), Outcome::NotOff(_)), "{record:?}");
            assert!(matches!(undo_in(&drs, &store, test), Undone::Refused(_)), "{record:?}");
            assert_eq!(drs.profiles(), [nvidias(), profile.clone()]);
            assert_eq!(store.get(), Some(record));
        }
        // One the store will not read (not this user's, a link).
        let drs = Fake::new(vec![nvidias(), profile.clone()], &journal);
        let store = Mem { refuse: Some("not this user's alone".into()), ..Mem::new(None, &journal) };
        assert!(matches!(ensure(&drs, &store, A), Outcome::NotOff(why) if why.contains("not this user's")));
        assert!(matches!(undo_in(&drs, &store, false), Undone::Refused(_)));
        assert_eq!(drs.saves.get(), 0);
    }

    #[test]
    fn the_undo_leaves_what_a_person_added() {
        let record = confirmed(&[A]);
        let held =
            |apps: &[Entry], settings: u32, own_value| Held { predefined: false, apps: apps.to_vec(), complete: true, settings, own_value };
        let whole = UndoPlan { profile: true, ..UndoPlan::default() };
        assert_eq!(undo_plan(&record, &held(&[ours_for(A)], 1, Some(FORCE_OFF))), whole);
        // Luna's setting changed by the person: still only Luna's.
        assert_eq!(undo_plan(&record, &held(&[ours_for("c:/apps/luna/LUNA.EXE")], 1, Some(0))), whole);
        // A setting of the person's: Luna's entry and setting go.
        assert_eq!(
            undo_plan(&record, &held(&[ours_for(A)], 2, Some(FORCE_OFF))),
            UndoPlan { profile: false, apps: vec![A.into()], setting: true }
        );
        // ... but not a value the person chose.
        assert_eq!(
            undo_plan(&record, &held(&[ours_for(A)], 2, Some(0))),
            UndoPlan { profile: false, apps: vec![A.into()], setting: false }
        );
        // An exe of the person's: Luna's entry goes, the setting serves theirs.
        assert_eq!(
            undo_plan(&record, &held(&[ours_for(A), theirs("C:/x.exe")], 1, Some(FORCE_OFF))),
            UndoPlan { profile: false, apps: vec![A.into()], setting: false }
        );

        let journal = Journal::default();
        let drs = Fake::new(vec![nvidias(), lunas(&[A], &[(VRR_APP_OVERRIDE, FORCE_OFF), (OTHER_SETTING, 7)])], &journal);
        let store = Mem::new(Some(record.clone()), &journal);
        assert!(matches!(undo_in(&drs, &store, false), Undone::Done(_)));
        assert_eq!(drs.saved(NAME), Some(p(NAME, false, &[], &[(OTHER_SETTING, 7)])));
        assert_eq!(store.get(), None);
        // Nothing left is nothing to do.
        let drs = Fake::new(vec![nvidias()], &journal);
        let store = Mem::new(Some(record), &journal);
        assert_eq!(undo_in(&drs, &store, false), Undone::Nothing);
        assert_eq!(store.get(), None);
        // A refusal keeps the record for another try.
        let mut drs = Fake::new(vec![nvidias(), lunas(&[A], &[(VRR_APP_OVERRIDE, FORCE_OFF)])], &journal);
        drs.save_fails = Some(nvapi::INVALID_USER_PRIVILEGE);
        let store = Mem::new(Some(confirmed(&[A])), &journal);
        assert_eq!(undo_in(&drs, &store, false), Undone::Failed(nvapi::INVALID_USER_PRIVILEGE));
        assert_eq!(store.get(), Some(confirmed(&[A])));
    }

    #[test]
    fn a_record_that_will_not_go_does_not_unsay_the_undo() {
        let journal = Journal::default();
        let drs = Fake::new(vec![nvidias(), lunas(&[A], &[(VRR_APP_OVERRIDE, FORCE_OFF)])], &journal);
        let store = Mem { clear_fails: true, ..Mem::new(Some(confirmed(&[A])), &journal) };
        assert_eq!(undo_in(&drs, &store, false), Undone::Done(UndoPlan { profile: true, ..UndoPlan::default() }));
        assert_eq!(drs.profiles(), [nvidias()]);
        // The record left finds nothing the next time.
        assert_eq!(undo_in(&drs.reopen(), &Mem::new(store.get(), &journal), false), Undone::Nothing);
    }

    #[test]
    fn the_record_reads_as_written() {
        let record = Record { pending: Some(B.into()), ..confirmed(&[A]) };
        let text = serde_json::to_string(&record).unwrap();
        assert_eq!(
            text,
            r#"{"profile":"Luna (native window, G-SYNC off) 0a1b2c3d","marker":"00112233445566ff","test":false,"created":true,"apps":["C:/Apps/Luna/luna.exe"],"pending":"D:/Portable/Luna/luna.exe"}"#
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

    #[test]
    fn the_file_store_keeps_the_record_private() {
        let dir = scratch("store");
        let store = FileStore::in_data(&dir);
        assert_eq!(store.load(), Ok(None));
        store.save(&confirmed(&[A])).unwrap();
        assert_eq!(store.load(), Ok(Some(confirmed(&[A]))));
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
        assert!(store.load().is_ok());
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
        let undone = section.find("Call un.LunaNvidiaUndo").expect("the undo");
        assert!(section.find("!insertmacro NSIS_HOOK_PREUNINSTALL").unwrap() < checked && checked < undone);
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
        assert!(nvapi::why(nvapi::INVALID_USER_PRIVILEGE).contains("administrator"));
        assert!(hint("x").contains("Fixed Refresh"));
    }
}
