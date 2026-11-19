//! What is done to a running session: a person's keys and Luna's text into
//! it, resizes, quitting it the way a person would before it is killed,
//! deleting a chat, and the sessions no chat claims any more.

use super::probe::{meta_from_disk, probe_of, Probe};
use super::screen::{composer_keys, composer_seen, draft_after, paste_lapsed, typed_by_person};

/// How much of the screen's output is replayed to tell a dialog from the
/// composer as a key comes in: the last frames a TUI drew, not its history.
const KEYS_LOOK_BYTES: usize = 256 * 1024;
use super::start::claude_instructions_dir;
use super::{manager, now_ms, Input, PtyManager, Session, READER_WAIT};
use crate::provider::Provider;
use portable_pty::PtySize;
use serde::Serialize;
#[cfg(windows)]
use std::process::Command;
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// Input for a session. `reply`: the terminal's own answer to the program's
/// query (its size, a color, the device's attributes), which the native
/// window's terminal sends itself: it goes straight to the PTY, neither held
/// with a person's keys nor counted as their typing (`typed_ago_ms`,
/// `has_draft`), which an OSC color reply would otherwise count as text. The
/// page never sends it: xterm.js's replies take the keys' path as before.
#[tauri::command]
pub fn write_session(id: String, data: String, reply: Option<bool>) -> Result<(), String> {
    let state = manager();
    let sessions = state.sessions.lock().unwrap_or_else(|e| e.into_inner());
    let s = sessions.get(&id).ok_or("no such session")?;
    if reply == Some(true) {
        return s.input_tx.send(Input::Bytes(data.into_bytes()));
    }
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
    // A paste and Esc-led keys of a person's count too: before the CLI
    // repaints, its screen may still show the old empty placeholder.
    // A paste with no end marker by now is over (`paste_lapsed`): looked at
    // against the key before this one, then this one is stamped.
    let last = s.typed_ms.load(Ordering::Relaxed);
    let now = now_ms();
    for count in [&s.draft, &s.keys_draft] {
        count.store(paste_lapsed(count.load(Ordering::Relaxed), last, now), Ordering::Relaxed);
    }
    if typed_by_person(&data) {
        s.typed_ms.store(now, Ordering::Relaxed);
    }
    let draft = draft_after(s.draft.load(Ordering::Relaxed), &data);
    s.draft.store(draft, Ordering::Relaxed);
    // The move's veto counts composer text alone: a key that goes to a
    // dialog (a digit that picks an option) does not, looked at on the
    // screen's last stretch where a key would start a draft.
    let (keys, dialog) = composer_keys(s.keys_draft.load(Ordering::Relaxed), s.keys_dialog.load(Ordering::Relaxed), &data, || {
        let bytes = s.scrollback.lock().unwrap_or_else(|e| e.into_inner()).last(KEYS_LOOK_BYTES);
        let (rows, cols) = *s.size.lock().unwrap_or_else(|e| e.into_inner());
        !composer_seen(&bytes, rows, cols, s.provider)
    });
    s.keys_draft.store(keys, Ordering::Relaxed);
    s.keys_dialog.store(dialog, Ordering::Relaxed);
    if s.provider == Provider::Claude && data.contains('\x13') {
        s.stashing.store(true, Ordering::Relaxed);
    }
    // Queueing, so the command returns at once however busy the pty is.
    s.input_tx.send(Input::Bytes(data.into_bytes()))
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
        let Some(s) = sessions.get(&self.id) else {
            #[cfg(test)]
            if let Some(t) = fake_terminals().lock().unwrap_or_else(|e| e.into_inner()).get_mut(&self.id) {
                t.holds -= 1;
            }
            return;
        };
        let held = s.held_keys.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap_or_default();
        for data in held {
            let _ = take_keys(s, data);
        }
    }
}

/// A terminal a test stands in for, with no CLI behind it: the person's keys
/// held on it counted, and what Luna types kept rather than sent.
#[cfg(test)]
#[derive(Default)]
struct FakeTerminal {
    holds: u32,
    typed: Vec<u8>,
    /// When the person last typed into it (`typed_at_ms`), 0 if never.
    typed_ms: u64,
}

#[cfg(test)]
fn fake_terminals() -> &'static std::sync::Mutex<std::collections::HashMap<String, FakeTerminal>> {
    static F: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, FakeTerminal>>> = std::sync::OnceLock::new();
    F.get_or_init(Default::default)
}

/// `id` as a terminal of the test's (`FakeTerminal`), or no longer one.
#[cfg(test)]
pub(crate) fn fake_terminal_for_test(id: &str, on: bool) {
    let mut fakes = fake_terminals().lock().unwrap_or_else(|e| e.into_inner());
    if on {
        fakes.insert(id.to_string(), FakeTerminal::default());
    } else {
        fakes.remove(id);
    }
}

/// The person's keys into the test's terminal `id`, taken as `take_keys`
/// takes them: stamped, and on their way to a CLI that has not acted on them.
#[cfg(test)]
pub(crate) fn fake_person_keys(id: &str) {
    if let Some(t) = fake_terminals().lock().unwrap_or_else(|e| e.into_inner()).get_mut(id) {
        t.typed_ms = now_ms();
    }
}

/// What Luna has typed into the test's terminal `id`, and whether the
/// person's keys are held there now.
#[cfg(test)]
pub(crate) fn fake_terminal_of(id: &str) -> (String, bool) {
    let fakes = fake_terminals().lock().unwrap_or_else(|e| e.into_inner());
    fakes.get(id).map(|t| (String::from_utf8_lossy(&t.typed).into_owned(), t.holds > 0)).unwrap_or_default()
}

/// Queued behind the keystrokes that preceded it and applied on the writer
/// thread. A failure there is logged rather than returned: by the time it
/// happens the caller has moved on, and it never looked at the result anyway.
#[tauri::command]
pub fn resize_session(id: String, cols: u16, rows: u16, pixel_width: Option<u16>, pixel_height: Option<u16>) -> Result<(), String> {
    let state = manager();
    let sessions = state.sessions.lock().unwrap_or_else(|e| e.into_inner());
    let s = sessions.get(&id).ok_or("no such session")?;
    // The grid's size in pixels, as TIOCSWINSZ carries it on Unix (ConPTY
    // takes the cells only): the native window sends it, the page does not.
    let (pixel_width, pixel_height) = (pixel_width.unwrap_or(0), pixel_height.unwrap_or(0));
    s.input_tx.send(Input::Resize(PtySize { rows, cols, pixel_width, pixel_height }))
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
/// How many deleted chat ids a core keeps refusing to start (`delete`).
const DELETED_KEPT: usize = 4096;

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

impl PtyManager {
    /// Takes the session out of the map first and shuts it down after: the
    /// shutdown waits on the process, and waiting under the map would put
    /// every other chat's keystrokes behind it. Then waits for its reader to
    /// have said it exited (`ReaderEnd`). Called under the chat's spawn gate,
    /// held throughout: a spawn of the chat still in flight (a worktree being
    /// fetched, a helper deleted the moment it was made) is waited out, as a
    /// session removed before that spawn put its process in the map would run
    /// on with no chat to reach it from; and a start after it waits until the
    /// old session's exit is said, which said after the start would name the
    /// new session. Blocks for up to `GRACE` and `READER_WAIT`.
    fn take_and_quit(&self, id: &str) {
        let session = self.sessions.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
        let Some(s) = session else { return };
        // Its exit is Luna's doing, not its CLI version's.
        crate::cli::session_stopped(id);
        let (end, child, generation) = (Arc::clone(&s.reader_end), Arc::clone(&s.child), s.generation);
        shut_down(s, GRACE);
        if !end.wait(READER_WAIT) {
            let who = crate::agents::label(id);
            crate::log::warn("pty", &format!("{who}'s reader was not through {READER_WAIT:?} after its end; its exit is said for it"));
        }
        // One already saying it is through by the time this returns, and
        // none says it after: a delete's cleanup and a kill's start that
        // follows come after the exit. One not said yet is said here.
        let code = child.lock().unwrap_or_else(|e| e.into_inner()).try_wait().ok().flatten().map(|status| status.exit_code());
        self.retire(id, generation, code);
    }
}

/// Asks every session to quit the way a delete does, before an update: the
/// installer ends the process, and a CLI given its exit hooks leaves a whole
/// transcript where one killed mid-write would not. A background helper's
/// turn is stopped too: one still running would keep the old core from being
/// replaced, and its parent chat has just been stopped. Returns how many it
/// stopped. The chats' ties stay frozen until the update is called off
/// (`update_called_off`), the installer ends the process, or the freeze's
/// bound after the stop is up (`agents::freeze_for_update`).
#[tauri::command]
pub async fn stop_all_sessions() -> Result<usize, String> {
    tauri::async_runtime::spawn_blocking(|| {
        // What a chat was doing is written down before quitting changes it.
        crate::background::save();
        stopped_for_update(|| manager().shut_down_all(GRACE) + crate::background::stop_turns(false))
    })
    .await
    .map_err(|e| e.to_string())
}

/// `stop` run with the chats' ties frozen for an update, the freeze not to
/// lift until it has returned (`agents::freeze_for_update`).
fn stopped_for_update(stop: impl FnOnce() -> usize) -> usize {
    let _stopping = crate::agents::freeze_for_update();
    stop()
}

/// The update the sessions were stopped for is not being installed (its
/// install failed): the core goes on, and the chats' ties are saved again,
/// at once or once the sessions are no longer being stopped for it
/// (`agents::call_off_update`).
pub fn update_called_off() {
    crate::agents::call_off_update();
}

/// Ends a chat's session, under its spawn gate (`take_and_quit`), on a
/// thread for blocking work.
#[tauri::command]
pub async fn kill_session(id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = manager();
        let gate = state.gate(&id);
        let _quitting = gate.lock().unwrap_or_else(|e| e.into_inner());
        state.take_and_quit(&id);
    })
    .await
    .map_err(|e| e.to_string())
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
) -> Result<Deleted, String> {
    // On a thread for blocking work: the delete holds the chat's gate while
    // the session quits, which takes up to `GRACE`.
    tauri::async_runtime::spawn_blocking(move || manager().delete(&id, &folder, worktree_path, drop_worktree))
        .await
        .map_err(|e| e.to_string())?
}

/// What a chat's delete did with its worktree.
#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Deleted {
    /// The chat's worktree, as the delete found it, gone or kept.
    pub worktree: Option<String>,
    /// Asked to go with the chat, it stayed: another session is in it or on
    /// its way in. The window says so; the worktree is the sweep's to offer
    /// once nobody is in it.
    pub worktree_kept: bool,
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
        let sessions = state.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions.iter().filter(|(id, s)| !known.contains(id) && s.alive.load(Ordering::SeqCst)).map(|(id, s)| probe_of(id, s)).collect()
    };
    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    // Naming one means reading the head of its transcript, so do it on a
    // thread for blocking work.
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
        #[cfg(test)]
        if let Some(t) = fake_terminals().lock().unwrap_or_else(|e| e.into_inner()).get(id).filter(|_| !sessions.contains_key(id)) {
            return t.typed_ms;
        }
        sessions.get(id).map_or(0, |s| s.typed_ms.load(Ordering::Relaxed))
    }

    /// How long ago a person last typed into the session, None if never.
    pub fn typed_ago_ms(&self, id: &str) -> Option<u64> {
        let at = self.typed_at_ms(id);
        (at > 0).then(|| now_ms().saturating_sub(at))
    }

    /// Keeps a person's keys back from the session until the hold is
    /// dropped, then sends them on in the order they came: Luna is typing
    /// around their draft, and a key landing in between would go out with
    /// Luna's text or into the stash. None if the session is not there.
    pub fn hold_keys(&self, id: &str) -> Option<KeyHold> {
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        #[cfg(test)]
        if !sessions.contains_key(id) {
            let mut fakes = fake_terminals().lock().unwrap_or_else(|e| e.into_inner());
            let t = fakes.get_mut(id)?;
            t.holds += 1;
            return Some(KeyHold { id: id.to_string() });
        }
        let s = sessions.get(id)?;
        s.held_keys.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Vec::new);
        Some(KeyHold { id: id.to_string() })
    }

    /// Queues bytes for the session's writer thread.
    pub fn write(&self, id: &str, data: &[u8]) -> Result<(), String> {
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        #[cfg(test)]
        if let Some(t) = fake_terminals().lock().unwrap_or_else(|e| e.into_inner()).get_mut(id).filter(|_| !sessions.contains_key(id)) {
            t.typed.extend_from_slice(data);
            return Ok(());
        }
        let s = sessions.get(id).ok_or("no such session")?;
        s.input_tx.send(Input::Bytes(data.to_vec()))
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
            #[cfg(test)]
            if !sessions.contains_key(id) && fake_terminals().lock().unwrap_or_else(|e| e.into_inner()).contains_key(id) {
                drop(sessions);
                return self.write(id, text.as_bytes());
            }
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

    /// Whether a session other than `id` is in this worktree, for a delete
    /// of its own (`worktree::Look::Delete`): a running one started in it or
    /// into it (`Session::enters`: Claude Code's registry says so only once
    /// the CLI is up) or moved into it itself, or a background helper sent to
    /// it or working in it, between its turns too. By where each path leads
    /// as well as by its spelling (`worktree::Hold`): a session started
    /// through a link to the project is in the worktree a delete names by its
    /// real path, the link gone since or not.
    pub fn worktree_shared(&self, id: &str, wt: &str) -> bool {
        self.holds(Some(id)).keeps(&crate::worktree::Target::of(wt, None), crate::worktree::Look::Delete)
    }

    /// Tears a chat's session down: resolve its worktree, quit the process,
    /// drop attachments and the hooks file, and the worktree only if asked.
    /// Returns the worktree path either way, and whether one asked to go
    /// stayed (`Deleted`). Blocks while the session quits
    /// (`GRACE` at most) and the worktree goes: callers run it off the main
    /// thread.
    ///
    /// Under the chat's spawn gate from before the session is taken until
    /// the last of it is gone. The cleanup below goes by the chat's id alone:
    /// with the gate let go once the session was out of the map, a spawn of
    /// the same id while the old process quit (a pane still asking for it, a
    /// helper's id taken again) ran its start, and the cleanup then forgot
    /// the new session's registry entry and token, removed the hooks file it
    /// had just been given and let go of its rollout. Such a spawn now waits,
    /// and starts on what the delete left.
    ///
    /// A chat deleted is not started again, whatever asks (`spawn`): a pane
    /// that has not let go yet waits out the delete on the gate and is then
    /// refused, where it would have brought back a session the delete had just
    /// cleared away. The window never gives a chat id out twice.
    pub fn delete(&self, id: &str, folder: &str, worktree_path: Option<String>, drop_worktree: bool) -> Result<Deleted, String> {
        let gate = self.gate(id);
        let _deleting = gate.lock().unwrap_or_else(|e| e.into_inner());
        {
            let mut deleted = self.deleted.lock().unwrap_or_else(|e| e.into_inner());
            // Bounded: forgotten all at once past it, the oldest ids long gone.
            if deleted.len() >= DELETED_KEPT {
                deleted.clear();
            }
            deleted.insert(id.to_string());
        }
        // Resolve before killing: once the process is gone its registry entry goes too.
        let resolved = worktree_path.filter(|p| !p.is_empty()).or_else(|| {
            let probe = self.probe(id)?;
            let scwd = probe.working_dir().unwrap_or(probe.cwd);
            crate::worktree::is_worktree_of(folder, &scwd).then_some(scwd)
        });

        self.take_and_quit(id);
        // Everything else that kept count of it, so nothing keeps the chat on
        // after it is gone: the agent registry (luna_session_list, the helper
        // limit, its token), the turn counters, a waiting cleared on it, the
        // Codex rollout it held.
        crate::agents::forget(id);
        crate::activity::forget(id);
        crate::asking::forget(id);
        crate::codex::session::release(id);

        let _ = crate::media::clear_media(id.to_string());
        for sub in ["hooks", "mcp"] {
            let _ = std::fs::remove_file(crate::paths::data_dir().join(sub).join(format!("{}.json", crate::paths::id_file_name(id))));
        }
        let _ = claude_instructions_dir(id, None);
        crate::log::info("delete", &format!("chat {}, worktree {resolved:?}, dropping {drop_worktree}", crate::agents::label(id)));

        // A helper spawned from a chat in a worktree runs in that same
        // worktree. Dropping "its" worktree would take the other chat's work
        // with it, so a worktree another session or a helper is in stays.
        // Looked at only now, its own session gone, so one that went in while
        // this one quit counts too; and held from the look to the end of the
        // removal (`worktree::removing`), so a start that is on its way in,
        // not yet listed where the look goes, keeps it as well, and one that
        // comes during the removal is refused.
        let worktree_kept = match &resolved {
            Some(wt) if drop_worktree => {
                let target = crate::worktree::Target::of(wt, Some(crate::worktree::Place::of(folder)));
                match crate::worktree::removing(target, crate::worktree::Look::Delete, || self.worktree_shared(id, wt)) {
                    Some(_removing) => {
                        crate::worktree::remove_worktree_now(folder.to_string(), wt.clone())?;
                        false
                    }
                    None => {
                        crate::log::warn("delete", &format!("kept worktree {wt}: another session is in it or on its way in"));
                        true
                    }
                }
            }
            _ => false,
        };
        Ok(Deleted { worktree: resolved, worktree_kept })
    }

    /// Asks every live session to quit, each with `grace` to do it on its
    /// own, and returns how many there were. Blocks; for the shutdown path.
    pub fn shut_down_all(&self, grace: std::time::Duration) -> usize {
        let taken: Vec<(String, Session)> = {
            let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
            let ids: Vec<String> = sessions.iter().filter(|(_, s)| s.alive.load(Ordering::SeqCst)).map(|(id, _)| id.clone()).collect();
            ids.into_iter().filter_map(|id| sessions.remove(&id).map(|s| (id, s))).collect()
        };
        // Their exits are Luna's doing, not their CLI versions'.
        let taken: Vec<Session> = taken
            .into_iter()
            .map(|(id, s)| {
                crate::cli::session_stopped(&id);
                s
            })
            .collect();
        let n = taken.len();
        let handles: Vec<_> = taken.into_iter().map(|s| std::thread::spawn(move || shut_down(s, grace))).collect();
        for h in handles {
            let _ = h.join();
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::super::spawn::{capture, exit_word, reader_exited, said, spawn, Attached, Launch};
    use super::super::{InputTx, ReaderEnd};
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU64};
    use std::sync::Mutex;
    use std::time::Duration;

    /// A CLI a test stands in for, gone once `quit` is set: by the test, or
    /// by the kill that ends a shutdown.
    #[derive(Debug)]
    struct FakeChild {
        quit: Arc<AtomicBool>,
    }

    impl portable_pty::ChildKiller for FakeChild {
        fn kill(&mut self) -> std::io::Result<()> {
            self.quit.store(true, Ordering::SeqCst);
            Ok(())
        }
        fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
            Box::new(FakeChild { quit: Arc::clone(&self.quit) })
        }
    }

    impl portable_pty::Child for FakeChild {
        fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
            Ok(self.quit.load(Ordering::SeqCst).then(|| portable_pty::ExitStatus::with_exit_code(0)))
        }
        fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
            while !self.quit.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(portable_pty::ExitStatus::with_exit_code(0))
        }
        fn process_id(&self) -> Option<u32> {
            None
        }
        #[cfg(windows)]
        fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> {
            None
        }
    }

    /// How a test's session takes input and goes.
    #[derive(Clone, Copy)]
    struct Fake {
        /// Its writer takes what it is sent; else nothing is ever written.
        drains: bool,
        /// How long its reader takes, once the process is gone, to say so.
        reader_lag: Duration,
    }

    const PROMPT: Fake = Fake { drains: true, reader_lag: Duration::ZERO };

    /// A session of the test's in the map as `id`, running in `cwd` until
    /// the flag returned is set, with a reader thread that then says it
    /// exited as the real one does (`reader_exited`), `reader_lag` later.
    fn fake_session(id: &str, cwd: &str, how: Fake) -> Arc<AtomicBool> {
        fake_session_ending(
            id,
            cwd,
            how.drains,
            Box::new(move |id, generation, end| {
                std::thread::sleep(how.reader_lag);
                reader_exited(id, generation, Some(0), end);
            }),
        )
    }

    /// A reader the test lets through a step at a time: it says it is at
    /// its end (`at_end`), waits for `go` and says when it is through.
    struct HeldReader {
        at_end: std::sync::mpsc::Receiver<()>,
        go: std::sync::mpsc::Sender<()>,
        through: std::sync::mpsc::Receiver<()>,
    }

    /// A session of the test's whose reader waits for the test at its end:
    /// before it begins to say the exit, or with `saying`, once it has
    /// begun (`ReaderEnd::finish`) and before the word itself.
    fn held_session(id: &str, cwd: &str, saying: bool) -> (Arc<AtomicBool>, HeldReader) {
        use std::sync::mpsc::channel;
        let ((at_end_tx, at_end), (go, go_rx), (through_tx, through)) = (channel(), channel(), channel());
        let hold = move || {
            let _ = at_end_tx.send(());
            let _ = go_rx.recv();
        };
        let quit = fake_session_ending(
            id,
            cwd,
            true,
            Box::new(move |id, generation, end| {
                if saying {
                    end.finish(|| {
                        hold();
                        exit_word(id, generation, Some(0));
                    });
                } else {
                    hold();
                    reader_exited(id, generation, Some(0), end);
                }
                let _ = through_tx.send(());
            }),
        );
        (quit, HeldReader { at_end, go, through })
    }

    /// How a test's reader says its session exited, once its process is gone.
    type Ending = Box<dyn FnOnce(&str, u64, &ReaderEnd) + Send>;

    fn fake_session_ending(id: &str, cwd: &str, drains: bool, ending: Ending) -> Arc<AtomicBool> {
        let quit = Arc::new(AtomicBool::new(false));
        let child: Box<dyn portable_pty::Child + Send + Sync> = Box::new(FakeChild { quit: Arc::clone(&quit) });
        let (input_tx, input_rx, written) = InputTx::new(id);
        std::thread::spawn(move || {
            let mut never_written = Vec::new();
            for input in input_rx {
                if drains {
                    written.release(input.len());
                } else {
                    never_written.push(input);
                }
            }
        });
        let alive = Arc::new(AtomicBool::new(true));
        let reader_end = Arc::new(ReaderEnd::default());
        let generation = manager().next_generation(id);
        {
            let (id, quit, alive, end) = (id.to_string(), Arc::clone(&quit), Arc::clone(&alive), Arc::clone(&reader_end));
            std::thread::spawn(move || {
                while !quit.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                alive.store(false, Ordering::SeqCst);
                ending(&id, generation, &end);
            });
        }
        let session = Session {
            provider: Provider::Claude,
            generation,
            reader_end,
            input_tx: Arc::new(input_tx),
            killer: child.clone_killer(),
            child: Arc::new(Mutex::new(child)),
            scrollback: Arc::new(Mutex::new(super::super::screen::Scrollback::with_capacity(0))),
            drawn: Arc::new(Mutex::new(super::super::screen::Drawn::default())),
            modes: Arc::new(Mutex::new(super::super::screen::Modes::default())),
            alive,
            last_output_ms: Arc::new(AtomicU64::new(now_ms())),
            work_output_ms: Arc::new(AtomicU64::new(now_ms())),
            pid: None,
            cwd: cwd.to_string(),
            enters: None,
            hold: Arc::new(crate::worktree::Hold::start(&[cwd], None)),
            spawned_at_ms: now_ms() as u128,
            resume: None,
            account_path: String::new(),
            job: None,
            size: Arc::new(Mutex::new((30, 100))),
            typed_ms: Arc::new(AtomicU64::new(0)),
            draft: Arc::new(AtomicU64::new(0)),
            keys_draft: Arc::new(AtomicU64::new(0)),
            keys_dialog: Arc::new(AtomicBool::new(false)),
            draft_seen: Arc::new(Mutex::new((0, false))),
            held_keys: Arc::new(Mutex::new(None)),
            stashing: Arc::new(AtomicBool::new(false)),
        };
        manager().sessions.lock().unwrap_or_else(|e| e.into_inner()).insert(id.to_string(), session);
        quit
    }

    /// `id`'s session given what a start would have held (`Session::hold`):
    /// the folders it went into and, given `making`, where its CLI makes the
    /// worktree it asked for.
    fn held_as(id: &str, into: &[&str], making: bool) {
        let mut sessions = manager().sessions.lock().unwrap_or_else(|e| e.into_inner());
        let s = sessions.get_mut(id).unwrap();
        let making =
            making.then(|| crate::worktree::Unplaced { provider: s.provider, folder: s.cwd.clone(), account_path: s.account_path.clone() });
        s.hold = Arc::new(crate::worktree::Hold::start(into, making.as_ref()));
    }

    /// What a delete answers where it kept the chat's worktree `wt`.
    fn kept(wt: &str) -> Result<Deleted, String> {
        Ok(Deleted { worktree: Some(wt.to_string()), worktree_kept: true })
    }

    /// Until the delete or kill has taken `id`'s session out of the map.
    fn taken(id: &str) {
        let until = std::time::Instant::now() + Duration::from_secs(10);
        while manager().sessions.lock().unwrap_or_else(|e| e.into_inner()).contains_key(id) {
            assert!(std::time::Instant::now() < until, "{id} was never taken");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("luna-delete-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A start of `id` in `cwd` on another thread, captured: what it would
    /// run is built, and nothing runs.
    fn start(id: &'static str, cwd: String) -> std::thread::JoinHandle<Result<String, String>> {
        capture::expect(id);
        std::thread::spawn(move || {
            tauri::async_runtime::block_on(spawn(id.into(), move || {
                Ok(Launch {
                    provider: Provider::Claude,
                    cwd,
                    account_path: "acc".into(),
                    args: Vec::new(),
                    envs: Vec::new(),
                    resume: None,
                    describe: String::new(),
                    applied: Vec::new(),
                    reserved: None,
                    enters: None,
                    makes_worktree: false,
                })
            }))
            .map(|a: Attached| a.backlog)
        })
    }

    /// A start of a chat while it is deleted waits for the delete, the old
    /// session's exit said by then, and is refused: the chat is gone, and a
    /// session started for it would run on with its registry entry, token and
    /// files cleared.
    #[test]
    fn a_start_while_the_chat_is_deleted_waits_and_is_refused() {
        let id = "dl-respawn";
        let dir = scratch("respawn");
        let cwd = dir.to_string_lossy().into_owned();
        let quit = fake_session(id, &cwd, Fake { reader_lag: Duration::from_millis(200), ..PROMPT });
        let folder = cwd.clone();
        let deleting = std::thread::spawn(move || manager().delete(id, &folder, None, false));
        taken(id);
        let respawn = start(id, cwd);
        // Let through, the start would be done by now.
        std::thread::sleep(Duration::from_millis(300));
        quit.store(true, Ordering::SeqCst);
        assert_eq!(deleting.join().unwrap(), Ok(Deleted { worktree: None, worktree_kept: false }));
        let refused = respawn.join().unwrap().unwrap_err();
        assert!(refused.contains("deleted"), "{refused}");
        assert!(capture::take(id).is_none(), "nothing of a start was built");
        assert!(said::at(&format!("exit {id}")).is_some(), "the old session's exit was said");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A start after a kill waits for the old session's reader to have said
    /// it exited: said after, the exit would name the new session.
    #[test]
    fn a_start_after_a_kill_comes_after_the_old_sessions_exit() {
        let id = "dl-killed";
        let dir = scratch("killed");
        let cwd = dir.to_string_lossy().into_owned();
        let quit = fake_session(id, &cwd, Fake { reader_lag: Duration::from_millis(300), ..PROMPT });
        let killing = std::thread::spawn(move || tauri::async_runtime::block_on(kill_session(id.into())));
        taken(id);
        let respawn = start(id, cwd);
        quit.store(true, Ordering::SeqCst);
        killing.join().unwrap().unwrap();
        assert_eq!(respawn.join().unwrap(), Ok(String::new()));
        assert!(capture::take(id).is_some(), "the start ran");
        let (exit, launch) = (said::at(&format!("exit {id}")), said::at(&format!("launch {id}")));
        assert!(exit.is_some() && exit < launch, "the old exit was said before the start: {exit:?}, {launch:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A reader that is not through in time says nothing at all: whatever it
    /// would say is about a session that has been taken.
    #[test]
    fn a_reader_not_through_in_time_says_nothing() {
        let end = ReaderEnd::default();
        assert!(!end.wait(Duration::from_millis(20)));
        let generation = manager().next_generation("dl-late");
        reader_exited("dl-late", generation, Some(1), &end);
        assert_eq!(said::at("exit dl-late"), None);
        assert!(end.wait(Duration::ZERO), "through all the same");
    }

    /// A worktree another session went into while the deleted chat's process
    /// was quitting is kept: whether it is shared is asked once that process
    /// is gone.
    #[test]
    fn a_worktree_entered_while_the_delete_waits_is_kept() {
        let (id, other) = ("dl-wt-gone", "dl-wt-came");
        let dir = scratch("worktree");
        let wt = dir.join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        let (folder, wt_path) = (dir.to_string_lossy().into_owned(), wt.to_string_lossy().into_owned());
        let quit = fake_session(id, &wt_path, PROMPT);
        let (f, w) = (folder.clone(), wt_path.clone());
        let deleting = std::thread::spawn(move || manager().delete(id, &f, Some(w), true));
        taken(id);
        let came = fake_session(other, &wt_path, PROMPT);
        quit.store(true, Ordering::SeqCst);
        assert_eq!(deleting.join().unwrap(), kept(&wt_path), "kept, and said so");
        assert!(wt.exists());
        came.store(true, Ordering::SeqCst);
        manager().sessions.lock().unwrap_or_else(|e| e.into_inner()).remove(other);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A session in a worktree by way of a link or junction to its project
    /// keeps it from a delete that names it by its real path.
    #[test]
    fn a_worktree_reached_through_a_link_is_kept_from_a_delete_by_its_real_path() {
        let (id, other) = ("dl-wt-real", "dl-wt-linked");
        let dir = scratch("linked");
        let wt = dir.join("real").join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        crate::worktree::link_dir(&dir.join("link"), &dir.join("real"));
        let s = |p: &std::path::Path| p.to_string_lossy().into_owned();
        let (folder, wt_path) = (s(&dir.join("real")), s(&wt));
        let came = fake_session(other, &s(&dir.join("link").join("wt")), PROMPT);
        let quit = fake_session(id, &wt_path, PROMPT);
        let (f, w) = (folder.clone(), wt_path.clone());
        let deleting = std::thread::spawn(move || manager().delete(id, &f, Some(w), true));
        taken(id);
        quit.store(true, Ordering::SeqCst);
        assert_eq!(deleting.join().unwrap(), kept(&wt_path), "kept, and said so");
        assert!(wt.exists());
        came.store(true, Ordering::SeqCst);
        manager().sessions.lock().unwrap_or_else(|e| e.into_inner()).remove(other);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A session started through a link to its project, the link gone
    /// before its CLI has said anything, is still where the link led as it
    /// started: a delete by the real path keeps the worktree it is in, and
    /// the one it was started into by name.
    #[test]
    fn a_session_started_through_a_link_since_gone_keeps_its_worktree() {
        let (id, other) = ("dl-wt-pinned", "dl-wt-unlinked");
        let dir = scratch("unlinked");
        let wt = dir.join("real").join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        crate::worktree::link_dir(&dir.join("link"), &dir.join("real"));
        let s = |p: &std::path::Path| p.to_string_lossy().into_owned();
        let (folder, wt_path) = (s(&dir.join("real")), s(&wt));
        let came = fake_session(other, &s(&dir.join("link")), PROMPT);
        held_as(other, &[&s(&dir.join("link")), &s(&dir.join("link").join("wt"))], false);
        crate::worktree::unlink_dir(&dir.join("link"));
        let quit = fake_session(id, &wt_path, PROMPT);
        let (f, w) = (folder.clone(), wt_path.clone());
        let deleting = std::thread::spawn(move || manager().delete(id, &f, Some(w), true));
        taken(id);
        quit.store(true, Ordering::SeqCst);
        assert_eq!(deleting.join().unwrap(), kept(&wt_path), "kept, and said so");
        assert!(wt.exists());
        came.store(true, Ordering::SeqCst);
        manager().sessions.lock().unwrap_or_else(|e| e.into_inner()).remove(other);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A session started into a worktree by name, its `cwd` the project
    /// (Claude Code's `--worktree <name>`), keeps that worktree from another
    /// chat's delete from its start on: Claude Code's registry says where it
    /// went only once the CLI has come up.
    #[test]
    fn a_worktree_a_session_was_started_into_is_kept_from_its_start() {
        let (id, other) = ("dl-wt-before", "dl-wt-named");
        let dir = scratch("named");
        let wt = dir.join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        let (folder, wt_path) = (dir.to_string_lossy().into_owned(), wt.to_string_lossy().into_owned());
        let came = fake_session(other, &folder, PROMPT);
        manager().sessions.lock().unwrap_or_else(|e| e.into_inner()).get_mut(other).unwrap().enters = Some(wt_path.clone());
        held_as(other, &[&folder, &wt_path], false);
        let target = crate::worktree::Target::of(&wt_path, Some(crate::worktree::Place::of(&folder)));
        assert!(super::super::holds(None).keeps(&target, crate::worktree::Look::Sweep), "in use to the orphan sweep too");
        let quit = fake_session(id, &wt_path, PROMPT);
        let (f, w) = (folder.clone(), wt_path.clone());
        let deleting = std::thread::spawn(move || manager().delete(id, &f, Some(w), true));
        taken(id);
        quit.store(true, Ordering::SeqCst);
        assert_eq!(deleting.join().unwrap(), kept(&wt_path), "kept, and said so");
        assert!(wt.exists());
        came.store(true, Ordering::SeqCst);
        manager().sessions.lock().unwrap_or_else(|e| e.into_inner()).remove(other);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A session started with a new worktree whose place its CLI picks
    /// keeps every worktree of its folder where that CLI makes them from the
    /// sweep, while its working folder has not said where it went; one
    /// started without does not, and neither keeps one from a delete.
    #[test]
    fn a_session_making_a_worktree_is_waited_for_until_it_says_where() {
        let (making, plain) = ("dl-unplaced", "dl-placed");
        let dir = scratch("unplaced");
        let folder = dir.to_string_lossy().into_owned();
        let wt = crate::worktree::named_path(&folder, "picked-by-its-cli");
        let target = || crate::worktree::Target::of(&wt, Some(crate::worktree::Place::of(&folder)));
        let plain_quit = fake_session(plain, &folder, PROMPT);
        assert!(!super::super::holds(None).keeps(&target(), crate::worktree::Look::Sweep), "one started without");
        let making_quit = fake_session(making, &folder, PROMPT);
        held_as(making, &[&folder], true);
        assert!(super::super::holds(None).keeps(&target(), crate::worktree::Look::Sweep), "waited for");
        assert!(!super::super::holds(None).keeps(&target(), crate::worktree::Look::Delete), "not by a delete");
        for (id, quit) in [(making, making_quit), (plain, plain_quit)] {
            quit.store(true, Ordering::SeqCst);
            manager().sessions.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The sweep's last look before it removes an orphan, under the removal's
    /// hold, keeps one a session making a worktree may have made since the
    /// orphans were listed, which no session is in yet.
    #[test]
    fn the_sweeps_last_look_keeps_what_a_session_making_a_worktree_may_have_made() {
        let id = "dl-making";
        let dir = scratch("making");
        let folder = dir.to_string_lossy().into_owned();
        let quit = fake_session(id, &folder, PROMPT);
        let wt = crate::worktree::named_path(&folder, "made-by-its-cli");
        std::fs::create_dir_all(&wt).unwrap();
        assert!(!crate::worktree::kept_from_removal(&folder, &wt, &[], &[]), "nobody making one: an orphan still");
        held_as(id, &[&folder], true);
        assert!(!manager().worktree_shared("", &wt), "nobody is in it");
        assert!(crate::worktree::kept_from_removal(&folder, &wt, &[], &[]), "kept for the session making one");
        quit.store(true, Ordering::SeqCst);
        manager().sessions.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The sweep's removal looks again before it removes each orphan: one a
    /// session went into after Luna's sessions were read for the listing is
    /// kept, said as in use again, and nothing of it goes.
    #[test]
    fn the_sweep_keeps_an_orphan_a_session_went_into_since_the_listing() {
        let id = "dl-swept";
        let dir = scratch("swept");
        let folder = dir.to_string_lossy().into_owned();
        let wt = crate::worktree::named_path(&folder, "gone-into");
        std::fs::create_dir_all(&wt).unwrap();
        let quit = fake_session(id, &format!("{wt}/src"), PROMPT);
        // Read before the session went in: nobody there.
        let read_before = crate::worktree::Hold::default();
        let swept = crate::worktree::remove_orphans_now(&folder, &[], &[], &[], vec![wt.clone()], &read_before, 0).unwrap();
        assert!(swept.removed.is_empty());
        let failed: Vec<_> = swept.failed.iter().map(|f| (f.path.as_str(), f.error.as_str())).collect();
        assert_eq!(failed, [(wt.as_str(), crate::worktree::IN_USE_AGAIN)]);
        assert!(std::path::Path::new(&wt).exists());
        quit.store(true, Ordering::SeqCst);
        manager().sessions.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A session running once every session has been stopped for an update
    /// (one started while the stop went on) leaves the chats' ties frozen:
    /// the installer may yet end the process, and a save then would write
    /// what the stopping did. The update called off lifts it.
    #[test]
    fn a_session_running_after_a_stop_leaves_the_chats_frozen_until_it_is_called_off() {
        let _alone = crate::agents::stops_for_test();
        stopped_for_update(|| 0);
        let quit = fake_session("fz-escaped", "/nowhere", PROMPT);
        assert!(manager().live_count() > 0);
        assert!(crate::agents::chats_frozen(), "frozen with a session running");
        update_called_off();
        assert!(!crate::agents::chats_frozen(), "lifted with the update called off");
        quit.store(true, Ordering::SeqCst);
        manager().sessions.lock().unwrap_or_else(|e| e.into_inner()).remove("fz-escaped");
    }

    /// The window's word that the update is off, said while the sessions
    /// are still being stopped for it (by a page script, say: the window's
    /// install starts only once the stop has returned), leaves the chats'
    /// ties frozen until the stop returns, and lifts the freeze then.
    #[test]
    fn an_update_called_off_while_its_sessions_are_stopped_waits_for_the_stop() {
        let _alone = crate::agents::stops_for_test();
        let stopped = stopped_for_update(|| {
            update_called_off();
            assert!(crate::agents::chats_frozen(), "the freeze holds while they are stopped");
            3
        });
        assert_eq!(stopped, 3);
        assert!(!crate::agents::chats_frozen(), "lifted as the stop returned");
    }

    /// An exit said for a session whose chat has a start under way since, or
    /// was taken and let go, is not said: the registry's exit and the pane's
    /// event would land on the session after it. The current one's is, with
    /// its spawn number.
    #[test]
    fn an_exit_said_late_lands_on_no_later_session() {
        let id = "dl-gen";
        let old = manager().next_generation(id);
        // A start of the chat has begun; its session is not in the map yet.
        let new = manager().next_generation(id);
        reader_exited(id, old, Some(3), &ReaderEnd::default());
        assert_eq!(said::at(&format!("exit {id}")), None, "the old session's exit was not said");
        let told = || crate::emit::sent_for_test("pty://exit").into_iter().filter(|e| e["id"] == id).collect::<Vec<_>>();
        assert!(told().is_empty());
        reader_exited(id, new, Some(0), &ReaderEnd::default());
        assert_eq!(told(), [serde_json::json!({ "id": id, "code": 0, "generation": new })]);
        // Taken and let go (a kill's wait over): said already, it is not said
        // again, by the taker or after.
        manager().retire(id, new, Some(0));
        reader_exited(id, new, Some(0), &ReaderEnd::default());
        assert_eq!(told().len(), 1);
    }

    /// A kill of a session whose reader, held, has its word that the
    /// session exited said or not by the kill's end, and is let through
    /// after it, if it was held: what the window was told of `id`'s exit as
    /// the kill returned, with a reader still held by then held all along,
    /// and once the reader is through; and the session's spawn number.
    fn kill_with_a_held_reader(
        id: &'static str,
        saying: bool,
        through_first: bool,
    ) -> (Vec<serde_json::Value>, Vec<serde_json::Value>, u64) {
        let dir = scratch(id);
        let (quit, reader) = held_session(id, &dir.to_string_lossy(), saying);
        let generation = manager().sessions.lock().unwrap_or_else(|e| e.into_inner())[id].generation;
        let told = || crate::emit::sent_for_test("pty://exit").into_iter().filter(|e| e["id"] == id).collect::<Vec<_>>();
        let killing = std::thread::spawn(move || tauri::async_runtime::block_on(kill_session(id.into())));
        taken(id);
        quit.store(true, Ordering::SeqCst);
        reader.at_end.recv().unwrap();
        if through_first {
            reader.go.send(()).unwrap();
            reader.through.recv().unwrap();
        }
        killing.join().unwrap().unwrap();
        let at_the_kills_end = told();
        if !through_first {
            reader.go.send(()).unwrap();
            reader.through.recv().unwrap();
        }
        let _ = std::fs::remove_dir_all(&dir);
        (at_the_kills_end, told(), generation)
    }

    /// A kill whose session's reader has not begun to say the exit by the
    /// end of its wait says it itself: the pane, or the login window, learns
    /// the session is gone, from the kill, the reader still held; and the
    /// reader coming through after says nothing.
    #[test]
    fn a_kill_past_its_wait_says_the_exit_for_a_reader_not_begun() {
        let (by_the_kill, told, generation) = kill_with_a_held_reader("dl-reader-stuck", false, false);
        let exit = serde_json::json!({ "id": "dl-reader-stuck", "code": 0, "generation": generation });
        assert_eq!(by_the_kill, std::slice::from_ref(&exit), "said by the kill");
        assert_eq!(told, [exit], "and by nobody after");
    }

    /// A reader in the middle of saying the exit as the kill's wait ends,
    /// and its word coming after the kill's: said once, by the kill.
    #[test]
    fn a_reader_saying_the_exit_after_the_kill_said_it_says_nothing() {
        let (by_the_kill, told, generation) = kill_with_a_held_reader("dl-reader-late", true, false);
        let exit = serde_json::json!({ "id": "dl-reader-late", "code": 0, "generation": generation });
        assert_eq!(by_the_kill, std::slice::from_ref(&exit), "said by the kill");
        assert_eq!(told, [exit], "and by nobody after");
    }

    /// A reader that said the exit before the kill came to say it: said
    /// once, by the reader.
    #[test]
    fn a_kill_after_the_reader_said_the_exit_says_nothing() {
        let (by_then, told, generation) = kill_with_a_held_reader("dl-reader-first", true, true);
        let exit = serde_json::json!({ "id": "dl-reader-first", "code": 0, "generation": generation });
        assert_eq!((by_then, told), (vec![exit.clone()], vec![exit]));
    }

    /// A start into a worktree that a delete has found free and is removing
    /// is refused, saying so; the same start once the removal is over goes.
    #[test]
    fn a_start_into_a_worktree_being_removed_is_refused() {
        let id = "dl-wt-late";
        let dir = scratch("removing");
        let wt = dir.join("wt").to_string_lossy().into_owned();
        let target = crate::worktree::Target::of(&wt, None);
        let removal = crate::worktree::removing(target, crate::worktree::Look::Delete, || manager().worktree_shared("dl-wt-owner", &wt))
            .expect("free");
        let refused = start(id, format!("{wt}/sub")).join().unwrap().unwrap_err();
        assert!(refused.contains("being removed"), "{refused}");
        assert!(capture::take(id).is_none());
        drop(removal);
        assert_eq!(start(id, wt).join().unwrap(), Ok(String::new()));
        assert!(capture::take(id).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Keys a session does not take, sent as the window sends them (a send
    /// through `corecmd::dispatch` whose answer nobody reads), are refused,
    /// and the window hears so once; the core's answer to a cursor query
    /// still goes in.
    #[test]
    fn keys_a_session_does_not_take_reach_the_window_as_an_event() {
        let id = "dl-stuck";
        let quit = fake_session(id, "/nowhere", Fake { drains: false, ..PROMPT });
        let chunk = "k".repeat(1024 * 1024);
        let sent: Vec<bool> =
            (0..10).map(|_| crate::corecmd::dispatch("write_session", serde_json::json!({ "id": id, "data": chunk })).is_ok()).collect();
        assert!(sent.iter().take(8).all(|ok| *ok) && !sent[8] && !sent[9], "{sent:?}");
        let told: Vec<_> = crate::emit::sent_for_test("pty://input-refused").into_iter().filter(|e| e["id"] == id).collect();
        assert_eq!(told.len(), 1, "once a stretch: {told:?}");
        assert_eq!(told[0]["waitingKb"], 8 * 1024);
        let sessions = manager().sessions.lock().unwrap_or_else(|e| e.into_inner());
        assert!(sessions[id].input_tx.reply(b"\x1b[1;1R".to_vec()).is_ok(), "a cursor answer goes whatever waits");
        drop(sessions);
        quit.store(true, Ordering::SeqCst);
        manager().sessions.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
    }

    /// The native window's terminal answers the program's queries itself and
    /// says so (`reply`): an OSC color or a size report is not a person
    /// typing, and starts no draft; the same bytes as keys would.
    #[test]
    fn a_reply_from_the_windows_terminal_is_not_typing() {
        let id = "dl-reply";
        let quit = fake_session(id, "/nowhere", PROMPT);
        let counts = || {
            let sessions = manager().sessions.lock().unwrap_or_else(|e| e.into_inner());
            let s = &sessions[id];
            (s.typed_ms.load(Ordering::Relaxed), s.draft.load(Ordering::Relaxed), s.keys_draft.load(Ordering::Relaxed))
        };
        for data in ["\x1b]11;rgb:ffff/ffff/ffff\x1b\\", "\x1b[8;30;100t", "\x1b[?62;22c"] {
            let sent = crate::corecmd::dispatch("write_session", serde_json::json!({ "id": id, "data": data, "reply": true }));
            assert!(sent.is_ok(), "{data:?}: {sent:?}");
            assert_eq!(counts(), (0, 0, 0), "{data:?}");
        }
        // Without it, as the page sends everything: the color's digits count
        // as a draft, the size report as typing.
        for data in ["\x1b]11;rgb:ffff/ffff/ffff\x1b\\", "\x1b[8;30;100t"] {
            crate::corecmd::dispatch("write_session", serde_json::json!({ "id": id, "data": data })).unwrap();
        }
        let (typed, draft, _) = counts();
        assert!(typed > 0 && draft > 0, "{:?}", counts());
        quit.store(true, Ordering::SeqCst);
        manager().sessions.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
    }
}
