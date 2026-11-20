//! The window's library commands: settings changed and laid out, an account's
//! own instructions taken in, and the library's own files read, written, made
//! and deleted.

use super::*;

/// What the library holds and where each copy stands, without writing.
pub fn library_overview() -> Result<Overview, String> {
    overview(false)
}

/// Lays the library out: everything that needs no asking.
pub fn library_sync() -> Result<Overview, String> {
    overview(true)
}

/// A pass as Sync makes one, for a caller that only needs it done. A file
/// changed in the library by another program — a model improving a skill
/// there — is seen by no window, and the accounts would keep the old copy
/// until a click in it.
pub fn lay_out(when: &str) {
    match overview(true) {
        Err(e) => crate::log::warn("library", &format!("laying the library out {when}: {e}")),
        // No window sees what a start's pass could not do (a backup that
        // would not write, so a changed copy not written over): the log says
        // each place once a run, as long as it stays so.
        Ok(o) => {
            for place in o.places.iter().filter(|p| p.status == Status::Failed) {
                let problem = place.problem.as_deref().unwrap_or("no reason given");
                if failure_is_news(&place.dest, problem) {
                    crate::log::warn("library", &format!("laying the library out {when}: {} not laid out: {problem}", place.dest));
                }
            }
        }
    }
}

/// Whether this place's failure, with this reason, is not what the log said
/// of it last in this run.
fn failure_is_news(dest: &str, problem: &str) -> bool {
    static SAID: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());
    let mut said = SAID.lock().unwrap_or_else(|e| e.into_inner());
    said.insert(dest.to_string(), problem.to_string()).as_deref() != Some(problem)
}

/// `lay_out` before a session starts on `account_path`, the account counted
/// as in use from before the pass waits for the lock until its process is
/// counted itself: a rename of the account that would go first waits for
/// the same lock and, finding it in use, refuses (accounts.rs). One that
/// went first, before it was counted, has moved the folder by the time the
/// pass is done: the start is refused then, not made on a folder that is
/// gone, which the CLI would make anew as an account signed in to nothing.
/// So is one whose folder went before the pass began: a start is never made
/// on an account folder that is not there.
pub fn lay_out_for(when: &str, account_path: &str) -> Result<crate::account_use::Running, String> {
    let reserved = crate::account_use::running_on(account_path);
    #[cfg(test)]
    let made_up = !Path::new(account_path).is_dir() && !STRICT.with(|s| s.get());
    lay_out(when);
    #[cfg(test)]
    if made_up {
        return Ok(reserved);
    }
    if !Path::new(account_path).is_dir() {
        return Err(format!("{account_path} is not there any more: the account was renamed or deleted as this session started"));
    }
    Ok(reserved)
}

#[cfg(test)]
thread_local! {
    /// The launch tests start sessions on account paths made up for them
    /// (`/golden/acc-claude`), never made on disk: in a test build, and only
    /// there, a start on a folder missing from the first goes on unless the
    /// test asks for the check as it is made outside tests.
    static STRICT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Points the library at another folder; empty = back to `<data>/library`.
/// Nothing is moved.
pub fn library_set_source(path: String) -> Result<Overview, String> {
    let path = path.trim().to_string();
    if !path.is_empty() && !Path::new(&path).is_absolute() {
        return Err("The library's folder must be an absolute path".into());
    }
    crate::log::info("library", &format!("library folder: {}", if path.is_empty() { "<default>" } else { &path }));
    change(|l| l.source = (!path.is_empty()).then_some(path))
}

/// The accounts instructions go into (`name` None: the accounts that get
/// none), each entry taken from whichever other instructions had it; empty =
/// none.
pub fn library_set_instructions(name: Option<String>, accounts: Vec<String>) -> Result<Overview, String> {
    if let Some(n) = &name {
        check_name(n)?;
    }
    check_reach(&accounts)?;
    change(|l| give_instructions(l, name.as_deref(), accounts))
}

/// The accounts a skill goes into; empty = none.
pub fn library_set_accounts(skill: String, accounts: Vec<String>) -> Result<Overview, String> {
    check_skill_name(&skill)?;
    let reach = tidy_reach(accounts)?;
    change(|l| {
        if reach.is_empty() {
            l.skills.remove(&skill);
        } else {
            l.skills.insert(skill, reach);
        }
    })
}

/// Lists a folder, saying nothing of any skill yet, or takes it off the list
/// with all it says.
pub fn library_set_folder(path: String, listed: bool) -> Result<Overview, String> {
    if !Path::new(&path).is_absolute() {
        return Err("A folder must be an absolute path".into());
    }
    let on_disk = slugs_on_disk(&Dirs::current());
    change(|l| {
        // Kept before any goes, so that none passes to another.
        settle_slugs(l);
        let at = l.folders.iter().position(|f| crate::pty::same_dir(&f.path, &path));
        match (at, listed) {
            (Some(i), false) => {
                l.folders.remove(i);
            }
            (None, true) => {
                let mut taken: BTreeSet<String> = l.folders.iter().map(|f| f.slug.clone()).collect();
                taken.extend(on_disk);
                let slug = fresh_slug(&path, &taken);
                l.folders.push(LibraryFolder { path, slug, ..Default::default() });
            }
            _ => {}
        }
    })
}

/// The accounts whose sessions in a listed folder get a skill (empty: none
/// of them), or — None — the folder saying nothing of it, so that they go by
/// what it is meant for and by the folders it lies in.
pub fn library_set_folder_skill(path: String, skill: String, accounts: Option<Vec<String>>) -> Result<Overview, String> {
    check_skill_name(&skill)?;
    let reach = accounts.map(tidy_reach).transpose()?;
    if crate::settings::get().library.folders.iter().all(|f| !crate::pty::same_dir(&f.path, &path)) {
        return Err(format!("{path} is not one of the library's folders"));
    }
    change(|l| {
        let Some(f) = l.folders.iter_mut().find(|f| crate::pty::same_dir(&f.path, &path)) else { return };
        match reach {
            Some(reach) => f.skills.insert(skill, reach),
            None => f.skills.remove(&skill),
        };
    })
}

/// What sessions on an account (a reach entry: as a rule `provider/name`) in
/// a listed folder get in place of its instructions: `name`'s, or none at all
/// (None); `inherit`, what holds outside the folder.
pub fn library_set_folder_instructions(path: String, account: String, inherit: bool, name: Option<String>) -> Result<Overview, String> {
    check_reach(std::slice::from_ref(&account))?;
    if let Some(n) = &name {
        check_name(n)?;
    }
    if crate::settings::get().library.folders.iter().all(|f| !crate::pty::same_dir(&f.path, &path)) {
        return Err(format!("{path} is not one of the library's folders"));
    }
    change(|l| {
        let Some(f) = l.folders.iter_mut().find(|f| crate::pty::same_dir(&f.path, &path)) else { return };
        match inherit {
            true => f.instructions.remove(&account),
            false => f.instructions.insert(account, name),
        };
    })
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LibraryFile {
    pub path: String,
    pub text: String,
    pub exists: bool,
    /// The library's folder it was read from, by its path and by its
    /// identity on its drive (`held::Dir::id`; empty: no folder there yet),
    /// and the sha256 of the bytes read (None: no file there): what a save
    /// bound to them (`library_write_checked`) holds the file to.
    pub root: String,
    pub root_id: String,
    pub revision: Option<String>,
}

/// The file the editor has open: `{"skill": name}`, `{"instructions": name}`
/// or `{"system": name}`, a system skill's override (system.rs).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Entry {
    Skill(String),
    Instructions(String),
    System(String),
}

/// The library's file of `entry`, walked down from the library's root
/// (system.rs `inside`): no link, junction or file in a folder's place on the
/// way, and inside the library once resolved, so a `skills/` or a skill's
/// folder planted as a link to an account's leads no read or write there.
fn library_path(dirs: &Dirs, entry: &Entry) -> Result<PathBuf, String> {
    match entry {
        Entry::Skill(s) => {
            check_skill_name(s)?;
            in_library_dir(dirs, &dirs.skills().join(s).join(SKILL_FILE))
        }
        Entry::Instructions(i) => {
            check_name(i)?;
            in_library_dir(dirs, &dirs.instructions_file(i))
        }
        // Read and written by system.rs alone, which keeps to its own folder.
        Entry::System(name) => Err(format!("{name} is a system skill")),
    }
}

/// `path`, a path under the library's root, as `inside` walks it down.
fn in_library_dir(dirs: &Dirs, path: &Path) -> Result<PathBuf, String> {
    let rel = path.strip_prefix(&dirs.root).map_err(|_| format!("{} lies outside the library", path.display()))?;
    let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    inside(&dirs.root, &parts.iter().map(String::as_str).collect::<Vec<_>>())
}

/// Instructions, a skill's SKILL.md or a system skill's override, for the
/// editor: where there is no override yet, Luna's own text to start one from.
/// Opened as itself, relative to the held root (`read_held`): a link at it is
/// refused, not followed, and a FIFO there holds nothing up.
pub fn library_read(entry: Entry) -> Result<LibraryFile, String> {
    if let Entry::System(name) = &entry {
        luna_text(name)?;
    }
    read_bound(&Dirs::current(), &entry)
}

/// `entry`'s file with the identity of the library's folder it was read
/// from: the root opened once and held, its identity taken from that handle,
/// and the folders down to the file and the file opened relative to it (as
/// the save walks them, no link or junction among them), so the text and the
/// identity are of the same folder whatever a link at the root's path does
/// meanwhile, even back to where it led.
fn read_bound(dirs: &Dirs, entry: &Entry) -> Result<LibraryFile, String> {
    read_held(dirs, entry, || {})
}

/// `read_bound`, with `held` called once the root is held and before anything
/// is read through it: where a test moves the path from under the read.
fn read_held(dirs: &Dirs, entry: &Entry, held: impl FnOnce()) -> Result<LibraryFile, String> {
    use crate::held::Dir;
    use std::io::Read;
    let (folders, name) = place(entry, "")?;
    let system = matches!(entry, Entry::System(_));
    let mut path = dirs.root.clone();
    path.extend(&folders);
    path.push(&name);
    let (shown, root) = (path.to_string_lossy().into_owned(), dirs.root.to_string_lossy().into_owned());
    // Nothing there yet: an override starts from Luna's own text.
    let start = |root_id: String| -> Result<LibraryFile, String> {
        let text = match entry {
            Entry::System(name) => luna_text(name)?.to_string(),
            _ => String::new(),
        };
        Ok(LibraryFile { path: shown.clone(), text, exists: false, root: root.clone(), root_id, revision: None })
    };
    let at = |e: std::io::Error| format!("{}: {e}", dirs.root.display());
    // The root as it leads (it may be a link of the person's choosing); none
    // there is none read, the id empty.
    let mut dir = match Dir::open_followed(&dirs.root) {
        Ok(dir) => dir,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return start(String::new()),
        Err(e) => return Err(at(e)),
    };
    let root_id = dir.id().map_err(at)?;
    held();
    let mut at_folder = dirs.root.clone();
    for folder in &folders {
        at_folder.push(folder);
        match dir.child_dir(folder, false).map_err(|e| format!("{}: {e}", at_folder.display()))? {
            Some(child) => dir = child,
            None => return start(root_id),
        }
    }
    let Some(file) = dir.open_file(&name).map_err(|e| format!("{shown}: {e}"))? else { return start(root_id) };
    if system {
        // The person's own file (system.rs `read_own_file`).
        match crate::secret::own_file(&file) {
            Ok(Ok(())) => {}
            Ok(Err(why)) => return Err(format!("{shown} is not a file of yours ({why}); the override is your own")),
            Err(e) => return Err(format!("{shown}: {e}")),
        }
    }
    let mut bytes = Vec::new();
    file.take(EDIT_MAX_BYTES + 1).read_to_end(&mut bytes).map_err(|e| format!("{shown}: {e}"))?;
    if bytes.len() as u64 > EDIT_MAX_BYTES {
        return Err(format!("{shown} is over {} KB, too large to open here", EDIT_MAX_BYTES / 1024));
    }
    let revision = Some(sha256_hex(&bytes));
    let text = String::from_utf8(bytes).map_err(|_| format!("{shown} is not UTF-8 text"))?;
    Ok(LibraryFile { path: shown, text, exists: true, root, root_id, revision })
}

/// What a save bound to its source came to: saved, with the revision now on
/// disk, or refused for `conflict` with nothing written. Once saved, the
/// library is laid out: its overview, or why laying it out failed
/// (`layout_error`), which leaves the save as it is.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Written {
    pub conflict: Option<String>,
    pub revision: Option<String>,
    pub overview: Option<Overview>,
    pub layout_error: Option<String>,
}

/// Saves the editor's text only into the library it was read from (`root`,
/// its path, and `root_id`, the folder's identity) and over the file as it
/// was read (`revision`, None: none there), under the library's lock
/// (`save_held`): a file changed on disk since, or a library folder changed
/// meanwhile (another path, a link there retargeted, a folder made again),
/// is a `conflict`, nothing written, for the window to keep its text and
/// offer to reload or overwrite. `force` (Overwrite) lets a changed file go,
/// never another library. The native window's alone (native/panels.rs); an
/// addition beside `library_write`, which the page keeps using.
pub fn library_write_checked(
    entry: Entry,
    text: String,
    root: String,
    root_id: String,
    revision: Option<String>,
    force: bool,
) -> Result<Written, String> {
    if let Entry::System(name) = &entry {
        luna_text(name)?;
    }
    let _guard = lock()?;
    let dirs = Dirs::current();
    let revision = match save_held(&dirs, &entry, &text, &root, &root_id, revision.as_deref(), force)? {
        Ok(revision) => revision,
        Err((conflict, revision)) => return Ok(Written { conflict: Some(conflict), revision, overview: None, layout_error: None }),
    };
    if let Entry::System(name) = &entry {
        crate::log::info("library", &format!("system skill {name}: the override in {} saved", dirs.root.display()));
    }
    // Saved, whatever laying it out comes to: said apart.
    let (overview, layout_error) = match overview_locked(!matches!(entry, Entry::System(_))) {
        Ok(o) => (Some(o), None),
        Err(e) => (None, Some(e)),
    };
    Ok(Written { conflict: None, revision: Some(revision), overview, layout_error })
}

/// Where `entry`'s file lies under the library's root: its folders, then its
/// name; a system text's `text` held to the most it may hold.
fn place(entry: &Entry, text: &str) -> Result<(Vec<String>, String), String> {
    match entry {
        Entry::Skill(s) => {
            check_name(s)?;
            Ok((vec!["skills".into(), s.clone()], SKILL_FILE.into()))
        }
        Entry::Instructions(i) => {
            check_name(i)?;
            Ok((vec![INSTRUCTIONS_DIR.into()], format!("{i}.md")))
        }
        Entry::System(name) => system_place(name, text).map(|(dirs, file)| (dirs, file.into())),
    }
}

/// Why nothing was saved, and the revision on disk now.
type Conflict = (String, Option<String>);

/// The save itself, under the library's lock: the library's folder opened
/// and held to `root_id`, the folders down to the file opened or made
/// relative to it (no link or junction among them), the file's revision
/// checked through that handle, the text written to a new temp file beside
/// it, the revision checked again, and the temp renamed onto the file's
/// name relative to the same handle: in place of what is there where a file
/// was read (or `force`), else only where nothing is. The revision on disk
/// now, or why not.
///
/// A program that writes the file without Luna's lock can still change it
/// between the last check and the rename, and lose that change to the save:
/// only Luna's own writers take the lock.
fn save_held(
    dirs: &Dirs,
    entry: &Entry,
    text: &str,
    root: &str,
    root_id: &str,
    revision: Option<&str>,
    force: bool,
) -> Result<Result<String, Conflict>, String> {
    use crate::held::Dir;
    use std::io::Write;
    let now = dirs.root.to_string_lossy();
    if now != root {
        return Ok(Err((format!("The library's folder is {now} now; this text was read from {root}. Nothing was saved."), None)));
    }
    let (folders, name) = place(entry, text)?;
    let at = |e: std::io::Error| format!("{}: {e}", dirs.root.display());
    let elsewhere = || -> Result<Result<String, Conflict>, String> {
        Ok(Err((
            format!(
                "{root} is not the folder this text was read from: it was made again, or a link there leads elsewhere now. \
                 Nothing was saved."
            ),
            None,
        )))
    };
    let library = match Dir::open_followed(&dirs.root) {
        Ok(dir) if !root_id.is_empty() && dir.id().map_err(at)? == root_id => dir,
        Ok(_) => return elsewhere(),
        // None there when it was read, and none now: made here, as itself.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && root_id.is_empty() => {
            if let Some(parent) = dirs.root.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
            }
            match std::fs::create_dir(&dirs.root) {
                Ok(()) => Dir::open_itself(&dirs.root).map_err(at)?,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return elsewhere(),
                Err(e) => return Err(at(e)),
            }
        }
        Err(e) => return Err(at(e)),
    };
    let mut dir = library;
    let mut path = dirs.root.clone();
    for folder in &folders {
        path.push(folder);
        let shown = path.display().to_string();
        dir = dir.child_dir(folder, true).map_err(|e| format!("{shown}: {e}"))?.ok_or_else(|| format!("{shown} was not made"))?;
    }
    path.push(&name);
    let system = matches!(entry, Entry::System(_));
    // The file as it is through the held folder: anything but a plain file
    // refused, Overwrite or not, and for an override one of this user's
    // making (system.rs `read_own_file`).
    let on_disk = |dir: &Dir| -> Result<Option<String>, String> {
        use std::io::Read;
        let Some(file) = dir.open_file(&name).map_err(|e| format!("{}: {e}", path.display()))? else { return Ok(None) };
        if system {
            match crate::secret::own_file(&file) {
                Ok(Ok(())) => {}
                Ok(Err(why)) => return Err(format!("{} is not a file of yours ({why}); the override is your own", path.display())),
                Err(e) => return Err(format!("{}: {e}", path.display())),
            }
        }
        let mut bytes = Vec::new();
        file.take(EDIT_MAX_BYTES + 1).read_to_end(&mut bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Some(sha256_hex(&bytes)))
    };
    let changed = |now: Option<String>| -> Conflict {
        let why = match (&now, revision) {
            (None, _) => "The file was deleted on disk since it was opened. Nothing was saved.",
            (Some(_), None) => "A file was made at its name on disk since it was opened. Nothing was saved.",
            (Some(_), Some(_)) => "The file was changed on disk since it was opened. Nothing was saved.",
        };
        (why.into(), now)
    };
    let first = on_disk(&dir)?;
    if !force && first.as_deref() != revision {
        return Ok(Err(changed(first)));
    }
    let tmp = format!("{name}.{}{TMP_SUFFIX}", crate::secret::random_hex(8));
    // Private in the data dir's own `library/`, as `Dirs::write` has it; of
    // the folder's access in a library of the person's choosing.
    let mut file = dir.create_new(&tmp, dirs.root.starts_with(&dirs.data)).map_err(|e| format!("{}: {e}", path.display()))?;
    if let Err(e) = file.write_all(text.as_bytes()).and_then(|()| file.sync_all()) {
        dir.remove_file(&file, &tmp);
        return Err(format!("{}: {e}", path.display()));
    }
    // The last look, right before the file is replaced.
    let last = match on_disk(&dir) {
        Ok(last) => last,
        Err(e) => {
            dir.remove_file(&file, &tmp);
            return Err(e);
        }
    };
    if !force && last.as_deref() != revision {
        dir.remove_file(&file, &tmp);
        return Ok(Err(changed(last)));
    }
    let replace = force || revision.is_some();
    match dir.rename(&file, &tmp, &name, replace) {
        Ok(()) => Ok(Ok(sha256_hex(text.as_bytes()))),
        Err(e) => {
            dir.remove_file(&file, &tmp);
            if e.kind() == std::io::ErrorKind::AlreadyExists && !replace {
                return Ok(Err(changed(on_disk(&dir).unwrap_or(None))));
            }
            Err(format!("{}: {e}", path.display()))
        }
    }
}

/// Saves what the editor has, then lays it out. A skill whose SKILL.md does
/// not do is still saved — it is the user's draft — and laid out nowhere until
/// it does. A system skill's override is laid out nowhere: it is served.
pub fn library_write(entry: Entry, text: String) -> Result<Overview, String> {
    if let Entry::System(name) = &entry {
        luna_text(name)?;
        let _guard = lock()?;
        let path = write_system(&Dirs::current(), name, &text)?;
        crate::log::info("library", &format!("system skill {name}: the override at {} saved", path.display()));
        return overview_locked(false);
    }
    let _guard = lock()?;
    write_entry(&Dirs::current(), &entry, &text)?;
    overview_locked(true)
}

/// An instructions file or a skill's SKILL.md written where `library_path`
/// leads, over nothing but a plain file.
fn write_entry(dirs: &Dirs, entry: &Entry, text: &str) -> Result<(), String> {
    let path = library_path(dirs, entry)?;
    plain_or_none(&path)?;
    dirs.write(&path, text.as_bytes())
}

/// Why this text would not do as the override of the system text
/// `name`, so that Luna would serve its own in its place; None when it would.
pub fn library_check_system(name: String, text: String) -> Option<String> {
    luna_text(&name).err().or_else(|| check_override(&name, &text).err())
}

/// Luna's own text of a system skill and the library's override of it, to
/// compare.
pub fn library_system_compare(name: String) -> Result<SystemCompare, String> {
    luna_text(&name)?;
    compare_system(&Dirs::current(), &name)
}

/// Back to Luna's own text for a system skill: the override goes to Luna's
/// backups, or stays aside in `system/` where they would not take it whole,
/// which the overview names (`system_aside`).
pub fn library_system_reset(name: String) -> Result<Overview, String> {
    luna_text(&name)?;
    let _guard = lock()?;
    match reset_system(&Dirs::current(), &name)? {
        Some(Moved::Backup(saved)) => {
            crate::log::info("library", &format!("system skill {name}: back to Luna's own, the override kept in {}", saved.display()))
        }
        Some(Moved::Aside(kept)) => crate::log::warn(
            "library",
            &format!("system skill {name}: back to Luna's own, the override set aside at {}, not in the backups", kept.display()),
        ),
        None => {}
    }
    overview_locked(false)
}

pub fn library_new_skill(name: String) -> Result<Overview, String> {
    let name = name.trim().to_string();
    check_name(&name)?;
    let _guard = lock()?;
    let dirs = Dirs::current();
    let dir = in_library_dir(&dirs, &dirs.skills().join(&name))?;
    if std::fs::symlink_metadata(&dir).is_ok() {
        return Err(format!("The library already has a skill named {name}"));
    }
    let text = format!(
        "---\nname: {name}\ndescription: What this skill does and when to use it. The model reads only this line until it decides to.\n---\n\n# {name}\n\nThe steps, rules and examples to follow.\n"
    );
    dirs.write(&dir.join(SKILL_FILE), text.as_bytes())?;
    overview_locked(false)
}

/// Takes a skill out of the library — into the backups — and out of every
/// list; Luna's copies of it go too.
pub fn library_delete_skill(name: String) -> Result<Overview, String> {
    check_skill_name(&name)?;
    let _guard = lock()?;
    let dirs = Dirs::current();
    delete_skill_folder(&dirs, &name)?;
    crate::settings::update(|s| {
        s.library.skills.remove(&name);
        s.library.audience.skills.remove(&name);
        for f in &mut s.library.folders {
            f.skills.remove(&name);
        }
    })?;
    crate::log::info("library", &format!("skill {name} taken out of the library"));
    overview_locked(true)
}

/// Skill `from` renamed `to` in the settings: its reach, audience, and
/// what each folder says of it.
fn rename_skill(lib: &mut LibrarySettings, from: &str, to: &str) {
    if let Some(reach) = lib.skills.remove(from) {
        lib.skills.insert(to.to_string(), reach);
    }
    if let Some(audience) = lib.audience.skills.remove(from) {
        lib.audience.skills.insert(to.to_string(), audience);
    }
    for f in &mut lib.folders {
        if let Some(reach) = f.skills.remove(from) {
            f.skills.insert(to.to_string(), reach);
        }
    }
}

/// What a skill's rename came to, once it had begun to change anything.
#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Outcome {
    /// Folder, SKILL.md and settings renamed.
    Committed,
    /// A step failed and everything done before it was undone: as it was.
    RolledBack,
    /// A step failed and undoing what was done failed too: `why` says what
    /// is left where.
    RollbackFailed,
}

/// A skill's rename as it went, with the overview after it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Renamed {
    pub outcome: Outcome,
    pub why: Option<String>,
    pub overview: Option<Overview>,
}

/// The most of a SKILL.md a rename reads to rewrite its name.
const SKILL_MAX_BYTES: u64 = 1024 * 1024;

/// Renames a skill: its folder, the `name` of its SKILL.md where that named
/// it, and what the settings say of it. Refused, with nothing changed, where
/// the skill's folder or anything in it is a link, a junction or not a plain
/// file or folder (system.rs `inside`, `plain_tree`), where the new name is
/// taken, and where SKILL.md names it and could not be made to name the new
/// one. The folder moves without replacing (files.rs `rename_no_replace`):
/// a folder of the new name made meanwhile fails the move. Then SKILL.md and
/// the settings, each step undone if a later one fails (`RolledBack`); an
/// undo that fails is said as such (`RollbackFailed`), with what is left
/// where. Its copies go by the pass after: those of the old name, still as
/// Luna wrote them, are taken out, and the new name's laid out. The native
/// window's alone (native/panels.rs).
pub fn library_rename_skill(from: String, to: String) -> Result<Renamed, String> {
    let to = to.trim().to_string();
    check_name(&from)?;
    check_name(&to)?;
    let _guard = lock()?;
    if from == to {
        return Ok(Renamed { outcome: Outcome::Committed, why: None, overview: Some(overview_locked(false)?) });
    }
    let (outcome, why) = rename_skill_in(&Dirs::current(), &from, &to, |change| crate::settings::update(|s| change(&mut s.library)))?;
    match outcome {
        Outcome::Committed => crate::log::info("library", &format!("skill {from} renamed {to}")),
        Outcome::RolledBack => crate::log::warn("library", &format!("skill {from} not renamed {to}: {}", why.as_deref().unwrap_or(""))),
        Outcome::RollbackFailed => {
            crate::log::warn("library", &format!("skill {from} renamed {to} only partly: {}", why.as_deref().unwrap_or("")))
        }
    }
    Ok(Renamed { outcome, why, overview: overview_locked(true).ok() })
}

/// The rename's steps under `dirs`, the settings changed by `settings`.
/// Err: refused before anything changed.
fn rename_skill_in(
    dirs: &Dirs,
    from: &str,
    to: &str,
    settings: impl FnOnce(&dyn Fn(&mut LibrarySettings)) -> Result<(), String>,
) -> Result<(Outcome, Option<String>), String> {
    let old = inside(&dirs.root, &["skills", from])?;
    match std::fs::symlink_metadata(&old) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(format!("The library has no skill named {from}")),
        Err(e) => return Err(format!("{}: {e}", old.display())),
        Ok(m) if crate::boundary::is_link(&m) || !m.is_dir() => {
            return Err(format!("{} is a link or not a folder — Luna leaves it alone", old.display()));
        }
        Ok(_) => {}
    }
    plain_tree(&old)?;
    let new = inside(&dirs.root, &["skills", to])?;
    if std::fs::symlink_metadata(&new).is_ok() {
        return Err(format!("The library already has a skill named {to}"));
    }
    // Read as itself, before anything moves: a FIFO put in its place since
    // `plain_tree` is refused, not waited on with the lock held.
    let was = read_plain(&old.join(SKILL_FILE), SKILL_MAX_BYTES + 1)?;
    let rewritten = match was.as_deref() {
        None => None,
        Some(bytes) if bytes.len() as u64 > SKILL_MAX_BYTES => {
            return Err(format!("SKILL.md is over {} KB: nothing was renamed", SKILL_MAX_BYTES / 1024));
        }
        Some(bytes) => match std::str::from_utf8(bytes) {
            Ok(text) => renamed(text, from, to)?,
            Err(_) => return Err("SKILL.md is not UTF-8 text: nothing was renamed".into()),
        },
    };
    rename_no_replace(&old, &new).map_err(|e| format!("{} to {}: {e}", old.display(), new.display()))?;
    let file = new.join(SKILL_FILE);
    // Back as it was, as far as it goes: the file's old text, then the folder.
    let undo = |text: bool| -> Result<(), String> {
        if let (true, Some(bytes)) = (text, was.as_deref()) {
            dirs.write(&file, bytes).map_err(|e| format!("SKILL.md is left saying {to}: {e}"))?;
        }
        rename_no_replace(&new, &old).map_err(|e| format!("the folder is left at {}: {e}", new.display()))
    };
    let failed = |e: String, text: bool| match undo(text) {
        Ok(()) => Ok((Outcome::RolledBack, Some(e))),
        Err(left) => Ok((Outcome::RollbackFailed, Some(format!("{e}; and undoing it failed: {left}")))),
    };
    if let Some(text) = &rewritten {
        if let Err(e) = dirs.write(&file, text.as_bytes()) {
            return failed(e, false);
        }
    }
    if let Err(e) = settings(&|lib| rename_skill(lib, from, to)) {
        return failed(e, rewritten.is_some());
    }
    Ok((Outcome::Committed, None))
}

/// The library's folder of skill `name` into the backups and gone, if it is
/// there: refused, nothing touched, where it or the library's `skills` is a
/// link or junction (`deletable`).
fn delete_skill_folder(dirs: &Dirs, name: &str) -> Result<(), String> {
    let dir = dirs.skills().join(name);
    deletable(dirs, &dir)?;
    if std::fs::symlink_metadata(&dir).is_ok() {
        // All of it, a temp-named file of the person's too: the folder goes
        // whole.
        let files = read_dir_files_all(&dir)?;
        back_up(dirs, &dir, &files)?;
        std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    Ok(())
}

/// A library skill's folder fit to delete: neither it nor the library's
/// `skills` a link or junction, through which the deletion would take what
/// it leads to (an account's own skills, say).
fn deletable(dirs: &Dirs, dir: &Path) -> Result<(), String> {
    no_link_between(&dirs.root, dir).map_err(|e| format!("The skill was not deleted: {e}"))?;
    match std::fs::symlink_metadata(dir) {
        Ok(m) if crate::boundary::is_link(&m) => {
            Err(format!("The skill was not deleted: {} is a link or junction — Luna deletes nothing through it", dir.display()))
        }
        Ok(m) if !m.is_dir() => Err(format!("The skill was not deleted: {} is not a folder", dir.display())),
        _ => Ok(()),
    }
}

/// New instructions, blank and going nowhere until written and picked for an
/// account. A reach left in the settings by a file of the name deleted by
/// hand goes: new instructions start from none.
pub fn library_new_instructions(name: String) -> Result<Overview, String> {
    let name = name.trim().to_string();
    check_name(&name)?;
    let _guard = lock()?;
    let dirs = Dirs::current();
    let path = in_library_dir(&dirs, &dirs.instructions_file(&name))?;
    if std::fs::symlink_metadata(&path).is_ok() {
        return Err(format!("The library already has instructions named {name}"));
    }
    dirs.write(&path, b"")?;
    crate::settings::update(|s| forget_instructions(&mut s.library, &name))?;
    overview_locked(false)
}

/// What the settings keep of instructions `name` gone: its reach, its
/// audience, and its being laid into accounts for the chats (`laid_out`),
/// which only instructions an earlier Luna laid out carry; one made anew
/// under the name inherits none of it.
fn forget_instructions(lib: &mut LibrarySettings, name: &str) {
    lib.instructions.remove(name);
    lib.audience.instructions.remove(name);
    lib.laid_out.remove(name);
}

/// Instructions `from` renamed `to` in the settings: their reach, audience
/// and `laid_out` move with them, or the next pass would take for per session
/// what is laid into accounts and remove its copies; the folders that gave
/// them give them under the new name.
fn rename_instructions(lib: &mut LibrarySettings, from: &str, to: &str) {
    if let Some(reach) = lib.instructions.remove(from) {
        lib.instructions.insert(to.to_string(), reach);
    }
    if let Some(audience) = lib.audience.instructions.remove(from) {
        lib.audience.instructions.insert(to.to_string(), audience);
    }
    if lib.laid_out.remove(from) {
        lib.laid_out.insert(to.to_string());
    }
    for choice in lib.folders.iter_mut().flat_map(|f| f.instructions.values_mut()) {
        if choice.as_deref() == Some(from) {
            *choice = Some(to.to_string());
        }
    }
}

/// Renames instructions, their reach with them. Their copies stay as they
/// are: the ledger knows a copy by where it lies, not by what it came from.
pub fn library_rename_instructions(from: String, to: String) -> Result<Overview, String> {
    let to = to.trim().to_string();
    check_name(&from)?;
    check_name(&to)?;
    let _guard = lock()?;
    let dirs = Dirs::current();
    let (old, new) = (dirs.instructions_file(&from), dirs.instructions_file(&to));
    if from == to {
        return overview_locked(false);
    }
    if std::fs::symlink_metadata(&new).is_ok() {
        return Err(format!("The library already has instructions named {to}"));
    }
    std::fs::rename(&old, &new).map_err(|e| format!("{}: {e}", old.display()))?;
    // The file and its reach move together or not at all: the file renamed
    // without its reach would take every one of its copies out.
    let moved = crate::settings::update(|s| rename_instructions(&mut s.library, &from, &to));
    if let Err(e) = moved {
        let _ = std::fs::rename(&new, &old);
        return Err(e);
    }
    crate::log::info("library", &format!("instructions {from} renamed {to}"));
    overview_locked(true)
}

/// Takes instructions out of the library — into the backups — and out of the
/// settings. The accounts they went to get the next closest instructions, or
/// none; Luna's copies of them are replaced or go.
pub fn library_delete_instructions(name: String) -> Result<Overview, String> {
    check_name(&name)?;
    let _guard = lock()?;
    let dirs = Dirs::current();
    let path = dirs.instructions_file(&name);
    if let Some(files) = read_at(&path, true)? {
        back_up(&dirs, &path, &files)?;
        std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    crate::settings::update(|s| {
        forget_instructions(&mut s.library, &name);
        // A folder that gave them goes by what holds outside it again.
        for f in &mut s.library.folders {
            f.instructions.retain(|_, choice| choice.as_deref() != Some(name.as_str()));
        }
    })?;
    crate::log::info("library", &format!("instructions {name} taken out of the library"));
    overview_locked(true)
}

/// Opens the library's folder, or a skill's, in the file manager — for the
/// files beside SKILL.md, which the editor does not cover.
pub fn library_open(skill: Option<String>) -> Result<(), String> {
    let dirs = Dirs::current();
    let dir = match skill {
        Some(s) => {
            check_skill_name(&s)?;
            dirs.skills().join(s)
        }
        None => dirs.root,
    };
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program).arg(&dir).spawn().map(|_| ()).map_err(|e| format!("{program}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `skills/` or a skill's folder planted as a link to an account's: the
    /// editor's read and save are refused, and nothing is written there.
    #[test]
    fn the_editor_reads_and_writes_nothing_through_a_link() {
        let f = Fixture::new("editor-links");
        f.skill("review", "v1");
        let elsewhere = f.account(0).join("skills");
        std::fs::create_dir_all(elsewhere.join("taken")).unwrap();
        std::fs::write(elsewhere.join("taken").join(SKILL_FILE), "theirs").unwrap();
        // A skill's folder that is a link.
        link_dir(&f.dirs.skills().join("taken"), &elsewhere.join("taken"));
        let entry = Entry::Skill("taken".into());
        assert!(read_bound(&f.dirs, &entry).err().unwrap().contains("link"));
        assert!(write_entry(&f.dirs, &entry, "mine").unwrap_err().contains("link"));
        assert_eq!(std::fs::read_to_string(elsewhere.join("taken").join(SKILL_FILE)).unwrap(), "theirs");
        // A plain one beside it still reads and saves.
        let review = Entry::Skill("review".into());
        assert!(read_bound(&f.dirs, &review).unwrap().text.contains("v1"));
        write_entry(&f.dirs, &review, "v2").unwrap();
        // SKILL.md itself a link to a file: neither read nor replaced.
        #[cfg(unix)]
        {
            let md = f.dirs.skills().join("review").join(SKILL_FILE);
            std::fs::remove_file(&md).unwrap();
            std::os::unix::fs::symlink(elsewhere.join("taken").join(SKILL_FILE), &md).unwrap();
            assert!(read_bound(&f.dirs, &review).err().unwrap().contains("link"));
            assert!(write_entry(&f.dirs, &review, "mine").unwrap_err().contains("link"));
            assert_eq!(std::fs::read_to_string(elsewhere.join("taken").join(SKILL_FILE)).unwrap(), "theirs");
        }
        let _ = std::fs::remove_dir(f.dirs.skills().join("taken")).or_else(|_| std::fs::remove_file(f.dirs.skills().join("taken")));
    }

    /// A save is bound to the file as it was read and to the library it was
    /// read from: changed, made or deleted on disk since, or another library,
    /// it is refused and nothing is written; Overwrite lets a changed file
    /// go, never another library.
    #[test]
    fn a_save_holds_to_the_file_and_the_library_it_was_read_from() {
        let f = Fixture::new("editor-revision");
        f.skill("review", "v1");
        let entry = Entry::Skill("review".into());
        let read = read_bound(&f.dirs, &entry).unwrap();
        let (root, id) = (read.root.clone(), read.root_id.clone());
        assert_eq!(root, f.dirs.root.to_string_lossy());
        assert!(!id.is_empty());
        let file = f.dirs.skills().join("review").join(SKILL_FILE);
        let save = |entry: &Entry, text: &str, revision: Option<&str>, force: bool| {
            save_held(&f.dirs, entry, text, &root, &id, revision, force).unwrap()
        };
        // Changed by another program meanwhile.
        f.skill("review", "edited elsewhere");
        let theirs = std::fs::read_to_string(&file).unwrap();
        let refused = save(&entry, "mine", read.revision.as_deref(), false).unwrap_err();
        assert!(refused.0.contains("changed on disk"), "{}", refused.0);
        assert_eq!(refused.1, read_bound(&f.dirs, &entry).unwrap().revision);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), theirs);
        let saved = save(&entry, "mine", read.revision.as_deref(), true).expect("Overwrite");
        assert_eq!(
            (std::fs::read_to_string(&file).unwrap().as_str(), Some(saved)),
            ("mine", read_bound(&f.dirs, &entry).unwrap().revision)
        );
        // Another library, Overwrite or not.
        assert!(save_held(&f.dirs, &entry, "x", "/elsewhere", &id, None, true).unwrap().is_err());
        // A new file is saved only while none is there.
        let new = Entry::Instructions("fresh".into());
        assert_eq!(read_bound(&f.dirs, &new).unwrap().revision, None);
        f.instructions("fresh", "made meanwhile");
        assert!(save(&new, "mine", None, false).unwrap_err().0.contains("made at its name"));
        assert_eq!(std::fs::read_to_string(f.dirs.instructions_file("fresh")).unwrap(), "made meanwhile");
        // Deleted on disk.
        std::fs::remove_file(f.dirs.instructions_file("fresh")).unwrap();
        let rev = Some(sha256_hex(b"made meanwhile"));
        assert!(save(&new, "mine", rev.as_deref(), false).unwrap_err().0.contains("deleted"));
        save(&new, "mine", None, false).unwrap();
        assert_eq!(std::fs::read_to_string(f.dirs.instructions_file("fresh")).unwrap(), "mine");
        // No temp file is left beside either.
        let left = |dir: &Path| std::fs::read_dir(dir).unwrap().flatten().any(|e| e.file_name().to_string_lossy().ends_with(TMP_SUFFIX));
        assert!(!left(&f.dirs.instructions()) && !left(&f.dirs.skills().join("review")));
    }

    /// The library a link of the person's choosing, retargeted between the
    /// read and the save to another library holding the same file (the same
    /// revision), or its folder made again at the same path: the save is
    /// refused, Overwrite or not, and neither folder is written into.
    #[test]
    fn a_save_goes_into_no_folder_but_the_one_read_from() {
        let f = Fixture::new("editor-retarget");
        let (one, two, link) = (f.base.join("one"), f.base.join("two"), f.base.join("lib"));
        for lib in [&one, &two] {
            std::fs::create_dir_all(lib.join("skills").join("review")).unwrap();
            std::fs::write(lib.join("skills").join("review").join(SKILL_FILE), "v1").unwrap();
        }
        link_dir(&link, &one);
        let dirs = Dirs { root: link.clone(), data: f.dirs.data.clone() };
        let entry = Entry::Skill("review".into());
        let read = read_bound(&dirs, &entry).unwrap();
        let _ = std::fs::remove_dir(&link).or_else(|_| std::fs::remove_file(&link));
        link_dir(&link, &two);
        for force in [false, true] {
            let refused = save_held(&dirs, &entry, "mine", &read.root, &read.root_id, read.revision.as_deref(), force).unwrap();
            assert!(refused.unwrap_err().0.contains("not the folder"), "force {force}");
        }
        // The file's absence is bound too: a new one goes nowhere else either.
        let fresh = Entry::Instructions("fresh".into());
        let _ = std::fs::remove_dir(&link).or_else(|_| std::fs::remove_file(&link));
        link_dir(&link, &one);
        let none = read_bound(&dirs, &fresh).unwrap();
        let _ = std::fs::remove_dir(&link).or_else(|_| std::fs::remove_file(&link));
        link_dir(&link, &two);
        assert!(save_held(&dirs, &fresh, "mine", &none.root, &none.root_id, None, false).unwrap().is_err());
        for lib in [&one, &two] {
            assert_eq!(std::fs::read_to_string(lib.join("skills").join("review").join(SKILL_FILE)).unwrap(), "v1");
            assert!(!lib.join(INSTRUCTIONS_DIR).exists());
        }
        let _ = std::fs::remove_dir(&link).or_else(|_| std::fs::remove_file(&link));
        // Made again at the same path, with the same file in it.
        let dirs = Dirs { root: one.clone(), data: f.dirs.data.clone() };
        let read = read_bound(&dirs, &entry).unwrap();
        std::fs::rename(&one, f.base.join("aside")).unwrap();
        std::fs::create_dir_all(one.join("skills").join("review")).unwrap();
        std::fs::write(one.join("skills").join("review").join(SKILL_FILE), "v1").unwrap();
        let refused = save_held(&dirs, &entry, "mine", &read.root, &read.root_id, read.revision.as_deref(), false).unwrap();
        assert!(refused.unwrap_err().0.contains("not the folder"));
        assert_eq!(std::fs::read_to_string(one.join("skills").join("review").join(SKILL_FILE)).unwrap(), "v1");
        // The folder read from, though, takes it.
        let dirs = Dirs { root: f.base.join("aside"), data: f.dirs.data.clone() };
        let root = dirs.root.to_string_lossy().into_owned();
        save_held(&dirs, &entry, "mine", &root, &read.root_id, read.revision.as_deref(), false).unwrap().unwrap();
        assert_eq!(std::fs::read_to_string(dirs.skills().join("review").join(SKILL_FILE)).unwrap(), "mine");
    }

    /// The library a link of the person's choosing, retargeted to another
    /// library once the read holds the root (and, as a second case, back
    /// again): the text and the identity are of the folder held, never of the
    /// one the path leads to meanwhile.
    #[test]
    fn a_read_is_of_the_folder_held_whatever_the_path_does_meanwhile() {
        use crate::held::Dir;
        let f = Fixture::new("editor-read-retarget");
        let (one, two, link) = (f.base.join("one"), f.base.join("two"), f.base.join("lib"));
        for (lib, text) in [(&one, "from one"), (&two, "from two")] {
            std::fs::create_dir_all(lib.join("skills").join("review")).unwrap();
            std::fs::write(lib.join("skills").join("review").join(SKILL_FILE), text).unwrap();
        }
        let id_of = |p: &Path| Dir::open_followed(p).unwrap().id().unwrap();
        let relink = |to: &Path| {
            let _ = std::fs::remove_dir(&link).or_else(|_| std::fs::remove_file(&link));
            link_dir(&link, to);
        };
        link_dir(&link, &one);
        let dirs = Dirs { root: link.clone(), data: f.dirs.data.clone() };
        let entry = Entry::Skill("review".into());
        let read = read_held(&dirs, &entry, || relink(&two)).unwrap();
        assert_eq!((read.text.as_str(), read.root_id.as_str()), ("from one", id_of(&one).as_str()));
        assert_eq!(read.revision, Some(sha256_hex(b"from one")));
        // Away and back during the read: the same.
        relink(&one);
        let read = read_held(&dirs, &entry, || {
            relink(&two);
            relink(&one);
        })
        .unwrap();
        assert_eq!((read.text.as_str(), read.root_id.as_str()), ("from one", id_of(&one).as_str()));
        relink(&two);
        let read = read_held(&dirs, &entry, || relink(&one)).unwrap();
        assert_eq!((read.text.as_str(), read.root_id.as_str()), ("from two", id_of(&two).as_str()));
        let _ = std::fs::remove_dir(&link).or_else(|_| std::fs::remove_file(&link));
    }

    /// A save into a library that was not there when the file was read makes
    /// it; one made meanwhile by something else is not written into.
    #[test]
    fn a_library_not_there_when_read_is_made_by_the_save_alone() {
        let f = Fixture::new("editor-made");
        let dirs = Dirs { root: f.base.join("later"), data: f.dirs.data.clone() };
        let entry = Entry::Instructions("main".into());
        let read = read_bound(&dirs, &entry).unwrap();
        assert!(read.root_id.is_empty() && !read.exists);
        std::fs::create_dir_all(&dirs.root).unwrap();
        assert!(save_held(&dirs, &entry, "mine", &read.root, &read.root_id, None, true).unwrap().is_err());
        std::fs::remove_dir(&dirs.root).unwrap();
        save_held(&dirs, &entry, "mine", &read.root, &read.root_id, None, false).unwrap().unwrap();
        assert_eq!(std::fs::read_to_string(dirs.instructions_file("main")).unwrap(), "mine");
    }

    /// A rename moves the folder and the whole `name` of SKILL.md, folded
    /// value and all, and its left-out places; refused with nothing changed
    /// where the new name is taken, a link is in the skill, or the name
    /// cannot be rewritten; undone where the settings fail.
    #[test]
    fn a_skill_is_renamed_whole_or_put_back() {
        let f = Fixture::new("rename-skill");
        let skill = |name: &str, text: &str| write_file(&f.dirs.skills().join(name).join(SKILL_FILE), text.as_bytes()).unwrap();
        skill("review", "---\nname: >-\n  review\ndescription: Reviews.\n---\nBody\n");
        let mut lib = LibrarySettings { skills: skills(&["review"]), ..Default::default() };
        let ok = |lib: &mut LibrarySettings| {
            let mut held = std::mem::take(lib);
            let out = rename_skill_in(&f.dirs, "review", "check", |change| {
                change(&mut held);
                Ok(())
            });
            *lib = held;
            out
        };
        assert_eq!(ok(&mut lib).unwrap(), (Outcome::Committed, None));
        let text = std::fs::read_to_string(f.dirs.skills().join("check").join(SKILL_FILE)).unwrap();
        assert_eq!(front_matter(&text).unwrap().0.as_deref(), Some("check"), "{text}");
        assert!(!text.contains("  review"), "the folded value's lines go: {text}");
        assert!(!f.dirs.skills().join("review").exists());
        assert!(lib.skills.contains_key("check"));

        // The settings fail: the file and the folder are put back.
        let before = std::fs::read(f.dirs.skills().join("check").join(SKILL_FILE)).unwrap();
        let out = rename_skill_in(&f.dirs, "check", "audit", |_| Err("no settings".into())).unwrap();
        assert_eq!(out.0, Outcome::RolledBack);
        assert_eq!(std::fs::read(f.dirs.skills().join("check").join(SKILL_FILE)).unwrap(), before);
        assert!(!f.dirs.skills().join("audit").exists());

        // Taken: refused, nothing moves.
        skill("audit", "---\nname: audit\ndescription: Audits.\n---\n");
        assert!(rename_skill_in(&f.dirs, "check", "audit", |_| Ok(())).unwrap_err().contains("already"));
        // Two names in the front matter are rewritten together.
        skill("twice", "---\nname: twice\ndescription: x\nname: twice\n---\n");
        assert_eq!(rename_skill_in(&f.dirs, "twice", "once", |_| Ok(())).unwrap().0, Outcome::Committed);
        let text = std::fs::read_to_string(f.dirs.skills().join("once").join(SKILL_FILE)).unwrap();
        assert_eq!(front_matter(&text).unwrap().0.as_deref(), Some("once"), "{text}");
        // A link inside the skill: refused before anything moves.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(f.account(0), f.dirs.skills().join("audit").join("out")).unwrap();
            assert!(rename_skill_in(&f.dirs, "audit", "other", |_| Ok(())).unwrap_err().contains("link"));
            assert!(f.dirs.skills().join("audit").is_dir());
        }
    }

    /// The no-replace move fails on a name taken, file or folder, and leaves
    /// both as they were.
    #[test]
    fn a_move_never_replaces_what_is_there() {
        let f = Fixture::new("rename-no-replace");
        let (a, b) = (f.base.join("a"), f.base.join("b"));
        std::fs::create_dir_all(a.join("x")).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        assert!(rename_no_replace(&a, &b).is_err(), "an empty folder of the name is not replaced");
        assert!(a.join("x").is_dir());
        std::fs::remove_dir(&b).unwrap();
        std::fs::write(&b, "file").unwrap();
        assert!(rename_no_replace(&a, &b).is_err());
        std::fs::remove_file(&b).unwrap();
        rename_no_replace(&a, &b).unwrap();
        assert!(b.join("x").is_dir() && !a.exists());
    }

    /// A FIFO at an instructions file's name holds neither the read nor a
    /// save up, with the library's lock held around them.
    #[cfg(unix)]
    #[test]
    fn a_fifo_in_the_editor_s_place_holds_nothing_up() {
        let f = Fixture::new("editor-fifo");
        f.instructions("main", "Be brief.\n");
        let path = f.dirs.instructions_file("main");
        std::fs::remove_file(&path).unwrap();
        let c = std::ffi::CString::new(path.to_string_lossy().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let dirs = Dirs { root: f.dirs.root.clone(), data: f.dirs.data.clone() };
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let entry = Entry::Instructions("main".into());
            let _ = tx.send((read_bound(&dirs, &entry).err(), write_entry(&dirs, &entry, "x").err()));
        });
        let (read, write) = rx.recv_timeout(std::time::Duration::from_secs(5)).expect("answered without waiting on a FIFO");
        assert!(read.is_some_and(|e| e.contains("not a plain entry")));
        assert!(write.is_some_and(|e| e.contains("not a file")));
    }

    /// A start's pass says a place it could not lay out once, and again
    /// only when the reason changes.
    #[test]
    fn a_failure_at_a_start_is_logged_once_a_reason() {
        let dest = "E:/acc/test-failure-news/CLAUDE.md";
        assert!(failure_is_news(dest, "its backup would not write"));
        assert!(!failure_is_news(dest, "its backup would not write"));
        assert!(failure_is_news(dest, "a file held open"));
    }

    /// Deleting a skill from the library backs up all of its folder, a
    /// temp-named file in it too. Where the library's `skills`, or the
    /// skill's folder, is a link or junction (to an account's skills, say),
    /// nothing is deleted or backed up and the error says why.
    #[test]
    fn a_skill_is_deleted_only_from_the_librarys_own_folder() {
        let f = Fixture::new("delete-skill");
        f.skill("review", "v1");
        std::fs::write(f.dirs.skills().join("review").join("draft.luna-tmp"), "the person's").unwrap();
        delete_skill_folder(&f.dirs, "review").unwrap();
        assert!(!f.dirs.skills().join("review").exists());
        let backed = backed_up(&f.dirs);
        assert!(backed.iter().any(|b| b == "the person's") && backed.iter().any(|b| b.contains("v1")), "{backed:?}");

        // The library's `skills` a link to an account's own.
        let theirs = f.account(0).join("skills");
        write_file(&theirs.join("notes").join(SKILL_FILE), b"the account's own").unwrap();
        std::fs::remove_dir_all(f.dirs.skills()).unwrap();
        link_dir(&f.dirs.skills(), &theirs);
        let why = delete_skill_folder(&f.dirs, "notes").unwrap_err();
        assert!(why.contains("link or junction"), "{why}");
        assert!(theirs.join("notes").join(SKILL_FILE).is_file());
        unlink_dir(&f.dirs.skills());

        // The skill's own folder a link to it.
        std::fs::create_dir_all(f.dirs.skills()).unwrap();
        link_dir(&f.dirs.skills().join("notes"), &theirs.join("notes"));
        let why = delete_skill_folder(&f.dirs, "notes").unwrap_err();
        assert!(why.contains("link or junction"), "{why}");
        assert!(theirs.join("notes").join(SKILL_FILE).is_file());
        assert_eq!(backed_up(&f.dirs).len(), backed.len(), "nothing more backed up");
        unlink_dir(&f.dirs.skills().join("notes"));
    }

    /// Chats' instructions an earlier Luna laid into accounts, read as this
    /// one migrates them, then renamed: they stay laid out, worded, in a
    /// Claude Code and a Codex account alike; deleted, they are not laid out
    /// for whatever takes the name next.
    #[test]
    fn instructions_laid_out_before_stay_laid_out_through_a_rename() {
        let f = Fixture::new("laid-out-rename");
        f.instructions("main", "Speak Russian to me.\n");
        let old: crate::settings::Settings =
            serde_json::from_str(r#"{"library": {"instructions": true, "audience": {"instructions": {"main": "chats"}}}}"#).unwrap();
        let mut lib = old.library;
        assert!(lib.laid_out.contains("main"), "migrated as laid out");
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        let worded =
            |i: usize, file: &str| f.read(i, file).ends_with("Speak Russian to me.\n") && f.read(i, file).starts_with("> For chats");
        assert!(worded(0, "CLAUDE.md") && worded(1, "AGENTS.md"), "laid into both providers' accounts");

        std::fs::rename(f.dirs.instructions_file("main"), f.dirs.instructions_file("russian")).unwrap();
        rename_instructions(&mut lib, "main", "russian");
        assert!(lib.laid_out.contains("russian") && !lib.laid_out.contains("main"));
        let p = f.run(&lib, &mut ledger, true);
        assert!(worded(0, "CLAUDE.md") && worded(1, "AGENTS.md"), "still in both accounts: {:?}", p.places);

        forget_instructions(&mut lib, "russian");
        assert!(lib.laid_out.is_empty(), "nothing left for a new entry of the name to inherit");
    }

    /// A skill renamed keeps what the settings say of it under its new name,
    /// in the library and in each folder, and touches no other skill's.
    #[test]
    fn a_renamed_skill_keeps_its_reach_audience_and_folders() {
        let mut lib = LibrarySettings::default();
        lib.skills.insert("review".into(), vec![ALL.into()]);
        lib.skills.insert("other".into(), vec!["claude".into()]);
        lib.audience.skills.insert("review".into(), Audience::CHATS);
        let mut folder = LibraryFolder { path: "/work".into(), slug: "work".into(), ..Default::default() };
        folder.skills.insert("review".into(), vec![]);
        lib.folders.push(folder);
        rename_skill(&mut lib, "review", "audit");
        assert_eq!(lib.skills.get("audit"), Some(&vec![ALL.to_string()]));
        assert!(!lib.skills.contains_key("review"));
        assert_eq!(lib.skills.get("other"), Some(&vec!["claude".to_string()]));
        assert_eq!(lib.audience.skills.get("audit"), Some(&Audience::CHATS));
        assert_eq!(lib.folders[0].skills.get("audit"), Some(&vec![]));
        assert!(!lib.folders[0].skills.contains_key("review"));
        // Names the library refuses are refused before anything is touched.
        assert!(library_rename_skill("review".into(), "Bad Name".into()).is_err());
        assert!(library_rename_skill("../x".into(), "audit".into()).is_err());
    }

    /// With no lock to be had — a folder where the lock file goes, which opens
    /// on neither Windows nor Unix — every change is refused with nothing
    /// written, the overview still reads, and a session's start goes on
    /// without its pass, said in the log.
    #[test]
    fn without_the_lock_nothing_is_laid_out_but_the_overview_reads() {
        let d = std::env::temp_dir().join(format!("luna-library-no-lock-{}", std::process::id()));
        std::fs::create_dir_all(d.join("inside")).unwrap();
        LOCK_AT.with(|l| *l.borrow_mut() = Some(d.clone()));
        let dirs = Dirs::current();
        let ledger = std::fs::read(dirs.ledger()).ok();
        let settings = std::fs::read(crate::paths::data_dir().join("settings.json")).ok();

        let why = library_sync().err().expect("a sync is refused");
        assert!(why.starts_with("Nothing laid out: no lock on"), "{why}");
        assert!(why.contains("lock files"), "says why it matters: {why}");
        assert!(library_set_accounts("review".into(), vec![ALL.into()]).is_err());
        assert!(library_new_skill("no-lock-skill".into()).is_err());
        assert!(!dirs.skills().join("no-lock-skill").exists());
        assert_eq!(std::fs::read(dirs.ledger()).ok(), ledger, "the ledger is as it was");
        assert_eq!(std::fs::read(crate::paths::data_dir().join("settings.json")).ok(), settings, "and the settings");

        assert!(library_overview().is_ok(), "the overview still reads");
        let when = format!("in the no-lock test {}", std::process::id());
        lay_out(&when);
        let log = std::fs::read_to_string(crate::log::path()).unwrap_or_default();
        assert!(log.contains(&format!("laying the library out {when}: Nothing laid out")), "said in the log");

        LOCK_AT.with(|l| *l.borrow_mut() = None);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A start on either kind of account, waiting for the lock a rename holds:
    /// its account counts as in use while it waits, which a rename asking
    /// under the lock refuses for; and a rename that asked before it came
    /// and moved the folder has the start refused, not made on the old path.
    #[test]
    fn a_start_waiting_for_the_lock_holds_its_account_and_sees_it_moved() {
        for vendor in ["anthropic", "openai"] {
            let base = std::env::temp_dir().join(format!("luna-start-race-{vendor}-{}", std::process::id()));
            let account = base.join(vendor).join("work");
            std::fs::create_dir_all(&account).unwrap();
            let path = account.to_string_lossy().into_owned();
            let held = lock().unwrap();
            let start = {
                let path = path.clone();
                std::thread::spawn(move || lay_out_for("in the start race test", &path).map(drop))
            };
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !crate::account_use::running(&path) {
                assert!(std::time::Instant::now() < deadline, "{vendor}: the start counts its account while it waits");
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            std::fs::rename(&account, account.with_file_name("office")).unwrap();
            drop(held);
            let why = start.join().unwrap().unwrap_err();
            assert!(why.contains("renamed or deleted"), "{vendor}: {why}");
            assert!(!crate::account_use::running(&path), "{vendor}: and lets it go");
            let _ = std::fs::remove_dir_all(&base);
        }
    }

    /// A rename done before the start's pass began (the start paused in its
    /// worktree fetch, say): the folder is gone before `lay_out_for` looks,
    /// and the start is refused all the same, on either kind of account.
    #[test]
    fn a_start_whose_account_went_before_its_pass_is_refused() {
        STRICT.with(|s| s.set(true));
        for vendor in ["anthropic", "openai"] {
            let base = std::env::temp_dir().join(format!("luna-start-gone-{vendor}-{}", std::process::id()));
            let account = base.join(vendor).join("work");
            std::fs::create_dir_all(&account).unwrap();
            std::fs::rename(&account, account.with_file_name("office")).unwrap();
            let path = account.to_string_lossy().into_owned();
            let why = lay_out_for("in the gone-before test", &path).map(drop).unwrap_err();
            assert!(why.contains("renamed or deleted"), "{vendor}: {why}");
            assert!(!crate::account_use::running(&path), "{vendor}: nothing held");
            assert!(!account.exists(), "{vendor}: nothing made at the old path");
            let _ = std::fs::remove_dir_all(&base);
        }
        STRICT.with(|s| s.set(false));
    }
}
