//! One door for every event the app sends the UI, and a way to close it.
//!
//! Sessions outlive the window by design — quit only comes from the tray — so
//! pty reader threads are still streaming when the event loop starts tearing
//! itself down. An emit that lands on a destroyed tao loop is a panic ("cannot
//! move state from Destroyed"), not an `Err`, and a flag checked beforehand
//! only narrows that race: the loop can die between the check and the emit it
//! guards. The gate closes it. Emitters hold it while they emit; shutdown takes
//! it exclusively, which waits out whatever is already in flight and lets
//! nothing new start.

use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::RwLock;
use tauri::{AppHandle, Emitter};

static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);
static GATE: RwLock<()> = RwLock::new(());

/// Sends an event to the UI unless the app is on its way out.
pub fn to_ui<S: Serialize + Clone>(app: &AppHandle, event: &str, payload: S) {
    // A poisoned gate must not silence the app: one emitter panicking is no
    // reason for every pane to stop painting.
    let _gate = GATE.read().unwrap_or_else(|e| e.into_inner());
    if SHUTTING_DOWN.load(Ordering::SeqCst) {
        return;
    }
    let _ = app.emit(event, payload);
}

/// Shuts the door, then waits for the emits that were already through it.
pub fn stop() {
    let _gate = GATE.write().unwrap_or_else(|e| e.into_inner());
    SHUTTING_DOWN.store(true, Ordering::SeqCst);
}
