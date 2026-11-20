//! Where a new Codex chat's model, effort, approval and sandbox come from when
//! nobody has said otherwise: Codex's own `config.toml` layers, its system one
//! first (`config::system_layer`), then the account's, then the project's
//! `.codex/config.toml` layers.
//!
//! The model and its effort, which Luna always passes, are the last layer's
//! that Codex trusts (codex/trust.rs `Decisions`), or where nothing is said
//! what Codex itself would pick (`source: codex`): the first model of its
//! catalog (codex/models.rs) at that model's default level.
//!
//! The approval and the sandbox Luna does not resolve: what Codex makes of its
//! layers (which it trusts, which key wins, its own defaults) is Codex's. A
//! chat or a helper that names neither runs in "Codex's own, from its config"
//! (`OWN`, `source: codex`): Luna passes no flag for it and claims no value.
//! The one thing Luna decides from the files is whether it can read them: a
//! layer that will not read or parse, or one naming an approval, a sandbox or
//! a permission profile Luna does not know (Codex's retired `untrusted`
//! among them), in any layer Codex might read, trusted or not, makes them
//! `on-request` and `read-only` (`source: closed`), passed explicitly. A
//! session is also not started where the account's own `approval_policy` is
//! `untrusted`, which Codex refuses whatever flag Luna passes
//! (`refuses_untrusted`).

use super::config;
use super::models::CodexModel;
use serde::Serialize;

use crate::boundary::{AbsPath, AccountPath, OrEmpty};

/// One resolved value and the file it came from, so the dialog can say why it
/// is showing what it is showing.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Setting {
    pub value: String,
    /// `system` | `account` | `project` | `codex` | `stock` | `closed`.
    pub source: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CodexDefaults {
    /// None only when no model list could be had: Codex picks.
    pub model: Option<Setting>,
    pub effort: Setting,
    pub approval: Setting,
    pub sandbox: Setting,
}

/// The effort a chat starts with when neither the files nor a model list say one.
const STOCK_EFFORT: &str = "high";

/// Luna's word for an approval or a sandbox it leaves to Codex: "Codex's own,
/// from its config". No flag goes for it, and Luna does not say what it is.
pub(crate) const OWN: &str = "codex";

/// What the approval and the sandbox are where Luna cannot read the files:
/// Codex asks before anything the sandbox would block, and the sandbox writes
/// nothing. Passed explicitly.
pub(crate) const CLOSED_APPROVAL: &str = "on-request";
pub(crate) const CLOSED_SANDBOX: &str = "read-only";
const CLOSED: &str = "closed";

const SANDBOXES: [&str; 3] = ["read-only", "workspace-write", "danger-full-access"];

/// Codex's built-in permission profiles (0.169.0, protocol/src/models.rs
/// `BUILT_IN_PERMISSION_PROFILE_*`); any other name is a profile of the
/// config's own.
const BUILT_IN_PROFILES: [&str; 3] = [":read-only", ":workspace", ":danger-full-access"];

/// The levels are per model and come off the CLI's model list, so any plain
/// word passes.
fn effort_level(raw: &str) -> Option<String> {
    let v = raw.trim().to_lowercase();
    (!v.is_empty() && v.chars().all(|c| c.is_ascii_lowercase())).then_some(v)
}

/// An `approval_policy` Codex 0.169.0 runs: `on-request` (and its alias
/// `on-failure`), `never`, or a `{ granular = … }` table with the fields
/// `GranularApprovalConfig` takes — `sandbox_approval`, `rules` and
/// `mcp_elicitations` required, `skill_approval` and `request_permissions`
/// optional, all booleans. Not `untrusted`, which it refuses.
fn approval_known(item: &toml_edit::Item) -> bool {
    if let Some(s) = item.as_str() {
        return matches!(s.trim().to_lowercase().as_str(), "on-request" | "on-failure" | "never");
    }
    let Some(table) = item.as_table_like() else { return false };
    let Some(granular) = table.get("granular").and_then(|g| g.as_table_like()).filter(|_| table.len() == 1) else { return false };
    let flag = |k: &str, required: bool| match granular.get(k) {
        Some(v) => v.as_bool().is_some(),
        None => !required,
    };
    flag("sandbox_approval", true)
        && flag("rules", true)
        && flag("mcp_elicitations", true)
        && flag("skill_approval", false)
        && flag("request_permissions", false)
}

fn sandbox_known(item: &toml_edit::Item) -> bool {
    item.as_str().is_some_and(|v| SANDBOXES.contains(&v.trim().to_lowercase().as_str()))
}

/// A `default_permissions`: a built-in profile Codex has, or a name of the
/// config's own.
fn permissions_known(item: &toml_edit::Item) -> bool {
    item.as_str().map(str::trim).is_some_and(|n| !n.is_empty() && (!n.starts_with(':') || BUILT_IN_PROFILES.contains(&n)))
}

/// Whether a layer names an approval, a sandbox or a profile Luna does not know.
fn says_unknown(doc: &toml_edit::DocumentMut) -> bool {
    doc.get("approval_policy").is_some_and(|i| !approval_known(i))
        || doc.get("sandbox_mode").is_some_and(|i| !sandbox_known(i))
        || doc.get("default_permissions").is_some_and(|i| !permissions_known(i))
}

/// What the files say: the model and effort of the layers Codex trusts, and
/// whether any layer Codex might read could not be read or names a mode Luna
/// does not know.
#[derive(Default)]
struct Said {
    model: Option<Setting>,
    effort: Option<Setting>,
    closed: bool,
}

fn take(source: &str, doc: &toml_edit::DocumentMut, key: &str) -> Option<Setting> {
    config::string_at(doc, key).map(|value| Setting { value, source: source.to_owned() })
}

/// Async and off to a blocking thread: with no catalog on disk the model
/// list comes from the CLI, which takes seconds.
///
/// That CLI runs with `CODEX_HOME` set to the account (`codex_models`), and
/// a folder's `.codex/config.toml` layers are read: both paths are checked
/// (boundary.rs). Either may be empty: no account gives Luna's own defaults
/// without reading or running anything, and no folder leaves the project's
/// files out.
pub async fn codex_defaults(account_path: OrEmpty<AccountPath>, folder: OrEmpty<AbsPath>) -> CodexDefaults {
    let (account_path, folder) = (account_path.into_string(), folder.into_string());
    tauri::async_runtime::spawn_blocking(move || {
        let models = if account_path.is_empty() { vec![] } else { super::models::models_for(&account_path) };
        resolve(&account_path, &folder, &models)
    })
    .await
    // Nothing read is not nothing said: closed, not Codex's own.
    .unwrap_or_else(|_| settle(Said { closed: true, ..Default::default() }, &[]))
}

fn resolve(account_path: &str, folder: &str, models: &[CodexModel]) -> CodexDefaults {
    settle(read(account_path, folder, false), models)
}

fn settle(said: Said, models: &[CodexModel]) -> CodexDefaults {
    let model = said
        .model
        .filter(|s| !s.value.trim().is_empty())
        .or_else(|| models.first().map(|m| Setting { value: m.slug.clone(), source: OWN.to_owned() }));
    let own_level = model
        .as_ref()
        .and_then(|m| models.iter().find(|l| l.slug == m.value))
        .and_then(|m| m.default_effort.as_deref())
        .and_then(effort_level);
    let effort = said.effort.and_then(|s| effort_level(&s.value).map(|value| Setting { value, source: s.source }));
    let mode = |closed: &str| match said.closed {
        true => Setting { value: closed.to_owned(), source: CLOSED.to_owned() },
        false => Setting { value: OWN.to_owned(), source: OWN.to_owned() },
    };
    CodexDefaults {
        effort: effort
            .or_else(|| own_level.map(|value| Setting { value, source: OWN.to_owned() }))
            .unwrap_or_else(|| Setting { value: STOCK_EFFORT.to_owned(), source: "stock".to_owned() }),
        model,
        approval: mode(CLOSED_APPROVAL),
        sandbox: mode(CLOSED_SANDBOX),
    }
}

/// What the layers say for a run of the account in `folder`: a chat's, or a
/// `helper`'s, which Luna trusts its folder for (pty/start.rs `codex_trust`).
/// Every layer is looked at for what Luna cannot read; the model and effort
/// are taken only from those Codex trusts (codex/trust.rs `Decisions`), as
/// they are passed as flags.
fn read(account_path: &str, folder: &str, helper: bool) -> Said {
    let mut out = Said::default();
    if account_path.is_empty() {
        return out;
    }
    let Ok(account_file) = config::account_file(account_path) else { return out };
    let mut files = vec![("system", config::system_layer(), true), ("account", account_file, true)];
    if !folder.is_empty() {
        match config::project_root_and_layers(account_path, folder) {
            Ok((root, layers)) => {
                let trust = super::trust::Decisions::of(account_path, folder, helper);
                let root = root.to_string_lossy().into_owned();
                for file in layers {
                    let dir = config::layer_dir(&file);
                    let trusted = trust.layer_trusted(&dir, &root, folder);
                    files.push(("project", file, trusted));
                }
            }
            Err(_) => out.closed = true,
        }
    }
    for (source, file, trusted) in files {
        match config::read(&file) {
            Ok(doc) => {
                out.closed |= says_unknown(&doc);
                if trusted {
                    if let Some(s) = take(source, &doc, "model") {
                        out.model = Some(s);
                    }
                    if let Some(s) = take(source, &doc, "model_reasoning_effort") {
                        out.effort = Some(s);
                    }
                }
            }
            Err(_) => out.closed = true,
        }
    }
    out
}

/// A chat is not started where Codex 0.169.0 would refuse it: its effective
/// `approval_policy` is `untrusted`, refused whatever `-a` Luna passes
/// (core/src/config/mod.rs `UnsupportedUntrustedApprovalPolicyError`). Said
/// here, naming the file, rather than as a session that ends at once. Only
/// where Luna is sure: the last word on the approval among the system's, the
/// account's and the project layers Codex trusts is `untrusted`, and no layer
/// Luna takes for untrusted, or cannot read, names one, which Codex might
/// read all the same. Unsure, it starts, and Codex decides.
pub(crate) fn refuses_untrusted(account_path: &str, folder: &str) -> Result<(), String> {
    let Ok(account_file) = config::account_file(account_path) else { return Ok(()) };
    let mut files = vec![(config::system_layer(), true), (account_file, true)];
    if !folder.is_empty() {
        let Ok((root, layers)) = config::project_root_and_layers(account_path, folder) else { return Ok(()) };
        let trust = super::trust::Decisions::of(account_path, folder, false);
        let root = root.to_string_lossy().into_owned();
        files.extend(layers.into_iter().map(|f| {
            let trusted = trust.layer_trusted(&config::layer_dir(&f), &root, folder);
            (f, trusted)
        }));
    }
    let mut last: Option<(bool, String)> = None;
    for (file, trusted) in files {
        let Ok(doc) = config::read(&file) else { return Ok(()) };
        let Some(item) = doc.get("approval_policy") else { continue };
        if !trusted {
            // A layer Luna takes for untrusted that names an approval: Codex
            // may read it, and then its word goes.
            return Ok(());
        }
        let untrusted = item.as_str().is_some_and(|s| s.trim().eq_ignore_ascii_case("untrusted"));
        last = Some((untrusted, file.to_string_lossy().into_owned()));
    }
    match last {
        Some((true, file)) => Err(format!(
            "{file} sets approval_policy = \"untrusted\", which Codex no longer supports: it refuses to start with \
             \"approval_policy = \\\"untrusted\\\" is no longer supported; remove this setting\". Remove the key there, or \
             set on-request or never"
        )),
        _ => Ok(()),
    }
}

/// An approval and a sandbox as a chat is started in: what Luna runs as it
/// is, `on-failure` as `on-request`, and a `granular` or `profile` a chat of
/// 0.20's first builds recorded as Codex's own; anything else closed, both
/// together, as a layer Luna cannot read closes both.
pub(crate) fn launch_mode(approval: &str, sandbox: &str) -> (String, String) {
    use super::launch::{AUTO_REVIEW, READ_ONLY_NET};
    let approval = match approval.trim().to_lowercase().as_str() {
        a @ ("on-request" | "never" | AUTO_REVIEW | OWN) => Some(a.to_string()),
        "on-failure" => Some("on-request".to_string()),
        "granular" => Some(OWN.to_string()),
        _ => None,
    };
    let sandbox = match sandbox.trim().to_lowercase().as_str() {
        s @ ("read-only" | READ_ONLY_NET | "workspace-write" | "danger-full-access" | OWN) => Some(s.to_string()),
        "profile" => Some(OWN.to_string()),
        _ => None,
    };
    match approval {
        Some(approval) => (approval, sandbox.unwrap_or_else(|| CLOSED_SANDBOX.to_string())),
        None => (CLOSED_APPROVAL.to_string(), CLOSED_SANDBOX.to_string()),
    }
}

/// What a helper of this account in this folder starts with when its spawn
/// names nothing (its folder trusted for its run), for `luna_account_get` and
/// the spawn — off the model list as far as it is known without asking the
/// CLI. The approval and the sandbox are `codex` (Codex's own, no flag) or
/// closed (`approvalFrom`, `sandboxFrom` say which).
pub fn flags(account_path: &str, folder: &str) -> serde_json::Value {
    let d = settle(read(account_path, folder, true), &super::models::models_known(account_path));
    serde_json::json!({
        "model": d.model.map(|m| m.value),
        "effort": d.effort.value,
        "approval": d.approval.value,
        "approvalFrom": d.approval.source,
        "sandbox": d.sandbox.value,
        "sandboxFrom": d.sandbox.source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn write(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    fn root(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("luna-codex-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        root
    }

    fn pair(d: &CodexDefaults) -> (&str, &str, &str, &str) {
        (d.approval.value.as_str(), d.approval.source.as_str(), d.sandbox.value.as_str(), d.sandbox.source.as_str())
    }

    const OWN_PAIR: (&str, &str, &str, &str) = (OWN, OWN, OWN, OWN);
    const CLOSED_PAIR: (&str, &str, &str, &str) = ("on-request", "closed", "read-only", "closed");

    /// Whatever the files name that Luna can read, the approval and the
    /// sandbox are Codex's own: Luna claims no value and passes no flag.
    #[test]
    fn what_luna_can_read_is_codexs_own_and_claims_no_value() {
        let root = root("own-modes");
        let account = root.join("account");
        let acc = account.to_string_lossy().into_owned();
        std::fs::create_dir_all(&account).unwrap();
        assert_eq!(pair(&resolve(&acc, "", &[])), OWN_PAIR, "no file");
        for config in [
            "model_reasoning_effort = \"High\"\n",
            "approval_policy = \"never\"\nsandbox_mode = \"danger-full-access\"\n",
            "approval_policy = \"On-Failure\"\nsandbox_mode = \"workspace-write\"\n",
            "approval_policy = { granular = { sandbox_approval = true, rules = false, mcp_elicitations = true } }\n",
            "[approval_policy.granular]\nsandbox_approval = false\nrules = true\nmcp_elicitations = false\n",
            "default_permissions = \":workspace\"\n",
            "default_permissions = \"locked-down\"\n",
        ] {
            write(&account.join("config.toml"), config);
            assert_eq!(pair(&resolve(&acc, "", &[])), OWN_PAIR, "{config}");
        }
        let d = resolve(&acc, "", &[]);
        assert!(d.model.is_none(), "no model list, no model");
        assert_eq!(resolve("", "", &[]).effort.source, "stock");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Any layer naming a mode Luna does not know, or that will not read,
    /// closes both, passed explicitly: `untrusted` among them.
    #[test]
    fn what_luna_cannot_read_is_closed_both() {
        let root = root("closed-modes");
        let account = root.join("account");
        let acc = account.to_string_lossy().into_owned();
        for config in [
            "approval_policy = \"untrusted\"\n",
            "approval_policy = \"sometimes\"\n",
            "approval_policy = 3\n",
            "approval_policy = { granular = { sandbox_approval = true } }\n",
            "approval_policy = { other = true }\n",
            "sandbox_mode = \"read-only-net\"\n",
            "sandbox_mode = \"everything\"\n",
            "default_permissions = \":everything\"\n",
            "default_permissions = 3\n",
            "approval_policy = \"never\"\nsandbox_mode = \"danger-full-access\"\n[broken\n",
        ] {
            write(&account.join("config.toml"), config);
            assert_eq!(pair(&resolve(&acc, "", &[])), CLOSED_PAIR, "{config}");
        }
        // A file that cannot be read: here a folder where the file should be.
        let _ = std::fs::remove_file(account.join("config.toml"));
        std::fs::create_dir_all(account.join("config.toml")).unwrap();
        assert_eq!(pair(&resolve(&acc, "", &[])), CLOSED_PAIR);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A project layer counts for closing whether or not Codex trusts it:
    /// the closed fallback does not depend on Luna's reading of trust. The
    /// model comes only from layers Codex trusts, as it is passed.
    #[test]
    fn every_project_layer_counts_for_closing_and_trusted_ones_for_the_model() {
        let root = root("layers");
        let (account, repo) = (root.join("account"), root.join("repo"));
        let sub = repo.join("app");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(repo.join(".git").join("HEAD"), "ref: refs/heads/main\n").unwrap();
        write(&account.join("config.toml"), "model = \"gpt-acc\"\n");
        write(&repo.join(".codex").join("config.toml"), "model = \"gpt-repo\"\n");
        write(&sub.join(".codex").join("config.toml"), "sandbox_mode = \"workspace-write\"\n");
        let (acc, dir) = (account.to_string_lossy().into_owned(), sub.to_string_lossy().into_owned());
        let d = resolve(&acc, &dir, &[]);
        assert_eq!((d.model.as_ref().unwrap().value.as_str(), pair(&d)), ("gpt-acc", OWN_PAIR), "untrusted: the account's model");
        super::super::trust::codex_trust_folder(acc.clone(), repo.to_string_lossy().into_owned()).unwrap();
        let d = resolve(&acc, &dir, &[]);
        assert_eq!((d.model.as_ref().unwrap().value.as_str(), pair(&d)), ("gpt-repo", OWN_PAIR), "the repository trusted");
        // An untrusted folder's layer naming what Luna does not know.
        let other = root.join("other");
        write(&other.join(".codex").join("config.toml"), "sandbox_mode = \"wide-open\"\n");
        assert_eq!(pair(&resolve(&acc, &other.to_string_lossy(), &[])), CLOSED_PAIR);
        write(&other.join(".codex").join("config.toml"), "approval_policy = [never\n");
        assert_eq!(pair(&resolve(&acc, &other.to_string_lossy(), &[])), CLOSED_PAIR);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_chat_starts_in_what_luna_runs_or_closed() {
        for (asked, want) in [
            (("never", "danger-full-access"), ("never", "danger-full-access")),
            (("On-Failure", "workspace-write"), ("on-request", "workspace-write")),
            ((OWN, OWN), (OWN, OWN)),
            (("never", OWN), ("never", OWN)),
            (("granular", "profile"), (OWN, OWN)),
            (("auto-review", "whatever"), ("auto-review", "read-only")),
            // An approval Luna does not know closes the sandbox with it.
            (("untrusted", "read-only-net"), ("on-request", "read-only")),
            (("", ""), ("on-request", "read-only")),
            (("{granular={}}", "danger-full-access"), ("on-request", "read-only")),
        ] {
            let (a, s) = launch_mode(asked.0, asked.1);
            assert_eq!((a.as_str(), s.as_str()), want, "{asked:?}");
        }
    }

    /// `untrusted` refuses a chat only where Luna is sure Codex reads it as
    /// the last word: the account's, with no project layer naming another;
    /// a project layer that may go over it, read or not, lets Codex decide.
    #[test]
    fn an_untrusted_approval_is_refused_only_where_codex_surely_reads_it() {
        let root = root("untrusted");
        let (account, repo) = (root.join("account"), root.join("repo"));
        let sub = repo.join("app");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(repo.join(".git").join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::create_dir_all(&sub).unwrap();
        let (acc, dir) = (account.to_string_lossy().into_owned(), sub.to_string_lossy().into_owned());
        write(&account.join("config.toml"), "approval_policy = \"untrusted\"\n");
        let e = refuses_untrusted(&acc, "").unwrap_err();
        assert!(e.contains("approval_policy = \"untrusted\"") && e.contains("no longer supported") && e.contains("config.toml"), "{e}");
        assert!(refuses_untrusted(&acc, &dir).is_err(), "no project layer names one");
        // The repository's layer names another, untrusted by Luna's reading:
        // Codex may read it, so the chat starts.
        write(&repo.join(".codex").join("config.toml"), "approval_policy = \"on-request\"\n");
        assert_eq!(refuses_untrusted(&acc, &dir), Ok(()));
        // Trusted, by the repository: it goes over the account's.
        super::super::trust::codex_trust_folder(acc.clone(), repo.to_string_lossy().into_owned()).unwrap();
        assert_eq!(refuses_untrusted(&acc, &dir), Ok(()));
        // The repository's own `untrusted`, trusted, over an account saying never.
        write(&account.join("config.toml"), "approval_policy = \"never\"\n");
        super::super::trust::codex_trust_folder(acc.clone(), repo.to_string_lossy().into_owned()).unwrap();
        write(&repo.join(".codex").join("config.toml"), "approval_policy = \"untrusted\"\n");
        assert!(refuses_untrusted(&acc, &dir).is_err());
        // Only the folder trusted, not the root: Codex skips the root's layer.
        write(&account.join("config.toml"), "approval_policy = \"never\"\n");
        super::super::trust::codex_trust_folder(acc.clone(), dir.clone()).unwrap();
        assert_eq!(refuses_untrusted(&acc, &dir), Ok(()));
        let _ = std::fs::remove_dir_all(&root);
    }

    fn model(slug: &str, level: &str) -> CodexModel {
        CodexModel {
            slug: slug.into(),
            display_name: slug.into(),
            default_effort: Some(level.into()),
            efforts: vec![],
            effort_hints: Default::default(),
        }
    }

    #[test]
    fn nothing_said_is_codexs_first_model_at_its_own_level() {
        let list = [model("gpt-6-sol", "low"), model("gpt-6-astra", "medium")];
        let d = resolve("", "", &list);
        let m = d.model.unwrap();
        assert_eq!((m.value.as_str(), m.source.as_str()), ("gpt-6-sol", "codex"));
        assert_eq!((d.effort.value.as_str(), d.effort.source.as_str()), ("low", "codex"));

        let root = root("own");
        write(&root.join("config.toml"), "model = \"gpt-6-astra\"\n");
        let d = resolve(&root.to_string_lossy(), "", &list);
        assert_eq!(d.model.unwrap().source, "account");
        assert_eq!(d.effort.value, "medium", "the named model's own level");
        let _ = std::fs::remove_dir_all(&root);
    }
}
