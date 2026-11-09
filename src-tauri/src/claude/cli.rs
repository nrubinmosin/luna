//! Where Claude Code releases come from: the same bucket the official
//! installer reads, a plain-text `latest` pointer, a manifest with per-platform
//! sha256s and sizes, signed by Anthropic's release key (`manifest_sig`), and
//! a bare executable per platform.

use crate::cli::{download_to, sane_version, version_cmp, Package, PackageFile, Source};
use crate::provider::Provider;
use std::fs;
use std::io::Read;
use std::path::Path;

const RELEASES: &str = "https://downloads.claude.ai/claude-code-releases";
/// `latest` or `stable` — the same two channels the official installer takes.
const CHANNEL: &str = "latest";

#[cfg(windows)]
const BIN_NAME: &str = "claude.exe";
#[cfg(not(windows))]
const BIN_NAME: &str = "claude";

pub static SOURCE: Source = Source { root_name: "claude-cli", bin_rel: BIN_NAME, path_fallback: "claude", latest, fetch };

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

/// The oldest Claude Code a fresh tree takes: the version Luna was last
/// checked live with (docs/cli-compatibility.md). The `latest` pointer is not
/// signed, so without a floor a bucket in other hands could offer an old
/// signed release with its known flaws.
const MIN_VERSION: &str = "2.1.319";
/// `<root>/signed-floor`: the highest version whose signed manifest Luna has
/// accepted. Nothing below it, or below `MIN_VERSION`, is installed.
const FLOOR: &str = "signed-floor";

fn latest(agent: &ureq::Agent) -> Result<String, String> {
    let text = agent
        .get(&format!("{RELEASES}/{CHANNEL}"))
        .call()
        .map_err(|e| format!("version lookup: {e}"))?
        .into_string()
        .map_err(|e| e.to_string())?;
    let version = sane_version(&text, CHANNEL)?;
    // Refused here as well as at the manifest: an older recorded install
    // left in `versions/` would otherwise be flipped back to with no fetch.
    at_or_above_floor(&crate::cli::root(Provider::Claude), &version)?;
    Ok(version)
}

/// The floor: `MIN_VERSION`, or the highest signed version accepted, if
/// that is higher.
fn floor(root: &Path) -> String {
    let kept = fs::read_to_string(root.join(FLOOR)).ok().and_then(|t| sane_version(&t, FLOOR).ok());
    match kept {
        Some(v) if version_cmp(&v, MIN_VERSION).is_gt() => v,
        _ => MIN_VERSION.to_string(),
    }
}

fn at_or_above_floor(root: &Path, version: &str) -> Result<(), String> {
    let floor = floor(root);
    if version_cmp(version, &floor).is_lt() {
        return Err(format!(
            "refusing Claude Code {version}: older than {floor}, the highest signed version Luna has accepted (or the oldest it takes)"
        ));
    }
    Ok(())
}

/// Moves the floor up to `version`, never down.
fn raise_floor(root: &Path, version: &str) -> Result<(), String> {
    let kept = fs::read_to_string(root.join(FLOOR)).unwrap_or_default();
    if version_cmp(version, &floor(root)).is_lt() || kept.trim() == version {
        return Ok(());
    }
    fs::create_dir_all(root).map_err(|e| format!("mkdir {}: {e}", root.display()))?;
    crate::secret::write_private_synced(&root.join(FLOOR), version.as_bytes()).map_err(|e| format!("write {FLOOR}: {e}"))
}

/// The manifest fetched for `version`, taken only if its signature holds,
/// it names `version` itself (a signed manifest of another release served
/// under this one's URLs is a relabel), and `version` is not below the
/// floor; then the floor moves up to it. Gives the exe's sha256 and size.
fn accept_manifest(root: &Path, bytes: &[u8], sig: &[u8], version: &str) -> Result<(String, Option<u64>), String> {
    at_or_above_floor(root, version)?;
    super::manifest_sig::verify(bytes, sig)?;
    let manifest: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| format!("manifest: {e}"))?;
    let signed_for = manifest["version"].as_str().unwrap_or_default();
    if signed_for != version {
        return Err(format!("the manifest served for {version} is signed for {signed_for:?}: refused"));
    }
    let entry = &manifest["platforms"][platform()];
    let checksum = entry["checksum"]
        .as_str()
        .map(|s| s.to_ascii_lowercase())
        .ok_or_else(|| format!("platform {} not in manifest for {version}", platform()))?;
    raise_floor(root, version)?;
    Ok((checksum, entry["size"].as_u64()))
}

/// A small file off the bucket, whole and as served: the signature is over
/// the manifest's exact bytes.
fn get_small(agent: &ureq::Agent, url: &str, what: &str) -> Result<Vec<u8>, String> {
    const LIMIT: u64 = 1 << 20;
    let resp = agent.get(url).call().map_err(|e| format!("{what}: {e}"))?;
    let mut bytes = Vec::new();
    resp.into_reader().take(LIMIT + 1).read_to_end(&mut bytes).map_err(|e| format!("{what}: {e}"))?;
    if bytes.len() as u64 > LIMIT {
        return Err(format!("{what}: larger than {LIMIT} bytes"));
    }
    Ok(bytes)
}

/// The sha256 and the size the manifest gives our platform's exe, once the
/// manifest is accepted (`accept_manifest`). Manifests carry both; the size
/// is taken as optional all the same, the hash being what pins the file.
fn expected(agent: &ureq::Agent, version: &str) -> Result<(String, Option<u64>), String> {
    let bytes = get_small(agent, &format!("{RELEASES}/{version}/manifest.json"), "manifest")?;
    let sig = get_small(agent, &format!("{RELEASES}/{version}/manifest.json.sig"), "manifest signature")?;
    accept_manifest(&crate::cli::root(Provider::Claude), &bytes, &sig, version)
}

/// Downloads `version` into `dir`, verifying the sha256 from the manifest
/// before the file gets its real name. The package is that one file.
fn fetch(agent: &ureq::Agent, version: &str, dir: &Path, progress: &mut dyn FnMut(u64, Option<u64>)) -> Result<Package, String> {
    let (checksum, size) = expected(agent, version)?;
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
    let signed = format!("{version}/manifest.json, signed by the {}", super::manifest_sig::KEY_NAME);
    let (size, verified) = match size {
        Some(size) => (size, format!("sha256 and size from {signed}")),
        None => (fs::metadata(&dest).map_err(|e| e.to_string())?.len(), format!("sha256 from {signed}")),
    };
    Ok(Package { files: vec![PackageFile { rel: BIN_NAME.into(), size, sha256: Some(checksum) }], verified })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // A real release's signed manifest (see manifest_sig.rs).
    const MANIFEST: &[u8] = include_bytes!("testdata/manifest-2.1.319.json");
    const SIG: &[u8] = include_bytes!("testdata/manifest-2.1.319.json.sig");

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("luna-claude-floor-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn versions_order_by_their_numbers() {
        use std::cmp::Ordering::*;
        assert_eq!(version_cmp("2.1.100", "2.1.99"), Greater);
        assert_eq!(version_cmp("2.1.319", "2.1.319"), Equal);
        assert_eq!(version_cmp("2.1", "2.1.0"), Equal);
        assert_eq!(version_cmp("2.0.999", "2.1.0"), Less);
        assert_eq!(version_cmp("10.0.0", "9.9.9"), Greater);
    }

    #[test]
    fn a_signed_manifest_served_under_another_version_is_refused() {
        let root = scratch("relabel");
        let refused = accept_manifest(&root, MANIFEST, SIG, "2.1.320").unwrap_err();
        assert!(refused.contains("signed for \"2.1.319\""), "{refused}");
        assert!(!root.join(FLOOR).exists(), "a refused manifest moves no floor");

        let (checksum, size) = accept_manifest(&root, MANIFEST, SIG, "2.1.319").unwrap();
        assert_eq!(checksum.len(), 64);
        assert!(size.is_some());
        assert_eq!(fs::read_to_string(root.join(FLOOR)).unwrap(), "2.1.319");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_signed_release_below_the_floor_is_refused() {
        let root = scratch("replay");
        fs::write(root.join(FLOOR), "2.1.330").unwrap();
        let refused = accept_manifest(&root, MANIFEST, SIG, "2.1.319").unwrap_err();
        assert!(refused.contains("refusing Claude Code 2.1.319: older than 2.1.330"), "{refused}");
        at_or_above_floor(&root, "2.1.331").unwrap();
        // The floor only rises.
        raise_floor(&root, "2.1.320").unwrap();
        assert_eq!(floor(&root), "2.1.330");
        raise_floor(&root, "2.1.340").unwrap();
        assert_eq!(floor(&root), "2.1.340");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_fresh_tree_takes_nothing_below_the_shipped_minimum() {
        let root = scratch("fresh");
        assert_eq!(floor(&root), MIN_VERSION);
        assert!(at_or_above_floor(&root, "2.1.300").is_err());
        assert!(at_or_above_floor(&root, "2.0.999").is_err());
        at_or_above_floor(&root, MIN_VERSION).unwrap();
        at_or_above_floor(&root, "2.2.0").unwrap();
        // A floor file below the minimum does not lower it.
        fs::write(root.join(FLOOR), "1.0.0").unwrap();
        assert_eq!(floor(&root), MIN_VERSION);
        let _ = fs::remove_dir_all(&root);
    }
}
