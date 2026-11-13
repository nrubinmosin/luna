//! Handoff (docs/adr/0020-session-self-handoff.md). Every person's chat with
//! Luna's tools is listed `luna_session_handoff`, a tool no other session is
//! offered, and it answers only while the chat is asked for a handoff: by
//! auto-handoff's threshold (`handoff` in its settings, in percent of its
//! context window), whose notice Luna types once a turn ends past it; by the
//! person, through the pane's Hand off now (`handoff_now`), whose request Luna
//! types once the chat is at rest; or by a move (moving.rs). Asked, the tool
//! without a file answers how, with the instructions Luna serves for it
//! (docs/adr/0022-content-luna-serves-at-its-own-trigger.md); with one, Luna
//! snapshots the file, waits for the chat to be at rest, types the CLI's own
//! reset — `/clear` (Claude Code), `/new` (Codex), fixed text — and then the
//! prompt that opens the next conversation from the snapshot and the facts
//! Luna knows. The chat's id, its MCP token, its helpers and what they owe it
//! stay as they are: the CLI's process goes on, and Luna follows it onto the
//! new conversation (read.rs `moved_to`). Notices to the chat and text sent to
//! it wait from the call until the prompt is in. Nothing switches unless the
//! session asks, and it asks only when asked to. No other session hears of any
//! of it: not the tool, not a guide topic, not the instructions.
//!
//! - `file`: the handoff checked and copied.
//! - `phase`: the switch, a machine whose every phase is saved.
//! - `store`: what is saved, `<data>/handoffs/handoffs.json`.
//! - `watch`: when the notices go.
//! - `facts`: what the notices, the tool's answer and the prompt say.
//! - `served`: the instructions served, what is kept of them, and what the
//!   chat's panel shows of them.
//! - `moving`: a chat moved to another account of its CLI, opened from a
//!   document the person confirmed (docs/adr/0023-moving-a-chat-to-another-account.md);
//!   `document` writes that document from the chat's records, `excerpts`
//!   reads them bounded, `gitfacts` asks git, `scrub` cleans it.

mod document;
mod excerpts;
mod facts;
mod file;
mod gitfacts;
mod lead;
mod moving;
mod phase;
mod scrub;
mod served;
mod store;
mod watch;

pub use lead::lead_taken_up;
pub use moving::{
    move_ask, move_build_from_records, move_cancel, move_confirm, move_facts, move_list, move_prepare, move_preview, move_step, moved_dir,
    opening_refusal, stopping_for_move, MOVE_TAG,
};
// Called off a lead's turn's thread, which tests do not start (background/events.rs).
#[cfg_attr(test, allow(unused_imports))]
pub use lead::lead_turn_ended;
pub(in crate::agents) use lead::HandedOff;
pub use served::{applied_of, HandoffApplied};

use crate::agents::delivery::{type_in, TypingInto, POLL, SEND_TYPING_MS, TYPING_WAIT};
use crate::agents::held::DRAFT_ALERT_MS;
use crate::agents::notifier::{ready_for_notice, NOTIFY_EVERY};
use crate::agents::read::transcript_path;
use crate::agents::registry::{chat_name, label, reg, DEV_CALLER};
use crate::provider::Provider;
use crate::throttle::now_ms;
use phase::{Before, Ended, Look, Phase, Phases, Refused, Step, Switch, Term, Typed};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

pub use facts::PROMPT_TAG;

/// The thresholds a chat may have, in percent of its context window. The
/// window offers from 10; lower is for a test that is not to spend a
/// conversation's worth of tokens getting there.
pub const THRESHOLD_MIN: u8 = 1;
pub const THRESHOLD_MAX: u8 = 90;

/// Where handoffs are kept: the record, and a folder of snapshots per chat.
pub(super) fn root() -> PathBuf {
    crate::paths::data_dir().join("handoffs")
}

/// A chat's folder of snapshots. Its id is a name Luna gave it; one that
/// is not a plain name is not made a path of.
pub fn dir_of(chat: &str) -> Result<PathBuf, String> {
    let plain = !chat.is_empty() && chat.len() <= 64 && chat.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !plain {
        return Err(format!("{chat} is not a chat id Luna keeps handoffs for"));
    }
    Ok(root().join(crate::paths::id_file_name(chat)))
}

/// Under a chat's folder of copies, where it writes its handoff: the one
/// place in Luna's folder a handoff file is taken from (file.rs), holding
/// only what the chat wrote there. Luna's own, so private: the system's temp
/// folder may be shared, and a handoff quotes the person and the project.
pub(super) const DRAFTS: &str = "drafts";

// The handoffs' folders are checked top down (`private_chain`): in a data
// folder others share (paths.rs `scope`), a link planted at any of them would
// lead what Luna writes there, and what it gives a CLI to write in, to a
// folder another user holds.
use crate::boundary::private_chain;

/// `chat`'s folder of copies, checked from Luna's folder of handoffs down
/// (`private_chain`) before a copy is written in it.
pub(super) fn copies_of(chat: &str) -> Result<PathBuf, String> {
    dir_of(chat)?;
    private_chain(&root(), &[&crate::paths::id_file_name(chat)])
}

/// `chat`'s drafts folder as its CLI starts, before that CLI runs: made
/// private from Luna's folder of handoffs down (`private_chain`), the drafts
/// folder's DACL put back to this user's, SYSTEM's and Administrators' alone,
/// whatever was added to it before. Every start of the chat's CLI, a resume
/// and a restart included, goes through it (pty/start.rs). Claude Code
/// reaches the folder within the folder of copies it is given (`--add-dir`);
/// Codex by an `--add-dir` of its own, on which its Windows sandbox then
/// grants itself write (0.167.0), which `drafts_of` leaves.
///
/// One that cannot be made private (a DACL that denies this user WRITE_DAC,
/// say, while another may write there) is not used until a start can: the
/// chat is named no drafts folder and none of its files is taken
/// (`drafts_refused`), the start gives its CLI none, and the log says why.
pub fn drafts_at_start(chat: &str) -> Result<PathBuf, String> {
    let made = dir_of(chat).and_then(|_| private_chain(&root(), &[&crate::paths::id_file_name(chat), DRAFTS]));
    let mut refused = refused_drafts().lock().unwrap_or_else(|e| e.into_inner());
    match &made {
        Ok(_) => {
            refused.remove(chat);
        }
        Err(why) => {
            refused.insert(chat.to_string(), why.clone());
            crate::log::warn("handoff", &format!("{}: its drafts folder is not used until a start makes it private: {why}", label(chat)));
        }
    }
    made
}

/// The chats whose drafts folder their CLI's last start could not make
/// private, with why (`drafts_at_start`).
fn refused_drafts() -> &'static Mutex<HashMap<String, String>> {
    static R: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    R.get_or_init(Default::default)
}

/// Why `chat`'s drafts folder is not to be named to it nor taken a file
/// from: its CLI's last start could not make it private.
pub(super) fn drafts_refused(chat: &str) -> Option<String> {
    refused_drafts().lock().unwrap_or_else(|e| e.into_inner()).get(chat).cloned()
}

/// What a chat's What Luna applied panel adds to its handoff tool where its
/// start gave no drafts folder, the folder not made private for `why`.
pub fn no_drafts_applied(why: &str) -> String {
    format!("; no --add-dir: the drafts folder could not be made private ({why}), so the chat writes its handoff outside Luna's folder")
}

/// `chat`'s drafts folder while its CLI runs, before it is named to the chat
/// (the call without `file`): the folders above it made private as ever
/// (`private_chain`), the drafts folder held to a real folder, no link,
/// owned by this user, with its DACL left as it is (`secret::own_dir`): the
/// grant the CLI's sandbox gave itself there since its start stays. No one
/// else could add to that DACL meanwhile: `drafts_at_start` left it this
/// user's, SYSTEM's and Administrators' alone, and only they may change it.
/// Made private where it went meanwhile.
pub fn drafts_of(chat: &str) -> Result<PathBuf, String> {
    dir_of(chat)?;
    let drafts = private_chain(&root(), &[&crate::paths::id_file_name(chat)])?.join(DRAFTS);
    crate::secret::own_dir(&drafts).map_err(|e| format!("{} is no folder of Luna's own: {e}", drafts.display()))?;
    Ok(drafts)
}

/// A session's `settings` as a caller is shown them (agents/list.rs, which
/// every row a tool gives goes through): whole to the session itself, and to
/// any other without its auto-handoff, which only the chat with it on is to
/// know of.
pub fn settings_shown(mut settings: Value, to_itself: bool) -> Value {
    if !to_itself {
        if let Some(o) = settings.as_object_mut() {
            o.remove("handoff");
        }
    }
    settings
}

/// The threshold `settings` holds, if auto-handoff is on in them.
pub(super) fn threshold_in(settings: &Value) -> Option<u8> {
    settings["handoff"].as_u64().and_then(|t| u8::try_from(t).ok()).filter(|t| (THRESHOLD_MIN..=THRESHOLD_MAX).contains(t))
}

/// Whether `id` is listed `luna_session_handoff`: a person's own chat with
/// Luna's tools (`chat_listed`), or a lead while auto-handoff for leads is on
/// (lead.rs `listed_to`). A helper without Luna's tools never is: it is its
/// orchestrator's to rotate. Listed is not answered: a call needs an ask
/// (`open_ask`).
pub fn may_list(id: &str) -> bool {
    chat_listed(id) || lead::listed_to(id)
}

/// Whether lead `id` hands its conversation off by itself as its context
/// fills: listed the tool while auto-handoff for leads is on (lead.rs
/// `listed_to`). Notices about its context advise its parent no rotation then
/// (notifier usage.rs).
pub(in crate::agents) fn lead_hands_off(id: &str) -> bool {
    lead::listed_to(id)
}

/// Whether `id` is a person's own chat (no parent, no background helper, not
/// the dev caller, which has no terminal) with Luna's tools, auto-handoff on or
/// not: what the threshold's beat, Hand off now and a move go by.
pub(super) fn chat_listed(id: &str) -> bool {
    if id == DEV_CALLER || crate::background::has(id) {
        return false;
    }
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    r.known.get(id).is_some_and(|k| k.tools && k.parent.is_none() && !k.orphan)
}

/// Auto-handoff for leads as the settings have it, its threshold held to the
/// range a chat's is.
pub fn lead_settings() -> crate::settings::LeadHandoff {
    #[cfg(test)]
    if let Some(l) = LEAD_FOR_TEST.with(std::cell::Cell::get) {
        let mut l = l;
        l.threshold = l.threshold.clamp(THRESHOLD_MIN, THRESHOLD_MAX);
        return l;
    }
    let mut l = crate::settings::get().lead_handoff;
    l.threshold = l.threshold.clamp(THRESHOLD_MIN, THRESHOLD_MAX);
    l
}

// A test's own word for the setting, on its thread: tests share the data
// dir's settings.json.
#[cfg(test)]
thread_local! {
    static LEAD_FOR_TEST: std::cell::Cell<Option<crate::settings::LeadHandoff>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
pub(crate) fn lead_settings_for_test(l: Option<crate::settings::LeadHandoff>) {
    LEAD_FOR_TEST.with(|c| c.set(l));
}

/// Whether auto-handoff's threshold is set on `id`, a chat that may be listed
/// the tool: the trigger that asks it at its threshold.
pub(super) fn auto_on(id: &str) -> bool {
    chat_listed(id) && reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(id).is_some_and(|k| threshold_in(&k.settings).is_some())
}

/// Who has `id` asked for a handoff now, if anyone: the threshold or the
/// person. A move's ask is moving.rs's own (`moving::Move::asking`).
pub(super) fn open_ask(id: &str) -> Option<store::AskKind> {
    store::records().get(id).and_then(|r| r.asked.as_ref()).map(|a| a.kind)
}

/// Whether `id` is asked for a handoff in the conversation its CLI is on now
/// (`ask_holds`): what a call is answered by. Refused with what the chat is
/// to hear: nobody asked it (no ask, the person's request not typed yet, or
/// another conversation asked), or Luna cannot yet tell which conversation
/// it is on.
pub(super) fn asked_now(id: &str) -> Result<(), &'static str> {
    let asked = store::records().get(id).and_then(|r| r.asked.clone());
    // Unasked, a call in the turn a move's closed ask was answered in is that
    // ask's late answer: the move was decided otherwise, which is no failure
    // of the chat's.
    let Some(a) = asked else { return Err(if moving::answers_closed_ask(id, "") { moving::MOVE_OVER } else { NOT_ASKED }) };
    let now = conversation_now(id);
    if ask_holds(&a, now.as_deref()) {
        return Ok(());
    }
    let pending = a.kind == store::AskKind::Manual && a.typed.is_none();
    let elsewhere = matches!((a.conversation.as_deref(), now.as_deref()), (Some(asked), Some(now)) if asked != now);
    Err(if pending || elsewhere { NOT_ASKED } else { NOT_YET })
}

/// Whether ask `a` holds with the chat on `conversation`: only where both are
/// known and the same, and a person's ask only once its request is typed. A
/// request still waiting to go in asked nothing yet: text injected into a
/// conversation the person started meanwhile must not find it open. A
/// conversation the person started since (`/clear`, `/new`) was asked
/// nothing: a handoff it sent would reset a conversation nobody asked to hand
/// off, maybe on words injected into it. Where either is not known it does
/// not hold: the threshold's ask is bound as it opens, the person's as its
/// request is typed, and a call made while the chat's conversation cannot be
/// told goes through once it can.
fn ask_holds(a: &store::Asked, conversation: Option<&str>) -> bool {
    if a.kind == store::AskKind::Manual && a.typed.is_none() {
        return false;
    }
    matches!((a.conversation.as_deref(), conversation), (Some(asked), Some(now)) if asked == now)
}

/// The conversation `id`'s CLI is on now, where its terminal runs here and
/// tells (`conversation_of`).
fn conversation_now(id: &str) -> Option<String> {
    #[cfg(test)]
    if let Some(c) = test_conversations().lock().unwrap_or_else(|e| e.into_inner()).get(id) {
        return Some(c.clone());
    }
    let pty = crate::pty::manager();
    let provider = pty.provider(id).filter(|_| pty.alive(id))?;
    conversation_of(pty, id, provider)
}

/// The conversation a test says each chat's CLI is on: tests run no terminal.
#[cfg(test)]
fn test_conversations() -> &'static Mutex<HashMap<String, String>> {
    static C: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

#[cfg(test)]
pub(super) fn set_conversation_for_test(id: &str, conversation: Option<&str>) {
    let mut c = test_conversations().lock().unwrap_or_else(|e| e.into_inner());
    match conversation {
        Some(conv) => c.insert(id.to_string(), conv.to_string()),
        None => c.remove(id),
    };
}

/// What a chat asked hears, calling it while Luna cannot tell which
/// conversation its CLI is on: whoever asked, the threshold or the person,
/// the ask holds only in the conversation it asked.
pub(super) const NOT_YET: &str = "Luna cannot tell yet which conversation this chat is on, so luna_session_handoff does \
     nothing for a moment. Call it again in a few seconds.";

/// What a chat listed the tool hears, calling it unasked: that nothing is
/// asked of it, and where the person asks for a handoff.
pub(super) const NOT_ASKED: &str = "nobody asked this chat for a handoff, so luna_session_handoff does nothing now. The person \
     asks for one from the chat pane's menu (Hand off now), and auto-handoff at its threshold; Luna then types the request \
     here, and the tool answers.";

/// How long a person's ask stays open once its request went into the chat:
/// time for a handoff to be written and called, as a move's ask gets
/// (moving.rs `ASK_WAIT_MS`). Past it the ask closes and the chat and the
/// person hear so, rather than a request the chat forgot answering hours
/// later.
pub(super) const MANUAL_ASK_MS: u64 = 10 * 60_000;

/// How long a person's ask waits for its request to go in, from when it was
/// opened: the chat never at its prompt, its conversation not known, a draft
/// that stays, its CLI not running. Past it the ask closes undelivered, and
/// the person may ask again, rather than an ask nobody can answer refusing
/// the next one for ever, across restarts too.
pub(super) const MANUAL_DELIVER_MS: u64 = MANUAL_ASK_MS;

/// The chats whose CLI has been given the tool in `tools/list` since it
/// started: one that was not (it started before Luna listed the tool to every
/// chat with its tools) is asked for no handoff until it starts again and
/// lists it.
fn listed_set() -> &'static Mutex<HashSet<String>> {
    static L: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    L.get_or_init(Default::default)
}

/// The tool went to `id`'s CLI in its `tools/list`.
pub fn listed(id: &str) {
    listed_set().lock().unwrap_or_else(|e| e.into_inner()).insert(id.to_string());
}

/// `id`'s CLI is starting: what it lists is to be seen again.
pub(super) fn unlisted(id: &str) {
    listed_set().lock().unwrap_or_else(|e| e.into_inner()).remove(id);
}

fn is_listed(id: &str) -> bool {
    listed_set().lock().unwrap_or_else(|e| e.into_inner()).contains(id)
}

/// Whether a handoff of `id` is under way: from the call until the prompt is
/// in or it is given up, notices to it and text sent to it wait, and go into
/// the new conversation after.
pub fn holds_text(id: &str) -> bool {
    store::records().get(id).is_some_and(|r| r.switch.is_some())
}

/// What a caller `luna_session_handoff` is not listed to hears, should it
/// call the tool all the same: no word of the handoff, which only a person's
/// chat is to know of. The MCP layer refuses such a call before this, as it
/// does a tool there is not (mcp/dispatch.rs).
pub(super) const NOT_OFFERED: &str = "luna_session_handoff is not one of this session's tools";

/// `luna_session_handoff` without `file`: how the caller's own chat hands off,
/// with the instructions Luna serves for it now (served.rs).
pub fn instructions(caller: &str) -> Result<String, String> {
    // A lead's: its switch is a turn's end, not a terminal's (lead.rs).
    if crate::background::has(caller) {
        return lead::instructions(caller);
    }
    served::instructions(caller)
}

/// `luna_session_handoff` with `file`: the caller's own chat, and only that,
/// handed over to a new conversation from it, while it is asked for one.
/// Checks and copies the file, numbers the handoff and schedules it; the
/// switch waits for the caller's turn to end.
pub fn handoff(caller: &str, file: &str) -> Result<Value, String> {
    if !may_list(caller) {
        return Err(NOT_OFFERED.into());
    }
    if crate::background::has(caller) {
        return lead::handoff(caller, file);
    }
    // Asked for a move's document: collected, nothing reset, so the CLI's
    // own jobs may go on (moving.rs). A move's late answer is refused there.
    if let Some(answer) = moving::asked_file(caller, file) {
        return answer;
    }
    asked_now(caller)?;
    let jobs = crate::activity::own_jobs(caller);
    if !jobs.is_empty() {
        return Err(format!(
            "not now: your CLI's own jobs are running ({}), and they cannot cross to a new conversation. Let them finish or \
             stop them, then call luna_session_handoff again",
            jobs.join(", ")
        ));
    }
    let pty = crate::pty::manager();
    let provider = pty.provider(caller).filter(|_| pty.alive(caller)).ok_or("this chat's CLI is not running in Luna's terminal")?;
    let before = Before {
        conversation: conversation_of(pty, caller, provider),
        transcript: transcript_path(pty, caller).map(|p| p.to_string_lossy().into_owned()),
    };
    schedule(caller, provider, file, before)
}

/// The handoff of `caller`, on the conversation `before`, scheduled from
/// `file` once it is copied: the rest of `handoff`, past what takes a terminal.
/// The number is reserved under the record's lock, and the file read and
/// copied outside it (file.rs, bounded in time): a slow or hostile path holds
/// up this chat's call alone. The switch is then recorded, on disk, under the
/// notifier's exclusion for the chat (`hand_off`): a notice or a held send on
/// its way into the chat goes in first, into the conversation that will hand
/// it on, and none starts once the switch is there.
fn schedule(caller: &str, provider: Provider, file: &str, before: Before) -> Result<Value, String> {
    // Checked before the copy goes in it, and before what is in it is read.
    let dir = copies_of(caller)?;
    // Past any copy on disk: one a core left behind as it stopped between the
    // copy and its record (a number reserved is not saved) is never reused.
    let free = file::next_free(&dir);
    let (n, served) = {
        let mut r = store::records();
        let rec = r.entry(caller.to_string()).or_default();
        if let Some(s) = &rec.switch {
            return Err(format!("handoff {} is under way already; end your turn", s.n));
        }
        if rec.reserved.is_some() {
            return Err("a handoff of this chat is being scheduled already; end your turn".into());
        }
        let n = (rec.last + 1).max(free);
        rec.last = n;
        rec.reserved = Some(n);
        (n, rec.served.clone())
    };
    let unreserve = || {
        if let Some(rec) = store::records().get_mut(caller).filter(|r| r.reserved == Some(n)) {
            rec.reserved = None;
        }
    };
    let snap = match file::snapshot(caller, file, &crate::paths::data_dir(), &dir, n) {
        Ok(snap) => snap,
        Err(e) => {
            unreserve();
            return Err(e);
        }
    };
    let snapshot = snap.path.to_string_lossy().into_owned();
    // Written to Luna's own instructions: the sections of their checklist the
    // file has no heading for are named in the answer, nothing refused. An
    // override's are its author's to look for.
    let missing = if served::checks_sections(served.as_ref()) { facts::missing_sections(&snap.text) } else { Vec::new() };
    let give_up = |e: String| {
        unreserve();
        let _ = std::fs::remove_file(&snap.path);
        Err(e)
    };
    let Ok(_exclusive) = crate::agents::notifier::hand_off(caller) else {
        return give_up("Luna is typing into this chat right now; call luna_session_handoff again in a moment".into());
    };
    let conversation = before.conversation.clone();
    let switch = Switch {
        n,
        provider,
        snapshot: snapshot.clone(),
        sha256: snap.sha256.clone(),
        source: snap.source.clone(),
        scheduled_at: now_ms(),
        phase: Phase::Scheduled,
        before,
        attempts: 0,
        instructions: served,
        opened: None,
    };
    // Under the record's lock, the registry looked at inside it: the ask this
    // handoff answers closed since the call (auto-handoff turned off under
    // `set_auto_handoff`, which takes them in the same order, the person's
    // ask lapsed, or the ask is of another conversation than the one called
    // from), or the chat started again without Luna's tools, leaves nothing
    // to record.
    let committed = store::change(caller, |rec| {
        if rec.reserved != Some(n) || rec.switch.is_some() {
            return Err(
                "this chat's handoffs changed while the file was copied (auto-handoff turned off, or another handoff); nothing will switch",
            );
        }
        rec.reserved = None;
        // A move begun since the call took the chat (moving.rs `taken`): its
        // reset would come in the middle of the move.
        if rec.moving.as_ref().is_some_and(moving::Move::under_way) {
            return Err("a move of this chat to another account began while the file was copied; nothing will switch");
        }
        if !rec.asked.as_ref().is_some_and(|a| ask_holds(a, conversation.as_deref())) || !may_list(caller) {
            return Err("the ask for this handoff closed while the file was copied (auto-handoff turned off, the person's ask \
                        lapsed, or the chat started again without Luna's tools), or it asks another conversation than this \
                        one; nothing will switch");
        }
        rec.switch = Some(switch);
        Ok(())
    });
    match committed {
        Ok(Ok(())) => {}
        Ok(Err(why)) => return give_up(why.into()),
        Err(e) => return give_up(format!("the handoff could not be saved, so nothing will switch: {e}")),
    }
    drop(_exclusive);
    // A request not yet typed, the threshold's or the person's, would land in
    // the new conversation, asking it for a handoff it has just been given.
    reg().lock().unwrap_or_else(|e| e.into_inner()).news.retain(|(c, line)| !(c == caller && facts::is_notice(line)));
    crate::agents::save_chats();
    let thin = if missing.is_empty() { String::new() } else { format!("; no heading for {}", missing.join(", ")) };
    crate::log::info(
        "handoff",
        &format!("{} scheduled handoff {n} from {} ({} bytes copied, sha256 {}){thin}", label(caller), snap.source, snap.size, snap.sha256),
    );
    let mut answer = json!({
        "scheduled": true,
        "n": n,
        "snapshot": snapshot,
        "sha256": snap.sha256,
        "next": "End your turn now. Once this chat is at rest Luna clears this conversation and opens the next one with your \
                 handoff and the facts it knows; notices and text sent to this chat wait until then. If it cannot, a notice \
                 here says why.",
    });
    if !missing.is_empty() {
        answer["missing"] = json!(missing);
        answer["note"] = json!(
            "Your file has no heading for these sections of Luna's checklist. It goes as it is; if one of them matters to the \
             next conversation, say it in your last words before you end your turn: the conversation before stays readable."
        );
    }
    Ok(answer)
}

/// The conversation `id`'s CLI is on: Claude Code's session id in its
/// registry entry, which `/clear` changes in place; the thread Codex names in
/// its terminal's title, which `/new` does, but not the subagent's its TUI is
/// watching (`/subagents`), which is no conversation of the chat's
/// (codex/session/rollouts.rs `own_thread`).
fn conversation_of(pty: &crate::pty::PtyManager, id: &str, provider: Provider) -> Option<String> {
    let p = pty.probe(id)?;
    match provider {
        Provider::Claude => crate::claude::session::session_id(p.pid, p.resume.as_deref(), &p.cwd, p.spawned_at_ms, &p.account_path),
        Provider::Codex => {
            let live = crate::codex::session::Live {
                chat_id: &p.id,
                cwd: &p.cwd,
                spawned_at_ms: p.spawned_at_ms,
                resume: p.resume.as_deref(),
                account_path: &p.account_path,
                last_output_ms: p.last_output_ms,
            };
            crate::codex::session::own_thread(&live, pty.codex_thread(id))
        }
    }
}

/// Whether the transcript at `path` is positively `live`'s, the conversation
/// the chat's CLI is on (`conversation_of`): its name carries its own id —
/// Claude Code's `<session id>.jsonl`, Codex's `rollout-<time>-<thread
/// id>.jsonl` — and that id is `live`'s. The transcript a chat is found on
/// may be the one before: after Codex's `/new` its rollout is the old
/// thread's until the new one writes its own (codex/session/rollouts.rs
/// `rollout_on`), and a transcript not found now is the last one found
/// (read.rs `transcript_seen`). Neither says anything of the conversation
/// the CLI is on, so nothing about an ask is decided on it.
fn transcript_is(path: &Path, provider: Provider, live: &str) -> bool {
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else { return false };
    if live.is_empty() {
        return false;
    }
    match provider {
        Provider::Claude => name.strip_suffix(".jsonl") == Some(live),
        Provider::Codex => crate::codex::session::parse_name(&name).is_some_and(|(_, id)| id.starts_with(live)),
    }
}

/// Whether the transcript at `path` holds handoff `n`'s prompt.
fn holds_prompt(path: &Path, n: u32) -> bool {
    crate::pty::tail(path, 1024 * 1024).is_some_and(|t| t.contains(&facts::prompt_opening(n)))
}

/// The chat's terminal, as the switch drives it.
struct Pty;

impl Term for Pty {
    fn look(&self, chat: &str, s: &Switch) -> Look {
        let pty = crate::pty::manager();
        let alive = pty.alive(chat);
        if !alive {
            return Look { now: now_ms(), ..Default::default() };
        }
        let transcript = transcript_path(pty, chat);
        let left = |t: &PathBuf| s.before.transcript.as_deref() != Some(&*t.to_string_lossy());
        let prompted = transcript.as_ref().filter(|t| left(t)).is_some_and(|t| holds_prompt(t, s.n));
        Look {
            now: now_ms(),
            alive,
            at_rest: ready_for_notice(pty, chat),
            draft: pty.has_draft(chat),
            typing: pty.typed_ago_ms(chat).is_some_and(|ms| ms < SEND_TYPING_MS),
            jobs: crate::activity::own_jobs(chat),
            conversation: conversation_of(pty, chat, s.provider),
            // After the conversation, in this order (`Look::keys`).
            keys: pty.typed_at_ms(chat),
            prompted,
        }
    }

    fn conversation(&self, chat: &str, s: &Switch) -> Option<String> {
        conversation_of(crate::pty::manager(), chat, s.provider)
    }

    fn keys(&self, chat: &str) -> u64 {
        crate::pty::manager().typed_at_ms(chat)
    }

    fn type_line(&self, chat: &str, text: &str, commit: &dyn Fn() -> Result<(), Refused>) -> Result<Typed, String> {
        type_line(chat, text, commit)
    }

    fn pause(&self) {
        std::thread::sleep(POLL);
    }
}

/// Types `text` into `chat`'s terminal as a prompt, as `Term::type_line`
/// says: the switch's and a move's opening line alike.
fn type_line(chat: &str, text: &str, commit: &dyn Fn() -> Result<(), Refused>) -> Result<Typed, String> {
    type_line_with(chat, text, commit, false)
}

/// `type_line`; `readable`: a composer the screen does not show at all is not
/// taken for an empty one (a move's line goes only where it can be read).
fn type_line_with(chat: &str, text: &str, commit: &dyn Fn() -> Result<(), Refused>, readable: bool) -> Result<Typed, String> {
    let pty = crate::pty::manager();
    // From here to the Enter a person's keys wait (`hold_keys`): what the
    // composer shows now is what the text lands in.
    let hold = pty.hold_keys(chat).ok_or("the chat's terminal is gone")?;
    let written = match pty.composer(chat) {
        Some(c) => !c.text.trim().is_empty() || c.stashed,
        None => readable && !pty.composer_visible(chat),
    };
    // A move's line also takes Luna's own count of a person's keys as a veto
    // no read of the screen clears (`keys_left_text`).
    let keys = readable && pty.keys_left_text(chat);
    if written || keys || pty.has_draft(chat) || pty.typed_ago_ms(chat).is_some_and(|ms| ms < SEND_TYPING_MS) {
        return Ok(Typed::NotClear);
    }
    if let Err(r) = commit() {
        return Ok(Typed::NotCommitted(r));
    }
    let before = crate::activity::turn_state(chat);
    type_in(pty, chat, text, before.map(|t| t.turn), before.map_or(0, |t| t.turns_ended), hold).map(|_| Typed::In)
}

/// The switches as the store keeps them.
struct Stored;

impl Phases for Stored {
    fn switch_of(&self, chat: &str) -> Option<Switch> {
        store::records().get(chat).and_then(|r| r.switch.clone())
    }

    fn advance(&self, chat: &str, n: u32, from: Phase, to: Phase, attempts: u32) -> Result<(), Refused> {
        let moved = store::change(chat, |r| match r.switch.as_mut().filter(|s| s.n == n && s.phase == from) {
            Some(s) => {
                s.phase = to;
                s.attempts = attempts;
                true
            }
            None => false,
        });
        match moved {
            Ok(true) => {
                crate::log::info("handoff", &format!("{}: handoff {n} {to:?}, reset tried {attempts} time(s)", label(chat)));
                Ok(())
            }
            Ok(false) => Err(Refused::Gone),
            Err(e) => {
                crate::log::warn("handoff", &format!("{}: handoff {n} could not move to {to:?}: {e}", label(chat)));
                Err(Refused::Unsaved(e))
            }
        }
    }

    fn opened(&self, chat: &str, n: u32, conversation: &str) -> Result<(), Refused> {
        // Once: a conversation on record is never written over.
        let set = store::change(chat, |r| {
            match r.switch.as_mut().filter(|s| s.n == n && matches!(s.phase, Phase::Resetting { .. }) && s.opened.is_none()) {
                Some(s) => {
                    s.opened = Some(conversation.to_string());
                    true
                }
                None => false,
            }
        });
        match set {
            Ok(true) => {
                crate::log::info("handoff", &format!("{}: handoff {n}'s reset opened {conversation}", label(chat)));
                Ok(())
            }
            Ok(false) => Err(Refused::Gone),
            Err(e) => {
                crate::log::warn("handoff", &format!("{}: where handoff {n}'s reset went could not be saved: {e}", label(chat)));
                Err(Refused::Unsaved(e))
            }
        }
    }
}

/// The chats a switch is running for on a thread of its own.
fn running() -> &'static Mutex<HashSet<String>> {
    static R: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    R.get_or_init(Default::default)
}

/// A chat's place in `running`, let go when dropped.
struct Running(String);

impl Drop for Running {
    fn drop(&mut self) {
        running().lock().unwrap_or_else(|e| e.into_inner()).remove(&self.0);
    }
}

/// Runs `chat`'s switch on a thread of its own, its terminal claimed for the
/// whole of it: nothing else Luna types goes in between the reset and the
/// prompt, where it would open the new conversation.
fn start_switch(chat: &str, s: &Switch) {
    if !running().lock().unwrap_or_else(|e| e.into_inner()).insert(chat.to_string()) {
        return;
    }
    let (id, s) = (chat.to_string(), s.clone());
    let started = std::thread::Builder::new().name("luna-handoff-switch".into()).spawn(move || {
        let _running = Running(id.clone());
        let Some(_typing) = TypingInto::wait_for(&id, TYPING_WAIT) else { return };
        switch_on(&Pty, &id, &s);
    });
    if let Err(e) = started {
        running().lock().unwrap_or_else(|e| e.into_inner()).remove(chat);
        crate::log::warn("handoff", &format!("could not start the switch of {}: {e}", label(chat)));
    }
}

/// `chat`'s switch `s` run through `term` to its end or back to the beat.
/// Nothing is reset or prompted on a copy that is not the one Luna made
/// (`copy_holds`): the chat and the person are told why instead, and the
/// prompt that would point at the copy is not given.
fn switch_on(term: &dyn Term, chat: &str, s: &Switch) {
    if let Err(why) = copy_holds(chat, s) {
        let reset = s.phase != Phase::Scheduled;
        finish(chat, s.n, Ended::Failed { why: format!("its copy cannot be trusted: {why}"), reset }, &String::new);
        return;
    }
    // The facts as they are when the prompt goes in, past the reset.
    let prompt = || continuation(chat, s);
    let ended = phase::run(term, chat, s.n, &prompt, &Stored);
    finish(chat, s.n, ended, &prompt);
}

/// Whether `s`'s copy is still the one Luna made, before anything is typed on
/// it or a prompt names it: right in the chat's folder of copies, checked
/// from the folder of handoffs down (`copies_of`), read as a private file of
/// this user's opened as itself (`secret::read_private`), its SHA-256 the one
/// on record. A switch a core takes up again may otherwise meet a link put at
/// the copy's name while the folder was still others' to write, and the next
/// conversation, which reads the copy by its path, would read what it leads to.
fn copy_holds(chat: &str, s: &Switch) -> Result<(), String> {
    let dir = copies_of(chat)?;
    let path = Path::new(&s.snapshot);
    if path.parent() != Some(dir.as_path()) {
        return Err(format!("{} is not in this chat's folder of copies", path.display()));
    }
    let bytes = crate::secret::read_private(path, file::MAX_BYTES)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("{} is gone", path.display()))?;
    let sha256: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    if sha256 != s.sha256 {
        return Err(format!("{} is not the file Luna copied (its SHA-256 differs)", path.display()));
    }
    Ok(())
}

/// The prompt that opens `s`'s conversation, the facts as of now. Past what a
/// prompt is typed as whole (delivery.rs `spill`), the facts go in a file of
/// their own beside the snapshot, in the chat's folder of copies as checked
/// (`copies_of`), and the prompt names it.
fn continuation(chat: &str, s: &Switch) -> String {
    let facts = facts::fact_block(&gather(chat, s.before.transcript.as_deref(), s.provider));
    let name = chat_name(chat);
    let previous = s.before.transcript.as_deref();
    let whole = facts::prompt(s.n, chat, name.as_deref(), &s.snapshot, &s.sha256, &s.source, previous, &facts);
    if whole.chars().count() <= 6000 {
        return whole;
    }
    // A new private file renamed into place, which takes the place of a link
    // at the name rather than writing through it.
    let written = copies_of(chat).and_then(|dir| {
        let path = dir.join(format!("{}-facts.md", s.n));
        crate::secret::write_private(&path, facts.as_bytes()).map(|()| path).map_err(|e| e.to_string())
    });
    let pointer = match written {
        Ok(path) => format!("What Luna knows now is in {}: read it after the handoff.", path.display()),
        Err(_) => facts.chars().take(4000).collect(),
    };
    facts::prompt(s.n, chat, name.as_deref(), &s.snapshot, &s.sha256, &s.source, previous, &pointer)
}

/// What Luna knows of `chat` now, for the prompt: the skills loaded in the
/// conversation before, too, read off its transcript at `previous`.
fn gather(chat: &str, previous: Option<&str>, provider: Provider) -> facts::Facts {
    let listing = crate::agents::list::list(chat, None, false);
    let (known, owed, held, usage) = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let owed: Vec<String> = r.owed.keys().filter(|(c, _)| c == chat).map(|(_, id)| id.clone()).collect();
        let held: Vec<String> = r.held.iter().filter(|h| h.caller == chat).map(|h| h.id.clone()).collect();
        (r.known.get(chat).cloned(), owed, held, r.usage.clone())
    };
    let helpers: Vec<facts::Helper> = listing
        .sessions
        .iter()
        .map(|row| facts::Helper {
            id: row.id.clone(),
            name: row.name.clone(),
            account: format!("{}/{}", row.provider.as_str(), row.account),
            model: row.signs.resolved_model.clone().or_else(|| row.settings["model"].as_str().map(str::to_owned)),
            effort: row.settings["effort"].as_str().map(str::to_owned),
            state: row.state.to_string(),
            task: row.task.clone(),
            worktree: row.worktree.path.clone(),
            branch: row.worktree.branch.clone(),
            sha: row.worktree.sha.clone(),
            transcript: transcript_path(crate::pty::manager(), &row.id).map(|p| p.to_string_lossy().into_owned()),
            owes: owed.contains(&row.id),
            undelivered: crate::agents::lost::undelivered_for(&row.id, chat, row.parent.as_deref() == Some(chat)).len(),
        })
        .collect();
    let mine: HashSet<&str> = listing.sessions.iter().map(|r| r.id.as_str()).collect();
    let owed_by_others: Vec<String> = owed.iter().filter(|id| !mine.contains(id.as_str())).cloned().collect();
    // The accounts the chat and its helpers run on, and how far each has gone.
    let mut accounts: Vec<String> = listing.sessions.iter().map(|r| format!("{}/{}", r.provider.as_str(), r.account)).collect();
    if let Some(k) = &known {
        accounts.insert(0, format!("{}/{}", k.provider.as_str(), k.account));
    }
    let mut seen = HashSet::new();
    accounts.retain(|a| seen.insert(a.clone()));
    let accounts = accounts.into_iter().filter_map(|a| Some((a.clone(), usage_words(usage.get(&a)?)?))).collect();
    let mut held_for = held;
    held_for.dedup();
    // From its handoff call on, a turn an ask opened is the handoff being
    // written, not the work: left out of the skills. Before it is work the
    // ask let the chat finish ("at the next point where your work can pause").
    let ask_turns = excerpts::Skip { opens: facts::is_ask_prompt, ask: None, from_call: true };
    facts::Facts {
        provider: known.as_ref().map(|k| k.provider),
        account: known.as_ref().map(|k| k.account.clone()).unwrap_or_default(),
        folder: known.as_ref().map(|k| k.folder.clone()).unwrap_or_default(),
        settings: known.as_ref().map(|k| k.settings.clone()).unwrap_or_default(),
        threshold: known.as_ref().and_then(|k| threshold_in(&k.settings)),
        helpers,
        owed_by_others,
        held_for,
        accounts,
        skills: previous.and_then(|p| match excerpts::skills_in(Path::new(p), provider, Some(ask_turns)) {
            Ok(skills) => Some(skills),
            Err(e) => {
                crate::log::warn("handoff", &format!("{}: the skills its conversation loaded are not known: {e}", label(chat)));
                None
            }
        }),
        at: crate::background::iso(now_ms()).unwrap_or_default(),
    }
}

/// An account's use as the window last sent it, as words: each window's
/// share and when it resets.
fn usage_words(u: &Value) -> Option<String> {
    let windows: Vec<String> = u["limits"]
        .as_array()?
        .iter()
        .filter_map(|l| {
            let pct = l["usedPct"].as_f64()?;
            let resets = l["resetsAt"].as_str().map(|t| format!(", resets {t}")).unwrap_or_default();
            Some(format!("{} {pct:.0}%{resets}", l["label"].as_str()?))
        })
        .collect();
    (!windows.is_empty()).then(|| windows.join(", "))
}

/// Handoff `n` of record `r` closed, if it is the one there and `only_if`
/// says so: kept as past (the last KEPT), with those dropped from the record
/// handed back for their copies to go. One through lets go of what the chat
/// was served and asked, the ask it answered with it: the new conversation
/// has its own to come.
fn close(
    r: &mut store::Record,
    n: Option<u32>,
    done: bool,
    why: Option<String>,
    only_if: impl Fn(&Switch) -> bool,
) -> Option<(Switch, Vec<store::Past>)> {
    if !r.switch.as_ref().is_some_and(|s| n.is_none_or(|n| s.n == n) && only_if(s)) {
        return None;
    }
    let s = r.switch.take()?;
    r.past.push(store::Past {
        n: s.n,
        at: now_ms(),
        provider: s.provider,
        snapshot: s.snapshot.clone(),
        sha256: s.sha256.clone(),
        previous: s.before.transcript.clone(),
        done,
        why,
        instructions: s.instructions.clone(),
        moved: false,
    });
    r.fresh = done;
    if done {
        r.served = None;
        r.asked = None;
        if let Some(t) = r.told.as_mut() {
            t.shares.clear();
            t.compacted = 0;
        }
    }
    let over = r.past.len().saturating_sub(store::KEPT);
    let gone: Vec<store::Past> = r.past.drain(..over).collect();
    Some((s, gone))
}

/// Handoff `n` of `chat` has ended: kept on record, its place given up, and
/// when it was given up, the chat and the person told; one given up after its
/// reset with the prompt (`prompt` makes it, the facts as of then). A record
/// that will not save lets go of it all the same: held notices must not wait
/// for a disk. One given up past its reset closes its ask too: the
/// conversation that was asked is gone.
fn finish(chat: &str, n: u32, ended: Ended, prompt: &dyn Fn() -> String) {
    let (done, why, reset, left): (bool, Option<String>, bool, Option<LeftLine>) = match ended {
        Ended::Back => return,
        Ended::Done => (true, None, true, None),
        Ended::Failed { why, reset } => (false, Some(why), reset, None),
        Ended::Left => (false, Some(phase::LEFT.to_string()), false, Some(facts::left_line)),
        Ended::Stray(why) => (false, Some(why), false, Some(facts::stray_line)),
    };
    let end = |r: &mut store::Record| {
        let closed = close(r, Some(n), done, why.clone(), |_| true);
        if closed.is_some() && (reset || left.is_some()) {
            r.asked = None;
        }
        closed
    };
    let closed = match store::change(chat, end) {
        Ok(closed) => closed,
        Err(e) => {
            crate::log::warn("handoff", &format!("{}: handoff {n} could not be saved as ended: {e}", label(chat)));
            store::records().get_mut(chat).and_then(end)
        }
    };
    let Some((s, gone)) = closed else { return };
    ended_with(chat, &s, gone, why, reset, left, prompt);
}

/// What the conversation a chat went on in hears of a handoff given up with
/// nothing typed there (`Ended::Left`, `Ended::Stray`): its number and copy.
type LeftLine = fn(u32, &str) -> String;

/// What follows a handoff's end: the copies dropped from the record go, and one
/// given up is told to the chat and the person; one given up on a conversation
/// it typed nothing into, by `left`'s line.
fn ended_with(
    chat: &str,
    s: &Switch,
    gone: Vec<store::Past>,
    why: Option<String>,
    reset: bool,
    left: Option<LeftLine>,
    prompt: &dyn Fn() -> String,
) {
    // Only out of the chat's folder of copies as checked: a removal through a
    // link on the way would land wherever it leads.
    if let Ok(dir) = copies_of(chat) {
        for p in gone.iter().filter(|p| Path::new(&p.snapshot).parent() == Some(dir.as_path())) {
            let _ = std::fs::remove_file(&p.snapshot);
            let _ = std::fs::remove_file(dir.join(format!("{}-facts.md", p.n)));
        }
    }
    match why {
        None => {
            crate::log::info("handoff", &format!("{} went through handoff {}: a new conversation from {}", label(chat), s.n, s.snapshot))
        }
        Some(why) => {
            crate::log::warn("handoff", &format!("{} gave up handoff {}: {why}", label(chat), s.n));
            let prompt = if reset { prompt() } else { String::new() };
            // The conversation that called may call again only while it is
            // still asked.
            let asked = may_list(chat) && open_ask(chat).is_some();
            // Told in the conversation the person went on in, which asked
            // nothing: what became of the one before's call, nothing to do.
            let line = match left {
                Some(line) => line(s.n, &s.snapshot),
                None => facts::failed_line(s.n, &why, &s.snapshot, reset, asked, &prompt),
            };
            // The notifier types it later, maybe after the person went on in
            // another conversation: one with the continuation in it is bound
            // to the conversation the reset opened (`as_told`).
            if left.is_none() && reset && !prompt.is_empty() {
                bind_continued(chat, &line, s.opened.clone(), facts::continued_short(Some(s.n), Some(&why), Some(&s.snapshot)));
            }
            reg().lock().unwrap_or_else(|e| e.into_inner()).news.push((chat.to_string(), line));
            crate::agents::save_chats();
            let name = chat_name(chat).unwrap_or_else(|| chat.to_string());
            crate::power::notify_chat(chat, "A handoff did not go through", &format!("\"{name}\": {why}"));
        }
    }
}

/// Failure line `line` of `chat`, which carries the continuation, kept bound
/// to `conversation`, with `short` to take its place elsewhere. The last few
/// are kept: a line waits in the notifier's queue only until it is told. A
/// record that will not save keeps it in memory, as `finish` does its end.
fn bind_continued(chat: &str, line: &str, conversation: Option<String>, short: String) {
    let bound = store::Continued { line_sha256: sha256_of(line), conversation, short };
    let add = |r: &mut store::Record| {
        r.continued.push(bound.clone());
        let over = r.continued.len().saturating_sub(store::KEPT);
        r.continued.drain(..over);
    };
    if let Err(e) = store::change(chat, add) {
        crate::log::warn("handoff", &format!("{}: which conversation a failure line is for could not be saved: {e}", label(chat)));
        if let Some(r) = store::records().get_mut(chat) {
            add(r);
        }
    }
}

fn sha256_of(text: &str) -> String {
    Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// Line `line` of the notifier's queue for `chat` as it is to be typed now. A
/// handoff's failure line carrying the continuation goes in whole only while
/// the chat is on the conversation it was made for, the one Luna's reset
/// opened, both known and the same (`Record::continued`); anywhere else, or
/// where Luna cannot tell, its short line, which names only the copy, takes
/// its place: a prompt typed into a conversation the person started since
/// would hand that one a handoff it never asked for. Any other line as it is.
pub(in crate::agents) fn as_told(chat: &str, line: &str) -> String {
    if !bound_to_conversation(line) {
        return line.to_string();
    }
    let sha = sha256_of(line);
    let bound = store::records().get(chat).and_then(|r| r.continued.iter().find(|c| c.line_sha256 == sha).cloned());
    let now = conversation_now(chat);
    match bound {
        Some(b) if b.conversation.is_some() && b.conversation == now => line.to_string(),
        Some(b) => b.short,
        None => facts::continued_short(None, None, None),
    }
}

/// Whether line `line` of the notifier's queue goes in whole only on the
/// conversation it was made for (`as_told`): a handoff's failure line
/// carrying the continuation.
pub(in crate::agents) fn bound_to_conversation(line: &str) -> bool {
    line.contains(facts::CONTINUED)
}

/// Whether line `line` of the notifier's queue is for one conversation of
/// the chat's alone, a continuation (`bound_to_conversation`) or a
/// threshold notice (its ask's, `notice_here`): either goes in only with no
/// key of the person's since the notifier's look (notifier
/// `deliver_notice`), as a `/clear` or `/new` they just sent may not have
/// reached the CLI yet, which still reads as the conversation asked.
pub(in crate::agents) fn keyed_to_conversation(line: &str) -> bool {
    bound_to_conversation(line) || facts::is_threshold_notice(line)
}

/// Whether line `line` of the notifier's queue for `chat` is to be dropped
/// rather than typed: a handoff's notice while a move of the chat is under way
/// (`moving::under_way`). A move's start drops one queued and retires the ask
/// it opened, but one already put together, or queued as the move began, would
/// still ask the chat for a handoff that resets it in the middle of the move.
pub(in crate::agents) fn withheld(chat: &str, line: &str) -> bool {
    facts::is_notice(line) && (moving::under_way(chat) || (facts::is_threshold_notice(line) && !threshold_ask_of(chat, line)))
}

/// Whether threshold notice `line` still has the ask it was queued with open
/// on `chat`: the threshold's, bound to that very notice (`Asked::notice`;
/// one from before that was kept is taken as its own), with auto-handoff
/// still on. The notifier puts a notice together from its queue and types it
/// seconds later: auto-handoff turned off meanwhile closes the ask and
/// unqueues the notice, a later ask replaces it, and a copy already put
/// together must not ask for what the tool would refuse. Gone for good, so
/// the notifier drops it.
fn threshold_ask_of(chat: &str, line: &str) -> bool {
    let asked = store::records().get(chat).and_then(|r| r.asked.clone());
    let Some(a) = asked.filter(|a| a.kind == store::AskKind::Auto) else { return false };
    a.notice.as_deref().is_none_or(|n| n == sha256_of(line)) && threshold_now(chat).is_some()
}

/// Whether line `line` of the notifier's queue for `chat` goes in now, with
/// the person's keys held, right before the paste: a threshold notice only
/// while its ask stands (`withheld`) and the chat is on the conversation it
/// asks, both known (`Asked::conversation`), as the tool would answer only
/// there. Any other line, as far as this goes. Not now is not never: one on
/// another conversation is closed by the beat (`drifted`) and then dropped.
pub(in crate::agents) fn notice_here(chat: &str, line: &str) -> bool {
    if !facts::is_threshold_notice(line) {
        return true;
    }
    let asked = store::records().get(chat).and_then(|r| r.asked.as_ref().and_then(|a| a.conversation.clone()));
    !withheld(chat, line) && asked.is_some() && asked == conversation_now(chat)
}

/// What the switches' beat keeps between looks.
#[derive(Default)]
struct Beat {
    /// Looks in a row each chat was found at rest.
    rest: HashMap<String, u32>,
    /// Since when a draft has held each chat's switch, and whether the desktop
    /// has been told.
    drafts: HashMap<String, (u64, bool)>,
    /// The turn count each chat's context was last looked at for.
    turns: HashMap<String, u32>,
}

/// Runs for the life of the core: every few seconds, an ask whose chat went on
/// in another conversation is closed, each chat with auto-handoff on is
/// looked at for its threshold once a turn has ended, or once it is at rest
/// after auto-handoff was turned on (`checks_now`), each person's ask is
/// typed or lapses, and each handoff under way is taken a step on.
pub(super) fn beat() {
    let mut b = Beat::default();
    loop {
        std::thread::sleep(NOTIFY_EVERY);
        let pty = crate::pty::manager();
        drifted(pty);
        thresholds(pty, &mut b);
        manual_asks(now_ms());
        switches(&mut b);
        moving::beat();
    }
}

/// Each ask open with no handoff under way, held to its conversation
/// (`ask_holds`): closed once the chat is found on another. Every ask knows
/// its conversation as it opens (the threshold's) or is typed (the person's).
/// Luna's own switch is under way while its conversation changes, and closes
/// its ask as it ends (`finish`); a look that cannot tell which conversation
/// the chat is on leaves it be.
fn drifted(pty: &crate::pty::PtyManager) {
    let open: Vec<(String, store::Asked)> = store::records()
        .iter()
        .filter(|(_, r)| r.switch.is_none())
        .filter_map(|(chat, r)| Some((chat.clone(), r.asked.clone()?)))
        .collect();
    for (chat, a) in open {
        // Luna is typing into it: a request going in, or a switch.
        if running().lock().unwrap_or_else(|e| e.into_inner()).contains(&chat) {
            continue;
        }
        let Some(provider) = pty.provider(&chat).filter(|_| pty.alive(&chat)) else { continue };
        let Some(now) = conversation_of(pty, &chat, provider) else { continue };
        drifted_to(&chat, &a, &now);
    }
}

/// Ask `a` of `chat`, with the chat on `now`: closed where it asks another
/// conversation. A person's request not typed yet asks nothing, and is left
/// be. Only the ask that was looked at, and only while no handoff is under
/// way: one scheduled since answered it. Whether it closed.
fn drifted_to(chat: &str, a: &store::Asked, now: &str) -> bool {
    if a.conversation.as_deref().is_none_or(|asked| asked == now) {
        return false;
    }
    let changed = store::change(chat, |r| {
        if r.switch.is_some() || r.asked.as_ref() != Some(a) {
            return false;
        }
        r.asked = None;
        true
    });
    match changed {
        Ok(true) => {
            crate::log::info(
                "handoff",
                &format!("{}: went on in another conversation ({now}): the ask for a handoff of the one before closed", label(chat)),
            );
            true
        }
        Ok(false) => false,
        Err(e) => {
            crate::log::warn("handoff", &format!("{}: its ask for a handoff could not be saved: {e}", label(chat)));
            false
        }
    }
}

/// The chats with auto-handoff on that may be listed the tool (`may_list`):
/// id, threshold, provider.
fn opted_in() -> Vec<(String, u8, Provider)> {
    let set: Vec<(String, u8, Provider)> = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        r.known.iter().filter_map(|(id, k)| Some((id.clone(), threshold_in(&k.settings)?, k.provider))).collect()
    };
    set.into_iter().filter(|(id, ..)| chat_listed(id)).collect()
}

/// The chats whose threshold is to be looked at as soon as they are at rest,
/// whatever turn they are at: auto-handoff turned on, or set at another
/// threshold (`set_auto_handoff`). The look at a turn's end goes by the turns
/// ended, so a chat already past its new threshold would otherwise hear
/// nothing until its next turn ends, which may be long in coming. Each mark is
/// numbered, and a look lets go only of the mark it was taken under
/// (`look_at`): one set again meanwhile, at another threshold, gets a look of
/// its own. Kept in memory only: a core started again looks at every chat at
/// rest at its first beat (`Beat::turns` starts empty), which is the same look.
fn checks_now() -> &'static Mutex<HashMap<String, u64>> {
    static C: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// `id`'s threshold is to be looked at once it is at rest (`checks_now`).
fn check_now(id: &str) {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let mark = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    checks_now().lock().unwrap_or_else(|e| e.into_inner()).insert(id.to_string(), mark);
}

/// The mark `id`'s threshold is to be looked at under, if it has one.
fn check_due(id: &str) -> Option<u64> {
    checks_now().lock().unwrap_or_else(|e| e.into_inner()).get(id).copied()
}

/// `id`'s mark let go: `mark` alone where it is given (its look is over), any
/// where it is not (auto-handoff off, the chat gone).
fn unmark(id: &str, mark: Option<u64>) {
    let mut c = checks_now().lock().unwrap_or_else(|e| e.into_inner());
    if mark.is_none_or(|m| c.get(id) == Some(&m)) {
        c.remove(id);
    }
}

/// The threshold notices due, each queued for the notifier (`Registry::news`),
/// which types it in once the chat is at rest, a draft respected.
fn thresholds(pty: &crate::pty::PtyManager, b: &mut Beat) {
    let chats = opted_in();
    b.turns.retain(|id, _| chats.iter().any(|(c, ..)| c == id));
    for (chat, threshold, provider) in chats {
        let Some((turns_ended, mark)) = to_look(b, &chat, crate::activity::turn_state(&chat)) else { continue };
        let Some(path) = transcript_path(pty, &chat) else { continue };
        // Only a transcript that is positively the conversation the CLI is
        // on says how far that conversation has come, and which the ask is
        // of: Codex's rollout read after a `/new` is the old thread's until
        // the new one writes its own. Otherwise looked at again next beat.
        let Some(live) = conversation_of(pty, &chat, provider).filter(|live| transcript_is(&path, provider, live)) else {
            b.turns.remove(&chat);
            continue;
        };
        let Some(context) = crate::background::Context::of(&path, provider) else { continue };
        let conversation = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        // Read only for a record a handoff left fresh, which its first look
        // clears: once per conversation a handoff began.
        let fresh = store::records().get(&chat).is_some_and(|r| r.fresh);
        // A turn over since: the floor is where it ended, not where the
        // context stands at a look a queued prompt, a notice or a core's
        // restart put off; one with no reading of it goes by now's.
        let floor_at =
            if fresh { excerpts::handoffs_first_turn(&path, provider).map(|t| t.ended_pct.unwrap_or(context.pct)) } else { None };
        let reading = Reading { live, conversation, pct: context.pct, tokens: context.tokens, floor_at };
        look_at(&chat, threshold, &reading, turns_ended, mark);
    }
}

/// Whether `chat`'s context is to be read for its threshold now, at turn
/// state `ts`, and if so the turn count and the mark it is read under: at
/// rest (`Turn::Idle`) at a turn count not looked at yet, or at any while it
/// is marked to be looked at now (`checks_now`). Not while a person's ask is
/// open, which is the chat's request already, nor while a handoff or a move
/// of it is under way (`moving::under_way`), nor while its CLI has not listed
/// the tool, which a quick resume brings (`set_auto_handoff`'s `resume`): a
/// mark waits for each of them to pass.
fn to_look(b: &mut Beat, chat: &str, ts: Option<crate::activity::TurnState>) -> Option<(u32, Option<u64>)> {
    if holds_text(chat) || moving::under_way(chat) || !is_listed(chat) || open_ask(chat) == Some(store::AskKind::Manual) {
        return None;
    }
    let ts = ts.filter(|t| t.turn == crate::activity::Turn::Idle)?;
    let mark = check_due(chat);
    let seen = b.turns.insert(chat.to_string(), ts.turns_ended) == Some(ts.turns_ended);
    (!seen || mark.is_some()).then_some((ts.turns_ended, mark))
}

/// What a look at a chat's context read: the conversation its CLI is on
/// (`live`), which its transcript was checked to be (`transcript_is`), that
/// transcript's name, how full the context is, and for a conversation Luna's
/// handoff prompt opened, the share its first turn ended at, or stands at
/// while that turn is the last (`excerpts::handoffs_first_turn`).
struct Reading {
    live: String,
    conversation: String,
    pct: u64,
    tokens: u64,
    floor_at: Option<u64>,
}

/// `chat`'s context as `reading` found it at `turns_ended`, held to its
/// `threshold`: what the conversation was told kept, and where a notice is
/// due, the threshold's ask opened, bound to the conversation read, its
/// notice queued with it (`ask_auto`). The one look both a turn's end and a
/// mark (`checks_now`) take; the mark it was taken under is let go.
fn look_at(chat: &str, threshold: u8, reading: &Reading, turns_ended: u32, mark: Option<u64>) {
    if mark.is_some() {
        unmark(chat, mark);
    }
    // The floor a handoff gives the conversation it began (`Record::fresh`)
    // is where its first turn ended: one that begins above the threshold is
    // not to be sent straight into the next handoff, however late it is first
    // looked at (auto-handoff turned on since, a notice or a queued prompt
    // first, a core started again). A conversation the person opened since
    // has none.
    let (told, fresh) = store::records().get(chat).map(|r| (r.told.clone(), r.fresh)).unwrap_or_default();
    let floor_at = reading.floor_at.filter(|_| fresh);
    let now = watch::told_now(told.as_ref(), &reading.conversation, reading.pct, reading.tokens, floor_at, threshold);
    if let Some(t) = told.as_ref().filter(|t| t.conversation == now.conversation && t.level > now.level) {
        let why = if now.compacted > t.compacted {
            "its CLI compacted the conversation".to_string()
        } else {
            format!("its context fell with no fewer tokens ({}, a larger window?)", reading.tokens)
        };
        crate::log::info(
            "handoff",
            &format!(
                "{}: {why}, now at {}%, before a handoff (asked at {:?}); it will be asked again",
                label(chat),
                reading.pct,
                now.shares
            ),
        );
    }
    let level = watch::level(reading.pct, threshold, now.floor);
    if !watch::due(&now, level) {
        // A conversation looked at for the first time keeps its floor, and
        // one whose context fell is to be asked again.
        if told.as_ref() != Some(&now) {
            match store::change(chat, |r| kept_told(r, now)) {
                Ok(true) => crate::log::info("handoff", &format!("{}: the threshold's ask closed with its notice", label(chat))),
                Ok(false) => {}
                Err(e) => crate::log::warn("handoff", &format!("{}: where its context stands could not be saved: {e}", label(chat))),
            }
        }
        return;
    }
    // The firm notice carries the checklist where Luna's own instructions
    // are what the chat would be served.
    let firm = level == 2;
    let inline = firm && crate::library::handoff_instructions().source == crate::library::Source::Luna;
    // Told once it is on disk: a notice a later core would repeat is the
    // smaller harm than none, so a save that fails still lets it go.
    let mut shares = now.shares.clone();
    shares.push(reading.pct);
    let told = store::Told { level, shares, tokens: reading.tokens, ..now };
    // A fresh handoff request, the ask the tool answers while it is open,
    // of the conversation whose context passed the threshold.
    let asked =
        store::Asked { conversation: Some(reading.live.clone()), ..store::Asked::new(store::AskKind::Auto, now_ms(), Some(turns_ended)) };
    let line = facts::notice(reading.pct, threshold, firm, inline);
    match store::change(chat, |r| ask_auto(chat, r, threshold, told, inline, asked, line)) {
        Ok(true) => {}
        Ok(false) => {
            crate::log::info(
                "handoff",
                &format!("{}: not asked at its threshold: auto-handoff, an ask or a handoff changed as its context was read", label(chat)),
            );
            return;
        }
        Err(e) => crate::log::warn("handoff", &format!("{}: the notice it was told could not be saved: {e}", label(chat))),
    }
    crate::agents::save_chats();
    crate::log::info("handoff", &format!("{} is at {}% of its context: asked for a handoff (level {level})", label(chat), reading.pct));
}

/// The threshold's ask opened on `chat`'s record `r`, with what the chat was
/// told (`told_of`), if what the look before found still stands under the
/// record's lock: its context was read outside it, and meanwhile auto-handoff
/// may have been turned off (`set_auto_handoff` closes the ask under this
/// lock) or set to another threshold, the person asked, a move begun, or a
/// handoff been scheduled. Opening it then would undo the one or
/// take the place of the other. From it on, what the chat sends is its own,
/// not an earlier ask's late answer (moving.rs `late_answer`). Its notice
/// `line` is queued under the same lock: a move or the person's ask begun
/// right after drops it as it opens (moving.rs `threshold_retired`,
/// `open_manual`), where one queued after them would ask the chat anyway.
/// Whether it opened.
fn ask_auto(chat: &str, r: &mut store::Record, threshold: u8, told: store::Told, inline: bool, asked: store::Asked, line: String) -> bool {
    let manual = r.asked.as_ref().is_some_and(|a| a.kind == store::AskKind::Manual);
    let moving = r.moving.as_ref().is_some_and(moving::Move::under_way);
    if manual || moving || r.switch.is_some() || r.reserved.is_some() || threshold_now(chat) != Some(threshold) {
        return false;
    }
    told_of(r, told, inline);
    r.ask_closed = None;
    // The notice bound to this ask: a copy the notifier holds goes in only
    // while it is the one open (`withheld`).
    r.asked = Some(store::Asked { notice: Some(sha256_of(&line)), ..asked });
    reg().lock().unwrap_or_else(|e| e.into_inner()).news.push((chat.to_string(), line));
    true
}

/// The threshold auto-handoff is set at on `id` now, a chat that may be
/// listed the tool.
fn threshold_now(id: &str) -> Option<u8> {
    if !chat_listed(id) {
        return None;
    }
    reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(id).and_then(|k| threshold_in(&k.settings))
}

/// What a conversation due no notice is told now (`watch::told_now`), kept
/// on its record. One the threshold re-armed (its context fell well under it,
/// or it is another conversation) closes the threshold's ask with the notice
/// it came from: the chat is not near its limit any more, and the threshold
/// asks again if it gets there. Bounded by context rather than by time: a
/// handoff being written in a long turn is not cut off, and the threshold, not
/// noticing again at a level it told of, would leave a chat whose ask timed
/// out past its threshold with nobody asking. Whether an ask closed.
fn kept_told(r: &mut store::Record, now: store::Told) -> bool {
    let rearmed = r.told.as_ref().is_some_and(|t| t.level > 0) && now.level == 0;
    let close = rearmed && r.switch.is_none() && r.asked.as_ref().is_some_and(|a| a.kind == store::AskKind::Auto);
    if close {
        r.asked = None;
    }
    r.told = Some(now);
    r.fresh = false;
    close
}

/// A notice due, as the chat's record keeps it: what it was told, and where
/// the notice carries the checklist (`inline`) and the chat was given nothing
/// yet, that checklist as what its handoff is written by: one that follows it
/// without asking the tool has its handoff's provenance all the same.
fn told_of(r: &mut store::Record, told: store::Told, inline: bool) {
    r.told = Some(told);
    r.fresh = false;
    if inline && r.served.is_none() {
        r.served = Some(served::Served::checklist());
    }
}

/// The person's asks, each a step on (`manual_ask_of`).
fn manual_asks(now: u64) {
    let open: Vec<String> = store::records()
        .iter()
        .filter(|(_, r)| r.asked.as_ref().is_some_and(|a| a.kind == store::AskKind::Manual))
        .map(|(chat, _)| chat.clone())
        .collect();
    for chat in open {
        manual_ask_of(&chat, now);
    }
}

/// `chat`'s ask from the person, a step on: its request typed once its CLI
/// lists the tool and stands at its prompt, strictly, the turn state looked at
/// again under the held keys, as a move's ask is (moving.rs `type_ask`): the
/// notifier looks before it types, and a Codex line typed into a turn the
/// person just began would steer that turn. Closed MANUAL_ASK_MS after it
/// went in, or MANUAL_DELIVER_MS after it was opened if it never could, the
/// chat and the person told, unless a handoff answering it is under way.
/// Nothing is done while a move of the chat is under way (`moving::under_way`),
/// whose start withdraws the ask (moving.rs `taken`): a request typed then
/// would ask for a handoff that resets the chat in the middle of the move.
fn manual_ask_of(chat: &str, now: u64) {
    let Some(a) = store::records().get(chat).and_then(|r| r.asked.clone()).filter(|a| a.kind == store::AskKind::Manual) else {
        return;
    };
    if holds_text(chat) || moving::under_way(chat) {
        return;
    }
    match a.typed {
        // Checked before each try at typing it: one that can never go in
        // would otherwise refuse the person's next ask for good.
        None if now.saturating_sub(a.at) > MANUAL_DELIVER_MS => undelivered(chat, &a),
        None => {
            if is_listed(chat) && chat_listed(chat) && moving::prompt_verdict(crate::pty::manager(), chat).is_ok() {
                type_manual(chat, a.at);
            }
        }
        Some(typed) if now.saturating_sub(typed) > MANUAL_ASK_MS => {
            let closed = store::change(chat, |r| lapsed(r, &a));
            if !closed.unwrap_or_else(|e| {
                crate::log::warn("handoff", &format!("{}: the person's ask could not be saved as lapsed: {e}", label(chat)));
                false
            }) {
                return;
            }
            let minutes = MANUAL_ASK_MS / 60_000;
            crate::log::info("handoff", &format!("{}: the person's ask for a handoff lapsed after {minutes} minutes", label(chat)));
            reg().lock().unwrap_or_else(|e| e.into_inner()).news.push((chat.to_string(), facts::manual_lapsed(minutes)));
            crate::agents::save_chats();
            let name = chat_name(chat).unwrap_or_else(|| chat.to_string());
            crate::power::notify_chat(
                chat,
                "A handoff did not come",
                &format!("\"{name}\" wrote no handoff within {minutes} minutes of your request; ask again from its pane's menu."),
            );
        }
        _ => {}
    }
}

/// The person's ask `a`, its request never typed, closed as undelivered
/// (`lapsed`, so only that same ask and only with no handoff answering it),
/// the chat and the person told: its own word, not the typed request's lapse.
fn undelivered(chat: &str, a: &store::Asked) {
    // Not while its request may be going in right now: the try decides.
    if running().lock().unwrap_or_else(|e| e.into_inner()).contains(chat) {
        return;
    }
    let closed = store::change(chat, |r| lapsed(r, a)).unwrap_or_else(|e| {
        crate::log::warn("handoff", &format!("{}: the person's ask could not be saved as undelivered: {e}", label(chat)));
        false
    });
    if !closed {
        return;
    }
    let minutes = MANUAL_DELIVER_MS / 60_000;
    crate::log::info(
        "handoff",
        &format!("{}: the person's ask for a handoff could not be delivered within {minutes} minutes", label(chat)),
    );
    reg().lock().unwrap_or_else(|e| e.into_inner()).news.push((chat.to_string(), facts::manual_undelivered(minutes)));
    crate::agents::save_chats();
    let name = chat_name(chat).unwrap_or_else(|| chat.to_string());
    crate::power::notify_chat(
        chat,
        "A handoff request could not be delivered",
        &format!(
            "Luna could not type your request into \"{name}\" within {minutes} minutes (it was not at its prompt, or its CLI was not running); the request is withdrawn. Ask again from its pane's menu."
        ),
    );
}

/// The person's ask `a` closed on record `r` as lapsed, if it is still the
/// one there and no handoff answers it: a call may have scheduled one since
/// the look before, which then goes on under the ask it answers. The
/// threshold, whose ask it may have taken the place of, is re-armed
/// (`store::rearmed_unasked`). Whether it closed.
fn lapsed(r: &mut store::Record, a: &store::Asked) -> bool {
    let open = r.switch.is_none() && r.asked.as_ref() == Some(a);
    if open {
        r.asked = None;
        store::rearmed_unasked(r);
    }
    open
}

/// The person's ask opened at `at` taken as typed on record `r` at `now`, with
/// the conversation it goes into, if it is still the one there, not typed yet,
/// and no handoff is under way. Whether it was.
fn manual_typed(r: &mut store::Record, at: u64, now: u64, conversation: String) -> bool {
    if r.switch.is_some() {
        return false;
    }
    match r.asked.as_mut().filter(|x| x.kind == store::AskKind::Manual && x.at == at && x.typed.is_none()) {
        Some(x) => {
            x.typed = Some(now);
            x.conversation = Some(conversation);
            true
        }
        None => false,
    }
}

/// The person's request typed into `chat` on a thread of its own, its
/// terminal claimed, and taken as typed (`manual_typed`) only once the chat is
/// found at its prompt again with the person's keys held. Not typed, it is
/// tried again at a later beat.
fn type_manual(chat: &str, at: u64) {
    if !running().lock().unwrap_or_else(|e| e.into_inner()).insert(chat.to_string()) {
        return;
    }
    let id = chat.to_string();
    let started = std::thread::Builder::new().name("luna-handoff-ask".into()).spawn(move || {
        let _running = Running(id.clone());
        let Some(_typing) = TypingInto::wait_for(&id, TYPING_WAIT) else { return };
        let commit = || {
            if !moving::still_ready(crate::pty::manager(), &id) {
                return Err(Refused::Gone);
            }
            // The request asks the conversation it goes into, and only one
            // Luna can name: an ask bound later might bind to one the person
            // started meanwhile. Not known now, it is typed at a later beat.
            let Some(conversation) = conversation_now(&id) else { return Err(Refused::Gone) };
            match store::change(&id, |r| manual_typed(r, at, now_ms(), conversation.clone())) {
                // Logged here, as it opens: the chat may answer it before the
                // paste below returns.
                Ok(true) => {
                    crate::log::info(
                        "handoff",
                        &format!("{}: the person's ask for a handoff is on record as typed; its line goes in", label(&id)),
                    );
                    Ok(())
                }
                Ok(false) => Err(Refused::Gone),
                Err(e) => Err(Refused::Unsaved(e)),
            }
        };
        match type_line(&id, &facts::manual_notice(), &commit) {
            Ok(Typed::In) => {
                crate::log::info("handoff", &format!("{}: the person's ask for a handoff went in; the CLI took it", label(&id)))
            }
            Ok(Typed::NotClear | Typed::NotCommitted(Refused::Gone)) => {}
            Ok(Typed::NotCommitted(Refused::Unsaved(e))) => {
                crate::log::warn("handoff", &format!("{}: the person's ask could not be saved as typed: {e}", label(&id)))
            }
            Err(e) => crate::log::warn("handoff", &format!("{}: the person's ask could not be typed: {e}", label(&id))),
        }
    });
    if let Err(e) = started {
        running().lock().unwrap_or_else(|e| e.into_inner()).remove(chat);
        crate::log::warn("handoff", &format!("could not type the person's ask into {}: {e}", label(chat)));
    }
}

/// Each handoff under way, a step on: a switch started once its chat is
/// ready, a draft holding it told to the desktop once, and what a core's
/// restart left looked at.
fn switches(b: &mut Beat) {
    let under_way: Vec<(String, Switch)> =
        store::records().iter().filter_map(|(chat, r)| Some((chat.clone(), r.switch.clone()?))).collect();
    b.rest.retain(|c, _| under_way.iter().any(|(u, _)| u == c));
    b.drafts.retain(|c, _| under_way.iter().any(|(u, _)| u == c));
    for (chat, s) in under_way {
        if running().lock().unwrap_or_else(|e| e.into_inner()).contains(&chat) {
            continue;
        }
        // A switch whose thread is gone mid-phase is looked at as a restart's is.
        if matches!(s.phase, Phase::Resetting { .. } | Phase::Prompting { .. }) {
            let _ = Stored.advance(&chat, s.n, s.phase, Phase::Recovering { since: now_ms() }, s.attempts);
            continue;
        }
        let look = Pty.look(&chat, &s);
        let rest = b.rest.entry(chat.clone()).or_default();
        *rest = if look.alive && look.at_rest && !look.draft { *rest + 1 } else { 0 };
        let step = phase::next(&s, &look, *rest);
        if step != Step::Deferred {
            b.drafts.remove(&chat);
        }
        match step {
            Step::Reset | Step::Prompt => start_switch(&chat, &s),
            Step::Deferred => {
                let (since, told) = b.drafts.entry(chat.clone()).or_insert((look.now, false));
                if !*told && look.now.saturating_sub(*since) >= DRAFT_ALERT_MS {
                    *told = true;
                    let name = chat_name(&chat).unwrap_or_else(|| chat.clone());
                    crate::power::notify_chat(
                        &chat,
                        "A handoff is waiting",
                        &format!("\"{name}\" has an unsent draft, so Luna waits to open its new conversation until you send or clear it."),
                    );
                }
            }
            Step::Retry => {
                crate::log::info("handoff", &format!("{} is still on the conversation handoff {} left: trying again", label(&chat), s.n));
                let _ = Stored.advance(&chat, s.n, s.phase, Phase::Scheduled, s.attempts);
            }
            Step::Done => finish(&chat, s.n, Ended::Done, &String::new),
            Step::Left => finish(&chat, s.n, Ended::Left, &String::new),
            Step::Stray => finish(&chat, s.n, Ended::Stray(phase::STRAY.into()), &String::new),
            Step::Fail(why) => {
                // The prompt again for the new conversation, only on the one
                // the reset opened (`phase::failed`) and a copy that is still
                // Luna's.
                let prompt = || if copy_holds(&chat, &s).is_ok() { continuation(&chat, &s) } else { String::new() };
                finish(&chat, s.n, phase::failed(&s, &look, why), &prompt);
            }
            Step::Wait => {}
        }
    }
}

/// What the window's "What Luna applied" panel answers when auto-handoff is
/// set on a chat: whether its CLI is to start again, on the same conversation,
/// for the tool to be listed to it.
#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AutoHandoff {
    pub resume: bool,
}

/// The window's toggle: auto-handoff on at `threshold`, or off. Takes effect
/// in the registry now, and on, the chat's threshold is looked at as soon as
/// it is at rest (`check_now`), not at its next turn's end: a chat already
/// past it is asked then. The tool reaches a CLI only as it lists its tools,
/// at its start: one that started before Luna listed it to every chat with
/// its tools is to be resumed (`resume`) for the threshold to ask it, and the
/// look waits for that. Off closes the threshold's ask, drops its notice not
/// yet typed, and a handoff only scheduled from it is given up; one in the
/// middle of its switch goes on. A person's ask, and a handoff answering it,
/// are theirs: off leaves them be.
pub fn set_auto_handoff(id: String, threshold: Option<u8>) -> Result<AutoHandoff, String> {
    if let Some(t) = threshold.filter(|t| !(THRESHOLD_MIN..=THRESHOLD_MAX).contains(t)) {
        return Err(format!("a threshold of {t}% is not one: from {THRESHOLD_MIN} to {THRESHOLD_MAX}"));
    }
    // The registry's word and what it stops, in one hold of the chat's record
    // (records, then the registry: the order a schedule's commitment takes
    // them in too). Off, past an ask of the threshold's (or a record from
    // before asks were kept): a number reserved for a file being copied is no
    // longer one a switch can be recorded under, and a handoff still only
    // scheduled is given up; a switch that has taken its reset (`advance`,
    // under the same lock) goes on.
    let why = "auto-handoff was turned off for this chat".to_string();
    let only_scheduled = |s: &Switch| s.phase == Phase::Scheduled;
    let apply = |r: &mut store::Record| {
        let tools = set_in_registry(&id, threshold);
        // On again: a conversation told before it was turned off, with nothing
        // asking it now, is asked again if it is still past the threshold.
        if threshold.is_some() {
            store::rearmed_unasked(r);
        }
        if threshold.is_some() || r.asked.as_ref().is_some_and(|a| a.kind == store::AskKind::Manual) {
            return (tools, None);
        }
        r.asked = None;
        r.reserved = None;
        // Its notice not yet typed would ask for what nobody asks any more,
        // and the tool would refuse it: dropped under the lock it was queued
        // under (`ask_auto`).
        reg().lock().unwrap_or_else(|e| e.into_inner()).news.retain(|(c, line)| !(c == &id && facts::is_notice(line)));
        (tools, close(r, None, false, Some(why.clone()), only_scheduled))
    };
    let (tools, closed) = match store::change(&id, apply) {
        Ok(done) => done,
        Err(e) => {
            crate::log::warn("handoff", &format!("{}: auto-handoff's change could not be saved: {e}", label(&id)));
            let mut r = store::records();
            apply(r.entry(id.clone()).or_default())
        }
    };
    if let Some((s, gone)) = closed {
        ended_with(&id, &s, gone, Some(why), false, None, &String::new);
    }
    // After the registry has it: the look reads the threshold from there.
    match threshold {
        Some(_) => check_now(&id),
        None => {
            unmark(&id, None);
            crate::agents::save_chats();
        }
    }
    crate::log::info("handoff", &format!("{}: auto-handoff {}", label(&id), threshold.map_or("off".into(), |t| format!("at {t}%"))));
    // Off needs nothing listed; on, only a CLI that has not listed the tool.
    let resume = tools && threshold.is_some() && crate::pty::manager().alive(&id) && !is_listed(&id);
    Ok(AutoHandoff { resume })
}

/// What the window's Hand off now answers: whether the chat's CLI is to start
/// again, on the same conversation, for the tool to be listed to it; the
/// request is typed once it is.
#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HandoffNow {
    pub resume: bool,
}

/// The pane's Hand off now: the person asks `chat_id` for a handoff. Refused,
/// with what stands in the way, where the chat could not answer: no tool to
/// answer with, no file it can write, a handoff or a move's ask under way, an
/// ask of the person's open already, its CLI's own jobs at work (they cannot
/// cross), its CLI not running. Otherwise the ask is opened, and its request
/// typed by the beat once the chat is at rest (`manual_ask_of`).
pub fn handoff_now(chat_id: String) -> Result<HandoffNow, String> {
    let chat = chat_id.as_str();
    if let Some(why) = now_refusal(chat) {
        return Err(why);
    }
    if !crate::pty::manager().alive(chat) {
        return Err("the chat's CLI is not running: start it in its pane, then ask again".into());
    }
    open_manual(chat)
}

/// Why `chat` is not to be asked for a handoff now, what takes a terminal
/// aside.
fn now_refusal(chat: &str) -> Option<String> {
    if !chat_listed(chat) {
        return Some(
            "only a person's chat started with Luna's tools hands off: this one has none (start a chat with the Luna tools box \
             ticked)"
                .into(),
        );
    }
    let settings = reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(chat).map(|k| k.settings.clone()).unwrap_or_default();
    if let Some(why) = served::unwritable(&settings) {
        return Some(format!("the chat writes no file, its handoff included. {why}"));
    }
    if let Some(n) = store::records().get(chat).and_then(|r| r.switch.as_ref().map(|s| s.n)) {
        return Some(format!("handoff {n} of this chat is under way already"));
    }
    if moving::under_way(chat) {
        return Some(
            "a move of this chat to another account is under way, and no handoff is asked while it is: let it finish or cancel it first"
                .into(),
        );
    }
    if open_ask(chat) == Some(store::AskKind::Manual) {
        return Some(format!(
            "this chat has been asked for a handoff already; it lapses {} minutes after Luna typed the request, or is withdrawn {} minutes after you asked if Luna could not type it",
            MANUAL_ASK_MS / 60_000,
            MANUAL_DELIVER_MS / 60_000
        ));
    }
    let jobs = crate::activity::own_jobs(chat);
    if !jobs.is_empty() {
        return Some(format!(
            "the chat's CLI has its own jobs running ({}), which cannot cross to a new conversation: let them finish or stop \
             them, then ask again",
            jobs.join(", ")
        ));
    }
    None
}

/// The person's ask opened on `chat`, taking the place of one the threshold
/// left open: the request asks for the handoff now.
fn open_manual(chat: &str) -> Result<HandoffNow, String> {
    let turns = crate::activity::turn_state(chat).map(|t| t.turns_ended);
    // Looked at again under the record's lock: a handoff or a move begun
    // since the look before takes the chat. A threshold notice not yet typed
    // would ask a second time: dropped under the same lock, which the
    // threshold queues its notice under (`ask_auto`).
    let opened = store::change(chat, |r| {
        if r.switch.is_some() || r.moving.as_ref().is_some_and(moving::Move::under_way) {
            return false;
        }
        r.ask_closed = None;
        r.asked = Some(store::Asked::new(store::AskKind::Manual, now_ms(), turns));
        reg().lock().unwrap_or_else(|e| e.into_inner()).news.retain(|(c, line)| !(c == chat && facts::is_notice(line)));
        true
    })
    .map_err(|e| format!("the ask could not be saved: {e}"))?;
    if !opened {
        return Err("a handoff or a move of this chat began meanwhile".into());
    }
    crate::agents::save_chats();
    crate::log::info("handoff", &format!("{}: the person asks for a handoff", label(chat)));
    Ok(HandoffNow { resume: !is_listed(chat) })
}

/// Auto-handoff at `threshold` (or off) in `id`'s registry settings; whether it
/// has Luna's tools. Called with its record's lock held.
fn set_in_registry(id: &str, threshold: Option<u8>) -> bool {
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let Some(k) = r.known.get_mut(id) else { return false };
    match threshold {
        Some(t) => k.settings["handoff"] = json!(t),
        None => {
            if let Some(o) = k.settings.as_object_mut() {
                o.remove("handoff");
            }
        }
    }
    k.tools
}

/// Where a chat's latest handoff stands, for its divider in the window.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HandoffMark {
    pub n: u32,
    /// When it was scheduled (under way) or ended.
    pub at: u64,
    /// `underWay`, `done` or `failed`.
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// The conversation it left can be read.
    pub previous: bool,
    /// The document a move to another account opened the chat with.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub moved: bool,
}

/// `id`'s latest handoff, under way or past.
pub fn mark_of(id: &str) -> Option<HandoffMark> {
    let r = store::records();
    let rec = r.get(id)?;
    if let Some(s) = &rec.switch {
        return Some(HandoffMark {
            n: s.n,
            at: s.scheduled_at,
            state: "underWay",
            why: None,
            previous: s.before.transcript.is_some(),
            moved: false,
        });
    }
    let p = rec.past.last()?;
    Some(HandoffMark {
        n: p.n,
        at: p.at,
        state: if p.done { "done" } else { "failed" },
        why: p.why.clone(),
        previous: p.previous.is_some(),
        moved: p.moved,
    })
}

/// A chat asked for a handoff that has not come, for its pane: the share of
/// its context at each notice of its conversation's, and how often its CLI
/// compacted the conversation since, before a handoff.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HandoffAsked {
    pub at: Vec<u64>,
    pub compacted: u32,
}

/// `id` was asked for a handoff in its conversation and none is under way or
/// through since: what it was asked at. Nothing for a chat whose threshold is
/// not set: these are the threshold's notices.
pub fn asked_of(id: &str) -> Option<HandoffAsked> {
    if !auto_on(id) {
        return None;
    }
    let r = store::records();
    let rec = r.get(id).filter(|rec| rec.switch.is_none())?;
    let told = rec.told.as_ref().filter(|t| !t.shares.is_empty())?;
    Some(HandoffAsked { at: told.shares.clone(), compacted: told.compacted })
}

/// The ask for a handoff open on a chat, for its pane: what the menu's Hand
/// off now and the chip go by, so the window keeps no clock of its own.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HandoffAsk {
    /// `auto` (the threshold's) or `manual` (the person's).
    pub kind: &'static str,
    /// When it was opened.
    pub at: u64,
    /// The person's: when its request went into the chat; none while it
    /// waits to be typed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub typed: Option<u64>,
    /// The person's, typed: when it lapses (`MANUAL_ASK_MS`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lapses_at: Option<u64>,
}

/// The ask open on `id`, if one is: the threshold's or the person's,
/// pending or typed. Closed, nothing.
pub fn handoff_ask_of(id: &str) -> Option<HandoffAsk> {
    let a = store::records().get(id).and_then(|r| r.asked.clone())?;
    let manual = a.kind == store::AskKind::Manual;
    Some(HandoffAsk {
        kind: if manual { "manual" } else { "auto" },
        at: a.at,
        typed: a.typed.filter(|_| manual),
        lapses_at: a.typed.filter(|_| manual).map(|t| t + MANUAL_ASK_MS),
    })
}

/// The window's look at the ask open on a chat (`handoff_ask_of`) where
/// `session_meta` has nothing to say, its CLI not running: the ask is the
/// core's whether a CLI runs or not, and one that lapsed meanwhile must not
/// stay on the chip and in the menu's refusal.
pub fn handoff_ask(chat_id: String) -> Option<HandoffAsk> {
    handoff_ask_of(&chat_id)
}

/// Handoff `n` of `chat` as the record has it.
fn past_of(chat: &str, n: u32) -> Result<(String, Option<String>, Provider), String> {
    let r = store::records();
    let rec = r.get(chat).ok_or("this chat has had no handoff")?;
    if let Some(s) = rec.switch.as_ref().filter(|s| s.n == n) {
        return Ok((s.snapshot.clone(), s.before.transcript.clone(), s.provider));
    }
    let p = rec.past.iter().find(|p| p.n == n).ok_or_else(|| format!("no handoff {n} on record"))?;
    Ok((p.snapshot.clone(), p.previous.clone(), p.provider))
}

/// The window's "open handoff": Luna's copy of handoff `n`, as text. The path
/// is the record's, never the window's, and the copy is read only right in
/// the chat's folder of copies as checked (`copies_of`), as the private file
/// Luna made (`secret::read_private`), never through a link put on the way.
pub fn handoff_file(chat_id: String, n: u32) -> Result<String, String> {
    let (snapshot, ..) = past_of(&chat_id, n)?;
    let dir = copies_of(&chat_id)?;
    if Path::new(&snapshot).parent() != Some(dir.as_path()) {
        return Err(format!("handoff {n}'s copy is not in this chat's folder of copies"));
    }
    let bytes = crate::secret::read_private(Path::new(&snapshot), file::MAX_BYTES)
        .map_err(|e| format!("handoff {n}'s copy: {e}"))?
        .ok_or_else(|| format!("handoff {n}'s copy is gone"))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// The window's "previous conversation": the one handoff `n` left, read-only,
/// as a helper's is shown (background/rows.rs `view_of`). The path is the
/// record's, never the window's.
pub fn handoff_transcript(chat_id: String, n: u32, since: Option<u64>) -> Result<crate::background::Transcript, String> {
    let (_, previous, provider) = past_of(&chat_id, n)?;
    let path = previous.ok_or("Luna did not know where the conversation before it was")?;
    Ok(crate::background::view_of(Path::new(&path), provider, since.unwrap_or(0), false))
}

/// `id` is gone for good: its record and its snapshots with it.
pub fn forget(id: &str) {
    unlisted(id);
    unmark(id, None);
    refused_drafts().lock().unwrap_or_else(|e| e.into_inner()).remove(id);
    if let Err(e) = store::remove(id) {
        crate::log::warn("handoff", &format!("{id}'s handoffs could not be forgotten on disk: {e}"));
    }
    // Only where the folder of handoffs and the chat's are Luna's own
    // folders, no link on the way: a planted one would take the removal to
    // wherever it leads.
    let Ok(dir) = dir_of(id) else { return };
    let real_dir = |d: &Path| std::fs::symlink_metadata(d).is_ok_and(|m| m.is_dir() && !crate::boundary::is_link(&m));
    if real_dir(&root()) && real_dir(&dir) {
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod tests;
