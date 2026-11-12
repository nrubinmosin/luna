//! What a session is told of its role, at memory level
//! (docs/adr/0025-session-roles-at-memory-level.md). Memory files outweigh an
//! appended system prompt and a note on a prompt, and survive a compaction
//! and a handoff; so a session's role goes where its instructions files do.
//!
//! A session gets four layers, in this order:
//! 1. its account's file, `CLAUDE.md` or `AGENTS.md`, as the library lays it
//!    out (Claude Code: or the one a folder gives in its place, first among
//!    Luna's layers);
//! 2. Luna's default for its role, a system text the person may override
//!    (system.rs `role-chat`, empty, `role-lead`, `role-helper`);
//! 3. the library's instructions for its role (audience.rs), in name order;
//! 4. its constraints: what the agents above it asked of every session under
//!    them, word for word (agents/spawn `Constraint`). Last, so they bind most.
//!
//! Layers 2-4 go to Claude Code as the `CLAUDE.md` of a folder of the
//! session's own, read as memory (`--add-dir` with
//! `CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD`), written at every start
//! and turn: an edit reaches it at its next turn. To Codex they go as
//! `-c developer_instructions`, after the value its own config stack sets
//! (codex/config.rs `developer_instructions_in`), which the `-c` would
//! otherwise replace; Codex sends that at a thread's start and after a
//! compaction, never on a resume, so an edit reaches a running Codex session
//! only then (docs/cli-compatibility.md).
//!
//! The note after a task from another agent (`RoleTexts::note`) is a system
//! text too; every note Luna has served stays known to the readers that take
//! it off a prompt (pty/start.rs `without_note`), its override's by its hash
//! (`note_served`).

use super::*;

/// The most of a Codex `developer_instructions` value Luna passes, in bytes:
/// it goes on the command line, which Windows caps at 32,767 characters for
/// every argument together.
pub const CODEX_ROLE_MAX: usize = 24 * 1024;

/// What a session's start reads of the system texts for its role.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RoleTexts {
    /// Luna's default for the role, or the person's override: empty is none.
    pub default: String,
    /// The note after a task from another agent, for a lead or a helper.
    pub note: Option<String>,
    /// Whose text each is, for what Luna applied: Luna's own, the person's,
    /// or the person's that will not do and why.
    pub said: Vec<String>,
}

/// The system text of `role`'s default, and of its note where it has one.
pub fn role_text_names(role: Role) -> (&'static str, Option<&'static str>) {
    match role {
        Role::Chat => (ROLE_CHAT, None),
        Role::Lead => (ROLE_LEAD, Some(NOTE_LEAD)),
        Role::Helper => (ROLE_HELPER, Some(NOTE_HELPER)),
    }
}

/// The texts for `role` as they are served now. Read before any lock of the
/// caller's: an override's read waits on the system lock while an adoption
/// swaps one in (system.rs `system_lock`).
pub fn role_texts(role: Role) -> RoleTexts {
    role_texts_in(&Dirs::current(), role)
}

pub(super) fn role_texts_in(dirs: &Dirs, role: Role) -> RoleTexts {
    let (default, note) = role_text_names(role);
    let mut said = Vec::new();
    let mut read = |name: &str| {
        let r = resolve_text(dirs, name)?;
        said.push(match (&r.source, &r.problem) {
            (Source::Library, _) => format!("{name}: yours, sha {}", r.short_sha()),
            (Source::Luna, Some(why)) => format!("{name}: yours will not do ({why}), Luna's own served"),
            (Source::Luna, None) => format!("{name}: Luna's own"),
        });
        Some(r)
    };
    let default = read(default).map(|r| r.body().to_string()).unwrap_or_default();
    let note = note.and_then(read).map(|r| {
        if r.source == Source::Library {
            note_served(&dirs.data, r.body());
        }
        r.body().to_string()
    });
    RoleTexts { default, note, said }
}

/// Layers 2-4 as one text, each under a heading of its own; None where all
/// of them are empty. `folder`: the instructions a folder gives a Claude Code
/// session in place of its account's, which come first and as they are, in
/// layer 1's place.
pub fn compose_layers(folder: Option<&str>, default: &str, entries: &[(String, String)], constraints: &[String]) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(text) = folder.map(str::trim).filter(|t| !t.is_empty()) {
        parts.push(text.to_string());
    }
    if !default.trim().is_empty() {
        parts.push(format!("# Your role\n\n{}", default.trim()));
    }
    for (name, text) in entries.iter().filter(|(_, t)| !t.trim().is_empty()) {
        parts.push(format!("# Instructions: {name}\n\n{}", text.trim()));
    }
    let constraints: Vec<&str> = constraints.iter().map(|c| c.trim()).filter(|c| !c.is_empty()).collect();
    if !constraints.is_empty() {
        parts.push(format!(
            "# Constraints\n\nSet by the agents that started this work, for every session under them. They come before anything \
             else in your instructions.\n\n{}",
            constraints.join("\n\n")
        ));
    }
    (!parts.is_empty()).then(|| parts.join("\n\n") + "\n")
}

/// A session's layers 2-4: its role's texts, what the library gives its role
/// (`SessionSkills::instructions` and `role_entries`) and its constraints.
pub fn role_layers(texts: &RoleTexts, skills: &SessionSkills, constraints: &[String]) -> Option<String> {
    // Instructions the library no longer has are none.
    let folder = skills.instructions.as_ref().and_then(|f| std::fs::read_to_string(f).ok());
    compose_layers(folder.as_deref(), &texts.default, &skills.role_entries, constraints)
}

/// What Luna applied of a session's role layers, given as `how` (a folder
/// read as memory, a `-c`): whose text its role's is, what the library gives
/// its role, and its constraints word for word.
pub fn layers_applied(texts: &RoleTexts, skills: &SessionSkills, constraints: &[String], how: &str) -> Vec<crate::applied::Applied> {
    use crate::applied::Item;
    let mut parts = vec![how.to_string()];
    parts.extend(texts.said.iter().filter(|s| !s.starts_with(NOTE_LEAD) && !s.starts_with(NOTE_HELPER)).cloned());
    if let Some(f) = &skills.instructions {
        parts.push(format!("the folder's instructions {}", f.file_stem().unwrap_or_default().to_string_lossy()));
    }
    let entries: Vec<&str> = skills.role_entries.iter().map(|(n, _)| n.as_str()).collect();
    if !entries.is_empty() {
        parts.push(format!("the library's instructions for its role: {}", entries.join(", ")));
    }
    let mut out = vec![Item::RoleLayers.with(parts.join("; "))];
    if !constraints.is_empty() {
        out.push(Item::Constraints.with(constraints.join("\n\n")));
    }
    out
}

/// The `developer_instructions` a Codex session is given: what its config
/// stack sets (`stack`), then its layers, a blank line between. Refused over
/// CODEX_ROLE_MAX, with the reason.
pub fn codex_developer_instructions(stack: Option<&str>, layers: &str) -> Result<String, String> {
    let value = match stack.map(str::trim).filter(|s| !s.is_empty()) {
        Some(stack) => format!("{stack}\n\n{layers}"),
        None => layers.to_string(),
    };
    if value.len() > CODEX_ROLE_MAX {
        return Err(format!(
            "its developer_instructions — the config's own, then its role, the library's instructions for it and its constraints — \
             come to {} KB, over the {} KB a command line takes",
            value.len().div_ceil(1024),
            CODEX_ROLE_MAX / 1024
        ));
    }
    Ok(value)
}

/// `developer_instructions` as a `-c` override: the value as a TOML string.
pub fn codex_developer_args(value: &str) -> [String; 2] {
    ["-c".into(), format!("developer_instructions={}", toml_edit::Value::from(value))]
}

/// Where the notes served from an override are kept, by their hash.
const NOTES_SERVED: &str = "notes-served.json";

/// How many of them: the oldest go first.
const NOTES_SERVED_MAX: usize = 64;

fn sha_of(text: &str) -> String {
    Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// The hashes of the override notes served, oldest first, as `data` holds
/// them; none where the file will not read.
fn served_in(data: &Path) -> Vec<String> {
    crate::secret::read_private(&data.join(NOTES_SERVED), 64 * 1024)
        .ok()
        .flatten()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Notes the person's override of a note text, as served: a helper's
/// conversation keeps it after its prompt, and the readers have to know it to
/// take it off (`was_served`), after the override has changed too.
fn note_served(data: &Path, note: &str) {
    let sha = sha_of(note);
    let mut served = served_in(data);
    if served.contains(&sha) {
        return;
    }
    served.push(sha);
    let over = served.len().saturating_sub(NOTES_SERVED_MAX);
    served.drain(..over);
    let _ = std::fs::create_dir_all(data);
    let written =
        serde_json::to_vec(&served).map_err(std::io::Error::other).and_then(|b| crate::secret::write_private(&data.join(NOTES_SERVED), &b));
    if let Err(e) = written {
        crate::log::warn("library", &format!("could not note the note served: {e}"));
    }
    *served_cache().lock().unwrap_or_else(|e| e.into_inner()) = None;
}

type ServedCache = Mutex<Option<(Option<std::time::SystemTime>, Vec<String>)>>;

fn served_cache() -> &'static ServedCache {
    static C: std::sync::OnceLock<ServedCache> = std::sync::OnceLock::new();
    C.get_or_init(Default::default)
}

/// Whether `note` is one Luna served: its default for a lead or a helper,
/// or an override of either served at some point (`note_served`). Read for
/// every message a reader takes a note off, so held in memory while the
/// file's time stays the same: the core serves, the window reads too.
pub fn was_served(note: &str) -> bool {
    if [NOTE_LEAD, NOTE_HELPER].iter().filter_map(|n| luna_text(n).ok()).any(|t| t.trim() == note) {
        return true;
    }
    let data = crate::paths::data_dir();
    let stamp = std::fs::metadata(data.join(NOTES_SERVED)).and_then(|m| m.modified()).ok();
    let mut cache = served_cache().lock().unwrap_or_else(|e| e.into_inner());
    if cache.as_ref().is_none_or(|(at, _)| *at != stamp) {
        *cache = Some((stamp, served_in(&data)));
    }
    let sha = sha_of(note);
    cache.as_ref().is_some_and(|(_, served)| served.contains(&sha))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_text(f: &Fixture, name: &str, text: &str) {
        let dir = f.dirs.root.join("system").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("TEXT.md"), text).unwrap();
    }

    #[test]
    fn each_role_gets_luna_s_texts_until_the_person_writes_their_own() {
        let f = Fixture::new("role-texts");
        let chat = role_texts_in(&f.dirs, Role::Chat);
        assert_eq!((chat.default.as_str(), chat.note.as_deref()), ("", None), "a chat is told nothing by default");
        let helper = role_texts_in(&f.dirs, Role::Helper);
        assert!(helper.default.starts_with("Your task comes from another agent"));
        assert!(helper.note.as_deref().unwrap().starts_with("This task comes from another agent, not a person"));
        let lead = role_texts_in(&f.dirs, Role::Lead);
        assert!(lead.default.contains("constraints") && lead.default.contains("helpers"));
        assert_eq!(lead.said, ["role-lead: Luna's own", "note-lead: Luna's own"]);
        // Neither note names a person's operator, Luna or other sessions.
        for n in [NOTE_LEAD, NOTE_HELPER] {
            let t = luna_text(n).unwrap();
            for word in ["operator", "Luna", "session", "[", "]"] {
                assert!(!t.contains(word), "{n}: {word}");
            }
            assert_eq!(check_override(n, t), Ok(()), "Luna's own passes its own check");
        }
        write_text(&f, ROLE_HELPER, "Be terse.\n");
        write_text(&f, NOTE_HELPER, "From an agent: answer it.\n");
        let helper = role_texts_in(&f.dirs, Role::Helper);
        assert_eq!((helper.default.as_str(), helper.note.as_deref()), ("Be terse.", Some("From an agent: answer it.")));
        assert!(helper.said[0].starts_with("role-helper: yours, sha "));
        assert!(served_in(&f.dirs.data).contains(&sha_of("From an agent: answer it.")), "its note is kept known");
        // An override that will not do is not served, and says why.
        write_text(&f, NOTE_HELPER, "Two\nlines.\n");
        let helper = role_texts_in(&f.dirs, Role::Helper);
        assert!(helper.note.unwrap().starts_with("This task comes from another agent"));
        assert!(helper.said[1].contains("more than one line"), "{:?}", helper.said);
        // An empty role says nothing; a role of the chat's may be given.
        write_text(&f, ROLE_HELPER, "");
        assert_eq!(role_texts_in(&f.dirs, Role::Helper).default, "");
        write_text(&f, ROLE_CHAT, "Plan with me first.\n");
        assert_eq!(role_texts_in(&f.dirs, Role::Chat).default, "Plan with me first.");
    }

    #[test]
    fn a_note_s_override_is_one_plain_line() {
        assert_eq!(check_override(NOTE_LEAD, "Answer the agent.\n"), Ok(()));
        for (text, why) in [
            ("", "empty"),
            ("  \n", "empty"),
            ("One\nTwo", "more than one line"),
            ("See [this]", "bracket"),
            ("Bell\u{7}", "control character"),
        ] {
            assert!(check_override(NOTE_LEAD, text).unwrap_err().contains(why), "{text:?}");
        }
        assert!(check_override(NOTE_LEAD, &"a".repeat(NOTE_MAX_CHARS + 1)).unwrap_err().contains("characters"));
        assert_eq!(check_override(NOTE_LEAD, &"é".repeat(NOTE_MAX_CHARS)), Ok(()), "characters, not bytes");
        assert!(check_override(ROLE_LEAD, &"a".repeat(ROLE_MAX_BYTES + 1)).unwrap_err().contains("KB"));
        assert_eq!(check_override(ROLE_LEAD, ""), Ok(()), "a role may say nothing");
        assert!(check_override("role-nobody", "x").is_err());
    }

    #[test]
    fn the_plain_texts_take_no_skill_s_name_and_adopt_none() {
        let f = Fixture::new("role-texts-reserved");
        for t in SYSTEM_TEXTS {
            assert_eq!(reserved(t.name), t.kind == TextKind::Skill, "{}", t.name);
        }
        f.skill(ROLE_LEAD, "v1");
        assert!(adopt_system(&f.dirs, ROLE_LEAD).unwrap_err().contains("not a system skill"));
        assert!(f.dirs.skills().join(ROLE_LEAD).is_dir(), "left where it is");
    }

    #[test]
    fn the_layers_come_in_order_each_under_its_heading() {
        let entries = vec![("a-rules".to_string(), "Rule A.\n".to_string()), ("b-rules".to_string(), "Rule B.".to_string())];
        let constraints = vec!["Touch nothing in src/.".to_string(), "No network.\n".to_string()];
        let all = compose_layers(Some("Folder's own.\n"), "Your task comes from another agent.", &entries, &constraints).unwrap();
        let at = |s: &str| all.find(s).unwrap_or_else(|| panic!("{s} in {all}"));
        assert!(all.starts_with("Folder's own.\n\n# Your role"), "the folder's in the account's place, first");
        assert!(at("# Your role") < at("# Instructions: a-rules") && at("Rule A.") < at("# Instructions: b-rules"));
        assert!(at("Rule B.") < at("# Constraints") && at("Touch nothing in src/.") < at("No network."), "constraints last");
        assert!(all.ends_with("No network.\n"));
        assert_eq!(compose_layers(None, " \n", &[("x".into(), "\n".into())], &[]), None, "nothing to say, no file");
        assert_eq!(compose_layers(Some("Mine.\n"), "", &[], &[]).as_deref(), Some("Mine.\n"), "a folder's alone, as it is");
    }

    #[test]
    fn codex_gets_the_stack_s_value_first_and_nothing_over_the_cap() {
        assert_eq!(codex_developer_instructions(Some("Theirs.\n"), "# Your role\n\nMine.\n").unwrap(), "Theirs.\n\n# Your role\n\nMine.\n");
        assert_eq!(codex_developer_instructions(Some("  "), "Mine.").unwrap(), "Mine.");
        assert_eq!(codex_developer_instructions(None, "Mine.").unwrap(), "Mine.");
        let big = "a".repeat(CODEX_ROLE_MAX);
        assert!(codex_developer_instructions(Some("x"), &big).unwrap_err().contains("over the 24 KB"));
        // A TOML string Codex reads back as it was, quotes and lines and all.
        let value = "Say \"hi\".\nC:\\path\n'single' \"\"\" ";
        let [c, arg] = codex_developer_args(value);
        assert_eq!(c, "-c");
        let doc: toml_edit::DocumentMut = arg.parse().unwrap();
        assert_eq!(doc["developer_instructions"].as_str(), Some(value));
    }

    #[test]
    fn the_served_notes_are_kept_and_the_oldest_go() {
        let f = Fixture::new("role-notes-served");
        for i in 0..NOTES_SERVED_MAX + 3 {
            note_served(&f.dirs.data, &format!("note {i}"));
        }
        let served = served_in(&f.dirs.data);
        assert_eq!(served.len(), NOTES_SERVED_MAX);
        assert!(!served.contains(&sha_of("note 0")) && served.contains(&sha_of(&format!("note {}", NOTES_SERVED_MAX + 2))));
        note_served(&f.dirs.data, "note 3");
        assert_eq!(served_in(&f.dirs.data).len(), NOTES_SERVED_MAX, "once each");
    }
}
