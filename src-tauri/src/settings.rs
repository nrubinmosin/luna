//! The few settings the Rust side has to know on its own — today just where
//! the account folders live. They sit in `<data>/settings.json` rather than
//! the webview's localStorage because background threads (models.rs) need
//! them before, and without, any window asking.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Absolute path; None means the default (`Documents/luna-accounts`).
    pub accounts_root: Option<String>,
    /// Hold the machine awake while any session is busy (power.rs).
    pub keep_awake: bool,
    /// How long everything has to be quiet before an armed power-off counts down.
    pub quiet_window_s: u64,
    /// The last action armed, so the menu opens on it: shutdown | hibernate | sleep.
    pub power_action: String,
    /// `provider/name` of accounts agents may not spawn on (agents.rs). Every
    /// account is available unless listed here.
    pub agent_blocked_accounts: Vec<String>,
    /// Per provider, the account whose settings file the others copy
    /// (account_settings.rs), by name.
    pub settings_parents: SettingsParents,
    /// Copy them once at every launch.
    pub settings_sync_on_start: bool,
    /// `git fetch` origin before a new worktree branches off it (worktree.rs).
    pub fetch_before_worktree: bool,
    /// Codex sessions run a project's hooks without asking for trust first
    /// (codex/hooks.rs): the default, and the folders kept otherwise.
    pub codex_hooks_unasked: bool,
    pub codex_hooks_unasked_in: std::collections::BTreeMap<String, bool>,
    /// Folders whose projects Codex sessions start trusted on this machine,
    /// without an entry in any account's config.toml (codex/trust.rs).
    pub codex_trusted_roots: Vec<String>,
    /// The user's skills and instructions, and where they are laid out
    /// (library.rs).
    pub library: LibrarySettings,
}

/// Which accounts something goes into: `all`, a provider's every account
/// (`claude`, `codex`) or one account (`provider/name`) per entry.
pub type Reach = Vec<String>;

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase", from = "LibrarySettingsFile")]
pub struct LibrarySettings {
    /// Absolute path of the library's folder; None means `<data>/library`.
    pub source: Option<String>,
    /// The accounts the library's instructions go into.
    pub instructions: Reach,
    /// The accounts each skill goes into, by the skill's name.
    pub skills: std::collections::BTreeMap<String, Reach>,
    /// Folders the user named, each with the skills that go into it.
    pub folders: Vec<LibraryFolder>,
    /// Destinations left out on purpose, by path.
    pub skipped: Vec<String>,
}

/// The library's settings as the file may hold them: before a reach, the
/// instructions were on or off for every account and `everywhere` named the
/// skills that went into every account.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct LibrarySettingsFile {
    source: Option<String>,
    instructions: Option<InstructionsField>,
    skills: std::collections::BTreeMap<String, Reach>,
    everywhere: Vec<String>,
    folders: Vec<LibraryFolder>,
    skipped: Vec<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum InstructionsField {
    On(bool),
    Reach(Reach),
}

impl From<LibrarySettingsFile> for LibrarySettings {
    fn from(f: LibrarySettingsFile) -> Self {
        let mut skills = f.skills;
        for name in f.everywhere {
            skills.entry(name).or_insert_with(|| vec!["all".into()]);
        }
        LibrarySettings {
            source: f.source,
            instructions: match f.instructions {
                Some(InstructionsField::On(true)) => vec!["all".into()],
                Some(InstructionsField::On(false)) | None => Vec::new(),
                Some(InstructionsField::Reach(r)) => r,
            },
            skills,
            folders: f.folders,
            skipped: f.skipped,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct LibraryFolder {
    pub path: String,
    pub skills: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct SettingsParents {
    pub claude: Option<String>,
    pub codex: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            accounts_root: None,
            keep_awake: true,
            quiet_window_s: 120,
            power_action: "shutdown".into(),
            agent_blocked_accounts: Vec::new(),
            settings_parents: SettingsParents::default(),
            settings_sync_on_start: false,
            fetch_before_worktree: true,
            codex_hooks_unasked: false,
            codex_hooks_unasked_in: Default::default(),
            codex_trusted_roots: Vec::new(),
            library: LibrarySettings::default(),
        }
    }
}

fn file() -> PathBuf {
    crate::paths::data_dir().join("settings.json")
}

/// What the file was when it was last read: a write from elsewhere moves one
/// or the other.
type Stamp = Option<(std::time::SystemTime, u64)>;

fn stamp(path: &std::path::Path) -> Stamp {
    std::fs::metadata(path).ok().and_then(|m| Some((m.modified().ok()?, m.len())))
}

/// The file's settings; the defaults only when there is no file. One that
/// will not read or parse is an error: taken for the defaults, it would have
/// the library take every copy out and the next write throw it all away.
fn read(path: &std::path::Path) -> Result<Settings, String> {
    match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn cache() -> &'static Mutex<(Settings, Stamp)> {
    static S: OnceLock<Mutex<(Settings, Stamp)>> = OnceLock::new();
    S.get_or_init(|| Mutex::new((Settings::default(), None)))
}

/// The settings as the file has them. Another process may write the file
/// too, so the copy in memory is only trusted while the file is unchanged —
/// one `stat` per read.
pub fn get() -> Settings {
    let path = file();
    let now = stamp(&path);
    let Ok(mut c) = cache().lock() else { return read(&path).unwrap_or_default() };
    if c.1 != now || now.is_none() {
        // A file that will not read keeps what was read last, its stamp
        // unchanged so the next call reads again.
        if let Ok(s) = read(&path) {
            *c = (s, now);
        }
    }
    c.0.clone()
}

/// Changes one or more fields and writes the file. Under a lock file, and
/// from what is on disk at that moment rather than from memory: two
/// processes each writing back their own stale copy would undo each other.
pub fn update(f: impl FnOnce(&mut Settings)) -> Result<(), String> {
    let path = file();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.with_extension("json.lock"))
        .map_err(|e| format!("settings lock: {e}"))?;
    lock.lock().map_err(|e| format!("settings lock: {e}"))?;

    let mut s = read(&path)?;
    f(&mut s);
    let raw = serde_json::to_string_pretty(&s).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, raw).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("write {}: {e}", path.display()))?;
    if let Ok(mut c) = cache().lock() {
        *c = (s, stamp(&path));
    }
    Ok(())
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AccountsRootInfo {
    pub path: String,
    /// True when no override is set and `path` is the default.
    pub is_default: bool,
}

fn info() -> Result<AccountsRootInfo, String> {
    let s = get();
    Ok(AccountsRootInfo {
        path: crate::accounts::accounts_root()?.to_string_lossy().into_owned(),
        is_default: s.accounts_root.is_none(),
    })
}

#[tauri::command]
pub fn get_accounts_root() -> Result<AccountsRootInfo, String> {
    info()
}

/// Points the account list at another directory — created if missing. Nothing
/// is moved: the accounts that were under the old root stay there, and the
/// list shows whatever folders the new one holds. Empty string = back to the
/// default.
#[tauri::command]
pub fn set_accounts_root(path: String) -> Result<AccountsRootInfo, String> {
    let trimmed = path.trim();
    let root = if trimmed.is_empty() {
        None
    } else {
        let p = PathBuf::from(trimmed);
        if !p.is_absolute() {
            return Err("Accounts folder must be an absolute path".into());
        }
        std::fs::create_dir_all(&p).map_err(|e| format!("cannot create {}: {e}", p.display()))?;
        Some(p.to_string_lossy().into_owned())
    };
    crate::log::info("settings", &format!("accounts root: {}", root.as_deref().unwrap_or("<default>")));
    update(|s| s.accounts_root = root)?;
    info()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_update_starts_from_the_file_not_from_memory() {
        let dir = std::env::temp_dir().join(format!("luna-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, r#"{"quietWindowS": 300}"#).unwrap();
        assert_eq!(read(&path).unwrap().quiet_window_s, 300);
        assert!(read(&path).unwrap().keep_awake, "missing fields take their defaults");

        // Another process rewrites the file: the stamp moves, so a cached
        // copy is not trusted past it.
        let before = stamp(&path);
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&path, r#"{"quietWindowS": 60, "keepAwake": false}"#).unwrap();
        assert_ne!(stamp(&path), before);
        assert_eq!(read(&path).unwrap().quiet_window_s, 60);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_library_before_a_reach_reads_as_every_account() {
        let old: Settings = serde_json::from_str(r#"{"library": {"instructions": true, "everywhere": ["review"], "skipped": ["x"]}}"#).unwrap();
        assert_eq!(old.library.instructions, vec!["all".to_string()]);
        assert_eq!(old.library.skills["review"], vec!["all".to_string()]);
        assert_eq!(old.library.skipped, vec!["x".to_string()]);
        let off: Settings = serde_json::from_str(r#"{"library": {"instructions": false}}"#).unwrap();
        assert!(off.library.instructions.is_empty());

        // Written back, it reads the same.
        let mut s = Settings::default();
        s.library.instructions = vec!["claude".into(), "codex/home".into()];
        s.library.skills.insert("review".into(), vec!["claude/work".into()]);
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.library.instructions, s.library.instructions);
        assert_eq!(back.library.skills, s.library.skills);
    }
}
