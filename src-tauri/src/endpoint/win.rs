//! The endpoint on Windows: a named pipe, every handle overlapped so that a
//! read and a write can wait at once on one connection, each with its own
//! deadline.

use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, LocalFree, ERROR_ACCESS_DENIED, ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_NO_DATA, ERROR_OPERATION_ABORTED,
    ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, ERROR_PIPE_NOT_CONNECTED, GENERIC_READ, GENERIC_WRITE, HANDLE, HLOCAL, WAIT_OBJECT_0,
};
use windows::Win32::Security::Authorization::{
    ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
    SDDL_REVISION_1, SE_KERNEL_OBJECT,
};
use windows::Win32::Security::{DACL_SECURITY_INFORMATION, LABEL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, FILE_SHARE_NONE, OPEN_EXISTING,
    PIPE_ACCESS_DUPLEX, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeServerProcessId, WaitNamedPipeW, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject, INFINITE};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

const PREFIX: &str = r"\\.\pipe\luna-core-";
/// Each direction's buffer: a frame larger than this waits for its reader,
/// as it would on a socket.
const BUFFER: u32 = 64 * 1024;

pub fn fresh_name() -> String {
    format!("{PREFIX}{}", crate::secret::random_hex(16))
}

pub fn valid_name(name: &str) -> bool {
    name.strip_prefix(PREFIX).is_some_and(|rest| rest.len() == 32 && rest.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
}

/// The pipe's security, as SDDL: full access for `user` and SYSTEM, read and
/// write for Administrators (an administrator of another account may ask the
/// core to stop and nothing more, peer.rs), nobody else; and a medium label
/// that refuses a process of lower integrity read and write alike, so a
/// low-integrity one cannot even open it. Protected, so nothing is inherited.
pub(crate) fn pipe_sddl(user: &str) -> String {
    format!("D:P(A;;GA;;;{user})(A;;GA;;;SY)(A;;GRGW;;;BA)S:(ML;;NWNR;;;ME)")
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// A kernel handle, closed when dropped.
struct Owned(HANDLE);

// SAFETY: a kernel handle is a process-wide value any thread may use; the
// pipe's operations below take no thread-affine state.
unsafe impl Send for Owned {}
// SAFETY: as above; concurrent reads and writes on one overlapped handle are
// what it is for, each with its own OVERLAPPED.
unsafe impl Sync for Owned {}

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: a handle this module opened and nothing else closes.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// A manual-reset event for one overlapped operation.
fn event() -> io::Result<Owned> {
    // SAFETY: no attributes, no name; the handle is owned from here.
    Ok(Owned(unsafe { CreateEventW(None, true, false, PCWSTR::null()) }.map_err(io_err)?))
}

fn is(e: &windows::core::Error, code: windows::Win32::Foundation::WIN32_ERROR) -> bool {
    e.code() == code.to_hresult()
}

/// A Win32 failure as std's error of that code, so its kind reads as std's
/// (NotFound, PermissionDenied, BrokenPipe): the crate's own conversion keeps
/// the HRESULT, which std knows nothing of.
pub(crate) fn io_err(e: windows::core::Error) -> io::Error {
    const FACILITY_WIN32: i32 = 7;
    let hr = e.code().0;
    if (hr >> 16) & 0x1fff == FACILITY_WIN32 {
        io::Error::from_raw_os_error(hr & 0xffff)
    } else {
        io::Error::other(e)
    }
}

/// What makes a pipe's end read as the end of the stream: the other side gone
/// or disconnected.
fn is_end(e: &io::Error) -> bool {
    [ERROR_BROKEN_PIPE, ERROR_PIPE_NOT_CONNECTED, ERROR_NO_DATA].iter().any(|c| e.raw_os_error() == Some(c.0 as i32))
}

struct Inner {
    handle: Owned,
    /// The core's end of a connection: `shutdown` disconnects the client too.
    server: bool,
    /// Set by `shutdown`: no read or write starts after it.
    closed: AtomicBool,
    /// Milliseconds, 0 for none: shared by every clone, as a socket's are.
    read_ms: AtomicU64,
    write_ms: AtomicU64,
}

/// One connection's end. Clones share the handle.
pub struct Stream(Arc<Inner>);

impl Stream {
    fn new(handle: Owned, server: bool) -> Stream {
        Stream(Arc::new(Inner { handle, server, closed: AtomicBool::new(false), read_ms: AtomicU64::new(0), write_ms: AtomicU64::new(0) }))
    }

    /// Opens the endpoint `name`, waiting up to `within` for a free instance
    /// while the core is busy taking the last one's connection. Opened for
    /// identification only: the server can learn who this is, never act as it,
    /// so a server that is not the core gains nothing from a window's open.
    pub fn connect(name: &str, within: Duration) -> io::Result<Stream> {
        if !valid_name(name) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a core endpoint"));
        }
        let wide = wide(name);
        let deadline = Instant::now() + within;
        loop {
            // SAFETY: a NUL-terminated name that outlives the call; no
            // security attributes, so the handle is not inheritable.
            let opened = unsafe {
                CreateFileW(
                    PCWSTR(wide.as_ptr()),
                    GENERIC_READ.0 | GENERIC_WRITE.0,
                    FILE_SHARE_NONE,
                    None,
                    OPEN_EXISTING,
                    FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                    None,
                )
            };
            match opened {
                Ok(h) => return Ok(Stream::new(Owned(h), false)),
                Err(e) if is(&e, ERROR_PIPE_BUSY) => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Err(io::Error::new(io::ErrorKind::TimedOut, "the core's endpoint stayed busy"));
                    }
                    // SAFETY: as above. Returns once an instance is free or
                    // the time is up; the open is tried again either way.
                    let _ = unsafe { WaitNamedPipeW(PCWSTR(wide.as_ptr()), left.as_millis().clamp(1, 60_000) as u32) };
                }
                Err(e) => return Err(io_err(e)),
            }
        }
    }

    pub fn try_clone(&self) -> io::Result<Stream> {
        Ok(Stream(Arc::clone(&self.0)))
    }

    /// Ends the connection both ways: a read or write waiting on it, on any
    /// thread, returns, and none starts after. On the core's end the client
    /// is disconnected and hears the end at once.
    pub fn shutdown(&self) {
        self.0.closed.store(true, Ordering::SeqCst);
        // SAFETY: the handle lives as long as `self`; canceling and
        // disconnecting are valid on any pipe handle, at any time.
        unsafe {
            let _ = CancelIoEx(self.0.handle.0, None);
            if self.0.server {
                let _ = DisconnectNamedPipe(self.0.handle.0);
            }
        }
    }

    pub fn set_read_timeout(&self, within: Option<Duration>) -> io::Result<()> {
        self.0.read_ms.store(millis(within)?, Ordering::SeqCst);
        Ok(())
    }

    pub fn set_write_timeout(&self, within: Option<Duration>) -> io::Result<()> {
        self.0.write_ms.store(millis(within)?, Ordering::SeqCst);
        Ok(())
    }

    /// The process serving the endpoint this end opened.
    pub fn server_pid(&self) -> io::Result<u32> {
        let mut pid = 0u32;
        // SAFETY: a pipe handle of this stream; `pid` is written once.
        unsafe { GetNamedPipeServerProcessId(self.0.handle.0, &mut pid) }.map_err(io_err)?;
        Ok(pid)
    }

    /// The pipe handle, for reading who is at the other end (peer.rs).
    pub(crate) fn raw(&self) -> HANDLE {
        self.0.handle.0
    }

    fn closed(&self) -> bool {
        self.0.closed.load(Ordering::SeqCst)
    }

    /// One overlapped read or write, `start`ed with the OVERLAPPED it is to
    /// use, waited on for at most `ms` (0: no limit). Returns only once the
    /// operation is over, canceled or not, so the buffer and the OVERLAPPED
    /// it was given outlive it.
    fn overlapped(&self, ms: u64, start: impl FnOnce(*mut OVERLAPPED) -> windows::core::Result<()>) -> io::Result<u32> {
        let done = event()?;
        let mut ov = OVERLAPPED { hEvent: done.0, ..Default::default() };
        match start(&mut ov) {
            Ok(()) => {}
            Err(e) if is(&e, ERROR_IO_PENDING) => {}
            Err(e) => return Err(io_err(e)),
        }
        let wait = if ms == 0 { INFINITE } else { ms.min(u64::from(INFINITE - 1)) as u32 };
        // SAFETY: the event made above, alive until the end of this call.
        let waited = unsafe { WaitForSingleObject(done.0, wait) };
        let timed_out = waited != WAIT_OBJECT_0;
        if timed_out {
            // SAFETY: cancels this operation alone, if it is still pending.
            let _ = unsafe { CancelIoEx(self.0.handle.0, Some(&ov)) };
        }
        let mut n = 0u32;
        // SAFETY: waits for the operation to be over, as it is once
        // canceled; `ov` and the buffer are alive meanwhile.
        match unsafe { GetOverlappedResult(self.0.handle.0, &ov, &mut n, true) } {
            // Done after all, the timeout notwithstanding: what came is real.
            Ok(()) => Ok(n),
            Err(e) if timed_out && is(&e, ERROR_OPERATION_ABORTED) => Err(io::Error::new(io::ErrorKind::TimedOut, "deadline passed")),
            Err(e) => Err(io_err(e)),
        }
    }

    fn read_some(&self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() || self.closed() {
            return Ok(0);
        }
        let len = buf.len().min(u32::MAX as usize);
        let h = self.0.handle.0;
        // SAFETY: the buffer outlives the operation (`overlapped` waits for
        // its end); no byte count is asked of an overlapped call.
        let got =
            self.overlapped(self.0.read_ms.load(Ordering::SeqCst), |ov| unsafe { ReadFile(h, Some(&mut buf[..len]), None, Some(ov)) });
        match got {
            Ok(n) => Ok(n as usize),
            Err(e) if is_end(&e) => Ok(0),
            // Canceled by `shutdown`: the end, as a socket shut down reads.
            Err(e) if self.closed() && e.raw_os_error() == Some(ERROR_OPERATION_ABORTED.0 as i32) => Ok(0),
            Err(e) => Err(e),
        }
    }

    fn write_some(&self, buf: &[u8]) -> io::Result<usize> {
        if self.closed() {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "the connection was shut down"));
        }
        if buf.is_empty() {
            return Ok(0);
        }
        let len = buf.len().min(u32::MAX as usize);
        let h = self.0.handle.0;
        // SAFETY: as in `read_some`.
        let wrote =
            self.overlapped(self.0.write_ms.load(Ordering::SeqCst), |ov| unsafe { WriteFile(h, Some(&buf[..len]), None, Some(ov)) });
        match wrote {
            Ok(n) => Ok(n as usize),
            Err(e) if is_end(&e) || (self.closed() && e.raw_os_error() == Some(ERROR_OPERATION_ABORTED.0 as i32)) => {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, e))
            }
            Err(e) => Err(e),
        }
    }
}

fn millis(within: Option<Duration>) -> io::Result<u64> {
    match within {
        // As std's sockets: a zero timeout is no timeout anyone could mean.
        Some(d) if d.is_zero() => Err(io::Error::new(io::ErrorKind::InvalidInput, "a zero timeout")),
        Some(d) => Ok((d.as_millis() as u64).max(1)),
        None => Ok(0),
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.read_some(buf)
    }
}

impl Read for &Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.read_some(buf)
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.write_some(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Write for &Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.write_some(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A security descriptor made from SDDL, freed when dropped.
struct Descriptor(PSECURITY_DESCRIPTOR);

// SAFETY: an immutable block of memory, only read once made.
unsafe impl Send for Descriptor {}
// SAFETY: as above.
unsafe impl Sync for Descriptor {}

impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: LocalAlloc'd by the conversion that made it; freed once.
        let _ = unsafe { LocalFree(Some(HLOCAL(self.0 .0))) };
    }
}

/// The DACL and label of the pipe `h` belongs to, as SDDL.
fn security_of(h: HANDLE) -> io::Result<String> {
    let mut sd = PSECURITY_DESCRIPTOR::default();
    let which = DACL_SECURITY_INFORMATION | LABEL_SECURITY_INFORMATION;
    // SAFETY: a handle with READ_CONTROL (the server's own, full access); the
    // descriptor is LocalAlloc'd for the caller, freed below.
    let got = unsafe { GetSecurityInfo(h, SE_KERNEL_OBJECT, which, None, None, None, None, Some(&mut sd)) };
    if got.is_err() {
        return Err(io::Error::from_raw_os_error(got.0 as i32));
    }
    let sd = Descriptor(sd);
    let mut text = PWSTR::null();
    // SAFETY: the descriptor is alive; the string is LocalAlloc'd, read and
    // freed here.
    unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(sd.0, SDDL_REVISION_1, which, &mut text, None).map_err(io_err)?;
        let out = text.to_string();
        let _ = LocalFree(Some(HLOCAL(text.0.cast())));
        out.map_err(io::Error::other)
    }
}

/// The core's end of the endpoint: always one instance waiting for the next
/// client, so the name is never free for someone else to take between two
/// connections.
pub struct Listener {
    wide: Vec<u16>,
    sd: Descriptor,
    /// What the first instance read back as: every later one must too, or it
    /// joined a pipe of someone else's making.
    security: String,
    next: Mutex<Option<Owned>>,
}

impl Listener {
    /// Serves `name`, refusing it when it is there already: a pipe someone
    /// made first under that name is theirs, with their DACL, and whoever
    /// connected to it would be talking to them.
    pub fn bind(name: &str) -> io::Result<Listener> {
        if !valid_name(name) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a core endpoint"));
        }
        let user = crate::secret::acl::user_sid()?;
        let sddl = wide(&pipe_sddl(&user));
        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SAFETY: NUL-terminated SDDL; the descriptor is owned from here.
        unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(PCWSTR(sddl.as_ptr()), SDDL_REVISION_1, &mut sd, None) }
            .map_err(io_err)?;
        let mut listener = Listener { wide: wide(name), sd: Descriptor(sd), security: String::new(), next: Mutex::new(None) };
        let first = listener.instance(true).map_err(|e| {
            if e.raw_os_error() == Some(ERROR_ACCESS_DENIED.0 as i32) {
                io::Error::new(io::ErrorKind::AddrInUse, format!("{name} is held by another process"))
            } else {
                e
            }
        })?;
        listener.security = security_of(first.0)?;
        *listener.next.get_mut().unwrap_or_else(|e| e.into_inner()) = Some(first);
        Ok(listener)
    }

    fn instance(&self, first: bool) -> io::Result<Owned> {
        let sa = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.sd.0 .0,
            bInheritHandle: false.into(),
        };
        let mut open = PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED;
        if first {
            open |= FILE_FLAG_FIRST_PIPE_INSTANCE;
        }
        let mode = PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS;
        // SAFETY: the name and the descriptor outlive the call; the handle
        // is owned from here.
        let h = unsafe { CreateNamedPipeW(PCWSTR(self.wide.as_ptr()), open, mode, PIPE_UNLIMITED_INSTANCES, BUFFER, BUFFER, 0, Some(&sa)) };
        if h.is_invalid() {
            return Err(io::Error::last_os_error());
        }
        let h = Owned(h);
        if !first && security_of(h.0)? != self.security {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "the pipe's name was taken over by another process"));
        }
        Ok(h)
    }

    /// The next client, once one connects. Only the core's accept loop calls
    /// this, one call at a time.
    pub fn accept(&self) -> io::Result<Stream> {
        let mut next = self.next.lock().unwrap_or_else(|e| e.into_inner());
        let pipe = match next.take() {
            Some(pipe) => pipe,
            None => self.instance(false)?,
        };
        connect(&pipe)?;
        // The next instance before this one is handed on, while this one keeps
        // the name: should it fail, the next call makes one and checks it.
        *next = self.instance(false).ok();
        Ok(Stream::new(pipe, true))
    }
}

/// Waits for a client on `pipe`.
fn connect(pipe: &Owned) -> io::Result<()> {
    loop {
        let done = event()?;
        let mut ov = OVERLAPPED { hEvent: done.0, ..Default::default() };
        // SAFETY: `ov` and its event outlive the operation: it is waited for
        // below before either is dropped.
        match unsafe { ConnectNamedPipe(pipe.0, Some(&mut ov)) } {
            Ok(()) => return Ok(()),
            // A client came between the instance's making and this call.
            Err(e) if is(&e, ERROR_PIPE_CONNECTED) => return Ok(()),
            Err(e) if is(&e, ERROR_IO_PENDING) => {
                let mut n = 0u32;
                // SAFETY: as above; waits for the connection.
                return unsafe { GetOverlappedResult(pipe.0, &ov, &mut n, true) }.map_err(io_err);
            }
            // A client came and went before it was taken: the instance is
            // freed and waits again.
            Err(e) if is(&e, ERROR_NO_DATA) => {
                // SAFETY: the instance's own handle.
                let _ = unsafe { DisconnectNamedPipe(pipe.0) };
            }
            Err(e) => return Err(io_err(e)),
        }
    }
}

#[cfg(test)]
pub(super) fn test_name() -> String {
    fresh_name()
}

#[cfg(test)]
pub(super) fn invalid_names() -> Vec<&'static str> {
    vec![
        "",
        r"\\.\pipe\luna-core-",
        r"\\.\pipe\luna-core-0123456789abcdef0123456789ABCDEF",
        r"\\.\pipe\luna-core-0123456789abcdef0123456789abcdef0",
        r"\\.\pipe\other-0123456789abcdef0123456789abcdef",
        r"\\server\pipe\luna-core-0123456789abcdef0123456789abcdef",
        r"\\?\pipe\luna-core-0123456789abcdef0123456789abcdef",
        r"\\.\pipe\luna-core-0123456789abcdef\..\0123456789abcdef",
        "core-0123456789abcdef.sock",
    ]
}

#[cfg(test)]
fn handle_inheritable(h: HANDLE) -> bool {
    use windows::Win32::Foundation::{GetHandleInformation, HANDLE_FLAG_INHERIT};
    let mut flags = 0u32;
    // SAFETY: a live handle; `flags` is written once.
    unsafe { GetHandleInformation(h, &mut flags) }.expect("handle information");
    flags & HANDLE_FLAG_INHERIT.0 != 0
}

#[cfg(test)]
pub(super) fn inheritable(s: &Stream) -> bool {
    handle_inheritable(s.raw())
}

#[cfg(test)]
pub(super) fn inheritable_listener(l: &Listener) -> bool {
    let next = l.next.lock().unwrap();
    handle_inheritable(next.as_ref().expect("an instance waiting").0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A name someone else made first is not served: whoever connected to it
    /// would be talking to them, under their DACL.
    #[test]
    fn a_squatted_name_is_refused() {
        let name = fresh_name();
        let wide = wide(&name);
        // The squatter: an instance of the name with no DACL of Luna's.
        // SAFETY: a NUL-terminated name; the handle is owned from here.
        let squat = unsafe {
            CreateNamedPipeW(
                PCWSTR(wide.as_ptr()),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
                PIPE_TYPE_BYTE,
                PIPE_UNLIMITED_INSTANCES,
                0,
                0,
                0,
                None,
            )
        };
        assert!(!squat.is_invalid());
        let squat = Owned(squat);
        let err = Listener::bind(&name).err().expect("the name is taken");
        assert_eq!(err.kind(), io::ErrorKind::AddrInUse, "{err:?}");
        drop(squat);
        // Once let go of, the name is Luna's to take.
        assert!(Listener::bind(&name).is_ok());
    }

    /// While a client is connected, a new instance is always there for the
    /// next one, and it carries the first one's security.
    #[test]
    fn every_instance_has_the_first_ones_security() {
        let (l, _client, _server) = super::super::tests::pair();
        let next = l.next.lock().unwrap();
        let next = next.as_ref().expect("the next instance");
        assert_eq!(security_of(next.0).unwrap(), l.security);
        // The user as Windows writes it back: by alias where it has one, as
        // `LA` for the built-in Administrator (RID 500) that CI's runners
        // run as, else by its SID.
        let user = crate::secret::acl::trustee(&crate::secret::acl::user_sid().unwrap()).unwrap();
        let (dacl, label) = l.security.split_once("S:").expect("a label");
        assert!(crate::secret::sddl_is_private(dacl, &user), "protected, this user, SYSTEM and Administrators alone: {}", l.security);
        assert!(dacl.contains(&format!(";;;{user})")), "this user is in it: {}", l.security);
        assert!(label.contains("(ML;;NWNR;;;ME)"), "{}", l.security);
    }
}
