//! The names Luna goes by, for each flavor of the build: the release's, which
//! every Luna installed since 0.6.31 was installed under, and the test
//! build's, so that it runs beside the Luna in use (its own single instance,
//! shortcut and toasts).
//!
//! build.rs includes this file (`#[path]`) for the exe's version resource;
//! the crate reads it for the AUMID, the single instance's names and what a
//! person sees (desktop/mod.rs), and for where 0.19's window kept its storage
//! (native/import.rs). The installer has the release's in its own defines
//! (windows/installer.nsi), and scripts/test-build.ps1 the test build's
//! identifier for its shortcut; the tests hold both to these.

/// One flavor's names.
pub struct Identity {
    /// The app's identifier: the AUMID of the Start-menu shortcut and of
    /// every toast, the bundle identifier on macOS.
    pub identifier: &'static str,
    /// The name a person sees: the window's title, the tray's tooltip, a
    /// notification's sender, the install's folder and the exe's product.
    pub product: &'static str,
    /// The publisher in the exe's version resource and the installer's
    /// registry key (`HKCU\Software\<publisher>`): the identifier's second
    /// part, as Tauri's bundler took it. build.rs reads it; the crate's
    /// tests hold it to the installer's.
    #[cfg_attr(not(test), allow(dead_code))]
    pub publisher: &'static str,
}

/// The release's.
pub const RELEASE: Identity = Identity { identifier: "rm.luna", product: "Luna", publisher: "luna" };

/// The test build's (`LUNA_FLAVOR=test`).
pub const TEST: Identity = Identity { identifier: "rm.luna.test", product: "Luna Test", publisher: "luna" };

/// The names of the build whose `LUNA_FLAVOR` is `flavor`.
pub fn of(flavor: &str) -> &'static Identity {
    if flavor.trim() == "test" {
        &TEST
    } else {
        &RELEASE
    }
}
