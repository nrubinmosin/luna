//! What the volume a file or folder is on can be trusted with, as Windows
//! tells it through a handle on that file: whether it keeps ACLs (a private
//! file is private only where it does) and whether a file's id names it for
//! as long as it exists (the single instance's key, single.rs).

/// `FILE_PERSISTENT_ACLS`: the volume keeps the ACLs set on its files.
const PERSISTENT_ACLS: u32 = 0x0000_0008;
/// `FILE_SUPPORTS_OPEN_BY_FILE_ID`: a file can be opened by its id, which
/// the file system therefore keeps for the file's life.
const OPEN_BY_FILE_ID: u32 = 0x0100_0000;

/// A volume, as `GetVolumeInformationByHandleW` and the path to a file on it
/// describe it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Volume {
    /// The file system's name: NTFS, ReFS, FAT32, exFAT and so on.
    pub file_system: String,
    /// Its `FILE_*` flags.
    pub flags: u32,
    /// Reached over the network: its path is a UNC one.
    pub remote: bool,
}

impl Volume {
    /// Whether the volume keeps ACLs: FAT and exFAT keep none, so a file
    /// made private there is readable by everyone who can reach the volume.
    pub fn persistent_acls(&self) -> bool {
        self.flags & PERSISTENT_ACLS != 0
    }

    /// Whether a file's id stays its own for as long as the file does: a
    /// local volume that opens files by id. Not FAT or exFAT, whose ids are
    /// where a directory entry sits and change as entries move, whatever the
    /// flags say; not a share, whose server may make ids up, give part of
    /// one, or fail to give any.
    pub fn stable_ids(&self) -> bool {
        let fat = matches!(self.file_system.to_ascii_lowercase().as_str(), "fat" | "fat12" | "fat16" | "fat32" | "vfat" | "exfat");
        !self.remote && !fat && self.flags & OPEN_BY_FILE_ID != 0
    }
}

/// The path Windows resolves the handle `file` to, normalized, in its `\\?\`
/// form (`\\?\C:\…`, `\\?\UNC\server\share\…`): every link and junction on
/// the way followed as it was when the file was opened, each name spelled as
/// the volume keeps it.
#[cfg(windows)]
pub fn final_path(file: &std::fs::File) -> std::io::Result<String> {
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
    String::from_utf16(&buf).map_err(|_| std::io::Error::other("the path is not Unicode"))
}

/// The volume the file open on `file` is on; `path` is where it resolves
/// (`final_path`), which tells a share.
#[cfg(windows)]
pub fn of(file: &std::fs::File, path: &str) -> std::io::Result<Volume> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::GetVolumeInformationByHandleW;
    let mut flags = 0u32;
    let mut name = [0u16; 64];
    // SAFETY: a handle open for as long as `file`; the flags are written
    // once, the name into the buffer of the length passed.
    unsafe { GetVolumeInformationByHandleW(HANDLE(file.as_raw_handle()), None, None, None, Some(&mut flags), Some(&mut name)) }?;
    let len = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    Ok(Volume { file_system: String::from_utf16_lossy(&name[..len]), flags, remote: is_remote(path) })
}

/// Whether `path`, as `final_path` gives it, is on a share.
pub fn is_remote(path: &str) -> bool {
    let upper = path.get(..8).map(str::to_ascii_uppercase);
    upper.as_deref() == Some(r"\\?\UNC\") || (path.starts_with(r"\\") && !path.starts_with(r"\\?\"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn volume(file_system: &str, flags: u32, remote: bool) -> Volume {
        Volume { file_system: file_system.into(), flags, remote }
    }

    /// NTFS, as a Windows 11 drive gives its flags.
    const NTFS: u32 = 0x03E7_2EFF;
    /// FAT32 and exFAT: case kept and Unicode names, and nothing like ACLs
    /// or ids.
    const FAT: u32 = 0x0000_0006;

    #[test]
    fn only_a_local_volume_that_opens_by_id_has_ids_for_good() {
        assert!(volume("NTFS", NTFS, false).stable_ids());
        assert!(volume("ReFS", OPEN_BY_FILE_ID | PERSISTENT_ACLS, false).stable_ids());
        assert!(!volume("NTFS", NTFS, true).stable_ids(), "a share");
        assert!(!volume("NTFS", NTFS & !OPEN_BY_FILE_ID, false).stable_ids());
        for fat in ["FAT", "FAT32", "exFAT", "vfat"] {
            assert!(!volume(fat, FAT, false).stable_ids(), "{fat}");
            assert!(!volume(fat, FAT | OPEN_BY_FILE_ID, false).stable_ids(), "{fat} whatever its flags say");
        }
    }

    #[test]
    fn fat_keeps_no_acls_and_ntfs_does() {
        assert!(volume("NTFS", NTFS, false).persistent_acls());
        assert!(!volume("exFAT", FAT, false).persistent_acls());
        assert!(!volume("FAT32", FAT, false).persistent_acls());
    }

    #[test]
    fn a_unc_path_is_a_share_and_a_drive_is_not() {
        assert!(is_remote(r"\\?\UNC\server\share\Luna"));
        assert!(is_remote(r"\\?\unc\server\share\Luna"));
        assert!(is_remote(r"\\server\share\Luna"));
        assert!(!is_remote(r"\\?\C:\Luna"));
        assert!(!is_remote(r"C:\Luna"));
        assert!(!is_remote(""));
    }
}
