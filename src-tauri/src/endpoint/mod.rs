//! The core's own door (coreproc.rs, corelink.rs): a local endpoint that only
//! this user's processes can open, and that tells the core who opened it.
//!
//! No secret stands behind it. What lets a process in is what the OS says of
//! it as it connects (peer.rs): this user, not confined by a sandbox, of the
//! core's integrity or above. A file Luna writes could be read by anything of
//! this user's, a CLI's sandbox included, and so cannot be the key
//! ([ADR-0021](../../../docs/adr/0021-core-endpoint-by-os-identity.md)).
//!
//! - Windows: a named pipe, `\\.\pipe\luna-core-<random>`, whose DACL lets in
//!   this user, SYSTEM and Administrators only, under a medium integrity label
//!   that keeps lower ones from opening it at all. Remote clients are refused,
//!   and the first instance is made with `FILE_FLAG_FIRST_PIPE_INSTANCE`, so a
//!   name someone else holds is never served. Every handle is overlapped: a
//!   synchronous pipe handle makes a write wait behind a read pending on it,
//!   and the core reads and writes each connection at once.
//! - Unix: a socket in the core folder, which is this user's alone (0700),
//!   named in the discovery file by its file name only.
//!
//! The window finds the endpoint's name in `core.json` and checks that the
//! process serving it is the core that file names before it says anything.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

#[cfg_attr(windows, path = "win.rs")]
#[cfg_attr(not(windows), path = "unix.rs")]
mod imp;

#[cfg(target_os = "linux")]
pub(crate) use imp::peer_cred;
#[cfg(all(unix, not(target_os = "linux")))]
pub(crate) use imp::peer_pid;
#[cfg(unix)]
pub(crate) use imp::sweep_stale;
pub use imp::{Listener, Stream};

/// What a core listens on, new for each core: a pipe name on Windows, a file
/// name in the core folder elsewhere. Another core of another data dir (the
/// test build beside the installed one) listens on another.
pub fn fresh_name() -> String {
    imp::fresh_name()
}

/// The socket file a Unix endpoint's name stands for: in the core folder.
#[cfg_attr(windows, allow(dead_code))]
pub fn socket_path(name: &str) -> PathBuf {
    crate::ipc::core_dir().join(name)
}

/// What a deadline sets on a stream before each read or write (admit.rs).
pub trait Timeouts {
    fn set_read_timeout(&self, within: Option<Duration>) -> io::Result<()>;
    fn set_write_timeout(&self, within: Option<Duration>) -> io::Result<()>;
}

impl Timeouts for std::net::TcpStream {
    fn set_read_timeout(&self, within: Option<Duration>) -> io::Result<()> {
        std::net::TcpStream::set_read_timeout(self, within)
    }
    fn set_write_timeout(&self, within: Option<Duration>) -> io::Result<()> {
        std::net::TcpStream::set_write_timeout(self, within)
    }
}

impl Timeouts for Stream {
    fn set_read_timeout(&self, within: Option<Duration>) -> io::Result<()> {
        Stream::set_read_timeout(self, within)
    }
    fn set_write_timeout(&self, within: Option<Duration>) -> io::Result<()> {
        Stream::set_write_timeout(self, within)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::time::Instant;

    /// A listener on a fresh name and a client connected to it, with the
    /// server's end: what most tests here start from. The listener is kept so
    /// the name stays served.
    pub(crate) fn pair() -> (Listener, Stream, Stream) {
        let name = test_name();
        let listener = Listener::bind(&name).expect("bind");
        let client = std::thread::spawn(move || Stream::connect(&name, Duration::from_secs(5)).expect("connect"));
        let server = listener.accept().expect("accept");
        (listener, client.join().unwrap(), server)
    }

    /// A fresh name for a test. On Unix in a folder of the test's own, as the
    /// core folder may be shared by tests running at once.
    pub(crate) fn test_name() -> String {
        imp::test_name()
    }

    #[test]
    fn bytes_go_both_ways() {
        let (_l, mut client, mut server) = pair();
        client.write_all(b"ping").unwrap();
        let mut got = [0u8; 4];
        server.read_exact(&mut got).unwrap();
        assert_eq!(&got, b"ping");
        server.write_all(b"pong").unwrap();
        client.read_exact(&mut got).unwrap();
        assert_eq!(&got, b"pong");
        drop(client);
        assert_eq!(server.read(&mut got).unwrap(), 0, "the other end gone reads as the end");
    }

    /// A read waiting on one thread does not hold up a write on another, at
    /// either end: the core writes events to a window while it waits for its
    /// next request. With a synchronous pipe handle this would hang.
    #[test]
    fn a_pending_read_does_not_block_a_write() {
        let (_l, client, server) = pair();
        let reader = server.try_clone().unwrap();
        let waiting = std::thread::spawn(move || {
            let mut buf = [0u8; 1];
            (&reader).read_exact(&mut buf).map(|()| buf[0])
        });
        std::thread::sleep(Duration::from_millis(100));
        let (tx, rx) = std::sync::mpsc::channel();
        let writer = server.try_clone().unwrap();
        std::thread::spawn(move || {
            let _ = tx.send((&writer).write_all(b"event"));
        });
        assert!(rx.recv_timeout(Duration::from_secs(5)).expect("the write did not wait for the read").is_ok());
        let mut got = [0u8; 5];
        (&client).read_exact(&mut got).unwrap();
        assert_eq!(&got, b"event");
        (&client).write_all(b"x").unwrap();
        assert_eq!(waiting.join().unwrap().unwrap(), b'x');
    }

    #[test]
    fn a_read_gives_up_at_its_timeout() {
        let (_l, _client, server) = pair();
        server.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
        let begun = Instant::now();
        let err = (&server).read(&mut [0u8; 8]).unwrap_err();
        assert!(matches!(err.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock), "{err:?}");
        assert!(begun.elapsed() < Duration::from_secs(2), "{:?}", begun.elapsed());
        // And the stream still works after.
        server.set_read_timeout(None).unwrap();
        (&_client).write_all(b"y").unwrap();
        let mut b = [0u8; 1];
        (&server).read_exact(&mut b).unwrap();
        assert_eq!(&b, b"y");
    }

    /// `shutdown` ends a read and a write that are waiting, on other threads:
    /// how the core lets go of a window that stopped reading (coreproc.rs).
    #[test]
    fn shutdown_ends_what_is_waiting_and_the_other_end_hears_it() {
        let (_l, client, server) = pair();
        let reader = server.try_clone().unwrap();
        let read = std::thread::spawn(move || (&reader).read(&mut [0u8; 8]));
        // A writer the client never reads from fills the pipe and waits.
        let writer = server.try_clone().unwrap();
        let wrote = std::thread::spawn(move || {
            let chunk = vec![0u8; 64 * 1024];
            loop {
                if let Err(e) = (&writer).write_all(&chunk) {
                    return e;
                }
            }
        });
        std::thread::sleep(Duration::from_millis(300));
        let begun = Instant::now();
        server.shutdown();
        assert!(matches!(read.join().unwrap(), Ok(0) | Err(_)), "the waiting read ended");
        let _ = wrote.join().unwrap();
        assert!(begun.elapsed() < Duration::from_secs(5), "{:?}", begun.elapsed());
        // The client drains what was sent and then hears the end.
        client.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut sink = Vec::new();
        let end = (&client).read_to_end(&mut sink);
        assert!(end.is_ok() || end.as_ref().is_err_and(|e| e.kind() != io::ErrorKind::TimedOut), "{end:?}");
    }

    /// A client of a name nobody serves is told so at once.
    #[test]
    fn nobody_at_the_name_is_not_found() {
        let name = test_name();
        let begun = Instant::now();
        let err = Stream::connect(&name, Duration::from_secs(2)).err().expect("nobody there");
        assert_eq!(err.kind(), io::ErrorKind::NotFound, "{err:?}");
        assert!(begun.elapsed() < Duration::from_secs(1));
    }

    /// The client learns which process serves the endpoint: corelink.rs
    /// refuses one that is not the core `core.json` names.
    #[test]
    fn the_client_sees_the_servers_pid() {
        let (_l, client, _server) = pair();
        assert_eq!(client.server_pid().unwrap(), std::process::id());
    }

    /// A discovery file names nothing but what `fresh_name` makes: a name
    /// pointing anywhere else (a remote pipe, a path out of the core folder)
    /// is refused before anything is opened, by the client and the server.
    #[test]
    fn names_are_fresh_and_nothing_else_is_opened() {
        let (a, b) = (fresh_name(), fresh_name());
        assert_ne!(a, b);
        assert!(imp::valid_name(&a) && imp::valid_name(&b), "{a} {b}");
        for bad in imp::invalid_names() {
            assert!(!imp::valid_name(bad), "{bad:?} is not a name a core makes");
            let err = Stream::connect(bad, Duration::from_secs(1)).err().expect("not opened");
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "{bad:?}");
            assert_eq!(Listener::bind(bad).err().map(|e| e.kind()), Some(io::ErrorKind::InvalidInput), "{bad:?}");
        }
    }

    /// Neither end's handle is passed on to a process the core or a window
    /// starts: a CLI session holding one would hold a way into the core.
    #[test]
    fn no_child_inherits_either_end() {
        let (l, client, server) = pair();
        assert!(!imp::inheritable_listener(&l), "the listener");
        assert!(!imp::inheritable(&client), "the client's end");
        assert!(!imp::inheritable(&server), "the server's end");
    }
}
