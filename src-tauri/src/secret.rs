//! The secrets Luna mints for this run and the checks against them: the MCP
//! bearer tokens (agents.rs), the hook URLs' secrets (hub.rs) and the core's
//! token (coreproc.rs, `core.json`). Each one is all that keeps another local
//! process off a door on loopback, so it comes from the OS's CSPRNG, never
//! from anything a process on the same machine could reconstruct (time, pid,
//! addresses). The files that hold them are made here too, readable by
//! their owner alone.

use std::io;
use std::path::Path;

/// `bytes` from the OS's CSPRNG, as lowercase hex (twice as many characters).
/// With no randomness to be had the process cannot hand out a door it could
/// keep shut, so it stops rather than mint something weaker.
pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).expect("the OS gives no randomness");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// Whether a presented secret is the one expected, in time that depends only
/// on their lengths (`subtle`), so a caller timing the answer learns nothing
/// about how much of a guess was right. The lengths are no secret: every one
/// Luna mints is of a fixed size.
pub fn same(presented: &str, expected: &str) -> bool {
    use subtle::ConstantTimeEq;
    presented.as_bytes().ct_eq(expected.as_bytes()).into()
}

/// HMAC-SHA256 (RFC 2104) of `msg` under `key`, as lowercase hex: a secret
/// of its own for each `msg`, which nobody can work out from another's
/// without `key`.
pub fn hmac_hex(key: &[u8], msg: &[u8]) -> String {
    use hmac::{Hmac, Mac};
    // Any key length will do for HMAC; `new_from_slice` cannot fail for it.
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(key).expect("HMAC takes a key of any length");
    mac.update(msg);
    mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect()
}

/// Makes `dir` (and its parents) for files that hold a secret, and leaves it
/// readable by its owner alone, whatever an older Luna made it with. On Unix
/// that is 0700: with umask 022 and a traversable data dir, another OS user
/// could otherwise list it and read what is in it. On Windows it is a DACL of
/// its own, not inherited: the data dir is often the install folder, which
/// takes the ACL of wherever the user put it, and on a second drive that
/// commonly lets every signed-in user change anything (`acl::claim`).
pub fn private_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        if let Some(parent) = dir.parent() {
            std::fs::DirBuilder::new().recursive(true).mode(0o700).create(parent)?;
        }
        // SAFETY: geteuid has no preconditions and cannot fail.
        claim_private(dir, unsafe { libc::geteuid() }).map_err(io::Error::other)
    }
    #[cfg(windows)]
    {
        std::fs::create_dir_all(dir)?;
        acl::claim(dir, false)
    }
    #[cfg(not(any(unix, windows)))]
    std::fs::create_dir_all(dir)
}

/// Windows' side of `private_dir`, and of the install folder's protection
/// (paths.rs `protect_install_from_args`).
#[cfg(windows)]
pub mod acl {
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use windows::core::{BOOL, PCWSTR, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL};
    use windows::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetNamedSecurityInfoW, SetNamedSecurityInfoW,
        SDDL_REVISION_1, SE_FILE_OBJECT,
    };
    use windows::Win32::Security::{
        GetSecurityDescriptorDacl, GetTokenInformation, TokenUser, ACL, DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, TOKEN_QUERY, TOKEN_USER,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    /// BUILTIN\Administrators, the owner of what an elevated process makes.
    const ADMINISTRATORS: &str = "S-1-5-32-544";

    /// Gives `dir` a protected DACL — full control for this user, SYSTEM and
    /// Administrators, inherited by what is in it, nothing from above — and
    /// with `readable_by_users` read and execute for BUILTIN\Users as well,
    /// as Program Files grants. Written by well-known SIDs, so no account
    /// name in any language is involved. Refused first: a link or junction,
    /// which the DACL would be written through to wherever it points, and a
    /// folder owned by anyone but this user (or Administrators, for an
    /// elevated run): its owner may rewrite any DACL on it at will.
    pub fn claim(dir: &Path, readable_by_users: bool) -> io::Result<()> {
        let meta = std::fs::symlink_metadata(dir)?;
        if !meta.is_dir() {
            return Err(io::Error::other(format!("{} is not a folder (a link or a file is there)", dir.display())));
        }
        let user = user_sid()?;
        let owner = owner_sid(dir)?;
        if owner != user && owner != ADMINISTRATORS {
            return Err(io::Error::other(format!("{} belongs to someone else ({owner})", dir.display())));
        }
        // 0x1200a9: FILE_GENERIC_READ | FILE_GENERIC_EXECUTE.
        let users = if readable_by_users { "(A;OICI;0x1200a9;;;BU)" } else { "" };
        set_dacl(dir, &format!("D:P(A;OICI;FA;;;{user})(A;OICI;FA;;;SY)(A;OICI;FA;;;BA){users}"))
    }

    fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
        s.encode_wide().chain(Some(0)).collect()
    }

    /// The SID as text (`S-1-5-…`), for a SID the caller keeps alive.
    fn sid_text(sid: PSID) -> io::Result<String> {
        let mut text = PWSTR::null();
        // SAFETY: `sid` is valid for the call; the string it makes is
        // LocalAlloc'd, read here and freed once.
        unsafe {
            ConvertSidToStringSidW(sid, &mut text)?;
            let out = text.to_string();
            let _ = LocalFree(Some(HLOCAL(text.0.cast())));
            out.map_err(io::Error::other)
        }
    }

    /// The SID of the user this process runs as.
    pub fn user_sid() -> io::Result<String> {
        let mut token = HANDLE::default();
        // SAFETY: the token is opened for query only, read into a buffer of
        // the size it asks for (u64s, aligned for TOKEN_USER), and closed.
        unsafe {
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)?;
            let mut len = 0u32;
            let _ = GetTokenInformation(token, TokenUser, None, 0, &mut len);
            let mut buf = vec![0u64; (len as usize).div_ceil(8)];
            let got = GetTokenInformation(token, TokenUser, Some(buf.as_mut_ptr().cast()), len, &mut len);
            let _ = CloseHandle(token);
            got?;
            sid_text((*buf.as_ptr().cast::<TOKEN_USER>()).User.Sid)
        }
    }

    /// The SID of the owner of `path`.
    fn owner_sid(path: &Path) -> io::Result<String> {
        let name = wide(path.as_os_str());
        let (mut owner, mut sd) = (PSID::default(), PSECURITY_DESCRIPTOR::default());
        // SAFETY: `owner` points into `sd`, which is read before it is freed.
        unsafe {
            GetNamedSecurityInfoW(
                PCWSTR(name.as_ptr()),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION,
                Some(&mut owner),
                None,
                None,
                None,
                &mut sd,
            )
            .ok()?;
            let text = sid_text(owner);
            let _ = LocalFree(Some(HLOCAL(sd.0)));
            text
        }
    }

    /// Writes `sddl`'s DACL on `path`, protected from what is above it.
    fn set_dacl(path: &Path, sddl: &str) -> io::Result<()> {
        let name = wide(path.as_os_str());
        let text: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SAFETY: the descriptor is made from the string, its DACL read out of
        // it and handed on while it lives, and freed once.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(PCWSTR(text.as_ptr()), SDDL_REVISION_1, &mut sd, None)?;
            let (mut present, mut defaulted, mut dacl) = (BOOL::default(), BOOL::default(), std::ptr::null_mut::<ACL>());
            let set = GetSecurityDescriptorDacl(sd, &mut present, &mut dacl, &mut defaulted).map_err(io::Error::from).and_then(|()| {
                let info = DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION;
                SetNamedSecurityInfoW(PCWSTR(name.as_ptr()), SE_FILE_OBJECT, info, None, None, Some(dacl.cast_const()), None)
                    .ok()
                    .map_err(io::Error::from)
            });
            let _ = LocalFree(Some(HLOCAL(sd.0)));
            set
        }
    }

    /// `path`'s DACL as SDDL, to check what `claim` left.
    #[cfg(test)]
    pub fn dacl_sddl(path: &Path) -> io::Result<String> {
        use windows::Win32::Security::Authorization::ConvertSecurityDescriptorToStringSecurityDescriptorW;
        let name = wide(path.as_os_str());
        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SAFETY: as in `owner_sid`; the string is freed once it is read.
        unsafe {
            GetNamedSecurityInfoW(PCWSTR(name.as_ptr()), SE_FILE_OBJECT, DACL_SECURITY_INFORMATION, None, None, None, None, &mut sd)
                .ok()?;
            let mut text = PWSTR::null();
            let made =
                ConvertSecurityDescriptorToStringSecurityDescriptorW(sd, SDDL_REVISION_1, DACL_SECURITY_INFORMATION, &mut text, None);
            let _ = LocalFree(Some(HLOCAL(sd.0)));
            made?;
            let out = text.to_string();
            let _ = LocalFree(Some(HLOCAL(text.0.cast())));
            out.map_err(io::Error::other)
        }
    }
}

/// Makes `dir` `0700` if it is not there, and checks that it is a folder
/// (not a link, which a chmod would follow out to wherever it points) owned
/// by `uid` with no access for anyone else: one this user made before is
/// tightened, one someone else made is refused, since its owner could open
/// it again at will.
#[cfg(unix)]
pub fn claim_private(dir: &Path, uid: u32) -> Result<(), String> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(format!("cannot create {}: {e}", dir.display())),
    }
    let meta = std::fs::symlink_metadata(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    if !meta.file_type().is_dir() {
        return Err(format!("{} is not a folder (a link or a file is there)", dir.display()));
    }
    if meta.uid() != uid {
        return Err(format!("{} belongs to another user (uid {})", dir.display(), meta.uid()));
    }
    if meta.mode() & 0o077 != 0 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("cannot make {} private: {e}", dir.display()))?;
    }
    Ok(())
}

/// A new file at `path`, owner-only on Unix (0600) from the moment it exists:
/// no window in which it is readable at the umask's mode. Fails if `path` is
/// there already, so a file planted ahead of it is never written into.
pub fn create_private(path: &Path) -> io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// Makes an open file the owner's alone on Unix (0600), for one written in
/// place rather than made anew. Elsewhere it keeps the ACL it has: the one
/// file written so is the CLI's own, in its account folder (claude/oauth.rs).
pub fn restrict(file: &std::fs::File) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
    }
    #[cfg(not(unix))]
    {
        let _ = file;
        Ok(())
    }
}

/// Writes `bytes` to `path` through `create_private` and a rename in the
/// same folder: a reader sees the old file or the new one whole, and the new
/// one is never readable by anyone else on Unix. The temporary file's name is
/// random, so nobody can have one ready for it.
pub fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    write_through(path, &path.with_file_name(format!("{name}.{}.tmp", random_hex(8))), bytes)
}

/// `write_private` through `tmp`. What is left of a failed write is removed
/// only once this call has made `tmp`: a file someone else has there is not
/// ours to delete.
fn write_through(path: &Path, tmp: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let mut file = create_private(tmp)?;
    let wrote = file.write_all(bytes);
    // Closed before the rename, which Windows refuses for a file held open.
    drop(file);
    let written = wrote.and_then(|()| std::fs::rename(tmp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(tmp);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("luna-secret-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn hmac_is_rfc_4231s() {
        // Test cases 1, 2 and 6 (a key longer than a block).
        assert_eq!(hmac_hex(&[0x0b; 20], b"Hi There"), "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7");
        assert_eq!(hmac_hex(b"Jefe", b"what do ya want for nothing?"), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
        assert_eq!(
            hmac_hex(&[0xaa; 131], b"Test Using Larger Than Block-Size Key - Hash Key First"),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn a_private_file_replaces_the_old_whole_and_leaves_no_temp_behind() {
        let dir = scratch("write");
        private_dir(&dir.join("sub")).unwrap();
        let path = dir.join("sub").join("core.json");
        write_private(&path, b"one").unwrap();
        write_private(&path, b"two").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        let names: Vec<_> = std::fs::read_dir(dir.join("sub")).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(names, [std::ffi::OsString::from("core.json")]);
        assert!(create_private(&path).is_err(), "never into a file already there");
        // A temp name someone else holds: refused, and their file left alone.
        let theirs = dir.join("sub").join("core.json.planted.tmp");
        std::fs::write(&theirs, "theirs").unwrap();
        assert!(write_through(&path, &theirs, b"three").is_err());
        assert_eq!(std::fs::read(&theirs).unwrap(), b"theirs");
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_secret_folder_is_this_users_alone_or_refused() {
        use std::os::unix::fs::PermissionsExt;
        let base = scratch("claim");
        std::fs::create_dir_all(&base).unwrap();
        // SAFETY: geteuid has no preconditions.
        let uid = unsafe { libc::geteuid() };
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;

        let fresh = base.join("fresh");
        claim_private(&fresh, uid).unwrap();
        assert_eq!(mode(&fresh), 0o700);
        let open = base.join("open");
        std::fs::create_dir(&open).unwrap();
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o777)).unwrap();
        claim_private(&open, uid).unwrap();
        assert_eq!(mode(&open), 0o700, "one of this user's own is tightened");

        // Another user's, as a folder made first by someone else reads.
        assert!(claim_private(&fresh, uid.wrapping_add(1)).unwrap_err().contains("another user"));
        // A link to a folder: refused, and what it points at keeps its mode.
        let target = base.join("target");
        std::fs::create_dir(&target).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
        let link = base.join("hooks");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(private_dir(&link).unwrap_err().to_string().contains("not a folder"));
        assert_eq!(mode(&target), 0o755);
        let file = base.join("file");
        std::fs::write(&file, "").unwrap();
        assert!(claim_private(&file, uid).is_err());
        // Root in the container: a folder really owned by somebody else.
        if uid == 0 {
            let foreign = base.join("foreign");
            std::fs::create_dir(&foreign).unwrap();
            std::os::unix::fs::chown(&foreign, Some(65534), None).unwrap();
            assert!(private_dir(&foreign).unwrap_err().to_string().contains("another user"));
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn secrets_are_the_owners_alone_whatever_the_umask_or_the_old_mode() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let dir = scratch("modes");
        let sub = dir.join("hooks");
        // As an older Luna left it: open to everyone.
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o755)).unwrap();
        private_dir(&sub).unwrap();
        assert_eq!(mode(&sub), 0o700);
        private_dir(&dir.join("mcp").join("deep")).unwrap();
        assert_eq!(mode(&dir.join("mcp").join("deep")), 0o700);
        // umask 0 would give 0666 to a plain write.
        // SAFETY: umask only swaps the process's file-mode mask; nothing here
        // depends on it but the files this test makes.
        let old = unsafe { libc::umask(0) };
        let file = sub.join("c.json");
        let written = write_private(&file, b"secret");
        unsafe { libc::umask(old) };
        written.unwrap();
        assert_eq!(mode(&file), 0o600);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// In the test executables' own folder, under `test-build/`: on this
    /// machine it inherits Authenticated Users' Modify, as an install folder
    /// on a second drive does. icacls's view is printed for the record.
    #[cfg(windows)]
    #[test]
    fn a_secret_folder_on_windows_is_the_users_systems_and_administrators_alone() {
        let icacls = |p: &Path, args: &[&str]| {
            let out = std::process::Command::new("icacls").arg(p).args(args).output().unwrap();
            assert!(out.status.success(), "icacls {args:?}: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        let base = crate::paths::data_dir().join(format!("acl-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let dir = base.join("core");
        std::fs::create_dir_all(&dir).unwrap();
        // Granted outright too, whatever the folder above gives.
        icacls(&dir, &["/grant", "*S-1-5-11:(OI)(CI)M"]);
        std::fs::write(dir.join("core.json"), "token").unwrap();
        println!("before:\n{}", icacls(&dir, &[]));
        private_dir(&dir).unwrap();
        println!("after:\n{}", icacls(&dir, &[]));
        let user = acl::user_sid().unwrap();
        let broad = [";AU)", ";S-1-5-11)", ";BU)", ";WD)", ";IU)"];
        for p in [dir.clone(), dir.join("core.json")] {
            let sddl = acl::dacl_sddl(&p).unwrap();
            assert!(!broad.iter().any(|b| sddl.contains(b)), "{}: {sddl}", p.display());
            assert!(sddl.contains(&user) && sddl.contains(";SY)") && sddl.contains(";BA)"), "{sddl}");
        }
        assert!(acl::dacl_sddl(&dir).unwrap().starts_with("D:P"), "inherits nothing from above");
        private_dir(&dir).unwrap();
        assert!(dir.join("core.json").is_file(), "claimed again, nothing lost");

        // A junction where the folder should be: refused, its target untouched.
        let target = base.join("target");
        std::fs::create_dir(&target).unwrap();
        let before = acl::dacl_sddl(&target).unwrap();
        let link = base.join("hooks");
        let made = std::process::Command::new("cmd").args(["/d", "/c", "mklink", "/J"]).arg(&link).arg(&target).output().unwrap();
        assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stderr));
        assert!(private_dir(&link).unwrap_err().to_string().contains("not a folder"));
        assert_eq!(acl::dacl_sddl(&target).unwrap(), before);

        // As the install folder gets it: everyone on the machine may read and run.
        acl::claim(&base, true).unwrap();
        let sddl = acl::dacl_sddl(&base).unwrap();
        assert!(sddl.contains(";;;BU)") && !sddl.contains("FA;;;BU)") && !sddl.contains(";AU)"), "{sddl}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn random_hex_is_long_lowercase_hex_and_never_repeats() {
        let all: Vec<String> = (0..1000).map(|_| random_hex(16)).collect();
        for s in &all {
            assert_eq!(s.len(), 32);
            assert!(s.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')), "{s}");
        }
        let unique: std::collections::HashSet<&String> = all.iter().collect();
        assert_eq!(unique.len(), all.len());
        assert_eq!(random_hex(20).len(), 40);
        assert_eq!(random_hex(32).len(), 64);
    }

    #[test]
    fn same_tells_equal_from_unequal() {
        assert!(same("abc123", "abc123"));
        assert!(same("", ""));
        assert!(!same("abc124", "abc123"));
        assert!(!same("bbc123", "abc123"));
        assert!(!same("abc12", "abc123"));
        assert!(!same("abc1234", "abc123"));
        assert!(!same("", "abc123"));
    }
}
