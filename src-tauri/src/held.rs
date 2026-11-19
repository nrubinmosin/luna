//! Folders held open by handle, and what lies in them reached through that
//! handle rather than by a path: a check made on a folder and the write or
//! removal that follows it then concern the same folder, whatever a link,
//! a junction or a folder put in its place at the same path does meanwhile.
//! The native window's library save and account deletion go through it
//! (library/commands.rs `library_write_checked`, accounts.rs `remove_held`).
//!
//! A folder is named by its identity on its drive (`Dir::id`): the volume's
//! serial number and the 128-bit file id on Windows, the device and inode
//! elsewhere. A folder held on Windows is shared for reading and writing but
//! not deletion, so it is neither renamed nor deleted while it is held.
//!
//! Windows: names are opened and made with `NtCreateFile` relative to the
//! folder's handle (its `RootDirectory`), a reparse point at a name opened as
//! itself (`FILE_OPEN_REPARSE_POINT`) and refused, or removed as itself; files
//! are renamed relative to the folder's handle (`NtSetInformationFile`) and
//! deleted through their own. Unix: `openat` with `O_NOFOLLOW`, `mkdirat`,
//! `renameat` (`renameat2(RENAME_NOREPLACE)` on Linux, `renameatx_np`'s
//! `RENAME_EXCL` on macOS, `linkat` elsewhere, where nothing may be
//! replaced) and `unlinkat` on the descriptors; a removal never crosses into
//! another device (`st_dev`), a mount inside the tree.

use std::fs::File;
use std::io;
use std::path::Path;

/// A folder held open.
pub struct Dir {
    file: File,
}

/// A link, a junction or another reparse point, where a plain folder or file
/// was asked for.
fn not_plain(name: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, format!("{name} is a link, a junction or not a plain entry — Luna leaves it alone"))
}

/// One name, as an entry of a folder: no separator, no `.` or `..`, no
/// stream suffix, no NUL. What every call here is handed, so none of them
/// walks further than the one folder.
fn one_name(name: &str) -> io::Result<()> {
    let bad = name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', '\0']) || (cfg!(windows) && name.contains(':'));
    if bad {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{name:?} is not a plain name")));
    }
    Ok(())
}

impl Dir {
    /// The folder at `path`, a link or junction at it followed: for a folder
    /// the person chose (the library's, the accounts root), which may be a
    /// link of their choosing. What it leads to is held from here on.
    pub fn open_followed(path: &Path) -> io::Result<Dir> {
        sys::open_path(path, true).map(|file| Dir { file })
    }

    /// The folder at `path` itself: a link, a junction or any other reparse
    /// point there is refused, never followed.
    pub fn open_itself(path: &Path) -> io::Result<Dir> {
        let file = sys::open_path(path, false)?;
        sys::plain_dir(&file).map_err(|_| not_plain(&path.to_string_lossy()))?;
        Ok(Dir { file })
    }

    /// The folder's identity on its drive.
    pub fn id(&self) -> io::Result<String> {
        sys::id(&self.file)
    }

    /// Whether `other` lies on the same volume or device as this folder.
    pub fn same_device(&self, other: &Dir) -> io::Result<bool> {
        Ok(sys::device(&self.file)? == sys::device(&other.file)?)
    }

    /// The folder `name` in this one, opened as itself (a link or any other
    /// reparse point there refused), made where `make` and it is not there;
    /// None where it is not there and not made.
    pub fn child_dir(&self, name: &str, make: bool) -> io::Result<Option<Dir>> {
        one_name(name)?;
        let file = match sys::child_dir(&self.file, name, make, false) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound && !make => return Ok(None),
            Err(e) => return Err(e),
        };
        sys::plain_dir(&file).map_err(|_| not_plain(name))?;
        Ok(Some(Dir { file }))
    }

    /// The file `name` in this folder, opened as itself for reading and
    /// held by the handle's own word to a plain file (no link, no reparse
    /// point, no FIFO waited on); None where nothing is there.
    pub fn open_file(&self, name: &str) -> io::Result<Option<File>> {
        one_name(name)?;
        let file = match sys::open_file(&self.file, name) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        if !sys::plain_file(&file)? {
            return Err(not_plain(name));
        }
        Ok(Some(file))
    }

    /// A new file `name` in this folder, made there and nowhere else (one
    /// already at the name, a link included, is an error), open for writing:
    /// this user's alone from the moment it exists where `private`, else of
    /// the folder's access, as anything made there is.
    pub fn create_new(&self, name: &str, private: bool) -> io::Result<File> {
        one_name(name)?;
        sys::create_new(&self.file, name, private)
    }

    /// Renames `file`, at `from` in this folder, onto `to` in it: in place of
    /// what is there where `replace` (that entry goes; nothing is written
    /// into what it named), else only while nothing is (`AlreadyExists`).
    pub fn rename(&self, file: &File, from: &str, to: &str, replace: bool) -> io::Result<()> {
        one_name(from)?;
        one_name(to)?;
        sys::rename(&self.file, file, from, to, replace)
    }

    /// Takes away a file a write that failed left at `name`, best effort.
    pub fn remove_file(&self, file: &File, name: &str) {
        if one_name(name).is_ok() {
            sys::remove_file(&self.file, file, name);
        }
    }

    /// The folder `name` in this one and everything in it, removed as it is
    /// held: opened as itself (a link or any other reparse point there
    /// refused), held to `id` where one is given before anything goes, on
    /// this folder's device, and emptied through its handle, a link or
    /// junction inside removed as itself and never followed, a mount inside
    /// refused before anything goes. Ok(false): nothing was at the name.
    pub fn remove_tree(&self, name: &str, id: Option<&str>) -> io::Result<bool> {
        one_name(name)?;
        let dir = match sys::child_dir(&self.file, name, false, true) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e),
        };
        sys::plain_dir(&dir).map_err(|_| not_plain(name))?;
        if let Some(id) = id {
            if sys::id(&dir)? != id {
                return Err(io::Error::other(format!("{name} is not the folder you were asked about: it was replaced")));
            }
        }
        let top = sys::device(&self.file)?;
        same_device(top, sys::device(&dir)?, name)?;
        sys::remove_tree(&self.file, dir, name, top)?;
        Ok(true)
    }
}

/// Refuses a folder on another device or volume than `top`: a mount inside a
/// tree being removed, whose files are not the tree's.
fn same_device(top: u64, here: u64, name: &str) -> io::Result<()> {
    if top != here {
        return Err(io::Error::other(format!("{name} is another drive mounted here — Luna removes nothing past it")));
    }
    Ok(())
}

#[cfg(windows)]
mod sys {
    use super::same_device;
    use std::fs::File;
    use std::io;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, RawHandle};
    use std::path::Path;
    use windows::core::{PCWSTR, PWSTR};
    use windows::Wdk::Foundation::OBJECT_ATTRIBUTES;
    use windows::Wdk::Storage::FileSystem::{
        FileRenameInformation, NtCreateFile, NtSetInformationFile, FILE_CREATE, FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN,
        FILE_OPEN_IF, FILE_OPEN_REPARSE_POINT, FILE_RENAME_INFORMATION, FILE_SYNCHRONOUS_IO_NONALERT, NTCREATEFILE_CREATE_DISPOSITION,
        NTCREATEFILE_CREATE_OPTIONS,
    };
    use windows::Win32::Foundation::{LocalFree, RtlNtStatusToDosError, ERROR_NO_MORE_FILES, HANDLE, HLOCAL, OBJ_CASE_INSENSITIVE, UNICODE_STRING};
    use windows::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
    use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_DESCRIPTOR};
    use windows::Win32::Storage::FileSystem::{
        FileAttributeTagInfo, FileBasicInfo, FileDispositionInfo, FileDispositionInfoEx, FileFullDirectoryInfo, FileFullDirectoryRestartInfo,
        FileIdInfo, GetFileInformationByHandleEx, SetFileInformationByHandle, DELETE, FILE_ACCESS_RIGHTS, FILE_ADD_FILE, FILE_ADD_SUBDIRECTORY,
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_READONLY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO,
        FILE_BASIC_INFO, FILE_DISPOSITION_FLAG_DELETE, FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE, FILE_DISPOSITION_FLAG_POSIX_SEMANTICS,
        FILE_DISPOSITION_INFO, FILE_DISPOSITION_INFO_EX, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_FULL_DIR_INFO,
        FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_ID_INFO, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_MODE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TRAVERSE, FILE_WRITE_ATTRIBUTES, READ_CONTROL, SYNCHRONIZE,
    };
    use windows::Win32::System::IO::IO_STATUS_BLOCK;

    fn win(e: windows::core::Error) -> io::Error {
        io::Error::from_raw_os_error(e.code().0 & 0xFFFF)
    }

    /// A folder by path, shared for reading and writing but not deletion;
    /// as itself unless `follow`.
    pub fn open_path(path: &Path, follow: bool) -> io::Result<File> {
        let flags = FILE_FLAG_BACKUP_SEMANTICS.0 | if follow { 0 } else { FILE_FLAG_OPEN_REPARSE_POINT.0 };
        std::fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0).custom_flags(flags).open(path)
    }

    fn attributes(file: &File) -> io::Result<u32> {
        let mut tag = FILE_ATTRIBUTE_TAG_INFO::default();
        // SAFETY: the File's handle, open through the call; the buffer is of the size passed.
        unsafe {
            GetFileInformationByHandleEx(
                HANDLE(file.as_raw_handle()),
                FileAttributeTagInfo,
                (&raw mut tag).cast(),
                size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
            )
        }
        .map_err(win)?;
        Ok(tag.FileAttributes)
    }

    /// A folder and no reparse point of any kind, by the handle's word.
    pub fn plain_dir(file: &File) -> io::Result<()> {
        let a = attributes(file)?;
        if a & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 || a & FILE_ATTRIBUTE_DIRECTORY.0 == 0 {
            return Err(io::Error::other("not a plain folder"));
        }
        Ok(())
    }

    pub fn plain_file(file: &File) -> io::Result<bool> {
        let a = attributes(file)?;
        Ok(a & (FILE_ATTRIBUTE_REPARSE_POINT.0 | FILE_ATTRIBUTE_DIRECTORY.0) == 0)
    }

    fn id_info(file: &File) -> io::Result<FILE_ID_INFO> {
        let mut id = FILE_ID_INFO::default();
        // SAFETY: the File's handle, open through the call; the buffer is of
        // the size the call is told.
        unsafe { GetFileInformationByHandleEx(HANDLE(file.as_raw_handle()), FileIdInfo, (&raw mut id).cast(), size_of::<FILE_ID_INFO>() as u32) }
            .map_err(win)?;
        Ok(id)
    }

    pub fn id(file: &File) -> io::Result<String> {
        let id = id_info(file)?;
        let hex: String = id.FileId.Identifier.iter().map(|b| format!("{b:02x}")).collect();
        Ok(format!("{:016x}:{hex}", id.VolumeSerialNumber))
    }

    pub fn device(file: &File) -> io::Result<u64> {
        Ok(id_info(file)?.VolumeSerialNumber)
    }

    /// `name` in the folder `parent` is open on, opened or made by
    /// `disposition`; one it makes gets `sddl` (None: what the folder hands
    /// down). Matched without regard to case, as Win32 matches names.
    fn create(
        parent: &File,
        name: &[u16],
        access: u32,
        share: FILE_SHARE_MODE,
        disposition: NTCREATEFILE_CREATE_DISPOSITION,
        options: NTCREATEFILE_CREATE_OPTIONS,
        sddl: Option<&str>,
    ) -> io::Result<File> {
        let bytes = u16::try_from(name.len() * 2).map_err(|_| io::Error::other("a name too long"))?;
        let object = UNICODE_STRING { Length: bytes, MaximumLength: bytes, Buffer: PWSTR(name.as_ptr().cast_mut()) };
        let mut sd = PSECURITY_DESCRIPTOR::default();
        if let Some(sddl) = sddl {
            let text: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
            // SAFETY: a NUL-terminated string; the descriptor made is freed below.
            unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(PCWSTR(text.as_ptr()), SDDL_REVISION_1, &mut sd, None) }
                .map_err(win)?;
        }
        let attributes = OBJECT_ATTRIBUTES {
            Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
            RootDirectory: HANDLE(parent.as_raw_handle()),
            ObjectName: &object,
            Attributes: OBJ_CASE_INSENSITIVE,
            SecurityDescriptor: sd.0 as *const SECURITY_DESCRIPTOR,
            SecurityQualityOfService: std::ptr::null(),
        };
        let mut handle = HANDLE::default();
        let mut status_block = IO_STATUS_BLOCK::default();
        // SAFETY: `object`, the name it points at and the descriptor outlive
        // the call; the handle returned is owned by the File made of it.
        let status = unsafe {
            NtCreateFile(
                &mut handle,
                FILE_ACCESS_RIGHTS(access),
                &attributes,
                &mut status_block,
                None,
                FILE_ATTRIBUTE_NORMAL,
                share,
                disposition,
                options,
                None,
                0,
            )
        };
        if !sd.0.is_null() {
            // SAFETY: the descriptor made above, freed once.
            let _ = unsafe { LocalFree(Some(HLOCAL(sd.0))) };
        }
        if status.0 < 0 {
            // SAFETY: a plain conversion of a status code.
            return Err(io::Error::from_raw_os_error(unsafe { RtlNtStatusToDosError(status) } as i32));
        }
        // SAFETY: a handle just opened, owned by nothing else.
        Ok(unsafe { File::from_raw_handle(handle.0 as RawHandle) })
    }

    fn wide(name: &str) -> Vec<u16> {
        name.encode_utf16().collect()
    }

    const DIR_ACCESS: u32 =
        FILE_LIST_DIRECTORY.0 | FILE_TRAVERSE.0 | FILE_ADD_FILE.0 | FILE_ADD_SUBDIRECTORY.0 | FILE_READ_ATTRIBUTES.0 | READ_CONTROL.0 | SYNCHRONIZE.0;

    /// A folder in `parent`, as itself; with `DELETE` where `delete`, to be
    /// removed through the handle.
    pub fn child_dir(parent: &File, name: &str, make: bool, delete: bool) -> io::Result<File> {
        let access = DIR_ACCESS | if delete { DELETE.0 | FILE_WRITE_ATTRIBUTES.0 } else { 0 };
        let disposition = if make { FILE_OPEN_IF } else { FILE_OPEN };
        create(
            parent,
            &wide(name),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            disposition,
            FILE_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
            None,
        )
    }

    pub fn open_file(parent: &File, name: &str) -> io::Result<File> {
        create(
            parent,
            &wide(name),
            FILE_GENERIC_READ.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_OPEN,
            FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
            None,
        )
    }

    pub fn create_new(parent: &File, name: &str, private: bool) -> io::Result<File> {
        let sddl = if private {
            let user = crate::secret::acl::user_sid()?;
            Some(format!("D:P(A;;FA;;;{user})(A;;FA;;;SY)(A;;FA;;;BA)"))
        } else {
            None
        };
        create(
            parent,
            &wide(name),
            FILE_GENERIC_WRITE.0 | FILE_GENERIC_READ.0 | DELETE.0,
            FILE_SHARE_READ,
            FILE_CREATE,
            FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
            sddl.as_deref(),
        )
    }

    /// `NtSetInformationFile`'s rename, as Win32's `SetFileInformationByHandle`
    /// refuses one relative to a folder's handle.
    pub fn rename(dir: &File, file: &File, _from: &str, to: &str, replace: bool) -> io::Result<()> {
        let name = wide(to);
        // The struct and the name's bytes after it, as Microsoft documents
        // the length: more than the name needs past `FileName`, which leaves
        // room for its NUL.
        let size = size_of::<FILE_RENAME_INFORMATION>() + name.len() * 2;
        // u64s, for the struct's alignment.
        let mut buf = vec![0u64; size.div_ceil(8)];
        let info = buf.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
        let mut status_block = IO_STATUS_BLOCK::default();
        // SAFETY: the buffer holds the struct and the name after it, NUL
        // included, and is aligned for it; both handles are open through
        // the call.
        let status = unsafe {
            (*info).Anonymous.ReplaceIfExists = replace;
            (*info).RootDirectory = HANDLE(dir.as_raw_handle());
            (*info).FileNameLength = (name.len() * 2) as u32;
            std::ptr::copy_nonoverlapping(name.as_ptr(), (&raw mut (*info).FileName).cast::<u16>(), name.len());
            NtSetInformationFile(HANDLE(file.as_raw_handle()), &mut status_block, info.cast(), size as u32, FileRenameInformation)
        };
        if status.0 < 0 {
            // SAFETY: a plain conversion of a status code.
            return Err(io::Error::from_raw_os_error(unsafe { RtlNtStatusToDosError(status) } as i32));
        }
        Ok(())
    }

    pub fn remove_file(_dir: &File, file: &File, _name: &str) {
        let _ = delete(file);
    }

    /// Deletes what `file` is open on (opened with `DELETE`): at once and
    /// read-only or not where the file system takes POSIX semantics, else
    /// once its last handle closes, read-only taken off first.
    fn delete(file: &File) -> io::Result<()> {
        let handle = HANDLE(file.as_raw_handle());
        let ex = FILE_DISPOSITION_INFO_EX {
            Flags: FILE_DISPOSITION_FLAG_DELETE | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
        };
        // SAFETY: the File's handle, open through the call; the struct is the call's.
        let posix =
            unsafe { SetFileInformationByHandle(handle, FileDispositionInfoEx, (&raw const ex).cast(), size_of::<FILE_DISPOSITION_INFO_EX>() as u32) };
        if posix.is_ok() {
            return Ok(());
        }
        let mut basic = FILE_BASIC_INFO::default();
        // SAFETY: as above.
        unsafe { GetFileInformationByHandleEx(handle, FileBasicInfo, (&raw mut basic).cast(), size_of::<FILE_BASIC_INFO>() as u32) }.map_err(win)?;
        if basic.FileAttributes & FILE_ATTRIBUTE_READONLY.0 != 0 {
            let left = basic.FileAttributes & !FILE_ATTRIBUTE_READONLY.0;
            basic.FileAttributes = if left == 0 { FILE_ATTRIBUTE_NORMAL.0 } else { left };
            // SAFETY: as above.
            unsafe { SetFileInformationByHandle(handle, FileBasicInfo, (&raw const basic).cast(), size_of::<FILE_BASIC_INFO>() as u32) }
                .map_err(win)?;
        }
        let info = FILE_DISPOSITION_INFO { DeleteFile: true };
        // SAFETY: as above.
        unsafe { SetFileInformationByHandle(handle, FileDispositionInfo, (&raw const info).cast(), size_of::<FILE_DISPOSITION_INFO>() as u32) }
            .map_err(win)
    }

    /// The names in the folder `dir` is open on, `.` and `..` aside, with
    /// their attributes.
    fn list(dir: &File) -> io::Result<Vec<(Vec<u16>, u32)>> {
        let mut out = Vec::new();
        let mut buf = vec![0u64; 8 * 1024];
        let mut class = FileFullDirectoryRestartInfo;
        loop {
            // SAFETY: the File's handle, open through the call; the buffer is
            // of the size passed.
            let got = unsafe { GetFileInformationByHandleEx(HANDLE(dir.as_raw_handle()), class, buf.as_mut_ptr().cast(), (buf.len() * 8) as u32) };
            if let Err(e) = got {
                if e.code() == ERROR_NO_MORE_FILES.to_hresult() {
                    return Ok(out);
                }
                return Err(win(e));
            }
            class = FileFullDirectoryInfo;
            let base = buf.as_ptr().cast::<u8>();
            let mut at = 0usize;
            loop {
                // SAFETY: the call wrote whole entries into the buffer, each
                // at an offset the one before names and aligned to 8; the
                // name lies within the entry.
                let (next, attrs, name) = unsafe {
                    let entry = base.add(at).cast::<FILE_FULL_DIR_INFO>();
                    let len = (*entry).FileNameLength as usize / 2;
                    let name = std::slice::from_raw_parts((&raw const (*entry).FileName).cast::<u16>(), len).to_vec();
                    ((*entry).NextEntryOffset as usize, (*entry).FileAttributes, name)
                };
                let dots = name == [b'.' as u16] || name == [b'.' as u16, b'.' as u16];
                if !dots {
                    out.push((name, attrs));
                }
                if next == 0 {
                    break;
                }
                at += next;
            }
        }
    }

    /// Empties the folder `dir` is open on (opened with `DELETE`) through
    /// its handle and deletes it. A reparse point inside is opened and
    /// deleted as itself, never followed; a mount point is one. `parent` and
    /// `name` are Unix's.
    pub fn remove_tree(_parent: &File, dir: File, _name: &str, top: u64) -> io::Result<()> {
        empty(&dir, top)?;
        delete(&dir)
    }

    fn empty(dir: &File, top: u64) -> io::Result<()> {
        for (name, attrs) in list(dir)? {
            let shown = String::from_utf16_lossy(&name);
            let child = create(
                dir,
                &name,
                DELETE.0 | FILE_LIST_DIRECTORY.0 | FILE_READ_ATTRIBUTES.0 | FILE_WRITE_ATTRIBUTES.0 | SYNCHRONIZE.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                FILE_OPEN,
                FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
                None,
            )
            .map_err(|e| io::Error::new(e.kind(), format!("{shown}: {e}")))?;
            // The handle's word, not the listing's: what is at the name now.
            let now = attributes(&child).unwrap_or(attrs);
            if now & FILE_ATTRIBUTE_DIRECTORY.0 != 0 && now & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0 {
                same_device(top, device(&child)?, &shown)?;
                empty(&child, top)?;
            }
            delete(&child).map_err(|e| io::Error::new(e.kind(), format!("{shown}: {e}")))?;
        }
        Ok(())
    }
}

#[cfg(unix)]
mod sys {
    use super::same_device;
    use std::ffi::{CStr, CString};
    use std::fs::File;
    use std::io;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
    use std::path::Path;

    fn c(name: &str) -> io::Result<CString> {
        CString::new(name).map_err(io::Error::other)
    }

    fn last<T>(r: libc::c_int, ok: T) -> io::Result<T> {
        if r < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(ok)
        }
    }

    fn own(fd: RawFd) -> io::Result<File> {
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a descriptor just opened, owned by nothing else.
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    pub fn open_path(path: &Path, follow: bool) -> io::Result<File> {
        let flags = libc::O_DIRECTORY | libc::O_CLOEXEC | if follow { 0 } else { libc::O_NOFOLLOW };
        std::fs::OpenOptions::new().read(true).custom_flags(flags).open(path)
    }

    pub fn plain_dir(file: &File) -> io::Result<()> {
        if !file.metadata()?.is_dir() {
            return Err(io::Error::other("not a plain folder"));
        }
        Ok(())
    }

    pub fn plain_file(file: &File) -> io::Result<bool> {
        Ok(file.metadata()?.is_file())
    }

    pub fn id(file: &File) -> io::Result<String> {
        let m = file.metadata()?;
        Ok(format!("{:x}:{:x}", m.dev(), m.ino()))
    }

    pub fn device(file: &File) -> io::Result<u64> {
        Ok(file.metadata()?.dev())
    }

    fn open_dir_at(parent: &File, name: &CStr) -> io::Result<File> {
        let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
        // SAFETY: the held folder's descriptor and a NUL-terminated name.
        own(unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) })
    }

    pub fn child_dir(parent: &File, name: &str, make: bool, _delete: bool) -> io::Result<File> {
        let name = c(name)?;
        if make {
            // SAFETY: as above.
            if unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o777) } != 0 {
                let e = io::Error::last_os_error();
                if e.kind() != io::ErrorKind::AlreadyExists {
                    return Err(e);
                }
            }
        }
        open_dir_at(parent, &name)
    }

    pub fn open_file(parent: &File, name: &str) -> io::Result<File> {
        let name = c(name)?;
        let flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC;
        // SAFETY: the held folder's descriptor and a NUL-terminated name.
        own(unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) })
    }

    pub fn create_new(parent: &File, name: &str, private: bool) -> io::Result<File> {
        let name = c(name)?;
        let flags = libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC;
        let mode: libc::c_uint = if private { 0o600 } else { 0o666 };
        // SAFETY: as above.
        own(unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags, mode) })
    }

    pub fn rename(dir: &File, _file: &File, from: &str, to: &str, replace: bool) -> io::Result<()> {
        let (from, to, fd) = (c(from)?, c(to)?, dir.as_raw_fd());
        if replace {
            // SAFETY: the held folder's descriptor twice, and NUL-terminated names.
            return last(unsafe { libc::renameat(fd, from.as_ptr(), fd, to.as_ptr()) }, ());
        }
        #[cfg(target_os = "linux")]
        {
            // SAFETY: as above.
            let r = unsafe { libc::renameat2(fd, from.as_ptr(), fd, to.as_ptr(), libc::RENAME_NOREPLACE) };
            match last(r, ()) {
                // A file system without it: a link and an unlink do the same.
                Err(e) if matches!(e.raw_os_error(), Some(libc::EINVAL) | Some(libc::ENOSYS)) => {}
                other => return other,
            }
        }
        #[cfg(target_os = "macos")]
        {
            // SAFETY: as above.
            let r = unsafe { libc::renameatx_np(fd, from.as_ptr(), fd, to.as_ptr(), libc::RENAME_EXCL) };
            match last(r, ()) {
                Err(e) if e.raw_os_error() == Some(libc::ENOTSUP) => {}
                other => return other,
            }
        }
        // SAFETY: as above.
        last(unsafe { libc::linkat(fd, from.as_ptr(), fd, to.as_ptr(), 0) }, ())?;
        // SAFETY: as above.
        unsafe { libc::unlinkat(fd, from.as_ptr(), 0) };
        Ok(())
    }

    pub fn remove_file(dir: &File, _file: &File, name: &str) {
        if let Ok(name) = c(name) {
            // SAFETY: the held folder's descriptor and a NUL-terminated name.
            unsafe { libc::unlinkat(dir.as_raw_fd(), name.as_ptr(), 0) };
        }
    }

    /// The names in the folder `dir` is open on, `.` and `..` aside.
    fn list(dir: &File) -> io::Result<Vec<CString>> {
        // SAFETY: dup of a descriptor open through the call; fdopendir owns
        // the copy, closedir closes it.
        let copy = unsafe { libc::dup(dir.as_raw_fd()) };
        if copy < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a descriptor of a folder, handed over.
        let stream = unsafe { libc::fdopendir(copy) };
        if stream.is_null() {
            let e = io::Error::last_os_error();
            // SAFETY: the copy, not taken by fdopendir.
            unsafe { libc::close(copy) };
            return Err(e);
        }
        // From the start, whatever an earlier read of the descriptor left.
        // SAFETY: the stream just opened.
        unsafe { libc::rewinddir(stream) };
        let mut out = Vec::new();
        loop {
            // SAFETY: the stream is open; the entry is read before the next call.
            let entry = unsafe { libc::readdir(stream) };
            if entry.is_null() {
                break;
            }
            // SAFETY: d_name is NUL-terminated within the entry.
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
            if name.to_bytes() != b"." && name.to_bytes() != b".." {
                out.push(name.to_owned());
            }
        }
        // SAFETY: the stream, closed once.
        unsafe { libc::closedir(stream) };
        Ok(out)
    }

    fn lstat_at(dir: &File, name: &CStr) -> io::Result<libc::stat> {
        // SAFETY: zeroed is a valid stat to be written over.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: the held folder's descriptor, a NUL-terminated name, a stat to fill.
        last(unsafe { libc::fstatat(dir.as_raw_fd(), name.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) }, st)
    }

    fn shown(name: &CStr) -> String {
        std::ffi::OsStr::from_bytes(name.to_bytes()).to_string_lossy().into_owned()
    }

    /// Walks the tree `dir` is open on, each folder opened as itself and on
    /// `top`'s device; removes what it walks where `remove`.
    fn walk(dir: &File, top: u64, remove: bool) -> io::Result<()> {
        for name in list(dir)? {
            let st = lstat_at(dir, &name)?;
            if st.st_mode & libc::S_IFMT == libc::S_IFDIR {
                same_device(top, st.st_dev as u64, &shown(&name))?;
                let child = open_dir_at(dir, &name)?;
                // What was opened is what was looked at.
                let m = child.metadata()?;
                if m.dev() != st.st_dev as u64 || m.ino() != st.st_ino as u64 {
                    return Err(io::Error::other(format!("{} changed while it was removed", shown(&name))));
                }
                walk(&child, top, remove)?;
                drop(child);
                if remove {
                    // SAFETY: the held folder's descriptor and a NUL-terminated name.
                    last(unsafe { libc::unlinkat(dir.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) }, ())
                        .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", shown(&name))))?;
                }
            } else if remove {
                // A file, a link (removed as itself), a FIFO or a socket.
                // SAFETY: as above.
                last(unsafe { libc::unlinkat(dir.as_raw_fd(), name.as_ptr(), 0) }, ())
                    .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", shown(&name))))?;
            }
        }
        Ok(())
    }

    /// Looks the whole tree over first (a mount anywhere in it refuses the
    /// removal before anything goes), then empties it through its handle and
    /// removes it from `parent`. Between the last look and that `unlinkat`
    /// another empty folder could be put at the name; that one goes then, and
    /// the one emptied stays, empty.
    pub fn remove_tree(parent: &File, dir: File, name: &str, top: u64) -> io::Result<()> {
        walk(&dir, top, false)?;
        walk(&dir, top, true)?;
        let held = dir.metadata()?;
        drop(dir);
        let name = c(name)?;
        let st = lstat_at(parent, &name)?;
        if st.st_dev as u64 != held.dev() || st.st_ino as u64 != held.ino() {
            return Err(io::Error::other(format!("{} was replaced while it was removed; its contents went", shown(&name))));
        }
        // SAFETY: the held folder's descriptor and a NUL-terminated name.
        last(unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) }, ())
    }
}

#[cfg(not(any(windows, unix)))]
mod sys {
    use std::fs::File;
    use std::io;
    use std::path::Path;

    fn none<T>() -> io::Result<T> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "no folder held by handle on this system"))
    }
    pub fn open_path(_: &Path, _: bool) -> io::Result<File> {
        none()
    }
    pub fn plain_dir(_: &File) -> io::Result<()> {
        none()
    }
    pub fn plain_file(_: &File) -> io::Result<bool> {
        none()
    }
    pub fn id(_: &File) -> io::Result<String> {
        none()
    }
    pub fn device(_: &File) -> io::Result<u64> {
        none()
    }
    pub fn child_dir(_: &File, _: &str, _: bool, _: bool) -> io::Result<File> {
        none()
    }
    pub fn open_file(_: &File, _: &str) -> io::Result<File> {
        none()
    }
    pub fn create_new(_: &File, _: &str, _: bool) -> io::Result<File> {
        none()
    }
    pub fn rename(_: &File, _: &File, _: &str, _: &str, _: bool) -> io::Result<()> {
        none()
    }
    pub fn remove_file(_: &File, _: &File, _: &str) {}
    pub fn remove_tree(_: &File, _: File, _: &str, _: u64) -> io::Result<()> {
        none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn base(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("luna-held-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A junction on Windows (no privilege needed), a symlink elsewhere.
    fn link_dir(link: &Path, target: &Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).unwrap();
        #[cfg(windows)]
        assert!(std::process::Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .is_ok_and(|o| o.status.success()));
    }

    fn unlink_dir(link: &Path) {
        let _ = std::fs::remove_dir(link).or_else(|_| std::fs::remove_file(link));
    }

    /// A folder held keeps its identity; what is reached through it is in
    /// it, whatever the link that led there leads to now.
    #[test]
    fn a_held_folder_is_the_one_opened_whatever_the_link_does_after() {
        let b = base("retarget");
        let (one, two, link) = (b.join("one"), b.join("two"), b.join("lib"));
        std::fs::create_dir_all(&one).unwrap();
        std::fs::create_dir_all(&two).unwrap();
        link_dir(&link, &one);
        let held = Dir::open_followed(&link).unwrap();
        let id = held.id().unwrap();
        assert_eq!(Dir::open_followed(&one).unwrap().id().unwrap(), id);
        // Retargeted: the path leads to another folder, of another id.
        unlink_dir(&link);
        link_dir(&link, &two);
        assert_ne!(Dir::open_followed(&link).unwrap().id().unwrap(), id);
        // Written through the handle, the file lands where it was held.
        let mut f = held.create_new("a.tmp", false).unwrap();
        f.write_all(b"x").unwrap();
        held.rename(&f, "a.tmp", "a.md", false).unwrap();
        drop(f);
        assert!(one.join("a.md").is_file() && !two.join("a.md").exists());
        // As itself, the link is refused.
        assert!(Dir::open_itself(&link).is_err());
        unlink_dir(&link);
        let _ = std::fs::remove_dir_all(&b);
    }

    #[test]
    fn a_rename_that_may_not_replace_leaves_what_is_there() {
        let b = base("noreplace");
        let held = Dir::open_itself(&b).unwrap();
        std::fs::write(b.join("a.md"), "theirs").unwrap();
        let mut f = held.create_new("a.tmp", false).unwrap();
        f.write_all(b"mine").unwrap();
        let e = held.rename(&f, "a.tmp", "a.md", false).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists, "{e}");
        assert_eq!(std::fs::read(b.join("a.md")).unwrap(), b"theirs");
        held.rename(&f, "a.tmp", "a.md", true).unwrap();
        drop(f);
        assert_eq!(std::fs::read(b.join("a.md")).unwrap(), b"mine");
        // Made new only: a name taken is not opened.
        assert!(held.create_new("a.md", true).is_err());
        let _ = std::fs::remove_dir_all(&b);
    }

    #[test]
    fn links_in_a_held_folder_are_refused_as_folders_and_files() {
        let b = base("links");
        let out = b.join("out");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("SKILL.md"), "outside").unwrap();
        let lib = b.join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        link_dir(&lib.join("skills"), &out);
        let held = Dir::open_itself(&lib).unwrap();
        assert!(held.child_dir("skills", false).is_err());
        assert!(held.child_dir("skills", true).is_err());
        assert!(held.child_dir("none", false).unwrap().is_none());
        let made = held.child_dir("made", true).unwrap().unwrap();
        assert!(made.open_file("SKILL.md").unwrap().is_none());
        std::fs::write(lib.join("made").join("SKILL.md"), "v1").unwrap();
        let mut text = String::new();
        made.open_file("SKILL.md").unwrap().unwrap().read_to_string(&mut text).unwrap();
        assert_eq!(text, "v1");
        assert!(held.open_file("../out").is_err() && held.child_dir("a/b", true).is_err());
        unlink_dir(&lib.join("skills"));
        let _ = std::fs::remove_dir_all(&b);
    }

    /// A tree removed through its handle: bound to the id it was asked
    /// about, a link inside removed as itself and what it leads to left.
    #[test]
    fn a_tree_goes_as_held_and_no_further() {
        let b = base("tree");
        let (parent, out) = (b.join("claude"), b.join("out"));
        std::fs::create_dir_all(parent.join("work").join("deep")).unwrap();
        std::fs::write(parent.join("work").join("deep").join("a.json"), "{}").unwrap();
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("keep.json"), "{}").unwrap();
        link_dir(&parent.join("work").join("link"), &out);
        let ro = parent.join("work").join("ro.txt");
        std::fs::write(&ro, "r").unwrap();
        let mut perms = std::fs::metadata(&ro).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&ro, perms).unwrap();
        let held = Dir::open_itself(&parent).unwrap();
        let id = held.child_dir("work", false).unwrap().unwrap().id().unwrap();
        // Replaced between the question and the removal: refused, all kept.
        std::fs::rename(parent.join("work"), b.join("aside")).unwrap();
        std::fs::create_dir_all(parent.join("work")).unwrap();
        assert!(held.remove_tree("work", Some(&id)).unwrap_err().to_string().contains("replaced"));
        assert!(parent.join("work").is_dir());
        std::fs::remove_dir(parent.join("work")).unwrap();
        std::fs::rename(b.join("aside"), parent.join("work")).unwrap();
        // The one asked about goes, with nothing past its link.
        assert!(held.remove_tree("work", Some(&id)).unwrap());
        assert!(!parent.join("work").exists());
        assert!(out.join("keep.json").is_file());
        assert!(!held.remove_tree("work", None).unwrap());
        // A link at the name itself is refused, and what it leads to stays.
        link_dir(&parent.join("work"), &out);
        assert!(held.remove_tree("work", None).is_err());
        assert!(out.join("keep.json").is_file());
        unlink_dir(&parent.join("work"));
        let _ = std::fs::remove_dir_all(&b);
    }

    /// A mount inside a tree is found by its device: the removal stops there.
    #[test]
    fn another_device_is_not_removed_into() {
        assert!(same_device(7, 7, "x").is_ok());
        assert!(same_device(7, 8, "mnt").unwrap_err().to_string().contains("mounted"));
    }
}
