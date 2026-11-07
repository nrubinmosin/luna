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
//! in and out (keeper.rs). There a job lists nothing: no window can be told from
//! a job left running, so nothing is counted, and what a turn left runs on past
//! every way out but a death.

#[cfg(windows)]
mod imp {
    use windows::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, FILETIME, HANDLE};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, Thread32First, Thread32Next, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
        TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicProcessIdList, JobObjectExtendedLimitInformation,
        QueryInformationJobObject, SetInformationJobObject, JOBOBJECT_BASIC_PROCESS_ID_LIST, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, GetProcessIoCounters, GetProcessTimes, OpenProcess, OpenThread, ResumeThread, TerminateProcess, IO_COUNTERS,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA, PROCESS_TERMINATE, THREAD_SUSPEND_RESUME,
    };
    use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId, IsWindowVisible};

    pub struct Job(HANDLE);
    // A job handle is a kernel object; any thread may query or close it.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Drop for Job {
        fn drop(&mut self) {
            let_go(self);
            // SAFETY: the handle was returned by CreateJobObjectW and is closed once.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    fn set_limit(job: &Job, flags: JOB_OBJECT_LIMIT) -> windows::core::Result<()> {
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = flags;
        // SAFETY: the job handle is open for as long as `job` lives, and the
        // struct is the one the class names, of the size passed.
        unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const std::ffi::c_void,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        }
    }

    /// Has the OS end everything in the job once its last handle closes, which
    /// this process's death does: nothing else holds one, and none is inherited.
    pub fn tie(job: &Job, _id: &str) -> bool {
        let tied = set_limit(job, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE);
        if let Err(e) = &tied {
            crate::log::warn("procs", &format!("could not tie a job to the core: {e}"));
        }
        tied.is_ok()
    }

    /// Undoes `tie`: what is in the job runs on when its handle closes.
    pub fn let_go(job: &Job) {
        let _ = set_limit(job, JOB_OBJECT_LIMIT(0));
    }

    /// What became of the last core's jobs: the OS saw to them as it died.
    pub struct LastTies;

    impl LastTies {
        /// A tied job was seen out, every one.
        pub fn held(&self, _id: &str) -> bool {
            true
        }

        /// Nothing written down, so nothing looked at to say otherwise now of a
        /// job that was not tied.
        pub fn confirmed(&self, _id: &str) -> bool {
            false
        }
    }

    pub fn end_last_ties() -> LastTies {
        LastTies
    }

    /// Needed only where a job goes by what is in the environment (keeper.rs).
    pub fn also_marked(_job: &Job, _var: &str, _value: &str) {}

    /// So is this.
    pub fn await_environment(_pid: u32) {}

    /// The tie needs no process of its own here.
    pub fn keeper_from_args() -> bool {
        false
    }

    /// The job's handle closed as a dying process's is, no `let_go` first.
    #[cfg(test)]
    pub fn close_as_in_a_death(job: Job) {
        let h = job.0;
        std::mem::forget(job);
        // SAFETY: the handle is open, and `forget` keeps `Drop` from closing it again.
        unsafe {
            let _ = CloseHandle(h);
        }
    }

    /// Puts a freshly spawned process into a job of its own. None when Windows
    /// refuses — a process already in a job that forbids nesting, say — and
    /// the caller falls back to the parent-pid walk.
    pub fn adopt(pid: u32) -> Option<Job> {
        // SAFETY: plain Win32 calls on handles this function owns; every
        // handle opened here is closed on every path.
        unsafe {
            let job = match CreateJobObjectW(None, windows::core::PCWSTR::null()) {
                Ok(h) => h,
                Err(e) => {
                    crate::log::warn("procs", &format!("CreateJobObject for pid {pid}: {e}"));
                    return None;
                }
            };
            let proc_ = match OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, false, pid) {
                Ok(h) => h,
                Err(e) => {
                    crate::log::warn("procs", &format!("OpenProcess {pid} for job: {e}"));
                    let _ = CloseHandle(job);
                    return None;
                }
            };
            let assigned = AssignProcessToJobObject(job, proc_);
            let _ = CloseHandle(proc_);
            if let Err(e) = assigned {
                crate::log::warn("procs", &format!("AssignProcessToJobObject {pid}: {e}"));
                let _ = CloseHandle(job);
                return None;
            }
            Some(Job(job))
        }
    }

    /// Puts another process into a job made by `adopt`: a helper's later
    /// turns join the job of its first.
    pub fn join(job: &Job, pid: u32) -> bool {
        // SAFETY: a process handle opened and closed here; the job handle is
        // the caller's and stays open.
        unsafe {
            let Ok(proc_) = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, false, pid) else { return false };
            let assigned = AssignProcessToJobObject(job.0, proc_);
            let _ = CloseHandle(proc_);
            assigned.is_ok()
        }
    }

    /// Lets a process created suspended run, once it is in its job: nothing it
    /// starts can be born outside. Created so, it has the one thread.
    pub fn resume(pid: u32) -> bool {
        let mut resumed = false;
        // SAFETY: a snapshot and thread handles opened and closed here; the
        // entry is a plain stack struct of the size it says.
        unsafe {
            let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) else { return false };
            let mut t = THREADENTRY32 { dwSize: size_of::<THREADENTRY32>() as u32, ..Default::default() };
            if Thread32First(snap, &mut t).is_ok() {
                loop {
                    if t.th32OwnerProcessID == pid {
                        if let Ok(h) = OpenThread(THREAD_SUSPEND_RESUME, false, t.th32ThreadID) {
                            resumed |= ResumeThread(h) != u32::MAX;
                            let _ = CloseHandle(h);
                        }
                    }
                    if Thread32Next(snap, &mut t).is_err() {
                        break;
                    }
                }
            }
            let _ = CloseHandle(snap);
        }
        resumed
    }

    /// A process held open: its pid is not given to another process while
    /// this lives, however soon the process itself ends.
    pub struct Pin(Option<HANDLE>);
    // A process handle is a kernel object; any thread may close it.
    unsafe impl Send for Pin {}

    impl Drop for Pin {
        fn drop(&mut self) {
            if let Some(h) = self.0.take() {
                // SAFETY: opened by `pin` and closed once.
                unsafe {
                    let _ = CloseHandle(h);
                }
            }
        }
    }

    /// Pins `pid` while it is still known to be the process meant: taken where
    /// something else holds it open (a turn's `Child`), it stays that one's after.
    pub fn pin(pid: u32) -> Pin {
        // SAFETY: a handle owned by the `Pin`, closed when it drops.
        Pin(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok())
    }

    /// Ends one process, if it is still the one created at `started_ms`: a
    /// pid read a moment ago may belong to another process by now.
    pub fn end(pid: u32, started_ms: u64) -> bool {
        // SAFETY: a process handle opened and closed here; out-params are
        // plain stack structs.
        unsafe {
            let Ok(h) = OpenProcess(PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
            let (mut c, mut x, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
            let same = GetProcessTimes(h, &mut c, &mut x, &mut k, &mut u).is_ok() && filetime_to_unix_ms(c) == started_ms;
            let ended = same && TerminateProcess(h, 1).is_ok();
            let _ = CloseHandle(h);
            ended
        }
    }

    /// The pids that own a visible top-level window.
    pub fn windowed() -> std::collections::HashSet<u32> {
        use windows::Win32::Foundation::{HWND, LPARAM};
        unsafe extern "system" fn each(hwnd: HWND, out: LPARAM) -> windows::core::BOOL {
            // SAFETY: `out` is the set passed below, alive for the whole
            // synchronous enumeration.
            unsafe {
                if IsWindowVisible(hwnd).as_bool() {
                    let mut pid = 0u32;
                    GetWindowThreadProcessId(hwnd, Some(&mut pid));
                    (*(out.0 as *mut std::collections::HashSet<u32>)).insert(pid);
                }
            }
            windows::core::BOOL(1)
        }
        let mut out = std::collections::HashSet::new();
        // SAFETY: the callback only writes into `out`, which outlives the call.
        unsafe {
            let _ = EnumWindows(Some(each), LPARAM(&mut out as *mut _ as isize));
        }
        out
    }

    /// (pid, parent pid) of every process now.
    pub fn parents() -> Vec<(u32, u32)> {
        let mut edges = Vec::new();
        // SAFETY: as in `descendants`.
        unsafe {
            let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return edges };
            let mut e = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
            if Process32FirstW(snap, &mut e).is_ok() {
                loop {
                    edges.push((e.th32ProcessID, e.th32ParentProcessID));
                    if Process32NextW(snap, &mut e).is_err() {
                        break;
                    }
                }
            }
            let _ = CloseHandle(snap);
        }
        edges
    }

    /// Every pid in the job right now, the CLI itself included.
    pub fn members(job: &Job) -> Vec<u32> {
        let mut cap: usize = 256;
        loop {
            // Header (two u32) followed by `cap` usize pids.
            let bytes = size_of::<JOBOBJECT_BASIC_PROCESS_ID_LIST>() + cap * size_of::<usize>();
            let mut buf = vec![0u8; bytes];
            // SAFETY: the buffer is at least a header long and outlives the call;
            // the header is read only after the call filled it.
            let (ok, assigned, listed) = unsafe {
                let r = QueryInformationJobObject(Some(job.0), JobObjectBasicProcessIdList, buf.as_mut_ptr() as *mut _, bytes as u32, None);
                let head = &*(buf.as_ptr() as *const JOBOBJECT_BASIC_PROCESS_ID_LIST);
                (r.is_ok(), head.NumberOfAssignedProcesses as usize, head.NumberOfProcessIdsInList as usize)
            };
            if ok {
                // SAFETY: `listed` pids follow the two header words; the list
                // is laid out as the struct's flexible array.
                let pids = unsafe {
                    let first = (buf.as_ptr() as *const u32).add(2) as *const usize;
                    (0..listed.min(cap)).map(|i| *first.add(i) as u32).collect()
                };
                return pids;
            }
            // ERROR_MORE_DATA: the header still says how many there are.
            if assigned > cap && cap < 1 << 16 {
                cap = assigned + 64;
                continue;
            }
            return Vec::new();
        }
    }

    /// The pids below `root` by parent links, `root` included. Only for a CLI
    /// that could not be put in a job.
    pub fn descendants(root: u32) -> Vec<u32> {
        let mut edges: Vec<(u32, u32)> = Vec::new(); // (pid, parent)
                                                     // SAFETY: a snapshot handle owned and closed here; the entry struct is
                                                     // sized before the first call as the API requires.
        unsafe {
            let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return vec![root] };
            let mut e = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
            if Process32FirstW(snap, &mut e).is_ok() {
                loop {
                    edges.push((e.th32ProcessID, e.th32ParentProcessID));
                    if Process32NextW(snap, &mut e).is_err() {
                        break;
                    }
                }
            }
            let _ = CloseHandle(snap);
        }
        let mut out = vec![root];
        let mut i = 0;
        while i < out.len() {
            let parent = out[i];
            for (pid, ppid) in &edges {
                if *ppid == parent && !out.contains(pid) {
                    out.push(*pid);
                }
            }
            i += 1;
        }
        out
    }

    fn filetime_100ns(ft: FILETIME) -> u64 {
        ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64
    }

    /// 100ns intervals since 1601 → unix milliseconds.
    fn filetime_to_unix_ms(ft: FILETIME) -> u64 {
        const EPOCH_DIFF: u64 = 116_444_736_000_000_000;
        filetime_100ns(ft).saturating_sub(EPOCH_DIFF) / 10_000
    }

    /// CPU time, IO volume, and start time of one process, if it can be opened.
    pub fn read(pid: u32) -> Option<super::ProcRead> {
        // SAFETY: a process handle opened and closed here; out-params are
        // plain stack structs.
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let (mut c, mut x, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
            let times = GetProcessTimes(h, &mut c, &mut x, &mut k, &mut u);
            let mut io = IO_COUNTERS::default();
            let _ = GetProcessIoCounters(h, &mut io);
            let _ = CloseHandle(h);
            times.ok()?;
            Some(super::ProcRead {
                pid,
                cpu_ms: (filetime_100ns(k) + filetime_100ns(u)) / 10_000,
                io_bytes: io_bytes(&io),
                started_ms: filetime_to_unix_ms(c),
            })
        }
    }

    /// What a process moved, read and written. Not "other" transfer, which is
    /// control traffic: a Git Bash `grep` waiting on a pipe from `tail -f`
    /// polls it some 70 times a second, 2.8 KB/s of "other" with nothing read
    /// or written, which read as work and held a PC up all night. Nothing
    /// real is lost with it: a download's network bytes show in none of the
    /// three, only as the writes of what it saves (a `curl` to NUL: 2 MB/s
    /// written, 32 bytes/s other).
    pub(super) fn io_bytes(io: &IO_COUNTERS) -> u64 {
        io.ReadTransferCount + io.WriteTransferCount
    }

    /// Whether a process of this pid is running now.
    pub fn alive(pid: u32) -> bool {
        // The exit code a process reads as while it runs.
        const STILL_ACTIVE: u32 = 259;
        // SAFETY: a process handle opened and closed here.
        unsafe {
            match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                Ok(h) => {
                    let mut code = 0u32;
                    let got = GetExitCodeProcess(h, &mut code);
                    let _ = CloseHandle(h);
                    got.is_err() || code == STILL_ACTIVE
                }
                // One that may not be opened is still there.
                Err(e) => e.code() == windows::core::HRESULT::from_win32(ERROR_ACCESS_DENIED.0),
            }
        }
    }

    /// Executable names by pid, for saying what is holding the machine up.
    pub fn names(pids: &[u32]) -> Vec<(u32, String)> {
        let mut out = Vec::new();
        // SAFETY: as in `descendants`.
        unsafe {
            let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
            let mut e = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
            if Process32FirstW(snap, &mut e).is_ok() {
                loop {
                    if pids.contains(&e.th32ProcessID) {
                        let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                        out.push((e.th32ProcessID, String::from_utf16_lossy(&e.szExeFile[..len])));
                    }
                    if Process32NextW(snap, &mut e).is_err() {
                        break;
                    }
                }
            }
            let _ = CloseHandle(snap);
        }
        out
    }
}

#[cfg(not(windows))]
mod imp {
    pub use crate::keeper::{adopt, also_marked, await_environment, end_last_ties, from_args as keeper_from_args, join, let_go, tie, Job};

    /// Nothing holds a pid here: the CLI is reaped as it ends, and what kills by
    /// pid looks again before it does (keeper.rs `end_tree`).
    pub struct Pin;

    pub fn pin(_pid: u32) -> Pin {
        Pin
    }

    pub fn end(_pid: u32, _started_ms: u64) -> bool {
        false
    }
    pub fn windowed() -> std::collections::HashSet<u32> {
        Default::default()
    }
    pub fn parents() -> Vec<(u32, u32)> {
        Vec::new()
    }
    pub fn members(_job: &Job) -> Vec<u32> {
        Vec::new()
    }
    pub fn descendants(root: u32) -> Vec<u32> {
        vec![root]
    }
    pub fn read(_pid: u32) -> Option<super::ProcRead> {
        None
    }
    /// `kill -0` answers while the process lives.
    pub fn alive(pid: u32) -> bool {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }
    pub fn names(_pids: &[u32]) -> Vec<(u32, String)> {
        Vec::new()
    }
}

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

/// A process a session's turns left running past their end — a helper's in its
/// job, a chat's under its CLI — as last read, and when it was last at work.
/// One model for both: what `luna_list` names, and what holds the machine.
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
/// At work this recently: what `luna_list` calls `active`.
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

/// What tests that start processes share: real ones, ended however a test
/// ends, a panic included, and none that runs on unbounded if one is missed.
#[cfg(all(test, windows))]
pub(crate) mod testkit {
    /// Processes a test starts, ended when it is dropped. A test binary killed
    /// outright takes them along by the job `job_of` ties them in.
    pub struct Kids(pub Vec<std::process::Child>);

    impl Drop for Kids {
        fn drop(&mut self) {
            for c in &mut self.0 {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
    }

    pub fn hidden(exe: &str, args: &[&str]) -> std::process::Child {
        use std::os::windows::process::CommandExt;
        std::process::Command::new(exe)
            .args(args)
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap()
    }

    /// A ping that sits there for at most `secs`.
    pub fn idler(secs: u32) -> std::process::Child {
        hidden("ping", &["-n", &secs.to_string(), "127.0.0.1"])
    }

    /// A loop that burns a core for at most `secs`.
    pub fn burner(secs: u32) -> std::process::Child {
        let script = format!("$end = [DateTime]::Now.AddSeconds({secs}); while ([DateTime]::Now -lt $end) {{ }}");
        hidden("powershell", &["-NoProfile", "-NonInteractive", "-Command", &script])
    }

    /// A job for a test's processes, tied as a helper's is: they go with the
    /// test binary should it die.
    pub fn job_of(first: &std::process::Child) -> std::sync::Arc<super::Job> {
        let job = super::adopt(first.id()).unwrap();
        super::tie(&job, "test");
        std::sync::Arc::new(job)
    }

    pub fn ended_within(c: &mut std::process::Child, ms: u64) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms);
        while std::time::Instant::now() < deadline {
            if c.try_wait().unwrap().is_some() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        false
    }
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
        imp::close_as_in_a_death(job);
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
        imp::close_as_in_a_death(job);
        assert!(!ended_within(&mut c, 500), "let go of, its handle closing ends nothing");
        let _ = c.kill();
        let _ = c.wait();

        // A chat's job is never tied.
        let mut c = start();
        imp::close_as_in_a_death(adopt(c.id()).unwrap());
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
        // Active for luna_list, holding for the machine, and letting go once.
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
