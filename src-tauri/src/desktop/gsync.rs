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
//! So the window, as it starts on Windows with an NVIDIA driver, makes sure
//! the profile its exe resolves to has that one setting at Force Off: the
//! profile the driver applies to the exe (NVIDIA ships one named "Luna" for
//! any `luna.exe`, a game's, with nothing of G-SYNC in it), else Luna's own,
//! `OWN_PROFILE`, made with the exe in it. It writes only where the value
//! differs, nothing but that setting, and records what it changed in
//! `<data>/nvidia-profile.json`; the uninstaller's `luna --nvidia-undo` puts
//! back exactly that (the setting's earlier value in that profile, or Luna's
//! own profile removed), not on an update. Where the driver refuses (it may
//! ask for administrator rights, `NVAPI_INVALID_USER_PRIVILEGE`) or anything
//! else fails, the window logs one line and shows a one-line hint pointing to
//! the user guide's Fixed Refresh setting. The test build only reads.
//!
//! NVAPI is the driver's library, loaded at run time from System32
//! (`nvapi64.dll`, through its one export, `nvapi_QueryInterface`), no SDK
//! linked: the interface ids, status values and structure layouts below are
//! those of NVIDIA's MIT-licensed headers (github.com/NVIDIA/nvapi:
//! nvapi.h, nvapi_interface.h, nvapi_lite_common.h, NvApiDriverSettings.h).

use serde::{Deserialize, Serialize};
use std::path::Path;

/// "G-SYNC - Application State" (`VRR_APP_OVERRIDE_ID`, "G-SYNC").
pub const VRR_APP_OVERRIDE: u32 = 0x10A8_79CF;
/// Its "Force Off" (`VRR_APP_OVERRIDE_FORCE_OFF`).
pub const FORCE_OFF: u32 = 1;
/// The profile Luna makes for its exe where no profile has it.
pub const OWN_PROFILE: &str = "Luna (native window)";
/// What Luna changed, in its data folder, for the undo.
const RECORD: &str = "nvidia-profile.json";

/// The profile the driver applies to the exe, and the setting there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub profile: String,
    /// The setting as the driver applies it to the exe: the profile's own
    /// or inherited (the global or base profile's); None when nothing sets
    /// it.
    pub value: Option<u32>,
    /// The value the profile holds as a user's setting of its own (not
    /// predefined, not inherited): what the undo puts back.
    pub own_value: Option<u32>,
}

/// What `ensure` does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    /// Off already.
    Nothing,
    /// Set it in the profile the exe resolves to.
    Set { profile: String },
    /// No profile has the exe: Luna's own, found or made, with the exe in it.
    Own,
}

pub fn plan(found: Option<&Found>) -> Plan {
    match found {
        Some(f) if f.value == Some(FORCE_OFF) => Plan::Nothing,
        Some(f) => Plan::Set { profile: f.profile.clone() },
        None => Plan::Own,
    }
}

/// What Luna changed in the driver's profiles (`<data>/nvidia-profile.json`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub profile: String,
    /// Luna's own profile: the undo removes it whole.
    pub own: bool,
    /// The setting's own value in that profile before Luna first set it;
    /// None when the profile had none of its own.
    pub previous: Option<u32>,
}

/// The record after Luna set the setting in `profile`, which held
/// `previous` of its own: an earlier record of the same profile keeps the
/// value from before Luna's first change, not one Luna wrote itself.
pub fn record_after(old: Option<Record>, profile: &str, previous: Option<u32>) -> Record {
    let previous = match old {
        Some(old) if old.profile == profile => old.previous,
        _ => previous,
    };
    Record { profile: profile.to_owned(), own: profile == OWN_PROFILE, previous }
}

/// What `undo` does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Undo {
    Nothing,
    /// Luna's own profile, removed.
    DeleteProfile,
    /// The profile's earlier value of its own, put back.
    Restore(u32),
    /// The profile's own value removed: the predefined or inherited one
    /// applies again.
    DeleteSetting,
}

/// The undo of `record`, the profile it names holding `own_value` of its
/// own now, or gone (`exists`). A value someone else changed since Luna set
/// it is theirs, and stays. Only a profile of Luna's own name is ever
/// removed, whatever the record says.
pub fn undo_plan(record: &Record, exists: bool, own_value: Option<u32>) -> Undo {
    let own = record.own && record.profile == OWN_PROFILE;
    match (exists, own, own_value) {
        (false, _, _) => Undo::Nothing,
        (true, true, _) => Undo::DeleteProfile,
        (true, false, Some(FORCE_OFF)) => record.previous.map_or(Undo::DeleteSetting, Undo::Restore),
        (true, false, _) => Undo::Nothing,
    }
}

/// How `ensure` ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// No NVIDIA driver (or no NVIDIA GPU driving a display): nothing to do.
    NoNvidia,
    /// Force Off already, in this profile.
    Off(String),
    /// Set to Force Off now, in this profile.
    Set(String),
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

fn read_record(data: &Path) -> Option<Record> {
    let text = std::fs::read_to_string(data.join(RECORD)).ok()?;
    match serde_json::from_str(&text) {
        Ok(record) => Some(record),
        Err(e) => {
            crate::log::warn("app", &format!("{RECORD} does not read ({e}); left as it is"));
            None
        }
    }
}

fn write_record(data: &Path, record: &Record) {
    let wrote = serde_json::to_vec_pretty(record)
        .map_err(|e| e.to_string())
        .and_then(|bytes| crate::secret::write_private(&data.join(RECORD), &bytes).map_err(|e| e.to_string()));
    if let Err(e) = wrote {
        // The setting stays as set; only the uninstaller's undo of it is lost.
        crate::log::warn("app", &format!("{RECORD} was not written ({e}): the uninstaller will leave the G-SYNC setting in place"));
    }
}

/// The setting at Force Off for the running exe (module docs). `write`
/// false only reads (the test build). Blocks: NVAPI loads and parses the
/// driver's database, which takes a moment.
#[cfg(windows)]
pub fn ensure(data: &Path, write: bool) -> Outcome {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => return Outcome::NotOff(format!("the exe's path did not read: {e}")),
    };
    let Some(api) = nvapi::Api::load() else { return Outcome::NoNvidia };
    let session = match api.session() {
        Ok(s) => s,
        Err(status) if nvapi::no_nvidia(status) => return Outcome::NoNvidia,
        Err(status) => return Outcome::NotOff(nvapi::why(status)),
    };
    let found = match session.find_exe(&exe) {
        Ok(found) => found,
        Err(status) => return Outcome::NotOff(nvapi::why(status)),
    };
    let step = plan(found.as_ref().map(|(_, f)| f));
    if step == Plan::Nothing {
        return Outcome::Off(found.map(|(_, f)| f.profile).unwrap_or_default());
    }
    if !write {
        return Outcome::ReadOnly;
    }
    let set = match (step, found) {
        (Plan::Set { profile }, Some((handle, f))) => {
            session.set(handle, FORCE_OFF).and_then(|()| session.save()).map(|()| (profile, f.own_value))
        }
        _ => session
            .own_profile(&exe)
            .and_then(|handle| session.set(handle, FORCE_OFF))
            .and_then(|()| session.save())
            .map(|()| (OWN_PROFILE.to_owned(), None)),
    };
    match set {
        Ok((profile, previous)) => {
            write_record(data, &record_after(read_record(data), &profile, previous));
            Outcome::Set(profile)
        }
        Err(status) => Outcome::NotOff(nvapi::why(status)),
    }
}

#[cfg(not(windows))]
pub fn ensure(_data: &Path, _write: bool) -> Outcome {
    Outcome::NoNvidia
}

/// `luna --nvidia-undo`, for the uninstaller: puts back what the record
/// says Luna changed, and removes the record once that is done.
pub fn undo_from_args() -> bool {
    if std::env::args().nth(1).as_deref() != Some("--nvidia-undo") {
        return false;
    }
    let data = crate::paths::data_dir();
    if let Some(record) = read_record(&data) {
        if undo(&record) {
            let _ = std::fs::remove_file(data.join(RECORD));
        }
    }
    true
}

/// Puts back what `record` says; true when done or nothing is left to do.
#[cfg(windows)]
fn undo(record: &Record) -> bool {
    let Some(api) = nvapi::Api::load() else { return false };
    let Ok(session) = api.session() else { return false };
    let profile = match session.find_profile(&record.profile) {
        Ok(found) => found,
        Err(_) => return false,
    };
    let own_value = match profile {
        Some(handle) => match session.own_value(handle) {
            Ok(v) => v,
            Err(_) => return false,
        },
        None => None,
    };
    let done = match (undo_plan(record, profile.is_some(), own_value), profile) {
        (Undo::Nothing, _) | (_, None) => return true,
        (Undo::DeleteProfile, Some(h)) => session.delete_profile(h),
        (Undo::Restore(value), Some(h)) => session.set(h, value),
        (Undo::DeleteSetting, Some(h)) => session.delete_setting(h),
    };
    done.and_then(|()| session.save()).is_ok()
}

#[cfg(not(windows))]
fn undo(_record: &Record) -> bool {
    true
}

/// NVAPI's types as its headers lay them out, and on Windows the calls.
mod nvapi {
    pub type Status = i32;
    pub const OK: Status = 0;
    pub const LIBRARY_NOT_FOUND: Status = -2;
    pub const NVIDIA_DEVICE_NOT_FOUND: Status = -6;
    pub const INVALID_USER_PRIVILEGE: Status = -137;
    pub const SETTING_NOT_FOUND: Status = -160;
    pub const PROFILE_NOT_FOUND: Status = -163;
    pub const EXECUTABLE_NOT_FOUND: Status = -166;

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

    /// An exe's full path as nvapi.h writes one (`c:/Folder1/Folder2/App.exe`).
    pub fn app_path(exe: &std::path::Path) -> String {
        exe.to_string_lossy().replace('\\', "/")
    }

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
            s => format!("NVAPI error {s}"),
        }
    }

    #[cfg(windows)]
    pub use calls::*;

    #[cfg(windows)]
    mod calls {
        use super::*;
        use std::ffi::c_void;
        use std::path::Path;
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

        impl Session<'_> {
            /// The profile the driver applies to `exe` (asked by its full
            /// path, as nvapi.h recommends), and the setting there; None
            /// when no profile has it.
            pub fn find_exe(&self, exe: &Path) -> Result<Option<(Handle, crate::desktop::gsync::Found)>, Status> {
                let name = unicode(&app_path(exe));
                let mut profile: Handle = std::ptr::null_mut();
                // SAFETY: a zeroed V4 application with its version, which
                // NVAPI fills in.
                let mut app = unsafe { zeroed::<Application>() };
                app.version = APPLICATION_VER;
                // SAFETY: the name is a full NvAPI_UnicodeString; NVAPI
                // writes the profile's handle and the application.
                let status = unsafe { (self.api.find_application_by_name)(self.handle, name.as_ptr(), &mut profile, &mut *app) };
                if status == EXECUTABLE_NOT_FOUND {
                    return Ok(None);
                }
                check(status)?;
                // SAFETY: a zeroed profile with its version, which NVAPI fills in.
                let mut info = unsafe { zeroed::<Profile>() };
                info.version = PROFILE_VER;
                // SAFETY: the profile handle NVAPI just gave.
                check(unsafe { (self.api.get_profile_info)(self.handle, profile, &mut *info) })?;
                let (value, own_value) = self.setting(profile)?;
                Ok(Some((profile, crate::desktop::gsync::Found { profile: text(&info.name), value, own_value })))
            }

            /// The setting as it applies in `profile`, and the profile's own
            /// user value of it.
            fn setting(&self, profile: Handle) -> Result<(Option<u32>, Option<u32>), Status> {
                // SAFETY: a zeroed setting with its version, which NVAPI fills in.
                let mut setting = unsafe { zeroed::<Setting>() };
                setting.version = SETTING_VER;
                // SAFETY: a profile handle of this session.
                let status =
                    unsafe { (self.api.get_setting)(self.handle, profile, crate::desktop::gsync::VRR_APP_OVERRIDE, &mut *setting) };
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

            /// The profile's own user value of the setting.
            pub fn own_value(&self, profile: Handle) -> Result<Option<u32>, Status> {
                self.setting(profile).map(|(_, own)| own)
            }

            pub fn find_profile(&self, name: &str) -> Result<Option<Handle>, Status> {
                let name = unicode(name);
                let mut profile: Handle = std::ptr::null_mut();
                // SAFETY: the name is a full NvAPI_UnicodeString.
                match unsafe { (self.api.find_profile_by_name)(self.handle, name.as_ptr(), &mut profile) } {
                    PROFILE_NOT_FOUND => Ok(None),
                    status => check(status).map(|()| Some(profile)),
                }
            }

            /// Luna's own profile, found or made, with `exe` in it.
            pub fn own_profile(&self, exe: &Path) -> Result<Handle, Status> {
                let profile = match self.find_profile(crate::desktop::gsync::OWN_PROFILE)? {
                    Some(profile) => profile,
                    None => {
                        // SAFETY: a zeroed profile with its version and name.
                        let mut info = unsafe { zeroed::<Profile>() };
                        info.version = PROFILE_VER;
                        info.name = *unicode(crate::desktop::gsync::OWN_PROFILE);
                        let mut profile: Handle = std::ptr::null_mut();
                        // SAFETY: NVAPI writes the new profile's handle.
                        check(unsafe { (self.api.create_profile)(self.handle, &mut *info, &mut profile) })?;
                        profile
                    }
                };
                // SAFETY: a zeroed V4 application with its version and names.
                let mut app = unsafe { zeroed::<Application>() };
                app.version = APPLICATION_VER;
                app.app_name = *unicode(&app_path(exe));
                app.friendly_name = *unicode("Luna");
                // SAFETY: a profile handle of this session.
                check(unsafe { (self.api.create_application)(self.handle, profile, &mut *app) })?;
                Ok(profile)
            }

            /// The setting at `value` in `profile`, as a DWORD of its own.
            pub fn set(&self, profile: Handle, value: u32) -> Result<(), Status> {
                // SAFETY: a zeroed setting with its version, id, type and value.
                let mut setting = unsafe { zeroed::<Setting>() };
                setting.version = SETTING_VER;
                setting.id = crate::desktop::gsync::VRR_APP_OVERRIDE;
                setting.current[0] = value;
                // SAFETY: a profile handle of this session.
                check(unsafe { (self.api.set_setting)(self.handle, profile, &mut *setting) })
            }

            pub fn delete_setting(&self, profile: Handle) -> Result<(), Status> {
                // SAFETY: a profile handle of this session.
                check(unsafe { (self.api.delete_profile_setting)(self.handle, profile, crate::desktop::gsync::VRR_APP_OVERRIDE) })
            }

            pub fn delete_profile(&self, profile: Handle) -> Result<(), Status> {
                // SAFETY: a profile handle of this session.
                check(unsafe { (self.api.delete_profile)(self.handle, profile) })
            }

            /// The session's changes, into the driver's database.
            pub fn save(&self) -> Result<(), Status> {
                // SAFETY: this session.
                check(unsafe { (self.api.save_settings)(self.handle) })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(profile: &str, value: Option<u32>, own_value: Option<u32>) -> Found {
        Found { profile: profile.into(), value, own_value }
    }

    #[test]
    fn the_setting_is_written_only_where_it_is_not_off() {
        assert_eq!(plan(Some(&found("Luna", Some(FORCE_OFF), Some(FORCE_OFF)))), Plan::Nothing);
        // Off by inheritance (the global profile's) is off too.
        assert_eq!(plan(Some(&found("Luna", Some(FORCE_OFF), None))), Plan::Nothing);
        for value in [None, Some(0), Some(2), Some(4)] {
            assert_eq!(plan(Some(&found("Luna", value, None))), Plan::Set { profile: "Luna".into() }, "{value:?}");
        }
        assert_eq!(plan(None), Plan::Own);
    }

    #[test]
    fn the_record_keeps_the_value_from_before_lunas_first_change() {
        let first = record_after(None, "Luna", Some(0));
        assert_eq!(first, Record { profile: "Luna".into(), own: false, previous: Some(0) });
        // Set again after someone changed it: what was there first stays.
        assert_eq!(record_after(Some(first.clone()), "Luna", Some(4)), first);
        // Another profile now: its own value.
        assert_eq!(record_after(Some(first), "Other", None), Record { profile: "Other".into(), own: false, previous: None });
        assert!(record_after(None, OWN_PROFILE, None).own);
    }

    #[test]
    fn the_undo_puts_back_only_what_luna_set() {
        let set = |previous| Record { profile: "Luna".into(), own: false, previous };
        assert_eq!(undo_plan(&set(None), true, Some(FORCE_OFF)), Undo::DeleteSetting);
        assert_eq!(undo_plan(&set(Some(0)), true, Some(FORCE_OFF)), Undo::Restore(0));
        // Changed by someone since, or no longer there: left alone.
        assert_eq!(undo_plan(&set(None), true, Some(2)), Undo::Nothing);
        assert_eq!(undo_plan(&set(None), true, None), Undo::Nothing);
        assert_eq!(undo_plan(&set(None), false, None), Undo::Nothing);
        let own = Record { profile: OWN_PROFILE.into(), own: true, previous: None };
        assert_eq!(undo_plan(&own, true, Some(2)), Undo::DeleteProfile);
        assert_eq!(undo_plan(&own, false, None), Undo::Nothing);
        // A record that calls another profile Luna's own removes nothing.
        let forged = Record { profile: "Base Profile".into(), own: true, previous: None };
        assert_eq!(undo_plan(&forged, true, Some(2)), Undo::Nothing);
    }

    #[test]
    fn the_record_reads_as_written() {
        let record = Record { profile: "Luna".into(), own: false, previous: Some(4) };
        let text = serde_json::to_string(&record).unwrap();
        assert_eq!(text, r#"{"profile":"Luna","own":false,"previous":4}"#);
        assert_eq!(serde_json::from_str::<Record>(&text).unwrap(), record);
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

    #[test]
    fn an_exe_is_named_by_its_full_path_with_forward_slashes() {
        assert_eq!(nvapi::app_path(Path::new(r"C:\Users\a\Luna\luna.exe")), "C:/Users/a/Luna/luna.exe");
    }

    /// Read only: which profile this machine's NVIDIA driver applies to the
    /// exe `LUNA_GSYNC_PROBE` names, and the setting there. Run by hand on a
    /// Windows PC with an NVIDIA driver (`--ignored`); it changes nothing.
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn probe_the_drivers_profile_for_an_exe() {
        let exe = std::env::var("LUNA_GSYNC_PROBE").expect("LUNA_GSYNC_PROBE names an exe");
        let api = nvapi::Api::load().expect("nvapi64.dll in System32");
        let session = api.session().expect("a session on the driver's profiles");
        let found = session.find_exe(Path::new(&exe)).expect("the lookup answers");
        println!("{exe}: {:?} -> {:?}", found.as_ref().map(|(_, f)| f), plan(found.as_ref().map(|(_, f)| f)));
        if let Ok(Some(own)) = session.find_profile(OWN_PROFILE) {
            println!("{OWN_PROFILE} exists: own value {:?}", session.own_value(own));
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
