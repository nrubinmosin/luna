//! What a session's turns left running past their end, as last read, and when
//! it was last at work: what `luna_session_list` names, and what holds the machine.

use super::*;

/// A process a session's turns left running past their end — a helper's in its
/// job, a chat's under its CLI — as last read, and when it was last at work.
/// One model for both: what `luna_session_list` names, and what holds the machine.
#[derive(Clone, Debug, PartialEq)]
pub struct Leftover {
    pub pid: u32,
    pub exe: String,
    pub started_ms: u64,
    /// Its counters as of `read_ms`, which its rate is next taken from.
    pub cpu_ms: u64,
    pub io_bytes: u64,
    /// When the counters were read: kept for LEFTOVER_WINDOW_MS, so a rate is
    /// taken over that long.
    pub read_ms: u64,
    /// When it was last looked at, every sample.
    pub seen_ms: u64,
    /// When it was last at work, or since when the job it came into was quiet.
    pub moved_ms: u64,
}

/// A leftover is at work while it uses at least this much CPU per second of
/// the time between two reads (1% of a core), or reads and writes
/// LEFTOVER_IO_PER_S bytes. Measured on Windows over a minute or less: a
/// `tail -F` polling its file used 0.5 ms a second and read 4 bytes, a Git
/// Bash `grep` waiting on its pipe 1.5 ms and none, a `ping` 1 ms; a slow
/// download of 8 KiB a second, a compiler or a test run is past one or both.
pub const LEFTOVER_CPU_MS_PER_S: u64 = 10;
pub const LEFTOVER_IO_PER_S: u64 = 1024;
/// The shortest stretch a rate is taken over; a read sooner after the last is
/// measured with a later one. A minute, not a sample: Windows counts CPU time
/// in ticks of 15.6 ms, and a Git Bash loop forking a `grep` and a `sleep` every
/// ten seconds, 1 ms a second on average, now and then passed a rate over one
/// five-second sample — each time its LEFTOVER_HOLD_MS started over, and a
/// dozen loops waiting for lines that never came held a PC up all night.
pub const LEFTOVER_WINDOW_MS: u64 = 60_000;
/// A process first seen younger than this is judged by its CPU since it
/// started over this long: a start's own CPU (a `sleep` loading its DLLs, 16 ms
/// and 20 KB read in its first 100 ms) is no work, nor its I/O then. Seen in
/// the middle of its start, the rest of it is spread over the window after.
const NEWBORN_WINDOW_MS: u64 = 30_000;
/// At work this recently: what `luna_session_list` calls `active`.
pub const LEFTOVER_ACTIVE_MS: u64 = 10 * 60 * 1000;
/// A leftover holds the machine awake until it has done nothing for this long
/// on end, then lets go, and the log says so; any work starts it over. Doing
/// nothing is no proof a job is over — a Docker or remote client waits on work
/// done elsewhere, a slow download dips under any rate — so this is long, but
/// it ends: a forgotten `tail -F` once kept a PC up all night.
pub const LEFTOVER_HOLD_MS: u64 = 2 * 60 * 60 * 1000;

/// Each process of `now` with when it was last at work, by the rates it moved at
/// since `old` read it. Birth is no work: one not in `old` is as quiet as the job
/// it came into (the latest any of `old` was at work, or `last_work`, its turn's
/// end), unless its own counters since it started show work — so a loop that
/// starts a `sleep 60` every minute stays as idle as it is.
pub fn leftovers(old: &[Leftover], now: Vec<(ProcRead, String)>, at: u64, last_work: u64) -> Vec<Leftover> {
    let quiet_since = old.iter().map(|o| o.moved_ms).max().unwrap_or(0).max(last_work);
    let working = |cpu: u64, io: u64, ms: u64| {
        cpu.saturating_mul(1000) >= LEFTOVER_CPU_MS_PER_S * ms || io.saturating_mul(1000) >= LEFTOVER_IO_PER_S * ms
    };
    now.into_iter()
        .map(|(r, exe)| {
            let lived = at.saturating_sub(r.started_ms);
            let moved_ms = match old.iter().find(|o| o.pid == r.pid && o.started_ms == r.started_ms) {
                Some(o) if at.saturating_sub(o.read_ms) < LEFTOVER_WINDOW_MS => return Leftover { exe, seen_ms: at, ..o.clone() },
                Some(o) => {
                    let ms = at.saturating_sub(o.read_ms).max(1);
                    if working(r.cpu_ms.saturating_sub(o.cpu_ms), r.io_bytes.saturating_sub(o.io_bytes), ms) {
                        at
                    } else {
                        o.moved_ms
                    }
                }
                None if working(r.cpu_ms, 0, lived.max(NEWBORN_WINDOW_MS)) => at,
                None => quiet_since,
            };
            Leftover {
                pid: r.pid,
                exe,
                started_ms: r.started_ms,
                cpu_ms: r.cpu_ms,
                io_bytes: r.io_bytes,
                read_ms: at,
                seen_ms: at,
                moved_ms,
            }
        })
        .collect()
}

impl Leftover {
    /// At work within LEFTOVER_ACTIVE_MS.
    pub fn active(&self, now: u64) -> bool {
        now.saturating_sub(self.moved_ms) < LEFTOVER_ACTIVE_MS
    }

    /// Not idle for LEFTOVER_HOLD_MS on end: it holds the machine.
    pub fn holds(&self, now: u64) -> bool {
        now.saturating_sub(self.moved_ms) < LEFTOVER_HOLD_MS
    }

    /// "tail.exe (pid 22104)".
    pub fn label(&self) -> String {
        format!("{} (pid {})", self.exe, self.pid)
    }
}

/// Those of `new` that have let the machine go since `old` was looked at, idle
/// for LEFTOVER_HOLD_MS on end: for the log to say so.
pub fn gone_quiet(old: &[Leftover], new: &[Leftover], now: u64) -> Vec<String> {
    new.iter()
        .filter(|l| !l.holds(now) && old.iter().any(|o| o.pid == l.pid && o.started_ms == l.started_ms && o.holds(o.seen_ms)))
        .map(Leftover::label)
        .collect()
}

/// How long all of these have done nothing, the least of them; None for none.
pub fn quiet_ms(left: &[Leftover], now: u64) -> Option<u64> {
    left.iter().map(|l| now.saturating_sub(l.moved_ms)).min()
}

/// A few of them by `label`, for a notice.
pub fn left_names(left: &[Leftover]) -> String {
    const SHOWN: usize = 3;
    let mut names: Vec<String> = left.iter().take(SHOWN).map(Leftover::label).collect();
    if left.len() > SHOWN {
        names.push(format!("{} more", left.len() - SHOWN));
    }
    names.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rates a leftover counts as at work by, on figures as Windows gave them
    /// for a `tail -F` and a `ping` left running, and for real work; and birth is
    /// no work.
    #[test]
    fn a_leftover_is_at_work_by_what_it_uses_not_by_being_there() {
        let read = |pid: u32, cpu_ms: u64, io_bytes: u64| (ProcRead { pid, cpu_ms, io_bytes, started_ms: 7 }, format!("p{pid}"));
        // Just left by a turn that ended at 900: as quiet as the job since then.
        let first = leftovers(&[], vec![read(1, 16, 20_000), read(2, 31, 19_000), read(3, 50, 0), read(4, 0, 0)], 1_000, 900);
        assert!(first.iter().all(|l| l.moved_ms == 900), "{first:?}");
        let old = first.iter().map(|l| Leftover { moved_ms: 0, ..l.clone() }).collect::<Vec<_>>();
        // A minute on: a `tail -F`, a `ping`, a compiler, a download at 8 KiB/s.
        let next = leftovers(&old, vec![read(1, 46, 35_000), read(2, 91, 25_000), read(3, 30_000, 0), read(4, 0, 480_000)], 61_000, 900);
        let moved: Vec<bool> = next.iter().map(|l| l.moved_ms == 61_000).collect();
        assert_eq!(moved, [false, false, true, true], "tail -F, ping: idle; a compiler, a download: at work");
        // A second on, one tick of CPU is no rate yet: measured with the next.
        let soon = leftovers(&old, vec![read(1, 32, 20_000)], 2_000, 900);
        assert_eq!((soon[0].moved_ms, soon[0].read_ms, soon[0].cpu_ms), (0, 1_000, 16));
        // A child that takes another's place is as quiet as the job was: a loop's
        // `sleep 60`, its start's CPU and I/O loading, not work...
        let now = 10 * 60_000;
        let sleep = (ProcRead { pid: 9, cpu_ms: 31, io_bytes: 30_000, started_ms: now - 2_000 }, "sleep.exe".to_string());
        let quiet = leftovers(&old, vec![sleep.clone()], now, 900);
        assert_eq!(quiet[0].moved_ms, 900, "birth is no work");
        // Seen in the middle of its start, the rest of its loading is no work
        // either: it is taken over the window after.
        let mid = |cpu_ms: u64, io_bytes: u64| (ProcRead { pid: 11, cpu_ms, io_bytes, started_ms: now - 50 }, "sleep.exe".to_string());
        let seen = leftovers(&old, vec![mid(15, 2_000)], now, 900);
        let next_read = leftovers(&seen, vec![mid(31, 30_000)], now + 5_000, 900);
        assert_eq!((next_read[0].read_ms, next_read[0].seen_ms), (now, now + 5_000), "too soon for a rate");
        let window_on = leftovers(&next_read, vec![mid(31, 30_000)], now + LEFTOVER_WINDOW_MS, 900);
        assert_eq!((window_on[0].moved_ms, window_on[0].read_ms), (900, now + LEFTOVER_WINDOW_MS), "{window_on:?}");
        // ...but one that has used a core since it started is.
        let busy = (ProcRead { pid: 10, cpu_ms: 2_500, io_bytes: 0, started_ms: now - 3_000 }, "rustc.exe".to_string());
        assert_eq!(leftovers(&old, vec![busy], now, 900)[0].moved_ms, now);
        // A pid taken by another process is a new one.
        let reborn = leftovers(&old, vec![(ProcRead { pid: 1, cpu_ms: 31, io_bytes: 21_400, started_ms: 8 }, "p1".into())], 6_000, 900);
        assert_eq!(reborn[0].moved_ms, 900);
        assert_eq!(left_names(&next), "p1 (pid 1), p2 (pid 2), p3 (pid 3), 1 more");
        // Active for luna_session_list, holding for the machine, and letting go once.
        let t = 61_000;
        assert!(next[2].active(t + LEFTOVER_ACTIVE_MS - 1) && !next[2].active(t + LEFTOVER_ACTIVE_MS));
        assert!(next[2].holds(t + LEFTOVER_HOLD_MS - 1) && !next[2].holds(t + LEFTOVER_HOLD_MS));
        let later = t + LEFTOVER_HOLD_MS;
        let after = leftovers(&next, vec![read(3, 30_000, 0)], later, 900);
        assert_eq!(gone_quiet(&next, &after, later), ["p3 (pid 3)"]);
        assert!(
            gone_quiet(&after, &leftovers(&after, vec![read(3, 30_000, 0)], later + 5_000, 900), later + 5_000).is_empty(),
            "said once"
        );
        assert_eq!(quiet_ms(&after, later), Some(LEFTOVER_HOLD_MS));
    }

    /// A Git Bash `grep` left waiting on a pipe from `tail -f`, read every five
    /// seconds as Windows gave its figures: a tick of CPU every ten seconds,
    /// nothing read or written (its polling is "other" I/O, `imp::io_bytes`).
    /// It holds the machine for LEFTOVER_HOLD_MS after its turn, then lets go,
    /// once, and nothing it does in between is work.
    #[test]
    fn a_grep_waiting_on_a_pipe_lets_go() {
        let turn_end = 900;
        let read = |cpu_ms: u64| vec![(ProcRead { pid: 5, cpu_ms, io_bytes: 0, started_ms: 100 }, "grep.exe".to_string())];
        let mut left = leftovers(&[], read(16), 1_000, turn_end);
        let mut said = Vec::new();
        let mut cpu = 16;
        let end = turn_end + LEFTOVER_HOLD_MS + 10 * 60_000;
        let mut at = 1_000;
        for i in 1.. {
            at += 5_000;
            if at >= end {
                break;
            }
            if i % 2 == 0 {
                cpu += 16;
            }
            let next = leftovers(&left, read(cpu), at, turn_end);
            said.extend(gone_quiet(&left, &next, at).into_iter().map(|g| (at, g)));
            left = next;
            assert_eq!(left[0].moved_ms, turn_end, "at {at}: no work");
            assert_eq!(left[0].holds(at), at - turn_end < LEFTOVER_HOLD_MS, "at {at}");
        }
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(said[0].0 - turn_end >= LEFTOVER_HOLD_MS && said[0].0 - turn_end < LEFTOVER_HOLD_MS + 5_000);
    }
}
