//! A Claude Code session's folder skills as plugins of its own: one folder per
//! session under `<data>/session-skills/<session>/`, holding a plugin for each
//! library folder the session works in, passed with `--plugin-dir` (Claude Code
//! loads each child of a folder of plugins, 2.1.325). Only the skills this
//! session is given are in it, so nothing is hidden by name afterwards (Claude
//! Code's `skillOverrides` does not reach a plugin's skills), and nothing waits
//! in an account or a project: the folder is Luna's own, private, written at
//! every start and every helper's turn and gone with the session.
//!
//! A skill there is listed as `<folder slug>:<skill>`, as Codex lists a
//! folder's plugin, so no name of the account's or the repository's is taken.
//! `claudeFolderCopies` in the library's settings keeps the earlier way —
//! copies under `library-folders/`, passed with `--add-dir` — while this one
//! is piloted.

use super::*;

/// The folder under Luna's data folder where each session's plugins are made.
const PLUGINS_DIR: &str = "session-skills";

/// One library folder's skills for one session, with the files each has now:
/// the plugin `<slug>`, its skills listed as `<slug>:<skill>`.
#[derive(Default, Debug, PartialEq, Clone)]
pub struct FolderPlugin {
    pub slug: String,
    pub skills: BTreeMap<String, Files>,
}

/// The files of the library's skill `name`, when it is one fit to be laid out
/// (`read_skills`): a folder of its own, not a link, with a SKILL.md in it.
pub(super) fn skill_files(dirs: &Dirs, name: &str) -> Option<Files> {
    let dir = dirs.skills().join(name);
    let meta = std::fs::symlink_metadata(&dir).ok()?;
    if !meta.is_dir() || crate::boundary::is_link(&meta) || check_skill_name(name).is_err() {
        return None;
    }
    read_dir_files(&dir).ok().filter(|f| f.contains_key(SKILL_FILE))
}

/// Makes the plugins `skills` names for `session` and has its start pass them
/// (`--plugin-dir` on `skills.args`); with none, takes away what an earlier
/// start made. Err, with why, where they will not write: the session starts
/// without the skills (a chat says so, `library.skills-off`).
pub fn give_plugins(session: &str, skills: &mut SessionSkills) -> Result<(), String> {
    give_plugins_in(&crate::paths::data_dir(), session, skills)
}

fn give_plugins_in(data: &Path, session: &str, skills: &mut SessionSkills) -> Result<(), String> {
    if skills.plugins.is_empty() {
        drop_plugins_in(data, session);
        return Ok(());
    }
    let dir = write_plugins(data, session, &skills.plugins).map_err(|why| {
        let said = format!("Luna could not make the folder skills of {session}: {why}");
        crate::log::warn("library", &said);
        said
    })?;
    skills.args.extend(["--plugin-dir".to_string(), dir.to_string_lossy().into_owned()]);
    Ok(())
}

/// Takes away `session`'s plugins: its end, or a start with none.
pub fn drop_plugins(session: &str) {
    drop_plugins_in(&crate::paths::data_dir(), session);
}

fn drop_plugins_in(data: &Path, session: &str) {
    let dir = plugins_of(data, session);
    let Ok(meta) = std::fs::symlink_metadata(&dir) else { return };
    // A link where the folder goes is removed as itself, never followed.
    let gone = match crate::boundary::is_link(&meta) {
        true => std::fs::remove_dir(&dir).or_else(|_| std::fs::remove_file(&dir)),
        false => std::fs::remove_dir_all(&dir),
    };
    if let Err(e) = gone {
        crate::log::warn("library", &format!("could not take away {}: {e}", dir.display()));
    }
}

fn plugins_of(data: &Path, session: &str) -> PathBuf {
    data.join(PLUGINS_DIR).join(crate::paths::id_file_name(session))
}

/// The session's folder made afresh: nothing an earlier start left is kept or
/// trusted, and every folder on the way down is Luna's, private, no link
/// (`boundary::private_chain`), as the files in it (`write_file`).
fn write_plugins(data: &Path, session: &str, plugins: &[FolderPlugin]) -> Result<PathBuf, String> {
    crate::boundary::private_chain(data, &[PLUGINS_DIR])?;
    drop_plugins_in(data, session);
    let name = crate::paths::id_file_name(session);
    let root = crate::boundary::private_chain(data, &[PLUGINS_DIR, name.as_str()])?;
    for plugin in plugins {
        let at = root.join(&plugin.slug);
        let manifest = serde_json::json!({
            "name": plugin.slug,
            "description": "Skills Luna's library gives this session's folder",
        });
        write_file(&at.join(".claude-plugin").join("plugin.json"), (manifest.to_string() + "\n").as_bytes())?;
        for (skill, files) in &plugin.skills {
            for (rel, bytes) in files {
                write_file(&at.join("skills").join(skill).join(rel), bytes)?;
            }
        }
    }
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin(slug: &str, skills: &[(&str, &str)]) -> FolderPlugin {
        FolderPlugin {
            slug: slug.into(),
            skills: skills
                .iter()
                .map(|(n, body)| {
                    (
                        n.to_string(),
                        Files::from([(SKILL_FILE.to_string(), body.as_bytes().to_vec()), ("ref/a.md".to_string(), b"a".to_vec())]),
                    )
                })
                .collect(),
        }
    }

    #[test]
    fn a_session_gets_one_plugin_per_folder_in_a_folder_of_its_own() {
        let f = Fixture::new("plugin-write");
        let mut skills =
            SessionSkills { plugins: vec![plugin("repo", &[("review", "v1")]), plugin("outer", &[("notes", "n")])], ..Default::default() };
        give_plugins_in(&f.dirs.data, "chat:1", &mut skills).unwrap();
        let root = f.dirs.data.join("session-skills").join(crate::paths::id_file_name("chat:1"));
        assert_eq!(skills.args, ["--plugin-dir".to_string(), root.to_string_lossy().into_owned()]);
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("repo/.claude-plugin/plugin.json")).unwrap()).unwrap();
        assert_eq!(manifest["name"], "repo");
        assert_eq!(std::fs::read(root.join("repo/skills/review/SKILL.md")).unwrap(), b"v1");
        assert_eq!(std::fs::read(root.join("repo/skills/review/ref/a.md")).unwrap(), b"a");
        assert!(root.join("outer/skills/notes/SKILL.md").is_file());
    }

    #[test]
    fn a_start_makes_it_afresh_and_one_with_none_takes_it_away() {
        let f = Fixture::new("plugin-afresh");
        let mut first = SessionSkills { plugins: vec![plugin("repo", &[("review", "v1"), ("old", "o")])], ..Default::default() };
        give_plugins_in(&f.dirs.data, "s", &mut first).unwrap();
        let root = plugins_of(&f.dirs.data, "s");
        let mut second = SessionSkills { plugins: vec![plugin("repo", &[("review", "v2")])], ..Default::default() };
        give_plugins_in(&f.dirs.data, "s", &mut second).unwrap();
        assert_eq!(std::fs::read(root.join("repo/skills/review/SKILL.md")).unwrap(), b"v2");
        assert!(!root.join("repo/skills/old").exists(), "a skill no longer given is gone");

        let mut none = SessionSkills::default();
        give_plugins_in(&f.dirs.data, "s", &mut none).unwrap();
        assert!(none.args.is_empty() && !root.exists());
        drop_plugins_in(&f.dirs.data, "s");
    }

    /// A link or junction where the session's folder or the folder of all of
    /// them goes is never written through, and one at the session's folder is
    /// removed as itself.
    #[test]
    fn a_link_is_never_followed() {
        let f = Fixture::new("plugin-link");
        let elsewhere = f.base.join("elsewhere");
        write_file(&elsewhere.join("keep.txt"), b"theirs").unwrap();
        let top = f.dirs.data.join("session-skills");
        std::fs::create_dir_all(&top).unwrap();
        link_dir(&plugins_of(&f.dirs.data, "s"), &elsewhere);
        let mut skills = SessionSkills { plugins: vec![plugin("repo", &[("review", "v1")])], ..Default::default() };
        give_plugins_in(&f.dirs.data, "s", &mut skills).unwrap();
        assert_eq!(std::fs::read(elsewhere.join("keep.txt")).unwrap(), b"theirs");
        assert!(!elsewhere.join("repo").exists(), "nothing written through it");
        assert!(plugins_of(&f.dirs.data, "s").join("repo/skills/review/SKILL.md").is_file());

        // The folder of all of them led elsewhere: refused, nothing written there.
        let mut again = SessionSkills { plugins: vec![plugin("repo", &[("review", "v1")])], ..Default::default() };
        std::fs::remove_dir_all(&top).unwrap();
        link_dir(&top, &elsewhere);
        assert!(give_plugins_in(&f.dirs.data, "t", &mut again).is_err());
        assert!(again.args.is_empty());
        assert!(!elsewhere.join(crate::paths::id_file_name("t")).exists());
        unlink_dir(&top);
    }

    #[test]
    fn only_a_skill_fit_to_be_laid_out_is_read() {
        let f = Fixture::new("plugin-skill-files");
        f.skill("review", "v1");
        write_file(&f.dirs.skills().join("empty/readme.md"), b"x").unwrap();
        assert!(skill_files(&f.dirs, "review").is_some_and(|files| files.contains_key(SKILL_FILE)));
        assert!(skill_files(&f.dirs, "empty").is_none(), "no SKILL.md");
        assert!(skill_files(&f.dirs, "missing").is_none());
        let elsewhere = f.base.join("elsewhere");
        write_file(&elsewhere.join(SKILL_FILE), b"---\n---\n").unwrap();
        link_dir(&f.dirs.skills().join("linked"), &elsewhere);
        assert!(skill_files(&f.dirs, "linked").is_none(), "a link is not a skill's folder");
        unlink_dir(&f.dirs.skills().join("linked"));
    }
}
