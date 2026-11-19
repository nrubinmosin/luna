//! One pass over every copy the library wants: each destination compared with
//! the library and with what Luna last wrote there, and, when it acts, made the
//! library's wherever Luna's ledger says the copy is Luna's.

use super::*;

/// One copy the library wants somewhere.
pub(super) struct Unit {
    /// The instructions file itself, or the skill's folder.
    pub(super) dest: PathBuf,
    /// The folder `dest` lies in that no folder below may be a link (files.rs
    /// `no_link_between`, `top_of`): the account's, or for a Claude Code
    /// folder copy Luna's data folder, so `library-folders/` is looked at.
    pub(super) top: PathBuf,
    pub(super) file: bool,
    /// None for instructions.
    pub(super) skill: Option<String>,
    /// Which instructions, for instructions.
    pub(super) instructions: Option<String>,
    /// The name a folder's copy goes by where the skill's own is taken
    /// (`free_names`); its `source` is then the skill under that name.
    pub(super) shown: Option<String>,
    target: Target,
    /// What the library has for it, or why it has nothing fit to lay out:
    /// for instructions laid out for the person's chats alone, with the line
    /// saying so before them (audience.rs `worded`).
    pub(super) source: Result<Files, String>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Target {
    Account {
        provider: Provider,
        name: String,
    },
    /// What the sessions in a folder get on one account.
    Folder {
        path: String,
        provider: Provider,
        name: String,
    },
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    /// The same as the library.
    Current,
    /// Luna's copy, other than the library (behind it, or changed there); a
    /// pass writes the library's over it.
    Behind,
    /// Not there (yet, or deleted there); a pass puts it there.
    Missing,
    /// Something Luna did not write is in its place: left alone.
    Taken,
    /// Left out on purpose.
    Skipped,
    /// Cannot be laid out: `problem` says why.
    Failed,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Place {
    pub dest: String,
    pub target: Target,
    pub skill: Option<String>,
    pub instructions: Option<String>,
    /// The name the copy goes by, where it is not the skill's own.
    pub shown_as: Option<String>,
    pub status: Status,
    pub problem: Option<String>,
}

#[derive(Default)]
pub(super) struct Pass {
    pub(super) places: Vec<Place>,
}

/// Looks at every copy the library wants and, with `act`, makes it so: puts
/// in what is missing, writes the library's over Luna's own copies wherever
/// they differ from it (behind it, or changed or deleted there), adopts a
/// destination that already holds exactly the library's, and takes out
/// Luna's copies no longer wanted. The ledger alone says which copies are
/// Luna's; a destination holding anything else is someone else's file, only
/// reported, never written over or taken out. A copy changed there goes to
/// the backups before it is written over or taken out, and each such one is
/// logged. `accounts` and `folders` (every folder the library lists) are
/// where a copy no longer wanted may lie (files.rs `top_of`).
pub(super) fn pass(
    dirs: &Dirs,
    accounts: &[AccountInfo],
    folders: &[LibraryFolder],
    units: &[Unit],
    skipped: &[String],
    ledger: &mut Ledger,
    act: bool,
) -> Pass {
    // The ledger names the place, not where a link on the way to it leads: a
    // copy goes only where no link between leads its removal elsewhere.
    let removable = |dest: &Path| -> Result<(), String> {
        let top = top_of(dirs, accounts, folders, dest).ok_or("it lies in no account's folder nor a listed one")?;
        no_link_between(&top, dest)
    };
    // An account's folder or Luna's own: where a copy of Luna's stays Luna's
    // when changed there. One an earlier Luna laid into a listed folder
    // itself, changed since, is that folder's own.
    let on_lunas_ground = |dest: &Path| within(dest, &dirs.data) || accounts.iter().any(|a| within(dest, Path::new(&a.path)));
    let mut out = Pass::default();
    // Each copy wanted by `ident`, with its path as spelled now: two
    // spellings of one file are one copy.
    let mut wanted: BTreeMap<String, String> = BTreeMap::new();
    for u in units {
        let key = u.dest.to_string_lossy().into_owned();
        let id = ident(&u.dest);
        if wanted.contains_key(&id) {
            continue;
        }
        wanted.insert(id.clone(), key.clone());
        let place = |status, problem| Place {
            dest: key.clone(),
            target: u.target.clone(),
            skill: u.skill.clone(),
            instructions: u.instructions.clone(),
            shown_as: u.shown.clone(),
            status,
            problem,
        };
        if skipped.iter().any(|s| ident(Path::new(s)) == id) {
            out.places.push(place(Status::Skipped, None));
            continue;
        }
        // Noted under another spelling of the same path (an account renamed
        // in case only, on Windows): the same copy, noted as spelled now.
        if !ledger.written.contains_key(&key) {
            let alias = ledger.written.keys().find(|k| ident(Path::new(k)) == id).cloned();
            if let Some(h) = alias.and_then(|a| ledger.written.remove(&a)) {
                ledger.written.insert(key.clone(), h);
            }
        }
        let source = match &u.source {
            Ok(s) => s,
            Err(e) => {
                out.places.push(place(Status::Failed, Some(e.clone())));
                continue;
            }
        };
        let want = hashes(source);
        // `changed`: what is there, Luna's copy changed there since Luna
        // wrote it, for the backups before the library's goes over it.
        let write = |ledger: &mut Ledger, changed: Option<&Files>| -> Result<(), String> {
            no_link_between(&u.top, &u.dest)?;
            let saved = match changed {
                Some(there) => {
                    Some(back_up(dirs, &u.dest, there).map_err(|e| format!("not written over, its backup would not write: {e}"))?)
                }
                None => None,
            };
            if let Err(e) = write_at(&u.dest, u.file, source) {
                // Stopped partway (a file there held open, say): what is there
                // is noted as Luna's, to finish next time, its backup taken.
                if let Ok(Some(now)) = read_at(&u.dest, u.file) {
                    ledger.written.insert(key.clone(), hashes(&now));
                }
                return Err(match saved {
                    Some(saved) => format!("{e}; what was there is in {}", saved.display()),
                    None => e,
                });
            }
            ledger.wrote(&key, want.clone());
            if let Some(saved) = saved {
                crate::log::info(
                    "library",
                    &format!(
                        "wrote the library's copy over {key}, changed there since Luna wrote it; what was there is in {}",
                        saved.display()
                    ),
                );
            }
            Ok(())
        };
        let noted = ledger.written.contains_key(&key);
        let (status, changed) = match read_at(&u.dest, u.file) {
            Err(e) => (Err(e), None),
            Ok(None) => (Ok(Status::Missing), None),
            Ok(Some(have)) => {
                let now = hashes(&have);
                match ledger.written.get(&key) {
                    // Exactly the library's, whoever wrote it: adopted, or
                    // noted as what Luna's copy holds now.
                    _ if now == want => {
                        if act {
                            ledger.wrote(&key, want.clone());
                        }
                        (Ok(Status::Current), None)
                    }
                    None => (Ok(Status::Taken), None),
                    Some(w) if *w == now => (Ok(Status::Behind), None),
                    Some(_) => (Ok(Status::Behind), Some(have)),
                }
            }
        };
        let deleted = noted && matches!(status, Ok(Status::Missing));
        let status = match status {
            Ok(Status::Missing | Status::Behind) if act => write(ledger, changed.as_ref()).map(|()| {
                if deleted {
                    crate::log::info("library", &format!("put the library's copy back at {key}, deleted there since Luna wrote it"));
                }
                Status::Current
            }),
            other => other,
        };
        match status {
            Ok(s) => out.places.push(place(s, None)),
            Err(e) => out.places.push(place(Status::Failed, Some(e))),
        }
    }
    if act {
        let gone: Vec<String> = ledger.written.keys().filter(|k| wanted.get(&ident(Path::new(k))) != Some(*k)).cloned().collect();
        for key in gone {
            let h = ledger.written.remove(&key).unwrap_or_default();
            let dest = PathBuf::from(&key);
            let file = is_file(&h);
            // Another spelling of a copy still wanted is that copy's file:
            // only the note goes.
            if wanted.contains_key(&ident(&dest)) || in_library(dirs, &dest) {
                continue;
            }
            let taken = match read_at(&dest, file) {
                Ok(None) => continue,
                Ok(Some(have)) if hashes(&have) == h => removable(&dest).and_then(|()| remove_at(&dest, file)).map(|()| None),
                // Changed there since: Luna's copy all the same, backed up first.
                Ok(Some(have)) if on_lunas_ground(&dest) => removable(&dest).and_then(|()| {
                    let saved = back_up(dirs, &dest, &have)?;
                    remove_at(&dest, file)?;
                    Ok(Some(saved))
                }),
                Ok(Some(_)) => {
                    crate::log::info(
                        "library",
                        &format!("left {key} to the folder it lies in: changed there since an earlier Luna laid it out"),
                    );
                    continue;
                }
                Err(e) => Err(e),
            };
            match taken {
                Ok(None) => {}
                Ok(Some(saved)) => crate::log::info(
                    "library",
                    &format!(
                        "took out {key}, no longer wanted there and changed there since Luna wrote it; what was there is in {}",
                        saved.display()
                    ),
                ),
                // Not taken out: still Luna's, for a later pass to try.
                Err(e) => {
                    if link_is_news(&dest) {
                        crate::log::warn("library", &format!("could not take out {key}: {e}"));
                    }
                    ledger.wrote(&key, h);
                }
            }
        }
    }
    out
}

/// Every copy the library wants, from the settings and what the library and
/// the accounts folder hold now; the ledger tells Luna's own copies there from
/// skills of the same name that are not.
pub(super) fn units(
    dirs: &Dirs,
    lib: &LibrarySettings,
    accounts: &[AccountInfo],
    skills: &Result<Skills, String>,
    instructions: &Result<Instructions, String>,
    ledger: &Ledger,
) -> Vec<Unit> {
    let mut out = Vec::new();
    let account = |a: &AccountInfo| Target::Account { provider: a.provider, name: a.name.clone() };
    // A skill's files; while the skills will not list, every one a failure.
    // One of a system skill's name is no copy anywhere (system.rs `reserved`):
    // a copy of it laid out before goes as one no longer wanted.
    let files_of = |name: &str| match skills {
        _ if reserved(name) => None,
        Ok(found) => found.get(name).map(|(_, files)| files.clone()),
        Err(e) => Some(Err(e.clone())),
    };
    // Only instructions the library has compete for an account; while they
    // will not list, every one named in the settings does, and fails.
    let have = |name: &str| instructions.as_ref().map_or(true, |found| found.contains_key(name));
    for a in accounts {
        let Some(name) = chosen(lib, have, a) else { continue };
        // Blank is no instructions; a file that will not read (held open
        // mid-save, say) is a failure like a broken skill's, and takes none
        // of its copies out.
        let source = match instructions.as_ref().map(|found| &found[name]) {
            Ok(Ok(None)) => continue,
            // Laid out for the person's chats alone, it says so (audience.rs).
            Ok(Ok(Some(text))) => Ok(Files::from([(String::new(), worded(text, banner_of(lib, name)))])),
            Ok(Err(e)) | Err(e) => Err(e.clone()),
        };
        out.push(Unit {
            dest: Path::new(&a.path).join(instructions_name(a.provider)),
            top: PathBuf::from(&a.path),
            file: true,
            skill: None,
            instructions: Some(name.to_string()),
            shown: None,
            target: account(a),
            source,
        });
    }
    for name in lib.skills.keys() {
        let Some(files) = files_of(name.as_str()) else { continue };
        for a in accounts.iter().filter(|a| account_copy_for(lib, name, a)) {
            out.push(Unit {
                dest: Path::new(&a.path).join("skills").join(name),
                top: PathBuf::from(&a.path),
                file: false,
                skill: Some(name.clone()),
                instructions: None,
                shown: None,
                target: account(a),
                source: files.clone(),
            });
        }
    }
    for (f, slug) in folder_slugs(&lib.folders) {
        let there = Path::new(&f.path).is_dir();
        for a in accounts {
            let going: Vec<(&String, Result<Files, String>)> =
                f.skills.keys().filter(|n| folder_copy_for(lib, f, n, a)).filter_map(|n| Some((n, files_of(n)?))).collect();
            if going.is_empty() {
                continue;
            }
            let shown = match a.provider {
                Provider::Claude if there => claude_shown_names(f, a, ledger, &going.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>()),
                _ => BTreeMap::new(),
            };
            for (name, files) in going {
                let shown = shown.get(name.as_str()).cloned();
                let (dest, top) = match a.provider {
                    Provider::Claude => (
                        claude_folder_dir(dirs, &slug, a).join(".claude").join("skills").join(shown.as_deref().unwrap_or(name)),
                        dirs.data.clone(),
                    ),
                    Provider::Codex => (codex_plugin_dir(a, &slug).join("skills").join(name), PathBuf::from(&a.path)),
                };
                let source = match (there, &shown) {
                    (false, _) => Err(format!("{} is not there", f.path)),
                    (true, Some(s)) => files.map(|files| with_name(files, s)),
                    (true, None) => files,
                };
                out.push(Unit {
                    dest,
                    top,
                    file: false,
                    skill: Some(name.clone()),
                    instructions: None,
                    shown,
                    target: Target::Folder { path: f.path.clone(), provider: a.provider, name: a.name.clone() },
                    source,
                });
            }
        }
    }
    for u in &mut out {
        if in_library(dirs, &u.dest) {
            u.source = Err(format!("{} is in the library itself", u.dest.display()));
        }
    }
    out
}

/// A path as the file system tells paths apart: `/`-separated, without a
/// trailing one, and on Windows — where either slash separates and case is
/// ignored — lower-case, so two spellings of one file come out the same.
pub(super) fn ident(p: &Path) -> String {
    let s = p.to_string_lossy();
    if cfg!(windows) {
        s.replace('\\', "/").trim_end_matches('/').to_lowercase()
    } else {
        s.trim_end_matches('/').to_string()
    }
}

/// Whether `a` is `b` or inside it, per `ident`.
pub(super) fn within(a: &Path, b: &Path) -> bool {
    let (a, b) = (ident(a), ident(b));
    a == b || a.strip_prefix(&b).is_some_and(|rest| rest.starts_with('/'))
}

/// A destination in the library's skills (a library pointed at an account
/// folder, which has its `skills/` where the library does), or one holding
/// the library: its copy would be the
/// source itself, adopted as Luna's and taken out — the library's only one —
/// the day it is no longer wanted there.
pub(super) fn in_library(dirs: &Dirs, dest: &Path) -> bool {
    let skills = dirs.skills();
    if within(dest, &skills) || within(&dirs.root, dest) {
        return true;
    }
    // Reached through a link or junction (a project's `.agents/skills`
    // pointing at the library's) the spelling differs but the files do not.
    let (dest, skills, root) = (resolved(dest), resolved(&skills), resolved(&dirs.root));
    within(&dest, &skills) || within(&root, &dest)
}

/// `p` with the part of it that is there canonicalized, links and junctions
/// followed, and the rest put back on. Both sides of a comparison go through
/// it, so they carry the same form (`\\?\` on Windows).
pub(crate) fn resolved(p: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut at = p;
    loop {
        if let Ok(real) = std::fs::canonicalize(at) {
            return rest.iter().rev().fold(real, |acc, part| acc.join(part));
        }
        match (at.parent(), at.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name);
                at = parent;
            }
            _ => return p.to_path_buf(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pass_lays_out_once_and_keeps_up_with_the_library() {
        let f = Fixture::new("lay");
        f.skill("review", "v1");
        f.instructions("main", "Be brief.\n");
        let lib = LibrarySettings { instructions: first(all()), skills: skills(&["review"]), ..Default::default() };
        let mut ledger = Ledger::default();

        let p = f.run(&lib, &mut ledger, false);
        assert_eq!(Fixture::status(&p, &f.account(0).join("CLAUDE.md")), Status::Missing);
        assert!(!f.account(0).join("CLAUDE.md").exists(), "looking writes nothing");

        let p = f.run(&lib, &mut ledger, true);
        assert!(p.places.iter().all(|x| x.status == Status::Current), "{:?}", p.places);
        assert_eq!(std::fs::read_to_string(f.account(0).join("CLAUDE.md")).unwrap(), "Be brief.\n");
        assert_eq!(std::fs::read_to_string(f.account(1).join("AGENTS.md")).unwrap(), "Be brief.\n");
        assert!(f.account(1).join("skills/review/SKILL.md").is_file());

        // Nothing new: nothing written.
        let stamp = std::fs::metadata(f.account(0).join("CLAUDE.md")).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        f.run(&lib, &mut ledger, true);
        assert_eq!(std::fs::metadata(f.account(0).join("CLAUDE.md")).unwrap().modified().unwrap(), stamp);

        // The library moves on; Luna's own copies follow, a file taken out of
        // the skill goes from them too.
        f.skill("review", "v2");
        write_file(&f.dirs.skills().join("review/notes/a.md"), b"a").unwrap();
        let p = f.run(&lib, &mut ledger, false);
        assert_eq!(Fixture::status(&p, &f.account(0).join("skills/review")), Status::Behind);
        f.run(&lib, &mut ledger, true);
        assert!(std::fs::read_to_string(f.account(0).join("skills/review/SKILL.md")).unwrap().contains("v2"));
        assert!(f.account(0).join("skills/review/notes/a.md").is_file());
        std::fs::remove_dir_all(f.dirs.skills().join("review/notes")).unwrap();
        f.run(&lib, &mut ledger, true);
        assert!(!f.account(0).join("skills/review/notes").exists());
    }

    /// Luna's copies changed where they lie, in a Claude Code and a Codex
    /// account alike: looking writes nothing, a pass that acts writes the
    /// library's over them, what was there backed up first. One deleted
    /// there is put back.
    #[test]
    fn a_copy_changed_or_deleted_there_is_the_librarys_again() {
        let f = Fixture::new("changed");
        f.instructions("main", "Be brief.\n");
        f.skill("review", "v1");
        let lib = LibrarySettings { instructions: first(all()), skills: skills(&["review"]), ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        let (claude_md, agents_md) = (f.account(0).join("CLAUDE.md"), f.account(1).join("AGENTS.md"));
        let codex_skill = f.account(1).join("skills").join("review");
        std::fs::write(&claude_md, "Be brief. And kind.\n").unwrap();
        std::fs::write(&agents_md, "Be long.\n").unwrap();
        std::fs::write(codex_skill.join("extra.md"), "added there").unwrap();

        let p = f.run(&lib, &mut ledger, false);
        for dest in [&claude_md, &agents_md, &codex_skill] {
            assert_eq!(Fixture::status(&p, dest), Status::Behind, "{}", dest.display());
        }
        assert_eq!(f.read(0, "CLAUDE.md"), "Be brief. And kind.\n", "looking writes nothing");
        assert!(walk(&f.dirs.backups()).is_empty());

        let p = f.run(&lib, &mut ledger, true);
        assert!(p.places.iter().all(|x| x.status == Status::Current), "{:?}", p.places);
        assert_eq!(f.read(0, "CLAUDE.md"), "Be brief.\n");
        assert_eq!(f.read(1, "AGENTS.md"), "Be brief.\n");
        assert!(!codex_skill.join("extra.md").exists(), "the copy holds the library's files alone");
        let backed = backed_up(&f.dirs);
        for was in ["Be brief. And kind.\n", "Be long.\n", "added there"] {
            assert!(backed.iter().any(|b| b == was), "{was:?} backed up: {backed:?}");
        }
        // Nothing more to write: no more backups.
        f.run(&lib, &mut ledger, true);
        assert_eq!(backed_up(&f.dirs).len(), backed.len());

        // Changed to exactly the library's: Luna's copy, current, no backup.
        f.instructions("main", "Be terse.\n");
        std::fs::write(&claude_md, "Be terse.\n").unwrap();
        let p = f.run(&lib, &mut ledger, true);
        assert_eq!(Fixture::status(&p, &claude_md), Status::Current);
        assert_eq!(backed_up(&f.dirs).len(), backed.len());
        assert_eq!(f.read(1, "AGENTS.md"), "Be terse.\n");

        // Deleted there: put back.
        std::fs::remove_file(&claude_md).unwrap();
        std::fs::remove_dir_all(&codex_skill).unwrap();
        let p = f.run(&lib, &mut ledger, false);
        assert_eq!(Fixture::status(&p, &claude_md), Status::Missing);
        f.run(&lib, &mut ledger, true);
        assert_eq!(f.read(0, "CLAUDE.md"), "Be terse.\n");
        assert!(codex_skill.join(SKILL_FILE).is_file());
    }

    /// A changed copy whose backup will not write (a file where the backups'
    /// folder goes) is not written over: it fails for the pass, and is
    /// written over once the backup can be had.
    #[test]
    fn a_changed_copy_is_not_written_over_without_its_backup() {
        let f = Fixture::new("changed-no-backup");
        f.instructions("main", "Be brief.\n");
        let lib = LibrarySettings { instructions: first(all()), ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        let claude_md = f.account(0).join("CLAUDE.md");
        std::fs::write(&claude_md, "Mine.\n").unwrap();
        std::fs::create_dir_all(&f.dirs.data).unwrap();
        std::fs::write(f.dirs.backups(), "in the way").unwrap();

        let p = f.run(&lib, &mut ledger, true);
        let place = p.places.iter().find(|x| Path::new(&x.dest) == claude_md).unwrap();
        assert_eq!(place.status, Status::Failed);
        assert!(place.problem.as_deref().is_some_and(|e| e.contains("backup")), "{place:?}");
        assert_eq!(f.read(0, "CLAUDE.md"), "Mine.\n");

        std::fs::remove_file(f.dirs.backups()).unwrap();
        f.run(&lib, &mut ledger, true);
        assert_eq!(f.read(0, "CLAUDE.md"), "Be brief.\n");
        assert_eq!(backed_up(&f.dirs), ["Mine.\n"]);
    }

    #[test]
    fn what_luna_did_not_write_is_left_alone() {
        let f = Fixture::new("foreign");
        f.skill("review", "mine");
        let lib = LibrarySettings { skills: skills(&["review"]), ..Default::default() };
        // The account already has a skill of that name, and another one.
        write_file(&f.account(0).join("skills/review/SKILL.md"), b"someone else's").unwrap();
        write_file(&f.account(0).join("skills/other/SKILL.md"), b"other").unwrap();
        // The second account holds exactly the library's: adopted as it is.
        let ours = std::fs::read(f.dirs.skills().join("review/SKILL.md")).unwrap();
        write_file(&f.account(1).join("skills/review/SKILL.md"), &ours).unwrap();
        let mut ledger = Ledger::default();

        let p = f.run(&lib, &mut ledger, true);
        assert_eq!(Fixture::status(&p, &f.account(0).join("skills/review")), Status::Taken);
        assert_eq!(Fixture::status(&p, &f.account(1).join("skills/review")), Status::Current);
        assert_eq!(std::fs::read(f.account(0).join("skills/review/SKILL.md")).unwrap(), b"someone else's");

        // The library moves on: Luna's copy follows, the foreign one stays.
        f.skill("review", "v2");
        let p = f.run(&lib, &mut ledger, true);
        assert_eq!(Fixture::status(&p, &f.account(0).join("skills/review")), Status::Taken);
        assert_eq!(std::fs::read(f.account(0).join("skills/review/SKILL.md")).unwrap(), b"someone else's");
        assert!(f.read(1, "skills/review/SKILL.md").contains("v2"));
        assert!(walk(&f.dirs.backups()).is_empty(), "nothing of theirs touched, so nothing backed up");

        // No longer wanted: Luna's copy goes, the foreign ones stay.
        let none = LibrarySettings::default();
        f.run(&none, &mut ledger, true);
        assert!(!f.account(1).join("skills/review").exists());
        assert!(f.account(0).join("skills/review/SKILL.md").is_file());
        assert!(f.account(0).join("skills/other/SKILL.md").is_file());
        assert!(f.account(1).join("skills").is_dir(), "only the skill's own folder goes");
    }

    /// Luna's copies no longer wanted go, changed there or not, in a Claude
    /// Code and a Codex account alike: a changed one backed up first.
    #[test]
    fn a_copy_no_longer_wanted_goes_changed_there_or_not() {
        let f = Fixture::new("unwanted");
        f.skill("review", "v1");
        f.instructions("main", "Be brief.\n");
        let lib = LibrarySettings { instructions: first(all()), skills: skills(&["review"]), ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        let (claude_skill, codex_skill) = (f.account(0).join("skills").join("review"), f.account(1).join("skills").join("review"));
        std::fs::write(claude_skill.join("extra.md"), "added there").unwrap();
        std::fs::write(f.account(1).join("AGENTS.md"), "Be long.\n").unwrap();

        let none = LibrarySettings::default();
        f.run(&none, &mut ledger, false);
        assert!(claude_skill.join("extra.md").is_file(), "looking takes nothing out");
        f.run(&none, &mut ledger, true);
        assert!(!claude_skill.exists() && !codex_skill.exists());
        assert!(!f.account(0).join("CLAUDE.md").exists() && !f.account(1).join("AGENTS.md").exists());
        assert!(f.account(2).join("skills").is_dir(), "only the skill's own folder goes");
        assert!(ledger.written.is_empty(), "no longer laid out");
        let backed = backed_up(&f.dirs);
        assert!(backed.iter().any(|b| b == "added there") && backed.iter().any(|b| b.contains("v1")), "{backed:?}");
        assert!(backed.iter().any(|b| b == "Be long.\n"), "{backed:?}");
        assert!(!backed.iter().any(|b| b == "Be brief.\n"), "what was still Luna's is not backed up: {backed:?}");
    }

    /// A copy an earlier Luna laid into a listed folder itself, changed there
    /// since, is that folder's own: left there and forgotten. One still as
    /// Luna wrote it goes.
    #[test]
    fn an_old_copy_changed_in_a_listed_folder_is_left_to_the_folder() {
        let f = Fixture::new("unwanted-old");
        f.skill("review", "v1");
        let repo = f.base.join("repo");
        let files = read_dir_files(&f.dirs.skills().join("review")).unwrap();
        let mut ledger = Ledger::default();
        for name in ["review", "notes"] {
            write_at(&repo.join(name), false, &files).unwrap();
            ledger.written.insert(repo.join(name).to_string_lossy().into_owned(), hashes(&files));
        }
        std::fs::write(repo.join("review").join("extra.md"), "the team's").unwrap();
        let folder = LibraryFolder { path: repo.to_string_lossy().into_owned(), ..Default::default() };
        let lib = LibrarySettings { folders: vec![folder], ..Default::default() };

        f.run(&lib, &mut ledger, true);
        assert!(repo.join("review").join("extra.md").is_file(), "changed there: the folder's");
        assert!(!repo.join("notes").exists(), "as Luna wrote it: taken out");
        assert!(ledger.written.is_empty());
        assert!(walk(&f.dirs.backups()).is_empty());
    }

    /// The ledger 0.19 left, with copies it kept in place as no longer
    /// wanted but changed there (`kept`, for its Needs a look), and the line
    /// it noted for each chats' copy (`worded`): it loads, its kept copies
    /// stay as they are, no longer Luna's, and the next save drops both.
    #[test]
    fn a_ledger_with_0_19_s_kept_copies_loads_and_leaves_them_be() {
        let f = Fixture::new("kept-0-19");
        f.skill("review", "v1");
        let lib = LibrarySettings { skills: BTreeMap::from([("review".into(), reach(&["claude/work"]))]), ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        std::fs::create_dir_all(&f.dirs.data).unwrap();
        ledger.save(&f.dirs).unwrap();
        let kept = f.account(2).join("skills").join("review");
        write_at(&kept, false, &read_dir_files(&f.dirs.skills().join("review")).unwrap()).unwrap();
        std::fs::write(kept.join("extra.md"), "added there").unwrap();
        let mut state: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(f.dirs.ledger()).unwrap()).unwrap();
        let mut old = serde_json::Map::new();
        old.insert(kept.to_string_lossy().into_owned(), serde_json::json!({ "SKILL.md": "00", "extra.md": "11" }));
        state["kept"] = serde_json::Value::Object(old);
        // 0.19's `worded`, in both forms it wrote: read by nothing now.
        let claude_md = f.account(0).join("CLAUDE.md").to_string_lossy().into_owned();
        let agents_md = f.account(1).join("AGENTS.md").to_string_lossy().into_owned();
        state["worded"] = serde_json::json!({ claude_md: ["chat"], agents_md: "chats" });
        std::fs::write(f.dirs.ledger(), serde_json::to_string_pretty(&state).unwrap()).unwrap();

        let mut ledger = Ledger::load(&f.dirs);
        assert!(ledger.written.contains_key(&*f.account(0).join("skills").join("review").to_string_lossy()), "the rest loads");
        f.run(&lib, &mut ledger, true);
        assert!(kept.join("extra.md").is_file(), "left where it lies");
        // Wanted there now: someone else's file, so left alone too.
        let every = LibrarySettings { skills: skills(&["review"]), ..Default::default() };
        let p = f.run(&every, &mut ledger, true);
        assert_eq!(Fixture::status(&p, &kept), Status::Taken);
        assert!(kept.join("extra.md").is_file());
        ledger.save(&f.dirs).unwrap();
        let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(f.dirs.ledger()).unwrap()).unwrap();
        assert!(saved.get("kept").is_none() && saved.get("worded").is_none(), "{saved}");
    }

    /// A link standing for an account's `skills` folder, to a folder holding
    /// a copy no longer wanted, byte for byte what Luna wrote or changed
    /// since: neither is taken out through it nor backed up from it; Luna's
    /// note stays for a later pass.
    #[test]
    fn nothing_is_taken_out_through_a_link_on_the_way() {
        let f = Fixture::new("remove-link");
        f.skill("review", "v1");
        let lib = LibrarySettings { skills: skills(&["review"]), ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        // Both accounts' `skills` become links to other folders, each holding
        // what Luna wrote, account 1's with a file added.
        let mut others = Vec::new();
        for i in [0, 1] {
            let other = f.base.join(format!("other-{i}"));
            std::fs::create_dir_all(other.join("review")).unwrap();
            std::fs::copy(f.dirs.skills().join("review").join(SKILL_FILE), other.join("review").join(SKILL_FILE)).unwrap();
            std::fs::rename(f.account(i).join("skills"), f.base.join(format!("moved-{i}"))).unwrap();
            link_dir(&f.account(i).join("skills"), &other);
            others.push(other);
        }
        std::fs::write(others[1].join("review").join("extra.md"), "added there").unwrap();

        let only_2 = LibrarySettings { skills: BTreeMap::from([("review".into(), reach(&["claude/home"]))]), ..Default::default() };
        f.run(&only_2, &mut ledger, true);
        f.run(&only_2, &mut ledger, true);
        for other in &others {
            assert!(other.join("review").join(SKILL_FILE).is_file(), "{} taken out through the link", other.display());
        }
        assert!(others[1].join("review").join("extra.md").is_file());
        assert!(walk(&f.dirs.backups()).is_empty(), "nothing backed up through the link");
        for i in [0, 1] {
            let copy = f.account(i).join("skills").join("review");
            assert!(ledger.written.contains_key(&*copy.to_string_lossy()), "still Luna's, for a later pass");
        }
        for i in [0, 1] {
            unlink_dir(&f.account(i).join("skills"));
        }
    }

    /// A link standing for an account's `skills` folder (a junction on
    /// Windows) to a folder of someone else's `review`: Luna's copy there,
    /// changed by its ledger, is neither backed up nor written over through
    /// it, nor taken out once no longer wanted.
    #[test]
    fn a_linked_folder_above_a_copy_is_neither_written_nor_emptied() {
        let f = Fixture::new("linked");
        f.skill("review", "v1");
        let lib = LibrarySettings { skills: skills(&["review"]), ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        let elsewhere = f.base.join("elsewhere");
        std::fs::create_dir_all(elsewhere.join("review")).unwrap();
        std::fs::write(elsewhere.join("review").join(SKILL_FILE), "not Luna's").unwrap();
        std::fs::rename(f.account(0).join("skills"), f.base.join("moved-skills")).unwrap();
        link_dir(&f.account(0).join("skills"), &elsewhere);
        let copy = f.account(0).join("skills").join("review");

        let p = f.run(&lib, &mut ledger, true);
        let place = p.places.iter().find(|x| Path::new(&x.dest) == copy).unwrap();
        assert_eq!(place.status, Status::Failed);
        assert!(place.problem.as_deref().is_some_and(|e| e.contains("link")), "{place:?}");
        let only_others =
            LibrarySettings { skills: BTreeMap::from([("review".into(), reach(&["codex/home", "claude/home"]))]), ..Default::default() };
        f.run(&only_others, &mut ledger, true);

        assert_eq!(std::fs::read_to_string(elsewhere.join("review").join(SKILL_FILE)).unwrap(), "not Luna's");
        assert_eq!(walk(&elsewhere).len(), 1, "nothing written into the linked folder");
        assert!(walk(&f.dirs.backups()).is_empty(), "nothing backed up from it");
        assert!(ledger.written.contains_key(&*copy.to_string_lossy()), "still Luna's, for a later pass");
        unlink_dir(&f.account(0).join("skills"));
    }

    /// A listed folder inside an account's or Luna's data folder (its
    /// `skills`, `library-folders/`) is no top of its own: made a link to a
    /// folder holding byte for byte what Luna wrote, nothing is taken out
    /// through it.
    #[test]
    fn a_listed_folder_inside_a_managed_one_takes_nothing_out_through_its_link() {
        let f = Fixture::new("listed-overlap");
        f.skill("review", "v1");
        let repo = f.base.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let in_repo = LibraryFolder {
            path: repo.to_string_lossy().into_owned(),
            skills: BTreeMap::from([("review".into(), reach(&["claude/work"]))]),
            slug: "repo".into(),
            ..Default::default()
        };
        let lib = LibrarySettings {
            skills: BTreeMap::from([("review".into(), reach(&["codex/home"]))]),
            folders: vec![in_repo],
            ..Default::default()
        };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        let account_copy = f.account(1).join("skills").join("review");
        let folder_copy = claude_folder_dir(&f.dirs, "repo", &f.accounts[0]).join(".claude").join("skills").join("review");
        assert!(account_copy.join(SKILL_FILE).is_file() && folder_copy.join(SKILL_FILE).is_file());

        // Both the account's `skills` and `library-folders/` become links to
        // what was there, and are listed as folders themselves.
        let other_skills = f.base.join("other-skills");
        std::fs::rename(f.account(1).join("skills"), &other_skills).unwrap();
        link_dir(&f.account(1).join("skills"), &other_skills);
        let other_folders = f.base.join("other-folders");
        std::fs::rename(f.dirs.folders(), &other_folders).unwrap();
        link_dir(&f.dirs.folders(), &other_folders);
        let listed =
            |p: PathBuf, slug: &str| LibraryFolder { path: p.to_string_lossy().into_owned(), slug: slug.into(), ..Default::default() };
        let none = LibrarySettings {
            folders: vec![listed(f.account(1).join("skills"), "a"), listed(f.dirs.folders(), "b")],
            ..Default::default()
        };

        f.run(&none, &mut ledger, true);
        assert!(other_skills.join("review").join(SKILL_FILE).is_file(), "nothing taken out through the account's link");
        assert!(other_folders.join("repo").join("work").join(".claude").join("skills").join("review").join(SKILL_FILE).is_file());
        assert!(ledger.written.contains_key(&*account_copy.to_string_lossy()));
        assert!(ledger.written.contains_key(&*folder_copy.to_string_lossy()));
        unlink_dir(&f.account(1).join("skills"));
        unlink_dir(&f.dirs.folders());
    }

    /// `library-folders/` is Luna's own: made a link, nothing of a folder's
    /// Claude Code copies is written through it.
    #[test]
    fn a_linked_library_folders_is_not_written_through() {
        let f = Fixture::new("folders-link");
        f.skill("review", "v1");
        let repo = f.base.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let folder = LibraryFolder {
            path: repo.to_string_lossy().into_owned(),
            skills: BTreeMap::from([("review".into(), reach(&["claude/work"]))]),
            slug: "repo".into(),
            ..Default::default()
        };
        let lib = LibrarySettings { folders: vec![folder], ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        let copy = claude_folder_dir(&f.dirs, "repo", &f.accounts[0]).join(".claude").join("skills").join("review");
        assert!(copy.join(SKILL_FILE).is_file());
        let elsewhere = f.base.join("elsewhere");
        std::fs::rename(f.dirs.folders(), &elsewhere).unwrap();
        link_dir(&f.dirs.folders(), &elsewhere);
        let theirs = elsewhere.join("repo").join("work").join(".claude").join("skills").join("review").join(SKILL_FILE);
        let before = std::fs::read(&theirs).unwrap();

        // The library moves on: Luna's copy would be brought up to it.
        f.skill("review", "v2");
        let p = f.run(&lib, &mut ledger, true);
        let place = p.places.iter().find(|x| Path::new(&x.dest) == copy).unwrap();
        assert_eq!(place.status, Status::Failed);
        assert!(place.problem.as_deref().is_some_and(|e| e.contains("link")), "{place:?}");
        assert_eq!(std::fs::read(&theirs).unwrap(), before, "nothing written through the link");
        unlink_dir(&f.dirs.folders());
    }

    #[test]
    fn a_broken_skill_is_laid_out_nowhere_and_its_copies_stay() {
        let f = Fixture::new("broken");
        f.skill("review", "v1");
        let lib = LibrarySettings { skills: skills(&["review"]), ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        std::fs::write(f.dirs.skills().join("review/SKILL.md"), "no front matter").unwrap();
        let p = f.run(&lib, &mut ledger, true);
        assert!(p.places.iter().all(|x| x.status == Status::Failed));
        assert!(f.account(0).join("skills/review/SKILL.md").is_file(), "a draft mid-edit takes nothing out");
        assert!(std::fs::read_to_string(f.account(0).join("skills/review/SKILL.md")).unwrap().contains("v1"));
    }

    #[cfg(windows)]
    #[test]
    fn another_spelling_of_a_copy_is_the_same_copy() {
        let mut f = Fixture::new("case");
        f.instructions("main", "Be brief.\n");
        let lib = LibrarySettings { instructions: first(all()), ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        // The account renamed in case only: the same folder, spelled anew.
        f.accounts[0].path = f.accounts[0].path.to_uppercase();
        let claude_md = f.account(0).join("CLAUDE.md");
        let p = f.run(&lib, &mut ledger, true);
        assert_eq!(Fixture::status(&p, &claude_md), Status::Current);
        assert!(claude_md.is_file(), "the old spelling's note takes nothing out");
        let noted: Vec<String> = ledger.written.keys().filter(|k| ident(Path::new(k)) == ident(&claude_md)).cloned().collect();
        assert_eq!(noted, vec![claude_md.to_string_lossy().into_owned()]);
    }

    #[test]
    fn skipped_places_are_neither_written_nor_reported() {
        let f = Fixture::new("skip");
        f.instructions("main", "Be brief.\n");
        let claude_md = f.account(0).join("CLAUDE.md").to_string_lossy().into_owned();
        let lib = LibrarySettings { instructions: first(all()), skipped: vec![claude_md.clone()], ..Default::default() };
        let p = f.run(&lib, &mut Ledger::default(), true);
        assert_eq!(Fixture::status(&p, Path::new(&claude_md)), Status::Skipped);
        assert!(!Path::new(&claude_md).exists());
        assert!(f.account(1).join("AGENTS.md").is_file());
    }

    #[test]
    fn a_new_winner_replaces_lunas_copy_changed_there_or_not() {
        let f = Fixture::new("winner");
        f.instructions("main", "For everyone.\n");
        f.instructions("work", "For work.\n");
        let mut lib = LibrarySettings { instructions: first(all()), ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        assert_eq!(f.read(0, "CLAUDE.md"), "For everyone.\n");

        give_instructions(&mut lib, Some("work"), reach(&["claude/work"]));
        let p = f.run(&lib, &mut ledger, false);
        assert_eq!(Fixture::status(&p, &f.account(0).join("CLAUDE.md")), Status::Behind);
        f.run(&lib, &mut ledger, true);
        assert_eq!(f.read(0, "CLAUDE.md"), "For work.\n");

        // Changed by hand, then given other instructions: replaced, what was
        // there backed up.
        std::fs::write(f.account(2).join("CLAUDE.md"), "Mine now.\n").unwrap();
        give_instructions(&mut lib, Some("work"), reach(&["claude/work", "claude/home"]));
        let p = f.run(&lib, &mut ledger, true);
        let place = p.places.iter().find(|x| Path::new(&x.dest) == f.account(2).join("CLAUDE.md")).unwrap();
        assert_eq!((place.status, place.instructions.as_deref()), (Status::Current, Some("work")));
        assert_eq!(f.read(2, "CLAUDE.md"), "For work.\n");
        assert_eq!(backed_up(&f.dirs), ["Mine now.\n"]);

        // Back to the first: Luna's copy follows again.
        give_instructions(&mut lib, Some("work"), Vec::new());
        f.run(&lib, &mut ledger, true);
        assert_eq!(f.read(0, "CLAUDE.md"), "For everyone.\n");
    }

    #[test]
    fn copies_laid_out_before_several_instructions_stay_lunas() {
        let f = Fixture::new("upgrade");
        // As the Luna of one instructions file left things: the file in the
        // library's root, a reach of its own, its copies in the ledger, one
        // of them since changed by hand.
        std::fs::write(f.dirs.root.join(OLD_INSTRUCTIONS), "Be brief.\n").unwrap();
        let mut ledger = Ledger::default();
        for (i, file) in [(0, "CLAUDE.md"), (1, "AGENTS.md"), (2, "CLAUDE.md")] {
            let dest = f.account(i).join(file);
            std::fs::write(&dest, "Be brief.\n").unwrap();
            ledger.written.insert(dest.to_string_lossy().into_owned(), hashes(&Files::from([(String::new(), b"Be brief.\n".to_vec())])));
        }
        std::fs::write(f.account(2).join("CLAUDE.md"), "Be brief. Mine.\n").unwrap();
        let before = ledger.written.clone();
        let lib: LibrarySettings = serde_json::from_str(r#"{"instructions": ["all"]}"#).unwrap();

        // Before the move, nothing is taken out for want of instructions.
        let p = f.run(&lib, &mut ledger, true);
        assert!(p.places.iter().all(|x| x.status == Status::Failed), "{:?}", p.places);
        assert_eq!(f.read(0, "CLAUDE.md"), "Be brief.\n");

        migrate_instructions(&f.dirs);
        let p = f.run(&lib, &mut ledger, true);
        assert_eq!(Fixture::status(&p, &f.account(0).join("CLAUDE.md")), Status::Current);
        assert_eq!(Fixture::status(&p, &f.account(1).join("AGENTS.md")), Status::Current);
        assert_eq!(Fixture::status(&p, &f.account(2).join("CLAUDE.md")), Status::Current);
        assert_eq!(f.read(2, "CLAUDE.md"), "Be brief.\n", "changed there, still Luna's copy: not someone else's");
        assert_eq!(ledger.written, before);
    }
}
