//! The library's skills and instructions files as it holds them: read, checked
//! fit to lay out, and named.

use super::*;

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    /// Why it is laid out nowhere, while it is not fit to be.
    pub problem: Option<String>,
    /// The accounts it goes into, wherever no folder says otherwise.
    pub accounts: Reach,
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

/// `name` and `description` from SKILL.md's front matter — what both CLIs
/// need of it — as plain strings: quoted, folded (`>`) and literal (`|`)
/// values and indented continuation lines all read.
fn front_matter(text: &str) -> Result<(Option<String>, Option<String>), String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.lines();
    if lines.next().map(str::trim_end) != Some("---") {
        return Err("SKILL.md does not open with front matter (a --- line)".into());
    }
    let mut keys: BTreeMap<&str, (String, bool)> = BTreeMap::new();
    let mut current: Option<&str> = None;
    for line in lines {
        if line.trim_end() == "---" {
            let get = |k: &str| {
                keys.get(k).map(|(v, _)| {
                    let v = v.trim();
                    let unquoted = (v.len() >= 2
                        && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\''))))
                    .then(|| &v[1..v.len() - 1]);
                    unquoted.unwrap_or(v).to_string()
                })
            };
            return Ok((get("name"), get("description")));
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
    Err("SKILL.md's front matter is never closed (a second --- line)".into())
}

/// Why this SKILL.md would not do for a skill of this name, if it would not.
pub(super) fn check_skill(name: &str, text: &str) -> Result<String, String> {
    check_name(name)?;
    let (n, description) = front_matter(text)?;
    match n.as_deref() {
        None | Some("") => return Err("The front matter has no name".into()),
        Some(n) if n != name => return Err(format!("The front matter says name: {n}, the folder is {name}")),
        _ => {}
    }
    let description = description.filter(|d| !d.is_empty()).ok_or("The front matter has no description")?;
    if description.chars().count() > 1024 {
        return Err("The description is longer than 1024 characters".into());
    }
    Ok(description)
}

/// Every skill in the library, by name, with its files when it is fit to be
/// laid out. A `skills/` that will not list is an error, not an empty one: a
/// skill missed reads as taken out of the library, and its copies with it.
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
        let files = read_dir_files(&entry.path());
        let checked = files.as_ref().map_err(String::clone).and_then(|f| {
            let text = f.get(SKILL_FILE).ok_or("There is no SKILL.md")?;
            check_skill(&name, &String::from_utf8_lossy(text))
        });
        let (description, problem) = match &checked {
            Ok(d) => (d.clone(), None),
            Err(e) => (String::new(), Some(e.clone())),
        };
        let files = checked.and(files);
        out.insert(name.clone(), (SkillInfo { name, description, problem, accounts: Vec::new() }, files));
    }
    Ok(out)
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct InstructionsInfo {
    pub name: String,
    /// Its first heading, to tell it by (`instructions_title`); empty without one.
    pub description: String,
    /// Why it is laid out nowhere, while it is not fit to be.
    pub problem: Option<String>,
    /// The accounts its reach takes in; each gets it unless a closer reach
    /// gives it other instructions.
    pub accounts: Reach,
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
    InstructionsInfo { name: name.to_string(), description, problem, accounts: lib.instructions.get(name).cloned().unwrap_or_default() }
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
    fn front_matter_reads_the_ways_yaml_writes_a_line() {
        let ok = |t: &str| check_skill("review", t);
        assert_eq!(ok("---\nname: review\ndescription: Reviews code.\n---\nbody").unwrap(), "Reviews code.");
        assert_eq!(ok("---\nname: \"review\"\ndescription: 'Quoted: yes'\n---\n").unwrap(), "Quoted: yes");
        assert_eq!(ok("---\nname: review\ndescription: >\n  Folded\n  over lines\nother: x\n---\n").unwrap(), "Folded over lines");
        assert_eq!(ok("---\nname: review\ndescription: Plain\n  continued\n---\n").unwrap(), "Plain continued");
        assert!(ok("no front matter").unwrap_err().contains("front matter"));
        assert!(ok("---\nname: review\ndescription: x\n").unwrap_err().contains("never closed"));
        assert!(ok("---\nname: other\ndescription: x\n---\n").unwrap_err().contains("other"));
        assert!(ok("---\nname: review\n---\n").unwrap_err().contains("description"));
        assert!(check_skill("Bad Name", "---\nname: Bad Name\ndescription: x\n---\n").is_err());
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
