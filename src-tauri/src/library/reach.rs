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
}
