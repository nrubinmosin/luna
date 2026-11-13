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

/// A copy of Luna's no longer wanted where it lies, left there because it was
/// changed since Luna wrote it (`Ledger::kept`): the window takes it out or
/// keeps it as the user's own (commands.rs `library_kept`).
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct KeptCopy {
    pub dest: String,
    /// The account it lies in, or the folder's sessions on one account it was
    /// for; None where the path no longer says (a folder taken off the list).
    pub target: Option<Target>,
    /// The skill's name there; None for an instructions file.
    pub skill: Option<String>,
}

/// What a kept copy's path says of it: a skill or instructions, and whose.
fn kept_copy(dirs: &Dirs, lib: &LibrarySettings, accounts: &[AccountInfo], dest: &str, file: bool) -> KeptCopy {
    let path = Path::new(dest);
    let parent = path.parent().map(ident);
    let id = ident(path);
    let slugs = folder_slugs(&lib.folders);
    let target = accounts.iter().find_map(|a| {
        let account = Path::new(&a.path);
        let laid =
            if file { id == ident(&account.join(instructions_name(a.provider))) } else { parent == Some(ident(&account.join("skills"))) };
        if laid {
            return Some(Target::Account { provider: a.provider, name: a.name.clone() });
        }
        slugs.iter().find_map(|(f, slug)| {
            let skills = match a.provider {
                Provider::Claude => claude_folder_dir(dirs, slug, a).join(".claude").join("skills"),
                Provider::Codex => codex_plugin_dir(a, slug).join("skills"),
            };
            (!file && parent == Some(ident(&skills))).then(|| Target::Folder {
                path: f.path.clone(),
                provider: a.provider,
                name: a.name.clone(),
            })
        })
    });
    let skill = (!file).then(|| path.file_name().unwrap_or_default().to_string_lossy().into_owned());
    KeptCopy { dest: dest.to_string(), target, skill }
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
    /// Copies no longer wanted but changed where they lie, left there: each
    /// on every overview until it is settled.
    pub kept: Vec<KeptCopy>,
    /// The system texts, each as it is served now (system.rs): Luna's own
    /// text, the library's override of it, or one that will not do.
    pub system: Vec<SystemSkillState>,
    /// Folders set aside in `system/`, read by nothing (system.rs
    /// `set_aside`): for the person to move or delete.
    pub system_aside: Vec<String>,
    /// The places a put-back or a kept copy's settling could not settle, with
    /// why (commands.rs `library_put_back`, `library_kept`); empty from every
    /// other command.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unsettled: Vec<String>,
}

/// Whether a link `codex_manifests` keeps clear of, or a copy a pass would not
/// take out (a link on the way to it), is yet to be said: once a run, as a
/// pass comes with every session's start.
pub(super) fn link_is_news(link: &Path) -> bool {
    static SAID: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());
    SAID.lock().unwrap_or_else(|e| e.into_inner()).insert(link.to_path_buf())
}

/// Whether these kept copies are not what the log said last: a pass comes
/// before every session's start, and each list once is enough.
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
    // Only a pass that lays out needs the lock file (`lock_to_read`).
    let _guard = if act { lock()? } else { lock_to_read() };
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
    let p = pass(&dirs, &accounts, &lib.folders, &units, &lib.skipped, &mut ledger, act && found);
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
    let kept = p.kept.iter().map(|k| kept_copy(&dirs, &lib, &accounts, k, ledger.kept.get(k).is_some_and(is_file))).collect();
    // An entry given per session that would push a Codex session's
    // developer_instructions off its command line (role.rs).
    let mut over = if found { codex_overflows(&dirs, &lib, &accounts) } else { BTreeMap::new() };
    let mut info = |name: &String, text| {
        let mut i = instructions_info(name, text, &lib);
        i.problem = i.problem.or_else(|| over.remove(name));
        i
    };
    Ok(Overview {
        root: dirs.root.to_string_lossy().into_owned(),
        is_default: lib.source.is_none(),
        found,
        instructions: instructions.as_ref().map(|i| i.iter().map(|(name, text)| info(name, text)).collect()).unwrap_or_default(),
        instructions_for,
        old_instructions: old_instructions.is_file().then(|| old_instructions.to_string_lossy().into_owned()),
        skills,
        no_instructions: lib.no_instructions,
        accounts,
        folders,
        places: p.places,
        found_instructions,
        overridden,
        kept,
        system: SYSTEM_TEXTS.iter().filter_map(|t| resolve_text(&dirs, t.name)).map(|r| r.state()).collect(),
        system_aside: set_aside(&dirs),
        unsettled: Vec::new(),
    })
}

/// A settings change, then a pass to lay it out: under the lock from the
/// start, so that a change made is one laid out, or with no lock not made.
pub(super) fn change(f: impl FnOnce(&mut LibrarySettings)) -> Result<Overview, String> {
    let _guard = lock()?;
    crate::settings::update(|s| f(&mut s.library))?;
    overview_locked(true)
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
