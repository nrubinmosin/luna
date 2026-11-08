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
    pub worktree: bool,
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
}

/// Every helper, for the window.
#[tauri::command]
pub fn background_helpers() -> Vec<Row> {
    let ids: Vec<(String, Spec, u64, Option<u64>, Vec<String>, bool)> = lock()
        .entries
        .iter()
        .map(|(id, e)| {
            let left = if e.run.is_none() && e.left_running { e.left.iter().map(Leftover::label).collect() } else { Vec::new() };
            (id.clone(), e.spec.clone(), e.started_ms, e.run.as_ref().map(|r| r.started_ms), left, e.promoting)
        })
        .collect();
    ids.into_iter()
        .map(|(id, spec, started_ms, turn_started_ms, left_running, promoting)| Row {
            parent: crate::agents::parent_of(&id),
            name: crate::agents::chat_name(&id).unwrap_or_default(),
            state: crate::agents::state_of(&id).0,
            ask: ask(&id),
            provider: spec.provider,
            account: std::path::Path::new(&spec.account_path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            model: spec.model,
            effort: spec.effort,
            worktree: spec.worktree,
            started_ms,
            turn_started_ms,
            left_running,
            promoting,
            id,
        })
        .collect()
}

/// Tells the window the list has changed.
pub fn changed() {
    crate::emit::to_ui("agent://background", background_helpers());
}

/// What `luna_list` says of a helper beyond its state.
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
            left_running: left_shown(e, crate::throttle::now_ms()),
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
        e.promoting = true;
        (e.spec.clone(), e.cwd.clone(), e.session.clone())
    };
    changed();
    let worktree_path = cwd.filter(|c| !crate::pty::same_dir(c, &spec.folder));
    Ok(Promoted {
        provider: spec.provider,
        account_path: spec.account_path.clone(),
        folder: crate::worktree::project_of(&spec.folder).unwrap_or_else(|| spec.folder.clone()),
        worktree_path: worktree_path.or_else(|| crate::worktree::project_of(&spec.folder).map(|_| spec.folder.clone())),
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
/// what one gone before it was opening, whose row it finds or not.
#[tauri::command]
pub fn background_promoted(id: String, error: Option<String>) -> Result<(), String> {
    let Some(why) = error else {
        let mut count = None;
        crate::agents::take_over(&id, || {
            // Gone already (deleted meanwhile): the row has its conversation.
            let mut e = lock().entries.remove(&id)?;
            count = Some(e.turns_ended);
            let mut unsent = std::mem::take(&mut e.undelivered);
            unsent.extend(std::mem::take(&mut e.queue));
            Some(unsent)
        });
        // Its count goes on in the chat's track, so a count anyone was shown of it
        // stays one it has passed.
        if let Some(n) = count {
            crate::activity::count_at_least(&id, n);
        }
        changed();
        // Or a core killed now would take it up beside the chat it has become.
        save();
        return Ok(());
    };
    crate::log::warn("background", &format!("{} was not opened as a chat: {why}", crate::agents::label(&id)));
    let next = {
        let mut r = lock();
        let Some(e) = r.entries.get_mut(&id) else { return Ok(()) };
        e.promoting = false;
        (e.run.is_none() && !e.ending_left && !e.queue.is_empty()).then(|| {
            claim(e);
            reserve(e)
        })
    };
    let next = next.and_then(|res| settle_batch(&id, res)).map(|b| b.text);
    match next {
        Some(text) => {
            save();
            if run_turn(&id, &text).is_err() {
                if let Some(e) = lock().entries.get_mut(&id) {
                    e.in_flight.clear();
                }
            }
        }
        // A `resumeOnRestart` helper a restart left to go on, which the opening
        // held back: it goes on now, if nothing else has taken it up.
        None if crate::agents::resumes_on_restart(&id) => resume(&id),
        None => {}
    }
    changed();
    Ok(())
}

#[tauri::command]
pub fn background_kill(id: String) {
    kill(&id);
}

/// The person's Delete. A helper at work goes only with `stop_working`, and a
/// worktree holding unsaved work only with `drop_unsaved`: each is a yes of its
/// own, which a box ticked for the chat above it is not (agents/teardown.rs).
#[tauri::command]
pub fn background_delete(
    id: String,
    drop_worktree: Option<bool>,
    stop_working: Option<bool>,
    drop_unsaved: Option<bool>,
) -> Result<crate::agents::UserDeleted, String> {
    crate::agents::delete_by_user(&id, drop_worktree.unwrap_or(true), stop_working.unwrap_or(false), drop_unsaved.unwrap_or(false))
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
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// A helper's conversation from `since` on, for the window's read-only view of it:
/// read off its transcript by the same readers `luna_read` uses, tool calls marked.
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
        return Ok(Transcript { messages: Vec::new(), cursor: 0, file: None, running, left_out: 0 });
    };
    let same = file.is_some_and(|f| std::path::Path::new(&f) == path);
    Ok(view_of(&path, provider, if same { since.unwrap_or(0) } else { 0 }, running))
}

/// `background_transcript` of a file.
fn view_of(path: &std::path::Path, provider: Provider, since: u64, running: bool) -> Transcript {
    let (mut messages, cursor) = match provider {
        Provider::Claude => crate::claude::session::messages_from(path, since, true),
        Provider::Codex => crate::codex::session::messages_from(path, since),
    };
    let left_out = messages.len().saturating_sub(VIEW_MESSAGES);
    messages.drain(..left_out);
    for m in &mut messages {
        if m.text.chars().count() > VIEW_TEXT {
            m.text = format!("{}…", m.text.chars().take(VIEW_TEXT).collect::<String>());
        }
    }
    Transcript { messages, cursor, file: Some(path.to_string_lossy().into_owned()), running, left_out }
}

/// The helper's turn as the agent layer counts it: busy while a turn's
/// process runs, waiting while a question or permission is held.
pub fn turn_state(id: &str) -> Option<crate::activity::TurnState> {
    use crate::activity::Turn;
    let r = lock();
    let e = r.entries.get(id)?;
    let turn = match &e.run {
        Some(_) if !e.held.is_empty() => Turn::Waiting,
        Some(_) => Turn::Busy,
        None => Turn::Idle,
    };
    Some(crate::activity::TurnState {
        turn,
        busy: e.run.is_some(),
        turns_ended: e.turns_ended,
        children_busy: e.run.is_none() && e.left_running,
    })
}

/// Whether a helper's turn keeps the machine awake: one held on a question or
/// permission waits for whoever answers it, which blocks a shutdown but keeps
/// nothing awake.
pub(super) fn turn_holds(t: &crate::activity::TurnState) -> bool {
    t.busy && t.turn != crate::activity::Turn::Waiting
}

/// What the power rules see of every helper: its turn, and between turns what
/// its turns left running, which holds the machine until it has done nothing
/// for LEFTOVER_HOLD_MS on end (`Leftover::holds`). Noted for `turn_state`,
/// which is what tells its caller and `reap` that a job still runs, however idle.
pub fn activity() -> Vec<crate::activity::SessionActivity> {
    let ids: Vec<(String, Option<std::sync::Arc<crate::procs::Job>>)> =
        lock().entries.iter().map(|(id, e)| (id.clone(), e.job.clone().filter(|_| e.run.is_none()))).collect();
    let mut moved = false;
    let out = ids
        .into_iter()
        .filter_map(|(id, job)| {
            let read = job.map(|job| crate::procs::left_in(&job));
            let now = crate::throttle::now_ms();
            let mut at_work = Vec::new();
            let mut gone_quiet = Vec::new();
            if let Some(read) = read {
                if let Some(e) = lock().entries.get_mut(&id).filter(|e| e.run.is_none()) {
                    let pids = |left: &[Leftover]| left.iter().map(|l| (l.pid, l.started_ms)).collect::<Vec<_>>();
                    let before = pids(&e.left);
                    e.left_running = !read.is_empty();
                    let left = leftovers(&e.left, read, now, e.observed.turn_ended_ms);
                    gone_quiet = crate::procs::gone_quiet(&e.left, &left, now);
                    e.left = left;
                    moved |= pids(&e.left) != before;
                    at_work = e.left.iter().filter(|l| l.holds(now)).map(Leftover::label).collect();
                }
            }
            if !gone_quiet.is_empty() {
                crate::log::warn(
                    "power",
                    &format!(
                        "{}: no longer holds the PC by what its turns left, idle {} min on end: {}",
                        crate::agents::label(&id),
                        crate::procs::LEFTOVER_HOLD_MS / 60_000,
                        gone_quiet.join(", ")
                    ),
                );
            }
            let t = turn_state(&id)?;
            let mut a = crate::activity::SessionActivity::of_turn(&id, t.turn, turn_holds(&t));
            if !at_work.is_empty() {
                at_work.sort();
                at_work.dedup();
                a.busy = true;
                a.procs = at_work;
            }
            Some(a)
        })
        .collect();
    if moved {
        changed();
    }
    out
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
    /// providers by the readers `luna_read` uses; another file starts over.
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
