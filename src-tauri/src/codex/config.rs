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

/// Whether the account's config.toml or the project's `.codex/config.toml`
/// in `folder` names a `windows.sandbox`: a choice of the user's, which Luna
/// does not override with its own (codex/launch.rs `codex_windows_sandbox`).
pub fn windows_sandbox_set(account_path: &str, folder: &str) -> bool {
    let project = Path::new(folder).join(".codex").join("config.toml");
    let files = account_file(account_path).into_iter().chain([project]);
    files.map(|f| read_or_empty(&f)).any(|doc| doc.get("windows").and_then(|w| w.get("sandbox")).is_some())
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
    fn a_windows_sandbox_of_the_accounts_or_the_projects_is_the_users_choice() {
        let root = std::env::temp_dir().join(format!("luna-codex-winsandbox-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (account, project) = (root.join("acc"), root.join("proj"));
        std::fs::create_dir_all(&account).unwrap();
        std::fs::create_dir_all(project.join(".codex")).unwrap();
        let (a, p) = (account.to_string_lossy().into_owned(), project.to_string_lossy().into_owned());
        assert!(!windows_sandbox_set(&a, &p), "no config at all");
        std::fs::write(account.join("config.toml"), "model = \"x\"\n[windows]\nother = 1\n").unwrap();
        assert!(!windows_sandbox_set(&a, &p), "a [windows] table without a sandbox");
        std::fs::write(account.join("config.toml"), "[windows]\nsandbox = \"elevated\"\n").unwrap();
        assert!(windows_sandbox_set(&a, &p), "the account's");
        std::fs::write(account.join("config.toml"), "model = [broken\n").unwrap();
        assert!(!windows_sandbox_set(&a, &p), "a config that will not parse says nothing");
        std::fs::write(project.join(".codex").join("config.toml"), "windows.sandbox = \"unelevated\"\n").unwrap();
        assert!(windows_sandbox_set(&a, &p), "the project's, dotted");
        let _ = std::fs::remove_dir_all(&root);
    }
}
