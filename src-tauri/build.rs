use std::process::Command;

/// Stamps the binary with the commit it was built from. The version in
/// Cargo.toml has never moved off 0.1.0, so without this every build logs the
/// same startup line and a log spanning a week of builds cannot be told apart.
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

fn main() {
    println!("cargo:rustc-env=LUNA_BUILD={}", build_stamp());
    println!("cargo:rerun-if-env-changed=LUNA_BUILD");
    tauri_build::build()
}
