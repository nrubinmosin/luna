//! A parent account per provider whose file the others get an exact copy of:
//! credentials, logins and Codex's trusted projects never travel.

use super::*;

/// The parent's file as text, checked to parse: a broken file is not spread
/// onto every account. A parent without one has nothing to copy.
fn parent_text(provider: Provider, parent: &str) -> Result<String, String> {
    let path = config_file(provider, parent)?;
    let read = || match std::fs::read_to_string(&path) {
        Ok(t) => Ok(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(format!("{} does not exist", path.display())),
        Err(e) => Err(format!("{}: {e}", path.display())),
    };
    // An editor saving a file may empty it first, so a blank one is looked at
    // again. Still blank, that is what the parent says: a Codex config may
    // hold nothing, as one does once a fix has taken its only key, and the
    // others follow it. Blank is no JSON, which stops Claude Code's below.
    let mut text = read()?;
    for _ in 0..4 {
        if !text.trim().is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        text = read()?;
    }
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
pub(crate) fn sync_one(provider: Provider, parent: &str, child: &str) -> Result<(), String> {
    sync_one_seeing(provider, parent, child, || ())
}

/// `sync_one`, calling `read` once the parent has been read. Under the lock
/// from before that read: a parent read ahead of it could be written over the
/// child after a change to both had gone through in between.
fn sync_one_seeing(provider: Provider, parent: &str, child: &str, read: impl FnOnce()) -> Result<(), String> {
    let _editing = editing()?;
    let from = parent_text(provider, parent)?;
    read();
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

/// What a sync of this account waits for. Codex rewrites config.toml from what
/// it read, so an account with a Codex process on it keeps its file until it is
/// free. A helper between turns or long finished has none, and its next turn
/// waits out the hold: a retained entry is no reason to skip the account.
pub(crate) fn sync_gate(provider: Provider, account: &str) -> Result<Option<crate::account_use::Held>, String> {
    match provider {
        Provider::Codex => crate::account_use::hold(std::slice::from_ref(&account.to_string())).map(Some),
        Provider::Claude => Ok(None),
    }
}

fn sync(provider: Provider) -> Result<SyncReport, String> {
    let s = crate::settings::get();
    let parent_name = match provider {
        Provider::Claude => s.settings_parents.claude,
        Provider::Codex => s.settings_parents.codex,
    }
    .ok_or("No parent account chosen")?;
    let accounts: Vec<_> = crate::accounts::list_accounts()?.into_iter().filter(|a| a.provider == provider).collect();
    let parent = accounts.iter().find(|a| a.name == parent_name).ok_or_else(|| format!("The parent account \"{parent_name}\" is gone"))?;
    let mut report = SyncReport::default();
    for a in accounts.iter().filter(|a| a.name != parent.name) {
        match sync_gate(provider, &a.path).and_then(|_held| sync_one(provider, &parent.path, &a.path)) {
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
    if report.failed.is_empty() {
        crate::log::info("settings", &msg)
    } else {
        crate::log::warn("settings", &msg)
    }
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
/// for each provider that has a parent. Asked of the core, which alone knows
/// which accounts are in use.
pub fn sync_on_start() {
    if !crate::settings::get().settings_sync_on_start {
        return;
    }
    std::thread::spawn(|| {
        let s = crate::settings::get();
        for (provider, parent) in [(Provider::Claude, &s.settings_parents.claude), (Provider::Codex, &s.settings_parents.codex)] {
            if parent.is_some() {
                if let Err(e) = crate::corelink::request("sync_settings_now".into(), json!({ "provider": provider })) {
                    crate::log::warn("settings", &format!("{} settings sync skipped: {e}", provider.as_str()));
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

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
        std::fs::write(
            parent.join("config.toml"),
            "# parent\nmodel_reasoning_effort = \"high\"\n[projects.'C:\\\\p']\ntrust_level = \"trusted\"\n[tui]\nshow_tooltips = false\n",
        )
        .unwrap();
        std::fs::write(
            child.join("config.toml"),
            "# mine\nmodel_reasoning_effort = \"low\"\nmine = 1\n[projects.'C:\\\\c']\ntrust_level = \"trusted\"\n",
        )
        .unwrap();
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
        assert_eq!(
            std::fs::read_to_string(child.join("config.toml")).unwrap(),
            "# parent\nmodel_reasoning_effort = \"high\"\n[tui]\nshow_tooltips = false\n"
        );
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
        std::fs::write(child.join("config.toml"), "model = \"x\"\n").unwrap();
        assert!(sync_one(Provider::Codex, &p, &c).is_err(), "no file to copy");
        assert_eq!(std::fs::read_to_string(child.join("config.toml")).unwrap(), "model = \"x\"\n");
        // Blank is no JSON.
        std::fs::write(parent.join("settings.json"), "\n").unwrap();
        assert!(sync_one(Provider::Claude, &p, &c).is_err());
        let _ = std::fs::remove_dir_all(&parent);
        let _ = std::fs::remove_dir_all(&child);
    }

    #[test]
    fn a_sync_under_way_is_not_overtaken_by_a_change_to_parent_and_child() {
        let (parent, child) = (dir("stale-parent"), dir("stale-child"));
        let (p, c) = (parent.to_string_lossy().into_owned(), child.to_string_lossy().into_owned());
        std::fs::write(parent.join("settings.json"), r#"{"disableAllHooks":true}"#).unwrap();
        std::fs::write(child.join("settings.json"), r#"{"disableAllHooks":true}"#).unwrap();

        let (done, came) = std::sync::mpsc::channel();
        let accounts = [p.clone(), c.clone()];
        let mut change = None;
        // The moment a stale read could go wrong: the parent read, the child
        // not yet written, and a change to both on its way.
        sync_one_seeing(Provider::Claude, &p, &c, || {
            change = Some(std::thread::spawn(move || {
                let off = |d: &Doc| d.bool_at(&["disableAllHooks"]) == Some(true);
                let wrote = edit_together(KeyFile::ClaudeSettings, &accounts, &accounts, &[Edit::Remove(&["disableAllHooks"])], &off);
                done.send(()).unwrap();
                wrote
            }));
            assert!(came.recv_timeout(std::time::Duration::from_millis(200)).is_err(), "it went through in the middle of the sync");
        })
        .unwrap();
        // The sync has written what it read; the change comes after it, whole.
        assert_eq!(change.unwrap().join().unwrap().unwrap(), 2);
        for d in [&parent, &child] {
            let v: Value = serde_json::from_str(&std::fs::read_to_string(d.join("settings.json")).unwrap()).unwrap();
            assert_eq!(v, json!({}));
        }
        let _ = std::fs::remove_dir_all(&parent);
        let _ = std::fs::remove_dir_all(&child);
    }

    #[test]
    fn a_blank_codex_parent_empties_its_children_but_for_their_own_folders() {
        use std::fs::TryLockError;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        use std::time::{Duration, Instant};
        let (parent, child) = (dir("blank-parent"), dir("blank-child"));
        let (p, c) = (parent.to_string_lossy().into_owned(), child.to_string_lossy().into_owned());
        std::fs::write(parent.join("config.toml"), "\n").unwrap();
        std::fs::write(
            child.join("config.toml"),
            "model = \"x\"\n[features]\nhooks = false\n[projects.'C:\\\\c']\ntrust_level = \"trusted\"\n",
        )
        .unwrap();
        sync_one(Provider::Codex, &p, &c).unwrap();
        let doc: DocumentMut = std::fs::read_to_string(child.join("config.toml")).unwrap().parse().unwrap();
        assert!(doc.get("model").is_none() && doc.get("features").is_none(), "{doc}");
        assert!(doc.get("projects").is_some(), "{doc}");

        // An editor's save caught half way: the look that follows gets what it
        // wrote. The sync takes its lock, looks, and finding the file blank
        // looks four more times 100 ms apart, so a save done within 300 ms of
        // the lock last being free comes before its last look. The save waits
        // until the sync holds the lock, a file of this test's own, and lands
        // 50 ms later, past the look that follows the lock at once. A save held
        // up longer, its thread kept waiting on a busy machine, may have missed
        // every look and proves nothing: that round goes again.
        let locks = dir("blank-lock");
        let lock = locks.join("account-settings.lock");
        let probe = Arc::new(std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&lock).unwrap());
        LOCK_AT.with(|l| *l.borrow_mut() = Some(lock));
        let saved = "model = \"saved\"\n";
        let since = Instant::now();
        loop {
            std::fs::write(parent.join("config.toml"), "").unwrap();
            std::fs::write(child.join("config.toml"), "model = \"x\"\n").unwrap();
            let synced = Arc::new(AtomicBool::new(false));
            let (path, probe, done, mut free) = (parent.join("config.toml"), probe.clone(), synced.clone(), Instant::now());
            let saving = std::thread::spawn(move || {
                loop {
                    let now = Instant::now();
                    match probe.try_lock() {
                        Ok(()) => {
                            probe.unlock().unwrap();
                            free = now;
                        }
                        Err(TryLockError::WouldBlock) => break,
                        Err(TryLockError::Error(e)) => panic!("{e}"),
                    }
                    if done.load(Ordering::SeqCst) {
                        return None;
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                std::thread::sleep(Duration::from_millis(50));
                std::fs::write(path, saved).unwrap();
                Some(free.elapsed())
            });
            sync_one(Provider::Codex, &p, &c).unwrap();
            synced.store(true, Ordering::SeqCst);
            if saving.join().unwrap().is_some_and(|after| after < Duration::from_millis(300)) {
                break;
            }
            assert!(since.elapsed() < Duration::from_secs(30), "no save came in time for the sync's looks in 30 s");
        }
        assert_eq!(std::fs::read_to_string(child.join("config.toml")).unwrap(), saved);
        LOCK_AT.with(|l| *l.borrow_mut() = None);
        drop(probe);
        let _ = std::fs::remove_dir_all(&parent);
        let _ = std::fs::remove_dir_all(&child);
        let _ = std::fs::remove_dir_all(&locks);
    }
}
