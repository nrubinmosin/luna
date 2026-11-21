//! What Windows gives: a Job Object per session that its CLI's children inherit,
//! its members and what the CLI's walk finds without one, each process's counters,
//! windows and name, and ending one.

use windows::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, FILETIME, HANDLE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, Thread32First, Thread32Next, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicProcessIdList, JobObjectExtendedLimitInformation, QueryInformationJobObject,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_BASIC_PROCESS_ID_LIST, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows::Win32::System::Threading::{
    GetExitCodeProcess, GetProcessTimes, OpenProcess, OpenThread, QueryFullProcessImageNameW, ResumeThread, TerminateProcess,
    PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA, PROCESS_TERMINATE, THREAD_SUSPEND_RESUME,
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

/// Needed only where a job goes by what is in the environment (keeper/).
pub fn also_marked(_job: &Job, _var: &str, _value: &str) {}

/// So is this.
pub fn await_environment(_pid: u32) {}

/// And this: a Job Object holds what a chat's CLI starts, whatever it does.
pub fn marked(_job: &Job, _id: &str) {}

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

/// A job for a short run that must leave nothing behind (cli/probe.rs): its
/// every member ends with it, by `end` or by its last handle closing, this
/// process's death included (`untied`: not by that). Unlike a session's
/// `Job`, it is never let go, and nothing in it is spared for having a window.
pub struct KillJob(HANDLE);
// A job handle is a kernel object; any thread may use or close it.
unsafe impl Send for KillJob {}
unsafe impl Sync for KillJob {}

impl KillJob {
    pub fn new() -> windows::core::Result<KillJob> {
        let job = KillJob::untied()?;
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: the handle is open, and the struct is the one the class
        // names, of the size passed.
        unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const std::ffi::c_void,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )?;
        }
        Ok(job)
    }

    /// One whose members outlive this process's death, ended only by `end`
    /// or a drop: for a run that must not be cut short when Luna leaves
    /// (claude/models.rs, a CLI that may be saving a renewed token).
    pub fn untied() -> windows::core::Result<KillJob> {
        // SAFETY: a new job handle, owned by the KillJob from here on, which
        // closes it once.
        Ok(KillJob(unsafe { CreateJobObjectW(None, windows::core::PCWSTR::null())? }))
    }

    /// Puts `pid`, created suspended, into the job.
    pub fn assign(&self, pid: u32) -> windows::core::Result<()> {
        // SAFETY: a process handle opened and closed here; the job's is open.
        unsafe {
            let proc_ = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, false, pid)?;
            let assigned = AssignProcessToJobObject(self.0, proc_);
            let _ = CloseHandle(proc_);
            assigned
        }
    }

    /// Ends every process in the job.
    pub fn end(&self) {
        // SAFETY: the job handle is open for as long as `self` lives.
        unsafe {
            let _ = TerminateJobObject(self.0, 1);
        }
    }
}

impl Drop for KillJob {
    fn drop(&mut self) {
        self.end();
        // SAFETY: the handle was returned by CreateJobObjectW and is closed once.
        unsafe {
            let _ = CloseHandle(self.0);
        }
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

/// Every pid in the job right now, the CLI itself included; None when Windows
/// will not say.
fn members(job: &Job) -> Option<Vec<u32>> {
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
            return Some(pids);
        }
        // ERROR_MORE_DATA: the header still says how many there are.
        if assigned > cap && cap < 1 << 16 {
            cap = assigned + 64;
            continue;
        }
        return None;
    }
}

/// What the job holds now, each process read and named; None when the job
/// will not list. One gone between the list and the read is left out.
pub fn list(job: &Job) -> Option<Vec<(super::ProcRead, String)>> {
    Some(named(members(job)?))
}

/// `list` for a CLI that could not be put in a job: the processes below it by
/// parent links.
pub fn list_below(root: u32) -> Option<Vec<(super::ProcRead, String)>> {
    Some(named(descendants(root)?))
}

/// Each pid read and named through a handle of its own. The activity read
/// lists every session's job every few seconds, and a snapshot of every
/// process on the machine (`names`) for the two or three in it was most of
/// what that cost; it is taken only for one whose image name would not read.
fn named(pids: Vec<u32>) -> Vec<(super::ProcRead, String)> {
    let reads: Vec<(super::ProcRead, Option<String>)> = pids.into_iter().filter_map(read_named).collect();
    let unnamed: Vec<u32> = reads.iter().filter(|(_, name)| name.is_none()).map(|(r, _)| r.pid).collect();
    let names = if unnamed.is_empty() { Vec::new() } else { names(&unnamed) };
    reads
        .into_iter()
        .filter_map(|(r, name)| Some((r, name.or_else(|| names.iter().find(|(p, _)| *p == r.pid).map(|(_, n)| n.clone()))?)))
        .collect()
}

/// `read`, with the process's image file name (its last path component) when
/// that reads too.
fn read_named(pid: u32) -> Option<(super::ProcRead, Option<String>)> {
    // SAFETY: a process handle opened and closed here; out-params are plain
    // stack structs, and the name's buffer and length go as a pair.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let (mut c, mut x, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        let times = GetProcessTimes(h, &mut c, &mut x, &mut k, &mut u);
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        let image = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(h);
        times.ok()?;
        let name = image.ok().map(|_| {
            let full = String::from_utf16_lossy(&buf[..len as usize]);
            full.rsplit(['\\', '/']).next().unwrap_or(&full).to_string()
        });
        Some((super::ProcRead { pid, started_ms: filetime_to_unix_ms(c) }, name.filter(|n| !n.is_empty())))
    }
}

/// The pids below `root` by parent links, `root` included. Only for a CLI
/// that could not be put in a job. None when the processes cannot be listed:
/// not known is not empty.
fn descendants(root: u32) -> Option<Vec<u32>> {
    let mut edges: Vec<(u32, u32)> = Vec::new(); // (pid, parent)
                                                 // SAFETY: a snapshot handle owned and closed here; the entry struct is
                                                 // sized before the first call as the API requires.
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return None };
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
    Some(out)
}

fn filetime_100ns(ft: FILETIME) -> u64 {
    ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64
}

/// 100ns intervals since 1601 → unix milliseconds.
fn filetime_to_unix_ms(ft: FILETIME) -> u64 {
    const EPOCH_DIFF: u64 = 116_444_736_000_000_000;
    filetime_100ns(ft).saturating_sub(EPOCH_DIFF) / 10_000
}

/// When one process was created, if it can be opened.
pub fn read(pid: u32) -> Option<super::ProcRead> {
    // SAFETY: a process handle opened and closed here; out-params are
    // plain stack structs.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let (mut c, mut x, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        let times = GetProcessTimes(h, &mut c, &mut x, &mut k, &mut u);
        let _ = CloseHandle(h);
        times.ok()?;
        Some(super::ProcRead { pid, started_ms: filetime_to_unix_ms(c) })
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

/// Executable names by pid, from one snapshot of every process.
fn names(pids: &[u32]) -> Vec<(u32, String)> {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A process named through its own handle is named as the snapshot of
    /// every process names it, and one gone is left out.
    #[test]
    fn a_pid_is_named_as_the_snapshot_names_it() {
        let kids = crate::procs::testkit::Kids(vec![crate::procs::testkit::idler(30)]);
        let pids = vec![std::process::id(), kids.0[0].id()];
        let by_handle = named(pids.clone());
        assert_eq!(by_handle.len(), 2, "{by_handle:?}");
        for (r, name) in &by_handle {
            let snap = names(&[r.pid]);
            assert_eq!(snap.first().map(|(_, n)| n), Some(name), "pid {}", r.pid);
            assert_eq!(Some(r.started_ms), read(r.pid).map(|r| r.started_ms));
        }
        assert!(by_handle[1].1.to_ascii_lowercase().starts_with("ping"), "{by_handle:?}");
        // No process has pid 0 to open: it is not listed.
        assert!(named(vec![0]).is_empty());
    }
}
