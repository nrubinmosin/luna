//! An account's own settings file — Claude Code's `settings.json`, Codex's
//! `config.toml` — as Luna can help with it: settings recommended for anyone
//! running the CLI inside Luna, applied key by key, and a "parent" account per
//! provider whose file is copied onto the others.
//!
//! Every write keeps what it does not touch: other keys, their order, and in
//! the TOML file the user's comments. Credentials, logins and Codex's trusted
//! projects never travel.

use crate::provider::Provider;
use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use toml_edit::{DocumentMut, Item};

/// One recommendation as the dialog lists it.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Rec {
    /// Dotted path, e.g. `tui` or `env.CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY`.
    pub key: String,
    /// What the file says now, as written there; None when it says nothing.
    pub current: Option<String>,
    pub recommended: String,
    pub why: String,
    /// Ticked when the dialog opens. Off for what is a matter of taste.
    pub default_on: bool,
    /// The file already says exactly this.
    pub applied: bool,
}

/// A recommendation in the making: the path, the value, and who it is for.
struct Spec {
    path: &'static [&'static str],
    value: Value,
    why: &'static str,
    default_on: bool,
    /// Windows only, e.g. a PowerShell default.
    windows_only: bool,
}

fn spec(path: &'static [&'static str], value: Value, why: &'static str, default_on: bool) -> Spec {
    Spec { path, value, why, default_on, windows_only: false }
}

/// Claude Code 2.1, every key checked against the CLI's own settings schema.
fn claude_specs() -> Vec<Spec> {
    vec![
        spec(&["tui"], json!("fullscreen"),
            "Draws on the alternate screen with its own scrollback: no flicker, and a pane resize redraws cleanly.", true),
        spec(&["spinnerTipsEnabled"], json!(false), "No tips under the spinner.", true),
        spec(&["feedbackDrafts"], json!("off"), "No feedback drafts queued by the model.", true),
        spec(&["env", "CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY"], json!("1"), "No feedback survey mid-session.", true),
        spec(&["awaySummaryEnabled"], json!(false),
            "No recap when you come back to a pane: Luna's list already says what each chat did.", true),
        spec(&["skipDangerousModePermissionPrompt"], json!(true),
            "Bypass is a choice made in Luna's new-chat dialog; the CLI need not ask again.", true),
        Spec { windows_only: true, ..spec(&["defaultShell"], json!("powershell"),
            "`!` commands in the input box run in PowerShell instead of bash.", true) },
        spec(&["cleanupPeriodDays"], json!(14), "Transcripts older than two weeks are cleaned up (default 30).", false),
        spec(&["attribution"], json!({ "commit": "", "pr": "", "sessionUrl": false }),
            "No Co-Authored-By or session links in commits and PRs.", false),
        spec(&["disableRemoteControl"], json!(true), "No Remote Control for sessions on this account.", false),
        spec(&["effortLevel"], json!("high"), "New sessions start at high effort.", false),
        spec(&["permissions", "defaultMode"], json!("bypassPermissions"), "New sessions start in Bypass.", false),
    ]
}

/// Codex 0.162, the same comforts in Codex's words; every key checked against
/// the CLI's config structs.
fn codex_specs() -> Vec<Spec> {
    vec![
        spec(&["check_for_update_on_startup"], json!(false), "Luna keeps Codex up to date itself.", true),
        spec(&["tui", "show_tooltips"], json!(false), "No tips in the composer.", true),
        spec(&["tui", "notifications"], json!(false),
            "No desktop notifications from Codex: Luna says when a chat is waiting for you.", true),
        spec(&["analytics", "enabled"], json!(false), "No usage analytics.", true),
        spec(&["feedback", "enabled"], json!(false), "No feedback prompts.", true),
        spec(&["web_search"], json!("live"), "Web search, as Claude Code has it.", true),
        spec(&["model_reasoning_effort"], json!("high"), "New sessions start at high effort.", false),
        spec(&["approval_policy"], json!("never"), "With the next one: new sessions start in Bypass.", false),
        spec(&["sandbox_mode"], json!("danger-full-access"), "With the one above: no sandbox.", false),
    ]
}

fn specs(provider: Provider) -> Vec<Spec> {
    let all = match provider {
        Provider::Claude => claude_specs(),
        Provider::Codex => codex_specs(),
    };
    all.into_iter().filter(|s| !s.windows_only || cfg!(windows)).collect()
}

// ------------------------------------------------------------------ files --

fn claude_file(account_path: &str) -> Result<PathBuf, String> {
    if account_path.is_empty() {
        return Err("No account selected".into());
    }
    Ok(Path::new(account_path).join("settings.json"))
}

/// The file as an object, or an empty one when it is missing. A file that
/// does not parse is an error: writing over it would lose the user's settings.
fn read_claude(path: &Path) -> Result<Value, String> {
    match std::fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Ok(json!({})),
        Ok(text) => {
            let v: Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
            if v.is_object() { Ok(v) } else { Err(format!("{}: not a JSON object", path.display())) }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn write_claude(path: &Path, v: &Value) -> Result<(), String> {
    let text = serde_json::to_string_pretty(v).map_err(|e| e.to_string())? + "\n";
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    // A rename refused (the CLI holding the file) leaves no temp file behind.
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })
}

fn json_at<'a>(v: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter().try_fold(v, |cur, k| cur.get(*k))
}

fn json_set(v: &mut Value, path: &[&str], value: Value) {
    let (last, parents) = path.split_last().expect("non-empty path");
    let mut cur = v;
    for k in parents {
        if !cur.get(*k).is_some_and(Value::is_object) {
            cur[*k] = json!({});
        }
        cur = cur.get_mut(*k).expect("just set");
    }
    cur[*last] = value;
}

fn toml_at<'a>(doc: &'a DocumentMut, path: &[&str]) -> Option<&'a Item> {
    let mut cur = doc.as_item();
    for k in path {
        cur = cur.get(*k)?;
    }
    Some(cur)
}

/// A JSON scalar as a TOML value; the Codex specs use nothing else.
fn toml_value(v: &Value) -> Option<toml_edit::Value> {
    match v {
        Value::Bool(b) => Some((*b).into()),
        Value::String(s) => Some(s.as_str().into()),
        Value::Number(n) => n.as_i64().map(Into::into).or_else(|| n.as_f64().map(Into::into)),
        _ => None,
    }
}

/// A value as written, without the spacing and comment around it.
fn plain(v: &toml_edit::Value) -> String {
    let mut v = v.clone();
    v.decor_mut().clear();
    v.to_string()
}

/// Sets a key, replacing its value in place when it is there: `insert` would
/// replace the key too, and with it the comment written above it.
fn toml_put(table: &mut toml_edit::Table, key: &str, item: Item) {
    match table.get_mut(key) {
        Some(slot) => *slot = item,
        None => {
            table.insert(key, item);
        }
    }
}

fn toml_set(doc: &mut DocumentMut, path: &[&str], value: toml_edit::Value) {
    let (last, parents) = path.split_last().expect("non-empty path");
    let mut table = doc.as_table_mut();
    for k in parents {
        if !table.get(k).is_some_and(|i| i.is_table_like()) {
            let mut t = toml_edit::Table::new();
            t.set_implicit(false);
            table.insert(k, Item::Table(t));
        }
        table = match table.get_mut(k).and_then(Item::as_table_mut) {
            Some(t) => t,
            None => return, // an inline table: left as the user wrote it
        };
    }
    toml_put(table, last, toml_edit::value(value));
}

// -------------------------------------------------------- recommendations --

fn recommend(provider: Provider, account_path: &str) -> Result<Vec<Rec>, String> {
    let mut out = vec![];
    match provider {
        Provider::Claude => {
            let doc = read_claude(&claude_file(account_path)?)?;
            for s in specs(provider) {
                let current = json_at(&doc, s.path);
                out.push(Rec {
                    key: s.path.join("."),
                    current: current.map(Value::to_string),
                    recommended: s.value.to_string(),
                    why: s.why.into(),
                    default_on: s.default_on,
                    applied: current == Some(&s.value),
                });
            }
        }
        Provider::Codex => {
            let doc = crate::codex::config::read(&crate::codex::config::account_file(account_path)?)?;
            for s in specs(provider) {
                let current = toml_at(&doc, s.path).and_then(Item::as_value).map(plain);
                let recommended = toml_value(&s.value).as_ref().map(plain).unwrap_or_default();
                out.push(Rec {
                    key: s.path.join("."),
                    applied: current.as_deref() == Some(recommended.as_str()),
                    current,
                    recommended,
                    why: s.why.into(),
                    default_on: s.default_on,
                });
            }
        }
    }
    Ok(out)
}

fn apply(provider: Provider, account_path: &str, keys: &[String]) -> Result<(), String> {
    let picked: Vec<Spec> = specs(provider).into_iter().filter(|s| keys.contains(&s.path.join("."))).collect();
    if picked.is_empty() {
        return Ok(());
    }
    match provider {
        Provider::Claude => {
            let path = claude_file(account_path)?;
            let mut doc = read_claude(&path)?;
            for s in picked {
                json_set(&mut doc, s.path, s.value);
            }
            write_claude(&path, &doc)
        }
        Provider::Codex => {
            let path = crate::codex::config::account_file(account_path)?;
            let mut doc = crate::codex::config::read(&path)?;
            for s in picked {
                if let Some(v) = toml_value(&s.value) {
                    toml_set(&mut doc, s.path, v);
                }
            }
            crate::codex::config::write(&path, &doc)
        }
    }
}

#[tauri::command]
pub fn recommended_settings(provider: Provider, account_path: String) -> Result<Vec<Rec>, String> {
    recommend(provider, &account_path)
}

#[tauri::command]
pub fn apply_recommended(provider: Provider, account_path: String, keys: Vec<String>) -> Result<(), String> {
    apply(provider, &account_path, &keys)?;
    crate::log::info("settings", &format!("applied {} recommended setting(s) to {account_path}", keys.len()));
    Ok(())
}

// ----------------------------------------------------------------- editor --

/// The account's settings file as text, for editing it by hand.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigFile {
    pub path: String,
    pub text: String,
    /// False when the account has no such file yet; `text` is a starting point.
    pub exists: bool,
}

fn config_file(provider: Provider, account_path: &str) -> Result<PathBuf, String> {
    match provider {
        Provider::Claude => claude_file(account_path),
        Provider::Codex => crate::codex::config::account_file(account_path),
    }
}

/// Whether `text` is a settings file this provider can read: a JSON object
/// for Claude Code, TOML for Codex. The message says where it breaks.
fn check_config(provider: Provider, text: &str) -> Result<(), String> {
    match provider {
        Provider::Claude => match serde_json::from_str::<Value>(text) {
            Ok(v) if v.is_object() => Ok(()),
            Ok(_) => Err("settings.json has to be one JSON object: { … }".into()),
            Err(e) => Err(format!("Not valid JSON: {e}")),
        },
        Provider::Codex => text.parse::<DocumentMut>().map(|_| ()).map_err(|e| format!("Not valid TOML: {e}")),
    }
}

#[tauri::command]
pub fn read_account_config(provider: Provider, account_path: String) -> Result<ConfigFile, String> {
    let path = config_file(provider, &account_path)?;
    let (text, exists) = match std::fs::read_to_string(&path) {
        Ok(t) => (t, true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            (if provider == Provider::Claude { "{
}
".to_string() } else { String::new() }, false)
        }
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    Ok(ConfigFile { path: path.to_string_lossy().into_owned(), text, exists })
}

/// Writes the text as typed — comments, order and spacing are the user's —
/// once it parses; a file that would not is never written.
#[tauri::command]
pub fn write_account_config(provider: Provider, account_path: String, text: String) -> Result<(), String> {
    check_config(provider, &text)?;
    let path = config_file(provider, &account_path)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("edit.tmp");
    std::fs::write(&tmp, &text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })?;
    crate::log::info("settings", &format!("{} edited by hand", path.display()));
    Ok(())
}

// ------------------------------------------------------------------- sync --

/// Objects merge key by key, the parent winning; anything else is replaced.
/// A key only the child has stays.
fn merge_json(child: &mut Value, parent: &Value) {
    match (child, parent) {
        (Value::Object(c), Value::Object(p)) => {
            for (k, pv) in p {
                match c.get_mut(k) {
                    Some(cv) if cv.is_object() && pv.is_object() => merge_json(cv, pv),
                    Some(cv) => *cv = pv.clone(),
                    None => {
                        c.insert(k.clone(), pv.clone());
                    }
                }
            }
        }
        (c, p) => *c = p.clone(),
    }
}

/// Top-level keys of a Codex config that belong to the account, not to taste:
/// the folders it trusts.
const CODEX_KEEP: &[&str] = &["projects"];

fn merge_toml(child: &mut toml_edit::Table, parent: &toml_edit::Table, top: bool) {
    for (k, pv) in parent.iter() {
        if top && CODEX_KEEP.contains(&k) {
            continue;
        }
        if let (Some(Item::Table(ct)), Item::Table(pt)) = (child.get_mut(k), pv) {
            merge_toml(ct, pt, false);
            continue;
        }
        toml_put(child, k, pv.clone());
    }
}

fn sync_one(provider: Provider, parent: &str, child: &str) -> Result<(), String> {
    match provider {
        Provider::Claude => {
            let from = read_claude(&claude_file(parent)?)?;
            let path = claude_file(child)?;
            let mut doc = read_claude(&path)?;
            let before = doc.clone();
            merge_json(&mut doc, &from);
            if doc != before { write_claude(&path, &doc) } else { Ok(()) }
        }
        Provider::Codex => {
            let from = crate::codex::config::read(&crate::codex::config::account_file(parent)?)?;
            let path = crate::codex::config::account_file(child)?;
            let mut doc = crate::codex::config::read(&path)?;
            let before = doc.to_string();
            merge_toml(doc.as_table_mut(), from.as_table(), true);
            if doc.to_string() != before { crate::codex::config::write(&path, &doc) } else { Ok(()) }
        }
    }
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SyncReport {
    pub synced: Vec<String>,
    /// `name: why` for every account that could not be written.
    pub failed: Vec<String>,
}

fn sync(provider: Provider) -> Result<SyncReport, String> {
    let s = crate::settings::get();
    let parent_name = match provider {
        Provider::Claude => s.settings_parents.claude,
        Provider::Codex => s.settings_parents.codex,
    }
    .ok_or("No parent account chosen")?;
    let accounts: Vec<_> = crate::accounts::list_accounts()?.into_iter().filter(|a| a.provider == provider).collect();
    let parent = accounts
        .iter()
        .find(|a| a.name == parent_name)
        .ok_or_else(|| format!("The parent account \"{parent_name}\" is gone"))?;
    let mut report = SyncReport::default();
    for a in accounts.iter().filter(|a| a.name != parent.name) {
        match sync_one(provider, &parent.path, &a.path) {
            Ok(()) => report.synced.push(a.name.clone()),
            Err(e) => report.failed.push(format!("{}: {e}", a.name)),
        }
    }
    let msg = format!(
        "{} settings from \"{}\" copied to {} account(s){}",
        provider.as_str(),
        parent.name,
        report.synced.len(),
        if report.failed.is_empty() { String::new() } else { format!("; failed: {}", report.failed.join(", ")) }
    );
    if report.failed.is_empty() { crate::log::info("settings", &msg) } else { crate::log::warn("settings", &msg) }
    Ok(report)
}

#[tauri::command]
pub fn sync_settings_now(provider: Provider) -> Result<SyncReport, String> {
    sync(provider)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncState {
    pub claude: Option<String>,
    pub codex: Option<String>,
    pub on_start: bool,
}

#[tauri::command]
pub fn settings_sync_state() -> SyncState {
    let s = crate::settings::get();
    SyncState { claude: s.settings_parents.claude, codex: s.settings_parents.codex, on_start: s.settings_sync_on_start }
}

#[tauri::command]
pub fn set_settings_parent(provider: Provider, parent: Option<String>) -> Result<SyncState, String> {
    crate::settings::update(|s| match provider {
        Provider::Claude => s.settings_parents.claude = parent,
        Provider::Codex => s.settings_parents.codex = parent,
    })?;
    Ok(settings_sync_state())
}

#[tauri::command]
pub fn set_settings_sync_on_start(on: bool) -> Result<SyncState, String> {
    crate::settings::update(|s| s.settings_sync_on_start = on)?;
    Ok(settings_sync_state())
}

/// Once per launch, off the startup path: a parent's settings onto the rest,
/// for each provider that has a parent.
pub fn sync_on_start() {
    if !crate::settings::get().settings_sync_on_start {
        return;
    }
    std::thread::spawn(|| {
        let s = crate::settings::get();
        for (provider, parent) in [(Provider::Claude, &s.settings_parents.claude), (Provider::Codex, &s.settings_parents.codex)] {
            if parent.is_some() {
                if let Err(e) = sync(provider) {
                    crate::log::warn("settings", &format!("{} settings sync skipped: {e}", provider.as_str()));
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("luna-acct-settings-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn claude_apply_keeps_other_keys_and_their_order() {
        let d = dir("claude");
        std::fs::write(d.join("settings.json"), r#"{"model":"opus","env":{"FOO":"1"},"zeta":true}"#).unwrap();
        let p = d.to_string_lossy().into_owned();
        apply(Provider::Claude, &p, &["tui".into(), "env.CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY".into()]).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(d.join("settings.json")).unwrap()).unwrap();
        assert_eq!(v["model"], "opus");
        assert_eq!(v["env"]["FOO"], "1");
        assert_eq!(v["env"]["CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY"], "1");
        assert_eq!(v["tui"], "fullscreen");
        let keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
        assert_eq!(&keys[..3], &["model", "env", "zeta"]);
        let recs = recommend(Provider::Claude, &p).unwrap();
        assert!(recs.iter().find(|r| r.key == "tui").unwrap().applied);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_powershell_default_is_only_offered_on_windows() {
        let offered = specs(Provider::Claude).iter().any(|s| s.path == ["defaultShell"]);
        assert_eq!(offered, cfg!(windows));
    }

    #[test]
    fn codex_apply_keeps_comments_and_writes_tables() {
        let d = dir("codex");
        std::fs::write(d.join("config.toml"), "# mine\nmodel = \"gpt-6\" # keep\n# searching\nweb_search = \"off\"\n\n[tui]\ntheme = \"dark\"\n").unwrap();
        let p = d.to_string_lossy().into_owned();
        apply(Provider::Codex, &p, &["tui.show_tooltips".into(), "analytics.enabled".into(), "web_search".into()]).unwrap();
        let text = std::fs::read_to_string(d.join("config.toml")).unwrap();
        assert!(text.contains("# mine"));
        assert!(text.contains("model = \"gpt-6\" # keep"));
        assert!(text.contains("theme = \"dark\""));
        assert!(text.contains("# searching\nweb_search = \"live\""), "{text}");
        let doc: DocumentMut = text.parse().unwrap();
        assert_eq!(doc["tui"]["show_tooltips"].as_bool(), Some(false));
        assert_eq!(doc["analytics"]["enabled"].as_bool(), Some(false));
        assert_eq!(doc["web_search"].as_str(), Some("live"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn syncing_an_account_copies_settings_and_nothing_private() {
        let parent = dir("sync-parent");
        let child = dir("sync-child");
        // Claude: settings travel, the child's login and state do not.
        std::fs::write(parent.join("settings.json"), r#"{"tui":"fullscreen","env":{"A":"1"}}"#).unwrap();
        std::fs::write(parent.join(".credentials.json"), "parent-secret").unwrap();
        std::fs::write(child.join("settings.json"), r#"{"model":"opus","env":{"B":"2"}}"#).unwrap();
        std::fs::write(child.join(".credentials.json"), "child-secret").unwrap();
        let (p, c) = (parent.to_string_lossy().into_owned(), child.to_string_lossy().into_owned());
        sync_one(Provider::Claude, &p, &c).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(child.join("settings.json")).unwrap()).unwrap();
        assert_eq!(v, json!({ "model": "opus", "env": { "B": "2", "A": "1" }, "tui": "fullscreen" }));
        assert_eq!(std::fs::read_to_string(child.join(".credentials.json")).unwrap(), "child-secret");

        // Codex: config travels with its comments, trusted folders and auth stay.
        std::fs::write(parent.join("config.toml"), "# parent\nmodel_reasoning_effort = \"high\"\n[tui]\nshow_tooltips = false\n[projects.'C:\\\\p']\ntrust_level = \"trusted\"\n").unwrap();
        std::fs::write(child.join("config.toml"), "# mine\nmodel_reasoning_effort = \"low\"\n[projects.'C:\\\\c']\ntrust_level = \"trusted\"\n").unwrap();
        std::fs::write(child.join("auth.json"), "child-auth").unwrap();
        sync_one(Provider::Codex, &p, &c).unwrap();
        let text = std::fs::read_to_string(child.join("config.toml")).unwrap();
        assert!(text.contains("# mine"));
        assert!(text.contains("C:\\\\c") && !text.contains("C:\\\\p"));
        let doc: DocumentMut = text.parse().unwrap();
        assert_eq!(doc["model_reasoning_effort"].as_str(), Some("high"));
        assert_eq!(doc["tui"]["show_tooltips"].as_bool(), Some(false));
        assert_eq!(std::fs::read_to_string(child.join("auth.json")).unwrap(), "child-auth");

        // A second sync with nothing new writes nothing.
        let before = std::fs::metadata(child.join("config.toml")).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        sync_one(Provider::Codex, &p, &c).unwrap();
        assert_eq!(std::fs::metadata(child.join("config.toml")).unwrap().modified().unwrap(), before);
        let _ = std::fs::remove_dir_all(&parent);
        let _ = std::fs::remove_dir_all(&child);
    }

    #[test]
    fn the_editor_writes_only_what_parses_and_keeps_it_as_typed() {
        let d = dir("editor");
        let p = d.to_string_lossy().into_owned();
        assert!(!read_account_config(Provider::Claude, p.clone()).unwrap().exists);
        assert!(write_account_config(Provider::Claude, p.clone(), "{ \"tui\": ".into()).is_err());
        assert!(write_account_config(Provider::Claude, p.clone(), "[1]".into()).is_err());
        let typed = "{
  \"tui\":   \"fullscreen\"
}
";
        write_account_config(Provider::Claude, p.clone(), typed.into()).unwrap();
        assert_eq!(read_account_config(Provider::Claude, p.clone()).unwrap().text, typed);
        assert!(write_account_config(Provider::Codex, p.clone(), "model = [unterminated".into()).is_err());
        let toml = "# mine
model = \"gpt-6\"   # keep
";
        write_account_config(Provider::Codex, p.clone(), toml.into()).unwrap();
        assert_eq!(std::fs::read_to_string(d.join("config.toml")).unwrap(), toml);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_broken_settings_file_is_left_alone() {
        let d = dir("broken");
        std::fs::write(d.join("settings.json"), "{ not json").unwrap();
        let p = d.to_string_lossy().into_owned();
        assert!(apply(Provider::Claude, &p, &["tui".into()]).is_err());
        assert_eq!(std::fs::read_to_string(d.join("settings.json")).unwrap(), "{ not json");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn json_sync_merges_and_keeps_the_childs_own_keys() {
        let mut child = json!({ "env": { "MINE": "1", "SHARED": "old" }, "only": true, "tui": "default" });
        merge_json(&mut child, &json!({ "env": { "SHARED": "new" }, "tui": "fullscreen", "model": "opus" }));
        assert_eq!(child, json!({ "env": { "MINE": "1", "SHARED": "new" }, "only": true, "tui": "fullscreen", "model": "opus" }));
    }

    #[test]
    fn toml_sync_never_carries_trusted_projects() {
        let mut child: DocumentMut = "[projects.'C:\\\\a']\ntrust_level = \"trusted\"\n[tui]\ntheme = \"x\"\n".parse().unwrap();
        let parent: DocumentMut = "model = \"m\"\n[projects.'C:\\\\b']\ntrust_level = \"trusted\"\n[tui]\nshow_tooltips = false\n".parse().unwrap();
        merge_toml(child.as_table_mut(), parent.as_table(), true);
        let text = child.to_string();
        assert!(text.contains("C:\\\\a"));
        assert!(!text.contains("C:\\\\b"));
        assert_eq!(child["model"].as_str(), Some("m"));
        assert_eq!(child["tui"]["theme"].as_str(), Some("x"));
        assert_eq!(child["tui"]["show_tooltips"].as_bool(), Some(false));
    }
}
