//! Sessions run by sessions. A chat started with Luna's MCP server attached
//! can spawn other chats — another model, another account — send them text,
//! read what they answered, wait for them, and kill or delete them, worktree
//! included. This module is the registry behind those tools: who is who,
//! who may touch whom, and the round trips to the UI that owns the chat list.
//!
//! The core does not know chats, only pty sessions (see ARCHITECTURE.md), so
//! a spawn is a request to the frontend — `agent://spawn` out, `agent_spawned`
//! back — and a delete tells the frontend to drop the row after the session
//! and its worktree are gone. Lineage lives here: every session registered
//! with a parent is that parent's, and a caller may only reach its own
//! descendants — except to list every session, type a message into one and
//! read or wait on its answer, which is how sessions talk to each other; only
//! killing and deleting stay with the lineage. Children outlive their parent:
//! they pass to its parent, or with none stand alone, the window's to manage
//! (see `forget`).
//!
//! A helper does not report back on its own the way a subagent does: its
//! answer sits in its own transcript until someone waits for it or reads it.
//! A caller that asked something (spawn, send) and ended its turn without
//! collecting the answer would sit idle beside a helper that finished long
//! ago, so the notifier here types a one-line notice into the caller once it
//! is idle: the helper answered, stopped on a prompt, or exited.

use crate::provider::Provider;
use crate::throttle::now_ms;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Condvar, Mutex, OnceLock};

/// How long the frontend gets to make the chat and start its session.
const SPAWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);
/// The most a caller may spawn under itself, all generations counted.
const MAX_DESCENDANTS: usize = 8;
/// A child of a child may exist; a child of that may not have tools.
const MAX_TOOL_DEPTH: usize = 1;
/// Enter, sent after the text so the TUI sees a paste and then a keypress.
/// Both CLIs treat bytes arriving in a quick burst as one paste, and an Enter
/// inside the burst as a newline in it: at 80 ms Codex 0.157 left the text
/// sitting in its composer. Well past the burst window, then.
const ENTER_GAP: std::time::Duration = std::time::Duration::from_millis(400);
/// If no turn has begun this long after Enter, Enter is pressed once more:
/// on a composer still holding the text it submits; on an empty one it is a
/// no-op in both CLIs.
const ENTER_RETRY: std::time::Duration = std::time::Duration::from_secs(3);
/// How long a fresh session gets to start working on its opening prompt
/// before `spawn` answers without that assurance. Codex brings up its MCP
/// servers first, which can take a while; a session still not busy after
/// this is on a notice, a picker or an error, and the answer carries its
/// screen so the caller can see which — or has not drawn at all yet, and the
/// answer says `starting` instead.
const START_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// The same for text typed into a running session: a turn should begin
/// within a few samples of Enter.
const SEND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(12);
/// The activity sampler runs every 5 s; the probes here look between samples
/// so the answer comes as soon as the verdict is in.
const POLL: std::time::Duration = std::time::Duration::from_millis(500);
/// How long a background spawn in a worktree of its own waits for the CLI
/// to say where it made it.
const WORKTREE_WAIT: std::time::Duration = std::time::Duration::from_secs(20);
/// The rendered screen as an agent gets it: the last lines of the terminal,
/// enough to read a dialog, not a transcript.
const SCREEN_LINES: usize = 40;
/// How often the notifier looks for answers nobody collected.
const NOTIFY_EVERY: std::time::Duration = std::time::Duration::from_secs(2);
/// Looks in a row the caller has to be found idle before a notice goes in:
/// a turn that has just ended may still be followed by the one it queued.
const NOTIFY_IDLE_LOOKS: u32 = 2;
/// A person typed into the caller this recently: hold the notice, it would
/// land in the middle of what they are writing.
const TYPING_QUIET_MS: u64 = 20_000;
/// The same for `send`, whose text is held until then: a person who
/// stopped typing this long ago has likely paused.
const SEND_TYPING_MS: u64 = 5_000;
/// How long a tool call waits for other text Luna is typing into the same
/// terminal: more than a notice takes to go in, a draft put aside and back
/// included, and its turn to start (SEND_TIMEOUT).
const TYPING_WAIT: std::time::Duration = std::time::Duration::from_secs(25);
/// A notice or a send held this long by an unsent draft is announced on the
/// desktop, once: the draft may be forgotten, the text is not.
const DRAFT_ALERT_MS: u64 = 60_000;
/// How long Claude Code gets to show a draft put aside, or back, after the
/// key that does it.
const STASH_WAIT: std::time::Duration = std::time::Duration::from_millis(1500);
/// A turn has ended with the session's processes still at work: give them
/// this long to settle before calling it a job left in the background. Right
/// after a turn they often move for a sample or two anyway.
const BACKGROUND_GRACE_MS: u64 = 20_000;
/// After a caller answers a prompt, how long the prompt it answered may still
/// read as open (the sampler runs every 5 s).
const ANSWERED_MS: u64 = 8_000;
/// Claude Code fires its Stop hook before the turn's last message reaches the
/// transcript: a turn seen over with no reply yet gets this long for one.
const REPLY_GRACE_MS: u64 = 5_000;
/// A `wait` out for an answer hands it over itself, so the notifier holds its
/// notice — this long past the answer at most, for a client that has dropped
/// the call it moved to the background.
const WAIT_HOLD_MS: u64 = 10_000;
/// Text typed into a visible session mid-turn is the CLI's to place: into
/// the turn that runs, or a turn of its own right after it. A turn end
/// counts as the answer to it once no turn has started this long after —
/// more than one activity sample.
const QUEUED_GRACE_MS: u64 = 8_000;
/// How long a Codex caller's turn has to look over before a `wait` it made
/// counts as let go (see `let_go`): more than one activity sample.
const LET_GO_MS: u64 = 8_000;
/// The most of a session's words one tool result carries: Claude Code cuts
/// an MCP result past 25,000 tokens.
const MAX_TEXT: usize = 60_000;
/// An `autoDelete` helper goes once nobody is owed its answer and it has sat
/// idle this long since, nobody reading it either: room for a review of its
/// work and a follow-up first.
const AUTO_DELETE_IDLE_MS: u64 = 30 * 60 * 1000;
/// How long into a core's life an answer stays owed to a caller it has not
/// met. What a helper is owed comes back with the helper, before anything
/// else (`tie_back`), and the chat it is owed to only when the window starts
/// that chat's CLI again, seconds later; but a caller that never comes back
/// must not keep a helper from `reap` for good.
const CALLER_RETURN_MS: u64 = 10 * 60 * 1000;
/// How long into a core's life a chat the last core knew (`restore_chats`) is
/// given to start again before whoever is owed by it hears it has not: the
/// window starts the ones in panes within seconds, the rest when opened.
const SESSION_RETURN_MS: u64 = 60_000;
/// The most turn-end notices one caller gets about one session within
/// BREAKER_WINDOW_MS before Luna stops telling it: each costs a turn of the
/// caller, and two chats that keep answering each other's notices would go on
/// until a person came back. Real use never came near it (at most 3 in half an
/// hour per pair, luna.log 10-28 to 11-01); the loop of that night ran one a
/// minute per pair, for 13 hours.
const BREAKER_NOTICES: usize = 12;
const BREAKER_WINDOW_MS: u64 = 15 * 60 * 1000;
/// The notice turns kept per session (see `NoticeTurn`): the debts that ask
/// about them are never many turns behind.
const NOTICE_TURNS_KEPT: usize = 64;
/// How every notice opens, so a model tells it from what the user types.
pub const NOTICE_TAG: &str = "[Luna notice, not typed by the user]";
/// The most warning lines a spawn passes on from the helper's screen.
const MAX_WARNINGS: usize = 5;
/// How much of a report's first line a notice quotes.
const NOTICE_QUOTE: usize = 150;
/// The percentages of a usage window a caller hears of, once each per window.
const QUOTA_LEVELS: [u8; 2] = [90, 95];
/// A window this far below the level it was told at has reset, whatever its
/// reset time says; a point or two is rounding.
const QUOTA_SLACK: f64 = 5.0;
/// How long a wait on any of several goes on after the first has finished, so
/// that those done at the same moment come too: longer than a wait's look.
const SAME_LOOK: std::time::Duration = std::time::Duration::from_millis(700);
/// What a `wait` that was let go says.
const LET_GO: &str = "this wait was let go before it ended; the answer stays owed and comes as a notice";

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct Message {
    pub role: String,
    pub text: String,
}

/// What a session waits on a person for: a question its model put (Claude
/// Code's `AskUserQuestion`, Codex's `request_user_input`), or leave to run a
/// tool. `luna_answer` answers either.
#[derive(Clone, Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Ask {
    /// A background helper's: which of its held asks this is, for `luna_answer`
    /// to name, so that an answer written on one cannot land on the next. A
    /// terminal shows one dialog at a time and has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// `question` or `permission`.
    pub kind: &'static str,
    /// The turn holds until it is answered. Codex's async question does not
    /// hold it: the model works on beside it.
    pub blocking: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub questions: Vec<AskQuestion>,
    /// A permission's tool, and what it would do with it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Clone, Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AskQuestion {
    pub text: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    /// More than one option may be picked.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub multi: bool,
    /// A list on a Codex screen (`codex_list_on`): the option its cursor is
    /// on. Such a list takes only its own options, no words of one's own.
    #[serde(skip)]
    pub list_at: Option<usize>,
}

impl Ask {
    pub fn question(questions: Vec<AskQuestion>, blocking: bool) -> Ask {
        Ask { id: None, kind: "question", blocking, questions, tool: None, detail: None }
    }

    pub fn permission(tool: Option<String>, detail: Option<String>) -> Ask {
        Ask { id: None, kind: "permission", blocking: true, questions: Vec::new(), tool, detail }
    }

    /// Claude Code's `AskUserQuestion` input: `questions[]` with `question`,
    /// `options[].label` and `multiSelect`.
    pub fn from_claude_questions(input: &serde_json::Value) -> Option<Ask> {
        let questions: Vec<AskQuestion> = input["questions"]
            .as_array()?
            .iter()
            .filter_map(|q| {
                let text = q["question"].as_str()?.trim().to_string();
                let options =
                    q["options"].as_array().map(|o| o.iter().filter_map(|o| o["label"].as_str().map(str::to_owned)).collect()).unwrap_or_default();
                Some(AskQuestion { text, options, multi: q["multiSelect"].as_bool().unwrap_or(false), list_at: None })
            })
            .collect();
        (!questions.is_empty()).then(|| Ask::question(questions, true))
    }

    /// A Claude Code tool call held for permission: the tool, and the one
    /// field of its input that says what it would do.
    pub fn from_claude_tool(name: &str, input: &serde_json::Value) -> Ask {
        let detail = ["command", "file_path", "path", "url", "pattern", "description"]
            .iter()
            .find_map(|k| input[*k].as_str())
            .map(str::to_owned)
            .or_else(|| input.is_object().then(|| input.to_string()))
            .map(|d| d.chars().take(400).collect());
        Ask::permission(Some(name.to_string()), detail)
    }

    /// One line for a notice: the questions with their options numbered.
    pub fn line(&self) -> String {
        if self.kind == "permission" {
            let what = [self.tool.as_deref(), self.detail.as_deref()].into_iter().flatten().collect::<Vec<_>>().join(": ");
            return if what.is_empty() { "asks for permission".into() } else { format!("asks for permission to run {what}") };
        }
        let qs: Vec<String> = self
            .questions
            .iter()
            .map(|q| {
                let opts: Vec<String> = q.options.iter().enumerate().map(|(i, o)| format!("{}. {o}", i + 1)).collect();
                if opts.is_empty() {
                    format!("\"{}\"", q.text)
                } else {
                    format!("\"{}\" ({})", q.text, opts.join(", "))
                }
            })
            .collect();
        format!("asks: {}", qs.join(" "))
    }
}

/// A session the agent layer knows: every one started with tools, and every
/// one an agent spawned.
#[derive(Clone)]
struct Known {
    provider: Provider,
    folder: String,
    account_path: String,
    account: String,
    parent: Option<String>,
    /// What the session was started with, as the CLI's flags spell it:
    /// model, effort and permission mode (Claude) or approval and sandbox
    /// (Codex). Echoed by `list` and `spawn`.
    settings: serde_json::Value,
    /// Luna's tools were given to it: `tools/list` offers them.
    tools: bool,
    /// Known from what the last core saved (`restore_chats`): its CLI has not
    /// started under this one yet.
    waiting: bool,
    /// Spawned by a chat since deleted, with no parent left to hand it to (see
    /// `forget`): no agent reaches it, only the window.
    orphan: bool,
}

/// What an agent asks for. Everything but the prompt may be left to the
/// defaults the frontend would apply to a chat made by hand.
#[derive(Clone, Serialize, Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct SpawnParams {
    pub provider: Option<String>,
    pub account: Option<AccountAsk>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub permission_mode: Option<String>,
    pub approval: Option<String>,
    pub sandbox: Option<String>,
    pub folder: Option<String>,
    pub worktree: Option<bool>,
    pub prompt: String,
    pub name: Option<String>,
    pub tools: Option<bool>,
    /// Luna's to act on (see `reap`); the frontend never sees it.
    #[serde(skip_serializing)]
    pub auto_delete: Option<bool>,
    /// A background helper a core's going stopped mid-turn starts its next
    /// turn by itself under the core after it (`background::resume_stopped`).
    /// Luna's to act on, kept in the settings it runs with.
    #[serde(skip_serializing)]
    pub resume_on_restart: Option<bool>,
    /// A chat the user sees, run by the window. Without it the helper runs
    /// in the background (background.rs) and the frontend never hears of it.
    #[serde(skip_serializing)]
    pub visible: Option<bool>,
    /// Without the edit tools (Claude Code); in a read-only sandbox (Codex).
    pub read_only: Option<bool>,
    /// The agent it runs as: Claude Code's `--agent`.
    pub agent: Option<String>,
}

/// `account` in a spawn: a name, or names to choose from (`pick_from_list`).
/// By the time the frontend hears of it, the one name taken.
#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(untagged)]
pub enum AccountAsk {
    One(String),
    List(Vec<String>),
}

impl SpawnParams {
    /// The account's name, once `spawn` has settled it.
    fn account_name(&self) -> Option<&str> {
        match &self.account {
            Some(AccountAsk::One(name)) => Some(name),
            _ => None,
        }
    }
}

/// The request as the frontend receives it.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpawnRequest {
    request_id: u64,
    parent_id: String,
    /// A worktree the helper is to run in without owning it: its caller's.
    /// The chat is filed under `folder`, the worktree's project.
    in_worktree: Option<String>,
    /// Codex: run the project's hooks unasked, as the folder would start a
    /// chat of the user's (codex/hooks.rs). Not the helper's to choose.
    hooks_unasked: bool,
    #[serde(flatten)]
    params: SpawnParams,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Deleted {
    id: String,
}

/// A visible chat has passed to its deleted parent's parent (`forget`). The
/// window keeps a chat's parent and names it at every spawn, a core started
/// later included, so the row has to hear.
#[derive(Clone, Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Reparented {
    id: String,
    parent: String,
}

#[derive(Default)]
struct Pending {
    done: Option<Result<String, String>>,
}

/// A `luna_spawn` taken on, in `chat-ties.json` until a helper has come of it and
/// its caller is owed by that: a core going before then leaves the caller an error
/// from the call, or nothing at all, and this is what the next core tells it from.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpawnTaken {
    request: u64,
    caller: String,
    /// What was asked, its account and folder settled.
    params: SpawnParams,
    /// What `params` does not write, the window having no word on them.
    auto_delete: Option<bool>,
    resume_on_restart: bool,
    visible: bool,
    in_worktree: Option<String>,
    account_path: String,
    /// With `worktree`: the worktrees there were as it was taken on.
    worktrees_before: Vec<String>,
    /// What the window was asked for, until a session has come of it (`agent_spawns`).
    /// Not saved: it means a request of this core's, and one a core left is settled.
    #[serde(skip)]
    asked: Option<SpawnRequest>,
    /// The session it came to, as soon as there is one (`spawn_is`).
    helper: Option<String>,
    /// That session owes its caller by now, whatever has become of the debt since,
    /// and no core after this is to make it owe again (`take_up_spawns`).
    #[serde(default)]
    owed_placed: bool,
    /// Read from what the last core left: no call waits on it any more.
    #[serde(skip)]
    left_by_core: bool,
    /// Given up on, as far as starting goes: a session the window is still trying
    /// to start for it is refused (`spawn_is`), so what its caller is told stays true.
    #[serde(skip)]
    settled: bool,
    /// What its caller is to be told, once worked out (`settle_spawns`).
    #[serde(skip)]
    news: Option<String>,
    /// Asked for again and not answered yet, so the later request stands for it and
    /// this has only its word to say: not saved, as two on disk would start two.
    #[serde(skip)]
    retried: bool,
}

/// A spawn's record for as long as its call runs, however that ends.
struct SpawnUnderWay(u64);

impl SpawnUnderWay {
    /// On disk before anything is made. `replaces`: the record of the spawn this
    /// asks for again, gone in the same hold, as two would start two after a restart.
    fn begin(caller: &str, params: &SpawnParams, account_path: &str, in_worktree: Option<&str>, replaces: Option<u64>) -> SpawnUnderWay {
        let worktrees_before = match (params.worktree, params.folder.as_deref()) {
            (Some(true), Some(folder)) => crate::worktree::made_for(folder, account_path),
            _ => Vec::new(),
        };
        let taken = SpawnTaken {
            request: 0,
            caller: caller.to_string(),
            params: params.clone(),
            auto_delete: params.auto_delete,
            resume_on_restart: params.resume_on_restart == Some(true),
            visible: params.visible == Some(true),
            in_worktree: in_worktree.map(str::to_owned),
            account_path: account_path.to_string(),
            worktrees_before,
            asked: None,
            helper: None,
            owed_placed: false,
            left_by_core: false,
            settled: false,
            news: None,
            retried: false,
        };
        let request = {
            let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
            let request = r.next_request;
            r.next_request += 1;
            if let Some(old) = replaces {
                r.spawns.remove(&old);
            }
            r.spawns.insert(request, SpawnTaken { request, ..taken });
            request
        };
        crate::background::save();
        SpawnUnderWay(request)
    }
}

impl Drop for SpawnUnderWay {
    fn drop(&mut self) {
        reg().lock().unwrap_or_else(|e| e.into_inner()).spawns.remove(&self.0);
        crate::background::save();
    }
}

/// A visible helper spawned with `resumeOnRestart` that a core's going stopped
/// mid-turn, until the text that carries it on has gone in or will not.
struct ChatResume {
    /// What it was first asked: with no conversation to carry on (the CLI came
    /// back afresh), this is what it gets.
    opening: Option<String>,
    /// Looks in a row that found it up, drawn and between turns.
    looks: u32,
}

/// What a visible helper was first asked, word for word, kept until it has a
/// conversation that holds it: for its CLI should it come back with none, and for
/// its caller should nothing then take it up.
struct Opening {
    text: String,
    /// The turn count that says nothing of it: one under this core past that has a
    /// conversation to go on with. A count kept across a core may be only a turn
    /// that went with it, and 1 or more here is that (`take_up_chats`).
    floor: u32,
    /// Its CLI came back afresh, with no conversation, and nothing has been given
    /// this since: its caller is told (`first_turn_lost`).
    lost: bool,
    /// When it was asked: a conversation that holds this and began since is its own,
    /// should nothing have said so before a core went (`conversation_on_disk`).
    at_ms: u64,
}

struct Registry {
    tokens: HashMap<String, String>,
    known: HashMap<String, Known>,
    pending: HashMap<u64, std::sync::Arc<(Mutex<Pending>, Condvar)>>,
    /// Spawns whose caller stopped waiting (SPAWN_TIMEOUT), or which the last
    /// core left, by request id: the caller to hand the chat to should the
    /// frontend still make it.
    abandoned: HashMap<u64, (String, bool)>,
    next_request: u64,
    /// Spawns taken on and not yet seen through, by request id.
    spawns: HashMap<u64, SpawnTaken>,
    /// Visible helpers that are to go on by themselves, by id.
    resumes: HashMap<String, ChatResume>,
    /// What each visible helper was first asked, by id.
    openings: HashMap<String, Opening>,
    /// What Luna named the worktree of a visible Claude Code helper, by id: the one
    /// it knows to be that helper's, what a fresh start goes back into if it is still
    /// as made (`worktree_for_start`). A background helper's is in its `Spec`.
    worktree_names: HashMap<String, String>,
    /// A Codex helper to make a worktree of its own, by id, until its CLI has said
    /// where: the worktrees there were as it was spawned. Codex takes no name, so one
    /// new since is all that tells what a core may have cut it off making.
    worktrees_before: HashMap<String, Vec<String>>,
    /// What a helper began and is not going on in, by id, until its parent has heard
    /// (`worktrees_left_line`).
    worktrees_left: HashMap<String, Vec<String>>,
    /// Helpers that run in a worktree they were handed rather than made:
    /// deleting one never drops that worktree, whose work is its caller's.
    borrowed: std::collections::HashSet<String>,
    /// The `turnsEnded` each caller was last shown for each session, by
    /// (caller, session). `wait(turn_done)` without `afterTurn` waits for a
    /// turn past that — the one the caller has not seen — rather than past
    /// the count at the moment of the call, which a fast reply had already
    /// moved on: `spawn`, a two-second answer, `wait` → timeout.
    seen: HashMap<(String, String), u32>,
    /// The chat rows' names by chat id, as the frontend last sent them. Every
    /// chat's, not only the known ones: a name may arrive before its session
    /// registers.
    names: HashMap<String, String>,
    /// The window group each chat row sits in, 0-based, sent with the names:
    /// sessions the user put side by side in one group are the ones meant to
    /// work things out between them.
    groups: HashMap<String, u32>,
    /// When each caller last typed into a session that was stopped on a
    /// prompt, by (caller, session): the answer to the prompt. For a few
    /// seconds after it the prompt may still look open, and `wait` must not
    /// hand it back as if it were a new one.
    answered: HashMap<(String, String), u64>,
    /// Answers owed, by (caller, session): the caller spawned the session or
    /// typed into it, and has not been shown a turn past `baseline` since.
    /// The notifier tells the caller when the answer comes.
    owed: HashMap<(String, String), Owed>,
    /// `wait` calls in flight, by (caller, session).
    waits: HashMap<(String, String), u32>,
    /// Sessions a `wait_many` in flight will answer for, by (caller, session):
    /// their notices wait for it, however long the others take.
    reserved: HashMap<(String, String), u32>,
    /// Answers handed over so far, by (caller, session), the latest being the
    /// only one a cancel may take back (see `hand_over`).
    receipts: HashMap<(String, String), u64>,
    /// Helpers spawned with `autoDelete`, by id.
    auto: HashMap<String, Auto>,
    /// Each account's usage as the window last showed it, by `provider/name`.
    usage: HashMap<String, serde_json::Value>,
    /// When each caller last raised a desktop notification (`notify`).
    notified: HashMap<String, u64>,
    /// The tool a Claude Code session's hooks last said is up for an answer
    /// (`AskUserQuestion`, or one held for permission), with its input.
    hook_asks: HashMap<String, (String, serde_json::Value)>,
    /// Text sent to visible sessions where a person was typing or had a
    /// draft Luna could not put aside, in the order it came: the notifier
    /// types each in once the way is clear (see `deliver_held`).
    held: Vec<Held>,
    /// Notices a caller's draft holds back, by caller, for the chat row.
    notices_held: HashMap<String, usize>,
    /// The turns Luna's notices started, by the session they went into.
    notice_turns: HashMap<String, Vec<NoticeTurn>>,
    /// When each turn-end notice went, by (caller, session), within
    /// BREAKER_WINDOW_MS.
    notice_times: HashMap<(String, String), Vec<u64>>,
    /// The pairs Luna has stopped telling (BREAKER_NOTICES), and since when.
    stopped: HashMap<(String, String), u64>,
    /// How each pty session last exited, by id, until it runs again.
    exits: HashMap<String, Option<u32>>,
    /// How far each caller has been told a usage window has gone, by (caller,
    /// `provider/name`, the window's label).
    quota_told: HashMap<(String, String, String), QuotaTold>,
}

/// A turn a Luna notice started. Told a session is not done yet, a caller is
/// not told again of a turn that only answers a notice: two chats with a job
/// each would wake each other for as long as the jobs run.
struct NoticeTurn {
    /// The `turnsEnded` it ends at.
    turn: u32,
    /// Those the notice told of that were the session's own helpers then.
    helpers: Vec<String>,
    /// It told of nobody else: the turn was the session at work on what its
    /// own helpers brought, and nobody's notice came back to it.
    only_helpers: bool,
}

/// The last a caller heard of an account's usage window.
#[derive(Clone, Debug, PartialEq)]
struct QuotaTold {
    level: u8,
    resets_at: Option<String>,
}

/// A `send` waiting for a person to send or clear what they typed.
struct Held {
    caller: String,
    id: String,
    text: String,
    since: u64,
    /// The desktop has been told (DRAFT_ALERT_MS).
    told: bool,
    /// On its way in: found so by the next core, a core went during that.
    going_in: Option<GoingIn>,
}

/// How a core after this one tells whether text being typed arrived: the CLI went
/// with the core, so what is in its conversation is what it took.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoingIn {
    /// The start of it, words only, to look for in what the session was told.
    needle: String,
    /// Where the transcript ended before it went in.
    cursor: u64,
}

/// How much of a text is looked for: enough to tell it, in case its tail changed.
const NEEDLE: usize = 200;

/// Text with its white space as one space each.
fn words(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What to look for of `text` when `typed` is what goes in: itself, or if it went
/// in with more or went to a file (`spill`), what was typed.
fn needle_of(typed: &str, text: &str) -> String {
    let (typed, text) = (words(typed), words(text));
    (if typed.contains(&text) { text } else { typed }).chars().take(NEEDLE).collect()
}

/// An `autoDelete` helper's progress towards going.
#[derive(Default)]
struct Auto {
    /// Since when it has been idle with nobody owed its answer.
    idle_since: Option<u64>,
    /// Luna is deleting it now, so its end goes down as `auto-deleted`.
    deleting: bool,
}

#[derive(Clone, Copy)]
struct Owed {
    /// Turns over when the caller asked; the answer ends a later one.
    baseline: u32,
    /// The caller has been told the session is stopped on a prompt.
    told_waiting: bool,
    /// The question the caller was last told the session asks beside its
    /// work, by hash (0: none).
    told_ask: u64,
    /// The turn count at which the answer was first seen in but held back,
    /// and when: processes still at work (BACKGROUND_GRACE_MS), no reply yet
    /// (REPLY_GRACE_MS), a turn yet to start on queued text (QUEUED_GRACE_MS).
    held: Option<(u32, u64)>,
    /// When the notifier first found the answer in while a `wait` was out
    /// for it (see WAIT_HOLD_MS).
    ready_at: Option<u64>,
    /// Where the session's transcript ended when the caller asked (the first
    /// of its questions still unanswered): what it said from there on is the
    /// answer, however many turns it took.
    cursor: u64,
    /// Asked by text typed into a visible session mid-turn (see
    /// QUEUED_GRACE_MS).
    queued: bool,
    /// Asked by text Luna held (`Held`) and typed in later: the `turnsEnded`
    /// the caller got with `held` is older than the question.
    late: bool,
    /// The session exited with the caller's text still held, never typed in.
    lost: bool,
    /// A core's going stopped its turn and it has gone on by itself
    /// (`resumeOnRestart`); the caller has yet to hear.
    resumed: bool,
    /// A chat's turn asked about went with a core (`restore_chats`): it did
    /// not finish, and the caller hears so.
    cut_short: bool,
    /// Told it is not done under a core since gone: on Windows what it had running
    /// went with that core, and its own helpers' answers come as ever.
    job_went: bool,
    /// Text was going in as a core went, and nothing can tell whether it arrived
    /// (`went_in`): not typed again, and the caller is told.
    unsure: bool,
    /// The caller has been told the session is not done yet. It hears again
    /// when that ends or a turn no notice started does, not of every turn; a
    /// question asked since is a new debt.
    told_pending: bool,
    /// Which debt this is, whatever its numbers: asked again with nothing
    /// moved since, a new one has the same baseline and cursor as the old, and
    /// what was decided about the old is not to touch it.
    rev: u64,
}

impl Owed {
    fn from(baseline: u32, cursor: u64) -> Owed {
        static NEXT_REV: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Owed {
            rev: NEXT_REV.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            baseline,
            told_waiting: false,
            told_ask: 0,
            held: None,
            ready_at: None,
            cursor,
            queued: false,
            late: false,
            lost: false,
            resumed: false,
            cut_short: false,
            job_went: false,
            unsure: false,
            told_pending: false,
        }
    }

    /// What is left of a debt once the caller has heard it is not done yet.
    fn pending(baseline: u32, cursor: u64) -> Owed {
        Owed { told_pending: true, ..Owed::from(baseline, cursor) }
    }
}

fn reg() -> &'static Mutex<Registry> {
    static R: OnceLock<Mutex<Registry>> = OnceLock::new();
    let r = R.get_or_init(|| {
        Mutex::new(Registry {
            tokens: HashMap::new(),
            known: HashMap::new(),
            pending: HashMap::new(),
            abandoned: HashMap::new(),
            borrowed: Default::default(),
            // A page can still answer a request of a core gone since, which is
            // not to land on one of this core's.
            next_request: now_ms(),
            spawns: HashMap::new(),
            resumes: HashMap::new(),
            openings: HashMap::new(),
            worktree_names: HashMap::new(),
            worktrees_before: HashMap::new(),
            worktrees_left: HashMap::new(),
            seen: HashMap::new(),
            names: HashMap::new(),
            groups: HashMap::new(),
            answered: HashMap::new(),
            owed: HashMap::new(),
            waits: HashMap::new(),
            reserved: HashMap::new(),
            receipts: HashMap::new(),
            auto: HashMap::new(),
            usage: HashMap::new(),
            notified: HashMap::new(),
            hook_asks: HashMap::new(),
            held: Vec::new(),
            notices_held: HashMap::new(),
            notice_turns: HashMap::new(),
            notice_times: HashMap::new(),
            stopped: HashMap::new(),
            exits: HashMap::new(),
            quota_told: HashMap::new(),
        })
    });
    // Tests assert with this held and all of them share it: one that fails is
    // that test's failure, not that of every test after it.
    #[cfg(test)]
    r.clear_poison();
    r
}

/// `caller` has been handed the answer `id` gave by `turns_ended`: a turn past
/// what it asked about settles what it was owed. It has seen that count too
/// (see `seen`).
fn shown(caller: &str, id: &str, turns_ended: u32) {
    let key = (caller.to_string(), id.to_string());
    let goes_on = take_debt(&mut reg().lock().unwrap_or_else(|e| e.into_inner()), &key, turns_ended, None);
    owed_on(key, turns_ended, goes_on);
    seen(caller, id, turns_ended);
}

/// Takes what `caller` is owed by `id` if it has been handed the answer: a turn
/// past what it asked about, or, `job_debt` being this very debt, the end of a
/// job it was told of. In one hold of the lock, so that a question asked a
/// moment later is a debt that stands. Returns whether what the session goes on
/// to say is owed on (`owed_on`).
fn take_debt(r: &mut Registry, key: &(String, String), turns_ended: u32, job_debt: Option<u64>) -> bool {
    let Some(o) = r.owed.get(key) else { return false };
    if turns_ended <= o.baseline && job_debt != Some(o.rev) {
        return false;
    }
    r.owed.remove(key);
    waits_on_helpers_in(r, &key.1, &key.0)
}

/// What a session says once its own helpers have answered it is what it was
/// asked for: still owed, past what the caller has now, as a notice's "not
/// done" leaves it. Off the lock, as this reads the transcript's length, and so
/// behind a question asked meanwhile, whose debt covers that too.
fn owed_on(key: (String, String), turns_ended: u32, goes_on: bool) {
    if goes_on {
        let cursor = transcript_end(&key.1);
        reg().lock().unwrap_or_else(|e| e.into_inner()).owed.entry(key).or_insert_with(|| Owed::pending(turns_ended, cursor));
    }
}

/// `shown`, for a tool response the client may yet drop unread (a cancel
/// that crosses it): returns what puts the debt and the count seen back as
/// they were. Whatever a later call has recorded meanwhile stands, and a later
/// call that handed the answer over too has delivered it: this one's response
/// being dropped then owes nothing.
fn hand_over(caller: &str, id: &str, turns_ended: u32, job_debt: Option<u64>) -> impl FnOnce() + Send + 'static {
    let key = (caller.to_string(), id.to_string());
    let (owed, seen_before, receipt, goes_on) = {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let receipt = r.receipts.entry(key.clone()).or_default();
        *receipt += 1;
        let receipt = *receipt;
        let (owed, seen_before) = (r.owed.get(&key).copied(), r.seen.get(&key).copied());
        (owed, seen_before, receipt, take_debt(&mut r, &key, turns_ended, job_debt))
    };
    owed_on(key.clone(), turns_ended, goes_on);
    seen(caller, id, turns_ended);
    // What never reached it goes with this response alone: any later one no
    // longer finds it, so the receipt below says nothing of it.
    let parent = parent_of(id).as_deref() == Some(caller);
    let taken = take_undelivered(id, caller, parent);
    let id = id.to_string();
    move || {
        put_undelivered_back(&id, taken);
        crate::background::save();
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        if r.receipts.get(&key) != Some(&receipt) {
            return;
        }
        if let Some(o) = owed {
            r.owed.entry(key.clone()).and_modify(|now| now.cursor = now.cursor.min(o.cursor)).or_insert(o);
        }
        if r.seen.get(&key) == Some(&turns_ended) {
            match seen_before {
                Some(n) => r.seen.insert(key, n),
                None => r.seen.remove(&key),
            };
        }
    }
}

/// Records the turn count a tool answer showed `caller` of `id`, settling
/// nothing: the default `afterTurn` of its next `wait` once nothing is owed.
fn seen(caller: &str, id: &str, turns_ended: u32) {
    reg().lock().unwrap_or_else(|e| e.into_inner()).seen.insert((caller.to_string(), id.to_string()), turns_ended);
    looked(id);
}

/// Somebody has looked at `id`, so an `autoDelete` helper counts its idle
/// time from now.
fn looked(id: &str) {
    if let Some(a) = reg().lock().unwrap_or_else(|e| e.into_inner()).auto.get_mut(id) {
        a.idle_since = None;
    }
}

/// `caller` has asked `id` something; the answer ends a turn past `baseline`
/// and is what the session says from `cursor` on, where its transcript ended
/// before the question went in (0 for a session just spawned, whose whole
/// transcript answers its opening prompt). Asked again before an earlier
/// answer reached it, the caller is owed that one too: the cursor stays the
/// earlier one. The dev caller has no terminal to be told in.
fn owe_from(caller: &str, id: &str, baseline: u32, cursor: u64) {
    if caller == DEV_CALLER {
        return;
    }
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let key = (caller.to_string(), id.to_string());
    // A new turn answers this one; that the last was cut short has yet to be
    // said all the same.
    let (cursor, resumed) = r.owed.get(&key).map_or((cursor, false), |o| (o.cursor.min(cursor), o.resumed || o.cut_short));
    r.owed.insert(key, Owed { resumed, ..Owed::from(baseline, cursor) });
    drop(r);
    // A spawn's or a question's: a core killed now must not forget either.
    crate::background::save();
}

/// A terminal Luna is typing into, for as long as this is held, whatever it is
/// typing: a notice, a send, a held send, an answer. Text goes in as a paste and
/// an Enter 400 ms later, with a draft put aside first and brought back after,
/// and anything else typed in between lands in it or breaks the draft's return.
/// Held until the turn the text starts has begun, which a notice does off the
/// notifier's loop: one slow caller is not to keep the others waiting.
struct TypingInto(String);

impl TypingInto {
    fn all() -> &'static (Mutex<std::collections::HashSet<String>>, Condvar) {
        static T: OnceLock<(Mutex<std::collections::HashSet<String>>, Condvar)> = OnceLock::new();
        T.get_or_init(Default::default)
    }

    /// The terminal if nothing else is typing into it now.
    fn claim(id: &str) -> Option<TypingInto> {
        Self::all().0.lock().unwrap_or_else(|e| e.into_inner()).insert(id.to_string()).then(|| TypingInto(id.to_string()))
    }

    /// The terminal once what is typing into it is done, for a tool call that
    /// has text to put in now; None if that takes longer than `timeout`.
    fn wait_for(id: &str, timeout: std::time::Duration) -> Option<TypingInto> {
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

    fn under_way(id: &str) -> bool {
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

/// How far the session's transcript goes now, 0 before it has one.
fn transcript_end(id: &str) -> u64 {
    transcript_path(&crate::pty::manager(), id).and_then(|p| std::fs::metadata(p).ok()).map_or(0, |m| m.len())
}

/// A `wait` by `caller` on a session, counted for as long as it runs.
struct WaitOut((String, String));

impl WaitOut {
    fn start(caller: &str, id: &str) -> WaitOut {
        let key = (caller.to_string(), id.to_string());
        *reg().lock().unwrap_or_else(|e| e.into_inner()).waits.entry(key.clone()).or_default() += 1;
        WaitOut(key)
    }
}

impl Drop for WaitOut {
    fn drop(&mut self) {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = r.waits.get_mut(&self.0) {
            *n -= 1;
            if *n == 0 {
                r.waits.remove(&self.0);
            }
        }
    }
}

/// A `wait_many` out on a session, for as long as it runs. Its answer stays owed
/// until the whole call has its response, so that a core going in between
/// leaves it owed in `helpers.json`; this is what keeps the notice from telling
/// it meanwhile.
struct Reserved((String, String));

impl Reserved {
    fn start(caller: &str, id: &str) -> Reserved {
        let key = (caller.to_string(), id.to_string());
        *reg().lock().unwrap_or_else(|e| e.into_inner()).reserved.entry(key.clone()).or_default() += 1;
        Reserved(key)
    }
}

impl Drop for Reserved {
    fn drop(&mut self) {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = r.reserved.get_mut(&self.0) {
            *n -= 1;
            if *n == 0 {
                r.reserved.remove(&self.0);
            }
        }
    }
}

/// The last count `caller` was shown for `id`, if any.
fn last_shown(caller: &str, id: &str) -> Option<u32> {
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    r.seen.get(&(caller.to_string(), id.to_string())).copied()
}

/// Starts the notifier and what types in the sends held for people, once.
pub fn init() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        std::thread::Builder::new().name("luna-notifier".into()).spawn(notifier).expect("spawn notifier thread");
        std::thread::Builder::new().name("luna-held".into()).spawn(held_sender).expect("spawn held sender thread");
    });
}

fn account_name(account_path: &str) -> String {
    std::path::Path::new(account_path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Whose a session is: the core's record (`pass_up`, kept across cores), and only
/// with none the window's word, never for an orphan.
fn parent_by_record(record: Option<&Known>, window_says: Option<&str>) -> Option<String> {
    let orphan = record.is_some_and(|k| k.orphan);
    record.and_then(|k| k.parent.clone()).or_else(|| window_says.filter(|_| !orphan).map(str::to_owned))
}

/// Whom a session the window is starting is to be told it answers to.
pub fn parent_on_start(id: &str, window_says: Option<String>) -> Option<String> {
    parent_by_record(reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(id), window_says.as_deref())
}

/// A session is starting. Returns the bearer token for its MCP config when it
/// gets tools. Called at every spawn, so a resumed session is re-registered
/// with the same lineage and a fresh token.
pub fn register(
    id: &str,
    provider: Provider,
    folder: &str,
    account_path: &str,
    parent: Option<&str>,
    tools: bool,
    settings: serde_json::Value,
) -> Option<String> {
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let account = account_name(account_path);
    let previous = r.known.get(id).cloned();
    r.exits.remove(id);
    // Whose it is since it last ran, if anything has changed that (`forget`):
    // the window still names the parent it was spawned by.
    let orphan = previous.as_ref().is_some_and(|k| k.orphan);
    let window_says = parent;
    let parent = parent_by_record(previous.as_ref(), parent);
    // The window's settings are the flags it starts a CLI with: this one is the
    // core's, kept across every start.
    let mut settings = settings;
    if previous.as_ref().is_some_and(|k| k.settings["resumeOnRestart"] == true) {
        settings["resumeOnRestart"] = serde_json::json!(true);
    }
    // A session running again: what the chats' ties say is news once more.
    CHATS_FROZEN.store(false, std::sync::atomic::Ordering::SeqCst);
    // Kept across cores too (`restore_chats`), so a new parent the window never
    // heard of before its core went is said again, and this stays the one record.
    if let (Some(says), Some(is)) = (window_says, &parent) {
        if says != is {
            crate::emit::to_ui("agent://reparent", Reparented { id: id.to_string(), parent: is.clone() });
        }
    }
    r.known.insert(
        id.to_string(),
        Known {
            provider,
            folder: folder.to_string(),
            account_path: account_path.to_string(),
            account,
            parent,
            settings,
            tools,
            waiting: false,
            orphan,
        },
    );
    // One token per session: a respawn invalidates the last one.
    r.tokens.retain(|_, chat| chat != id);
    if !tools {
        return None;
    }
    let token = mint(id);
    r.tokens.insert(token.clone(), id.to_string());
    Some(token)
}

/// A caller that is not a session: for driving the agent layer from outside
/// — a test against a build, `curl` against the MCP endpoint. Registered only
/// when `LUNA_DEV_TOKEN` is set in the environment, with `LUNA_DEV_FOLDER`
/// as its project folder and `LUNA_DEV_ACCOUNT` as its account directory;
/// spawns it asks for come up as top-level chats, since it has no row of its
/// own. Never set for a Luna a person uses.
pub fn register_dev_caller() {
    let Ok(token) = std::env::var("LUNA_DEV_TOKEN") else { return };
    let folder = std::env::var("LUNA_DEV_FOLDER").unwrap_or_default();
    let account_path = std::env::var("LUNA_DEV_ACCOUNT").unwrap_or_default();
    if token.trim().is_empty() || folder.is_empty() || account_path.is_empty() {
        crate::log::warn("agents", "LUNA_DEV_TOKEN needs LUNA_DEV_FOLDER and LUNA_DEV_ACCOUNT too; dev caller off");
        return;
    }
    let provider = if account_path.replace('\\', "/").contains("/openai/") { Provider::Codex } else { Provider::Claude };
    register(DEV_CALLER, provider, &folder, &account_path, None, false, serde_json::json!({}));
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    r.tokens.insert(token.trim().to_string(), DEV_CALLER.to_string());
    let url = crate::hub::mcp_url().unwrap_or_default();
    crate::log::warn("agents", &format!("dev caller registered for {folder} ({account_path}); MCP at {url}"));
}

/// A pty session's process has gone, with this exit code if it left one.
pub fn session_exited(id: &str, code: Option<u32>) {
    reg().lock().unwrap_or_else(|e| e.into_inner()).exits.insert(id.to_string(), code);
}

/// The id of the dev caller (see `register_dev_caller`).
pub const DEV_CALLER: &str = "dev";

/// The frontend's name for a chat — set when it spawns one, ahead of the next
/// `chat_names`.
pub fn set_name(id: &str, name: &str) {
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    r.names.insert(id.to_string(), name.to_string());
}

/// A chat's name as the user sees it, if the frontend has sent one.
pub fn chat_name(id: &str) -> Option<String> {
    reg().lock().unwrap_or_else(|e| e.into_inner()).names.get(id).cloned().filter(|n| !n.trim().is_empty())
}

/// A session as the log names it: the id with the chat's name, which is what
/// is left to go by once the session is deleted.
pub fn label(id: &str) -> String {
    match chat_name(id) {
        Some(name) => format!("{id} \"{name}\""),
        None => id.to_string(),
    }
}

pub(crate) fn mint(id: &str) -> String {
    use sha2::{Digest, Sha256};
    use std::sync::atomic::{AtomicU64, Ordering};
    // Two spawns in the same millisecond must not share a token.
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    let mut h = Sha256::new();
    h.update(id.as_bytes());
    h.update(now_ms().to_le_bytes());
    h.update(std::process::id().to_le_bytes());
    h.update(SERIAL.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    let slot = 0u8;
    h.update((&slot as *const u8 as usize).to_le_bytes());
    h.finalize().iter().take(20).map(|b| format!("{b:02x}")).collect()
}

/// The session behind a bearer token.
pub fn caller_of(token: &str) -> Option<String> {
    reg().lock().unwrap_or_else(|e| e.into_inner()).tokens.get(token).cloned()
}

/// A token for a session registered without tools: a background Claude Code
/// helper takes its permission prompts through Luna's MCP server all the
/// same, and `tools/list` offers it that one tool only.
fn issue_token(id: &str) -> String {
    let token = mint(id);
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    r.tokens.retain(|_, chat| chat != id);
    r.tokens.insert(token.clone(), id.to_string());
    token
}

/// Whether a session was given Luna's tools (the dev caller always has them).
pub fn has_tools(id: &str) -> bool {
    id == DEV_CALLER || reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(id).is_some_and(|k| k.tools)
}

pub fn parent_of(id: &str) -> Option<String> {
    reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(id)?.parent.clone()
}

pub fn settings_of(id: &str) -> serde_json::Value {
    reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(id).map(|k| k.settings.clone()).unwrap_or_default()
}

/// A background helper has become a chat the window runs: no longer one
/// Luna deletes on its own, and the window registers it again when its
/// session starts.
pub fn promoted(id: &str) {
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    r.auto.remove(id);
    if let Some(k) = r.known.get_mut(id) {
        if let Some(o) = k.settings.as_object_mut() {
            o.remove("background");
        }
    }
}

/// Whether a helper was spawned with `resumeOnRestart`.
pub fn resumes_on_restart(id: &str) -> bool {
    reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(id).is_some_and(|k| k.settings["resumeOnRestart"] == true)
}

/// A helper goes on by itself after a core's going stopped its turn: whoever
/// is owed its answer is owed the end of the turn about to start, past the
/// `turns_ended` counted so far, and has yet to hear of it. The cursor stays:
/// the answer is all it has said since they asked.
pub fn resumed(id: &str, turns_ended: u32) {
    resumed_in(&mut reg().lock().unwrap_or_else(|e| e.into_inner()), id, turns_ended);
}

fn resumed_in(r: &mut Registry, id: &str, turns_ended: u32) {
    for (_, o) in r.owed.iter_mut().filter(|((_, s), _)| s == id) {
        o.baseline = o.baseline.max(turns_ended);
        o.resumed = true;
        o.cut_short = false;
    }
}

/// What the agent layer holds of a background helper, saved with it
/// (background.rs `save`) so a core started later takes it up where this one
/// left it: whose it is, what it is called, and what its callers are owed.
#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Ties {
    pub parent: Option<String>,
    pub name: Option<String>,
    pub settings: serde_json::Value,
    pub tools: bool,
    pub borrowed: bool,
    pub auto_delete: bool,
    /// See `Known::orphan`.
    #[serde(default)]
    pub orphan: bool,
    /// (caller, baseline, cursor) of every answer owed by it (see `Owed`).
    pub owed: Vec<(String, u32, u64)>,
    /// (caller, turns ended) each caller was last shown of it (see `seen`).
    pub seen: Vec<(String, u32)>,
    /// The callers among `owed` whose turn went with a core (`Owed::cut_short`):
    /// the count alone reads as an answer once that turn is counted as ended.
    #[serde(default)]
    pub cut_short: Vec<String>,
    /// See `Opening`: written only until a first turn has ended, or its caller has
    /// heard it was lost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opening: Option<String>,
    /// See `Opening::lost`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub opening_lost: bool,
    /// See `Opening::at_ms`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opening_at: Option<u64>,
    /// See `Registry::worktree_names`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_name: Option<String>,
    /// The callers among `owed` told it is not done (`Owed::told_pending`): the
    /// count alone reads as nothing having happened, and what it had running goes
    /// with the core.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub told_pending: Vec<String>,
    /// See `Registry::worktrees_before`: written only until its CLI has said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktrees_before: Option<Vec<String>>,
    /// See `Registry::worktrees_left`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub worktrees_left: Vec<String>,
}

pub fn ties_of(id: &str) -> Option<Ties> {
    ties_in(&reg().lock().unwrap_or_else(|e| e.into_inner()), id)
}

fn ties_in(r: &Registry, id: &str) -> Option<Ties> {
    let k = r.known.get(id)?;
    Some(Ties {
        parent: k.parent.clone(),
        name: r.names.get(id).cloned(),
        settings: k.settings.clone(),
        tools: k.tools,
        borrowed: r.borrowed.contains(id),
        auto_delete: r.auto.contains_key(id),
        orphan: k.orphan,
        owed: r.owed.iter().filter(|((_, s), _)| s == id).map(|((c, _), o)| (c.clone(), o.baseline, o.cursor)).collect(),
        seen: r.seen.iter().filter(|((_, s), _)| s == id).map(|((c, _), n)| (c.clone(), *n)).collect(),
        cut_short: r.owed.iter().filter(|((_, s), o)| s == id && o.cut_short).map(|((c, _), _)| c.clone()).collect(),
        opening: r.openings.get(id).map(|o| o.text.clone()),
        opening_lost: r.openings.get(id).is_some_and(|o| o.lost),
        opening_at: r.openings.get(id).map(|o| o.at_ms),
        worktree_name: r.worktree_names.get(id).cloned(),
        told_pending: r.owed.iter().filter(|((_, s), o)| s == id && o.told_pending).map(|((c, _), _)| c.clone()).collect(),
        worktrees_before: r.worktrees_before.get(id).cloned(),
        worktrees_left: r.worktrees_left.get(id).cloned().unwrap_or_default(),
    })
}

/// A helper a previous core saved is known again, as it was. Returns the
/// token for its MCP config, a fresh one: the old core's went with it.
pub fn tie_back(id: &str, provider: Provider, folder: &str, account_path: &str, ties: Ties) -> Option<String> {
    let token = register(id, provider, folder, account_path, ties.parent.as_deref(), ties.tools, ties.settings);
    let token = token.or_else(|| (provider == Provider::Claude).then(|| issue_token(id)));
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(name) = ties.name {
        r.names.insert(id.to_string(), name);
    }
    if let Some(k) = r.known.get_mut(id) {
        k.orphan = ties.orphan;
    }
    if ties.borrowed {
        r.borrowed.insert(id.to_string());
    }
    // Its idle time counts afresh: nobody has had the chance to look at it.
    if ties.auto_delete {
        r.auto.insert(id.to_string(), Auto::default());
    }
    for (caller, baseline, cursor) in ties.owed {
        let told = ties.told_pending.contains(&caller);
        let o = if told { Owed { job_went: true, ..Owed::pending(baseline, cursor) } } else { Owed::from(baseline, cursor) };
        r.owed.insert((caller, id.to_string()), o);
    }
    for (caller, n) in ties.seen {
        r.seen.insert((caller, id.to_string()), n);
    }
    if let Some(before) = ties.worktrees_before {
        r.worktrees_before.insert(id.to_string(), before);
    }
    if !ties.worktrees_left.is_empty() {
        r.worktrees_left.insert(id.to_string(), ties.worktrees_left);
    }
    token
}

/// A session the window runs, as `<data>/chat-ties.json` keeps it for the core
/// started after this one, as `helpers.json` does a background helper's: whose
/// it is, what it was handed and what its callers are owed. The window starts
/// it again, and `register` takes it from there.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedChat {
    id: String,
    provider: Provider,
    folder: String,
    account_path: String,
    /// See `activity::turns_counted`.
    turns_ended: u32,
    transcript: Option<std::path::PathBuf>,
    /// Its turn was running as this was written, or was yet to be carried on
    /// (`Registry::resumes`).
    #[serde(default)]
    mid_turn: bool,
    ties: Ties,
}

/// Text Luna told its sender it would type in (`Held`).
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedSend {
    caller: String,
    id: String,
    text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    going_in: Option<GoingIn>,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct SavedChats {
    chats: Vec<SavedChat>,
    sends: Vec<SavedSend>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    spawns: Vec<SpawnTaken>,
}

fn chat_ties_file() -> std::path::PathBuf {
    crate::paths::data_dir().join("chat-ties.json")
}

/// Set once `restore_chats` has read the file: an empty registry before it is
/// not to be written over what the last core left.
static CHATS_READ: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// What the last write says stands, for the core after this one. Set as every
/// session is stopped (a core leaving, an update): a turn asked to quit is a turn
/// ended to the sampler, Codex writing an abort for it that counts as its end, and
/// written down that would have the next core take it for one that finished.
static CHATS_FROZEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn freeze_chats() {
    CHATS_FROZEN.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// What there is to keep of the sessions the window runs. A chat nobody spawned,
/// wrote to or handed anything has nothing to keep: it comes back as it was.
fn chats_now() -> SavedChats {
    let (mut chats, sends, mut spawns, floors) = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let sends: Vec<SavedSend> =
            r.held.iter().map(|h| SavedSend { caller: h.caller.clone(), id: h.id.clone(), text: h.text.clone(), going_in: h.going_in.clone() }).collect();
        let chats: Vec<SavedChat> = r
            .known
            .iter()
            .filter(|(id, k)| {
                k.settings["background"] != true
                    && id.as_str() != DEV_CALLER
                    && (k.parent.is_some()
                        || k.orphan
                        || r.auto.contains_key(*id)
                        || r.borrowed.contains(*id)
                        || r.owed.keys().any(|(_, s)| s == *id)
                        || sends.iter().any(|h| h.id == **id))
            })
            .filter_map(|(id, k)| {
                Some(SavedChat {
                    id: id.clone(),
                    provider: k.provider,
                    folder: k.folder.clone(),
                    account_path: k.account_path.clone(),
                    turns_ended: 0,
                    transcript: None,
                    mid_turn: r.resumes.contains_key(id),
                    ties: ties_in(&r, id)?,
                })
            })
            .collect();
        let floors: HashMap<String, u32> = r.openings.iter().filter(|(_, o)| !o.lost).map(|(id, o)| (id.clone(), o.floor)).collect();
        (chats, sends, r.spawns.values().filter(|s| !s.retried).cloned().collect::<Vec<SpawnTaken>>(), floors)
    };
    let pty = crate::pty::manager();
    for c in &mut chats {
        c.turns_ended = crate::activity::turns_counted(&c.id).unwrap_or(0);
        let first_turn = floors.get(&c.id).filter(|floor| c.turns_ended <= **floor).is_some();
        // Where its conversation is, while it may be its first: a core after this
        // goes by that as to whether it has one to come back to (`first_turn_lost`).
        if first_turn {
            transcript_path(&pty, &c.id);
        }
        c.transcript = transcripts().lock().unwrap_or_else(|e| e.into_inner()).get(&c.id).cloned();
        c.mid_turn |= crate::activity::turn_state(&c.id).is_some_and(|t| matches!(t.turn, crate::activity::Turn::Busy | crate::activity::Turn::Waiting));
        if !first_turn && floors.contains_key(&c.id) {
            c.ties.opening = None;
            c.ties.worktrees_before = None;
        }
    }
    chats.sort_by(|a, b| a.id.cmp(&b.id));
    spawns.sort_by_key(|s| s.request);
    SavedChats { chats, sends, spawns }
}

/// Writes `chats_now` out if it changed since the last write (background.rs
/// `save` keeps the beat).
pub fn save_chats() {
    use std::sync::atomic::Ordering;
    static LAST: Mutex<Vec<u8>> = Mutex::new(Vec::new());
    if !CHATS_READ.load(Ordering::SeqCst) || CHATS_FROZEN.load(Ordering::SeqCst) {
        return;
    }
    let Ok(bytes) = serde_json::to_vec_pretty(&chats_now()) else { return };
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if *last == bytes {
        return;
    }
    let path = chat_ties_file();
    let tmp = path.with_extension("json.tmp");
    match std::fs::write(&tmp, &bytes).and_then(|()| std::fs::rename(&tmp, &path)) {
        Ok(()) => *last = bytes,
        Err(e) => crate::log::warn("agents", &format!("could not save the chats' ties to {}: {e}", path.display())),
    }
}

/// Takes up what the last core saved of the sessions the window runs, before
/// anything can ask after them. They are known again as they were, lineage
/// and all, though not running until the window starts them (`Known::waiting`).
pub fn restore_chats() {
    let saved: SavedChats = match std::fs::read(chat_ties_file()) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            crate::log::warn("agents", &format!("could not read chat-ties.json: {e}"));
            SavedChats::default()
        }),
        Err(_) => SavedChats::default(),
    };
    let n = saved.chats.len();
    take_up_chats(saved);
    CHATS_READ.store(true, std::sync::atomic::Ordering::SeqCst);
    if n > 0 {
        crate::log::warn("agents", &format!("took up the ties of {n} chat(s) the last core left"));
    }
}

fn take_up_chats(saved: SavedChats) {
    let mut counts: Vec<(String, u32, Option<std::path::PathBuf>)> = Vec::new();
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    for c in saved.chats {
        let ties = c.ties;
        let resumes = ties.settings["resumeOnRestart"] == true;
        r.known.entry(c.id.clone()).or_insert(Known {
            provider: c.provider,
            account: account_name(&c.account_path),
            folder: c.folder,
            account_path: c.account_path,
            parent: ties.parent,
            settings: ties.settings,
            tools: ties.tools,
            waiting: true,
            orphan: ties.orphan,
        });
        if let Some(name) = ties.name {
            r.names.insert(c.id.clone(), name);
        }
        if ties.borrowed {
            r.borrowed.insert(c.id.clone());
        }
        if ties.auto_delete {
            r.auto.insert(c.id.clone(), Auto::default());
        }
        // An answer not in by the time the core went will not come: the CLI
        // it was being written by went with it, and the one the window starts
        // is between turns. That turn counts as ended, as a helper's does.
        let mut count = c.turns_ended;
        // One cut short under an earlier core and not carried on by now was called
        // off (`call_off_resume`): the mark would still be there otherwise.
        let mut went_now = c.mid_turn;
        for (caller, baseline, cursor) in ties.owed {
            let mut o = Owed::from(baseline, cursor);
            let already = ties.cut_short.contains(&caller);
            if c.turns_ended <= baseline || already {
                o.cut_short = true;
                count = count.max(baseline.saturating_add(1));
                went_now |= !already;
            }
            r.owed.insert((caller, c.id.clone()), o);
        }
        for (caller, n) in ties.seen {
            r.seen.insert((caller, c.id.clone()), n);
        }
        if let Some(opening) = ties.opening {
            // As asked: a file an older core wrote has it as typed, with any note.
            let text = crate::pty::without_note(&opening).to_string();
            r.openings.insert(c.id.clone(), Opening { text, floor: count, lost: ties.opening_lost, at_ms: ties.opening_at.unwrap_or(0) });
        }
        if let Some(name) = ties.worktree_name {
            r.worktree_names.insert(c.id.clone(), name);
        }
        if let Some(before) = ties.worktrees_before {
            r.worktrees_before.insert(c.id.clone(), before);
        }
        if !ties.worktrees_left.is_empty() {
            r.worktrees_left.insert(c.id.clone(), ties.worktrees_left);
        }
        if resumes && went_now {
            let opening = r.openings.get(&c.id).map(|o| o.text.clone());
            r.resumes.insert(c.id.clone(), ChatResume { opening, looks: 0 });
        }
        counts.push((c.id, count, c.transcript));
    }
    for h in saved.sends {
        r.held.push(Held { caller: h.caller, id: h.id, text: h.text, since: now_ms(), told: false, going_in: h.going_in });
    }
    // The window may be part way through one yet, and reach this core with it.
    for mut s in saved.spawns {
        s.left_by_core = true;
        r.next_request = r.next_request.max(s.request.saturating_add(1));
        r.abandoned.insert(s.request, (s.caller.clone(), s.in_worktree.is_some()));
        r.spawns.insert(s.request, s);
    }
    drop(r);
    for (id, count, transcript) in counts {
        crate::activity::keep_count(&id, count);
        if let Some(path) = transcript {
            transcripts().lock().unwrap_or_else(|e| e.into_inner()).insert(id, path);
        }
    }
}

/// How long this core has been up.
fn core_up_ms() -> u64 {
    static UP: OnceLock<u64> = OnceLock::new();
    now_ms().saturating_sub(*UP.get_or_init(now_ms))
}

/// Whether a session the last core knew may yet be started by the window.
fn returning(k: &Known, up_ms: u64) -> bool {
    k.waiting && up_ms < SESSION_RETURN_MS
}

/// A spawn the last core left that had come to a session it saved, and that had not
/// got as far as `owe_from`: that session answers its caller as any other, so owes
/// it now. With no turn of it ended that turn went with the core, as `take_up_chats`
/// has it for a chat's debt the count had not passed, and it may yet go on by itself.
/// With one ended, it is the answer. Once for each spawn (`owed_placed`): under a later
/// core the debt may have been told and settled, and the helper be long done. Once
/// both registries are read.
pub fn take_up_spawns() {
    let due: Vec<(u64, String, String)> = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        r.spawns
            .values()
            .filter(|s| !s.owed_placed)
            .filter_map(|s| s.helper.as_ref().filter(|h| r.known.contains_key(*h)).map(|h| (s.request, s.caller.clone(), h.clone())))
            .collect()
    };
    for (request, caller, id) in due {
        let chat = !crate::background::has(&id);
        let ended = crate::activity::turns_counted(&id).unwrap_or(0);
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = r.spawns.get_mut(&request) {
            s.owed_placed = true;
        }
        let key = (caller, id.clone());
        if r.owed.contains_key(&key) {
            continue;
        }
        let went = chat && ended == 0;
        r.owed.insert(key.clone(), Owed { cut_short: went, ..Owed::from(0, 0) });
        r.seen.entry(key).or_insert(0);
        if !went {
            continue;
        }
        if let Some(opening) = r.openings.get_mut(&id) {
            opening.floor = opening.floor.max(1);
        }
        if r.known.get(&id).is_some_and(|k| k.settings["resumeOnRestart"] == true) && !r.resumes.contains_key(&id) {
            let opening = r.openings.get(&id).map(|o| o.text.clone());
            r.resumes.insert(id.clone(), ChatResume { opening, looks: 0 });
        }
        drop(r);
        crate::activity::keep_count(&id, 1);
    }
}

/// The window's Delete on a background helper: as its agent would, the
/// worktree it made going too, unless it was handed one or the person said to
/// keep it.
pub fn delete_by_user(id: &str, drop_worktree: bool) -> Result<(), String> {
    let known = reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(id).cloned().ok_or("no such helper")?;
    match remove(id, known, drop_worktree, None)?.worktree_error {
        Some(why) => Err(why),
        None => Ok(()),
    }
}

/// A Claude Code hook (hub.rs): what it puts up for an answer, kept until
/// the tool runs or the turn is over.
pub fn on_hook(chat: &str, body: &str) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else { return };
    let tool = v["tool_name"].as_str().unwrap_or("");
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    match v["hook_event_name"].as_str() {
        Some("PermissionRequest") => {
            r.hook_asks.insert(chat.to_string(), (tool.to_string(), v["tool_input"].clone()));
        }
        Some("PreToolUse") if tool == "AskUserQuestion" => {
            r.hook_asks.insert(chat.to_string(), (tool.to_string(), v["tool_input"].clone()));
        }
        Some("PostToolUse" | "PostToolUseFailure" | "Stop" | "StopFailure" | "UserPromptSubmit" | "SessionEnd") => {
            r.hook_asks.remove(chat);
        }
        _ => {}
    }
}

/// Whether the session is there to talk to:a pty session that runs, or a
/// background helper, which is there between its turns too.
fn live(pty: &crate::pty::PtyManager, id: &str) -> bool {
    crate::background::has(id) || pty.alive(id)
}

/// What a session waits on a person for, if anything: a background helper's
/// held call; a Claude Code `AskUserQuestion` with no answer yet, or the tool
/// a permission prompt is up for; a Codex question tool not yet answered, or
/// the approval its screen shows.
pub fn ask_of(id: &str) -> Option<Ask> {
    if crate::background::has(id) {
        return crate::background::ask(id);
    }
    let pty = crate::pty::manager();
    let p = pty.probe(id)?;
    let turn = crate::activity::turn_state(id).map(|t| t.turn);
    let waiting = turn == Some(crate::activity::Turn::Waiting);
    match p.provider {
        Provider::Claude => {
            // The hooks say it whole; the transcript has a tool call held for
            // permission only sometimes, and a question not before its answer.
            let hooked = reg().lock().unwrap_or_else(|e| e.into_inner()).hook_asks.get(id).cloned();
            let (name, input) = hooked
                .filter(|_| turn.is_some_and(|t| t != crate::activity::Turn::Idle))
                .or_else(|| waiting.then(|| crate::claude::session::pending_tool(&transcript_path(&pty, id)?)).flatten())?;
            if name == "AskUserQuestion" {
                Ask::from_claude_questions(&input)
            } else {
                waiting.then(|| Ask::from_claude_tool(&name, &input))
            }
        }
        Provider::Codex => crate::codex::session::pending_ask(&crate::codex::session::Live {
            chat_id: &p.id,
            cwd: &p.cwd,
            spawned_at_ms: p.spawned_at_ms,
            resume: p.resume.as_deref(),
            account_path: &p.account_path,
            last_output_ms: p.last_output_ms,
        })
        .or_else(|| waiting.then(|| Ask::permission(None, pty.screen(id).as_deref().and_then(codex_command_on))))
        .or_else(|| (turn != Some(crate::activity::Turn::Busy)).then(|| pty.screen(id).as_deref().and_then(codex_list_on)).flatten()),
    }
}

/// The command a Codex approval dialog shows, on its `$ ` line.
fn codex_command_on(screen: &str) -> Option<String> {
    screen.lines().map(str::trim).find_map(|l| l.strip_prefix("$ ")).map(|c| c.trim().to_string()).filter(|c| !c.is_empty())
}

/// A list Codex holds its screen on until an option is picked — "Hooks need
/// review" at startup and the like — as a question: the lines above the
/// options for its text, the options as numbered, and the one the cursor is
/// on. Only at the bottom of the screen, above a footer naming Enter and Esc,
/// so that a numbered list in the conversation is not taken for one.
fn codex_list_on(screen: &str) -> Option<Ask> {
    let lines: Vec<&str> = screen.lines().map(str::trim).collect();
    let foot = lines.iter().rposition(|l| !l.is_empty())?;
    let f = lines[foot].to_lowercase();
    if !(f.contains("enter") && f.contains("esc")) {
        return None;
    }
    let numbered = |l: &str| -> Option<(usize, bool, String)> {
        let (on, rest) = match l.strip_prefix('›') {
            Some(r) => (true, r.trim_start()),
            None => (false, l),
        };
        let digits = rest.find(|c: char| !c.is_ascii_digit()).filter(|&d| d > 0)?;
        let n = rest[..digits].parse().ok()?;
        // A description may follow the label after a run of spaces.
        let label = rest[digits..].strip_prefix(". ")?.split("  ").next()?.trim();
        (!label.is_empty()).then(|| (n, on, label.to_string()))
    };
    // Up from the footer to option 1; lines between options are descriptions.
    let mut found: Vec<(usize, bool, String)> = Vec::new();
    let mut first = None;
    for i in (foot.saturating_sub(30)..foot).rev() {
        // The composer's `›` below a numbered list: the conversation, with a
        // footer of its own (a selection's "enter copy · esc clear").
        if lines[i].starts_with('›') && numbered(lines[i]).is_none() {
            return None;
        }
        if let Some(o) = numbered(lines[i]) {
            if found.last().is_some_and(|last| last.0 != o.0 + 1) {
                return None;
            }
            let one = o.0 == 1;
            found.push(o);
            if one {
                first = Some(i);
                break;
            }
        }
    }
    let first = first.filter(|_| found.len() >= 2)?;
    found.reverse();
    let at = found.iter().position(|o| o.1).unwrap_or(0);
    let options = found.into_iter().map(|o| o.2).collect();
    // The paragraph right above the options.
    let mut text: Vec<&str> =
        lines[..first].iter().rev().skip_while(|l| l.is_empty()).take_while(|l| !l.is_empty()).copied().collect();
    text.reverse();
    let text = if text.is_empty() { "Codex asks".to_string() } else { text.join(" ") };
    Some(Ask::question(vec![AskQuestion { text, options, multi: false, list_at: Some(at) }], true))
}

/// One word for where a session stands, and what it asks if it asks:
/// `working`, `waiting` (a permission), `question` (a question that holds its
/// turn), `background` (its turn is over, a job it started still runs),
/// `idle`, `starting` (its CLI has not drawn yet), `exited`.
pub fn state_of(id: &str) -> (&'static str, Option<Ask>) {
    use crate::activity::Turn;
    let pty = crate::pty::manager();
    if !live(&pty, id) {
        return ("exited", None);
    }
    if still_starting(&pty, id) {
        return ("starting", None);
    }
    let ask = ask_of(id);
    let ts = crate::activity::turn_state(id);
    let state = match (ts.map(|t| t.turn), &ask) {
        (_, Some(a)) if a.blocking && a.kind == "question" => "question",
        (Some(Turn::Waiting), _) => "waiting",
        (Some(Turn::Busy), _) => "working",
        _ if ts.is_some_and(|t| t.children_busy) || waits_on_helpers(id, "") => "background",
        _ => "idle",
    };
    (state, ask)
}

/// Whether `id` has helpers of its own still to answer it (`except` aside): it
/// is not through with what it was asked until it has their answers.
fn waits_on_helpers(id: &str, except: &str) -> bool {
    waits_on_helpers_in(&reg().lock().unwrap_or_else(|e| e.into_inner()), id, except)
}

fn waits_on_helpers_in(r: &Registry, id: &str, except: &str) -> bool {
    r.owed.keys().any(|(caller, helper)| caller == id && helper != except && is_descendant(r, id, helper))
}

/// An answer to one question as the CLI is to get it: an option's number
/// (several, apart by commas, where more than one may be picked) or label
/// becomes that option's label; anything else is the answer in its own words.
pub fn resolve_option(q: &AskQuestion, answer: &str) -> String {
    let pick = |part: &str| -> Option<String> {
        let part = part.trim();
        if let Ok(n) = part.parse::<usize>() {
            return q.options.get(n.checked_sub(1)?).cloned();
        }
        q.options.iter().find(|o| o.eq_ignore_ascii_case(part)).cloned()
    };
    if let Some(o) = pick(answer) {
        return o;
    }
    if q.multi {
        let parts: Option<Vec<String>> = answer.split(',').map(pick).collect();
        if let Some(p) = parts.filter(|p| !p.is_empty()) {
            return p.join(", ");
        }
    }
    answer.trim().to_string()
}

fn depth_of(r: &Registry, id: &str) -> usize {
    let mut d = 0;
    let mut cur = id.to_string();
    while let Some(p) = r.known.get(&cur).and_then(|k| k.parent.clone()) {
        d += 1;
        cur = p;
        if d > 16 {
            break;
        }
    }
    d
}

fn is_descendant(r: &Registry, of: &str, id: &str) -> bool {
    let mut cur = id.to_string();
    for _ in 0..16 {
        match r.known.get(&cur).and_then(|k| k.parent.clone()) {
            Some(p) if p == of => return true,
            Some(p) => cur = p,
            None => return false,
        }
    }
    false
}

fn descendants(r: &Registry, of: &str) -> Vec<String> {
    r.known.keys().filter(|id| is_descendant(r, of, id)).cloned().collect()
}

/// The target must be something the caller spawned, directly or through a
/// child. Anything else — the user's own chats, a sibling — is out of reach.
fn authorize(caller: &str, target: &str) -> Result<Known, String> {
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    if !is_descendant(&r, caller, target) {
        return Err(format!("session {target} is not one you spawned"));
    }
    r.known.get(target).cloned().ok_or_else(|| format!("unknown session {target}"))
}

/// Reading a session and waiting on it reach any session but the caller's
/// own: the transcript is on disk for anyone `luna_sessions` names it to, and
/// a caller that typed into a session has to be able to collect the answer.
fn authorize_view(caller: &str, target: &str) -> Result<Known, String> {
    if caller == target {
        return Err("that is your own session".into());
    }
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    r.known.get(target).cloned().ok_or_else(|| format!("unknown session {target}; luna_sessions lists them"))
}

/// Text may go to any session Luna runs, the user's own chats included —
/// that is how one session passes a message to another — but not back into
/// the caller's own composer.
fn authorize_send(caller: &str, target: &str) -> Result<Known, String> {
    if caller == target {
        return Err("that is your own session".into());
    }
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    r.known.get(target).cloned().ok_or_else(|| format!("unknown session {target}; luna_sessions lists them"))
}

// ------------------------------------------------------------------ tools --

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRow {
    pub id: String,
    pub name: String,
    /// The window group, 1-based as the title bar counts (I = 1).
    pub group: Option<u32>,
    pub provider: Provider,
    pub account: String,
    pub parent: Option<String>,
    /// See `state_of`.
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ask: Option<Ask>,
    pub turns_ended: u32,
    /// The flags it runs with (see `Known::settings`).
    pub settings: serde_json::Value,
    #[serde(flatten)]
    pub worktree: Worktree,
    /// An `autoDelete` helper idle with nobody owed its answer: the minutes
    /// left before it goes, unless it is read or written to first.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deletes_in_min: Option<u64>,
    #[serde(flatten)]
    pub signs: crate::background::Signs,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listing {
    pub me: String,
    pub accounts: Vec<AccountRow>,
    pub sessions: Vec<SessionRow>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AccountRow {
    pub provider: Provider,
    pub name: String,
    /// The settings a helper gets here unless the spawn says otherwise, for
    /// the caller's folder: the flags as `spawn` echoes them.
    pub defaults: serde_json::Value,
    /// The models a helper may ask for here: Claude Code's aliases (a full
    /// id works too), Codex's catalogue with each model's name and levels.
    pub models: Vec<serde_json::Value>,
    /// Its usage as the window last showed it (see `account_usage`); none
    /// before the window has said.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<serde_json::Value>,
}

/// Accounts an agent may spawn on — every account folder on disk, minus the
/// ones the user switched off for agents — with what a helper would get in
/// `folder` on each.
fn account_rows(folder: &str) -> Vec<AccountRow> {
    let usage = reg().lock().unwrap_or_else(|e| e.into_inner()).usage.clone();
    allowed_account_dirs()
        .into_iter()
        .map(|a| {
            let defaults = match a.provider {
                Provider::Claude => crate::claude::defaults::flags(&a.path, folder),
                Provider::Codex => crate::codex::defaults::flags(&a.path, folder),
            };
            let usage = usage.get(&format!("{}/{}", a.provider.as_str(), a.name)).cloned();
            let models = match a.provider {
                Provider::Claude => CLAUDE_MODELS.iter().map(|m| serde_json::json!({ "id": m })).collect(),
                Provider::Codex => crate::codex::models::models_known(&a.path)
                    .into_iter()
                    .map(|m| {
                        serde_json::json!({ "id": m.slug, "name": m.display_name, "efforts": m.efforts, "defaultEffort": m.default_effort })
                    })
                    .collect(),
            };
            AccountRow { provider: a.provider, name: a.name, defaults, models, usage }
        })
        .collect()
}

/// The same with each account's folder.
fn allowed_account_dirs() -> Vec<crate::accounts::AccountInfo> {
    let blocked = crate::settings::get().agent_blocked_accounts;
    crate::accounts::list_accounts()
        .unwrap_or_default()
        .into_iter()
        .filter(|a| !blocked.iter().any(|b| b == &format!("{}/{}", a.provider.as_str(), a.name)))
        .collect()
}

/// Whether an account folder holds a login its CLI can start on: Claude
/// Code's OAuth refresh token, or Codex's ChatGPT tokens or API key. A
/// helper sent to an account nobody signed in to sits on a login screen.
fn signed_in(provider: Provider, account_path: &str) -> bool {
    let read = |file: &str| {
        std::fs::read_to_string(std::path::Path::new(account_path).join(file))
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
    };
    match provider {
        Provider::Claude => read(".credentials.json").is_some_and(|v| v["claudeAiOauth"]["refreshToken"].is_string()),
        Provider::Codex => read("auth.json").is_some_and(|v| {
            v["tokens"].is_object() || v["OPENAI_API_KEY"].as_str().is_some_and(|k| !k.is_empty())
        }),
    }
}

/// What `luna_list` says of a visible session beyond its state: its turn's
/// start from the activity tracker, and from its transcript when the CLI last
/// wrote there — the screen is no measure, a spinner paints through a hung
/// call — and the model. What it spent only where the file has it to hand:
/// Codex's rollout states the thread's total. Claude Code's transcript would
/// have to be summed whole, line by line with repeats, its subagents in files
/// of their own and no cost anywhere.
fn visible_signs(pty: &crate::pty::PtyManager, id: &str, provider: Provider) -> crate::background::Signs {
    let turn_started_at = crate::activity::turn_started_ms(id).and_then(crate::background::iso);
    let Some(path) = transcript_path(pty, id) else {
        return crate::background::Signs { turn_started_at, ..Default::default() };
    };
    let last_activity_at = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|d| crate::background::iso(d.as_millis() as u64));
    let (resolved_model, spend) = match provider {
        Provider::Claude => (crate::claude::session::last_model(&path), None),
        Provider::Codex => {
            let tail = crate::codex::session::read_tail(&path);
            let session = tail.total_usage.as_ref().map(crate::background::Usage::of_codex);
            // The rollout names the model a turn is to run on before asking it:
            // the one that answered is the last with tokens spent after it.
            let model = crate::codex::session::answered_model(&path);
            (model, session.map(|session| crate::background::Spend { last_turn: None, session }))
        }
    };
    let left_running = crate::background::shown(&crate::activity::left_running(id), now_ms());
    crate::background::Signs { turn_started_at, last_activity_at, resolved_model, spend, left_running }
}

pub fn list(caller: &str) -> Listing {
    let rows: Vec<(String, Known, String, Option<u32>)> = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        descendants(&r, caller)
            .into_iter()
            .filter_map(|id| {
                let name = r.names.get(&id).cloned().unwrap_or_default();
                let group = r.groups.get(&id).map(|g| g + 1);
                r.known.get(&id).cloned().map(|k| (id, k, name, group))
            })
            .collect()
    };
    let pty = crate::pty::manager();
    let sessions = rows
        .into_iter()
        .map(|(id, k, name, group)| {
            let deletes_in_min = reg()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .auto
                .get(&id)
                .and_then(|a| a.idle_since)
                .map(|since| AUTO_DELETE_IDLE_MS.saturating_sub(now_ms().saturating_sub(since)).div_ceil(60_000));
            let ts = crate::activity::turn_state(&id);
            // A listing is not a reading: it does not move what the caller
            // is taken to have seen of a session's turns.
            let turns_ended = ts.map(|t| t.turns_ended).unwrap_or(0);
            let (state, ask) = state_of(&id);
            let worktree = worktree_of(&pty, &id);
            let signs = crate::background::signs(&id).unwrap_or_else(|| visible_signs(&pty, &id, k.provider));
            SessionRow {
                state,
                ask,
                turns_ended,
                id,
                name,
                group,
                provider: k.provider,
                account: k.account,
                parent: k.parent,
                settings: k.settings,
                worktree,
                deletes_in_min,
                signs,
            }
        })
        .collect();
    let folder = reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(caller).map(|k| k.folder.clone()).unwrap_or_default();
    Listing { me: caller.to_string(), accounts: account_rows(&folder), sessions }
}

/// One row of `sessions`: any chat Luna has run a session for since it
/// started, whoever started it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnySession {
    pub id: String,
    pub name: String,
    /// The window group, 1-based as the title bar counts (I = 1).
    pub group: Option<u32>,
    pub provider: Provider,
    pub account: String,
    /// The account directory — `CLAUDE_CONFIG_DIR` / `CODEX_HOME` of the
    /// session, where its registry and transcripts live.
    pub account_path: String,
    /// The chat's project folder.
    pub folder: String,
    /// Where the CLI actually runs: the worktree, for a chat in one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// The CLI's own id for the conversation (`--resume` / `codex resume`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// The transcript (Claude Code) or rollout (Codex) file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    pub parent: Option<String>,
    /// The caller itself.
    pub you: bool,
    /// Spawned by the caller (directly or through a child): every tool
    /// reaches it, not only `luna_send`.
    pub yours: bool,
    pub alive: bool,
    /// See `state_of`.
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ask: Option<Ask>,
    /// Runs with no terminal and no row (background.rs).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub background: bool,
    pub settings: serde_json::Value,
}

/// Which sessions `sessions` returns: a window group, `mine` for the
/// caller's own, and text to look for in the name, folder, account or id.
#[derive(Deserialize, Default)]
pub struct SessionFilter {
    #[serde(default)]
    pub group: Option<serde_json::Value>,
    #[serde(default)]
    pub q: Option<String>,
    /// Past spawns instead (agentlog.rs), deleted helpers included.
    #[serde(default)]
    pub history: Option<bool>,
}

impl SessionFilter {
    /// The 1-based group asked for, `mine` resolved against the caller's.
    fn group(&self, mine: Option<u32>) -> Result<Option<u32>, String> {
        let Some(v) = &self.group else { return Ok(None) };
        if v.as_str().is_some_and(|s| s.trim().eq_ignore_ascii_case("mine")) {
            return mine.map(Some).ok_or_else(|| "your session is in no window group, so \"mine\" matches nothing".to_string());
        }
        v.as_u64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
            .filter(|g| *g >= 1)
            .map(|g| Some(g as u32))
            .ok_or_else(|| format!("group {v} is neither a number from 1 nor \"mine\""))
    }
}

/// Whether a row passes a filter whose group is already resolved.
fn keeps(row: &AnySession, group: Option<u32>, q: Option<&str>) -> bool {
    if group.is_some_and(|g| row.group != Some(g)) {
        return false;
    }
    let Some(q) = q.map(str::trim).filter(|q| !q.is_empty()) else { return true };
    let q = q.to_lowercase();
    [&row.name, &row.folder, &row.account, &row.id].iter().any(|f| f.to_lowercase().contains(&q))
}

/// Every session Luna runs, for a caller that has to find another one — to
/// write to it, or to read its transcript off the disk.
pub fn sessions(caller: &str, filter: &SessionFilter) -> Result<Vec<AnySession>, String> {
    type Row = (String, Known, String, Option<u32>, bool);
    let (rows, mine): (Vec<Row>, Option<u32>) = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let rows = r
            .known
            .iter()
            .filter(|(id, _)| id.as_str() != DEV_CALLER || caller == DEV_CALLER)
            .map(|(id, k)| {
                let name = r.names.get(id).cloned().unwrap_or_default();
                let group = r.groups.get(id).map(|g| g + 1);
                (id.clone(), k.clone(), name, group, is_descendant(&r, caller, id))
            })
            .collect();
        (rows, r.groups.get(caller).map(|g| g + 1))
    };
    let group = filter.group(mine)?;
    let pty = Some(crate::pty::manager());
    let mut out: Vec<AnySession> = rows
        .into_iter()
        .map(|(id, k, name, group, yours)| {
            let probe = pty.as_ref().and_then(|p| p.probe(&id));
            let meta = probe.as_ref().and_then(disk_meta);
            let transcript = pty.as_ref().and_then(|p| transcript_path(p, &id)).map(|p| p.to_string_lossy().into_owned());
            // The row's name, else what the CLI calls the conversation.
            let name = Some(name)
                .filter(|n| !n.trim().is_empty())
                .or_else(|| meta.as_ref().and_then(|m| m.title.clone().or_else(|| m.first_prompt.clone())))
                .unwrap_or_default();
            let background = crate::background::has(&id);
            let (state, ask) = state_of(&id);
            AnySession {
                you: id == caller,
                yours,
                alive: probe.is_some() || background,
                state,
                ask,
                background,
                pid: probe.as_ref().and_then(|p| p.pid),
                cwd: meta
                    .as_ref()
                    .and_then(|m| m.cwd.clone())
                    .or_else(|| probe.as_ref().map(|p| p.cwd.clone()))
                    .or_else(|| crate::background::working_dir(&id)),
                session_id: meta.and_then(|m| m.session_id).or_else(|| crate::background::session_id(&id)),
                transcript,
                id,
                name,
                group,
                provider: k.provider,
                account: k.account,
                account_path: k.account_path,
                folder: k.folder,
                parent: k.parent,
                settings: k.settings,
            }
        })
        .collect();
    out.retain(|row| keeps(row, group, filter.q.as_deref()));
    out.sort_by(|a, b| b.alive.cmp(&a.alive).then_with(|| (&a.account, &a.name).cmp(&(&b.account, &b.name))));
    Ok(out)
}

/// What the CLI has on disk about a live session: its cwd, ids and title.
fn disk_meta(p: &crate::pty::ActivityProbe) -> Option<crate::pty::SessionMeta> {
    match p.provider {
        Provider::Claude => crate::claude::session::meta(p.pid, p.resume.as_deref(), &p.cwd, p.spawned_at_ms, &p.account_path),
        Provider::Codex => crate::codex::session::meta(&crate::codex::session::Live {
            chat_id: &p.id,
            cwd: &p.cwd,
            spawned_at_ms: p.spawned_at_ms,
            resume: p.resume.as_deref(),
            account_path: &p.account_path,
            last_output_ms: p.last_output_ms,
        }),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Spawned {
    pub id: String,
    /// Pass to `wait` as `afterTurn`: the reply to the prompt ends turn 1.
    pub turns_ended: u32,
    /// The flags the session runs with, as resolved from the request and the
    /// account's defaults.
    pub settings: serde_json::Value,
    /// True when the session was seen working on the prompt (or already done
    /// with it) within START_TIMEOUT. False means the prompt has not been
    /// taken up: look at `screen`.
    pub started: bool,
    /// The terminal as it stands, only when `started` is false and the CLI
    /// has drawn something.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screen: Option<String>,
    /// What it stopped on instead, when Luna can read it: a dialog of the
    /// CLI's own, which `luna_answer` answers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ask: Option<Ask>,
    /// See `still_starting`: not stuck, just not up yet.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub starting: bool,
    /// What the CLI flagged on its screen as it came up — a model the
    /// organization does not allow and the one used instead — which the
    /// settings above, being what it was started with, cannot say.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// Other live sessions working in the same directory, the caller aside:
    /// they share its files and its git index, commits and all.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub same_folder: Vec<Neighbour>,
    /// The worktree it works in and its branch, once the CLI has made it.
    #[serde(flatten)]
    pub worktree: Worktree,
    /// Asked for a worktree of its own and the CLI has not said where yet:
    /// `luna_list` names it once it has.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub worktree_pending: bool,
    /// With `account` as a list: the one taken, and why not the ones before.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_choice: Option<AccountChoice>,
}

/// How a list of accounts came down to one.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AccountChoice {
    pub account: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<SkippedAccount>,
    /// Why this one: the first with room, and how full it is; the full list's
    /// best; or the first signed in, its usage not known yet.
    pub why: String,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SkippedAccount {
    pub account: String,
    pub why: String,
}

/// Where a session in a worktree works: the path, and the branch there.
#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Worktree {
    #[serde(rename = "worktree", skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

/// The worktree a session works in — the one it made or the one it was
/// handed — with its branch; nothing for one in a plain folder. Its root,
/// wherever in it the CLI says it is now.
fn worktree_of(pty: &crate::pty::PtyManager, id: &str) -> Worktree {
    let dir = if crate::background::has(id) {
        crate::background::made_worktree(id).or_else(|| crate::background::working_dir(id))
    } else {
        pty.probe(id).and_then(|p| p.working_dir())
    };
    let Some((dir, project)) = dir.and_then(|d| crate::worktree::root_of(&d)) else {
        return Worktree::default();
    };
    Worktree { branch: crate::worktree::branch_of(&project, &dir), path: Some(dir) }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Neighbour {
    pub id: String,
    pub name: String,
    pub busy: bool,
}

/// What a CLI marks as warnings on its screen (⚠), each once, put back on one
/// line: a helper no pane shows has a narrow terminal, and the CLI wraps the
/// rest of a warning onto indented lines below it.
fn warnings_of(pty: &crate::pty::PtyManager, id: &str) -> Vec<String> {
    warnings_in(&pty.screen(id).unwrap_or_default())
}

fn warnings_in(screen: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut lines = screen.lines().peekable();
    while let Some(line) = lines.next() {
        let line = line.trim();
        if !line.starts_with('⚠') {
            continue;
        }
        let mut warning = line.to_string();
        while let Some(more) = lines.peek().filter(|l| l.starts_with("  ") && !l.trim().is_empty()) {
            warning.push(' ');
            warning.push_str(more.trim());
            lines.next();
        }
        if !out.contains(&warning) && out.len() < MAX_WARNINGS {
            out.push(warning);
        }
    }
    out
}

/// Live sessions other than `skip` that run in `dir`, by what the CLI says
/// its cwd is: the project folder for a chat in it, the worktree for one in
/// a worktree.
fn neighbours_in(dir: &str, skip: &[&str]) -> Vec<Neighbour> {
    let ids: Vec<String> = reg().lock().unwrap_or_else(|e| e.into_inner()).known.keys().cloned().collect();
    let pty = crate::pty::manager();
    let mut out = Vec::new();
    for id in ids.into_iter().filter(|id| !skip.contains(&id.as_str())) {
        let cwd = match (crate::background::working_dir(&id), pty.probe(&id)) {
            (Some(dir), _) => dir,
            (None, Some(probe)) => disk_meta(&probe).and_then(|m| m.cwd).unwrap_or_else(|| probe.cwd.clone()),
            (None, None) => continue,
        };
        if crate::pty::same_dir(&cwd, dir) {
            out.push(Neighbour {
                busy: crate::activity::turn_state(&id).is_some_and(|t| t.busy),
                name: chat_name(&id).unwrap_or_default(),
                id,
            });
        }
    }
    out
}

/// The screen says the CLI did not know the slash command `text` opens with.
/// Claude Code answers an unknown one on the spot and starts no turn, so
/// there is no start to wait out.
fn unknown_command(pty: &crate::pty::PtyManager, id: &str, text: &str) -> bool {
    let Some(cmd) = text.trim_start().strip_prefix('/').and_then(|rest| rest.split_whitespace().next()) else {
        return false;
    };
    pty.screen(id).is_some_and(|s| s.contains(&format!("Unknown command: /{cmd}")))
}

/// The last lines of a session's terminal, for a caller that has to see
/// what a helper is stuck on.
fn screen_of(pty: &crate::pty::PtyManager, id: &str) -> Option<String> {
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
    crate::activity::turn_state(id)
        .map(|t| matches!(t.turn, Turn::Busy | Turn::Waiting) || t.turns_ended > after_turn)
        .unwrap_or(false)
}

/// True while the CLI has not drawn anything yet: alive, no turn begun or
/// over, and a blank terminal. Claude Code syncs the org's skills and plugins
/// and makes its worktree before it draws a thing — minutes on a bad day,
/// with the session looking dead all along. The opening prompt is a CLI
/// argument and waits for it intact; text typed now would be lost. Of this
/// process's turns only: the count goes on across respawns, and one started again
/// after a core's restart is as blank as a new one.
fn still_starting(pty: &crate::pty::PtyManager, id: &str) -> bool {
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
fn await_start(
    pty: &crate::pty::PtyManager,
    id: &str,
    text: &str,
    after_turn: u32,
    timeout: std::time::Duration,
) -> bool {
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

/// The worktree a caller started with `--worktree` runs in. It registers
/// with the project folder, the only one known at spawn, and the CLI makes
/// the worktree itself; what it leaves on disk says where that went.
fn caller_worktree(caller: &str) -> Option<String> {
    let folder = reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(caller)?.folder.clone();
    let cwd = match crate::background::working_dir(caller) {
        Some(dir) => dir,
        None => crate::pty::manager().probe(caller)?.working_dir()?,
    };
    // Its root: the caller's shell may be in a folder of it.
    let (root, _) = crate::worktree::root_of(&cwd)?;
    crate::worktree::is_worktree_of(&folder, &root).then_some(root)
}

/// At this share of a usage window an account counts as spent when a spawn
/// lists several: usage is as old as the window's last fetch and a helper's
/// turn can use up what is left, stopping it on its limit mid-work.
const ACCOUNT_SPENT_PCT: f64 = 95.0;

/// The windows of an account's usage (as `account_usage` keeps it) that are
/// still running, with the percent used: one past its reset says nothing any
/// more, usage being only as fresh as the window's last fetch. None when it
/// has no figures yet.
fn usage_windows(usage: Option<&serde_json::Value>, now_ms: u64) -> Option<Vec<(String, f64)>> {
    let limits = usage?["limits"].as_array().filter(|l| !l.is_empty())?;
    let over = |l: &serde_json::Value| {
        l["resetsAt"]
            .as_str()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .is_some_and(|t| t.timestamp_millis() <= now_ms as i64)
    };
    Some(
        limits
            .iter()
            .filter(|l| !over(l))
            .filter_map(|l| Some((l["label"].as_str().unwrap_or("usage").to_string(), l["usedPct"].as_f64()?)))
            .collect(),
    )
}

/// The provider a list of accounts runs on: one that has every name agents may
/// use at all, the caller's first. An account name can exist under both.
fn list_provider(
    names: &[String],
    allowed: &[crate::accounts::AccountInfo],
    asked: Option<Provider>,
    mine: Provider,
) -> Result<Provider, String> {
    let known: Vec<&String> = names.iter().filter(|n| allowed.iter().any(|a| a.name == **n)).collect();
    let other = if mine == Provider::Claude { Provider::Codex } else { Provider::Claude };
    let order = asked.map_or(vec![mine, other], |p| vec![p]);
    order
        .into_iter()
        .find(|p| known.iter().all(|n| allowed.iter().any(|a| a.provider == *p && a.name == **n)))
        .ok_or_else(|| match asked {
            Some(p) => format!("the accounts in the list must all be {} accounts", p.as_str()),
            None => "the accounts in the list must all be of one provider".to_string(),
        })
}

/// The first account of the list that is there, signed in and has room in all
/// its windows; failing that, of those signed in, the one with the most room.
/// Unknown usage is no reason to pass over an account the caller put first.
fn pick_from_list(
    names: &[String],
    provider: Provider,
    allowed: &[crate::accounts::AccountInfo],
    usage: &HashMap<String, serde_json::Value>,
    signed_in: impl Fn(&crate::accounts::AccountInfo) -> bool,
    now_ms: u64,
) -> Result<(crate::accounts::AccountInfo, AccountChoice), String> {
    let mut skipped: Vec<SkippedAccount> = Vec::new();
    // Signed in, over the line: what is left if nothing qualifies.
    let mut spent: Vec<(&crate::accounts::AccountInfo, f64)> = Vec::new();
    for name in names {
        let mut skip = |why: String| skipped.push(SkippedAccount { account: name.clone(), why });
        let Some(a) = allowed.iter().find(|a| a.provider == provider && a.name == *name) else {
            skip(format!("no {} account of that name is available to agents", provider.as_str()));
            continue;
        };
        if !signed_in(a) {
            skip("not signed in".into());
            continue;
        }
        let windows = usage_windows(usage.get(&format!("{}/{}", provider.as_str(), a.name)), now_ms);
        let fullest = windows.iter().flatten().max_by(|x, y| x.1.total_cmp(&y.1));
        match fullest {
            Some((label, pct)) if *pct >= ACCOUNT_SPENT_PCT => {
                skip(format!("{label} at {pct:.0}%"));
                spent.push((a, *pct));
            }
            _ => {
                let why = match (&windows, fullest) {
                    (None, _) => "the first in the list signed in; its usage is not known yet".to_string(),
                    (Some(_), None) => "the first in the list with room: its windows have reset since Luna last read them".to_string(),
                    (Some(_), Some((label, pct))) => {
                        format!("the first in the list with room in every window (under {ACCOUNT_SPENT_PCT:.0}%); the fullest, {label}, is at {pct:.0}%")
                    }
                };
                return Ok((a.clone(), AccountChoice { account: a.name.clone(), skipped, why }));
            }
        }
    }
    if let Some((a, pct)) = spent.iter().min_by(|x, y| x.1.total_cmp(&y.1)) {
        skipped.retain(|s| s.account != a.name);
        let why = format!(
            "every signed-in account in the list is at {ACCOUNT_SPENT_PCT:.0}% or more of a window; this one has the most room, at {pct:.0}%"
        );
        return Ok(((*a).clone(), AccountChoice { account: a.name.clone(), skipped, why }));
    }
    if names.is_empty() {
        return Err("the account list is empty".into());
    }
    let why: Vec<String> = skipped.iter().map(|s| format!("{}: {}", s.account, s.why)).collect();
    Err(format!("no account in the list can take a helper ({}); luna_list shows the ones that can", why.join("; ")))
}

/// An agent's name as Claude Code takes one, a plugin's `plugin:name` too, and
/// a guard against one being read as a flag.
fn agent_name_ok(name: &str) -> bool {
    !name.starts_with('-') && name.chars().all(|c| c.is_ascii_alphanumeric() || "._:-".contains(c))
}

/// `readOnly` and `agent` checked, for a helper however it starts: what cannot
/// hold together is refused, and a read-only Codex gets its sandbox here.
fn settle_role(params: &mut SpawnParams, provider: Provider) -> Result<(), String> {
    use crate::pty::{AUTO_REVIEW, AUTO_REVIEW_SANDBOX, READ_ONLY_NET};
    params.agent = asked(&params.agent).map(str::to_owned);
    if let Some(agent) = params.agent.as_deref() {
        if provider == Provider::Codex {
            return Err("agent is a Claude Code option: Codex cannot start a session as a named agent; its roles \
                        are for the subagents a session spawns itself"
                .into());
        }
        if !agent_name_ok(agent) {
            return Err(format!("\"{agent}\" is not an agent's name"));
        }
    }
    if !params.read_only.unwrap_or(false) {
        return Ok(());
    }
    match provider {
        Provider::Claude => {
            if asked(&params.permission_mode) == Some("acceptEdits") {
                return Err("readOnly takes the edit tools away, so acceptEdits has nothing to accept; leave one out".into());
            }
        }
        Provider::Codex => {
            if asked(&params.approval) == Some(AUTO_REVIEW) {
                return Err(format!("{AUTO_REVIEW} runs in {AUTO_REVIEW_SANDBOX}, which is not read-only; leave one out"));
            }
            match asked(&params.sandbox) {
                None => params.sandbox = Some("read-only".into()),
                Some("read-only") | Some(READ_ONLY_NET) => {}
                Some(s) => {
                    return Err(format!(
                        "readOnly contradicts sandbox \"{s}\"; leave sandbox out, or use read-only or {READ_ONLY_NET}"
                    ))
                }
            }
        }
    }
    Ok(())
}

/// Asks the frontend to make the chat and start its session; blocks until it
/// answers or the timeout passes.
pub fn spawn(caller: &str, params: SpawnParams) -> Result<Spawned, String> {
    spawn_with(caller, params, None, &mut false)
}

/// `spawn`. `replaces`: the record of the spawn this asks for again. `still_coming`:
/// set when the error is only that the window has not answered in time, the request
/// standing for as long as it may yet (`agent_spawned`): no failure to act on.
fn spawn_with(caller: &str, mut params: SpawnParams, replaces: Option<u64>, still_coming: &mut bool) -> Result<Spawned, String> {
    if params.prompt.trim().is_empty() {
        return Err("prompt is required".into());
    }
    // Off the registry lock: this reads the CLI's registry from disk. A
    // helper works where its caller does, worktree and all.
    let worktree = if params.folder.is_none() { caller_worktree(caller) } else { None };
    let (taken, slot, in_worktree, account_choice) = {
        let me = {
            let r = reg().lock().unwrap_or_else(|e| e.into_inner());
            let me = r.known.get(caller).cloned().ok_or("caller is not a registered session")?;
            if descendants(&r, caller).len() >= MAX_DESCENDANTS {
                return Err(format!("you already have {MAX_DESCENDANTS} sessions; delete some first"));
            }
            if params.tools.unwrap_or(false) && depth_of(&r, caller) >= MAX_TOOL_DEPTH {
                return Err("a session this deep cannot spawn sessions with tools of their own".into());
            }
            me
        };
        // Off the registry lock: what follows reads folders and login files,
        // and every Claude Code session's hook waits on this lock with its next
        // step behind it (activity.rs `sample` has the same care).
        if params.folder.is_none() {
            params.folder = Some(worktree.unwrap_or_else(|| me.folder.clone()));
        }
        // A folder inside a CLI-made worktree files the chat under the
        // worktree's project, like the chat it came from; it runs in that
        // worktree unless it asked for one of its own.
        let mut in_worktree = None;
        if let Some(project) = params.folder.as_deref().and_then(crate::worktree::project_of) {
            let wt = params.folder.replace(project);
            if !params.worktree.unwrap_or(false) {
                in_worktree = wt;
            }
        }
        let allowed = allowed_account_dirs();
        let (one, list) = match params.account.take() {
            Some(AccountAsk::One(name)) => (Some(name), None),
            Some(AccountAsk::List(names)) => (None, Some(names)),
            None => (None, None),
        };
        let provider = match params.provider.as_deref() {
            // An account runs one provider only: one named that the
            // caller's provider has no account of brings its own.
            None => match one.as_deref() {
                Some(name) if !allowed.iter().any(|a| a.provider == me.provider && a.name == name) => {
                    allowed.iter().find(|a| a.name == name).map_or(me.provider, |a| a.provider)
                }
                _ => me.provider,
            },
            Some("claude") => Provider::Claude,
            Some("codex") => Provider::Codex,
            Some(other) => return Err(format!("unknown provider {other}; use claude or codex")),
        };
        let provider = match &list {
            Some(names) => list_provider(names, &allowed, params.provider.is_some().then_some(provider), me.provider)?,
            None => provider,
        };
        settle_role(&mut params, provider)?;
        params.provider = Some(provider.as_str().to_string());
        let picked = match &list {
            Some(names) => {
                // A copy, and off the lock: choosing reads login files.
                let usage = reg().lock().unwrap_or_else(|e| e.into_inner()).usage.clone();
                Some(pick_from_list(names, provider, &allowed, &usage, |a| signed_in(provider, &a.path), crate::throttle::now_ms())?)
            }
            None => None,
        };
        let mut ours = allowed.iter().filter(|a| a.provider == provider);
        let pick = match (&picked, one.as_deref()) {
            (Some((a, _)), _) => Some(a),
            (None, Some(name)) => ours.find(|a| a.name == name),
            // The caller's own account, else one somebody signed in to, else
            // any: the user said which accounts agents may use, not which
            // of them works.
            (None, None) => allowed
                .iter()
                .find(|a| a.provider == provider && provider == me.provider && a.name == me.account)
                .or_else(|| allowed.iter().find(|a| a.provider == provider && signed_in(provider, &a.path)))
                .or_else(|| ours.next()),
        };
        let Some(account) = pick.cloned() else {
            return Err(match one.as_deref() {
                Some(name) => format!(
                    "no {} account named {name:?} is available to agents; luna_list shows the ones that are",
                    provider.as_str()
                ),
                None => format!("no {} account is available to agents", provider.as_str()),
            });
        };
        params.account = Some(AccountAsk::One(account.name.clone()));
        let account_choice = picked.map(|(_, choice)| choice);
        let taken = SpawnUnderWay::begin(caller, &params, &account.path, in_worktree.as_deref(), replaces);
        if !params.visible.unwrap_or(false) {
            return spawn_background(caller, params, account.path, in_worktree, taken.0).map(|s| Spawned { account_choice, ..s });
        }
        let slot = std::sync::Arc::new((Mutex::new(Pending::default()), Condvar::new()));
        reg().lock().unwrap_or_else(|e| e.into_inner()).pending.insert(taken.0, slot.clone());
        (taken, slot, in_worktree, account_choice)
    };
    let request_id = taken.0;

    let borrowed = in_worktree.is_some();
    let auto_delete = params.auto_delete.unwrap_or(false);
    let own_worktree = params.worktree.unwrap_or(false);
    let folder = params.folder.clone().unwrap_or_default();
    // Where it will work: a worktree of its own shares nothing.
    let run_dir = (!own_worktree).then(|| in_worktree.clone().unwrap_or_else(|| folder.clone()));
    let (provider, account, prompt) =
        (params.provider.clone().unwrap_or_default(), params.account_name().unwrap_or_default().to_string(), params.prompt.clone());
    // A command line holds some 32 thousand characters on Windows.
    params.prompt = if provider == Provider::Codex.as_str() {
        spill(&crate::pty::codex_helper_prompt(caller, &params.prompt))
    } else {
        spill(&params.prompt)
    };
    let hooks_unasked = provider == Provider::Codex.as_str() && crate::codex::hooks::unasked_in(&folder);
    let asking = SpawnRequest { request_id, parent_id: caller.to_string(), in_worktree, hooks_unasked, params };
    if let Some(s) = reg().lock().unwrap_or_else(|e| e.into_inner()).spawns.get_mut(&request_id) {
        s.asked = Some(asking.clone());
    }
    crate::emit::to_ui("agent://spawn", asking);

    let (lock, cv) = &*slot;
    let mut p = lock.lock().unwrap_or_else(|e| e.into_inner());
    let deadline = std::time::Instant::now() + SPAWN_TIMEOUT;
    while p.done.is_none() {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            break;
        }
        let (guard, _) = cv.wait_timeout(p, left).unwrap_or_else(|e| e.into_inner());
        p = guard;
    }
    let mut outcome = p.done.take();
    drop(p);
    {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        r.pending.remove(&request_id);
        if outcome.is_none() {
            // The frontend may still make the chat (a slow trust write, a
            // worktree): `agent_spawned` then hands it to the caller.
            r.abandoned.insert(request_id, (caller.to_string(), borrowed));
        }
    }
    if outcome.is_none() {
        // An answer that landed between the timeout and the line above.
        outcome = lock.lock().unwrap_or_else(|e| e.into_inner()).done.take();
        if outcome.is_some() {
            reg().lock().unwrap_or_else(|e| e.into_inner()).abandoned.remove(&request_id);
        }
    }
    let id = match outcome {
        Some(Ok(id)) => {
            if borrowed {
                reg().lock().unwrap_or_else(|e| e.into_inner()).borrowed.insert(id.clone());
            }
            id
        }
        Some(Err(e)) => return Err(e),
        None => {
            // Still under way, for as long as the window may answer it.
            std::mem::forget(taken);
            *still_coming = true;
            return Err(format!(
                "the app did not start the session within {}s. It may still come up: luna_list will show it as yours \
                 and Luna will tell you when it answers, so check luna_list before spawning again. Helpers are made by \
                 Luna's window: if it is closed, they start once the user opens it",
                SPAWN_TIMEOUT.as_secs()
            ))
        }
    };
    // The frontend has registered the session by now (ensure_* ran before it
    // answered), so its settings are known; whether it took the prompt is
    // what the wait below finds out.
    let settings = reg()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .known
        .get(&id)
        .map(|k| k.settings.clone())
        .unwrap_or(serde_json::Value::Null);
    if auto_delete {
        reg().lock().unwrap_or_else(|e| e.into_inner()).auto.insert(id.clone(), Auto::default());
    }
    crate::agentlog::spawned(&crate::agentlog::Spawn {
        id: &id,
        parent: caller,
        parent_name: chat_name(caller),
        folder: &folder,
        worktree: own_worktree,
        provider: &provider,
        account: &account,
        settings: &settings,
        name: chat_name(&id),
        prompt: &prompt,
        auto_delete,
        background: false,
    });
    let pty = crate::pty::manager();
    let started = await_start(&pty, &id, &prompt, 0, START_TIMEOUT);
    let starting = !started && still_starting(&pty, &id);
    if starting {
        crate::log::info("agents", &format!("session {} spawned by {} has drawn nothing within {START_TIMEOUT:?}; still starting", label(&id), label(caller)));
    } else if !started && unknown_command(&pty, &id, &prompt) {
        crate::log::info("agents", &format!("session {} spawned by {} does not know the command its prompt opens with", label(&id), label(caller)));
    } else if !started {
        crate::log::warn("agents", &format!("session {} spawned by {} did not start on its prompt within {START_TIMEOUT:?}", label(&id), label(caller)));
    }
    let screen = (!started && !starting).then(|| screen_of(&pty, &id)).flatten();
    let ask = screen.is_some().then(|| ask_of(&id)).flatten();
    // A screen handed over says it all already.
    let warnings = if screen.is_none() { warnings_of(&pty, &id) } else { Vec::new() };
    let same_folder = run_dir.map(|dir| neighbours_in(&dir, &[caller, &id])).unwrap_or_default();
    shown(caller, &id, 0);
    owe_from(caller, &id, 0, 0);
    let worktree = worktree_of(&pty, &id);
    let worktree_pending = own_worktree && worktree.path.is_none();
    Ok(Spawned { id, turns_ended: 0, settings, started, screen, ask, starting, warnings, same_folder, worktree, worktree_pending, account_choice })
}

/// Claude Code's model aliases, as the new-chat dialog lists them.
pub const CLAUDE_MODELS: [&str; 4] = ["fable", "opus", "sonnet", "haiku"];
const CLAUDE_EFFORTS: [&str; 6] = ["low", "medium", "high", "xhigh", "max", "ultracode"];
const CLAUDE_MODES: [&str; 4] = ["default", "acceptEdits", "plan", "bypassPermissions"];

/// Given and not blank.
fn asked(v: &Option<String>) -> Option<&str> {
    v.as_deref().map(str::trim).filter(|v| !v.is_empty())
}

/// The settings a background helper runs with: what the request asks for
/// over the account's defaults, refused in the same words the window uses
/// for a visible one (agents/bridge.ts).
fn background_spec(params: &SpawnParams, provider: Provider, account_path: &str, folder: &str) -> Result<crate::background::Spec, String> {
    let worktree = params.worktree.unwrap_or(false);
    let tools = params.tools.unwrap_or(false);
    match provider {
        Provider::Claude => {
            if asked(&params.approval).is_some() || asked(&params.sandbox).is_some() {
                return Err("approval and sandbox are Codex settings; Claude takes permissionMode".into());
            }
            let d = crate::claude::defaults::flags(account_path, folder);
            let model = match asked(&params.model) {
                Some(m) if CLAUDE_MODELS.iter().any(|a| m.to_lowercase().contains(a)) => Some(m.to_string()),
                Some(m) => {
                    return Err(format!(
                        "unknown Claude model \"{m}\"; use {} or a full id such as claude-opus-5",
                        CLAUDE_MODELS.join(", ")
                    ))
                }
                None => d["model"].as_str().map(str::to_owned),
            };
            let effort = match asked(&params.effort) {
                Some(e) if CLAUDE_EFFORTS.contains(&e) => e.to_string(),
                Some(e) => return Err(format!("unknown Claude effort \"{e}\"; use {}", CLAUDE_EFFORTS.join(", "))),
                None => d["effort"].as_str().unwrap_or("high").to_string(),
            };
            let mode = match asked(&params.permission_mode) {
                Some(p) if CLAUDE_MODES.contains(&p) => p.to_string(),
                Some(p) => return Err(format!("unknown permission mode \"{p}\"; use {}", CLAUDE_MODES.join(", "))),
                None => d["permissionMode"].as_str().unwrap_or("default").to_string(),
            };
            Ok(crate::background::Spec {
                provider,
                account_path: account_path.to_string(),
                folder: folder.to_string(),
                worktree,
                worktree_name: None,
                model,
                effort,
                mode,
                auto_review: false,
                tools,
                hooks_unasked: false,
                parent_moved: false,
                read_only: params.read_only.unwrap_or(false),
                agent: params.agent.clone(),
                parent: None,
                token: None,
            })
        }
        Provider::Codex => {
            use crate::pty::{AUTO_REVIEW, AUTO_REVIEW_SANDBOX};
            if asked(&params.permission_mode).is_some() {
                return Err("permissionMode is a Claude setting; Codex takes approval and sandbox".into());
            }
            // Nobody is there to ask, but a reviewer model can answer.
            let auto_review = match asked(&params.approval) {
                None | Some("never") => false,
                Some(AUTO_REVIEW) => true,
                Some(_) => {
                    return Err(format!(
                        "a background Codex helper has nobody to ask for approval; leave approval out, \
                         use {AUTO_REVIEW}, or spawn it with visible: true"
                    ))
                }
            };
            let d = crate::codex::defaults::flags(account_path, folder);
            let sandbox = match asked(&params.sandbox) {
                Some(s) if auto_review && s != AUTO_REVIEW_SANDBOX => {
                    return Err(format!("{AUTO_REVIEW} runs in {AUTO_REVIEW_SANDBOX} only; leave sandbox out"))
                }
                Some(s) if ["read-only", crate::pty::READ_ONLY_NET, "workspace-write", "danger-full-access"].contains(&s) => s.to_string(),
                Some(s) => return Err(format!("unknown sandbox \"{s}\"; use read-only, read-only-net, workspace-write, danger-full-access")),
                None if auto_review => AUTO_REVIEW_SANDBOX.to_string(),
                None => d["sandbox"].as_str().unwrap_or("read-only").to_string(),
            };
            let models = crate::codex::models::models_known(account_path);
            let model = asked(&params.model).map(str::to_owned).or_else(|| d["model"].as_str().map(str::to_owned));
            let listed = model.as_deref().and_then(|m| models.iter().find(|x| x.slug == m));
            if let (Some(m), None, false) = (asked(&params.model), listed, models.is_empty()) {
                let all: Vec<&str> = models.iter().map(|x| x.slug.as_str()).collect();
                return Err(format!("this Codex account does not list model \"{m}\"; it has {}", all.join(", ")));
            }
            // As in the dialog: a model asked for brings its own default level.
            let effort = match (asked(&params.effort), listed) {
                (Some(e), Some(l)) if !l.efforts.is_empty() && !l.efforts.iter().any(|x| x == e) => {
                    return Err(format!("{} takes effort {}, not \"{e}\"", l.slug, l.efforts.join(", ")))
                }
                (Some(e), _) => e.to_string(),
                (None, Some(l)) if asked(&params.model).is_some() && l.default_effort.is_some() => l.default_effort.clone().unwrap_or_default(),
                (None, _) => d["effort"].as_str().unwrap_or("medium").to_string(),
            };
            Ok(crate::background::Spec {
                provider,
                account_path: account_path.to_string(),
                folder: folder.to_string(),
                worktree,
                worktree_name: None,
                model,
                effort,
                mode: sandbox,
                auto_review,
                tools,
                hooks_unasked: false,
                parent_moved: false,
                read_only: false,
                agent: None,
                parent: None,
                token: None,
            })
        }
    }
}

/// `spawn` for a helper with no row: started here, in the core, on the
/// settings `background_spec` works out.
fn spawn_background(caller: &str, params: SpawnParams, account_path: String, in_worktree: Option<String>, request: u64) -> Result<Spawned, String> {
    let provider = if params.provider.as_deref() == Some("codex") { Provider::Codex } else { Provider::Claude };
    let project = params.folder.clone().unwrap_or_default();
    let own_worktree = params.worktree.unwrap_or(false);
    // A helper handed its caller's worktree works in it and never owns it.
    let run_in = in_worktree.clone().unwrap_or_else(|| project.clone());
    let mut spec = background_spec(&params, provider, &account_path, &run_in)?;
    // As its project folder would start a chat of the user's: a helper does
    // not choose this for itself.
    spec.hooks_unasked = provider == Provider::Codex && crate::codex::hooks::unasked_in(&project);
    spec.parent = Some(caller.to_string());
    let id = crate::background::mint_id();
    spec.worktree_name = (own_worktree && provider == Provider::Claude).then(|| helper_worktree_name(&id));
    let mut settings = match provider {
        Provider::Claude => crate::pty::with_role(
            serde_json::json!({
                "model": spec.model, "effort": spec.effort, "permissionMode": spec.mode, "worktree": own_worktree, "background": true,
            }),
            spec.read_only,
            spec.agent.as_deref(),
        ),
        Provider::Codex => serde_json::json!({
            "model": spec.model, "effort": spec.effort,
            "approval": if spec.auto_review { crate::pty::AUTO_REVIEW } else { "never" }, "sandbox": spec.mode,
            "hooksUnasked": spec.hooks_unasked, "background": true,
        }),
    };
    if params.resume_on_restart == Some(true) {
        settings["resumeOnRestart"] = serde_json::json!(true);
    }
    let tools = spec.tools;
    let auto_delete = params.auto_delete.unwrap_or(true);
    let token = register(&id, provider, &run_in, &account_path, Some(caller), tools, settings.clone());
    // Claude Code asks its permissions of Luna, tools or not.
    spec.token = token.or_else(|| (provider == Provider::Claude).then(|| issue_token(&id)));
    let name = params
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_owned)
        .or_else(|| crate::pty::title_from_prompt(&params.prompt))
        .unwrap_or_else(|| "helper".into());
    set_name(&id, &name);
    {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        if in_worktree.is_some() {
            r.borrowed.insert(id.clone());
        }
        // Nobody sees it to delete it: it goes once its answer is out,
        // unless its caller means to delete it itself.
        if auto_delete {
            r.auto.insert(id.clone(), Auto::default());
        }
    }
    if let Err(e) = spawn_is(request, &id) {
        forget(&id);
        return Err(e);
    }
    if let Err(e) = crate::background::start(&id, spec, &params.prompt) {
        forget(&id);
        return Err(e);
    }
    crate::agentlog::spawned(&crate::agentlog::Spawn {
        id: &id,
        parent: caller,
        parent_name: chat_name(caller),
        folder: &project,
        worktree: own_worktree,
        provider: provider.as_str(),
        account: params.account_name().unwrap_or(""),
        settings: &settings,
        name: Some(name),
        prompt: &params.prompt,
        auto_delete,
        background: true,
    });
    let same_folder = if own_worktree { Vec::new() } else { neighbours_in(&run_in, &[caller, &id]) };
    shown(caller, &id, 0);
    owe_from(caller, &id, 0, 0);
    // The CLI makes its worktree as the first turn starts: worth a few
    // seconds, so the caller need not go looking for it.
    let pty = crate::pty::manager();
    let mut worktree = Worktree::default();
    if own_worktree {
        let deadline = std::time::Instant::now() + WORKTREE_WAIT;
        loop {
            worktree = worktree_of(&pty, &id);
            if worktree.path.is_some() || std::time::Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(POLL);
        }
    }
    let worktree_pending = own_worktree && worktree.path.is_none();
    Ok(Spawned { id, turns_ended: 0, settings, started: true, screen: None, ask: None, starting: false, warnings: Vec::new(), same_folder, worktree, worktree_pending, account_choice: None })
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
fn spill(text: &str) -> String {
    let n = text.chars().count();
    if n <= LONG_TEXT {
        return text.to_string();
    }
    let dir = notes_dir();
    let path = dir.join(format!("{}.md", crate::background::mint_id()));
    if std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, text)).is_err() {
        return text.to_string();
    }
    format!(
        "This message is {n} characters long, so Luna put it in a file: {}. Read the whole file first and take it as this message.",
        path.display()
    )
}

/// The session a spawn came to, as soon as it is registered and before its CLI is
/// started: what the spawn asked of it that the window does not carry goes on it now,
/// as a core may go before the spawn gets any further, and from here on
/// `take_up_spawns` finds it. Refused for a spawn given up on (`settle_spawns`), or
/// asked for again since, whose caller has been or is being told so: the session
/// goes unstarted and the window drops its row, rather than a second helper run or
/// what was said prove false.
pub fn spawn_is(request: u64, id: &str) -> Result<(), String> {
    {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let Some(s) = r.spawns.get_mut(&request).filter(|s| !s.settled) else {
            r.known.remove(id);
            r.tokens.retain(|_, chat| chat != id);
            return Err("Luna's core has since given up on this spawn".into());
        };
        s.helper = Some(id.to_string());
        let (resumes, borrowed, auto) = (s.resume_on_restart, s.in_worktree.is_some(), s.auto_delete == Some(true));
        // A background helper keeps its own (`Entry::opening`).
        let opening = s.visible.then(|| s.params.prompt.clone());
        let own_worktree = s.params.worktree == Some(true);
        let codex = s.params.provider.as_deref() == Some(Provider::Codex.as_str());
        let name = (own_worktree && !codex && s.visible).then(|| helper_worktree_name(id));
        let before = (own_worktree && codex).then(|| s.worktrees_before.clone());
        if borrowed {
            r.borrowed.insert(id.to_string());
        }
        if auto {
            r.auto.entry(id.to_string()).or_default();
        }
        if resumes {
            if let Some(k) = r.known.get_mut(id) {
                k.settings["resumeOnRestart"] = serde_json::json!(true);
            }
        }
        if let Some(text) = opening {
            r.openings.insert(id.to_string(), Opening { text, floor: 0, lost: false, at_ms: now_ms() });
        }
        if let Some(name) = name {
            r.worktree_names.insert(id.to_string(), name);
        }
        if let Some(before) = before {
            r.worktrees_before.insert(id.to_string(), before);
        }
    }
    crate::background::save();
    Ok(())
}

/// A visible helper's CLI is starting on no prompt of its own. With a conversation
/// to go back to (`conversation`) that holds what it was asked and the CLI knows
/// where it works. With none, its first turn having gone with a core, nothing has
/// what it was asked but `Opening`, and its caller is to get that back.
pub fn chat_starting(id: &str, conversation: bool) {
    let count = crate::activity::turns_counted(id).unwrap_or(0);
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    if conversation {
        r.worktrees_before.remove(id);
    } else if let Some(o) = r.openings.get_mut(id).filter(|o| o.floor >= 1 && count <= o.floor) {
        o.lost = true;
    }
}


/// What Luna names the worktree of a Claude Code helper it spawns, and so knows to be
/// that helper's: nothing else goes by this name, where a worktree merely new since the
/// spawn may be anybody's. Claude Code takes it by `--worktree=<name>`, makes it if it
/// is not there and goes back in if it is. Codex takes no name.
pub fn helper_worktree_name(id: &str) -> String {
    format!("luna-{id}")
}

/// The name a visible helper's CLI is to ask for, starting on no prompt of its own:
/// in a conversation the one it has, afresh the one it is still to go by. None for a
/// session Luna named none for: its CLI names its own, as a chat of a person's does.
pub fn worktree_for_start(id: &str, folder: &str, account_path: &str, conversation: bool) -> Option<String> {
    let name = reg().lock().unwrap_or_else(|e| e.into_inner()).worktree_names.get(id).cloned()?;
    if conversation {
        return Some(name);
    }
    let name = name_to_go_on(id, &name, folder, account_path);
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    match &name {
        Some(name) => r.worktree_names.insert(id.to_string(), name.clone()),
        None => r.worktree_names.remove(id),
    };
    name
}

/// What a helper that has no conversation asks for, its worktree named `name`: that
/// name if nothing is there, as the CLI then makes it, or if what is there is still as
/// the CLI made it with nobody in it (`worktree::as_made`), as going back in is then the
/// same as starting in a new one. Anything else is left alone, its parent hears of it
/// (`worktrees_left_line`), and the helper takes the next name. None past a few of those:
/// the CLI names its own.
pub fn name_to_go_on(id: &str, name: &str, folder: &str, account_path: &str) -> Option<String> {
    let mut busy = crate::pty::live_cwds();
    busy.extend(crate::worktree::claude_sessions_running(account_path));
    let mut left = Vec::new();
    let mut chosen = None;
    for n in 1..=9 {
        let candidate = if n == 1 { name.to_string() } else { format!("{name}-{n}") };
        let path = crate::worktree::named_path(folder, &candidate);
        let there = std::path::Path::new(&path).exists();
        if there && (!crate::worktree::as_made(folder, &path) || crate::worktree::someone_in(&path, &busy)) {
            left.push(path);
            continue;
        }
        if there {
            crate::log::warn("agents", &format!("{} goes back into {path}, which a core cut it off in", label(id)));
        }
        chosen = Some(candidate);
        break;
    }
    leave_worktrees(id, left);
    chosen.filter(|n| crate::worktree::name_ok(n))
}

/// A Codex helper starting afresh: what is new of Codex's since it was spawned may be
/// what a core cut it off making, and its parent hears. It is not gone back into, as
/// nothing says whose it is: Codex takes no name, and says where it works only with a
/// thread, which is then a conversation to resume. Ahead of what this start makes.
pub fn report_codex_worktrees(id: &str, folder: &str, account_path: &str) {
    let Some(before) = reg().lock().unwrap_or_else(|e| e.into_inner()).worktrees_before.remove(id) else { return };
    leave_worktrees(id, crate::worktree::codex_new_since(folder, account_path, &before, &crate::pty::live_cwds()));
}

/// `id`'s CLI is not back to go on in any of it: its parent hears of what it began, and
/// opened after that it makes its own.
fn leave_half_made(id: &str, k: &Known) {
    if k.provider == Provider::Codex {
        return report_codex_worktrees(id, &k.folder, &k.account_path);
    }
    let Some(name) = reg().lock().unwrap_or_else(|e| e.into_inner()).worktree_names.get(id).cloned() else { return };
    let path = crate::worktree::named_path(&k.folder, &name);
    if std::path::Path::new(&path).exists() {
        reg().lock().unwrap_or_else(|e| e.into_inner()).worktree_names.insert(id.to_string(), format!("{name}-2"));
        leave_worktrees(id, vec![path]);
    }
}

fn leave_worktrees(id: &str, left: Vec<String>) {
    if left.is_empty() {
        return;
    }
    crate::log::warn("agents", &format!("{} leaves what a core cut it off in: {}", label(id), left.join(", ")));
    {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let told = r.worktrees_left.entry(id.to_string()).or_default();
        for path in left {
            if !told.contains(&path) {
                told.push(path);
            }
        }
    }
    crate::background::save();
}

/// The conversation a visible helper's CLI was on as a core went, where nothing has
/// said so: it is told to the window by polling, every few seconds, and a core can go
/// first. A conversation is on disk or it is not, and that is what decides whether it
/// comes back to it, not whether the row has heard. Its session id as the CLI knows it.
pub fn conversation_of(id: &str, provider: Provider, folder: &str, account_path: &str) -> Option<String> {
    let path = conversation_on_disk(id, provider, folder, account_path)?;
    let file = path.file_name()?.to_string_lossy().into_owned();
    let session = match provider {
        Provider::Claude => file.strip_suffix(".jsonl")?.to_string(),
        Provider::Codex => crate::codex::session::parse_name(&file)?.1,
    };
    crate::log::warn("agents", &format!("{} goes back to its conversation {session}, which the window had not heard of", label(id)));
    transcripts().lock().unwrap_or_else(|e| e.into_inner()).insert(id.to_string(), path);
    Some(session)
}

/// The transcript of a conversation a helper whose first turn went with a core began:
/// the one the last core knew of if it holds anything, else one begun since it was asked
/// that holds what it was asked, in where it was to work, and no other session's.
fn conversation_on_disk(id: &str, provider: Provider, folder: &str, account_path: &str) -> Option<std::path::PathBuf> {
    let (text, at_ms, name) = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let o = r.openings.get(id).filter(|o| o.floor >= 1)?;
        (o.text.clone(), o.at_ms, r.worktree_names.get(id).cloned())
    };
    let (kept, others) = {
        let known = transcripts().lock().unwrap_or_else(|e| e.into_inner());
        let others: Vec<std::path::PathBuf> = known.iter().filter(|(k, _)| k.as_str() != id).map(|(_, p)| p.clone()).collect();
        (known.get(id).cloned(), others)
    };
    if let Some(path) = kept.filter(|p| p.is_file() && !messages_at(p, provider, 0).is_empty()) {
        return Some(path);
    }
    let found = match provider {
        Provider::Claude => {
            let mut cwds = vec![folder.to_string()];
            cwds.extend(name.map(|name| crate::worktree::named_path(folder, &name)));
            crate::claude::session::transcripts_since(account_path, &cwds, at_ms)
        }
        Provider::Codex => crate::codex::session::rollouts_since(account_path, folder, at_ms),
    };
    let needle: String = words(&text).chars().take(NEEDLE).collect();
    found.into_iter().filter(|p| !others.contains(p)).find(|p| said_to(&messages_at(p, provider, 0), &needle) == WentIn::Yes)
}

/// What the window has been asked to start and nothing has come of yet. An event
/// reaches only a page that is listening: `agent://spawn` sent with no window, or
/// while one is still loading, is gone, and the spawn times out on a window that is
/// there. So the page asks for these as it comes up and at every core, and takes each
/// once (`bridge.ts`). The oldest first, in the order they were asked.
#[tauri::command]
pub fn agent_spawns() -> Vec<SpawnRequest> {
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let mut asked: Vec<SpawnRequest> = r.spawns.values().filter(|s| s.helper.is_none() && !s.settled).filter_map(|s| s.asked.clone()).collect();
    asked.sort_by_key(|s| s.request_id);
    asked
}

/// The frontend's answer to `agent://spawn`.
#[tauri::command]
pub fn agent_spawned(request_id: u64, chat_id: Option<String>, name: Option<String>, error: Option<String>) {
    if let (Some(id), Some(n)) = (&chat_id, &name) {
        set_name(id, n);
    }
    // The answer goes in under the registry lock, which `spawn` takes to
    // give up on it: either it is in before the caller stops waiting, or the
    // request is found abandoned here.
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let Some(slot) = r.pending.get(&request_id).cloned() else {
        let caller = r.abandoned.remove(&request_id);
        if let (Some((_, true)), Some(id)) = (&caller, &chat_id) {
            r.borrowed.insert(id.clone());
        }
        // One the last core had made a session of owes already (`take_up_spawns`),
        // and what it said is not to be started over.
        let owed = chat_id.as_ref().zip(caller.as_ref()).is_some_and(|(id, (c, _))| r.owed.contains_key(&(c.clone(), id.clone())));
        // A failure of one the last core left is still news to its caller, who has
        // heard nothing: `settle_spawns` tells it. One that had timed out was told it
        // may yet come up, and is told now that it did not.
        let mut failed = None;
        if chat_id.is_some() {
            r.spawns.remove(&request_id);
        } else if let Some(s) = r.spawns.get_mut(&request_id).filter(|s| !s.left_by_core && !s.settled) {
            s.settled = true;
            failed = Some(s.clone());
        }
        drop(r);
        if let Some(s) = failed {
            let why = error.clone().unwrap_or_else(|| "spawn failed".into());
            let _ = std::thread::Builder::new().name("luna-spawn-failed".into()).spawn(move || spawn_failed_late(s, &why));
        }
        // Too late for the caller's spawn, not for the caller: the session
        // registered as its child, so `luna_list` shows it, and the answer
        // to its prompt is owed like any other.
        if let (Some((caller, _)), Some(id)) = (caller, chat_id) {
            crate::log::warn("agents", &format!("session {} for {} came up after its spawn timed out", label(&id), label(&caller)));
            if !owed {
                shown(&caller, &id, 0);
                owe_from(&caller, &id, 0, 0);
            }
        }
        crate::background::save();
        return;
    };
    let (lock, cv) = &*slot;
    let mut p = lock.lock().unwrap_or_else(|e| e.into_inner());
    p.done = Some(match (chat_id, error) {
        (Some(id), _) => Ok(id),
        (None, Some(e)) => Err(e),
        (None, None) => Err("spawn failed".into()),
    });
    cv.notify_all();
    drop(p);
    drop(r);
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sent {
    pub ok: bool,
    /// Where the session stood when the text went in (see `state_of`).
    pub state_before: &'static str,
    /// It was working: the text joins the turn in flight or is taken up
    /// after it (a background helper: always after), and `started` says
    /// nothing yet.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub queued: bool,
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
    /// background helper, whose queue holds the text until that turn ends.
    pub turns_ended: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screen: Option<String>,
}

/// Types text into a session and presses Enter after it; returns whether a
/// turn began. `hold` has kept a person's keys back since before the last look at
/// the session, and does until the Enter: one landing in between would go out with
/// the text. They go on after it, into an empty composer.
fn type_in(
    pty: &crate::pty::PtyManager,
    id: &str,
    text: &str,
    turn_before: Option<crate::activity::Turn>,
    baseline: u32,
    hold: crate::pty::KeyHold,
) -> Result<bool, String> {
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

/// Types text into the session, presses Enter after it, and reports whether
/// a turn began. With `interrupt` the turn in flight, and any prompt it is
/// stopped on, is cut short first (Esc; a background helper's process
/// stopped), and the text is the next turn rather than one queued after it.
pub fn send(caller: &str, id: &str, text: &str, interrupt: bool) -> Result<Sent, String> {
    authorize_send(caller, id)?;
    let pty = crate::pty::manager();
    if !live(&pty, id) {
        return Err(format!("session {id} is not running"));
    }
    // Before anything is read of it: what is going in now may be a notice, and
    // the session busy on it by the time this text's turn comes. Not there in
    // time, the text is held like any other (`hold`), and goes in when it can.
    // A background helper has no terminal: its queue takes turns by itself.
    let terminal = if crate::background::has(id) { None } else { TypingInto::wait_for(id, TYPING_WAIT) };
    let (state_before, ask) = state_of(id);
    if let Some(a) = ask.as_ref().filter(|a| a.blocking && !interrupt) {
        return Err(format!(
            "session {id} is on a {}: typed text would land in its dialog. Answer it with luna_answer, or send with interrupt: true to dismiss it",
            a.kind
        ));
    }
    let mut before = crate::activity::turn_state(id);
    let mut baseline = before.map(|t| t.turns_ended).unwrap_or(0);
    if crate::background::has(id) {
        // Taken before the text goes in: a turn it starts may be over, and
        // written, by the time the call returns.
        let cursor = transcript_end(id);
        let (queued, interrupted) = if interrupt {
            let i = crate::background::interrupt(id, text, Some(caller))?;
            (i.queued, i.stopped)
        } else {
            (crate::background::send(id, text, Some(caller))?, false)
        };
        // The turn it was on ends before the one this text starts, and is
        // counted: the answer ends the turn after that.
        if queued || interrupted {
            baseline += 1;
        }
        seen(caller, id, baseline);
        owe_from(caller, id, baseline, cursor);
        return Ok(Sent {
            ok: true,
            state_before,
            queued,
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
        press(&pty, id, &[Key::Seq("\x1b")])?;
        let deadline = std::time::Instant::now() + INTERRUPT_SETTLE;
        while busy(crate::activity::turn_state(id)) && std::time::Instant::now() < deadline {
            std::thread::sleep(POLL);
        }
        before = crate::activity::turn_state(id);
        baseline = before.map(|t| t.turns_ended).unwrap_or(0);
    }
    if still_starting(&pty, id) {
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
    let typed = if typing || terminal.is_none() { None } else { put_in(caller, id, &spill(text), before, baseline, interrupted)? };
    let Some(sent) = typed else {
        hold(caller, id, text);
        return Ok(Sent {
            ok: true,
            state_before,
            queued: false,
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
    let pty = crate::pty::manager();
    // After an Esc the TUI is between turns whatever the sampler has caught
    // up with: the text is a prompt of its own, not one queued.
    let turn_before = if interrupted { None } else { before.map(|t| t.turn) };
    let queued = turn_before == Some(crate::activity::Turn::Busy);
    // Before the text goes in: `type_in` waits for the turn to start, and a
    // short one is over, and written, by then.
    let cursor = transcript_end(id);
    let Some(started) = type_text(&pty, id, text, turn_before, baseline)? else { return Ok(None) };
    let started = started && !queued;
    if turn_before == Some(crate::activity::Turn::Waiting) {
        reg().lock().unwrap_or_else(|e| e.into_inner()).answered.insert((caller.to_string(), id.to_string()), now_ms());
    }
    let screen = (!started && !queued).then(|| screen_of(&pty, id)).flatten();
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
    Ok(Some(Sent {
        ok: true,
        state_before: "",
        queued,
        interrupted,
        held: false,
        started,
        turns_ended: baseline,
        screen,
    }))
}

/// Types text in as a prompt (`type_in`) — around a person's draft where
/// there is one and the CLI can put it aside (`stash_and_type`). Ok(None):
/// a draft is in the way, and nothing was typed.
fn type_text(
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
fn type_held(
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
    let started = turn_before == Some(crate::activity::Turn::Busy)
        || await_start(pty, id, text, baseline, SEND_TIMEOUT.saturating_sub(ENTER_RETRY));
    Ok(Some(started))
}

/// Puts a caller's text by for the notifier to type in (`deliver_held`).
fn hold(caller: &str, id: &str, text: &str) {
    crate::log::info("agents", &format!("send from {} to {} held: the user is typing or has a draft, or other text is going in", label(caller), label(id)));
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
fn held_sender() {
    loop {
        std::thread::sleep(NOTIFY_EVERY);
        let pty = crate::pty::manager();
        resume_chats(&pty);
        deliver_held(&pty);
    }
}

/// Whether Luna means to carry this chat on by itself and has not yet: the text
/// held for it goes in with that (`resume_chat`), and what it said so far is no
/// answer while the rest is on its way.
fn resume_pending(id: &str) -> bool {
    reg().lock().unwrap_or_else(|e| e.into_inner()).resumes.contains_key(id)
}

/// It is off: the caller hears the plain "did NOT finish" (`notice_line`), and
/// what was held for it goes in as any held text does.
fn call_off_resume(id: &str, why: &str) {
    if reg().lock().unwrap_or_else(|e| e.into_inner()).resumes.remove(id).is_some() {
        crate::log::warn("agents", &format!("{}: not carried on by itself after a core stopped its turn: {why}", label(id)));
    }
}

/// The chats the window is to start for this (`resume_chats`): a chat's CLI
/// otherwise starts as a pane attaches, and a helper usually sits in none.
#[tauri::command]
pub fn agent_resumes() -> Vec<String> {
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    r.resumes.keys().filter(|id| r.known.get(*id).is_some_and(|k| k.waiting)).cloned().collect()
}

/// Where a chat to carry on stands: Ok(true) ready now, Ok(false) not yet, Err
/// why it is off. `ready`: its CLI stands at an empty prompt, between turns with
/// nothing up. Given as long as a chat is to come back at all, then as
/// long as a caller is: Claude Code can take minutes to draw.
fn resume_look(waiting: bool, up_ms: u64, alive: bool, typed: bool, ready: bool) -> Result<bool, &'static str> {
    if waiting {
        return if up_ms < SESSION_RETURN_MS { Ok(false) } else { Err("the window has not started it") };
    }
    if !alive {
        return Err("its CLI did not stay up");
    }
    if typed {
        return Err("a person has typed into it");
    }
    if up_ms >= CALLER_RETURN_MS {
        return Err("its CLI has not come to its prompt");
    }
    Ok(ready)
}

/// One look at every chat to carry on, typing it in where it has stood ready for
/// two looks in a row (`NOTIFY_IDLE_LOOKS`, as for a notice).
fn resume_chats(pty: &crate::pty::PtyManager) {
    let ids: Vec<String> = reg().lock().unwrap_or_else(|e| e.into_inner()).resumes.keys().cloned().collect();
    for id in ids {
        let waiting = reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(&id).map_or(true, |k| k.waiting);
        let alive = pty.alive(&id);
        let ready = alive
            && crate::activity::turn_state(&id).is_some_and(|t| t.turn == crate::activity::Turn::Idle)
            && pty.at_prompt(&id)
            && !ask_of(&id).is_some_and(|a| a.blocking);
        match resume_look(waiting, core_up_ms(), alive, pty.typed_ago_ms(&id).is_some(), ready) {
            Err(why) => call_off_resume(&id, why),
            Ok(ready) => {
                let looks = {
                    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
                    let Some(res) = r.resumes.get_mut(&id) else { continue };
                    res.looks = if ready { res.looks + 1 } else { 0 };
                    res.looks
                };
                if looks >= NOTIFY_IDLE_LOOKS {
                    resume_chat(pty, &id);
                }
            }
        }
    }
}

/// What a chat is typed to go on: with a conversation (`resuming`), why it stopped
/// and what never reached it, as a background helper is told; with none, its
/// opening prompt and that. None with neither to go on.
fn resume_words(resuming: bool, opening: Option<String>, batch: &[crate::background::Queued]) -> Option<String> {
    if resuming {
        return Some(crate::background::resume_text(batch));
    }
    let first = crate::background::Queued { from: None, text: opening? };
    Some(crate::background::joined(&[std::slice::from_ref(&first), batch].concat()))
}

/// Whether text a core went in the middle of typing is in the conversation the CLI
/// came back on: what is there it took, and what is not it did not, the CLI that
/// was to take it having gone with the core. A new conversation has none of it.
#[derive(Debug, PartialEq)]
enum WentIn {
    Yes,
    No,
    CannotTell,
}

fn went_in(pty: &crate::pty::PtyManager, id: &str, g: &GoingIn) -> WentIn {
    let Some(p) = pty.probe(id) else { return WentIn::CannotTell };
    if p.resume.is_none() {
        return WentIn::No;
    }
    let Some(path) = transcript_path(pty, id).filter(|path| path.is_file()) else { return WentIn::CannotTell };
    said_to(&messages_at(&path, p.provider, g.cursor), &g.needle)
}

/// Whether anyone told the session this, in what a transcript holds.
fn said_to(messages: &[Message], needle: &str) -> WentIn {
    if messages.iter().any(|m| m.role == "user" && words(&m.text).contains(needle)) {
        WentIn::Yes
    } else {
        WentIn::No
    }
}

/// Text found on its way in (`Held::going_in`), its session back: what arrived is
/// not typed again, its sender owed the end of a turn that went with the core.
/// What did not goes in as any held text. What cannot be told is not typed again
/// either, and the sender is told: twice is what an agent cannot take back.
fn reconcile_held(pty: &crate::pty::PtyManager, id: &str) {
    let marked: Vec<(String, String, GoingIn)> = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        r.held.iter().filter(|h| h.id == id).filter_map(|h| Some((h.caller.clone(), h.text.clone(), h.going_in.clone()?))).collect()
    };
    if marked.is_empty() {
        return;
    }
    for (caller, text, g) in marked {
        let went = went_in(pty, id, &g);
        {
            let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
            let at = r.held.iter().position(|h| h.id == id && h.caller == caller && h.text == text && h.going_in.is_some());
            if let Some(at) = at {
                if went == WentIn::No {
                    r.held[at].going_in = None;
                } else {
                    r.held.remove(at);
                }
            }
        }
        crate::log::warn("agents", &format!("text from {} to {} was going in as a core went: {went:?}", label(&caller), label(id)));
        if went != WentIn::No {
            owe_cut_short(&caller, id, g.cursor, went == WentIn::CannotTell);
        }
    }
    crate::background::save();
}

/// `caller`'s text went in and its turn went with a core: as `take_up_chats` has it
/// for a debt known as the core went, found out only now.
fn owe_cut_short(caller: &str, id: &str, cursor: u64, unsure: bool) {
    let count = crate::activity::turns_counted(id).unwrap_or(0);
    seen(caller, id, count);
    owe_from(caller, id, count, cursor);
    if let Some(o) = reg().lock().unwrap_or_else(|e| e.into_inner()).owed.get_mut(&(caller.to_string(), id.to_string())) {
        o.cut_short = true;
        o.resumed = false;
        o.late = true;
        o.unsure = unsure;
    }
    crate::activity::count_at_least(id, count.saturating_add(1));
}

/// Marks what is held for `id` as on its way in, as `typed`, on disk before it goes:
/// a core gone before it is taken off leaves the next one to tell (`reconcile_held`).
/// With `only` just that one, `Held::since` naming it.
fn mark_going_in(id: &str, typed: &str, cursor: u64, only: Option<(&str, u64)>) {
    {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let mut one = only.is_some();
        for h in r.held.iter_mut().filter(|h| h.id == id) {
            if let Some((caller, since)) = only {
                if h.caller != caller || h.since != since || !one {
                    continue;
                }
                one = false;
            }
            h.going_in = Some(GoingIn { needle: needle_of(typed, &h.text), cursor });
        }
    }
    crate::background::save();
}

/// It did not go in after all.
fn unmark_going_in(id: &str) {
    for h in reg().lock().unwrap_or_else(|e| e.into_inner()).held.iter_mut().filter(|h| h.id == id) {
        h.going_in = None;
    }
}

/// Types in the text that carries a chat on, with what was held for it. A person
/// or a `send` that has got there first has taken it off the list by now (as
/// `resume` finds under its lock), and any `send` after this finds the turn it
/// starts and queues behind it. Its callers are owed the end of that turn, and told
/// it is running; a text that does not go in leaves them the plain notice.
fn resume_chat(pty: &crate::pty::PtyManager, id: &str) {
    let Some(typing) = TypingInto::claim(id) else { return };
    // No key of a person's lands from here to the Enter, so what this looks at now
    // is what the text goes in on. What changed since the last look is looked at again.
    let Some(hold) = pty.hold_keys(id) else { return };
    if pty.typed_ago_ms(id).is_some() {
        return call_off_resume(id, "a person has typed into it");
    }
    let before = crate::activity::turn_state(id);
    if before.map(|t| t.turn) != Some(crate::activity::Turn::Idle) || !pty.at_prompt(id) {
        if let Some(res) = reg().lock().unwrap_or_else(|e| e.into_inner()).resumes.get_mut(id) {
            res.looks = 0;
        }
        return;
    }
    reconcile_held(pty, id);
    let resuming = pty.probe(id).is_some_and(|p| p.resume.is_some());
    let (opening, batch) = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let Some(res) = r.resumes.get(id) else { return };
        let batch: Vec<crate::background::Queued> =
            r.held.iter().filter(|h| h.id == id).map(|h| crate::background::Queued { from: Some(h.caller.clone()), text: h.text.clone() }).collect();
        // As its first start would have had it: Codex has nowhere else to hear its note.
        let noted = r.known.get(id).filter(|k| k.provider == Provider::Codex).and_then(|k| k.parent.clone());
        let opening = res.opening.as_deref().map(|text| noted.as_deref().map_or(text.to_string(), |parent| crate::pty::codex_helper_prompt(parent, text)));
        (opening, batch)
    };
    let Some(text) = resume_words(resuming, opening, &batch) else {
        return call_off_resume(id, "it came back with no conversation, and its prompt was not kept");
    };
    // Counted once `reconcile_held` has had its say.
    let count = crate::activity::turn_state(id).map_or(0, |t| t.turns_ended);
    let cursor = transcript_end(id);
    let typed = spill(&text);
    mark_going_in(id, &typed, cursor, None);
    let started = match type_held(pty, id, &typed, before.map(|t| t.turn), count, hold) {
        Ok(Some(started)) => started,
        Ok(None) => {
            unmark_going_in(id);
            return call_off_resume(id, "a person's draft is in the way");
        }
        Err(e) => {
            unmark_going_in(id);
            return call_off_resume(id, &e);
        }
    };
    // Each sender's text went in as `deliver_held` would have typed it.
    let mut senders: Vec<&str> = batch.iter().filter_map(|q| q.from.as_deref()).collect();
    senders.dedup();
    for sender in senders {
        seen(sender, id, count);
        owe_from(sender, id, count, cursor);
        if let Some(o) = reg().lock().unwrap_or_else(|e| e.into_inner()).owed.get_mut(&(sender.to_string(), id.to_string())) {
            o.late = true;
        }
    }
    {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let mut typed_in = batch.len();
        r.held.retain(|h| {
            let went = h.id == id && typed_in > 0;
            typed_in -= usize::from(went);
            !went
        });
        // In one hold with its leaving the list, or the notifier could tell the
        // plain notice in between.
        if started {
            resumed_in(&mut r, id, count);
        }
        r.resumes.remove(id);
        // Given what it was asked just now, if it had no conversation.
        if let Some(o) = r.openings.get_mut(id) {
            o.lost = false;
        }
    }
    drop(typing);
    crate::background::save();
    if started {
        crate::log::warn("agents", &format!("{}: carried on by itself after a core stopped its turn, {} held message(s) with it", label(id), batch.len()));
    } else {
        crate::log::warn("agents", &format!("{}: Luna typed in what carries it on and no turn began", label(id)));
    }
}

/// Types in the sends held for people at their keys (`hold`), each once its
/// session is clear, in the order they came; tells the caller of a session
/// that went before its text could, and the desktop of text a draft has held
/// for long.
fn deliver_held(pty: &crate::pty::PtyManager) {
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
        if still_all_lost(pty, &caller, &id) && reg().lock().unwrap_or_else(|e| e.into_inner()).owed.contains_key(&(caller.clone(), id.clone())) {
            continue;
        }
        if !live(pty, &id) {
            // Kept across a core: it goes in once the window has started it.
            if reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(&id).is_some_and(|k| returning(k, core_up_ms())) {
                continue;
            }
            drop_it(&mut *reg().lock().unwrap_or_else(|e| e.into_inner()));
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
            let going = spill(&text);
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
                drop_it(&mut *r);
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
                    if let Some(h) = reg().lock().unwrap_or_else(|e| e.into_inner()).held.iter_mut().find(|h| h.id == id && h.since == since) {
                        h.told = true;
                    }
                    let chat = chat_name(&id).unwrap_or_else(|| id.clone());
                    let from = chat_name(&caller).unwrap_or_else(|| caller.clone());
                    crate::power::notify_chat(
                        &id,
                        "A message is waiting",
                        &format!("\"{chat}\" has an unsent draft, so Luna is holding a message from \"{from}\" until you send or clear it."),
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
fn is_held(caller: &str, id: &str) -> bool {
    reg().lock().unwrap_or_else(|e| e.into_inner()).held.iter().any(|h| h.caller == caller && h.id == id)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Answered {
    pub ok: bool,
    /// Where it stands now (see `state_of`): a dialog that is still up, or
    /// the next question of several, shows here and in `screen`.
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ask: Option<Ask>,
    /// The `afterTurn` for the `wait` that collects the reply.
    pub turns_ended: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screen: Option<String>,
}

/// One press for a terminal: text as typed, or a key's escape sequence.
#[derive(Debug, PartialEq)]
enum Key {
    Text(String),
    Seq(&'static str),
}

/// `{enter}`, `{esc}`, `{down}` … between literal text, as `luna_answer`'s
/// `keys` take them.
fn parse_keys(spec: &str) -> Result<Vec<Key>, String> {
    let mut out = Vec::new();
    let mut rest = spec;
    while !rest.is_empty() {
        let Some(open) = rest.find('{') else {
            out.push(Key::Text(rest.to_string()));
            break;
        };
        if open > 0 {
            out.push(Key::Text(rest[..open].to_string()));
        }
        let close = rest[open..].find('}').ok_or("a { without its }")? + open;
        let seq = match rest[open + 1..close].trim().to_lowercase().as_str() {
            "enter" => "\r",
            "esc" => "\x1b",
            "tab" => "\t",
            "shift+tab" => "\x1b[Z",
            "up" => "\x1b[A",
            "down" => "\x1b[B",
            "right" => "\x1b[C",
            "left" => "\x1b[D",
            "shift+left" => "\x1b[1;2D",
            "shift+right" => "\x1b[1;2C",
            "space" => " ",
            "backspace" => "\x7f",
            "ctrl+c" => "\x03",
            other => return Err(format!("unknown key {{{other}}}")),
        };
        out.push(Key::Seq(seq));
        rest = &rest[close + 1..];
    }
    Ok(out)
}

/// The presses that answer `ask` in a CLI's terminal: an option is reached
/// with the arrows from the first one and taken with Enter; words of its own
/// go in the free-text row that follows the options; a permission is
/// Enter on its first choice (yes) or Esc (no, with the note typed after as
/// what to do instead).
fn presses_for(provider: Provider, ask: &Ask, answers: &[String], allow: Option<bool>, note: Option<&str>) -> Result<Vec<Key>, String> {
    let mut keys = Vec::new();
    if ask.kind == "permission" {
        match allow {
            Some(true) => keys.push(match provider {
                Provider::Claude => Key::Seq("\r"),
                Provider::Codex => Key::Text("y".into()),
            }),
            Some(false) => {
                keys.push(Key::Seq("\x1b"));
                if let Some(n) = note.map(str::trim).filter(|n| !n.is_empty()) {
                    keys.push(Key::Text(n.to_string()));
                    keys.push(Key::Seq("\r"));
                }
            }
            None => return Err("this is a permission: answer with allow true or false".into()),
        }
        return Ok(keys);
    }
    if answers.len() != ask.questions.len() {
        return Err(format!("{} question(s), {} answer(s): give one answer per question", ask.questions.len(), answers.len()));
    }
    for (q, a) in ask.questions.iter().zip(answers) {
        let picked = resolve_option(q, a);
        if let Some(at) = q.list_at {
            let i = q.options.iter().position(|o| *o == picked).ok_or_else(|| {
                format!("pick one of the options of \"{}\", by number or label; it takes no words of one's own", q.text)
            })?;
            let (key, n) = if i >= at { ("\x1b[B", i - at) } else { ("\x1b[A", at - i) };
            keys.extend(std::iter::repeat_with(|| Key::Seq(key)).take(n));
            keys.push(Key::Seq("\r"));
            continue;
        }
        match q.options.iter().position(|o| *o == picked) {
            Some(i) => keys.extend(std::iter::repeat_with(|| Key::Seq("\x1b[B")).take(i)),
            None => {
                keys.extend(std::iter::repeat_with(|| Key::Seq("\x1b[B")).take(q.options.len()));
                keys.push(Key::Text(picked));
            }
        }
        keys.push(Key::Seq("\r"));
    }
    // Several questions end on a review of the answers, to submit.
    if ask.questions.len() > 1 && provider == Provider::Claude {
        keys.push(Key::Seq("\r"));
    }
    Ok(keys)
}

/// Writes presses one at a time, far enough apart that the CLI takes each
/// as a key and not a burst of paste (see ENTER_GAP).
fn press(pty: &crate::pty::PtyManager, id: &str, keys: &[Key]) -> Result<(), String> {
    for k in keys {
        match k {
            Key::Text(t) => {
                pty.write(id, t.as_bytes())?;
                std::thread::sleep(ENTER_GAP);
            }
            Key::Seq(s) => {
                pty.write(id, s.as_bytes())?;
                std::thread::sleep(KEY_GAP);
            }
        }
    }
    Ok(())
}

/// How long an interrupted turn gets to read as over before the text goes in.
const INTERRUPT_SETTLE: std::time::Duration = std::time::Duration::from_secs(3);
/// Between two keys pressed for an answer.
const KEY_GAP: std::time::Duration = std::time::Duration::from_millis(150);
/// How long after an answer its effect is looked at.
const ANSWER_SETTLE: std::time::Duration = std::time::Duration::from_millis(1500);

/// `luna_answer`: answers what the session waits on — a question or a
/// permission (`ask_of`), or with `keys`, any dialog on its screen.
pub fn answer(
    caller: &str,
    id: &str,
    ask_id: Option<&str>,
    answers: &[String],
    allow: Option<bool>,
    note: Option<&str>,
    keys: Option<&str>,
) -> Result<Answered, String> {
    let known = authorize_send(caller, id)?;
    let pty = crate::pty::manager();
    if !live(&pty, id) {
        return Err(format!("session {id} is not running"));
    }
    // Keys in the middle of a notice's paste would land in it: its turn first,
    // and what is to be answered as it stands after that. A background helper
    // has no terminal.
    let terminal = if crate::background::has(id) {
        None
    } else {
        Some(TypingInto::wait_for(id, TYPING_WAIT).ok_or_else(|| format!("Luna is still typing into session {id}; try again in a little while"))?)
    };
    let baseline = crate::activity::turn_state(id).map(|t| t.turns_ended).unwrap_or(0);
    let ask = ask_of(id);
    // What the session says on the answer starts right away, long before
    // the call returns.
    let cursor = transcript_end(id);
    if crate::background::has(id) {
        if keys.is_some() {
            return Err(
                "a background helper has no terminal: answer with answers or allow, or stop its turn with luna_send interrupt: true".into(),
            );
        }
        crate::background::answer(id, ask_id, answers, allow, note)?;
    } else {
        // A person's keys between Luna's would land in the dialog or the answer.
        let hold = pty.hold_keys(id).ok_or("no such session")?;
        if pty.has_draft(id) || pty.typed_ago_ms(id).is_some_and(|ms| ms < SEND_TYPING_MS) {
            return Err(format!("the user is typing in session {id}; try again in a little while"));
        }
        match (keys, &ask) {
            (Some(k), _) => press(&pty, id, &parse_keys(k)?)?,
            // An async question holds no dialog: the answer is a message.
            (None, Some(a)) if !a.blocking => {
                let text: Vec<String> =
                    a.questions.iter().zip(answers).map(|(q, ans)| format!("\"{}\": {}", q.text, resolve_option(q, ans))).collect();
                if text.is_empty() {
                    return Err("give one answer per question".into());
                }
                let before = crate::activity::turn_state(id);
                type_in(&pty, id, &format!("Answers to your question: {}", text.join("; ")), before.map(|t| t.turn), baseline, hold)?;
            }
            (None, Some(a)) => press(&pty, id, &presses_for(known.provider, a, answers, allow, note)?)?,
            (None, None) => {
                return Err(format!(
                    "session {id} is not on a question or permission Luna can read; luna_read with screen: true shows its \
                     terminal, and keys answers any dialog"
                ))
            }
        }
        reg().lock().unwrap_or_else(|e| e.into_inner()).answered.insert((caller.to_string(), id.to_string()), now_ms());
    }
    drop(terminal);
    std::thread::sleep(ANSWER_SETTLE);
    seen(caller, id, baseline);
    owe_from(caller, id, baseline, cursor);
    let (state, ask) = state_of(id);
    let screen = (!crate::background::has(id)).then(|| screen_of(&pty, id)).flatten();
    Ok(Answered { ok: true, state, ask, turns_ended: baseline, screen })
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
fn transcripts() -> &'static Mutex<HashMap<String, std::path::PathBuf>> {
    static T: OnceLock<Mutex<HashMap<String, std::path::PathBuf>>> = OnceLock::new();
    T.get_or_init(Default::default)
}

fn transcript_path(pty: &crate::pty::PtyManager, id: &str) -> Option<std::path::PathBuf> {
    let found = find_transcript(pty, id);
    let mut known = transcripts().lock().unwrap_or_else(|e| e.into_inner());
    match found {
        Some(path) => {
            known.insert(id.to_string(), path.clone());
            Some(path)
        }
        None => known.get(id).cloned(),
    }
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
fn transcript_found(pty: &crate::pty::PtyManager, id: &str) -> bool {
    transcript_path(pty, id).is_some_and(|p| p.is_file())
}

/// Why a session's last turn stopped short, if it did: an API error, a usage
/// limit, a CLI that quit on an error or was killed. On one line, as it goes
/// into a notice. A background helper knows from its process and its events;
/// a Claude Code chat from its transcript's last word and its `StopFailure`
/// hook; a Codex chat from its rollout; and either from its exit code.
fn failure_of(pty: &crate::pty::PtyManager, id: &str, provider: Provider) -> Option<String> {
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
    let path = transcript_path(&pty, id);
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
    let done = ts.map_or(true, |t| t.turn == crate::activity::Turn::Idle && !t.children_busy);
    let settles = whole
        && done
        && !resume_pending(id)
        && owed.is_some_and(|o| turns_ended > o.baseline && (!o.queued || queue_settled(&key, o, turns_ended)));
    // Ahead of the hand-over, which takes it off. With an answer still to come
    // it stays for the notice; with none owed this read is all that tells it.
    let undelivered = undelivered_note(caller, id);
    if undelivered.is_some() && lost_whole(&pty, caller, id) {
        // No answer is to come: this was all of it, and no notice is to say it again.
        // Cancelled first, it and the debt are put back together.
        call.commit(hand_over(caller, id, lost_count(caller, id), None));
    } else if settles || (undelivered.is_some() && owed.is_none()) {
        // Cancelled first, it is put back; the response goes nowhere anyway.
        call.commit(hand_over(caller, id, turns_ended, None));
    } else {
        seen(caller, id, turns_ended);
    }
    Ok(Reading {
        transcript_missing: messages.is_empty() && !transcript_found(&pty, id),
        messages,
        cursor,
        left_out,
        state,
        ask,
        turns_ended,
        screen: with_screen.then(|| screen_of(&pty, id)).flatten(),
        undelivered,
    })
}

/// Whether a turn end at `turns_ended` answers a question typed in mid-turn
/// (`Owed::queued`) by now: a turn past the one it was typed into has ended,
/// or this end has stood QUEUED_GRACE_MS with no turn begun on the text.
/// Notes when the end was first seen, for the next look.
fn queue_settled(key: &(String, String), o: Owed, turns_ended: u32) -> bool {
    if turns_ended >= o.baseline.saturating_add(2) {
        return true;
    }
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let Some(owed) = r.owed.get_mut(key) else { return false };
    match owed.held {
        Some((n, at)) if n == turns_ended => now_ms().saturating_sub(at) >= QUEUED_GRACE_MS,
        _ => {
            owed.held = Some((turns_ended, now_ms()));
            false
        }
    }
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Waited {
    pub outcome: &'static str,
    /// See `state_of`.
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ask: Option<Ask>,
    pub turns_ended: u32,
    /// On `turn_done`, and on `exit` after such a turn: all the session said
    /// since the caller asked it something, as `reply_of` puts it, so no
    /// `read` is needed. On `job_done`: what it said since it was announced as
    /// not done, the same as that notice would bring.
    pub reply: Option<String>,
    /// The turn did not end by itself: it stopped on this error (`failure_of`),
    /// and `reply` is what it said before that, if anything.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed: Option<String>,
    /// Luna found no transcript, so a null `reply` says nothing about whether
    /// it replied.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub transcript_missing: bool,
    /// On `timeout`, `waiting`, a turn that ended on an error or without a
    /// reply: the terminal's last lines, so the caller sees what the session
    /// is on — a permission prompt, a question, an error, a notice nobody
    /// dismissed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screen: Option<String>,
    /// See `undelivered_note`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub undelivered: Option<String>,
    /// On `job_done`, the debt that end settles (`take_debt`), for whichever
    /// hands it over: this `wait`, or a `wait_many` once it has its response.
    #[serde(skip)]
    job_debt: Option<u64>,
}

/// Blocks until the session reaches `until`, or `timeout_s` passes:
/// `turn_done` — a turn ended after `after_turn` (default: the count the
/// caller's question still unanswered went in at, else the count it was
/// last shown, so the reply to a prompt it just sent counts even if it
/// landed before this call, and a turn it has already read does not — or
/// the turn stopped on a prompt, which nothing but an answer moves on, or,
/// told it is not done, the job it left running ended, outcome `job_done`);
/// `waiting` — it stopped for a permission; `exit` — the process ended.
/// A wait its caller let go (`call` cancelled, or see `let_go`) returns an
/// error and hands nothing over: the answer stays owed, for the notice.
pub fn wait(
    caller: &str,
    id: &str,
    until: &str,
    timeout_s: u64,
    after_turn: Option<u32>,
    call: &crate::mcp::Call,
) -> Result<Waited, String> {
    wait_for(caller, id, until, timeout_s, after_turn, call, true).map(|(waited, _)| waited)
}

/// The wait itself, and whether what it returns answers what was asked. With
/// `alone` that answer is handed over here, the reply cut to fit a result; a
/// `wait_many` hands over and cuts for all its sessions at once, when it has
/// its response (`alone` false).
fn wait_for(
    caller: &str,
    id: &str,
    until: &str,
    timeout_s: u64,
    after_turn: Option<u32>,
    call: &crate::mcp::Call,
    alone: bool,
) -> Result<(Waited, bool), String> {
    if !matches!(until, "turn_done" | "waiting" | "exit") {
        return Err(format!("unknown until {until:?}; use turn_done, waiting or exit"));
    }
    let known = authorize_view(caller, id)?;
    let _out = WaitOut::start(caller, id);
    let key = (caller.to_string(), id.to_string());
    let holding = let_go(caller);
    let gone = || call.cancelled() || holding() == Holding::No;
    let gone_err = || -> Result<(Waited, bool), String> { Err(LET_GO.into()) };
    let pty = crate::pty::manager();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_s.clamp(1, 1800));
    // A first turn lost is all there is to wait for, and what Luna holds of the
    // caller's stays for it (`deliver_held`), so nothing below would ever see it through.
    // An `exit` waits for the session itself.
    let lost = || (until == "turn_done" || !live(&pty, id)) && still_all_lost(&pty, caller, id) && holding() != Holding::Unsure;
    // Text Luna holds for a person's draft goes in first; the turn to wait
    // for is the one it starts, past whatever the person sent meanwhile. So does
    // what carries it on after a core stopped its turn.
    while is_held(caller, id) || resume_pending(id) {
        if gone() {
            return gone_err();
        }
        if lost() {
            return lost_answer(caller, id, call, alone);
        }
        if std::time::Instant::now() >= deadline {
            let (state, ask) = state_of(id);
            let turns_ended = crate::activity::turn_state(id).map_or(0, |t| t.turns_ended);
            let screen = screen_of(&pty, id);
            return Ok((Waited { outcome: "timeout", state, ask, turns_ended, reply: None, failed: None, transcript_missing: false, screen, undelivered: None, job_debt: None }, false));
        }
        std::thread::sleep(POLL);
    }
    let now = crate::activity::turn_state(id).map(|t| t.turns_ended);
    let (owed_since, answered_at, queued, late, told_debt) = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let owed = r.owed.get(&key);
        (
            owed.map(|o| o.baseline),
            r.answered.get(&key).copied(),
            owed.is_some_and(|o| o.queued),
            owed.is_some_and(|o| o.late),
            owed.filter(|o| o.told_pending).map(|o| o.rev),
        )
    };
    let after_turn = if late { None } else { after_turn };
    // Told it is not done, a wait is for what a notice would bring next: the
    // job's end or a turn no notice started. A count asked for is that count.
    let told_debt = told_debt.filter(|_| after_turn.is_none());
    let told_pending = told_debt.is_some();
    // A count shown since the question (a read of part of the answer, a
    // timeout) does not make the answer seen.
    let mut baseline = match (after_turn, owed_since.or_else(|| last_shown(caller, id)), now) {
        (Some(n), _, _) => n,
        // A count the session is behind has been left over from before it
        // was respawned; waiting past it would sit out the whole timeout.
        (None, Some(seen), Some(now)) => seen.min(now),
        (None, Some(seen), None) => seen,
        (None, None, now) => now.unwrap_or(0),
    };
    let mut looks = 0u32;
    // The turn count a turn end was first seen at, and when (QUEUED_GRACE_MS).
    let mut quiet: Option<(u32, u64)> = None;
    loop {
        if gone() {
            return gone_err();
        }
        if lost() {
            return lost_answer(caller, id, call, alone);
        }
        let alive = live(&pty, id);
        let ts = crate::activity::turn_state(id);
        let turns_ended = ts.map(|t| t.turns_ended).unwrap_or(0);
        let turn = ts.map(|t| t.turn);
        let mut ended = turns_ended > baseline && turn != Some(crate::activity::Turn::Busy);
        // A job's end that has stood its grace (`job_end_stood`).
        let mut job_done = false;
        if told_pending {
            let waits = waits_on_helpers(id, caller);
            let job = job_at_work(id, ts, alive) || waits;
            // What it has said since the caller last heard, as a notice would go by.
            let upward = || {
                let cursor = reg().lock().unwrap_or_else(|e| e.into_inner()).owed.get(&key).map(|o| o.cursor);
                turn != Some(crate::activity::Turn::Busy) && cursor.is_some_and(|at| news_for_parent(&pty, caller, id, &known, at))
            };
            let now = verdict(&Owed::pending(baseline, 0), turns_ended, turn, job, real_turn_past(id, caller, baseline, turns_ended, waits, upward));
            if now == Verdict::OnlyNotices {
                baseline = turns_ended;
            }
            ended = now == Verdict::TurnOver;
            job_done = job_end_stood(&key, turns_ended, now == Verdict::JobOver && alive);
        }
        if !ended {
            quiet = None;
        } else if queued {
            let since = match quiet {
                Some((n, at)) if n == turns_ended => at,
                _ => {
                    let at = now_ms();
                    quiet = Some((turns_ended, at));
                    at
                }
            };
            ended = now_ms().saturating_sub(since) >= QUEUED_GRACE_MS;
        }
        // A question in the transcript holds the turn whatever the turn
        // word says; read for it every few looks, not every one.
        looks += 1;
        let question = looks % 4 == 1 && ask_of(id).is_some_and(|a| a.blocking && a.kind == "question");
        // A turn stopped on a prompt will not end by itself: `turn_done`
        // hands it back too, unless it is the prompt the caller has just
        // answered and the sampler has not caught up with yet.
        let asking = (turn == Some(crate::activity::Turn::Waiting) || question)
            && answered_at.map_or(true, |at| now_ms().saturating_sub(at) >= ANSWERED_MS);
        let done = match until {
            "exit" => !alive,
            "waiting" => turn == Some(crate::activity::Turn::Waiting) || question || !alive,
            _ => ended || job_done || asking || !alive,
        };
        if done && holding() != Holding::Unsure {
            let outcome = if !alive {
                "exit"
            } else if until == "waiting" || (asking && !ended && !job_done) {
                "waiting"
            } else if job_done && !ended {
                "job_done"
            } else {
                "turn_done"
            };
            // A turn it answered in before it exited is handed over too.
            let says = outcome == "turn_done" || (outcome == "exit" && turns_ended > baseline);
            // A job's end is what the caller was owed, with no turn to wait
            // for the words of: what it said since is there already.
            let answers = says || outcome == "job_done";
            // Read from where the caller's question found the transcript.
            let (asked_at, debt_cut) = {
                let r = reg().lock().unwrap_or_else(|e| e.into_inner());
                r.owed.get(&key).map_or((None, false), |o| (Some(o.cursor), o.cut_short))
            };
            // A core's going stopped the turn (the helper's word or the debt's, a
            // chat brought back): as the notice says it, and with what it said.
            let cut = says && (crate::background::cut_short(id) || debt_cut);
            let failure = || {
                (answers || outcome == "exit")
                    .then(|| failure_of(&pty, id, known.provider))
                    .flatten()
                    .or_else(|| cut.then(|| "it did NOT finish its turn: Luna's core restarted while it worked and stopped it".to_string()))
            };
            let mut failed = failure();
            // A turn that stopped on an error has said all it will.
            let reply_now = |stopped: bool| reply_of(&pty, id, known.provider, asked_at, alone, stopped);
            let mut reply = if outcome == "job_done" {
                asked_at.and_then(|at| reply_text(id, &said_since(&pty, id, known.provider, at), alone))
            } else if says {
                reply_now(failed.is_some())
            } else {
                None
            };
            let late = std::time::Instant::now() + std::time::Duration::from_millis(REPLY_GRACE_MS);
            while says && reply.is_none() && std::time::Instant::now() < late && !gone() {
                std::thread::sleep(POLL);
                failed = failed.or_else(failure);
                reply = reply_now(failed.is_some());
            }
            // In doubt again after the grace, it is the notice's to bring.
            if gone() || (answers && holding() == Holding::Unsure) {
                return gone_err();
            }
            let failed = failed.or_else(failure);
            let transcript_missing = says && reply.is_none() && !transcript_found(&pty, id);
            let bare = outcome == "turn_done" && (reply.is_none() || failed.is_some());
            let screen = (outcome == "waiting" || bare).then(|| screen_of(&pty, id)).flatten();
            // Ahead of the hand-over, which takes it off.
            let undelivered = if answers { undelivered_note(caller, id) } else { None };
            let job_debt = told_debt.filter(|_| outcome == "job_done");
            if !answers {
                // Nothing was handed over: whatever is owed stays owed.
                looked(id);
            } else if alone && !call.commit(hand_over(caller, id, turns_ended, job_debt)) {
                // A cancel since the last look: it stays owed after all.
                return gone_err();
            }
            let (state, ask) = state_of(id);
            return Ok((Waited { outcome, state, ask, turns_ended, reply, failed, transcript_missing, screen, undelivered, job_debt }, answers));
        }
        if std::time::Instant::now() >= deadline {
            let screen = screen_of(&pty, id);
            // Nothing was handed over: whatever is owed stays owed, and the
            // next wait still waits for it.
            looked(id);
            let (state, ask) = state_of(id);
            return Ok((Waited { outcome: "timeout", state, ask, turns_ended, reply: None, failed: None, transcript_missing: false, screen, undelivered: None, job_debt: None }, false));
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

/// What a `wait` on several sessions says of one of them.
#[derive(Serialize)]
pub struct WaitedOne {
    pub id: String,
    #[serde(flatten)]
    pub waited: Waited,
}

#[derive(Serialize)]
pub struct WaitedMany {
    /// `finished`, or `timeout` when the time ran out first.
    pub outcome: &'static str,
    /// The sessions that got to where they were waited for, in the order
    /// asked for: what they say is in `results`, handed over as by one wait.
    pub finished: Vec<String>,
    pub results: Vec<WaitedOne>,
}

/// `wait` on several sessions at once, each by a `wait` of its own in a thread
/// of its own, so that what holds a notice back, what a reply covers and what
/// settles a debt are one wait's for each. `all` has them go on until each has
/// finished or the time is up; otherwise the first to finish stops the others,
/// which come back `pending` with whatever they are owed still owed.
///
/// Nothing is handed over until the response is: a session that has finished
/// stays owed, reserved from the notice (`Reserved`), so a cancel, a caller found
/// to have let the call go (`let_go`, which any of the waits may be the one to
/// see) or a core going meanwhile leaves its answer for the notice to bring.
/// Once the response is made they are all handed over together, and a cancel
/// crossing it undoes them together.
pub fn wait_many(
    caller: &str,
    ids: &[String],
    all: bool,
    until: &str,
    timeout_s: u64,
    call: &crate::mcp::Call,
) -> Result<WaitedMany, String> {
    if !matches!(until, "turn_done" | "waiting" | "exit") {
        return Err(format!("unknown until {until:?}; use turn_done, waiting or exit"));
    }
    let mut unique: Vec<&str> = Vec::new();
    for id in ids {
        if !unique.contains(&id.as_str()) {
            unique.push(id);
        }
    }
    if unique.is_empty() {
        return Err("ids is empty".into());
    }
    for id in &unique {
        authorize_view(caller, id)?;
    }
    let _reserved: Vec<Reserved> = unique.iter().map(|id| Reserved::start(caller, id)).collect();
    let calls: Vec<crate::mcp::Call> = unique.iter().map(|_| crate::mcp::Call::new()).collect();
    let mut got: Vec<Option<Result<(Waited, bool), String>>> = unique.iter().map(|_| None).collect();
    let mut let_go_seen = false;
    let mut no_thread: Option<String> = None;
    std::thread::scope(|scope| {
        let (tx, rx) = std::sync::mpsc::channel();
        for (i, id) in unique.iter().copied().enumerate() {
            let (tx, one) = (tx.clone(), &calls[i]);
            let thread = std::thread::Builder::new()
                .name(format!("luna-wait-{id}"))
                .spawn_scoped(scope, move || {
                    let _ = tx.send((i, wait_for(caller, id, until, timeout_s, None, one, false)));
                });
            if let Err(e) = thread {
                no_thread = Some(e.to_string());
            }
        }
        drop(tx);
        let mut stopped = no_thread.is_some();
        if stopped {
            calls.iter().for_each(|c| c.cancel());
        }
        // When the first of `any` came.
        let mut first_at: Option<std::time::Instant> = None;
        loop {
            match rx.recv_timeout(std::time::Duration::from_millis(200)) {
                Ok((i, r)) => {
                    // Not stopped by this call: the caller has let it go.
                    let_go_seen |= !stopped && r.as_ref().is_err_and(|e| e == LET_GO);
                    got[i] = Some(r);
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
            if !all && first_at.is_none() && got.iter().flatten().any(|r| r.as_ref().is_ok_and(|(w, _)| w.outcome != "timeout")) {
                first_at = Some(std::time::Instant::now());
            }
            let enough = first_at.is_some_and(|at| at.elapsed() >= SAME_LOOK);
            if !stopped && (enough || let_go_seen || call.cancelled()) {
                stopped = true;
                calls.iter().for_each(|c| c.cancel());
            }
        }
    });
    if let Some(e) = no_thread {
        return Err(e);
    }
    if let_go_seen || call.cancelled() {
        return Err(LET_GO.into());
    }
    // (session, the count it answered by, the debt a job's end settles), for
    // the hand-over below.
    let mut answers: Vec<(String, u32, Option<u64>)> = Vec::new();
    let mut results: Vec<WaitedOne> = unique
        .iter()
        .copied()
        .zip(got)
        .map(|(id, r)| {
            let waited = match r {
                Some(Ok((w, answered))) => {
                    if answered {
                        answers.push((id.to_string(), w.turns_ended, w.job_debt));
                    }
                    w
                }
                other => {
                    // Stopped for another's answer, nothing handed over. Any
                    // other error is a session gone since it was looked up.
                    let stopped = matches!(&other, Some(Err(e)) if e == LET_GO);
                    let (state, ask) = state_of(id);
                    let turns_ended = crate::activity::turn_state(id).map_or(0, |t| t.turns_ended);
                    looked(id);
                    Waited {
                        outcome: if stopped { "pending" } else { "exit" },
                        state,
                        ask,
                        turns_ended,
                        reply: None,
                        failed: None,
                        transcript_missing: false,
                        screen: None,
                        undelivered: None,
                        job_debt: None,
                    }
                }
            };
            WaitedOne { id: id.to_string(), waited }
        })
        .collect();
    share_replies(&mut results);
    let finished: Vec<String> =
        results.iter().filter(|r| !matches!(r.waited.outcome, "timeout" | "pending")).map(|r| r.id.clone()).collect();
    let outcome = if !finished.is_empty() && (!all || finished.len() == results.len()) { "finished" } else { "timeout" };
    let undo: Vec<_> = answers.iter().map(|(id, turns_ended, job_debt)| hand_over(caller, id, *turns_ended, *job_debt)).collect();
    if !call.commit(move || undo.into_iter().for_each(|u| u())) {
        return Err(LET_GO.into());
    }
    Ok(WaitedMany { outcome, finished, results })
}

/// Holds several replies, whole as `reply_of` gives them to a `wait_many`, to
/// what one tool result carries (MAX_TEXT): the short ones whole, the rest cut to
/// an even share of what is left, each naming a file with the whole of it, as a
/// reply cut by itself does.
fn share_replies(results: &mut [WaitedOne]) {
    let mut order: Vec<usize> = (0..results.len()).filter(|&i| results[i].waited.reply.is_some()).collect();
    let length = |r: &WaitedOne| r.waited.reply.as_ref().map_or(0, |t| t.chars().count());
    order.sort_by_key(|&i| length(&results[i]));
    let mut left = MAX_TEXT;
    let mut others = order.len();
    for i in order {
        let share = left / others;
        others -= 1;
        let n = length(&results[i]);
        if n <= share {
            left -= n;
            continue;
        }
        left -= share;
        let r = &mut results[i];
        if let Some(whole) = r.waited.reply.take() {
            let head: String = whole.chars().take(share).collect();
            r.waited.reply = Some(format!("{head}\n\n[cut at {share} characters, several sessions sharing this result; {}]", answer_file(&r.id, &[whole])));
        }
    }
}

/// Whether the call a `wait` by `caller` answers has been let go without a
/// cancel saying so: the caller is gone, or — a Codex caller, held in the
/// tool call for as long as it lasts — the turn that made the call is over
/// (an interrupt) and no other has begun, for LET_GO_MS on end: a sample
/// read just before the call's turn began, and published after it, shows
/// the turn before over at a count past the one the call found; the next
/// sample finds the caller busy in this one. Until then it is `Unsure`.
/// Claude Code moves a long call to the background and may end its turn
/// while the call runs on, and sends a cancel when it drops one.
fn let_go(caller: &str) -> impl Fn() -> Holding {
    let caller = caller.to_string();
    let codex = reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(&caller).is_some_and(|k| k.provider == Provider::Codex);
    let turns = crate::activity::turn_state(&caller).map(|t| t.turns_ended);
    // Since when the caller's turn has looked over.
    let over_since = std::cell::Cell::new(None::<u64>);
    move || {
        if caller == DEV_CALLER {
            return Holding::Yes;
        }
        if !live(&crate::pty::manager(), &caller) {
            return Holding::No;
        }
        let over = codex
            && turns.is_some_and(|n| {
                crate::activity::turn_state(&caller).is_some_and(|t| t.turns_ended > n && t.turn == crate::activity::Turn::Idle)
            });
        if !over {
            over_since.set(None);
            return Holding::Yes;
        }
        let since = over_since.get().unwrap_or_else(now_ms);
        over_since.set(Some(since));
        if now_ms().saturating_sub(since) >= LET_GO_MS {
            Holding::No
        } else {
            Holding::Unsure
        }
    }
}

/// Whether a caller still holds the `wait` it made (see `let_go`).
#[derive(Clone, Copy, PartialEq)]
enum Holding {
    Yes,
    /// It may have let it go: nothing is handed over until that is known.
    Unsure,
    No,
}

/// What a `wait` hands back: all the session said past `asked_at`, where
/// the caller's question found its transcript, report first (`answer_text`);
/// with no question out, its last message. None when it said nothing, or the
/// prompt is the last word of a turn that has not `stopped` short
/// (`own_words`). `cut` as in `reply_text`.
fn reply_of(pty: &crate::pty::PtyManager, id: &str, provider: Provider, asked_at: Option<u64>, cut: bool, stopped: bool) -> Option<String> {
    let replies = match asked_at {
        Some(at) => replies_since(pty, id, provider, at, stopped),
        None => replies_since(pty, id, provider, 0, false).pop().into_iter().collect(),
    };
    reply_text(id, &replies, cut)
}

/// Messages as one reply in a tool result; None for no messages. `cut` keeps it
/// to what one result carries, naming a file with the whole; a `wait_many`
/// shares that out itself (`share_replies`).
fn reply_text(id: &str, replies: &[String], cut: bool) -> Option<String> {
    if replies.is_empty() {
        return None;
    }
    let text = answer_text(replies);
    if !cut || text.chars().count() <= MAX_TEXT {
        return Some(text);
    }
    let head: String = text.chars().take(MAX_TEXT).collect();
    Some(format!("{head}\n\n[cut at {MAX_TEXT} characters; {}]", answer_file(id, replies)))
}

/// What `kill` did beyond stopping a turn.
#[derive(Serialize, Debug, Default)]
pub struct Killed {
    pub ok: bool,
    /// A background helper between turns: what its turns had left running,
    /// which is what there was to stop.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ended: Vec<String>,
}

/// Stops the session; the chat row stays and can be resumed by hand. A
/// background helper between turns has only what its turns left running to
/// stop (`background::kill`).
pub fn kill(caller: &str, id: &str) -> Result<Killed, String> {
    authorize(caller, id)?;
    let between_turns = crate::background::has(id) && crate::activity::turn_state(id).is_some_and(|t| !t.busy);
    // Stopped is not done with: what it was in the middle of is still there
    // to go on from, so it stays until it is deleted. Between turns it was in the
    // middle of nothing, and goes by itself as before.
    if !between_turns {
        reg().lock().unwrap_or_else(|e| e.into_inner()).auto.remove(id);
    }
    if crate::background::has(id) {
        let ended = crate::background::kill(id);
        return Ok(Killed { ok: true, ended });
    }
    let pty = crate::pty::manager();
    tauri::async_runtime::block_on(pty.kill(id));
    Ok(Killed { ok: true, ..Default::default() })
}

/// Stops the session, removes its attachments and — when asked — the
/// worktree it made (never one it was handed), then tells the frontend to
/// drop the row. Returns the worktree removed, if any. A session's own
/// children go on, passed up to its parent (see `forget`).
///
/// A person's Delete has its dialog to warn of the work in a worktree; an
/// agent's refuses while there is any, unless `force`. Nothing has been touched
/// by the time it does, the session still running if it was.
pub fn delete(caller: &str, id: &str, drop_worktree: bool, force: bool) -> Result<Removed, String> {
    let known = authorize(caller, id)?;
    if drop_worktree && !force {
        let folder = crate::worktree::project_of(&known.folder).unwrap_or_else(|| known.folder.clone());
        if let Some(wt) = worktree_to_drop(id, &folder) {
            if let Some(work) = crate::worktree::unsaved_work(&folder, &wt) {
                return Err(format!(
                    "not deleted: worktree {wt} holds {work}, which removing it would lose (files git ignores do not count, and would go too). \
                     Save what matters (commit and merge, or copy it), or call luna_delete without dropWorktree to keep the worktree, \
                     or with force: true to remove it anyway"
                ));
            }
            // Clean as it stands; a session still at it may yet change that. It
            // is stopped first and looked at again.
            stop_for_good(id);
            if let Some(work) = crate::worktree::unsaved_work(&folder, &wt) {
                return remove(id, known, false, None)
                    .map(|_| Removed::left(Some(wt), format!("kept: it came to hold {work} as the session was stopped")));
            }
            return remove(id, known, true, Some(wt));
        }
    }
    remove(id, known, drop_worktree, None)
}

/// Stops a session and what it has running, its row kept.
fn stop_for_good(id: &str) {
    if crate::background::has(id) {
        crate::background::stop_for_good(id);
    } else {
        tauri::async_runtime::block_on(crate::pty::manager().kill(id));
    }
}

/// The worktree `remove` would take with the session, as it would judge it:
/// one the session made, that it was not handed and no other session is in.
fn worktree_to_drop(id: &str, folder: &str) -> Option<String> {
    if reg().lock().unwrap_or_else(|e| e.into_inner()).borrowed.contains(id) {
        return None;
    }
    let pty = crate::pty::manager();
    let wt = if crate::background::has(id) {
        crate::background::removable_worktree(id)
    } else {
        let probe = pty.probe(id)?;
        let cwd = probe.working_dir().unwrap_or(probe.cwd);
        crate::worktree::is_worktree_of(folder, &cwd).then_some(cwd)
    }?;
    (!pty.worktree_shared(id, &wt)).then_some(wt)
}

/// What a delete did with the session's worktree. The session is gone either
/// way: a worktree that would not go (a file held open) is named, for the
/// caller to deal with, not an error that a retry could only answer with "not
/// one you spawned".
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Removed {
    pub ok: bool,
    pub worktree_removed: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_left: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_error: Option<String>,
    /// What went with a worktree without counting as work, said when one goes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<&'static str>,
}

/// Files git ignores do not stop a delete: build output would stop every one.
const IGNORED_GO: &str = "files git ignores went with it";

impl Removed {
    fn of(worktree_removed: Option<String>) -> Removed {
        let note = worktree_removed.is_some().then_some(IGNORED_GO);
        Removed { ok: true, worktree_removed, worktree_left: None, worktree_error: None, note }
    }

    fn left(worktree: Option<String>, why: String) -> Removed {
        crate::log::warn("delete", &format!("the session is gone, its worktree is not: {why}"));
        Removed { ok: true, worktree_removed: None, worktree_left: worktree, worktree_error: Some(why), note: None }
    }
}

/// `worktree`: the one to drop, where the session may no longer say itself.
fn remove(id: &str, known: Known, drop_worktree: bool, worktree: Option<String>) -> Result<Removed, String> {
    let pty = crate::pty::manager();
    // What `luna_sessions` history has left to go on once it is gone.
    if known.parent.is_some() || known.orphan {
        let transcript = transcript_path(&pty, id);
        let last = transcript.as_ref().and_then(|_| replies_since(&pty, id, known.provider, 0, false).pop());
        crate::agentlog::left(
            id,
            transcript.map(|p| p.to_string_lossy().into_owned()).as_deref(),
            worktree_of(&pty, id).path.as_deref(),
            last.as_deref(),
        );
    }
    // A helper resumed in a worktree knows the worktree as its folder.
    let folder = crate::worktree::project_of(&known.folder).unwrap_or(known.folder);
    let drop_worktree = drop_worktree && !reg().lock().unwrap_or_else(|e| e.into_inner()).borrowed.contains(id);
    if crate::background::has(id) {
        let who = label(id);
        let made = crate::background::remove(id);
        forget(id);
        crate::activity::forget(id);
        crate::log::info("delete", &format!("background helper {who}, worktree {made:?}, dropping {drop_worktree}"));
        return Ok(match made {
            // As for a chat (`PtyManager::delete`): a worktree another
            // session or helper is in stays, its work with it.
            Some(wt) if drop_worktree && pty.worktree_shared(id, &wt) => {
                crate::log::warn("delete", &format!("kept worktree {wt}: another session is in it"));
                Removed::of(None)
            }
            Some(wt) if drop_worktree => match crate::worktree::remove_worktree_now(folder, wt.clone()) {
                Ok(()) => Removed::of(Some(wt)),
                Err(why) => Removed::left(Some(wt), why),
            },
            _ => Removed::of(None),
        });
    }
    // Asked before it goes: its process is what says where it works.
    let at_stake = if drop_worktree { worktree.clone().or_else(|| worktree_to_drop(id, &folder)) } else { None };
    // PtyManager::delete forgets the session here too (see `forget`).
    let deleted = tauri::async_runtime::block_on(pty.delete(id, &folder, worktree, drop_worktree));
    crate::emit::to_ui("agent://deleted", Deleted { id: id.to_string() });
    Ok(match deleted {
        // Without the drop, `delete` names the worktree that stays.
        Ok(worktree) => Removed::of(worktree.filter(|_| drop_worktree)),
        Err(why) => Removed::left(at_stake, why),
    })
}

/// The shortest gap between two notifications from one session: a model in
/// a loop must not bury the desktop in them.
const NOTIFY_GAP_MS: u64 = 10_000;

/// A desktop notification from `caller`, under its own chat's name so it
/// cannot pass for another; a click on it opens that chat.
pub fn notify(caller: &str, text: &str) -> Result<(), String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("text is empty".into());
    }
    let provider = {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let now = now_ms();
        if let Some(last) = r.notified.get(caller) {
            let left = NOTIFY_GAP_MS.saturating_sub(now.saturating_sub(*last));
            if left > 0 {
                return Err(format!("not shown: one notification per {} s, try again in {} s", NOTIFY_GAP_MS / 1000, left.div_ceil(1000)));
            }
        }
        r.notified.insert(caller.to_string(), now);
        r.known.get(caller).map(|k| k.provider)
    };
    let who = match provider {
        Some(Provider::Codex) => "Codex",
        _ => "Claude",
    };
    let chat = chat_name(caller).unwrap_or_else(|| caller.to_string());
    // A toast shows a few lines at most; the rest would only be cut.
    let body: String = text.chars().take(300).collect();
    crate::power::notify_chat(caller, &format!("“{chat}” · {who}"), &body);
    Ok(())
}

/// Deletes the `autoDelete` helpers that are done with: nobody is owed their
/// answer or waiting on them, nothing is owed to them, no helper of theirs is
/// still there, and they have sat idle — or exited — for AUTO_DELETE_IDLE_MS
/// since. One a person has typed into is theirs now and stays. Its worktree
/// stays too, with whatever work is in it.
fn reap(pty: &crate::pty::PtyManager) {
    let ids: Vec<String> = reg().lock().unwrap_or_else(|e| e.into_inner()).auto.keys().cloned().collect();
    for id in ids {
        if pty.typed_ago_ms(&id).is_some() {
            reg().lock().unwrap_or_else(|e| e.into_inner()).auto.remove(&id);
            continue;
        }
        // Between turns with no job left running; what its terminal last drew
        // and when does not matter here.
        let idle = !live(pty, &id)
            || crate::activity::turn_state(&id)
                .is_some_and(|t| t.turn == crate::activity::Turn::Idle && !t.children_busy);
        // Gone, it would take with it what its own helpers are to answer it,
        // and what it would make of that.
        let below = descendants(&reg().lock().unwrap_or_else(|e| e.into_inner()), &id);
        let helpers_there = below.iter().any(|k| live(pty, k));
        let known = {
            let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
            let quiet = !helpers_there
                && !r.owed.keys().any(|(c, s)| *s == id || *c == id)
                && !r.waits.keys().any(|(_, s)| *s == id);
            let known = r.known.get(&id).cloned();
            let Some(a) = r.auto.get_mut(&id) else { continue };
            if !(quiet && idle) {
                a.idle_since = None;
                continue;
            }
            let since = *a.idle_since.get_or_insert(now_ms());
            if now_ms().saturating_sub(since) < AUTO_DELETE_IDLE_MS {
                continue;
            }
            a.deleting = true;
            known
        };
        match known.map(|k| remove(&id, k, false, None)) {
            Some(Ok(_)) => crate::log::info("agents", &format!("auto-deleted helper {}", label(&id))),
            Some(Err(e)) => {
                crate::log::warn("agents", &format!("could not auto-delete helper {}: {e}", label(&id)));
                reg().lock().unwrap_or_else(|e| e.into_inner()).auto.remove(&id);
            }
            None => {
                reg().lock().unwrap_or_else(|e| e.into_inner()).auto.remove(&id);
            }
        }
    }
}

/// A chat is gone, however it was deleted: nothing here may keep it — not in
/// `luna_sessions`, not against a caller's helper limit, not as a live token,
/// and not as an answer owed to or by it.
///
/// Its helpers go on. Each passes to its own parent, whose it already was
/// through this one, for that one's tools to go on reaching it. With no parent
/// to pass to, it is an orphan: no agent has any claim on it, so it is the
/// window's to stop or delete, and a background one, which no row shows and no
/// answer of which is awaited any more, goes by itself once it has been idle
/// for AUTO_DELETE_IDLE_MS, rather than hold its account and folder for good.
pub fn forget(id: &str) {
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let gone = r.known.get(id).cloned();
    let spawned = gone.as_ref().is_some_and(|k| k.parent.is_some() || k.orphan);
    let auto = r.auto.remove(id);
    r.known.remove(id);
    let (handed, passed, moved) = pass_up(&mut r, id, gone.and_then(|k| k.parent));
    r.tokens.retain(|_, chat| chat != id);
    r.seen.retain(|(_, chat), _| chat != id);
    r.owed.retain(|(who, chat), _| who != id && chat != id);
    r.answered.retain(|(who, chat), _| who != id && chat != id);
    r.abandoned.retain(|_, (caller, _)| caller != id);
    r.spawns.retain(|_, s| s.caller != id);
    r.resumes.remove(id);
    r.openings.remove(id);
    r.worktree_names.remove(id);
    r.worktrees_before.remove(id);
    r.worktrees_left.remove(id);
    r.borrowed.remove(id);
    r.waits.retain(|(who, chat), _| who != id && chat != id);
    r.notified.remove(id);
    r.hook_asks.remove(id);
    r.notice_turns.remove(id);
    r.notice_times.retain(|(who, chat), _| who != id && chat != id);
    r.stopped.retain(|(who, chat), _| who != id && chat != id);
    r.exits.remove(id);
    let told = r.quota_told.len();
    r.quota_told.retain(|(caller, _, _), _| caller != id);
    let quota_now = (r.quota_told.len() != told).then(|| quota_bytes(&r.quota_told));
    r.receipts.retain(|(who, chat), _| who != id && chat != id);
    drop(r);
    if let Some(bytes) = quota_now {
        save_quota_told(bytes);
    }
    transcripts().lock().unwrap_or_else(|e| e.into_inner()).remove(id);
    if spawned {
        crate::agentlog::ended(id, if auto.is_some_and(|a| a.deleting) { "auto-deleted" } else { "deleted" });
    }
    for chat in passed {
        crate::emit::to_ui("agent://reparent", chat);
    }
    for (helper, parent) in moved {
        crate::background::set_parent(&helper, parent);
    }
    if handed {
        crate::background::changed();
    }
    // A core killed now must not bring it back.
    crate::background::save();
}

/// `id`'s helpers pass to `up`, or with no `up` stand alone (see `forget`).
/// Returns whether it had any, the visible ones that have a new parent, and the
/// background ones with who they answer to now. A visible one with none keeps the
/// parent its row names: its row says it is an orphan by that, and in a core
/// started later a parent nobody knows reaches it no better.
fn pass_up(r: &mut Registry, id: &str, up: Option<String>) -> (bool, Vec<Reparented>, Vec<(String, Option<String>)>) {
    let Registry { known, auto, .. } = r;
    let mut handed = false;
    let mut passed = Vec::new();
    let mut moved = Vec::new();
    for (kid, k) in known.iter_mut().filter(|(_, k)| k.parent.as_deref() == Some(id)) {
        k.parent = up.clone();
        k.orphan = up.is_none();
        let background = k.settings["background"] == true;
        if k.orphan && background {
            auto.entry(kid.clone()).or_default();
        }
        if let (Some(parent), false) = (&up, background) {
            passed.push(Reparented { id: kid.clone(), parent: parent.clone() });
        }
        if background {
            moved.push((kid.clone(), up.clone()));
        }
        handed = true;
    }
    (handed, passed, moved)
}

// --------------------------------------------------------------- notifier --

/// What became of a session the caller is owed an answer by.
enum Settled {
    /// A turn past the one asked about ended.
    Answered,
    /// A turn ended, but the session left a job running in the background;
    /// owed on from this turn count and transcript end, for that job's end or
    /// a turn of its own.
    Pending(u32, u64),
    /// A turn ended but is not settled — processes still at work, or no
    /// reply in the transcript yet: nothing to say, start the grace from now.
    Hold(u32),
    /// A job looked over and is at work again: the grace starts over.
    Unhold,
    /// Told it is not done, and all that ended since were turns on Luna's own
    /// notices: nothing to say, and owed on from this turn count, what they
    /// said included.
    Skip(u32),
    Waiting,
    /// It asks a question it does not wait on; told once per question.
    Asked(u64),
    /// A core's going stopped its turn and it went on by itself; told once.
    Resumed,
    Exited,
}

/// What the session has said since `cursor` (see `Owed::cursor`), read
/// straight off its transcript, its own messages in order (`own_words`;
/// `stopped`: its turn stopped short); empty when there is no transcript to
/// read (`transcript_found` tells that from saying nothing). A transcript
/// with nothing past the cursor has started over in a file of its own (a
/// resume): the last message there is the answer.
fn replies_since(pty: &crate::pty::PtyManager, id: &str, provider: Provider, cursor: u64, stopped: bool) -> Vec<String> {
    let Some(path) = transcript_path(pty, id) else { return Vec::new() };
    let mut messages = messages_at(&path, provider, cursor);
    if messages.is_empty() && cursor > 0 {
        messages = messages_at(&path, provider, 0).into_iter().last().into_iter().collect();
    }
    own_words(messages, stopped)
}

/// `replies_since` for a caller already told of what came before `cursor`:
/// nothing past it is nothing to tell, not a resume to look further back for.
fn said_since(pty: &crate::pty::PtyManager, id: &str, provider: Provider, cursor: u64) -> Vec<String> {
    let Some(path) = transcript_path(pty, id) else { return Vec::new() };
    own_words(messages_at(&path, provider, cursor), false)
}

fn messages_at(path: &std::path::Path, provider: Provider, from: u64) -> Vec<Message> {
    match provider {
        Provider::Claude => crate::claude::session::messages_from(path, from, false).0,
        Provider::Codex => crate::codex::session::messages_from(path, from).0,
    }
}

/// The session's own messages of these, in order. None while a prompt is the
/// last word: its reply is still to come, and what came before it answered
/// something else. Not so for a turn that `stopped` short (an error, a core's
/// going), which has said all it will: what it got to say is handed over.
/// What the CLI put in itself (a background task's end it takes up) is no
/// prompt, and its transcript reader leaves it out.
fn own_words(messages: Vec<Message>, stopped: bool) -> Vec<String> {
    if !stopped && messages.last().map_or(true, |m| m.role != "assistant") {
        return Vec::new();
    }
    messages.into_iter().filter(|m| m.role == "assistant").map(|m| m.text).collect()
}

/// Where a notice says the answer is: a file holding it, report first
/// (`answer_text`). The notice itself quotes no more than the report's first
/// line (`begins`) — a caller that had a long answer cut read the whole of it
/// back anyway, and a long notice reaches the CLI as a paste.
fn answer_file(id: &str, replies: &[String]) -> String {
    answer_where(note_file(id, &answer_text(replies)).as_deref())
}

fn answer_where(file: Option<&std::path::Path>) -> String {
    match file {
        Some(path) => format!("the answer is in {}, its report first", path.display()),
        None => "luna_read gives the answer".into(),
    }
}

/// `text` in a file for a notice to name, under `notes_dir`. Named by what it
/// holds: a notice a draft keeps back is worked out again at every look, and the
/// same answer is the same file. Two answers are two files however close together
/// they come, as a name of 128 bits of their contents cannot land one on the other.
fn note_file(id: &str, text: &str) -> Option<std::path::PathBuf> {
    use sha2::{Digest, Sha256};
    let dir = notes_dir();
    let digest: String = Sha256::digest(text.as_bytes()).iter().take(16).map(|b| format!("{b:02x}")).collect();
    let path = dir.join(format!("{id}-{digest}.md"));
    if path.is_file() {
        // Named just now, it is not to go with the week's sweep, which goes by
        // when a file was last written.
        let _ = std::fs::File::options().write(true).open(&path).and_then(|f| f.set_modified(std::time::SystemTime::now()));
        return Some(path);
    }
    std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, text)).ok().map(|()| path)
}

/// Whether a visible helper has nothing to carry on from, as one
/// `background::never_started` says so of has not: its first turn went with a core,
/// and its CLI came back afresh, or has not come back and has no conversation on disk
/// to come back to. Not while it is yet to go on by itself, which gives it its prompt.
fn first_turn_lost(pty: &crate::pty::PtyManager, id: &str) -> bool {
    let count = crate::activity::turns_counted(id).unwrap_or(0);
    let known = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let Some(o) = r.openings.get(id) else { return false };
        if r.resumes.contains_key(id) {
            return false;
        }
        if o.lost {
            return true;
        }
        if o.floor == 0 || count > o.floor {
            return false;
        }
        r.known.get(id).cloned()
    };
    if pty.alive(id) {
        return false;
    }
    known.map_or(true, |k| conversation_on_disk(id, k.provider, &k.folder, &k.account_path).is_none())
}

/// Whether what `caller` is to hear of `id` is all there will be: it was asked, a core
/// went, and nothing has that but Luna. No turn is coming to answer it, and what Luna
/// holds of the caller's stays for this rather than going in.
fn lost_whole(pty: &crate::pty::PtyManager, caller: &str, id: &str) -> bool {
    parent_of(id).as_deref() == Some(caller) && (crate::background::never_started(id) || first_turn_lost(pty, id))
}

/// `lost_whole` of a visible helper with nothing asked of it since: `caller` is owed
/// the turn a core went with and no other. Asked since, a text that went in has started
/// it, and what it says to that is still to come, though what it was first asked is lost.
fn still_all_lost(pty: &crate::pty::PtyManager, caller: &str, id: &str) -> bool {
    let cut = reg().lock().unwrap_or_else(|e| e.into_inner()).owed.get(&(caller.to_string(), id.to_string())).map_or(true, |o| o.cut_short);
    cut && parent_of(id).as_deref() == Some(caller) && first_turn_lost(pty, id)
}

/// The count that hands over a debt on a first turn lost: past a turn that went with a
/// core, which a session not back has no track to count. A question asked since is no
/// such debt, and still waits for its turn.
fn lost_count(caller: &str, id: &str) -> u32 {
    let count = crate::activity::turn_state(id).map(|t| t.turns_ended).or_else(|| crate::activity::turns_counted(id)).unwrap_or(0);
    let cut = reg().lock().unwrap_or_else(|e| e.into_inner()).owed.get(&(caller.to_string(), id.to_string())).filter(|o| o.cut_short).map(|o| o.baseline.saturating_add(1));
    count.max(cut.unwrap_or(0))
}

/// A `wait` on a helper whose first turn is lost: what there is, at once, handed over
/// as any answer is, so that a cancel crossing it puts it back with the debt.
fn lost_answer(caller: &str, id: &str, call: &crate::mcp::Call, alone: bool) -> Result<(Waited, bool), String> {
    let turns_ended = lost_count(caller, id);
    let undelivered = undelivered_note(caller, id);
    if alone && !call.commit(hand_over(caller, id, turns_ended, None)) {
        return Err(LET_GO.into());
    }
    let outcome = if live(&crate::pty::manager(), id) { "turn_done" } else { "exit" };
    let (state, ask) = state_of(id);
    let failed = Some("it never got started: Luna's core restarted before its first turn got anywhere".to_string());
    Ok((Waited { outcome, state, ask, turns_ended, reply: None, failed, transcript_missing: false, screen: None, undelivered, job_debt: None }, true))
}

/// What never reached `id` that `caller` is to hear of: what a background helper
/// keeps for it (`background::undelivered_for`), and for the parent of a visible one
/// that has lost its first turn, what it first asked and what Luna holds of its.
fn undelivered_for(id: &str, caller: &str, parent: bool) -> Vec<String> {
    let mut lost = crate::background::undelivered_for(id, caller, parent);
    if parent && first_turn_lost(&crate::pty::manager(), id) {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        lost.extend(r.openings.get(id).map(|o| o.text.clone()));
        lost.extend(r.held.iter().filter(|h| h.id == id && h.caller == caller).map(|h| h.text.clone()));
    }
    lost
}

/// What `take_undelivered` took off, for a cancel to put back.
struct Undelivered {
    queued: Vec<crate::background::Queued>,
    opening: Option<Opening>,
    held: Vec<Held>,
    worktrees: Option<Vec<String>>,
}

/// Takes off what `undelivered_for` gives, and the worktrees the parent hears of
/// with it, `caller` having been told.
fn take_undelivered(id: &str, caller: &str, parent: bool) -> Undelivered {
    let queued = crate::background::take_undelivered_for(id, caller, parent);
    let lost = parent && first_turn_lost(&crate::pty::manager(), id);
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let opening = if lost { r.openings.remove(id) } else { None };
    let mut held = Vec::new();
    if lost {
        let (mine, rest): (Vec<Held>, Vec<Held>) = std::mem::take(&mut r.held).into_iter().partition(|h| h.id == id && h.caller == caller);
        r.held = rest;
        held = mine;
    }
    let worktrees = if parent { r.worktrees_left.remove(id) } else { None };
    Undelivered { queued, opening, held, worktrees }
}

fn put_undelivered_back(id: &str, taken: Undelivered) {
    crate::background::put_back(id, taken.queued);
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(opening) = taken.opening {
        r.openings.insert(id.to_string(), opening);
    }
    let later = std::mem::replace(&mut r.held, taken.held);
    r.held.extend(later);
    if let Some(worktrees) = taken.worktrees {
        r.worktrees_left.insert(id.to_string(), worktrees);
    }
}

/// What never reached a helper (`undelivered_for`), as its answer file ends on
/// it: labelled so that it is not read as anything the helper said.
fn undelivered_text(lost: &[String]) -> String {
    let mut text = format!(
        "NOT DELIVERED: {} meant for this helper when Luna's core stopped its turn. It never acted on them; send again what still matters. \
         Word for word, oldest first (the first is its opening prompt, if it never got started):",
        count_of(lost)
    );
    for (i, message) in lost.iter().enumerate() {
        text.push_str(&format!("\n\n--- message {} of {} ---\n\n{message}", i + 1, lost.len()));
    }
    text
}

/// Words that end a sentence in a notice, with a stop unless they bring their
/// own: a CLI's error mostly does ("…pick a different model.").
fn sentence(words: &str) -> String {
    let words = words.trim_end();
    if words.ends_with(['.', '!', '?', '…']) {
        words.to_string()
    } else {
        format!("{words}.")
    }
}

fn count_of(messages: &[String]) -> String {
    match messages.len() {
        1 => "1 message".to_string(),
        n => format!("{n} messages"),
    }
}

/// The notice's words on what never reached the helper, from the file that
/// holds it; `after_answer` when the file opens on the answer instead.
fn undelivered_line(lost: &[String], after_answer: bool, file: Option<&std::path::Path>) -> String {
    if lost.is_empty() {
        return String::new();
    }
    let what = count_of(lost);
    match file {
        Some(_) if after_answer => format!(" Not delivered: {what} queued for it, at the end of that file; send again what still matters."),
        Some(path) => format!(" Not delivered: {what} queued for it, in {}; send again what still matters.", path.display()),
        None => format!(" Not delivered: {what} queued for it; luna_read gives them, send again what still matters."),
    }
}

/// The same for a tool result: where they are, or them, cut as any words are
/// that one result carries, should no file take them. Only what `caller` itself
/// is to hear of.
fn undelivered_note(caller: &str, id: &str) -> Option<String> {
    let parent = parent_of(id).as_deref() == Some(caller);
    let lost = undelivered_for(id, caller, parent);
    let left = if parent { worktrees_left_line(id) } else { String::new() };
    if lost.is_empty() {
        return (!left.is_empty()).then(|| left.trim_start().to_string());
    }
    let text = undelivered_text(&lost);
    let whole = lost_whole(&crate::pty::manager(), caller, id);
    Some(match note_file(id, &text) {
        Some(path) if whole => {
            format!("It never got started: Luna's core restarted before its first turn got anywhere; {}. Send again what still matters.{left}", never_started_what(&lost, Some(&path)))
        }
        Some(path) => format!("{}{left}", undelivered_line(&lost, false, Some(&path)).trim_start()),
        None => format!("{}{left}", text.chars().take(MAX_TEXT).collect::<String>()),
    })
}

/// How a notice says what it gives back of a helper that never got started.
fn never_started_what(lost: &[String], file: Option<&std::path::Path>) -> String {
    match (lost.len(), file) {
        (_, None) => "its prompt was not kept".to_string(),
        (1, Some(path)) => format!("its prompt is in {}", path.display()),
        (n, Some(path)) => format!("its prompt and {} are in {}", count_of(&lost[1.min(n)..]).replace("message", "queued message"), path.display()),
    }
}

/// What a notice says of worktrees begun and not gone on in. Not all are known for its
/// own (Codex's go by what is new), so it says what Luna can stand behind.
fn left_words(left: &[String]) -> String {
    if left.is_empty() {
        return String::new();
    }
    format!(" worktreeLeft: {}: begun for it as far as Luna can tell; nothing runs there and Luna has not removed it.", left.join(", "))
}

/// The same of what a helper a core cut off left (`Registry::worktrees_left`).
fn worktrees_left_line(id: &str) -> String {
    reg().lock().unwrap_or_else(|e| e.into_inner()).worktrees_left.get(id).map_or(String::new(), |left| left_words(left))
}

/// A helper leaves with text that never reached it and nobody told: somebody's
/// words all the same, so to a file the log names.
pub fn undelivered_left(id: &str, lost: &[crate::background::Queued]) {
    if lost.is_empty() {
        return;
    }
    let lost: Vec<String> = lost.iter().map(|q| q.text.clone()).collect();
    let lost = lost.as_slice();
    let file = note_file(id, &undelivered_text(lost));
    crate::log::warn(
        "agents",
        &format!("{} went with {} nobody was told of: {}", label(id), count_of(lost), file.map_or("not written".into(), |p| p.display().to_string())),
    );
}

/// A helper's messages as one answer: the report (`report_of`), ahead of the
/// progress lines that came before it; then the last one, a closing word
/// ("that was a stale wakeup, nothing changed") the report must not bury;
/// then the rest, in order.
fn answer_text(replies: &[String]) -> String {
    let longest = report_of(replies);
    let last = replies.len().saturating_sub(1);
    let mut whole = replies[longest].clone();
    if last != longest {
        whole.push_str(&format!("\n\n---\n\nIts last message:\n\n{}", replies[last]));
    }
    let rest: Vec<&str> = (0..replies.len()).filter(|&i| i != longest && i != last).map(|i| replies[i].as_str()).collect();
    if !rest.is_empty() {
        whole.push_str(&format!("\n\n---\n\nEverything else it said, in order:\n\n{}", rest.join("\n\n---\n\n")));
    }
    whole
}

/// Shorter than this, a message is a line or two: progress, a closing word, a
/// short answer. None of them is a report the others lead up to.
const SHORT_MESSAGE: usize = 400;

/// Which message is the report: the longest, unless every one is short — a
/// Codex helper writes "still running…" a dozen times before "DONE", the
/// longest of those a progress line — when it is the last, where it ended up.
fn report_of(replies: &[String]) -> usize {
    let longest = (0..replies.len()).max_by_key(|&i| replies[i].chars().count()).unwrap_or(0);
    if replies.get(longest).map_or(0, |r| r.chars().count()) < SHORT_MESSAGE {
        replies.len().saturating_sub(1)
    } else {
        longest
    }
}

/// A report's first line that says anything, on one line and short, without
/// its markdown: the verdict, for a caller deciding whether to open the file.
fn opening_line(report: &str) -> Option<String> {
    let mut fenced = false;
    for line in report.lines() {
        let line = line.trim();
        if line.starts_with("```") || line.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        let bare = line.trim_start_matches(|c: char| c == '#' || c == '>' || c.is_whitespace());
        let bare = ["- ", "* ", "+ "].iter().find_map(|m| bare.strip_prefix(m)).unwrap_or(bare);
        let bare = bare.replace("**", "").replace('`', "");
        let bare = bare.trim_matches(|c: char| c == '*' || c == '_' || c.is_whitespace());
        let flat = bare.split_whitespace().collect::<Vec<_>>().join(" ");
        let flat: String = flat.chars().filter(|c| !c.is_control()).collect();
        // A rule, a table's dashes, an empty heading.
        if !flat.chars().any(char::is_alphanumeric) {
            continue;
        }
        if flat.chars().count() <= NOTICE_QUOTE {
            return Some(flat);
        }
        let head: String = flat.chars().take(NOTICE_QUOTE).collect();
        return Some(format!("{}…", head.trim_end()));
    }
    None
}

/// What a notice says of the answer beyond where it is. Not for a turn that
/// stopped short: the first line of what it got to say is no verdict on it.
fn begins(replies: &[String]) -> String {
    match replies.get(report_of(replies)).and_then(|r| opening_line(r)) {
        Some(line) => format!(". It begins: \"{line}\""),
        None => String::new(),
    }
}

/// Where a debt stands, by what the session is doing now.
#[derive(Debug, PartialEq, Clone, Copy)]
enum Verdict {
    /// No news of a turn or a job: the session may still be on a prompt.
    Nothing,
    /// A turn the caller is to hear of ended.
    TurnOver,
    /// Told it is not done, and only turns on Luna's own notices ended since.
    OnlyNotices,
    /// Told it is not done, and it still is not.
    AtWork,
    /// Told it is not done, and what it had running has ended with no turn.
    JobOver,
}

/// `real_turn`: a turn past the baseline counts for this caller (`real_turn_past`).
fn verdict(o: &Owed, ended: u32, turn: Option<crate::activity::Turn>, job: bool, real_turn: bool) -> Verdict {
    use crate::activity::Turn;
    let past = ended > o.baseline && turn != Some(Turn::Busy);
    if !o.told_pending {
        return if past { Verdict::TurnOver } else { Verdict::Nothing };
    }
    match (past, real_turn) {
        (true, true) => Verdict::TurnOver,
        (true, false) => Verdict::OnlyNotices,
        _ if turn != Some(Turn::Idle) => Verdict::Nothing,
        _ if job => Verdict::AtWork,
        _ => Verdict::JobOver,
    }
}

/// Whether a turn of `id` past `after` and up to `ended` ended that `caller`,
/// told it is not done, is to hear of: one no Luna notice started; or, once
/// `id` has all its own helpers' answers (`waits`: not yet), one on a notice
/// from them, in which it goes on with what it was asked. Not when it is
/// `caller` who is that helper: that is its own notice coming back.
///
/// `upward`: `caller` is `id`'s parent and `id` has written more than a line
/// or two since it last heard (`news_for_parent`). Then turns on its own
/// helpers' notices reach it while others are still out, every turn since
/// being one of those: what a helper makes of its helpers' answers goes up the
/// tree it was spawned in, and nothing goes back to a session whose notice
/// started a turn. Two chats that wrote to each other are no parent and helper
/// to each other; one that is the other's parent hears only of turns on
/// notices about its helper's own helpers, and the helper never of its parent.
fn real_turn_past(id: &str, caller: &str, after: u32, ended: u32, waits: bool, upward: impl FnOnce() -> bool) -> bool {
    let turns = after.saturating_add(1)..=ended;
    let (counts, on_helpers) = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let marked = r.notice_turns.get(id);
        let mark = |turn: u32| marked.and_then(|m| m.iter().find(|m| m.turn == turn));
        let counts = turns.clone().any(|turn| match mark(turn) {
            None => true,
            Some(m) => !waits && m.helpers.iter().any(|h| h != caller),
        });
        (counts, turns.clone().all(|turn| mark(turn).is_some_and(|m| m.only_helpers && !m.helpers.is_empty())))
    };
    // Read off the lock: it looks at the transcript.
    counts || (on_helpers && !turns.is_empty() && upward())
}

/// For `real_turn_past`: `caller` is `id`'s parent, and `id` has written a
/// message since `cursor` that is more than a line or two (SHORT_MESSAGE): a
/// report worth a turn of the parent's, not "noted, waiting on the rest",
/// which comes with the answer all the same.
fn news_for_parent(pty: &crate::pty::PtyManager, caller: &str, id: &str, k: &Known, cursor: u64) -> bool {
    k.parent.as_deref() == Some(caller) && said_since(pty, id, k.provider, cursor).iter().any(|m| m.chars().count() >= SHORT_MESSAGE)
}

/// A notice about `about` went into `caller` and starts its turn ending at
/// `turn`.
fn notice_turn(r: &mut Registry, caller: &str, turn: u32, about: &[String]) {
    let helpers: Vec<String> = about.iter().filter(|a| is_descendant(r, caller, a)).cloned().collect();
    let only_helpers = helpers.len() == about.len();
    let turns = r.notice_turns.entry(caller.to_string()).or_default();
    turns.push(NoticeTurn { turn, helpers, only_helpers });
    if turns.len() > NOTICE_TURNS_KEPT {
        turns.remove(0);
    }
}

/// Counts a notice to `caller` about `id`. Returns whether it was the last
/// BREAKER_NOTICES allows: the pair is stopped from here on.
fn count_notice(r: &mut Registry, caller: &str, id: &str, now: u64) -> bool {
    let key = (caller.to_string(), id.to_string());
    let times = r.notice_times.entry(key.clone()).or_default();
    times.retain(|at| now.saturating_sub(*at) < BREAKER_WINDOW_MS);
    times.push(now);
    let last = times.len() >= BREAKER_NOTICES;
    if last {
        r.notice_times.remove(&key);
        r.stopped.insert(key, now);
    }
    last
}

/// Whether the next notice to `caller` about `id` is the last the breaker lets
/// through, for it to say so.
fn last_notice(caller: &str, id: &str, now: u64) -> bool {
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    r.notice_times.get(&(caller.to_string(), id.to_string())).is_some_and(|times| {
        times.iter().filter(|at| now.saturating_sub(**at) < BREAKER_WINDOW_MS).count() + 1 >= BREAKER_NOTICES
    })
}

/// Whether Luna has stopped telling `caller` of `id`'s turns and jobs. A person
/// typing into either of them since is someone looking at it, and lifts that;
/// nothing a model does does, or two models answering each other would lift it
/// forever.
fn pair_stopped(pty: &crate::pty::PtyManager, caller: &str, id: &str) -> bool {
    let key = (caller.to_string(), id.to_string());
    let Some(at) = reg().lock().unwrap_or_else(|e| e.into_inner()).stopped.get(&key).copied() else { return false };
    let since = now_ms().saturating_sub(at);
    let typed = |s: &str| pty.typed_ago_ms(s).is_some_and(|ago| ago < since);
    if !typed(caller) && !typed(id) {
        return true;
    }
    reg().lock().unwrap_or_else(|e| e.into_inner()).stopped.remove(&key);
    crate::log::info("agents", &format!("notices to {} about {} go on: a person has typed in", label(caller), label(id)));
    false
}

/// The notice for one session, or None while nothing has happened to it.
fn notice_line(
    pty: &crate::pty::PtyManager,
    caller: &str,
    who: &str,
    id: &str,
    k: &Known,
    o: Owed,
) -> Option<(Option<String>, Settled)> {
    let ts = crate::activity::turn_state(id);
    let ended = ts.map(|t| t.turns_ended).unwrap_or(0);
    let turn = ts.map(|t| t.turn);
    let alive = live(pty, id);
    // A Claude Code chat ends a turn on a job it put in the background and takes
    // it up again in a turn of its own when the job is done (`claude -p` stays
    // in its turn until then); for anything else that job's end is all there
    // will be to hear of.
    let left = if ts.is_some_and(|t| t.children_busy) && alive { left_quiet(id) } else { None };
    // All it left running has sat there doing nothing for long: no job at work.
    let idle = left.as_ref().filter(|(quiet, _)| *quiet >= IDLE_JOB_MS);
    let background = ts.is_some_and(|t| t.children_busy) && alive && idle.is_none();
    let waits = waits_on_helpers(id, caller);
    let between_turns = turn != Some(crate::activity::Turn::Busy);
    let real_turn = o.told_pending && real_turn_past(id, caller, o.baseline, ended, waits, || between_turns && news_for_parent(pty, caller, id, k, o.cursor));
    let now = verdict(&o, ended, turn, background || waits, real_turn);
    if now == Verdict::OnlyNotices {
        return Some((None, Settled::Skip(ended)));
    }
    if now == Verdict::AtWork && o.held.is_some() {
        return Some((None, Settled::Unhold));
    }
    if now == Verdict::JobOver && alive {
        // Its processes come and go between samples, and a job's end may be
        // followed by the turn the CLI takes it up in.
        return match o.held {
            Some((n, at)) if n == ended && now_ms().saturating_sub(at) >= BACKGROUND_GRACE_MS => {
                // Its turns on notices may have said something since.
                let said = said_since(pty, id, k.provider, o.cursor);
                let failed = failure_of(pty, id, k.provider);
                let quoted = if failed.is_some() { String::new() } else { begins(&said) };
                let said = match said.len() {
                    0 => "it has said nothing since your last notice. It does not take that up by itself; luna_send asks it how it went".to_string(),
                    1 => format!("it wrote 1 message since your last notice; {}{quoted}", answer_file(id, &said)),
                    n => format!("it wrote {n} messages since your last notice; {}{quoted}", answer_file(id, &said)),
                };
                let failed = failed.map_or(String::new(), |why| format!(" Its last turn stopped on an error: {}", sentence(&why)));
                if let Some((quiet, names)) = idle {
                    return Some((
                        Some(format!(
                            "{who} has only idle processes left, which have done nothing for {} min and will bring nothing by themselves: {names}. luna_kill ends them. As for its words, {said}.{failed}",
                            quiet / 60_000
                        )),
                        Settled::Answered,
                    ));
                }
                let since = if o.job_went { " that Luna's core has restarted" } else { "" };
                Some((Some(format!("{who} has nothing left running now{since}, and {said}.{failed}")), Settled::Answered))
            }
            Some((n, _)) if n == ended => None,
            _ => Some((None, Settled::Hold(ended))),
        };
    }
    // Going on by itself any moment: what it said so far is no end of anything.
    if now == Verdict::TurnOver && !resume_pending(id) {
        let cut = crate::background::cut_short(id) || o.cut_short;
        let failed = if cut { None } else { failure_of(pty, id, k.provider) };
        let replies = replies_since(pty, id, k.provider, o.cursor, cut || failed.is_some());
        let settle_ms = if background {
            BACKGROUND_GRACE_MS
        } else if replies.is_empty() {
            REPLY_GRACE_MS
        } else {
            0
        };
        // Asked mid-turn: the CLI may start a turn on the text right after.
        let settle_ms = if o.queued { settle_ms.max(QUEUED_GRACE_MS) } else { settle_ms };
        if settle_ms > 0 {
            match o.held {
                Some((n, at)) if n == ended => {
                    if now_ms().saturating_sub(at) < settle_ms {
                        return None;
                    }
                }
                _ => return Some((None, Settled::Hold(ended))),
            }
        }
        let parent = k.parent.as_deref() == Some(caller);
        let lost = undelivered_for(id, caller, parent);
        let file = match (replies.is_empty(), lost.is_empty()) {
            (true, true) => None,
            (true, false) => note_file(id, &undelivered_text(&lost)),
            (false, true) => note_file(id, &answer_text(&replies)),
            (false, false) => note_file(id, &format!("{}\n\n---\n\n{}", answer_text(&replies), undelivered_text(&lost))),
        };
        let reply = match replies.len() {
            0 if !transcript_found(pty, id) => {
                ", but Luna cannot find its transcript, so whether it replied is unknown (luna_read with screen: true shows what there is)".to_string()
            }
            0 => " without a reply (luna_read with screen: true shows why, if anything does)".to_string(),
            1 => format!(" with 1 message since you asked; {}", answer_where(file.as_deref())),
            n => format!(" with {n} messages since you asked; {}", answer_where(file.as_deref())),
        };
        if parent && (crate::background::never_started(id) || (o.cut_short && first_turn_lost(pty, id))) {
            // Before it is told: what its next turn will not go on in is told with this.
            crate::background::settle_worktree(id);
            let what = never_started_what(&lost, file.as_deref());
            return Some((
                Some(format!("{who} never got started: Luna's core restarted before its first turn got anywhere, so its prompt was lost and there is nothing to carry on from; {what}. luna_send starts it afresh.")),
                Settled::Answered,
            ));
        }
        let lost = undelivered_line(&lost, !replies.is_empty(), file.as_deref());
        let unsure = if o.unsure {
            " Luna cannot tell whether your last message reached it: the core went as it was going in, and its transcript cannot be read. luna_read shows what it has."
        } else {
            ""
        };
        if cut {
            return Some((
                Some(format!("{who} did NOT finish its turn: Luna's core restarted while it worked and stopped it. It ended{reply}.{lost}{unsure} luna_send has it carry on.")),
                Settled::Answered,
            ));
        }
        // Whatever else is going on: this turn did not get to its end.
        if let Some(why) = failed {
            let said = if replies.is_empty() { String::new() } else { format!(" It ended{reply}.") };
            return Some((Some(format!("{who} stopped on an error: {}{said}{lost} luna_send has it carry on.", sentence(&why))), Settled::Answered));
        }
        let reply = format!("{reply}{}", begins(&replies));
        if background || waits {
            // Said first and plainly: a weak model read "ended a turn" as
            // done and deleted the helper mid-job. Once per debt: a job can
            // run for days, and each notice costs the caller a turn.
            let what = match &left {
                Some((_, names)) if background => format!("a job it started still runs in the background ({names})"),
                _ if background => "a job it started still runs in the background".to_string(),
                _ => "helpers of its own have yet to answer it".to_string(),
            };
            let line = if o.told_pending {
                format!("{who} ended another turn{reply}, but {what}. The next notice comes when that is over, or with a turn of its own.")
            } else {
                format!("{who} is NOT done yet: {what}. It ended a turn{reply}. The next notice comes when that is over, or with a turn of its own.")
            };
            return Some((Some(line), Settled::Pending(ended, transcript_end(id))));
        }
        // Too short a turn for a notice of its own in between.
        let resumed = if o.resumed { " Luna's core stopped it on the way and it resumed automatically." } else { "" };
        let idle = idle.map_or(String::new(), |(quiet, names)| {
            format!(" It left processes running that have done nothing for {} min: {names}; luna_kill ends them.", quiet / 60_000)
        });
        return Some((Some(format!("{who} finished its turn{reply}.{lost}{resumed}{unsure}{idle}")), Settled::Answered));
    }
    standing_line(pty, caller, who, id, k, o)
}

/// What a notice has to say of where the session stands, whatever its turns
/// have come to: it has exited, or it holds on a prompt or a question. All a
/// pair the breaker has stopped still hears: a helper must never sit on a
/// prompt until it times out for the breaker's sake.
fn standing_line(pty: &crate::pty::PtyManager, caller: &str, who: &str, id: &str, k: &Known, o: Owed) -> Option<(Option<String>, Settled)> {
    let turn = crate::activity::turn_state(id).map(|t| t.turn);
    if !live(pty, id) {
        // The window has yet to start it, as it does a chat in a pane.
        if returning(k, core_up_ms()) {
            return None;
        }
        if o.cut_short && k.parent.as_deref() == Some(caller) && first_turn_lost(pty, id) {
            leave_half_made(id, k);
            let lost = undelivered_for(id, caller, true);
            let file = note_file(id, &undelivered_text(&lost));
            let what = never_started_what(&lost, file.as_deref());
            // No `luna_send` for one that is not running. Opened by a person, it
            // would start with nothing to go on.
            return Some((
                Some(format!("{who} never got started: Luna's core restarted before its first turn got anywhere, and its CLI has not started again, so there is nothing to carry on from; {what}. Spawn it again if it still matters, and luna_delete this one.")),
                Settled::Exited,
            ));
        }
        if o.cut_short {
            return Some((
                Some(format!("{who} did NOT finish its turn: Luna's core restarted while it worked, and it has not started again. It runs again once the user opens it.")),
                Settled::Exited,
            ));
        }
        if o.lost {
            return Some((
                Some(format!("{who} has exited before Luna could type your message in: the user's draft held it until then.")),
                Settled::Exited,
            ));
        }
        return Some((
            Some(match failure_of(pty, id, k.provider) {
                Some(why) => format!("{who} has exited on an error: {}", sentence(&why)),
                None => format!("{who} has exited."),
            }),
            Settled::Exited,
        ));
    }
    if o.resumed && !resume_pending(id) {
        return Some((
            Some(format!("{who} was cut short when Luna's core restarted and has resumed automatically, with anything queued for it. It works on; another notice comes when its turn ends.")),
            Settled::Resumed,
        ));
    }
    let ask = ask_of(id);
    let held = turn == Some(crate::activity::Turn::Waiting) || ask.as_ref().is_some_and(|a| a.blocking && a.kind == "question");
    if held && !o.told_waiting {
        let line = match &ask {
            Some(a) => match &a.id {
                Some(ask_id) => format!("{who} {}. Answer with luna_answer, askId {ask_id}.", a.line()),
                None => format!("{who} {}. Answer with luna_answer.", a.line()),
            },
            None => format!(
                "{who} is stopped on a prompt in its terminal; luna_read with screen: true shows it, luna_answer with keys answers it."
            ),
        };
        return Some((Some(line), Settled::Waiting));
    }
    // Codex's async question: the helper works on, the question waits.
    if let Some(a) = ask.filter(|a| !a.blocking) {
        let key = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            a.line().hash(&mut h);
            h.finish()
        };
        if o.told_ask != key {
            return Some((
                Some(format!("{who} {} It works on meanwhile; luna_answer answers it.", a.line())),
                Settled::Asked(key),
            ));
        }
    }
    None
}

/// The level of `QUOTA_LEVELS` a window at `pct` percent has reached, 0 if none.
fn quota_level(pct: f64) -> u8 {
    QUOTA_LEVELS.iter().copied().filter(|l| pct >= f64::from(*l)).max().unwrap_or(0)
}

/// Whether an instant has passed; one that does not read has not.
fn is_past(at: Option<&str>, now: u64) -> bool {
    at.and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()).is_some_and(|t| (t.timestamp_millis() as u64) < now)
}

/// Where a usage window stands against what the caller was last told of it:
/// the level to remember (0: nothing), and whether that is news. Numbers of a
/// window already over are stale, and say nothing until new ones come.
fn quota_step(told: Option<&QuotaTold>, pct: f64, resets_at: Option<&str>, now: u64) -> (u8, bool) {
    if is_past(resets_at, now) {
        return (0, false);
    }
    let reset = told.is_some_and(|t| {
        pct + QUOTA_SLACK < f64::from(t.level) || (t.resets_at.as_deref() != resets_at && is_past(t.resets_at.as_deref(), now))
    });
    let had = if reset { 0 } else { told.map_or(0, |t| t.level) };
    let level = quota_level(pct);
    (level.max(had), level > had)
}

fn quota_file() -> std::path::PathBuf {
    crate::paths::data_dir().join("quota-told.json")
}

/// `quota_told` as its file keeps it: a core started later is not to tell a
/// caller again what this one already did.
fn quota_bytes(told: &HashMap<(String, String, String), QuotaTold>) -> Vec<u8> {
    let mut rows: Vec<(&String, &String, &String, u8, &Option<String>)> =
        told.iter().map(|((c, a, l), t)| (c, a, l, t.level, &t.resets_at)).collect();
    rows.sort();
    serde_json::to_vec(&rows).unwrap_or_default()
}

/// The file's rows, but for windows over since they were written.
fn quota_from(bytes: &[u8], now: u64) -> HashMap<(String, String, String), QuotaTold> {
    let rows: Vec<(String, String, String, u8, Option<String>)> = serde_json::from_slice(bytes).unwrap_or_default();
    rows.into_iter()
        .filter(|row| !is_past(row.4.as_deref(), now))
        .map(|(c, a, l, level, resets_at)| ((c, a, l), QuotaTold { level, resets_at }))
        .collect()
}

/// Writes it out, with the registry's lock let go.
fn save_quota_told(bytes: Vec<u8>) {
    let path = quota_file();
    let tmp = path.with_extension("json.tmp");
    if let Err(e) = std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, &path)) {
        crate::log::warn("agents", &format!("could not save {}: {e}", path.display()));
    }
}

/// A usage window a caller is yet to hear of.
struct QuotaNews {
    key: (String, String, String),
    told: QuotaTold,
    line: String,
}

/// What each caller is yet to hear of the usage windows of the accounts its
/// live helpers run on, all generations counted, as `account_usage` last showed
/// them. It is told only once the notice has gone in (`quota_told`); what has
/// reset or come down is settled here.
fn quota_news(pty: &crate::pty::PtyManager) -> HashMap<String, Vec<QuotaNews>> {
    // Every helper's account, under every session above it.
    type Window = (String, f64, Option<String>);
    let (windows, helpers): (HashMap<String, Vec<Window>>, Vec<(String, String, String)>) = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let windows: HashMap<String, Vec<Window>> = r
            .usage
            .iter()
            .map(|(account, u)| {
                let limits = u["limits"].as_array().into_iter().flatten();
                let w: Vec<Window> = limits
                    .filter_map(|l| {
                        let resets = l["resetsAt"].as_str().map(str::to_owned);
                        Some((l["label"].as_str()?.to_string(), l["usedPct"].as_f64()?, resets))
                    })
                    .collect();
                (account.clone(), w)
            })
            .collect();
        // Nothing near a level and nothing told: the usual case, every look.
        if r.quota_told.is_empty() && !windows.values().flatten().any(|w| quota_level(w.1) > 0) {
            return HashMap::new();
        }
        let mut helpers = Vec::new();
        for (id, k) in &r.known {
            let account = format!("{}/{}", k.provider.as_str(), k.account);
            let mut above = k.parent.clone();
            while let Some(caller) = above {
                above = r.known.get(&caller).and_then(|p| p.parent.clone());
                if caller != DEV_CALLER && r.known.contains_key(&caller) {
                    helpers.push((caller, account.clone(), id.clone()));
                }
            }
        }
        (windows, helpers)
    };
    let live_ones: Vec<(String, String)> = helpers.into_iter().filter(|(_, _, id)| live(pty, id)).map(|(c, a, _)| (c, a)).collect();
    let mut news: HashMap<String, Vec<QuotaNews>> = HashMap::new();
    let now = now_ms();
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let before = quota_bytes(&r.quota_told);
    let mut pairs: Vec<&(String, String)> = live_ones.iter().collect();
    pairs.sort();
    pairs.dedup();
    for (caller, account) in pairs {
        for (label, pct, resets_at) in windows.get(account).into_iter().flatten() {
            let key = (caller.clone(), account.clone(), label.clone());
            let (level, say) = quota_step(r.quota_told.get(&key), *pct, resets_at.as_deref(), now);
            let told = QuotaTold { level, resets_at: resets_at.clone() };
            if say {
                let resets = resets_at.as_deref().map(|t| format!(" (resets {t})")).unwrap_or_default();
                let line = format!("Account {account}, which your helpers run on, has used {pct:.0}% of its \"{label}\" window{resets}.");
                news.entry(caller.clone()).or_default().push(QuotaNews { key, told, line });
            } else if level == 0 {
                r.quota_told.remove(&key);
            } else {
                r.quota_told.insert(key, told);
            }
        }
    }
    let after = quota_bytes(&r.quota_told);
    drop(r);
    if after != before {
        save_quota_told(after);
    }
    news
}

/// Whether a notice may go into `caller` now: its CLI is up and between turns
/// and nobody is typing into it. A draft in its composer is `type_text`'s to
/// put aside, or to keep the notice back for.
fn ready_for_notice(pty: &crate::pty::PtyManager, caller: &str) -> bool {
    // A background caller takes it as its next turn, between turns.
    if crate::background::has(caller) {
        return crate::activity::turn_state(caller).is_some_and(|t| t.turn == crate::activity::Turn::Idle);
    }
    pty.alive(caller)
        && crate::activity::turn_state(caller).is_some_and(|t| t.turn == crate::activity::Turn::Idle)
        && !still_starting(pty, caller)
        && pty.typed_ago_ms(caller).map_or(true, |ms| ms >= TYPING_QUIET_MS)
        && drawn_for_notice(pty, caller)
}

/// A CLI just started has drawn something, and may not be taking text yet: its
/// prompt says it is. For its first minute only: a prompt Luna cannot read (a mode
/// with a mark of its own, a CLI that draws it differently) must not silence notices.
fn drawn_for_notice(pty: &crate::pty::PtyManager, id: &str) -> bool {
    let long_up = pty.probe(id).map_or(true, |p| (now_ms() as u128).saturating_sub(p.spawned_at_ms) >= SESSION_RETURN_MS as u128);
    long_up || pty.at_prompt(id) || pty.has_draft(id)
}

/// Whether any of these owed answers has come in, without saying it yet.
fn anything_to_say(pty: &crate::pty::PtyManager, items: &[(String, Known, Owed, String)]) -> bool {
    items.iter().any(|(id, _, o, _)| {
        let ts = crate::activity::turn_state(id);
        let ended = ts.map(|t| t.turns_ended).unwrap_or(0);
        o.resumed
            || (ended > o.baseline && ts.map(|t| t.turn) != Some(crate::activity::Turn::Busy))
            || job_over(id, o, ts)
            || !live(pty, id)
    })
}

/// Whether a caller told a session is not done yet may have that session's job
/// to hear of: between turns, with no job of its own at work (`job_at_work`). For
/// the holds that wait on news; helpers still to answer it keep `notice_line` from
/// saying so.
fn job_over(id: &str, o: &Owed, ts: Option<crate::activity::TurnState>) -> bool {
    o.told_pending && ts.is_some_and(|t| t.turn == crate::activity::Turn::Idle) && !job_at_work(id, ts, true)
}

/// What a session's turns left running and has done nothing (no CPU, no I/O:
/// `left_quiet`) for this long is no job at work: its caller
/// is told so once, with what they are and how to end them, and the debt settles
/// as at a job's end. A real job's quiet stretch is minutes (a link, a test on a
/// timeout); half an hour of nothing is a `tail -F` or a server nobody uses, whose
/// end would never come and whose caller would never hear. As long as an
/// `autoDelete` helper waits once idle.
const IDLE_JOB_MS: u64 = 30 * 60 * 1000;

/// Whether `id` has a job at work between turns: what its turns left running,
/// unless all of that has sat idle for IDLE_JOB_MS.
fn job_at_work(id: &str, ts: Option<crate::activity::TurnState>, alive: bool) -> bool {
    alive && ts.is_some_and(|t| t.children_busy) && !left_quiet(id).is_some_and(|(quiet, _)| quiet >= IDLE_JOB_MS)
}

/// Between turns, how long all a session's turns left running has done nothing,
/// and their names: a helper's job's (background.rs), a chat's under its CLI
/// (activity.rs). None with nothing left.
fn left_quiet(id: &str) -> Option<(u64, String)> {
    if crate::background::has(id) {
        return crate::background::left_quiet(id);
    }
    crate::activity::left_quiet(id)
}

/// For a `wait`: whether a job's end (`over`) has stood BACKGROUND_GRACE_MS, by
/// the debt's own clock (`Owed::held`), the one the notifier goes by too: with
/// a clock each, the notice could go out while the wait still counted, and the
/// caller hear of it twice. A job at work again starts it over. With no debt
/// left to clock (a notice has just settled it), there is nothing to hold for.
fn job_end_stood(key: &(String, String), turns_ended: u32, over: bool) -> bool {
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let Some(o) = r.owed.get_mut(key) else { return over };
    if !over {
        o.held = None;
        return false;
    }
    match o.held {
        Some((n, at)) if n == turns_ended => now_ms().saturating_sub(at) >= BACKGROUND_GRACE_MS,
        _ => {
            o.held = Some((turns_ended, now_ms()));
            false
        }
    }
}

/// Whether a `wait` the caller has out on `id` is to hand this answer over
/// rather than a notice: while nothing has come in, and for WAIT_HOLD_MS
/// after something did — the wait answers within a second or two, so one
/// still out by then belongs to a client that has let the call go. A job's end
/// comes in only once it has stood its grace, on the clock the wait goes by
/// too (`job_end_stood`), which this starts for a wait that is not counting.
fn held_for_wait(pty: &crate::pty::PtyManager, caller: &str, id: &str, o: &Owed) -> bool {
    let key = (caller.to_string(), id.to_string());
    {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        if r.reserved.contains_key(&key) {
            return true;
        }
        if !r.waits.contains_key(&key) {
            return false;
        }
    }
    let ts = crate::activity::turn_state(id);
    let turn = ts.map(|t| t.turn);
    let alive = live(pty, id);
    let ended = ts.map_or(0, |t| t.turns_ended);
    // A prompt it stopped on is news too: `wait(turn_done)` hands that back.
    let other_news = (ended > o.baseline && turn != Some(crate::activity::Turn::Busy))
        || turn == Some(crate::activity::Turn::Waiting)
        || !alive;
    let job_ended = alive && job_over(id, o, ts) && !waits_on_helpers(id, caller);
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let Some(owed) = r.owed.get_mut(&key) else { return true };
    let job_news = job_ended && {
        let since = match owed.held {
            Some((n, at)) if n == ended => at,
            _ => {
                let at = now_ms();
                owed.held = Some((ended, at));
                at
            }
        };
        now_ms().saturating_sub(since) >= BACKGROUND_GRACE_MS
    };
    if !other_news && !job_news {
        // Taken back by a turn begun since (a queued send): the wait gets
        // its whole hold again for the news that turn brings.
        owed.ready_at = None;
        // A job at work again: its end starts its grace over.
        if owed.told_pending && !job_ended {
            owed.held = None;
        }
        return true;
    }
    let at = *owed.ready_at.get_or_insert(now_ms());
    now_ms().saturating_sub(at) < WAIT_HOLD_MS
}

/// Whether what `caller` is owed by `id` still stands `up_ms` into this core's
/// life: a delete takes the session or the caller from `known`, but a caller
/// not started under this core yet may be one on its way back
/// (CALLER_RETURN_MS).
fn owed_stands(known: &HashMap<String, Known>, caller: &str, id: &str, up_ms: u64) -> bool {
    known.contains_key(id) && (known.get(caller).is_some_and(|k| !k.waiting) || up_ms < CALLER_RETURN_MS)
}

/// How a spawn is named to its caller, who may have several: as it was named,
/// else by its prompt.
fn spawn_called(s: &SpawnTaken) -> String {
    let name = s.params.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).map(str::to_owned);
    match name.or_else(|| crate::pty::title_from_prompt(&s.params.prompt)) {
        Some(name) => format!("\"{name}\""),
        None => format!("number {}", s.request),
    }
}

/// What a notice says of a worktree this spawn had begun that nothing came of. It
/// is not removed: it may hold what a person would want, and nothing has claimed
/// it. A worktree another chat made meanwhile that has no session running in it
/// reads the same; it is only named.
fn worktrees_left(s: &SpawnTaken) -> String {
    let (Some(true), Some(folder)) = (s.params.worktree, s.params.folder.as_deref()) else { return String::new() };
    left_words(&crate::worktree::left_since(crate::worktree::made_for(folder, &s.account_path), &s.worktrees_before, &crate::pty::live_cwds()))
}

/// The notice of a spawn nothing came of. It says why the call itself failed: the
/// caller's CLI went with the same core, and all it has of that call is an error.
fn never_completed(s: &SpawnTaken, why: Option<&str>, left: &str) -> String {
    let prompt = match note_file(&format!("spawn-{}", s.request), &s.params.prompt) {
        Some(path) => format!("Its prompt is in {}", path.display()),
        None => "Its prompt was not kept".to_string(),
    };
    let why = why.map_or(String::new(), |w| format!(" (started afresh, it failed too: {w})"));
    format!(
        "Your luna_spawn {} never completed: Luna's core restarted while it was being made, which is also why that call ended in an error or with no answer. No helper came of it{why}. {prompt}; spawn it again if it still matters.{left}",
        spawn_called(s),
    )
}

/// Asks for it again as it was asked (`resumeOnRestart`), as `spawn` would have it
/// had it not settled anything yet, and leaves what its caller is to hear. Blocks
/// as a spawn does.
fn spawn_afresh(s: SpawnTaken) {
    let mut params = s.params.clone();
    params.visible = Some(s.visible);
    params.auto_delete = s.auto_delete;
    params.resume_on_restart = Some(true);
    if let Some(worktree) = &s.in_worktree {
        params.folder = Some(worktree.clone());
    }
    // Before this one can make a worktree of its own, which is no leftover.
    let left = worktrees_left(&s);
    let mut still_coming = false;
    let made = spawn_with(&s.caller, params, Some(s.request), &mut still_coming);
    let told = match &made {
        Ok(made) => {
            crate::log::warn("agents", &format!("{}'s spawn {} started afresh as {} after a core cut it off", label(&s.caller), s.request, label(&made.id)));
            format!(
                "Your luna_spawn {} was cut off when Luna's core restarted, which is also why that call ended in an error or with no answer. As it asked with resumeOnRestart, Luna started it afresh as {}: do not spawn it again, and its answer comes as a notice.{left}",
                spawn_called(&s),
                made.id,
            )
        }
        // The later request stands and the window may yet answer it, so this is no
        // failure: told to spawn again, the caller would have two.
        Err(why) if still_coming => {
            crate::log::warn("agents", &format!("{}'s spawn {} asked for afresh is not answered yet: {why}", label(&s.caller), s.request));
            format!(
                "Your luna_spawn {} was cut off when Luna's core restarted, which is also why that call ended in an error or with no answer. As it asked with resumeOnRestart, Luna asked for it afresh, and the window has not started it yet. It may still come up: luna_list will show it and its answer comes as a notice, or a notice says it failed. Do not spawn it again.{left}",
                spawn_called(&s),
            )
        }
        Err(why) => {
            crate::log::warn("agents", &format!("{}'s spawn {} could not start afresh after a core cut it off: {why}", label(&s.caller), s.request));
            never_completed(&s, Some(why), &left)
        }
    };
    let helper = made.ok().map(|made| made.id);
    let owed_placed = helper.is_some();
    let told = SpawnTaken { news: Some(told), helper, owed_placed, settled: true, retried: still_coming, ..s };
    reg().lock().unwrap_or_else(|e| e.into_inner()).spawns.insert(told.request, told);
    crate::background::save();
}

/// A spawn that had timed out, its caller told it may yet come up, and the window
/// says it did not.
fn spawn_failed_late(s: SpawnTaken, why: &str) {
    crate::log::warn("agents", &format!("{}'s spawn {} failed after it timed out: {why}", label(&s.caller), s.request));
    let prompt = match note_file(&format!("spawn-{}", s.request), &s.params.prompt) {
        Some(path) => format!("Its prompt is in {}", path.display()),
        None => "Its prompt was not kept".to_string(),
    };
    let news = format!(
        "Your luna_spawn {}, which had not started in time, did not come up after all: {} No helper came of it. {prompt}; spawn it again if it still matters.{}",
        spawn_called(&s),
        sentence(why),
        worktrees_left(&s)
    );
    if let Some(left) = reg().lock().unwrap_or_else(|e| e.into_inner()).spawns.get_mut(&s.request) {
        left.news = Some(news);
    }
    crate::background::save();
}

/// What the spawns the last core left have come to, once a window has had the time
/// to finish any it was part way through (`SESSION_RETURN_MS`, as for a chat coming
/// back): a session known by then is one that came of it, and answers as any other
/// (`take_up_spawns`, and a row since deleted is no longer known); for the rest the
/// caller is told, or with `resumeOnRestart` it is started afresh and then told.
/// What it came to is decided under the lock that gives it up (`SpawnTaken::settled`):
/// past that a session the window is still starting for it is refused (`spawn_is`),
/// and before it that session is the one that came of it.
fn settle_spawns(up_ms: u64) {
    if up_ms < SESSION_RETURN_MS {
        return;
    }
    let due: Vec<(SpawnTaken, bool)> = {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let Registry { spawns, known, abandoned, .. } = &mut *r;
        let mut due = Vec::new();
        for s in spawns.values_mut().filter(|s| s.left_by_core && !s.settled) {
            let made = s.helper.as_ref().is_some_and(|h| known.contains_key(h));
            s.settled = true;
            if !made && s.resume_on_restart {
                abandoned.remove(&s.request);
            }
            due.push((s.clone(), made));
        }
        for (s, made) in &due {
            if *made {
                spawns.remove(&s.request);
            }
        }
        due
    };
    for (s, made) in due {
        if made {
            crate::log::info("agents", &format!("{}'s spawn {} had come to {} when a core went", label(&s.caller), s.request, s.helper.as_deref().unwrap_or("")));
            crate::background::save();
        } else if s.resume_on_restart {
            let _ = std::thread::Builder::new().name("luna-spawn-again".into()).spawn(move || spawn_afresh(s));
        } else {
            let news = never_completed(&s, None, &worktrees_left(&s));
            crate::log::warn("agents", &format!("{}'s spawn {} went with a core: {}", label(&s.caller), s.request, news));
            if let Some(left) = reg().lock().unwrap_or_else(|e| e.into_inner()).spawns.get_mut(&s.request) {
                left.news = Some(news);
            }
        }
    }
}

/// What each caller is to hear of its spawns (`settle_spawns`), by caller: the
/// request and the line, once it is there to hear it. One that has not come back
/// in the time a caller gets (CALLER_RETURN_MS), or the dev caller with no terminal
/// to be told in, goes to the log, which names the file.
fn spawn_news(up_ms: u64) -> HashMap<String, Vec<(u64, String)>> {
    let mut news: HashMap<String, Vec<(u64, String)>> = HashMap::new();
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let Registry { spawns, known, .. } = &mut *r;
    let mut gave_up = Vec::new();
    let mut told: Vec<&SpawnTaken> = spawns.values().filter(|s| s.news.is_some()).collect();
    told.sort_by_key(|s| s.request);
    for s in told {
        let line = s.news.clone().unwrap_or_default();
        match known.get(&s.caller) {
            Some(k) if !k.waiting && s.caller != DEV_CALLER => news.entry(s.caller.clone()).or_default().push((s.request, line)),
            Some(k) if k.waiting && up_ms < CALLER_RETURN_MS => {}
            _ => gave_up.push((s.request, s.caller.clone(), line)),
        }
    }
    for (request, caller, line) in gave_up {
        spawns.remove(&request);
        crate::log::warn("agents", &format!("{caller} is not there to be told: {line}"));
    }
    news
}

/// Runs for the life of the app: every few seconds, for each caller owed an
/// answer, checks whether one came and tells the caller once it is idle; and
/// deletes the `autoDelete` helpers that are done (see `reap`).
fn notifier() {
    if let Ok(bytes) = std::fs::read(quota_file()) {
        reg().lock().unwrap_or_else(|e| e.into_inner()).quota_told = quota_from(&bytes, now_ms());
    }
    let mut idle_looks: HashMap<String, u32> = HashMap::new();
    // Callers whose notice a draft is holding back: since when, and whether
    // the desktop has been told.
    let mut drafts: HashMap<String, (u64, bool)> = HashMap::new();
    loop {
        std::thread::sleep(NOTIFY_EVERY);
        let pty = crate::pty::manager();
        reap(&pty);
        // Owed by caller, each session with what is known of it; anything a
        // delete has made unknown is dropped on the way, and a caller yet to
        // come back has nothing to be told in.
        let mut by_caller: std::collections::BTreeMap<String, Vec<(String, Known, Owed, String)>> = Default::default();
        let mut gave_up: Vec<(String, String)> = Vec::new();
        {
            let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
            let Registry { owed, known, names, .. } = &mut *r;
            let up_ms = core_up_ms();
            owed.retain(|(caller, id), _| {
                let stands = owed_stands(known, caller, id, up_ms);
                if !stands && known.contains_key(id) {
                    gave_up.push((caller.clone(), id.clone()));
                }
                stands
            });
            for ((caller, id), o) in owed.iter() {
                let Some(k) = known.get(id) else { continue };
                if known.get(caller).map_or(true, |k| k.waiting) {
                    continue;
                }
                let parent = k.parent.as_deref() == Some(caller.as_str());
                let name = names.get(id).map(|n| n.trim()).filter(|n| !n.is_empty());
                let who = match (parent, name) {
                    (true, Some(n)) => format!("Your helper \"{n}\" ({id})"),
                    (true, None) => format!("Your helper {id}"),
                    (false, Some(n)) => format!("Session \"{n}\" ({id}), which you wrote to,"),
                    (false, None) => format!("Session {id}, which you wrote to,"),
                };
                by_caller.entry(caller.clone()).or_default().push((id.clone(), k.clone(), *o, who));
            }
        }
        for (caller, id) in gave_up {
            crate::log::warn("agents", &format!("{caller} is owed the answer of {} but has not come back in {} min; no notice will go", label(&id), CALLER_RETURN_MS / 60_000));
        }
        // A caller owed nothing still hears of the quota its helpers run on,
        // and of a spawn of its own that a core's going cut off.
        let mut quota = quota_news(&pty);
        for caller in quota.keys() {
            by_caller.entry(caller.clone()).or_default();
        }
        settle_spawns(core_up_ms());
        let mut spawns = spawn_news(core_up_ms());
        for caller in spawns.keys() {
            by_caller.entry(caller.clone()).or_default();
        }
        idle_looks.retain(|c, _| by_caller.contains_key(c));
        drafts.retain(|c, _| by_caller.contains_key(c));
        reg().lock().unwrap_or_else(|e| e.into_inner()).notices_held.retain(|c, _| by_caller.contains_key(c));
        for (caller, items) in by_caller {
            // The last notice is still going in: it starts a turn of its own.
            if TypingInto::under_way(&caller) {
                idle_looks.insert(caller, 0);
                continue;
            }
            if pty.has_draft(&caller) && pty.alive(&caller) && anything_to_say(&pty, &items) {
                let (since, told) = drafts.entry(caller.clone()).or_insert((now_ms(), false));
                if !*told && now_ms().saturating_sub(*since) >= DRAFT_ALERT_MS {
                    *told = true;
                    let chat = chat_name(&caller).unwrap_or_else(|| caller.clone());
                    crate::power::notify_chat(
                        &caller,
                        "A helper has answered",
                        &format!("\"{chat}\" has an unsent draft, so Luna is holding the helper's notice until you send or clear it."),
                    );
                }
            } else {
                drafts.remove(&caller);
            }
            let looks = idle_looks.entry(caller.clone()).or_default();
            *looks = if ready_for_notice(&pty, &caller) { *looks + 1 } else { 0 };
            if *looks < NOTIFY_IDLE_LOOKS {
                continue;
            }
            let mut lines = Vec::new();
            let mut settled = Vec::new();
            for (id, k, o, who) in &items {
                if held_for_wait(&pty, &caller, id, o) {
                    continue;
                }
                let said = if pair_stopped(&pty, &caller, id) { standing_line(&pty, &caller, who, id, k, *o) } else { notice_line(&pty, &caller, who, id, k, *o) };
                match said {
                    Some((Some(mut line), how)) => {
                        // Told once, with whatever is told first.
                        if k.parent.as_deref() == Some(caller.as_str()) {
                            line.push_str(&worktrees_left_line(id));
                        }
                        if counts_for_breaker(&how) && last_notice(&caller, id, now_ms()) {
                            line.push_str(&format!(
                                " Luna stops telling you of its turns and jobs now: {BREAKER_NOTICES} notices in {} minutes look like chats waking each other. Its prompts and questions still come, luna_wait and luna_read still reach it, and a person typing into either chat lifts this.",
                                BREAKER_WINDOW_MS / 60_000
                            ));
                        }
                        lines.push(line);
                        settled.push((id.clone(), how, *o));
                    }
                    Some((None, Settled::Hold(ended))) => {
                        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(o) = r.owed.get_mut(&(caller.clone(), id.clone())) {
                            o.held = Some((ended, now_ms()));
                        }
                    }
                    Some((None, Settled::Unhold)) => {
                        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(o) = r.owed.get_mut(&(caller.clone(), id.clone())) {
                            o.held = None;
                        }
                    }
                    Some((None, Settled::Skip(ended))) => {
                        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(o) = r.owed.get_mut(&(caller.clone(), id.clone())).filter(|now| now.rev == o.rev) {
                            o.baseline = ended;
                            o.held = None;
                        }
                    }
                    _ => {}
                }
            }
            let quota_lines = quota.remove(&caller).unwrap_or_default();
            lines.extend(quota_lines.iter().map(|n| n.line.clone()));
            let spawn_lines = spawns.remove(&caller).unwrap_or_default();
            lines.extend(spawn_lines.iter().map(|(_, line)| line.clone()));
            if lines.is_empty() {
                reg().lock().unwrap_or_else(|e| e.into_inner()).notices_held.remove(&caller);
                continue;
            }
            // Typing it waits up to SEND_TIMEOUT for the turn to start, off this
            // loop: one caller stuck on that is no reason to hold back the rest.
            let Some(typing) = TypingInto::claim(&caller) else { continue };
            idle_looks.insert(caller.clone(), 0);
            let text = format!("{NOTICE_TAG} {}", lines.join(" "));
            let typed = std::thread::Builder::new().name("luna-notice".into()).spawn(move || {
                let _typing = typing;
                deliver_notice(&crate::pty::manager(), &caller, &text, settled, quota_lines, spawn_lines);
            });
            if let Err(e) = typed {
                crate::log::warn("agents", &format!("could not start typing a notice: {e}"));
            }
        }
    }
}

/// What the breaker counts, and all it silences: notices a turn's or a job's
/// end brings. Not a prompt or a question, which hold until they are answered
/// and cannot loop by themselves, and a busy helper may ask many.
fn counts_for_breaker(how: &Settled) -> bool {
    matches!(how, Settled::Answered | Settled::Pending(..))
}

/// Types a notice into `caller` and settles what it said (`settled`: the
/// session, what became of it, the debt as it was announced; `quota_lines`: the
/// usage windows it told of; `spawn_lines`: the spawns cut off by a core going).
fn deliver_notice(
    pty: &crate::pty::PtyManager,
    caller: &str,
    text: &str,
    settled: Vec<(String, Settled, Owed)>,
    quota_lines: Vec<QuotaNews>,
    spawn_lines: Vec<(u64, String)>,
) {
    let before = crate::activity::turn_state(caller);
    let baseline = before.map(|t| t.turns_ended).unwrap_or(0);
    // The turn it starts: the next, or for a background caller that has begun
    // one since the look, the one after that.
    let (typed, turn) = if crate::background::has(caller) {
        match crate::background::send(caller, text, None) {
            Ok(queued) => (Ok(Some(true)), baseline + 1 + u32::from(queued)),
            Err(e) => (Err(e), 0),
        }
    } else {
        (type_text(pty, caller, text, before.map(|t| t.turn), baseline), baseline + 1)
    };
    let about = settled
        .iter()
        .map(|(id, _, _)| label(id))
        .chain(quota_lines.iter().map(|n| format!("{} at {}%", n.key.1, n.told.level)))
        .chain(spawn_lines.iter().map(|(request, _)| format!("spawn {request}")))
        .collect::<Vec<_>>()
        .join(", ");
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    match typed {
        Ok(None) => {
            r.notices_held.insert(caller.to_string(), settled.len() + quota_lines.len() + spawn_lines.len());
        }
        _ => {
            r.notices_held.remove(caller);
        }
    }
    drop(r);
    match typed {
        Ok(None) => {}
        Ok(Some(started)) => {
            if started {
                crate::log::info("agents", &format!("notice to {} about {about}", label(caller)));
            } else {
                crate::log::warn("agents", &format!("notice to {} about {about} did not start a turn", label(caller)));
            }
            // Its words on what never reached them went with this notice.
            let told_lost: Vec<String> = settled.iter().filter(|(_, how, _)| matches!(how, Settled::Answered | Settled::Exited)).map(|(id, _, _)| id.clone()).collect();
            let told_left: Vec<String> = settled.iter().map(|(id, _, _)| id.clone()).filter(|id| parent_of(id).as_deref() == Some(caller)).collect();
            let mut stopped = Vec::new();
            let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
            for id in &told_left {
                r.worktrees_left.remove(id);
            }
            // A turn it did not start stays unmarked: at worst the caller hears
            // of a turn once too often, never misses one.
            if started {
                let told_of: Vec<String> = settled.iter().map(|(id, _, _)| id.clone()).collect();
                notice_turn(&mut r, caller, turn, &told_of);
            }
            for (id, how, told) in settled {
                if counts_for_breaker(&how) && count_notice(&mut r, caller, &id, now_ms()) {
                    stopped.push(id.clone());
                }
                settle_told(&mut r, (caller.to_string(), id), how, &told);
            }
            let told_quota = !quota_lines.is_empty();
            for n in quota_lines {
                r.quota_told.insert(n.key, n.told);
            }
            let told_spawns = !spawn_lines.is_empty();
            for (request, _) in spawn_lines {
                r.spawns.remove(&request);
            }
            let quota_now = told_quota.then(|| quota_bytes(&r.quota_told));
            drop(r);
            if let Some(bytes) = quota_now {
                save_quota_told(bytes);
            }
            for id in &told_lost {
                take_undelivered(id, caller, parent_of(id).as_deref() == Some(caller));
            }
            if !told_lost.is_empty() || !told_left.is_empty() || told_spawns {
                crate::background::save();
            }
            for id in stopped {
                breaker_tripped(caller, &id);
            }
        }
        Err(e) => crate::log::warn("agents", &format!("notice to {} about {about} failed: {e}", label(caller))),
    }
}

/// What a notice that went in does to the debt it told of (`told`). Only to
/// that debt: typing the notice takes seconds, and the turn it starts may ask
/// again before they are over, a question that is owed on; and a `wait` may
/// have taken the answer meanwhile, which leaves nothing to carry on.
fn settle_told(r: &mut Registry, key: (String, String), how: Settled, told: &Owed) {
    if r.owed.get(&key).map(|o| o.rev) != Some(told.rev) {
        return;
    }
    match how {
        Settled::Answered | Settled::Exited => {
            r.owed.remove(&key);
        }
        Settled::Waiting => {
            if let Some(o) = r.owed.get_mut(&key) {
                o.told_waiting = true;
            }
        }
        Settled::Pending(ended, cursor) => {
            r.owed.insert(key, Owed::pending(ended, cursor));
        }
        Settled::Asked(ask) => {
            if let Some(o) = r.owed.get_mut(&key) {
                o.told_ask = ask;
            }
        }
        Settled::Resumed => {
            if let Some(o) = r.owed.get_mut(&key) {
                o.resumed = false;
            }
        }
        Settled::Hold(_) | Settled::Unhold | Settled::Skip(_) => {}
    }
}

/// Says that the breaker has stopped a pair: in the log, and once on the
/// desktop, as nothing else will tell the person why these chats went quiet.
fn breaker_tripped(caller: &str, id: &str) {
    crate::log::warn(
        "agents",
        &format!(
            "stopped telling {} of the turns and jobs of {}: {BREAKER_NOTICES} notices within {} minutes; a person typing into either lifts this",
            label(caller),
            label(id),
            BREAKER_WINDOW_MS / 60_000
        ),
    );
    let chat = chat_name(caller).unwrap_or_else(|| caller.to_string());
    let other = chat_name(id).unwrap_or_else(|| id.to_string());
    crate::power::notify_chat(
        caller,
        "Luna stopped a notice loop",
        &format!("\"{chat}\" and \"{other}\" kept waking each other, so Luna has stopped telling the first when the second's turns end. Type into either chat to let it go on."),
    );
}

// --------------------------------------------------------------- commands --

/// Every chat's name and window group, sent whenever one changes: what
/// `luna_sessions` calls the user's own chats and where it puts them, which
/// the core otherwise knows only by id.
#[tauri::command]
pub fn chat_names(mut names: HashMap<String, String>, groups: Option<HashMap<String, u32>>) {
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    // A row deleted while no core ran told nobody.
    let gone = gone_from_the_window(&r, &names);
    // A background helper has no row to be named by: it keeps the name it
    // was spawned with (`set_name`) until the window runs it as a chat.
    for (id, name) in &r.names {
        if !names.contains_key(id) && r.known.get(id).is_some_and(|k| k.settings["background"] == true) {
            names.insert(id.clone(), name.clone());
        }
    }
    r.names = names;
    if let Some(groups) = groups {
        r.groups = groups;
    }
    drop(r);
    for id in gone {
        crate::log::info("agents", &format!("{id} has no row any more; letting go of what the last core kept of it"));
        forget(&id);
        crate::activity::forget(&id);
    }
}

/// The sessions kept from the last core (`Known::waiting`) that the window, now
/// it has said which chats it has, does not have.
fn gone_from_the_window(r: &Registry, names: &HashMap<String, String>) -> Vec<String> {
    if names.is_empty() {
        return Vec::new();
    }
    r.known.iter().filter(|(id, k)| k.waiting && !names.contains_key(*id)).map(|(id, _)| id.clone()).collect()
}

/// Each account's usage as the window shows it, by `provider/name`, sent
/// whenever it changes: `luna_list` passes it on, so an agent picks an
/// account by the numbers the user sees — and the core asks no usage
/// endpoint of its own, which would only draw their 429s sooner.
#[tauri::command]
pub fn account_usage(usage: HashMap<String, serde_json::Value>) {
    reg().lock().unwrap_or_else(|e| e.into_inner()).usage = usage;
}

/// Account names the user has switched off for agents, as `provider/name`.
#[tauri::command]
pub fn agent_blocked_accounts() -> Vec<String> {
    crate::settings::get().agent_blocked_accounts
}

#[tauri::command]
pub fn set_agent_account(provider: Provider, name: String, allowed: bool) -> Result<Vec<String>, String> {
    let key = format!("{}/{}", provider.as_str(), name);
    crate::settings::update(|s| {
        s.agent_blocked_accounts.retain(|k| k != &key);
        if !allowed {
            s.agent_blocked_accounts.push(key.clone());
        }
    })?;
    Ok(crate::settings::get().agent_blocked_accounts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(provider: Provider, name: &str) -> crate::accounts::AccountInfo {
        crate::accounts::AccountInfo { provider, name: name.into(), path: format!("C:\\acc\\{name}") }
    }

    /// One window per figure, resetting far ahead.
    fn used(pairs: &[(&str, &[(&str, f64)])]) -> HashMap<String, serde_json::Value> {
        pairs
            .iter()
            .map(|(name, windows)| {
                let limits: Vec<_> = windows
                    .iter()
                    .map(|(label, pct)| serde_json::json!({ "label": label, "usedPct": pct, "resetsAt": "2999-01-01T00:00:00Z" }))
                    .collect();
                (format!("claude/{name}"), serde_json::json!({ "limits": limits }))
            })
            .collect()
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn a_list_takes_the_first_with_room_and_says_what_it_passed_over() {
        let allowed = [account(Provider::Claude, "a"), account(Provider::Claude, "b"), account(Provider::Claude, "c")];
        let usage = used(&[("a", &[("5 hours", 97.0), ("week", 10.0)]), ("b", &[("5 hours", 94.0)]), ("c", &[("5 hours", 1.0)])]);
        let (taken, choice) = pick_from_list(&names(&["a", "b", "c"]), Provider::Claude, &allowed, &usage, |_| true, 0).unwrap();
        assert_eq!(taken.name, "b", "under the line, and before c");
        assert_eq!(choice.account, "b");
        assert_eq!(choice.skipped, [SkippedAccount { account: "a".into(), why: "5 hours at 97%".into() }]);
        assert_eq!(choice.why, "the first in the list with room in every window (under 95%); the fullest, 5 hours, is at 94%");
    }

    #[test]
    fn a_list_passes_over_who_is_not_signed_in_or_not_allowed() {
        let allowed = [account(Provider::Claude, "out"), account(Provider::Claude, "in")];
        let (taken, choice) =
            pick_from_list(&names(&["blocked", "out", "in"]), Provider::Claude, &allowed, &HashMap::new(), |a| a.name == "in", 0).unwrap();
        assert_eq!(taken.name, "in");
        assert_eq!(choice.skipped.len(), 2);
        assert_eq!(choice.skipped[1].why, "not signed in");
        assert_eq!(choice.why, "the first in the list signed in; its usage is not known yet");
    }

    #[test]
    fn a_full_list_comes_down_to_the_most_room() {
        let allowed = [account(Provider::Claude, "a"), account(Provider::Claude, "b"), account(Provider::Claude, "c")];
        let usage = used(&[("a", &[("week", 100.0)]), ("b", &[("week", 96.0), ("5 hours", 20.0)]), ("c", &[("week", 99.0)])]);
        let (taken, choice) = pick_from_list(&names(&["a", "b", "c"]), Provider::Claude, &allowed, &usage, |_| true, 0).unwrap();
        assert_eq!(taken.name, "b");
        assert_eq!(choice.skipped.iter().map(|s| s.account.as_str()).collect::<Vec<_>>(), ["a", "c"]);
        assert!(choice.why.contains("the most room, at 96%"), "{}", choice.why);
    }

    #[test]
    fn a_list_with_nobody_to_take_it_is_an_error() {
        let allowed = [account(Provider::Claude, "a")];
        let e = pick_from_list(&names(&["a", "gone"]), Provider::Claude, &allowed, &HashMap::new(), |_| false, 0).err().unwrap();
        assert!(e.contains("a: not signed in") && e.contains("gone: no claude account"), "{e}");
        assert!(pick_from_list(&[], Provider::Claude, &allowed, &HashMap::new(), |_| true, 0).err().unwrap().contains("empty"));
    }

    #[test]
    fn a_window_past_its_reset_no_longer_counts() {
        let allowed = [account(Provider::Claude, "a")];
        let usage: HashMap<String, serde_json::Value> = [(
            "claude/a".to_string(),
            serde_json::json!({ "limits": [{ "label": "5 hours", "usedPct": 100, "resetsAt": "2026-01-01T00:00:00+00:00" }] }),
        )]
        .into();
        let before = chrono::DateTime::parse_from_rfc3339("2025-12-31T00:00:00Z").unwrap().timestamp_millis() as u64;
        let after = chrono::DateTime::parse_from_rfc3339("2026-01-02T00:00:00Z").unwrap().timestamp_millis() as u64;
        let pick = |now| pick_from_list(&names(&["a"]), Provider::Claude, &allowed, &usage, |_| true, now).unwrap().1;
        assert!(pick(before).why.contains("the most room, at 100%"), "spent, and the only one");
        assert!(pick(after).why.contains("reset since"), "the figure is from before the reset");
    }

    #[test]
    fn a_list_is_of_one_provider() {
        let allowed = [account(Provider::Claude, "c"), account(Provider::Codex, "x"), account(Provider::Claude, "both"), account(Provider::Codex, "both")];
        let of = |list: &[&str], asked| list_provider(&names(list), &allowed, asked, Provider::Claude);
        assert_eq!(of(&["c", "both"], None), Ok(Provider::Claude));
        assert_eq!(of(&["x", "both"], None), Ok(Provider::Codex), "the names bring their provider");
        assert_eq!(of(&["both"], None), Ok(Provider::Claude), "the caller's first");
        assert_eq!(of(&["c", "unknown"], None), Ok(Provider::Claude), "an unknown one is skipped later, with a reason");
        assert!(of(&["c", "x"], None).is_err());
        assert!(of(&["c"], Some(Provider::Codex)).is_err());
        assert_eq!(of(&["both", "x"], Some(Provider::Codex)), Ok(Provider::Codex));
    }

    #[test]
    fn account_comes_as_a_name_or_a_list_and_goes_out_as_a_name() {
        let p: SpawnParams = serde_json::from_value(serde_json::json!({ "prompt": "x", "account": "a", "readOnly": true, "agent": "checker" })).unwrap();
        assert_eq!(p.account_name(), Some("a"));
        assert_eq!(p.read_only, Some(true));
        assert_eq!(p.agent.as_deref(), Some("checker"));
        let p: SpawnParams = serde_json::from_value(serde_json::json!({ "prompt": "x", "account": ["a", "b"] })).unwrap();
        assert!(matches!(p.account, Some(AccountAsk::List(ref l)) if l.len() == 2));
        assert_eq!(p.account_name(), None, "not settled yet");
        let p = SpawnParams { account: Some(AccountAsk::One("a".into())), read_only: Some(true), agent: Some("checker".into()), ..Default::default() };
        let out = serde_json::to_value(&p).unwrap();
        assert_eq!(out["account"], "a", "the frontend reads a string");
        assert_eq!(out["readOnly"], true);
        assert_eq!(out["agent"], "checker");
    }

    fn role(read_only: bool, f: impl FnOnce(&mut SpawnParams)) -> SpawnParams {
        let mut p = SpawnParams { read_only: Some(read_only), ..Default::default() };
        f(&mut p);
        p
    }

    #[test]
    fn read_only_codex_goes_to_the_read_only_sandbox_unless_told_which() {
        let mut p = role(true, |_| {});
        settle_role(&mut p, Provider::Codex).unwrap();
        assert_eq!(p.sandbox.as_deref(), Some("read-only"), "no network unless asked for");
        let mut p = role(true, |p| p.sandbox = Some("read-only-net".into()));
        settle_role(&mut p, Provider::Codex).unwrap();
        assert_eq!(p.sandbox.as_deref(), Some("read-only-net"));
        let mut p = role(false, |_| {});
        settle_role(&mut p, Provider::Codex).unwrap();
        assert_eq!(p.sandbox, None, "the account's default");
    }

    #[test]
    fn read_only_net_goes_with_never_and_not_with_auto_review() {
        let spec = |approval: &str| {
            let p = SpawnParams { sandbox: Some("read-only-net".into()), approval: Some(approval.into()), ..Default::default() };
            background_spec(&p, Provider::Codex, "", "")
        };
        assert_eq!(spec("never").ok().unwrap().mode, "read-only-net");
        assert!(spec("auto-review").err().unwrap().contains("workspace-write only"), "the reviewer's sandbox is fixed");
    }

    #[test]
    fn what_contradicts_read_only_is_refused() {
        for sandbox in ["workspace-write", "danger-full-access"] {
            let mut p = role(true, |p| p.sandbox = Some(sandbox.into()));
            assert!(settle_role(&mut p, Provider::Codex).unwrap_err().contains("contradicts"), "{sandbox}");
        }
        let mut p = role(true, |p| p.approval = Some("auto-review".into()));
        assert!(settle_role(&mut p, Provider::Codex).unwrap_err().contains("auto-review"));
        let mut p = role(true, |p| p.permission_mode = Some("acceptEdits".into()));
        assert!(settle_role(&mut p, Provider::Claude).unwrap_err().contains("acceptEdits"));
        // A checker with Bash and nobody to ask is the point of it.
        let mut p = role(true, |p| p.permission_mode = Some("bypassPermissions".into()));
        settle_role(&mut p, Provider::Claude).unwrap();
    }

    #[test]
    fn an_agent_is_for_claude_code_and_a_name_only() {
        let mut p = role(false, |p| p.agent = Some("checker".into()));
        settle_role(&mut p, Provider::Claude).unwrap();
        assert_eq!(p.agent.as_deref(), Some("checker"));
        assert!(settle_role(&mut p, Provider::Codex).unwrap_err().contains("Claude Code option"));
        let mut p = role(false, |p| p.agent = Some("  ".into()));
        settle_role(&mut p, Provider::Codex).unwrap();
        assert_eq!(p.agent, None, "blank is not asked");
        for bad in ["--dangerously-skip-permissions", "a b", "a/b"] {
            let mut p = role(false, |p| p.agent = Some(bad.into()));
            assert!(settle_role(&mut p, Provider::Claude).is_err(), "{bad}");
        }
        let mut p = role(false, |p| p.agent = Some("plugin:review-2.0_x".into()));
        settle_role(&mut p, Provider::Claude).unwrap();
    }

    #[test]
    fn sessions_filter_by_group_and_text() {
        let none = serde_json::json!({});
        register("grp-me", Provider::Claude, "C:\\work\\luna", "C:\\acc\\anthropic\\one", None, true, none.clone());
        register("grp-peer", Provider::Codex, "C:\\work\\asuri", "C:\\acc\\openai\\two", None, false, none.clone());
        register("grp-far", Provider::Claude, "C:\\work\\luna", "C:\\acc\\anthropic\\one", None, false, none.clone());
        register("grp-loose", Provider::Claude, "C:\\work\\luna", "C:\\acc\\anthropic\\one", None, false, none);
        // Added to what is there and not said as `chat_names` says it: the window's
        // whole list, which would let go of every other test's waiting chat.
        {
            let mut r = reg().lock().unwrap();
            r.names.extend([("grp-me", "builder"), ("grp-peer", "Tester"), ("grp-far", "docs")].map(|(k, v)| (k.to_string(), v.to_string())));
            r.groups.extend([("grp-me", 1), ("grp-peer", 1), ("grp-far", 0)].map(|(k, v)| (k.to_string(), v)));
        }

        let ids = |caller: &str, f: SessionFilter| -> Vec<String> {
            let mut v: Vec<String> =
                sessions(caller, &f).unwrap().into_iter().map(|s| s.id).filter(|id| id.starts_with("grp-")).collect();
            v.sort();
            v
        };
        let by = |g: serde_json::Value| SessionFilter { group: Some(g), q: None, history: None };
        // Groups go out 1-based, as the title bar counts them.
        assert_eq!(ids("grp-me", by(serde_json::json!("mine"))), ["grp-me", "grp-peer"]);
        assert_eq!(ids("grp-me", by(serde_json::json!(1))), ["grp-far"]);
        assert_eq!(ids("grp-me", by(serde_json::json!("2"))), ["grp-me", "grp-peer"]);
        assert!(sessions("grp-loose", &by(serde_json::json!("mine"))).is_err(), "a session in no group has no mine");
        assert!(sessions("grp-me", &by(serde_json::json!(0))).is_err());
        let q = |text: &str| SessionFilter { group: None, q: Some(text.into()), history: None };
        assert_eq!(ids("grp-me", q("tester")), ["grp-peer"], "name, any case");
        assert_eq!(ids("grp-me", q("asuri")), ["grp-peer"], "folder");
        assert_eq!(ids("grp-me", SessionFilter { group: Some(serde_json::json!("mine")), q: Some("build".into()), history: None }), ["grp-me"]);
    }

    #[test]
    fn lineage_and_reach() {
        let none = serde_json::json!({});
        let a = register("a", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone()).expect("token");
        assert_eq!(caller_of(&a).as_deref(), Some("a"));
        register("b", Provider::Codex, "C:\\p", "C:\\acc\\openai\\two", Some("a"), false, none.clone());
        register("c", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("b"), false, none.clone());
        register("x", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, false, none);
        assert!(authorize("a", "b").is_ok());
        assert!(authorize("a", "c").is_ok(), "grandchildren are reachable");
        assert!(authorize("b", "a").is_err(), "not upwards");
        assert!(authorize("a", "x").is_err(), "not the user's own chats");
        assert!(authorize("a", "a").is_err(), "not itself");
        assert!(authorize_send("a", "x").is_ok(), "text may go to the user's own chats");
        assert!(authorize_send("b", "a").is_ok(), "and upwards");
        assert!(authorize_send("a", "a").is_err(), "but not into its own composer");
        assert!(authorize_send("a", "nobody").is_err());
        assert!(authorize_view("b", "a").is_ok(), "a session written to can be read and waited on");
        assert!(authorize_view("a", "a").is_err(), "but not one's own");
        let r = reg().lock().unwrap();
        assert_eq!(depth_of(&r, "c"), 2);
        let mut d = descendants(&r, "a");
        d.sort();
        assert_eq!(d, vec!["b", "c"]);
    }

    #[test]
    fn an_answer_is_owed_until_the_caller_sees_a_later_turn() {
        let none = serde_json::json!({});
        register("o-parent", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone());
        register("o-child", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("o-parent"), false, none);
        let owed = || reg().lock().unwrap().owed.get(&("o-parent".to_string(), "o-child".to_string())).map(|o| o.baseline);
        owe_from("o-parent", "o-child", 1, 0);
        shown("o-parent", "o-child", 1);
        assert_eq!(owed(), Some(1), "the send itself shows the count it went in at");
        shown("o-parent", "o-child", 2);
        assert_eq!(owed(), None, "a wait or read past it settles the answer");
        owe_from(DEV_CALLER, "o-child", 0, 0);
        assert!(reg().lock().unwrap().owed.get(&(DEV_CALLER.to_string(), "o-child".to_string())).is_none());
    }

    #[test]
    fn asked_again_the_earlier_answer_is_still_owed() {
        let key = ("m-parent".to_string(), "m-child".to_string());
        let owed = || reg().lock().unwrap().owed.get(&key).map(|o| (o.baseline, o.cursor));
        owe_from("m-parent", "m-child", 1, 100);
        owe_from("m-parent", "m-child", 2, 300);
        assert_eq!(owed(), Some((2, 100)), "the second answer ends a later turn; both are read from the first question on");
        reg().lock().unwrap().owed.remove(&key);
        owe_from("m-parent", "m-child", 3, 500);
        assert_eq!(owed(), Some((3, 500)), "once collected, a new question starts where it went in");
    }

    #[test]
    fn a_hand_over_undone_puts_back_what_it_took() {
        let key = ("h-parent".to_string(), "h-child".to_string());
        owe_from("h-parent", "h-child", 1, 40);
        seen("h-parent", "h-child", 1);
        let undo = hand_over("h-parent", "h-child", 2, None);
        assert!(reg().lock().unwrap().owed.get(&key).is_none(), "handed over");
        assert_eq!(last_shown("h-parent", "h-child"), Some(2));
        undo();
        assert_eq!(reg().lock().unwrap().owed.get(&key).map(|o| (o.baseline, o.cursor)), Some((1, 40)));
        assert_eq!(last_shown("h-parent", "h-child"), Some(1));
        // A question asked in between stands, owing the dropped answer too.
        let undo = hand_over("h-parent", "h-child", 2, None);
        owe_from("h-parent", "h-child", 2, 90);
        seen("h-parent", "h-child", 2);
        undo();
        assert_eq!(reg().lock().unwrap().owed.get(&key).map(|o| (o.baseline, o.cursor)), Some((2, 40)));
        reg().lock().unwrap().owed.remove(&key);
    }

    #[test]
    fn a_dropped_response_does_not_take_back_what_a_later_call_delivered() {
        let key = ("hr-parent".to_string(), "hr-child".to_string());
        owe_from("hr-parent", "hr-child", 1, 40);
        let dropped = hand_over("hr-parent", "hr-child", 2, None);
        let read = hand_over("hr-parent", "hr-child", 2, None);
        dropped();
        assert!(reg().lock().unwrap().owed.get(&key).is_none(), "the later read delivered it");
        assert_eq!(last_shown("hr-parent", "hr-child"), Some(2));
        read();
        forget("hr-parent");
        forget("hr-child");
    }

    #[test]
    fn a_cut_read_goes_on_where_the_kept_messages_end() {
        let path = std::env::temp_dir().join(format!("luna-cursor-after-{}.jsonl", std::process::id()));
        std::fs::write(&path, "a\nb\nc\n").unwrap();
        // One message a line, as the transcript readers count them.
        let messages_at = |at: u64| -> Messages {
            let text = std::fs::read_to_string(&path).unwrap();
            let rest = &text[(at as usize).min(text.len())..];
            (rest.lines().map(|l| Message { role: "assistant".into(), text: l.into() }).collect(), text.len() as u64)
        };
        assert_eq!(cursor_after(&path, 0, 6, 3, 1, &messages_at), 2);
        assert_eq!(cursor_after(&path, 2, 6, 2, 1, &messages_at), 4);
        let _ = std::fs::remove_file(&path);
        let nothing = |at: u64| -> Messages { (Vec::new(), at) };
        assert_eq!(cursor_after(&path, 2, 6, 2, 1, &nothing), 2, "unreadable: this page over, not past the rest");
    }

    #[test]
    fn a_queued_question_settles_after_a_later_turn_or_the_grace() {
        let key = ("q-parent".to_string(), "q-child".to_string());
        owe_from("q-parent", "q-child", 4, 0);
        let o = || reg().lock().unwrap().owed.get(&key).copied().unwrap();
        assert!(!queue_settled(&key, o(), 5), "the turn it was typed into may be followed by its own");
        assert!(!queue_settled(&key, o(), 5), "not yet");
        reg().lock().unwrap().owed.get_mut(&key).unwrap().held = Some((5, now_ms() - QUEUED_GRACE_MS));
        assert!(queue_settled(&key, o(), 5), "no turn began on it in the grace");
        assert!(queue_settled(&key, o(), 6), "a turn past the running one ended");
        reg().lock().unwrap().owed.remove(&key);
    }

    #[test]
    fn an_answer_file_puts_the_report_first_and_the_last_word_next() {
        let read = |line: String| {
            let file = line.trim_start_matches("the answer is in ").trim_end_matches(", its report first").to_string();
            let whole = std::fs::read_to_string(&file).unwrap();
            let _ = std::fs::remove_file(&file);
            whole
        };
        assert_eq!(read(answer_file("q", &["Done.".into()])), "Done.");
        let replies = vec![
            "Let me look.".to_string(),
            format!("REPORT: the long one, with what was done. {}", "Details. ".repeat(50)),
            "Now the tests.".to_string(),
            "Nothing else changed.".to_string(),
        ];
        let whole = read(answer_file("q", &replies));
        assert!(whole.starts_with("REPORT: the long one"), "the report leads: {whole}");
        let last = whole.find("Nothing else changed.").unwrap();
        assert!(last < whole.find("Let me look.").unwrap(), "the last word before the progress lines");
        assert!(whole.find("Let me look.").unwrap() < whole.find("Now the tests.").unwrap(), "the rest in order");
    }

    /// As Codex helper qa-r2 put it: a progress line every so often while its
    /// command ran, then the answer, shorter than most of them.
    #[test]
    fn with_every_message_short_the_last_leads() {
        let replies: Vec<String> = [
            "I’ll run the ping in the foreground and wait for it to finish.",
            "The restart disconnected the shell session. I’m checking the running ping processes so I can wait for completion.",
            "The ping remains active.",
            "I’m waiting for the ping process to exit.",
            "R2 DONE FOURTH\nKIWI",
        ]
        .map(String::from)
        .to_vec();
        let whole = answer_text(&replies);
        assert!(whole.starts_with("R2 DONE FOURTH\nKIWI\n\n---\n\nEverything else it said, in order:\n\nI’ll run the ping"), "{whole}");
        assert!(!whole.contains("Its last message"), "it leads already");
        assert_eq!(begins(&replies), ". It begins: \"R2 DONE FOURTH\"");
        // One that is not short is the report, wherever it is.
        let mut with_report = replies.clone();
        with_report.insert(1, format!("Findings: {}", "a finding. ".repeat(40)));
        assert!(answer_text(&with_report).starts_with("Findings:"));
    }

    #[test]
    fn the_same_answer_is_the_same_file() {
        let path = |replies: &[String]| answer_file("same", replies).trim_start_matches("the answer is in ").trim_end_matches(", its report first").to_string();
        let replies = vec!["The report.".to_string()];
        let (first, again) = (path(&replies), path(&replies));
        assert_eq!(first, again, "a notice a draft holds back writes no new file at every look");
        let other = path(&["Another report.".to_string()]);
        assert_ne!(first, other, "a different answer is another file");
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "The report.");
        // Named again, it is as good as new for the week's sweep.
        let week_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(8 * 24 * 3600);
        std::fs::File::options().write(true).open(&first).unwrap().set_modified(week_ago).unwrap();
        assert_eq!(path(&replies), first);
        let age = std::fs::metadata(&first).unwrap().modified().unwrap().elapsed().unwrap();
        assert!(age < std::time::Duration::from_secs(60), "{age:?}");
        // Gone meanwhile (the week's sweep), it is written again.
        std::fs::remove_file(&first).unwrap();
        assert_eq!(path(&replies), first);
        assert!(std::path::Path::new(&first).is_file());
        let _ = std::fs::remove_file(&first);
        let _ = std::fs::remove_file(&other);
    }

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
    fn a_worktree_still_being_made_says_so() {
        let spawned = |worktree: Worktree, worktree_pending: bool| {
            serde_json::to_value(Spawned {
                account_choice: None,
                id: "w".into(),
                turns_ended: 0,
                settings: serde_json::Value::Null,
                started: true,
                screen: None,
                ask: None,
                starting: false,
                warnings: Vec::new(),
                same_folder: Vec::new(),
                worktree,
                worktree_pending,
            })
            .unwrap()
        };
        let pending = spawned(Worktree::default(), true);
        assert_eq!(pending["worktreePending"], true);
        assert!(pending.get("worktree").is_none());
        let made = spawned(Worktree { path: Some("C:\\w".into()), branch: None }, false);
        assert_eq!(made["worktree"], "C:\\w");
        assert!(made.get("worktreePending").is_none());
    }

    #[test]
    fn a_notice_quotes_the_first_line_of_the_report_alone() {
        assert_eq!(opening_line("## **Done**: all `tests` pass\n\nDetails"), Some("Done: all tests pass".into()));
        assert_eq!(opening_line("---\n\n# \n> *Fixed the bug*"), Some("Fixed the bug".into()), "rules and empty headings say nothing");
        assert_eq!(opening_line("- first point\n- second"), Some("first point".into()));
        assert_eq!(opening_line("```rust\nfn main() {}\n```\nAfter the code"), Some("After the code".into()));
        assert_eq!(opening_line("|---|---|\n| a | b |"), Some("| a | b |".into()));
        assert_eq!(opening_line("  \n---\n"), None);
        assert_eq!(opening_line("bell\u{7} in\tit"), Some("bell in it".into()));

        let long = "word ".repeat(100);
        let cut = opening_line(&long).unwrap();
        assert!(cut.ends_with("word…"), "{cut}");
        assert!(cut.chars().count() <= NOTICE_QUOTE + 1, "{}", cut.chars().count());

        let report = format!("# Report\nThe long one, with what was done. {}", "Details. ".repeat(50));
        let replies = vec!["Let me look.".to_string(), report, "Nothing else.".to_string()];
        assert_eq!(begins(&replies), ". It begins: \"Report\"", "the longest message, as the file leads with it");
        assert!(!begins(&replies).contains('\n'));
        assert_eq!(begins(&["---".to_string()]), "", "nothing worth quoting, no clause");
        assert_eq!(begins(&[]), "");
    }

    #[test]
    fn a_window_is_told_once_per_level_until_it_resets() {
        let now = now_ms();
        let later = crate::background::iso(now + 3_600_000).unwrap();
        let later = Some(later.as_str());
        let told = |level: u8| QuotaTold { level, resets_at: later.map(str::to_owned) };

        assert_eq!(quota_step(None, 89.0, later, now), (0, false));
        assert_eq!(quota_step(None, 90.0, later, now), (90, true));
        assert_eq!(quota_step(Some(&told(90)), 91.0, later, now), (90, false), "once per level");
        assert_eq!(quota_step(Some(&told(90)), 95.0, later, now), (95, true));
        assert_eq!(quota_step(Some(&told(95)), 99.0, later, now), (95, false));
        assert_eq!(quota_step(None, 97.0, later, now), (95, true), "a jump past both is one line");
        assert_eq!(quota_step(Some(&told(95)), 93.0, later, now), (95, false), "a point or two down is rounding");
        assert_eq!(quota_step(Some(&told(90)), 3.0, later, now), (0, false), "reset: forgotten");

        // A new window that is already high, seen only by its reset time.
        let gone = crate::background::iso(now - 60_000).unwrap();
        let old = QuotaTold { level: 95, resets_at: Some(gone.clone()) };
        assert_eq!(quota_step(Some(&old), 96.0, later, now), (95, true), "told again for the new window");
        // Numbers of a window already over say nothing.
        assert_eq!(quota_step(Some(&old), 96.0, Some(&gone), now), (0, false));
        assert_eq!(quota_step(None, 96.0, Some(&gone), now), (0, false));
        // A reset time moving about within a window is not a new one.
        let drift = crate::background::iso(now + 3_700_000).unwrap();
        assert_eq!(quota_step(Some(&told(95)), 96.0, Some(&drift), now), (95, false));
    }

    #[test]
    fn quota_news_goes_to_whoever_a_live_helper_runs_under() {
        let none = serde_json::json!({});
        let acc = "C:/acc/anthropic/qn-hot";
        register("qn-top", Provider::Claude, "C:/p", "C:/acc/anthropic/qn-cold", None, true, none.clone());
        register("qn-mid", Provider::Claude, "C:/p", "C:/acc/anthropic/qn-cold", Some("qn-top"), true, none.clone());
        register("qn-low", Provider::Claude, "C:/p", acc, Some("qn-mid"), false, none.clone());
        register("qn-dead", Provider::Claude, "C:/p", "C:/acc/anthropic/qn-idle", Some("qn-top"), false, none);
        crate::background::test_helper("qn-low", acc, None, false);
        let pty = crate::pty::manager();
        let limits = |pct: u8| serde_json::json!({ "limits": [{ "label": "week", "usedPct": pct, "resetsAt": null }] });
        let set = |pct: u8| {
            let mut r = reg().lock().unwrap();
            r.usage.insert("claude/qn-hot".into(), limits(pct));
            r.usage.insert("claude/qn-idle".into(), limits(99));
        };
        set(92);
        let news = quota_news(&pty);
        for caller in ["qn-top", "qn-mid"] {
            let lines = &news[caller];
            let hot: Vec<&QuotaNews> = lines.iter().filter(|n| n.key.1 == "claude/qn-hot").collect();
            assert_eq!(hot.len(), 1, "{caller}");
            assert!(hot[0].line.contains("has used 92% of its \"week\" window"), "{}", hot[0].line);
        }
        assert!(!news.values().flatten().any(|n| n.key.1 == "claude/qn-idle"), "no live helper of its runs there");
        assert!(!news.contains_key("qn-low"), "a session does not hear of its own account");

        // Not told until the notice has gone in: asked again until it has.
        assert!(quota_news(&pty).contains_key("qn-top"));
        for n in news.into_values().flatten() {
            reg().lock().unwrap().quota_told.insert(n.key, n.told);
        }
        assert!(!quota_news(&pty).contains_key("qn-top"), "once");
        set(96);
        assert_eq!(quota_news(&pty)["qn-top"][0].told.level, 95, "the next level");
        forget("qn-top");
        forget("qn-mid");
        assert!(!reg().lock().unwrap().quota_told.keys().any(|(c, _, _)| c == "qn-top" || c == "qn-mid"));
        crate::background::remove("qn-low");
        forget("qn-low");
        forget("qn-dead");
        let mut r = reg().lock().unwrap();
        r.usage.remove("claude/qn-hot");
        r.usage.remove("claude/qn-idle");
    }

    /// A caller and two background helpers of its, one whose answer is on
    /// disk and owed, the other in the middle of a turn.
    fn several(prefix: &str) -> (String, String, String, std::path::PathBuf) {
        let none = serde_json::json!({});
        let dir = std::env::temp_dir().join(format!("luna-wait-many-{prefix}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let account = dir.to_string_lossy().into_owned();
        let (caller, done, busy) = (format!("{prefix}-caller"), format!("{prefix}-done"), format!("{prefix}-busy"));
        register(&caller, Provider::Claude, &account, &account, None, true, none.clone());
        crate::background::test_helper(&caller, &account, None, false);
        for (id, sid, working) in [(&done, "sid-done", false), (&busy, "sid-busy", true)] {
            register(id, Provider::Claude, &account, &account, Some(&caller), false, none.clone());
            crate::background::test_helper(id, &account, Some(sid), working);
            let path = crate::claude::session::transcript_file(&account, &account, sid);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, format!("{}\n", serde_json::json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": format!("answer of {sid}") }] } }))).unwrap();
            owe_from(&caller, id, 0, 0);
        }
        crate::background::test_turn_ended(&done);
        (caller, done, busy, dir)
    }

    fn leave(ids: &[&String], dir: &std::path::Path) {
        for id in ids {
            crate::background::remove(id);
            forget(id);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    fn owed_to(caller: &str, id: &str) -> bool {
        reg().lock().unwrap().owed.contains_key(&(caller.to_string(), id.to_string()))
    }

    #[test]
    fn waiting_on_any_hands_over_the_first_and_leaves_the_rest_owed() {
        let (caller, done, busy, dir) = several("any");
        let call = crate::mcp::Call::new();
        let w = wait_many(&caller, &[done.clone(), busy.clone(), done.clone()], false, "turn_done", 20, &call).unwrap();
        assert_eq!(w.outcome, "finished");
        assert_eq!(w.finished, [done.clone()]);
        assert_eq!(w.results.len(), 2, "an id asked for twice is one");
        assert_eq!(w.results[0].waited.outcome, "turn_done");
        assert_eq!(w.results[0].waited.reply.as_deref(), Some("answer of sid-done"));
        assert_eq!(w.results[1].waited.outcome, "pending");
        assert_eq!(w.results[1].waited.reply, None);
        assert_eq!(w.results[1].waited.state, "working");
        assert!(!owed_to(&caller, &done), "handed over: no notice follows");
        assert!(owed_to(&caller, &busy), "the other is still owed");
        let v = serde_json::to_value(&w).unwrap();
        assert_eq!(v["results"][0]["id"], serde_json::json!(done));
        assert_eq!(v["results"][0]["turnsEnded"], 1, "the shape one wait returns");

        // Two done at once: neither is passed over for the other.
        crate::background::test_turn_ended(&busy);
        owe_from(&caller, &done, 1, 0);
        crate::background::test_helper(&done, &dir.to_string_lossy(), Some("sid-done"), false);
        crate::background::test_turn_ended(&done);
        crate::background::test_turn_ended(&done);
        let call = crate::mcp::Call::new();
        let w = wait_many(&caller, &[done.clone(), busy.clone()], false, "turn_done", 20, &call).unwrap();
        assert_eq!(w.finished, [done.clone(), busy.clone()]);
        assert!(!owed_to(&caller, &done) && !owed_to(&caller, &busy));
        leave(&[&caller, &done, &busy], &dir);
    }

    #[test]
    fn waiting_on_all_goes_on_until_each_has_finished_or_the_time_is_up() {
        let (caller, done, busy, dir) = several("all");
        let ids = [done.clone(), busy.clone()];

        // While the other runs, what has come is still owed, as a core going
        // now would save it, and held from the notice.
        let call = std::sync::Arc::new(crate::mcp::Call::new());
        let waiting = {
            let (caller, ids, call) = (caller.clone(), ids.clone(), call.clone());
            std::thread::spawn(move || wait_many(&caller, &ids, true, "turn_done", 3, &call).is_ok())
        };
        std::thread::sleep(std::time::Duration::from_millis(1500));
        assert!(owed_to(&caller, &done));
        assert_eq!(ties_of(&done).unwrap().owed.len(), 1, "saved with the helper");
        assert!(last_shown(&caller, &done) != Some(1), "not seen yet either");
        let o = reg().lock().unwrap().owed.get(&(caller.clone(), done.clone())).copied().unwrap();
        reg().lock().unwrap().owed.get_mut(&(caller.clone(), done.clone())).unwrap().ready_at = Some(now_ms() - WAIT_HOLD_MS - 1);
        assert!(held_for_wait(&crate::pty::manager(), &caller, &done, &o), "past the hold of a wait on one");
        assert!(waiting.join().unwrap());
        assert!(!owed_to(&caller, &done), "handed over with the response");
        reg().lock().unwrap().seen.remove(&(caller.clone(), done.clone()));
        crate::background::test_helper(&done, &dir.to_string_lossy(), Some("sid-done"), false);
        crate::background::test_turn_ended(&done);
        owe_from(&caller, &done, 0, 0);

        // The time runs out on one: what came is handed over, the other is
        // a timeout with its screen, still owed.
        let call = crate::mcp::Call::new();
        let w = wait_many(&caller, &ids, true, "turn_done", 1, &call).unwrap();
        assert_eq!(w.outcome, "timeout");
        assert_eq!(w.finished, [done.clone()]);
        assert_eq!(w.results[0].waited.reply.as_deref(), Some("answer of sid-done"));
        assert_eq!(w.results[1].waited.outcome, "timeout");
        assert!(!owed_to(&caller, &done));
        assert!(owed_to(&caller, &busy));

        // The other ends a little later: the one already seen does not count
        // again, so the wait is for the turn still running.
        let (end, id) = (busy.clone(), busy.clone());
        crate::background::test_helper(&id, &dir.to_string_lossy(), Some("sid-busy"), true);
        let ender = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(1500));
            crate::background::test_turn_ended(&end);
        });
        crate::background::test_helper(&done, &dir.to_string_lossy(), Some("sid-done"), false);
        crate::background::test_turn_ended(&done);
        reg().lock().unwrap().seen.remove(&(caller.clone(), done.clone()));
        owe_from(&caller, &done, 0, 0);
        let call = crate::mcp::Call::new();
        let w = wait_many(&caller, &ids, true, "turn_done", 20, &call).unwrap();
        ender.join().unwrap();
        assert_eq!(w.outcome, "finished");
        assert_eq!(w.finished, ids);
        assert!(w.results.iter().all(|r| r.waited.outcome == "turn_done" && r.waited.reply.is_some()));
        assert!(!owed_to(&caller, &done) && !owed_to(&caller, &busy));
        leave(&[&caller, &done, &busy], &dir);
    }

    #[test]
    fn a_wait_on_several_that_is_cancelled_hands_nothing_over() {
        // Before it answers: the hand-over made while another ran is undone.
        let (caller, done, busy, dir) = several("cxl");
        let ids = [done.clone(), busy.clone()];
        let call = std::sync::Arc::new(crate::mcp::Call::new());
        let canceller = call.clone();
        let cancelling = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(1500));
            canceller.cancel();
        });
        assert!(wait_many(&caller, &ids, true, "turn_done", 20, &call).is_err());
        cancelling.join().unwrap();
        assert!(owed_to(&caller, &done), "still owed, for the notice");
        assert!(owed_to(&caller, &busy));
        assert!(last_shown(&caller, &done).is_none());

        // Crossing the response: the same, after it has been handed over.
        let call = crate::mcp::Call::new();
        let w = wait_many(&caller, &ids, false, "turn_done", 20, &call).unwrap();
        assert_eq!(w.finished, [done.clone()]);
        assert!(!owed_to(&caller, &done));
        call.cancel();
        assert!(owed_to(&caller, &done), "dropped unread: owed again");

        // Another call collects it during the wait, which is then cancelled:
        // nothing comes back, as nothing was the wait's to take.
        let cancelled = std::sync::Arc::new(crate::mcp::Call::new());
        let waiting = {
            let (caller, ids, call) = (caller.clone(), ids.clone(), cancelled.clone());
            std::thread::spawn(move || wait_many(&caller, &ids, true, "turn_done", 20, &call).is_err())
        };
        std::thread::sleep(std::time::Duration::from_millis(1200));
        let _read = hand_over(&caller, &done, 1, None);
        cancelled.cancel();
        assert!(waiting.join().unwrap());
        assert!(!owed_to(&caller, &done), "the read delivered it");
        leave(&[&caller, &done, &busy], &dir);
    }

    #[test]
    fn a_long_answer_is_cut_once_and_its_file_has_the_whole() {
        let (caller, done, busy, dir) = several("long");
        let account = dir.to_string_lossy().into_owned();
        let long = "z".repeat(MAX_TEXT + 500);
        let path = crate::claude::session::transcript_file(&account, &account, "sid-done");
        std::fs::write(&path, format!("{}\n", serde_json::json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": long }] } }))).unwrap();
        crate::background::test_turn_ended(&busy);
        let call = crate::mcp::Call::new();
        let w = wait_many(&caller, &[done.clone()], false, "turn_done", 20, &call).unwrap();
        let reply = w.results[0].waited.reply.as_ref().unwrap();
        assert_eq!(reply.matches("[cut at").count(), 1, "once");
        let file = reply.rsplit("the answer is in ").next().unwrap().trim_end_matches(", its report first]");
        assert_eq!(std::fs::read_to_string(file).unwrap(), long, "the whole of it");
        let _ = std::fs::remove_file(file);
        leave(&[&caller, &done, &busy], &dir);
    }

    #[test]
    fn answer_files_never_share_a_name() {
        let line = |text: &str| answer_file("same-ms", &[text.to_string()]);
        let lines: Vec<String> = (0..20).map(|i| line(&format!("answer {i}"))).collect();
        for (i, l) in lines.iter().enumerate() {
            let file = l.trim_start_matches("the answer is in ").trim_end_matches(", its report first");
            assert_eq!(std::fs::read_to_string(file).unwrap(), format!("answer {i}"));
            let _ = std::fs::remove_file(file);
        }
    }

    #[test]
    fn what_a_caller_was_told_of_quota_outlives_the_core() {
        let now = now_ms();
        let later = crate::background::iso(now + 3_600_000);
        let gone = crate::background::iso(now - 60_000);
        let mut told = HashMap::new();
        let key = |l: &str| ("c".to_string(), "claude/a".to_string(), l.to_string());
        told.insert(key("week"), QuotaTold { level: 95, resets_at: later.clone() });
        told.insert(key("5 hours"), QuotaTold { level: 90, resets_at: gone });
        let back = quota_from(&quota_bytes(&told), now);
        assert_eq!(back.len(), 1, "a window over since is forgotten");
        assert_eq!(back[&key("week")], QuotaTold { level: 95, resets_at: later.clone() });
        let (level, say) = quota_step(back.get(&key("week")), 96.0, later.as_deref(), now);
        assert_eq!((level, say), (95, false), "not told again");
        assert!(quota_from(b"not json", now).is_empty());
    }

    #[test]
    fn a_wait_on_several_is_refused_whole() {
        let none = serde_json::json!({});
        register("mw-me", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none);
        let call = crate::mcp::Call::new();
        assert!(wait_many("mw-me", &[], false, "turn_done", 1, &call).err().unwrap().contains("empty"));
        assert!(wait_many("mw-me", &["nobody".into()], false, "turn_done", 1, &call).err().unwrap().contains("unknown session"));
        assert!(wait_many("mw-me", &["mw-me".into()], false, "turn_done", 1, &call).err().unwrap().contains("your own"));
        assert!(wait_many("mw-me", &["x".into()], false, "soon", 1, &call).err().unwrap().contains("unknown until"));
        forget("mw-me");
    }

    #[test]
    fn several_long_replies_share_one_result() {
        let one = |id: &str, reply: Option<String>| WaitedOne {
            id: id.into(),
            waited: Waited { outcome: "turn_done", state: "idle", ask: None, turns_ended: 1, reply, failed: None, transcript_missing: false, screen: None, undelivered: None, job_debt: None },
        };
        let mut results = [
            one("s-short", Some("short".into())),
            one("s-none", None),
            one("s-long", Some("x".repeat(MAX_TEXT))),
            one("s-long2", Some("y".repeat(MAX_TEXT))),
        ];
        share_replies(&mut results);
        assert_eq!(results[0].waited.reply.as_deref(), Some("short"), "room left for the short one whole");
        assert_eq!(results[1].waited.reply, None);
        for r in &results[2..] {
            let text = r.waited.reply.as_ref().unwrap();
            assert!(text.contains("several sessions sharing this result"), "{}", &text[text.len() - 80..]);
            let file = text.rsplit("the answer is in ").next().unwrap().trim_end_matches(", its report first]");
            assert_eq!(std::fs::read_to_string(file).unwrap().chars().count(), MAX_TEXT, "the whole of it");
            let _ = std::fs::remove_file(file);
        }
        let total: usize = results.iter().filter_map(|r| r.waited.reply.as_ref()).map(|t| t.chars().count()).sum();
        assert!(total < MAX_TEXT + 1000, "{total}");

        let mut small = [one("s-a", Some("a".repeat(100))), one("s-b", Some("b".repeat(100)))];
        share_replies(&mut small);
        assert_eq!(small[0].waited.reply.as_ref().unwrap().len(), 100, "what fits is left alone");
    }

    #[test]
    fn a_failed_turn_quotes_nothing_as_its_verdict() {
        use crate::activity::{on_hook, HookEvent};
        let path = std::env::temp_dir().join(format!("luna-verdict-{}.jsonl", std::process::id()));
        std::fs::write(&path, "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"All done and fixed.\"}]}}\n").unwrap();
        for (id, failed) in [("vd-fine", false), ("vd-failed", true)] {
            register(id, Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, false, serde_json::json!({}));
            transcripts().lock().unwrap().insert(id.into(), path.clone());
            on_hook(id, HookEvent::Busy);
            on_hook(id, if failed { HookEvent::Failed("rate_limit") } else { HookEvent::Idle });
        }
        let fine = line_for("vd-fine");
        assert!(fine.contains("finished its turn") && fine.contains("It begins: \"All done and fixed.\""), "{fine}");
        let failed = line_for("vd-failed");
        assert!(failed.contains("stopped on an error: rate_limit"), "{failed}");
        assert!(failed.contains("It ended with 1 message"), "what it said before is still named: {failed}");
        assert!(!failed.contains("It begins") && !failed.contains("All done"), "its words are no verdict on it: {failed}");
        for id in ["vd-fine", "vd-failed"] {
            forget(id);
            crate::activity::forget(id);
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_wait_on_several_hands_a_jobs_end_over_with_its_report() {
        let (caller, done, busy, dir) = several("jd");
        let account = dir.to_string_lossy().into_owned();
        let path = crate::claude::session::transcript_file(&account, &account, "sid-done");
        let before = "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"Started the job.\"}]}}\n";
        let after = "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"The real results.\"}]}}\n";
        std::fs::write(&path, format!("{before}{after}")).unwrap();
        // Told it is not done, its job over, the grace stood: `busy` still works.
        let key = (caller.clone(), done.clone());
        let turns = crate::activity::turn_state(&done).map_or(0, |t| t.turns_ended);
        let told = Owed::pending(turns, before.len() as u64);
        {
            let mut r = reg().lock().unwrap();
            r.owed.insert(key.clone(), told);
            r.owed.get_mut(&key).unwrap().held = Some((turns, now_ms() - BACKGROUND_GRACE_MS));
        }
        let call = crate::mcp::Call::new();
        let w = wait_many(&caller, &[done.clone(), busy.clone()], false, "turn_done", 20, &call).unwrap();
        assert_eq!(w.finished, [done.clone()]);
        let one = &w.results[0].waited;
        assert_eq!(one.outcome, "job_done");
        assert_eq!(one.reply.as_deref(), Some("The real results."), "what it said since, not an old message");
        assert!(!owed_to(&caller, &done), "the job's end settles the debt it ended");
        assert_eq!(w.results[1].waited.outcome, "pending");
        leave(&[&caller, &done, &busy], &dir);
    }

    #[test]
    fn a_wrapped_warning_comes_back_whole() {
        let screen = " ▐▛███▛█   Claude Code\n\n⚠ Model \"haiku\" is restricted by your organization's\n  settings. Using claude-sonnet-5-5 instead.\n\n● Unknown command: /x\n";
        assert_eq!(
            warnings_in(screen),
            ["⚠ Model \"haiku\" is restricted by your organization's settings. Using claude-sonnet-5-5 instead."]
        );
        assert!(warnings_in("● all fine\n").is_empty());
    }

    #[test]
    fn a_wait_out_holds_the_notice_for_a_while() {
        let none = serde_json::json!({});
        register("w-parent", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone());
        register("w-child", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("w-parent"), false, none);
        owe_from("w-parent", "w-child", 0, 0);
        let pty = crate::pty::manager();
        let key = ("w-parent".to_string(), "w-child".to_string());
        let o = || reg().lock().unwrap().owed.get(&key).copied().unwrap();
        assert!(!held_for_wait(&pty, "w-parent", "w-child", &o()), "no wait out, nothing held");
        {
            let _out = WaitOut::start("w-parent", "w-child");
            // Not running, so there is news: the wait has WAIT_HOLD_MS to say it.
            assert!(held_for_wait(&pty, "w-parent", "w-child", &o()));
            reg().lock().unwrap().owed.get_mut(&key).unwrap().ready_at = Some(now_ms() - WAIT_HOLD_MS - 1);
            assert!(!held_for_wait(&pty, "w-parent", "w-child", &o()), "a wait that sat on news is let go");
        }
        assert!(!reg().lock().unwrap().waits.contains_key(&key), "the count goes with the call");
        {
            // A wait on several holds it however long the others take.
            let _reserved = Reserved::start("w-parent", "w-child");
            assert!(held_for_wait(&pty, "w-parent", "w-child", &o()));
        }
        assert!(!held_for_wait(&pty, "w-parent", "w-child", &o()), "and no longer");
    }

    #[test]
    fn an_auto_delete_helper_counts_idle_only_once_its_answer_is_out() {
        let none = serde_json::json!({});
        register("ad-parent", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone());
        register("ad-child", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("ad-parent"), false, none);
        reg().lock().unwrap().auto.insert("ad-child".into(), Auto::default());
        let since = || reg().lock().unwrap().auto.get("ad-child").and_then(|a| a.idle_since);
        let pty = crate::pty::manager();
        owe_from("ad-parent", "ad-child", 0, 0);
        reap(&pty);
        assert_eq!(since(), None, "its answer is still owed");
        reg().lock().unwrap().owed.remove(&("ad-parent".to_string(), "ad-child".to_string()));
        reap(&pty);
        assert!(since().is_some(), "exited and nobody owed: the clock runs");
        owe_from("ad-parent", "ad-child", 1, 0);
        reap(&pty);
        assert_eq!(since(), None, "a follow-up starts it over");
        reg().lock().unwrap().auto.remove("ad-child");
    }

    /// The loop of that night: two chats that wrote to each other, a stray
    /// process under each. Every notice starts a turn in the chat it goes into,
    /// and that turn's end is what the other is owed. Driven by the notifier's
    /// own decisions, with a turn counted the moment its notice goes in.
    #[test]
    fn two_chats_with_a_job_each_do_not_wake_each_other_forever() {
        use crate::activity::Turn;
        let (a, b) = ("pp-a", "pp-b");
        let key = |caller: &str, id: &str| (caller.to_string(), id.to_string());
        let mut turns: HashMap<&str, u32> = HashMap::from([(a, 1), (b, 1)]);
        owe_from(a, b, 0, 0);
        owe_from(b, a, 0, 0);
        let mut notices = 0;
        // One pass over both debts, with or without the jobs; returns how many
        // notices went.
        let pass = |job: bool, turns: &mut HashMap<&str, u32>| {
            let mut sent = 0;
            for (caller, id) in [(a, b), (b, a)] {
                let Some(o) = reg().lock().unwrap().owed.get(&key(caller, id)).copied() else { continue };
                let ended = turns[id];
                let v = verdict(&o, ended, Some(Turn::Idle), job, real_turn_past(id, caller, o.baseline, ended, false, || true));
                let settled = match v {
                    Verdict::OnlyNotices => {
                        reg().lock().unwrap().owed.get_mut(&key(caller, id)).unwrap().baseline = ended;
                        continue;
                    }
                    Verdict::TurnOver if job => Settled::Pending(ended, 0),
                    Verdict::TurnOver | Verdict::JobOver => Settled::Answered,
                    Verdict::Nothing | Verdict::AtWork => continue,
                };
                sent += 1;
                let mut r = reg().lock().unwrap();
                notice_turn(&mut r, caller, turns[caller] + 1, &[id.to_string()]);
                match settled {
                    Settled::Pending(n, c) => {
                        r.owed.insert(key(caller, id), Owed::pending(n, c));
                    }
                    _ => {
                        r.owed.remove(&key(caller, id));
                    }
                }
                drop(r);
                *turns.get_mut(caller).unwrap() += 1;
            }
            sent
        };
        for _ in 0..500 {
            notices += pass(true, &mut turns);
        }
        assert_eq!(notices, 2, "\"NOT done yet\" once per debt, whatever the turns on those notices");
        let at_rest = turns.clone();
        for _ in 0..500 {
            notices += pass(true, &mut turns);
        }
        assert_eq!(turns, at_rest, "no notice, no turn: the chats are left at rest");
        // The jobs end: each is told once, and that is the end of it.
        for _ in 0..500 {
            notices += pass(false, &mut turns);
        }
        assert_eq!(notices, 4, "the job's end is news, once per debt");
        assert!(reg().lock().unwrap().owed.get(&key(a, b)).is_none());
        assert!(reg().lock().unwrap().owed.get(&key(b, a)).is_none());
        forget(a);
        forget(b);
    }

    #[test]
    fn told_it_is_not_done_a_caller_hears_only_of_news() {
        use crate::activity::Turn::{Busy, Idle, Waiting};
        let told = Owed::pending(5, 0);
        assert_eq!(verdict(&Owed::from(5, 0), 6, Some(Idle), true, false), Verdict::TurnOver, "a first turn end is news, job or not");
        assert_eq!(verdict(&Owed::from(5, 0), 5, Some(Idle), false, false), Verdict::Nothing);
        assert_eq!(verdict(&told, 5, Some(Idle), true, false), Verdict::AtWork, "the job runs on");
        assert_eq!(verdict(&told, 5, Some(Idle), false, false), Verdict::JobOver, "it ended with no turn");
        assert_eq!(verdict(&told, 5, Some(Busy), false, false), Verdict::Nothing, "a turn runs: its end will say");
        assert_eq!(verdict(&told, 5, Some(Waiting), false, false), Verdict::Nothing, "on a prompt: its own notice");
        assert_eq!(verdict(&told, 6, Some(Idle), true, false), Verdict::OnlyNotices, "a turn on a notice is no news");
        assert_eq!(verdict(&told, 6, Some(Idle), true, true), Verdict::TurnOver, "a turn of its own is");
        assert_eq!(verdict(&told, 6, Some(Busy), true, true), Verdict::Nothing, "not before it is over");
    }

    #[test]
    fn a_turn_counts_for_a_caller_told_it_is_not_done_unless_a_notice_started_it() {
        let mut r = reg().lock().unwrap();
        notice_turn(&mut r, "rt-s", 4, &["rt-other".into()]);
        drop(r);
        let past = |after, ended| real_turn_past("rt-s", "rt-c", after, ended, false, || false);
        assert!(!past(3, 4), "the one a notice started");
        assert!(past(3, 5), "and the one after it, which none did");
        assert!(past(2, 4), "or one before");
        assert!(!past(4, 4), "none past the count");
        // A notice that did not start a turn is not marked: at worst one notice more.
        assert!(real_turn_past("rt-unmarked", "rt-c", 0, 1, false, || false));
        forget("rt-s");
    }

    /// A top chat, a helper with tools, and the helper's own helper: the first
    /// answer of the middle one is "I have started it", and what it makes of
    /// its helper's answer is what the top chat asked for.
    #[test]
    fn a_turn_on_its_own_helpers_news_reaches_the_caller_above() {
        use crate::activity::Turn::Idle;
        let none = serde_json::json!({});
        register("tr-top", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone());
        register("tr-mid", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("tr-top"), true, none.clone());
        register("tr-low", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("tr-mid"), false, none.clone());
        register("tr-low2", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("tr-mid"), false, none);
        owe_from("tr-mid", "tr-low", 0, 0);
        assert!(waits_on_helpers("tr-mid", "tr-top"));
        assert!(!waits_on_helpers("tr-mid", "tr-low"), "not counting the one that would be told");
        assert!(!waits_on_helpers("tr-top", ""), "answers owed to it, not those owed by it");
        owe_from("tr-mid", "tr-low2", 0, 0);

        // Its first turn ended: told it is not done (`notice_line`), the debt goes on.
        let told = Owed::pending(1, 0);
        // The first helper's notice starts its turn 2, with the other still out.
        reg().lock().unwrap().owed.remove(&("tr-mid".to_string(), "tr-low".to_string()));
        notice_turn(&mut reg().lock().unwrap(), "tr-mid", 2, &["tr-low".into()]);
        let waits = waits_on_helpers("tr-mid", "tr-top");
        assert!(waits);
        assert_eq!(verdict(&told, 2, Some(Idle), waits, real_turn_past("tr-mid", "tr-top", 1, 2, waits, || false)), Verdict::OnlyNotices, "quiet while one is out");
        assert_eq!(
            verdict(&told, 2, Some(Idle), waits, real_turn_past("tr-mid", "tr-top", 1, 2, waits, || true)),
            Verdict::TurnOver,
            "unless what it made of that is news for its parent"
        );
        // The last one's notice starts its turn 3: this is the result.
        reg().lock().unwrap().owed.remove(&("tr-mid".to_string(), "tr-low2".to_string()));
        notice_turn(&mut reg().lock().unwrap(), "tr-mid", 3, &["tr-low2".into()]);
        let waits = waits_on_helpers("tr-mid", "tr-top");
        assert!(!waits);
        let told = Owed::pending(2, 0);
        assert_eq!(verdict(&told, 3, Some(Idle), waits, real_turn_past("tr-mid", "tr-top", 2, 3, waits, || false)), Verdict::TurnOver, "it reaches the top");
        // A notice about something else, or about the caller itself, does not count.
        notice_turn(&mut reg().lock().unwrap(), "tr-mid", 4, &["tr-stranger".into()]);
        assert!(!real_turn_past("tr-mid", "tr-top", 3, 4, false, || false));
        assert!(!real_turn_past("tr-mid", "tr-top", 3, 4, false, || true), "news or not");
        // A notice about its helper and someone else: not all its own helpers' doing.
        notice_turn(&mut reg().lock().unwrap(), "tr-mid", 6, &["tr-low".into(), "tr-stranger".into()]);
        assert!(!real_turn_past("tr-mid", "tr-top", 5, 6, true, || true));
        notice_turn(&mut reg().lock().unwrap(), "tr-low", 1, &["tr-mid".into()]);
        assert!(!real_turn_past("tr-low", "tr-mid", 0, 1, false, || false), "not a helper of its own");
        notice_turn(&mut reg().lock().unwrap(), "tr-mid", 5, &["tr-low".into()]);
        assert!(!real_turn_past("tr-mid", "tr-low", 4, 5, false, || false), "its own notice coming back");
        for id in ["tr-low", "tr-low2", "tr-mid", "tr-top"] {
            forget(id);
        }
    }

    /// The QA helper of 0.8.1: its helpers' notices start its turns, each ends
    /// on a report its parent needs now, and its parent heard none until the
    /// last helper had answered.
    #[test]
    fn a_report_made_on_its_own_helpers_news_reaches_its_parent_once() {
        let none = serde_json::json!({});
        register("ir-top", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone());
        register("ir-mid", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("ir-top"), true, serde_json::json!({ "background": true }));
        register("ir-low", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("ir-mid"), false, none.clone());
        register("ir-low2", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("ir-mid"), false, none.clone());
        register("ir-peer", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none);
        crate::background::idle_for_test("ir-mid", Provider::Claude, 2, false);
        owe_from("ir-mid", "ir-low2", 0, 0);
        let said = |t: &str| format!("{}\n", serde_json::json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": t }] } }));
        let notice = format!("{}\n", serde_json::json!({ "type": "user", "message": { "role": "user", "content": format!("{NOTICE_TAG} Your helper ir-low finished its turn.") } }));
        let before = said("Started both helpers.");
        let path = std::env::temp_dir().join(format!("luna-ir-mid-{}.jsonl", std::process::id()));
        transcripts().lock().unwrap().insert("ir-mid".into(), path.clone());
        // Its turn 2 is on ir-low's notice, with ir-low2 still out.
        notice_turn(&mut reg().lock().unwrap(), "ir-mid", 2, &["ir-low".into()]);
        let known = reg().lock().unwrap().known.get("ir-mid").cloned().unwrap();
        let pty = crate::pty::manager();
        let line = |caller: &str, transcript: &str| {
            std::fs::write(&path, format!("{before}{transcript}")).unwrap();
            notice_line(&pty, caller, "Your helper", "ir-mid", &known, Owed::pending(1, before.len() as u64))
        };
        let report = format!("Phase 1 is in: kill the core before 12:35 UTC. {}", "Evidence. ".repeat(45));

        let Some((Some(text), Settled::Pending(2, cursor))) = line("ir-top", &format!("{notice}{}", said(&report))) else { panic!("its parent hears") };
        assert!(text.contains("ended another turn") && text.contains("helpers of its own have yet to answer it"), "{text}");
        assert!(text.contains("It begins: \"Phase 1 is in"), "{text}");
        assert_eq!(cursor, std::fs::metadata(&path).unwrap().len(), "and what follows is what it hears next");
        let file = text.split("the answer is in ").nth(1).and_then(|t| t.split(", its report first").next()).unwrap();
        assert!(std::fs::read_to_string(file).unwrap().starts_with("Phase 1 is in"));
        let _ = std::fs::remove_file(file);
        // A line or two is no news worth a turn: it comes with the answer.
        assert!(matches!(line("ir-top", &format!("{notice}{}", said("Noted; waiting on ir-low2."))), Some((None, Settled::Skip(2)))));
        // Only its parent: a session that wrote to it is told of its own turns alone.
        assert!(matches!(line("ir-peer", &format!("{notice}{}", said(&report))), Some((None, Settled::Skip(2)))));
        // A turn on a notice about anyone but its own helpers is no news for the parent.
        notice_turn(&mut reg().lock().unwrap(), "ir-mid", 2, &["ir-low".into(), "ir-peer".into()]);
        reg().lock().unwrap().notice_turns.get_mut("ir-mid").unwrap().remove(0);
        assert!(matches!(line("ir-top", &format!("{notice}{}", said(&report))), Some((None, Settled::Skip(2)))));
        let _ = std::fs::remove_file(&path);
        crate::background::gone_for_test("ir-mid");
        for id in ["ir-low", "ir-low2", "ir-mid", "ir-top", "ir-peer"] {
            forget(id);
        }
    }

    /// The loop of that night, between a chat and its own helper that wrote to
    /// each other, every turn on a notice saying plenty: what goes up to the
    /// parent never comes back down.
    #[test]
    fn a_parent_and_its_helper_that_wrote_to_each_other_do_not_wake_each_other_forever() {
        use crate::activity::Turn;
        let (p, h) = ("pq-parent", "pq-helper");
        let none = serde_json::json!({});
        register(p, Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone());
        register(h, Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some(p), true, none);
        let key = |caller: &str, id: &str| (caller.to_string(), id.to_string());
        let mut turns: HashMap<&str, u32> = HashMap::from([(p, 1), (h, 1)]);
        owe_from(p, h, 0, 0);
        owe_from(h, p, 0, 0);
        let mut notices = 0;
        for _ in 0..500 {
            for (caller, id) in [(p, h), (h, p)] {
                let Some(o) = reg().lock().unwrap().owed.get(&key(caller, id)).copied() else { continue };
                let ended = turns[id];
                // Always news, wherever its parent is the one to hear it.
                let upward = || parent_of(id).as_deref() == Some(caller);
                let v = verdict(&o, ended, Some(Turn::Idle), true, o.told_pending && real_turn_past(id, caller, o.baseline, ended, false, upward));
                match v {
                    Verdict::OnlyNotices => reg().lock().unwrap().owed.get_mut(&key(caller, id)).unwrap().baseline = ended,
                    Verdict::TurnOver => {
                        notices += 1;
                        let mut r = reg().lock().unwrap();
                        notice_turn(&mut r, caller, turns[caller] + 1, &[id.to_string()]);
                        r.owed.insert(key(caller, id), Owed::pending(ended, 0));
                        drop(r);
                        *turns.get_mut(caller).unwrap() += 1;
                    }
                    _ => {}
                }
            }
        }
        assert_eq!(notices, 2, "\"NOT done yet\" once each, and quiet after");
        forget(h);
        forget(p);
    }

    #[test]
    fn a_wait_that_takes_a_first_answer_leaves_the_rest_owed() {
        let none = serde_json::json!({});
        register("cw-top", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone());
        register("cw-mid", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("cw-top"), true, none.clone());
        register("cw-low", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("cw-mid"), false, none);
        let key = ("cw-top".to_string(), "cw-mid".to_string());
        owe_from("cw-top", "cw-mid", 0, 0);
        owe_from("cw-mid", "cw-low", 0, 0);
        shown("cw-top", "cw-mid", 1);
        let o = reg().lock().unwrap().owed.get(&key).copied().expect("what it makes of its helper's answer is still owed");
        assert!(o.told_pending);
        assert_eq!(o.baseline, 1);
        // With no helper out, a turn past the question settles it, as ever.
        reg().lock().unwrap().owed.remove(&("cw-mid".to_string(), "cw-low".to_string()));
        shown("cw-top", "cw-mid", 2);
        assert!(reg().lock().unwrap().owed.get(&key).is_none());
        for id in ["cw-low", "cw-mid", "cw-top"] {
            forget(id);
        }
    }

    #[test]
    fn a_helper_with_helpers_or_answers_to_come_is_not_auto_deleted() {
        let none = serde_json::json!({});
        register("rq-top", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone());
        register("rq-mid", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("rq-top"), true, none.clone());
        register("rq-low", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("rq-mid"), false, none);
        reg().lock().unwrap().auto.insert("rq-mid".into(), Auto::default());
        let since = || reg().lock().unwrap().auto.get("rq-mid").and_then(|a| a.idle_since);
        let pty = crate::pty::manager();
        owe_from("rq-mid", "rq-low", 0, 0);
        reap(&pty);
        assert_eq!(since(), None, "an answer is still owed to it");
        reg().lock().unwrap().owed.remove(&("rq-mid".to_string(), "rq-low".to_string()));
        reap(&pty);
        assert!(since().is_some(), "nothing owed, and its helper has gone: the clock runs");
        for id in ["rq-low", "rq-mid", "rq-top"] {
            forget(id);
        }
    }

    #[test]
    fn a_deleted_chats_helpers_pass_to_its_parent_or_stand_alone() {
        let none = serde_json::json!({});
        let bg = serde_json::json!({ "background": true });
        register("og-top", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone());
        register("og-mid", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("og-top"), true, bg.clone());
        register("og-low", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("og-mid"), false, bg.clone());
        register("og-vis", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("og-top"), false, none.clone());
        register("og-other", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone());
        owe_from("og-mid", "og-low", 0, 0);
        assert!(authorize("og-top", "og-low").is_ok());
        // What the window is told, for a visible chat whose parent goes.
        register("rp-top", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone());
        register("rp-mid", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("rp-top"), true, none.clone());
        register("rp-vis", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("rp-mid"), false, none.clone());
        register("rp-bg", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("rp-mid"), false, bg.clone());
        let (handed, passed, moved) = pass_up(&mut reg().lock().unwrap(), "rp-mid", Some("rp-top".into()));
        assert!(handed);
        assert_eq!(moved, [("rp-bg".to_string(), Some("rp-top".to_string()))], "a background one hears as its next turn starts");
        assert_eq!(passed, [Reparented { id: "rp-vis".into(), parent: "rp-top".into() }], "a background one has no row to tell");
        // As a core started later finds it: the row now names the one it passed to.
        reg().lock().unwrap().known.remove("rp-vis");
        register("rp-vis", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("rp-top"), false, none.clone());
        assert!(authorize("rp-top", "rp-vis").is_ok(), "still within reach");
        let (_, passed, _) = pass_up(&mut reg().lock().unwrap(), "rp-top", None);
        assert!(passed.is_empty(), "with no one to pass to, the row keeps saying whose it was");
        for id in ["rp-bg", "rp-vis", "rp-mid", "rp-top"] {
            forget(id);
        }

        // A helper in the middle goes: its helper is the top chat's directly,
        // which could reach it all along.
        forget("og-mid");
        assert_eq!(parent_of("og-low").as_deref(), Some("og-top"));
        assert!(authorize("og-top", "og-low").is_ok(), "kill and delete still reach it");
        assert!(list_ids("og-top").contains(&"og-low".to_string()), "and the list shows it");
        assert!(reg().lock().unwrap().auto.get("og-low").is_none(), "its parent's to delete, as it says");

        // The top chat goes: nobody is left above them.
        forget("og-top");
        assert_eq!(parent_of("og-low"), None);
        assert!(reg().lock().unwrap().known.get("og-low").unwrap().orphan);
        assert!(reg().lock().unwrap().auto.contains_key("og-low"), "a background one goes by itself once idle");
        assert!(reg().lock().unwrap().known.get("og-vis").unwrap().orphan);
        assert!(!reg().lock().unwrap().auto.contains_key("og-vis"), "a chat with a row is the person's to delete");
        assert!(authorize("og-other", "og-low").is_err(), "no other chat's agent has a claim on it");
        assert!(authorize_view("og-other", "og-low").is_ok(), "though it can read it, as any session");

        // Running again, the window names the parent it was spawned by; that stands no more.
        register("og-vis", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("og-top"), false, none.clone());
        assert_eq!(parent_of("og-vis"), None);
        // And saved with it, for the core after this one.
        let ties = ties_of("og-low").unwrap();
        assert!(ties.orphan && ties.parent.is_none() && ties.auto_delete);
        let old: Ties = serde_json::from_value(serde_json::json!({
            "parent": null, "name": null, "settings": {}, "tools": false, "borrowed": false, "autoDelete": false, "owed": [], "seen": []
        }))
        .expect("what an older core saved still reads");
        assert!(!old.orphan);
        for id in ["og-low", "og-vis", "og-other"] {
            forget(id);
        }
    }

    fn list_ids(caller: &str) -> Vec<String> {
        descendants(&reg().lock().unwrap(), caller)
    }

    #[test]
    fn a_question_asked_since_makes_a_new_debt() {
        owe_from("nd-a", "nd-b", 1, 10);
        reg().lock().unwrap().owed.insert(("nd-a".into(), "nd-b".into()), Owed::pending(2, 20));
        owe_from("nd-a", "nd-b", 3, 30);
        let o = reg().lock().unwrap().owed.remove(&("nd-a".to_string(), "nd-b".to_string())).unwrap();
        assert!(!o.told_pending, "this one may be told it is not done, once");
        assert_eq!((o.baseline, o.cursor), (3, 20));
    }

    #[test]
    fn only_a_job_ending_or_a_later_turn_settles_a_caller_told_it_is_not_done() {
        let key = ("js-a".to_string(), "js-b".to_string());
        let owed = || reg().lock().unwrap().owed.get(&key).copied();
        let told = Owed::pending(3, 0);
        reg().lock().unwrap().owed.insert(key.clone(), told);
        // A wait with an `afterTurn` from before the notice hands over a turn
        // the caller was told of: the job's end is still to come.
        drop(hand_over("js-a", "js-b", 3, None));
        assert!(owed().is_some_and(|o| o.told_pending), "still owed");
        drop(hand_over("js-a", "js-b", 3, Some(told.rev)));
        assert!(owed().is_none(), "the job's end, with no turn past it");
        reg().lock().unwrap().owed.insert(key.clone(), Owed::pending(3, 0));
        drop(hand_over("js-a", "js-b", 4, None));
        assert!(owed().is_none(), "a later turn, as ever");
    }

    #[test]
    fn a_question_asked_meanwhile_outlives_what_was_decided_about_the_last_one() {
        let key = ("rv-a".to_string(), "rv-b".to_string());
        let owed = || reg().lock().unwrap().owed.get(&key).copied();
        // A wait saw the job end on the debt it knew; the caller has asked again.
        let old = Owed::pending(3, 0);
        reg().lock().unwrap().owed.insert(key.clone(), old);
        owe_from("rv-a", "rv-b", 3, 0);
        let new = owed().unwrap();
        assert_ne!(new.rev, old.rev, "the same numbers, another debt");
        drop(hand_over("rv-a", "rv-b", 3, Some(old.rev)));
        assert_eq!(owed().map(|o| o.rev), Some(new.rev), "the new question stands");
        // A notice about the old one went in meanwhile: nor does that take it.
        settle_told(&mut reg().lock().unwrap(), key.clone(), Settled::Answered, &old);
        assert_eq!(owed().map(|o| o.rev), Some(new.rev));
        settle_told(&mut reg().lock().unwrap(), key.clone(), Settled::Pending(9, 9), &old);
        assert_eq!(owed().map(|o| (o.rev, o.baseline)), Some((new.rev, 3)));
        settle_told(&mut reg().lock().unwrap(), key.clone(), Settled::Waiting, &old);
        assert!(!owed().unwrap().told_waiting);
        // The one it told of it settles, and a debt a wait took stays gone.
        settle_told(&mut reg().lock().unwrap(), key.clone(), Settled::Answered, &new);
        assert!(owed().is_none());
        settle_told(&mut reg().lock().unwrap(), key.clone(), Settled::Pending(9, 9), &new);
        assert!(owed().is_none(), "a notice does not bring back what a wait handed over");
    }

    #[test]
    fn taking_a_debt_and_owing_on_leave_a_new_question_alone() {
        let none = serde_json::json!({});
        register("tk-top", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone());
        register("tk-mid", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("tk-top"), true, none.clone());
        register("tk-low", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("tk-mid"), false, none);
        let key = ("tk-top".to_string(), "tk-mid".to_string());
        owe_from("tk-mid", "tk-low", 0, 0);
        owe_from("tk-top", "tk-mid", 0, 0);
        let goes_on = take_debt(&mut reg().lock().unwrap(), &key, 1, None);
        assert!(goes_on, "its own helper has yet to answer it");
        assert!(reg().lock().unwrap().owed.get(&key).is_none(), "taken in the one hold of the lock");
        // A question between the taking and the owing on is the debt from here.
        owe_from("tk-top", "tk-mid", 1, 50);
        let asked = reg().lock().unwrap().owed.get(&key).copied().unwrap();
        owed_on(key.clone(), 1, goes_on);
        let now = reg().lock().unwrap().owed.get(&key).copied().unwrap();
        assert_eq!((now.rev, now.told_pending), (asked.rev, false));
        for id in ["tk-low", "tk-mid", "tk-top"] {
            forget(id);
        }
    }

    /// A helper announced as not done, its job over, with a transcript on disk
    /// holding what `said` past `cursor`, the debt `caller` is owed.
    fn job_over_helper(id: &str, caller: &str, said: &str) -> (Known, Owed, std::path::PathBuf) {
        let none = serde_json::json!({});
        register(id, Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, false, none);
        crate::background::idle_for_test(id, Provider::Claude, 3, false);
        let path = std::env::temp_dir().join(format!("luna-{id}-{}.jsonl", std::process::id()));
        let before = "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"I am starting the job.\"}]}}\n";
        std::fs::write(&path, format!("{before}{said}")).unwrap();
        transcripts().lock().unwrap().insert(id.into(), path.clone());
        let o = Owed::pending(3, before.len() as u64);
        reg().lock().unwrap().owed.insert((caller.to_string(), id.to_string()), o);
        (reg().lock().unwrap().known.get(id).cloned().unwrap(), o, path)
    }

    /// The night of 0.8.1: a `tail -F` its own tool left running after its turn
    /// kept a helper "not done" for 14 hours, its caller told nothing.
    #[test]
    fn what_a_turn_left_is_named_and_an_idle_one_is_told_of_once() {
        use crate::activity::Turn::Idle;
        let (known, o, path) = job_over_helper("lj-s", "lj-c", "");
        let pty = crate::pty::manager();
        let line = |o: Owed| notice_line(&pty, "lj-c", "Your helper", "lj-s", &known, o);
        // Its first turn's end, with the tail just left: not done, and what runs is named.
        crate::background::left_for_test("lj-s", &[("tail.exe", 22104, 0)]);
        let mut first = Owed::from(2, o.cursor);
        first.held = Some((3, now_ms() - BACKGROUND_GRACE_MS));
        let Some((Some(text), Settled::Pending(3, _))) = line(first) else { panic!("not done yet") };
        assert!(text.contains("NOT done yet: a job it started still runs in the background (tail.exe (pid 22104))"), "{text}");
        assert!(job_at_work("lj-s", crate::activity::turn_state("lj-s"), true));
        // Quiet for a while, but not for long: still a job, nothing to say.
        crate::background::left_for_test("lj-s", &[("tail.exe", 22104, IDLE_JOB_MS - 60_000)]);
        assert!(matches!(line(o), Some((None, Settled::Hold(3))) | None));
        // Half an hour of nothing: told once, how to end it with it, and settled.
        crate::background::left_for_test("lj-s", &[("tail.exe", 22104, IDLE_JOB_MS + 60_000)]);
        assert!(!job_at_work("lj-s", crate::activity::turn_state("lj-s"), true));
        assert!(job_over("lj-s", &o, Some(crate::activity::TurnState { turn: Idle, busy: false, turns_ended: 3, children_busy: true })));
        assert!(matches!(line(o), Some((None, Settled::Hold(3)))), "after the grace a job's end stands for");
        let mut stood = o;
        stood.held = Some((3, now_ms() - BACKGROUND_GRACE_MS));
        let Some((Some(text), Settled::Answered)) = line(stood) else { panic!("its idle job is news") };
        assert!(text.contains("only idle processes left, which have done nothing for 31 min") && text.contains("tail.exe (pid 22104). luna_kill ends them."), "{text}");
        assert_eq!(crate::agents::state_of("lj-s").0, "background", "what is there is there");
        // A first notice that finds it idle already says so as it says it finished.
        let mut late = Owed::from(2, o.cursor);
        late.held = Some((3, now_ms() - BACKGROUND_GRACE_MS));
        let Some((Some(text), Settled::Answered)) = line(late) else { panic!("finished") };
        assert!(text.contains("finished its turn") && text.contains("It left processes running that have done nothing for 31 min: tail.exe (pid 22104)"), "{text}");
        crate::background::gone_for_test("lj-s");
        forget("lj-s");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_stop_between_turns_ends_what_is_left_and_leaves_it_to_go_by_itself() {
        let none = serde_json::json!({});
        register("ks-c", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone());
        register("ks-s", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("ks-c"), false, serde_json::json!({ "background": true }));
        crate::background::idle_for_test("ks-s", Provider::Claude, 1, false);
        reg().lock().unwrap().auto.insert("ks-s".into(), Auto::default());
        let k = kill("ks-c", "ks-s").unwrap();
        assert!(k.ok && k.ended.is_empty());
        assert!(reg().lock().unwrap().auto.contains_key("ks-s"), "between turns it was in the middle of nothing");
        crate::background::in_turn_for_test("ks-s");
        kill("ks-c", "ks-s").unwrap();
        assert!(!reg().lock().unwrap().auto.contains_key("ks-s"), "stopped mid-turn, it stays for luna_send");
        crate::background::gone_for_test("ks-s");
        forget("ks-s");
        forget("ks-c");
    }

    #[test]
    fn a_jobs_end_brings_what_was_said_on_notices_since() {
        let prompt = "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"[Luna notice]\"}}\n";
        let result = "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"Here are the real results.\"}]}}\n";
        let (known, o, path) = job_over_helper("jr-s", "jr-c", &format!("{prompt}{result}"));
        let pty = crate::pty::manager();
        // As `wait` puts it for `job_done`.
        assert_eq!(reply_text("jr-s", &said_since(&pty, "jr-s", Provider::Claude, o.cursor), true).as_deref(), Some("Here are the real results."));
        // The same as the notice brings: nothing is settled without its report.
        let mut stood = o;
        stood.held = Some((3, now_ms() - BACKGROUND_GRACE_MS));
        let Some((Some(line), Settled::Answered)) = notice_line(&pty, "jr-c", "Your helper", "jr-s", &known, stood) else { panic!("the job's end is news") };
        assert!(line.contains("it wrote 1 message since your last notice"), "{line}");
        // Nothing since: nothing to hand over, not an old message.
        let (_, o2, path2) = job_over_helper("jr-q", "jr-c", "");
        assert_eq!(reply_text("jr-q", &said_since(&pty, "jr-q", Provider::Claude, o2.cursor), true), None);
        for id in ["jr-s", "jr-q"] {
            crate::background::gone_for_test(id);
            forget(id);
        }
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&path2);
    }

    #[test]
    fn a_jobs_end_goes_to_a_wait_that_is_out_and_not_to_a_notice_too() {
        let (known, o, path) = job_over_helper("jw-s", "jw-c", "");
        let key = ("jw-c".to_string(), "jw-s".to_string());
        let pty = crate::pty::manager();
        let debt = || reg().lock().unwrap().owed.get(&key).copied().unwrap();
        let out = WaitOut::start("jw-c", "jw-s");
        // The one clock: whoever looks first starts it, the other reads it.
        assert!(held_for_wait(&pty, "jw-c", "jw-s", &o), "the wait has it reserved");
        let started = debt().held.expect("the hold starts the clock");
        assert!(!job_end_stood(&key, 3, true), "the wait goes by the same clock");
        assert_eq!(debt().held, Some(started), "and does not start its own");
        assert_eq!(debt().ready_at, None, "not news yet");
        // Past where a notice used to give up on the wait, still reserved.
        reg().lock().unwrap().owed.get_mut(&key).unwrap().held = Some((3, now_ms() - BACKGROUND_GRACE_MS + 5_000));
        assert!(held_for_wait(&pty, "jw-c", "jw-s", &debt()), "through the whole grace");
        assert!(!job_end_stood(&key, 3, true));
        // The grace is over: the wait hands it over now, with its hold to do it in.
        reg().lock().unwrap().owed.get_mut(&key).unwrap().held = Some((3, now_ms() - BACKGROUND_GRACE_MS));
        assert!(job_end_stood(&key, 3, true), "the wait's moment");
        assert!(held_for_wait(&pty, "jw-c", "jw-s", &debt()), "and the notice still holds for it");
        assert!(debt().ready_at.is_some(), "from here on it is WAIT_HOLD_MS");
        // A wait that sat on it past that has been let go: the notice is free.
        reg().lock().unwrap().owed.get_mut(&key).unwrap().ready_at = Some(now_ms() - WAIT_HOLD_MS - 1);
        assert!(!held_for_wait(&pty, "jw-c", "jw-s", &debt()));
        // With the wait gone the notice goes by the clock the wait left running.
        drop(out);
        let stood = debt();
        assert!(!held_for_wait(&pty, "jw-c", "jw-s", &stood));
        assert!(matches!(notice_line(&pty, "jw-c", "Your helper", "jw-s", &known, stood), Some((Some(_), Settled::Answered))));
        // A job back at work starts it over.
        assert!(!job_end_stood(&key, 3, false));
        assert_eq!(debt().held, None);
        assert!(!job_end_stood(&key, 3, true), "from nothing");
        // A notice settled it first: the wait is not left counting for ever.
        reg().lock().unwrap().owed.remove(&key);
        assert!(job_end_stood(&key, 3, true));
        crate::background::gone_for_test("jw-s");
        forget("jw-s");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_breaker_lets_so_many_notices_through_and_stops_the_pair() {
        let (caller, id) = ("br-a", "br-b");
        let pty = crate::pty::manager();
        let t0 = now_ms();
        for i in 0..BREAKER_NOTICES - 1 {
            assert!(!pair_stopped(&pty, caller, id));
            assert!(!last_notice(caller, id, t0));
            assert!(!count_notice(&mut reg().lock().unwrap(), caller, id, t0), "notice {i}");
        }
        assert!(last_notice(caller, id, t0), "it says so as it goes");
        assert!(count_notice(&mut reg().lock().unwrap(), caller, id, t0));
        assert!(pair_stopped(&pty, caller, id), "nothing more for this pair");
        assert!(!pair_stopped(&pty, id, caller), "the other way round is a pair of its own");
        forget(caller);
        assert!(!pair_stopped(&pty, caller, id), "gone with the chat");
    }

    #[test]
    fn a_stopped_pair_still_hears_of_prompts_and_exits_and_nothing_else() {
        let (known, o, path) = job_over_helper("bp-s", "bp-c", "");
        let pty = crate::pty::manager();
        let t0 = now_ms();
        for _ in 0..BREAKER_NOTICES {
            count_notice(&mut reg().lock().unwrap(), "bp-c", "bp-s", t0);
        }
        assert!(pair_stopped(&pty, "bp-c", "bp-s"));
        // A job's end or a turn's, which is all the notifier would have for a
        // stopped pair: nothing.
        assert!(standing_line(&pty, "bp-c", "Your helper", "bp-s", &known, o).is_none());
        // Held on a prompt, it is told, or the helper sits there until it times out.
        crate::background::in_turn_for_test("bp-s");
        let asks = std::thread::spawn(|| crate::background::permission("bp-s", &serde_json::json!({ "tool_name": "Bash", "input": { "command": "ls" } })));
        for _ in 0..200 {
            if ask_of("bp-s").is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let Some((Some(line), Settled::Waiting)) = standing_line(&pty, "bp-c", "Your helper", "bp-s", &known, o) else { panic!("a prompt goes through") };
        assert!(line.contains("asks for permission to run Bash: ls") && line.contains("askId"), "{line}");
        crate::background::kill("bp-s");
        let _ = asks.join();
        // And gone is gone.
        crate::background::gone_for_test("bp-s");
        assert!(matches!(standing_line(&pty, "bp-c", "Your helper", "bp-s", &known, o), Some((Some(_), Settled::Exited))));
        forget("bp-s");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_breaker_counts_only_what_falls_within_its_window() {
        let (caller, id) = ("bw-a", "bw-b");
        let t0 = now_ms();
        for _ in 0..BREAKER_NOTICES - 1 {
            assert!(!count_notice(&mut reg().lock().unwrap(), caller, id, t0));
        }
        // A notice now and then over a long job, as real use has it.
        let later = t0 + BREAKER_WINDOW_MS;
        assert!(!last_notice(caller, id, later), "the old ones have aged out");
        assert!(!count_notice(&mut reg().lock().unwrap(), caller, id, later));
        forget(caller);
    }

    #[test]
    fn the_breaker_counts_turn_ends_and_not_prompts() {
        assert!(counts_for_breaker(&Settled::Answered));
        assert!(counts_for_breaker(&Settled::Pending(1, 0)));
        assert!(!counts_for_breaker(&Settled::Waiting), "a busy helper asks often, and holds until answered");
        assert!(!counts_for_breaker(&Settled::Asked(1)));
        assert!(!counts_for_breaker(&Settled::Exited));
    }

    /// `notice_line` on a session known only by its hooks, the debt past the
    /// grace it gives a turn with no reply yet.
    fn line_for(id: &str) -> String {
        let pty = crate::pty::manager();
        let known = reg().lock().unwrap().known.get(id).cloned().unwrap();
        let mut o = Owed::from(0, 0);
        o.held = Some((1, now_ms() - REPLY_GRACE_MS));
        match notice_line(&pty, "its-caller", "Your helper", id, &known, o) {
            Some((Some(line), Settled::Answered)) => line,
            _ => panic!("a turn is over: there is something to say"),
        }
    }

    #[test]
    fn a_turn_that_failed_is_not_reported_as_finished() {
        use crate::activity::{on_hook, HookEvent};
        let none = serde_json::json!({});
        register("fl-limit", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, false, none.clone());
        on_hook("fl-limit", HookEvent::Busy);
        on_hook("fl-limit", HookEvent::Failed("rate_limit"));
        let line = line_for("fl-limit");
        assert!(line.starts_with("Your helper stopped on an error: rate_limit."), "{line}");
        assert!(!line.contains("finished"), "{line}");
        // No error known and nothing to read: that it is not known is what it says.
        register("fl-lost", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, false, none);
        on_hook("fl-lost", HookEvent::Busy);
        on_hook("fl-lost", HookEvent::Idle);
        let line = line_for("fl-lost");
        assert!(line.contains("cannot find its transcript"), "{line}");
        assert!(!line.contains("without a reply"), "{line}");
        forget("fl-limit");
        forget("fl-lost");
        crate::activity::forget("fl-limit");
        crate::activity::forget("fl-lost");
    }

    /// A turn that stopped short hands over what it said before that, whatever
    /// came after it; and a line the CLI put in itself (the end of a background
    /// task it takes up) is no prompt whose reply is still to come.
    #[test]
    fn what_a_turn_said_comes_with_its_notice_whatever_came_after() {
        use crate::activity::{on_hook, HookEvent};
        let user = |t: &str| format!("{}\n", serde_json::json!({ "type": "user", "message": { "role": "user", "content": t } }));
        let said = |t: &str| format!("{}\n", serde_json::json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": t }] } }));
        let task = format!(
            "{}\n",
            serde_json::json!({ "type": "user", "message": { "role": "user", "content": "<task-notification>\n<task-id>b1</task-id>\n</task-notification>" },
                "origin": { "kind": "task-notification" }, "promptSource": "system", "turnOrigin": "task_notification" })
        );
        let worked = format!("{}{}{}", user("fix it"), said("Read the code."), said("Half the fix is in."));
        let pty = crate::pty::manager();
        let line_of = |id: &str, transcript: String, cut: bool| -> String {
            let path = std::env::temp_dir().join(format!("luna-{id}-{}.jsonl", std::process::id()));
            std::fs::write(&path, transcript).unwrap();
            transcripts().lock().unwrap().insert(id.into(), path.clone());
            let known = reg().lock().unwrap().known.get(id).cloned().unwrap();
            let mut o = Owed::from(0, 0);
            o.cut_short = cut;
            o.held = Some((1, now_ms() - REPLY_GRACE_MS));
            let Some((Some(line), Settled::Answered)) = notice_line(&pty, "ws-caller", "Your helper", id, &known, o) else { panic!("its turn is over") };
            let _ = std::fs::remove_file(&path);
            line
        };
        let helper = |id: &str| {
            register(id, Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, false, serde_json::json!({}));
            crate::background::idle_for_test(id, Provider::Claude, 1, false);
        };

        // Cut short on the way to what was queued behind its turn.
        helper("ws-cut");
        let line = line_of("ws-cut", format!("{worked}{}", user("then the tests")), true);
        assert!(line.contains("did NOT finish") && line.contains("It ended with 2 messages since you asked"), "{line}");
        // The same transcript with nothing cut: a prompt last is a reply to come.
        helper("ws-open");
        assert!(line_of("ws-open", format!("{worked}{}", user("then the tests")), false).contains("without a reply"));
        // Claude Code taking up the end of its background task wrote that line itself.
        helper("ws-task");
        let line = line_of("ws-task", format!("{worked}{task}"), true);
        assert!(line.contains("It ended with 2 messages since you asked"), "{line}");
        let line = line_of("ws-task", format!("{worked}{task}"), false);
        assert!(line.contains("finished its turn with 2 messages since you asked"), "{line}");
        // Stopped on an error: what it said before that, and the error's own stop.
        register("ws-fail", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, false, serde_json::json!({}));
        on_hook("ws-fail", HookEvent::Busy);
        on_hook("ws-fail", HookEvent::Failed("rate_limit"));
        let line = line_of("ws-fail", format!("{worked}{}", user("then the tests")), false);
        assert!(line.starts_with("Your helper stopped on an error: rate_limit. It ended with 2 messages since you asked"), "{line}");
        for id in ["ws-cut", "ws-open", "ws-task", "ws-fail"] {
            crate::background::gone_for_test(id);
            forget(id);
        }
        crate::activity::forget("ws-fail");
        assert_eq!(sentence("pick a different model."), "pick a different model.");
        assert_eq!(sentence("rate_limit"), "rate_limit.");
        assert_eq!(sentence("cut short…"), "cut short…");
    }

    /// A wait on a helper a restart cut off hands over what it said, as its notice
    /// would, and says it did not finish: with no error to go by and a prompt as
    /// the last word, it once handed back nothing and settled the debt.
    #[test]
    fn a_wait_on_a_turn_a_restart_cut_short_keeps_what_it_said() {
        let none = serde_json::json!({});
        let dir = std::env::temp_dir().join(format!("luna-wait-cut-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let account = dir.to_string_lossy().into_owned();
        register("wc-caller", Provider::Claude, &account, &account, None, true, none.clone());
        crate::background::test_helper("wc-caller", &account, None, false);
        register("wc-helper", Provider::Claude, &account, &account, Some("wc-caller"), false, none);
        crate::background::test_helper("wc-helper", &account, None, false);
        let line = |role: &str, t: &str| match role {
            "user" => format!("{}\n", serde_json::json!({ "type": "user", "message": { "role": "user", "content": t } })),
            _ => format!("{}\n", serde_json::json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": t }] } })),
        };
        let path = dir.join("wc-helper.jsonl");
        std::fs::write(&path, format!("{}{}{}", line("user", "fix it"), line("assistant", "Half the fix is in."), line("user", "then the tests"))).unwrap();
        transcripts().lock().unwrap().insert("wc-helper".into(), path);
        owe_from("wc-caller", "wc-helper", 0, 0);
        crate::background::test_turn_ended("wc-helper");
        crate::background::cut_for_test("wc-helper");
        let call = crate::mcp::Call::new();
        let w = wait("wc-caller", "wc-helper", "turn_done", 10, None, &call).unwrap();
        assert_eq!(w.outcome, "turn_done");
        assert_eq!(w.reply.as_deref(), Some("Half the fix is in."));
        assert!(w.failed.as_deref().is_some_and(|f| f.contains("did NOT finish")), "{:?}", w.failed);
        for id in ["wc-helper", "wc-caller"] {
            crate::background::remove(id);
            forget(id);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_transcript_on_disk_with_no_reply_is_said_as_that() {
        use crate::activity::{on_hook, HookEvent};
        let path = std::env::temp_dir().join(format!("luna-no-reply-{}.jsonl", std::process::id()));
        std::fs::write(&path, "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"go\"}}\n").unwrap();
        register("nr-chat", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, false, serde_json::json!({}));
        transcripts().lock().unwrap().insert("nr-chat".into(), path.clone());
        on_hook("nr-chat", HookEvent::Busy);
        on_hook("nr-chat", HookEvent::Idle);
        let line = line_for("nr-chat");
        assert!(line.contains("finished its turn without a reply"), "{line}");
        forget("nr-chat");
        crate::activity::forget("nr-chat");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn an_exit_says_how_it_went() {
        let none = serde_json::json!({});
        register("ex-bad", Provider::Codex, "C:\\p", "C:\\acc\\openai\\one", None, false, none.clone());
        let pty = crate::pty::manager();
        assert_eq!(failure_of(&pty, "ex-bad", Provider::Codex), None);
        session_exited("ex-bad", Some(3));
        assert_eq!(failure_of(&pty, "ex-bad", Provider::Codex).as_deref(), Some("its CLI exited with code 3"));
        session_exited("ex-bad", Some(0));
        assert_eq!(failure_of(&pty, "ex-bad", Provider::Codex), None, "a clean exit is no error");
        session_exited("ex-bad", Some(3));
        register("ex-bad", Provider::Codex, "C:\\p", "C:\\acc\\openai\\one", None, false, none);
        assert_eq!(failure_of(&pty, "ex-bad", Provider::Codex), None, "running again, that is behind it");
        forget("ex-bad");
    }

    #[test]
    fn an_answer_restored_ahead_of_its_caller_waits_for_it() {
        let none = serde_json::json!({});
        // The order a core goes in: the helper first, as `restore` ties it back...
        let ties = Ties {
            parent: Some("rb-caller".into()),
            owed: vec![("rb-caller".into(), 3, 120)],
            seen: vec![("rb-caller".into(), 3)],
            ..Default::default()
        };
        tie_back("rb-helper", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", ties);
        let stands = |up_ms| owed_stands(&reg().lock().unwrap().known, "rb-caller", "rb-helper", up_ms);
        assert!(stands(0), "a caller not back yet is not one that is gone");
        assert!(stands(CALLER_RETURN_MS - 1));
        assert!(!stands(CALLER_RETURN_MS), "one that never comes back must not hold the helper from `reap` for good");
        // ... and its caller seconds later, as the window starts its CLI again.
        register("rb-caller", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none);
        assert!(stands(CALLER_RETURN_MS * 100), "known, it is owed for as long as it takes");
        let key = ("rb-caller".to_string(), "rb-helper".to_string());
        assert_eq!(reg().lock().unwrap().owed.get(&key).map(|o| (o.baseline, o.cursor)), Some((3, 120)));
        assert_eq!(last_shown("rb-caller", "rb-helper"), Some(3));
        // Deleted, it purges what it was owed as ever.
        forget("rb-caller");
        assert!(!reg().lock().unwrap().owed.contains_key(&key));
        assert!(!owed_stands(&reg().lock().unwrap().known, "rb-caller", "nobody", 0), "and an unknown session is owed nothing");
        forget("rb-helper");
    }

    /// The notice as the notifier would make it, past the grace for a reply
    /// that is not coming.
    fn notice_for(caller: &str, id: &str, turns_ended: u32) -> (Option<String>, Settled) {
        let (k, mut o) = {
            let r = reg().lock().unwrap();
            (r.known.get(id).cloned().unwrap(), r.owed.get(&(caller.to_string(), id.to_string())).copied().unwrap())
        };
        o.held = Some((turns_ended, now_ms() - REPLY_GRACE_MS));
        notice_line(&crate::pty::manager(), caller, "Your helper", id, &k, o).unwrap()
    }

    #[test]
    fn text_queued_as_the_core_went_comes_back_with_the_notice() {
        let none = serde_json::json!({});
        register("ql-child", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("ql-parent"), false, none);
        crate::background::take_up_for_test("ql-child", "C:\\p", 3, &["first line\nsecond line", "and another"], None);
        owe_from("ql-parent", "ql-child", 3, 0);
        let (line, how) = notice_for("ql-parent", "ql-child", 5);
        let line = line.unwrap();
        assert!(matches!(how, Settled::Answered));
        assert!(line.contains("did NOT finish") && line.contains("Not delivered: 2 messages queued for it"), "{line}");
        let file = line.split(", in ").nth(1).and_then(|l| l.split(';').next()).unwrap();
        assert!(line.replace(file, "").len() < 360, "a notice is typed into a CLI: {line}");
        let held = std::fs::read_to_string(file).unwrap();
        let _ = std::fs::remove_file(file);
        assert!(held.starts_with("NOT DELIVERED: 2 messages"), "{held}");
        let (first, second) = (held.find("first line\nsecond line").unwrap(), held.find("and another").unwrap());
        assert!(first < second, "word for word, oldest first: {held}");

        crate::background::remove("ql-child");
        forget("ql-child");
    }

    #[test]
    fn what_never_reached_a_helper_goes_only_to_its_sender_and_a_cancel_brings_it_back() {
        register("qs-child", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("qs-parent"), false, serde_json::json!({}));
        crate::background::take_up_for_test("qs-child", "C:\\p", 3, &[], None);
        crate::background::lose_for_test("qs-child", "qs-a", "a's private words");
        crate::background::lose_for_test("qs-child", "qs-b", "b's words");
        owe_from("qs-b", "qs-child", 3, 0);
        let line = notice_for("qs-b", "qs-child", 4).0.unwrap();
        assert!(line.contains("Not delivered: 1 message"), "{line}");
        let file = line.split(", in ").nth(1).and_then(|l| l.split(';').next()).unwrap();
        let held = std::fs::read_to_string(file).unwrap();
        let _ = std::fs::remove_file(file);
        assert!(held.contains("b's words") && !held.contains("a's private"), "{held}");
        assert!(undelivered_note("qs-nobody", "qs-child").is_none(), "another reader hears nothing of it");
        assert!(undelivered_note("qs-parent", "qs-child").is_none(), "nor does its parent, of what others sent");

        // Handed over, it is gone; a cancel crossing the response brings it back.
        let of_a = || crate::background::undelivered_for("qs-child", "qs-a", false);
        assert_eq!(of_a(), ["a's private words"]);
        let undo = hand_over("qs-a", "qs-child", 5, None);
        let later = hand_over("qs-a", "qs-child", 5, None);
        drop(later);
        assert!(of_a().is_empty());
        assert_eq!(crate::background::undelivered_for("qs-child", "qs-b", false), ["b's words"], "b's stays");
        undo();
        assert_eq!(of_a(), ["a's private words"]);

        crate::background::remove("qs-child");
        forget("qs-child");
        reg().lock().unwrap().owed.retain(|(_, s), _| s != "qs-child");
    }

    #[test]
    fn a_helper_that_resumed_owes_the_end_of_that_turn_and_says_so_once() {
        let none = serde_json::json!({ "resumeOnRestart": true });
        register("rs-child", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("rs-parent"), false, none);
        assert!(resumes_on_restart("rs-child"));
        assert!(!resumes_on_restart("nobody"));
        crate::background::take_up_for_test("rs-child", "C:\\p", 3, &[], None);
        owe_from("rs-parent", "rs-child", 3, 40);
        // The stopped turn is counted (4); the answer ends the one started past it.
        resumed("rs-child", 4);
        let key = ("rs-parent".to_string(), "rs-child".to_string());
        let owed = || reg().lock().unwrap().owed.get(&key).copied().unwrap();
        assert_eq!((owed().baseline, owed().cursor), (4, 40), "all it has said since asked, up to the end of the new turn");
        let (line, how) = notice_for("rs-parent", "rs-child", 4);
        assert!(matches!(how, Settled::Resumed));
        assert!(line.unwrap().contains("resumed automatically"));
        owe_from("rs-parent", "rs-child", 4, 90);
        assert!(owed().resumed && owed().cursor == 40, "asked again before the notice went, it still has to go");
        crate::background::remove("rs-child");
        forget("rs-child");
    }

    #[test]
    fn a_helper_that_never_got_started_says_so_and_hands_back_its_prompt() {
        register("ns-child", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("ns-parent"), false, serde_json::json!({}));
        crate::background::take_up_for_test("ns-child", "C:\\p", 0, &["and a follow-up"], Some("do the whole job"));
        owe_from("ns-parent", "ns-child", 0, 0);
        let (line, how) = notice_for("ns-parent", "ns-child", 2);
        let line = line.unwrap();
        assert!(matches!(how, Settled::Answered));
        assert!(line.contains("never got started") && line.contains("its prompt was lost") && line.contains("luna_send starts it afresh"), "{line}");
        assert!(line.contains("its prompt and 1 queued message are in "), "{line}");
        let file = line.split(" are in ").nth(1).and_then(|l| l.split(". luna_send").next()).unwrap();
        let held = std::fs::read_to_string(file).unwrap();
        let _ = std::fs::remove_file(file);
        assert!(held.find("do the whole job").unwrap() < held.find("and a follow-up").unwrap(), "{held}");
        // Still there to delete, which is what writes its end down.
        assert!(reg().lock().unwrap().known.contains_key("ns-child"));
        crate::background::remove("ns-child");
        forget("ns-child");
    }

    #[test]
    fn what_the_last_core_knew_of_a_chat_comes_back_with_it() {
        let saved = SavedChats {
            chats: vec![SavedChat {
                id: "tc-chat".into(),
                provider: Provider::Codex,
                folder: "C:\\p".into(),
                // In the platform's own separators: the account's name is its last part.
                account_path: if cfg!(windows) { "C:\\acc\\openai\\two" } else { "/acc/openai/two" }.into(),
                turns_ended: 5,
                transcript: None,
                mid_turn: false,
                ties: Ties {
                    parent: Some("tc-parent".into()),
                    name: Some("the chat".into()),
                    settings: serde_json::json!({ "sandbox": "read-only" }),
                    tools: false,
                    borrowed: true,
                    auto_delete: true,
                    // One whose answer was in, one whose turn went with the core.
                    owed: vec![("tc-parent".into(), 4, 10), ("tc-peer".into(), 5, 20)],
                    seen: vec![("tc-parent".into(), 5)],
                    cut_short: Vec::new(),
                    orphan: false,
                    ..Default::default()
                },
            }],
            sends: vec![SavedSend { caller: "tc-parent".into(), id: "tc-chat".into(), text: "held across".into(), going_in: None }],
            spawns: Vec::new(),
        };
        // As the file has it.
        let saved: SavedChats = serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
        take_up_chats(saved);
        let key = |caller: &str| (caller.to_string(), "tc-chat".to_string());
        {
            let r = reg().lock().unwrap();
            let k = &r.known["tc-chat"];
            assert!(k.waiting && k.parent.as_deref() == Some("tc-parent") && k.account == "two");
            assert!(r.borrowed.contains("tc-chat") && r.auto.contains_key("tc-chat"));
            assert_eq!(r.names.get("tc-chat").map(String::as_str), Some("the chat"));
            assert!(!r.owed[&key("tc-parent")].cut_short, "its answer was in before the core went");
            assert!(r.owed[&key("tc-peer")].cut_short, "this one never will be");
            assert_eq!(r.seen.get(&key("tc-parent")), Some(&5));
            assert!(r.held.iter().any(|h| h.id == "tc-chat" && h.text == "held across"));
        }
        assert_eq!(crate::activity::turns_counted("tc-chat"), Some(6), "the stopped turn counts as ended, as a helper's does");
        let k = reg().lock().unwrap().known["tc-chat"].clone();
        assert!(returning(&k, 0) && returning(&k, SESSION_RETURN_MS - 1));
        assert!(!returning(&k, SESSION_RETURN_MS), "it has had its chance");
        // Held off while it may come back; then the caller hears.
        let o = reg().lock().unwrap().owed[&key("tc-peer")];
        assert!(notice_line(&crate::pty::manager(), "tc-peer", "Session", "tc-chat", &k, o).is_none(), "the window has yet to start it");
        let mut gone = k.clone();
        gone.waiting = false;
        let (said, how) = notice_line(&crate::pty::manager(), "tc-peer", "Session", "tc-chat", &gone, o).unwrap();
        assert!(matches!(how, Settled::Exited));
        assert!(said.unwrap().contains("has not started again"));

        // Text going in before that word is out does not swallow it.
        owe_from("tc-peer", "tc-chat", 6, 30);
        let o = reg().lock().unwrap().owed[&key("tc-peer")];
        assert!(o.resumed && !o.cut_short && o.cursor == 20, "this turn answers; the last one's end is still to be told");

        // Not back, it is not known for good to its callers either.
        let stands = |up: u64| owed_stands(&reg().lock().unwrap().known, "tc-chat", "someone", up);
        reg().lock().unwrap().known.entry("someone".into()).or_insert_with(|| k.clone());
        assert!(stands(0) && !stands(CALLER_RETURN_MS), "a waiting caller is given as long as an unknown one");
        reg().lock().unwrap().known.remove("someone");

        // The window has no such row: what was kept goes.
        {
            let r = reg().lock().unwrap();
            let names: HashMap<String, String> = [("other".to_string(), "x".to_string())].into_iter().collect();
            assert!(gone_from_the_window(&r, &names).contains(&"tc-chat".to_string()));
            assert!(gone_from_the_window(&r, &HashMap::new()).is_empty(), "a window with no list yet says nothing");
            let names: HashMap<String, String> = [("tc-chat".to_string(), "x".to_string())].into_iter().collect();
            assert!(!gone_from_the_window(&r, &names).contains(&"tc-chat".to_string()));
        }

        // The turn it counted as ended stays one that did not finish, core after core.
        let again = ties_of("tc-chat").unwrap();
        let again = SavedChat { id: "tc-second".into(), provider: Provider::Codex, folder: "C:\\p".into(), account_path: "C:\\acc\\openai\\two".into(), turns_ended: 9, transcript: None, mid_turn: false, ties: again };
        take_up_chats(SavedChats { chats: vec![again], ..Default::default() });
        assert!(reg().lock().unwrap().owed[&("tc-parent".to_string(), "tc-second".to_string())].cut_short == false, "this debt was answered");
        let cut: Ties = Ties { owed: vec![("tc-peer".into(), 5, 20)], cut_short: vec!["tc-peer".into()], ..Default::default() };
        let cut = SavedChat { id: "tc-third".into(), provider: Provider::Codex, folder: "C:\\p".into(), account_path: "C:\\acc\\openai\\two".into(), turns_ended: 6, transcript: None, mid_turn: false, ties: cut };
        take_up_chats(SavedChats { chats: vec![cut], ..Default::default() });
        assert!(reg().lock().unwrap().owed[&("tc-peer".to_string(), "tc-third".to_string())].cut_short, "6 > 5 reads as answered but for the mark");
        forget("tc-second");
        forget("tc-third");

        // Started again, it is any chat, and what it keeps stays for the next core.
        register("tc-chat", Provider::Codex, "C:\\p", "C:\\acc\\openai\\two", Some("tc-parent"), false, serde_json::json!({}));
        assert!(!reg().lock().unwrap().known["tc-chat"].waiting);
        let now = chats_now();
        let c = now.chats.iter().find(|c| c.id == "tc-chat").expect("a chat with ties is saved");
        assert_eq!(c.turns_ended, 6);
        assert!(c.ties.borrowed && c.ties.auto_delete && c.ties.owed.len() == 2);
        assert!(now.sends.iter().any(|h| h.id == "tc-chat"));
        reg().lock().unwrap().held.retain(|h| h.id != "tc-chat");
        forget("tc-chat");
        crate::activity::forget("tc-chat");
    }

    #[test]
    fn a_chat_with_nothing_to_keep_or_a_helper_is_not_saved_as_one() {
        let none = serde_json::json!({});
        register("sv-plain", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, false, none.clone());
        register("sv-bg", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("sv-plain"), false, serde_json::json!({ "background": true }));
        register("sv-child", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("sv-plain"), false, none);
        let ids: Vec<String> = chats_now().chats.into_iter().map(|c| c.id).filter(|id| id.starts_with("sv-")).collect();
        assert_eq!(ids, ["sv-child"], "helpers.json has the background one");
        for id in ["sv-plain", "sv-bg", "sv-child"] {
            forget(id);
        }
    }

    #[test]
    fn a_helper_is_told_of_the_parent_it_passes_to_and_a_chat_as_it_starts() {
        let (account, none) = ("C:\\acc\\anthropic\\one", serde_json::json!({}));
        let background = serde_json::json!({ "background": true });
        register("tp-grand", Provider::Claude, "C:\\p", account, None, true, none.clone());
        register("tp-parent", Provider::Claude, "C:\\p", account, Some("tp-grand"), true, none.clone());
        register("tp-kid", Provider::Claude, "C:\\p", account, Some("tp-parent"), false, background);
        crate::background::take_up_for_test("tp-kid", "C:\\p", 1, &[], None);
        crate::background::set_parent("tp-kid", Some("tp-parent".into()));
        register("tp-chat", Provider::Claude, "C:\\p", account, Some("tp-parent"), false, none);

        forget("tp-parent");
        assert_eq!(crate::background::told_parent_for_test("tp-kid").as_deref(), Some("tp-grand"), "a background helper's next turn");
        // A running chat keeps what it started with; its next start goes by the
        // record, whatever parent the window last heard of.
        assert_eq!(parent_on_start("tp-chat", Some("tp-parent".into())).as_deref(), Some("tp-grand"));
        assert_eq!(parent_on_start("tp-unknown", Some("tp-parent".into())).as_deref(), Some("tp-parent"), "no record: the window's word");

        forget("tp-grand");
        assert_eq!(crate::background::told_parent_for_test("tp-kid"), None, "nobody left to answer to");
        assert_eq!(parent_on_start("tp-chat", Some("tp-parent".into())), None, "an orphan, whatever its row still names");
        crate::background::remove("tp-kid");
        forget("tp-kid");
        forget("tp-chat");
    }

    #[test]
    fn the_core_keeps_the_one_record_of_a_chats_parent_across_a_restart() {
        // Passed up while the window was not there to hear, and saved so.
        let saved = |id: &str, parent: Option<&str>, orphan: bool| SavedChat {
            id: id.into(),
            provider: Provider::Claude,
            folder: "C:\\p".into(),
            account_path: "C:\\acc\\anthropic\\one".into(),
            turns_ended: 0,
            transcript: None,
            mid_turn: false,
            ties: Ties { parent: parent.map(str::to_owned), orphan, ..Default::default() },
        };
        take_up_chats(SavedChats { chats: vec![saved("rp-kid", Some("rp-grand"), false), saved("rp-orphan", None, true)], ..Default::default() });
        // The window starts each under the parent it last heard of.
        register("rp-kid", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("rp-gone"), false, serde_json::json!({}));
        assert_eq!(parent_of("rp-kid").as_deref(), Some("rp-grand"), "the core's record stands");
        register("rp-orphan", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("rp-gone"), false, serde_json::json!({}));
        assert_eq!(parent_of("rp-orphan"), None, "an orphan stays one");
        // With no record, as of a chat with nothing kept, the window's word is the one.
        register("rp-new", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("rp-parent"), false, serde_json::json!({}));
        assert_eq!(parent_of("rp-new").as_deref(), Some("rp-parent"));
        // A chat's orphan mark is itself kept for the next core.
        assert!(chats_now().chats.iter().any(|c| c.id == "rp-orphan" && c.ties.orphan));
        for id in ["rp-kid", "rp-orphan", "rp-new"] {
            forget(id);
        }
    }

    #[test]
    fn a_visible_helper_may_resume_on_restart_and_the_window_never_hears_of_it() {
        let p: SpawnParams = serde_json::from_value(serde_json::json!({ "prompt": "go", "visible": true, "resumeOnRestart": true })).unwrap();
        assert!(spawn("nobody", p).err().unwrap().contains("not a registered session"));
        let p: SpawnParams = serde_json::from_value(serde_json::json!({ "prompt": "go", "resumeOnRestart": true })).unwrap();
        assert_eq!(p.resume_on_restart, Some(true));
        assert!(serde_json::to_value(&p).unwrap().get("resumeOnRestart").is_none(), "Luna's to act on, not the window's");
    }

    /// A chat as the last core left it: spawned by `parent`, its turn cut short
    /// with `flag` or not, and owed to `parent` from count 3 on.
    fn cut_short_chat(id: &str, parent: &str, flag: bool, mid_turn: bool, already: bool) -> SavedChat {
        let settings = if flag { serde_json::json!({ "resumeOnRestart": true }) } else { serde_json::json!({}) };
        SavedChat {
            id: id.into(),
            provider: Provider::Claude,
            folder: "C:\\p".into(),
            account_path: "C:\\acc\\anthropic\\one".into(),
            turns_ended: 3,
            transcript: None,
            mid_turn,
            ties: Ties {
                parent: Some(parent.into()),
                settings,
                owed: vec![(parent.into(), 3, 40)],
                seen: vec![(parent.into(), 3)],
                cut_short: if already { vec![parent.into()] } else { Vec::new() },
                ..Default::default()
            },
        }
    }

    fn take_up(chats: Vec<SavedChat>, sends: Vec<SavedSend>, spawns: Vec<SpawnTaken>) {
        take_up_chats(serde_json::from_slice(&serde_json::to_vec(&SavedChats { chats, sends, spawns }).unwrap()).unwrap());
    }

    #[test]
    fn a_visible_helper_cut_short_with_the_flag_waits_to_go_on_and_says_nothing_yet() {
        take_up(vec![cut_short_chat("vr-chat", "vr-parent", true, true, false)], Vec::new(), Vec::new());
        assert!(resume_pending("vr-chat"));
        assert!(agent_resumes().contains(&"vr-chat".to_string()), "the window is asked to start it");
        // Its turn counts as ended, but it is no answer while it may yet go on.
        let (k, o) = {
            let r = reg().lock().unwrap();
            (r.known["vr-chat"].clone(), r.owed[&("vr-parent".to_string(), "vr-chat".to_string())])
        };
        assert!(o.cut_short);
        assert_eq!(crate::activity::turns_counted("vr-chat"), Some(4));
        let pty = crate::pty::manager();
        assert!(notice_line(&pty, "vr-parent", "Your helper", "vr-chat", &k, Owed { held: Some((4, 0)), ..o }).is_none());
        // Told to start, it is no longer the window's to start, and keeps its flag.
        register("vr-chat", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("vr-parent"), false, serde_json::json!({ "model": "sonnet" }));
        assert!(!agent_resumes().contains(&"vr-chat".to_string()));
        assert!(resumes_on_restart("vr-chat"), "the window's settings do not carry it");
        // Typed in: both notices are owed, the first now.
        resumed("vr-chat", 4);
        reg().lock().unwrap().resumes.remove("vr-chat");
        let o = reg().lock().unwrap().owed[&("vr-parent".to_string(), "vr-chat".to_string())];
        assert!(o.resumed && !o.cut_short && o.baseline == 4 && o.cursor == 40, "all it said since it was asked, past the new turn");
        forget("vr-chat");
        assert!(!resume_pending("vr-chat"));
    }

    #[test]
    fn called_off_a_helper_gets_the_plain_notice_and_is_not_carried_on_by_the_next_core() {
        take_up(vec![cut_short_chat("co-chat", "co-parent", true, true, false)], Vec::new(), Vec::new());
        call_off_resume("co-chat", "a person has typed into it");
        assert!(!resume_pending("co-chat"));
        let o = reg().lock().unwrap().owed[&("co-parent".to_string(), "co-chat".to_string())];
        assert!(o.cut_short && !o.resumed, "the plain did NOT finish");
        // A core going before that notice: the mark comes back, no new turn with it.
        let saved = {
            let r = reg().lock().unwrap();
            ties_in(&r, "co-chat").unwrap()
        };
        assert_eq!(saved.cut_short, vec!["co-parent".to_string()]);
        forget("co-chat");
        take_up(vec![cut_short_chat("co-chat", "co-parent", true, false, true)], Vec::new(), Vec::new());
        assert!(!resume_pending("co-chat"), "called off stays off");
        forget("co-chat");
        // Without the flag, or nothing cut short, nothing to carry on.
        take_up(vec![cut_short_chat("co-plain", "co-parent", false, true, false)], Vec::new(), Vec::new());
        assert!(!resume_pending("co-plain"));
        forget("co-plain");
        let mut done = cut_short_chat("co-done", "co-parent", true, false, false);
        done.turns_ended = 4;
        take_up(vec![done], Vec::new(), Vec::new());
        assert!(!resume_pending("co-done"), "its answer was in as the core went");
        forget("co-done");
        // A turn running that nobody is owed the end of goes on all the same.
        let mut own = cut_short_chat("co-own", "co-parent", true, true, false);
        own.ties.owed.clear();
        take_up(vec![own], Vec::new(), Vec::new());
        assert!(resume_pending("co-own"));
        forget("co-own");
    }

    #[test]
    fn a_chat_goes_on_only_up_drawn_and_untouched_and_gives_up_in_time() {
        let up = 1000;
        assert_eq!(resume_look(true, up, false, false, false), Ok(false), "the window has yet to start it");
        assert!(resume_look(true, SESSION_RETURN_MS, false, false, false).is_err(), "and did not");
        assert_eq!(resume_look(false, up, true, false, false), Ok(false), "up, not drawn or not idle");
        assert_eq!(resume_look(false, up, true, false, true), Ok(true));
        assert!(resume_look(false, up, true, true, true).is_err(), "a person typed: never");
        assert!(resume_look(false, up, false, false, false).is_err(), "its CLI went");
        assert_eq!(resume_look(false, CALLER_RETURN_MS - 1, true, false, false), Ok(false), "a CLI can take minutes to draw");
        assert!(resume_look(false, CALLER_RETURN_MS, true, false, false).is_err());
    }

    #[test]
    fn a_chat_is_told_why_with_what_was_held_or_given_its_prompt_with_no_conversation() {
        let batch = [crate::background::Queued { from: Some("x".into()), text: "the held text".into() }];
        let told = resume_words(true, Some("the prompt".into()), &batch).unwrap();
        assert!(told.starts_with(NOTICE_TAG) && told.contains("core restarted") && told.contains("Carry on") && told.contains("the held text"), "{told}");
        assert!(!told.contains("the prompt"), "its conversation has that");
        assert_eq!(resume_words(false, Some("the prompt".into()), &batch).unwrap(), "the prompt\n\nthe held text", "a first turn, on what it was given");
        assert_eq!(resume_words(false, None, &batch), None, "nothing to go on from");
    }

    #[test]
    fn text_held_across_a_core_waits_for_the_resume_and_a_saved_opening_comes_back() {
        let mut chat = cut_short_chat("ho-chat", "ho-parent", true, true, false);
        chat.turns_ended = 0;
        chat.ties.owed = vec![("ho-parent".into(), 0, 0)];
        chat.ties.opening = Some("do it".into());
        take_up(vec![chat], vec![SavedSend { caller: "ho-parent".into(), id: "ho-chat".into(), text: "and this".into(), going_in: None }], Vec::new());
        assert_eq!(reg().lock().unwrap().resumes["ho-chat"].opening.as_deref(), Some("do it"));
        assert_eq!(crate::activity::turns_counted("ho-chat"), Some(1), "its turn counts as gone with the core");
        let saved = || chats_now().chats.into_iter().find(|c| c.id == "ho-chat").unwrap().ties.opening;
        assert_eq!(saved().as_deref(), Some("do it"), "that count is no first turn: a second restart still has the prompt");
        crate::activity::keep_count("ho-chat", 2);
        assert_eq!(saved(), None, "one that ended under this core is");
        assert!(is_held("ho-parent", "ho-chat"));
        // `deliver_held` looks and leaves it (`resume_pending`), the session not
        // running here and so the same in either case while it may yet return.
        deliver_held(&crate::pty::manager());
        assert!(is_held("ho-parent", "ho-chat"));
        reg().lock().unwrap().held.retain(|h| h.id != "ho-chat");
        forget("ho-chat");
        assert!(!reg().lock().unwrap().openings.contains_key("ho-chat"));
    }

    /// `settle_spawns` and its like go over every spawn there is, a test's or
    /// another's, as they do in a core: one test at a time.
    fn spawns_to_myself() -> std::sync::MutexGuard<'static, ()> {
        static ONE: Mutex<()> = Mutex::new(());
        ONE.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn spawn_taken(request: u64, caller: &str, helper: Option<&str>, resume: bool) -> SpawnTaken {
        SpawnTaken {
            request,
            caller: caller.into(),
            params: serde_json::from_value(serde_json::json!({ "prompt": "the whole job, word for word", "name": "the job" })).unwrap(),
            auto_delete: None,
            resume_on_restart: resume,
            visible: true,
            in_worktree: None,
            account_path: "C:\\acc\\anthropic\\one".into(),
            worktrees_before: Vec::new(),
            asked: None,
            helper: helper.map(str::to_owned),
            owed_placed: false,
            left_by_core: false,
            settled: false,
            news: None,
            retried: false,
        }
    }

    #[test]
    fn a_spawn_is_on_disk_for_as_long_as_it_is_under_way() {
        let params: SpawnParams = serde_json::from_value(serde_json::json!({ "prompt": "go", "visible": true, "resumeOnRestart": true, "autoDelete": true })).unwrap();
        let saved = |request: u64| chats_now().spawns.into_iter().find(|s| s.request == request);
        let under_way = SpawnUnderWay::begin("uw-caller", &params, "C:\\acc\\anthropic\\one", Some("C:\\p\\wt"), None);
        let request = under_way.0;
        let s = saved(request).unwrap();
        assert_eq!((s.caller.as_str(), s.params.prompt.as_str(), s.in_worktree.as_deref()), ("uw-caller", "go", Some("C:\\p\\wt")));
        assert!(s.resume_on_restart && s.visible && s.auto_delete == Some(true), "what the window never hears of");
        // Its session registers, and what the window does not carry goes on it.
        register("uw-helper", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("uw-caller"), false, serde_json::json!({}));
        spawn_is(request, "uw-helper").unwrap();
        assert_eq!(saved(request).unwrap().helper.as_deref(), Some("uw-helper"));
        assert!(resumes_on_restart("uw-helper"));
        {
            let r = reg().lock().unwrap();
            assert!(r.borrowed.contains("uw-helper") && r.auto.contains_key("uw-helper"));
            assert_eq!(ties_in(&r, "uw-helper").unwrap().opening.as_deref(), Some("go"), "as it was asked");
            assert_eq!(ties_in(&r, "uw-helper").unwrap().worktrees_before, None, "no worktree was asked for");
        }
        drop(under_way);
        assert!(saved(request).is_none(), "seen through, nothing is left to tell");
        forget("uw-helper");
    }

    #[test]
    fn what_the_window_was_asked_stays_there_to_ask_for_until_a_session_comes_of_it() {
        let _one = spawns_to_myself();
        let pending = || agent_spawns().into_iter().map(|s| s.request_id).filter(|r| (9_000_500..9_000_600).contains(r)).collect::<Vec<u64>>();
        let put = |request: u64| {
            let mut s = spawn_taken(request, "as-caller", None, false);
            s.asked = Some(SpawnRequest { request_id: request, parent_id: "as-caller".into(), in_worktree: None, hooks_unasked: false, params: s.params.clone() });
            reg().lock().unwrap().spawns.insert(request, s);
        };
        put(9_000_502);
        put(9_000_501);
        // A background spawn asks the window nothing.
        reg().lock().unwrap().spawns.insert(9_000_503, spawn_taken(9_000_503, "as-caller", None, false));
        assert_eq!(pending(), [9_000_501, 9_000_502], "in the order they were asked");
        let wire = serde_json::to_value(&agent_spawns().into_iter().find(|s| s.request_id == 9_000_501).unwrap()).unwrap();
        assert_eq!((wire["requestId"].as_u64(), wire["parentId"].as_str(), wire["prompt"].as_str()), (Some(9_000_501), Some("as-caller"), Some("the whole job, word for word")), "as the event carries it");

        // Its session registers: the window has it, whatever comes of the page.
        register("as-helper", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("as-caller"), false, serde_json::json!({}));
        spawn_is(9_000_501, "as-helper").unwrap();
        assert_eq!(pending(), [9_000_502]);
        // Given up on: a session would be refused, and its caller has been told.
        reg().lock().unwrap().spawns.get_mut(&9_000_502).unwrap().settled = true;
        assert!(pending().is_empty());
        // A core after this one settles it from the file, and asks for nothing.
        let saved = serde_json::to_vec(&reg().lock().unwrap().spawns[&9_000_501]).unwrap();
        let back: SpawnTaken = serde_json::from_slice(&saved).unwrap();
        assert!(back.asked.is_none());
        // The record gone, so is the asking.
        put(9_000_504);
        assert_eq!(pending(), [9_000_504]);
        drop(SpawnUnderWay(9_000_504));
        assert!(pending().is_empty());
        forget("as-helper");
        reg().lock().unwrap().spawns.retain(|request, _| !(9_000_500..9_000_600).contains(request));
    }

    #[test]
    fn a_spawn_keeps_what_was_asked_with_or_without_the_flag_and_a_background_helper_its_own_way() {
        let spawned = |ask: serde_json::Value, id: &str| {
            let params: SpawnParams = serde_json::from_value(ask).unwrap();
            let taken = SpawnUnderWay::begin("ka-caller", &params, "C:\\no\\such\\account", None, None);
            register(id, Provider::Codex, "C:\\no\\such\\folder", "C:\\no\\such\\account", Some("ka-caller"), false, serde_json::json!({}));
            spawn_is(taken.0, id).unwrap();
            let r = reg().lock().unwrap();
            ties_in(&r, id).unwrap()
        };
        // Claude Code's worktree goes by a name Luna gave it.
        let ties = spawned(serde_json::json!({ "prompt": "go", "visible": true, "worktree": true, "folder": "C:\\no\\such\\folder" }), "ka-visible");
        assert_eq!(ties.opening.as_deref(), Some("go"));
        assert_eq!(ties.worktree_name.as_deref(), Some("luna-ka-visible"));
        assert_eq!(ties.worktrees_before, None);
        // Codex takes no name: what is new since is all that says what it began.
        let ties = spawned(serde_json::json!({ "prompt": "go", "visible": true, "worktree": true, "provider": "codex", "folder": "C:\\no\\such\\folder" }), "ka-codex");
        assert_eq!(ties.worktree_name, None);
        assert_eq!(ties.worktrees_before, Some(Vec::new()), "none yet is a word too: all that come after may be its");
        let ties = spawned(serde_json::json!({ "prompt": "go", "worktree": true, "folder": "C:\\no\\such\\folder" }), "ka-background");
        assert_eq!(ties.opening, None, "`Entry::opening` has that");
        assert_eq!(ties.worktree_name, None, "its `Spec` has that");
        forget("ka-visible");
        forget("ka-codex");
        forget("ka-background");
    }

    /// A visible helper as the last core left it, its first turn having gone with
    /// it: asked `opening`, owed to `parent`, with `held` from that parent waiting.
    fn first_turn_cut(id: &str, parent: &str, provider: Provider, opening: &str, held: &[&str]) {
        let mut chat = cut_short_chat(id, parent, false, true, false);
        chat.provider = provider;
        chat.turns_ended = 0;
        chat.ties.owed = vec![(parent.into(), 0, 0)];
        chat.ties.seen = Vec::new();
        chat.ties.opening = Some(opening.into());
        let sends = held.iter().map(|text| SavedSend { caller: parent.into(), id: id.into(), text: (*text).into(), going_in: None }).collect();
        take_up(vec![chat], sends, Vec::new());
    }

    /// The window starts it: no longer waiting for that.
    fn back(id: &str, parent: &str, provider: Provider) {
        register(id, provider, "C:\\p", "C:\\acc\\anthropic\\one", Some(parent), false, serde_json::json!({}));
    }

    /// What the file a notice names holds, the notice's words before the name given.
    fn file_after(line: &str, before: &str) -> String {
        let at = line.find(before).unwrap_or_else(|| panic!("{before:?} not in {line}")) + before.len();
        let path = line[at..].split(".md").next().unwrap();
        let path = format!("{path}.md");
        let text = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        text
    }

    #[test]
    fn a_visible_helper_back_with_nothing_never_got_started_and_gives_back_what_it_was_asked() {
        // Both CLIs alike. One from a file an older core wrote has the note on it.
        for (id, provider, kept) in [
            ("fs-claude", Provider::Claude, "the whole job\nword for word".to_string()),
            ("fs-codex", Provider::Codex, crate::pty::codex_helper_prompt("fs-parent", "the whole job\nword for word")),
        ] {
            first_turn_cut(id, "fs-parent", provider, &kept, &["and a follow-up"]);
            back(id, "fs-parent", provider);
            chat_starting(id, false);
            crate::activity::between_turns_for_test(id);
            let (line, how) = notice_for("fs-parent", id, 1);
            let line = line.unwrap();
            assert!(matches!(how, Settled::Answered));
            assert!(line.contains("never got started") && line.contains("luna_send starts it afresh") && !line.contains("did NOT finish"), "{line}");
            let held = file_after(&line, "its prompt and 1 queued message are in ");
            assert!(held.find("the whole job\nword for word").unwrap() < held.find("and a follow-up").unwrap(), "{held}");
            assert!(!held.contains("A note from Luna"), "what it asked, not what Luna added: {held}");

            // Only its own sender hears of it, and read or waited on, it goes with
            // that response; a cancel crossing it brings it back.
            assert!(undelivered_note("fs-nobody", id).is_none());
            let note = undelivered_note("fs-parent", id).unwrap();
            assert!(note.contains("never got started") && note.contains("its prompt and 1 queued message are in "), "{note}");
            let undo = hand_over("fs-parent", id, 1, None);
            assert!(undelivered_note("fs-parent", id).is_none() && !is_held("fs-parent", id));
            assert!(!first_turn_lost(&crate::pty::manager(), id), "told: from here on it is as any other");
            undo();
            assert!(undelivered_note("fs-parent", id).is_some() && is_held("fs-parent", id));

            reg().lock().unwrap().held.retain(|h| h.id != id);
            forget(id);
        }
    }

    #[test]
    fn what_a_visible_helper_lost_stays_lost_through_a_turn_it_was_not_asked_for_and_across_a_core() {
        first_turn_cut("fl-chat", "fl-parent", Provider::Claude, "the job", &[]);
        back("fl-chat", "fl-parent", Provider::Claude);
        chat_starting("fl-chat", false);
        // A person typed into it, and a turn on that ended: the job is no nearer.
        crate::activity::keep_count("fl-chat", 5);
        assert!(first_turn_lost(&crate::pty::manager(), "fl-chat"));
        assert_eq!(chats_now().chats.into_iter().find(|c| c.id == "fl-chat").unwrap().ties.opening.as_deref(), Some("the job"), "a count does not settle this");
        // A core going before its parent hears.
        let saved = chats_now().chats.into_iter().find(|c| c.id == "fl-chat").unwrap();
        assert!(saved.ties.opening_lost);
        forget("fl-chat");
        take_up(vec![saved], Vec::new(), Vec::new());
        assert!(reg().lock().unwrap().openings["fl-chat"].lost);
        forget("fl-chat");
    }

    #[test]
    fn a_visible_helper_with_a_conversation_gets_the_plain_notice() {
        let path = std::env::temp_dir().join(format!("luna-fc-{}.jsonl", std::process::id()));
        std::fs::write(&path, "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"the job\"}}\n").unwrap();
        first_turn_cut("fc-chat", "fc-parent", Provider::Claude, "the job", &[]);
        transcripts().lock().unwrap().insert("fc-chat".into(), path.clone());
        back("fc-chat", "fc-parent", Provider::Claude);
        chat_starting("fc-chat", true);
        crate::activity::between_turns_for_test("fc-chat");
        assert!(!first_turn_lost(&crate::pty::manager(), "fc-chat"));
        let line = notice_for("fc-parent", "fc-chat", 1).0.unwrap();
        assert!(line.contains("did NOT finish") && line.contains("luna_send has it carry on") && !line.contains("never got started"), "{line}");
        assert!(undelivered_note("fc-parent", "fc-chat").is_none(), "its conversation holds it");
        forget("fc-chat");
        let _ = std::fs::remove_file(&path);

        // A first turn no core has been near: a CLI started afresh on nothing is
        // no loss of anything.
        register("fc-first", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("fc-parent"), false, serde_json::json!({}));
        reg().lock().unwrap().openings.insert("fc-first".into(), Opening { text: "go".into(), floor: 0, lost: false, at_ms: 0 });
        chat_starting("fc-first", false);
        assert!(!first_turn_lost(&crate::pty::manager(), "fc-first"));
        forget("fc-first");
    }

    #[test]
    fn a_visible_helper_not_back_in_time_is_told_of_by_what_is_on_disk() {
        // No conversation on disk: its text stays for this notice, not dropped as
        // lost to a session that went (`deliver_held`).
        first_turn_cut("fb-none", "fb-parent", Provider::Codex, "the whole job", &["and a follow-up"]);
        back("fb-none", "fb-parent", Provider::Codex);
        deliver_held(&crate::pty::manager());
        assert!(is_held("fb-parent", "fb-none"));
        reg().lock().unwrap().worktrees_before.insert("fb-none".into(), Vec::new());
        let (line, how) = notice_for("fb-parent", "fb-none", 1);
        let line = line.unwrap();
        assert!(matches!(how, Settled::Exited));
        assert!(line.contains("never got started") && line.contains("has not started again") && line.contains("luna_delete"), "{line}");
        assert!(!line.contains("luna_send"), "no sending to one not running: {line}");
        let held = file_after(&line, "its prompt and 1 queued message are in ");
        assert!(held.find("the whole job").unwrap() < held.find("and a follow-up").unwrap(), "{held}");
        assert!(!reg().lock().unwrap().worktrees_before.contains_key("fb-none"), "told of what it began, it makes its own if opened");
        reg().lock().unwrap().held.retain(|h| h.id != "fb-none");
        forget("fb-none");

        // One on disk: it comes back to that when opened.
        let path = std::env::temp_dir().join(format!("luna-fb-{}.jsonl", std::process::id()));
        std::fs::write(&path, "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"the job\"}}\n").unwrap();
        first_turn_cut("fb-said", "fb-parent", Provider::Claude, "the job", &[]);
        transcripts().lock().unwrap().insert("fb-said".into(), path.clone());
        back("fb-said", "fb-parent", Provider::Claude);
        let line = notice_for("fb-parent", "fb-said", 1).0.unwrap();
        assert!(line.contains("did NOT finish") && line.contains("runs again once the user opens it"), "{line}");
        forget("fb-said");
        let _ = std::fs::remove_file(&path);
    }

    /// A throwaway repository with worktrees made as Claude Code makes them.
    fn repo_with(tag: &str, worktrees: &[&str]) -> Option<(std::path::PathBuf, String, Vec<String>)> {
        let root = std::env::temp_dir().join(format!("luna-rw-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).ok()?;
        let repo = root.to_string_lossy().into_owned();
        let git = |dir: &str, args: &[&str]| {
            let mut c = std::process::Command::new("git");
            c.arg("-C").arg(dir).args(["-c", "user.email=t@t", "-c", "user.name=t"]).args(args);
            c.output().is_ok_and(|o| o.status.success())
        };
        if !git(&repo, &["init", "-q"]) || !git(&repo, &["commit", "-q", "--allow-empty", "-m", "i"]) {
            return None;
        }
        let mut made = Vec::new();
        for name in worktrees {
            let wt = crate::worktree::named_path(&repo, name);
            if !git(&repo, &["worktree", "add", "-q", "-b", &format!("worktree-{name}"), &wt]) {
                return None;
            }
            made.push(wt);
        }
        Some((root, repo, made))
    }

    /// A visible helper its name was given, as `spawn_is` gives it.
    fn named_helper(id: &str, repo: &str, name: &str) {
        register(id, Provider::Claude, repo, "C:\\no\\such\\account", Some("rw-parent"), false, serde_json::json!({}));
        reg().lock().unwrap().worktree_names.insert(id.to_string(), name.to_string());
    }

    #[test]
    fn a_fresh_start_goes_back_only_into_the_worktree_luna_named_and_only_as_made() {
        let Some((root, repo, made)) = repo_with("reuse", &["luna-a", "luna-b", "someone-elses"]) else { return };
        let (a, b, elses) = (&made[0], &made[1], &made[2]);
        let start = |id: &str| worktree_for_start(id, &repo, "C:\\no\\such\\account", false);

        // Its own, as made. The clean one beside it, new as it is, is nobody's to take.
        named_helper("rw-a", &repo, "luna-a");
        assert_eq!(start("rw-a").as_deref(), Some("luna-a"));
        assert_eq!(worktrees_left_line("rw-a"), "");
        // Not there: Claude Code makes it by that name.
        named_helper("rw-new", &repo, "luna-new");
        assert_eq!(start("rw-new").as_deref(), Some("luna-new"));
        // Named none, a person's chat: its CLI names its own, and nothing is taken.
        register("rw-none", Provider::Claude, &repo, "C:\\no\\such\\account", None, false, serde_json::json!({}));
        assert_eq!(start("rw-none"), None);

        // Something in it: left alone, told of, and the next name from here on.
        std::fs::write(std::path::Path::new(b).join("half-done.txt"), "work").unwrap();
        named_helper("rw-b", &repo, "luna-b");
        assert_eq!(start("rw-b").as_deref(), Some("luna-b-2"));
        let said = worktrees_left_line("rw-b");
        assert!(said.contains("worktreeLeft") && said.contains(b.as_str()) && !said.contains(elses.as_str()) && !said.contains(a.as_str()), "{said}");
        assert_eq!(reg().lock().unwrap().worktree_names["rw-b"], "luna-b-2");
        assert_eq!(start("rw-b").as_deref(), Some("luna-b-2"));
        assert_eq!(worktrees_left_line("rw-b"), said, "the one it left is not told of twice");
        // In a conversation it is its own, whatever is in it.
        named_helper("rw-talk", &repo, "luna-b");
        assert_eq!(worktree_for_start("rw-talk", &repo, "C:\\no\\such\\account", true).as_deref(), Some("luna-b"));
        // A name that would be a flag never goes out as one.
        named_helper("rw-flag", &repo, "--dangerously-skip-permissions");
        assert_eq!(start("rw-flag"), None);

        // Told once: with what is told first, or with a read; a cancel brings it back.
        let note = undelivered_note("rw-parent", "rw-b").unwrap();
        assert!(note.starts_with("worktreeLeft") && note.contains(b.as_str()), "{note}");
        assert!(undelivered_note("rw-nobody", "rw-b").is_none());
        let undo = hand_over("rw-parent", "rw-b", 1, None);
        assert_eq!(worktrees_left_line("rw-b"), "");
        undo();
        assert!(worktrees_left_line("rw-b").contains(b.as_str()));

        // Written down, it comes back with the helper.
        let ties = {
            let r = reg().lock().unwrap();
            ties_in(&r, "rw-b").unwrap()
        };
        assert_eq!(ties.worktrees_left, [b.clone()]);
        assert_eq!(ties.worktree_name.as_deref(), Some("luna-b-2"));
        let mut chat = cut_short_chat("rw-back", "rw-parent", false, true, false);
        chat.ties.worktree_name = Some("luna-back".into());
        chat.ties.worktrees_left = vec!["b".into()];
        take_up(vec![chat], Vec::new(), Vec::new());
        {
            let r = reg().lock().unwrap();
            assert_eq!(r.worktree_names["rw-back"], "luna-back");
            assert_eq!(r.worktrees_left["rw-back"], ["b"]);
        }
        for id in ["rw-a", "rw-new", "rw-none", "rw-b", "rw-talk", "rw-flag", "rw-back"] {
            forget(id);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_helper_not_back_has_what_luna_named_told_of_and_then_makes_another() {
        let Some((root, repo, made)) = repo_with("notback", &["luna-gone"]) else { return };
        named_helper("nb-chat", &repo, "luna-gone");
        let k = reg().lock().unwrap().known["nb-chat"].clone();
        leave_half_made("nb-chat", &k);
        assert!(worktrees_left_line("nb-chat").contains(made[0].as_str()));
        assert_eq!(reg().lock().unwrap().worktree_names["nb-chat"], "luna-gone-2");
        forget("nb-chat");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_conversation_on_disk_is_gone_back_to_and_never_told_of_as_never_started() {
        let root = std::env::temp_dir().join(format!("luna-rc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let folder = root.join("proj").to_string_lossy().into_owned();
        std::fs::create_dir_all(&folder).unwrap();
        let account = root.join("anthropic").to_string_lossy().into_owned();
        // Started by no window yet, a look at it goes by where it was to work.
        let on_disk = |id: &str, provider: Provider, account: &str| {
            register(id, provider, &folder, account, Some("rc-parent"), false, serde_json::json!({}));
        };
        let user_line = |text: &str| format!("{}\n", serde_json::json!({ "type": "user", "message": { "role": "user", "content": text } }));

        // Claude Code: the window has not heard, and what it was asked is in one begun since.
        first_turn_cut("rc-claude", "rc-parent", Provider::Claude, "the whole job", &[]);
        on_disk("rc-claude", Provider::Claude, &account);
        assert_eq!(conversation_of("rc-claude", Provider::Claude, &folder, &account), None, "nothing there yet");
        assert!(first_turn_lost(&crate::pty::manager(), "rc-claude"));
        let path = crate::claude::session::transcript_file(&account, &folder, "11111111-aaaa");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, user_line("some other job")).unwrap();
        assert_eq!(conversation_of("rc-claude", Provider::Claude, &folder, &account), None, "not what it was asked: another session's");
        std::fs::write(&path, user_line("the  whole\njob")).unwrap();
        assert!(!first_turn_lost(&crate::pty::manager(), "rc-claude"), "not back, with a conversation to come back to");
        let line = notice_for("rc-parent", "rc-claude", 1).0.unwrap();
        assert!(line.contains("did NOT finish") && !line.contains("never got started"), "{line}");
        reg().lock().unwrap().openings.get_mut("rc-claude").unwrap().at_ms = now_ms() + 3_600_000;
        assert_eq!(conversation_of("rc-claude", Provider::Claude, &folder, &account), None, "begun before it was asked: an older one's");
        reg().lock().unwrap().openings.get_mut("rc-claude").unwrap().at_ms = 0;
        // Another session's, known to be, is not to be taken for it.
        transcripts().lock().unwrap().insert("rc-other".into(), path.clone());
        assert_eq!(conversation_of("rc-claude", Provider::Claude, &folder, &account), None);
        transcripts().lock().unwrap().remove("rc-other");
        assert_eq!(conversation_of("rc-claude", Provider::Claude, &folder, &account).as_deref(), Some("11111111-aaaa"));
        // It goes back: no prompt is lost, and the caller hears it did not finish.
        chat_starting("rc-claude", true);
        assert!(!reg().lock().unwrap().openings["rc-claude"].lost);
        assert!(undelivered_note("rc-parent", "rc-claude").is_none());
        forget("rc-claude");

        // Whatever the last core knew of: that one, whatever it holds of the prompt.
        first_turn_cut("rc-kept", "rc-parent", Provider::Claude, "a prompt not in it", &[]);
        on_disk("rc-kept", Provider::Claude, &account);
        transcripts().lock().unwrap().insert("rc-kept".into(), path.clone());
        assert_eq!(conversation_of("rc-kept", Provider::Claude, &folder, &account).as_deref(), Some("11111111-aaaa"));
        forget("rc-kept");

        // Codex: by its rollout, in the folder or a worktree of it.
        let account = root.join("openai").to_string_lossy().into_owned();
        first_turn_cut("rc-codex", "rc-parent", Provider::Codex, "the whole job", &[]);
        on_disk("rc-codex", Provider::Codex, &account);
        assert_eq!(conversation_of("rc-codex", Provider::Codex, &folder, &account), None);
        let now = chrono::Local::now();
        let dir = std::path::Path::new(&account).join("sessions").join(now.format("%Y").to_string()).join(now.format("%m").to_string()).join(now.format("%d").to_string());
        std::fs::create_dir_all(&dir).unwrap();
        let rollout = |source: &str, cwd: &str, said: &str, thread: &str| {
            let file = dir.join(format!("rollout-{}-{thread}.jsonl", now.format("%Y-%m-%dT%H-%M-%S")));
            let meta = serde_json::json!({ "type": "session_meta", "payload": { "id": thread, "cwd": cwd, "source": source } });
            let user = serde_json::json!({ "type": "response_item", "payload": { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": said }] } });
            std::fs::write(file, format!("{meta}\n{user}\n")).unwrap();
        };
        rollout("exec", &folder, "the whole job", "aaaa0000-exec");
        rollout("cli", &root.join("elsewhere").to_string_lossy(), "the whole job", "aaaa0000-away");
        rollout("cli", &folder, "another job", "aaaa0000-else");
        assert_eq!(conversation_of("rc-codex", Provider::Codex, &folder, &account), None, "a helper's, another folder's, another's job");
        rollout("cli", &folder, &crate::pty::codex_helper_prompt("rc-parent", "the whole job"), "aaaa0000-mine");
        assert_eq!(conversation_of("rc-codex", Provider::Codex, &folder, &account).as_deref(), Some("aaaa0000-mine"), "with its note on it");
        forget("rc-codex");
        transcripts().lock().unwrap().retain(|k, _| !k.starts_with("rc-"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A caller a wait is not let go by, and a helper that lost its first turn, back
    /// afresh with `held` from it waiting.
    fn lost_helper(caller: &str, id: &str, dir: &str, held: &[&str]) {
        register(caller, Provider::Claude, dir, dir, None, true, serde_json::json!({}));
        crate::background::test_helper(caller, dir, None, false);
        first_turn_cut(id, caller, Provider::Claude, "the whole job", held);
        back(id, caller, Provider::Claude);
        chat_starting(id, false);
    }

    #[test]
    fn a_wait_on_a_lost_first_turn_answers_at_once_though_text_is_held_for_it() {
        let dir = std::env::temp_dir().to_string_lossy().into_owned();
        lost_helper("lw-parent", "lw-chat", &dir, &["and a follow-up"]);
        let key = ("lw-parent".to_string(), "lw-chat".to_string());
        // Held text clears only by going in, and this one is kept back: the wait used
        // to sit its whole time out on it, with nothing to show.
        let call = crate::mcp::Call::new();
        let t0 = std::time::Instant::now();
        let w = wait("lw-parent", "lw-chat", "turn_done", 30, None, &call).unwrap();
        assert!(t0.elapsed() < std::time::Duration::from_secs(5), "{:?}", t0.elapsed());
        assert!(w.failed.as_deref().is_some_and(|f| f.contains("never got started")), "{:?}", w.failed);
        let note = w.undelivered.clone().unwrap();
        assert!(note.contains("its prompt and 1 queued message are in "), "{note}");
        let held = file_after(&note, "its prompt and 1 queued message are in ");
        assert!(held.find("the whole job").unwrap() < held.find("and a follow-up").unwrap(), "{held}");
        // Handed over with the debt, so no notice says it again.
        assert!(!reg().lock().unwrap().owed.contains_key(&key));
        assert!(!is_held("lw-parent", "lw-chat") && !first_turn_lost(&crate::pty::manager(), "lw-chat"));
        // A cancel crossing the response brings back both, together.
        call.cancel();
        assert!(reg().lock().unwrap().owed.contains_key(&key) && is_held("lw-parent", "lw-chat"));
        assert!(first_turn_lost(&crate::pty::manager(), "lw-chat"));
        // A cancel first hands over nothing.
        let call = crate::mcp::Call::new();
        call.cancel();
        assert!(wait("lw-parent", "lw-chat", "turn_done", 30, None, &call).is_err());
        assert!(reg().lock().unwrap().owed.contains_key(&key) && is_held("lw-parent", "lw-chat"));
        // Not running, it is all there is to wait for, whatever the wait was for.
        let w = wait("lw-parent", "lw-chat", "waiting", 1, None, &crate::mcp::Call::new()).unwrap();
        assert!(w.undelivered.is_some());
        assert!(!still_all_lost(&crate::pty::manager(), "lw-nobody", "lw-chat"), "only its parent's to have");

        // Asked something since that went in, it has started on that: the wait is for
        // its answer, and text held since goes in as any does.
        first_turn_cut("lw-sent", "lw-parent", Provider::Claude, "the whole job", &["a later one"]);
        back("lw-sent", "lw-parent", Provider::Claude);
        chat_starting("lw-sent", false);
        assert!(still_all_lost(&crate::pty::manager(), "lw-parent", "lw-sent"));
        owe_from("lw-parent", "lw-sent", 1, 0);
        assert!(!still_all_lost(&crate::pty::manager(), "lw-parent", "lw-sent"));
        assert!(first_turn_lost(&crate::pty::manager(), "lw-sent"), "what it was first asked is lost all the same, and told of with that answer");
        assert!(undelivered_for("lw-sent", "lw-parent", true).iter().any(|t| t == "the whole job"));
        reg().lock().unwrap().held.retain(|h| h.id != "lw-chat" && h.id != "lw-sent");
        forget("lw-sent");
        forget("lw-chat");
        crate::background::gone_for_test("lw-parent");
        forget("lw-parent");
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

    #[test]
    fn what_a_spawn_came_to_decides_what_its_caller_is_told() {
        let _one = spawns_to_myself();
        // One that had come to a session the last core saved: nothing to say of the
        // spawn, and that session owes its caller as if the call had got there.
        register("ws-caller", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, serde_json::json!({}));
        let mut made = cut_short_chat("ws-made", "ws-caller", true, false, false);
        made.turns_ended = 0;
        made.ties.owed.clear();
        made.ties.opening = Some("the opening".into());
        // Asked for again by a request above any the file holds.
        let lost = spawn_taken(9_000_001, "ws-caller", None, false);
        take_up(vec![made], Vec::new(), vec![spawn_taken(9_000_000, "ws-caller", Some("ws-made"), true), lost]);
        assert!(reg().lock().unwrap().next_request > 9_000_001);
        take_up_spawns();
        {
            let r = reg().lock().unwrap();
            let o = r.owed[&("ws-caller".to_string(), "ws-made".to_string())];
            assert!(o.cut_short && o.baseline == 0 && o.cursor == 0);
            assert_eq!(r.resumes["ws-made"].opening.as_deref(), Some("the opening"));
        }
        assert_eq!(crate::activity::turns_counted("ws-made"), Some(1), "its turn went with the core");
        // Nothing before the window has had its chance.
        settle_spawns(SESSION_RETURN_MS - 1);
        assert!(reg().lock().unwrap().spawns[&9_000_001].news.is_none());
        settle_spawns(SESSION_RETURN_MS);
        {
            let r = reg().lock().unwrap();
            assert!(!r.spawns.contains_key(&9_000_000), "a helper came of it");
            let news = r.spawns[&9_000_001].news.clone().unwrap();
            assert!(news.contains("\"the job\"") && news.contains("never completed") && news.contains("core restarted"), "{news}");
            assert!(news.contains("ended in an error"), "it says what became of the call: {news}");
            assert!(!news.contains("worktreeLeft"), "{news}");
            let file = news.split("Its prompt is in ").nth(1).and_then(|l| l.split("; spawn it again").next()).unwrap();
            assert_eq!(std::fs::read_to_string(file).unwrap(), "the whole job, word for word");
            let _ = std::fs::remove_file(file);
        }
        // Told only once it is back, and what was told goes.
        let news = spawn_news(1000);
        assert_eq!(news["ws-caller"].len(), 1);
        reg().lock().unwrap().known.get_mut("ws-caller").unwrap().waiting = true;
        assert!(!spawn_news(1000).contains_key("ws-caller"), "not started yet");
        assert!(reg().lock().unwrap().spawns.contains_key(&9_000_001));
        assert!(!spawn_news(CALLER_RETURN_MS).contains_key("ws-caller"));
        assert!(!reg().lock().unwrap().spawns.contains_key(&9_000_001), "not back in time: to the log");
        forget("ws-made");
        forget("ws-caller");
    }

    #[test]
    fn a_row_the_window_dropped_makes_it_a_spawn_nothing_came_of() {
        let _one = spawns_to_myself();
        register("wd-caller", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, serde_json::json!({}));
        let mut made = cut_short_chat("wd-gone", "wd-caller", false, false, false);
        made.ties.owed.clear();
        take_up(vec![made], Vec::new(), vec![spawn_taken(9_000_100, "wd-caller", Some("wd-gone"), false)]);
        take_up_spawns();
        // `chat_names` finds no row for it: the caller's debt goes with it.
        forget("wd-gone");
        assert!(!reg().lock().unwrap().owed.contains_key(&("wd-caller".to_string(), "wd-gone".to_string())));
        settle_spawns(SESSION_RETURN_MS);
        let news = reg().lock().unwrap().spawns[&9_000_100].news.clone().unwrap();
        assert!(news.contains("never completed"), "{news}");
        forget("wd-caller");
        assert!(!reg().lock().unwrap().spawns.contains_key(&9_000_100));
    }

    #[test]
    fn the_window_finishing_a_spawn_with_the_next_core_owes_its_caller() {
        let _one = spawns_to_myself();
        register("wf-caller", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, serde_json::json!({}));
        take_up(Vec::new(), Vec::new(), vec![spawn_taken(9_000_200, "wf-caller", None, false), spawn_taken(9_000_201, "wf-caller", None, false)]);
        register("wf-helper", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("wf-caller"), false, serde_json::json!({}));
        agent_spawned(9_000_200, Some("wf-helper".into()), Some("named".into()), None);
        {
            let r = reg().lock().unwrap();
            assert!(!r.spawns.contains_key(&9_000_200), "seen through");
            assert!(r.owed.contains_key(&("wf-caller".to_string(), "wf-helper".to_string())));
        }
        // One it gives up on: nobody has told the caller yet.
        agent_spawned(9_000_201, None, None, Some("the core stopped".into()));
        assert!(reg().lock().unwrap().spawns.contains_key(&9_000_201));
        forget("wf-helper");
        forget("wf-caller");
    }

    #[test]
    fn a_session_starting_for_a_spawn_given_up_on_is_refused_so_what_was_said_stays_true() {
        let _one = spawns_to_myself();
        register("gu-caller", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, serde_json::json!({}));
        take_up(Vec::new(), Vec::new(), vec![spawn_taken(9_000_400, "gu-caller", None, false), spawn_taken(9_000_401, "gu-caller", None, false)]);
        // Started as it is given up on: decided under one lock, whichever comes first.
        register("gu-first", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("gu-caller"), false, serde_json::json!({}));
        spawn_is(9_000_400, "gu-first").unwrap();
        settle_spawns(SESSION_RETURN_MS);
        assert!(!reg().lock().unwrap().spawns.contains_key(&9_000_400), "before: that session is what came of it");
        register("gu-late", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("gu-caller"), false, serde_json::json!({}));
        assert!(spawn_is(9_000_401, "gu-late").is_err(), "after: it does not start");
        assert!(!reg().lock().unwrap().known.contains_key("gu-late"), "and leaves nothing behind");
        assert!(reg().lock().unwrap().spawns[&9_000_401].helper.is_none());
        // A request nothing stands for any more is refused the same.
        register("gu-none", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("gu-caller"), false, serde_json::json!({}));
        assert!(spawn_is(1, "gu-none").is_err());
        forget("gu-first");
        forget("gu-caller");
    }

    #[test]
    fn a_retry_still_coming_is_not_saved_beside_its_own_request_and_a_late_failure_is_told() {
        let _one = spawns_to_myself();
        register("rc-caller", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, serde_json::json!({}));
        let word = SpawnTaken { news: Some("asked again".into()), settled: true, retried: true, ..spawn_taken(9_000_500, "rc-caller", None, true) };
        reg().lock().unwrap().spawns.insert(9_000_500, word);
        assert!(!chats_now().spawns.iter().any(|s| s.request == 9_000_500), "two on disk would start two");
        assert!(spawn_news(1000)["rc-caller"].iter().any(|(request, _)| *request == 9_000_500), "its word is still said");
        // A spawn the caller was told may yet come up, and the window says it did not.
        let timed_out = spawn_taken(9_000_501, "rc-caller", None, false);
        {
            let mut r = reg().lock().unwrap();
            r.abandoned.insert(9_000_501, ("rc-caller".into(), false));
            r.spawns.insert(9_000_501, timed_out);
        }
        agent_spawned(9_000_501, None, None, Some("the folder is gone".into()));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let news = loop {
            if let Some(news) = reg().lock().unwrap().spawns[&9_000_501].news.clone() {
                break news;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        assert!(news.contains("did not come up after all: the folder is gone") && news.contains("Its prompt is in "), "{news}");
        let file = news.split("Its prompt is in ").nth(1).and_then(|l| l.split("; spawn it again").next()).unwrap();
        let _ = std::fs::remove_file(file);
        forget("rc-caller");
    }

    #[test]
    fn a_helper_a_later_core_finds_done_is_neither_owed_again_nor_carried_on() {
        let _one = spawns_to_myself();
        register("fd-caller", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, serde_json::json!({}));
        // The same spawn as a third core finds it: under the second the helper went on,
        // finished, and its caller was told; the first left it no debt.
        let mut done = cut_short_chat("fd-done", "fd-caller", true, false, false);
        done.turns_ended = 2;
        done.ties.owed.clear();
        let placed = SpawnTaken { owed_placed: true, ..spawn_taken(9_000_600, "fd-caller", Some("fd-done"), true) };
        // Under the first core, with a turn ended before `owe_from`: that is its answer.
        let mut answered = cut_short_chat("fd-ans", "fd-caller", true, false, false);
        answered.turns_ended = 1;
        answered.ties.owed.clear();
        let first = spawn_taken(9_000_601, "fd-caller", Some("fd-ans"), true);
        take_up(vec![done, answered], Vec::new(), vec![placed, first]);
        take_up_spawns();
        {
            let r = reg().lock().unwrap();
            assert!(!r.owed.contains_key(&("fd-caller".to_string(), "fd-done".to_string())), "told already: no false did NOT finish");
            assert!(!r.resumes.contains_key("fd-done"), "and no carry on");
            let o = r.owed[&("fd-caller".to_string(), "fd-ans".to_string())];
            assert!(!o.cut_short, "its turn ended: it finished");
            assert!(!r.resumes.contains_key("fd-ans"));
            assert!(r.spawns[&9_000_601].owed_placed, "and not again under the next");
        }
        forget("fd-done");
        forget("fd-ans");
        forget("fd-caller");
    }

    #[test]
    fn text_going_in_is_found_in_what_the_session_was_told() {
        let m = |role: &str, text: &str| Message { role: role.into(), text: text.into() };
        let needle = needle_of("[tag] carry on\n\nThese messages:\n\nthe held   text", "the held\ntext");
        assert_eq!(needle, "the held text", "words, however they are laid out");
        let told = [m("assistant", "the held text"), m("user", "[tag] carry on\n\nthe held\n text and more")];
        assert_eq!(said_to(&told, &needle), WentIn::Yes);
        assert_eq!(said_to(&told[..1], &needle), WentIn::No, "said by the session itself is not told to it");
        assert_eq!(said_to(&[], &needle), WentIn::No);
        // A text long enough to go to a file is looked for as what was typed.
        let spilled = "This message is 9000 characters long, so Luna put it in a file: x.md.";
        assert_eq!(needle_of(spilled, &"long ".repeat(2000)), spilled);
        assert_eq!(needle_of(&"a ".repeat(500), &"a ".repeat(500)).chars().count(), NEEDLE);
    }

    #[test]
    fn a_held_text_on_its_way_in_comes_back_marked_and_only_it_is_marked() {
        let held = |caller: &str, text: &str, since| Held { caller: caller.into(), id: "gi-chat".into(), text: text.into(), since, told: false, going_in: None };
        reg().lock().unwrap().held.extend([held("gi-a", "first", 1), held("gi-b", "second", 2)]);
        mark_going_in("gi-chat", "first", 77, Some(("gi-a", 1)));
        let marks = || chats_now().sends.into_iter().filter(|s| s.id == "gi-chat").map(|s| s.going_in).collect::<Vec<_>>();
        assert_eq!(marks(), [Some(GoingIn { needle: "first".into(), cursor: 77 }), None]);
        mark_going_in("gi-chat", "carry on with first and second", 80, None);
        assert!(marks().iter().all(|m| m.as_ref().is_some_and(|g| g.cursor == 80)), "a resume's batch");
        // As a file written so reads back under the next core.
        let saved: SavedSend = serde_json::from_str(&serde_json::to_string(&chats_now().sends.into_iter().find(|s| s.id == "gi-chat").unwrap()).unwrap()).unwrap();
        assert_eq!(saved.going_in.as_ref().map(|g| g.needle.as_str()), Some("first"));
        unmark_going_in("gi-chat");
        assert_eq!(marks(), [None, None]);
        let old: SavedSend = serde_json::from_str(r#"{"caller":"x","id":"y","text":"z"}"#).unwrap();
        assert_eq!(old.going_in, None, "a file an older core left");
        reg().lock().unwrap().held.retain(|h| h.id != "gi-chat");
    }

    #[test]
    fn a_text_that_went_in_as_the_core_went_owes_a_turn_cut_short() {
        register("cs-chat", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("cs-caller"), false, serde_json::json!({}));
        crate::activity::keep_count("cs-chat", 3);
        // Back under this core, with a track to count on.
        crate::activity::on_hook("cs-chat", crate::activity::HookEvent::Gone);
        owe_cut_short("cs-caller", "cs-chat", 55, true);
        let o = reg().lock().unwrap().owed[&("cs-caller".to_string(), "cs-chat".to_string())];
        assert!(o.cut_short && o.late && o.unsure && !o.resumed && o.baseline == 3 && o.cursor == 55);
        assert_eq!(crate::activity::turns_counted("cs-chat"), Some(4), "so its end reads as one");
        let (line, how) = notice_for("cs-caller", "cs-chat", 4);
        let line = line.unwrap();
        assert!(matches!(how, Settled::Answered));
        assert!(line.contains("did NOT finish") && line.contains("cannot tell whether your last message reached it"), "{line}");
        forget("cs-chat");
        crate::activity::forget("cs-chat");
    }

    #[test]
    fn a_helper_told_it_was_not_done_is_heard_of_past_a_core_that_took_its_job() {
        crate::background::idle_for_test("tp-helper", Provider::Codex, 3, false);
        // As the last core saved it.
        let ties = Ties {
            parent: Some("tp-caller".into()),
            settings: serde_json::json!({ "background": true }),
            owed: vec![("tp-caller".into(), 3, 40)],
            told_pending: vec!["tp-caller".into()],
            ..Default::default()
        };
        let ties: Ties = serde_json::from_slice(&serde_json::to_vec(&ties).unwrap()).unwrap();
        tie_back("tp-helper", Provider::Codex, "C:\\p", "C:\\acc\\openai\\two", ties);
        let key = ("tp-caller".to_string(), "tp-helper".to_string());
        let o = reg().lock().unwrap().owed[&key];
        assert!(o.told_pending && o.job_went);
        // Not a turn in sight and nothing running: its job's end is all there will be.
        assert_eq!(verdict(&o, 3, Some(crate::activity::Turn::Idle), false, false), Verdict::JobOver);
        let (k, pty) = (reg().lock().unwrap().known["tp-helper"].clone(), crate::pty::manager());
        let held = Owed { held: Some((3, now_ms() - BACKGROUND_GRACE_MS)), ..o };
        let (line, how) = notice_line(&pty, "tp-caller", "Your helper", "tp-helper", &k, held).unwrap();
        let line = line.unwrap();
        assert!(matches!(how, Settled::Answered));
        assert!(line.contains("has nothing left running now that Luna's core has restarted"), "{line}");
        // And it comes back marked again from what this core saves.
        assert_eq!(ties_of("tp-helper").unwrap().told_pending, vec!["tp-caller".to_string()]);
        crate::background::gone_for_test("tp-helper");
        forget("tp-helper");
    }

    #[test]
    fn a_spawn_with_the_flag_is_started_afresh_and_says_so_or_why_not() {
        let _one = spawns_to_myself();
        // No such caller to spawn for: as a spawn would fail, and the caller told.
        let s = SpawnTaken { left_by_core: true, ..spawn_taken(9_000_300, "af-nobody", None, true) };
        spawn_afresh(s);
        let r = reg().lock().unwrap();
        let news = r.spawns[&9_000_300].news.clone().unwrap();
        assert!(news.contains("never completed") && news.contains("started afresh, it failed too"), "{news}");
        drop(r);
        reg().lock().unwrap().spawns.remove(&9_000_300);
    }

    #[test]
    fn keys_read_as_presses() {
        assert_eq!(
            parse_keys("2{enter}").unwrap(),
            [Key::Text("2".into()), Key::Seq("\r")]
        );
        assert_eq!(parse_keys("{Down}{down}{ENTER}").unwrap(), [Key::Seq("\x1b[B"), Key::Seq("\x1b[B"), Key::Seq("\r")]);
        assert!(parse_keys("{f13}").is_err());
        assert!(parse_keys("{enter").is_err());
    }

    #[test]
    fn an_answer_picks_by_number_label_or_words() {
        let q = AskQuestion { text: "Which?".into(), options: vec!["Red".into(), "Blue".into()], multi: false, list_at: None };
        assert_eq!(resolve_option(&q, "2"), "Blue");
        assert_eq!(resolve_option(&q, "red"), "Red");
        assert_eq!(resolve_option(&q, "9"), "9", "out of range is words");
        assert_eq!(resolve_option(&q, " green "), "green");
        let multi = AskQuestion { multi: true, ..q.clone() };
        assert_eq!(resolve_option(&multi, "1, 2"), "Red, Blue");

        let ask = Ask::question(vec![q], true);
        let keys = presses_for(Provider::Claude, &ask, &["2".into()], None, None).unwrap();
        assert_eq!(keys, [Key::Seq("\x1b[B"), Key::Seq("\r")]);
        let own = presses_for(Provider::Claude, &ask, &["green".into()], None, None).unwrap();
        assert_eq!(own, [Key::Seq("\x1b[B"), Key::Seq("\x1b[B"), Key::Text("green".into()), Key::Seq("\r")]);
        assert!(presses_for(Provider::Claude, &ask, &[], None, None).is_err());
        let perm = Ask::permission(Some("Bash".into()), Some("ls".into()));
        assert_eq!(presses_for(Provider::Codex, &perm, &[], Some(true), None).unwrap(), [Key::Text("y".into())]);
        assert_eq!(
            presses_for(Provider::Claude, &perm, &[], Some(false), Some("use dir")).unwrap(),
            [Key::Seq("\x1b"), Key::Text("use dir".into()), Key::Seq("\r")]
        );
        assert_eq!(perm.line(), "asks for permission to run Bash: ls");
        let codex = "  Would you like to run the following command?\n\n  $ Set-Content -LiteralPath made.txt -Value hi\n\n› 1. Yes, proceed (y)";
        assert_eq!(codex_command_on(codex).as_deref(), Some("Set-Content -LiteralPath made.txt -Value hi"));
    }

    #[test]
    fn a_codex_list_on_screen_is_a_question_answered_by_its_own_options() {
        let screen = "\n  Hooks need review\n  5 hooks are new or changed.\n  Hooks can run outside the sandbox after you trust them.\n\n\n\
                      › 1. Review hooks\n  2. Trust all and continue\n  3. Continue without trusting (hooks won't run)\n\n  enter confirm · esc skip\n";
        let ask = codex_list_on(screen).unwrap();
        let q = &ask.questions[0];
        assert_eq!(q.text, "Hooks need review 5 hooks are new or changed. Hooks can run outside the sandbox after you trust them.");
        assert_eq!(q.options, ["Review hooks", "Trust all and continue", "Continue without trusting (hooks won't run)"]);
        assert!(ask.blocking && ask.kind == "question");
        let keys = presses_for(Provider::Codex, &ask, &["3".into()], None, None).unwrap();
        assert_eq!(keys, [Key::Seq("\x1b[B"), Key::Seq("\x1b[B"), Key::Seq("\r")]);
        assert!(presses_for(Provider::Codex, &ask, &["trust them all".into()], None, None).is_err(), "no words of one's own");

        // From where the cursor is, not from the top.
        let moved = screen.replace("› 1.", "  1.").replace("  2. Trust", "› 2. Trust");
        let ask = codex_list_on(&moved).unwrap();
        let keys = presses_for(Provider::Codex, &ask, &["Review hooks".into()], None, None).unwrap();
        assert_eq!(keys, [Key::Seq("\x1b[A"), Key::Seq("\r")]);

        // A numbered list in the conversation, with the composer below it, is none.
        let chat = "• Steps:\n  1. Build\n  2. Test\n\n› Ask Codex to do anything\n\n  ? for shortcuts";
        assert!(codex_list_on(chat).is_none());
        let selecting = chat.replace("? for shortcuts", "ctrl+c copy · enter copy & follow · esc clear");
        assert!(codex_list_on(&selecting).is_none());
    }

    #[test]
    fn a_respawn_replaces_the_token() {
        let none = serde_json::json!({});
        let t1 = register("r", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone()).unwrap();
        let t2 = register("r", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none).unwrap();
        assert_ne!(t1, t2);
        assert_eq!(caller_of(&t1), None);
        assert_eq!(caller_of(&t2).as_deref(), Some("r"));
    }
}
