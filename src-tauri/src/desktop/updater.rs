//! Luna's own updater, in place of tauri-plugin-updater and compatible with
//! it both ways: a 0.19 window updates onto a release this serves, and this
//! updates onto any later one, from the same endpoint, `latest.json` and
//! signing key (tauri.conf.json `plugins.updater`).
//!
//! - `check` fetches `latest.json` (scripts/release-lib.ps1
//!   `New-UpdaterManifest`) and offers its release when its version is
//!   newer than this build's, by semver, as the plugin compares them. The
//!   platform is looked up as `<os>-<arch>-nsis`, then `<os>-<arch>`
//!   (Luna publishes `windows-x86_64`). The test build never offers one, as
//!   its page never asked (updates.store.ts). Only an https URL is taken.
//! - `download` fetches the installer and checks it before anything is
//!   written: minisign, on the plugin's own `minisign-verify`, against the
//!   public key, the version in the signature's trusted comment held to the
//!   release's, and the file an NSIS setup (`is_nsis_setup`). The plugin
//!   lets a signature without a version pass unless told otherwise; every
//!   Luna signature has carried one (scripts/updater-signature.mjs), so one
//!   without it is refused here. It is staged in the data dir's `updates/`,
//!   a private folder (`secret::private_dir`) under a data dir that is no
//!   link and a trusted owner's (`check_root`), under a name the app gives
//!   it (`file_name`): `latest.json` is not signed, so its URL chooses
//!   nothing about the file that is started.
//! - `install` opens the staged file as itself (never through a link at its
//!   name), shared for reading alone, checks what it reads through that
//!   handle once more, and starts it by the path Windows gives that handle
//!   (`final_path`) while the handle is still open, so what runs is what was
//!   checked (`hold`). It starts it the way the plugin starts an NSIS
//!   installer on Windows: "open" with `/P /UPDATE /R /ARGS <this run's
//!   arguments>` (passive, update mode, start Luna again afterwards, with
//!   those arguments, when they fit the setup's command line,
//!   `parameters_for`). The caller then exits at once, as the plugin's
//!   `exit(0)` did: the installer stops a Luna that is still running before
//!   it replaces the exe. Off Windows an update is installed by hand.
//!
//! Neither `download` nor `install` takes a release that is not newer than
//! this build, whatever the caller passes them.
//!
//! Requests go through `crate::http`, which blocks its thread: call these off
//! the window's loop.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The endpoint and the key every installed Luna polls and checks with.
pub const ENDPOINT: &str = "https://github.com/nrubinmosin/luna/releases/latest/download/latest.json";
pub const PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDJCNDNDMUIxMkFEMjg2MjIKUldRaWh0SXFzY0ZESzN5VytwM1d2VFlzdVM4Qy9IRGJQbC9SeURzYTIzMkVjVVM5SHlBbnF6UGkK";

/// The most an installer download may be.
const DOWNLOAD_MAX: usize = 512 << 20;

/// Where updates come from and what they are checked against.
#[derive(Clone, Debug)]
pub struct Config {
    pub endpoint: String,
    /// Base64 of the minisign public key file, as tauri.conf.json holds it.
    pub pubkey: String,
    pub current: semver::Version,
}

impl Config {
    /// Luna's own: the endpoint, the key and this build's version.
    pub fn luna() -> Config {
        Config {
            endpoint: ENDPOINT.into(),
            pubkey: PUBKEY.into(),
            current: semver::Version::parse(env!("CARGO_PKG_VERSION")).expect("the crate's version is semver"),
        }
    }
}

/// A release newer than this build, for this platform.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: semver::Version,
    /// The update chip's text (the CHANGELOG section's intro).
    pub notes: Option<String>,
    pub pub_date: Option<String>,
    pub url: String,
    /// Base64 of the minisign signature file.
    pub signature: String,
    /// The core fingerprint the release was built with: the same as the
    /// running core's, and the update restarts only the window.
    pub core_fingerprint: Option<String>,
}

/// An installer staged on disk whose signature checked out. Made by
/// `download` alone, and taken by `install` as it was made.
#[derive(Clone, Debug)]
pub struct Downloaded {
    /// The data dir it was staged under, checked again by `install`.
    root: PathBuf,
    path: PathBuf,
    version: semver::Version,
    signature: String,
}

impl Downloaded {
    /// Where it is staged.
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn version(&self) -> &semver::Version {
        &self.version
    }
}

/// The platform keys to look a release up by, in order.
pub fn targets() -> Vec<String> {
    let os = if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "linux"
    };
    let arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        std::env::consts::ARCH
    };
    let mut keys = Vec::new();
    if cfg!(windows) {
        keys.push(format!("{os}-{arch}-nsis"));
    }
    keys.push(format!("{os}-{arch}"));
    keys
}

/// A version as a manifest or a trusted comment writes it, a `v` allowed.
fn version(text: &str) -> Result<semver::Version, String> {
    semver::Version::parse(text.trim().trim_start_matches('v')).map_err(|e| format!("not a version, {text:?}: {e}"))
}

/// What `latest.json` offers this build: `None` when its release is not
/// newer, an error when it does not read or has nothing for this platform.
pub fn parse(manifest: &[u8], current: &semver::Version, targets: &[String]) -> Result<Option<Release>, String> {
    let json: serde_json::Value = serde_json::from_slice(manifest).map_err(|e| format!("latest.json does not read: {e}"))?;
    let text = |v: &serde_json::Value| v.as_str().map(str::to_string);
    let v = json.get("version").or_else(|| json.get("name")).and_then(serde_json::Value::as_str).ok_or("latest.json has no version")?;
    let v = version(v)?;
    let pub_date = json.get("pub_date").and_then(text);
    if let Some(date) = &pub_date {
        date.parse::<jiff::Timestamp>().map_err(|e| format!("latest.json's pub_date {date:?}: {e}"))?;
    }
    // Not newer is no update, whatever it has or lacks for this platform.
    if v <= *current {
        return Ok(None);
    }
    let entry = match json.get("platforms") {
        Some(platforms) => {
            targets.iter().find_map(|t| platforms.get(t)).ok_or_else(|| format!("latest.json has nothing for {}", targets.join(" or ")))?
        }
        // The plugin's other shape: one platform's url and signature at the top.
        None => &json,
    };
    let url = entry.get("url").and_then(text).ok_or("latest.json names no url")?;
    let signature = entry.get("signature").and_then(text).ok_or("latest.json carries no signature")?;
    if !fetchable(&url) {
        return Err(format!("latest.json's url is not https: {url}"));
    }
    Ok(Some(Release {
        version: v,
        notes: json.get("notes").and_then(text),
        pub_date,
        url,
        signature,
        core_fingerprint: json.get("core_fingerprint").and_then(text),
    }))
}

/// Checks `signature` (base64 of a minisign signature file) over `data`
/// against `pubkey` (base64 of a minisign public key file), and that the
/// trusted comment names `version`.
pub fn verify(data: &[u8], signature: &str, pubkey: &str, version: &semver::Version) -> Result<(), String> {
    use base64::Engine;
    let decode = |what: &str, b64: &str| {
        let bytes = base64::engine::general_purpose::STANDARD.decode(b64.trim()).map_err(|e| format!("{what} is not base64: {e}"))?;
        String::from_utf8(bytes).map_err(|_| format!("{what} is not text"))
    };
    let key = minisign_verify::PublicKey::decode(&decode("the public key", pubkey)?).map_err(|e| format!("the public key: {e}"))?;
    let sig = minisign_verify::Signature::decode(&decode("the signature", signature)?).map_err(|e| format!("the signature: {e}"))?;
    // Legacy signatures (over the file, not its hash) allowed, as the plugin
    // allows them.
    key.verify(data, &sig, true).map_err(|e| format!("the signature does not check out: {e}"))?;
    // Only now is the trusted comment the signer's: the global signature,
    // checked above, covers it.
    let signed = sig.trusted_comment().split('\t').find_map(|f| f.strip_prefix("version:")).ok_or("the signature names no version")?;
    if self::version(signed)? != *version {
        return Err(format!("the installer was signed for {signed}, not {version}"));
    }
    Ok(())
}

/// The release on offer, if any. The test build is never offered one.
pub fn check(config: &Config) -> Result<Option<Release>, String> {
    if crate::paths::is_test() && !cfg!(test) {
        return Ok(None);
    }
    let resp = agent()?.get(&config.endpoint).header("Accept", "application/json").call().map_err(|e| e.to_string())?;
    if resp.status() == 204 {
        return Ok(None);
    }
    parse(&resp.bytes(1 << 20)?, &config.current, &targets())
}

fn agent() -> Result<crate::http::Agent, String> {
    use std::time::Duration;
    crate::http::Agent::new(crate::http::Timeouts { connect: Duration::from_secs(30), read: Duration::from_secs(60), total: None })
}

/// The folder for downloaded installers in the data dir `root`
/// (`crate::paths::data_dir()` for the window).
pub fn downloads_dir(root: &Path) -> PathBuf {
    root.join("updates")
}

/// Refuses a data dir `root` an installer is not to be staged under: a link
/// or junction, or any other reparse point (a OneDrive folder's among them),
/// which would put `updates/` somewhere else than where it is read again;
/// not a folder; one whose owner is not this user, Administrators, SYSTEM or
/// TrustedInstaller (root on Unix), who could swap what is in it; or, on
/// Windows, one not on a local fixed drive (`fixed_drive`). `updates/`
/// itself is `secret::private_dir`'s to check.
pub fn check_root(root: &Path) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(root).map_err(|e| format!("{}: {e}", root.display()))?;
    #[cfg(windows)]
    let link = {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
    };
    #[cfg(not(windows))]
    let link = meta.file_type().is_symlink();
    if link || !meta.is_dir() {
        return Err(format!(
            "{} is not a plain folder (a link, a junction, another reparse point such as a OneDrive folder's, or a file is there): \
             no update is staged under it",
            root.display()
        ));
    }
    #[cfg(windows)]
    {
        let resolved = open_dir(root).and_then(|d| final_path(&d)).map_err(|e| format!("{}: {e}", root.display()))?;
        fixed_drive(&resolved).map_err(|why| format!("{} is {why}: no update is staged under it or started from it", root.display()))?;
    }
    #[cfg(windows)]
    let trusted = crate::secret::acl::owner_trusted(root).map_err(|e| format!("{}: {e}", root.display()))?;
    #[cfg(unix)]
    let trusted = {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no preconditions and cannot fail.
        meta.uid() == unsafe { libc::geteuid() } || meta.uid() == 0
    };
    #[cfg(not(any(windows, unix)))]
    let trusted = true;
    if !trusted {
        return Err(format!("{} belongs to someone else: no update is staged under it", root.display()));
    }
    Ok(())
}

/// Drive types `GetDriveTypeW` gives (winbase.h): the only one an update is
/// staged on and started from, and the two refused by name.
#[cfg(windows)]
const DRIVE_REMOVABLE: u32 = 2;
#[cfg(windows)]
const DRIVE_FIXED: u32 = 3;
#[cfg(windows)]
const DRIVE_REMOTE: u32 = 4;

/// Refuses a path, in the plain form `final_path` gives, that is not on a
/// local fixed drive: a share (`\\server\share\…`, refused before anything
/// asks the network), a mapped network drive, a removable one, anything
/// else `GetDriveTypeW` does not call fixed. What is on a share is the
/// share's to change under Luna, whatever its folder's owner says.
#[cfg(windows)]
pub fn fixed_drive(path: &str) -> Result<(), &'static str> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetVolumePathNameW};
    if path.starts_with(r"\\") {
        return Err("on a network share");
    }
    let mut volume = vec![0u16; path.encode_utf16().count() + 2];
    // SAFETY: a path that outlives the call, and a buffer longer than the
    // path and its NUL, which the path of its volume never passes.
    unsafe { GetVolumePathNameW(&HSTRING::from(path), &mut volume) }.map_err(|_| "on no volume Windows can name")?;
    // SAFETY: the volume's path just written, NUL-terminated.
    match unsafe { GetDriveTypeW(PCWSTR(volume.as_ptr())) } {
        DRIVE_FIXED => Ok(()),
        DRIVE_REMOTE => Err("on a network drive"),
        DRIVE_REMOVABLE => Err("on a removable drive"),
        _ => Err("not on a local fixed drive"),
    }
}

/// A folder opened for its final path (`final_path`), a link at its name
/// followed: Windows opens a folder only with `FILE_FLAG_BACKUP_SEMANTICS`.
#[cfg(windows)]
fn open_dir(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS;
    std::fs::OpenOptions::new().read(true).custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0).open(path)
}

/// A folder held where it is while an installer is written into it, and the
/// check that the file written is the one at its name in it. On Windows the
/// folder is open as itself (a link at its name not followed), sharing reads
/// and writes but not deletion, so that neither it nor a folder above it can
/// be renamed or deleted while it is held; the file is taken only if the path
/// its own handle resolves to is the folder's, resolved the same way, and its
/// name. On Unix the folder is open without following a link, and the file
/// reached by its name from that descriptor must be the one at the path (the
/// same device and inode).
struct Pin {
    dir: std::fs::File,
}

impl Pin {
    fn open(path: &Path) -> Result<Pin, String> {
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows::Win32::Storage::FileSystem::{
                FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
            };
            options
                .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW);
        }
        let dir = options.open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Pin { dir })
    }

    /// Whether the file at `path` is the one named `name` in the pinned
    /// folder, and a private file of Luna's.
    fn holds(&self, path: &Path, name: &str) -> Result<(), String> {
        let refuse = |why: String| Err(format!("{} is not the installer just staged: {why}", path.display()));
        let file = open_held(path).map_err(|e| format!("{}: {e}", path.display()))?;
        match crate::secret::private_file(&file) {
            Ok(Ok(())) => {}
            Ok(Err(why)) => return refuse(why),
            Err(e) => return refuse(e.to_string()),
        }
        #[cfg(windows)]
        {
            match (final_path(&self.dir), final_path(&file)) {
                (Ok(dir), Ok(at)) if at == format!(r"{dir}\{name}") => Ok(()),
                (Ok(dir), Ok(at)) => refuse(format!("it resolves to {at}, not into {dir}")),
                (Err(e), _) | (_, Err(e)) => refuse(e.to_string()),
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            use std::os::unix::fs::MetadataExt;
            use std::os::unix::io::{AsRawFd, FromRawFd};
            let c_name = match std::ffi::CString::new(std::ffi::OsStr::new(name).as_bytes()) {
                Ok(n) => n,
                Err(_) => return refuse("a name with a NUL".into()),
            };
            let flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC;
            // SAFETY: the pinned folder's descriptor and a NUL-terminated
            // name; the descriptor returned is owned by the File made of it.
            let fd = unsafe { libc::openat(self.dir.as_raw_fd(), c_name.as_ptr(), flags) };
            if fd < 0 {
                return refuse(std::io::Error::last_os_error().to_string());
            }
            // SAFETY: a descriptor just opened, owned by nothing else.
            let pinned = unsafe { std::fs::File::from_raw_fd(fd) };
            match (pinned.metadata(), file.metadata()) {
                (Ok(a), Ok(b)) if a.dev() == b.dev() && a.ino() == b.ino() => Ok(()),
                (Ok(_), Ok(_)) => refuse("another file is at its name".into()),
                (Err(e), _) | (_, Err(e)) => refuse(e.to_string()),
            }
        }
        #[cfg(not(any(windows, unix)))]
        {
            let _ = name;
            Ok(())
        }
    }
}

/// Whether an installer is fetched from `url`: https alone. The signature
/// holds the bytes whatever the transport; https keeps what is fetched from
/// whom between Luna and the host. A test's server on loopback is plain
/// http.
fn fetchable(url: &str) -> bool {
    url.starts_with("https://") || (cfg!(test) && url.starts_with("http://127.0.0.1:"))
}

/// The name an installer is staged under: the app's own, never the URL's
/// name or extension, as `latest.json` is not signed. The name Tauri's
/// bundler and `build-windows.ps1` give the setup.
pub fn file_name(version: &semver::Version) -> String {
    format!("Luna_{version}_x64-setup.exe")
}

/// Whether `bytes` are an NSIS setup, as makensis writes one (Luna's own and
/// Tauri's alike): its stub, a 32-bit x86 GUI executable whatever it
/// installs (`MZ`; at the offset the DOS header gives, `PE\0\0`, machine
/// 0x014C, an executable image and no DLL; a PE32 optional header,
/// subsystem 2), followed right where its sections' data ends by NSIS's
/// first header (`0xDEADBEEF`, `NullsoftInst`) and as many bytes as that
/// header says follow. The x64 in a setup's name is the app's: what it
/// installs is held to AMD64 when it is built (build-windows.ps1). Checked
/// on every platform: off Windows a setup is only ever staged, never run.
pub fn is_nsis_setup(bytes: &[u8]) -> bool {
    let Some(end) = stub_end(bytes) else { return false };
    // firstheader: flags, 0xDEADBEEF, "NullsoftInst", the header's length,
    // then the length of everything from here on, itself included.
    let Some(first) = end.checked_add(28).and_then(|to| bytes.get(end..to)) else { return false };
    let following = u32::from_le_bytes(first[24..28].try_into().unwrap()) as usize;
    first[4..8] == 0xDEAD_BEEFu32.to_le_bytes()
        && &first[8..20] == b"NullsoftInst"
        && following >= 28
        && end.checked_add(following).is_some_and(|to| to <= bytes.len())
}

/// Where the data of an NSIS setup's stub ends, there being one (see
/// `is_nsis_setup`): where its sections' data ends, which is where makensis
/// puts its own.
fn stub_end(bytes: &[u8]) -> Option<usize> {
    let u16_at = |at: usize| bytes.get(at..at.checked_add(2)?).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u32_at = |at: usize| bytes.get(at..at.checked_add(4)?).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    const I386: u16 = 0x014C;
    const EXECUTABLE_IMAGE: u16 = 0x0002;
    const DLL: u16 = 0x2000;
    const PE32: u16 = 0x010B;
    const GUI: u16 = 2;
    if !bytes.starts_with(b"MZ") {
        return None;
    }
    let pe = u32_at(0x3C)? as usize;
    if bytes.get(pe..pe.checked_add(4)?)? != b"PE\0\0" {
        return None;
    }
    // The COFF header, then the optional header.
    let coff = pe + 4;
    let sections = u16_at(coff + 2)? as usize;
    let optional_size = u16_at(coff + 16)? as usize;
    let characteristics = u16_at(coff + 18)?;
    let optional = coff + 20;
    if u16_at(coff)? != I386
        || characteristics & EXECUTABLE_IMAGE == 0
        || characteristics & DLL != 0
        || u16_at(optional)? != PE32
        || optional_size < 70
        || u16_at(optional + 68)? != GUI
        || sections == 0
    {
        return None;
    }
    let table = optional + optional_size;
    let mut end = 0usize;
    for i in 0..sections {
        let at = table + i * 40;
        let size = u32_at(at + 16)? as usize;
        let start = u32_at(at + 20)? as usize;
        if size != 0 {
            end = end.max(start.checked_add(size)?);
        }
    }
    Some(end)
}

/// Refuses a release no newer than this build: `parse` offers none, and a
/// caller handing `download` or `install` one of its own gets no downgrade.
fn newer(config: &Config, version: &semver::Version) -> Result<(), String> {
    if *version <= config.current {
        return Err(format!("{version} is not newer than this build's {}", config.current));
    }
    Ok(())
}

/// What an installer's bytes must be: signed for `version`, and Luna's setup.
fn check_setup(config: &Config, bytes: &[u8], version: &semver::Version, signature: &str) -> Result<(), String> {
    verify(bytes, signature, &config.pubkey, version)?;
    if !is_nsis_setup(bytes) {
        return Err("the installer is not an NSIS setup".into());
    }
    Ok(())
}

/// Downloads `release`'s installer and stages it in the data dir `root`'s
/// `updates/` (`downloads_dir`), `progress` told the bytes so far and the
/// total when the server says it. Nothing is written unless it checks out
/// (`check_setup`). `root` must be a plain folder of a trusted owner
/// (`check_root`); `updates/` is made private, or refused where it is a
/// link or junction or someone else's (`secret::private_dir`); the file is
/// private from the moment it exists (`secret::write_private`).
pub fn download(config: &Config, release: &Release, root: &Path, mut progress: impl FnMut(u64, Option<u64>)) -> Result<Downloaded, String> {
    newer(config, &release.version)?;
    if !fetchable(&release.url) {
        return Err(format!("the installer's url is not https: {}", release.url));
    }
    check_root(root)?;
    let resp = agent()?.get(&release.url).header("Accept", "application/octet-stream").call().map_err(|e| e.to_string())?;
    let total = resp.header("content-length").and_then(|v| v.parse::<u64>().ok());
    let mut bytes = Vec::new();
    resp.each_piece(|piece| {
        if bytes.len() + piece.len() > DOWNLOAD_MAX {
            return Err(format!("the installer is larger than {DOWNLOAD_MAX} bytes"));
        }
        bytes.extend_from_slice(piece);
        progress(bytes.len() as u64, total);
        Ok(())
    })?;
    check_setup(config, &bytes, &release.version, &release.signature)?;
    // Again, the data dir held where it is first: the download took a
    // while. From here until the written file is checked, neither the data
    // dir nor `updates/` can be renamed or deleted (`Pin`).
    let root_pin = Pin::open(root)?;
    check_root(root)?;
    let dir = downloads_dir(root);
    crate::secret::private_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let dir_pin = Pin::open(&dir)?;
    let name = file_name(&release.version);
    let path = dir.join(&name);
    crate::secret::write_private(&path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    dir_pin.holds(&path, &name)?;
    drop((dir_pin, root_pin));
    Ok(Downloaded { root: root.to_path_buf(), path, version: release.version.clone(), signature: release.signature.clone() })
}

/// Clears what earlier downloads left in the data dir `root`'s `updates/`:
/// an installer runs once.
pub fn clear_downloads(root: &Path) {
    if let Ok(entries) = std::fs::read_dir(downloads_dir(root)) {
        for e in entries.flatten() {
            if e.file_type().is_ok_and(|t| t.is_file()) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// An argument as the NSIS installer's command line takes it, the way the
/// plugin writes it: Windows' own quoting, and a `/` quoted too, which NSIS
/// would otherwise read as the start of an option of its own.
pub fn nsis_arg(arg: &str) -> String {
    let quote = arg.is_empty() || arg.contains([' ', '\t', '/']);
    let mut out = String::new();
    if quote {
        out.push('"');
    }
    let mut backslashes = 0;
    for c in arg.chars() {
        if c == '\\' {
            backslashes += 1;
        } else {
            if c == '"' {
                // 2n+1 backslashes before an inner quote.
                out.extend(std::iter::repeat_n('\\', backslashes + 1));
            }
            backslashes = 0;
        }
        out.push(c);
    }
    if quote {
        // 2n before the closing one.
        out.extend(std::iter::repeat_n('\\', backslashes));
        out.push('"');
    }
    out
}

/// Whether `arg`, written by `nsis_arg`, reaches the restarted Luna as it
/// is. The setup's command line is read twice: by NSIS's own start-up,
/// which knows quotes but not Windows' backslash escapes, so that a `"`
/// inside an argument puts the two readings out of step, and which takes an
/// argument opening with `/` for an option of its own (`/S` silent,
/// `/NCRC`), quoted or not; then by Luna, as Windows quotes. Control
/// characters are left out with them.
pub fn survives(arg: &str) -> bool {
    !arg.contains('"') && !arg.starts_with('/') && !arg.chars().any(char::is_control)
}

/// This run's arguments (its program name left out) as the restarted Luna
/// is to get them: those `survives` passes. One that would not, or is not
/// Unicode, is left out and said in the log, rather than reach Luna changed
/// or change the setup's own options. Luna's own arguments are plain flags,
/// numbers and paths, which all pass.
pub fn restart_args(args: impl IntoIterator<Item = OsString>) -> Vec<String> {
    let mut kept = Vec::new();
    for arg in args {
        match arg.into_string() {
            Ok(arg) if survives(&arg) => kept.push(arg),
            _ => crate::log::warn("updater", "an argument of this run is not passed on to Luna's restart after the update"),
        }
    }
    kept
}

/// The installer's command line for an update, `args` from `restart_args`:
/// passive, update mode, start Luna again with them once installed. The
/// installer hands Luna everything after `/ARGS` as it stands
/// (installer.nsi `SplitCmdLine`).
pub fn installer_parameters(args: &[String]) -> String {
    let mut parts = vec!["/P".to_string(), "/UPDATE".to_string(), "/R".to_string(), "/ARGS".to_string()];
    parts.extend(args.iter().map(|a| nsis_arg(a)));
    parts.join(" ")
}

/// The size of the string NSIS copies the setup's command line into,
/// `NSIS_MAX_STRLEN` units, its closing NUL among them: 1024 in the makensis
/// that builds the setup (installer.nsi refuses a smaller one). Windows takes
/// 32,767. NSIS cuts the rest, and the setup takes a line that fills its
/// string (1023 units) for one that was cut (installer.nsi `ArgsCut`): a
/// line it reads whole is 1022 units at most.
pub const SETUP_CMDLINE_MAX: usize = 1024;

/// The installer's parameters for the setup at `file` (`installer_parameters`),
/// with `args` when the command line Windows makes of the two, `"file"
/// parameters`, with its NUL and the unit left free, fits
/// `SETUP_CMDLINE_MAX`; else with none of them, said in
/// the log, rather than cut where NSIS would cut it. An error when even
/// the bare update's does not fit.
pub fn parameters_for(file: &str, args: &[String]) -> Result<String, String> {
    // The two quotes around the file, the space after it, the NUL, and the
    // unit a line the setup reads whole leaves free.
    let fits = |parameters: &str| file.encode_utf16().count() + 5 + parameters.encode_utf16().count() <= SETUP_CMDLINE_MAX;
    let with = installer_parameters(args);
    if fits(&with) {
        return Ok(with);
    }
    let bare = installer_parameters(&[]);
    if !fits(&bare) {
        return Err(format!("the installer's path is too long for its own command line: {file}"));
    }
    crate::log::warn(
        "updater",
        "this run's arguments are too long for the installer's command line: Luna restarts without them after the update",
    );
    Ok(bare)
}

/// A staged installer, open from its check until it has started: shared for
/// reading alone on Windows, so that nobody may write, rename or delete it
/// meanwhile, and opened as itself, never through a link or junction at its
/// name. What was checked is what was read through this handle.
struct Held {
    // Keeps the file as it is for as long as it is open.
    #[cfg_attr(not(windows), allow(dead_code))]
    file: std::fs::File,
    path: PathBuf,
}

/// `path` opened for `Held`: for reading, sharing reads alone, and at a link
/// the link itself (`FILE_FLAG_OPEN_REPARSE_POINT`), which
/// `secret::private_file` then refuses. On Unix `O_NOFOLLOW` refuses one,
/// and `O_NONBLOCK` keeps a FIFO from holding the open.
fn open_held(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows::Win32::Storage::FileSystem::{FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ};
        options.share_mode(FILE_SHARE_READ.0).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    options.open(path)
}

/// Opens the staged installer (`Held`), the data dir it is staged under
/// checked again first (`check_root`), holds it to a private file of
/// Luna's (a plain file of one name, a trusted owner's, nobody else may
/// open) and checks what it reads through that handle as `download` checked
/// the download.
fn hold(config: &Config, downloaded: &Downloaded) -> Result<Held, String> {
    use std::io::Read;
    let path = &downloaded.path;
    newer(config, &downloaded.version)?;
    check_root(&downloaded.root)?;
    let file = open_held(path).map_err(|e| format!("{}: {e}", path.display()))?;
    match crate::secret::private_file(&file) {
        Ok(Ok(())) => {}
        Ok(Err(why)) => return Err(format!("{} is not the installer Luna staged ({why})", path.display())),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    }
    let mut bytes = Vec::new();
    (&file).take(DOWNLOAD_MAX as u64 + 1).read_to_end(&mut bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() > DOWNLOAD_MAX {
        return Err(format!("the installer is larger than {DOWNLOAD_MAX} bytes"));
    }
    check_setup(config, &bytes, &downloaded.version, &downloaded.signature)?;
    Ok(Held { file, path: path.clone() })
}

/// Starts the staged installer, checked again first through the handle it
/// stays open on until it has started (`hold`), by the path Windows gives
/// that handle (`final_path`). On success the caller exits now, so that the
/// installer finds no Luna running.
pub fn install(config: &Config, downloaded: &Downloaded) -> Result<(), String> {
    let held = hold(config, downloaded)?;
    #[cfg(windows)]
    {
        let file = final_path(&held.file).map_err(|e| format!("{}: {e}", held.path.display()))?;
        fixed_drive(&file).map_err(|why| format!("the installer at {file} is {why}: it is not started"))?;
        let parameters = parameters_for(&file, &restart_args(std::env::args_os().skip(1)))?;
        launch(&file, &parameters)
    }
    #[cfg(not(windows))]
    {
        Err(format!("on this platform an update is installed by hand: {}", held.path.display()))
    }
}

/// The path of the file open on `file`, as Windows resolves its handle
/// (`GetFinalPathNameByHandleW`): where the file is, every link and
/// junction on the way to it as it led when the file was opened, so that
/// the setup is started by a path through none of them. In its plain form
/// (`dos_path`).
#[cfg(windows)]
fn final_path(file: &std::fs::File) -> std::io::Result<String> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        GetFinalPathNameByHandleW, FILE_NAME_NORMALIZED, GETFINALPATHNAMEBYHANDLE_FLAGS, VOLUME_NAME_DOS,
    };
    let handle = HANDLE(file.as_raw_handle());
    let mut buf = vec![0u16; 1024];
    loop {
        // SAFETY: a handle open for as long as `file`; the buffer is the
        // length passed with it.
        let n = unsafe {
            GetFinalPathNameByHandleW(handle, &mut buf, GETFINALPATHNAMEBYHANDLE_FLAGS(FILE_NAME_NORMALIZED.0 | VOLUME_NAME_DOS.0))
        } as usize;
        if n == 0 {
            return Err(std::io::Error::last_os_error());
        }
        // Too small: `n` is the length it needs, its NUL counted.
        if n >= buf.len() {
            buf.resize(n, 0);
            continue;
        }
        buf.truncate(n);
        break;
    }
    let path = String::from_utf16(&buf).map_err(|_| std::io::Error::other("the installer's path is not Unicode"))?;
    dos_path(&path).ok_or_else(|| std::io::Error::other(format!("the installer's path has no plain form to start it by: {path}")))
}

/// `\\?\C:\…` and `\\?\UNC\server\share\…` as `C:\…` and `\\server\share\…`,
/// the form the shell starts a file by. None where that form would name
/// something else: read that way, Windows drops a name's trailing dot or
/// space and takes `.` and `..` as steps, which the `\\?\` form keeps. Any
/// other form is not what `GetFinalPathNameByHandleW` gives for a DOS
/// volume name, and is None too.
pub fn dos_path(path: &str) -> Option<String> {
    let plain = if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else {
        let rest = path.strip_prefix(r"\\?\")?;
        let drive = rest.as_bytes();
        if drive.len() < 3 || !drive[0].is_ascii_alphabetic() || drive[1] != b':' || drive[2] != b'\\' {
            return None;
        }
        rest.to_string()
    };
    let odd = plain.split('\\').skip(1).any(|name| name.ends_with('.') || name.ends_with(' '));
    (!odd).then_some(plain)
}

/// Starts the installer at `file` with `parameters`, through
/// `ShellExecuteExW` "open" as the plugin did, which also brings up a UAC
/// prompt should a setup ask for one. `SEE_MASK_NOCLOSEPROCESS` and
/// `SEE_MASK_NOASYNC` make it return once the process is there, so the
/// handle `Held` keeps covers the whole of the start; from then on the
/// image is mapped, and Windows lets nobody write the file.
#[cfg(windows)]
fn launch(file: &str, parameters: &str) -> Result<(), String> {
    use windows::core::{w, HSTRING, PCWSTR};
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOW;
    let file = HSTRING::from(file);
    let parameters = HSTRING::from(parameters);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        lpVerb: w!("open"),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(parameters.as_ptr()),
        nShow: SW_SHOW.0,
        ..Default::default()
    };
    // SAFETY: the struct and the strings it points at outlive the call; the
    // process handle it hands back is closed once, here.
    unsafe {
        ShellExecuteExW(&mut info).map_err(|e| format!("the installer did not start: {e}"))?;
        if !info.hProcess.is_invalid() {
            let _ = CloseHandle(info.hProcess);
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use base64::Engine;
    use ring::signature::{Ed25519KeyPair, KeyPair};
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn v(text: &str) -> semver::Version {
        semver::Version::parse(text).unwrap()
    }

    fn b64(bytes: impl AsRef<[u8]>) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    /// A throwaway minisign key, made here: base64 of its public key file,
    /// as tauri.conf.json holds one, and a signer of legacy signatures
    /// (over the file itself), which needs no BLAKE2b. The prehashed form is
    /// the checked-in Tauri fixture's.
    pub(crate) struct Key {
        pair: Ed25519KeyPair,
        id: [u8; 8],
    }

    impl Key {
        pub(crate) fn new(id: [u8; 8]) -> Key {
            let rng = ring::rand::SystemRandom::new();
            let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
            Key { pair: Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap(), id }
        }

        pub(crate) fn public(&self) -> String {
            let mut bin = b"Ed".to_vec();
            bin.extend(self.id);
            bin.extend(self.pair.public_key().as_ref());
            b64(format!("untrusted comment: minisign public key: test\n{}\n", b64(bin)))
        }

        fn sign_with_comment(&self, data: &[u8], trusted: &str) -> String {
            let sig = self.pair.sign(data);
            let mut bin = b"Ed".to_vec();
            bin.extend(self.id);
            bin.extend(sig.as_ref());
            let mut global = sig.as_ref().to_vec();
            global.extend(trusted.as_bytes());
            let global = self.pair.sign(&global);
            b64(format!("untrusted comment: signature from tauri secret key\n{}\ntrusted comment: {trusted}\n{}\n", b64(bin), b64(global)))
        }

        pub(crate) fn sign(&self, data: &[u8], version: &str) -> String {
            self.sign_with_comment(data, &format!("timestamp:1763500000\tfile:setup.exe\tversion:{version}"))
        }
    }

    /// A setup as far as the checks go: an NSIS stub for `machine` (a DOS
    /// header, a PE header, a PE32 optional header for the GUI subsystem and
    /// one section of 0x200 bytes at 0x200), then NSIS's first header, which
    /// says 0x20 bytes follow from it, and the rest of those.
    const fn stub(machine: u16) -> [u8; 0x420] {
        const fn put(b: &mut [u8; 0x420], at: usize, bytes: &[u8]) {
            let mut i = 0;
            while i < bytes.len() {
                b[at + i] = bytes[i];
                i += 1;
            }
        }
        let mut b = [0u8; 0x420];
        put(&mut b, 0, b"MZ");
        put(&mut b, 0x3C, &0x40u32.to_le_bytes());
        put(&mut b, 0x40, b"PE\0\0");
        // COFF: machine, one section, 224 bytes of optional header, an
        // executable 32-bit image.
        put(&mut b, 0x44, &machine.to_le_bytes());
        put(&mut b, 0x46, &1u16.to_le_bytes());
        put(&mut b, 0x54, &224u16.to_le_bytes());
        put(&mut b, 0x56, &0x0102u16.to_le_bytes());
        // Optional header at 0x58: PE32, subsystem GUI.
        put(&mut b, 0x58, &0x010Bu16.to_le_bytes());
        put(&mut b, 0x58 + 68, &2u16.to_le_bytes());
        // The section table at 0x58 + 224.
        put(&mut b, 0x138, b".text\0\0\0");
        put(&mut b, 0x138 + 16, &0x200u32.to_le_bytes());
        put(&mut b, 0x138 + 20, &0x200u32.to_le_bytes());
        // firstheader where the section's data ends.
        put(&mut b, 0x404, &0xDEAD_BEEFu32.to_le_bytes());
        put(&mut b, 0x408, b"NullsoftInst");
        put(&mut b, 0x414, &0x10u32.to_le_bytes());
        put(&mut b, 0x418, &0x20u32.to_le_bytes());
        b
    }

    pub(crate) const SETUP: &[u8] = &stub(0x014C);

    #[test]
    fn a_good_signature_checks_out_and_every_kind_of_bad_one_does_not() {
        let key = Key::new(*b"luna-key");
        let sig = key.sign(SETUP, "0.20.0");
        assert_eq!(verify(SETUP, &sig, &key.public(), &v("0.20.0")), Ok(()));
        // The file changed.
        let mut other = SETUP.to_vec();
        other[5] ^= 1;
        assert!(verify(&other, &sig, &key.public(), &v("0.20.0")).unwrap_err().contains("does not check out"));
        // Signed for another version: an old release passed off as new.
        assert!(verify(SETUP, &key.sign(SETUP, "0.19.0"), &key.public(), &v("0.20.0")).unwrap_err().contains("signed for 0.19.0"));
        // No version in the comment.
        let bare = key.sign_with_comment(SETUP, "timestamp:1763500000\tfile:setup.exe");
        assert!(verify(SETUP, &bare, &key.public(), &v("0.20.0")).unwrap_err().contains("names no version"));
        // Another key, with another id, and one forging this key's id.
        let stranger = Key::new(*b"stranger");
        assert!(verify(SETUP, &stranger.sign(SETUP, "0.20.0"), &key.public(), &v("0.20.0")).is_err());
        let forger = Key::new(*b"luna-key");
        assert!(verify(SETUP, &forger.sign(SETUP, "0.20.0"), &key.public(), &v("0.20.0")).is_err());
        // A trusted comment changed after signing.
        let text = String::from_utf8(base64::engine::general_purpose::STANDARD.decode(&sig).unwrap()).unwrap();
        let edited = b64(text.replace("version:0.20.0", "version:0.21.0"));
        assert!(verify(SETUP, &edited, &key.public(), &v("0.21.0")).is_err());
        assert!(verify(SETUP, "not base64!", &key.public(), &v("0.20.0")).is_err());
    }

    /// A Tauri-made key and signature (prehashed, as `tauri signer sign`
    /// makes them): fixtures/updater/README.md says how they were made.
    #[test]
    fn a_signature_made_by_tauris_signer_checks_out() {
        // In the test exe: the Windows tests run away from the tree they were built in.
        let data = include_bytes!("../../fixtures/updater/setup.bin");
        let sig = include_str!("../../fixtures/updater/setup.bin.sig");
        let key = include_str!("../../fixtures/updater/key.pub");
        // The .pub file as tauri signer writes it is the base64 the config holds.
        assert_eq!(verify(data, sig, key, &v("0.20.0")), Ok(()));
        assert!(verify(data, sig, key, &v("0.20.1")).is_err());
        assert!(verify(b"other bytes", sig, key, &v("0.20.0")).is_err());
        // Luna's real key is not the one that signed it.
        assert!(verify(data, sig, PUBKEY, &v("0.20.0")).unwrap_err().contains("does not check out"));
    }

    #[test]
    fn the_endpoint_and_key_are_tauris_config_ones() {
        let conf: serde_json::Value = serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
        assert_eq!(conf["plugins"]["updater"]["pubkey"], PUBKEY);
        assert_eq!(conf["plugins"]["updater"]["endpoints"], serde_json::json!([ENDPOINT]));
        let text = String::from_utf8(base64::engine::general_purpose::STANDARD.decode(PUBKEY).unwrap()).unwrap();
        assert!(minisign_verify::PublicKey::decode(&text).is_ok());
        assert_eq!(Config::luna().current, v(env!("CARGO_PKG_VERSION")));
    }

    /// latest.json as release-lib.ps1 `New-UpdaterManifest` writes it.
    fn manifest(version: &str, platform: &str) -> String {
        serde_json::json!({
            "version": version,
            "core_fingerprint": "0123456789abcdef",
            "notes": "Luna in a native window",
            "pub_date": "2026-11-30T12:00:00Z",
            "platforms": { platform: {
                "signature": "c2ln",
                "url": format!("https://github.com/nrubinmosin/luna/releases/download/v{version}/Luna_{version}_x64-setup.exe"),
            } },
        })
        .to_string()
    }

    /// latest.json offering `version` for this platform at `url`, so signed.
    pub(crate) fn manifest_for(version: &str, url: &str, signature: &str) -> String {
        let platform = targets().last().unwrap().clone();
        serde_json::json!({ "version": version, "platforms": { platform: { "signature": signature, "url": url } } }).to_string()
    }

    fn windows() -> Vec<String> {
        vec!["windows-x86_64-nsis".into(), "windows-x86_64".into()]
    }

    #[test]
    fn a_newer_release_is_offered_with_everything_lunas_manifest_says() {
        let got = parse(manifest("0.20.0", "windows-x86_64").as_bytes(), &v("0.19.0"), &windows()).unwrap().unwrap();
        assert_eq!(got.version, v("0.20.0"));
        assert_eq!(got.notes.as_deref(), Some("Luna in a native window"));
        assert_eq!(got.core_fingerprint.as_deref(), Some("0123456789abcdef"));
        assert_eq!(got.signature, "c2ln");
        assert!(got.url.ends_with("/v0.20.0/Luna_0.20.0_x64-setup.exe"));
        // The -nsis key first, as the plugin looks.
        let both = r#"{"version":"0.20.0","platforms":{
            "windows-x86_64":{"url":"https://a/plain.exe","signature":"p"},
            "windows-x86_64-nsis":{"url":"https://a/nsis.exe","signature":"n"}}}"#;
        assert_eq!(parse(both.as_bytes(), &v("0.19.0"), &windows()).unwrap().unwrap().signature, "n");
        // The plugin's other shape, and a v before the version.
        let dynamic = r#"{"version":"v0.21.0","url":"https://a/b.exe","signature":"s"}"#;
        assert_eq!(parse(dynamic.as_bytes(), &v("0.20.0"), &windows()).unwrap().unwrap().version, v("0.21.0"));
    }

    #[test]
    fn the_same_or_an_older_version_is_no_update_whatever_it_lacks() {
        for older in ["0.19.0", "0.18.3"] {
            assert_eq!(parse(manifest(older, "darwin-aarch64").as_bytes(), &v("0.19.0"), &windows()), Ok(None));
        }
        // A pre-release of the running version is older than it.
        assert_eq!(parse(manifest("0.20.0-rc.1", "windows-x86_64").as_bytes(), &v("0.20.0"), &windows()), Ok(None));
    }

    #[test]
    fn a_manifest_that_does_not_read_is_an_error() {
        let current = v("0.19.0");
        assert!(parse(b"<html>", &current, &windows()).is_err());
        assert!(parse(manifest("zero.twenty", "windows-x86_64").as_bytes(), &current, &windows()).unwrap_err().contains("not a version"));
        assert!(parse(manifest("0.20.0", "darwin-aarch64").as_bytes(), &current, &windows()).unwrap_err().contains("nothing for"));
        let dated = manifest("0.20.0", "windows-x86_64").replace("2026-11-30T12:00:00Z", "last Tuesday");
        assert!(parse(dated.as_bytes(), &current, &windows()).unwrap_err().contains("pub_date"));
        let file = manifest("0.20.0", "windows-x86_64").replace("https://github.com", "file://c:");
        assert!(parse(file.as_bytes(), &current, &windows()).is_err());
        let plain = manifest("0.20.0", "windows-x86_64").replace("https://", "http://");
        assert!(parse(plain.as_bytes(), &current, &windows()).unwrap_err().contains("not https"));
    }

    #[test]
    fn this_platform_is_looked_up_the_plugins_way() {
        let keys = targets();
        if cfg!(all(windows, target_arch = "x86_64")) {
            assert_eq!(keys, windows());
        }
        assert!(keys.last().unwrap().contains('-'));
    }

    #[test]
    fn the_installer_is_told_to_update_quietly_and_start_luna_again() {
        assert_eq!(installer_parameters(&[]), "/P /UPDATE /R /ARGS");
        let args = ["--flag".to_string(), "C:/a b".to_string(), "".to_string(), r"tail\".to_string()];
        assert_eq!(installer_parameters(&args), r#"/P /UPDATE /R /ARGS --flag "C:/a b" "" tail\"#);
        assert_eq!(nsis_arg(r"C:\dir with space\"), r#""C:\dir with space\\""#);
        assert_eq!(nsis_arg(r#"say "hi""#), r#""say \"hi\"""#);
    }

    #[test]
    fn only_arguments_that_reach_luna_as_they_are_are_passed_on() {
        let os = |s: &str| OsString::from(s);
        let args = [os("--after"), os("4242"), os(r"C:\Users\a b\x"), os(""), os(r#"say "hi""#), os("/S x"), os("/D=C:\\evil"), os("a\nb")];
        assert_eq!(restart_args(args), ["--after", "4242", r"C:\Users\a b\x", ""]);
        // Not Unicode: an unpaired surrogate, or bytes that are not UTF-8.
        #[cfg(windows)]
        let odd = {
            use std::os::windows::ffi::OsStringExt;
            OsString::from_wide(&[0x61, 0xD800])
        };
        #[cfg(unix)]
        let odd = {
            use std::os::unix::ffi::OsStringExt;
            OsString::from_vec(vec![0x61, 0xFF])
        };
        assert_eq!(restart_args([odd, os("--flag")]), ["--flag"]);
    }

    #[test]
    fn the_installer_is_staged_under_the_apps_name_whatever_the_url_says() {
        assert_eq!(file_name(&v("0.20.0")), "Luna_0.20.0_x64-setup.exe");
        assert_eq!(file_name(&v("0.21.0-rc.1")), "Luna_0.21.0-rc.1_x64-setup.exe");
    }

    #[test]
    fn only_an_nsis_setup_passes_for_one() {
        assert!(is_nsis_setup(SETUP));
        // An x64 or arm64 executable is no NSIS stub.
        assert!(!is_nsis_setup(&stub(0x8664)), "x64");
        assert!(!is_nsis_setup(&stub(0xAA64)), "arm64");
        assert!(!is_nsis_setup(b"MZ\x90\x00 and no PE header"));
        let changed = |at: usize, bytes: &[u8]| {
            let mut b = SETUP.to_vec();
            b[at..at + bytes.len()].copy_from_slice(bytes);
            b
        };
        assert!(!is_nsis_setup(&changed(0x3C, &u32::MAX.to_le_bytes())), "e_lfanew out of the file");
        assert!(!is_nsis_setup(&changed(0, b"ZM")), "no MZ");
        assert!(!is_nsis_setup(&changed(0x56, &0x2102u16.to_le_bytes())), "a DLL");
        assert!(!is_nsis_setup(&changed(0x56, &0x0100u16.to_le_bytes())), "not executable");
        assert!(!is_nsis_setup(&changed(0x58, &0x020Bu16.to_le_bytes())), "PE32+");
        assert!(!is_nsis_setup(&changed(0x58 + 68, &3u16.to_le_bytes())), "a console program");
        assert!(!is_nsis_setup(&changed(0x408, b"NullsoftInsx")), "no first header");
        assert!(!is_nsis_setup(&changed(0x404, &0u32.to_le_bytes())), "no 0xDEADBEEF");
        assert!(!is_nsis_setup(&changed(0x418, &0x21u32.to_le_bytes())), "data said to run past the end");
        assert!(!is_nsis_setup(&changed(0x138 + 20, &0x300u32.to_le_bytes())), "the first header elsewhere");
        assert!(!is_nsis_setup(&SETUP[..0x41F]), "cut short");
    }

    /// A real setup's start, as `fixtures/updater/README.md` says it was
    /// taken: its first KB (headers and section table), then the 28 bytes of
    /// NSIS's first header. Laid out again as the setup has them, the rest
    /// zeros.
    fn real_setup(head: &[u8]) -> Vec<u8> {
        let (stub_head, first) = head.split_at(1024);
        let end = stub_end(stub_head).expect("a PE header with its sections");
        let following = u32::from_le_bytes(first[24..28].try_into().unwrap()) as usize;
        let mut bytes = stub_head.to_vec();
        bytes.resize(end, 0);
        bytes.extend_from_slice(first);
        bytes.resize(end + following, 0);
        bytes
    }

    /// The defect a re-review found: the check asked for an AMD64 image, and
    /// every NSIS setup's stub is x86, the 0.19 release's and the one
    /// `build-windows.ps1 -Installer -Native` makes alike.
    #[test]
    fn real_nsis_setups_pass_for_one() {
        for head in [
            &include_bytes!("../../fixtures/updater/setup-0.19.0.head")[..],
            &include_bytes!("../../fixtures/updater/setup-native.head")[..],
        ] {
            assert_eq!(head.len(), 1024 + 28);
            let pe = u32::from_le_bytes(head[0x3C..0x40].try_into().unwrap()) as usize;
            assert_eq!(head[pe + 4..pe + 6], 0x014Cu16.to_le_bytes(), "an x86 stub");
            let setup = real_setup(head);
            assert!(is_nsis_setup(&setup));
            assert!(!is_nsis_setup(&setup[..setup.len() - 1]), "its data cut short");
        }
    }

    /// The setup at `LUNA_CHECK_SETUP`, the one a build just made, passes
    /// the updater's own check. Run by hand after a build:
    /// `LUNA_CHECK_SETUP=<setup> <test exe> --ignored --exact desktop::updater::tests::the_setup_in_luna_check_setup_passes_for_one`.
    #[test]
    #[ignore = "needs a built setup, named in LUNA_CHECK_SETUP"]
    fn the_setup_in_luna_check_setup_passes_for_one() {
        let path = std::env::var_os("LUNA_CHECK_SETUP").expect("LUNA_CHECK_SETUP names no setup");
        let bytes = std::fs::read(&path).unwrap();
        assert!(is_nsis_setup(&bytes), "{} is not one the updater takes", Path::new(&path).display());
    }

    #[test]
    fn the_restart_arguments_go_only_when_the_whole_command_line_fits() {
        let file = r"C:\Users\a\AppData\Local\Luna\updates\Luna_0.21.0_x64-setup.exe";
        let args = ["--after".to_string(), "4242".to_string()];
        assert_eq!(parameters_for(file, &args).unwrap(), "/P /UPDATE /R /ARGS --after 4242");
        // Exactly at the limit, and one unit over: then none of them, not some.
        let bare = installer_parameters(&[]);
        let room = SETUP_CMDLINE_MAX - (file.len() + 5 + bare.len() + 1);
        assert_eq!(parameters_for(file, &["x".repeat(room)]).unwrap(), format!("{bare} {}", "x".repeat(room)));
        assert_eq!(parameters_for(file, &["x".repeat(room + 1)]).unwrap(), bare);
        assert_eq!(parameters_for(file, &[args[0].clone(), "x".repeat(room)]).unwrap(), bare);
        // Counted in UTF-16 units, as Windows and NSIS count: a character
        // outside the BMP takes two.
        assert_eq!(parameters_for(file, &["\u{1F319}".repeat(room / 2 + 1)]).unwrap(), bare);
        let long = format!(r"C:\{}\setup.exe", "d".repeat(SETUP_CMDLINE_MAX));
        assert!(parameters_for(&long, &[]).unwrap_err().contains("too long"));
    }

    #[test]
    fn a_final_path_is_started_by_its_plain_form_only_where_that_names_the_same_file() {
        assert_eq!(dos_path(r"\\?\C:\Users\a\Luna\updates\setup.exe").as_deref(), Some(r"C:\Users\a\Luna\updates\setup.exe"));
        assert_eq!(dos_path(r"\\?\UNC\server\share\Luna\setup.exe").as_deref(), Some(r"\\server\share\Luna\setup.exe"));
        // Read plainly, Windows would drop the dot or the space.
        assert_eq!(dos_path(r"\\?\C:\Luna.\updates\setup.exe"), None);
        assert_eq!(dos_path(r"\\?\C:\Luna \updates\setup.exe"), None);
        // A volume GUID, or no \\?\ at all: not what GetFinalPathNameByHandleW gives for VOLUME_NAME_DOS.
        assert_eq!(dos_path(r"\\?\Volume{0b1c}\setup.exe"), None);
        assert_eq!(dos_path(r"C:\setup.exe"), None);
    }

    /// A link to the folder `target` at `link`: a junction on Windows, which
    /// takes no privilege, a symlink elsewhere.
    fn link_dir(link: &Path, target: &Path) {
        #[cfg(windows)]
        {
            let made = std::process::Command::new("cmd").args(["/d", "/c", "mklink", "/J"]).arg(link).arg(target).output().unwrap();
            assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stdout));
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).unwrap();
    }

    /// The link `link_dir` made taken away, what it led to left as it is.
    fn unlink_dir(link: &Path) {
        #[cfg(windows)]
        std::fs::remove_dir(link).unwrap();
        #[cfg(unix)]
        std::fs::remove_file(link).unwrap();
    }

    /// A server that answers each of `answers` in turn, one request each.
    pub(crate) fn serve(answers: Vec<Vec<u8>>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for answer in answers {
                let (mut conn, _) = listener.accept().unwrap();
                let mut buf = [0u8; 4096];
                let _ = conn.read(&mut buf);
                let _ = conn.write_all(&answer);
            }
        });
        format!("http://{addr}")
    }

    pub(crate) fn ok(body: &[u8]) -> Vec<u8> {
        let mut answer = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
        answer.extend_from_slice(body);
        answer
    }

    /// A data dir of the test's own, made, empty.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("luna-updater-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn release(base: &str, name: &str, signature: String) -> Release {
        Release { version: v("0.20.0"), notes: None, pub_date: None, url: format!("{base}/{name}"), signature, core_fingerprint: None }
    }

    #[test]
    fn an_update_is_checked_before_anything_is_written_and_staged_privately_under_the_apps_name() {
        let key = Key::new(*b"luna-key");
        let config = Config { endpoint: String::new(), pubkey: key.public(), current: v("0.19.0") };
        // The URL names a script; the file is staged as the setup all the same.
        let base = serve(vec![ok(SETUP)]);
        let root = scratch("good");
        let dir = downloads_dir(&root);
        let mut seen = Vec::new();
        let got =
            download(&config, &release(&base, "evil.bat?x=1", key.sign(SETUP, "0.20.0")), &root, |n, total| seen.push((n, total))).unwrap();
        assert_eq!(got.path(), dir.join("Luna_0.20.0_x64-setup.exe"));
        assert_eq!(std::fs::read(got.path()).unwrap(), SETUP);
        assert_eq!(seen.last(), Some(&(SETUP.len() as u64, Some(SETUP.len() as u64))));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
            assert_eq!(std::fs::metadata(got.path()).unwrap().permissions().mode() & 0o777, 0o600);
        }
        #[cfg(windows)]
        assert!(crate::secret::acl::is_private(&dir).unwrap());
        clear_downloads(&root);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&root);

        // Signed for another release, or not a setup: nothing written.
        let x64 = stub(0x8664);
        let base = serve(vec![ok(SETUP), ok(&x64)]);
        let root = scratch("bad");
        assert!(download(&config, &release(&base, "a.exe", key.sign(SETUP, "0.19.5")), &root, |_, _| {})
            .unwrap_err()
            .contains("signed for"));
        assert!(download(&config, &release(&base, "a.exe", key.sign(&x64, "0.20.0")), &root, |_, _| {}).unwrap_err().contains("NSIS"));
        assert!(!downloads_dir(&root).exists());

        // Refused before anything is fetched: plain http, and no newer than this build.
        let plain = release("http://example.com", "a.exe", key.sign(SETUP, "0.20.0"));
        assert!(download(&config, &plain, &root, |_, _| {}).unwrap_err().contains("not https"));
        let same = Config { current: v("0.20.0"), ..config };
        let ours = release("https://example.com", "a.exe", key.sign(SETUP, "0.20.0"));
        assert!(download(&same, &ours, &root, |_, _| {}).unwrap_err().contains("not newer"));
        assert!(!downloads_dir(&root).exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The data dir an update is staged under, and read again from, is no
    /// link or junction: one there would put `updates/` wherever it leads.
    #[test]
    fn nothing_is_staged_under_a_data_dir_that_is_a_link() {
        let key = Key::new(*b"luna-key");
        let config = Config { endpoint: String::new(), pubkey: key.public(), current: v("0.19.0") };
        let base = scratch("root-link");
        let real = base.join("real");
        std::fs::create_dir(&real).unwrap();
        let link = base.join("link");
        link_dir(&link, &real);
        assert!(check_root(&real).is_ok());
        assert!(check_root(&link).unwrap_err().contains("not a plain folder"));
        std::fs::write(base.join("file"), b"").unwrap();
        assert!(check_root(&base.join("file")).unwrap_err().contains("not a plain folder"));
        assert!(check_root(&base.join("missing")).is_err());
        // Refused before anything is fetched: the URL has no server.
        let unserved = release("http://127.0.0.1:9", "a.exe", key.sign(SETUP, "0.20.0"));
        assert!(download(&config, &unserved, &link, |_, _| {}).unwrap_err().contains("not a plain folder"));
        assert!(!real.join("updates").exists());

        // Staged under a plain one, which a link then takes the place of:
        // refused at the install.
        let served = serve(vec![ok(SETUP)]);
        let got = download(&config, &release(&served, "a.exe", key.sign(SETUP, "0.20.0")), &real, |_, _| {}).unwrap();
        std::fs::rename(&real, base.join("moved")).unwrap();
        link_dir(&real, &base.join("moved"));
        assert!(install(&config, &got).unwrap_err().contains("not a plain folder"));
        unlink_dir(&real);
        unlink_dir(&link);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The gap between the check and the start: `install` checks the bytes
    /// it reads through a handle it keeps open until the setup has started
    /// (`hold`), which nobody may write, rename or delete through meanwhile;
    /// a file changed before that handle is opened is refused.
    #[test]
    fn the_staged_installer_stays_what_was_checked_until_it_starts() {
        let key = Key::new(*b"luna-key");
        let config = Config { endpoint: String::new(), pubkey: key.public(), current: v("0.19.0") };
        let base = serve(vec![ok(SETUP)]);
        let root = scratch("held");
        let dir = downloads_dir(&root);
        let got = download(&config, &release(&base, "setup.exe", key.sign(SETUP, "0.20.0")), &root, |_, _| {}).unwrap();

        let held = hold(&config, &got).unwrap();
        #[cfg(windows)]
        {
            // ERROR_SHARING_VIOLATION, each of them.
            let shared = |r: std::io::Result<()>| r.unwrap_err().raw_os_error() == Some(32);
            assert!(shared(std::fs::OpenOptions::new().write(true).open(got.path()).map(drop)), "written while held");
            assert!(shared(std::fs::rename(got.path(), dir.join("moved.exe"))), "renamed while held");
            assert!(shared(std::fs::remove_file(got.path())), "deleted while held");
            // Reading it, as Windows' loader does, still goes.
            assert_eq!(std::fs::read(got.path()).unwrap(), SETUP);
        }
        drop(held);

        // Changed before the hold: refused, and so never started.
        let mut changed = SETUP.to_vec();
        changed[0x48] ^= 1;
        std::fs::write(got.path(), changed).unwrap();
        assert!(install(&config, &got).unwrap_err().contains("does not check out"));
        // A link in its place is not followed.
        #[cfg(unix)]
        {
            let elsewhere = dir.join("elsewhere.exe");
            std::fs::write(&elsewhere, SETUP).unwrap();
            std::fs::remove_file(got.path()).unwrap();
            std::os::unix::fs::symlink(&elsewhere, got.path()).unwrap();
            assert!(hold(&config, &got).is_err());
        }
        // No newer than this build any more: refused whatever the file.
        let later = Config { current: v("0.20.0"), ..config };
        assert!(install(&later, &got).unwrap_err().contains("not newer"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// What the setup is started by is where the held file is, through no
    /// junction: one above the data dir retargeted after the check leads the
    /// start nowhere else. While it is held, neither the file nor a folder
    /// on that path can be renamed, so that path keeps naming it.
    #[cfg(windows)]
    #[test]
    fn the_setup_is_started_by_the_path_its_handle_resolves_to() {
        let key = Key::new(*b"luna-key");
        let config = Config { endpoint: String::new(), pubkey: key.public(), current: v("0.19.0") };
        let base = scratch("final");
        std::fs::create_dir_all(base.join("real").join("data")).unwrap();
        let link = base.join("link");
        link_dir(&link, &base.join("real"));
        let root = link.join("data");
        let served = serve(vec![ok(SETUP)]);
        let got = download(&config, &release(&served, "a.exe", key.sign(SETUP, "0.20.0")), &root, |_, _| {}).unwrap();

        let held = hold(&config, &got).unwrap();
        let started = final_path(&held.file).unwrap();
        let real = std::fs::canonicalize(base.join("real")).unwrap();
        let expected = dos_path(&real.join("data").join("updates").join(file_name(&v("0.20.0"))).to_string_lossy()).unwrap();
        assert!(started.eq_ignore_ascii_case(&expected), "{started} is not {expected}");
        assert!(!started.to_ascii_lowercase().contains(r"\link\"));
        // The folders on that path stay where they are while it is held.
        let real_data = base.join("real").join("data");
        assert!(std::fs::rename(real_data.join("updates"), real_data.join("moved")).is_err(), "updates/ renamed while held");
        assert!(std::fs::rename(&real_data, base.join("real").join("moved")).is_err(), "the data dir renamed while held");
        // The junction above it may go; the path started by does not pass it.
        unlink_dir(&link);
        assert_eq!(std::fs::read(&started).unwrap(), SETUP);
        drop(held);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// An update is staged on and started from a local fixed drive only: a
    /// share is refused before anything asks the network.
    #[cfg(windows)]
    #[test]
    fn only_a_local_fixed_drive_takes_an_update() {
        assert_eq!(fixed_drive(r"\\server\share\Luna\updates\setup.exe"), Err("on a network share"));
        let temp = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        let temp = dos_path(&temp.to_string_lossy()).unwrap();
        assert_eq!(fixed_drive(&temp), Ok(()), "{temp}");
        let root = scratch("fixed");
        assert_eq!(check_root(&root), Ok(()));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The file a download writes is taken only where it is the one at its
    /// name in the folder held through the write.
    #[test]
    fn a_staged_file_is_taken_only_at_its_name_in_the_pinned_folder() {
        let root = scratch("pin");
        let dir = downloads_dir(&root);
        crate::secret::private_dir(&dir).unwrap();
        let pin = Pin::open(&dir).unwrap();
        let path = dir.join("setup.exe");
        crate::secret::write_private(&path, SETUP).unwrap();
        assert_eq!(pin.holds(&path, "setup.exe"), Ok(()));
        // The same bytes elsewhere are not the file in the folder.
        let elsewhere = root.join("setup.exe");
        crate::secret::write_private(&elsewhere, SETUP).unwrap();
        assert!(pin.holds(&elsewhere, "setup.exe").unwrap_err().contains("not the installer just staged"));
        // Held, the folder can be neither renamed nor deleted.
        #[cfg(windows)]
        assert!(std::fs::rename(&dir, root.join("moved")).is_err(), "renamed while pinned");
        drop(pin);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_check_reads_the_endpoints_manifest() {
        let body = manifest("0.20.0", targets().last().unwrap());
        let base = serve(vec![ok(body.as_bytes()), b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n".to_vec()]);
        let config = Config { endpoint: format!("{base}/latest.json"), pubkey: PUBKEY.into(), current: v("0.19.0") };
        assert_eq!(check(&config).unwrap().unwrap().version, v("0.20.0"));
        assert_eq!(check(&config), Ok(None));
    }
}
