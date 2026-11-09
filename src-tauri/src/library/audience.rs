//! Whose sessions a library entry is for: the person's own chats, the helpers
//! other sessions start, or both (the default, so nothing changes until the
//! person chooses). A helper is briefed by an agent, not the person: rules
//! written for the person's chat (its language, its report format, a plan
//! agreed with them first) and skills whose own descriptions say helpers never
//! load them only cost it context and mislead it.
//!
//! What each CLI allows:
//! - Claude Code leaves a file out of one session (`claudeMdExcludes`) and
//!   turns a skill off for one (`skillOverrides`), both in the settings it is
//!   started with: `folder_args` hides what is not for the session's kind.
//! - Codex turns a skill off for one run (`-c skills.config`), but reads its
//!   account's `AGENTS.md` in every session with no switch to leave it out
//!   (0.167.0: `CodexHomeUserInstructionsProvider` in `codex-rs/codex-home`,
//!   installed unconditionally by `codex-rs/cli/src/main.rs`).
//!
//! So instructions meant for one kind are laid out with a line before them
//! saying whose they are (`worded`), on both CLIs alike: a Codex session reads
//! it, and so does a session Luna does not start. The line is part of the copy,
//! written by the pass; nothing in an account is rewritten per launch.

use super::*;

/// Which kind of session a start is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionKind {
    /// The person's own chat, in a pane.
    Chat,
    /// A helper another session started, run headless.
    Helper,
}

impl Audience {
    /// Whether a session of this kind is meant to see the entry.
    pub fn takes(self, kind: SessionKind) -> bool {
        match self {
            Audience::Both => true,
            Audience::Chats => kind == SessionKind::Chat,
            Audience::Helpers => kind == SessionKind::Helper,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Audience::Both => "every session",
            Audience::Chats => "chats only",
            Audience::Helpers => "helpers only",
        }
    }
}

pub(super) fn instructions_audience(lib: &LibrarySettings, name: &str) -> Audience {
    lib.audience.instructions.get(name).copied().unwrap_or_default()
}

pub(super) fn skill_audience(lib: &LibrarySettings, name: &str) -> Audience {
    lib.audience.skills.get(name).copied().unwrap_or_default()
}

/// What goes before instructions meant for one kind of session. A helper is
/// named by how it came to be, the words a model can check against its own
/// start: Luna's note tells its helpers they are one.
const FOR_CHATS: &str = "> For the person's own chats only: a helper session — one another agent started and briefed, \
through Luna or as a subagent — skips this whole file.";
const FOR_HELPERS: &str = "> For helper sessions only — those another agent started and briefed, through Luna or as a \
subagent: the person's own chats skip this whole file.";

fn wording(audience: Audience) -> Option<&'static str> {
    match audience {
        Audience::Both => None,
        Audience::Chats => Some(FOR_CHATS),
        Audience::Helpers => Some(FOR_HELPERS),
    }
}

/// Instructions as their copies hold them: for one kind only, the line saying
/// whose they are and a blank line before them.
pub(super) fn worded(text: &[u8], audience: Audience) -> Vec<u8> {
    match wording(audience) {
        Some(line) => [line.as_bytes(), b"\n\n", text].concat(),
        None => text.to_vec(),
    }
}

/// A copy's instructions as the library keeps them: without the line `worded`
/// put before them for `banner` — the one Luna's ledger says it wrote there,
/// `Both` where it wrote none — while that line and the blank one after it are
/// still there word for word, with either line ending (an editor may have
/// turned the copy's into CRLF). Anything else is the person's text, taken
/// whole: the same words in a copy Luna wrote no line into are theirs.
pub(super) fn unworded(text: &[u8], banner: Audience) -> Vec<u8> {
    let Some(line) = wording(banner) else { return text.to_vec() };
    ["\n\n", "\r\n\r\n"]
        .iter()
        .find_map(|end| text.strip_prefix([line.as_bytes(), end.as_bytes()].concat().as_slice()))
        .unwrap_or(text)
        .to_vec()
}

/// Whether the copy at `path` opens on the line `worded` puts there for
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
    format!("{what} {name} ({})", audience.label())
}

/// Codex's instructions meant for the other kind: still read, with the line
/// saying whose they are.
pub(super) fn worded_note(name: &str, audience: Audience) -> String {
    format!("instructions {name} ({}): AGENTS.md says so, as Codex reads it in every session", audience.label())
}

/// Codex's instructions meant for the other kind, read by this session from a
/// copy that does not say so (changed there, left out, not written).
pub(super) fn unworded_note(name: &str, audience: Audience) -> String {
    format!(
        "instructions {name} ({}): the account's AGENTS.md does not say so — changed there, left out or not written — and Codex reads it in every session",
        audience.label()
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

/// Whose sessions a skill or instructions file is for; both drops it from
/// the settings.
#[tauri::command(async)]
pub fn library_set_audience(entry: Entry, audience: Audience) -> Result<Overview, String> {
    let (skill, name) = match entry {
        Entry::Skill(n) => (true, n),
        Entry::Instructions(n) => (false, n),
    };
    check_name(&name)?;
    crate::log::info("library", &format!("{} {name}: for {}", if skill { "skill" } else { "instructions" }, audience.label()));
    change(|l| {
        let map = if skill { &mut l.audience.skills } else { &mut l.audience.instructions };
        match audience {
            Audience::Both => map.remove(&name),
            other => map.insert(name, other),
        };
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instructions_for_one_kind_are_worded_and_come_back_plain() {
        let text = b"# Me\n\nSpeak Russian to me.\n";
        assert_eq!(worded(text, Audience::Both), text);
        for audience in [Audience::Chats, Audience::Helpers] {
            let w = worded(text, audience);
            assert!(w.ends_with(text) && w.len() > text.len());
            assert_eq!(unworded(&w, audience), text, "taken back into the library as it was");
            // An editor that turned the copy's line endings into CRLF.
            let crlf = String::from_utf8_lossy(&w).replace('\n', "\r\n").into_bytes();
            assert_eq!(unworded(&crlf, audience), b"# Me\r\n\r\nSpeak Russian to me.\r\n", "the line goes, the rest as it is");
        }
        assert!(String::from_utf8_lossy(&worded(text, Audience::Chats)).starts_with("> For the person's own chats only:"));
        // Only the line Luna wrote there comes off.
        let w = worded(text, Audience::Chats);
        assert_eq!(unworded(&w, Audience::Both), w, "a copy Luna wrote no line into: the same words are the person's");
        assert_eq!(unworded(&w, Audience::Helpers), w, "nor another line than the one it wrote");
        // A line edited there is the person's text now, kept whole.
        let edited = b"> For the person's own chats, mostly.\n\nRest.\n";
        assert_eq!(unworded(edited, Audience::Chats), edited);
    }

    #[test]
    fn an_audience_takes_its_kind_and_both_take_every_kind() {
        use SessionKind::*;
        assert!(Audience::Both.takes(Chat) && Audience::Both.takes(Helper));
        assert!(Audience::Chats.takes(Chat) && !Audience::Chats.takes(Helper));
        assert!(Audience::Helpers.takes(Helper) && !Audience::Helpers.takes(Chat));
    }

    #[test]
    fn a_codex_copy_says_whose_it_is_and_a_change_of_audience_rewrites_it() {
        let f = Fixture::new("audience-worded");
        f.instructions("main", "Be brief.\n");
        let mut lib = LibrarySettings { instructions: first(all()), ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        assert_eq!(f.read(1, "AGENTS.md"), "Be brief.\n", "both: the library's word for word");
        lib.audience.instructions.insert("main".into(), Audience::Chats);
        let p = f.run(&lib, &mut ledger, false);
        assert_eq!(Fixture::status(&p, &f.account(1).join("AGENTS.md")), Status::Behind, "Luna's own copy, behind");
        f.run(&lib, &mut ledger, true);
        assert_eq!(f.read(1, "AGENTS.md"), format!("{FOR_CHATS}\n\nBe brief.\n"));
        assert_eq!(f.read(0, "CLAUDE.md"), format!("{FOR_CHATS}\n\nBe brief.\n"), "worded on Claude Code too");
        let agents = f.account(1).join("AGENTS.md").to_string_lossy().into_owned();
        assert_eq!(ledger.worded.get(&agents), Some(&Audience::Chats), "the ledger says which line Luna wrote there");
        lib.audience = Default::default();
        f.run(&lib, &mut ledger, true);
        assert_eq!(f.read(1, "AGENTS.md"), "Be brief.\n");
        assert!(ledger.worded.is_empty(), "rewritten without it");
    }

    /// `main` for everyone, worded for the person's chats; `report` (theirs),
    /// `cleanup` (helpers') and `review` (both) into every account; and a
    /// repository giving every account's sessions there `solo`, the person's
    /// chats' alone, which no account has as a whole.
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
        lib.audience.instructions.insert("main".into(), Audience::Chats);
        lib.audience.skills.extend([
            ("report".into(), Audience::Chats),
            ("cleanup".into(), Audience::Helpers),
            ("solo".into(), Audience::Chats),
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
        let helper = folder_args(&f.dirs, &lib, a, &elsewhere, SessionKind::Helper);
        assert_eq!(helper.hidden, ["report"]);
        assert_eq!(helper.excluded, [claude_md], "the account's instructions are the person's chats'");
        assert_eq!(helper.instructions, None, "and nothing in their place");
        assert_eq!(helper.audience, ["instructions main (chats only)", "skill report (chats only)"]);
        let chat = folder_args(&f.dirs, &lib, a, &elsewhere, SessionKind::Chat);
        assert_eq!(chat.hidden, ["cleanup"]);
        assert!(chat.excluded.is_empty(), "the account's own file, worded for it, is read as ever");
        assert_eq!(chat.audience, ["skill cleanup (helpers only)"]);
        // In the repository a folder's copy goes too, by the name it is listed under.
        let helper = folder_args(&f.dirs, &lib, a, &repo, SessionKind::Helper);
        assert_eq!(helper.hidden, ["report", "solo"]);
        assert_eq!(helper.args.len(), 2, "the folder's skills are still added, for what is the helper's");
        let chat = folder_args(&f.dirs, &lib, a, &repo, SessionKind::Chat);
        assert_eq!(chat.hidden, ["cleanup"]);
    }

    #[test]
    fn a_codex_helper_is_kept_from_skills_for_chats_and_told_whose_its_instructions_are() {
        let f = Fixture::new("audience-codex");
        let (lib, repo, elsewhere) = audiences(&f);
        let a = &f.accounts[1];
        let helper = folder_args(&f.dirs, &lib, a, &elsewhere, SessionKind::Helper);
        assert_eq!(helper.args, ["-c", "skills.config=[{name=\"report\",enabled=false}]"]);
        assert!(helper.excluded.is_empty() && helper.hidden.is_empty(), "nothing Codex could take");
        assert_eq!(
            helper.audience,
            ["instructions main (chats only): AGENTS.md says so, as Codex reads it in every session", "skill report (chats only)"]
        );
        assert!(helper.unkept.is_empty());
        // A copy changed there, without the line: not claimed, said as not kept.
        std::fs::write(f.account(1).join("AGENTS.md"), "Speak Russian to me.\n").unwrap();
        let changed = folder_args(&f.dirs, &lib, a, &elsewhere, SessionKind::Helper);
        assert_eq!(changed.audience, ["skill report (chats only)"]);
        assert!(changed.unkept.len() == 1 && changed.unkept[0].contains("does not say so"), "{:?}", changed.unkept);
        let applied = audience_applied(&changed);
        let unkept = applied.iter().find(|i| i.key == "library.audience-unkept").expect("said");
        assert_eq!(unkept.class, crate::applied::Class::UserAction);
        // In CRLF, as an editor may leave it, it still says so.
        std::fs::write(f.account(1).join("AGENTS.md"), format!("{FOR_CHATS}\r\n\r\nSpeak Russian to me.\r\n")).unwrap();
        assert!(folder_args(&f.dirs, &lib, a, &elsewhere, SessionKind::Helper).unkept.is_empty());
        let chat = folder_args(&f.dirs, &lib, a, &elsewhere, SessionKind::Chat);
        assert_eq!(chat.args, ["-c", "skills.config=[{name=\"cleanup\",enabled=false}]"]);
        assert_eq!(chat.audience, ["skill cleanup (helpers only)"]);
        let helper = folder_args(&f.dirs, &lib, a, &repo, SessionKind::Helper);
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
    fn with_every_entry_for_both_a_session_of_either_kind_gets_what_it_did() {
        let f = Fixture::new("audience-none");
        let (mut lib, repo, elsewhere) = audiences(&f);
        lib.audience = Default::default();
        for a in &f.accounts {
            for folder in [&repo, &elsewhere] {
                let chat = folder_args(&f.dirs, &lib, a, folder, SessionKind::Chat);
                assert_eq!(folder_args(&f.dirs, &lib, a, folder, SessionKind::Helper), chat);
                assert!(chat.audience.is_empty() && chat.hidden.is_empty() && chat.excluded.is_empty());
            }
        }
    }

    #[test]
    fn instructions_a_folder_gives_that_are_not_for_the_session_are_none() {
        let f = Fixture::new("audience-folder-instructions");
        let (mut lib, repo, _) = audiences(&f);
        f.instructions("terse", "Say less.\n");
        lib.audience.instructions = BTreeMap::from([("terse".into(), Audience::Chats)]);
        lib.folders[0].instructions = BTreeMap::from([("claude".into(), Some("terse".into()))]);
        let a = &f.accounts[0];
        let chat = folder_args(&f.dirs, &lib, a, &repo, SessionKind::Chat);
        assert_eq!(chat.instructions, Some(f.dirs.instructions_file("terse")));
        let helper = folder_args(&f.dirs, &lib, a, &repo, SessionKind::Helper);
        assert_eq!(helper.instructions, None);
        assert_eq!(helper.excluded, chat.excluded, "the account's left out all the same");
        assert_eq!(helper.audience[0], "instructions terse (chats only)");
    }
}
