//! The tie to the core off Windows: what `procs::tie` is, by other means.
//!
//! Windows ends a job when the core's last handle to it closes. Linux and macOS
//! have no such object, so a small process stands in for it: the keeper, this
//! same exe started with `--keep-ties`, one to a core, once a helper has a turn
//! to tie. It reads a pipe only the core holds the other end of. However the
//! core goes the OS closes that end, so the end of its input is the core's
//! death: it ends every job it was told of and not since told to let go of. A
//! core that leaves lets go of them all first, and the same end of input ends
//! nothing. `PR_SET_PDEATHSIG` would not do: it fires as the *thread* that
//! spawned the process ends, and turns start on threads that do not last; it
//! reaches that one child and nothing it started; macOS has no such thing.
//!
//! The end of its input is not all it goes by: on macOS a pipe becomes
//! close-on-exec a moment after it is made, and a process started in that moment
//! keeps the write end for its life. So it also looks at the core itself now and
//! then, by pid and start time.
//!
//! A keeper can go with its core (`killall -9`, the OOM killer), so a core also
//! writes down what it has tied as it ties it, a file to a job, and the next one
//! ends what is left of those before it takes the helpers up (`end_last_ties`).
//! That alone would keep a resumed turn from overlapping the old one, but it
//! would leave a dead core's turns running until a next core starts, which may be
//! never. A file to a job so that letting go of one is one unlink, which a full
//! disk does not refuse: a released job must not be there for the next core to
//! end. A job not seen out stays written down, for the core after to try again.
//!
//! What is in a job. A process group is not enough: both CLIs start each tool
//! command in a session of its own (Claude Code spawns it `detached`, Codex
//! calls `setsid`), and what such a command leaves behind is re-parented away
//! from the turn. So a job is, at any one look, whatever has its mark in the
//! environment (`MARK`, which goes down through a new session and a new parent),
//! the turns' own processes by pid and start time with their groups, and
//! everything below any of those by parent link. Codex filters what its tools
//! inherit by its config, so there the mark may not get through; `CODEX_THREAD_ID`
//! it sets after the filter, and that counts too (`also_marked`). A turn whose own
//! process does not show its mark is not counted as tied at all (`tie`). Only a
//! process that both cleared its environment and lost its parents gets away.
//!
//! No window spared and none could be: a dead core takes the programs a helper
//! opened, as it does on Windows. Every way out that is not a death lets go.
//!
//! A stop that is chosen ends a tree instead (`end_tree`, for `pty::kill_tree`),
//! which is `taskkill /T` here: the process and all that hangs from it, the tool
//! commands with what they started. The same walk without the mark: what only
//! the mark ties to the job has outlived its tool, and nothing here tells a job
//! left running from a program with a window, so it is left, as `let_go` leaves
//! it.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::io::{Read, Write};
use std::os::fd::FromRawFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

const FLAG: &str = "--keep-ties";
/// Set on a helper's turn, so on all it starts. No KEY, SECRET or TOKEN in the
/// name: Codex keeps those from its tools by default.
pub const MARK: &str = "LUNA_JOB";
/// The keeper's own value of `MARK`: an `end_tree` of its core leaves it, to do
/// its work as for any other death.
const KEEPER: &str = "keeper";
/// How long a look at a job goes on finding what will not die: a process stuck
/// in the kernel. The job then counts as not ended.
const ENDING_TAKES: Duration = Duration::from_secs(5);
const LOOK_AGAIN: Duration = Duration::from_millis(25);
/// Looks at a tree being frozen before it is ended whatever is still moving: what
/// has stopped cannot fork, so only a fork bomb, or one stuck in the kernel and not
/// stopping, takes more than a few.
const FREEZE_LOOKS: usize = 50;
/// How long the keeper listens before it looks at the core itself. The pipe says
/// at once; this is for a pipe something else holds open as well.
const CORE_LOOK_MS: i32 = 2000;
/// A keeper that lasted less than this is not started again by itself.
const KEEPER_LASTS: Duration = Duration::from_secs(5);
/// How long a turn just started may take to have an environment to read.
const ENVIRONMENT_WITHIN: Duration = Duration::from_secs(1);

// -------------------------------------------------------------- the system --

/// One process, as one look found it. One that has exited is not looked at.
#[derive(Clone, Debug, PartialEq)]
struct Proc {
    pid: i32,
    parent: i32,
    group: i32,
    session: i32,
    /// Ticks since boot on Linux, microseconds since the epoch on macOS: only
    /// ever compared with its like, on this machine.
    started: u64,
    name: String,
}

#[cfg(target_os = "linux")]
mod os {
    use super::Proc;

    pub fn pids() -> Option<Vec<i32>> {
        let dir = std::fs::read_dir("/proc").ok()?;
        Some(dir.flatten().filter_map(|e| e.file_name().to_str()?.parse().ok()).collect())
    }

    pub fn read(pid: i32) -> Option<Proc> {
        let stat = std::fs::read(format!("/proc/{pid}/stat")).ok()?;
        parse_stat(pid, &String::from_utf8_lossy(&stat))
    }

    pub fn parse_stat(pid: i32, stat: &str) -> Option<Proc> {
        // The name is in parentheses and may hold anything, those included.
        let (name, rest) = stat.split_once('(')?.1.rsplit_once(')')?;
        let mut fields = rest.split_ascii_whitespace();
        if matches!(fields.next()?, "Z" | "X" | "x") {
            return None;
        }
        let parent = fields.next()?.parse().ok()?;
        let group = fields.next()?.parse().ok()?;
        let session = fields.next()?.parse().ok()?;
        // Field 22 of proc(5); those were 3 to 6.
        let started = fields.nth(15)?.parse().ok()?;
        Some(Proc { pid, parent, group, session, started, name: name.to_string() })
    }

    /// One the same user may read, which leaves out whatever changed user.
    pub fn environment(pid: i32) -> Option<Vec<u8>> {
        std::fs::read(format!("/proc/{pid}/environ")).ok()
    }

    /// Whether it has exited or its pid is another's. Only on the word of the
    /// system: a look that fails otherwise (out of descriptors) is no death.
    pub fn gone(pid: i32, started: u64) -> bool {
        match std::fs::read(format!("/proc/{pid}/stat")) {
            Ok(stat) => parse_stat(pid, &String::from_utf8_lossy(&stat)).is_none_or(|p| p.started != started),
            Err(e) => matches!(e.raw_os_error(), Some(libc::ENOENT | libc::ESRCH)),
        }
    }

    /// Whether no thread of it can start anything any more: a process has stopped
    /// only as each of its threads takes the signal, and any of them may be the one
    /// forking (Codex starts its tools from a worker thread). One that cannot be
    /// looked at has gone, or leaves this to go by what the look finds.
    pub fn stopped(pid: i32) -> bool {
        let Ok(threads) = std::fs::read_dir(format!("/proc/{pid}/task")) else { return true };
        threads.flatten().all(|thread| {
            std::fs::read(thread.path().join("stat")).map_or(true, |stat| {
                let stat = String::from_utf8_lossy(&stat);
                stat.rsplit_once(')')
                    .and_then(|(_, rest)| rest.split_ascii_whitespace().next())
                    .is_none_or(|state| matches!(state, "T" | "t" | "Z" | "X" | "x"))
            })
        })
    }

    /// Ticks since boot start over at a boot, so a pid and a start time say
    /// nothing of a process written down before it.
    pub fn boot_id() -> Option<String> {
        Some(std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?.trim().to_string())
    }

    /// To that process and no other: a pidfd stays with a process whatever
    /// becomes of its number, so what is read after opening one is what will
    /// be signaled.
    pub fn signal(p: &Proc, which: i32) {
        // SAFETY: plain system calls; the descriptor opened here is closed here,
        // and a null siginfo is what a plain signal takes.
        unsafe {
            let fd = libc::syscall(libc::SYS_pidfd_open, p.pid, 0);
            if fd < 0 {
                // A kernel before 5.3: as close to the signal as it gets.
                if super::is_still(p) {
                    libc::kill(p.pid, which);
                }
                return;
            }
            if super::is_still(p) {
                libc::syscall(libc::SYS_pidfd_send_signal, fd, which, std::ptr::null::<libc::siginfo_t>(), 0);
            }
            libc::close(fd as i32);
        }
    }
}

#[cfg(target_os = "macos")]
mod os {
    use super::Proc;
    use std::mem::{size_of, zeroed};
    use std::os::raw::{c_char, c_int};
    use std::ptr::null_mut;

    /// `SSTOP` in <sys/proc.h>.
    const STOPPED: u32 = 4;
    /// `SZOMB` in <sys/proc.h>.
    const ZOMBIE: u32 = 5;

    pub fn pids() -> Option<Vec<i32>> {
        // SAFETY: the first call only asks for a count; the second writes at
        // most the size passed into a buffer of that size.
        unsafe {
            let count = libc::proc_listallpids(null_mut(), 0);
            if count <= 0 {
                return None;
            }
            // Room for what is born between the two calls.
            let mut pids = vec![0i32; count as usize + 64];
            let got = libc::proc_listallpids(pids.as_mut_ptr().cast(), (pids.len() * size_of::<i32>()) as c_int);
            if got <= 0 {
                return None;
            }
            pids.truncate(got as usize);
            Some(pids)
        }
    }

    /// The system's account of a process, or the error it gave.
    fn info(pid: i32) -> Result<libc::proc_bsdinfo, Option<i32>> {
        let size = size_of::<libc::proc_bsdinfo>() as c_int;
        // SAFETY: a plain struct of the size passed, all zeroes valid for it.
        unsafe {
            let mut info: libc::proc_bsdinfo = zeroed();
            if libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&mut info as *mut libc::proc_bsdinfo).cast(), size) != size {
                return Err(std::io::Error::last_os_error().raw_os_error());
            }
            Ok(info)
        }
    }

    fn started(info: &libc::proc_bsdinfo) -> u64 {
        info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec
    }

    pub fn read(pid: i32) -> Option<Proc> {
        let info = info(pid).ok()?;
        if info.pbi_status == ZOMBIE {
            return None;
        }
        let text = |name: &[c_char]| {
            String::from_utf8_lossy(&name.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect::<Vec<u8>>()).into_owned()
        };
        let name = Some(text(&info.pbi_name)).filter(|n| !n.is_empty()).unwrap_or_else(|| text(&info.pbi_comm));
        Some(Proc {
            pid,
            parent: info.pbi_ppid as i32,
            group: info.pbi_pgid as i32,
            // SAFETY: a plain query; -1, for one it will not say, leads to nothing.
            session: unsafe { libc::getsid(pid) },
            started: started(&info),
            name,
        })
    }

    /// Whether it has exited or its pid is another's. Only on the word of the
    /// system: a look that fails otherwise is no death.
    pub fn gone(pid: i32, since: u64) -> bool {
        match info(pid) {
            Ok(info) => info.pbi_status == ZOMBIE || started(&info) != since,
            Err(errno) => errno == Some(libc::ESRCH),
        }
    }

    /// Whether it can start nothing any more. One that cannot be looked at has
    /// gone, or leaves this to go by what the look finds.
    pub fn stopped(pid: i32) -> bool {
        info(pid).map_or(true, |info| matches!(info.pbi_status, STOPPED | ZOMBIE))
    }

    /// `kern.bootsessionuuid`, and not `kern.boottime`, which moves with the clock
    /// and would make this boot look like another. None tells nothing, which is
    /// safe: start times here are of the clock and do not repeat across boots.
    pub fn boot_id() -> Option<String> {
        let mut buf = [0u8; 64];
        let mut size = buf.len();
        // SAFETY: writes at most `size` bytes into a buffer of that size.
        let got = unsafe { libc::sysctlbyname(c"kern.bootsessionuuid".as_ptr(), buf.as_mut_ptr().cast(), &mut size, null_mut(), 0) };
        if got != 0 {
            return None;
        }
        let id = String::from_utf8_lossy(&buf[..size]).trim_end_matches('\0').trim().to_string();
        (!id.is_empty()).then_some(id)
    }

    /// There is no pidfd here: as close to the signal as it gets.
    pub fn signal(p: &Proc, which: i32) {
        if super::is_still(p) {
            // SAFETY: a plain signal, to a process just seen to be this one.
            unsafe { libc::kill(p.pid, which) };
        }
    }

    /// The kernel may leave the environment out for a process signed as
    /// restricted, which then goes by its parent links.
    pub fn environment(pid: i32) -> Option<Vec<u8>> {
        let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
        let mut size = 0usize;
        // SAFETY: the first call only asks for a size; the second writes at
        // most `size` bytes into a buffer of that size.
        unsafe {
            if libc::sysctl(mib.as_mut_ptr(), 3, null_mut(), &mut size, null_mut(), 0) != 0 {
                return None;
            }
            let mut buf = vec![0u8; size];
            if libc::sysctl(mib.as_mut_ptr(), 3, buf.as_mut_ptr().cast(), &mut size, null_mut(), 0) != 0 {
                return None;
            }
            super::environment_of_procargs(buf.get(..size)?).map(<[u8]>::to_vec)
        }
    }
}

/// Nothing to look with: `tie` says so, and no helper counts as tied.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod os {
    use super::Proc;
    pub fn pids() -> Option<Vec<i32>> {
        None
    }
    pub fn read(_pid: i32) -> Option<Proc> {
        None
    }
    pub fn environment(_pid: i32) -> Option<Vec<u8>> {
        None
    }
    pub fn gone(_pid: i32, _started: u64) -> bool {
        false
    }
    pub fn stopped(_pid: i32) -> bool {
        true
    }
    pub fn boot_id() -> Option<String> {
        None
    }
    pub fn signal(_p: &Proc, _which: i32) {}
}

fn holds(environment: &[u8], mark: &str) -> bool {
    environment.split(|b| *b == 0).any(|var| var == mark.as_bytes())
}

/// The environment in what `KERN_PROCARGS2` gives: argc, the path it was started
/// by, padding, the arguments, then the environment. Only that part: a command
/// with a job's mark among its arguments (`grep`) is not of the job.
#[cfg(any(target_os = "macos", test))]
fn environment_of_procargs(args: &[u8]) -> Option<&[u8]> {
    let argc = i32::from_ne_bytes(args.get(..4)?.try_into().ok()?);
    let rest = args.get(4..)?;
    let mut at = rest.iter().position(|b| *b == 0)?;
    while rest.get(at) == Some(&0) {
        at += 1;
    }
    for _ in 0..argc {
        at += rest.get(at..)?.iter().position(|b| *b == 0)? + 1;
    }
    rest.get(at..)
}

/// Whether a pid is still the process that was looked at.
fn is_still(p: &Proc) -> bool {
    os::read(p.pid).is_some_and(|now| now.started == p.started)
}

// ------------------------------------------------------------ what a job is --

/// A turn's process. Its pid may be another's by now; the start time tells.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
struct Leader {
    pid: i32,
    started: u64,
}

fn leader(pid: u32) -> Option<Leader> {
    os::read(pid as i32).map(|p| Leader { pid: p.pid, started: p.started })
}

/// A tied job, as told to the keeper and written down for the next core.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Record {
    id: String,
    mark: String,
    /// The boot it was written in: of another, all it names ended with that.
    #[serde(default)]
    boot: Option<String>,
    /// What else in an environment says it is of this job (`also_marked`): for a
    /// process born since the job began, as what a core let go of may have it too.
    #[serde(default)]
    also: Option<String>,
    #[serde(default)]
    since: u64,
    leaders: Vec<Leader>,
}

/// Every process but `me` and init, which are never of a job or a tree, nor is
/// anything only through them.
fn others(all: &[Proc], me: i32) -> Vec<&Proc> {
    all.iter().filter(|p| p.pid > 1 && p.pid != me).collect()
}

/// `found` and all that hangs from it at one look: what it started, and what is
/// in a group or session one of it leads. The CLIs' tool commands lead both, so
/// a job a tool put in the background is still in them once its parent has gone;
/// the number of a group or session with anyone in it is not given out again.
/// Born after whatever it hangs from, since a parent's pid may be another's by
/// now (`procs::shown`).
fn and_below<'a>(all: &[&'a Proc], mut found: Vec<&'a Proc>) -> Vec<&'a Proc> {
    loop {
        let more: Vec<&Proc> = all
            .iter()
            .copied()
            .filter(|c| !found.iter().any(|f| f.pid == c.pid))
            .filter(|c| found.iter().any(|f| [c.parent, c.group, c.session].contains(&f.pid) && c.started >= f.started))
            .collect();
        if more.is_empty() {
            return found;
        }
        found.extend(more);
    }
}

/// What a job holds at one look.
fn members<'a>(all: &'a [Proc], leaders: &[Leader], me: i32, marked: impl Fn(&Proc) -> bool) -> Vec<&'a Proc> {
    let all = others(all, me);
    let led = |p: &Proc| leaders.iter().any(|l| l.pid == p.pid && l.started == p.started);
    let seeds = all.iter().copied().filter(|p| led(p) || marked(p)).collect();
    and_below(&all, seeds)
}

/// The tree of `root` at one look. With `root` gone what it started has other
/// parents, and what is left of its group is all that names it: nobody else can
/// have that number while the group has anyone in it. `child_of` is for a caller
/// that only ever means a child of its own, which stays its child until reaped,
/// so that a live pid with another parent is some other program's by now.
fn tree(all: &[Proc], root: i32, me: i32, child_of: Option<i32>) -> Vec<&Proc> {
    let all = others(all, me);
    let seeds = match all.iter().copied().find(|p| p.pid == root) {
        Some(top) if child_of.is_some_and(|parent| top.parent != parent) => return Vec::new(),
        Some(top) => vec![top],
        None => all.iter().copied().filter(|p| p.group == root).collect(),
    };
    and_below(&all, seeds)
}

/// Ends all that `look` finds. Each is stopped first, from the top down, and
/// looked below again, as a process killed outright would leave what it forked
/// that moment with another parent and out of the walk. Each signal goes to the
/// process looked at and no other (`os::signal`), so no group is signaled by its
/// number, which may be another's by the time it arrives. A signal sent is not a
/// process stopped: one may yet finish the fork it is in the middle of, so a look
/// that finds nothing new counts only if all were still by the time it began.
/// Returns what it ended; None with nothing to look with.
fn freeze_and_end(look: impl Fn(&[Proc]) -> Vec<Proc>) -> Option<Vec<Proc>> {
    let mut frozen: Vec<Proc> = Vec::new();
    for _ in 0..FREEZE_LOOKS {
        let all_still = frozen.iter().all(|p| !is_still(p) || os::stopped(p.pid));
        let all: Vec<Proc> = os::pids()?.into_iter().filter_map(os::read).collect();
        let new: Vec<Proc> = look(&all).into_iter().filter(|p| !frozen.iter().any(|f| f.pid == p.pid && f.started == p.started)).collect();
        if new.is_empty() {
            if all_still {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        for p in new {
            os::signal(&p, libc::SIGSTOP);
            frozen.push(p);
        }
    }
    for p in &frozen {
        os::signal(p, libc::SIGKILL);
    }
    Some(frozen)
}

/// Ends a process and all that hangs from it, the way `taskkill /T` does: what
/// has a window goes too if it still hangs from the tree.
pub fn end_tree(root: u32) {
    let (root, me) = (root as i32, std::process::id() as i32);
    if root <= 1 || root == me {
        return;
    }
    // Nothing to look with: the group and the process itself, as a last resort.
    if os::pids().is_none() {
        // SAFETY: plain signals, to the pid its owner asked to end.
        unsafe {
            libc::killpg(root, libc::SIGKILL);
            libc::kill(root, libc::SIGKILL);
        }
        return;
    }
    // The window also kills a core that will not stop, which need not be its own
    // child; the core only ever means a CLI it started.
    let child_of = crate::lifecycle::is_core().then_some(me);
    let keeper = format!("{MARK}={KEEPER}");
    freeze_and_end(|all| {
        tree(all, root, me, child_of)
            .into_iter()
            .filter(|p| !os::environment(p.pid).is_some_and(|env| holds(&env, &keeper)))
            .cloned()
            .collect()
    });
}

/// Ends all a job holds, looking until nothing is left or `within` is up.
/// Returns whether nothing is left, and the names of what it ended.
/// What the job's second mark (`also`) names, at one look: the processes that
/// have it and were born since the job began. Taken as soon as the core is seen
/// gone, and kept: one that comes by it later is no turn's of that core, but of a
/// later core's that took the same conversation up.
fn thread_marked(job: &Record) -> HashSet<(i32, u64)> {
    let Some(also) = job.also.as_deref() else { return HashSet::new() };
    let me = std::process::id() as i32;
    let all: Vec<Proc> = os::pids().unwrap_or_default().into_iter().filter_map(os::read).collect();
    others(&all, me)
        .into_iter()
        .filter(|p| p.started >= job.since && os::environment(p.pid).is_some_and(|env| holds(&env, also)))
        .map(|p| (p.pid, p.started))
        .collect()
}

/// Ends the job: what has its mark, what its leaders are, `by_thread`
/// (`thread_marked`), and all that hangs from any of it.
fn end_job(job: &Record, by_thread: &HashSet<(i32, u64)>, within: Duration) -> (bool, Vec<String>) {
    let me = std::process::id() as i32;
    let mark = format!("{MARK}={}", job.mark);
    let marked = |p: &Proc| by_thread.contains(&(p.pid, p.started)) || os::environment(p.pid).is_some_and(|env| holds(&env, &mark));
    let deadline = Instant::now() + within;
    let mut ended: Vec<String> = Vec::new();
    loop {
        let Some(found) = freeze_and_end(|all| members(all, &job.leaders, me, marked).into_iter().cloned().collect()) else {
            return (false, ended);
        };
        if found.is_empty() {
            return (true, ended);
        }
        for p in found {
            if !ended.contains(&p.name) {
                ended.push(p.name);
            }
        }
        if Instant::now() >= deadline {
            return (false, ended);
        }
        std::thread::sleep(LOOK_AGAIN);
    }
}

fn report(job: &Record, gone: bool, ended: &[String], why: &str) {
    if !gone {
        crate::log::error("tie", &format!("{}: {why}, and not all of its job could be ended (of {})", job.id, ended.join(", ")));
    } else if !ended.is_empty() {
        crate::log::warn("tie", &format!("{}: {why}: ended {}", job.id, ended.join(", ")));
    }
}

// -------------------------------------------------------------- the keeper --

#[derive(Debug, PartialEq, Serialize, Deserialize)]
enum Word {
    /// Who the core is, for the keeper to look at. The first word it hears.
    Core(Leader),
    Tie(Record),
    LetGo(String),
}

fn say(to: &mut impl Write, word: &Word) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(word)?;
    line.push(b'\n');
    to.write_all(&line)
}

/// `luna --keep-ties --log <path>`: runs as the keeper and returns true, or
/// false for an ordinary launch.
pub fn from_args() -> bool {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some(FLAG) {
        return false;
    }
    if let Some(i) = args.iter().position(|a| a == "--log") {
        if let Some(p) = args.get(i + 1) {
            crate::log::set_path(PathBuf::from(p));
        }
    }
    crate::log::set_role("keeper");
    keep();
    true
}

/// What the keeper has heard so far.
#[derive(Default)]
struct Heard {
    core: Option<Leader>,
    jobs: BTreeMap<String, Record>,
    partial: Vec<u8>,
}

impl Heard {
    fn take(&mut self, bytes: &[u8]) {
        self.partial.extend_from_slice(bytes);
        while let Some(end) = self.partial.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.partial.drain(..=end).collect();
            match serde_json::from_slice(&line) {
                Ok(Word::Core(core)) => self.core = Some(core),
                Ok(Word::Tie(job)) => {
                    self.jobs.insert(job.mark.clone(), job);
                }
                Ok(Word::LetGo(mark)) => {
                    self.jobs.remove(&mark);
                }
                Err(e) => crate::log::warn("tie", &format!("a word the keeper could not read: {e}")),
            }
        }
    }
}

/// Whether there is something to read on its input within `ms`, its end included.
fn input_within(ms: i32) -> bool {
    let mut input = libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 };
    // SAFETY: one pollfd, as the count says, that outlives the call.
    unsafe { libc::poll(&mut input, 1, ms) > 0 }
}

/// Listens until the core is gone: the end of its input, or the core itself looked
/// at. Its last words are in the pipe by then, whoever else holds the other end.
fn listen(heard: &mut Heard) {
    // SAFETY: the pipe the core started it on, owned by nothing else in this process.
    let mut input = unsafe { std::fs::File::from_raw_fd(0) };
    let mut buf = [0u8; 8192];
    let mut leaving = false;
    loop {
        if input_within(if leaving { 0 } else { CORE_LOOK_MS }) {
            match input.read(&mut buf) {
                Ok(0) => return,
                Ok(n) => heard.take(&buf[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => return,
            }
        } else if leaving {
            return;
        } else {
            leaving = heard.core.is_some_and(|core| os::gone(core.pid, core.started));
        }
    }
}

fn keep() {
    // Only the core's going is the core's death. A signal sent by name (`killall
    // luna`) would otherwise end this one along with the core, and what it keeps
    // would run on.
    for signal in [libc::SIGHUP, libc::SIGINT, libc::SIGTERM] {
        // SAFETY: ignoring a signal touches nothing of this program's.
        unsafe { libc::signal(signal, libc::SIG_IGN) };
    }
    let mut heard = Heard::default();
    listen(&mut heard);
    // Every job's at once, before any is ended, which can take a while each.
    let marked: Vec<(&Record, HashSet<(i32, u64)>)> = heard.jobs.values().map(|job| (job, thread_marked(job))).collect();
    for (job, by_thread) in marked {
        let (gone, ended) = end_job(job, &by_thread, ENDING_TAKES);
        report(job, gone, &ended, "the core went");
    }
}

fn keeper_command() -> std::io::Result<Command> {
    #[cfg(not(test))]
    let mut cmd = {
        // On Linux the running file itself, which its path may no longer name:
        // it can be replaced under a running core (corelink.rs).
        let exe = if cfg!(target_os = "linux") { PathBuf::from("/proc/self/exe") } else { std::env::current_exe()? };
        let mut cmd = Command::new(exe);
        cmd.arg(FLAG).arg("--log").arg(crate::log::path()).arg0("luna-keeper");
        cmd
    };
    // The tests' keeper is this test binary, by way of a test of its own.
    #[cfg(test)]
    let mut cmd = {
        let mut cmd = Command::new(std::env::current_exe()?);
        cmd.args(["--exact", "keeper::tests::keeper_entry", "--ignored", "--nocapture"]).env("LUNA_KEEPER_TEST", "1");
        cmd
    };
    // A group of its own, so that a signal to the core's does not reach it; not
    // in any project folder, so that it never holds one open.
    cmd.current_dir("/").env(MARK, KEEPER).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).process_group(0);
    Ok(cmd)
}

// ------------------------------------------------------------ on the core --

struct Ties {
    records: BTreeMap<String, Record>,
    /// The write end, which closes when the core ends or never before: a live
    /// keeper whose pipe closes takes that for the core's death.
    keeper: Option<ChildStdin>,
    generation: u64,
}

static TIES: Mutex<Ties> = Mutex::new(Ties { records: BTreeMap::new(), keeper: None, generation: 0 });

fn ties() -> MutexGuard<'static, Ties> {
    TIES.lock().unwrap_or_else(|e| e.into_inner())
}

/// In the data dir, written on every change: `helpers.json` goes by a two-second
/// beat, and a core killed within it must still leave this behind.
fn records_dir() -> PathBuf {
    crate::paths::data_dir().join("tied-jobs")
}

fn record_file(dir: &Path, mark: &str) -> PathBuf {
    let name: String = mark.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    dir.join(format!("{name}.json"))
}

/// One that cannot be written is not left saying less than is so: with none at
/// all the next core does not count the job as over.
fn write_record(dir: &Path, job: &Record) {
    let path = record_file(dir, &job.mark);
    let tmp = path.with_extension("tmp");
    // Made private as it is made (`secret::create_private`), so the one a
    // write that died left is cleared first.
    let _ = std::fs::remove_file(&tmp);
    let written = std::fs::create_dir_all(dir)
        .and_then(|()| serde_json::to_vec(job).map_err(std::io::Error::other))
        .and_then(|bytes| crate::secret::create_private(&tmp).and_then(|mut f| f.write_all(&bytes)))
        .and_then(|()| std::fs::rename(&tmp, &path));
    if let Err(e) = written {
        crate::log::warn("tie", &format!("could not write {}: {e}", path.display()));
        let _ = std::fs::remove_file(&tmp);
        let _ = std::fs::remove_file(&path);
    }
}

/// A job let go of must not be there for the next core to end.
fn remove_record(dir: &Path, mark: &str) {
    let path = record_file(dir, mark);
    match std::fs::remove_file(&path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            crate::log::error("tie", &format!("could not remove {}, and the next core will end that job: {e}", path.display()));
        }
        _ => {}
    }
}

fn start_keeper(ties: &mut Ties) -> bool {
    let mut child = match keeper_command().and_then(|mut cmd| cmd.spawn()) {
        Ok(child) => child,
        Err(e) => {
            crate::log::warn("tie", &format!("could not start the keeper: {e}"));
            return false;
        }
    };
    let Some(mut pipe) = child.stdin.take() else { return false };
    let core = leader(std::process::id()).map(Word::Core);
    if !core
        .iter()
        .chain(&ties.records.values().map(|job| Word::Tie(job.clone())).collect::<Vec<_>>())
        .all(|word| say(&mut pipe, word).is_ok())
    {
        crate::log::warn("tie", "the keeper went as it started");
        let _ = child.kill();
        let _ = child.wait();
        return false;
    }
    ties.generation += 1;
    let generation = ties.generation;
    ties.keeper = Some(pipe);
    if let Err(e) = std::thread::Builder::new().name("luna-keeper".into()).spawn(move || see_out(child, generation)) {
        crate::log::warn("tie", &format!("nothing to watch the keeper with: {e}"));
    }
    true
}

/// Reaps a keeper that went while its core runs, and starts another on all that
/// is still tied. A keeper that does not last is left for the next word to try,
/// and the next core stands behind it.
fn see_out(mut child: Child, generation: u64) {
    let since = Instant::now();
    let _ = child.wait();
    let mut ties = ties();
    if ties.generation != generation {
        return;
    }
    ties.keeper = None;
    crate::log::warn("tie", "the keeper went while the core runs");
    if since.elapsed() >= KEEPER_LASTS && !ties.records.is_empty() {
        start_keeper(&mut ties);
    }
}

/// Whether the keeper knows now. A write fails only on a keeper that is gone,
/// so replacing it here never drops the pipe of a live one.
fn tell(word: Word) -> bool {
    let mut ties = ties();
    match &word {
        Word::Core(_) => {}
        Word::Tie(job) => {
            ties.records.insert(job.mark.clone(), job.clone());
            write_record(&records_dir(), job);
        }
        Word::LetGo(mark) => {
            ties.records.remove(mark);
            remove_record(&records_dir(), mark);
        }
    }
    if ties.keeper.as_mut().is_some_and(|keeper| say(keeper, &word).is_ok()) {
        return true;
    }
    ties.keeper = None;
    (matches!(word, Word::LetGo(_)) && ties.records.is_empty()) || start_keeper(&mut ties)
}

/// What a turn of helper `id` is to have in its environment, before it starts.
/// This core's own: a program a helper left running past a core that let go of
/// it is no part of the job the same helper has under the next.
pub fn mark_of(id: &str) -> String {
    static CORE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let core = CORE.get_or_init(|| format!("{}-{}", std::process::id(), crate::throttle::now_ms()));
    format!("{core}-{id}")
}

/// A session's turns' processes, and whether they are tied.
pub struct Job(Mutex<JobState>);

struct JobState {
    leaders: Vec<Leader>,
    /// When its first turn began.
    since: u64,
    also: Option<String>,
    /// The helper the keeper has it for, until `let_go`.
    tied_for: Option<String>,
}

impl JobState {
    fn record(&self, id: &str) -> Record {
        Record {
            id: id.to_string(),
            mark: mark_of(id),
            boot: os::boot_id(),
            also: self.also.clone(),
            since: self.since,
            leaders: self.leaders.clone(),
        }
    }
}

impl Job {
    fn state(&self) -> MutexGuard<'_, JobState> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        let_go(self);
    }
}

/// A job for a process just started, which leads a group of its own. Nothing
/// runs and nothing is told until `tie`: a chat's job is never tied.
pub fn adopt(pid: u32) -> Option<Job> {
    let first = leader(pid);
    Some(Job(Mutex::new(JobState {
        leaders: first.into_iter().collect(),
        since: first.map_or(0, |l| l.started),
        also: None,
        tied_for: None,
    })))
}

/// For straight after a `spawn`, before `tie` or `join` and with nothing held, as
/// this can take a while on a busy machine. `spawn` returns as the exec replaces the
/// process's memory, before the new stack is laid out, and until it is Linux reads
/// the environment as empty, which `shows_mark` would take for a mark hidden. Linux
/// only: elsewhere an empty one may be all there will ever be (`os::environment`).
pub fn await_environment(pid: u32) {
    if !cfg!(target_os = "linux") {
        return;
    }
    let pid = pid as i32;
    let deadline = Instant::now() + ENVIRONMENT_WITHIN;
    // One that has exited reads as empty for good, and `read` does not look at it.
    while os::read(pid).is_some() && os::environment(pid).is_some_and(|env| env.is_empty()) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Whether a turn's process shows its mark, or has already gone, showing nothing
/// and hiding nothing. Where it does not (macOS hides the environment of a
/// restricted binary), what the turn leaves behind could not be told either.
fn shows_mark(turn: &Leader, id: &str) -> bool {
    let mark = format!("{MARK}={}", mark_of(id));
    let shown = os::environment(turn.pid).is_some_and(|env| holds(&env, &mark)) || os::gone(turn.pid, turn.started);
    if !shown {
        crate::log::warn("tie", &format!("{id}: its turn does not show its mark: not counted as tied, though it still goes with the core"));
    }
    shown
}

/// Has the keeper end the job with the core. True only once the keeper has it,
/// where there is a way to look at what a job holds, and its turn can be seen to
/// carry the mark: what says the job is over when the next core takes it up.
pub fn tie(job: &Job, id: &str) -> bool {
    if os::pids().is_none_or(|pids| pids.is_empty()) {
        crate::log::warn("tie", "no way to list processes here: helpers are not tied to the core");
        return false;
    }
    let mut state = job.state();
    if !tell(Word::Tie(state.record(id))) {
        // Not tied, so nothing lets go of it later: the next core must not end it.
        tell(Word::LetGo(mark_of(id)));
        return false;
    }
    state.tied_for = Some(id.to_string());
    state.leaders.iter().all(|turn| shows_mark(turn, id))
}

/// A later turn of the same helper, whose mark it has too. The turns already
/// over need no keeping: what they left has the mark, and a helper has turns
/// without number.
pub fn join(job: &Job, pid: u32) -> bool {
    let mut state = job.state();
    state.leaders.retain(|l| os::read(l.pid).is_some_and(|p| p.started == l.started));
    let turn = leader(pid);
    state.leaders.extend(turn);
    match state.tied_for.clone() {
        Some(id) => tell(Word::Tie(state.record(&id))) && turn.is_none_or(|turn| shows_mark(&turn, &id)),
        None => true,
    }
}

/// Something else in the environment that says a process is of this job, once it
/// is known: `CODEX_THREAD_ID`, which Codex sets on its tools after its
/// `shell_environment_policy`, whose `include_only` no setting of ours gets past.
pub fn also_marked(job: &Job, var: &str, value: &str) {
    let mut state = job.state();
    let also = format!("{var}={value}");
    if state.also.as_deref() != Some(&also) {
        state.also = Some(also);
        if let Some(id) = state.tied_for.clone() {
            tell(Word::Tie(state.record(&id)));
        }
    }
}

/// Undoes `tie`: what is in the job runs on past the core.
pub fn let_go(job: &Job) {
    let id = job.state().tied_for.take();
    if let Some(id) = id {
        tell(Word::LetGo(mark_of(&id)));
    }
}

/// What became of the jobs the last core left written down.
pub struct LastTies {
    over: HashSet<String>,
}

impl LastTies {
    /// Whether a helper's tied job was seen out: nothing of it is left.
    pub fn held(&self, id: &str) -> bool {
        self.over.contains(id)
    }

    /// Whether a job an earlier core could not see out has been now.
    pub fn confirmed(&self, id: &str) -> bool {
        self.over.contains(id)
    }
}

/// Ends what is left of the jobs earlier cores tied and did not let go of, which a
/// keeper has done unless it went too. For taking the helpers up.
pub fn end_last_ties() -> LastTies {
    end_records(&records_dir(), ENDING_TAKES, os::boot_id().as_deref())
}

/// A job's file goes once nothing of it is left, and only then: one not seen out
/// is tried again by the core after, and until then says nothing of its helper.
/// One of another boot needs no look: that boot took it all, and its pids and
/// start times may by now name something else.
fn end_records(dir: &Path, within: Duration, this_boot: Option<&str>) -> LastTies {
    let (mut over, mut left) = (HashSet::new(), HashSet::new());
    for path in std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()) {
        let job: Option<Record> = (path.extension().is_some_and(|e| e == "json"))
            .then(|| std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()))
            .flatten();
        let Some(job) = job else {
            // Half a write or an unreadable file: it names nothing to act on, and
            // with nothing written its helper does not count as seen out.
            let _ = std::fs::remove_file(&path);
            continue;
        };
        let of_another_boot = matches!((job.boot.as_deref(), this_boot), (Some(then), Some(now)) if then != now);
        let gone = of_another_boot || {
            let (gone, ended) = end_job(&job, &thread_marked(&job), within);
            report(&job, gone, &ended, "the last core's keeper did not see to it");
            gone
        };
        if gone {
            let _ = std::fs::remove_file(&path);
            over.insert(job.id);
        } else {
            left.insert(job.id);
        }
    }
    over.retain(|id| !left.contains(id));
    LastTies { over }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};

    fn p(pid: i32, parent: i32, group: i32, started: u64) -> Proc {
        Proc { pid, parent, group, session: 0, started, name: format!("p{pid}") }
    }

    fn in_session(session: i32, p: Proc) -> Proc {
        Proc { session, ..p }
    }

    fn pids(found: Vec<&Proc>) -> Vec<i32> {
        let mut pids: Vec<i32> = found.iter().map(|p| p.pid).collect();
        pids.sort();
        pids
    }

    /// The tests' keeper (`keeper_command`), not a test of anything.
    #[test]
    #[ignore]
    fn keeper_entry() {
        if std::env::var_os("LUNA_KEEPER_TEST").is_some() {
            keep();
        }
    }

    #[test]
    fn a_job_is_its_leader_its_group_its_mark_and_all_below_them() {
        let all = [
            p(1, 0, 1, 1),
            p(100, 1, 100, 500),   // the core
            p(200, 100, 200, 600), // the turn's CLI
            p(201, 200, 200, 610), // in its group
            p(210, 200, 210, 620), // a tool in a session of its own
            p(211, 210, 210, 630), // what that started
            p(300, 1, 300, 640),   // left behind, re-parented: only the mark says
            p(301, 300, 300, 650), // and below that
            p(400, 1, 400, 660),   // someone else's
        ];
        let leaders = [Leader { pid: 200, started: 600 }];
        let found = members(&all, &leaders, 999, |p| p.pid == 300);
        assert_eq!(pids(found), [200, 201, 210, 211, 300, 301]);
    }

    #[test]
    fn a_pid_taken_by_another_process_is_not_the_jobs() {
        // Pid 200 is another's now, with a group of the same number and a child.
        let all = [p(200, 1, 200, 900), p(201, 200, 200, 910)];
        let leaders = [Leader { pid: 200, started: 600 }];
        assert!(members(&all, &leaders, 999, |_| false).is_empty());
        // A child older than its parent's pid was another's child.
        let all = [p(200, 1, 200, 600), p(201, 200, 700, 500)];
        assert_eq!(pids(members(&all, &leaders, 999, |_| false)), [200]);
    }

    #[test]
    fn neither_this_process_nor_init_is_ever_of_a_job_nor_what_is_only_through_them() {
        let all = [p(1, 0, 1, 1), p(50, 1, 50, 10), p(51, 50, 50, 20)];
        assert_eq!(pids(members(&all, &[], 50, |_| true)), [51], "marked in its own right");
        assert!(members(&all, &[], 50, |p| p.pid == 1 || p.pid == 50).is_empty());
    }

    /// The core 100, a chat's or a helper's CLI 200 under it, and all a turn does.
    fn a_tree() -> Vec<Proc> {
        vec![
            p(100, 1, 100, 500),
            p(200, 100, 200, 600),
            p(201, 200, 200, 610),                  // in its group
            in_session(210, p(210, 200, 210, 620)), // a tool in a session of its own
            in_session(210, p(211, 210, 211, 630)), // a job its shell gave a group of its own
            in_session(210, p(212, 1, 213, 640)),   // parent gone, in the live tool's session
            in_session(210, p(214, 1, 210, 650)),   // parent gone, in the live tool's group
            in_session(300, p(301, 1, 300, 660)),   // outlived a tool that has exited
            p(400, 1, 400, 670),                    // someone else's
        ]
    }

    #[test]
    fn a_tree_is_all_that_hangs_from_its_root_and_nothing_that_outlived_its_tool() {
        assert_eq!(pids(tree(&a_tree(), 200, 100, Some(100))), [200, 201, 210, 211, 212, 214]);
    }

    #[test]
    fn with_its_root_gone_a_tree_is_what_is_left_of_its_group() {
        let all: Vec<Proc> = a_tree().into_iter().filter(|p| p.pid != 200).collect();
        assert_eq!(pids(tree(&all, 200, 100, Some(100))), [201], "the tool's parent link went with the root");
        assert!(tree(&all, 999, 100, Some(100)).is_empty());
    }

    #[test]
    fn a_live_pid_that_is_no_child_of_the_core_is_some_other_programs() {
        let mut all = a_tree();
        all[1].parent = 1;
        assert!(tree(&all, 200, 100, Some(100)).is_empty(), "the pid is another's, and so is the group of that number");
        // The window, killing a core it may not have started.
        assert_eq!(pids(tree(&a_tree(), 100, 42, None)), [100, 200, 201, 210, 211, 212, 214]);
    }

    #[test]
    fn a_process_older_than_what_it_would_hang_from_does_not() {
        let all = [p(200, 100, 200, 600), p(201, 200, 300, 500), p(202, 1, 200, 500), in_session(200, p(203, 1, 203, 500))];
        assert_eq!(pids(tree(&all, 200, 1, None)), [200]);
    }

    #[test]
    fn an_environment_holds_a_mark_whole_or_not_at_all() {
        let env = b"PATH=/bin\0LUNA_JOB=7-8-c1\0HOME=/h\0";
        assert!(holds(env, "LUNA_JOB=7-8-c1"));
        assert!(!holds(env, "LUNA_JOB=7-8-c"));
        assert!(!holds(env, "LUNA_JOB=7-8-c11"));
        assert!(holds(b"LUNA_JOB=x", "LUNA_JOB=x"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_stat_line_is_read_past_a_name_of_any_shape() {
        let stat = "123 (a) b (c)) S 45 67 68 0 -1 4194560 1 0 0 0 0 0 0 0 20 0 1 0 8910 1000 1 2";
        assert_eq!(
            os::parse_stat(123, stat),
            Some(Proc { pid: 123, parent: 45, group: 67, session: 68, started: 8910, name: "a) b (c)".into() })
        );
        assert_eq!(os::parse_stat(123, &stat.replace(" S ", " Z ")), None, "exited, nothing to end");
    }

    #[test]
    fn a_word_to_the_keeper_is_one_line() {
        let word = Word::Tie(Record {
            id: "c 1".into(),
            mark: "m".into(),
            boot: None,
            also: None,
            since: 0,
            leaders: vec![Leader { pid: 2, started: 3 }],
        });
        let mut out = Vec::new();
        say(&mut out, &word).unwrap();
        assert_eq!(out.iter().filter(|b| **b == b'\n').count(), 1);
        assert_eq!(out.last(), Some(&b'\n'));
        assert_eq!(serde_json::from_slice::<Word>(&out).unwrap(), word);
    }

    /// A turn as the CLIs run one: leading a group of its own, with a job in that
    /// group, a tool in a session of its own with the mark cleared, and what a
    /// tool that has since exited left: a new session, a new parent.
    fn a_turn(mark: &str) -> (Child, Vec<i32>) {
        const SETSID: &str = "perl -e 'use POSIX; POSIX::setsid(); exec @ARGV'";
        let script =
            format!("sleep 60 &\necho $!\n(unset {MARK}; exec {SETSID} sleep 60) &\necho $!\n{SETSID} sh -c 'sleep 60 & echo $!'\nwait\n");
        let mut child = Command::new("sh")
            .args(["-c", &script])
            .env(MARK, mark)
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut out = BufReader::new(child.stdout.take().unwrap());
        let mut pids = Vec::new();
        for _ in 0..3 {
            let mut line = String::new();
            out.read_line(&mut line).unwrap();
            pids.push(line.trim().parse().unwrap());
        }
        (child, pids)
    }

    fn record_of(child: &Child, mark: &str) -> Record {
        Record {
            id: format!("id-{mark}"),
            mark: mark.to_string(),
            boot: None,
            also: None,
            since: 0,
            leaders: leader(child.id()).into_iter().collect(),
        }
    }

    /// A folder of the test's own, under the data dir that the test build keeps
    /// beside its exe. The records of the tests that tie for real are elsewhere.
    fn scratch(name: &str) -> PathBuf {
        let dir = crate::paths::data_dir().join("keeper-tests").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn own_keeper() -> (Child, ChildStdin) {
        let mut keeper = keeper_command().unwrap().spawn().unwrap();
        let pipe = keeper.stdin.take().unwrap();
        (keeper, pipe)
    }

    /// Whether `done` comes true in time.
    fn within(ms: u64, done: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_millis(ms);
        while !done() {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        true
    }

    fn all_gone_within(child: &mut Child, pids: &[i32], ms: u64) -> bool {
        let deadline = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < deadline {
            let done = child.try_wait().unwrap().is_some();
            if done && pids.iter().all(|pid| os::read(*pid).is_none()) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    fn clean_up(child: &mut Child, pids: &[i32]) {
        for pid in pids {
            // SAFETY: a plain signal, to a process this test started and has
            // just seen alive; the test ends here either way.
            unsafe { libc::kill(*pid, libc::SIGKILL) };
        }
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn a_tied_job_goes_with_a_death_and_not_with_a_way_out_chosen() {
        // As when a core dies: the pipe closes with nothing let go of.
        let (mut turn, left) = a_turn("death");
        assert_ne!(os::read(left[2]).unwrap().parent, turn.id() as i32, "re-parented: only its mark says");
        let (mut keeper, mut pipe) = own_keeper();
        say(&mut pipe, &Word::Tie(record_of(&turn, "death"))).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        assert!(left.iter().all(|pid| os::read(*pid).is_some()), "the core lives");
        assert_eq!(turn.try_wait().unwrap(), None);
        drop(pipe);
        assert!(all_gone_within(&mut turn, &left, 5000), "what is in it ends, however it got away from the turn");
        keeper.wait().unwrap();

        // A helper deleted or opened as a chat, or a core leaving: let go of first.
        let (mut turn, left) = a_turn("letgo");
        let (mut keeper, mut pipe) = own_keeper();
        say(&mut pipe, &Word::Tie(record_of(&turn, "letgo"))).unwrap();
        say(&mut pipe, &Word::LetGo("letgo".into())).unwrap();
        drop(pipe);
        keeper.wait().unwrap();
        assert!(!all_gone_within(&mut turn, &left, 300));
        assert!(left.iter().all(|pid| os::read(*pid).is_some()), "let go of, its keeper's going ends nothing");
        clean_up(&mut turn, &left);
    }

    #[test]
    fn the_next_core_ends_what_a_dead_keeper_left_and_says_of_which() {
        let (mut turn, left) = a_turn("next-core");
        let job = record_of(&turn, "next-core");
        let dir = scratch("next-core");
        write_record(&dir, &job);
        assert!(end_records(&dir, ENDING_TAKES, None).held(&job.id));
        assert!(all_gone_within(&mut turn, &left, 3000));
        assert!(!record_file(&dir, &job.mark).exists(), "seen out, so no longer written down");
        // Nothing left of one its keeper saw to: over all the same.
        write_record(&dir, &job);
        assert!(end_records(&dir, ENDING_TAKES, None).held(&job.id));
    }

    #[test]
    fn a_job_not_seen_out_stays_written_down_for_the_core_after() {
        let (mut turn, left) = a_turn("not-seen-out");
        let job = record_of(&turn, "not-seen-out");
        let dir = scratch("not-seen-out");
        write_record(&dir, &job);
        // No time to see it die: it counts as still there.
        let first = end_records(&dir, Duration::ZERO, None);
        assert!(!first.held(&job.id) && !first.confirmed(&job.id));
        assert!(record_file(&dir, &job.mark).exists(), "for the next core to try again");
        assert!(all_gone_within(&mut turn, &left, 3000));
        let second = end_records(&dir, ENDING_TAKES, None);
        assert!(second.held(&job.id) && second.confirmed(&job.id));
        assert!(!record_file(&dir, &job.mark).exists());
    }

    #[test]
    fn a_helper_with_two_records_is_over_only_when_both_are() {
        let mut stuck = Command::new("sleep").arg("60").process_group(0).spawn().unwrap();
        let dir = scratch("two-records");
        let old = Record { id: "two".into(), mark: "two-old".into(), boot: None, also: None, since: 0, leaders: vec![] };
        let new = Record { mark: "two-new".into(), leaders: leader(stuck.id()).into_iter().collect(), ..old.clone() };
        write_record(&dir, &old);
        write_record(&dir, &new);
        assert!(!end_records(&dir, Duration::ZERO, None).held("two"));
        let _ = stuck.wait();
    }

    #[test]
    fn a_record_of_another_boot_names_nothing_now() {
        let mut other = Command::new("sleep").arg("60").process_group(0).spawn().unwrap();
        // The very pid and start time a process of the last boot had.
        let job = Record {
            id: "rebooted".into(),
            mark: "rebooted".into(),
            boot: Some("then".into()),
            also: None,
            since: 0,
            leaders: vec![leader(other.id()).unwrap()],
        };
        let dir = scratch("rebooted");
        write_record(&dir, &job);
        let last = end_records(&dir, ENDING_TAKES, Some("now"));
        assert!(last.held("rebooted"), "that boot took it all");
        assert!(!record_file(&dir, &job.mark).exists());
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(other.try_wait().unwrap(), None, "the process with that pid is another's");
        // Of this boot, or of one that cannot be told: it goes by what it names.
        write_record(&dir, &job);
        assert!(end_records(&dir, ENDING_TAKES, Some("then")).held("rebooted"));
        other.wait().unwrap();
    }

    #[test]
    fn a_boot_has_an_id_where_the_system_gives_one() {
        #[cfg(target_os = "linux")]
        assert!(os::boot_id().is_some_and(|id| id.len() > 8));
        assert_eq!(os::boot_id(), os::boot_id());
    }

    #[test]
    fn a_job_let_go_of_is_gone_from_disk_whatever_became_of_the_others() {
        let dir = scratch("released");
        let keep = Record { id: "keep".into(), mark: "keep".into(), boot: None, also: None, since: 0, leaders: vec![] };
        let release = Record { id: "release".into(), mark: "release".into(), ..keep.clone() };
        write_record(&dir, &keep);
        write_record(&dir, &release);
        remove_record(&dir, "release");
        assert!(!record_file(&dir, "release").exists());
        assert!(record_file(&dir, "keep").exists());
        remove_record(&dir, "never-there");
    }

    #[test]
    fn a_record_that_cannot_be_written_is_not_left_saying_less_than_is_so() {
        let dir = scratch("unwritable");
        let job = Record { id: "w".into(), mark: "w".into(), boot: None, also: None, since: 0, leaders: vec![] };
        write_record(&dir, &job);
        assert!(record_file(&dir, "w").exists());
        // A directory where its temporary file goes: the write fails, for root too.
        std::fs::create_dir(record_file(&dir, "w").with_extension("tmp")).unwrap();
        write_record(&dir, &Record { leaders: vec![Leader { pid: 5, started: 6 }], ..job });
        assert!(!record_file(&dir, "w").exists(), "with none at all its helper does not count as seen out");
    }

    #[test]
    fn a_broken_record_names_nothing_and_goes() {
        let dir = scratch("broken");
        std::fs::write(dir.join("half.json"), b"{\"id\":\"x").unwrap();
        std::fs::write(dir.join("half.tmp"), b"{").unwrap();
        let last = end_records(&dir, ENDING_TAKES, None);
        assert!(!last.held("x"));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    }

    #[test]
    fn a_signal_goes_to_the_process_looked_at_and_no_other() {
        let mut other = Command::new("sleep").arg("60").process_group(0).spawn().unwrap();
        let now = os::read(other.id() as i32).unwrap();
        os::signal(&Proc { started: now.started + 1, ..now.clone() }, libc::SIGKILL);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(other.try_wait().unwrap(), None, "started at another time: another process");
        os::signal(&now, libc::SIGKILL);
        other.wait().unwrap();
    }

    #[test]
    fn only_the_environment_in_procargs_counts_and_not_a_command_naming_the_mark() {
        let mark = "LUNA_JOB=7-8-c1";
        let procargs = |args: &[&str], env: &[&str]| {
            let mut buf = (args.len() as i32).to_ne_bytes().to_vec();
            buf.extend_from_slice(b"/usr/bin/grep\0\0\0\0");
            for s in args.iter().chain(env) {
                buf.extend_from_slice(s.as_bytes());
                buf.push(0);
            }
            buf
        };
        let of = |buf: &[u8]| holds(environment_of_procargs(buf).unwrap(), mark);
        assert!(!of(&procargs(&["grep", "-F", mark], &["PATH=/bin"])), "`grep -F <mark>` is not of the job");
        assert!(of(&procargs(&["sleep", "60"], &["PATH=/bin", mark])));
        assert!(of(&procargs(&["sleep"], &[mark, "PATH=/bin"])));
        assert!(!of(&procargs(&["sleep", "60"], &[])), "a restricted binary's: nothing shown");
        assert_eq!(environment_of_procargs(b"\x01"), None);
        assert_eq!(environment_of_procargs(&2i32.to_ne_bytes()), None);
    }

    #[test]
    fn a_keeper_goes_by_the_core_itself_when_something_else_holds_the_pipe() {
        // The core's going, with its pipe held open by what leaked it: no end of
        // input comes, and only the look at the core itself says.
        let mut core = Command::new("sleep").arg("60").process_group(0).spawn().unwrap();
        let (mut turn, left) = a_turn("held-open");
        let (mut keeper, mut pipe) = own_keeper();
        say(&mut pipe, &Word::Core(leader(core.id()).unwrap())).unwrap();
        say(&mut pipe, &Word::Tie(record_of(&turn, "held-open"))).unwrap();
        std::thread::sleep(Duration::from_millis(u64::from(CORE_LOOK_MS.unsigned_abs()) + 300));
        assert!(left.iter().all(|pid| os::read(*pid).is_some()), "the core lives");
        // Left a zombie, as a core whose window never reaps it is.
        core.kill().unwrap();
        assert!(all_gone_within(&mut turn, &left, 3 * CORE_LOOK_MS as u64 + 3000));
        keeper.wait().unwrap();
        drop(pipe);
        core.wait().unwrap();

        // Its last words are heard all the same.
        let mut core = Command::new("sleep").arg("60").process_group(0).spawn().unwrap();
        let (mut turn, left) = a_turn("held-open-let-go");
        let (mut keeper, mut pipe) = own_keeper();
        say(&mut pipe, &Word::Core(leader(core.id()).unwrap())).unwrap();
        say(&mut pipe, &Word::Tie(record_of(&turn, "held-open-let-go"))).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        say(&mut pipe, &Word::LetGo("held-open-let-go".into())).unwrap();
        core.kill().unwrap();
        keeper.wait().unwrap();
        assert!(left.iter().all(|pid| os::read(*pid).is_some()), "let go of first");
        drop(pipe);
        core.wait().unwrap();
        clean_up(&mut turn, &left);
    }

    #[test]
    fn a_second_mark_counts_for_what_was_born_since_the_job_began() {
        const SETSID: &str = "perl -e 'use POSIX; POSIX::setsid(); exec @ARGV'";
        // What Codex's filter leaves: no LUNA_JOB, its own variable, a new session,
        // a new parent. Nothing but the second mark says whose it is.
        let start = |thread: &str| {
            let mut c = Command::new("sh")
                .args(["-c", &format!("{SETSID} sh -c 'sleep 60 & echo left $!'")])
                .env("CODEX_THREAD_ID", thread)
                .env_remove(MARK)
                .process_group(0)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            let pid = the(&said(&mut c, 1), "left")[0];
            c.wait().unwrap();
            pid
        };
        let older = start("thread-a");
        let since = os::read(older).unwrap().started + 1;
        std::thread::sleep(Duration::from_millis(30));
        let mine = start("thread-a");
        let someone_elses = start("thread-b");
        let job = Record {
            id: "second".into(),
            mark: "second-mark".into(),
            boot: None,
            also: Some("CODEX_THREAD_ID=thread-a".into()),
            since,
            leaders: vec![],
        };
        let by_thread = thread_marked(&job);
        // Born once the core was seen gone: a later core's turn on the same thread.
        let later = start("thread-a");
        let (gone, ended) = end_job(&job, &by_thread, ENDING_TAKES);
        assert!(gone && ended.contains(&"sleep".to_string()), "{gone} {ended:?}");
        let deadline = Instant::now() + Duration::from_secs(3);
        while os::read(mine).is_some() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(os::read(mine).is_none(), "its own conversation's");
        assert!(os::read(older).is_some(), "left by a core before this job: it let go of that");
        assert!(os::read(someone_elses).is_some(), "another conversation's");
        assert!(os::read(later).is_some(), "the same conversation's, taken up by a later core");
        for pid in [older, someone_elses, later] {
            // SAFETY: a plain signal, to a process this test started and just saw.
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
    }

    #[test]
    fn a_turn_shows_its_mark_however_soon_after_its_start_it_is_looked_at() {
        for _ in 0..300 {
            let mut turn = Command::new("sleep").arg("60").env(MARK, mark_of("so-soon")).process_group(0).spawn().unwrap();
            let pid = turn.id();
            await_environment(pid);
            let shown = leader(pid).is_some_and(|l| shows_mark(&l, "so-soon"));
            let _ = turn.kill();
            let _ = turn.wait();
            assert!(shown, "a turn just started has nothing to hide");
        }
    }

    #[test]
    fn a_turn_that_does_not_show_its_mark_is_not_counted_as_tied_but_still_goes() {
        let hidden = Command::new("sleep").arg("60").env_remove(MARK).process_group(0).spawn().unwrap();
        await_environment(hidden.id());
        let job = adopt(hidden.id()).unwrap();
        assert!(!tie(&job, "hidden"), "nothing could tell what it leaves behind");
        assert!(ties().records.contains_key(&mark_of("hidden")), "the keeper has it for all that");
        drop(job);
        assert!(!ties().records.contains_key(&mark_of("hidden")));
        // A later turn the same way: what it says goes against the helper's tie.
        let shown = Command::new("sleep").arg("60").env(MARK, mark_of("later")).process_group(0).spawn().unwrap();
        await_environment(shown.id());
        let job = adopt(shown.id()).unwrap();
        assert!(tie(&job, "later"));
        let other = Command::new("sleep").arg("60").env_remove(MARK).process_group(0).spawn().unwrap();
        await_environment(other.id());
        assert!(!join(&job, other.id()));
        // One gone already hides nothing.
        let mut quick = Command::new("true").spawn().unwrap();
        quick.wait().unwrap();
        assert!(join(&job, quick.id()));
        drop(job);
        for mut c in [hidden, shown, other] {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    #[test]
    fn a_pid_since_reused_is_left_alone_by_the_next_core() {
        let mut other = Command::new("sleep").arg("60").process_group(0).spawn().unwrap();
        let was = leader(other.id()).unwrap();
        let job = Record {
            id: "reused".into(),
            mark: "reused-no-such-mark".into(),
            boot: None,
            also: None,
            since: 0,
            leaders: vec![Leader { pid: was.pid, started: was.started + 1 }],
        };
        let dir = scratch("reused");
        write_record(&dir, &job);
        assert!(end_records(&dir, ENDING_TAKES, None).held("reused"), "nothing of that job is there");
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(other.try_wait().unwrap(), None, "the process with its old pid is another's");
        let _ = other.kill();
        let _ = other.wait();
    }

    /// A process that goes on saying `text` as `sh`, leading a group of its own.
    fn script(text: &str) -> Child {
        Command::new("sh").args(["-c", text]).process_group(0).stdin(Stdio::null()).stdout(Stdio::piped()).spawn().unwrap()
    }

    /// What a test's script says it started, `<word> <pid>` to a line, in the
    /// order they come, which is no order when two shells are saying it.
    fn said(child: &mut Child, count: usize) -> Vec<(String, i32)> {
        let mut out = BufReader::new(child.stdout.take().unwrap());
        (0..count)
            .map(|_| {
                let mut line = String::new();
                out.read_line(&mut line).unwrap();
                let (word, pid) = line.trim().split_once(' ').unwrap();
                (word.to_string(), pid.parse().unwrap())
            })
            .collect()
    }

    fn the(said: &[(String, i32)], word: &str) -> Vec<i32> {
        said.iter().filter(|(w, _)| w == word).map(|(_, pid)| *pid).collect()
    }

    #[test]
    fn a_tree_ends_whole_and_what_outlived_its_tool_is_left() {
        const SETSID: &str = "perl -e 'use POSIX; POSIX::setsid(); exec @ARGV'";
        // A job in the turn's group; a tool in a session of its own, still
        // running, with a job, a detached job and a command in hand; and what a
        // tool that has exited left behind.
        let tool = "sleep 60 & echo on $!; (nohup sleep 60 > /dev/null 2>&1 & echo on $!); sleep 60 & echo on $!; wait";
        let mut turn = script(&format!(
            "sleep 60 &\necho on $!\n{SETSID} sh -c '{tool}' &\n{SETSID} sh -c 'echo gone $$; sleep 60 & echo left $!'\nwait\n"
        ));
        let said = said(&mut turn, 6);
        let (on, left, gone) = (the(&said, "on"), the(&said, "left"), the(&said, "gone"));
        assert_eq!((on.len(), left.len(), gone.len()), (4, 1, 1), "{said:?}");
        assert!(on.iter().chain(&left).all(|pid| os::read(*pid).is_some()));
        // Said is not done: until that tool has exited, what it left still hangs from it.
        assert!(within(3000, || os::read(gone[0]).is_none()), "the tool that exits has");
        end_tree(turn.id());
        assert!(all_gone_within(&mut turn, &on, 3000), "the tool commands go too");
        assert!(os::read(left[0]).is_some(), "what outlived its tool is let go of");
        clean_up(&mut turn, &left);
    }

    #[test]
    fn a_process_told_to_stop_has_stopped_only_once_it_has() {
        let mut turn = Command::new("sleep").arg("60").process_group(0).spawn().unwrap();
        let pid = turn.id() as i32;
        assert!(within(3000, || os::read(pid).is_some()));
        assert!(!os::stopped(pid), "asleep is not stopped");
        // SAFETY: a plain signal, to a child of this test not yet reaped.
        unsafe { libc::kill(pid, libc::SIGSTOP) };
        assert!(within(3000, || os::stopped(pid)));
        let _ = turn.kill();
        let _ = turn.wait();
        assert!(os::stopped(pid), "nothing left to start anything");
    }

    #[test]
    fn what_a_tree_forks_as_it_goes_goes_too() {
        // Each round leaves a sleep with no parent, in the group of the root.
        let mark = format!("forks-as-it-goes-{}", std::process::id());
        let mut turn = Command::new("sh")
            .args(["-c", "while :; do (sleep 60 &); sleep 0.002; done"])
            .env(MARK, &mark)
            .process_group(0)
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(200));
        let holding = |proc: &Proc| os::environment(proc.pid).is_some_and(|env| holds(&env, &format!("{MARK}={mark}")));
        let any_left = || os::pids().unwrap().into_iter().filter_map(os::read).any(|p| holding(&p));
        assert!(any_left());
        end_tree(turn.id());
        let deadline = Instant::now() + Duration::from_secs(3);
        while any_left() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!any_left(), "nothing forked its way out");
        turn.wait().unwrap();
    }

    #[test]
    fn what_is_left_of_a_group_goes_once_its_root_is_gone() {
        let mut turn = script("sleep 60 & echo left $!");
        let left = the(&said(&mut turn, 1), "left");
        turn.wait().unwrap();
        assert!(os::read(left[0]).is_some());
        end_tree(turn.id());
        let deadline = Instant::now() + Duration::from_secs(3);
        while os::read(left[0]).is_some() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(os::read(left[0]).is_none());
    }

    #[test]
    fn a_keeper_under_a_tree_is_left_to_do_its_work() {
        let mut core = script(&format!("{MARK}={KEEPER} sleep 60 & echo keeper $!; sleep 60 & echo other $!; wait"));
        let said = said(&mut core, 2);
        let (keeper, other) = (the(&said, "keeper"), the(&said, "other"));
        // The shell says the pid as it forks, and the mark is there only from the exec.
        let its_mark = format!("{MARK}={KEEPER}");
        assert!(within(3000, || os::environment(keeper[0]).is_some_and(|env| holds(&env, &its_mark))), "it has its mark");
        end_tree(core.id());
        assert!(all_gone_within(&mut core, &other, 3000));
        assert!(os::read(keeper[0]).is_some(), "the core's death is what it is there for");
        clean_up(&mut core, &keeper);
    }

    #[test]
    fn a_tie_is_written_down_until_let_go_and_a_job_dropped_lets_go() {
        let mark = mark_of("written");
        let turn = || Command::new("sleep").arg("60").env(MARK, mark_of("written")).process_group(0).spawn().unwrap();
        let on_disk =
            || -> Record { serde_json::from_slice(&std::fs::read(record_file(&records_dir(), &mark_of("written"))).unwrap()).unwrap() };
        let mut first = turn();
        await_environment(first.id());
        let job = adopt(first.id()).unwrap();
        assert!(job.state().tied_for.is_none(), "untied, as a chat's stays");
        assert!(tie(&job, "written"));
        assert!(ties().records.contains_key(&mark));
        assert_eq!(on_disk().id, "written");
        assert_eq!(on_disk().leaders, [leader(first.id()).unwrap()]);
        assert_eq!(on_disk().boot, os::boot_id());

        // A turn over is forgotten as the next joins.
        let _ = first.kill();
        let _ = first.wait();
        let mut second = turn();
        await_environment(second.id());
        assert!(join(&job, second.id()));
        assert_eq!(on_disk().leaders, [leader(second.id()).unwrap()]);
        assert_eq!(on_disk().id, "written");

        // What Codex's filter cannot take out, once its thread has a name.
        also_marked(&job, "CODEX_THREAD_ID", "t1");
        assert_eq!(on_disk().also.as_deref(), Some("CODEX_THREAD_ID=t1"));
        assert_eq!(on_disk().since, leader(second.id()).map(|_| job.state().since).unwrap());

        drop(job);
        assert!(!ties().records.contains_key(&mark));
        assert!(!record_file(&records_dir(), &mark).exists(), "let go of, nothing for the next core to end");
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(second.try_wait().unwrap(), None, "let go of, nothing ends it");
        let _ = second.kill();
        let _ = second.wait();
    }
}
