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

/// What one package may unpack to: a tarball that matches its checksum is
/// still not let fill the disk or the folder without end. rust-v0.167.0's
/// largest, x86_64-pc-windows-msvc, holds 52 entries and 581 MB (Linux x64
/// 52 and 541 MB, macOS arm64 50 and 427 MB), none of them links; these
/// leave it room to grow several times over.
const MAX_BYTES: u64 = 4 << 30;
const MAX_ENTRIES: usize = 2_000;

struct Caps {
    bytes: u64,
    entries: usize,
}

/// Unpacks the package tarball into `dir` and lists the regular files it put
/// there, each at the size its tar header gives.
fn unpack(reader: impl std::io::Read, dir: &Path) -> Result<Vec<PackageFile>, String> {
    unpack_capped(reader, dir, Caps { bytes: MAX_BYTES, entries: MAX_ENTRIES })
}

fn unpack_capped(reader: impl std::io::Read, dir: &Path, caps: Caps) -> Result<Vec<PackageFile>, String> {
    let gz = flate2::read::GzDecoder::new(reader);
    let mut archive = tar::Archive::new(gz);
    // Entries are relative (`bin/codex.exe`); anything that tries to climb
    // out of the version directory is refused by `unpack_in`.
    archive.set_preserve_permissions(true);
    // By path: a later entry for the same path is what stays on disk.
    let mut files = std::collections::BTreeMap::new();
    let (mut bytes, mut entries) = (0u64, 0usize);
    for entry in archive.entries().map_err(|e| format!("tar: {e}"))? {
        let mut entry = entry.map_err(|e| format!("tar: {e}"))?;
        let kind = entry.header().entry_type();
        let size = entry.header().size().map_err(|e| format!("tar: {e}"))?;
        let path = entry.path().map_err(|e| format!("tar: {e}"))?.into_owned();
        // Files and folders only. A link, hard or symbolic, could point a
        // later entry, or the record's own check, outside the folder; the
        // packages have none. Devices, pipes and sparse files have no place
        // in one either.
        if kind.is_symlink() || kind.is_hard_link() {
            return Err(format!("tar: refused: a link entry, {}", path.display()));
        }
        let metadata =
            kind.is_pax_global_extensions() || kind.is_pax_local_extensions() || kind.is_gnu_longname() || kind.is_gnu_longlink();
        if !(kind.is_file() || kind.is_dir() || metadata) {
            return Err(format!("tar: refused: an entry of type {:?}, {}", kind.as_byte() as char, path.display()));
        }
        entries += 1;
        if kind.is_file() {
            bytes = bytes.saturating_add(size);
        }
        if entries > caps.entries || bytes > caps.bytes {
            return Err(format!("tar: refused: more than {} entries or {} bytes", caps.entries, caps.bytes));
        }
        let unpacked = entry.unpack_in(dir).map_err(|e| format!("unpack: {e}"))?;
        if let (true, true, Some(rel)) = (unpacked, kind.is_file(), rel_of(&path)) {
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
    use super::{checksum_from_sums, unpack, unpack_capped, Caps, BIN_REL, MAX_BYTES, MAX_ENTRIES};
    use std::io::Write;
    use std::path::PathBuf;

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

    const HELPER: &str = "codex-resources/codex-command-runner.exe";

    enum Entry<'a> {
        File(&'a str, &'a [u8]),
        Symlink(&'a str, &'a str),
        HardLink(&'a str, &'a str),
    }

    fn tar_gz(entries: &[Entry]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for entry in entries {
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o755);
            let (path, data, link) = match entry {
                Entry::File(path, bytes) => (path, *bytes, None),
                Entry::Symlink(path, target) => (path, &[][..], Some((tar::EntryType::Symlink, target))),
                Entry::HardLink(path, target) => (path, &[][..], Some((tar::EntryType::Link, target))),
            };
            header.set_size(data.len() as u64);
            if let Some((kind, target)) = link {
                header.set_entry_type(kind);
                header.set_link_name(target).unwrap();
            }
            header.set_cksum();
            builder.append_data(&mut header, path, data).unwrap();
        }
        let tar = builder.into_inner().unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&tar).unwrap();
        gz.finish().unwrap()
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("luna-codex-unpack-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// What `cli::validate` holds the folder to: every file, at its header's
    /// size.
    #[test]
    fn a_whole_package_unpacks_listing_each_file_at_its_size() {
        let dir = scratch("whole");
        let (exe, helper) = (vec![7u8; 4000], vec![9u8; 3000]);
        let files = unpack(&tar_gz(&[Entry::File(BIN_REL, &exe), Entry::File(HELPER, &helper)])[..], &dir).unwrap();
        let listed: Vec<_> = files.iter().map(|f| (f.rel.as_str(), f.size)).collect();
        assert_eq!(listed, [(BIN_REL, 4000), (HELPER, 3000)]);
        crate::cli::validate(&dir, "0.1.0", BIN_REL, crate::cli::Package { files, verified: "test".into() }).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn link_entries_are_refused() {
        let exe = vec![7u8; 100];
        for link in [Entry::Symlink("codex-path/rg.exe", "../../../outside"), Entry::HardLink("codex-path/rg.exe", "bin/codex.exe")] {
            let dir = scratch("link");
            let refused = unpack(&tar_gz(&[Entry::File(BIN_REL, &exe), link])[..], &dir).err().expect("refused");
            assert!(refused.contains("a link entry"), "{refused}");
            assert!(std::fs::symlink_metadata(dir.join("codex-path").join("rg.exe")).is_err(), "nothing was made for it");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn a_package_past_its_caps_is_refused_before_the_entry_is_written() {
        let (exe, helper) = (vec![7u8; 4000], vec![9u8; 3000]);
        let tarball = tar_gz(&[Entry::File(BIN_REL, &exe), Entry::File(HELPER, &helper)]);

        let dir = scratch("caps-bytes");
        let refused = unpack_capped(&tarball[..], &dir, Caps { bytes: 5000, entries: MAX_ENTRIES }).err().expect("refused");
        assert!(refused.contains("refused: more than"), "{refused}");
        assert!(dir.join(BIN_REL).is_file() && !dir.join(HELPER).exists());
        let _ = std::fs::remove_dir_all(&dir);

        let dir = scratch("caps-entries");
        let refused = unpack_capped(&tarball[..], &dir, Caps { bytes: MAX_BYTES, entries: 1 }).err().expect("refused");
        assert!(refused.contains("refused: more than"), "{refused}");
        assert!(!dir.join(HELPER).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
