//! The keeper itself: this exe started with `--keep-ties`, reading the words its
//! core sends down the pipe and ending every job it holds once that input ends
//! or the core is seen gone.

use super::*;

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub(super) enum Word {
    /// Who the core is, for the keeper to look at. The first word it hears.
    Core(Leader),
    Tie(Record),
    LetGo(String),
}

pub(super) fn say(to: &mut impl Write, word: &Word) -> std::io::Result<()> {
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

pub(super) fn keeper_command() -> std::io::Result<Command> {
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
        cmd.args(["--exact", "keeper::process::tests::keeper_entry", "--ignored", "--nocapture"]).env("LUNA_KEEPER_TEST", "1");
        cmd
    };
    // A group of its own, so that a signal to the core's does not reach it; not
    // in any project folder, so that it never holds one open.
    cmd.current_dir("/").env(MARK, KEEPER).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).process_group(0);
    Ok(cmd)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tests' keeper (`keeper_command`), not a test of anything.
    #[test]
    #[ignore]
    fn keeper_entry() {
        if std::env::var_os("LUNA_KEEPER_TEST").is_some() {
            keep();
        }
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
}
