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
//! The job sets no limits. What is left in it when a session is done with
//! is ended by `end_leftovers`, which the CLI's own tree kill misses: a
//! process whose parent had already gone. A program with a window of its
//! own — a browser a login opened, an editor — is the user's, not the
//! session's work, and is neither counted nor ended.

#[cfg(windows)]
mod imp {
    use windows::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, FILETIME, HANDLE};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicProcessIdList, QueryInformationJobObject,
        JOBOBJECT_BASIC_PROCESS_ID_LIST,
    };
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, GetProcessIoCounters, GetProcessTimes, OpenProcess, TerminateProcess, IO_COUNTERS,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
    };
    use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId, IsWindowVisible};

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
        edges
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
    pub fn join(_job: &Job, _pid: u32) -> bool {
        false
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

pub use imp::{adopt, alive, join, Job};

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
    /// Read + write + other transfer, cumulative.
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

/// What is left running in a helper's job between its turns, by name: every
/// process, as each was born of a turn, but the CLIs' own helpers
/// (`without_furniture`) and programs with a window of their own.
pub fn left_running(job: &Job) -> Vec<String> {
    let pids: Vec<u32> = children(Some(job), None).iter().map(|r| r.pid).collect();
    let Some(b) = without_furniture(Some(Busy { active: Vec::new(), young: pids })) else { return Vec::new() };
    let mut names: Vec<String> = imp::names(&b.young).into_iter().map(|(_, n)| n).collect();
    names.sort();
    names.dedup();
    names
}

/// Ends what is left in a job once its session or helper is done with: what
/// the CLI's tree kill missed, a process whose parent had gone first. Not
/// `keep` (the CLI, which its own kill takes), and not a program with a
/// window of its own (`children`). Returns the names of those it ended.
pub fn end_leftovers(job: &Job, keep: Option<u32>) -> Vec<String> {
    let left = children(Some(job), keep);
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
/// is the model's doing — a background job it means to come back to — and
/// counts while it lives, however long after its turn: nothing tells a job
/// that waits a day for its moment from one forgotten, and a machine let
/// sleep or shut down on a guess stops work only a person can restart. The
/// MCP servers and shells
/// that came up with the session are furniture and only count while they
/// move, as do the CLIs' own helpers (`without_furniture`).
pub fn judge(prev: &[ProcRead], now: &[ProcRead], born_after_ms: u64, min_cpu_ms: u64) -> Option<Busy> {
    let mut active = Vec::new();
    let mut young = Vec::new();
    for p in now {
        if let Some(q) = prev.iter().find(|q| q.pid == p.pid && q.started_ms == p.started_ms) {
            if p.cpu_ms.saturating_sub(q.cpu_ms) >= min_cpu_ms || p.io_bytes > q.io_bytes {
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

    fn p(pid: u32, cpu_ms: u64, io_bytes: u64, started_ms: u64) -> ProcRead {
        ProcRead { pid, cpu_ms, io_bytes, started_ms }
    }

    #[test]
    fn quiet_furniture_is_not_busy() {
        let prev = [p(10, 100, 1000, 1_000)];
        let now = [p(10, 100, 1000, 1_000)];
        assert_eq!(judge(&prev, &now, 5_000, 50), None);
    }

    #[test]
    fn cpu_growth_is_busy() {
        let prev = [p(10, 100, 0, 1_000)];
        let now = [p(10, 180, 0, 1_000)];
        assert_eq!(judge(&prev, &now, 0, 50).unwrap().active, vec![10]);
    }

    #[test]
    fn io_growth_is_busy_even_without_cpu() {
        let prev = [p(10, 100, 500, 1_000)];
        let now = [p(10, 101, 900, 1_000)];
        assert_eq!(judge(&prev, &now, 0, 50).unwrap().active, vec![10]);
    }

    #[test]
    fn a_sleeping_child_of_a_turn_is_busy() {
        // Born after the first turn started, no CPU at all: `sleep 90` in the background.
        let prev = [p(10, 0, 0, 9_000)];
        let now = [p(10, 0, 0, 9_000)];
        let b = judge(&prev, &now, 8_000, 50).unwrap();
        assert!(b.active.is_empty());
        assert_eq!(b.young, vec![10]);
    }

    #[test]
    fn a_child_older_than_the_first_turn_is_furniture() {
        let prev = [p(10, 0, 0, 7_000)];
        let now = [p(10, 0, 0, 7_000)];
        assert_eq!(judge(&prev, &now, 8_000, 50), None);
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
        assert_eq!(judge(&prev, &now, 0, 50), None);
    }
}
