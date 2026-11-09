//! The endpoint off Windows: a Unix socket in the core folder, which is this
//! user's alone (0700, secret.rs `private_dir`), so another user cannot reach
//! it at all; who of this user's connected is the peer check's (peer.rs).

use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::time::Duration;

const PREFIX: &str = "core-";
const SUFFIX: &str = ".sock";

pub fn fresh_name() -> String {
    format!("{PREFIX}{}{SUFFIX}", crate::secret::random_hex(8))
}

/// A file name in the core folder and nothing else: no path, no `..`.
pub fn valid_name(name: &str) -> bool {
    name.strip_prefix(PREFIX)
        .and_then(|rest| rest.strip_suffix(SUFFIX))
        .is_some_and(|hex| hex.len() == 16 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
}

/// Socket files a core left behind when it did not end by leaving (a crash,
/// a kill). Called by a core that holds the lock, so none of them is served.
pub fn sweep_stale() {
    let Ok(entries) = std::fs::read_dir(crate::ipc::core_dir()) else { return };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if valid_name(&name) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

pub struct Stream(UnixStream);

impl Stream {
    /// Connects to the endpoint `name`, giving up at `within`: a core whose
    /// backlog is full, its accept loop stuck, holds no window, restart or
    /// `--stop-core` past it.
    pub fn connect(name: &str, within: Duration) -> io::Result<Stream> {
        if !valid_name(name) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a core endpoint"));
        }
        connect_within(&super::socket_path(name), within).map(Stream)
    }

    pub fn try_clone(&self) -> io::Result<Stream> {
        self.0.try_clone().map(Stream)
    }

    /// Ends the connection both ways: what waits on it returns, and the other
    /// end hears the end.
    pub fn shutdown(&self) {
        let _ = self.0.shutdown(std::net::Shutdown::Both);
    }

    pub fn set_read_timeout(&self, within: Option<Duration>) -> io::Result<()> {
        self.0.set_read_timeout(within)
    }

    pub fn set_write_timeout(&self, within: Option<Duration>) -> io::Result<()> {
        self.0.set_write_timeout(within)
    }

    /// The process serving the endpoint this end connected to: the one that
    /// called `listen` on it.
    pub fn server_pid(&self) -> io::Result<u32> {
        peer_pid(self.raw())
    }

    /// The socket, for reading who is at the other end (peer.rs).
    pub(crate) fn raw(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}

/// A stream socket, nonblocking and close-on-exec (no child inherits it).
fn nonblocking_socket() -> io::Result<OwnedFd> {
    #[cfg(target_os = "linux")]
    let kind = libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC;
    #[cfg(not(target_os = "linux"))]
    let kind = libc::SOCK_STREAM;
    // SAFETY: plain arguments; the descriptor is owned from here.
    let fd = unsafe { libc::socket(libc::AF_UNIX, kind, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a fresh descriptor of this process's.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    // macOS takes no flags in the type: set them after.
    #[cfg(not(target_os = "linux"))]
    {
        // SAFETY: fcntl on a descriptor this function owns.
        let ok = unsafe {
            libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) == 0
                && libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) | libc::O_NONBLOCK) == 0
        };
        if !ok {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(fd)
}

/// Connects to the socket at `path` by `within`. Nonblocking: a listener whose
/// backlog is full answers `EAGAIN` (Linux), and the connect is tried again
/// until the deadline; one that answers `EINPROGRESS` (BSDs) is waited on for
/// as long. Blocking again once connected, as the rest of the stream expects.
fn connect_within(path: &std::path::Path, within: Duration) -> io::Result<UnixStream> {
    use std::os::unix::ffi::OsStrExt;
    let bytes = path.as_os_str().as_bytes();
    // SAFETY: a plain C struct, all zeroes a valid value.
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if bytes.len() >= addr.sun_path.len() || bytes.contains(&0) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "the socket's path is too long for a Unix socket"));
    }
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (to, from) in addr.sun_path.iter_mut().zip(bytes) {
        *to = *from as libc::c_char;
    }
    let len = (std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1) as libc::socklen_t;
    let fd = nonblocking_socket()?;
    let deadline = std::time::Instant::now() + within;
    let timed_out = || io::Error::new(io::ErrorKind::TimedOut, "the core's endpoint took no connection in time");
    loop {
        // SAFETY: the address and its length match; the socket is this one's.
        let rc = unsafe { libc::connect(fd.as_raw_fd(), (&addr as *const libc::sockaddr_un).cast(), len) };
        if rc == 0 {
            break;
        }
        let e = io::Error::last_os_error();
        match e.raw_os_error() {
            Some(libc::EINTR) => continue,
            Some(libc::EAGAIN) => {
                let left = deadline.saturating_duration_since(std::time::Instant::now());
                if left.is_zero() {
                    return Err(timed_out());
                }
                std::thread::sleep(left.min(Duration::from_millis(10)));
            }
            Some(libc::EINPROGRESS) => {
                let left = deadline.saturating_duration_since(std::time::Instant::now());
                let mut poll = libc::pollfd { fd: fd.as_raw_fd(), events: libc::POLLOUT, revents: 0 };
                // SAFETY: one pollfd, as counted.
                let ready = unsafe { libc::poll(&mut poll, 1, left.as_millis().min(i32::MAX as u128) as libc::c_int) };
                match ready {
                    0 => return Err(timed_out()),
                    r if r < 0 => return Err(io::Error::last_os_error()),
                    _ => {}
                }
                let mut err: libc::c_int = 0;
                let mut err_len = size_of::<libc::c_int>() as libc::socklen_t;
                // SAFETY: a socket of this process; `err` and its length match.
                let got = unsafe {
                    libc::getsockopt(fd.as_raw_fd(), libc::SOL_SOCKET, libc::SO_ERROR, (&mut err as *mut libc::c_int).cast(), &mut err_len)
                };
                if got != 0 {
                    return Err(io::Error::last_os_error());
                }
                if err != 0 {
                    return Err(io::Error::from_raw_os_error(err));
                }
                break;
            }
            _ => return Err(e),
        }
    }
    // SAFETY: fcntl on a descriptor this function owns.
    let blocking = unsafe {
        let flags = libc::fcntl(fd.as_raw_fd(), libc::F_GETFL);
        flags >= 0 && libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags & !libc::O_NONBLOCK) == 0
    };
    if !blocking {
        return Err(io::Error::last_os_error());
    }
    Ok(UnixStream::from(fd))
}

/// The pid of the process at the other end of `fd`, as the kernel recorded it
/// at connect (or listen, for a client asking after its server).
#[cfg(target_os = "linux")]
pub(crate) fn peer_pid(fd: RawFd) -> io::Result<u32> {
    let cred = peer_cred(fd)?;
    u32::try_from(cred.pid).ok().filter(|&p| p != 0).ok_or_else(|| io::Error::other("the peer's pid is not visible here"))
}

#[cfg(target_os = "linux")]
pub(crate) fn peer_cred(fd: RawFd) -> io::Result<libc::ucred> {
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: a socket of this process; `cred` and `len` match.
    let rc = unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_PEERCRED, (&mut cred as *mut libc::ucred).cast(), &mut len) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(cred)
}

/// macOS keeps the pid apart from the credentials (`LOCAL_PEERPID`).
#[cfg(not(target_os = "linux"))]
pub(crate) fn peer_pid(fd: RawFd) -> io::Result<u32> {
    let mut pid: libc::pid_t = 0;
    let mut len = size_of::<libc::pid_t>() as libc::socklen_t;
    // SAFETY: a socket of this process; `pid` and `len` match.
    let rc = unsafe { libc::getsockopt(fd, libc::SOL_LOCAL, libc::LOCAL_PEERPID, (&mut pid as *mut libc::pid_t).cast(), &mut len) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    u32::try_from(pid).ok().filter(|&p| p != 0).ok_or_else(|| io::Error::other("the peer's pid is not visible here"))
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (&self.0).read(buf)
    }
}

impl Read for &Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (&self.0).read(buf)
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (&self.0).write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        (&self.0).flush()
    }
}

impl Write for &Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (&self.0).write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        (&self.0).flush()
    }
}

pub struct Listener {
    listener: UnixListener,
    path: PathBuf,
}

impl Listener {
    /// Serves `name` in the core folder, refusing it when anything is there
    /// already: a fresh name is never there unless someone put it there.
    pub fn bind(name: &str) -> io::Result<Listener> {
        if !valid_name(name) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a core endpoint"));
        }
        let path = super::socket_path(name);
        if std::fs::symlink_metadata(&path).is_ok() {
            return Err(io::Error::new(io::ErrorKind::AddrInUse, format!("{} is there already", path.display())));
        }
        let listener = UnixListener::bind(&path)?;
        // The folder keeps everyone else out already; the socket too.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        Ok(Listener { listener, path })
    }

    pub fn accept(&self) -> io::Result<Stream> {
        self.listener.accept().map(|(s, _)| Stream(s))
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
pub(super) fn test_name() -> String {
    let _ = std::fs::create_dir_all(crate::ipc::core_dir());
    fresh_name()
}

#[cfg(test)]
pub(super) fn invalid_names() -> Vec<&'static str> {
    vec![
        "",
        "core-.sock",
        "core-0123456789ABCDEF.sock",
        "core-0123456789abcdef0.sock",
        "../core-0123456789abcdef.sock",
        "/tmp/core-0123456789abcdef.sock",
        "core-0123456789abcdef.sock/x",
        r"\\.\pipe\luna-core-0123456789abcdef0123456789abcdef",
    ]
}

#[cfg(test)]
fn fd_inheritable(fd: RawFd) -> bool {
    // SAFETY: a descriptor of this process; F_GETFD reads its flags only.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    flags >= 0 && flags & libc::FD_CLOEXEC == 0
}

#[cfg(test)]
pub(super) fn inheritable(s: &Stream) -> bool {
    fd_inheritable(s.raw())
}

#[cfg(test)]
pub(super) fn inheritable_listener(l: &Listener) -> bool {
    fd_inheritable(l.listener.as_raw_fd())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A name someone put a file at first is not served.
    #[test]
    fn a_squatted_name_is_refused() {
        let name = test_name();
        let path = super::super::socket_path(&name);
        std::fs::write(&path, b"").unwrap();
        let err = Listener::bind(&name).err().expect("the name is taken");
        assert_eq!(err.kind(), io::ErrorKind::AddrInUse, "{err:?}");
        std::fs::remove_file(&path).unwrap();
        let l = Listener::bind(&name).expect("free again");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        drop(l);
        assert!(std::fs::symlink_metadata(&path).is_err(), "the socket goes with the listener");
    }

    /// A core that takes no connections, its backlog full and its accept loop
    /// stuck, does not hold a window's (or `--stop-core`'s) connect past the
    /// deadline the caller gave.
    #[test]
    fn a_connect_to_a_full_backlog_gives_up_at_its_deadline() {
        let name = test_name();
        let path = super::super::socket_path(&name);
        let listener = UnixListener::bind(&path).unwrap();
        // Room for no more than the one connection waiting, which this takes.
        // SAFETY: listen again on a listening socket of this process only
        // changes its backlog.
        assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 0) }, 0);
        let _waiting = UnixStream::connect(&path).expect("the one the backlog holds");
        let (tx, rx) = std::sync::mpsc::channel();
        let connecting = name.clone();
        std::thread::spawn(move || {
            let begun = std::time::Instant::now();
            let got = Stream::connect(&connecting, Duration::from_millis(300)).map(drop);
            let _ = tx.send((got, begun.elapsed()));
        });
        let (got, took) = rx.recv_timeout(Duration::from_secs(5)).expect("the connect came back at all");
        assert_eq!(got.err().map(|e| e.kind()), Some(io::ErrorKind::TimedOut));
        assert!(took < Duration::from_secs(2), "{took:?}");
        drop(listener);
        let _ = std::fs::remove_file(&path);
    }
}
