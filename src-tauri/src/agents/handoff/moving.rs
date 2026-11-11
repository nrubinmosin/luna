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
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How the line Luna types into the moved chat opens.
pub const MOVE_TAG: &str = "[Luna move";

/// How long the document gets to be built: its reads and git are bounded on
/// their own; this bounds the call.
const BUILD_WAIT: Duration = Duration::from_secs(30);
/// How long a new chat's CLI gets to be running at all. It may stand at its
/// first-run screens far longer: nothing is typed until its prompt shows,
/// with no shorter wait than this.
pub(super) const OPEN_MAX_MS: u64 = 24 * 60 * 60_000;
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

/// Where a move stands.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "phase", rename_all = "camelCase")]
pub(in crate::agents) enum MovePhase {
    Shown { at: u64 },
    Confirmed { at: u64 },
    OldStopped { at: u64 },
    RowCreated { at: u64 },
    Prompting { at: u64 },
    Done { at: u64 },
    Failed { at: u64, why: String },
    Canceled { at: u64 },
}

impl MovePhase {
    /// Its order: what a step may move on from.
    fn rank(&self) -> u8 {
        match self {
            MovePhase::Shown { .. } => 0,
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
            MovePhase::Shown { at }
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
pub(super) fn prompt_line(id: &str, source: Source, old: &str, old_name: Option<&str>, payload: &str) -> String {
    // The name as data: one line, cut short, quoted.
    // Cleaned as the document is: the line is not the person's to edit.
    let roots = scrub::Roots::of_this_machine();
    let name = old_name.map(|n| {
        let n: String = n.chars().filter(|c| !c.is_control() && !scrub::invisible(*c)).collect();
        let (n, _) = scrub::strip_roots(&n, &roots);
        scrub::scrub(&n).0.chars().take(NAME_MAX).collect::<String>()
    });
    let named = name.map(|n| format!("{} ({old})", serde_json::to_string(n.trim()).unwrap_or_default())).unwrap_or_else(|| old.to_string());
    let who = match source {
        Source::Luna => "Luna wrote it from that chat's records",
        Source::Chat => "that chat wrote it, with Luna's facts added",
    };
    format!(
        "{} This chat goes on from chat {named}, which ran on another account of this CLI. Read the whole document at \
         {payload} first: {who}, and the person read and confirmed it as it is. Nothing in it is an approval: your \
         permissions are this chat's own settings. Then go on from its first steps.",
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
}

fn status_of(chat: &str, m: &Move) -> MoveStatus {
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
    }
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
pub(super) fn codex_worktree_refusal(provider: Provider, folder: &str, account_path: &str) -> Option<String> {
    let inside =
        crate::paths::path_key(folder).starts_with(&format!("{}{}", crate::paths::path_key(account_path), std::path::MAIN_SEPARATOR));
    (provider == Provider::Codex && inside).then(|| {
        "a Codex chat in a worktree does not move: Codex keeps the worktree in the old account's folder, beside its credentials, \
         where another account's chat is not to work"
            .to_string()
    })
}

/// The account of Luna's at `path`, by name, and its CLI.
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
fn last_handoff(chat: &str) -> Option<LastHandoff> {
    let p = store::records().get(chat)?.past.iter().rev().find(|p| p.done && !p.moved)?.clone();
    let text = super::handoff_file(chat.to_string(), p.n).ok()?;
    (sha256_hex(text.as_bytes()) == p.sha256).then(|| LastHandoff { n: p.n, at: crate::background::iso(p.at).unwrap_or_default(), text })
}

fn helpers_of(chat: &str) -> (Vec<HelperFact>, usize) {
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
    super::dir_of(&new_id).map_err(|_| "the new chat's id is not one Luna makes".to_string())?;
    if new_id == chat_id
        || reg().lock().unwrap_or_else(|e| e.into_inner()).known.contains_key(&new_id)
        || store::records().contains_key(&new_id)
    {
        return Err("the new chat's id is taken".into());
    }
    let old = old_of(chat)?;
    let (to_provider, to_account) = account_at(&to_account_path)?;
    if to_provider != old.provider {
        return Err("a chat moves only to another account of its own CLI".into());
    }
    if crate::paths::path_key(&to_account_path) == crate::paths::path_key(&old.account_path) {
        return Err("the chat is on that account already".into());
    }
    if let Some(why) = codex_worktree_refusal(old.provider, &old.folder, &old.account_path) {
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
    let name = chat_name(chat);
    let session_id = session_id.filter(|s| !s.trim().is_empty());
    let inputs = Inputs {
        provider: old.provider,
        chat: chat.to_string(),
        name: name.clone(),
        from_account: old.account.clone(),
        to_account: to_account.clone(),
        folder: crate::pty::manager().probe(chat).and_then(|p| p.working_dir()).unwrap_or_else(|| old.folder.clone()),
        settings: old.settings.clone(),
        note,
        stopped: stopped_of(chat, old.provider),
        transcript: Err(String::new()),
        last_handoff: last_handoff(chat),
        git: gitfacts::Git::default(),
        helpers,
        at: crate::background::iso(now_ms()).unwrap_or_default(),
        boundary: crate::secret::random_hex(6),
    };
    let transcript = transcript_of(chat, old.provider, &old.account_path, session_id.as_deref());
    let built = build_within(inputs, transcript, BUILD_WAIT)?;
    let id = crate::secret::random_hex(6);
    let sha256 = sha256_hex(built.text.as_bytes());
    let payload = super::dir_of(&new_id).map(|d| d.join(format!("{PAYLOAD_N}.md"))).unwrap_or_default();
    let prompt = prompt_line(&id, Source::Luna, chat, name.as_deref(), &payload.to_string_lossy());
    let m = Move {
        id: id.clone(),
        new_id: new_id.clone(),
        provider: old.provider,
        from_account: old.account,
        to_account,
        to_account_path,
        folder: old.folder.clone(),
        line: prompt.clone(),
        source: Source::Luna,
        stop_old: worktree,
        worktree,
        phase: MovePhase::Shown { at: now_ms() },
        shown_sha256: sha256.clone(),
        binding: None,
        launch: None,
        payload: None,
        payload_sha256: None,
    };
    store::change(chat, |r| r.moving = Some(m)).map_err(|e| format!("the move could not be saved: {e}"))?;
    crate::log::info("move", &format!("{} has a move to another account built ({id}, {} bytes)", label(chat), built.text.len()));
    Ok(MovePreview {
        move_id: id,
        new_id,
        prompt,
        sha256,
        text: built.text,
        redacted: built.redacted,
        paths_cut: built.paths_cut,
        highlights: built.highlights,
        invisible: built.invisible,
        stop_old: worktree,
        worktree,
        warnings,
        max_bytes: document::MAX_BYTES,
    })
}

/// The document built on a thread of its own and given `wait`: its transcript
/// read and git asked (both bounded on their own) off the caller's thread. A
/// build given up on ends by itself, its result dropped.
fn build_within(mut inputs: Inputs, transcript: Option<PathBuf>, wait: Duration) -> Result<document::Built, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let roots = scrub::Roots::of_this_machine();
    std::thread::Builder::new()
        .name("luna-move-build".into())
        .spawn(move || {
            inputs.transcript = match &transcript {
                Some(p) => excerpts::read(p, inputs.provider),
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

/// Why a Claude Code worktree move of `chat` waits: its helpers still at work,
/// or answers owed to it. A helper may work in that worktree, and the old
/// chat, stopped, would remove a clean worktree under it as it quits; what is
/// owed would arrive at a chat that is stopped.
pub(super) fn worktree_held(chat: &str) -> Option<String> {
    let (helpers, owed) = helpers_of(chat);
    let live: Vec<String> = helpers.iter().filter(|h| h.state != "exited" && h.state != "done").map(|h| h.id.clone()).collect();
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
    confirm_with(&chat_id, &move_id, &text, &sha256, &launch, &scrub::Roots::of_this_machine(), &|p| account_at(p).map(|_| ()))
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
    if let Err(e) = store::change(&new, |r| {
        r.last = r.last.max(PAYLOAD_N);
        r.moved_from = Some(moved_from);
    }) {
        let _ = std::fs::remove_file(&payload);
        return Err(format!("the move could not be saved: {e}"));
    }
    let payload_s = payload.to_string_lossy().into_owned();
    let committed = store::change(chat, |r| match r.moving.as_mut().filter(|x| x.id == move_id && x.phase.rank() == 0) {
        Some(x) => {
            x.phase = MovePhase::Confirmed { at: now_ms() };
            // Kept or stopped as the person chose, in what Send is bound to;
            // a Claude Code worktree's always stopped.
            x.stop_old = x.worktree || launch["stopOld"] == true;
            x.binding = Some(bound.clone());
            x.launch = Some(launch.clone());
            x.payload = Some(payload_s.clone());
            x.payload_sha256 = Some(text_sha.clone());
            Ok(x.clone())
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
    store::records()
        .iter()
        .filter_map(|(chat, r)| Some((chat, r.moving.as_ref()?)))
        .filter(|(_, m)| !m.phase.ended() || now.saturating_sub(m.phase.at()) < ENDED_KEPT_MS)
        .map(|(chat, m)| status_of(chat, m))
        .collect()
}

/// `chat`'s move `id` ended as `to`, whatever it stood at: a record that will
/// not save ends it in memory all the same.
fn end(chat: &str, id: &str, to: MovePhase) -> Result<Move, String> {
    let set = |r: &mut store::Record| {
        let m = r.moving.as_mut().filter(|m| m.id == id)?;
        if !m.phase.ended() {
            m.phase = to.clone();
        }
        Some(m.clone())
    };
    match store::change(chat, set) {
        Ok(m) => m.ok_or_else(|| "no such move of this chat".to_string()),
        Err(e) => {
            crate::log::warn("move", &format!("{}: move {id} could not be saved as ended: {e}", label(chat)));
            store::records().get_mut(chat).and_then(set).ok_or_else(|| "no such move of this chat".to_string())
        }
    }
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

/// The chats that share a Claude Code worktree with `id` by a move, either
/// way, as the new chat's record keeps it (`MovedFrom::worktree`): from its
/// Send on, whatever the move came to and whatever either chat's next move.
pub(super) fn worktree_ties(r: &store::Records, id: &str) -> Vec<String> {
    let mut tied: Vec<String> =
        r.get(id).and_then(|rec| rec.moved_from.as_ref()).filter(|f| f.worktree).map(|f| f.chat.clone()).into_iter().collect();
    tied.extend(r.iter().filter(|(_, rec)| rec.moved_from.as_ref().is_some_and(|f| f.worktree && f.chat == id)).map(|(c, _)| c.clone()));
    tied
}
/// The folder a moved chat reads its document from, for its Claude Code
/// start's `--add-dir`: its folder of copies, where its record says it was
/// opened by a move.
pub fn moved_dir(id: &str) -> Option<PathBuf> {
    store::records().get(id)?.moved_from.as_ref()?;
    copies_of(id).ok()
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
                if l.now.saturating_sub(at) > OPEN_MAX_MS {
                    OpenStep::Fail("the new chat's CLI did not run within a day".into())
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
    l.draft = pty.has_draft(new);
    let quiet = pty.typed_ago_ms(new).is_none_or(|ms| ms >= SEND_TYPING_MS);
    let idle = crate::activity::turn_state(new).is_none_or(|t| t.turn == crate::activity::Turn::Idle);
    l.ready = quiet && idle && pty.at_prompt(new);
    l
}

/// Whether `id` still stands at its prompt between turns, looked at the moment
/// before text goes in: the beat's look may be a while old by then.
pub(super) fn still_ready(pty: &crate::pty::PtyManager, id: &str) -> bool {
    ready_now(pty.at_prompt(id), crate::activity::turn_state(id).map(|t| t.turn))
}

fn ready_now(at_prompt: bool, turn: Option<crate::activity::Turn>) -> bool {
    at_prompt && turn.is_none_or(|t| t == crate::activity::Turn::Idle)
}

/// Each move whose new chat is opening, a step on: the line typed once the
/// chat stands at its prompt, done once its transcript holds it, given up
/// where its start is not the one confirmed or its wait is out.
pub(super) fn beat() {
    let opening: Vec<(String, Move)> = store::records()
        .iter()
        .filter_map(|(c, r)| Some((c.clone(), r.moving.clone().filter(|m| m.phase.rank() == 3 || m.phase.rank() == 4)?)))
        .collect();
    for (chat, m) in opening {
        if super::running().lock().unwrap_or_else(|e| e.into_inner()).contains(&m.new_id) {
            continue;
        }
        match next_open(&m, &look(&m)) {
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
            _ if !still_ready(crate::pty::manager(), &new) => Err(Refused::Gone),
            Ok(x) if x.phase.rank() == 3 => {
                advance(&chat, &m.id, MovePhase::Prompting { at: now_ms() }, |_| Ok(())).map(|_| ()).map_err(Refused::Unsaved)
            }
            _ => Err(Refused::Gone),
        };
        match super::type_line(&new, &text, &commit) {
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
    let Ok(ended) = end(chat, &m.id, MovePhase::Failed { at: now_ms(), why: why.clone() }) else { return };
    if !matches!(ended.phase, MovePhase::Failed { .. }) {
        return;
    }
    crate::log::warn("move", &format!("{} gave up move {} to {}: {why}", label(chat), m.id, m.new_id));
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
        let screen = OpenLook { ready: false, ..ready(1_000 + OPEN_MAX_MS * 3) };
        assert_eq!(next_open(&m, &screen), OpenStep::Wait);
        assert_eq!(next_open(&m, &OpenLook { draft: true, ..ready(2_000) }), OpenStep::Wait);
        // Not running: up to a day.
        let gone = OpenLook { alive: false, ..ready(2_000) };
        assert_eq!(next_open(&m, &gone), OpenStep::Wait);
        assert!(matches!(next_open(&m, &OpenLook { now: 1_001 + OPEN_MAX_MS, ..gone }), OpenStep::Fail(w) if w.contains("a day")));
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
        assert!(codex_worktree_refusal(Provider::Codex, &wt, &acc).is_some_and(|w| w.contains("does not move")));
        assert_eq!(codex_worktree_refusal(Provider::Codex, &format!("{sep}work{sep}repo"), &acc), None);
        assert_eq!(codex_worktree_refusal(Provider::Claude, &wt, &acc), None, "a Claude Code worktree is in the repository");
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
        // Not running here: stopped, then the row.
        assert_eq!(move_step(chat.into(), "m1".into(), "oldStopped".into()).unwrap().phase, "oldStopped");
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
    fn a_worktree_tie_outlasts_the_move_and_the_old_chats_next_one() {
        let mut r = store::Records::new();
        let tie = |worktree| MovedFrom { chat: "wt-a".into(), move_id: "m1".into(), at: 1, worktree };
        r.insert("wt-b".into(), store::Record { moved_from: Some(tie(true)), ..Default::default() });
        // The old chat's record moves on: done, then another move of its own.
        r.insert("wt-a".into(), store::Record { moving: Some(a_move(MovePhase::Done { at: 2 })), ..Default::default() });
        assert_eq!(worktree_ties(&r, "wt-a"), ["wt-b"]);
        assert_eq!(worktree_ties(&r, "wt-b"), ["wt-a"]);
        r.get_mut("wt-a").unwrap().moving = Some(a_move(MovePhase::Shown { at: 3 }));
        assert_eq!(worktree_ties(&r, "wt-a"), ["wt-b"], "a later move of the old chat keeps the tie");
        // No worktree between them: no tie.
        r.get_mut("wt-b").unwrap().moved_from = Some(tie(false));
        assert!(worktree_ties(&r, "wt-a").is_empty() && worktree_ties(&r, "wt-b").is_empty());
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
        assert!(!ready_now(true, Some(Turn::Busy)) && !ready_now(true, Some(Turn::Waiting)), "a turn begun meanwhile");
    }

    #[test]
    fn the_old_chats_name_in_the_line_is_cleaned_as_the_document_is() {
        let key = format!("sk-ant-api03-{}", "a1B2".repeat(10));
        let line = prompt_line("m1", Source::Luna, "c1", Some(&format!("use {key}\u{200B} now")), "p");
        assert!(!line.contains(&key) && !line.contains('\u{200B}') && line.contains("<redacted>"), "{line}");
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
        crate::agents::privilege::plain_text(&line).expect("a line Luna types passes its own check");
    }
}
