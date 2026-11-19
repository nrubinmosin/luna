//! The library's skills and instructions files as it holds them: read, checked
//! fit to lay out (a skill only by what copying it takes), and named.

use super::*;

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    /// Why it is laid out nowhere: its folder will not read, holds no
    /// SKILL.md, or is named what no folder can be.
    pub problem: Option<String>,
    /// The accounts it goes into, wherever no folder says otherwise.
    pub accounts: Reach,
    /// Whose sessions it is for (audience.rs).
    pub audience: Audience,
}

/// A name both CLIs take for a skill, and the only kind Luna makes a folder
/// of; instructions are named the same way, for one rule in the window.
pub(super) fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 64 {
        return Err("A name is 1 to 64 characters".into());
    }
    if !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') || name.starts_with('-') {
        return Err("A name is lowercase letters, digits and hyphens".into());
    }
    Ok(())
}

/// A name Luna can make a folder of, on any platform the library may be read
/// on: what a skill's folder is called, and all Luna asks of it. Whether the
/// CLIs take the name is theirs to say.
pub(super) fn check_skill_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.trim() != name {
        return Err("A name is not empty and has no space at either end".into());
    }
    if name.len() > 255 {
        return Err("A name is 255 bytes at most".into());
    }
    // A leading dot is a folder the library does not list (`read_skills`).
    if name.starts_with('.') || name.ends_with('.') {
        return Err("A name neither starts nor ends with a dot".into());
    }
    if name.chars().any(|c| c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')) {
        return Err("A name has none of / \\ : * ? \" < > | in it".into());
    }
    let stem = name.split('.').next().unwrap_or_default().to_ascii_lowercase();
    let reserved = matches!(stem.as_str(), "con" | "prn" | "aux" | "nul")
        || (stem.len() == 4 && (stem.starts_with("com") || stem.starts_with("lpt")) && stem.ends_with(|c: char| c.is_ascii_digit()));
    if reserved {
        return Err(format!("{name} is a name Windows keeps for itself"));
    }
    Ok(())
}

/// The `description` in SKILL.md's front matter, to show beside the skill's
/// name, as plain text: quoted, folded (`>`) and literal (`|`) values and
/// indented continuation lines all read. Best effort and display only: a
/// file with no front matter, or none that closes, shows no description and
/// is laid out all the same.
fn description(text: &str) -> String {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.lines();
    if lines.next().map(str::trim_end) != Some("---") {
        return String::new();
    }
    let mut keys: BTreeMap<&str, (String, bool)> = BTreeMap::new();
    let mut current: Option<&str> = None;
    for line in lines {
        if line.trim_end() == "---" {
            let v = keys.get("description").map(|(v, _)| v.trim()).unwrap_or_default();
            let quoted = v.len() >= 2 && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\'')));
            return if quoted { v[1..v.len() - 1].to_string() } else { v.to_string() };
        }
        if line.starts_with([' ', '\t']) || line.trim().is_empty() {
            if let Some((value, literal)) = current.and_then(|k| keys.get_mut(k)) {
                if !line.trim().is_empty() {
                    if !value.is_empty() {
                        value.push(if *literal { '\n' } else { ' ' });
                    }
                    value.push_str(line.trim());
                }
            }
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            current = None;
            continue;
        };
        let value = value.trim();
        let (value, literal) = match value {
            ">" | ">-" | ">+" => ("", false),
            "|" | "|-" | "|+" => ("", true),
            v => (v, false),
        };
        current = Some(key.trim());
        keys.insert(key.trim(), (value.to_string(), literal));
    }
    String::new()
}

/// Every skill in the library, by name, with its files when its folder can be
/// laid out: a name that makes a folder, a folder that reads, a SKILL.md in
/// it. What SKILL.md says is the CLI's to judge. A `skills/` that will not
/// list is an error, not an empty one: a skill missed reads as taken out of
/// the library, and its copies with it.
pub(super) fn read_skills(dirs: &Dirs) -> Result<Skills, String> {
    let mut out = BTreeMap::new();
    let failed = |e: std::io::Error| format!("{}: {e}", dirs.skills().display());
    let entries = match std::fs::read_dir(dirs.skills()) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(failed(e)),
    };
    for entry in entries {
        let entry = entry.map_err(failed)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || !entry.file_type().map_err(failed)?.is_dir() {
            continue;
        }
        let files = check_skill_name(&name)
            .and_then(|()| read_dir_files(&entry.path()))
            .and_then(|f| if f.contains_key(SKILL_FILE) { Ok(f) } else { Err("There is no SKILL.md".to_string()) });
        let (description, problem) = match &files {
            Ok(f) => (description(&String::from_utf8_lossy(&f[SKILL_FILE])), None),
            Err(e) => (String::new(), Some(e.clone())),
        };
        out.insert(name.clone(), (SkillInfo { name, description, problem, accounts: Vec::new(), audience: Audience::EVERY }, files));
    }
    Ok(out)
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct InstructionsInfo {
    pub name: String,
    /// Its first heading, to tell it by (`instructions_title`); empty without one.
    pub description: String,
    /// Why it is laid out nowhere, while it is not fit to be; or, given per
    /// session, why it is too long for some Codex sessions' command line
    /// (role.rs `codex_overflows`).
    pub problem: Option<String>,
    /// The accounts its reach takes in; each gets it unless a closer reach
    /// gives it other instructions.
    pub accounts: Reach,
    /// Whose sessions it is for (audience.rs).
    pub audience: Audience,
    /// For some roles only: given to each of their sessions as Luna starts
    /// it, never laid into an account, so its accounts do not compete with
    /// other instructions for one (audience.rs `laid_into_accounts`).
    pub per_session: bool,
}

/// Every instructions file in the library, by name. A folder that will not
/// list is an error, as `skills/` is; so is the old `instructions.md` not
/// moved into it yet: read as no instructions, either would take their
/// copies out.
pub(super) fn read_instructions(dirs: &Dirs) -> Result<Instructions, String> {
    let old = dirs.root.join(OLD_INSTRUCTIONS);
    if old.is_file() && !dirs.instructions_file(crate::settings::FIRST_INSTRUCTIONS).exists() {
        return Err(format!("{} is yet to move into {}", old.display(), dirs.instructions().display()));
    }
    let mut out = BTreeMap::new();
    let failed = |e: std::io::Error| format!("{}: {e}", dirs.instructions().display());
    let entries = match std::fs::read_dir(dirs.instructions()) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(failed(e)),
    };
    for entry in entries {
        let entry = entry.map_err(failed)?;
        let file = entry.file_name().to_string_lossy().into_owned();
        let Some(name) = file.strip_suffix(".md") else { continue };
        if name.starts_with('.') || entry.file_type().map_err(failed)?.is_dir() {
            continue;
        }
        let text = check_name(name).and_then(|()| std::fs::read(entry.path()).map_err(|e| format!("{}: {e}", entry.path().display())));
        out.insert(name.to_string(), text.map(|t| (!String::from_utf8_lossy(&t).trim().is_empty()).then_some(t)));
    }
    Ok(out)
}

/// What tells instructions apart in the window: their first `#` heading, or
/// the first `##` where they have none — the names their author gave them.
/// Without either, nothing: a line of prose cut off mid-sentence, or one
/// section's heading, would claim to say what the whole file is about.
/// Lines in a fenced block are code, so a shell comment there is no heading.
fn instructions_title(text: &str) -> String {
    let mut fenced = false;
    let mut second = None;
    for line in text.lines().map(str::trim) {
        if line.starts_with("```") || line.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        let level = line.chars().take_while(|&c| c == '#').count();
        let rest = &line[level..];
        // `#tag` and `###` are not what the file is called.
        if fenced || !(1..=2).contains(&level) || !(rest.is_empty() || rest.starts_with([' ', '\t'])) {
            continue;
        }
        let title = rest.trim();
        if title.is_empty() {
            continue;
        }
        if level == 1 {
            return title.to_string();
        }
        second.get_or_insert_with(|| title.to_string());
    }
    second.unwrap_or_default()
}

/// The window's line for instructions: what tells them apart, or what is
/// wrong with them.
pub(super) fn instructions_info(name: &str, text: &Result<Option<Vec<u8>>, String>, lib: &LibrarySettings) -> InstructionsInfo {
    let (description, problem) = match text {
        Ok(Some(t)) => (instructions_title(&String::from_utf8_lossy(t)), None),
        Ok(None) => (String::new(), Some("Blank: laid out nowhere until it has something in it".to_string())),
        Err(e) => (String::new(), Some(e.clone())),
    };
    InstructionsInfo {
        name: name.to_string(),
        description,
        problem,
        accounts: lib.instructions.get(name).cloned().unwrap_or_default(),
        audience: instructions_audience(lib, name),
        per_session: !laid_into_accounts(lib, name),
    }
}

/// Moves the one instructions file of old, `instructions.md`, to
/// `instructions/main.md`, the name settings already read its reach under.
/// Once moved there is nothing left to move. With both there — one put back
/// by hand, a library shared with an older Luna — neither is touched; the
/// old one stays, and the window says so.
pub(super) fn migrate_instructions(dirs: &Dirs) {
    let old = dirs.root.join(OLD_INSTRUCTIONS);
    let new = dirs.instructions_file(crate::settings::FIRST_INSTRUCTIONS);
    if !old.is_file() || std::fs::symlink_metadata(&new).is_ok() {
        return;
    }
    // A rename within the library: the file is at one name or the other at
    // every moment, never at neither.
    match std::fs::create_dir_all(dirs.instructions()).and_then(|()| std::fs::rename(&old, &new)) {
        Ok(()) => crate::log::info("library", &format!("moved {} to {}", old.display(), new.display())),
        Err(e) => crate::log::warn("library", &format!("could not move {} to {}: {e}", old.display(), new.display())),
    }
}

/// A name for instructions taken from an account: the account's, made one
/// `check_name` takes, and none the library has yet.
pub(super) fn new_instructions_name(dirs: &Dirs, account: &str) -> String {
    let base: String = account.to_lowercase().chars().map(|c| if c.is_ascii_lowercase() || c.is_ascii_digit() { c } else { '-' }).collect();
    let base: String = base.trim_start_matches('-').chars().take(56).collect();
    let base = if base.is_empty() { "instructions".to_string() } else { base };
    let free = |n: &str| std::fs::symlink_metadata(dirs.instructions_file(n)).is_err();
    if free(&base) {
        return base;
    }
    (2..).map(|i| format!("{base}-{i}")).find(|n| free(n)).unwrap_or(base)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::FIRST_INSTRUCTIONS;

    #[test]
    fn the_description_is_read_the_ways_yaml_writes_a_line_and_never_judged() {
        assert_eq!(description("---\nname: review\ndescription: Reviews code.\n---\nbody"), "Reviews code.");
        assert_eq!(description("---\nname: \"review\"\ndescription: 'Quoted: yes'\n---\n"), "Quoted: yes");
        assert_eq!(description("---\nname: review\ndescription: >\n  Folded\n  over lines\nother: x\n---\n"), "Folded over lines");
        assert_eq!(description("---\nname: review\ndescription: Plain\n  continued\n---\n"), "Plain continued");
        // Whatever else the file is, it shows what it can and no more.
        assert_eq!(description("no front matter"), "");
        assert_eq!(description("---\nname: review\ndescription: x\n"), "");
        assert_eq!(description("---\nname: other\n---\n"), "");
    }

    #[test]
    fn a_skill_name_is_only_a_name_a_folder_can_have() {
        for ok in ["review", "Code Review", "ünï-cödé", "v1.2", "a_b", "console"] {
            assert!(check_skill_name(ok).is_ok(), "{ok}");
        }
        for bad in ["", " review", "review ", ".hidden", "dots.", "a/b", "a\\b", "a:b", "a*b", "a\"b", "a|b", "a\nb", "NUL", "con.txt", "Com1", "lpt9"] {
            assert!(check_skill_name(bad).is_err(), "{bad:?}");
        }
        assert!(check_skill_name(&"x".repeat(256)).is_err());
    }

    #[test]
    fn a_skill_is_laid_out_as_it_is_whatever_its_skill_md_says() {
        let f = Fixture::new("as-is");
        let long = "d".repeat(2000);
        let put = |name: &str, text: &str| write_file(&f.dirs.skills().join(name).join(SKILL_FILE), text.as_bytes()).unwrap();
        put("long", &format!("# No front matter\n\ndescription: {long}\n"));
        put("Mixed Case", &format!("---\nname: something-else\ndescription: {long}\n---\n"));
        put("empty", "");
        let found = read_skills(&f.dirs).unwrap();
        for name in ["long", "Mixed Case", "empty"] {
            let (info, files) = &found[name];
            assert!(info.problem.is_none(), "{name}: {:?}", info.problem);
            assert!(files.as_ref().is_ok_and(|f| f.contains_key(SKILL_FILE)), "{name}");
        }
        assert_eq!(found["Mixed Case"].0.description.len(), 2000, "shown as written, not cut");

        let lib = LibrarySettings { skills: skills(&["long", "Mixed Case"]), ..Default::default() };
        let p = f.run(&lib, &mut Ledger::default(), true);
        assert!(p.places.iter().all(|p| p.problem.is_none()), "{:?}", p.places);
        // Claude Code's accounts and Codex's get the same copy.
        for account in 0..3 {
            for name in ["long", "Mixed Case"] {
                let copy = f.account(account).join("skills").join(name).join(SKILL_FILE);
                assert!(std::fs::read_to_string(copy).unwrap().contains(&long), "account {account}: {name}");
            }
        }
    }

    #[test]
    fn a_folder_without_skill_md_is_the_one_thing_a_skill_lacks() {
        let f = Fixture::new("bare-folder");
        std::fs::create_dir_all(f.dirs.skills().join("bare")).unwrap();
        std::fs::write(f.dirs.skills().join("bare/notes.md"), "x").unwrap();
        let found = read_skills(&f.dirs).unwrap();
        assert_eq!(found["bare"].0.problem.as_deref(), Some("There is no SKILL.md"));
        assert!(found["bare"].1.is_err());
    }

    #[test]
    fn instructions_are_told_by_their_first_heading() {
        assert_eq!(instructions_title("# Talking to the operator\n\n## Language\n"), "Talking to the operator");
        assert_eq!(instructions_title("Some prose first.\n\n## Language\n\n# Late title\n"), "Late title");
        assert_eq!(instructions_title("Some prose first.\n\n## Language\n## Reports\n"), "Language");
        assert_eq!(instructions_title("```sh\n# a comment\n```\n## Real\n"), "Real");
        assert_eq!(instructions_title("#tag\n### Deep\n#\n"), "");
        assert_eq!(instructions_title("Only prose, cut off somewhere.\n"), "");
    }

    #[test]
    fn blank_instructions_lay_nothing_out() {
        let f = Fixture::new("blank");
        f.instructions("main", " \n");
        let lib = LibrarySettings { instructions: first(all()), ..Default::default() };
        let p = f.run(&lib, &mut Ledger::default(), true);
        assert!(p.places.is_empty());
        assert!(!f.account(0).join("CLAUDE.md").exists());
    }

    #[test]
    fn the_one_instructions_file_of_old_moves_to_main_once() {
        let f = Fixture::new("migrate");
        let old = f.dirs.root.join(OLD_INSTRUCTIONS);
        let main = f.dirs.instructions_file(FIRST_INSTRUCTIONS);
        std::fs::write(&old, "Be brief.\n").unwrap();
        assert!(read_instructions(&f.dirs).is_err(), "not moved yet: no instructions is not what the library has");

        migrate_instructions(&f.dirs);
        assert!(!old.exists());
        assert_eq!(std::fs::read_to_string(&main).unwrap(), "Be brief.\n");
        let found = read_instructions(&f.dirs).unwrap();
        assert_eq!(found[FIRST_INSTRUCTIONS], Ok(Some(b"Be brief.\n".to_vec())));
        migrate_instructions(&f.dirs);
        assert_eq!(std::fs::read_to_string(&main).unwrap(), "Be brief.\n", "a second run moves nothing");

        // Both there: neither is touched, and the folder's is the one read.
        std::fs::write(&old, "Older.\n").unwrap();
        migrate_instructions(&f.dirs);
        assert_eq!(std::fs::read_to_string(&old).unwrap(), "Older.\n");
        assert_eq!(std::fs::read_to_string(&main).unwrap(), "Be brief.\n");
        assert_eq!(read_instructions(&f.dirs).unwrap()[FIRST_INSTRUCTIONS], Ok(Some(b"Be brief.\n".to_vec())));
    }

    #[test]
    fn settings_of_one_instructions_read_as_mains() {
        let read = |json: &str| serde_json::from_str::<LibrarySettings>(json).unwrap();
        assert_eq!(read(r#"{"instructions": ["claude", "codex/home"]}"#).instructions, first(reach(&["claude", "codex/home"])));
        assert_eq!(read(r#"{"instructions": true}"#).instructions, first(all()));
        assert!(read(r#"{"instructions": []}"#).instructions.is_empty());
        assert!(read(r#"{"instructions": false}"#).instructions.is_empty());
        let now = read(r#"{"instructions": {"main": ["all"], "work": ["claude/work"]}, "noInstructions": ["codex/home"]}"#);
        assert_eq!(now.instructions["work"], reach(&["claude/work"]));
        assert_eq!(now.no_instructions, reach(&["codex/home"]));
        let back = read(&serde_json::to_string(&now).unwrap());
        assert_eq!((back.instructions, back.no_instructions), (now.instructions, now.no_instructions));
    }

    #[test]
    fn instructions_taken_from_an_account_are_named_after_it() {
        let f = Fixture::new("names");
        assert_eq!(new_instructions_name(&f.dirs, "Work Laptop"), "work-laptop");
        assert_eq!(new_instructions_name(&f.dirs, "--"), "instructions");
        f.instructions("work", "x");
        assert_eq!(new_instructions_name(&f.dirs, "work"), "work-2");
        assert!(check_name(&new_instructions_name(&f.dirs, "Ünïcode.name")).is_ok());
    }
}
