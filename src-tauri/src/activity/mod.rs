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
//! - Codex: Luna's status hooks (codex/hooks.rs; the same `on_hook`) — a turn
//!   begun, held on a permission, over (`Stop`, `Interrupt`) — its rollout's
//!   turn boundaries (`task_started`, `task_complete`, `turn_aborted`) and the
//!   shells it says a turn left running, its subagents' rollouts, and the
//!   shells in the session's own job.
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
//! - `hooks`: what a hook of either CLI says.
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Default)]
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
    /// Codex: its rollout is read beside its hooks, and is its turn count.
    codex: bool,
    /// The hook's word, once any hook has spoken for this session.
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
    /// When the registry last set its status word (`statusUpdatedAt`), as last
    /// read: a registry newer than a hook is the later word.
    disk_at: Option<u64>,
    /// Codex: its rollout as last read.
    rollout: Option<RolloutRead>,
    /// Codex: the permission its `PermissionRequest` hook said is up, by the
    /// command it is for ("" where it named none), and when (`now_ms`); until
    /// that command's call has its output or its `PostToolUse`, a turn ends
    /// after it, or the next prompt goes in.
    asked: Option<(String, u64)>,
    /// Codex: when this CLI process's last prompt went in, by its
    /// `UserPromptSubmit` (`now_ms`), 0 for none: a turn runs from it until the
    /// rollout writes an end after it.
    prompt_ms: u64,
    /// Codex: the latest turn end of this CLI process any read of its rollout
    /// showed (epoch ms), kept through a read that fails.
    end_seen_ms: u64,
    /// Codex: the shells its rollout names that are taken as gone, by session
    /// id and the time of the call that started each (an id Codex gives again
    /// is another shell): at the CLI's exit (`SessionEnd`, and the first read
    /// after it). Never by the clock.
    shells_gone: std::collections::HashSet<(String, u64)>,
    /// Codex: when its `SessionEnd` was heard (`now_ms`), 0 for none: the
    /// record reads `unknown` until a read taken after it (`take`) settles it.
    exited_ms: u64,
    /// Codex: an approval dialog on its screen while its rollout says a turn
    /// runs.
    dialog: bool,
    /// What its job holds besides its CLI, as last listed (`procs::in_job`);
    /// None before the first read. Both CLIs: a background shell is in it
    /// whatever the CLI says of it.
    job: Option<Vec<(crate::procs::ProcRead, String)>>,
    /// Its job would not list at the last read: what runs beside its turns is
    /// not known, which counts as something running.
    unlisted: bool,
    /// What was in its job before its first turn (`turned`): the CLI's own
    /// furniture, its MCP servers. Snapshot at every look until then, and
    /// last as its first prompt goes in (`submitting`), never after, per CLI
    /// process.
    furniture: std::collections::HashSet<crate::procs::ProcRead>,
    /// The CLI process (by its spawn time) whose first prompt has gone in,
    /// its furniture taken then: no look adds to it after.
    furnished_for: u128,
    /// A turn of this CLI process has begun, by any signal: a hook, the
    /// rollout, the turn count.
    turned: bool,
    /// When its first turn began as the CLI says (Claude Code's first turn
    /// hook, Codex's first `task_started`), epoch ms: what its job holds born
    /// before it is furniture too, where start times are epoch ms
    /// (`procs::STARTS_IN_EPOCH_MS`).
    first_turn_ms: Option<u64>,
    /// What the inputs were at the last read, by hash: a change is activity
    /// (`State::last_activity`).
    signature: u64,
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
    /// The turn on last was ended by a hook (`Stop`, `StopFailure`), and
    /// counted there: its record going from a turn to none counts it again
    /// only once a new turn has begun.
    ended_by_hook: bool,
    /// The turn on last was counted by a read, its `Stop` missed: that `Stop`
    /// arriving late counts nothing. Cleared as a turn begins.
    read_counted: bool,
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
    /// Helpers under it still at work on an answer it is owed
    /// (`agents::helpers_owing`): the window shows it in the background
    /// while its own turn is over.
    pub helpers: u32,
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
            helpers: 0,
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
    /// Since when no session has been active, wall time, for the window to
    /// show; None while one is. The power rule goes by `last_activity`.
    pub idle_since_ms: Option<u64>,
    /// The last activity of any kind as of this board (`State::last_activity`),
    /// on the monotonic clock: what the power rule measures its quiet window and
    /// countdown from. None on a board made before any read, which is no quiet.
    #[serde(skip)]
    pub last_activity: Option<std::time::Instant>,
    pub sessions: Vec<SessionLife>,
}

impl Summary {
    /// The board of these sessions, the last activity having been at
    /// `last_activity`.
    pub fn of(sessions: Vec<SessionLife>, last_activity: std::time::Instant) -> Summary {
        let busy = sessions.iter().filter(|s| s.busy).count();
        Summary {
            busy,
            waiting: sessions.iter().filter(|s| s.life.phase == Phase::Asking).count(),
            stalled: sessions.iter().filter(|s| s.stalled.is_some()).count(),
            idle_since_ms: (busy == 0).then(|| now_ms().saturating_sub(last_activity.elapsed().as_millis() as u64)),
            last_activity: Some(last_activity),
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
    /// The last activity of any kind, on the monotonic clock, so no change of
    /// the wall clock moves it: a read that found a session active or any input
    /// of a record changed (`take`), any hook, a helper's turn claimed or ended
    /// or its job changed, an agent's call that may start work coming or going
    /// (`touch`), a sleep (`reset_idle`). Activity between two reads lands here
    /// too, so a power-off's window and countdown start over for it. The core's
    /// start to begin with.
    last_activity: std::time::Instant,
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    let s = S.get_or_init(|| {
        Mutex::new(State {
            tracks: HashMap::new(),
            ended: HashMap::new(),
            summary: Summary::default(),
            last_activity: std::time::Instant::now(),
        })
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

/// A session the record has seen and knows nothing of yet, as one just
/// started is, for the tests of what is said of it.
#[cfg(test)]
pub(crate) fn not_known_for_test(chat_id: &str) {
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    let st = &mut *st;
    let t = track(&mut st.tracks, &mut st.ended, chat_id);
    refold(t, now_ms());
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
/// more specific word wins, and no word is eternal; every age is counted from
/// the later of the two words, so a word never ages out on time that passed
/// before it was said.
fn fold_claude(t: &mut Track, now: u64) -> Option<Turn> {
    let disk = t.disk;
    // How long the registry has said idle since the hook last spoke. Its idle
    // counts against a hook only when the registry set it after that hook
    // (`statusUpdatedAt`): an idle from before, or of no known time, is an
    // older word, however long Luna has been reading it.
    let idle_after_hook = t.disk_at.is_some_and(|at| at > t.hook_at_ms);
    let idle_for = t.disk_idle_since_ms.filter(|_| idle_after_hook).map(|s| now.saturating_sub(s.max(t.hook_at_ms))).unwrap_or(0);
    // Claude Code calls itself busy while a subagent of it works, so an idle
    // registry that has stood a while says none is left: a SubagentStop that
    // never came must not keep the session at work for good. Not one older
    // than the subagent's last hook, which moves `hook_at_ms` too.
    if idle_for > STALE_HOOK_MS && !t.agents_on_disk {
        t.agents.clear();
    }
    let hook = match t.hook {
        // A hook that says busy against a registry that has said idle for a
        // while since has missed its Stop.
        Some(Turn::Busy) if idle_for > STALE_HOOK_MS => {
            t.hook = Some(Turn::Idle);
            Some(Turn::Idle)
        }
        // A hook that says waiting against a registry that has said idle for
        // a while since: the prompt went away with its turn, and no Stop followed.
        Some(Turn::Waiting) if idle_for > STALE_WAIT_MS => {
            t.hook = Some(Turn::Idle);
            Some(Turn::Idle)
        }
        h => h,
    };
    // The registry says busy, but the hook said the turn ended. A busy the
    // registry set after that hook is a turn whose start hook was missed, and
    // stands; one from before it is a subagent of the ended turn at work, or,
    // with nothing spoken since, a registry left behind. Not a turn either way.
    let newer = match t.disk_at {
        Some(at) => at > t.hook_at_ms,
        None => t.disk_busy_since_ms.is_some_and(|since| since > t.hook_at_ms + STALE_HOOK_MS),
    };
    let disk = match (hook, disk) {
        (Some(Turn::Idle), Some(Turn::Busy)) if !t.agents.is_empty() => Some(Turn::Idle),
        (Some(Turn::Idle), Some(Turn::Busy)) if newer => disk,
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

/// A Codex session's phase, outcome and error: its rollout's (`codex_phase`),
/// which alone says a turn ended (`task_complete`, `turn_aborted`) and how,
/// and counts the turns. Its hooks never end one: Codex runs `Stop` before it
/// decides to go on or compact, and a `Stop` or `Interrupt` only moves the
/// activity stamp. They give what the rollout cannot: a turn running from a
/// prompt until the rollout writes an end after it (`prompt_ms`: an older end
/// a rollout whose writes stopped still shows never masks it), or while the
/// rollout says nothing (none yet, unreadable, no turn read) from a tool's
/// hook, else not known; and a permission up (`asked`), asking whatever the
/// rollout says of the turn. An approval dialog on its screen while the
/// rollout says a turn runs is asking too: a `PermissionRequest` post may be
/// lost while the others came.
fn fold_codex(t: &Track) -> (Phase, Option<Outcome>, Option<String>) {
    if t.exited_ms > 0 {
        return (Phase::Unknown, None, None);
    }
    let rollout = codex_phase(t.rollout.as_ref());
    let prompted = t.prompt_ms > 0 && t.end_seen_ms <= t.prompt_ms;
    let running = match rollout.0 {
        Phase::Running => true,
        Phase::Ended => prompted,
        Phase::Unknown => prompted || matches!(t.hook, Some(Turn::Busy | Turn::Waiting)),
        Phase::Asking => false,
    };
    if t.asked.is_some() || (t.dialog && running) {
        return (Phase::Asking, None, None);
    }
    if running {
        return (Phase::Running, None, None);
    }
    rollout
}

/// Codex: the first read taken after its `SessionEnd` settles the record. A
/// claim the rollout still shows then is the exited process's and goes with
/// it; what the rollout says of its turn stands. Until that read the record
/// reads `unknown` (`fold_codex`): the rollout may not have its last lines yet.
/// A claim is never ended by the clock: with no end written and no exit it
/// stands.
fn settle_exit(t: &mut Track, read_at: u64) {
    if t.exited_ms == 0 || read_at <= t.exited_ms {
        return;
    }
    if let Some(r) = &t.rollout {
        t.shells_gone.extend(r.shells.iter().map(|(id, _, at)| (id.clone(), *at)));
    }
    t.exited_ms = 0;
}

/// Codex: the permission `asked` answered, by what the rollout says now: the
/// call of exactly that command has its output (a denial's included), any
/// tool's output for one that named no command, or a turn ended, after it was
/// asked. 0.169.0's `PermissionRequest` carries no call id to match by. Lines
/// are placed against the hook by their own time, the machine's clock both.
fn settle_asked(t: &mut Track) {
    let (Some((command, at)), Some(r)) = (&t.asked, &t.rollout) else { return };
    let ended = r.last_end_ms.is_some_and(|end| end > *at);
    let answered = r.answered.iter().any(|(ran, out)| *out > *at && (command.is_empty() || ran == command));
    if ended || answered {
        t.asked = None;
    }
}

/// What a session's job holds that its turns started: everything listed but
/// what was there before its first turn (`furniture`) and, where start times
/// are epoch ms, what was born before that turn began. None while the job will
/// not list.
fn shells_of(t: &Track) -> Option<Vec<String>> {
    if t.unlisted {
        return None;
    }
    let Some(job) = t.job.as_ref() else { return Some(Vec::new()) };
    if !t.turned {
        return Some(Vec::new());
    }
    let before_turns =
        |r: &crate::procs::ProcRead| crate::procs::STARTS_IN_EPOCH_MS && t.first_turn_ms.is_some_and(|first| r.started_ms < first);
    Some(
        job.iter()
            .filter(|(r, _)| !t.furniture.contains(r) && !before_turns(r))
            .map(|(r, name)| format!("{name} (pid {})", r.pid))
            .collect(),
    )
}

/// Folds what the track holds into its record.
fn refold(t: &mut Track, now: u64) {
    let (mut phase, outcome, error) = if t.codex {
        fold_codex(t)
    } else {
        let turn = fold_claude(t, now);
        claude_phase(turn, t.failed, t.api_error)
    };
    // A subagent on a prompt of its own waits for a person as a turn on one does.
    if t.agents.values().any(|asks| *asks) && phase != Phase::Running {
        phase = Phase::Asking;
    }
    if matches!(phase, Phase::Running | Phase::Asking) {
        t.turned = true;
    }
    let shells = shells_of(t);
    let listed = shells.as_ref().map_or(0, |s| s.len() as u32);
    // Codex: the shells its rollout says a turn left running, by name, but
    // those taken as gone.
    let named: Vec<String> = match (&t.rollout, t.codex) {
        (Some(r), true) => {
            r.shells.iter().filter(|(id, _, at)| !t.shells_gone.contains(&(id.clone(), *at))).map(|(_, name, _)| name.clone()).collect()
        }
        _ => Vec::new(),
    };
    let background = Background {
        // What its job shows, and what the CLI says itself runs beside the
        // turn: Claude Code's registry `shell` and its last `Stop`'s tasks,
        // which hold until a look at the job and the registry finds none;
        // the shells Codex's rollout names, until one's end is written.
        shells: if t.codex { listed.max(named.len() as u32) } else { listed.max(t.registry_shells).max(t.stop_tasks) },
        crons: t.stop_crons,
        subagents: t.agents.values().filter(|asks| !**asks).count() as u32,
        names: named.into_iter().chain(shells.clone().unwrap_or_default()).collect(),
        unlisted: shells.is_none(),
    };
    match phase {
        Phase::Running | Phase::Asking => {
            t.turn_started_ms.get_or_insert(now);
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
    touch();
}

/// Something happened that a read may not see as active: a helper's turn
/// claimed or ended, its job changed, an agent's call that may start work.
/// The power-off's quiet window and countdown start over.
pub fn touch() {
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    stamp(&mut st);
}

/// Activity now, under the lock.
fn stamp(st: &mut State) {
    let now = std::time::Instant::now();
    st.last_activity = now;
    st.summary.idle_since_ms = None;
    st.summary.last_activity = Some(now);
}

/// The last activity of any kind, as it stands this moment: what a power-off
/// checks again just before it fires.
pub fn last_activity() -> std::time::Instant {
    state().lock().unwrap_or_else(|e| e.into_inner()).last_activity
}

#[cfg(test)]
mod tests {
    /// A chat whose turn is over while a helper under it works on its answer
    /// reads as background in the window, off the core's own record.
    #[test]
    fn the_window_reads_a_chat_its_helpers_owe_as_background() {
        let ended = Life { phase: Phase::Ended, ..Life::default() };
        let read = |helpers: u32, life: &Life| {
            let s = SessionLife { helpers, ..SessionLife::of("chat-a", life.clone()) };
            let json = serde_json::to_value(&s).unwrap();
            serde_json::from_value::<luna_window::status::dto::SessionActivity>(json).unwrap().status()
        };
        assert_eq!(read(0, &ended), Some("idle"));
        assert_eq!(read(1, &ended), Some("background"));
        // Its own turn still runs: working, whatever its helpers do.
        assert_eq!(read(1, &Life { phase: Phase::Running, ..Life::default() }), Some("busy"));
    }

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
    /// since, set after the hook; the registry wins, and a shell the registry
    /// names keeps it active.
    #[test]
    fn a_missed_stop_is_settled_by_the_registry() {
        let mut t = Track { hook: Some(Turn::Busy), hook_at_ms: 1_000, ..Default::default() };
        t.disk = Some(Turn::Idle);
        t.disk_at = Some(1_100);
        t.disk_idle_since_ms = Some(1_000);
        refold(&mut t, 2_000);
        assert_eq!(t.life.phase, Phase::Running, "a Stop may still be on its way");
        refold(&mut t, 1_000 + STALE_HOOK_MS + 1);
        assert_eq!(t.life.phase, Phase::Ended);
        t.registry_shells = 1;
        refold(&mut t, 1_000 + STALE_HOOK_MS + 2);
        assert!(t.life.active() && t.life.phase == Phase::Ended, "its background shell runs on");
    }

    /// An idle the registry set before the hook that says busy, or at no known
    /// time, is an older word: read for however long, it never ends the turn
    /// the hook began, nor drops a subagent whose hook came after it.
    #[test]
    fn an_idle_older_than_the_hook_never_ends_its_turn() {
        for disk_at in [Some(900), Some(1_000), None] {
            let mut t = Track { hook: Some(Turn::Busy), hook_at_ms: 1_000, disk: Some(Turn::Idle), disk_at, ..Default::default() };
            t.disk_idle_since_ms = Some(500);
            refold(&mut t, 1_000 + 10 * STALE_HOOK_MS);
            assert_eq!(t.life.phase, Phase::Running, "{disk_at:?}");
        }
        // The turn's Stop came, then a subagent's hook: the registry's idle,
        // set between the two, does not take the subagent away.
        let mut t = Track { hook: Some(Turn::Idle), hook_at_ms: 1_000, disk: Some(Turn::Idle), disk_at: Some(1_500), ..Default::default() };
        t.disk_idle_since_ms = Some(1_500);
        apply(&mut t, HookEvent::AgentAtWork("a1".into()), 2_000);
        refold(&mut t, 2_000 + 10 * STALE_HOOK_MS);
        assert_eq!(t.life.background.subagents, 1, "its hook is later than the idle");
        assert!(t.life.active());
    }

    /// Codex: the rollout's boundaries and the shells in its job.
    #[test]
    fn a_codex_record_follows_its_rollout_and_its_job() {
        let mut t = Track { codex: true, ..Default::default() };
        refold(&mut t, 1_000);
        assert_eq!(t.life.phase, Phase::Unknown, "no rollout yet: not known");
        t.rollout = Some(RolloutRead { seen_turn: true, working: true, ..Default::default() });
        refold(&mut t, 2_000);
        assert_eq!((t.life.phase, t.life.turn_started_ms), (Phase::Running, Some(2_000)));
        t.rollout = Some(RolloutRead { seen_turn: true, completed: 1, ..Default::default() });
        t.job = Some(vec![(crate::procs::ProcRead { pid: 9, started_ms: 5 }, "node.exe".into())]);
        refold(&mut t, 3_000);
        assert_eq!((t.life.phase, t.life.background.shells), (Phase::Ended, 1));
        assert!(t.life.active(), "a shell left in its job");
        assert_eq!(t.life.background.names, ["node.exe (pid 9)"]);
        t.job = Some(Vec::new());
        refold(&mut t, 4_000);
        assert!(!t.life.active());
        t.unlisted = true;
        refold(&mut t, 5_000);
        assert!(t.life.active(), "a job that would not list: not known");
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
