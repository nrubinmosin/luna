//! An account's own settings file — Claude Code's `settings.json`, Codex's
//! `config.toml` — as Luna can help with it: settings recommended for anyone
//! running the CLI inside Luna, added or taken out key by key, the file
//! itself checked and formatted for editing by hand, and a "parent" account
//! per provider whose file the others get an exact copy of.
//!
//! Recommendations keep what they do not touch: other keys, their order, and
//! in the TOML file the user's comments. Credentials, logins and Codex's
//! trusted projects never travel.

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
    /// Part of the set "Tick recommended" picks. Off for what is a matter of
    /// taste.
    pub default_on: bool,
    /// The file already says exactly this.
    pub applied: bool,
    /// The file it is kept in, when that is not the account's settings file.
    pub file: Option<String>,
}

/// A recommendation in the making: the path, the value, and who it is for.
struct Spec {
    path: &'static [&'static str],
    value: Value,
    why: &'static str,
    default_on: bool,
    /// Windows only, e.g. a PowerShell default.
    windows_only: bool,
    /// In Claude Code's `.claude.json` rather than its `settings.json`: a few
    /// of its settings are read from there only.
    global: bool,
}

fn spec(path: &'static [&'static str], value: Value, why: &'static str, default_on: bool) -> Spec {
    Spec { path, value, why, default_on, windows_only: false, global: false }
}

const COPY_ON_SELECT_WHY: &str =
    "Selecting in a pane copies nothing until Ctrl+C, so the clipboard keeps what you put there.";

/// Claude Code 2.1, every key checked against the CLI's own settings schema.
fn claude_specs() -> Vec<Spec> {
    vec![
        spec(&["tui"], json!("fullscreen"),
            "Draws on the alternate screen with its own scrollback: no flicker, and a pane resize redraws cleanly.", true),
        spec(&["spinnerTipsEnabled"], json!(false), "No tips under the spinner.", true),
        spec(&["feedbackDrafts"], json!("off"), "No feedback drafts queued by the model.", true),
        spec(&["env", "CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY"], json!("1"), "No feedback survey mid-session.", true),
        spec(&["idleCompaction"], json!(false),
            "No compacting a long session while it sits idle: its context stays whole until it fills the window.", true),
        spec(&["skipDangerousModePermissionPrompt"], json!(true),
            "Bypass is a choice made in Luna's new-chat dialog; the CLI need not ask again.", true),
        Spec { windows_only: true, ..spec(&["defaultShell"], json!("powershell"),
            "`!` commands in the input box run in PowerShell instead of bash.", true) },
        Spec { global: true, ..spec(&["copyOnSelect"], json!(false), COPY_ON_SELECT_WHY, true) },
        spec(&["cleanupPeriodDays"], json!(14), "Transcripts older than two weeks are cleaned up (default 30).", false),
        spec(&["attribution"], json!({ "commit": "", "pr": "", "sessionUrl": false }),
            "No Co-Authored-By or session links in commits and PRs.", false),
        spec(&["disableRemoteControl"], json!(true), "No Remote Control for sessions on this account.", false),
        spec(&["permissions", "defaultMode"], json!("bypassPermissions"), "New sessions start in Bypass.", false),
    ]
}

/// Codex 0.165, the same comforts in Codex's words; every key checked against
/// the CLI's config structs.
fn codex_specs() -> Vec<Spec> {
    vec![
        spec(&["check_for_update_on_startup"], json!(false), "Luna keeps Codex up to date itself.", true),
        spec(&["tui", "show_tooltips"], json!(false), "No tips in the composer.", true),
        spec(&["tui", "notifications"], json!(false),
            "No desktop notifications from Codex: Luna says when a chat is waiting for you.", true),
        spec(&["tui", "copy_on_select"], json!("never"), COPY_ON_SELECT_WHY, true),
        spec(&["analytics", "enabled"], json!(false), "No usage analytics.", true),
        spec(&["feedback", "enabled"], json!(false), "No feedback prompts.", true),
        spec(&["web_search"], json!("live"), "Web search, as Claude Code has it.", true),
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

/// Claude Code's global config, which `CLAUDE_CONFIG_DIR` puts in the account
/// folder. The CLI saves it by re-reading the file under a lock, so a key set
/// here while sessions run is kept, not written over.
const CLAUDE_GLOBAL: &str = ".claude.json";

fn claude_spec_file(account_path: &str, global: bool) -> Result<PathBuf, String> {
    let file = claude_file(account_path)?;
    Ok(if global { file.with_file_name(CLAUDE_GLOBAL) } else { file })
}

/// The file as an object, or an empty one when it is missing. A file that
/// does not parse is an error: writing over it would lose the user's settings.
/// The CLI rewrites `.claude.json` in place, so a read can catch it empty or
/// half-written, and on Windows held open: such a read is tried again before
/// it counts, and only a file still blank after that is taken as empty.
fn read_claude(path: &Path) -> Result<Value, String> {
    let mut attempt = 0;
    loop {
        let tried = match std::fs::read_to_string(path) {
            Ok(text) if text.trim().is_empty() => Ok(json!({})),
            Ok(text) => match serde_json::from_str::<Value>(&text) {
                Ok(v) if v.is_object() => return Ok(v),
                Ok(_) => return Err(format!("{}: not a JSON object", path.display())),
                Err(e) => Err(format!("{}: {e}", path.display())),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(json!({})),
            Err(e) => Err(format!("{}: {e}", path.display())),
        };
        attempt += 1;
        if attempt == 3 {
            return tried;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

fn write_claude(path: &Path, v: &Value) -> Result<(), String> {
    write_text(path, &(serde_json::to_string_pretty(v).map_err(|e| e.to_string())? + "\n"))
}

/// Writes via a sibling temp file so a crash mid-write cannot truncate the file.
fn write_text(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    // A temp file of this write's own: two writers sharing one could rename
    // the other's half-written bytes into place (the start-up sync runs on a
    // thread of its own beside the window's commands).
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}-{}.tmp", std::process::id(), SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, text).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })?;
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

/// Takes the key out, and with it any parent object it leaves empty.
fn json_remove(v: &mut Value, path: &[&str]) {
    let Some((first, rest)) = path.split_first() else { return };
    let Some(obj) = v.as_object_mut() else { return };
    if rest.is_empty() {
        obj.shift_remove(*first);
        return;
    }
    if let Some(child) = obj.get_mut(*first) {
        json_remove(child, rest);
        if child.as_object().is_some_and(|o| o.is_empty()) {
            obj.shift_remove(*first);
        }
    }
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

/// Takes the key out, and with it any table it leaves empty — which in a
/// TOML file is a `[header]` with nothing under it.
fn toml_remove(table: &mut toml_edit::Table, path: &[&str]) {
    let Some((first, rest)) = path.split_first() else { return };
    if rest.is_empty() {
        table.remove(first);
        return;
    }
    let emptied = match table.get_mut(first).and_then(Item::as_table_mut) {
        Some(child) => {
            toml_remove(child, rest);
            child.is_empty()
        }
        None => false, // an inline table: left as the user wrote it
    };
    if emptied {
        table.remove(first);
    }
}

// -------------------------------------------------------- recommendations --

fn recommend(provider: Provider, account_path: &str) -> Result<Vec<Rec>, String> {
    let mut out = vec![];
    match provider {
        Provider::Claude => {
            let docs = [
                read_claude(&claude_spec_file(account_path, false)?)?,
                read_claude(&claude_spec_file(account_path, true)?)?,
            ];
            for s in specs(provider) {
                let current = json_at(&docs[s.global as usize], s.path);
                out.push(Rec {
                    key: s.path.join("."),
                    current: current.map(Value::to_string),
                    recommended: s.value.to_string(),
                    why: s.why.into(),
                    default_on: s.default_on,
                    applied: current == Some(&s.value),
                    file: s.global.then(|| CLAUDE_GLOBAL.into()),
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
                    file: None,
                });
            }
        }
    }
    Ok(out)
}

/// Sets the `add` keys to their recommended values and takes the `remove`
/// ones out — only where the file still says exactly what was recommended:
/// a value the user wrote themselves is theirs, not a recommendation to undo.
fn apply(provider: Provider, account_path: &str, add: &[String], remove: &[String]) -> Result<(), String> {
    let named = |keys: &[String]| -> Vec<Spec> {
        specs(provider).into_iter().filter(|s| keys.contains(&s.path.join("."))).collect()
    };
    let (add, remove) = (named(add), named(remove));
    if add.is_empty() && remove.is_empty() {
        return Ok(());
    }
    match provider {
        Provider::Claude => {
            // `.claude.json` is the CLI's own: touched only for a key of its.
            for global in [false, true] {
                let add: Vec<&Spec> = add.iter().filter(|s| s.global == global).collect();
                let remove: Vec<&Spec> = remove.iter().filter(|s| s.global == global).collect();
                if add.is_empty() && remove.is_empty() {
                    continue;
                }
                let path = claude_spec_file(account_path, global)?;
                let mut doc = read_claude(&path)?;
                for s in add {
                    json_set(&mut doc, s.path, s.value.clone());
                }
                for s in remove {
                    if json_at(&doc, s.path) == Some(&s.value) {
                        json_remove(&mut doc, s.path);
                    }
                }
                write_claude(&path, &doc)?;
            }
            Ok(())
        }
        Provider::Codex => {
            let path = crate::codex::config::account_file(account_path)?;
            let mut doc = crate::codex::config::read(&path)?;
            for s in add {
                if let Some(v) = toml_value(&s.value) {
                    toml_set(&mut doc, s.path, v);
                }
            }
            for s in remove {
                let said = toml_at(&doc, s.path).and_then(Item::as_value).map(plain);
                if said.is_some() && said == toml_value(&s.value).as_ref().map(plain) {
                    toml_remove(doc.as_table_mut(), s.path);
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
pub fn apply_recommended(provider: Provider, account_path: String, add: Vec<String>, remove: Vec<String>) -> Result<(), String> {
    apply(provider, &account_path, &add, &remove)?;
    crate::log::info(
        "settings",
        &format!("recommended settings on {account_path}: {} added, {} taken out", add.len(), remove.len()),
    );
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

/// Where `text` stops being a file this provider can read, for the editor to
/// mark as it is typed; None when it reads.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigProblem {
    pub message: String,
    /// 1-based, when the parser says where.
    pub line: Option<usize>,
    pub column: Option<usize>,
}

#[tauri::command]
pub fn check_account_config(provider: Provider, text: String) -> Option<ConfigProblem> {
    match provider {
        Provider::Claude => match serde_json::from_str::<Value>(&text) {
            Ok(v) if v.is_object() => None,
            Ok(_) => Some(ConfigProblem { message: "settings.json has to be one JSON object: { … }".into(), line: None, column: None }),
            Err(e) => Some(ConfigProblem { message: e.to_string(), line: Some(e.line()), column: Some(e.column()) }),
        },
        Provider::Codex => text.parse::<DocumentMut>().err().map(|e| {
            let at = e.span().map(|s| line_col(&text, s.start));
            ConfigProblem {
                // The parser's own message repeats the whole line with a caret
                // under it; the first line is the reason.
                message: e.message().lines().next().unwrap_or("not valid TOML").to_string(),
                line: at.map(|(l, _)| l),
                column: at.map(|(_, c)| c),
            }
        }),
    }
}

/// 1-based line and column of a byte offset.
fn line_col(text: &str, offset: usize) -> (usize, usize) {
    let end = (0..=offset.min(text.len())).rev().find(|&i| text.is_char_boundary(i)).unwrap_or(0);
    let before = &text[..end];
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, column)
}

/// The text laid out the way each CLI writes its own file: JSON indented by
/// two, TOML by taplo's rules — comments and key order stay where they are.
#[tauri::command]
pub fn format_account_config(provider: Provider, text: String) -> Result<String, String> {
    check_config(provider, &text)?;
    match provider {
        Provider::Claude => {
            let v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            Ok(serde_json::to_string_pretty(&v).map_err(|e| e.to_string())? + "\n")
        }
        Provider::Codex => {
            Ok(taplo::formatter::format(&text, Default::default()))
        }
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
    write_text(&path, &text)?;
    crate::log::info("settings", &format!("{} edited by hand", path.display()));
    Ok(())
}

// ------------------------------------------------------------------- sync --

/// The parent's file as text, checked to parse: a broken file is not spread
/// onto every account. A parent without one has nothing to copy.
fn parent_text(provider: Provider, parent: &str) -> Result<String, String> {
    let path = config_file(provider, parent)?;
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(format!("{} does not exist", path.display())),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    check_config(provider, &text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(text)
}

/// Top-level tables of a Codex config that hold the account's own state, not
/// taste: the folders it trusts, which Codex and Luna write per account.
const CODEX_KEEP: &[&str] = &["projects"];

/// The parent's config with its own `CODEX_KEEP` tables taken out and the
/// child's put at the end. Rendered apart and joined as text: tables moved
/// between documents keep their old positions and would land among the
/// parent's.
fn codex_copy(parent: &str, child: &DocumentMut) -> Result<String, String> {
    let mut doc: DocumentMut = parent.parse().map_err(|e| format!("Not valid TOML: {e}"))?;
    let mut kept = DocumentMut::new();
    for k in CODEX_KEEP {
        doc.remove(k);
        if let Some(item) = child.get(k) {
            kept.insert(k, item.clone());
        }
    }
    let (text, kept) = (doc.to_string(), kept.to_string());
    Ok(match (text.trim_end(), kept.trim()) {
        (_, "") => text,
        ("", _) => kept,
        (text, _) => format!("{text}\n\n{}", kept.trim_start()),
    })
}

/// Makes the child's file the parent's. Claude Code's `settings.json` holds no
/// login or state and is copied as is; Codex's `config.toml` keeps the child's
/// trusted folders. A file already the same is not written.
fn sync_one(provider: Provider, parent: &str, child: &str) -> Result<(), String> {
    let from = parent_text(provider, parent)?;
    let path = config_file(provider, child)?;
    let text = match provider {
        Provider::Claude => from,
        Provider::Codex => codex_copy(&from, &crate::codex::config::read(&path)?)?,
    };
    if std::fs::read_to_string(&path).ok().as_deref() == Some(text.as_str()) {
        return Ok(());
    }
    write_text(&path, &text)
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
        apply(Provider::Claude, &p, &["tui".into(), "env.CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY".into()], &[]).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(d.join("settings.json")).unwrap()).unwrap();
        assert_eq!(v["model"], "opus");
        assert_eq!(v["env"]["FOO"], "1");
        assert_eq!(v["env"]["CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY"], "1");
        assert_eq!(v["tui"], "fullscreen");
        let keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
        assert_eq!(&keys[..3], &["model", "env", "zeta"]);
        let recs = recommend(Provider::Claude, &p).unwrap();
        assert!(recs.iter().find(|r| r.key == "tui").unwrap().applied);
        assert!(!d.join(".claude.json").exists(), "no key of .claude.json was asked for");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn copy_on_select_goes_in_claude_json_and_leaves_the_rest_of_it() {
        let d = dir("global");
        std::fs::write(d.join(".claude.json"), r#"{"numStartups":3,"oauthAccount":{"emailAddress":"a@b"}}"#).unwrap();
        let p = d.to_string_lossy().into_owned();
        apply(Provider::Claude, &p, &["copyOnSelect".into(), "tui".into()], &[]).unwrap();
        let global: Value = serde_json::from_str(&std::fs::read_to_string(d.join(".claude.json")).unwrap()).unwrap();
        assert_eq!(global, json!({ "numStartups": 3, "oauthAccount": { "emailAddress": "a@b" }, "copyOnSelect": false }));
        let settings: Value = serde_json::from_str(&std::fs::read_to_string(d.join("settings.json")).unwrap()).unwrap();
        assert_eq!(settings, json!({ "tui": "fullscreen" }));
        let rec = recommend(Provider::Claude, &p).unwrap().into_iter().find(|r| r.key == "copyOnSelect").unwrap();
        assert!(rec.applied);
        assert_eq!(rec.file.as_deref(), Some(".claude.json"));

        apply(Provider::Claude, &p, &[], &["copyOnSelect".into()]).unwrap();
        let global: Value = serde_json::from_str(&std::fs::read_to_string(d.join(".claude.json")).unwrap()).unwrap();
        assert!(global.get("copyOnSelect").is_none() && global["numStartups"] == 3);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn taking_a_recommendation_out_leaves_no_empty_parents_and_spares_the_users_own() {
        let d = dir("remove");
        std::fs::write(d.join("settings.json"), r#"{"model":"opus","env":{"CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY":"1"},"tui":"fullscreen","cleanupPeriodDays":60}"#).unwrap();
        let p = d.to_string_lossy().into_owned();
        let out = ["env.CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY".to_string(), "tui".into(), "cleanupPeriodDays".into()];
        apply(Provider::Claude, &p, &[], &out).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(d.join("settings.json")).unwrap()).unwrap();
        assert_eq!(v, json!({ "model": "opus", "cleanupPeriodDays": 60 }), "60 is the user's, not the recommendation");

        std::fs::write(d.join("config.toml"), "# mine
web_search = \"live\"

[tui]
show_tooltips = false

[feedback]
enabled = false
other = 1
").unwrap();
        let out = ["web_search".to_string(), "tui.show_tooltips".into(), "feedback.enabled".into()];
        apply(Provider::Codex, &p, &[], &out).unwrap();
        let text = std::fs::read_to_string(d.join("config.toml")).unwrap();
        let doc: DocumentMut = text.parse().unwrap();
        assert!(doc.get("web_search").is_none() && doc.get("tui").is_none(), "{text}");
        assert_eq!(doc["feedback"]["other"].as_integer(), Some(1));
        assert!(doc["feedback"].get("enabled").is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_editor_checks_and_lays_out_both_files() {
        let bad = check_account_config(Provider::Codex, "a = 1
b = [
".into()).unwrap();
        assert!(bad.line.is_some(), "{}", bad.message);
        assert!(check_account_config(Provider::Codex, "a = 1
".into()).is_none());
        let bad = check_account_config(Provider::Claude, "{
  \"a\": ,
}".into()).unwrap();
        assert_eq!(bad.line, Some(2));
        let laid = format_account_config(Provider::Claude, r#"{"b":1,"a":{"c":true}}"#.into()).unwrap();
        assert_eq!(laid, "{
  \"b\": 1,
  \"a\": {
    \"c\": true
  }
}
");
        let laid = format_account_config(Provider::Codex, "# keep
z   =  1
a=\"x\"
".into()).unwrap();
        assert!(laid.contains("# keep") && laid.find("z = 1") < laid.find("a = \"x\""), "{laid}");
        assert!(format_account_config(Provider::Codex, "a = [".into()).is_err());
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
        apply(Provider::Codex, &p, &["tui.show_tooltips".into(), "analytics.enabled".into(), "web_search".into()], &[]).unwrap();
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
    fn syncing_an_account_makes_its_file_the_parents_and_copies_nothing_private() {
        let parent = dir("sync-parent");
        let child = dir("sync-child");
        // Claude: the file arrives byte for byte, the child's own keys go, its
        // login stays.
        let settings = "{\n  \"tui\": \"fullscreen\",\n  \"env\": { \"A\": \"1\" }\n}\n";
        std::fs::write(parent.join("settings.json"), settings).unwrap();
        std::fs::write(parent.join(".credentials.json"), "parent-secret").unwrap();
        std::fs::write(child.join("settings.json"), r#"{"model":"opus","env":{"B":"2"}}"#).unwrap();
        std::fs::write(child.join(".credentials.json"), "child-secret").unwrap();
        let (p, c) = (parent.to_string_lossy().into_owned(), child.to_string_lossy().into_owned());
        sync_one(Provider::Claude, &p, &c).unwrap();
        assert_eq!(std::fs::read_to_string(child.join("settings.json")).unwrap(), settings);
        assert_eq!(std::fs::read_to_string(child.join(".credentials.json")).unwrap(), "child-secret");

        // A key taken out of the parent goes from the child at the next sync.
        std::fs::write(parent.join("settings.json"), "{}\n").unwrap();
        sync_one(Provider::Claude, &p, &c).unwrap();
        assert_eq!(std::fs::read_to_string(child.join("settings.json")).unwrap(), "{}\n");

        // Codex: the parent's config with its comments, the child's trusted
        // folders in place of the parent's, auth untouched.
        std::fs::write(parent.join("config.toml"), "# parent\nmodel_reasoning_effort = \"high\"\n[projects.'C:\\\\p']\ntrust_level = \"trusted\"\n[tui]\nshow_tooltips = false\n").unwrap();
        std::fs::write(child.join("config.toml"), "# mine\nmodel_reasoning_effort = \"low\"\nmine = 1\n[projects.'C:\\\\c']\ntrust_level = \"trusted\"\n").unwrap();
        std::fs::write(child.join("auth.json"), "child-auth").unwrap();
        sync_one(Provider::Codex, &p, &c).unwrap();
        let text = std::fs::read_to_string(child.join("config.toml")).unwrap();
        assert_eq!(text, "# parent\nmodel_reasoning_effort = \"high\"\n[tui]\nshow_tooltips = false\n\n[projects.'C:\\\\c']\ntrust_level = \"trusted\"\n");
        assert_eq!(std::fs::read_to_string(child.join("auth.json")).unwrap(), "child-auth");

        // A second sync with nothing new writes nothing.
        let before = std::fs::metadata(child.join("config.toml")).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        sync_one(Provider::Codex, &p, &c).unwrap();
        assert_eq!(std::fs::metadata(child.join("config.toml")).unwrap().modified().unwrap(), before);

        // A child with no trusted folders gets the parent's file minus its own.
        std::fs::write(child.join("config.toml"), "").unwrap();
        sync_one(Provider::Codex, &p, &c).unwrap();
        assert_eq!(std::fs::read_to_string(child.join("config.toml")).unwrap(), "# parent\nmodel_reasoning_effort = \"high\"\n[tui]\nshow_tooltips = false\n");
        let _ = std::fs::remove_dir_all(&parent);
        let _ = std::fs::remove_dir_all(&child);
    }

    #[test]
    fn a_broken_or_missing_parent_is_not_copied() {
        let parent = dir("sync-bad-parent");
        let child = dir("sync-bad-child");
        let (p, c) = (parent.to_string_lossy().into_owned(), child.to_string_lossy().into_owned());
        std::fs::write(child.join("settings.json"), "{\"model\":\"opus\"}").unwrap();
        assert!(sync_one(Provider::Claude, &p, &c).is_err());
        std::fs::write(parent.join("settings.json"), "{ not json").unwrap();
        assert!(sync_one(Provider::Claude, &p, &c).is_err());
        assert_eq!(std::fs::read_to_string(child.join("settings.json")).unwrap(), "{\"model\":\"opus\"}");
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
        assert!(apply(Provider::Claude, &p, &["tui".into()], &[]).is_err());
        assert_eq!(std::fs::read_to_string(d.join("settings.json")).unwrap(), "{ not json");
        let _ = std::fs::remove_dir_all(&d);
    }

}
