//! Helpers with no terminal and no row: what `luna_spawn` starts unless it is
//! asked for a visible chat. Each turn is one run of the CLI in its headless
//! mode — `claude -p --output-format stream-json`, `codex exec --json` — with
//! the prompt on stdin, the events on stdout, and the next turn a resume of
//! the same conversation. Whether it works is whether that process runs;
//! what it said is the turn's last message. No screen to read, no keys to
//! press, and no window needed to start one.
//!
//! Claude Code's stdin stays open until its first `result` (`feed_stdin`), so
//! text sent meanwhile joins the turn: the CLI takes it in at the next tool
//! boundary, and says so (`took`). `codex exec` reads its stdin once, to the
//! end: text sent to a Codex helper waits for the next turn.
//!
//! A Claude Code helper is never left to ask the air: its permission prompts
//! and `AskUserQuestion` come to Luna through `--permission-prompt-tool`
//! (`luna_permission`), which holds the CLI's call until the agent that
//! spawned it answers with `luna_answer`. Codex's headless mode has no one to
//! ask: a question it has comes in its reply, and the answer is the next turn.
//!
//! The conversation itself lands on disk as with any session — Claude Code's
//! transcript, Codex's rollout — so `luna_read` reads it there.

use crate::agents::Ask;
use crate::provider::Provider;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::sync::{Condvar, Mutex, OnceLock};

/// Lines of what a helper did kept for `luna_read(screen)`: its messages,
/// commands and errors, one line each.
const LOG_LINES: usize = 200;
/// How much of a message one log line keeps.
const LOG_CHARS: usize = 300;
/// How long a held permission or question waits for `luna_answer` before the
/// CLI is told nobody answered. Under the MCP call timeout the CLIs are given
/// (pty.rs `MCP_TOOL_TIMEOUT_MS`).
const ASK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// What a helper is started with, every turn the same.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Spec {
    pub provider: Provider,
    pub account_path: String,
    /// The project folder, or the worktree it was handed to run in.
    pub folder: String,
    /// A worktree of its own, made by the CLI on the first turn.
    pub worktree: bool,
    /// What Luna named that worktree (`agents::helper_worktree_name`): Claude Code's,
    /// as Codex takes no name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_name: Option<String>,
    pub model: Option<String>,
    pub effort: String,
    /// Claude: the permission mode. Codex: the sandbox.
    pub mode: String,
    /// Codex: `--approve-for-me`, a reviewer model answering what it would ask.
    pub auto_review: bool,
    pub tools: bool,
    /// Codex: run the project's hooks without their trust, which `exec` has
    /// no way to ask for (codex/hooks.rs).
    pub hooks_unasked: bool,
    /// Claude: without the edit tools. Codex's sandbox says it there.
    #[serde(default)]
    pub read_only: bool,
    /// Claude: the agent it runs as (`--agent`).
    #[serde(default)]
    pub agent: Option<String>,
    /// The session it answers to, for what it is told of itself: who started it
    /// until that one is deleted (`set_parent`). Here and not looked up:
    /// `command_for` runs under this module's lock.
    #[serde(default)]
    pub parent: Option<String>,
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

/// What a session's own events and files show beyond its state, as `luna_list`
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

/// One process a helper's turns left running, as `luna_list` shows it.
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

use crate::procs::{left_names, leftovers, Leftover};

/// An instant as the rest of `luna_list` writes one.
pub fn iso(ms: u64) -> Option<String> {
    let t = chrono::DateTime::<chrono::Utc>::from_timestamp((ms / 1000) as i64, 0)?;
    Some(t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

/// What a helper's turns have shown of it. All but the last event's moment
/// is saved with it: the model it ran on must outlive a core, and a moment
/// rewritten with every event would rewrite the file every SAVE_EVERY of every
/// turn.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Observed {
    model: Option<String>,
    /// When its last turn ended: the last it did, for a core started since.
    turn_ended_ms: u64,
    /// When it last printed anything.
    #[serde(skip)]
    event_ms: u64,
    /// Codex has said this turn runs on another model than it asked for.
    #[serde(skip)]
    rerouted: bool,
    /// Which turn this is, for what is read of the last one once it is over.
    #[serde(skip)]
    generation: u64,
    /// The model has written or run something this turn: a CLI that goes before
    /// that has started nothing of the model's (`turn_over`).
    #[serde(skip)]
    acted: bool,
    /// A turn's model has answered, this one's or one before: until then the
    /// model it was asked for is no model it ran on, whatever its error says
    /// (a next turn's start clears that, a restore drops it).
    answered: bool,
}

impl Observed {
    /// A turn's process is up.
    fn begin_turn(&mut self) {
        self.generation += 1;
        self.rerouted = false;
        self.acted = false;
    }

    /// What the rollout said of a Codex turn once it was over, unless another
    /// has begun since: its model.
    fn apply_rollout(&mut self, generation: u64, model: Option<String>) {
        if self.generation == generation && model.is_some() {
            self.model = model;
        }
    }
}

struct Run {
    pid: Option<u32>,
    /// When the turn started, for the window's "working 4 min".
    started_ms: u64,
    /// A stop came while the turn was claimed but had no process yet: it
    /// has no pid to kill, so `spawn_turn` does not start it, or kills what
    /// it just started.
    stopped: bool,
    /// Claude Code's stdin, while it still takes text (`feed_stdin`).
    feed: Option<std::sync::mpsc::Sender<String>>,
    /// The uuid of the line this process was started on.
    line: Option<String>,
    /// A `result` of this process's said it failed: a later one that succeeds
    /// clears what that one left in `Entry::error`.
    result_failed: bool,
}

impl Run {
    fn claimed(pid: Option<u32>, started_ms: u64) -> Run {
        Run { pid, started_ms, stopped: false, feed: None, line: None, result_failed: false }
    }
}

/// A fresh id in the shape Claude Code's stream-json wants of a message's uuid.
/// Unique is all it has to be; no crate here makes them.
fn new_id() -> String {
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

/// A batch as one prompt, as the turn gets it.
pub(crate) fn joined(batch: &[Queued]) -> String {
    batch.iter().map(|q| q.text.as_str()).collect::<Vec<_>>().join("\n\n")
}

/// A batch goes as one line of stdin, named for its first message.
fn stamp(batch: &mut [Queued]) {
    let line = batch.first().map(|q| q.id.clone());
    for q in batch {
        q.line = line.clone();
    }
}

/// Text that a CLI would read as a command (`/`, `!`) goes in only if its
/// sender may still send it here, as when it was taken: the sender may have
/// been deleted, run lower or lost the user's setting meanwhile. Luna's own
/// (no sender) was checked when it was made. Dropped, not kept as
/// undelivered, which a resume would type in.
fn still_may_send(id: &str, q: &Queued) -> bool {
    let Some(from) = q.from.as_deref() else { return true };
    match crate::agents::command_reach_now(from, id, &q.text) {
        Ok(()) => true,
        Err(why) => {
            crate::log::warn("background", &format!("{id}: dropped queued text from {from}: {why}"));
            false
        }
    }
}

/// What is queued becomes the batch of the turn about to start, less any
/// text its sender may no longer send (`still_may_send`).
fn take_queue(id: &str, e: &mut Entry) -> String {
    e.queue.retain(|q| still_may_send(id, q));
    e.in_flight = std::mem::take(&mut e.queue);
    stamp(&mut e.in_flight);
    joined(&e.in_flight)
}

/// A permission or question held for an answer.
struct Held {
    serial: u64,
    ask: Ask,
    /// The tool input as the CLI sent it, returned with the answer.
    input: Value,
    answer: Option<Value>,
}

struct Entry {
    spec: Spec,
    /// When it was spawned.
    started_ms: u64,
    /// Claude Code's session id / Codex's thread id, once the first turn has
    /// said it.
    session: Option<String>,
    /// Where the CLI says it works: the worktree it made, if it made one.
    cwd: Option<String>,
    run: Option<Run>,
    turns_ended: u32,
    /// Text sent while a turn ran, for the turn after it.
    queue: Vec<Queued>,
    /// The batch a turn was claimed for, from then until the CLI has it: saved,
    /// so that a core killed in between neither forgets it nor, reading an older
    /// save, hands back what was processed. Claude Code has it when it says so
    /// (`took`), a pipe taking the bytes being no sign of that; Codex when its
    /// stdin has taken it, all there is to go by.
    in_flight: Vec<Queued>,
    /// Text that joined the running turn on stdin, until the CLI says it took
    /// it in. One killed before that has not kept it, whatever the pipe took.
    fed: Vec<Queued>,
    log: VecDeque<String>,
    /// What it is held on, oldest first: parallel subagents and tool calls ask
    /// at once, and the first still unanswered is the one shown.
    held: VecDeque<Held>,
    /// The CLI could not run or quit on an error before saying anything.
    error: Option<String>,
    /// Its last turn was stopped by the core going, not ended by the CLI:
    /// what the caller is told of it says so.
    cut_short: bool,
    /// Text that was queued behind a turn the core's going stopped: it never
    /// reached the CLI. Kept, across cores too, until a caller has been told
    /// (agents.rs `undelivered_shown`) or a turn of `resume` has taken it.
    undelivered: Vec<Queued>,
    /// The helpers whose budget lines its CLI's stdin has taken, until the agent
    /// layer has noted it: the one word that a line arrived (`stdin_took`).
    budgets_landed: Vec<String>,
    /// Its turn's processes are in a job tied to the core (procs.rs), so a core
    /// that dies takes them along. Saved: without it the next core cannot tell
    /// that a turn it did not stop is over.
    tied: bool,
    /// Taken up with nothing of the last core's turn left to overlap (`tied`, or
    /// that core stopped it itself): `resume` may start another.
    resumable: bool,
    /// Nothing says an earlier core's turn ended. Saved, or the next core would
    /// read the `cut_short` this core saves as that core having stopped it. Until
    /// its next turn, which its caller chose knowing, or a later look (`LastTies`).
    unsure: bool,
    /// What its latest turn was started on, for as long as no turn has named
    /// its conversation: with nothing on disk to resume, that is all there is
    /// of a helper a core's going stops now.
    opening: Option<String>,
    /// The id of the line that carried it, kept with it: taken up again it is the
    /// same message, not a copy of whatever else holds it (`lone_opening`).
    opening_id: Option<String>,
    /// The job every turn's process is put in, the first one's made for it:
    /// what a turn leaves running is still the helper's between turns, holds
    /// the machine as a chat's would, and is ended with the helper. Tied to
    /// the core: a helper has no terminal whose going would end its CLI, and
    /// one left running by a core that died would run alongside the turn the
    /// next core starts on the same conversation (procs.rs).
    job: Option<std::sync::Arc<crate::procs::Job>>,
    /// Its turns left something running in that job, when last looked: at the
    /// end of its turn and at every activity sample since. Reading a job takes
    /// a process snapshot, too much for every `turn_state`, and not under the
    /// registry lock.
    left_running: bool,
    /// What that is, process by process, as last sampled between turns: what
    /// `luna_list` names, and whether it does anything (`leftovers`).
    left: Vec<Leftover>,
    /// A stop between turns is ending what its turns left (`kill`): no turn
    /// starts meanwhile, as its CLI would land in the job being emptied; text
    /// sent queues, and starts the next turn once the stop is through.
    ending_left: bool,
    /// Being opened as a chat (`background_promote`), until the window says how
    /// its row came out: no turn starts meanwhile, as its conversation may be
    /// resumed in a terminal any moment; text sent queues, and goes with it.
    promoting: bool,
    observed: Observed,
}

struct Registry {
    entries: HashMap<String, Entry>,
    next_serial: u64,
    /// Which helper each ask this core has given out went to, answered or not:
    /// an `askId` that was never one is told apart from one that has gone.
    asked: HashMap<u64, String>,
    /// The core is leaving (`stop_all`): no turn starts from here on.
    closed: bool,
}

fn reg() -> &'static (Mutex<Registry>, Condvar) {
    static R: OnceLock<(Mutex<Registry>, Condvar)> = OnceLock::new();
    R.get_or_init(|| {
        (Mutex::new(Registry { entries: HashMap::new(), next_serial: 1, asked: HashMap::new(), closed: false }), Condvar::new())
    })
}

fn lock() -> std::sync::MutexGuard<'static, Registry> {
    reg().0.lock().unwrap_or_else(|e| e.into_inner())
}

/// A chat id like the window's own (chats.store `newId`), so a helper opened
/// as a chat later looks like any other.
pub fn mint_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let base36 = |mut n: u64| {
        let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
        let mut s = Vec::new();
        loop {
            s.push(digits[(n % 36) as usize]);
            n /= 36;
            if n == 0 {
                break;
            }
        }
        s.reverse();
        String::from_utf8(s).unwrap_or_default()
    };
    format!("c{}{}", base36(crate::throttle::now_ms()), base36(SEQ.fetch_add(1, Ordering::Relaxed) + 1000))
}

pub fn has(id: &str) -> bool {
    lock().entries.contains_key(id)
}

/// How many helpers are in a turn, or between turns have what their turns left
/// still at work (`left_holds`): what keeps the core, as a running session does,
/// for the core's going would end that work. Left idle past LEFTOVER_HOLD_MS it
/// keeps the core no more than it keeps the machine.
pub fn running() -> usize {
    let now = crate::throttle::now_ms();
    lock().entries.values().filter(|e| e.run.is_some() || leftovers_hold(e, now)).count()
}

/// Whether a helper runs on this account folder, between turns too: its next
/// turn starts there.
pub fn uses_account(account_path: &str) -> bool {
    lock().entries.values().any(|e| crate::pty::same_dir(&e.spec.account_path, account_path))
}

/// Every folder the helpers other than `except` work in, between turns too:
/// the one each was sent to, and the worktree it made once the CLI has said
/// where — Codex says it only in its rollout, read here off the registry.
pub fn working_dirs(except: Option<&str>) -> Vec<String> {
    let found: Vec<(String, Option<String>, Provider, String, Option<String>)> = lock()
        .entries
        .iter()
        .filter(|(id, _)| Some(id.as_str()) != except)
        .map(|(_, e)| (e.spec.folder.clone(), e.cwd.clone(), e.spec.provider, e.spec.account_path.clone(), e.session.clone()))
        .collect();
    found
        .into_iter()
        .flat_map(|(folder, cwd, provider, account, session)| {
            let cwd = cwd.or_else(|| match provider {
                Provider::Codex => crate::codex::session::thread_cwd(&account, session.as_deref()?),
                Provider::Claude => None,
            });
            [Some(folder), cwd].into_iter().flatten()
        })
        .collect()
}

/// The core is leaving: every turn in progress is stopped with its process
/// tree, and none starts after.
pub fn stop_all() {
    stop_turns(true);
    save();
    // A core that takes the helpers up again has no job of theirs to see
    // what is left by: it goes with this one, but for programs with a window
    // of their own, which the OS would end with the job if it were still tied.
    let jobs: Vec<(String, std::sync::Arc<crate::procs::Job>)> =
        lock().entries.iter().filter_map(|(id, e)| Some((id.clone(), e.job.clone()?))).collect();
    for (id, job) in jobs {
        end_leftovers(&id, Some(&job));
        crate::procs::let_go(&job);
    }
}

/// Ends what a helper's turns left running, once the helper is done with.
fn end_leftovers(id: &str, job: Option<&crate::procs::Job>) {
    let Some(job) = job else { return };
    let ended = crate::procs::end_leftovers(job, None);
    if !ended.is_empty() {
        crate::log::info("background", &format!("{}: ended what its turns left running: {}", crate::agents::label(id), ended.join(", ")));
    }
}

/// Stops every turn in progress with its process tree and takes what was
/// queued off it, kept as undelivered; `close` also keeps any from starting
/// after. A turn claimed but not yet started is stopped too: it does not
/// start. Returns how many it stopped.
pub fn stop_turns(close: bool) -> usize {
    let (pids, claimed): (Vec<(u32, crate::procs::Pin)>, usize) = {
        let mut r = lock();
        r.closed |= close;
        let mut pids = Vec::new();
        let mut claimed = 0;
        for e in r.entries.values_mut() {
            match stop_for_luna(e) {
                Some(pid) => pids.push((pid, crate::procs::pin(pid))),
                None if e.run.is_some() => claimed += 1,
                None => {}
            }
        }
        (pids, claimed)
    };
    reg().1.notify_all();
    let stopped = pids.len() + claimed;
    for (pid, _pin) in pids {
        crate::pty::kill_tree(Some(pid));
    }
    stopped
}

/// `stop`, for Luna's sake and not the agent's: not ended, as its caller hears,
/// and what was queued behind it, which `stop` drops, is kept for its sender.
fn stop_for_luna(e: &mut Entry) -> Option<u32> {
    e.cut_short |= e.run.is_some();
    let flying = std::mem::take(&mut e.in_flight);
    e.undelivered.extend(flying);
    let fed = std::mem::take(&mut e.fed);
    e.undelivered.extend(fed);
    let queued = e.queue.clone();
    e.undelivered.extend(queued);
    stop(e)
}

/// Takes a helper's turn off, under the lock: drops what was queued, counted
/// as a turn that ended (whoever queued it waits on its end, which will not
/// come now), lets a held question go, and marks a claimed turn with no
/// process yet stopped. What joined the turn goes with it, its senders waiting
/// on the end of this very turn. Returns the pid left to kill.
fn stop(e: &mut Entry) -> Option<u32> {
    e.in_flight.clear();
    e.fed.clear();
    if !e.queue.is_empty() {
        e.queue.clear();
        e.turns_ended += 1;
    }
    e.held.clear();
    let run = e.run.as_mut()?;
    run.feed = None;
    if run.pid.is_none() {
        run.stopped = true;
    }
    run.pid
}

/// Starts a helper on its opening prompt.
pub fn start(id: &str, spec: Spec, prompt: &str) -> Result<(), String> {
    let mut e = Entry {
        spec,
        started_ms: crate::throttle::now_ms(),
        session: None,
        cwd: None,
        run: None,
        turns_ended: 0,
        queue: Vec::new(),
        in_flight: Vec::new(),
        fed: Vec::new(),
        log: VecDeque::new(),
        held: VecDeque::new(),
        error: None,
        cut_short: false,
        undelivered: Vec::new(),
        budgets_landed: Vec::new(),
        tied: false,
        resumable: false,
        unsure: false,
        opening: None,
        opening_id: None,
        job: None,
        left_running: false,
        left: Vec::new(),
        ending_left: false,
        promoting: false,
        observed: Observed::default(),
    };
    claim(&mut e);
    lock().entries.insert(id.to_string(), e);
    if let Err(e) = run_turn(id, prompt) {
        lock().entries.remove(id);
        changed();
        return Err(e);
    }
    changed();
    Ok(())
}

/// The arguments and working directory of the next turn.
fn command_for(e: &Entry, id: &str) -> (Vec<String>, String, Vec<(&'static str, String)>) {
    let s = &e.spec;
    let mut args: Vec<String> = Vec::new();
    let mut envs: Vec<(&'static str, String)> = Vec::new();
    // A worktree the first turn made, while it is there; Claude Code may
    // drop a clean one on exit, and is then asked for it again by name.
    let made = e.cwd.clone().filter(|c| !crate::pty::same_dir(c, &s.folder));
    let mut dir = s.folder.clone();
    match s.provider {
        Provider::Claude => {
            args.extend(
                ["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose", "--replay-user-messages"]
                    .map(String::from),
            );
            if let Some(m) = &s.model {
                args.extend(["--model".into(), m.clone()]);
            }
            args.extend(["--effort".into(), s.effort.clone(), "--permission-mode".into(), s.mode.clone()]);
            match (&e.session, &made) {
                (Some(sid), Some(wt)) if std::path::Path::new(wt).is_dir() => {
                    dir = wt.clone();
                    args.extend(["--resume".into(), sid.clone()]);
                }
                (Some(sid), Some(wt)) => {
                    let name = std::path::Path::new(wt).file_name().map(|n| n.to_string_lossy().into_owned());
                    // By a name that will do, or it stays where it is: no worktree of
                    // any other name is the one this conversation was in.
                    if let Some(name) = name.filter(|n| crate::worktree::name_ok(n)) {
                        args.push(crate::worktree::worktree_arg(Some(&name)));
                    }
                    args.extend(["--resume".into(), sid.clone()]);
                }
                (Some(sid), None) => args.extend(["--resume".into(), sid.clone()]),
                (None, _) if s.worktree => args.push(crate::worktree::worktree_arg(s.worktree_name.as_deref())),
                (None, _) => {}
            }
            if let Some(agent) = &s.agent {
                args.extend(["--agent".into(), agent.clone()]);
            }
            let mut denied: Vec<&str> = if s.read_only { crate::pty::CLAUDE_EDIT_TOOLS.to_vec() } else { Vec::new() };
            if let Some(mcp) = s.token.as_deref().and_then(|t| crate::pty::claude_mcp_file(id, t)) {
                args.extend(["--mcp-config".into(), mcp.to_string_lossy().into_owned()]);
                args.extend(["--permission-prompt-tool".into(), "mcp__luna__luna_permission".into()]);
                // The CLI's to call, not the model's: hidden from its tools,
                // and the prompts still come through.
                denied.push("mcp__luna__luna_permission");
                if s.tools {
                    args.extend(["--allowedTools".into(), "mcp__luna".into()]);
                }
            }
            if !denied.is_empty() {
                args.extend(["--disallowedTools".into(), denied.join(",")]);
            }
            if let Some(parent) = &s.parent {
                args.extend(crate::pty::claude_helper_args(parent));
            }
            // A helper with tools has helpers of its own to hear from: what a
            // notice names is a file there, to read without asking.
            if s.tools {
                let notes = crate::agents::notes_dir();
                if std::fs::create_dir_all(&notes).is_ok() {
                    args.extend(["--add-dir".into(), notes.to_string_lossy().into_owned()]);
                }
            }
            // The skills the library gives sessions where it works and keeps
            // from them, on every turn: each is a process of its own
            // (library.rs `session_args`). No hooks: its stream says it all; but
            // the guard against endless waits, a helper's as any (guard.rs).
            let skills = crate::library::session_args(Provider::Claude, &s.account_path, &dir);
            if let Some(settings) = crate::pty::claude_settings_file(id, false, s.tools || s.parent.is_some(), &skills) {
                args.extend(["--settings".into(), settings.to_string_lossy().into_owned()]);
            }
            if let Some(dir) = crate::pty::claude_instructions_dir(id, skills.instructions.as_deref()) {
                args.extend(["--add-dir".into(), dir.to_string_lossy().into_owned()]);
                envs.push((crate::pty::CLAUDE_ADDED_DIR_MEMORY, "1".into()));
            }
            args.extend(skills.args);
            envs.push(("CLAUDE_CONFIG_DIR", s.account_path.clone()));
            envs.push(("DISABLE_AUTOUPDATER", "1".into()));
        }
        Provider::Codex => {
            args.push("exec".into());
            match &e.session {
                Some(thread) => {
                    args.extend(["resume".into(), thread.clone()]);
                    if let Some(wt) = made.filter(|w| std::path::Path::new(w).is_dir()) {
                        dir = wt;
                    }
                }
                None => {
                    args.extend(["-C".into(), s.folder.clone()]);
                    if s.worktree {
                        args.push("--worktree".into());
                    }
                }
            }
            args.extend(["--json".into(), "--skip-git-repo-check".into()]);
            // `exec` cannot ask for the folder's trust and goes without the
            // project's config and hooks if it lacks it; a helper is where it
            // was sent, trusted for the run (codex/trust.rs).
            args.extend(crate::codex::trust::trust_override(&s.folder));
            if let Some(m) = &s.model {
                args.extend(["--model".into(), m.clone()]);
            }
            args.extend(["-c".into(), format!("model_reasoning_effort=\"{}\"", s.effort)]);
            if s.hooks_unasked {
                args.push(crate::pty::HOOKS_UNASKED.into());
            }
            args.extend(crate::pty::codex_guard_args(id, s.tools || s.parent.is_some(), s.hooks_unasked));
            if s.auto_review {
                args.push("--approve-for-me".into());
            } else {
                // `exec` asks nobody: the sandbox is what holds it.
                args.extend(["-c".into(), "approval_policy=\"never\"".into()]);
                args.extend(["-c".into(), crate::pty::CODEX_PERSON_REVIEWS.into()]);
                args.extend(crate::pty::codex_sandbox_args(&s.mode));
            }
            if let Some(sb) = crate::pty::codex_windows_sandbox(&s.mode) {
                args.extend(["-c".into(), sb]);
            }
            if let Some(path) = crate::pty::codex_sandbox_path(&s.mode) {
                envs.push(("PATH", path));
            }
            if let Some(token) = s.token.clone().filter(|_| s.tools) {
                args.extend(crate::pty::codex_mcp_args());
                envs.push(("LUNA_MCP_TOKEN", token));
            }
            envs.extend(crate::pty::codex_git_envs(&s.mode));
            envs.push(("CODEX_HOME", s.account_path.clone()));
            args.extend(crate::library::session_args(Provider::Codex, &s.account_path, &dir).args);
            // The prompt comes on stdin; Claude Code reads it there unasked.
            args.push("-".into());
        }
    }
    envs.extend(crate::pty::session_envs(id, s.parent.as_deref()));
    (args, dir, envs)
}

/// Takes the helper's run for a turn about to start, under the same lock that
/// found it free: text that arrives before the process is up queues behind
/// it rather than starting a second process on the same conversation.
fn claim(e: &mut Entry) {
    e.run = Some(Run::claimed(None, crate::throttle::now_ms()));
}

/// Starts the next turn's process with `prompt` on its stdin, on a run
/// `claim` has taken; a turn that could not start gives the claim back,
/// counted as a turn that ended on the error: whoever queued it, or queued
/// behind it, counts on its end, and `turn_over` never comes for it. Text
/// queued behind it fails with it, counted as the turn after, so whoever
/// waits on it hears — rather than starting what would most likely fail the
/// same way.
fn run_turn(id: &str, prompt: &str) -> Result<(), String> {
    let started = spawn_turn(id, prompt);
    if let Err(err) = &started {
        if let Some(e) = lock().entries.get_mut(id) {
            if e.run.as_ref().is_some_and(|r| r.pid.is_none()) {
                e.run = None;
                note(e, &format!("could not start the turn: {err}"));
                e.error = Some(err.clone());
                e.turns_ended += 1;
                if !e.queue.is_empty() {
                    e.queue.clear();
                    note(e, "dropped what was queued behind it");
                    e.turns_ended += 1;
                }
            }
        }
        reg().1.notify_all();
        changed();
    }
    started
}

/// What a turn's CLI is given: `prompt`, which on every turn of a Codex helper
/// carries Luna's note (`pty::codex_helper_prompt`), naming its parent as it is
/// now. Claude Code's system prompt says it every turn; Codex has nowhere but
/// the conversation, where one note at its start goes with a compaction, and a
/// long helper would go on as the chat of a person. Not in `Entry::opening`,
/// which goes back to the caller as what it asked, nor in what `luna_read`
/// gives back (codex/session.rs `message_of`).
fn stdin_text(e: &Entry, prompt: &str) -> String {
    match (&e.spec.parent, e.spec.provider) {
        (Some(parent), Provider::Codex) => crate::pty::codex_helper_prompt(parent, prompt),
        _ => prompt.to_string(),
    }
}

/// A message as a line of Claude Code's stream-json stdin. The uuid is what its
/// replay of the line comes back under, and what its transcript keeps.
fn user_line(uuid: &str, text: &str) -> String {
    let line = json!({
        "type": "user",
        "message": { "role": "user", "content": text },
        "parent_tool_use_id": null,
        "session_id": "",
        "uuid": uuid,
    });
    format!("{line}\n")
}

/// Claude Code's stdin for a turn: `first`, then whatever comes down `lines`,
/// until they close, which is the end of its input. A thread of its own, as
/// nothing says the CLI is reading: a full pipe holds this, and no lock.
fn feed_stdin(mut stdin: impl Write, first: &str, lines: std::sync::mpsc::Receiver<String>) {
    let mut put = |line: &str| stdin.write_all(line.as_bytes()).and_then(|()| stdin.flush());
    if put(first).is_err() {
        return;
    }
    for line in lines {
        if put(&line).is_err() {
            return;
        }
    }
}

/// What a turn starts on, for as long as no turn has named its conversation.
fn note_opening(e: &mut Entry, prompt: &str, line: &str) {
    e.opening = e.session.is_none().then(|| prompt.to_string());
    e.opening_id = e.session.is_none().then(|| line.to_string());
}

/// It has passed to another parent, or with none stands alone (agents.rs
/// `pass_up`): what its next turn is told. One running now has its environment
/// and system prompt already.
pub fn set_parent(id: &str, parent: Option<String>) {
    if let Some(e) = lock().entries.get_mut(id) {
        e.spec.parent = parent;
    }
}

/// Why a turn claimed before a stop does not run.
const STOPPED: &str = "it was stopped before it started";
/// Once the CLI's process has gone, a reader blocked on its pipe this long has
/// read all it wrote: the pipe is empty, and what holds it open is something it
/// left running, which may do so for days.
const DRAINED_AFTER: std::time::Duration = std::time::Duration::from_millis(500);
/// The most a turn's end waits on a reader that is not there yet (a stalled
/// disk behind the registry lock, a job writing to the pipe without end).
const DRAIN_AT_MOST: std::time::Duration = std::time::Duration::from_secs(30);

/// How the reader of a turn's stdout stands, for the thread that sees the turn
/// out: the turn is over when the CLI is, not when the pipe closes, as a job it
/// left running can have inherited the pipe. What the CLI wrote before it went
/// is still to be read by then, and a blocked read is the only sign that all
/// of it has been.
#[derive(Default)]
struct OutReader {
    /// In a read that has not returned.
    reading: std::sync::atomic::AtomicBool,
    reads: std::sync::atomic::AtomicU64,
    /// The turn is over: what comes down the pipe from here is not the CLI's.
    over: std::sync::atomic::AtomicBool,
}

impl OutReader {
    /// Reads `out` to its end as bytes, handing each line that holds JSON to
    /// `on_event`, which says whether to go on.
    fn read_events(&self, out: impl std::io::Read, mut on_event: impl FnMut(&Value) -> bool) {
        self.read_lines(out, |line| serde_json::from_slice::<Value>(line).map_or(true, |v| on_event(&v)));
    }

    /// Reads `out` to its end as bytes, handing each line to `on_line`, which
    /// says whether to go on; the last one too, should it come with no line end.
    fn read_lines(&self, mut out: impl std::io::Read, mut on_line: impl FnMut(&[u8]) -> bool) {
        use std::sync::atomic::Ordering::SeqCst;
        let mut buf = vec![0u8; 64 * 1024];
        let mut line = Vec::new();
        loop {
            self.reading.store(true, SeqCst);
            let n = out.read(&mut buf).unwrap_or(0);
            self.reading.store(false, SeqCst);
            self.reads.fetch_add(1, SeqCst);
            if n == 0 {
                break;
            }
            for chunk in buf[..n].split_inclusive(|b| *b == b'\n') {
                line.extend_from_slice(chunk);
                if chunk.last() == Some(&b'\n') {
                    if !on_line(&line) {
                        return;
                    }
                    line.clear();
                }
            }
        }
        if !line.is_empty() {
            on_line(&line);
        }
    }

    /// Waits for what the CLI wrote before it went to have been read: for the
    /// reader to end (`read_out` closes with it), or to sit in a read with
    /// nothing coming, however long it took to get through the rest.
    fn await_last_words(&self, read_out: &std::sync::mpsc::Receiver<()>, drained_after: std::time::Duration, at_most: std::time::Duration) {
        use std::sync::atomic::Ordering::SeqCst;
        let start = std::time::Instant::now();
        let (mut reads, mut since) = (self.reads.load(SeqCst), start);
        while start.elapsed() < at_most {
            if !matches!(read_out.recv_timeout(std::time::Duration::from_millis(20)), Err(std::sync::mpsc::RecvTimeoutError::Timeout)) {
                return;
            }
            let now = self.reads.load(SeqCst);
            if now != reads {
                (reads, since) = (now, std::time::Instant::now());
            }
            if self.reading.load(SeqCst) && since.elapsed() >= drained_after {
                return;
            }
        }
    }
}

/// One event off a turn's stdout, into its helper. The flag is looked at under
/// the lock: `turn_over` takes it after the flag is up and the next turn starts
/// after that, so an event of this turn can land neither after its end nor in
/// the next one. Whether to go on reading: not once the helper is gone.
fn take_event(id: &str, provider: Provider, reader: &OutReader, v: &Value) -> bool {
    let mut r = lock();
    if reader.over.load(std::sync::atomic::Ordering::SeqCst) {
        return true;
    }
    let Some(e) = r.entries.get_mut(id) else { return false };
    e.observed.event_ms = crate::throttle::now_ms();
    let unnamed = e.session.is_none();
    let landed = match provider {
        Provider::Claude => on_claude_event(e, v),
        Provider::Codex => {
            on_codex_event(e, v);
            false
        }
    };
    let named = unnamed && e.session.is_some();
    drop(r);
    // At once: a core killed now would take it up as one that never got started,
    // and start its work over; or hand back as undelivered what the model has.
    if named || landed {
        save();
    }
    true
}

/// A line off a turn's stderr, into its helper's log, under the same flag as
/// `take_event`: what a process the turn left writes there after the turn is
/// over is neither the helper's word nor a sign of it at work. Read on to the
/// end all the same, so that nothing writing there blocks.
fn take_stderr(id: &str, reader: &OutReader, line: &[u8]) -> bool {
    let line = String::from_utf8_lossy(line);
    let line = line.trim();
    if line.is_empty() || line.starts_with("Reading additional input") {
        return true;
    }
    let mut r = lock();
    if reader.over.load(std::sync::atomic::Ordering::SeqCst) {
        return true;
    }
    if let Some(e) = r.entries.get_mut(id) {
        e.observed.event_ms = crate::throttle::now_ms();
        note(e, &format!("stderr: {}", one_line(line)));
    }
    true
}

fn spawn_turn(id: &str, prompt: &str) -> Result<(), String> {
    settle_worktree(id);
    // Before the registry is locked for the command line: a pass reads and
    // writes every account's copies, and every helper's events wait on that
    // lock (library.rs `session_args`).
    crate::library::lay_out("before a helper's turn");
    let (args, dir, envs, provider, on_stdin, line, account) = {
        let r = lock();
        if r.closed {
            return Err("Luna is stopping".into());
        }
        let e = r.entries.get(id).ok_or("no such helper")?;
        if e.run.as_ref().is_some_and(|run| run.stopped) {
            return Err(STOPPED.into());
        }
        let (args, dir, envs) = command_for(e, id);
        // A batch's own, on disk by now; what no batch carries needs only to be a uuid.
        let line = e.in_flight.first().map_or_else(new_id, |q| q.id.clone());
        (args, dir, envs, e.spec.provider, stdin_text(e, prompt), line, e.spec.account_path.clone())
    };
    if !std::path::Path::new(&dir).is_dir() {
        return Err(format!("{dir} does not exist"));
    }
    let mut cmd = std::process::Command::new(crate::cli::binary(provider));
    cmd.args(&args).current_dir(&dir);
    crate::pty::clean_env(&mut cmd);
    for (k, v) in &envs {
        cmd.env(k, v);
    }
    cmd.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    // Suspended until it is in the helper's job (`procs::resume`): what it
    // starts at once would otherwise be born outside it.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000 | 0x0000_0004); // CREATE_NO_WINDOW | CREATE_SUSPENDED
    }
    // A group of its own, as a pty gives its CLI: what kills a turn
    // (`pty::kill_tree`) takes that group with the rest of the tree. The mark
    // goes with what leaves it, for a core's death (keeper.rs).
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
        cmd.env(crate::keeper::MARK, crate::keeper::mark_of(id));
    }
    let thread_err = |e: std::io::Error| format!("could not start {}: {e}", provider.as_str());
    // The thread that sees the turn out is there before the process: one
    // that could not be made after it would leave the process running with
    // nobody to reap it or end its turn. It is handed the process once the
    // turn is recorded, and nothing if the turn does not start.
    type Pipes = (std::process::Child, Option<std::process::ChildStdout>, Option<std::process::ChildStderr>, crate::account_use::Running);
    let (hand, take) = std::sync::mpsc::channel::<Pipes>();
    {
        let id = id.to_string();
        std::thread::Builder::new()
            .name(format!("luna-bg-{id}"))
            .spawn(move || {
                let Ok((mut child, stdout, stderr, _on_account)) = take.recv() else { return };
                // Each pipe on a thread of its own (see `OutReader`). Both are
                // read to their end as bytes: `lines()` stops at the first line
                // that is not UTF-8 — a hook's output in the console's code page
                // — and a pipe nobody drains any more blocks the CLI once it fills.
                let reader = std::sync::Arc::new(OutReader::default());
                let (drained, read_out) = std::sync::mpsc::channel::<()>();
                if let Some(out) = stdout {
                    let (helper, on_it) = (id.clone(), reader.clone());
                    let read = std::thread::Builder::new().name(format!("luna-bg-out-{id}")).spawn(move || {
                        let _drained = drained;
                        on_it.read_events(out, |v| take_event(&helper, provider, &on_it, v));
                    });
                    if let Err(e) = read {
                        crate::log::warn("background", &format!("cannot read what {} says: {e}", crate::agents::label(&id)));
                    }
                }
                let err_reader = std::sync::Arc::new(OutReader::default());
                let (err_drained, err_out) = std::sync::mpsc::channel::<()>();
                if let Some(err) = stderr {
                    let (helper, on_it) = (id.clone(), err_reader.clone());
                    let read = std::thread::Builder::new().name(format!("luna-bg-err-{id}")).spawn(move || {
                        let _drained = err_drained;
                        on_it.read_lines(err, |line| take_stderr(&helper, &on_it, line));
                    });
                    if let Err(e) = read {
                        crate::log::warn("background", &format!("cannot read what {} says on stderr: {e}", crate::agents::label(&id)));
                    }
                }
                let code = child.wait().ok().and_then(|s| s.code());
                reader.await_last_words(&read_out, DRAINED_AFTER, DRAIN_AT_MOST);
                err_reader.await_last_words(&err_out, DRAINED_AFTER, DRAIN_AT_MOST);
                reader.over.store(true, std::sync::atomic::Ordering::SeqCst);
                err_reader.over.store(true, std::sync::atomic::Ordering::SeqCst);
                turn_over(&id, code);
            })
            .map_err(thread_err)?;
    }
    // Held by the thread above until it has reaped the process; dropped here
    // on the ways out where the turn does not start.
    let on_account = crate::account_use::running_on(&account);
    // Ahead of the process: what it starts is born after this (`turn_over`).
    let started_ms = crate::throttle::now_ms();
    let mut child = cmd.spawn().map_err(|e| {
        crate::log::error("background", &format!("could not start {}: {e}", crate::agents::label(id)));
        format!("could not start {}: {e}", provider.as_str())
    })?;
    let pid = Some(child.id());
    // Into the helper's job before it has had time to start anything. What is
    // waited for here is not waited for with every helper's lock held.
    if let Some(pid) = pid {
        crate::procs::await_environment(pid);
        if let Some(e) = lock().entries.get_mut(id) {
            match &e.job {
                Some(job) => {
                    let joined = crate::procs::join(job, pid);
                    e.tied = e.tied && joined;
                }
                None => {
                    e.job = crate::procs::adopt(pid).map(std::sync::Arc::new);
                    e.tied = e.job.as_deref().is_some_and(|job| crate::procs::tie(job, id));
                }
            }
            mark_by_thread(e);
        }
        #[cfg(windows)]
        if !crate::procs::resume(pid) {
            crate::log::error("background", &format!("could not let {} run once in its job", crate::agents::label(id)));
            abandon(child, Some(pid));
            return Err(format!("could not start {}: its process would not resume", provider.as_str()));
        }
    }
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let (feed, lines) = std::sync::mpsc::channel::<String>();
    if let Some(mut stdin) = stdin {
        let prompt = on_stdin;
        let writer_id = id.to_string();
        let writer = std::thread::Builder::new();
        let spawned = match provider {
            // Open until its first `result` (`on_claude_event`), for what joins the turn.
            Provider::Claude => {
                let first = user_line(&line, &prompt);
                writer.spawn(move || feed_stdin(stdin, &first, lines))
            }
            // Written whole and closed: the CLI reads its prompt to the end of input.
            Provider::Codex => writer.spawn(move || {
                let wrote = stdin.write_all(prompt.as_bytes()).is_ok();
                drop(stdin);
                let landed = wrote && lock().entries.get_mut(&writer_id).is_some_and(stdin_took);
                if landed {
                    save();
                }
            }),
        };
        if let Err(e) = spawned {
            abandon(child, pid);
            return Err(thread_err(e));
        }
    }
    {
        let mut r = lock();
        let closed = r.closed;
        // A stop that came while the process was starting found no pid to
        // kill: the process goes here instead.
        let refused = match r.entries.get_mut(id) {
            _ if closed => Some("Luna is stopping"),
            None => Some("the helper was deleted"),
            Some(e) if e.run.as_ref().is_some_and(|run| run.stopped) => Some(STOPPED),
            Some(e) => {
                let claude = provider == Provider::Claude;
                let opening_line = line.clone();
                e.run = Some(Run { feed: claude.then_some(feed), line: claude.then_some(line), ..Run::claimed(pid, started_ms) });
                e.observed.begin_turn();
                e.error = None;
                e.cut_short = false;
                e.unsure = false;
                note_opening(e, prompt, &opening_line);
                note(e, &format!("› {}", one_line(prompt)));
                None
            }
        };
        if let Some(why) = refused {
            drop(r);
            abandon(child, pid);
            return Err(why.into());
        }
    }
    crate::log::info(
        "background",
        &format!("{} turn started, pid {pid:?}: {} {}", crate::agents::label(id), provider.as_str(), args.join(" ")),
    );
    // The reader waits on the other end; should it be gone, the turn still
    // ends here rather than with nobody to say so.
    if let Err(std::sync::mpsc::SendError((child, _, _, _))) = hand.send((child, stdout, stderr, on_account)) {
        abandon(child, pid);
        turn_over(id, None);
    }
    Ok(())
}

/// Codex's tools have this whatever its config keeps from them (`procs::also_marked`).
fn mark_by_thread(e: &Entry) {
    if let (Provider::Codex, Some(job), Some(thread)) = (e.spec.provider, e.job.as_deref(), e.session.as_deref()) {
        crate::procs::also_marked(job, "CODEX_THREAD_ID", thread);
    }
}

/// Kills a turn's process that is not to run, its tree with it, and reaps it.
fn abandon(mut child: std::process::Child, pid: Option<u32>) {
    crate::pty::kill_tree(pid);
    let _ = child.kill();
    let _ = child.wait();
}

/// Claude Code has taken the line `uuid` in: what it carried is the model's now,
/// wherever it was kept. Its word can come after a stop has put the text by.
/// Whether it carried anything.
fn took(e: &mut Entry, uuid: &str) -> bool {
    let mut landed = Vec::new();
    for store in [&mut e.in_flight, &mut e.fed, &mut e.undelivered] {
        let (of_line, rest): (Vec<Queued>, Vec<Queued>) = std::mem::take(store).into_iter().partition(|q| q.line.as_deref() == Some(uuid));
        *store = rest;
        landed.extend(of_line);
    }
    e.budgets_landed.extend(landed.iter().flat_map(|q| q.budget.iter().map(|m| m.id.clone())));
    for q in &landed {
        note(e, &format!("took in: {}", one_line(&q.text)));
    }
    !landed.is_empty()
}

/// A background helper has nobody to answer the CLI's own requests on stdin (an
/// MCP server's elicitation comes this way with stdin open), which would hold
/// its turn for good. An error reads as the cancel it gets with stdin closed.
fn refuse_request(e: &mut Entry, v: &Value) {
    let Some(request) = v["request_id"].as_str() else { return };
    let line = json!({
        "type": "control_response",
        "response": { "subtype": "error", "request_id": request, "error": "a background helper has nobody to ask" },
    });
    let Some(run) = e.run.as_ref() else { return };
    let refused = run.feed.as_ref().is_some_and(|feed| feed.send(format!("{line}\n")).is_ok());
    if refused {
        note(e, &format!("refused the CLI's request: {}", v["request"]["subtype"].as_str().unwrap_or("?")));
    }
}

/// What an event of a Claude Code turn says of the helper; whether a message of
/// its has landed, which is to be saved.
fn on_claude_event(e: &mut Entry, v: &Value) -> bool {
    match (v["type"].as_str(), v["state"].as_str()) {
        (Some("user"), _) if v["isReplay"] == true => return v["uuid"].as_str().is_some_and(|u| took(e, u)),
        (Some("command_lifecycle"), Some("started")) => return v["command_uuid"].as_str().is_some_and(|u| took(e, u)),
        // A line the CLI will not run gets no `result`: one its transcript has
        // already is passed over, and a stdin left open would hold it for good.
        // lang-check: allow
        (Some("command_lifecycle"), Some("completed" | "cancelled")) => {
            if let Some(run) = e.run.as_mut().filter(|run| run.line.is_some() && run.line.as_deref() == v["command_uuid"].as_str()) {
                run.feed = None;
            }
            return false;
        }
        (Some("control_request"), _) => {
            refuse_request(e, v);
            return false;
        }
        _ => {}
    }
    if v["type"] == "assistant" {
        // The CLI's own stand-in for a message no model wrote reads `<synthetic>`.
        if v["message"]["model"].as_str().is_some_and(|m| !m.starts_with('<')) {
            e.observed.acted = true;
        }
    }
    match (v["type"].as_str(), v["subtype"].as_str()) {
        (Some("system"), Some("init")) => {
            e.session = v["session_id"].as_str().map(str::to_owned).or(e.session.take());
            // Where its first turn started, or the worktree it made: the init of a
            // turn the CLI takes up by itself (a background task's end) says where
            // its shell has since gone with `cd`, which is not where it works.
            if e.cwd.is_none() {
                e.cwd = v["cwd"].as_str().map(str::to_owned);
            }
            // Its `model` is only what it was asked for, resolved: a model that
            // does not exist reads there too, and the turn fails on it.
        }
        (Some("assistant"), _) if v["parent_tool_use_id"].is_null() => {
            // The model that wrote it: a fallback mid-turn shows here too.
            if let Some(m) = v["message"]["model"].as_str().filter(|m| !m.starts_with('<')) {
                e.observed.model = Some(m.to_string());
            }
            for b in v["message"]["content"].as_array().into_iter().flatten() {
                match b["type"].as_str() {
                    Some("text") => note(e, &format!("said: {}", one_line(b["text"].as_str().unwrap_or("")))),
                    Some("tool_use") => {
                        let a = Ask::from_claude_tool(b["name"].as_str().unwrap_or("?"), &b["input"]);
                        note(e, &format!("{}: {}", a.tool.unwrap_or_default(), one_line(&a.detail.unwrap_or_default())));
                    }
                    _ => {}
                }
            }
        }
        (Some("result"), sub) => {
            let ok = sub == Some("success") && v["is_error"].as_bool() != Some(true);
            let secs = v["duration_ms"].as_f64().unwrap_or(0.0) / 1000.0;
            // From here on text waits for the next process: stdin's end lets
            // this one finish what it has and go, as it would have with stdin
            // closed all along. What it has been given it still runs.
            let earlier_failed = e.run.as_mut().is_some_and(|run| {
                run.feed = None;
                std::mem::replace(&mut run.result_failed, !ok)
            });
            if ok {
                note(e, &format!("turn done in {secs:.0}s"));
                if earlier_failed {
                    e.error = None;
                }
            } else {
                let why = v["result"].as_str().or(sub).unwrap_or("error").to_string();
                note(e, &format!("turn failed: {}", one_line(&why)));
                e.error = Some(why);
            }
        }
        _ => {}
    }
    false
}

/// The model Codex says it moved a turn to, from the one error item that is no
/// error: `model rerouted: <from> -> <to> (<reason>)`.
fn rerouted_to(message: &str) -> Option<&str> {
    let (_, moved) = message.split_once("model rerouted: ")?;
    let (_, to) = moved.split_once(" -> ")?;
    let to = to.split(" (").next()?.trim();
    (!to.is_empty()).then_some(to)
}

fn on_codex_event(e: &mut Entry, v: &Value) {
    let item = &v["item"];
    if v["type"].as_str().is_some_and(|t| t.starts_with("item.")) && item["type"] != "error" {
        e.observed.acted = true;
    }
    match v["type"].as_str() {
        Some("thread.started") => {
            e.session = v["thread_id"].as_str().map(str::to_owned).or(e.session.take());
            mark_by_thread(e);
        }
        Some("item.completed") => match item["type"].as_str() {
            Some("agent_message") => note(e, &format!("said: {}", one_line(item["text"].as_str().unwrap_or("")))),
            Some("command_execution") => note(
                e,
                &format!(
                    "ran: {} → {}",
                    one_line(item["command"].as_str().unwrap_or("")),
                    item["exit_code"].as_i64().map(|c| c.to_string()).unwrap_or_else(|| "?".into())
                ),
            ),
            Some("error") if item["message"].as_str().and_then(rerouted_to).is_some() => {
                let message = item["message"].as_str().unwrap_or("");
                e.observed.model = rerouted_to(message).map(str::to_owned);
                e.observed.rerouted = true;
                note(e, &one_line(message));
            }
            Some(other) if other != "reasoning" => note(e, other),
            _ => {}
        },
        Some("turn.failed") | Some("error") => {
            let why = v["error"]["message"].as_str().or(v["message"].as_str()).unwrap_or("error").to_string();
            note(e, &format!("error: {}", one_line(&why)));
            // Codex says its retries the same way ("Reconnecting... 2/5"): a
            // turn that gets through after them has not failed.
            if !why.starts_with("Reconnecting") {
                e.error = Some(why);
            }
        }
        Some("turn.completed") => {
            note(e, "turn done");
            e.error = None;
        }
        _ => {}
    }
}

/// The process of a Claude Code turn has gone and all it said has been read:
/// what it never took in, it never will. What joined its turn gets one more try,
/// as the start of the next and ahead of what was queued behind it, which came
/// later; returned, for its senders. A batch it was started on and never took
/// goes back to who sent it, with anything that joined behind it: a CLI that went
/// before taking even that would most likely do the same again.
fn sort_out_untaken(e: &mut Entry) -> Vec<Queued> {
    if !e.in_flight.is_empty() {
        let batch = std::mem::take(&mut e.in_flight);
        e.undelivered.extend(batch);
        let fed = std::mem::take(&mut e.fed);
        e.undelivered.extend(fed);
        note(e, "its CLI went before taking in what it was started on");
        return Vec::new();
    }
    let retry = std::mem::take(&mut e.fed);
    if !retry.is_empty() {
        note(e, "its CLI went before taking in what joined the turn: it starts the next");
        let behind = std::mem::take(&mut e.queue);
        e.queue = [retry.clone(), behind].concat();
    }
    retry
}

/// The turn's process has exited: count the turn, let any held question go,
/// and start the turn the queue has waiting.
fn turn_over(id: &str, code: Option<i32>) {
    let (job, strays) = match lock().entries.get(id) {
        Some(e) => (e.job.clone(), e.run.as_ref().filter(|_| !e.observed.acted).and_then(|run| Some((run.pid?, run.started_ms)))),
        None => (None, None),
    };
    // A CLI that went before its model did anything (an unknown `--agent`, a
    // model that is not there) leaves what it started of its own, a `git` it
    // asked where it was: no job of anyone's, and no reason to call the helper
    // busy or keep it from going. Only what is its own by descent: what earlier
    // turns left, and what they started since, stay.
    if let (Some(job), Some((cli, since))) = (&job, strays) {
        let ended = crate::procs::end_tree_of(job, cli, since);
        if !ended.is_empty() {
            crate::log::info(
                "background",
                &format!(
                    "{}: its CLI went before its model did anything; ended what it had started: {}",
                    crate::agents::label(id),
                    ended.join(", ")
                ),
            );
        }
    }
    // Before the turn reads as over: whoever is owed its answer must not hear
    // "finished" ahead of the job it left running.
    let left = job.map(|job| crate::procs::left_in(&job)).unwrap_or_default();
    let (next, thread, generation, rerouted, acted, retried, count) = {
        let mut r = lock();
        let Some(e) = r.entries.get_mut(id) else { return };
        e.run = None;
        e.turns_ended += 1;
        e.observed.turn_ended_ms = crate::throttle::now_ms();
        e.held.clear();
        e.left_running = !left.is_empty();
        // What was already there before the turn keeps its own clock; what is
        // new is as quiet as the turn's end.
        let now = crate::throttle::now_ms();
        e.left = leftovers(&e.left, left, now, now);
        if code != Some(0) && e.error.is_none() {
            e.error = Some(match code {
                Some(c) => format!("{} exited with code {c}", e.spec.provider.as_str()),
                // A signal on Unix: nothing says which but that it was not its own doing.
                None => format!("{} was killed", e.spec.provider.as_str()),
            });
        }
        // Codex says where it works only in its rollout.
        if e.spec.provider == Provider::Codex && e.cwd.is_none() {
            if let Some(thread) = &e.session {
                e.cwd = crate::codex::session::thread_cwd(&e.spec.account_path, thread);
            }
        }
        let retried = match e.spec.provider {
            Provider::Claude => sort_out_untaken(e),
            Provider::Codex => {
                e.in_flight.clear();
                Vec::new()
            }
        };
        let next = (!e.queue.is_empty()).then(|| take_queue(id, e));
        if next.is_some() {
            claim(e);
        }
        let thread = e.session.clone().filter(|_| e.spec.provider == Provider::Codex).map(|t| (e.spec.account_path.clone(), t));
        e.observed.answered |= e.observed.acted;
        (next, thread, e.observed.generation, std::mem::take(&mut e.observed.rerouted), e.observed.acted, retried, e.turns_ended)
    };
    // Ahead of anything that could be read as the answer: the helper reads as
    // busy from the claim above until this has moved what its senders wait on.
    let senders: Vec<String> = retried.iter().filter_map(|q| q.from.clone()).collect();
    crate::agents::retried(id, &senders, count);
    reg().1.notify_all();
    crate::log::info("background", &format!("{} turn over, code {code:?}", crate::agents::label(id)));
    // Codex's events name no model but a reroute's, which stands for its turn;
    // otherwise its rollout says what the turn ran on, read with the lock let go.
    // Not of a turn the model never got to: the rollout names the model it was
    // to run on before asking for it, one that is not there too.
    let thread = thread.filter(|_| acted && !rerouted);
    if let Some(rollout) = thread.and_then(|(account, t)| crate::codex::session::rollout_of_thread(&account, &t)) {
        let model = crate::codex::session::last_model(&rollout);
        if let Some(e) = lock().entries.get_mut(id) {
            e.observed.apply_rollout(generation, model);
        }
    }
    // One that cannot start is recorded as ended by `run_turn`. Stopped for
    // the core's sake as it started, its text is kept as if still queued.
    if let Some(text) = next {
        // On disk before the CLI has it (`Entry::in_flight`).
        save();
        if run_turn(id, &text).is_err() {
            if let Some(e) = lock().entries.get_mut(id) {
                let batch = std::mem::take(&mut e.in_flight);
                if e.cut_short || !retried.is_empty() {
                    e.undelivered.extend(batch);
                }
            }
        }
    }
    changed();
}

fn note(e: &mut Entry, line: &str) {
    if e.log.len() >= LOG_LINES {
        e.log.pop_front();
    }
    e.log.push_back(line.to_string());
}

pub fn one_line(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= LOG_CHARS {
        return flat;
    }
    format!("{}…", flat.chars().take(LOG_CHARS).collect::<String>())
}

/// The helper's turn as the agent layer counts it: busy while a turn's
/// process runs, waiting while a question or permission is held.
pub fn turn_state(id: &str) -> Option<crate::activity::TurnState> {
    use crate::activity::Turn;
    let r = lock();
    let e = r.entries.get(id)?;
    let turn = match &e.run {
        Some(_) if !e.held.is_empty() => Turn::Waiting,
        Some(_) => Turn::Busy,
        None => Turn::Idle,
    };
    Some(crate::activity::TurnState {
        turn,
        busy: e.run.is_some(),
        turns_ended: e.turns_ended,
        children_busy: e.run.is_none() && e.left_running,
    })
}

/// Whether a helper's turn keeps the machine awake: one held on a question or
/// permission waits for whoever answers it, which blocks a shutdown but keeps
/// nothing awake.
fn turn_holds(t: &crate::activity::TurnState) -> bool {
    t.busy && t.turn != crate::activity::Turn::Waiting
}

/// What the power rules see of every helper: its turn, and between turns what
/// its turns left running, which holds the machine until it has done nothing
/// for LEFTOVER_HOLD_MS on end (`Leftover::holds`). Noted for `turn_state`,
/// which is what tells its caller and `reap` that a job still runs, however idle.
pub fn activity() -> Vec<crate::activity::SessionActivity> {
    let ids: Vec<(String, Option<std::sync::Arc<crate::procs::Job>>)> =
        lock().entries.iter().map(|(id, e)| (id.clone(), e.job.clone().filter(|_| e.run.is_none()))).collect();
    let mut moved = false;
    let out = ids
        .into_iter()
        .filter_map(|(id, job)| {
            let read = job.map(|job| crate::procs::left_in(&job));
            let now = crate::throttle::now_ms();
            let mut at_work = Vec::new();
            let mut gone_quiet = Vec::new();
            if let Some(read) = read {
                if let Some(e) = lock().entries.get_mut(&id).filter(|e| e.run.is_none()) {
                    let pids = |left: &[Leftover]| left.iter().map(|l| (l.pid, l.started_ms)).collect::<Vec<_>>();
                    let before = pids(&e.left);
                    e.left_running = !read.is_empty();
                    let left = leftovers(&e.left, read, now, e.observed.turn_ended_ms);
                    gone_quiet = crate::procs::gone_quiet(&e.left, &left, now);
                    e.left = left;
                    moved |= pids(&e.left) != before;
                    at_work = e.left.iter().filter(|l| l.holds(now)).map(Leftover::label).collect();
                }
            }
            if !gone_quiet.is_empty() {
                crate::log::warn(
                    "power",
                    &format!(
                        "{}: no longer holds the PC by what its turns left, idle {} min on end: {}",
                        crate::agents::label(&id),
                        crate::procs::LEFTOVER_HOLD_MS / 60_000,
                        gone_quiet.join(", ")
                    ),
                );
            }
            let t = turn_state(&id)?;
            let mut a = crate::activity::SessionActivity::of_turn(&id, t.turn, turn_holds(&t));
            if !at_work.is_empty() {
                at_work.sort();
                at_work.dedup();
                a.busy = true;
                a.procs = at_work;
            }
            Some(a)
        })
        .collect();
    if moved {
        changed();
    }
    out
}

/// The turn has what `in_flight` holds: not to be handed back as undelivered, and
/// the budget lines in it have arrived. Whether there was any.
fn stdin_took(e: &mut Entry) -> bool {
    let batch = std::mem::take(&mut e.in_flight);
    e.budgets_landed.extend(batch.iter().flat_map(|q| q.budget.iter().map(|m| m.id.clone())));
    !batch.is_empty()
}

/// Where text sent to a helper went.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Placed {
    /// It starts a turn.
    Started,
    /// It waits for the turn after the one running.
    Queued,
    /// It is on the stdin of the turn running, which ends once that has answered
    /// it (Claude Code, at its next tool boundary).
    Joined,
}

/// Onto the stdin of the turn running, if that still takes text and nothing
/// waits ahead of it: what came first goes first. `q` is back if it cannot go.
// The error is the caller's own message handed back whole, to be queued
// instead; boxing it would only move it to the heap and out again.
#[allow(clippy::result_large_err)]
fn join(e: &mut Entry, mut q: Queued) -> Result<(), Queued> {
    if !e.queue.is_empty() {
        return Err(q);
    }
    let Some(feed) = e.run.as_ref().and_then(|run| run.feed.as_ref()) else { return Err(q) };
    q.line = Some(q.id.clone());
    if feed.send(user_line(&q.id, &q.text)).is_err() {
        q.line = None;
        return Err(q);
    }
    note(e, &format!("joined the turn: {}", one_line(&q.text)));
    e.fed.push(q);
    Ok(())
}

/// Puts text where it goes: on the turn running if it may `join` that, behind it,
/// or for the turn now claimed. One with budget lines is `in_flight` as queued text
/// is by then, so they count as arrived when the CLI has it, not before.
fn place(e: &mut Entry, q: Queued, may_join: bool) -> Placed {
    if e.run.is_some() || e.ending_left || e.promoting {
        let q = if may_join {
            match join(e, q) {
                Ok(()) => return Placed::Joined,
                Err(q) => q,
            }
        } else {
            q
        };
        note(e, &format!("queued: {}", one_line(&q.text)));
        e.queue.push(q);
        return Placed::Queued;
    }
    claim(e);
    if !q.budget.is_empty() {
        e.in_flight.push(q);
        stamp(&mut e.in_flight);
    }
    Placed::Started
}

/// Text for the helper: the next turn now, into the one running, or after it.
pub fn send(id: &str, text: &str, from: Option<&str>) -> Result<Placed, String> {
    send_as(id, Queued::new(from.map(str::to_owned), text.to_string(), Vec::new()), true)
}

/// A notice with budget lines in it, or none. It was aimed at a helper found
/// idle, and if a turn has begun since, it waits for that turn's end: what it
/// reports is what the next turn is to act on, and it starts one by itself.
/// Whether it was queued.
pub fn send_marked(id: &str, text: &str, from: Option<&str>, budget: Vec<BudgetMark>) -> Result<bool, String> {
    send_as(id, Queued::new(from.map(str::to_owned), text.to_string(), budget), false).map(|placed| placed != Placed::Started)
}

fn send_as(id: &str, q: Queued, may_join: bool) -> Result<Placed, String> {
    let marked = !q.budget.is_empty();
    let text = q.text.clone();
    let placed = {
        let mut r = lock();
        let e = r.entries.get_mut(id).ok_or("no such helper")?;
        if !e.held.is_empty() {
            return Err(format!("helper {id} is waiting on a question or permission: answer it with luna_answer"));
        }
        place(e, q, may_join)
    };
    if placed != Placed::Started || marked {
        // At once: a core that goes hard (power, a crash) writes nothing more.
        save();
    }
    if placed == Placed::Started {
        if let Err(err) = run_turn(id, &text) {
            if let Some(e) = lock().entries.get_mut(id) {
                e.in_flight.retain(|q| q.budget.is_empty());
            }
            return Err(err);
        }
    }
    changed();
    Ok(placed)
}

/// Where a helper's budget line stands with a background parent.
#[derive(Debug, PartialEq)]
pub enum MarkStands {
    /// In none of what it holds: never given, or dropped with the rest of a queue.
    Nothing,
    /// Queued, or with a turn starting on it.
    Coming,
    /// Kept undelivered: it will not reach the parent by itself.
    Undelivered,
    /// Its CLI's stdin took it.
    Landed,
}

pub fn budget_mark(parent: &str, helper: &str) -> MarkStands {
    let r = lock();
    let Some(e) = r.entries.get(parent) else { return MarkStands::Nothing };
    let holds = |batch: &[Queued]| batch.iter().any(|q| q.budget.iter().any(|m| m.id == helper));
    if e.budgets_landed.iter().any(|h| h == helper) {
        MarkStands::Landed
    } else if holds(&e.queue) || holds(&e.in_flight) {
        MarkStands::Coming
    } else if holds(&e.undelivered) {
        MarkStands::Undelivered
    } else {
        MarkStands::Nothing
    }
}

/// The agent layer has noted that the line landed.
pub fn budget_mark_noted(parent: &str, helper: &str) {
    if let Some(e) = lock().entries.get_mut(parent) {
        e.budgets_landed.retain(|h| h != helper);
    }
}

/// Takes a helper's line out of what is kept undelivered, as its next notice has it,
/// and with it any message left with nothing but `opening`, the words notices open on.
pub fn drop_undelivered_mark(parent: &str, helper: &str, opening: &str) {
    let mut r = lock();
    let Some(e) = r.entries.get_mut(parent) else { return };
    for q in e.undelivered.iter_mut() {
        let (gone, kept): (Vec<BudgetMark>, Vec<BudgetMark>) = std::mem::take(&mut q.budget).into_iter().partition(|m| m.id == helper);
        q.budget = kept;
        for m in gone {
            q.text = q.text.replace(&m.line, "");
        }
    }
    e.undelivered.retain(|q| {
        let left = q.text.trim();
        !left.is_empty() && left != opening
    });
}

/// Stops the turn in progress; the conversation stays for the next one. Between
/// turns there is no turn to stop, and what its turns left running is what is
/// left of it to stop: that is ended, and what went is returned by name.
pub fn kill(id: &str) -> Vec<String> {
    let (pid, job, _pin) = {
        let mut r = lock();
        let Some(e) = r.entries.get_mut(id) else { return Vec::new() };
        // Whoever is owed the turn hears it was stopped, not an exit code.
        if e.run.is_some() {
            e.error = Some("it was stopped, by luna_kill or from the window".into());
        }
        let job = e.job.clone().filter(|_| e.run.is_none() && e.left_running && !e.ending_left);
        // Only the Stop that takes the ending on sets the mark, and only it
        // lets it go: a second one meanwhile would let a send start a turn in
        // the job the first is still clearing.
        if job.is_some() {
            e.ending_left = true;
        }
        let pid = stop(e);
        // Under the lock, while the turn's own handle still holds the pid: a
        // turn that ends a moment later then leaves it to nobody else.
        (pid, job, pid.map(crate::procs::pin))
    };
    reg().1.notify_all();
    crate::pty::kill_tree(pid);
    let Some(job) = job else { return Vec::new() };
    let ended = crate::procs::end_leftovers(&job, None);
    // A process ended is still in the job for a moment, as the OS tears it down.
    let mut still = crate::procs::left_in(&job);
    for _ in 0..20 {
        if still.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        still = crate::procs::left_in(&job);
    }
    let next = {
        let mut r = lock();
        let Some(e) = r.entries.get_mut(id) else { return ended };
        e.ending_left = false;
        e.left_running = !still.is_empty();
        e.left = leftovers(&e.left, still, crate::throttle::now_ms(), e.observed.turn_ended_ms);
        // What was sent meanwhile is the next turn's, as after any turn.
        (e.run.is_none() && !e.promoting && !e.queue.is_empty()).then(|| {
            claim(e);
            take_queue(id, e)
        })
    };
    if !ended.is_empty() {
        crate::log::info(
            "background",
            &format!("{}: stopped between turns, which ended what its turns left running: {}", crate::agents::label(id), ended.join(", ")),
        );
    }
    if let Some(text) = next {
        save();
        if run_turn(id, &text).is_err() {
            if let Some(e) = lock().entries.get_mut(id) {
                e.in_flight.clear();
            }
        }
    }
    changed();
    ended
}

/// Whether anything its turns left still holds the machine (`Leftover::holds`):
/// once none does, an idle helper with only that left goes as any idle one does.
pub fn left_holds(id: &str) -> bool {
    let now = crate::throttle::now_ms();
    lock().entries.get(id).is_some_and(|e| leftovers_hold(e, now))
}

fn leftovers_hold(e: &Entry, now: u64) -> bool {
    e.left_running && (e.left.is_empty() || e.left.iter().any(|l| l.holds(now)))
}

/// Between turns, what its turns left running, for `luna_list`.
fn left_shown(e: &Entry, now: u64) -> Vec<LeftRunning> {
    if e.run.is_some() {
        return Vec::new();
    }
    shown(&e.left, now)
}

/// Leftovers as `luna_list` shows them, a helper's or a chat's.
pub fn shown(left: &[Leftover], now: u64) -> Vec<LeftRunning> {
    left.iter()
        .map(|l| LeftRunning {
            exe: l.exe.clone(),
            pid: l.pid,
            started_at: (l.started_ms > 0).then(|| iso(l.started_ms)).flatten(),
            active: l.active(now),
            quiet_min: now.saturating_sub(l.moved_ms) / 60_000,
        })
        .collect()
}

/// Between turns, how long all its turns left running has done nothing — the
/// least of them — and their names for a notice; None with nothing left, or
/// in a turn.
pub fn left_quiet(id: &str) -> Option<(u64, String)> {
    let r = lock();
    let e = r.entries.get(id).filter(|e| e.run.is_none() && e.left_running && !e.left.is_empty())?;
    let now = crate::throttle::now_ms();
    let quiet = crate::procs::quiet_ms(&e.left, now)?;
    Some((quiet, left_names(&e.left)))
}

/// Why its last turn stopped short, if it did.
pub fn last_error(id: &str) -> Option<String> {
    lock().entries.get(id)?.error.clone()
}

/// How an `interrupt` went.
#[derive(Debug, PartialEq, Clone, Copy)]
pub struct Interrupted {
    /// The turn it was on was stopped, and the text starts the next.
    pub stopped: bool,
    /// The text waits behind a turn that goes on: one that would not stop in
    /// time, or that has no process to stop yet. It ends a turn after that one.
    pub queued: bool,
}

/// How long a turn claimed a moment ago gets to have a process to stop.
const CLAIM_WAIT: std::time::Duration = std::time::Duration::from_secs(3);
/// How long a stopped turn's process gets to read as gone.
const STOP_WAIT: std::time::Duration = std::time::Duration::from_secs(15);

/// Stops the turn in progress and starts the next on `text`, behind whatever
/// was queued for after it: the conversation goes on, the work the turn was
/// doing does not. All of it waits in the queue, which the turn's end takes up
/// whole, so nobody's text hangs on how the stop goes: what would not stop
/// still has it all behind it.
pub fn interrupt(id: &str, text: &str, from: Option<&str>) -> Result<Interrupted, String> {
    let free = {
        let mut r = lock();
        let e = r.entries.get_mut(id).ok_or("no such helper")?;
        e.held.clear();
        let free = e.run.is_none() && !e.ending_left && !e.promoting;
        if free {
            claim(e);
        } else {
            note(e, "interrupted");
            e.queue.push(Queued::new(from.map(str::to_owned), text.to_string(), Vec::new()));
        }
        free
    };
    reg().1.notify_all();
    // In the queue it is what a core killed now hands back to its sender.
    save();
    if free {
        run_turn(id, text)?;
        changed();
        return Ok(Interrupted { stopped: false, queued: false });
    }
    let queued = Interrupted { stopped: false, queued: true };
    let deadline = std::time::Instant::now() + CLAIM_WAIT;
    let (pid, _pin) = loop {
        {
            let r = lock();
            let e = r.entries.get(id).ok_or("no such helper")?;
            // Taken up already: that turn ended by itself, and this is the next.
            if e.queue.is_empty() {
                return Ok(queued);
            }
            match e.run.as_ref().map(|run| run.pid) {
                // Pinned under the lock, as `kill` does.
                Some(Some(pid)) => break (pid, crate::procs::pin(pid)),
                Some(None) if std::time::Instant::now() < deadline => {}
                _ => return Ok(queued),
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    crate::pty::kill_tree(Some(pid));
    // The turn is over once its reader has seen the process go.
    let deadline = std::time::Instant::now() + STOP_WAIT;
    let (m, cv) = reg();
    let mut r = m.lock().unwrap_or_else(|e| e.into_inner());
    while r.entries.get(id).and_then(|e| e.run.as_ref()).is_some_and(|run| run.pid == Some(pid)) {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            drop(r);
            changed();
            return Ok(queued);
        }
        r = cv.wait_timeout(r, left).unwrap_or_else(|e| e.into_inner()).0;
    }
    drop(r);
    changed();
    Ok(Interrupted { stopped: true, queued: false })
}

/// The worktree it made, once the CLI has said where: Claude Code as its
/// first turn starts, Codex in its rollout.
pub fn made_worktree(id: &str) -> Option<String> {
    let (cwd, folder, provider, account, session) = {
        let r = lock();
        let e = r.entries.get(id)?;
        if !e.spec.worktree {
            return None;
        }
        (e.cwd.clone(), e.spec.folder.clone(), e.spec.provider, e.spec.account_path.clone(), e.session.clone())
    };
    let cwd = cwd.or_else(|| match provider {
        Provider::Codex => crate::codex::session::thread_cwd(&account, session.as_deref()?),
        Provider::Claude => None,
    })?;
    (!crate::pty::same_dir(&cwd, &folder)).then_some(cwd)
}

/// The worktree `remove` would hand back, with nothing touched: where the CLI
/// has said it works, off Codex's rollout until the turn's end records it.
pub fn removable_worktree(id: &str) -> Option<String> {
    let (cwd, folder, provider, account, session) = {
        let r = lock();
        let e = r.entries.get(id)?;
        (e.cwd.clone(), e.spec.folder.clone(), e.spec.provider, e.spec.account_path.clone(), e.session.clone())
    };
    let cwd = cwd.or_else(|| match provider {
        Provider::Codex => crate::codex::session::thread_cwd(&account, session.as_deref()?),
        Provider::Claude => None,
    })?;
    (!crate::pty::same_dir(&cwd, &folder) && crate::worktree::is_worktree_of(&folder, &cwd)).then_some(cwd)
}

/// Stops its turn and what its turns left, for a look at what they leave behind.
pub fn stop_for_good(id: &str) {
    kill(id);
    let job = lock().entries.get(id).and_then(|e| e.job.clone());
    end_leftovers(id, job.as_deref());
}

/// Stops it and forgets it. Returns the worktree it made, if any.
pub fn remove(id: &str) -> Option<String> {
    let made = removable_worktree(id);
    kill(id);
    let e = lock().entries.remove(id);
    reg().1.notify_all();
    // What each of its turns was started with, as a chat's go when it is deleted.
    for sub in ["hooks", "mcp"] {
        let _ = std::fs::remove_file(crate::paths::data_dir().join(sub).join(format!("{id}.json")));
    }
    crate::pty::claude_instructions_dir(id, None);
    changed();
    let e = e?;
    crate::agents::undelivered_left(id, &e.undelivered);
    end_leftovers(id, e.job.as_deref());
    made
}

/// The last lines of what it did, in place of a screen.
pub fn screen(id: &str, lines: usize) -> Option<String> {
    let r = lock();
    let e = r.entries.get(id)?;
    let from = e.log.len().saturating_sub(lines);
    let mut out: Vec<String> = e.log.iter().skip(from).cloned().collect();
    if let Some(err) = &e.error {
        out.push(format!("last error: {}", one_line(err)));
    }
    Some(out.join("\n"))
}

/// Its transcript (Claude Code) or rollout (Codex), once the first turn has
/// named its conversation.
pub fn transcript(id: &str) -> Option<std::path::PathBuf> {
    let (provider, account, session, cwd, folder) = {
        let r = lock();
        let e = r.entries.get(id)?;
        (e.spec.provider, e.spec.account_path.clone(), e.session.clone()?, e.cwd.clone(), e.spec.folder.clone())
    };
    match provider {
        Provider::Claude => Some(crate::claude::session::transcript_file(&account, cwd.as_deref().unwrap_or(&folder), &session)),
        Provider::Codex => crate::codex::session::rollout_of_thread(&account, &session),
    }
}

/// The conversation's id as the CLI knows it.
pub fn session_id(id: &str) -> Option<String> {
    lock().entries.get(id)?.session.clone()
}

/// Where it works: the worktree its first turn made, or its folder.
pub fn working_dir(id: &str) -> Option<String> {
    let r = lock();
    let e = r.entries.get(id)?;
    Some(e.cwd.clone().unwrap_or_else(|| e.spec.folder.clone()))
}

pub fn ask(id: &str) -> Option<Ask> {
    lock().entries.get(id)?.held.iter().find(|h| h.answer.is_none()).map(|h| h.ask.clone())
}

/// `luna_permission`: the CLI asks before a tool runs, or carries the model's
/// `AskUserQuestion`. Held until `luna_answer`, the turn's end or the timeout;
/// the answer is the JSON the CLI takes back.
pub fn permission(id: &str, args: &Value) -> Value {
    let tool = args["tool_name"].as_str().unwrap_or("").to_string();
    let input = args["input"].clone();
    let mut ask = if tool == "AskUserQuestion" {
        Ask::from_claude_questions(&input).unwrap_or_else(|| Ask::from_claude_tool(&tool, &input))
    } else {
        Ask::from_claude_tool(&tool, &input)
    };
    let serial = {
        let mut r = lock();
        let serial = r.next_serial;
        r.next_serial += 1;
        let Some(e) = r.entries.get_mut(id) else {
            return json!({ "behavior": "deny", "message": "Luna does not know this session." });
        };
        ask.id = Some(format!("a{serial}"));
        note(e, &format!("waits: {}", ask.line()));
        e.held.push_back(Held { serial, ask, input, answer: None });
        r.asked.insert(serial, id.to_string());
        serial
    };
    changed();
    let deadline = std::time::Instant::now() + ASK_TIMEOUT;
    let (m, cv) = reg();
    let mut r = m.lock().unwrap_or_else(|e| e.into_inner());
    let answer = loop {
        let held = r.entries.get_mut(id).filter(|e| e.run.is_some()).and_then(|e| e.held.iter_mut().find(|h| h.serial == serial));
        match held {
            Some(h) => {
                if let Some(a) = h.answer.take() {
                    break Some(a);
                }
            }
            // Killed, deleted, or its turn is gone: nobody will answer.
            None => break None,
        }
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            break None;
        }
        r = cv.wait_timeout(r, left).unwrap_or_else(|e| e.into_inner()).0;
    };
    if let Some(e) = r.entries.get_mut(id) {
        e.held.retain(|h| h.serial != serial);
    }
    drop(r);
    changed();
    answer.unwrap_or_else(|| json!({ "behavior": "deny", "message": "Nobody answered in time; go on without it or ask in your reply." }))
}

/// The agent's answer to what the helper holds: `answers` for a question
/// (an option's number or label, or words of its own, one per question;
/// several numbers apart by commas where more than one may be picked),
/// `allow` for a permission, `note` as the reason for a refusal.
///
/// `ask_id` says which of what it holds this answers: two responders who saw
/// the same ask must not have the slower one's `allow` land on the next. One no
/// longer open is refused. Without it the answer goes through only when a
/// single ask is held, as it did when there could be only one: with several,
/// nothing says it was written on the first.
pub fn answer(id: &str, ask_id: Option<&str>, answers: &[String], allow: Option<bool>, note_text: Option<&str>) -> Result<(), String> {
    let mut r = lock();
    let given = ask_id.is_some_and(|want| {
        want.strip_prefix('a').and_then(|n| n.parse::<u64>().ok()).and_then(|n| r.asked.get(&n)).is_some_and(|to| to == id)
    });
    let e = r.entries.get_mut(id).ok_or("no such helper")?;
    let mut open = e.held.iter_mut().filter(|h| h.answer.is_none());
    let h = match ask_id {
        Some(want) => open.find(|h| h.ask.id.as_deref() == Some(want)).ok_or_else(|| {
            if given {
                format!("ask {want} of helper {id} is no longer open: it was answered or went with its turn. Its `ask` in luna_list says what it holds now")
            } else {
                format!("helper {id} has had no ask {want} (since Luna's core last started): the id is unknown. Its `ask` in luna_list has the id of what it holds now, if anything")
            }
        })?,
        None => {
            let first = open.next().ok_or_else(|| format!("helper {id} is not waiting on anything"))?;
            if open.next().is_some() {
                return Err(format!(
                    "helper {id} holds several asks at once: say which with askId. The first is {}: it {}",
                    first.ask.id.as_deref().unwrap_or("?"),
                    first.ask.line()
                ));
            }
            first
        }
    };
    let reply = build_answer(&h.ask, &h.input, answers, allow, note_text)?;
    h.answer = Some(reply);
    drop(r);
    reg().1.notify_all();
    Ok(())
}

fn build_answer(ask: &Ask, input: &Value, answers: &[String], allow: Option<bool>, note_text: Option<&str>) -> Result<Value, String> {
    if ask.kind == "permission" {
        return match allow {
            Some(true) => Ok(json!({ "behavior": "allow", "updatedInput": input })),
            Some(false) => Ok(json!({ "behavior": "deny", "message": note_text.unwrap_or("The user declined.") })),
            None => Err("this is a permission: answer with allow true or false".into()),
        };
    }
    if allow == Some(false) {
        return Ok(json!({ "behavior": "deny", "message": note_text.unwrap_or("The user declined to answer.") }));
    }
    if answers.len() != ask.questions.len() {
        return Err(format!("{} question(s), {} answer(s): give one answer per question", ask.questions.len(), answers.len()));
    }
    let mut picked = serde_json::Map::new();
    for (q, a) in ask.questions.iter().zip(answers) {
        picked.insert(q.text.clone(), Value::String(crate::agents::resolve_option(q, a)));
    }
    let mut updated = input.clone();
    if let Some(o) = updated.as_object_mut() {
        o.insert("answers".into(), Value::Object(picked));
    }
    Ok(json!({ "behavior": "allow", "updatedInput": updated }))
}

/// A helper as the window lists it under its parent.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub id: String,
    pub parent: Option<String>,
    pub name: String,
    pub provider: Provider,
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ask: Option<Ask>,
    /// The account folder's name.
    pub account: String,
    pub model: Option<String>,
    pub effort: String,
    pub worktree: bool,
    pub started_ms: u64,
    /// While a turn runs, when it started.
    pub turn_started_ms: Option<u64>,
    /// Between turns, what its turns left running ("tail.exe (pid 22104)"):
    /// what its Stop ends then.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub left_running: Vec<String>,
    /// Being opened as a chat (`background_promote`).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub promoting: bool,
}

/// Every helper, for the window.
#[tauri::command]
pub fn background_helpers() -> Vec<Row> {
    let ids: Vec<(String, Spec, u64, Option<u64>, Vec<String>, bool)> = lock()
        .entries
        .iter()
        .map(|(id, e)| {
            let left = if e.run.is_none() && e.left_running { e.left.iter().map(Leftover::label).collect() } else { Vec::new() };
            (id.clone(), e.spec.clone(), e.started_ms, e.run.as_ref().map(|r| r.started_ms), left, e.promoting)
        })
        .collect();
    ids.into_iter()
        .map(|(id, spec, started_ms, turn_started_ms, left_running, promoting)| Row {
            parent: crate::agents::parent_of(&id),
            name: crate::agents::chat_name(&id).unwrap_or_default(),
            state: crate::agents::state_of(&id).0,
            ask: ask(&id),
            provider: spec.provider,
            account: std::path::Path::new(&spec.account_path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            model: spec.model,
            effort: spec.effort,
            worktree: spec.worktree,
            started_ms,
            turn_started_ms,
            left_running,
            promoting,
            id,
        })
        .collect()
}

/// Tells the window the list has changed.
pub fn changed() {
    crate::emit::to_ui("agent://background", background_helpers());
}

/// What `luna_list` says of a helper beyond its state.
pub fn signs(id: &str) -> Option<Signs> {
    let (signs, provider) = {
        let r = lock();
        let e = r.entries.get(id)?;
        let o = &e.observed;
        let turn_started = e.run.as_ref().map(|run| run.started_ms);
        let last = [o.event_ms, o.turn_ended_ms, turn_started.unwrap_or(0)].into_iter().max().filter(|ms| *ms > 0);
        let signs = Signs {
            turn_started_at: turn_started.and_then(iso),
            last_activity_at: last.and_then(iso),
            // A Codex rollout that would not read: what it was asked to run on,
            // once a turn has got through on it.
            resolved_model: o.model.clone().or_else(|| e.spec.model.clone().filter(|_| e.spec.provider == Provider::Codex && o.answered)),
            context: None,
            left_running: left_shown(e, crate::throttle::now_ms()),
        };
        (signs, e.spec.provider)
    };
    // Off the transcript, with the lock let go: every helper's events wait on it.
    let context = transcript(id).and_then(|path| Context::of(&path, provider));
    Some(Signs { context, ..signs })
}

/// While a turn runs, how long its CLI has said nothing: no event on its
/// stdout or stderr since this long, or since the turn began. A tool call that
/// hangs shows as this and nothing else.
pub fn turn_quiet_ms(id: &str, now: u64) -> Option<u64> {
    let r = lock();
    let e = r.entries.get(id)?;
    let run = e.run.as_ref()?;
    Some(now.saturating_sub(e.observed.event_ms.max(run.started_ms)))
}

/// What the window needs to make a helper a chat of its own.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Promoted {
    pub id: String,
    pub provider: Provider,
    pub account_path: String,
    pub folder: String,
    pub worktree_path: Option<String>,
    pub session_id: Option<String>,
    pub parent: Option<String>,
    pub name: Option<String>,
    pub settings: Value,
    pub tools: bool,
}

/// Starts handing a helper over to a chat row, from which on the window runs
/// it by resuming its conversation in a terminal like any chat's: what the row
/// needs. Not while a turn runs — the conversation would be in two processes at
/// once. The helper stays, with no turn starting, until the window says how the
/// row came out (`background_promoted`): a row it could not make leaves the
/// helper as it was, rather than neither.
#[tauri::command]
pub fn background_promote(id: String) -> Result<Promoted, String> {
    let (spec, cwd, session) = {
        let mut r = lock();
        let e = r.entries.get_mut(&id).ok_or("no such helper")?;
        if e.run.is_some() {
            return Err("it is in the middle of a turn; open it once the turn is over".into());
        }
        // Its job goes with the entry, and what is still in it would run on
        // with nobody counting it for the machine or ending it.
        if e.left_running || e.ending_left {
            return Err("what its turns left is still running; stop it first, or open it once that is over".into());
        }
        // One opening at a time: a second would make a second row of the one id.
        if e.promoting {
            return Err("it is being opened as a chat already".into());
        }
        e.promoting = true;
        (e.spec.clone(), e.cwd.clone(), e.session.clone())
    };
    changed();
    let worktree_path = cwd.filter(|c| !crate::pty::same_dir(c, &spec.folder));
    Ok(Promoted {
        provider: spec.provider,
        account_path: spec.account_path.clone(),
        folder: crate::worktree::project_of(&spec.folder).unwrap_or_else(|| spec.folder.clone()),
        worktree_path: worktree_path.or_else(|| crate::worktree::project_of(&spec.folder).map(|_| spec.folder.clone())),
        session_id: session,
        parent: crate::agents::parent_of(&id),
        name: crate::agents::chat_name(&id),
        settings: crate::agents::settings_of(&id),
        tools: spec.tools,
        id,
    })
}

/// The window's word on a helper `background_promote` began handing over: its
/// row made (`error` None), the helper goes, what was sent to it meanwhile with
/// what never reached it; or the row could not be made, and it goes on as a
/// helper, what was sent meanwhile its next turn. Also said by a window settling
/// what one gone before it was opening, whose row it finds or not.
#[tauri::command]
pub fn background_promoted(id: String, error: Option<String>) -> Result<(), String> {
    let Some(why) = error else {
        // Gone already (deleted meanwhile): the row has its conversation.
        let Some(mut e) = lock().entries.remove(&id) else { return Ok(()) };
        let queued = std::mem::take(&mut e.queue);
        e.undelivered.extend(queued);
        crate::agents::undelivered_left(&id, &e.undelivered);
        // Its count goes on in the chat's track: what its callers are owed, and
        // what they were told to wait past, go by it.
        crate::activity::count_at_least(&id, e.turns_ended);
        crate::agents::promoted(&id);
        changed();
        // Or a core killed now would take it up beside the chat it has become.
        save();
        return Ok(());
    };
    crate::log::warn("background", &format!("{} was not opened as a chat: {why}", crate::agents::label(&id)));
    let next = {
        let mut r = lock();
        let Some(e) = r.entries.get_mut(&id) else { return Ok(()) };
        e.promoting = false;
        (e.run.is_none() && !e.ending_left && !e.queue.is_empty()).then(|| {
            claim(e);
            take_queue(&id, e)
        })
    };
    match next {
        Some(text) => {
            save();
            if run_turn(&id, &text).is_err() {
                if let Some(e) = lock().entries.get_mut(&id) {
                    e.in_flight.clear();
                }
            }
        }
        // A `resumeOnRestart` helper a restart left to go on, which the opening
        // held back: it goes on now, if nothing else has taken it up.
        None if crate::agents::resumes_on_restart(&id) => resume(&id),
        None => {}
    }
    changed();
    Ok(())
}

#[tauri::command]
pub fn background_kill(id: String) {
    kill(&id);
}

#[tauri::command]
pub fn background_delete(id: String, drop_worktree: Option<bool>) -> Result<(), String> {
    crate::agents::delete_by_user(&id, drop_worktree.unwrap_or(true))
}

// ------------------------------------------------------- across cores --

/// How often the helpers are written out, when they have changed.
const SAVE_EVERY: std::time::Duration = std::time::Duration::from_secs(2);

/// A helper as `<data>/helpers.json` keeps it, for the core started after
/// this one: between turns a helper is nothing but this and its
/// conversation, which the CLI keeps on disk. What it was told and said is
/// not here, only where to find it.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Saved {
    id: String,
    spec: Spec,
    started_ms: u64,
    session: Option<String>,
    cwd: Option<String>,
    turns_ended: u32,
    /// A turn ran as this was written: should the core go before the next
    /// write, it went mid-turn and stopped it.
    running: bool,
    /// Text was queued behind that turn, which goes with it.
    queued: bool,
    /// That text, word for word, for its sender to send again (a file an
    /// older core left has only `queued`).
    #[serde(default)]
    queue: Vec<Queued>,
    /// See `Entry::in_flight`.
    #[serde(default)]
    in_flight: Vec<Queued>,
    /// See `Entry::fed`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    fed: Vec<Queued>,
    /// See `Entry::budgets_landed`.
    #[serde(default)]
    budgets_landed: Vec<String>,
    /// See `Entry::undelivered`.
    #[serde(default)]
    undelivered: Vec<Queued>,
    /// See `Entry::tied`.
    #[serde(default)]
    tied: bool,
    /// See `Entry::unsure`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    unsure: bool,
    /// See `Entry::opening`: written only while no turn has named the
    /// conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    opening: Option<String>,
    /// See `Entry::opening_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    opening_id: Option<String>,
    cut_short: bool,
    ties: crate::agents::Ties,
    #[serde(default)]
    observed: Observed,
}

fn saved_file() -> std::path::PathBuf {
    crate::paths::data_dir().join("helpers.json")
}

/// Set once `restore` has read the file: before it, an empty registry is
/// not to be written over what the last core left.
static RESTORED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Writes the helpers out if they changed since the last write: every
/// SAVE_EVERY, and at once on what a core killed now must not undo (a spawn, a
/// delete, a question asked). Not to be called with the registry's lock or the
/// agent layer's held.
pub fn save() {
    use std::sync::atomic::Ordering;
    static LAST: Mutex<Vec<u8>> = Mutex::new(Vec::new());
    if !RESTORED.load(Ordering::SeqCst) {
        return;
    }
    // Held from before the reading: a save that read the registry ahead of a
    // delete and wrote behind that delete's own save would bring the helper
    // back, for a core killed before the next one.
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    // The chats' ties go by the same beat, in a file of their own.
    crate::agents::save_chats();
    let Ok(bytes) = serde_json::to_vec_pretty(&snapshot()) else { return };
    if *last == bytes {
        return;
    }
    let path = saved_file();
    let tmp = path.with_extension("json.tmp");
    match std::fs::write(&tmp, &bytes).and_then(|()| std::fs::rename(&tmp, &path)) {
        Ok(()) => *last = bytes,
        Err(e) => crate::log::warn("background", &format!("could not save the helpers to {}: {e}", path.display())),
    }
}

/// The registry as `helpers.json` keeps it.
fn snapshot() -> Vec<Saved> {
    let mut saved: Vec<Saved> = lock()
        .entries
        .iter()
        .map(|(id, e)| Saved {
            id: id.clone(),
            spec: e.spec.clone(),
            started_ms: e.started_ms,
            session: e.session.clone(),
            cwd: e.cwd.clone(),
            turns_ended: e.turns_ended,
            running: e.run.is_some(),
            queued: !e.queue.is_empty(),
            queue: e.queue.clone(),
            in_flight: e.in_flight.clone(),
            fed: e.fed.clone(),
            budgets_landed: e.budgets_landed.clone(),
            undelivered: e.undelivered.clone(),
            tied: e.tied,
            unsure: e.unsure,
            opening: e.opening.clone().filter(|_| e.session.is_none()),
            opening_id: e.opening_id.clone().filter(|_| e.session.is_none()),
            cut_short: e.cut_short,
            observed: e.observed.clone(),
            ties: Default::default(),
        })
        .collect();
    // The agent layer's lock after this one is let go: it is the one taken
    // first where both are held.
    for s in &mut saved {
        s.ties = crate::agents::ties_of(&s.id).unwrap_or_default();
        // Its CLI has said where it works.
        if s.session.is_some() || s.cwd.is_some() {
            s.ties.worktrees_before = None;
        }
    }
    saved.sort_by_key(|s| s.started_ms);
    saved
}

/// Takes up the helpers the last core left, and keeps them saved from here
/// on. A turn that core was in the middle of is over, stopped with it; the
/// caller owed its answer is told so, and given back what was queued behind
/// it. One spawned with `resumeOnRestart` goes on by itself (`resume_stopped`).
/// One stopped before any turn named its conversation is taken up all the
/// same: its next turn starts one, and what it was started on goes back to the
/// caller with the rest, or to that next turn if it resumes by itself.
pub fn restore() {
    let saved: Vec<Saved> = match std::fs::read(saved_file()) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            crate::log::warn("background", &format!("could not read helpers.json: {e}"));
            Vec::new()
        }),
        Err(_) => Vec::new(),
    };
    let mut back = Vec::new();
    let last_ties = crate::procs::end_last_ties();
    for mut s in saved {
        s.tied = s.tied && last_ties.held(&s.id);
        s.unsure = s.unsure && !last_ties.confirmed(&s.id);
        let (account, session) = (s.spec.account_path.clone(), s.session.clone());
        settle_lines(&mut s, &|line| {
            let path = crate::claude::session::find_transcript(&account, session.as_deref()?)?;
            crate::claude::session::took_line(&path, line)
        });
        let ties = std::mem::take(&mut s.ties);
        // The agent layer's record of whose it is (`Known::parent`) is the one.
        s.spec.parent = ties.parent.clone();
        let token = crate::agents::tie_back(&s.id, s.spec.provider, &s.spec.folder, &s.spec.account_path, ties);
        let (id, running) = (s.id.clone(), s.running);
        let mut e = taken_up(s);
        e.spec.token = token;
        lock().entries.insert(id.clone(), e);
        back.push(format!("{}{}", crate::agents::label(&id), if running { " (stopped mid-turn)" } else { "" }));
    }
    RESTORED.store(true, std::sync::atomic::Ordering::SeqCst);
    if !back.is_empty() {
        crate::log::warn("background", &format!("took up {} helper(s) the last core left: {}", back.len(), back.join(", ")));
        changed();
    }
    save();
    let _ = std::thread::Builder::new().name("luna-bg-save".into()).spawn(|| loop {
        std::thread::sleep(SAVE_EVERY);
        save();
    });
}

/// The helpers a core took up stopped mid-turn that were spawned with
/// `resumeOnRestart`: each starts its next turn by itself. Off this thread,
/// and only once the hub listens: a turn's MCP config names its port.
pub fn resume_stopped() {
    let (ids, unsure): (Vec<(String, bool)>, Vec<(String, bool)>) = lock()
        .entries
        .iter()
        .filter(|(_, e)| e.cut_short && e.run.is_none())
        .map(|(id, e)| (id.clone(), e.resumable))
        .partition(|(_, sure)| *sure);
    for (id, _) in unsure.iter().filter(|(id, _)| crate::agents::resumes_on_restart(id)) {
        crate::log::warn(
            "background",
            &format!("{}: not resumed: nothing says the last core's turn ended, and a second could overlap it", crate::agents::label(id)),
        );
    }
    let ids: Vec<String> = ids.into_iter().map(|(id, _)| id).collect();
    let ids: Vec<String> = ids.into_iter().filter(|id| crate::agents::resumes_on_restart(id)).collect();
    if ids.is_empty() {
        return;
    }
    let _ = std::thread::Builder::new().name("luna-bg-resume".into()).spawn(move || {
        for id in ids {
            resume(&id);
        }
    });
}

/// Starts the turn that carries on from one a core's going stopped. Its
/// callers are owed the end of that turn, and told it is running. One that
/// cannot start ends as `run_turn` records it and leaves the helper cut short
/// with its text kept: the caller's notice is then the one for a helper that
/// did not go on.
fn resume(id: &str) {
    let (text, count, carried) = {
        let mut r = lock();
        let Some(e) = r.entries.get_mut(id) else { return };
        // Something got there first. No conversation to go on with and no
        // words of its first turn (a file from an older core): nothing to run.
        let unnamed = e.session.is_none();
        // What may have reached it stays with its sender to decide on.
        let (doubtful, sure): (Vec<Queued>, Vec<Queued>) = std::mem::take(&mut e.undelivered).into_iter().partition(|q| q.maybe_reached);
        // Or a turn it was sent meanwhile has gone on already, and been told.
        // Or it is being opened as a chat, which takes the conversation up.
        if e.run.is_some() || e.promoting || !e.cut_short || !e.resumable || (unnamed && sure.is_empty()) {
            e.undelivered = [sure, doubtful].concat();
            return;
        }
        claim(e);
        // This batch only: whatever is put by after this goes on being kept.
        e.in_flight = sure;
        e.undelivered = doubtful;
        stamp(&mut e.in_flight);
        // Never started, it is started now, on what it was given as it was.
        let text = if unnamed { joined(&e.in_flight) } else { resume_text(&e.in_flight) };
        (text, e.turns_ended, e.in_flight.len())
    };
    save();
    crate::agents::resumed(id, count);
    match run_turn(id, &text) {
        Ok(()) => {
            if let Some(e) = lock().entries.get_mut(id) {
                note(e, "resumed by itself after a core stopped its turn");
            }
            crate::log::warn(
                "background",
                &format!(
                    "{}: resumed by itself after a core stopped its turn, {carried} undelivered message(s) with it",
                    crate::agents::label(id)
                ),
            );
        }
        Err(why) => {
            if let Some(e) = lock().entries.get_mut(id) {
                let batch = std::mem::take(&mut e.in_flight);
                let later = std::mem::take(&mut e.undelivered);
                e.undelivered = [batch, later].concat();
            }
            crate::log::warn("background", &format!("{}: could not resume by itself: {why}", crate::agents::label(id)));
        }
    }
    changed();
}

/// What a resumed helper is told: why its turn stopped, and what was queued
/// for it, word for word, as it would have come.
pub(crate) fn resume_text(queued: &[Queued]) -> String {
    let mut text = format!(
        "{} Luna's core restarted and stopped your last turn before it finished. Carry on where you left off.",
        crate::agents::NOTICE_TAG
    );
    if !queued.is_empty() {
        text.push_str("\n\nThese messages were sent to you while you worked and never reached you, oldest first:");
        for message in queued {
            text.push_str("\n\n");
            text.push_str(&message.text);
        }
    }
    text
}

/// A turn no core stopped ended only if its tie saw to it (`Entry::tied`).
fn turn_known_over(s: &Saved) -> bool {
    !s.unsure && (!s.running || s.cut_short || s.tied)
}

/// What the last core had on a Claude Code helper's stdin and heard nothing more
/// of, settled by what the conversation holds, before any of it is told of or
/// sent again: `reached` says whether a line is there, None if that cannot be
/// read. In it, the model has it, and it is off the books; not in it, with the
/// turn known to be over, it never got there; otherwise nobody can say, and its
/// sender is told that (`Queued::maybe_reached`) rather than it going out twice.
fn settle_lines(s: &mut Saved, reached: &dyn Fn(&str) -> Option<bool>) {
    if s.spec.provider != Provider::Claude {
        return;
    }
    let over = turn_known_over(s);
    let mut known: HashMap<String, Option<bool>> = HashMap::new();
    let mut landed: Vec<String> = Vec::new();
    for store in [&mut s.in_flight, &mut s.fed, &mut s.undelivered] {
        store.retain_mut(|q| {
            let Some(line) = q.line.take() else { return true };
            match *known.entry(line).or_insert_with_key(|line| reached(line)) {
                Some(true) => {
                    landed.extend(q.budget.iter().map(|m| m.id.clone()));
                    false
                }
                Some(false) if over => true,
                _ => {
                    q.maybe_reached = true;
                    true
                }
            }
        });
    }
    s.budgets_landed.extend(landed);
}

/// What a helper stopped with no conversation yet was started on, as a message,
/// if nothing else holds it. A batch it was started on is kept already, with what
/// is known of its fate, and a second copy of it would go out as new. It goes under
/// the id it had. By its words in a file from before ids.
fn lone_opening(s: &mut Saved, stopped: bool) -> Option<Queued> {
    let text = s.opening.take().filter(|_| stopped && s.session.is_none())?;
    let id = s.opening_id.take();
    let kept = match &id {
        Some(id) => [&s.in_flight, &s.fed, &s.undelivered].iter().any(|store| store.iter().any(|q| q.id == *id)),
        None => !s.in_flight.is_empty() && joined(&s.in_flight) == text,
    };
    if kept {
        return None;
    }
    let mut q = Queued::new(None, text, Vec::new());
    if let Some(id) = id {
        q.id = id;
    }
    Some(q)
}

/// A saved helper as a registry entry, between turns: a turn that ran went
/// with the old core and counts as ended, and so does the text queued
/// behind it, which never reached the CLI (as `stop` counts them) and is kept
/// for its sender.
fn taken_up(mut s: Saved) -> Entry {
    let stopped = s.running || s.cut_short;
    let resumable = turn_known_over(&s);
    let first: Vec<Queued> = lone_opening(&mut s, stopped).into_iter().collect();
    let undelivered = [first, s.undelivered, s.in_flight, s.fed, s.queue].concat();
    let cwd = s.cwd.map(|c| home_of(&s.spec, c));
    let mut e = Entry {
        spec: s.spec,
        started_ms: s.started_ms,
        session: s.session,
        cwd,
        run: None,
        turns_ended: s.turns_ended + u32::from(s.running) + u32::from(s.queued),
        queue: Vec::new(),
        in_flight: Vec::new(),
        fed: Vec::new(),
        log: VecDeque::new(),
        held: VecDeque::new(),
        error: None,
        cut_short: stopped,
        undelivered,
        budgets_landed: s.budgets_landed,
        tied: false,
        resumable,
        unsure: !resumable,
        opening: None,
        opening_id: None,
        job: None,
        left_running: false,
        left: Vec::new(),
        ending_left: false,
        promoting: false,
        observed: s.observed,
    };
    if s.running {
        note(&mut e, "Luna's core went mid-turn and stopped the turn");
        e.error = Some("Luna's core restarted mid-turn and stopped the turn".into());
    }
    if s.queued {
        note(&mut e, "what was queued behind it never reached it");
    }
    note(&mut e, "taken up by a new core");
    e
}

/// Where a helper works, from a cwd an older core saved after the CLI's shell
/// had moved it: the worktree it made and is in, else the folder it was sent
/// to if it is in that.
fn home_of(spec: &Spec, cwd: String) -> String {
    if let Some((root, _)) = crate::worktree::root_of(&cwd).filter(|_| spec.worktree) {
        return root;
    }
    let inside = std::path::Path::new(&cwd).ancestors().any(|dir| crate::pty::same_dir(&dir.to_string_lossy(), &spec.folder));
    if inside {
        spec.folder.clone()
    } else {
        cwd
    }
}

/// Whether its last turn was stopped by the core going rather than ended.
pub fn cut_short(id: &str) -> bool {
    lock().entries.get(id).is_some_and(|e| e.cut_short)
}

/// Whom it is told it answers to, for the tests of its passing to another.
#[cfg(test)]
pub fn told_parent_for_test(id: &str) -> Option<String> {
    lock().entries.get(id)?.spec.parent.clone()
}

/// Whether a core's going stopped it before any turn named its conversation:
/// nothing it did can be carried on from.
pub fn never_started(id: &str) -> bool {
    lock().entries.get(id).is_some_and(|e| e.cut_short && e.session.is_none())
}

/// A helper a core's going stopped before its CLI said where it works: what its next
/// turn does of the worktree that turn began, and what its parent hears of it. Claude
/// Code goes back into the one Luna named if that is still as made (`agents::
/// name_to_go_on`), and Codex makes another, naming none. A first turn no core cut off
/// is no business of this: it has not been stopped.
pub fn settle_worktree(id: &str) {
    let (provider, folder, account, name) = {
        let r = lock();
        let Some(e) = r.entries.get(id).filter(|e| e.cut_short && e.session.is_none() && e.spec.worktree) else { return };
        (e.spec.provider, e.spec.folder.clone(), e.spec.account_path.clone(), e.spec.worktree_name.clone())
    };
    match (provider, name) {
        (Provider::Codex, _) => crate::agents::report_codex_worktrees(id, &folder, &account),
        (Provider::Claude, None) => {}
        (Provider::Claude, Some(name)) => {
            let name = crate::agents::name_to_go_on(id, &name, &folder, &account);
            if let Some(e) = lock().entries.get_mut(id) {
                e.spec.worktree_name = name;
            }
            save();
        }
    }
}

/// What never reached it that `caller` is to hear of (`Entry::undelivered`):
/// what it sent, and for its parent what no session did.
pub fn undelivered_for(id: &str, caller: &str, parent: bool) -> Vec<String> {
    let mine = |q: &Queued| q.from.as_deref().map_or(parent, |f| f == caller);
    let told = |q: &Queued| {
        if q.maybe_reached {
            format!("[This one MAY have reached it before Luna's core went: Luna cannot tell. Check what it did before sending it again.]\n\n{}", q.text)
        } else {
            q.text.clone()
        }
    };
    lock().entries.get(id).map(|e| e.undelivered.iter().filter(|q| mine(q)).map(told).collect()).unwrap_or_default()
}

/// Takes it off, `caller` having been told: `put_back` undoes that.
pub fn take_undelivered_for(id: &str, caller: &str, parent: bool) -> Vec<Queued> {
    let mut r = lock();
    let Some(e) = r.entries.get_mut(id) else { return Vec::new() };
    let (taken, left) =
        std::mem::take(&mut e.undelivered).into_iter().partition(|q: &Queued| q.from.as_deref().map_or(parent, |f| f == caller));
    e.undelivered = left;
    taken
}

pub fn put_back(id: &str, taken: Vec<Queued>) {
    if let Some(e) = lock().entries.get_mut(id) {
        let left = std::mem::take(&mut e.undelivered);
        e.undelivered = [taken, left].concat();
    }
}

/// Text that never reached a helper, by whom, for the tests of who hears of it.
/// A notice with `mark` in it, given to a helper as `send_marked` does, the part
/// that starts a CLI aside. Returns whether it was queued.
#[cfg(test)]
pub fn place_for_test(id: &str, text: &str, mark: BudgetMark) -> bool {
    place(lock().entries.get_mut(id).unwrap(), Queued::new(None, text.to_string(), vec![mark]), false) != Placed::Started
}

/// A turn of its runs.
#[cfg(test)]
pub fn set_running_for_test(id: &str) {
    lock().entries.get_mut(id).unwrap().run = Some(Run::claimed(None, 0));
}

/// A turn begins on what is queued, as `turn_over` starts the next.
#[cfg(test)]
pub fn start_queued_for_test(id: &str) {
    let mut r = lock();
    let e = r.entries.get_mut(id).unwrap();
    e.in_flight = std::mem::take(&mut e.queue);
}

/// Its CLI's stdin has taken what the turn was started on.
#[cfg(test)]
pub fn stdin_took_for_test(id: &str) {
    assert!(stdin_took(lock().entries.get_mut(id).unwrap()));
}

/// Its turn ended without its stdin taking anything, as a CLI gone first leaves it.
#[cfg(test)]
pub fn turn_ended_for_test(id: &str) {
    turn_over(id, Some(1));
}

/// It stops on a permission, as a turn's `luna_permission` holds it.
#[cfg(test)]
pub fn hold_for_test(id: &str) {
    let ask = Ask::from_claude_tool("Bash", &json!({ "command": "x" }));
    lock().entries.get_mut(id).unwrap().held.push_back(Held { serial: 1, ask, input: Value::Null, answer: None });
}

/// `luna_kill`: what was queued is dropped.
#[cfg(test)]
pub fn killed_for_test(id: &str) {
    stop(lock().entries.get_mut(id).unwrap());
}

/// What `helpers.json` keeps of it, written and read back as a core started later
/// finds it.
#[cfg(test)]
pub fn across_a_core_for_test(id: &str) {
    let bytes = serde_json::to_vec(&snapshot().into_iter().filter(|s| s.id == id).collect::<Vec<Saved>>()).unwrap();
    let saved: Vec<Saved> = serde_json::from_slice(&bytes).unwrap();
    let e = taken_up(saved.into_iter().next().unwrap());
    lock().entries.insert(id.to_string(), e);
}

/// `resume`: what was kept undelivered goes on in a turn by itself.
#[cfg(test)]
pub fn resume_for_test(id: &str) {
    let mut r = lock();
    let e = r.entries.get_mut(id).unwrap();
    e.in_flight = std::mem::take(&mut e.undelivered);
}

#[cfg(test)]
pub fn lose_for_test(id: &str, from: &str, text: &str) {
    lock().entries.get_mut(id).unwrap().undelivered.push(Queued::new(Some(from.to_string()), text.to_string(), Vec::new()));
}

/// A helper as a core takes it up, for the tests of what its caller is told.
#[cfg(test)]
pub fn take_up_for_test(id: &str, folder: &str, turns_ended: u32, queue: &[&str], opening: Option<&str>) {
    let spec: Spec = serde_json::from_value(json!({
        "provider": "claude", "accountPath": "C:\\acc", "folder": folder, "worktree": false, "model": null,
        "effort": "low", "mode": "default", "autoReview": false, "tools": false, "hooksUnasked": false,
    }))
    .unwrap();
    let e = taken_up(Saved {
        id: id.to_string(),
        spec,
        started_ms: 0,
        session: opening.is_none().then(|| "sid".to_string()),
        cwd: None,
        turns_ended,
        running: true,
        queued: !queue.is_empty(),
        queue: queue.iter().map(|t| Queued::from(*t)).collect(),
        in_flight: Vec::new(),
        fed: Vec::new(),
        undelivered: Vec::new(),
        budgets_landed: Vec::new(),
        tied: true,
        unsure: false,
        opening: opening.map(str::to_owned),
        opening_id: None,
        cut_short: false,
        observed: Default::default(),
        ties: Default::default(),
    });
    lock().entries.insert(id.to_string(), e);
}

/// For the agent layer's tests: a helper as it stands between turns, there to
/// talk to with no process behind it.
#[cfg(test)]
pub(crate) fn idle_for_test(id: &str, provider: Provider, turns_ended: u32, left_running: bool) {
    let mut e = tests::entry(provider);
    e.turns_ended = turns_ended;
    e.left_running = left_running;
    lock().entries.insert(id.to_string(), e);
}

/// A test helper whose last turn a core's going stopped, as one taken up again
/// after a second restart is: no error of that turn's left to say so.
#[cfg(test)]
pub(crate) fn cut_for_test(id: &str) {
    let mut r = lock();
    let e = r.entries.get_mut(id).expect("a test helper");
    e.cut_short = true;
    e.error = None;
}

/// What a test helper's turns left running: each by name and pid, quiet for so
/// many ms.
#[cfg(test)]
pub(crate) fn left_for_test(id: &str, left: &[(&str, u32, u64)]) {
    let now = crate::throttle::now_ms();
    let mut r = lock();
    let e = r.entries.get_mut(id).expect("a test helper");
    e.left_running = !left.is_empty();
    e.left = left
        .iter()
        .map(|(exe, pid, quiet)| Leftover {
            pid: *pid,
            exe: exe.to_string(),
            started_ms: 0,
            cpu_ms: 0,
            io_bytes: 0,
            read_ms: now,
            seen_ms: now,
            moved_ms: now - quiet,
        })
        .collect();
}

#[cfg(test)]
pub(crate) fn gone_for_test(id: &str) {
    lock().entries.remove(id);
}

/// The helper in a turn with no process, so nothing real can be killed.
#[cfg(test)]
pub(crate) fn in_turn_for_test(id: &str) {
    if let Some(e) = lock().entries.get_mut(id) {
        claim(e);
    }
}

/// A test helper in a turn that has said nothing for `quiet` ms.
#[cfg(test)]
pub(crate) fn quiet_for_test(id: &str, quiet: u64) {
    let now = crate::throttle::now_ms();
    let mut r = lock();
    let e = r.entries.get_mut(id).expect("a test helper");
    if e.run.is_none() {
        claim(e);
    }
    if let Some(run) = e.run.as_mut() {
        run.started_ms = now - quiet;
    }
    e.observed.event_ms = now - quiet;
}

/// A helper as `agents`'s tests need one, with no process behind it: in a
/// turn or between turns, on a conversation they can write the transcript of.
#[cfg(test)]
pub fn test_helper(id: &str, account_path: &str, session: Option<&str>, busy: bool) {
    let e = Entry {
        spec: Spec {
            provider: Provider::Claude,
            account_path: account_path.into(),
            folder: account_path.into(),
            worktree: false,
            worktree_name: None,
            model: None,
            effort: "low".into(),
            mode: "default".into(),
            auto_review: false,
            tools: false,
            hooks_unasked: false,
            read_only: false,
            agent: None,
            parent: None,
            token: None,
        },
        started_ms: 0,
        session: session.map(str::to_owned),
        cwd: None,
        run: busy.then(|| Run::claimed(None, 1)),
        turns_ended: 0,
        queue: Vec::new(),
        in_flight: Vec::new(),
        fed: Vec::new(),
        log: VecDeque::new(),
        held: VecDeque::new(),
        error: None,
        cut_short: false,
        undelivered: Vec::new(),
        budgets_landed: Vec::new(),
        tied: false,
        resumable: false,
        unsure: false,
        opening: None,
        opening_id: None,
        job: None,
        left_running: false,
        left: Vec::new(),
        ending_left: false,
        promoting: false,
        observed: Observed::default(),
    };
    lock().entries.insert(id.to_string(), e);
}

/// A test helper's turn has ended, its process never having been.
#[cfg(test)]
pub fn test_turn_ended(id: &str) {
    let mut r = lock();
    let e = r.entries.get_mut(id).expect("a test helper");
    e.run = None;
    e.turns_ended += 1;
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

    /// A helper as an older core saved it, with the tokens and cost it counted
    /// then: it comes back all the same, with what is still kept.
    #[test]
    fn a_helper_saved_with_what_it_spent_comes_back() {
        let saved = json!([{
            "id": "c1", "spec": serde_json::to_value(&entry(Provider::Claude).spec).unwrap(), "startedMs": 5,
            "session": "sid", "turnsEnded": 3, "running": false, "queued": false, "cutShort": false,
            "ties": serde_json::to_value(crate::agents::Ties::default()).unwrap(),
            "observed": {
                "spend": {
                    "lastTurn": { "input": 2, "output": 4, "cacheRead": 30145, "cacheWrite": 479, "costUsd": 0.00039945 },
                    "session": { "input": 4, "output": 36, "cacheRead": 46707, "cacheWrite": 14062, "costUsd": 0.00329787 }
                },
                "model": "claude-haiku-5-5", "turnEndedMs": 7,
                "unreported": { "input": 0, "output": 9, "cacheRead": 0, "cacheWrite": 0 }, "answered": true
            }
        }]);
        let saved: Vec<Saved> = serde_json::from_value(saved).unwrap();
        let e = taken_up(saved.into_iter().next().unwrap());
        assert_eq!(
            (e.turns_ended, e.observed.model.as_deref(), e.observed.turn_ended_ms, e.observed.answered),
            (3, Some("claude-haiku-5-5"), 7, true)
        );
        let json = serde_json::to_value(&e.observed).unwrap();
        assert!(json.get("spend").is_none() && json.get("unreported").is_none(), "and is written without them");
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

    /// A background helper's comes from the transcript its conversation is in.
    #[test]
    fn a_helpers_context_is_read_off_its_transcript() {
        let account = std::env::temp_dir().join(format!("luna-helper-context-{}", std::process::id()));
        let account = account.to_string_lossy().into_owned();
        test_helper("context-0", &account, None, false);
        assert_eq!(signs("context-0").unwrap().context, None, "no conversation yet");
        test_helper("context-1", &account, Some("sid-context"), false);
        assert_eq!(signs("context-1").unwrap().context, None, "no turn has run");
        let path = crate::claude::session::transcript_file(&account, &account, "sid-context");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let answer = r#"{"type":"assistant","message":{"model":"claude-sonnet-4-5","usage":{"input_tokens":150000},"content":[]}}"#;
        std::fs::write(&path, format!("{answer}\n")).unwrap();
        let s = signs("context-1").unwrap();
        assert_eq!(s.context, Some(Context { pct: 75, tokens: 150_000, window: 200_000 }));
        assert!(serde_json::to_string(&s).unwrap().contains(r#""context":{"pct":75,"tokens":150000,"window":200000}"#));
        lock().entries.remove("context-0");
        lock().entries.remove("context-1");
        let _ = std::fs::remove_dir_all(&account);
    }

    #[test]
    fn a_reroute_names_the_model_of_its_turn_and_the_rollout_the_next() {
        assert_eq!(rerouted_to("model rerouted: gpt-a -> gpt-b (high risk)"), Some("gpt-b"));
        assert_eq!(rerouted_to("model rerouted: gpt-a -> gpt-b"), Some("gpt-b"));
        assert_eq!(rerouted_to("the model is overloaded"), None);

        let dir = std::env::temp_dir().join(format!("luna-reroute-{}", std::process::id()));
        let day = dir.join("sessions").join("2026").join("11").join("01");
        std::fs::create_dir_all(&day).unwrap();
        std::fs::write(
            day.join("rollout-2026-11-01T10-31-15-thread-reroute.jsonl"),
            format!("{}\n", r#"{"timestamp":"t","type":"turn_context","payload":{"model":"gpt-a"}}"#),
        )
        .unwrap();
        let mut h = entry(Provider::Codex);
        h.spec.account_path = dir.to_string_lossy().into_owned();
        h.spec.model = Some("gpt-asked".into());
        h.session = Some("thread-reroute".into());
        lock().entries.insert("reroute-1".into(), h);
        let model = || lock().entries.get("reroute-1").unwrap().observed.model.clone();

        assert_eq!(signs("reroute-1").unwrap().resolved_model, None, "before its first turn");
        // The turn's process could not reach its model: what the rollout names is
        // only what it was to run on.
        turn_over("reroute-1", Some(1));
        assert_eq!(model(), None);
        assert_eq!(signs("reroute-1").unwrap().resolved_model, None, "a failed turn ran on nothing");
        // Nor once the next one starts (its error cleared), or a core takes it up.
        lock().entries.get_mut("reroute-1").unwrap().error = None;
        assert_eq!(signs("reroute-1").unwrap().resolved_model, None, "no model has answered yet");
        lock().entries.get_mut("reroute-1").unwrap().error = None;
        on_codex_event(
            lock().entries.get_mut("reroute-1").unwrap(),
            &json!({ "type": "item.completed", "item": { "id": "item_0", "type": "agent_message", "text": "ok" } }),
        );
        turn_over("reroute-1", Some(0));
        assert_eq!(model().as_deref(), Some("gpt-a"), "the rollout, with no reroute");
        on_codex_event(
            lock().entries.get_mut("reroute-1").unwrap(),
            &json!({ "type": "item.completed", "item": { "id": "item_1", "type": "error", "message": "model rerouted: gpt-a -> gpt-b (high risk)" } }),
        );
        assert_eq!(model().as_deref(), Some("gpt-b"));
        turn_over("reroute-1", Some(0));
        assert_eq!(model().as_deref(), Some("gpt-b"), "the rollout does not take it back");
        turn_over("reroute-1", Some(0));
        assert_eq!(model().as_deref(), Some("gpt-a"), "the next turn asks again");
        assert!(lock().entries.get("reroute-1").unwrap().error.is_none(), "no failure");

        // A rollout that does not read: what it was asked to run on.
        {
            let mut r = lock();
            let e = r.entries.get_mut("reroute-1").unwrap();
            e.observed.model = None;
        }
        assert_eq!(signs("reroute-1").unwrap().resolved_model.as_deref(), Some("gpt-asked"));
        lock().entries.remove("reroute-1");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_model_is_the_one_the_cli_says_it_used() {
        let mut e = entry(Provider::Claude);
        on_claude_event(&mut e, &json!({ "type": "system", "subtype": "init", "session_id": "s", "model": "claude-haiku-0-1" }));
        assert_eq!(e.observed.model, None, "what it was asked for, which may not be there");
        let said = |model: &str, parent: Value| json!({ "type": "assistant", "parent_tool_use_id": parent, "message": { "model": model, "content": [] } });
        on_claude_event(&mut e, &said("<synthetic>", Value::Null));
        assert_eq!(e.observed.model, None, "no model wrote that");
        assert!(!e.observed.acted);
        on_claude_event(&mut e, &said("claude-haiku-5-5", Value::Null));
        assert_eq!(e.observed.model.as_deref(), Some("claude-haiku-5-5"));
        assert!(e.observed.acted);
        on_claude_event(&mut e, &said("claude-sonnet-5-5", json!("toolu_1")));
        assert_eq!(e.observed.model.as_deref(), Some("claude-haiku-5-5"), "a subagent's model is its own");
        on_claude_event(&mut e, &said("claude-sonnet-5-5", Value::Null));
        assert_eq!(e.observed.model.as_deref(), Some("claude-sonnet-5-5"), "a fallback shows");
    }

    /// A turn Claude Code takes up by itself (a background task's end) starts
    /// with an `init` that says where its shell went with `cd`.
    #[test]
    fn where_a_helper_works_does_not_follow_its_shell() {
        // A project and worktree gone from disk, where the shape is all there is.
        let s = |p: &std::path::Path| p.to_string_lossy().into_owned();
        let project = std::env::temp_dir().join(format!("luna-home-of-{}", std::process::id())).join("p");
        let wt = s(&project.join(".claude").join("worktrees").join("joyful-dragonfly"));
        let inside = |dir: &str, parts: &[&str]| s(&parts.iter().fold(std::path::PathBuf::from(dir), |p, part| p.join(part)));
        let mut e = entry(Provider::Claude);
        e.spec.folder = s(&project);
        e.spec.worktree = true;
        let init = |cwd: &str| json!({ "type": "system", "subtype": "init", "session_id": "s", "cwd": cwd });
        on_claude_event(&mut e, &init(&wt));
        on_claude_event(&mut e, &init(&inside(&wt, &["src-tauri", "src"])));
        assert_eq!(e.cwd.as_deref(), Some(wt.as_str()));
        // What an older core saved with the shell's move in it.
        assert_eq!(home_of(&e.spec, inside(&wt, &["src-tauri", "src"])), wt);
        assert_eq!(home_of(&e.spec, wt.clone()), wt);
        let mut plain = entry(Provider::Claude);
        plain.spec.folder = s(&project);
        assert_eq!(home_of(&plain.spec, inside(&plain.spec.folder, &["src"])), plain.spec.folder, "the folder it was sent to");
        let elsewhere = s(&std::env::temp_dir().join("luna-elsewhere"));
        assert_eq!(home_of(&plain.spec, elsewhere.clone()), elsewhere, "nothing to go by");
        // Sent into a folder of a worktree, that folder is where it works.
        plain.spec.folder = inside(&wt, &["docs"]);
        assert_eq!(home_of(&plain.spec, inside(&wt, &["docs", "img"])), plain.spec.folder);
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
    fn signs_say_when_the_turn_began_and_what_it_was_last_seen_doing() {
        test_helper("signs-1", "C:\\acc", Some("s"), false);
        let s = signs("signs-1").unwrap();
        assert_eq!((s.turn_started_at.clone(), s.last_activity_at.clone(), s.context.as_ref()), (None, None, None), "nothing known yet");
        let text = serde_json::to_string(&s).unwrap();
        assert_eq!(text, r#"{"turnStartedAt":null}"#, "null between turns, the rest left out");
        {
            let mut r = lock();
            let e = r.entries.get_mut("signs-1").unwrap();
            e.run = Some(Run::claimed(None, 1_793_000_000_000));
            e.observed.event_ms = 1_793_000_060_000;
        }
        let s = signs("signs-1").unwrap();
        assert_eq!(s.turn_started_at.as_deref(), iso(1_793_000_000_000).as_deref());
        assert_eq!(s.last_activity_at.as_deref(), iso(1_793_000_060_000).as_deref());
        assert!(s.turn_started_at.unwrap().ends_with('Z'));
        lock().entries.remove("signs-1");
    }

    /// What keeps the core: a turn, or between turns what the turns left while
    /// it is at work, by the machine's measure.
    #[test]
    fn leftovers_at_work_keep_the_core_and_long_idle_ones_do_not() {
        let now = 1_793_000_000_000;
        let left = |moved_ms: u64| Leftover {
            pid: 7,
            exe: "cargo.exe".into(),
            started_ms: now - 60_000,
            cpu_ms: 0,
            io_bytes: 0,
            read_ms: now,
            seen_ms: now,
            moved_ms,
        };
        let mut e = entry(Provider::Codex);
        assert!(!leftovers_hold(&e, now), "nothing left");
        e.left_running = true;
        assert!(leftovers_hold(&e, now), "left, not looked at yet");
        e.left = vec![left(now - 60_000)];
        assert!(leftovers_hold(&e, now), "at work a minute ago");
        e.left = vec![left(now - crate::procs::LEFTOVER_HOLD_MS - 1)];
        assert!(!leftovers_hold(&e, now), "idle past the hold: the core may go");
    }

    #[test]
    fn a_finished_helper_left_on_the_list_does_not_keep_a_sync_off_its_account() {
        let path = "/accounts/finished-helper";
        let mut e = entry(Provider::Codex);
        e.spec.account_path = path.into();
        lock().entries.insert("finished-helper".into(), e);
        assert!(uses_account(path));
        assert!(crate::account_settings::sync_gate(Provider::Codex, path).is_ok(), "it has no process to race");
        assert!(crate::account_use::quiet(&[path.to_string()]).is_err(), "a recommendation still goes by the session");
        let on_it = crate::account_use::running_on(path);
        assert!(crate::account_settings::sync_gate(Provider::Codex, path).is_err(), "its next turn does");
        drop(on_it);
        lock().entries.remove("finished-helper");
    }

    pub(super) fn entry(provider: Provider) -> Entry {
        Entry {
            spec: Spec {
                provider,
                account_path: "C:\\acc".into(),
                folder: "C:\\p".into(),
                worktree: false,
                worktree_name: None,
                model: Some("haiku".into()),
                effort: "low".into(),
                mode: "read-only".into(),
                auto_review: false,
                tools: false,
                hooks_unasked: false,
                read_only: false,
                agent: None,
                parent: None,
                token: None,
            },
            started_ms: 0,
            session: None,
            cwd: None,
            run: None,
            turns_ended: 0,
            queue: Vec::new(),
            in_flight: Vec::new(),
            fed: Vec::new(),
            log: VecDeque::new(),
            held: VecDeque::new(),
            error: None,
            cut_short: false,
            undelivered: Vec::new(),
            budgets_landed: Vec::new(),
            tied: false,
            resumable: false,
            unsure: false,
            opening: None,
            opening_id: None,
            job: None,
            left_running: false,
            left: Vec::new(),
            ending_left: false,
            promoting: false,
            observed: Observed::default(),
        }
    }

    /// An entry in a turn with no process yet, so nothing real can be killed.
    fn in_claimed_turn(id: &str) {
        let mut e = entry(Provider::Claude);
        claim(&mut e);
        lock().entries.insert(id.into(), e);
    }

    #[test]
    fn two_prompts_at_once_are_both_held_and_answered_in_order() {
        in_claimed_turn("two-asks");
        let ask = |tool: &'static str| {
            std::thread::spawn(move || permission("two-asks", &json!({ "tool_name": tool, "input": { "command": tool } })))
        };
        let held = || lock().entries.get("two-asks").unwrap().held.len();
        let until = |n: usize| {
            for _ in 0..200 {
                if held() == n {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            panic!("never {n} held");
        };
        let first = ask("Bash");
        until(1);
        let second = ask("Edit");
        until(2);
        let asked = super::ask("two-asks").unwrap();
        assert_eq!(asked.tool.as_deref(), Some("Bash"), "the one that came first");
        assert_eq!(turn_state("two-asks").unwrap().turn, crate::activity::Turn::Waiting);
        assert!(!turn_holds(&turn_state("two-asks").unwrap()), "held: waiting, keeping nothing awake");
        let stale = asked.id.clone().unwrap();
        answer("two-asks", Some(&stale), &[], Some(true), None).unwrap();
        reg().1.notify_all();
        assert_eq!(first.join().unwrap()["behavior"], "allow", "the second did not cost it its answer");
        let next = super::ask("two-asks").unwrap();
        assert_eq!(next.tool.as_deref(), Some("Edit"), "and then the next");
        assert_ne!(next.id, asked.id);
        assert_eq!(turn_state("two-asks").unwrap().turn, crate::activity::Turn::Waiting, "still held on that one");
        // A second responder who also saw the first, its answer late.
        let late = answer("two-asks", Some(&stale), &[], Some(true), None).unwrap_err();
        assert!(late.contains("no longer open"), "{late}");
        // An id it never had: none it was given, or another helper's.
        for never in ["a0", "nonsense"] {
            let e = answer("two-asks", Some(never), &[], Some(true), None).unwrap_err();
            assert!(e.contains("unknown") && !e.contains("no longer open"), "{e}");
        }
        lock().asked.insert(u64::MAX, "another".into());
        assert!(answer("two-asks", Some(&format!("a{}", u64::MAX)), &[], Some(true), None).unwrap_err().contains("unknown"));
        lock().asked.remove(&u64::MAX);
        assert_eq!(super::ask("two-asks").unwrap().id, next.id, "the next is untouched");
        answer("two-asks", None, &[], Some(false), Some("not that")).unwrap();
        let no = second.join().unwrap();
        assert_eq!((no["behavior"].as_str(), no["message"].as_str()), (Some("deny"), Some("not that")));
        assert_eq!(super::ask("two-asks"), None);
        assert_eq!(turn_state("two-asks").unwrap().turn, crate::activity::Turn::Busy);
        lock().entries.remove("two-asks");
    }

    #[test]
    fn with_several_held_an_answer_says_which() {
        in_claimed_turn("which-ask");
        let ask = |tool: &'static str| std::thread::spawn(move || permission("which-ask", &json!({ "tool_name": tool, "input": {} })));
        let first = ask("Bash");
        for _ in 0..200 {
            if lock().entries.get("which-ask").unwrap().held.len() == 1 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let second = ask("Edit");
        for _ in 0..200 {
            if lock().entries.get("which-ask").unwrap().held.len() == 2 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let unsaid = answer("which-ask", None, &[], Some(true), None).unwrap_err();
        assert!(unsaid.contains("askId") && unsaid.contains("Bash"), "says how, and what is first: {unsaid}");
        assert!(lock().entries.get("which-ask").unwrap().held.iter().all(|h| h.answer.is_none()), "and answers nothing");
        // One named out of turn is the one it means.
        let last = lock().entries.get("which-ask").unwrap().held.back().unwrap().ask.id.clone().unwrap();
        answer("which-ask", Some(&last), &[], Some(false), None).unwrap();
        reg().1.notify_all();
        assert_eq!(second.join().unwrap()["behavior"], "deny");
        assert_eq!(super::ask("which-ask").unwrap().tool.as_deref(), Some("Bash"));
        // A single ask is answered with no id, as before.
        answer("which-ask", None, &[], Some(true), None).unwrap();
        reg().1.notify_all();
        assert_eq!(first.join().unwrap()["behavior"], "allow");
        assert!(answer("which-ask", None, &[], Some(true), None).unwrap_err().contains("not waiting"));
        lock().entries.remove("which-ask");
    }

    #[test]
    fn an_ask_says_its_id_to_whoever_is_to_answer_it() {
        in_claimed_turn("id-ask");
        let held = std::thread::spawn(|| permission("id-ask", &json!({ "tool_name": "Bash", "input": { "command": "ls" } })));
        for _ in 0..200 {
            if ask("id-ask").is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let shown = serde_json::to_value(ask("id-ask").unwrap()).unwrap();
        assert!(shown["id"].as_str().is_some_and(|i| !i.is_empty()), "{shown}");
        kill("id-ask");
        let _ = held.join();
        lock().entries.remove("id-ask");
        assert!(serde_json::to_value(Ask::permission(None, None)).unwrap().get("id").is_none(), "a terminal's has none");
    }

    #[test]
    fn a_kill_lets_every_held_prompt_go() {
        in_claimed_turn("kill-asks");
        let ask = |tool: &'static str| std::thread::spawn(move || permission("kill-asks", &json!({ "tool_name": tool, "input": {} })));
        let (a, b) = (ask("Bash"), ask("Edit"));
        for _ in 0..200 {
            if lock().entries.get("kill-asks").unwrap().held.len() == 2 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        kill("kill-asks");
        assert_eq!(a.join().unwrap()["behavior"], "deny");
        assert_eq!(b.join().unwrap()["behavior"], "deny");
        lock().entries.remove("kill-asks");
    }

    #[test]
    fn an_interrupt_keeps_the_queue_and_counts_a_turn_with_no_process_yet() {
        in_claimed_turn("int-claimed");
        lock().entries.get_mut("int-claimed").unwrap().queue.push("from another caller".into());
        // No process turns up to stop: the text waits behind the turn, which
        // it ends a turn after, and nothing is lost.
        let i = interrupt("int-claimed", "from this one", None).unwrap();
        assert_eq!(i, Interrupted { stopped: false, queued: true });
        assert_eq!(lock().entries.get("int-claimed").unwrap().queue, ["from another caller", "from this one"]);
        // When that turn is over, they go together, in the order they came,
        // and an empty queue in the meantime says the text has been taken up.
        let text = joined(&std::mem::take(&mut lock().entries.get_mut("int-claimed").unwrap().queue));
        assert_eq!(text, "from another caller\n\nfrom this one");
        lock().entries.remove("int-claimed");
    }

    #[test]
    fn notes_are_open_to_a_helper_with_tools() {
        let mut e = entry(Provider::Claude);
        let has_dir = |e: &Entry| command_for(e, "x").0.iter().any(|a| a == "--add-dir");
        assert!(!has_dir(&e), "one with no tools gets no notices");
        e.spec.tools = true;
        assert!(has_dir(&e));
        let (args, _, _) = command_for(&e, "x");
        let at = args.iter().position(|a| a == "--add-dir").unwrap();
        assert_eq!(args[at + 1], crate::agents::notes_dir().to_string_lossy());
        let mut codex = entry(Provider::Codex);
        codex.spec.tools = true;
        assert!(!has_dir(&codex), "Codex's sandboxes read anywhere");
    }

    #[cfg(windows)]
    use crate::procs::testkit::{burner, ended_within, hidden, idler, job_of, Kids};

    /// `--agent no-such-agent`: Claude Code exits at once, and a `git` it had
    /// started runs on in the helper's job for minutes. Only what is the CLI's by
    /// descent goes: an earlier turn's job, and what that starts, stay.
    #[cfg(windows)]
    #[test]
    fn what_a_cli_gone_before_its_model_did_anything_started_goes_with_its_turn() {
        let pause = || std::thread::sleep(std::time::Duration::from_millis(60));
        // A job an earlier turn left running.
        let mut kids = Kids(vec![idler(30)]);
        let job = job_of(&kids.0[0]);
        pause();
        let since = crate::throttle::now_ms();
        pause();
        // This turn's CLI, which starts a ping of its own and goes.
        kids.0.push(hidden("cmd", &["/d", "/c", "ping -n 30 127.0.0.1 >nul"]));
        let cli = kids.0[1].id();
        assert!(crate::procs::join(&job, cli));
        let mut its = None;
        for _ in 0..40 {
            its = crate::procs::left_in(&job)
                .into_iter()
                .find(|(r, n)| n.eq_ignore_ascii_case("ping.exe") && r.pid != kids.0[0].id())
                .map(|(r, _)| r.pid);
            if its.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let its = its.expect("the CLI started its ping");
        let _ = kids.0[1].kill();
        let _ = kids.0[1].wait();
        let mut e = entry(Provider::Claude);
        e.job = Some(job.clone());
        e.run = Some(Run::claimed(Some(cli), since));
        lock().entries.insert("strays".into(), e);
        turn_over("strays", Some(1));
        let alive = |pid: u32| crate::procs::children(Some(&*job), None).iter().any(|r| r.pid == pid);
        for _ in 0..30 {
            if !alive(its) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(!alive(its), "the CLI's own, started this turn");
        assert!(!ended_within(&mut kids.0[0], 300), "an earlier turn's job runs on");
        assert!(turn_state("strays").unwrap().children_busy, "and is what keeps it busy");
        // Its model at work: what the turn starts is its job.
        kids.0.push(idler(30));
        assert!(crate::procs::join(&job, kids.0[2].id()));
        {
            let mut r = lock();
            let e = r.entries.get_mut("strays").unwrap();
            e.run = Some(Run::claimed(Some(cli), since));
            e.observed.acted = true;
        }
        turn_over("strays", Some(0));
        assert!(!ended_within(&mut kids.0[2], 300));
        lock().entries.remove("strays");
    }

    /// Born during a turn that came to nothing, but of what an earlier turn
    /// left, or of a launcher since gone: nothing says it is the CLI's, and it
    /// is spared.
    #[cfg(windows)]
    #[test]
    fn only_what_is_the_clis_by_descent_is_ended_with_its_turn() {
        // `cmd` starting a ping is a leftover starting a worker.
        let mut kids = Kids(vec![hidden("cmd", &["/d", "/c", "ping -n 30 127.0.0.1 >nul"])]);
        let job = job_of(&kids.0[0]);
        let since = crate::throttle::now_ms().saturating_sub(5_000);
        let mut worker = None;
        for _ in 0..40 {
            worker = crate::procs::left_in(&job).into_iter().find(|(_, n)| n.eq_ignore_ascii_case("ping.exe")).map(|(r, _)| r.pid);
            if worker.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let worker = worker.expect("cmd started its ping");
        let alive = |pid: u32| crate::procs::children(Some(&*job), None).iter().any(|r| r.pid == pid);
        // Another CLI's: the worker is not of it.
        let ended = crate::procs::end_tree_of(&job, u32::MAX - 1, since);
        assert!(ended.is_empty() && alive(worker), "{ended:?}");
        // Its launcher gone: no way up, so nothing says whose it is.
        let _ = kids.0[0].kill();
        let _ = kids.0[0].wait();
        assert!(crate::procs::end_tree_of(&job, u32::MAX - 1, since).is_empty() && alive(worker));
        // Of the CLI by descent, as that gone launcher's: it goes.
        let ended = crate::procs::end_tree_of(&job, kids.0[0].id(), since);
        assert!(ended.iter().any(|n| n.eq_ignore_ascii_case("ping.exe")), "{ended:?}");
    }

    /// A job left between turns holds the machine until it has done nothing for
    /// long, `luna_list` says what of it is at work, and a stop between turns ends
    /// all of it.
    #[cfg(windows)]
    #[test]
    fn a_leftover_holds_the_machine_until_long_idle_and_a_stop_ends_it() {
        let mut kids = Kids(vec![idler(120), burner(60)]);
        let job = job_of(&kids.0[0]);
        assert!(crate::procs::join(&job, kids.0[1].id()));
        let mut e = entry(Provider::Claude);
        e.turns_ended = 1;
        e.job = Some(job.clone());
        e.observed.turn_ended_ms = crate::throttle::now_ms();
        lock().entries.insert("left-work".into(), e);
        let sample = || activity().into_iter().find(|a| a.id == "left-work").unwrap();
        let first = sample();
        assert!(first.busy, "just left: as quiet as its turn's end, which is no time ago");
        assert!(turn_state("left-work").unwrap().children_busy);
        // As if both had sat there for longer than LEFTOVER_HOLD_MS since, and
        // were last read a window ago: the next sample takes their rates.
        for l in lock().entries.get_mut("left-work").unwrap().left.iter_mut() {
            l.moved_ms = l.moved_ms.saturating_sub(crate::procs::LEFTOVER_HOLD_MS + 60_000);
            l.read_ms = l.read_ms.saturating_sub(crate::procs::LEFTOVER_WINDOW_MS);
        }
        std::thread::sleep(std::time::Duration::from_millis(2_000));
        let now = sample();
        assert!(now.busy, "the loop burns a core");
        assert!(
            now.procs.len() == 1 && now.procs[0].to_ascii_lowercase().starts_with("powershell.exe"),
            "and is all that holds the machine, the long idle ping not: {:?}",
            now.procs
        );
        let shown = signs("left-work").unwrap().left_running;
        let active = |exe: &str| shown.iter().find(|l| l.exe.eq_ignore_ascii_case(exe)).map(|l| l.active);
        assert_eq!((active("powershell.exe"), active("PING.EXE")), (Some(true), Some(false)), "{shown:?}");
        assert!(shown.iter().all(|l| l.started_at.is_some()));
        assert!(left_holds("left-work"));
        // A stop between turns is for all that is left; a send meanwhile waits for it.
        let ended = kill("left-work");
        for exe in ["ping.exe", "powershell.exe"] {
            assert!(ended.iter().any(|n| n.eq_ignore_ascii_case(exe)), "{exe}: {ended:?}");
        }
        assert!(!turn_state("left-work").unwrap().children_busy);
        assert!(signs("left-work").unwrap().left_running.is_empty());
        assert!(ended_within(&mut kids.0[0], 3000) && ended_within(&mut kids.0[1], 3000));
        lock().entries.remove("left-work");
    }

    /// A send while a stop between turns ends what is left queues: a turn's CLI
    /// started meanwhile would land in the job being emptied, and be ended too.
    #[test]
    fn a_send_during_a_stop_between_turns_waits_for_it() {
        let mut e = entry(Provider::Claude);
        e.ending_left = true;
        lock().entries.insert("stopping".into(), e);
        assert_eq!(send("stopping", "go on", None), Ok(Placed::Queued));
        assert!(lock().entries.get("stopping").unwrap().run.is_none(), "no turn started");
        assert_eq!(interrupt("stopping", "now", None).map(|i| i.queued), Ok(true));
        assert_eq!(lock().entries.get("stopping").unwrap().queue.len(), 2);
        // A second stop meanwhile has nothing to end and leaves the first's mark.
        assert!(kill("stopping").is_empty());
        assert!(lock().entries.get("stopping").unwrap().ending_left, "still the first stop's");
        assert_eq!(send("stopping", "and this", None), Ok(Placed::Queued), "still queued");
        lock().entries.remove("stopping");
    }

    /// Long idle, its leftovers hold nothing, and an idle helper with only them
    /// left is as idle as one with nothing (`agents::reap`).
    #[test]
    fn leftovers_idle_past_their_grace_hold_nothing() {
        idle_for_test("held-long", Provider::Claude, 1, false);
        left_for_test("held-long", &[("tail.exe", 22104, crate::procs::LEFTOVER_HOLD_MS - 60_000)]);
        assert!(left_holds("held-long"));
        left_for_test("held-long", &[("tail.exe", 22104, crate::procs::LEFTOVER_HOLD_MS + 60_000)]);
        assert!(!left_holds("held-long"));
        lock().entries.remove("held-long");
    }

    #[test]
    fn a_job_left_running_reads_as_one_between_turns_only() {
        let mut e = entry(Provider::Claude);
        e.turns_ended = 1;
        e.left_running = true;
        lock().entries.insert("left-job".into(), e);
        let children_busy = || turn_state("left-job").unwrap().children_busy;
        assert!(children_busy(), "between turns, what its turn left is a job");
        lock().entries.get_mut("left-job").unwrap().run = Some(Run::claimed(Some(1), 0));
        assert!(!children_busy(), "in a turn it is simply working");
        lock().entries.remove("left-job");
    }

    #[test]
    fn a_helper_saved_mid_turn_comes_back_between_turns_with_that_turn_ended() {
        let mut spec = entry(Provider::Claude).spec;
        spec.token = Some("secret".into());
        let json = serde_json::to_value(&spec).unwrap();
        assert!(json.get("token").is_none(), "a token is never saved");
        let saved = |running, queued| Saved {
            id: "c1".into(),
            spec: serde_json::from_value(json.clone()).unwrap(),
            started_ms: 5,
            session: Some("sid".into()),
            cwd: None,
            turns_ended: 3,
            running,
            queued,
            queue: if queued { vec!["first".into(), "second".into()] } else { Vec::new() },
            in_flight: Vec::new(),
            fed: Vec::new(),
            undelivered: Vec::new(),
            budgets_landed: Vec::new(),
            tied: false,
            unsure: false,
            opening: None,
            opening_id: None,
            cut_short: false,
            ties: Default::default(),
            observed: Default::default(),
        };

        let e = taken_up(saved(true, true));
        assert!(e.run.is_none() && e.spec.token.is_none());
        assert_eq!(e.turns_ended, 5, "the turn stopped and the text queued behind it");
        assert!(e.cut_short && e.error.is_some());
        assert_eq!(e.session.as_deref(), Some("sid"));
        assert_eq!(e.undelivered, ["first", "second"], "kept word for word, none of it queued for a turn");
        assert!(e.queue.is_empty());

        let e = taken_up(saved(false, false));
        assert_eq!(e.turns_ended, 3);
        assert!(!e.cut_short && e.error.is_none());

        // A batch a turn was claimed for and never got comes back in its place.
        let e = taken_up(Saved { in_flight: vec!["flying".into()], ..saved(true, true) });
        assert_eq!(e.undelivered, ["flying", "first", "second"]);

        // A turn no core stopped is over only if the OS saw to it.
        assert!(!taken_up(saved(true, false)).resumable, "no tie: its CLI may be running yet");
        assert!(taken_up(Saved { tied: true, ..saved(true, false) }).resumable);
        assert!(taken_up(Saved { cut_short: true, ..saved(true, false) }).resumable, "the last core stopped it itself");
        assert!(taken_up(saved(false, false)).resumable);

        // Not known to have ended stays so through the next core, which reads the
        // `cut_short` this one saved as if that core had stopped it, until its next
        // turn or a later look says.
        let first = taken_up(saved(true, false));
        assert!(first.unsure && first.cut_short && first.run.is_none());
        lock().entries.insert("unsure-across".into(), first);
        let on_disk = snapshot().into_iter().find(|s| s.id == "unsure-across").unwrap();
        lock().entries.remove("unsure-across");
        assert!(on_disk.unsure && on_disk.cut_short && !on_disk.running && !on_disk.tied);
        let second = taken_up(on_disk);
        assert!(!second.resumable && second.unsure, "as it was");
        let cleared = taken_up(Saved { unsure: false, cut_short: true, ..saved(false, false) });
        assert!(cleared.resumable && !cleared.unsure);
        assert!(
            !serde_json::to_value(saved(false, false)).unwrap().as_object().unwrap().contains_key("unsure"),
            "and nothing in the file otherwise"
        );
    }

    #[test]
    fn a_batch_on_its_way_is_kept_if_the_core_stops_it_and_goes_if_a_kill_does() {
        let mut e = entry(Provider::Codex);
        e.run = Some(Run::claimed(None, 0));
        e.in_flight = vec!["flying".into()];
        e.queue = vec!["behind it".into()];
        stop_for_luna(&mut e);
        assert_eq!(e.undelivered, ["flying", "behind it"]);
        assert!(e.in_flight.is_empty());

        let mut e = entry(Provider::Codex);
        e.in_flight = vec!["flying".into()];
        stop(&mut e);
        assert!(e.in_flight.is_empty() && e.undelivered.is_empty());
    }

    #[test]
    fn a_resume_nobody_can_vouch_for_is_not_tried_and_one_takes_only_its_batch() {
        take_up_for_test("unsure", "C:\\luna-no-such-folder", 3, &["queued"], None);
        lock().entries.get_mut("unsure").unwrap().resumable = false;
        resume("unsure");
        let e = lock().entries.remove("unsure").unwrap();
        assert_eq!(e.turns_ended, 5, "no turn tried");
        assert_eq!(e.undelivered, ["queued"]);

        // What is put by while a turn is being started stays, behind what failed to go.
        take_up_for_test("batch", "C:\\luna-no-such-folder", 3, &["queued"], None);
        resume("batch");
        lock().entries.get_mut("batch").unwrap().undelivered.push("later".into());
        put_back("batch", vec!["queued again".into()]);
        let e = lock().entries.remove("batch").unwrap();
        assert_eq!(e.undelivered, ["queued again", "queued", "later"]);
    }

    #[test]
    fn a_file_an_older_core_left_still_reads() {
        let spec = serde_json::to_value(&entry(Provider::Codex).spec).unwrap();
        let s: Saved = serde_json::from_value(json!({
            "id": "c1", "spec": spec, "startedMs": 5, "session": "t", "cwd": null, "turnsEnded": 3,
            "running": true, "queued": true, "cutShort": false, "ties": {
                "parent": null, "name": null, "settings": {}, "tools": false, "borrowed": false, "autoDelete": false, "owed": [], "seen": [],
            },
        }))
        .unwrap();
        let e = taken_up(s);
        assert_eq!(e.turns_ended, 5, "what it queued is counted though its words are gone");
        assert!(e.undelivered.is_empty());
    }

    #[test]
    fn what_a_stopped_turn_had_queued_is_kept_and_counted() {
        let mut e = entry(Provider::Codex);
        e.run = Some(Run::claimed(None, 0));
        e.queue = vec!["one".into(), "two".into()];
        stop_for_luna(&mut e);
        assert_eq!(e.undelivered, ["one", "two"]);
        assert!(e.queue.is_empty() && e.cut_short);
        assert_eq!(e.turns_ended, 1, "whoever queued it waits on an end that will not come");
    }

    #[test]
    fn a_kill_drops_what_was_queued_as_it_did() {
        let mut e = entry(Provider::Codex);
        e.run = Some(Run::claimed(None, 0));
        e.queue = vec!["one".into()];
        assert_eq!(stop(&mut e), None);
        assert!(e.queue.is_empty() && e.undelivered.is_empty() && !e.cut_short, "the agent's own stop, not Luna's");
    }

    #[test]
    fn a_helper_stopped_before_it_named_its_conversation_keeps_what_it_was_given() {
        // Saved as such only while no turn has named it.
        let mut e = entry(Provider::Claude);
        e.opening = Some("the prompt".into());
        lock().entries.insert("unnamed".into(), e);
        let saved = |id: &str| snapshot().into_iter().find(|s| s.id == id).unwrap();
        assert_eq!(saved("unnamed").opening.as_deref(), Some("the prompt"));
        on_claude_event(lock().entries.get_mut("unnamed").unwrap(), &json!({ "type": "system", "subtype": "init", "session_id": "s-1" }));
        assert_eq!(saved("unnamed").opening, None, "the conversation holds it now");
        lock().entries.remove("unnamed");

        take_up_for_test("never", "C:\\luna-no-such-folder", 0, &["then this"], Some("the prompt"));
        assert!(never_started("never"));
        assert_eq!(undelivered_for("never", "its parent", true), ["the prompt", "then this"], "what it was started on goes first");
        // A first turn again: no conversation to resume, its words as they were.
        let (args, _, _) = command_for(lock().entries.get("never").unwrap(), "never");
        assert!(!args.contains(&"--resume".to_string()));
        resume("never");
        let e = lock().entries.remove("never").unwrap();
        assert_eq!(e.undelivered, ["the prompt", "then this"], "it could not start: kept");
        assert!(e.cut_short);

        // Stopped by nothing: a first turn that failed by itself has lost nothing to the core.
        let s = serde_json::from_value::<Saved>(json!({
            "id": "c1", "spec": serde_json::to_value(&entry(Provider::Claude).spec).unwrap(), "startedMs": 5, "session": null, "cwd": null,
            "turnsEnded": 1, "running": false, "queued": false, "opening": "the prompt", "cutShort": false, "ties": {
                "parent": null, "name": null, "settings": {}, "tools": false, "borrowed": false, "autoDelete": false, "owed": [], "seen": [],
            },
        }))
        .unwrap();
        let e = taken_up(s);
        assert!(e.undelivered.is_empty() && !e.cut_short);
    }

    #[test]
    fn a_resumed_turn_is_told_why_and_given_what_was_queued() {
        let text = resume_text(&[]);
        assert!(text.starts_with(crate::agents::NOTICE_TAG) && text.contains("Carry on where you left off"));
        assert!(!text.contains("never reached you"));
        let text = resume_text(&["fix the test\nand push".into(), "also the docs".into()]);
        let (first, second) = (text.find("fix the test\nand push").unwrap(), text.find("also the docs").unwrap());
        assert!(text.contains("never reached you") && first < second, "word for word, oldest first: {text}");
    }

    #[test]
    fn a_resume_finds_a_turn_sent_meanwhile_has_gone_on_already() {
        take_up_for_test("resume-late", "C:\\luna-no-such-folder", 3, &[], None);
        lock().entries.get_mut("resume-late").unwrap().cut_short = false;
        resume("resume-late");
        let e = lock().entries.remove("resume-late").unwrap();
        assert_eq!(e.turns_ended, 4, "no second turn tried");
    }

    #[test]
    fn a_resume_that_cannot_start_leaves_the_helper_cut_short_with_its_text() {
        take_up_for_test("resume-fail", "C:\\luna-no-such-folder", 3, &["queued"], None);
        resume("resume-fail");
        let e = lock().entries.remove("resume-fail").unwrap();
        assert!(e.run.is_none() && e.cut_short && e.error.is_some());
        assert_eq!(e.undelivered, ["queued"], "still the caller's to resend");
        assert_eq!(e.turns_ended, 6, "the stopped turn, what was queued behind it, the turn that could not start");
    }

    #[test]
    fn a_turn_resumes_the_conversation_the_first_one_named() {
        let mut e = entry(Provider::Codex);
        let (args, dir, _) = command_for(&e, "x");
        assert_eq!(args[..3], ["exec", "-C", "C:\\p"]);
        assert_eq!(args.last().map(String::as_str), Some("-"), "the prompt comes on stdin");
        assert!(args.contains(&"sandbox_mode=\"read-only\"".to_string()));
        assert_eq!(dir, "C:\\p");
        on_codex_event(&mut e, &json!({ "type": "thread.started", "thread_id": "t-1" }));
        let (args, _, _) = command_for(&e, "x");
        assert_eq!(args[..3], ["exec", "resume", "t-1"]);

        let mut e = entry(Provider::Claude);
        on_claude_event(&mut e, &json!({ "type": "system", "subtype": "init", "session_id": "s-1", "cwd": "C:\\p" }));
        let (args, _, envs) = command_for(&e, "x");
        assert_eq!(args[..3], ["-p", "--input-format", "stream-json"]);
        let at = args.iter().position(|a| a == "--resume").unwrap();
        assert_eq!(args[at + 1], "s-1");
        assert!(envs.iter().any(|(k, v)| *k == "CLAUDE_CONFIG_DIR" && v == "C:\\acc"));
    }

    #[test]
    fn a_helpers_worktree_goes_by_the_name_luna_gave_it_never_as_an_argument_of_its_own() {
        const FLAG: &str = "--dangerously-skip-permissions";
        // Named, with `=`, from the project.
        let mut e = entry(Provider::Claude);
        e.spec.worktree = true;
        e.spec.worktree_name = Some("luna-x".into());
        let (args, dir, _) = command_for(&e, "x");
        assert!(args.contains(&"--worktree=luna-x".to_string()) && !args.contains(&"--worktree".to_string()));
        assert_eq!(dir, "C:\\p");
        assert!(!args.iter().any(|a| a == "--resume"), "there is no conversation to resume");

        // A name that is a flag turns permissions off, as Claude Code reads it, if it
        // ever gets to be an argument: not from the spec, and not from where it worked.
        e.spec.worktree_name = Some(FLAG.into());
        let (args, _, _) = command_for(&e, "x");
        assert!(args.contains(&"--worktree".to_string()) && !args.iter().any(|a| a.contains(FLAG)), "{args:?}");
        e.session = Some("s-1".into());
        e.cwd = Some(std::path::Path::new("C:\\no\\such").join(FLAG).to_string_lossy().into_owned());
        let (args, _, _) = command_for(&e, "x");
        assert!(args.contains(&"--resume".to_string()) && !args.iter().any(|a| a.contains(FLAG)), "{args:?}");
        e.cwd = Some(std::path::Path::new("C:\\no\\such").join("luna-x").to_string_lossy().into_owned());
        assert!(command_for(&e, "x").0.contains(&"--worktree=luna-x".to_string()), "gone from disk, Claude Code makes it again by name");

        // Codex names none, and always makes its own.
        let mut e = entry(Provider::Codex);
        e.spec.worktree = true;
        let (args, _, _) = command_for(&e, "x");
        assert_eq!(args[..3], ["exec", "-C", "C:\\p"]);
        assert!(args.contains(&"--worktree".to_string()));

        // No core has cut off a first turn that is only just starting.
        let mut e = entry(Provider::Claude);
        e.spec.worktree = true;
        e.spec.worktree_name = Some("luna-first".into());
        lock().entries.insert("first-turn".into(), e);
        settle_worktree("first-turn");
        assert_eq!(lock().entries.remove("first-turn").unwrap().spec.worktree_name.as_deref(), Some("luna-first"));
    }

    fn value_of<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
        args.iter().position(|a| a == flag).map(|at| args[at + 1].as_str())
    }

    #[test]
    fn a_codex_helpers_note_goes_with_every_turn() {
        let mut e = entry(Provider::Codex);
        e.spec.parent = Some("p-1".into());
        let notes = |text: String| text.matches("A note from Luna").count();
        assert_eq!(notes(stdin_text(&e, "the prompt")), 1, "the turn that starts its conversation");
        assert_eq!(stdin_text(&e, "/review"), "/review", "not into a command's arguments");
        e.session = Some("t-1".into());
        assert_eq!(notes(stdin_text(&e, "Carry on where you left off.")), 1, "and each one after: a compaction loses the first");
        let mut e = entry(Provider::Claude);
        e.spec.parent = Some("p-1".into());
        assert_eq!(stdin_text(&e, "the prompt"), "the prompt", "its system prompt says it there");
    }

    #[test]
    fn a_core_after_this_runs_a_helper_as_it_was_spawned() {
        // Through the file: what `save` writes and `restore` reads.
        let through_a_core = |mut e: Entry, session: Option<&str>| {
            e.session = session.map(str::to_owned);
            let spec: Spec = serde_json::from_slice(&serde_json::to_vec(&e.spec).unwrap()).unwrap();
            taken_up(Saved {
                id: "x".into(),
                spec,
                started_ms: 0,
                session: e.session.clone(),
                cwd: None,
                turns_ended: 1,
                running: true,
                queued: false,
                queue: Vec::new(),
                in_flight: Vec::new(),
                fed: Vec::new(),
                undelivered: Vec::new(),
                budgets_landed: Vec::new(),
                tied: true,
                unsure: false,
                opening: Some("the prompt".into()),
                opening_id: None,
                cut_short: false,
                observed: Default::default(),
                ties: Default::default(),
            })
        };
        let mut claude = entry(Provider::Claude);
        claude.spec.read_only = true;
        claude.spec.agent = Some("checker".into());
        claude.spec.parent = Some("p-1".into());
        let mut codex = entry(Provider::Codex);
        codex.spec.mode = crate::pty::READ_ONLY_NET.into();
        codex.spec.parent = Some("p-1".into());
        // One that goes on and one that never got started alike.
        for session in [Some("s-1"), None] {
            let e = through_a_core(entry_like(&claude), session);
            let (args, _, envs) = command_for(&e, "x");
            assert_eq!(value_of(&args, "--agent"), Some("checker"));
            assert_eq!(value_of(&args, "--disallowedTools"), Some("Edit,Write,NotebookEdit"));
            assert_eq!(value_of(&args, "--resume"), session);
            assert!(value_of(&args, "--append-system-prompt").unwrap().contains("by session p-1"));
            assert!(envs.iter().any(|(k, v)| *k == "LUNA_PARENT_ID" && v == "p-1"));

            let e = through_a_core(entry_like(&codex), session);
            let (args, _, envs) = command_for(&e, "x");
            assert!(args.iter().any(|a| a.starts_with("default_permissions=\"luna-")), "{args:?}");
            assert_eq!(args.get(1).map(String::as_str) == Some("resume"), session.is_some());
            assert!(envs.iter().any(|(k, v)| *k == "LUNA_PARENT_ID" && v == "p-1"));
            // The caller gets back what it asked, the note going on as that turn starts.
            if session.is_none() {
                assert_eq!(e.undelivered, ["the prompt"]);
                assert_eq!(stdin_text(&e, &e.undelivered[0].text).matches("A note from Luna").count(), 1);
            }
        }
    }

    fn entry_like(e: &Entry) -> Entry {
        let mut new = entry(e.spec.provider);
        new.spec = e.spec.clone();
        new
    }

    #[test]
    fn a_helper_passed_to_another_parent_is_told_on_its_next_turn() {
        for provider in [Provider::Claude, Provider::Codex] {
            let mut e = entry(provider);
            e.spec.parent = Some("old".into());
            e.session = Some("s-1".into());
            lock().entries.insert("moved".into(), e);
            set_parent("moved", Some("new".into()));
            let r = lock();
            let e = &r.entries["moved"];
            let (args, _, envs) = command_for(e, "moved");
            assert!(envs.iter().any(|(k, v)| *k == "LUNA_PARENT_ID" && v == "new"), "{provider:?}");
            let line = stdin_text(e, "go on");
            match provider {
                Provider::Claude => {
                    assert!(value_of(&args, "--append-system-prompt").unwrap().contains("by session new"));
                    assert_eq!(line, "go on", "its system prompt says it");
                }
                Provider::Codex => {
                    assert!(line.starts_with("go on") && line.contains("by session new"), "{line}");
                    assert_eq!(line.matches("A note from Luna").count(), 1);
                }
            }
            drop(r);
            // With nobody to answer to, it is told of nobody.
            set_parent("moved", None);
            let e = lock().entries.remove("moved").unwrap();
            let (args, _, envs) = command_for(&e, "moved");
            assert!(!envs.iter().any(|(k, _)| *k == "LUNA_PARENT_ID"));
            assert!(!args.iter().any(|a| a == "--append-system-prompt"));
            assert_eq!(stdin_text(&e, "go on"), "go on");
        }
    }

    #[test]
    fn read_only_net_goes_to_codex_as_a_profile_on_every_turn() {
        let mut e = entry(Provider::Codex);
        e.spec.mode = crate::pty::READ_ONLY_NET.into();
        for session in [None, Some("t-1".to_string())] {
            e.session = session;
            let (args, _, envs) = command_for(&e, "x");
            assert!(args.iter().any(|a| a.starts_with("default_permissions=\"luna-")));
            assert!(args.iter().any(|a| a.ends_with(".network.enabled=true")));
            assert!(!args.iter().any(|a| a.starts_with("sandbox_mode=")), "Codex's docs say not to set both");
            assert_eq!(envs.iter().any(|(k, _)| *k == "GIT_CONFIG_COUNT"), cfg!(windows), "git https, which schannel cannot do there");
            assert!(args.contains(&"approval_policy=\"never\"".to_string()));
            assert!(args.contains(&crate::pty::CODEX_PERSON_REVIEWS.to_string()), "no reviewer model from a config");
        }
    }

    #[test]
    fn read_only_takes_the_edit_tools_away_from_claude() {
        let mut e = entry(Provider::Claude);
        let (args, _, _) = command_for(&e, "x");
        assert_eq!(value_of(&args, "--disallowedTools"), None);
        e.spec.read_only = true;
        let (args, _, _) = command_for(&e, "x");
        assert_eq!(value_of(&args, "--disallowedTools"), Some("Edit,Write,NotebookEdit"));
        assert_eq!(args.iter().filter(|a| *a == "--disallowedTools").count(), 1);
    }

    #[test]
    fn an_agent_is_claude_codes_flag_on_every_turn() {
        let mut e = entry(Provider::Claude);
        e.spec.agent = Some("checker".into());
        let (args, _, _) = command_for(&e, "x");
        assert_eq!(value_of(&args, "--agent"), Some("checker"));
        e.session = Some("s-1".into());
        let (args, _, _) = command_for(&e, "x");
        assert_eq!(value_of(&args, "--agent"), Some("checker"));
        assert_eq!(value_of(&args, "--resume"), Some("s-1"));
    }

    #[test]
    fn a_helper_knows_it_is_one_on_both_clis() {
        for provider in [Provider::Claude, Provider::Codex] {
            let mut e = entry(provider);
            e.spec.parent = Some("p-1".into());
            let (args, _, envs) = command_for(&e, "x");
            assert!(envs.iter().any(|(k, v)| *k == "LUNA_SESSION_ID" && v == "x"));
            assert!(envs.iter().any(|(k, v)| *k == "LUNA_PARENT_ID" && v == "p-1"));
            assert!(!args.iter().any(|a| a.contains("developer_instructions")), "that would replace what its config says");
            if provider == Provider::Claude {
                assert!(value_of(&args, "--append-system-prompt").unwrap().contains("started through Luna by session p-1"));
            }
        }
        for provider in [Provider::Claude, Provider::Codex] {
            let (args, _, envs) = command_for(&entry(provider), "x");
            assert!(envs.iter().any(|(k, v)| *k == "LUNA_SESSION_ID" && v == "x"), "whoever started it");
            assert!(!envs.iter().any(|(k, _)| *k == "LUNA_PARENT_ID"));
            assert!(!args.iter().any(|a| a == "--append-system-prompt"));
        }
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

    #[test]
    fn a_helper_started_afresh_has_started_once_its_cli_names_a_conversation() {
        take_up_for_test("afresh", "C:\\p", 0, &[], Some("the prompt"));
        assert!(never_started("afresh"));
        on_claude_event(
            lock().entries.get_mut("afresh").unwrap(),
            &json!({ "type": "system", "subtype": "init", "session_id": "started-afresh", "cwd": "C:\\p" }),
        );
        assert!(!never_started("afresh"));
        lock().entries.remove("afresh");
    }

    #[test]
    fn auto_review_is_codex_own_flag_without_a_policy_or_sandbox() {
        let mut e = entry(Provider::Codex);
        e.spec.mode = "workspace-write".into();
        e.spec.auto_review = true;
        let (args, _, _) = command_for(&e, "x");
        assert!(args.contains(&"--approve-for-me".to_string()));
        assert!(!args.iter().any(|a| a.starts_with("approvals_reviewer=")), "its own flag says who reviews");
        assert!(!args.iter().any(|a| a.starts_with("approval_policy=") || a.starts_with("sandbox_mode=")), "Codex refuses them beside it");
    }

    #[test]
    fn hooks_run_unasked_only_when_the_folder_says_so() {
        let mut e = entry(Provider::Codex);
        let (args, _, _) = command_for(&e, "x");
        assert!(!args.contains(&crate::pty::HOOKS_UNASKED.to_string()));
        e.spec.hooks_unasked = true;
        on_codex_event(&mut e, &json!({ "type": "thread.started", "thread_id": "t-1" }));
        let (args, _, _) = command_for(&e, "x");
        assert_eq!(args[..3], ["exec", "resume", "t-1"]);
        assert!(args.contains(&crate::pty::HOOKS_UNASKED.to_string()), "every turn, resumes too");
        assert!(args.iter().any(|a| a.starts_with("projects={") && a.contains("trust_level")), "trusted for the run");
    }

    #[test]
    fn a_failed_turn_says_why() {
        let mut e = entry(Provider::Claude);
        on_claude_event(
            &mut e,
            &json!({ "type": "result", "subtype": "error_during_execution", "is_error": true, "result": "rate limited" }),
        );
        assert_eq!(e.error.as_deref(), Some("rate limited"));
        let mut e = entry(Provider::Codex);
        on_codex_event(&mut e, &json!({ "type": "turn.failed", "error": { "message": "boom" } }));
        assert_eq!(e.error.as_deref(), Some("boom"));
        assert!(e.log.back().unwrap().contains("boom"));
    }

    /// A pipe its writer has left: what was written, then a read that blocks
    /// for as long as something else holds it open, as a stray job does.
    struct Pipe(std::sync::mpsc::Receiver<Vec<u8>>);

    impl std::io::Read for Pipe {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let Ok(bytes) = self.0.recv() else { return Ok(0) };
            buf[..bytes.len()].copy_from_slice(&bytes);
            Ok(bytes.len())
        }
    }

    /// Reads `lines` off such a pipe, each event taking `each` to handle, and
    /// returns once `await_last_words` does: what was handled by then.
    fn drained_with_a_stray(lines: &[&str], each: std::time::Duration) -> Vec<String> {
        use std::sync::{Arc, Mutex};
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        for line in lines {
            tx.send(format!("{line}\n").into_bytes()).unwrap();
        }
        let reader = Arc::new(OutReader::default());
        let got = Arc::new(Mutex::new(Vec::new()));
        let (drained, read_out) = std::sync::mpsc::channel::<()>();
        let (on_it, into) = (reader.clone(), got.clone());
        std::thread::spawn(move || {
            let _drained = drained;
            on_it.read_events(Pipe(rx), |v| {
                std::thread::sleep(each);
                into.lock().unwrap().push(v["type"].as_str().unwrap_or("").to_string());
                true
            });
        });
        reader.await_last_words(&read_out, std::time::Duration::from_millis(150), std::time::Duration::from_secs(20));
        let seen = got.lock().unwrap().clone();
        drop(tx); // Only now does the stray let go.
        seen
    }

    #[test]
    fn a_turns_last_events_are_read_with_a_stray_holding_the_pipe() {
        let events = [r#"{"type":"assistant"}"#, "not json", r#"{"type":"result"}"#];
        let started = std::time::Instant::now();
        assert_eq!(drained_with_a_stray(&events, std::time::Duration::ZERO), ["assistant", "result"]);
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "and the stray is not waited out");
    }

    #[test]
    fn however_slowly_the_last_events_are_taken_none_is_left_behind() {
        // Each behind a stalled lock: more than a turn's end used to give them.
        let events = [r#"{"type":"one"}"#, r#"{"type":"two"}"#, r#"{"type":"result"}"#];
        assert_eq!(drained_with_a_stray(&events, std::time::Duration::from_millis(900)), ["one", "two", "result"]);
    }

    #[test]
    fn a_pipe_that_closes_ends_the_wait_at_once_and_a_last_line_counts_without_its_end() {
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        tx.send(br#"{"type":"a"}"#.to_vec()).unwrap();
        tx.send(b"\n{\"type\":\"b\"}\n{\"ty".to_vec()).unwrap();
        tx.send(br#"pe":"c"}"#.to_vec()).unwrap();
        drop(tx);
        let reader = OutReader::default();
        let mut got = Vec::new();
        reader.read_events(Pipe(rx), |v| {
            got.push(v["type"].as_str().unwrap_or("").to_string());
            true
        });
        assert_eq!(got, ["a", "b", "c"], "split across reads, and the last with no line end");
        let (drained, read_out) = std::sync::mpsc::channel::<()>();
        drop(drained);
        let started = std::time::Instant::now();
        reader.await_last_words(&read_out, std::time::Duration::from_secs(9), std::time::Duration::from_secs(9));
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn an_event_of_a_turn_over_lands_in_nothing() {
        let id = "ev-after";
        lock().entries.insert(id.into(), entry(Provider::Codex));
        let reader = OutReader::default();
        let failed = json!({ "type": "turn.failed", "error": { "message": "boom" } });
        assert!(take_event(id, Provider::Codex, &reader, &failed));
        assert_eq!(last_error(id).as_deref(), Some("boom"));
        lock().entries.get_mut(id).unwrap().error = None;
        // The turn is over and the next may be on: this one's words stay out of it.
        reader.over.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(take_event(id, Provider::Codex, &reader, &failed), "it reads on, to keep the pipe drained");
        assert_eq!(last_error(id), None);
        lock().entries.remove(id);
        let reader = OutReader::default();
        assert!(!take_event(id, Provider::Codex, &reader, &failed), "the helper is gone: no more to read for");
    }

    #[test]
    fn a_retry_codex_got_through_is_no_failure() {
        // As `codex exec --json` 0.166 words them on a connection it retries.
        let retry = json!({ "type": "error", "message": "Reconnecting... 2/5 (unexpected status 401 Unauthorized)" });
        let mut e = entry(Provider::Codex);
        on_codex_event(&mut e, &retry);
        assert_eq!(e.error, None, "said in its log, and not a failure");
        assert!(e.log.back().unwrap().contains("Reconnecting"));
        on_codex_event(&mut e, &json!({ "type": "turn.completed" }));
        assert_eq!(e.error, None);
        // One that does not read as a retry, followed by a turn that completes.
        on_codex_event(&mut e, &json!({ "type": "error", "message": "stream hiccup" }));
        assert_eq!(e.error.as_deref(), Some("stream hiccup"));
        on_codex_event(&mut e, &json!({ "type": "turn.completed" }));
        assert_eq!(e.error, None, "the turn got through after all");
        // The retries run out.
        on_codex_event(&mut e, &retry);
        on_codex_event(&mut e, &json!({ "type": "error", "message": "unexpected status 401 Unauthorized" }));
        on_codex_event(&mut e, &json!({ "type": "turn.failed", "error": { "message": "unexpected status 401 Unauthorized" } }));
        assert_eq!(e.error.as_deref(), Some("unexpected status 401 Unauthorized"));
    }

    #[test]
    fn a_turn_killed_or_crashed_ends_on_an_error() {
        let run = |e: &mut Entry| e.run = Some(Run::claimed(Some(1), 0));
        for (id, code, why) in [("end-crash", Some(1), "codex exited with code 1"), ("end-signal", None, "codex was killed")] {
            let mut e = entry(Provider::Codex);
            run(&mut e);
            lock().entries.insert(id.into(), e);
            turn_over(id, code);
            assert_eq!(last_error(id).as_deref(), Some(why));
            assert_eq!(turn_state(id).unwrap().turns_ended, 1, "it still ends the turn");
            lock().entries.remove(id);
        }
        // Stopped on purpose, it says so rather than the code that leaves.
        let mut e = entry(Provider::Claude);
        run(&mut e);
        e.run.as_mut().unwrap().pid = None;
        lock().entries.insert("end-killed".into(), e);
        kill("end-killed");
        turn_over("end-killed", Some(1));
        assert_eq!(last_error("end-killed").as_deref(), Some("it was stopped, by luna_kill or from the window"));
        lock().entries.remove("end-killed");
        // A clean exit says nothing.
        let mut e = entry(Provider::Claude);
        run(&mut e);
        lock().entries.insert("end-clean".into(), e);
        turn_over("end-clean", Some(0));
        assert_eq!(last_error("end-clean"), None);
        lock().entries.remove("end-clean");
    }

    #[test]
    fn an_answer_is_shaped_as_the_cli_takes_it() {
        let input = json!({ "questions": [{ "question": "Which color?", "options": [{ "label": "Red" }, { "label": "Blue" }] }] });
        let ask = Ask::from_claude_questions(&input).unwrap();
        let a = build_answer(&ask, &input, &["2".into()], None, None).unwrap();
        assert_eq!(a["behavior"], "allow");
        assert_eq!(a["updatedInput"]["answers"]["Which color?"], "Blue");
        assert_eq!(a["updatedInput"]["questions"], input["questions"], "the input goes back with the answers added");
        assert!(build_answer(&ask, &input, &[], None, None).is_err(), "one answer per question");
        let own = build_answer(&ask, &input, &["green, actually".into()], None, None).unwrap();
        assert_eq!(own["updatedInput"]["answers"]["Which color?"], "green, actually");

        let cmd = json!({ "command": "rm -rf build" });
        let perm = Ask::from_claude_tool("Bash", &cmd);
        assert_eq!(build_answer(&perm, &cmd, &[], Some(true), None).unwrap(), json!({ "behavior": "allow", "updatedInput": cmd }));
        let no = build_answer(&perm, &cmd, &[], Some(false), Some("not now")).unwrap();
        assert_eq!((no["behavior"].as_str(), no["message"].as_str()), (Some("deny"), Some("not now")));
        assert!(build_answer(&perm, &cmd, &[], None, None).is_err());
    }

    // ------------------------------------------- text joining a running turn --

    /// A Claude Code turn with its stdin open: what goes down `feed` is at the
    /// other end, and the process has no pid to kill.
    fn feeding(e: &mut Entry) -> std::sync::mpsc::Receiver<String> {
        let (feed, lines) = std::sync::mpsc::channel();
        e.run = Some(Run { feed: Some(feed), line: Some("opening-line".into()), ..Run::claimed(None, 0) });
        lines
    }

    #[test]
    fn claude_helpers_keep_stdin_for_stream_json_and_codex_ones_do_not() {
        let has = |args: &[String], flag: &str| args.iter().any(|a| a == flag);
        let (args, _, _) = command_for(&entry(Provider::Claude), "x");
        assert!(args.windows(2).any(|w| w == ["--input-format", "stream-json"]), "{args:?}");
        assert!(has(&args, "--replay-user-messages"), "its word that a line was taken in");
        let (args, _, _) = command_for(&entry(Provider::Codex), "x");
        assert!(!has(&args, "--input-format") && !has(&args, "--replay-user-messages"));
    }

    #[test]
    fn a_message_is_one_line_of_json_whatever_it_says() {
        let text = "two\nlines, \"quotes\", a back\\slash and 🌧";
        let line = user_line("the-uuid", text);
        assert!(line.ends_with('\n') && line.matches('\n').count() == 1, "{line}");
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!((v["type"].as_str(), v["uuid"].as_str(), v["message"]["role"].as_str()), (Some("user"), Some("the-uuid"), Some("user")));
        assert_eq!(v["message"]["content"].as_str(), Some(text));
        assert!(v["parent_tool_use_id"].is_null());
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

    /// A stdin that takes what `pass` lets through, one write at a time.
    struct Stalled {
        pass: std::sync::mpsc::Receiver<()>,
        wrote: std::sync::Arc<Mutex<Vec<u8>>>,
    }

    impl Write for Stalled {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.pass.recv().map_err(|_| std::io::ErrorKind::BrokenPipe)?;
            self.wrote.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Nobody reading stdin holds the writer and nothing else; what got in stays
    /// in order; its going ends the writer.
    #[test]
    fn a_stdin_nobody_reads_holds_up_nothing_but_its_writer() {
        let (open, pass) = std::sync::mpsc::channel();
        let wrote = std::sync::Arc::new(Mutex::new(Vec::new()));
        let stdin = Stalled { pass, wrote: wrote.clone() };
        let (feed, lines) = std::sync::mpsc::channel();
        let writer = std::thread::spawn(move || feed_stdin(stdin, "first\n", lines));
        let started = std::time::Instant::now();
        for n in 0..10_000 {
            feed.send(format!("{n}\n")).unwrap();
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "a send does not wait for the pipe");
        open.send(()).unwrap();
        open.send(()).unwrap();
        drop(feed);
        // The pipe breaks: the writer goes with most of it unwritten.
        drop(open);
        writer.join().unwrap();
        assert_eq!(String::from_utf8(wrote.lock().unwrap().clone()).unwrap(), "first\n0\n");
    }

    #[test]
    fn stdin_ends_when_its_lines_do() {
        let (feed, lines) = std::sync::mpsc::channel::<String>();
        feed.send("second\n".into()).unwrap();
        drop(feed);
        let mut out = Vec::new();
        feed_stdin(&mut out, "first\n", lines);
        assert_eq!(out, b"first\nsecond\n");
    }

    #[test]
    fn text_joins_a_turn_with_its_stdin_open_and_goes_first_come_first_served() {
        let mut e = entry(Provider::Claude);
        let lines = feeding(&mut e);
        assert_eq!(place(&mut e, "stop at the next commit".into(), true), Placed::Joined);
        let line: Value = serde_json::from_str(&lines.try_recv().unwrap()).unwrap();
        assert_eq!(line["message"]["content"].as_str(), Some("stop at the next commit"));
        assert_eq!(line["uuid"].as_str(), Some(e.fed[0].id.as_str()));
        assert_eq!(e.fed[0].line.as_deref(), Some(e.fed[0].id.as_str()));
        assert!(e.queue.is_empty());

        // Something waits already: it goes behind that, not past it.
        e.queue.push("cancel the commit".into());
        assert_eq!(place(&mut e, "and this".into(), true), Placed::Queued);
        assert_eq!(e.queue, ["cancel the commit", "and this"]);
        assert!(lines.try_recv().is_err());
    }

    /// A turn claimed has no stdin yet: what comes then queues, and what comes
    /// once it has one must not overtake that.
    #[test]
    fn what_comes_as_a_turn_starts_is_not_overtaken_once_it_has_started() {
        let mut e = entry(Provider::Claude);
        claim(&mut e);
        assert_eq!(place(&mut e, "do A".into(), true), Placed::Queued, "no stdin yet");
        let lines = feeding(&mut e);
        assert_eq!(place(&mut e, "cancel A".into(), true), Placed::Queued);
        assert_eq!(e.queue, ["do A", "cancel A"]);
        assert!(lines.try_recv().is_err() && e.fed.is_empty());
    }

    #[test]
    fn a_notice_a_codex_turn_and_a_closed_stdin_all_queue() {
        let mut e = entry(Provider::Claude);
        let _lines = feeding(&mut e);
        assert_eq!(place(&mut e, "a notice".into(), false), Placed::Queued, "aimed at an idle helper");
        let mut e = entry(Provider::Codex);
        e.run = Some(Run::claimed(Some(1), 0));
        assert_eq!(place(&mut e, "text".into(), true), Placed::Queued);
        let mut e = entry(Provider::Claude);
        let lines = feeding(&mut e);
        drop(lines);
        assert_eq!(place(&mut e, "its writer is gone".into(), true), Placed::Queued);
        assert!(e.fed.is_empty() && e.queue[0].line.is_none());
    }

    #[test]
    fn its_first_result_ends_stdin_and_text_then_waits_for_the_next_process() {
        let mut e = entry(Provider::Claude);
        let lines = feeding(&mut e);
        on_claude_event(&mut e, &json!({ "type": "result", "subtype": "success", "duration_ms": 10 }));
        assert!(matches!(lines.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected)), "the end of its input");
        assert!(e.run.is_some(), "the turn is over when its process is");
        assert_eq!(place(&mut e, "too late for it".into(), true), Placed::Queued);
    }

    /// A line the CLI passes over has no `result`, which must not hold stdin open.
    #[test]
    fn the_end_of_the_line_it_started_on_ends_stdin_and_of_another_does_not() {
        let mut e = entry(Provider::Claude);
        let lines = feeding(&mut e);
        let ended = |uuid: &str, state: &str| json!({ "type": "command_lifecycle", "command_uuid": uuid, "state": state });
        on_claude_event(&mut e, &ended("a-joined-line", "completed"));
        assert!(!matches!(lines.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected)));
        on_claude_event(&mut e, &ended("opening-line", "queued"));
        assert!(e.run.as_ref().unwrap().feed.is_some());
        on_claude_event(&mut e, &ended("opening-line", "completed"));
        assert!(matches!(lines.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected)));

        let mut e = entry(Provider::Claude);
        let _lines = feeding(&mut e);
        // lang-check: allow
        on_claude_event(&mut e, &ended("opening-line", "cancelled"));
        assert!(e.run.as_ref().unwrap().feed.is_none());
    }

    #[test]
    fn the_clis_word_is_what_retires_a_message_not_the_pipe() {
        let mut e = entry(Provider::Claude);
        let _lines = feeding(&mut e);
        assert_eq!(place(&mut e, "joined".into(), true), Placed::Joined);
        let uuid = e.fed[0].id.clone();
        assert!(!on_claude_event(&mut e, &json!({ "type": "user", "uuid": uuid, "message": {} })), "a tool result, say");
        assert!(
            !on_claude_event(&mut e, &json!({ "type": "command_lifecycle", "command_uuid": uuid, "state": "queued" })),
            "queued is lost with a kill"
        );
        assert_eq!(e.fed.len(), 1);
        assert!(!on_claude_event(&mut e, &json!({ "type": "user", "isReplay": true, "uuid": "another" })));
        assert!(on_claude_event(&mut e, &json!({ "type": "user", "isReplay": true, "uuid": uuid })));
        assert!(e.fed.is_empty());

        // A batch goes as the line of its first, the opening one too.
        let mark = BudgetMark { id: "kid".into(), line: "over budget".into() };
        e.in_flight = vec![Queued::new(None, "one".into(), vec![mark]), "two".into()];
        stamp(&mut e.in_flight);
        let uuid = e.in_flight[0].id.clone();
        assert!(on_claude_event(&mut e, &json!({ "type": "command_lifecycle", "command_uuid": uuid, "state": "started" })));
        assert!(e.in_flight.is_empty());
        assert_eq!(e.budgets_landed, ["kid"], "a budget line counts once the CLI has it");
    }

    /// A stop puts the text by a moment before the last of what the CLI said is read.
    #[test]
    fn its_word_arriving_after_a_stop_takes_the_text_back_off_the_undelivered() {
        let mut e = entry(Provider::Claude);
        let _lines = feeding(&mut e);
        assert_eq!(place(&mut e, "joined".into(), true), Placed::Joined);
        let uuid = e.fed[0].id.clone();
        stop_for_luna(&mut e);
        assert_eq!(e.undelivered, ["joined"]);
        assert!(on_claude_event(&mut e, &json!({ "type": "user", "isReplay": true, "uuid": uuid })));
        assert!(e.undelivered.is_empty());
    }

    #[test]
    fn a_stop_for_lunas_sake_keeps_what_joined_and_counts_no_turn_for_it_and_a_kill_drops_it() {
        let mut e = entry(Provider::Claude);
        let _lines = feeding(&mut e);
        assert_eq!(place(&mut e, "joined".into(), true), Placed::Joined);
        e.queue.push("behind it".into());
        e.turns_ended = 3;
        stop_for_luna(&mut e);
        assert_eq!(e.undelivered, ["joined", "behind it"]);
        assert!(e.fed.is_empty() && e.run.as_ref().unwrap().feed.is_none());
        assert_eq!(e.turns_ended, 4, "the one for what was queued, none for what joined");

        let mut e = entry(Provider::Claude);
        let _lines = feeding(&mut e);
        assert_eq!(place(&mut e, "joined".into(), true), Placed::Joined);
        e.turns_ended = 3;
        stop(&mut e);
        assert!(e.fed.is_empty() && e.undelivered.is_empty());
        assert_eq!(e.turns_ended, 3, "its sender waits on this turn's end");
    }

    /// One try more, under the same id, and then back to who sent it: a CLI that
    /// keeps going before it takes anything does not loop.
    #[test]
    fn what_a_cli_went_without_starts_the_next_turn_once() {
        let mut e = entry(Provider::Claude);
        let _lines = feeding(&mut e);
        assert_eq!(place(&mut e, "joined".into(), true), Placed::Joined);
        let uuid = e.fed[0].id.clone();
        e.queue.push("behind it".into());
        assert_eq!(sort_out_untaken(&mut e), ["joined"]);
        assert_eq!(e.queue, ["joined", "behind it"], "in the order they came");
        assert!(e.fed.is_empty() && e.undelivered.is_empty());

        // The next process is started on it. Its pipe taking the line retires nothing.
        assert_eq!(take_queue("x", &mut e), "joined\n\nbehind it");
        assert_eq!(e.in_flight[0].id, uuid, "the id it had");
        assert_eq!(e.in_flight[1].line.as_deref(), Some(uuid.as_str()), "the batch goes by the first's");

        // That one goes too.
        assert!(sort_out_untaken(&mut e).is_empty());
        assert!(e.in_flight.is_empty() && e.queue.is_empty());
        assert_eq!(e.undelivered, ["joined", "behind it"]);
    }

    #[test]
    fn what_joined_behind_a_batch_never_taken_is_not_sent_past_it() {
        let mut e = entry(Provider::Claude);
        e.in_flight = vec!["the batch".into()];
        stamp(&mut e.in_flight);
        let _lines = feeding(&mut e);
        assert_eq!(place(&mut e, "joined".into(), true), Placed::Joined);
        assert!(sort_out_untaken(&mut e).is_empty());
        assert_eq!(e.undelivered, ["the batch", "joined"]);
        assert!(e.queue.is_empty());
    }

    #[test]
    fn a_turn_over_takes_up_what_missed_it_and_what_the_cli_took_is_done() {
        let mut e = entry(Provider::Claude);
        let _lines = feeding(&mut e);
        assert_eq!(place(&mut e, "joined".into(), true), Placed::Joined);
        lock().entries.insert("missed".into(), e);
        // No spawn gets far: the folder is not there.
        lock().entries.get_mut("missed").unwrap().spec.folder = "C:\\luna-no-such-folder".into();
        turn_over("missed", Some(1));
        let e = lock().entries.remove("missed").unwrap();
        assert_eq!(e.turns_ended, 2, "the turn, and the next that could not start");
        assert_eq!(e.undelivered, ["joined"], "not dropped with the turn that could not start");
    }

    fn pending(uuid: &str, text: &str) -> Queued {
        let mut q = Queued::new(Some("x".into()), text.to_string(), Vec::new());
        q.id = uuid.to_string();
        q.line = Some(uuid.to_string());
        q
    }

    fn with_lines(tied: bool) -> Saved {
        let spec = entry(Provider::Claude).spec;
        let mut s: Saved = serde_json::from_value(json!({
            "id": "s", "spec": serde_json::to_value(&spec).unwrap(), "startedMs": 0, "session": "sid", "cwd": null, "turnsEnded": 1,
            "running": true, "queued": false, "cutShort": false, "ties": serde_json::to_value(crate::agents::Ties::default()).unwrap(),
        }))
        .unwrap();
        s.tied = tied;
        s
    }

    /// In the transcript it is the model's, whatever Luna had time to note: not
    /// given back, not sent again.
    #[test]
    fn a_core_gone_before_noting_the_clis_word_goes_by_the_transcript() {
        let mut s = with_lines(true);
        let mark = BudgetMark { id: "kid".into(), line: "over budget".into() };
        let mut flying = pending("batch", "started on");
        flying.budget = vec![mark];
        s.in_flight = vec![flying, Queued { line: Some("batch".into()), ..pending("second", "with it") }];
        s.fed = vec![pending("folded", "taken in"), pending("missed", "not taken in")];
        settle_lines(&mut s, &|line| Some(line != "missed"));
        assert!(s.in_flight.is_empty(), "a batch goes by its line");
        assert_eq!(s.fed, ["not taken in"]);
        assert!(!s.fed[0].maybe_reached, "the turn is over and it is not there: it never got there");
        assert!(s.fed[0].line.is_none());
        assert_eq!(s.budgets_landed, ["kid"]);
        let e = taken_up(s);
        assert_eq!(e.undelivered, ["not taken in"]);
    }

    #[test]
    fn what_cannot_be_established_is_said_so_and_not_sent_again() {
        // The transcript cannot be read.
        let mut s = with_lines(true);
        s.fed = vec![pending("a", "joined")];
        settle_lines(&mut s, &|_| None);
        assert!(s.fed[0].maybe_reached);
        // Or nothing says the turn is over, so the CLI may yet take it.
        let mut s = with_lines(false);
        s.in_flight = vec![pending("b", "flying")];
        settle_lines(&mut s, &|_| Some(false));
        assert!(s.in_flight[0].maybe_reached);

        let mut e = taken_up(s);
        e.resumable = true;
        e.undelivered.push(Queued::new(Some("x".into()), "plainly lost".into(), Vec::new()));
        e.session = Some("sid".into());
        e.spec.folder = "C:\\luna-no-such-folder".into();
        lock().entries.insert("doubt".into(), e);
        let told = undelivered_for("doubt", "x", false);
        assert!(told[0].contains("MAY have reached it") && told[0].ends_with("flying"), "{told:?}");
        assert_eq!(told[1], "plainly lost");
        resume("doubt");
        let e = lock().entries.remove("doubt").unwrap();
        assert_eq!(e.undelivered.iter().filter(|q| q.maybe_reached).count(), 1, "still its sender's to decide");
        assert!(e.undelivered.iter().filter(|q| q.maybe_reached).all(|q| q.text == "flying"));
    }

    /// What a stop put by, and a core's save, go the same way.
    #[test]
    fn what_a_stop_put_by_on_stdin_is_settled_too_and_what_never_went_is_not() {
        let mut s = with_lines(true);
        s.running = false;
        s.cut_short = true;
        s.undelivered = vec![pending("told", "went in"), "never on stdin".into()];
        settle_lines(&mut s, &|line| {
            assert_eq!(line, "told");
            Some(true)
        });
        assert_eq!(s.undelivered, ["never on stdin"]);
    }

    #[test]
    fn a_codex_helpers_text_is_left_as_it_was() {
        let mut s = with_lines(true);
        s.spec.provider = Provider::Codex;
        s.in_flight = vec![pending("a", "flying")];
        settle_lines(&mut s, &|_| panic!("Codex has no such lines"));
        assert!(!s.in_flight[0].maybe_reached);
    }

    #[test]
    fn stable_ids_cross_a_core_and_an_older_file_gets_them() {
        let mut e = entry(Provider::Claude);
        let _lines = feeding(&mut e);
        assert_eq!(place(&mut e, "joined".into(), true), Placed::Joined);
        let uuid = e.fed[0].id.clone();
        lock().entries.insert("carried".into(), e);
        let bytes = serde_json::to_vec(&snapshot().into_iter().filter(|s| s.id == "carried").collect::<Vec<Saved>>()).unwrap();
        lock().entries.remove("carried");
        let saved: Vec<Saved> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(saved[0].fed[0].id, uuid);
        assert_eq!(saved[0].fed[0].line.as_deref(), Some(uuid.as_str()));
        let e = taken_up(saved.into_iter().next().unwrap());
        assert_eq!(e.undelivered, ["joined"]);
        assert_eq!(e.turns_ended, 1, "the turn; none for what joined it");

        let old: Queued = serde_json::from_value(json!({ "from": null, "text": "older" })).unwrap();
        assert_eq!(old.id.len(), 36);
        assert!(old.line.is_none() && !old.maybe_reached);
    }

    #[test]
    fn the_clis_own_requests_are_refused_on_stdin_rather_than_left_to_hang() {
        let mut e = entry(Provider::Claude);
        let lines = feeding(&mut e);
        let asked = json!({ "type": "control_request", "request_id": "r-1", "request": { "subtype": "elicitation", "message": "Pick" } });
        assert!(!on_claude_event(&mut e, &asked));
        let v: Value = serde_json::from_str(&lines.try_recv().unwrap()).unwrap();
        assert_eq!(v["type"].as_str(), Some("control_response"));
        assert_eq!((v["response"]["subtype"].as_str(), v["response"]["request_id"].as_str()), (Some("error"), Some("r-1")));
        // Its stdin has ended: the CLI cancels that by itself.
        e.run.as_mut().unwrap().feed = None;
        on_claude_event(&mut e, &asked);
        assert!(lines.try_recv().is_err());
    }

    #[test]
    fn a_later_result_clears_what_an_earlier_one_of_the_process_left_and_nothing_else() {
        let mut e = entry(Provider::Claude);
        let failed = json!({ "type": "result", "subtype": "error_during_execution", "is_error": true, "result": "rate limited" });
        let done = json!({ "type": "result", "subtype": "success", "duration_ms": 1 });
        e.run = Some(Run::claimed(None, 0));
        on_claude_event(&mut e, &failed);
        assert_eq!(e.error.as_deref(), Some("rate limited"));
        on_claude_event(&mut e, &done);
        assert_eq!(e.error, None, "what was fed in behind it went through");

        e.error = Some("it was stopped, by luna_kill or from the window".into());
        on_claude_event(&mut e, &done);
        assert!(e.error.is_some(), "not a result's to clear");
    }

    fn unnamed(opening: &str, id: Option<&str>) -> Saved {
        let mut s = with_lines(true);
        s.session = None;
        s.opening = Some(opening.to_string());
        s.opening_id = id.map(str::to_owned);
        s
    }

    /// Started on a tracked batch with no conversation named yet, and the core goes:
    /// one message, with what is known of it, and no sure copy for a resume to send.
    #[test]
    fn a_tracked_opening_is_not_rebuilt_as_a_fresh_copy() {
        let mut s = unnamed("do A", Some("a-id"));
        s.in_flight = vec![pending("a-id", "do A")];
        settle_lines(&mut s, &|_| None);
        let mut e = taken_up(s);
        assert_eq!(e.undelivered, ["do A"]);
        assert!(e.undelivered[0].maybe_reached, "the one there is says so");

        e.resumable = true;
        e.spec.folder = "C:\\luna-no-such-folder".into();
        let before = e.turns_ended;
        lock().entries.insert("one-copy".into(), e);
        resume("one-copy");
        let e = lock().entries.remove("one-copy").unwrap();
        assert!(e.run.is_none() && e.turns_ended == before, "nothing sure to go on with: a turn tried would have counted");
        assert_eq!(e.undelivered.len(), 1);

        // The same when a stop has put it by.
        let mut s = unnamed("do A", Some("a-id"));
        s.undelivered = vec![pending("a-id", "do A")];
        assert_eq!(taken_up(s).undelivered, ["do A"]);
        // And in a file from before ids, by its words.
        let mut s = unnamed("do A", None);
        s.in_flight = vec!["do A".into()];
        assert_eq!(taken_up(s).undelivered, ["do A"]);
    }

    #[test]
    fn an_opening_nothing_holds_comes_back_under_the_id_it_had() {
        let e = taken_up(unnamed("do A", Some("a-id")));
        assert_eq!(e.undelivered, ["do A"]);
        assert_eq!(e.undelivered[0].id, "a-id");
        assert!(!e.undelivered[0].maybe_reached, "as ever: it goes on by itself");

        // A batch claimed behind an opening that failed is another message.
        let mut s = unnamed("do A", Some("a-id"));
        s.in_flight = vec![pending("b-id", "then B")];
        assert_eq!(taken_up(s).undelivered, ["do A", "then B"]);
    }

    #[test]
    fn a_turn_notes_the_line_of_what_it_starts_on_while_nothing_names_it() {
        let mut e = entry(Provider::Claude);
        note_opening(&mut e, "do A", "a-id");
        assert_eq!((e.opening.as_deref(), e.opening_id.as_deref()), (Some("do A"), Some("a-id")));
        e.session = Some("sid".into());
        note_opening(&mut e, "do B", "b-id");
        assert_eq!((e.opening.as_deref(), e.opening_id.as_deref()), (None, None));
    }

    #[test]
    fn queued_text_a_cli_reads_as_a_command_is_checked_again_against_its_sender() {
        use crate::agents::{forget, register};
        let mode = |m: &str| serde_json::json!({ "permissionMode": m });
        register("bq-full", Provider::Claude, "C:/p", "C:/acc/one", None, true, mode("bypassPermissions"));
        register("bq-kid", Provider::Claude, "C:/p", "C:/acc/one", Some("bq-full"), false, mode("default"));
        let queued = |from: &str, text: &str| Queued::new(Some(from.to_string()), text.to_string(), Vec::new());
        let mut e = entry(Provider::Claude);
        // Taken while its sender was at full access: it would go in now.
        e.queue.push(queued("bq-full", "/project-command"));
        assert_eq!(take_queue("bq-kid", &mut e), "/project-command");
        // The sender runs lower by the time the turn starts: dropped; plain text still goes.
        register("bq-full", Provider::Claude, "C:/p", "C:/acc/one", None, true, mode("default"));
        e.queue.extend([queued("bq-full", "/project-command"), queued("bq-full", "plain words")]);
        assert_eq!(take_queue("bq-kid", &mut e), "plain words");
        // Its sender gone: dropped too. Luna's own (no sender) goes.
        forget("bq-full");
        e.queue.extend([queued("bq-full", "! whoami"), Queued::new(None, "a notice".into(), Vec::new())]);
        assert_eq!(take_queue("bq-kid", &mut e), "a notice");
        forget("bq-kid");
    }
}
