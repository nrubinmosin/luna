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
//! - Instructions for the person's chats alone that an earlier Luna laid into
//!   accounts stay there (`LibrarySettings::laid_out`), with a line before
//!   them saying whose they are (`worded`): a Codex session reads its
//!   account's `AGENTS.md` whoever's it is, and chats started outside Luna
//!   read them there.
//!
//! What each CLI allows for what is in an account:
//! - Claude Code leaves a file out of one session (`claudeMdExcludes`) and
//!   turns a skill off for one (`skillOverrides`), both in the settings it is
//!   started with: `folder_args` hides what is not for the session's role.
//! - Codex turns a skill off for one run (`-c skills.config`), but reads its
//!   account's `AGENTS.md` in every session with no switch to leave it out
//!   (0.167.0: `CodexHomeUserInstructionsProvider` in `codex-rs/codex-home`,
//!   installed unconditionally by the app-server).

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

/// Whether the instructions `name` are laid into accounts: every session's,
/// or the person's chats' that an earlier Luna laid out (`laid_out`). The
/// rest reach their sessions one session at a time.
pub(super) fn laid_into_accounts(lib: &LibrarySettings, name: &str) -> bool {
    let audience = instructions_audience(lib, name);
    audience.is_every() || (audience == Audience::CHATS && lib.laid_out.contains(name))
}

/// The line an account's copy of the instructions `name` opens on: the
/// person's chats' where they are laid out for them, none for every
/// session's.
pub(super) fn banner_of(lib: &LibrarySettings, name: &str) -> Audience {
    match laid_into_accounts(lib, name) && !instructions_audience(lib, name).is_every() {
        true => Audience::CHATS,
        false => Audience::EVERY,
    }
}

/// What goes before instructions laid into accounts for the person's chats
/// alone. Named by how a session's task came to it, which every session can
/// tell of itself: the note on a task from another agent says so.
const FOR_CHATS: &str = "> For chats with a person only: a session whose task came from another agent skips this whole file.";

/// The lines earlier Lunas put there, for the person's chats and for helpers:
/// a copy still opening on one is Luna's own line, not the person's text.
const FOR_CHATS_BEFORE: &str = "> For the person's own chats only: a helper session — one another agent started and briefed, \
through Luna or as a subagent — skips this whole file.";
const FOR_HELPERS_BEFORE: &str = "> For helper sessions only — those another agent started and briefed, through Luna or as a \
subagent: the person's own chats skip this whole file.";

fn wording(banner: Audience) -> Option<&'static str> {
    (banner == Audience::CHATS).then_some(FOR_CHATS)
}

/// Every line a copy written for `banner` may open on: this Luna's, then
/// earlier ones'.
fn wordings(banner: Audience) -> Vec<&'static str> {
    if banner == Audience::CHATS {
        vec![FOR_CHATS, FOR_CHATS_BEFORE]
    } else if banner == Audience::of(&[Role::Lead, Role::Helper]) {
        vec![FOR_HELPERS_BEFORE]
    } else {
        Vec::new()
    }
}

/// Instructions as their copies in accounts hold them: for the person's
/// chats alone, the line saying whose they are and a blank line before them.
pub(super) fn worded(text: &[u8], banner: Audience) -> Vec<u8> {
    match wording(banner) {
        Some(line) => [line.as_bytes(), b"\n\n", text].concat(),
        None => text.to_vec(),
    }
}

/// A copy's instructions as the library keeps them: without the line `worded`
/// put before them for `banner` — the one Luna's ledger says it wrote there,
/// every session's where it wrote none — this Luna's line or an earlier one's,
/// while that line and the blank one after it are still there word for word,
/// with either line ending (an editor may have turned the copy's into CRLF).
/// Anything else is the person's text, taken whole: the same words in a copy
/// Luna wrote no line into are theirs.
pub(super) fn unworded(text: &[u8], banner: Audience) -> Vec<u8> {
    wordings(banner)
        .into_iter()
        .flat_map(|line| ["\n\n", "\r\n\r\n"].map(|end| [line.as_bytes(), end.as_bytes()].concat()))
        .find_map(|head| text.strip_prefix(head.as_slice()))
        .unwrap_or(text)
        .to_vec()
}

/// Whether the copy at `path` opens on a line `worded` puts there for
/// `banner`, with either line ending: what a session reading it is told, not
/// what the settings would have it told. A copy changed there, left out on
/// purpose or not yet written may not.
pub(super) fn carries_banner(path: &Path, banner: Audience) -> bool {
    let Ok(text) = std::fs::read(path) else { return false };
    wording(banner).is_some() && unworded(&text, banner).len() < text.len()
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

/// Codex's instructions meant for the person's chats: still read, with the
/// line saying whose they are.
pub(super) fn worded_note(name: &str, audience: Audience) -> String {
    format!("instructions {name} ({}): AGENTS.md says so, as Codex reads it in every session", label(audience))
}

/// Codex's instructions meant for the person's chats, read by this session
/// from a copy that does not say so (changed there, left out, not written).
pub(super) fn unworded_note(name: &str, audience: Audience) -> String {
    format!(
        "instructions {name} ({}): the account's AGENTS.md does not say so — changed there, left out or not written — and Codex reads it in every session",
        label(audience)
    )
}

/// What a start says of the audience it went by (`SessionSkills::audience`),
/// and of what it could not keep from the session (`SessionSkills::unkept`).
pub fn audience_applied(s: &SessionSkills) -> Vec<crate::applied::Applied> {
    let mut out = Vec::new();
    if !s.audience.is_empty() {
        out.push(crate::applied::Item::LibraryAudience.with(s.audience.join("; ")));
    }
    if !s.unkept.is_empty() {
        out.push(crate::applied::Item::LibraryAudienceUnkept.with(s.unkept.join("; ")));
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
/// it from the settings. Instructions given any other audience than the
/// person's chats' alone stop being laid into accounts (`laid_out`): from
/// then on they reach their sessions one session at a time. Instructions
/// made every session's where that would leave them reaching nobody are
/// refused (`every_session_refused`).
#[tauri::command(async)]
pub fn library_set_audience(entry: Entry, audience: Audience) -> Result<Overview, String> {
    let (skill, name) = match entry {
        Entry::Skill(n) => (true, n),
        Entry::Instructions(n) => (false, n),
        // Served only where it is due, the same to every chat it is served to.
        Entry::System(n) => return Err(format!("{n} is a system text: it has no audience to pick")),
    };
    check_name(&name)?;
    let mut refused = None;
    let overview = change(|l| {
        if !skill && audience.is_every() {
            refused = every_session_refused(l, &name);
            if refused.is_some() {
                return;
            }
        }
        if !skill && audience != Audience::CHATS {
            l.laid_out.remove(&name);
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
        // A chats' entry an earlier Luna laid out competes already.
        lib.audience.instructions.insert("review".into(), Audience::CHATS);
        lib.laid_out.insert("review".into());
        assert_eq!(every_session_refused(&lib, "review"), None);
    }

    #[test]
    fn instructions_for_the_chats_are_worded_and_come_back_plain() {
        let text = b"# Me\n\nSpeak Russian to me.\n";
        assert_eq!(worded(text, Audience::EVERY), text);
        assert_eq!(worded(text, LEADS_AND_HELPERS), text, "only the chats' are laid out with a line");
        let w = worded(text, Audience::CHATS);
        assert!(w.ends_with(text) && w.len() > text.len());
        assert_eq!(unworded(&w, Audience::CHATS), text, "taken back into the library as it was");
        // An editor that turned the copy's line endings into CRLF.
        let crlf = String::from_utf8_lossy(&w).replace('\n', "\r\n").into_bytes();
        assert_eq!(unworded(&crlf, Audience::CHATS), b"# Me\r\n\r\nSpeak Russian to me.\r\n", "the line goes, the rest as it is");
        assert!(String::from_utf8_lossy(&w).starts_with("> For chats with a person only:"));
        // Only the line Luna wrote there comes off.
        assert_eq!(unworded(&w, Audience::EVERY), w, "a copy Luna wrote no line into: the same words are the person's");
        assert_eq!(unworded(&w, LEADS_AND_HELPERS), w, "nor another line than the one it wrote");
        // A line edited there is the person's text now, kept whole.
        let edited = b"> For the person's own chats, mostly.\n\nRest.\n";
        assert_eq!(unworded(edited, Audience::CHATS), edited);
    }

    #[test]
    fn the_lines_of_before_roles_still_come_off() {
        let text = b"Be brief.\n";
        for (line, banner) in [(FOR_CHATS_BEFORE, Audience::CHATS), (FOR_HELPERS_BEFORE, LEADS_AND_HELPERS)] {
            for end in ["\n\n", "\r\n\r\n"] {
                let old = [line.as_bytes(), end.as_bytes(), text].concat();
                assert_eq!(unworded(&old, banner), text, "{line:?}");
                assert_eq!(unworded(&old, Audience::EVERY), old);
            }
        }
        let dir = std::env::temp_dir().join(format!("luna-banner-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("AGENTS.md");
        std::fs::write(&file, worded(text, Audience::CHATS)).unwrap();
        assert!(carries_banner(&file, Audience::CHATS));
        std::fs::write(&file, text).unwrap();
        assert!(!carries_banner(&file, Audience::CHATS));
        let _ = std::fs::remove_dir_all(&dir);
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
    fn only_every_sessions_and_the_chats_laid_out_go_into_accounts() {
        let mut lib = LibrarySettings::default();
        assert!(laid_into_accounts(&lib, "main"), "every session's");
        lib.audience.instructions.insert("main".into(), Audience::CHATS);
        assert!(!laid_into_accounts(&lib, "main"), "a new chats' entry is given per session");
        lib.laid_out.insert("main".into());
        assert!(laid_into_accounts(&lib, "main"));
        assert_eq!(banner_of(&lib, "main"), Audience::CHATS);
        lib.audience.instructions.insert("main".into(), LEADS_AND_HELPERS);
        assert!(!laid_into_accounts(&lib, "main"), "laid out for the chats alone");
        assert_eq!(banner_of(&lib, "main"), Audience::EVERY);
    }

    #[test]
    fn a_codex_copy_says_whose_it_is_and_a_change_of_audience_rewrites_it() {
        let f = Fixture::new("audience-worded");
        f.instructions("main", "Be brief.\n");
        let mut lib = LibrarySettings { instructions: first(all()), ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        assert_eq!(f.read(1, "AGENTS.md"), "Be brief.\n", "every session's: the library's word for word");
        lib.audience.instructions.insert("main".into(), Audience::CHATS);
        lib.laid_out.insert("main".into());
        let p = f.run(&lib, &mut ledger, false);
        assert_eq!(Fixture::status(&p, &f.account(1).join("AGENTS.md")), Status::Behind, "Luna's own copy, behind");
        f.run(&lib, &mut ledger, true);
        assert_eq!(f.read(1, "AGENTS.md"), format!("{FOR_CHATS}\n\nBe brief.\n"));
        assert_eq!(f.read(0, "CLAUDE.md"), format!("{FOR_CHATS}\n\nBe brief.\n"), "worded on Claude Code too");
        let agents = f.account(1).join("AGENTS.md").to_string_lossy().into_owned();
        assert_eq!(ledger.worded.get(&agents), Some(&Audience::CHATS), "the ledger says which line Luna wrote there");
        lib.audience = Default::default();
        f.run(&lib, &mut ledger, true);
        assert_eq!(f.read(1, "AGENTS.md"), "Be brief.\n");
        assert!(ledger.worded.is_empty(), "rewritten without it");
    }

    #[test]
    fn a_copy_with_the_line_of_before_roles_is_rewritten_with_this_ones() {
        let f = Fixture::new("audience-reworded");
        f.instructions("main", "Be brief.\n");
        let mut lib = LibrarySettings { instructions: first(all()), ..Default::default() };
        lib.audience.instructions.insert("main".into(), Audience::CHATS);
        lib.laid_out.insert("main".into());
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        // As an earlier Luna left it: its line in the copy, and in the ledger.
        let old = format!("{FOR_CHATS_BEFORE}\n\nBe brief.\n");
        for i in [0, 1] {
            let name = if i == 0 { "CLAUDE.md" } else { "AGENTS.md" };
            let dest = f.account(i).join(name);
            std::fs::write(&dest, &old).unwrap();
            ledger.wrote(&dest.to_string_lossy(), hashes(&Files::from([(String::new(), old.clone().into_bytes())])), Audience::CHATS);
        }
        let p = f.run(&lib, &mut ledger, false);
        assert_eq!(Fixture::status(&p, &f.account(1).join("AGENTS.md")), Status::Behind, "Luna's copy, behind: not the person's");
        f.run(&lib, &mut ledger, true);
        assert_eq!(f.read(1, "AGENTS.md"), format!("{FOR_CHATS}\n\nBe brief.\n"));
        assert_eq!(f.read(0, "CLAUDE.md"), format!("{FOR_CHATS}\n\nBe brief.\n"));
    }

    #[test]
    fn instructions_for_leads_and_helpers_leave_the_accounts() {
        let f = Fixture::new("audience-leave");
        f.instructions("main", "Be brief.\n");
        let mut lib = LibrarySettings { instructions: first(all()), ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        assert_eq!(f.read(1, "AGENTS.md"), "Be brief.\n");
        lib.audience.instructions.insert("main".into(), LEADS_AND_HELPERS);
        f.run(&lib, &mut ledger, true);
        assert!(!f.account(1).join("AGENTS.md").exists(), "Luna's own copy goes: given per session from now on");
        assert!(!f.account(0).join("CLAUDE.md").exists());
    }

    /// `main` for everyone, laid out for the person's chats; `report`
    /// (theirs), `cleanup` (leads' and helpers') and `review` (every
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
        lib.laid_out.insert("main".into());
        lib.audience.skills.extend([
            ("report".into(), Audience::CHATS),
            ("cleanup".into(), LEADS_AND_HELPERS),
            ("solo".into(), Audience::CHATS),
        ]);
        f.run(&lib, &mut Ledger::default(), true);
        codex_manifests(&lib, &f.accounts);
        let elsewhere = f.base.join("elsewhere").to_string_lossy().into_owned();
        (lib, repo.to_string_lossy().into_owned(), elsewhere)
    }

    #[test]
    fn a_claude_helper_is_kept_from_what_is_meant_for_chats_and_a_chat_from_what_is_meant_for_helpers() {
        let f = Fixture::new("audience-claude");
        let (lib, repo, elsewhere) = audiences(&f);
        let a = &f.accounts[0];
        let claude_md = f.account(0).join("CLAUDE.md").to_string_lossy().replace('\\', "/");
        for role in [Role::Helper, Role::Lead] {
            let helper = folder_args(&f.dirs, &lib, a, &elsewhere, role);
            assert_eq!(helper.hidden, ["report"]);
            assert_eq!(helper.excluded, std::slice::from_ref(&claude_md), "the account's instructions are the person's chats'");
            assert_eq!(helper.instructions, None, "and nothing in their place");
            assert!(helper.role_entries.is_empty());
            assert_eq!(helper.audience, ["instructions main (chats only)", "skill report (chats only)"]);
        }
        let chat = folder_args(&f.dirs, &lib, a, &elsewhere, Role::Chat);
        assert_eq!(chat.hidden, ["cleanup"]);
        assert!(chat.excluded.is_empty(), "the account's own file, worded for it, is read as ever");
        assert_eq!(chat.audience, ["skill cleanup (leads and helpers only)"]);
        // In the repository a folder's copy goes too, by the name it is listed under.
        let helper = folder_args(&f.dirs, &lib, a, &repo, Role::Helper);
        assert_eq!(helper.hidden, ["report", "solo"]);
        assert_eq!(helper.args.len(), 2, "the folder's skills are still added, for what is the helper's");
        let chat = folder_args(&f.dirs, &lib, a, &repo, Role::Chat);
        assert_eq!(chat.hidden, ["cleanup"]);
    }

    #[test]
    fn a_codex_helper_is_kept_from_skills_for_chats_and_told_whose_its_instructions_are() {
        let f = Fixture::new("audience-codex");
        let (lib, repo, elsewhere) = audiences(&f);
        let a = &f.accounts[1];
        let helper = folder_args(&f.dirs, &lib, a, &elsewhere, Role::Helper);
        assert_eq!(helper.args, ["-c", "skills.config=[{name=\"report\",enabled=false}]"]);
        assert!(helper.excluded.is_empty() && helper.hidden.is_empty(), "nothing Codex could take");
        assert_eq!(
            helper.audience,
            ["instructions main (chats only): AGENTS.md says so, as Codex reads it in every session", "skill report (chats only)"]
        );
        assert!(helper.unkept.is_empty());
        // A copy changed there, without the line: not claimed, said as not kept.
        std::fs::write(f.account(1).join("AGENTS.md"), "Speak Russian to me.\n").unwrap();
        let changed = folder_args(&f.dirs, &lib, a, &elsewhere, Role::Helper);
        assert_eq!(changed.audience, ["skill report (chats only)"]);
        assert!(changed.unkept.len() == 1 && changed.unkept[0].contains("does not say so"), "{:?}", changed.unkept);
        let applied = audience_applied(&changed);
        let unkept = applied.iter().find(|i| i.key == "library.audience-unkept").expect("said");
        assert_eq!(unkept.class, crate::applied::Class::UserAction);
        // In CRLF, as an editor may leave it, it still says so.
        std::fs::write(f.account(1).join("AGENTS.md"), format!("{FOR_CHATS}\r\n\r\nSpeak Russian to me.\r\n")).unwrap();
        assert!(folder_args(&f.dirs, &lib, a, &elsewhere, Role::Helper).unkept.is_empty());
        let chat = folder_args(&f.dirs, &lib, a, &elsewhere, Role::Chat);
        assert_eq!(chat.args, ["-c", "skills.config=[{name=\"cleanup\",enabled=false}]"]);
        assert_eq!(chat.audience, ["skill cleanup (leads and helpers only)"]);
        let helper = folder_args(&f.dirs, &lib, a, &repo, Role::Helper);
        assert_eq!(
            helper.args,
            [
                "-c",
                "plugins.repo@luna.enabled=true",
                "-c",
                "skills.config=[{name=\"repo:solo\",enabled=false},{name=\"report\",enabled=false}]"
            ]
        );
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
