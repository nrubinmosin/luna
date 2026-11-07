//! Real turns for the tests of tying and ending them: processes laid out as the
//! CLIs lay theirs out, a keeper of a test's own, and waiting on them.

use super::*;
use std::io::{BufRead, BufReader};

/// A turn as the CLIs run one: leading a group of its own, with a job in that
/// group, a tool in a session of its own with the mark cleared, and what a
/// tool that has since exited left: a new session, a new parent.
pub(super) fn a_turn(mark: &str) -> (Child, Vec<i32>) {
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

pub(super) fn record_of(child: &Child, mark: &str) -> Record {
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
pub(super) fn scratch(name: &str) -> PathBuf {
    let dir = crate::paths::data_dir().join("keeper-tests").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub(super) fn own_keeper() -> (Child, ChildStdin) {
    let mut keeper = keeper_command().unwrap().spawn().unwrap();
    let pipe = keeper.stdin.take().unwrap();
    (keeper, pipe)
}

/// Whether `done` comes true in time.
pub(super) fn within(ms: u64, done: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_millis(ms);
    while !done() {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    true
}

pub(super) fn all_gone_within(child: &mut Child, pids: &[i32], ms: u64) -> bool {
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

pub(super) fn clean_up(child: &mut Child, pids: &[i32]) {
    for pid in pids {
        // SAFETY: a plain signal, to a process this test started and has
        // just seen alive; the test ends here either way.
        unsafe { libc::kill(*pid, libc::SIGKILL) };
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// What a test's script says it started, `<word> <pid>` to a line, in the
/// order they come, which is no order when two shells are saying it.
pub(super) fn said(child: &mut Child, count: usize) -> Vec<(String, i32)> {
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

pub(super) fn the(said: &[(String, i32)], word: &str) -> Vec<i32> {
    said.iter().filter(|(w, _)| w == word).map(|(_, pid)| *pid).collect()
}
