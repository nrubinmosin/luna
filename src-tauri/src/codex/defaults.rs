//! Where a new Codex chat's model, effort, approval and sandbox come from when
//! nobody has said otherwise: Codex's own `config.toml`, account first, then
//! the project's `.codex/config.toml` — which Codex only reads for a trusted
//! folder, so neither does this.
//!
//! Luna always spawns Codex with the flags, so whatever the files say would
//! otherwise be overridden on every session. Reading them here is what makes
//! the flags agree with the files instead of fighting them.
//!
//! As for Claude (claude/defaults.rs), the values come out settled: one Luna
//! cannot pass on reads as nothing said, and nothing said is Luna's stock
//! choice (`source: stock`) — except the model, where nothing said leaves it
//! to Codex.

use super::config;
use serde::Serialize;
use std::path::Path;

/// One resolved value and the file it came from, so the dialog can say why it
/// is showing what it is showing.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Setting {
    pub value: String,
    /// `account` | `project` | `stock`.
    pub source: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CodexDefaults {
    /// None: whatever Codex itself defaults to.
    pub model: Option<Setting>,
    pub effort: Setting,
    pub approval: Setting,
    pub sandbox: Setting,
}

/// What a chat starts with when the files say nothing Luna can use.
const STOCK_EFFORT: &str = "high";
const STOCK_APPROVAL: &str = "never";
const STOCK_SANDBOX: &str = "danger-full-access";

const APPROVALS: [&str; 2] = ["on-request", "never"];
const SANDBOXES: [&str; 3] = ["read-only", "workspace-write", "danger-full-access"];

/// The levels are per model and come off the CLI's model list, so any plain
/// word passes.
fn effort_level(raw: &str) -> Option<String> {
    let v = raw.trim().to_lowercase();
    (!v.is_empty() && v.chars().all(|c| c.is_ascii_lowercase())).then_some(v)
}

fn one_of(list: &[&str], raw: &str) -> Option<String> {
    let v = raw.to_lowercase();
    list.iter().find(|m| **m == v).map(|m| m.to_string())
}

/// What the files settled on, or the stock value when they said nothing usable.
fn settle(said: Option<Setting>, usable: impl Fn(&str) -> Option<String>, stock: &str) -> Setting {
    said.and_then(|s| usable(&s.value).map(|value| Setting { value, source: s.source }))
        .unwrap_or_else(|| Setting { value: stock.to_owned(), source: "stock".to_owned() })
}

/// What the files say, as they say it.
#[derive(Default)]
struct Said {
    model: Option<Setting>,
    effort: Option<Setting>,
    approval: Option<Setting>,
    sandbox: Option<Setting>,
}

fn take(source: &str, doc: &toml_edit::DocumentMut, key: &str) -> Option<Setting> {
    config::string_at(doc, key).map(|value| Setting { value, source: source.to_owned() })
}

fn apply(out: &mut Said, source: &str, doc: &toml_edit::DocumentMut) {
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
    let said = read(&account_path, &folder);
    CodexDefaults {
        model: said.model.filter(|s| !s.value.trim().is_empty()),
        effort: settle(said.effort, effort_level, STOCK_EFFORT),
        approval: settle(said.approval, |v| one_of(&APPROVALS, v), STOCK_APPROVAL),
        sandbox: settle(said.sandbox, |v| one_of(&SANDBOXES, v), STOCK_SANDBOX),
    }
}

fn read(account_path: &str, folder: &str) -> Said {
    let mut out = Said::default();
    if account_path.is_empty() {
        return out;
    }
    let Ok(account_file) = config::account_file(account_path) else { return out };
    apply(&mut out, "account", &config::read_or_empty(&account_file));

    if !folder.is_empty() {
        let trusted = super::trust::codex_folder_trusted(account_path.to_owned(), folder.to_owned())
            .unwrap_or(false);
        if trusted {
            let project = Path::new(folder).join(".codex").join("config.toml");
            apply(&mut out, "project", &config::read_or_empty(&project));
        }
    }
    out
}

/// The same as the flags a helper would be spawned with, for `luna_list`.
pub fn flags(account_path: &str, folder: &str) -> serde_json::Value {
    let d = codex_defaults(account_path.to_owned(), folder.to_owned());
    serde_json::json!({
        "model": d.model.map(|m| m.value),
        "effort": d.effort.value,
        "approval": d.approval.value,
        "sandbox": d.sandbox.value,
    })
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
        assert_eq!(d.effort.value, "xhigh");
        assert_eq!(d.approval.value, "never");
        assert_eq!(d.sandbox.source, "account");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn nothing_usable_is_stock() {
        let root = std::env::temp_dir().join(format!("luna-codex-stock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let account = root.join("account");
        write(&account.join("config.toml"), "model_reasoning_effort = \"High\"\napproval_policy = \"untrusted\"\n");
        let d = codex_defaults(account.to_string_lossy().into_owned(), String::new());
        assert!(d.model.is_none(), "no model is Codex's own");
        assert_eq!((d.effort.value.as_str(), d.effort.source.as_str()), ("high", "account"));
        assert_eq!((d.approval.value.as_str(), d.approval.source.as_str()), (STOCK_APPROVAL, "stock"));
        assert_eq!(d.sandbox.value, STOCK_SANDBOX);
        let none = codex_defaults(String::new(), String::new());
        assert_eq!(none.effort.source, "stock");
        let _ = std::fs::remove_dir_all(&root);
    }
}
