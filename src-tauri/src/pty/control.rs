//! What is done to a running session: a person's keys and Luna's text into
//! it, resizes, quitting it the way a person would before it is killed,
//! deleting a chat, and the sessions no chat claims any more.

use super::probe::{meta_from_disk, probe_of, Probe};
use super::screen::draft_after;
use super::start::claude_instructions_dir;
use super::{manager, now_ms, Input, PtyManager, Session};
use crate::provider::Provider;
use portable_pty::PtySize;
use serde::Serialize;
#[cfg(windows)]
use std::process::Command;
use std::sync::atomic::Ordering;

#[tauri::command]
pub fn write_session(id: String, data: String) -> Result<(), String> {
    let state = manager();
    let sessions = state.sessions.lock().unwrap();
    let s = sessions.get(&id).ok_or("no such session")?;
    if let Some(held) = s.held_keys.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        held.push(data);
        return Ok(());
    }
    take_keys(s, data)
}

/// A person's keys into the session, noted for `typed_ago_ms` and
/// `has_draft`.
fn take_keys(s: &Session, data: String) -> Result<(), String> {
    // Keys, not what the terminal reports by itself (focus, mouse), which
    // arrives as escape sequences on the same path.
    if !data.starts_with('\x1b') {
        s.typed_ms.store(now_ms(), Ordering::Relaxed);
    }
    let draft = draft_after(s.draft.load(Ordering::Relaxed), &data);
    s.draft.store(draft, Ordering::Relaxed);
    if s.provider == Provider::Claude && data.contains('\x13') {
        s.stashing.store(true, Ordering::Relaxed);
    }
    // Queueing, so the command returns at once however busy the pty is.
    s.input_tx.send(Input::Bytes(data.into_bytes())).map_err(|_| "session writer is gone".to_string())
}

/// A person's keys held back from a session (`PtyManager::hold_keys`); they
/// go on when this is dropped.
pub struct KeyHold {
    id: String,
}

impl Drop for KeyHold {
    fn drop(&mut self) {
        // Under the session map, which every key takes on its way in: none
        // can overtake the ones held.
        let sessions = manager().sessions.lock().unwrap_or_else(|e| e.into_inner());
        let Some(s) = sessions.get(&self.id) else { return };
        let held = s.held_keys.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap_or_default();
        for data in held {
            let _ = take_keys(s, data);
        }
    }
}

/// Queued behind the keystrokes that preceded it and applied on the writer
/// thread. A failure there is logged rather than returned: by the time it
/// happens the caller has moved on, and it never looked at the result anyway.
#[tauri::command]
pub fn resize_session(id: String, cols: u16, rows: u16) -> Result<(), String> {
    let state = manager();
    let sessions = state.sessions.lock().unwrap();
    let s = sessions.get(&id).ok_or("no such session")?;
    s.input_tx
        .send(Input::Resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }))
        .map_err(|_| "session writer is gone".to_string())
}

// The pty child is the CLI; everything it spawned (node, MCP servers, shells)
// is a grandchild and survives a plain kill. Sweep the tree first, then kill
// the child directly as a backstop.
pub(crate) fn kill_tree(pid: Option<u32>) {
    let Some(pid) = pid else { return };
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .output();
    }
    // No one call does it here: the CLIs start each tool command in a session of
    // its own, out of the CLI's group and with children of its own.
    #[cfg(unix)]
    crate::keeper::end_tree(pid);
    #[cfg(not(any(windows, unix)))]
    let _ = pid;
}

/// How long a session gets to leave on its own before it is taken down.
/// Neither CLI is out within two seconds of its second Ctrl+C: Claude Code
/// runs its exit hooks first — among them Luna's own SessionEnd hook, a curl
/// with a 3 s timeout — and Codex shuts its MCP servers down. At 1.5 s every
/// deleted chat ended in the hard kill this is meant to avoid. The same for
/// every session at once: an update, Quit and shut-down-when-done.
pub const GRACE: std::time::Duration = std::time::Duration::from_secs(10);
/// Longer than any CLI waits to see whether an Esc starts an escape sequence.
const ESC_GAP: std::time::Duration = std::time::Duration::from_millis(300);
/// Ctrl+C is one keystroke to the CLI and two in a row mean quit; one write
/// carrying both lands in a single input tick, where the second cannot see
/// the state the first set. Space them the way a hand would.
const CTRL_C_GAP: std::time::Duration = std::time::Duration::from_millis(150);
/// How long the CLI gets after the Ctrl+Cs to be on its way out before an
/// exit dialog is assumed to be holding it, and between the Enters that
/// answer one.
const EXIT_DIALOG_WAIT: std::time::Duration = std::time::Duration::from_millis(1500);

/// Asks the CLI to quit, and kills it only if it does not.
///
/// `taskkill /F` ends the process without its exit hooks, and the CLI keeps
/// state that only those hooks clean up. A fullscreen launch records itself as
/// pending until it has been healthy for ten seconds; a launch that dies with
/// the record still there costs the next launch on that account its fullscreen
/// renderer ("didn't finish starting last time"), and two of them turn the
/// renderer off. Deleting a chat you had just made, or closing the login
/// window, did exactly that. So first the keystrokes a user would type —
/// Ctrl+C, and again when it asks — and the hard kill only for a process that
/// is past listening. Codex reads Ctrl+C the same way: interrupt, then quit.
///
/// Blocks for up to `grace`: callers run it off the main thread.
fn shut_down(mut s: Session, grace: std::time::Duration) {
    let exited = || s.child.lock().unwrap_or_else(|e| e.into_inner()).try_wait().map(|status| status.is_some()).unwrap_or(true);
    let deadline = std::time::Instant::now() + grace;
    // A modal question — Claude Code's "wait for the limit to reset?", a
    // picker — ignores Ctrl+C and answers Esc, and sat out the whole grace
    // into the hard kill. Esc first, then a pause long enough that the CLI
    // does not read it and the Ctrl+C after as one Alt+Ctrl+C.
    if s.input_tx.send(Input::Bytes(vec![0x1b])).is_ok() {
        std::thread::sleep(ESC_GAP);
    }
    // A turn in flight takes one Ctrl+C to interrupt before the next two ask
    // for and confirm the exit.
    for _ in 0..3 {
        if exited() || s.input_tx.send(Input::Bytes(b"\x03".to_vec())).is_err() {
            break;
        }
        std::thread::sleep(CTRL_C_GAP);
    }
    let wait = |until: std::time::Instant| {
        while !exited() && std::time::Instant::now() < until.min(deadline) {
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    };
    // Claude Code in a worktree answers the quit with "Exiting worktree
    // session — keep or remove?", which Ctrl+C does not dismiss: every deleted
    // worktree chat sat out the whole grace and then took the hard kill. Enter
    // takes the focused default, Keep — the right answer either way, since
    // Luna removes the worktree itself afterwards when the delete asks for
    // that. The Ctrl+Cs have already cleared the prompt, so an Enter that
    // finds no dialog submits nothing. Again until the CLI is gone: the dialog
    // comes up only once Claude Code has looked the worktree over, its
    // submodules with up to 5 s of their own, and in a large repository one
    // Enter could come before it and leave it to the hard kill.
    loop {
        wait(std::time::Instant::now() + EXIT_DIALOG_WAIT);
        if exited() || std::time::Instant::now() >= deadline {
            break;
        }
        let _ = s.input_tx.send(Input::Bytes(b"\r".to_vec()));
    }
    if !exited() {
        crate::log::warn("pty", &format!("pid {:?} did not quit on Ctrl+C within {grace:?}; killing it", s.pid));
    }
    // Either way: the CLI's own children (shells, MCP servers) do not follow it
    // out, and a taskkill on a pid that has just exited is a no-op.
    kill_tree(s.pid);
    let _ = s.killer.kill();
    end_leftovers(&s);
}

/// What the tree kill cannot reach goes too: a process whose parent left
/// first is in the session's job but under nobody.
pub(super) fn end_leftovers(s: &Session) {
    if let Some(job) = &s.job {
        let ended = crate::procs::end_leftovers(job, s.pid);
        if !ended.is_empty() {
            crate::log::info("pty", &format!("pid {:?}: ended what it left running: {}", s.pid, ended.join(", ")));
        }
    }
}

/// Takes the session out of the map first and shuts it down after: the
/// shutdown waits on the process, and waiting under the map would put every
/// other chat's keystrokes behind it.
///
/// A spawn of the chat still in flight — a worktree being fetched, a helper
/// deleted the moment it was made — is waited out under its gate: removed
/// before that spawn put its process in the map, the session would have run
/// on with no chat to reach it from.
async fn take_and_shut_down(state: &PtyManager, id: &str) {
    let session = {
        let gate = state.gate(id);
        let _spawning = gate.lock().unwrap_or_else(|e| e.into_inner());
        state.sessions.lock().unwrap_or_else(|e| e.into_inner()).remove(id)
    };
    if let Some(s) = session {
        let _ = tauri::async_runtime::spawn_blocking(move || shut_down(s, GRACE)).await;
    }
}

/// Asks every session to quit the way a delete does, before an update: the
/// installer ends the process, and a CLI given its exit hooks leaves a whole
/// transcript where one killed mid-write would not. A background helper's
/// turn is stopped too: one still running would keep the old core from being
/// replaced, and its parent chat has just been stopped. Returns how many it
/// stopped.
#[tauri::command]
pub async fn stop_all_sessions() -> Result<usize, String> {
    tauri::async_runtime::spawn_blocking(|| {
        // What a chat was doing is written down before quitting changes it.
        crate::background::save();
        crate::agents::freeze_chats();
        manager().shut_down_all(GRACE) + crate::background::stop_turns(false)
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn kill_session(id: String) -> Result<(), String> {
    take_and_shut_down(manager(), &id).await;
    Ok(())
}

/// Tears a chat down in one shot: resolve where the session actually lives,
/// kill it, drop its worktree (and the branch made for it) and its
/// attachments. Doing this in one command removes the race the UI had — it
/// used to rely on a 4s poll having already reported the worktree path.
#[tauri::command]
// Async for the same reason as the worktree commands: removing a worktree can
// take seconds, and on the main thread every keystroke in every other chat
// waits behind it.
pub async fn delete_session(
    id: String,
    folder: String,
    worktree_path: Option<String>,
    // Off by default at the call site: the worktree may hold work that exists
    // nowhere else, and a deleted chat is a cheaper mistake than a deleted
    // branch. Returns the path either way, so the caller can say where the
    // kept worktree is.
    drop_worktree: bool,
) -> Result<Option<String>, String> {
    manager().delete(&id, &folder, worktree_path, drop_worktree).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrphanSession {
    /// The chat id the session was spawned under. Re-creating a chat row with
    /// this exact id is what reattaches it, scrollback included.
    pub id: String,
    pub provider: Provider,
    pub pid: Option<u32>,
    pub cwd: String,
    pub account_path: String,
    pub title: Option<String>,
    pub status: Option<String>,
}

/// Sessions this app is still running that no chat claims any more.
///
/// A chat row can go missing while its session keeps working — dropped by a
/// half-written state restore, say — and there is then no
/// way to reach the session from the UI at all: it holds an account's tokens
/// and answers to nobody. The frontend passes the chat ids it knows, and
/// anything alive outside that set comes back here.
///
/// Only this app's own ptys are considered. Claude Code registers its
/// subagents and shells in the same account registry, so walking that instead
/// would report every busy chat's children as orphans.
#[tauri::command]
pub async fn orphan_sessions(known: Vec<String>) -> Result<Vec<OrphanSession>, String> {
    let state = manager();
    let candidates: Vec<Probe> = {
        let sessions = state.sessions.lock().unwrap();
        sessions.iter().filter(|(id, s)| !known.contains(id) && s.alive.load(Ordering::SeqCst)).map(|(id, s)| probe_of(id, s)).collect()
    };
    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    // Naming one means reading the head of its transcript, so do it off the
    // main thread like session_meta does.
    tauri::async_runtime::spawn_blocking(move || {
        candidates
            .into_iter()
            .map(|p| {
                let meta = meta_from_disk(&p);
                OrphanSession {
                    id: p.id,
                    provider: p.provider,
                    pid: p.pid,
                    cwd: p.cwd,
                    account_path: p.account_path,
                    title: meta.as_ref().and_then(|m| m.title.clone().or_else(|| m.first_prompt.clone()).or_else(|| m.name.clone())),
                    status: meta.and_then(|m| m.status),
                }
            })
            .collect()
    })
    .await
    .map_err(|e| e.to_string())
}

impl PtyManager {
    /// When a person last typed into the session, 0 if never: to tell whether
    /// any key has come since.
    pub fn typed_at_ms(&self, id: &str) -> u64 {
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions.get(id).map_or(0, |s| s.typed_ms.load(Ordering::Relaxed))
    }

    /// How long ago a person last typed into the session, None if never.
    pub fn typed_ago_ms(&self, id: &str) -> Option<u64> {
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        let at = sessions.get(id)?.typed_ms.load(Ordering::Relaxed);
        (at > 0).then(|| now_ms().saturating_sub(at))
    }

    /// Keeps a person's keys back from the session until the hold is
    /// dropped, then sends them on in the order they came: Luna is typing
    /// around their draft, and a key landing in between would go out with
    /// Luna's text or into the stash. None if the session is not there.
    pub fn hold_keys(&self, id: &str) -> Option<KeyHold> {
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        let s = sessions.get(id)?;
        s.held_keys.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Vec::new);
        Some(KeyHold { id: id.to_string() })
    }

    /// Queues bytes for the session's writer thread.
    pub fn write(&self, id: &str, data: &[u8]) -> Result<(), String> {
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        let s = sessions.get(id).ok_or("no such session")?;
        s.input_tx.send(Input::Bytes(data.to_vec())).map_err(|_| "session writer is gone".to_string())
    }

    /// Types text the way a terminal pastes it: between bracketed-paste
    /// markers once the CLI has asked for them. Thrown in bare, a few
    /// thousand characters reached Claude Code as only their last couple of
    /// hundred — a notice lost its head, the tag that says what it is with
    /// it. Codex on Windows takes its text as key events (see `as_key_events`),
    /// which carry every character already and would take the markers for
    /// typing.
    pub fn paste(&self, id: &str, text: &str) -> Result<(), String> {
        let bracketed = {
            let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
            let s = sessions.get(id).ok_or("no such session")?;
            let keys = cfg!(windows) && s.provider == Provider::Codex;
            !keys && s.modes.lock().unwrap_or_else(|e| e.into_inner()).on.get(&2004) == Some(&true)
        };
        if bracketed {
            self.write(id, format!("\x1b[200~{text}\x1b[201~").as_bytes())
        } else {
            self.write(id, text.as_bytes())
        }
    }

    /// Whether a session other than `id` is in this worktree: a running one
    /// started in it or moved into it itself, or a background helper sent to
    /// it or working in it, between its turns too.
    pub fn worktree_shared(&self, id: &str, wt: &str) -> bool {
        let norm = |p: &str| p.replace('/', "\\").trim_end_matches('\\').to_lowercase();
        let wt = norm(wt);
        let inside = |p: &str| {
            let p = norm(p);
            p == wt || p.starts_with(&format!("{wt}\\"))
        };
        // Probed off the session map: the disk reads below must not hold it.
        self.activity_probes().iter().filter(|p| p.id != id).any(|p| inside(&p.cwd) || p.working_dir().is_some_and(|real| inside(&real)))
            || crate::background::working_dirs(Some(id)).iter().any(|d| inside(d))
    }

    /// Tears a chat's session down: resolve its worktree, quit the process,
    /// drop attachments and the hooks file, and the worktree only if asked.
    /// Returns the worktree path either way.
    pub async fn delete(
        &self,
        id: &str,
        folder: &str,
        worktree_path: Option<String>,
        drop_worktree: bool,
    ) -> Result<Option<String>, String> {
        // Resolve before killing: once the process is gone its registry entry goes too.
        let resolved = worktree_path.filter(|p| !p.is_empty()).or_else(|| {
            let probe = self.probe(id)?;
            let scwd = probe.working_dir().unwrap_or(probe.cwd);
            crate::worktree::is_worktree_of(folder, &scwd).then_some(scwd)
        });
        // A helper spawned from a chat in a worktree runs in that same
        // worktree. Dropping "its" worktree would take the other chat's work
        // with it, so a worktree another session or a helper is in stays.
        let shared = resolved.as_deref().is_some_and(|wt| self.worktree_shared(id, wt));

        take_and_shut_down(self, id).await;
        // Everything else that kept count of it, so nothing keeps the chat on
        // after it is gone: the agent registry (luna_sessions, the helper
        // limit, its token), the turn counters, a waiting cleared on it, the
        // Codex rollout it held.
        crate::agents::forget(id);
        crate::activity::forget(id);
        crate::asking::forget(id);
        crate::codex::session::release(id);

        let _ = crate::media::clear_media(id.to_string());
        for sub in ["hooks", "mcp"] {
            let _ = std::fs::remove_file(crate::paths::data_dir().join(sub).join(format!("{id}.json")));
        }
        claude_instructions_dir(id, None);
        crate::log::info("delete", &format!("chat {}, worktree {resolved:?}, dropping {drop_worktree}", crate::agents::label(id)));

        match &resolved {
            Some(wt) if drop_worktree && shared => {
                crate::log::warn("delete", &format!("kept worktree {wt}: another session is in it"));
                Ok(None)
            }
            Some(wt) if drop_worktree => crate::worktree::remove_worktree_now(folder.to_string(), wt.clone()).map(|_| resolved.clone()),
            Some(_) => Ok(resolved),
            None => Ok(None),
        }
    }

    /// Asks every live session to quit, each with `grace` to do it on its
    /// own, and returns how many there were. Blocks; for the shutdown path.
    pub fn shut_down_all(&self, grace: std::time::Duration) -> usize {
        let taken: Vec<Session> = {
            let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
            let ids: Vec<String> = sessions.iter().filter(|(_, s)| s.alive.load(Ordering::SeqCst)).map(|(id, _)| id.clone()).collect();
            ids.into_iter().filter_map(|id| sessions.remove(&id)).collect()
        };
        let n = taken.len();
        let handles: Vec<_> = taken.into_iter().map(|s| std::thread::spawn(move || shut_down(s, grace))).collect();
        for h in handles {
            let _ = h.join();
        }
        n
    }
}
