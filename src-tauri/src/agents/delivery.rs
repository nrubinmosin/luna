//! Typing into a terminal: `luna_send`, one writer per terminal at a time
//! (`TypingInto`), a person's draft put aside and back, and text held while
//! someone types (`hold`, `deliver_held`).

use crate::agents::answer::{answer_reach, press, Key, INTERRUPT_SETTLE};
use crate::agents::ask::{ask_of, live, state_of};
use crate::agents::lost::still_all_lost;
use crate::agents::notifier::NOTIFY_EVERY;
use crate::agents::owed::{owe_from, seen, Owed};
use crate::agents::persist::{core_up_ms, returning};
use crate::agents::privilege::{answerer, command_reach_now, plain_text};
use crate::agents::read::transcript_end;
use crate::agents::registry::{authorize_send, chat_name, is_mine, label, reg, Registry, DEV_CALLER};
use crate::agents::resume::{call_off_resume, mark_going_in, reconcile_held, resume_chats, resume_pending, unmark_going_in};
use crate::provider::Provider;
use crate::throttle::now_ms;
use serde::{Deserialize, Serialize};
use std::sync::{Condvar, Mutex, OnceLock};

/// Enter, sent after the text so the TUI sees a paste and then a keypress.
/// Both CLIs treat bytes arriving in a quick burst as one paste, and an Enter
/// inside the burst as a newline in it: at 80 ms Codex 0.157 left the text
/// sitting in its composer. Well past the burst window, then.
pub(super) const ENTER_GAP: std::time::Duration = std::time::Duration::from_millis(400);

/// If no turn has begun this long after Enter, Enter is pressed once more:
/// on a composer still holding the text it submits; on an empty one it is a
/// no-op in both CLIs.
const ENTER_RETRY: std::time::Duration = std::time::Duration::from_secs(3);

/// The same for text typed into a running session: a turn should begin
/// within a few samples of Enter.
const SEND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(12);

/// The activity sampler runs every 5 s; the probes here look between samples
/// so the answer comes as soon as the verdict is in.
pub(super) const POLL: std::time::Duration = std::time::Duration::from_millis(500);

/// The rendered screen as an agent gets it: the last lines of the terminal,
/// enough to read a dialog, not a transcript.
const SCREEN_LINES: usize = 40;

/// The same for `send`, whose text is held until then: a person who
/// stopped typing this long ago has likely paused.
pub(super) const SEND_TYPING_MS: u64 = 5_000;

/// How long a tool call waits for other text Luna is typing into the same
/// terminal: more than a notice takes to go in, a draft put aside and back
/// included, and its turn to start (SEND_TIMEOUT).
pub(super) const TYPING_WAIT: std::time::Duration = std::time::Duration::from_secs(25);

/// A notice or a send held this long by an unsent draft is announced on the
/// desktop, once: the draft may be forgotten, the text is not.
pub(super) const DRAFT_ALERT_MS: u64 = 60_000;

/// How long Claude Code gets to show a draft put aside, or back, after the
/// key that does it.
const STASH_WAIT: std::time::Duration = std::time::Duration::from_millis(1500);

/// A `send` waiting for a person to send or clear what they typed.
pub(super) struct Held {
    pub(super) caller: String,
    pub(super) id: String,
    pub(super) text: String,
    pub(super) since: u64,
    /// The desktop has been told (DRAFT_ALERT_MS).
    pub(super) told: bool,
    /// On its way in: found so by the next core, a core went during that.
    pub(super) going_in: Option<GoingIn>,
}

/// How a core after this one tells whether text being typed arrived: the CLI went
/// with the core, so what is in its conversation is what it took.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GoingIn {
    /// The start of it, words only, to look for in what the session was told.
    pub(super) needle: String,
    /// Where the transcript ended before it went in.
    pub(super) cursor: u64,
}

/// How much of a text is looked for: enough to tell it, in case its tail changed.
pub(super) const NEEDLE: usize = 200;

/// Text with its white space as one space each.
pub(super) fn words(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What to look for of `text` when `typed` is what goes in: itself, or if it went
/// in with more or went to a file (`spill`), what was typed.
pub(super) fn needle_of(typed: &str, text: &str) -> String {
    let (typed, text) = (words(typed), words(text));
    (if typed.contains(&text) { text } else { typed }).chars().take(NEEDLE).collect()
}

/// A terminal Luna is typing into, for as long as this is held, whatever it is
/// typing: a notice, a send, a held send, an answer. Text goes in as a paste and
/// an Enter 400 ms later, with a draft put aside first and brought back after,
/// and anything else typed in between lands in it or breaks the draft's return.
/// Held until the turn the text starts has begun, which a notice does off the
/// notifier's loop: one slow caller is not to keep the others waiting.
pub(super) struct TypingInto(String);

impl TypingInto {
    pub(super) fn all() -> &'static (Mutex<std::collections::HashSet<String>>, Condvar) {
        static T: OnceLock<(Mutex<std::collections::HashSet<String>>, Condvar)> = OnceLock::new();
        T.get_or_init(Default::default)
    }

    /// The terminal if nothing else is typing into it now.
    pub(super) fn claim(id: &str) -> Option<TypingInto> {
        Self::all().0.lock().unwrap_or_else(|e| e.into_inner()).insert(id.to_string()).then(|| TypingInto(id.to_string()))
    }

    /// The terminal once what is typing into it is done, for a tool call that
    /// has text to put in now; None if that takes longer than `timeout`.
    pub(super) fn wait_for(id: &str, timeout: std::time::Duration) -> Option<TypingInto> {
        let (m, cv) = Self::all();
        let deadline = std::time::Instant::now() + timeout;
        let mut typing = m.lock().unwrap_or_else(|e| e.into_inner());
        while !typing.insert(id.to_string()) {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return None;
            }
            typing = cv.wait_timeout(typing, left).unwrap_or_else(|e| e.into_inner()).0;
        }
        Some(TypingInto(id.to_string()))
    }

    pub(super) fn under_way(id: &str) -> bool {
        Self::all().0.lock().unwrap_or_else(|e| e.into_inner()).contains(id)
    }
}

impl Drop for TypingInto {
    fn drop(&mut self) {
        let (m, cv) = Self::all();
        m.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.0);
        cv.notify_all();
    }
}

/// The screen says the CLI did not know the slash command `text` opens with.
/// Claude Code answers an unknown one on the spot and starts no turn, so
/// there is no start to wait out.
pub(super) fn unknown_command(pty: &crate::pty::PtyManager, id: &str, text: &str) -> bool {
    let Some(cmd) = text.trim_start().strip_prefix('/').and_then(|rest| rest.split_whitespace().next()) else {
        return false;
    };
    pty.screen(id).is_some_and(|s| s.contains(&format!("Unknown command: /{cmd}")))
}

/// The last lines of a session's terminal, for a caller that has to see
/// what a helper is stuck on.
pub(super) fn screen_of(pty: &crate::pty::PtyManager, id: &str) -> Option<String> {
    // A helper with no terminal has what it did instead.
    if crate::background::has(id) {
        return crate::background::screen(id, SCREEN_LINES);
    }
    let full = pty.screen(id)?;
    let lines: Vec<&str> = full.lines().collect();
    let from = lines.len().saturating_sub(SCREEN_LINES);
    Some(lines[from..].join("\n"))
}

/// True once the session has begun a turn — or finished one — since
/// `after_turn` turns had ended.
fn has_started(id: &str, after_turn: u32) -> bool {
    use crate::activity::Turn;
    // A permission prompt is inside a turn: one that comes up before the
    // first sample has seen the session busy is still a start.
    crate::activity::turn_state(id).map(|t| matches!(t.turn, Turn::Busy | Turn::Waiting) || t.turns_ended > after_turn).unwrap_or(false)
}

/// True while the CLI has not drawn anything yet: alive, no turn begun or
/// over, and a blank terminal. Claude Code syncs the org's skills and plugins
/// and makes its worktree before it draws a thing — minutes on a bad day,
/// with the session looking dead all along. The opening prompt is a CLI
/// argument and waits for it intact; text typed now would be lost. Of this
/// process's turns only: the count goes on across respawns, and one started again
/// after a core's restart is as blank as a new one.
pub(super) fn still_starting(pty: &crate::pty::PtyManager, id: &str) -> bool {
    use crate::activity::Turn;
    if crate::background::has(id) {
        return false;
    }
    let Some(p) = pty.probe(id) else { return false };
    !crate::activity::turn_state(id).is_some_and(|t| matches!(t.turn, Turn::Busy | Turn::Waiting))
        && crate::activity::turns_of_process(id, p.spawned_at_ms) == 0
        && pty.screen(id).is_some_and(|s| s.trim().is_empty())
}

/// Polls until the session starts working on `text` (see `has_started`),
/// turns it down as an unknown command, exits, or `timeout` passes. Returns
/// whether it started.
pub(super) fn await_start(pty: &crate::pty::PtyManager, id: &str, text: &str, after_turn: u32, timeout: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if has_started(id, after_turn) {
            return true;
        }
        if !pty.alive(id) || unknown_command(pty, id, text) || std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(POLL);
    }
}

/// Past this many characters a prompt or message goes to a visible session
/// as a file it is told to read: a command line holds some 32 thousand on
/// Windows, and a Codex composer takes typed text one key event at a time.
const LONG_TEXT: usize = 8000;

/// Where long prompts and messages are put for the sessions they are for.
/// A Claude Code session an agent spawned gets it as an `--add-dir`.
pub fn notes_dir() -> std::path::PathBuf {
    crate::paths::data_dir().join("agents")
}

/// `text` as it is to be typed or passed: itself, or past LONG_TEXT a line
/// that points at a file holding it.
pub(super) fn spill(text: &str) -> String {
    let n = text.chars().count();
    if n <= LONG_TEXT {
        return text.to_string();
    }
    let dir = notes_dir();
    let path = dir.join(format!("{}.md", crate::background::mint_id()));
    if std::fs::create_dir_all(&dir).and_then(|()| crate::secret::write_private(&path, text.as_bytes())).is_err() {
        return text.to_string();
    }
    format!(
        "This message is {n} characters long, so Luna put it in a file: {}. Read the whole file first and take it as this message.",
        path.display()
    )
}

/// `spill` of text the caller sends `id`, already `as_sent`: past LONG_TEXT the
/// line pointing at the file is what the session reads first, and it says
/// whose the message is as the text would have (`as_sent`).
pub(super) fn spill_sent(caller: &str, id: &str, sent: &str) -> String {
    let typed = spill(sent);
    if typed == sent {
        return typed;
    }
    as_sent(caller, id, &typed)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sent {
    pub ok: bool,
    /// Where the session stood when the text went in (see `state_of`).
    pub state_before: &'static str,
    /// It was working: the text joins the turn in flight or is taken up
    /// after it (a background Codex helper: always after), and `started` says
    /// nothing yet.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub queued: bool,
    /// Of `queued`, a background Claude Code helper's stdin has it: it takes it in
    /// at its next tool boundary, and this turn's end is the answer.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub joined: bool,
    /// The turn it was working on was stopped for this text (`interrupt`).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub interrupted: bool,
    /// Not typed in yet: a person is typing there, or left a draft Luna
    /// cannot put aside. Luna types it in once they send or clear it; the
    /// answer comes as a notice, or to a `wait`, which waits for that too.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub held: bool,
    /// A turn began on the text within SEND_TIMEOUT. False on a session that
    /// was not working means the text did not become a prompt: `screen`
    /// shows where it went.
    pub started: bool,
    /// The `afterTurn` for the `wait` that collects the reply: turns over
    /// before the text went in, the one it is queued behind counted for a
    /// background helper whose queue holds the text until that turn ends.
    pub turns_ended: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screen: Option<String>,
}

/// Types text into a session and presses Enter after it; returns whether a
/// turn began. `hold` has kept a person's keys back since before the last look at
/// the session, and does until the Enter: one landing in between would go out with
/// the text. They go on after it, into an empty composer.
pub(super) fn type_in(
    pty: &crate::pty::PtyManager,
    id: &str,
    text: &str,
    turn_before: Option<crate::activity::Turn>,
    baseline: u32,
    hold: crate::pty::KeyHold,
) -> Result<bool, String> {
    // Every text Luna types goes through here or `stash_and_type`: none of it
    // as keys, whoever wrote it and whenever it was kept.
    plain_text(text)?;
    let keys_at = pty.typed_at_ms(id);
    pty.paste(id, text)?;
    std::thread::sleep(ENTER_GAP);
    pty.write(id, b"\r")?;
    drop(hold);
    // Mid-turn the text is queued by the CLI and nothing here can tell when
    // it is taken up; only an idle session is expected to start now.
    if turn_before == Some(crate::activity::Turn::Busy) || await_start(pty, id, text, baseline, ENTER_RETRY) {
        return Ok(true);
    }
    // An Enter the CLI missed gets another, unless a person has typed since, which
    // it would send. Their next key waits while this looks.
    let Some(again) = pty.hold_keys(id) else { return Ok(true) };
    if pty.typed_at_ms(id) == keys_at && pty.write(id, b"\r").is_err() {
        // The text went in and the session is gone since — `/exit` does
        // exactly that. It was taken; there is just no turn to wait for.
        return Ok(true);
    }
    drop(again);
    Ok(await_start(pty, id, text, baseline, SEND_TIMEOUT.saturating_sub(ENTER_RETRY)))
}

/// What text sent to a session that is not the caller's opens with: whose words
/// they are, which that session would otherwise take for its user's.
pub(super) fn from_line(caller: &str) -> String {
    format!("[From session {caller} via Luna]")
}

/// Text as it goes into `id`: as written, into the caller's own helper, which
/// knows its parent's voice; into any other session after `from_line`, on the
/// same line, so the transcript keeps whose it was too.
pub(super) fn as_sent(caller: &str, id: &str, text: &str) -> String {
    if is_mine(caller, id) {
        text.to_string()
    } else {
        format!("{} {text}", from_line(caller))
    }
}

/// Types text into the session, presses Enter after it, and reports whether
/// a turn began. With `interrupt` the turn in flight, and any prompt it is
/// stopped on, is cut short first (Esc; a background helper's process
/// stopped), and the text is the next turn rather than one queued after it:
/// only in the caller's own helpers, as stopping a turn ends work and dismisses
/// a question that is someone else's to answer.
pub fn send(caller: &str, id: &str, text: &str, interrupt: bool) -> Result<Sent, String> {
    let known = authorize_send(caller, id)?;
    let mine = is_mine(caller, id);
    if interrupt && !mine {
        return Err(format!(
            "interrupt only stops sessions you spawned, directly or through a helper; {id} is not one. Send without \
             interrupt: the text is queued after its turn"
        ));
    }
    let pty = crate::pty::manager();
    if !live(pty, id) {
        return Err(format!("session {id} is not running"));
    }
    // Before anything is read of it: what is going in now may be a notice, and
    // the session busy on it by the time this text's turn comes. Not there in
    // time, the text is held like any other (`hold`), and goes in when it can.
    // A background helper has no terminal: its queue takes turns by itself.
    plain_text(text)?;
    command_reach_now(caller, id, text)?;
    // Checked as written: a `/` or `!` the line would hide is refused all the same.
    let text = &as_sent(caller, id, text);
    let terminal = if crate::background::has(id) { None } else { TypingInto::wait_for(id, TYPING_WAIT) };
    let (state_before, ask) = state_of(id);
    if let Some(a) = ask.as_ref().filter(|a| a.blocking && !interrupt) {
        let way_out = if mine { ", or send with interrupt: true to dismiss it" } else { "; its own chat or the user may" };
        return Err(format!("session {id} is on a {}: typed text would land in its dialog. Answer it with luna_answer{way_out}", a.kind));
    }
    // Esc is to dismiss a dialog before the text goes in, but text that comes
    // while it is still up lands in it: past a permission or a CLI's own list,
    // only a caller whose yes it could be (`answer_reach`). A background helper
    // takes text as a message, never as keys.
    if let Some(a) = ask.as_ref().filter(|a| a.blocking && terminal.is_some()) {
        if a.kind == "permission" || a.questions.iter().any(|q| q.list_at.is_some()) {
            let (own, approver) = answerer(caller, id);
            answer_reach(own, approver, known.provider, Some(a), Some(true), None)
                .map_err(|e| format!("session {id} is on a {}, which text sent with interrupt could land in. {e}", a.kind))?;
        }
    }
    let mut before = crate::activity::turn_state(id);
    let mut baseline = before.map(|t| t.turns_ended).unwrap_or(0);
    if crate::background::has(id) {
        // Taken before the text goes in: a turn it starts may be over, and
        // written, by the time the call returns.
        let cursor = transcript_end(id);
        use crate::background::Placed;
        let (placed, interrupted) = if interrupt {
            let i = crate::background::interrupt(id, text, Some(caller))?;
            (if i.queued { Placed::Queued } else { Placed::Started }, i.stopped)
        } else {
            (crate::background::send(id, text, Some(caller))?, false)
        };
        let (queued, joined) = (placed != Placed::Started, placed == Placed::Joined);
        // The turn it was on ends before the one this text starts, and is
        // counted: the answer ends the turn after that. Text that joined that
        // turn is answered by its end.
        if placed == Placed::Queued || interrupted {
            baseline += 1;
        }
        seen(caller, id, baseline);
        owe_from(caller, id, baseline, cursor);
        return Ok(Sent {
            ok: true,
            state_before,
            queued,
            joined,
            interrupted,
            held: false,
            started: !queued,
            turns_ended: baseline,
            screen: None,
        });
    }
    let busy = |t: Option<crate::activity::TurnState>| {
        t.is_some_and(|t| matches!(t.turn, crate::activity::Turn::Busy | crate::activity::Turn::Waiting))
    };
    let interrupted = interrupt && busy(before) && terminal.is_some();
    if interrupted {
        press(pty, id, &[Key::Seq("\x1b")])?;
        let deadline = std::time::Instant::now() + INTERRUPT_SETTLE;
        while busy(crate::activity::turn_state(id)) && std::time::Instant::now() < deadline {
            std::thread::sleep(POLL);
        }
        before = crate::activity::turn_state(id);
        baseline = before.map(|t| t.turns_ended).unwrap_or(0);
    }
    if still_starting(pty, id) {
        return Err(format!(
            "session {id} has not come up yet (its terminal is still blank), so typed text would be lost; \
             an opening prompt from luna_spawn is held and runs once it is up — luna_wait(turn_done) for the reply"
        ));
    }
    // What a caller sends now is what it goes on with.
    call_off_resume(id, "a session sent it text first");
    // Text typed now would land in the middle of what a person is typing, or
    // go out with a draft they left: it waits for them (`deliver_held`),
    // unless the draft can be put aside (`type_text`).
    let typing = pty.typed_ago_ms(id).is_some_and(|ms| ms < SEND_TYPING_MS);
    let typed =
        if typing || terminal.is_none() { None } else { put_in(caller, id, &spill_sent(caller, id, text), before, baseline, interrupted)? };
    let Some(sent) = typed else {
        hold(caller, id, text);
        return Ok(Sent {
            ok: true,
            state_before,
            queued: false,
            joined: false,
            interrupted,
            held: true,
            started: false,
            turns_ended: baseline,
            screen: None,
        });
    };
    Ok(Sent { state_before, interrupted, ..sent })
}

/// Types a caller's text into a visible session and owes the caller the
/// answer; None when a person's draft is in the way and stays there.
fn put_in(
    caller: &str,
    id: &str,
    text: &str,
    before: Option<crate::activity::TurnState>,
    baseline: u32,
    interrupted: bool,
) -> Result<Option<Sent>, String> {
    // Held text too comes through here, typed long after it was taken.
    command_reach_now(caller, id, text)?;
    let pty = crate::pty::manager();
    // After an Esc the TUI is between turns whatever the sampler has caught
    // up with: the text is a prompt of its own, not one queued.
    let turn_before = if interrupted { None } else { before.map(|t| t.turn) };
    let queued = turn_before == Some(crate::activity::Turn::Busy);
    // Before the text goes in: `type_in` waits for the turn to start, and a
    // short one is over, and written, by then.
    let cursor = transcript_end(id);
    let Some(started) = type_text(pty, id, text, turn_before, baseline)? else { return Ok(None) };
    let started = started && !queued;
    if turn_before == Some(crate::activity::Turn::Waiting) {
        reg().lock().unwrap_or_else(|e| e.into_inner()).answered.insert((caller.to_string(), id.to_string()), now_ms());
    }
    let screen = (!started && !queued).then(|| screen_of(pty, id)).flatten();
    // The count before the text went in: the reply to it ends the next
    // turn, which the caller has not seen even if it is already over. Typed
    // mid-turn, that is the turn running now or the one the CLI starts on
    // the text after it, whichever the CLI makes of it (QUEUED_GRACE_MS).
    seen(caller, id, baseline);
    owe_from(caller, id, baseline, cursor);
    if queued {
        if let Some(o) = reg().lock().unwrap_or_else(|e| e.into_inner()).owed.get_mut(&(caller.to_string(), id.to_string())) {
            o.queued = true;
        }
    }
    Ok(Some(Sent { ok: true, state_before: "", queued, joined: false, interrupted, held: false, started, turns_ended: baseline, screen }))
}

/// Types text in as a prompt (`type_in`) — around a person's draft where
/// there is one and the CLI can put it aside (`stash_and_type`). Ok(None):
/// a draft is in the way, and nothing was typed.
pub(super) fn type_text(
    pty: &crate::pty::PtyManager,
    id: &str,
    text: &str,
    turn_before: Option<crate::activity::Turn>,
    baseline: u32,
) -> Result<Option<bool>, String> {
    let hold = pty.hold_keys(id).ok_or("no such session")?;
    type_held(pty, id, text, turn_before, baseline, hold)
}

/// `type_text` with a person's keys held back already: what is to look at the
/// session once more first takes them before it looks.
pub(super) fn type_held(
    pty: &crate::pty::PtyManager,
    id: &str,
    text: &str,
    turn_before: Option<crate::activity::Turn>,
    baseline: u32,
    hold: crate::pty::KeyHold,
) -> Result<Option<bool>, String> {
    if !pty.has_draft(id) {
        return type_in(pty, id, text, turn_before, baseline, hold).map(Some);
    }
    // Codex has no way to put a draft aside: one that leaves the
    // composer by its keys (a line cut, a whole buffer selected) does not
    // come back with pastes and images in it.
    if pty.provider(id) != Some(Provider::Claude) {
        return Ok(None);
    }
    stash_and_type(pty, id, text, turn_before, baseline, hold)
}

/// Claude Code puts a draft aside on Ctrl+S and brings it back, pastes and
/// images and all, when the next prompt goes ("Draft restored"): Luna's text
/// goes in between, the person's keys held back meanwhile. Not when a draft
/// is aside already — another Ctrl+S on text replaces it, and it is lost —
/// nor when the composer will not read or does not empty: Ok(None), and
/// the draft is where it was.
fn stash_and_type(
    pty: &crate::pty::PtyManager,
    id: &str,
    text: &str,
    turn_before: Option<crate::activity::Turn>,
    baseline: u32,
    hold: crate::pty::KeyHold,
) -> Result<Option<bool>, String> {
    plain_text(text)?;
    if !pty.composer(id).is_some_and(|c| !c.text.is_empty() && !c.stashed) {
        return Ok(None);
    }
    let count = pty.draft_count(id);
    let until = |timeout: std::time::Duration, ok: &dyn Fn(crate::pty::Composer) -> bool| {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if pty.composer(id).is_some_and(ok) {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    };
    pty.write(id, b"\x13")?;
    if !until(STASH_WAIT, &|c| c.text.is_empty() && c.stashed) {
        // Gone from the composer with no word of the stash: Ctrl+S again
        // brings a stashed draft back, and does nothing to an empty one.
        if pty.composer(id).is_some_and(|c| c.text.is_empty()) {
            pty.write(id, b"\x13")?;
        }
        crate::log::warn("agents", &format!("{}: Ctrl+S did not put the draft aside; the text waits", label(id)));
        return Ok(None);
    }
    pty.paste(id, text)?;
    std::thread::sleep(ENTER_GAP);
    pty.write(id, b"\r")?;
    // The draft coming back is the CLI taking the text.
    let back = |c: crate::pty::Composer| !c.stashed;
    let mut restored = until(ENTER_RETRY, &back);
    // Luna's own text still sitting there, word for word, takes another
    // Enter; anything else may be the draft back, its mark not gone yet.
    if !restored && pty.composer(id).is_some_and(|c| c.stashed && words(&c.text) == words(text)) {
        pty.write(id, b"\r")?;
        restored = until(ENTER_RETRY, &back);
    }
    if !restored && pty.composer(id).is_some_and(|c| c.stashed && c.text.is_empty()) {
        pty.write(id, b"\x13")?;
        restored = until(STASH_WAIT, &back);
    }
    if restored {
        pty.set_draft(id, count);
    } else {
        crate::log::warn("agents", &format!("{}: the draft Luna put aside did not come back", label(id)));
    }
    drop(hold);
    // A turn as `type_in` judges one, without its second Enter: that one
    // would send the draft.
    let started =
        turn_before == Some(crate::activity::Turn::Busy) || await_start(pty, id, text, baseline, SEND_TIMEOUT.saturating_sub(ENTER_RETRY));
    Ok(Some(started))
}

/// Puts a caller's text by for the notifier to type in (`deliver_held`).
fn hold(caller: &str, id: &str, text: &str) {
    crate::log::info(
        "agents",
        &format!("send from {} to {} held: the user is typing or has a draft, or other text is going in", label(caller), label(id)),
    );
    reg().lock().unwrap_or_else(|e| e.into_inner()).held.push(Held {
        caller: caller.to_string(),
        id: id.to_string(),
        text: text.to_string(),
        since: now_ms(),
        told: false,
        going_in: None,
    });
    // "Do not send it again" goes back with the result: it must hold.
    crate::background::save();
}

/// What Luna holds for a chat: sends waiting to go into it, and notices a
/// draft in it keeps back. For the chat row.
pub fn held_count(id: &str) -> usize {
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    r.held.iter().filter(|h| h.id == id).count() + r.notices_held.get(id).copied().unwrap_or(0)
}

/// Looks for held sends to type in, on a thread of its own: typing one waits
/// for its turn to start, which is not to keep the notices back.
pub(super) fn held_sender() {
    loop {
        std::thread::sleep(NOTIFY_EVERY);
        let pty = crate::pty::manager();
        resume_chats(pty);
        deliver_held(pty);
    }
}

/// Types in the sends held for people at their keys (`hold`), each once its
/// session is clear, in the order they came; tells the caller of a session
/// that went before its text could, and the desktop of text a draft has held
/// for long.
pub(super) fn deliver_held(pty: &crate::pty::PtyManager) {
    // What a core went in the middle of typing, its session back; one carrying on
    // sees to it itself (`resume_chat`).
    let marked: std::collections::HashSet<String> =
        reg().lock().unwrap_or_else(|e| e.into_inner()).held.iter().filter(|h| h.going_in.is_some()).map(|h| h.id.clone()).collect();
    for id in marked {
        if pty.alive(&id) && !resume_pending(&id) {
            reconcile_held(pty, &id);
        }
    }
    let held: Vec<(String, String, String, u64, bool)> = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        r.held.iter().map(|h| (h.caller.clone(), h.id.clone(), h.text.clone(), h.since, h.told)).collect()
    };
    let mut stuck: std::collections::HashSet<String> = Default::default();
    for (caller, id, text, since, told) in held {
        let drop_it = |r: &mut Registry| {
            if let Some(i) = r.held.iter().position(|h| h.caller == caller && h.id == id && h.since == since) {
                r.held.remove(i);
            }
        };
        // Its parent gets it back with what it first asked (`undelivered_for`), the
        // two being no use apart: typed in, this would start it on the wrong foot. Not
        // once that parent has sent it something that went in: it has started on that.
        if still_all_lost(pty, &caller, &id)
            && reg().lock().unwrap_or_else(|e| e.into_inner()).owed.contains_key(&(caller.clone(), id.clone()))
        {
            continue;
        }
        if !live(pty, &id) {
            // Kept across a core: it goes in once the window has started it.
            if reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(&id).is_some_and(|k| returning(k, core_up_ms())) {
                continue;
            }
            drop_it(&mut reg().lock().unwrap_or_else(|e| e.into_inner()));
            if caller != DEV_CALLER {
                let ended = crate::activity::turn_state(&id).map_or(0, |t| t.turns_ended);
                let mut o = Owed::from(ended, transcript_end(&id));
                o.lost = true;
                reg().lock().unwrap_or_else(|e| e.into_inner()).owed.insert((caller.clone(), id.clone()), o);
            }
            continue;
        }
        // It goes in with what carries the chat on (`resume_chat`).
        if resume_pending(&id) {
            continue;
        }
        // One at a time per session, so they go in the order they came.
        if stuck.contains(&id) {
            continue;
        }
        let blocked = ask_of(&id).is_some_and(|a| a.blocking)
            || still_starting(pty, &id)
            || pty.typed_ago_ms(&id).is_some_and(|ms| ms < SEND_TYPING_MS);
        let before = crate::activity::turn_state(&id);
        let baseline = before.map(|t| t.turns_ended).unwrap_or(0);
        let typing = if blocked { None } else { TypingInto::claim(&id) };
        let typed = if typing.is_some() {
            let going = spill_sent(&caller, &id, &text);
            mark_going_in(&id, &going, transcript_end(&id), Some((caller.as_str(), since)));
            put_in(&caller, &id, &going, before, baseline, false)
        } else {
            Ok(None)
        };
        if !matches!(typed, Ok(Some(_))) {
            unmark_going_in(&id);
        }
        drop(typing);
        match typed {
            Ok(Some(sent)) => {
                let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
                drop_it(&mut r);
                if let Some(o) = r.owed.get_mut(&(caller.clone(), id.clone())) {
                    o.late = true;
                }
                drop(r);
                // Or a core killed now would type it in again.
                crate::background::save();
                crate::log::info(
                    "agents",
                    &format!("held send from {} typed into {} (started {})", label(&caller), label(&id), sent.started),
                );
            }
            Ok(None) => {
                stuck.insert(id.clone());
                if !told && pty.has_draft(&id) && now_ms().saturating_sub(since) >= DRAFT_ALERT_MS {
                    if let Some(h) =
                        reg().lock().unwrap_or_else(|e| e.into_inner()).held.iter_mut().find(|h| h.id == id && h.since == since)
                    {
                        h.told = true;
                    }
                    let chat = chat_name(&id).unwrap_or_else(|| id.clone());
                    let from = chat_name(&caller).unwrap_or_else(|| caller.clone());
                    crate::power::notify_chat(
                        &id,
                        "A message is waiting",
                        &format!(
                            "\"{chat}\" has an unsent draft, so Luna is holding a message from \"{from}\" until you send or clear it."
                        ),
                    );
                }
            }
            Err(e) => {
                stuck.insert(id.clone());
                crate::log::warn("agents", &format!("held send from {} to {} failed: {e}", label(&caller), label(&id)));
            }
        }
    }
}

/// Whether text `caller` sent to `id` is still held.
pub(super) fn is_held(caller: &str, id: &str) -> bool {
    reg().lock().unwrap_or_else(|e| e.into_inner()).held.iter().any(|h| h.caller == caller && h.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::privilege::command_reach_now;
    use crate::agents::registry::register;
    use crate::agents::teardown::forget;

    #[test]
    fn one_at_a_time_is_typed_into_a_session() {
        let first = TypingInto::claim("ty-chat").expect("nobody is typing into it");
        assert!(TypingInto::under_way("ty-chat"));
        assert!(TypingInto::claim("ty-chat").is_none(), "a notice and a held send would land in each other");
        assert!(TypingInto::claim("ty-other").is_some(), "another session is another matter");
        drop(first);
        assert!(!TypingInto::under_way("ty-chat"));
        assert!(TypingInto::claim("ty-chat").is_some());
    }

    #[test]
    fn a_tool_call_waits_its_turn_at_a_terminal() {
        use std::time::{Duration, Instant};
        let notice = TypingInto::claim("tw-chat").expect("nobody is typing into it");
        // As a send finds it with a notice going in.
        assert!(TypingInto::wait_for("tw-chat", Duration::from_millis(60)).is_none(), "not in time: it is held, not typed over the notice");
        let send = std::thread::spawn(|| {
            let started = Instant::now();
            let got = TypingInto::wait_for("tw-chat", Duration::from_secs(10)).expect("its turn comes");
            (started.elapsed(), got)
        });
        std::thread::sleep(Duration::from_millis(200));
        assert!(!send.is_finished(), "still behind the notice");
        assert!(TypingInto::claim("tw-chat").is_none(), "and nothing gets in between");
        drop(notice);
        let (waited, got) = send.join().unwrap();
        assert!(waited >= Duration::from_millis(150), "it went in after the notice, not during it");
        assert!(TypingInto::under_way("tw-chat"), "now it is the one typing");
        assert!(TypingInto::wait_for("tw-other", Duration::from_millis(1)).is_some(), "another terminal is another matter");
        drop(got);
        assert!(!TypingInto::under_way("tw-chat"));
    }

    #[test]
    fn an_interrupt_stops_only_the_callers_own() {
        let (claude, codex, none) = ("C:\\acc\\anthropic\\one", "C:\\acc\\openai\\two", serde_json::json!({}));
        let background = serde_json::json!({ "background": true });
        register("ir-me", Provider::Claude, "C:\\p", claude, None, true, none.clone());
        // A person's chats, one of each provider, and their helpers: one
        // working in the background and stopped on a question of its own.
        register("ir-chat", Provider::Claude, "C:\\p", claude, None, true, none.clone());
        register("ir-codex-chat", Provider::Codex, "C:\\p", codex, None, false, none.clone());
        register("ir-theirs", Provider::Claude, "C:\\p", claude, Some("ir-chat"), false, background.clone());
        crate::background::take_up_for_test("ir-theirs", "C:\\p", 1, &[], None);
        crate::background::set_running_for_test("ir-theirs");
        crate::background::hold_for_test("ir-theirs");
        register("ir-codex-theirs", Provider::Codex, "C:\\p", codex, Some("ir-codex-chat"), false, background);
        for id in ["ir-chat", "ir-codex-chat", "ir-theirs", "ir-codex-theirs"] {
            let e = send("ir-me", id, "drop that and do this", true).err().unwrap_or_else(|| panic!("{id} was interrupted"));
            assert!(e.contains("interrupt only stops sessions you spawned"), "{id}: {e}");
        }
        // Its turn goes on, and its question still waits for whoever owns it.
        assert!(crate::background::ask("ir-theirs").is_some(), "its question was dismissed");
        // Its own helper, either provider, is not refused for being someone else's
        // (here they are simply not running).
        register("ir-mine", Provider::Claude, "C:\\p", claude, Some("ir-me"), false, none.clone());
        register("ir-mine-codex", Provider::Codex, "C:\\p", codex, Some("ir-mine"), false, none);
        for id in ["ir-mine", "ir-mine-codex"] {
            let e = send("ir-me", id, "drop that and do this", true).err().unwrap();
            assert!(e.contains("is not running"), "{id}: {e}");
        }
        crate::background::remove("ir-theirs");
        for id in ["ir-mine-codex", "ir-mine", "ir-theirs", "ir-codex-theirs", "ir-chat", "ir-codex-chat", "ir-me"] {
            forget(id);
        }
    }

    #[test]
    fn text_to_a_session_not_the_callers_says_whose_it_is() {
        let (claude, none) = ("C:\\acc\\anthropic\\one", serde_json::json!({}));
        let background = serde_json::json!({ "background": true });
        register("ff-me", Provider::Claude, "C:\\p", claude, None, true, none.clone());
        register("ff-chat", Provider::Claude, "C:\\p", claude, None, true, none);
        register("ff-theirs", Provider::Claude, "C:\\p", claude, Some("ff-chat"), false, background.clone());
        register("ff-mine", Provider::Claude, "C:\\p", claude, Some("ff-me"), false, background);
        for id in ["ff-theirs", "ff-mine"] {
            crate::background::take_up_for_test(id, "C:\\p", 1, &[], None);
            crate::background::set_running_for_test(id);
        }
        assert!(send("ff-me", "ff-theirs", "the build is green", false).unwrap().queued);
        assert!(send("ff-me", "ff-mine", "the build is green", false).unwrap().queued);
        assert_eq!(crate::background::queued_for_test("ff-theirs"), ["[From session ff-me via Luna] the build is green"]);
        assert_eq!(crate::background::queued_for_test("ff-mine"), ["the build is green"], "its own helper knows its parent");
        // A command the line would hide is refused as written.
        assert!(send("ff-me", "ff-theirs", "/permissions", false).is_err());
        for id in ["ff-theirs", "ff-mine"] {
            crate::background::remove(id);
        }
        for id in ["ff-theirs", "ff-mine", "ff-chat", "ff-me"] {
            forget(id);
        }
    }

    #[test]
    fn a_long_message_points_at_its_file_in_the_senders_name() {
        let (account, none) = ("C:\\acc\\anthropic\\one", serde_json::json!({}));
        register("sp-me", Provider::Claude, "C:\\p", account, None, true, none.clone());
        register("sp-kid", Provider::Claude, "C:\\p", account, Some("sp-me"), false, none.clone());
        register("sp-chat", Provider::Claude, "C:\\p", account, None, false, none);
        let long = "word ".repeat(LONG_TEXT / 4);
        let file_of = |line: &str| {
            let after = line.split("put it in a file: ").nth(1).unwrap();
            after.split(". Read the whole file").next().unwrap().to_string()
        };
        // To a session not the caller's: the pointer opens as the message would.
        let theirs = spill_sent("sp-me", "sp-chat", &as_sent("sp-me", "sp-chat", &long));
        assert!(theirs.starts_with("[From session sp-me via Luna] This message is"), "{theirs}");
        let file = file_of(&theirs);
        assert!(std::fs::read_to_string(&file).unwrap().starts_with("[From session sp-me via Luna] word"));
        let _ = std::fs::remove_file(&file);
        // To its own helper, as before; a short one as it is.
        let mine = spill_sent("sp-me", "sp-kid", &as_sent("sp-me", "sp-kid", &long));
        assert!(mine.starts_with("This message is"), "{mine}");
        let _ = std::fs::remove_file(file_of(&mine));
        assert_eq!(spill_sent("sp-me", "sp-chat", "[From session sp-me via Luna] hi"), "[From session sp-me via Luna] hi");
        for id in ["sp-kid", "sp-chat", "sp-me"] {
            forget(id);
        }
    }

    #[test]
    fn a_command_is_checked_again_when_held_text_is_typed() {
        let none = serde_json::json!({ "permissionMode": "default" });
        let bypass = serde_json::json!({ "permissionMode": "bypassPermissions" });
        register("cr-asks", Provider::Claude, "C:\\p", "C:\\acc\\one", None, true, none.clone());
        register("cr-full", Provider::Claude, "C:\\p", "C:\\acc\\one", None, true, bypass);
        register("cr-kid", Provider::Codex, "C:\\p", "C:\\acc\\one", Some("cr-asks"), false, none.clone());
        register("cr-kid2", Provider::Codex, "C:\\p", "C:\\acc\\one", Some("cr-full"), false, none);
        // `put_in` types what `send` took and what was held: refused there
        // before anything reaches the terminal.
        let e = put_in("cr-asks", "cr-kid", "/permissions :workspace", None, 0, false).err().unwrap();
        assert!(e.contains("not a prompt"), "{e}");
        assert!(put_in("cr-full", "cr-kid", " !whoami", None, 0, false).err().unwrap().contains("not a prompt"), "not its helper");
        assert_eq!(command_reach_now("cr-full", "cr-kid2", "/permissions :workspace"), Ok(()));
        assert_eq!(command_reach_now("cr-asks", "cr-kid", "permissions, please"), Ok(()));
        for id in ["cr-kid", "cr-kid2", "cr-asks", "cr-full"] {
            forget(id);
        }
    }
}
