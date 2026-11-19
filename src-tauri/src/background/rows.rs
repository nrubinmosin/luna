//! Helpers as the window and the power rules see them: rows, signs, turn states,
//! the read-only view of one's conversation, and handing one over to the person
//! as a chat of their own.

use super::*;

/// A helper as the window lists it under its parent.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub id: String,
    pub parent: Option<String>,
    pub name: String,
    pub provider: Provider,
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ask: Option<Ask>,
    /// The account folder's name.
    pub account: String,
    pub model: Option<String>,
    pub effort: String,
    pub started_ms: u64,
    /// While a turn runs, when it started.
    pub turn_started_ms: Option<u64>,
    /// Between turns, what its turns left running ("tail.exe (pid 22104)"):
    /// what its Stop ends then.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub left_running: Vec<String>,
    /// Being opened as a chat (`background_promote`).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub promoting: bool,
    /// When it last did anything (`last_activity_ms`).
    pub last_activity_ms: Option<u64>,
    /// The first line of what it was spawned on (agents/ `task_of`).
    pub task: Option<String>,
    /// The chat at the top of its line, `parent` managing it ("managed by
    /// <parent> · chat <top>"); None with no chat above it.
    pub top: Option<String>,
    /// The person's chats waiting on it now that are not above it ("watched by").
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub watched_by: Vec<String>,
    /// The person's last move of it to another chat ("moved from <from>, <when>").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub moved: Option<crate::agents::Moved>,
    /// The person's chats refused a write into it, the latest last: "Move to
    /// <chat>?" (`background_move`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub suggested: Vec<crate::agents::Suggested>,
}

/// Every helper, for the window.
#[tauri::command]
pub fn background_helpers() -> Vec<Row> {
    type Seen = (String, Spec, u64, Option<u64>, Vec<String>, bool, Option<u64>);
    let ids: Vec<Seen> = lock()
        .entries
        .iter()
        .map(|(id, e)| {
            let left = if e.run.is_none() && e.left_running { e.left.iter().map(Leftover::label).collect() } else { Vec::new() };
            let turn = e.run.as_ref().map(|r| r.started_ms);
            (id.clone(), e.spec.clone(), e.started_ms, turn, left, e.promoting, activity_of(e))
        })
        .collect();
    ids.into_iter()
        .map(|(id, spec, started_ms, turn_started_ms, left_running, promoting, last_activity_ms)| {
            let owner = crate::agents::ownership_of(&id);
            Row {
                parent: crate::agents::parent_of(&id),
                name: crate::agents::chat_name(&id).unwrap_or_default(),
                state: crate::agents::state_of(&id).0,
                ask: ask(&id),
                provider: spec.provider,
                account: std::path::Path::new(&spec.account_path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                model: spec.model,
                effort: spec.effort,
                started_ms,
                turn_started_ms,
                left_running,
                promoting,
                last_activity_ms,
                task: crate::agents::task_of(&id),
                top: owner.top,
                watched_by: owner.watched_by,
                moved: owner.moved,
                suggested: owner.suggested,
                id,
            }
        })
        .collect()
}

/// Tells the window the list has changed.
pub fn changed() {
    crate::emit::to_ui("agent://background", background_helpers());
}

/// When a helper last did anything (its last event, a turn's start or end),
/// off what Luna has seen of it alone: a listing's cheap look, with no
/// transcript read (`signs` reads one).
pub fn last_activity_ms(id: &str) -> Option<u64> {
    activity_of(lock().entries.get(id)?)
}

fn activity_of(e: &Entry) -> Option<u64> {
    let o = &e.observed;
    let started = e.run.as_ref().map_or(0, |run| run.started_ms);
    [o.event_ms, o.turn_ended_ms, started].into_iter().max().filter(|ms| *ms > 0)
}

/// What `luna_session_get` says of a helper beyond its state.
pub fn signs(id: &str) -> Option<Signs> {
    let (signs, provider) = {
        let r = lock();
        let e = r.entries.get(id)?;
        let o = &e.observed;
        let turn_started = e.run.as_ref().map(|run| run.started_ms);
        let last = [o.event_ms, o.turn_ended_ms, turn_started.unwrap_or(0)].into_iter().max().filter(|ms| *ms > 0);
        let signs = Signs {
            turn_started_at: turn_started.and_then(iso),
            last_activity_at: last.and_then(iso),
            // A Codex rollout that would not read: what it was asked to run on,
            // once a turn has got through on it.
            resolved_model: o.model.clone().or_else(|| e.spec.model.clone().filter(|_| e.spec.provider == Provider::Codex && o.answered)),
            context: None,
            left_running: left_shown(e),
        };
        (signs, e.spec.provider)
    };
    // Off the transcript, with the lock let go: every helper's events wait on it.
    let context = transcript(id).and_then(|path| Context::of(&path, provider));
    Some(Signs { context, ..signs })
}

/// While a turn runs, how long its CLI has said nothing: no event on its
/// stdout or stderr since this long, or since the turn began. A tool call that
/// hangs shows as this and nothing else.
pub fn turn_quiet_ms(id: &str, now: u64) -> Option<u64> {
    let r = lock();
    let e = r.entries.get(id)?;
    let run = e.run.as_ref()?;
    Some(now.saturating_sub(e.observed.event_ms.max(run.started_ms)))
}

/// Whether a helper's turn has answered and is up only for what it left in the
/// background (`Run::after_result`), without reading its job.
pub fn turn_answered(id: &str) -> bool {
    lock().entries.get(id).and_then(|e| e.run.as_ref()).is_some_and(|run| run.after_result)
}

/// What a helper's turn that has answered waits on, as its job shows it.
pub struct Waiting {
    /// The processes in its job besides the CLI, by name, each once.
    pub procs: Vec<String>,
}

/// Whether a helper's turn is only waiting on what it left in the background:
/// its CLI has written the turn's result and is still up (`Run::after_result`).
/// Its job as last read (`lives`).
pub fn waits_on_background(id: &str) -> Option<Waiting> {
    let r = lock();
    let run = r.entries.get(id)?.run.as_ref().filter(|run| run.after_result)?;
    let mut procs: Vec<String> = Vec::new();
    for l in run.watched.iter().filter(|l| Some(l.pid) != run.pid) {
        if !procs.contains(&l.exe) {
            procs.push(l.exe.clone());
        }
    }
    Some(Waiting { procs })
}

/// Reads the jobs of the turns that have answered and wait on their
/// background work (`Run::watched`), the CLI's own process with them.
fn watch_waiting() {
    let waiting: Vec<(String, std::sync::Arc<crate::procs::Job>)> = lock()
        .entries
        .iter()
        .filter(|(_, e)| e.run.as_ref().is_some_and(|run| run.after_result))
        .filter_map(|(id, e)| Some((id.clone(), e.job.clone()?)))
        .collect();
    for (id, job) in waiting {
        // Read with no lock held: it takes a snapshot of every process.
        let read = left_of(crate::procs::left_in(&job).unwrap_or_default());
        if let Some(run) = lock().entries.get_mut(&id).and_then(|e| e.run.as_mut()).filter(|run| run.after_result) {
            run.watched = read;
        }
    }
}

/// What the window needs to make a helper a chat of the person's own. No parent:
/// the chat is nobody's helper.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Promoted {
    pub id: String,
    pub provider: Provider,
    pub account_path: String,
    pub folder: String,
    pub worktree_path: Option<String>,
    pub session_id: Option<String>,
    pub name: Option<String>,
    pub settings: Value,
    pub tools: bool,
}

/// Starts handing a helper over to the person as a chat row of their own, from
/// which on the window runs it by resuming its conversation in a terminal like
/// any chat's: what the row needs. Not while a turn runs — the conversation would
/// be in two processes at once. The helper stays, with no turn starting and out of
/// its parent's reach to stop or delete, until the window says how the row came out
/// (`background_promoted`): a row it could not make leaves the helper as it was,
/// its parent's, rather than neither.
#[tauri::command]
pub fn background_promote(id: String) -> Result<Promoted, String> {
    let (spec, cwd, session) = {
        let mut r = lock();
        let e = r.entries.get_mut(&id).ok_or("no such helper")?;
        if e.run.is_some() {
            return Err("it is in the middle of a turn; open it once the turn is over".into());
        }
        // Its job goes with the entry, and what is still in it would run on
        // with nobody counting it for the machine or ending it.
        if e.left_running || e.ending_left {
            return Err("what its turns left is still running; stop it first, or open it once that is over".into());
        }
        // One opening at a time: a second would make a second row of the one id.
        if e.promoting {
            return Err("it is being opened as a chat already".into());
        }
        // A delete has it (agents/teardown.rs): it goes, not to the person.
        if e.deleting {
            return Err("it is being deleted".into());
        }
        // A move to another chat has it for a moment (agents/reparent.rs).
        if e.moving {
            return Err("it is being moved to another chat; open it once that is through".into());
        }
        e.promoting = true;
        e.promoting_since = crate::throttle::now_ms();
        (e.spec.clone(), e.cwd.clone(), e.session.clone())
    };
    // A core killed now holds it the same way in the next (persist.rs), until the
    // window settles the row it may have made.
    save();
    changed();
    let worktree_path = cwd.filter(|c| !crate::pty::same_dir(c, &spec.folder));
    Ok(Promoted {
        provider: spec.provider,
        account_path: spec.account_path.clone(),
        folder: crate::checkout::project_of(&spec.folder).unwrap_or_else(|| spec.folder.clone()),
        worktree_path: worktree_path.or_else(|| crate::checkout::project_of(&spec.folder).map(|_| spec.folder.clone())),
        session_id: session,
        name: crate::agents::chat_name(&id),
        settings: crate::agents::settings_of(&id),
        tools: spec.tools,
        id,
    })
}

/// The window's word on a helper `background_promote` began handing over: its
/// row made (`error` None), it is the person's for good (agents/handover.rs) and
/// its record goes, what was sent to it meanwhile and what never reached it told of
/// to whoever sent it; or the row could not be made, and it goes on as its parent's
/// helper, what was sent meanwhile its next turn. Also said by a window settling
/// what one gone before it was opening, whose row it finds or not. Whether it was
/// handed over: not for a row made for an opening that is over (PROMOTE_TIMEOUT_MS
/// gave it back to its parent), which the window lets go of. Said again (a retry
/// whose first answer was lost), it changes nothing: the handover happens once,
/// and what the person's chat has come to owe or be owed since stands.
#[tauri::command]
pub fn background_promoted(id: String, error: Option<String>) -> Result<bool, String> {
    let Some(why) = error else {
        {
            let mut r = lock();
            match r.entries.get_mut(&id) {
                Some(e) if !e.promoting => return Ok(false),
                // Being handed over on an earlier word.
                Some(e) if e.handing => return Ok(true),
                // Not to lapse while it is handed over.
                Some(e) => {
                    e.handing = true;
                    e.promoting_since = crate::throttle::now_ms();
                }
                // Handed over already, or deleted meanwhile: nothing more to do.
                None => return Ok(true),
            }
        }
        let (mut count, mut thread) = (None, None);
        let handed = crate::agents::take_over(&id, || {
            // Gone already (deleted meanwhile): the row has its conversation.
            let mut e = lock().entries.remove(&id)?;
            count = Some(e.turns_ended);
            thread = e.session.clone();
            let mut unsent = std::mem::take(&mut e.undelivered);
            unsent.extend(std::mem::take(&mut e.queue));
            Some(unsent)
        });
        // Nothing changed hands (a notice about it would not finish going in): it
        // is still being opened, and the window says it again.
        if let Err(e) = handed {
            if let Some(entry) = lock().entries.get_mut(&id) {
                entry.handing = false;
            }
            return Err(e);
        }
        // Its count goes on in the chat's track, so a count anyone was shown of it
        // stays one it has passed.
        if let Some(n) = count {
            crate::activity::count_at_least(&id, n);
        }
        // The conversation the pane resumes: a Codex one still carries what it
        // was started with, and is told so there (agents/handover.rs).
        crate::agents::started_in(&id, thread.as_deref());
        changed();
        // Or a core killed now would take it up beside the chat it has become.
        save();
        return Ok(true);
    };
    crate::log::warn("background", &format!("{} was not opened as a chat: {why}", crate::agents::label(&id)));
    match lock().entries.get_mut(&id) {
        Some(e) => e.promoting = false,
        None => return Ok(false),
    }
    save();
    go_on(&id);
    Ok(false)
}

/// How long an opening as a chat may go unsettled before the helper goes back to
/// its parent: the window confirms in seconds and tries again for minutes, and one
/// that went away settles it when it comes back (agents/bridge.ts). Past this the
/// parent, which can neither stop nor delete it meanwhile, has waited enough.
pub(super) const PROMOTE_TIMEOUT_MS: u64 = 15 * 60 * 1000;

/// Openings unsettled past PROMOTE_TIMEOUT_MS at `now` go back to their parents,
/// which hear so (agents/handover.rs `opening_lapsed`). From each read of the
/// records (`lives`).
pub(super) fn lapse_openings(now: u64) {
    let overdue = |e: &Entry| e.promoting && now.saturating_sub(e.promoting_since) >= PROMOTE_TIMEOUT_MS;
    let ids: Vec<String> = lock().entries.iter().filter(|(_, e)| overdue(e)).map(|(id, _)| id.clone()).collect();
    for id in ids {
        // Again under the lock: a window settling it meanwhile has it.
        let lapsed = lock().entries.get_mut(&id).filter(|e| overdue(e)).map(|e| e.promoting = false).is_some();
        if !lapsed {
            continue;
        }
        crate::log::warn(
            "background",
            &format!(
                "{} was not opened as a chat: the window did not settle the opening within {} minutes",
                crate::agents::label(&id),
                PROMOTE_TIMEOUT_MS / 60_000
            ),
        );
        save();
        crate::agents::opening_lapsed(&id);
        go_on(&id);
    }
}

#[tauri::command]
pub fn background_kill(id: String) {
    kill(&id);
}

/// The person's Delete. A helper at work goes only with `stop_working`, a yes of
/// its own, which a box ticked for the chat above it is not (agents/teardown.rs).
/// `of_chat`, from a chat's Delete dialog: refused if the helper hangs from
/// another chat by now.
#[tauri::command]
pub fn background_delete(id: String, stop_working: Option<bool>, of_chat: Option<String>) -> Result<(), String> {
    crate::agents::delete_by_user(&id, stop_working.unwrap_or(false), of_chat.as_deref())
}

/// The person moves the helper, with the helpers under it, to chat `to`: from
/// the "Move to chat…" link on its row or the row's "Move to <chat>?"
/// (agents/reparent.rs).
/// Err, with nothing changed, says why not.
#[tauri::command]
pub fn background_move(id: String, to: String) -> Result<crate::agents::HelperMoved, String> {
    crate::agents::move_helper(&id, &to)
}

/// The chats helper `id` may move to, each with why not where it may not, for
/// the Move dialog to gray out (agents/reparent.rs `MoveTargets`).
#[tauri::command]
pub fn background_move_targets(id: String) -> crate::agents::MoveTargets {
    crate::agents::move_targets(&id)
}

/// What each helper named has at stake in a delete (agents/teardown.rs `AtStake`),
/// for the Delete dialog to list before anything goes.
#[tauri::command]
pub fn background_at_stake(ids: Vec<String>) -> Vec<crate::agents::AtStake> {
    // A dialog lists a chat's helpers, a handful: more is no call of the window's.
    crate::agents::at_stake(&ids[..ids.len().min(MAX_AT_STAKE)])
}

/// The most helpers one `background_at_stake` looks at: each takes a git status.
const MAX_AT_STAKE: usize = 32;

/// The most messages a look at a helper's conversation gives, the newest; and
/// the most characters of each. A first look at a long conversation would
/// otherwise carry all of it to the window at once.
const VIEW_MESSAGES: usize = 200;
const VIEW_TEXT: usize = 20_000;
/// The most of a conversation's file one look reads, back from its end: a first
/// look at a long one, or one grown by more since the last, starts that far back,
/// at a line's start, and says it passed over what came before. With tool results
/// in it a transcript runs to hundreds of MB, and a look comes every second.
const VIEW_SCAN: u64 = 2 * 1024 * 1024;
/// A line longer than this (a tool's whole output, a file read in) is passed over
/// unread rather than parsed.
const VIEW_LINE: usize = 256 * 1024;

/// What the window's read-only view of a helper shows (`background_transcript`).
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Transcript {
    pub messages: Vec<crate::agents::Message>,
    /// Pass back as `since` for what comes after.
    pub cursor: u64,
    /// The file it was read from: another one (a new conversation) starts the
    /// view over from the top.
    pub file: Option<String>,
    /// A turn is running: the view looks again soon.
    pub running: bool,
    /// Older messages a first look left out (VIEW_MESSAGES).
    #[serde(skip_serializing_if = "is_zero")]
    pub left_out: usize,
    /// Part of the conversation was passed over unread: too far back (VIEW_SCAN),
    /// or a line too long (VIEW_LINE).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub passed_over: bool,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// A helper's conversation from `since` on, for the window's read-only view of it:
/// read off its transcript by the same readers `luna_message_list` uses, tool calls marked.
/// The file comes from the helper's own record, never from the window; `file` is
/// the one `since` was a cursor into, and a cursor into another reads from the top.
#[tauri::command]
pub fn background_transcript(id: String, since: Option<u64>, file: Option<String>) -> Result<Transcript, String> {
    let (provider, running) = {
        let r = lock();
        let e = r.entries.get(&id).ok_or("no such helper")?;
        (e.spec.provider, e.run.is_some())
    };
    let Some(path) = transcript(&id) else {
        return Ok(Transcript { messages: Vec::new(), cursor: 0, file: None, running, left_out: 0, passed_over: false });
    };
    let same = file.is_some_and(|f| std::path::Path::new(&f) == path);
    Ok(view_of(&path, provider, if same { since.unwrap_or(0) } else { 0 }, running))
}

/// `background_transcript` of a file: at most VIEW_SCAN of it read into memory,
/// its complete lines no longer than VIEW_LINE given to the transcript readers
/// that work on bytes (`luna_lib::transcripts`), the newest VIEW_MESSAGES kept.
pub(crate) fn view_of(path: &std::path::Path, provider: Provider, since: u64, running: bool) -> Transcript {
    let len = std::fs::metadata(path).map_or(0, |m| m.len());
    // A cursor past the end is one into a file since cut short or written anew.
    let since = if since > len { 0 } else { since };
    let start = since.max(len.saturating_sub(VIEW_SCAN));
    let bytes = read_span(path, start, len);
    // Started past the cursor, most likely mid-line: from the next line on. With
    // none yet, from here, the fragment read as no message.
    let skip = if start > since { bytes.iter().position(|b| *b == b'\n').map_or(0, |i| i + 1) } else { 0 };
    let span = &bytes[skip..];
    // A last line without its newline is still being written: the next look's.
    let whole = span.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    let mut lines = Vec::with_capacity(whole);
    let mut too_long = false;
    for line in span[..whole].split_inclusive(|b| *b == b'\n') {
        if line.len() > VIEW_LINE {
            too_long = true;
            lines.push(b'\n');
        } else {
            lines.extend_from_slice(line);
        }
    }
    drop(bytes);
    let mut messages = match provider {
        Provider::Claude => crate::claude::session::messages_in(&lines, 0, true).0,
        Provider::Codex => crate::codex::session::messages_in(&lines).0,
    };
    let left_out = messages.len().saturating_sub(VIEW_MESSAGES);
    messages.drain(..left_out);
    for m in &mut messages {
        if m.text.chars().count() > VIEW_TEXT {
            m.text = format!("{}…", m.text.chars().take(VIEW_TEXT).collect::<String>());
        }
    }
    let cursor = start + (skip + whole) as u64;
    let passed_over = start > since || too_long;
    Transcript { messages, cursor, file: Some(path.to_string_lossy().into_owned()), running, left_out, passed_over }
}

/// Bytes `from..to` of a file, fewer if it is shorter by now; none if it will not read.
fn read_span(path: &std::path::Path, from: u64, to: u64) -> Vec<u8> {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else { return Vec::new() };
    if f.seek(SeekFrom::Start(from)).is_err() {
        return Vec::new();
    }
    let mut bytes = Vec::with_capacity(to.saturating_sub(from) as usize);
    let _ = f.take(to.saturating_sub(from)).read_to_end(&mut bytes);
    bytes
}

/// A helper's lifecycle record, off what Luna holds of it: its turn's process
/// and what its stream-json events said (a question or permission held on
/// `luna_permission`), and between turns what its turns left in its job, each a
/// shell running for as long as it is there.
pub(super) fn life_of(e: &Entry) -> crate::activity::Life {
    use crate::activity::{Background, Life, Outcome, Phase};
    let left = e.run.is_none() && e.left_running;
    let phase = match &e.run {
        Some(_) if !e.held.is_empty() => Phase::Asking,
        Some(_) => Phase::Running,
        None => Phase::Ended,
    };
    let ended = phase == Phase::Ended && e.turns_ended > 0;
    Life {
        phase,
        outcome: ended.then(|| Outcome::of_error(e.error.as_deref())),
        error: e.error.clone().filter(|_| ended),
        turn_started_ms: e.run.as_ref().map(|run| run.started_ms),
        background: Background {
            // One read as left that a stop is still ending, or one not yet
            // looked at by name, is there all the same.
            shells: if left { e.left.len().max(1) as u32 } else { 0 },
            names: if left { e.left.iter().map(Leftover::label).collect() } else { Vec::new() },
            unlisted: e.run.is_none() && (e.left_unlisted || !e.outside.is_empty()),
            ..Default::default()
        },
        turns_ended: e.turns_ended,
    }
}

/// A helper's record, None for no helper of that id.
pub fn life(id: &str) -> Option<crate::activity::Life> {
    lock().entries.get(id).map(life_of)
}

/// The helper's turn as the agent layer counts it, off its record.
pub fn turn_state(id: &str) -> Option<crate::activity::TurnState> {
    life(id).map(|l| crate::activity::TurnState::of(&l))
}

/// What a helper's turns left in its job, as read now, by name.
pub(super) fn left_of(read: Vec<(crate::procs::ProcRead, String)>) -> Vec<Leftover> {
    read.into_iter().map(|(r, exe)| Leftover { pid: r.pid, exe, started_ms: r.started_ms }).collect()
}

/// Notes what a helper's job held at a look between its turns (`procs::left_in`):
/// None, a job that would not list, keeps what was last seen and marks it not
/// known (`left_unlisted`), which its record counts as active. Whether what is
/// there changed, for the window and the power-off's quiet window.
pub(super) fn note_left(e: &mut Entry, read: Option<Vec<(crate::procs::ProcRead, String)>>) -> bool {
    let Some(read) = read else {
        let changed = !e.left_unlisted;
        e.left_unlisted = true;
        return changed;
    };
    let read = left_of(read);
    let pids = |left: &[Leftover]| left.iter().map(|l| (l.pid, l.started_ms)).collect::<Vec<_>>();
    let changed = e.left_unlisted || pids(&read) != pids(&e.left);
    e.left_unlisted = false;
    e.left_running = !read.is_empty();
    e.left = read;
    changed
}

/// Every helper's record, for the board power/ follows: between turns what
/// its turns left is read afresh from its job first. Each read lapses the
/// openings the window never settled too.
pub fn lives() -> Vec<(String, crate::activity::Life)> {
    lapse_openings(crate::throttle::now_ms());
    watch_waiting();
    let jobs: Vec<(String, std::sync::Arc<crate::procs::Job>)> =
        lock().entries.iter().filter(|(_, e)| e.run.is_none()).filter_map(|(id, e)| Some((id.clone(), e.job.clone()?))).collect();
    let mut moved = false;
    for (id, job) in jobs {
        // Read with no lock held: it takes a snapshot of every process.
        let read = crate::procs::left_in(&job);
        if let Some(e) = lock().entries.get_mut(&id).filter(|e| e.run.is_none() && !e.ending_left) {
            moved |= note_left(e, read);
        }
    }
    // A turn that was in no job: walked below by parent links until nothing
    // of it is left.
    let outside: Vec<(String, Vec<(u32, u64)>)> = lock()
        .entries
        .iter()
        .filter(|(_, e)| e.run.is_none() && !e.outside.is_empty())
        .map(|(id, e)| (id.clone(), e.outside.clone()))
        .collect();
    for (id, turns) in outside {
        let reads: Vec<((u32, u64), Option<Vec<(crate::procs::ProcRead, String)>>)> =
            turns.into_iter().map(|(pid, since)| ((pid, since), crate::procs::left_below(pid, since))).collect();
        if let Some(e) = lock().entries.get_mut(&id) {
            moved |= see_outside(e, reads);
        }
    }
    if moved {
        // What a job holds changing is activity, a process that came and went
        // between two reads included only if a read saw it.
        crate::activity::touch();
        changed();
    }
    lock().entries.iter().map(|(id, e)| (id.clone(), life_of(e))).collect()
}

/// What a walk found below each turn that was in no job (`Entry::outside`):
/// one with nothing left below it is done with; one that could not be walked,
/// or with something there, stays not known. Whether any went.
pub(super) fn see_outside(e: &mut Entry, reads: Vec<((u32, u64), Option<Vec<(crate::procs::ProcRead, String)>>)>) -> bool {
    let before = e.outside.len();
    for (turn, read) in reads {
        if read.is_some_and(|left| left.is_empty()) {
            e.outside.retain(|t| *t != turn);
        }
    }
    e.outside.len() != before
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A background helper's comes from the transcript its conversation is in.
    #[test]
    fn a_helpers_context_is_read_off_its_transcript() {
        let account = std::env::temp_dir().join(format!("luna-helper-context-{}", std::process::id()));
        let account = account.to_string_lossy().into_owned();
        test_helper("context-0", &account, None, false);
        assert_eq!(signs("context-0").unwrap().context, None, "no conversation yet");
        test_helper("context-1", &account, Some("sid-context"), false);
        assert_eq!(signs("context-1").unwrap().context, None, "no turn has run");
        let path = crate::claude::session::transcript_file(&account, &account, "sid-context");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let answer = r#"{"type":"assistant","message":{"model":"claude-sonnet-4-5","usage":{"input_tokens":150000},"content":[]}}"#;
        std::fs::write(&path, format!("{answer}\n")).unwrap();
        let s = signs("context-1").unwrap();
        assert_eq!(s.context, Some(Context { pct: 75, tokens: 150_000, window: 200_000 }));
        assert!(serde_json::to_string(&s).unwrap().contains(r#""context":{"pct":75,"tokens":150000,"window":200000}"#));
        lock().entries.remove("context-0");
        lock().entries.remove("context-1");
        let _ = std::fs::remove_dir_all(&account);
    }

    /// A turn that could not be put in the helper's job left what it started in
    /// no job: after it, the helper is not known (active), not ended on an
    /// empty job, until a walk below the turn finds nothing left.
    #[test]
    fn a_turn_in_no_job_is_not_known_until_a_walk_finds_nothing() {
        test_helper("outside-1", "C:\\acc", Some("s"), false);
        let mut r = lock();
        let e = r.entries.get_mut("outside-1").unwrap();
        e.turns_ended = 1;
        e.outside.push((4242, 1_000));
        assert!(e.job.is_none(), "nothing to list: no job at all");
        let l = life_of(e);
        assert!(l.background.unlisted && l.active(), "{l:?}");
        assert!(!see_outside(e, vec![((4242, 1_000), None)]), "a walk that could not be made says nothing");
        let shell = (crate::procs::ProcRead { pid: 4300, started_ms: 2_000 }, "node.exe".to_string());
        assert!(!see_outside(e, vec![((4242, 1_000), Some(vec![shell]))]));
        assert!(life_of(e).active(), "something of it still runs");
        assert!(see_outside(e, vec![((4242, 1_000), Some(Vec::new()))]));
        assert!(!life_of(e).active(), "nothing of it left");
        r.entries.remove("outside-1");
    }

    #[test]
    fn signs_say_when_the_turn_began_and_what_it_was_last_seen_doing() {
        test_helper("signs-1", "C:\\acc", Some("s"), false);
        let s = signs("signs-1").unwrap();
        assert_eq!((s.turn_started_at.clone(), s.last_activity_at.clone(), s.context.as_ref()), (None, None, None), "nothing known yet");
        let text = serde_json::to_string(&s).unwrap();
        assert_eq!(text, r#"{"turnStartedAt":null}"#, "null between turns, the rest left out");
        {
            let mut r = lock();
            let e = r.entries.get_mut("signs-1").unwrap();
            e.run = Some(Run::claimed(None, 1_793_000_000_000));
            e.observed.event_ms = 1_793_000_060_000;
        }
        let s = signs("signs-1").unwrap();
        assert_eq!(s.turn_started_at.as_deref(), iso(1_793_000_000_000).as_deref());
        assert_eq!(s.last_activity_at.as_deref(), iso(1_793_000_060_000).as_deref());
        assert!(s.turn_started_at.unwrap().ends_with('Z'));
        lock().entries.remove("signs-1");
    }

    /// The window's view of a helper tails its conversation: a first look has
    /// what there is (the newest, if long), each next one what came since, both
    /// providers by the readers `luna_message_list` uses; another file starts over.
    #[test]
    fn the_view_of_a_helper_tails_its_conversation_read_only() {
        let dir = std::env::temp_dir().join(format!("luna-view-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let append = |path: &std::path::Path, line: String| {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
            writeln!(f, "{line}").unwrap();
        };
        let claude_user = |t: &str| json!({ "type": "user", "message": { "role": "user", "content": t } }).to_string();
        let claude_said = |t: &str| json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": t }] } }).to_string();
        let codex_user = |t: &str| {
            json!({ "type": "response_item", "payload": { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": t }] } }).to_string()
        };
        let codex_said = |t: &str| {
            json!({ "type": "response_item", "payload": { "type": "message", "role": "assistant", "content": [{ "type": "output_text", "text": t }] } }).to_string()
        };
        for (provider, user, said) in [
            (Provider::Claude, &claude_user as &dyn Fn(&str) -> String, &claude_said as &dyn Fn(&str) -> String),
            (Provider::Codex, &codex_user, &codex_said),
        ] {
            let path = dir.join(format!("{}.jsonl", provider.as_str()));
            append(&path, user("do the job"));
            append(&path, said("starting on it"));
            let first = view_of(&path, provider, 0, true);
            let texts: Vec<&str> = first.messages.iter().map(|m| m.text.as_str()).collect();
            assert_eq!(texts, ["do the job", "starting on it"], "{provider:?}");
            assert!(first.running && first.left_out == 0);
            // Nothing new: nothing.
            assert!(view_of(&path, provider, first.cursor, true).messages.is_empty());
            // A line still being written is not read until it is whole.
            {
                use std::io::Write;
                let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
                write!(f, "{}", &said("half")[..10]).unwrap();
            }
            let half = view_of(&path, provider, first.cursor, true);
            assert!(half.messages.is_empty() && half.cursor == first.cursor, "{provider:?}");
            let _ = std::fs::write(&path, format!("{}\n{}\n{}\n", user("do the job"), said("starting on it"), said("done: all green")));
            let next = view_of(&path, provider, first.cursor, false);
            assert_eq!(next.messages.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(), ["done: all green"], "{provider:?}");
            assert!(!next.running);
            let _ = std::fs::remove_file(&path);
        }
        // A long conversation: the newest, each cut to its length.
        let long = dir.join("long.jsonl");
        for i in 0..(VIEW_MESSAGES + 5) {
            append(&long, claude_said(&format!("message {i}")));
        }
        append(&long, claude_said(&"x".repeat(VIEW_TEXT + 10)));
        let v = view_of(&long, Provider::Claude, 0, false);
        assert_eq!((v.messages.len(), v.left_out), (VIEW_MESSAGES, 6));
        assert!(v.messages.last().unwrap().text.ends_with('…') && v.messages.last().unwrap().text.chars().count() == VIEW_TEXT + 1);
        let _ = std::fs::remove_dir_all(&dir);
        // Only a helper there is, and by its own record.
        assert!(background_transcript("view-nobody".into(), None, None).unwrap_err().contains("no such helper"));
        test_helper("view-0", "C:\\acc", None, false);
        let none = background_transcript("view-0".into(), Some(99), Some("C:\\elsewhere.jsonl".into())).unwrap();
        assert!(none.messages.is_empty() && none.file.is_none(), "no conversation yet");
        lock().entries.remove("view-0");
    }

    /// A look reads no more than VIEW_SCAN back from the end, whatever the file or
    /// the cursor, from a line's start, and passes over a line past VIEW_LINE
    /// unread; it says when it did either.
    #[test]
    fn the_view_of_a_helper_reads_a_bounded_tail() {
        let dir = std::env::temp_dir().join(format!("luna-view-bounded-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let said = |t: &str| json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": t }] } }).to_string();
        let codex_said = |t: &str| {
            json!({ "type": "response_item", "payload": { "type": "message", "role": "assistant", "content": [{ "type": "output_text", "text": t }] } }).to_string()
        };
        for (provider, line) in [(Provider::Claude, &said as &dyn Fn(&str) -> String), (Provider::Codex, &codex_said)] {
            let path = dir.join(format!("{}.jsonl", provider.as_str()));
            // Old lines worth more than a look reads, then the newest.
            let filler = line(&"o".repeat(1000));
            let mut text = String::new();
            while (text.len() as u64) < VIEW_SCAN + 4096 {
                text.push_str(&filler);
                text.push('\n');
            }
            text.push_str(&format!("{}\n", line(&"x".repeat(VIEW_LINE + 1))));
            text.push_str(&format!("{}\n", line("the newest")));
            std::fs::write(&path, &text).unwrap();
            let v = view_of(&path, provider, 0, false);
            assert!(v.passed_over, "{provider:?}");
            assert_eq!(v.cursor, text.len() as u64);
            assert_eq!(v.messages.last().map(|m| m.text.as_str()), Some("the newest"), "{provider:?}");
            assert!(v.messages.iter().all(|m| !m.text.starts_with('x')), "the long line was not read");
            assert!(v.messages.len() <= VIEW_MESSAGES);
            // Grown by more than a look reads since the last: the same.
            let later = view_of(&path, provider, 10, false);
            assert!(later.passed_over && later.cursor == text.len() as u64);
            // Nothing new: nothing, and nothing passed over.
            let again = view_of(&path, provider, v.cursor, false);
            assert!(again.messages.is_empty() && !again.passed_over);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An opening the window never settles goes back to the helper's parent past
    /// PROMOTE_TIMEOUT_MS, which hears so; the row a window makes for it after
    /// that is refused, the helper its parent's still. Both providers alike.
    #[test]
    fn an_opening_left_unsettled_goes_back_to_its_parent() {
        for (provider, tag) in [(Provider::Claude, "cl"), (Provider::Codex, "cx")] {
            let (parent, id) = (format!("lo-{tag}-parent"), format!("lo-{tag}-helper"));
            crate::agents::register(&parent, Provider::Claude, "C:\\p", "C:\\acc\\one", None, true, json!({}));
            crate::agents::register(&id, provider, "C:\\p", "C:\\acc\\one", Some(&parent), false, json!({ "background": true }));
            idle_for_test(&id, provider, 1, false);
            background_promote(id.clone()).unwrap();
            let since = lock().entries[&id].promoting_since;
            lapse_openings(since + PROMOTE_TIMEOUT_MS - 1);
            assert!(promoting(&id), "not yet");
            lapse_openings(since + PROMOTE_TIMEOUT_MS);
            assert!(!promoting(&id), "its parent's again");
            let told = crate::agents::news_for(&parent);
            assert!(told.iter().any(|l| l.contains(&format!("your helper {id}")) && l.contains("did not open")), "{told:?}");
            assert_eq!(background_promoted(id.clone(), None), Ok(false), "a row made now is refused");
            assert!(has(&id) && crate::agents::parent_of(&id).as_deref() == Some(parent.as_str()));
            gone_for_test(&id);
            crate::agents::forget(&id);
            crate::agents::forget(&parent);
        }
    }

    #[test]
    fn a_row_carries_its_task_and_last_activity() {
        let mut e = entry(Provider::Claude);
        e.observed.event_ms = 42;
        lock().entries.insert("row-task".into(), e);
        crate::agents::set_task_for_test("row-task", "Check the changelog");
        let row = background_helpers().into_iter().find(|r| r.id == "row-task").unwrap();
        assert_eq!(row.task.as_deref(), Some("Check the changelog"));
        assert_eq!(row.last_activity_ms, Some(42));
        let json = serde_json::to_value(&row).unwrap();
        assert_eq!(json["lastActivityMs"], 42);
        assert_eq!(json["task"], "Check the changelog");
        lock().entries.remove("row-task");
        crate::agents::forget("row-task");
    }

    #[test]
    fn a_job_left_running_reads_as_one_between_turns_only() {
        let mut e = entry(Provider::Claude);
        e.turns_ended = 1;
        e.left_running = true;
        lock().entries.insert("left-job".into(), e);
        let children_busy = || turn_state("left-job").unwrap().children_busy;
        assert!(children_busy(), "between turns, what its turn left is a job");
        lock().entries.get_mut("left-job").unwrap().run = Some(Run::claimed(Some(1), 0));
        assert!(!children_busy(), "in a turn it is simply working");
        lock().entries.remove("left-job");
    }
}
