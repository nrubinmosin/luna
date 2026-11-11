//! Auto-handoff (docs/adr/0020-session-self-handoff.md). A person's chat with
//! Luna's tools that has it on (`handoff` in its settings: the threshold, in
//! percent of its context window) is told, once a turn ends past it, to write a
//! handoff and call `luna_session_handoff`, the one tool only such a chat is
//! offered: without a file it answers how, with the instructions Luna serves
//! for it (docs/adr/0022-content-luna-serves-at-its-own-trigger.md); with one,
//! Luna snapshots the file, waits for the chat to be at rest, types the CLI's
//! own reset — `/clear` (Claude Code), `/new` (Codex), fixed text — and then
//! the prompt that opens the next conversation from the snapshot and the facts
//! Luna knows. The chat's id, its MCP token, its helpers and what they owe it
//! stay as they are: the CLI's process goes on, and Luna follows it onto the
//! new conversation (read.rs `moved_to`). Notices to the chat and text sent to
//! it wait from the call until the prompt is in. Nothing switches unless the
//! session asks. No other session hears of any of it: not the tool, not a
//! guide topic, not the instructions.
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
mod moving;
mod phase;
mod scrub;
mod served;
mod store;
mod watch;

pub use moving::{
    move_ask, move_cancel, move_confirm, move_list, move_prepare, move_preview, move_step, moved_dir, opening_refusal, MOVE_TAG,
};
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

/// What a chat's What Luna applied panel adds to its auto-handoff where its
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

/// Whether `id` is offered `luna_session_handoff`: a person's own chat (no
/// parent, no background helper, not the dev caller, which has no terminal)
/// with Luna's tools and auto-handoff on. Helpers are their orchestrator's to
/// rotate.
pub fn offers_tool(id: &str) -> bool {
    if id == DEV_CALLER || crate::background::has(id) {
        return false;
    }
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    r.known.get(id).is_some_and(|k| k.tools && k.parent.is_none() && !k.orphan && threshold_in(&k.settings).is_some())
}

/// The chats whose CLI has been given the tool in `tools/list` since it
/// started: one that was not (auto-handoff turned on while it ran) is asked
/// for no handoff until it starts again and lists it.
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

/// What a caller `luna_session_handoff` is not offered to hears, should it
/// call the tool all the same: no word of auto-handoff, which only a chat with
/// it on is to know of. The MCP layer refuses such a call before this, as it
/// does a tool there is not (mcp/dispatch.rs).
pub(super) const NOT_OFFERED: &str = "luna_session_handoff is not one of this session's tools";

/// `luna_session_handoff` without `file`: how the caller's own chat hands off,
/// with the instructions Luna serves for it now (served.rs).
pub fn instructions(caller: &str) -> Result<String, String> {
    served::instructions(caller)
}

/// `luna_session_handoff` with `file`: the caller's own chat, and only that,
/// handed over to a new conversation from it. Checks and copies the file,
/// numbers the handoff and schedules it; the switch waits for the caller's
/// turn to end.
pub fn handoff(caller: &str, file: &str) -> Result<Value, String> {
    if !offers_tool(caller) {
        return Err(NOT_OFFERED.into());
    }
    // Asked for a move's document: collected, nothing reset, so the CLI's
    // own jobs may go on (moving.rs).
    if let Some(answer) = moving::asked_file(caller, file) {
        return answer;
    }
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
    };
    // Under the record's lock, the registry looked at inside it: auto-handoff
    // turned off since the call (`set_auto_handoff`, which takes them in the
    // same order, or the chat started again without it) leaves nothing to
    // record.
    let committed = store::change(caller, |rec| {
        if rec.reserved != Some(n) || rec.switch.is_some() {
            return Err(
                "this chat's handoffs changed while the file was copied (auto-handoff turned off, or another handoff); nothing will switch",
            );
        }
        rec.reserved = None;
        if !offers_tool(caller) {
            return Err("auto-handoff was turned off for this chat while the file was copied; nothing will switch");
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
    // A threshold notice not yet typed would land in the new conversation,
    // asking it for a handoff it has just been given.
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
/// its terminal's title, which `/new` does.
fn conversation_of(pty: &crate::pty::PtyManager, id: &str, provider: Provider) -> Option<String> {
    match provider {
        Provider::Claude => {
            let p = pty.probe(id)?;
            crate::claude::session::session_id(p.pid, p.resume.as_deref(), &p.cwd, p.spawned_at_ms, &p.account_path)
        }
        Provider::Codex => pty.codex_thread(id),
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
            prompted,
        }
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
    let pty = crate::pty::manager();
    // From here to the Enter a person's keys wait (`hold_keys`): what the
    // composer shows now is what the text lands in.
    let hold = pty.hold_keys(chat).ok_or("the chat's terminal is gone")?;
    let written = pty.composer(chat).is_some_and(|c| !c.text.trim().is_empty() || c.stashed);
    if written || pty.has_draft(chat) || pty.typed_ago_ms(chat).is_some_and(|ms| ms < SEND_TYPING_MS) {
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
    let facts = facts::fact_block(&gather(chat));
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

/// What Luna knows of `chat` now, for the prompt.
fn gather(chat: &str) -> facts::Facts {
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
/// was served and asked: the new conversation has its own to come.
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
        if let Some(t) = r.told.as_mut() {
            t.asked.clear();
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
/// for a disk.
fn finish(chat: &str, n: u32, ended: Ended, prompt: &dyn Fn() -> String) {
    let (done, why, reset) = match ended {
        Ended::Back => return,
        Ended::Done => (true, None, true),
        Ended::Failed { why, reset } => (false, Some(why), reset),
    };
    let closed = match store::change(chat, |r| close(r, Some(n), done, why.clone(), |_| true)) {
        Ok(closed) => closed,
        Err(e) => {
            crate::log::warn("handoff", &format!("{}: handoff {n} could not be saved as ended: {e}", label(chat)));
            store::records().get_mut(chat).and_then(|r| close(r, Some(n), done, why.clone(), |_| true))
        }
    };
    let Some((s, gone)) = closed else { return };
    ended_with(chat, &s, gone, why, reset, prompt);
}

/// What follows a handoff's end: the copies dropped from the record go, and one
/// given up is told to the chat and the person.
fn ended_with(chat: &str, s: &Switch, gone: Vec<store::Past>, why: Option<String>, reset: bool, prompt: &dyn Fn() -> String) {
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
            let line = facts::failed_line(s.n, &why, &s.snapshot, reset, offers_tool(chat), &prompt);
            reg().lock().unwrap_or_else(|e| e.into_inner()).news.push((chat.to_string(), line));
            crate::agents::save_chats();
            let name = chat_name(chat).unwrap_or_else(|| chat.to_string());
            crate::power::notify_chat(chat, "A handoff did not go through", &format!("\"{name}\": {why}"));
        }
    }
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

/// Runs for the life of the core: every few seconds, each chat with
/// auto-handoff on is looked at for its threshold once a turn has ended, and
/// each handoff under way is taken a step on.
pub(super) fn beat() {
    let mut b = Beat::default();
    loop {
        std::thread::sleep(NOTIFY_EVERY);
        let pty = crate::pty::manager();
        thresholds(pty, &mut b);
        switches(&mut b);
        moving::beat();
    }
}

/// The chats with auto-handoff on: id, threshold, provider.
fn opted_in() -> Vec<(String, u8, Provider)> {
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    r.known
        .iter()
        .filter(|(id, k)| *id != DEV_CALLER && k.tools && k.parent.is_none() && !k.orphan)
        .filter_map(|(id, k)| Some((id.clone(), threshold_in(&k.settings)?, k.provider)))
        .collect()
}

/// The threshold notices due, each queued for the notifier (`Registry::news`),
/// which types it in once the chat is at rest, a draft respected.
fn thresholds(pty: &crate::pty::PtyManager, b: &mut Beat) {
    let chats = opted_in();
    b.turns.retain(|id, _| chats.iter().any(|(c, ..)| c == id));
    for (chat, threshold, provider) in chats {
        if holds_text(&chat) || moving::asking(&chat) || !is_listed(&chat) {
            continue;
        }
        let Some(ts) = crate::activity::turn_state(&chat).filter(|t| t.turn == crate::activity::Turn::Idle) else { continue };
        if b.turns.insert(chat.clone(), ts.turns_ended) == Some(ts.turns_ended) {
            continue;
        }
        let Some(path) = transcript_path(pty, &chat) else { continue };
        let Some(context) = crate::background::Context::of(&path, provider) else { continue };
        let conversation = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let (told, fresh) = store::records().get(&chat).map(|r| (r.told.clone(), r.fresh)).unwrap_or_default();
        let now = watch::told_now(told.as_ref(), &conversation, context.pct, context.tokens, fresh, threshold);
        if let Some(t) = told.as_ref().filter(|t| t.conversation == now.conversation && t.level > now.level) {
            let why = if now.compacted > t.compacted {
                "its CLI compacted the conversation".to_string()
            } else {
                format!("its context fell with no fewer tokens ({}, a larger window?)", context.tokens)
            };
            crate::log::info(
                "handoff",
                &format!(
                    "{}: {why}, now at {}%, before a handoff (asked at {:?}); it will be asked again",
                    label(&chat),
                    context.pct,
                    now.asked
                ),
            );
        }
        let level = watch::level(context.pct, threshold, now.floor);
        if !watch::due(&now, level) {
            // A conversation looked at for the first time keeps its floor, and
            // one whose context fell is to be asked again.
            if told.as_ref() != Some(&now) {
                let kept = store::change(&chat, |r| {
                    r.told = Some(now);
                    r.fresh = false;
                });
                if let Err(e) = kept {
                    crate::log::warn("handoff", &format!("{}: where its context stands could not be saved: {e}", label(&chat)));
                }
            }
            continue;
        }
        // The firm notice carries the checklist where Luna's own instructions
        // are what the chat would be served.
        let firm = level == 2;
        let inline = firm && crate::library::handoff_instructions().source == crate::library::Source::Luna;
        // Told once it is on disk: a notice a later core would repeat is the
        // smaller harm than none, so a save that fails still lets it go.
        let mut asked = now.asked.clone();
        asked.push(context.pct);
        let told = store::Told { level, asked, tokens: context.tokens, ..now };
        // A fresh handoff request: from it on, what the chat sends is its own,
        // not an earlier ask's late answer (moving.rs `late_answer`).
        if let Err(e) = store::change(&chat, |r| {
            told_of(r, told, inline);
            r.ask_closed = None;
        }) {
            crate::log::warn("handoff", &format!("{}: the notice it was told could not be saved: {e}", label(&chat)));
        }
        let line = facts::notice(context.pct, threshold, firm, inline);
        reg().lock().unwrap_or_else(|e| e.into_inner()).news.push((chat.clone(), line));
        crate::agents::save_chats();
        crate::log::info(
            "handoff",
            &format!("{} is at {}% of its context: asked for a handoff (level {level})", label(&chat), context.pct),
        );
    }
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
            Step::Fail(why) => {
                let reset = look.conversation.is_some() && look.conversation != s.before.conversation;
                // The prompt again for the new conversation, only on a copy
                // that is still Luna's.
                let prompt = || if copy_holds(&chat, &s).is_ok() { continuation(&chat, &s) } else { String::new() };
                finish(&chat, s.n, Ended::Failed { why, reset }, &prompt);
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
/// in the registry now, so the threshold and the tool's refusal follow at
/// once; the tool itself reaches a CLI only as it lists its tools, at its
/// start, so a chat running without it is to be resumed (`resume`). Off, a
/// handoff only scheduled is given up; one in the middle of its switch goes on.
pub fn set_auto_handoff(id: String, threshold: Option<u8>) -> Result<AutoHandoff, String> {
    if let Some(t) = threshold.filter(|t| !(THRESHOLD_MIN..=THRESHOLD_MAX).contains(t)) {
        return Err(format!("a threshold of {t}% is not one: from {THRESHOLD_MIN} to {THRESHOLD_MAX}"));
    }
    // The registry's word and what it stops, in one hold of the chat's record
    // (records, then the registry: the order a schedule's commitment takes
    // them in too). Off: a number reserved for a file being copied is no
    // longer one a switch can be recorded under, and a handoff still only
    // scheduled is given up; a switch that has taken its reset (`advance`,
    // under the same lock) goes on.
    let why = "auto-handoff was turned off for this chat".to_string();
    let only_scheduled = |s: &Switch| s.phase == Phase::Scheduled;
    let apply = |r: &mut store::Record| {
        let tools = set_in_registry(&id, threshold);
        if threshold.is_some() {
            return (tools, None);
        }
        r.reserved = None;
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
        ended_with(&id, &s, gone, Some(why), false, &String::new);
    }
    crate::log::info("handoff", &format!("{}: auto-handoff {}", label(&id), threshold.map_or("off".into(), |t| format!("at {t}%"))));
    let resume = tools && threshold.is_some() && crate::pty::manager().alive(&id) && !is_listed(&id);
    Ok(AutoHandoff { resume })
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
/// through since: what it was asked at. Nothing for a chat not offered the tool.
pub fn asked_of(id: &str) -> Option<HandoffAsked> {
    if !offers_tool(id) {
        return None;
    }
    let r = store::records();
    let rec = r.get(id).filter(|rec| rec.switch.is_none())?;
    let told = rec.told.as_ref().filter(|t| !t.asked.is_empty())?;
    Some(HandoffAsked { at: told.asked.clone(), compacted: told.compacted })
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
