//! Where a new Codex chat's model, effort, approval and sandbox come from when
//! nobody has said otherwise: Codex's own `config.toml`, account first, then
//! the project's `.codex/config.toml` — which Codex only reads for a trusted
//! folder, so neither does this.
//!
//! Luna always spawns Codex with the flags, so whatever the files say would
//! otherwise be overridden on every session. Reading them here is what makes
//! the flags agree with the files instead of fighting them.

use super::config;
use serde::Serialize;
use std::path::Path;

/// One resolved value and the file it came from, so the dialog can say why it
/// is showing what it is showing.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Setting {
    pub value: String,
    /// `account` | `project`.
    pub source: String,
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct CodexDefaults {
    pub model: Option<Setting>,
    pub effort: Option<Setting>,
    pub approval: Option<Setting>,
    pub sandbox: Option<Setting>,
}

fn take(source: &str, doc: &toml_edit::DocumentMut, key: &str) -> Option<Setting> {
    config::string_at(doc, key).map(|value| Setting { value, source: source.to_owned() })
}

fn apply(out: &mut CodexDefaults, source: &str, doc: &toml_edit::DocumentMut) {
    if let Some(s) = take(source, doc, "model") {
        out.model = Some(s);
    }
    if let Some(s) = take(source, doc, "model_reasoning_effort") {
        out.effort = Some(s);
    }
    // `approval_policy` can also be a `{ granular = … }` table; that is not a
    // mode Luna can put in a title bar, so it reads as nothing said.
    if let Some(s) = take(source, doc, "approval_policy") {
        out.approval = Some(s);
    }
    if let Some(s) = take(source, doc, "sandbox_mode") {
        out.sandbox = Some(s);
    }
}

#[tauri::command]
pub fn codex_defaults(account_path: String, folder: String) -> CodexDefaults {
    let mut out = CodexDefaults::default();
    if account_path.is_empty() {
        return out;
    }
    let Ok(account_file) = config::account_file(&account_path) else { return out };
    apply(&mut out, "account", &config::read_or_empty(&account_file));

    if !folder.is_empty() {
        let trusted = super::trust::codex_folder_trusted(account_path.clone(), folder.clone())
            .unwrap_or(false);
        if trusted {
            let project = Path::new(&folder).join(".codex").join("config.toml");
            apply(&mut out, "project", &config::read_or_empty(&project));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn project_wins_only_when_trusted() {
        let root = std::env::temp_dir().join(format!("luna-codex-defaults-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let account = root.join("account");
        let folder = root.join("project");
        write(
            &account.join("config.toml"),
            "model = \"gpt-5-codex\"\nmodel_reasoning_effort = \"xhigh\"\napproval_policy = \"never\"\nsandbox_mode = \"danger-full-access\"\n",
        );
        write(&folder.join(".codex").join("config.toml"), "model = \"gpt-5-mini\"\n");
        let acc = account.to_string_lossy().into_owned();
        let dir = folder.to_string_lossy().into_owned();

        let d = codex_defaults(acc.clone(), dir.clone());
        assert_eq!(d.model.as_ref().unwrap().value, "gpt-5-codex");
        assert_eq!(d.model.as_ref().unwrap().source, "account");

        super::super::trust::codex_trust_folder(acc.clone(), dir.clone()).unwrap();
        let d = codex_defaults(acc, dir);
        let model = d.model.unwrap();
        assert_eq!(model.value, "gpt-5-mini");
        assert_eq!(model.source, "project");
        assert_eq!(d.effort.unwrap().value, "xhigh");
        assert_eq!(d.approval.unwrap().value, "never");
        assert_eq!(d.sandbox.unwrap().source, "account");

        let _ = std::fs::remove_dir_all(&root);
    }
}
