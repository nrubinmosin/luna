//! One pass over every copy the library wants: each destination compared with
//! the library and with what Luna last wrote there, and, when it acts, brought up
//! to date where that takes nothing of anyone else's.

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
    /// What the library has for it, or why it has nothing fit to lay out.
    pub(super) source: Result<Files, String>,
    /// Instructions for one kind of session: `source` has the line saying so
    /// before them (audience.rs `worded`). `Both` for everything else.
    pub(super) banner: Audience,
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
    /// Luna's copy, behind the library; a pass brings it up to date.
    Behind,
    /// Not there yet; a pass puts it there.
    Missing,
    /// Changed there since Luna wrote it.
    Changed,
    /// Deleted there since Luna wrote it.
    Gone,
    /// Something Luna did not write is in its place.
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
    /// `changed`, with the library changed too since Luna wrote it.
    pub both: bool,
    pub problem: Option<String>,
}

#[derive(Default)]
pub(super) struct Pass {
    pub(super) places: Vec<Place>,
    /// Copies no longer wanted, left where they are because they had changed
    /// (`Ledger::kept`): every one still there and still changed.
    pub(super) kept: Vec<String>,
}

/// Looks at every copy the library wants and, with `act`, does what needs no
/// asking: puts in what is missing, brings Luna's own copies up to the
/// library, adopts a destination that already holds exactly the library's,
/// and takes out the copies no longer wanted that are still as Luna wrote
/// them, remembering those changed there (`Ledger::kept`). Changed, deleted
/// and foreign destinations, and the kept copies, are only reported.
/// `accounts` and `folders` (every folder the library lists) are where a
/// copy no longer wanted may lie (files.rs `top_of`).
pub(super) fn pass(
    dirs: &Dirs,
    accounts: &[AccountInfo],
    folders: &[LibraryFolder],
    units: &[Unit],
    skipped: &[String],
    ledger: &mut Ledger,
    act: bool,
) -> Pass {
    // Equal bytes say the content is Luna's, not that the place is: a copy
    // goes only where no link on the way leads its removal elsewhere.
    let removable = |dest: &Path| -> Result<(), String> {
        let top = top_of(dirs, accounts, folders, dest).ok_or("it lies in no account's folder nor a listed one")?;
        no_link_between(&top, dest)
    };
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
        let place = |status, both, problem| Place {
            dest: key.clone(),
            target: u.target.clone(),
            skill: u.skill.clone(),
            instructions: u.instructions.clone(),
            shown_as: u.shown.clone(),
            status,
            both,
            problem,
        };
        if skipped.iter().any(|s| ident(Path::new(s)) == id) {
            out.places.push(place(Status::Skipped, false, None));
            continue;
        }
        // Noted under another spelling of the same path (an account renamed
        // in case only, on Windows): the same copy, noted as spelled now.
        if !ledger.written.contains_key(&key) {
            let alias = ledger.written.keys().find(|k| ident(Path::new(k)) == id).cloned();
            if let Some(a) = alias {
                if let Some(h) = ledger.written.remove(&a) {
                    ledger.written.insert(key.clone(), h);
                }
                if let Some(w) = ledger.worded.remove(&a) {
                    ledger.worded.insert(key.clone(), w);
                }
            }
        }
        // A copy left in place as no longer wanted (`kept`), wanted here
        // again: Luna's copy once more, compared with what Luna wrote there.
        if let Some(k) = ledger.kept.keys().find(|k| ident(Path::new(k)) == id).cloned() {
            if let Some(h) = ledger.kept.remove(&k) {
                ledger.written.entry(key.clone()).or_insert(h);
            }
        }
        let source = match &u.source {
            Ok(s) => s,
            Err(e) => {
                out.places.push(place(Status::Failed, false, Some(e.clone())));
                continue;
            }
        };
        let want = hashes(source);
        let write = |ledger: &mut Ledger| -> Result<(), String> {
            no_link_between(&u.top, &u.dest)?;
            keep_blobs(dirs, source);
            if let Err(e) = write_at(&u.dest, u.file, source) {
                // Stopped partway (a file there held open, say): what is there
                // is still all Luna's, and noted as such it stays Luna's to
                // finish next time instead of reading as changed by someone.
                if let Ok(Some(now)) = read_at(&u.dest, u.file) {
                    ledger.written.insert(key.clone(), hashes(&now));
                }
                return Err(e);
            }
            ledger.wrote(&key, want.clone(), u.banner);
            Ok(())
        };
        let status = match read_at(&u.dest, u.file) {
            Err(e) => Err(e),
            Ok(None) if ledger.written.contains_key(&key) => Ok(Status::Gone),
            Ok(None) => Ok(Status::Missing),
            Ok(Some(have)) => {
                let have = hashes(&have);
                match ledger.written.get(&key) {
                    None if have == want => {
                        if act {
                            keep_blobs(dirs, source);
                            ledger.wrote(&key, want.clone(), u.banner);
                        }
                        Ok(Status::Current)
                    }
                    None => Ok(Status::Taken),
                    Some(w) if *w != have => {
                        out.places.push(place(Status::Changed, *w != want, None));
                        continue;
                    }
                    Some(_) if have == want => Ok(Status::Current),
                    Some(_) => Ok(Status::Behind),
                }
            }
        };
        let status = match status {
            Ok(Status::Missing | Status::Behind) if act => write(ledger).map(|()| Status::Current),
            other => other,
        };
        match status {
            Ok(s) => out.places.push(place(s, false, None)),
            Err(e) => out.places.push(place(Status::Failed, false, Some(e))),
        }
    }
    if act {
        let gone: Vec<String> = ledger.written.keys().filter(|k| wanted.get(&ident(Path::new(k))) != Some(*k)).cloned().collect();
        for key in gone {
            let banner = ledger.worded.get(&key).copied().unwrap_or_default();
            let h = ledger.forget(&key).unwrap_or_default();
            let dest = PathBuf::from(&key);
            // Another spelling of a copy still wanted is that copy's file:
            // only the note goes.
            if wanted.contains_key(&ident(&dest)) || in_library(dirs, &dest) {
                continue;
            }
            match read_at(&dest, is_file(&h)) {
                Ok(None) => {}
                Ok(Some(have)) if hashes(&have) == h => {
                    // Not taken out: still Luna's, for a later pass to try.
                    if let Err(e) = removable(&dest).and_then(|()| remove_at(&dest, is_file(&h))) {
                        if link_is_news(&dest) {
                            crate::log::warn("library", &format!("could not take out {key}: {e}"));
                        }
                        ledger.wrote(&key, h, banner);
                    }
                }
                // Changed there: left, and remembered as Luna's until the
                // window settles it.
                _ => {
                    ledger.kept.insert(key, h);
                }
            }
        }
    }
    // Each copy left in place, as it is now: gone, or its place left out,
    // it is forgotten; back to exactly what Luna wrote, it goes as any copy
    // no longer wanted; still changed, it is reported on every pass.
    let kept: Vec<String> = ledger.kept.keys().cloned().collect();
    for key in kept {
        let dest = PathBuf::from(&key);
        let id = ident(&dest);
        if skipped.iter().any(|s| ident(Path::new(s)) == id) || wanted.contains_key(&id) || in_library(dirs, &dest) {
            ledger.kept.remove(&key);
            continue;
        }
        let h = ledger.kept[&key].clone();
        match read_at(&dest, is_file(&h)) {
            Ok(None) => {
                ledger.kept.remove(&key);
            }
            Ok(Some(have)) if hashes(&have) == h => {
                if !act {
                    continue;
                }
                // Not taken out, the record stays for a later pass.
                match removable(&dest).and_then(|()| remove_at(&dest, is_file(&h))) {
                    Ok(()) => {
                        ledger.kept.remove(&key);
                    }
                    Err(e) if link_is_news(&dest) => crate::log::warn("library", &format!("could not take out {key}: {e}")),
                    Err(_) => {}
                }
            }
            _ => out.kept.push(key),
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
            // Meant for one kind of session, it says so (audience.rs).
            Ok(Ok(Some(text))) => Ok(Files::from([(String::new(), worded(text, instructions_audience(lib, name)))])),
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
            banner: instructions_audience(lib, name),
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
                banner: Audience::Both,
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
                    banner: Audience::Both,
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

    #[test]
    fn a_copy_changed_there_is_asked_about_never_replaced() {
        let f = Fixture::new("changed");
        f.instructions("main", "Be brief.\n");
        let lib = LibrarySettings { instructions: first(all()), ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        let claude_md = f.account(0).join("CLAUDE.md");
        std::fs::write(&claude_md, "Be brief. And kind.\n").unwrap();

        let p = f.run(&lib, &mut ledger, true);
        let place = p.places.iter().find(|x| x.dest == claude_md.to_string_lossy()).unwrap();
        assert_eq!((place.status, place.both), (Status::Changed, false));
        assert_eq!(std::fs::read_to_string(&claude_md).unwrap(), "Be brief. And kind.\n");

        // The library changes too: still asked, and said that both did.
        f.instructions("main", "Be terse.\n");
        let p = f.run(&lib, &mut ledger, true);
        let place = p.places.iter().find(|x| x.dest == claude_md.to_string_lossy()).unwrap();
        assert_eq!((place.status, place.both), (Status::Changed, true));
        assert_eq!(std::fs::read_to_string(&claude_md).unwrap(), "Be brief. And kind.\n");
        // The other account, unchanged there, follows the library.
        assert_eq!(std::fs::read_to_string(f.account(1).join("AGENTS.md")).unwrap(), "Be terse.\n");

        // Deleted there: gone, not put back unasked.
        std::fs::remove_file(&claude_md).unwrap();
        let p = f.run(&lib, &mut ledger, true);
        assert_eq!(Fixture::status(&p, &claude_md), Status::Gone);
        assert!(!claude_md.exists());
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

        // No longer wanted: Luna's copy goes, the foreign ones stay.
        let none = LibrarySettings::default();
        f.run(&none, &mut ledger, true);
        assert!(!f.account(1).join("skills/review").exists());
        assert!(f.account(0).join("skills/review/SKILL.md").is_file());
        assert!(f.account(0).join("skills/other/SKILL.md").is_file());
        assert!(f.account(1).join("skills").is_dir(), "only the skill's own folder goes");
    }

    #[test]
    fn a_copy_no_longer_wanted_but_changed_is_kept() {
        let f = Fixture::new("kept");
        f.skill("review", "v1");
        let lib = LibrarySettings { skills: BTreeMap::from([("review".into(), reach(&["claude/work"]))]), ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        let copy = f.account(0).join("skills").join("review");
        assert!(copy.join("SKILL.md").is_file());
        std::fs::write(copy.join("extra.md"), "added there").unwrap();

        let none = LibrarySettings::default();
        let p = f.run(&none, &mut ledger, true);
        let key = copy.to_string_lossy().into_owned();
        assert_eq!(p.kept, vec![key.clone()]);
        assert!(copy.join("extra.md").is_file());
        assert!(ledger.written.is_empty(), "no longer laid out");
        // Remembered, and said on every pass, looking or acting.
        assert!(ledger.kept.contains_key(&key));
        assert_eq!(f.run(&none, &mut ledger, false).kept, vec![key.clone()]);
        assert_eq!(f.run(&none, &mut ledger, true).kept, vec![key.clone()]);
        assert!(copy.join("extra.md").is_file());

        // Back to exactly what Luna wrote: it goes as any copy no longer wanted.
        std::fs::remove_file(copy.join("extra.md")).unwrap();
        assert!(f.run(&none, &mut ledger, false).kept.is_empty(), "no longer changed");
        assert!(copy.is_dir(), "looking takes nothing out");
        f.run(&none, &mut ledger, true);
        assert!(!copy.exists());
        assert!(ledger.kept.is_empty());
    }

    /// Kept copies a pass no longer remembers: one deleted there, one whose
    /// place is wanted again (then Luna's copy, changed there, once more), one
    /// whose place is left out.
    #[test]
    fn a_kept_copy_is_forgotten_once_it_is_no_longer_kept() {
        let f = Fixture::new("kept-ends");
        f.skill("review", "v1");
        let lib = LibrarySettings { skills: skills(&["review"]), ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        let copies: Vec<PathBuf> = (0..3).map(|i| f.account(i).join("skills").join("review")).collect();
        for c in &copies {
            std::fs::write(c.join("extra.md"), "added there").unwrap();
        }
        let none = LibrarySettings::default();
        let p = f.run(&none, &mut ledger, true);
        assert_eq!(p.kept.len(), 3, "{:?}", p.kept);

        std::fs::remove_dir_all(&copies[0]).unwrap();
        let mut lib = LibrarySettings {
            skills: BTreeMap::from([("review".into(), reach(&["claude/home"]))]),
            skipped: vec![copies[1].to_string_lossy().into_owned()],
            ..Default::default()
        };
        let p = f.run(&lib, &mut ledger, true);
        assert!(p.kept.is_empty(), "{:?}", p.kept);
        assert!(ledger.kept.is_empty());
        assert_eq!(Fixture::status(&p, &copies[2]), Status::Changed, "Luna's copy again, changed there");
        assert!(copies[1].join("extra.md").is_file(), "a place left out is not touched");
        assert!(copies[2].join("extra.md").is_file());

        // Left out and taken back in: no longer Luna's, so someone else's.
        lib.skipped.clear();
        lib.skills.insert("review".into(), all());
        let p = f.run(&lib, &mut ledger, true);
        assert_eq!(Fixture::status(&p, &copies[1]), Status::Taken);
    }

    /// A link standing for an account's `skills` folder, to a folder holding
    /// byte for byte what Luna wrote: neither a copy no longer wanted nor a
    /// kept one changed back is taken out through it; Luna's note stays for a
    /// later pass.
    #[test]
    fn nothing_is_taken_out_through_a_link_on_the_way() {
        let f = Fixture::new("remove-link");
        f.skill("review", "v1");
        let lib = LibrarySettings { skills: skills(&["review"]), ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        // Account 1's copy changed there, then no longer wanted: kept.
        let kept = f.account(1).join("skills").join("review");
        std::fs::write(kept.join("extra.md"), "added there").unwrap();
        let none = LibrarySettings::default();
        let only_0 = LibrarySettings { skills: BTreeMap::from([("review".into(), reach(&["claude/work"]))]), ..Default::default() };
        f.run(&only_0, &mut ledger, true);
        assert!(ledger.kept.contains_key(&*kept.to_string_lossy()));
        // Both accounts' `skills` become links to other folders, each holding
        // exactly what Luna wrote.
        let mut others = Vec::new();
        for i in [0, 1] {
            let other = f.base.join(format!("other-{i}"));
            std::fs::create_dir_all(other.join("review")).unwrap();
            std::fs::copy(f.dirs.skills().join("review").join(SKILL_FILE), other.join("review").join(SKILL_FILE)).unwrap();
            std::fs::rename(f.account(i).join("skills"), f.base.join(format!("moved-{i}"))).unwrap();
            link_dir(&f.account(i).join("skills"), &other);
            others.push(other);
        }

        f.run(&none, &mut ledger, true);
        f.run(&none, &mut ledger, true);
        for other in &others {
            assert!(other.join("review").join(SKILL_FILE).is_file(), "{} taken out through the link", other.display());
        }
        let copy = f.account(0).join("skills").join("review");
        assert!(ledger.written.contains_key(&*copy.to_string_lossy()), "still Luna's, for a later pass");
        assert!(ledger.kept.contains_key(&*kept.to_string_lossy()), "still kept, for a later pass");
        for i in [0, 1] {
            unlink_dir(&f.account(i).join("skills"));
        }
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
    fn a_new_winner_replaces_lunas_copy_and_never_a_changed_one() {
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

        // Changed by hand, then given other instructions: asked, not replaced.
        std::fs::write(f.account(2).join("CLAUDE.md"), "Mine now.\n").unwrap();
        give_instructions(&mut lib, Some("work"), reach(&["claude/work", "claude/home"]));
        let p = f.run(&lib, &mut ledger, true);
        let place = p.places.iter().find(|x| Path::new(&x.dest) == f.account(2).join("CLAUDE.md")).unwrap();
        assert_eq!((place.status, place.both, place.instructions.as_deref()), (Status::Changed, true, Some("work")));
        assert_eq!(f.read(2, "CLAUDE.md"), "Mine now.\n");

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
        assert_eq!(Fixture::status(&p, &f.account(2).join("CLAUDE.md")), Status::Changed, "changed, not someone else's");
        assert_eq!(ledger.written, before);
    }
}
