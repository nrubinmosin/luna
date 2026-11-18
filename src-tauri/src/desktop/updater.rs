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
//!   its page never asked (updates.store.ts).
//! - `download` fetches the installer into the data dir's `updates/` and
//!   checks its signature before it is written: minisign, on the plugin's
//!   own `minisign-verify`, against the public key, the version in the
//!   signature's trusted comment held to the release's. The plugin lets a
//!   signature without a version pass unless told otherwise; every Luna
//!   signature has carried one (scripts/updater-signature.mjs), so one
//!   without it is refused here.
//! - `install` checks the file once more and starts it the way the plugin
//!   starts an NSIS installer on Windows: `ShellExecuteW` "open" with
//!   `/P /UPDATE /R /ARGS <this run's arguments>` (passive, update mode,
//!   start Luna again afterwards, with those arguments). The caller then
//!   exits at once, as the plugin's `exit(0)` did: the installer stops a
//!   Luna that is still running before it replaces the exe.
//!   Off Windows an update is installed by hand.
//!
//! Requests go through `crate::http`, which blocks its thread: call these off
//! the window's loop.

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

/// An installer on disk whose signature checked out.
#[derive(Clone, Debug)]
pub struct Downloaded {
    pub path: PathBuf,
    pub version: semver::Version,
    signature: String,
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
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(format!("latest.json's url is not http(s): {url}"));
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

/// The installer's name in `dir`: the URL's own when it is a plain one.
fn file_name(release: &Release) -> String {
    let last = release.url.split(['?', '#']).next().unwrap_or("").rsplit('/').next().unwrap_or("");
    let plain = !last.is_empty() && !last.starts_with('.') && last.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
    if plain {
        last.to_string()
    } else {
        format!("Luna_{}_setup{}", release.version, std::env::consts::EXE_SUFFIX)
    }
}

/// Downloads `release`'s installer into `dir`, `progress` told the bytes so
/// far and the total when the server says it. Nothing is written unless the
/// signature checks out.
pub fn download(config: &Config, release: &Release, dir: &Path, mut progress: impl FnMut(u64, Option<u64>)) -> Result<Downloaded, String> {
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
    verify(&bytes, &release.signature, &config.pubkey, &release.version)?;
    // What the plugin runs as an NSIS installer: an executable.
    if cfg!(windows) && !bytes.starts_with(b"MZ") {
        return Err("the download is not a Windows executable".into());
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(file_name(release));
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

/// The installer's command line for an update, `args` this run's arguments
/// (its program name left out): passive, update mode, start Luna again
/// with them once installed.
pub fn installer_parameters(args: &[String]) -> String {
    let mut parts = vec!["/P".to_string(), "/UPDATE".to_string(), "/R".to_string(), "/ARGS".to_string()];
    parts.extend(args.iter().map(|a| nsis_arg(a)));
    parts.join(" ")
}

/// Starts the downloaded installer, its signature checked again first. On
/// success the caller exits now, so that the installer finds no Luna running.
pub fn install(config: &Config, downloaded: &Downloaded) -> Result<(), String> {
    let bytes = std::fs::read(&downloaded.path).map_err(|e| format!("{}: {e}", downloaded.path.display()))?;
    verify(&bytes, &downloaded.signature, &config.pubkey, &downloaded.version)?;
    #[cfg(windows)]
    {
        use windows::core::{w, HSTRING, PCWSTR};
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOW;
        let args: Vec<String> = std::env::args().skip(1).collect();
        let file = HSTRING::from(downloaded.path.as_os_str());
        let params = HSTRING::from(installer_parameters(&args));
        // SAFETY: strings that outlive the call.
        let done = unsafe { ShellExecuteW(None, w!("open"), &file, &params, PCWSTR::null(), SW_SHOW) };
        // ShellExecuteW says success with a value above 32.
        if done.0 as isize <= 32 {
            return Err(format!("the installer did not start (ShellExecuteW {})", done.0 as isize));
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = bytes;
        Err(format!("on this platform an update is installed by hand: {}", downloaded.path.display()))
    }
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

    const SETUP: &[u8] = b"MZ\x90\x00 a Windows executable, as far as the check goes";

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
        let args = ["--flag".to_string(), "C:/a b".to_string(), "".to_string(), r#"say "hi""#.to_string(), r"tail\".to_string()];
        assert_eq!(installer_parameters(&args), r#"/P /UPDATE /R /ARGS --flag "C:/a b" "" "say \"hi\"" tail\"#);
        assert_eq!(nsis_arg(r"C:\dir with space\"), r#""C:\dir with space\\""#);
        assert_eq!(nsis_arg("/S"), r#""/S""#);
    }

    #[test]
    fn the_installer_keeps_a_plain_name_and_gets_one_otherwise() {
        let mut r = parse(manifest("0.20.0", "windows-x86_64").as_bytes(), &v("0.19.0"), &windows()).unwrap().unwrap();
        assert_eq!(file_name(&r), "Luna_0.20.0_x64-setup.exe");
        r.url = "https://example.com/dl/../..%2Fevil.exe?x=1".into();
        assert_eq!(file_name(&r), format!("Luna_0.20.0_setup{}", std::env::consts::EXE_SUFFIX));
        r.url = "https://example.com/".into();
        assert_eq!(file_name(&r), format!("Luna_0.20.0_setup{}", std::env::consts::EXE_SUFFIX));
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

    #[test]
    fn an_update_is_checked_downloaded_and_its_signature_held_before_anything_is_written() {
        let key = Key::new(*b"luna-key");
        let config = Config { endpoint: String::new(), pubkey: key.public(), current: v("0.19.0") };
        let base = serve(vec![ok(SETUP), ok(SETUP)]);
        let release = Release {
            version: v("0.20.0"),
            notes: None,
            pub_date: None,
            url: format!("{base}/Luna_0.20.0_x64-setup.exe"),
            signature: key.sign(SETUP, "0.20.0"),
            core_fingerprint: None,
        };
        let dir = scratch("good");
        let mut seen = Vec::new();
        let got = download(&config, &release, &dir, |n, total| seen.push((n, total))).unwrap();
        assert_eq!(got.path, dir.join("Luna_0.20.0_x64-setup.exe"));
        assert_eq!(std::fs::read(&got.path).unwrap(), SETUP);
        assert_eq!(seen.last(), Some(&(SETUP.len() as u64, Some(SETUP.len() as u64))));
        // Swapped on disk after the download: refused before it runs.
        std::fs::write(&got.path, b"MZ but something else").unwrap();
        assert!(install(&config, &got).unwrap_err().contains("does not check out"));
        clear_downloads(&dir);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);

        // Signed for another release: nothing written.
        let forged = Release { signature: key.sign(SETUP, "0.19.5"), ..release };
        let dir = scratch("bad");
        assert!(download(&config, &forged, &dir, |_, _| {}).is_err());
        assert!(!dir.exists());
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
