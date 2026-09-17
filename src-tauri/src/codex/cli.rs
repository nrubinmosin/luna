//! Where Codex releases come from: GitHub releases of `openai/codex`, tagged
//! `rust-v<ver>`. The artifact is the package tarball rather than the bare
//! exe — only the package has a line in the release's `SHA256SUMS`, and it
//! carries the helpers the Windows sandbox needs next to the binary:
//!
//! ```text
//! bin/codex.exe
//! bin/codex-code-mode-host.exe
//! codex-path/rg.exe
//! codex-resources/codex-command-runner.exe
//! codex-resources/codex-windows-sandbox-setup.exe
//! codex-package.json
//! ```
//!
//! That layout is kept as-is under `versions/<ver>/`.

use crate::cli::{download_to, sane_version, Source};
use std::fs;
use std::path::Path;

const REPO: &str = "openai/codex";
const TAG_PREFIX: &str = "rust-v";

#[cfg(windows)]
const BIN_REL: &str = "bin/codex.exe";
#[cfg(not(windows))]
const BIN_REL: &str = "bin/codex";

pub static SOURCE: Source = Source {
    root_name: "codex-cli",
    bin_rel: BIN_REL,
    path_fallback: "codex",
    latest,
    fetch,
};

fn target() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "aarch64") => "aarch64-pc-windows-msvc",
        ("windows", _) => "x86_64-pc-windows-msvc",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", _) => "x86_64-apple-darwin",
        ("linux", "aarch64") => "aarch64-unknown-linux-musl",
        _ => "x86_64-unknown-linux-musl",
    }
}

fn asset_name() -> String {
    format!("codex-package-{}.tar.gz", target())
}

fn latest(agent: &ureq::Agent) -> Result<String, String> {
    let rel: serde_json::Value = agent
        .get(&format!("https://api.github.com/repos/{REPO}/releases/latest"))
        .set("User-Agent", "luna")
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("release lookup: {e}"))?
        .into_json()
        .map_err(|e| format!("release lookup: {e}"))?;
    let tag = rel["tag_name"].as_str().ok_or("release lookup: no tag_name")?;
    let ver = tag
        .strip_prefix(TAG_PREFIX)
        .ok_or_else(|| format!("unexpected tag {tag:?}"))?;
    sane_version(ver, "github")
}

/// The sha256 of our asset out of the release's `codex-package_SHA256SUMS`
/// (`<hex>  <file>` per line, like `sha256sum` writes it).
pub fn checksum_from_sums(sums: &str, asset: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hex = parts.next()?;
        let name = parts.next()?;
        (name == asset && hex.len() == 64).then(|| hex.to_ascii_lowercase())
    })
}

fn expected_checksum(agent: &ureq::Agent, version: &str) -> Result<String, String> {
    let url = format!(
        "https://github.com/{REPO}/releases/download/{TAG_PREFIX}{version}/codex-package_SHA256SUMS"
    );
    let text = agent
        .get(&url)
        .set("User-Agent", "luna")
        .call()
        .map_err(|e| format!("checksums: {e}"))?
        .into_string()
        .map_err(|e| format!("checksums: {e}"))?;
    checksum_from_sums(&text, &asset_name())
        .ok_or_else(|| format!("{} not in SHA256SUMS for {version}", asset_name()))
}

fn fetch(
    agent: &ureq::Agent,
    version: &str,
    dir: &Path,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<(), String> {
    let checksum = expected_checksum(agent, version)?;
    let asset = asset_name();
    let part = dir.join(format!("{asset}.part"));
    let url = format!("https://github.com/{REPO}/releases/download/{TAG_PREFIX}{version}/{asset}");
    let actual = download_to(agent, &url, &part, progress)?;
    if actual != checksum {
        let _ = fs::remove_file(&part);
        return Err(format!("checksum mismatch for {version}: expected {checksum}, got {actual}"));
    }

    let file = fs::File::open(&part).map_err(|e| format!("open {}: {e}", part.display()))?;
    let gz = flate2::read::GzDecoder::new(std::io::BufReader::new(file));
    let mut archive = tar::Archive::new(gz);
    // Entries are relative (`bin/codex.exe`); anything that tries to climb
    // out of the version directory is refused by `unpack_in`.
    archive.set_preserve_permissions(true);
    for entry in archive.entries().map_err(|e| format!("tar: {e}"))? {
        let mut entry = entry.map_err(|e| format!("tar: {e}"))?;
        entry.unpack_in(dir).map_err(|e| format!("unpack: {e}"))?;
    }
    let _ = fs::remove_file(&part);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::checksum_from_sums;

    #[test]
    fn picks_the_matching_line() {
        let sums = "aaaa  codex-package-aarch64-pc-windows-msvc.tar.gz\n\
                    94cc5b3632769504c809f6c0364b693c0dfddc5c30c8361095d2263a07ac45a4  codex-package-x86_64-pc-windows-msvc.tar.gz\n";
        assert_eq!(
            checksum_from_sums(sums, "codex-package-x86_64-pc-windows-msvc.tar.gz").as_deref(),
            Some("94cc5b3632769504c809f6c0364b693c0dfddc5c30c8361095d2263a07ac45a4")
        );
        // A short hash is a malformed line, not a checksum.
        assert_eq!(checksum_from_sums(sums, "codex-package-aarch64-pc-windows-msvc.tar.gz"), None);
        assert_eq!(checksum_from_sums(sums, "other"), None);
    }
}
