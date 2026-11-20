//! The publisher's Authenticode signature on a downloaded CLI's executables,
//! on Windows, checked before a version is made current. The release's
//! checksum says the package is what the release lists; the signature says the
//! executables in it are the publisher's, which a checksum served beside the
//! package does not: a release whose package and `SHA256SUMS` were both
//! replaced still carries no valid signature of the publisher's.
//!
//! Every program file of a package (`is_pe`: by its extension, `.exe`, `.dll`
//! and the like, or by the `MZ` a Windows program opens with, whatever it is
//! called) must be signed, validly, by the named signer (the subject's common
//! name of the certificate that signed it), but those the source lists as
//! published unsigned (Codex's `rg.exe`, ripgrep's). A DLL may also be a
//! runtime the vendor bundles as Microsoft signed it (`dll_also`), and one
//! named file may carry one named other signer (`path_also`: Codex's bundled
//! `vcruntime140.dll`, which 0.171.0 ships signed by Microsoft's compatibility
//! publisher). A file with
//! a signature that does not match its bytes, a signer of another name or none
//! at all refuses the whole package. Not checked: revocation
//! (`WTD_REVOKE_NONE`, no network call at a download's end: Windows holds the
//! chain to its roots and the timestamp, not to a revocation list fetched
//! from the net). On other systems Authenticode has no meaning and nothing is
//! checked; the checksum and, for Claude Code, the signed manifest stand.
//!
//! What stays open (threat model): a file the vendor ships unsigned cannot be
//! held to anything but the checksum, which comes from the release beside the
//! package. Whoever controls both can swap that one file (`unsigned`, one
//! name for Codex).

use super::PackageFile;
use std::path::Path;

/// Who must have signed a package's program files, and which of them are
/// published unsigned.
pub struct Signers {
    /// The signing certificate's subject name (its common name).
    pub signer: &'static str,
    /// Other signers a `.dll` may carry: the runtime libraries the vendor
    /// bundles as Microsoft signed them.
    pub dll_also: &'static [&'static str],
    /// A file, by its path in the package (`/`-separated, any case), and one
    /// more signer it alone may carry: for a signer that also signs others'
    /// submissions, so held to that one path and no other file, of whatever
    /// name or kind.
    pub path_also: &'static [(&'static str, &'static str)],
    /// Files, by their path in the package (`/`-separated, any case), that the
    /// publisher ships unsigned. Exactly these and no others.
    pub unsigned: &'static [&'static str],
}

/// The extensions of files Windows loads as programs.
const PE_EXTENSIONS: [&str; 9] = [".exe", ".dll", ".sys", ".ocx", ".scr", ".cpl", ".com", ".node", ".pyd"];

/// Whether `rel` (at `path`) is a Windows program file: named like one, or
/// opening with `MZ` under any other name.
fn is_pe(rel: &str, path: &Path) -> bool {
    use std::io::Read;
    let lower = rel.to_ascii_lowercase();
    if PE_EXTENSIONS.iter().any(|e| lower.ends_with(e)) {
        return true;
    }
    let mut magic = [0u8; 2];
    std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut magic)).is_ok() && &magic == b"MZ"
}

/// Holds the executables of `files` (relative to `dir`) to `rule`: Ok with a
/// line for the record where they were checked, Ok(None) where this is not
/// Windows, Err, with the file and why, at the first that fails.
pub fn require(dir: &Path, files: &[PackageFile], rule: &Signers) -> Result<Option<String>, String> {
    #[cfg(windows)]
    {
        require_with(dir, files, rule, signer_of).map(Some)
    }
    #[cfg(not(windows))]
    {
        let _ = (dir, files, rule);
        Ok(None)
    }
}

/// `require` with the verdict on one file given by `verify`: the signer's name
/// where its signature is valid, why not where it is not. Off Windows only
/// the tests call it.
#[cfg_attr(not(windows), allow(dead_code))]
fn require_with(
    dir: &Path,
    files: &[PackageFile],
    rule: &Signers,
    verify: impl Fn(&Path) -> Result<String, String>,
) -> Result<String, String> {
    let mut checked = 0;
    let mut by_vendor = 0;
    for file in files {
        let path = dir.join(&file.rel);
        if !is_pe(&file.rel, &path) || rule.unsigned.iter().any(|u| u.eq_ignore_ascii_case(&file.rel)) {
            continue;
        }
        let signer = verify(&path).map_err(|why| format!("{} is {why}: refused", file.rel))?;
        let dll = file.rel.to_ascii_lowercase().ends_with(".dll");
        let excused = rule.path_also.iter().any(|(rel, by)| rel.eq_ignore_ascii_case(&file.rel) && *by == signer);
        if signer != rule.signer && !(dll && rule.dll_also.contains(&signer.as_str())) && !excused {
            return Err(format!("{} is signed by \"{signer}\", not \"{}\": refused", file.rel, rule.signer));
        }
        checked += 1;
        by_vendor += usize::from(signer == rule.signer);
    }
    if by_vendor == 0 {
        // A package with no program of the publisher's is not the package.
        return Err(format!("no program of the package is signed by \"{}\": refused", rule.signer));
    }
    Ok(format!("{checked} program files Authenticode-signed ({by_vendor} by {})", rule.signer))
}

/// The signer of the file at `path`, if Windows finds its embedded signature
/// valid and trusted; why not otherwise.
#[cfg(windows)]
fn signer_of(path: &Path) -> Result<String, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{HANDLE, HWND, INVALID_HANDLE_VALUE};
    use windows::Win32::Security::Cryptography::{CertGetNameStringW, CERT_NAME_SIMPLE_DISPLAY_TYPE};
    use windows::Win32::Security::WinTrust::{
        WTHelperGetProvCertFromChain, WTHelperGetProvSignerFromChain, WTHelperProvDataFromStateData, WinVerifyTrust,
        WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_PROVIDER_FLAGS, WINTRUST_FILE_INFO, WTD_CACHE_ONLY_URL_RETRIEVAL,
        WTD_CHOICE_FILE, WTD_REVOCATION_CHECK_NONE, WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE,
    };

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut file = WINTRUST_FILE_INFO {
        cbStruct: size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: PCWSTR(wide.as_ptr()),
        hFile: HANDLE::default(),
        pgKnownSubject: std::ptr::null_mut(),
    };
    // SAFETY: WINTRUST_DATA is plain integers, pointers and a union of
    // pointers; all zero is its empty state, and the fields WinVerifyTrust
    // reads are set below.
    let mut data: WINTRUST_DATA = unsafe { std::mem::zeroed() };
    data.cbStruct = size_of::<WINTRUST_DATA>() as u32;
    data.dwUIChoice = WTD_UI_NONE;
    data.fdwRevocationChecks = WTD_REVOKE_NONE;
    data.dwUnionChoice = WTD_CHOICE_FILE;
    data.Anonymous.pFile = &mut file;
    data.dwStateAction = WTD_STATEACTION_VERIFY;
    data.dwProvFlags = WINTRUST_DATA_PROVIDER_FLAGS(WTD_REVOCATION_CHECK_NONE.0 | WTD_CACHE_ONLY_URL_RETRIEVAL.0);
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    // No window to ask anything in: INVALID_HANDLE_VALUE, as the call says.
    let hwnd = HWND(INVALID_HANDLE_VALUE.0);

    // SAFETY: `data` points at `file`, which points at `wide`, all alive until
    // the CLOSE call below; the state handle it returns is read only between
    // the two calls, and the strings and structs it points into belong to it.
    let verdict = unsafe { WinVerifyTrust(hwnd, &mut action, std::ptr::addr_of_mut!(data).cast()) };
    let outcome = if verdict == 0 {
        // SAFETY: as above; every helper returns null where it finds nothing.
        unsafe {
            let provider = WTHelperProvDataFromStateData(data.hWVTStateData);
            let signer = if provider.is_null() { std::ptr::null_mut() } else { WTHelperGetProvSignerFromChain(provider, 0, false, 0) };
            let leaf = if signer.is_null() { std::ptr::null_mut() } else { WTHelperGetProvCertFromChain(signer, 0) };
            if leaf.is_null() || (*leaf).pCert.is_null() {
                Err("signed, but Windows gives no signing certificate".to_string())
            } else {
                let mut name = [0u16; 512];
                let n = CertGetNameStringW((*leaf).pCert, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, None, Some(&mut name)) as usize;
                Ok(String::from_utf16_lossy(&name[..n.saturating_sub(1).min(name.len())]))
            }
        }
    } else {
        Err(untrusted(verdict as u32))
    };
    data.dwStateAction = WTD_STATEACTION_CLOSE;
    // SAFETY: the same data, to release the state the first call made.
    unsafe { WinVerifyTrust(hwnd, &mut action, std::ptr::addr_of_mut!(data).cast()) };
    outcome
}

/// What a `WinVerifyTrust` verdict says of the file, in words.
#[cfg(windows)]
fn untrusted(verdict: u32) -> String {
    match verdict {
        0x800B_0100 => "not signed".into(),
        0x800B_0003 => "not a program file whose signature Windows can read".into(),
        0x8009_6010 => "signed, but its bytes no longer match the signature".into(),
        0x800B_0109 => "signed by a publisher whose certificate does not chain to a trusted root".into(),
        0x800B_0101 => "signed with a certificate that is not valid now and was not timestamped".into(),
        0x800B_010C => "signed with a revoked certificate".into(),
        0x800B_0004 => "signed by a publisher this machine does not trust".into(),
        other => format!("not verifiable (WinVerifyTrust {other:#010x})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(rels: &[&str]) -> Vec<PackageFile> {
        rels.iter().map(|r| PackageFile { rel: (*r).into(), size: 1, sha256: None }).collect()
    }

    const RULE: Signers = Signers {
        signer: "OpenAI OpCo, LLC",
        dll_also: &["Microsoft Windows"],
        path_also: &[("codex-resources/voice/bin/vcruntime140.dll", COMPAT)],
        unsigned: &["codex-path/rg.exe"],
    };
    const COMPAT: &str = "Microsoft Windows Software Compatibility Publisher";

    #[test]
    fn every_program_file_but_the_one_published_unsigned_must_be_the_publisher_s() {
        let package = files(&[
            "bin/codex.exe",
            "codex-path/rg.exe",
            "codex-resources/codex-command-runner.exe",
            "codex-package.json",
            "codex-resources/voice/bin/opus.dll",
        ]);
        let seen = std::cell::RefCell::new(Vec::new());
        let line = require_with(Path::new("d"), &package, &RULE, |p| {
            seen.borrow_mut().push(p.to_path_buf());
            Ok("OpenAI OpCo, LLC".to_string())
        })
        .unwrap();
        assert_eq!(
            seen.into_inner(),
            [
                Path::new("d").join("bin/codex.exe"),
                Path::new("d").join("codex-resources/codex-command-runner.exe"),
                Path::new("d").join("codex-resources/voice/bin/opus.dll")
            ]
        );
        assert!(line.starts_with("3 program files") && line.contains("OpenAI OpCo, LLC"), "{line}");
        // A DLL the vendor bundles as Microsoft signed it; an exe cannot be.
        let with_runtime = files(&["bin/codex.exe", "voice/vcruntime140.dll"]);
        let microsoft = |p: &Path| Ok(if p.ends_with("vcruntime140.dll") { "Microsoft Windows" } else { "OpenAI OpCo, LLC" }.to_string());
        assert!(require_with(Path::new("d"), &with_runtime, &RULE, microsoft).is_ok());
        let why = require_with(Path::new("d"), &files(&["bin/codex.exe", "x.exe"]), &RULE, |p| {
            Ok(if p.ends_with("x.exe") { "Microsoft Windows" } else { "OpenAI OpCo, LLC" }.to_string())
        })
        .unwrap_err();
        assert!(why.contains("x.exe is signed by \"Microsoft Windows\""), "{why}");
        // Any case of the exemption's path, and of an executable's extension.
        let loud = files(&["BIN/CODEX.EXE", "Codex-Path/RG.EXE"]);
        assert!(require_with(Path::new("d"), &loud, &RULE, |_| Ok("OpenAI OpCo, LLC".into())).is_ok());
    }

    #[test]
    fn an_unsigned_a_mismatched_or_another_publisher_s_executable_refuses_the_package() {
        let package = files(&["bin/codex.exe", "codex-resources/codex-command-runner.exe"]);
        let why = require_with(Path::new("d"), &package, &RULE, |p| {
            Ok(if p.ends_with("codex-command-runner.exe") { "Somebody Else Ltd" } else { "OpenAI OpCo, LLC" }.to_string())
        })
        .unwrap_err();
        assert!(why.contains("codex-command-runner.exe is signed by \"Somebody Else Ltd\", not \"OpenAI OpCo, LLC\""), "{why}");
        let why = require_with(Path::new("d"), &package, &RULE, |p| {
            if p.ends_with("codex.exe") {
                Err("not signed".into())
            } else {
                Ok("OpenAI OpCo, LLC".into())
            }
        })
        .unwrap_err();
        assert_eq!(why, "bin/codex.exe is not signed: refused");
        let why = require_with(Path::new("d"), &files(&["bin/codex.exe"]), &RULE, |_| {
            Err("signed, but its bytes no longer match the signature".into())
        })
        .unwrap_err();
        assert!(why.contains("no longer match"), "{why}");
        // Nothing to check is no package.
        let why = require_with(Path::new("d"), &files(&["codex-path/rg.exe", "x.json"]), &RULE, |_| Ok(String::new())).unwrap_err();
        assert!(why.contains("no program of the package is signed"), "{why}");
    }

    #[test]
    fn an_unsigned_dll_or_a_program_under_another_name_refuses_the_package() {
        let ok = |p: &Path| -> Result<String, String> {
            if p.ends_with("opus.dll") || p.ends_with("sneaky.bin") {
                Err("not signed".into())
            } else {
                Ok("OpenAI OpCo, LLC".into())
            }
        };
        let why = require_with(Path::new("d"), &files(&["bin/codex.exe", "voice/bin/opus.dll"]), &RULE, ok).unwrap_err();
        assert_eq!(why, "voice/bin/opus.dll is not signed: refused");
        // Only the one path is excused, not another file of the same name or kind.
        let why = require_with(Path::new("d"), &files(&["bin/codex.exe", "other/rg.exe"]), &RULE, |p| {
            if p.ends_with("other/rg.exe") {
                Err("not signed".into())
            } else {
                Ok("OpenAI OpCo, LLC".into())
            }
        })
        .unwrap_err();
        assert_eq!(why, "other/rg.exe is not signed: refused");
        // A program called something else is found by its `MZ`.
        let dir = std::env::temp_dir().join(format!("luna-authenticode-names-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("sneaky.bin"), b"MZ\x90\x00").unwrap();
        std::fs::write(dir.join("notes.txt"), b"hello").unwrap();
        std::fs::write(dir.join("codex.exe"), b"").unwrap();
        let package = files(&["codex.exe", "sneaky.bin", "notes.txt"]);
        let why = require_with(&dir, &package, &RULE, ok).unwrap_err();
        assert_eq!(why, "sneaky.bin is not signed: refused");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_compatibility_publisher_is_taken_on_its_one_path_only() {
        let signed = |odd: &'static str| {
            move |p: &Path| -> Result<String, String> { Ok(if p.ends_with(odd) { COMPAT } else { "OpenAI OpCo, LLC" }.to_string()) }
        };
        let package = files(&["bin/codex.exe", "codex-resources/voice/bin/opus.dll", "codex-resources/voice/bin/vcruntime140.dll"]);
        let line = require_with(Path::new("d"), &package, &RULE, signed("vcruntime140.dll")).unwrap();
        assert!(line.starts_with("3 program files") && line.contains("(2 by OpenAI OpCo, LLC)"), "{line}");
        // Any case of the path, as for `unsigned`.
        let loud = files(&["bin/codex.exe", "Codex-Resources/Voice/Bin/VCRUNTIME140.DLL"]);
        assert!(require_with(Path::new("d"), &loud, &RULE, signed("VCRUNTIME140.DLL")).is_ok());
        // The same signer anywhere else: another DLL, an exe, the same name in
        // another folder, the path with a prefix.
        for (rel, name) in [
            ("codex-resources/voice/bin/opus.dll", "opus.dll"),
            ("codex-resources/codex-command-runner.exe", "codex-command-runner.exe"),
            ("codex-resources/vcruntime140.dll", "vcruntime140.dll"),
            ("x/codex-resources/voice/bin/vcruntime140.dll", "vcruntime140.dll"),
        ] {
            let why = require_with(Path::new("d"), &files(&["bin/codex.exe", rel]), &RULE, signed(name)).unwrap_err();
            assert_eq!(why, format!("{rel} is signed by \"{COMPAT}\", not \"OpenAI OpCo, LLC\": refused"));
        }
        // On its path, a third signer is still refused.
        let why = require_with(Path::new("d"), &files(&["bin/codex.exe", "codex-resources/voice/bin/vcruntime140.dll"]), &RULE, |p| {
            Ok(if p.ends_with("vcruntime140.dll") { "Somebody Else Ltd" } else { "OpenAI OpCo, LLC" }.to_string())
        })
        .unwrap_err();
        assert!(why.contains("signed by \"Somebody Else Ltd\""), "{why}");
        // Not the publisher's: a package of that file alone is no package.
        let why = require_with(Path::new("d"), &files(&["codex-resources/voice/bin/vcruntime140.dll"]), &RULE, |_| Ok(COMPAT.into()))
            .unwrap_err();
        assert!(why.contains("no program of the package is signed"), "{why}");
    }

    #[cfg(not(windows))]
    #[test]
    fn off_windows_nothing_is_checked() {
        assert_eq!(require(Path::new("d"), &files(&["bin/codex"]), &RULE), Ok(None));
    }

    /// A PowerShell 7 of this machine, signed by Microsoft inside the file
    /// (an installed `pwsh.exe`; the system's own files are signed by catalog,
    /// which the check does not take). None where there is none to try.
    #[cfg(windows)]
    fn signed_exe() -> Option<std::path::PathBuf> {
        let candidates =
            [std::env::var_os("ProgramFiles").map(|p| std::path::PathBuf::from(p).join("PowerShell").join("7").join("pwsh.exe"))];
        candidates.into_iter().flatten().find(|p| p.is_file())
    }

    #[cfg(windows)]
    #[test]
    fn windows_names_a_signed_file_s_signer_and_refuses_an_unsigned_or_altered_one() {
        let Some(signed) = signed_exe() else { return };
        let signer = signer_of(&signed).unwrap();
        assert!(signer.contains("Microsoft"), "{signer}");
        let dir = std::env::temp_dir().join(format!("luna-authenticode-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A program with no signature (this test's own) is not signed; a file
        // that is no program has no signature to read.
        let plain = dir.join("plain.exe");
        std::fs::copy(std::env::current_exe().unwrap(), &plain).unwrap();
        assert_eq!(signer_of(&plain).unwrap_err(), "not signed");
        let text = dir.join("text.exe");
        std::fs::write(&text, b"MZ not a program").unwrap();
        assert!(signer_of(&text).unwrap_err().contains("not a program file"), "{:?}", signer_of(&text));
        // A byte changed inside the hashed part: the signature no longer holds.
        let mut bytes = std::fs::read(&signed).unwrap();
        let at = bytes.len() / 3;
        bytes[at] ^= 0xff;
        let altered = dir.join("altered.exe");
        std::fs::write(&altered, &bytes).unwrap();
        let why = signer_of(&altered).unwrap_err();
        assert!(why.contains("no longer match") || why.contains("not verifiable"), "{why}");
        // The same checks as a package's.
        let package = files(&["pwsh.exe"]);
        std::fs::copy(&signed, dir.join("pwsh.exe")).unwrap();
        let rule = Signers { signer: Box::leak(signer.into_boxed_str()), dll_also: &[], path_also: &[], unsigned: &[] };
        assert!(require(&dir, &package, &rule).unwrap().is_some());
        let other = Signers { signer: "Nobody", dll_also: &[], path_also: &[], unsigned: &[] };
        assert!(require(&dir, &package, &other).unwrap_err().contains("not \"Nobody\""));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
