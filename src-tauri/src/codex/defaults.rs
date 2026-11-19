//! Where a new Codex chat's model, effort, approval and sandbox come from when
//! nobody has said otherwise: Codex's own `config.toml` layers, its system one
//! first (`config::system_layer`), then the account's, then the project's
//! `.codex/config.toml` layers — which Codex only reads for a trusted folder,
//! so neither does this.
//!
//! Reading them is what makes the dialog show what a chat will run in, and
//! the flags Luna passes agree with the files instead of fighting them.
//!
//! The model and its effort, where nothing is said, are what Codex itself
//! would pick (`source: codex`): the first model of its catalog
//! (codex/models.rs) at that model's default level, named so the dialog shows
//! the model a chat really runs on. The approval and the sandbox, where the
//! files name none, are Codex's own defaults (`source: codex`), and a chat
//! left on them is started without a flag for them (`left_to_codex`): Codex
//! 0.169.0 asks `on-request`, and runs `read-only`, or `workspace-write` in a
//! folder it trusts (core/src/config/mod.rs, the approval's `unwrap_or_else`;
//! config/src/config_toml.rs `derive_permission_profile`).
//!
//! A sandbox may also come of `default_permissions`: Codex's built-in
//! profiles (`:read-only`, `:workspace`, `:danger-full-access`) read as the
//! sandbox each stands for; any other is a profile of the user's own,
//! `PROFILE`, shown by its name and passed through, never overridden. Of the
//! two keys, the last layer to set either wins, and within one layer
//! `default_permissions` (config/src/permission_config.rs
//! `resolve_permission_config`).
//!
//! Where Luna has to pick and cannot tell what the files say — a layer that
//! will not read or parse, a value it does not know — the approval and the
//! sandbox are `on-request` and `read-only` (`source: closed`). Codex's
//! `on-failure` is its alias of `on-request`; its `untrusted`, which 0.169.0
//! refuses to start on ("no longer supported"), reads as a value Luna does
//! not know, and a session is not started on it (`refuses_untrusted`); a `{
//! granular = … }` table is `granular`, passed to Codex as it is
//! (`granular_policy`).

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
    /// The name of the user's own permission profile the sandbox stands for
    /// where it is `PROFILE`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

/// The effort a chat starts with when neither the files nor a model list say one.
const STOCK_EFFORT: &str = "high";

/// Codex's own approval and sandbox where the files name none (0.169.0; see
/// the module's comment): the sandbox `workspace-write` in a folder Codex
/// trusts.
const CODEX_APPROVAL: &str = "on-request";
const CODEX_SANDBOX: &str = "read-only";
const CODEX_SANDBOX_TRUSTED: &str = "workspace-write";
const CODEX: &str = "codex";

/// Luna's word for a sandbox that is a permission profile of the user's own
/// (`default_permissions = "<name>"`): Codex runs it as the files define it,
/// and no `--sandbox` of Luna's goes over it.
pub(crate) const PROFILE: &str = "profile";

/// Codex's built-in permission profiles, each as the sandbox mode it stands
/// for (0.169.0, protocol/src/models.rs `BUILT_IN_PERMISSION_PROFILE_*`).
const BUILT_IN_PROFILES: [(&str, &str); 3] =
    [(":read-only", "read-only"), (":workspace", "workspace-write"), (":danger-full-access", "danger-full-access")];

/// What the approval and the sandbox are where Luna cannot tell what the files
/// say: Codex asks before anything the sandbox would block, and the sandbox
/// writes nothing.
pub(crate) const CLOSED_APPROVAL: &str = "on-request";
pub(crate) const CLOSED_SANDBOX: &str = "read-only";
const CLOSED: &str = "closed";

/// Luna's word for an `approval_policy = { granular = { … } }` table: Codex
/// asks a person only for the kinds of request the table lets through and
/// turns the rest down unasked. `--ask-for-approval` does not take it; it goes
/// to Codex as `-c approval_policy=…` (`granular_policy`).
pub(crate) const GRANULAR: &str = "granular";

const SANDBOXES: [&str; 3] = ["read-only", "workspace-write", "danger-full-access"];

/// The levels are per model and come off the CLI's model list, so any plain
/// word passes.
fn effort_level(raw: &str) -> Option<String> {
    let v = raw.trim().to_lowercase();
    (!v.is_empty() && v.chars().all(|c| c.is_ascii_lowercase())).then_some(v)
}

/// What one layer says of a key: nothing, a value Luna knows, or one it does
/// not (None).
type Word = Option<Option<String>>;

/// An `approval_policy` as Luna runs it: `on-request` (Codex's `on-failure` is
/// its alias), `never`, or `granular` with the table as Codex will be given
/// it; None for anything else, `untrusted` included.
fn approval_of(item: &toml_edit::Item) -> Option<(String, Option<String>)> {
    if let Some(s) = item.as_str() {
        return match s.trim().to_lowercase().as_str() {
            "on-request" | "on-failure" => Some(("on-request".into(), None)),
            "never" => Some(("never".into(), None)),
            _ => None,
        };
    }
    let table = item.as_table_like()?;
    if table.len() != 1 {
        return None;
    }
    granular_table(table.get("granular")?).map(|t| (GRANULAR.to_string(), Some(t)))
}

/// A `granular` table as `-c approval_policy=` takes it, rebuilt from the
/// fields Codex 0.169.0 reads (`GranularApprovalConfig`): `sandbox_approval`,
/// `rules` and `mcp_elicitations` it requires, `skill_approval` and
/// `request_permissions` default to false. None if any is missing or not a
/// boolean: Codex would refuse to start on it.
fn granular_table(item: &toml_edit::Item) -> Option<String> {
    let t = item.as_table_like()?;
    let flag = |k: &str, required: bool| match t.get(k) {
        Some(v) => v.as_bool(),
        None => (!required).then_some(false),
    };
    let fields = [
        ("sandbox_approval", flag("sandbox_approval", true)?),
        ("rules", flag("rules", true)?),
        ("skill_approval", flag("skill_approval", false)?),
        ("request_permissions", flag("request_permissions", false)?),
        ("mcp_elicitations", flag("mcp_elicitations", true)?),
    ];
    let inner = fields.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(",");
    Some(format!("{{granular={{{inner}}}}}"))
}

fn sandbox_of(item: &toml_edit::Item) -> Option<String> {
    let v = item.as_str()?.trim().to_lowercase();
    SANDBOXES.contains(&v.as_str()).then_some(v)
}

/// A `default_permissions` as a sandbox: a built-in profile as the mode it
/// stands for, one of the user's own as `PROFILE` with its name; None for a
/// built-in name Codex does not have, or a value that is not a name.
fn permissions_of(item: &toml_edit::Item) -> (Option<String>, Option<String>) {
    let Some(name) = item.as_str().map(str::trim).filter(|n| !n.is_empty()) else { return (None, None) };
    if name.starts_with(':') {
        return (BUILT_IN_PROFILES.iter().find(|(p, _)| *p == name).map(|(_, mode)| mode.to_string()), None);
    }
    (Some(PROFILE.to_string()), Some(name.to_string()))
}

/// What the files say, as they say it: per key the last layer's word and its
/// source, and whether a layer could not be read at all.
#[derive(Default)]
struct Said {
    model: Option<Setting>,
    effort: Option<Setting>,
    approval: Option<(Word, String)>,
    /// The table a `granular` approval stands for, from the layer that set it.
    granular: Option<String>,
    /// The file whose `approval_policy = "untrusted"` is the last word on it.
    untrusted: Option<String>,
    sandbox: Option<(Word, String)>,
    /// The user's own profile the sandbox stands for, from the layer that set it.
    profile: Option<String>,
    /// Codex trusts the folder: its own sandbox there is `workspace-write`.
    trusted: bool,
    unreadable: bool,
}

fn take(source: &str, doc: &toml_edit::DocumentMut, key: &str) -> Option<Setting> {
    config::string_at(doc, key).map(|value| Setting { value, source: source.to_owned() })
}

/// One layer over the ones before it; `file`, its path, for what is refused.
fn apply(out: &mut Said, source: &str, file: &str, doc: &toml_edit::DocumentMut) {
    if let Some(s) = take(source, doc, "model") {
        out.model = Some(s);
    }
    if let Some(s) = take(source, doc, "model_reasoning_effort") {
        out.effort = Some(s);
    }
    if let Some(item) = doc.get("approval_policy") {
        let read = approval_of(item);
        out.granular = read.as_ref().and_then(|(_, t)| t.clone());
        out.untrusted = item.as_str().is_some_and(|s| s.trim().eq_ignore_ascii_case("untrusted")).then(|| file.to_owned());
        out.approval = Some((Some(read.map(|(v, _)| v)), source.to_owned()));
    }
    if let Some(item) = doc.get("sandbox_mode") {
        out.sandbox = Some((Some(sandbox_of(item)), source.to_owned()));
        out.profile = None;
    }
    // After `sandbox_mode`: within one layer it wins.
    if let Some(item) = doc.get("default_permissions") {
        let (sandbox, profile) = permissions_of(item);
        out.sandbox = Some((Some(sandbox), source.to_owned()));
        out.profile = profile;
    }
}

/// Async and off to a blocking thread: with no catalog on disk the model
/// list comes from the CLI, which takes seconds.
///
/// That CLI runs with `CODEX_HOME` set to the account (`codex_models`), and
/// a trusted folder's `.codex/config.toml` is read: both paths are checked
/// (boundary.rs). Either may be empty: no account gives Luna's own defaults
/// without reading or running anything, and no folder leaves the project's
/// file out.
#[tauri::command]
pub async fn codex_defaults(account_path: OrEmpty<AccountPath>, folder: OrEmpty<AbsPath>) -> CodexDefaults {
    let (account_path, folder) = (account_path.into_string(), folder.into_string());
    tauri::async_runtime::spawn_blocking(move || {
        let models = if account_path.is_empty() { vec![] } else { super::models::models_for(&account_path) };
        resolve(&account_path, &folder, &models)
    })
    .await
    // Nothing read is not nothing said: closed, not Codex's own.
    .unwrap_or_else(|_| settle(Said { unreadable: true, ..Default::default() }, &[]))
}

fn resolve(account_path: &str, folder: &str, models: &[CodexModel]) -> CodexDefaults {
    settle(read(account_path, folder), models)
}

/// A mode the files settled on: the last layer's word where Luna knows it;
/// `closed` where it does not, or where it said nothing beside a layer that
/// could not be read or a mode Luna does not know; else Codex's own.
fn mode(word: Option<(Word, String)>, wary: bool, own: &str, closed: &str) -> Setting {
    match word {
        Some((Some(Some(value)), source)) => Setting { value, source },
        Some(_) => Setting { value: closed.to_owned(), source: CLOSED.to_owned() },
        None if wary => Setting { value: closed.to_owned(), source: CLOSED.to_owned() },
        None => Setting { value: own.to_owned(), source: CODEX.to_owned() },
    }
}

fn settle(said: Said, models: &[CodexModel]) -> CodexDefaults {
    let own_sandbox = if said.trusted { CODEX_SANDBOX_TRUSTED } else { CODEX_SANDBOX };
    let model = said
        .model
        .filter(|s| !s.value.trim().is_empty())
        .or_else(|| models.first().map(|m| Setting { value: m.slug.clone(), source: "codex".to_owned() }));
    let own_level = model
        .as_ref()
        .and_then(|m| models.iter().find(|l| l.slug == m.value))
        .and_then(|m| m.default_effort.as_deref())
        .and_then(effort_level);
    let effort = said.effort.and_then(|s| effort_level(&s.value).map(|value| Setting { value, source: s.source }));
    let unknown = |w: &Option<(Word, String)>| matches!(w, Some((Some(None), _)));
    let wary = said.unreadable || unknown(&said.approval) || unknown(&said.sandbox);
    let (approval, sandbox) = if said.unreadable {
        // A layer Luna could not read may say anything over the others.
        (mode(None, true, CODEX_APPROVAL, CLOSED_APPROVAL), mode(None, true, own_sandbox, CLOSED_SANDBOX))
    } else {
        (mode(said.approval, wary, CODEX_APPROVAL, CLOSED_APPROVAL), mode(said.sandbox, wary, own_sandbox, CLOSED_SANDBOX))
    };
    let profile = said.profile.filter(|_| sandbox.value == PROFILE);
    CodexDefaults {
        effort: effort
            .or_else(|| own_level.map(|value| Setting { value, source: CODEX.to_owned() }))
            .unwrap_or_else(|| Setting { value: STOCK_EFFORT.to_owned(), source: "stock".to_owned() }),
        model,
        approval,
        sandbox,
        profile,
    }
}

/// Whether Codex reads the project's layers for a session of this account in
/// this folder: trusted in the account's config, under a folder trusted on
/// this machine, or for the account's sessions while it was in use
/// (pty/start.rs `codex_trust`).
fn project_trusted(account_path: &str, folder: &str) -> bool {
    super::trust::root_override(folder).is_some()
        || super::trust::session_override(account_path, folder).is_some()
        || super::trust::codex_folder_trusted(account_path.to_owned(), folder.to_owned()).unwrap_or(false)
}

fn read(account_path: &str, folder: &str) -> Said {
    let mut out = Said::default();
    if account_path.is_empty() {
        return out;
    }
    let Ok(account_file) = config::account_file(account_path) else { return out };
    let mut files = vec![("system", config::system_layer()), ("account", account_file)];
    out.trusted = !folder.is_empty() && project_trusted(account_path, folder);
    if out.trusted {
        match config::project_layers(account_path, folder) {
            Ok(layers) => files.extend(layers.into_iter().map(|f| ("project", f))),
            Err(_) => out.unreadable = true,
        }
    }
    for (source, file) in files {
        match config::read(&file) {
            Ok(doc) => apply(&mut out, source, &file.to_string_lossy(), &doc),
            Err(_) => out.unreadable = true,
        }
    }
    out
}

/// A session is not started where the files' last word on the approval is
/// `untrusted`: Codex 0.169.0 refuses to start on it, as a value of the
/// config, whatever flag Luna passes (core/src/config/mod.rs
/// `UnsupportedUntrustedApprovalPolicyError`). Said here, naming the file,
/// rather than as a session that ends at once.
pub(crate) fn refuses_untrusted(account_path: &str, folder: &str) -> Result<(), String> {
    match read(account_path, folder).untrusted {
        Some(file) => Err(format!(
            "{file} sets approval_policy = \"untrusted\", which Codex no longer supports: it refuses to start with \
             \"approval_policy = \\\"untrusted\\\" is no longer supported; remove this setting\". Remove the key there, or \
             set on-request or never"
        )),
        None => Ok(()),
    }
}

/// What of a chat's `approval` and `sandbox` Luna leaves to Codex, passing no
/// flag for it, as the files stand now: one they name none of, still what
/// Codex would settle on by itself (`source: codex`), and a profile of the
/// user's own (`PROFILE`) the files still name. (approval, sandbox).
pub(crate) fn left_to_codex(account_path: &str, folder: &str, approval: &str, sandbox: &str) -> (bool, bool) {
    let d = resolve(account_path, folder, &[]);
    let own = |s: &Setting, v: &str| s.source == CODEX && s.value == v;
    (own(&d.approval, approval), own(&d.sandbox, sandbox) || (sandbox == PROFILE && d.profile.is_some()))
}

/// The `approval_policy` table a chat whose approval is `granular` is started
/// with, as the files say it now; None if they no longer say one, and the
/// chat then starts in `CLOSED_APPROVAL` (pty/start.rs).
pub(crate) fn granular_policy(account_path: &str, folder: &str) -> Option<String> {
    let said = read(account_path, folder);
    if said.unreadable {
        return None;
    }
    match said.approval {
        Some((Some(Some(v)), _)) if v == GRANULAR => said.granular,
        _ => None,
    }
}

/// An approval and a sandbox as a chat is started in: what Luna runs as it
/// is, anything else as it would read in the files — `on-failure` as
/// `on-request`, a value it does not know (`untrusted` included) closed. An
/// approval closed closes the sandbox with it, as a layer saying what Luna
/// does not know does (`settle`).
pub(crate) fn launch_mode(approval: &str, sandbox: &str) -> (String, String) {
    use super::launch::{AUTO_REVIEW, READ_ONLY_NET};
    let approval = match approval.trim().to_lowercase().as_str() {
        a @ ("on-request" | "never" | AUTO_REVIEW | GRANULAR) => Some(a.to_string()),
        "on-failure" => Some("on-request".to_string()),
        _ => None,
    };
    let sandbox = match sandbox.trim().to_lowercase().as_str() {
        s @ ("read-only" | READ_ONLY_NET | "workspace-write" | "danger-full-access" | PROFILE) => Some(s.to_string()),
        _ => None,
    };
    match approval {
        Some(approval) => (approval, sandbox.unwrap_or_else(|| CLOSED_SANDBOX.to_string())),
        None => (CLOSED_APPROVAL.to_string(), CLOSED_SANDBOX.to_string()),
    }
}

/// What the files settle on for a session of this account in this folder,
/// for `luna_account_get` and a helper spawned without a mode — off the model
/// list as far as it is known without asking the CLI. `approvalFrom` and
/// `sandboxFrom` say whose: `codex` for Codex's own defaults.
pub fn flags(account_path: &str, folder: &str) -> serde_json::Value {
    let d = resolve(account_path, folder, &super::models::models_known(account_path));
    serde_json::json!({
        "model": d.model.map(|m| m.value),
        "effort": d.effort.value,
        "approval": d.approval.value,
        "approvalFrom": d.approval.source,
        "sandbox": d.sandbox.value,
        "sandboxFrom": d.sandbox.source,
        "profile": d.profile,
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

    #[test]
    fn project_wins_only_when_trusted() {
        let root = root("defaults");
        let account = root.join("account");
        let folder = root.join("project");
        write(
            &account.join("config.toml"),
            "model = \"gpt-5-codex\"\nmodel_reasoning_effort = \"xhigh\"\napproval_policy = \"never\"\nsandbox_mode = \"danger-full-access\"\n",
        );
        write(&folder.join(".codex").join("config.toml"), "model = \"gpt-5-mini\"\nsandbox_mode = \"read-only\"\n");
        let acc = account.to_string_lossy().into_owned();
        let dir = folder.to_string_lossy().into_owned();

        let d = resolve(&acc, &dir, &[]);
        assert_eq!(d.model.as_ref().unwrap().value, "gpt-5-codex");
        assert_eq!(d.model.as_ref().unwrap().source, "account");
        assert_eq!(pair(&d), ("never", "account", "danger-full-access", "account"), "an untrusted project's layer is not read");

        super::super::trust::codex_trust_folder(acc.clone(), dir.clone()).unwrap();
        let d = resolve(&acc, &dir, &[]);
        let model = d.model.as_ref().unwrap();
        assert_eq!(model.value, "gpt-5-mini");
        assert_eq!(model.source, "project");
        assert_eq!(d.effort.value, "xhigh");
        assert_eq!(pair(&d), ("never", "account", "read-only", "project"), "the project's sandbox over the account's");

        // The project saying something Luna does not know closes the pair,
        // over what the account says.
        write(&folder.join(".codex").join("config.toml"), "sandbox_mode = \"wide-open\"\n");
        assert_eq!(pair(&resolve(&acc, &dir, &[])), ("never", "account", "read-only", "closed"));
        // A project layer that will not parse may say anything: closed, both.
        write(&folder.join(".codex").join("config.toml"), "approval_policy = [never\n");
        assert_eq!(pair(&resolve(&acc, &dir, &[])), ("on-request", "closed", "read-only", "closed"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn nothing_said_is_codexs_own_and_nothing_known_is_closed() {
        let root = root("stock");
        let account = root.join("account");
        let acc = account.to_string_lossy().into_owned();
        // No file at all, or one that says neither: Codex's own.
        std::fs::create_dir_all(&account).unwrap();
        assert_eq!(pair(&resolve(&acc, "", &[])), (CODEX_APPROVAL, "codex", CODEX_SANDBOX, "codex"));
        write(&account.join("config.toml"), "model_reasoning_effort = \"High\"\n");
        let d = resolve(&acc, "", &[]);
        assert!(d.model.is_none(), "no model list, no model");
        assert_eq!((d.effort.value.as_str(), d.effort.source.as_str()), ("high", "account"));
        assert_eq!(pair(&d), (CODEX_APPROVAL, "codex", CODEX_SANDBOX, "codex"));
        let none = resolve("", "", &[]);
        assert_eq!(none.effort.source, "stock");

        // Each approval: as Codex reads it, or closed. A value Luna does not
        // know closes the sandbox the file leaves unsaid too.
        for (policy, want) in [
            ("\"never\"", ("never", "account", CODEX_SANDBOX, "codex")),
            ("\"on-request\"", ("on-request", "account", CODEX_SANDBOX, "codex")),
            ("\"On-Failure\"", ("on-request", "account", CODEX_SANDBOX, "codex")),
            ("\"untrusted\"", ("on-request", "closed", "read-only", "closed")),
            ("\"sometimes\"", ("on-request", "closed", "read-only", "closed")),
            ("3", ("on-request", "closed", "read-only", "closed")),
            (
                "{ granular = { sandbox_approval = true, rules = false, mcp_elicitations = true } }",
                ("granular", "account", CODEX_SANDBOX, "codex"),
            ),
            ("{ granular = { sandbox_approval = true } }", ("on-request", "closed", "read-only", "closed")),
            (
                "{ granular = { sandbox_approval = \"yes\", rules = true, mcp_elicitations = true } }",
                ("on-request", "closed", "read-only", "closed"),
            ),
            ("{ other = true }", ("on-request", "closed", "read-only", "closed")),
        ] {
            write(&account.join("config.toml"), &format!("approval_policy = {policy}\n"));
            assert_eq!(pair(&resolve(&acc, "", &[])), want, "{policy}");
        }
        // The table as a table of its own, too.
        write(
            &account.join("config.toml"),
            "[approval_policy.granular]\nsandbox_approval = false\nrules = true\nmcp_elicitations = false\n",
        );
        assert_eq!(pair(&resolve(&acc, "", &[])), ("granular", "account", CODEX_SANDBOX, "codex"));
        assert_eq!(
            granular_policy(&acc, "").as_deref(),
            Some("{granular={sandbox_approval=false,rules=true,skill_approval=false,request_permissions=false,mcp_elicitations=false}}")
        );
        // Each sandbox: Codex's three, or closed, the approval with it.
        for (sandbox, want) in [
            ("read-only", ("never", "account", "read-only", "account")),
            ("workspace-write", ("never", "account", "workspace-write", "account")),
            ("danger-full-access", ("never", "account", "danger-full-access", "account")),
            ("read-only-net", ("never", "account", "read-only", "closed")),
            ("everything", ("never", "account", "read-only", "closed")),
        ] {
            write(&account.join("config.toml"), &format!("approval_policy = \"never\"\nsandbox_mode = \"{sandbox}\"\n"));
            assert_eq!(pair(&resolve(&acc, "", &[])), want, "{sandbox}");
        }
        write(&account.join("config.toml"), "sandbox_mode = \"everything\"\n");
        assert_eq!(pair(&resolve(&acc, "", &[])), ("on-request", "closed", "read-only", "closed"), "the unsaid approval closed too");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_config_luna_cannot_read_is_closed_never_codexs_own() {
        let root = root("unread");
        let account = root.join("account");
        let acc = account.to_string_lossy().into_owned();
        // Malformed TOML, after the explicit bypass it might have said.
        write(&account.join("config.toml"), "approval_policy = \"never\"\nsandbox_mode = \"danger-full-access\"\n[broken\n");
        assert_eq!(pair(&resolve(&acc, "", &[])), ("on-request", "closed", "read-only", "closed"));
        assert_eq!(granular_policy(&acc, ""), None);
        // A file that cannot be read: here a folder where the file should be.
        let _ = std::fs::remove_file(account.join("config.toml"));
        std::fs::create_dir_all(account.join("config.toml")).unwrap();
        assert_eq!(pair(&resolve(&acc, "", &[])), ("on-request", "closed", "read-only", "closed"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_trusted_projects_layers_go_over_the_account_from_its_root_down() {
        let root = root("layers");
        let (account, repo) = (root.join("account"), root.join("repo"));
        let sub = repo.join("app");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(repo.join(".git").join("HEAD"), "ref: refs/heads/main\n").unwrap();
        write(&account.join("config.toml"), "approval_policy = \"never\"\nsandbox_mode = \"danger-full-access\"\n");
        write(&repo.join(".codex").join("config.toml"), "approval_policy = \"on-failure\"\n");
        write(&sub.join(".codex").join("config.toml"), "sandbox_mode = \"workspace-write\"\n");
        let (acc, dir) = (account.to_string_lossy().into_owned(), sub.to_string_lossy().into_owned());
        super::super::trust::codex_trust_folder(acc.clone(), dir.clone()).unwrap();
        let d = resolve(&acc, &dir, &[]);
        assert_eq!(pair(&d), ("on-request", "project", "workspace-write", "project"));
        // The deepest layer's granular table is the one passed on.
        write(
            &sub.join(".codex").join("config.toml"),
            "approval_policy = { granular = { sandbox_approval = true, rules = true, mcp_elicitations = false } }\n",
        );
        assert_eq!(pair(&resolve(&acc, &dir, &[])).0, "granular");
        assert!(granular_policy(&acc, &dir).unwrap().contains("mcp_elicitations=false"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_chat_starts_in_what_luna_runs_or_closed() {
        for (asked, want) in [
            (("never", "danger-full-access"), ("never", "danger-full-access")),
            (("On-Failure", "workspace-write"), ("on-request", "workspace-write")),
            (("granular", "read-only"), ("granular", "read-only")),
            (("never", "profile"), ("never", "profile")),
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

    /// Codex's `default_permissions`: a built-in profile is the mode it stands
    /// for, one of the user's own is passed through by name; the last layer to
    /// name either key wins, and within one layer the profile.
    #[test]
    fn a_permission_profile_is_its_mode_or_passed_through_by_name() {
        let root = root("profiles");
        let account = root.join("account");
        let acc = account.to_string_lossy().into_owned();
        let sandbox = |d: &CodexDefaults| (d.sandbox.value.clone(), d.sandbox.source.clone(), d.profile.clone());
        let s = |v: &str, src: &str, p: Option<&str>| (v.to_string(), src.to_string(), p.map(str::to_owned));
        for (config, want) in [
            ("default_permissions = \":read-only\"\n", s("read-only", "account", None)),
            ("default_permissions = \":workspace\"\n", s("workspace-write", "account", None)),
            ("default_permissions = \":danger-full-access\"\n", s("danger-full-access", "account", None)),
            ("default_permissions = \"locked-down\"\n", s(PROFILE, "account", Some("locked-down"))),
            ("sandbox_mode = \"danger-full-access\"\ndefault_permissions = \"mine\"\n", s(PROFILE, "account", Some("mine"))),
            ("default_permissions = \":everything\"\n", s("read-only", "closed", None)),
            ("default_permissions = 3\n", s("read-only", "closed", None)),
        ] {
            write(&account.join("config.toml"), config);
            assert_eq!(sandbox(&resolve(&acc, "", &[])), want, "{config}");
        }
        // A profile left as the files name it is Codex's to run: no flag.
        write(&account.join("config.toml"), "default_permissions = \"mine\"\n");
        assert_eq!(left_to_codex(&acc, "", "on-request", PROFILE), (true, true));
        assert_eq!(left_to_codex(&acc, "", "never", "read-only"), (false, false));
        // A trusted project's sandbox_mode over the account's profile.
        let folder = root.join("project");
        write(&folder.join(".codex").join("config.toml"), "sandbox_mode = \"workspace-write\"\n");
        let dir = folder.to_string_lossy().into_owned();
        super::super::trust::codex_trust_folder(acc.clone(), dir.clone()).unwrap();
        assert_eq!(sandbox(&resolve(&acc, &dir, &[])), s("workspace-write", "project", None));
        assert_eq!(left_to_codex(&acc, &dir, "on-request", PROFILE), (true, false), "named no more");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Nothing named: Codex's own, on-request, and read-only or, in a folder
    /// it trusts, workspace-write; left to Codex, so no flag passes for them.
    #[test]
    fn nothing_named_is_left_to_codex() {
        let root = root("left-to-codex");
        let (account, folder) = (root.join("account"), root.join("project"));
        std::fs::create_dir_all(&folder).unwrap();
        write(&account.join("config.toml"), "model = \"m\"\n");
        let (acc, dir) = (account.to_string_lossy().into_owned(), folder.to_string_lossy().into_owned());
        assert_eq!(pair(&resolve(&acc, &dir, &[])), ("on-request", "codex", "read-only", "codex"));
        assert_eq!(left_to_codex(&acc, &dir, "on-request", "read-only"), (true, true));
        assert_eq!(left_to_codex(&acc, &dir, "never", "workspace-write"), (false, false), "changed in the dialog: passed");
        super::super::trust::codex_trust_folder(acc.clone(), dir.clone()).unwrap();
        assert_eq!(pair(&resolve(&acc, &dir, &[])), ("on-request", "codex", "workspace-write", "codex"));
        // Named, it is passed as named.
        write(&account.join("config.toml"), "approval_policy = \"on-request\"\n");
        super::super::trust::codex_trust_folder(acc.clone(), dir.clone()).unwrap();
        assert_eq!(left_to_codex(&acc, &dir, "on-request", "workspace-write"), (false, true));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `untrusted`, which Codex refuses to start on as a value of the config
    /// whatever the flags, refuses the session by the key and the file; a
    /// later layer naming another approval lets it start.
    #[test]
    fn an_untrusted_approval_is_refused_naming_the_key() {
        let root = root("untrusted");
        let (account, folder) = (root.join("account"), root.join("project"));
        let (acc, dir) = (account.to_string_lossy().into_owned(), folder.to_string_lossy().into_owned());
        write(&account.join("config.toml"), "approval_policy = \"untrusted\"\n");
        let e = refuses_untrusted(&acc, "").unwrap_err();
        assert!(e.contains("approval_policy = \"untrusted\"") && e.contains("no longer supported") && e.contains("config.toml"), "{e}");
        write(&folder.join(".codex").join("config.toml"), "approval_policy = \"never\"\n");
        super::super::trust::codex_trust_folder(acc.clone(), dir.clone()).unwrap();
        assert_eq!(refuses_untrusted(&acc, &dir), Ok(()));
        write(&account.join("config.toml"), "approval_policy = \"never\"\n");
        assert_eq!(refuses_untrusted(&acc, ""), Ok(()));
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
