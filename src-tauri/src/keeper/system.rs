//! What the OS says of a process: the list of them, one's parents, group,
//! session, start time, state and environment, and signals. Linux reads /proc,
//! macOS asks libproc and sysctl; anywhere else there is nothing to look with.

use super::*;

#[cfg(target_os = "linux")]
pub(super) mod os {
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
pub(super) mod os {
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
pub(super) mod os {
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

pub(super) fn holds(environment: &[u8], mark: &str) -> bool {
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
pub(super) fn is_still(p: &Proc) -> bool {
    os::read(p.pid).is_some_and(|now| now.started == p.started)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn a_boot_has_an_id_where_the_system_gives_one() {
        #[cfg(target_os = "linux")]
        assert!(os::boot_id().is_some_and(|id| id.len() > 8));
        assert_eq!(os::boot_id(), os::boot_id());
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
}
