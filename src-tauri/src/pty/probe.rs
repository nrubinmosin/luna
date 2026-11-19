//! What is read of a session from outside it: the activity sampler's probes,
//! what its CLI leaves on disk (name, status, context), the tails of files,
//! and the sweep of hook and MCP files no session needs any more.

use super::screen::{CliTitle, SessionMeta};
use super::{manager, PtyManager, Session};
use crate::provider::Provider;
use std::io::Read;
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// Removes the hook and MCP files of sessions that are not running. Each is
/// written afresh when its session starts or resumes, so one belonging to a
/// chat that is gone — deleted some way other than through Luna, lost to a
/// crash — is only clutter, and one belonging to a chat that is not running
/// comes back when it does. Ten minutes' grace, so a file is never pulled
/// from under a session that is being spawned right now.
fn prune_session_files(live: &std::collections::HashSet<String>) -> usize {
    // A session's files go by its id spelled as a file name.
    let live: std::collections::HashSet<String> = live.iter().map(|id| crate::paths::id_file_name(id)).collect();
    let mut removed = 0;
    for sub in ["hooks", "mcp"] {
        let Ok(entries) = std::fs::read_dir(crate::paths::data_dir().join(sub)) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_stem().and_then(|s| s.to_str()) else { continue };
            if path.extension().and_then(|e| e.to_str()) != Some("json") || live.contains(name) {
                continue;
            }
            let fresh = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_none_or(|age| age < std::time::Duration::from_secs(600));
            if !fresh && std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
    }
    // Long messages handed over as files (agents/delivery.rs `spill`): read at once,
    // kept a week for whoever wants to see what was sent.
    if let Ok(entries) = std::fs::read_dir(crate::agents::notes_dir()) {
        for entry in entries.flatten() {
            let old = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > std::time::Duration::from_secs(7 * 24 * 3600));
            if old && std::fs::remove_file(entry.path()).is_ok() {
                removed += 1;
            }
        }
    }
    removed
}

/// At startup and every few hours after, like the media sweep: the app can
/// run for weeks without a restart.
pub fn prune_session_files_periodically() {
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(15 * 60));
        let live: std::collections::HashSet<String> = {
            let state = manager();
            let sessions = state.sessions.lock().unwrap_or_else(|e| e.into_inner());
            sessions.keys().cloned().collect()
        };
        let removed = prune_session_files(&live);
        if removed > 0 {
            crate::log::info("pty", &format!("removed {removed} hook, mcp and note file(s) no longer needed"));
        }
        std::thread::sleep(std::time::Duration::from_secs(6 * 3600 - 15 * 60));
    });
}

/// What the activity sampler needs of a live session (activity/).
pub struct ActivityProbe {
    pub id: String,
    pub provider: Provider,
    pub pid: Option<u32>,
    pub cwd: String,
    /// The worktree it was started into, where that is not its `cwd`
    /// (`Session::enters`).
    pub enters: Option<String>,
    /// What it holds against a worktree's removal (`Session::hold`).
    pub hold: Arc<crate::worktree::Hold>,
    pub spawned_at_ms: u128,
    pub resume: Option<String>,
    pub account_path: String,
    pub last_output_ms: u64,
    /// Its last output that was not a repaint after a resize or a focus
    /// report: what counts as it printing (`Session::work_output_ms`).
    pub work_output_ms: u64,
    pub job: Option<Arc<crate::procs::Job>>,
    /// Codex only: its screen shows an approval dialog (see
    /// `codex::session::asks`). Filled in by the activity sampler, which
    /// renders the screen outside the session map.
    pub asking: bool,
}

impl ActivityProbe {
    /// Where the CLI works now, when it has moved from where it started: both
    /// make their `--worktree` checkout after the spawn, and only what they
    /// leave on disk says where — Claude Code's registry, Codex's rollout.
    pub fn working_dir(&self) -> Option<String> {
        match self.provider {
            Provider::Claude => {
                crate::claude::session::session_cwd(self.pid, self.resume.as_deref(), &self.cwd, self.spawned_at_ms, &self.account_path)
            }
            Provider::Codex => crate::codex::session::rollout_cwd(&crate::codex::session::Live {
                chat_id: &self.id,
                cwd: &self.cwd,
                spawned_at_ms: self.spawned_at_ms,
                resume: self.resume.as_deref(),
                account_path: &self.account_path,
                last_output_ms: self.last_output_ms,
            }),
        }
    }
}

fn activity_probe(id: &str, s: &Session) -> ActivityProbe {
    ActivityProbe {
        id: id.to_string(),
        provider: s.provider,
        pid: s.pid,
        cwd: s.cwd.clone(),
        enters: s.enters.clone(),
        hold: Arc::clone(&s.hold),
        spawned_at_ms: s.spawned_at_ms,
        resume: s.resume.clone(),
        account_path: s.account_path.clone(),
        last_output_ms: s.last_output_ms.load(Ordering::Relaxed),
        work_output_ms: s.work_output_ms.load(Ordering::Relaxed),
        job: s.job.clone(),
        asking: false,
    }
}

/// The cwd of every live session this app runs, and where every background
/// helper works, as spelled: what a spawn cut off by a core going left is
/// told from by them (agents/spawn). Whether a worktree may be removed goes
/// by places (`holds`).
pub fn live_cwds() -> Vec<String> {
    let mut cwds: Vec<String> = manager()
        .activity_probes()
        .into_iter()
        .flat_map(|p| {
            let moved = p.working_dir();
            [Some(p.cwd), p.enters, moved].into_iter().flatten()
        })
        .collect();
    cwds.extend(crate::background::working_dirs(None));
    cwds
}

/// What Luna's live sessions and helpers, all but `except`, hold against a
/// worktree's removal (`worktree::Hold`).
pub fn holds(except: Option<&str>) -> crate::worktree::Hold {
    manager().holds(except)
}

impl PtyManager {
    /// What the live sessions and helpers but `except` hold against a
    /// worktree's removal: each session's hold, resolved as it started, with
    /// the folder its CLI says it works in now (a worktree it made: as its
    /// start's places led then if it is under one of them by their spelling,
    /// and as it leads now, `Hold::add_at`) and, until it has said one, where
    /// its CLI makes the worktree it asked for; each helper's alike
    /// (`background::holds`). Both of a session come of one read of its
    /// folder, both of a helper of one read of where it works.
    pub fn holds(&self, except: Option<&str>) -> crate::worktree::Hold {
        let mut held = crate::background::holds(except);
        // Probed off the session map: the disk reads below must not hold it.
        for p in self.activity_probes().into_iter().filter(|p| Some(p.id.as_str()) != except) {
            let moved = p.working_dir();
            held.add(&p.hold, moved.as_deref().is_none_or(|w| super::same_dir(w, &p.cwd)));
            if let Some(moved) = moved {
                held.add_at(&p.hold, &moved);
            }
        }
        held
    }
}

/// The last `max` bytes of a file as text, or None if it cannot be read.
pub fn tail(path: &std::path::Path, max: u64) -> Option<String> {
    use std::io::{Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let take = len.min(max);
    f.seek(SeekFrom::End(-(take as i64))).ok()?;
    let mut bytes = Vec::with_capacity(take as usize);
    f.read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// Calls `each` with every complete line of a file from byte `from` on and
/// returns the offset just past the last of them. A last line without its
/// newline is still being written: it is left, cursor and all, for the read
/// after.
pub fn complete_lines(path: &std::path::Path, from: u64, mut each: impl FnMut(&str)) -> u64 {
    use std::io::{BufRead, BufReader, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else { return from };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let from = from.min(len);
    if f.seek(SeekFrom::Start(from)).is_err() {
        return from;
    }
    let mut reader = BufReader::new(f);
    let mut line = Vec::new();
    let mut read = from;
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(n) if line.ends_with(b"\n") => read += n as u64,
            _ => break,
        }
        each(String::from_utf8_lossy(&line).trim_end_matches(['\r', '\n']));
    }
    read
}

/// `complete_lines` over bytes in memory, from the top: each complete line
/// handed over as that does, and the offset just past the last of them.
pub fn complete_lines_in(bytes: &[u8], mut each: impl FnMut(&str)) -> usize {
    let mut read = 0;
    while let Some(end) = bytes[read..].iter().position(|b| *b == b'\n') {
        let line = &bytes[read..=read + end];
        each(String::from_utf8_lossy(line).trim_end_matches(['\r', '\n']));
        read += line.len();
    }
    read
}

/// Every line a reader holds, a trailing CR dropped, decoded as the rest of
/// the transcript readers decode (`tail`, `complete_lines`): a byte that is
/// not UTF-8 reads as U+FFFD rather than ending the lines or losing one.
pub fn read_lossy_lines(reader: impl std::io::BufRead) -> impl Iterator<Item = String> {
    reader.split(b'\n').map_while(Result::ok).map(|line| String::from_utf8_lossy(line.strip_suffix(b"\r").unwrap_or(&line)).into_owned())
}

/// `read_lossy_lines` of bytes in memory.
pub fn lossy_lines(bytes: &[u8]) -> impl Iterator<Item = std::borrow::Cow<'_, str>> {
    // As a reader splits: no line after a last newline, and none in nothing.
    let body = (!bytes.is_empty()).then(|| bytes.strip_suffix(b"\n").unwrap_or(bytes));
    body.into_iter().flat_map(|b| b.split(|c| *c == b'\n')).map(|line| String::from_utf8_lossy(line.strip_suffix(b"\r").unwrap_or(line)))
}

/// What a session-meta read needs, copied out from under the map.
pub(super) struct Probe {
    pub(super) id: String,
    pub(super) provider: Provider,
    pub(super) pid: Option<u32>,
    pub(super) cwd: String,
    pub(super) spawned_at_ms: u128,
    pub(super) resume: Option<String>,
    pub(super) account_path: String,
    pub(super) last_output_ms: u64,
}

pub(super) fn probe_of(id: &str, s: &Session) -> Probe {
    Probe {
        id: id.to_string(),
        provider: s.provider,
        pid: s.pid,
        cwd: s.cwd.clone(),
        spawned_at_ms: s.spawned_at_ms,
        resume: s.resume.clone(),
        account_path: s.account_path.clone(),
        last_output_ms: s.last_output_ms.load(Ordering::Relaxed),
    }
}

pub(super) fn meta_from_disk(p: &Probe) -> Option<SessionMeta> {
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

/// A chat the window asks about (`sessions_meta`): its id, and the account
/// folder its row names.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaAsk {
    pub id: String,
    pub account_path: String,
}

/// What `sessions_meta` says of one chat.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatLook {
    pub id: String,
    /// None: no live session (its CLI exited or never started), or an
    /// account folder that is none of Luna's.
    pub meta: Option<SessionMeta>,
}

/// Name, status and context of every chat the window shows, read off what
/// each CLI leaves on disk — free, no tokens — in one call a tick: the window
/// used to ask for each chat in turn, every few seconds, one round trip and
/// thread each, and each check of the same account folder again. A chat
/// with no live session answers no meta rather than failing: resting chats
/// are asked about too, and an error each meant thousands of warnings a day
/// in the log. Each account path is held to an account folder of Luna's
/// (`boundary::AccountPath`) once a call, before anything under it is read;
/// a chat whose path is refused answers no meta, as one whose own call
/// failed did. The files read are kept by their stamp (`stamped`), so at
/// idle a tick reads next to nothing.
pub fn sessions_meta(chats: Vec<MetaAsk>) -> Vec<ChatLook> {
    looks(chats, |path| crate::boundary::AccountPath::try_from(path.to_string()).is_ok(), session_meta)
}

/// `sessions_meta`, the account check and the look at one chat given.
fn looks(
    chats: Vec<MetaAsk>,
    mut account_ok: impl FnMut(&str) -> bool,
    mut look: impl FnMut(&str, String) -> Option<SessionMeta>,
) -> Vec<ChatLook> {
    let mut checked: std::collections::HashMap<String, bool> = std::collections::HashMap::new();
    chats
        .into_iter()
        .map(|c| {
            let ok = match checked.get(&c.account_path) {
                Some(ok) => *ok,
                None => *checked.entry(c.account_path.clone()).or_insert(account_ok(&c.account_path)),
            };
            let meta = if ok { look(&c.id, c.account_path) } else { None };
            ChatLook { id: c.id, meta }
        })
        .collect()
}

/// Name, status and context of a live session, None for one that is not
/// running. Account path checked by the caller (`sessions_meta`).
fn session_meta(id: &str, account_path: String) -> Option<SessionMeta> {
    let probe = {
        let sessions = manager().sessions.lock().unwrap_or_else(|e| e.into_inner());
        let s = sessions.get(id).filter(|s| s.alive.load(Ordering::SeqCst))?;
        let mut p = probe_of(id, s);
        // The caller's idea of the account wins, as it always has: the chat
        // row is the source of truth for which folder a chat belongs to.
        p.account_path = account_path;
        p
    };

    let codex = probe.provider == Provider::Codex;
    let mut m = meta_from_disk(&probe)?;
    // Codex's approval dialog is not in its rollout and the screen does not
    // go quiet under it, so the rollout calls the session working; the
    // activity sampler reads the dialog off the screen (`codex::session::asks`).
    let asking = || crate::activity::turn_state(id).is_some_and(|t| t.turn == crate::activity::Turn::Waiting);
    if codex && m.status.as_deref() == Some("working") && asking() {
        m.status = Some("waiting".into());
    }
    crate::asking::overlay(id, &mut m);
    overlay_background(id, &mut m);
    m.draft = manager().has_draft(id);
    m.held = crate::agents::held_count(id);
    m.limit = crate::activity::limit_of(id, probe.provider);
    pin_name(&mut m);
    Some(m)
}

/// A conversation 0.19's handoff or a move opened begins on Luna's prompt
/// (`[Luna handoff N, …]`, `[Luna move …]`), which is no name for the chat: it
/// keeps the one it had (Codex names a chat after its conversation's first
/// prompt). Luna types no such prompt since 0.20 (ADR-0030); a chat resumed on
/// such a conversation still has one.
fn pin_name(m: &mut SessionMeta) {
    const RETIRED_TAGS: [&str; 2] = ["[Luna handoff", "[Luna move"];
    if m.first_prompt.as_deref().is_some_and(|p| RETIRED_TAGS.iter().any(|tag| p.trim_start().starts_with(tag))) {
        m.first_prompt = None;
    }
}

/// A chat between turns with work of its own going on reads `background`, under
/// waiting and working. Claude Code's registry says busy for as long as a
/// subagent works, its turn over or not: only with the turn over, by its
/// hooks, is that busy the subagent's. A hook that says a turn has begun is
/// newer than any sample, and an idle registry may not have caught up with it
/// yet. With the turn over, a subagent on a prompt is a wait, as the sampler
/// has it. A Codex rollout says idle whatever its subagents do, and working as
/// soon as a turn starts.
fn overlay_background(id: &str, m: &mut SessionMeta) {
    use crate::activity::{self, Turn};
    let over = match m.status.as_deref().map_or(Turn::Idle, activity::turn_of) {
        Turn::Idle => activity::hook_turn(id) != Some(Turn::Busy),
        Turn::Busy if activity::turn_over_under_subagents(id) && activity::subagent_asks(id) => {
            m.status = Some("waiting".into());
            return;
        }
        Turn::Busy => activity::turn_over_under_subagents(id),
        Turn::Waiting => false,
    };
    if over && activity::in_background(id) {
        m.status = Some("background".into());
    }
}

#[cfg(test)]
mod background_tests {
    use super::*;
    use crate::activity::{forget, on_hook, sampled_for_test, subagents_for_test, HookEvent, Turn};

    const MARKER: &str = "AWAITING OPERATOR";

    fn reads(id: &str, status: Option<&str>) -> Option<String> {
        said_and_reads(id, status, "All done.")
    }

    /// What a chat reads as `session_meta` works it out, its CLI's word being
    /// `status` and its last message `said`.
    fn said_and_reads(id: &str, status: Option<&str>, said: &str) -> Option<String> {
        let mut m = SessionMeta { status: status.map(Into::into), last_said: Some(said.into()), ..Default::default() };
        crate::asking::overlay_marked(id, &mut m, MARKER);
        overlay_background(id, &mut m);
        m.status
    }

    /// A turn that started a subagent and ended with it at work, by the hooks.
    fn turn_over_with_a_subagent(id: &str) {
        on_hook(id, HookEvent::Busy);
        on_hook(id, HookEvent::AgentAtWork("a1".into()));
        on_hook(id, HookEvent::Idle);
    }

    #[test]
    fn a_chat_between_turns_with_a_job_reads_background() {
        let id = "ob-idle";
        assert_eq!(reads(id, Some("idle")).as_deref(), Some("idle"), "nothing goes on");
        assert_eq!(reads(id, None), None);
        subagents_for_test(id, 1);
        assert_eq!(reads(id, Some("idle")).as_deref(), Some("background"));
        assert_eq!(reads(id, Some("resting")).as_deref(), Some("background"), "Codex's word for it");
        assert_eq!(reads(id, None).as_deref(), Some("background"));
        forget(id);
    }

    #[test]
    fn waiting_and_working_beat_background() {
        let id = "ob-beat";
        on_hook(id, HookEvent::Busy);
        on_hook(id, HookEvent::AgentAtWork("a1".into()));
        assert_eq!(reads(id, Some("waiting")).as_deref(), Some("waiting"), "a prompt's");
        sampled_for_test(id, Some(Turn::Busy));
        assert_eq!(reads(id, Some("busy")).as_deref(), Some("busy"), "its turn runs");
        on_hook(id, HookEvent::Idle);
        on_hook(id, HookEvent::AgentWaiting("a1".into()));
        sampled_for_test(id, Some(Turn::Busy));
        assert_eq!(reads(id, Some("busy")).as_deref(), Some("waiting"), "a subagent's prompt after the turn is a wait, not background");
        forget(id);
    }

    /// The registry goes on saying busy for the subagent, so only the hooks
    /// tell that the turn ended, on a question to the user.
    #[test]
    fn a_turn_ended_on_the_marker_waits_while_its_subagent_works() {
        let id = "ob-marker";
        let asks = "Ship it?\n\n`STATUS: AWAITING OPERATOR`";
        turn_over_with_a_subagent(id);
        assert_eq!(said_and_reads(id, Some("busy"), asks).as_deref(), Some("waiting"), "before any sample");
        sampled_for_test(id, Some(Turn::Busy));
        assert_eq!(said_and_reads(id, Some("busy"), asks).as_deref(), Some("waiting"), "and after one");
        assert_eq!(said_and_reads(id, Some("busy"), "All done.").as_deref(), Some("background"), "no marker");
        on_hook(id, HookEvent::AgentWaiting("a1".into()));
        sampled_for_test(id, Some(Turn::Busy));
        let mut m = SessionMeta { status: Some("busy".into()), last_said: Some(asks.into()), ..Default::default() };
        crate::asking::overlay_marked(id, &mut m, MARKER);
        overlay_background(id, &mut m);
        assert_eq!(m.status.as_deref(), Some("waiting"), "its subagent asks too: the turn's own question stands");
        assert!(m.marked.is_some(), "on the marker, which the user may clear");
        assert_eq!(said_and_reads(id, Some("busy"), "All done.").as_deref(), Some("waiting"), "no marker: the subagent's prompt");
        crate::asking::forget(id);
        forget(id);
    }

    #[test]
    fn a_new_prompt_is_working_before_the_next_sample() {
        let id = "ob-prompt";
        turn_over_with_a_subagent(id);
        sampled_for_test(id, Some(Turn::Busy));
        assert_eq!(reads(id, Some("busy")).as_deref(), Some("background"), "the sample said the turn is over");
        on_hook(id, HookEvent::Busy);
        assert_eq!(reads(id, Some("busy")).as_deref(), Some("busy"), "the hook is newer than the sample");
        assert_eq!(reads(id, Some("idle")).as_deref(), Some("idle"), "nor while the registry catches up");
        sampled_for_test(id, Some(Turn::Busy));
        assert_eq!(reads(id, Some("busy")).as_deref(), Some("busy"));
        forget(id);
    }

    /// Codex has no hooks: its rollout says a turn has started before any
    /// sample does.
    #[test]
    fn a_codex_turn_is_working_before_the_next_sample() {
        let id = "ob-codex";
        subagents_for_test(id, 1);
        sampled_for_test(id, Some(Turn::Idle));
        assert_eq!(reads(id, Some("resting")).as_deref(), Some("background"));
        assert_eq!(reads(id, Some("working")).as_deref(), Some("working"));
        forget(id);
    }

    /// As in `a_background_agent_is_no_turn`: the turn is over and the registry
    /// goes on saying busy for the agent it started.
    #[test]
    fn claude_busy_only_for_its_subagents_reads_background() {
        let id = "ob-claude";
        turn_over_with_a_subagent(id);
        sampled_for_test(id, Some(Turn::Busy));
        assert_eq!(reads(id, Some("busy")).as_deref(), Some("background"));
        on_hook(id, HookEvent::AgentDone("a1".into()));
        assert_eq!(reads(id, Some("busy")).as_deref(), Some("busy"), "no subagent left: a turn on its result");
        forget(id);
    }
}

#[cfg(test)]
mod pin_tests {
    use super::*;

    #[test]
    fn a_conversation_a_0_19_handoff_opened_keeps_the_chats_name() {
        let named = |first: &str| {
            let mut m = SessionMeta { first_prompt: Some(first.into()), ..Default::default() };
            pin_name(&mut m);
            m.first_prompt
        };
        assert_eq!(named("[Luna handoff 2, not typed by the user] This is a new conversation…"), None);
        assert_eq!(named("[Luna move 3fa9c2, not typed by the user] This chat goes on from…"), None, "nor a move's line");
        assert_eq!(named("Fix the updater"), Some("Fix the updater".into()), "a person's first prompt names it");
    }

    /// Each account folder is checked once a call, and nothing of a chat
    /// whose folder is refused is looked at.
    #[test]
    fn a_chat_on_a_refused_account_is_not_looked_at_and_each_account_is_checked_once() {
        let ask = |id: &str, account: &str| MetaAsk { id: id.into(), account_path: account.into() };
        let (mut checks, mut looked) = (Vec::new(), Vec::new());
        let got = looks(
            vec![ask("lk-1", "A"), ask("lk-2", "bad"), ask("lk-3", "A"), ask("lk-4", "bad")],
            |path| {
                checks.push(path.to_string());
                path == "A"
            },
            |id, account| {
                looked.push(format!("{id}@{account}"));
                (id == "lk-1").then(SessionMeta::default)
            },
        );
        assert_eq!(checks, ["A", "bad"]);
        assert_eq!(looked, ["lk-1@A", "lk-3@A"]);
        let ids: Vec<(&str, bool)> = got.iter().map(|l| (l.id.as_str(), l.meta.is_some())).collect();
        assert_eq!(ids, [("lk-1", true), ("lk-2", false), ("lk-3", false), ("lk-4", false)]);
    }

    /// `saved_title` reads nothing under a folder that is no account of
    /// Luna's: its account path is refused as the page's arguments are read,
    /// though a transcript with a title lies there to be found.
    #[test]
    fn a_saved_title_is_not_looked_for_outside_lunas_accounts() {
        let root = std::env::temp_dir().join(format!("luna-saved-title-elsewhere-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("projects").join("E--p");
        std::fs::create_dir_all(&dir).unwrap();
        let sid = "4d1f0e2a-9b7c-4c1e-8f3a-2b6d5e7f9a10";
        std::fs::write(dir.join(format!("{sid}.jsonl")), format!(r#"{{"type":"ai-title","aiTitle":"Planted","sessionId":"{sid}"}}"#))
            .unwrap();
        let acct = root.to_string_lossy().into_owned();
        assert!(crate::claude::session::saved_title(&acct, sid).is_some(), "there to be read, were the path let through");
        type Arg = crate::boundary::OrEmpty<crate::boundary::AccountPath>;
        let refused = serde_json::from_value::<Arg>(serde_json::json!(acct));
        assert!(refused.is_err_and(|e| e.to_string().contains("account folder")), "the command's account path takes only Luna's");
        let none = serde_json::from_value::<Arg>(serde_json::json!("")).unwrap();
        assert_eq!(tauri::async_runtime::block_on(saved_title(Provider::Claude, none, sid.into())), None, "and no path is no title");
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// The CLI's title for a session that is not running, by its id — how a chat
/// restored from an earlier run picks up the title its CLI gave it. The
/// account path is held to an account folder of Luna's as it is read
/// (`boundary::AccountPath`), before anything under it is; a chat with none
/// has no title saved anywhere.
#[tauri::command]
pub async fn saved_title(
    provider: Provider,
    account_path: crate::boundary::OrEmpty<crate::boundary::AccountPath>,
    session_id: String,
) -> Option<CliTitle> {
    let account_path = account_path.into_string();
    if account_path.is_empty() {
        return None;
    }
    tauri::async_runtime::spawn_blocking(move || match provider {
        Provider::Claude => crate::claude::session::saved_title(&account_path, &session_id),
        Provider::Codex => crate::codex::session::thread_title(&account_path, &session_id),
    })
    .await
    .ok()
    .flatten()
}

impl PtyManager {
    /// Every live session, copied out from under the map.
    pub fn activity_probes(&self) -> Vec<ActivityProbe> {
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions.iter().filter(|(_, s)| s.alive.load(Ordering::SeqCst)).map(|(id, s)| activity_probe(id, s)).collect()
    }

    /// One live session, or None.
    pub fn probe(&self, id: &str) -> Option<ActivityProbe> {
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions.get(id).filter(|s| s.alive.load(Ordering::SeqCst)).map(|s| activity_probe(id, s))
    }
}
