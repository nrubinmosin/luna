//! Changes key by key: to one of an account's files, to several accounts
//! together or none of them, and the recommendations taken or taken out.

use super::*;

/// One of an account's files, as a fix changes it (checks.rs).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum KeyFile {
    ClaudeSettings,
    ClaudeGlobal,
    CodexConfig,
}

/// A file's contents, its formatting kept as far as the format has any.
pub(crate) enum Doc {
    Json(Value),
    Toml(DocumentMut),
}

/// A change to one key.
pub(crate) enum Edit {
    Set(&'static [&'static str], Value),
    Remove(&'static [&'static str]),
    /// Takes the key out only while it says this.
    RemoveIf(&'static [&'static str], bool),
}

impl KeyFile {
    fn path(self, account_path: &str) -> Result<PathBuf, String> {
        match self {
            KeyFile::ClaudeSettings => claude_spec_file(account_path, false),
            KeyFile::ClaudeGlobal => claude_spec_file(account_path, true),
            KeyFile::CodexConfig => crate::codex::config::account_file(account_path),
        }
    }

    /// The file as it stands; an error for one that does not parse.
    pub(crate) fn read(self, account_path: &str) -> Result<Doc, String> {
        let path = self.path(account_path)?;
        match self {
            KeyFile::CodexConfig => crate::codex::config::read(&path).map(Doc::Toml),
            _ => read_claude(&path).map(Doc::Json),
        }
    }
}

/// Takes the key out of tables and inline tables alike, and with it any it
/// leaves empty.
fn toml_take(table: &mut dyn toml_edit::TableLike, path: &[&str]) {
    let Some((first, rest)) = path.split_first() else { return };
    if rest.is_empty() {
        table.remove(first);
        return;
    }
    let emptied = match table.get_mut(first).and_then(|i| i.as_table_like_mut()) {
        Some(child) => {
            toml_take(child, rest);
            child.is_empty()
        }
        None => false,
    };
    if emptied {
        table.remove(first);
    }
}

impl Doc {
    pub(crate) fn bool_at(&self, path: &[&str]) -> Option<bool> {
        match self {
            Doc::Json(v) => json_at(v, path).and_then(Value::as_bool),
            Doc::Toml(d) => toml_at(d, path).and_then(Item::as_bool),
        }
    }

    /// The value at the key as the file writes it.
    pub(crate) fn said(&self, path: &[&str]) -> Option<String> {
        match self {
            Doc::Json(v) => json_at(v, path).map(Value::to_string),
            Doc::Toml(d) => toml_at(d, path).and_then(Item::as_value).map(plain),
        }
    }

    fn edit(&mut self, edit: &Edit) {
        match edit {
            Edit::RemoveIf(path, value) => {
                if self.bool_at(path) == Some(*value) {
                    self.edit(&Edit::Remove(path));
                }
            }
            Edit::Set(path, value) => match self {
                Doc::Json(v) => json_set(v, path, value.clone()),
                Doc::Toml(d) => {
                    if let Some(value) = toml_value(value) {
                        toml_set(d, path, value);
                    }
                }
            },
            Edit::Remove(path) => match self {
                Doc::Json(v) => json_remove(v, path),
                Doc::Toml(d) => toml_take(d.as_table_mut(), path),
            },
        }
    }

    fn text(&self) -> Result<String, String> {
        match self {
            Doc::Json(v) => Ok(serde_json::to_string_pretty(v).map_err(|e| e.to_string())? + "\n"),
            Doc::Toml(d) => Ok(d.to_string()),
        }
    }
}

/// What is to be written to one file, and what to put back if another fails.
struct Change {
    path: PathBuf,
    before: Option<String>,
    text: String,
}

/// Writes them all, or puts back the ones written: nothing half done stays.
fn commit(changes: &[Change], mut write: impl FnMut(&Path, &str) -> Result<(), String>) -> Result<(), String> {
    for (at, c) in changes.iter().enumerate() {
        if let Err(e) = write(&c.path, &c.text) {
            for done in &changes[..at] {
                let back = match &done.before {
                    Some(text) => write_text(&done.path, text),
                    None => std::fs::remove_file(&done.path).map_err(|e| e.to_string()),
                };
                if let Err(e) = back {
                    crate::log::error("settings", &format!("could not put {} back: {e}", done.path.display()));
                }
            }
            return Err(format!("{}: {e}", c.path.display()));
        }
    }
    Ok(())
}

/// The edits on each account's file whose `off` still holds, as one change:
/// every file is read and changed under the lock before the first is written,
/// so a file that does not read stops them all and a sync cannot come in
/// between. `off` is asked again of the changed file: a form the edits could not
/// reach, which would go on saying the same, is an error and not a success.
/// Which of `accounts` it is for is decided here, from what they say under the
/// lock; `shown` are the ones the user was told of, and another that turns out
/// to need it stops the change, for them to be told. How many files changed.
pub(crate) fn edit_together(
    file: KeyFile,
    accounts: &[String],
    shown: &[String],
    edits: &[Edit],
    off: &dyn Fn(&Doc) -> bool,
) -> Result<usize, String> {
    let _editing = editing()?;
    let mut changes = Vec::new();
    for account in accounts {
        let path = file.path(account)?;
        let mut doc = file.read(account)?;
        if !off(&doc) {
            continue;
        }
        if !shown.contains(account) {
            return Err(format!("{} changed after this was shown: nothing written, look again.", path.display()));
        }
        for edit in edits {
            doc.edit(edit);
        }
        if off(&doc) {
            return Err(format!("{}: written in a form Luna cannot change. Change it by hand.", path.display()));
        }
        let before = std::fs::read_to_string(&path).ok();
        changes.push(Change { path, before, text: doc.text()? });
    }
    commit(&changes, write_text)?;
    Ok(changes.len())
}

/// Sets the `add` keys to their recommended values and takes the `remove`
/// ones out — only where the file still says exactly what was recommended:
/// a value the user wrote themselves is theirs, not a recommendation to undo.
pub(super) fn apply(provider: Provider, account_path: &str, add: &[String], remove: &[String]) -> Result<(), String> {
    let named = |keys: &[String]| -> Vec<Spec> { specs(provider).into_iter().filter(|s| keys.contains(&s.path.join("."))).collect() };
    let (add, remove) = (named(add), named(remove));
    if add.is_empty() && remove.is_empty() {
        return Ok(());
    }
    // A Codex config and `.claude.json` are the CLI's to rewrite from what it
    // read: first the account's hold, as for a recommendation.
    let rewritten = provider == Provider::Codex || add.iter().chain(&remove).any(|s| s.global);
    let _held = if rewritten { Some(crate::account_use::hold(std::slice::from_ref(&account_path.to_string()))?) } else { None };
    // And a model ask's CLI, which the core's table does not see (claude/models.rs).
    let claude_global = provider == Provider::Claude && rewritten;
    let _rewriting = if claude_global { Some(rewriting_claude_config(account_path, crate::claude::models::ASK_WAIT)?) } else { None };
    let _editing = editing()?;
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
pub fn recommended_settings(provider: Provider, account_path: crate::boundary::AccountPath) -> Result<Vec<Rec>, String> {
    recommend(provider, &String::from(account_path))
}

#[tauri::command]
pub fn apply_recommended(provider: Provider, account_path: String, add: Vec<String>, remove: Vec<String>) -> Result<(), String> {
    apply(provider, &account_path, &add, &remove)?;
    crate::log::info("settings", &format!("recommended settings on {account_path}: {} added, {} taken out", add.len(), remove.len()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        std::fs::write(
            d.join("settings.json"),
            r#"{"model":"opus","env":{"CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY":"1"},"tui":"fullscreen","cleanupPeriodDays":60}"#,
        )
        .unwrap();
        let p = d.to_string_lossy().into_owned();
        let out = ["env.CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY".to_string(), "tui".into(), "cleanupPeriodDays".into()];
        apply(Provider::Claude, &p, &[], &out).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(d.join("settings.json")).unwrap()).unwrap();
        assert_eq!(v, json!({ "model": "opus", "cleanupPeriodDays": 60 }), "60 is the user's, not the recommendation");

        std::fs::write(
            d.join("config.toml"),
            "# mine
web_search = \"live\"

[tui]
show_tooltips = false

[feedback]
enabled = false
other = 1
",
        )
        .unwrap();
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
    fn codex_apply_keeps_comments_and_writes_tables() {
        let d = dir("codex");
        std::fs::write(
            d.join("config.toml"),
            "# mine\nmodel = \"gpt-6\" # keep\n# searching\nweb_search = \"off\"\n\n[tui]\ntheme = \"dark\"\n",
        )
        .unwrap();
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
    fn a_broken_settings_file_is_left_alone() {
        let d = dir("broken");
        std::fs::write(d.join("settings.json"), "{ not json").unwrap();
        let p = d.to_string_lossy().into_owned();
        assert!(apply(Provider::Claude, &p, &["tui".into()], &[]).is_err());
        assert_eq!(std::fs::read_to_string(d.join("settings.json")).unwrap(), "{ not json");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_change_to_two_accounts_goes_in_whole_or_not_at_all() {
        let (one, two) = (dir("together-one"), dir("together-two"));
        let (a, b) = (one.to_string_lossy().into_owned(), two.to_string_lossy().into_owned());
        let off = |d: &Doc| d.bool_at(&["disableAllHooks"]) == Some(true);
        let edits = [Edit::Remove(&["disableAllHooks"])];

        // The second file does not read: the first is not written either.
        std::fs::write(one.join("settings.json"), r#"{"disableAllHooks":true}"#).unwrap();
        std::fs::write(two.join("settings.json"), "{ not json").unwrap();
        let both = [a.clone(), b.clone()];
        assert!(edit_together(KeyFile::ClaudeSettings, &both, &both, &edits, &off).is_err());
        assert_eq!(std::fs::read_to_string(one.join("settings.json")).unwrap(), r#"{"disableAllHooks":true}"#);

        // The last write fails: the first goes back, a new file goes again.
        std::fs::write(one.join("settings.json"), "old").unwrap();
        let changes = [
            Change { path: one.join("settings.json"), before: Some("old".into()), text: "new".into() },
            Change { path: two.join("fresh.json"), before: None, text: "new".into() },
            Change { path: two.join("refused.json"), before: None, text: "new".into() },
        ];
        let failed = commit(&changes, |path, text| {
            if path.ends_with("refused.json") {
                return Err("refused".into());
            }
            write_text(path, text)
        });
        assert!(failed.unwrap_err().contains("refused"));
        assert_eq!(std::fs::read_to_string(one.join("settings.json")).unwrap(), "old");
        assert!(!two.join("fresh.json").exists());

        // A file the fix has nothing to do for is left alone, to the byte.
        std::fs::write(one.join("settings.json"), r#"{"disableAllHooks":true}"#).unwrap();
        let clean = "{ \"model\":   \"opus\" }";
        std::fs::write(two.join("settings.json"), clean).unwrap();
        assert_eq!(edit_together(KeyFile::ClaudeSettings, &both, &both, &edits, &off).unwrap(), 1);
        assert_eq!(std::fs::read_to_string(two.join("settings.json")).unwrap(), clean);
        let _ = std::fs::remove_dir_all(&one);
        let _ = std::fs::remove_dir_all(&two);
    }

    #[test]
    fn a_key_goes_from_a_table_and_an_inline_table_alike_and_never_only_seems_to() {
        let edits = [Edit::RemoveIf(&["features", "hooks"], false), Edit::RemoveIf(&["features", "codex_hooks"], false)];
        let off = |d: &Doc| d.bool_at(&["features", "hooks"]) == Some(false) || d.bool_at(&["features", "codex_hooks"]) == Some(false);
        for (tag, before, after) in [
            (
                "table",
                "# mine\nmodel = \"x\"\n\n[features]\nhooks = false\nother = true\n",
                "# mine\nmodel = \"x\"\n\n[features]\nother = true\n",
            ),
            ("table-gone", "model = \"x\"\n\n[features]\nhooks = false\n", "model = \"x\"\n"),
            ("dotted", "model = \"x\"\nfeatures.hooks = false\n", "model = \"x\"\n"),
            ("alias", "[features]\ncodex_hooks = false\nother = true\n", "[features]\nother = true\n"),
            ("inline", "features = { hooks = false, other = true }\nmodel = \"x\"\n", "features = { other = true }\nmodel = \"x\"\n"),
            ("inline-gone", "features = { hooks = false }\nmodel = \"x\"\n", "model = \"x\"\n"),
        ] {
            let d = dir(&format!("toml-{tag}"));
            let p = d.to_string_lossy().into_owned();
            std::fs::write(d.join("config.toml"), before).unwrap();
            let one = std::slice::from_ref(&p);
            assert_eq!(edit_together(KeyFile::CodexConfig, one, one, &edits, &off).unwrap(), 1, "{tag}");
            let text = std::fs::read_to_string(d.join("config.toml")).unwrap();
            let doc: DocumentMut = text.parse().unwrap();
            assert!(!off(&Doc::Toml(doc)), "{tag}: {text}");
            assert_eq!(text, after, "{tag}");
            let _ = std::fs::remove_dir_all(&d);
        }

        // Edits that leave it saying the same are an error, and nothing is written.
        let d = dir("toml-stuck");
        let p = d.to_string_lossy().into_owned();
        let before = "features = { hooks = false }\n";
        std::fs::write(d.join("config.toml"), before).unwrap();
        let wrong = [Edit::Remove(&["elsewhere", "hooks"])];
        let one = std::slice::from_ref(&p);
        let e = edit_together(KeyFile::CodexConfig, one, one, &wrong, &off).unwrap_err();
        assert!(e.contains("cannot change"), "{e}");
        assert_eq!(std::fs::read_to_string(d.join("config.toml")).unwrap(), before);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_key_set_leaves_the_rest_of_the_file_as_it_was() {
        let d = dir("edit-set");
        let p = d.to_string_lossy().into_owned();
        std::fs::write(d.join("settings.json"), r#"{"model":"opus"}"#).unwrap();
        let off = |d: &Doc| d.bool_at(&["skipDangerousModePermissionPrompt"]) != Some(true);
        let edits = [Edit::Set(&["skipDangerousModePermissionPrompt"], json!(true))];
        let one = std::slice::from_ref(&p);
        edit_together(KeyFile::ClaudeSettings, one, one, &edits, &off).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(d.join("settings.json")).unwrap()).unwrap();
        assert_eq!(v, json!({ "model": "opus", "skipDangerousModePermissionPrompt": true }));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_comfort_key_a_cli_rewrites_waits_for_its_account_to_be_free() {
        let d = dir("comfort");
        let p = d.to_string_lossy().into_owned();
        std::fs::write(d.join("config.toml"), "model = \"x\"\n").unwrap();
        let on_it = crate::account_use::running_on(&p);
        let e = apply_recommended(Provider::Codex, p.clone(), vec!["web_search".into()], vec![]).unwrap_err();
        assert!(e.contains("in use"), "{e}");
        assert_eq!(std::fs::read_to_string(d.join("config.toml")).unwrap(), "model = \"x\"\n");
        assert!(apply_recommended(Provider::Claude, p.clone(), vec!["copyOnSelect".into()], vec![]).unwrap_err().contains("in use"));
        assert!(!d.join(".claude.json").exists());
        // `settings.json` is not rewritten that way, and a key that is no
        // recommendation asks nothing of the account.
        apply_recommended(Provider::Claude, p.clone(), vec!["tui".into()], vec![]).unwrap();
        apply_recommended(Provider::Codex, p.clone(), vec!["no.such.key".into()], vec![]).unwrap();
        drop(on_it);
        apply_recommended(Provider::Codex, p.clone(), vec!["web_search".into()], vec![]).unwrap();
        assert!(std::fs::read_to_string(d.join("config.toml")).unwrap().contains("web_search"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
