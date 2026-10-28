//! Codex keeps per-project trust in the account's `config.toml`:
//!
//! ```toml
//! [projects.'E:\Projects\app']
//! trust_level = "trusted"
//! ```
//!
//! Without it the TUI opens its trust screen on first use of a folder — and
//! right after that decision, on Windows, its sandbox setup prompt. Writing
//! the bit ahead of time is what lets a session start straight into the
//! prompt. Trust on the repository root also covers its worktrees.
//!
//! Without trust Codex also leaves the project's own `.codex/` out — its
//! config, hooks and exec policies — and `codex exec`, which background
//! helpers are, does so without asking. Trust can instead come for one run
//! from a `-c` override (`trust_override`), which writes nothing: background
//! helpers always get one, and so does any session in a project under one of
//! the folders Settings lists as trusted on this machine. A trusted parent
//! folder in `config.toml` would not do: Codex looks trust up by the
//! checkout's exact path.

use super::config;
use toml_edit::{Item, Table};

/// Codex compares project keys case-insensitively on Windows and tries both
/// spellings of the separator; the same leniency here.
fn same_project(a: &str, b: &str) -> bool {
    let n = |s: &str| s.replace('/', "\\").trim_end_matches('\\').to_lowercase();
    n(a) == n(b)
}

fn native(folder: &str) -> String {
    #[cfg(windows)]
    {
        folder.replace('/', "\\").trim_end_matches('\\').to_string()
    }
    #[cfg(not(windows))]
    {
        folder.trim_end_matches('/').to_string()
    }
}

fn trusted_in(doc: &toml_edit::DocumentMut, folder: &str) -> bool {
    let Some(projects) = doc.get("projects").and_then(Item::as_table_like) else {
        return false;
    };
    projects.iter().any(|(key, entry)| {
        same_project(key, folder)
            && entry
                .as_table_like()
                .and_then(|t| t.get("trust_level"))
                .and_then(Item::as_str)
                == Some("trusted")
    })
}

#[tauri::command]
pub fn codex_folder_trusted(account_path: String, folder: String) -> Result<bool, String> {
    if under_root(&folder, &crate::settings::get().codex_trusted_roots) {
        return Ok(true);
    }
    let doc = config::read_or_empty(&config::account_file(&account_path)?);
    Ok(trusted_in(&doc, &folder))
}

/// The checkout `folder` is in, where Codex looks its trust up — a CLI-made
/// worktree's project first — or the folder itself outside git.
fn checkout_of(folder: &str) -> String {
    let project = crate::worktree::project_of(folder).unwrap_or_else(|| folder.to_string());
    let mut dir = std::path::Path::new(&project);
    loop {
        if dir.join(".git").exists() {
            return dir.to_string_lossy().into_owned();
        }
        match dir.parent() {
            Some(up) => dir = up,
            None => return project,
        }
    }
}

/// `-c` trusting the project `folder` is in for this one run.
pub fn trust_override(folder: &str) -> [String; 2] {
    let key = native(&checkout_of(folder)).replace('\\', "\\\\").replace('"', "\\\"");
    ["-c".into(), format!("projects={{\"{key}\"={{trust_level=\"trusted\"}}}}")]
}

/// `trust_override` when `folder` is under a folder trusted on this machine.
pub fn root_override(folder: &str) -> Option<[String; 2]> {
    let project = crate::worktree::project_of(folder).unwrap_or_else(|| folder.to_string());
    under_root(&project, &crate::settings::get().codex_trusted_roots).then(|| trust_override(folder))
}

fn under_root(folder: &str, roots: &[String]) -> bool {
    let n = |s: &str| {
        let s = s.replace('\\', "/").trim_end_matches('/').to_string();
        if cfg!(windows) { s.to_lowercase() } else { s }
    };
    let folder = n(folder);
    roots.iter().map(|r| n(r)).filter(|r| !r.is_empty()).any(|r| folder == r || folder.starts_with(&format!("{r}/")))
}

#[tauri::command]
pub fn codex_trusted_roots() -> Vec<String> {
    crate::settings::get().codex_trusted_roots
}

#[tauri::command]
pub fn set_codex_trusted_roots(roots: Vec<String>) -> Result<Vec<String>, String> {
    let roots: Vec<String> = roots.into_iter().map(|r| r.trim().to_string()).filter(|r| !r.is_empty()).collect();
    crate::settings::update(|s| s.codex_trusted_roots = roots.clone())?;
    Ok(roots)
}

/// Marks the folder trusted for this account — the same entry Codex's own
/// trust screen writes. Leaves every other key in the config untouched.
#[tauri::command]
pub fn codex_trust_folder(account_path: String, folder: String) -> Result<(), String> {
    let path = config::account_file(&account_path)?;
    let mut doc = config::read(&path)?;
    if trusted_in(&doc, &folder) {
        return Ok(());
    }

    let projects = doc
        .entry("projects")
        .or_insert_with(|| {
            let mut t = Table::new();
            t.set_implicit(true);
            Item::Table(t)
        });
    if !projects.is_table_like() {
        *projects = Item::Table(Table::new());
    }
    let projects = projects.as_table_mut().ok_or("projects is not a table")?;
    projects.set_implicit(true);

    // Reuse an existing entry for this folder under any spelling, so a
    // `trust_level = "untrusted"` becomes trusted rather than getting a twin.
    let existing = projects
        .iter()
        .find(|(k, _)| same_project(k, &folder))
        .map(|(k, _)| k.to_string());
    let key = existing.unwrap_or_else(|| native(&folder));
    let entry = projects.entry(&key).or_insert_with(|| Item::Table(Table::new()));
    if !entry.is_table_like() {
        *entry = Item::Table(Table::new());
    }
    entry
        .as_table_like_mut()
        .ok_or("project entry is not a table")?
        .insert("trust_level", toml_edit::value("trusted"));

    config::write(&path, &doc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_root_covers_the_folders_under_it_and_nothing_beside_it() {
        let roots = vec!["E:\\Projects\\".to_string()];
        assert!(under_root("E:\\Projects", &roots));
        assert!(under_root("E:/Projects/Sprouter/app", &roots), "either separator");
        assert!(!under_root("E:\\Projects2\\app", &roots), "a sibling that shares the prefix");
        assert!(!under_root("E:\\Projects\\app", &[String::new()]));
    }

    #[test]
    fn the_override_names_the_checkout_as_toml() {
        let root = temp("ov");
        std::fs::create_dir_all(root.join("repo").join(".git")).unwrap();
        std::fs::create_dir_all(root.join("repo").join("sub")).unwrap();
        let sub = root.join("repo").join("sub").to_string_lossy().into_owned();
        let [flag, value] = trust_override(&sub);
        assert_eq!(flag, "-c");
        let parsed: toml_edit::DocumentMut = value.parse().unwrap();
        let (key, entry) = parsed["projects"].as_inline_table().unwrap().iter().next().unwrap();
        assert!(same_project(key, &root.join("repo").to_string_lossy()), "the checkout, not the subfolder: {key}");
        assert_eq!(entry.as_inline_table().unwrap().get("trust_level").and_then(|v| v.as_str()), Some("trusted"));
        let _ = std::fs::remove_dir_all(&root);
    }

    fn temp(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("luna-codex-trust-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn writes_and_reads_back_with_backslashes() {
        let root = temp("rt");
        let acc = root.to_string_lossy().into_owned();
        std::fs::write(root.join("config.toml"), "model = \"x\"\n").unwrap();

        assert!(!codex_folder_trusted(acc.clone(), r"E:\Projects\App".into()).unwrap());
        codex_trust_folder(acc.clone(), r"E:\Projects\App".into()).unwrap();
        assert!(codex_folder_trusted(acc.clone(), r"E:\Projects\App".into()).unwrap());
        // Other spellings of the same folder count too.
        assert!(codex_folder_trusted(acc.clone(), "e:/projects/app/".into()).unwrap());

        let text = std::fs::read_to_string(root.join("config.toml")).unwrap();
        assert!(text.starts_with("model = \"x\"\n"), "{text}");
        assert!(text.contains("trust_level = \"trusted\""), "{text}");
        // Parses back as the key Codex will look up.
        let doc: toml_edit::DocumentMut = text.parse().unwrap();
        let (key, _) = doc["projects"].as_table().unwrap().iter().next().unwrap();
        assert!(same_project(key, r"E:\Projects\App"), "{key}");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn upgrades_an_untrusted_entry_in_place() {
        let root = temp("up");
        let acc = root.to_string_lossy().into_owned();
        std::fs::write(
            root.join("config.toml"),
            "[projects.'E:\\Projects\\App']\ntrust_level = \"untrusted\"\n",
        )
        .unwrap();
        codex_trust_folder(acc.clone(), "E:/Projects/App".into()).unwrap();
        let text = std::fs::read_to_string(root.join("config.toml")).unwrap();
        assert_eq!(text.matches("trust_level").count(), 1, "{text}");
        assert!(text.contains("\"trusted\""));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn dotted_form_is_recognised() {
        let doc: toml_edit::DocumentMut =
            "[projects]\n\"E:\\\\Projects\\\\App\" = { trust_level = \"trusted\" }\n".parse().unwrap();
        assert!(trusted_in(&doc, r"e:\projects\app"));
    }
}
