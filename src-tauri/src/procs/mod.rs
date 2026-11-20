//! The processes a session's CLI has under it.
//!
//! A CLI that has finished its turn can still have work running in the shell:
//! a background build, a test run, a `sleep` it means to come back to. Where
//! the CLI does not say so itself (Codex, a helper's turn gone), the trace is
//! the process tree. On Windows the tree is read off a Job Object each session
//! is put in at spawn — children inherit it, so `QueryInformationJobObject`
//! lists them all without walking parent pids, which pid reuse makes
//! unreliable. The walk stays as the fallback for a CLI that could not be
//! assigned. What is there is all that is read: whether it does anything is
//! not measured.
//!
//! A chat's job sets no limits: its CLI goes with its terminal. What is left in
//! a job when a session is done with is ended by `end_leftovers`, which the
//! CLI's own tree kill misses: a process whose parent had already gone. A
//! program with a window of its own — a browser a login opened, an editor — is
//! the user's, not the session's work, and is not ended. It is counted all the
//! same: `Start-Process python server.py` or `start cmd /k npm run dev` shows a
//! console window of its own and is a turn's work too, and a program that came
//! up in a job after the CLI's furniture was taken is either, which only a
//! person can tell. Liveness errs active, ending errs spared.
//!
//! A background helper has no terminal to go with, so its job is tied to the
//! core (`tie`): the OS ends what is in it when the core's handle closes, as it
//! does however the core dies, and a CLI does not run on unowned on a
//! conversation the next core may resume. That takes windowed programs too,
//! which the OS will not except, so every way out that is not a death lets go
//! of the tie first (`let_go`, and dropping a job), and `end_leftovers` spares
//! them as ever. Off Windows a process stands in for the OS, with the same ways
//! in and out (keeper/), and a job lists by the keeper's walk of the process
//! table (`keeper::listed`). There no window can be told from a job left
//! running: all of it is counted, and ended with the session.
//!
//! This module holds a CLI's children as read now. The rest is split by what
//! it does:
//!
//! - `imp`: what each OS gives — the Job Object and its members, process
//!   reads, ends and windows on Windows (win.rs), the keeper's tie off it
//!   (unix.rs).
//! - `leftovers`: what a session's turns left running past their end.
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
pub use imp::{
    adopt, alive, also_marked, await_environment, end, end_last_ties, join, keeper_from_args, let_go, marked, pin, tie, Job, Pin,
};
#[cfg(windows)]
pub use imp::{resume, KillJob};

/// When a process was created, epoch ms, where the platform says.
#[cfg(windows)]
pub fn created_ms(pid: u32) -> Option<u64> {
    imp::read(pid).map(|r| r.started_ms)
}

/// When a process was created, epoch ms, where the platform says: not here.
#[cfg(not(windows))]
pub fn created_ms(_pid: u32) -> Option<u64> {
    None
}

/// Whether `ProcRead::started_ms` is epoch milliseconds, comparable with a
/// CLI's own times: on Windows. Elsewhere it is the OS's own count, comparable
/// only with another process's.
pub const STARTS_IN_EPOCH_MS: bool = cfg!(windows);

/// One process, as read now: a pid and when it was created, which tells it
/// from another that took the pid since.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProcRead {
    pub pid: u32,
    pub started_ms: u64,
}

/// What a session's job holds right now, each process with its name, the CLI
/// itself (`root`) left out and every program with a window of its own kept:
/// this is for liveness, which counts a console a turn opened for its server.
/// None when the job cannot be listed: not known is not empty.
pub fn listed(job: Option<&Job>, root: Option<u32>) -> Option<Vec<(ProcRead, String)>> {
    let all = match (job, root) {
        (Some(j), _) => imp::list(j)?,
        (None, Some(pid)) => imp::list_below(pid)?,
        (None, None) => return Some(Vec::new()),
    };
    Some(all.into_iter().filter(|(r, _)| Some(r.pid) != root).collect())
}

/// `listed` without the programs with a window of their own and what is below
/// them (`shown`), which are the user's to keep and so not Luna's to end.
fn listed_to_end(job: Option<&Job>, root: Option<u32>) -> Option<Vec<(ProcRead, String)>> {
    let all = listed(job, root)?;
    if all.is_empty() {
        return Some(all);
    }
    Some(unshown(all, &imp::windowed(), &imp::parents()))
}

fn unshown(all: Vec<(ProcRead, String)>, windowed: &std::collections::HashSet<u32>, edges: &[(u32, u32)]) -> Vec<(ProcRead, String)> {
    let reads: Vec<ProcRead> = all.iter().map(|(r, _)| *r).collect();
    let shown = shown(&reads, windowed, edges);
    all.into_iter().filter(|(r, _)| !shown.contains(&r.pid)).collect()
}

/// What runs below a helper's turn whose process could not be put in its job
/// (`adopt` or `join` refused): every process below its CLI (`cli`) by parent
/// links born no earlier than the turn (`since_ms`, so a pid taken since by
/// another is no parent), the CLI itself and its own helpers aside, as `left_in`
/// has a job; a program with a window of its own counts. None where processes
/// cannot be walked so (off Windows, a snapshot refused): not known is not
/// empty.
pub fn left_below(cli: u32, since_ms: u64) -> Option<Vec<(ProcRead, String)>> {
    let all: Vec<(ProcRead, String)> =
        imp::list_below(cli)?.into_iter().filter(|(r, _)| r.pid != cli && (!STARTS_IN_EPOCH_MS || r.started_ms >= since_ms)).collect();
    Some(all.into_iter().filter(|(_, name)| !is_furniture(name)).collect())
}

/// `listed` without names, nothing for a job that cannot be listed: for the
/// tests that look for one process in a job.
#[cfg(all(test, windows))]
pub fn children(job: Option<&Job>, root: Option<u32>) -> Vec<ProcRead> {
    listed(job, root).unwrap_or_default().into_iter().map(|(r, _)| r).collect()
}

/// Of `reads`, the ones that own a visible window and every one below them,
/// which Luna does not end (`listed_to_end`):
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
/// its name: every process, as each was born of a turn, but the CLIs' own
/// helpers (`is_furniture`). None when the job cannot be listed.
pub fn left_in(job: &Job) -> Option<Vec<(ProcRead, String)>> {
    Some(listed(Some(job), None)?.into_iter().filter(|(_, name)| !is_furniture(name)).collect())
}

/// What a terminal session's job holds besides its CLI (`root`), for its
/// lifecycle record: each process with its name, the CLIs' own helpers left
/// out and programs with a window of their own counted. Which of it came up with the
/// CLI and which a turn started is the record's to tell (activity/). None when
/// the job cannot be listed.
pub fn in_job(job: Option<&Job>, root: Option<u32>) -> Option<Vec<(ProcRead, String)>> {
    Some(listed(job, root)?.into_iter().filter(|(_, name)| !is_furniture(name)).collect())
}

/// Ends what is left in a job once its session or helper is done with: what
/// the CLI's tree kill missed, a process whose parent had gone first. Not
/// `keep` (the CLI, which its own kill takes), and not a program with a
/// window of its own (`listed_to_end`). Returns the names of those it ended.
pub fn end_leftovers(job: &Job, keep: Option<u32>) -> Vec<String> {
    end_all(&listed_to_end(Some(job), keep).unwrap_or_default())
}

/// `end_leftovers` for what a turn that came to nothing started of its own:
/// what in the job descends from its CLI (`cli`, started at `since_ms`) through
/// parents still there, each born no earlier than `since_ms` nor than the child
/// it is taken for the parent of, so a reused pid is no parent. Only that: a
/// process whose way up breaks (a launcher that has gone) is no proof of whose
/// it is and is spared, as is all an earlier turn left and what that starts.
pub fn end_tree_of(job: &Job, cli: u32, since_ms: u64) -> Vec<String> {
    let reads = listed_to_end(Some(job), None).unwrap_or_default();
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
            match reads.iter().map(|(p, _)| p).find(|p| p.pid == ppid && p.started_ms >= since_ms && p.started_ms <= at.started_ms) {
                Some(p) => at = *p,
                None => return false,
            }
        }
        false
    };
    let its: Vec<(ProcRead, String)> = reads.iter().filter(|(r, _)| r.pid != cli && of_cli(r)).cloned().collect();
    end_all(&its)
}

/// `end_leftovers` sparing `kept` (pid and start, as read before) and the
/// turn's CLI itself (`cli`, its server's own to end): off
/// Windows, where no parent links tell what a turn started, what its job (the
/// keeper's listing: the turns' groups, sessions and mark) holds that was not
/// there before it.
#[cfg(not(windows))]
pub fn end_all_but(job: &Job, cli: u32, kept: &[(u32, u64)]) -> Vec<String> {
    let reads = listed_to_end(Some(job), Some(cli)).unwrap_or_default();
    let new: Vec<(ProcRead, String)> = reads.into_iter().filter(|(r, _)| !kept.contains(&(r.pid, r.started_ms))).collect();
    end_all(&new)
}

fn end_all(left: &[(ProcRead, String)]) -> Vec<String> {
    left.iter().filter(|(r, _)| end(r.pid, r.started_ms)).map(|(_, name)| name.clone()).collect()
}

/// Helpers a CLI starts by itself the first time a turn needs one and keeps
/// for the rest of the session. Born of a turn, they read as a job the model
/// left running, and a Codex helper whose turn was over was announced as
/// "NOT done yet" with nothing to come. They are furniture, never a shell. A
/// console host is never the job itself: a job that runs on in one has its own
/// process alongside to count, and the code-mode host comes with one of its
/// own.
const CLI_FURNITURE: &[&str] = &["codex-code-mode-host", "conhost"];

fn is_furniture(exe: &str) -> bool {
    let exe = exe.to_ascii_lowercase();
    let stem = exe.strip_suffix(".exe").unwrap_or(&exe);
    CLI_FURNITURE.contains(&stem)
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

    fn p(pid: u32, started_ms: u64) -> ProcRead {
        ProcRead { pid, started_ms }
    }

    /// A terminal's job lists what is in it by name, the CLI itself aside.
    #[cfg(windows)]
    #[test]
    fn a_job_lists_its_processes_by_name_but_the_cli() {
        let kids = testkit::Kids(vec![testkit::idler(30)]);
        let pid = kids.0[0].id();
        let job = testkit::job_of(&kids.0[0]);
        let named = in_job(Some(&job), None).expect("a job lists");
        assert!(named.len() == 1 && named[0].1.to_ascii_lowercase().starts_with("ping") && named[0].0.pid == pid, "{named:?}");
        assert!(in_job(Some(&job), Some(pid)).unwrap().is_empty(), "the CLI itself is not listed");
    }

    #[test]
    fn a_window_and_everything_under_it_is_the_users() {
        // 20 a browser with a window, 21 its tab (born after), 30 a job, 31 a
        // process older than the browser whose parent pid 20 was another's.
        let reads = [p(20, 1_000), p(21, 2_000), p(30, 1_000), p(31, 500)];
        let windowed: std::collections::HashSet<u32> = [20].into();
        let shown = shown(&reads, &windowed, &[(21, 20), (30, 1), (31, 20)]);
        assert!(shown.contains(&20) && shown.contains(&21));
        assert!(!shown.contains(&30), "work with no window is the session's");
        assert!(!shown.contains(&31), "a child older than its parent pid is not its child");
    }

    /// A console a turn opened for its server shows a window: `listed` keeps it
    /// for liveness, and ending spares it as the user's (`unshown`).
    #[test]
    fn a_member_with_a_window_is_spared_by_ending_only() {
        let all =
            vec![(p(20, 1_000), "cmd.exe".to_string()), (p(21, 2_000), "node.exe".to_string()), (p(30, 1_000), "ping.exe".to_string())];
        let windowed: std::collections::HashSet<u32> = [20].into();
        let to_end: Vec<u32> = unshown(all, &windowed, &[(21, 20), (30, 1)]).iter().map(|(r, _)| r.pid).collect();
        assert_eq!(to_end, vec![30], "ending spares the window and what is under it");
    }

    /// Real: a process with a console of its own, in a job, is listed.
    #[cfg(windows)]
    #[test]
    fn a_job_lists_a_process_with_a_console_window_of_its_own() {
        use std::os::windows::process::CommandExt;
        let c = std::process::Command::new("cmd")
            .args(["/k", "ping -n 30 127.0.0.1 >nul"])
            .creation_flags(0x0000_0010) // CREATE_NEW_CONSOLE
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let kids = testkit::Kids(vec![c]);
        let job = testkit::job_of(&kids.0[0]);
        let pid = kids.0[0].id();
        assert!(listed(Some(&job), None).unwrap().iter().any(|(r, _)| r.pid == pid), "counted whether or not it shows a window");
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
}
