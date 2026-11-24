//! The folders the user names: the name each one's skills wait under, the Codex
//! plugins they wait in (Claude Code's are each session's own,
//! session_plugin.rs), and what a session started in one, or in a worktree of
//! it, is given and has hidden.

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

/// The names folders' skills wait under on disk: each Codex account's
/// marketplace. A folder gone may have left copies changed since there, which
/// stay: a new folder is not to take them up as its own.
pub(super) fn slugs_on_disk() -> BTreeSet<String> {
    let names = |dir: PathBuf| -> Vec<String> {
        std::fs::read_dir(dir).map(|e| e.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default()
    };
    let mut out = BTreeSet::new();
    for a in crate::accounts::list_accounts().unwrap_or_default().iter().filter(|a| a.provider == Provider::Codex) {
        out.extend(names(codex_market_dir(a)));
    }
    out
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

/// Whether the account has the skill as a whole, in its own `skills/`.
pub(super) fn account_copy_for(lib: &LibrarySettings, name: &str, a: &AccountInfo) -> bool {
    lib.skills.get(name).is_some_and(|reach| reaches(reach, a))
}

/// Whether a folder gives its sessions on the account a copy of the skill of
/// their own: where it gives the skill to the account, unless the account
/// has it as a whole already — twice over, the session would list it under
/// its own name and again in the folder's plugin — or a folder this one lies
/// in gives it a copy: every session here lies in that one too, and would
/// list both (as `<outer>:<name>` and `<inner>:<name>`).
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

/// The manifest of each folder's plugin in each Codex account: there while
/// the plugin's `skills/` holds a copy — Codex loads a plugin by it — and
/// gone, with the folders that leaves empty, once none does. Luna's own file
/// in Luna's own marketplace folder, not a copy of anything: the ledger
/// notes each one Luna wrote (`Ledger::manifests`), and only those are taken
/// out again, so a plugin someone else put in that folder keeps its own.
pub(super) fn codex_manifests(lib: &LibrarySettings, accounts: &[AccountInfo], ledger: &mut Ledger) {
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
            // link on the way down from the account's folder (`plugins`,
            // `plugins/cache` too) the manifest would be written, or taken
            // out, wherever that leads.
            let linked = no_link_between(Path::new(&a.path), &manifest).err().or_else(|| {
                std::fs::symlink_metadata(&manifest)
                    .is_ok_and(|m| crate::boundary::is_link(&m))
                    .then(|| format!("{} is a link", manifest.display()))
            });
            if let Some(why) = linked {
                if link_is_news(&manifest) {
                    crate::log::warn("library", &format!("Luna leaves the plugin at {} alone: {why}", plugin.display()));
                }
                continue;
            }
            let id = ident(&manifest);
            let noted = ledger.manifests.iter().find(|m| ident(Path::new(m)) == id).cloned();
            let holds = std::fs::read_dir(plugin.join("skills")).is_ok_and(|mut e| e.next().is_some());
            if holds {
                // A folder no longer listed holds only copies left in place;
                // its manifest stays as it was.
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
                        continue;
                    }
                }
                if noted.is_none() {
                    ledger.manifests.insert(manifest.to_string_lossy().into_owned());
                }
            } else {
                // Not Luna's: someone else's plugin, left as it is.
                let Some(noted) = noted else { continue };
                if std::fs::remove_file(&manifest).is_ok() || !manifest.exists() {
                    ledger.manifests.remove(&noted);
                }
                for dir in [plugin.join("skills"), plugin.join(".codex-plugin"), plugin.clone(), market.join(&slug), market.clone()] {
                    let _ = std::fs::remove_dir(dir);
                }
            }
        }
    }
}

/// The folders an earlier Luna made in `library-folders` for Claude Code's
/// sessions in a folder, `<slug>/<account>/.claude/skills`, once nothing is
/// left in them, and each folder above that this leaves empty, up to
/// `library-folders` itself: no session reads them now (its folder skills
/// are a plugin of its own, session_plugin.rs), and a pass takes out the
/// copies Luna wrote there, each skill's folder with them (pass.rs `pass`).
/// Only that layout: any other folder there, empty or not (an accounts
/// folder put there, say), stays, and so do the folders around it. Listed
/// folders' are no longer kept for their sessions: none is given them. No
/// link is gone through or taken out: at `library-folders` it leads out of
/// Luna's data folder.
pub(super) fn prune_folder_dirs(dirs: &Dirs) {
    let top = dirs.folders();
    let plain = |p: &Path| std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir() && !crate::boundary::is_link(&m));
    let dirs_in = |p: &Path| -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(p) else { return Vec::new() };
        entries.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.path()).collect()
    };
    if !plain(&top) {
        return;
    }
    let mut any = false;
    for slug in dirs_in(&top) {
        let mut emptied = false;
        for holder in dirs_in(&slug) {
            let claude = holder.join(".claude");
            let skills = claude.join("skills");
            emptied |= plain(&claude) && plain(&skills) && [&skills, &claude, &holder].into_iter().all(|d| std::fs::remove_dir(d).is_ok());
        }
        any |= emptied && std::fs::remove_dir(&slug).is_ok();
    }
    if any {
        let _ = std::fs::remove_dir(&top);
    }
}

/// What a session Luna starts gets of the library's folders.
#[derive(Default, Debug, PartialEq)]
pub struct SessionSkills {
    /// For its CLI: Codex a `-c` per folder's plugin and a capped one for the
    /// skills its folders keep from it; Claude Code nothing until the `--plugin-dir`
    /// of `plugins` is added once they are made (`give_plugins`).
    pub args: Vec<String>,
    /// Claude Code: the folders' skills this session is given as plugins
    /// of its own, one per folder, only the skills it is to have in them.
    pub plugins: Vec<FolderPlugin>,
    /// The account's skills its folders keep from the session, as its CLI
    /// keys them: Claude Code by the name it
    /// lists them under — `skillOverrides` in the settings it is started with
    /// (pty/start.rs `claude_settings_file`), the one way to keep a skill from
    /// a session that its account has as a whole; Codex helpers by the path of the
    /// skill's `SKILL.md` in the thread's `skills.config`, so a skill of the repository's
    /// that happens to share the name, or one whose front matter names it
    /// otherwise than its folder, is neither hidden nor missed.
    pub hidden: Vec<String>,
    /// Codex chats: the skills to hide that did not fit the command line's room for
    /// them (`CODEX_HIDE_MAX`), by path: visible to the session after all.
    pub unhidden: Vec<String>,
    /// Claude Code: the account's instructions file, where a folder the
    /// session lies in gives it other instructions or none, or they are not
    /// for its role — `claudeMdExcludes` in the same settings, once the
    /// session's start has kept only what takes nothing of the project's own
    /// (pty/start.rs `claude_account_excludes`). Codex has no way to leave its
    /// account's `AGENTS.md` out of one session, so a folder says nothing to
    /// it of instructions.
    pub excluded: Vec<String>,
    /// Claude Code: what `excluded` would have left out but could not, said
    /// as what Luna applied.
    pub excluded_kept: Vec<String>,
    /// Claude Code: the memory files of the other folders added to the
    /// session, which reading the library's instructions would have it read
    /// too (claude/launch.rs `memory_excludes`); set by the session's start,
    /// beside `excluded` in `claudeMdExcludes`.
    pub memory_off: Vec<String>,
    /// Claude Code: the folders `memory_off` would have left out but could
    /// not, their patterns colliding with the session's own instruction paths
    /// (claude/launch.rs `keep_own_memory`), said as what Luna applied.
    pub memory_kept: Vec<String>,
    /// Claude Code: the library's file of the instructions a folder gives in
    /// place of the account's, which its caller gives the session among its
    /// role layers (role.rs `role_layers`).
    pub instructions: Option<PathBuf>,
    /// The library's instructions for some roles that this session's role
    /// takes and its account's reach does, by name and in name order, with
    /// their text: given to it alone, in its role layers (role.rs), never laid
    /// into an account (audience.rs).
    pub role_entries: Vec<(String, String)>,
    /// What of the above the entries' audience decided (library/audience.rs),
    /// said as what Luna applied (`audience_applied`).
    pub audience: Vec<String>,
}

/// What a session Luna starts gets of the library's folders, for each folder
/// its work lies in — the folder it starts in, or the project a worktree of
/// it belongs to: Claude Code a plugin of the session's own holding the
/// skills that folder gives it (`FolderPlugin`), Codex a `-c` turning that
/// folder's plugin on where the account has one; and, from either, the skills those folders keep from it
/// (`deciding`) hidden. Due at every start, resumes and each helper turn
/// included: one started without them keeps the skills named in its
/// conversation, and none of them answers. Its caller lays the library out
/// first (`lay_out`), so the session reads its copies as the library has
/// them now — off any lock of the caller's, as a pass reads and writes every
/// account's copies. `role`: whose session it is, for the entries meant for
/// some roles alone (library/audience.rs).
pub fn session_args(provider: Provider, account_path: &str, folder: &str, role: Role) -> SessionSkills {
    // A library a test holds, for the accounts it made (testkit
    // `GoldenLibrary`), in place of Luna's own settings and accounts.
    #[cfg(test)]
    if let Some((dirs, lib, a)) = held(provider, account_path) {
        return folder_args(&dirs, &lib, &a, folder, role);
    }
    let lib = crate::settings::get().library;
    if lib.folders.is_empty() && lib.audience.is_empty() {
        return SessionSkills::default();
    }
    let account = crate::accounts::list_accounts()
        .unwrap_or_default()
        .into_iter()
        .find(|a| a.provider == provider && crate::pty::same_dir(&a.path, account_path));
    match account {
        Some(a) => folder_args(&Dirs::current(), &lib, &a, folder, role),
        None => SessionSkills::default(),
    }
}

/// The library's folders a session's work lies in: those the folder it starts
/// in lies in — in a worktree, the same folder of the project it belongs to
/// (`<worktree>/src` is `<project>/src`), wherever the worktree itself lies.
fn folders_of<'a>(lib: &'a LibrarySettings, folder: &str) -> Vec<(&'a LibraryFolder, String)> {
    let work = crate::checkout::root_of_in_settings(folder, lib.folders.iter().map(|f| f.path.as_str()))
        .and_then(|(root, project)| Path::new(folder).strip_prefix(&root).ok().map(|rel| Path::new(&project).join(rel)));
    folder_slugs(&lib.folders)
        .into_iter()
        .filter(|(f, _)| {
            let chosen = Path::new(&f.path);
            let path = work.as_deref().unwrap_or_else(|| Path::new(folder));
            within(path, chosen) || crate::checkout::within_settings_root(path, chosen)
        })
        .collect()
}

pub(super) fn folder_args(dirs: &Dirs, lib: &LibrarySettings, a: &AccountInfo, folder: &str, role: Role) -> SessionSkills {
    let lies_in = folders_of(lib, folder);
    let mut out = SessionSkills::default();
    for (f, slug) in &lies_in {
        match a.provider {
            Provider::Claude => {
                // Only what the session is to have, so there is nothing to
                // hide afterwards (`hidden_here`).
                let skills: BTreeMap<String, Files> = f
                    .skills
                    .keys()
                    .filter(|n| folder_copy_for(lib, f, n, a) && skill_audience(lib, n).takes(role))
                    .filter(|n| gets_in(lib, lies_in.iter().map(|(f, _)| *f), n, a))
                    .filter_map(|n| Some((n.clone(), skill_files(dirs, n)?)))
                    .collect();
                if !skills.is_empty() {
                    out.plugins.push(FolderPlugin { slug: slug.clone(), skills });
                }
            }
            Provider::Codex => {
                if codex_plugin_dir(a, slug).join(".codex-plugin").join("plugin.json").is_file() {
                    out.args.extend(["-c".to_string(), format!("plugins.{slug}@{CODEX_MARKET}.enabled=true")]);
                }
            }
        }
    }
    // Codex reads its account's own file in every session and no folder's in
    // its place. That file holds only every session's instructions, which
    // fit every role: only a folder's choice can be unfit for this one.
    if a.provider == Provider::Claude {
        let folder_choice = folder_instructions(lies_in.iter().map(|(f, _)| *f), a);
        let unfit = folder_choice.flatten().filter(|&n| !instructions_audience(lib, n).takes(role));
        if let Some(n) = unfit {
            out.audience.push(kept_note("instructions", n, instructions_audience(lib, n)));
        }
        let choice = if unfit.is_some() { Some(None) } else { folder_choice };
        if let Some(choice) = choice {
            let own = Path::new(&a.path).join(instructions_name(a.provider));
            // Instructions the library no longer has are none.
            let text = choice.and_then(|n| std::fs::read(dirs.instructions_file(n)).ok());
            // What the account's file holds already stays, as the library
            // lays it out there; anything else — none at all, or a copy of
            // the account's changed there — leaves out whatever it holds,
            // the library's or not.
            let there = text.as_ref().is_some_and(|t| std::fs::read(&own).is_ok_and(|o| o == *t));
            if !there {
                out.excluded.push(own.to_string_lossy().replace('\\', "/"));
                out.instructions = choice.filter(|_| text.as_ref().is_some_and(|t| !t.is_empty())).map(|n| dirs.instructions_file(n));
            }
        }
    }
    // The library's instructions for some roles, this one among them, whose
    // reach takes the account: each given to this session alone, all of them
    // in name order (audience.rs). Not one a folder gives in the account's
    // place, which is given as that. Blank or gone is none.
    let given = out.instructions.clone();
    for (name, reach) in &lib.instructions {
        let audience = instructions_audience(lib, name);
        if laid_into_accounts(lib, name) || !audience.takes(role) || !reaches(reach, a) {
            continue;
        }
        let file = dirs.instructions_file(name);
        if given.as_ref() == Some(&file) {
            continue;
        }
        if let Some(text) = std::fs::read_to_string(&file).ok().filter(|t| !t.trim().is_empty()) {
            out.audience.push(given_note(name, audience));
            out.role_entries.push((name.clone(), text));
        }
    }
    let (hidden, by_audience) = hidden_here(lib, a, &lies_in, role);
    out.audience.extend(by_audience);
    match a.provider {
        Provider::Claude => out.hidden = hidden,
        // One `-c` for them all: a second on the key would take the first's
        // place. Codex adds its entries to the account's own `skills.config`
        // ones, selector by selector, rather than putting them in their place.
        // By path (a `SKILL.md`'s), as Codex's `skills.config` takes it: a
        // spelling of it that Codex normalizes alike, junctions followed
        // (0.169.0).
        // Keep the capped argv override for helpers too until thread-level
        // filtering is verified. A helper also sends the full list with its thread.
        Provider::Codex if !hidden.is_empty() => {
            let value = |entries: &[String]| format!("skills.config=[{}]", entries.join(","));
            let mut entries: Vec<String> = Vec::new();
            let mut trimmed = 0;
            for path in &hidden {
                entries.push(format!("{{path={},enabled=false}}", toml_edit::Value::from(path.as_str())));
                if command_line_units(&value(&entries)) > CODEX_HIDE_MAX {
                    entries.pop();
                    trimmed += 1;
                    if role == Role::Chat {
                        out.unhidden.push(path.clone());
                    }
                }
            }
            if trimmed != 0 {
                crate::log::warn(
                    "library",
                    &format!(
                        "library.hidden-trimmed: {trimmed} of {} skills to hide from a Codex session ({role:?}) do not fit its command line",
                        hidden.len()
                    ),
                );
            }
            if !entries.is_empty() {
                out.args.extend(["-c".to_string(), value(&entries)]);
            }
            if role != Role::Chat {
                out.hidden = hidden;
            }
        }
        Provider::Codex => {}
    }
    out
}

/// The skills a session on this account would see that the folders it lies
/// in keep from it, as its CLI keys them: Claude Code the account's own copy
/// by the skill's name (`skillOverrides`); Codex each copy by the path of its `SKILL.md` (`skill_path`), the account's
/// own and a folder's plugin's, so a skill of the repository's with the same
/// name stays. A Claude Code plugin holds only what its session is given
/// (`folder_args`), so a copy kept from it is never made. A skill not meant
/// for this session's role (`role`) is kept from it wherever it comes from,
/// and said in the second list.
fn hidden_here(lib: &LibrarySettings, a: &AccountInfo, lies_in: &[(&LibraryFolder, String)], role: Role) -> (Vec<String>, Vec<String>) {
    let named: BTreeSet<&String> = lib.skills.keys().chain(lies_in.iter().flat_map(|(f, _)| f.skills.keys())).collect();
    let mut out = BTreeSet::new();
    let mut by_audience = Vec::new();
    for name in named {
        let audience = skill_audience(lib, name);
        let not_for_it = !audience.takes(role);
        if !not_for_it && gets_in(lib, lies_in.iter().map(|(f, _)| *f), name, a) {
            continue;
        }
        let before = out.len();
        // A folder's copy left out of a Claude Code plugin counts as kept too.
        let mut withheld = false;
        if account_copy_for(lib, name, a) {
            out.insert(match a.provider {
                Provider::Claude => name.clone(),
                Provider::Codex => skill_path(&Path::new(&a.path).join("skills"), name),
            });
        }
        for (_, slug) in lies_in.iter().filter(|(f, _)| folder_copy_for(lib, f, name, a)) {
            match a.provider {
                Provider::Claude => withheld = true,
                Provider::Codex => {
                    out.insert(skill_path(&codex_plugin_dir(a, slug).join("skills"), name));
                }
            }
        }
        if not_for_it && (out.len() > before || withheld) {
            by_audience.push(kept_note("skill", name, audience));
        }
    }
    (out.into_iter().collect(), by_audience)
}

/// The path of the `SKILL.md` of the skill `name` in `skills`, which is how
/// Codex's `skills.config` names a skill by where it is (0.169.0): the file,
/// not its folder. Slashes on Windows too, which Codex matches as it does
/// backslashes and which a `-c` value takes without escapes.
fn skill_path(skills: &Path, name: &str) -> String {
    let path = skills.join(name).join(SKILL_FILE).to_string_lossy().into_owned();
    if cfg!(windows) {
        path.replace('\\', "/")
    } else {
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a chat of the person's started there gets: these tests are of
    /// folders, the same for every role where no audience is set.
    fn folder_args(dirs: &Dirs, lib: &LibrarySettings, a: &AccountInfo, folder: &str) -> SessionSkills {
        super::folder_args(dirs, lib, a, folder, Role::Chat)
    }

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

    /// The registration Git writes for a worktree in the Codex account.
    fn codex_worktree(f: &Fixture, repo: &Path) -> PathBuf {
        let wt = f.account(1).join("worktrees/0a1b/repo");
        let marker = wt.join(".git");
        let gitdir = repo.join(".git/worktrees/repo");
        write_file(&marker, format!("gitdir: {}\n", gitdir.display()).as_bytes()).unwrap();
        write_file(&gitdir.join("commondir"), b"../..\n").unwrap();
        write_file(&gitdir.join("gitdir"), format!("{}\n", marker.display()).as_bytes()).unwrap();
        wt
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
        codex_manifests(&lib, &f.accounts, &mut ledger);
        assert!(f.account(0).join("skills/review/SKILL.md").is_file());
        assert!(!f.dirs.folders().exists(), "Claude Code's are a plugin made for each session");
        let plugin = f.account(1).join("plugins/cache/rm-luna/asuri-app/local");
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
        f.run(&lib, &mut ledger, true);
        codex_manifests(&lib, &f.accounts, &mut ledger);
        assert!(!f.account(0).join("skills/review").exists());
        assert!(!f.account(1).join("plugins/cache/rm-luna").exists(), "a plugin with nothing in it goes, manifest and all");
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
        assert!(!f.dirs.folders().exists(), "nor does one wait for its sessions in Luna's folder");
    }

    /// A session of Claude Code is given its folder's skills as a plugin made
    /// for it (session_plugin.rs), so no copy waits for it anywhere.
    #[test]
    fn a_claude_sessions_folder_skills_are_a_plugin_and_no_copy_waits_for_it() {
        let f = Fixture::new("folder-plugin");
        f.skill("review", "v1");
        f.skill("notes", "v1");
        // The repository has a review of its own, and so has the account: a
        // plugin's skills are namespaced, so neither name is taken.
        let (repo, folder) = repo_folder(&f, "repo", &["review", "notes"]);
        write_file(&repo.join(".claude/skills/review/SKILL.md"), b"---\nname: review\ndescription: The repo's.\n---\n").unwrap();
        let on = LibrarySettings { folders: vec![folder], ..Default::default() };
        let p = f.run(&on, &mut Ledger::default(), true);
        assert!(p.places.iter().all(|x| !x.dest.contains("library-folders")), "{:?}", p.places);
        assert!(!f.dirs.folders().exists());

        let repo_s = repo.to_string_lossy().into_owned();
        let work = folder_args(&f.dirs, &on, &f.accounts[0], &repo_s);
        assert!(work.args.is_empty() && work.hidden.is_empty());
        let given: Vec<(&str, Vec<&str>)> =
            work.plugins.iter().map(|p| (p.slug.as_str(), p.skills.keys().map(String::as_str).collect())).collect();
        assert_eq!(given, [("repo", vec!["notes", "review"])]);
        assert!(work.plugins[0].skills["review"].contains_key(SKILL_FILE));
        // A folder that is not the folder's: nothing.
        let elsewhere = f.base.join("elsewhere").to_string_lossy().into_owned();
        assert_eq!(folder_args(&f.dirs, &on, &f.accounts[0], &elsewhere), SessionSkills::default());
        // A skill gone from the library is not given.
        std::fs::remove_dir_all(f.dirs.skills().join("notes")).unwrap();
        let work = folder_args(&f.dirs, &on, &f.accounts[0], &repo_s);
        assert_eq!(work.plugins[0].skills.keys().map(String::as_str).collect::<Vec<_>>(), ["review"]);
    }

    /// What an earlier Luna left in `library-folders` for Claude Code's
    /// sessions goes, whichever account it was for: its copies, noted as
    /// Luna's, are taken out (one changed since to the backups first), and
    /// the folders they leave empty go. What Luna did not write there stays,
    /// and Codex's copies, in its account's plugin cache, stay as they are.
    #[test]
    fn copies_an_earlier_luna_left_for_claude_sessions_go() {
        let f = Fixture::new("folder-leftovers");
        f.skill("review", "v1");
        let (_, folder) = repo_folder(&f, "repo", &["review"]);
        let lib = LibrarySettings { folders: vec![folder], ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        codex_manifests(&lib, &f.accounts, &mut ledger);
        let codex = codex_plugin_dir(&f.accounts[1], "repo").join("skills").join("review");
        let codex_note = ledger.written.get(&*codex.to_string_lossy()).cloned().expect("Codex's copy, noted");

        // 0.20's layout: `<slug>/<account>/.claude/skills/<name>`, for a
        // listed account and for one no longer there.
        let files = read_dir_files(&f.dirs.skills().join("review")).unwrap();
        let copy = |account: &str, name: &str| f.dirs.folders().join("repo").join(account).join(".claude/skills").join(name);
        let (work, changed, gone) = (copy("work", "review"), copy("home", "luna-review"), copy("gone", "review"));
        for at in [&work, &changed, &gone] {
            write_at(at, false, &files).unwrap();
            ledger.written.insert(at.to_string_lossy().into_owned(), hashes(&files));
        }
        std::fs::write(changed.join(SKILL_FILE), "changed there").unwrap();
        let theirs = copy("home", "theirs").join(SKILL_FILE);
        write_file(&theirs, b"not Luna's").unwrap();
        std::fs::create_dir_all(f.dirs.folders().join("old-folder/work/.claude/skills")).unwrap();

        let p = f.run(&lib, &mut ledger, true);
        codex_manifests(&lib, &f.accounts, &mut ledger);
        prune_folder_dirs(&f.dirs);
        assert!(!work.exists() && !changed.exists() && !gone.exists(), "Luna's copies are taken out");
        let mut removed = p.removed.clone();
        removed.sort();
        let mut want: Vec<String> = [&work, &changed, &gone].iter().map(|p| p.to_string_lossy().into_owned()).collect();
        want.sort();
        assert_eq!(removed, want, "and said as taken out");
        assert!(backed_up(&f.dirs).iter().any(|b| b == "changed there"), "the changed one is in the backups");
        assert!(!ledger.written.keys().any(|k| k.contains("library-folders")), "no note of them stays");
        assert_eq!(std::fs::read(&theirs).unwrap(), b"not Luna's", "someone else's file stays");
        assert!(!f.dirs.folders().join("repo/work").exists() && !f.dirs.folders().join("repo/gone").exists());
        assert!(!f.dirs.folders().join("old-folder").exists(), "empty folders go, listed or not");
        assert!(codex.join(SKILL_FILE).is_file(), "Codex's copy stays");
        assert_eq!(ledger.written.get(&*codex.to_string_lossy()), Some(&codex_note));
        assert_eq!(Fixture::status(&p, &codex), Status::Current);

        // Once nothing is left, `library-folders` itself goes.
        std::fs::remove_dir_all(theirs.parent().unwrap()).unwrap();
        prune_folder_dirs(&f.dirs);
        assert!(!f.dirs.folders().exists());
    }

    /// Only the folders of the copies' layout go: an empty folder of someone
    /// else's beside them under a listed folder's slug, or one standing for
    /// an accounts folder put in `library-folders`, stays, and so do the
    /// folders around it.
    #[test]
    fn only_the_copies_own_folders_are_pruned() {
        let f = Fixture::new("folder-prune-foreign");
        let (_, folder) = repo_folder(&f, "repo", &[]);
        let lib = LibrarySettings { folders: vec![folder], ..Default::default() };
        let top = f.dirs.folders();
        for dir in ["repo/work/.claude/skills", "repo/foreign", "accounts/anthropic/work", "other/home/.claude/notes"] {
            std::fs::create_dir_all(top.join(dir)).unwrap();
        }
        f.run(&lib, &mut Ledger::default(), true);
        prune_folder_dirs(&f.dirs);
        assert!(!top.join("repo/work").exists(), "the copies' own folders go");
        assert!(top.join("repo/foreign").is_dir(), "someone else's empty folder stays");
        assert!(top.join("accounts/anthropic/work").is_dir(), "and an accounts folder put there");
        assert!(top.join("other/home/.claude/notes").is_dir(), "a `.claude` with no `skills` alone in it stays");
    }

    /// A link at `library-folders` leads out of Luna's data folder: nothing
    /// is pruned through it.
    #[test]
    fn a_linked_library_folders_is_not_pruned_through() {
        let f = Fixture::new("folder-prune-link");
        let elsewhere = f.base.join("elsewhere");
        std::fs::create_dir_all(elsewhere.join("repo/work/.claude/skills")).unwrap();
        std::fs::create_dir_all(&f.dirs.data).unwrap();
        link_dir(&f.dirs.folders(), &elsewhere);
        prune_folder_dirs(&f.dirs);
        assert!(elsewhere.join("repo/work/.claude/skills").is_dir());
        unlink_dir(&f.dirs.folders());
    }

    /// A copy an earlier Luna laid into a folder still listed, but given no
    /// skill any more, goes too: the folder is listed, so its top. One in a
    /// folder no longer listed stays, Luna's note with it.
    #[test]
    fn an_old_copy_in_a_listed_folder_giving_nothing_goes_and_an_unlisted_ones_stays() {
        let f = Fixture::new("folder-old-idle");
        f.skill("review", "v1");
        let (repo, folder) = repo_folder(&f, "repo", &[]);
        let (gone_repo, _) = repo_folder(&f, "unlisted", &[]);
        let files = read_dir_files(&f.dirs.skills().join("review")).unwrap();
        let mut ledger = Ledger::default();
        let (old, stray) = (repo.join("review"), gone_repo.join("review"));
        for at in [&old, &stray] {
            write_at(at, false, &files).unwrap();
            ledger.written.insert(at.to_string_lossy().into_owned(), hashes(&files));
        }
        f.run(&LibrarySettings { folders: vec![folder], ..Default::default() }, &mut ledger, true);
        assert!(!old.exists(), "a listed folder is its top");
        assert!(stray.join(SKILL_FILE).is_file(), "an unlisted one has none: not taken out");
        assert!(ledger.written.contains_key(&*stray.to_string_lossy()), "its note stays");
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

    /// A link or junction on the way down from a Codex account's folder to a
    /// plugin's manifest Luna wrote and noted, at the plugin's `.codex-plugin`
    /// or at `plugins/cache` above all of Luna's plugins: the manifest is
    /// neither written through it nor, once the folder's copies are gone,
    /// taken out through it, and its note stays for a later pass.
    #[test]
    fn a_manifest_behind_a_link_is_left_alone() {
        type At = fn(&Path, &Path) -> PathBuf;
        let ways: [(&str, At); 2] =
            [("plugin", |plugin, _| plugin.join(".codex-plugin")), ("cache", |_, account| account.join("plugins").join("cache"))];
        for (tag, at) in ways {
            let f = Fixture::new(&format!("manifest-link-{tag}"));
            f.skill("review", "v1");
            let (_, folder) = repo_folder(&f, "repo", &["review"]);
            let lib = LibrarySettings { folders: vec![folder], ..Default::default() };
            let mut ledger = Ledger::default();
            f.run(&lib, &mut ledger, true);
            codex_manifests(&lib, &f.accounts, &mut ledger);
            let plugin = codex_plugin_dir(&f.accounts[1], "repo");
            let manifest = plugin.join(".codex-plugin").join("plugin.json");
            assert!(manifest.is_file() && ledger.manifests.contains(&*manifest.to_string_lossy()), "{tag}: noted as Luna's");

            // That folder moved elsewhere, a link in its place, and what it
            // leads to given someone else's manifest.
            let link = at(&plugin, &f.account(1));
            let elsewhere = f.base.join("elsewhere");
            std::fs::rename(&link, &elsewhere).unwrap();
            link_dir(&link, &elsewhere);
            std::fs::write(&manifest, "theirs").unwrap();
            codex_manifests(&lib, &f.accounts, &mut ledger);
            assert_eq!(std::fs::read_to_string(&manifest).unwrap(), "theirs", "{tag}: not written through the link");

            let none = LibrarySettings::default();
            f.run(&none, &mut ledger, true);
            codex_manifests(&none, &f.accounts, &mut ledger);
            assert_eq!(std::fs::read_to_string(&manifest).unwrap(), "theirs", "{tag}: nor taken out through it");
            assert!(ledger.manifests.contains(&*manifest.to_string_lossy()), "{tag}: still noted, for a later pass");
            unlink_dir(&link);
        }
    }

    #[test]
    fn a_session_in_a_folder_or_a_worktree_of_it_is_given_its_skills() {
        let f = Fixture::new("folder-args");
        f.skill("review", "v1");
        let (repo, folder) = repo_folder(&f, "repo", &["review"]);
        let lib = LibrarySettings { folders: vec![folder], ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        codex_manifests(&lib, &f.accounts, &mut ledger);
        let repo_s = repo.to_string_lossy().into_owned();

        // In it, in a folder inside it, and in a worktree Claude Code made of
        // it: the same plugin.
        let sub = repo.join("src").to_string_lossy().into_owned();
        let claude_wt = repo.join(".claude/worktrees/x").to_string_lossy().into_owned();
        for at in [&repo_s, &sub, &claude_wt] {
            let given = folder_args(&f.dirs, &lib, &f.accounts[0], at);
            assert!(given.args.is_empty(), "nothing is added: the plugin is passed once it is made");
            assert_eq!(given.plugins.iter().map(|p| p.slug.as_str()).collect::<Vec<_>>(), ["repo"], "{at}");
        }
        let empty = LibrarySettings { folders: vec![LibraryFolder { path: repo_s.clone(), ..Default::default() }], ..Default::default() };
        assert!(folder_args(&f.dirs, &empty, &f.accounts[2], &repo_s).plugins.is_empty(), "a folder giving nothing, no plugin");

        // Codex, in a registered worktree it made of the repository.
        let wt = codex_worktree(&f, &repo);
        let codex = ["-c".to_string(), "plugins.repo@rm-luna.enabled=true".to_string()];
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[1], &repo_s).args, codex);
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[1], &wt.to_string_lossy()).args, codex);
        #[cfg(windows)]
        if let Some(short) = crate::checkout::short_alias(&f.base) {
            let short = short.join(repo.file_name().unwrap());
            let mut alternate = lib.clone();
            alternate.folders[0].path = short.to_string_lossy().into_owned();
            assert_eq!(folder_args(&f.dirs, &alternate, &f.accounts[1], &repo_s).args, codex);
            assert_eq!(folder_args(&f.dirs, &alternate, &f.accounts[1], &wt.to_string_lossy()).args, codex);
        }
        #[cfg(any(unix, windows))]
        {
            let alias = f.base.join("repo-parent-alias");
            link_dir(&alias, repo.parent().unwrap());
            let mut aliased = lib.clone();
            aliased.folders[0].path = alias.join(repo.file_name().unwrap()).to_string_lossy().into_owned();
            assert_eq!(folder_args(&f.dirs, &aliased, &f.accounts[1], &wt.join("src").to_string_lossy()).args, codex);
            unlink_dir(&alias);
        }
        f.run(&empty, &mut ledger, true);
        codex_manifests(&empty, &f.accounts, &mut ledger);
        assert!(folder_args(&f.dirs, &empty, &f.accounts[1], &repo_s).args.is_empty(), "no plugin, nothing to turn on");

        let elsewhere = f.base.join("elsewhere").to_string_lossy().into_owned();
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[0], &elsewhere), SessionSkills::default());
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[1], &elsewhere), SessionSkills::default());
    }

    /// Only a manifest the ledger says Luna wrote is taken out: a plugin
    /// someone else put in Luna's marketplace folder, with no `skills/`,
    /// keeps its own; Luna's goes once its folder's copies have.
    #[test]
    fn only_a_manifest_luna_wrote_is_taken_out() {
        let f = Fixture::new("manifest-theirs");
        f.skill("review", "v1");
        let (_, folder) = repo_folder(&f, "repo", &["review"]);
        let lib = LibrarySettings { folders: vec![folder], ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        let theirs = codex_plugin_dir(&f.accounts[1], "theirs").join(".codex-plugin").join("plugin.json");
        write_file(&theirs, b"{}").unwrap();
        codex_manifests(&lib, &f.accounts, &mut ledger);
        let ours = codex_plugin_dir(&f.accounts[1], "repo").join(".codex-plugin").join("plugin.json");
        assert!(ours.is_file() && ledger.manifests.contains(&*ours.to_string_lossy()));
        assert!(theirs.is_file());

        let none = LibrarySettings::default();
        f.run(&none, &mut ledger, true);
        codex_manifests(&none, &f.accounts, &mut ledger);
        assert!(!ours.exists(), "Luna's, taken out");
        assert!(ledger.manifests.is_empty());
        assert_eq!(std::fs::read(&theirs).unwrap(), b"{}", "someone else's stays");
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
        codex_manifests(&lib, &f.accounts, &mut ledger);
        assert!(f.account(0).join("skills/review/SKILL.md").is_file(), "the account's copy stays where it is");
        assert!(!f.dirs.folders().join("work").exists(), "Claude Code's folder skills are a plugin made for each session");

        // In the outer folder: review hidden, its copy the account's own.
        let other = outer.join("other").to_string_lossy().into_owned();
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[0], &other).hidden, ["review"]);
        // Codex hides each copy by the path of its SKILL.md.
        let md = |dir: PathBuf, name: &str| dir.join(name).join("SKILL.md").to_string_lossy().replace('\\', "/");
        let review = md(f.account(1).join("skills"), "review");
        let notes = md(codex_plugin_dir(&f.accounts[1], "work").join("skills"), "notes");
        let codex = folder_args(&f.dirs, &lib, &f.accounts[1], &other).args;
        assert_eq!(
            codex,
            ["-c", "plugins.work@rm-luna.enabled=true", "-c", &format!("skills.config=[{{path=\"{review}\",enabled=false}}]")]
        );

        // In the repository: notes too. Claude Code's account copy by its name,
        // the outer folder's copy never made for the plugin; Codex's by path,
        // the account's own and the outer folder's plugin's.
        let repo_s = repo.to_string_lossy().into_owned();
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[2], &repo_s).hidden, ["review"]);
        assert!(folder_args(&f.dirs, &lib, &f.accounts[2], &repo_s).plugins.is_empty(), "the outer folder's notes are kept from it");
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[0], &repo_s).hidden, ["notes", "review"]);
        let codex = folder_args(&f.dirs, &lib, &f.accounts[1], &repo_s).args;
        let mut both = [notes, review];
        both.sort();
        let entries: Vec<String> = both.iter().map(|p| format!("{{path=\"{p}\",enabled=false}}")).collect();
        assert_eq!(codex.last().map(String::as_str), Some(format!("skills.config=[{}]", entries.join(",")).as_str()));

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
        codex_manifests(&lib, &f.accounts, &mut ledger);
        assert!(f.account(0).join("skills/review/SKILL.md").is_file());
        assert!(!f.account(2).join("skills/review").exists());
        assert!(f.account(1).join("plugins/cache/rm-luna/app/local/skills/review/SKILL.md").is_file());
        assert!(f.account(1).join("plugins/cache/rm-luna/web/local/skills/review/SKILL.md").is_file());

        let (app_s, web_s) = (app.to_string_lossy().into_owned(), web.to_string_lossy().into_owned());
        assert!(folder_args(&f.dirs, &lib, &f.accounts[0], &app_s).hidden.is_empty());
        assert!(folder_args(&f.dirs, &lib, &f.accounts[2], &app_s).hidden.is_empty());
        // Claude Code's folder skills are a plugin made for the session: in
        // app, claude/home's gets it, claude/work's has it as a whole already,
        // in web neither is given it.
        let plugin_slugs = |i: usize, folder: &str| -> Vec<String> {
            folder_args(&f.dirs, &lib, &f.accounts[i], folder).plugins.iter().map(|p| p.slug.clone()).collect()
        };
        assert_eq!(plugin_slugs(2, &app_s), ["app"]);
        assert!(plugin_slugs(0, &app_s).is_empty(), "work has it as a whole");
        assert!(plugin_slugs(0, &web_s).is_empty() && plugin_slugs(2, &web_s).is_empty());
        assert!(!f.dirs.folders().exists(), "no copy waits for a Claude Code session");
        // In web the account that has it as a whole does not get it; Codex does.
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[0], &web_s).hidden, ["review"]);
        assert!(folder_args(&f.dirs, &lib, &f.accounts[2], &web_s).hidden.is_empty(), "nothing of it to hide");
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[1], &web_s).args, ["-c", "plugins.web@rm-luna.enabled=true"]);
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
        let wt = codex_worktree(&f, &repo);
        std::fs::create_dir_all(wt.join("src")).unwrap();
        let review = f.account(1).join("skills/review/SKILL.md").to_string_lossy().replace('\\', "/");
        let hide = ["-c".to_string(), format!("skills.config=[{{path=\"{review}\",enabled=false}}]")];
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
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        codex_manifests(&lib, &f.accounts, &mut ledger);
        assert!(!f.account(1).join("plugins/cache/rm-luna/web").exists());
        let web = repo.join("web").to_string_lossy().into_owned();
        assert_eq!(folder_args(&f.dirs, &lib, &f.accounts[1], &web).args, ["-c", "plugins.repo@rm-luna.enabled=true"]);
        let plugins = |lib: &LibrarySettings| -> Vec<String> {
            folder_args(&f.dirs, lib, &f.accounts[0], &web).plugins.iter().map(|p| p.slug.clone()).collect()
        };
        assert_eq!(plugins(&lib), ["repo"], "Claude Code's plugin of the outer folder holds it");

        // Kept from the account outside, given again inside: the inside's own.
        lib.folders[0].skills.insert("review".into(), reach(&["codex"]));
        assert_eq!(plugins(&lib), ["web"]);
        assert!(folder_args(&f.dirs, &lib, &f.accounts[0], &web).hidden.is_empty());
    }

    /// Eighty skills meant for chats alone are more to hide from a Codex
    /// helper than the command line has room for: as many go in as fit, the
    /// rest are named as still visible, and the argument stays within its
    /// share.
    #[test]
    fn a_long_hide_list_is_cut_for_the_chat_and_whole_for_the_helper() {
        let f = Fixture::new("hide-budget");
        let names: Vec<String> = (0..80).map(|i| format!("a-skill-with-a-longish-name-{i:02}")).collect();
        let elsewhere = f.base.join("elsewhere").to_string_lossy().into_owned();
        let lib = LibrarySettings {
            skills: names.iter().map(|n| (n.clone(), all())).collect(),
            folders: vec![LibraryFolder {
                path: elsewhere.clone(),
                skills: names.iter().map(|n| (n.clone(), Vec::new())).collect(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let a = &f.accounts[1];
        let chat = folder_args(&f.dirs, &lib, a, &elsewhere);
        let value = &chat.args[1];
        assert!(command_line_units(value) <= CODEX_HIDE_MAX, "{} units", command_line_units(value));
        assert!(!chat.unhidden.is_empty(), "all eighty cannot fit");
        assert_eq!(value.matches("{path=").count() + chat.unhidden.len(), 80);
        let applied = audience_applied(&chat);
        let said = applied.iter().find(|i| i.key == "library.hidden-trimmed").expect("said");
        assert!(said.value.starts_with(&format!("{} stay visible", chat.unhidden.len())), "{}", said.value);
        for role in [Role::Lead, Role::Helper] {
            let warning = format!(
                "library.hidden-trimmed: {} of {} skills to hide from a Codex session ({role:?})",
                chat.unhidden.len(),
                names.len()
            );
            let warning_count = || std::fs::read_to_string(crate::log::path()).unwrap_or_default().matches(&warning).count();
            let before = warning_count();
            let helper = super::folder_args(&f.dirs, &lib, a, &elsewhere, role);
            assert!(warning_count() > before, "the capped argv is logged");
            assert_eq!(helper.args, chat.args, "the same capped argv fallback");
            assert!(audience_applied(&helper).iter().all(|i| i.key != "library.hidden-trimmed"));
            assert_eq!(helper.hidden.len(), names.len());
        }
        // A short list is whole.
        let few = LibrarySettings { skills: names.iter().take(3).map(|n| (n.clone(), all())).collect(), ..lib.clone() };
        let chat = folder_args(&f.dirs, &few, a, &elsewhere);
        assert!(chat.unhidden.is_empty() && chat.args[1].matches("{path=").count() == 3);
    }
}
