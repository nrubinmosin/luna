use std::process::Command;

// The fingerprint's one definition, shared with the tests that hold it to
// every compiled-in input.
#[path = "src/fingerprint.rs"]
mod fingerprint;

// The names each flavor goes by, shared with the crate.
#[path = "src/identity.rs"]
#[allow(dead_code)]
mod identity;

/// Stamps the binary with the commit it was built from. The version in
/// Cargo.toml only moves on a release, so without this every test build in
/// between logs the same startup line and cannot be told apart.
///
/// Release builds run in Docker, where the repo's `.git` is a worktree pointer
/// at a host path that does not exist inside the container — scripts/build-windows.ps1
/// resolves the commit on the host and passes it in as LUNA_BUILD. The git call
/// here is the fallback for building on the host directly.
fn build_stamp() -> String {
    if let Ok(s) = std::env::var("LUNA_BUILD") {
        let s = s.trim();
        if !s.is_empty() {
            return s.to_owned();
        }
    }

    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
    };

    match git(&["rev-parse", "--short", "HEAD"]) {
        Some(sha) if !sha.is_empty() => {
            // A trailing + means the tree had uncommitted changes, so the sha
            // alone does not describe what is running.
            let dirty = git(&["status", "--porcelain"]).is_some_and(|s| !s.is_empty());
            format!("{sha}{}", if dirty { "+" } else { "" })
        }
        _ => "dev".to_owned(),
    }
}

/// Leaves the fingerprint in `core-fingerprint.txt` beside the exe, in the
/// profile folder (`target/<triple>/release/`), where the release scripts read
/// it for `latest.json` (scripts/release-bundle.ps1): the exe is a Windows
/// binary built on Linux, and a release runner cannot ask it. The file is
/// written from the same value the binary is stamped with, so the two cannot
/// differ; a test holds `--core-fingerprint` to it (coreproc.rs).
///
/// Cargo gives a build script only OUT_DIR, `<profile>/build/<pkg>/<hash>/out`
/// since 1.100 (`<profile>/build/<pkg>-<hash>/out` before), and asks it to keep
/// its writes there; the profile folder is the one place a script outside
/// cargo can find without cargo's JSON.
/// A layout of another shape gets a warning and no file, and the release then
/// stops on the missing file rather than publishing a guess. The file's path
/// goes to the crate as `LUNA_CORE_FP_FILE`, for that test.
fn leave_core_fingerprint(fp: &str) {
    let Some(out) = std::env::var_os("OUT_DIR").map(std::path::PathBuf::from) else { return };
    let build = out.ancestors().skip(2).take(2).find(|a| a.file_name().is_some_and(|n| n == "build"));
    match build.and_then(|b| b.parent()) {
        Some(profile) => {
            let file = profile.join("core-fingerprint.txt");
            match std::fs::write(&file, fp) {
                Ok(()) => println!("cargo:rustc-env=LUNA_CORE_FP_FILE={}", file.display()),
                Err(e) => println!("cargo:warning=cannot write {}: {e}", file.display()),
            }
        }
        None => println!("cargo:warning=OUT_DIR {} is not <profile>/build/<pkg>/[<hash>/]out; no core-fingerprint.txt", out.display()),
    }
}

/// What the Windows exe carries besides its code: its icon, as resource 1
/// (the first, which Explorer and the shortcuts show, and the one gpui loads
/// for the window's class), and its version resource, this build's version
/// and its flavor's names (identity.rs). Compiled by embed-resource, which
/// gpui-pre's build script already uses for its manifest, with the
/// toolchain's llvm-rc, so a Linux host cross-builds it as it builds the
/// rest. No manifest here: gpui-pre's is linked in (it asks for per-monitor
/// DPI awareness and Common Controls 6), and two would be a duplicate
/// resource, which the linker refuses.
fn windows_resources(flavor: &str) {
    let names = identity::of(flavor);
    let version = std::env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION");
    let part = |name: &str| std::env::var(name).ok().and_then(|v| v.parse::<u16>().ok()).unwrap_or(0);
    let numbers = format!("{},{},{},0", part("CARGO_PKG_VERSION_MAJOR"), part("CARGO_PKG_VERSION_MINOR"), part("CARGO_PKG_VERSION_PATCH"));
    let dir = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let icon = dir.join("icons").join("icon.ico");
    // A string in the script: its backslashes doubled, as rc reads escapes.
    let quoted = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\"\""));
    let strings = [
        ("CompanyName", names.publisher),
        ("FileDescription", names.product),
        ("FileVersion", version.as_str()),
        ("ProductName", names.product),
        ("ProductVersion", version.as_str()),
    ];
    let values: String = strings.iter().map(|(k, v)| format!("      VALUE {}, {}\n", quoted(k), quoted(v))).collect();
    // Language neutral, Unicode: the block and translation tauri-winres wrote.
    let rc = format!(
        "1 ICON {icon}\n\
         1 VERSIONINFO\n\
         FILEVERSION {numbers}\n\
         PRODUCTVERSION {numbers}\n\
         FILEOS 0x40004\n\
         FILETYPE 0x1\n\
         {{\n  BLOCK \"StringFileInfo\"\n  {{\n    BLOCK \"000004b0\"\n    {{\n{values}    }}\n  }}\n  \
         BLOCK \"VarFileInfo\"\n  {{\n    VALUE \"Translation\", 0x0, 0x04b0\n  }}\n}}\n",
        icon = quoted(&icon.to_string_lossy()),
    );
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("luna.rc");
    std::fs::write(&out, rc).expect("luna.rc");
    println!("cargo:rerun-if-changed=icons/icon.ico");
    embed_resource::compile(&out, embed_resource::NONE).manifest_optional().expect("the exe's resources");
}

fn main() {
    let flavor = std::env::var("LUNA_FLAVOR").unwrap_or_default();
    let fp = fingerprint::core_fingerprint(std::path::Path::new("."), &flavor);
    leave_core_fingerprint(&fp);
    println!("cargo:rustc-env=LUNA_CORE_FP={fp}");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=Cargo.lock");
    for input in fingerprint::INPUTS {
        println!("cargo:rerun-if-changed={input}");
    }
    println!("cargo:rustc-env=LUNA_BUILD={}", build_stamp());
    println!("cargo:rerun-if-env-changed=LUNA_BUILD");
    // "test" for the build scripts/test-build.ps1 makes (paths.rs); empty otherwise.
    println!("cargo:rustc-env=LUNA_FLAVOR={}", std::env::var("LUNA_FLAVOR").unwrap_or_default().trim());
    println!("cargo:rerun-if-env-changed=LUNA_FLAVOR");
    // `luna_test`: what only the test build has (its terminals by argument,
    // the driver, the stand-in clipboard, the tray's test hook) is compiled
    // into it alone, never into a release (native.rs, native/desktop.rs).
    println!("cargo::rustc-check-cfg=cfg(luna_test)");
    if flavor.trim() == "test" {
        println!("cargo:rustc-cfg=luna_test");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        windows_resources(&flavor);
        // The Visual C++ runtime linked in and the Universal CRT left to
        // Windows, which has it from 10 on: the exe needs no
        // vcruntime140.dll beside it, as the Tauri-built 0.19 did not.
        // Nothing for a target other than MSVC.
        static_vcruntime::metabuild();
    }
}
