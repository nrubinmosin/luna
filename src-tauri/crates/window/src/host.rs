//! The window's one way out and one way in. Out: the host's commands, by the
//! names and with the camelCase arguments the React page uses today
//! (`src/ipc/*.ts`), so the window asks for what the page asks for. In: the
//! events the host sends, `(name, payload)` as the page hears them, and work
//! the host wants done on the window's thread. The crate knows nothing of what
//! answers: the app hosts it (luna_lib's native.rs), a fake does in the tests
//! and the shots (demo.rs).

use futures::channel::mpsc;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

/// A command's answer, or why there is none.
pub type Answer = Pin<Box<dyn Future<Output = Result<Value, String>> + Send>>;

/// What the window asks of the app.
pub trait Host: Send + Sync + 'static {
    /// A command answered off the window's thread: the future resolves when
    /// the answer is there, and the window waits for it on a task, never on
    /// its thread.
    fn call(&self, cmd: &str, args: Value) -> Answer;

    /// Keystrokes and resizes: handed over in the order they come, never
    /// answered. An error says the command could not be handed over at all.
    fn send(&self, cmd: &str, args: Value) -> Result<(), String>;
}

/// The commands the window names, each one the host must answer (a test in
/// luna_lib holds its dispatch to this list).
pub mod cmd {
    /// `CoreStateDto` of the window's line to the core.
    pub const CORE_STATE: &str = "core_state";
    /// Stops the running core and starts one of this build.
    pub const CORE_RESTART: &str = "core_restart";
    /// Keeps the theme the person chose for the next start: `{ theme }`.
    pub const SET_WINDOW_THEME: &str = "set_window_theme";

    pub const ALL: &[&str] = &[CORE_STATE, CORE_RESTART, SET_WINDOW_THEME];
}

/// The events the window listens to.
pub mod event {
    /// The line to the core changed: a `CoreStateDto`.
    pub const CORE_STATE: &str = "core://state";

    pub const ALL: &[&str] = &[CORE_STATE];
}

/// What comes into the window.
pub(crate) enum Message {
    Event { name: String, payload: Value },
    Run(Box<dyn FnOnce() + Send>),
}

/// The host's end of the window's inbox: cheap to clone, usable from any
/// thread. Each send says whether the window is still there to take it.
#[derive(Clone)]
pub struct Mailbox(mpsc::UnboundedSender<Message>);

impl Mailbox {
    /// An event for the window, as the page would hear it.
    pub fn event(&self, name: &str, payload: Value) -> bool {
        self.0.unbounded_send(Message::Event { name: name.to_owned(), payload }).is_ok()
    }

    /// Runs `f` on the window's thread, after whatever came in before it.
    pub fn run_on_ui(&self, f: impl FnOnce() + Send + 'static) -> bool {
        self.0.unbounded_send(Message::Run(Box::new(f))).is_ok()
    }
}

/// The window's end, drained by one task on its thread (lib.rs `pump`).
pub struct Inbox(pub(crate) mpsc::UnboundedReceiver<Message>);

/// A new inbox and the mailbox that feeds it. Unbounded: what comes in is the
/// core's events, which the core itself bounds per window (it lets a window
/// that falls behind go), and the window drains it as fast as it comes.
pub fn mailbox() -> (Mailbox, Inbox) {
    let (tx, rx) = mpsc::unbounded();
    (Mailbox(tx), Inbox(rx))
}
