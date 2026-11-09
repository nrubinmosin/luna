//! System skills: text Luna serves at its own trigger, never laid out
//! (docs/adr/0022-content-luna-serves-at-its-own-trigger.md). There is one,
//! `session-handoff`, what a chat with auto-handoff on is given to write its
//! handoff by (agents/handoff). Luna's own text is compiled in, so it cannot be
//! missing; the person may override it with one file for everyone,
//! `<library>/system/session-handoff/SKILL.md`, checked as a library skill is.
//! An override that will not do is not served: Luna's own is, and the reason
//! goes with it.
//!
//! Nothing here writes anywhere. The pass reads only `skills/` and
//! `instructions/`, so `system/` reaches no account or folder, and a library
//! skill of the same name under `skills/` is not the override (`resolve_in`
//! never reads it).
//!
//! - `handoff_instructions`: what is served now, `Resolved`, with where the
//!   override is or would go (`path`) and why one there is not used.
//! - `Resolved::state`: the same as the window is shown it, `SystemSkillState`.
//! - `LUNA_HANDOFF`: Luna's own text, what an override starts from or is
//!   compared with.

use super::*;

/// The handoff's instructions, by their skill name: what the override's folder
/// and its front matter are called, and the name a library skill does not take.
pub const HANDOFF: &str = "session-handoff";

/// Under the library's root: the system skills' overrides, a folder each.
const SYSTEM_DIR: &str = "system";

/// The most an override's SKILL.md may hold: all of it goes into the chat's
/// context at once, as a tool's answer.
pub const SYSTEM_MAX_BYTES: usize = 64 * 1024;

/// Luna's own instructions for a handoff: its version's, compiled in.
pub const LUNA_HANDOFF: &str = include_str!("../../resources/skills/session-handoff/SKILL.md");

/// Whose text is served.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    /// Luna's own, of this version.
    Luna,
    /// The person's override in the library.
    Library,
}

/// What a system skill resolves to now.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    pub source: Source,
    /// The SKILL.md served, front matter and all.
    pub text: String,
    /// SHA-256 of `text`, as hex: what tells one text from another.
    pub sha256: String,
    /// Where the override is, or would be: used only when `source` says so.
    pub path: PathBuf,
    /// Why the override there is not used: there is one, and it will not do.
    pub problem: Option<String>,
}

impl Resolved {
    /// The text past its front matter: what a session reads.
    pub fn body(&self) -> &str {
        body_of(&self.text)
    }

    /// The first 12 characters of the hash, as the window and a log line say it.
    pub fn short_sha(&self) -> &str {
        &self.sha256[..self.sha256.len().min(12)]
    }

    /// As the window shows it.
    pub fn state(&self) -> SystemSkillState {
        SystemSkillState {
            name: HANDOFF,
            state: match (self.source, &self.problem) {
                (Source::Library, _) => SystemState::Library,
                (Source::Luna, Some(_)) => SystemState::Broken,
                (Source::Luna, None) => SystemState::Luna,
            },
            version: env!("CARGO_PKG_VERSION"),
            sha256: self.sha256.clone(),
            path: self.path.to_string_lossy().into_owned(),
            problem: self.problem.clone(),
        }
    }
}

/// Where a system skill stands, for the window.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SystemState {
    /// Luna's own: there is no override.
    Luna,
    /// The override is served.
    Library,
    /// There is an override, it will not do (`problem`), and Luna's own is served.
    Broken,
}

/// A system skill as the window is shown it: whose text is served, Luna's
/// version, the hash of what is served, where the override is or would go, and
/// why it is not used when it is not.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SystemSkillState {
    pub name: &'static str,
    pub state: SystemState,
    /// Luna's version: whose own text this is, where it is Luna's.
    pub version: &'static str,
    pub sha256: String,
    pub path: String,
    pub problem: Option<String>,
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// `text` past its front matter, the blank lines after it too; all of it when
/// it has none.
pub fn body_of(text: &str) -> &str {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.split_inclusive('\n');
    if lines.next().map(str::trim_end) != Some("---") {
        return text;
    }
    let mut at = text.find('\n').map_or(text.len(), |i| i + 1);
    for line in lines {
        at += line.len();
        if line.trim_end() == "---" {
            return text[at..].trim_start_matches(['\r', '\n']);
        }
    }
    text
}

impl Dirs {
    fn system(&self, name: &str) -> PathBuf {
        self.root.join(SYSTEM_DIR).join(name)
    }
}

/// What a chat with auto-handoff on is served to write its handoff by, now:
/// the override when there is one that will do, else Luna's own.
pub fn handoff_instructions() -> Resolved {
    resolve_in(&Dirs::current())
}

fn resolve_in(dirs: &Dirs) -> Resolved {
    let path = dirs.system(HANDOFF).join(SKILL_FILE);
    let luna = |problem: Option<String>| Resolved {
        source: Source::Luna,
        text: LUNA_HANDOFF.to_string(),
        sha256: sha256_hex(LUNA_HANDOFF.as_bytes()),
        path: path.clone(),
        problem,
    };
    match read_override(dirs) {
        Ok(None) => luna(None),
        Ok(Some(text)) => {
            Resolved { source: Source::Library, sha256: sha256_hex(text.as_bytes()), text, path: path.clone(), problem: None }
        }
        Err(why) => luna(Some(why)),
    }
}

/// The override's SKILL.md if it will do; None where there is none; why not
/// where it will not. Checked as the library's own skills are (`check_skill`,
/// no link anywhere in its folder), and held to SYSTEM_MAX_BYTES of UTF-8 with
/// something past its front matter.
fn read_override(dirs: &Dirs) -> Result<Option<String>, String> {
    let system = dirs.root.join(SYSTEM_DIR);
    let dir = dirs.system(HANDOFF);
    for d in [&system, &dir] {
        match std::fs::symlink_metadata(d) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("{}: {e}", d.display())),
            Ok(m) if m.file_type().is_symlink() => return Err(format!("{} is a link; the override is a folder of its own", d.display())),
            Ok(m) if !m.is_dir() => return Err(format!("{} is not a folder", d.display())),
            Ok(_) => {}
        }
    }
    let files = read_dir_files(&dir)?;
    let bytes = files.get(SKILL_FILE).ok_or("There is no SKILL.md")?;
    if bytes.len() > SYSTEM_MAX_BYTES {
        return Err(format!("SKILL.md is over {} KB", SYSTEM_MAX_BYTES / 1024));
    }
    let text = String::from_utf8(bytes.clone()).map_err(|_| "SKILL.md is not UTF-8 text".to_string())?;
    check_skill(HANDOFF, &text)?;
    if body_of(&text).trim().is_empty() {
        return Err("SKILL.md has nothing past its front matter".into());
    }
    Ok(Some(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_override(f: &Fixture, text: &str) -> PathBuf {
        let path = f.dirs.system(HANDOFF).join(SKILL_FILE);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn luna_s_own_text_is_a_skill_of_its_name() {
        assert!(check_skill(HANDOFF, LUNA_HANDOFF).is_ok(), "{:?}", check_skill(HANDOFF, LUNA_HANDOFF));
        assert!(LUNA_HANDOFF.len() <= SYSTEM_MAX_BYTES);
        assert!(!body_of(LUNA_HANDOFF).starts_with("---") && !body_of(LUNA_HANDOFF).trim().is_empty());
    }

    /// Luna's own text goes in with LF endings from every checkout
    /// (.gitattributes), so the hash a handoff's record names is the same on
    /// every machine; a change to the text changes it here, on purpose.
    #[test]
    fn luna_s_own_text_has_one_hash_from_every_checkout() {
        assert!(!LUNA_HANDOFF.contains('\r'));
        assert_eq!(sha256_hex(LUNA_HANDOFF.as_bytes()), "3054a9e14c774f592c9c2fed08da4ff7eb38befdb00321184f3d1b324c8aedd9");
    }

    /// The override is served, never laid out: a pass that lays the
    /// library's skills and instructions into every account leaves no trace of
    /// it in any of them.
    #[test]
    fn the_override_reaches_no_account() {
        let f = Fixture::new("system-nowhere");
        write_override(&f, "---\nname: session-handoff\ndescription: Ours.\n---\nBody\n");
        f.skill("review", "Reviews.");
        f.instructions("main", "Be brief.\n");
        let lib = LibrarySettings { skills: skills(&["review"]), instructions: first(all()), ..Default::default() };
        let p = f.run(&lib, &mut Ledger::default(), true);
        assert!(!p.places.is_empty() && f.account(0).join("skills").join("review").is_dir(), "the rest is laid out");
        fn walk(dir: &Path, found: &mut Vec<PathBuf>) {
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.contains(HANDOFF) || name == SYSTEM_DIR {
                    found.push(path.clone());
                }
                if path.is_dir() {
                    walk(&path, found);
                }
            }
        }
        let mut found = Vec::new();
        walk(&f.base.join("accounts"), &mut found);
        walk(&f.dirs.folders(), &mut found);
        assert!(found.is_empty(), "{found:?}");
        assert!(p.places.iter().all(|place| !place.dest.contains(HANDOFF)));
    }

    #[test]
    fn without_an_override_luna_s_own_is_served() {
        let f = Fixture::new("system-none");
        let r = resolve_in(&f.dirs);
        assert_eq!((r.source, r.problem.as_deref(), r.text.as_str()), (Source::Luna, None, LUNA_HANDOFF));
        assert_eq!(r.sha256, sha256_hex(LUNA_HANDOFF.as_bytes()));
        assert_eq!(r.path, f.dirs.root.join("system").join("session-handoff").join("SKILL.md"));
        let s = r.state();
        assert_eq!((s.state, s.version, s.name), (SystemState::Luna, env!("CARGO_PKG_VERSION"), "session-handoff"));
        // A library skill of the same name is not the override.
        f.skill(HANDOFF, "Mine, laid out as a skill.");
        assert_eq!(resolve_in(&f.dirs).source, Source::Luna, "skills/session-handoff is never read for it");
    }

    #[test]
    fn an_override_that_will_do_is_served_over_luna_s() {
        let f = Fixture::new("system-ok");
        let text = "---\nname: session-handoff\ndescription: Ours.\n---\n\n# Our handoff\nWrite it our way.\n";
        let path = write_override(&f, text);
        let r = resolve_in(&f.dirs);
        assert_eq!((r.source, r.problem.as_deref(), r.text.as_str(), &r.path), (Source::Library, None, text, &path));
        assert_eq!(r.sha256, sha256_hex(text.as_bytes()));
        assert_eq!(r.body(), "# Our handoff\nWrite it our way.\n");
        assert_eq!(r.state().state, SystemState::Library);
        assert_eq!(r.short_sha().len(), 12);
    }

    #[test]
    fn a_broken_override_falls_back_to_luna_s_and_says_why() {
        let f = Fixture::new("system-broken");
        for (text, why) in [
            ("no front matter", "front matter"),
            ("---\nname: other\ndescription: x\n---\nbody", "other"),
            ("---\nname: session-handoff\n---\nbody", "description"),
            ("---\nname: session-handoff\ndescription: x\n---\n \n", "nothing past"),
        ] {
            write_override(&f, text);
            let r = resolve_in(&f.dirs);
            assert_eq!((r.source, r.text.as_str()), (Source::Luna, LUNA_HANDOFF), "{text}");
            assert!(r.problem.as_deref().is_some_and(|p| p.contains(why)), "{text}: {:?}", r.problem);
            assert_eq!(r.state().state, SystemState::Broken);
        }
        let path = write_override(&f, "x");
        std::fs::write(&path, [0x2d, 0x2d, 0x2d, 0x0a, 0xff]).unwrap();
        assert!(resolve_in(&f.dirs).problem.unwrap().contains("UTF-8"));
        let big = format!("---\nname: session-handoff\ndescription: x\n---\n{}", "a".repeat(SYSTEM_MAX_BYTES));
        std::fs::write(&path, big).unwrap();
        assert!(resolve_in(&f.dirs).problem.unwrap().contains("KB"));
        // A folder without SKILL.md is an override that will not do, not none.
        std::fs::remove_file(&path).unwrap();
        assert!(resolve_in(&f.dirs).problem.unwrap().contains("no SKILL.md"));
    }

    #[test]
    fn front_matter_is_left_out_of_what_is_read() {
        assert_eq!(body_of("---\nname: a\n---\n\nBody\n"), "Body\n");
        assert_eq!(body_of("\u{feff}---\r\nname: a\r\n---\r\nBody"), "Body");
        assert_eq!(body_of("No front matter\n---\n"), "No front matter\n---\n");
        assert_eq!(body_of("---\nnever closed\n"), "---\nnever closed\n");
    }
}
