//! Whose sessions a library entry is for: every session (the default, so
//! nothing changes until the person chooses), or some of the three roles —
//! the person's own chats, leads (helpers started with Luna's tools) and
//! helpers. A session briefed by an agent, not the person: rules written for
//! the person's chat (its language, its report format, a plan agreed with
//! them first) and skills whose own descriptions say it never loads them only
//! cost it context and mislead it.
//!
//! Where each entry goes:
//! - Every session's: laid into the accounts its reach takes, one
//!   instructions file per account (reach.rs `chosen`), as ever.
//! - Some roles': given per session, never laid into an account, as one of
//!   the role layers its start composes (role.rs): Claude Code reads it as
//!   memory of a folder of the session's own, Codex in its
//!   `developer_instructions`. Every such entry whose audience takes the role
//!   and whose reach takes the account goes in, in name order.
//!   The person's chats' alone go so too, on both CLIs: an earlier Luna laid
//!   those into accounts with a line saying whose they were, which a Codex
//!   helper read all the same. The copies it left are Luna's own in the
//!   ledger and no longer wanted, so a pass takes one still as Luna wrote it
//!   out (pass.rs `pass`). One edited in the account since is the person's:
//!   left where it is, and read by every session on the account. Where an
//!   every-session entry wants that path, `edited_chats_copy` recognizes
//!   an edited old chats-only copy before the pass lays out instructions.
//!
//! What each CLI allows for what is in an account:
//! - Claude Code leaves a file out of one session (`claudeMdExcludes`) and
//!   turns a skill off for one (`skillOverrides`), both in the settings it is
//!   started with: `folder_args` hides what is not for the session's role.
//! - Codex turns a skill off for one run (`skills.config`), but reads its
//!   account's `AGENTS.md` in every session with no switch to leave it out
//!   (0.167.0: `CodexHomeUserInstructionsProvider` in `codex-rs/codex-home`,
//!   installed unconditionally by the app-server): only every session's
//!   instructions go there.

use super::*;

pub use crate::settings::Role;

/// What a list of roles is called where Luna says whose an entry is.
fn label(audience: Audience) -> String {
    if audience.is_every() {
        return "every session".into();
    }
    let names: Vec<&str> = audience
        .roles()
        .into_iter()
        .map(|r| match r {
            Role::Chat => "chats",
            Role::Lead => "leads",
            Role::Helper => "helpers",
        })
        .collect();
    format!("{} only", names.join(" and "))
}

pub(super) fn instructions_audience(lib: &LibrarySettings, name: &str) -> Audience {
    lib.audience.instructions.get(name).copied().unwrap_or_default()
}

pub(super) fn skill_audience(lib: &LibrarySettings, name: &str) -> Audience {
    lib.audience.skills.get(name).copied().unwrap_or_default()
}

/// Whether the instructions `name` are laid into accounts: every session's
/// alone. The rest reach their sessions one session at a time.
pub(super) fn laid_into_accounts(lib: &LibrarySettings, name: &str) -> bool {
    instructions_audience(lib, name).is_every()
}

/// The lines earlier Lunas put before the person's chats' instructions they
/// laid into accounts: the last one's, then the one's before roles.
const CHATS_LINES: [&str; 2] = [
    "> For chats with a person only: a session whose task came from another agent skips this whole file.",
    "> For the person's own chats only: a helper session — one another agent started and briefed, through Luna or as a \
     subagent — skips this whole file.",
];

/// Whether `there`, read at `dest`, a copy the ledger notes as Luna's
/// (`noted`), is one of the account copies an earlier Luna laid out for the
/// person's chats alone, edited there since: an instructions file changed
/// from what Luna wrote, where either the file still opens on one of
/// `CHATS_LINES`, whatever an editor made of its line ends or put before it
/// (a byte order mark), or Luna's note is of such a line before instructions
/// as the library has them now. Used where every-session instructions want
/// the same path. If the line is gone and the old text has changed, was
/// deleted, or cannot be read or listed, the hash cannot identify the copy.
pub(super) fn edited_chats_copy(dirs: &Dirs, dest: &Path, there: &Files, noted: &Hashes) -> bool {
    let named = Provider::ALL.into_iter().any(|p| dest.file_name().is_some_and(|f| f == instructions_name(p)));
    let Some(text) = there.get("").filter(|_| named && is_file(noted) && hashes(there) != *noted) else { return false };
    let text = text.strip_prefix("\u{feff}".as_bytes()).unwrap_or(text);
    let opens = |line: &str| text.strip_prefix(line.as_bytes()).is_some_and(|t| t.starts_with(b"\n") || t.starts_with(b"\r\n"));
    let wrote = |line: &str, of: &[u8]| hashes(&Files::from([(String::new(), [line.as_bytes(), b"\n\n", of].concat())])) == *noted;
    CHATS_LINES.into_iter().any(opens)
        || read_instructions(dirs)
            .is_ok_and(|found| found.into_values().filter_map(|t| t.ok().flatten()).any(|t| CHATS_LINES.into_iter().any(|l| wrote(l, &t))))
}

/// Said once when the pass gives an edited instructions copy to the person.
pub(super) fn log_edited_instructions_kept(key: &str) {
    #[cfg(test)]
    tests::LEFT.with(|l| l.borrow_mut().push(key.to_string()));
    crate::log::info("library", &format!("left {key} as it is, no longer Luna's: edited there since Luna laid it out"));
}

/// What the library keeps from this session for its audience, as said in
/// what Luna applied.
pub(super) fn kept_note(what: &str, name: &str, audience: Audience) -> String {
    format!("{what} {name} ({})", label(audience))
}

/// What the library gives this session for its role alone, as said in what
/// Luna applied.
pub(super) fn given_note(name: &str, audience: Audience) -> String {
    format!("instructions {name} ({}): given to this session alone", label(audience))
}

/// What a start says of the audience it went by (`SessionSkills::audience`)
/// and of the skills it could not hide (`SessionSkills::unhidden`).
pub fn audience_applied(s: &SessionSkills) -> Vec<crate::applied::Applied> {
    let mut out = Vec::new();
    if !s.audience.is_empty() {
        out.push(crate::applied::Item::LibraryAudience.with(s.audience.join("; ")));
    }
    if !s.unhidden.is_empty() {
        out.push(crate::applied::Item::LibraryHiddenTrimmed.with(format!("{} stay visible: {}", s.unhidden.len(), s.unhidden.join(", "))));
    }
    out
}

/// Why instructions `name`, given per session now, cannot be made every
/// session's: an account, a provider or every account (`all`) their reach
/// names would get, in every account it takes in, the file of instructions
/// laid into accounts already that name it as closely or more (reach.rs
/// `chosen`: `codex` and `claude` together take every account from `all`),
/// or none, as set to get none. Per session they went beside those; laid into
/// accounts they would compete for the account's one file, and lose it there
/// to reach nobody. Refused rather than settled for the person, as a reach is
/// given only by their choosing it (reach.rs `give_instructions`).
pub(super) fn every_session_refused(lib: &LibrarySettings, name: &str) -> Option<String> {
    if laid_into_accounts(lib, name) {
        return None;
    }
    let reach = lib.instructions.get(name)?;
    let words = |r: &str| match r {
        ALL => "every account".to_string(),
        r if !r.contains('/') => format!("every {r} account"),
        r => r.to_string(),
    };
    let laid: Vec<(&String, &Reach)> = lib.instructions.iter().filter(|(n, _)| n.as_str() != name && laid_into_accounts(lib, n)).collect();
    let mut clashes: Vec<String> = Vec::new();
    for r in reach {
        let mut by: BTreeSet<&str> = BTreeSet::new();
        let mut none = false;
        let mut wins = false;
        for a in stand_ins(lib, r) {
            let c = closeness(std::slice::from_ref(r), &a);
            let over: Vec<&str> = laid.iter().filter(|(_, theirs)| closeness(theirs, &a) >= c).map(|(n, _)| n.as_str()).collect();
            let kept_out = closeness(&lib.no_instructions, &a) >= c;
            if over.is_empty() && !kept_out {
                wins = true;
                break;
            }
            by.extend(over);
            none |= kept_out;
        }
        if wins {
            continue;
        }
        let names: Vec<&str> = by.into_iter().collect();
        let goes = if names.len() == 1 { "goes" } else { "go" };
        clashes.push(match (names.is_empty(), none) {
            (true, _) => format!("{} is set to get no instructions", words(r)),
            (false, false) => format!("{} already {goes} into {}", names.join(" and "), words(r)),
            (false, true) => format!("{} already {goes} into {}, and the rest of it is set to get none", names.join(" and "), words(r)),
        });
    }
    (!clashes.is_empty()).then(|| {
        format!(
            "{name} cannot be for every session while {}: an account's own file holds one of them, and {name} would reach nobody \
             there. Change {name}'s accounts or the other's first.",
            clashes.join(", and ")
        )
    })
}

/// An account of `provider` called `name`, for `stand_ins`.
fn stand_in(provider: Provider, name: &str) -> AccountInfo {
    AccountInfo { provider, name: name.into(), path: String::new() }
}

/// One account of each kind selector `r` takes in, as reach.rs `closeness`
/// tells them apart: each account a reach of the library names, and for each
/// provider one named nowhere (no name, which no selector has), standing for
/// all its others, there now or added later.
fn stand_ins(lib: &LibrarySettings, r: &str) -> Vec<AccountInfo> {
    let named: BTreeSet<&str> = lib.instructions.values().flatten().chain(&lib.no_instructions).map(String::as_str).collect();
    Provider::ALL
        .into_iter()
        .flat_map(|p| {
            let names = named.iter().filter_map(move |x| x.split_once('/').filter(|(q, _)| *q == p.as_str()).map(|(_, n)| n));
            names.chain([""]).map(move |n| stand_in(p, n)).collect::<Vec<_>>()
        })
        .filter(|a| reaches(&[r.to_string()], a))
        .collect()
}

/// Whose sessions a skill or instructions file is for; every session's drops
/// it from the settings. Instructions given any other audience stop being
/// laid into accounts: from then on they reach their sessions one session at
/// a time. Instructions made every session's where that would leave them
/// reaching nobody are refused (`every_session_refused`).
pub fn library_set_audience(entry: Entry, audience: Audience) -> Result<Overview, String> {
    let (skill, name) = match entry {
        Entry::Skill(n) => (true, n),
        Entry::Instructions(n) => (false, n),
        // Served only where it is due, the same to every chat it is served to.
        Entry::System(n) => return Err(format!("{n} is a system text: it has no audience to pick")),
    };
    if skill {
        check_skill_name(&name)?;
    } else {
        check_name(&name)?;
    }
    let mut refused = None;
    let overview = change(|l| {
        if !skill && audience.is_every() {
            refused = every_session_refused(l, &name);
            if refused.is_some() {
                return;
            }
        }
        let map = if skill { &mut l.audience.skills } else { &mut l.audience.instructions };
        match audience.is_every() {
            true => map.remove(&name),
            false => map.insert(name.clone(), audience),
        };
    })?;
    if let Some(why) = refused {
        return Err(why);
    }
    crate::log::info("library", &format!("{} {name}: for {}", if skill { "skill" } else { "instructions" }, label(audience)));
    Ok(overview)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEADS_AND_HELPERS: Audience = Audience { chat: false, lead: true, helper: true };

    #[test]
    fn instructions_made_every_session_s_where_they_would_reach_nobody_are_refused() {
        let mut lib = LibrarySettings::default();
        lib.instructions.insert("main".into(), all());
        lib.instructions.insert("review".into(), all());
        lib.audience.instructions.insert("review".into(), Audience::of(&[Role::Helper]));
        let why = every_session_refused(&lib, "review").expect("both would want every account's file");
        assert!(why.contains("main already goes into every account") && why.contains("review would reach nobody"), "{why}");
        assert_eq!(every_session_refused(&lib, "main"), None, "laid into accounts already");
        // Named more closely, it wins those accounts: nothing to refuse.
        lib.instructions.insert("review".into(), vec!["claude/work".into()]);
        assert_eq!(every_session_refused(&lib, "review"), None);
        // An account set to get none would keep it out as well.
        lib.no_instructions = vec!["claude/work".into()];
        assert!(every_session_refused(&lib, "review").unwrap().contains("claude/work is set to get no instructions"));
        lib.instructions.insert("review".into(), vec!["codex".into()]);
        lib.instructions.insert("main".into(), vec!["codex".into()]);
        assert!(every_session_refused(&lib, "review").unwrap().contains("main already goes into every codex account"));
        // Named by none alike, but shadowed all the same: `main` for both
        // providers takes every account from `review`'s `all`, more closely.
        lib.no_instructions.clear();
        lib.audience.instructions.insert("review".into(), Audience::of(&[Role::Helper]));
        lib.instructions.insert("main".into(), vec!["claude".into(), "codex".into()]);
        lib.instructions.insert("review".into(), all());
        let why = every_session_refused(&lib, "review").expect("main wins every account");
        assert!(why.contains("main already goes into every account") && why.contains("nobody there"), "{why}");
        // One provider's alone leaves it the other's accounts.
        lib.instructions.insert("main".into(), vec!["claude".into()]);
        assert_eq!(every_session_refused(&lib, "review"), None);
        // Each account named, and the rest of that provider set to get none.
        lib.instructions.insert("review".into(), vec!["codex".into()]);
        lib.instructions.insert("main".into(), vec!["codex/home".into()]);
        lib.no_instructions = vec!["codex".into()];
        let why = every_session_refused(&lib, "review").unwrap();
        assert!(why.contains("main already goes into every codex account, and the rest of it is set to get none"), "{why}");
        lib.no_instructions.clear();
        assert_eq!(every_session_refused(&lib, "review"), None, "every other codex account is its");
    }

    #[test]
    fn an_audience_takes_its_roles_and_every_takes_every_role() {
        for role in Role::ALL {
            assert!(Audience::EVERY.takes(role));
            assert_eq!(Audience::of(&[role]).roles(), [role]);
            for other in Role::ALL.into_iter().filter(|r| *r != role) {
                assert!(!Audience::of(&[role]).takes(other), "{role:?} {other:?}");
                assert!(Audience::of(&[role, other]).takes(other));
            }
        }
        assert_eq!(label(Audience::EVERY), "every session");
        assert_eq!(label(LEADS_AND_HELPERS), "leads and helpers only");
    }

    #[test]
    fn only_every_session_s_instructions_go_into_accounts() {
        let mut lib = LibrarySettings::default();
        assert!(laid_into_accounts(&lib, "main"), "every session's");
        for audience in [Audience::CHATS, LEADS_AND_HELPERS, Audience::of(&[Role::Chat, Role::Helper])] {
            lib.audience.instructions.insert("main".into(), audience);
            assert!(!laid_into_accounts(&lib, "main"), "{audience:?}: given per session");
        }
    }

    /// Instructions made the person's chats' alone, or leads' and helpers',
    /// leave the accounts: Luna's own copies go, in a Claude Code and a Codex
    /// account alike.
    #[test]
    fn instructions_for_some_roles_leave_the_accounts() {
        for (tag, audience) in [("chats", Audience::CHATS), ("helpers", LEADS_AND_HELPERS)] {
            let f = Fixture::new(&format!("audience-leave-{tag}"));
            f.instructions("main", "Be brief.\n");
            let mut lib = LibrarySettings { instructions: first(all()), ..Default::default() };
            let mut ledger = Ledger::default();
            f.run(&lib, &mut ledger, true);
            assert_eq!(f.read(1, "AGENTS.md"), "Be brief.\n");
            lib.audience.instructions.insert("main".into(), audience);
            f.run(&lib, &mut ledger, true);
            assert!(!f.account(1).join("AGENTS.md").exists(), "{tag}: Luna's own copy goes: given per session from now on");
            assert!(!f.account(0).join("CLAUDE.md").exists(), "{tag}");
        }
    }

    /// The lines earlier Lunas put before the person's chats' instructions
    /// they laid into accounts: the last one's, and the one's before roles.
    const FOR_CHATS_LAST: &str = CHATS_LINES[0];
    const FOR_CHATS_BEFORE_ROLES: &str = CHATS_LINES[1];

    thread_local! {
        /// The copies `log_edited_instructions_kept` said were left, in this test.
        pub(super) static LEFT: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    /// Account `i`'s instructions file.
    fn copy_in(f: &Fixture, i: usize) -> PathBuf {
        f.account(i).join(instructions_name(f.accounts[i].provider))
    }

    /// The settings and ledger an earlier Luna left with `main` for the
    /// person's chats laid into every account, read as this Luna reads the
    /// file (its `laidOut` passed over), each copy opening on `line` and
    /// noted as Luna's.
    fn left_before(f: &Fixture, line: &str) -> (LibrarySettings, Ledger) {
        f.instructions("main", "Be brief.\n");
        let file =
            r#"{"library": {"instructions": true, "audience": {"roles": {"instructions": {"main": ["chat"]}}}, "laidOut": ["main"]}}"#;
        let lib = serde_json::from_str::<crate::settings::Settings>(file).unwrap().library;
        assert_eq!(lib.audience.instructions["main"], Audience::CHATS);
        let mut ledger = Ledger::default();
        let old = format!("{line}\n\nBe brief.\n");
        for i in 0..f.accounts.len() {
            let dest = copy_in(f, i);
            std::fs::write(&dest, &old).unwrap();
            ledger.wrote(&dest.to_string_lossy(), hashes(&Files::from([(String::new(), old.clone().into_bytes())])));
        }
        (lib, ledger)
    }

    /// A pass that does nothing: nothing written or taken out, the ledger as
    /// it was, no backup made and no copy said to be left.
    fn does_nothing(f: &Fixture, lib: &LibrarySettings, ledger: &mut Ledger) {
        let (backups, notes) = (backed_up(&f.dirs), ledger.written.clone());
        LEFT.with(|l| l.borrow_mut().clear());
        let p = f.run(lib, ledger, true);
        assert!(p.written.is_empty() && p.removed.is_empty() && p.not_removed.is_empty());
        assert_eq!(backed_up(&f.dirs), backups);
        assert_eq!(ledger.written, notes);
        assert!(LEFT.with(|l| l.borrow().is_empty()));
    }

    /// The person's chats' instructions an earlier Luna laid into accounts,
    /// each copy still as it wrote it, with either of its lines: looking
    /// takes nothing out; the first pass takes every copy out, in a Claude
    /// Code and a Codex account alike, with nothing backed up (the library
    /// has them); a second does nothing.
    #[test]
    fn chats_copies_laid_out_before_go_once_as_luna_wrote_them() {
        for (tag, line) in [("last", FOR_CHATS_LAST), ("roles", FOR_CHATS_BEFORE_ROLES)] {
            let f = Fixture::new(&format!("audience-move-{tag}"));
            let (lib, mut ledger) = left_before(&f, line);
            let p = f.run(&lib, &mut ledger, false);
            assert!(p.places.is_empty(), "{tag}: no account wants a copy: {:?}", p.places);
            assert!((0..3).all(|i| copy_in(&f, i).is_file()), "{tag}: looking takes nothing out");

            let p = f.run(&lib, &mut ledger, true);
            let mut removed: Vec<PathBuf> = p.removed.iter().map(PathBuf::from).collect();
            removed.sort();
            let mut copies: Vec<PathBuf> = (0..3).map(|i| copy_in(&f, i)).collect();
            copies.sort();
            assert_eq!(removed, copies, "{tag}: a pass says what it took out");
            assert!(p.not_removed.is_empty(), "{tag}: {:?}", p.not_removed);
            assert!(copies.iter().all(|c| !c.exists()), "{tag}");
            assert!(ledger.written.is_empty(), "{tag}: no note left");
            assert!(backed_up(&f.dirs).is_empty(), "{tag}: nothing of the person's in them");
            assert_eq!(std::fs::read_to_string(f.dirs.instructions_file("main")).unwrap(), "Be brief.\n", "{tag}: the library's own stays");
            does_nothing(&f, &lib, &mut ledger);
        }
    }

    /// The copies edited in the account since, the line kept, whatever an
    /// editor made of it (line ends turned to CRLF, a byte order mark put
    /// before it): each is the person's now, left byte for byte as it is,
    /// nothing backed up, its note dropped and its being left said once. No
    /// session on the account leaves it out, whatever its role; the chats
    /// still get the instructions per session. Looking first changes nothing.
    #[test]
    fn a_chats_copy_edited_there_is_the_person_s_and_said_once() {
        let f = Fixture::new("audience-move-changed");
        let (lib, mut ledger) = left_before(&f, FOR_CHATS_LAST);
        let edits = [
            format!("{FOR_CHATS_LAST}\n\nBe brief. And kind.\n"),
            format!("{FOR_CHATS_LAST}\r\n\r\nBe brief.\r\n"),
            format!("\u{feff}{FOR_CHATS_LAST}\n\nBe brief.\n"),
        ];
        for (i, e) in edits.iter().enumerate() {
            std::fs::write(copy_in(&f, i), e).unwrap();
        }
        let notes = ledger.written.clone();
        f.run(&lib, &mut ledger, false);
        assert_eq!(ledger.written, notes, "a look drops no note");
        assert!(LEFT.with(|l| l.borrow().is_empty()));

        let p = f.run(&lib, &mut ledger, true);
        assert!(p.removed.is_empty() && p.not_removed.is_empty(), "{:?} {:?}", p.removed, p.not_removed);
        for (i, e) in edits.iter().enumerate() {
            assert_eq!(std::fs::read(copy_in(&f, i)).unwrap(), e.as_bytes(), "account {i}: untouched");
        }
        assert!(backed_up(&f.dirs).is_empty(), "nothing taken out, nothing backed up");
        assert!(ledger.written.is_empty(), "Luna's no longer");
        let mut left = LEFT.with(|l| l.take());
        left.sort();
        let mut copies: Vec<String> = (0..3).map(|i| copy_in(&f, i).to_string_lossy().into_owned()).collect();
        copies.sort();
        assert_eq!(left, copies, "each said once");
        does_nothing(&f, &lib, &mut ledger);

        std::fs::create_dir_all(&f.dirs.data).unwrap();
        ledger.save(&f.dirs).unwrap();
        let elsewhere = f.base.join("elsewhere").to_string_lossy().into_owned();
        for (i, a) in f.accounts.iter().enumerate() {
            for role in Role::ALL {
                let s = folder_args(&f.dirs, &lib, a, &elsewhere, role);
                assert!(s.excluded.is_empty() && s.instructions.is_none(), "account {i} {role:?}: read as the account's own");
                assert_eq!(s.role_entries.is_empty(), role != Role::Chat, "account {i} {role:?}");
            }
        }
    }

    /// An edited copy where every session's instructions go now (a broader
    /// entry the chats' one won the account's file from): the person's, left
    /// as it is and said to be someone else's (`taken`), never written over,
    /// not then nor at the next pass. A copy as Luna wrote it there is
    /// Luna's, written over with the new entry's.
    #[test]
    fn an_edited_chats_copy_is_not_written_over_by_every_session_s_instructions() {
        let f = Fixture::new("audience-move-wanted");
        let (mut lib, mut ledger) = left_before(&f, FOR_CHATS_LAST);
        lib.instructions.insert("main".into(), vec!["claude/work".into(), "codex/home".into(), "claude/home".into()]);
        f.instructions("team", "Team rules.\n");
        lib.instructions.insert("team".into(), all());
        let edited = format!("{FOR_CHATS_LAST}\r\n\r\nBe brief. Mine.\r\n");
        std::fs::write(copy_in(&f, 0), &edited).unwrap();
        let p = f.run(&lib, &mut ledger, true);
        assert_eq!(std::fs::read_to_string(copy_in(&f, 0)).unwrap(), edited, "left as it is");
        assert_eq!(Fixture::status(&p, &copy_in(&f, 0)), Status::Taken);
        assert!(!ledger.written.contains_key(&*copy_in(&f, 0).to_string_lossy()), "Luna's no longer");
        assert_eq!(LEFT.with(|l| l.take()), [copy_in(&f, 0).to_string_lossy().into_owned()]);
        for i in [1, 2] {
            assert_eq!(std::fs::read_to_string(copy_in(&f, i)).unwrap(), "Team rules.\n", "account {i}: written over as Luna's");
        }
        assert!(backed_up(&f.dirs).is_empty());
        does_nothing(&f, &lib, &mut ledger);
        assert_eq!(std::fs::read_to_string(copy_in(&f, 0)).unwrap(), edited);

        // A later library edit matching those bytes must leave them owned
        // by the person, even though the old ownership note is gone.
        f.instructions("team", &edited);
        let p = f.run(&lib, &mut ledger, true);
        assert_eq!(Fixture::status(&p, &copy_in(&f, 0)), Status::Taken);
        assert!(!ledger.written.contains_key(&*copy_in(&f, 0).to_string_lossy()));
    }

    /// Where every session's instructions want the path, the old chats-only
    /// copy can be recognized by the hash of the line plus the old text.
    /// The removal step keeps any edited instructions copy.
    #[test]
    fn a_chats_copy_rewritten_without_the_line_is_the_person_s_while_its_note_tells() {
        for wanted in [false, true] {
            let f = Fixture::new(&format!("audience-move-unlined-{wanted}"));
            let (mut lib, mut ledger) = left_before(&f, FOR_CHATS_BEFORE_ROLES);
            if wanted {
                f.instructions("team", "Team rules.\n");
                lib.instructions.insert("team".into(), all());
            }
            std::fs::write(copy_in(&f, 0), "My own rules now.\n").unwrap();
            f.run(&lib, &mut ledger, true);
            assert_eq!(f.read(0, "CLAUDE.md"), "My own rules now.\n", "wanted: {wanted}");
            assert!(backed_up(&f.dirs).is_empty(), "wanted: {wanted}");
            assert_eq!(LEFT.with(|l| l.take()), [copy_in(&f, 0).to_string_lossy().into_owned()]);
            does_nothing(&f, &lib, &mut ledger);
        }
        let f = Fixture::new("audience-move-unlined-changed");
        let (lib, mut ledger) = left_before(&f, FOR_CHATS_LAST);
        f.instructions("main", "Be briefer.\n");
        std::fs::write(copy_in(&f, 0), "My own rules now.\n").unwrap();
        f.run(&lib, &mut ledger, true);
        assert_eq!(f.read(0, "CLAUDE.md"), "My own rules now.\n");
        assert!(ledger.written.is_empty() && backed_up(&f.dirs).is_empty());
        assert_eq!(LEFT.with(|l| l.take()), [copy_in(&f, 0).to_string_lossy().into_owned()]);
        does_nothing(&f, &lib, &mut ledger);
    }

    #[test]
    fn old_chats_copies_use_the_hash_when_the_library_listing_fails() {
        let f = Fixture::new("audience-move-listing-fails");
        let (lib, mut ledger) = left_before(&f, FOR_CHATS_LAST);
        std::fs::remove_file(f.dirs.instructions_file("main")).unwrap();
        std::fs::remove_dir(f.dirs.instructions()).unwrap();
        std::fs::write(f.dirs.instructions(), "A file in the folder's place.\n").unwrap();
        assert!(read_instructions(&f.dirs).is_err());
        assert_unlined_copy_is_kept(&f, &lib, &mut ledger);
    }

    #[test]
    fn old_chats_copies_use_the_hash_when_the_library_entry_is_deleted() {
        let f = Fixture::new("audience-move-deleted");
        let (mut lib, mut ledger) = left_before(&f, FOR_CHATS_LAST);
        std::fs::remove_file(f.dirs.instructions_file("main")).unwrap();
        lib.instructions.clear();
        lib.audience.instructions.clear();
        assert_unlined_copy_is_kept(&f, &lib, &mut ledger);
    }

    fn assert_unlined_copy_is_kept(f: &Fixture, lib: &LibrarySettings, ledger: &mut Ledger) {
        let edited = "My own rules now.\n";
        std::fs::write(copy_in(f, 0), edited).unwrap();
        let p = f.run(lib, ledger, true);
        assert_eq!(p.removed.len(), 2, "the unedited copies go: {:?}", p.removed);
        assert!(p.not_removed.is_empty());
        assert_eq!(f.read(0, "CLAUDE.md"), edited);
        assert!(ledger.written.is_empty() && backed_up(&f.dirs).is_empty());
        assert_eq!(LEFT.with(|l| l.take()), [copy_in(f, 0).to_string_lossy().into_owned()]);
        does_nothing(f, lib, ledger);
    }

    /// The copies in accounts taken off Luna's list keep their notes and stay
    /// untouched; listed again, one as Luna wrote it is taken out, and one
    /// edited meanwhile left as the person's.
    #[test]
    fn a_chats_copy_in_an_account_not_listed_waits_for_it() {
        let f = Fixture::new("audience-move-unlisted");
        let (lib, mut ledger) = left_before(&f, FOR_CHATS_LAST);
        let listed = &f.accounts[..1];
        let units = units(&f.dirs, &lib, listed, &read_skills(&f.dirs), &read_instructions(&f.dirs));
        pass(&f.dirs, listed, &lib.folders, &units, &mut ledger, true);
        assert!(!copy_in(&f, 0).exists());
        assert!(copy_in(&f, 1).is_file() && copy_in(&f, 2).is_file(), "not listed: untouched");
        assert_eq!(ledger.written.len(), 2, "their notes kept");
        let edited = "Mine.\n";
        std::fs::write(copy_in(&f, 2), edited).unwrap();
        f.instructions("main", "Library rules changed meanwhile.\n");
        f.run(&lib, &mut ledger, true);
        assert!(!copy_in(&f, 1).exists());
        assert_eq!(std::fs::read_to_string(copy_in(&f, 2)).unwrap(), edited);
        assert!(ledger.written.is_empty() && backed_up(&f.dirs).is_empty());
        assert_eq!(LEFT.with(|l| l.take()), [copy_in(&f, 2).to_string_lossy().into_owned()]);
        does_nothing(&f, &lib, &mut ledger);
    }

    /// A session running on the account holds nothing back: a pass writes
    /// and takes out an account's instructions with sessions running there as
    /// ever, and every start is preceded by one. A copy saved there just as
    /// the pass comes to it keeps the save and its note; the next pass finds
    /// it edited, and leaves it.
    #[test]
    fn a_chats_copy_goes_with_a_session_on_its_account_and_one_saved_meanwhile_is_left() {
        let f = Fixture::new("audience-move-in-use");
        let (lib, mut ledger) = left_before(&f, FOR_CHATS_LAST);
        let _running: Vec<crate::account_use::Running> = f.accounts.iter().map(|a| crate::account_use::running_on(&a.path)).collect();
        assert!(f.accounts.iter().all(|a| crate::account_use::running(&a.path)));
        let codex = copy_in(&f, 1);
        let at = codex.clone();
        let saved = format!("{FOR_CHATS_LAST}\n\nSaved meanwhile.\n");
        let mut once = Some(saved.clone());
        BEFORE_EACH.with(|h| {
            *h.borrow_mut() = Some(Box::new(move |p: &Path| {
                if p == at {
                    if let Some(bytes) = once.take() {
                        std::fs::write(&at, bytes).unwrap();
                    }
                }
            }))
        });
        let p = f.run(&lib, &mut ledger, true);
        BEFORE_EACH.with(|h| *h.borrow_mut() = None);
        assert!(!copy_in(&f, 0).exists() && !copy_in(&f, 2).exists(), "taken out under their sessions");
        assert_eq!(std::fs::read_to_string(&codex).unwrap(), saved, "the save stays");
        assert!(p.not_removed.iter().any(|e| e.contains("changed there")), "{:?}", p.not_removed);
        assert!(ledger.written.contains_key(&*codex.to_string_lossy()), "still Luna's, for the next pass");

        let p = f.run(&lib, &mut ledger, true);
        assert!(p.removed.is_empty());
        assert_eq!(std::fs::read_to_string(&codex).unwrap(), saved, "edited: left where it is");
        assert!(backed_up(&f.dirs).is_empty());
        does_nothing(&f, &lib, &mut ledger);
    }

    /// A copy held open where Windows lets no one delete it (a program
    /// reading it without sharing deletion): not taken out, its note kept;
    /// once it is let go, the next pass takes it out.
    #[cfg(windows)]
    #[test]
    fn a_chats_copy_held_open_goes_at_the_pass_after_it_is_let_go() {
        use std::os::windows::fs::OpenOptionsExt;
        let f = Fixture::new("audience-move-held");
        let (lib, mut ledger) = left_before(&f, FOR_CHATS_LAST);
        let codex = copy_in(&f, 1);
        // FILE_SHARE_READ alone: no deleting it while held.
        let held = std::fs::OpenOptions::new().read(true).share_mode(1).open(&codex).unwrap();
        let p = f.run(&lib, &mut ledger, true);
        assert!(codex.is_file());
        assert!(p.not_removed.iter().any(|e| e.starts_with(&*codex.to_string_lossy())), "{:?}", p.not_removed);
        assert!(ledger.written.contains_key(&*codex.to_string_lossy()));
        assert!(backed_up(&f.dirs).is_empty(), "still as Luna wrote it");
        drop(held);
        let p = f.run(&lib, &mut ledger, true);
        assert_eq!(p.removed, [codex.to_string_lossy().into_owned()]);
        assert!(!codex.exists());
        does_nothing(&f, &lib, &mut ledger);
    }

    /// A link put in an old copy's place, to a file of someone else's that
    /// holds just what Luna wrote: neither that file nor the link is taken
    /// out or backed up through it, and the note stays for a later pass.
    /// (Windows makes a file link only with Developer Mode or as an
    /// administrator; without, there is nothing to test.)
    #[test]
    fn a_link_in_a_chats_copy_s_place_is_left_alone() {
        let f = Fixture::new("audience-move-link");
        let (lib, mut ledger) = left_before(&f, FOR_CHATS_LAST);
        let codex = copy_in(&f, 1);
        let theirs = f.base.join("theirs.md");
        std::fs::rename(&codex, &theirs).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&theirs, &codex).unwrap();
        #[cfg(windows)]
        if std::os::windows::fs::symlink_file(&theirs, &codex).is_err() {
            return;
        }
        let was = std::fs::read(&theirs).unwrap();
        for _ in 0..2 {
            let p = f.run(&lib, &mut ledger, true);
            assert!(p.not_removed.iter().any(|e| e.contains("link")), "{:?}", p.not_removed);
        }
        assert!(std::fs::symlink_metadata(&codex).unwrap().file_type().is_symlink(), "the link stays");
        assert_eq!(std::fs::read(&theirs).unwrap(), was, "what it leads to stays as it was");
        assert!(ledger.written.contains_key(&*codex.to_string_lossy()));
        assert!(backed_up(&f.dirs).is_empty(), "nothing read through it into the backups");
        std::fs::remove_file(&codex).unwrap();
    }

    /// Once the copies are gone, the person's chats on both CLIs get the
    /// instructions per session, in their role layers, and leads and helpers
    /// get none of them, with nothing in the account left to read.
    #[test]
    fn after_the_move_chats_get_the_chats_instructions_per_session_and_helpers_do_not() {
        let f = Fixture::new("audience-after-move");
        let (lib, mut ledger) = left_before(&f, FOR_CHATS_LAST);
        f.run(&lib, &mut ledger, true);
        let elsewhere = f.base.join("elsewhere").to_string_lossy().into_owned();
        for (i, a) in f.accounts.iter().enumerate() {
            assert!(!copy_in(&f, i).exists());
            let chat = folder_args(&f.dirs, &lib, a, &elsewhere, Role::Chat);
            assert_eq!(chat.role_entries, [("main".to_string(), "Be brief.\n".to_string())], "{:?}", a.provider);
            assert_eq!(chat.audience, ["instructions main (chats only): given to this session alone"]);
            assert!(chat.excluded.is_empty() && chat.instructions.is_none());
            for role in [Role::Lead, Role::Helper] {
                let s = folder_args(&f.dirs, &lib, a, &elsewhere, role);
                assert!(s.role_entries.is_empty() && s.excluded.is_empty() && s.instructions.is_none(), "{:?} {role:?}", a.provider);
                assert!(s.audience.is_empty(), "{:?}", s.audience);
            }
        }
    }

    /// `main` for the person's chats, given per session; `report` (theirs),
    /// `cleanup` (leads' and helpers') and `review` (every
    /// session's) into every account; and a repository giving every account's
    /// sessions there `solo`, the person's chats' alone, which no account has
    /// as a whole.
    fn audiences(f: &Fixture) -> (LibrarySettings, String, String) {
        f.instructions("main", "Speak Russian to me.\n");
        for s in ["report", "cleanup", "review", "solo"] {
            f.skill(s, "v1");
        }
        let repo = f.base.join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let folder = LibraryFolder {
            path: repo.to_string_lossy().into_owned(),
            skills: BTreeMap::from([("solo".into(), all())]),
            slug: "repo".into(),
            ..Default::default()
        };
        let mut lib = LibrarySettings {
            instructions: first(all()),
            skills: skills(&["report", "cleanup", "review"]),
            folders: vec![folder],
            ..Default::default()
        };
        lib.audience.instructions.insert("main".into(), Audience::CHATS);
        lib.audience.skills.extend([
            ("report".into(), Audience::CHATS),
            ("cleanup".into(), LEADS_AND_HELPERS),
            ("solo".into(), Audience::CHATS),
        ]);
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        codex_manifests(&lib, &f.accounts, &mut ledger);
        let elsewhere = f.base.join("elsewhere").to_string_lossy().into_owned();
        (lib, repo.to_string_lossy().into_owned(), elsewhere)
    }

    #[test]
    fn a_claude_helper_is_kept_from_what_is_meant_for_chats_and_a_chat_from_what_is_meant_for_helpers() {
        let f = Fixture::new("audience-claude");
        let (lib, repo, elsewhere) = audiences(&f);
        let a = &f.accounts[0];
        for role in [Role::Helper, Role::Lead] {
            let helper = folder_args(&f.dirs, &lib, a, &elsewhere, role);
            assert_eq!(helper.hidden, ["report"]);
            assert!(helper.excluded.is_empty(), "the account's own file holds nothing of the chats'");
            assert_eq!(helper.instructions, None);
            assert!(helper.role_entries.is_empty());
            assert_eq!(helper.audience, ["skill report (chats only)"]);
        }
        let chat = folder_args(&f.dirs, &lib, a, &elsewhere, Role::Chat);
        assert_eq!(chat.hidden, ["cleanup"]);
        assert!(chat.excluded.is_empty());
        assert_eq!(chat.role_entries, [("main".to_string(), "Speak Russian to me.\n".to_string())]);
        assert_eq!(
            chat.audience,
            ["instructions main (chats only): given to this session alone", "skill cleanup (leads and helpers only)"]
        );
        // In the repository a folder's skill meant for chats is left out of the
        // helper's plugin, so there is nothing of it to hide, and said all the same.
        let helper = folder_args(&f.dirs, &lib, a, &repo, Role::Helper);
        assert_eq!(helper.hidden, ["report"]);
        assert!(helper.plugins.is_empty() && helper.args.is_empty(), "the folder gives it nothing but `solo`");
        assert!(helper.audience.contains(&"skill solo (chats only)".to_string()), "{:?}", helper.audience);
        let chat = folder_args(&f.dirs, &lib, a, &repo, Role::Chat);
        assert_eq!(chat.hidden, ["cleanup"]);
        let given: Vec<(&str, Vec<&str>)> =
            chat.plugins.iter().map(|p| (p.slug.as_str(), p.skills.keys().map(String::as_str).collect())).collect();
        assert_eq!(given, [("repo", vec!["solo"])]);
    }

    #[test]
    fn a_codex_helper_is_kept_from_what_is_meant_for_chats_and_a_chat_from_what_is_meant_for_helpers() {
        let f = Fixture::new("audience-codex");
        let (lib, repo, elsewhere) = audiences(&f);
        let a = &f.accounts[1];
        // Hidden by the path of the skill's SKILL.md, wherever the account has it.
        let at = |skills: PathBuf, name: &str| skills.join(name).join("SKILL.md").to_string_lossy().replace('\\', "/");
        let own = |name: &str| at(f.account(1).join("skills"), name);
        let off = |paths: &[String]| {
            let entries: Vec<String> = paths.iter().map(|p| format!("{{path=\"{p}\",enabled=false}}")).collect();
            ["-c".to_string(), format!("skills.config=[{}]", entries.join(","))]
        };
        let helper = folder_args(&f.dirs, &lib, a, &elsewhere, Role::Helper);
        assert_eq!(helper.args, off(&[own("report")]));
        assert_eq!(helper.hidden, [own("report")]);
        assert!(helper.excluded.is_empty(), "Codex reads the account's instructions");
        assert!(helper.role_entries.is_empty());
        assert!(!f.account(1).join("AGENTS.md").exists(), "nothing of the chats' in the account for Codex to read");
        assert_eq!(helper.audience, ["skill report (chats only)"]);
        let chat = folder_args(&f.dirs, &lib, a, &elsewhere, Role::Chat);
        assert_eq!(chat.args, off(&[own("cleanup")]));
        assert_eq!(chat.role_entries, [("main".to_string(), "Speak Russian to me.\n".to_string())]);
        assert_eq!(
            chat.audience,
            ["instructions main (chats only): given to this session alone", "skill cleanup (leads and helpers only)"]
        );
        // The repository's plugin copy of `solo` is hidden by its path too, in
        // the plugin the folder's name makes.
        let helper = folder_args(&f.dirs, &lib, a, &repo, Role::Helper);
        let solo = at(codex_plugin_dir(a, "repo").join("skills"), "solo");
        let mut hidden = vec![own("report"), solo];
        hidden.sort();
        assert_eq!(helper.args[..2], ["-c", "plugins.repo@rm-luna.enabled=true"]);
        assert_eq!(helper.args[2..], off(&hidden));
        assert_eq!(helper.hidden, hidden);
    }

    #[test]
    fn with_every_entry_for_every_session_each_role_gets_what_it_did() {
        let f = Fixture::new("audience-none");
        let (mut lib, repo, elsewhere) = audiences(&f);
        lib.audience = Default::default();
        for a in &f.accounts {
            for folder in [&repo, &elsewhere] {
                let chat = folder_args(&f.dirs, &lib, a, folder, Role::Chat);
                for role in [Role::Lead, Role::Helper] {
                    assert_eq!(folder_args(&f.dirs, &lib, a, folder, role), chat);
                }
                assert!(chat.audience.is_empty() && chat.hidden.is_empty() && chat.excluded.is_empty() && chat.role_entries.is_empty());
            }
        }
    }

    #[test]
    fn instructions_for_some_roles_are_given_per_session_in_name_order() {
        let f = Fixture::new("audience-per-session");
        f.instructions("main", "Everyone.\n");
        f.instructions("b-lead", "Split the work.\n");
        f.instructions("a-lead", "Check what they report.\n");
        f.instructions("helper", "Be brief.\n");
        f.instructions("blank", "");
        let mut lib = LibrarySettings { instructions: first(all()), ..Default::default() };
        for (n, reach) in [("b-lead", all()), ("a-lead", vec!["claude".to_string()]), ("helper", all()), ("blank", all())] {
            lib.instructions.insert(n.into(), reach);
        }
        lib.audience.instructions.extend([
            ("b-lead".into(), Audience::of(&[Role::Lead])),
            ("a-lead".into(), Audience::of(&[Role::Lead])),
            ("helper".into(), Audience::of(&[Role::Helper])),
            ("blank".into(), Audience::of(&[Role::Lead])),
        ]);
        f.run(&lib, &mut Ledger::default(), true);
        assert_eq!(f.read(0, "CLAUDE.md"), "Everyone.\n", "the account's file is every session's");
        let elsewhere = f.base.join("elsewhere").to_string_lossy().into_owned();
        let entries = |lib: &LibrarySettings, i: usize, role| folder_args(&f.dirs, lib, &f.accounts[i], &elsewhere, role).role_entries;
        let pair = |n: &str, t: &str| (n.to_string(), t.to_string());
        assert_eq!(entries(&lib, 0, Role::Lead), [pair("a-lead", "Check what they report.\n"), pair("b-lead", "Split the work.\n")]);
        assert_eq!(entries(&lib, 1, Role::Lead), [pair("b-lead", "Split the work.\n")], "a-lead's reach is Claude Code's");
        assert_eq!(entries(&lib, 1, Role::Helper), [pair("helper", "Be brief.\n")]);
        assert!(entries(&lib, 0, Role::Chat).is_empty());
        let lead = folder_args(&f.dirs, &lib, &f.accounts[0], &elsewhere, Role::Lead);
        assert!(lead.excluded.is_empty(), "the account's own is every session's: read as ever");
        assert_eq!(
            lead.audience,
            [
                "instructions a-lead (leads only): given to this session alone",
                "instructions b-lead (leads only): given to this session alone"
            ]
        );
        // An account whose own file holds none still gets its roles'.
        lib.no_instructions = vec!["claude/work".into()];
        assert_eq!(entries(&lib, 0, Role::Lead).len(), 2);
        // And giving one of them an account takes it from no other.
        give_instructions(&mut lib, Some("helper"), vec!["claude/work".into()]);
        assert_eq!(lib.instructions["b-lead"], all(), "per session, they do not compete");
        assert_eq!(lib.instructions["main"], all());
        assert_eq!(lib.no_instructions, ["claude/work".to_string()]);
    }

    #[test]
    fn instructions_a_folder_gives_that_are_not_for_the_session_are_none() {
        let f = Fixture::new("audience-folder-instructions");
        let (mut lib, repo, _) = audiences(&f);
        f.instructions("terse", "Say less.\n");
        lib.audience.instructions = BTreeMap::from([("terse".into(), Audience::CHATS)]);
        lib.folders[0].instructions = BTreeMap::from([("claude".into(), Some("terse".into()))]);
        let a = &f.accounts[0];
        let chat = folder_args(&f.dirs, &lib, a, &repo, Role::Chat);
        assert_eq!(chat.instructions, Some(f.dirs.instructions_file("terse")));
        let helper = folder_args(&f.dirs, &lib, a, &repo, Role::Helper);
        assert_eq!(helper.instructions, None);
        assert_eq!(helper.excluded, chat.excluded, "the account's left out all the same");
        assert_eq!(helper.audience[0], "instructions terse (chats only)");
    }
}
