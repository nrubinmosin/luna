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
use crate::procs::{left_names, leftovers, Leftover};
use crate::provider::Provider;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::sync::{Condvar, Mutex, OnceLock};

mod ask;
mod command;
mod control;
mod events;
mod persist;
mod queue;
mod rows;
mod run;
#[cfg(test)]
mod testkit;
mod types;

// Every file of the module reaches the others' items through `use super::*`,
// and the rest of the crate keeps its `crate::background::X` paths.
pub(crate) use ask::*;
use command::*;
pub(crate) use control::*;
pub(crate) use events::*;
pub(crate) use persist::*;
pub(crate) use queue::*;
pub(crate) use rows::*;
pub(crate) use run::*;
#[cfg(test)]
pub(crate) use testkit::*;
pub(crate) use types::*;

/// Lines of what a helper did kept for `luna_read(screen)`: its messages,
/// commands and errors, one line each.
const LOG_LINES: usize = 200;
/// How much of a message one log line keeps.
const LOG_CHARS: usize = 300;
/// How long a held permission or question waits for `luna_answer` before the
/// CLI is told nobody answered. Under the MCP call timeout the CLIs are given
/// (pty/start.rs `MCP_TOOL_TIMEOUT_MS`).
const ASK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30 * 60);

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
    /// (agents/ `undelivered_shown`) or a turn of `resume` has taken it.
    undelivered: Vec<Queued>,
    /// The helpers whose budget lines its CLI's stdin has taken, until the agent
    /// layer has noted it: the one word that a line arrived (`stdin_took`).
    budgets_landed: Vec<String>,
    /// Its turn's processes are in a job tied to the core (procs/), so a core
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
    /// next core starts on the same conversation (procs/).
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

/// The core stays after all — a shutdown of the PC it had wound down for was
/// taken back: turns may start again.
pub fn reopen() {
    lock().closed = false;
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

/// It has passed to another parent, or with none stands alone (agents/teardown.rs
/// `pass_up`): what its next turn is told. One running now has its environment
/// and system prompt already.
pub fn set_parent(id: &str, parent: Option<String>) {
    if let Some(e) = lock().entries.get_mut(id) {
        e.spec.parent = parent;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
