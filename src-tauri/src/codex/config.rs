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

/// The document, or an empty one when the file is missing. A file that does
/// not parse is an error rather than an empty document: writing back over it
/// would throw the user's config away.
pub fn read(path: &Path) -> Result<DocumentMut, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => text
            .parse::<DocumentMut>()
            .map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(DocumentMut::new()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// For lookups only: a config that will not parse simply has nothing to say.
pub fn read_or_empty(path: &Path) -> DocumentMut {
    read(path).unwrap_or_default()
}

/// Writes via a sibling temp file so a crash mid-write cannot truncate the
/// account's whole config.
pub fn write(path: &Path, doc: &DocumentMut) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, doc.to_string()).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

/// What a fresh account starts with. Luna updates the CLI itself, so the CLI's
/// own startup check would only nag about a version Luna is about to install.
pub fn seed(account_path: &str) -> Result<(), String> {
    let path = account_file(account_path)?;
    let mut doc = read(&path)?;
    if doc.get("check_for_update_on_startup").is_none() {
        doc["check_for_update_on_startup"] = toml_edit::value(false);
    }
    write(&path, &doc)
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
    fn seed_keeps_what_is_there() {
        let root = std::env::temp_dir().join(format!("luna-codex-config-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("config.toml");
        std::fs::write(&path, "# mine\nmodel = \"gpt-5-codex\" # keep\n").unwrap();

        seed(&root.to_string_lossy()).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# mine"));
        assert!(text.contains("model = \"gpt-5-codex\" # keep"));
        assert!(text.contains("check_for_update_on_startup = false"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_broken_file_is_not_overwritten() {
        let root = std::env::temp_dir().join(format!("luna-codex-broken-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("config.toml");
        std::fs::write(&path, "model = [unterminated\n").unwrap();
        assert!(seed(&root.to_string_lossy()).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "model = [unterminated\n");
        let _ = std::fs::remove_dir_all(&root);
    }
}
