//! Whether each session is working, waiting for a person, or done — the one
//! answer keep-awake and shutdown-when-done both hang on.
//!
//! Three signals per session, any of which makes it busy:
//!
//! - the turn: Claude Code says so through its hooks (see hub.rs) and its
//!   registry file, Codex through the rollout; a permission prompt counts as
//!   *waiting*, which is neither working nor done — unless what it waits
//!   on is the dialog an API error left (a usage limit), which is *stalled*:
//!   the turn is over and nothing more happens by itself, so it counts as done;
//! - the children: the processes the CLI has under it (procs.rs) — moving
//!   ones, and ones born of the current turn even if they only sleep;
//! - the screen: pty output in the last half minute.
//!
//! The sampler runs every few seconds on its own thread and hands the summary
//! to power.rs. The UI's own 4s poll (useSessionWatch) keeps drawing the
//! chat rows; this tracker is what the machine's power state follows.

use crate::procs::{self, ProcRead};
use crate::provider::Provider;
use crate::throttle::now_ms;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

pub const SAMPLE_EVERY: std::time::Duration = std::time::Duration::from_secs(5);
/// Output this recent still counts as work: a TUI mid-turn repaints
/// continuously, and a shell command streaming its log is work by any measure.
const OUTPUT_FRESH_MS: u64 = 30_000;
/// CPU and I/O growth per sample below this is a spinner or a poll, not a job:
/// the rates a leftover is at work by (`procs::leftovers`), over a sample.
const MIN_CPU_MS: u64 = procs::LEFTOVER_CPU_MS_PER_S * SAMPLE_EVERY.as_secs();
const MIN_IO_BYTES: u64 = procs::LEFTOVER_IO_PER_S * SAMPLE_EVERY.as_secs();
/// A hook said busy but the registry has said idle for this long: the Stop
/// hook was missed (a crash, a CLI too old for that event), the registry wins.
const STALE_HOOK_MS: u64 = 15_000;
/// A hook said waiting (a permission or a question) but the registry has said
/// idle for this long: the prompt was dismissed with the turn (Esc), and no
/// Stop followed. A live prompt sits inside a turn the registry calls busy, so
/// an idle registry this old means nobody is being asked anything.
const STALE_WAIT_MS: u64 = 60_000;
/// The hook said the turn ended, and since then the registry has kept saying
/// busy for this long with no hook at all: a turn that starts or does anything
/// fires a hook, so the registry has simply been left behind.
const STALE_DISK_MS: u64 = 60_000;
/// What a CLI starts this soon after it is spawned — its MCP servers, its
/// shell, the helpers it brings up with itself — is furniture, however early
/// its first turn: a chat spawned with its prompt is busy from its first
/// sample, and those servers would otherwise read as the turn's work and
/// hold the machine for as long as the chat is open.
const STARTUP_MS: u64 = 30_000;
/// A process born up to this long before the sampled turn start still belongs
/// to the turn: without hooks the start is only seen at the next sample.
const TURN_SLACK_MS: u64 = SAMPLE_EVERY.as_millis() as u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Turn {
    Busy,
    Waiting,
    Idle,
}

/// What a Claude Code hook told us.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HookEvent {
    /// `UserPromptSubmit`, `PostToolUse`: the model is at work.
    Busy,
    /// `Notification` with `permission_prompt` or `elicitation_dialog`.
    Waiting,
    /// `Stop`: the turn is over.
    Idle,
    /// `StopFailure`: the turn is over because the API refused it — a usage
    /// limit, a lost login. No `Stop` follows.
    Failed(&'static str),
    /// `SessionEnd`.
    Gone,
    /// A subagent of the session (the hook carries its `agent_id`) has
    /// started or used a tool: its work, not the turn's. A background agent
    /// works on long after the turn that started it is over.
    AgentAtWork(String),
    /// A subagent's own permission prompt or question is up.
    AgentWaiting(String),
    /// `SubagentStop`.
    AgentDone(String),
}

/// Claude Code's word for why a turn failed, as a static string. Anything it
/// adds later reads as `unknown` until it is listed here.
pub fn api_error_kind(s: &str) -> &'static str {
    const KINDS: [&str; 13] = [
        "authentication_failed", "oauth_org_not_allowed", "account_on_hold", "verification_required",
        "billing_error", "rate_limit", "overloaded", "invalid_request", "model_not_found", "server_error",
        "max_output_tokens", "cloud_credential_error", "unknown",
    ];
    KINDS.iter().copied().find(|k| *k == s).unwrap_or("unknown")
}

impl HookEvent {
    pub fn parse(body: &str) -> Option<HookEvent> {
        let v: serde_json::Value = serde_json::from_str(body).ok()?;
        let asks = |v: &serde_json::Value| match v["hook_event_name"].as_str() {
            Some("PermissionRequest") => true,
            Some("PreToolUse") => v["tool_name"] == "AskUserQuestion",
            Some("Notification") => matches!(v["notification_type"].as_str(), Some("permission_prompt" | "elicitation_dialog")),
            _ => false,
        };
        // Only a subagent's hooks carry its id, and none of them is the turn's:
        // the turn that started a background agent may long be over.
        if let Some(agent) = v["agent_id"].as_str().filter(|a| !a.is_empty()) {
            let agent = agent.to_string();
            return match v["hook_event_name"].as_str()? {
                "SubagentStop" => Some(HookEvent::AgentDone(agent)),
                _ if asks(&v) => Some(HookEvent::AgentWaiting(agent)),
                "Notification" | "PreToolUse" => None,
                _ => Some(HookEvent::AgentAtWork(agent)),
            };
        }
        match v["hook_event_name"].as_str()? {
            "UserPromptSubmit" | "PostToolUse" | "PostToolUseFailure" => Some(HookEvent::Busy),
            "Stop" => Some(HookEvent::Idle),
            // A permission prompt is up, or a question the model put.
            "PermissionRequest" => Some(HookEvent::Waiting),
            "PreToolUse" if v["tool_name"] == "AskUserQuestion" => Some(HookEvent::Waiting),
            "StopFailure" => Some(HookEvent::Failed(api_error_kind(v["error"].as_str().unwrap_or("")))),
            "SessionEnd" => Some(HookEvent::Gone),
            "Notification" => match v["notification_type"].as_str().unwrap_or("") {
                "permission_prompt" | "elicitation_dialog" => Some(HookEvent::Waiting),
                // `idle_prompt` (waiting for the next prompt), `auth_success`:
                // nothing to act on, but a well-formed event.
                _ => None,
            },
            // PreToolUse and the rest: the turn is already known to be busy.
            _ => None,
        }
    }
}

/// What the sampler keeps per live session between samples.
#[derive(Default)]
struct Track {
    /// The hook's word, once any hook has spoken for this session.
    hook: Option<Turn>,
    hook_at_ms: u64,
    /// When the registry / rollout last reported idle, for aging a stale hook.
    disk_idle_since_ms: Option<u64>,
    /// When the registry / rollout started saying busy, for aging it against
    /// a hook that has since said the turn is over.
    disk_busy_since_ms: Option<u64>,
    /// The two words as last read, for the log.
    disk: Option<Turn>,
    /// The combined verdict of the last sample.
    turn: Turn,
    /// When the first turn began (0 = never): children born after it are the
    /// model's own, whichever turn started them.
    first_turn_ms: u64,
    /// When the turn on now began, for `luna_list`: a hook's moment, or the
    /// sample that first saw it where no hook speaks.
    turn_started_ms: Option<u64>,
    prev: Vec<ProcRead>,
    procs: Option<procs::Busy>,
    /// What its turns started that is still there (`procs::Busy::young`), each
    /// with when it was last at work: what holds the machine of it, what
    /// `luna_list` names between turns, and what its caller hears has sat idle.
    left: Vec<procs::Leftover>,
    /// The last sample its turn was on (busy, or holding on a prompt): what a
    /// child born since is as quiet as, to begin with (`procs::leftovers`).
    turn_at_ms: u64,
    output_fresh: bool,
    /// Turns finished: what an agent's `wait` counts. From the CLI's own
    /// record where there is one — Claude Code's `Stop` hook, Codex's
    /// rollout — and from busy → not-busy transitions between samples only
    /// for a session with neither (a Claude Code that outlived a Luna
    /// restart). A turn shorter than the sampling period is invisible to
    /// the transitions, and a one-word answer is exactly that short.
    ///
    /// It only grows, across respawns and resumes too: an agent keeps the
    /// count it was last shown and waits for a turn past it, and a count
    /// that started over would leave it waiting out its whole timeout.
    turns_ended: u32,
    /// The process the count is taken for, by its spawn time, and
    /// `turns_ended` when it started: Codex's count of its own turns goes on
    /// from there (`count_on`).
    spawned_at_ms: u128,
    turns_before: u32,
    /// The last verdict `busy` was computed from, for the transition above.
    was_busy: bool,
    /// The API error the last turn ended on, per `StopFailure`; cleared by
    /// any other hook of the turn's own.
    failed: Option<&'static str>,
    /// Its subagents at work, by id, each with whether a prompt of its own is
    /// up: from `SubagentStart` (or its first tool) to `SubagentStop`, or until
    /// the registry has said idle a while, which it does only with none left.
    agents: HashMap<String, bool>,
}

/// What an agent's `wait`/`read` asks about a session.
#[derive(Clone, Copy)]
pub struct TurnState {
    pub turn: Turn,
    pub busy: bool,
    pub turns_ended: u32,
    /// Processes under the CLI are at work — moving, or started by a turn and
    /// still alive — or a subagent of its own is: a job it left running in
    /// the background.
    pub children_busy: bool,
}

/// Between turns, what a session's turns left running (`Track::left`): for
/// `luna_list`, and for its caller to hear when all of it has sat idle long.
pub fn left_running(chat_id: &str) -> Vec<procs::Leftover> {
    let st = state().lock().unwrap_or_else(|e| e.into_inner());
    st.tracks.get(chat_id).filter(|t| t.turn == Turn::Idle).map(|t| t.left.clone()).unwrap_or_default()
}

/// Whether anything of a session's children still holds the machine: what moved
/// at the last sample, or what its turns left that has not sat idle for
/// LEFTOVER_HOLD_MS, or a subagent at work. `background::left_holds` for a
/// session with a terminal.
pub fn children_hold(chat_id: &str) -> bool {
    let st = state().lock().unwrap_or_else(|e| e.into_inner());
    let now = now_ms();
    st.tracks.get(chat_id).is_some_and(|t| {
        !t.agents.is_empty()
            || t.procs.as_ref().is_some_and(|b| !b.active.is_empty() || (t.left.is_empty() && !b.young.is_empty()))
            || t.left.iter().any(|l| l.holds(now))
    })
}

/// `background::left_quiet` for a session the activity track follows. None
/// while a subagent of it works: no process tells whether it is stuck.
pub fn left_quiet(chat_id: &str) -> Option<(u64, String)> {
    if subagents(chat_id) > 0 {
        return None;
    }
    let left = left_running(chat_id);
    Some((procs::quiet_ms(&left, now_ms())?, procs::left_names(&left)))
}

pub fn turn_state(chat_id: &str) -> Option<TurnState> {
    // A helper with no terminal keeps its own count (background.rs).
    if let Some(t) = crate::background::turn_state(chat_id) {
        return Some(t);
    }
    let st = state().lock().unwrap_or_else(|e| e.into_inner());
    let t = st.tracks.get(chat_id)?;
    let busy = st.summary.sessions.iter().find(|s| s.id == chat_id).map(|s| s.busy).unwrap_or(false);
    Some(TurnState { turn: t.turn, busy, turns_ended: t.turns_ended, children_busy: t.procs.is_some() || !t.agents.is_empty() })
}

/// How many subagents of a session are at work, as its hooks have it.
pub fn subagents(chat_id: &str) -> usize {
    state().lock().unwrap_or_else(|e| e.into_inner()).tracks.get(chat_id).map_or(0, |t| t.agents.len())
}

/// The API error a session's last turn ended on, per its `StopFailure` hook.
pub fn failed_of(chat_id: &str) -> Option<&'static str> {
    state().lock().unwrap_or_else(|e| e.into_inner()).tracks.get(chat_id)?.failed
}

/// When a visible session's current turn began, None between turns.
pub fn turn_started_ms(chat_id: &str) -> Option<u64> {
    state().lock().unwrap_or_else(|e| e.into_inner()).tracks.get(chat_id)?.turn_started_ms
}

impl Default for Turn {
    fn default() -> Self {
        Turn::Idle
    }
}

/// One session's state, as the UI shows it in the power chip's tooltip.
#[derive(Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionActivity {
    pub id: String,
    pub turn: Turn,
    /// Children keeping it busy, by exe name, when any, and its subagents at
    /// work, by count.
    pub procs: Vec<String>,
    pub output_fresh: bool,
    pub busy: bool,
    /// Waiting, but on what an API error left on screen — a usage limit, a
    /// lost login — rather than on a question from its turn: the turn is
    /// over, and the session cannot go on by itself before someone acts or
    /// the limit resets hours later. Done, as far as the power-off goes.
    pub stalled: Option<&'static str>,
    /// Where the verdict came from, for the log: the hook's word, the
    /// registry's word, and how long ago the screen last moved.
    #[serde(skip)]
    pub hook: Option<Turn>,
    #[serde(skip)]
    pub disk: Option<Turn>,
    #[serde(skip)]
    pub output_age_ms: u64,
}

impl SessionActivity {
    /// A session known only by its turn: a background helper, whose turn is
    /// its process (background.rs).
    pub fn of_turn(id: &str, turn: Turn, busy: bool) -> SessionActivity {
        SessionActivity {
            id: id.to_string(),
            turn,
            procs: Vec::new(),
            output_fresh: false,
            busy,
            stalled: None,
            hook: None,
            disk: None,
            output_age_ms: 0,
        }
    }

    /// Why this session holds the machine, or "" when it does not.
    pub fn why(&self) -> String {
        let word = |t: Option<Turn>| match t {
            Some(Turn::Busy) => "busy",
            Some(Turn::Waiting) => "waiting",
            Some(Turn::Idle) => "idle",
            None => "-",
        };
        let mut parts = Vec::new();
        match self.turn {
            Turn::Busy => parts.push("mid-turn".to_string()),
            Turn::Waiting => match self.stalled {
                Some(e) => parts.push(format!("stopped on {e}")),
                None => parts.push("waiting for a person".to_string()),
            },
            Turn::Idle => {}
        }
        if !self.procs.is_empty() {
            parts.push(format!("running {}", self.procs.join(", ")));
        }
        if self.output_fresh {
            parts.push(format!("printed {}s ago", self.output_age_ms / 1000));
        }
        if parts.is_empty() {
            return String::new();
        }
        format!("{} [hook {}, registry {}]", parts.join(", "), word(self.hook), word(self.disk))
    }
}

/// The whole board at one sample.
#[derive(Clone, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub busy: usize,
    /// Waiting for a person — stalled sessions not counted.
    pub waiting: usize,
    /// Waiting only on what an API error left (see `SessionActivity::stalled`).
    pub stalled: usize,
    /// Since when nothing has been busy or waiting, if that is the case.
    pub idle_since_ms: Option<u64>,
    pub sessions: Vec<SessionActivity>,
}

struct State {
    tracks: HashMap<String, Track>,
    /// The turn count of each session whose track was dropped when it went
    /// away, for the track it gets when it comes back (see `turns_ended`).
    ended: HashMap<String, u32>,
    summary: Summary,
    idle_since_ms: Option<u64>,
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    let s = S.get_or_init(|| {
        Mutex::new(State { tracks: HashMap::new(), ended: HashMap::new(), summary: Summary::default(), idle_since_ms: None })
    });
    // Tests assert with this held and all of them share it: one that fails is
    // that test's failure, not that of every test after it.
    #[cfg(test)]
    s.clear_poison();
    s
}

/// A session's track, made on first sight with the turn count it had when it
/// last went away.
fn track<'a>(tracks: &'a mut HashMap<String, Track>, ended: &mut HashMap<String, u32>, id: &str) -> &'a mut Track {
    tracks
        .entry(id.to_string())
        .or_insert_with(|| Track { turns_ended: ended.remove(id).unwrap_or(0), ..Default::default() })
}

/// A session back between turns on the count it had, as the sampler has it once
/// the window has started it, for the tests of what its caller is told.
#[cfg(test)]
pub(crate) fn between_turns_for_test(chat_id: &str) {
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    let st = &mut *st;
    track(&mut st.tracks, &mut st.ended, chat_id);
}

/// Drops the tracks of sessions that are no longer live, keeping their counts.
fn drop_gone(tracks: &mut HashMap<String, Track>, ended: &mut HashMap<String, u32>, live: &[&str]) {
    tracks.retain(|id, t| {
        let keep = live.contains(&id.as_str());
        if !keep {
            ended.insert(id.clone(), t.turns_ended);
        }
        keep
    });
}

/// Counts a sample's turns for the process it was taken of: a new one (a
/// respawn, a resume) starts from the count so far, and `disk` — the turns
/// its own rollout records since it started, where it keeps one — adds to
/// that.
fn count_on(t: &mut Track, spawned_at_ms: u128, disk: Option<u32>) {
    if t.spawned_at_ms != spawned_at_ms {
        t.spawned_at_ms = spawned_at_ms;
        t.turns_before = t.turns_ended;
    }
    if let Some(n) = disk {
        t.turns_ended = t.turns_ended.max(t.turns_before + n);
    }
}

/// A session's turn count, running or gone.
pub fn turns_counted(chat_id: &str) -> Option<u32> {
    let st = state().lock().unwrap_or_else(|e| e.into_inner());
    st.tracks.get(chat_id).map(|t| t.turns_ended).or_else(|| st.ended.get(chat_id).copied())
}

/// The count a session had under the last core, for the track it gets when
/// the window starts it again: a caller's baseline goes by that count.
pub fn keep_count(chat_id: &str, turns_ended: u32) {
    state().lock().unwrap_or_else(|e| e.into_inner()).ended.insert(chat_id.to_string(), turns_ended);
}

/// The count is at least `turns_ended`: a turn that went with a core, found out
/// only once the session is back.
pub fn count_at_least(chat_id: &str, turns_ended: u32) {
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    match st.tracks.get_mut(chat_id) {
        Some(t) => t.turns_ended = t.turns_ended.max(turns_ended),
        None => {
            let kept = st.ended.entry(chat_id.to_string()).or_default();
            *kept = (*kept).max(turns_ended);
        }
    }
}

/// How many turns the process started at `spawned_at_ms` has ended, the count of
/// those before it aside; none before the sampler has first seen it.
pub fn turns_of_process(chat_id: &str, spawned_at_ms: u128) -> u32 {
    let st = state().lock().unwrap_or_else(|e| e.into_inner());
    st.tracks.get(chat_id).filter(|t| t.spawned_at_ms == spawned_at_ms).map_or(0, |t| t.turns_ended.saturating_sub(t.turns_before))
}

/// A chat was deleted: its track and the count kept for its return go.
pub fn forget(chat_id: &str) {
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    st.tracks.remove(chat_id);
    st.ended.remove(chat_id);
}

/// A hook arrived for a session (hub.rs). Takes effect at the next sample;
/// the timestamp is the truth for "when did the turn start".
pub fn on_hook(chat_id: &str, ev: HookEvent) {
    let now = now_ms();
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    let st = &mut *st;
    let t = track(&mut st.tracks, &mut st.ended, chat_id);
    // A subagent's word: the turn, its start and its end are not its to say.
    match ev {
        HookEvent::AgentAtWork(agent) => {
            t.agents.insert(agent, false);
            return;
        }
        HookEvent::AgentWaiting(agent) => {
            t.agents.insert(agent, true);
            return;
        }
        HookEvent::AgentDone(agent) => {
            // The registry may go on saying busy: the turn Claude Code starts
            // on the agent's result. Judged afresh from here (STALE_DISK_MS),
            // not by a busy that was the agent's. Not for an agent never
            // seen at work (one of the CLI's own): its busy was none of ours.
            if t.agents.remove(&agent).is_some() {
                t.disk_busy_since_ms = None;
            }
            return;
        }
        _ => {}
    }
    t.failed = None;
    match ev {
        HookEvent::Busy => {
            if t.first_turn_ms == 0 {
                t.first_turn_ms = now;
            }
            t.turn_started_ms.get_or_insert(now);
            t.hook = Some(Turn::Busy);
        }
        HookEvent::Waiting => {
            t.turn_started_ms.get_or_insert(now);
            t.hook = Some(Turn::Waiting);
        }
        HookEvent::Idle => {
            t.hook = Some(Turn::Idle);
            t.turn_started_ms = None;
            // `Stop`: a turn is over, whether or not any sample saw it run.
            t.turns_ended += 1;
            t.was_busy = false;
        }
        HookEvent::Failed(error) => {
            t.hook = Some(Turn::Idle);
            t.turn_started_ms = None;
            t.turns_ended += 1;
            t.was_busy = false;
            t.failed = Some(error);
        }
        HookEvent::Gone => {
            t.hook = Some(Turn::Idle);
            t.turn_started_ms = None;
            t.agents.clear();
        }
        HookEvent::AgentAtWork(_) | HookEvent::AgentWaiting(_) | HookEvent::AgentDone(_) => return,
    }
    t.hook_at_ms = now;
}

/// The hook's last word on a session, if any — for the listener's test.
#[cfg(test)]
pub fn hook_turn(chat_id: &str) -> Option<Turn> {
    state().lock().unwrap_or_else(|e| e.into_inner()).tracks.get(chat_id).and_then(|t| t.hook)
}

/// The last summary the sampler produced.
pub fn summary() -> Summary {
    state().lock().unwrap_or_else(|e| e.into_inner()).summary.clone()
}

/// Forget the idle stretch: after a sleep the machine has not been idle for
/// the time the clock says it has.
pub fn reset_idle() {
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    st.idle_since_ms = None;
    st.summary.idle_since_ms = None;
}

/// What the disk says about a session's turn right now, and when it last
/// changed, where the CLI says (Claude Code's registry).
fn disk_turn(p: &crate::pty::ActivityProbe) -> Option<(Turn, Option<u64>)> {
    let (raw, at) = match p.provider {
        Provider::Claude => crate::claude::session::status(p.pid, p.resume.as_deref(), &p.cwd, p.spawned_at_ms, &p.account_path)?,
        Provider::Codex => (
            crate::codex::session::status(&crate::codex::session::Live {
                chat_id: &p.id,
                cwd: &p.cwd,
                spawned_at_ms: p.spawned_at_ms,
                resume: p.resume.as_deref(),
                account_path: &p.account_path,
                last_output_ms: p.last_output_ms,
            })?,
            None,
        ),
    };
    // Codex keeps no record of a pending approval; its screen says so.
    if p.asking && turn_of(&raw) == Turn::Busy {
        return Some((Turn::Waiting, at));
    }
    Some((turn_of(&raw), at))
}

/// The API error a session's last turn ended on, per its transcript. Codex
/// ends such a turn like any other, so it never sits waiting on one.
fn disk_api_error(p: &crate::pty::ActivityProbe) -> Option<&'static str> {
    match p.provider {
        Provider::Claude => crate::claude::session::last_api_error(p.pid, p.resume.as_deref(), &p.cwd, p.spawned_at_ms, &p.account_path),
        Provider::Codex => None,
    }
}

/// Turns the CLI's own record says this process has finished, where it
/// keeps one.
fn disk_turns_ended(p: &crate::pty::ActivityProbe) -> Option<u32> {
    match p.provider {
        Provider::Claude => None,
        Provider::Codex => crate::codex::session::turns_completed(&crate::codex::session::Live {
            chat_id: &p.id,
            cwd: &p.cwd,
            spawned_at_ms: p.spawned_at_ms,
            resume: p.resume.as_deref(),
            account_path: &p.account_path,
            last_output_ms: p.last_output_ms,
        }),
    }
}

/// The CLIs' words for a turn, folded the way the chat rows fold them.
pub fn turn_of(raw: &str) -> Turn {
    match raw {
        "busy" | "working" => Turn::Busy,
        s if ["wait", "input", "block", "attention", "permission"].iter().any(|w| s.contains(w)) => Turn::Waiting,
        _ => Turn::Idle,
    }
}

/// One session's verdict from its hook word, the disk's word, and the clock.
fn combine(t: &mut Track, disk: Option<Turn>, now: u64) -> Turn {
    t.disk = disk;
    match disk {
        Some(Turn::Idle) => {
            t.disk_idle_since_ms.get_or_insert(now);
        }
        _ => t.disk_idle_since_ms = None,
    }
    match disk {
        Some(Turn::Busy) => {
            t.disk_busy_since_ms.get_or_insert(now);
        }
        _ => t.disk_busy_since_ms = None,
    }
    let idle_for = t.disk_idle_since_ms.map(|s| now.saturating_sub(s)).unwrap_or(0);
    // Claude Code calls itself busy while a subagent of it works, so an idle
    // registry that has stood a while says none is left: a SubagentStop that
    // never came must not keep the session at work for good.
    if idle_for > STALE_HOOK_MS {
        t.agents.clear();
    }
    let hook = match t.hook {
        // A hook that says busy against a registry that has said idle for a
        // while has missed its Stop.
        Some(Turn::Busy) if idle_for > STALE_HOOK_MS => {
            t.hook = Some(Turn::Idle);
            Some(Turn::Idle)
        }
        // A hook that says waiting against a registry that has said idle for
        // a while: the prompt went away with its turn, and no Stop followed.
        Some(Turn::Waiting) if idle_for > STALE_WAIT_MS => {
            t.hook = Some(Turn::Idle);
            Some(Turn::Idle)
        }
        h => h,
    };
    // The registry says busy, but the hook said the turn ended: a subagent
    // of it at work, or, with nothing spoken since, a registry left behind.
    // Not a turn either way.
    let disk = match (hook, disk) {
        (Some(Turn::Idle), Some(Turn::Busy)) if !t.agents.is_empty() => Some(Turn::Idle),
        (Some(Turn::Idle), Some(Turn::Busy)) => {
            let since = t.disk_busy_since_ms.unwrap_or(now).max(t.hook_at_ms);
            if now.saturating_sub(since) > STALE_DISK_MS {
                Some(Turn::Idle)
            } else {
                disk
            }
        }
        _ => disk,
    };
    match (hook, disk) {
        // A permission prompt sits inside a turn the registry calls busy, a
        // subagent's too, whether or not the turn that started it is over.
        (Some(Turn::Waiting), _) => Turn::Waiting,
        _ if t.agents.values().any(|asks| *asks) => Turn::Waiting,
        // The registry says a dialog is up, so the model is not working,
        // whatever the last hook said: a turn that died on an API error
        // before Claude Code fired `StopFailure` left its hook at busy.
        (Some(Turn::Busy), Some(Turn::Waiting)) => Turn::Waiting,
        (Some(Turn::Busy), _) | (_, Some(Turn::Busy)) => Turn::Busy,
        (None, Some(d)) => d,
        (Some(h), None) => h,
        (Some(Turn::Idle), Some(d)) => d,
        (None, None) => Turn::Idle,
    }
}

/// One session's children at a sample: what of them is at work, and what holds
/// the machine, by label, and what of its turns' leftovers has just let it go.
/// What came up with the session holds it while it moves (`judge`, sample to
/// sample); what its turns started (`young`) until it has sat idle for
/// LEFTOVER_HOLD_MS, by its rate over LEFTOVER_WINDOW_MS, as a helper's
/// leftovers do (`procs::leftovers`): a loop's one busy sample is no work, and
/// counted as moving it would start a power-off's quiet window over each time.
/// One with no name has gone since the read.
fn children_at(t: &mut Track, children: Vec<ProcRead>, born_after: u64, name_of: &dyn Fn(u32) -> Option<String>, now: u64) -> (Vec<String>, Vec<String>) {
    t.procs = procs::without_furniture(procs::judge(&t.prev, &children, born_after, MIN_CPU_MS, MIN_IO_BYTES));
    if let Some(b) = t.procs.as_mut() {
        let young = b.young.clone();
        b.active.retain(|p| !young.contains(p));
    }
    let young: Vec<(ProcRead, String)> = match &t.procs {
        Some(b) => children.iter().filter(|r| b.young.contains(&r.pid)).filter_map(|r| Some((*r, name_of(r.pid)?))).collect(),
        None => Vec::new(),
    };
    let left = procs::leftovers(&t.left, young, now, t.turn_at_ms);
    let gone_quiet = procs::gone_quiet(&t.left, &left, now);
    t.left = left;
    t.prev = children;
    let moving: Vec<u32> = t.procs.as_ref().map(|b| b.active.clone()).unwrap_or_default();
    let mut holding: Vec<String> = moving.iter().filter_map(|&pid| Some(format!("{} (pid {pid})", name_of(pid)?))).collect();
    holding.extend(t.left.iter().filter(|l| l.holds(now) && !moving.contains(&l.pid)).map(procs::Leftover::label));
    holding.sort();
    holding.dedup();
    (holding, gone_quiet)
}

/// Its subagents at work, for what holds the machine: they work in the CLI
/// itself, with no process of their own to show, a job all the same. Not one
/// on a prompt of its own, which waits for a person as a turn on one does.
fn agents_at_work(t: &Track) -> Option<String> {
    match t.agents.values().filter(|asks| !**asks).count() {
        0 => None,
        1 => Some("1 subagent".to_string()),
        n => Some(format!("{n} subagents")),
    }
}

/// One pass over every live session, plus the background helpers as they
/// stand (`helpers`), which keep no track here. Called by the sampler thread;
/// public so a test can drive it with probes of its own.
///
/// The registry, rollouts, transcripts and process table are read outside
/// the state lock: a hook is answered only once `on_hook` has had it, and
/// Claude Code holds its turn until the hook's curl returns, so reads under
/// the lock held up every busy session's next step for as long as they took.
pub fn sample(probes: Vec<crate::pty::ActivityProbe>, helpers: Vec<SessionActivity>) -> Summary {
    let reads: Vec<_> = probes
        .iter()
        .map(|p| (disk_turn(p), disk_turns_ended(p), procs::children(p.job.as_deref(), p.pid)))
        .collect();
    // Every child's name, from one snapshot of the process table.
    let named = procs::names(&reads.iter().flat_map(|(_, _, children)| children.iter().map(|r| r.pid)).collect::<Vec<_>>());
    let name_of = |pid: u32| named.iter().find(|(p, _)| *p == pid).map(|(_, n)| n.clone());
    let now = now_ms();

    let mut sessions = Vec::with_capacity(probes.len());
    // Per session, whether to look behind its wait for an API error: read once
    // the lock is let go. And whose leftovers have let the machine go, for the log.
    let mut later: Vec<bool> = Vec::with_capacity(probes.len());
    let mut let_go: Vec<(String, Vec<String>)> = Vec::new();
    {
        let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
        let st = &mut *st;

        let live: Vec<&str> = probes.iter().map(|p| p.id.as_str()).collect();
        drop_gone(&mut st.tracks, &mut st.ended, &live);

        for (p, (disk_read, disk_turns, children)) in probes.iter().zip(reads) {
            let t = track(&mut st.tracks, &mut st.ended, &p.id);
            // A respawn (new settings, a resume) is a new CLI with servers of
            // its own: what counts as its turns' work starts over with it.
            if t.spawned_at_ms != 0 && t.spawned_at_ms != p.spawned_at_ms {
                t.first_turn_ms = 0;
                t.prev.clear();
                t.left.clear();
                t.agents.clear();
            }

            let (disk, disk_at) = disk_read.map_or((None, None), |(d, at)| (Some(d), at));
            // A registry that changed after the hook spoke is the later word: a
            // prompt answered "No" ends the turn with no Stop, and the waiting
            // the hook said would otherwise stand for STALE_WAIT_MS.
            if disk == Some(Turn::Idle) && t.hook == Some(Turn::Waiting) && disk_at.is_some_and(|at| at > t.hook_at_ms) {
                t.hook = Some(Turn::Idle);
            }
            let turn = combine(t, disk, now);
            // Without a hook the start of a turn is seen at the sample after it
            // happened; a child born in between is still the turn's.
            if turn == Turn::Busy && t.first_turn_ms == 0 {
                t.first_turn_ms = now.saturating_sub(TURN_SLACK_MS);
            }
            t.turn = turn;
            if turn != Turn::Idle {
                t.turn_at_ms = now;
            }
            match turn {
                Turn::Idle => t.turn_started_ms = None,
                // A registry still busy after the hook's `Stop` has been left
                // behind: no turn starts on its word.
                _ if t.hook != Some(Turn::Idle) => {
                    t.turn_started_ms.get_or_insert(now);
                }
                _ => {}
            }

            let born_after = match t.first_turn_ms {
                0 => 0,
                first => first.max(p.spawned_at_ms as u64 + STARTUP_MS),
            };
            let (mut holding, gone_quiet) = children_at(t, children, born_after, &name_of, now);
            holding.extend(agents_at_work(t));
            if !gone_quiet.is_empty() {
                let_go.push((p.id.clone(), gone_quiet));
            }
            t.output_fresh = now.saturating_sub(p.last_output_ms) < OUTPUT_FRESH_MS;

            // A turn "ends" for an agent when the model stops, not when its
            // background jobs do: `wait` wants the reply, and the children are
            // the machine's concern (power.rs), not the caller's.
            let model_busy = t.turn == Turn::Busy;
            // Codex: the rollout's count is the truth.
            count_on(t, p.spawned_at_ms, disk_turns);
            // Claude Code with hooks: `Stop` counts (on_hook). Without any hook
            // heard, the transitions are all there is.
            if disk_turns.is_none() && t.hook.is_none() && t.was_busy && !model_busy {
                t.turns_ended += 1;
            }
            t.was_busy = model_busy;

            let busy = model_busy || !holding.is_empty() || t.output_fresh;
            // Only a waiting session is asked: the hook's word if one came, else
            // the transcript's last line.
            let waiting = t.turn == Turn::Waiting;
            later.push(waiting && t.failed.is_none());
            sessions.push(SessionActivity {
                id: p.id.clone(),
                turn: t.turn,
                procs: holding,
                output_fresh: t.output_fresh,
                busy,
                stalled: t.failed.filter(|_| waiting),
                hook: t.hook,
                disk: t.disk,
                output_age_ms: now.saturating_sub(p.last_output_ms),
            });
        }
    }

    for (id, gone) in let_go {
        crate::log::warn(
            "power",
            &format!("{}: no longer holds the PC by what its turns left, idle {} min on end: {}", crate::agents::label(&id), procs::LEFTOVER_HOLD_MS / 60_000, gone.join(", ")),
        );
    }
    for ((s, p), ask) in sessions.iter_mut().zip(&probes).zip(later) {
        if ask {
            s.stalled = disk_api_error(p);
        }
    }
    sessions.extend(helpers);
    let busy = sessions.iter().filter(|s| s.busy).count();
    let parked = |s: &&SessionActivity| !s.busy && s.turn == Turn::Waiting;
    let waiting = sessions.iter().filter(parked).filter(|s| s.stalled.is_none()).count();
    let stalled = sessions.iter().filter(parked).filter(|s| s.stalled.is_some()).count();
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    let st = &mut *st;
    if busy == 0 && waiting == 0 {
        st.idle_since_ms.get_or_insert(now);
    } else {
        st.idle_since_ms = None;
    }
    st.summary = Summary { busy, waiting, stalled, idle_since_ms: st.idle_since_ms, sessions };
    st.summary.clone()
}

/// The sampler thread: probes, samples, hands the board to power.rs.
pub fn start() {
    std::thread::Builder::new()
        .name("luna-activity".into())
        .spawn(move || {
            let mut last = std::time::Instant::now();
            loop {
                std::thread::sleep(SAMPLE_EVERY);
                let gap = last.elapsed();
                last = std::time::Instant::now();
                // A sample that took far longer than its period to come round
                // is the machine having been asleep, not the sessions idle.
                if gap > SAMPLE_EVERY * 6 {
                    crate::log::warn("activity", &format!("clock jumped {gap:?}; idle stretch reset"));
                    reset_idle();
                    crate::power::woke();
                }
                let pty = crate::pty::manager();
                let mut probes = pty.activity_probes();
                for p in probes.iter_mut().filter(|p| p.provider == Provider::Codex) {
                    p.asking = pty.screen(&p.id).is_some_and(|s| crate::codex::session::asks(&s));
                }
                let summary = sample(probes, crate::background::activity());
                crate::power::tick(&summary);
            }
        })
        .expect("spawn activity thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A chat's leftovers by a helper's measure: a `tail -F` its tool left held
    /// the machine for as long as it lived.
    #[test]
    fn a_chats_leftover_holds_the_machine_until_long_idle() {
        let read = |pid: u32, cpu_ms: u64| ProcRead { pid, cpu_ms, io_bytes: 0, started_ms: 2_000 };
        let name = |pid: u32| Some(format!("p{pid}.exe"));
        // Started by its turns (after 1_000), which were on until 9_000: as quiet as that.
        let mut t = Track { turn_at_ms: 9_000, ..Default::default() };
        let (first, _) = children_at(&mut t, vec![read(1, 10), read(2, 10)], 1_000, &name, 10_000);
        assert_eq!(first, ["p1.exe (pid 1)", "p2.exe (pid 2)"]);
        assert!(t.left.iter().all(|l| l.moved_ms == 9_000), "birth is no work");
        // Long past LEFTOVER_HOLD_MS, a window after their last read: one burns
        // CPU, one only sits there and lets go.
        let at = 9_000 + procs::LEFTOVER_HOLD_MS + 15_000;
        t.left.iter_mut().for_each(|l| {
            l.read_ms = at - procs::LEFTOVER_WINDOW_MS;
            l.seen_ms = 9_000 + procs::LEFTOVER_HOLD_MS - 1_000;
        });
        let (holding, gone) = children_at(&mut t, vec![read(1, 10), read(2, 2_010)], 1_000, &name, at);
        assert_eq!((holding, gone), (vec!["p2.exe (pid 2)".to_string()], vec!["p1.exe (pid 1)".to_string()]));
        // That one gone, the idle one holds nothing, and is still there for its notice.
        let (holding, gone) = children_at(&mut t, vec![read(1, 11)], 1_000, &name, at + 5_000);
        assert!(holding.is_empty() && gone.is_empty(), "let go once");
        assert!(t.procs.as_ref().is_some_and(|b| b.young == [1]), "its job is not over");
        assert_eq!(procs::quiet_ms(&t.left, at + 5_000), Some(at + 5_000 - 9_000));
    }

    /// A wait loop its turn left (`until grep …; do sleep 10; done`), sampled
    /// every five seconds: a fork's 80 ms in one sample is no work. It neither
    /// holds the machine at that sample, which would start a power-off's quiet
    /// window over, nor starts its LEFTOVER_HOLD_MS over; it lets go once.
    #[test]
    fn a_loops_busy_sample_is_no_work() {
        let read = |cpu_ms: u64| ProcRead { pid: 7, cpu_ms, io_bytes: 0, started_ms: 2_000 };
        let name = |pid: u32| Some(format!("bash{pid}.exe"));
        let mut t = Track { turn_at_ms: 9_000, ..Default::default() };
        children_at(&mut t, vec![read(100)], 1_000, &name, 10_000);
        // Just short of LEFTOVER_HOLD_MS since its turn, as last looked at a sample ago.
        let start = 9_000 + procs::LEFTOVER_HOLD_MS - 30_000;
        t.left.iter_mut().for_each(|l| {
            l.read_ms = start - 5_000;
            l.seen_ms = start - 5_000;
        });
        let mut cpu = 100;
        let mut gone_said = 0;
        for i in 0..36u64 {
            let at = start + i * SAMPLE_EVERY.as_millis() as u64;
            // A tick of CPU now and then, and one sample of 80 ms once it has
            // sat idle long enough to let go.
            cpu += match i {
                8 => 80,
                i if i % 3 == 0 => 16,
                _ => 0,
            };
            let (holding, gone) = children_at(&mut t, vec![read(cpu)], 1_000, &name, at);
            let quiet = at.saturating_sub(9_000) >= procs::LEFTOVER_HOLD_MS;
            assert_eq!(holding.is_empty(), quiet, "sample {i}: {holding:?}");
            gone_said += gone.len();
            assert_eq!(t.left[0].moved_ms, 9_000, "sample {i}: no work, so none is noted");
        }
        assert_eq!(gone_said, 1, "let go, and said so once");
    }

    #[test]
    fn parses_hook_events() {
        let ev = |name: &str, extra: &str| {
            HookEvent::parse(&format!(r#"{{"session_id":"s","hook_event_name":"{name}"{extra}}}"#))
        };
        assert_eq!(ev("UserPromptSubmit", ""), Some(HookEvent::Busy));
        assert_eq!(ev("PostToolUse", r#","tool_name":"Bash""#), Some(HookEvent::Busy));
        assert_eq!(ev("Stop", r#","stop_hook_active":false"#), Some(HookEvent::Idle));
        assert_eq!(ev("SessionEnd", ""), Some(HookEvent::Gone));
        assert_eq!(ev("Notification", r#","notification_type":"permission_prompt""#), Some(HookEvent::Waiting));
        assert_eq!(ev("Notification", r#","notification_type":"idle_prompt""#), None);
        assert_eq!(ev("PreToolUse", ""), None);
        assert_eq!(ev("PreToolUse", r#","tool_name":"AskUserQuestion""#), Some(HookEvent::Waiting));
        assert_eq!(ev("PermissionRequest", r#","tool_name":"Bash""#), Some(HookEvent::Waiting));
        assert_eq!(HookEvent::parse("not json"), None);
        assert_eq!(ev("StopFailure", r#","error":"rate_limit""#), Some(HookEvent::Failed("rate_limit")));
        assert_eq!(ev("StopFailure", r#","error":"something_new""#), Some(HookEvent::Failed("unknown")));
        // A subagent's hooks, by the id only they carry, are never the turn's.
        let agent = |name: &str, extra: &str| ev(name, &format!(r#","agent_id":"a1"{extra}"#));
        let a1 = || "a1".to_string();
        assert_eq!(agent("SubagentStart", r#","agent_type":"general-purpose""#), Some(HookEvent::AgentAtWork(a1())));
        assert_eq!(agent("PostToolUse", r#","tool_name":"Bash""#), Some(HookEvent::AgentAtWork(a1())));
        assert_eq!(agent("PermissionRequest", r#","tool_name":"Bash""#), Some(HookEvent::AgentWaiting(a1())));
        assert_eq!(agent("Notification", r#","notification_type":"permission_prompt""#), Some(HookEvent::AgentWaiting(a1())));
        assert_eq!(agent("Notification", r#","notification_type":"idle_prompt""#), None);
        assert_eq!(agent("SubagentStop", ""), Some(HookEvent::AgentDone(a1())));
        assert_eq!(ev("SubagentStop", ""), None, "nobody's to count");
    }

    /// As on 2026-11-04: the turn ends having started a background agent,
    /// whose tools then fire hooks for an hour while the registry says busy.
    /// The turn is over, the session's job is not.
    #[test]
    fn a_background_agent_is_no_turn() {
        let id = "bg-agent";
        let hook = |body: &str| on_hook(id, HookEvent::parse(body).unwrap());
        hook(r#"{"hook_event_name":"UserPromptSubmit","prompt":"review it"}"#);
        hook(r#"{"hook_event_name":"SubagentStart","agent_id":"a1","agent_type":"general-purpose"}"#);
        hook(r#"{"hook_event_name":"PostToolUse","tool_name":"Agent"}"#);
        hook(r#"{"hook_event_name":"Stop"}"#);
        hook(r#"{"hook_event_name":"PostToolUse","tool_name":"Bash","agent_id":"a1"}"#);
        let mut st = state().lock().unwrap();
        let t = st.tracks.get_mut(id).unwrap();
        assert_eq!((t.hook, t.turns_ended, t.agents.len()), (Some(Turn::Idle), 1, 1), "its tool spoke for itself");
        t.turn = combine(t, Some(Turn::Busy), now_ms());
        assert_eq!(t.turn, Turn::Idle, "a busy registry is the agent's, not a turn");
        assert_eq!(agents_at_work(t).as_deref(), Some("1 subagent"));
        drop(st);
        let ts = turn_state(id).unwrap();
        assert!(ts.turn == Turn::Idle && ts.children_busy, "between turns, with a job at work");
        assert!(children_hold(id), "which holds the machine");
        assert_eq!(left_quiet(id), None, "and is not to be called idle");

        // Its own prompt holds the session on it, turn or no turn.
        hook(r#"{"hook_event_name":"PermissionRequest","tool_name":"Bash","agent_id":"a1"}"#);
        let mut st = state().lock().unwrap();
        let t = st.tracks.get_mut(id).unwrap();
        assert_eq!(combine(t, Some(Turn::Busy), now_ms()), Turn::Waiting);
        assert_eq!(agents_at_work(t), None, "waiting for a person keeps no machine awake");
        drop(st);
        hook(r#"{"hook_event_name":"PostToolUse","tool_name":"Bash","agent_id":"a1"}"#);

        // One of the CLI's own agents, never seen at work, ends: nothing of
        // the session's starts over for it.
        let mut st = state().lock().unwrap();
        st.tracks.get_mut(id).unwrap().disk_busy_since_ms = Some(1);
        drop(st);
        hook(r#"{"hook_event_name":"SubagentStop","agent_id":"internal"}"#);
        assert_eq!(state().lock().unwrap().tracks[id].disk_busy_since_ms, Some(1));

        // It ends; Claude Code takes its result up in a turn of its own,
        // which the registry says before any hook may: busy, judged afresh.
        hook(r#"{"hook_event_name":"SubagentStop","agent_id":"a1"}"#);
        let mut st = state().lock().unwrap();
        let t = st.tracks.get_mut(id).unwrap();
        let at = now_ms();
        assert_eq!(combine(t, Some(Turn::Busy), at), Turn::Busy);
        assert_eq!(combine(t, Some(Turn::Busy), at + STALE_DISK_MS + 1), Turn::Idle, "no hook since: left behind");
        drop(st);
        assert!(!turn_state(id).unwrap().children_busy);
        forget(id);
    }

    /// A SubagentStop that never came: the registry, idle a while, says no
    /// agent is left.
    #[test]
    fn an_idle_registry_ends_lost_agents() {
        let mut t = Track { hook: Some(Turn::Idle), ..Default::default() };
        t.agents.insert("lost".into(), false);
        assert_eq!(combine(&mut t, Some(Turn::Idle), 1_000), Turn::Idle);
        assert_eq!(t.agents.len(), 1, "a registry just gone idle may not have caught up");
        combine(&mut t, Some(Turn::Idle), 1_000 + STALE_HOOK_MS + 1);
        assert!(t.agents.is_empty());
    }

    #[test]
    fn a_turn_s_start_lasts_as_long_as_the_turn() {
        let id = "chat-turn-start";
        assert_eq!(turn_started_ms(id), None);
        on_hook(id, HookEvent::Busy);
        let at = turn_started_ms(id).expect("begun");
        state().lock().unwrap().tracks.get_mut(id).unwrap().turn_started_ms = Some(at - 1000);
        on_hook(id, HookEvent::Busy);
        on_hook(id, HookEvent::Waiting);
        assert_eq!(turn_started_ms(id), Some(at - 1000), "a tool call or a prompt inside the turn does not move it");
        on_hook(id, HookEvent::Idle);
        assert_eq!(turn_started_ms(id), None, "null between turns");
        on_hook(id, HookEvent::Busy);
        on_hook(id, HookEvent::Failed("rate_limit"));
        assert_eq!(turn_started_ms(id), None);
        forget(id);
    }

    #[test]
    fn a_failed_turn_is_over_and_remembers_why() {
        on_hook("chat-limit", HookEvent::Busy);
        on_hook("chat-limit", HookEvent::Failed("rate_limit"));
        {
            let st = state().lock().unwrap();
            let t = st.tracks.get("chat-limit").unwrap();
            assert_eq!((t.hook, t.failed, t.turns_ended), (Some(Turn::Idle), Some("rate_limit"), 1));
        }
        // The next prompt is a new turn, and the error is behind it.
        on_hook("chat-limit", HookEvent::Busy);
        assert_eq!(state().lock().unwrap().tracks.get("chat-limit").unwrap().failed, None);
    }

    #[test]
    fn a_dialog_in_the_registry_beats_a_busy_hook() {
        // The turn died on a usage limit with no Stop: the hook still says
        // busy, the registry says the limit dialog is up.
        let mut t = Track { hook: Some(Turn::Busy), ..Default::default() };
        assert_eq!(combine(&mut t, Some(Turn::Waiting), 1000), Turn::Waiting);
    }

    #[test]
    fn a_stop_hook_ends_a_turn_no_sample_saw() {
        on_hook("chat-fast-turn", HookEvent::Busy);
        on_hook("chat-fast-turn", HookEvent::Idle);
        on_hook("chat-fast-turn", HookEvent::Busy);
        on_hook("chat-fast-turn", HookEvent::Idle);
        assert_eq!(turn_state("chat-fast-turn").map(|t| t.turns_ended), Some(2));
        // SessionEnd is not a turn.
        on_hook("chat-fast-turn", HookEvent::Gone);
        assert_eq!(turn_state("chat-fast-turn").map(|t| t.turns_ended), Some(2));
    }

    #[test]
    fn a_session_that_comes_back_counts_on() {
        let mut tracks = HashMap::new();
        let mut ended = HashMap::new();
        // Codex, first process: its rollout says two turns.
        count_on(track(&mut tracks, &mut ended, "c"), 100, Some(2));
        assert_eq!(tracks["c"].turns_ended, 2);
        // Gone at a sample; back as a resumed process whose rollout has had
        // no turn of its own yet.
        drop_gone(&mut tracks, &mut ended, &[]);
        assert!(tracks.is_empty());
        let t = track(&mut tracks, &mut ended, "c");
        count_on(t, 200, Some(0));
        assert_eq!(t.turns_ended, 2, "not back to 0");
        count_on(t, 200, Some(1));
        assert_eq!(t.turns_ended, 3);
        // Respawned before any sample saw it gone: the same.
        count_on(t, 300, None);
        count_on(t, 300, Some(1));
        assert_eq!(t.turns_ended, 4);
        // Claude Code counts by hooks; a new process keeps the count.
        let mut tracks = HashMap::new();
        let mut ended = HashMap::from([("k".to_string(), 5)]);
        assert_eq!(track(&mut tracks, &mut ended, "k").turns_ended, 5);
        assert!(ended.is_empty());
    }

    #[test]
    fn folds_the_clis_words() {
        assert_eq!(turn_of("busy"), Turn::Busy);
        assert_eq!(turn_of("working"), Turn::Busy);
        assert_eq!(turn_of("waiting"), Turn::Waiting);
        assert_eq!(turn_of("waiting_for_permission"), Turn::Waiting);
        assert_eq!(turn_of("idle"), Turn::Idle);
        assert_eq!(turn_of("resting"), Turn::Idle);
    }

    #[test]
    fn waiting_hook_beats_busy_registry() {
        let mut t = Track { hook: Some(Turn::Waiting), ..Default::default() };
        assert_eq!(combine(&mut t, Some(Turn::Busy), 1000), Turn::Waiting);
    }

    #[test]
    fn busy_registry_beats_idle_hook() {
        let mut t = Track { hook: Some(Turn::Idle), ..Default::default() };
        assert_eq!(combine(&mut t, Some(Turn::Busy), 1000), Turn::Busy);
    }

    #[test]
    fn stale_busy_hook_yields_to_idle_registry() {
        let mut t = Track { hook: Some(Turn::Busy), ..Default::default() };
        assert_eq!(combine(&mut t, Some(Turn::Idle), 1000), Turn::Busy);
        assert_eq!(combine(&mut t, Some(Turn::Idle), 1000 + STALE_HOOK_MS + 1), Turn::Idle);
        assert_eq!(t.hook, Some(Turn::Idle));
    }

    #[test]
    fn stale_waiting_hook_yields_to_idle_registry() {
        let mut t = Track { hook: Some(Turn::Waiting), ..Default::default() };
        // A live prompt: the registry calls the turn busy, waiting stands.
        assert_eq!(combine(&mut t, Some(Turn::Busy), 1000), Turn::Waiting);
        // The turn was dismissed; the registry says idle, and for a while the
        // hook's word still holds, in case a Stop is on its way.
        assert_eq!(combine(&mut t, Some(Turn::Idle), 2000), Turn::Waiting);
        assert_eq!(combine(&mut t, Some(Turn::Idle), 2000 + STALE_WAIT_MS), Turn::Waiting);
        assert_eq!(combine(&mut t, Some(Turn::Idle), 2000 + STALE_WAIT_MS + 1), Turn::Idle);
        assert_eq!(t.hook, Some(Turn::Idle));
    }

    #[test]
    fn stale_busy_registry_yields_to_idle_hook() {
        // Stop arrived at t=1000; the registry keeps saying busy.
        let mut t = Track { hook: Some(Turn::Idle), hook_at_ms: 1000, ..Default::default() };
        assert_eq!(combine(&mut t, Some(Turn::Busy), 1000), Turn::Busy);
        assert_eq!(combine(&mut t, Some(Turn::Busy), 1000 + STALE_DISK_MS), Turn::Busy);
        assert_eq!(combine(&mut t, Some(Turn::Busy), 1000 + STALE_DISK_MS + 1), Turn::Idle);
        // A registry that goes idle and busy again starts the clock over.
        assert_eq!(combine(&mut t, Some(Turn::Idle), 200_000), Turn::Idle);
        assert_eq!(combine(&mut t, Some(Turn::Busy), 200_005), Turn::Busy);
        // And a hook that speaks again is believed at once.
        t.hook = Some(Turn::Busy);
        t.hook_at_ms = 200_010;
        assert_eq!(combine(&mut t, Some(Turn::Busy), 400_000), Turn::Busy);
    }

    #[test]
    fn without_hooks_a_busy_registry_stands() {
        let mut t = Track::default();
        assert_eq!(combine(&mut t, Some(Turn::Busy), 1), Turn::Busy);
        assert_eq!(combine(&mut t, Some(Turn::Busy), 1 + 10 * STALE_DISK_MS), Turn::Busy);
    }

    #[test]
    fn without_hooks_the_disk_decides() {
        let mut t = Track::default();
        assert_eq!(combine(&mut t, Some(Turn::Waiting), 1), Turn::Waiting);
        assert_eq!(combine(&mut t, None, 2), Turn::Idle);
    }
}
