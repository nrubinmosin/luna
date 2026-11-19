//! The pane's way into its session: keys and pastes (`write_session`) and the
//! PTY's size (`resize_session`), sent on the window's thread in the order
//! they are made. Only while the session runs, as the page's `ptySender`
//! gates them: before its start answers, or after it exited, there is no
//! session to take them, and a write would come back as "no such session".
//! The size is kept meanwhile and sent once it runs.
//!
//! A size the host refused to hand over (the core's line was full for a
//! moment) is not taken for sent: the pane is asked to try again
//! (`Io::new`'s `retry`), so a size is never left stuck at one the PTY never got.

use crate::host::{Host, cmd};
use futures::channel::mpsc::UnboundedSender;
use serde_json::json;
use std::cell::Cell;
use std::sync::Arc;

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
    /// The size the pane last laid out at, and the last one the host took.
    size: Cell<Option<PtySize>>,
    sent: Cell<Option<PtySize>>,
    /// Told when a size was refused: the pane calls `flush_size` a moment later.
    retry: UnboundedSender<()>,
}

impl Io {
    pub fn new(host: Arc<dyn Host>, id: String, retry: UnboundedSender<()>) -> Io {
        Io { host, id, live: Cell::new(false), size: Cell::new(None), sent: Cell::new(None), retry }
    }

    /// The session runs (or no longer does). Coming alive sends the size the
    /// pane is at, unless that was sent already.
    pub fn set_live(&self, live: bool) {
        self.live.set(live);
        if live {
            self.flush_size();
        } else {
            // A session started again is told its size afresh.
            self.sent.set(None);
        }
    }

    /// Input for the session: dropped while it does not run.
    pub fn write(&self, data: &str) {
        self.write_as(data, false);
    }

    /// A reply to the program's query (a size, a color, the device's
    /// attributes): sent with `reply`, so the core does not count it as a
    /// person typing (pty/control.rs `write_session`).
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

    /// The pane laid out at `size`: sent when it differs from the last sent.
    pub fn resize(&self, size: PtySize) {
        self.size.set(Some(size));
        self.flush_size();
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
