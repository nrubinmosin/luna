//! What Luna's loopback listeners let in before they know who knocked: the
//! core's (coreproc.rs) and the hub's (hub.rs). Any local process can connect
//! to either, so until a connection has shown its secret it gets a slot from a
//! fixed number of them, a deadline for what it must send and a cap on its
//! size; one past the slots is turned away at once rather than queued.

use std::io::{self, Read};
use std::net::TcpStream;
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

/// Reads from a socket until a deadline for all of it, not for each read: a
/// peer trickling a byte at a time cannot hold the connection open past it.
pub struct Deadline<'a> {
    stream: &'a TcpStream,
    until: Instant,
}

impl<'a> Deadline<'a> {
    pub fn new(stream: &'a TcpStream, within: Duration) -> Deadline<'a> {
        Deadline { stream, until: Instant::now() + within }
    }

    /// A new deadline, `within` from now: for what is read once the peer is
    /// known.
    pub fn extend(&mut self, within: Duration) {
        self.until = Instant::now() + within;
    }
}

impl Read for Deadline<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = self.until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "deadline passed"));
        }
        self.stream.set_read_timeout(Some(left))?;
        let mut s = self.stream;
        s.read(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;

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
}
