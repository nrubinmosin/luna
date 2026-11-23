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
pub mod gsync;
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
        // Not the name NVIDIA's "Luna" profile holds (desktop/gsync.rs), nor
        // one the core's copy would be taken for (paths.rs `scope`); the
        // default folder the data dir's fallback.
        let exe = define(nsi, "MAINBINARYNAME");
        assert!(exe != "luna" && !exe.ends_with("-core"), "{exe}");
        assert_eq!(define(nsi, "LOCALDIR"), crate::paths::APP_DIR);
        // Both are folders under LOCALAPPDATA the uninstaller removes whole:
        // one must not be the other.
        assert_ne!(define(nsi, "LOCALDIR").to_lowercase(), define(nsi, "BUNDLEID").to_lowercase());
        let script = include_str!("../../../scripts/test-build.ps1");
        assert!(script.contains(&format!("$appId = '{}'", TEST.identifier)), "test-build.ps1 names another AUMID");
        assert_ne!(TEST.identifier, RELEASE.identifier);
        assert_ne!(TEST.product, RELEASE.product);
        let flavor = of(env!("LUNA_FLAVOR"));
        assert_eq!((super::identifier(), super::product_name()), (flavor.identifier, flavor.product));
    }

    /// The lines of an NSIS script from the one that opens a block (`Section
    /// Install`, `Function .onInstSuccess`) to its end, or none where no such
    /// block is.
    fn block<'a>(nsi: &'a str, opens: &str) -> Vec<&'a str> {
        let mut lines = nsi.lines().skip_while(|l| l.trim_end() != opens);
        let Some(first) = lines.next() else { return Vec::new() };
        let end = if opens.starts_with("Section") { "SectionEnd" } else { "FunctionEnd" };
        std::iter::once(first).chain(lines.take_while(|l| l.trim_end() != end)).collect()
    }

    /// The setup starts Luna again after an update (`/R`) from the end of
    /// its install section, never from .onInstSuccess: with the window an
    /// update's `/P` gives it, that callback runs inside a message the
    /// install thread sends the window, where COM refuses the call to the
    /// shell (RPC_E_CANTCALLOUT_ININPUTSYNCCALL, seen in the 0.19 to 0.20
    /// rehearsal), and Luna was started as the setup's child instead, in the
    /// updating Luna's environment.
    #[test]
    fn the_restart_after_an_update_is_asked_of_the_shell_from_the_install_section() {
        let nsi = include_str!("../../windows/installer.nsi");
        let calls = |lines: &[&str]| lines.iter().filter(|l| l.trim() == "Call LunaRunAsUser").count();
        let install = block(nsi, "Section Install");
        assert!(!install.is_empty(), "installer.nsi has no Section Install");
        assert_eq!(calls(&install), 1, "the install section starts Luna again once");
        let restart = install.iter().position(|l| l.contains(r#"${GetOptions} $CmdHead "/R""#)).expect("the install section reads /R");
        assert!(install[restart..].iter().any(|l| l.trim() == "Call LunaRunAsUser"), "the restart follows the /R check");
        assert_eq!(calls(&block(nsi, "Function .onInstSuccess")), 0, ".onInstSuccess may not ask the shell");
    }

    /// The setup's refusals (a 0.20 Luna still installed; an update with no
    /// install of this Luna's names to update) are made in .onInit, which
    /// runs before any section: nothing written, no folder made, no setup's
    /// mutex taken before them.
    #[test]
    fn the_refusals_come_before_anything_is_changed() {
        let nsi = include_str!("../../windows/installer.nsi");
        let init = block(nsi, "Function .onInit");
        assert!(!init.is_empty(), "installer.nsi has no .onInit");
        let refusal = init.iter().position(|l| l.trim() == "!insertmacro LunaRefuse").expect("the refusal is in .onInit");
        for (what, needle) in
            [("the 0.20 record", "${FORMERUNINSTKEY}"), ("the update's record", "${UNINSTKEY}"), ("the exe", "${MAINBINARYNAME}.exe")]
        {
            assert!(init[..refusal].iter().any(|l| l.contains(needle)), "{what} is checked before the refusal");
        }
        let changes = [
            "SetOutPath",
            "Write",
            "Delete",
            "RMDir",
            "CreateDirectory",
            "CopyFiles",
            "Rename",
            "Exec",
            "CreateMutex",
            "Call LunaOneSetupAtATime",
        ];
        for line in &init {
            assert!(!changes.iter().any(|c| line.trim_start().starts_with(c)), ".onInit changes something: {line}");
        }
    }

    /// A partial install never starts Luna again: NSIS only raises its error
    /// flag for a file it could not write or a registry value it could not
    /// set, so the install section clears the flag before Luna's files and
    /// before the install's record, and stops (`LunaRefuse`) where it is
    /// raised after either, before the restart.
    #[test]
    fn a_partial_install_stops_before_the_restart() {
        let nsi = include_str!("../../windows/installer.nsi");
        let install = block(nsi, "Section Install");
        let at = |what: &str| install.iter().position(|l| l.contains(what)).unwrap_or_else(|| panic!("the install section has no {what}"));
        let all = |what: &str| install.iter().enumerate().filter(|(_, l)| l.trim() == what).map(|(i, _)| i).collect::<Vec<_>>();
        let (clears, checks, refusals) = (all("ClearErrors"), all("${If} ${Errors}"), all("!insertmacro LunaRefuse"));
        let checked_between =
            |from: usize, to: usize| checks.iter().any(|&c| from < c && c < to && refusals.iter().any(|&r| c < r && r < c + 4));
        let exe = at(r#"File "/oname=${MAINBINARYNAME}.exe""#);
        let (uninstaller, read, record) =
            (at("WriteUninstaller"), at("ReadRegStr $OldMainBinaryName"), at(r#"WriteRegStr SHCTX "${PRODUCTKEY}""#));
        let (last_value, restart) = (at(r#""NoRepair""#), at("Call LunaRunAsUser"));
        assert!(clears.iter().any(|&c| c < exe), "the flag is cleared before Luna's files");
        assert!(checked_between(uninstaller, read), "a file not written stops the setup");
        assert!(clears.iter().any(|&c| read < c && c < record), "the flag the record's reads raise is cleared before its writes");
        assert!(checked_between(last_value, restart), "a record not written stops the setup before the restart");
    }
}
