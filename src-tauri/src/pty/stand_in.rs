//! The test build's stand-in for a CLI: a throwaway account folder holding
//! `luna-test-command.json` (`{"program": "...", "args": [...]}`) has its
//! sessions run that command instead of the CLI, in a real pty through the
//! core, so a soak can keep a pane printing for hours without a model or a
//! login. Compiled into the test build alone; its accounts live under
//! `test-build/accounts`.

use serde::Deserialize;
use std::path::Path;

const FILE: &str = "luna-test-command.json";

#[derive(Deserialize)]
struct StandIn {
    program: String,
    #[serde(default)]
    args: Vec<String>,
}

/// The program and arguments the account asks for, if it asks: None when
/// the file is not there, does not read, or names no program.
pub(super) fn read(account_path: &str) -> Option<(String, Vec<String>)> {
    let text = std::fs::read_to_string(Path::new(account_path).join(FILE)).ok()?;
    let s: StandIn = serde_json::from_str(&text).ok()?;
    (!s.program.trim().is_empty()).then_some((s.program, s.args))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_account_naming_a_program_gets_a_stand_in() {
        let dir = std::env::temp_dir().join(format!("luna-stand-in-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let account = dir.to_string_lossy().into_owned();
        assert_eq!(read(&account), None, "no file");
        for (text, want) in [
            (r#"{"program": "pwsh", "args": ["-NoProfile"]}"#, Some(("pwsh", vec!["-NoProfile"]))),
            (r#"{"program": "sh"}"#, Some(("sh", vec![]))),
            (r#"{"program": " "}"#, None),
            (r#"{"args": ["x"]}"#, None),
            ("not json", None),
        ] {
            std::fs::write(dir.join(FILE), text).unwrap();
            let want = want.map(|(p, a)| (p.to_owned(), a.into_iter().map(str::to_owned).collect::<Vec<_>>()));
            assert_eq!(read(&account), want, "{text}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
