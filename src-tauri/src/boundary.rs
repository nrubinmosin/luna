//! Paths that arrive from the page, checked where they come in. The page is
//! the least trusted thing that can drive the core (a script injected into it
//! would have all of its authority, threat model B1), so a path it names is
//! held to its shape before anything opens, writes or starts a process there:
//! absolute, every part a plain name (no `.`, no `..`, no device or verbatim
//! prefix, no stream suffix), and an account folder must be one Luna lists.
//!
//! The checks are on the path's text, plus one look at the account folder
//! itself; what lies under a checked path is the commands' own business
//! (worktree removal keeps its own checks, worktree/paths.rs).

use serde::Deserialize;
use std::path::{Component, Path, PathBuf};

/// Longer than any path Windows or Linux opens.
const MAX_PATH: usize = 32_768;

/// An absolute, normalized path: a project folder, a worktree, a session's
/// folder. Refuses `.` and `..` parts outright rather than resolving them, so
/// what is checked is what gets used.
pub fn absolute(s: &str) -> Result<PathBuf, String> {
    let bad = |why: &str| Err(format!("not a usable path ({why})"));
    if s.is_empty() || s.len() > MAX_PATH {
        return bad("empty or too long");
    }
    if s.chars().any(char::is_control) {
        return bad("control characters");
    }
    let path = Path::new(s);
    if !path.is_absolute() {
        return bad("not absolute");
    }
    for part in s.split(['/', '\\']) {
        if part == "." || part == ".." {
            return bad("a . or .. part");
        }
    }
    for c in path.components() {
        match c {
            Component::RootDir | Component::Normal(_) => {}
            #[cfg(windows)]
            Component::Prefix(p) => {
                use std::path::Prefix;
                // `\\?\` skips the normalizing the checks here rely on, and
                // `\\.\` names devices, not folders.
                if !matches!(p.kind(), Prefix::Disk(_) | Prefix::UNC(..)) {
                    return bad("a device or verbatim prefix");
                }
            }
            _ => return bad("a . or .. part"),
        }
    }
    // A colon past the drive is an NTFS stream (`folder:name`), not a folder.
    #[cfg(windows)]
    if s.get(2..).is_some_and(|rest| rest.contains(':')) {
        return bad("a stream name");
    }
    Ok(path.to_path_buf())
}

/// An account folder: `<root>/<provider dir>/<name>` for one of `roots` (the
/// providers' folders under the accounts root), a valid account name, and a
/// folder of its own, not a link to one elsewhere. The same folders
/// `list_accounts` lists, which is where the page gets every account path.
pub fn account_in(s: &str, roots: &[PathBuf]) -> Result<PathBuf, String> {
    let path = absolute(s)?;
    let parent = path.parent().map(|p| crate::paths::path_key(&p.to_string_lossy()));
    let under = roots.iter().any(|r| Some(crate::paths::path_key(&r.to_string_lossy())) == parent);
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if !under || crate::accounts::validate_name(&name).is_err() {
        return Err("not an account folder of Luna's".into());
    }
    match std::fs::symlink_metadata(&path) {
        Ok(m) if m.is_dir() && !m.file_type().is_symlink() => Ok(path),
        Ok(_) => Err("not an account folder of Luna's (a link or a file)".into()),
        Err(e) => Err(format!("not an account folder of Luna's: {e}")),
    }
}

/// `account_in` under the accounts root in use.
pub fn account(s: &str) -> Result<PathBuf, String> {
    let roots = crate::provider::Provider::ALL.iter().map(|p| crate::accounts::provider_root(*p)).collect::<Result<Vec<_>, _>>()?;
    account_in(s, &roots)
}

/// A path argument the page sends, checked as it is deserialized: a command
/// that takes one cannot be reached with a path that fails `absolute`.
#[derive(Deserialize, Debug)]
#[serde(try_from = "String")]
pub struct AbsPath(String);

impl TryFrom<String> for AbsPath {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        absolute(&s).map(|_| Self(s))
    }
}

impl From<AbsPath> for String {
    fn from(p: AbsPath) -> String {
        p.0
    }
}

/// An account path argument, checked as `account`.
#[derive(Deserialize, Debug)]
#[serde(try_from = "String")]
pub struct AccountPath(String);

impl TryFrom<String> for AccountPath {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        account(&s).map(|_| Self(s))
    }
}

impl From<AccountPath> for String {
    fn from(p: AccountPath) -> String {
        p.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn abs(rel: &str) -> String {
        if cfg!(windows) {
            format!(r"C:\{}", rel.replace('/', r"\"))
        } else {
            format!("/{rel}")
        }
    }

    #[test]
    fn plain_absolute_paths_pass() {
        for p in [abs("p/luna"), abs("p/luna/.claude/worktrees/x"), abs("My Projects/a b")] {
            assert!(absolute(&p).is_ok(), "{p}");
        }
        #[cfg(windows)]
        assert!(absolute(r"\\server\share\p").is_ok());
    }

    #[test]
    fn traversal_relative_and_odd_paths_are_refused() {
        let mut bad = vec![
            String::new(),
            "relative/dir".into(),
            "..".into(),
            abs("p/../etc"),
            abs("p/./x"),
            abs("p/.."),
            format!("{}\0x", abs("p")),
            format!("{}\nx", abs("p")),
            "x".repeat(MAX_PATH + 1),
        ];
        if cfg!(windows) {
            bad.extend([r"\\?\C:\p".into(), r"\\.\PhysicalDrive0".into(), r"C:\p\x:stream".into(), r"C:p".into(), r"\p".into()]);
        }
        for p in bad {
            assert!(absolute(&p).is_err(), "{p:?} was let through");
        }
    }

    #[test]
    fn only_a_listed_account_folder_passes() {
        let root = std::env::temp_dir().join(format!("luna-boundary-{}", std::process::id()));
        let anthropic = root.join("anthropic");
        std::fs::create_dir_all(anthropic.join("work")).unwrap();
        std::fs::create_dir_all(root.join("elsewhere")).unwrap();
        std::fs::write(anthropic.join("file"), "").unwrap();
        let roots = [anthropic.clone(), root.join("openai")];
        let s = |p: PathBuf| p.to_string_lossy().into_owned();

        assert!(account_in(&s(anthropic.join("work")), &roots).is_ok());
        // Outside the providers' folders, through `..`, a file, one not there.
        for p in [
            s(root.join("elsewhere")),
            s(root.clone()),
            format!("{}{}..{}elsewhere", s(anthropic.join("work")), std::path::MAIN_SEPARATOR, std::path::MAIN_SEPARATOR),
            s(anthropic.join("file")),
            s(anthropic.join("missing")),
            s(anthropic.join("work").join("deeper")),
        ] {
            assert!(account_in(&p, &roots).is_err(), "{p} was let through");
        }
        // A link in an account's place, to a folder elsewhere.
        let link = anthropic.join("linked");
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(root.join("elsewhere"), &link).is_ok();
        #[cfg(windows)]
        let linked = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&link)
            .arg(root.join("elsewhere"))
            .output()
            .is_ok_and(|o| o.status.success());
        if linked {
            assert!(account_in(&s(link.clone()), &roots).is_err(), "a link was let through");
        }
        let _ = std::fs::remove_dir(&link).or_else(|_| std::fs::remove_file(&link));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn arguments_are_checked_as_they_are_read() {
        assert!(serde_json::from_value::<AbsPath>(serde_json::json!(abs("p"))).is_ok());
        let err = serde_json::from_value::<AbsPath>(serde_json::json!(abs("p/../q"))).unwrap_err().to_string();
        assert!(err.contains("not a usable path"), "{err}");
        let err = serde_json::from_value::<AccountPath>(serde_json::json!(abs("not-an-account"))).unwrap_err().to_string();
        assert!(err.contains("account folder"), "{err}");
    }
}
