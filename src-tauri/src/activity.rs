//! Whether each session is working, waiting for a person, or done — the one
//! answer keep-awake and shutdown-when-done both hang on.
//!
//! Three signals per session, any of which makes it busy:
//!
//! - the turn: Claude Code says so through its hooks (see hub.rs) and its
//!   registry file, Codex through the rollout; a permission prompt counts as
//!   *waiting*, which is neither working nor done;
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
/// CPU growth per sample below this is a spinner, not a job.
const MIN_CPU_MS: u64 = 50;
/// A hook said busy but the registry has said idle for this long: the Stop
/// hook was missed (a crash, a CLI too old for that event), the registry wins.
const STALE_HOOK_MS: u64 = 15_000;
/// A process born up to this long before the sampled turn start still belongs
/// to the turn: without hooks the start is only seen at the next sample.
const TURN_SLACK_MS: u64 = SAMPLE_EVERY.as_millis() as u64;
/// A child this old that does not move is a helper the CLI keeps, not a job.
const STALE_CHILD_MS: u64 = 2 * 3600 * 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Turn {
    Busy,
    Waiting,
    Idle,
}

/// What a Claude Code hook told us.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookEvent {
    /// `UserPromptSubmit`, `PostToolUse`: the model is at work.
    Busy,
    /// `Notification` with `permission_prompt` or `elicitation_dialog`.
    Waiting,
    /// `Stop`: the turn is over.
    Idle,
    /// `SessionEnd`.
    Gone,
}

impl HookEvent {
    pub fn parse(body: &str) -> Option<HookEvent> {
        let v: serde_json::Value = serde_json::from_str(body).ok()?;
        match v["hook_event_name"].as_str()? {
            "UserPromptSubmit" | "PostToolUse" | "PostToolUseFailure" | "SubagentStop" => Some(HookEvent::Busy),
            "Stop" => Some(HookEvent::Idle),
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
    /// The combined verdict of the last sample.
    turn: Turn,
    /// When the first turn began (0 = never): children born after it are the
    /// model's own, whichever turn started them.
    first_turn_ms: u64,
    prev: Vec<ProcRead>,
    procs: Option<procs::Busy>,
    output_fresh: bool,
    /// Busy → not-busy transitions seen: what an agent's `wait` counts.
    turns_ended: u32,
    /// The last verdict `busy` was computed from, for the transition above.
    was_busy: bool,
}

/// What an agent's `wait`/`read` asks about a session.
#[derive(Clone, Copy)]
pub struct TurnState {
    pub turn: Turn,
    pub busy: bool,
    pub turns_ended: u32,
}

pub fn turn_state(chat_id: &str) -> Option<TurnState> {
    let st = state().lock().unwrap_or_else(|e| e.into_inner());
    let t = st.tracks.get(chat_id)?;
    let busy = st.summary.sessions.iter().find(|s| s.id == chat_id).map(|s| s.busy).unwrap_or(false);
    Some(TurnState { turn: t.turn, busy, turns_ended: t.turns_ended })
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
    /// Children keeping it busy, by exe name, when any.
    pub procs: Vec<String>,
    pub output_fresh: bool,
    pub busy: bool,
}

/// The whole board at one sample.
#[derive(Clone, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub busy: usize,
    pub waiting: usize,
    /// Since when nothing has been busy or waiting, if that is the case.
    pub idle_since_ms: Option<u64>,
    pub sessions: Vec<SessionActivity>,
}

struct State {
    tracks: HashMap<String, Track>,
    summary: Summary,
    idle_since_ms: Option<u64>,
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(State { tracks: HashMap::new(), summary: Summary::default(), idle_since_ms: None }))
}

/// A hook arrived for a session (hub.rs). Takes effect at the next sample;
/// the timestamp is the truth for "when did the turn start".
pub fn on_hook(chat_id: &str, ev: HookEvent) {
    let now = now_ms();
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    let t = st.tracks.entry(chat_id.to_string()).or_default();
    match ev {
        HookEvent::Busy => {
            if t.first_turn_ms == 0 {
                t.first_turn_ms = now;
            }
            t.hook = Some(Turn::Busy);
        }
        HookEvent::Waiting => t.hook = Some(Turn::Waiting),
        HookEvent::Idle | HookEvent::Gone => t.hook = Some(Turn::Idle),
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

/// What the disk says about a session's turn right now.
fn disk_turn(p: &crate::pty::ActivityProbe) -> Option<Turn> {
    let raw = match p.provider {
        Provider::Claude => crate::claude::session::status(p.pid, &p.cwd, p.spawned_at_ms, &p.account_path)?,
        Provider::Codex => crate::codex::session::status(&crate::codex::session::Live {
            chat_id: &p.id,
            cwd: &p.cwd,
            spawned_at_ms: p.spawned_at_ms,
            resume: p.resume.as_deref(),
            account_path: &p.account_path,
            last_output_ms: p.last_output_ms,
        })?,
    };
    Some(turn_of(&raw))
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
    match disk {
        Some(Turn::Idle) => {
            t.disk_idle_since_ms.get_or_insert(now);
        }
        _ => t.disk_idle_since_ms = None,
    }
    let hook = match t.hook {
        // A hook that says busy against a registry that has said idle for a
        // while has missed its Stop.
        Some(Turn::Busy) if t.disk_idle_since_ms.is_some_and(|s| now.saturating_sub(s) > STALE_HOOK_MS) => {
            t.hook = Some(Turn::Idle);
            Some(Turn::Idle)
        }
        h => h,
    };
    match (hook, disk) {
        // A permission prompt sits inside a turn the registry calls busy.
        (Some(Turn::Waiting), _) => Turn::Waiting,
        (Some(Turn::Busy), _) | (_, Some(Turn::Busy)) => Turn::Busy,
        (None, Some(d)) => d,
        (Some(h), None) => h,
        (Some(Turn::Idle), Some(d)) => d,
        (None, None) => Turn::Idle,
    }
}

/// One pass over every live session. Called by the sampler thread; public so
/// a test can drive it with probes of its own.
pub fn sample(probes: Vec<crate::pty::ActivityProbe>) -> Summary {
    let now = now_ms();
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    let st = &mut *st;

    let live: Vec<&str> = probes.iter().map(|p| p.id.as_str()).collect();
    st.tracks.retain(|id, _| live.contains(&id.as_str()));

    let mut sessions = Vec::with_capacity(probes.len());
    for p in &probes {
        let t = st.tracks.entry(p.id.clone()).or_default();

        let disk = disk_turn(p);
        let turn = combine(t, disk, now);
        // Without a hook the start of a turn is seen at the sample after it
        // happened; a child born in between is still the turn's.
        if turn == Turn::Busy && t.first_turn_ms == 0 {
            t.first_turn_ms = now.saturating_sub(TURN_SLACK_MS);
        }
        t.turn = turn;

        let children = procs::children(p.job.as_deref(), p.pid);
        t.procs = procs::judge(&t.prev, &children, t.first_turn_ms, now.saturating_sub(STALE_CHILD_MS), MIN_CPU_MS);
        t.prev = children;
        t.output_fresh = now.saturating_sub(p.last_output_ms) < OUTPUT_FRESH_MS;

        let pids: Vec<u32> = t
            .procs
            .as_ref()
            .map(|b| b.active.iter().chain(b.young.iter()).copied().collect())
            .unwrap_or_default();
        let mut names: Vec<String> = procs::names(&pids).into_iter().map(|(_, n)| n).collect();
        names.sort();
        names.dedup();

        // A turn "ends" for an agent when the model stops, not when its
        // background jobs do: `wait` wants the reply, and the children are
        // the machine's concern (power.rs), not the caller's.
        let model_busy = t.turn == Turn::Busy;
        if t.was_busy && !model_busy {
            t.turns_ended += 1;
        }
        t.was_busy = model_busy;

        let busy = model_busy || t.procs.is_some() || t.output_fresh;
        sessions.push(SessionActivity { id: p.id.clone(), turn: t.turn, procs: names, output_fresh: t.output_fresh, busy });
    }

    let busy = sessions.iter().filter(|s| s.busy).count();
    let waiting = sessions.iter().filter(|s| !s.busy && s.turn == Turn::Waiting).count();
    if busy == 0 && waiting == 0 {
        st.idle_since_ms.get_or_insert(now);
    } else {
        st.idle_since_ms = None;
    }
    st.summary = Summary { busy, waiting, idle_since_ms: st.idle_since_ms, sessions };
    st.summary.clone()
}

/// The sampler thread: probes, samples, hands the board to power.rs.
pub fn start(app: tauri::AppHandle) {
    std::thread::Builder::new()
        .name("luna-activity".into())
        .spawn(move || {
            use tauri::Manager;
            let mut last = std::time::Instant::now();
            loop {
                std::thread::sleep(SAMPLE_EVERY);
                let gap = last.elapsed();
                last = std::time::Instant::now();
                // A sample that took far longer than its period to come round
                // is the machine having been asleep, not the sessions idle.
                if gap > SAMPLE_EVERY * 6 {
                    crate::log::info("activity", &format!("clock jumped {gap:?}; idle stretch reset"));
                    reset_idle();
                    crate::power::woke(&app);
                }
                let probes = app.state::<crate::pty::PtyManager>().activity_probes();
                let summary = sample(probes);
                crate::power::tick(&app, &summary);
            }
        })
        .expect("spawn activity thread");
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(HookEvent::parse("not json"), None);
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
    fn without_hooks_the_disk_decides() {
        let mut t = Track::default();
        assert_eq!(combine(&mut t, Some(Turn::Waiting), 1), Turn::Waiting);
        assert_eq!(combine(&mut t, None, 2), Turn::Idle);
    }
}
