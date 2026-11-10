//! The library its tests lay out: a folder of its own with three accounts in it,
//! and the reaches they are given.

use super::*;
use crate::settings::FIRST_INSTRUCTIONS;

/// The temp folder as the disk spells it. A runner's TEMP can be an 8.3 short
/// name (`C:\Users\RUNNER~1\…`), and a session given a folder under it is told
/// of both its spellings (pty/start.rs `added_dir`): a golden written for one
/// would meet the other unredacted. Without Windows' `\\?\` prefix, which
/// canonicalizing adds and no path a CLI reports carries.
fn temp_as_on_disk() -> PathBuf {
    let temp = std::env::temp_dir();
    let Ok(full) = std::fs::canonicalize(&temp) else { return temp };
    let text = full.to_string_lossy().into_owned();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with(r"UNC\") => PathBuf::from(rest),
        _ => full,
    }
}

pub(super) struct Fixture {
    pub(super) base: PathBuf,
    pub(super) dirs: Dirs,
    pub(super) accounts: Vec<AccountInfo>,
}

impl Fixture {
    pub(super) fn new(tag: &str) -> Fixture {
        let base = temp_as_on_disk().join(format!("luna-library-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let dirs = Dirs { root: base.join("library"), data: base.join("data") };
        let accounts =
            [(Provider::Claude, "anthropic", "work"), (Provider::Codex, "openai", "home"), (Provider::Claude, "anthropic", "home")]
                .into_iter()
                .map(|(provider, vendor, name)| {
                    let path = base.join("accounts").join(vendor).join(name);
                    std::fs::create_dir_all(&path).unwrap();
                    AccountInfo { provider, name: name.into(), path: path.to_string_lossy().into_owned() }
                })
                .collect();
        std::fs::create_dir_all(&dirs.root).unwrap();
        Fixture { base, dirs, accounts }
    }
    pub(super) fn skill(&self, name: &str, body: &str) {
        let text = format!("---\nname: {name}\ndescription: Does {name}.\n---\n{body}\n");
        write_file(&self.dirs.skills().join(name).join(SKILL_FILE), text.as_bytes()).unwrap();
    }
    pub(super) fn instructions(&self, name: &str, text: &str) {
        write_file(&self.dirs.instructions_file(name), text.as_bytes()).unwrap();
    }
    pub(super) fn account(&self, i: usize) -> PathBuf {
        PathBuf::from(&self.accounts[i].path)
    }
    pub(super) fn read(&self, i: usize, file: &str) -> String {
        std::fs::read_to_string(self.account(i).join(file)).unwrap_or_default()
    }
    pub(super) fn run(&self, lib: &LibrarySettings, ledger: &mut Ledger, act: bool) -> Pass {
        let skills = read_skills(&self.dirs);
        let instructions = read_instructions(&self.dirs);
        let units = units(&self.dirs, lib, &self.accounts, &skills, &instructions, ledger);
        pass(&self.dirs, &self.accounts, &units, &lib.skipped, ledger, act)
    }
    pub(super) fn status(p: &Pass, dest: &Path) -> Status {
        // By components: a test's `join("a/b")` keeps its slash on Windows.
        p.places.iter().find(|x| Path::new(&x.dest) == dest).map(|x| x.status).expect("a place for it")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

/// A folder link at `link` to `target`: a symlink on Unix, a junction on
/// Windows (no privilege needed), as a person or a program would plant one.
pub(super) fn link_dir(link: &Path, target: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
    #[cfg(windows)]
    assert!(std::process::Command::new("cmd")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .is_ok_and(|o| o.status.success()));
}

/// The link itself, never what it leads to.
pub(super) fn unlink_dir(link: &Path) {
    let _ = std::fs::remove_dir(link).or_else(|_| std::fs::remove_file(link));
}

pub(super) fn all() -> Reach {
    vec![ALL.into()]
}

/// These skills, each into every account.
pub(super) fn skills(names: &[&str]) -> BTreeMap<String, Reach> {
    names.iter().map(|n| (n.to_string(), all())).collect()
}

/// The first instructions, `main`, with this reach.
pub(super) fn first(reach: Reach) -> BTreeMap<String, Reach> {
    BTreeMap::from([(FIRST_INSTRUCTIONS.to_string(), reach)])
}

pub(super) fn reach(r: &[&str]) -> Reach {
    r.iter().map(|s| s.to_string()).collect()
}

/// A library with one folder in it, for the launch golden tests (pty/start.rs,
/// background/command.rs): a repository whose sessions get `notes` from it and
/// not `review`, which their accounts have as a whole, and on Claude Code its
/// `terse` instructions in place of the account's. While it is held,
/// `session_args` reads it for its accounts (`held`).
pub(crate) struct GoldenLibrary {
    f: Fixture,
    /// The repository.
    pub(crate) folder: String,
}

/// The libraries tests hold, with the accounts each has.
static HELD: Mutex<Vec<(Dirs, LibrarySettings, Vec<AccountInfo>)>> = Mutex::new(Vec::new());

impl GoldenLibrary {
    /// What its `terse` instructions say.
    pub(crate) const INSTRUCTIONS: &'static str = "# Terse\n\nSay less.\n";
    /// The name its folder's skills wait under.
    pub(crate) const SLUG: &'static str = "golden";

    pub(crate) fn new(tag: &str) -> Self {
        let f = Fixture::new(&format!("golden-{tag}"));
        f.skill("review", "v1");
        f.skill("notes", "v1");
        f.instructions("terse", Self::INSTRUCTIONS);
        let repo = f.base.join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let folder = LibraryFolder {
            path: repo.to_string_lossy().into_owned(),
            skills: BTreeMap::from([("notes".into(), all()), ("review".into(), Vec::new())]),
            instructions: BTreeMap::from([("claude".into(), Some("terse".into()))]),
            slug: Self::SLUG.into(),
        };
        let lib = LibrarySettings { skills: skills(&["review"]), folders: vec![folder], ..Default::default() };
        f.run(&lib, &mut Ledger::default(), true);
        codex_manifests(&lib, &f.accounts);
        let dirs = Dirs { root: f.dirs.root.clone(), data: f.dirs.data.clone() };
        HELD.lock().unwrap_or_else(|e| e.into_inner()).push((dirs, lib, f.accounts.clone()));
        GoldenLibrary { folder: repo.to_string_lossy().into_owned(), f }
    }

    /// A library whose entries are all the person's chats' (audience.rs): its
    /// `main` instructions and `report` skill, into every account. Its folder
    /// is a repository the library says nothing of.
    pub(crate) fn chats_only(tag: &str) -> Self {
        let f = Fixture::new(&format!("golden-{tag}"));
        f.skill("report", "v1");
        f.instructions(FIRST_INSTRUCTIONS, "# Me\n\nSpeak Russian to me.\n");
        let repo = f.base.join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let mut lib = LibrarySettings { instructions: first(all()), skills: skills(&["report"]), ..Default::default() };
        lib.audience.instructions.insert(FIRST_INSTRUCTIONS.into(), Audience::Chats);
        lib.audience.skills.insert("report".into(), Audience::Chats);
        f.run(&lib, &mut Ledger::default(), true);
        let dirs = Dirs { root: f.dirs.root.clone(), data: f.dirs.data.clone() };
        HELD.lock().unwrap_or_else(|e| e.into_inner()).push((dirs, lib, f.accounts.clone()));
        GoldenLibrary { folder: repo.to_string_lossy().into_owned(), f }
    }

    /// Its Claude Code account, `claude/work`.
    pub(crate) fn claude(&self) -> String {
        self.f.accounts[0].path.clone()
    }

    /// Its Codex account.
    pub(crate) fn codex(&self) -> String {
        self.f.accounts[1].path.clone()
    }

    /// Where the folder's skills wait for the Claude Code account's sessions.
    pub(crate) fn claude_skills(&self) -> String {
        claude_folder_dir(&self.f.dirs, Self::SLUG, &self.f.accounts[0]).to_string_lossy().into_owned()
    }
}

impl Drop for GoldenLibrary {
    fn drop(&mut self) {
        HELD.lock().unwrap_or_else(|e| e.into_inner()).retain(|(dirs, _, _)| dirs.root != self.f.dirs.root);
    }
}

/// The library a test holds for this account, if one does.
pub(super) fn held(provider: Provider, account_path: &str) -> Option<(Dirs, LibrarySettings, AccountInfo)> {
    let held = HELD.lock().unwrap_or_else(|e| e.into_inner());
    held.iter().find_map(|(dirs, lib, accounts)| {
        let a = accounts.iter().find(|a| a.provider == provider && a.path == account_path)?;
        Some((Dirs { root: dirs.root.clone(), data: dirs.data.clone() }, lib.clone(), a.clone()))
    })
}
