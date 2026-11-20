//! A Claude Code session's folder skills as plugins of its own: one folder per
//! session under `<data>/session-skills/<session>/`, holding a plugin for each
//! library folder the session works in, passed with `--plugin-dir` (Claude Code
//! loads each child of a folder of plugins, 2.1.325). Only the skills this
//! session is given are in it, so nothing is hidden by name afterwards (Claude
//! Code's `skillOverrides` does not reach a plugin's skills), and nothing waits
//! in an account or a project: the folder is Luna's own, private, written at
//! every start and every helper's turn and gone with the session.
//!
//! A plugin's name is `luna-<folder slug>` (`plugin_name`), stable for the
//! folder, so its skills are listed as `luna-<slug>:<skill>`: no name of the
//! account's or the repository's is taken, and no plugin the account has
//! enabled, named for a folder (`github`), is shadowed or shadows it.
//! `claudeFolderCopies` in the library's settings keeps the earlier way —
//! copies under `library-folders/`, passed with `--add-dir` — while this one
//! is piloted; a session launched on it holds those copies where they are
//! (`note_fallback`).

use super::*;

/// The folder under Luna's data folder where each session's plugins are made.
pub(crate) const PLUGINS_DIR: &str = "session-skills";

/// What every plugin's name starts with.
const PLUGIN_PREFIX: &str = "luna-";

/// The name of the plugin a library folder's skills are given as.
pub fn plugin_name(slug: &str) -> String {
    format!("{PLUGIN_PREFIX}{slug}")
}

/// One library folder's skills for one session, with the files each has now:
/// the plugin `luna-<slug>`, its skills listed as `luna-<slug>:<skill>`.
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
/// without the skills (`library.skills-off`).
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

/// Takes away the plugin folder named `name` (a session's id as a file name)
/// that no session needs: the startup sweep's.
pub(crate) fn drop_plugins_named(name: &str) {
    drop_plugins_at(&crate::paths::data_dir(), name);
}

/// `session-skills` and the session's folder in it, each a plain folder or
/// not there: a link or junction at either (the data folder itself is Luna's
/// to place, so only looked past) refuses what would be written or removed
/// through it.
fn plain_chain(data: &Path, name: &str) -> Result<PathBuf, String> {
    crate::boundary::walk_down(&data.join(PLUGINS_DIR), &[name], |dir, _| match std::fs::symlink_metadata(dir) {
        Ok(meta) if crate::boundary::is_link(&meta) => Err(format!("{} is a link", dir.display())),
        _ => Ok(()),
    })
}

fn drop_plugins_in(data: &Path, session: &str) {
    drop_plugins_at(data, &crate::paths::id_file_name(session));
}

fn drop_plugins_at(data: &Path, name: &str) {
    let dir = match plain_chain(data, name) {
        Ok(dir) => dir,
        Err(why) => {
            crate::log::warn("library", &format!("Luna leaves the folder skills named {name} alone: {why}"));
            return;
        }
    };
    // No link on the way down, and `remove_dir_all` follows none below.
    if std::fs::symlink_metadata(&dir).is_ok() {
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            crate::log::warn("library", &format!("could not take away {}: {e}", dir.display()));
        }
    }
}

#[cfg(test)]
fn plugins_of(data: &Path, session: &str) -> PathBuf {
    data.join(PLUGINS_DIR).join(crate::paths::id_file_name(session))
}

/// The session's folder made afresh: nothing an earlier start left is kept or
/// trusted, and `session-skills` and below are Luna's, private, no link
/// (`boundary::private_chain`), as the files in it (`write_file`). The data
/// folder is not made private here: where it is shared (paths.rs `scope`) its
/// own access is the person's, and only Luna's own folders in it are Luna's to
/// lock. A write that fails takes what it wrote away.
fn write_plugins(data: &Path, session: &str, plugins: &[FolderPlugin]) -> Result<PathBuf, String> {
    let name = crate::paths::id_file_name(session);
    plain_chain(data, &name)?;
    let top = data.join(PLUGINS_DIR);
    crate::boundary::private_chain(&top, &[])?;
    drop_plugins_at(data, &name);
    let root = crate::boundary::private_chain(&top, &[name.as_str()])?;
    let written = (|| {
        for plugin in plugins {
            let at = root.join(&plugin.slug);
            let manifest = serde_json::json!({
                "name": plugin_name(&plugin.slug),
                "description": "Skills Luna's library gives this session's folder",
            });
            write_file(&at.join(".claude-plugin").join("plugin.json"), (manifest.to_string() + "\n").as_bytes())?;
            for (skill, files) in &plugin.skills {
                for (rel, bytes) in files {
                    write_file(&at.join("skills").join(skill).join(rel), bytes)?;
                }
            }
        }
        Ok::<(), String>(())
    })();
    match written {
        Ok(()) => Ok(root),
        Err(why) => {
            drop_plugins_at(data, &name);
            Err(why)
        }
    }
}

/// The folders under `library-folders/` that each session launched on the
/// fallback (`claudeFolderCopies`) was given with `--add-dir`: the copies in
/// them are in use until it ends, so a pass that no longer lays them out
/// leaves them where they are (`held_copies`).
static FALLBACK: Mutex<BTreeMap<String, Vec<PathBuf>>> = Mutex::new(BTreeMap::new());

#[cfg(test)]
thread_local! {
    /// The sessions a test says are running.
    static RUNNING: std::cell::RefCell<BTreeSet<String>> = const { std::cell::RefCell::new(BTreeSet::new()) };
}

fn session_runs(id: &str) -> bool {
    #[cfg(test)]
    if RUNNING.with(|r| r.borrow().contains(id)) {
        return true;
    }
    crate::pty::manager().alive(id) || crate::background::has(id)
}

/// Notes that `session` was launched with these `skill_args`: each `--add-dir`
/// under Luna's `library-folders/` is a Claude Code folder copy it may read
/// until it ends.
pub fn note_fallback(session: &str, skill_args: &[String]) {
    let top = crate::paths::data_dir().join(FOLDERS_DIR);
    let held: Vec<PathBuf> =
        skill_args.windows(2).filter(|w| w[0] == "--add-dir").map(|w| PathBuf::from(&w[1])).filter(|p| within(p, &top)).collect();
    let mut notes = FALLBACK.lock().unwrap_or_else(|e| e.into_inner());
    notes.retain(|id, _| id == session || session_runs(id));
    if held.is_empty() {
        notes.remove(session);
    } else {
        notes.insert(session.to_string(), held);
    }
}

/// A session running that was launched on the copies in `dir`, or no longer.
#[cfg(test)]
pub(super) fn pretend_fallback_session(session: &str, dir: Option<PathBuf>) {
    let mut notes = FALLBACK.lock().unwrap_or_else(|e| e.into_inner());
    match dir {
        Some(dir) => {
            notes.insert(session.to_string(), vec![dir]);
            RUNNING.with(|r| r.borrow_mut().insert(session.to_string()));
        }
        None => {
            notes.remove(session);
            RUNNING.with(|r| r.borrow_mut().remove(session));
        }
    }
}

/// The folder copies a session still running was launched on.
pub(super) fn held_copies() -> Vec<PathBuf> {
    let notes = FALLBACK.lock().unwrap_or_else(|e| e.into_inner());
    notes.iter().filter(|(id, _)| session_runs(id)).flat_map(|(_, dirs)| dirs.clone()).collect()
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
        assert_eq!(manifest["name"], "luna-repo", "named for Luna, so no plugin of the account's by the folder's name is shadowed");
        assert_eq!(plugin_name("outer"), "luna-outer");
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

    /// A folder of plugins that a junction stands for stays as it is under
    /// every way Luna takes plugins away or makes them: the session's own is
    /// left alone as a link, and what it leads to is never written or removed.
    #[test]
    fn a_link_at_the_session_folder_is_left_alone_by_every_path() {
        let f = Fixture::new("plugin-link-session");
        let elsewhere = f.base.join("elsewhere");
        write_file(&elsewhere.join("keep.txt"), b"theirs").unwrap();
        std::fs::create_dir_all(f.dirs.data.join("session-skills")).unwrap();
        let link = plugins_of(&f.dirs.data, "s");
        link_dir(&link, &elsewhere);
        let mut skills = SessionSkills { plugins: vec![plugin("repo", &[("review", "v1")])], ..Default::default() };
        assert!(give_plugins_in(&f.dirs.data, "s", &mut skills).is_err(), "refused, not replaced");
        assert!(skills.args.is_empty());
        let mut none = SessionSkills::default();
        give_plugins_in(&f.dirs.data, "s", &mut none).unwrap();
        drop_plugins_in(&f.dirs.data, "s");
        drop_plugins_named(&crate::paths::id_file_name("s"));
        assert_eq!(std::fs::read(elsewhere.join("keep.txt")).unwrap(), b"theirs");
        assert!(!elsewhere.join("repo").exists());
        assert!(std::fs::symlink_metadata(&link).is_ok(), "the link itself stays too");
        unlink_dir(&link);
    }

    /// A junction at `session-skills` leads to a folder holding the session's
    /// folder: neither a chat's delete (`drop_plugins`) nor a start with no
    /// folder skills (`give_plugins`) nor the sweep removes anything through it,
    /// and a start with skills writes nothing there.
    #[test]
    fn a_link_at_session_skills_is_left_alone_by_every_path() {
        let f = Fixture::new("plugin-link-top");
        let elsewhere = f.base.join("elsewhere");
        let theirs = elsewhere.join(crate::paths::id_file_name("s"));
        write_file(&theirs.join("keep.txt"), b"theirs").unwrap();
        std::fs::create_dir_all(&f.dirs.data).unwrap();
        let top = f.dirs.data.join("session-skills");
        link_dir(&top, &elsewhere);

        drop_plugins_in(&f.dirs.data, "s");
        assert!(theirs.join("keep.txt").is_file(), "a chat's delete");
        let mut none = SessionSkills::default();
        give_plugins_in(&f.dirs.data, "s", &mut none).unwrap();
        assert!(theirs.join("keep.txt").is_file(), "a start with no folder skills");
        drop_plugins_named(&crate::paths::id_file_name("s"));
        assert!(theirs.join("keep.txt").is_file(), "the sweep");
        let mut some = SessionSkills { plugins: vec![plugin("repo", &[("review", "v1")])], ..Default::default() };
        assert!(give_plugins_in(&f.dirs.data, "s", &mut some).is_err());
        assert!(!theirs.join("repo").exists() && !elsewhere.join("t").exists());
        assert!(some.args.is_empty());
        unlink_dir(&top);
    }

    /// Making a session's plugin never reapplies access to the data folder:
    /// where that is shared (paths.rs `scope`) it is the person's. Only
    /// `session-skills` and what is below it are Luna's to lock.
    #[test]
    fn the_data_folders_own_access_is_not_touched() {
        fn access(path: &Path) -> String {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                format!("{:o}", std::fs::metadata(path).unwrap().permissions().mode() & 0o7777)
            }
            #[cfg(windows)]
            {
                let out = std::process::Command::new("icacls").arg(path).output().unwrap();
                String::from_utf8_lossy(&out.stdout).replace(&*path.to_string_lossy(), "")
            }
        }
        let f = Fixture::new("plugin-acl");
        std::fs::create_dir_all(&f.dirs.data).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&f.dirs.data, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let before = access(&f.dirs.data);
        let mut skills = SessionSkills { plugins: vec![plugin("repo", &[("review", "v1")])], ..Default::default() };
        give_plugins_in(&f.dirs.data, "s", &mut skills).unwrap();
        assert_eq!(access(&f.dirs.data), before, "the data folder's access is as the person left it");
        #[cfg(unix)]
        assert_eq!(access(&f.dirs.data.join("session-skills")), "700", "while Luna's own folder is private");
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

    /// A session on the fallback holds the folder copies it was given while it
    /// runs, and no longer once it does not.
    #[test]
    fn a_running_fallback_session_holds_its_copies() {
        let top = crate::paths::data_dir().join(FOLDERS_DIR);
        let mine = top.join("repo").join("work");
        let args = vec!["--add-dir".to_string(), mine.to_string_lossy().into_owned(), "--add-dir".into(), "/elsewhere".into()];
        note_fallback("fallback-test-session", &args);
        assert!(!held_copies().contains(&mine), "not running: nothing held");
        RUNNING.with(|r| r.borrow_mut().insert("fallback-test-session".into()));
        note_fallback("fallback-test-session", &args);
        assert_eq!(held_copies().iter().filter(|p| **p == mine).count(), 1, "{:?}", held_copies());
        assert!(!held_copies().iter().any(|p| p.ends_with("elsewhere")), "only the copies under library-folders");
        RUNNING.with(|r| r.borrow_mut().clear());
        assert!(!held_copies().contains(&mine), "ended: no longer held");
    }
}
