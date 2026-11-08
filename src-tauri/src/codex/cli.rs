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
//! That layout is kept as-is under `versions/<ver>/`. The release publishes
//! no hash of the files inside; what pins them is the tarball's sha256 and
//! the size each tar header gives, which `cli::validate` holds the unpacked
//! directory to.

use crate::cli::{download_to, sane_version, Package, PackageFile, Source};
use std::fs;
use std::path::{Component, Path};

const REPO: &str = "openai/codex";
const TAG_PREFIX: &str = "rust-v";

#[cfg(windows)]
const BIN_REL: &str = "bin/codex.exe";
#[cfg(not(windows))]
const BIN_REL: &str = "bin/codex";

pub static SOURCE: Source = Source { root_name: "codex-cli", bin_rel: BIN_REL, path_fallback: "codex", latest, fetch };

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
    let ver = tag.strip_prefix(TAG_PREFIX).ok_or_else(|| format!("unexpected tag {tag:?}"))?;
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
    let url = format!("https://github.com/{REPO}/releases/download/{TAG_PREFIX}{version}/codex-package_SHA256SUMS");
    let text = agent
        .get(&url)
        .set("User-Agent", "luna")
        .call()
        .map_err(|e| format!("checksums: {e}"))?
        .into_string()
        .map_err(|e| format!("checksums: {e}"))?;
    checksum_from_sums(&text, &asset_name()).ok_or_else(|| format!("{} not in SHA256SUMS for {version}", asset_name()))
}

fn fetch(agent: &ureq::Agent, version: &str, dir: &Path, progress: &mut dyn FnMut(u64, Option<u64>)) -> Result<Package, String> {
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
    let unpacked = unpack(std::io::BufReader::new(file), dir);
    let _ = fs::remove_file(&part);
    Ok(Package { files: unpacked?, verified: format!("{asset} sha256 from codex-package_SHA256SUMS; files at their tar header sizes") })
}

/// Unpacks the package tarball into `dir` and lists the regular files it put
/// there, each at the size its tar header gives.
pub(crate) fn unpack(reader: impl std::io::Read, dir: &Path) -> Result<Vec<PackageFile>, String> {
    let gz = flate2::read::GzDecoder::new(reader);
    let mut archive = tar::Archive::new(gz);
    // Entries are relative (`bin/codex.exe`); anything that tries to climb
    // out of the version directory is refused by `unpack_in`.
    archive.set_preserve_permissions(true);
    // By path: a later entry for the same path is what stays on disk.
    let mut files = std::collections::BTreeMap::new();
    for entry in archive.entries().map_err(|e| format!("tar: {e}"))? {
        let mut entry = entry.map_err(|e| format!("tar: {e}"))?;
        let regular = entry.header().entry_type().is_file();
        let size = entry.header().size().map_err(|e| format!("tar: {e}"))?;
        let rel = rel_of(&entry.path().map_err(|e| format!("tar: {e}"))?);
        let unpacked = entry.unpack_in(dir).map_err(|e| format!("unpack: {e}"))?;
        if let (true, true, Some(rel)) = (unpacked, regular, rel) {
            files.insert(rel, size);
        }
    }
    Ok(files.into_iter().map(|(rel, size)| PackageFile { rel, size, sha256: None }).collect())
}

/// A tar path as the `/`-separated relative path `cli` keeps; None for one
/// that is not plainly inside the directory.
fn rel_of(path: &Path) -> Option<String> {
    let mut parts = Vec::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::Normal(p) => parts.push(p.to_str()?.to_string()),
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::{checksum_from_sums, unpack, BIN_REL};
    use std::io::Write;

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

    /// A tarball of `entries`, its tar cut to `keep` bytes first.
    fn tar_gz(entries: &[(&str, &[u8])], keep: Option<usize>) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (path, bytes) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append_data(&mut header, path, *bytes).unwrap();
        }
        let mut tar = builder.into_inner().unwrap();
        if let Some(keep) = keep {
            tar.truncate(keep);
        }
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&tar).unwrap();
        gz.finish().unwrap()
    }

    /// The real unpacker on a package whose exe entry stops halfway: the
    /// unpack fails or the directory does not validate. Whole, it does.
    #[test]
    fn a_package_with_its_exe_cut_short_does_not_validate() {
        let dir = std::env::temp_dir().join(format!("luna-codex-unpack-{}", std::process::id()));
        let exe = vec![7u8; 4000];
        let helper = vec![9u8; 3000];
        let entries: [(&str, &[u8]); 2] = [(BIN_REL, &exe), ("codex-resources/codex-command-runner.exe", &helper)];
        let attempt = |keep: Option<usize>| {
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let files = unpack(&tar_gz(&entries, keep)[..], &dir)?;
            crate::cli::validate(&dir, "0.1.0", BIN_REL, crate::cli::Package { files, verified: "test".into() }).map(|_| ())
        };
        // One header block, then half the exe.
        assert!(attempt(Some(512 + 2000)).is_err());
        attempt(None).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
