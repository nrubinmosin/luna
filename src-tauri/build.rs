use std::process::Command;

// The fingerprint's one definition, shared with the tests that hold it to
// every compiled-in input.
#[path = "src/fingerprint.rs"]
mod fingerprint;

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
/// Cargo gives a build script only OUT_DIR, `<profile>/build/luna-<hash>/out`,
/// and asks it to keep its writes there; the profile folder is the one place a
/// script outside cargo can find without cargo's JSON, which `tauri build` does
/// not pass on. A layout of another shape gets a warning and no file, and the
/// release then stops on the missing file rather than publishing a guess.
fn leave_core_fingerprint(fp: &str) {
    let Some(out) = std::env::var_os("OUT_DIR").map(std::path::PathBuf::from) else { return };
    let mut up = out.ancestors().skip(2);
    match (up.next(), up.next()) {
        (Some(build), Some(profile)) if build.file_name().is_some_and(|n| n == "build") => {
            if let Err(e) = std::fs::write(profile.join("core-fingerprint.txt"), fp) {
                println!("cargo:warning=cannot write {}: {e}", profile.join("core-fingerprint.txt").display());
            }
        }
        _ => println!("cargo:warning=OUT_DIR {} is not <profile>/build/<pkg>/out; no core-fingerprint.txt", out.display()),
    }
}

/// The app commands the window registers, read from the one list there is,
/// `generate_handler!` in lib.rs (each entry's last path segment). They go
/// into the application manifest, which holds every app command to the
/// capabilities as Tauri holds a plugin's: one that no capability grants to
/// the calling window and origin is refused before it runs. Read rather than
/// kept as a second list, so the two cannot drift.
fn app_commands() -> &'static [&'static str] {
    const OPEN: &str = "generate_handler![";
    let lib = std::fs::read_to_string("src/lib.rs").expect("src/lib.rs");
    let start = lib.find(OPEN).expect("lib.rs has no generate_handler![") + OPEN.len();
    let end = start + lib[start..].find(']').expect("generate_handler![ is not closed");
    let names: Vec<&'static str> = lib[start..end]
        .split(',')
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(|e| &*Box::leak(e.rsplit("::").next().unwrap_or(e).to_owned().into_boxed_str()))
        .collect();
    let plain = |n: &str| !n.is_empty() && n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    assert!(names.iter().all(|n| plain(n)), "unexpected entry in generate_handler!: {names:?}");
    Box::leak(names.into_boxed_slice())
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
    // Writes `permissions/autogenerated/<command>.toml`, but only where its
    // text differs: committed, they let a read-only checkout build too.
    let manifest = tauri_build::AppManifest::new().commands(app_commands());
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest)).expect("tauri-build")
}
