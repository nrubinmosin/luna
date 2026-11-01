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
//! A keeper can go with its core (`killall -9`, the OOM killer), so a core also
//! writes down what it has tied as it ties it, and the next one ends what is
//! left of that before it takes the helpers up (`end_last_ties`). That alone
//! would keep a resumed turn from overlapping the old one, but it would leave a
//! dead core's turns running until a next core starts, which may be never.
//!
//! What is in a job. A process group is not enough: both CLIs start each tool
//! command in a session of its own (Claude Code spawns it `detached`, Codex
//! calls `setsid`), and what such a command leaves behind is re-parented away
//! from the turn. So a job is, at any one look, whatever has its mark in the
//! environment (`MARK`, which goes down through a new session and a new parent),
//! the turns' own processes by pid and start time with their groups, and
//! everything below any of those by parent link. Only a process that both
//! cleared its environment and lost its parents gets away.
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
use std::io::{BufRead, Write};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
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
/// Looks at a tree being frozen before it is ended whatever is still moving:
/// what is frozen cannot fork, so only a fork bomb takes more than a few.
const FREEZE_LOOKS: usize = 50;
/// A keeper that lasted less than this is not started again by itself.
const KEEPER_LASTS: Duration = Duration::from_secs(5);

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
}

#[cfg(target_os = "macos")]
mod os {
    use super::Proc;
    use std::mem::{size_of, zeroed};
    use std::os::raw::{c_char, c_int};
    use std::ptr::null_mut;

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

    pub fn read(pid: i32) -> Option<Proc> {
        let size = size_of::<libc::proc_bsdinfo>() as c_int;
        // SAFETY: a plain struct of the size passed, all zeroes valid for it.
        let info = unsafe {
            let mut info: libc::proc_bsdinfo = zeroed();
            if libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&mut info as *mut libc::proc_bsdinfo).cast(), size) != size {
                return None;
            }
            info
        };
        if info.pbi_status == ZOMBIE {
            return None;
        }
        let text = |name: &[c_char]| String::from_utf8_lossy(&name.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect::<Vec<u8>>()).into_owned();
        let name = Some(text(&info.pbi_name)).filter(|n| !n.is_empty()).unwrap_or_else(|| text(&info.pbi_comm));
        Some(Proc {
            pid,
            parent: info.pbi_ppid as i32,
            group: info.pbi_pgid as i32,
            // SAFETY: a plain query; -1, for one it will not say, leads to nothing.
            session: unsafe { libc::getsid(pid) },
            started: info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec,
            name,
        })
    }

    /// Its arguments and environment; the kernel may leave the environment out
    /// for a process signed as restricted, which then goes by its parent links.
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
            buf.truncate(size);
            Some(buf)
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
}

fn holds(environment: &[u8], mark: &str) -> bool {
    environment.split(|b| *b == 0).any(|var| var == mark.as_bytes())
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

/// A signal, if it is still the process that was looked at: a pid read a moment
/// ago may belong to another by now.
fn signal(p: &Proc, which: i32) {
    if os::read(p.pid).is_some_and(|now| now.started == p.started) {
        // SAFETY: a plain signal, to a process just seen to be this one.
        unsafe { libc::kill(p.pid, which) };
    }
}

/// Ends a process and all that hangs from it, the way `taskkill /T` does: what
/// has a window goes too if it still hangs from the tree. Each is stopped first,
/// from the top down, and looked below again, as a process killed outright would
/// leave what it forked that moment with another parent and out of the walk.
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
    let mut frozen: Vec<Proc> = Vec::new();
    for _ in 0..FREEZE_LOOKS {
        let Some(pids) = os::pids() else { break };
        let all: Vec<Proc> = pids.into_iter().filter_map(os::read).collect();
        let new: Vec<Proc> = tree(&all, root, me, child_of)
            .into_iter()
            .filter(|p| !frozen.iter().any(|f| f.pid == p.pid && f.started == p.started))
            .filter(|p| !os::environment(p.pid).is_some_and(|env| holds(&env, &keeper)))
            .cloned()
            .collect();
        if new.is_empty() {
            break;
        }
        for p in new {
            signal(&p, libc::SIGSTOP);
            frozen.push(p);
        }
    }
    for p in &frozen {
        signal(p, libc::SIGKILL);
    }
}

/// Ends all a job holds, looking until nothing is left or `within` is up.
/// Returns whether nothing is left, and the names of what it ended.
fn end_job(job: &Record, within: Duration) -> (bool, Vec<String>) {
    let me = std::process::id() as i32;
    let mark = format!("{MARK}={}", job.mark);
    let deadline = Instant::now() + within;
    let mut ended: Vec<String> = Vec::new();
    loop {
        let Some(pids) = os::pids() else { return (false, ended) };
        let all: Vec<Proc> = pids.into_iter().filter_map(os::read).collect();
        let found = members(&all, &job.leaders, me, |p| os::environment(p.pid).is_some_and(|env| holds(&env, &mark)));
        if found.is_empty() {
            return (true, ended);
        }
        for p in &found {
            // A group at one stroke: what it forks meanwhile goes too.
            if p.group == p.pid && job.leaders.iter().any(|l| l.pid == p.pid && l.started == p.started) {
                // SAFETY: a plain signal, to a group just seen led by this process.
                unsafe { libc::killpg(p.pid, libc::SIGKILL) };
            }
        }
        for p in found {
            signal(p, libc::SIGKILL);
            if !ended.contains(&p.name) {
                ended.push(p.name.clone());
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

fn keep() {
    // Only the end of its input is the core's death. A signal sent by name
    // (`killall luna`) would otherwise end this one along with the core, and
    // what it keeps would run on.
    for signal in [libc::SIGHUP, libc::SIGINT, libc::SIGTERM] {
        // SAFETY: ignoring a signal touches nothing of this program's.
        unsafe { libc::signal(signal, libc::SIG_IGN) };
    }
    let mut jobs: BTreeMap<String, Record> = BTreeMap::new();
    for line in std::io::stdin().lock().lines().map_while(Result::ok) {
        match serde_json::from_str(&line) {
            Ok(Word::Tie(job)) => {
                jobs.insert(job.mark.clone(), job);
            }
            Ok(Word::LetGo(mark)) => {
                jobs.remove(&mark);
            }
            // The core killed halfway through a line: what it had said before stands.
            Err(e) => crate::log::warn("tie", &format!("a word the keeper could not read: {e}")),
        }
    }
    for job in jobs.values() {
        let (gone, ended) = end_job(job, ENDING_TAKES);
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
    /// The write end. Rust opens pipes close-on-exec, so nothing the core
    /// starts holds it, and it closes when the core ends, or never before: a
    /// live keeper whose pipe closes takes that for the core's death.
    keeper: Option<ChildStdin>,
    generation: u64,
}

static TIES: Mutex<Ties> = Mutex::new(Ties { records: BTreeMap::new(), keeper: None, generation: 0 });

fn ties() -> MutexGuard<'static, Ties> {
    TIES.lock().unwrap_or_else(|e| e.into_inner())
}

/// In the data dir, written on every change: `helpers.json` goes by a two-second
/// beat, and a core killed within it must still leave this behind.
fn records_file() -> PathBuf {
    crate::paths::data_dir().join("tied-jobs.json")
}

fn write_records(records: &BTreeMap<String, Record>) {
    let path = records_file();
    if records.is_empty() {
        let _ = std::fs::remove_file(&path);
        return;
    }
    let tmp = path.with_extension("json.tmp");
    let written = serde_json::to_vec(&records.values().collect::<Vec<_>>())
        .map_err(std::io::Error::other)
        .and_then(|bytes| std::fs::write(&tmp, bytes))
        .and_then(|()| std::fs::rename(&tmp, &path));
    if let Err(e) = written {
        crate::log::warn("tie", &format!("could not write {}: {e}", path.display()));
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
    if !ties.records.values().all(|job| say(&mut pipe, &Word::Tie(job.clone())).is_ok()) {
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
        Word::Tie(job) => {
            ties.records.insert(job.mark.clone(), job.clone());
        }
        Word::LetGo(mark) => {
            ties.records.remove(mark);
        }
    }
    write_records(&ties.records);
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
    /// The helper the keeper has it for, until `let_go`.
    tied_for: Option<String>,
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
    Some(Job(Mutex::new(JobState { leaders: leader(pid).into_iter().collect(), tied_for: None })))
}

/// Has the keeper end the job with the core. True only once the keeper has it,
/// and where there is a way to look at what a job holds.
pub fn tie(job: &Job, id: &str) -> bool {
    if os::pids().is_none_or(|pids| pids.is_empty()) {
        crate::log::warn("tie", "no way to list processes here: helpers are not tied to the core");
        return false;
    }
    let mut state = job.state();
    let told = tell(Word::Tie(Record { id: id.to_string(), mark: mark_of(id), leaders: state.leaders.clone() }));
    if told {
        state.tied_for = Some(id.to_string());
    } else {
        // Not tied, so nothing lets go of it later: the next core must not end it.
        tell(Word::LetGo(mark_of(id)));
    }
    told
}

/// A later turn of the same helper, whose mark it has too. The turns already
/// over need no keeping: what they left has the mark, and a helper has turns
/// without number.
pub fn join(job: &Job, pid: u32) -> bool {
    let mut state = job.state();
    state.leaders.retain(|l| os::read(l.pid).is_some_and(|p| p.started == l.started));
    state.leaders.extend(leader(pid));
    match state.tied_for.clone() {
        Some(id) => tell(Word::Tie(Record { mark: mark_of(&id), id, leaders: state.leaders.clone() })),
        None => true,
    }
}

/// Undoes `tie`: what is in the job runs on past the core.
pub fn let_go(job: &Job) {
    let id = job.state().tied_for.take();
    if let Some(id) = id {
        tell(Word::LetGo(mark_of(&id)));
    }
}

/// Ends what is left of the jobs the last core tied and did not let go of, which
/// its keeper has done unless it went too. For taking the helpers up: says of a
/// helper whether its job is known to be over.
pub fn end_last_ties() -> impl Fn(&str) -> bool {
    let left: Vec<Record> = std::fs::read(records_file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let ended = end_all(&left);
    let _ = std::fs::remove_file(records_file());
    move |id: &str| ended.contains(id)
}

fn end_all(left: &[Record]) -> HashSet<String> {
    left.iter()
        .filter_map(|job| {
            let (gone, ended) = end_job(job, ENDING_TAKES);
            report(job, gone, &ended, "the last core's keeper did not see to it");
            gone.then(|| job.id.clone())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufReader, Read};

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
            p(100, 1, 100, 500), // the core
            p(200, 100, 200, 600), // the turn's CLI
            p(201, 200, 200, 610), // in its group
            p(210, 200, 210, 620), // a tool in a session of its own
            p(211, 210, 210, 630), // what that started
            p(300, 1, 300, 640), // left behind, re-parented: only the mark says
            p(301, 300, 300, 650), // and below that
            p(400, 1, 400, 660), // someone else's
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
            p(201, 200, 200, 610),                     // in its group
            in_session(210, p(210, 200, 210, 620)),    // a tool in a session of its own
            in_session(210, p(211, 210, 211, 630)),    // a job its shell gave a group of its own
            in_session(210, p(212, 1, 213, 640)),      // parent gone, in the live tool's session
            in_session(210, p(214, 1, 210, 650)),      // parent gone, in the live tool's group
            in_session(300, p(301, 1, 300, 660)),      // outlived a tool that has exited
            p(400, 1, 400, 670),                       // someone else's
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
        assert_eq!(os::parse_stat(123, stat), Some(Proc { pid: 123, parent: 45, group: 67, session: 68, started: 8910, name: "a) b (c)".into() }));
        assert_eq!(os::parse_stat(123, &stat.replace(" S ", " Z ")), None, "exited, nothing to end");
    }

    #[test]
    fn a_word_to_the_keeper_is_one_line() {
        let word = Word::Tie(Record { id: "c 1".into(), mark: "m".into(), leaders: vec![Leader { pid: 2, started: 3 }] });
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
        let script = format!(
            "sleep 60 &\necho $!\n(unset {MARK}; exec {SETSID} sleep 60) &\necho $!\n{SETSID} sh -c 'sleep 60 & echo $!'\nwait\n"
        );
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
        Record { id: format!("id-{mark}"), mark: mark.to_string(), leaders: leader(child.id()).into_iter().collect() }
    }

    fn own_keeper() -> (Child, ChildStdin) {
        let mut keeper = keeper_command().unwrap().spawn().unwrap();
        let pipe = keeper.stdin.take().unwrap();
        (keeper, pipe)
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
        let ended = end_all(&[job.clone()]);
        assert!(ended.contains(&job.id));
        assert!(all_gone_within(&mut turn, &left, 3000));
        // Nothing left of one its keeper saw to: over all the same.
        assert!(end_all(&[job.clone()]).contains(&job.id));
    }

    #[test]
    fn a_pid_since_reused_is_left_alone_by_the_next_core() {
        let mut other = Command::new("sleep").arg("60").process_group(0).spawn().unwrap();
        let was = leader(other.id()).unwrap();
        let job = Record { id: "reused".into(), mark: "reused-no-such-mark".into(), leaders: vec![Leader { pid: was.pid, started: was.started + 1 }] };
        assert!(end_all(&[job]).contains("reused"), "nothing of that job is there");
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
            "sleep 60 &\necho on $!\n{SETSID} sh -c '{tool}' &\n{SETSID} sh -c 'sleep 60 & echo left $!'\nwait\n"
        ));
        let said = said(&mut turn, 5);
        let (on, left) = (the(&said, "on"), the(&said, "left"));
        assert_eq!((on.len(), left.len()), (4, 1), "{said:?}");
        assert!(on.iter().chain(&left).all(|pid| os::read(*pid).is_some()));
        end_tree(turn.id());
        assert!(all_gone_within(&mut turn, &on, 3000), "the tool commands go too");
        assert!(os::read(left[0]).is_some(), "what outlived its tool is let go of");
        clean_up(&mut turn, &left);
    }

    #[test]
    fn what_a_tree_forks_as_it_goes_goes_too() {
        // Each round leaves a sleep with no parent, in the group of the root.
        let mark = "forks-as-it-goes";
        let mut turn = Command::new("sh")
            .args(["-c", "while :; do (sleep 60 &); sleep 0.002; done"])
            .env(MARK, mark)
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
        end_tree(core.id());
        assert!(all_gone_within(&mut core, &other, 3000));
        assert!(os::read(keeper[0]).is_some(), "the core's death is what it is there for");
        clean_up(&mut core, &keeper);
    }

    #[test]
    fn a_tie_is_written_down_until_let_go_and_a_job_dropped_lets_go() {
        let mut first = Command::new("sleep").arg("60").process_group(0).spawn().unwrap();
        let job = adopt(first.id()).unwrap();
        assert!(job.state().tied_for.is_none(), "untied, as a chat's stays");
        assert!(tie(&job, "written"));
        let mark = mark_of("written");
        assert!(ties().records.contains_key(&mark));
        let mut on_disk = String::new();
        std::fs::File::open(records_file()).unwrap().read_to_string(&mut on_disk).unwrap();
        let on_disk: Vec<Record> = serde_json::from_str(&on_disk).unwrap();
        let mine = on_disk.iter().find(|r| r.mark == mark).unwrap();
        assert_eq!(mine.id, "written");
        assert_eq!(mine.leaders, [leader(first.id()).unwrap()]);

        // A turn over is forgotten as the next joins.
        let _ = first.kill();
        let _ = first.wait();
        let mut second = Command::new("sleep").arg("60").process_group(0).spawn().unwrap();
        assert!(join(&job, second.id()));
        assert_eq!(ties().records[&mark].leaders, [leader(second.id()).unwrap()]);
        assert_eq!(ties().records[&mark].id, "written");

        drop(job);
        assert!(!ties().records.contains_key(&mark));
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(second.try_wait().unwrap(), None, "let go of, nothing ends it");
        let _ = second.kill();
        let _ = second.wait();
    }
}
