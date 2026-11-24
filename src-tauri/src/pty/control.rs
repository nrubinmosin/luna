//! What is done to a running session: a person's keys and Luna's text into
//! it, resizes, quitting it the way a person would before it is killed,
//! deleting a chat, and the sessions no chat claims any more.

use super::probe::{meta_from_disk, probe_of, Probe};
use super::screen::{draft_after, paste_lapsed, typed_by_person, DRAFT_FLAGS};
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
/// `has_draft`), which an OSC color reply would otherwise count as text. Only
/// a write that is one such answer whole (`terminal_reply`) is taken for one;
/// anything else so labeled takes the keys' path, held and counted as keys
/// are. The page never sends it: xterm.js's replies take the keys' path as
/// before.
pub fn write_session(id: String, data: String, reply: Option<bool>) -> Result<(), String> {
    let reply = reply == Some(true) && terminal_reply(&data);
    // A person at the keys is activity before the key lands: an Enter that
    // starts a turn after a power-off's fresh read moves the stamp it is
    // decided by (power/ `barrier`). Never refused: a person is not. The
    // terminal's own answer is not a person.
    if !reply && typed_by_person(&data) {
        crate::activity::touch();
    }
    // An Enter on a draft sends a prompt: the CLI's furniture is taken first.
    if !reply && manager().sends_draft(&id, &data) {
        crate::activity::submitting(&id);
    }
    let state = manager();
    let sessions = state.sessions.lock().unwrap_or_else(|e| e.into_inner());
    let s = sessions.get(&id).ok_or("no such session")?;
    if reply {
        return s.input_tx.send(Input::Bytes(data.into_bytes()));
    }
    if let Some(held) = s.held_keys.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        held.push(data);
        return Ok(());
    }
    take_keys(s, data)
}

/// The longest terminal answer taken as one (`terminal_reply`): a color is 26
/// bytes, a size report 20 or so.
const REPLY_MAX: usize = 64;

/// Whether `data` is one answer a terminal makes to its program's query, whole,
/// and nothing else: what alacritty_terminal 0.26 answers (src/term/mod.rs),
/// the device's attributes (`CSI ? 6 c`, `CSI > 0 ; v ; 1 c`), its status and
/// the cursor's place (`CSI 0 n`, `CSI r ; c R`), a mode's state
/// (`CSI ? m ; s $ y`), the keyboard protocol's flags (`CSI ? f u`), the text
/// area's size (`CSI 4|8 ; h ; w t`), a color (`OSC 4|10..19 ; ... rgb:...`
/// ended by BEL or ST), and the focus coming and going (`CSI I`, `CSI O`);
/// and a DCS status answer (`DCS 0|1 $|+ r ... ST`). Keys never take these
/// shapes but for an F3 with modifiers, which shares the cursor's; a paste
/// is framed, or has no ESC.
pub(super) fn terminal_reply(data: &str) -> bool {
    if data.len() > REPLY_MAX {
        return false;
    }
    if let Some(csi) = data.strip_prefix("\x1b[") {
        if matches!(csi, "I" | "O") {
            return true;
        }
        let (prefix, rest) = match csi.as_bytes().first() {
            Some(c @ (b'?' | b'>')) => (Some(*c), &csi[1..]),
            _ => (None, csi),
        };
        let Some(last) = rest.bytes().last().filter(u8::is_ascii) else { return false };
        let body = &rest[..rest.len() - 1];
        let body = body.strip_suffix('$').filter(|_| last == b'y').unwrap_or(body);
        let params = !body.is_empty() && body.bytes().all(|c| c.is_ascii_digit() || c == b';');
        return params
            && matches!(
                (prefix, last),
                (Some(b'?' | b'>'), b'c') | (None | Some(b'?'), b'n' | b'y') | (None, b'R' | b't') | (Some(b'?'), b'u')
            );
    }
    if let Some(osc) = data.strip_prefix("\x1b]") {
        let Some(body) = osc.strip_suffix('\x07').or_else(|| osc.strip_suffix("\x1b\\")) else { return false };
        let (number, rest) = body.split_once(';').unwrap_or((body, ""));
        let colored = match number.parse::<u8>() {
            Ok(4) => rest.split_once(';').is_some_and(|(i, c)| i.bytes().all(|b| b.is_ascii_digit()) && c.starts_with("rgb:")),
            Ok(10..=19) => rest.starts_with("rgb:"),
            _ => false,
        };
        return colored && rest.bytes().all(|b| b.is_ascii_graphic());
    }
    if let Some(dcs) = data.strip_prefix("\x1bP") {
        let Some(body) = dcs.strip_suffix("\x1b\\") else { return false };
        let b = body.as_bytes();
        return b.len() >= 3
            && matches!(b[0], b'0' | b'1')
            && matches!(b[1], b'$' | b'+')
            && b[2] == b'r'
            && b.iter().all(|c| c.is_ascii_graphic());
    }
    false
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
    s.draft.store(paste_lapsed(s.draft.load(Ordering::Relaxed), last, now), Ordering::Relaxed);
    if typed_by_person(&data) {
        s.typed_ms.store(now, Ordering::Relaxed);
    }
    let draft = draft_after(s.draft.load(Ordering::Relaxed), &data);
    s.draft.store(draft, Ordering::Relaxed);
    if s.provider == Provider::Claude && data.contains('\x13') {
        s.stashing.store(true, Ordering::Relaxed);
    }
    // Queueing, so the command returns at once however busy the pty is.
    s.input_tx.send(Input::Bytes(data.into_bytes()))
}

/// Codex on Windows takes its text as key events (`as_key_events`), not as a
/// paste.
fn takes_keys(provider: Provider) -> bool {
    cfg!(windows) && provider == Provider::Codex
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
pub fn resize_session(id: String, cols: u16, rows: u16, pixel_width: Option<u16>, pixel_height: Option<u16>) -> Result<(), String> {
    let state = manager();
    let sessions = state.sessions.lock().unwrap_or_else(|e| e.into_inner());
    let s = sessions.get(&id).ok_or("no such session")?;
    // The grid's size in pixels, as TIOCSWINSZ carries it on Unix (ConPTY
    // takes the cells only): the native window sends it, the page does not.
    let (pixel_width, pixel_height) = (pixel_width.unwrap_or(0), pixel_height.unwrap_or(0));
    s.input_tx.send(Input::Resize(PtySize { rows, cols, pixel_width, pixel_height }))
}

/// How long `taskkill` gets to end a tree before it is given up on: a process
/// that will not die (one in a driver's wait) is left as it is rather than hold
/// up whoever stops it, a core's going and a window replacing a core among them
/// (corelink.rs `LINK_WAIT`, which counts this).
#[cfg(windows)]
const KILL_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

// The pty child is the CLI; everything it spawned (node, MCP servers, shells)
// is a grandchild and survives a plain kill. Sweep the tree first, then kill
// the child directly as a backstop.
pub(crate) fn kill_tree(pid: Option<u32>) {
    let Some(pid) = pid else { return };
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let killer = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        if let Ok(mut killer) = killer {
            let deadline = std::time::Instant::now() + KILL_WAIT;
            loop {
                match killer.try_wait() {
                    Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(std::time::Duration::from_millis(10)),
                    Ok(None) => {
                        crate::log::warn("pty", &format!("taskkill of pid {pid}'s tree was not through within {KILL_WAIT:?}; left to it"));
                        let _ = killer.kill();
                        let _ = killer.wait();
                        break;
                    }
                    _ => break,
                }
            }
        }
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
/// runs its exit hooks first — among them Luna's own SessionEnd hook, a post
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
    // takes the focused default, Keep — the right answer, as Luna removes no
    // worktree. The Ctrl+Cs have already cleared the prompt, so an Enter that
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
    // out. On Windows the session holds the process's handle until it is
    // dropped below, so its pid is nobody else's yet and a taskkill on it is a
    // no-op at worst.
    #[cfg(windows)]
    {
        kill_tree(s.pid);
        let _ = s.killer.kill();
    }
    // Off Windows a reaped pid may be another process's already, another child
    // of the core's among them, which both the tree walk and the killer (a
    // signal to the pid by number) would take for this one. Every reap of the
    // CLI goes through its lock (`try_wait`: here, the reader's watch, the
    // exit's code), so the check and the signals are one step under it: a
    // process found unreaped stays unreaped, its pid its own, until they are
    // sent. One reaped already is past signaling; what it left goes by the
    // session's job (`end_leftovers`).
    #[cfg(not(windows))]
    {
        let child = Arc::clone(&s.child);
        let mut child = child.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(child.try_wait(), Ok(None)) {
            kill_tree(s.pid);
            let _ = s.killer.kill();
        }
    }
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
    /// held throughout: a spawn of the chat still in flight (a helper deleted
    /// the moment it was made) is waited out, as a
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
/// turn is stopped too, and what helpers' turns left running is ended
/// (`background::stop_for_update`): a turn would count as a session and keep
/// the old core from being replaced, the parent chats having just been
/// stopped, and what was left would go only with that core. Returns how many it stopped. The chats' ties stay frozen, and the start gate
/// closed (`crate::starts`), until the update is called off
/// (`update_called_off`, a panic in the stop, `agents::window_came`) or the
/// updated window replaces this core (`agents::freeze_for_update`). `asker`:
/// the window process it was asked on (coreproc.rs `requests`). Refused once
/// the core is stopping (`agents::admit_shutdown`).
///
/// `confirmed`: the sessions the person agreed to stop, by id (the window's
/// update panel counted them off this core's board): none where it said the
/// sessions keep running or none runs. One started since, by a launch let in
/// before the gate closed or a helper's turn claimed by then, is not among
/// them, and the stop is refused with `UNCONFIRMED` before it stops anything
/// (`stopped_for_update`). `call_offs`: the update's call offs counted as
/// the request was read (`agents::freeze_for_update`): one read after
/// it but run first refuses it.
pub async fn stop_all_sessions(asker: Option<crate::agents::Window>, confirmed: Vec<String>, call_offs: u64) -> Result<usize, String> {
    crate::runtime::spawn_blocking(move || {
        // What a chat was doing is written down before quitting changes it.
        crate::background::save();
        let look = || {
            let helpers = crate::background::running_for_update();
            let mut ids = manager().live_ids();
            ids.extend(helpers.iter().map(|(id, _)| id.clone()));
            (ids, helpers)
        };
        stopped_for_update(asker, call_offs, LAUNCH_WAIT, &confirmed, look, |helpers| {
            manager().shut_down_all(GRACE) + crate::background::stop_for_update(&helpers)
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// What an update's stop is refused with where it would stop a session the
/// person did not agree to stop (`stop_all_sessions`): the window calls the
/// update off and draws its panel again, for what runs now.
pub const UNCONFIRMED: &str =
    "a session runs that the update's confirmation did not count among those it stops, so none is stopped: look again before installing";

/// How long an update's stop waits for the pane launches under way to
/// register (`starts::seal`): a launch takes a second or two, its CLI's start
/// not waited for.
const LAUNCH_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// `stop` run with the chats' ties frozen, and the start gate closed, for an
/// update, neither to lift until it has returned (`agents::freeze_for_update`).
/// Nothing is stopped where the freeze is refused: the core is leaving, or
/// the update was called off after the stop was asked (`call_offs`,
/// `agents::freeze_for_update`). With the gate closed, the pane
/// launches under way are waited for up to `launch_wait` and sealed off
/// (`starts::seal`); then `look` lists, by id, every session the stop can
/// end, a helper's turn claimed before the gate closed among them, with what
/// `stop` is held to (the helpers' turns by claim, `background::stop_for_update`).
/// Where a launch is still under way when the wait is up (its CLI started,
/// about to register as a session nobody was asked about), or a session
/// listed is not in `confirmed`, nothing is stopped: the update is called
/// off, the gate opened again and the freeze lifted as this returns, and the
/// stop is refused with `UNCONFIRMED`; the late launch registers as it would
/// have.
fn stopped_for_update<S>(
    asker: Option<crate::agents::Window>,
    call_offs: u64,
    launch_wait: std::time::Duration,
    confirmed: &[String],
    look: impl FnOnce() -> (Vec<String>, S),
    stop: impl FnOnce(S) -> usize,
) -> Result<usize, String> {
    let stopping = crate::agents::freeze_for_update(asker, call_offs)?;
    if let Err(late) = crate::starts::seal(launch_wait) {
        crate::log::warn("update", &format!("the sessions' stop is refused: {late} pane launch(es) not registered in time"));
        stopping.call_off();
        return Err(UNCONFIRMED.to_owned());
    }
    let (running, seen) = look();
    let unconfirmed: Vec<String> = running.into_iter().filter(|id| !confirmed.contains(id)).collect();
    if !unconfirmed.is_empty() {
        let who: Vec<String> = unconfirmed.iter().map(|id| crate::agents::label(id)).collect();
        crate::log::warn("update", &format!("the sessions' stop is refused: {} not confirmed for it", who.join(", ")));
        stopping.call_off();
        return Err(UNCONFIRMED.to_owned());
    }
    Ok(stop(seen))
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
pub async fn kill_session(id: String) -> Result<(), String> {
    crate::runtime::spawn_blocking(move || {
        let state = manager();
        let gate = state.gate(&id);
        let _quitting = gate.lock().unwrap_or_else(|e| e.into_inner());
        state.take_and_quit(&id);
    })
    .await
    .map_err(|e| e.to_string())
}

/// What the orphan line's kill and adopt are refused with where the session
/// under the id is not the one the line showed: another start of it, by this
/// core or by one that replaced it (`Start`).
pub const NOT_THE_ORPHAN: &str = "that session is no longer the one shown";

/// The orphan line's kill (`orphan_sessions`): ends the session under `id`
/// only while it is the very start the line showed (`Start`: its generation
/// in this core, by this core's mark), checked under the spawn gate, which a
/// start of the id waits on, so no other process can take the id between the
/// check and the kill. One gone already is nothing to kill; another start of
/// the id, or a start another core gave, is left running and refused with
/// `NOT_THE_ORPHAN`. Not refused for a helper's id as `delete_session` is: it
/// ends that start's process alone and forgets no registry entry or token.
pub async fn kill_orphan(id: String, start: super::Start) -> Result<(), String> {
    crate::runtime::spawn_blocking(move || {
        let state = manager();
        let gate = state.gate(&id);
        let _quitting = gate.lock().unwrap_or_else(|e| e.into_inner());
        let running = state.sessions.lock().unwrap_or_else(|e| e.into_inner()).get(&id).map(|s| s.generation);
        match running {
            None if start.core == super::core_nonce() => Ok(()),
            Some(g) if start.is(g) => {
                state.take_and_quit(&id);
                Ok(())
            }
            _ => Err(NOT_THE_ORPHAN.to_owned()),
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Whether the session the orphan line showed (`id` at `start`) still runs:
/// asked before Adopt lists a chat for it, whose pane then attaches to that
/// start alone (`spawn`'s `expect`).
pub fn orphan_live(id: String, start: super::Start) -> bool {
    let sessions = manager().sessions.lock().unwrap_or_else(|e| e.into_inner());
    sessions.get(&id).is_some_and(|s| start.is(s.generation) && s.alive.load(Ordering::SeqCst))
}

fn refuse_a_helper(id: &str) -> Result<(), String> {
    if crate::background::has(id) {
        // The window shows this refusal in the delete dialog.
        return Err(format!(
            "{id} {}; it can be deleted once it has opened or is back with its agent",
            luna_window::dialogs::rows::HELD_AS_HELPER
        ));
    }
    Ok(())
}

/// Tears a chat down in one shot: kill its session and remove its attachments.
/// A worktree it ran in is left as it is: Luna removes none.
// Async: the session quits in up to `GRACE`, and on the main thread every
// keystroke in every other chat waits behind it.
pub async fn delete_session(id: String) -> Result<(), String> {
    // A helper's id, still the core's to hand over or give back: deleting it as
    // a chat would forget its registry entry and token and leave the helper
    // (a window whose row of an opening showed before its first list of
    // helpers came). The person deletes a helper as a helper.
    refuse_a_helper(&id)?;
    // On a thread for blocking work: the delete holds the chat's gate while
    // the session quits, which takes up to `GRACE`.
    crate::runtime::spawn_blocking(move || manager().delete(&id)).await.map_err(|e| e.to_string())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrphanSession {
    /// The chat id the session was spawned under. Re-creating a chat row with
    /// this exact id is what reattaches it, scrollback included.
    pub id: String,
    /// Which start of the id it is (`Start`): what the line's kill and adopt
    /// are held to (`kill_orphan`, `orphan_live`, the adopted pane's ensure).
    pub start: super::Start,
    pub provider: Provider,
    pub pid: Option<u32>,
    /// Where it runs: its CLI's own word where it has one, a worktree the
    /// CLI made for it included; else the folder it was started in.
    pub cwd: String,
    pub account_path: String,
    pub title: Option<String>,
    pub status: Option<String>,
    /// The CLI's own session, for the adopted chat to resume should its
    /// process exit.
    pub session_id: Option<String>,
    /// What it was started with, where the agents' registry knows it.
    pub modes: Option<OrphanModes>,
}

/// What an orphan was started with, as the CLI's flags spell it (the
/// registry's settings, agents/registry.rs `Known::settings`), and whether it
/// was given Luna's tools: what an adopted chat keeps, should its CLI start
/// again.
#[derive(Serialize, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OrphanModes {
    pub model: Option<String>,
    pub effort: Option<String>,
    pub permission_mode: Option<String>,
    pub approval: Option<String>,
    pub sandbox: Option<String>,
    pub read_only: bool,
    pub agent: Option<String>,
    pub tools: bool,
}

fn modes_of(id: &str) -> Option<OrphanModes> {
    if !crate::agents::is_known(id) {
        return None;
    }
    let s = crate::agents::settings_of(id);
    let text = |key: &str| s.get(key).and_then(serde_json::Value::as_str).filter(|v| !v.is_empty()).map(str::to_owned);
    Some(OrphanModes {
        model: text("model"),
        effort: text("effort"),
        permission_mode: text("permissionMode"),
        approval: text("approval"),
        sandbox: text("sandbox"),
        read_only: s.get("readOnly").and_then(serde_json::Value::as_bool).unwrap_or(false),
        agent: text("agent"),
        tools: crate::agents::has_tools(id),
    })
}

/// An orphan as the line is told of it: the session's probe and start, with
/// what its CLI left on disk (`meta`), read in the folder it runs in now.
fn orphan_of(p: Probe, generation: u64, meta: Option<super::SessionMeta>, modes: Option<OrphanModes>) -> OrphanSession {
    let cwd = meta.as_ref().and_then(|m| m.cwd.clone()).filter(|c| !c.is_empty()).unwrap_or(p.cwd);
    OrphanSession {
        id: p.id,
        start: super::Start { generation, core: super::core_nonce().to_owned() },
        provider: p.provider,
        pid: p.pid,
        cwd,
        account_path: p.account_path,
        title: meta.as_ref().and_then(|m| m.title.clone().or_else(|| m.first_prompt.clone()).or_else(|| m.name.clone())),
        status: meta.as_ref().and_then(|m| m.status.clone()),
        session_id: meta.and_then(|m| m.session_id),
        modes,
    }
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
/// would report every busy chat's children as orphans. A sign-in, which runs
/// in the account folder itself (`same_dir`, as its start tells one), is the
/// accounts window's and no chat's: it is left out, so no chat adopts it and
/// no Kill all ends it.
pub async fn orphan_sessions(known: Vec<String>) -> Result<Vec<OrphanSession>, String> {
    let state = manager();
    let candidates: Vec<(Probe, u64)> = {
        let sessions = state.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions
            .iter()
            .filter(|(id, s)| !known.contains(id) && s.alive.load(Ordering::SeqCst) && !super::same_dir(&s.cwd, &s.account_path))
            .map(|(id, s)| (probe_of(id, s), s.generation))
            .collect()
    };
    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    // Naming one means reading the head of its transcript, so do it on a
    // thread for blocking work.
    crate::runtime::spawn_blocking(move || {
        candidates
            .into_iter()
            .map(|(p, generation)| {
                let (meta, modes) = (meta_from_disk(&p), modes_of(&p.id));
                orphan_of(p, generation, meta, modes)
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
        let at = self.typed_at_ms(id);
        (at > 0).then(|| now_ms().saturating_sub(at))
    }

    /// Whether a person's `data` holds an Enter that sends a prompt: one on
    /// text in the composer, as the keys before it leave it (`draft_after`),
    /// not inside a paste nor after a backslash. An Enter on an empty composer
    /// (a startup dialog, a menu) sends none.
    pub fn sends_draft(&self, id: &str, data: &str) -> bool {
        let Some(at) = data.find('\r') else { return false };
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        let Some(s) = sessions.get(id) else { return false };
        let before = draft_after(s.draft.load(Ordering::Relaxed), &data[..at]);
        before & !DRAFT_FLAGS > 0 && before & DRAFT_FLAGS == 0
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

    /// Queues bytes for the session's writer thread: Luna's own text, which
    /// is activity, and which a power-off being decided or an update's stop
    /// refuses. The stamp first, then the gate (`crate::starts`): a write
    /// either moves the stamp before the decision reads it, or finds the gate
    /// shut.
    pub fn write(&self, id: &str, data: &[u8]) -> Result<(), String> {
        crate::activity::touch();
        crate::starts::open_or_refuse(id)?;
        self.write_through(id, data)
    }

    /// `write` as a part of a typing a power-off let in (`power::typing`),
    /// which the decision counts as work and waits on or is canceled by: its
    /// later parts are not refused by a gate closed since, which would leave
    /// text pasted and unsent.
    pub fn write_in(&self, id: &str, data: &[u8], _typing: &crate::power::Entered) -> Result<(), String> {
        crate::activity::touch();
        self.write_through(id, data)
    }

    fn write_through(&self, id: &str, data: &[u8]) -> Result<(), String> {
        // Luna's Enter sends what it typed: the CLI's furniture is taken first.
        if data.contains(&b'\r') {
            crate::activity::submitting(id);
        }
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
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
    ///
    /// Part of a typing a power-off let in (`write_in`): Luna's text is never
    /// pasted outside one.
    pub fn paste_in(&self, id: &str, text: &str, typing: &crate::power::Entered) -> Result<(), String> {
        let put = |data: &[u8]| self.write_in(id, data, typing);
        let bracketed = {
            let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
            let s = sessions.get(id).ok_or("no such session")?;
            !takes_keys(s.provider) && s.modes.lock().unwrap_or_else(|e| e.into_inner()).on.get(&2004) == Some(&true)
        };
        if bracketed {
            put(format!("\x1b[200~{text}\x1b[201~").as_bytes())
        } else {
            put(text.as_bytes())
        }
    }

    /// Whether text typed into the session goes in as key events, one per
    /// character (`paste_in`), which the CLI takes in at its own pace.
    pub fn types_keys(&self, id: &str) -> bool {
        self.provider(id).is_some_and(takes_keys)
    }

    /// Tears a chat's session down: quit the process, drop attachments and the
    /// hooks file. Blocks while the session quits (`GRACE` at most): callers
    /// run it off the main thread.
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
    pub fn delete(&self, id: &str) {
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
        self.take_and_quit(id);
        // Everything else that kept count of it, so nothing keeps the chat on
        // after it is gone: the agent registry (luna_session_list, the helper
        // limit, its token), the turn counters, a waiting cleared on it, the
        // Codex rollout it held.
        crate::agents::forget(id);
        crate::activity::forget(id);
        crate::codex::session::release(id);

        let _ = crate::media::clear_media(id.to_string());
        for sub in ["hooks", "mcp"] {
            let _ = std::fs::remove_file(crate::paths::data_dir().join(sub).join(format!("{}.json", crate::paths::id_file_name(id))));
        }
        let _ = claude_instructions_dir(id, None);
        crate::library::drop_plugins(id);
        crate::log::info("delete", &format!("chat {}", crate::agents::label(id)));
    }

    /// Puts a session just launched in the map under its launch's ticket
    /// (`starts::Launching::register`), unless an update's stop sealed the
    /// launches off without it, having waited for it as long as it would:
    /// then it is asked to quit as the stop asked the rest, and its launch is
    /// refused.
    pub(super) fn register(&self, id: &str, launching: &crate::starts::Launching, s: Session) -> Result<(), String> {
        let put = |s| {
            self.sessions.lock().unwrap_or_else(|e| e.into_inner()).insert(id.to_string(), s);
        };
        let Err(s) = launching.register(s, put) else { return Ok(()) };
        crate::log::warn("pty", &format!("{id} started after an update's stop had taken its snapshot; asking it to quit"));
        crate::cli::session_stopped(id);
        shut_down(s, GRACE);
        Err(crate::starts::updating())
    }

    /// Asks every live session to quit, each with `grace` to do it on its
    /// own, and returns how many there were. Blocks; for the shutdown path.
    pub fn shut_down_all(&self, grace: std::time::Duration) -> usize {
        self.shut_down_of(&|_| true, grace)
    }

    /// `shut_down_all` for the sessions `of` picks.
    fn shut_down_of(&self, of: &dyn Fn(&str) -> bool, grace: std::time::Duration) -> usize {
        let taken: Vec<(String, Session)> = {
            let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
            let ids: Vec<String> =
                sessions.iter().filter(|(id, s)| of(id) && s.alive.load(Ordering::SeqCst)).map(|(id, _)| id.clone()).collect();
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
pub(super) mod tests {
    use super::super::spawn::{capture, exit_word, reader_exited, said, spawn, Attached, Launch, ADOPTED_GONE};
    use super::super::{InputTx, ReaderEnd};
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU64};
    use std::sync::Mutex;
    use std::time::Duration;

    /// A row a window shows of an opening before its first list of helpers
    /// came is not deleted as a chat while the core still holds the helper:
    /// its registry and token would go and the helper stay.
    #[test]
    fn a_helpers_id_is_not_deleted_as_a_chat() {
        let id = "ds-held-helper";
        crate::background::idle_for_test(id, Provider::Codex, 1, false);
        crate::background::promoting_for_test(id, true);
        assert!(crate::runtime::block_on(delete_session(id.into())).unwrap_err().contains("still a helper"));
        crate::background::gone_for_test(id);
        assert!(refuse_a_helper(id).is_ok(), "the person's once the core has let go");
    }

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
        let (session, quit) = fake_session_built(id, cwd, drains, None, ending);
        manager().sessions.lock().unwrap_or_else(|e| e.into_inner()).insert(id.to_string(), session);
        quit
    }

    /// A session of the test's as a launch has it before it registers. One
    /// given `asked` goes on a Ctrl+C, as a CLI asked to quit does, and sets
    /// it then; the hard kill sets only the flag returned.
    fn fake_session_built(id: &str, cwd: &str, drains: bool, asked: Option<Arc<AtomicBool>>, ending: Ending) -> (Session, Arc<AtomicBool>) {
        let quit = Arc::new(AtomicBool::new(false));
        let child: Box<dyn portable_pty::Child + Send + Sync> = Box::new(FakeChild { quit: Arc::clone(&quit) });
        let (input_tx, input_rx, written) = InputTx::new(id);
        let quits = Arc::clone(&quit);
        std::thread::spawn(move || {
            let mut never_written = Vec::new();
            for input in input_rx {
                let ctrl_c = matches!(&input, Input::Bytes(b) if b.as_slice() == b"\x03");
                if let Some(asked) = asked.as_ref().filter(|_| ctrl_c) {
                    asked.store(true, Ordering::SeqCst);
                    quits.store(true, Ordering::SeqCst);
                }
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
            skills_unhidden: 0,
            reader_end,
            input_tx: Arc::new(input_tx),
            killer: child.clone_killer(),
            child: Arc::new(Mutex::new(child)),
            scrollback: Arc::new(Mutex::new(super::super::screen::Scrollback::with_capacity(0))),
            drawn: Arc::new(Mutex::new(super::super::screen::Drawn::default())),
            modes: Arc::new(Mutex::new(super::super::screen::Modes::default())),
            sent: Arc::default(),
            alive,
            last_output_ms: Arc::new(AtomicU64::new(now_ms())),
            pid: None,
            cwd: cwd.to_string(),
            spawned_at_ms: now_ms() as u128,
            codex_compaction: None,
            resume: None,
            // The account the tests' starts ask for (`start`).
            account_path: "acc".into(),
            account_dir: None,
            job: None,
            size: Arc::new(Mutex::new((30, 100))),
            typed_ms: Arc::new(AtomicU64::new(0)),
            draft: Arc::new(AtomicU64::new(0)),
            draft_seen: Arc::new(Mutex::new((0, false))),
            held_keys: Arc::new(Mutex::new(None)),
            stashing: Arc::new(AtomicBool::new(false)),
        };
        (session, quit)
    }

    pub(crate) struct NoticeTerminal<'a> {
        id: &'a str,
        quit: Arc<AtomicBool>,
        pub(crate) input: std::sync::mpsc::Receiver<String>,
    }

    impl Drop for NoticeTerminal<'_> {
        fn drop(&mut self) {
            manager().sessions.lock().unwrap().remove(self.id);
            self.quit.store(true, Ordering::SeqCst);
        }
    }

    pub(crate) fn notice_terminal(id: &str, provider: Provider, draft: bool) -> NoticeTerminal<'_> {
        let (mut session, quit) = fake_session_built(id, ".", true, None, Box::new(|_, _, _| {}));
        session.provider = provider;
        session.spawned_at_ms = now_ms().saturating_sub(120_000) as u128;
        session.draft.store(u64::from(draft), Ordering::SeqCst);
        {
            let mut screen = session.scrollback.lock().unwrap();
            screen.bytes.extend(b"Ready\r\n");
            screen.written = screen.bytes.len() as u64;
        }
        let (writer, reader, pending) = InputTx::new(id);
        session.input_tx = Arc::new(writer);
        let (tx, input) = std::sync::mpsc::channel();
        let chat = id.to_string();
        std::thread::spawn(move || {
            for input in reader {
                pending.release(input.len());
                if let Input::Bytes(bytes) = input {
                    let submitted = bytes == b"\r";
                    let _ = tx.send(String::from_utf8_lossy(&bytes).into_owned());
                    if submitted {
                        let count = crate::activity::turn_state(&chat).unwrap().turns_ended;
                        crate::activity::count_at_least(&chat, count + 1);
                    }
                }
            }
        });
        manager().sessions.lock().unwrap().insert(id.to_string(), session);
        crate::activity::between_turns_for_test(id);
        crate::activity::count_at_least(id, 1);
        NoticeTerminal { id, quit, input }
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

    fn chat_context(provider: Provider, id: &str, account: &str, resume: &str) -> (serde_json::Value, serde_json::Value) {
        let pty = manager();
        let (mut session, quit) = fake_session_built(id, account, true, None, Box::new(|_, _, _| {}));
        session.provider = provider;
        session.resume = Some(resume.to_string());
        session.account_path = account.to_string();
        if provider == Provider::Codex {
            session.codex_compaction = crate::codex::compact::at_launch(id, account, account);
            assert!(session.codex_compaction.is_some());
        }
        let alive = Arc::clone(&session.alive);
        pty.sessions.lock().unwrap().insert(id.to_string(), session);
        crate::agents::register(id, provider, account, account, None, false, serde_json::json!({}));

        let get = || crate::agents::session_get(id, id, false, &["diagnostics".into()]).unwrap()["context"].clone();
        let context = get();
        let pane = serde_json::to_value(super::super::probe::session_meta(id, account.to_string())).unwrap();
        alive.store(false, Ordering::SeqCst);
        assert!(pty.compaction_launch(id, None).is_none());
        assert_eq!(get()["basis"], "model");

        quit.store(true, Ordering::SeqCst);
        pty.sessions.lock().unwrap().remove(id);
        crate::agents::forget(id);
        (context, pane)
    }

    #[test]
    fn a_claude_chat_keeps_its_compaction_basis_without_post_tokens() {
        let dir = scratch("claude-context");
        let account = dir.to_str().unwrap();
        let thread = "chat-context-claude";
        let path = crate::claude::session::transcript_file(account, account, thread);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(dir.join("settings.json"), r#"{"autoCompactWindow":195000}"#).unwrap();
        let registry = dir.join("sessions");
        std::fs::create_dir(&registry).unwrap();
        let pid = std::process::id();
        let entry = serde_json::json!({ "cwd": account, "sessionId": thread, "entrypoint": "cli", "pid": pid });
        std::fs::write(registry.join(format!("{pid}.json")), entry.to_string()).unwrap();
        std::fs::write(
            &path,
            concat!(
                r#"{"type":"assistant","message":{"model":"claude-sonnet-4-5","usage":{"input_tokens":150000}}}"#,
                "\n",
                r#"{"type":"system","subtype":"compact_boundary","compactMetadata":{"preTokens":150000}}"#,
                "\n",
            ),
        )
        .unwrap();

        let (context, pane) = chat_context(Provider::Claude, "chat-claude-context", account, thread);
        assert_eq!(context, serde_json::json!({ "pct": 93, "tokens": 150_000, "window": 162_000, "basis": "autoCompact" }));
        assert_eq!(pane["context"], 150_000.0 / 162_000.0);
        assert_eq!(pane["contextAutoCompact"], true);
        assert!(pane["contextToCompact"].is_null(), "the boundary has no current count");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_codex_chat_keeps_its_compaction_basis_when_usage_is_recomputed() {
        crate::codex::config::use_empty_machine_layers_for_test();
        let dir = scratch("codex-context");
        let account = dir.to_str().unwrap();
        let thread = "0192a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b";
        let day = dir.join("sessions/2026/09/17");
        std::fs::create_dir_all(&day).unwrap();
        std::fs::write(dir.join("config.toml"), "model_auto_compact_token_limit = 100000\n").unwrap();
        let path = day.join(format!("rollout-2026-09-17T14-03-22-{thread}.jsonl"));
        std::fs::write(&path, concat!(
            r#"{"type":"turn_context","payload":{"model":"gpt-6.1-sol"}}"#, "\n",
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":80000},"last_token_usage":{"total_tokens":80000},"model_context_window":200000}}}"#, "\n",
            r#"{"type":"turn_context","payload":{"model":"gpt-0-unanswered"}}"#, "\n",
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":80000},"last_token_usage":{"total_tokens":18000},"model_context_window":200000}}}"#, "\n",
        )).unwrap();

        let (context, pane) = chat_context(Provider::Codex, "chat-codex-context", account, thread);
        assert_eq!(context, serde_json::json!({ "pct": 18, "tokens": 18_000, "window": 100_000, "basis": "autoCompact" }));
        assert_eq!(pane["context"], 0.18);
        assert_eq!(pane["contextAutoCompact"], true);
        assert_eq!(pane["contextToCompact"], 82_000);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A start of `id` in `cwd` on another thread, captured: what it would
    /// run is built, and nothing runs.
    fn start(id: &'static str, cwd: String) -> std::thread::JoinHandle<Result<String, String>> {
        capture::expect(id);
        std::thread::spawn(move || {
            crate::runtime::block_on(spawn(id.into(), "acc", None, None, move || {
                Ok(Launch {
                    provider: Provider::Claude,
                    cwd,
                    account_path: "acc".into(),
                    args: Vec::new(),
                    envs: Vec::new(),
                    resume: None,
                    describe: String::new(),
                    applied: Vec::new(),
                    skills_unhidden: 0,
                    reserved: None,
                })
            }))
            .map(|a: Attached| a.backlog)
        })
    }

    /// A start of an id whose live session runs on another account folder
    /// is refused, not attached to: a login's id names its account, not the
    /// folder, which may since be another. On the same folder it attaches,
    /// and neither builds a launch.
    #[test]
    fn a_live_session_on_another_account_folder_is_not_attached_to() {
        let id = "attach-other-account";
        let dir = scratch("attach-other");
        let quit = fake_session(id, &dir.to_string_lossy(), PROMPT);
        {
            let mut sessions = manager().sessions.lock().unwrap_or_else(|e| e.into_inner());
            let session = sessions.get_mut(id).unwrap();
            session.provider = Provider::Codex;
            session.skills_unhidden = 12;
        }
        let ensure = |account: &'static str, account_id: Option<&str>| {
            capture::expect(id);
            crate::runtime::block_on(spawn(id.into(), account, account_id, None, || Err("a launch was built".into())))
                .map(|attached| attached.skills_unhidden)
        };
        let refused = ensure("elsewhere", None).unwrap_err();
        assert!(refused.contains("another account folder"), "{refused}");
        assert_eq!(ensure("acc", None), Ok(12));
        // A start bound to a folder's identity is held to the session's own,
        // whatever the paths say: the root may lead elsewhere since.
        manager().sessions.lock().unwrap_or_else(|e| e.into_inner()).get_mut(id).unwrap().account_dir = Some("dir-1".into());
        assert!(ensure("acc", Some("dir-2")).unwrap_err().contains("another account folder"));
        assert_eq!(ensure("acc", Some("dir-1")), Ok(12));
        quit.store(true, Ordering::SeqCst);

        // A session whose folder could not be read as it started: a start
        // naming an identity reads it again, once, and keeps what it read.
        let folder = dir.join("account");
        std::fs::create_dir_all(&folder).unwrap();
        let folder_s = folder.to_string_lossy().into_owned();
        let read = crate::accounts::folder_identity(&folder_s).expect("a folder of the test's");
        let quit = fake_session(id, &dir.to_string_lossy(), PROMPT);
        manager().sessions.lock().unwrap_or_else(|e| e.into_inner()).get_mut(id).unwrap().account_path = folder_s.clone();
        capture::expect(id);
        let again = crate::runtime::block_on(spawn(id.into(), &folder_s, Some(&read), None, || Err("a launch was built".into())));
        assert!(again.is_ok(), "{:?}", again.map(|_| ()));
        let kept = manager().sessions.lock().unwrap_or_else(|e| e.into_inner()).get(id).and_then(|s| s.account_dir.clone());
        assert_eq!(kept.as_deref(), Some(read.as_str()));
        quit.store(true, Ordering::SeqCst);
        let _ = capture::take(id);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An adopted orphan's first start that names the account folder's
    /// identity too: both hold. Another start of the id is refused as gone,
    /// by the map alone, before any folder is read; the start adopted on
    /// another identity is refused as on another folder; the start adopted on
    /// its own identity attaches, launching nothing.
    #[test]
    fn an_adopt_start_and_an_account_identity_both_hold() {
        let id = "adopt-and-account";
        let dir = scratch("adopt-account");
        let quit = fake_session(id, &dir.to_string_lossy(), PROMPT);
        let generation = {
            let mut sessions = manager().sessions.lock().unwrap_or_else(|e| e.into_inner());
            let s = sessions.get_mut(id).unwrap();
            s.account_dir = Some("dir-1".into());
            s.generation
        };
        let start = super::super::Start { generation, core: super::super::core_nonce().to_owned() };
        let other = super::super::Start { generation: generation + 1, ..start.clone() };
        let ensure = |expect: super::super::Start, account_id: Option<&str>| {
            capture::expect(id);
            crate::runtime::block_on(spawn(id.into(), "acc", account_id, Some(expect), || Err("a launch was built".into())))
        };
        assert_eq!(ensure(other, Some("dir-1")).map(|_| ()), Err(ADOPTED_GONE.to_owned()));
        let refused = ensure(start.clone(), Some("dir-2")).map(|_| ()).unwrap_err();
        assert!(refused.contains("another account folder"), "{refused}");
        assert_eq!(ensure(start, Some("dir-1")).map(|a| a.generation), Ok(generation));
        assert!(capture::take(id).is_none(), "nothing was launched");
        quit.store(true, Ordering::SeqCst);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A start naming the account folder's identity is held to the session's
    /// own, whatever the paths say; one naming none attaches on the same path,
    /// or on another path to a folder of the same identity.
    #[test]
    fn a_session_is_on_the_account_folder_its_identity_says() {
        use super::super::spawn::same_account;
        assert!(same_account("C:/a/x", Some("d1"), "C:/a/x", Some("d1"), None));
        assert!(!same_account("C:/a/x", Some("d1"), "C:/a/x", Some("d2"), Some("d1")), "the id named rules");
        assert!(!same_account("C:/a/x", None, "C:/a/x", Some("d1"), None), "a session of no known folder is none");
        assert!(same_account("C:/a/x", Some("d1"), "D:/b/x", None, Some("d1")), "another link to the same folder");
        assert!(!same_account("C:/a/x", Some("d1"), "D:/b/x", None, Some("d2")), "another path, another folder");
        assert!(same_account("C:/a/x", Some("d1"), "C:/a/x", None, Some("d2")), "the same path, as before");
        assert!(same_account("C:/a/x", None, "C:/a/x/", None, Some("d1")), "no identity of its own: the paths");
        assert!(!same_account("C:/a/x", Some("d1"), "C:/a/y", None, None), "none known now: the paths");
    }

    /// A live session its CLI has left nothing on disk for answers what its
    /// record says (`probe::session_meta`), not nothing, which the window
    /// shows as resting; with no record either it answers nothing.
    #[test]
    fn a_live_session_with_nothing_on_disk_answers_its_records_status() {
        let id = "sm-no-disk";
        let dir = scratch("sm-no-disk");
        let quit = fake_session(id, &dir.to_string_lossy(), PROMPT);
        // An account folder with nothing in it: no registry entry to read.
        let account = dir.join("account").to_string_lossy().into_owned();
        let look = || super::super::probe::session_meta(id, account.clone());
        assert!(look().is_none(), "no record and nothing on disk");
        crate::activity::not_known_for_test(id);
        let m = look().expect("the record's meta");
        assert_eq!((m.status.as_deref(), m.title.as_deref(), m.session_id.as_deref()), (Some("busy"), None, None), "not known yet");
        crate::activity::between_turns_for_test(id);
        assert_eq!(look().and_then(|m| m.status).as_deref(), Some("idle"), "a turn over with nothing beside it");
        quit.store(true, Ordering::SeqCst);
        crate::activity::forget(id);
        let _ = std::fs::remove_dir_all(&dir);
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
        let deleting = std::thread::spawn(move || manager().delete(id));
        taken(id);
        let respawn = start(id, cwd);
        // Let through, the start would be done by now.
        std::thread::sleep(Duration::from_millis(300));
        quit.store(true, Ordering::SeqCst);
        deleting.join().unwrap();
        let refused = respawn.join().unwrap().unwrap_err();
        assert!(refused.contains("deleted"), "{refused}");
        assert!(capture::take(id).is_none(), "nothing of a start was built");
        assert!(said::at(&format!("exit {id}")).is_some(), "the old session's exit was said");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A person's keys are activity as they come, before they land: an Enter
    /// after a power-off's fresh read moves the stamp that decides it. Only an
    /// Enter on text in the composer sends a prompt.
    #[test]
    fn a_persons_enter_moves_the_stamp_and_sends_a_draft() {
        let id = "keys-enter";
        let dir = scratch("keys-enter");
        let quit = fake_session(id, &dir.to_string_lossy(), PROMPT);
        assert!(!manager().sends_draft(id, "\r"), "an Enter on an empty composer: a dialog, a menu");
        let before = std::time::Instant::now();
        write_session(id.into(), "\r".into(), None).unwrap();
        assert!(crate::activity::last_activity() >= before, "the Enter moved the stamp");
        write_session(id.into(), "fix it".into(), None).unwrap();
        assert!(manager().sends_draft(id, "\r"));
        assert!(manager().sends_draft(id, " now\r"));
        assert!(!manager().sends_draft(id, "\x1b[200~a\rb"), "inside a paste: text");
        quit.store(true, Ordering::SeqCst);
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
        let killing = std::thread::spawn(move || crate::runtime::block_on(kill_session(id.into())));
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

    /// An orphan is listed, with its start and what the registry knows it was
    /// started with, until a chat has its id; the adopted pane's first start
    /// attaches to that start alone, launching nothing; and the orphan line's
    /// check and kill hold to that start, in this core. No process is
    /// signaled: the session's is the test's own stand-in, with no pid.
    #[test]
    fn an_orphan_is_adopted_by_attaching_and_killed_only_as_the_session_shown() {
        let id = "or-adopted";
        let dir = scratch("orphan");
        let cwd = dir.to_string_lossy().into_owned();
        let quit = fake_session(id, &cwd, PROMPT);
        crate::agents::register(
            id,
            Provider::Claude,
            &cwd,
            "acc",
            None,
            true,
            serde_json::json!({ "permissionMode": "plan", "readOnly": true }),
        );
        let scan = |known: Vec<String>| crate::runtime::block_on(orphan_sessions(known)).unwrap();
        let found = scan(Vec::new());
        let orphan = found.iter().find(|o| o.id == id).expect("listed while no chat has it");
        let start = orphan.start.clone();
        assert_eq!(start.core, super::super::core_nonce());
        assert_eq!((orphan.pid, orphan.cwd.as_str()), (None, cwd.as_str()));
        let modes = orphan.modes.as_ref().expect("the registry knows it");
        assert_eq!((modes.permission_mode.as_deref(), modes.read_only, modes.tools), (Some("plan"), true, true));
        assert!(scan(vec![id.into()]).iter().all(|o| o.id != id), "a chat's session is no orphan");
        // A sign-in runs in the account folder itself, and is no chat's.
        let login = "login:claude:or-acct";
        let signing_in = fake_session(login, &cwd, PROMPT);
        manager().sessions.lock().unwrap().get_mut(login).unwrap().account_path = cwd.clone();
        assert!(scan(Vec::new()).iter().all(|o| o.id != login), "a sign-in is no orphan");
        signing_in.store(true, Ordering::SeqCst);

        // Another start of the id, or the same number from another core, is
        // not the one shown.
        let other = super::super::Start { generation: start.generation + 1, ..start.clone() };
        let elsewhere = super::super::Start { core: "another-core".into(), ..start.clone() };
        assert!(orphan_live(id.into(), start.clone()));
        assert!(!orphan_live(id.into(), other.clone()) && !orphan_live(id.into(), elsewhere.clone()));
        assert!(!orphan_live("or-nobody".into(), start.clone()));

        // The adopted pane's first start: an attach to that start alone.
        capture::expect(id);
        let attach = |expect: super::super::Start| crate::runtime::block_on(spawn(id.into(), "acc", None, Some(expect), || unreachable!()));
        assert_eq!(attach(other.clone()).unwrap_err(), ADOPTED_GONE, "another start: nothing attached, nothing launched");
        assert_eq!(attach(start.clone()).map(|a| a.generation), Ok(start.generation));
        assert!(capture::take(id).is_none(), "nothing was launched");
        assert_eq!(said::at(&format!("launch {id}")), None);

        // Another start, or another core's: left running.
        for not_it in [other, elsewhere] {
            assert_eq!(crate::runtime::block_on(kill_orphan(id.into(), not_it)).unwrap_err(), NOT_THE_ORPHAN);
        }
        assert!(manager().sessions.lock().unwrap().contains_key(id) && !quit.load(Ordering::SeqCst));

        // The one shown is ended; asked again, there is nothing to kill, and
        // the adopted pane's first start finds nothing to attach to and
        // starts nothing.
        let shown = start.clone();
        let killing = std::thread::spawn(move || crate::runtime::block_on(kill_orphan(id.into(), shown)));
        taken(id);
        quit.store(true, Ordering::SeqCst);
        killing.join().unwrap().unwrap();
        assert!(!orphan_live(id.into(), start.clone()));
        assert_eq!(crate::runtime::block_on(kill_orphan(id.into(), start.clone())), Ok(()));
        capture::expect(id);
        assert_eq!(attach(start).unwrap_err(), ADOPTED_GONE);
        assert!(capture::take(id).is_none(), "nothing was launched");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An orphan's folder is where its CLI says it runs, a worktree it made
    /// for it included, of either CLI; with nothing said, where it started.
    #[test]
    fn an_orphan_runs_where_its_cli_says() {
        let probe = |provider| Probe {
            id: "or-wt".into(),
            provider,
            pid: None,
            cwd: "/repo".into(),
            spawned_at_ms: 0,
            codex_compaction: None,
            resume: None,
            account_path: "/acc".into(),
        };
        let meta = |cwd: &str| Some(super::super::SessionMeta { cwd: Some(cwd.into()), ..Default::default() });
        let claude = orphan_of(probe(Provider::Claude), 1, meta("/repo/.claude/worktrees/fix-1"), None);
        assert_eq!(claude.cwd, "/repo/.claude/worktrees/fix-1");
        let codex = orphan_of(probe(Provider::Codex), 2, meta("/acc/worktrees/1a2b/repo"), None);
        assert_eq!(codex.cwd, "/acc/worktrees/1a2b/repo");
        assert_eq!(orphan_of(probe(Provider::Codex), 3, None, None).cwd, "/repo");
        assert_eq!(orphan_of(probe(Provider::Claude), 4, meta(""), None).cwd, "/repo");
    }

    /// A CLI that took itself off and was reaped is past signaling: off
    /// Windows its pid may be another process's by then, so neither the tree
    /// walk nor the killer is sent to it. On Windows the session's handle
    /// keeps its pid its own, and the killer still goes.
    #[test]
    fn a_reaped_cli_is_not_signaled_by_its_pid() {
        #[derive(Debug)]
        struct Reaped {
            signaled: Arc<AtomicBool>,
        }
        impl portable_pty::ChildKiller for Reaped {
            fn kill(&mut self) -> std::io::Result<()> {
                self.signaled.store(true, Ordering::SeqCst);
                Ok(())
            }
            fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
                Box::new(Reaped { signaled: Arc::clone(&self.signaled) })
            }
        }
        impl portable_pty::Child for Reaped {
            fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
                Ok(Some(portable_pty::ExitStatus::with_exit_code(0)))
            }
            fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
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
        let dir = scratch("reaped");
        let (mut session, quit) = fake_session_built("or-reaped", &dir.to_string_lossy(), true, None, Box::new(|_, _, _| {}));
        let signaled = Arc::new(AtomicBool::new(false));
        let child = Reaped { signaled: Arc::clone(&signaled) };
        session.killer = portable_pty::ChildKiller::clone_killer(&child);
        session.child = Arc::new(Mutex::new(Box::new(child)));
        shut_down(session, Duration::from_millis(50));
        assert_eq!(signaled.load(Ordering::SeqCst), cfg!(windows));
        quit.store(true, Ordering::SeqCst);
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

    /// A Codex composer holding a line recalled from history (Up, Ctrl+R) is a
    /// draft though no key was typed in it; an empty one, its placeholder
    /// dim, is none. Claude Code's is asked only on the keys' word.
    #[test]
    fn a_draft_recalled_into_codex_s_composer_is_found_by_the_screen_with_no_typing() {
        let id = "dr-codex-recall";
        let quit = fake_session(id, "/nowhere", PROMPT);
        let show = |provider: Provider, composer: &str| {
            let mut sessions = manager().sessions.lock().unwrap_or_else(|e| e.into_inner());
            let s = sessions.get_mut(id).unwrap();
            s.provider = provider;
            // The screen as the Codex TUI draws it: the conversation, the
            // composer on its `›` row, the status line under it.
            let screen = format!(
                "\x1b[2J\x1b[2;1H\x1b[2m› \x1b[22mReply with just OK.\x1b[6;1H› {composer}\x1b[9;1H  GPT-6.1-Sol low · C:\\src\x1b[6;3H"
            );
            let mut scrollback = s.scrollback.lock().unwrap_or_else(|e| e.into_inner());
            scrollback.bytes.extend(screen.as_bytes());
            scrollback.written += screen.len() as u64;
            s.last_output_ms.fetch_add(1, Ordering::Relaxed);
        };
        // Nothing typed, an empty composer with its placeholder.
        show(Provider::Codex, "\x1b[2mAsk Codex to do anything\x1b[22m");
        assert!(!manager().has_draft(id));
        // Nothing typed, a recalled line in it: held for.
        show(Provider::Codex, "fix the build");
        assert!(manager().has_draft(id), "the screen says there is one");
        assert!(manager().draft_count(id) > 0, "and the count follows it");
        // Claude Code, count zero: the screen is not asked.
        manager().set_draft(id, 0);
        show(Provider::Claude, "fix the build");
        assert!(!manager().has_draft(id));
        quit.store(true, Ordering::SeqCst);
        manager().sessions.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
    }

    #[test]
    fn a_session_running_after_a_stop_leaves_the_chats_frozen_until_it_is_called_off() {
        let _alone = crate::agents::stops_for_test();
        stop_with(|| 0).unwrap();
        let quit = fake_session("fz-escaped", "/nowhere", PROMPT);
        assert!(manager().live_count() > 0);
        assert!(crate::agents::chats_frozen(), "frozen with a session running");
        update_called_off();
        assert!(!crate::agents::chats_frozen(), "lifted with the update called off");
        quit.store(true, Ordering::SeqCst);
        manager().sessions.lock().unwrap_or_else(|e| e.into_inner()).remove("fz-escaped");
    }

    /// An update's stop as `stop_all_sessions` runs it, over the test's
    /// pane only and with a short grace, the pane confirmed for it.
    fn update_stop_of(id: &'static str, launch_wait: Duration) -> usize {
        update_stop_confirmed(id, launch_wait, &[id.to_owned()]).unwrap()
    }

    /// `update_stop_of`, with what the person confirmed stopping.
    fn update_stop_confirmed(id: &'static str, launch_wait: Duration, confirmed: &[String]) -> Result<usize, String> {
        let look = || (manager().live_ids().into_iter().filter(|i| i == id).collect(), ());
        let since = crate::agents::update_call_offs();
        stopped_for_update(None, since, launch_wait, confirmed, look, |()| manager().shut_down_of(&|i| i == id, Duration::from_millis(500)))
    }

    /// An update's stop of nothing listed, asked now, running `stop`.
    fn stop_with(stop: impl FnOnce() -> usize) -> Result<usize, String> {
        stopped_for_update(None, crate::agents::update_call_offs(), Duration::ZERO, &[], || (Vec::new(), ()), |()| stop())
    }

    /// A pane as a launch has it before it registers, and the flag its
    /// CLI sets when asked to quit (not by the hard kill).
    fn a_pane(id: &str) -> (Session, Arc<AtomicBool>) {
        let asked = Arc::new(AtomicBool::new(false));
        let ending: Ending = Box::new(|id, generation, end| reader_exited(id, generation, Some(0), end));
        (fake_session_built(id, "/nowhere", true, Some(Arc::clone(&asked)), ending).0, asked)
    }

    /// A pane's launch under way as an update's stop begins (let in by the
    /// gate, not registered yet): the stop waits for it before its snapshot
    /// and stops it with the rest. A launch asked for once the stop has
    /// begun is refused before anything of it is built, both CLIs' alike
    /// (`spawn`). The gate stays closed past the stop, for the install.
    #[test]
    fn a_pane_launch_racing_the_update_stop_is_stopped_with_the_rest() {
        let _alone = crate::agents::stops_for_test();
        let (id, later) = ("sg-race", "sg-race-later");
        crate::starts::under_update_for_test(id);
        crate::starts::under_update_for_test(later);
        let launching = crate::starts::launching(id).expect("open");
        let (session, asked) = a_pane(id);
        std::thread::scope(|s| {
            let stop = s.spawn(|| update_stop_of(id, Duration::from_secs(30)));
            while crate::starts::open(id) {
                std::thread::yield_now();
            }
            capture::expect(later);
            let refused =
                crate::runtime::block_on(spawn(later.into(), "acc", None, None, || panic!("nothing of a refused launch is built")));
            assert!(refused.err().unwrap().starts_with(crate::starts::UPDATING));
            assert!(capture::take(later).is_none(), "no launch was built");
            std::thread::sleep(Duration::from_millis(50));
            assert!(!stop.is_finished(), "the stop waits for the launch under way");
            assert_eq!(manager().register(id, &launching, session), Ok(()));
            drop(launching);
            assert_eq!(stop.join().unwrap(), 1, "the late pane was in the snapshot");
        });
        assert!(asked.load(Ordering::SeqCst), "and was asked to quit, not killed");
        assert!(!manager().sessions.lock().unwrap_or_else(|e| e.into_inner()).contains_key(id));
        assert!(!crate::starts::open(id), "closed until the install or a call-off");
        update_called_off();
        assert!(crate::starts::open(id), "a call-off opens it");
    }

    /// A launch that outlasts the stop's wait is a CLI started that would
    /// register as a session nobody was asked about, whose id the stop
    /// cannot know yet: the stop is refused, whatever was confirmed, the
    /// update called off and the gate open again, and the pane registers
    /// and runs on as it would have, not asked to quit.
    #[test]
    fn a_pane_launch_that_outlasts_the_update_stops_wait_refuses_the_stop() {
        let _alone = crate::agents::stops_for_test();
        let id = "sg-outlasts";
        crate::starts::under_update_for_test(id);
        let launching = crate::starts::launching(id).expect("open");
        let (session, asked) = a_pane(id);
        assert_eq!(update_stop_confirmed(id, Duration::from_millis(10), &[id.to_owned()]), Err(UNCONFIRMED.to_owned()));
        assert!(crate::starts::open(id), "the gate is open again");
        assert!(!crate::agents::chats_frozen() && !crate::agents::stopped_for_update(), "the update is off");
        assert_eq!(manager().register(id, &launching, session), Ok(()));
        drop(launching);
        assert!(!asked.load(Ordering::SeqCst), "not asked to quit");
        assert!(manager().alive(id), "it runs on");
        manager().shut_down_of(&|i| i == id, Duration::from_millis(500));
    }

    /// The panel said nothing stops (none ran, or the sessions keep
    /// running), and a pane's launch let in before the gate closed registers
    /// as the stop seals the launches: the stop sees it and refuses, having
    /// stopped nothing, and the update is called off, the gate open again.
    /// Confirmed, the same pane is stopped (`a_pane_launch_racing_...`).
    #[test]
    fn a_stop_refuses_a_session_its_confirmation_did_not_count() {
        let _alone = crate::agents::stops_for_test();
        let id = "sg-unconfirmed";
        crate::starts::under_update_for_test(id);
        let launching = crate::starts::launching(id).expect("open");
        let (session, asked) = a_pane(id);
        std::thread::scope(|s| {
            let stop = s.spawn(|| update_stop_confirmed(id, Duration::from_secs(30), &["sg-other".to_owned()]));
            while crate::starts::open(id) {
                std::thread::yield_now();
            }
            std::thread::sleep(Duration::from_millis(50));
            assert!(!stop.is_finished(), "the stop waits for the launch under way");
            assert_eq!(manager().register(id, &launching, session), Ok(()));
            drop(launching);
            assert_eq!(stop.join().unwrap(), Err(UNCONFIRMED.to_owned()));
        });
        assert!(!asked.load(Ordering::SeqCst), "nothing was stopped");
        assert!(manager().alive(id), "the pane runs on");
        assert!(crate::starts::open(id), "the gate is open again");
        assert!(!crate::agents::chats_frozen() && !crate::agents::stopped_for_update(), "the update is off");
        manager().shut_down_of(&|i| i == id, Duration::from_millis(500));
    }

    /// A stop that panics opens the gate and lifts the freeze at once: no
    /// update follows a stop that failed, and nothing would start until a
    /// window of this build came again.
    #[test]
    fn a_stop_that_panics_opens_the_gate_again() {
        let _alone = crate::agents::stops_for_test();
        let id = "sg-panics";
        crate::starts::under_update_for_test(id);
        let failed = std::panic::catch_unwind(|| {
            stop_with(|| {
                assert!(!crate::starts::open(id), "closed while it stops");
                panic!("the stop failed")
            })
        });
        assert!(failed.is_err());
        assert!(crate::starts::open(id));
        assert!(crate::starts::launching(id).is_ok());
        assert!(!crate::agents::chats_frozen());
        assert!(!crate::agents::stopped_for_update());
    }

    /// The window's word that the update is off, said while the sessions
    /// are still being stopped for it (by a page script, say: the window's
    /// install starts only once the stop has returned), leaves the chats'
    /// ties frozen until the stop returns, and lifts the freeze then.
    #[test]
    fn an_update_called_off_while_its_sessions_are_stopped_waits_for_the_stop() {
        let _alone = crate::agents::stops_for_test();
        let stopped = stop_with(|| {
            update_called_off();
            assert!(crate::agents::chats_frozen(), "the freeze holds while they are stopped");
            3
        });
        assert_eq!(stopped, Ok(3));
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
        let killing = std::thread::spawn(move || crate::runtime::block_on(kill_session(id.into())));
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
            (s.typed_ms.load(Ordering::Relaxed), s.draft.load(Ordering::Relaxed))
        };
        for data in ["\x1b]11;rgb:ffff/ffff/ffff\x1b\\", "\x1b[8;30;100t", "\x1b[?62;22c"] {
            let sent = crate::corecmd::dispatch("write_session", serde_json::json!({ "id": id, "data": data, "reply": true }));
            assert!(sent.is_ok(), "{data:?}: {sent:?}");
            assert_eq!(counts(), (0, 0), "{data:?}");
        }
        // Without it, as the page sends everything: the color's digits count
        // as a draft, the size report as typing.
        for data in ["\x1b]11;rgb:ffff/ffff/ffff\x1b\\", "\x1b[8;30;100t"] {
            crate::corecmd::dispatch("write_session", serde_json::json!({ "id": id, "data": data })).unwrap();
        }
        let (typed, draft) = counts();
        assert!(typed > 0 && draft > 0, "{:?}", counts());
        quit.store(true, Ordering::SeqCst);
        manager().sessions.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
    }

    /// What alacritty_terminal answers, and the focus reports, are taken
    /// for answers; keys, a paste, a query, an answer with more after it or
    /// cut short, another OSC, and anything past the bound are not.
    #[test]
    fn only_a_terminal_answer_whole_is_taken_for_one() {
        let long = format!("\x1b[{}t", "1;".repeat(40));
        for yes in [
            "\x1b[?6c",
            "\x1b[>0;2600;1c",
            "\x1b[0n",
            "\x1b[12;40R",
            "\x1b[?2004;1$y",
            "\x1b[4;2$y",
            "\x1b[?0u",
            "\x1b[4;600;800t",
            "\x1b[8;30;100t",
            "\x1b]11;rgb:ffff/ffff/ffff\x1b\\",
            "\x1b]4;1;rgb:cdcd/0000/0000\x07",
            "\x1b[I",
            "\x1b[O",
            "\x1bP1$r0m\x1b\\",
        ] {
            assert!(terminal_reply(yes), "{yes:?}");
        }
        for no in [
            "",
            "hello",
            "\x1b",
            "\x1b[A",
            "\x1b[1;5D",
            "\x1b[15~",
            "\x1b[200~x\x1b[201~",
            "\x1b[<0;3;4M",
            "\x1b[13u",
            "\x1b[c",
            "\x1b[$y",
            "\x1b[?6cx",
            "x\x1b[?6c",
            "\x1b[?6c\x1b[?6c",
            "\x1b[\u{e9}",
            "\x1b]11;rgb:ffff/ffff/ffff",
            "\x1b]11;rgb:ff ff\x07",
            "\x1b]0;title\x07",
            "\x1b]52;c;aGk=\x07",
            "\x1bPq#0;2;0;0;0\x1b\\",
            long.as_str(),
        ] {
            assert!(!terminal_reply(no), "{no:?}");
        }
    }

    /// Input labeled a reply that is no terminal answer takes the keys'
    /// path: held while Luna holds the keys, and counted as typing and a
    /// draft once let through; a real answer still goes past the hold,
    /// counted as nothing.
    #[test]
    fn input_labeled_a_reply_is_held_and_counted_as_keys() {
        let id = "dl-not-a-reply";
        let quit = fake_session(id, "/nowhere", PROMPT);
        let write = |data: &str| crate::corecmd::dispatch("write_session", serde_json::json!({ "id": id, "data": data, "reply": true }));
        let counts = || {
            let sessions = manager().sessions.lock().unwrap_or_else(|e| e.into_inner());
            let s = &sessions[id];
            (s.typed_ms.load(Ordering::Relaxed), s.draft.load(Ordering::Relaxed))
        };
        let held = || manager().sessions.lock().unwrap_or_else(|e| e.into_inner())[id].held_keys.lock().unwrap().clone();
        let hold = manager().hold_keys(id).expect("a hold");
        write("abc").unwrap();
        write("\x1b[?6c").unwrap();
        assert_eq!(held(), Some(vec!["abc".to_string()]), "the answer went past the hold, the text did not");
        assert_eq!(counts(), (0, 0));
        drop(hold);
        assert_eq!(held(), None);
        let (typed, draft) = counts();
        assert!(typed > 0 && draft > 0, "{:?}", counts());
        quit.store(true, Ordering::SeqCst);
        manager().sessions.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
    }
}
