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
    /// What this pass did, for a Sync to say: the copies it wrote the
    /// library's into, those it took out, and those it could not take out
    /// (`dest: why`). Empty on a look.
    pub(super) written: Vec<String>,
    pub(super) removed: Vec<String>,
    pub(super) not_removed: Vec<String>,
}

/// Looks at every copy the library wants and, with `act`, makes it so: puts
/// in what is missing, writes the library's over Luna's own copies wherever
/// they differ from it (behind it, or changed or deleted there), adopts a
/// destination that already holds exactly the library's, and takes out
/// Luna's copies no longer wanted. The ledger alone says which copies are
/// Luna's, and in a skill's folder which files (`luna_part`): a destination
/// holding anything else is someone else's file, only reported, never
/// written over or taken out, and what else lies in a folder of Luna's is
/// never looked at. A copy changed there goes to the backups before it is
/// written over or taken out, and each such one is logged; one that changes
/// again while the pass is at it is left for the next. `accounts` and
/// `folders` (every folder the library lists) are where a copy no longer
/// wanted may lie (files.rs `top_of`).
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
        let noted = ledger.written.get(&key).cloned();
        // What of the destination is Luna's to look at: in a skill's folder,
        // the files it wrote there and those the library has for it.
        let owned: Vec<String> = noted.iter().flat_map(|h| h.keys()).chain(want.keys()).cloned().collect();
        let part = |all: Files| luna_part(all, u.file, &owned);
        let disowned = ledger.disowned.iter().find(|d| ident(Path::new(d)) == id).cloned();
        let read = read_at(&u.dest, u.file);
        // A place 0.19 left as the person's own is theirs for as long as
        // anything is there, whatever it holds; once nothing is, the
        // library's copy may go there as anywhere.
        if let Some(d) = &disowned {
            match &read {
                Ok(Some(_)) if noted.is_none() => {
                    out.places.push(place(Status::Taken, None));
                    continue;
                }
                Ok(None) if act => {
                    ledger.disowned.remove(d);
                }
                _ => {}
            }
        }
        // `there`: Luna's part of what is there, as read now; `changed`, it
        // differs from what Luna wrote, for the backups first.
        let (status, there, changed) = match read {
            Err(e) => (Err(e), None, false),
            Ok(None) => (Ok(Status::Missing), None, false),
            Ok(Some(all)) => match &noted {
                // Exactly the library's, whoever wrote it: adopted, or noted
                // as what Luna's copy holds now. Adopted only whole: a folder
                // with more in it than the library's is not Luna's to take.
                None if hashes(&all) == want => {
                    if act {
                        ledger.wrote(&key, want.clone());
                    }
                    (Ok(Status::Current), None, false)
                }
                None => (Ok(Status::Taken), None, false),
                Some(w) => {
                    let mine = part(all);
                    let now = hashes(&mine);
                    if now == want {
                        if act {
                            ledger.wrote(&key, want.clone());
                        }
                        (Ok(Status::Current), None, false)
                    } else {
                        let changed = now != *w && !mine.is_empty();
                        (Ok(Status::Behind), Some(mine), changed)
                    }
                }
            },
        };
        let status = match status {
            Ok(was @ (Status::Missing | Status::Behind)) if act => {
                match lay_over(dirs, u, &key, source, &want, noted.as_ref(), there.as_ref(), changed, &part, ledger) {
                    Ok(true) => {
                        out.written.push(key.clone());
                        if was == Status::Missing && noted.is_some() {
                            crate::log::info(
                                "library",
                                &format!("put the library's copy back at {key}, deleted there since Luna wrote it"),
                            );
                        }
                        Ok(Status::Current)
                    }
                    Ok(false) => Ok(was),
                    Err(e) => Err(e),
                }
            }
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
            let in_account = accounts.iter().any(|a| within(&dest, Path::new(&a.path)));
            let in_listed = folders.iter().any(|f| within(&dest, Path::new(&f.path)));
            // A Claude Code folder copy waits in `library-folders/<slug>/<the
            // account's name>/`: its account's own while that is listed.
            let staged = within(&dest, &dirs.folders()) && staged_for_listed(dirs, accounts, &dest);
            // In an account Luna does not list now (one taken off its list
            // with its folder left, or not there for the moment), or a folder
            // no longer listed: its note stays as it is, for when the account
            // is back.
            if !in_account && !in_listed && !staged {
                if link_is_news(&dest) {
                    crate::log::info(
                        "library",
                        &format!("kept Luna's note of {key}: it lies in no account Luna lists now, nor a folder it does"),
                    );
                }
                ledger.wrote(&key, h);
                continue;
            }
            let there = match read_at(&dest, file) {
                Ok(None) => continue,
                Ok(Some(all)) => luna_part(all, file, h.keys()),
                Err(e) => {
                    if link_is_news(&dest) {
                        crate::log::warn("library", &format!("could not take out {key}: {e}"));
                    }
                    out.not_removed.push(format!("{key}: {e}"));
                    ledger.wrote(&key, h);
                    continue;
                }
            };
            // Nothing of Luna's left in it: only what others put there.
            if there.is_empty() {
                continue;
            }
            let changed = hashes(&there) != h;
            // A copy an earlier Luna laid into a listed folder, changed there
            // since, is that folder's own, wherever the folder lies.
            if changed && in_listed {
                crate::log::info("library", &format!("left {key} to the listed folder it lies in: changed there since Luna laid it out"));
                continue;
            }
            match take_out_copy(dirs, &dest, file, &there, changed, &removable) {
                Ok(saved) => {
                    if let Some(saved) = saved {
                        crate::log::info(
                            "library",
                            &format!(
                                "took out {key}, no longer wanted there and changed there since Luna wrote it; what was there is in {}",
                                saved.display()
                            ),
                        );
                    }
                    out.removed.push(key);
                }
                // Not taken out: still Luna's, for a later pass to try.
                Err(e) => {
                    if link_is_news(&dest) {
                        crate::log::warn("library", &format!("could not take out {key}: {e}"));
                    }
                    out.not_removed.push(format!("{key}: {e}"));
                    ledger.wrote(&key, h);
                }
            }
        }
    }
    out
}

/// Whether a copy in `library-folders/` lies in the folder of a Claude Code
/// account listed now (`claude_folder_dir`, `<slug>/<name>`); one of an
/// account not listed for the moment is that account's, kept as it is.
fn staged_for_listed(dirs: &Dirs, accounts: &[AccountInfo], dest: &Path) -> bool {
    let below: Vec<String> =
        dest.components().skip(dirs.folders().components().count()).map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    let [slug, _, ..] = below.as_slice() else { return false };
    accounts.iter().filter(|a| a.provider == Provider::Claude).any(|a| within(dest, &claude_folder_dir(dirs, slug, a)))
}

/// The library's copy laid at a unit's place: `there`, Luna's part of what
/// is there now (None: nothing), backed up first where `changed`. Each file
/// is looked at again right before it is written (`write_over`): Ok(false)
/// where one changed meanwhile, the place left as it was for the next pass,
/// logged. A step that fails is undone too, the place failed for this pass
/// and logged; only where the undoing does not go through either does the
/// ledger note what of it is now the library's (`noted_after_failure`).
#[allow(clippy::too_many_arguments)]
fn lay_over(
    dirs: &Dirs,
    u: &Unit,
    key: &str,
    source: &Files,
    want: &Hashes,
    noted: Option<&Hashes>,
    there: Option<&Files>,
    changed: bool,
    part: &dyn Fn(Files) -> Files,
    ledger: &mut Ledger,
) -> Result<bool, String> {
    no_link_between(&u.top, &u.dest)?;
    let saved = match there {
        Some(there) if changed => {
            Some(back_up(dirs, &u.dest, there).map_err(|e| format!("not written over, its backup would not write: {e}"))?)
        }
        _ => None,
    };
    match write_over(&u.dest, u.file, source, there.unwrap_or(&Files::new())) {
        Ok(()) => {}
        Err(Halt::Changed(at)) => {
            crate::log::info("library", &format!("left {key} for the next pass: {} changed there while this one was at it", at.display()));
            return Ok(false);
        }
        Err(Halt::Failed { error, undone }) => {
            if !undone {
                let now = read_at(&u.dest, u.file).ok().flatten().map(part);
                let left = noted_after_failure(noted, want, now.as_ref());
                if left.is_empty() {
                    ledger.written.remove(key);
                } else {
                    ledger.written.insert(key.to_string(), left);
                }
            }
            let left = if undone { "left as it was" } else { "left partway, and could not be put back" };
            let e = match saved {
                Some(saved) => format!("{error}; {left}; what was there is in {}", saved.display()),
                None => format!("{error}; {left}"),
            };
            crate::log::warn("library", &format!("could not lay the library's copy out at {key}: {e}"));
            return Err(e);
        }
    }
    ledger.wrote(key, want.clone());
    if let Some(saved) = saved {
        crate::log::info(
            "library",
            &format!("wrote the library's copy over {key}, changed there since Luna wrote it; what was there is in {}", saved.display()),
        );
    }
    Ok(true)
}

/// What the ledger is to say of a copy whose write stopped partway: what it
/// said before, less the files gone that the library no longer has, and each
/// file now exactly the library's noted as Luna's. Nothing else there is
/// taken for Luna's: a person's file it did not get to stays theirs to back
/// up at the next pass.
fn noted_after_failure(before: Option<&Hashes>, want: &Hashes, now: Option<&Files>) -> Hashes {
    let now = now.map(hashes).unwrap_or_default();
    let mut out = before.cloned().unwrap_or_default();
    out.retain(|rel, _| now.contains_key(rel) || want.contains_key(rel));
    for (rel, h) in want {
        if now.get(rel) == Some(h) {
            out.insert(rel.clone(), h.clone());
        }
    }
    out
}

/// Luna's part of a copy no longer wanted (`there`, as just read) taken
/// out, backed up first where `changed`: the backup's folder, where there
/// is one. Refused through a link on the way (`removable`); left as it was,
/// an error, where a file of it changed meanwhile or would not go.
fn take_out_copy(
    dirs: &Dirs,
    dest: &Path,
    file: bool,
    there: &Files,
    changed: bool,
    removable: &dyn Fn(&Path) -> Result<(), String>,
) -> Result<Option<PathBuf>, String> {
    removable(dest)?;
    let saved = if changed { Some(back_up(dirs, dest, there)?) } else { None };
    take_out(dest, file, there).map_err(|h| match h {
        Halt::Changed(at) => format!("{} changed there while the pass was at it; left for the next", at.display()),
        Halt::Failed { error, undone: true } => format!("{error}; left as it was"),
        Halt::Failed { error, undone: false } => format!("{error}; left partway, and could not be put back"),
    })?;
    Ok(saved)
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
    let files_of = |name: &str| match skills {
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
        let mut written = p.written.clone();
        written.sort();
        let mut wanted: Vec<String> = p.places.iter().map(|x| x.dest.clone()).collect();
        wanted.sort();
        assert_eq!(written, wanted, "a pass says what it wrote");
        assert_eq!(std::fs::read_to_string(f.account(0).join("CLAUDE.md")).unwrap(), "Be brief.\n");
        assert_eq!(std::fs::read_to_string(f.account(1).join("AGENTS.md")).unwrap(), "Be brief.\n");
        assert!(f.account(1).join("skills/review/SKILL.md").is_file());

        // Nothing new: nothing written.
        let stamp = std::fs::metadata(f.account(0).join("CLAUDE.md")).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(f.run(&lib, &mut ledger, true).written.is_empty());
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
        std::fs::write(codex_skill.join(SKILL_FILE), "edited there").unwrap();

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
        assert!(std::fs::read_to_string(codex_skill.join(SKILL_FILE)).unwrap().contains("v1"));
        let backed = backed_up(&f.dirs);
        for was in ["Be brief. And kind.\n", "Be long.\n", "edited there"] {
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
        std::fs::write(claude_skill.join(SKILL_FILE), "edited there").unwrap();
        std::fs::write(f.account(1).join("AGENTS.md"), "Be long.\n").unwrap();

        let none = LibrarySettings::default();
        f.run(&none, &mut ledger, false);
        assert!(claude_skill.join(SKILL_FILE).is_file(), "looking takes nothing out");
        let p = f.run(&none, &mut ledger, true);
        assert!(p.removed.iter().any(|r| Path::new(r) == claude_skill), "a pass says what it took out: {:?}", p.removed);
        assert!(p.written.is_empty() && p.not_removed.is_empty());
        assert!(!claude_skill.exists() && !codex_skill.exists());
        assert!(!f.account(0).join("CLAUDE.md").exists() && !f.account(1).join("AGENTS.md").exists());
        assert!(f.account(2).join("skills").is_dir(), "only the skill's own folder goes");
        assert!(ledger.written.is_empty(), "no longer laid out");
        let backed = backed_up(&f.dirs);
        assert!(backed.iter().any(|b| b == "edited there"), "{backed:?}");
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
        std::fs::write(repo.join("review").join(SKILL_FILE), "the team's").unwrap();
        let folder = LibraryFolder { path: repo.to_string_lossy().into_owned(), ..Default::default() };
        let lib = LibrarySettings { folders: vec![folder], ..Default::default() };

        f.run(&lib, &mut ledger, true);
        assert_eq!(std::fs::read_to_string(repo.join("review").join(SKILL_FILE)).unwrap(), "the team's", "changed there: the folder's");
        assert!(!repo.join("notes").exists(), "as Luna wrote it: taken out");
        assert!(ledger.written.is_empty());
        assert!(walk(&f.dirs.backups()).is_empty());
    }

    /// The ledger 0.19 left, with copies it kept in place as no longer
    /// wanted but changed there (`kept`, for its Needs a look), and the line
    /// it noted for each chats' copy (`worded`): it loads, its kept copies
    /// stay as they are, the person's own, never adopted even once they hold
    /// the library's very bytes; the log names them at the save that drops
    /// 0.19's record. Once one is gone, the library's copy goes there.
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
        state.as_object_mut().unwrap().remove("format");
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
        let line = ledger.left_by_0_19_line().expect("a line for the log");
        assert!(line.contains(&*kept.to_string_lossy()) && line.contains("person's own"), "{line}");
        f.run(&lib, &mut ledger, true);
        assert!(kept.join("extra.md").is_file(), "left where it lies");
        // Wanted there now: someone else's file, so left alone too, even as
        // byte for byte the library's.
        let every = LibrarySettings { skills: skills(&["review"]), ..Default::default() };
        let p = f.run(&every, &mut ledger, true);
        assert_eq!(Fixture::status(&p, &kept), Status::Taken);
        std::fs::remove_file(kept.join("extra.md")).unwrap();
        let p = f.run(&every, &mut ledger, true);
        assert_eq!(Fixture::status(&p, &kept), Status::Taken, "never adopted");
        assert!(!ledger.written.contains_key(&*kept.to_string_lossy()));
        ledger.save(&f.dirs).unwrap();
        let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(f.dirs.ledger()).unwrap()).unwrap();
        assert!(saved.get("kept").is_none() && saved.get("worded").is_none(), "{saved}");
        let mut ledger = Ledger::load(&f.dirs);
        assert!(ledger.left_by_0_19_line().is_none(), "said once");
        // Still not adopted after the save; unassigned, it stays.
        assert_eq!(Fixture::status(&f.run(&every, &mut ledger, true), &kept), Status::Taken);
        f.run(&lib, &mut ledger, true);
        assert!(kept.join(SKILL_FILE).is_file(), "not Luna's to take out");

        // Gone: the place is the library's again.
        std::fs::remove_dir_all(&kept).unwrap();
        f.run(&every, &mut ledger, true);
        assert!(ledger.written.contains_key(&*kept.to_string_lossy()) && ledger.disowned.is_empty());
    }

    /// Files a person or a tool put into a folder of Luna's (a Python
    /// cache, a `.DS_Store`, a draft named as Luna's temp files are) are no
    /// part of the copy: not a change, not backed up, not written over; once
    /// the skill is no longer wanted Luna's own files go and they stay, in
    /// their folder. On a Claude Code and a Codex account alike.
    #[test]
    fn what_others_put_in_a_skill_folder_is_left_alone() {
        let f = Fixture::new("extras");
        f.skill("review", "v1");
        let lib = LibrarySettings { skills: skills(&["review"]), ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        let extras = ["__pycache__/helper.cpython-312.pyc", ".DS_Store", "draft.luna-tmp", "node_modules/x/index.js"];
        let copies = [f.account(0).join("skills").join("review"), f.account(1).join("skills").join("review")];
        for c in &copies {
            for e in extras {
                write_file(&c.join(e), b"theirs").unwrap();
            }
        }

        let p = f.run(&lib, &mut ledger, true);
        for c in &copies {
            assert_eq!(Fixture::status(&p, c), Status::Current, "{}", c.display());
        }
        f.skill("review", "v2");
        f.run(&lib, &mut ledger, true);
        assert!(walk(&f.dirs.backups()).is_empty(), "nothing of theirs backed up");
        for c in &copies {
            assert!(std::fs::read_to_string(c.join(SKILL_FILE)).unwrap().contains("v2"));
            for e in extras {
                assert_eq!(std::fs::read(c.join(e)).unwrap(), b"theirs", "{e}");
            }
        }

        f.run(&LibrarySettings::default(), &mut ledger, true);
        assert!(ledger.written.is_empty());
        assert!(walk(&f.dirs.backups()).is_empty());
        for c in &copies {
            assert!(!c.join(SKILL_FILE).exists(), "Luna's file taken out");
            for e in extras {
                assert!(c.join(e).is_file(), "{e} stays");
            }
        }
    }

    /// Stands in for a CLI saving `path` with `bytes` the moment the pass is
    /// about to look at it again before writing or taking it out, once.
    fn saved_meanwhile(path: PathBuf, bytes: &'static str) {
        BEFORE_EACH.with(|h| {
            let mut once = Some(bytes);
            *h.borrow_mut() = Some(Box::new(move |at: &Path| {
                if at == path {
                    if let Some(bytes) = once.take() {
                        write_file(&path, bytes.as_bytes()).unwrap();
                    }
                }
            }));
        });
    }

    fn no_saves_meanwhile() {
        BEFORE_EACH.with(|h| *h.borrow_mut() = None);
    }

    /// A file of a copy saved by someone after the pass has begun writing
    /// the copy, between two of its files: what the pass had written of it
    /// is put back, the save stays, and the copy is left for the next pass,
    /// which backs it all up and writes the library's over it. The same for
    /// a destination made where the pass had found none, and for a copy no
    /// longer wanted that is being taken out.
    #[test]
    fn a_copy_saved_while_the_pass_writes_it_is_left_for_the_next() {
        let f = Fixture::new("race");
        f.instructions("main", "Be brief.\n");
        f.skill("review", "v1");
        write_file(&f.dirs.skills().join("review").join("guide.md"), b"guide v1").unwrap();
        let lib = LibrarySettings { instructions: first(all()), skills: skills(&["review"]), ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        let copy = f.account(0).join("skills").join("review");
        std::fs::write(copy.join(SKILL_FILE), "edited there").unwrap();
        f.skill("review", "v2");
        write_file(&f.dirs.skills().join("review").join("guide.md"), b"guide v2").unwrap();
        let before = ledger.written[&*copy.to_string_lossy()].clone();

        // `SKILL.md` goes first (upper case sorts first), then `guide.md`,
        // saved just as the pass comes to it.
        saved_meanwhile(copy.join("guide.md"), "saved meanwhile");
        let p = f.run(&lib, &mut ledger, true);
        no_saves_meanwhile();
        assert_eq!(Fixture::status(&p, &copy), Status::Behind);
        assert_eq!(std::fs::read_to_string(copy.join(SKILL_FILE)).unwrap(), "edited there", "what the pass wrote is put back");
        assert_eq!(std::fs::read_to_string(copy.join("guide.md")).unwrap(), "saved meanwhile");
        assert_eq!(ledger.written[&*copy.to_string_lossy()], before);
        f.run(&lib, &mut ledger, true);
        assert!(std::fs::read_to_string(copy.join(SKILL_FILE)).unwrap().contains("v2"));
        assert_eq!(std::fs::read_to_string(copy.join("guide.md")).unwrap(), "guide v2");
        let backed = backed_up(&f.dirs);
        assert!(backed.iter().any(|b| b == "saved meanwhile") && backed.iter().any(|b| b == "edited there"), "{backed:?}");

        // Deleted there, then made again by a CLI just as the pass puts the
        // library's back: the new file stays.
        let agents_md = f.account(1).join("AGENTS.md");
        std::fs::remove_file(&agents_md).unwrap();
        saved_meanwhile(agents_md.clone(), "made meanwhile");
        let p = f.run(&lib, &mut ledger, true);
        no_saves_meanwhile();
        assert_eq!(Fixture::status(&p, &agents_md), Status::Missing);
        assert_eq!(f.read(1, "AGENTS.md"), "made meanwhile");
        f.run(&lib, &mut ledger, true);
        assert_eq!(f.read(1, "AGENTS.md"), "Be brief.\n");
        assert!(backed_up(&f.dirs).iter().any(|b| b == "made meanwhile"));

        // No longer wanted: `SKILL.md` taken out, then `guide.md` saved just
        // as the pass comes to it: `SKILL.md` is put back, the note stays.
        let only_codex = LibrarySettings { skills: BTreeMap::from([("review".into(), reach(&["codex/home"]))]), ..Default::default() };
        saved_meanwhile(copy.join("guide.md"), "saved as it went");
        f.run(&only_codex, &mut ledger, true);
        no_saves_meanwhile();
        assert!(std::fs::read_to_string(copy.join(SKILL_FILE)).unwrap().contains("v2"), "put back");
        assert_eq!(std::fs::read_to_string(copy.join("guide.md")).unwrap(), "saved as it went");
        assert!(ledger.written.contains_key(&*copy.to_string_lossy()), "still Luna's, for the next pass");
        f.run(&only_codex, &mut ledger, true);
        assert!(!copy.join(SKILL_FILE).exists() && !copy.join("guide.md").exists());
        assert!(backed_up(&f.dirs).iter().any(|b| b == "saved as it went"));
    }

    /// A rename Windows refuses (a file of the copy held open without
    /// letting others replace it): the place fails for this pass, logged,
    /// and is not left half the library's: the file already written is put
    /// back. Once the file is let go, the next pass writes it all.
    #[cfg(windows)]
    #[test]
    fn a_refused_rename_leaves_the_copy_as_it_was() {
        use std::os::windows::fs::OpenOptionsExt;
        let f = Fixture::new("held-open");
        f.skill("review", "v1");
        write_file(&f.dirs.skills().join("review").join("guide.md"), b"guide v1").unwrap();
        let lib = LibrarySettings { skills: skills(&["review"]), ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        let copy = f.account(0).join("skills").join("review");
        let was_skill = std::fs::read(copy.join(SKILL_FILE)).unwrap();
        let before = ledger.written[&*copy.to_string_lossy()].clone();
        f.skill("review", "v2");
        write_file(&f.dirs.skills().join("review").join("guide.md"), b"guide v2").unwrap();
        // FILE_SHARE_READ alone: no replacing it while held.
        let held = std::fs::OpenOptions::new().read(true).share_mode(1).open(copy.join("guide.md")).unwrap();

        let p = f.run(&lib, &mut ledger, true);
        let place = p.places.iter().find(|x| Path::new(&x.dest) == copy).unwrap();
        assert_eq!(place.status, Status::Failed);
        assert!(place.problem.as_deref().is_some_and(|e| e.contains("left as it was")), "{place:?}");
        assert_eq!(std::fs::read(copy.join(SKILL_FILE)).unwrap(), was_skill, "not half written");
        assert_eq!(std::fs::read_to_string(copy.join("guide.md")).unwrap(), "guide v1");
        assert_eq!(ledger.written[&*copy.to_string_lossy()], before);
        assert!(!walk(&copy).iter().any(|p| p.to_string_lossy().ends_with(".luna-tmp")), "no temp file left");

        drop(held);
        f.run(&lib, &mut ledger, true);
        assert!(std::fs::read_to_string(copy.join(SKILL_FILE)).unwrap().contains("v2"));
        assert_eq!(std::fs::read_to_string(copy.join("guide.md")).unwrap(), "guide v2");
    }

    /// A write that stops partway and cannot be put back notes as Luna's
    /// only what is now the library's: a person's file it did not reach is
    /// never taken for Luna's.
    #[test]
    fn a_write_left_partway_notes_only_what_it_wrote() {
        let h = |s: &str| hashes(&Files::from([(String::new(), s.as_bytes().to_vec())]));
        let one = |s: &str| Files::from([(String::new(), s.as_bytes().to_vec())]);
        assert_eq!(noted_after_failure(Some(&h("Luna's")), &h("library's"), Some(&one("mine"))), h("Luna's"));
        assert!(noted_after_failure(None, &h("library's"), Some(&one("mine"))).is_empty());
        let lib = Files::from([(SKILL_FILE.to_string(), b"v2".to_vec()), ("guide.md".to_string(), b"g2".to_vec())]);
        let before = hashes(&Files::from([
            (SKILL_FILE.to_string(), b"v1".to_vec()),
            ("guide.md".to_string(), b"g1".to_vec()),
            ("old.md".to_string(), b"o".to_vec()),
        ]));
        let now = Files::from([(SKILL_FILE.to_string(), b"v2".to_vec()), ("guide.md".to_string(), b"mine".to_vec())]);
        let left = noted_after_failure(Some(&before), &hashes(&lib), Some(&now));
        assert_eq!(left[SKILL_FILE], hashes(&lib)[SKILL_FILE]);
        assert_eq!(left["guide.md"], before["guide.md"], "the person's file is not noted as Luna's");
        assert!(!left.contains_key("old.md"));
    }

    /// A link or junction inside a skill's folder of Luna's (a person's
    /// `data` pointing elsewhere) is no part of the copy: the copy is still
    /// kept up with the library, and once no longer wanted Luna's files go
    /// while the link, and what it leads to, stay.
    #[test]
    fn a_link_inside_a_copy_is_read_past_and_left() {
        let f = Fixture::new("link-inside");
        f.skill("review", "v1");
        let lib = LibrarySettings { skills: skills(&["review"]), ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        let copy = f.account(0).join("skills").join("review");
        let elsewhere = f.base.join("elsewhere");
        write_file(&elsewhere.join("keep"), b"theirs").unwrap();
        link_dir(&copy.join("data"), &elsewhere);

        assert_eq!(Fixture::status(&f.run(&lib, &mut ledger, true), &copy), Status::Current);
        f.skill("review", "v2");
        let p = f.run(&lib, &mut ledger, true);
        assert_eq!(Fixture::status(&p, &copy), Status::Current, "{:?}", p.places);
        assert!(std::fs::read_to_string(copy.join(SKILL_FILE)).unwrap().contains("v2"));

        f.run(&LibrarySettings::default(), &mut ledger, true);
        assert!(!copy.join(SKILL_FILE).exists());
        assert!(std::fs::symlink_metadata(copy.join("data")).is_ok(), "the link stays");
        assert!(elsewhere.join("keep").is_file());
        assert!(walk(&f.dirs.backups()).is_empty());
        unlink_dir(&copy.join("data"));
    }

    /// An account Luna does not list for the moment, its folder still there:
    /// its copies, changed there or not, and a Claude Code account's copies
    /// waiting in `library-folders/` for its sessions in a folder, keep their
    /// notes as they are, and are Luna's again once it is back.
    #[test]
    fn an_account_not_listed_keeps_its_notes() {
        let f = Fixture::new("absent");
        f.instructions("main", "Be brief.\n");
        f.skill("review", "v1");
        f.skill("notes", "v1");
        let repo = f.base.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let folder = LibraryFolder {
            path: repo.to_string_lossy().into_owned(),
            skills: BTreeMap::from([("notes".into(), reach(&["claude/home"]))]),
            slug: "repo".into(),
            ..Default::default()
        };
        let lib = LibrarySettings { instructions: first(all()), skills: skills(&["review"]), folders: vec![folder], ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        std::fs::write(f.account(2).join("CLAUDE.md"), "Mine.\n").unwrap();
        let staged = claude_folder_dir(&f.dirs, "repo", &f.accounts[2]).join(".claude").join("skills").join("notes");
        assert!(staged.join(SKILL_FILE).is_file(), "the folder's copy for its sessions");
        let before = ledger.written.clone();
        let two = &f.accounts[..2];
        let run_two = |ledger: &mut Ledger| {
            let units = units(&f.dirs, &lib, two, &read_skills(&f.dirs), &read_instructions(&f.dirs), ledger);
            pass(&f.dirs, two, &lib.folders, &units, &lib.skipped, ledger, true)
        };
        run_two(&mut ledger);
        assert_eq!(ledger.written, before, "its notes stay as they were");
        assert_eq!(f.read(2, "CLAUDE.md"), "Mine.\n");
        assert!(f.account(2).join("skills").join("review").join(SKILL_FILE).is_file());
        assert!(staged.join(SKILL_FILE).is_file(), "its folder copy waits too");
        assert!(walk(&f.dirs.backups()).is_empty());

        f.run(&lib, &mut ledger, true);
        assert_eq!(f.read(2, "CLAUDE.md"), "Be brief.\n", "Luna's copy, written over");
        assert_eq!(backed_up(&f.dirs), ["Mine.\n"]);
    }

    /// A listed folder that lies in an account (its `skills`, listed as a
    /// folder of its own): a copy of Luna's in it, changed there and no
    /// longer wanted, is that folder's own and stays.
    #[test]
    fn a_listed_folder_inside_an_account_keeps_a_changed_copy() {
        let f = Fixture::new("listed-inside");
        f.skill("review", "v1");
        let lib = LibrarySettings { skills: skills(&["review"]), ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        let copy = f.account(0).join("skills").join("review");
        std::fs::write(copy.join(SKILL_FILE), "the team's").unwrap();
        let listed =
            LibraryFolder { path: f.account(0).join("skills").to_string_lossy().into_owned(), slug: "s".into(), ..Default::default() };
        let none = LibrarySettings { folders: vec![listed], ..Default::default() };
        f.run(&none, &mut ledger, true);
        assert_eq!(std::fs::read_to_string(copy.join(SKILL_FILE)).unwrap(), "the team's");
        assert!(!f.account(1).join("skills").join("review").exists(), "elsewhere Luna's goes");
        assert!(ledger.written.is_empty());
        assert!(walk(&f.dirs.backups()).is_empty());
    }

    /// A folder's copies changed where they wait (Claude Code's in
    /// `library-folders/`, Codex's in the account's plugin): written over
    /// while wanted, taken out once not, each backed up first.
    #[test]
    fn a_folders_changed_copies_are_written_over_and_taken_out() {
        let f = Fixture::new("folder-changed");
        f.skill("review", "v1");
        let repo = f.base.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let folder = |skills: BTreeMap<String, Reach>| LibraryFolder {
            path: repo.to_string_lossy().into_owned(),
            skills,
            slug: "repo".into(),
            ..Default::default()
        };
        let lib = LibrarySettings { folders: vec![folder(BTreeMap::from([("review".into(), all())]))], ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        let claude = claude_folder_dir(&f.dirs, "repo", &f.accounts[0]).join(".claude").join("skills").join("review");
        let codex = codex_plugin_dir(&f.accounts[1], "repo").join("skills").join("review");
        for (c, was) in [(&claude, "claude's edit"), (&codex, "codex's edit")] {
            std::fs::write(c.join(SKILL_FILE), was).unwrap();
        }
        f.run(&lib, &mut ledger, true);
        let backed = backed_up(&f.dirs);
        for (c, was) in [(&claude, "claude's edit"), (&codex, "codex's edit")] {
            assert!(std::fs::read_to_string(c.join(SKILL_FILE)).unwrap().contains("v1"), "{}", c.display());
            assert!(backed.iter().any(|b| b == was), "{was}: {backed:?}");
            std::fs::write(c.join(SKILL_FILE), format!("{was}, again")).unwrap();
        }

        let none = LibrarySettings { folders: vec![folder(BTreeMap::new())], ..Default::default() };
        f.run(&none, &mut ledger, true);
        let backed = backed_up(&f.dirs);
        for (c, was) in [(&claude, "claude's edit"), (&codex, "codex's edit")] {
            assert!(!c.exists(), "{}", c.display());
            assert!(backed.iter().any(|b| *b == format!("{was}, again")), "{was}: {backed:?}");
        }
        assert!(ledger.written.is_empty());
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
    fn a_skill_with_no_skill_md_is_laid_out_nowhere_and_its_copies_stay() {
        let f = Fixture::new("no-skill-md");
        f.skill("review", "v1");
        let lib = LibrarySettings { skills: skills(&["review"]), ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        std::fs::remove_file(f.dirs.skills().join("review/SKILL.md")).unwrap();
        let p = f.run(&lib, &mut ledger, true);
        assert!(p.places.iter().all(|x| x.status == Status::Failed));
        assert!(f.account(0).join("skills/review/SKILL.md").is_file(), "a file moved for a moment takes nothing out");
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
