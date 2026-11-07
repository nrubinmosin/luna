//! The folders the user names: the name each one's skills wait under, the Claude
//! Code folders and Codex plugins they wait in, and what a session started in
//! one, or in a worktree of it, is given and has hidden.

use super::*;

/// Each folder of the library with the name its skills wait under: the one it
/// keeps (`LibraryFolder::slug`), unless an earlier folder keeps that too, else
/// one made for it (`fresh_slug`) past those kept. A list with none kept, as
/// before they were, gets the names it always had: by its order.
pub(super) fn folder_slugs(folders: &[LibraryFolder]) -> Vec<(&LibraryFolder, String)> {
    let mut taken = BTreeSet::new();
    let kept: Vec<bool> = folders.iter().map(|f| slug_ok(&f.slug) && taken.insert(f.slug.clone())).collect();
    folders
        .iter()
        .zip(kept)
        .map(|(f, kept)| {
            if kept {
                return (f, f.slug.clone());
            }
            let slug = fresh_slug(&f.path, &taken);
            taken.insert(slug.clone());
            (f, slug)
        })
        .collect()
}

/// A name for the folder at `path` to keep its skills under: its own name as
/// `check_name` takes names — the prefix Codex lists them by, and a part of a
/// `-c` key, which Codex splits at every dot — with `-2`, `-3` after one taken.
pub(super) fn fresh_slug(path: &str, taken: &BTreeSet<String>) -> String {
    let own = Path::new(path).file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    let base: String = own.chars().map(|c| if c.is_ascii_lowercase() || c.is_ascii_digit() { c } else { '-' }).collect();
    let base: String = base.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-").chars().take(56).collect();
    let base = if base.is_empty() { "folder".to_string() } else { base.trim_end_matches('-').to_string() };
    std::iter::once(base.clone()).chain((2..).map(|i| format!("{base}-{i}"))).find(|s| !taken.contains(s)).unwrap_or(base)
}

/// Whether a kept slug is one `fresh_slug` could have made: a settings file
/// edited by hand is not to put a dot or a path in a `-c` key.
fn slug_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && !s.starts_with('-')
        && !s.ends_with('-')
        && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Every folder keeps the name it goes by now, so that none passes to another
/// as an earlier one goes.
pub(super) fn settle_slugs(lib: &mut LibrarySettings) {
    let slugs: Vec<String> = folder_slugs(&lib.folders).into_iter().map(|(_, s)| s).collect();
    for (f, slug) in lib.folders.iter_mut().zip(slugs) {
        f.slug = slug;
    }
}

/// The names folders' skills wait under on disk: `library-folders` and each
/// Codex account's marketplace. A folder gone may have left copies changed
/// since there, which stay: a new folder is not to take them up as its own.
pub(super) fn slugs_on_disk(dirs: &Dirs) -> BTreeSet<String> {
    let names = |dir: PathBuf| -> Vec<String> {
        std::fs::read_dir(dir).map(|e| e.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default()
    };
    let mut out: BTreeSet<String> = names(dirs.folders()).into_iter().collect();
    for a in crate::accounts::list_accounts().unwrap_or_default().iter().filter(|a| a.provider == Provider::Codex) {
        out.extend(names(codex_market_dir(a)));
    }
    out
}

/// Where a folder's skills for one Claude Code account wait: the folder a
/// session there is given with `--add-dir`, its `.claude/skills` listed as the
/// session's own.
pub(super) fn claude_folder_dir(dirs: &Dirs, slug: &str, a: &AccountInfo) -> PathBuf {
    dirs.folders().join(slug).join(&a.name)
}

/// Luna's marketplace folder in a Codex account's plugin cache.
fn codex_market_dir(a: &AccountInfo) -> PathBuf {
    Path::new(&a.path).join("plugins").join("cache").join(CODEX_MARKET)
}

/// A folder's plugin in a Codex account: its version folder, `local` — the
/// one Codex takes over any version — holding the manifest and `skills/`.
pub(super) fn codex_plugin_dir(a: &AccountInfo, slug: &str) -> PathBuf {
    codex_market_dir(a).join(slug).join("local")
}

/// The names of the skills a Claude Code session in `folder` on this account
/// sees besides the library's: the `.claude/skills` of the folder and of each
/// folder up to the repository's root (where Claude Code stops looking), the
/// account's `skills/` but for Luna's own copies there — the skill itself,
/// which a folder then gives to no session of the account, or one on its way
/// out — and the skills it syncs from claude.ai there.
fn claude_names_taken(folder: &str, a: &AccountInfo, ledger: &Ledger) -> BTreeSet<String> {
    let names = |dir: PathBuf| -> Vec<String> {
        std::fs::read_dir(dir).map(|e| e.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default()
    };
    let mut out = BTreeSet::new();
    for dir in Path::new(folder).ancestors() {
        out.extend(names(dir.join(".claude").join("skills")));
        if dir.join(".git").exists() {
            break;
        }
    }
    let skills = Path::new(&a.path).join("skills");
    for synced in names(skills.join("synced")) {
        out.extend(names(skills.join("synced").join(synced)));
    }
    let lunas: BTreeSet<String> = ledger.written.keys().map(|k| ident(Path::new(k))).collect();
    out.extend(names(skills.clone()).into_iter().filter(|n| !lunas.contains(&ident(&skills.join(n)))));
    out
}

/// The names a folder's Claude Code copies on this account go by where a
/// skill a session there sees has theirs (`free_names`); `going` is every
/// skill the folder gives the account, as each takes a name from the rest.
pub(super) fn claude_shown_names(f: &LibraryFolder, a: &AccountInfo, ledger: &Ledger, going: &[&str]) -> BTreeMap<String, String> {
    free_names(going, &claude_names_taken(&f.path, a, ledger))
}

/// Whether the account has the skill as a whole, in its own `skills/`.
pub(super) fn account_copy_for(lib: &LibrarySettings, name: &str, a: &AccountInfo) -> bool {
    lib.skills.get(name).is_some_and(|reach| reaches(reach, a))
}

/// Whether a folder gives its sessions on the account a copy of the skill of
/// their own: where it gives the skill to the account, unless the account
/// has it as a whole already — twice over, Claude Code would list the second
/// copy scoped to Luna's folder — or a folder this one lies in gives it a
/// copy: every session here lies in that one too, and would list both (Codex
/// as `<outer>:<name>` and `<inner>:<name>`).
pub(super) fn folder_copy_for(lib: &LibrarySettings, f: &LibraryFolder, name: &str, a: &AccountInfo) -> bool {
    let gives = |g: &LibraryFolder| g.skills.get(name).is_some_and(|reach| reaches(reach, a));
    let outer =
        |g: &&LibraryFolder| ident(Path::new(&g.path)) != ident(Path::new(&f.path)) && within(Path::new(&f.path), Path::new(&g.path));
    gives(f) && !account_copy_for(lib, name, a) && !lib.folders.iter().filter(outer).any(gives)
}

/// What the folders a session's work lies in say of a skill: the accounts
/// whose sessions get it there, or nothing (None) — and then it goes by what
/// the skill is meant for. The deepest of them that names it decides, so one
/// inside another can take back what the outer one says.
fn deciding<'a>(lies_in: impl IntoIterator<Item = &'a LibraryFolder>, name: &str) -> Option<&'a Reach> {
    lies_in
        .into_iter()
        .filter(|f| f.skills.contains_key(name))
        .max_by_key(|f| ident(Path::new(&f.path)).len())
        .and_then(|f| f.skills.get(name))
}

/// What the folders a session's work lies in say of its account's
/// instructions: other instructions by name, none (`Some(None)`), or nothing
/// (None) — and then it has its account's. The deepest folder with an entry
/// that takes the account in decides, by its closest entry.
fn folder_instructions<'a>(lies_in: impl IntoIterator<Item = &'a LibraryFolder>, a: &AccountInfo) -> Option<Option<&'a str>> {
    lies_in
        .into_iter()
        .filter_map(|f| {
            let (_, choice) = f
                .instructions
                .iter()
                .filter_map(|(entry, choice)| Some((closeness(std::slice::from_ref(entry), a)?, choice)))
                .max_by_key(|(c, _)| *c)?;
            Some((ident(Path::new(&f.path)).len(), choice.as_deref()))
        })
        .max_by_key(|(depth, _)| *depth)
        .map(|(_, choice)| choice)
}

/// Whether the sessions on an account whose work lies in these folders get
/// the skill.
pub(super) fn gets_in<'a>(
    lib: &LibrarySettings,
    lies_in: impl IntoIterator<Item = &'a LibraryFolder>,
    name: &str,
    a: &AccountInfo,
) -> bool {
    match deciding(lies_in, name) {
        Some(reach) => reaches(reach, a),
        None => account_copy_for(lib, name, a),
    }
}

/// The name each of `ours` goes by where `taken` has its own: `luna-<name>`,
/// or with `-2`, `-3` after it where that is taken too — by another skill
/// there or one of ours. The names that stay as they are are not in it.
fn free_names(ours: &[&str], taken: &BTreeSet<String>) -> BTreeMap<String, String> {
    let mut used: BTreeSet<String> = ours.iter().map(|n| n.to_string()).chain(taken.iter().cloned()).collect();
    let mut out = BTreeMap::new();
    for name in ours.iter().filter(|n| taken.contains(**n)) {
        let base: String = format!("{RENAMED}{name}").chars().take(60).collect();
        let base = base.trim_end_matches('-').to_string();
        let Some(free) = std::iter::once(base.clone()).chain((2..100).map(|i| format!("{base}-{i}"))).find(|c| !used.contains(c)) else {
            continue;
        };
        used.insert(free.clone());
        out.insert(name.to_string(), free);
    }
    out
}

/// A skill's files with `name:` in its SKILL.md's front matter made `name` —
/// a folder's copy under another name, which both CLIs want the same as its
/// folder's. Nothing else in the file changes.
pub(super) fn with_name(mut files: Files, name: &str) -> Files {
    if let Some(bytes) = files.get_mut(SKILL_FILE) {
        let text = String::from_utf8_lossy(bytes).into_owned();
        let mut out = String::with_capacity(text.len() + name.len());
        let mut front = false;
        let mut done = false;
        for (i, line) in text.split_inclusive('\n').enumerate() {
            let bare = line.trim_end_matches(['\r', '\n']);
            if i == 0 {
                front = bare.trim_start_matches('\u{feff}').trim_end() == "---";
            } else if front && !done {
                if bare.trim_end() == "---" {
                    front = false;
                } else if !line.starts_with([' ', '\t']) && bare.split_once(':').is_some_and(|(k, _)| k.trim() == "name") {
                    out.push_str(&format!("name: {name}{}", &line[bare.len()..]));
                    done = true;
                    continue;
                }
            }
            out.push_str(line);
        }
        *bytes = out.into_bytes();
    }
    files
}

/// The manifest of each folder's plugin in each Codex account: there while
/// the plugin's `skills/` holds a copy — Codex loads a plugin by it — and
/// gone, with the folders that leaves empty, once none does. Luna's own file
/// in Luna's own marketplace folder: not a copy of anything, so neither noted
/// nor asked about.
pub(super) fn codex_manifests(lib: &LibrarySettings, accounts: &[AccountInfo]) {
    let slugs: BTreeMap<String, &str> = folder_slugs(&lib.folders).into_iter().map(|(f, s)| (s, f.path.as_str())).collect();
    for a in accounts.iter().filter(|a| a.provider == Provider::Codex) {
        let market = codex_market_dir(a);
        let mut names: BTreeSet<String> = slugs.keys().cloned().collect();
        if let Ok(entries) = std::fs::read_dir(&market) {
            names.extend(entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()));
        }
        for slug in names {
            let plugin = codex_plugin_dir(a, &slug);
            let manifest = plugin.join(".codex-plugin").join("plugin.json");
            // Luna's own folders, and none that leads elsewhere: through a
            // link on the way the manifest would be written, or taken out,
            // wherever that leads.
            let way = [market.clone(), market.join(&slug), plugin.clone(), plugin.join(".codex-plugin"), manifest.clone()];
            if let Some(link) = way.iter().find(|p| std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink())) {
                if link_is_news(link) {
                    crate::log::warn("library", &format!("{} is a link — Luna leaves the plugin there alone", link.display()));
                }
                continue;
            }
            let holds = std::fs::read_dir(plugin.join("skills")).is_ok_and(|mut e| e.next().is_some());
            if holds {
                // A folder no longer listed holds only copies changed since,
                // left in place; its manifest stays as it was.
                let Some(path) = slugs.get(&slug) else { continue };
                let manifest_json = serde_json::json!({
                    "name": slug,
                    "description": format!("Skills Luna's library gives sessions in {path}"),
                    "skills": "./skills/",
                });
                let text = serde_json::to_string_pretty(&manifest_json).unwrap_or_default() + "\n";
                if std::fs::read(&manifest).ok().as_deref() != Some(text.as_bytes()) {
                    if let Err(e) = write_copy(&manifest, text.as_bytes()) {
                        crate::log::warn("library", &format!("could not write {}: {e}", manifest.display()));
                    }
                }
            } else {
                let _ = std::fs::remove_file(&manifest);
                for dir in [plugin.join("skills"), plugin.join(".codex-plugin"), plugin.clone(), market.join(&slug), market.clone()] {
                    let _ = std::fs::remove_dir(dir);
                }
            }
        }
    }
}

/// What `library-folders` holds for folders no longer listed, once nothing
/// is left in it: copies changed since stay, and so do the folders around
/// them. A listed folder's are kept even empty, for the sessions given them.
pub(super) fn prune_folder_dirs(dirs: &Dirs, lib: &LibrarySettings) {
    let listed: BTreeSet<String> = folder_slugs(&lib.folders).into_iter().map(|(_, s)| s).collect();
    if let Ok(entries) = std::fs::read_dir(dirs.folders()) {
        for entry in entries.flatten().filter(|e| !listed.contains(&*e.file_name().to_string_lossy())) {
            prune_empty(&entry.path());
            let _ = std::fs::remove_dir(entry.path());
        }
    }
}

/// What a session Luna starts gets of the library's folders.
#[derive(Default, Debug, PartialEq)]
pub struct SessionSkills {
    /// For its CLI: Claude Code an `--add-dir` per folder, Codex a `-c` per
    /// folder's plugin and one for the skills its folders keep from it.
    pub args: Vec<String>,
    /// Claude Code: the skills it would list that its folders keep from it,
    /// by the names it lists them under — `skillOverrides` in the settings
    /// it is started with (pty/start.rs `claude_settings_file`), the one way to
    /// keep a skill from a session that its account has as a whole.
    pub hidden: Vec<String>,
    /// Claude Code: the account's instructions file, where a folder the
    /// session lies in gives it other instructions or none —
    /// `claudeMdExcludes` in the same settings. Codex has no way to leave its
    /// account's `AGENTS.md` out of one session, so a folder says nothing to
    /// it of instructions.
    pub excluded: Vec<String>,
    /// Claude Code: the library's file of the other instructions, which its
    /// caller gives the session as a `CLAUDE.md` of its own
    /// (pty/start.rs `claude_instructions_dir`).
    pub instructions: Option<PathBuf>,
}

/// What a session Luna starts gets of the library's folders, for each folder
/// its work lies in — the folder it starts in, or the project a worktree of
/// it belongs to: Claude Code an `--add-dir` of the skills waiting there for
/// this account, Codex a `-c` turning that folder's plugin on where the
/// account has one; and, from either, the skills those folders keep from it
/// (`deciding`) hidden. Due at every start, resumes and each helper turn
/// included: one started without them keeps the skills named in its
/// conversation, and none of them answers. Its caller lays the library out
/// first (`lay_out`), so the session reads its copies as the library has
/// them now — off any lock of the caller's, as a pass reads and writes every
/// account's copies.
pub fn session_args(provider: Provider, account_path: &str, folder: &str) -> SessionSkills {
    // A library a test holds, for the accounts it made (testkit
    // `GoldenLibrary`), in place of Luna's own settings and accounts.
    #[cfg(test)]
    if let Some((dirs, lib, a)) = held(provider, account_path) {
        return folder_args(&dirs, &lib, &a, folder);
    }
    let lib = crate::settings::get().library;
    if lib.folders.is_empty() {
        return SessionSkills::default();
    }
    let account = crate::accounts::list_accounts()
        .unwrap_or_default()
        .into_iter()
        .find(|a| a.provider == provider && crate::pty::same_dir(&a.path, account_path));
    match account {
        Some(a) => folder_args(&Dirs::current(), &lib, &a, folder),
        None => SessionSkills::default(),
    }
}

/// The library's folders a session's work lies in: those the folder it starts
/// in lies in — in a worktree, the same folder of the project it belongs to
/// (`<worktree>/src` is `<project>/src`), wherever the worktree itself lies.
fn folders_of<'a>(lib: &'a LibrarySettings, folder: &str) -> Vec<(&'a LibraryFolder, String)> {
    let work = crate::worktree::root_of(folder)
        .and_then(|(root, project)| Path::new(folder).strip_prefix(&root).ok().map(|rel| Path::new(&project).join(rel)))
        .unwrap_or_else(|| PathBuf::from(folder));
    folder_slugs(&lib.folders).into_iter().filter(|(f, _)| within(&work, Path::new(&f.path))).collect()
}

fn folder_args(dirs: &Dirs, lib: &LibrarySettings, a: &AccountInfo, folder: &str) -> SessionSkills {
    let lies_in = folders_of(lib, folder);
    let mut out = SessionSkills::default();
    for (f, slug) in &lies_in {
        match a.provider {
            Provider::Claude => {
                // Made if it is not there yet, skills or none: a session
                // started without its `.claude/skills` never sees one added
                // to it later.
                let dir = claude_folder_dir(dirs, slug, a);
                match std::fs::create_dir_all(dir.join(".claude").join("skills")) {
                    Ok(()) => out.args.extend(["--add-dir".to_string(), dir.to_string_lossy().into_owned()]),
                    Err(e) => crate::log::warn("library", &format!("could not make {} for {}: {e}", dir.display(), f.path)),
                }
            }
            Provider::Codex => {
                if codex_plugin_dir(a, slug).join(".codex-plugin").join("plugin.json").is_file() {
                    out.args.extend(["-c".to_string(), format!("plugins.{slug}@{CODEX_MARKET}.enabled=true")]);
                }
            }
        }
    }
    if a.provider == Provider::Claude {
        if let Some(choice) = folder_instructions(lies_in.iter().map(|(f, _)| *f), a) {
            let own = Path::new(&a.path).join(instructions_name(a.provider));
            // Instructions the library no longer has are none.
            let text = choice.and_then(|n| std::fs::read(dirs.instructions_file(n)).ok());
            // What the account's file holds already stays; anything else —
            // none at all, or a copy of the account's changed there — leaves
            // out whatever it holds, the library's or not.
            let there = text.as_ref().is_some_and(|t| std::fs::read(&own).is_ok_and(|o| o == *t));
            if !there {
                out.excluded.push(own.to_string_lossy().replace('\\', "/"));
                out.instructions = choice.filter(|_| text.as_ref().is_some_and(|t| !t.is_empty())).map(|n| dirs.instructions_file(n));
            }
        }
    }
    let hidden = hidden_here(dirs, lib, a, &lies_in);
    match a.provider {
        Provider::Claude => out.hidden = hidden,
        // One `-c` for them all: a second on the key would take the first's
        // place. Codex adds its entries to the account's own `skills.config`
        // ones, selector by selector, rather than putting them in their place.
        Provider::Codex if !hidden.is_empty() => {
            let entries: Vec<String> = hidden.iter().map(|n| format!("{{name=\"{n}\",enabled=false}}")).collect();
            out.args.extend(["-c".to_string(), format!("skills.config=[{}]", entries.join(","))]);
        }
        Provider::Codex => {}
    }
    out
}

/// The skills a session on this account would see that the folders it lies
/// in keep from it, by the names its CLI lists them under: the account's own
/// copy by the skill's name; a copy an outer folder gives, where an inner one
/// keeps it from this account, by its Claude Code name (`luna-<name>` where
/// it was renamed) or as Codex's `<folder>:<name>`. Both CLIs hide a skill by
/// name, so one of the same name the repository has goes too.
fn hidden_here(dirs: &Dirs, lib: &LibrarySettings, a: &AccountInfo, lies_in: &[(&LibraryFolder, String)]) -> Vec<String> {
    let named: BTreeSet<&String> = lib.skills.keys().chain(lies_in.iter().flat_map(|(f, _)| f.skills.keys())).collect();
    let mut out = BTreeSet::new();
    // Read only for a Claude Code copy an outer folder gives, whose name the
    // layout may have changed.
    let mut ledger = None;
    for name in named {
        if gets_in(lib, lies_in.iter().map(|(f, _)| *f), name, a) {
            continue;
        }
        if account_copy_for(lib, name, a) {
            out.insert(name.clone());
        }
        for (f, slug) in lies_in.iter().filter(|(f, _)| folder_copy_for(lib, f, name, a)) {
            match a.provider {
                Provider::Claude => {
                    let ledger = ledger.get_or_insert_with(|| Ledger::load(dirs));
                    // As `units` takes them: the skills the library has.
                    let going: Vec<&str> = f
                        .skills
                        .keys()
                        .filter(|n| folder_copy_for(lib, f, n, a) && dirs.skills().join(n).is_dir())
                        .map(String::as_str)
                        .collect();
                    let shown = claude_shown_names(f, a, ledger, &going);
                    out.insert(shown.get(name.as_str()).cloned().unwrap_or_else(|| name.clone()));
                }
                Provider::Codex => {
                    out.insert(format!("{slug}:{name}"));
                }
            }
        }
    }
    out.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_that_is_gone_is_not_made_again() {
        let f = Fixture::new("gone-folder");
        f.skill("review", "v1");
        let folder = f.base.join("deleted-repo/skills");
        let lib = LibrarySettings {
            folders: vec![LibraryFolder { path: folder.to_string_lossy().into_owned(), skills: given(&["review"]), ..Default::default() }],
            ..Default::default()
        };
        let p = f.run(&lib, &mut Ledger::default(), true);
        assert!(p.places.iter().all(|p| p.status == Status::Failed));
        assert!(!f.base.join("deleted-repo").exists());
        assert!(!f.dirs.folders().exists(), "nothing waits for a folder that is not there");
    }

    /// These skills, each to every account's sessions in a folder.
    fn given(names: &[&str]) -> BTreeMap<String, Reach> {
        skills(names)
    }

    /// A repository at `<base>/<name>`, and a folder of the library on it
    /// giving these skills to every account's sessions there.
    fn repo_folder(f: &Fixture, name: &str, skills: &[&str]) -> (PathBuf, LibraryFolder) {
        let repo = f.base.join(name);
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        (repo.clone(), LibraryFolder { path: repo.to_string_lossy().into_owned(), skills: given(skills), ..Default::default() })
    }

    #[test]
    fn a_folders_skills_wait_for_its_sessions_and_never_go_into_it() {
        let f = Fixture::new("folder-or");
        f.skill("review", "v1");
        let (repo, folder) = repo_folder(&f, "asuri-app", &["review"]);
        // Meant for claude/work as a whole, and for every account in the folder.
        let lib = LibrarySettings {
            skills: BTreeMap::from([("review".into(), reach(&["claude/work"]))]),
            folders: vec![folder],
            ..Default::default()
        };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        codex_manifests(&lib, &f.accounts);
        assert!(f.account(0).join("skills/review/SKILL.md").is_file());
        assert!(!f.dirs.folders().join("asuri-app/work").exists(), "it has it as a whole already");
        assert!(f.dirs.folders().join("asuri-app/home/.claude/skills/review/SKILL.md").is_file());
        let plugin = f.account(1).join("plugins/cache/luna/asuri-app/local");
        assert!(plugin.join("skills/review/SKILL.md").is_file());
        let manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(plugin.join(".codex-plugin/plugin.json")).unwrap()).unwrap();
        assert_eq!(manifest["name"], "asuri-app");
        assert_eq!(manifest["skills"], "./skills/");
        assert!(!repo.join("review").exists() && !repo.join(".claude").exists(), "nothing goes into the folder");

        // Its accounts only in the folder: none as a whole.
        let mut lib = lib;
        lib.skills.clear();
        lib.folders[0].skills = BTreeMap::from([("review".into(), reach(&["claude/work"]))]);
        let p = f.run(&lib, &mut ledger, true);
        codex_manifests(&lib, &f.accounts);
        assert!(p.kept.is_empty());
        assert!(!f.account(0).join("skills/review").exists());
        assert!(f.dirs.folders().join("asuri-app/work/.claude/skills/review/SKILL.md").is_file());
        assert!(!f.dirs.folders().join("asuri-app/home/.claude/skills/review").exists());
        assert!(!f.account(1).join("plugins/cache/luna").exists(), "a plugin with nothing in it goes, manifest and all");

        // A folder no longer listed: what waited for it goes.
        let lib = LibrarySettings { folders: Vec::new(), ..lib };
        f.run(&lib, &mut ledger, true);
        prune_folder_dirs(&f.dirs, &lib);
        assert!(!f.dirs.folders().join("asuri-app").exists());
    }

    #[test]
    fn copies_once_laid_into_a_folder_go_as_ever() {
        let f = Fixture::new("folder-old");
        f.skill("review", "v1");
        let (repo, folder) = repo_folder(&f, "repo", &["review"]);
        // Where a folder's skill went before: `<folder>/<name>`, noted as Luna's.
        let old = repo.join("review");
        let files = read_dir_files(&f.dirs.skills().join("review")).unwrap();
        write_at(&old, false, &files).unwrap();
        let mut ledger = Ledger::default();
        ledger.written.insert(old.to_string_lossy().into_owned(), hashes(&files));
        f.run(&LibrarySettings { folders: vec![folder], ..Default::default() }, &mut ledger, true);
        assert!(!old.exists());
        assert!(f.dirs.folders().join("repo/work/.claude/skills/review/SKILL.md").is_file());
    }

    #[test]
    fn a_claude_copy_whose_name_is_taken_there_goes_by_another() {
        let f = Fixture::new("folder-rename");
        f.skill("review", "v1");
        f.skill("notes", "v1");
        f.skill("luna-review", "ours too");
        let (repo, folder) = repo_folder(&f, "repo", &["review", "notes", "luna-review"]);
        // The repository has a review of its own; claude/home's account a notes.
        write_file(&repo.join(".claude/skills/review/SKILL.md"), b"---\nname: review\ndescription: The repo's.\n---\n").unwrap();
        write_file(&f.account(2).join("skills/notes/SKILL.md"), b"---\nname: notes\ndescription: The account's.\n---\n").unwrap();
        let lib = LibrarySettings { folders: vec![folder], ..Default::default() };
        let mut ledger = Ledger::default();
        let p = f.run(&lib, &mut ledger, true);
        let work = f.dirs.folders().join("repo/work/.claude/skills");
        let home = f.dirs.folders().join("repo/home/.claude/skills");
        let renamed = std::fs::read_to_string(work.join("luna-review-2/SKILL.md")).unwrap();
        assert!(renamed.starts_with("---\nname: luna-review-2\ndescription: Does review.\n"), "{renamed}");
        assert!(check_skill("luna-review-2", &renamed).is_ok());
        assert!(work.join("luna-review/SKILL.md").is_file(), "ours keeps the name it has");
        assert!(work.join("notes/SKILL.md").is_file() && !work.join("luna-notes").exists());
        assert!(home.join("luna-notes/SKILL.md").is_file());
        assert!(f.account(1).join("plugins/cache/luna/repo/local/skills/review/SKILL.md").is_file(), "Codex prefixes them all");
        let shown = p.places.iter().find(|x| Path::new(&x.dest) == work.join("luna-review-2")).unwrap();
        assert_eq!(shown.skill.as_deref(), Some("review"));
        assert_eq!(shown.shown_as.as_deref(), Some("luna-review-2"));

        // Compared with the copy as it is to be: nothing to do next time.
        let p = f.run(&lib, &mut ledger, true);
        assert_eq!(Fixture::status(&p, &work.join("luna-review-2")), Status::Current);
    }

    #[test]
    fn a_name_put_on_a_copy_comes_off_it() {
        let text = "---\r\nname: \"review\"\r\ndescription: >\r\n  name: not this one\r\n---\r\nname: nor this\r\n";
        let files = Files::from([(SKILL_FILE.to_string(), text.as_bytes().to_vec()), ("ref.md".to_string(), b"name: x".to_vec())]);
        let renamed = with_name(files.clone(), "luna-review");
        assert_eq!(String::from_utf8_lossy(&renamed[SKILL_FILE]), text.replace("name: \"review\"", "name: luna-review"));
        assert_eq!(renamed["ref.md"], files["ref.md"]);
        let back = with_name(renamed, "review");
        assert!(check_skill("review", &String::from_utf8_lossy(&back[SKILL_FILE])).is_ok());

        let taken = BTreeSet::from(["x".to_string(), "luna-y".to_string()]);
        let names = free_names(&["x", "luna-x", "y"], &taken);
        assert_eq!(names, BTreeMap::from([("x".to_string(), "luna-x-2".to_string())]));
    }

    #[test]
    fn a_folders_name_is_one_codex_takes_in_a_key() {
        let folders: Vec<LibraryFolder> = ["C:/a/Asuri App", "D:/b/asuri.app", "E:/c/asuri-app", "/"]
            .iter()
            .map(|p| LibraryFolder { path: p.to_string(), ..Default::default() })
            .collect();
        let slugs: Vec<String> = folder_slugs(&folders).into_iter().map(|(_, s)| s).collect();
        assert_eq!(slugs, ["asuri-app", "asuri-app-2", "asuri-app-3", "folder"]);
        assert!(slugs.iter().all(|s| check_name(s).is_ok()));
    }

    /// Kept, a folder's name stays its own as an earlier folder goes; one the
    /// file holds that no folder could have been given is not taken.
    #[test]
    fn a_folder_keeps_its_name_when_an_earlier_one_goes() {
        let mut lib = LibrarySettings {
            folders: ["E:/a/app", "E:/b/app", "E:/c/app"]
                .iter()
                .map(|p| LibraryFolder { path: p.to_string(), ..Default::default() })
                .collect(),
            ..Default::default()
        };
        settle_slugs(&mut lib);
        let slugs = |lib: &LibrarySettings| folder_slugs(&lib.folders).into_iter().map(|(_, s)| s).collect::<Vec<_>>();
        assert_eq!(slugs(&lib), ["app", "app-2", "app-3"], "as they always were, by their order");
        lib.folders.remove(0);
        assert_eq!(slugs(&lib), ["app-2", "app-3"]);
        // A new one takes none listed and none on disk.
        let taken: BTreeSet<String> = lib.folders.iter().map(|f| f.slug.clone()).chain(["app".to_string()]).collect();
        assert_eq!(fresh_slug("E:/d/app", &taken), "app-4");
        lib.folders[0].slug = "../x".into();
        lib.folders[1].slug = "app-2".into();
        assert_eq!(slugs(&lib), ["app", "app-2"], "a bad one is made anew, past the one kept");
    }

    /// A link on the way to a plugin's manifest: neither written through nor
    /// taken out through.
    #[cfg(unix)]
    #[test]
    fn a_manifest_behind_a_link_is_left_alone() {
        let f = Fixture::new("manifest-link");
        f.skill("review", "v1");
        let (_, folder) = repo_folder(&f, "repo", &["review"]);
        let lib = LibrarySettings { folders: vec![folder], ..Default::default() };
        let elsewhere = f.base.join("elsewhere");
        std::fs::create_dir_all(elsewhere.join("skills").join("x")).unwrap();
        std::fs::write(elsewhere.join("skills").join("x").join("SKILL.md"), "x").unwrap();
        let plugin = codex_plugin_dir(&f.accounts[1], "repo");
        std::fs::create_dir_all(plugin.join("skills").join("review")).unwrap();
        std::fs::write(plugin.join("skills").join("review").join("SKILL.md"), "v1").unwrap();
        std::os::unix::fs::symlink(&elsewhere, plugin.join(".codex-plugin")).unwrap();
        codex_manifests(&lib, &f.accounts);
        assert!(!elsewhere.join("plugin.json").exists(), "not written through the link");
        std::fs::remove_dir_all(plugin.join("skills")).unwrap();
        std::fs::write(elsewhere.join("plugin.json"), "theirs").unwrap();
        codex_manifests(&lib, &f.accounts);
        assert_eq!(std::fs::read_to_string(elsewhere.join("plugin.json")).unwrap(), "theirs", "nor taken out through it");
    }

    #[test]
    fn a_session_in_a_folder_or_a_worktree_of_it_is_given_its_skills() {
        let f = Fixture::new("folder-args");
        f.skill("review", "v1");
        let (repo, folder) = repo_folder(&f, "repo", &["review"]);
        let lib = LibrarySettings { folders: vec![folder], ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        codex_manifests(&lib, &f.accounts);
        let repo_s = repo.to_string_lossy().into_owned();
        let wait = f.dirs.folders().join("repo").join("work");

        let args = folder_args(&f.dirs, &lib, &f.accounts[0], &repo_s).args;
        assert_eq!(args, ["--add-dir".to_string(), wait.to_string_lossy().into_owned()]);
        let sub = repo.join("src").to_string_lossy().into_owned();
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[0], &sub).args, args, "a folder inside it is in it");
        let claude_wt = repo.join(".claude/worktrees/x").to_string_lossy().into_owned();
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[0], &claude_wt).args, args);

        // Made for a session even with nothing in it yet: one added later
        // reaches only a session that had its `.claude/skills` from the start.
        let empty = LibrarySettings { folders: vec![LibraryFolder { path: repo_s.clone(), ..Default::default() }], ..Default::default() };
        let home = folder_args(&f.dirs, &empty, &f.accounts[2], &repo_s).args;
        assert_eq!(home[1], f.dirs.folders().join("repo").join("home").to_string_lossy());
        assert!(f.dirs.folders().join("repo/home/.claude/skills").is_dir());

        // Codex, in a worktree it made of the repository: known by its `.git` file.
        let wt = f.account(1).join("worktrees/0a1b/repo");
        write_file(&wt.join(".git"), format!("gitdir: {}\n", repo.join(".git").join("worktrees").join("repo").display()).as_bytes())
            .unwrap();
        let codex = ["-c".to_string(), "plugins.repo@luna.enabled=true".to_string()];
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[1], &repo_s).args, codex);
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[1], &wt.to_string_lossy()).args, codex);
        f.run(&empty, &mut ledger, true);
        codex_manifests(&empty, &f.accounts);
        assert!(folder_args(&f.dirs, &empty, &f.accounts[1], &repo_s).args.is_empty(), "no plugin, nothing to turn on");

        let elsewhere = f.base.join("elsewhere").to_string_lossy().into_owned();
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[0], &elsewhere), SessionSkills::default());
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[1], &elsewhere), SessionSkills::default());
    }

    #[test]
    fn a_folder_keeps_a_skill_from_its_sessions_and_one_inside_it_takes_it_back() {
        let f = Fixture::new("folder-off");
        f.skill("review", "v1");
        f.skill("notes", "v1");
        // `work` gives notes and keeps review; `work/repo` inside it keeps notes.
        let outer = f.base.join("work");
        std::fs::create_dir_all(&outer).unwrap();
        let outer_folder = LibraryFolder {
            path: outer.to_string_lossy().into_owned(),
            skills: BTreeMap::from([("notes".into(), all()), ("review".into(), Vec::new())]),
            ..Default::default()
        };
        let (repo, mut inner) = repo_folder(&f, "work/repo", &[]);
        inner.skills.insert("notes".into(), Vec::new());
        let lib = LibrarySettings {
            skills: BTreeMap::from([("review".into(), all()), ("notes".into(), reach(&["claude/work"]))]),
            folders: vec![outer_folder, inner],
            ..Default::default()
        };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        codex_manifests(&lib, &f.accounts);
        assert!(f.account(0).join("skills/review/SKILL.md").is_file(), "the account's copy stays where it is");
        assert!(f.dirs.folders().join("work/home/.claude/skills/notes/SKILL.md").is_file());

        // In the outer folder: review hidden, its copy the account's own.
        let other = outer.join("other").to_string_lossy().into_owned();
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[0], &other).hidden, ["review"]);
        let codex = folder_args(&f.dirs, &lib, &f.accounts[1], &other).args;
        assert_eq!(codex, ["-c", "plugins.work@luna.enabled=true", "-c", "skills.config=[{name=\"review\",enabled=false}]"]);

        // In the repository: notes too, the outer folder's copy by the name
        // each CLI lists it under, or the account's own.
        let repo_s = repo.to_string_lossy().into_owned();
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[2], &repo_s).hidden, ["notes", "review"]);
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[0], &repo_s).hidden, ["notes", "review"]);
        let codex = folder_args(&f.dirs, &lib, &f.accounts[1], &repo_s).args;
        assert_eq!(
            codex.last().map(String::as_str),
            Some("skills.config=[{name=\"review\",enabled=false},{name=\"work:notes\",enabled=false}]")
        );

        // Ticked in the repository again, the closer folder decides.
        let mut lib = lib;
        lib.folders[1] = LibraryFolder { skills: given(&["review", "notes"]), ..lib.folders[1].clone() };
        f.run(&lib, &mut ledger, true);
        assert!(folder_args(&f.dirs, &lib, &f.accounts[2], &repo_s).hidden.is_empty());
        let elsewhere = f.base.join("elsewhere").to_string_lossy().into_owned();
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[0], &elsewhere), SessionSkills::default());
    }

    #[test]
    fn a_folder_says_what_its_sessions_get_of_a_skill_it_names_not() {
        let f = Fixture::new("folder-inherited");
        let outer = f.base.join("work").to_string_lossy().into_owned();
        let inner = f.base.join("work").join("repo").to_string_lossy().into_owned();
        let lib = LibrarySettings {
            skills: BTreeMap::from([("review".into(), all()), ("notes".into(), reach(&["claude/work"]))]),
            folders: vec![
                LibraryFolder { path: outer.clone(), skills: BTreeMap::from([("review".into(), reach(&["codex"]))]), ..Default::default() },
                LibraryFolder { path: inner.clone(), ..Default::default() },
            ],
            ..Default::default()
        };
        let (o, i) = (&lib.folders[0], &lib.folders[1]);
        assert_eq!(
            inherited(&lib, &f.accounts, o, "review"),
            ["claude/work", "codex/home", "claude/home"],
            "every account has it as a whole"
        );
        assert_eq!(inherited(&lib, &f.accounts, i, "review"), ["codex/home"], "the folder it lies in gives it to Codex alone");
        assert_eq!(inherited(&lib, &f.accounts, o, "notes"), ["claude/work"]);
        assert!(inherited(&lib, &f.accounts, o, "tidy").is_empty(), "meant for none, given nowhere");
    }

    /// One account everywhere, another only in one folder, and none of them
    /// in another: what a reach of the folder's own says, account by account.
    #[test]
    fn a_folder_gives_a_skill_to_some_accounts_and_keeps_it_from_others() {
        let f = Fixture::new("folder-reach");
        f.skill("review", "v1");
        let (app, mut app_folder) = repo_folder(&f, "app", &[]);
        let (web, mut web_folder) = repo_folder(&f, "web", &[]);
        // claude/work everywhere; claude/home and codex too, in app; in web,
        // Codex alone.
        app_folder.skills.insert("review".into(), reach(&["claude", "codex"]));
        web_folder.skills.insert("review".into(), reach(&["codex"]));
        let lib = LibrarySettings {
            skills: BTreeMap::from([("review".into(), reach(&["claude/work"]))]),
            folders: vec![app_folder, web_folder],
            ..Default::default()
        };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        codex_manifests(&lib, &f.accounts);
        assert!(f.account(0).join("skills/review/SKILL.md").is_file());
        assert!(!f.account(2).join("skills/review").exists());
        assert!(f.dirs.folders().join("app/home/.claude/skills/review/SKILL.md").is_file());
        assert!(!f.dirs.folders().join("app/work/.claude/skills/review").exists(), "work has it as a whole");
        assert!(!f.dirs.folders().join("web/home/.claude/skills/review").exists());
        assert!(f.account(1).join("plugins/cache/luna/app/local/skills/review/SKILL.md").is_file());
        assert!(f.account(1).join("plugins/cache/luna/web/local/skills/review/SKILL.md").is_file());

        let (app_s, web_s) = (app.to_string_lossy().into_owned(), web.to_string_lossy().into_owned());
        assert!(folder_args(&f.dirs, &lib, &f.accounts[0], &app_s).hidden.is_empty());
        assert!(folder_args(&f.dirs, &lib, &f.accounts[2], &app_s).hidden.is_empty());
        // In web the account that has it as a whole does not get it; Codex does.
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[0], &web_s).hidden, ["review"]);
        assert!(folder_args(&f.dirs, &lib, &f.accounts[2], &web_s).hidden.is_empty(), "nothing of it to hide");
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[1], &web_s).args, ["-c", "plugins.web@luna.enabled=true"]);
    }

    #[test]
    fn a_folder_gives_its_claude_sessions_other_instructions_or_none() {
        let f = Fixture::new("folder-instructions");
        f.instructions("main", "# Main\n");
        f.instructions("terse", "# Terse\n");
        f.instructions("blank", "");
        let (repo, mut folder) = repo_folder(&f, "repo", &[]);
        // work: none; every other Claude Code account: terse; Codex: none,
        // which it has no way to take.
        folder.instructions =
            BTreeMap::from([("claude/work".into(), None), ("claude".into(), Some("terse".into())), ("codex/home".into(), None)]);
        let mut lib = LibrarySettings { instructions: first(all()), folders: vec![folder], ..Default::default() };
        let repo_s = repo.join("src").to_string_lossy().into_owned();
        let claude_md = |i: usize| format!("{}/CLAUDE.md", f.account(i).to_string_lossy().replace('\\', "/"));
        let appended = |s: &SessionSkills| s.instructions.clone();

        let work = folder_args(&f.dirs, &lib, &f.accounts[0], &repo_s);
        assert_eq!(work.excluded, [claude_md(0)]);
        assert_eq!(appended(&work), None);
        let home = folder_args(&f.dirs, &lib, &f.accounts[2], &repo_s);
        assert_eq!(home.excluded, [claude_md(2)]);
        assert_eq!(appended(&home), Some(f.dirs.instructions_file("terse")));
        let codex = folder_args(&f.dirs, &lib, &f.accounts[1], &repo_s);
        assert!(codex.excluded.is_empty() && codex.args.is_empty());
        let elsewhere = f.base.join("elsewhere").to_string_lossy().into_owned();
        assert!(folder_args(&f.dirs, &lib, &f.accounts[0], &elsewhere).excluded.is_empty());

        // What the account's file holds already: nothing to change — but a
        // copy changed there is no longer what was picked. Blank: none. Gone
        // from the library: none.
        lib.folders[0].instructions =
            BTreeMap::from([("claude/work".into(), Some("main".into())), ("claude/home".into(), Some("blank".into()))]);
        f.run(&lib, &mut Ledger::default(), true);
        assert_eq!(f.read(0, "CLAUDE.md"), "# Main\n");
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[0], &repo_s), SessionSkills { args: work.args.clone(), ..Default::default() });
        std::fs::write(f.account(0).join("CLAUDE.md"), "# Main, changed here\n").unwrap();
        let changed = folder_args(&f.dirs, &lib, &f.accounts[0], &repo_s);
        assert_eq!((changed.excluded.clone(), appended(&changed)), (vec![claude_md(0)], Some(f.dirs.instructions_file("main"))));
        let blank = folder_args(&f.dirs, &lib, &f.accounts[2], &repo_s);
        assert_eq!((blank.excluded.len(), appended(&blank)), (1, None));
        lib.folders[0].instructions = BTreeMap::from([("claude".into(), Some("gone".into()))]);
        let gone = folder_args(&f.dirs, &lib, &f.accounts[2], &repo_s);
        assert_eq!((gone.excluded.len(), appended(&gone)), (1, None));

        // A folder inside it decides for the accounts it names; the outer one
        // for the rest.
        let inner = LibraryFolder {
            path: repo.join("src").to_string_lossy().into_owned(),
            instructions: BTreeMap::from([("claude/home".into(), Some("main".into()))]),
            ..Default::default()
        };
        lib.folders[0].instructions = BTreeMap::from([("claude".into(), None)]);
        lib.folders.push(inner);
        std::fs::write(f.account(2).join("CLAUDE.md"), "# Main\n").unwrap();
        assert!(folder_args(&f.dirs, &lib, &f.accounts[2], &repo_s).excluded.is_empty());
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[0], &repo_s).excluded, [claude_md(0)]);

        // Renamed and deleted accounts take their entries along.
        rename_account(&mut lib, "claude/home", Some("claude/personal"));
        assert_eq!(lib.folders[1].instructions.keys().collect::<Vec<_>>(), ["claude/personal"]);
        rename_account(&mut lib, "claude/personal", None);
        assert!(lib.folders[1].instructions.is_empty());
    }

    /// A folder inside a repository holds in that folder of each of its
    /// worktrees, wherever the worktree lies.
    #[test]
    fn a_folder_inside_a_repository_holds_in_its_worktrees_too() {
        let f = Fixture::new("folder-in-worktree");
        f.skill("review", "v1");
        let (repo, _) = repo_folder(&f, "repo", &[]);
        let src = LibraryFolder {
            path: repo.join("src").to_string_lossy().into_owned(),
            skills: BTreeMap::from([("review".into(), Vec::new())]),
            ..Default::default()
        };
        let lib = LibrarySettings { skills: skills(&["review"]), folders: vec![src], ..Default::default() };
        f.run(&lib, &mut Ledger::default(), true);
        // A Codex worktree of it, in the account's own folder.
        let wt = f.account(1).join("worktrees/0a1b/repo");
        write_file(&wt.join(".git"), format!("gitdir: {}\n", repo.join(".git").join("worktrees").join("repo").display()).as_bytes())
            .unwrap();
        std::fs::create_dir_all(wt.join("src")).unwrap();
        let hide = ["-c".to_string(), "skills.config=[{name=\"review\",enabled=false}]".to_string()];
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[1], &wt.join("src").to_string_lossy()).args, hide);
        assert!(
            folder_args(&f.dirs, &lib, &f.accounts[1], &wt.to_string_lossy()).args.is_empty(),
            "the worktree's root is the repository's"
        );
    }

    /// Where an outer folder and one inside it both give an account a skill,
    /// the outer one's copy serves the sessions of both.
    #[test]
    fn folders_one_inside_another_give_an_account_one_copy() {
        let f = Fixture::new("folder-nested-copy");
        f.skill("review", "v1");
        let (repo, outer) = repo_folder(&f, "repo", &["review"]);
        std::fs::create_dir_all(repo.join("web")).unwrap();
        let inner =
            LibraryFolder { path: repo.join("web").to_string_lossy().into_owned(), skills: given(&["review"]), ..Default::default() };
        let mut lib = LibrarySettings { folders: vec![outer, inner], ..Default::default() };
        f.run(&lib, &mut Ledger::default(), true);
        codex_manifests(&lib, &f.accounts);
        assert!(f.dirs.folders().join("repo/work/.claude/skills/review/SKILL.md").is_file());
        assert!(!f.dirs.folders().join("web/work/.claude/skills/review").exists());
        assert!(!f.account(1).join("plugins/cache/luna/web").exists());
        let web = repo.join("web").to_string_lossy().into_owned();
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[1], &web).args, ["-c", "plugins.repo@luna.enabled=true"]);

        // Kept from the account outside, given again inside: the inside's own.
        lib.folders[0].skills.insert("review".into(), reach(&["codex"]));
        f.run(&lib, &mut Ledger::default(), true);
        assert!(f.dirs.folders().join("web/work/.claude/skills/review/SKILL.md").is_file());
        assert!(folder_args(&f.dirs, &lib, &f.accounts[0], &web).hidden.is_empty());
    }
}
