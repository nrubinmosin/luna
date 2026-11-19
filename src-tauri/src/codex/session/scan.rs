//! The running read of a chat's rollout, from where the last one stopped: the
//! turns begun and ended, the failure a turn ended on, a question left open, and
//! the subagents at work under it.

use super::*;
use std::sync::Arc;

/// What the turn tracker of each chat has read of its rollout, and where it
/// stopped.
pub(super) struct TurnCount {
    path: PathBuf,
    /// Bytes consumed: the end of the last complete line.
    offset: u64,
    scan: Scan,
    /// The rollouts it was on before this one in this process (a `/new`, a
    /// `/resume` in its TUI), by path, as far as they were read: their turns
    /// count on with this one's, once each, and one gone back to goes on from
    /// where it was left.
    before: HashMap<PathBuf, (u64, Scan)>,
}

/// The rollout read so far, as the tracker keeps it.
#[derive(Default, Clone)]
pub(super) struct Scan {
    /// Turns ended since the process was spawned.
    pub(super) completed: u32,
    /// Whether the last turn event read started a turn that has not ended;
    /// None before any.
    pub(super) working: Option<bool>,
    /// The question the model last put to the user that nothing has answered
    /// yet, with the call id it came in.
    pub(super) ask: Option<(String, crate::agents::Ask)>,
    /// Why the last turn stopped short, if it did: `task_complete` carries the
    /// error it ended on (Codex 0.166), `turn_aborted` why it was cut off.
    pub(super) failed: Option<String>,
    /// What the model last said, as the last turn's end has it; None while a
    /// turn is on.
    pub(super) last_said: Option<String>,
    /// When that end was written.
    pub(super) last_said_at: Option<String>,
    /// When the first turn of this process began, by its `task_started`
    /// line's time (epoch ms): what in its job came before is the CLI's own.
    pub(super) first_started_ms: Option<i64>,
    /// The last turn begun is dated before this process and nothing places
    /// it after the spawn (`Since::offset`): whether it is this process's, on
    /// now, or an earlier one's, gone with it, is not known (`working` None).
    /// A wall clock set back writes a new turn's line so.
    pub(super) doubt: bool,
}

/// What tells a rollout's lines of this process from those of one before it.
#[derive(Clone, Copy, Debug)]
pub(super) struct Since {
    /// The process's spawn, epoch ms: a line dated at or after it is new.
    pub(super) ms: i64,
    /// Where the file ended as the process was spawned, where Luna saw it
    /// then (the thread it resumed): a line before that is old and one past it
    /// new, whatever its date says.
    pub(super) offset: Option<u64>,
}

impl Since {
    /// By the lines' dates alone.
    pub(super) fn dated(ms: i64) -> Since {
        Since { ms, offset: None }
    }
}

/// Each chat's rollout as it stood when its process was spawned, where it
/// resumed a thread (`note_spawn`): the file and its length then.
fn at_spawn() -> &'static Mutex<HashMap<String, (u128, PathBuf, u64)>> {
    static S: OnceLock<Mutex<HashMap<String, (u128, PathBuf, u64)>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// A Codex process spawned at `spawned_at_ms` on the thread `resume`: its
/// rollout's length now, before the process writes to it, is where its own
/// lines begin. Every line before is an earlier process's, an open turn there
/// one that went with it, whatever the wall clock said or says.
pub fn note_spawn(chat_id: &str, spawned_at_ms: u128, account_path: &str, resume: Option<&str>) {
    let found = resume.and_then(|thread| thread_file(account_path, thread));
    let mut m = at_spawn().lock().unwrap_or_else(|e| e.into_inner());
    match found.and_then(|p| Some((std::fs::metadata(&p).ok()?.len(), p))) {
        Some((len, path)) => m.insert(chat_id.to_string(), (spawned_at_ms, path, len)),
        None => m.remove(chat_id),
    };
}

/// What tells the lines of `path` of the live process from an earlier one's.
fn since_for(live: &Live, path: &Path) -> Since {
    let m = at_spawn().lock().unwrap_or_else(|e| e.into_inner());
    let offset = m.get(live.chat_id).filter(|(at, p, _)| *at == live.spawned_at_ms && p == path).map(|(_, _, len)| *len);
    Since { ms: live.spawned_at_ms as i64, offset }
}

/// Each chat's turn tracker, behind a lock of its own: a look reads and
/// parses what its rollout gained under it, from byte zero the first time,
/// and behind one lock for all of them one chat's read held up every other
/// chat's metadata, activity and questions. The map's own lock is held only
/// to find or drop a chat's tracker.
pub(super) fn turn_counts() -> &'static Mutex<HashMap<String, Arc<Mutex<Option<TurnCount>>>>> {
    static C: OnceLock<Mutex<HashMap<String, Arc<Mutex<Option<TurnCount>>>>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// The rollout read on from where the last look stopped. Counted off the
/// file rather than off sampled state, so a turn that starts and ends between
/// two reads of the records still counts — a one-word answer takes
/// two seconds, the records are read every five — and read whole rather than by
/// its tail, which a long turn outgrows: a megabyte of tool output after
/// `task_started` and the tail read the session as resting. None until the
/// rollout exists, i.e. before the first turn.
pub(super) fn tracked(live: &Live) -> Option<Scan> {
    tracked_at(live, rollout_for(live)?)
}

/// `tracked`, the chat on the rollout at `path` now.
pub(super) fn tracked_at(live: &Live, path: PathBuf) -> Option<Scan> {
    tracked_fresh(live, path).map(|(scan, _)| scan)
}

/// `tracked_at`, and whether the file read through to its end this time: one
/// that would not open, seek or read leaves the scan of the reads before,
/// which is no word on what came since (`rollout_read`).
pub(super) fn tracked_fresh(live: &Live, path: PathBuf) -> Option<(Scan, bool)> {
    let tracker = Arc::clone(turn_counts().lock().unwrap_or_else(|e| e.into_inner()).entry(live.chat_id.to_string()).or_default());
    let mut tracker = tracker.lock().unwrap_or_else(|e| e.into_inner());
    let entry = tracker.get_or_insert_with(|| TurnCount { path: path.clone(), offset: 0, scan: Scan::default(), before: HashMap::new() });
    let since = since_for(live, &path);
    if entry.path != path {
        // What the one it leaves wrote since the last look counts first: a turn
        // that ended just before the `/new`. Read no further after that: what
        // comes there later is another process's on that thread.
        if !past_end(&entry.path, entry.offset) {
            entry.offset = scan_from(&entry.path, entry.offset, since_for(live, &entry.path), &mut entry.scan).0;
        }
        let (offset, scan) = entry.before.remove(&path).unwrap_or_default();
        let was = std::mem::replace(&mut entry.path, path.clone());
        let left = (std::mem::replace(&mut entry.offset, offset), std::mem::replace(&mut entry.scan, scan));
        entry.before.insert(was, left);
    }
    let through = if past_end(&path, entry.offset) {
        // Cut short or written anew under the same name: what was read of it
        // no longer lines up, so it is read again from the top. The count
        // goes on from the higher of the two. Started over lower, it would
        // sit under the count `activity` already gave out (which only grows)
        // until it caught up, and a wait for the next turn would not wake;
        // added on top, turns the new file holds that were counted already
        // would count twice and wake one early. Old turn ends written back
        // after this still count as new ones; a floor under the file's own
        // count would not, but would hold back a real new turn the same way
        // starting over lower does. The CLIs only append, so a rollout cut
        // short is rare, and one that later gets its old turns back rarer.
        let mut scan = Scan::default();
        let (offset, through) = scan_from(&path, 0, since, &mut scan);
        scan.completed = scan.completed.max(entry.scan.completed);
        (entry.offset, entry.scan) = (offset, scan);
        through
    } else {
        let through;
        (entry.offset, through) = scan_from(&path, entry.offset, since, &mut entry.scan);
        through
    };
    let mut scan = entry.scan.clone();
    scan.completed += entry.before.values().map(|(_, s)| s.completed).sum::<u32>();
    Some((scan, through))
}

/// Whether `offset` lies past the end of the file: a cursor into one since
/// cut short or written anew.
pub(super) fn past_end(path: &Path, offset: u64) -> bool {
    std::fs::metadata(path).is_ok_and(|m| offset > m.len())
}

/// Why this chat's last turn stopped short, if it did, as of the rollout's last
/// read: the one the turn count comes from, so it goes with the turn counted,
/// and the session may have exited since.
pub fn last_failure(chat_id: &str) -> Option<String> {
    let tracker = Arc::clone(turn_counts().lock().unwrap_or_else(|e| e.into_inner()).get(chat_id)?);
    let failed = tracker.lock().unwrap_or_else(|e| e.into_inner()).as_ref()?.scan.failed.clone();
    failed
}

/// The question the session has put to the user and nobody has answered:
/// `request_user_input`, which holds the turn until it is answered, or
/// `request_user_input_async`, which lets the model work on and is open until
/// the user next says something.
pub fn pending_ask(live: &Live) -> Option<crate::agents::Ask> {
    tracked(live)?.ask.map(|(_, a)| a)
}

/// What a chat's subagents have shown of themselves (`children_at_work`).
#[derive(Default)]
pub(super) struct Children {
    /// Each rollout looked at, with the thread its first line says spawned it:
    /// None for one that is no subagent's. A first line never changes.
    parent_of: HashMap<PathBuf, Option<String>>,
    /// A subagent's rollout as read so far, and where the read stopped. Kept
    /// for as long as the session's process lives, whatever a listing says
    /// later: its subagents run in it.
    read: HashMap<PathBuf, (u64, Scan)>,
    /// Every rollout on the days looked at, as the first look found them: one
    /// not among them was begun after it, whatever its name's date says.
    first_look: Option<std::collections::HashSet<PathBuf>>,
}

/// What stands for subagents a look could not see (`children_at_work`): a
/// folder that would not list, a known subagent's rollout that would not read.
/// One subagent at work, to the record: not known, and so active.
pub const NOT_SEEN: &str = "(not seen: its rollouts would not list or read)";

/// Each chat's, behind a lock of its own as the turn trackers are
/// (`turn_counts`): a look reads the first line of each new rollout and what
/// each of its subagents' gained under it.
pub(super) fn children() -> &'static Mutex<HashMap<String, Arc<Mutex<Children>>>> {
    static C: OnceLock<Mutex<HashMap<String, Arc<Mutex<Children>>>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// The thread a rollout's first line says spawned it, if it is a subagent's.
pub(super) fn spawned_by(meta: &Value) -> Option<String> {
    meta["payload"]["source"]["subagent"]["thread_spawn"]["parent_thread_id"].as_str().map(str::to_owned)
}

/// A live session's subagents at work, by thread id. Codex (0.166) runs them
/// in the session's own process and says nothing of them on its screen or in
/// its own turns: each writes a rollout of its own beside the session's, whose
/// first line names the thread that spawned it, and is at work while the last
/// turn there has not ended. One done with its task stays open, asleep until it
/// is written to, and is not at work; one it spawned in turn counts too. All of
/// them go with the process. None until the session's own rollout is known.
///
/// Only threads begun since the process was spawned, so not one the session
/// spawned in an earlier run and wakes now, nor the subagents of a thread it
/// has left (`/new`, `/resume`), whatever becomes of those: by the name's date,
/// or, for one dated earlier (a wall clock set back), by its not being there at
/// the first look. One found is kept for the process's life, past the two
/// weeks of folders looked at and past a listing that fails. What cannot be
/// seen (a folder that would not list, a subagent's rollout that would not
/// read) is `NOT_SEEN`, at work; but a rollout that is no longer there once the
/// session's own turn has ended is ended (see the loop).
pub fn children_at_work(live: &Live) -> Option<Vec<String>> {
    let spawned = local_at(live.spawned_at_ms as i64)?;
    // A subagent's thread is begun after the session's process, give or take
    // the clocks, in the folder of the day it was begun: every day since the
    // spawn, for a session days old whose subagent from its first day is
    // woken now. Two weeks at most.
    let not_before = spawned - 5.seconds();
    let today = local_today();
    let days = today.since(not_before.date()).map_or(0, |s| s.get_days()).clamp(0, 13) as usize + 1;
    let (all, whole) = rollouts_listed(live.account_path, days_back(today, days));
    let seen = Arc::clone(children().lock().unwrap_or_else(|e| e.into_inner()).entry(live.chat_id.to_string()).or_default());
    let mut seen = seen.lock().unwrap_or_else(|e| e.into_inner());
    // Taken before the session's own rollout is there, at the first read.
    let first_look = seen.first_look.get_or_insert_with(|| all.iter().map(|(_, _, p)| p.clone()).collect()).clone();
    let own = rollout_for(live)?;
    let (_, thread) = parse_name(&own.file_name()?.to_string_lossy())?;
    let mut files: Vec<(String, PathBuf)> =
        all.into_iter().filter(|(ts, _, p)| *ts >= not_before || !first_look.contains(p)).map(|(_, id, p)| (id, p)).collect();
    let listed: std::collections::HashSet<PathBuf> = files.iter().map(|(_, p)| p.clone()).collect();
    // Its subagents found before stay, listed now or not.
    let known: Vec<(String, PathBuf)> = seen
        .read
        .keys()
        .filter(|p| !listed.contains(*p))
        .filter_map(|p| Some((parse_name(&p.file_name()?.to_string_lossy())?.1, p.clone())))
        .collect();
    files.extend(known);
    let kept: std::collections::HashSet<&PathBuf> = files.iter().map(|(_, p)| p).collect();
    seen.parent_of.retain(|p, _| kept.contains(p));
    for (_, path) in &files {
        if !seen.parent_of.contains_key(path) {
            // Not kept while its first line does not read yet: a file just
            // begun may hold half of it.
            if let Some(meta) = first_line(path) {
                seen.parent_of.insert(path.clone(), spawned_by(&meta));
            }
        }
    }
    let kids = descendants(&files, thread, |path| seen.parent_of.get(path).cloned().flatten());
    let since = Since::dated(live.spawned_at_ms as i64);
    let mut at_work = Vec::new();
    let mut unseen = !whole;
    // Whether the session's own last turn, this process's, has ended: read
    // only if a subagent's rollout has gone, and once.
    let mut parent_ended = None;
    for (id, path) in kids {
        // A rollout found and gone since (archived, moved away: the file is
        // not there, which is not a read that failed) of a subagent whose
        // session's turn has ended is ended with it: Codex writes no end for a
        // thread it archives, and one it never reads again would hold the PC
        // for as long as the process lives. While the turn runs it stays not
        // seen, as the turn itself is at work.
        let gone = seen.read.contains_key(&path) && std::fs::metadata(&path).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound);
        if gone && *parent_ended.get_or_insert_with(|| turn_events(&own, live.spawned_at_ms as i64).1.is_some()) {
            seen.read.remove(&path);
            continue;
        }
        let (offset, scan) = seen.read.entry(path.clone()).or_default();
        if past_end(&path, *offset) {
            (*offset, *scan) = (0, Scan::default());
        }
        let through;
        (*offset, through) = scan_from(&path, *offset, since, scan);
        unseen |= !through;
        // A turn dated before the spawn may be this process's all the same.
        if scan.working == Some(true) || scan.doubt {
            at_work.push(id);
        }
    }
    if unseen {
        at_work.push(NOT_SEEN.to_string());
    }
    Some(at_work)
}

/// The threads of `files` (thread id, rollout) spawned under `thread`, at any
/// depth, by the thread each rollout's first line names (`parent_of`).
fn descendants(files: &[(String, PathBuf)], thread: String, parent_of: impl Fn(&PathBuf) -> Option<String>) -> Vec<(String, PathBuf)> {
    let mut family = std::collections::HashSet::from([thread]);
    let mut kids: Vec<(String, PathBuf)> = Vec::new();
    loop {
        let found =
            files.iter().filter(|(id, path)| !family.contains(id) && parent_of(path).is_some_and(|parent| family.contains(&parent)));
        let found: Vec<(String, PathBuf)> = found.cloned().collect();
        if found.is_empty() {
            break;
        }
        family.extend(found.iter().map(|(id, _)| id.clone()));
        kids.extend(found);
    }
    kids
}

/// How long before or after a thread's turn ends a subagent's turn aborted
/// "interrupted" is taken for `codex exec` cutting it off with that end.
const CUT_AT_END_MS: i64 = 5_000;

/// The subagents `codex exec` cut off as the turn of `thread` ended, by name
/// (`subagent_name`, "" for one with none). Codex 0.167.2 interrupts every
/// subagent still at work then (its rollout's last turn ends `turn_aborted`,
/// reason `interrupted`, the moment the parent's does) and exits 0, saying
/// nothing of it. They run in its process: one killed or crashed leaves the
/// turn of each still at work open, its own end written or not, and they
/// went with it. Subagents at any depth begun since `since_ms`, the turn's
/// start, as `children_at_work` finds them, whose turn began since then by
/// its own timestamp; one its model interrupted itself earlier in the turn
/// is none of them. Nothing for a thread with no turn begun since then.
pub fn subagents_cut_at_turn_end(account_path: &str, thread: &str, since_ms: u64) -> Vec<String> {
    let since = since_ms as i64;
    let Some(own) = rollout_of_thread(account_path, thread) else { return Vec::new() };
    let (open, end) = turn_events(&own, since);
    let ended = end.map(|(_, at)| at);
    if !open && ended.is_none() {
        return Vec::new();
    }
    let Some(started) = local_at(since) else { return Vec::new() };
    let not_before = started - 5.seconds();
    let today = local_today();
    let days = today.since(not_before.date()).map_or(0, |s| s.get_days()).clamp(0, 13) as usize + 1;
    let files: Vec<(String, PathBuf)> = rollouts_on(account_path, days_back(today, days))
        .into_iter()
        .filter(|(ts, _, _)| *ts >= not_before)
        .map(|(_, id, p)| (id, p))
        .collect();
    // Each header read once, for whose it is and what it is called.
    let heads: HashMap<&PathBuf, Value> = files.iter().filter_map(|(_, p)| Some((p, first_line(p)?))).collect();
    let parent_of = |path: &PathBuf| heads.get(path).and_then(spawned_by);
    let kids = descendants(&files, thread.to_string(), parent_of);
    let cut = kids.iter().filter(|(_, path)| {
        let (open, end) = turn_events(path, since);
        open || matches!((ended, end), (Some(ended), Some((true, at))) if (at - ended).abs() <= CUT_AT_END_MS)
    });
    // One a subagent spawned went with it: counted as that top-level
    // subagent, once, as Claude Code's are.
    let path_of: HashMap<&str, &PathBuf> = kids.iter().map(|(id, path)| (id.as_str(), path)).collect();
    let mut tops: Vec<&PathBuf> = Vec::new();
    for (_, path) in cut {
        let mut top = path;
        while let Some(up) = parent_of(top).filter(|p| p != thread).and_then(|p| path_of.get(p.as_str()).copied()) {
            top = up;
        }
        if !tops.contains(&top) {
            tops.push(top);
        }
    }
    tops.into_iter().map(|path| heads.get(path).map(subagent_name).unwrap_or_default()).collect()
}

/// What a subagent's rollout calls it: its `session_meta`'s `agent_nickname`,
/// else its `agent_role` (both beside `parent_thread_id` in 0.169.0's
/// binary, not seen in a rollout yet); "" for neither.
fn subagent_name(meta: &Value) -> String {
    let p = &meta["payload"];
    p["agent_nickname"].as_str().or(p["agent_role"].as_str()).map(str::trim).unwrap_or_default().to_string()
}

/// A rollout's last turn, if it began since `since_ms` (by a timestamp that
/// reads): whether it has no end; and how it ended, whether aborted as
/// interrupted and when (None while it is open, or for a turn begun before).
fn turn_events(path: &Path, since_ms: i64) -> (bool, Option<(bool, i64)>) {
    let (mut open, mut end, mut ours) = (false, None, false);
    crate::pty::complete_lines(path, 0, |line| {
        if !line.contains("\"event_msg\"") {
            return;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { return };
        let at = v["timestamp"].as_str().and_then(|t| t.parse::<jiff::Timestamp>().ok()).map(|t| t.as_millisecond());
        let payload = &v["payload"];
        match payload["type"].as_str() {
            Some("task_started" | "turn_started") => {
                end = None;
                ours = at.is_some_and(|at| at >= since_ms);
                open = ours;
            }
            Some(kind @ ("task_complete" | "turn_complete" | "turn_aborted" | "task_aborted")) => {
                end = at
                    .filter(|at| ours && *at >= since_ms)
                    .map(|at| (kind.ends_with("_aborted") && payload["reason"] == "interrupted", at));
                open = false;
            }
            _ => {}
        }
    });
    (open, end)
}

/// Reads the complete lines from byte `from` on into `scan` and returns the
/// offset just past the last of them. Turn ends count only when written at or
/// after `since_ms`; a line whose timestamp does not read is taken as new.
#[cfg(test)]
pub(super) fn scan_rollout(path: &Path, from: u64, since_ms: i64, scan: &mut Scan) -> u64 {
    scan_from(path, from, Since::dated(since_ms), scan).0
}

/// `scan_rollout`, lines told old from new by `since`, and whether the file
/// read through to its end: one that would not open, seek or read did not,
/// and says nothing of what came since (what it read before it broke off is in
/// the scan, and the offset past it).
pub(super) fn scan_from(path: &Path, from: u64, since: Since, scan: &mut Scan) -> (u64, bool) {
    let mut read = from;
    match crate::pty::lines_from(path, from, |end, line| {
        read = end;
        scan.line_at(line, since, end);
    }) {
        Ok(end) => (end, true),
        Err(_) => (read, false),
    }
}

impl Scan {
    /// One complete line of the rollout read into the scan, by its date alone.
    pub(super) fn line(&mut self, line: &str, since_ms: i64) {
        self.line_at(line, Since::dated(since_ms), 0);
    }

    /// One complete line of the rollout, ending at byte `ends_at`, read into the
    /// scan, as `scan_from` reads each. Where the file ended at the spawn is
    /// known, that says whether the line is this process's; else its date
    /// does, and a turn begun dated before the spawn is in doubt: an earlier
    /// process's, or this one's under a wall clock set back.
    pub(super) fn line_at(&mut self, line: &str, since: Since, ends_at: u64) {
        let event = line.contains("\"event_msg\"");
        if !event && !line.contains("request_user_input") && !line.contains("\"function_call_output\"") {
            return;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { return };
        let payload = &v["payload"];
        let at_ms = v["timestamp"].as_str().and_then(|t| t.parse::<jiff::Timestamp>().ok()).map(|t| t.as_millisecond());
        let (new, sure) = match since.offset {
            Some(at_spawn) => (ends_at > at_spawn, true),
            None => {
                let new = at_ms.is_none_or(|t| t >= since.ms);
                (new, new)
            }
        };
        match (v["type"].as_str(), payload["type"].as_str()) {
            (Some("event_msg"), Some("task_started" | "turn_started")) => {
                // A turn an earlier process was on went with it, and Codex (0.166)
                // writes no end for it as it resumes: it sits at its composer.
                // Not known to be one, it is not known to be over either.
                self.working = sure.then_some(new);
                self.doubt = !sure;
                if new && self.first_started_ms.is_none() {
                    self.first_started_ms = at_ms;
                }
                self.failed = None;
                self.last_said = None;
                self.last_said_at = None;
            }
            (Some("event_msg"), Some(end @ ("task_complete" | "turn_complete" | "turn_aborted" | "task_aborted"))) => {
                self.working = Some(false);
                self.doubt = false;
                self.last_said = payload["last_agent_message"].as_str().map(str::to_owned);
                self.last_said_at = v["timestamp"].as_str().map(str::to_owned);
                self.failed = payload["error"]["message"].as_str().map(str::to_owned).or_else(|| {
                    end.ends_with("_aborted").then(|| match payload["reason"].as_str() {
                        Some(why) => format!("its turn was aborted ({why})"),
                        None => "its turn was aborted".to_string(),
                    })
                });
                // A blocking question goes with the turn that asked it.
                if self.ask.as_ref().is_some_and(|(_, a)| a.blocking) {
                    self.ask = None;
                }
                if new {
                    self.completed += 1;
                }
            }
            // Whatever the user says next is the answer, typed or picked:
            // `user_message` before 0.163, a completed `UserMessage` item since.
            (Some("event_msg"), Some("user_message")) => self.ask = None,
            (Some("event_msg"), Some("item_completed")) if payload["item"]["type"] == "UserMessage" => self.ask = None,
            (Some("response_item"), Some("function_call")) => {
                let blocking = match payload["name"].as_str() {
                    Some("request_user_input") => true,
                    Some("request_user_input_async") => false,
                    _ => return,
                };
                let args = payload["arguments"].as_str().and_then(|a| serde_json::from_str::<Value>(a).ok());
                if let Some(ask) = args.as_ref().and_then(|a| ask_of(a, blocking)) {
                    self.ask = Some((payload["call_id"].as_str().unwrap_or("").to_string(), ask));
                }
            }
            (Some("response_item"), Some("function_call_output")) => {
                let answered = self.ask.as_ref().is_some_and(|(id, a)| a.blocking && payload["call_id"].as_str() == Some(id));
                if answered {
                    self.ask = None;
                }
            }
            _ => {}
        }
    }
}

/// `{"questions":[…]}` as Codex's question tools take it: each with its text
/// under `question` (`title` in the async one) and the suggested answers
/// under `options`.
fn ask_of(args: &Value, blocking: bool) -> Option<crate::agents::Ask> {
    let questions: Vec<crate::agents::AskQuestion> = args["questions"]
        .as_array()?
        .iter()
        .filter_map(|q| {
            let text = q["question"].as_str().or(q["title"].as_str())?.trim().to_string();
            let options = q["options"]
                .as_array()
                .map(|o| o.iter().filter_map(|o| o["label"].as_str().or(o.as_str()).map(str::to_owned)).collect())
                .unwrap_or_default();
            (!text.is_empty()).then_some(crate::agents::AskQuestion { text, options, multi: false, list_at: None })
        })
        .collect();
    (!questions.is_empty()).then(|| crate::agents::Ask::question(questions, blocking))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A session's subagents, at any depth, are at work while the last turn
    /// their own rollout opened has not ended. One done with its task sleeps
    /// until it is written to again, and another session's are none of its.
    #[test]
    fn a_sessions_subagents_are_at_work_while_their_own_turn_is_open() {
        let (root, t0) = rollout_bench("children");
        let acct = root.to_string_lossy().into_owned();
        let chat = format!("chat-children-{t0}");
        let (own, kid, grandkid, stranger) = (format!("own{t0}"), format!("kid{t0}"), format!("grand{t0}"), format!("stranger{t0}"));
        write_rollout(&root, t0 + 1_000, &own, Some(r#""cli""#));
        let live = Live { chat_id: &chat, cwd: "E:\\p", spawned_at_ms: t0 as u128, resume: None, account_path: &acct };
        let at_work = || {
            let mut kids = children_at_work(&live).unwrap();
            kids.sort();
            kids
        };
        assert!(at_work().is_empty());
        write_rollout(&root, t0 + 2_000, &kid, Some(&spawned_by_json(&own)));
        write_rollout(&root, t0 + 3_000, &grandkid, Some(&spawned_by_json(&kid)));
        write_rollout(&root, t0 + 3_000, &stranger, Some(&spawned_by_json("someone-else")));
        let (kid_file, grandkid_file) = (rollout_path(&root, &kid, t0 + 2_000), rollout_path(&root, &grandkid, t0 + 3_000));
        assert!(at_work().is_empty(), "begun, with no turn yet");
        append(kid_file.clone(), turn_event("task_started", t0 + 4_000));
        append(grandkid_file.clone(), turn_event("task_started", t0 + 5_000));
        append(rollout_path(&root, &stranger, t0 + 3_000), turn_event("task_started", t0 + 5_000));
        assert_eq!(at_work(), vec![grandkid.clone(), kid.clone()], "its own, at any depth");
        append(grandkid_file.clone(), turn_event("task_complete", t0 + 6_000));
        assert_eq!(at_work(), vec![kid.clone()], "done with its task: asleep");
        append(grandkid_file.clone(), turn_event("task_started", t0 + 7_000));
        assert_eq!(at_work(), vec![grandkid.clone(), kid.clone()], "woken by a message");
        append(kid_file, turn_event("turn_aborted", t0 + 8_000));
        append(grandkid_file, turn_event("task_complete", t0 + 8_000));
        assert!(at_work().is_empty());
        release(&chat);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Codex 0.167.2's `exec` interrupts a subagent its turn spawned as that
    /// turn ends, seen live: the subagent's turn aborted "interrupted" the
    /// moment its parent's completed. One done by then, or interrupted well
    /// before the end (its model's own doing), is not cut off; nor is any
    /// subagent of a turn before `since`.
    #[test]
    fn a_subagent_interrupted_as_its_parents_turn_ended_is_cut_off() {
        let (root, t0) = rollout_bench("cut-off");
        let acct = root.to_string_lossy().into_owned();
        let (cut, done) = ((format!("cutp{t0}"), format!("cutk{t0}")), (format!("donep{t0}"), format!("donek{t0}")));
        cut_off_pair_for_test(&root, t0, &cut.0, &cut.1, true);
        cut_off_pair_for_test(&root, t0, &done.0, &done.1, false);
        assert_eq!(subagents_cut_at_turn_end(&acct, &cut.0, t0), [""], "one, its rollout naming it nothing");
        assert!(subagents_cut_at_turn_end(&acct, &done.0, t0).is_empty(), "done before its parent's turn ended");
        assert!(subagents_cut_at_turn_end(&acct, &cut.0, t0 + 10_000).is_empty(), "of a turn before this one");
        // Interrupted by its parent's model mid-turn, a minute before the end.
        let (early, kid) = (format!("earlyp{t0}"), format!("earlyk{t0}"));
        cut_off_pair_for_test(&root, t0, &early, &kid, false);
        append(rollout_path(&root, &early, t0), turn_event("task_started", t0 + 5_000) + &turn_event("task_complete", t0 + 65_000));
        let at = jiff::Timestamp::from_millisecond((t0 + 5_000) as i64).unwrap().strftime("%Y-%m-%dT%H:%M:%S%.3fZ");
        let aborted = format!(r#"{{"timestamp":"{at}","type":"event_msg","payload":{{"type":"turn_aborted","reason":"interrupted"}}}}"#);
        append(rollout_path(&root, &kid, t0 + 1_000), turn_event("task_started", t0 + 4_500) + &aborted + "\n");
        assert!(subagents_cut_at_turn_end(&acct, &early, t0).is_empty(), "interrupted long before the end");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A subagent cut with its own: counted as that top-level subagent, once,
    /// as Claude Code's are; so too a subagent's own cut under one done. A
    /// subagent's turn interrupted at the end that began before this turn is
    /// none of this turn's.
    #[test]
    fn a_cut_grandchild_counts_as_its_child_and_an_earlier_turn_is_none() {
        let (root, t0) = rollout_bench("nested");
        let acct = root.to_string_lossy().into_owned();
        let (parent, kid, grandkid, done, under_done, old) =
            (format!("np{t0}"), format!("nk{t0}"), format!("ng{t0}"), format!("nd{t0}"), format!("nu{t0}"), format!("no{t0}"));
        let aborted = |ms: u64| {
            let at = jiff::Timestamp::from_millisecond(ms as i64).unwrap().strftime("%Y-%m-%dT%H:%M:%S%.3fZ");
            format!(
                "{{\"timestamp\":\"{at}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"turn_aborted\",\"reason\":\"interrupted\"}}}}\n"
            )
        };
        let named = |uuid: &str, at: u64, by: &str, nick: &str, turns: String| {
            let head = format!(
                r#"{{"type":"session_meta","payload":{{"cwd":"E:\\p","source":{},"agent_nickname":"{nick}"}}}}"#,
                spawned_by_json(by)
            );
            write_rollout(&root, at, uuid, Some(&spawned_by_json(by)));
            std::fs::write(rollout_path(&root, uuid, at), head + "\n" + &turns).unwrap();
        };
        write_rollout(&root, t0, &parent, Some(r#""exec""#));
        append(rollout_path(&root, &parent, t0), turn_event("task_started", t0) + &turn_event("task_complete", t0 + 10_000));
        // Its child at work, and the child's own, cut with it: the child, once.
        named(&kid, t0 + 1_000, &parent, "Euclid", turn_event("task_started", t0 + 2_000));
        named(&grandkid, t0 + 3_000, &kid, "Hypatia", turn_event("task_started", t0 + 4_000) + &aborted(t0 + 10_000));
        // A child done, whose own was cut: that child, by its name.
        named(&done, t0 + 1_000, &parent, "Noether", turn_event("task_started", t0 + 2_000) + &turn_event("task_complete", t0 + 5_000));
        named(&under_done, t0 + 3_000, &done, "Gauss", turn_event("task_started", t0 + 4_000));
        // Of an earlier run, its header within the five seconds the file
        // names allow: interrupted at the end, its turn begun before this one.
        named(&old, t0 - 3_000, &parent, "Archimedes", turn_event("task_started", t0 - 2_000) + &aborted(t0 + 10_000));
        let mut cut = subagents_cut_at_turn_end(&acct, &parent, t0);
        cut.sort();
        assert_eq!(cut, ["Euclid", "Noether"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// An `exec` gone before its own turn ended (killed, crashed) writes no
    /// end for it, nor for the subagents that ran in its process: those whose
    /// turn it left open went with it, by the name their rollout gives them.
    /// One done by then, or with no turn begun in this one, did not.
    #[test]
    fn subagents_left_open_by_an_exec_gone_mid_turn_went_with_it() {
        let (root, t0) = rollout_bench("killed");
        let acct = root.to_string_lossy().into_owned();
        let (parent, named, done, idle) = (format!("kp{t0}"), format!("kn{t0}"), format!("kd{t0}"), format!("ki{t0}"));
        write_rollout(&root, t0, &parent, Some(r#""exec""#));
        append(rollout_path(&root, &parent, t0), turn_event("task_started", t0));
        let source = spawned_by_json(&parent);
        let meta = |nick: &str| {
            format!(
                r#"{{"type":"session_meta","payload":{{"cwd":"E:\\p","source":{source},"agent_nickname":"{nick}","agent_role":"worker"}}}}"#
            )
        };
        write_rollout(&root, t0 + 1_000, &named, Some(&source));
        std::fs::write(rollout_path(&root, &named, t0 + 1_000), meta("Ramanujan") + "\n" + &turn_event("task_started", t0 + 2_000))
            .unwrap();
        write_rollout(&root, t0 + 1_000, &done, Some(&source));
        append(rollout_path(&root, &done, t0 + 1_000), turn_event("task_started", t0 + 2_000) + &turn_event("task_complete", t0 + 3_000));
        write_rollout(&root, t0 + 1_000, &idle, Some(&source));
        // Of an earlier run, its header within the five seconds the file
        // names allow: a turn begun before this one's, or at a moment that
        // does not read, is none of this one's.
        let (before, unread) = (format!("kb{t0}"), format!("ku{t0}"));
        write_rollout(&root, t0 - 3_000, &before, Some(&source));
        append(rollout_path(&root, &before, t0 - 3_000), turn_event("task_started", t0 - 2_000));
        write_rollout(&root, t0 - 3_000, &unread, Some(&source));
        append(
            rollout_path(&root, &unread, t0 - 3_000),
            r#"{"timestamp":"soon","type":"event_msg","payload":{"type":"task_started"}}"#.to_string() + "\n",
        );
        assert_eq!(subagents_cut_at_turn_end(&acct, &parent, t0), ["Ramanujan"]);
        // Killed after its own end was written and before the subagent's
        // abort: still open, it went with the process all the same.
        append(rollout_path(&root, &parent, t0), turn_event("task_complete", t0 + 20_000));
        assert_eq!(subagents_cut_at_turn_end(&acct, &parent, t0), ["Ramanujan"]);
        // A thread with no turn begun since then cut nothing.
        assert!(subagents_cut_at_turn_end(&acct, &parent, t0 + 30_000).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A session days old: a subagent begun on its first day, in that day's
    /// folder, is still its own when woken now.
    #[test]
    fn a_subagent_from_an_earlier_day_of_the_session_is_found() {
        let (root, now) = rollout_bench("old-kid");
        let acct = root.to_string_lossy().into_owned();
        let t0 = now - 3 * 86_400_000;
        let chat = format!("chat-oldkid-{t0}");
        let (own, kid) = (format!("ownold{t0}"), format!("kidold{t0}"));
        write_rollout(&root, t0 + 1_000, &own, Some(r#""cli""#));
        write_rollout(&root, t0 + 2_000, &kid, Some(&spawned_by_json(&own)));
        append(rollout_path(&root, &kid, t0 + 2_000), turn_event("task_started", now));
        let live = Live { chat_id: &chat, cwd: "E:\\p", spawned_at_ms: t0 as u128, resume: None, account_path: &acct };
        // On its own thread since its first day, as its TUI said then.
        assert!(rollout_on(&live, Some(own.clone())).is_some());
        assert_eq!(children_at_work(&live), Some(vec![kid.clone()]));
        release(&chat);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Killed mid-turn with its core and resumed: Codex 0.166 sits at its composer
    /// and writes no end for that turn, which must not read as one on now. Where
    /// the file ended at the spawn is known, that turn is over for sure; by its
    /// date alone it is in doubt, and not known.
    #[test]
    fn a_turn_an_earlier_process_was_on_is_not_running() {
        let dir = std::env::temp_dir().join(format!("luna-rollout-resumed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("r.jsonl");
        let line =
            |at: &str, kind: &str| format!("{{\"timestamp\":\"{at}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"{kind}\"}}}}\n");
        let old = line("2026-11-01T08:00:00.000Z", "task_started");
        std::fs::write(&path, &old).unwrap();
        let ms = "2026-11-01T09:00:00.000Z".parse::<jiff::Timestamp>().unwrap().as_millisecond();
        let placed = Since { ms, offset: Some(old.len() as u64) };
        let mut scan = Scan::default();
        let (at, through) = scan_from(&path, 0, placed, &mut scan);
        assert!(through);
        assert_eq!((scan.working, scan.doubt), (Some(false), false));
        let mut dated = Scan::default();
        scan_from(&path, 0, Since::dated(ms), &mut dated);
        assert_eq!((dated.working, dated.doubt), (None, true), "by its date alone: not known");
        // The first turn this process runs reads as one.
        let more = line("2026-11-01T09:00:05.000Z", "task_started");
        std::fs::write(&path, format!("{old}{more}")).unwrap();
        scan_from(&path, at, placed, &mut scan);
        assert_eq!(scan.working, Some(true));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The wall clock set back an hour after the spawn: a turn begun now is
    /// dated before the spawn. Past where the file ended then, it is this
    /// process's and on; with nothing to place it, not known; never over.
    #[test]
    fn a_clock_set_back_never_ends_a_turn() {
        let dir = std::env::temp_dir().join(format!("luna-rollout-setback-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("r.jsonl");
        let line =
            |at: &str, kind: &str| format!("{{\"timestamp\":\"{at}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"{kind}\"}}}}\n");
        let before = line("2026-11-01T08:00:00.000Z", "task_complete");
        let spawned = "2026-11-01T09:00:00.000Z".parse::<jiff::Timestamp>().unwrap().as_millisecond();
        std::fs::write(&path, format!("{before}{}", line("2026-11-01T08:30:00.000Z", "task_started"))).unwrap();
        let mut placed = Scan::default();
        scan_from(&path, 0, Since { ms: spawned, offset: Some(before.len() as u64) }, &mut placed);
        assert_eq!(placed.working, Some(true), "past the spawn's end of file: this process's turn");
        let mut dated = Scan::default();
        scan_from(&path, 0, Since::dated(spawned), &mut dated);
        assert_eq!((dated.working, dated.doubt), (None, true), "dated alone: not known, so active");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A subagent begun under a wall clock set back is named and dated before
    /// the spawn: not there at the first look, it is the session's all the
    /// same. One found stays found when its rollout drops out of the listing,
    /// and one that will not read is not known, which is at work.
    #[test]
    fn subagents_are_kept_through_a_clock_set_back_and_a_listing_that_misses_them() {
        let (root, t0) = rollout_bench("kept-kids");
        let acct = root.to_string_lossy().into_owned();
        let chat = format!("chat-kept-kids-{t0}");
        let (own, kid) = (format!("own{t0}"), format!("kid{t0}"));
        let live = Live { chat_id: &chat, cwd: "E:\\p", spawned_at_ms: (t0 + 60_000) as u128, resume: None, account_path: &acct };
        write_rollout(&root, t0 + 61_000, &own, Some(r#""cli""#));
        assert_eq!(children_at_work(&live), Some(vec![]), "the first look");
        // Set back: the subagent's name and its turn are dated before the spawn.
        write_rollout(&root, t0 + 1_000, &kid, Some(&spawned_by_json(&own)));
        let kid_file = rollout_path(&root, &kid, t0 + 1_000);
        append(kid_file.clone(), turn_event("task_started", t0 + 2_000));
        assert_eq!(children_at_work(&live), Some(vec![kid.clone()]));
        // Its rollout gone from where the listing looks: still the session's,
        // and what it does now is not known.
        let away = root.join("away.jsonl");
        std::fs::rename(&kid_file, &away).unwrap();
        assert_eq!(children_at_work(&live), Some(vec![kid.clone(), NOT_SEEN.to_string()]));
        // The session's own turn ending ends it with it: Codex writes no end
        // for a thread it archives, and a rollout that is not there (not one
        // that would not read) is not read again.
        append(rollout_path(&root, &own, t0 + 61_000), turn_event("task_started", t0 + 65_000) + &turn_event("task_complete", t0 + 70_000));
        assert_eq!(children_at_work(&live), Some(vec![]), "gone, and its session's turn over: ended");
        assert_eq!(children_at_work(&live), Some(vec![]), "and stays so");
        release(&chat);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A rollout that will not read through says so, and its last scan stands
    /// for nothing: the record reads not known, never the ended it said.
    #[test]
    fn a_rollout_that_will_not_read_is_not_ended() {
        let dir = std::env::temp_dir().join(format!("luna-rollout-gone-{}", std::process::id()));
        let mut scan = Scan { working: Some(false), completed: 1, ..Default::default() };
        let (at, through) = scan_from(&dir.join("not-there.jsonl"), 40, Since::dated(0), &mut scan);
        assert_eq!((at, through), (40, false));
    }

    /// The 0.157 rollout: `task_started` / `task_complete` for the turn, and
    /// the conversation as `response_item` messages with roles.
    #[test]
    fn reads_the_current_rollout_format() {
        let root = std::env::temp_dir().join(format!("luna-codex-fmt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("rollout-2026-09-26T15-43-59-abc.jsonl");
        let lines = [
            r#"{"timestamp":"t","type":"session_meta","payload":{"id":"abc","cwd":"E:\\p"}}"#,
            r#"{"timestamp":"t","type":"event_msg","payload":{"type":"task_started","turn_id":"t1"}}"#,
            r#"{"timestamp":"t","type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"<skills_instructions>…"}]}}"#,
            r##"{"timestamp":"t","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"# AGENTS.md instructions for E:\\p\n\nrules"}]}}"##,
            r#"{"timestamp":"t","type":"turn_context","payload":{"turn_id":"t1","cwd":"E:\\p","model":"gpt-6-astra"}}"#,
            r#"{"timestamp":"t","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Reply with the single word READY."}]}}"#,
            r#"{"timestamp":"t","type":"event_msg","payload":{"type":"item_completed","item":{"type":"UserMessage","text":"Reply with the single word READY."}}}"#,
            r#"{"timestamp":"t","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"READY"}]}}"#,
            r#"{"timestamp":"t","type":"event_msg","payload":{"type":"task_complete","turn_id":"t1","last_agent_message":"READY"}}"#,
            // A helper's next turn, with the note Luna puts on each.
            &format!(
                r#"{{"timestamp":"t","type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":{}}}]}}}}"#,
                serde_json::to_string(&crate::pty::helper_prompt(
                    &crate::library::role_texts(crate::library::Role::Helper).note.unwrap(),
                    "And now?"
                ))
                .unwrap()
            ),
        ];
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();

        let t = read_tail(&path);
        assert_eq!(t.model.as_deref(), Some("gpt-6-astra"));
        assert_eq!(read_first_prompt(&path).as_deref(), Some("Reply with the single word READY."));

        let (msgs, cursor) = messages_from(&path, 0);
        let got: Vec<(&str, &str)> = msgs.iter().map(|m| (m.role.as_str(), m.text.as_str())).collect();
        assert_eq!(
            got,
            vec![("user", "Reply with the single word READY."), ("assistant", "READY"), ("user", "And now?")],
            "as asked, without Luna's note"
        );
        assert_eq!(cursor, std::fs::metadata(&path).unwrap().len());
        let (more, _) = messages_from(&path, cursor);
        assert!(more.is_empty(), "nothing new after the cursor");
        // A reply still being written is not read, nor stepped over.
        let reply = r#"{"timestamp":"t","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"DONE"}]}}"#;
        let whole = lines.join("\n") + "\n";
        std::fs::write(&path, format!("{whole}{}", &reply[..40])).unwrap();
        assert_eq!(messages_from(&path, cursor), (vec![], cursor));
        std::fs::write(&path, format!("{whole}{reply}\n")).unwrap();
        let (more, _) = messages_from(&path, cursor);
        assert_eq!(
            more,
            vec![crate::agents::Message { role: "assistant".into(), text: "DONE".into(), after_cli: false, final_answer: false }]
        );
        std::fs::write(&path, &whole).unwrap();

        // Turn ends are counted off complete lines only, and incrementally.
        let mut s = Scan::default();
        let consumed = scan_rollout(&path, 0, 0, &mut s);
        assert_eq!((s.completed, s.working), (1, Some(false)));
        assert_eq!(consumed, std::fs::metadata(&path).unwrap().len());
        let mut grown = std::fs::read_to_string(&path).unwrap();
        grown.push_str(r#"{"timestamp":"t","type":"event_msg","payload":{"type":"task_complete","turn_id":"t2"}}"#);
        std::fs::write(&path, &grown).unwrap();
        assert_eq!(scan_rollout(&path, consumed, 0, &mut s), consumed, "a line without its newline is not there yet");
        assert_eq!(s.completed, 1);
        grown.push('\n');
        std::fs::write(&path, &grown).unwrap();
        scan_rollout(&path, consumed, 0, &mut s);
        assert_eq!(s.completed, 2);

        // A resumed thread: the turns of an earlier run are not this one's.
        let resumed = [
            r#"{"timestamp":"2026-09-26T15:44:00.000Z","type":"event_msg","payload":{"type":"task_complete","turn_id":"t1"}}"#,
            r#"{"timestamp":"2026-09-26T16:10:00.500Z","type":"event_msg","payload":{"type":"task_complete","turn_id":"t2"}}"#,
        ];
        std::fs::write(&path, resumed.join("\n") + "\n").unwrap();
        let spawned = "2026-09-26T16:00:00Z".parse::<jiff::Timestamp>().unwrap().as_millisecond();
        let count = |since: i64| {
            let mut s = Scan::default();
            scan_rollout(&path, 0, since, &mut s);
            s.completed
        };
        assert_eq!(count(spawned), 1);
        assert_eq!(count(0), 2);

        // Mid-turn, however much the turn has written since it started: a
        // megabyte of tool output still reads as working.
        let filler = format!(
            r#"{{"timestamp":"t","type":"response_item","payload":{{"type":"function_call_output","call_id":"c","output":"{}"}}}}"#,
            "x".repeat(4096)
        );
        let long: Vec<String> = lines[..2].iter().map(|l| l.to_string()).chain(std::iter::repeat_n(filler, 300)).collect();
        std::fs::write(&path, long.join("\n") + "\n").unwrap();
        let mut s = Scan::default();
        scan_rollout(&path, 0, 0, &mut s);
        assert_eq!(s.working, Some(true));

        let _ = std::fs::remove_dir_all(&root);
    }

    /// How a turn that failed reads in a 0.166 rollout (a bad key, seen on a
    /// throwaway account): a `task_complete` like any other, with the error.
    #[test]
    fn a_turn_that_failed_says_why() {
        let root = std::env::temp_dir().join(format!("luna-codex-failed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("rollout-2026-11-01T10-52-43-abc.jsonl");
        let failed_after = |lines: &[&str]| {
            std::fs::write(&path, lines.join("\n") + "\n").unwrap();
            let mut s = Scan::default();
            scan_rollout(&path, 0, 0, &mut s);
            (s.failed, s.completed)
        };
        let started = r#"{"timestamp":"t","type":"event_msg","payload":{"type":"task_started","turn_id":"t1"}}"#;
        let failed = r#"{"timestamp":"t","ordinal":14,"type":"event_msg","payload":{"type":"task_complete","turn_id":"t1","last_agent_message":null,"error":{"message":"unexpected status 401 Unauthorized"}}}"#;
        let fine = r#"{"timestamp":"t","type":"event_msg","payload":{"type":"task_complete","turn_id":"t2","last_agent_message":"READY"}}"#;
        let aborted = r#"{"timestamp":"t","type":"event_msg","payload":{"type":"turn_aborted","turn_id":"t3","reason":"interrupted"}}"#;
        assert_eq!(failed_after(&[started, failed]), (Some("unexpected status 401 Unauthorized".into()), 1), "it ends a turn all the same");
        assert_eq!(failed_after(&[started, failed, started]), (None, 1), "the next turn is another one");
        assert_eq!(failed_after(&[started, failed, started, fine]), (None, 2));
        assert_eq!(failed_after(&[started, aborted]).0.as_deref(), Some("its turn was aborted (interrupted)"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Codex 0.163 asks through `request_user_input_async` (the turn goes on)
    /// and `request_user_input` (the turn waits for the answer).
    #[test]
    fn a_question_is_open_until_it_is_answered() {
        let root = std::env::temp_dir().join(format!("luna-codex-ask-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("rollout-2026-10-22T04-51-02-abc.jsonl");
        let write = |lines: &[&str]| std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        let ask = |lines: &[&str]| {
            write(lines);
            let mut s = Scan::default();
            scan_rollout(&path, 0, 0, &mut s);
            s.ask.map(|(_, a)| a)
        };
        let started = r#"{"timestamp":"t","type":"event_msg","payload":{"type":"task_started"}}"#;
        let async_q = r#"{"timestamp":"t","type":"response_item","payload":{"type":"function_call","name":"request_user_input_async","arguments":"{\"questions\":[{\"title\":\"Can you paste CLAUDE.md here?\"}]}","call_id":"call_a"}}"#;
        let accepted = r#"{"timestamp":"t","type":"response_item","payload":{"type":"function_call_output","call_id":"call_a","output":"{\"accepted\":true}"}}"#;
        let done = r#"{"timestamp":"t","type":"event_msg","payload":{"type":"task_complete"}}"#;
        let user = r#"{"timestamp":"t","type":"event_msg","payload":{"type":"user_message","message":"here it is"}}"#;

        let a = ask(&[started, async_q, accepted, done]).expect("an async question outlives its turn");
        assert!(!a.blocking);
        assert_eq!(a.questions[0].text, "Can you paste CLAUDE.md here?");
        assert!(ask(&[started, async_q, accepted, done, user]).is_none(), "the user's next word answers it");
        let item = r#"{"timestamp":"t","type":"event_msg","payload":{"type":"item_completed","item":{"type":"UserMessage","id":"u"}}}"#;
        assert!(ask(&[started, async_q, accepted, item]).is_none(), "as 0.163 writes it");
        let plain = r#"{"timestamp":"t","type":"response_item","payload":{"type":"function_call","name":"request_user_input_async","arguments":"{\"questions\":[{\"title\":\"Size?\",\"options\":[\"Small\",\"Large\"]}]}","call_id":"c"}}"#;
        assert_eq!(ask(&[started, plain]).unwrap().questions[0].options, ["Small", "Large"], "options as bare strings");

        let blocking = r#"{"timestamp":"t","type":"response_item","payload":{"type":"function_call","name":"request_user_input","arguments":"{\"questions\":[{\"id\":\"color\",\"header\":\"Color\",\"question\":\"Which color?\",\"options\":[{\"label\":\"Red (Recommended)\",\"description\":\"\"},{\"label\":\"Blue\",\"description\":\"\"}]}]}","call_id":"call_b"}}"#;
        let answered = r#"{"timestamp":"t","type":"response_item","payload":{"type":"function_call_output","call_id":"call_b","output":"{\"answers\":{}}"}}"#;
        let a = ask(&[started, blocking]).expect("a blocking question");
        assert!(a.blocking);
        assert_eq!(a.questions[0].options, ["Red (Recommended)", "Blue"]);
        assert!(ask(&[started, blocking, answered]).is_none(), "its output is the answer");
        assert!(ask(&[started, blocking, done]).is_none(), "it goes with the turn");

        let _ = std::fs::remove_dir_all(&root);
    }
}
