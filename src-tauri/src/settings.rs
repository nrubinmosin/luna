//! The settings the Rust side has to know on its own, such as where the
//! account folders live. They sit in `<data>/settings.json` rather than
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
    /// The last action armed, so the menu opens on it: shutdown | restart | hibernate | sleep.
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
    /// Which browser a terminal's links open in (browsers.rs).
    pub browsers: crate::browsers::BrowserSettings,
    /// A chat whose last message ends on a line holding this text is waiting
    /// for its user (asking.rs).
    pub waiting_marker_on: bool,
    pub waiting_marker: String,
    /// Recommendations (checks.rs) left off on purpose: no chip for these.
    pub recommendations_dismissed: Vec<String>,
}

/// Which accounts something goes into: `all`, a provider's every account
/// (`claude`, `codex`) or one account (`provider/name`) per entry.
pub type Reach = Vec<String>;

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase", from = "LibrarySettingsFile")]
pub struct LibrarySettings {
    /// Absolute path of the library's folder; None means `<data>/library`.
    pub source: Option<String>,
    /// The accounts each instructions file goes into, by its name. An
    /// account gets one: the one whose reach names it most closely.
    pub instructions: std::collections::BTreeMap<String, Reach>,
    /// Accounts that get no instructions, even where another reach takes
    /// them in less closely.
    pub no_instructions: Reach,
    /// The accounts each skill goes into, by the skill's name: wherever no
    /// folder says otherwise. One no account gets is not here.
    pub skills: std::collections::BTreeMap<String, Reach>,
    /// Folders the user named, each saying for its sessions which accounts
    /// get a skill there.
    pub folders: Vec<LibraryFolder>,
    /// Destinations left out on purpose, by path.
    pub skipped: Vec<String>,
}

/// The library's settings as the file may hold them: before a reach, the
/// instructions were on or off for every account and `everywhere` named the
/// skills that went into every account; before there could be several
/// instructions, the one file had a reach of its own — now `main`'s. Before
/// a folder gave each skill a reach of its own, it ticked skills (`skills`)
/// or turned them off (`off`), and `folders_only` named the skills a tick
/// gave only to their own accounts — into none of them as a whole.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct LibrarySettingsFile {
    source: Option<String>,
    instructions: Option<InstructionsField>,
    no_instructions: Reach,
    skills: std::collections::BTreeMap<String, Reach>,
    everywhere: Vec<String>,
    folders: Vec<LibraryFolderFile>,
    folders_only: Vec<String>,
    skipped: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct LibraryFolderFile {
    path: String,
    skills: Option<FolderSkillsField>,
    off: Vec<String>,
    instructions: std::collections::BTreeMap<String, Option<String>>,
    slug: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum FolderSkillsField {
    Ticked(Vec<String>),
    ByName(std::collections::BTreeMap<String, Reach>),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum InstructionsField {
    On(bool),
    Reach(Reach),
    ByName(std::collections::BTreeMap<String, Reach>),
}

/// The name the one instructions file of old goes by in the library
/// (library.rs moves the file to match).
pub const FIRST_INSTRUCTIONS: &str = "main";

impl From<LibrarySettingsFile> for LibrarySettings {
    fn from(f: LibrarySettingsFile) -> Self {
        let mut skills = f.skills;
        for name in f.everywhere {
            skills.entry(name).or_insert_with(|| vec!["all".into()]);
        }
        let first = |reach: Reach| std::collections::BTreeMap::from([(FIRST_INSTRUCTIONS.to_string(), reach)]);
        // A tick gave a skill to every account's sessions there, or — folders
        // only — to its own accounts' alone, which then had it nowhere else;
        // off kept it from all of them.
        let folders: Vec<LibraryFolder> = f
            .folders
            .into_iter()
            .map(|folder| {
                // A folder of before that ticked nothing could leave its
                // `skills` out and still turn some off.
                let skills = match folder.skills.unwrap_or(FolderSkillsField::Ticked(Vec::new())) {
                    FolderSkillsField::ByName(m) => m,
                    FolderSkillsField::Ticked(ticked) => {
                        let mut m: std::collections::BTreeMap<String, Reach> = folder.off.into_iter().map(|n| (n, Vec::new())).collect();
                        for name in ticked {
                            let reach = match f.folders_only.contains(&name) {
                                true => skills.get(&name).cloned().unwrap_or_default(),
                                false => vec!["all".into()],
                            };
                            m.insert(name, reach);
                        }
                        m
                    }
                };
                LibraryFolder { path: folder.path, skills, instructions: folder.instructions, slug: folder.slug }
            })
            .collect();
        for name in &f.folders_only {
            skills.remove(name);
        }
        LibrarySettings {
            source: f.source,
            instructions: match f.instructions {
                Some(InstructionsField::On(true)) => first(vec!["all".into()]),
                Some(InstructionsField::On(false)) | None => Default::default(),
                Some(InstructionsField::Reach(r)) if r.is_empty() => Default::default(),
                Some(InstructionsField::Reach(r)) => first(r),
                Some(InstructionsField::ByName(m)) => m,
            },
            no_instructions: f.no_instructions,
            skills,
            folders,
            skipped: f.skipped,
        }
    }
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct LibraryFolder {
    pub path: String,
    /// The accounts whose sessions here get a skill, by its name, in place
    /// of what it is meant for and of what a folder this one lies in says
    /// (library.rs `deciding`); empty, no account's. A skill not named here
    /// goes by those.
    pub skills: std::collections::BTreeMap<String, Reach>,
    /// The instructions its sessions get in place of their account's, by
    /// the reach entry they are for (an account, as a rule): one of the
    /// library's by name, or None for none at all. The closest entry decides
    /// for an account, the deepest folder with one for it before that
    /// (library.rs `folder_instructions`); an account none takes in goes by
    /// its own.
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub instructions: std::collections::BTreeMap<String, Option<String>>,
    /// The name its skills wait under (library.rs `folder_slugs`), given once
    /// and kept: one read off its place in the list would pass to another
    /// folder as an earlier one goes.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub slug: String,
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
            browsers: Default::default(),
            waiting_marker_on: true,
            waiting_marker: "AWAITING OPERATOR".into(),
            recommendations_dismissed: Vec::new(),
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

/// `get` for what must not act on a guess: an error while the file does not
/// read, rather than what was read last or — in a process that has read
/// nothing yet — the defaults. A library pass on those would take out every
/// copy the real settings want.
pub fn get_checked() -> Result<Settings, String> {
    let path = file();
    let now = stamp(&path);
    let s = read(&path)?;
    if let Ok(mut c) = cache().lock() {
        *c = (s.clone(), now);
    }
    Ok(s)
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
        assert_eq!(old.library.instructions[FIRST_INSTRUCTIONS], vec!["all".to_string()]);
        assert_eq!(old.library.skills["review"], vec!["all".to_string()]);
        assert_eq!(old.library.skipped, vec!["x".to_string()]);
        let off: Settings = serde_json::from_str(r#"{"library": {"instructions": false}}"#).unwrap();
        assert!(off.library.instructions.is_empty());

        // Written back, it reads the same.
        let mut s = Settings::default();
        s.library.instructions.insert("main".into(), vec!["claude".into(), "codex/home".into()]);
        s.library.no_instructions = vec!["codex/work".into()];
        s.library.skills.insert("review".into(), vec!["claude/work".into()]);
        s.library.folders.push(LibraryFolder {
            path: "/w/app".into(),
            skills: [("review".to_string(), vec!["codex".to_string()]), ("deploy".to_string(), vec![])].into(),
            instructions: [("claude/work".to_string(), None), ("claude".to_string(), Some("terse".to_string()))].into(),
            slug: "app".into(),
        });
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.library.instructions, s.library.instructions);
        assert_eq!(back.library.no_instructions, s.library.no_instructions);
        assert_eq!(back.library.skills, s.library.skills);
        assert_eq!(back.library.folders[0].skills, s.library.folders[0].skills);
        assert_eq!(back.library.folders[0].instructions, s.library.folders[0].instructions);
        assert_eq!(back.library.folders[0].slug, "app");
    }

    #[test]
    fn folders_that_ticked_skills_read_as_each_skills_accounts_there() {
        let old: Settings = serde_json::from_str(
            r#"{"library": {
                "skills": {"review": ["claude"], "deploy": ["codex/work"], "lint": ["all"]},
                "foldersOnly": ["deploy"],
                "folders": [
                    {"path": "/w/app", "skills": ["review", "deploy"], "off": ["lint"], "slug": "app"},
                    {"path": "/w/app/web", "skills": ["lint"], "off": []},
                    {"path": "/w/lib", "off": ["review"]}
                ]
            }}"#,
        )
        .unwrap();
        let lib = &old.library;
        // Or: its accounts everywhere, and every account's sessions where ticked.
        assert_eq!(lib.skills["review"], vec!["claude".to_string()]);
        assert_eq!(lib.folders[0].skills["review"], vec!["all".to_string()]);
        // And: nowhere as a whole, and its own accounts' sessions where ticked.
        assert!(!lib.skills.contains_key("deploy"));
        assert_eq!(lib.folders[0].skills["deploy"], vec!["codex/work".to_string()]);
        // Off: no account's sessions; a tick inside takes it back for all.
        assert_eq!(lib.folders[0].skills["lint"], Vec::<String>::new());
        assert_eq!(lib.folders[1].skills["lint"], vec!["all".to_string()]);
        assert_eq!(lib.folders[0].slug, "app");
        assert_eq!(lib.folders[1].slug, "");
        assert_eq!(lib.folders[2].skills["review"], Vec::<String>::new(), "off with nothing ticked");
    }
}
