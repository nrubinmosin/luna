//! What a job is at one look — its turns' processes with their groups, what has
//! its mark, and all below them — and ending one: a tied job when its core dies,
//! and a tree (`end_tree`) for a stop that is chosen.

use super::*;

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
pub(super) fn thread_marked(job: &Record) -> HashSet<(i32, u64)> {
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
pub(super) fn end_job(job: &Record, by_thread: &HashSet<(i32, u64)>, within: Duration) -> (bool, Vec<String>) {
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

pub(super) fn report(job: &Record, gone: bool, ended: &[String], why: &str) {
    if !gone {
        crate::log::error("tie", &format!("{}: {why}, and not all of its job could be ended (of {})", job.id, ended.join(", ")));
    } else if !ended.is_empty() {
        crate::log::warn("tie", &format!("{}: {why}: ended {}", job.id, ended.join(", ")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// A process that goes on saying `text` as `sh`, leading a group of its own.
    fn script(text: &str) -> Child {
        Command::new("sh").args(["-c", text]).process_group(0).stdin(Stdio::null()).stdout(Stdio::piped()).spawn().unwrap()
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
}
