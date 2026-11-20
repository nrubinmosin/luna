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
//! directory to. It then records each file's sha256 as unpacked, which going
//! back to the version later holds it to again (`cli::reverify`).

use crate::cli::{download_to, sane_version, Package, PackageFile, Source};
use crate::http::Agent;
use std::fs;
use std::path::{Component, Path};

const REPO: &str = "openai/codex";
const TAG_PREFIX: &str = "rust-v";

#[cfg(windows)]
const BIN_REL: &str = "bin/codex.exe";
#[cfg(not(windows))]
const BIN_REL: &str = "bin/codex";

pub static SOURCE: Source = Source { root_name: "codex-cli", bin_rel: BIN_REL, latest, fetch };

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

fn latest(agent: &Agent) -> Result<String, String> {
    let rel: serde_json::Value = agent
        .get(&format!("https://api.github.com/repos/{REPO}/releases/latest"))
        .header("User-Agent", "luna")
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("release lookup: {e}"))?
        .json()
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

fn expected_checksum(agent: &Agent, version: &str) -> Result<String, String> {
    let url = format!("https://github.com/{REPO}/releases/download/{TAG_PREFIX}{version}/codex-package_SHA256SUMS");
    let text = agent
        .get(&url)
        .header("User-Agent", "luna")
        .call()
        .map_err(|e| format!("checksums: {e}"))?
        .text()
        .map_err(|e| format!("checksums: {e}"))?;
    checksum_from_sums(&text, &asset_name()).ok_or_else(|| format!("{} not in SHA256SUMS for {version}", asset_name()))
}

fn fetch(agent: &Agent, version: &str, dir: &Path, progress: &mut dyn FnMut(u64, Option<u64>)) -> Result<Package, String> {
    let checksum = expected_checksum(agent, version)?;
    let asset = asset_name();
    let part = dir.join(format!("{asset}.part"));
    let url = format!("https://github.com/{REPO}/releases/download/{TAG_PREFIX}{version}/{asset}");
    let actual = download_to(agent, &url, &part, progress)?;
    if actual != checksum {
        let _ = fs::remove_file(&part);
        return Err(format!("checksum mismatch for {version}: expected {checksum}, got {actual}"));
    }

    let unpacked = unpack(|| fs::File::open(&part).map(std::io::BufReader::new), dir);
    let _ = fs::remove_file(&part);
    let files = unpacked?;
    let mut verified = format!("{asset} sha256 from codex-package_SHA256SUMS; files at their tar header sizes");
    if let Some(signed) = crate::cli::authenticode::require(dir, &files, &SIGNERS)? {
        verified = format!("{verified}; {signed}");
    }
    Ok(Package { files, verified })
}

/// Who signs the package's program files (0.169.0 and 0.171.0: every `.exe`
/// but `rg.exe` and every `.dll` under `codex-resources/voice/bin/` but
/// `vcruntime140.dll` are "OpenAI OpCo, LLC", status Valid), and what ships
/// unsigned: ripgrep's `rg.exe`, the one path excused. That
/// `vcruntime140.dll`, the same bytes in both, carries in its own signature
/// "Microsoft Windows Software Compatibility Publisher", a certificate
/// Microsoft also signs others' submitted drivers and programs with, so it is
/// taken on that one path only, never as a `dll_also`. Checked on Windows
/// only (`cli::authenticode`).
const SIGNERS: crate::cli::authenticode::Signers = crate::cli::authenticode::Signers {
    signer: "OpenAI OpCo, LLC",
    dll_also: &["Microsoft Windows", "Microsoft Corporation"],
    path_also: &[("codex-resources/voice/bin/vcruntime140.dll", "Microsoft Windows Software Compatibility Publisher")],
    unsigned: &["codex-path/rg.exe"],
};

/// What one package may unpack to: a tarball that matches its checksum is
/// still not let fill the disk or the folder without end. rust-v0.167.0's
/// largest, x86_64-pc-windows-msvc, holds 52 entries and 581 MB (Linux x64
/// 52 and 541 MB, macOS arm64 50 and 427 MB), none of them links; these
/// leave it room to grow several times over.
const MAX_BYTES: u64 = 4 << 30;
const MAX_ENTRIES: usize = 2_000;
/// The most one metadata record (a PAX extension header, a GNU long name)
/// may hold. The package is written by Python's `tarfile` in PAX format
/// (`scripts/codex_package/archive.py` in openai/codex), which adds a small
/// PAX record for an entry's fractional mtime; a path or two fits in far less.
const MAX_META: u64 = 64 << 10;

struct Caps {
    bytes: u64,
    entries: usize,
    /// The decompressed tar stream as a whole: what the entries may hold,
    /// plus a header, padding and a metadata record for each.
    stream: u64,
}

impl Caps {
    fn of(bytes: u64, entries: usize) -> Caps {
        Caps { bytes, entries, stream: bytes + (entries as u64 + 2) * (2 * 1024 + MAX_META) }
    }
}

/// The decompressed stream, cut off with an error past `left` bytes: a gzip
/// bomb, or entries whose sizes lie, stop there however the tar reader
/// reads.
struct Capped<R> {
    inner: R,
    left: u64,
}

impl<R: std::io::Read> std::io::Read for Capped<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.left == 0 {
            // Exactly at the cap is fine if the stream ends there.
            let mut probe = [0u8; 1];
            return match self.inner.read(&mut probe)? {
                0 => Ok(0),
                _ => Err(std::io::Error::other("the package unpacks past its cap")),
            };
        }
        let want = buf.len().min(usize::try_from(self.left).unwrap_or(usize::MAX));
        let n = self.inner.read(&mut buf[..want])?;
        self.left -= n as u64;
        Ok(n)
    }
}

/// Unpacks the package tarball `open` gives into `dir` and lists the regular
/// files it put there, each at the size its tar header gives.
fn unpack<R: std::io::Read>(open: impl Fn() -> std::io::Result<R>, dir: &Path) -> Result<Vec<PackageFile>, String> {
    unpack_capped(open, dir, Caps::of(MAX_BYTES, MAX_ENTRIES))
}

/// Two passes over the tarball. The first reads it raw, as the headers
/// stand, and refuses before anything is written: a metadata record past
/// `MAX_META` (the tar reader would take it into memory whole), a PAX `size`
/// override (Python writes one only for a file of 8 GiB or more, none of
/// which a package has, and it would make the header's size a lie), a link,
/// any entry but a file or a folder, and more entries or bytes than `caps`.
/// The second unpacks, the PAX records applied, budgeting each entry's size
/// as the tar reader takes it, through the same cap on the stream, and
/// refuses, before the entry is written, a path that is not plain and
/// relative (`rel_of`), one spelled two ways, or a second entry for a file:
/// what lands on disk is exactly what the list names.
fn unpack_capped<R: std::io::Read>(open: impl Fn() -> std::io::Result<R>, dir: &Path, caps: Caps) -> Result<Vec<PackageFile>, String> {
    let tar_err = |e: std::io::Error| format!("tar: {e}");
    let archive = |reader: R| tar::Archive::new(Capped { inner: flate2::read::GzDecoder::new(reader), left: caps.stream });
    let over = |entries: usize, bytes: u64| entries > caps.entries || bytes > caps.bytes;
    let refused = || format!("tar: refused: more than {} entries or {} bytes", caps.entries, caps.bytes);

    let mut raw = archive(open().map_err(tar_err)?);
    let (mut bytes, mut entries) = (0u64, 0usize);
    for entry in raw.entries().map_err(tar_err)?.raw(true) {
        let mut entry = entry.map_err(tar_err)?;
        let kind = entry.header().entry_type();
        let size = entry.header().size().map_err(tar_err)?;
        let name = String::from_utf8_lossy(&entry.path_bytes()).into_owned();
        if kind.is_pax_local_extensions() || kind.is_pax_global_extensions() || kind.is_gnu_longname() {
            if size > MAX_META {
                return Err(format!("tar: refused: a metadata record of {size} bytes, {name}"));
            }
            if let Some(records) = entry.pax_extensions().map_err(tar_err)? {
                for record in records {
                    let record = record.map_err(tar_err)?;
                    if record.key().is_ok_and(|k| k == "size") {
                        return Err(format!("tar: refused: a PAX size override, {name}"));
                    }
                }
            }
            continue;
        }
        // Files and folders only. A link, hard or symbolic, could point a
        // later entry, or the record's own check, outside the folder; the
        // packages have none. Devices, pipes and sparse files have no place
        // in one either.
        if kind.is_symlink() || kind.is_hard_link() || kind.is_gnu_longlink() {
            return Err(format!("tar: refused: a link entry, {name}"));
        }
        if !(kind.is_file() || kind.is_dir()) {
            return Err(format!("tar: refused: an entry of type {:?}, {name}", kind.as_byte() as char));
        }
        entries += 1;
        if kind.is_file() {
            bytes = bytes.saturating_add(size);
        }
        if over(entries, bytes) {
            return Err(refused());
        }
    }

    let mut archive = archive(open().map_err(tar_err)?);
    archive.set_preserve_permissions(true);
    let mut files = std::collections::BTreeMap::new();
    // Each path (and folder on the way) as first spelled, by its lowercase:
    // on a case-insensitive file system two spellings are one file, and the
    // list must name what is on disk.
    let mut spelled = std::collections::HashMap::new();
    let (mut bytes, mut entries) = (0u64, 0usize);
    for entry in archive.entries().map_err(tar_err)? {
        let mut entry = entry.map_err(tar_err)?;
        let kind = entry.header().entry_type();
        // A global PAX record, vetted above, which the reader hands on as is.
        if kind.is_pax_global_extensions() {
            continue;
        }
        // The size the reader will write, a PAX record's if there is one.
        let size = entry.size();
        let path = entry.path().map_err(tar_err)?.into_owned();
        if !(kind.is_file() || kind.is_dir()) {
            return Err(format!("tar: refused: an entry of type {:?}, {}", kind.as_byte() as char, path.display()));
        }
        // Only a plain relative path, checked here before anything is
        // written: the tar reader drops a root or a drive from a path and
        // writes the file anyway, under a name the list would not hold.
        let Some(rel) = rel_of(&path) else {
            if kind.is_dir() && path.components().all(|c| c == Component::CurDir) {
                continue;
            }
            return Err(format!("tar: refused: an entry that is not a plain relative path, {}", path.display()));
        };
        let mut at = String::new();
        for part in rel.split('/') {
            at = if at.is_empty() { part.to_string() } else { format!("{at}/{part}") };
            let first = spelled.entry(at.to_lowercase()).or_insert_with(|| at.clone());
            if *first != at {
                return Err(format!("tar: refused: {at} and {first}, one path in two spellings"));
            }
        }
        if kind.is_file() && files.contains_key(&rel) {
            return Err(format!("tar: refused: a second entry for {rel}"));
        }
        entries += 1;
        if kind.is_file() {
            bytes = bytes.saturating_add(size);
        }
        if over(entries, bytes) {
            return Err(refused());
        }
        if !entry.unpack_in(dir).map_err(|e| format!("unpack: {e}"))? {
            return Err(format!("tar: refused: {rel} was not unpacked"));
        }
        if kind.is_file() {
            files.insert(rel, size);
        }
    }
    Ok(files.into_iter().map(|(rel, size)| PackageFile { rel, size, sha256: None }).collect())
}

/// A tar path as the `/`-separated relative path `cli` keeps; None for one
/// that is not plainly inside the directory: a root, a drive, `..`, a name
/// with `:` (a Windows stream) or `\`, or nothing at all.
fn rel_of(path: &Path) -> Option<String> {
    let mut parts = Vec::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::Normal(p) => parts.push(p.to_str()?.to_string()),
            _ => return None,
        }
    }
    let rel = parts.join("/");
    (!parts.is_empty() && crate::cli::plain_rel(&rel)).then_some(rel)
}

#[cfg(test)]
mod tests {
    use super::{checksum_from_sums, unpack, unpack_capped, Caps, BIN_REL, MAX_BYTES, MAX_ENTRIES};
    use std::io::Write;
    use std::path::{Path, PathBuf};

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
        /// A PAX record for the entry after it.
        Pax(&'a [(&'a str, &'a [u8])]),
        /// A header of `kind` that claims `size` bytes, followed by `data`.
        Claim(tar::EntryType, u64, &'a [u8]),
        /// A file under a name written into its header as is, which the
        /// builder would refuse (a root, a drive, `..`).
        Raw(&'a str, &'a [u8]),
    }

    fn tar_gz(entries: &[Entry]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for entry in entries {
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o755);
            let (path, data, link) = match entry {
                Entry::File(path, bytes) => (*path, *bytes, None),
                Entry::Symlink(path, target) => (*path, &[][..], Some((tar::EntryType::Symlink, target))),
                Entry::HardLink(path, target) => (*path, &[][..], Some((tar::EntryType::Link, target))),
                Entry::Pax(records) => {
                    builder.append_pax_extensions(records.iter().copied()).unwrap();
                    continue;
                }
                Entry::Raw(name, data) => {
                    header.as_gnu_mut().unwrap().name[..name.len()].copy_from_slice(name.as_bytes());
                    if name.ends_with('/') {
                        header.set_entry_type(tar::EntryType::Directory);
                    }
                    header.set_size(data.len() as u64);
                    header.set_cksum();
                    builder.append(&header, *data).unwrap();
                    continue;
                }
                Entry::Claim(kind, size, data) => {
                    header.set_entry_type(*kind);
                    header.set_size(*size);
                    header.set_path("././@LongLink").unwrap();
                    header.set_cksum();
                    builder.get_mut().extend_from_slice(header.as_bytes());
                    builder.get_mut().extend_from_slice(data);
                    continue;
                }
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

    fn unpack_bytes(tarball: &[u8], dir: &Path, caps: Caps) -> Result<Vec<crate::cli::PackageFile>, String> {
        unpack_capped(|| Ok(tarball), dir, caps)
    }

    fn written(dir: &Path) -> usize {
        std::fs::read_dir(dir).unwrap().count()
    }

    /// What `cli::validate` holds the folder to: every file, at its header's
    /// size. A PAX mtime record, as Python's `tarfile` writes for each entry,
    /// and a GNU long name are taken.
    #[test]
    fn a_whole_package_unpacks_listing_each_file_at_its_size() {
        let dir = scratch("whole");
        let (exe, helper) = (vec![7u8; 4000], vec![9u8; 3000]);
        let long = format!("codex-resources/{}/x.dll", "deep".repeat(30));
        let tarball = tar_gz(&[
            Entry::Pax(&[("mtime", b"1762412345.25")]),
            Entry::File(BIN_REL, &exe),
            Entry::File(HELPER, &helper),
            Entry::File(&long, b"dll"),
        ]);
        let files = unpack(|| Ok(&tarball[..]), &dir).unwrap();
        let listed: Vec<_> = files.iter().map(|f| (f.rel.as_str(), f.size)).collect();
        assert_eq!(listed, [(BIN_REL, 4000), (HELPER, 3000), (long.as_str(), 3)]);
        crate::cli::validate(&dir, "0.1.0", BIN_REL, crate::cli::Package { files, verified: "test".into() }).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn link_entries_are_refused() {
        let exe = vec![7u8; 100];
        for link in [Entry::Symlink("codex-path/rg.exe", "../../../outside"), Entry::HardLink("codex-path/rg.exe", "bin/codex.exe")] {
            let dir = scratch("link");
            let refused =
                unpack_bytes(&tar_gz(&[Entry::File(BIN_REL, &exe), link]), &dir, Caps::of(MAX_BYTES, MAX_ENTRIES)).err().expect("refused");
            assert!(refused.contains("a link entry"), "{refused}");
            assert_eq!(written(&dir), 0, "nothing was written");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// A path the tar reader would rewrite, or skip, rather than refuse: a
    /// root, a drive, `..`, a stream; a file spelled two ways, or twice. Each
    /// is refused before it is written, here at the package's very path.
    #[test]
    fn an_entry_that_is_not_one_plain_relative_path_is_refused() {
        let dll = "codex-resources/voice/bin/vcruntime140.dll";
        let rooted = format!("/{dll}");
        let unix_drive = format!("C:/{dll}");
        let loud_name = BIN_REL.replace("codex", "CODEX");
        let windows_drive = format!("C:\\{}", dll.replace('/', "\\"));
        for (bad, why) in [
            (Entry::Raw(&rooted, b"x"), "not a plain relative path"),
            (Entry::Raw(&unix_drive, b"x"), "not a plain relative path"),
            (Entry::Raw(&windows_drive, b"x"), "not a plain relative path"),
            (Entry::Raw("codex-resources/../../evil.dll", b"x"), "not a plain relative path"),
            (Entry::Raw("../evil.dll", b"x"), "not a plain relative path"),
            (Entry::Raw("bin/codex.exe:payload", b"x"), "not a plain relative path"),
            (Entry::File("BIN/codex.exe", b"x"), "one path in two spellings"),
            (Entry::File(&loud_name, b"x"), "one path in two spellings"),
            (Entry::File(BIN_REL, b"again"), "a second entry for bin/codex"),
            (Entry::Symlink(dll, "../../../bin/codex.exe"), "a link entry"),
        ] {
            let dir = scratch("plain");
            let refused =
                unpack_bytes(&tar_gz(&[Entry::File(BIN_REL, b"exe"), bad]), &dir, Caps::of(MAX_BYTES, MAX_ENTRIES)).err().expect("refused");
            assert!(refused.contains(why), "{refused}");
            let _ = std::fs::remove_dir_all(&dir);
        }
        // `./` before a path, and a `./` folder entry, are plain enough.
        let dir = scratch("dot");
        let tarball = tar_gz(&[Entry::Raw("./", b""), Entry::Raw("./bin/codex.exe", b"exe")]);
        let files = unpack_bytes(&tarball, &dir, Caps::of(MAX_BYTES, MAX_ENTRIES)).unwrap();
        assert_eq!(files.iter().map(|f| f.rel.as_str()).collect::<Vec<_>>(), [BIN_REL]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_package_past_its_caps_is_refused_before_anything_is_written() {
        let (exe, helper) = (vec![7u8; 4000], vec![9u8; 3000]);
        let tarball = tar_gz(&[Entry::File(BIN_REL, &exe), Entry::File(HELPER, &helper)]);
        for caps in [Caps::of(5000, MAX_ENTRIES), Caps::of(MAX_BYTES, 1)] {
            let dir = scratch("caps");
            let refused = unpack_bytes(&tarball, &dir, caps).err().expect("refused");
            assert!(refused.contains("refused: more than"), "{refused}");
            assert_eq!(written(&dir), 0);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// A header that claims a few bytes, a PAX record that says more: the
    /// tar reader writes the PAX size, which a budget on the header's would
    /// never see.
    #[test]
    fn a_pax_size_override_is_refused() {
        let dir = scratch("pax-size");
        let tarball = tar_gz(&[Entry::Pax(&[("size", b"5368709120")]), Entry::File(BIN_REL, b"tiny")]);
        let refused = unpack_bytes(&tarball, &dir, Caps::of(MAX_BYTES, MAX_ENTRIES)).err().expect("refused");
        assert!(refused.contains("a PAX size override"), "{refused}");
        assert_eq!(written(&dir), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A PAX record or GNU long name the tar reader would take into memory
    /// whole: refused on its header's word, before a byte of it is read.
    #[test]
    fn a_huge_metadata_record_is_refused_unread() {
        for kind in [tar::EntryType::XHeader, tar::EntryType::GNULongName, tar::EntryType::XGlobalHeader] {
            let dir = scratch("meta");
            let tarball = tar_gz(&[Entry::Claim(kind, 1 << 30, &[b'a'; 1024]), Entry::File(BIN_REL, b"exe")]);
            let refused = unpack_bytes(&tarball, &dir, Caps::of(MAX_BYTES, MAX_ENTRIES)).err().expect("refused");
            assert!(refused.contains("a metadata record of 1073741824 bytes"), "{refused}");
            assert_eq!(written(&dir), 0);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// The decompressed stream itself is capped, whatever the entries say: a
    /// gzip bomb stops at the cap.
    #[test]
    fn the_decompressed_stream_is_capped() {
        let dir = scratch("bomb");
        let tarball = tar_gz(&[Entry::File(BIN_REL, &vec![0u8; 64 << 10])]);
        let caps = Caps { bytes: MAX_BYTES, entries: MAX_ENTRIES, stream: 16 << 10 };
        let refused = unpack_bytes(&tarball, &dir, caps).err().expect("refused");
        assert!(refused.contains("past its cap"), "{refused}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
