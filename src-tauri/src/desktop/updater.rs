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
//!   release's, and the file an x64 Windows executable. The plugin lets a
//!   signature without a version pass unless told otherwise; every Luna
//!   signature has carried one (scripts/updater-signature.mjs), so one
//!   without it is refused here. It is staged in the data dir's `updates/`,
//!   a private folder (`secret::private_dir`), under a name the app gives
//!   it (`file_name`): `latest.json` is not signed, so its URL chooses
//!   nothing about the file that is started.
//! - `install` opens the staged file as itself (never through a link at its
//!   name), shared for reading alone, checks what it reads through that
//!   handle once more, and starts it while the handle is still open, so
//!   what runs is what was checked (`hold`). It starts it the way the
//!   plugin starts an NSIS installer on Windows: "open" with
//!   `/P /UPDATE /R /ARGS <this run's arguments>` (passive, update mode,
//!   start Luna again afterwards, with those arguments). The caller then
//!   exits at once, as the plugin's `exit(0)` did: the installer stops a
//!   Luna that is still running before it replaces the exe. Off Windows an
//!   update is installed by hand.
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

/// The data dir's folder for downloaded installers.
pub fn downloads_dir() -> PathBuf {
    crate::paths::data_dir().join("updates")
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

/// Whether `bytes` are a PE image for x64: `MZ`, then at the offset the DOS
/// header gives, `PE\0\0` and the machine 0x8664 (IMAGE_FILE_MACHINE_AMD64).
/// What the updater starts is Luna's x64 setup, on every platform: off
/// Windows it is only ever staged, never run.
pub fn is_x64_pe(bytes: &[u8]) -> bool {
    let Some(at) = bytes.get(0x3C..0x40).map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize) else {
        return false;
    };
    let header = bytes.get(at..at.saturating_add(6));
    bytes.starts_with(b"MZ") && header.is_some_and(|h| h[..4] == *b"PE\0\0" && h[4..6] == 0x8664u16.to_le_bytes())
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
    if !is_x64_pe(bytes) {
        return Err("the installer is not an x64 Windows executable".into());
    }
    Ok(())
}

/// Downloads `release`'s installer and stages it in `dir`, `progress` told
/// the bytes so far and the total when the server says it. Nothing is
/// written unless it checks out (`check_setup`). `dir` is made private, or
/// refused where it is a link or junction or someone else's
/// (`secret::private_dir`); the file is private from the moment it exists
/// (`secret::write_private`).
pub fn download(config: &Config, release: &Release, dir: &Path, mut progress: impl FnMut(u64, Option<u64>)) -> Result<Downloaded, String> {
    newer(config, &release.version)?;
    if !fetchable(&release.url) {
        return Err(format!("the installer's url is not https: {}", release.url));
    }
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
    crate::secret::private_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(file_name(&release.version));
    crate::secret::write_private(&path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Downloaded { path, version: release.version.clone(), signature: release.signature.clone() })
}

/// Clears what earlier downloads left in `dir`: an installer runs once.
pub fn clear_downloads(dir: &Path) {
    if let Ok(entries) = std::fs::read_dir(dir) {
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

/// A staged installer, open from its check until it has started: shared for
/// reading alone on Windows, so that nobody may write, rename or delete it
/// meanwhile, and opened as itself, never through a link or junction at its
/// name. What was checked is what was read through this handle.
struct Held {
    // Keeps the file as it is for as long as it is open.
    _file: std::fs::File,
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

/// Opens the staged installer (`Held`), holds it to a private file of
/// Luna's (a plain file of one name, a trusted owner's, nobody else may
/// open) and checks what it reads through that handle as `download` checked
/// the download.
fn hold(config: &Config, downloaded: &Downloaded) -> Result<Held, String> {
    use std::io::Read;
    let path = &downloaded.path;
    newer(config, &downloaded.version)?;
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
    Ok(Held { _file: file, path: path.clone() })
}

/// Starts the staged installer, checked again first through the handle it
/// stays open on until it has started (`hold`). On success the caller exits
/// now, so that the installer finds no Luna running.
pub fn install(config: &Config, downloaded: &Downloaded) -> Result<(), String> {
    let held = hold(config, downloaded)?;
    #[cfg(windows)]
    {
        let parameters = installer_parameters(&restart_args(std::env::args_os().skip(1)));
        launch(&held, &parameters)
    }
    #[cfg(not(windows))]
    {
        Err(format!("on this platform an update is installed by hand: {}", held.path.display()))
    }
}

/// Starts `held`'s installer with `parameters`, through `ShellExecuteExW`
/// "open" as the plugin did, which also brings up a UAC prompt should a
/// setup ask for one. `SEE_MASK_NOCLOSEPROCESS` and `SEE_MASK_NOASYNC` make
/// it return once the process is there, so the handle `held` keeps covers
/// the whole of the start; from then on the image is mapped, and Windows
/// lets nobody write the file.
#[cfg(windows)]
fn launch(held: &Held, parameters: &str) -> Result<(), String> {
    use windows::core::{w, HSTRING, PCWSTR};
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOW;
    let file = HSTRING::from(held.path.as_os_str());
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
mod tests {
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
    struct Key {
        pair: Ed25519KeyPair,
        id: [u8; 8],
    }

    impl Key {
        fn new(id: [u8; 8]) -> Key {
            let rng = ring::rand::SystemRandom::new();
            let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
            Key { pair: Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap(), id }
        }

        fn public(&self) -> String {
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

        fn sign(&self, data: &[u8], version: &str) -> String {
            self.sign_with_comment(data, &format!("timestamp:1763500000\tfile:setup.exe\tversion:{version}"))
        }
    }

    /// A setup as far as the checks go: a DOS header whose e_lfanew points at
    /// a PE header for `machine`, then filler.
    const fn pe(machine: u16) -> [u8; 0x50] {
        let mut b = [b'.'; 0x50];
        b[0] = b'M';
        b[1] = b'Z';
        b[0x3C] = 0x40;
        b[0x3D] = 0;
        b[0x3E] = 0;
        b[0x3F] = 0;
        b[0x40] = b'P';
        b[0x41] = b'E';
        b[0x42] = 0;
        b[0x43] = 0;
        let m = machine.to_le_bytes();
        b[0x44] = m[0];
        b[0x45] = m[1];
        b
    }

    const SETUP: &[u8] = &pe(0x8664);

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
    fn only_an_x64_windows_executable_passes_for_a_setup() {
        assert!(is_x64_pe(SETUP));
        assert!(!is_x64_pe(&pe(0x014C)), "x86");
        assert!(!is_x64_pe(&pe(0xAA64)), "arm64");
        assert!(!is_x64_pe(b"MZ\x90\x00 and no PE header"));
        let mut far = SETUP.to_vec();
        far[0x3C..0x40].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(!is_x64_pe(&far));
        let mut not_mz = SETUP.to_vec();
        not_mz[0] = b'Z';
        assert!(!is_x64_pe(&not_mz));
    }

    /// A server that answers each of `answers` in turn, one request each.
    fn serve(answers: Vec<Vec<u8>>) -> String {
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

    fn ok(body: &[u8]) -> Vec<u8> {
        let mut answer = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
        answer.extend_from_slice(body);
        answer
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("luna-updater-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
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
        let dir = scratch("good");
        let mut seen = Vec::new();
        let got =
            download(&config, &release(&base, "evil.bat?x=1", key.sign(SETUP, "0.20.0")), &dir, |n, total| seen.push((n, total))).unwrap();
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
        clear_downloads(&dir);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);

        // Signed for another release, or not a setup: nothing written.
        let base = serve(vec![ok(SETUP), ok(&pe(0x014C))]);
        let dir = scratch("bad");
        assert!(download(&config, &release(&base, "a.exe", key.sign(SETUP, "0.19.5")), &dir, |_, _| {})
            .unwrap_err()
            .contains("signed for"));
        let x86 = pe(0x014C);
        assert!(download(&config, &release(&base, "a.exe", key.sign(&x86, "0.20.0")), &dir, |_, _| {}).unwrap_err().contains("x64"));
        assert!(!dir.exists());

        // Refused before anything is fetched: plain http, and no newer than this build.
        let plain = release("http://example.com", "a.exe", key.sign(SETUP, "0.20.0"));
        assert!(download(&config, &plain, &dir, |_, _| {}).unwrap_err().contains("not https"));
        let same = Config { current: v("0.20.0"), ..config };
        let ours = release("https://example.com", "a.exe", key.sign(SETUP, "0.20.0"));
        assert!(download(&same, &ours, &dir, |_, _| {}).unwrap_err().contains("not newer"));
        assert!(!dir.exists());
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
        let dir = scratch("held");
        let got = download(&config, &release(&base, "setup.exe", key.sign(SETUP, "0.20.0")), &dir, |_, _| {}).unwrap();

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
        let _ = std::fs::remove_dir_all(&dir);
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
