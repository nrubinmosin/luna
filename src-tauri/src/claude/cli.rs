//! Where Claude Code releases come from: the same bucket the official
//! installer reads, a plain-text `latest` pointer, a manifest with per-platform
//! sha256s, and a bare executable per platform.

use crate::cli::{download_to, sane_version, Source};
use std::fs;
use std::path::Path;

const RELEASES: &str = "https://downloads.claude.ai/claude-code-releases";
/// `latest` or `stable` — the same two channels the official installer takes.
const CHANNEL: &str = "latest";

#[cfg(windows)]
const BIN_NAME: &str = "claude.exe";
#[cfg(not(windows))]
const BIN_NAME: &str = "claude";

pub static SOURCE: Source = Source {
    root_name: "claude-cli",
    bin_rel: BIN_NAME,
    path_fallback: "claude",
    latest,
    fetch,
};

fn platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "aarch64") => "win32-arm64",
        ("windows", _) => "win32-x64",
        ("macos", "aarch64") => "darwin-arm64",
        ("macos", _) => "darwin-x64",
        ("linux", "aarch64") => "linux-arm64",
        _ => "linux-x64",
    }
}

fn latest(agent: &ureq::Agent) -> Result<String, String> {
    let text = agent
        .get(&format!("{RELEASES}/{CHANNEL}"))
        .call()
        .map_err(|e| format!("version lookup: {e}"))?
        .into_string()
        .map_err(|e| e.to_string())?;
    sane_version(&text, CHANNEL)
}

fn expected_checksum(agent: &ureq::Agent, version: &str) -> Result<String, String> {
    let manifest: serde_json::Value = agent
        .get(&format!("{RELEASES}/{version}/manifest.json"))
        .call()
        .map_err(|e| format!("manifest: {e}"))?
        .into_json()
        .map_err(|e| format!("manifest: {e}"))?;
    manifest["platforms"][platform()]["checksum"]
        .as_str()
        .map(|s| s.to_ascii_lowercase())
        .ok_or_else(|| format!("platform {} not in manifest for {version}", platform()))
}

/// Downloads `version` into `dir`, verifying the sha256 from the manifest
/// before the file gets its real name.
fn fetch(
    agent: &ureq::Agent,
    version: &str,
    dir: &Path,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<(), String> {
    let checksum = expected_checksum(agent, version)?;
    let dest = dir.join(BIN_NAME);
    let part = dir.join(format!("{BIN_NAME}.part"));
    let url = format!("{RELEASES}/{version}/{}/{BIN_NAME}", platform());
    let actual = download_to(agent, &url, &part, progress)?;
    if actual != checksum {
        let _ = fs::remove_file(&part);
        return Err(format!("checksum mismatch for {version}: expected {checksum}, got {actual}"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&part, fs::Permissions::from_mode(0o755));
    }
    fs::rename(&part, &dest).map_err(|e| format!("rename: {e}"))?;
    Ok(())
}
