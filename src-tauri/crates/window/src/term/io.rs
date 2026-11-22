//! The pane's way into its session: keys and pastes (`write_session`) and the
//! PTY's size (`resize_session`), sent on the window's thread in the order
//! they are made. Only while the session runs, as the page's `ptySender`
//! gates them: before its start answers, or after it exited, there is no
//! session to take them, and a write would come back as "no such session".
//! The size is kept meanwhile and sent once it runs. Input (keys, pastes,
//! the mouse, focus) waits on a gate of its own besides (`set_typing`): a
//! reattach takes none until the session's modes are parsed, while the
//! replies to queries in what it replays already go.
//!
//! A size the host refused to hand over (the core's line was full for a
//! moment) is not taken for sent: the pane is asked to try again
//! (`Io::new`'s `retry`), so a size is never left stuck at one the PTY never got.
//!
//! While a divider is dragged (`hold`), the size goes at most once every
//! `HELD_GAP`, a refused one tried again included, and the last one as the
//! divider is let go: every size makes the CLI paint its whole screen again
//! (0.19's sizing.ts did the same).

use crate::host::{Host, cmd};
use futures::channel::mpsc::UnboundedSender;
use serde_json::json;
use std::cell::Cell;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How often a size goes at most while the pane's size is held.
pub const HELD_GAP: Duration = Duration::from_millis(80);

/// A PTY's size: cells, and the grid in device pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PtySize {
    pub cols: u16,
    pub rows: u16,
    pub width: u16,
    pub height: u16,
}

pub struct Io {
    host: Arc<dyn Host>,
    id: String,
    live: Cell<bool>,
    /// Input goes, while live (`set_typing`).
    typing: Cell<bool>,
    /// The size the pane last laid out at, and the last one the host took.
    size: Cell<Option<PtySize>>,
    sent: Cell<Option<PtySize>>,
    /// Told when a size was refused: the pane calls `flush_size` a moment later.
    retry: UnboundedSender<()>,
    /// A divider is being dragged (`hold`), and when a size last went since.
    held: Cell<bool>,
    held_sent: Cell<Option<Instant>>,
}

impl Io {
    pub fn new(host: Arc<dyn Host>, id: String, retry: UnboundedSender<()>) -> Io {
        Io {
            host,
            id,
            live: Cell::new(false),
            typing: Cell::new(false),
            size: Cell::new(None),
            sent: Cell::new(None),
            retry,
            held: Cell::new(false),
            held_sent: Cell::new(None),
        }
    }

    /// The session runs (or no longer does). Coming alive sends the size the
    /// pane is at, unless that was sent already; it takes no input until
    /// `set_typing`.
    pub fn set_live(&self, live: bool) {
        self.live.set(live);
        self.typing.set(false);
        if live {
            self.flush_size();
        } else {
            // A session started again is told its size afresh.
            self.sent.set(None);
        }
    }

    /// The session, live, takes input (or no longer does): its modes, which
    /// keys and pastes are encoded by, are known.
    pub fn set_typing(&self, on: bool) {
        self.typing.set(on && self.live.get());
    }

    /// Whether input goes now: anything typed meanwhile is dropped, not
    /// encoded against modes not known yet.
    pub fn takes_input(&self) -> bool {
        self.live.get() && self.typing.get()
    }

    /// Input for the session: dropped while it takes none.
    pub fn write(&self, data: &str) {
        if self.takes_input() {
            self.write_as(data, false);
        }
    }

    /// A reply to the program's query (a size, a color, the device's
    /// attributes): sent with `reply`, so the core does not count it as a
    /// person typing (pty/control.rs `write_session`); while the session
    /// runs, input or not.
    pub fn reply(&self, data: &str) {
        self.write_as(data, true);
    }

    fn write_as(&self, data: &str, reply: bool) {
        if !self.live.get() || data.is_empty() {
            return;
        }
        let args = if reply { json!({ "id": self.id, "data": data, "reply": true }) } else { json!({ "id": self.id, "data": data }) };
        if let Err(e) = self.host.send(cmd::WRITE_SESSION, args) {
            log::warn!("write_session: {e}");
        }
    }

    /// The pane laid out at `size` at `now` (the executor's clock): sent when
    /// it differs from the last sent, and while held, no sooner than
    /// `HELD_GAP` after the last.
    pub fn resize(&self, size: PtySize, now: Instant) {
        self.size.set(Some(size));
        if self.sent.get() != Some(size) {
            self.flush_due(now);
        }
    }

    /// A size the host refused, tried again at `now`: while held, as a new
    /// size is, no sooner than `HELD_GAP` after the last; one held back then
    /// goes with the next frame's or as the divider is let go.
    pub fn retry_size(&self, now: Instant) {
        self.flush_due(now);
    }

    fn flush_due(&self, now: Instant) {
        if self.held.get() {
            if self.held_sent.get().is_some_and(|at| now.duration_since(at) < HELD_GAP) {
                return;
            }
            self.held_sent.set(Some(now));
        }
        self.flush_size();
    }

    /// A divider is dragged (or let go of): letting go sends the size the
    /// pane is at now.
    pub fn hold(&self, on: bool) {
        if self.held.replace(on) && !on {
            self.held_sent.set(None);
            self.flush_size();
        }
    }

    pub fn size(&self) -> Option<PtySize> {
        self.size.get()
    }

    /// The size the pane is at, unless the host has it already.
    pub fn flush_size(&self) {
        let Some(size) = self.size.get() else { return };
        if self.live.get() && self.sent.get() != Some(size) {
            self.send_size(size);
        }
    }

    /// `size` sent as it is, whatever was sent before: the repaint nudge's
    /// row less and back.
    pub fn send_size(&self, size: PtySize) {
        if !self.live.get() || size.cols < 1 || size.rows < 1 {
            return;
        }
        let args = json!({
            "id": self.id,
            "cols": size.cols,
            "rows": size.rows,
            "pixelWidth": size.width,
            "pixelHeight": size.height,
        });
        match self.host.send(cmd::RESIZE_SESSION, args) {
            Ok(()) => self.sent.set(Some(size)),
            Err(e) => {
                log::warn!("resize_session: {e}");
                // Whatever went before, the host does not have this one.
                self.sent.set(None);
                let _ = self.retry.unbounded_send(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::FakeHost;

    /// While held, a size refused and tried again keeps to `HELD_GAP` as a
    /// new one does; let go, the pane's size goes at once.
    #[test]
    fn a_held_size_and_its_retry_go_at_most_once_a_gap() {
        let host = FakeHost::new();
        let (retry, mut asked) = futures::channel::mpsc::unbounded();
        let io = Io::new(host.clone(), "c".into(), retry);
        io.set_live(true);
        let sent = || host.sends_of(cmd::RESIZE_SESSION).iter().map(|a| a["cols"].as_u64().unwrap()).collect::<Vec<_>>();
        let size = |cols: u16| PtySize { cols, rows: 24, width: cols * 8, height: 24 * 16 };
        let t0 = Instant::now();
        io.hold(true);
        io.resize(size(80), t0);
        assert_eq!(sent(), [80]);
        io.resize(size(81), t0 + HELD_GAP / 2);
        assert_eq!(sent(), [80], "inside the gap");
        host.refuse(cmd::RESIZE_SESSION, 1);
        io.resize(size(82), t0 + HELD_GAP);
        assert!(asked.try_recv().is_ok(), "refused: a retry is asked for");
        io.retry_size(t0 + HELD_GAP + HELD_GAP / 2);
        assert_eq!(sent(), [80], "the retry keeps to the gap too");
        io.retry_size(t0 + HELD_GAP * 2);
        assert_eq!(sent(), [80, 82]);
        io.resize(size(83), t0 + HELD_GAP * 2);
        io.hold(false);
        assert_eq!(sent(), [80, 82, 83], "let go: the size it is at");
    }
}
