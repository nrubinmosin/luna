//! What is read of a session from outside it: the records' probes,
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
    // The folder-skill plugins of sessions that are gone (library/session_plugin.rs):
    // each is made afresh when its session starts or has a turn, and taken away
    // with the session, so one left by a crash or a refused launch is clutter.
    // Taken out by name through the library's own removal, which follows no link.
    if let Ok(entries) = std::fs::read_dir(crate::paths::data_dir().join(crate::library::PLUGINS_DIR)) {
        for entry in entries.flatten() {
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else { continue };
            let fresh = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_none_or(|age| age < std::time::Duration::from_secs(600));
            if !live.contains(&name) && !fresh {
                crate::library::drop_plugins_named(&name);
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
        // The chats running and the background helpers, between turns too:
        // a helper's plugin is read while its turn runs.
        let live: std::collections::HashSet<String> = {
            let state = manager();
            let sessions = state.sessions.lock().unwrap_or_else(|e| e.into_inner());
            sessions.keys().cloned().chain(crate::background::ids()).collect()
        };
        let removed = prune_session_files(&live);
        if removed > 0 {
            crate::log::info("pty", &format!("removed {removed} hook, mcp and note file(s) no longer needed"));
        }
        std::thread::sleep(std::time::Duration::from_secs(6 * 3600 - 15 * 60));
    });
}

#[cfg(all(test, unix))]
mod prune_tests {
    use super::*;

    /// A session's folder-skill plugin that nothing runs and nobody has made
    /// for ten minutes goes; a running session's and a fresh one stay.
    #[test]
    fn the_sweep_takes_plugins_of_sessions_that_are_gone() {
        let top = crate::paths::data_dir().join(crate::library::PLUGINS_DIR);
        let name = |s: &str| crate::paths::id_file_name(s);
        let age = |dir: &std::path::Path| {
            let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
            std::fs::File::open(dir).unwrap().set_modified(old).unwrap();
        };
        for s in ["sweep-gone", "sweep-live", "sweep-fresh"] {
            std::fs::create_dir_all(top.join(name(s)).join("repo")).unwrap();
        }
        age(&top.join(name("sweep-gone")));
        age(&top.join(name("sweep-live")));
        let live: std::collections::HashSet<String> = ["sweep-live".to_string()].into();
        prune_session_files(&live);
        assert!(!top.join(name("sweep-gone")).exists(), "gone, and not made for ten minutes");
        assert!(top.join(name("sweep-live")).exists(), "a running session's");
        assert!(top.join(name("sweep-fresh")).exists(), "made a moment ago: a launch under way");
        for s in ["sweep-live", "sweep-fresh"] {
            let _ = std::fs::remove_dir_all(top.join(name(s)));
        }
    }
}

/// What the reads behind a session's lifecycle record need of a live session
/// (activity/).
pub struct ActivityProbe {
    pub id: String,
    pub provider: Provider,
    pub pid: Option<u32>,
    pub cwd: String,
    pub spawned_at_ms: u128,
    pub resume: Option<String>,
    pub account_path: String,
    pub job: Option<Arc<crate::procs::Job>>,
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
        job: s.job.clone(),
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
    let mut read = from;
    match lines_from(path, from, |end, line| {
        read = end;
        each(line);
    }) {
        Ok(end) => end,
        Err(_) => read,
    }
}

/// `complete_lines`, each line with the offset just past it, and an error for
/// a file that would not open, seek or read through: what was read before it
/// was handed over, but a reader that keeps a word of its own must not take
/// the file to have said nothing new.
pub fn lines_from(path: &std::path::Path, from: u64, mut each: impl FnMut(u64, &str)) -> std::io::Result<u64> {
    use std::io::{BufRead, BufReader, Seek, SeekFrom};
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();
    let from = from.min(len);
    f.seek(SeekFrom::Start(from))?;
    let mut reader = BufReader::new(f);
    let mut line = Vec::new();
    let mut read = from;
    loop {
        line.clear();
        let n = reader.read_until(b'\n', &mut line)?;
        if !line.ends_with(b"\n") {
            break;
        }
        read += n as u64;
        each(read, String::from_utf8_lossy(&line).trim_end_matches(['\r', '\n']));
    }
    Ok(read)
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
    pub(super) codex_compaction: Option<Arc<crate::codex::compact::Startup>>,
}

impl Probe {
    fn compaction_launch(&self, folder: Option<String>) -> crate::to_compact::Launch {
        crate::to_compact::Launch {
            provider: self.provider,
            account_path: self.account_path.clone(),
            folder: folder.unwrap_or_else(|| self.cwd.clone()),
            start: self.spawned_at_ms as u64,
            claude_window: None,
            codex: self.codex_compaction.clone(),
        }
    }
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
        codex_compaction: s.codex_compaction.clone(),
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
pub(super) fn session_meta(id: &str, account_path: String) -> Option<SessionMeta> {
    let probe = {
        let sessions = manager().sessions.lock().unwrap_or_else(|e| e.into_inner());
        let s = sessions.get(id).filter(|s| s.alive.load(Ordering::SeqCst))?;
        let mut p = probe_of(id, s);
        // The caller's idea of the account wins, as it always has: the chat
        // row is the source of truth for which folder a chat belongs to.
        p.account_path = account_path;
        p
    };

    window_meta(&probe)
}

fn window_meta(probe: &Probe) -> Option<SessionMeta> {
    let id = &probe.id;
    let life = crate::activity::life(id);
    let mut m = with_life(meta_from_disk(probe), life.as_ref())?;
    m.draft = manager().has_draft(id);
    m.held = crate::agents::held_count(id);
    m.limit = crate::activity::limit_of(id);
    if let Some((tokens, window)) = m.context_tokens.zip(m.context_window) {
        let launch = probe.compaction_launch(m.cwd.clone());
        if let Some((context, left)) = crate::background::Context::measure(id, Some(&launch), tokens, window, m.compaction_sample.as_ref())
        {
            m.context = Some(context.fraction());
            m.context_to_compact = left;
        }
    }
    pin_name(&mut m);
    Some(m)
}

/// Read a saved helper's conversation as a pane would after opening it as a chat.
#[cfg(test)]
pub(crate) fn resumed_claude_look_for_test(id: &str, account: &str, pid: u32) -> ChatLook {
    let probe = Probe {
        id: id.into(),
        provider: Provider::Claude,
        pid: Some(pid),
        cwd: account.into(),
        spawned_at_ms: 1,
        resume: None,
        account_path: account.into(),
        codex_compaction: None,
    };
    ChatLook { id: probe.id.clone(), meta: window_meta(&probe) }
}

impl PtyManager {
    /// A chat's launch inputs for an on-demand context reading.
    pub(crate) fn compaction_launch(&self, id: &str, folder: Option<String>) -> Option<crate::to_compact::Launch> {
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        let s = sessions.get(id)?;
        Some(probe_of(id, s).compaction_launch(folder))
    }
}

/// A live session's meta: what its CLI left on disk, with its record's status
/// where there is a record. A session its CLI has left nothing on disk for
/// yet (no transcript, no hooks heard) is still the record's: its status
/// alone, which a record not known yet says as `busy`. None only where
/// neither has anything.
///
/// The status is `Life::status` as is (`busy`, `waiting`, `background` for a
/// turn over with work of its own beside it, `idle`), read as the power board
/// reads the same record, so a row says what the status bar and the power
/// popover count it as (ADR-0031). Until there is a record the disk's own
/// word stands.
fn with_life(disk: Option<SessionMeta>, life: Option<&crate::activity::Life>) -> Option<SessionMeta> {
    let status = life.map(crate::activity::Life::status);
    let mut m = match disk {
        Some(m) => m,
        None if status.is_some() => SessionMeta::default(),
        None => return None,
    };
    if let Some(status) = status {
        m.status = Some(status.to_string());
    }
    Some(m)
}

/// A conversation 0.19's handoff or a move opened begins on Luna's prompt
/// (`[Luna handoff N, not typed by the user]`, agents `retired_opening`),
/// which is no name for the chat, as a first prompt or as the title Codex
/// gave its thread from it: the chat keeps the one it had. Luna types no such
/// prompt since 0.20 (ADR-0030); a chat resumed on such a conversation still
/// has one.
fn pin_name(m: &mut SessionMeta) {
    if m.first_prompt.as_deref().is_some_and(crate::agents::retired_opening) {
        m.first_prompt = None;
    }
    if !m.title_renamed && m.title.as_deref().is_some_and(crate::agents::retired_opening) {
        m.title = None;
    }
}

#[cfg(test)]
mod status_tests {
    use super::*;
    use crate::activity::{Background, Life, Phase};

    /// The row says what the record says, as the power board reads it, and
    /// the disk's word only where there is no record; a session with nothing
    /// on disk answers the record's word alone, and with neither, nothing.
    #[test]
    fn a_rows_status_is_its_records() {
        let life = |phase, subagents| Life { phase, background: Background { subagents, ..Default::default() }, ..Default::default() };
        let status = |l: Option<&Life>| with_life(None, l).map(|m| m.status);
        assert_eq!(status(None), None, "no record and nothing on disk");
        assert_eq!(status(Some(&life(Phase::Running, 0))), Some(Some("busy".into())));
        assert_eq!(status(Some(&life(Phase::Unknown, 0))), Some(Some("busy".into())), "not known is not over");
        assert_eq!(status(Some(&life(Phase::Asking, 1))), Some(Some("waiting".into())));
        assert_eq!(status(Some(&life(Phase::Ended, 1))), Some(Some("background".into())));
        assert_eq!(status(Some(&life(Phase::Ended, 0))), Some(Some("idle".into())));
        let disk = || Some(SessionMeta { status: Some("resting".into()), title: Some("t".into()), ..Default::default() });
        assert_eq!(with_life(disk(), None).and_then(|m| m.status).as_deref(), Some("resting"), "the disk's word stands");
        let m = with_life(disk(), Some(&Life::default())).expect("a meta");
        assert_eq!((m.status.as_deref(), m.title.as_deref()), (Some("busy"), Some("t")), "the record's word over the disk's");
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
        assert_eq!(named("[Luna movement notes] for the release").as_deref(), Some("[Luna movement notes] for the release"));
        assert_eq!(named("[Luna handoff notes] the plan").as_deref(), Some("[Luna handoff notes] the plan"));
        // The title Codex gave the thread from that opening, too, unless the person renamed it so.
        let titled = |title: &str, renamed: bool| {
            let mut m = SessionMeta { title: Some(title.into()), title_renamed: renamed, ..Default::default() };
            pin_name(&mut m);
            m.title
        };
        let opening = "[Luna handoff 2, not typed by the user] This is a new conversation";
        assert_eq!(titled(opening, false), None);
        assert_eq!(titled(opening, true).as_deref(), Some(opening), "a rename stands");
        assert_eq!(titled("Fix the updater", false).as_deref(), Some("Fix the updater"));
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
    /// Luna's: its account path is refused as the window's arguments are read,
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
        assert_eq!(saved_title(Provider::Claude, none, sid), None, "and no path is no title");
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// The CLI's title for a session that is not running, by its id — how a chat
/// restored from an earlier run picks up the title its CLI gave it. The
/// account path is held to an account folder of Luna's as it is read
/// (`boundary::AccountPath`), before anything under it is; a chat with none
/// has no title saved anywhere. A title that is only the tag 0.19 typed a
/// handoff's or a move's prompt under is none. Reads files: off the
/// window's thread.
pub fn saved_title(
    provider: Provider,
    account_path: crate::boundary::OrEmpty<crate::boundary::AccountPath>,
    session_id: &str,
) -> Option<CliTitle> {
    let account_path = account_path.into_string();
    if account_path.is_empty() {
        return None;
    }
    match provider {
        Provider::Claude => crate::claude::session::saved_title(&account_path, session_id),
        Provider::Codex => crate::codex::session::thread_title(&account_path, session_id),
    }
    .filter(|t| t.renamed || !crate::agents::retired_opening(&t.title))
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
