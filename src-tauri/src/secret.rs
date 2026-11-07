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
        acl::claim(dir)
    }
    #[cfg(not(any(unix, windows)))]
    std::fs::create_dir_all(dir)
}

/// The DACL, as SDDL, of a folder that is this user's, SYSTEM's and
/// Administrators' alone: full control for each, by SID, inherited by what
/// is in it, and nothing from above (`P`). No BUILTIN\Users: the data dir
/// holds what the sessions were told and answered (agents.jsonl), the log,
/// settings, attachments and the WebView's profile, and is the install
/// folder too where the exe's own is writable.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn private_sddl(user: &str) -> String {
    format!("D:P(A;OICI;FA;;;{user})(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)")
}

/// SYSTEM and Administrators as Windows writes them back, by alias or SID.
#[cfg_attr(not(windows), allow(dead_code))]
const MACHINE_TRUSTEES: [&str; 4] = ["SY", "S-1-5-18", "BA", "S-1-5-32-544"];

/// The ACEs of a DACL in SDDL (`D:<flags>(ace)(ace)…`), each split into its
/// fields (type; flags; rights; object; inherited object; trustee), and the
/// DACL's own flags. None for what is no DACL.
#[cfg_attr(not(windows), allow(dead_code))]
fn sddl_aces(sddl: &str) -> Option<(&str, Vec<Vec<&str>>)> {
    let rest = sddl.strip_prefix("D:")?;
    let at = rest.find('(').unwrap_or(rest.len());
    let (flags, aces) = rest.split_at(at);
    let aces =
        aces.strip_prefix('(').and_then(|a| a.strip_suffix(')')).map(|a| a.split(")(").map(|ace| ace.split(';').collect()).collect());
    Some((flags, aces.unwrap_or_default()))
}

/// Whether `sddl` is protected and lets nobody in but `user`, SYSTEM and
/// Administrators: any allow of another kind or for anyone else, and it is not.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn sddl_is_private(sddl: &str, user: &str) -> bool {
    let Some((flags, aces)) = sddl_aces(sddl) else { return false };
    flags.contains('P')
        && !aces.is_empty()
        && aces.iter().all(|ace| match ace.as_slice() {
            ["D", ..] => true,
            ["A", _, _, _, _, who] => *who == user || MACHINE_TRUSTEES.contains(who),
            _ => false,
        })
}

/// Whether `sddl` takes everything from its parent: not protected, and every
/// ACE inherited (`ID`). What is not gets reset by `acl::protect_tree`, since
/// an explicit ACE outlives any change to the folder above it.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn sddl_inherits_only(sddl: &str) -> bool {
    sddl_aces(sddl).is_some_and(|(flags, aces)| !flags.contains('P') && aces.iter().all(|ace| ace.get(1).is_some_and(|f| f.contains("ID"))))
}

/// Windows' side of `private_dir`, and of the data dir's protection
/// (paths.rs `protect_data_dir`).
#[cfg(windows)]
pub mod acl {
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use windows::core::{BOOL, PCWSTR, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL};
    use windows::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        ConvertStringSidToSidW, GetNamedSecurityInfoW, SetNamedSecurityInfoW, SDDL_REVISION_1, SE_FILE_OBJECT,
    };
    use windows::Win32::Security::{
        GetSecurityDescriptorDacl, GetTokenInformation, TokenUser, ACL, DACL_SECURITY_INFORMATION, OBJECT_SECURITY_INFORMATION,
        OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, TOKEN_QUERY, TOKEN_USER,
        UNPROTECTED_DACL_SECURITY_INFORMATION,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    /// Who may own what is in a private folder: this user, Administrators (an
    /// elevated run's), SYSTEM and TrustedInstaller. Anyone else who owns a
    /// file may rewrite its DACL at will, whatever the folder says.
    fn owner_ok(owner: &str, user: &str) -> bool {
        const TRUSTED_INSTALLER: &str = "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464";
        owner == user || ["S-1-5-32-544", "S-1-5-18", TRUSTED_INSTALLER].contains(&owner)
    }

    /// Gives `dir` the protected DACL of `super::private_sddl`, after
    /// `check_root`. For a token folder (`private_dir`), whose files Luna
    /// writes itself; a tree that may hold anything is `protect_tree`'s.
    pub fn claim(dir: &Path) -> io::Result<()> {
        let user = check_root(dir, true)?;
        set_dacl(dir, &super::private_sddl(&user), PROTECTED_DACL_SECURITY_INFORMATION)
    }

    /// What every root is refused for: a link or junction, which a DACL would
    /// be written through to wherever it points; something other than a
    /// folder where one is `want_dir`; and an owner `owner_ok` does not name,
    /// who could rewrite any DACL on it at will. This user's SID.
    fn check_root(path: &Path, want_dir: bool) -> io::Result<String> {
        let meta = std::fs::symlink_metadata(path)?;
        if meta.file_type().is_symlink() || (want_dir && !meta.is_dir()) {
            return Err(io::Error::other(format!("{} is not a folder (a link or a file is there)", path.display())));
        }
        let user = user_sid()?;
        let owner = owner_sid(path)?;
        if !owner_ok(&owner, &user) {
            return Err(io::Error::other(format!("{} belongs to someone else ({owner})", path.display())));
        }
        Ok(user)
    }

    /// Whether `path` belongs to someone `owner_ok` names.
    pub fn owner_trusted(path: &Path) -> io::Result<bool> {
        Ok(owner_ok(&owner_sid(path)?, &user_sid()?))
    }

    /// Whether `path`'s own DACL is `super::sddl_is_private`: the root alone,
    /// a quick look for the tests; `verify` is the whole of it.
    #[cfg(test)]
    pub fn is_private(path: &Path) -> io::Result<bool> {
        Ok(super::sddl_is_private(&dacl_sddl(path)?, &trustee(&user_sid()?)?))
    }

    /// Makes `root` (a folder or a file) and everything in it private:
    /// everything below reset to inherit, since an explicit ACE (or a
    /// protected DACL) below outlives any change to the root; a file owned by
    /// someone `owner_ok` does not name taken over, since its owner could open
    /// it again; links and junctions left alone and not gone into. The root
    /// gets its protected DACL last, so a pass that fails halfway leaves it
    /// as it was and is gone through again; `verify` says whether one held.
    /// What was taken over or left alone is returned, for the log and
    /// Settings' checks: a file taken over may have been changed by its owner.
    pub fn protect_tree(root: &Path) -> io::Result<Vec<String>> {
        let user = check_root(root, false)?;
        let mut odd = Vec::new();
        if std::fs::symlink_metadata(root)?.is_dir() {
            reset_below(root, &user, &mut odd)?;
        }
        set_dacl(root, &super::private_sddl(&user), PROTECTED_DACL_SECURITY_INFORMATION)?;
        Ok(odd)
    }

    /// What keeps `root` from being private down to the last file: none when
    /// it is. The root's own DACL must be `super::sddl_is_private`; each
    /// thing below must inherit only, belong to someone `owner_ok` names and
    /// be no link or junction.
    pub fn verify(root: &Path) -> io::Result<Vec<String>> {
        let user = user_sid()?;
        let named = trustee(&user)?;
        let mut problems = Vec::new();
        let meta = std::fs::symlink_metadata(root)?;
        if meta.file_type().is_symlink() {
            problems.push(format!("{}: a link or junction", root.display()));
            return Ok(problems);
        }
        let owner = owner_sid(root)?;
        if !owner_ok(&owner, &user) {
            problems.push(format!("{}: owned by {owner}", root.display()));
        }
        if !super::sddl_is_private(&dacl_sddl(root)?, &named) {
            problems.push(format!("{}: open to others", root.display()));
        }
        if meta.is_dir() {
            verify_below(root, &user, &named, &mut problems)?;
        }
        Ok(problems)
    }

    /// `user` is this user's SID, for owners; `named` is the same user as a
    /// DACL's SDDL names them (`trustee`).
    fn verify_below(dir: &Path, user: &str, named: &str, problems: &mut Vec<String>) -> io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            match verify_one(&path, user, named, problems) {
                // Gone meanwhile: a running WebView comes and goes with files.
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                done => done.map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))?,
            }
        }
        Ok(())
    }

    fn verify_one(path: &Path, user: &str, named: &str, problems: &mut Vec<String>) -> io::Result<()> {
        let meta = std::fs::symlink_metadata(path)?;
        if meta.file_type().is_symlink() {
            problems.push(format!("{}: a link or junction", path.display()));
            return Ok(());
        }
        let owner = owner_sid(path)?;
        if !owner_ok(&owner, user) {
            problems.push(format!("{}: owned by {owner}", path.display()));
        }
        // A token folder keeps the private DACL of its own (`private_dir`).
        let sddl = dacl_sddl(path)?;
        if !super::sddl_inherits_only(&sddl) && !super::sddl_is_private(&sddl, named) {
            problems.push(format!("{}: has access entries of its own", path.display()));
        }
        if meta.is_dir() {
            verify_below(path, user, named, problems)?;
        }
        Ok(())
    }

    fn reset_below(dir: &Path, user: &str, odd: &mut Vec<String>) -> io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            match reset(&path, user, odd) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                done => done.map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))?,
            }
        }
        Ok(())
    }

    fn reset(path: &Path, user: &str, odd: &mut Vec<String>) -> io::Result<()> {
        let meta = std::fs::symlink_metadata(path)?;
        if meta.file_type().is_symlink() {
            odd.push(format!("{}: a link or junction, left alone", path.display()));
            return Ok(());
        }
        let owner = owner_sid(path)?;
        if !owner_ok(&owner, user) {
            set_owner(path, user)?;
            odd.push(format!("{}: was owned by {owner}, taken over", path.display()));
        }
        if !super::sddl_inherits_only(&dacl_sddl(path)?) {
            // An empty DACL, unprotected: what is left is what it inherits.
            set_dacl(path, "D:", UNPROTECTED_DACL_SECURITY_INFORMATION)?;
        }
        if meta.is_dir() {
            reset_below(path, user, odd)?;
        }
        Ok(())
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

    /// `sid` as a DACL's SDDL names it. Windows writes a SID it has an alias
    /// for by the alias: the machine's built-in Administrator account (RID
    /// 500), which an admin may well run as and GitHub's Windows runners do,
    /// reads back as `LA`, not as the SID `user_sid` gives. A DACL read back
    /// is matched against this, or a private one would never look private.
    pub fn trustee(sid: &str) -> io::Result<String> {
        let written = as_written(&format!("D:(A;;FA;;;{sid})"))?;
        super::sddl_aces(&written)
            .and_then(|(_, aces)| aces.first().and_then(|ace| ace.get(5)).map(|who| who.to_string()))
            .ok_or_else(|| io::Error::other(format!("no trustee in {written}")))
    }

    /// `sddl` (a DACL) as Windows writes it back once it is set.
    pub(super) fn as_written(sddl: &str) -> io::Result<String> {
        let text: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SAFETY: the descriptor is made from the string, read and freed once.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(PCWSTR(text.as_ptr()), SDDL_REVISION_1, &mut sd, None)?;
            let out = sd_text(sd);
            let _ = LocalFree(Some(HLOCAL(sd.0)));
            out
        }
    }

    /// The DACL of `sd` as SDDL, for a descriptor the caller keeps alive.
    fn sd_text(sd: PSECURITY_DESCRIPTOR) -> io::Result<String> {
        let mut text = PWSTR::null();
        // SAFETY: `sd` is valid for the call; the string it makes is
        // LocalAlloc'd, read here and freed once.
        unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(sd, SDDL_REVISION_1, DACL_SECURITY_INFORMATION, &mut text, None)?;
            let out = text.to_string();
            let _ = LocalFree(Some(HLOCAL(text.0.cast())));
            out.map_err(io::Error::other)
        }
    }

    /// The SID, as text, that an SDDL alias (`LA`, `BA`…) stands for here.
    #[cfg(test)]
    pub fn sid_of(alias: &str) -> io::Result<String> {
        let text: Vec<u16> = alias.encode_utf16().chain(Some(0)).collect();
        let mut psid = PSID::default();
        // SAFETY: the SID is made from the string, read and freed once.
        unsafe {
            ConvertStringSidToSidW(PCWSTR(text.as_ptr()), &mut psid)?;
            let out = sid_text(psid);
            let _ = LocalFree(Some(HLOCAL(psid.0)));
            out
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

    /// Makes `sid` (text) the owner of `path`: this user may, holding full
    /// control of it through the folder.
    fn set_owner(path: &Path, sid: &str) -> io::Result<()> {
        let name = wide(path.as_os_str());
        let text: Vec<u16> = sid.encode_utf16().chain(Some(0)).collect();
        let mut psid = PSID::default();
        // SAFETY: the SID is made from the string, handed on while it lives,
        // and freed once.
        unsafe {
            ConvertStringSidToSidW(PCWSTR(text.as_ptr()), &mut psid)?;
            let set =
                SetNamedSecurityInfoW(PCWSTR(name.as_ptr()), SE_FILE_OBJECT, OWNER_SECURITY_INFORMATION, Some(psid), None, None, None)
                    .ok()
                    .map_err(io::Error::from);
            let _ = LocalFree(Some(HLOCAL(psid.0)));
            set
        }
    }

    /// Writes `sddl`'s DACL on `path`, `inherit` saying whether it is
    /// protected from what is above it or takes that in as well.
    fn set_dacl(path: &Path, sddl: &str, inherit: OBJECT_SECURITY_INFORMATION) -> io::Result<()> {
        let name = wide(path.as_os_str());
        let text: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SAFETY: the descriptor is made from the string, its DACL read out of
        // it and handed on while it lives, and freed once.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(PCWSTR(text.as_ptr()), SDDL_REVISION_1, &mut sd, None)?;
            let (mut present, mut defaulted, mut dacl) = (BOOL::default(), BOOL::default(), std::ptr::null_mut::<ACL>());
            let set = GetSecurityDescriptorDacl(sd, &mut present, &mut dacl, &mut defaulted).map_err(io::Error::from).and_then(|()| {
                SetNamedSecurityInfoW(
                    PCWSTR(name.as_ptr()),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | inherit,
                    None,
                    None,
                    Some(dacl.cast_const()),
                    None,
                )
                .ok()
                .map_err(io::Error::from)
            });
            let _ = LocalFree(Some(HLOCAL(sd.0)));
            set
        }
    }

    /// A new file at `path` (`CREATE_NEW`), open for writing, made with a
    /// protected DACL for this user, SYSTEM and Administrators: set as it is
    /// created, never a moment open to what the folder hands down. A rename
    /// in the same volume keeps it.
    pub fn create_new(path: &Path) -> io::Result<std::fs::File> {
        use std::os::windows::io::{FromRawHandle, RawHandle};
        use windows::Win32::Foundation::GENERIC_WRITE;
        use windows::Win32::Security::SECURITY_ATTRIBUTES;
        use windows::Win32::Storage::FileSystem::{
            CreateFileW, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        let user = user_sid()?;
        // A file has nothing to hand down: the folder's ACEs without OICI.
        let sddl: Vec<u16> = format!("D:P(A;;FA;;;{user})(A;;FA;;;SY)(A;;FA;;;BA)").encode_utf16().chain(Some(0)).collect();
        let name = wide(path.as_os_str());
        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SAFETY: the descriptor is made from the string and lives through
        // the call that reads it, then is freed once; the handle CreateFileW
        // returns is handed to the File, which closes it.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(PCWSTR(sddl.as_ptr()), SDDL_REVISION_1, &mut sd, None)?;
            let attrs = SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: sd.0,
                bInheritHandle: BOOL(0),
            };
            // The share modes std opens with, so others' reads go on as before.
            let made = CreateFileW(
                PCWSTR(name.as_ptr()),
                GENERIC_WRITE.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                Some(&attrs),
                CREATE_NEW,
                FILE_ATTRIBUTE_NORMAL,
                None,
            );
            let _ = LocalFree(Some(HLOCAL(sd.0)));
            // As the Win32 error it wraps, so its kind is told (AlreadyExists,
            // NotFound) as std's own open tells it; `open_private` goes by it.
            let made = made.map_err(|e| match e.code().0 as u32 {
                h if h & 0xFFFF_0000 == 0x8007_0000 => io::Error::from_raw_os_error((h & 0xFFFF) as i32),
                _ => io::Error::from(e),
            })?;
            Ok(std::fs::File::from_raw_handle(made.0 as RawHandle))
        }
    }

    /// `path`'s DACL as SDDL.
    pub fn dacl_sddl(path: &Path) -> io::Result<String> {
        let name = wide(path.as_os_str());
        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SAFETY: as in `owner_sid`; the string is freed once it is read.
        unsafe {
            GetNamedSecurityInfoW(PCWSTR(name.as_ptr()), SE_FILE_OBJECT, DACL_SECURITY_INFORMATION, None, None, None, None, &mut sd)
                .ok()?;
            let out = sd_text(sd);
            let _ = LocalFree(Some(HLOCAL(sd.0)));
            out
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

/// A new file at `path`, private from the moment it exists: owner-only on
/// Unix (0600), whatever the umask; on Windows made with a protected DACL of
/// this user's, SYSTEM's and Administrators' (`acl::create_new`), whatever
/// the folder would have it inherit. A data dir that is not Luna's alone has
/// only Luna's own things made private, and only those there as it starts
/// (paths.rs `scope`): a file made later in the open folder would take its
/// ACL. Fails if `path` is there already, so a file planted ahead of it is
/// never written into.
pub fn create_private(path: &Path) -> io::Result<std::fs::File> {
    #[cfg(windows)]
    {
        acl::create_new(path)
    }
    #[cfg(not(windows))]
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(path)
    }
}

/// `path` opened with `options`, made first through `create_private` where
/// it is not there: for a file appended to or held as a lock, which is
/// written in place rather than replaced. One there already keeps what it
/// has; the data dir's pass at start sees to Luna's own (paths.rs).
/// `options` must not create: what is not there is made here.
pub fn open_private(path: &Path, options: &std::fs::OpenOptions) -> io::Result<std::fs::File> {
    match options.open(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        opened => return opened,
    }
    match create_private(path) {
        Ok(_) => {}
        // Another process (or thread) made it in between: as good.
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
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
/// one is never readable by anyone else. Luna's state files in its data dir
/// are written so, or through `open_private`. The temporary file's name is
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
        // Never into a file already there, and said so (`open_private` goes by it).
        assert_eq!(create_private(&path).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        // A temp name someone else holds: refused, and their file left alone.
        let theirs = dir.join("sub").join("core.json.planted.tmp");
        std::fs::write(&theirs, "theirs").unwrap();
        assert!(write_through(&path, &theirs, b"three").is_err());
        assert_eq!(std::fs::read(&theirs).unwrap(), b"theirs");
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A data dir that is not Luna's alone stays open to others, only what of
    /// Luna's is there as it starts made private (paths.rs `scope`): what it
    /// writes there later is private all the same, replaced or appended to.
    #[test]
    fn a_file_made_later_in_an_open_data_dir_is_private_all_the_same() {
        use std::io::Write;
        #[cfg(windows)]
        let dir = crate::paths::data_dir().join(format!("open-test-{}", std::process::id()));
        #[cfg(not(windows))]
        let dir = scratch("open");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        #[cfg(windows)]
        {
            // As a shared tools folder on a second drive has it.
            let out = std::process::Command::new("icacls").arg(&dir).args(["/grant", "*S-1-5-11:(OI)(CI)M"]).output().unwrap();
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
            let plain = dir.join("notes.txt");
            std::fs::write(&plain, "theirs").unwrap();
            assert!(!acl::is_private(&plain).unwrap(), "the folder is open to what is made plainly");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
        }
        let helpers = dir.join("helpers.json");
        write_private(&helpers, b"[]").unwrap();
        write_private(&helpers, b"[{}]").unwrap();
        let log = dir.join("agents.jsonl");
        for line in ["one", "two"] {
            writeln!(open_private(&log, std::fs::OpenOptions::new().append(true)).unwrap(), "{line}").unwrap();
        }
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "one\ntwo\n");
        let lock = dir.join("settings.json.lock");
        open_private(&lock, std::fs::OpenOptions::new().write(true)).unwrap().lock().unwrap();
        for p in [&helpers, &log, &lock] {
            #[cfg(windows)]
            {
                assert!(acl::is_private(p).unwrap(), "{}: {}", p.display(), acl::dacl_sddl(p).unwrap());
                assert_eq!(acl::verify(p).unwrap(), Vec::<String>::new());
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(std::fs::metadata(p).unwrap().permissions().mode() & 0o777, 0o600, "{}", p.display());
            }
        }
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
        let user = acl::trustee(&acl::user_sid().unwrap()).unwrap();
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

        let _ = std::fs::remove_dir_all(&base);
    }

    /// A data dir as an install on a second drive leaves it, under
    /// `test-build/` (which inherits Authenticated Users' Modify here): an
    /// explicit grant on a file, a protected folder of its own, a junction out.
    #[cfg(windows)]
    #[test]
    fn the_data_dir_is_made_private_down_to_what_is_in_it() {
        let icacls = |p: &Path, args: &[&str]| {
            let out = std::process::Command::new("icacls").arg(p).args(args).output().unwrap();
            assert!(out.status.success(), "icacls {args:?}: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        let root = crate::paths::data_dir().join(format!("tree-test-{}", std::process::id()));
        let outside = crate::paths::data_dir().join(format!("tree-out-{}", std::process::id()));
        for d in [&root, &outside] {
            let _ = std::fs::remove_dir_all(d);
        }
        std::fs::create_dir_all(root.join("webview").join("Default")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(root.join("luna.exe"), "exe").unwrap();
        std::fs::write(root.join("agents.jsonl"), "{}").unwrap();
        std::fs::write(root.join("webview").join("Default").join("Local State"), "{}").unwrap();
        icacls(&root.join("luna.exe"), &["/grant", "*S-1-5-11:M"]);
        icacls(&root.join("webview"), &["/inheritance:d"]);
        let link = root.join("elsewhere");
        let made = std::process::Command::new("cmd").args(["/d", "/c", "mklink", "/J"]).arg(&link).arg(&outside).output().unwrap();
        assert!(made.status.success());
        let outside_before = acl::dacl_sddl(&outside).unwrap();
        println!("before, root:\n{}before, luna.exe:\n{}", icacls(&root, &[]), icacls(&root.join("luna.exe"), &[]));
        assert!(!acl::is_private(&root).unwrap());

        let odd = acl::protect_tree(&root).unwrap();
        println!("after, root:\n{}after, luna.exe:\n{}", icacls(&root, &[]), icacls(&root.join("luna.exe"), &[]));
        assert!(acl::is_private(&root).unwrap());
        assert_eq!(odd.len(), 1, "{odd:?}");
        assert!(odd[0].contains("elsewhere") && odd[0].contains("link"), "{odd:?}");
        for p in [
            root.join("luna.exe"),
            root.join("agents.jsonl"),
            root.join("webview"),
            root.join("webview").join("Default").join("Local State"),
        ] {
            let sddl = acl::dacl_sddl(&p).unwrap();
            assert!(sddl_inherits_only(&sddl) && !sddl.contains(";AU)") && !sddl.contains(";BU)"), "{}: {sddl}", p.display());
        }
        assert_eq!(acl::dacl_sddl(&outside).unwrap(), outside_before, "nothing done through the junction");
        // The junction is what is still in the way, and nothing else.
        let open = acl::verify(&root).unwrap();
        assert!(open.len() == 1 && open[0].contains("elsewhere"), "{open:?}");
        std::fs::remove_dir(&link).unwrap();
        assert_eq!(acl::verify(&root).unwrap(), Vec::<String>::new());
        // A token folder's own private DACL is no problem.
        private_dir(&root.join("core")).unwrap();
        assert_eq!(acl::verify(&root).unwrap(), Vec::<String>::new());
        for d in [&root, &outside] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    /// A pass that cannot finish leaves the root as it was, so the next start
    /// goes through it again: here a file whose DACL nobody may rewrite, its
    /// owner included (OWNER RIGHTS denied WRITE_DAC).
    #[cfg(windows)]
    #[test]
    fn a_pass_that_fails_halfway_leaves_the_root_open_to_be_done_again() {
        let icacls = |p: &Path, args: &[&str]| {
            let out = std::process::Command::new("icacls").arg(p).args(args).output().unwrap();
            assert!(out.status.success(), "icacls {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        let root = crate::paths::data_dir().join(format!("half-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let stuck = root.join("stuck.json");
        std::fs::write(&stuck, "{}").unwrap();
        let user = acl::user_sid().unwrap();
        icacls(&stuck, &["/grant", "*S-1-5-11:M", "/deny", "*S-1-3-4:(WDAC)", &format!("*{user}:(WDAC)")]);
        assert!(acl::protect_tree(&root).is_err());
        assert!(!acl::is_private(&root).unwrap(), "the root is done last");
        assert!(!acl::verify(&root).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The built-in Administrator's SID reads back from a DACL as `LA`: what
    /// a DACL says is matched against that, so a folder made private for an
    /// admin running as that account is private (GitHub's runners do).
    #[cfg(windows)]
    #[test]
    fn a_user_windows_names_by_an_alias_is_matched_by_it() {
        let admin = acl::sid_of("LA").unwrap();
        assert!(admin.starts_with("S-1-5-21-") && admin.ends_with("-500"), "{admin}");
        assert_eq!(acl::trustee(&admin).unwrap(), "LA");
        let written = acl::as_written(&private_sddl(&admin)).unwrap();
        assert!(!sddl_is_private(&written, &admin), "by its SID it is not there: {written}");
        assert!(sddl_is_private(&written, &acl::trustee(&admin).unwrap()), "{written}");
        // Anyone else keeps the SID.
        assert_eq!(acl::trustee("S-1-5-21-1-2-3-1001").unwrap(), "S-1-5-21-1-2-3-1001");
    }

    #[test]
    fn a_dacl_is_private_only_when_protected_and_for_the_user_and_the_machine_alone() {
        let user = "S-1-5-21-1-2-3-1001";
        assert!(sddl_is_private(&private_sddl(user), user));
        // As Windows writes it back, inherited flags and all.
        assert!(sddl_is_private("D:PAI(A;OICI;FA;;;S-1-5-21-1-2-3-1001)(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)", user));
        assert!(!sddl_is_private("D:AI(A;OICI;FA;;;S-1-5-21-1-2-3-1001)(A;OICI;FA;;;SY)", user), "not protected");
        assert!(!sddl_is_private("D:P(A;OICI;FA;;;S-1-5-21-1-2-3-1001)(A;OICI;0x1200a9;;;BU)", user), "Users may read");
        assert!(!sddl_is_private("D:P(A;OICI;FA;;;S-1-5-21-1-2-3-1002)", user), "another user");
        assert!(!sddl_is_private("D:P(A;ID;0x1301bf;;;AU)", user));
        assert!(!sddl_is_private("D:P", user), "no one at all");
        assert!(!sddl_is_private("O:SYD:P(A;;FA;;;SY)", user), "not a DACL on its own");
        assert!(sddl_is_private("D:P(D;;FA;;;WD)(A;;FA;;;SY)", user), "a deny takes nothing away from privacy");

        assert!(sddl_inherits_only("D:AI(A;ID;FA;;;SY)(A;OICIID;FA;;;BA)"));
        assert!(!sddl_inherits_only("D:AI(A;;0x1301bf;;;AU)(A;ID;FA;;;SY)"), "an explicit grant");
        assert!(!sddl_inherits_only("D:PAI(A;ID;FA;;;SY)"), "protected");
        assert!(sddl_inherits_only("D:AI"), "nothing but what is inherited, which is nothing");
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
