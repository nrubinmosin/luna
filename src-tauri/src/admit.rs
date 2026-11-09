//! What Luna's listeners let in before they know who knocked: the core's
//! endpoint (coreproc.rs) and the hub's loopback port (hub.rs). Until a
//! connection has said who it is — a hello on the core's, a secret on the
//! hub's — it gets a slot from a fixed number of them, a deadline for what it
//! must send and a cap on its size; one past the slots is turned away at once
//! rather than queued.

use crate::endpoint::Timeouts;
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A fixed number of slots, taken without waiting.
#[derive(Clone)]
pub struct Slots {
    taken: Arc<AtomicUsize>,
    max: usize,
}

/// A slot held; dropping it gives it back.
pub struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Slots {
    pub fn new(max: usize) -> Slots {
        Slots { taken: Arc::new(AtomicUsize::new(0)), max }
    }

    /// A slot, or None when all are taken.
    pub fn take(&self) -> Option<Slot> {
        let mut n = self.taken.load(Ordering::Acquire);
        loop {
            if n >= self.max {
                return None;
            }
            match self.taken.compare_exchange_weak(n, n + 1, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return Some(Slot(self.taken.clone())),
                Err(now) => n = now,
            }
        }
    }

    #[cfg(test)]
    pub fn taken(&self) -> usize {
        self.taken.load(Ordering::Acquire)
    }
}

/// Reads from or writes to a stream (a socket, the core's endpoint) until a
/// deadline for all of it, not for each call: a peer trickling a byte at a
/// time, or reading one, cannot hold the connection open past it.
pub struct Deadline<'a, S> {
    stream: &'a S,
    until: Instant,
}

impl<'a, S> Deadline<'a, S> {
    pub fn new(stream: &'a S, within: Duration) -> Deadline<'a, S> {
        Deadline { stream, until: Instant::now() + within }
    }

    /// A new deadline, `within` from now: for what is read once the peer is
    /// known.
    pub fn extend(&mut self, within: Duration) {
        self.until = Instant::now() + within;
    }

    fn left(&self) -> io::Result<Duration> {
        let left = self.until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "deadline passed"));
        }
        Ok(left)
    }
}

impl<'a, S: Timeouts> Read for Deadline<'a, S>
where
    &'a S: Read,
{
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.stream.set_read_timeout(Some(self.left()?))?;
        let mut s = self.stream;
        s.read(buf)
    }
}

impl<'a, S: Timeouts> Write for Deadline<'a, S>
where
    &'a S: Write,
{
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stream.set_write_timeout(Some(self.left()?))?;
        let mut s = self.stream;
        s.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut s = self.stream;
        s.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::{TcpListener, TcpStream};

    #[test]
    fn slots_are_bounded_and_come_back() {
        let slots = Slots::new(2);
        let a = slots.take().expect("first");
        let b = slots.take().expect("second");
        assert!(slots.take().is_none(), "a third is turned away");
        drop(a);
        let c = slots.take().expect("a slot given back is there again");
        assert!(slots.take().is_none());
        drop((b, c));
        assert_eq!(slots.taken(), 0);
    }

    #[test]
    fn a_trickling_peer_is_cut_off_at_the_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let peer = std::thread::spawn(move || {
            let mut c = TcpStream::connect(addr).unwrap();
            // A byte every 50 ms, each well inside any per-read timeout.
            for _ in 0..40 {
                if c.write_all(b"x").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        let (s, _) = listener.accept().unwrap();
        let begun = Instant::now();
        let mut buf = [0u8; 1000];
        let err = Deadline::new(&s, Duration::from_millis(300)).read_exact(&mut buf).unwrap_err();
        assert!(matches!(err.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock), "{err:?}");
        assert!(begun.elapsed() < Duration::from_millis(1000), "{:?}", begun.elapsed());
        drop(s);
        let _ = peer.join();
    }

    /// The same on the core's endpoint, where the deadline is the overlapped
    /// read's own on Windows.
    #[test]
    fn a_trickling_peer_on_the_endpoint_is_cut_off_at_the_deadline() {
        let (_l, client, server) = crate::endpoint::tests::pair();
        let peer = std::thread::spawn(move || {
            for _ in 0..40 {
                if (&client).write_all(b"x").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        let begun = Instant::now();
        let mut buf = [0u8; 1000];
        let err = Deadline::new(&server, Duration::from_millis(300)).read_exact(&mut buf).unwrap_err();
        assert!(matches!(err.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock), "{err:?}");
        assert!(begun.elapsed() < Duration::from_millis(1000), "{:?}", begun.elapsed());
        drop(server);
        let _ = peer.join();
    }
}
