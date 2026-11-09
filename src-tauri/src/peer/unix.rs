//! The peer check's facts off Windows: the credentials the kernel recorded as
//! the client connected, and on Linux what `/proc` says of that process.

use super::{CoreFacts, PeerFacts, Seccomp, HIGH, MEDIUM};
use crate::endpoint::Stream;

fn integrity_of(uid: u32) -> u32 {
    if uid == 0 {
        HIGH
    } else {
        MEDIUM
    }
}

/// `NoNewPrivs:` in a `/proc/<pid>/status`; None when the line is not there
/// (a kernel before 4.10), which refuses rather than guess.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn no_new_privs(status: &str) -> Option<bool> {
    let value = status.lines().find_map(|l| l.strip_prefix("NoNewPrivs:"))?.trim();
    match value {
        "0" => Some(false),
        "1" => Some(true),
        _ => None,
    }
}

/// The seccomp state in a `/proc/<pid>/status`: `Seccomp:` and, from 5.9,
/// `Seccomp_filters:`. A kernel built without seccomp prints neither, and no
/// process there has any; a line that is there and does not read is no state
/// to judge by (None, refused).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn seccomp_of(status: &str) -> Option<Seccomp> {
    let field = |name: &str| status.lines().find_map(|l| l.strip_prefix(name)).map(|v| v.trim().parse::<u32>().ok());
    let mode = field("Seccomp:").unwrap_or(Some(0))?;
    let filters = match field("Seccomp_filters:") {
        Some(count) => Some(count?),
        None => None,
    };
    Some(Seccomp { mode, filters })
}

#[cfg(target_os = "linux")]
pub(super) fn core_facts(refuses_work: bool) -> Option<CoreFacts> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    // SAFETY: geteuid has no preconditions and cannot fail.
    let uid = unsafe { libc::geteuid() };
    Some(CoreFacts { integrity: integrity_of(uid), no_new_privs: no_new_privs(&status)?, seccomp: seccomp_of(&status)?, refuses_work })
}

#[cfg(not(target_os = "linux"))]
pub(super) fn core_facts(refuses_work: bool) -> Option<CoreFacts> {
    // SAFETY: geteuid has no preconditions and cannot fail.
    let uid = unsafe { libc::geteuid() };
    Some(CoreFacts { integrity: integrity_of(uid), no_new_privs: false, seccomp: Seccomp::default(), refuses_work })
}

/// `SO_PEERPIDFD` (Linux 6.5), which the libc crate does not name yet: the
/// same on x86_64 and aarch64, the targets Luna builds for.
#[cfg(target_os = "linux")]
const SO_PEERPIDFD: libc::c_int = 77;

/// What the kernel answered when asked for the peer's pidfd.
#[cfg(target_os = "linux")]
#[derive(Debug, PartialEq)]
enum Pidfd {
    /// One for the process that connected.
    Held,
    /// The kernel does not know the option (before 6.5): the check goes on
    /// with the pid alone, which a process taking the pid of one gone could
    /// pass for it (a residual on those kernels).
    Unsupported,
    /// Anything else, a peer reaped before it was asked (`EINVAL` on 6.8) or
    /// no descriptor left among them: refused, since `/proc/<pid>` read then
    /// could be another process's.
    Failed(i32),
}

/// `getsockopt(SO_PEERPIDFD)`'s outcome: `rc`, the descriptor it wrote, and
/// the errno when it failed. Only `ENOPROTOOPT`, what the kernel says of an
/// option it does not know (net/core/sock.c), is a kernel without it.
#[cfg(target_os = "linux")]
fn pidfd_kind(rc: libc::c_int, pidfd: libc::c_int, errno: i32) -> Pidfd {
    match (rc, pidfd) {
        (0, fd) if fd >= 0 => Pidfd::Held,
        (0, _) => Pidfd::Failed(libc::EBADF),
        _ if errno == libc::ENOPROTOOPT => Pidfd::Unsupported,
        _ => Pidfd::Failed(errno),
    }
}

/// A pidfd for the process at the other end of `fd`; None on a kernel before
/// 6.5, which does not know the option; an error otherwise (refused).
#[cfg(target_os = "linux")]
fn peer_pidfd(fd: std::os::fd::RawFd) -> std::io::Result<Option<std::os::fd::OwnedFd>> {
    use std::os::fd::FromRawFd;
    let mut pidfd: libc::c_int = -1;
    let mut len = size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: a socket of this process; `pidfd` and `len` match. The fd it
    // returns is this process's to close, which `OwnedFd` does.
    let rc = unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, SO_PEERPIDFD, (&mut pidfd as *mut libc::c_int).cast(), &mut len) };
    let errno = if rc == 0 { 0 } else { std::io::Error::last_os_error().raw_os_error().unwrap_or(0) };
    match pidfd_kind(rc, pidfd, errno) {
        // SAFETY: a fresh descriptor the call handed over.
        Pidfd::Held => Ok(Some(unsafe { std::os::fd::OwnedFd::from_raw_fd(pidfd) })),
        Pidfd::Unsupported => Ok(None),
        Pidfd::Failed(errno) => Err(std::io::Error::from_raw_os_error(errno)),
    }
}

/// Whether the process a pidfd stands for is still there: if it is, its pid
/// has not been given to anyone else.
#[cfg(target_os = "linux")]
fn still_there(pidfd: &std::os::fd::OwnedFd) -> bool {
    use std::os::fd::AsRawFd;
    // SAFETY: signal 0 checks for the process without sending anything.
    unsafe { libc::syscall(libc::SYS_pidfd_send_signal, pidfd.as_raw_fd(), 0, std::ptr::null::<libc::siginfo_t>(), 0) == 0 }
}

/// What the kernel and `/proc` say of the client of `stream`, or None when any
/// of it cannot be read: a pid not visible from here (another pid namespace
/// that is not below the core's), a `/proc` entry this user may not read, a
/// process gone before it was read.
#[cfg(target_os = "linux")]
pub(super) fn peer_facts(stream: &Stream) -> Option<PeerFacts> {
    let fd = stream.raw();
    let cred = crate::endpoint::peer_cred(fd).ok()?;
    let pid = u32::try_from(cred.pid).ok().filter(|&p| p != 0)?;
    let pidfd = peer_pidfd(fd).ok()?;
    let proc = |what: &str| format!("/proc/{pid}/{what}");
    let theirs = std::fs::read_link(proc("ns/user")).ok()?;
    let mine = std::fs::read_link("/proc/self/ns/user").ok()?;
    let status = std::fs::read_to_string(proc("status")).ok()?;
    let (nnp, seccomp) = (no_new_privs(&status)?, seccomp_of(&status)?);
    let image = std::fs::read_to_string(proc("comm")).map(|s| s.trim().to_string()).unwrap_or_default();
    // What was read was the connecting process's only if it is still there.
    if pidfd.as_ref().is_some_and(|p| !still_there(p)) {
        return None;
    }
    // SAFETY: geteuid has no preconditions and cannot fail.
    let me = unsafe { libc::geteuid() };
    Some(PeerFacts {
        pid,
        image,
        same_user: cred.uid == me,
        restricted: false,
        app_container: false,
        integrity: integrity_of(cred.uid),
        foreign_userns: theirs != mine,
        no_new_privs: nnp,
        seccomp,
    })
}

/// macOS: the uid, and a pid that can be read. Nothing there marks a Seatbelt
/// sandbox to the other end of a socket.
#[cfg(not(target_os = "linux"))]
pub(super) fn peer_facts(stream: &Stream) -> Option<PeerFacts> {
    let fd = stream.raw();
    let (mut uid, mut gid) = (0, 0);
    // SAFETY: a socket of this process; both out-parameters are written once.
    if unsafe { libc::getpeereid(fd, &mut uid, &mut gid) } != 0 {
        return None;
    }
    let pid = crate::endpoint::peer_pid(fd).ok()?;
    // SAFETY: geteuid has no preconditions and cannot fail.
    let me = unsafe { libc::geteuid() };
    Some(PeerFacts {
        pid,
        image: String::new(),
        same_user: uid == me,
        restricted: false,
        app_container: false,
        integrity: integrity_of(uid),
        foreign_userns: false,
        no_new_privs: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_new_privs_is_read_from_status_and_unknown_when_absent() {
        assert_eq!(no_new_privs("Name:\tcodex\nNoNewPrivs:\t1\nSeccomp:\t2\n"), Some(true));
        assert_eq!(no_new_privs("NoNewPrivs:\t0\n"), Some(false));
        assert_eq!(no_new_privs("Name:\told-kernel\n"), None);
    }

    #[test]
    fn the_seccomp_state_is_read_from_status() {
        let status = "Name:\tcodex\nNoNewPrivs:\t1\nSeccomp:\t2\nSeccomp_filters:\t3\nSpeculation_Store_Bypass:\tx\n";
        assert_eq!(seccomp_of(status), Some(Seccomp { mode: 2, filters: Some(3) }));
        // Before 5.9 the count is not there; without CONFIG_SECCOMP, neither line.
        assert_eq!(seccomp_of("Seccomp:\t0\n"), Some(Seccomp { mode: 0, filters: None }));
        assert_eq!(seccomp_of("Name:\tx\n"), Some(Seccomp { mode: 0, filters: None }));
        // A line that is there but unreadable is no state to judge by.
        assert_eq!(seccomp_of("Seccomp:\tfilter\n"), None);
        assert_eq!(seccomp_of("Seccomp:\t2\nSeccomp_filters:\t-\n"), None);
    }

    /// The client half of the tests below and of the live socket test: run as
    /// a child of the test process (`LUNA_PEER_PROBE` names the endpoint), it
    /// connects as a window does, says hello and waits for the answer, which
    /// keeps it there while the core reads it. It prints `welcomed`, `refused
    /// at the connect: <why>` or `refused: <why>`, and fails unless welcomed.
    #[test]
    #[ignore = "a child process of the peer tests"]
    fn probe_client() {
        let Ok(name) = std::env::var("LUNA_PEER_PROBE") else { return };
        let wait = std::time::Duration::from_secs(5);
        let mut s = match Stream::connect(&name, wait) {
            Ok(s) => s,
            Err(e) => {
                println!("refused at the connect: {e}");
                panic!("refused at the connect");
            }
        };
        let _ = s.set_read_timeout(Some(wait));
        let hello = crate::ipc::Hello { proto: crate::ipc::PROTO, role: "ctl".into() };
        let req = crate::ipc::Req { id: 1, cmd: "hello".into(), args: serde_json::to_value(hello).unwrap() };
        match crate::ipc::send(&mut s, crate::ipc::REQ, &req).and_then(|()| crate::ipc::recv::<crate::ipc::Res>(&mut s, crate::ipc::RES)) {
            Ok(res) if res.ok.is_some() => println!("welcomed"),
            Ok(res) => panic!("refused: {:?}", res.err),
            Err(e) => {
                println!("refused: {e}");
                panic!("refused");
            }
        }
    }

    /// What the peer check makes of a child of this process that `confine`
    /// has confined, as it starts, before it connects; None when the confining
    /// is refused here (a container's seccomp profile refuses a user
    /// namespace).
    #[cfg(target_os = "linux")]
    fn verdict_on_child(confine: fn() -> std::io::Result<()>) -> Option<(Option<PeerFacts>, Option<String>)> {
        use std::os::unix::process::CommandExt;
        let name = crate::endpoint::tests::test_name();
        let listener = crate::endpoint::Listener::bind(&name).unwrap();
        let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
        cmd.args(["--exact", "peer::imp::tests::probe_client", "--ignored", "--quiet"]).env("LUNA_PEER_PROBE", &name);
        cmd.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        // SAFETY: `confine` makes one syscall, async-signal-safe.
        unsafe { cmd.pre_exec(confine) };
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("skipped: could not confine the child here: {e}");
                return None;
            }
        };
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(listener.accept().map(|s| (peer_facts(&s), super::super::refuse_reason(&s))));
        });
        let got = rx.recv_timeout(std::time::Duration::from_secs(30)).expect("the child connected").expect("accept");
        let _ = child.kill();
        let _ = child.wait();
        Some(got)
    }

    /// A child with no_new_privs set, as Codex's Linux sandbox and Landlock
    /// both set it: refused, where the same child without it is let in.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_client_with_no_new_privs_is_refused() {
        let core = core_facts(false).unwrap();
        if core.no_new_privs {
            eprintln!("skipped: this process runs with no_new_privs itself");
            return;
        }
        let (facts, refused) = verdict_on_child(|| Ok(())).expect("a plain child");
        assert_eq!(refused, None, "a plain child is let in: {facts:?}");
        let (facts, refused) = verdict_on_child(|| {
            // SAFETY: prctl with the documented arguments for this option.
            match unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } {
                0 => Ok(()),
                _ => Err(std::io::Error::last_os_error()),
            }
        })
        .expect("no_new_privs can always be set");
        assert!(facts.as_ref().is_some_and(|f| f.no_new_privs), "{facts:?}");
        assert!(refused.is_some_and(|r| r.contains("no_new_privs")), "{facts:?}");
    }

    /// Only a kernel that does not know `SO_PEERPIDFD` (before 6.5) lets the
    /// check go on without a pidfd. Any other failure, such as a peer reaped
    /// before it was asked (`EINVAL` on 6.8) or no descriptor left, refuses:
    /// read without the pidfd, `/proc/<pid>` could be another process's.
    #[cfg(target_os = "linux")]
    #[test]
    fn only_a_kernel_without_the_option_goes_on_without_a_pidfd() {
        assert_eq!(pidfd_kind(0, 7, 0), Pidfd::Held);
        assert_eq!(pidfd_kind(-1, -1, libc::ENOPROTOOPT), Pidfd::Unsupported);
        for errno in [libc::EINVAL, libc::EMFILE, libc::ENFILE, libc::ESRCH, libc::ENOMEM, libc::EFAULT] {
            assert_eq!(pidfd_kind(-1, -1, errno), Pidfd::Failed(errno), "errno {errno}");
        }
        assert!(matches!(pidfd_kind(0, -1, 0), Pidfd::Failed(_)), "a success that hands over no descriptor");
    }

    /// What a kernel before 6.5 answers for `SO_PEERPIDFD`, seen on this one
    /// with an option no kernel knows: `ENOPROTOOPT` (net/core/sock.c).
    #[cfg(target_os = "linux")]
    #[test]
    fn an_option_the_kernel_does_not_know_reads_as_enoprotoopt() {
        let (_l, client, _server) = crate::endpoint::tests::pair();
        let mut value: libc::c_int = 0;
        let mut len = size_of::<libc::c_int>() as libc::socklen_t;
        // SAFETY: a socket of this process; `value` and `len` match.
        let rc = unsafe { libc::getsockopt(client.raw(), libc::SOL_SOCKET, 0x7fff, (&mut value as *mut libc::c_int).cast(), &mut len) };
        assert_eq!((rc, std::io::Error::last_os_error().raw_os_error()), (-1, Some(libc::ENOPROTOOPT)));
    }

    /// A peer gone and reaped before the core reads it is refused, and on a
    /// kernel that knows `SO_PEERPIDFD` never read without a pidfd.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_peer_reaped_before_it_is_read_is_refused() {
        let name = crate::endpoint::tests::test_name();
        let listener = crate::endpoint::Listener::bind(&name).unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "peer::imp::tests::probe_client", "--ignored", "--quiet"])
            .env("LUNA_PEER_PROBE", &name)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let server = listener.accept().unwrap();
        let _ = child.kill();
        let _ = child.wait();
        match peer_pidfd(server.raw()) {
            Err(_) => {}
            Ok(Some(pidfd)) => assert!(!still_there(&pidfd), "a reaped peer reads as there"),
            Ok(None) => panic!("a kernel that knows SO_PEERPIDFD went on without one"),
        }
        assert_eq!(peer_facts(&server), None);
    }

    /// Confines the child as a seccomp sandbox does: no_new_privs, then one
    /// filter (allowing every call; its being there is what counts).
    #[cfg(target_os = "linux")]
    fn with_a_seccomp_filter() -> std::io::Result<()> {
        // BPF_RET | BPF_K, SECCOMP_RET_ALLOW.
        let mut allow = [libc::sock_filter { code: 0x06, jt: 0, jf: 0, k: 0x7fff_0000 }];
        let prog = libc::sock_fprog { len: 1, filter: allow.as_mut_ptr() };
        // SAFETY: prctl with the documented arguments for each option; the
        // program outlives the call, which copies it.
        unsafe {
            if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0
                || libc::prctl(libc::PR_SET_SECCOMP, libc::SECCOMP_MODE_FILTER, &prog as *const libc::sock_fprog) != 0
            {
                return Err(std::io::Error::last_os_error());
            }
        }
        Ok(())
    }

    /// A child that added a seccomp filter to what it was started with, as
    /// Codex's sandbox does, reads as stricter than the core: refused, and
    /// refused for that by a core that has no_new_privs itself, where the bit
    /// tells it nothing.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_client_under_a_stricter_seccomp_than_the_cores_is_refused() {
        let core = core_facts(false).unwrap();
        let (facts, refused) = verdict_on_child(with_a_seccomp_filter).expect("a filter can always be added under no_new_privs");
        let facts = facts.expect("facts");
        assert!(facts.seccomp.stricter_than(&core.seccomp), "{facts:?} against {core:?}");
        assert!(refused.is_some(), "{facts:?}");
        let under_nnp = CoreFacts { no_new_privs: true, ..core };
        assert_eq!(
            super::super::verdict(Some(&facts), &under_nnp),
            super::super::Verdict::Refuse("a stricter seccomp filter than the core's (a CLI sandbox)")
        );
    }

    /// A child in a user namespace of its own, as bubblewrap runs Codex's
    /// commands: refused, though its uid reads as the core's own.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_client_in_another_user_namespace_is_refused() {
        let Some((facts, refused)) = verdict_on_child(|| {
            // SAFETY: unshare with a documented flag; it fails cleanly where
            // a seccomp profile or sysctl refuses it.
            match unsafe { libc::unshare(libc::CLONE_NEWUSER) } {
                0 => Ok(()),
                _ => Err(std::io::Error::last_os_error()),
            }
        }) else {
            return;
        };
        let facts = facts.expect("facts");
        assert!(facts.same_user && facts.foreign_userns, "{facts:?}");
        assert!(refused.is_some_and(|r| r.contains("user namespace")), "{facts:?}");
    }
}
