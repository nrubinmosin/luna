//! An account's files read and written — Claude Code's JSON, Codex's TOML with
//! its comments kept — and keys found, set and removed in them.

use super::*;

pub(super) fn claude_file(account_path: &str) -> Result<PathBuf, String> {
    if account_path.is_empty() {
        return Err("No account selected".into());
    }
    Ok(Path::new(account_path).join("settings.json"))
}

/// Claude Code's global config, which `CLAUDE_CONFIG_DIR` puts in the account
/// folder. The CLI saves it by re-reading the file under a lock, so a key set
/// here while sessions run is kept, not written over.
pub(super) const CLAUDE_GLOBAL: &str = ".claude.json";

pub(super) fn claude_spec_file(account_path: &str, global: bool) -> Result<PathBuf, String> {
    let file = claude_file(account_path)?;
    Ok(if global { file.with_file_name(CLAUDE_GLOBAL) } else { file })
}

/// The file as an object, or an empty one when it is missing. A file that
/// does not parse is an error: writing over it would lose the user's settings.
/// The CLI rewrites `.claude.json` in place, so a read can catch it empty or
/// half-written, and on Windows held open: such a read is tried again before
/// it counts, and only a file still blank after that is taken as empty.
pub(super) fn read_claude(path: &Path) -> Result<Value, String> {
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

pub(super) fn write_claude(path: &Path, v: &Value) -> Result<(), String> {
    write_text(path, &(serde_json::to_string_pretty(v).map_err(|e| e.to_string())? + "\n"))
}

/// Writes via a sibling temp file so a crash mid-write cannot truncate the file.
pub(super) fn write_text(path: &Path, text: &str) -> Result<(), String> {
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

pub(super) fn json_at<'a>(v: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter().try_fold(v, |cur, k| cur.get(*k))
}

pub(super) fn json_set(v: &mut Value, path: &[&str], value: Value) {
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
pub(super) fn json_remove(v: &mut Value, path: &[&str]) {
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

pub(super) fn toml_at<'a>(doc: &'a DocumentMut, path: &[&str]) -> Option<&'a Item> {
    let mut cur = doc.as_item();
    for k in path {
        cur = cur.get(*k)?;
    }
    Some(cur)
}

/// A JSON scalar as a TOML value; the Codex specs use nothing else.
pub(super) fn toml_value(v: &Value) -> Option<toml_edit::Value> {
    match v {
        Value::Bool(b) => Some((*b).into()),
        Value::String(s) => Some(s.as_str().into()),
        Value::Number(n) => n.as_i64().map(Into::into).or_else(|| n.as_f64().map(Into::into)),
        _ => None,
    }
}

/// A value as written, without the spacing and comment around it.
pub(super) fn plain(v: &toml_edit::Value) -> String {
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

pub(super) fn toml_set(doc: &mut DocumentMut, path: &[&str], value: toml_edit::Value) {
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
pub(super) fn toml_remove(table: &mut toml_edit::Table, path: &[&str]) {
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
