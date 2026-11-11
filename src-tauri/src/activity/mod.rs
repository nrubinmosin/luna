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
//! - the children: the processes the CLI has under it (procs/) — moving
//!   ones, and ones born of the current turn even if they only sleep;
//! - the screen: pty output in the last half minute.
//!
//! The sampler runs every few seconds on its own thread and hands the summary
//! to power/. The UI's own 4s poll (useSessionWatch) keeps drawing the
//! chat rows; this tracker is what the machine's power state follows.
//!
//! This module holds what the sampler keeps per session, the counts of turns
//! and what is asked of it. The rest is split by where a signal comes from:
//!
//! - `hooks`: what a Claude Code hook says, and the turn it marks.
//! - `sample`: what the disk and the processes say at each sample, the verdict
//!   on each session drawn from all three, and the sampler thread.

use crate::procs::{self, ProcRead};
use crate::provider::Provider;
use crate::throttle::now_ms;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

mod hooks;
mod sample;

// Every file of the module reaches the others' items through `use super::*`,
// and the rest of the crate keeps its `crate::activity::X` paths.
pub use hooks::*;
pub use sample::*;

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
#[derive(Default)]
pub enum Turn {
    Busy,
    Waiting,
    #[default]
    Idle,
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
    /// When the turn on now began, for `luna_session_get`: a hook's moment, or the
    /// sample that first saw it where no hook speaks.
    turn_started_ms: Option<u64>,
    prev: Vec<ProcRead>,
    procs: Option<procs::Busy>,
    /// What its turns started that is still there (`procs::Busy::young`), each
    /// with when it was last at work: what holds the machine of it, what
    /// `luna_session_get` names between turns, and what its caller hears has sat idle.
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
    /// up. Claude Code's from `SubagentStart` (or its first tool) to
    /// `SubagentStop`, or until the registry has said idle a while, which it
    /// does only with none left; Codex's as their rollouts have them at each
    /// sample (`disk_subagents`).
    agents: HashMap<String, bool>,
    /// `agents` is read off the disk whole at each sample (Codex), not kept
    /// from hooks.
    agents_on_disk: bool,
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
/// `luna_session_get`, and for its caller to hear when all of it has sat idle long.
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

/// Whether work of a chat's goes on beside its turn: what holds the machine of
/// it (`children_hold`), so that the dot and the power-off let go at the same
/// moment, or a helper of it, or of its helpers, it is still owed an answer by.
pub fn in_background(chat_id: &str) -> bool {
    children_hold(chat_id) || crate::agents::owes_answer(chat_id)
}

/// Whether a CLI that says busy is busy only for its subagents: Claude Code's
/// registry does so while one works, the turn that started it over or not, and
/// the hook said it is over. By the hook rather than the last sample, which a
/// `Stop` or a new prompt is ahead of by up to SAMPLE_EVERY. Not with one of
/// them on a prompt, which is a wait. Codex says no such busy: a rollout that
/// says working is a turn.
pub fn busy_for_subagents_only(chat_id: &str) -> bool {
    let st = state().lock().unwrap_or_else(|e| e.into_inner());
    st.tracks.get(chat_id).is_some_and(|t| t.hook == Some(Turn::Idle) && !t.agents.is_empty() && !t.agents.values().any(|asks| *asks))
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
    // A helper with no terminal keeps its own count (background/).
    if let Some(t) = crate::background::turn_state(chat_id) {
        return Some(t);
    }
    let st = state().lock().unwrap_or_else(|e| e.into_inner());
    let t = st.tracks.get(chat_id)?;
    let busy = st.summary.sessions.iter().find(|s| s.id == chat_id).map(|s| s.busy).unwrap_or(false);
    Some(TurnState { turn: t.turn, busy, turns_ended: t.turns_ended, children_busy: t.procs.is_some() || !t.agents.is_empty() })
}

/// What a session's CLI runs of its own beside its turn, by name: processes its
/// turns started that are still there, whatever they are doing, and its
/// subagents at work. What a handoff cannot take across to a new conversation
/// (agents/handoff). A process the last sample saw that has gone since is not
/// counted: a command that wrote a file a moment ago is no job.
pub fn own_jobs(chat_id: &str) -> Vec<String> {
    let (left, agents) = {
        let st = state().lock().unwrap_or_else(|e| e.into_inner());
        st.tracks.get(chat_id).map(|t| (t.left.clone(), t.agents.len())).unwrap_or_default()
    };
    let mut jobs: Vec<String> = left.iter().filter(|l| procs::alive(l.pid)).map(procs::Leftover::label).collect();
    match agents {
        0 => {}
        1 => jobs.push("1 subagent".into()),
        n => jobs.push(format!("{n} subagents")),
    }
    jobs
}

/// Subagents of a session at work, for the tests of what may not cross a
/// handoff.
#[cfg(test)]
pub(crate) fn subagents_for_test(chat_id: &str, n: usize) {
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    let st = &mut *st;
    let t = track(&mut st.tracks, &mut st.ended, chat_id);
    t.agents = (0..n).map(|i| (format!("a{i}"), false)).collect();
}

/// A sample's verdict on a session whose disk says `disk`, for the tests of what
/// a chat reads.
#[cfg(test)]
pub(crate) fn sampled_for_test(chat_id: &str, disk: Option<Turn>) {
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    let st = &mut *st;
    let t = track(&mut st.tracks, &mut st.ended, chat_id);
    t.turn = combine(t, disk, now_ms());
}

/// How many subagents of a session are at work, as its hooks or rollouts have
/// it.
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
    /// its process (background/).
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
    tracks.entry(id.to_string()).or_insert_with(|| Track { turns_ended: ended.remove(id).unwrap_or(0), ..Default::default() })
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A job a turn left, last at work `idle_ms` ago, as a sample has it: still
    /// `young` while it lives, whatever it does.
    fn left_idle_for(id: &str, idle_ms: u64) {
        let now = now_ms();
        let mut st = state().lock().unwrap();
        let st = &mut *st;
        let t = track(&mut st.tracks, &mut st.ended, id);
        t.procs = Some(procs::Busy { active: Vec::new(), young: vec![7] });
        t.left = vec![procs::Leftover {
            pid: 7,
            exe: "tail".into(),
            started_ms: 0,
            cpu_ms: 0,
            io_bytes: 0,
            read_ms: now,
            seen_ms: now,
            moved_ms: now - idle_ms,
        }];
    }

    #[test]
    fn a_subagent_at_work_is_in_the_background() {
        let id = "ib-agent";
        assert!(!in_background(id), "nothing known of it");
        subagents_for_test(id, 1);
        assert!(in_background(id));
        subagents_for_test(id, 0);
        assert!(!in_background(id));
        forget(id);
    }

    #[test]
    fn a_leftover_is_in_the_background_as_long_as_it_holds_the_machine() {
        let id = "ib-left";
        left_idle_for(id, procs::LEFTOVER_HOLD_MS - 60_000);
        assert!(in_background(id), "idle, but not yet for long enough");
        left_idle_for(id, procs::LEFTOVER_HOLD_MS + 1);
        assert!(!in_background(id), "forgotten: the dot lets go when the machine does");
        forget(id);
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
}
