//! One door for every event the app sends the UI, and a way to close it.
//!
//! Whatever is on the other side of the door is installed once: the window's
//! inbox and its sessions' output sinks (native.rs, `install_for`, which
//! takes only the events the window listens to), or, in the core, the
//! connections of the windows linked to it (coreproc.rs, `install`). Nothing that emits holds an app handle
//! of its own, so the code that runs the sessions does not depend on there
//! being a window in the same process.
//!
//! Sessions outlive the window by design — quit only comes from the tray — so
//! pty reader threads are still streaming when the event loop starts tearing
//! itself down. An emit that lands on a loop being torn down must not reach
//! it, and a flag checked beforehand only narrows that race: the loop can die
//! between the check and the emit it guards. The gate closes it. Emitters hold it while they emit; shutdown takes
//! it exclusively, which waits out whatever is already in flight and lets
//! nothing new start.

use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{OnceLock, RwLock};

type Sink = Box<dyn Fn(&str, serde_json::Value) + Send + Sync>;
type Wants = Box<dyn Fn(&str) -> bool + Send + Sync>;

static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);
static GATE: RwLock<()> = RwLock::new(());
static SINK: OnceLock<(Wants, Sink)> = OnceLock::new();

/// Where events go from now on, every one of them. The first sink installed
/// stays.
pub fn install(sink: impl Fn(&str, serde_json::Value) + Send + Sync + 'static) {
    install_for(|_| true, sink);
}

/// `install`, for the events `wants` takes: the others are dropped before
/// their payload is even turned into JSON.
pub fn install_for(wants: impl Fn(&str) -> bool + Send + Sync + 'static, sink: impl Fn(&str, serde_json::Value) + Send + Sync + 'static) {
    let _ = SINK.set((Box::new(wants), Box::new(sink)));
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
    #[cfg(test)]
    if let Ok(value) = serde_json::to_value(&payload) {
        SENT.lock().unwrap_or_else(|e| e.into_inner()).push((event.to_string(), value));
    }
    let Some((_, sink)) = SINK.get().filter(|(wants, _)| wants(event)) else { return };
    let Ok(payload) = serde_json::to_value(payload) else { return };
    sink(event, payload);
}

/// Every event a test's code sent, sink or not.
#[cfg(test)]
static SENT: std::sync::Mutex<Vec<(String, serde_json::Value)>> = std::sync::Mutex::new(Vec::new());

/// The payloads of `event` sent so far in this test run.
#[cfg(test)]
pub fn sent_for_test(event: &str) -> Vec<serde_json::Value> {
    SENT.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|(e, _)| e == event).map(|(_, v)| v.clone()).collect()
}

/// Shuts the door, then waits for the emits that were already through it.
pub fn stop() {
    let _gate = GATE.write().unwrap_or_else(|e| e.into_inner());
    SHUTTING_DOWN.store(true, Ordering::SeqCst);
}
