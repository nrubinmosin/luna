//! `config.toml` under a Codex account (`CODEX_HOME`), read and written with
//! the user's own formatting and comments intact.

use std::path::{Path, PathBuf};
use toml_edit::DocumentMut;

pub fn account_file(account_path: &str) -> Result<PathBuf, String> {
    if account_path.is_empty() {
        return Err("No account selected".into());
    }
    Ok(Path::new(account_path).join("config.toml"))
}

/// The document to change and write back, or an empty one when the file is
/// missing. A file that will not read or parse is an error rather than an
/// empty document: writing back over it would throw the user's config away.
/// Codex or an editor rewriting the file in place can be caught midway —
/// blank, cut short, or on Windows held open — so such a read looks again a
/// moment later. Blank on every look, the file is blank, and an empty
/// document loses nothing of it.
pub fn read(path: &Path) -> Result<DocumentMut, String> {
    let mut looks = 0;
    loop {
        let got = match std::fs::read_to_string(path) {
            Ok(text) => match text.parse::<DocumentMut>() {
                Ok(doc) if !text.trim().is_empty() => return Ok(doc),
                Ok(doc) => Ok(doc),
                Err(e) => Err(format!("{}: {e}", path.display())),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(DocumentMut::new()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        };
        looks += 1;
        if looks == 3 {
            return got;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// The MCP servers an account's `config.toml` defines, each by its name as
/// written with the keys its table sets; none without the file. One that will not read or parse, after
/// `read`'s looks again, is an error and not an empty list: a helper's start
/// turns those servers off by name, and a list read short would leave them on.
pub fn mcp_servers(account_path: &str) -> Result<Vec<(String, Vec<String>)>, String> {
    let doc = read(&account_file(account_path)?)?;
    let servers = doc.get("mcp_servers").and_then(|t| t.as_table_like());
    let keys = |v: &toml_edit::Item| v.as_table_like().map(|t| t.iter().map(|(k, _)| k.to_string()).collect()).unwrap_or_default();
    Ok(servers.map(|t| t.iter().map(|(name, v)| (name.to_string(), keys(v))).collect()).unwrap_or_default())
}

/// `mcp_servers` for a helper's start, which goes no further without it.
pub fn helper_mcp_servers(account_path: &str) -> Result<Vec<(String, Vec<String>)>, String> {
    mcp_servers(account_path).map_err(|e| {
        format!(
            "the helper is not started: Luna could not read which MCP servers its account's config.toml defines, to turn them off ({e})"
        )
    })
}

/// For lookups only: a config that will not parse simply has nothing to say.
/// One look, as nothing is written back.
pub fn read_or_empty(path: &Path) -> DocumentMut {
    std::fs::read_to_string(path).ok().and_then(|t| t.parse().ok()).unwrap_or_default()
}

/// Writes via a sibling temp file so a crash mid-write cannot truncate the
/// account's whole config. The temp name is this write's own (process and
/// count), so two writes at once never share one.
pub fn write(path: &Path, doc: &DocumentMut) -> Result<(), String> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = path.with_extension(format!("toml.{}-{n}.tmp", std::process::id()));
    std::fs::write(&tmp, doc.to_string()).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })?;
    // A rename refused (Codex holding the file) leaves no temp file behind.
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })
}

/// Whether the account's config.toml names a `windows.sandbox`: the user's
/// choice, which Luna does not override with its own (codex/launch.rs
/// `codex_windows_sandbox`). Not a project's `.codex/config.toml`: that is
/// the repository's say, and Luna's sandbox stands over it as ever.
pub fn windows_sandbox_set(account_path: &str) -> bool {
    account_file(account_path).is_ok_and(|f| read_or_empty(&f).get("windows").and_then(|w| w.get("sandbox")).is_some())
}

/// A string-valued top-level key, trimmed; None for anything else.
pub fn string_at(doc: &DocumentMut, key: &str) -> Option<String> {
    let s = doc.get(key)?.as_str()?.trim();
    (!s.is_empty()).then(|| s.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_accounts_servers_are_read_or_the_reading_fails() {
        let root = std::env::temp_dir().join(format!("luna-codex-servers-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let account = root.to_string_lossy().into_owned();
        assert_eq!(mcp_servers(&account), Ok(Vec::new()), "no file: no servers");
        std::fs::write(root.join("config.toml"), "model = \"m\"\n[mcp_servers.gmail]\ncommand = \"node\"\n").unwrap();
        assert_eq!(mcp_servers(&account), Ok(vec![("gmail".to_string(), vec!["command".to_string()])]));
        // Caught mid-write, or broken: an error, never a short list.
        std::fs::write(root.join("config.toml"), "[mcp_servers.gmail\ncommand = ").unwrap();
        assert!(mcp_servers(&account).is_err());
        assert!(helper_mcp_servers(&account).unwrap_err().contains("not started"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_windows_sandbox_of_the_accounts_is_the_users_choice_and_a_projects_is_not() {
        let root = std::env::temp_dir().join(format!("luna-codex-winsandbox-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (account, project) = (root.join("acc"), root.join("proj"));
        std::fs::create_dir_all(&account).unwrap();
        std::fs::create_dir_all(project.join(".codex")).unwrap();
        let a = account.to_string_lossy().into_owned();
        assert!(!windows_sandbox_set(&a), "no config at all");
        std::fs::write(account.join("config.toml"), "model = \"x\"\n[windows]\nother = 1\n").unwrap();
        assert!(!windows_sandbox_set(&a), "a [windows] table without a sandbox");
        std::fs::write(account.join("config.toml"), "[windows]\nsandbox = \"elevated\"\n").unwrap();
        assert!(windows_sandbox_set(&a), "the account's");
        std::fs::write(account.join("config.toml"), "model = [broken\n").unwrap();
        assert!(!windows_sandbox_set(&a), "a config that will not parse says nothing");
        std::fs::write(project.join(".codex").join("config.toml"), "windows.sandbox = \"elevated\"\n").unwrap();
        assert!(!windows_sandbox_set(&a), "the project's is the repository's say, not the user's");
        std::fs::write(account.join("config.toml"), "windows.sandbox = \"elevated\"\n").unwrap();
        assert!(windows_sandbox_set(&a), "the account's, dotted");
        let _ = std::fs::remove_dir_all(&root);
    }
}
