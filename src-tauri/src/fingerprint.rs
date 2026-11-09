//! The core's fingerprint: a hash of everything the Rust side is built from,
//! save the version number. Two builds with the same fingerprint run the same
//! Rust code, whatever their versions and frontends; an update that only
//! touched the frontend keeps it, and a window attaches to a running core only
//! of its own fingerprint (corelink.rs `verdict`).
//!
//! build.rs includes this file (`#[path]`) to stamp the binary; the crate
//! compiles it for its tests alone, which hold it to every input the core is
//! compiled from.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// What the core is compiled from beside `src/`, `Cargo.toml` and
/// `Cargo.lock`, relative to the crate: build.rs itself, and the files an
/// `include_str!` or `include_bytes!` outside a test reaches outside `src/` —
/// the CLI policy, whose `minSafe` a release may raise alone (cli/known_good.rs).
/// What `tauri::generate_context!` compiles in (`tauri.conf.json`,
/// `capabilities/`, `permissions/`, `icons/`, the frontend) is the window's: the
/// core never builds a Tauri app (lib.rs `run`, coreproc.rs).
pub const INPUTS: &[&str] = &["build.rs", "cli-known-good.json"];

/// The fingerprint of the crate at `root`, its `LUNA_FLAVOR` being `flavor`.
/// Line endings are hashed as LF so a Windows checkout and the Docker mount of
/// it agree.
pub fn core_fingerprint(root: &Path, flavor: &str) -> String {
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
    let text = |p: &Path| std::fs::read_to_string(root.join(p)).unwrap_or_default().replace("\r\n", "\n");

    let mut files = Vec::new();
    walk(&root.join("src"), &mut files);
    let mut files: Vec<PathBuf> = files.into_iter().filter_map(|f| f.strip_prefix(root).ok().map(Path::to_path_buf)).collect();
    files.sort();
    let mut h = Sha256::new();
    for f in &files {
        h.update(f.to_string_lossy().replace('\\', "/").as_bytes());
        h.update(text(f).as_bytes());
    }
    for input in INPUTS {
        h.update(text(Path::new(input)).as_bytes());
    }
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
    h.update(flavor.trim().as_bytes());
    h.finalize().iter().take(8).map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("luna-fingerprint-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        dir
    }

    /// A release that changes only the CLI policy (`cli-known-good.json`, its
    /// `minSafe`) is a new core: a window of it must not attach to the old
    /// one, which would go on enforcing the old policy.
    #[test]
    fn a_policy_only_change_is_a_new_core() {
        let root = scratch("policy");
        for (f, text) in [
            ("src/lib.rs", "fn main() {}\n"),
            ("build.rs", "fn main() {}\n"),
            ("Cargo.toml", "[package]\nname = \"luna\"\nversion = \"1.0.0\"\n"),
            ("Cargo.lock", "[[package]]\nname = \"luna\"\nversion = \"1.0.0\"\n"),
            ("cli-known-good.json", "{\"claude\":{\"minSafe\":\"2.1.300\"}}\n"),
        ] {
            std::fs::write(root.join(f), text).unwrap();
        }
        let before = core_fingerprint(&root, "");
        std::fs::write(root.join("cli-known-good.json"), "{\"claude\":{\"minSafe\":\"2.1.319\"}}\n").unwrap();
        assert_ne!(core_fingerprint(&root, ""), before, "the policy is compiled in");
        // A version bump alone is not.
        let after = core_fingerprint(&root, "");
        std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"luna\"\nversion = \"1.0.1\"\n").unwrap();
        std::fs::write(root.join("Cargo.lock"), "[[package]]\nname = \"luna\"\nversion = \"1.0.1\"\n").unwrap();
        assert_eq!(core_fingerprint(&root, ""), after);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `text` without its inline modules gated on `test` (`#[cfg(test)]`,
    /// `#[cfg(all(test, …))]`), found by their braces: what a build of the
    /// app compiles of the file. Braces inside the tests' strings come in
    /// pairs, as format strings do.
    fn shipped(text: &str) -> String {
        let mut out = String::new();
        let mut lines = text.lines().peekable();
        while let Some(line) = lines.next() {
            let t = line.trim_start();
            let gated = t.starts_with("#[cfg(") && t.contains("test") && !t.contains("not(test");
            if gated && lines.peek().is_some_and(|l| l.trim_start().starts_with("mod ") && l.trim_end().ends_with('{')) {
                let mut depth = 0i32;
                for l in lines.by_ref() {
                    depth += l.matches('{').count() as i32 - l.matches('}').count() as i32;
                    if depth <= 0 {
                        break;
                    }
                }
                continue;
            }
            out.push_str(line);
            out.push('\n');
        }
        out
    }

    #[test]
    fn a_test_module_is_not_what_ships() {
        let text = "const A: &str = include_str!(\"a\");\n#[cfg(all(test, not(windows)))]\nmod acl {\n    fn f() { format!(\"{x}\"); }\n    \
                    const B: &str = include_str!(\"b\");\n}\nconst C: u8 = 0;\n#[cfg(test)]\nmod tests {\n    const D: &str = include_str!(\"d\");\n}\n";
        assert_eq!(shipped(text), "const A: &str = include_str!(\"a\");\nconst C: u8 = 0;\n");
    }

    /// Every file an `include_str!` / `include_bytes!` outside a test module
    /// compiles in from outside `src/` is one of `INPUTS`, so a change to it
    /// moves the fingerprint. A path built with `concat!` is not read here:
    /// those are test fixtures (session/fixtures.rs, coreproc.rs tests), and
    /// neither is a file that is itself a test module (golden.rs, live.rs),
    /// which reads none outside `src/`. Not on Windows: it reads the checkout
    /// as it runs, and the Windows test executables run away from the
    /// container they were built in.
    #[cfg(not(windows))]
    #[test]
    fn every_file_the_core_compiles_in_is_fingerprinted() {
        let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let src = crate_dir.join("src");
        let mut files = Vec::new();
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push(p);
                }
            }
        }
        walk(&src, &mut files);
        let mut outside = Vec::new();
        for f in &files {
            let text = std::fs::read_to_string(f).unwrap().replace("\r\n", "\n");
            let shipped = shipped(&text);
            for mac in ["include_str!(\"", "include_bytes!(\""] {
                for (i, _) in shipped.match_indices(mac) {
                    let rest = &shipped[i + mac.len()..];
                    let rel = &rest[..rest.find('"').unwrap()];
                    let target = f.parent().unwrap().join(rel);
                    let target = std::fs::canonicalize(&target).unwrap_or_else(|e| panic!("{}: {rel}: {e}", f.display()));
                    if !target.starts_with(std::fs::canonicalize(&src).unwrap()) {
                        let rel = target.strip_prefix(std::fs::canonicalize(crate_dir).unwrap()).unwrap_or(&target);
                        outside.push(rel.to_string_lossy().replace('\\', "/"));
                    }
                }
            }
        }
        assert!(!outside.is_empty(), "the scan found nothing: cli-known-good.json at least is compiled in");
        for f in &outside {
            assert!(INPUTS.contains(&f.as_str()), "{f} is compiled into the core but not in fingerprint::INPUTS");
        }
    }
}
