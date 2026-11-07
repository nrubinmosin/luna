//! The core's side of the tie: the keeper it starts and tells of each job, the
//! record of each it writes down as it ties it, the `Job` a helper's turn holds,
//! and what the next core ends of the records a dead one left.

use super::*;

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
