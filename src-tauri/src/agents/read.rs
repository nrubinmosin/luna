//! A session's transcript: where it is, what it holds as messages, and
//! `luna_message_list`, which hands them over from a cursor on.

use crate::agents::ask::{state_of, Ask};
use crate::agents::delivery::screen_of;
use crate::agents::lost::{lost_count, lost_whole, undelivered_note};
use crate::agents::owed::{hand_over, looked, queue_settled, seen};
use crate::agents::registry::{authorize_view, label, reg};
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
    /// Its CLI marked it the turn's final answer (Claude Code's `end_turn`, Codex's
    /// `final_answer` phase): the report, said so rather than guessed (`report_of`).
    #[serde(skip)]
    pub final_answer: bool,
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
    pub rest: usize,
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
    /// A report read's report was picked by length, its CLI having marked no
    /// final answer (`said::report_guessed`).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub report_guessed: bool,
    /// Of a report read with `rest`: a file holding all it said, report first.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// The answer owed for this report was collected (`ack`): no notice follows.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub acked: bool,
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
/// that answer, as a `wait` would. `report` reads the report alone (`report_read`).
#[allow(clippy::too_many_arguments)]
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
    let mut rest = 0;
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
            rest = messages.len() - keep;
            front += rest;
            messages.drain(..rest);
        } else {
            let keep = fit(&mut messages.iter().map(size));
            rest = messages.len() - keep;
            back = rest;
            if rest > 0 {
                messages.truncate(keep);
                if let Some(p) = &path {
                    cursor = cursor_after(p, from, cursor, found, keep, &messages_at);
                }
            }
        }
        for m in messages.iter_mut() {
            if let Some(cut) = cut_to_max(&m.text) {
                m.text = cut;
                text_cut = true;
            }
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
    let settles = whole && done && owed.is_some_and(|o| turns_ended > o.baseline && (!o.queued || queue_settled(&key, o, turns_ended)));
    // Ahead of the hand-over, which takes it off. With an answer still to come
    // it stays for the notice; with none owed this read is all that tells it.
    let undelivered = undelivered_note(caller, id);
    if undelivered.is_some() && lost_whole(caller, id) {
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
        rest,
        state,
        ask,
        turns_ended,
        screen: with_screen.then(|| screen_of(pty, id)).flatten(),
        undelivered,
        report_guessed: false,
        file: None,
        acked: false,
    })
}

/// The report of what the session said since `since`, else since the caller
/// last asked it something, else of its last turn that said anything: the one
/// message a notice's answer file opens with (`said::report_of`), for a caller
/// that wants no more of it. The rest it said is counted in `rest`. Not the
/// answer collected: a notice owed still comes, its file holding the whole.
///
/// Read as any read is: the cursor handed back is where the reader stopped,
/// before a line still being written; a report past MAX_TEXT is cut as any
/// message is (`cut_to_max`); a cursor past the transcript's end reads it from
/// the top. An explicit `since` that finds nothing is nothing: only a read
/// with no cursor of its own takes, as a notice does (`said::replies_since`),
/// the last word of a conversation that went on in a transcript of its own.
fn report_read(caller: &str, id: &str, provider: Provider, since: Option<u64>) -> Reading {
    let pty = crate::pty::manager();
    let owed = reg().lock().unwrap_or_else(|e| e.into_inner()).owed.get(&(caller.to_string(), id.to_string())).map(|o| o.cursor);
    let (path, moved) = transcript_seen(pty, id);
    let past_end = |at: u64| path.as_ref().is_some_and(|p| std::fs::metadata(p).is_ok_and(|m| at > m.len()));
    let from = match since {
        Some(at) if past_end(at) => 0,
        Some(at) => moved.unwrap_or(at),
        None => moved.or(owed).unwrap_or(0),
    };
    let read_at = |at: u64| -> Messages {
        match &path {
            Some(p) => match provider {
                Provider::Claude => crate::claude::session::messages_from(p, at, false),
                Provider::Codex => crate::codex::session::messages_from(p, at),
            },
            None => (Vec::new(), at),
        }
    };
    let (mut found, cursor) = read_at(from);
    if found.is_empty() && since.is_none() && from > 0 {
        found = read_at(0).0.into_iter().last().into_iter().collect();
    }
    let replies = crate::agents::said::said_of(found);
    let report_guessed = crate::agents::said::report_guessed(&replies);
    // The rest it said is a file away, report first, for a caller that wants it.
    let file = (replies.len() > 1)
        .then(|| crate::agents::said::note_file(id, &crate::agents::said::answer_text(&replies)))
        .flatten()
        .map(|p| p.to_string_lossy().into_owned());
    let messages: Vec<Message> = crate::agents::said::report_text(&replies)
        .map(|text| Message { role: "assistant".into(), text: cut_to_max(&text).unwrap_or(text), after_cli: false, final_answer: false })
        .into_iter()
        .collect();
    let (state, ask) = state_of(id);
    Reading {
        transcript_missing: messages.is_empty() && !transcript_found(pty, id),
        rest: replies.len().saturating_sub(messages.len()),
        messages,
        cursor,
        state,
        ask,
        turns_ended: crate::activity::turn_state(id).map(|t| t.turns_ended).unwrap_or(0),
        screen: None,
        undelivered: None,
        report_guessed,
        file,
        acked: false,
    }
}

/// `luna_report_get`: the report of what the session said (`report_read`), as a
/// look at it that starts its autoDelete count over (`looked`). With `ack`, a
/// report of all of an owed answer, its turn over, collects that answer as a
/// whole read would, so no notice follows; whatever else, it stays owed, and
/// `acked` says which. A cancel after the response puts the debt back.
pub fn report_get(caller: &str, id: &str, since: Option<u64>, ack: bool, call: &crate::mcp::Call) -> Result<Reading, String> {
    let known = authorize_view(caller, id)?;
    let ts = crate::activity::turn_state(id);
    let mut reading = report_read(caller, id, known.provider, since);
    looked(id);
    if !ack {
        return Ok(reading);
    }
    let key = (caller.to_string(), id.to_string());
    let owed = reg().lock().unwrap_or_else(|e| e.into_inner()).owed.get(&key).copied();
    let turns_ended = ts.map(|t| t.turns_ended).unwrap_or(0);
    let done = ts.is_none_or(|t| t.turn == crate::activity::Turn::Idle && !t.children_busy);
    // All of it: read from where the question found the transcript, its turn
    // over, the session's own word last.
    let whole = owed.is_some_and(|o| {
        since.is_none_or(|at| at <= o.cursor)
            && turns_ended > o.baseline
            && (!o.queued || queue_settled(&key, o, turns_ended))
            && !reading.messages.is_empty()
    });
    if whole && done && call.commit(hand_over(caller, id, turns_ended, None)) {
        reading.acked = true;
    }
    Ok(reading)
}

/// A message's text cut to MAX_TEXT characters, saying how much more there is
/// and where it is whole; None when it fits.
fn cut_to_max(text: &str) -> Option<String> {
    let n = text.chars().count();
    (n > MAX_TEXT).then(|| {
        let more = n - MAX_TEXT;
        format!(
            "{}\n\n[cut: {more} more characters; the transcript luna_session_get names has it whole]",
            text.chars().take(MAX_TEXT).collect::<String>()
        )
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
    use crate::agents::owed::owe_from;
    use crate::agents::registry::{reg, register, session_exited};
    use crate::agents::teardown::forget;

    /// `luna_report_get` reads as a read does: the reader's cursor, never past a
    /// line still being written; a long report cut at MAX_TEXT; an explicit
    /// cursor that finds nothing finds nothing.
    #[test]
    fn a_report_read_keeps_to_the_reader_its_limit_and_its_cursor() {
        let id = "rr-cursor";
        let path = std::env::temp_dir().join(format!("luna-report-read-{}.jsonl", std::process::id()));
        let line = |role: &str, text: &str| {
            let content = if role == "user" { serde_json::json!(text) } else { serde_json::json!([{ "type": "text", "text": text }]) };
            format!("{}\n", serde_json::json!({ "type": role, "message": { "role": role, "content": content } }))
        };
        let long = format!("REPORT {}", "x".repeat(MAX_TEXT + 500));
        let whole = format!("{}{}{}", line("user", "Do it."), line("assistant", "Working."), line("assistant", &long));
        let half = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"half"#;
        std::fs::write(&path, format!("{whole}{half}")).unwrap();
        transcripts().lock().unwrap().insert(id.to_string(), path.clone());

        let r = report_read("rr-caller", id, Provider::Claude, None);
        assert_eq!(r.messages.len(), 1);
        assert!(r.messages[0].text.starts_with("REPORT "), "the report");
        assert!(r.messages[0].text.contains("[cut: 507 more characters;"), "cut at MAX_TEXT like any read");
        assert_eq!(r.rest, 1, "the other message it said");
        assert_eq!(r.cursor, whole.len() as u64, "before the line still being written, not the file's end");

        // From that cursor nothing is there yet: nothing, not an older message.
        let r = report_read("rr-caller", id, Provider::Claude, Some(whole.len() as u64));
        assert!(r.messages.is_empty(), "{:?}", r.messages);
        assert_eq!(r.cursor, whole.len() as u64);

        // The line finished: it is the next read's.
        std::fs::write(&path, format!("{whole}{}", line("assistant", "Pushed."))).unwrap();
        let r = report_read("rr-caller", id, Provider::Claude, Some(whole.len() as u64));
        assert_eq!(r.messages.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(), ["Pushed."]);

        transcripts().lock().unwrap().remove(id);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_cut_read_goes_on_where_the_kept_messages_end() {
        let path = std::env::temp_dir().join(format!("luna-cursor-after-{}.jsonl", std::process::id()));
        std::fs::write(&path, "a\nb\nc\n").unwrap();
        // One message a line, as the transcript readers count them.
        let messages_at = |at: u64| -> Messages {
            let text = std::fs::read_to_string(&path).unwrap();
            let rest = &text[(at as usize).min(text.len())..];
            (rest.lines().map(|l| Message { role: "assistant".into(), text: l.into(), after_cli: false, final_answer: false }).collect(), text.len() as u64)
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
        register("lr-parent", Provider::Claude, &dir, &dir, None, true, serde_json::json!({}));
        crate::background::test_helper("lr-parent", &dir, None, false);
        register("lb-child", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("lr-parent"), false, serde_json::json!({}));
        crate::background::take_up_for_test("lb-child", "C:\\p", 0, &[], Some("do the whole job"));
        owe_from("lr-parent", "lb-child", 0, 0);
        let key = ("lr-parent".to_string(), "lb-child".to_string());
        let call = crate::mcp::Call::new();
        let note = read("lr-parent", "lb-child", None, None, false, false, &call).unwrap().undelivered.unwrap();
        assert!(note.contains("never got started"), "{note}");
        assert!(!reg().lock().unwrap().owed.contains_key(&key), "all there was: no notice is to say it again");
        assert_eq!(read("lr-parent", "lb-child", None, None, false, false, &crate::mcp::Call::new()).unwrap().undelivered, None);
        call.cancel();
        assert!(reg().lock().unwrap().owed.contains_key(&key), "payload and debt together");
        crate::background::remove("lb-child");
        forget("lb-child");
        crate::background::gone_for_test("lr-parent");
        forget("lr-parent");
    }
}
