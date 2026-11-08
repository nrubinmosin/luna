//! What the window is shown of the library: its skills, instructions and folders,
//! and where each copy stands, from a pass that lays out or only looks.

use super::*;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderInfo {
    pub path: String,
    /// The accounts whose sessions here get a skill, by the skills it says
    /// anything of.
    pub skills: BTreeMap<String, Reach>,
    /// The instructions its sessions get in place of their account's, by the
    /// reach entry they are for; None: none at all.
    pub instructions: BTreeMap<String, Option<String>>,
    pub found: bool,
    /// The accounts, as `provider/name`, whose sessions here would get each
    /// skill were the folder to say nothing of it (`inherited`); a skill no
    /// account's would get is not there.
    pub inherited: BTreeMap<String, Vec<String>>,
}

/// The accounts whose sessions in a folder would get a skill were it to say
/// nothing of it — by what the skill is meant for, or by the deepest folder
/// it lies in that says something.
pub(super) fn inherited(lib: &LibrarySettings, accounts: &[AccountInfo], f: &LibraryFolder, name: &str) -> Vec<String> {
    let outer: Vec<&LibraryFolder> = lib
        .folders
        .iter()
        .filter(|g| ident(Path::new(&g.path)) != ident(Path::new(&f.path)) && within(Path::new(&f.path), Path::new(&g.path)))
        .collect();
    accounts
        .iter()
        .filter(|a| gets_in(lib, outer.iter().copied(), name, a))
        .map(|a| format!("{}/{}", a.provider.as_str(), a.name))
        .collect()
}

/// An account's instructions file Luna did not write, where no instructions of
/// the library go: something to start new instructions from.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FoundFile {
    pub dest: String,
    pub target: Target,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    pub root: String,
    pub is_default: bool,
    /// False while the library's folder is not there: nothing is laid out,
    /// and nothing taken out, until it is.
    pub found: bool,
    pub instructions: Vec<InstructionsInfo>,
    /// The accounts that get no instructions however others reach them.
    pub no_instructions: Reach,
    /// The instructions each account gets, by `provider/name`; an account
    /// that gets none is not there.
    pub instructions_for: BTreeMap<String, String>,
    /// The `instructions.md` of before there could be several, still in the
    /// library beside `instructions/main.md`: nothing is laid out from it.
    pub old_instructions: Option<String>,
    pub skills: Vec<SkillInfo>,
    /// Every account, to pick a reach from.
    pub accounts: Vec<AccountInfo>,
    pub folders: Vec<FolderInfo>,
    pub places: Vec<Place>,
    pub found_instructions: Vec<FoundFile>,
    /// Codex accounts with an `AGENTS.override.md`, which Codex reads instead
    /// of the `AGENTS.md` Luna writes.
    pub overridden: Vec<String>,
    pub kept: Vec<String>,
}

/// Whether these kept copies are not what the log said last: a pass comes
/// before every session's start, and each list once is enough.
/// Whether a link `codex_manifests` keeps clear of is yet to be said: once a
/// run, as a pass comes with every session's start.
pub(super) fn link_is_news(link: &Path) -> bool {
    static SAID: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());
    SAID.lock().unwrap_or_else(|e| e.into_inner()).insert(link.to_path_buf())
}

fn kept_is_news(kept: &[String]) -> bool {
    static SAID: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let mut said = SAID.lock().unwrap_or_else(|e| e.into_inner());
    if *said == kept {
        return false;
    }
    *said = kept.to_vec();
    true
}

pub(super) fn overview(act: bool) -> Result<Overview, String> {
    let _guard = lock();
    overview_locked(act)
}

pub(super) fn overview_locked(act: bool) -> Result<Overview, String> {
    let dirs = Dirs::current();
    // A pass goes by settings as the file has them now, or not at all.
    let mut lib = if act { crate::settings::get_checked()?.library } else { crate::settings::get().library };
    // Folders listed before they kept their names keep the ones they have
    // gone by, which is what is on disk.
    if act && lib.folders.iter().any(|f| f.slug.is_empty()) {
        crate::settings::update(|s| settle_slugs(&mut s.library))?;
        settle_slugs(&mut lib);
    }
    let accounts = crate::accounts::list_accounts()?;
    let found = dirs.root.is_dir();
    if act && found {
        migrate_instructions(&dirs);
    }
    let skills = if found { read_skills(&dirs) } else { Ok(BTreeMap::new()) };
    let instructions = if found { read_instructions(&dirs) } else { Ok(BTreeMap::new()) };
    let mut ledger = Ledger::load(&dirs);
    // A library that is not there (a drive not plugged in, a folder moved)
    // wants nothing, and a pass on that would take every copy out.
    let units = if found { units(&dirs, &lib, &accounts, &skills, &instructions, &ledger) } else { Vec::new() };
    let p = pass(&dirs, &units, &lib.skipped, &mut ledger, act && found);
    if act && found {
        codex_manifests(&lib, &accounts);
        prune_folder_dirs(&dirs, &lib);
        ledger.save(&dirs)?;
        sweep_blobs(&dirs, &ledger);
        sweep_backups(&dirs);
        if !p.kept.is_empty() && kept_is_news(&p.kept) {
            crate::log::info("library", &format!("left in place, changed since Luna wrote them: {}", p.kept.join(", ")));
        }
    }
    // Where instructions do go, a file Luna did not write is the place's own
    // business (`taken`), settled in place.
    let found_instructions = accounts
        .iter()
        .map(|a| (a, Path::new(&a.path).join(instructions_name(a.provider))))
        .filter(|(_, dest)| dest.is_file() && !ledger.written.contains_key(&*dest.to_string_lossy()))
        .filter(|(_, dest)| !units.iter().any(|u| ident(&u.dest) == ident(dest)))
        .map(|(a, dest)| FoundFile {
            dest: dest.to_string_lossy().into_owned(),
            target: Target::Account { provider: a.provider, name: a.name.clone() },
        })
        .collect();
    let have = |name: &str| instructions.as_ref().map_or(true, |found| found.contains_key(name));
    let instructions_for =
        accounts.iter().filter_map(|a| Some((format!("{}/{}", a.provider.as_str(), a.name), chosen(&lib, have, a)?.to_string()))).collect();
    let old_instructions = dirs.root.join(OLD_INSTRUCTIONS);
    let skills: Vec<SkillInfo> = skills
        .unwrap_or_default()
        .into_values()
        .map(|(info, _)| SkillInfo {
            accounts: lib.skills.get(&info.name).cloned().unwrap_or_default(),
            audience: skill_audience(&lib, &info.name),
            ..info
        })
        .collect();
    let folders = lib
        .folders
        .iter()
        .map(|f| FolderInfo {
            found: Path::new(&f.path).is_dir(),
            inherited: skills
                .iter()
                .map(|s| (s.name.clone(), inherited(&lib, &accounts, f, &s.name)))
                .filter(|(_, gets)| !gets.is_empty())
                .collect(),
            path: f.path.clone(),
            skills: f.skills.clone(),
            instructions: f.instructions.clone(),
        })
        .collect();
    let overridden = accounts
        .iter()
        .filter(|a| a.provider == Provider::Codex && Path::new(&a.path).join("AGENTS.override.md").is_file())
        .map(|a| a.name.clone())
        .collect();
    Ok(Overview {
        root: dirs.root.to_string_lossy().into_owned(),
        is_default: lib.source.is_none(),
        found,
        instructions: instructions
            .as_ref()
            .map(|i| i.iter().map(|(name, text)| instructions_info(name, text, &lib)).collect())
            .unwrap_or_default(),
        instructions_for,
        old_instructions: old_instructions.is_file().then(|| old_instructions.to_string_lossy().into_owned()),
        skills,
        no_instructions: lib.no_instructions,
        accounts,
        folders,
        places: p.places,
        found_instructions,
        overridden,
        kept: p.kept,
    })
}

/// A settings change, then a pass to lay it out.
pub(super) fn change(f: impl FnOnce(&mut LibrarySettings)) -> Result<Overview, String> {
    crate::settings::update(|s| f(&mut s.library))?;
    overview(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pass before every session's start says the same kept copies once.
    #[test]
    fn kept_copies_are_logged_once_a_list() {
        let a = vec!["E:/acc/skills/x".to_string()];
        assert!(kept_is_news(&a));
        assert!(!kept_is_news(&a), "the same list again is no news");
        assert!(kept_is_news(&["E:/acc/skills/y".to_string()]));
    }
}
