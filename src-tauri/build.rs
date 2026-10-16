use std::process::Command;

/// Stamps the binary with the commit it was built from. The version in
/// Cargo.toml only moves on a release, so without this every test build in
/// between logs the same startup line and cannot be told apart.
///
/// Release builds run in Docker, where the repo's `.git` is a worktree pointer
/// at a host path that does not exist inside the container — build-windows.ps1
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

/// A hash of everything the Rust side is built from, save the version number.
/// Two builds with the same fingerprint run the same Rust code, whatever their
/// versions and frontends; an update that only touched the frontend keeps it.
/// Line endings are hashed as LF so a Windows checkout and the Docker mount of
/// it agree.
fn core_fingerprint() -> String {
    use sha2::{Digest, Sha256};
    use std::path::{Path, PathBuf};

    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else {
                out.push(p);
            }
        }
    }
    let text = |p: &Path| std::fs::read_to_string(p).unwrap_or_default().replace("\r\n", "\n");

    let mut files = Vec::new();
    walk(Path::new("src"), &mut files);
    files.sort();
    let mut h = Sha256::new();
    for f in &files {
        h.update(f.to_string_lossy().replace('\\', "/").as_bytes());
        h.update(text(f).as_bytes());
    }
    h.update(text(Path::new("build.rs")).as_bytes());
    // The manifest and the lock without the one line each that a release bumps.
    let manifest = text(Path::new("Cargo.toml"));
    let mut in_package = false;
    for line in manifest.lines() {
        if line.starts_with('[') {
            in_package = line == "[package]";
        }
        if !(in_package && line.starts_with("version")) {
            h.update(line.as_bytes());
        }
    }
    let lock = text(Path::new("Cargo.lock"));
    let mut after_luna = false;
    for line in lock.lines() {
        if !(after_luna && line.starts_with("version")) {
            h.update(line.as_bytes());
        }
        after_luna = line == "name = \"luna\"";
    }
    h.update(std::env::var("LUNA_FLAVOR").unwrap_or_default().trim().as_bytes());
    h.finalize().iter().take(8).map(|b| format!("{b:02x}")).collect()
}

fn main() {
    println!("cargo:rustc-env=LUNA_CORE_FP={}", core_fingerprint());
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=Cargo.lock");
    println!("cargo:rustc-env=LUNA_BUILD={}", build_stamp());
    println!("cargo:rerun-if-env-changed=LUNA_BUILD");
    // "test" for the build test-build.ps1 makes (paths.rs); empty otherwise.
    println!("cargo:rustc-env=LUNA_FLAVOR={}", std::env::var("LUNA_FLAVOR").unwrap_or_default().trim());
    println!("cargo:rerun-if-env-changed=LUNA_FLAVOR");
    tauri_build::build()
}
