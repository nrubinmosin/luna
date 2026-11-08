//! The processes a session's CLI has under it, and whether they are doing
//! anything.
//!
//! A CLI that has finished its turn can still have work running in the shell:
//! a background build, a test run, a `sleep` it means to come back to. The
//! registry says idle, the screen is quiet, and the only trace is the process
//! tree. On Windows the tree is read off a Job Object each session is put in
//! at spawn — children inherit it, so `QueryInformationJobObject` lists them
//! all without walking parent pids, which pid reuse makes unreliable. The
//! walk stays as the fallback for a CLI that could not be assigned.
//!
//! A chat's job sets no limits: its CLI goes with its terminal. What is left in
//! a job when a session is done with is ended by `end_leftovers`, which the
//! CLI's own tree kill misses: a process whose parent had already gone. A
//! program with a window of its own — a browser a login opened, an editor — is
//! the user's, not the session's work, and is neither counted nor ended.
//!
//! A background helper has no terminal to go with, so its job is tied to the
//! core (`tie`): the OS ends what is in it when the core's handle closes, as it
//! does however the core dies, and a CLI does not run on unowned on a
//! conversation the next core may resume. That takes windowed programs too,
//! which the OS will not except, so every way out that is not a death lets go
//! of the tie first (`let_go`, and dropping a job), and `end_leftovers` spares
//! them as ever. Off Windows a process stands in for the OS, with the same ways
//! in and out (keeper/). There a job lists nothing: no window can be told from
//! a job left running, so nothing is counted, and what a turn left runs on past
//! every way out but a death.
//!
//! This module holds a CLI's children as read at a sample and what is busy
//! among them. The rest is split by what it does:
//!
//! - `imp`: what each OS gives — the Job Object and its members, process
//!   reads, ends and windows on Windows (win.rs), the keeper's tie off it
//!   (unix.rs).
//! - `leftovers`: what a session's turns left running past their end, and
//!   when it is at work.
//! - `testkit`: real processes for tests across the crate (Windows).

#[cfg_attr(windows, path = "win.rs")]
#[cfg_attr(not(windows), path = "unix.rs")]
mod imp;
mod leftovers;
#[cfg(all(test, windows))]
pub(crate) mod testkit;

pub use leftovers::*;

/// A core's death, for a test of what a tie does to a job across the crate.
#[cfg(all(test, windows))]
pub use imp::close_as_in_a_death;
#[cfg(windows)]
pub use imp::resume;
pub use imp::{adopt, alive, also_marked, await_environment, end_last_ties, join, keeper_from_args, let_go, pin, tie, Job, Pin};

/// When a process was created, epoch ms, where the platform says.
pub fn created_ms(pid: u32) -> Option<u64> {
    imp::read(pid).map(|r| r.started_ms)
}

/// One process, as counted at one sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProcRead {
    pub pid: u32,
    /// Kernel + user time, cumulative.
    pub cpu_ms: u64,
    /// Read + write transfer, cumulative (`imp::io_bytes`).
    pub io_bytes: u64,
    pub started_ms: u64,
}

/// The children of a session's CLI right now — the CLI itself left out, since
/// its own idle repaint would count as work, and so is every program with a
/// window of its own (`shown`).
pub fn children(job: Option<&Job>, root: Option<u32>) -> Vec<ProcRead> {
    let pids = match (job, root) {
        (Some(j), _) => imp::members(j),
        (None, Some(pid)) => imp::descendants(pid),
        (None, None) => Vec::new(),
    };
    let reads: Vec<ProcRead> = pids.into_iter().filter(|p| Some(*p) != root).filter_map(imp::read).collect();
    if reads.is_empty() {
        return reads;
    }
    let shown = shown(&reads, &imp::windowed(), &imp::parents());
    reads.into_iter().filter(|r| !shown.contains(&r.pid)).collect()
}

/// Of `reads`, the ones that own a visible window and every one below them:
/// a browser's tab and GPU processes have no window, and go with the browser.
/// A child counts as below a parent only if it was born after it, since a
/// parent pid may since have been taken by another process.
fn shown(reads: &[ProcRead], windowed: &std::collections::HashSet<u32>, edges: &[(u32, u32)]) -> std::collections::HashSet<u32> {
    let born = |pid: u32| reads.iter().find(|r| r.pid == pid).map(|r| r.started_ms);
    let mut out: std::collections::HashSet<u32> = reads.iter().map(|r| r.pid).filter(|p| windowed.contains(p)).collect();
    loop {
        let more: Vec<u32> = edges
            .iter()
            .filter(|(pid, ppid)| {
                !out.contains(pid) && out.contains(ppid) && matches!((born(*pid), born(*ppid)), (Some(c), Some(p)) if c >= p)
            })
            .map(|(pid, _)| *pid)
            .collect();
        if more.is_empty() {
            return out;
        }
        out.extend(more);
    }
}

/// What is left running in a helper's job between its turns, each process with
/// its counters and name, for telling what is at work from what only sits there:
/// every process, as each was born of a turn, but the CLIs' own helpers
/// (`is_furniture`) and programs with a window of their own.
pub fn left_in(job: &Job) -> Vec<(ProcRead, String)> {
    let reads = children(Some(job), None);
    let names = imp::names(&reads.iter().map(|r| r.pid).collect::<Vec<_>>());
    // Names come from a snapshot of every process: one not in it has gone since
    // the job was read (a launcher on its way out).
    reads
        .into_iter()
        .filter_map(|r| Some((r, names.iter().find(|(p, _)| *p == r.pid)?.1.clone())))
        .filter(|(_, name)| !is_furniture(name))
        .collect()
}

/// Ends what is left in a job once its session or helper is done with: what
/// the CLI's tree kill missed, a process whose parent had gone first. Not
/// `keep` (the CLI, which its own kill takes), and not a program with a
/// window of its own (`children`). Returns the names of those it ended.
pub fn end_leftovers(job: &Job, keep: Option<u32>) -> Vec<String> {
    end_all(&children(Some(job), keep))
}

/// `end_leftovers` for what a turn that came to nothing started of its own:
/// what in the job descends from its CLI (`cli`, started at `since_ms`) through
/// parents still there, each born no earlier than `since_ms` nor than the child
/// it is taken for the parent of, so a reused pid is no parent. Only that: a
/// process whose way up breaks (a launcher that has gone) is no proof of whose
/// it is and is spared, as is all an earlier turn left and what that starts.
pub fn end_tree_of(job: &Job, cli: u32, since_ms: u64) -> Vec<String> {
    let reads = children(Some(job), None);
    let edges = imp::parents();
    let parent_of = |pid: u32| edges.iter().find(|(p, _)| *p == pid).map(|(_, ppid)| *ppid);
    let of_cli = |r: &ProcRead| {
        if r.started_ms < since_ms {
            return false;
        }
        let mut at = *r;
        for _ in 0..32 {
            let Some(ppid) = parent_of(at.pid) else { return false };
            if ppid == cli {
                return true;
            }
            match reads.iter().find(|p| p.pid == ppid && p.started_ms >= since_ms && p.started_ms <= at.started_ms) {
                Some(p) => at = *p,
                None => return false,
            }
        }
        false
    };
    let its: Vec<ProcRead> = reads.iter().filter(|r| r.pid != cli && of_cli(r)).copied().collect();
    end_all(&its)
}

fn end_all(left: &[ProcRead]) -> Vec<String> {
    let names = imp::names(&left.iter().map(|r| r.pid).collect::<Vec<_>>());
    left.iter()
        .filter(|r| imp::end(r.pid, r.started_ms))
        .map(|r| names.iter().find(|(p, _)| *p == r.pid).map_or_else(|| r.pid.to_string(), |(_, n)| n.clone()))
        .collect()
}

pub fn names(pids: &[u32]) -> Vec<(u32, String)> {
    imp::names(pids)
}

/// Why a set of children counts as busy, if it does.
#[derive(Clone, Debug, PartialEq)]
pub struct Busy {
    /// Pids that moved: CPU time or IO volume grew since the last sample.
    pub active: Vec<u32>,
    /// Pids that started after the turn did and are still there — a job the
    /// CLI put in the background and means to come back to, even if it is
    /// only sleeping.
    pub young: Vec<u32>,
}

/// Compares two samples of the same session's children. `born_after_ms` is
/// when the CLI began its first turn (0 = never): a process born after that
/// is the model's doing — a background job it means to come back to — and is
/// `young` while it lives, however long after its turn: its session's job is
/// not over. Whether it holds the machine is its `Leftover`'s to say, by what
/// it does. The MCP servers and shells that came up with the session are
/// furniture and only count while they move (`min_cpu_ms`, `min_io_bytes` since
/// the last sample), as do the CLIs' own helpers (`without_furniture`).
pub fn judge(prev: &[ProcRead], now: &[ProcRead], born_after_ms: u64, min_cpu_ms: u64, min_io_bytes: u64) -> Option<Busy> {
    let mut active = Vec::new();
    let mut young = Vec::new();
    for p in now {
        if let Some(q) = prev.iter().find(|q| q.pid == p.pid && q.started_ms == p.started_ms) {
            if p.cpu_ms.saturating_sub(q.cpu_ms) >= min_cpu_ms || p.io_bytes.saturating_sub(q.io_bytes) >= min_io_bytes {
                active.push(p.pid);
            }
        }
        if born_after_ms > 0 && p.started_ms >= born_after_ms {
            young.push(p.pid);
        }
    }
    (!active.is_empty() || !young.is_empty()).then_some(Busy { active, young })
}

/// Helpers a CLI starts by itself the first time a turn needs one and keeps
/// for the rest of the session. Born of a turn, they read as a job the model
/// left running, and a Codex helper whose turn was over was announced as
/// "NOT done yet" with nothing to come. They are furniture: busy only while
/// they move. A console host is never the job itself: a job that runs on
/// in one has its own process alongside to count, and the code-mode host
/// comes with one of its own.
const CLI_FURNITURE: &[&str] = &["codex-code-mode-host", "conhost"];

fn is_furniture(exe: &str) -> bool {
    let exe = exe.to_ascii_lowercase();
    let stem = exe.strip_suffix(".exe").unwrap_or(&exe);
    CLI_FURNITURE.contains(&stem)
}

/// `judge`'s verdict with the CLIs' own helpers no longer counted as young.
pub fn without_furniture(busy: Option<Busy>) -> Option<Busy> {
    let mut b = busy?;
    if !b.young.is_empty() {
        let furniture: Vec<u32> = names(&b.young).into_iter().filter(|(_, n)| is_furniture(n)).map(|(p, _)| p).collect();
        b.young.retain(|p| !furniture.contains(p));
    }
    (!b.active.is_empty() || !b.young.is_empty()).then_some(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn a_tied_job_goes_with_a_death_and_not_with_a_way_out_chosen() {
        use std::os::windows::process::CommandExt;
        let start = || {
            std::process::Command::new("ping")
                .args(["-n", "30", "127.0.0.1"])
                .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap()
        };
        let ended_within = |c: &mut std::process::Child, ms: u64| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms);
            while std::time::Instant::now() < deadline {
                if c.try_wait().unwrap().is_some() {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            false
        };

        // As when a core dies.
        let mut c = start();
        let job = adopt(c.id()).unwrap();
        tie(&job, "t");
        close_as_in_a_death(job);
        assert!(ended_within(&mut c, 3000), "the OS ends what is in it");

        // A helper deleted or opened as a chat: the job is dropped.
        let mut c = start();
        let job = adopt(c.id()).unwrap();
        tie(&job, "t");
        drop(job);
        assert!(!ended_within(&mut c, 500), "`end_leftovers` chose what goes; the rest stays");
        let _ = c.kill();
        let _ = c.wait();

        // A core leaving: the job is never dropped, the process just ends.
        let mut c = start();
        let job = adopt(c.id()).unwrap();
        tie(&job, "t");
        let_go(&job);
        close_as_in_a_death(job);
        assert!(!ended_within(&mut c, 500), "let go of, its handle closing ends nothing");
        let _ = c.kill();
        let _ = c.wait();

        // A chat's job is never tied.
        let mut c = start();
        close_as_in_a_death(adopt(c.id()).unwrap());
        assert!(!ended_within(&mut c, 500));
        let _ = c.kill();
        let _ = c.wait();
    }

    /// A helper's CLI is made suspended and let run once in its job.
    #[cfg(windows)]
    #[test]
    fn a_process_made_suspended_runs_once_resumed() {
        use std::os::windows::process::CommandExt;
        let c = std::process::Command::new("cmd")
            .args(["/c", "exit", "0"])
            .creation_flags(0x0800_0000 | 0x0000_0004) // CREATE_NO_WINDOW | CREATE_SUSPENDED
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let mut kids = testkit::Kids(vec![c]);
        let c = &mut kids.0[0];
        assert!(!testkit::ended_within(c, 500), "suspended, it does not run");
        let job = testkit::job_of(c);
        assert!(resume(c.id()));
        assert!(testkit::ended_within(c, 5000), "resumed, it runs to its end");
        drop(job);
    }

    fn p(pid: u32, cpu_ms: u64, io_bytes: u64, started_ms: u64) -> ProcRead {
        ProcRead { pid, cpu_ms, io_bytes, started_ms }
    }

    /// The pipe polling that made such a `grep` read as at work is not counted.
    #[cfg(windows)]
    #[test]
    fn other_transfer_is_not_io() {
        use windows::Win32::System::Threading::IO_COUNTERS;
        let polling = IO_COUNTERS { OtherOperationCount: 1_417, OtherTransferCount: 56_680, ..Default::default() };
        assert_eq!(imp::io_bytes(&polling), 0);
        let download =
            IO_COUNTERS { WriteOperationCount: 2_067, WriteTransferCount: 16_826_368, OtherTransferCount: 256, ..Default::default() };
        assert_eq!(imp::io_bytes(&download), 16_826_368);
    }

    #[test]
    fn quiet_furniture_is_not_busy() {
        let prev = [p(10, 100, 1000, 1_000)];
        let now = [p(10, 100, 1000, 1_000)];
        assert_eq!(judge(&prev, &now, 5_000, 50, 1024), None);
    }

    #[test]
    fn cpu_growth_is_busy() {
        let prev = [p(10, 100, 0, 1_000)];
        let now = [p(10, 180, 0, 1_000)];
        assert_eq!(judge(&prev, &now, 0, 50, 1024).unwrap().active, vec![10]);
    }

    #[test]
    fn io_growth_is_busy_even_without_cpu() {
        let prev = [p(10, 100, 500, 1_000)];
        let now = [p(10, 101, 90_000, 1_000)];
        assert_eq!(judge(&prev, &now, 0, 50, 64 * 1024).unwrap().active, vec![10]);
        // A `tail -F`'s polling of its file is no work.
        let polled = [p(10, 101, 1_900, 1_000)];
        assert_eq!(judge(&prev, &polled, 0, 50, 64 * 1024), None);
    }

    #[test]
    fn a_sleeping_child_of_a_turn_is_busy() {
        // Born after the first turn started, no CPU at all: `sleep 90` in the background.
        let prev = [p(10, 0, 0, 9_000)];
        let now = [p(10, 0, 0, 9_000)];
        let b = judge(&prev, &now, 8_000, 50, 1024).unwrap();
        assert!(b.active.is_empty());
        assert_eq!(b.young, vec![10]);
    }

    #[test]
    fn a_child_older_than_the_first_turn_is_furniture() {
        let prev = [p(10, 0, 0, 7_000)];
        let now = [p(10, 0, 0, 7_000)];
        assert_eq!(judge(&prev, &now, 8_000, 50, 1024), None);
    }

    #[test]
    fn a_window_and_everything_under_it_is_the_users() {
        // 20 a browser with a window, 21 its tab (born after), 30 a job, 31 a
        // process older than the browser whose parent pid 20 was another's.
        let reads = [p(20, 0, 0, 1_000), p(21, 0, 0, 2_000), p(30, 0, 0, 1_000), p(31, 0, 0, 500)];
        let windowed: std::collections::HashSet<u32> = [20].into();
        let shown = shown(&reads, &windowed, &[(21, 20), (30, 1), (31, 20)]);
        assert!(shown.contains(&20) && shown.contains(&21));
        assert!(!shown.contains(&30), "work with no window is the session's");
        assert!(!shown.contains(&31), "a child older than its parent pid is not its child");
    }

    #[test]
    fn a_clis_own_helper_is_furniture_by_name() {
        assert!(is_furniture("codex-code-mode-host.exe"));
        assert!(is_furniture("Codex-Code-Mode-Host.EXE"));
        assert!(is_furniture("codex-code-mode-host"));
        assert!(is_furniture("conhost.exe"));
        assert!(!is_furniture("codex.exe"));
        assert!(!is_furniture("node.exe"));
    }

    #[test]
    fn pid_reuse_does_not_count_as_growth() {
        let prev = [p(10, 5000, 0, 1_000)];
        let now = [p(10, 10, 0, 2_000)]; // same pid, a different process
        assert_eq!(judge(&prev, &now, 0, 50, 1024), None);
    }
}
