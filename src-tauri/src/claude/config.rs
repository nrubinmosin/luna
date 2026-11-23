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
use std::time::{Duration, Instant};

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

/// The lock Claude Code saves its global config under (2.1.330: a
/// proper-lockfile mkdir lock at `<config file>.lock`, inside which it reads
/// the file afresh, changes it and writes it), and its `stale`: the
/// library's default, which the CLI leaves as it is, the holder renewing it
/// every 5 s.
const CLI_LOCK_STALE: Duration = Duration::from_secs(10);

/// Changes the account's `.claude.json` by `change`, which says whether it
/// changed anything; every other key stays, in its place (serde_json's
/// `preserve_order`), the file written out again pretty-printed. Taken in
/// turn, each waited for within `wait` all told: the account's
/// `rewriting_claude_config` lock (a model ask's CLI rewrites the file from
/// what it read as it starts, and would undo the change), `editing`, and
/// the CLI's own lock on the file (`CLI_LOCK_STALE`), so no Claude Code
/// saving its config meanwhile writes back what it read before. The read is
/// inside all three. Nothing is written where `change` changes nothing, or
/// where `.claude.json` is a link or junction (the replacement would be a
/// plain file in its place, the link's target left behind); an error leaves
/// the file as it was.
pub(crate) fn edit(
    account_path: &str,
    wait: Duration,
    missing: Missing,
    change: impl FnOnce(&mut Map<String, Value>) -> bool,
) -> Result<Edited, String> {
    let path = config_file(account_path)?;
    let deadline = Instant::now() + wait;
    let left = || deadline.saturating_duration_since(Instant::now());
    let _rewriting = crate::account_settings::rewriting_claude_config(account_path, left())?;
    let _editing = crate::account_settings::editing_within(left())?;
    match missing {
        Missing::Create => fs::create_dir_all(account_path).map_err(|e| e.to_string())?,
        // No file to write, nor the CLI's lock beside it to make.
        Missing::Skip if fs::symlink_metadata(&path).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) => return Ok(Edited::NoFile),
        Missing::Skip => {}
    }
    let mut lock = path.clone().into_os_string();
    lock.push(".lock");
    let _cli = crate::claude::oauth::take_within(Path::new(&lock), CLI_LOCK_STALE, left())?;
    if fs::symlink_metadata(&path).is_ok_and(|m| crate::boundary::is_link(&m)) {
        return Err(format!("{} is a link or junction: not written through", path.display()));
    }
    let (mut cfg, mode) = match (read_for_write(&path)?, missing) {
        (Some(cfg), _) => (cfg, file_mode(&path)),
        (None, Missing::Create) => (Map::new(), None),
        (None, Missing::Skip) => return Ok(Edited::NoFile),
    };
    if !change(&mut cfg) {
        return Ok(Edited::AsItWas);
    }
    // Two-space indents, as the CLI writes it.
    replace(&path, &serde_json::to_vec_pretty(&cfg).map_err(|e| e.to_string())?, mode)?;
    Ok(Edited::Written)
}

/// The file's permission bits, for its replacement to keep (Unix; Windows
/// gives the new file the folder's inherited access, as the CLI's own
/// rewrite does).
fn file_mode(path: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).ok().map(|m| m.permissions().mode() & 0o7777)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// `bytes` as the file at `path`, through a temp file of a name of its own
/// beside it, made new (`create_new`: nothing already there, a link
/// included, is opened or followed) and renamed over `path`, so a crash
/// mid-write cannot truncate the account's whole config. On Unix the temp
/// file is made 0600, the CLI's own mode, so nobody else can read it at any
/// moment, and takes `mode`, the old file's, before anything is written.
/// It is gone after any failure, and `path` is as it was.
fn replace(path: &Path, bytes: &[u8], mode: Option<u32>) -> Result<(), String> {
    use std::io::Write;
    let tmp = path.with_extension(format!("json.{}.luna-tmp", crate::secret::random_hex(8)));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let wrote = (|| -> std::io::Result<()> {
        #[cfg(unix)]
        if let Some(mode) = mode {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(mode))?;
        }
        #[cfg(not(unix))]
        let _ = mode;
        file.write_all(bytes)?;
        file.sync_all()
    })();
    drop(file);
    // A rename refused (the CLI holding the file on Windows) leaves no temp
    // file behind either.
    if let Err(e) = wrote.and_then(|()| fs::rename(&tmp, path)) {
        let _ = fs::remove_file(&tmp);
        return Err(e.to_string());
    }
    Ok(())
}

/// How long a chat's start waits, all told, for the locks of `edit` before
/// it starts without the write: the few seconds of a model ask's start, not
/// the whole of `ASK_WAIT`.
const CHAT_START_WAIT: Duration = Duration::from_secs(5);

/// Before a Claude Code chat starts (a terminal session; helpers have no
/// screen and logins are left alone), and so before its CLI is found and its
/// process made: the account's `copyOnSelect` set as Settings has it. Claude
/// Code reads it from `.claude.json` alone (docs/cli-compatibility.md), so
/// this is the one setting of the person's Luna writes into an account
/// (ADR-0030's note of 2026-11-23). What it did, for the ledger; None where
/// the setting leaves it to the account. Luna's settings.json that will not
/// read writes nothing, rather than the default; a write that fails is
/// logged; the chat starts all the same.
pub(crate) fn copy_on_select_before_chat(account_path: &str) -> Option<Applied> {
    before_chat(account_path, setting_for(account_path), CHAT_START_WAIT)
}

/// The setting a test's start runs with, by account, in place of
/// settings.json, which every test of the process shares; by account and
/// not by thread, as a start builds its launch on the runtime's threads.
#[cfg(test)]
static FOR_TEST: std::sync::Mutex<std::collections::BTreeMap<String, CopyOnSelect>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

#[cfg(test)]
pub(crate) fn setting_for_test(account_path: &str, setting: Option<CopyOnSelect>) {
    let mut m = FOR_TEST.lock().unwrap_or_else(|e| e.into_inner());
    match setting {
        Some(s) => m.insert(account_path.to_owned(), s),
        None => m.remove(account_path),
    };
}

fn setting_for(account_path: &str) -> Result<CopyOnSelect, String> {
    #[cfg(test)]
    if let Some(s) = FOR_TEST.lock().unwrap_or_else(|e| e.into_inner()).get(account_path) {
        return Ok(*s);
    }
    let _ = account_path;
    crate::settings::get_checked().map(|s| s.copy_on_select)
}

fn before_chat(account_path: &str, setting: Result<CopyOnSelect, String>, wait: Duration) -> Option<Applied> {
    match setting {
        Ok(setting) => apply_copy_on_select(account_path, setting, wait),
        Err(e) => {
            crate::log::warn("claude-config", &format!("{account_path}: copyOnSelect left as it is, Luna's settings do not read: {e}"));
            Some(Item::CopyOnSelect.with(format!("not written: Luna's settings do not read: {e}")))
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn account(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("luna-claude-config-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// Long enough for the other tests of the process to let `editing` go;
    /// a test of a lock held waits `SHORT`.
    const WAIT: Duration = Duration::from_secs(10);
    const SHORT: Duration = Duration::from_millis(200);

    fn apply(d: &Path, setting: CopyOnSelect) -> Option<String> {
        apply_within(d, setting, WAIT)
    }

    fn apply_within(d: &Path, setting: CopyOnSelect, wait: Duration) -> Option<String> {
        apply_copy_on_select(&d.to_string_lossy(), setting, wait).map(|a| {
            assert_eq!(a.key, "copy-on-select");
            a.value
        })
    }

    /// What is in the account folder besides the config: no temp file and
    /// no lock of Luna's is left behind.
    fn leftovers(d: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(d).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        names.retain(|n| n != ".claude.json");
        names.sort();
        names
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
        assert_eq!(leftovers(&d), Vec::<String>::new());
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

    /// A `.claude.json` that is a link is not written through, nor replaced
    /// by a plain file: the link and what it leads to stay as they were.
    #[cfg(unix)]
    #[test]
    fn copy_on_select_does_not_write_through_a_link() {
        let d = account("link");
        let target = d.join("dotfiles.json");
        fs::write(&target, r#"{"copyOnSelect": true}"#).unwrap();
        std::os::unix::fs::symlink(&target, d.join(".claude.json")).unwrap();
        let got = apply(&d, CopyOnSelect::Off).unwrap();
        assert!(got.starts_with("off: not written: ") && got.contains("link"), "{got}");
        assert!(fs::symlink_metadata(d.join(".claude.json")).unwrap().file_type().is_symlink());
        assert_eq!(fs::read_to_string(&target).unwrap(), r#"{"copyOnSelect": true}"#);
        let _ = fs::remove_dir_all(&d);
    }

    /// The replacement keeps the file's permission bits.
    #[cfg(unix)]
    #[test]
    fn copy_on_select_keeps_the_files_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let d = account("mode");
        let file = d.join(".claude.json");
        fs::write(&file, r#"{"copyOnSelect": true}"#).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o640)).unwrap();
        assert_eq!(apply(&d, CopyOnSelect::Off).as_deref(), Some("off: written into .claude.json"));
        assert_eq!(fs::metadata(&file).unwrap().permissions().mode() & 0o7777, 0o640);
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
        let started = Instant::now();
        let got = apply_within(&d, CopyOnSelect::Off, SHORT).unwrap();
        assert!(started.elapsed() >= SHORT, "it waited");
        assert!(got.starts_with("off: not written: ") && got.contains("try again"), "{got}");
        assert_eq!(fs::read_to_string(&file).unwrap(), r#"{"copyOnSelect": true}"#);
        drop(held);
        assert_eq!(apply(&d, CopyOnSelect::Off).as_deref(), Some("off: written into .claude.json"), "once it is let go");
        let _ = fs::remove_dir_all(&d);
    }

    /// Claude Code saving its config holds `.claude.json.lock`: nothing is
    /// written while it is held, and the CLI's lock is left as it was; one
    /// left stale by a CLI that died holding it is taken over, as oauth.rs
    /// takes the refresh locks, and let go again.
    #[test]
    fn copy_on_select_stands_aside_for_the_clis_own_lock() {
        let d = account("cli-lock");
        let file = d.join(".claude.json");
        let lock = d.join(".claude.json.lock");
        fs::write(&file, r#"{"copyOnSelect": true}"#).unwrap();
        fs::create_dir(&lock).unwrap();
        let got = apply_within(&d, CopyOnSelect::Off, Duration::from_secs(3)).unwrap();
        assert!(got.starts_with("off: not written: ") && got.contains("is held"), "{got}");
        assert_eq!(fs::read_to_string(&file).unwrap(), r#"{"copyOnSelect": true}"#);
        assert!(lock.is_dir(), "the CLI's lock is left as it was");
        #[cfg(unix)]
        {
            let old = std::time::SystemTime::now() - Duration::from_secs(60);
            fs::File::open(&lock).unwrap().set_times(fs::FileTimes::new().set_modified(old).set_accessed(old)).unwrap();
            assert_eq!(apply(&d, CopyOnSelect::Off).as_deref(), Some("off: written into .claude.json"), "a stale lock is taken over");
            assert!(!lock.exists(), "and let go");
        }
        let _ = fs::remove_dir_all(&d);
    }

    /// A settings.json that will not read writes nothing into the account,
    /// rather than the default.
    #[test]
    fn copy_on_select_writes_nothing_when_luna_s_settings_do_not_read() {
        let d = account("no-settings");
        let file = d.join(".claude.json");
        fs::write(&file, r#"{"copyOnSelect": true}"#).unwrap();
        let got = before_chat(&d.to_string_lossy(), Err("settings.json: expected value".into()), SHORT).unwrap();
        assert!(got.value.starts_with("not written: Luna's settings do not read"), "{}", got.value);
        assert_eq!(fs::read_to_string(&file).unwrap(), r#"{"copyOnSelect": true}"#);
        let _ = fs::remove_dir_all(&d);
    }
}
