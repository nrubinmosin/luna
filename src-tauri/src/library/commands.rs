//! The window's library commands: settings changed and laid out, a changed copy
//! settled either way, and the library's own files read, written, made and
//! deleted.

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
    if let Err(e) = overview(true) {
        crate::log::warn("library", &format!("laying the library out {when}: {e}"));
    }
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

/// What a destination is to the library.
enum Located {
    /// A copy the library wants now: where it comes from, whether it is one
    /// file, the skill's name for a skill, what the copy is to hold — the
    /// library's, under the name it goes by there (`shown`) where that is
    /// another — or why the library has nothing fit for it.
    /// `banner`: the line saying whose the library's copy is (pass.rs `Unit`).
    /// `top`: the folder it lies in that no link may stand below (pass.rs `Unit`).
    Copy {
        from: PathBuf,
        top: PathBuf,
        file: bool,
        skill: Option<String>,
        shown: Option<String>,
        source: Result<Files, String>,
        banner: Audience,
    },
    /// An account's instructions file where no instructions of the library go.
    Unclaimed(AccountInfo),
}

fn locate(dirs: &Dirs, dest: &str) -> Result<Located, String> {
    let lib = crate::settings::get().library;
    let accounts = crate::accounts::list_accounts()?;
    let units = units(dirs, &lib, &accounts, &read_skills(dirs), &read_instructions(dirs), &Ledger::load(dirs));
    if let Some(u) = units.into_iter().find(|u| u.dest.to_string_lossy() == dest) {
        let from = match (&u.skill, &u.instructions) {
            (Some(s), _) => dirs.skills().join(s),
            (None, Some(i)) => dirs.instructions_file(i),
            (None, None) => return Err(format!("{dest} is laid out from nothing")),
        };
        return Ok(Located::Copy { from, top: u.top, file: u.file, skill: u.skill, shown: u.shown, source: u.source, banner: u.banner });
    }
    match accounts.into_iter().find(|a| Path::new(&a.path).join(instructions_name(a.provider)).to_string_lossy() == dest) {
        Some(a) => Ok(Located::Unclaimed(a)),
        None => Err(format!("{dest} is not a place the library is laid out to")),
    }
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
    keep_blobs(dirs, &there);
    ledger.wrote(dest, hashes(&there), Audience::EVERY);
    ledger.save(dirs)?;
    let key = format!("{}/{}", a.provider.as_str(), a.name);
    crate::settings::update(|s| give_instructions(&mut s.library, Some(&name), vec![key]))?;
    crate::log::info("library", &format!("took {dest} into the library as instructions {name}"));
    Ok(())
}

/// Settles a copy that changed, went or was never Luna's: `take` makes it the
/// library's (and so every other copy's), otherwise the library's goes back
/// in its place. Whichever side is replaced is backed up first. An account's
/// file where no instructions go is taken as new instructions of its own.
#[tauri::command(async)]
pub fn library_resolve(dest: String, take: bool) -> Result<Overview, String> {
    let _guard = lock()?;
    let dirs = Dirs::current();
    let located = locate(&dirs, &dest)?;
    let to = PathBuf::from(&dest);
    if in_library(&dirs, &to) {
        return Err(format!("{dest} is in the library itself"));
    }
    let mut ledger = Ledger::load(&dirs);
    let (from, top, file, skill, shown, source, banner) = match located {
        Located::Copy { from, top, file, skill, shown, source, banner } => (from, top, file, skill, shown, source, banner),
        Located::Unclaimed(a) if take => {
            take_unclaimed(&dirs, &mut ledger, &dest, &a)?;
            return overview_locked(true);
        }
        Located::Unclaimed(_) => return Err("No instructions of the library go there".into()),
    };
    if take {
        let there = read_at(&to, file)?.ok_or("Nothing is there to take")?;
        // A copy under another name goes back under the skill's own;
        // instructions without the line Luna wrote there saying whose they
        // are, as its ledger has it (`unworded`).
        let wrote = ledger.worded.get(&dest).copied().unwrap_or_default();
        let taken = match (&skill, &shown) {
            (Some(name), Some(_)) => with_name(there.clone(), name),
            (None, _) => there.iter().map(|(k, v)| (k.clone(), unworded(v, wrote))).collect(),
            _ => there.clone(),
        };
        // What stays there still has the line, unless it came off nothing.
        let still = if skill.is_none() && taken != there { wrote } else { Audience::EVERY };
        if let Some(name) = &skill {
            let text = taken.get(SKILL_FILE).ok_or("There is no SKILL.md there")?;
            check_skill(name, &String::from_utf8_lossy(text))?;
        }
        if let Some(old) = read_at(&from, file)? {
            back_up(&dirs, &from, &old)?;
        }
        write_at(&from, file, &taken)?;
        keep_blobs(&dirs, &there);
        ledger.wrote(&dest, hashes(&there), still);
        crate::log::info("library", &format!("took {dest} into the library"));
    } else {
        put_back(&dirs, &mut ledger, &top, &dest, file, shown.as_ref().or(skill.as_ref()), source, banner)?;
    }
    ledger.save(&dirs)?;
    overview_locked(true)
}

/// Writes the library's copy at `dest` over what is there, which is backed
/// up first. `name`: the skill's name there, None for instructions. Refused
/// where a folder between `top` and `dest` is a link (`no_link_between`).
#[allow(clippy::too_many_arguments)]
fn put_back(
    dirs: &Dirs,
    ledger: &mut Ledger,
    top: &Path,
    dest: &str,
    file: bool,
    name: Option<&String>,
    source: Result<Files, String>,
    banner: Audience,
) -> Result<(), String> {
    let to = Path::new(dest);
    no_link_between(top, to)?;
    let library = source.map_err(|e| format!("The library has nothing fit for it: {e}"))?;
    if let Some(name) = name {
        let text = library.get(SKILL_FILE).ok_or("The library's copy has no SKILL.md")?;
        check_skill(name, &String::from_utf8_lossy(text))?;
    }
    if let Some(old) = read_at(to, file)? {
        back_up(dirs, to, &old)?;
    }
    write_at(to, file, &library)?;
    keep_blobs(dirs, &library);
    ledger.wrote(dest, hashes(&library), banner);
    crate::log::info("library", &format!("put the library's copy back at {dest}"));
    Ok(())
}

/// Puts the library's copy back at each of `dests`, then lays the library out:
/// the window's way to make every account hold what the library says. Over
/// what is there only at the places in `replace`, the changed and foreign
/// copies the window's confirmation counted; elsewhere only where nothing is
/// there now: the window asked for deleted copies, and one made again since
/// it looked is not overwritten unasked. A place left out stays out; one that
/// will not settle is said in the overview's `unsettled` and the rest go on.
#[tauri::command(async)]
pub fn library_put_back(dests: Vec<String>, replace: Vec<String>) -> Result<Overview, String> {
    let _guard = lock()?;
    let dirs = Dirs::current();
    let lib = crate::settings::get_checked()?.library;
    let accounts = crate::accounts::list_accounts()?;
    let mut ledger = Ledger::load(&dirs);
    let units = units(&dirs, &lib, &accounts, &read_skills(&dirs), &read_instructions(&dirs), &ledger);
    let unsettled = put_back_all(&dirs, &units, &lib.skipped, &mut ledger, &dests, &replace);
    ledger.save(&dirs)?;
    for line in &unsettled {
        crate::log::warn("library", &format!("putting the library's copy back: {line}"));
    }
    Ok(Overview { unsettled, ..overview_locked(true)? })
}

/// Each of `dests` the library's copy again, as far as it goes, over what is
/// there only at a place in `replace`; what would not, a line each.
fn put_back_all(dirs: &Dirs, units: &[Unit], skipped: &[String], ledger: &mut Ledger, dests: &[String], replace: &[String]) -> Vec<String> {
    let mut unsettled = Vec::new();
    for dest in dests {
        let id = ident(Path::new(dest));
        let replace = replace.iter().any(|r| ident(Path::new(r)) == id);
        let done = match units.iter().find(|u| ident(&u.dest) == id) {
            _ if skipped.iter().any(|s| ident(Path::new(s)) == id) => Err("it is left out".to_string()),
            _ if in_library(dirs, Path::new(dest)) => Err("it is in the library itself".to_string()),
            None => Err("the library lays nothing out there".to_string()),
            Some(u) if !replace && matches!(read_at(&u.dest, u.file), Ok(Some(_))) => {
                Err("something is there again; it waits under Needs a look".to_string())
            }
            // Under the path as the pass spells it now, which the ledger
            // goes by, not as the window last saw it.
            Some(u) => put_back(
                dirs,
                ledger,
                &u.top,
                &u.dest.to_string_lossy(),
                u.file,
                u.shown.as_ref().or(u.skill.as_ref()),
                u.source.clone(),
                u.banner,
            ),
        };
        if let Err(e) = done {
            unsettled.push(format!("{dest}: {e}"));
        }
    }
    unsettled
}

/// Settles each of `dests`, copies Luna left in place as no longer wanted but
/// changed there (`Ledger::kept`), then lays the library out: `take_out`
/// backs what is there up and removes it, otherwise Luna forgets it and the
/// file stays as the user's own. A place left out, or wanted again, is not
/// touched; one that will not settle is said in the overview's `unsettled`
/// and the rest go on.
#[tauri::command(async)]
pub fn library_kept(dests: Vec<String>, take_out: bool) -> Result<Overview, String> {
    let _guard = lock()?;
    let dirs = Dirs::current();
    let lib = crate::settings::get_checked()?.library;
    let accounts = crate::accounts::list_accounts()?;
    let mut ledger = Ledger::load(&dirs);
    let units = units(&dirs, &lib, &accounts, &read_skills(&dirs), &read_instructions(&dirs), &ledger);
    let unsettled = settle_kept(&dirs, &accounts, &lib.folders, &units, &lib.skipped, &mut ledger, &dests, take_out);
    ledger.save(&dirs)?;
    for line in &unsettled {
        crate::log::warn("library", &format!("settling a copy left in place: {line}"));
    }
    Ok(Overview { unsettled, ..overview_locked(true)? })
}

/// Each of `dests` taken out (backed up first) or kept as the user's own, as
/// far as it goes; what would not, a line each.
#[allow(clippy::too_many_arguments)]
fn settle_kept(
    dirs: &Dirs,
    accounts: &[AccountInfo],
    folders: &[LibraryFolder],
    units: &[Unit],
    skipped: &[String],
    ledger: &mut Ledger,
    dests: &[String],
    take_out: bool,
) -> Vec<String> {
    let mut unsettled = Vec::new();
    for dest in dests {
        let id = ident(Path::new(dest));
        // Under the path as Luna noted it, which may be spelled otherwise.
        let key = ledger.kept.keys().find(|k| ident(Path::new(k)) == id).cloned();
        let done = match key {
            _ if skipped.iter().any(|s| ident(Path::new(s)) == id) => Err("it is left out".to_string()),
            _ if units.iter().any(|u| ident(&u.dest) == id) => Err("the library lays it out there again".to_string()),
            _ if in_library(dirs, Path::new(dest)) => Err("it is in the library itself".to_string()),
            None => Err("Luna keeps no note of it".to_string()),
            Some(key) if take_out => take_out_kept(dirs, accounts, folders, ledger, &key),
            Some(key) => {
                ledger.kept.remove(&key);
                crate::log::info("library", &format!("kept {key} as the user's own"));
                Ok(())
            }
        };
        if let Err(e) = done {
            unsettled.push(format!("{dest}: {e}"));
        }
    }
    unsettled
}

/// A kept copy out of its place, what is there backed up first; refused
/// where a folder on the way to it is a link (`no_link_between`), from the
/// account folder it lies in or, for a Claude Code folder copy, Luna's data
/// folder, or the listed folder a copy an earlier Luna laid into it lies in
/// (`top_of`).
fn take_out_kept(dirs: &Dirs, accounts: &[AccountInfo], folders: &[LibraryFolder], ledger: &mut Ledger, key: &str) -> Result<(), String> {
    let file = ledger.kept.get(key).is_some_and(is_file);
    let at = Path::new(key);
    let top = top_of(dirs, accounts, folders, at).ok_or("it lies in no account's folder nor a listed one")?;
    no_link_between(&top, at)?;
    if let Some(there) = read_at(at, file)? {
        back_up(dirs, at, &there)?;
        remove_at(at, file)?;
    }
    ledger.kept.remove(key);
    crate::log::info("library", &format!("took out {key}, no longer wanted there"));
    Ok(())
}

/// One file of a copy as it is in each of three places: what Luna wrote
/// there last, the library now, and there now. None where it is not.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDiff {
    pub path: String,
    pub base: Option<String>,
    pub library: Option<String>,
    pub there: Option<String>,
    /// Not text: shown as changed, not line by line.
    pub binary: bool,
}

#[tauri::command(async)]
pub fn library_diff(dest: String) -> Result<Vec<FileDiff>, String> {
    let _guard = lock_to_read();
    let dirs = Dirs::current();
    let ledger = Ledger::load(&dirs);
    let base_of = |h: &Hashes| -> Files { h.iter().filter_map(|(k, v)| Some((k.clone(), blob(&dirs, v)?))).collect() };
    let (base, library, file) = match ledger.kept.get(&dest) {
        // A copy left in place as no longer wanted: what changed there since
        // Luna wrote it, the library having nothing for that place now.
        Some(h) => (base_of(h), base_of(h), is_file(h)),
        None => {
            let Located::Copy { from, file, source, .. } = locate(&dirs, &dest)? else {
                return Err("No instructions of the library go there".into());
            };
            let base = ledger.written.get(&dest).map(base_of).unwrap_or_default();
            // As the copy is to be, under the name it goes by there; a skill
            // not fit to be laid out, as the library has it.
            let library = match source {
                Ok(files) => files,
                Err(_) => read_at(&from, file)?.unwrap_or_default(),
            };
            (base, library, file)
        }
    };
    let there = read_at(Path::new(&dest), file)?.unwrap_or_default();
    let paths: BTreeSet<&String> = base.keys().chain(library.keys()).chain(there.keys()).collect();
    let name = Path::new(&dest).file_name().unwrap_or_default().to_string_lossy().into_owned();
    Ok(paths
        .into_iter()
        .filter(|p| !(base.get(*p) == library.get(*p) && library.get(*p) == there.get(*p)))
        .map(|p| {
            let all = [base.get(p), library.get(p), there.get(p)];
            let binary = all.iter().flatten().any(|b| std::str::from_utf8(b).is_err());
            let text =
                |b: Option<&Vec<u8>>| if binary { b.map(|_| String::new()) } else { b.map(|b| String::from_utf8_lossy(b).into_owned()) };
            FileDiff {
                path: if p.is_empty() { name.clone() } else { p.clone() },
                base: text(all[0]),
                library: text(all[1]),
                there: text(all[2]),
                binary,
            }
        })
        .collect())
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
/// list; its copies go too, those still as Luna wrote them.
#[tauri::command(async)]
pub fn library_delete_skill(name: String) -> Result<Overview, String> {
    check_name(&name)?;
    let _guard = lock()?;
    let dirs = Dirs::current();
    let dir = dirs.skills().join(&name);
    if let Some(files) = read_at(&dir, false)? {
        back_up(&dirs, &dir, &files)?;
        std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
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
/// none; their copies are replaced or go, those still as Luna wrote them.
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

    /// Deleted, changed and foreign copies all become the library's again,
    /// the replaced ones backed up; a place left out or laid out to by
    /// nothing is said and the rest still go. Without `replace` only a
    /// deleted copy goes back.
    #[test]
    fn putting_back_makes_every_copy_the_librarys() {
        let f = Fixture::new("put-back");
        f.skill("review", "v1");
        f.instructions("main", "Be brief.\n");
        let taken = f.account(0).join("CLAUDE.md");
        std::fs::write(&taken, "Theirs.\n").unwrap();
        let skipped = f.account(2).join("CLAUDE.md");
        std::fs::write(&skipped, "Mine.\n").unwrap();
        let lib = LibrarySettings {
            instructions: first(all()),
            skills: skills(&["review"]),
            skipped: vec![skipped.to_string_lossy().into_owned()],
            ..Default::default()
        };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);

        let gone = f.account(0).join("skills/review");
        std::fs::remove_dir_all(&gone).unwrap();
        let changed = f.account(1).join("AGENTS.md");
        std::fs::write(&changed, "Be long.\n").unwrap();
        // The library moves on after: a copy put back is the library's now,
        // not what the ledger had.
        f.skill("review", "v2");
        let p = f.run(&lib, &mut ledger, true);
        assert_eq!(Fixture::status(&p, &gone), Status::Gone);
        assert_eq!(Fixture::status(&p, &changed), Status::Changed);
        assert_eq!(Fixture::status(&p, &taken), Status::Taken);

        let spell = |p: &Path| p.to_string_lossy().into_owned();
        let skills = read_skills(&f.dirs);
        let instructions = read_instructions(&f.dirs);
        let units = units(&f.dirs, &lib, &f.accounts, &skills, &instructions, &ledger);

        // Deleted ones only: what is there is not touched.
        let unsettled = put_back_all(&f.dirs, &units, &lib.skipped, &mut ledger, &[spell(&gone), spell(&changed)], &[]);
        assert_eq!(unsettled.len(), 1, "{unsettled:?}");
        assert!(unsettled[0].contains("something is there again"), "{unsettled:?}");
        assert!(f.read(0, "skills/review/SKILL.md").contains("v2"));
        assert_eq!(f.read(1, "AGENTS.md"), "Be long.\n");

        // Replaced only where asked: a deleted place remade meanwhile, not
        // among them, is not overwritten for being in the same batch.
        std::fs::write(
            gone.join("SKILL.md"),
            "Remade.
",
        )
        .unwrap();
        let nowhere = f.base.join("elsewhere/CLAUDE.md");
        let replace: Vec<String> = [&changed, &taken, &skipped, &nowhere].iter().map(|p| spell(p)).collect();
        let dests: Vec<String> = [spell(&gone)].into_iter().chain(replace.iter().cloned()).collect();
        let unsettled = put_back_all(&f.dirs, &units, &lib.skipped, &mut ledger, &dests, &replace);
        assert_eq!(unsettled.len(), 3, "{unsettled:?}");
        assert!(unsettled[0].contains("something is there again"), "{unsettled:?}");
        assert!(unsettled[1].ends_with("it is left out"), "{unsettled:?}");
        assert!(unsettled[2].ends_with("the library lays nothing out there"), "{unsettled:?}");
        assert_eq!(
            f.read(0, "skills/review/SKILL.md"),
            "Remade.
"
        );
        assert_eq!(f.read(2, "CLAUDE.md"), "Mine.\n", "a place left out is not touched");
        assert_eq!(f.read(1, "AGENTS.md"), "Be brief.\n");
        assert_eq!(f.read(0, "CLAUDE.md"), "Be brief.\n");
        let backed: Vec<String> = walk(&f.dirs.backups()).iter().filter_map(|p| std::fs::read_to_string(p).ok()).collect();
        assert!(backed.contains(&"Be long.\n".to_string()), "the changed copy is backed up: {backed:?}");
        assert!(backed.contains(&"Theirs.\n".to_string()), "the foreign file is backed up: {backed:?}");

        let p = f.run(&lib, &mut ledger, false);
        assert_eq!(Fixture::status(&p, &gone), Status::Changed);
        assert!(
            p.places.iter().filter(|x| Path::new(&x.dest) != gone).all(|x| matches!(x.status, Status::Current | Status::Skipped)),
            "{:?}",
            p.places
        );
    }

    /// A kept copy taken out goes to the backups first, as it is there; one
    /// kept as the user's own is forgotten and stays. A place left out, wanted
    /// again or never kept is said, and the rest still go.
    #[test]
    fn a_kept_copy_is_taken_out_or_kept() {
        let f = Fixture::new("kept-settle");
        f.skill("review", "v1");
        let lib = LibrarySettings { skills: skills(&["review"]), ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        let copies: Vec<PathBuf> = (0..3).map(|i| f.account(i).join("skills").join("review")).collect();
        for c in &copies {
            std::fs::write(c.join("extra.md"), "added there").unwrap();
        }
        // Wanted on the third account alone, which takes one copy off the kept.
        let lib = LibrarySettings { skills: BTreeMap::from([("review".into(), reach(&["claude/home"]))]), ..Default::default() };
        let p = f.run(&lib, &mut ledger, true);
        assert_eq!(p.kept.len(), 2, "{:?}", p.kept);
        let units = units(&f.dirs, &lib, &f.accounts, &read_skills(&f.dirs), &read_instructions(&f.dirs), &ledger);
        let spell = |p: &Path| p.to_string_lossy().into_owned();

        let nowhere = f.base.join("elsewhere/skills/review");
        let dests = [spell(&copies[0]), spell(&copies[2]), spell(&nowhere)];
        let unsettled = settle_kept(&f.dirs, &f.accounts, &lib.folders, &units, &lib.skipped, &mut ledger, &dests, true);
        assert_eq!(unsettled.len(), 2, "{unsettled:?}");
        assert!(unsettled[0].ends_with("the library lays it out there again"), "{unsettled:?}");
        assert!(unsettled[1].ends_with("Luna keeps no note of it"), "{unsettled:?}");
        assert!(!copies[0].exists(), "taken out");
        assert!(copies[2].join("extra.md").is_file(), "a copy wanted again is not taken out");
        let backed: Vec<String> = walk(&f.dirs.backups()).iter().filter_map(|p| std::fs::read_to_string(p).ok()).collect();
        assert!(backed.contains(&"added there".to_string()), "what was there is backed up: {backed:?}");
        assert!(backed.iter().any(|t| t.contains("v1")), "its SKILL.md too: {backed:?}");

        let unsettled = settle_kept(&f.dirs, &f.accounts, &lib.folders, &units, &lib.skipped, &mut ledger, &[spell(&copies[1])], false);
        assert!(unsettled.is_empty(), "{unsettled:?}");
        assert!(copies[1].join("extra.md").is_file(), "kept as the user's own");
        assert!(ledger.kept.is_empty());
        assert!(f.run(&lib, &mut ledger, true).kept.is_empty(), "and no longer said");
        assert!(copies[1].join("extra.md").is_file());
    }

    /// A changed copy an earlier Luna laid into a listed folder itself, kept
    /// as no longer wanted, can be taken out: the listed folder is its top.
    #[test]
    fn a_kept_old_copy_in_a_listed_folder_can_be_taken_out() {
        let f = Fixture::new("kept-old");
        f.skill("review", "v1");
        let repo = f.base.join("repo");
        let old = repo.join("review");
        let files = read_dir_files(&f.dirs.skills().join("review")).unwrap();
        write_at(&old, false, &files).unwrap();
        std::fs::write(old.join("extra.md"), "added there").unwrap();
        let mut ledger = Ledger::default();
        ledger.written.insert(old.to_string_lossy().into_owned(), hashes(&files));
        let folder = LibraryFolder { path: repo.to_string_lossy().into_owned(), ..Default::default() };
        let lib = LibrarySettings { folders: vec![folder], ..Default::default() };
        let p = f.run(&lib, &mut ledger, true);
        assert_eq!(p.kept, vec![old.to_string_lossy().into_owned()]);

        let units = units(&f.dirs, &lib, &f.accounts, &read_skills(&f.dirs), &read_instructions(&f.dirs), &ledger);
        let dests = [old.to_string_lossy().into_owned()];
        let unsettled = settle_kept(&f.dirs, &f.accounts, &lib.folders, &units, &[], &mut ledger, &dests, true);
        assert!(unsettled.is_empty(), "{unsettled:?}");
        assert!(!old.exists());
        let backed: Vec<String> = walk(&f.dirs.backups()).iter().filter_map(|p| std::fs::read_to_string(p).ok()).collect();
        assert!(backed.contains(&"added there".to_string()), "{backed:?}");
    }

    /// A kept copy under an account's `skills` folder made a link, that folder
    /// listed by the library as well: still not taken out through it.
    #[test]
    fn a_kept_copy_under_a_listed_linked_folder_is_not_taken_out() {
        let f = Fixture::new("kept-listed-link");
        f.skill("review", "v1");
        let lib = LibrarySettings { skills: skills(&["review"]), ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        let kept = f.account(0).join("skills").join("review");
        std::fs::write(kept.join("extra.md"), "added there").unwrap();
        let skills_dir = f.account(0).join("skills");
        let listed = LibraryFolder { path: skills_dir.to_string_lossy().into_owned(), slug: "x".into(), ..Default::default() };
        let lib = LibrarySettings { folders: vec![listed], ..Default::default() };
        assert_eq!(f.run(&lib, &mut ledger, true).kept.len(), 1);
        let elsewhere = f.base.join("elsewhere");
        std::fs::rename(&skills_dir, &elsewhere).unwrap();
        link_dir(&skills_dir, &elsewhere);

        let units = units(&f.dirs, &lib, &f.accounts, &read_skills(&f.dirs), &read_instructions(&f.dirs), &ledger);
        let dests = [kept.to_string_lossy().into_owned()];
        let unsettled = settle_kept(&f.dirs, &f.accounts, &lib.folders, &units, &[], &mut ledger, &dests, true);
        assert!(unsettled.len() == 1 && unsettled[0].contains("is a link or junction"), "{unsettled:?}");
        assert!(elsewhere.join("review").join("extra.md").is_file());
        assert!(walk(&f.dirs.backups()).is_empty());
        unlink_dir(&skills_dir);
    }

    /// A link standing for an account's `skills` folder (a junction on
    /// Windows) leads neither a take out nor a put-back over what is there
    /// into the folder it points at.
    #[test]
    fn a_linked_folder_above_a_copy_is_neither_written_nor_emptied() {
        let f = Fixture::new("linked");
        f.skill("review", "v1");
        let lib = LibrarySettings { skills: skills(&["review"]), ..Default::default() };
        let mut ledger = Ledger::default();
        f.run(&lib, &mut ledger, true);
        // Account 0's copy is kept (no longer wanted, changed there); then its
        // `skills` becomes a link to a folder of someone else's `review`.
        let only_others =
            LibrarySettings { skills: BTreeMap::from([("review".into(), reach(&["codex/home", "claude/home"]))]), ..Default::default() };
        let kept = f.account(0).join("skills").join("review");
        std::fs::write(kept.join("extra.md"), "added there").unwrap();
        assert_eq!(f.run(&only_others, &mut ledger, true).kept.len(), 1);
        let elsewhere = f.base.join("elsewhere");
        std::fs::create_dir_all(elsewhere.join("review")).unwrap();
        std::fs::write(elsewhere.join("review").join("SKILL.md"), "not Luna's").unwrap();
        std::fs::rename(f.account(0).join("skills"), f.base.join("moved-skills")).unwrap();
        link_dir(&f.account(0).join("skills"), &elsewhere);
        let spell = |p: &Path| p.to_string_lossy().into_owned();

        let away = units(&f.dirs, &only_others, &f.accounts, &read_skills(&f.dirs), &read_instructions(&f.dirs), &ledger);
        let unsettled = settle_kept(&f.dirs, &f.accounts, &[], &away, &[], &mut ledger, &[spell(&kept)], true);
        assert_eq!(unsettled.len(), 1, "{unsettled:?}");
        assert!(unsettled[0].contains("is a link or junction"), "{unsettled:?}");

        let wanted = units(&f.dirs, &lib, &f.accounts, &read_skills(&f.dirs), &read_instructions(&f.dirs), &ledger);
        let unsettled = put_back_all(&f.dirs, &wanted, &[], &mut ledger, &[spell(&kept)], &[spell(&kept)]);
        assert_eq!(unsettled.len(), 1, "{unsettled:?}");
        assert!(unsettled[0].contains("is a link or junction"), "{unsettled:?}");
        // Nor does a pass write through it.
        f.run(&lib, &mut ledger, true);

        assert_eq!(std::fs::read_to_string(elsewhere.join("review").join("SKILL.md")).unwrap(), "not Luna's");
        assert_eq!(walk(&elsewhere).len(), 1, "nothing written into the linked folder");
        assert!(walk(&f.dirs.backups()).is_empty(), "nothing backed up from it");
        unlink_dir(&f.account(0).join("skills"));
    }

    /// Every file under `dir`.
    fn walk(dir: &Path) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
        entries.flatten().flat_map(|e| if e.path().is_dir() { walk(&e.path()) } else { vec![e.path()] }).collect()
    }
}
