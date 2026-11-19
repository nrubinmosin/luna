//! Each session's lifecycle record (`Life`, record.rs), kept from what its CLI
//! says, and every view of it the rest of Luna reads: the agents' turn state,
//! the rows' status, the power rule's board.
//!
//! The signals, per CLI:
//!
//! - Claude Code: its hooks (hub.rs hands each over, `on_hook`) — a turn begun,
//!   held on a question or permission, over (`Stop`, with the background tasks
//!   and session crons it leaves), failed (`StopFailure`) — and its registry
//!   file `sessions/<pid>.json` (status, `shell`), which settles a hook missed.
//! - Codex: its rollout's turn boundaries (`task_started`, `task_complete`,
//!   `turn_aborted`), its subagents' rollouts, and the shells in the session's
//!   own job. Codex status hooks, once trusted, are one more source here.
//! - A helper (background/): its turn's process and stream-json events, and
//!   what is left in its job; `background::lives` makes its record.
//!
//! Nothing is measured: no CPU, no I/O, no screen. What a CLI does not say is
//! not known, and not known counts as active.
//!
//! This module holds the records and their views. The rest is split by where a
//! signal comes from:
//!
//! - `record`: the record and how each CLI's words fold into it.
//! - `hooks`: what a Claude Code hook says.
//! - `reconcile`: what the disk and the jobs say, read every few seconds, and
//!   the board power/ follows.

use crate::provider::Provider;
use crate::throttle::now_ms;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

mod hooks;
mod reconcile;
mod record;

// Every file of the module reaches the others' items through `use super::*`,
// and the rest of the crate keeps its `crate::activity::X` paths.
pub use hooks::*;
pub use reconcile::*;
pub use record::*;

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

/// The turn as the agents' layer counts it, read off the record (`Life::turn`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Turn {
    Busy,
    Waiting,
    #[default]
    Idle,
}

/// What a session's CLI has said, kept between reads: the record is folded
/// from it (`refold`) at every hook and every read.
#[derive(Default)]
struct Track {
    /// Codex: no hooks; its rollout is read instead.
    codex: bool,
    /// Claude Code: the hook's word, once any hook has spoken for this session.
    hook: Option<Turn>,
    hook_at_ms: u64,
    /// The registry's word as last read.
    disk: Option<Turn>,
    /// When the registry last started saying idle, for aging a stale hook.
    disk_idle_since_ms: Option<u64>,
    /// When the registry started saying busy, for aging it against a hook that
    /// has since said the turn is over.
    disk_busy_since_ms: Option<u64>,
    /// The API error the last turn ended on, per `StopFailure`; cleared by any
    /// other hook of the turn's own.
    failed: Option<&'static str>,
    /// The API error the transcript's last line is, read while the registry
    /// says a dialog is up: a turn stopped on one before any hook said so.
    api_error: Option<&'static str>,
    /// What `Stop` said runs beside the turn it ended: background tasks and
    /// session crons. Stands until the next `Stop`, `SessionEnd` or respawn.
    stop_tasks: u32,
    stop_crons: u32,
    /// Background shells the registry says run (`shell`), as last read.
    registry_shells: u32,
    /// Codex: its rollout as last read, and whether a person typed into it or
    /// it resumed a thread (a rollout appears with the first turn).
    rollout: Option<RolloutRead>,
    typed_or_resumed: bool,
    /// Codex: what runs in its job born of its turns, by label.
    job: Vec<String>,
    /// When the first turn began (0 = never): what its job holds from then on
    /// is the turns' work; what came up with the CLI is not.
    first_turn_ms: u64,
    /// When the turn on now began.
    turn_started_ms: Option<u64>,
    /// Its subagents at work, by id, each with whether a prompt of its own is
    /// up. Claude Code's from `SubagentStart` (or its first tool) to
    /// `SubagentStop`, or until the registry has said idle a while, which it
    /// does only with none left; Codex's as their rollouts have them at each
    /// read.
    agents: HashMap<String, bool>,
    agents_on_disk: bool,
    /// Turns finished: what an agent's `wait` counts. From the CLI's own
    /// record — Claude Code's `Stop` hook, Codex's rollout — and from busy →
    /// not-busy transitions between reads only for a session with neither (a
    /// Claude Code that outlived a Luna restart).
    ///
    /// It only grows, across respawns and resumes too: an agent keeps the
    /// count it was last shown and waits for a turn past it, and a count that
    /// started over would leave it waiting out its whole timeout.
    turns_ended: u32,
    /// The process the count is taken for, by its spawn time, and
    /// `turns_ended` when it started: Codex's count of its own turns goes on
    /// from there (`count_on`).
    spawned_at_ms: u128,
    turns_before: u32,
    /// Whether the last fold said a turn was on, for the transition above.
    was_busy: bool,
    /// The record as last folded.
    life: Life,
}

/// What an agent's `wait`/`read` asks about a session, off its record.
#[derive(Clone, Copy, Debug)]
pub struct TurnState {
    pub turn: Turn,
    /// Its turn is not over: running, asking, or not known.
    pub busy: bool,
    pub turns_ended: u32,
    /// Work of its own goes on beside its turn: shells, crons, subagents
    /// (`Background::any`).
    pub children_busy: bool,
}

impl TurnState {
    pub fn of(life: &Life) -> TurnState {
        TurnState {
            turn: life.turn(),
            busy: life.phase != Phase::Ended,
            turns_ended: life.turns_ended,
            children_busy: life.background.any(),
        }
    }
}

/// A session's record: a helper's from its own (background/), else a
/// terminal's as last folded.
pub fn life(chat_id: &str) -> Option<Life> {
    if let Some(l) = crate::background::life(chat_id) {
        return Some(l);
    }
    state().lock().unwrap_or_else(|e| e.into_inner()).tracks.get(chat_id).map(|t| t.life.clone())
}

pub fn turn_state(chat_id: &str) -> Option<TurnState> {
    life(chat_id).map(|l| TurnState::of(&l))
}

/// Between turns, what a chat runs beside them, by name where Luna can tell,
/// else by count: for `luna_session_get`.
pub fn left_running(chat_id: &str) -> Vec<String> {
    let st = state().lock().unwrap_or_else(|e| e.into_inner());
    st.tracks.get(chat_id).filter(|t| t.life.phase == Phase::Ended).map(|t| shown(&t.life.background)).unwrap_or_default()
}

/// What runs beside a turn, by name where Luna can tell, else by count.
fn shown(bg: &Background) -> Vec<String> {
    match (&bg.names, bg.any()) {
        (names, _) if !names.is_empty() => names.clone(),
        (_, true) => vec![bg.line()],
        _ => Vec::new(),
    }
}

/// Whether work of a chat's goes on beside its turn: what it runs itself
/// (`Background::any`), or a helper of it, or of its helpers, it is still owed
/// an answer by.
pub fn in_background(chat_id: &str) -> bool {
    life(chat_id).is_some_and(|l| l.background.any()) || crate::agents::owes_answer(chat_id)
}

/// How many subagents of a session are at work, as its hooks or rollouts have
/// it.
pub fn subagents(chat_id: &str) -> usize {
    life(chat_id).map_or(0, |l| l.background.subagents as usize)
}

/// The API error a session's last turn ended on, per its `StopFailure` hook.
pub fn failed_of(chat_id: &str) -> Option<&'static str> {
    state().lock().unwrap_or_else(|e| e.into_inner()).tracks.get(chat_id)?.failed
}

/// When a visible session's current turn began, None between turns.
pub fn turn_started_ms(chat_id: &str) -> Option<u64> {
    state().lock().unwrap_or_else(|e| e.into_inner()).tracks.get(chat_id)?.turn_started_ms
}

/// One session on the board: its record, and what the power chip has always
/// been shown of it.
#[derive(Clone, Serialize, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SessionLife {
    pub id: String,
    pub turn: Turn,
    /// What it runs beside its turn, by name or count.
    pub procs: Vec<String>,
    /// Active (`Life::active`): it keeps the PC awake and a power-off back.
    pub busy: bool,
    /// The error its last turn ended on, a usage limit or another: over, and
    /// going nowhere by itself.
    pub stalled: Option<String>,
    pub life: Life,
}

impl SessionLife {
    pub fn of(id: &str, life: Life) -> SessionLife {
        SessionLife {
            id: id.to_string(),
            turn: life.turn(),
            procs: shown(&life.background),
            busy: life.active(),
            stalled: match (life.phase, life.outcome) {
                (Phase::Ended, Some(Outcome::UsageLimit)) => Some(life.error.clone().unwrap_or_else(|| "usage limit".into())),
                (Phase::Ended, Some(Outcome::Failed)) => Some(life.error.clone().unwrap_or_else(|| "error".into())),
                _ => None,
            },
            life,
        }
    }

    /// Why this session holds the machine, or "" when it does not.
    pub fn why(&self) -> String {
        self.life.why()
    }
}

/// The whole board at one read.
#[derive(Clone, Serialize, PartialEq, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    /// Sessions active (`Life::active`), unknown ones included.
    pub busy: usize,
    /// Of those, the ones held on a question or a permission.
    pub waiting: usize,
    /// Sessions whose last turn ended on an error: not active.
    pub stalled: usize,
    /// Since when no session has been active, if that is the case.
    pub idle_since_ms: Option<u64>,
    pub sessions: Vec<SessionLife>,
}

impl Summary {
    /// The board of these sessions, quiet since `idle_since_ms` if none is active.
    pub fn of(sessions: Vec<SessionLife>, idle_since_ms: Option<u64>) -> Summary {
        Summary {
            busy: sessions.iter().filter(|s| s.busy).count(),
            waiting: sessions.iter().filter(|s| s.life.phase == Phase::Asking).count(),
            stalled: sessions.iter().filter(|s| s.stalled.is_some()).count(),
            idle_since_ms,
            sessions,
        }
    }
}

struct State {
    tracks: HashMap<String, Track>,
    /// The turn count of each session whose track was dropped when it went
    /// away, for the track it gets when it comes back (see `turns_ended`).
    ended: HashMap<String, u32>,
    summary: Summary,
    /// The last moment any session was seen active, or did anything at all: a
    /// read that found one active, any hook, a helper's turn's end
    /// (`touch`). A turn begun and ended between two reads lands here too,
    /// so the quiet window starts over for it. The core's start to begin with.
    last_active_ms: u64,
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    let s = S.get_or_init(|| {
        Mutex::new(State { tracks: HashMap::new(), ended: HashMap::new(), summary: Summary::default(), last_active_ms: now_ms() })
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
    tracks.entry(id.to_string()).or_insert_with(|| {
        let turns_ended = ended.remove(id).unwrap_or(0);
        Track { turns_ended, life: Life { turns_ended, ..Default::default() }, ..Default::default() }
    })
}

/// A session back between turns on the count it had, as a read has it once
/// the window has started it, for the tests of what its caller is told.
#[cfg(test)]
pub(crate) fn between_turns_for_test(chat_id: &str) {
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    let st = &mut *st;
    let t = track(&mut st.tracks, &mut st.ended, chat_id);
    t.disk = Some(Turn::Idle);
    refold(t, now_ms());
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

/// Counts a read's turns for the process it was taken of: a new one (a
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

/// Claude Code's turn from the hook's word, the registry's word, and the
/// clock; None while neither has spoken. Where they disagree, the later and
/// more specific word wins, and no word is eternal.
fn fold_claude(t: &mut Track, now: u64) -> Option<Turn> {
    let disk = t.disk;
    let idle_for = t.disk_idle_since_ms.map(|s| now.saturating_sub(s)).unwrap_or(0);
    // Claude Code calls itself busy while a subagent of it works, so an idle
    // registry that has stood a while says none is left: a SubagentStop that
    // never came must not keep the session at work for good.
    if idle_for > STALE_HOOK_MS && !t.agents_on_disk {
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
    Some(match (hook, disk) {
        // A permission prompt sits inside a turn the registry calls busy.
        (Some(Turn::Waiting), _) => Turn::Waiting,
        // The registry says a dialog is up, so the model is not working,
        // whatever the last hook said: a turn that died on an API error
        // before Claude Code fired `StopFailure` left its hook at busy.
        (Some(Turn::Busy), Some(Turn::Waiting)) => Turn::Waiting,
        (Some(Turn::Busy), _) | (_, Some(Turn::Busy)) => Turn::Busy,
        (None, Some(d)) => d,
        (Some(h), None) => h,
        (Some(Turn::Idle), Some(d)) => d,
        (None, None) => return None,
    })
}

/// Folds what the track holds into its record.
fn refold(t: &mut Track, now: u64) {
    let (mut phase, outcome, error) = if t.codex {
        codex_phase(t.rollout.as_ref(), t.typed_or_resumed)
    } else {
        let turn = fold_claude(t, now);
        claude_phase(turn, t.failed, t.api_error)
    };
    // A subagent on a prompt of its own waits for a person as a turn on one does.
    if t.agents.values().any(|asks| *asks) && phase != Phase::Running {
        phase = Phase::Asking;
    }
    let shells = if t.codex { t.job.len() as u32 } else { t.stop_tasks.max(t.registry_shells) };
    let background = Background {
        shells,
        crons: t.stop_crons,
        subagents: t.agents.values().filter(|asks| !**asks).count() as u32,
        names: if t.codex { t.job.clone() } else { Vec::new() },
    };
    match phase {
        Phase::Running | Phase::Asking => {
            t.turn_started_ms.get_or_insert(now);
            if t.first_turn_ms == 0 {
                t.first_turn_ms = now;
            }
        }
        Phase::Ended => t.turn_started_ms = None,
        Phase::Unknown => {}
    }
    t.life = Life {
        phase,
        outcome: (phase == Phase::Ended).then_some(outcome).flatten(),
        error: if phase == Phase::Ended { error } else { None },
        turn_started_ms: t.turn_started_ms,
        background,
        turns_ended: t.turns_ended,
    };
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
        Some(t) => {
            t.turns_ended = t.turns_ended.max(turns_ended);
            t.life.turns_ended = t.turns_ended;
        }
        None => {
            let kept = st.ended.entry(chat_id.to_string()).or_default();
            *kept = (*kept).max(turns_ended);
        }
    }
}

/// How many turns the process started at `spawned_at_ms` has ended, the count of
/// those before it aside; none before a read has first seen it.
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

/// The last board read.
pub fn summary() -> Summary {
    state().lock().unwrap_or_else(|e| e.into_inner()).summary.clone()
}

/// Forget the quiet stretch: after a sleep the machine has not been quiet for
/// the time the clock says it has.
pub fn reset_idle() {
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    st.last_active_ms = now_ms();
    st.summary.idle_since_ms = None;
}

/// Something happened that a read may not see as active: a helper's turn
/// that began and ended between two reads. The quiet window starts over.
pub fn touch() {
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    st.last_active_ms = now_ms();
    st.summary.idle_since_ms = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_that_comes_back_counts_on() {
        let mut tracks = HashMap::new();
        let mut ended = HashMap::new();
        // Codex, first process: its rollout says two turns.
        count_on(track(&mut tracks, &mut ended, "c"), 100, Some(2));
        assert_eq!(tracks["c"].turns_ended, 2);
        // Gone at a read; back as a resumed process whose rollout has had
        // no turn of its own yet.
        drop_gone(&mut tracks, &mut ended, &[]);
        assert!(tracks.is_empty());
        let t = track(&mut tracks, &mut ended, "c");
        count_on(t, 200, Some(0));
        assert_eq!(t.turns_ended, 2, "not back to 0");
        count_on(t, 200, Some(1));
        assert_eq!(t.turns_ended, 3);
        // Respawned before any read saw it gone: the same.
        count_on(t, 300, None);
        count_on(t, 300, Some(1));
        assert_eq!(t.turns_ended, 4);
        // Claude Code counts by hooks; a new process keeps the count.
        let mut tracks = HashMap::new();
        let mut ended = HashMap::from([("k".to_string(), 5)]);
        assert_eq!(track(&mut tracks, &mut ended, "k").turns_ended, 5);
        assert!(ended.is_empty());
    }

    /// A Claude Code chat with no hook and no registry yet is not known, and
    /// so active; its registry's first word settles it.
    #[test]
    fn a_claude_chat_not_heard_from_is_active_until_its_registry_speaks() {
        let mut t = Track::default();
        refold(&mut t, 1_000);
        assert_eq!(t.life.phase, Phase::Unknown);
        assert!(t.life.active());
        t.disk = Some(Turn::Idle);
        t.disk_idle_since_ms = Some(1_000);
        refold(&mut t, 2_000);
        assert_eq!(t.life.phase, Phase::Ended);
        assert!(!t.life.active());
    }

    /// `Stop` with background tasks or crons: the turn is over, the session
    /// is not done; the next `Stop` with none lets go.
    #[test]
    fn a_stop_that_leaves_tasks_or_crons_stays_active() {
        let id = "rec-stop-left";
        on_hook(id, HookEvent::Busy);
        on_hook(id, HookEvent::parse(r#"{"hook_event_name":"Stop","background_tasks":[{"id":"b1"}],"session_crons":[]}"#).unwrap());
        let l = life(id).unwrap();
        assert_eq!((l.phase, l.background.shells, l.background.crons), (Phase::Ended, 1, 0));
        assert!(l.active());
        assert_eq!(l.status(), "background");
        on_hook(id, HookEvent::Busy);
        on_hook(id, HookEvent::parse(r#"{"hook_event_name":"Stop","background_tasks":[],"session_crons":[{"id":"c"}]}"#).unwrap());
        let l = life(id).unwrap();
        assert_eq!((l.background.shells, l.background.crons), (0, 1), "a cron will start turns by itself");
        assert!(l.active());
        on_hook(id, HookEvent::Busy);
        on_hook(id, HookEvent::Idle);
        assert!(!life(id).unwrap().active());
        assert_eq!(life(id).unwrap().turns_ended, 3);
        forget(id);
    }

    /// A missed `Stop`: the hook said busy, the registry has said idle long
    /// since; the registry wins, and a shell the registry names keeps it active.
    #[test]
    fn a_missed_stop_is_settled_by_the_registry() {
        let mut t = Track { hook: Some(Turn::Busy), hook_at_ms: 1_000, ..Default::default() };
        t.disk = Some(Turn::Idle);
        t.disk_idle_since_ms = Some(1_000);
        refold(&mut t, 2_000);
        assert_eq!(t.life.phase, Phase::Running, "a Stop may still be on its way");
        refold(&mut t, 1_000 + STALE_HOOK_MS + 1);
        assert_eq!(t.life.phase, Phase::Ended);
        t.registry_shells = 1;
        refold(&mut t, 1_000 + STALE_HOOK_MS + 2);
        assert!(t.life.active() && t.life.phase == Phase::Ended, "its background shell runs on");
    }

    /// Codex: the rollout's boundaries and the shells in its job.
    #[test]
    fn a_codex_record_follows_its_rollout_and_its_job() {
        let mut t = Track { codex: true, ..Default::default() };
        refold(&mut t, 1_000);
        assert_eq!(t.life.phase, Phase::Ended, "no rollout, nothing typed: no turn yet");
        t.typed_or_resumed = true;
        refold(&mut t, 1_000);
        assert_eq!(t.life.phase, Phase::Unknown, "typed into, no rollout yet");
        t.rollout = Some(RolloutRead { working: true, ..Default::default() });
        refold(&mut t, 2_000);
        assert_eq!((t.life.phase, t.life.turn_started_ms), (Phase::Running, Some(2_000)));
        t.rollout = Some(RolloutRead { completed: 1, ..Default::default() });
        t.job = vec!["node.exe (pid 9)".into()];
        refold(&mut t, 3_000);
        assert_eq!((t.life.phase, t.life.background.shells), (Phase::Ended, 1));
        assert!(t.life.active(), "a shell left in its job");
        assert_eq!(t.life.background.names, ["node.exe (pid 9)"]);
        t.job.clear();
        refold(&mut t, 4_000);
        assert!(!t.life.active());
    }

    #[test]
    fn a_subagent_asking_is_a_wait() {
        let mut t = Track { hook: Some(Turn::Idle), ..Default::default() };
        t.agents.insert("a1".into(), true);
        refold(&mut t, 1_000);
        assert_eq!(t.life.phase, Phase::Asking);
        t.agents.insert("a1".into(), false);
        refold(&mut t, 1_000);
        assert_eq!((t.life.phase, t.life.background.subagents), (Phase::Ended, 1));
        assert!(t.life.active());
    }
}
