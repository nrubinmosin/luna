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
/// without `key`. By hand over `sha2`, already a dependency, rather than a
/// crate for these few lines.
pub fn hmac_hex(key: &[u8], msg: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    const BLOCK: usize = 64;
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let pad = |byte: u8| k.map(|b| b ^ byte);
    let inner = Sha256::new().chain_update(pad(0x36)).chain_update(msg).finalize();
    let outer = Sha256::new().chain_update(pad(0x5c)).chain_update(inner).finalize();
    outer.iter().map(|b| format!("{b:02x}")).collect()
}

/// Makes `dir` (and its parents) for files that hold a secret, and leaves it
/// readable by its owner alone on Unix (0700), whatever mode an older Luna
/// made it with: with umask 022 and a traversable data dir, another OS user
/// could otherwise list it and read what is in it. Elsewhere only made: on
/// Windows the data dir is in the user's profile (`LOCALAPPDATA`), whose ACL
/// grants the user, SYSTEM and Administrators alone and is inherited, or
/// beside the exe where the user chose to install, which inherits that
/// folder's ACL (threat-model.md, F4).
pub fn private_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        // `mode` is for what the call creates; one there already is tightened.
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(dir)
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
/// place rather than made anew. Nothing to do elsewhere: see `private_dir`.
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
    use std::io::Write;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = path.with_file_name(format!("{name}.{}.tmp", random_hex(8)));
    let written = create_private(&tmp).and_then(|mut f| f.write_all(bytes)).and_then(|()| std::fs::rename(&tmp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
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
        let _ = std::fs::remove_dir_all(&dir);
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
