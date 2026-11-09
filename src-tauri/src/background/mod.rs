//! Helpers with no terminal and no row: every one `luna_session_spawn` starts. The
//! person watches one read-only, or opens it as a chat, which hands it over to
//! them for good (agents/handover.rs). Each turn is one run of the CLI in its headless
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
//! spawned it answers with `luna_session_answer`. Codex's headless mode has no one to
//! ask: a question it has comes in its reply, and the answer is the next turn.
//!
//! The conversation itself lands on disk as with any session — Claude Code's
//! transcript, Codex's rollout — so `luna_message_list` reads it there.

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

/// Lines of what a helper did kept for `luna_message_list(screen)`: its messages,
/// commands and errors, one line each.
const LOG_LINES: usize = 200;
/// How much of a message one log line keeps.
const LOG_CHARS: usize = 300;
/// How long a held permission or question waits for `luna_session_answer` before the
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
    /// `luna_session_get` names, and whether it does anything (`leftovers`).
    left: Vec<Leftover>,
    /// A stop between turns is ending what its turns left (`kill`): no turn
    /// starts meanwhile, as its CLI would land in the job being emptied; text
    /// sent queues, and starts the next turn once the stop is through.
    ending_left: bool,
    /// Being opened as a chat (`background_promote`), until the window says how
    /// its row came out: no turn starts meanwhile, as its conversation may be
    /// resumed in a terminal any moment, and its parent can neither stop nor
    /// delete it; text sent queues, and is told of as never delivered once the
    /// person has it (agents/handover.rs).
    promoting: bool,
    /// When `promoting` began, for PROMOTE_TIMEOUT_MS: kept across a core, as
    /// `promoting` is.
    promoting_since: u64,
    /// A delete is under way (`claim_delete`), until it has removed the helper or
    /// been refused: no turn starts, and it is not opened as a chat, meanwhile, so
    /// what the delete went by (whose it is, that it is idle, its worktree) still
    /// holds when it removes it. Text sent queues, and goes with it.
    deleting: bool,
    /// The window's word that its row is made is being acted on (rows.rs
    /// `background_promoted`): a second word, a retry whose first answer was
    /// lost, hands nothing over again.
    handing: bool,
    observed: Observed,
}

impl Entry {
    /// Something has it held between turns: no turn starts until it lets go.
    fn held_back(&self) -> bool {
        self.promoting || self.deleting
    }
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

/// Whether the person is opening helper `id` as a chat (`background_promote`).
pub fn promoting(id: &str) -> bool {
    lock().entries.get(id).is_some_and(|e| e.promoting)
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
    // As the ties now stand: none.
    for e in lock().entries.values_mut() {
        e.tied = false;
    }
}

/// The core stays after all — a shutdown of the PC it had wound down for was
/// taken back. Every helper's job is tied to the core again, as `stop_all` let
/// them go for a core that leaves: a later turn joins that same job, and a core
/// dying then must still take its processes along, or the next core would read
/// the tie it saved and start a second turn beside them. Each helper this core
/// stopped is settled as a new core would have found it (`settle_stopped`), and
/// turns may start again.
pub fn reopen() {
    let jobs: Vec<(String, std::sync::Arc<crate::procs::Job>)> =
        lock().entries.iter().filter_map(|(id, e)| Some((id.clone(), e.job.clone()?))).collect();
    let ties = retie(&jobs);
    let reached = stopped_lines();
    let mut r = lock();
    for (id, tied) in ties {
        if let Some(e) = r.entries.get_mut(&id) {
            e.tied = tied;
        }
    }
    for (id, e) in r.entries.iter_mut() {
        settle_stopped(e, &|line| reached.get(&(id.clone(), line.to_string())).copied().flatten());
    }
    r.closed = false;
}

/// Ties each helper's job to the core again, and says for each whether it took.
/// Off the registry's lock: a tie is a kernel call, or a word to the keeper.
fn retie(jobs: &[(String, std::sync::Arc<crate::procs::Job>)]) -> Vec<(String, bool)> {
    jobs.iter().map(|(id, job)| (id.clone(), crate::procs::tie(job, id))).collect()
}

/// What the conversations say of the lines the stopped Claude Code turns had on
/// stdin — by helper and line, whether the transcript holds it, None where that
/// cannot be read — gathered off the registry's lock, as `restore` reads them
/// before taking a saved helper up.
fn stopped_lines() -> HashMap<(String, String), Option<bool>> {
    let pending: Vec<(String, String, Option<String>, Vec<String>)> = lock()
        .entries
        .iter()
        .filter(|(_, e)| e.spec.provider == Provider::Claude && e.cut_short && e.run.is_none())
        .map(|(id, e)| {
            let stores = [&e.in_flight, &e.fed, &e.undelivered];
            let lines = stores.iter().flat_map(|store| store.iter().filter_map(|q| q.line.clone())).collect();
            (id.clone(), e.spec.account_path.clone(), e.session.clone(), lines)
        })
        .collect();
    let mut reached = HashMap::new();
    for (id, account, session, lines) in pending {
        let transcript = session.and_then(|sid| crate::claude::session::find_transcript(&account, &sid));
        for line in lines {
            let took = transcript.as_deref().and_then(|path| crate::claude::session::took_line(path, &line));
            reached.insert((id.clone(), line), took);
        }
    }
    reached
}

/// Stops every turn in progress with its process tree and takes what was
/// queued off it, kept as undelivered; `close` also keeps any from starting
/// after. A turn claimed but not yet started is stopped too: it does not
/// start. Returns how many it stopped.
pub fn stop_turns(close: bool) -> usize {
    let (pids, claimed, ids): (Vec<(u32, crate::procs::Pin)>, usize, Vec<String>) = {
        let mut r = lock();
        r.closed |= close;
        let mut pids = Vec::new();
        let mut claimed = 0;
        let mut ids = Vec::new();
        for (id, e) in r.entries.iter_mut() {
            match stop_for_luna(e) {
                Some(pid) => {
                    pids.push((pid, crate::procs::pin(pid)));
                    ids.push(id.clone());
                }
                None if e.run.is_some() => claimed += 1,
                None => {}
            }
        }
        (pids, claimed, ids)
    };
    reg().1.notify_all();
    // Their turns' exits are Luna's doing, not their CLI versions'.
    for id in &ids {
        crate::cli::session_stopped(id);
    }
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

    /// `stop_all` lets every job go, for a core that leaves. A core that stays
    /// on after a shutdown taken back ties them again, or its death later would
    /// leave the helpers' processes running on unowned, and the next core,
    /// trusting the tie it saved, would start a second turn beside them.
    #[cfg(windows)]
    #[test]
    fn reopening_ties_the_helpers_jobs_to_the_core_again() {
        use crate::procs::testkit::{ended_within, idler, job_of};
        let mut child = idler(30);
        let job = job_of(&child);
        assert!(crate::procs::tie(&job, "reopen-tie"));
        crate::procs::let_go(&job);
        assert_eq!(retie(&[("reopen-tie".to_string(), job.clone())]), [("reopen-tie".to_string(), true)]);
        let job = std::sync::Arc::try_unwrap(job).ok().expect("the one handle left");
        crate::procs::close_as_in_a_death(job);
        assert!(ended_within(&mut child, 3000), "tied again, the job ends what is in it when the core dies");
    }

    /// What `taken_up` does for a new core, done here for the same one: a turn this
    /// core stopped is over, so the helper may resume; the prompt of one with no
    /// conversation yet is what it is owed first; a doubt an older core left stands.
    /// On entries of its own: the registry is every test's.
    #[test]
    fn a_helper_this_core_stopped_may_resume_once_the_core_stays_on() {
        // Stopped as `stop_all` stops a turn; the process is reaped by its thread
        // before the fate of the shutdown is known, which none ran here to do.
        let stopped = |mut e: Entry| {
            stop_for_luna(&mut e);
            e.run = None;
            settle_stopped(&mut e, &|_| None);
            e
        };
        for provider in [Provider::Claude, Provider::Codex] {
            let mut e = entry(provider);
            e.session = Some("sid".into());
            e.run = Some(Run::claimed(None, 0));
            e.queue = vec!["behind it".into()];
            let e = stopped(e);
            assert!(e.cut_short && e.resumable && !e.unsure, "{}", provider.as_str());
            assert_eq!(e.undelivered, ["behind it"], "{}", provider.as_str());
        }
        let mut e = entry(Provider::Claude);
        e.run = Some(Run::claimed(None, 0));
        e.opening = Some("the prompt".into());
        e.opening_id = Some("line-1".into());
        let e = stopped(e);
        assert!(e.resumable && e.opening.is_none());
        assert_eq!(e.undelivered, ["the prompt"]);
        let mut e = taken_up(with_lines(false));
        settle_stopped(&mut e, &|_| None);
        assert!(e.cut_short && !e.resumable && e.unsure, "nothing says the older core's turn ended");
    }

    /// What a stopped Claude Code turn had on stdin goes by the conversation
    /// before the helper resumes, as it does for a new core: in it, the model has
    /// it and it is not sent again; not in it, with the turn over, it never got
    /// there; unreadable, its sender decides. Codex has no lines to settle.
    #[test]
    fn what_a_stopped_turn_had_on_stdin_is_settled_against_the_conversation_before_it_resumes() {
        let mut e = entry(Provider::Claude);
        e.session = Some("sid".into());
        e.run = Some(Run::claimed(None, 0));
        e.fed = vec![pending("told", "went in"), pending("missed", "not taken in")];
        e.queue = vec!["behind it".into()];
        stop_for_luna(&mut e);
        e.run = None;
        settle_stopped(&mut e, &|line| Some(line == "told"));
        assert_eq!(e.undelivered, ["not taken in", "behind it"], "the one the model has is off the books");
        assert!(e.undelivered.iter().all(|q| !q.maybe_reached && q.line.is_none()), "the turn is over: what is not there never got there");

        let mut e = entry(Provider::Claude);
        e.session = Some("sid".into());
        e.run = Some(Run::claimed(None, 0));
        e.fed = vec![pending("a", "joined")];
        stop_for_luna(&mut e);
        e.run = None;
        settle_stopped(&mut e, &|_| None);
        assert!(e.resumable && e.undelivered[0].maybe_reached, "unreadable: withheld, its sender's to decide");

        let mut e = entry(Provider::Codex);
        e.session = Some("thread".into());
        e.run = Some(Run::claimed(None, 0));
        e.in_flight = vec![pending("c", "flying")];
        stop_for_luna(&mut e);
        e.run = None;
        settle_stopped(&mut e, &|_| panic!("Codex has no such lines"));
        assert_eq!(e.undelivered, ["flying"]);
        assert!(!e.undelivered[0].maybe_reached);
    }
}
