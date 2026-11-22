//! Claude Code's global config, an account's `.claude.json`: where the CLI
//! keeps its own state (sign-in, onboarding, projects) and the choices made
//! in its /config. Luna writes into it only what no start can pass: a
//! folder's trust bit the person ticked (trust.rs) and the copy-on-select
//! they chose in Settings (`copy_on_select_before_chat`). Every write goes
//! through `edit`.

use crate::applied::{Applied, Item};
use crate::settings::CopyOnSelect;
use serde_json::{Map, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The account's `.claude.json`: `<CLAUDE_CONFIG_DIR>/.claude.json`, which is
/// where the CLI keeps it once that variable is set, as Luna sets it.
pub(crate) fn config_file(account_path: &str) -> Result<PathBuf, String> {
    if account_path.is_empty() {
        return Err("No account selected".into());
    }
    Ok(Path::new(account_path).join(".claude.json"))
}

/// The config as far as it reads: empty where it is missing or will not
/// parse. For reading alone; a write starts from `read_for_write`.
pub(crate) fn read_config(path: &Path) -> Value {
    fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Map::new()))
}

/// Far above any `.claude.json` the CLI keeps (its history went to
/// `history.jsonl`); a larger one is not written.
const CONFIG_MAX: u64 = 32 << 20;

/// The config to write into; None where there is no file. One that will not
/// read or parse — the CLI rewrites it in place, so a read can catch it
/// half-written, and on Windows it may hold the file open — is an error, as
/// writing back over it would throw the account's whole config away. Read
/// as a settings file is (`boundary::read_settings_file`): a plain file
/// within `CONFIG_MAX`, so a FIFO there cannot hold a chat's start, and the
/// locks with it.
fn read_for_write(path: &Path) -> Result<Option<Map<String, Value>>, String> {
    let mut last = String::new();
    for attempt in 0..3 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(50));
        }
        match crate::boundary::read_settings_file(path, CONFIG_MAX) {
            Ok(Some(bytes)) => match serde_json::from_slice::<Value>(&bytes) {
                Ok(Value::Object(m)) => return Ok(Some(m)),
                Ok(_) => return Err(format!("{}: not a JSON object", path.display())),
                Err(e) => last = format!("{}: {e}", path.display()),
            },
            Ok(None) => return Ok(None),
            Err(e) => last = format!("{}: {e}", path.display()),
        }
    }
    Err(last)
}

/// What `edit` does where the account has no `.claude.json`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Missing {
    /// Starts one holding only what `change` puts in.
    Create,
    /// Writes nothing.
    Skip,
}

/// What `edit` did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Edited {
    Written,
    /// `change` found nothing to change: the file was not touched.
    AsItWas,
    /// No file, and `Missing::Skip`.
    NoFile,
}

/// Changes the account's `.claude.json` by `change`, which says whether it
/// changed anything; every other key stays as it was, in its place
/// (serde_json's `preserve_order`). Under the account's `rewriting_claude_config`
/// lock, waited for up to `wait` (a model ask's CLI rewrites the file from
/// what it read as it starts, which would undo the change), and `editing`,
/// with the read inside both. Nothing is written where `change` changes
/// nothing, and an error leaves the file as it was.
pub(crate) fn edit(
    account_path: &str,
    wait: Duration,
    missing: Missing,
    change: impl FnOnce(&mut Map<String, Value>) -> bool,
) -> Result<Edited, String> {
    let path = config_file(account_path)?;
    let _rewriting = crate::account_settings::rewriting_claude_config(account_path, wait)?;
    let _editing = crate::account_settings::editing()?;
    let mut cfg = match (read_for_write(&path)?, missing) {
        (Some(cfg), _) => cfg,
        (None, Missing::Create) => Map::new(),
        (None, Missing::Skip) => return Ok(Edited::NoFile),
    };
    if !change(&mut cfg) {
        return Ok(Edited::AsItWas);
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    // Two-space indents, as the CLI writes it. Via a sibling temp file so a
    // crash mid-write cannot truncate the account's whole config.
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(&cfg).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    // A rename refused (the CLI holding the file) leaves no temp file behind.
    fs::rename(&tmp, &path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        e.to_string()
    })?;
    Ok(Edited::Written)
}

/// How long a chat's start waits for a model ask on its account to let
/// `.claude.json` go (`edit`) before it starts without the write: the few
/// seconds of an ask's start, not the whole of `ASK_WAIT`.
const CHAT_START_WAIT: Duration = Duration::from_secs(5);

/// Before a Claude Code chat starts (a terminal session; helpers have no
/// screen and logins are left alone): the account's `copyOnSelect` set as
/// Settings has it. Claude Code reads it from `.claude.json` alone
/// (docs/cli-compatibility.md), so this is the one setting of the person's
/// Luna writes into an account (ADR-0030's note of 2026-11-23). What it did,
/// for the ledger; None where the setting leaves it to the account. A write
/// that fails is logged and the chat starts all the same.
pub(crate) fn copy_on_select_before_chat(account_path: &str) -> Option<Applied> {
    apply_copy_on_select(account_path, crate::settings::get().claude_copy_on_select, CHAT_START_WAIT)
}

pub(crate) fn apply_copy_on_select(account_path: &str, setting: CopyOnSelect, wait: Duration) -> Option<Applied> {
    let (want, word) = match setting {
        CopyOnSelect::Account => return None,
        CopyOnSelect::Off => (false, "off"),
        CopyOnSelect::On => (true, "on"),
    };
    // Claude Code reads `copyOnSelect ?? true`: absent is on already, so
    // "on" writes only over a value that is not `true`.
    let change = |cfg: &mut Map<String, Value>| {
        let differs = match cfg.get("copyOnSelect") {
            None => !want,
            Some(v) => v != &Value::Bool(want),
        };
        if differs {
            cfg.insert("copyOnSelect".into(), Value::Bool(want));
        }
        differs
    };
    // A missing file is left missing: the CLI makes it as it first starts
    // (a sign-in does), with defaults of its own that a file holding one key
    // of Luna's would stand in for.
    let value = match edit(account_path, wait, Missing::Skip, change) {
        Ok(Edited::Written) => {
            crate::log::info("claude-config", &format!("{account_path}: copyOnSelect set {want} in .claude.json, as Settings has it"));
            format!("{word}: written into .claude.json")
        }
        Ok(Edited::AsItWas) => format!("{word}: as .claude.json has it already"),
        Ok(Edited::NoFile) => format!("{word}: not written, the account has no .claude.json yet"),
        Err(e) => {
            crate::log::warn("claude-config", &format!("{account_path}: copyOnSelect left as it is, the chat starts all the same: {e}"));
            format!("{word}: not written: {e}")
        }
    };
    Some(Item::CopyOnSelect.with(value))
}

/// Settings › Chats: Claude Code's copy-on-select.
pub fn claude_copy_on_select() -> CopyOnSelect {
    crate::settings::get().claude_copy_on_select
}

pub fn set_claude_copy_on_select(value: CopyOnSelect) -> Result<CopyOnSelect, String> {
    crate::settings::update(|s| s.claude_copy_on_select = value)?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("luna-claude-config-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    const WAIT: Duration = Duration::from_millis(200);

    fn apply(d: &Path, setting: CopyOnSelect) -> Option<String> {
        apply_copy_on_select(&d.to_string_lossy(), setting, WAIT).map(|a| {
            assert_eq!(a.key, "claude.copy-on-select");
            a.value
        })
    }

    /// Off and On write the one key where it differs and leave every other
    /// key where it was; "as the account has it" writes nothing.
    #[test]
    fn copy_on_select_is_written_only_where_it_differs() {
        let d = account("differs");
        let file = d.join(".claude.json");
        fs::write(&file, "{\n  \"numStartups\": 3,\n  \"projects\": {}\n}").unwrap();
        assert_eq!(apply(&d, CopyOnSelect::Account), None);
        assert_eq!(apply(&d, CopyOnSelect::On).as_deref(), Some("on: as .claude.json has it already"), "absent is on");
        assert_eq!(fs::read_to_string(&file).unwrap(), "{\n  \"numStartups\": 3,\n  \"projects\": {}\n}");
        assert_eq!(apply(&d, CopyOnSelect::Off).as_deref(), Some("off: written into .claude.json"));
        assert_eq!(fs::read_to_string(&file).unwrap(), "{\n  \"numStartups\": 3,\n  \"projects\": {},\n  \"copyOnSelect\": false\n}");
        assert_eq!(apply(&d, CopyOnSelect::On).as_deref(), Some("on: written into .claude.json"));
        let cfg = read_config(&file);
        assert_eq!((cfg["copyOnSelect"].as_bool(), cfg["numStartups"].as_u64()), (Some(true), Some(3)));
        assert!(!d.join(".claude.json.tmp").exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn copy_on_select_as_it_is_leaves_the_file_untouched() {
        let d = account("same");
        let file = d.join(".claude.json");
        // Not as Luna would write it: a rewrite would show.
        let text = r#"{"copyOnSelect":false,  "z":1,"a":2}"#;
        fs::write(&file, text).unwrap();
        let before = fs::metadata(&file).unwrap().modified().unwrap();
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(apply(&d, CopyOnSelect::Off).as_deref(), Some("off: as .claude.json has it already"));
        assert_eq!(fs::read_to_string(&file).unwrap(), text);
        assert_eq!(fs::metadata(&file).unwrap().modified().unwrap(), before);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn copy_on_select_makes_no_file_where_there_is_none() {
        let d = account("missing");
        assert_eq!(apply(&d, CopyOnSelect::Off).as_deref(), Some("off: not written, the account has no .claude.json yet"));
        assert!(fs::read_dir(&d).unwrap().next().is_none(), "nothing made in the account");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn copy_on_select_leaves_a_file_that_will_not_parse() {
        let d = account("malformed");
        let file = d.join(".claude.json");
        for text in ["{\"copyOnSelect\": true, \"projects\": {", "[1, 2]"] {
            fs::write(&file, text).unwrap();
            let got = apply(&d, CopyOnSelect::Off).unwrap();
            assert!(got.starts_with("off: not written: "), "{got}");
            assert_eq!(fs::read_to_string(&file).unwrap(), text);
        }
        let _ = fs::remove_dir_all(&d);
    }

    /// Something other than a plain file at the name, a FIFO on Unix among
    /// them, is neither waited on nor written over.
    #[test]
    fn copy_on_select_leaves_what_is_not_a_plain_file() {
        let d = account("not-a-file");
        fs::create_dir(d.join(".claude.json")).unwrap();
        let got = apply(&d, CopyOnSelect::Off).unwrap();
        assert!(got.starts_with("off: not written: "), "{got}");
        assert!(d.join(".claude.json").is_dir());
        fs::remove_dir(d.join(".claude.json")).unwrap();
        #[cfg(unix)]
        {
            let fifo = std::ffi::CString::new(d.join(".claude.json").to_string_lossy().into_owned()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
            let got = apply(&d, CopyOnSelect::Off).unwrap();
            assert!(got.starts_with("off: not written: "), "{got}");
            use std::os::unix::fs::FileTypeExt;
            assert!(fs::symlink_metadata(d.join(".claude.json")).unwrap().file_type().is_fifo());
        }
        let _ = fs::remove_dir_all(&d);
    }

    /// A model ask holding the account's `.claude.json`: the start waits a
    /// moment, then goes on without the write.
    #[test]
    fn copy_on_select_waits_out_a_held_config_and_then_writes_nothing() {
        let d = account("held");
        let acc = d.to_string_lossy().into_owned();
        let file = d.join(".claude.json");
        fs::write(&file, r#"{"copyOnSelect": true}"#).unwrap();
        let held = crate::account_settings::rewriting_claude_config(&acc, Duration::ZERO).unwrap();
        let started = std::time::Instant::now();
        let got = apply(&d, CopyOnSelect::Off).unwrap();
        assert!(started.elapsed() >= WAIT, "it waited");
        assert!(got.starts_with("off: not written: ") && got.contains("try again"), "{got}");
        assert_eq!(fs::read_to_string(&file).unwrap(), r#"{"copyOnSelect": true}"#);
        drop(held);
        assert_eq!(apply(&d, CopyOnSelect::Off).as_deref(), Some("off: written into .claude.json"), "once it is let go");
        let _ = fs::remove_dir_all(&d);
    }
}
