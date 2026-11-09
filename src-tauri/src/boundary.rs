//! Paths that arrive from the page, checked where they come in. The page is
//! the least trusted thing that can drive the core (a script injected into it
//! would have all of its authority, threat model B1), so a path it names is
//! held to its shape before anything opens, writes or starts a process there:
//! absolute, every part a plain name (no `.`, no `..`, no device or verbatim
//! prefix, no stream suffix), and an account folder must be one Luna lists.
//!
//! A folder is checked on its text. An account folder is also checked on
//! disk: no link or junction between the accounts root and it, and the
//! folder the system resolves it to is that very one under the root as the
//! system resolves the root. What lies under a checked path is the commands'
//! own business (worktree removal keeps its own checks, worktree/paths.rs).
//! The look on disk comes before the command opens the path again, a race a
//! same-user process could win (threat model R6).
//!
//! A file a less trusted side put somewhere for Luna to read (a session's
//! handoff, the library's override of the handoff's instructions) is opened
//! as itself, never through a link at its name, and without waiting on a FIFO
//! (`open_no_follow`, `is_link`). A folder something is written into, moved
//! out of or given to a CLI is checked from a folder Luna trusts down, each
//! step before the next (`walk_down`, `private_chain`).

use crate::provider::Provider;
use serde::{Deserialize, Deserializer};
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

/// An account folder: `<root>/<provider dir>/<name>` with a valid account
/// name, the folders `list_accounts` lists, which is where the page gets
/// every account path. On disk:
///
/// - neither the provider's folder nor the account's is a link, junction or
///   mount point (`symlink_metadata`, which on Windows opens the path with
///   `FILE_FLAG_OPEN_REPARSE_POINT`; Windows counts any name-surrogate
///   reparse point as a link). The root itself may be one: it is the user's
///   choice in Settings;
/// - the account resolves (`canonicalize`: `GetFinalPathNameByHandleW` on the
///   opened folder on Windows, `realpath` elsewhere) to a folder directly in
///   a provider's folder of the root as it resolves, under the very name
///   given. Another spelling of the same folder (Windows' `name.` or
///   `name `, an 8.3 short name, other case) is refused rather than trusted.
pub fn account_in(s: &str, root: &Path) -> Result<PathBuf, String> {
    let refuse = |why: &str| Err(format!("not an account folder of Luna's ({why})"));
    // `<account>/` has lstat follow a link in the account's place.
    if s.ends_with(['/', '\\']) {
        return refuse("a trailing separator");
    }
    let path = absolute(s)?;
    let (Some(provider), Some(name)) = (path.parent(), path.file_name()) else { return refuse("no name") };
    let key = |p: &Path| crate::paths::path_key(&p.to_string_lossy());
    let in_root = provider.parent().is_some_and(|r| key(r) == key(root));
    let provider_dir = provider.file_name().map(|d| key(Path::new(d))).unwrap_or_default();
    let known = Provider::ALL.iter().any(|p| key(Path::new(p.accounts_dir())) == provider_dir);
    if !in_root || !known || crate::accounts::validate_name(&name.to_string_lossy()).is_err() {
        return refuse("not in a provider's folder of the accounts root");
    }
    for dir in [provider, path.as_path()] {
        match std::fs::symlink_metadata(dir) {
            Ok(m) if m.is_dir() && !m.file_type().is_symlink() => {}
            Ok(_) => return refuse(&format!("{} is a link or a file", dir.display())),
            Err(e) => return Err(format!("not an account folder of Luna's: {e}")),
        }
    }
    let real = std::fs::canonicalize(&path).map_err(|e| format!("not an account folder of Luna's: {e}"))?;
    let real_root = std::fs::canonicalize(root).map_err(|e| format!("the accounts root: {e}"))?;
    if real.file_name() != Some(name) || real.parent().and_then(Path::parent) != Some(real_root.as_path()) {
        return refuse("another spelling of it, or a folder elsewhere");
    }
    Ok(path)
}

/// `account_in` under the accounts root in use.
pub fn account(s: &str) -> Result<PathBuf, String> {
    account_in(s, &crate::accounts::accounts_root()?)
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

/// A path argument the page may leave empty, meaning none (no account
/// chosen yet, no folder): `""` passes as itself, anything else is checked
/// as `T`.
#[derive(Debug)]
pub struct OrEmpty<T>(Option<T>);

impl<'de, T: TryFrom<String, Error = String>> Deserialize<'de> for OrEmpty<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        if s.is_empty() {
            return Ok(Self(None));
        }
        T::try_from(s).map(|t| Self(Some(t))).map_err(serde::de::Error::custom)
    }
}

impl<T: Into<String>> OrEmpty<T> {
    /// The checked path, or `""` where none was given.
    pub fn into_string(self) -> String {
        self.0.map(Into::into).unwrap_or_default()
    }
}

/// A link or a junction, whatever std calls it: Windows counts any reparse
/// point that stands for another name as one.
pub fn is_link(meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    meta.file_type().is_symlink()
}

/// `top` and the folders `below` it, in turn, each handed to `step` (with its
/// depth, `top` 0) before the next is looked at; the innermost. A check of
/// the last folder alone would let a link planted above it lead whatever is
/// done there elsewhere. Each caller brings its own step: `private_chain`'s
/// makes Luna's own folders private, the library's (library/system.rs
/// `inside`) only looks.
pub fn walk_down(top: &Path, below: &[&str], mut step: impl FnMut(&Path, usize) -> Result<(), String>) -> Result<PathBuf, String> {
    let mut dir = top.to_path_buf();
    step(&dir, 0)?;
    for (i, name) in below.iter().enumerate() {
        dir.push(name);
        step(&dir, i + 1)?;
    }
    Ok(dir)
}

/// `top` and the folders `below` it, Luna's own, each made where it is not
/// there yet and checked where it is before anything is made below it: a
/// folder, no link or junction, this user's, private (`secret::private_dir`,
/// which checks the one folder it is given). The innermost.
pub fn private_chain(top: &Path, below: &[&str]) -> Result<PathBuf, String> {
    walk_down(top, below, |d, _| crate::secret::private_dir(d).map_err(|e| format!("{} is no private folder of Luna's: {e}", d.display())))
}

/// `private_chain` to a folder Luna gives a session's CLI to write in (a
/// chat's handoff drafts): the same walk, the innermost checked by
/// `secret::sandbox_dir`, which leaves the grant the CLI's sandbox gave itself
/// there and nothing else.
pub fn sandbox_chain(top: &Path, below: &[&str]) -> Result<PathBuf, String> {
    walk_down(top, below, |d, depth| {
        let made = if depth == below.len() { crate::secret::sandbox_dir(d) } else { crate::secret::private_dir(d) };
        made.map_err(|e| format!("{} is no private folder of Luna's: {e}", d.display()))
    })
}

/// The file at `path` itself, for reading a file someone other than Luna put
/// there (a session's handoff, the library's override of the handoff's
/// instructions): a link there opened as the link (and then refused by its
/// metadata, `is_link`) rather than followed. On Unix without waiting: a FIFO
/// put at the name blocks an open for reading until a writer comes, and is
/// then refused by the handle's metadata like any other thing that is not a
/// file. (Windows takes no FIFO at a path `absolute` lets through: its pipes
/// live under `\\.\pipe\`.)
pub fn open_no_follow(path: &Path) -> std::io::Result<std::fs::File> {
    let mut o = std::fs::OpenOptions::new();
    o.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        o.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    o.open(path)
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

    /// A symlink on Unix, a junction on Windows (no privilege needed).
    fn link_dir(link: &Path, target: &Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).unwrap();
        #[cfg(windows)]
        assert!(std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .is_ok_and(|o| o.status.success()));
    }

    fn unlink_dir(link: &Path) {
        // Removes the link itself, never what it points at.
        let _ = std::fs::remove_dir(link).or_else(|_| std::fs::remove_file(link));
    }

    fn s(p: &Path) -> String {
        p.to_string_lossy().into_owned()
    }

    fn scratch(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("luna-boundary-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("accounts").join("anthropic").join("work")).unwrap();
        std::fs::create_dir_all(root.join("elsewhere").join("work")).unwrap();
        root
    }

    #[test]
    fn only_a_listed_account_folder_passes() {
        let root = scratch("listed");
        let accounts = root.join("accounts");
        let anthropic = accounts.join("anthropic");
        std::fs::write(anthropic.join("file"), "").unwrap();

        assert!(account_in(&s(&anthropic.join("work")), &accounts).is_ok());
        let sep = std::path::MAIN_SEPARATOR;
        // Outside the providers' folders, through `..`, a file, one not there,
        // one level too deep, a provider Luna does not know.
        std::fs::create_dir_all(accounts.join("other").join("work")).unwrap();
        for p in [
            s(&root.join("elsewhere").join("work")),
            s(&accounts),
            s(&anthropic),
            format!("{}{sep}..{sep}elsewhere", s(&anthropic.join("work"))),
            s(&anthropic.join("file")),
            s(&anthropic.join("missing")),
            s(&anthropic.join("work").join("deeper")),
            s(&accounts.join("other").join("work")),
        ] {
            assert!(account_in(&p, &accounts).is_err(), "{p} was let through");
        }
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_link_in_an_accounts_place_is_refused_with_or_without_a_trailing_separator() {
        let root = scratch("link");
        let accounts = root.join("accounts");
        let link = accounts.join("anthropic").join("linked");
        link_dir(&link, &root.join("elsewhere").join("work"));
        // With the separator, lstat would follow the link to its target.
        for p in [s(&link), format!("{}/", s(&link)), format!("{}{}", s(&link), std::path::MAIN_SEPARATOR)] {
            assert!(account_in(&p, &accounts).is_err(), "{p} was let through");
        }
        // A real account with a trailing separator is refused too.
        assert!(account_in(&format!("{}/", s(&accounts.join("anthropic").join("work"))), &accounts).is_err());
        unlink_dir(&link);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_provider_folder_that_is_a_link_is_refused() {
        // `openai` itself leads elsewhere: an account folder in it passes
        // every check on its own, yet lies outside the root.
        let root = scratch("provider");
        let accounts = root.join("accounts");
        let openai = accounts.join("openai");
        link_dir(&openai, &root.join("elsewhere"));
        let err = account_in(&s(&openai.join("work")), &accounts).unwrap_err();
        assert!(err.contains("link"), "{err}");
        unlink_dir(&openai);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn other_spellings_of_an_account_are_refused() {
        let root = scratch("spelling");
        let accounts = root.join("accounts");
        let anthropic = accounts.join("anthropic");
        let sep = std::path::MAIN_SEPARATOR;
        let tail = format!("{sep}anthropic{sep}work");
        // Windows drops a trailing dot or space and opens the folder itself,
        // and ignores case; elsewhere these name folders that are not there.
        for p in [
            s(&anthropic.join("work.")),
            s(&anthropic.join("work ")),
            s(&anthropic.join("WORK")),
            s(&accounts.join("anthropic.").join("work")),
            format!("{}.{tail}", s(&accounts)),
            format!("{} {tail}", s(&accounts)),
            format!("{}{tail}", s(&root.join("Accounts."))),
        ] {
            assert!(account_in(&p, &accounts).is_err(), "{p} was let through");
        }
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn arguments_are_checked_as_they_are_read() {
        assert!(serde_json::from_value::<AbsPath>(serde_json::json!(abs("p"))).is_ok());
        let err = serde_json::from_value::<AbsPath>(serde_json::json!(abs("p/../q"))).unwrap_err().to_string();
        assert!(err.contains("not a usable path"), "{err}");
        let err = serde_json::from_value::<AccountPath>(serde_json::json!(abs("not-an-account"))).unwrap_err().to_string();
        assert!(err.contains("account folder"), "{err}");
        // Empty is none; anything else is held to the type.
        let none = serde_json::from_value::<OrEmpty<AccountPath>>(serde_json::json!("")).unwrap();
        assert_eq!(none.into_string(), "");
        assert!(serde_json::from_value::<OrEmpty<AccountPath>>(serde_json::json!(abs("not-an-account"))).is_err());
        assert!(serde_json::from_value::<OrEmpty<AbsPath>>(serde_json::json!("relative")).is_err());
        assert_eq!(serde_json::from_value::<OrEmpty<AbsPath>>(serde_json::json!(abs("p"))).unwrap().into_string(), abs("p"));
    }
}
