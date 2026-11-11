//! Which accounts a skill or an instructions file is meant for: all of them, a
//! provider's, or ones picked by name, and of several instructions files the one
//! that names an account most closely.

use super::*;

pub(super) const ALL: &str = "all";

/// How closely a reach takes in this account: 2 by its name, 1 by its
/// provider, 0 as one of all; None when it does not.
pub(super) fn closeness(reach: &[String], a: &AccountInfo) -> Option<u8> {
    let provider = a.provider.as_str();
    reach
        .iter()
        .filter_map(|r| match r {
            r if r.split_once('/') == Some((provider, a.name.as_str())) => Some(2),
            r if r == provider => Some(1),
            r if r == ALL => Some(0),
            _ => None,
        })
        .max()
}

/// Whether a reach takes in this account.
pub(super) fn reaches(reach: &[String], a: &AccountInfo) -> bool {
    closeness(reach, a).is_some()
}

/// Each entry `all`, a provider or `provider/account`.
pub(super) fn check_reach(reach: &[String]) -> Result<(), String> {
    let provider = |p: &str| Provider::ALL.iter().any(|x| x.as_str() == p);
    for r in reach {
        let fits = r == ALL || provider(r) || r.split_once('/').is_some_and(|(p, n)| provider(p) && !n.is_empty() && !n.contains('/'));
        if !fits {
            return Err(format!("{r} is not all, a provider or provider/account"));
        }
    }
    Ok(())
}

/// A reach as the user ticked it, checked and made the shortest that takes in
/// the same accounts: `all` alone, a provider without its own accounts,
/// nothing twice.
pub(super) fn tidy_reach(reach: Vec<String>) -> Result<Reach, String> {
    check_reach(&reach)?;
    if reach.iter().any(|r| r == ALL) {
        return Ok(vec![ALL.into()]);
    }
    let mut out: Reach = Vec::new();
    for r in &reach {
        let covered = r.split_once('/').is_some_and(|(p, _)| reach.iter().any(|x| x == p));
        if !covered && !out.contains(r) {
            out.push(r.clone());
        }
    }
    Ok(out)
}

/// Gives one instructions (None: no instructions at all) this reach, and
/// takes each of its entries away from every other: an account named, a
/// provider and `all` are each one instructions' at a time, so where two
/// reaches meet on an account the closer one decides and never a tie. An
/// account is named beside `all` or its provider on purpose here, unlike in
/// a skill's reach — named, it beats another's provider — so nothing but
/// repeats is tidied away.
pub(super) fn give_instructions(lib: &mut LibrarySettings, name: Option<&str>, mut reach: Reach) {
    let mut seen = BTreeSet::new();
    reach.retain(|r| seen.insert(r.clone()));
    for (n, r) in lib.instructions.iter_mut() {
        if Some(n.as_str()) != name {
            r.retain(|x| !reach.contains(x));
        }
    }
    match name {
        Some(n) => {
            lib.no_instructions.retain(|x| !reach.contains(x));
            lib.instructions.insert(n.to_string(), reach);
        }
        None => lib.no_instructions = reach,
    }
    lib.instructions.retain(|_, r| !r.is_empty());
}

/// The instructions an account gets, of those `have` says the library has:
/// the one whose reach takes it in most closely, or none when that is
/// `no_instructions`' (which wins a tie, as the safer side). Two at one
/// closeness only come of settings edited by hand: the first by name wins.
pub(super) fn chosen<'a>(lib: &'a LibrarySettings, have: impl Fn(&str) -> bool, a: &AccountInfo) -> Option<&'a str> {
    let mut best: Option<(u8, Option<&str>)> = closeness(&lib.no_instructions, a).map(|c| (c, None));
    for (name, reach) in lib.instructions.iter().filter(|(n, _)| have(n)) {
        if let Some(c) = closeness(reach, a) {
            if best.is_none_or(|(b, _)| c > b) {
                best = Some((c, Some(name.as_str())));
            }
        }
    }
    best.and_then(|(_, n)| n)
}

/// An account renamed (`to`) or deleted (None), as `provider/name`: every
/// reach that named it follows. A folder's reach left empty stays, keeping
/// the skill from every account there, as it kept it from all but that one.
pub fn rename_account(lib: &mut LibrarySettings, from: &str, to: Option<&str>) {
    let folders = lib.folders.iter_mut().flat_map(|f| f.skills.values_mut());
    let reaches =
        lib.instructions.values_mut().chain(lib.skills.values_mut()).chain(std::iter::once(&mut lib.no_instructions)).chain(folders);
    for reach in reaches {
        match to {
            Some(to) => reach.iter_mut().filter(|r| *r == from).for_each(|r| *r = to.to_string()),
            None => reach.retain(|r| r != from),
        }
    }
    lib.instructions.retain(|_, reach| !reach.is_empty());
    lib.skills.retain(|_, reach| !reach.is_empty());
    for f in &mut lib.folders {
        if let Some(choice) = f.instructions.remove(from) {
            if let Some(to) = to {
                f.instructions.insert(to.to_string(), choice);
            }
        }
    }
}

/// What a rename of an account changes in the settings, for accounts.rs to
/// make in the same write as its own: the reaches (`rename_account`) and the
/// places left out.
pub(crate) type CarryAlong<'a> = &'a dyn Fn(&mut LibrarySettings);

/// The account `provider/name` at `from` renamed `to`, under the library's
/// lock from before its folder moves (`move_folder`, which gives the new
/// folder) to after the settings are written (`settings`, given what the
/// library changes in them): a pass in between would take the copies at the
/// new path for someone else's and forget what it knew of the old one. The
/// ledger's notes under the folder follow it; so do a Claude Code account's
/// folder copies, outside it but named after it (`move_folder_copies`), and
/// the places left out among all of them. Settings that will not write put
/// everything back as it was. With no lock, nothing is done.
pub(crate) fn rename_account_held(
    provider: Provider,
    from: &Path,
    name: &str,
    to: &str,
    move_folder: impl FnOnce() -> Result<PathBuf, String>,
    settings: impl FnOnce(CarryAlong) -> Result<(), String>,
) -> Result<PathBuf, String> {
    let _guard = lock_or("The account was not renamed")?;
    carry_rename(&Dirs::current(), provider, from, name, to, move_folder, settings)
}

fn carry_rename(
    dirs: &Dirs,
    provider: Provider,
    from: &Path,
    name: &str,
    to: &str,
    move_folder: impl FnOnce() -> Result<PathBuf, String>,
    settings: impl FnOnce(CarryAlong) -> Result<(), String>,
) -> Result<PathBuf, String> {
    let (before, mut ledger) = ledger_as_it_is(dirs)?;
    let dest = move_folder()?;
    let mut moves = vec![(from.to_path_buf(), dest.clone())];
    if provider == Provider::Claude {
        moves.extend(move_folder_copies(dirs, name, to));
    }
    for (a, b) in &moves {
        ledger.moved(a, b);
    }
    // A folder copy's notes where its folder is not there (none laid out
    // yet, or taken out by hand) follow the name too; those of one that
    // stayed where it is (a link) stay with it.
    if provider == Provider::Claude {
        ledger.rekeyed(|k| folder_copy_renamed(dirs, k, name, to).filter(|_| !old_folder_copies_there(dirs, k)));
    }
    let (old_key, new_key) = (format!("{}/{name}", provider.as_str()), format!("{}/{to}", provider.as_str()));
    let carry = |lib: &mut LibrarySettings| {
        rename_account(lib, &old_key, Some(&new_key));
        // A place left out is the person's word on where a copy goes, whether
        // or not anything lies there: it follows the name wherever it does.
        for place in &mut lib.skipped {
            let moved = moves.iter().find_map(|(a, b)| moved_under(place, a, b));
            let folder = || (provider == Provider::Claude).then(|| folder_copy_renamed(dirs, place, name, to)).flatten();
            if let Some(moved) = moved.or_else(folder) {
                *place = moved;
            }
        }
    };
    match ledger.save(dirs).and_then(|()| settings(&carry)) {
        Ok(()) => Ok(dest),
        Err(e) => Err(match put_back(dirs, before.as_deref(), &moves) {
            Ok(()) => e,
            Err(left) => format!("{e}. Not all of it could be put back: {left}"),
        }),
    }
}

/// A rename undone: each folder moved back, the account's last (it is what
/// the chats on the old name point at), then the ledger as it was, or —
/// where a folder would not move back — as it was with that folder's notes
/// where the folder now is, so that the ledger and the disk agree. What would
/// not, said.
fn put_back(dirs: &Dirs, before: Option<&[u8]>, moves: &[(PathBuf, PathBuf)]) -> Result<(), String> {
    let mut stuck = Vec::new();
    let mut said = Vec::new();
    for (a, b) in moves.iter().rev() {
        if let Err(e) = rename_back(b, a) {
            said.push(format!("{} is still there, not at {}: {e}", b.display(), a.display()));
            stuck.push((a, b));
        }
    }
    let restored = match before {
        Some(bytes) if stuck.is_empty() => crate::secret::write_private(&dirs.ledger(), bytes).map_err(|e| e.to_string()),
        None if stuck.is_empty() => match std::fs::remove_file(dirs.ledger()) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        },
        _ => {
            let mut ledger: Ledger = before.and_then(|b| serde_json::from_slice(b).ok()).unwrap_or_default();
            for (a, b) in &stuck {
                ledger.moved(a, b);
            }
            ledger.save(dirs)
        }
    };
    if let Err(e) = restored {
        said.push(format!("{} holds the notes under the new name: {e}", dirs.ledger().display()));
    }
    for line in &said {
        crate::log::warn("library", &format!("undoing an account's rename: {line}"));
    }
    if said.is_empty() {
        Ok(())
    } else {
        Err(said.join("; "))
    }
}

// A folder a test makes refuse to move back.
#[cfg(test)]
thread_local! {
    static STUCK: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

fn rename_back(from: &Path, to: &Path) -> std::io::Result<()> {
    #[cfg(test)]
    if STUCK.with(|s| s.borrow().as_deref() == Some(from)) {
        return Err(std::io::Error::other("held for the test"));
    }
    std::fs::rename(from, to)
}

/// The ledger's bytes (None: there is none yet) and what they say, for a
/// rename or a deletion to change and save. Unlike `Ledger::load`, one that
/// will not read or parse is an error, before anything moves: taken for
/// empty, it would be saved empty, every note lost.
fn ledger_as_it_is(dirs: &Dirs) -> Result<(Option<Vec<u8>>, Ledger), String> {
    let path = dirs.ledger();
    match std::fs::read(&path) {
        Ok(bytes) => {
            let ledger = serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
            Ok((Some(bytes), ledger))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((None, Ledger::default())),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// The account `provider/name` at `dir` deleted (`remove`), under the
/// library's lock: its reaches and the places left out in it go with it, in
/// `settings`, then the ledger's notes under its folder. Each step can be
/// done again: a deletion that failed partway is finished by the next, the
/// folder gone already. Its folder copies outside it are a pass's to take
/// out, as copies no longer wanted.
pub(crate) fn delete_account_held(
    provider: Provider,
    dir: &Path,
    name: &str,
    remove: impl FnOnce() -> Result<(), String>,
    settings: impl FnOnce(CarryAlong) -> Result<(), String>,
) -> Result<(), String> {
    let _guard = lock_or("The account was not deleted")?;
    carry_delete(&Dirs::current(), provider, dir, name, remove, settings)
}

fn carry_delete(
    dirs: &Dirs,
    provider: Provider,
    dir: &Path,
    name: &str,
    remove: impl FnOnce() -> Result<(), String>,
    settings: impl FnOnce(CarryAlong) -> Result<(), String>,
) -> Result<(), String> {
    let (_, mut ledger) = ledger_as_it_is(dirs)?;
    remove()?;
    let key = format!("{}/{name}", provider.as_str());
    settings(&|lib: &mut LibrarySettings| {
        rename_account(lib, &key, None);
        lib.skipped.retain(|place| !within(Path::new(place), dir));
    })?;
    if ledger.dropped(dir) {
        ledger.save(dirs)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reach_takes_in_all_a_provider_or_one_account() {
        let f = Fixture::new("reach");
        f.skill("review", "v1");
        f.instructions("main", "Be brief.\n");
        let lib = LibrarySettings {
            instructions: first(reach(&["codex"])),
            skills: BTreeMap::from([("review".into(), reach(&["claude/work"]))]),
            ..Default::default()
        };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        assert!(!f.account(0).join("CLAUDE.md").exists());
        assert!(f.account(1).join("AGENTS.md").is_file());
        assert!(f.account(0).join("skills/review/SKILL.md").is_file());
        assert!(!f.account(1).join("skills/review").exists());

        // Narrowed: the account no longer meant goes, as for a skill unticked.
        let lib = LibrarySettings { skills: BTreeMap::from([("review".into(), reach(&["codex/home"]))]), ..lib };
        f.run(&lib, &mut ledger, true);
        assert!(!f.account(0).join("skills/review").exists());
        assert!(f.account(1).join("skills/review/SKILL.md").is_file());

        assert_eq!(tidy_reach(reach(&["claude/work", "all"])).unwrap(), all());
        assert_eq!(tidy_reach(reach(&["claude/work", "codex/home", "claude", "codex/home"])).unwrap(), reach(&["codex/home", "claude"]));
        assert!(tidy_reach(reach(&["gemini"])).is_err());
        assert!(tidy_reach(reach(&["claude/"])).is_err());
    }

    #[test]
    fn a_renamed_account_keeps_its_reach_and_a_deleted_one_leaves_it() {
        let mut lib = LibrarySettings {
            instructions: BTreeMap::from([("main".into(), reach(&["claude/work", "codex"])), ("work".into(), reach(&["claude/work"]))]),
            no_instructions: reach(&["claude/work"]),
            skills: BTreeMap::from([("review".into(), vec!["claude/work".into()]), ("notes".into(), all())]),
            ..Default::default()
        };
        rename_account(&mut lib, "claude/work", Some("claude/office"));
        assert_eq!(lib.instructions["main"], reach(&["claude/office", "codex"]));
        assert_eq!(lib.instructions["work"], reach(&["claude/office"]));
        assert_eq!(lib.no_instructions, reach(&["claude/office"]));
        assert_eq!(lib.skills["review"], vec!["claude/office".to_string()]);
        rename_account(&mut lib, "claude/office", None);
        assert_eq!(lib.instructions["main"], reach(&["codex"]));
        assert!(!lib.instructions.contains_key("work"), "instructions that go nowhere any more are dropped");
        assert!(lib.no_instructions.is_empty());
        assert!(!lib.skills.contains_key("review"), "a skill that goes nowhere any more is dropped");
        assert_eq!(lib.skills["notes"], all());
    }

    #[test]
    fn an_account_gets_the_instructions_that_name_it_most_closely() {
        let f = Fixture::new("closest");
        f.instructions("main", "For everyone.\n");
        f.instructions("claude", "For Claude Code.\n");
        f.instructions("work", "For work.\n");
        let mut lib = LibrarySettings {
            instructions: BTreeMap::from([
                ("main".into(), all()),
                ("claude".into(), reach(&["claude"])),
                ("work".into(), reach(&["claude/work"])),
            ]),
            ..Default::default()
        };
        let mut ledger = Ledger::default();
        let p = f.run(&lib, &mut ledger, true);
        assert_eq!(f.read(0, "CLAUDE.md"), "For work.\n", "named beats a provider and all");
        assert_eq!(f.read(2, "CLAUDE.md"), "For Claude Code.\n", "a provider beats all");
        assert_eq!(f.read(1, "AGENTS.md"), "For everyone.\n");
        assert_eq!(p.places.iter().filter(|x| x.instructions.is_some()).count(), 3, "one instructions file per account");

        // Named as getting none: Luna's copy goes, though `all` takes it in.
        lib.no_instructions = reach(&["claude/home"]);
        f.run(&lib, &mut ledger, true);
        assert!(!f.account(2).join("CLAUDE.md").exists());

        // Instructions not in the library compete for nothing: the next
        // closest has the account.
        std::fs::remove_file(f.dirs.instructions_file("work")).unwrap();
        f.run(&lib, &mut ledger, true);
        assert_eq!(f.read(0, "CLAUDE.md"), "For Claude Code.\n");

        // Two at one closeness (settings edited by hand): none wins over
        // instructions, the first by name over the rest.
        lib.instructions.insert("alpha".into(), reach(&["claude/home"]));
        lib.instructions.insert("beta".into(), reach(&["codex/home"]));
        lib.instructions.insert("gamma".into(), reach(&["codex/home"]));
        let have = |_: &str| true;
        assert_eq!(chosen(&lib, have, &f.accounts[2]), None);
        assert_eq!(chosen(&lib, have, &f.accounts[1]), Some("beta"));
    }

    #[test]
    fn giving_instructions_a_reach_takes_it_from_the_others() {
        let mut lib = LibrarySettings { instructions: first(reach(&["all", "codex/home"])), ..Default::default() };
        give_instructions(&mut lib, Some("work"), reach(&["claude/work"]));
        assert_eq!(lib.instructions["main"], reach(&["all", "codex/home"]), "other entries stay");
        assert_eq!(lib.instructions["work"], reach(&["claude/work"]));

        // A provider and an account of it are two entries: both stand.
        give_instructions(&mut lib, Some("cli"), reach(&["claude"]));
        assert_eq!(lib.instructions["work"], reach(&["claude/work"]));

        // Main named for the work account: work loses it, and goes nowhere.
        give_instructions(&mut lib, Some("main"), reach(&["all", "codex/home", "claude/work", "claude/work"]));
        assert_eq!(lib.instructions["main"], reach(&["all", "codex/home", "claude/work"]), "kept beside all, once");
        assert!(!lib.instructions.contains_key("work"));

        // None for an account takes it from instructions, and back.
        give_instructions(&mut lib, None, reach(&["codex/home"]));
        assert_eq!(lib.instructions["main"], reach(&["all", "claude/work"]));
        give_instructions(&mut lib, Some("cli"), reach(&["claude", "codex/home"]));
        assert!(lib.no_instructions.is_empty());

        // `all` is one instructions' at a time.
        give_instructions(&mut lib, Some("cli"), reach(&["all"]));
        assert_eq!(lib.instructions["main"], reach(&["claude/work"]));
        assert_eq!(lib.instructions["cli"], all());
        give_instructions(&mut lib, Some("cli"), Vec::new());
        assert!(!lib.instructions.contains_key("cli"));
    }

    /// A library laid out on a Claude Code and a Codex account with all a
    /// rename has to carry: instructions for the person's chats alone (their
    /// line in the copy), a copy changed there, one no longer wanted but
    /// changed (kept), a place left out, a skill named for the accounts, and a
    /// folder's skill waiting for their sessions there. The ledger saved.
    fn laid_for_a_rename(f: &Fixture) -> LibrarySettings {
        for s in ["review", "old", "spare", "notes"] {
            f.skill(s, "v1");
        }
        f.instructions("main", "Be brief.\n");
        let repo = f.base.join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let mut lib = LibrarySettings {
            instructions: first(all()),
            skills: BTreeMap::from([
                ("review".into(), reach(&["claude/work", "codex/home"])),
                ("old".into(), all()),
                ("spare".into(), all()),
            ]),
            folders: vec![LibraryFolder {
                path: repo.to_string_lossy().into_owned(),
                skills: skills(&["notes"]),
                slug: "repo".into(),
                ..Default::default()
            }],
            skipped: [0, 1].iter().map(|&i| f.account(i).join("skills").join("spare").to_string_lossy().into_owned()).collect(),
            ..Default::default()
        };
        lib.audience.instructions.insert("main".into(), Audience::Chats);
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        for i in [0, 1] {
            std::fs::write(f.account(i).join("skills/review/SKILL.md"), "mine\n").unwrap();
            std::fs::write(f.account(i).join("skills/old/SKILL.md"), "mine too\n").unwrap();
        }
        lib.skills.remove("old");
        let p = f.run(&lib, &mut ledger, true);
        assert_eq!(p.kept.len(), 2, "{:?}", p.kept);
        ledger.save(&f.dirs).unwrap();
        lib
    }

    /// Account `i` renamed `to` as accounts.rs renames one, its settings
    /// written as `write` says.
    fn rename(f: &mut Fixture, i: usize, to: &str, lib: &mut LibrarySettings, write: Result<(), String>) -> Result<PathBuf, String> {
        let a = f.accounts[i].clone();
        let from = PathBuf::from(&a.path);
        let dest = from.with_file_name(to);
        let moved = carry_rename(
            &f.dirs,
            a.provider,
            &from,
            &a.name,
            to,
            || std::fs::rename(&from, &dest).map(|()| dest.clone()).map_err(|e| e.to_string()),
            |carry| write.map(|()| carry(lib)),
        );
        if moved.is_ok() {
            f.accounts[i] = AccountInfo { provider: a.provider, name: to.into(), path: dest.to_string_lossy().into_owned() };
        }
        moved
    }

    fn folder_copies(f: &Fixture, i: usize) -> PathBuf {
        claude_folder_dir(&f.dirs, "repo", &f.accounts[i])
    }

    /// After a rename the next pass finds every copy as it left it: changed
    /// still changed rather than someone else's, kept still kept, its line
    /// still Luna's to take off, a place left out still out, and Claude Code's
    /// folder copies under the new name.
    #[test]
    fn a_renamed_account_keeps_all_the_library_knew_of_it() {
        let mut f = Fixture::new("rename-carry");
        let mut lib = laid_for_a_rename(&f);
        let old_copies = folder_copies(&f, 0);
        assert!(old_copies.join(".claude/skills/notes/SKILL.md").is_file());
        rename(&mut f, 0, "office", &mut lib, Ok(())).unwrap();
        rename(&mut f, 1, "house", &mut lib, Ok(())).unwrap();
        assert_eq!(lib.skills["review"], reach(&["claude/office", "codex/house"]));

        let mut ledger = Ledger::load(&f.dirs);
        let p = f.run(&lib, &mut ledger, true);
        for i in [0, 1] {
            let account = f.account(i);
            assert_eq!(Fixture::status(&p, &account.join("skills").join("review")), Status::Changed, "account {i}");
            assert_eq!(Fixture::status(&p, &account.join("skills").join("spare")), Status::Skipped, "account {i}");
            assert!(!account.join("skills").join("spare").exists(), "account {i}: still left out");
            assert!(p.kept.iter().any(|k| Path::new(k) == account.join("skills").join("old")), "account {i}: {:?}", p.kept);
            let file = account.join(instructions_name(f.accounts[i].provider));
            let dest = file.to_string_lossy().into_owned();
            assert_eq!(ledger.worded.get(&dest), Some(&Audience::Chats), "account {i}");
            // What taking it into the library keeps (commands.rs `library_resolve`).
            assert_eq!(unworded(&std::fs::read(&file).unwrap(), ledger.worded[&dest]), b"Be brief.\n");
        }
        let new_copies = folder_copies(&f, 0);
        assert!(!old_copies.exists());
        assert_eq!(Fixture::status(&p, &new_copies.join(".claude").join("skills").join("notes")), Status::Current);
    }

    /// Only the case of the name changes: the ledger's keys take the new
    /// spelling all the same, and so do the folder copies on disk.
    #[test]
    fn a_change_of_case_alone_is_carried_along_too() {
        let mut f = Fixture::new("rename-case");
        let mut lib = laid_for_a_rename(&f);
        rename(&mut f, 0, "Work", &mut lib, Ok(())).unwrap();
        rename(&mut f, 1, "Home", &mut lib, Ok(())).unwrap();
        let mut ledger = Ledger::load(&f.dirs);
        for i in [0, 1] {
            let dest = f.account(i).join(instructions_name(f.accounts[i].provider)).to_string_lossy().into_owned();
            assert!(ledger.written.contains_key(&dest) && ledger.worded.contains_key(&dest), "{dest} under its new spelling");
        }
        let names: Vec<String> = std::fs::read_dir(f.dirs.folders().join("repo"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"Work".to_string()) && !names.contains(&"work".to_string()), "{names:?}");
        let p = f.run(&lib, &mut ledger, true);
        assert_eq!(Fixture::status(&p, &f.account(0).join("skills").join("review")), Status::Changed);
        assert_eq!(Fixture::status(&p, &folder_copies(&f, 0).join(".claude").join("skills").join("notes")), Status::Current);
    }

    /// Settings that will not write after the folders moved: the account's
    /// folder, its folder copies and the ledger are all back as they were.
    #[test]
    fn a_rename_whose_settings_will_not_write_puts_everything_back() {
        let mut f = Fixture::new("rename-back");
        let mut lib = laid_for_a_rename(&f);
        let ledger = std::fs::read(f.dirs.ledger()).unwrap();
        let settings = serde_json::to_string(&lib).unwrap();
        let (account, copies) = (f.account(0), folder_copies(&f, 0));
        let why = rename(&mut f, 0, "office", &mut lib, Err("disk full".into())).unwrap_err();
        assert_eq!(why, "disk full");
        assert!(account.join("CLAUDE.md").is_file(), "the account's folder is back");
        assert!(!account.with_file_name("office").exists());
        assert!(copies.join(".claude/skills/notes/SKILL.md").is_file(), "and its folder copies");
        assert!(!copies.with_file_name("office").exists());
        assert_eq!(std::fs::read(f.dirs.ledger()).unwrap(), ledger, "the ledger as it was");
        assert_eq!(serde_json::to_string(&lib).unwrap(), settings);
    }

    /// A folder copy's place left out, with no folder of copies on disk for the
    /// account (nothing ever laid out there): it follows the name all the same,
    /// and so does a note of Luna's there.
    #[test]
    fn a_folder_copy_left_out_follows_the_name_with_nothing_on_disk() {
        let mut f = Fixture::new("rename-skip-folder");
        let mut lib = laid_for_a_rename(&f);
        let copies = folder_copies(&f, 0);
        let place = copies.join(".claude").join("skills").join("notes");
        lib.skipped.push(place.to_string_lossy().into_owned());
        std::fs::remove_dir_all(&copies).unwrap();
        rename(&mut f, 0, "office", &mut lib, Ok(())).unwrap();
        let new_place = folder_copies(&f, 0).join(".claude").join("skills").join("notes");
        assert!(lib.skipped.iter().any(|s| Path::new(s) == new_place), "{:?}", lib.skipped);
        assert!(lib.skipped.iter().all(|s| !within(Path::new(s), &copies)), "{:?}", lib.skipped);
        assert!(Ledger::load(&f.dirs).written.keys().any(|k| Path::new(k) == new_place));
        let mut ledger = Ledger::load(&f.dirs);
        let p = f.run(&lib, &mut ledger, true);
        assert_eq!(Fixture::status(&p, &new_place), Status::Skipped);
        assert!(!new_place.exists(), "still left out");
    }

    /// A folder that will not move back: the error says where it is, and the
    /// ledger has its notes there, the rest as they were.
    #[test]
    fn a_rename_that_cannot_be_undone_says_where_things_are() {
        let mut f = Fixture::new("rename-stuck");
        let mut lib = laid_for_a_rename(&f);
        let (account, copies) = (f.account(0), folder_copies(&f, 0));
        let office = account.with_file_name("office");
        STUCK.with(|s| *s.borrow_mut() = Some(office.clone()));
        let why = rename(&mut f, 0, "office", &mut lib, Err("disk full".into())).unwrap_err();
        STUCK.with(|s| *s.borrow_mut() = None);
        assert!(why.starts_with("disk full. Not all of it could be put back"), "{why}");
        assert!(why.contains(&*office.to_string_lossy()), "{why}");
        assert!(office.join("CLAUDE.md").is_file() && !account.exists());
        assert!(copies.join(".claude/skills/notes/SKILL.md").is_file(), "the folder copies came back");
        let ledger = Ledger::load(&f.dirs);
        assert!(ledger.written.keys().any(|k| Path::new(k) == office.join("CLAUDE.md")), "the notes where the folder is");
        assert!(ledger.written.keys().all(|k| !within(Path::new(k), &account)));
        assert!(ledger.written.keys().any(|k| within(Path::new(k), &copies)), "the folder copies' as they were");
    }

    /// A ledger that will not save after the folder is gone: the settings are
    /// clean already, and deleting again, the folder gone, finishes the rest.
    #[test]
    fn a_deletion_that_failed_partway_is_finished_by_the_next() {
        let f = Fixture::new("delete-again");
        let mut lib = laid_for_a_rename(&f);
        let dir = f.account(0);
        let remove = || match dir.exists() {
            true => std::fs::remove_dir_all(&dir).map_err(|e| e.to_string()),
            false => Ok(()),
        };
        SAVE_FAILS.with(|s| s.set(true));
        let failed = carry_delete(&f.dirs, Provider::Claude, &dir, "work", remove, |carry| {
            carry(&mut lib);
            Ok(())
        });
        SAVE_FAILS.with(|s| s.set(false));
        assert!(failed.is_err() && !dir.exists());
        assert_eq!(lib.skills["review"], reach(&["codex/home"]), "the settings are clean all the same");
        assert!(Ledger::load(&f.dirs).written.keys().any(|k| within(Path::new(k), &dir)));
        carry_delete(&f.dirs, Provider::Claude, &dir, "work", remove, |carry| {
            carry(&mut lib);
            Ok(())
        })
        .unwrap();
        assert!(Ledger::load(&f.dirs).written.keys().all(|k| !within(Path::new(k), &dir)));
    }

    /// A ledger that will not parse refuses the rename before anything moves,
    /// rather than be saved empty.
    #[test]
    fn a_rename_over_a_ledger_that_will_not_read_moves_nothing() {
        let mut f = Fixture::new("rename-bad-ledger");
        let mut lib = laid_for_a_rename(&f);
        std::fs::write(f.dirs.ledger(), "{ half").unwrap();
        let (account, copies) = (f.account(0), folder_copies(&f, 0));
        assert!(rename(&mut f, 0, "office", &mut lib, Ok(())).is_err());
        assert!(account.join("CLAUDE.md").is_file() && copies.is_dir());
        assert_eq!(std::fs::read_to_string(f.dirs.ledger()).unwrap(), "{ half");
    }

    /// A link where an account's folder copies lie is not moved, nor are its
    /// notes: the rename goes on, and a pass sees to the copies.
    #[test]
    fn a_link_where_the_folder_copies_lie_is_left_alone() {
        let mut f = Fixture::new("rename-link");
        let mut lib = laid_for_a_rename(&f);
        let copies = folder_copies(&f, 0);
        let real = f.base.join("elsewhere");
        std::fs::rename(&copies, &real).unwrap();
        link_dir(&copies, &real);
        let notes = |l: &Ledger| l.written.keys().filter(|k| within(Path::new(k), &copies)).cloned().collect::<Vec<_>>();
        let before = notes(&Ledger::load(&f.dirs));
        assert!(!before.is_empty());
        rename(&mut f, 0, "office", &mut lib, Ok(())).unwrap();
        assert!(std::fs::symlink_metadata(&copies).is_ok_and(|m| crate::boundary::is_link(&m)), "the link stays");
        assert!(!copies.with_file_name("office").exists());
        assert!(real.join(".claude/skills/notes/SKILL.md").is_file());
        assert_eq!(notes(&Ledger::load(&f.dirs)), before, "its notes stay as they were");
        unlink_dir(&copies);
    }

    /// A deleted account's notes and places left out go with it; another
    /// account's stay.
    #[test]
    fn a_deleted_account_takes_what_the_library_knew_of_it_along() {
        let f = Fixture::new("delete-carry");
        let mut lib = laid_for_a_rename(&f);
        let dir = f.account(0);
        carry_delete(
            &f.dirs,
            Provider::Claude,
            &dir,
            "work",
            || std::fs::remove_dir_all(&dir).map_err(|e| e.to_string()),
            |carry| {
                carry(&mut lib);
                Ok(())
            },
        )
        .unwrap();
        let ledger = Ledger::load(&f.dirs);
        let mut notes = ledger.written.keys().chain(ledger.worded.keys()).chain(ledger.kept.keys());
        assert!(notes.all(|k| !within(Path::new(k), &dir)));
        assert!(ledger.written.keys().any(|k| within(Path::new(k), &f.account(1))), "another account's stay");
        assert_eq!(lib.skipped, [f.account(1).join("skills").join("spare").to_string_lossy().into_owned()]);
        assert_eq!(lib.skills["review"], reach(&["codex/home"]));
    }
}
