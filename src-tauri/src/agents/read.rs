//! A session's transcript: where it is, what it holds as messages, and
//! `luna_read`, which hands them over from a cursor on.

use crate::agents::ask::{state_of, Ask};
use crate::agents::delivery::screen_of;
use crate::agents::lost::{lost_count, lost_whole, undelivered_note};
use crate::agents::owed::{hand_over, queue_settled, seen};
use crate::agents::registry::{authorize_view, label, reg};
use crate::agents::resume_pending;
use crate::provider::Provider;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// The most of a session's words one tool result carries: Claude Code cuts
/// an MCP result past 25,000 tokens.
pub(super) const MAX_TEXT: usize = 60_000;

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct Message {
    pub role: String,
    pub text: String,
    /// The first message after a prompt the CLI put in itself (a background
    /// task's end it takes up), which its reader leaves out: a turn's start
    /// all the same, for which report leads (`own_words`).
    #[serde(skip)]
    pub after_cli: bool,
}

/// How far the session's transcript goes now, 0 before it has one.
pub(super) fn transcript_end(id: &str) -> u64 {
    transcript_path(crate::pty::manager(), id).and_then(|p| std::fs::metadata(p).ok()).map_or(0, |m| m.len())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reading {
    pub messages: Vec<Message>,
    /// Pass back as `since` to read only what came after.
    pub cursor: u64,
    /// Messages MAX_TEXT left out of this result: read from `since` on,
    /// the ones after `cursor`, still to read; with `last`, older ones.
    #[serde(skip_serializing_if = "is_zero")]
    pub left_out: usize,
    /// See `state_of`.
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ask: Option<Ask>,
    pub turns_ended: u32,
    /// The terminal's last lines, when asked for (`screen: true`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screen: Option<String>,
    /// See `undelivered_note`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub undelivered: Option<String>,
    /// See `Waited::transcript_missing`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub transcript_missing: bool,
}

/// The last transcript found for each session: Claude Code drops the
/// registry entry it is found through when it exits, and an exited helper's
/// last reply is still asked for.
pub(super) fn transcripts() -> &'static Mutex<HashMap<String, std::path::PathBuf>> {
    static T: OnceLock<Mutex<HashMap<String, std::path::PathBuf>>> = OnceLock::new();
    T.get_or_init(Default::default)
}

pub(super) fn transcript_path(pty: &crate::pty::PtyManager, id: &str) -> Option<std::path::PathBuf> {
    transcript_seen(pty, id).0
}

/// `transcript_path`, and where what is owed now reads from in it if this very
/// look found the session gone on in another (`moved_to`): a cursor its caller
/// took before is into the last one.
pub(super) fn transcript_seen(pty: &crate::pty::PtyManager, id: &str) -> (Option<std::path::PathBuf>, Option<u64>) {
    let found = find_transcript(pty, id);
    let mut known = transcripts().lock().unwrap_or_else(|e| e.into_inner());
    match found {
        Some(path) => {
            let was = known.insert(id.to_string(), path.clone()).filter(|was| *was != path);
            drop(known);
            let moved = was.map(|was| moved_to(id, &was, &path));
            (Some(path), moved)
        }
        None => (known.get(id).cloned(), None),
    }
}

/// A session went on in another transcript: Claude Code's `/clear` or
/// `/resume`, Codex's `/new`, `/resume` or `/fork` in its TUI. What it is owed
/// for is read on from where its words in that one start: all of one made after
/// the last it was on, as a fresh conversation is; past what is there now of an
/// older one it went back to. Offsets into the last say nothing of it. (Claude
/// Code writes to the transcript it leaves after making the next.)
fn moved_to(id: &str, was: &std::path::Path, now: &std::path::Path) -> u64 {
    let born = |p: &std::path::Path| std::fs::metadata(p).and_then(|m| m.created()).ok();
    let fresh = matches!((born(now), born(was)), (Some(now), Some(was)) if now > was);
    let from = if fresh { 0 } else { std::fs::metadata(now).map_or(0, |m| m.len()) };
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let mut moved = 0;
    for ((_, of), o) in r.owed.iter_mut() {
        if of == id {
            o.cursor = from;
            moved += 1;
        }
    }
    drop(r);
    crate::log::info(
        "agents",
        &format!("{} goes on in {} (was {}): {moved} owed answer(s) read from {from}", label(id), now.display(), was.display()),
    );
    from
}

fn find_transcript(pty: &crate::pty::PtyManager, id: &str) -> Option<std::path::PathBuf> {
    if crate::background::has(id) {
        return crate::background::transcript(id);
    }
    let p = pty.probe(id)?;
    match p.provider {
        Provider::Claude => crate::claude::session::transcript_of(p.pid, p.resume.as_deref(), &p.cwd, p.spawned_at_ms, &p.account_path),
        Provider::Codex => crate::codex::session::rollout_for(&crate::codex::session::Live {
            chat_id: &p.id,
            cwd: &p.cwd,
            spawned_at_ms: p.spawned_at_ms,
            resume: p.resume.as_deref(),
            account_path: &p.account_path,
            last_output_ms: p.last_output_ms,
        }),
    }
}

/// Whether the session's transcript is on disk where Luna looks for it. Without
/// one, "it said nothing" cannot be told from "nothing could be read".
pub(super) fn transcript_found(pty: &crate::pty::PtyManager, id: &str) -> bool {
    transcript_path(pty, id).is_some_and(|p| p.is_file())
}

/// Why a session's last turn stopped short, if it did: an API error, a usage
/// limit, a CLI that quit on an error or was killed. On one line, as it goes
/// into a notice. A background helper knows from its process and its events;
/// a Claude Code chat from its transcript's last word and its `StopFailure`
/// hook; a Codex chat from its rollout; and either from its exit code.
pub(super) fn failure_of(pty: &crate::pty::PtyManager, id: &str, provider: Provider) -> Option<String> {
    let exit = || {
        let code = reg().lock().unwrap_or_else(|e| e.into_inner()).exits.get(id).copied()?;
        match code {
            Some(0) => None,
            Some(code) => Some(format!("its CLI exited with code {code}")),
            None => Some("its CLI was killed".to_string()),
        }
    };
    let why = match provider {
        _ if crate::background::has(id) => crate::background::last_error(id),
        Provider::Claude => transcript_path(pty, id)
            .and_then(|p| crate::claude::session::api_error_of(&p))
            .or_else(|| crate::activity::failed_of(id).map(str::to_owned))
            .or_else(exit),
        Provider::Codex => crate::codex::session::last_failure(id).or_else(exit),
    };
    why.map(|w| crate::background::one_line(&w))
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// Messages read off a transcript, with the offset the read stopped at.
type Messages = (Vec<Message>, u64);

/// The conversation of a session from `since` (a cursor from an earlier read;
/// None = from the start), trimmed to the last `last` messages when asked,
/// and to MAX_TEXT: read on from `since`, to the oldest that fit, the cursor
/// put back to where they end; with `last`, to the newest. A read that hands
/// over all the session said since the caller asked it something collects
/// that answer, as a `wait` would.
pub fn read(
    caller: &str,
    id: &str,
    since: Option<u64>,
    last: Option<usize>,
    with_tools: bool,
    with_screen: bool,
    call: &crate::mcp::Call,
) -> Result<Reading, String> {
    let known = authorize_view(caller, id)?;
    let pty = crate::pty::manager();
    let path = transcript_path(pty, id);
    // Taken before the transcript is read: a turn over by now has its words
    // in what is read.
    let ts = crate::activity::turn_state(id);
    // A cursor past the end points into a file since cut short or written
    // anew, which the transcript is then read from the top of: the start of
    // this read, for the cursor it hands back too.
    let past_end = |at: u64| path.as_ref().is_some_and(|p| std::fs::metadata(p).is_ok_and(|m| at > m.len()));
    let from = since.filter(|&at| !past_end(at)).unwrap_or(0);
    let messages_at = |at: u64| -> Messages {
        match &path {
            Some(p) => match known.provider {
                Provider::Claude => crate::claude::session::messages_from(p, at, with_tools),
                Provider::Codex => crate::codex::session::messages_from(p, at),
            },
            None => (Vec::new(), at),
        }
    };
    let (mut messages, mut cursor) = messages_at(from);
    let found = messages.len();
    // Left out at the front (`last`, or MAX_TEXT keeping the newest) and at
    // the back (MAX_TEXT keeping the oldest).
    let (mut front, mut back) = (0, 0);
    if let Some(n) = last {
        let n = n.max(1);
        if messages.len() > n {
            front = messages.len() - n;
            messages.drain(..front);
        }
    }
    let size = |m: &Message| m.text.chars().count();
    let mut left_out = 0;
    let mut text_cut = false;
    if messages.iter().map(size).sum::<usize>() > MAX_TEXT {
        // As many as fit, and the first one taken whatever its size.
        let fit = |sizes: &mut dyn Iterator<Item = usize>| {
            let (mut room, mut keep) = (MAX_TEXT, 0);
            for n in sizes {
                if n > room && keep > 0 {
                    break;
                }
                room = room.saturating_sub(n);
                keep += 1;
            }
            keep
        };
        if last.is_some() {
            let keep = fit(&mut messages.iter().rev().map(size));
            left_out = messages.len() - keep;
            front += left_out;
            messages.drain(..left_out);
        } else {
            let keep = fit(&mut messages.iter().map(size));
            left_out = messages.len() - keep;
            back = left_out;
            if left_out > 0 {
                messages.truncate(keep);
                if let Some(p) = &path {
                    cursor = cursor_after(p, from, cursor, found, keep, &messages_at);
                }
            }
        }
        for m in messages.iter_mut().filter(|m| m.text.chars().count() > MAX_TEXT) {
            let more = m.text.chars().count() - MAX_TEXT;
            m.text = format!(
                "{}\n\n[cut: {more} more characters; the transcript luna_sessions names has it whole]",
                m.text.chars().take(MAX_TEXT).collect::<String>()
            );
            text_cut = true;
        }
    }
    let turns_ended = ts.map(|t| t.turns_ended).unwrap_or(0);
    let (state, ask) = state_of(id);
    let key = (caller.to_string(), id.to_string());
    let owed = reg().lock().unwrap_or_else(|e| e.into_inner()).owed.get(&key).copied();
    // All of the answer: every message past where the question found the
    // transcript is in this result, whole, the session's own word last. A
    // read begun past that point, a `last` that left some of them out, a
    // read cut short, is not the answer, and the notice still comes.
    let whole = owed.is_some_and(|o| {
        !text_cut
            && back == 0
            && from <= o.cursor
            && messages.last().is_some_and(|m| m.role == "assistant")
            && (front == 0 || messages.len() >= messages_at(o.cursor).0.len())
    });
    // And the session was done with it before the read: a read while it
    // works on has seen a turn on the way, and the answer is still to come.
    let done = ts.is_none_or(|t| t.turn == crate::activity::Turn::Idle && !t.children_busy);
    let settles = whole
        && done
        && !resume_pending(id)
        && owed.is_some_and(|o| turns_ended > o.baseline && (!o.queued || queue_settled(&key, o, turns_ended)));
    // Ahead of the hand-over, which takes it off. With an answer still to come
    // it stays for the notice; with none owed this read is all that tells it.
    let undelivered = undelivered_note(caller, id);
    if undelivered.is_some() && lost_whole(pty, caller, id) {
        // No answer is to come: this was all of it, and no notice is to say it again.
        // Canceled first, it and the debt are put back together.
        call.commit(hand_over(caller, id, lost_count(caller, id), None));
    } else if settles || (undelivered.is_some() && owed.is_none()) {
        // Canceled first, it is put back; the response goes nowhere anyway.
        call.commit(hand_over(caller, id, turns_ended, None));
    } else {
        seen(caller, id, turns_ended);
    }
    Ok(Reading {
        transcript_missing: messages.is_empty() && !transcript_found(pty, id),
        messages,
        cursor,
        left_out,
        state,
        ask,
        turns_ended,
        screen: with_screen.then(|| screen_of(pty, id)).flatten(),
        undelivered,
    })
}

/// Where a read from `from`, which found `found` messages up to `end` and
/// hands over the first `keep` of them, is to pick up again: the first line
/// start past which only the rest are left. What the transcript gains
/// meanwhile is not counted as the rest; should it grow in the middle of a
/// look, the cursor lands early, and a message comes twice rather than not
/// at all.
fn cursor_after(path: &std::path::Path, from: u64, end: u64, found: usize, keep: usize, messages_at: &dyn Fn(u64) -> Messages) -> u64 {
    use std::io::{Read, Seek, SeekFrom};
    let mut bytes = Vec::new();
    let read = std::fs::File::open(path).and_then(|mut f| {
        f.seek(SeekFrom::Start(from))?;
        f.take(end.saturating_sub(from)).read_to_end(&mut bytes)
    });
    // Not knowing where the kept ones end, start this page over: they come
    // twice rather than the rest not at all.
    if read.is_err() {
        return from;
    }
    let starts: Vec<u64> = bytes.iter().enumerate().filter(|(_, b)| **b == b'\n').map(|(i, _)| from + i as u64 + 1).collect();
    let past_kept = |at: u64| {
        let (after, reached) = messages_at(at);
        let grown = if reached > end { messages_at(end).0.len() } else { 0 };
        found.saturating_sub(after.len().saturating_sub(grown)) >= keep
    };
    // The last start is `end` itself, past every one of them.
    starts.get(starts.partition_point(|&at| !past_kept(at))).copied().unwrap_or(end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::delivery::is_held;
    use crate::agents::forget;
    use crate::agents::owed::owe_from;
    use crate::agents::registry::{reg, register, session_exited};
    use crate::agents::tests::lost_helper;

    #[test]
    fn a_cut_read_goes_on_where_the_kept_messages_end() {
        let path = std::env::temp_dir().join(format!("luna-cursor-after-{}.jsonl", std::process::id()));
        std::fs::write(&path, "a\nb\nc\n").unwrap();
        // One message a line, as the transcript readers count them.
        let messages_at = |at: u64| -> Messages {
            let text = std::fs::read_to_string(&path).unwrap();
            let rest = &text[(at as usize).min(text.len())..];
            (rest.lines().map(|l| Message { role: "assistant".into(), text: l.into(), after_cli: false }).collect(), text.len() as u64)
        };
        assert_eq!(cursor_after(&path, 0, 6, 3, 1, &messages_at), 2);
        assert_eq!(cursor_after(&path, 2, 6, 2, 1, &messages_at), 4);
        let _ = std::fs::remove_file(&path);
        let nothing = |at: u64| -> Messages { (Vec::new(), at) };
        assert_eq!(cursor_after(&path, 2, 6, 2, 1, &nothing), 2, "unreadable: this page over, not past the rest");
    }

    #[test]
    fn an_exit_says_how_it_went() {
        let none = serde_json::json!({});
        register("ex-bad", Provider::Codex, "C:\\p", "C:\\acc\\openai\\one", None, false, none.clone());
        let pty = crate::pty::manager();
        assert_eq!(failure_of(pty, "ex-bad", Provider::Codex), None);
        session_exited("ex-bad", Some(3));
        assert_eq!(failure_of(pty, "ex-bad", Provider::Codex).as_deref(), Some("its CLI exited with code 3"));
        session_exited("ex-bad", Some(0));
        assert_eq!(failure_of(pty, "ex-bad", Provider::Codex), None, "a clean exit is no error");
        session_exited("ex-bad", Some(3));
        register("ex-bad", Provider::Codex, "C:\\p", "C:\\acc\\openai\\one", None, false, none);
        assert_eq!(failure_of(pty, "ex-bad", Provider::Codex), None, "running again, that is behind it");
        forget("ex-bad");
    }

    #[test]
    fn a_read_of_a_lost_first_turn_collects_it_and_a_cancel_puts_it_back_with_the_debt() {
        let dir = std::env::temp_dir().to_string_lossy().into_owned();
        lost_helper("lr-parent", "lr-chat", &dir, &["and a follow-up"]);
        let key = ("lr-parent".to_string(), "lr-chat".to_string());
        let read_it = |call: &crate::mcp::Call| read("lr-parent", "lr-chat", None, None, false, false, call).unwrap().undelivered;
        let call = crate::mcp::Call::new();
        let note = read_it(&call).unwrap();
        assert!(note.contains("never got started") && note.contains("its prompt and 1 queued message are in "), "{note}");
        assert!(!reg().lock().unwrap().owed.contains_key(&key), "all there was: no notice is to say it again");
        assert!(!is_held("lr-parent", "lr-chat"));
        assert_eq!(read_it(&crate::mcp::Call::new()), None, "and no read either");
        call.cancel();
        assert!(reg().lock().unwrap().owed.contains_key(&key) && is_held("lr-parent", "lr-chat"), "payload and debt together");
        assert!(read_it(&crate::mcp::Call::new()).is_some());
        // Another reader takes nothing of it.
        reg().lock().unwrap().held.retain(|h| h.id != "lr-chat");
        forget("lr-chat");

        // A background helper's is the same, its prompt its own way.
        register("lb-child", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("lr-parent"), false, serde_json::json!({}));
        crate::background::take_up_for_test("lb-child", "C:\\p", 0, &[], Some("do the whole job"));
        owe_from("lr-parent", "lb-child", 0, 0);
        let call = crate::mcp::Call::new();
        let note = read("lr-parent", "lb-child", None, None, false, false, &call).unwrap().undelivered.unwrap();
        assert!(note.contains("never got started"), "{note}");
        assert!(!reg().lock().unwrap().owed.contains_key(&("lr-parent".to_string(), "lb-child".to_string())));
        assert_eq!(read("lr-parent", "lb-child", None, None, false, false, &crate::mcp::Call::new()).unwrap().undelivered, None);
        crate::background::remove("lb-child");
        forget("lb-child");
        crate::background::gone_for_test("lr-parent");
        forget("lr-parent");
    }
}
