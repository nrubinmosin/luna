//! The window's library commands: settings changed and laid out, an account's
//! own instructions taken in, and the library's own files read, written, made
//! and deleted.

use super::*;

/// What the library holds and where each copy stands, without writing.
#[tauri::command(async)]
pub fn library_overview() -> Result<Overview, String> {
    overview(false)
}

/// Lays the library out: everything that needs no asking.
#[tauri::command(async)]
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
#[tauri::command(async)]
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
#[tauri::command(async)]
pub fn library_set_instructions(name: Option<String>, accounts: Vec<String>) -> Result<Overview, String> {
    if let Some(n) = &name {
        check_name(n)?;
    }
    check_reach(&accounts)?;
    change(|l| give_instructions(l, name.as_deref(), accounts))
}

/// The accounts a skill goes into; empty = none.
#[tauri::command(async)]
pub fn library_set_accounts(skill: String, accounts: Vec<String>) -> Result<Overview, String> {
    check_name(&skill)?;
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
#[tauri::command(async)]
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
#[tauri::command(async)]
pub fn library_set_folder_skill(path: String, skill: String, accounts: Option<Vec<String>>) -> Result<Overview, String> {
    check_name(&skill)?;
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
#[tauri::command(async)]
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

/// Leaves a destination out of every pass, or takes it back in. Left out, it
/// is no longer Luna's copy: whatever is there stays, as anyone's file.
#[tauri::command(async)]
pub fn library_skip(dest: String, on: bool) -> Result<Overview, String> {
    let _guard = lock()?;
    let dirs = Dirs::current();
    let mut ledger = Ledger::load(&dirs);
    // Under any spelling of the path, as a pass goes by.
    let id = ident(Path::new(&dest));
    let noted = ledger.written.len();
    if on {
        ledger.written.retain(|k, _| ident(Path::new(k)) != id);
    }
    if ledger.written.len() != noted {
        ledger.save(&dirs)?;
    }
    crate::settings::update(|s| {
        s.library.skipped.retain(|d| ident(Path::new(d)) != id);
        if on {
            s.library.skipped.push(dest.clone());
        }
    })?;
    overview_locked(true)
}

/// Takes an account's own instructions file, where no instructions of the
/// library go, into the library as new instructions named after the account,
/// then lays the library out.
#[tauri::command(async)]
pub fn library_take_in(dest: String) -> Result<Overview, String> {
    let _guard = lock()?;
    let dirs = Dirs::current();
    let lib = crate::settings::get_checked()?.library;
    let accounts = crate::accounts::list_accounts()?;
    let mut ledger = Ledger::load(&dirs);
    take_in(&dirs, &lib, &accounts, &mut ledger, &dest)?;
    overview_locked(true)
}

/// `library_take_in` on these settings and accounts: refused where the
/// library lays something out at `dest` or it is no account's instructions
/// file.
fn take_in(dirs: &Dirs, lib: &LibrarySettings, accounts: &[AccountInfo], ledger: &mut Ledger, dest: &str) -> Result<(), String> {
    let id = ident(Path::new(dest));
    let units = units(dirs, lib, accounts, &read_skills(dirs), &read_instructions(dirs), ledger);
    if units.iter().any(|u| ident(&u.dest) == id) {
        return Err(format!("{dest} is a place the library lays a copy out to"));
    }
    let Some(a) = accounts.iter().find(|a| ident(&Path::new(&a.path).join(instructions_name(a.provider))) == id) else {
        return Err(format!("{dest} is no account's instructions file"));
    };
    if in_library(dirs, Path::new(dest)) {
        return Err(format!("{dest} is in the library itself"));
    }
    // Noted as the pass spells it.
    let dest = Path::new(&a.path).join(instructions_name(a.provider)).to_string_lossy().into_owned();
    take_unclaimed(dirs, ledger, &dest, a)
}

/// Makes an account's own instructions file new instructions in the library,
/// named after the account and going into it alone: the account's file
/// stays as it is, now Luna's copy, and whether the instructions go further
/// is picked in the window after.
fn take_unclaimed(dirs: &Dirs, ledger: &mut Ledger, dest: &str, a: &AccountInfo) -> Result<(), String> {
    let there = read_at(Path::new(dest), true)?.ok_or("Nothing is there to take")?;
    if there.values().all(|t| String::from_utf8_lossy(t).trim().is_empty()) {
        return Err(format!("{dest} is blank"));
    }
    let name = new_instructions_name(dirs, &a.name);
    write_at(&dirs.instructions_file(&name), true, &there)?;
    ledger.wrote(dest, hashes(&there));
    ledger.save(dirs)?;
    let key = format!("{}/{}", a.provider.as_str(), a.name);
    crate::settings::update(|s| give_instructions(&mut s.library, Some(&name), vec![key]))?;
    crate::log::info("library", &format!("took {dest} into the library as instructions {name}"));
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryFile {
    pub path: String,
    pub text: String,
    pub exists: bool,
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

fn library_path(dirs: &Dirs, entry: &Entry) -> Result<PathBuf, String> {
    match entry {
        Entry::Skill(s) => {
            check_name(s)?;
            Ok(dirs.skills().join(s).join(SKILL_FILE))
        }
        Entry::Instructions(i) => {
            check_name(i)?;
            Ok(dirs.instructions_file(i))
        }
        // Read and written by system.rs alone, which keeps to its own folder.
        Entry::System(name) => Err(format!("{name} is a system skill")),
    }
}

/// Instructions, a skill's SKILL.md or a system skill's override, for the
/// editor: where there is no override yet, Luna's own text to start one from.
#[tauri::command(async)]
pub fn library_read(entry: Entry) -> Result<LibraryFile, String> {
    if let Entry::System(name) = &entry {
        luna_text(name)?;
        return read_system(&Dirs::current(), name);
    }
    let path = library_path(&Dirs::current(), &entry)?;
    let (text, exists) = match std::fs::read_to_string(&path) {
        Ok(t) => (t, true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (String::new(), false),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    Ok(LibraryFile { path: path.to_string_lossy().into_owned(), text, exists })
}

/// Saves what the editor has, then lays it out. A skill whose SKILL.md does
/// not do is still saved — it is the user's draft — and laid out nowhere until
/// it does. A system skill's override is laid out nowhere: it is served.
#[tauri::command(async)]
pub fn library_write(entry: Entry, text: String) -> Result<Overview, String> {
    if let Entry::System(name) = &entry {
        luna_text(name)?;
        let _guard = lock()?;
        let path = write_system(&Dirs::current(), name, &text)?;
        crate::log::info("library", &format!("system skill {name}: the override at {} saved", path.display()));
        return overview_locked(false);
    }
    let _guard = lock()?;
    let dirs = Dirs::current();
    let path = library_path(&dirs, &entry)?;
    dirs.write(&path, text.as_bytes())?;
    overview_locked(true)
}

/// Why this SKILL.md would not be laid out, or None when it would.
#[tauri::command(async)]
pub fn library_check_skill(skill: String, text: String) -> Option<String> {
    if reserved(&skill) {
        return Some(reserved_why(&skill));
    }
    check_skill(&skill, &text).err()
}

/// Why this SKILL.md would not do as the override of the system skill
/// `name`, so that Luna would serve its own in its place; None when it would.
#[tauri::command(async)]
pub fn library_check_system(name: String, text: String) -> Option<String> {
    luna_text(&name).err().or_else(|| check_override(&name, &text).err())
}

/// Luna's own text of a system skill and the library's override of it, to
/// compare.
#[tauri::command(async)]
pub fn library_system_compare(name: String) -> Result<SystemCompare, String> {
    luna_text(&name)?;
    compare_system(&Dirs::current(), &name)
}

/// Back to Luna's own text for a system skill: the override goes to Luna's
/// backups, or stays aside in `system/` where they would not take it whole,
/// which the overview names (`system_aside`).
#[tauri::command(async)]
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

/// Makes the library skill of a system skill's name that system skill's
/// override, and takes it off every list: as a skill it was laid out nowhere
/// anyway (`reserved`).
#[tauri::command(async)]
pub fn library_system_adopt(skill: String) -> Result<Overview, String> {
    luna_text(&skill)?;
    let _guard = lock()?;
    adopt_system(&Dirs::current(), &skill)?;
    crate::settings::update(|s| {
        s.library.skills.remove(&skill);
        s.library.audience.skills.remove(&skill);
        for f in &mut s.library.folders {
            f.skills.remove(&skill);
        }
    })?;
    crate::log::info("library", &format!("skill {skill} made the system skill's override"));
    overview_locked(true)
}

#[tauri::command(async)]
pub fn library_new_skill(name: String) -> Result<Overview, String> {
    let name = name.trim().to_string();
    check_name(&name)?;
    if reserved(&name) {
        return Err(reserved_why(&name));
    }
    let _guard = lock()?;
    let dirs = Dirs::current();
    let dir = dirs.skills().join(&name);
    if dir.exists() {
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
#[tauri::command(async)]
pub fn library_delete_skill(name: String) -> Result<Overview, String> {
    check_name(&name)?;
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
#[tauri::command(async)]
pub fn library_new_instructions(name: String) -> Result<Overview, String> {
    let name = name.trim().to_string();
    check_name(&name)?;
    let _guard = lock()?;
    let dirs = Dirs::current();
    let path = dirs.instructions_file(&name);
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
#[tauri::command(async)]
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
#[tauri::command(async)]
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
#[tauri::command(async)]
pub fn library_open(skill: Option<String>) -> Result<(), String> {
    let dirs = Dirs::current();
    let dir = match skill {
        Some(s) => {
            check_name(&s)?;
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
        let dest = dirs.data.join("no-lock-account").join("CLAUDE.md").to_string_lossy().into_owned();
        assert!(library_skip(dest, true).is_err());
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

    /// Only an account's own instructions file where none of the library's
    /// go is taken in: one the library lays out to, or a file of no
    /// account's, is refused before anything is written.
    #[test]
    fn only_an_accounts_own_instructions_where_none_go_are_taken_in() {
        let f = Fixture::new("take-in");
        f.instructions("main", "Be brief.\n");
        let lib = LibrarySettings { instructions: first(reach(&["claude/work"])), ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        let spell = |p: PathBuf| p.to_string_lossy().into_owned();

        let why = take_in(&f.dirs, &lib, &f.accounts, &mut ledger, &spell(f.account(0).join("CLAUDE.md"))).unwrap_err();
        assert!(why.contains("lays a copy out"), "{why}");
        let why = take_in(&f.dirs, &lib, &f.accounts, &mut ledger, &spell(f.base.join("elsewhere").join("AGENTS.md"))).unwrap_err();
        assert!(why.contains("no account's"), "{why}");
        let why = take_in(&f.dirs, &lib, &f.accounts, &mut ledger, &spell(f.account(1).join("AGENTS.md"))).unwrap_err();
        assert!(why.contains("Nothing is there"), "{why}");
        assert_eq!(read_instructions(&f.dirs).unwrap().len(), 1, "nothing new in the library");
    }
}
