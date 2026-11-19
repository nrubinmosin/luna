//! What Tauri's plugins and its tray do for the window, as plain Rust that
//! needs no Tauri: the tray, notifications, the single instance, the file
//! pickers, the clipboard, the window's size and place, and the updater
//! (docs/architecture/desktop-services.md). Each is functions and structs
//! taking callbacks, no window type of any toolkit, so that the native window
//! wires one in with a few lines.
//!
//! The native window wires them in (native.rs, native/desktop.rs); the
//! WebView window takes the single instance (lib.rs `run`) and keeps Tauri's
//! plugins for the rest.
// The blocking pickers and a few accessors wait for the window's callers.
#![allow(dead_code)]

pub mod clipboard;
pub mod dialog;
pub mod notify;
pub mod single;
pub mod tray;
pub mod updater;
pub mod window_state;

/// The app's identifier, as tauri.conf.json names it and the installer
/// registers it: the AUMID of the Start-menu shortcut and of every toast,
/// the bundle identifier on macOS, and what the single-instance names are
/// made from. The test build has one of its own (tauri.test.conf.json), so
/// it runs beside the Luna in use.
pub fn identifier() -> &'static str {
    if crate::paths::is_test() {
        "rm.luna.test"
    } else {
        "rm.luna"
    }
}

/// What brings the running window up: a second launch (`single`), set by
/// whichever window runs.
static BRING_UP: std::sync::Mutex<Option<std::sync::Arc<dyn Fn() + Send + Sync>>> = std::sync::Mutex::new(None);

/// Sets what `bring_up` does, once the window that does it is there.
pub fn on_bring_up(f: impl Fn() + Send + Sync + 'static) {
    *BRING_UP.lock().unwrap_or_else(|e| e.into_inner()) = Some(std::sync::Arc::new(f));
}

/// Brings the running window up, if one has said how; on the caller's
/// thread, the lock let go first.
pub fn bring_up() {
    let f = BRING_UP.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(f) = f {
        f();
    }
}

/// The name a person sees: the tray's tooltip, a notification's sender.
pub fn product_name() -> &'static str {
    if crate::paths::is_test() {
        "Luna Test"
    } else {
        "Luna"
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    /// What Tauri's config says, for as long as it is there: the installed
    /// Luna's shortcut, toasts and updater were made from it.
    fn conf(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn the_names_are_the_ones_tauris_config_gives() {
        // The test flavor's config overrides both names.
        let flavor = if crate::paths::is_test() {
            conf(include_str!("../../tauri.test.conf.json"))
        } else {
            conf(include_str!("../../tauri.conf.json"))
        };
        assert_eq!(flavor["identifier"], super::identifier());
        assert_eq!(flavor["productName"], super::product_name());
    }
}
