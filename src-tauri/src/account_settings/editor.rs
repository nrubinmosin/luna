//! The account's settings file as text, for editing it by hand: read, checked
//! with the line and column of a problem, laid out, and written only once it
//! parses.

use super::*;

/// The account's settings file as text, for editing it by hand.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigFile {
    pub path: String,
    pub text: String,
    /// False when the account has no such file yet; `text` is a starting point.
    pub exists: bool,
}

pub(super) fn config_file(provider: Provider, account_path: &str) -> Result<PathBuf, String> {
    match provider {
        Provider::Claude => claude_file(account_path),
        Provider::Codex => crate::codex::config::account_file(account_path),
    }
}

/// Whether `text` is a settings file this provider can read: a JSON object
/// for Claude Code, TOML for Codex. The message says where it breaks.
pub(super) fn check_config(provider: Provider, text: &str) -> Result<(), String> {
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
        Provider::Codex => Ok(taplo::formatter::format(&text, Default::default())),
    }
}

/// The page's account path is held to an account folder of Luna's
/// (boundary.rs) before anything there is read or written.
#[tauri::command]
pub fn read_account_config(provider: Provider, account_path: crate::boundary::AccountPath) -> Result<ConfigFile, String> {
    read_config_file(provider, &String::from(account_path))
}

#[tauri::command]
pub fn write_account_config(provider: Provider, account_path: crate::boundary::AccountPath, text: String) -> Result<(), String> {
    write_config_file(provider, &String::from(account_path), text)
}

pub(crate) fn read_config_file(provider: Provider, account_path: &str) -> Result<ConfigFile, String> {
    let path = config_file(provider, account_path)?;
    let (text, exists) = match std::fs::read_to_string(&path) {
        Ok(t) => (t, true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (
            if provider == Provider::Claude {
                "{
}
"
                .to_string()
            } else {
                String::new()
            },
            false,
        ),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    Ok(ConfigFile { path: path.to_string_lossy().into_owned(), text, exists })
}

/// Writes the text as typed — comments, order and spacing are the user's —
/// once it parses; a file that would not is never written.
pub(crate) fn write_config_file(provider: Provider, account_path: &str, text: String) -> Result<(), String> {
    check_config(provider, &text)?;
    let path = config_file(provider, account_path)?;
    let editing = editing()?;
    write_text(&path, &text)?;
    drop(editing);
    crate::log::info("settings", &format!("{} edited by hand", path.display()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_editor_checks_and_lays_out_both_files() {
        let bad = check_account_config(
            Provider::Codex,
            "a = 1
b = [
"
            .into(),
        )
        .unwrap();
        assert!(bad.line.is_some(), "{}", bad.message);
        assert!(check_account_config(
            Provider::Codex,
            "a = 1
"
            .into()
        )
        .is_none());
        let bad = check_account_config(
            Provider::Claude,
            "{
  \"a\": ,
}"
            .into(),
        )
        .unwrap();
        assert_eq!(bad.line, Some(2));
        let laid = format_account_config(Provider::Claude, r#"{"b":1,"a":{"c":true}}"#.into()).unwrap();
        assert_eq!(
            laid,
            "{
  \"b\": 1,
  \"a\": {
    \"c\": true
  }
}
"
        );
        let laid = format_account_config(
            Provider::Codex,
            "# keep
z   =  1
a=\"x\"
"
            .into(),
        )
        .unwrap();
        assert!(laid.contains("# keep") && laid.find("z = 1") < laid.find("a = \"x\""), "{laid}");
        assert!(format_account_config(Provider::Codex, "a = [".into()).is_err());
    }

    #[test]
    fn the_editor_writes_only_what_parses_and_keeps_it_as_typed() {
        let d = dir("editor");
        let p = d.to_string_lossy().into_owned();
        assert!(!read_config_file(Provider::Claude, &p).unwrap().exists);
        assert!(write_config_file(Provider::Claude, &p, "{ \"tui\": ".into()).is_err());
        assert!(write_config_file(Provider::Claude, &p, "[1]".into()).is_err());
        let typed = "{
  \"tui\":   \"fullscreen\"
}
";
        write_config_file(Provider::Claude, &p, typed.into()).unwrap();
        assert_eq!(read_config_file(Provider::Claude, &p).unwrap().text, typed);
        assert!(write_config_file(Provider::Codex, &p, "model = [unterminated".into()).is_err());
        let toml = "# mine
model = \"gpt-6\"   # keep
";
        write_config_file(Provider::Codex, &p, toml.into()).unwrap();
        assert_eq!(std::fs::read_to_string(d.join("config.toml")).unwrap(), toml);
        let _ = std::fs::remove_dir_all(&d);
    }
}
