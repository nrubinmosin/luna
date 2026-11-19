//! What a helper is made of: what it was spawned with (`Spec`), what is seen of it
//! (`Observed`, `Context`, `Signs`, `LeftRunning`), a turn's run (`Run`), and the text
//! queued for it (`Queued`, `Held`).

use super::*;

/// What a helper is started with, every turn the same.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Spec {
    pub provider: Provider,
    pub account_path: String,
    /// The project folder, or the worktree it was handed to run in.
    pub folder: String,
    pub model: Option<String>,
    pub effort: String,
    /// Claude: the permission mode. Codex: the sandbox.
    pub mode: String,
    /// Codex: `--approve-for-me`, a reviewer model answering what it would ask.
    pub auto_review: bool,
    pub tools: bool,
    /// Claude: without the edit tools. Codex's sandbox says it there.
    #[serde(default)]
    pub read_only: bool,
    /// Claude: the agent it runs as (`--agent`).
    #[serde(default)]
    pub agent: Option<String>,
    /// The session it answers to, for what it is told of itself: who started it
    /// until that one is deleted or the person moves it (`set_parent`). Here
    /// and not looked up: `command_for` runs under this module's lock.
    #[serde(default)]
    pub parent: Option<String>,
    /// Keeps the MCP servers and connectors of its account, as spawned with
    /// `connectors`; else it has Luna's server alone.
    #[serde(default)]
    pub connectors: bool,
    /// What it and every session under it keep to, its caller's first
    /// (agents/spawn `Constraint`): set at the spawn and never changed, and
    /// given as the last of its role layers at every turn (`command_for`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<crate::agents::Constraint>,
    /// The bearer token of its MCP config: Luna's tools when `tools`, and
    /// `luna_permission` for every Claude Code helper. Never saved: a core
    /// started later issues its own.
    #[serde(skip)]
    pub token: Option<String>,
}

/// How full a session's context window is, as its pane's meter reads it: what
/// the window held as of its latest answer, against the model's window.
#[derive(Serialize, Debug, PartialEq)]
pub struct Context {
    /// `tokens` of `window`, rounded: 0–100.
    pub pct: u64,
    pub tokens: u64,
    pub window: u64,
}

impl Context {
    /// Off the tail of a transcript (Claude Code) or rollout (Codex). None
    /// until an answer has stated its usage, as before the first turn.
    pub fn of(path: &std::path::Path, provider: Provider) -> Option<Context> {
        let (tokens, window) = match provider {
            Provider::Claude => crate::claude::session::last_context(path)?,
            Provider::Codex => {
                let tail = crate::codex::session::read_tail(path);
                (tail.context_tokens?, tail.context_window?)
            }
        };
        (tokens > 0.0 && window > 0.0).then(|| Context {
            pct: ((tokens / window).min(1.0) * 100.0).round() as u64,
            tokens: tokens as u64,
            window: window as u64,
        })
    }
}

/// What a session's own events and files show beyond its state, as `luna_session_get`
/// gives it: each row carries one of these whatever runs the session.
#[derive(Serialize, Default, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Signs {
    /// When the turn on now began; null between turns.
    pub turn_started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<String>,
    /// The full model id the CLI used, whatever alias it was given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<Context>,
    /// Between turns, what its turns left running (`Leftover`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub left_running: Vec<LeftRunning>,
}

/// One process a helper's turns left running, as `luna_session_get` shows it.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LeftRunning {
    pub exe: String,
    pub pid: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    /// It used CPU or I/O within LEFTOVER_ACTIVE_MS. One holds the machine until
    /// idle for LEFTOVER_HOLD_MS on end.
    pub active: bool,
    /// Minutes since it last did (or since its turn ended, if it has not since).
    pub quiet_min: u64,
}

/// An instant as the rest of `luna_session_get` writes one.
pub fn iso(ms: u64) -> Option<String> {
    let t = jiff::Timestamp::from_second((ms / 1000) as i64).ok()?;
    Some(t.strftime("%Y-%m-%dT%H:%M:%SZ").to_string())
}

/// What a helper's turns have shown of it. All but the last event's moment
/// is saved with it: the model it ran on must outlive a core, and a moment
/// rewritten with every event would rewrite the file every SAVE_EVERY of every
/// turn.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct Observed {
    pub(super) model: Option<String>,
    /// When its last turn ended: the last it did, for a core started since.
    pub(super) turn_ended_ms: u64,
    /// When it last printed anything.
    #[serde(skip)]
    pub(super) event_ms: u64,
    /// Codex has said this turn runs on another model than it asked for.
    #[serde(skip)]
    pub(super) rerouted: bool,
    /// Which turn this is, for what is read of the last one once it is over.
    #[serde(skip)]
    pub(super) generation: u64,
    /// The model has written or run something this turn: a CLI that goes before
    /// that has started nothing of the model's (`turn_over`).
    #[serde(skip)]
    pub(super) acted: bool,
    /// A turn's model has answered, this one's or one before: until then the
    /// model it was asked for is no model it ran on, whatever its error says
    /// (a next turn's start clears that, a restore drops it).
    pub(super) answered: bool,
    /// Its CLI said on stderr that it stopped what the turn left in the
    /// background, its wait for it over (`take_stderr`): subagents' work may be
    /// unfinished and uncommitted, which its callers' notices and replies say.
    /// Saved: a core that goes before they are in must not lose it. Kept past
    /// later turns (a queued message starts one at once) until every caller
    /// it concerns (`stop_owed`) has had it (`background_stop_told`).
    pub(super) background_stopped: bool,
    /// The turn count that stop came at the end of: which stop it is, so an
    /// undo of a reply that told of one leaves a later one alone.
    pub(super) stopped_turn: u32,
    /// The callers owed an answer when it came (`stop_owed_to`), who are to
    /// hear of it: taken then, as the turn each is owed moves on with a
    /// message queued behind that turn, a retry or a resume.
    pub(super) stop_owed: Vec<String>,
    /// Those of them it has been said to.
    pub(super) stop_told: Vec<String>,
    /// What stopped that work, for what its callers are told.
    pub(super) stop_cause: StopCause,
    /// The subagents of that turn it took with it unfinished, one entry each:
    /// a Claude Code subagent's description (events.rs `unfinished_subagents`),
    /// a Codex one's name, "" for one with none (codex::session
    /// `subagents_cut_at_turn_end`). What its callers are told by name and
    /// count; empty where only the CLI's own word says it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) stop_subagents: Vec<String>,
    /// How the turn's process went, where that and not the CLI's own end
    /// stopped the work. Apart from `stop_cause`, which keeps to the values
    /// 0.19 reads: a value it does not know there fails its whole
    /// `helpers.json`, while a field it does not know it passes over.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) stop_by: Option<LostBy>,
}

/// What stopped a helper's background work before it was done. Saved; a
/// value a later Luna adds reads as `Ceiling`, which is last for that.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StopCause {
    /// `codex exec` interrupted the subagents its turn spawned as the turn
    /// ended, or went mid-turn with them (codex::session
    /// `subagents_cut_at_turn_end`).
    TurnEnd,
    /// Claude Code ended the turn's background work itself: its wait for it
    /// ran out (background/run.rs `stops_background`), or, with `stop_by`,
    /// its process went with it.
    #[default]
    #[serde(other)]
    Ceiling,
}

/// How a turn's process went with subagents unfinished, by no word of its
/// CLI's (`Observed::stop_by`). Saved; a value a later Luna adds reads as
/// `Exited`, which is last for that.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LostBy {
    /// Luna stopped it: a stop, an interrupt, Luna's own going
    /// (`Run::luna_stopped`).
    Stopped,
    /// Killed by something else, or gone on an error.
    #[serde(other)]
    Exited,
}

impl Observed {
    /// A turn's process is up.
    pub(super) fn begin_turn(&mut self) {
        self.generation += 1;
        self.rerouted = false;
        self.acted = false;
    }

    /// What the rollout said of a Codex turn once it was over, unless another
    /// has begun since: its model.
    pub(super) fn apply_rollout(&mut self, generation: u64, model: Option<String>) {
        if self.generation == generation && model.is_some() {
            self.model = model;
        }
    }
}

pub(super) struct Run {
    pub(super) pid: Option<u32>,
    /// When the turn started, for the window's "working 4 min".
    pub(super) started_ms: u64,
    /// A stop came while the turn was claimed but had no process yet: it
    /// has no pid to kill, so `spawn_turn` does not start it, or kills what
    /// it just started.
    pub(super) stopped: bool,
    /// Claude Code's stdin, while it still takes text (`feed_stdin`).
    pub(super) feed: Option<std::sync::mpsc::Sender<String>>,
    /// The uuid of the line this process was started on.
    pub(super) line: Option<String>,
    /// A `result` of this process's said it failed: a later one that succeeds
    /// clears what that one left in `Entry::error`.
    pub(super) result_failed: bool,
    /// Its CLI has written the turn's `result` and the main loop has said
    /// nothing since: the process is up only for what the turn left in the
    /// background (`waits_on_background`).
    pub(super) after_result: bool,
    /// How many background tasks (subagents, shells, monitors) its CLI says
    /// it has running, from its last `background_tasks_changed`.
    pub(super) background_tasks: usize,
    /// The background subagents its CLI has said it started, and whether each
    /// is over (events.rs `track_subagent`): those not over when the process
    /// goes ended with it, their work undone.
    pub(super) subagents: Vec<Subagent>,
    /// The turn's `result` is the last its main thread wrote: a task stopped
    /// now is the CLI's doing on its way out, not the model's.
    pub(super) model_done: bool,
    /// Luna stopped this process (a stop, an interrupt, its own going): what
    /// went with it is no doing of its CLI's.
    pub(super) luna_stopped: bool,
    /// Since when a lead's stdin has been kept open past a `result` for the
    /// notices its helpers owe it, while its own background tasks run
    /// (events.rs `keeps_listening`); None once it is closed as ever.
    pub(super) listening_since: Option<u64>,
    /// The processes in its job, as sampled while it waits so (rows.rs
    /// `activity`): what tells a subagent's long command at work from a
    /// watcher that does nothing, where its stream says nothing of either.
    pub(super) watched: Vec<Leftover>,
    /// Which claim this is, of all this core has made: a batch reserved on it
    /// (`Reservation`) tells the claim it holds from one made after it.
    pub(super) claim: u64,
    /// The person's move to another chat that this turn's prompt told it of
    /// (agents/reparent.rs `move_note`): told for good once its model has
    /// answered that prompt (`turn_over`), and only if it is still the last move.
    pub(super) moved: Option<crate::agents::Moved>,
    /// Claimed with no process yet, its command made (run.rs `spawn_turn`):
    /// from here it starts as made, and a move of the person's waits for its
    /// process rather than change its parent under it (control.rs `claim_move`).
    pub(super) starting: bool,
    /// A move of the person's held the helper after this run was claimed and
    /// before its command was made: what it was to start on was reserved, and
    /// its command text judged, against the parent of before, so it does not
    /// start; the batch goes back to the queue for the turn after the move.
    pub(super) overtaken: bool,
}

/// A background subagent (a `local_agent` task) of a Claude Code turn's process.
pub(super) struct Subagent {
    pub(super) task_id: String,
    /// Which registration of it this is: a subagent resumed in the same
    /// process keeps its `task_id` and gets a new `run_id` (2.1.325).
    pub(super) run_id: Option<String>,
    /// What the model called it as it started it: what its callers read.
    pub(super) description: String,
    /// Its CLI said it completed or failed, or the model stopped it.
    pub(super) done: bool,
    /// Its CLI stopped it on its way out: never undone by a later event of
    /// the same run, whatever order they come in.
    pub(super) cut: bool,
}

/// Text moved from a helper's queue onto the stdin of its running process
/// (events.rs `feed_queued`): whose answer that process owes now
/// (agents/owed.rs `moved_onto_stdin`).
pub(super) struct Drained {
    /// Its `Queued::id`.
    pub(super) id: String,
    /// Its sender.
    pub(super) from: Option<String>,
    /// The helper's turn count while that process ran: its end goes past it.
    pub(super) turns_ended: u32,
}

impl Run {
    pub(super) fn claimed(pid: Option<u32>, started_ms: u64) -> Run {
        static CLAIMS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let claim = CLAIMS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Run {
            pid,
            started_ms,
            stopped: false,
            feed: None,
            line: None,
            result_failed: false,
            after_result: false,
            background_tasks: 0,
            subagents: Vec::new(),
            model_done: false,
            luna_stopped: false,
            listening_since: None,
            watched: Vec::new(),
            claim,
            moved: None,
            starting: false,
            overtaken: false,
        }
    }
}

/// A fresh id in the shape Claude Code's stream-json wants of a message's uuid.
/// Unique is all it has to be; no crate here makes them.
pub(super) fn new_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let word = |n: u64| {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(n);
        h.finish()
    };
    let (a, b) = (word(SEQ.fetch_add(1, Ordering::Relaxed)), word(crate::throttle::now_ms()));
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        a >> 32,
        (a >> 16) & 0xffff,
        a & 0xfff,
        0x8000 | ((b >> 48) & 0x3fff),
        b & 0xffff_ffff_ffff
    )
}

/// Text sent to a helper, and by whom: what never reaches it goes back only to
/// whoever sent it, sessions being free to keep things from each other.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct Queued {
    /// None: no session's (a notice of Luna's, an opening prompt), its parent's
    /// to hear of.
    #[serde(default)]
    pub from: Option<String>,
    pub text: String,
    /// The budget lines in a notice, which go with it wherever it goes and are
    /// saved with it: nothing else says a line is on its way, so a core going at
    /// any moment can neither lose one nor have it told twice.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub budget: Vec<BudgetMark>,
    /// Its own id, the same on every try.
    #[serde(default = "new_id")]
    pub id: String,
    /// The uuid of the stdin line that carries it: its own, or the first's of the
    /// batch it went in. What the CLI's word and its transcript go by (`took`,
    /// `settle_lines`). None once nothing can come of that line any more.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
    /// A core went with it on stdin and nothing can tell whether the model got
    /// it: its sender decides, and it is not sent again by itself.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub maybe_reached: bool,
}

impl Queued {
    pub fn new(from: Option<String>, text: String, budget: Vec<BudgetMark>) -> Queued {
        Queued { from, text, budget, id: new_id(), line: None, maybe_reached: false }
    }
}

/// A budget line in a notice: whose it is, and its words, to take back out of a
/// notice that did not reach its parent.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct BudgetMark {
    pub id: String,
    pub line: String,
}

#[cfg(test)]
impl From<&str> for Queued {
    fn from(text: &str) -> Queued {
        Queued::new(None, text.to_string(), Vec::new())
    }
}

#[cfg(test)]
impl PartialEq<&str> for Queued {
    fn eq(&self, other: &&str) -> bool {
        self.text == *other
    }
}

/// A permission or question held for an answer.
pub(super) struct Held {
    pub(super) serial: u64,
    pub(super) ask: Ask,
    /// The tool input as the CLI sent it, returned with the answer.
    pub(super) input: Value,
    /// The call's id, by which its stream-json message says who made it.
    pub(super) tool_use_id: Option<String>,
    pub(super) answer: Option<Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_read_of_a_turn_gone_by_does_not_touch_the_next() {
        let mut o = Observed::default();
        o.begin_turn();
        let generation = o.generation;
        o.begin_turn();
        o.model = Some("rerouted-to".into());
        o.rerouted = true;
        o.apply_rollout(generation, Some("asked-for".into()));
        assert_eq!(o.model.as_deref(), Some("rerouted-to"));
        assert!(o.rerouted, "its own lookup is its own to skip");
    }

    /// How full the context is, off a transcript and a rollout as Claude Code
    /// 2.1.3xx and Codex 0.166.0 write them, cut to the fields read here.
    #[test]
    fn context_is_what_the_last_answer_left_in_the_window() {
        let dir = std::env::temp_dir().join(format!("luna-context-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let claude = dir.join("transcript.jsonl");
        let prompt = r#"{"type":"user","message":{"role":"user","content":"hi"}}"#;
        std::fs::write(&claude, format!("{prompt}\n")).unwrap();
        assert_eq!(Context::of(&claude, Provider::Claude), None, "no answer yet");
        // Input side only, cache included; a subagent's runs in a window of its own.
        let answer = r#"{"type":"assistant","message":{"model":"claude-sonnet-4-5","usage":{"input_tokens":10,"cache_creation_input_tokens":20000,"cache_read_input_tokens":29990,"output_tokens":300},"content":[]}}"#;
        let subagent = r#"{"type":"assistant","isSidechain":true,"message":{"model":"claude-sonnet-4-5","usage":{"input_tokens":150000},"content":[]}}"#;
        std::fs::write(&claude, [prompt, answer, subagent].join("\n") + "\n").unwrap();
        let context = Context::of(&claude, Provider::Claude);
        assert_eq!(context, Some(Context { pct: 25, tokens: 50_000, window: 200_000 }));
        assert_eq!(serde_json::to_value(&context).unwrap(), json!({ "pct": 25, "tokens": 50000, "window": 200000 }));

        let codex = dir.join("rollout-2026-11-05T10-00-00-abc.jsonl");
        let started = concat!(
            r#"{"timestamp":"t","type":"session_meta","payload":{"id":"abc","cwd":"/p"}}"#,
            "\n",
            r#"{"timestamp":"t","type":"turn_context","payload":{"model":"gpt-5-codex"}}"#,
            "\n",
            r#"{"timestamp":"t","type":"event_msg","payload":{"type":"token_count","info":null,"rate_limits":{"primary":null}}}"#,
            "\n",
        );
        std::fs::write(&codex, started).unwrap();
        assert_eq!(Context::of(&codex, Provider::Codex), None, "a count of rate limits alone: no turn yet");
        // Reasoning is not kept in the window, as Codex itself counts it.
        let count = r#"{"timestamp":"t","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":90000},"last_token_usage":{"input_tokens":64000,"output_tokens":700,"reasoning_output_tokens":100,"total_tokens":64700},"model_context_window":258400}}}"#;
        std::fs::write(&codex, format!("{started}{count}\n")).unwrap();
        assert_eq!(Context::of(&codex, Provider::Codex), Some(Context { pct: 25, tokens: 64_600, window: 258_400 }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_model_comes_back_with_the_helper_and_the_last_event_only_as_the_turn_end() {
        let o = Observed { model: Some("m".into()), turn_ended_ms: 5, event_ms: 9, ..Default::default() };
        let json = serde_json::to_value(&o).unwrap();
        assert!(json.get("eventMs").is_none(), "not written with every event");
        let back: Observed = serde_json::from_value(json).unwrap();
        assert_eq!((back.model.as_deref(), back.turn_ended_ms, back.event_ms), (Some("m"), 5, 0));
        // helpers.json from before these fields reads as nothing known.
        let bare: Observed = serde_json::from_value(json!({})).unwrap();
        assert_eq!((bare.model, bare.turn_ended_ms), (None, 0));
    }

    #[test]
    fn a_helper_saved_before_these_options_comes_back_without_them() {
        let spec: Spec = serde_json::from_value(json!({
            "provider": "claude", "accountPath": "C:\\acc", "folder": "C:\\p", "worktree": false, "model": null,
            "effort": "low", "mode": "default", "autoReview": false, "tools": false, "hooksUnasked": false,
            // Gone since, and no bar to what it is said beside.
            "parentMoved": true,
        }))
        .expect("helpers.json from before them still reads");
        assert!(!spec.read_only && spec.agent.is_none() && spec.parent.is_none());
    }

    /// A helper 0.19 saved with a worktree of its own comes back: what Luna kept
    /// of the worktree is let go, the helper runs on.
    #[test]
    fn a_helper_saved_with_a_worktree_of_its_own_still_reads() {
        let spec: Spec = serde_json::from_value(json!({
            "provider": "claude", "accountPath": "C:\\acc", "folder": "C:\\p", "worktree": true, "worktreeName": "luna-x",
            "model": null, "effort": "low", "mode": "default", "autoReview": false, "tools": false, "hooksUnasked": false,
        }))
        .expect("a helpers.json of 0.19 still reads");
        assert_eq!(spec.folder, "C:\\p");
    }

    #[test]
    fn ids_are_uuids_and_differ() {
        let (a, b) = (new_id(), new_id());
        assert_ne!(a, b);
        let parts: Vec<&str> = a.split('-').collect();
        assert_eq!(parts.iter().map(|p| p.len()).collect::<Vec<_>>(), [8, 4, 4, 4, 12], "{a}");
        assert!(parts[2].starts_with('4') && "89ab".contains(&parts[3][..1]), "{a}");
        assert!(a.chars().all(|c| c == '-' || c.is_ascii_hexdigit()));
    }
}
