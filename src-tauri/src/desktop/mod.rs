//! The desktop's services for the window, as plain Rust: the tray, notifications, the single instance, the file
//! pickers, the clipboard, the window's size and place, and the updater
//! (docs/architecture/desktop-services.md). Each is functions and structs
//! taking callbacks, no window type of any toolkit, so that the native window
//! wires one in with a few lines.
//!
//! The native window wires them in (native.rs, native/desktop.rs); the
//! single instance is claimed before it opens (lib.rs `run`).
// The blocking pickers and a few accessors wait for the window's callers.
#![allow(dead_code)]

pub mod clipboard;
pub mod dialog;
pub mod notify;
pub mod single;
pub mod tray;
pub mod updater;
pub mod volume;
pub mod window_state;

/// This build's names (identity.rs): the release's, or the test build's,
/// which runs beside the Luna in use.
fn identity() -> &'static crate::identity::Identity {
    crate::identity::of(env!("LUNA_FLAVOR"))
}

/// The app's identifier, as the installer registers it: the AUMID of the
/// Start-menu shortcut and of every toast, the bundle identifier on macOS,
/// and what the single-instance names are made from.
pub fn identifier() -> &'static str {
    identity().identifier
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

/// The name a person sees: the window's title, the tray's tooltip, a
/// notification's sender.
pub fn product_name() -> &'static str {
    identity().product
}

#[cfg(test)]
mod tests {
    /// The value of `!define <name> "<value>"` in an NSIS script.
    fn define<'a>(nsi: &'a str, name: &str) -> &'a str {
        let line = nsi.lines().find(|l| l.starts_with(&format!("!define {name} "))).unwrap_or_else(|| panic!("no {name}"));
        line.split('"').nth(1).unwrap_or_else(|| panic!("{line}"))
    }

    /// The release's names are the ones the installer registers (an
    /// installed Luna's shortcut, toasts and uninstall key were made from
    /// them, and an update finds the install by them), and the test build's
    /// identifier the one its script gives its shortcut, which ties its
    /// toasts to it. Each build goes by its flavor's.
    #[test]
    fn the_names_are_the_ones_the_installer_and_the_test_build_use() {
        use crate::identity::{of, RELEASE, TEST};
        let nsi = include_str!("../../windows/installer.nsi");
        assert_eq!(define(nsi, "BUNDLEID"), RELEASE.identifier);
        assert_eq!(define(nsi, "PRODUCTNAME"), RELEASE.product);
        assert_eq!(define(nsi, "MANUFACTURER"), RELEASE.publisher);
        let script = include_str!("../../../scripts/test-build.ps1");
        assert!(script.contains(&format!("$appId = '{}'", TEST.identifier)), "test-build.ps1 names another AUMID");
        assert_ne!(TEST.identifier, RELEASE.identifier);
        assert_ne!(TEST.product, RELEASE.product);
        let flavor = of(env!("LUNA_FLAVOR"));
        assert_eq!((super::identifier(), super::product_name()), (flavor.identifier, flavor.product));
    }
}
