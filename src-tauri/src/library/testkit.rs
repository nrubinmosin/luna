//! The library its tests lay out: a folder of its own with three accounts in it,
//! and the reaches they are given.

use super::*;
use crate::settings::FIRST_INSTRUCTIONS;

pub(super) struct Fixture {
    pub(super) base: PathBuf,
    pub(super) dirs: Dirs,
    pub(super) accounts: Vec<AccountInfo>,
}

impl Fixture {
    pub(super) fn new(tag: &str) -> Fixture {
        let base = std::env::temp_dir().join(format!("luna-library-{tag}-{}", std::process::id()));
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
        pass(&self.dirs, &units, &lib.skipped, ledger, act)
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
