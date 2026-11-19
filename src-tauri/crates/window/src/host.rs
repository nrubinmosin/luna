//! The window's one way out and one way in. Out: the host's commands, by the
//! names and with the camelCase arguments the React page uses today
//! (`src/ipc/*.ts`), so the window asks for what the page asks for. In: the
//! events the host sends, `(name, payload)` as the page hears them, and work
//! the host wants done on the window's thread. The crate knows nothing of what
//! answers: the app hosts it (luna_lib's native.rs), a fake does in the tests
//! and the shots (demo.rs).
//!
//! A session's output is the one thing that does not come in through the
//! inbox: the host hands it to the session's `OutputSink`, off the window's
//! thread.

use futures::channel::mpsc;
use futures::{FutureExt, StreamExt};
use serde_json::Value;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

/// A command's answer, or why there is none.
pub type Answer = Pin<Box<dyn Future<Output = Result<Value, String>> + Send>>;

/// What the window asks of the app: its commands, and the desktop's
/// services (desktop.rs).
pub trait Host: crate::desktop::Desktop + Send + Sync + 'static {
    /// A command answered off the window's thread: the future resolves when
    /// the answer is there, and the window waits for it on a task, never on
    /// its thread.
    fn call(&self, cmd: &str, args: Value) -> Answer;

    /// Keystrokes, resizes and the window's own settings: handed over on the
    /// window's thread in the order they come, never answered. An error says
    /// the command could not be handed over at all.
    fn send(&self, cmd: &str, args: Value) -> Result<(), String>;

    /// Hands `session`'s output (`event::PTY_OUTPUT`) to `sink` from now on,
    /// in place of any sink before; None lets it go.
    fn route_output(&self, session: &str, sink: Option<Arc<dyn OutputSink>>);
}

/// Where a session's output goes: a terminal pane's parser, on a thread of
/// the pane's own. A session can write megabytes a second, and the window's
/// thread is for input and frames: the host hands each chunk straight from
/// the thread that reads it to the sink, and the pane has the window draw once
/// it has parsed (a `Mailbox::run_on_ui`, or a notify of its entity).
pub trait OutputSink: Send + Sync + 'static {
    /// The session's output, in the order it came, and where its text starts
    /// in all the session has sent (`pty://output`'s `offset`, UTF-8 bytes),
    /// when the core says. False once the pane is gone: the host lets go of
    /// the sink.
    fn bytes(&self, data: &[u8], offset: Option<u64>) -> bool;
}

/// The commands the window names.
pub mod cmd {
    /// `CoreStateDto` of the window's line to the core.
    pub const CORE_STATE: &str = "core_state";
    /// Stops the running core and starts one of this build: `{ revision }`,
    /// that of the state the window offered it on (`CoreState`). The host does
    /// nothing once the state has moved on from it.
    pub const CORE_RESTART: &str = "core_restart";
    /// Keeps the theme the person chose for the next start: `{ theme }`.
    /// Sent, so the choices reach the host in the order they were made.
    pub const SET_WINDOW_THEME: &str = "set_window_theme";
    /// Starts a chat's CLI, or reattaches to it if it runs: the page's
    /// arguments (src/ipc/sessions.ts), answered with `{ backlog, generation }`.
    pub const ENSURE_CLAUDE_SESSION: &str = "ensure_claude_session";
    pub const ENSURE_CODEX_SESSION: &str = "ensure_codex_session";
    /// The escapes that put back the terminal modes a session's CLI set:
    /// `{ id }`, answered with a string.
    pub const SESSION_MODES: &str = "session_modes";
    /// Input for a session: `{ id, data }`.
    pub const WRITE_SESSION: &str = "write_session";
    /// A session's PTY size: `{ id, cols, rows, pixelWidth, pixelHeight }`,
    /// the pixels those of the whole grid.
    pub const RESIZE_SESSION: &str = "resize_session";
    /// The window's own state as the host keeps it, and on the first start
    /// what 0.19's window kept: `{ saved, old, oldError }`, `saved` the last
    /// `save_window_state` text read back, or null when there is none; then
    /// `old` each key of the WebView's Local Storage as its text, or
    /// `oldError` why it could not be read (both null when there was no such
    /// store at all).
    pub const WINDOW_STATE: &str = "window_state";
    /// Keeps the window's state: `{ text }`, the whole of it, which replaces
    /// what was kept. Sent, so the host has the states in the order they
    /// were made; it writes the latest and lets an older one go.
    pub const SAVE_WINDOW_STATE: &str = "save_window_state";
    /// The accounts Luna knows: `[{ provider, name, path }]`.
    pub const LIST_ACCOUNTS: &str = "list_accounts";
    /// Every chat's session as the core sees it: `{ chats: [{ id,
    /// accountPath }] }`, answered `[{ id, meta }]` (`SessionMetaDto`).
    pub const SESSIONS_META: &str = "sessions_meta";
    /// The CLI's title for a session not running: `{ provider, accountPath,
    /// sessionId }`, answered `{ title, renamed }` or null.
    pub const SAVED_TITLE: &str = "saved_title";

    /// Called, each one the host must answer (a test in luna_lib holds its
    /// dispatch to this list).
    pub const ALL: &[&str] = &[
        CORE_STATE,
        CORE_RESTART,
        ENSURE_CLAUDE_SESSION,
        ENSURE_CODEX_SESSION,
        SESSION_MODES,
        WINDOW_STATE,
        LIST_ACCOUNTS,
        SESSIONS_META,
        SAVED_TITLE,
    ];
    /// Sent, each one the host must take (held so by the same test).
    pub const SENT: &[&str] = &[SET_WINDOW_THEME, WRITE_SESSION, RESIZE_SESSION, SAVE_WINDOW_STATE];
}

/// The events the window listens to.
pub mod event {
    /// The line to the core changed: a `CoreStateDto` with its `revision`.
    pub const CORE_STATE: &str = "core://state";
    /// A session's output: `{ id, data }`. Never in the inbox: the host hands
    /// it to the session's `OutputSink`, when one is routed (`Host::route_output`).
    pub const PTY_OUTPUT: &str = "pty://output";
    /// A session's exit: `{ id, code, generation }`, the spawn's number as
    /// its start answered it.
    pub const PTY_EXIT: &str = "pty://exit";
    /// A session refused input: `{ id, waitingKb }`, once a stretch.
    pub const PTY_INPUT_REFUSED: &str = "pty://input-refused";
    /// The core closed every session for a shutdown of the PC that was then
    /// taken back (power/machine.rs): `{ why }`. Every pane starts its over.
    pub const START_OVER: &str = "pty://start-over";
    /// A desktop notification the core asks for: `{ title, body, chatId }`
    /// (power/machine.rs), shown as the page showed it.
    pub const NOTIFY: &str = "ui://notify";

    /// Every event the inbox takes; any other is dropped before it is queued
    /// (`wants`). A view that listens to a new event adds it here.
    pub const ALL: &[&str] = &[CORE_STATE, PTY_EXIT, PTY_INPUT_REFUSED, START_OVER, NOTIFY];
    /// Of those, the ones where only the latest counts: one waits in the
    /// inbox at most, a newer replacing it, and where the payload carries a
    /// `revision`, an older one never replacing a newer (`Mailbox::event`).
    pub const LATEST: &[&str] = &[CORE_STATE];

    /// The inbox takes `name`: one of these, or of the desktop's.
    pub fn wants(name: &str) -> bool {
        ALL.contains(&name) || crate::desktop::event::ALL.contains(&name)
    }
}

/// What comes into the window.
pub(crate) enum Message {
    Event {
        name: String,
        payload: Value,
    },
    /// The latest payload of an `event::LATEST` event waits in `Latest`.
    Latest(&'static str),
    Run(Box<dyn FnOnce() + Send>),
    Request(crate::desktop::Request),
}

/// The latest payload of each `event::LATEST` event not taken yet.
type Latest = Arc<Mutex<HashMap<&'static str, Value>>>;

/// The host's end of the window's inbox: cheap to clone, usable from any
/// thread. Each send says whether the window is still there to take it.
#[derive(Clone)]
pub struct Mailbox {
    tx: mpsc::UnboundedSender<Message>,
    latest: Latest,
}

impl Mailbox {
    /// An event for the window, as the page would hear it. One the window
    /// does not take (`event::wants`) is dropped here; one of `event::LATEST`
    /// replaces the one still waiting, unless that one's `revision` is newer.
    pub fn event(&self, name: &str, payload: Value) -> bool {
        if !event::wants(name) {
            return !self.tx.is_closed();
        }
        let Some(&latest) = event::LATEST.iter().find(|&&e| e == name) else {
            return self.tx.unbounded_send(Message::Event { name: name.to_owned(), payload }).is_ok();
        };
        let mut waiting = self.latest.lock().unwrap_or_else(|e| e.into_inner());
        match waiting.get_mut(latest) {
            Some(older) => {
                if revision(older) <= revision(&payload) {
                    *older = payload;
                }
                !self.tx.is_closed()
            }
            // Nothing waiting: the payload, and a note in the queue where it
            // takes its turn. Both under the lock, so the pump cannot find the
            // note before the payload is there.
            None => {
                waiting.insert(latest, payload);
                self.tx.unbounded_send(Message::Latest(latest)).is_ok()
            }
        }
    }

    /// Asks the window to come up or the app to quit (desktop.rs), after
    /// whatever came in before it.
    pub fn request(&self, request: crate::desktop::Request) -> bool {
        self.tx.unbounded_send(Message::Request(request)).is_ok()
    }

    /// Runs `f` on the window's thread, after whatever came in before it.
    pub fn run_on_ui(&self, f: impl FnOnce() + Send + 'static) -> bool {
        self.tx.unbounded_send(Message::Run(Box::new(f))).is_ok()
    }
}

/// A payload's `revision`; one without any is as old as can be.
fn revision(payload: &Value) -> Option<u64> {
    payload.get("revision").and_then(Value::as_u64)
}

/// The window's end, drained by one task on its thread (lib.rs `pump`).
pub struct Inbox {
    rx: mpsc::UnboundedReceiver<Message>,
    latest: Latest,
}

impl Inbox {
    /// Waits for a message, then takes those already behind it, `max` in all
    /// at most, so that one turn of the window's thread does a bounded amount
    /// of work. None once every mailbox is gone and everything was taken.
    pub(crate) async fn next_batch(&mut self, max: usize) -> Option<Vec<Message>> {
        let first = self.rx.next().await?;
        let mut batch = Vec::new();
        self.take(first, &mut batch);
        while batch.len() < max {
            match self.rx.next().now_or_never() {
                Some(Some(message)) => self.take(message, &mut batch),
                _ => break,
            }
        }
        Some(batch)
    }

    fn take(&self, message: Message, batch: &mut Vec<Message>) {
        match message {
            Message::Latest(name) => {
                if let Some(payload) = self.latest.lock().unwrap_or_else(|e| e.into_inner()).remove(name) {
                    batch.push(Message::Event { name: name.to_owned(), payload });
                }
            }
            message => batch.push(message),
        }
    }
}

/// A new inbox and the mailbox that feeds it. Unbounded, but little can wait
/// in it: only the events the window takes come in (`event::ALL`), the core's
/// state one at a time (`event::LATEST`), a session's exit and refusals, the
/// core's rare start-over, and the host's own work (the heartbeat's ping). A
/// session's output never does (`OutputSink`).
pub fn mailbox() -> (Mailbox, Inbox) {
    let (tx, rx) = mpsc::unbounded();
    let latest = Latest::default();
    (Mailbox { tx, latest: latest.clone() }, Inbox { rx, latest })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn events(batch: Vec<Message>) -> Vec<(String, Value)> {
        batch
            .into_iter()
            .map(|m| match m {
                Message::Event { name, payload } => (name, payload),
                Message::Latest(_) => panic!("a note is never handed out"),
                Message::Run(_) => ("run".into(), Value::Null),
                Message::Request(_) => ("request".into(), Value::Null),
            })
            .collect()
    }

    #[test]
    fn only_the_events_the_window_takes_are_queued() {
        let (mailbox, mut inbox) = mailbox();
        for i in 0..10_000 {
            assert!(mailbox.event(event::PTY_OUTPUT, json!({ "id": "a", "data": i.to_string() })));
            assert!(mailbox.event("agents://changed", json!({})));
        }
        assert!(mailbox.run_on_ui(|| {}));
        let batch = futures::executor::block_on(inbox.next_batch(64)).unwrap();
        assert_eq!(events(batch), vec![("run".into(), Value::Null)]);
    }

    #[test]
    fn the_core_state_waits_once_and_the_newest_revision_wins() {
        let (mailbox, mut inbox) = mailbox();
        let at = |state: &str, revision: u64| json!({ "state": state, "revision": revision });
        mailbox.event(event::CORE_STATE, at("starting", 1));
        mailbox.run_on_ui(|| {});
        mailbox.event(event::CORE_STATE, at("up", 3));
        // Late: an older state sent after a newer one.
        mailbox.event(event::CORE_STATE, at("down", 2));
        let got = events(futures::executor::block_on(inbox.next_batch(64)).unwrap());
        // In the place of the first, with the newest's payload.
        assert_eq!(got, vec![(event::CORE_STATE.into(), at("up", 3)), ("run".into(), Value::Null)]);
        // Taken: the next one queues anew.
        mailbox.event(event::CORE_STATE, at("down", 4));
        let got = events(futures::executor::block_on(inbox.next_batch(64)).unwrap());
        assert_eq!(got, vec![(event::CORE_STATE.into(), at("down", 4))]);
    }

    #[test]
    fn a_batch_is_bounded_and_the_rest_waits_in_order() {
        let (mailbox, mut inbox) = mailbox();
        let seen = Arc::new(Mutex::new(Vec::new()));
        for i in 0..150 {
            let seen = seen.clone();
            mailbox.run_on_ui(move || seen.lock().unwrap().push(i));
        }
        let mut sizes = Vec::new();
        for _ in 0..3 {
            let batch = futures::executor::block_on(inbox.next_batch(64)).unwrap();
            sizes.push(batch.len());
            for m in batch {
                if let Message::Run(f) = m {
                    f()
                }
            }
        }
        assert_eq!(sizes, vec![64, 64, 22]);
        assert_eq!(*seen.lock().unwrap(), (0..150).collect::<Vec<_>>());
    }

    #[test]
    fn the_inbox_ends_with_the_last_mailbox() {
        let (mailbox, mut inbox) = mailbox();
        mailbox.run_on_ui(|| {});
        drop(mailbox);
        assert_eq!(futures::executor::block_on(inbox.next_batch(64)).map(|b| b.len()), Some(1));
        assert!(futures::executor::block_on(inbox.next_batch(64)).is_none());
    }
}
