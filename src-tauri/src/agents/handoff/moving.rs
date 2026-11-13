//! Moving a person's chat to another account of the same CLI
//! (docs/adr/0023-moving-a-chat-to-another-account.md). Its conversation
//! stays where it is: a new chat opens on the other account from a document
//! the person has read, edited if they like, and confirmed, and Luna types
//! one fixed line pointing at it once that chat stands at a prompt.
//!
//! Every phase is on disk (store.rs, on the old chat's record) before what
//! follows it, and each can be taken again without doing anything twice:
//!
//! Shown (the preview built and handed to the window) → Confirmed (the text,
//! the destination and the launch bound by one hash, the document copied) →
//! OldStopped (where the old chat is to stop, which a Claude Code worktree
//! always calls for) → RowCreated (the window made the new chat's row, which
//! starts its CLI) → Prompting (the line typed) → Done (the new transcript
//! holds it). Failed and Canceled end one where it stands.
//!
//! The window does what is its own (stopping the old chat's terminal, making
//! the row) and says so (`move_step`); the core looks before it takes the
//! step. A preview a core restart finds unconfirmed is given up, never sent
//! (store.rs `from_bytes`). A new chat's start is refused while its move
//! still needs the old chat, in a Claude Code worktree, to be stopped
//! (`opening_refusal`): the session that made a worktree removes it as it
//! quits when it is clean, whoever else is in it (docs/cli-compatibility.md).
//! A Codex chat in a worktree does not move: its worktree is in the old
//! account's folder, beside that account's credentials.

use super::document::{self, HelperFact, Inputs, LastHandoff, Stopped};
use super::phase::{Refused, Typed};
use super::{copies_of, excerpts, gitfacts, scrub, store};
use crate::agents::delivery::{TypingInto, SEND_TYPING_MS, TYPING_WAIT};
use crate::agents::registry::{chat_name, label, reg, DEV_CALLER};
use crate::provider::Provider;
use crate::throttle::now_ms;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

/// How the line Luna types into the moved chat opens.
pub const MOVE_TAG: &str = "[Luna move";

/// How long the document gets to be built: its reads and git are bounded on
/// their own; this bounds the call.
pub(super) const BUILD_WAIT: Duration = Duration::from_secs(30);
/// How long a new chat's CLI may be not running before its opening is given
/// up, said on its row: a pane starts it within seconds of its row, and a core
/// restart gives it this long again (`taken_up`).
pub(super) const NOT_RUNNING_MAX_MS: u64 = 10 * 60_000;
/// How long the new conversation gets to hold the line once typed.
pub(super) const PROMPT_WAIT_MS: u64 = 60_000;
/// How long an ended move stays on record for the window to read.
const ENDED_KEPT_MS: u64 = 7 * 24 * 60 * 60_000;

/// Where the document came from.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(in crate::agents) enum Source {
    /// Luna wrote it from the chat's records (document.rs).
    Luna,
    /// The chat wrote it, asked through its handoff tool.
    Chat,
}

/// How an ask for the chat's document ended without one, where the
/// reason says whether asking again makes sense; none on record is any
/// other (a restart, the ask not typed).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(in crate::agents) enum AskEnd {
    /// The chat stopped on a usage limit.
    Limit,
    /// Its CLI was not running.
    Exited,
    /// It did not come to rest, or wrote nothing, in time.
    Timeout,
    /// It went on in another conversation than the one asked.
    Left,
}

/// Where a move stands.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "phase", rename_all = "camelCase")]
pub(in crate::agents) enum MovePhase {
    /// The chat is to be asked for its document, once at a strict prompt.
    Asking {
        at: u64,
    },
    /// The ask is typed; its document is awaited through its handoff tool.
    Asked {
        at: u64,
    },
    Shown {
        at: u64,
    },
    Confirmed {
        at: u64,
    },
    OldStopped {
        at: u64,
    },
    RowCreated {
        at: u64,
    },
    Prompting {
        at: u64,
    },
    Done {
        at: u64,
    },
    Failed {
        at: u64,
        why: String,
    },
    Canceled {
        at: u64,
    },
}

impl MovePhase {
    /// Its order: what a step may move on from.
    fn rank(&self) -> u8 {
        match self {
            MovePhase::Asking { .. } | MovePhase::Asked { .. } | MovePhase::Shown { .. } => 0,
            MovePhase::Confirmed { .. } => 1,
            MovePhase::OldStopped { .. } => 2,
            MovePhase::RowCreated { .. } => 3,
            MovePhase::Prompting { .. } => 4,
            MovePhase::Done { .. } | MovePhase::Failed { .. } | MovePhase::Canceled { .. } => 5,
        }
    }

    pub(super) fn ended(&self) -> bool {
        self.rank() == 5
    }

    fn at(&self) -> u64 {
        match self {
            MovePhase::Asking { at }
            | MovePhase::Asked { at }
            | MovePhase::Shown { at }
            | MovePhase::Confirmed { at }
            | MovePhase::OldStopped { at }
            | MovePhase::RowCreated { at }
            | MovePhase::Prompting { at }
            | MovePhase::Done { at }
            | MovePhase::Failed { at, .. }
            | MovePhase::Canceled { at } => *at,
        }
    }

    fn word(&self) -> &'static str {
        match self {
            MovePhase::Asking { .. } => "asking",
            MovePhase::Asked { .. } => "asked",
            MovePhase::Shown { .. } => "shown",
            MovePhase::Confirmed { .. } => "confirmed",
            MovePhase::OldStopped { .. } => "oldStopped",
            MovePhase::RowCreated { .. } => "rowCreated",
            MovePhase::Prompting { .. } => "prompting",
            MovePhase::Done { .. } => "done",
            MovePhase::Failed { .. } => "failed",
            MovePhase::Canceled { .. } => "canceled",
        }
    }
}

/// A move, as the old chat's record keeps it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(in crate::agents) struct Move {
    pub id: String,
    pub new_id: String,
    pub provider: Provider,
    pub from_account: String,
    pub to_account: String,
    pub to_account_path: String,
    /// The old chat's folder, which the new one is filed under and starts in.
    #[serde(default)]
    pub folder: String,
    /// The line Luna types into the new chat, as the preview showed it.
    #[serde(default)]
    pub line: String,
    pub source: Source,
    /// The old chat is stopped before the new one starts.
    pub stop_old: bool,
    /// The old chat ran in a Claude Code worktree of its own, which the new
    /// one goes into by name.
    #[serde(default)]
    pub worktree: bool,
    pub phase: MovePhase,
    /// The SHA-256 of the text as it was shown, before any edit.
    pub shown_sha256: String,
    /// What Send was bound to: the text's hash, the destination and the
    /// launch settings (`binding`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch: Option<Value>,
    /// Luna's copy of the confirmed text, in the new chat's folder of copies,
    /// and its SHA-256.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_sha256: Option<String>,
    /// The person's note, for a document the chat writes: Luna adds it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// A document the chat wrote, as the preview is to show it, kept private
    /// in the old chat's folder of copies until the move ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    /// What cleaning that preview took out: secret shapes, paths, characters
    /// no screen shows.
    #[serde(default)]
    pub cleaned: [usize; 3],
    /// The person said, on the new chat, to type the line now: no prompt the
    /// screen check could read, but one they see.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub forced: bool,
    /// When Luna typed its ask into the chat: the turn under way since is the
    /// ask's, not the chat's own work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked_at: Option<u64>,
    /// The chat's turns ended as the ask was typed, between turns: the ask's
    /// turn is the one under way while no more have ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked_turns: Option<u32>,
    /// How its ask ended without the chat's document, where it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask_end: Option<AskEnd>,
    /// The conversation the ask was typed into, as `phase::Before::conversation`
    /// names one: the only one whose call it answers (`ask_here`). The
    /// person's own `/clear` or `/new` since starts a conversation nobody asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked_conversation: Option<String>,
}

impl Move {
    /// The chat is being asked for its document.
    pub(super) fn asking(&self) -> bool {
        matches!(self.phase, MovePhase::Asking { .. } | MovePhase::Asked { .. })
    }

    /// The move has not ended: from its ask or preview until it is done,
    /// failed or canceled.
    pub(super) fn under_way(&self) -> bool {
        !self.phase.ended()
    }

    /// The chat was asked for its document, and the move failed before one
    /// came: the person may have Luna build it from records, or ask again.
    pub(super) fn ask_failed(&self) -> bool {
        matches!(self.phase, MovePhase::Failed { .. }) && self.source == Source::Chat && self.preview.is_none() && self.binding.is_none()
    }
}

/// The new chat's side: which move opened it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(in crate::agents) struct MovedFrom {
    pub chat: String,
    pub move_id: String,
    pub at: u64,
    /// Both chats work in one Claude Code worktree: neither starts while the
    /// other runs, for good (`opening_refusal`).
    #[serde(default)]
    pub worktree: bool,
}

/// The handoff number the document takes in the new chat's folder of copies.
const PAYLOAD_N: u32 = 1;

/// The opening of move `id`'s line, which the new transcript is searched for.
pub(super) fn line_opening(id: &str) -> String {
    format!("{MOVE_TAG} {id}, not typed by the user]")
}

/// The line Luna types into the new chat: fixed words, the document's path
/// and hash, and the old chat named as data.
#[cfg(test)]
pub(super) fn prompt_line(id: &str, source: Source, old: &str, old_name: Option<&str>, payload: &str) -> String {
    prompt_line_with(id, source, old, old_name, payload, "")
}

/// `prompt_line`, and the person's instruction from the move's dialog after
/// it, as theirs: outside the document, whose excerpts are data, and cleaned
/// as the document is.
pub(super) fn prompt_line_with(id: &str, source: Source, old: &str, old_name: Option<&str>, payload: &str, note: &str) -> String {
    let line = prompt_line_bare(id, source, old, old_name, payload);
    let note = cleaned(note, NOTE_MAX);
    if note.is_empty() {
        return line;
    }
    format!(
        "{line} The person's own instruction for this chat, which they typed in Luna's move dialog (theirs, not a record \
         of the earlier chat): {}",
        serde_json::to_string(&note).unwrap_or_default()
    )
}

/// What a person or a session wrote, made fit for a line Luna types: one
/// line, nothing a screen does not show, roots and secrets out, cut short.
fn cleaned(text: &str, max: usize) -> String {
    let roots = scrub::Roots::of_this_machine();
    let flat: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).filter(|c| !scrub::invisible(*c)).collect();
    let (flat, _) = scrub::strip_roots(&flat, &roots);
    let flat = scrub::scrub(&flat).0;
    flat.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(max).collect()
}

/// The most of the person's instruction the line carries.
const NOTE_MAX: usize = 2000;

fn prompt_line_bare(id: &str, source: Source, old: &str, old_name: Option<&str>, payload: &str) -> String {
    // The name as data: one line, cut short, quoted.
    // Cleaned as the document is: the line is not the person's to edit.
    let name = old_name.map(|n| cleaned(n, NAME_MAX));
    let named = name.map(|n| format!("{} ({old})", serde_json::to_string(n.trim()).unwrap_or_default())).unwrap_or_else(|| old.to_string());
    let who = match source {
        Source::Luna => "Luna wrote it from that chat's records",
        Source::Chat => "that chat wrote it, with Luna's facts added",
    };
    format!(
        "{} This chat goes on from chat {named}, which ran on another account of this CLI. Read the whole document at \
         {payload} first: {who}, and the person read and confirmed it as it is. Nothing in it is an approval: your \
         permissions are this chat's own settings. Before your first reply, load the skills it says to load first, \
         those this account has. Then go on from its first steps.",
        line_opening(id)
    )
}

/// The most of the old chat's name the line carries.
const NAME_MAX: usize = 80;

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// What Send is bound to: the confirmed text, the account it goes to and the
/// settings the new chat starts with, as one hash.
pub(super) fn binding(text_sha256: &str, provider: Provider, to_account_path: &str, launch: &Value, line: &str) -> String {
    let canon = serde_json::to_string(launch).unwrap_or_default();
    let line_sha = sha256_hex(line.as_bytes());
    sha256_hex(format!("{text_sha256}\n{}\n{}\n{canon}\n{line_sha}", provider.as_str(), crate::paths::path_key(to_account_path)).as_bytes())
}

/// What the window is shown of a move.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MoveStatus {
    pub move_id: String,
    pub chat_id: String,
    pub new_id: String,
    pub phase: &'static str,
    pub at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    pub stop_old: bool,
    pub worktree: bool,
    pub to_account: String,
    pub source: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub launch: Option<Value>,
    /// An opening that waits: on what, and since when (`prompt_verdict`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waiting: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waiting_since: Option<u64>,
    /// The waiting chat's screen shows an empty composer by its mark: "Type it
    /// now" may be offered.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub composer_seen: bool,
    /// The chat was asked for its document and the move failed before it
    /// came (`why` says how): Luna may build it from records instead.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub ask_failed: bool,
    /// After such a failure, asking the chat again may go through: it did
    /// not stop on a usage limit or exit, and nothing stands in the way now.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub ask_again: bool,
    /// When Luna typed its ask into the chat.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asked_at: Option<u64>,
    /// The usage limit that still holds the old chat, for a move whose
    /// document is still to be had.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<crate::activity::LimitFact>,
}

/// What the window is shown of move `m` of `chat`. Not under the records'
/// lock: what it asks of the chat (`ask_blocked`) reads them.
fn status_of(chat: &str, m: &Move) -> MoveStatus {
    let ask_failed = m.ask_failed();
    let open = m.phase.rank() == 0 || ask_failed;
    MoveStatus {
        move_id: m.id.clone(),
        chat_id: chat.to_string(),
        new_id: m.new_id.clone(),
        phase: m.phase.word(),
        at: m.phase.at(),
        why: match &m.phase {
            MovePhase::Failed { why, .. } => Some(why.clone()),
            _ => None,
        },
        stop_old: m.stop_old,
        worktree: m.worktree,
        to_account: m.to_account.clone(),
        source: match m.source {
            Source::Luna => "luna",
            Source::Chat => "chat",
        },
        launch: m.launch.clone(),
        waiting: waiting_of(m).map(|(why, _)| why),
        waiting_since: waiting_of(m).map(|(_, since)| since),
        composer_seen: waiting_of(m).is_some() && seen_of(m),
        ask_failed,
        ask_again: ask_failed && old_of(chat).is_ok_and(|old| may_ask_again(m, ask_blocked(chat, &old.settings, old.provider).as_deref())),
        asked_at: m.asked_at,
        limit: open.then(|| crate::activity::limit_in_force(chat, m.provider)).flatten(),
    }
}

/// Whether asking the chat again makes sense after move `m`'s ask failed,
/// with what blocks an ask now: not after a usage limit or its CLI exiting,
/// which the person sees to first; after a timeout or the like, once
/// nothing blocks it.
pub(super) fn may_ask_again(m: &Move, blocked: Option<&str>) -> bool {
    m.ask_failed() && !matches!(m.ask_end, Some(AskEnd::Limit | AskEnd::Exited)) && blocked.is_none()
}

/// The preview the window shows, whole: the line Luna will type, fixed, and
/// the document, which the person may edit.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MovePreview {
    pub move_id: String,
    pub new_id: String,
    pub prompt: String,
    pub text: String,
    pub sha256: String,
    pub redacted: usize,
    pub paths_cut: usize,
    pub highlights: Vec<String>,
    pub invisible: usize,
    pub stop_old: bool,
    pub worktree: bool,
    pub warnings: Vec<String>,
    pub max_bytes: usize,
}

/// What the old chat is, as a move takes it.
struct Old {
    provider: Provider,
    account: String,
    account_path: String,
    folder: String,
    settings: Value,
}

fn old_of(chat: &str) -> Result<Old, String> {
    if chat == DEV_CALLER || crate::background::has(chat) {
        return Err("only a chat of yours moves to another account".into());
    }
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let k = r.known.get(chat).ok_or("Luna does not know this chat yet: start it once, then move it")?;
    if k.parent.is_some() || k.orphan {
        return Err("a helper does not move to another account: its orchestrator's account is its own".into());
    }
    Ok(Old {
        provider: k.provider,
        account: k.account.clone(),
        account_path: k.account_path.clone(),
        folder: k.folder.clone(),
        settings: k.settings.clone(),
    })
}

/// Why a chat of `provider` in `folder` on the account at `account_path` does
/// not move: a Codex worktree is in its account's own folder.
pub(super) fn codex_worktree_refusal(provider: Provider, folder: &str, account_path: &str, started_in_one: bool) -> Option<String> {
    let inside =
        crate::paths::path_key(folder).starts_with(&format!("{}{}", crate::paths::path_key(account_path), std::path::MAIN_SEPARATOR));
    (provider == Provider::Codex && (inside || started_in_one)).then(|| {
        "a Codex chat in a worktree does not move: Codex keeps the worktree in the old account's folder, beside its credentials, \
         where another account's chat is not to work"
            .to_string()
    })
}

/// Why the account at `path` takes no move: nobody is signed in to it, and
/// the new chat would meet a sign-in, not a prompt.
pub(super) fn signed_out_refusal(provider: Provider, path: &str, name: &str) -> Option<String> {
    (!crate::agents::spawn::account::signed_in(provider, path))
        .then(|| format!("{name} is signed out: sign in to it in Accounts first, then move the chat there"))
}

/// The account of Luna''s at `path`, by name, and its CLI.
fn account_at(path: &str) -> Result<(Provider, String), String> {
    let key = crate::paths::path_key(path);
    crate::accounts::list_accounts()?
        .into_iter()
        .find(|a| crate::paths::path_key(&a.path) == key)
        .map(|a| (a.provider, a.name))
        .ok_or_else(|| "that account is not one of Luna's (deleted meanwhile?)".to_string())
}

/// How the chat stands now, for the document.
fn stopped_of(chat: &str, provider: Provider) -> Stopped {
    let pty = crate::pty::manager();
    if let Some(kind) = crate::activity::limit_of(chat, provider) {
        return Stopped::Limit(kind.to_string());
    }
    if !pty.alive(chat) {
        return Stopped::NotRunning;
    }
    match crate::activity::turn_state(chat).map(|t| t.turn) {
        Some(crate::activity::Turn::Busy | crate::activity::Turn::Waiting) => Stopped::MidTurn,
        _ => Stopped::Finished,
    }
}

/// The chat's transcript: as its CLI runs it, else by the conversation the
/// window knows it by.
fn transcript_of(chat: &str, provider: Provider, account_path: &str, session_id: Option<&str>) -> Option<PathBuf> {
    if let Some(p) = crate::agents::read::transcript_path(crate::pty::manager(), chat) {
        return Some(p);
    }
    let sid = session_id?;
    match provider {
        Provider::Claude => crate::claude::session::find_transcript(account_path, sid),
        Provider::Codex => crate::codex::session::rollout_of_thread(account_path, sid),
    }
}

/// The chat's last handoff that went through, its copy read as Luna made it
/// (`handoff_file` checks where it is and reads it private) and its hash the
/// one on record.
pub(super) fn last_handoff(chat: &str) -> Option<LastHandoff> {
    let p = store::records().get(chat)?.past.iter().rev().find(|p| p.done && !p.moved)?.clone();
    let text = super::handoff_file(chat.to_string(), p.n).ok()?;
    (sha256_hex(text.as_bytes()) == p.sha256).then(|| LastHandoff { n: p.n, at: crate::background::iso(p.at).unwrap_or_default(), text })
}

pub(super) fn helpers_of(chat: &str) -> (Vec<HelperFact>, usize) {
    let listing = crate::agents::list::list(chat, None, false);
    let owed: Vec<String> =
        reg().lock().unwrap_or_else(|e| e.into_inner()).owed.keys().filter(|(c, _)| c == chat).map(|(_, id)| id.clone()).collect();
    let helpers: Vec<HelperFact> = listing
        .sessions
        .iter()
        .map(|row| HelperFact {
            id: row.id.clone(),
            name: row.name.clone(),
            state: row.state.to_string(),
            owes: owed.contains(&row.id),
            undelivered: crate::agents::lost::undelivered_for(&row.id, chat, row.parent.as_deref() == Some(chat)).len(),
        })
        .collect();
    (helpers, owed.len())
}

/// The window's "Move to another account…": the document built, kept as
/// shown, and handed back with the line Luna would type. Nothing goes
/// anywhere until `move_confirm`.
pub fn move_prepare(
    chat_id: String,
    new_id: String,
    to_account_path: String,
    session_id: Option<String>,
    note: String,
) -> Result<MovePreview, String> {
    let chat = chat_id.as_str();
    let (old, to_account, worktree, warnings) = checked(chat, &new_id, &to_account_path)?;
    let name = chat_name(chat);
    let session_id = session_id.filter(|s| !s.trim().is_empty());
    let note_for_line = note.clone();
    let mut inputs = inputs_of(chat, &old, &to_account, note);
    inputs.last_handoff = last_handoff(chat);
    let transcript = transcript_of(chat, old.provider, &old.account_path, session_id.as_deref());
    let built = build_within(inputs, transcript, ask_span(chat), BUILD_WAIT)?;
    let id = crate::secret::random_hex(6);
    let sha256 = sha256_hex(built.text.as_bytes());
    let payload = super::dir_of(&new_id).map(|d| d.join(format!("{PAYLOAD_N}.md"))).unwrap_or_default();
    let prompt = prompt_line_with(&id, Source::Luna, chat, name.as_deref(), &payload.to_string_lossy(), &note_for_line);
    let mut m = new_move(&id, &new_id, &old, to_account, to_account_path, prompt, Source::Luna, worktree);
    m.phase = MovePhase::Shown { at: now_ms() };
    m.shown_sha256 = sha256.clone();
    let withdrawn = store::change(chat, |r| taken(chat, r, &m)).map_err(|e| format!("the move could not be saved: {e}"))??;
    crate::agents::save_chats();
    if withdrawn {
        ask_withdrawn(chat);
    }
    crate::log::info("move", &format!("{} has a move to another account built ({id}, {} bytes)", label(chat), built.text.len()));
    Ok(preview_of(&m, built.text, [built.redacted, built.paths_cut, built.invisible], warnings))
}

/// What starting a move is refused with while a handoff of the chat is
/// scheduled, or its file is being copied: the reset it brings would come in
/// the middle of the move.
const HANDOFF_UNDER_WAY: &str = "a handoff of this chat is under way; move it once that is through";

/// Move `m`, as it begins by its ask or from records, taken on `chat`'s
/// record `r`, unless a handoff of the chat is scheduled or its file is being
/// copied (`HANDOFF_UNDER_WAY`), looked at again here under the record's lock.
/// An ask of a move before it closed has its answer late from now, and the
/// threshold's ask is retired (`threshold_retired`). The person's own ask for
/// a handoff still open, which no handoff answers yet (that one would be
/// scheduled), is withdrawn: the move is their later word, and a handoff
/// answering the ask would reset the chat in the middle of it. Its request not
/// typed yet is dropped with the threshold's notice, and one being typed now
/// finds it gone (mod.rs `manual_typed`). Whether it withdrew one, for the
/// person to be told (`ask_withdrawn`).
fn taken(chat: &str, r: &mut store::Record, m: &Move) -> Result<bool, &'static str> {
    if r.switch.is_some() || r.reserved.is_some() {
        return Err(HANDOFF_UNDER_WAY);
    }
    let withdrawn = r.asked.as_ref().is_some_and(|a| a.kind == store::AskKind::Manual);
    if withdrawn {
        r.asked = None;
    }
    threshold_retired(chat, r);
    // An ask this move takes the place of: its answer is late from now.
    if let Some(old) = r.moving.as_ref().filter(|o| matches!(o.phase, MovePhase::Asked { .. })) {
        r.ask_closed = Some((old.id.clone(), now_ms()));
        r.ask_closed_turns = crate::activity::turn_state(chat).map(|t| t.turns_ended);
    }
    r.moving = Some(m.clone());
    Ok(withdrawn)
}

/// The person told, in the chat's pane and on the desktop, that the move they
/// began took the place of the handoff they had asked `chat` for (`taken`).
fn ask_withdrawn(chat: &str) {
    crate::log::info("move", &format!("{}: the person's ask for a handoff is withdrawn; the move takes its place", label(chat)));
    let name = chat_name(chat).unwrap_or_else(|| chat.to_string());
    crate::power::notify_chat(
        chat,
        "A move took the place of a handoff",
        &format!(
            "Your request for a handoff of \"{name}\" is withdrawn: the move to another account you began takes its place. Ask again from its pane's menu once the move is through."
        ),
    );
}

/// The threshold's ask on `chat`'s record `r` retired as a move of the chat
/// begins, by its ask or from records: the threshold is quiet for the whole
/// move (`under_way`), and a handoff it asked for would reset the chat in the
/// middle of it. Its notice not typed yet is dropped under the same lock the
/// threshold queues it under (mod.rs `ask_auto`), so one queued just before
/// cannot be left behind; one already on its way into the chat the notifier
/// drops there (mod.rs `withheld`). The person's ask is not the threshold's
/// to retire. Re-armed as the move ends with the chat not stopped
/// (`end_where`).
fn threshold_retired(chat: &str, r: &mut store::Record) {
    if r.asked.as_ref().is_some_and(|a| a.kind == store::AskKind::Auto) {
        r.asked = None;
    }
    reg().lock().unwrap_or_else(|e| e.into_inner()).news.retain(|(c, line)| !(c == chat && super::facts::is_notice(line)));
}

/// A move of the chat `old` to `new_id`, as it begins.
#[allow(clippy::too_many_arguments)]
fn new_move(
    id: &str,
    new_id: &str,
    old: &Old,
    to_account: String,
    to_account_path: String,
    line: String,
    source: Source,
    worktree: bool,
) -> Move {
    Move {
        id: id.to_string(),
        new_id: new_id.to_string(),
        provider: old.provider,
        from_account: old.account.clone(),
        to_account,
        to_account_path,
        folder: old.folder.clone(),
        line,
        source,
        stop_old: worktree,
        worktree,
        phase: MovePhase::Asking { at: now_ms() },
        shown_sha256: String::new(),
        binding: None,
        launch: None,
        payload: None,
        payload_sha256: None,
        note: String::new(),
        preview: None,
        cleaned: [0; 3],
        forced: false,
        asked_at: None,
        asked_turns: None,
        ask_end: None,
        asked_conversation: None,
    }
}

/// What the window is shown of move `m` with its document `text`.
fn preview_of(m: &Move, text: String, cleaned: [usize; 3], warnings: Vec<String>) -> MovePreview {
    MovePreview {
        move_id: m.id.clone(),
        new_id: m.new_id.clone(),
        prompt: m.line.clone(),
        sha256: sha256_hex(text.as_bytes()),
        highlights: scrub::highlights(&text),
        text,
        redacted: cleaned[0],
        paths_cut: cleaned[1],
        invisible: cleaned[2],
        stop_old: m.worktree,
        worktree: m.worktree,
        warnings,
        max_bytes: document::MAX_BYTES,
    }
}

/// What the document is written from, but its transcript and git, which
/// `build_within` reads off the caller's thread.
fn inputs_of(chat: &str, old: &Old, to_account: &str, note: String) -> Inputs {
    let (helpers, _) = helpers_of(chat);
    Inputs {
        provider: old.provider,
        chat: chat.to_string(),
        name: chat_name(chat),
        from_account: old.account.clone(),
        to_account: to_account.to_string(),
        folder: crate::pty::manager().probe(chat).and_then(|p| p.working_dir()).unwrap_or_else(|| old.folder.clone()),
        settings: old.settings.clone(),
        note,
        stopped: stopped_of(chat, old.provider),
        transcript: Err(String::new()),
        last_handoff: None,
        git: gitfacts::Git::default(),
        helpers,
        at: crate::background::iso(now_ms()).unwrap_or_default(),
        boundary: crate::secret::random_hex(6),
        chat_text: None,
        lead: false,
    }
}

/// What every move of `chat` to the account at `to_account_path` as
/// `new_id` is held to before it begins: the old chat, the account, and
/// what the window is to warn of.
fn checked(chat: &str, new_id: &str, to_account_path: &str) -> Result<(Old, String, bool, Vec<String>), String> {
    super::dir_of(new_id).map_err(|_| "the new chat's id is not one Luna makes".to_string())?;
    if new_id == chat || reg().lock().unwrap_or_else(|e| e.into_inner()).known.contains_key(new_id) || store::records().contains_key(new_id)
    {
        return Err("the new chat's id is taken".into());
    }
    let old = old_of(chat)?;
    let (to_provider, to_account) = account_at(to_account_path)?;
    if let Some(why) = signed_out_refusal(to_provider, to_account_path, &to_account) {
        return Err(why);
    }
    if to_provider != old.provider {
        return Err("a chat moves only to another account of its own CLI".into());
    }
    if crate::paths::path_key(to_account_path) == crate::paths::path_key(&old.account_path) {
        return Err("the chat is on that account already".into());
    }
    // Started with `--worktree` by Luna (pty/start.rs) or running in one:
    // before its first turn its folder is still the project's, its checkout
    // already the account's.
    if let Some(why) = codex_worktree_refusal(old.provider, &old.folder, &old.account_path, old.settings["worktree"] == true) {
        return Err(why);
    }
    if let Some(r) = store::records().get(chat) {
        if r.switch.is_some() {
            return Err("a handoff of this chat is under way; move it once that is through".into());
        }
        if let Some(m) = r.moving.as_ref().filter(|m| m.phase.rank() >= 1 && !m.phase.ended()) {
            return Err(format!("this chat is being moved already (move {}, {})", m.id, m.phase.word()));
        }
    }
    let worktree = old.provider == Provider::Claude && old.settings["worktree"] == true;
    let warnings = warnings_of(chat, worktree);
    Ok((old, to_account, worktree, warnings))
}

/// What the window is to warn of before chat moves.
fn warnings_of(chat: &str, worktree: bool) -> Vec<String> {
    let (helpers, owed) = helpers_of(chat);
    let mut warnings = Vec::new();
    let live = helpers.iter().filter(|h| h.state != "exited" && h.state != "done").count();
    if live > 0 {
        warnings.push(format!("{live} helper(s) of this chat are still at work: they stay with this chat, not the new one."));
    }
    if owed > 0 {
        warnings.push(format!("{owed} session(s) owe this chat an answer: it arrives here, not in the new chat."));
    }
    if worktree {
        warnings.push(
            "This chat works in a Claude Code worktree: it is stopped before the new chat starts in that worktree, as two \
             chats in one worktree could lose it."
                .into(),
        );
    }
    warnings
}

/// The document built on a thread of its own and given `wait`: its transcript
/// read and git asked (both bounded on their own) off the caller's thread. A
/// build given up on ends by itself, its result dropped.
/// `ask` is the move's last ask (`ask_span`), for a transcript window that
/// starts inside its turn.
pub(super) fn build_within(
    mut inputs: Inputs,
    transcript: Option<PathBuf>,
    ask: Option<(u64, Option<u64>)>,
    wait: Duration,
) -> Result<document::Built, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let roots = scrub::Roots::of_this_machine();
    std::thread::Builder::new()
        .name("luna-move-build".into())
        .spawn(move || {
            // A turn a move's ask opened is the handoff attempt, not the work.
            inputs.transcript = match &transcript {
                Some(p) => excerpts::read(p, inputs.provider, Some(excerpts::Skip { opens: is_ask_line, ask, from_call: false })),
                None => Err("Luna does not know where this chat's transcript is".into()),
            };
            let since = inputs
                .transcript
                .as_ref()
                .ok()
                .and_then(|t| t.began.as_deref())
                .and_then(|b| chrono::DateTime::parse_from_rfc3339(b).ok())
                .map(|t| t.timestamp());
            inputs.git = gitfacts::of(&inputs.folder, since);
            let _ = tx.send(document::build(&inputs, &roots));
        })
        .map_err(|e| format!("could not start building the document: {e}"))?;
    rx.recv_timeout(wait).map_err(|_| format!("the document took longer than {} s to build", wait.as_secs()))
}

/// The chats being stopped for a move, between the look at their helpers and
/// the stop: no spawn under them is taken meanwhile.
fn stopping() -> &'static Mutex<std::collections::HashSet<String>> {
    static S: std::sync::OnceLock<Mutex<std::collections::HashSet<String>>> = std::sync::OnceLock::new();
    S.get_or_init(Default::default)
}

/// Whether `chat` is being stopped for a move; asked with the registry's lock
/// held, as the mark is set.
pub fn stopping_for_move(chat: &str) -> bool {
    stopping().lock().unwrap_or_else(|e| e.into_inner()).contains(chat)
}

/// A chat's mark in `stopping`, let go when dropped.
struct Stopping(String);

impl Stopping {
    fn mark(chat: &str) -> Stopping {
        let _r = reg().lock().unwrap_or_else(|e| e.into_inner());
        stopping().lock().unwrap_or_else(|e| e.into_inner()).insert(chat.to_string());
        Stopping(chat.to_string())
    }
}

impl Drop for Stopping {
    fn drop(&mut self) {
        stopping().lock().unwrap_or_else(|e| e.into_inner()).remove(&self.0);
    }
}

/// Why a Claude Code worktree move of `chat` waits: its helpers still at work,
/// or answers owed to it. A helper may work in that worktree, and the old
/// chat, stopped, would remove a clean worktree under it as it quits; what is
/// owed would arrive at a chat that is stopped.
pub(super) fn worktree_held(chat: &str) -> Option<String> {
    let (helpers, owed) = helpers_of(chat);
    let mut live: Vec<String> = helpers.iter().filter(|h| h.state != "exited" && h.state != "done").map(|h| h.id.clone()).collect();
    // Spawns taken and not yet seen through: helpers about to be.
    let pending = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        r.spawns.values().filter(|s| !s.settled && crate::agents::registry::top_of(&r, &s.caller) == chat).count()
    };
    if pending > 0 {
        live.push(format!("{pending} spawn(s) under way"));
    }
    if live.is_empty() && owed == 0 {
        return None;
    }
    let mut what = Vec::new();
    if !live.is_empty() {
        what.push(format!("its helpers at work ({})", live.join(", ")));
    }
    if owed > 0 {
        what.push(format!("{owed} answer(s) owed to it"));
    }
    Some(format!(
        "this chat works in a Claude Code worktree and is stopped for the move, but {} would be left: let them finish or \
         stop them, then send again",
        what.join(" and ")
    ))
}

/// The settings the new chat is to start with, as the window sends them,
/// held to the move: same CLI, the account it goes to, the new chat's id, no
/// Codex worktree, a Claude Code worktree by name where the old chat had one.
fn launch_refusal(m: &Move, launch: &Value) -> Option<String> {
    if !launch.is_object() {
        return Some("the launch settings are not an object".into());
    }
    if launch["provider"].as_str() != Some(m.provider.as_str()) {
        return Some("the new chat is to run the same CLI".into());
    }
    let path = launch["accountPath"].as_str().unwrap_or("");
    if crate::paths::path_key(path) != crate::paths::path_key(&m.to_account_path) {
        return Some("the new chat is to run on the account the preview named".into());
    }
    let folder = launch["folder"].as_str().unwrap_or("");
    if crate::paths::path_key(folder) != crate::paths::path_key(&m.folder) {
        return Some("the new chat is to work in the old chat's folder".into());
    }
    if m.provider == Provider::Codex && launch["worktree"] == true {
        return Some("a moved Codex chat does not make a worktree".into());
    }
    if m.worktree && launch["worktreeName"].as_str().is_none_or(str::is_empty) {
        return Some("the new chat is to go into the old chat's worktree by its name".into());
    }
    None
}

/// The window's Send: `text` as the person confirmed it (its SHA-256 as the
/// window showed it), bound with the destination and the launch settings,
/// checked once more for the folders it may not name, and copied private as
/// the new chat's document. Twice with the same is one: the second answers
/// as the first did.
pub fn move_confirm(chat_id: String, move_id: String, text: String, sha256: String, launch: Value) -> Result<MoveStatus, String> {
    confirm_with(&chat_id, &move_id, &text, &sha256, &launch, &scrub::Roots::of_this_machine(), &|p| {
        let (provider, name) = account_at(p)?;
        signed_out_refusal(provider, p, &name).map_or(Ok(()), Err)
    })
}

/// `move_confirm` against `roots`, the account looked for by `account_ok`.
fn confirm_with(
    chat: &str,
    move_id: &str,
    text: &str,
    sha256: &str,
    launch: &Value,
    roots: &scrub::Roots,
    account_ok: &dyn Fn(&str) -> Result<(), String>,
) -> Result<MoveStatus, String> {
    // One Send at a time: a second that comes while the first writes the
    // document finds it confirmed and answers as it did, never writing the
    // copy over or undoing it.
    static SENDING: Mutex<()> = Mutex::new(());
    let _one = SENDING.lock().unwrap_or_else(|e| e.into_inner());
    let (move_id, launch) = (move_id.to_string(), launch.clone());
    let m = current(chat, &move_id)?;
    let text_sha = sha256_hex(text.as_bytes());
    let bound = binding(&text_sha, m.provider, &m.to_account_path, &launch, &m.line);
    if m.phase.rank() >= 1 {
        return if m.binding.as_deref() == Some(bound.as_str()) && !m.phase.ended() {
            Ok(status_of(chat, &m))
        } else if m.phase.ended() {
            Err(format!("this move has ended ({})", m.phase.word()))
        } else {
            Err("this move was confirmed already, with another text or other settings".into())
        };
    }
    if !matches!(m.phase, MovePhase::Shown { .. }) {
        return Err("the chat has not written its document yet".into());
    }
    if text_sha != sha256 {
        return Err("the text is not the one shown: show the preview again".into());
    }
    if text.trim().is_empty() {
        return Err("the document is empty".into());
    }
    if text.len() > document::MAX_BYTES {
        return Err(format!("the document is over {} KB", document::MAX_BYTES / 1024));
    }
    if text.chars().any(scrub::invisible) {
        return Err("the document holds characters a screen does not show (zero-width, bidirectional or tag characters): \
                    take them out, nothing goes until then"
            .into());
    }
    if store::records().get(&m.new_id).and_then(|r| r.moved_from.as_ref()).is_some_and(|f| f.move_id != move_id) {
        return Err("the new chat's id belongs to another move".into());
    }
    let named = scrub::named_roots(text, roots);
    if !named.is_empty() {
        return Err(format!(
            "the document names {} folder(s) of an account or of Luna's own ({}): take them out, nothing goes until then",
            named.len(),
            named.join(", ")
        ));
    }
    if let Some(why) = launch_refusal(&m, &launch) {
        return Err(why);
    }
    if let Some(why) = m.worktree.then(|| worktree_held(chat)).flatten() {
        return Err(why);
    }
    account_ok(&m.to_account_path)?;
    let dir = copies_of(&m.new_id)?;
    let payload = dir.join(format!("{PAYLOAD_N}.md"));
    crate::secret::write_private_synced(&payload, text.as_bytes()).map_err(|e| format!("the document could not be kept: {e}"))?;
    let moved_from = MovedFrom { chat: chat.to_string(), move_id: move_id.clone(), at: now_ms(), worktree: m.worktree };
    let new = m.new_id.clone();
    // Both chats are in that worktree from here on, and so is whatever chat
    // either is moved to after: one key for all of them.
    let key = m.worktree.then(|| worktree_key(&m.folder, launch["worktreeName"].as_str().unwrap_or("")));
    if let Err(e) = store::change(&new, |r| {
        r.last = r.last.max(PAYLOAD_N);
        r.moved_from = Some(moved_from);
        r.worktree_key = key.clone();
    }) {
        let _ = std::fs::remove_file(&payload);
        return Err(format!("the move could not be saved: {e}"));
    }
    let payload_s = payload.to_string_lossy().into_owned();
    // A handoff scheduled since the move began (mod.rs `schedule` refuses
    // one while a move is under way) would reset the chat as it moves.
    let committed = store::change(chat, |r| match r.moving.as_mut().filter(|x| x.id == move_id && x.phase.rank() == 0) {
        Some(_) if r.switch.is_some() => Err(HANDOFF_UNDER_WAY),
        Some(x) => {
            x.phase = MovePhase::Confirmed { at: now_ms() };
            // Kept or stopped as the person chose, in what Send is bound to;
            // a Claude Code worktree's always stopped.
            x.stop_old = x.worktree || launch["stopOld"] == true;
            x.binding = Some(bound.clone());
            x.launch = Some(launch.clone());
            x.payload = Some(payload_s.clone());
            x.payload_sha256 = Some(text_sha.clone());
            let m = x.clone();
            if key.is_some() {
                r.worktree_key = key.clone();
            }
            Ok(m)
        }
        None => Err("the move changed meanwhile"),
    });
    match committed {
        Ok(Ok(m)) => {
            crate::log::info(
                "move",
                &format!(
                    "{} move {move_id} confirmed: to {}/{} as {new} ({} bytes, sha256 {}){}",
                    label(chat),
                    m.provider.as_str(),
                    m.to_account,
                    text.len(),
                    &text_sha[..16],
                    if text_sha == m.shown_sha256 { "" } else { ", edited" }
                ),
            );
            Ok(status_of(chat, &m))
        }
        Ok(Err(why)) => {
            undo_new(&new, &payload);
            Err(why.into())
        }
        Err(e) => {
            undo_new(&new, &payload);
            Err(format!("the move could not be saved: {e}"))
        }
    }
}

/// A new chat's side of a move given up before its row: its record and copy.
fn undo_new(new: &str, payload: &Path) {
    let _ = std::fs::remove_file(payload);
    let _ = store::remove(new);
}

/// `chat`'s move `id` as on record.
fn current(chat: &str, id: &str) -> Result<Move, String> {
    store::records()
        .get(chat)
        .and_then(|r| r.moving.clone())
        .filter(|m| m.id == id)
        .ok_or_else(|| "no such move of this chat (given up, or Luna restarted before it was confirmed)".to_string())
}

/// Moves `chat`'s move `id` on to `to`, if it stands at a phase below it and
/// `allowed` says so of it; the move as it then is.
fn advance(chat: &str, id: &str, to: MovePhase, allowed: impl Fn(&Move) -> Result<(), String>) -> Result<Move, String> {
    let out = store::change(chat, |r| {
        let m = r.moving.as_mut().filter(|m| m.id == id).ok_or("no such move of this chat")?;
        if m.phase.rank() >= to.rank() || m.phase.ended() {
            return Ok(m.clone());
        }
        allowed(m)?;
        m.phase = to.clone();
        Ok::<Move, String>(m.clone())
    });
    match out {
        Ok(r) => r,
        Err(e) => Err(format!("the move could not be saved: {e}")),
    }
}

/// The window's word that it did its part of a move: `oldStopped` (the old
/// chat's CLI stopped, looked at here) or `rowCreated` (the new chat's row
/// made). A step taken already answers as it stands.
pub fn move_step(chat_id: String, move_id: String, step: String) -> Result<MoveStatus, String> {
    let chat = chat_id.as_str();
    let alive = |c: &str| crate::pty::manager().alive(c);
    let m = match step.as_str() {
        // The core stops the old chat itself, right after the look at what
        // its worktree still holds: nothing can start in between that the
        // window's own stop would not have seen.
        "stopOld" => {
            let m = current(chat, &move_id)?;
            if m.phase.rank() < 1 {
                return Err("the move is not confirmed".into());
            }
            if m.phase.rank() == 1 {
                // Marked first, under the registry's lock (spawn/mod.rs looks
                // there): a helper spawned from here on is refused, one before
                // is seen by the look below. Let go however this ends.
                let _stopping = Stopping::mark(chat);
                if let Some(why) = m.worktree.then(|| worktree_held(chat)).flatten() {
                    return Err(why);
                }
                tauri::async_runtime::block_on(crate::pty::kill_session(chat.to_string()))?;
            }
            advance(chat, &move_id, MovePhase::OldStopped { at: now_ms() }, |_| {
                if alive(chat) {
                    Err("the old chat is still running".into())
                } else {
                    Ok(())
                }
            })?
        }
        // The person's "Type it now" on the new chat: the line goes in at the
        // next look whatever the screen check says, but never into a turn at
        // work or a composer with text in it.
        "typeNow" => {
            let set = store::change(chat, |r| match r.moving.as_mut().filter(|x| x.id == move_id) {
                Some(x) if matches!(x.phase, MovePhase::RowCreated { .. }) && !seen_of(x) => {
                    Err("its screen shows no empty composer: answer what it shows in its pane first".to_string())
                }
                Some(x) if matches!(x.phase, MovePhase::RowCreated { .. }) => {
                    x.forced = true;
                    Ok(x.clone())
                }
                Some(x) => Err(format!("there is no line waiting to be typed ({})", x.phase.word())),
                None => Err("no such move of this chat".to_string()),
            });
            let m = set.map_err(|e| format!("the move could not be saved: {e}"))??;
            crate::log::info("move", &format!("move {}: the person said to type its line into {} now", m.id, m.new_id));
            m
        }
        "oldStopped" => advance(chat, &move_id, MovePhase::OldStopped { at: now_ms() }, |m| {
            if m.phase.rank() < 1 {
                return Err("the move is not confirmed".into());
            }
            if alive(chat) {
                return Err("the old chat is still running".into());
            }
            if let Some(why) = m.worktree.then(|| worktree_held(chat)).flatten() {
                return Err(why);
            }
            Ok(())
        })?,
        "rowCreated" => advance(chat, &move_id, MovePhase::RowCreated { at: now_ms() }, |m| {
            if m.phase.rank() < 1 {
                return Err("the move is not confirmed".into());
            }
            if m.stop_old && (m.phase.rank() < 2 || alive(chat)) {
                return Err("the old chat is to be stopped first".into());
            }
            Ok(())
        })?,
        other => return Err(format!("no step {other}")),
    };
    Ok(status_of(chat, &m))
}

/// The window's Cancel: a move not yet typed into its new chat ends here.
/// Before the new chat's row its copy and record go; after, the new chat
/// stays as it is, with nothing typed into it.
pub fn move_cancel(chat_id: String, move_id: String) -> Result<MoveStatus, String> {
    let chat = chat_id.as_str();
    let m = current(chat, &move_id)?;
    if m.phase.ended() {
        return Ok(status_of(chat, &m));
    }
    if m.phase.rank() >= 4 {
        return Err(format!("the move is past what Cancel stops ({})", m.phase.word()));
    }
    let before_row = m.phase.rank() < 3;
    let m = end(chat, &move_id, MovePhase::Canceled { at: now_ms() })?;
    if before_row {
        if let Some(p) = &m.payload {
            undo_new(&m.new_id, Path::new(p));
        }
    }
    Ok(status_of(chat, &m))
}

/// The moves of every chat, for the window: those under way, and those ended
/// lately.
pub fn move_list() -> Vec<MoveStatus> {
    let now = now_ms();
    let moves: Vec<(String, Move)> = store::records()
        .iter()
        .filter_map(|(chat, r)| Some((chat.clone(), r.moving.clone()?)))
        .filter(|(_, m)| !m.phase.ended() || now.saturating_sub(m.phase.at()) < ENDED_KEPT_MS)
        .collect();
    moves.iter().map(|(chat, m)| status_of(chat, m)).collect()
}

/// `chat`'s move `id` ended as `to`, whatever it stood at: a record that will
/// not save ends it in memory all the same.
fn end(chat: &str, id: &str, to: MovePhase) -> Result<Move, String> {
    end_where(chat, id, to, |_| true, |_| {}).map(|(m, _)| m)
}

/// `end`, only where the move as it stands then is one `may` takes, `also`
/// done to it in the same change; with whether it was ended here. What looked
/// at the move before (an ask's timeout, say) may find it gone on meanwhile:
/// its preview built from records, or confirmed. A move ended with the old
/// chat not stopped, before its stop or in a move that keeps it running
/// (`stop_old` false), re-arms the threshold (`store::rearmed_unasked`),
/// quiet while it was under way: the chat goes on where it is, and is asked
/// again if it is still past its threshold.
fn end_where(chat: &str, id: &str, to: MovePhase, may: impl Fn(&Move) -> bool, also: impl Fn(&mut Move)) -> Result<(Move, bool), String> {
    let set = |r: &mut store::Record| {
        let m = r.moving.as_mut().filter(|m| m.id == id)?;
        let taken = !m.phase.ended() && may(m);
        // An ask ended before its document came: what the chat sends in the
        // same turn is that ask's late answer. One collected already is not.
        // Only one typed into the chat: a move canceled while still Asking
        // asked nothing.
        let unanswered = taken && matches!(m.phase, MovePhase::Asked { .. });
        let chat_goes_on = taken && to.ended() && (!m.stop_old || m.phase.rank() < MovePhase::OldStopped { at: 0 }.rank());
        if taken {
            also(m);
            m.phase = to.clone();
        }
        let m = m.clone();
        if unanswered {
            r.ask_closed = Some((m.id.clone(), now_ms()));
            r.ask_closed_turns = crate::activity::turn_state(chat).map(|t| t.turns_ended);
        }
        if chat_goes_on {
            store::rearmed_unasked(r);
        }
        Some((m, taken))
    };
    let ended = match store::change(chat, set) {
        Ok(m) => m.ok_or_else(|| "no such move of this chat".to_string()),
        Err(e) => {
            crate::log::warn("move", &format!("{}: move {id} could not be saved as ended: {e}", label(chat)));
            store::records().get_mut(chat).and_then(set).ok_or_else(|| "no such move of this chat".to_string())
        }
    };
    // A document the chat wrote goes with its move's end: the new chat has its
    // own copy of what was sent.
    if let Some(p) = ended.as_ref().ok().filter(|(m, taken)| *taken && m.phase.ended()).and_then(|(m, _)| m.preview.clone()) {
        if copies_of(chat).is_ok_and(|dir| Path::new(&p).parent() == Some(dir.as_path())) {
            let _ = std::fs::remove_file(p);
        }
    }
    ended
}

/// Why `id`'s CLI may not start now: it is a moved chat whose old chat ran
/// in a Claude Code worktree and is still running. Called at every start.
pub fn opening_refusal(id: &str) -> Option<String> {
    let tied = worktree_ties(&store::records(), id);
    let other = tied.into_iter().find(|c| crate::pty::manager().alive(c))?;
    Some(format!(
        "this chat shares a Claude Code worktree with {other}, the chat it moved from or to, which is running: the session \
         that made a worktree removes it as it quits when it is clean, whoever is inside, so the two do not run at once. \
         Stop {other} first"
    ))
}

/// The chats that share a Claude Code worktree with `id`: every chat whose
/// record names the same worktree (`Record::worktree_key`), however many
/// moves apart and whichever of them is gone, from the first Send on.
pub(super) fn worktree_ties(r: &store::Records, id: &str) -> Vec<String> {
    let Some(key) = r.get(id).and_then(|rec| rec.worktree_key.clone()) else { return Vec::new() };
    r.iter().filter(|(c, rec)| *c != id && rec.worktree_key.as_deref() == Some(key.as_str())).map(|(c, _)| c.clone()).collect()
}

/// A Claude Code worktree as the records name it: its project, as a path
/// key, and its name.
pub(super) fn worktree_key(folder: &str, name: &str) -> String {
    format!("{}|{name}", crate::paths::path_key(folder))
}
/// The folder a moved chat reads its document from, for its Claude Code
/// start's `--add-dir`: its folder of copies, where its record says it was
/// opened by a move.
pub fn moved_dir(id: &str) -> Option<PathBuf> {
    store::records().get(id)?.moved_from.as_ref()?;
    copies_of(id).ok()
}

/// How long a chat asked for its document gets to come to rest for the ask,
/// and then to write it.
pub(super) const ASK_REST_MS: u64 = 10 * 60_000;
pub(super) const ASK_WAIT_MS: u64 = 10 * 60_000;
/// Whether a file `name` the chat sends, no ask under way, answers an ask
/// that ended (`closed`: when, and the chat's ended turns then): a file
/// named for a move's draft (`move-…`, case aside) always; any other only in
/// the turn the ask ended in, and within ASK_LATE_MS. The chat's next turn,
/// the person's or one a handoff notice starts, is its own again.
pub(super) fn late_answer(name: &str, closed: Option<(u64, Option<u32>)>, now: u64, turn: Turn) -> bool {
    if name.to_ascii_lowercase().starts_with("move-") {
        return true;
    }
    let Some((at, ended_then)) = closed else { return false };
    if now.saturating_sub(at) >= ASK_LATE_MS {
        return false;
    }
    // A turn begun after the ask ended (the person's, a handoff notice's) is
    // the chat's own from its start; one begun before is the ask's.
    match turn.started {
        Some(started) => started <= at,
        None => turn.ended == ended_then,
    }
}

/// The chat's turn as a file comes in: when the one under way began, and how
/// many had ended.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Turn {
    pub started: Option<u64>,
    pub ended: Option<u32>,
}
/// How long after an ask ends unanswered a file the chat sends is taken for
/// its late answer, refused rather than handed off.
pub(super) const ASK_LATE_MS: u64 = 60 * 60_000;

/// How the tag of a move's ask closes.
const ASK_TAG_END: &str = " request, not typed by the user]";

/// Whether a prompt in a transcript is a move's ask Luna typed (`ask_line`):
/// as typed, or as Claude Code keeps a paste, inside its `<pasted_content …>`
/// wrapper (`facts::unwrapped`).
pub(super) fn is_ask_line(text: &str) -> bool {
    let Some(rest) = super::facts::unwrapped(text).strip_prefix(MOVE_TAG) else { return false };
    rest.split_once(']').is_some_and(|(tag, _)| format!("{tag}]").ends_with(ASK_TAG_END))
}

/// What Luna types into a chat asked for its move's document: fixed words.
pub(super) fn ask_line(id: &str) -> String {
    format!(
        "{MOVE_TAG} {id}{ASK_TAG_END} The person is moving this chat to another account of this CLI, \
         where a new chat goes on from a document you write now. Nothing is cleared: this conversation goes on as it is. \
         luna_session_handoff without arguments says how."
    )
}

/// Why a chat is not to be asked for its document now.
enum Unaskable {
    NotRunning,
    /// A usage limit, by its kind, that still holds it.
    Limit(String),
    Question,
}

impl Unaskable {
    fn words(&self) -> String {
        match self {
            Unaskable::NotRunning => "its CLI is not running".into(),
            Unaskable::Limit(kind) => format!("it stopped on a usage limit ({kind}) and cannot write now"),
            Unaskable::Question => "it is waiting on a question of its CLI's".into(),
        }
    }
}

/// Why `chat` is not to be asked for its document now: its CLI not running,
/// stopped on a usage limit still in force, or waiting on a question.
fn ask_refusal(chat: &str, provider: Provider) -> Option<Unaskable> {
    if !crate::pty::manager().alive(chat) {
        return Some(Unaskable::NotRunning);
    }
    if let Some(limit) = crate::activity::limit_in_force(chat, provider) {
        return Some(Unaskable::Limit(limit.kind));
    }
    crate::activity::turn_state(chat).filter(|t| t.turn == crate::activity::Turn::Waiting).map(|_| Unaskable::Question)
}

/// What `prompt_verdict` says of a composer holding a person's text.
pub(super) const HOLDS_TEXT: &str = "the composer holds text typed or pasted there: send or clear it (Esc), then Luna types its line";

/// What `prompt_verdict` says of a screen whose layout shows no prompt.
const NO_PROMPT: &str = "its screen shows no empty prompt (a dialog, a first-run screen, text in the composer)";

/// Whether `id`'s CLI stands at its prompt, strictly, with why not, for the
/// log and the window: the composer drawn and empty, no turn under way,
/// nobody typing. A dialog or a first-run screen is not one, and a Codex line
/// typed mid-turn would steer the turn. A stall shows what it waits on. Only
/// a turn at work keeps the line out: a CLI that says
/// it waits while its screen shows an empty composer (a notice after its
/// first-run screens, an idle reminder) asks nothing of anyone, and a real
/// question draws a dialog, which is no prompt.
pub(super) fn prompt_verdict(pty: &crate::pty::PtyManager, id: &str) -> Result<(), &'static str> {
    if !pty.alive(id) {
        return Err("its CLI is not running");
    }
    if pty.typed_ago_ms(id).is_some_and(|ms| ms < SEND_TYPING_MS) {
        return Err("a person typed into it a moment ago");
    }
    if crate::activity::turn_state(id).is_some_and(|t| t.turn == crate::activity::Turn::Busy) {
        return Err("a turn is under way");
    }
    if !pty.at_prompt(id) {
        return Err(NO_PROMPT);
    }
    // Never typed over: the person sends or clears it, then the line goes in.
    if pty.has_draft(id) || pty.keys_left_text(id) {
        return Err(HOLDS_TEXT);
    }
    Ok(())
}

/// Whether `chat` has a move under way, from its ask or preview until it is
/// done, failed or canceled: the threshold opens no ask (mod.rs `ask_auto`),
/// the notifier types no handoff notice (mod.rs `withheld`), the person's
/// Hand off now is refused (mod.rs `now_refusal`), the person's ask is not
/// typed (mod.rs `manual_ask_of`) and no handoff is scheduled (mod.rs
/// `schedule`) until it ends. A handoff
/// asked of a chat the person is moving would race the move for what the
/// chat writes, or reset it in the middle of the move, and the move carries
/// it on anyway.
pub(super) fn under_way(chat: &str) -> bool {
    store::records().get(chat).and_then(|r| r.moving.as_ref()).is_some_and(Move::under_way)
}

/// Why `chat`, run with `settings`, is not to be asked for its move's
/// document now, with what would clear the way: no handoff tool to send it
/// with, one its CLI has not listed, no file it can write, or its CLI not in
/// a state to answer. The person's ask for a handoff open is no bar: the move
/// takes its place (`taken`).
fn ask_blocked(chat: &str, settings: &Value, provider: Provider) -> Option<String> {
    if !super::may_list(chat) {
        return Some(
            "the chat runs without Luna's tools, so it has no luna_session_handoff to send its document with: start \
             it with the Luna tools box ticked to ask it"
                .into(),
        );
    }
    if !super::is_listed(chat) {
        return Some(
            "the chat's CLI started before Luna offered it luna_session_handoff: resume it on the same conversation, \
             then ask it"
                .into(),
        );
    }
    if let Some(why) = super::served::unwritable(settings) {
        return Some(format!("the chat writes no file: {why}"));
    }
    if let Some(why) = ask_refusal(chat, provider) {
        return Some(format!("the chat is not to be asked now: {}; ask again once that is past", why.words()));
    }
    None
}

/// The window's "Ask the chat": a person's chat with Luna's tools, whose CLI
/// has listed Luna's handoff tool, asked to write its own document for the
/// move through that tool, collected without a reset. Only where it can write
/// a file and is running, not stopped on a limit, and with no handoff
/// scheduled; typed only at a strict prompt. The person's own ask for a
/// handoff open is withdrawn, the person told (`taken`). The threshold's ask
/// open gives way: a file the chat sends goes to the move while it asks
/// (`asked_file`), never to a handoff, so the threshold's ask closes, its
/// notice not typed yet dropped (`threshold_retired`, as for a move from
/// records). The threshold stays quiet for the whole move
/// (`under_way`) and is re-armed as the move ends with the chat not stopped
/// (`end_where`), to ask again if the chat is still past it. The person asked
/// for the move; the threshold asked on its own, and asks again.
pub fn move_ask(chat_id: String, new_id: String, to_account_path: String, note: String) -> Result<MoveStatus, String> {
    let chat = chat_id.as_str();
    let (old, to_account, worktree, _) = checked(chat, &new_id, &to_account_path)?;
    if let Some(why) = ask_blocked(chat, &old.settings, old.provider) {
        return Err(why);
    }
    let id = crate::secret::random_hex(6);
    let payload = super::dir_of(&new_id).map(|d| d.join(format!("{PAYLOAD_N}.md"))).unwrap_or_default();
    let line = prompt_line_with(&id, Source::Chat, chat, chat_name(chat).as_deref(), &payload.to_string_lossy(), &note);
    let mut m = new_move(&id, &new_id, &old, to_account, to_account_path, line, Source::Chat, worktree);
    m.note = note;
    let withdrawn = store::change(chat, |r| taken(chat, r, &m)).map_err(|e| format!("the move could not be saved: {e}"))??;
    crate::agents::save_chats();
    if withdrawn {
        ask_withdrawn(chat);
    }
    crate::log::info("move", &format!("{} is to be asked for its move's document ({id})", label(chat)));
    Ok(status_of(chat, &m))
}

/// The preview of a document the chat wrote, once collected: read back as
/// the private copy Luna made, its hash the one shown.
pub fn move_preview(chat_id: String, move_id: String) -> Result<MovePreview, String> {
    let chat = chat_id.as_str();
    let m = current(chat, &move_id)?;
    if !matches!(m.phase, MovePhase::Shown { .. }) {
        return Err(format!("there is no preview to show ({})", m.phase.word()));
    }
    let path = m.preview.clone().ok_or("this move's preview was handed over as it was built")?;
    let dir = copies_of(chat)?;
    if Path::new(&path).parent() != Some(dir.as_path()) {
        return Err("the preview is not in this chat's folder of copies".into());
    }
    let bytes = crate::secret::read_private(Path::new(&path), document::MAX_BYTES as u64 + 1)
        .map_err(|e| e.to_string())?
        .ok_or("the preview is gone")?;
    if sha256_hex(&bytes) != m.shown_sha256 {
        return Err("the preview is not the one Luna made".into());
    }
    let text = String::from_utf8(bytes).map_err(|_| "the preview is not text")?;
    Ok(preview_of(&m, text, m.cleaned, warnings_of(chat, m.worktree)))
}

/// The window's "Build from records instead", for a move whose chat is
/// being asked for its document, or whose ask failed: the ask is closed as
/// Cancel closes it, so a document the chat sends late is refused, never
/// shown, and Luna builds its own from the chat's records into the same
/// preview, the move's destination and the person's note kept. The ask's own
/// turn is no part of it: neither in the excerpts nor in how the chat stood.
pub fn move_build_from_records(chat_id: String, move_id: String, session_id: Option<String>) -> Result<MovePreview, String> {
    let chat = chat_id.as_str();
    let m = current(chat, &move_id)?;
    if !m.asking() && !m.ask_failed() {
        return Err(format!("the chat is not being asked for this move's document ({})", m.phase.word()));
    }
    let (old, to_account, _, warnings) = checked(chat, &m.new_id, &m.to_account_path)?;
    built_from_records(chat, &m, &old, &to_account, session_id, warnings)
}

/// `move_build_from_records` for move `m` of `chat`, the old chat `old`, once
/// held to what every move is.
fn built_from_records(
    chat: &str,
    m: &Move,
    old: &Old,
    to_account: &str,
    session_id: Option<String>,
    warnings: Vec<String>,
) -> Result<MovePreview, String> {
    let session_id = session_id.filter(|s| !s.trim().is_empty());
    let mut inputs = inputs_of(chat, old, to_account, m.note.clone());
    let turn = Turn { started: crate::activity::turn_started_ms(chat), ended: crate::activity::turn_state(chat).map(|t| t.turns_ended) };
    let span = ask_span(chat);
    if inputs.stopped == Stopped::MidTurn && span.is_some_and(|(_, closed)| in_ask_turn(m, turn, closed)) {
        inputs.stopped = Stopped::Finished;
    }
    inputs.last_handoff = last_handoff(chat);
    let transcript = transcript_of(chat, old.provider, &old.account_path, session_id.as_deref());
    let built = build_within(inputs, transcript, span, BUILD_WAIT)?;
    let sha256 = sha256_hex(built.text.as_bytes());
    let payload = super::dir_of(&m.new_id).map(|d| d.join(format!("{PAYLOAD_N}.md"))).unwrap_or_default();
    let line = prompt_line_with(&m.id, Source::Luna, chat, chat_name(chat).as_deref(), &payload.to_string_lossy(), &m.note);
    let shown = store::change(chat, |r| {
        let x = r.moving.as_mut().filter(|x| x.id == m.id && (x.asking() || x.ask_failed()))?;
        // Typed into the chat and not answered: what it sends in that turn
        // is the ask's late answer (`asked_file`). One closed as it failed
        // keeps when that was; one a restart gave up was not closed; one
        // never typed asked nothing.
        let closed = r.ask_closed.as_ref().is_some_and(|(id, _)| *id == x.id);
        let typed = matches!(x.phase, MovePhase::Asked { .. }) || (x.asked_at.is_some() && !closed);
        x.source = Source::Luna;
        x.line = line.clone();
        x.phase = MovePhase::Shown { at: now_ms() };
        x.shown_sha256 = sha256.clone();
        x.preview = None;
        x.cleaned = [0; 3];
        let x = x.clone();
        if typed {
            r.ask_closed = Some((x.id.clone(), now_ms()));
            r.ask_closed_turns = crate::activity::turn_state(chat).map(|t| t.turns_ended);
        }
        Some(x)
    })
    .map_err(|e| format!("the move could not be saved: {e}"))?;
    let m = shown.ok_or("the move changed meanwhile (the chat's document came in, or the move was given up): look at it again")?;
    crate::log::info(
        "move",
        &format!("{} has its move {} built from records instead of asked ({} bytes)", label(chat), m.id, built.text.len()),
    );
    Ok(preview_of(&m, built.text, [built.redacted, built.paths_cut, built.invisible], warnings))
}

/// Whether the turn under way, `turn`, is the one answering move `m`'s ask,
/// which closed at `closed` if it has: no turn ended since the ask was
/// typed (it is typed only between turns), and begun once it was typed and
/// not after it closed; where no start is known, the ask still open. A turn
/// the person began after an ask timed out is the chat's own work.
pub(super) fn in_ask_turn(m: &Move, turn: Turn, closed: Option<u64>) -> bool {
    let Some(asked) = m.asked_at else { return false };
    if let (Some(then), Some(now)) = (m.asked_turns, turn.ended) {
        if now != then {
            return false;
        }
    }
    match turn.started {
        Some(s) => s >= asked && closed.is_none_or(|c| s <= c),
        None => matches!(m.phase, MovePhase::Asked { .. }),
    }
}

/// `chat`'s move's last ask, as (typed, closed) in ms: when it was typed, and
/// when Luna closed it, if it has (a move given up at a restart closed it as
/// it failed).
fn ask_span(chat: &str) -> Option<(u64, Option<u64>)> {
    let r = store::records();
    let rec = r.get(chat)?;
    let m = rec.moving.as_ref()?;
    let typed = m.asked_at?;
    let closed = rec.ask_closed.as_ref().filter(|(id, _)| *id == m.id).map(|(_, at)| *at).or_else(|| (!m.asking()).then(|| m.phase.at()));
    Some((typed, closed))
}

/// What the move window looks at before a move of `chat` begins: the usage
/// limit that still holds the chat, which has it offer Luna's document from
/// records first, and what keeps the chat from being asked for its own.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MoveFacts {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<crate::activity::LimitFact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ask_blocked: Option<String>,
}

pub fn move_facts(chat_id: String) -> Result<MoveFacts, String> {
    let chat = chat_id.as_str();
    let old = old_of(chat)?;
    Ok(MoveFacts {
        limit: crate::activity::limit_in_force(chat, old.provider),
        ask_blocked: ask_blocked(chat, &old.settings, old.provider),
    })
}

/// What a chat being asked for its move's document hears, calling the tool
/// from a conversation that ask was not typed into, or before it was typed.
const NOT_ASKED_HERE: &str = "this conversation was not asked for the move's document: Luna has not typed its request \
     yet, or typed it into another conversation of this chat. Nothing is done with this call and nothing switches; go on \
     as you were.";

/// Whether move `m`'s ask answers a call from the chat on `conversation`:
/// only once typed, and only in the conversation it was typed into, both
/// known. Text injected into a conversation the person started since, or
/// one Luna has not typed the ask into yet, finds nothing open.
pub(super) fn ask_here(m: &Move, conversation: Option<&str>) -> bool {
    matches!(m.phase, MovePhase::Asked { .. })
        && matches!((m.asked_conversation.as_deref(), conversation), (Some(asked), Some(now)) if asked == now)
}

/// `luna_session_handoff` with `file` from a chat asked for its move's
/// document: collected, with nothing scheduled and nothing reset, where the
/// ask holds in the conversation called from (`ask_here`), else refused. A file
/// named for a move (`move-…`) when none is asking is refused, so a late
/// answer never becomes a handoff. None: an ordinary handoff.
pub(super) fn asked_file(caller: &str, file: &str) -> Option<Result<Value, String>> {
    let m = store::records().get(caller).and_then(|r| r.moving.clone()).filter(Move::asking);
    let name = Path::new(file.trim()).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match m {
        Some(m) if !ask_here(&m, super::conversation_now(caller).as_deref()) => Some(Err(NOT_ASKED_HERE.into())),
        Some(m) => Some(collect(caller, &m, file)),
        None if answers_closed_ask(caller, &name) => Some(Err(MOVE_OVER.into())),
        None => None,
    }
}

/// What a chat hears answering a move's ask that Luna has closed, with its
/// document or a call for how to write it: the move was the person's, and
/// they decided it otherwise. Not the refusal of a chat nobody asked (mod.rs
/// `NOT_ASKED`), which would have it tell the person its call failed.
pub(super) const MOVE_OVER: &str = "the move that asked this chat for its document is over: the person had Luna write \
     the document from the chat's records, gave the move up or canceled it, or it was sent. There is nothing to do: \
     nothing is done with this call and nothing switches. Go on as you were.";

/// Whether a call from `caller` now, with a file named `name` (empty: a call
/// without one), is taken for the late answer of a move's ask that closed
/// (`late_answer`): a file named for a move, or any call in the turn the ask
/// was answered in, within ASK_LATE_MS of its close.
pub(super) fn answers_closed_ask(caller: &str, name: &str) -> bool {
    let closed = store::records().get(caller).and_then(|r| r.ask_closed.as_ref().map(|(_, at)| (*at, r.ask_closed_turns)));
    let turn =
        Turn { started: crate::activity::turn_started_ms(caller), ended: crate::activity::turn_state(caller).map(|t| t.turns_ended) };
    late_answer(name, closed, now_ms(), turn)
}

/// The chat's document copied and checked as a handoff's is (file.rs), Luna's
/// facts added, cleaned and kept private as the preview: the move is shown.
fn collect(caller: &str, m: &Move, file: &str) -> Result<Value, String> {
    let dir = copies_of(caller)?;
    let free = super::file::next_free(&dir);
    let n = store::change(caller, |r| {
        let n = (r.last + 1).max(free);
        r.last = n;
        n
    })?;
    let snap = super::file::snapshot(caller, file, &crate::paths::data_dir(), &dir, n)?;
    let _ = std::fs::remove_file(&snap.path);
    let text = super::file::without_bom(&snap.text).to_string();
    if text.len() > document::CHAT_DOC_MAX {
        return Err(format!(
            "the document is over {} KB, which leaves no room for what Luna adds: write it shorter and call again",
            document::CHAT_DOC_MAX / 1024
        ));
    }
    let old = old_of(caller)?;
    let mut inputs = inputs_of(caller, &old, &m.to_account, m.note.clone());
    // The turn under way is the one writing this document, asked at a prompt
    // between turns: the chat's own work had ended.
    if inputs.stopped == Stopped::MidTurn {
        inputs.stopped = Stopped::Finished;
    }
    inputs.chat_text = Some(text);
    let built = build_within(inputs, None, None, BUILD_WAIT)?;
    // A name of this call's own: two calls overlapping in one ask each write
    // theirs, the one that takes Shown keeps it, and the other's going takes
    // nothing of the winner's.
    let preview = dir.join(format!("move-{}-{}.md", m.id, crate::secret::random_hex(8)));
    crate::secret::write_private(&preview, built.text.as_bytes()).map_err(|e| format!("the document could not be kept: {e}"))?;
    let sha = sha256_hex(built.text.as_bytes());
    let path = preview.to_string_lossy().into_owned();
    // The ask looked at again as the document is taken: the chat may have
    // gone on in another conversation while it was copied and built.
    let conversation = super::conversation_now(caller);
    let shown = store::change(caller, |r| match r.moving.as_mut().filter(|x| x.id == m.id && ask_here(x, conversation.as_deref())) {
        Some(x) => {
            x.phase = MovePhase::Shown { at: now_ms() };
            x.shown_sha256 = sha.clone();
            x.preview = Some(path.clone());
            x.cleaned = [built.redacted, built.paths_cut, built.invisible];
            true
        }
        None => false,
    });
    if !matches!(shown, Ok(true)) {
        let _ = std::fs::remove_file(&preview);
        return Err("the move was given up meanwhile: nothing is done with the document, and nothing switches".into());
    }
    crate::log::info("move", &format!("{} wrote its move's document ({} bytes from {})", label(caller), snap.size, snap.source));
    Ok(serde_json::json!({
        "collected": true,
        "next": "End your turn now. Nothing is cleared and nothing switches: this conversation goes on as it is. The \
                 person reads your document, with what Luna adds, and decides whether it goes.",
    }))
}

/// `luna_session_handoff` without `file` while the chat is asked for its
/// move's document: how to write that one, and where, by the handoff's
/// instructions in effect under the move's own rules (served.rs
/// `move_instructions`). None otherwise.
pub(super) fn ask_instructions(caller: &str) -> Result<Option<String>, String> {
    let Some(m) = store::records().get(caller).and_then(|r| r.moving.clone()).filter(Move::asking) else { return Ok(None) };
    if !ask_here(&m, super::conversation_now(caller).as_deref()) {
        return Err(NOT_ASKED_HERE.into());
    }
    let place = match super::drafts_refused(caller) {
        Some(_) => format!("a file of your own outside Luna's folder (in your workspace, say), named move-{}.md", m.id),
        None => {
            let drafts = super::drafts_of(caller)?;
            let at = drafts.join(format!("move-{}-{}.md", m.id, crate::secret::random_hex(4)));
            format!("{}, in a folder Luna keeps for this chat alone", at.display())
        }
    };
    Ok(Some(super::served::move_instructions(caller, &place)))
}

/// The moves whose chat is being asked, a step on: the ask typed at a strict
/// prompt, given up where the chat cannot or does not answer in time.
fn asks() {
    let pty = crate::pty::manager();
    let asking: Vec<(String, Move)> =
        store::records().iter().filter_map(|(c, r)| Some((c.clone(), r.moving.clone().filter(Move::asking)?))).collect();
    for (chat, m) in asking {
        if super::running().lock().unwrap_or_else(|e| e.into_inner()).contains(&chat) {
            continue;
        }
        let now = now_ms();
        // A question of its CLI's (a permission to write the file, say) is the person's to answer: it holds the ask, it fails nothing.
        let refused = ask_refusal(&chat, m.provider).filter(|w| !matches!(w, Unaskable::Question));
        match (m.phase.clone(), refused) {
            (_, Some(why)) => {
                let end = if matches!(why, Unaskable::NotRunning) { AskEnd::Exited } else { AskEnd::Limit };
                ask_ended(&chat, &m, end, format!("the chat cannot write its document: {}", why.words()))
            }
            (MovePhase::Asking { at }, None) => {
                if prompt_verdict(pty, &chat).is_ok() {
                    type_ask(&chat, &m);
                } else if now.saturating_sub(at) > ASK_REST_MS {
                    let why = format!("the chat did not come to rest within {} minutes", ASK_REST_MS / 60_000);
                    ask_ended(&chat, &m, AskEnd::Timeout, why);
                }
            }
            (MovePhase::Asked { .. }, None) if left_asked(&m, super::conversation_now(&chat).as_deref()) => {
                ask_ended(&chat, &m, AskEnd::Left, LEFT_ASKED.into())
            }
            (MovePhase::Asked { at }, None) if now.saturating_sub(at) > ASK_WAIT_MS => {
                ask_ended(&chat, &m, AskEnd::Timeout, format!("the chat wrote no document within {} minutes", ASK_WAIT_MS / 60_000))
            }
            _ => {}
        }
    }
}

/// Why a move's ask failed whose chat went on in another conversation.
pub(super) const LEFT_ASKED: &str = "the chat went on in another conversation, which was not asked for the document";

/// Whether move `m`'s ask, typed, was left by its chat, now on `now`: the
/// conversation it was typed into and the one the chat is on both known and
/// not the same. The person's `/clear` or `/new` started one nobody asked
/// (`ask_here` refuses its call), so waiting out ASK_WAIT_MS would only keep
/// the window saying the chat writes. Not while Luna is typing into it
/// (`asks` looks only then).
pub(super) fn left_asked(m: &Move, now: Option<&str>) -> bool {
    matches!(m.phase, MovePhase::Asked { .. }) && matches!((m.asked_conversation.as_deref(), now), (Some(asked), Some(now)) if asked != now)
}

/// Move `m`'s ask given up as `end` says, for `why`: kept on the move, for
/// whether asking again makes sense.
fn ask_ended(chat: &str, m: &Move, end: AskEnd, why: String) {
    failed_where(chat, m, why, Move::asking, move |x| x.ask_end = Some(end));
}

/// The ask typed into `chat` on a thread of its own, its terminal claimed,
/// under `Asked` taken first.
fn type_ask(chat: &str, m: &Move) {
    if !super::running().lock().unwrap_or_else(|e| e.into_inner()).insert(chat.to_string()) {
        return;
    }
    let (chat, m) = (chat.to_string(), m.clone());
    let held = chat.clone();
    let started = std::thread::Builder::new().name("luna-move-ask".into()).spawn(move || {
        let _running = super::Running(chat.clone());
        let Some(_typing) = TypingInto::wait_for(&chat, TYPING_WAIT) else { return };
        let commit = || {
            // Looked at again with the keys held: Codex steers a line typed
            // into a turn begun meanwhile.
            if !still_ready(crate::pty::manager(), &chat) {
                return Err(Refused::Gone);
            }
            // The ask answers only in the conversation it goes into, one Luna
            // can name: not known now, it is typed at a later beat.
            let Some(conversation) = super::conversation_now(&chat) else { return Err(Refused::Gone) };
            let taken = store::change(&chat, |r| match r.moving.as_mut().filter(|x| x.id == m.id) {
                Some(x) if matches!(x.phase, MovePhase::Asking { .. }) => {
                    let at = now_ms();
                    x.phase = MovePhase::Asked { at };
                    x.asked_conversation = Some(conversation.clone());
                    x.asked_at = Some(at);
                    x.asked_turns = crate::activity::turn_state(&chat).map(|t| t.turns_ended);
                    true
                }
                _ => false,
            });
            match taken {
                // Logged here, as it opens: the chat may answer it before the
                // paste below returns.
                Ok(true) => {
                    crate::log::info(
                        "move",
                        &format!("{}: asked for move {}'s document: on record as typed; its line goes in", label(&chat), m.id),
                    );
                    Ok(())
                }
                Ok(false) => Err(Refused::Gone),
                Err(e) => Err(Refused::Unsaved(e)),
            }
        };
        match super::type_line(&chat, &ask_line(&m.id), &commit) {
            Ok(Typed::In) => crate::log::info("move", &format!("{}: move {}'s ask went in; the CLI took it", label(&chat), m.id)),
            Ok(Typed::NotClear | Typed::NotCommitted(Refused::Gone)) => {}
            // Only while still asked: the person may have had Luna's document
            // built meanwhile.
            Ok(Typed::NotCommitted(Refused::Unsaved(e))) => {
                failed_where(&chat, &m, format!("its state could not be saved: {e}"), Move::asking, |_| {})
            }
            Err(e) => failed_where(&chat, &m, format!("the ask could not be typed: {e}"), Move::asking, |_| {}),
        }
    });
    if let Err(e) = started {
        super::running().lock().unwrap_or_else(|e| e.into_inner()).remove(&held);
        crate::log::warn("move", &format!("could not ask {held}: {e}"));
    }
}
/// What a look at the new chat finds.
#[derive(Clone, Debug, Default)]
pub(super) struct OpenLook {
    pub now: u64,
    pub alive: bool,
    /// At its prompt, strictly: the screen shows the composer, empty, its
    /// turn not under way, nobody typing. A first-run screen is not one.
    pub ready: bool,
    pub draft: bool,
    /// Its transcript holds the move's line.
    pub prompted: bool,
    /// Why its start is not the one confirmed, if it is not.
    pub mismatch: Option<String>,
    /// Why it is not ready, as `prompt_verdict` says.
    pub waiting: Option<&'static str>,
    /// The person said to type it now, and the screen shows an empty
    /// composer by its mark.
    pub forced: bool,
    /// The screen shows an empty composer by its mark (`composer_visible`):
    /// what "Type it now" is offered by.
    pub composer_seen: bool,
}

/// What a look calls for.
#[derive(Debug, PartialEq)]
pub(super) enum OpenStep {
    Wait,
    Type,
    Done,
    Fail(String),
}

pub(super) fn next_open(m: &Move, l: &OpenLook) -> OpenStep {
    if let Some(why) = &l.mismatch {
        return OpenStep::Fail(why.clone());
    }
    match m.phase {
        MovePhase::RowCreated { at } => {
            if l.prompted {
                OpenStep::Done
            } else if !l.alive {
                if l.now.saturating_sub(at) > NOT_RUNNING_MAX_MS {
                    OpenStep::Fail(format!(
                        "its CLI was not running for {} minutes; open the chat to start it, and move again if it does not go on",
                        NOT_RUNNING_MAX_MS / 60_000
                    ))
                } else {
                    OpenStep::Wait
                }
            } else if l.ready && !l.draft {
                OpenStep::Type
            } else {
                // A first-run screen, a person's draft: no shorter wait.
                OpenStep::Wait
            }
        }
        MovePhase::Prompting { at } => {
            if l.prompted {
                OpenStep::Done
            } else if l.now.saturating_sub(at) > PROMPT_WAIT_MS {
                OpenStep::Fail(format!("the new chat did not take Luna's line within {} s", PROMPT_WAIT_MS / 1000))
            } else {
                OpenStep::Wait
            }
        }
        _ => OpenStep::Wait,
    }
}

/// Whether the new chat started as confirmed: the CLI, the account, the
/// folder, and the settings that say what it may do.
pub(super) fn start_mismatch(
    m: &Move,
    provider: Provider,
    account_path: &str,
    folder: &str,
    settings: &Value,
    tools: bool,
) -> Option<String> {
    let launch = m.launch.as_ref()?;
    let same = |a: &str, b: &str| crate::paths::path_key(a) == crate::paths::path_key(b);
    if provider != m.provider || !same(account_path, &m.to_account_path) {
        return Some("the new chat started on another account than the one confirmed".into());
    }
    if launch["folder"].as_str().is_some_and(|f| !same(f, folder)) {
        return Some("the new chat started in another folder than the one confirmed".into());
    }
    let differs = |ours: &str, theirs: &str| launch[theirs].as_str().is_some_and(|v| settings[ours].as_str() != Some(v));
    let wider = match provider {
        Provider::Claude => differs("permissionMode", "permissionMode") || (launch["readOnly"] == true && settings["readOnly"] != true),
        Provider::Codex => {
            differs("approval", "approval") || (differs("sandbox", "sandbox") && settings["approval"] != crate::codex::launch::AUTO_REVIEW)
        }
    };
    let more = (tools && launch["tools"] != true)
        || (provider == Provider::Claude && settings["agent"].as_str().unwrap_or("") != launch["agent"].as_str().unwrap_or(""))
        || (provider == Provider::Codex && settings["hooksUnasked"] == true && launch["hooksUnasked"] != true);
    (wider || more).then(|| "the new chat started with other permissions than the ones confirmed".into())
}

/// The new chat's terminal and records, as `next_open` looks at them.
fn look(m: &Move) -> OpenLook {
    let pty = crate::pty::manager();
    let new = m.new_id.as_str();
    let alive = pty.alive(new);
    let mut l = OpenLook { now: now_ms(), alive, ..Default::default() };
    let known = reg()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .known
        .get(new)
        .map(|k| (k.provider, k.account_path.clone(), k.folder.clone(), k.settings.clone(), k.tools));
    // Not registered (a start that is no chat's, a login): nothing to hold
    // to what was confirmed, so nothing is typed.
    let Some((provider, account, folder, settings, tools)) = known else { return l };
    l.mismatch = start_mismatch(m, provider, &account, &folder, &settings, tools);
    if !alive {
        return l;
    }
    l.prompted = crate::agents::read::transcript_path(pty, new)
        .and_then(|p| crate::pty::tail(&p, 1024 * 1024))
        .is_some_and(|t| t.contains(&line_opening(&m.id)));
    let verdict = prompt_verdict(pty, new);
    // The person's "Type it now" overrules the layout look alone (rules, a
    // footer a cramped pane draws otherwise): never a dialog or first-run
    // screen, an unreadable or written composer, a draft, a turn at work.
    l.composer_seen = pty.composer_visible(new);
    l.forced = m.forced && verdict == Err(NO_PROMPT) && l.composer_seen;
    l.draft = pty.has_draft(new) || pty.keys_left_text(new);
    l.ready = verdict.is_ok() || l.forced;
    l.waiting = verdict.err();
    l
}

/// Each opening's last verdict, by move: the log says it once per change, and
/// the window shows it on the new chat with "Type it now".
fn verdicts() -> &'static Mutex<HashMap<String, (&'static str, u64, bool)>> {
    static V: std::sync::OnceLock<Mutex<HashMap<String, (&'static str, u64, bool)>>> = std::sync::OnceLock::new();
    V.get_or_init(Default::default)
}

/// How long an opening may wait at a screen before the log has a copy of it,
/// and the window offers "Type it now".
pub(super) const STALL_MS: u64 = 2 * 60_000;

/// `l`'s verdict on move `m`, logged as it changes; past STALL_MS on one, the
/// new chat's screen once, so a stall can be read off luna.log.
fn note_verdict(m: &Move, l: &OpenLook) {
    let mut s = seen().lock().unwrap_or_else(|e| e.into_inner());
    if l.composer_seen {
        s.insert(m.id.clone());
    } else {
        s.remove(&m.id);
    }
    drop(s);
    let why = if l.ready { "ready" } else { l.waiting.unwrap_or(if l.alive { "waiting" } else { "its CLI is not running" }) };
    let mut v = verdicts().lock().unwrap_or_else(|e| e.into_inner());
    let entry = v.entry(m.id.clone()).or_insert((why, l.now, false));
    if entry.0 != why {
        *entry = (why, l.now, false);
        crate::log::info("move", &format!("move {}: opening {}: {why}", m.id, m.new_id));
    }
    if why != "ready" && !entry.2 && l.now.saturating_sub(entry.1) > STALL_MS {
        entry.2 = true;
        let screen = crate::pty::manager().screen(&m.new_id).unwrap_or_default();
        crate::log::warn(
            "move",
            &format!("move {}: opening {} waits {} min on \"{why}\"; its screen:\n{screen}", m.id, m.new_id, STALL_MS / 60_000),
        );
    }
}

/// The opening's last look found an empty composer by its mark.
fn seen_of(m: &Move) -> bool {
    seen().lock().unwrap_or_else(|e| e.into_inner()).contains(&m.id)
}

fn seen() -> &'static Mutex<std::collections::HashSet<String>> {
    static S: std::sync::OnceLock<Mutex<std::collections::HashSet<String>>> = std::sync::OnceLock::new();
    S.get_or_init(Default::default)
}

/// What the window is told of an opening that waits: why, and since when.
fn waiting_of(m: &Move) -> Option<(String, u64)> {
    let v = verdicts().lock().unwrap_or_else(|e| e.into_inner());
    v.get(&m.id).filter(|(why, ..)| *why != "ready").map(|(why, since, _)| (why.to_string(), *since))
}

/// Whether `id` still stands at its prompt between turns, looked at the moment
/// before text goes in: the beat's look may be a while old by then.
pub(super) fn still_ready(pty: &crate::pty::PtyManager, id: &str) -> bool {
    ready_now(pty.at_prompt(id), crate::activity::turn_state(id).map(|t| t.turn))
}

/// The look at delivery, with the keys held, forced or not: no turn at
/// work, no draft, and the prompt as the layout shows it or, where the
/// person said to type now, the empty composer by its mark.
fn ready_at_keys(pty: &crate::pty::PtyManager, id: &str, forced: bool) -> bool {
    let shown = pty.at_prompt(id) || (forced && pty.composer_visible(id));
    ready_now(shown, crate::activity::turn_state(id).map(|t| t.turn)) && !pty.has_draft(id) && !pty.keys_left_text(id)
}

fn ready_now(at_prompt: bool, turn: Option<crate::activity::Turn>) -> bool {
    at_prompt && turn != Some(crate::activity::Turn::Busy)
}

/// Each move whose new chat is opening, a step on: the line typed once the
/// chat stands at its prompt, done once its transcript holds it, given up
/// where its start is not the one confirmed or its wait is out.
pub(super) fn beat() {
    asks();
    let opening: Vec<(String, Move)> = store::records()
        .iter()
        .filter_map(|(c, r)| Some((c.clone(), r.moving.clone().filter(|m| m.phase.rank() == 3 || m.phase.rank() == 4)?)))
        .collect();
    for (chat, m) in opening {
        if super::running().lock().unwrap_or_else(|e| e.into_inner()).contains(&m.new_id) {
            continue;
        }
        let l = look(&m);
        note_verdict(&m, &l);
        match next_open(&m, &l) {
            OpenStep::Wait => {}
            OpenStep::Type => open_with_line(&chat, &m),
            OpenStep::Done => done(&chat, &m),
            OpenStep::Fail(why) => failed(&chat, &m, why),
        }
    }
}

/// The line typed into the new chat on a thread of its own, its terminal
/// claimed, under the phase `Prompting` taken first (on disk before the
/// text): a move given up meanwhile types nothing.
fn open_with_line(chat: &str, m: &Move) {
    let (Some(payload), Some(sha)) = (m.payload.clone(), m.payload_sha256.clone()) else {
        return failed(chat, m, "its document is not on record".into());
    };
    // The copy is what Luna wrote, still: right in the new chat's folder of
    // copies, private, its hash the one confirmed.
    let holds = copies_of(&m.new_id).ok().is_some_and(|dir| Path::new(&payload).parent() == Some(dir.as_path()))
        && crate::secret::read_private(Path::new(&payload), document::MAX_BYTES as u64 + 1)
            .ok()
            .flatten()
            .is_some_and(|b| sha256_hex(&b) == sha);
    if !holds {
        return failed(chat, m, "its document is not the one confirmed any more".into());
    }
    // The line as the preview showed it and Send bound it: never made again.
    let text = m.line.clone();
    if text.is_empty() {
        return failed(chat, m, "its line is not on record".into());
    }
    let new = m.new_id.clone();
    if !super::running().lock().unwrap_or_else(|e| e.into_inner()).insert(new.clone()) {
        return;
    }
    let held = new.clone();
    let (chat, m) = (chat.to_string(), m.clone());
    let started = std::thread::Builder::new().name("luna-move-open".into()).spawn(move || {
        let _running = super::Running(new.clone());
        let Some(_typing) = TypingInto::wait_for(&new, TYPING_WAIT) else { return };
        // Taken only from RowCreated: a move given up, or typed already,
        // types nothing.
        let commit = || match current(&chat, &m.id) {
            // Looked at again with the keys held: a turn started meanwhile (a
            // send, a notice) would take the line mid-turn.
            _ if !ready_at_keys(crate::pty::manager(), &new, m.forced) => Err(Refused::Gone),
            Ok(x) if x.phase.rank() == 3 => {
                advance(&chat, &m.id, MovePhase::Prompting { at: now_ms() }, |_| Ok(())).map(|_| ()).map_err(Refused::Unsaved)
            }
            _ => Err(Refused::Gone),
        };
        match super::type_line_with(&new, &text, &commit, true) {
            Ok(Typed::In) => crate::log::info("move", &format!("{}: move {} typed its line into {new}", label(&chat), m.id)),
            Ok(Typed::NotClear | Typed::NotCommitted(Refused::Gone)) => {}
            Ok(Typed::NotCommitted(Refused::Unsaved(e))) => failed(&chat, &m, format!("its state could not be saved: {e}")),
            Err(e) => failed(&chat, &m, format!("the line could not be typed: {e}")),
        }
    });
    if let Err(e) = started {
        super::running().lock().unwrap_or_else(|e| e.into_inner()).remove(&held);
        crate::log::warn("move", &format!("could not start opening {held}: {e}"));
    }
}

/// The move went through: on record on both sides, the document kept as the
/// new chat's first copy.
fn done(chat: &str, m: &Move) {
    let Ok(ended) = end(chat, &m.id, MovePhase::Done { at: now_ms() }) else { return };
    if !matches!(ended.phase, MovePhase::Done { .. }) {
        return;
    }
    let past = store::Past {
        n: PAYLOAD_N,
        at: now_ms(),
        provider: m.provider,
        snapshot: m.payload.clone().unwrap_or_default(),
        sha256: m.payload_sha256.clone().unwrap_or_default(),
        previous: None,
        done: true,
        why: None,
        instructions: None,
        moved: true,
    };
    if let Err(e) = store::change(&m.new_id, |r| {
        if !r.past.iter().any(|p| p.n == PAYLOAD_N) {
            r.past.push(past);
        }
    }) {
        crate::log::warn("move", &format!("{}: its move could not be kept on record: {e}", m.new_id));
    }
    crate::log::info("move", &format!("{} moved to {}/{} as {}", label(chat), m.provider.as_str(), m.to_account, m.new_id));
}

/// The move given up: the new chat, if made, stays with its document where
/// it is, the old chat untouched, and the person told.
fn failed(chat: &str, m: &Move, why: String) {
    failed_where(chat, m, why, |_| true, |_| {})
}

/// `failed`, only where the move as it stands then is one `may` takes, with
/// `also` done to it in the same change (`end_where`).
fn failed_where(chat: &str, m: &Move, why: String, may: impl Fn(&Move) -> bool, also: impl Fn(&mut Move)) {
    let Ok((ended, true)) = end_where(chat, &m.id, MovePhase::Failed { at: now_ms(), why: why.clone() }, may, also) else { return };
    if !matches!(ended.phase, MovePhase::Failed { .. }) {
        return;
    }
    crate::log::warn("move", &format!("{} gave up move {} to {}: {why}", label(chat), m.id, m.new_id));
    // A new chat already made says on its divider that its move did not go
    // through, and why: a row without its line is no mystery.
    if m.phase.rank() >= 3 {
        let past = store::Past {
            n: PAYLOAD_N,
            at: now_ms(),
            provider: m.provider,
            snapshot: m.payload.clone().unwrap_or_default(),
            sha256: m.payload_sha256.clone().unwrap_or_default(),
            previous: None,
            done: false,
            why: Some(why.clone()),
            instructions: None,
            moved: true,
        };
        let _ = store::change(&m.new_id, |r| {
            if !r.past.iter().any(|p| p.n == PAYLOAD_N) {
                r.past.push(past);
            }
        });
    }
    let name = chat_name(chat).unwrap_or_else(|| chat.to_string());
    let kept = m.payload.as_deref().map(|p| format!(" Its document is kept at {p}.")).unwrap_or_default();
    crate::power::notify_chat(chat, "A move did not go through", &format!("\"{name}\": {why}.{kept}"));
}

/// What a core taking the record up does with a move: a preview nobody
/// confirmed is given up (it is never sent on its own), and a line typed
/// that its transcript does not show yet is looked at again from the new
/// chat's prompt.
pub(super) fn taken_up(m: &mut Move, now: u64) {
    match m.phase {
        MovePhase::Shown { .. } => {
            m.phase = MovePhase::Failed { at: now, why: "Luna restarted before the preview was confirmed; nothing was sent".into() };
        }
        MovePhase::Asking { .. } | MovePhase::Asked { .. } => {
            m.phase = MovePhase::Failed { at: now, why: "Luna restarted while the chat was asked for its document".into() };
        }
        MovePhase::RowCreated { .. } | MovePhase::Prompting { .. } => m.phase = MovePhase::RowCreated { at: now },
        _ => {}
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    pub(in crate::agents::handoff) fn a_move(phase: MovePhase) -> Move {
        Move {
            id: "m1".into(),
            new_id: "c-new".into(),
            provider: Provider::Claude,
            from_account: "team".into(),
            to_account: "backend".into(),
            to_account_path: "C:/acc/anthropic/backend".into(),
            folder: "C:/p".into(),
            line: "[Luna move m1, not typed by the user] Read the document.".into(),
            source: Source::Luna,
            stop_old: false,
            worktree: false,
            phase,
            shown_sha256: "ab".into(),
            binding: None,
            launch: Some(
                serde_json::json!({ "provider": "claude", "accountPath": "C:/acc/anthropic/backend", "folder": "C:/p", "permissionMode": "default" }),
            ),
            payload: Some("/d/handoffs/c-new/1.md".into()),
            payload_sha256: Some("cd".into()),
            note: String::new(),
            preview: None,
            cleaned: [0; 3],
            forced: false,
            asked_at: None,
            asked_turns: None,
            ask_end: None,
            asked_conversation: None,
        }
    }

    fn ready(now: u64) -> OpenLook {
        OpenLook { now, alive: true, ready: true, ..Default::default() }
    }

    #[test]
    fn the_line_goes_in_only_at_a_strict_prompt_with_no_short_wait() {
        let m = a_move(MovePhase::RowCreated { at: 1_000 });
        assert_eq!(next_open(&m, &ready(2_000)), OpenStep::Type);
        // A first-run screen (theme, security notes, trust), or a draft: it
        // waits, however long, while the CLI runs.
        let screen = OpenLook { ready: false, ..ready(1_000 + 3 * 24 * 60 * 60_000) };
        assert_eq!(next_open(&m, &screen), OpenStep::Wait);
        assert_eq!(next_open(&m, &OpenLook { draft: true, ..ready(2_000) }), OpenStep::Wait);
        // Not running: up to ten minutes, then given up with why on its row.
        let gone = OpenLook { alive: false, ..ready(2_000) };
        assert_eq!(next_open(&m, &gone), OpenStep::Wait);
        assert!(
            matches!(next_open(&m, &OpenLook { now: 1_001 + NOT_RUNNING_MAX_MS, ..gone }), OpenStep::Fail(w) if w.contains("not running for 10 minutes"))
        );
        // Its transcript holds it already (a core that stopped after typing).
        assert_eq!(next_open(&m, &OpenLook { prompted: true, ready: false, ..ready(2_000) }), OpenStep::Done);
        // Typed: done once the transcript shows it, failed past the wait.
        let typed = a_move(MovePhase::Prompting { at: 10_000 });
        assert_eq!(next_open(&typed, &OpenLook { ready: false, ..ready(11_000) }), OpenStep::Wait);
        assert_eq!(next_open(&typed, &OpenLook { prompted: true, ..ready(11_000) }), OpenStep::Done);
        assert!(matches!(next_open(&typed, &ready(10_001 + PROMPT_WAIT_MS)), OpenStep::Fail(_)));
        // Started otherwise than confirmed: never typed into.
        let other = OpenLook { mismatch: Some("other permissions".into()), ..ready(2_000) };
        assert!(matches!(next_open(&m, &other), OpenStep::Fail(w) if w.contains("other permissions")));
    }

    #[test]
    fn the_person_can_have_the_line_typed_where_the_screen_check_sees_no_prompt() {
        let m = a_move(MovePhase::RowCreated { at: 1_000 });
        let stuck = OpenLook { ready: false, waiting: Some("its screen shows no empty prompt"), ..ready(1_000 + STALL_MS * 5) };
        assert_eq!(next_open(&m, &stuck), OpenStep::Wait, "it waits, however long, unless told");
        let told = OpenLook { ready: true, forced: true, composer_seen: true, ..stuck };
        assert_eq!(next_open(&m, &told), OpenStep::Type);
        // A draft holds even a forced line.
        assert_eq!(next_open(&m, &OpenLook { draft: true, ..told }), OpenStep::Wait);
    }

    #[test]
    fn a_signed_out_account_takes_no_move() {
        let dir = std::env::temp_dir().join(format!("luna-move-signed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.to_string_lossy().into_owned();
        assert!(signed_out_refusal(Provider::Claude, &p, "rOut").is_some_and(|w| w.contains("rOut is signed out")));
        std::fs::write(dir.join(".credentials.json"), r#"{"claudeAiOauth":{"refreshToken":"x"}}"#).unwrap();
        assert_eq!(signed_out_refusal(Provider::Claude, &p, "rA"), None);
        assert!(signed_out_refusal(Provider::Codex, &p, "c").is_some(), "Codex keeps its own");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_start_is_held_to_what_was_confirmed() {
        let m = a_move(MovePhase::RowCreated { at: 1 });
        let ok = serde_json::json!({ "permissionMode": "default", "model": "opus" });
        assert_eq!(start_mismatch(&m, Provider::Claude, "C:/acc/anthropic/backend", "C:/p", &ok, false), None);
        let wider = serde_json::json!({ "permissionMode": "bypassPermissions" });
        assert!(start_mismatch(&m, Provider::Claude, "C:/acc/anthropic/backend", "C:/p", &wider, false)
            .is_some_and(|w| w.contains("permissions")));
        assert!(start_mismatch(&m, Provider::Claude, "C:/acc/anthropic/team", "C:/p", &ok, false).is_some_and(|w| w.contains("account")));
        assert!(start_mismatch(&m, Provider::Claude, "C:/acc/anthropic/backend", "C:/q", &ok, false).is_some_and(|w| w.contains("folder")));
        assert!(start_mismatch(&m, Provider::Codex, "C:/acc/anthropic/backend", "C:/p", &ok, false).is_some());
        // Luna's tools or an agent nobody confirmed: other permissions.
        assert!(start_mismatch(&m, Provider::Claude, "C:/acc/anthropic/backend", "C:/p", &ok, true).is_some());
        let agent = serde_json::json!({ "permissionMode": "default", "agent": "reviewer" });
        assert!(start_mismatch(&m, Provider::Claude, "C:/acc/anthropic/backend", "C:/p", &agent, false).is_some());
    }

    #[test]
    fn send_is_bound_to_the_text_the_destination_and_the_launch() {
        let launch = serde_json::json!({ "provider": "claude", "perm": "default" });
        let b = binding("t1", Provider::Claude, "C:/acc/a", &launch, "line");
        assert_eq!(b, binding("t1", Provider::Claude, "C:/acc/a", &launch, "line"));
        assert_ne!(b, binding("t2", Provider::Claude, "C:/acc/a", &launch, "line"));
        assert_ne!(b, binding("t1", Provider::Claude, "C:/acc/b", &launch, "line"));
        assert_ne!(
            b,
            binding("t1", Provider::Claude, "C:/acc/a", &serde_json::json!({ "provider": "claude", "perm": "acceptEdits" }), "line")
        );
        assert_ne!(b, binding("t1", Provider::Claude, "C:/acc/a", &launch, "another line"), "the line typed is bound too");
    }

    #[test]
    fn a_launch_is_refused_where_the_move_does_not_allow_it() {
        let mut m = a_move(MovePhase::Shown { at: 1 });
        let ok = m.launch.clone().unwrap();
        assert_eq!(launch_refusal(&m, &ok), None);
        let mut other = ok.clone();
        other["provider"] = "codex".into();
        assert!(launch_refusal(&m, &other).is_some());
        let mut elsewhere = ok.clone();
        elsewhere["accountPath"] = "C:/acc/anthropic/team".into();
        assert!(launch_refusal(&m, &elsewhere).is_some());
        // A Claude Code worktree: by its name, or not at all.
        m.worktree = true;
        assert!(launch_refusal(&m, &ok).is_some_and(|w| w.contains("worktree")));
        let mut named = ok.clone();
        named["worktreeName"] = "mv-test".into();
        assert_eq!(launch_refusal(&m, &named), None);
        // Codex makes no worktree for a moved chat.
        let mut c = a_move(MovePhase::Shown { at: 1 });
        c.provider = Provider::Codex;
        let codex = serde_json::json!({ "provider": "codex", "accountPath": "C:/acc/anthropic/backend", "worktree": true });
        assert!(launch_refusal(&c, &codex).is_some());
    }

    #[test]
    fn a_codex_chat_in_a_worktree_of_its_account_does_not_move() {
        let sep = std::path::MAIN_SEPARATOR;
        let acc = format!("{sep}acc{sep}openai{sep}one");
        let wt = format!("{acc}{sep}worktrees{sep}abb1{sep}repo");
        assert!(codex_worktree_refusal(Provider::Codex, &wt, &acc, false).is_some_and(|w| w.contains("does not move")));
        assert_eq!(codex_worktree_refusal(Provider::Codex, &format!("{sep}work{sep}repo"), &acc, false), None);
        // Started with --worktree, before its first turn: still in the project
        // folder as far as any rollout says, its checkout the account's.
        assert!(codex_worktree_refusal(Provider::Codex, &format!("{sep}work{sep}repo"), &acc, true).is_some());
        assert_eq!(codex_worktree_refusal(Provider::Claude, &wt, &acc, true), None, "a Claude Code worktree is in the repository");
    }

    #[test]
    fn a_restart_never_sends_an_unconfirmed_preview() {
        let mut shown = a_move(MovePhase::Shown { at: 1 });
        taken_up(&mut shown, 50);
        assert!(matches!(&shown.phase, MovePhase::Failed { why, .. } if why.contains("nothing was sent")));
        let mut typed = a_move(MovePhase::Prompting { at: 1 });
        taken_up(&mut typed, 50);
        assert_eq!(typed.phase, MovePhase::RowCreated { at: 50 }, "looked at again: typed only if its transcript lacks it");
        let mut confirmed = a_move(MovePhase::Confirmed { at: 1 });
        taken_up(&mut confirmed, 50);
        assert_eq!(confirmed.phase, MovePhase::Confirmed { at: 1 }, "the person confirmed it: the window goes on");
    }

    /// A move of `chat` to `new` as shown, nothing of either on record before.
    fn shown(chat: &str, new: &str, worktree: bool) -> Move {
        for c in [chat, new] {
            let _ = store::remove(c);
            if let Ok(d) = super::super::dir_of(c) {
                let _ = std::fs::remove_dir_all(d);
            }
        }
        let mut m = a_move(MovePhase::Shown { at: 1 });
        m.new_id = new.into();
        m.worktree = worktree;
        store::change(chat, |r| r.moving = Some(m.clone())).unwrap();
        m
    }

    #[test]
    fn send_goes_once_bound_to_what_was_shown_and_each_step_is_looked_at() {
        let (chat, new) = ("mv-old", "mv-new");
        let m = shown(chat, new, false);
        let roots = scrub::Roots::of(&[("C:\\Luna", scrub::LUNA_FOLDER)]);
        let ok = |_: &str| Ok(());
        let text = "# Moved chat\nGo on.\n";
        let sha = sha256_hex(text.as_bytes());
        let launch = m.launch.clone().unwrap();
        // Not the text whose hash the window showed: refused.
        assert!(confirm_with(chat, "m1", "edited", &sha, &launch, &roots, &ok).unwrap_err().contains("not the one shown"));
        // A folder of Luna's put back in by an edit: refused, nothing kept.
        let bad = "see C:\\Luna\\core\\core.json";
        let e = confirm_with(chat, "m1", bad, &sha256_hex(bad.as_bytes()), &launch, &roots, &ok).unwrap_err();
        assert!(e.contains("folder(s) of an account or of Luna's own"), "{e}");
        let copy = super::super::dir_of(new).unwrap().join("1.md");
        assert!(!copy.exists());
        // The account gone meanwhile: refused.
        let gone = |_: &str| Err("that account is not one of Luna's".to_string());
        assert!(confirm_with(chat, "m1", text, &sha, &launch, &roots, &gone).is_err());
        // Sent: kept private, both sides on record.
        let s = confirm_with(chat, "m1", text, &sha, &launch, &roots, &ok).unwrap();
        assert_eq!((s.phase, s.stop_old), ("confirmed", false));
        assert_eq!(std::fs::read_to_string(&copy).unwrap(), text);
        assert_eq!(store::records()[new].moved_from.as_ref().map(|f| f.chat.as_str()), Some(chat));
        assert!(moved_dir(new).is_some(), "its start reads it without asking");
        // Sent twice: once. Then another text or launch: refused.
        assert_eq!(confirm_with(chat, "m1", text, &sha, &launch, &roots, &ok).unwrap().phase, "confirmed");
        let mut wider = launch.clone();
        wider["permissionMode"] = "bypassPermissions".into();
        assert!(confirm_with(chat, "m1", text, &sha, &wider, &roots, &ok).unwrap_err().contains("confirmed already"));
        // The window's steps, each taken again as it stands.
        assert!(move_step(chat.into(), "m1".into(), "nonsense".into()).is_err());
        assert_eq!(move_step(chat.into(), "m1".into(), "rowCreated".into()).unwrap().phase, "rowCreated");
        assert_eq!(move_step(chat.into(), "m1".into(), "rowCreated".into()).unwrap().phase, "rowCreated");
        // Canceled once its row is there: nothing typed, the new chat keeps its copy.
        assert_eq!(move_cancel(chat.into(), "m1".into()).unwrap().phase, "canceled");
        assert!(copy.exists() && store::records()[new].moved_from.is_some());
        assert!(move_list().iter().any(|s| s.move_id == "m1" && s.phase == "canceled"));
        for c in [chat, new] {
            let _ = store::remove(c);
        }
        let _ = std::fs::remove_dir_all(super::super::dir_of(new).unwrap());
    }

    #[test]
    fn a_claude_worktree_move_stops_the_old_chat_before_the_row_and_a_cancel_before_it_undoes_all() {
        let (chat, new) = ("mvw-old", "mvw-new");
        let m = shown(chat, new, true);
        let roots = scrub::Roots::default();
        let ok = |_: &str| Ok(());
        let text = "# Moved chat\n";
        let sha = sha256_hex(text.as_bytes());
        let mut launch = m.launch.clone().unwrap();
        // Kept by the box, stopped all the same: a Claude Code worktree.
        launch["stopOld"] = false.into();
        assert!(confirm_with(chat, "m1", text, &sha, &launch, &roots, &ok).unwrap_err().contains("worktree"));
        launch["worktreeName"] = "mv-test".into();
        let s = confirm_with(chat, "m1", text, &sha, &launch, &roots, &ok).unwrap();
        assert!(s.stop_old && s.worktree);
        let e = move_step(chat.into(), "m1".into(), "rowCreated".into()).unwrap_err();
        assert!(e.contains("stopped first"), "{e}");
        assert_eq!(worktree_ties(&store::records(), new), [chat], "both in the worktree from Send on");
        // Something owed to it meanwhile: the core does not stop it.
        reg().lock().unwrap().owed.insert((chat.into(), "mvw-peer".into()), crate::agents::owed::Owed::pending(1, 0));
        assert!(move_step(chat.into(), "m1".into(), "stopOld".into()).unwrap_err().contains("owed"));
        reg().lock().unwrap().owed.remove(&(chat.to_string(), "mvw-peer".to_string()));
        // Not running here: stopped by the core, then the row.
        assert_eq!(move_step(chat.into(), "m1".into(), "stopOld".into()).unwrap().phase, "oldStopped");
        assert_eq!(move_step(chat.into(), "m1".into(), "stopOld".into()).unwrap().phase, "oldStopped", "taken again");
        assert_eq!(opening_refusal(new), None, "the old chat runs no more");
        // Canceled before the row: its copy and its record go.
        let copy = super::super::dir_of(new).unwrap().join("1.md");
        assert!(copy.exists());
        assert_eq!(move_cancel(chat.into(), "m1".into()).unwrap().phase, "canceled");
        assert!(!copy.exists() && !store::records().contains_key(new));
        assert!(move_cancel(chat.into(), "m1".into()).is_ok(), "twice: as it stands");
        let _ = store::remove(chat);
    }
    #[test]
    fn a_worktree_tie_holds_across_every_chat_in_it_whatever_goes() {
        let key = worktree_key("C:/p", "mv-test");
        let in_it = |k: &str| store::Record { worktree_key: Some(k.to_string()), ..Default::default() };
        let mut r = store::Records::new();
        // A moved to B, B to C: all three in one worktree.
        for c in ["wt-a", "wt-b", "wt-c"] {
            r.insert(c.into(), in_it(&key));
        }
        r.insert("wt-other".into(), in_it(&worktree_key("C:/p", "another")));
        let mut tied = worktree_ties(&r, "wt-a");
        tied.sort();
        assert_eq!(tied, ["wt-b", "wt-c"]);
        // B deleted: A and C still never run together.
        r.remove("wt-b");
        assert_eq!(worktree_ties(&r, "wt-a"), ["wt-c"]);
        assert_eq!(worktree_ties(&r, "wt-c"), ["wt-a"]);
        assert!(worktree_ties(&r, "wt-other").is_empty() && worktree_ties(&r, "nobody").is_empty());
    }
    #[test]
    fn a_worktree_move_waits_for_what_its_old_chat_is_owed() {
        assert_eq!(worktree_held("mvh-old"), None);
        reg().lock().unwrap().owed.insert(("mvh-old".into(), "mvh-peer".into()), crate::agents::owed::Owed::pending(1, 0));
        let why = worktree_held("mvh-old").unwrap();
        assert!(why.contains("1 answer(s) owed") && why.contains("send again"), "{why}");
        // Refused at Send.
        let (chat, new) = ("mvh-old", "mvh-new");
        let m = shown(chat, new, true);
        let mut launch = m.launch.clone().unwrap();
        launch["worktreeName"] = "mv".into();
        let text = "# Moved\n";
        let e = confirm_with(chat, "m1", text, &sha256_hex(text.as_bytes()), &launch, &scrub::Roots::default(), &|_| Ok(())).unwrap_err();
        assert!(e.contains("owed"), "{e}");
        reg().lock().unwrap().owed.remove(&("mvh-old".to_string(), "mvh-peer".to_string()));
        let _ = store::remove(chat);
    }

    #[test]
    fn the_line_goes_in_only_at_a_prompt_between_turns_as_it_is_typed() {
        use crate::activity::Turn;
        assert!(ready_now(true, Some(Turn::Idle)) && ready_now(true, None));
        assert!(!ready_now(false, Some(Turn::Idle)), "a dialog or a screen of its own");
        assert!(!ready_now(true, Some(Turn::Busy)), "a turn begun meanwhile");
        // A CLI that says it waits at an empty composer asks nothing: after
        // its first-run screens, an idle reminder. A real question is a dialog.
        assert!(ready_now(true, Some(Turn::Waiting)));
    }

    #[test]
    fn the_persons_instruction_goes_in_the_line_as_theirs() {
        let line = prompt_line_with("m1", Source::Luna, "c1", None, "p", "Run the\ntests first. sk-ant-api03-0123456789abcdefghijklmnop");
        assert!(line.contains("The person's own instruction for this chat"), "{line}");
        assert!(line.contains("\"Run the tests first. <redacted>\""), "one line, cleaned: {line}");
        crate::agents::privilege::plain_text(&line).expect("a line Luna types passes its own check");
        assert_eq!(prompt_line_with("m1", Source::Luna, "c1", None, "p", "  \n"), prompt_line("m1", Source::Luna, "c1", None, "p"));
    }

    #[test]
    fn concurrent_sends_of_one_move_confirm_it_once_and_keep_its_copy() {
        let (chat, new) = ("mvc-old", "mvc-new");
        let m = shown(chat, new, false);
        let text = "# Moved\nGo on.\n";
        let sha = sha256_hex(text.as_bytes());
        let launch = m.launch.clone().unwrap();
        let send = || {
            let (sha, launch) = (sha.clone(), launch.clone());
            std::thread::spawn(move || {
                confirm_with("mvc-old", "m1", "# Moved\nGo on.\n", &sha, &launch, &scrub::Roots::default(), &|_| Ok(()))
            })
        };
        let (a, b) = (send(), send());
        assert!(a.join().unwrap().is_ok() && b.join().unwrap().is_ok(), "the second answers as the first did");
        let copy = super::super::dir_of(new).unwrap().join("1.md");
        assert_eq!(std::fs::read_to_string(&copy).unwrap(), text);
        for c in [chat, new] {
            let _ = store::remove(c);
        }
        let _ = std::fs::remove_dir_all(super::super::dir_of(new).unwrap());
    }

    #[test]
    fn a_move_given_up_after_its_row_says_why_on_the_new_chat() {
        let (chat, new) = ("mvf-old", "mvf-new");
        let mut m = shown(chat, new, false);
        m.phase = MovePhase::RowCreated { at: 1 };
        store::change(chat, |r| r.moving = Some(m.clone())).unwrap();
        failed(chat, &m, "the new chat did not take Luna's line within 60 s".into());
        let mark = super::super::mark_of(new).unwrap();
        assert!(mark.moved && mark.state == "failed" && mark.why.as_deref().is_some_and(|w| w.contains("60 s")));
        for c in [chat, new] {
            let _ = store::remove(c);
        }
    }

    #[test]
    fn the_old_chats_name_in_the_line_is_cleaned_as_the_document_is() {
        let key = format!("sk-ant-api03-{}", "a1B2".repeat(10));
        let line = prompt_line("m1", Source::Luna, "c1", Some(&format!("use {key}\u{200B} now")), "p");
        assert!(!line.contains(&key) && !line.contains('\u{200B}') && line.contains("<redacted>"), "{line}");
    }

    #[test]
    fn a_chat_asked_writes_its_document_through_its_tool_and_nothing_resets() {
        let (chat, new) = ("mva-old", "mva-new");
        let mut m = shown(chat, new, false);
        crate::agents::registry::register(
            chat,
            Provider::Claude,
            "C:/p",
            "C:/acc/anthropic/one",
            None,
            true,
            // No threshold: a chat with Luna's tools has the handoff's tool,
            // and the move's ask is what it answers.
            serde_json::json!({ "permissionMode": "acceptEdits" }),
        );
        m.source = Source::Chat;
        m.phase = MovePhase::Asked { at: now_ms() };
        m.asked_conversation = Some("old".into());
        m.note = "Keep going.".into();
        store::change(chat, |r| r.moving = Some(m.clone())).unwrap();
        super::super::set_conversation_for_test(chat, Some("old"));
        assert!(store::records()[chat].moving.as_ref().is_some_and(Move::asking), "no threshold notice meanwhile");
        // Served the move's own text, with a drafts name for it.
        super::super::drafts_at_start(chat).unwrap();
        let how = ask_instructions(chat).unwrap().unwrap();
        for want in ["move to another account", &format!("move-{}-", m.id), "Nothing is cleared", "not approvals"] {
            assert!(how.contains(want), "{want}:\n{how}");
        }
        let line = ask_line(&m.id);
        assert!(line.starts_with("[Luna move m1 request, not typed by the user]"));
        crate::agents::privilege::plain_text(&line).expect("an ask Luna types passes its own check");
        // Its file: collected, the move shown, nothing scheduled or reset.
        let dir = std::env::temp_dir().join(format!("luna-move-ask-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("move-doc.md");
        std::fs::write(&file, "# Goal\nShip it.\n").unwrap();
        let v = asked_file(chat, &file.to_string_lossy()).unwrap().unwrap();
        assert_eq!(v["collected"], true);
        assert!(store::records()[chat].switch.is_none(), "nothing scheduled, nothing reset");
        let p = move_preview(chat.into(), "m1".into()).unwrap();
        for want in ["Written by the earlier chat", "Ship it.", "## The person's instruction", "\n\nKeep going.\n"] {
            assert!(p.text.contains(want), "{want}:\n{}", p.text);
        }
        assert_eq!(p.prompt, m.line);
        assert!(!p.text.contains("did not finish"), "the writing turn is not the unfinished one");
        // Over: a late file named for a move is refused, and becomes no handoff.
        assert_eq!(move_cancel(chat.into(), "m1".into()).unwrap().phase, "canceled");
        let late = asked_file(chat, &dir.join("move-late.md").to_string_lossy()).unwrap().unwrap_err();
        assert!(late.contains("is over") && late.contains("nothing switches"), "{late}");
        assert!(store::records()[chat].switch.is_none());
        // Its document was in: a later handoff of the chat's is its own, but a
        // draft named for a move, in any case, is never one.
        assert!(store::records()[chat].ask_closed.is_none(), "collected, so no late answer is waited for");
        assert!(asked_file(chat, &dir.join("handoff.md").to_string_lossy()).is_none(), "an ordinary handoff is the handoff's");
        let e = asked_file(chat, &dir.join("MOVE-late.md").to_string_lossy()).unwrap().unwrap_err();
        assert!(e.contains("is over"), "{e}");
        assert!(store::records()[chat].switch.is_none(), "nothing scheduled");
        crate::agents::teardown::forget(chat);
        let _ = std::fs::remove_dir_all(&dir);
    }
    /// Two calls with a file overlapping in one ask: the first shows its
    /// document, and the second, finding the move shown, takes nothing of it
    /// as it gives up: the preview is still there, as shown.
    #[test]
    fn a_second_document_in_the_same_ask_leaves_the_first_ones_preview() {
        let (chat, new) = ("mvd-old", "mvd-new");
        let mut m = shown(chat, new, false);
        crate::agents::registry::register(
            chat,
            Provider::Claude,
            "C:/p",
            "C:/acc/anthropic/one",
            None,
            true,
            serde_json::json!({ "permissionMode": "acceptEdits" }),
        );
        m.source = Source::Chat;
        m.phase = MovePhase::Asked { at: now_ms() };
        m.asked_conversation = Some("old".into());
        store::change(chat, |r| r.moving = Some(m.clone())).unwrap();
        super::super::set_conversation_for_test(chat, Some("old"));
        super::super::drafts_at_start(chat).unwrap();
        let dir = std::env::temp_dir().join(format!("luna-move-twice-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (one, two) = (dir.join("one.md"), dir.join("two.md"));
        std::fs::write(&one, "# Goal\nThe first.\n").unwrap();
        std::fs::write(&two, "# Goal\nThe second.\n").unwrap();
        // Both read the move while it was asking; the first commits first.
        assert_eq!(collect(chat, &m, &one.to_string_lossy()).unwrap()["collected"], true);
        let e = collect(chat, &m, &two.to_string_lossy()).unwrap_err();
        assert!(e.contains("given up meanwhile"), "{e}");
        let p = move_preview(chat.into(), m.id.clone()).expect("the first one's preview stands");
        assert!(p.text.contains("The first.") && !p.text.contains("The second."), "{}", p.text);
        let kept = store::records()[chat].moving.as_ref().and_then(|x| x.preview.clone()).unwrap();
        let left: Vec<_> = std::fs::read_dir(Path::new(&kept).parent().unwrap())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(&format!("move-{}-", m.id)))
            .collect();
        assert_eq!(left.len(), 1, "the second's own file went with it");
        crate::agents::teardown::forget(chat);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A move's ask answers only once typed, and only in the conversation it
    /// was typed into: before, or after the person's `/clear` or `/new`,
    /// neither its instructions nor its collection are given, and a
    /// conversation changed while the document was built takes nothing.
    #[test]
    fn a_moves_ask_answers_only_once_typed_and_only_where_it_went() {
        let (chat, new) = ("mvq-old", "mvq-new");
        let mut m = shown(chat, new, false);
        crate::agents::registry::register(
            chat,
            Provider::Claude,
            "C:/p",
            "C:/acc/anthropic/one",
            None,
            true,
            serde_json::json!({ "permissionMode": "acceptEdits" }),
        );
        super::super::drafts_at_start(chat).unwrap();
        let dir = std::env::temp_dir().join(format!("luna-move-here-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("move-doc.md");
        std::fs::write(
            &file,
            "# Goal
Ship it.
",
        )
        .unwrap();
        let file = file.to_string_lossy().into_owned();
        super::super::set_conversation_for_test(chat, Some("old"));
        // Not typed yet: nothing open.
        m.source = Source::Chat;
        m.phase = MovePhase::Asking { at: now_ms() };
        store::change(chat, |r| r.moving = Some(m.clone())).unwrap();
        assert!(!ask_here(&m, Some("old")));
        assert!(ask_instructions(chat).unwrap_err().contains("not asked for the move's document"));
        assert!(asked_file(chat, &file).unwrap().unwrap_err().contains("not asked for the move's document"));
        // Typed into "old": a call from "theirs" (the person's /new) is refused.
        m.phase = MovePhase::Asked { at: now_ms() };
        m.asked_conversation = Some("old".into());
        store::change(chat, |r| r.moving = Some(m.clone())).unwrap();
        assert!(ask_here(&m, Some("old")) && !ask_here(&m, Some("theirs")) && !ask_here(&m, None));
        super::super::set_conversation_for_test(chat, Some("theirs"));
        assert!(ask_instructions(chat).unwrap_err().contains("not asked"));
        assert!(asked_file(chat, &file).unwrap().unwrap_err().contains("not asked"));
        assert!(matches!(store::records()[chat].moving.as_ref().unwrap().phase, MovePhase::Asked { .. }), "still asked");
        // Called where it was typed, the chat goes on elsewhere before the
        // document is taken: the commit refuses it.
        super::super::set_conversation_for_test(chat, Some("old"));
        assert!(ask_instructions(chat).unwrap().is_some());
        let looked = store::records()[chat].moving.clone().unwrap();
        super::super::set_conversation_for_test(chat, Some("theirs"));
        assert!(collect(chat, &looked, &file).is_err());
        assert!(matches!(store::records()[chat].moving.as_ref().unwrap().phase, MovePhase::Asked { .. }), "nothing shown");
        super::super::set_conversation_for_test(chat, Some("old"));
        assert_eq!(asked_file(chat, &file).unwrap().unwrap()["collected"], true);
        super::super::set_conversation_for_test(chat, None);
        crate::agents::teardown::forget(chat);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The move's ask goes to a person's chat with Luna's tools, threshold or
    /// none, whose CLI has listed the tool; each refusal says what stands in
    /// the way and how to clear it. The person's own ask for a handoff is
    /// none of them: the move takes its place.
    #[test]
    fn a_move_asks_a_chat_with_the_tool_listed_whatever_the_person_asked() {
        use crate::agents::registry::register;
        let acc = "C:/acc/anthropic/one";
        register("mvb-plain", Provider::Claude, "C:/p", acc, None, false, serde_json::json!({}));
        register("mvb-me", Provider::Claude, "C:/p", acc, None, true, serde_json::json!({}));
        let blocked = |chat: &str| ask_blocked(chat, &serde_json::json!({}), Provider::Claude).unwrap_or_default();
        let e = blocked("mvb-plain");
        assert!(e.contains("without Luna's tools") && e.contains("Luna tools box"), "{e}");
        let e = blocked("mvb-me");
        assert!(e.contains("started before Luna offered it") && e.contains("resume it"), "{e}");
        super::super::listed("mvb-me");
        // Past the tool: what is left is that no CLI runs here.
        assert!(blocked("mvb-me").contains("its CLI is not running"), "{}", blocked("mvb-me"));
        // The person's ask for a handoff is no bar: the move takes its place (`taken`).
        store::change("mvb-me", |r| r.asked = Some(store::Asked::new(store::AskKind::Manual, 1, None))).unwrap();
        assert!(blocked("mvb-me").contains("its CLI is not running"), "{}", blocked("mvb-me"));
        let _ = store::remove("mvb-me");
        for e in [blocked("mvb-plain"), blocked("mvb-me")] {
            assert!(!e.contains("let Luna build"), "{e}");
        }
        crate::agents::teardown::forget("mvb-plain");
        crate::agents::teardown::forget("mvb-me");
    }

    #[test]
    fn a_late_answer_is_the_asks_own_turns_and_its_drafts_name() {
        let closed = Some((1_000, Some(4)));
        let in_turn = |started| Turn { started, ended: Some(4) };
        assert!(late_answer("handoff.md", closed, 2_000, in_turn(Some(500))), "the turn the ask ended in");
        assert!(!late_answer("handoff.md", closed, 2_000, in_turn(Some(1_500))), "a turn begun after it, from its start");
        assert!(late_answer("handoff.md", closed, 2_000, in_turn(None)), "no word of the turn: as many ended as then");
        assert!(!late_answer("handoff.md", closed, 2_000, Turn { started: None, ended: Some(5) }));
        assert!(!late_answer("handoff.md", closed, 1_000 + ASK_LATE_MS, in_turn(Some(500))), "past the window");
        assert!(!late_answer("handoff.md", None, 2_000, in_turn(Some(500))), "no ask ended");
        assert!(late_answer("Move-3fa9c2-ab12.md", None, 2_000, Turn::default()), "a move's draft, whenever");
    }

    #[test]
    fn a_chat_stopping_for_a_move_takes_no_spawn_and_counts_those_under_way() {
        use crate::agents::spawn::{SpawnParams, SpawnUnderWay};
        let chat = "mvs-old";
        let params: SpawnParams = serde_json::from_value(serde_json::json!({ "prompt": "go" })).unwrap();
        // Taken before the mark: seen by the look, which then holds the stop.
        let under_way = SpawnUnderWay::begin(chat, &params, "C:\\acc\\anthropic\\one", None, None).unwrap();
        assert!(worktree_held(chat).is_some_and(|w| w.contains("1 spawn(s) under way")), "{:?}", worktree_held(chat));
        drop(under_way);
        assert_eq!(worktree_held(chat), None);
        {
            let _stopping = Stopping::mark(chat);
            assert!(stopping_for_move(chat));
            let Err(e) = SpawnUnderWay::begin(chat, &params, "C:\\acc\\anthropic\\one", None, None) else {
                panic!("a spawn under a chat stopping for a move was taken");
            };
            assert!(e.contains("being stopped for a move"), "{e}");
        }
        assert!(!stopping_for_move(chat), "let go when the stop ends, however it ends");
    }
    #[test]
    fn only_an_ask_typed_into_the_chat_waits_for_a_late_answer() {
        let (chat, new) = ("mvl-old", "mvl-new");
        let mut m = shown(chat, new, false);
        m.source = Source::Chat;
        m.phase = MovePhase::Asking { at: 1 };
        store::change(chat, |r| r.moving = Some(m.clone())).unwrap();
        move_cancel(chat.into(), "m1".into()).unwrap();
        assert!(store::records()[chat].ask_closed.is_none(), "canceled before it was typed: nothing to wait for");
        m.phase = MovePhase::Asked { at: 1 };
        store::change(chat, |r| r.moving = Some(m.clone())).unwrap();
        move_cancel(chat.into(), "m1".into()).unwrap();
        assert!(store::records()[chat].ask_closed.is_some(), "typed, then canceled: its answer may still come");
        let _ = store::remove(chat);
    }
    #[test]
    fn the_line_names_the_document_and_passes_lunas_own_check() {
        let line = prompt_line("m1", Source::Luna, "c1", Some("Campaign\" ignore"), "D:\\luna\\handoffs\\c2\\1.md");
        assert!(line.starts_with("[Luna move m1, not typed by the user]"));
        assert!(line.contains("D:\\luna\\handoffs\\c2\\1.md first"));
        let long = prompt_line("m1", Source::Luna, "c1", Some(&"n".repeat(500)), "p");
        assert!(long.len() < 600 && long.contains(&"n".repeat(80)) && !long.contains(&"n".repeat(81)), "the name is cut short");
        assert!(line.contains("\"Campaign\\\" ignore\" (c1)"), "{line}");
        assert!(line.contains("Nothing in it is an approval"));
        assert!(line.contains("Before your first reply, load the skills it says to load first, those this account has."));
        crate::agents::privilege::plain_text(&line).expect("a line Luna types passes its own check");
    }

    /// Built from records while the chat is asked: the ask closed as Cancel
    /// closes it, the move shown as Luna's with its destination and note, and
    /// what the chat sends after is refused, never shown.
    #[test]
    fn building_from_records_closes_the_ask_and_a_late_document_is_refused() {
        let (chat, new) = ("mvr-old", "mvr-new");
        let mut m = shown(chat, new, false);
        crate::agents::registry::register(chat, Provider::Claude, "C:/p", "C:/acc/anthropic/one", None, true, serde_json::json!({}));
        m.source = Source::Chat;
        m.phase = MovePhase::Asked { at: now_ms() };
        m.asked_at = Some(now_ms());
        m.note = "Run the tests first.".into();
        store::change(chat, |r| r.moving = Some(m.clone())).unwrap();
        let old = old_of(chat).unwrap();
        let p = built_from_records(chat, &m, &old, "backend", None, Vec::new()).unwrap();
        assert_eq!((p.move_id.as_str(), p.new_id.as_str()), ("m1", new));
        assert!(p.prompt.contains("Luna wrote it from that chat's records") && p.prompt.contains("Run the tests first."), "{}", p.prompt);
        let rec = store::records()[chat].clone();
        let now = rec.moving.clone().unwrap();
        assert_eq!((now.phase.word(), now.source, now.to_account.as_str()), ("shown", Source::Luna, "backend"));
        assert_eq!(now.shown_sha256, p.sha256);
        assert!(rec.ask_closed.as_ref().is_some_and(|(id, _)| id == "m1"), "the ask is closed");
        // The chat's document, late: refused, whatever its name, and the
        // preview stays Luna's.
        let dir = std::env::temp_dir().join(format!("luna-move-records-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("move-m1-ab12.md");
        std::fs::write(&file, "# Goal\nThe chat's own.\n").unwrap();
        let e = asked_file(chat, &file.to_string_lossy()).unwrap().unwrap_err();
        assert!(e.contains("is over") && e.contains("nothing switches"), "{e}");
        let e = asked_file(chat, &dir.join("handoff.md").to_string_lossy()).unwrap().unwrap_err();
        assert!(e.contains("is over"), "in the ask's turn any file is its late answer: {e}");
        // Its call for how to write it hears the move is over, not that
        // nobody asked it for a handoff; past the late window, it is unasked.
        let e = super::super::served::instructions(chat).unwrap_err();
        assert!(e.starts_with("the move that asked this chat for its document is over") && !e.contains("Hand off now"), "{e}");
        let closed = store::records()[chat].ask_closed.clone();
        store::change(chat, |r| r.ask_closed = Some(("m1".into(), now_ms() - ASK_LATE_MS - 1))).unwrap();
        assert_eq!(super::super::served::instructions(chat).unwrap_err(), super::super::NOT_ASKED);
        store::change(chat, |r| r.ask_closed = closed).unwrap();
        assert_eq!(store::records()[chat].moving.as_ref().map(|m| m.shown_sha256.clone()), Some(p.sha256.clone()));
        // Shown once: not to be built again over it.
        let again = store::records()[chat].moving.clone().unwrap();
        assert!(!again.asking() && !again.ask_failed());
        // A move whose ask failed is rebuilt the same way, and a restart's
        // failure, never closed, is closed now.
        let mut failed = m.clone();
        failed.phase = MovePhase::Failed { at: 1, why: "Luna restarted while the chat was asked for its document".into() };
        store::change(chat, |r| {
            r.moving = Some(failed.clone());
            r.ask_closed = None;
        })
        .unwrap();
        assert!(failed.ask_failed());
        assert_eq!(built_from_records(chat, &failed, &old, "backend", None, Vec::new()).unwrap().move_id, "m1");
        assert!(store::records()[chat].ask_closed.is_some());
        crate::agents::teardown::forget(chat);
        let _ = store::remove(chat);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_ask_line_is_told_from_every_other_line_and_its_turn_from_the_chats_own() {
        assert!(is_ask_line(&ask_line("3fa9c2")));
        assert!(is_ask_line(&format!("  {}", ask_line("m1"))));
        assert!(!is_ask_line(&prompt_line("m1", Source::Luna, "c1", None, "p")), "the moved chat's line is no ask");
        assert!(!is_ask_line("Please write the request, not typed by the user] part"));
        // As Claude Code 2.1.319 keeps a pasted prompt in its transcript.
        let wrapped = format!("<pasted_content id=\"p1\">{}</pasted_content id=\"p1\">", ask_line("m1"));
        assert!(is_ask_line(&wrapped) && is_ask_line(&format!(" <pasted_content id=\"p2\">\n{}", ask_line("m1"))));
        assert!(!is_ask_line(&format!("<pasted_content id=\"p3\">see {}", ask_line("m1"))), "quoted, not opening");
        assert!(!is_ask_line("<pasted_content id=\"p4\""), "an unclosed tag opens nothing");
        let at = |started| Turn { started, ended: Some(4) };
        let mut m = a_move(MovePhase::Asked { at: 1_000 });
        assert!(!in_ask_turn(&m, at(Some(2_000)), None), "never typed: no turn is the ask's");
        m.asked_at = Some(1_000);
        assert!(in_ask_turn(&m, at(Some(1_000)), None) && in_ask_turn(&m, at(None), None));
        assert!(!in_ask_turn(&m, at(Some(999)), None), "begun before the ask: the chat's own");
        m.phase = MovePhase::Failed { at: 5_000, why: "x".into() };
        assert!(!in_ask_turn(&m, at(None), Some(5_000)), "the ask over and no start known: the chat's own");
    }

    /// The ask timed out and the person went on with real work: that turn,
    /// under way as they pick Luna's document, is unfinished work.
    #[test]
    fn a_turn_begun_after_the_ask_timed_out_is_the_chats_own() {
        let mut m = a_move(MovePhase::Failed { at: 11_000, why: "the chat wrote no document within 10 minutes".into() });
        (m.asked_at, m.asked_turns) = (Some(1_000), Some(4));
        let closed = Some(11_000);
        assert!(in_ask_turn(&m, Turn { started: Some(1_000), ended: Some(4) }, closed), "the ask's own turn, running past the timeout");
        assert!(!in_ask_turn(&m, Turn { started: Some(12_000), ended: Some(4) }, closed), "begun after the ask closed");
        assert!(!in_ask_turn(&m, Turn { started: Some(5_000), ended: Some(5) }, closed), "a turn has ended since the ask's began");
        assert!(!in_ask_turn(&m, Turn { started: Some(5_000), ended: Some(5) }, None), "the ask still open, its turn over");
        // An ask typed before turns were counted goes by time alone.
        m.asked_turns = None;
        assert!(in_ask_turn(&m, Turn { started: Some(5_000), ended: Some(5) }, closed));
    }

    /// An ask's timeout looked at before the person had Luna's document
    /// built: it fails nothing once the move is shown, or confirmed.
    #[test]
    fn an_asks_failure_takes_only_a_move_still_asking() {
        let (chat, new) = ("mvf-old", "mvf-new");
        let mut m = shown(chat, new, false);
        m.source = Source::Chat;
        m.phase = MovePhase::Asked { at: 1 };
        m.asked_at = Some(1);
        let seen = m.clone();
        for now in [MovePhase::Shown { at: 2 }, MovePhase::Confirmed { at: 3 }] {
            let mut x = m.clone();
            x.phase = now.clone();
            store::change(chat, |r| r.moving = Some(x.clone())).unwrap();
            ask_ended(chat, &seen, AskEnd::Timeout, "the chat wrote no document within 10 minutes".into());
            let after = store::records()[chat].moving.clone().unwrap();
            assert_eq!((after.phase, after.ask_end), (now, None), "a move gone on is left as it is");
        }
        // Still asked: failed, with how its ask ended, in one change.
        store::change(chat, |r| r.moving = Some(m.clone())).unwrap();
        ask_ended(chat, &seen, AskEnd::Timeout, "the chat wrote no document within 10 minutes".into());
        let after = store::records()[chat].moving.clone().unwrap();
        assert!(matches!(after.phase, MovePhase::Failed { .. }) && after.ask_end == Some(AskEnd::Timeout), "{after:?}");
        assert!(store::records()[chat].ask_closed.as_ref().is_some_and(|(id, _)| id == "m1"));
        let _ = store::remove(chat);
    }

    #[test]
    fn asking_again_makes_sense_after_a_timeout_not_after_a_limit_or_an_exit() {
        let mut m = a_move(MovePhase::Failed { at: 1, why: "the chat wrote no document within 10 minutes".into() });
        m.source = Source::Chat;
        m.preview = None;
        m.binding = None;
        assert!(m.ask_failed());
        for (end, blocked, again) in [
            (Some(AskEnd::Timeout), None, true),
            (None, None, true),
            (Some(AskEnd::Timeout), Some("the chat is not to be asked now: its CLI is not running"), false),
            (Some(AskEnd::Limit), None, false),
            (Some(AskEnd::Exited), None, false),
            (Some(AskEnd::Left), None, true),
        ] {
            m.ask_end = end;
            assert_eq!(may_ask_again(&m, blocked), again, "{end:?} {blocked:?}");
        }
        // Not an ask that failed: Luna's own document, or one shown.
        m.ask_end = None;
        m.source = Source::Luna;
        assert!(!m.ask_failed() && !may_ask_again(&m, None));
        m.source = Source::Chat;
        m.preview = Some("p".into());
        assert!(!m.ask_failed());
        // Kept on the window's status after the dialog closed.
        m.preview = None;
        let s = status_of("mvx-nobody", &m);
        assert!(s.ask_failed && !s.ask_again && s.why.as_deref().is_some_and(|w| w.contains("no document")), "{s:?}");
    }

    /// A typed ask whose chat went on in another conversation (the person's
    /// `/clear`) fails at once, saying so, and may be asked again; one not
    /// typed yet, or where either conversation is not known, waits.
    #[test]
    fn an_ask_its_chat_left_fails_saying_so_and_may_be_asked_again() {
        let chat = "mvleft-old";
        let mut m = a_move(MovePhase::Asked { at: now_ms() });
        m.source = Source::Chat;
        m.preview = None;
        m.binding = None;
        m.asked_conversation = Some("old".into());
        assert!(left_asked(&m, Some("theirs")));
        assert!(!left_asked(&m, Some("old")) && !left_asked(&m, None), "still there, or not known");
        let asking = Move { phase: MovePhase::Asking { at: 1 }, ..m.clone() };
        assert!(!left_asked(&asking, Some("theirs")), "not typed: asks nothing yet");
        store::change(chat, |r| r.moving = Some(m.clone())).unwrap();
        ask_ended(chat, &m, AskEnd::Left, LEFT_ASKED.into());
        let after = store::records()[chat].moving.clone().unwrap();
        assert!(matches!(&after.phase, MovePhase::Failed { why, .. } if why == LEFT_ASKED), "{after:?}");
        assert!(may_ask_again(&after, None), "asked again in the conversation it is on");
        assert!(store::records()[chat].ask_closed.as_ref().is_some_and(|(id, _)| *id == m.id), "a late answer is refused");
        store::remove(chat).unwrap();
    }

    /// A move, by its ask or from records, takes the chat from the
    /// threshold's ask, which closes, the threshold re-armed only as the move
    /// ends, and from the person's, which is withdrawn for them to be told;
    /// never from a handoff scheduled or one whose file is being copied.
    #[test]
    fn a_move_takes_the_place_of_an_ask_never_of_a_handoff() {
        let chat = "mvtake-old";
        let told = store::Told { conversation: "c".into(), level: 1, floor: 0, shares: vec![72], compacted: 0, tokens: 1 };
        for m in [a_move(MovePhase::Asking { at: 1 }), a_move(MovePhase::Shown { at: 1 })] {
            let word = m.phase.word();
            let auto = store::Asked::new(store::AskKind::Auto, 1, None);
            let mut r = store::Record { told: Some(told.clone()), asked: Some(auto.clone()), ..Default::default() };
            assert_eq!(taken(chat, &mut r, &m), Ok(false), "{word}");
            assert_eq!((r.asked.as_ref(), r.told.as_ref().map(|t| t.level)), (None, Some(1)), "{word}: closed, not re-armed yet");
            assert_eq!(r.moving.as_ref().map(|x| x.phase.clone()), Some(m.phase.clone()));
            // The person's ask, typed or not: withdrawn, and said so.
            let typed =
                store::Asked { typed: Some(2), conversation: Some("c".into()), ..store::Asked::new(store::AskKind::Manual, 1, None) };
            for manual in [store::Asked::new(store::AskKind::Manual, 1, None), typed] {
                let mut r = store::Record { told: Some(told.clone()), asked: Some(manual), ..Default::default() };
                assert_eq!(taken(chat, &mut r, &m), Ok(true), "{word}: the move is the person's later word");
                assert!(r.asked.is_none() && r.moving.is_some(), "{word}");
            }
            // A handoff scheduled answers the ask, and would reset the chat
            // mid-move; one being copied would race it for the chat.
            let manual = store::Asked::new(store::AskKind::Manual, 1, None);
            let switched = store::Record {
                asked: Some(manual),
                switch: Some(super::super::phase::tests::switch(Provider::Claude)),
                ..Default::default()
            };
            let copying = store::Record { asked: Some(auto), reserved: Some(3), ..Default::default() };
            for mut r in [switched, copying] {
                let before = r.asked.clone();
                assert_eq!(taken(chat, &mut r, &m), Err(HANDOFF_UNDER_WAY), "{word}");
                assert!(r.moving.is_none() && r.asked == before, "{word}: nothing changed");
            }
        }
    }

    /// A move's Send is refused while a handoff of its chat is scheduled:
    /// its reset would come in the middle of the move.
    #[test]
    fn a_move_is_not_sent_while_a_handoff_of_its_chat_is_scheduled() {
        let (chat, new) = ("mvsw-old", "mvsw-new");
        let m = shown(chat, new, false);
        store::change(chat, |r| r.switch = Some(super::super::phase::tests::switch(Provider::Claude))).unwrap();
        let roots = scrub::Roots::of(&[("C:\\Luna", scrub::LUNA_FOLDER)]);
        let text = "# Moved chat\nGo on.\n";
        let e = confirm_with(chat, "m1", text, &sha256_hex(text.as_bytes()), &m.launch.clone().unwrap(), &roots, &|_| Ok(())).unwrap_err();
        assert_eq!(e, HANDOFF_UNDER_WAY);
        assert!(matches!(store::records()[chat].moving.as_ref().map(|x| &x.phase), Some(MovePhase::Shown { .. })), "not confirmed");
        assert!(store::records().get(new).is_none_or(|r| r.moved_from.is_none()), "the new chat's side undone");
        assert!(!super::super::dir_of(new).unwrap().join("1.md").exists());
        for c in [chat, new] {
            let _ = store::remove(c);
        }
        let _ = std::fs::remove_dir_all(super::super::dir_of(new).unwrap());
    }

    /// The threshold asks nothing while a move is under way, its preview
    /// under review and its steps after confirmation included, and is re-armed
    /// as a move ends with the chat not stopped, not after it was.
    #[test]
    fn the_threshold_is_quiet_for_the_whole_move_and_rearmed_where_the_chat_goes_on() {
        let chat = "mvquiet-old";
        let told = store::Told { conversation: "c".into(), level: 1, floor: 0, shares: vec![72], compacted: 0, tokens: 1 };
        let open = [
            MovePhase::Asking { at: 1 },
            MovePhase::Asked { at: 1 },
            MovePhase::Shown { at: 1 },
            MovePhase::Confirmed { at: 1 },
            MovePhase::OldStopped { at: 1 },
            MovePhase::RowCreated { at: 1 },
            MovePhase::Prompting { at: 1 },
        ];
        for phase in open {
            let word = phase.word();
            store::change(chat, |r| r.moving = Some(a_move(phase))).unwrap();
            assert!(under_way(chat), "{word}");
            let asked = store::Asked::new(store::AskKind::Auto, 2, None);
            let opened = store::change(chat, |r| super::super::ask_auto(chat, r, 70, told.clone(), false, asked, "n".into())).unwrap();
            assert!(!opened, "no threshold ask while {word}");
        }
        for phase in [MovePhase::Done { at: 1 }, MovePhase::Failed { at: 1, why: "x".into() }, MovePhase::Canceled { at: 1 }] {
            store::change(chat, |r| r.moving = Some(a_move(phase))).unwrap();
            assert!(!under_way(chat));
        }
        let level = || store::records().get(chat).and_then(|r| r.told.as_ref().map(|t| t.level));
        // Canceled at its preview: the chat goes on, asked again if still past.
        store::change(chat, |r| {
            r.told = Some(told.clone());
            r.moving = Some(a_move(MovePhase::Shown { at: 1 }));
        })
        .unwrap();
        end(chat, "m1", MovePhase::Canceled { at: 2 }).unwrap();
        assert_eq!(level(), Some(0), "re-armed");
        // Failed once the old chat was stopped: nothing to ask there.
        let stopping = |phase| Move { stop_old: true, ..a_move(phase) };
        store::change(chat, |r| {
            r.told = Some(told.clone());
            r.moving = Some(stopping(MovePhase::OldStopped { at: 1 }));
        })
        .unwrap();
        end(chat, "m1", MovePhase::Failed { at: 2, why: "x".into() }).unwrap();
        assert_eq!(level(), Some(1), "not re-armed");
        // Ended with the old chat kept running (no stop step): it goes on,
        // and is asked again if still past its threshold.
        for (phase, end_as) in [
            (MovePhase::Prompting { at: 1 }, MovePhase::Done { at: 2 }),
            (MovePhase::RowCreated { at: 1 }, MovePhase::Failed { at: 2, why: "x".into() }),
        ] {
            store::change(chat, |r| {
                r.told = Some(told.clone());
                r.moving = Some(a_move(phase));
            })
            .unwrap();
            end(chat, "m1", end_as).unwrap();
            assert_eq!(level(), Some(0), "re-armed: the old chat was never stopped");
        }
        // Done with the old chat stopped: not.
        store::change(chat, |r| {
            r.told = Some(told.clone());
            r.moving = Some(stopping(MovePhase::Prompting { at: 1 }));
        })
        .unwrap();
        end(chat, "m1", MovePhase::Done { at: 2 }).unwrap();
        assert_eq!(level(), Some(1), "not re-armed: the old chat was stopped");
        let _ = store::remove(chat);
    }

    /// A move begun, from records or by its ask, retires the threshold's ask
    /// open and drops its notice not yet typed, under the record's lock; the
    /// person's ask stays. A notice already put together is not typed while
    /// the move is under way.
    #[test]
    fn a_move_begun_retires_the_thresholds_ask_and_its_notice() {
        let notice = super::super::facts::notice(72, 70, false, true);
        let other = "Your helper h1 answered.";
        for (chat, by_ask) in [("mvret-shown", false), ("mvret-asked", true)] {
            let _ = store::remove(chat);
            // Auto-handoff on: a threshold notice goes in only while it is.
            let settings = serde_json::json!({ "handoff": 70 });
            crate::agents::registry::register(chat, Provider::Claude, "C:/p", "C:/acc/anthropic/one", None, true, settings);
            let asked = store::Asked { conversation: Some("c".into()), ..store::Asked::new(store::AskKind::Auto, 1, None) };
            store::change(chat, |r| r.asked = Some(asked.clone())).unwrap();
            reg().lock().unwrap().news.push((chat.to_string(), notice.clone()));
            reg().lock().unwrap().news.push((chat.to_string(), other.to_string()));
            assert!(!super::super::withheld(chat, &notice), "no move yet");
            let m = a_move(if by_ask { MovePhase::Asking { at: 1 } } else { MovePhase::Shown { at: 1 } });
            assert_eq!(store::change(chat, |r| taken(chat, r, &m)).unwrap(), Ok(false));
            let r = store::records().get(chat).cloned().unwrap();
            assert_eq!((r.asked, r.moving.map(|m| m.id)), (None, Some("m1".to_string())), "{chat}");
            let queued: Vec<String> = reg().lock().unwrap().news.iter().filter(|(c, _)| c == chat).map(|(_, l)| l.clone()).collect();
            assert_eq!(queued, [other], "the notice dropped, nothing else");
            assert!(super::super::withheld(chat, &notice), "one already on its way is not typed");
            assert!(!super::super::withheld(chat, other));
            reg().lock().unwrap().news.retain(|(c, _)| c != chat);
            let _ = store::remove(chat);
            crate::agents::teardown::forget(chat);
        }
        // The person's ask, its request not typed yet: withdrawn with it,
        // and nothing of it typed while the move is under way.
        let chat = "mvret-manual";
        let _ = store::remove(chat);
        let request = super::super::facts::manual_notice();
        store::change(chat, |r| r.asked = Some(store::Asked::new(store::AskKind::Manual, 1, None))).unwrap();
        reg().lock().unwrap().news.push((chat.to_string(), request.clone()));
        let withdrawn = store::change(chat, |r| taken(chat, r, &a_move(MovePhase::Shown { at: 1 }))).unwrap();
        assert_eq!(withdrawn, Ok(true));
        assert!(store::records()[chat].asked.is_none());
        assert!(!reg().lock().unwrap().news.iter().any(|(c, _)| c == chat), "its request dropped");
        assert!(super::super::withheld(chat, &request), "nor typed if already on its way");
        let _ = store::remove(chat);
    }
}
