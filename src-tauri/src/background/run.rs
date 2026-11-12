//! A helper's turn as a process: starting it, feeding its stdin, and reading its
//! stdout and stderr until it ends.

use super::*;

/// Starts a helper on its opening prompt.
pub fn start(id: &str, spec: Spec, prompt: &str) -> Result<(), String> {
    let mut e = Entry {
        spec,
        started_ms: crate::throttle::now_ms(),
        session: None,
        cwd: None,
        run: None,
        turns_ended: 0,
        queue: Vec::new(),
        in_flight: Vec::new(),
        fed: Vec::new(),
        log: VecDeque::new(),
        held: VecDeque::new(),
        error: None,
        cut_short: false,
        undelivered: Vec::new(),
        budgets_landed: Vec::new(),
        tied: false,
        resumable: false,
        unsure: false,
        opening: None,
        opening_id: None,
        job: None,
        left_running: false,
        left: Vec::new(),
        ending_left: false,
        promoting: false,
        promoting_since: 0,
        deleting: false,
        handing: false,
        observed: Observed::default(),
    };
    claim(&mut e);
    lock().entries.insert(id.to_string(), e);
    if let Err(e) = run_turn(id, prompt) {
        lock().entries.remove(id);
        changed();
        return Err(e);
    }
    changed();
    Ok(())
}

/// Starts the next turn's process with `prompt` on its stdin, on a run
/// `claim` has taken; a turn that could not start gives the claim back,
/// counted as a turn that ended on the error: whoever queued it, or queued
/// behind it, counts on its end, and `turn_over` never comes for it. Text
/// queued behind it fails with it, counted as the turn after, so whoever
/// waits on it hears — rather than starting what would most likely fail the
/// same way.
pub(super) fn run_turn(id: &str, prompt: &str) -> Result<(), String> {
    let started = spawn_turn(id, prompt);
    if let Err(err) = &started {
        if let Some(e) = lock().entries.get_mut(id) {
            if e.run.as_ref().is_some_and(|r| r.pid.is_none()) {
                e.run = None;
                note(e, &format!("could not start the turn: {err}"));
                e.error = Some(err.clone());
                e.turns_ended += 1;
                if !e.queue.is_empty() {
                    e.queue.clear();
                    note(e, "dropped what was queued behind it");
                    e.turns_ended += 1;
                }
            }
        }
        reg().1.notify_all();
        changed();
    }
    started
}

/// Claude Code's stdin for a turn: `first`, then whatever comes down `lines`,
/// until they close, which is the end of its input. A thread of its own, as
/// nothing says the CLI is reading: a full pipe holds this, and no lock.
fn feed_stdin(mut stdin: impl Write, first: &str, lines: std::sync::mpsc::Receiver<String>) {
    let mut put = |line: &str| stdin.write_all(line.as_bytes()).and_then(|()| stdin.flush());
    if put(first).is_err() {
        return;
    }
    for line in lines {
        if put(&line).is_err() {
            return;
        }
    }
}

/// What a turn starts on, for as long as no turn has named its conversation.
fn note_opening(e: &mut Entry, prompt: &str, line: &str) {
    e.opening = e.session.is_none().then(|| prompt.to_string());
    e.opening_id = e.session.is_none().then(|| line.to_string());
}

/// Why a turn claimed before a stop does not run.
const STOPPED: &str = "it was stopped before it started";
/// Once the CLI's process has gone, a reader blocked on its pipe this long has
/// read all it wrote: the pipe is empty, and what holds it open is something it
/// left running, which may do so for days.
const DRAINED_AFTER: std::time::Duration = std::time::Duration::from_millis(500);
/// The most a turn's end waits on a reader that is not there yet (a stalled
/// disk behind the registry lock, a job writing to the pipe without end).
const DRAIN_AT_MOST: std::time::Duration = std::time::Duration::from_secs(30);

/// How the reader of a turn's stdout stands, for the thread that sees the turn
/// out: the turn is over when the CLI is, not when the pipe closes, as a job it
/// left running can have inherited the pipe. What the CLI wrote before it went
/// is still to be read by then, and a blocked read is the only sign that all
/// of it has been.
#[derive(Default)]
pub(super) struct OutReader {
    /// In a read that has not returned.
    reading: std::sync::atomic::AtomicBool,
    reads: std::sync::atomic::AtomicU64,
    /// The turn is over: what comes down the pipe from here is not the CLI's.
    over: std::sync::atomic::AtomicBool,
}

impl OutReader {
    /// Reads `out` to its end as bytes, handing each line that holds JSON to
    /// `on_event`, which says whether to go on.
    pub(super) fn read_events(&self, out: impl std::io::Read, mut on_event: impl FnMut(&Value) -> bool) {
        self.read_lines(out, |line| serde_json::from_slice::<Value>(line).map_or(true, |v| on_event(&v)));
    }

    /// Reads `out` to its end as bytes, handing each line to `on_line`, which
    /// says whether to go on; the last one too, should it come with no line end.
    fn read_lines(&self, mut out: impl std::io::Read, mut on_line: impl FnMut(&[u8]) -> bool) {
        use std::sync::atomic::Ordering::SeqCst;
        let mut buf = vec![0u8; 64 * 1024];
        let mut line = Vec::new();
        loop {
            self.reading.store(true, SeqCst);
            let n = out.read(&mut buf).unwrap_or(0);
            self.reading.store(false, SeqCst);
            self.reads.fetch_add(1, SeqCst);
            if n == 0 {
                break;
            }
            for chunk in buf[..n].split_inclusive(|b| *b == b'\n') {
                line.extend_from_slice(chunk);
                if chunk.last() == Some(&b'\n') {
                    if !on_line(&line) {
                        return;
                    }
                    line.clear();
                }
            }
        }
        if !line.is_empty() {
            on_line(&line);
        }
    }

    /// Waits for what the CLI wrote before it went to have been read: for the
    /// reader to end (`read_out` closes with it), or to sit in a read with
    /// nothing coming, however long it took to get through the rest.
    pub(super) fn await_last_words(
        &self,
        read_out: &std::sync::mpsc::Receiver<()>,
        drained_after: std::time::Duration,
        at_most: std::time::Duration,
    ) {
        use std::sync::atomic::Ordering::SeqCst;
        let start = std::time::Instant::now();
        let (mut reads, mut since) = (self.reads.load(SeqCst), start);
        while start.elapsed() < at_most {
            if !matches!(read_out.recv_timeout(std::time::Duration::from_millis(20)), Err(std::sync::mpsc::RecvTimeoutError::Timeout)) {
                return;
            }
            let now = self.reads.load(SeqCst);
            if now != reads {
                (reads, since) = (now, std::time::Instant::now());
            }
            if self.reading.load(SeqCst) && since.elapsed() >= drained_after {
                return;
            }
        }
    }
}

/// One event off a turn's stdout, into its helper. The flag is looked at under
/// the lock: `turn_over` takes it after the flag is up and the next turn starts
/// after that, so an event of this turn can land neither after its end nor in
/// the next one. Whether to go on reading: not once the helper is gone.
fn take_event(id: &str, provider: Provider, reader: &OutReader, v: &Value) -> bool {
    let mut r = lock();
    if reader.over.load(std::sync::atomic::Ordering::SeqCst) {
        return true;
    }
    let Some(e) = r.entries.get_mut(id) else { return false };
    e.observed.event_ms = crate::throttle::now_ms();
    let unnamed = e.session.is_none();
    let landed = match provider {
        Provider::Claude => on_claude_event(e, v),
        Provider::Codex => {
            on_codex_event(e, v);
            false
        }
    };
    let named = unnamed && e.session.is_some();
    drop(r);
    // At once: a core killed now would take it up as one that never got started,
    // and start its work over; or hand back as undelivered what the model has.
    if named || landed {
        save();
    }
    true
}

/// The end of a turn's stderr kept, as lines, at most `STDERR_KEPT` bytes of
/// it: what the canary reads of a turn that ended early (cli/canary.rs).
fn keep_stderr(kept: &Mutex<String>, line: &[u8]) {
    let mut s = kept.lock().unwrap_or_else(|e| e.into_inner());
    s.push_str(&String::from_utf8_lossy(line));
    s.push('\n');
    if s.len() > STDERR_KEPT {
        let cut = (s.len() - STDERR_KEPT..s.len()).find(|&i| s.is_char_boundary(i)).unwrap_or(s.len());
        s.drain(..cut);
    }
}

const STDERR_KEPT: usize = 4096;

/// A line off a turn's stderr, into its helper's log, under the same flag as
/// `take_event`: what a process the turn left writes there after the turn is
/// over is neither the helper's word nor a sign of it at work. Read on to the
/// end all the same, so that nothing writing there blocks.
fn take_stderr(id: &str, reader: &OutReader, line: &[u8]) -> bool {
    let line = String::from_utf8_lossy(line);
    let line = line.trim();
    if line.is_empty() || line.starts_with("Reading additional input") {
        return true;
    }
    let mut r = lock();
    if reader.over.load(std::sync::atomic::Ordering::SeqCst) {
        return true;
    }
    if let Some(e) = r.entries.get_mut(id) {
        e.observed.event_ms = crate::throttle::now_ms();
        e.observed.background_stopped |= stops_background(line);
        note(e, &format!("stderr: {}", one_line(line)));
    }
    true
}

/// Whether a line of `claude -p`'s stderr is its word that it stops what the
/// last turn left in the background, its stdin closed and its wait for that
/// over (2.1.319+: "Background tasks still running 10m after the last turn
/// (…); stopping them. Set CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS=0 to wait
/// indefinitely."). A helper's turn sets that to `0` (command.rs); one that
/// still says this has had it overridden or ignored.
fn stops_background(line: &str) -> bool {
    line.starts_with("Background tasks still running") && line.contains("; stopping them.")
}

fn spawn_turn(id: &str, prompt: &str) -> Result<(), String> {
    // Nothing runs as administrator.
    crate::elevation::guard()?;
    settle_worktree(id);
    // Before the registry is locked for the command line: a pass reads and
    // writes every account's copies, and every helper's events wait on that
    // lock (library/folders.rs `session_args`).
    // The helper's account counted as in use from here, as a chat's start
    // counts its own (`library::lay_out_for`).
    let account_path = lock().entries.get(id).map(|e| e.spec.account_path.clone()).ok_or("no such helper")?;
    let reserved = crate::library::lay_out_for("before a helper's turn", &account_path)?;
    let (args, dir, envs, applied, provider, on_stdin, line, account) = {
        let r = lock();
        if r.closed {
            return Err("Luna is stopping".into());
        }
        let e = r.entries.get(id).ok_or("no such helper")?;
        if e.run.as_ref().is_some_and(|run| run.stopped) {
            return Err(STOPPED.into());
        }
        let (args, dir, envs, applied) = command_for(e, id)?;
        // A batch's own, on disk by now; what no batch carries needs only to be a uuid.
        let line = e.in_flight.first().map_or_else(new_id, |q| q.id.clone());
        (args, dir, envs, applied, e.spec.provider, stdin_text(e, prompt), line, e.spec.account_path.clone())
    };
    if !std::path::Path::new(&dir).is_dir() {
        return Err(format!("{dir} does not exist"));
    }
    // Its version directory held until the process has exited (cli.rs `launch`).
    let (exe, on_version) = crate::cli::launch(provider, Some(id))?;
    let mut cmd = std::process::Command::new(exe);
    cmd.args(&args).current_dir(&dir);
    crate::pty::clean_env(&mut cmd);
    for (k, v) in &envs {
        cmd.env(k, v);
    }
    cmd.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    // Suspended until it is in the helper's job (`procs::resume`): what it
    // starts at once would otherwise be born outside it.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000 | 0x0000_0004); // CREATE_NO_WINDOW | CREATE_SUSPENDED
    }
    // A group of its own, as a pty gives its CLI: what kills a turn
    // (`pty::kill_tree`) takes that group with the rest of the tree. The mark
    // goes with what leaves it, for a core's death (keeper/).
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
        cmd.env(crate::keeper::MARK, crate::keeper::mark_of(id));
    }
    let thread_err = |e: std::io::Error| format!("could not start {}: {e}", provider.as_str());
    // The thread that sees the turn out is there before the process: one
    // that could not be made after it would leave the process running with
    // nobody to reap it or end its turn. It is handed the process once the
    // turn is recorded, and nothing if the turn does not start.
    type Pipes = (
        std::process::Child,
        Option<std::process::ChildStdout>,
        Option<std::process::ChildStderr>,
        (crate::account_use::Running, Option<crate::cli::InUse>),
    );
    let (hand, take) = std::sync::mpsc::channel::<Pipes>();
    {
        let id = id.to_string();
        std::thread::Builder::new()
            .name(format!("luna-bg-{id}"))
            .spawn(move || {
                let Ok((mut child, stdout, stderr, _on_account)) = take.recv() else { return };
                // Each pipe on a thread of its own (see `OutReader`). Both are
                // read to their end as bytes: `lines()` stops at the first line
                // that is not UTF-8 — a hook's output in the console's code page
                // — and a pipe nobody drains any more blocks the CLI once it fills.
                let reader = std::sync::Arc::new(OutReader::default());
                let (drained, read_out) = std::sync::mpsc::channel::<()>();
                if let Some(out) = stdout {
                    let (helper, on_it) = (id.clone(), reader.clone());
                    let read = std::thread::Builder::new().name(format!("luna-bg-out-{id}")).spawn(move || {
                        let _drained = drained;
                        on_it.read_events(out, |v| take_event(&helper, provider, &on_it, v));
                    });
                    if let Err(e) = read {
                        crate::log::warn("background", &format!("cannot read what {} says: {e}", crate::agents::label(&id)));
                    }
                }
                let err_reader = std::sync::Arc::new(OutReader::default());
                let (err_drained, err_out) = std::sync::mpsc::channel::<()>();
                // The end of what it says on stderr, for the canary.
                let said = std::sync::Arc::new(Mutex::new(String::new()));
                if let Some(err) = stderr {
                    let (helper, on_it, kept) = (id.clone(), err_reader.clone(), said.clone());
                    let read = std::thread::Builder::new().name(format!("luna-bg-err-{id}")).spawn(move || {
                        let _drained = err_drained;
                        on_it.read_lines(err, |line| {
                            keep_stderr(&kept, line);
                            take_stderr(&helper, &on_it, line)
                        });
                    });
                    if let Err(e) = read {
                        crate::log::warn("background", &format!("cannot read what {} says on stderr: {e}", crate::agents::label(&id)));
                    }
                }
                let code = child.wait().ok().and_then(|s| s.code());
                crate::cli::session_exited(&id);
                reader.await_last_words(&read_out, DRAINED_AFTER, DRAIN_AT_MOST);
                err_reader.await_last_words(&err_out, DRAINED_AFTER, DRAIN_AT_MOST);
                reader.over.store(true, std::sync::atomic::Ordering::SeqCst);
                err_reader.over.store(true, std::sync::atomic::Ordering::SeqCst);
                // For the canary over a new CLI version's first sessions.
                crate::cli::session_ended(&id, code.is_some_and(|c| c != 0), || {
                    std::mem::take(&mut *said.lock().unwrap_or_else(|e| e.into_inner()))
                });
                turn_over(&id, code);
            })
            .map_err(thread_err)?;
    }
    // Held by the thread above until it has reaped the process; dropped here
    // on the ways out where the turn does not start.
    let on_account = crate::account_use::running_on(&account);
    drop(reserved);
    // Ahead of the process: what it starts is born after this (`turn_over`).
    let started_ms = crate::throttle::now_ms();
    let mut child = cmd.spawn().map_err(|e| {
        crate::log::error("background", &format!("could not start {}: {e}", crate::agents::label(id)));
        format!("could not start {}: {e}", provider.as_str())
    })?;
    // This turn's, for the window's "What Luna applied"; not under this
    // module's lock, which nothing may hold while it takes the registry's.
    crate::agents::set_applied(id, applied, &account, &dir);
    let pid = Some(child.id());
    // Into the helper's job before it has had time to start anything. What is
    // waited for here is not waited for with every helper's lock held.
    if let Some(pid) = pid {
        crate::procs::await_environment(pid);
        if let Some(e) = lock().entries.get_mut(id) {
            match &e.job {
                Some(job) => {
                    let joined = crate::procs::join(job, pid);
                    e.tied = e.tied && joined;
                }
                None => {
                    e.job = crate::procs::adopt(pid).map(std::sync::Arc::new);
                    e.tied = e.job.as_deref().is_some_and(|job| crate::procs::tie(job, id));
                }
            }
            mark_by_thread(e);
        }
        #[cfg(windows)]
        if !crate::procs::resume(pid) {
            crate::log::error("background", &format!("could not let {} run once in its job", crate::agents::label(id)));
            abandon(child, Some(pid));
            return Err(format!("could not start {}: its process would not resume", provider.as_str()));
        }
    }
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let (feed, lines) = std::sync::mpsc::channel::<String>();
    if let Some(mut stdin) = stdin {
        let prompt = on_stdin;
        let writer_id = id.to_string();
        let writer = std::thread::Builder::new();
        let spawned = match provider {
            // Open until its first `result` (`on_claude_event`), for what joins the turn.
            Provider::Claude => {
                let first = user_line(&line, &prompt);
                writer.spawn(move || feed_stdin(stdin, &first, lines))
            }
            // Written whole and closed: the CLI reads its prompt to the end of input.
            Provider::Codex => writer.spawn(move || {
                let wrote = stdin.write_all(prompt.as_bytes()).is_ok();
                drop(stdin);
                let landed = wrote && lock().entries.get_mut(&writer_id).is_some_and(stdin_took);
                if landed {
                    save();
                }
            }),
        };
        if let Err(e) = spawned {
            abandon(child, pid);
            return Err(thread_err(e));
        }
    }
    {
        let mut r = lock();
        let closed = r.closed;
        // A stop that came while the process was starting found no pid to
        // kill: the process goes here instead.
        let refused = match r.entries.get_mut(id) {
            _ if closed => Some("Luna is stopping"),
            None => Some("the helper was deleted"),
            Some(e) if e.run.as_ref().is_some_and(|run| run.stopped) => Some(STOPPED),
            Some(e) => {
                let claude = provider == Provider::Claude;
                let opening_line = line.clone();
                e.run = Some(Run { feed: claude.then_some(feed), line: claude.then_some(line), ..Run::claimed(pid, started_ms) });
                e.observed.begin_turn();
                e.error = None;
                e.cut_short = false;
                e.unsure = false;
                note_opening(e, prompt, &opening_line);
                note(e, &format!("› {}", one_line(prompt)));
                None
            }
        };
        if let Some(why) = refused {
            drop(r);
            abandon(child, pid);
            return Err(why.into());
        }
    }
    crate::log::info(
        "background",
        &format!("{} turn started, pid {pid:?}: {} {}", crate::agents::label(id), provider.as_str(), args_for_log(id, &args)),
    );
    // The reader waits on the other end; should it be gone, the turn still
    // ends here rather than with nobody to say so.
    if let Err(std::sync::mpsc::SendError((child, _, _, _))) = hand.send((child, stdout, stderr, (on_account, on_version))) {
        abandon(child, pid);
        turn_over(id, None);
    }
    Ok(())
}

/// A turn's arguments as the log keeps them. The chat's hook and guard URLs
/// carry its secret (a Codex start passes them in `-c` overrides) and go in as
/// what they are; the helper's identity line, the same for every turn and
/// most of the line's length, goes in as its length.
fn args_for_log(id: &str, args: &[String]) -> String {
    let urls: Vec<(String, &str)> = [(crate::hub::hook_url(id), "<hook-url>"), (crate::hub::guard_url(id), "<guard-url>")]
        .into_iter()
        .filter_map(|(url, to)| Some((url?, to)))
        .collect();
    let mut shown = Vec::with_capacity(args.len());
    let mut elide = false;
    for a in args {
        if std::mem::take(&mut elide) {
            shown.push(format!("<{} chars>", a.chars().count()));
            continue;
        }
        elide = a == "--append-system-prompt";
        shown.push(urls.iter().fold(a.clone(), |s, (url, to)| s.replace(url.as_str(), to)));
    }
    shown.join(" ")
}

/// The turn has what `in_flight` holds: not to be handed back as undelivered, and
/// the budget lines in it have arrived. Whether there was any.
pub(super) fn stdin_took(e: &mut Entry) -> bool {
    let batch = std::mem::take(&mut e.in_flight);
    e.budgets_landed.extend(batch.iter().flat_map(|q| q.budget.iter().map(|m| m.id.clone())));
    !batch.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_gets_no_hub_secret_and_not_the_identity_line() {
        crate::hub::start();
        let guard = crate::hub::guard_url("log-1").expect("the hub is listening");
        let secret = guard.rsplit('/').next().unwrap().to_string();
        let args: Vec<String> = [
            "-c".into(),
            format!("hooks.PreToolUse=[{{command='curl {guard}'}}]"),
            "--append-system-prompt".into(),
            "You are a helper".into(),
            "--model".into(),
            "opus".into(),
        ]
        .into();
        let shown = args_for_log("log-1", &args);
        assert!(!shown.contains(&secret), "{shown}");
        assert_eq!(shown, "-c hooks.PreToolUse=[{command='curl <guard-url>'}] --append-system-prompt <16 chars> --model opus");
    }

    #[test]
    fn a_turn_is_not_started_as_administrator() {
        // Refused before the helper is even looked up: no such helper is what
        // a process that may start one says.
        crate::elevation::refuse_here(Some(crate::elevation::ADMIN));
        let why = crate::elevation::refusal().map(String::from);
        let refused = spawn_turn("no-such-helper", "hi");
        crate::elevation::refuse_here(None);
        assert_eq!(refused.err(), why);
    }

    #[test]
    fn a_turns_last_events_are_read_with_a_stray_holding_the_pipe() {
        let events = [r#"{"type":"assistant"}"#, "not json", r#"{"type":"result"}"#];
        let started = std::time::Instant::now();
        assert_eq!(drained_with_a_stray(&events, std::time::Duration::ZERO), ["assistant", "result"]);
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "and the stray is not waited out");
    }

    #[test]
    fn however_slowly_the_last_events_are_taken_none_is_left_behind() {
        // Each behind a stalled lock: more than a turn's end used to give them.
        let events = [r#"{"type":"one"}"#, r#"{"type":"two"}"#, r#"{"type":"result"}"#];
        assert_eq!(drained_with_a_stray(&events, std::time::Duration::from_millis(900)), ["one", "two", "result"]);
    }

    #[test]
    fn a_pipe_that_closes_ends_the_wait_at_once_and_a_last_line_counts_without_its_end() {
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        tx.send(br#"{"type":"a"}"#.to_vec()).unwrap();
        tx.send(b"\n{\"type\":\"b\"}\n{\"ty".to_vec()).unwrap();
        tx.send(br#"pe":"c"}"#.to_vec()).unwrap();
        drop(tx);
        let reader = OutReader::default();
        let mut got = Vec::new();
        reader.read_events(Pipe(rx), |v| {
            got.push(v["type"].as_str().unwrap_or("").to_string());
            true
        });
        assert_eq!(got, ["a", "b", "c"], "split across reads, and the last with no line end");
        let (drained, read_out) = std::sync::mpsc::channel::<()>();
        drop(drained);
        let started = std::time::Instant::now();
        reader.await_last_words(&read_out, std::time::Duration::from_secs(9), std::time::Duration::from_secs(9));
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    /// 2.1.322's own words, the wait in any unit it writes.
    #[test]
    fn the_clis_word_that_it_stopped_background_work_is_kept_until_the_next_turn() {
        let said = "Background tasks still running 10m after the last turn (2 agents: \"refactor\", \"tests\"); stopping them. Set CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS=0 to wait indefinitely.";
        assert!(stops_background(said));
        assert!(stops_background(&said.replace("10m", "1h 30m")));
        assert!(!stops_background("Background tasks still running"), "not without what it does about them");
        assert!(!stops_background("Error: Background tasks still running; stopping them."), "only the CLI's own line");
        let id = "bg-stopped";
        lock().entries.insert(id.into(), entry(Provider::Claude));
        let reader = OutReader::default();
        assert!(take_stderr(id, &reader, b"some warning"));
        assert!(!background_stopped(id));
        assert!(take_stderr(id, &reader, said.as_bytes()));
        assert!(background_stopped(id), "what the caller hears of the turn says so");
        lock().entries.get_mut(id).unwrap().observed.begin_turn();
        assert!(!background_stopped(id), "a new turn has its own background");
        reader.over.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(take_stderr(id, &reader, said.as_bytes()));
        assert!(!background_stopped(id), "said after its turn is over: not this helper's");
        lock().entries.remove(id);
    }

    #[test]
    fn an_event_of_a_turn_over_lands_in_nothing() {
        let id = "ev-after";
        lock().entries.insert(id.into(), entry(Provider::Codex));
        let reader = OutReader::default();
        let failed = json!({ "type": "turn.failed", "error": { "message": "boom" } });
        assert!(take_event(id, Provider::Codex, &reader, &failed));
        assert_eq!(last_error(id).as_deref(), Some("boom"));
        lock().entries.get_mut(id).unwrap().error = None;
        // The turn is over and the next may be on: this one's words stay out of it.
        reader.over.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(take_event(id, Provider::Codex, &reader, &failed), "it reads on, to keep the pipe drained");
        assert_eq!(last_error(id), None);
        lock().entries.remove(id);
        let reader = OutReader::default();
        assert!(!take_event(id, Provider::Codex, &reader, &failed), "the helper is gone: no more to read for");
    }

    /// Nobody reading stdin holds the writer and nothing else; what got in stays
    /// in order; its going ends the writer.
    #[test]
    fn a_stdin_nobody_reads_holds_up_nothing_but_its_writer() {
        let (open, pass) = std::sync::mpsc::channel();
        let wrote = std::sync::Arc::new(Mutex::new(Vec::new()));
        let stdin = Stalled { pass, wrote: wrote.clone() };
        let (feed, lines) = std::sync::mpsc::channel();
        let writer = std::thread::spawn(move || feed_stdin(stdin, "first\n", lines));
        let started = std::time::Instant::now();
        for n in 0..10_000 {
            feed.send(format!("{n}\n")).unwrap();
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "a send does not wait for the pipe");
        open.send(()).unwrap();
        open.send(()).unwrap();
        drop(feed);
        // The pipe breaks: the writer goes with most of it unwritten.
        drop(open);
        writer.join().unwrap();
        assert_eq!(String::from_utf8(wrote.lock().unwrap().clone()).unwrap(), "first\n0\n");
    }

    #[test]
    fn stdin_ends_when_its_lines_do() {
        let (feed, lines) = std::sync::mpsc::channel::<String>();
        feed.send("second\n".into()).unwrap();
        drop(feed);
        let mut out = Vec::new();
        feed_stdin(&mut out, "first\n", lines);
        assert_eq!(out, b"first\nsecond\n");
    }

    #[test]
    fn a_turn_notes_the_line_of_what_it_starts_on_while_nothing_names_it() {
        let mut e = entry(Provider::Claude);
        note_opening(&mut e, "do A", "a-id");
        assert_eq!((e.opening.as_deref(), e.opening_id.as_deref()), (Some("do A"), Some("a-id")));
        e.session = Some("sid".into());
        note_opening(&mut e, "do B", "b-id");
        assert_eq!((e.opening.as_deref(), e.opening_id.as_deref()), (None, None));
    }
}
