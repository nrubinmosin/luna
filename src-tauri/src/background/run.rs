//! A helper's turn as a process: starting it, feeding its stdin, and reading its
//! stdout and stderr until it ends.

use super::*;

/// Starts a helper on its opening prompt, in the place promised to the spawn
/// `promise` it came of (places.rs `promise_place`), which its first turn
/// takes over in the hold that makes it: counted once, as a helper at work.
pub fn start_on(id: &str, spec: Spec, prompt: &str, promise: Option<u64>) -> Result<(), String> {
    let mut e = Entry {
        spec,
        started_ms: crate::throttle::now_ms(),
        session: None,
        cwd: None,
        run: None,
        compaction: None,
        turns_ended: 0,
        queue: Vec::new(),
        in_flight: Vec::new(),
        fed: Vec::new(),
        drained: Vec::new(),
        received: Default::default(),
        log: VecDeque::new(),
        held: VecDeque::new(),
        calls: VecDeque::new(),
        error: None,
        cut_short: false,
        undelivered: Vec::new(),
        tied: false,
        resumable: false,
        unsure: false,
        cli: None,
        opening: None,
        opening_id: None,
        job: None,
        left_running: false,
        left_unlisted: false,
        outside: Vec::new(),
        left_seen_out: false,
        left: Vec::new(),
        ending_left: false,
        promoting: false,
        promoting_since: 0,
        promoting_at: std::time::Instant::now(),
        promotion: None,
        called_off: None,
        deleting: false,
        handing: false,
        waits_since: None,
        observed: Observed::default(),
    };
    claim(&mut e);
    {
        let mut r = lock();
        if let Some(request) = promise {
            r.promised.remove(&request);
        }
        r.entries.insert(id.to_string(), e);
    }
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
/// same way; behind a turn stopped before it started, or one Luna stopped
/// (`cut_short`), it is kept as undelivered.
pub(super) fn run_turn(id: &str, prompt: &str) -> Result<(), String> {
    let started = spawn_turn(id, prompt);
    // A power-off being decided or an update's stop let none start (the
    // gate, `crate::starts`): the batch waits and goes on
    // once the gate is open again (`resume_waiting`, which it calls then)
    // rather than end as an error whose senders would be told it failed. An
    // update installed meanwhile leaves it queued, and the next core keeps it
    // for its senders.
    if started.as_ref().is_err_and(|err| crate::starts::refused(err)) && requeue_refused(id) {
        reg().1.notify_all();
        save();
        changed();
        // Opened meanwhile, and nothing is left to call it.
        if crate::starts::open(id) {
            go_on(id);
        }
        return Ok(());
    }
    if let Err(err) = &started {
        if let Some(e) = lock().entries.get_mut(id) {
            if e.run.as_ref().is_some_and(|r| r.pid.is_none()) {
                let stopped = e.run.as_ref().is_some_and(|r| r.stopped);
                e.run = None;
                note(e, &format!("could not start the turn: {err}"));
                // Stopped before its process was up: the stop's reason, as
                // a stop of a turn at work keeps it (`stop_of`), not a failure.
                e.error = match e.error.take().filter(|why| stopped && is_stop(why)) {
                    Some(why) => Some(why),
                    None => Some(err.clone()),
                };
                e.turns_ended += 1;
                if !e.queue.is_empty() {
                    // Behind a claim a stop ended before its process started
                    // (the person's Stop, which dropped what was queued then, or
                    // Luna's own, an update's say: `stop_for_luna`), or one a
                    // power-off or an update let none start under (the gate),
                    // it came after that stop and never got its chance: kept
                    // for its senders.
                    if stopped || e.cut_short || crate::starts::refused(err) {
                        let queued = std::mem::take(&mut e.queue);
                        e.undelivered.extend(queued);
                        note(e, "kept what was queued behind it for its senders");
                    } else {
                        e.queue.clear();
                        note(e, "dropped what was queued behind it");
                    }
                    e.turns_ended += 1;
                }
                e.observed.stopped = e.error.clone().filter(|why| stopped && is_stop(why)).map(|why| (e.turns_ended, why));
            }
        }
        reg().1.notify_all();
        changed();
    }
    started
}

/// A turn claimed for a batch and refused at the gate before its process
/// started: the batch is queued again ahead of what came behind it, the claim
/// given back, and the turn counted for nothing. False where there is no batch
/// to keep (a helper's opening prompt rides on no batch), a stop took the claim,
/// or Luna is stopping the turn (`cut_short`): the error path keeps those.
fn requeue_refused(id: &str) -> bool {
    let mut r = lock();
    let Some(e) = r.entries.get_mut(id) else { return false };
    let waiting = e.run.as_ref().is_some_and(|run| run.pid.is_none() && !run.stopped);
    if !waiting || e.in_flight.is_empty() || e.cut_short {
        return false;
    }
    let behind = std::mem::take(&mut e.queue);
    e.queue = std::mem::take(&mut e.in_flight);
    e.queue.extend(behind);
    e.run = None;
    note(e, "its turn waits: Luna is shutting down or updating, and starts nothing meanwhile");
    true
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
pub(super) const STOPPED: &str = "it was stopped before it started";
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
    let drained = e.drained.len();
    let held = e.held.len();
    let (landed, heard) = match provider {
        Provider::Claude => (on_claude_event(e, v), Heard::default()),
        Provider::Codex => {
            let heard = on_message(&mut r, id, v);
            (heard.landed, heard)
        }
    };
    let Some(e) = r.entries.get(id) else { return false };
    let named = unnamed && e.session.is_some();
    let moved = drained_since(e, drained);
    // An ask of its CLI's, held for the turn this reader's process runs,
    // under this lock (ask.rs `take_ask`); and what it held that its stdin's
    // closing has made unanswerable lets go.
    if provider == Provider::Claude {
        if let Some((request_id, request)) = permission_request(v) {
            take_ask(&mut r, id, &request_id, &request);
        }
        if let Some(e) = r.entries.get_mut(id) {
            drop_dead_asks(e);
        }
    }
    let asks_moved = r.entries.get(id).is_some_and(|e| e.held.len() != held);
    drop(r);
    if asks_moved {
        reg().1.notify_all();
        changed();
    }
    // Held for its caller's answer, which goes back to the server.
    for serial in &heard.asks {
        reply_when_answered(id.to_string(), *serial);
    }
    if !heard.asks.is_empty() || heard.settled {
        changed();
    }
    if heard.ended {
        see_out(id);
    }
    // Its text saved as sent first, then the turn asked for.
    if heard.turn_ready {
        send_turn(id);
    }
    if heard.turn_started {
        crate::cli::session_start_through(id);
    }
    // Moved from its queue onto this process's stdin: its senders are owed
    // by this process now, under the registry's lock, which goes first.
    if !moved.is_empty() {
        crate::agents::moved_onto_stdin(id, &moved);
    }
    // At once: a core killed now would take it up as one that never got started,
    // and start its work over; or hand back as undelivered what the model has.
    if named || landed || heard.approval_changed {
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

/// How much of the end of a failed turn's stderr goes into Luna's log.
const STDERR_LOGGED: usize = 1000;

/// How many characters of what a failed turn's stream said go into Luna's log:
/// an error is short, and a `result` that carries the model's words is cut.
const ERROR_LOGGED: usize = 200;

/// A turn whose process went with anything but success, or whose stream said
/// it failed, in Luna's log with its cause: what its stream said
/// (`Entry::error`: a `result`'s error, Codex's `turn.failed`, or Luna's own
/// stop), at most `ERROR_LOGGED` characters, and the last `STDERR_LOGGED`
/// bytes of its stderr, on one line and each `redacted`, so that a caller's
/// "failed" has a cause on record. Luna's own stop is not a failure of the
/// CLI's.
fn log_failed_end(id: &str, provider: Provider, code: Option<i32>, stderr: &str) {
    let Some((error, stopped)) = lock()
        .entries
        .get(id)
        .map(|e| (e.error.clone(), e.run.as_ref().is_some_and(|run| run.luna_stopped)))
        .filter(|(error, _)| code != Some(0) || error.is_some())
    else {
        return;
    };
    let how = match code {
        Some(c) => format!("exited with code {c}"),
        None => "was killed".to_string(),
    };
    let said = error.map_or_else(|| "its stream said nothing of it".to_string(), |e| first_chars(&one_line(&redacted(&e)), ERROR_LOGGED));
    let line = format!(
        "{}: its turn's {} {how}: {said}; stderr: {}",
        crate::agents::label(id),
        provider.as_str(),
        stderr_tail(&redacted(stderr), STDERR_LOGGED)
    );
    if stopped {
        crate::log::info("background", &line);
    } else {
        crate::log::warn("background", &line);
    }
}

/// `text`'s first `n` characters, marked where cut.
fn first_chars(text: &str, n: usize) -> String {
    if text.chars().count() <= n {
        return text.to_string();
    }
    format!("{}…", text.chars().take(n).collect::<String>())
}

/// Keys whose value `redacted` takes out wherever they stand: `"token":"…"`,
/// `token=…`, `Authorization: …`, lower case as compared.
const SECRET_KEYS: &[&str] = &[
    "authorization",
    "token",
    "access_token",
    "refresh_token",
    "id_token",
    "api_key",
    "apikey",
    "x-api-key",
    "password",
    "secret",
    "client_secret",
];

/// Where in `lower` (a word, lower case) the value of a `SECRET_KEYS` key
/// begins, after its `":`, `=` or `:`, the key standing at the word's start or
/// after a character that is no part of a name.
fn secret_value_at(lower: &str) -> Option<usize> {
    let named = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    SECRET_KEYS
        .iter()
        .flat_map(|key| ["\":", "=", ":"].map(|sep| format!("{key}{sep}")))
        .filter_map(|pat| {
            lower.match_indices(pat.as_str()).find(|(i, _)| !lower[..*i].chars().next_back().is_some_and(named)).map(|(i, _)| i + pat.len())
        })
        .min()
}

/// `text`, its lines kept and their spaces made one, with what may be a
/// secret taken out, for Luna's log: a
/// `SECRET_KEYS` key's value (in the same word or the next), the word after
/// `Bearer`, a URL's user info and its path and query (a hook URL carries its
/// chat's secret there), and any run of 24 or more letters, digits, `-` and
/// `_` with a digit in it (a token, a key, a hash). Taken out before any cut,
/// so a cut never keeps a value without what named it. Luna gives a helper no
/// secret it could print (its MCP token goes by environment, F3; it has no
/// hook URL), so this is for the CLI's own.
fn redacted(text: &str) -> String {
    let named = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
    let mut lines: Vec<String> = Vec::new();
    // Across lines too: `Bearer` at one's end names the next one's start.
    let mut next_is_secret = false;
    for line in text.lines() {
        let mut out: Vec<String> = Vec::new();
        for word in line.split_whitespace() {
            let lower = word.to_ascii_lowercase();
            let bare = lower.trim_matches(|c: char| !c.is_ascii_alphanumeric());
            // `Bearer` names what follows it, whatever came before.
            if bare == "bearer" || lower.ends_with("bearer") {
                next_is_secret = true;
                out.push(word.to_string());
                continue;
            }
            if next_is_secret {
                next_is_secret = false;
                out.push("<redacted>".into());
                continue;
            }
            if let Some(at) = secret_value_at(&lower) {
                let value = lower[at..].trim_matches(|c: char| !c.is_ascii_alphanumeric());
                // Its value is the next word: `"token": "…"`, `Authorization: …`.
                next_is_secret = value.is_empty();
                out.push(if value.is_empty() { word.to_string() } else { format!("{}<redacted>", &word[..at]) });
                continue;
            }
            if let Some(at) = word.find("://") {
                let rest = &word[at + 3..];
                let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
                let host = match rest[..end].rfind('@') {
                    Some(user) => format!("<redacted>@{}", &rest[user + 1..end]),
                    None => rest[..end].to_string(),
                };
                let path = if end < rest.len() { "/<redacted>" } else { "" };
                out.push(format!("{}{host}{path}", &word[..at + 3]));
                continue;
            }
            out.push(
                word.split_inclusive(|c: char| !named(c))
                    .map(|part| {
                        let core = part.trim_end_matches(|c: char| !named(c));
                        if core.len() >= 24 && core.chars().any(|c| c.is_ascii_digit()) {
                            format!("<redacted>{}", &part[core.len()..])
                        } else {
                            part.to_string()
                        }
                    })
                    .collect(),
            );
        }
        lines.push(out.join(" "));
    }
    lines.join("\n")
}

/// The last `at_most` bytes of `stderr`, from a character's start, its lines
/// on one line; "(none)" for nothing.
fn stderr_tail(stderr: &str, at_most: usize) -> String {
    let start = (stderr.len().saturating_sub(at_most)..=stderr.len()).find(|&i| stderr.is_char_boundary(i)).unwrap_or(stderr.len());
    let tail = stderr[start..].lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" | ");
    if tail.is_empty() {
        "(none)".into()
    } else {
        tail
    }
}

/// A line off a turn's stderr, into its helper's log, under the same flag as
/// `take_event`: what a process the turn left writes there after the turn is
/// over is neither the helper's word nor a sign of it at work. Read on to the
/// end all the same, so that nothing writing there blocks.
pub(super) fn take_stderr(id: &str, reader: &OutReader, line: &[u8]) -> bool {
    let line = String::from_utf8_lossy(line);
    let line = line.trim();
    if line.is_empty() || line.starts_with("Reading additional input") {
        return true;
    }
    let mut r = lock();
    if reader.over.load(std::sync::atomic::Ordering::SeqCst) {
        return true;
    }
    let mut stopped = None;
    if let Some(e) = r.entries.get_mut(id) {
        e.observed.event_ms = crate::throttle::now_ms();
        if stops_background(line) {
            // Of this turn, which ends when the process does; whom it
            // concerns comes below (`stop_owed_to`), which subagents it took
            // at that end (`turn_over`), once all of its stdout is read: a
            // subagent's end still on its way there is not undone by this.
            let turn = e.turns_ended + 1;
            record_loss(&mut e.observed, turn, StopCause::Ceiling, None, Vec::new(), Vec::new());
            stopped = Some(turn);
        }
        note(e, &format!("stderr: {}", one_line(line)));
    }
    drop(r);
    if let Some(turn) = stopped {
        // Whom it concerns, by the registry's lock, which goes before this
        // one: whoever is owed an answer now, the end of this turn or of one a
        // message queued behind it starts.
        stop_owed_to(id, turn, crate::agents::callers_owed(id));
    }
    true
}

/// Whether a line of `claude -p`'s stderr is its word that it stops what the
/// last turn left in the background, its stdin closed and its wait for that
/// over (2.1.319+: "Background tasks still running 10m after the last turn
/// (…); stopping them. Set CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS=0 to wait
/// indefinitely."). A helper's turn sets that to 3 hours (command.rs
/// `BG_WAIT_CEILING_MS`): past that, this is how its caller learns why.
fn stops_background(line: &str) -> bool {
    line.starts_with("Background tasks still running") && line.contains("; stopping them.")
}

/// The longest a turn waits for Luna's first copy of its CLI
/// (`cli::await_install`): well inside the tool call a helper's spawn
/// answers (pty/start.rs `MCP_TOOL_TIMEOUT_MS`).
const FIRST_INSTALL_WAIT: std::time::Duration = std::time::Duration::from_secs(10 * 60);

fn spawn_turn(id: &str, prompt: &str) -> Result<(), String> {
    // Refused before anything of the turn is built while the start gate is
    // closed (`crate::starts`); asked again just before the process, below,
    // after the activity stamp a power-off's decision reads.
    crate::starts::open_or_refuse(id)?;
    // Nothing runs as administrator.
    crate::elevation::guard()?;
    // A turn asked for before Luna's first copy of its CLI is in waits for
    // it, and goes off if it is stopped or the gate closes meanwhile; a
    // download that fails, or is still under way past `FIRST_INSTALL_WAIT`,
    // is said to whoever asked.
    let provider = lock().entries.get(id).map(|e| e.spec.provider).ok_or("no such helper")?;
    crate::cli::await_install(provider, FIRST_INSTALL_WAIT, &|| {
        crate::starts::open_or_refuse(id)?;
        match lock().entries.get(id) {
            None => Err("no such helper".into()),
            Some(e) if e.run.as_ref().is_some_and(|run| run.stopped) => Err(STOPPED.into()),
            Some(_) => Ok(()),
        }
    })?;
    // Before the registry is locked for the command line: a pass reads and
    // writes every account's copies, and every helper's events wait on that
    // lock (library/folders.rs `session_args`).
    // The helper's account counted as in use from here, as a chat's start
    // counts its own (`library::lay_out_for`).
    let (account_path, project, role) =
        lock().entries.get(id).map(|e| (e.spec.account_path.clone(), e.spec.folder.clone(), role_of(&e.spec))).ok_or("no such helper")?;
    let reserved = crate::library::lay_out_for("before a helper's turn", &account_path)?;
    // Its role's texts, also off the lock: an override is a file of the
    // person's, read from disk.
    let texts = crate::library::role_texts(role);
    let compact_tokens = crate::settings::delegate_auto_compact_tokens(crate::settings::get().delegate_auto_compact_tokens);
    let (args, dir, envs, provider, on_stdin, line, account, open, effort, applied) = {
        let r = lock();
        if r.closed {
            return Err("Luna is stopping".into());
        }
        let e = r.entries.get(id).ok_or("no such helper")?;
        if e.run.as_ref().is_some_and(|run| run.stopped) {
            return Err(STOPPED.into());
        }
        let ((args, dir, envs, applied), open) = command_and_open(e, id, &texts, compact_tokens)?;
        // A batch's own, on disk by now; what no batch carries needs only to be a uuid.
        let line = e.in_flight.first().map_or_else(new_id, |q| q.id.clone());
        (
            args,
            dir,
            envs,
            e.spec.provider,
            stdin_with(e, prompt, texts.note.as_deref()),
            line,
            e.spec.account_path.clone(),
            open,
            e.spec.effort.clone(),
            applied,
        )
    };
    // The top chat hears of a change in the folder-skills failure, off the
    // helpers' lock (the registry's is never taken under it).
    let skills_off = applied.iter().find(|a| a.key == crate::applied::Item::LibrarySkillsOff.key()).map(|a| a.value.as_str());
    crate::agents::turn_folder_skills(id, skills_off);
    if !std::path::Path::new(&dir).is_dir() {
        return Err(format!("{dir} does not exist"));
    }
    // Its version directory held until the process has exited (cli.rs `launch`).
    let (exe, on_version) = crate::cli::launch(provider, Some(id))?;
    let codex_compaction = if provider == Provider::Codex {
        let trust = crate::codex::trust::override_key(&project);
        crate::codex::compact::at_launch_with(id, &account, &dir, Some(&trust), compact_tokens)
    } else {
        None
    };
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
                let pid = child.id();
                let code = child.wait().ok().and_then(|s| s.code());
                if let Some(run) = lock().entries.get_mut(&id).and_then(|e| e.run.as_mut()).filter(|run| run.pid == Some(pid)) {
                    run.exited = true;
                }
                crate::cli::session_exited(&id);
                reader.await_last_words(&read_out, DRAINED_AFTER, DRAIN_AT_MOST);
                err_reader.await_last_words(&err_out, DRAINED_AFTER, DRAIN_AT_MOST);
                reader.over.store(true, std::sync::atomic::Ordering::SeqCst);
                err_reader.over.store(true, std::sync::atomic::Ordering::SeqCst);
                log_failed_end(&id, provider, code, &said.lock().unwrap_or_else(|e| e.into_inner()));
                // For the canary over a new CLI version's first sessions: the
                // end of its stderr, and what its own stream said the turn
                // failed on (a `result`, a Codex server's `error`), where a usage
                // limit or a refused sign-in is said, read out by now. No exit
                // code (a signal's end on Unix) is an error, and so is a Codex
                // server that broke the protocol, whatever it exited with.
                let broken = went_wrong(&id).is_some();
                crate::cli::session_ended(&id, code != Some(0) || broken, || crate::cli::LastWords {
                    said: std::mem::take(&mut *said.lock().unwrap_or_else(|e| e.into_inner())),
                    failure: lock().entries.get(&id).and_then(|e| e.error.clone()),
                });
                turn_over(&id, code);
            })
            .map_err(thread_err)?;
    }
    // Held by the thread above until it has reaped the process; dropped here
    // on the ways out where the turn does not start.
    let on_account = crate::account_use::running_on(&account);
    drop(reserved);
    // A turn about to start is activity, and a power-off being decided or an
    // update's stop lets none start: the stamp first, then the gate
    // (`crate::starts`). Its claim is in the registry by now, so a stop that
    // closed the gate after this lists the turn and stops it.
    crate::activity::touch();
    crate::starts::open_or_refuse(id)?;
    // Ahead of the process: what it starts is born after this (`turn_over`).
    let started_ms = crate::throttle::now_ms();
    let mut child = cmd.spawn().map_err(|e| {
        crate::log::error("background", &format!("could not start {}: {e}", crate::agents::label(id)));
        format!("could not start {}: {e}", provider.as_str())
    })?;
    // Up: the canary may count it (a spawn that failed never is).
    crate::cli::session_running(id);
    // A Codex server's start goes on until `turn/start` is answered, which
    // can take past the canary's half minute: a server that never gets there
    // still counts against its version (`watch`, appserver.rs).
    if provider == Provider::Codex {
        crate::cli::session_awaits_start(id);
    }
    let pid = Some(child.id());
    // Into the helper's job before it has had time to start anything. What is
    // waited for here is not waited for with every helper's lock held.
    if let Some(pid) = pid {
        crate::procs::await_environment(pid);
        // Read before it can exit, so a pid reused later is not taken for it.
        let cli = crate::procs::started_of(pid).map(|started| (pid, started));
        if let Some(e) = lock().entries.get_mut(id) {
            e.cli = cli;
            // What a turn that is in no job starts is listed nowhere: not
            // known, until a walk below it finds nothing (`outside`). Off
            // Windows a refused `join` is a tie the keeper does not have, the
            // turn listed by its pid all the same.
            let inside = match &e.job {
                Some(job) => {
                    let joined = crate::procs::join(job, pid);
                    e.tied = e.tied && joined;
                    joined || !cfg!(windows)
                }
                None => {
                    e.job = crate::procs::adopt(pid).map(std::sync::Arc::new);
                    e.tied = e.job.as_deref().is_some_and(|job| crate::procs::tie(job, id));
                    e.job.is_some()
                }
            };
            if !inside {
                crate::log::warn(
                    "background",
                    &format!("{}: its turn is in no job; what it starts is walked by parent", crate::agents::label(id)),
                );
                e.outside.push((pid, started_ms));
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
    // Claude Code's: open until its first `result` (`on_claude_event`), for
    // what joins the turn. A Codex server's: open until `turn/completed`
    // (appserver.rs), the turn's text going in its `turn/start`.
    let first = match provider {
        Provider::Claude => user_line(&line, &on_stdin),
        Provider::Codex => initialize_line(),
    };
    if let Some(stdin) = stdin {
        let spawned = std::thread::Builder::new().spawn(move || feed_stdin(stdin, &first, lines));
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
                let rpc = open.map(|open| {
                    // The Codex command adds this variable with Luna's server.
                    let luna_supplied = envs.iter().any(|(name, _)| *name == "LUNA_MCP_TOKEN");
                    Rpc::new(open, on_stdin.clone(), effort, opening_line.clone(), luna_supplied)
                });
                e.run = Some(Run { feed: Some(feed), rpc, line: claude.then_some(line), ..Run::claimed(pid, started_ms) });
                e.compaction = Some(crate::to_compact::Launch {
                    provider,
                    account_path: account.clone(),
                    folder: dir.clone(),
                    start: started_ms,
                    claude_window: compact_tokens.filter(|_| claude),
                    codex: codex_compaction,
                });
                e.observed.begin_turn();
                e.error = None;
                e.cut_short = false;
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
    reg().1.notify_all();
    // A Codex turn that stops speaking is not left to hang (appserver.rs `stalled`).
    if let (Provider::Codex, Some(pid)) = (provider, pid) {
        watch(id, pid);
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

/// A turn's arguments as the log keeps them. The chat's hook URL carries its
/// secret (an argument that held it would be a `-c` override) and goes in as
/// what it is.
fn args_for_log(id: &str, args: &[String]) -> String {
    let urls: Vec<(String, &str)> = crate::hub::hook_url(id).map(|url| (url, "<hook-url>")).into_iter().collect();
    args.iter().map(|a| urls.iter().fold(a.clone(), |s, (url, to)| s.replace(url.as_str(), to))).collect::<Vec<_>>().join(" ")
}

/// The turn has taken in its opening batch. Returns whether it held any text.
pub(super) fn stdin_took(e: &mut Entry) -> bool {
    let batch = std::mem::take(&mut e.in_flight);
    for q in &batch {
        super::events::received(e, q);
    }
    !batch.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_gets_no_hub_secret() {
        crate::hub::start();
        let hook = crate::hub::hook_url("log-1").expect("the hub is listening");
        let args: Vec<String> = ["-c".into(), format!("hooks.Stop=[{{command='curl {hook}'}}]"), "--model".into(), "opus".into()].into();
        let shown = args_for_log("log-1", &args);
        assert!(shown == "-c hooks.Stop=[{command='curl <hook-url>'}] --model opus", "only the hook URL is redacted");
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

    /// 2.1.322's own words, the wait in any unit it writes, kept as the stop
    /// of the turn it came in. Who is to hear of it is the delivery tests'.
    #[test]
    fn the_clis_word_that_it_stopped_background_work_is_kept_for_its_turn() {
        let said = "Background tasks still running 10m after the last turn (2 agents: \"refactor\", \"tests\"); stopping them. Set CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS=0 to wait indefinitely.";
        assert!(stops_background(said));
        assert!(stops_background(&said.replace("10m", "1h 30m")));
        assert!(!stops_background("Background tasks still running"), "not without what it does about them");
        assert!(!stops_background("Error: Background tasks still running; stopping them."), "only the CLI's own line");
        let id = "bg-stopped";
        lock().entries.insert(id.into(), entry(Provider::Claude));
        let reader = OutReader::default();
        assert!(take_stderr(id, &reader, b"some warning"));
        let turns = || lock().entries[id].observed.losses.iter().map(|l| l.turn).collect::<Vec<_>>();
        assert!(turns().is_empty());
        assert!(take_stderr(id, &reader, said.as_bytes()));
        assert_eq!(turns(), [1], "the stop of the turn under way");
        assert!(!background_stopped(id), "nobody was owed anything of it: nobody to tell");
        lock().entries.get_mut(id).unwrap().turns_ended = 1;
        reader.over.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(take_stderr(id, &reader, said.as_bytes()));
        assert_eq!(turns(), [1], "said after its turn is over: not this helper's");
        lock().entries.remove(id);
    }

    #[test]
    fn an_event_of_a_turn_over_lands_in_nothing() {
        let id = "ev-after";
        lock().entries.insert(id.into(), entry(Provider::Codex));
        let reader = OutReader::default();
        let failed = json!({ "method": "error", "params": { "error": { "message": "boom" }, "willRetry": false } });
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

    /// What a failed turn's log line keeps takes out what may be a secret:
    /// a bearer token, a URL's path, a long run that looks like a key.
    #[test]
    fn a_failed_turns_log_line_holds_no_secret_shaped_words() {
        assert_eq!(redacted("API Error: 401 authentication_error"), "API Error: 401 authentication_error");
        assert_eq!(redacted("https://api.anthropic.com"), "https://api.anthropic.com");
        for (case, raw, want) in [
            ("authorization header", "Authorization: Bearer abc.def", "Authorization: Bearer <redacted>"),
            (
                "hook URL",
                "posting to http://127.0.0.1:50101/hook/c1/0123456789abcdef?x=1 failed",
                "posting to http://127.0.0.1:50101/<redacted> failed",
            ),
            ("API key", "key sk-ant-api03-abcdefgh12345678XYZ, then", "key <redacted>, then"),
            ("token assignment", "token=0123456789abcdef0123456789abcdef01234567;", "token=<redacted>"),
            ("JSON bearer", r#"headers {"authorization":"Bearer abc123"}"#, r#"headers {"authorization":"Bearer <redacted>"#),
            ("JSON token", r#"{"token":"short","model":"x"}"#, r#"{"token":<redacted>"#),
            ("JSON access token", r#""access_token": "short", next"#, r#""access_token": <redacted> next"#),
            ("HTTPS proxy", "https://user:pass@proxy.example:8080/x?y", "https://<redacted>@proxy.example:8080/<redacted>"),
            ("SOCKS proxy", "socks5://u:p@host", "socks5://<redacted>@host"),
        ] {
            assert!(redacted(raw) == want, "{case}");
        }
        assert_eq!(redacted("tokens: 5 used"), "tokens: 5 used", "a word that only begins like a key");
        assert_eq!(redacted("a_long_identifier_without_digits_at_all"), "a_long_identifier_without_digits_at_all");
        assert_eq!(first_chars("abcdef", 3), "abc…");
        assert_eq!(first_chars("abc", 3), "abc");
    }

    /// Redacted before it is cut: a cut never keeps a value without what
    /// named it.
    #[test]
    fn a_failed_turns_stderr_is_redacted_before_it_is_cut() {
        // `Bearer` at one line's end, its value on the next, past the cut.
        let stderr = format!("{}\nAuthorization: Bearer\nshort-secret-value tail", "x".repeat(60));
        let cut = stderr_tail(&redacted(&stderr), 20);
        assert!(!cut.contains("short-secret-value") && cut.ends_with("<redacted> tail"), "multiline bearer is redacted before truncation");
    }

    /// What a failed turn's log line keeps of its stderr: the end, from a
    /// character's start, its lines on one.
    #[test]
    fn a_failed_turns_stderr_is_logged_by_its_end_on_one_line() {
        assert_eq!(stderr_tail("", 10), "(none)");
        assert_eq!(stderr_tail("\n  \n", 10), "(none)");
        assert_eq!(stderr_tail("first\n\n  second  \n", 100), "first | second");
        assert_eq!(stderr_tail("dropped\nkept end\n", 9), "kept end");
        // "é" is two bytes: a cut inside it starts after it.
        assert_eq!(stderr_tail("aéb", 2), "b");
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

    /// A turn the gate refuses (a power-off being decided) is not an error to
    /// its senders: its batch waits, ahead of what came behind it.
    #[test]
    fn a_turn_refused_at_the_gate_keeps_its_batch_queued() {
        let mut e = entry(Provider::Claude);
        e.run = Some(Run::claimed(None, 0));
        e.in_flight = vec!["flying".into()];
        e.queue = vec!["behind it".into()];
        lock().entries.insert("gate-refused".into(), e);
        assert!(requeue_refused("gate-refused"));
        {
            let r = lock();
            let e = &r.entries["gate-refused"];
            assert!(e.run.is_none() && e.in_flight.is_empty(), "the claim is given back");
            assert_eq!(e.queue, ["flying", "behind it"]);
            assert!(e.error.is_none() && e.undelivered.is_empty() && e.turns_ended == 0, "not an ended turn, nothing handed back");
        }
        // No batch (an opening prompt), a stop's claim, or Luna's own stop: the error path.
        let mut opening = entry(Provider::Claude);
        opening.run = Some(Run::claimed(None, 0));
        lock().entries.insert("gate-opening".into(), opening);
        assert!(!requeue_refused("gate-opening"));
        let mut cut = entry(Provider::Claude);
        cut.run = Some(Run::claimed(None, 0));
        cut.in_flight = vec!["flying".into()];
        cut.cut_short = true;
        lock().entries.insert("gate-cut".into(), cut);
        assert!(!requeue_refused("gate-cut"));
        let mut r = lock();
        for id in ["gate-refused", "gate-opening", "gate-cut"] {
            r.entries.remove(id);
        }
    }

    /// A send to an idle helper that the
    /// gate refuses while the sessions are stopped for an update waits in the
    /// queue: not an ended turn, nothing failed or handed back, for a
    /// call-off to start or the next core to keep for its sender.
    #[test]
    fn a_send_the_update_gate_refuses_waits_in_the_queue() {
        let _alone = crate::agents::stops_for_test();
        let id = "gate-update-send";
        crate::starts::under_update_for_test(id);
        for provider in [Provider::Claude, Provider::Codex] {
            lock().entries.insert(id.into(), entry(provider));
            let stopping = crate::agents::freeze_for_update(None, crate::agents::update_call_offs()).unwrap();
            assert_eq!(send(id, "after the stop", Some("gate-caller")), Ok(Placed::Started), "{provider:?}");
            {
                let r = lock();
                let e = &r.entries[id];
                assert!(e.run.is_none() && e.in_flight.is_empty(), "{provider:?}: the claim is given back");
                assert_eq!(e.queue, ["after the stop"], "{provider:?}");
                assert!(e.error.is_none() && e.undelivered.is_empty() && e.turns_ended == 0, "{provider:?}: not an ended turn");
            }
            drop(stopping);
            crate::agents::call_off_update();
            lock().entries.remove(id);
        }
    }
}
