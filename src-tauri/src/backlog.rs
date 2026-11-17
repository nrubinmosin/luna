//! The bytes handed to a writer and not written yet, held to a bound: the one
//! reservation every bounded queue here makes (a session's input, a window's
//! queue in the core, the window's connection to the core, the page's log
//! lines). Room is taken in one step under a lock, so producers at once can
//! never take more between them than the bound lets through: a look at what
//! waits followed by an add let two of them past it together.

use std::sync::Mutex;

#[derive(Default)]
pub struct Backlog {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    bytes: u64,
    /// A refusal has been made and the writer has not caught up since
    /// (`reserve_in_order`).
    refusing: bool,
}

/// Why room was refused: what waits, and whether this refusal is the first
/// since the writer last caught up (what a log line says once).
#[derive(Debug, PartialEq)]
pub struct Refused {
    pub waiting: u64,
    pub first: bool,
}

impl Backlog {
    /// Takes room for `len` bytes under `max`: there is room when nothing
    /// waits (one item alone may be larger than the bound) or what waits and
    /// `len` together fit.
    pub fn reserve(&self, len: u64, max: u64) -> Result<(), Refused> {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if s.bytes > 0 && s.bytes + len > max {
            let first = !s.refusing;
            s.refusing = true;
            return Err(Refused { waiting: s.bytes, first });
        }
        s.refusing = false;
        s.bytes += len;
        Ok(())
    }

    /// `reserve`, and after a refusal every reservation refused until the
    /// writer has caught up with all that waits: what does go in then keeps
    /// the order it was handed over in, with no gap.
    pub fn reserve_in_order(&self, len: u64, max: u64) -> Result<(), Refused> {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if s.bytes == 0 {
            s.refusing = false;
        }
        if s.refusing || (s.bytes > 0 && s.bytes + len > max) {
            let first = !s.refusing;
            s.refusing = true;
            return Err(Refused { waiting: s.bytes, first });
        }
        s.bytes += len;
        Ok(())
    }

    /// Takes room for `len` bytes whatever waits: what the writer's own side
    /// must hand over (a terminal's answer to its CLI's query), counted with
    /// the rest.
    pub fn force(&self, len: u64) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).bytes += len;
    }

    /// Gives back room taken: `len` bytes written, or not handed over after
    /// all.
    pub fn release(&self, len: u64) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.bytes = s.bytes.saturating_sub(len);
    }

    /// What waits now.
    #[cfg(test)]
    pub fn waiting(&self) -> u64 {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Producers at once never take more room between them than the bound,
    /// whatever their interleaving; one item alone is let through when
    /// nothing waits.
    #[test]
    fn producers_at_once_never_take_more_than_the_bound() {
        for in_order in [false, true] {
            let backlog = Arc::new(Backlog::default());
            let (max, len) = (1000u64, 7u64);
            let threads: Vec<_> = (0..8)
                .map(|_| {
                    let backlog = Arc::clone(&backlog);
                    std::thread::spawn(move || {
                        let take = |b: &Backlog| if in_order { b.reserve_in_order(len, max) } else { b.reserve(len, max) };
                        (0..500).filter(|_| take(&backlog).is_ok()).count() as u64
                    })
                })
                .collect();
            let taken: u64 = threads.into_iter().map(|t| t.join().unwrap()).sum();
            assert_eq!(backlog.waiting(), taken * len);
            assert!(taken * len <= max, "{} bytes taken under {max}", taken * len);
            assert_eq!(taken, max / len, "all the room there was");
        }
        let alone = Backlog::default();
        assert!(alone.reserve(5000, 1000).is_ok(), "one item alone may be larger");
        assert_eq!(alone.reserve(1, 1000), Err(Refused { waiting: 5000, first: true }));
        assert_eq!(alone.reserve(1, 1000), Err(Refused { waiting: 5000, first: false }));
    }

    /// In order, nothing is let in after a refusal until all that waited is
    /// written; then the next reservation starts afresh.
    #[test]
    fn in_order_nothing_slips_in_behind_a_refusal() {
        let b = Backlog::default();
        b.reserve_in_order(6, 10).unwrap();
        assert!(b.reserve_in_order(6, 10).unwrap_err().first);
        b.release(3);
        assert!(b.reserve_in_order(1, 10).is_err(), "room again, but behind a refusal");
        b.release(3);
        b.reserve_in_order(1, 10).unwrap();
        b.force(100);
        assert_eq!(b.waiting(), 101, "forced room is counted");
    }
}
