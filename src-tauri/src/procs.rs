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
//! Nothing is limited or killed through the job; it exists to be listed.

#[cfg(windows)]
mod imp {
    use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicProcessIdList, QueryInformationJobObject,
        JOBOBJECT_BASIC_PROCESS_ID_LIST,
    };
    use windows::Win32::System::Threading::{
        GetProcessIoCounters, GetProcessTimes, OpenProcess, IO_COUNTERS, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_SET_QUOTA, PROCESS_TERMINATE,
    };

    pub struct Job(HANDLE);
    // A job handle is a kernel object; any thread may query or close it.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: the handle was returned by CreateJobObjectW and is closed once.
            unsafe {
                let _ = CloseHandle(self.0);
            }
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

    /// Every pid in the job right now, the CLI itself included.
    pub fn members(job: &Job) -> Vec<u32> {
        let mut cap: usize = 256;
        loop {
            // Header (two u32) followed by `cap` usize pids.
            let bytes = std::mem::size_of::<JOBOBJECT_BASIC_PROCESS_ID_LIST>() + cap * std::mem::size_of::<usize>();
            let mut buf = vec![0u8; bytes];
            // SAFETY: the buffer is at least a header long and outlives the call;
            // the header is read only after the call filled it.
            let (ok, assigned, listed) = unsafe {
                let r = QueryInformationJobObject(
                    Some(job.0),
                    JobObjectBasicProcessIdList,
                    buf.as_mut_ptr() as *mut _,
                    bytes as u32,
                    None,
                );
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
            let mut e = PROCESSENTRY32W::default();
            e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
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
                io_bytes: io.ReadTransferCount + io.WriteTransferCount + io.OtherTransferCount,
                started_ms: filetime_to_unix_ms(c),
            })
        }
    }

    /// Executable names by pid, for saying what is holding the machine up.
    pub fn names(pids: &[u32]) -> Vec<(u32, String)> {
        let mut out = Vec::new();
        // SAFETY: as in `descendants`.
        unsafe {
            let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
            let mut e = PROCESSENTRY32W::default();
            e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
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
    pub struct Job(());
    pub fn adopt(_pid: u32) -> Option<Job> {
        None
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
    pub fn names(_pids: &[u32]) -> Vec<(u32, String)> {
        Vec::new()
    }
}

pub use imp::{adopt, Job};

/// One process, as counted at one sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProcRead {
    pub pid: u32,
    /// Kernel + user time, cumulative.
    pub cpu_ms: u64,
    /// Read + write + other transfer, cumulative.
    pub io_bytes: u64,
    pub started_ms: u64,
}

/// The children of a session's CLI right now — the CLI itself left out, since
/// its own idle repaint would count as work.
pub fn children(job: Option<&Job>, root: Option<u32>) -> Vec<ProcRead> {
    let pids = match (job, root) {
        (Some(j), _) => imp::members(j),
        (None, Some(pid)) => imp::descendants(pid),
        (None, None) => Vec::new(),
    };
    pids.into_iter().filter(|p| Some(*p) != root).filter_map(imp::read).collect()
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
/// is the model's doing — a background job it means to come back to — and
/// counts while it lives, whereas the MCP servers and shells that came up
/// with the session are furniture and only count while they move. A child
/// older than `stale_before_ms` that no longer moves is furniture too,
/// whatever started it: a helper the CLI keeps around must not hold the
/// machine for a day.
pub fn judge(
    prev: &[ProcRead],
    now: &[ProcRead],
    born_after_ms: u64,
    stale_before_ms: u64,
    min_cpu_ms: u64,
) -> Option<Busy> {
    let mut active = Vec::new();
    let mut young = Vec::new();
    for p in now {
        if let Some(q) = prev.iter().find(|q| q.pid == p.pid && q.started_ms == p.started_ms) {
            if p.cpu_ms.saturating_sub(q.cpu_ms) >= min_cpu_ms || p.io_bytes > q.io_bytes {
                active.push(p.pid);
            }
        }
        if born_after_ms > 0 && p.started_ms >= born_after_ms && p.started_ms > stale_before_ms {
            young.push(p.pid);
        }
    }
    (!active.is_empty() || !young.is_empty()).then_some(Busy { active, young })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, cpu_ms: u64, io_bytes: u64, started_ms: u64) -> ProcRead {
        ProcRead { pid, cpu_ms, io_bytes, started_ms }
    }

    #[test]
    fn quiet_furniture_is_not_busy() {
        let prev = [p(10, 100, 1000, 1_000)];
        let now = [p(10, 100, 1000, 1_000)];
        assert_eq!(judge(&prev, &now, 5_000, 0, 50), None);
    }

    #[test]
    fn cpu_growth_is_busy() {
        let prev = [p(10, 100, 0, 1_000)];
        let now = [p(10, 180, 0, 1_000)];
        assert_eq!(judge(&prev, &now, 0, 0, 50).unwrap().active, vec![10]);
    }

    #[test]
    fn io_growth_is_busy_even_without_cpu() {
        let prev = [p(10, 100, 500, 1_000)];
        let now = [p(10, 101, 900, 1_000)];
        assert_eq!(judge(&prev, &now, 0, 0, 50).unwrap().active, vec![10]);
    }

    #[test]
    fn a_sleeping_child_of_a_turn_is_busy() {
        // Born after the first turn started, no CPU at all: `sleep 90` in the background.
        let prev = [p(10, 0, 0, 9_000)];
        let now = [p(10, 0, 0, 9_000)];
        let b = judge(&prev, &now, 8_000, 0, 50).unwrap();
        assert!(b.active.is_empty());
        assert_eq!(b.young, vec![10]);
    }

    #[test]
    fn a_child_older_than_the_first_turn_is_furniture() {
        let prev = [p(10, 0, 0, 7_000)];
        let now = [p(10, 0, 0, 7_000)];
        assert_eq!(judge(&prev, &now, 8_000, 0, 50), None);
    }

    #[test]
    fn a_stale_quiet_child_is_furniture() {
        // Born of a turn, but hours old and not moving: a helper, not a job.
        let prev = [p(10, 0, 0, 9_000)];
        let now = [p(10, 0, 0, 9_000)];
        assert_eq!(judge(&prev, &now, 8_000, 10_000, 50), None);
    }

    #[test]
    fn pid_reuse_does_not_count_as_growth() {
        let prev = [p(10, 5000, 0, 1_000)];
        let now = [p(10, 10, 0, 2_000)]; // same pid, a different process
        assert_eq!(judge(&prev, &now, 0, 0, 50), None);
    }
}
