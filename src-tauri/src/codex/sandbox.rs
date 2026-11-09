//! What Codex's Windows sandbox keeps of its own in an account (0.167.0):
//! `<CODEX_HOME>/cap_sid`, JSON naming the capability SIDs its unelevated
//! sandbox restricts a command's token to and grants itself on the folders
//! that command may write: `workspace`, `readonly`, and by folder,
//! `workspace_by_cwd` and `writable_root_by_path`, each keyed by the path as
//! Codex writes it (lowercase, `/`-separated). A folder given as `--add-dir`
//! (a writable root) gets an inheritable Modify ACE for the SID
//! `writable_root_by_path` names for it (docs/cli-compatibility.md).
//!
//! - `sids_for_root`: that SID for one account's root, what a chat's handoff
//!   drafts folder may keep of its sandbox's grant (agents/handoff `drafts_of`).
//! - `sids_for`: the same over every Codex account, for the data folder's
//!   privacy check, which knows no chat.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

const CAP_SID: &str = "cap_sid";

/// Far past what Codex writes there (a few hundred bytes per root).
const MAX_BYTES: u64 = 256 * 1024;

#[derive(Deserialize)]
struct CapSids {
    #[serde(default)]
    writable_root_by_path: BTreeMap<String, String>,
}

/// A folder's path as Codex keys it, and ours as compared with it: `/` for
/// `\`, no trailing `/` nor Windows' `\\?\`, lowercase.
fn key(path: &str) -> String {
    path.trim_start_matches(r"\\?\").replace('\\', "/").trim_end_matches('/').to_lowercase()
}

/// A SID as Codex mints one for its sandbox, `S-1-5-21-` and four numbers:
/// nothing else in the file is taken for one.
fn sid_shaped(sid: &str) -> bool {
    sid.strip_prefix("S-1-5-21-").is_some_and(|rest| {
        let parts: Vec<&str> = rest.split('-').collect();
        parts.len() == 4 && parts.iter().all(|p| !p.is_empty() && p.len() <= 10 && p.bytes().all(|b| b.is_ascii_digit()))
    })
}

/// The SID Codex's sandbox on the account at `account` grants itself on the
/// writable root `root`, as the account's `cap_sid` names it: none where the
/// file is not there (no sandboxed command has run yet) or names nothing for
/// the root. The file is read as the account's own: opened as itself, never
/// through a link (`boundary::open_no_follow`), a file of this user's making
/// (`secret::own_file`), at most MAX_BYTES. Why not, where it cannot be.
pub fn sids_for_root(account: &Path, root: &Path) -> Result<Vec<String>, String> {
    use std::io::Read;
    let path = account.join(CAP_SID);
    let file = match crate::boundary::open_no_follow(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let meta = file.metadata().map_err(|e| format!("{}: {e}", path.display()))?;
    if crate::boundary::is_link(&meta) || !meta.is_file() {
        return Err(format!("{} is a link or not a file", path.display()));
    }
    if let Err(why) = crate::secret::own_file(&file).map_err(|e| format!("{}: {e}", path.display()))? {
        return Err(format!("{} is not this user's own ({why})", path.display()));
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(format!("{} is over {} KB", path.display(), MAX_BYTES / 1024));
    }
    let caps: CapSids = serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    let want = key(&root.to_string_lossy());
    Ok(caps.writable_root_by_path.into_iter().filter(|(k, sid)| key(k) == want && sid_shaped(sid)).map(|(_, sid)| sid).collect())
}

/// The SIDs any Codex account's sandbox grants itself on the writable root
/// `root`: what the data folder's privacy check takes as no one else's there.
/// An account whose file will not read gives none.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn sids_for(root: &Path) -> Vec<String> {
    let accounts = crate::accounts::list_accounts().unwrap_or_default();
    accounts
        .iter()
        .filter(|a| a.provider == crate::provider::Provider::Codex)
        .flat_map(|a| sids_for_root(Path::new(&a.path), root).unwrap_or_default())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home(tag: &str, text: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("luna-cap-sid-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(CAP_SID), text).unwrap();
        dir
    }

    /// The file as Codex 0.167.0 wrote it for a `codex sandbox` run with one
    /// writable root beside its cwd: the root's own SID, and no other's.
    #[test]
    fn the_sid_of_a_writable_root_is_read_from_the_accounts_cap_sid() {
        let drafts = r"E:\Data\Luna\handoffs\c1\drafts";
        let text = r#"{"workspace":"S-1-5-21-2168808566-384334433-1764654966-1022559879","readonly":"S-1-5-21-2068910092-2309591338-1925617247-3574389772","workspace_by_cwd":{"e:/data/luna/handoffs/c1/drafts":"S-1-5-21-1-2-3-4"},"writable_root_by_path":{"c:/users/me/appdata/local/temp":"S-1-5-21-1609353252-1116857426-1149687468-4213023062","e:/data/luna/handoffs/c1/drafts":"S-1-5-21-2439410019-928164549-3803989657-671039924"}}"#;
        let dir = home("read", text);
        assert_eq!(sids_for_root(&dir, Path::new(drafts)).unwrap(), ["S-1-5-21-2439410019-928164549-3803989657-671039924"]);
        assert_eq!(sids_for_root(&dir, Path::new(&format!(r"\\?\{drafts}\"))).unwrap().len(), 1, "as Windows may spell it");
        assert!(sids_for_root(&dir, Path::new(r"E:\Data\Luna\handoffs\c2\drafts")).unwrap().is_empty(), "another chat's");
        // Not there yet: none, and no error.
        std::fs::remove_file(dir.join(CAP_SID)).unwrap();
        assert!(sids_for_root(&dir, Path::new(drafts)).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cap_sid_that_will_not_do_names_nothing_and_says_why() {
        let drafts = Path::new("/data/handoffs/c1/drafts");
        for (text, why) in [
            ("not json", "expected"),
            (r#"{"writable_root_by_path":{"/data/handoffs/c1/drafts":"WD"}}"#, ""),
            (r#"{"writable_root_by_path":{"/data/handoffs/c1/drafts":"S-1-5-21-1-2-3"}}"#, ""),
        ] {
            let dir = home("bad", text);
            match sids_for_root(&dir, drafts) {
                Ok(sids) => assert!(why.is_empty() && sids.is_empty(), "{text}: {sids:?}"),
                Err(e) => assert!(!why.is_empty() && e.contains(why), "{text}: {e}"),
            }
            let _ = std::fs::remove_dir_all(&dir);
        }
        let dir = home("big", &" ".repeat(MAX_BYTES as usize + 1));
        assert!(sids_for_root(&dir, drafts).unwrap_err().contains("KB"));
        let _ = std::fs::remove_dir_all(&dir);
        // A link at its name is not followed.
        #[cfg(unix)]
        {
            let dir = home("link", "{}");
            let elsewhere = dir.join("elsewhere");
            std::fs::write(&elsewhere, r#"{"writable_root_by_path":{"/data/handoffs/c1/drafts":"S-1-5-21-1-2-3-4"}}"#).unwrap();
            std::fs::remove_file(dir.join(CAP_SID)).unwrap();
            std::os::unix::fs::symlink(&elsewhere, dir.join(CAP_SID)).unwrap();
            assert!(sids_for_root(&dir, drafts).is_err());
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
