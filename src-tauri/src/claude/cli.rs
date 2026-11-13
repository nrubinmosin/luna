//! Where Claude Code releases come from: the same bucket the official
//! installer reads, a plain-text `latest` pointer, a manifest with per-platform
//! sha256s and sizes, signed by Anthropic's release key (`manifest_sig`), and
//! a bare executable per platform.

use crate::cli::{download_to, sane_version, Package, PackageFile, Source};
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

pub static SOURCE: Source = Source { root_name: "claude-cli", bin_rel: BIN_NAME, latest, fetch };

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

/// The `latest` pointer is not signed: what it names is held to the floor
/// (cli.rs `update_in`), which keeps a bucket in other hands from offering an
/// old signed release with its known flaws.
fn latest(agent: &ureq::Agent) -> Result<String, String> {
    let text = agent
        .get(&format!("{RELEASES}/{CHANNEL}"))
        .call()
        .map_err(|e| format!("version lookup: {e}"))?
        .into_string()
        .map_err(|e| e.to_string())?;
    sane_version(&text, CHANNEL)
}

/// The manifest fetched for `version`, taken only if `version` is not below
/// the floor (cli.rs) or `min_safe`, its signature holds, and it names
/// `version` itself (a signed manifest of another release served under this
/// one's URLs is a relabel). Gives the exe's sha256 and size. The floor moves
/// only once a version becomes current, not here.
fn accept_manifest(root: &Path, min_safe: &str, bytes: &[u8], sig: &[u8], version: &str) -> Result<(String, Option<u64>), String> {
    crate::cli::at_or_above_floor_in(root, min_safe, version)?;
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
    accept_manifest(&crate::cli::root(Provider::Claude), crate::cli::min_safe(Provider::Claude), &bytes, &sig, version)
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
    /// A minimum the fixture's release meets, whatever the shipped list's is.
    const MIN: &str = "2.1.319";

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("luna-claude-floor-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_signed_manifest_served_under_another_version_is_refused() {
        let root = scratch("relabel");
        let refused = accept_manifest(&root, MIN, MANIFEST, SIG, "2.1.320").unwrap_err();
        assert!(refused.contains("signed for \"2.1.319\""), "{refused}");

        let (checksum, size) = accept_manifest(&root, MIN, MANIFEST, SIG, "2.1.319").unwrap();
        assert_eq!(checksum.len(), 64);
        assert!(size.is_some());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0, "a manifest taken moves no floor: a version does, once current");
        let _ = fs::remove_dir_all(&root);
    }

    /// The floor as Claude Code's tree had it before it rose on switch
    /// (`signed-floor`) still holds a signed release below it out.
    #[test]
    fn a_signed_release_below_the_floor_is_refused() {
        let root = scratch("replay");
        fs::write(root.join("signed-floor"), "2.1.330").unwrap();
        let refused = accept_manifest(&root, MIN, MANIFEST, SIG, "2.1.319").unwrap_err();
        assert!(refused.contains("refusing 2.1.319: older than 2.1.330"), "{refused}");
        let _ = fs::remove_dir_all(&root);
    }

    /// A Luna whose `minSafe` is above a release takes no manifest of it,
    /// signed or not, on a fresh tree: the pointer cannot bring it back.
    #[test]
    fn a_signed_release_below_min_safe_is_refused() {
        let root = scratch("min-safe");
        let refused = accept_manifest(&root, "2.1.323", MANIFEST, SIG, "2.1.319").unwrap_err();
        assert!(refused.contains("refusing 2.1.319: older than 2.1.323"), "{refused}");
        let _ = fs::remove_dir_all(&root);
    }
}
