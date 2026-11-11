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
    pub spawned_at_ms: u128,
    pub resume: Option<String>,
    pub account_path: String,
    pub last_output_ms: u64,
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
        spawned_at_ms: s.spawned_at_ms,
        resume: s.resume.clone(),
        account_path: s.account_path.clone(),
        last_output_ms: s.last_output_ms.load(Ordering::Relaxed),
        job: s.job.clone(),
        asking: false,
    }
}

/// The cwd of every live session this app runs, and where every background
/// helper works. A running session must never have its worktree swept, even
/// if the UI has not yet learned where it lives, and a helper has no row to
/// claim its own.
pub fn live_cwds() -> Vec<String> {
    let mut cwds: Vec<String> = manager()
        .activity_probes()
        .into_iter()
        .flat_map(|p| {
            let moved = p.working_dir();
            [Some(p.cwd), moved].into_iter().flatten()
        })
        .collect();
    cwds.extend(crate::background::working_dirs(None));
    cwds
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

// Name, status and context of a live session, read off what its CLI leaves on
// disk — free, no tokens.
//
// Async on purpose: this runs every few seconds for every open pane and reads
// the tail of a transcript that grows into the megabytes. As a sync command it
// would do that on the main thread, i.e. with the window frozen.
#[tauri::command]
//
// A chat with no live session answers None rather than failing: the watcher
// polls every chat every few seconds, resting ones included, so treating an
// absent session as an error meant thousands of warnings a day in the log —
// enough to push everything worth reading out of it.
pub async fn session_meta(id: String, account_path: String) -> Result<Option<SessionMeta>, String> {
    let state = manager();
    let probe = {
        let sessions = state.sessions.lock().unwrap();
        let Some(s) = sessions.get(&id) else {
            return Ok(None);
        };
        if !s.alive.load(Ordering::SeqCst) {
            return Ok(None);
        }
        let mut p = probe_of(&id, s);
        // The caller's idea of the account wins, as it always has: the chat
        // row is the source of truth for which folder a chat belongs to.
        if !account_path.is_empty() {
            p.account_path = account_path;
        }
        p
    };

    let codex = probe.provider == Provider::Codex;
    let mut meta = tauri::async_runtime::spawn_blocking(move || meta_from_disk(&probe)).await.map_err(|e| e.to_string())?;
    // Codex's approval dialog is not in its rollout and the screen does not
    // go quiet under it, so the rollout calls the session working; the
    // activity sampler reads the dialog off the screen (`codex::session::asks`).
    if let Some(m) = meta.as_mut().filter(|m| codex && m.status.as_deref() == Some("working")) {
        if crate::activity::turn_state(&id).is_some_and(|t| t.turn == crate::activity::Turn::Waiting) {
            m.status = Some("waiting".into());
        }
    }
    if let Some(m) = meta.as_mut() {
        crate::asking::overlay(&id, m);
        overlay_background(&id, m);
        // The screen may be read for it, which replays the scrollback: off
        // the main thread, like the disk.
        let of = id.clone();
        m.draft = tauri::async_runtime::spawn_blocking(move || manager().has_draft(&of)).await.unwrap_or(false);
        m.held = crate::agents::held_count(&id);
        m.handoff = crate::agents::mark_of(&id);
        m.handoff_asked = crate::agents::asked_of(&id);
        pin_name(m);
    }
    Ok(meta)
}

/// A chat between turns with work of its own going on reads `background`, under
/// waiting and working. Claude Code's registry says busy for as long as a
/// subagent works, its turn over or not: only with the turn over, by its
/// hooks, is that busy the subagent's. A Codex rollout says idle whatever its
/// subagents do.
fn overlay_background(id: &str, m: &mut SessionMeta) {
    use crate::activity::{self, Turn};
    let said = m.status.as_deref().map_or(Turn::Idle, activity::turn_of);
    let only_subagents =
        said == Turn::Busy && activity::subagents(id) > 0 && activity::turn_state(id).is_some_and(|t| t.turn == Turn::Idle);
    if (said == Turn::Idle || only_subagents) && activity::in_background(id) {
        m.status = Some("background".into());
    }
}

/// A conversation a handoff opened begins on Luna's prompt, which is no name
/// for the chat: it keeps the one it had (Codex names a chat after its
/// conversation's first prompt).
fn pin_name(m: &mut SessionMeta) {
    if m.first_prompt.as_deref().is_some_and(|p| p.trim_start().starts_with(crate::agents::handoff::PROMPT_TAG)) {
        m.first_prompt = None;
    }
}

#[cfg(test)]
mod background_tests {
    use super::*;
    use crate::activity::{forget, on_hook, subagents_for_test, turn_for_test, HookEvent, Turn};

    fn reads(id: &str, status: Option<&str>) -> Option<String> {
        let mut m = SessionMeta { status: status.map(Into::into), ..Default::default() };
        overlay_background(id, &mut m);
        m.status
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
        subagents_for_test(id, 1);
        assert_eq!(reads(id, Some("waiting")).as_deref(), Some("waiting"), "a marker's or a prompt's");
        turn_for_test(id, Turn::Busy);
        assert_eq!(reads(id, Some("busy")).as_deref(), Some("busy"), "its turn runs");
        turn_for_test(id, Turn::Waiting);
        assert_eq!(reads(id, Some("busy")).as_deref(), Some("busy"), "a prompt inside a busy registry is not background");
        forget(id);
    }

    /// As in `a_background_agent_is_no_turn`: the turn is over and the registry
    /// goes on saying busy for the agent it started.
    #[test]
    fn claude_busy_only_for_its_subagents_reads_background() {
        let id = "ob-claude";
        on_hook(id, HookEvent::Busy);
        on_hook(id, HookEvent::AgentAtWork("a1".into()));
        on_hook(id, HookEvent::Idle);
        turn_for_test(id, Turn::Idle);
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
    fn a_conversation_a_handoff_opened_keeps_the_chats_name() {
        let named = |first: &str| {
            let mut m = SessionMeta { first_prompt: Some(first.into()), ..Default::default() };
            pin_name(&mut m);
            m.first_prompt
        };
        assert_eq!(named("[Luna handoff 2, not typed by the user] This is a new conversation…"), None);
        assert_eq!(named("Fix the updater"), Some("Fix the updater".into()), "a person's first prompt names it");
    }
}

/// The CLI's title for a session that is not running, by its id — how a chat
/// restored from an earlier run picks up the title its CLI gave it.
#[tauri::command]
pub async fn saved_title(provider: Provider, account_path: String, session_id: String) -> Option<CliTitle> {
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
