//! One door for every event the app sends the UI, and a way to close it.
//!
//! Whatever is on the other side of the door is installed once (`install`):
//! the window's webview today. Nothing that emits holds an app handle of its
//! own, so the code that runs the sessions does not depend on there being a
//! window in the same process.
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
use std::sync::{OnceLock, RwLock};

type Sink = Box<dyn Fn(&str, serde_json::Value) + Send + Sync>;

static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);
static GATE: RwLock<()> = RwLock::new(());
static SINK: OnceLock<Sink> = OnceLock::new();

/// Where events go from now on. The first one installed stays.
pub fn install(sink: impl Fn(&str, serde_json::Value) + Send + Sync + 'static) {
    let _ = SINK.set(Box::new(sink));
}

/// Sends an event to the UI unless the app is on its way out. Before a sink
/// is installed there is nobody to tell, and the event is dropped.
pub fn to_ui<S: Serialize>(event: &str, payload: S) {
    // A poisoned gate must not silence the app: one emitter panicking is no
    // reason for every pane to stop painting.
    let _gate = GATE.read().unwrap_or_else(|e| e.into_inner());
    if SHUTTING_DOWN.load(Ordering::SeqCst) {
        return;
    }
    let (Some(sink), Ok(payload)) = (SINK.get(), serde_json::to_value(payload)) else { return };
    sink(event, payload);
}

/// Shuts the door, then waits for the emits that were already through it.
pub fn stop() {
    let _gate = GATE.write().unwrap_or_else(|e| e.into_inner());
    SHUTTING_DOWN.store(true, Ordering::SeqCst);
}
