//! The peer check's facts off Windows: the credentials the kernel recorded as
//! the client connected, and on Linux what `/proc` says of that process.

use super::{CoreFacts, PeerFacts, HIGH, MEDIUM};
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

#[cfg(target_os = "linux")]
pub(super) fn core_facts(refuses_work: bool) -> Option<CoreFacts> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    // SAFETY: geteuid has no preconditions and cannot fail.
    let uid = unsafe { libc::geteuid() };
    Some(CoreFacts { integrity: integrity_of(uid), no_new_privs: no_new_privs(&status)?, refuses_work })
}

#[cfg(not(target_os = "linux"))]
pub(super) fn core_facts(refuses_work: bool) -> Option<CoreFacts> {
    // SAFETY: geteuid has no preconditions and cannot fail.
    let uid = unsafe { libc::geteuid() };
    Some(CoreFacts { integrity: integrity_of(uid), no_new_privs: false, refuses_work })
}

/// `SO_PEERPIDFD` (Linux 6.5), which the libc crate does not name yet: the
/// same on x86_64 and aarch64, the targets Luna builds for.
#[cfg(target_os = "linux")]
const SO_PEERPIDFD: libc::c_int = 77;

/// A pidfd for the process at the other end of `fd`, where the kernel gives
/// one: None on a kernel before 6.5.
#[cfg(target_os = "linux")]
fn peer_pidfd(fd: std::os::fd::RawFd) -> Option<std::os::fd::OwnedFd> {
    use std::os::fd::FromRawFd;
    let mut pidfd: libc::c_int = -1;
    let mut len = size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: a socket of this process; `pidfd` and `len` match. The fd it
    // returns is this process's to close, which `OwnedFd` does.
    let rc = unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, SO_PEERPIDFD, (&mut pidfd as *mut libc::c_int).cast(), &mut len) };
    // SAFETY: a fresh descriptor the call handed over.
    (rc == 0 && pidfd >= 0).then(|| unsafe { std::os::fd::OwnedFd::from_raw_fd(pidfd) })
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
    let pidfd = peer_pidfd(fd);
    let proc = |what: &str| format!("/proc/{pid}/{what}");
    let theirs = std::fs::read_link(proc("ns/user")).ok()?;
    let mine = std::fs::read_link("/proc/self/ns/user").ok()?;
    let nnp = no_new_privs(&std::fs::read_to_string(proc("status")).ok()?)?;
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

    /// The client half of the tests below: run as a child of the test
    /// process (`LUNA_PEER_PROBE` names the endpoint), it connects and stays
    /// a moment so the core can read it.
    #[test]
    #[ignore = "a child process of the peer tests"]
    fn probe_client() {
        let Ok(name) = std::env::var("LUNA_PEER_PROBE") else { return };
        let s = Stream::connect(&name, std::time::Duration::from_secs(5)).expect("the probe connects");
        std::io::Write::write_all(&mut &s, b"x").unwrap();
        std::thread::sleep(std::time::Duration::from_secs(5));
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
