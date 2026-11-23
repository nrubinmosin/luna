//! Which project a folder belongs to, and the argument that asks a CLI for a
//! worktree. Luna makes, inspects and removes no worktree: each CLI makes its
//! own from `--worktree` (Claude Code under `<folder>/.claude/worktrees/<name>`
//! on a branch `worktree-<name>`, Codex under `<CODEX_HOME>/worktrees/<4 hex>/<repo>`
//! on a detached HEAD), and what is there afterwards is the user's. This reads
//! only where a session's folder is: a session in such a worktree belongs to
//! the project it was made from.

use crate::paths::{path_key as norm, KEY_SEP};
use std::ffi::{OsStr, OsString};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const GIT_RUN_FOR: Duration = Duration::from_secs(3);

fn git_executable() -> Option<&'static Path> {
    static GIT: OnceLock<Option<PathBuf>> = OnceLock::new();
    GIT.get_or_init(|| {
        let path = std::env::var_os("PATH")?;
        let name = if cfg!(windows) { "git.exe" } else { "git" };
        // Only absolute PATH entries. In particular, do not use Windows'
        // implicit executable search or interpret an empty entry as cwd.
        std::env::split_paths(&path).filter(|dir| dir.is_absolute()).map(|dir| dir.join(name)).find(|p| {
            let Ok(meta) = p.metadata() else { return false };
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                meta.is_file() && meta.permissions().mode() & 0o111 != 0
            }
            #[cfg(not(unix))]
            meta.is_file()
        })
    })
    .as_deref()
}

fn git_env_key(key: &OsStr) -> bool {
    let key = key.as_encoded_bytes();
    key.get(..4).is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"GIT_")) || key.eq_ignore_ascii_case(b"SSH_ASKPASS")
}

fn git_command(exe: &Path, inherited_keys: impl IntoIterator<Item = impl AsRef<OsStr>>) -> Command {
    let mut c = Command::new(exe);
    for key in inherited_keys {
        if git_env_key(key.as_ref()) {
            c.env_remove(key);
        }
    }
    // The transport allowlist is a second layer. Fetch can act on repo config
    // before reaching a transport check, so preventing lazy fetch is essential.
    c.env("GIT_ALLOW_PROTOCOL", "")
        .env("GIT_ASKPASS", "")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_NO_LAZY_FETCH", "1")
        .stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

fn git_on_path(exe: &Path) -> Command {
    git_command(exe, std::env::vars_os().map(|(key, _)| key))
}

fn git() -> Option<Command> {
    git_executable().map(git_on_path)
}

/// A per-query Git directory under Luna's private core directory. There is no
/// index, object data, project configuration, or path back to the real gitdir.
struct IgnoreRepo(PathBuf);

impl IgnoreRepo {
    fn new() -> io::Result<Self> {
        let core = crate::ipc::prepare_core_dir()?;
        if !core.is_absolute() {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let path = core.join(format!("ignore-{}", crate::secret::random_hex(16)));
        std::fs::create_dir(&path)?;
        let repo = Self(path);
        for dir in ["objects", "refs", "info"] {
            std::fs::create_dir(repo.0.join(dir))?;
        }
        std::fs::write(repo.0.join("HEAD"), b"ref: refs/heads/luna-ignore\n")?;
        std::fs::write(
            repo.0.join("config"),
            format!("[core]\nrepositoryformatversion = 0\nignoreCase = {}\n", cfg!(any(windows, target_os = "macos"))),
        )?;
        Ok(repo)
    }

    fn command(&self, mut command: Command) -> Command {
        let mut arg = OsString::from("--git-dir=");
        arg.push(&self.0);
        command.current_dir(&self.0).args([OsStr::new("--no-pager"), &arg]);
        command
    }

    fn isolated(&self, command: Command) -> Command {
        let mut command = self.command(command);
        // Git before 2.32 ignores GIT_CONFIG_GLOBAL. These homes stay private
        // even when -C changes the working directory to the checkout.
        command
            .env("HOME", &self.0)
            .env("XDG_CONFIG_HOME", &self.0)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.0.join("no-global-config"));
        #[cfg(windows)]
        command.env("USERPROFILE", &self.0).env("APPDATA", &self.0);
        command
    }

    fn output(&self, mut command: Command, deadline: Instant) -> Option<Output> {
        if Instant::now() >= deadline {
            return None;
        }
        // Git's builtins need only two small outputs. Spool them privately
        // so neither pipe backpressure nor a blocking read defeats the deadline.
        let stdout = self.0.join("stdout");
        let stderr = self.0.join("stderr");
        command.stdout(std::fs::File::create(&stdout).ok()?).stderr(std::fs::File::create(&stderr).ok()?);
        // On Windows, Git's PATH executable can be a launcher for the
        // actual Git process. End both on a deadline.
        let mut tree = crate::cli::ProbeTree::spawn(command).ok()?;
        let status = loop {
            match tree.try_finish() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
                _ => {
                    tree.end();
                    return None;
                }
            }
        };
        let stdout = bounded_bytes(std::fs::File::open(stdout).ok()?, crate::boundary::SETTINGS_MAX).ok()?;
        let stderr = bounded_bytes(std::fs::File::open(stderr).ok()?, crate::boundary::SETTINGS_MAX).ok()?;
        Some(Output { status, stdout, stderr })
    }
}

/// Called after the new core holds core.lock, before it serves requests.
pub(crate) fn sweep_ignore_repos(core: &Path) {
    let Ok(entries) = std::fs::read_dir(core) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(id) = name.to_str().and_then(|name| name.strip_prefix("ignore-")) else { continue };
        if id.len() == 32
            && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            && std::fs::symlink_metadata(entry.path()).is_ok_and(|m| m.is_dir() && !crate::boundary::is_link(&m))
        {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

impl Drop for IgnoreRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Read one checked global config at a time. Git parses a private snapshot
/// with includes disabled. Only unconditional include paths are followed.
struct GlobalConfig<'a> {
    repo: &'a IgnoreRepo,
    worktree: &'a Path,
    home: &'a Path,
    git: &'a dyn Fn() -> Option<Command>,
    remaining: u8,
    excludes: Option<PathBuf>,
}

impl GlobalConfig<'_> {
    fn scan(&mut self, path: &Path, depth: u8) -> Option<()> {
        if depth > 10 {
            return None;
        }
        self.remaining = self.remaining.checked_sub(1)?;
        let resolved = match resolve_chosen_path(path, true, |link| !inside_checkout(link, self.worktree)) {
            Ok(path) => path,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Some(()),
            Err(_) => return None,
        };
        if inside_checkout(&resolved, self.worktree) {
            return None;
        }
        let bytes = bounded_bytes(regular_file(&resolved).ok()?, crate::boundary::SETTINGS_MAX).ok()?;
        let snapshot = self.repo.0.join("global-config");
        std::fs::write(&snapshot, bytes).ok()?;
        let mut command = self.repo.isolated((self.git)()?);
        command
            .args(["config", "--no-includes", "--null", "--path", "--file"])
            .arg(snapshot)
            .args(["--get-regexp", r"^(core\.excludesfile|include\.path)$"]);
        let out = self.repo.output(command, Instant::now() + GIT_RUN_FOR)?;
        if !out.stderr.is_empty() || !matches!(out.status.code(), Some(0 | 1)) {
            return None;
        }
        // Process entries in Git's order, so that a later value overrides
        // an earlier value, whether it came from an include or the root.
        for entry in std::str::from_utf8(&out.stdout).ok()?.split_terminator('\0') {
            let (key, value) = entry.split_once('\n')?;
            let value = Path::new(value);
            // Git may read Trace2 settings before it sees --no-includes.
            // Keep its home private and translate its ~/ expansion here.
            let value = value.strip_prefix(&self.repo.0).map_or_else(|_| value.to_owned(), |suffix| self.home.join(suffix));
            if key == "include.path" {
                let included = configured_path(path.parent()?, &value)?;
                self.scan(&included, depth + 1)?;
            } else {
                self.excludes = Some(value);
            }
        }
        Some(())
    }
}

/// Path ancestors already checked by the resolver. Include each directory's
/// identity so an alternate spelling of the checkout cannot authorize a link.
fn inside_checkout(path: &Path, worktree: &Path) -> bool {
    path.ancestors()
        .skip(1)
        .any(|parent| parent == worktree || parent.to_str().zip(worktree.to_str()).is_some_and(|(a, b)| same_place(a, b)))
}

fn global_exclude(repo: &IgnoreRepo, worktree: &Path, git: &impl Fn() -> Option<Command>) -> Option<Vec<u8>> {
    let first = git()?;
    if std::env::var_os("GIT_CONFIG_GLOBAL").is_some()
        || first.get_envs().any(|(key, val)| key.eq_ignore_ascii_case("GIT_CONFIG_GLOBAL") && val.is_some())
    {
        return None;
    }
    let env = |key: &str| {
        first
            .get_envs()
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
            .map_or_else(|| std::env::var_os(key), |(_, value)| value.map(OsStr::to_owned))
            .filter(|value| !value.is_empty())
    };
    let home = env("HOME").map(PathBuf::from);
    #[cfg(windows)]
    let home = match home {
        Some(home) => Some(home),
        None => {
            let drive_home = match (env("HOMEDRIVE"), env("HOMEPATH")) {
                (Some(mut drive), Some(path)) => {
                    drive.push(path);
                    let path = registration_absolute(Path::new(&drive))?;
                    match resolve_chosen_path(&path, false, |_| true) {
                        Ok(_) => Some(path),
                        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
                        Err(_) => return None,
                    }
                }
                _ => None,
            };
            drive_home.or_else(|| env("USERPROFILE").map(PathBuf::from)).or_else(dirs::home_dir)
        }
    };
    // Anchor relative environment paths to the private cwd, never the project.
    let anchor = |path: PathBuf| registration_absolute(&repo.0.join(path).components().collect::<PathBuf>());
    let home = anchor(home.unwrap_or_default())?;
    let xdg = anchor(env("XDG_CONFIG_HOME").map_or_else(|| home.join(".config"), PathBuf::from))?.join("git");
    let mut config = GlobalConfig { repo, worktree, home: &home, git, remaining: 32, excludes: None };
    config.scan(&xdg.join("config"), 0)?;
    config.scan(&home.join(".gitconfig"), 0)?;

    let file = match config.excludes.as_deref() {
        Some(path) if !path.is_absolute() => return optional_exclude(&configured_path(worktree, path)?),
        Some(path) => registration_absolute(path)?,
        None => xdg.join("ignore"),
    };
    match resolve_chosen_path(&file, true, |link| !inside_checkout(link, worktree)) {
        Ok(file) => optional_exclude(&file),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Some(Vec::new()),
        Err(_) => None,
    }
}

fn optional_exclude(path: &Path) -> Option<Vec<u8>> {
    // Git ignores a missing excludes file. A link, special file, or read
    // failure is indeterminate. Never open a FIFO to find out what it is.
    match exclude_file(path).and_then(|file| bounded_bytes(file, crate::boundary::SETTINGS_MAX)) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Some(Vec::new()),
        Err(_) => None,
    }
}

fn exclude_file(path: &Path) -> io::Result<std::fs::File> {
    let path = registration_absolute(path).ok_or(io::ErrorKind::InvalidInput)?;
    check_registration_dirs(path.parent().ok_or(io::ErrorKind::InvalidInput)?)?;
    regular_file(&path)
}

fn readable_ignore_file(dir: &Path) -> bool {
    const GIT_PATTERN_MAX: u64 = 100 * 1024 * 1024;
    // The enclosing directory walk has already checked the ancestors.
    match regular_file(&dir.join(".gitignore")).and_then(|file| file.metadata()) {
        Ok(meta) => meta.len() <= GIT_PATTERN_MAX,
        Err(e) => e.kind() == io::ErrorKind::NotFound,
    }
}

fn ignored_by_git(worktree: &Path, common: &Path, rest: &str, git: impl Fn() -> Option<Command>) -> Option<bool> {
    let command = git()?;
    let exclude = optional_exclude(&common.join("info").join("exclude"))?;
    let repo = IgnoreRepo::new().ok()?;
    let global = global_exclude(&repo, worktree, &git)?;
    std::fs::write(repo.0.join("info").join("exclude"), exclude).ok()?;
    let copied = repo.0.join("global-exclude");
    std::fs::write(&copied, global).ok()?;
    let mut excludes_file = OsString::from("core.excludesFile=");
    excludes_file.push(copied);
    let mut work_tree = OsString::from("--work-tree=");
    work_tree.push(worktree);
    let mut c = repo.isolated(command);
    // The query reads only Luna's config. The user's exclude rules are data
    // in a separate file so Git keeps their precedence below info/exclude.
    c.arg("-c").arg(excludes_file).arg(work_tree).arg("-C").arg(worktree);
    // check-ignore rejects --literal-pathspecs. The leading "./" keeps a
    // colon at the start of a folder's name from introducing magic.
    let rest = if cfg!(windows) { rest.replace('\\', "/") } else { rest.to_owned() };
    c.args(["check-ignore", "-q", "--", &format!("./{rest}")]);
    ignore_result(repo.output(c, Instant::now() + GIT_RUN_FOR)?)
}

fn ignore_result(out: Output) -> Option<bool> {
    if !out.stderr.is_empty() {
        return None;
    }
    match out.status.code()? {
        0 => Some(true),
        1 => Some(false),
        _ => None,
    }
}

/// A name for `--worktree` that is one and can only be read as one: what Claude Code
/// takes (it refuses past 64 characters), and no leading dash. As a separate argument
/// `--dangerously-skip-permissions` is that flag, which turns permissions off.
pub fn name_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with(['-', '.'])
        && name.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

/// The argument that asks Claude Code for a worktree: that name if it will do, always
/// with `=` so that nothing after the flag is taken for it, else one of its own.
pub fn worktree_arg(name: Option<&str>) -> String {
    match name.filter(|n| name_ok(n)) {
        Some(name) => format!("--worktree={name}"),
        None => "--worktree".to_string(),
    }
}

fn claude_worktrees(folder: &str) -> PathBuf {
    Path::new(folder).join(".claude").join("worktrees")
}

/// Whether the path has a `.` or `..` in it: the text it reads as is not
/// where it leads.
fn has_dot_parts(path: &str) -> bool {
    path.split(['\\', '/']).any(|c| c == "." || c == "..")
}

fn under_claude_worktrees(folder: &str, candidate: &str) -> bool {
    if has_dot_parts(candidate) {
        return false;
    }
    let (c, base) = (norm(candidate), norm(&claude_worktrees(folder).to_string_lossy()));
    // One folder past a separator, as Claude Code names them:
    // `.claude\worktrees-old\x` is no worktree of the folder, and
    // `.claude\worktrees\x\src` only a part of one.
    c.strip_prefix(&base).and_then(|rest| rest.strip_prefix(KEY_SEP)).is_some_and(|name| !name.is_empty() && !name.contains(KEY_SEP))
}

/// Shaped like a worktree Codex made: `…\worktrees\<4 hex>\<repo>`.
pub fn codex_shaped(path: &str) -> bool {
    let p = Path::new(path.trim_end_matches(['\\', '/']));
    let bucket = p.parent();
    let root = bucket.and_then(Path::parent);
    let name = |q: Option<&Path>| q.and_then(Path::file_name).map(|n| n.to_string_lossy().into_owned());
    p.file_name().is_some()
        && name(bucket).is_some_and(|b| b.len() == 4 && b.chars().all(|c| c.is_ascii_hexdigit()))
        && name(root).is_some_and(|r| r.eq_ignore_ascii_case("worktrees"))
}

/// `codex_shaped`, and in the `worktrees` folder of a Codex account Luna runs
/// sessions on (`<accounts root>/openai/<account>`, its `CODEX_HOME`), which
/// is where Codex puts what `--worktree` makes (seen on 0.167.0 and 0.169.0:
/// `$CODEX_HOME/worktrees/<4 hex>/<repo>`, detached). That shape anywhere
/// else is a folder nobody can show Codex made.
fn codex_made(path: &str) -> bool {
    !has_dot_parts(path)
        && codex_shaped(path)
        && Path::new(path.trim_end_matches(['\\', '/'])).ancestors().nth(4).is_some_and(|accounts| {
            let accounts = accounts.to_string_lossy();
            #[cfg(test)]
            if TEST_CODEX_ACCOUNTS.lock().unwrap().iter().any(|a| same_place(a, &accounts)) {
                return true;
            }
            crate::accounts::provider_root(crate::provider::Provider::Codex)
                .is_ok_and(|root| same_place(&root.to_string_lossy(), &accounts))
        })
}

/// Folders the tests' fake Codex accounts are in, taken as the accounts root's.
#[cfg(test)]
static TEST_CODEX_ACCOUNTS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// <repo>/.git/worktrees/<id> → <repo>/.git. A submodule points at .git/modules.
fn worktree_common_dir(gitdir: &Path) -> Option<&Path> {
    gitdir.file_name()?;
    let worktrees = gitdir.parent()?;
    let dot_git = worktrees.parent()?;
    (worktrees.file_name()? == "worktrees" && dot_git.file_name()? == ".git").then_some(dot_git)
}

fn gitdir_from_marker(text: &str) -> Option<PathBuf> {
    let path = text.trim().strip_prefix("gitdir:")?.trim();
    // A trailing separator can make symlink_metadata follow the final link.
    Some(PathBuf::from(path.trim_end_matches(std::path::is_separator)))
}

/// Codex 0.171 checks the registration in the main repo before it accepts
/// that repo's trust in a linked checkout. A .git pointer alone cannot grant it.
pub(crate) fn registered_repo_of_linked(worktree: &str) -> Option<String> {
    registered_repo_with_roots(worktree, &RootAliases::default())
}

fn registered_repo_with_roots(worktree: &str, roots: &RootAliases) -> Option<String> {
    let worktree = registration_absolute(Path::new(worktree))?;
    let worktree = roots.translate(&worktree);
    check_registration_dirs(&worktree).ok()?;
    registered_repo_from_worktree(&worktree, roots)
}

fn registered_repo_from_worktree(worktree: &Path, roots: &RootAliases) -> Option<String> {
    let text = read_registration_marker(&worktree.join(".git"))?;
    let pointer = registration_path(worktree, &gitdir_from_marker(&text)?)?;
    worktree_common_dir(&pointer)?;
    let gitdir = roots.translate(&pointer);
    let main_git = worktree_common_dir(&gitdir)?;
    check_registration_dirs(&gitdir).ok()?;

    let common = read_registration_marker(&gitdir.join("commondir"))?;
    let common = roots.translate(&registration_path(&gitdir, Path::new(common.trim()))?);
    if common.file_name()? != ".git" || !same_plain_place(&common, main_git) {
        return None;
    }
    let back = read_registration_marker(&gitdir.join("gitdir"))?;
    let back = roots.translate(&registration_path(&gitdir, Path::new(back.trim()))?);
    let checkout = back.parent()?;
    if back.file_name()? != ".git" {
        return None;
    }
    if !same_plain_place(checkout, worktree) {
        return None;
    }
    Some(main_git.parent()?.to_str()?.to_owned())
}

/// Only Settings supplies alias roots. Worktree markers and session paths can
/// use a checked spelling but never add another alias.
#[derive(Default)]
struct RootAliases(Vec<(PathBuf, PathBuf)>);

impl RootAliases {
    fn new(paths: impl Iterator<Item = impl AsRef<str>>) -> Self {
        Self(
            paths
                .filter_map(|path| {
                    let path = registration_absolute(Path::new(path.as_ref()))?;
                    let resolved = resolve_chosen_path(&path, false, |_| true).ok()?;
                    (path != resolved).then_some((path, resolved))
                })
                .collect(),
        )
    }

    fn translate(&self, path: &Path) -> PathBuf {
        self.0
            .iter()
            .filter_map(|(chosen, resolved)| Some((chosen.as_os_str().len(), resolved.join(path.strip_prefix(chosen).ok()?))))
            .max_by_key(|(len, _)| *len)
            .map_or_else(|| path.to_path_buf(), |(_, resolved)| resolved)
    }
}

fn resolve_chosen_path(path: &Path, file: bool, follow_link: impl Fn(&Path) -> bool) -> io::Result<PathBuf> {
    let invalid = || io::Error::from(io::ErrorKind::InvalidInput);
    let mut path = path.to_owned();
    for _ in 0..=16 {
        path = registration_absolute(&path).ok_or_else(invalid)?;
        let ancestors: Vec<_> = path.ancestors().collect();
        let mut redirected = None;
        for dir in ancestors.into_iter().rev() {
            let meta = std::fs::symlink_metadata(dir)?;
            if crate::boundary::is_link(&meta) {
                if !follow_link(dir) {
                    return Err(invalid());
                }
                let link = std::fs::read_link(dir)?;
                #[cfg(windows)]
                let link = match link.components().next().ok_or_else(invalid)? {
                    // read_link represents a local junction's drive with a
                    // verbatim prefix. Only that drive form may be converted.
                    std::path::Component::Prefix(p) if matches!(p.kind(), std::path::Prefix::VerbatimDisk(_)) => {
                        PathBuf::from(link.to_str().and_then(|link| link.strip_prefix(r"\\?\")).ok_or_else(invalid)?)
                    }
                    _ => link,
                };
                let target = configured_path(dir.parent().ok_or_else(invalid)?, &link).ok_or_else(invalid)?;
                redirected = Some(target.join(path.strip_prefix(dir).map_err(|_| invalid())?));
                break;
            }
            let expected_type = if file && dir == path { meta.is_file() } else { meta.is_dir() };
            if !expected_type {
                return Err(invalid());
            }
        }
        match redirected {
            Some(next) => path = next,
            None => return Ok(path),
        }
    }
    Err(invalid())
}

fn configured_path(base: &Path, path: &Path) -> Option<PathBuf> {
    registration_path(base, path.strip_prefix(".").unwrap_or(path))
}

/// Git's relative pointers go up to a shared ancestor and then down.
/// Resolve those leading parents by text, with no lookup through a
/// component that would disappear (and could be a link).
fn registration_path(base: &Path, path: &Path) -> Option<PathBuf> {
    use std::path::Component;
    if path.as_os_str().is_empty() {
        return None;
    }
    if path.is_absolute() {
        return registration_absolute(path);
    }
    let mut resolved = base.to_path_buf();
    let mut parts = path.components().peekable();
    while parts.next_if_eq(&Component::ParentDir).is_some() {
        if !resolved.pop() {
            return None;
        }
    }
    for part in parts {
        let Component::Normal(name) = part else { return None };
        if cfg!(windows) && name.to_str()?.contains(':') {
            return None;
        }
        resolved.push(name);
    }
    registration_absolute(&resolved)
}

fn registration_absolute(path: &Path) -> Option<PathBuf> {
    let text = path.to_str()?;
    let path = crate::boundary::absolute(text).ok()?;
    if crate::boundary::on_a_share(text) {
        return None;
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::Storage::FileSystem::GetDriveTypeW;
        if path.components().any(|part| matches!(part, std::path::Component::Normal(name) if name.to_string_lossy().ends_with(['.', ' '])))
        {
            return None;
        }
        let root: Vec<_> = path.ancestors().last()?.as_os_str().encode_wide().chain([0]).collect();
        // SAFETY: a NUL-terminated drive root validated above.
        let kind = unsafe { GetDriveTypeW(windows::core::PCWSTR(root.as_ptr())) };
        if !local_drive(kind) {
            return None;
        }
    }
    // Drop trailing separators so symlink_metadata inspects the entry itself.
    Some(path.components().collect())
}

#[cfg(windows)]
fn local_drive(kind: u32) -> bool {
    matches!(kind, 2 | 3 | 5 | 6)
}

fn check_registration_dirs(path: &Path) -> io::Result<()> {
    let ancestors: Vec<_> = path.ancestors().collect();
    for dir in ancestors.into_iter().rev() {
        let meta = std::fs::symlink_metadata(dir)?;
        if !meta.is_dir() || crate::boundary::is_link(&meta) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
    }
    Ok(())
}

fn read_registration_marker(path: &Path) -> Option<String> {
    String::from_utf8(bounded_bytes(regular_file(path).ok()?, 64 * 1024).ok()?).ok()
}

fn regular_file(path: &Path) -> io::Result<std::fs::File> {
    let meta = std::fs::symlink_metadata(path)?;
    if !meta.is_file() || crate::boundary::is_link(&meta) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let file = crate::boundary::open_no_follow(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || crate::boundary::is_link(&meta) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    Ok(file)
}

fn bounded_bytes(file: std::fs::File, max: u64) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    file.take(max + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(bytes)
}

/// True when `candidate` is a worktree a CLI made for this folder: under its
/// `.claude/worktrees`, or one Codex made (`codex_made`) whose checkout points
/// back at it.
pub fn is_worktree_of(folder: &str, candidate: &str) -> bool {
    under_claude_worktrees(folder, candidate)
        || Path::new(candidate)
            .ancestors()
            .nth(3)
            .and_then(Path::to_str)
            .is_some_and(|project| under_claude_worktrees(project, candidate) && same_plain_place(Path::new(project), Path::new(folder)))
        || (!has_dot_parts(candidate)
            && codex_made(candidate)
            && registered_repo_of_linked(candidate).is_some_and(|r| same_plain_place(Path::new(&r), Path::new(folder))))
}

/// The project folder a CLI-made worktree belongs to, given that worktree or a
/// folder of its checkout: for Claude Code the part of the path before
/// `.claude\worktrees`, for Codex the repository its checkout points back at. The
/// shape of a path says where to look for the Git registration. Git checks
/// ignore rules only for a folder below a worktree's root.
pub fn project_of(path: &str) -> Option<String> {
    project_of_with_git(path, git)
}

fn project_of_with_git(path: &str, git: impl Fn() -> Option<Command>) -> Option<String> {
    claude_root(path, &RootAliases::default(), git)
        .map(|(_, project)| project)
        .or_else(|| codex_shaped(path).then(|| registered_repo_of_linked(path)).flatten())
}

fn claude_root(path: &str, roots: &RootAliases, git: impl Fn() -> Option<Command>) -> Option<(String, String)> {
    const MARK: &str = "\\.claude\\worktrees\\";
    // Found in a backslashed copy, cut from one that keeps the platform's own
    // separators: `/home/u/p` must not come back as `\home\u\p`. Both have the
    // same byte offsets.
    let p = if cfg!(windows) { path.replace('/', "\\") } else { path.to_string() };
    let low = p.replace('/', "\\").to_ascii_lowercase();
    let marks: Vec<usize> = low.match_indices(MARK).map(|(at, _)| at).filter(|&at| at > 0).collect();
    // The innermost first: a project can lie inside a worktree of another.
    for &at in marks.iter().rev() {
        let project = &p[..at];
        let below = &p[at + MARK.len()..];
        let name = below.find(['\\', '/']).unwrap_or(below.len());
        if name == 0 {
            continue;
        }
        let root = &p[..at + MARK.len() + name];
        let Some(checked) = registration_absolute(Path::new(root)) else { continue };
        let checked = roots.translate(&checked);
        match check_registration_dirs(&checked) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                // Claude Code removes a clean worktree on exit and makes it
                // again by name. Its shape still identifies it when gone.
                let project_dir = roots.translate(Path::new(project));
                if std::fs::symlink_metadata(&project_dir).is_err_and(|e| e.kind() == io::ErrorKind::NotFound)
                    || std::fs::symlink_metadata(project_dir.join(".git"))
                        .is_ok_and(|m| !crate::boundary::is_link(&m) && (m.is_dir() || m.is_file()))
                {
                    return Some((root.to_owned(), project.to_owned()));
                }
                continue;
            }
            Err(_) => continue,
        }
        let Some(repo) = registered_repo_from_worktree(&checked, roots) else { continue };
        if !in_checkout(&checked, &Path::new(&repo).join(".git"), below[name..].trim_matches(['\\', '/']), git) {
            break;
        }
        let project = if same_plain_place(Path::new(&repo), &roots.translate(Path::new(project))) { project.to_owned() } else { repo };
        return Some((root.to_owned(), project));
    }
    None
}

/// Whether a folder below a worktree's root (`rest`, empty for the root) is part of
/// its checkout: no repository of its own on the way down, and nothing git ignores,
/// as build output and test folders are.
fn in_checkout(root: &Path, common: &Path, rest: &str, git: impl Fn() -> Option<Command>) -> bool {
    if rest.is_empty() {
        return true;
    }
    let parts = Path::new(rest).components();
    if !parts.clone().all(|part| matches!(part, std::path::Component::Normal(_))) || registration_absolute(&root.join(rest)).is_none() {
        return true;
    }
    let mut dir = root.to_path_buf();
    let mut readable = readable_ignore_file(&dir);
    for part in parts {
        dir.push(part);
        match std::fs::symlink_metadata(&dir) {
            Ok(meta) if meta.is_dir() && !crate::boundary::is_link(&meta) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => break,
            _ => return true,
        }
        match std::fs::symlink_metadata(dir.join(".git")) {
            Ok(meta) if !crate::boundary::is_link(&meta) => return false,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            _ => return true,
        }
        readable &= readable_ignore_file(&dir);
    }
    if !readable {
        return true;
    }
    ignored_by_git(root, common, rest, git) != Some(true)
}

/// Whether two paths lead to one folder, however each is written. What git writes
/// (a `.git` file) has its long names, and a folder may come spelled in 8.3 short
/// ones (`C:\Users\RUNNER~1\…`, as a temp folder under a long user name is): only
/// the file system can tell those are one.
pub fn same_place(a: &str, b: &str) -> bool {
    match (directory_stamp(Path::new(a)), directory_stamp(Path::new(b))) {
        (Some((_, Some(a))), Some((_, Some(b)))) => a == b,
        (Some((a, _)), Some((b, _))) => a == b,
        (None, None) => norm(a) == norm(b),
        _ => false,
    }
}

pub(crate) fn same_plain_place(a: &Path, b: &Path) -> bool {
    let (Some(a), Some(b)) = (registration_absolute(a), registration_absolute(b)) else { return false };
    let (Ok(a), Ok(b)) = (plain_dirs(&a, false), plain_dirs(&b, false)) else { return false };
    a.last().unwrap().same_directory(b.last().unwrap())
}

/// Walk relative to held parents so the handles used for comparison are the
/// same ones whose type was checked. Only containment can use a missing tail.
fn plain_dirs(path: &Path, missing_tail: bool) -> io::Result<Vec<crate::held::Dir>> {
    let mut ancestors = path.ancestors().collect::<Vec<_>>().into_iter().rev();
    let root = crate::held::Dir::comparison_root(ancestors.next().unwrap())?;
    let mut dirs = vec![root];
    for path in ancestors {
        #[cfg(test)]
        COMPARISON_OPEN.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.as_ref().is_some_and(|(at, _)| at == path) {
                slot.take().unwrap().1();
            }
        });
        let name = path.file_name().and_then(|name| name.to_str()).ok_or(io::ErrorKind::InvalidInput)?;
        match dirs.last().unwrap().comparison_child(name) {
            Ok(dir) => dirs.push(dir),
            Err(e) if missing_tail && e.kind() == io::ErrorKind::NotFound => break,
            Err(e) => return Err(e),
        }
    }
    Ok(dirs)
}

#[cfg(test)]
type ComparisonOpen = (PathBuf, Box<dyn FnOnce()>);

#[cfg(test)]
thread_local! {
    static COMPARISON_OPEN: std::cell::RefCell<Option<ComparisonOpen>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn before_comparison_open(path: PathBuf, action: impl FnOnce() + 'static) {
    COMPARISON_OPEN.with(|slot| assert!(slot.replace(Some((path, Box::new(action)))).is_none()));
}

/// The directory's resolved spelling and its volume and file identity where
/// the filesystem provides stable IDs.
pub(crate) fn directory_stamp(path: &Path) -> Option<(String, Option<String>)> {
    #[cfg(windows)]
    {
        use crate::desktop::volume;
        use std::os::windows::fs::OpenOptionsExt;
        use windows::Win32::Storage::FileSystem::{FILE_FLAG_BACKUP_SEMANTICS, FILE_READ_ATTRIBUTES};
        let file =
            std::fs::OpenOptions::new().access_mode(FILE_READ_ATTRIBUTES.0).custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0).open(path).ok()?;
        if !file.metadata().ok()?.is_dir() {
            return None;
        }
        volume::directory_stamp(&file)
    }
    #[cfg(not(windows))]
    {
        let path = std::fs::canonicalize(path).ok()?;
        let meta = std::fs::metadata(&path).ok()?;
        if !meta.is_dir() {
            return None;
        }
        #[cfg(unix)]
        let id = {
            use std::os::unix::fs::MetadataExt;
            Some(format!("{:x}:{:x}", meta.dev(), meta.ino()))
        };
        #[cfg(not(unix))]
        let id = None;
        Some((path.into_os_string().into_string().ok()?, id))
    }
}

/// The CLI-made worktree `path` is in, itself or a folder of its checkout, with
/// the project it belongs to: a session's cwd follows its shell's `cd` (Claude Code
/// moves its own with it), and where it works is the worktree all the same. Its
/// root is the nearest folder up that is one of the project's worktrees by shape,
/// or by its `.git` file (one made from inside another worktree). `project_of`
/// knows a Codex worktree by its root alone, so a folder of one is looked up from.
pub fn root_of(path: &str) -> Option<(String, String)> {
    root_of_with_roots(path, &RootAliases::default())
}

pub(crate) fn root_of_in_settings<'a>(path: &str, projects: impl Iterator<Item = &'a str>) -> Option<(String, String)> {
    root_of(path).or_else(|| {
        let claude_shape = path.replace('/', "\\").to_ascii_lowercase().contains("\\.claude\\worktrees\\");
        if !claude_shape && !Path::new(path).ancestors().any(|dir| codex_shaped(&dir.to_string_lossy())) {
            return None;
        }
        let roots = aliases_in_settings(projects);
        if roots.0.is_empty() {
            return None;
        }
        root_of_with_roots(path, &roots)
    })
}

/// Compare a session's project path against a library folder from Settings.
/// Only the chosen root can authorize links.
pub(crate) fn within_settings_root(projected: &Path, chosen: &Path) -> bool {
    let Ok(chosen) = resolve_chosen_path(chosen, false, |_| true) else { return false };
    within_plain_root(projected, &chosen)
}

pub(crate) fn within_plain_root(path: &Path, root: &Path) -> bool {
    let (Some(path), Some(root)) = (registration_absolute(path), registration_absolute(root)) else { return false };
    let (Ok(root), Ok(path)) = (plain_dirs(&root, false), plain_dirs(&path, true)) else { return false };
    let root = root.last().unwrap();
    path.iter().rev().any(|dir| dir.same_directory(root))
}

pub(crate) fn rollout_matches_project<'a>(worktree: &str, project: &str, projects: impl Iterator<Item = &'a str>) -> bool {
    let checked = |roots: &RootAliases| {
        let project = roots.translate(&registration_absolute(Path::new(project))?);
        let project = plain_dirs(&project, false).ok()?;
        let repo = registered_repo_with_roots(worktree, roots)?;
        let repo = plain_dirs(Path::new(&repo), false).ok()?;
        Some(repo.last()?.same_directory(project.last()?))
    };
    checked(&RootAliases::default()).or_else(|| checked(&aliases_in_settings(projects))).unwrap_or(false)
}

fn aliases_in_settings<'a>(projects: impl Iterator<Item = &'a str>) -> RootAliases {
    let accounts = crate::settings::get().accounts_root;
    RootAliases::new(projects.map(str::to_owned).chain(accounts))
}

fn root_of_with_roots(path: &str, roots: &RootAliases) -> Option<(String, String)> {
    claude_root(path, roots, git).or_else(|| {
        let mut inner: Vec<&str> = Vec::new();
        for dir in Path::new(path).ancestors() {
            let dir = dir.to_str()?;
            if codex_shaped(dir) {
                let Some(project) = registered_repo_with_roots(dir, roots) else { continue };
                // A worktree made from within this one may have another shape.
                let root = inner
                    .into_iter()
                    .find(|d| registered_repo_with_roots(d, roots).is_some_and(|r| same_plain_place(Path::new(&r), Path::new(&project))))
                    .unwrap_or(dir);
                return Some((root.to_owned(), project));
            }
            inner.push(dir);
        }
        None
    })
}

/// The 8.3 short spelling of an existing folder where its volume keeps one, else
/// the path as it is: what git says of a repository comes in long names, and a
/// test spelling its own in short ones has Luna tell the two apart as a user's
/// temp folder under a long user name (`C:\Users\RUNNER~1\…`) would.
#[cfg(test)]
pub(crate) fn short_spelling(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        use windows::Win32::Storage::FileSystem::GetShortPathNameW;
        let long: Vec<_> = path.as_os_str().encode_wide().chain([0]).collect();
        let mut short = vec![0; 32_768];
        // SAFETY: long is NUL-terminated and short is writable for the call.
        let len = unsafe { GetShortPathNameW(windows::core::PCWSTR(long.as_ptr()), Some(&mut short)) } as usize;
        assert!(len > 0 && len < short.len(), "GetShortPathNameW failed for {}", path.display());
        PathBuf::from(OsString::from_wide(&short[..len]))
    }
    #[cfg(not(windows))]
    {
        path.to_path_buf()
    }
}

#[cfg(all(test, windows))]
pub(crate) fn short_alias(path: &Path) -> Option<PathBuf> {
    let short = short_spelling(path);
    if short.file_name() == path.file_name() {
        eprintln!("skipped the 8.3 alias check: the volume assigned no short name to {}", path.display());
        return None;
    }
    Some(short)
}

#[cfg(all(test, any(unix, windows)))]
pub(crate) fn link_dir(link: &Path, target: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // mklink treats a forward slash in an unquoted path as a switch.
        let made = Command::new("cmd").args(["/C", "mklink", "/J"]).args([link, target]).creation_flags(0x0800_0000).output().unwrap();
        assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stderr));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_git() -> Option<&'static Path> {
        let exe = git_executable();
        if exe.is_none() {
            assert!(std::env::var_os("CI").is_none(), "CI requires Git on the absolute PATH");
            eprintln!("skipped: no Git on the absolute PATH");
        }
        exe
    }

    fn fixture_git(folder: &str) -> Command {
        let mut c = git().expect("the fixture requires Git");
        c.env("HOME", folder).env("XDG_CONFIG_HOME", folder).env("GIT_CONFIG_NOSYSTEM", "1").args(["-C", folder]);
        c
    }

    /// A `.git` file cannot name a device or verbatim path, even when the
    /// name is in a worktree shape that would otherwise be classified.
    #[cfg(windows)]
    #[test]
    fn a_worktree_s_repository_is_taken_only_as_a_plain_path() {
        let root = std::env::temp_dir().join(format!("luna-wt-device-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let wt = root.join("p/.claude/worktrees/w");
        let sub = wt.join("src");
        std::fs::create_dir_all(&sub).unwrap();
        for at in [
            r"\\?\GLOBALROOT\Device\Mup\luna-no-such-host\share\repo",
            r"\\?\GLOBALROOT\Device\HarddiskVolume1\repo",
            r"\\.\UNC\luna-no-such-host\share\repo",
            r"\\.\C:\repo",
            r"\\?\C:\repo",
            r"\??\UNC\luna-no-such-host\share\repo",
            "//?/UNC/luna-no-such-host/share/repo",
            "//./pipe/repo",
        ] {
            std::fs::write(wt.join(".git"), format!("gitdir: {at}/.git/worktrees/w")).unwrap();
            assert_eq!(project_of(sub.to_str().unwrap()), None, "{at}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A local worktree whose `.git` file names a repository on a share leads
    /// to none, Claude Code's shape and Codex's alike, so nothing on that host
    /// is looked at (the host does not exist; a look would wait on it).
    #[cfg(windows)]
    #[test]
    fn a_worktree_pointing_at_a_share_leads_to_no_repository() {
        let root = std::env::temp_dir().join(format!("luna-wt-share-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let claude = root.join("p").join(".claude").join("worktrees").join("w");
        let codex = root.join("worktrees").join("ab12").join("repo");
        for wt in [&claude, &codex] {
            std::fs::create_dir_all(wt.join("src")).unwrap();
            std::fs::write(wt.join(".git"), r"gitdir: \\luna-no-such-host\share\repo\.git\worktrees\w").unwrap();
        }
        let s = |p: &Path| p.to_string_lossy().into_owned();
        assert_eq!(project_of(&s(&claude.join("src"))), None, "below a worktree whose repository is a share");
        assert_eq!(project_of(&s(&codex)), None);
        assert_eq!(root_of(&s(&codex.join("src"))), None);
        // Exercise the drive-type rejection without adding a Windows mapping.
        // SAFETY: GetLogicalDrives just returns the current drive bitmask.
        let drives = unsafe { windows::Win32::Storage::FileSystem::GetLogicalDrives() };
        assert!(!local_drive(4), "DRIVE_REMOTE is refused even for a valid drive letter");
        if let Some(letter) = (b'A'..=b'Z').find(|letter| drives & (1 << (*letter - b'A')) == 0) {
            let file = PathBuf::from(format!("{}:\\repo\\.git\\info\\exclude", char::from(letter)));
            assert!(registration_absolute(&file).is_none());
            assert!(exclude_file(&file).is_err_and(|e| e.kind() == io::ErrorKind::InvalidInput));
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A path written `E:\p\x` as this platform has it (`/E/p/x` off Windows),
    /// for what takes one apart by the platform's own separators.
    fn native(path: &str) -> String {
        if cfg!(windows) {
            path.to_string()
        } else {
            format!("/{}", path.replace(':', "").replace('\\', "/"))
        }
    }

    /// The folder a path leads to, to compare by: what Luna says of a project may be
    /// in git's spelling (long names) and a test's own in short ones. One gone, by
    /// its text.
    fn place(path: &str) -> String {
        std::fs::canonicalize(path).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| norm(path))
    }

    /// A repository with one linked worktree where Codex puts them, under a
    /// fake account (`<root>/account`, `root` taken for the accounts root's
    /// Codex folder): (root, repo, worktree), spelled short where the volume
    /// has short names. None where there is no git.
    fn codex_layout(tag: &str) -> Option<(PathBuf, String, String)> {
        test_git()?;
        let root = std::env::temp_dir().join(format!("luna-wt-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let root = short_spelling(&root);
        TEST_CODEX_ACCOUNTS.lock().unwrap().push(root.to_string_lossy().into_owned());
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let repo_s = repo.to_string_lossy().into_owned();
        let setup = |args: &[&str]| {
            let out = fixture_git(&repo_s).args(args).output().unwrap();
            assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        setup(&["init", "-q"]);
        setup(&["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "i"]);
        let wt = root.join("account").join("worktrees").join("0a1b").join("repo");
        std::fs::create_dir_all(wt.parent().unwrap()).unwrap();
        let wt_s = wt.to_string_lossy().into_owned();
        setup(&["worktree", "add", "-q", "--detach", &wt_s]);
        Some((root, repo_s, wt_s))
    }

    #[cfg(windows)]
    #[test]
    fn git_registration_agrees_between_long_and_short_temp_paths() {
        let Some((root, repo, worktree)) = codex_layout("short-temp-spelling") else { return };
        let (long, _) = directory_stamp(&root).unwrap();
        let long = Path::new(long.strip_prefix(r"\\?\").unwrap_or(&long));
        let Some(short) = short_alias(long) else {
            std::fs::remove_dir_all(&root).unwrap();
            return;
        };
        let rebase = |path: &str, prefix: &Path| prefix.join(Path::new(path).strip_prefix(&root).unwrap());
        let long_repo = rebase(&repo, long);
        let short_repo = rebase(&repo, &short);
        let long_wt = rebase(&worktree, long);
        let short_wt = rebase(&worktree, &short);
        let marker = long_wt.join(".git");
        let admin = gitdir_from_marker(&std::fs::read_to_string(&marker).unwrap()).unwrap();
        let admin = long_repo.join(".git").join("worktrees").join(admin.file_name().unwrap());
        std::fs::write(&marker, format!("gitdir: {}\n", admin.display())).unwrap();
        std::fs::write(admin.join("commondir"), format!("{}\n", short_repo.join(".git").display())).unwrap();
        std::fs::write(admin.join("gitdir"), format!("{}\n", long_wt.join(".git").display())).unwrap();

        let repo = short_repo.to_str().unwrap();
        let wt = short_wt.to_str().unwrap();
        for found in [registered_repo_of_linked(wt), project_of(wt)] {
            assert!(same_place(&found.expect("the short checkout is registered"), repo));
        }
        assert!(is_worktree_of(repo, long_wt.to_str().unwrap()));
        let claude = long_repo.join(".claude/worktrees/topic");
        assert!(is_worktree_of(repo, claude.to_str().unwrap()));
        assert!(rollout_matches_project(wt, repo, std::iter::empty()));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn same_place_distinguishes_directories_that_differ_only_in_case() {
        let root = std::env::temp_dir().join(format!("luna-wt-place-case-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let upper = root.join("Project");
        let lower = root.join("project");
        std::fs::create_dir_all(&upper).unwrap();
        let upper_path = upper.to_str().unwrap();
        let lower_path = lower.to_str().unwrap();
        match std::fs::create_dir(&lower) {
            Ok(()) => assert!(!same_place(upper_path, lower_path)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                assert!(same_place(upper_path, lower_path));
            }
            Err(e) => panic!("{e}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn linked_registration_refuses_unrelated_markers_and_a_linked_gitdir() {
        let temp = std::env::temp_dir();
        #[cfg(unix)]
        let temp = std::fs::canonicalize(temp).unwrap();
        let root = temp.join(format!("luna-wt-registration-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let repo = root.join("repo");
        // mklink treats a forward slash in an unquoted path as a switch.
        let admin = repo.join(".git").join("worktrees");
        let gitdir = admin.join("id");
        let wt = root.join("worktrees").join("1234").join("checkout");
        let other = root.join("other");
        for dir in [&gitdir, &wt, &other.join(".git")] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let marker = wt.join(".git");
        let register = |pointer: &Path| std::fs::write(&marker, format!("gitdir: {}\n", pointer.display())).unwrap();
        let back = |checkout: &Path| std::fs::write(gitdir.join("gitdir"), format!("{}\n", checkout.join(".git").display())).unwrap();
        let common = |dir: &Path| std::fs::write(gitdir.join("commondir"), format!("{}\n", dir.display())).unwrap();
        let registered = || registered_repo_of_linked(wt.to_str().unwrap());
        let project = || project_of(wt.to_str().unwrap());
        let is_repo = |found: Option<String>| found.is_some_and(|r| same_place(&r, repo.to_str().unwrap()));
        register(&gitdir);
        back(&wt);
        common(Path::new("../.."));
        assert!(is_repo(registered()));
        assert!(is_repo(project()));
        register(Path::new("../../../repo/.git/worktrees/id"));
        std::fs::write(gitdir.join("gitdir"), "../../../../worktrees/1234/checkout/.git\n").unwrap();
        assert!(is_repo(registered()), "Git's relative markers");
        register(&gitdir);
        back(&wt);

        common(&other.join(".git"));
        assert_eq!(registered(), None, "commondir from another repository");
        common(Path::new("../.."));
        back(&other);
        assert_eq!(registered(), None, "registration for a different checkout");
        back(&wt);

        let link = admin.join("alias");
        link_dir(&link, &gitdir);
        register(&link);
        assert_eq!(registered(), None, "the gitdir is a symlink or junction");
        register(Path::new(&format!("{}{}", link.display(), std::path::MAIN_SEPARATOR)));
        assert_eq!(registered(), None, "a trailing separator cannot hide the link");
        register(&Path::new("../../../repo/.git/worktrees").join("alias"));
        assert_eq!(registered(), None, "a relative pointer cannot hide the link");
        register(Path::new("../../../repo/.git/worktrees/alias/."));
        assert_eq!(registered(), None, "a trailing dot cannot hide the link");

        let repo_alias = root.join("repo-alias");
        link_dir(&repo_alias, &repo);
        register(&repo_alias.join(".git").join("worktrees").join("id"));
        assert_eq!(registered(), None, "the gitdir has a linked ancestor");
        assert_eq!(project(), None, "classification refuses the gitdir's linked ancestor");
        register(Path::new("../../../repo-alias/.git/worktrees/id"));
        assert_eq!(registered(), None, "the relative gitdir has a linked ancestor");
        register(Path::new("../../../repo-alias/../repo/.git/worktrees/id"));
        assert_eq!(registered(), None, "a parent component cannot hide a link");
        let accounts_alias = root.join("accounts-alias");
        link_dir(&accounts_alias, &root);
        let alias_checkout = accounts_alias.join("worktrees/1234/checkout");
        register(&repo_alias.join(".git").join("worktrees").join("id"));
        back(&alias_checkout);
        let chosen = RootAliases::new([accounts_alias.to_str().unwrap(), repo_alias.to_str().unwrap()].into_iter());
        assert_eq!(project_of(alias_checkout.to_str().unwrap()), None, "an input path alone cannot authorize aliases");
        assert!(
            is_repo(root_of_with_roots(alias_checkout.to_str().unwrap(), &chosen).map(|(_, project)| project)),
            "the chosen accounts and project roots can use aliases"
        );
        register(&gitdir);
        back(&wt);

        let common_alias = repo.join("common-alias");
        link_dir(&common_alias, &repo.join(".git"));
        common(&common_alias);
        assert_eq!(registered(), None, "commondir names a link with a different name");
        assert_eq!(project(), None, "classification requires the literal commondir");
        let other_common = root.join("other-common");
        std::fs::create_dir(&other_common).unwrap();
        let linked_common = other_common.join(".git");
        link_dir(&linked_common, &repo.join(".git"));
        common(&linked_common);
        assert_eq!(registered(), None, "the right directory reached through an unapproved link is refused");
        common(Path::new("../.."));

        let wt_alias = root.join("checkout-alias");
        link_dir(&wt_alias, &wt);
        back(&wt_alias);
        assert_eq!(registered(), None, "the back-pointer has a linked ancestor");
        assert_eq!(project(), None, "classification refuses the back-pointer's linked ancestor");
        back(&wt);
        std::fs::rename(&marker, wt.join("marker")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(wt.join("marker"), &marker).unwrap();
        #[cfg(windows)]
        link_dir(&marker, &gitdir);
        assert_eq!(project(), None, "classification does not follow the checkout marker");
        #[cfg(unix)]
        std::fs::remove_file(&marker).unwrap();
        #[cfg(windows)]
        std::fs::remove_dir(&marker).unwrap();
        std::fs::write(&marker, format!("gitdir: {}{}\n", gitdir.display(), " ".repeat(64 * 1024))).unwrap();
        assert_eq!(project(), None, "classification bounds the marker read");

        #[cfg(windows)]
        {
            for pointer in [
                r"\\?\C:\repo\.git\worktrees\id",
                r"\\.\C:\repo\.git\worktrees\id",
                r"\\?\UNC\luna-no-such-host\share\repo\.git\worktrees\id",
                r"\\luna-no-such-host\share\repo\.git\worktrees\id",
            ] {
                register(Path::new(pointer));
                assert_eq!(registered(), None, "device or share: {pointer}");
            }
            for link in [link, repo_alias, common_alias, wt_alias, accounts_alias] {
                std::fs::remove_dir(link).unwrap();
            }
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_in_a_worktree_registration_is_refused_without_waiting() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        use std::sync::mpsc;
        use std::time::Duration;

        let root = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("luna-wt-fifo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let wt = root.join("p/.claude/worktrees/checkout");
        let gitdir = root.join("repo/.git/worktrees/id");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::create_dir_all(&gitdir).unwrap();
        let markers = [
            (wt.join(".git"), format!("gitdir: {}\n", gitdir.display())),
            (gitdir.join("commondir"), "../..\n".into()),
            (gitdir.join("gitdir"), format!("{}/.git\n", wt.display())),
        ];
        for (path, text) in &markers {
            std::fs::write(path, text).unwrap();
        }
        assert!(project_of(wt.to_str().unwrap()).is_some());
        for (path, text) in &markers {
            std::fs::remove_file(path).unwrap();
            let name = CString::new(path.as_os_str().as_bytes()).unwrap();
            // SAFETY: a NUL-terminated path in the test fixture.
            assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
            let (tx, rx) = mpsc::channel();
            let wt = wt.clone();
            std::thread::spawn(move || tx.send(project_of(wt.to_str().unwrap())).ok());
            assert_eq!(rx.recv_timeout(Duration::from_secs(2)), Ok(None), "{}", path.display());
            std::fs::remove_file(path).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_name_cannot_be_read_as_a_flag_and_is_always_given_with_equals() {
        assert_eq!(worktree_arg(Some("feature-x")), "--worktree=feature-x");
        assert_eq!(worktree_arg(None), "--worktree");
        for bad in ["--dangerously-skip-permissions", "-x", ".hidden", "a b", "a/b", "", &"x".repeat(65)] {
            assert!(!name_ok(bad), "{bad}");
            assert_eq!(worktree_arg(Some(bad)), "--worktree", "{bad}: the CLI names its own");
        }
    }

    #[test]
    fn a_worktree_path_names_its_project() {
        let temp = std::env::temp_dir();
        #[cfg(unix)]
        let temp = std::fs::canonicalize(temp).unwrap();
        let gone = temp.join(format!("luna-wt-gone-project-{}", std::process::id()));
        let wt = gone.join(".claude/worktrees/a-b-c");
        assert_eq!(project_of(wt.to_str().unwrap()).as_deref(), gone.to_str());
        assert_eq!(project_of(gone.to_str().unwrap()), None);
        let Some((root, repo, wt)) = codex_layout("project") else { return };
        assert_eq!(project_of(&wt).map(|p| place(&p)), Some(place(&repo)));
        assert!(is_worktree_of(&repo, &wt));
        assert!(!is_worktree_of(&root.to_string_lossy(), &wt), "another folder's");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_folder_is_of_a_worktree_only_if_git_says_so_whatever_its_path_looks_like() {
        if test_git().is_none() {
            return;
        }
        let root = std::env::temp_dir().join(format!("luna-wt-of-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let root = short_spelling(&root);
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let ok = |dir: &Path, args: &[&str]| {
            fixture_git(&s(dir)).args(["-c", "user.email=t@t", "-c", "user.name=t"]).args(args).output().is_ok_and(|o| o.status.success())
        };
        assert!(ok(&repo, &["init", "-q"]));
        std::fs::write(repo.join(".gitignore"), "test-build/\n/:(literal/\n").unwrap();
        assert!(ok(&repo, &["add", "-A"]) && ok(&repo, &["commit", "-q", "-m", "i"]));
        let of = |p: &Path| project_of(&s(p)).map(|found| place(&found));
        let is_repo = Some(place(&s(&repo)));
        let worktrees = repo.join(".claude").join("worktrees");

        // A real one, and a folder of its checkout, tracked or not, there yet or not.
        let wt = worktrees.join("real");
        assert!(ok(&repo, &["worktree", "add", "-q", "-b", "worktree-real", &s(&wt)]));
        assert_eq!(of(&wt), is_repo);
        std::fs::create_dir_all(wt.join("src")).unwrap();
        assert_eq!(of(&wt.join("src")), is_repo);
        assert_eq!(of(&wt.join("src").join("not-yet")), is_repo);

        let plain = wt.join("test-build").join("harness").join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        assert_eq!(of(&plain), None, "a plain folder under <wt>\\test-build");
        assert_eq!(of(&wt.join("test-build")), None);
        #[cfg(unix)]
        {
            let colon = wt.join(":(literal");
            std::fs::create_dir(&colon).unwrap();
            assert_eq!(of(&colon), None, "a colon starts the folder's actual name");
        }

        // A repository of its own there, and what that one's CLIs make, are its own.
        let own = wt.join("test-build").join("own");
        std::fs::create_dir_all(&own).unwrap();
        assert!(ok(&own, &["init", "-q"]) && ok(&own, &["commit", "-q", "--allow-empty", "-m", "i"]));
        assert_eq!(of(&own), None);
        std::fs::create_dir_all(own.join("deep")).unwrap();
        assert_eq!(of(&own.join("deep")), None);
        let own_wt = own.join(".claude").join("worktrees").join("its");
        assert!(ok(&own, &["worktree", "add", "-q", "-b", "worktree-its", &s(&own_wt)]));
        assert_eq!(of(&own_wt), Some(place(&s(&own))));
        let codex = wt.join("test-build").join("account").join("worktrees").join("0a1b").join("own");
        std::fs::create_dir_all(codex.parent().unwrap()).unwrap();
        assert!(ok(&own, &["worktree", "add", "-q", "--detach", &s(&codex)]));
        assert_eq!(of(&codex), Some(place(&s(&own))), "Codex's, in an account under a worktree");

        // A worktree made from inside a worktree is of the repository.
        let nested = wt.join(".claude").join("worktrees").join("nested");
        assert!(ok(&wt, &["worktree", "add", "-q", "-b", "worktree-nested", &s(&nested)]));
        assert_eq!(of(&nested), is_repo);

        // In the right place and no worktree: a plain folder, a clone of its own, a
        // submodule's `.git` file, one that points at another repository's.
        let bare = worktrees.join("plain");
        std::fs::create_dir_all(&bare).unwrap();
        assert_eq!(of(&bare), None);
        let clone = worktrees.join("clone");
        std::fs::create_dir_all(&clone).unwrap();
        assert!(ok(&clone, &["init", "-q"]));
        assert_eq!(of(&clone), None);
        let sub = worktrees.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join(".git"), format!("gitdir: {}", s(&repo.join(".git").join("modules").join("sub")))).unwrap();
        assert_eq!(of(&sub), None);
        let moved = worktrees.join("moved");
        std::fs::create_dir_all(&moved).unwrap();
        std::fs::write(moved.join(".git"), format!("gitdir: {}", s(&root.join("gone").join(".git").join("worktrees").join("moved"))))
            .unwrap();
        assert_eq!(of(&moved), None, "its repository is not there");

        // Gone, as Claude Code leaves a clean one: where it was is all there is, in a
        // repository; in a folder that is none, nothing says it ever was one.
        assert_eq!(of(&worktrees.join("gone")), is_repo);
        assert_eq!(of(&worktrees.join("gone").join("src")), is_repo);
        let folder = root.join("no-repo");
        std::fs::create_dir_all(folder.join(".claude").join("worktrees").join("there")).unwrap();
        assert_eq!(of(&folder.join(".claude").join("worktrees").join("gone")), None);
        assert_eq!(of(&folder.join(".claude").join("worktrees").join("there")), None);
        assert_eq!(of(&worktrees), None, "the folder that holds them");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_clean_ignore_query_isolates_config_and_preserves_ignore_rules() {
        let Some(exe) = test_git() else { return };
        let temp = std::env::temp_dir();
        #[cfg(unix)]
        let temp = std::fs::canonicalize(temp).unwrap();
        let root = temp.join(format!("luna ignore parity {}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let repo = root.join("main");
        let wt = repo.join(".claude").join("worktrees").join("linked");
        let global_config = root.join(".gitconfig");
        let xdg = root.join("xdg");
        let global_exclude = root.join("global-ignore");
        std::fs::write(&global_config, "").unwrap();
        std::fs::write(&global_exclude, "/global-item\n").unwrap();
        let git = || {
            let mut c = git_on_path(exe);
            c.env("GIT_CONFIG_NOSYSTEM", "1").env("HOME", &root).env("XDG_CONFIG_HOME", &xdg);
            c.current_dir(&root);
            Some(c)
        };
        let run = |dir: &Path, args: &[&OsStr]| {
            let out = git().unwrap().arg("-C").arg(dir).args(args).output().unwrap();
            assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        let setup = |dir: &Path, args: &[&str]| run(dir, &args.iter().map(OsStr::new).collect::<Vec<_>>());
        run(&root, &[OsStr::new("config"), OsStr::new("--global"), OsStr::new("core.excludesFile"), global_exclude.as_os_str()]);
        setup(&root, &["init", "-q", "--template=", "main"]);
        for dir in ["ignored", "nested", "sparse", ".git/info"] {
            std::fs::create_dir_all(repo.join(dir)).unwrap();
        }
        let ignore_case = cfg!(any(windows, target_os = "macos"));
        setup(&repo, &["config", "core.ignoreCase", if ignore_case { "false" } else { "true" }]);
        for (path, rule) in [
            (".gitignore", "/build/\n/ignored/\n/root-item\n/CASE-ITEM\n/:(colon/\n"),
            ("nested/.gitignore", "*\n!keep\n"),
            ("sparse/.gitignore", "/sparse-only\n"),
            (".git/info/exclude", "/common-item\n"),
            ("ignored/tracked", "fixture\n"),
        ] {
            std::fs::write(repo.join(path), rule).unwrap();
        }
        setup(&repo, &["add", "-f", ".gitignore", "nested/.gitignore", "sparse/.gitignore", "ignored/tracked"]);
        setup(&repo, &["-c", "user.name=Test", "-c", "user.email=t@t", "commit", "-qm", "fixture"]);
        run(&repo, &[OsStr::new("worktree"), OsStr::new("add"), OsStr::new("-q"), OsStr::new("--detach"), wt.as_os_str()]);
        setup(&wt, &["update-index", "--skip-worktree", "sparse/.gitignore"]);
        std::fs::remove_file(wt.join("sparse/.gitignore")).unwrap();
        let common = repo.join(".git");
        let registered = registered_repo_of_linked(wt.to_str().unwrap()).expect("the fixture worktree is registered");
        assert!(same_place(&registered, repo.to_str().unwrap()), "{registered} and {}", repo.display());
        let cases = [
            ("root-item", true, true),
            ("nested/skip", true, true),
            ("nested/keep", false, false),
            ("common-item", true, true),
            ("global-item", true, true),
            ("ignored/tracked", false, true),
            ("ignored", false, true),
            ("sparse/sparse-only", true, false),
            ("case-item", !ignore_case, ignore_case),
            #[cfg(unix)]
            (":(colon", true, true),
        ];
        #[cfg(unix)]
        std::fs::create_dir(wt.join(":(colon")).unwrap();
        for (path, plain, clean) in cases {
            let check = git().unwrap().arg("-C").arg(&wt).args(["check-ignore", "-q", "--", &format!("./{path}")]).output().unwrap();
            assert_eq!(check.status.code(), Some(i32::from(!plain)), "plain Git: {path}");
            assert_eq!(ignored_by_git(&wt, &common, path, git), Some(clean), "clean Git: {path}");
        }
        let relative_xdg = || {
            let mut c = git()?;
            c.env("XDG_CONFIG_HOME", "global-decoy");
            Some(c)
        };
        std::fs::create_dir_all(wt.join("global-decoy/git")).unwrap();
        std::fs::write(wt.join("global-decoy/git/config"), "[unterminated").unwrap();
        assert_eq!(ignored_by_git(&wt, &common, "global-item", relative_xdg), Some(true));

        assert_no_fsmonitor(&root, &repo, &wt, git, setup);
        setup(&repo, &["config", "core.excludesFile", "project-ignore"]);
        std::fs::write(wt.join("project-ignore"), "/project-item\n").unwrap();
        assert_eq!(ignored_by_git(&wt, &common, "project-item", git), Some(false));
        assert_eq!(ignored_by_git(&wt, &common, "global-item", git), Some(true));

        // Git's global-only command selects ~/.gitconfig. Check that the
        // XDG config also contributes, with ~/.gitconfig taking precedence.
        std::fs::create_dir_all(xdg.join("git")).unwrap();
        std::fs::write(xdg.join("git/config"), "[core]\nexcludesFile = ./xdg-ignore\n").unwrap();
        std::fs::write(wt.join("xdg-ignore"), "/xdg-config-item\n").unwrap();
        assert_eq!(ignored_by_git(&wt, &common, "global-item", git), Some(true));
        std::fs::write(&global_config, "[includeIf \"gitdir:never-match/\"]\npath = ./unrelated\n").unwrap();
        let unrelated = root.join("unrelated");
        std::fs::write(&unrelated, "[include]\npath = unrelated-child\n").unwrap();
        let child = root.join("unrelated-child");
        std::fs::write(&child, "[unterminated").unwrap();
        assert_eq!(ignored_by_git(&wt, &common, "xdg-config-item", git), Some(true));
        std::fs::write(&unrelated, "[core]\nexcludesFile = different\n[include]\npath = unrelated-child\n").unwrap();
        assert_eq!(ignored_by_git(&wt, &common, "xdg-config-item", git), Some(true));
        std::fs::write(&global_config, "[include]\npath = ./unrelated\n").unwrap();
        std::fs::write(&child, "[core]\nexcludesFile = child-ignore\n").unwrap();
        std::fs::write(wt.join("child-ignore"), "/child-item\n").unwrap();
        assert_eq!(ignored_by_git(&wt, &common, "child-item", git), Some(true));
        std::fs::OpenOptions::new().write(true).open(&child).unwrap().set_len(crate::boundary::SETTINGS_MAX + 1).unwrap();
        assert_eq!(ignored_by_git(&wt, &common, "child-item", git), None);

        let target = root.join("outside-excludes");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("ignore"), "/outside-item\n").unwrap();
        let alias = wt.join("exclude-alias");
        link_dir(&alias, &target);
        let set_excludes = |path: &Path| {
            run(&root, &[OsStr::new("config"), OsStr::new("--global"), OsStr::new("core.excludesFile"), path.as_os_str()]);
        };
        std::fs::write(&global_config, "").unwrap();
        setup(&root, &["config", "--global", "core.excludesFile", "~/global-ignore"]);
        let trace = root.join("trace-output");
        run(&root, &[OsStr::new("config"), OsStr::new("--global"), OsStr::new("trace2.eventTarget"), trace.as_os_str()]);
        assert_eq!(ignored_by_git(&wt, &common, "global-item", git), Some(true));
        assert!(!trace.exists(), "the parser must start with private homes even before --no-includes takes effect");
        std::fs::write(&global_config, "").unwrap();
        set_excludes(&alias.join("ignore"));
        assert_eq!(ignored_by_git(&wt, &common, "outside-item", git), None, "an absolute setting inside the checkout is no-follow");
        set_excludes(&wt.join("xdg-ignore"));
        assert_eq!(ignored_by_git(&wt, &common, "xdg-config-item", git), Some(true));
        std::fs::write(&global_config, "").unwrap();

        let override_global = || {
            let mut c = git()?;
            c.env("GIT_CONFIG_GLOBAL", &global_config);
            Some(c)
        };
        assert_eq!(ignored_by_git(&wt, &common, "xdg-config-item", override_global), None);
        #[cfg(windows)]
        {
            let network_home = || {
                let mut c = git()?;
                c.env_remove("HOME").env("HOMEDRIVE", r"\\luna-no-such-host\share").env("HOMEPATH", r"\home").env("USERPROFILE", &root);
                Some(c)
            };
            assert_eq!(ignored_by_git(&wt, &common, "xdg-config-item", network_home), None);
        }
        #[cfg(unix)]
        {
            let relative_exclude = wt.join("xdg-ignore");
            std::fs::remove_file(&relative_exclude).unwrap();
            std::os::unix::fs::symlink(&global_exclude, relative_exclude).unwrap();
            assert_eq!(ignored_by_git(&wt, &common, "global-item", git), None);
        }

        std::fs::remove_file(xdg.join("git/config")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&global_exclude, xdg.join("git/ignore")).unwrap();
        #[cfg(windows)]
        {
            let target = xdg.join("actual-git");
            std::fs::rename(xdg.join("git"), &target).unwrap();
            std::fs::copy(&global_exclude, target.join("ignore")).unwrap();
            link_dir(&xdg.join("git"), &target);
        }
        assert_eq!(ignored_by_git(&wt, &common, "global-item", git), Some(true));
        let default_home = || {
            let mut c = git()?;
            c.env_remove("XDG_CONFIG_HOME");
            Some(c)
        };
        std::fs::create_dir_all(root.join(".config/git")).unwrap();
        std::fs::write(root.join(".config/git/ignore"), "/home-item\n").unwrap();
        assert_eq!(ignored_by_git(&wt, &common, "home-item", default_home), Some(true));

        #[cfg(unix)]
        {
            let target = wt.join("linked-dir-target");
            std::fs::create_dir_all(target.join(".git")).unwrap();
            std::os::unix::fs::symlink(target, wt.join("linked-dir")).unwrap();
            assert!(in_checkout(&wt, &common, "linked-dir", || panic!("a link must stop the probe")));
            std::os::unix::fs::symlink(&global_exclude, wt.join("sparse/.gitignore")).unwrap();
            assert!(in_checkout(&wt, &common, "sparse/sparse-only", || panic!("a linked .gitignore must stop the probe")));
        }
        let large = wt.join("nested/.gitignore");
        std::fs::OpenOptions::new().write(true).open(large).unwrap().set_len(100 * 1024 * 1024 + 1).unwrap();
        assert!(in_checkout(&wt, &common, "nested/skip", || panic!("an oversized .gitignore must stop the probe")));
        assert!(in_checkout(&wt, &common, "ignored", || None), "missing Git keeps the checkout");
        std::fs::rename(common.join("info/exclude"), common.join("info/saved")).unwrap();
        std::fs::create_dir(common.join("info/exclude")).unwrap();
        assert!(in_checkout(&wt, &common, "ignored", git), "an unreadable exclude keeps the checkout");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Plain Git runs the fixture hook when it reads the index. The isolated
    /// probe leaves no marker, regardless of which repo config sets the hook.
    fn assert_no_fsmonitor(root: &Path, repo: &Path, wt: &Path, git: impl Fn() -> Option<Command>, setup: impl Fn(&Path, &[&str])) {
        let s = |p: &Path| p.to_string_lossy().into_owned();
        // Git for Windows runs the hook through its own sh, which takes `C:/…`.
        let slashed = |p: &Path| s(p).replace('\\', "/");
        let plain = wt.join("build/harness/plain");
        std::fs::create_dir_all(&plain).unwrap();
        let nested = wt.join("src");
        std::fs::create_dir_all(&nested).unwrap();
        setup(&nested, &["init", "-q"]);
        let of = |p: &Path| project_of_with_git(&s(p), &git).map(|found| place(&found));
        let is_repo = Some(place(&s(repo)));

        let marker = root.join("marker");
        let hook = root.join("fsmonitor-hook");
        let shell_path = |p: &Path| format!("'{}'", slashed(p).replace('\'', "'\\''"));
        std::fs::write(&hook, format!("#!/bin/sh\n: > {}\nprintf 'fixture-token\\0'\n", shell_path(&marker))).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let hook = shell_path(&hook);
        setup(repo, &["config", "extensions.worktreeConfig", "true"]);
        std::fs::write(repo.join(".git").join("included"), format!("[core]\n\tfsmonitor = {hook:?}\n")).unwrap();
        let sources: [(&str, &Path, [&str; 4], [&str; 4]); 2] = [
            (
                "the worktree's own config",
                wt,
                ["config", "--worktree", "core.fsmonitor", &hook],
                ["config", "--worktree", "--unset", "core.fsmonitor"],
            ),
            ("an include.path", repo, ["config", "--local", "include.path", "included"], ["config", "--local", "--unset", "include.path"]),
        ];
        for (source, at, set, unset) in sources {
            setup(at, &set);
            let get = ["config", "--includes", "--null", "--get", "core.fsmonitor"];
            let out = git().unwrap().arg("-C").arg(wt).args(get).output().unwrap();
            assert_eq!(String::from_utf8_lossy(&out.stdout), format!("{hook}\0"), "{source}");

            // The same Git with FSMonitor enabled must run the fixture hook.
            let _ = std::fs::remove_file(&marker);
            let out = git().unwrap().arg("-C").arg(wt).args(["check-ignore", "-q", "--", "./build/harness/plain"]).output().unwrap();
            assert!(
                out.status.success() && marker.is_file(),
                "Git must ignore the folder and run the configured hook from {source}. Status: {}. {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            );
            std::fs::remove_file(&marker).unwrap();

            // Every classification first, the marker before what they say.
            let found = [of(&plain), of(&wt.join("build")), of(&wt.join("ignored")), of(&nested), of(wt)];
            assert!(!marker.exists(), "the core's probe ran the core.fsmonitor hook from {source}");
            let [ignored, ignored_root, tracked, own_repo, root_of] = found;
            assert_eq!(ignored, None, "ignored folder, {source}");
            assert_eq!(ignored_root, None, "ignored folder, {source}");
            assert_eq!(tracked, None, "the empty index does not exempt the tracked folder, {source}");
            assert_eq!(own_repo, None, "nested repository outside an ignore pattern, {source}");
            assert_eq!(root_of, is_repo, "the worktree's root, {source}");

            setup(at, &unset);
        }
    }

    #[test]
    fn a_cwd_inside_a_worktree_is_that_worktree() {
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let root = |p: &Path| root_of(&s(p)).map(|(wt, project)| (place(&wt), place(&project)));
        // Gone, as a clean one is between turns, from a project gone too: its shape.
        let gone = std::env::temp_dir().join(format!("luna-root-of-gone-{}", std::process::id())).join("x");
        let wt = gone.join(".claude").join("worktrees").join("a-b-c");
        let want = Some((place(&s(&wt)), place(&s(&gone))));
        assert_eq!(root(&wt), want);
        assert_eq!(root(&wt.join("src-tauri").join("src")), want, "a shell that went into it");
        assert_eq!(root(&gone.join("src")), None, "no worktree");
        assert_eq!(root(&gone.join(".claude").join("worktrees")), None);
        // Real ones, by what git says of them.
        let Some((tmp, repo, codex)) = codex_layout("root") else { return };
        assert_eq!(root(&Path::new(&codex).join("src")), Some((place(&codex), place(&repo))), "Codex's own shape");
        let claude = Path::new(&repo).join(".claude").join("worktrees").join("real");
        assert!(fixture_git(&repo)
            .args(["worktree", "add", "-q", "-b", "worktree-real", &s(&claude)])
            .output()
            .is_ok_and(|o| o.status.success()));
        std::fs::create_dir_all(claude.join("src").join("deep")).unwrap();
        assert_eq!(root(&claude.join("src").join("deep")), Some((place(&s(&claude)), place(&repo))));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn the_ignore_probe_sets_a_safe_environment() {
        let scrub = ["GIT_DIR", "GIT_CONFIG_COUNT", "Git_Ssh_Command", "GIT_TEST_FSMONITOR", "SSH_ASKPASS"];
        let keep = ["PATH", "HOME", "SYSTEMROOT", "GITHUB_TOKEN"];
        let replace = ["GIT_ALLOW_PROTOCOL", "GIT_NO_LAZY_FETCH"];
        let run = git_command(Path::new("fake-git"), scrub.into_iter().chain(keep).chain(replace));
        let env: std::collections::HashMap<_, _> = run.get_envs().collect();
        for name in scrub {
            assert!(env.iter().any(|(key, value)| key.eq_ignore_ascii_case(OsStr::new(name)) && value.is_none()), "{name}");
        }
        for name in keep {
            assert!(!env.contains_key(OsStr::new(name)), "{name}");
        }
        assert_eq!(env[OsStr::new("GIT_ALLOW_PROTOCOL")], Some(OsStr::new("")));
        assert_eq!(env[OsStr::new("GIT_NO_LAZY_FETCH")], Some(OsStr::new("1")));

        let repo = IgnoreRepo::new().unwrap();
        let isolated = repo.isolated(run);
        let env: std::collections::HashMap<_, _> = isolated.get_envs().collect();
        let keys =
            if cfg!(windows) { &["HOME", "XDG_CONFIG_HOME", "USERPROFILE", "APPDATA"][..] } else { &["HOME", "XDG_CONFIG_HOME"][..] };
        for key in keys {
            assert!(repo.0.is_absolute());
            assert_eq!(env[OsStr::new(key)], Some(repo.0.as_os_str()));
        }
    }

    #[test]
    fn a_git_child_must_finish_without_diagnostics_before_the_deadline() {
        let Some(exe) = test_git() else { return };
        let repo = IgnoreRepo::new().unwrap();
        for mode in ["stop", "wait"] {
            let marker = repo.0.join(mode);
            let output = repo.output(probe_tree_command(&marker, mode), Instant::now() + Duration::from_secs(5));
            let pid: u32 = std::fs::read_to_string(&marker).expect("the descendant started").parse().unwrap();
            let deadline = Instant::now() + Duration::from_secs(3);
            while crate::procs::alive(pid) {
                assert!(Instant::now() < deadline, "the {mode} probe left descendant {pid} running");
                std::thread::sleep(Duration::from_millis(20));
            }
            if mode == "stop" {
                assert!(output.is_some_and(|out| out.status.success()));
            } else {
                assert!(output.is_none(), "{output:?}");
            }
        }

        let mut command = git_on_path(exe);
        command.arg("--version");
        let mut out = repo.output(command, Instant::now() + Duration::from_secs(3)).unwrap();
        assert_eq!(out.status.code(), Some(0));
        out.stderr.extend_from_slice(b"warning: ignore file could not be read\n");
        assert_eq!(ignore_result(out), None);
    }

    fn probe_tree_command(marker: &Path, mode: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--ignored", "--exact", "checkout::tests::probe_tree_fixture"])
            .env("LUNA_CHECKOUT_PROBE_MARKER", marker)
            .env("LUNA_CHECKOUT_PROBE_MODE", mode);
        command
    }

    #[test]
    #[ignore = "subprocess fixture for probe cleanup"]
    #[allow(clippy::zombie_processes, reason = "the probe must clean up the intentionally orphaned fixture")]
    fn probe_tree_fixture() {
        let Some(marker) = std::env::var_os("LUNA_CHECKOUT_PROBE_MARKER").map(PathBuf::from) else { return };
        let mode = std::env::var("LUNA_CHECKOUT_PROBE_MODE").unwrap();
        if mode == "leaf" {
            std::fs::write(marker, std::process::id().to_string()).unwrap();
            std::thread::sleep(Duration::from_secs(15));
            return;
        }
        // Intentionally let the leaf outlive this leader. The probe owns it.
        let _leaf = probe_tree_command(&marker, "leaf").spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(4);
        while !std::fs::metadata(&marker).is_ok_and(|meta| meta.len() > 0) {
            assert!(Instant::now() < deadline, "the descendant did not start");
            std::thread::sleep(Duration::from_millis(10));
        }
        if mode == "wait" {
            std::thread::sleep(Duration::from_secs(12));
        }
    }

    #[test]
    fn core_start_sweeps_only_old_ignore_repositories() {
        let repo = IgnoreRepo::new().unwrap();
        let old = repo.0.join(format!("ignore-{}", "a".repeat(32)));
        let other = repo.0.join("ignore-other");
        let other_type = repo.0.join(format!("ignore-{}", "b".repeat(32)));
        std::fs::create_dir(&old).unwrap();
        std::fs::write(old.join("global-exclude"), "private").unwrap();
        std::fs::create_dir(&other).unwrap();
        std::fs::write(&other_type, "file").unwrap();
        #[cfg(unix)]
        let link = {
            let link = repo.0.join(format!("ignore-{}", "c".repeat(32)));
            std::os::unix::fs::symlink(&other, &link).unwrap();
            link
        };
        sweep_ignore_repos(&repo.0);
        assert!(!old.exists());
        assert!(other.is_dir() && other_type.is_file());
        #[cfg(unix)]
        assert!(std::fs::symlink_metadata(link).unwrap().is_symlink());
    }

    #[test]
    fn codex_worktrees_have_a_shape() {
        assert!(codex_shaped(&native(r"E:\acc\worktrees\d6ff\repo")));
        assert!(codex_shaped("/home/u/.codex/worktrees/0a1B/app/"));
        assert!(!codex_shaped(&native(r"E:\acc\worktrees\d6ff")));
        assert!(!codex_shaped(&native(r"E:\acc\worktrees\codex-1\repo")));
        assert!(!codex_shaped(&native(r"E:\p\.codex\worktrees\codex-1a2b3c")));
    }

    #[test]
    fn claude_worktree_membership() {
        let of = |candidate: &str| is_worktree_of(&native(r"E:\p"), &native(candidate));
        assert!(of(r"E:\p\.claude\worktrees\x"));
        assert!(!of(r"E:\p\.claude\worktrees"));
        assert!(!of(r"E:\q\.claude\worktrees\x"));
        assert!(!of(r"E:\p\.codex\worktrees\codex-abc"));
        assert!(!of(r"E:\p\.claude\worktrees\..\.."), "leads back to the folder");
        assert!(!of(r"E:\p\.claude\worktrees\x\..\..\.."));
        assert!(!of(r"E:\p\.claude\worktrees\.\x"));
        assert!(!of(r"E:\p\.claude\worktrees-old\x"));
        assert!(!of(r"E:\p\.claude\worktrees\x\src"), "a part of one");
        // Another spelling of it, on the one system where it certainly is one.
        assert_eq!(is_worktree_of(r"E:\p", "e:/p/.claude/worktrees/x"), cfg!(windows));
    }

    #[test]
    fn a_folder_that_differs_by_case_is_another_folders() {
        // Only Windows is sure of it: on macOS that volume may be case-sensitive.
        let wt = native(r"E:\p\.claude\worktrees\x");
        assert_eq!(is_worktree_of(&native(r"E:\P"), &wt), cfg!(windows));
        assert_eq!(is_worktree_of(&native(r"E:\p"), &native(r"E:\p\.CLAUDE\worktrees\x")), cfg!(windows));
        let Some((root, repo, wt)) = codex_layout("case") else { return };
        assert!(is_worktree_of(&repo, &wt));
        assert_eq!(is_worktree_of(&repo.to_uppercase(), &wt), cfg!(windows), "by what its checkout points back at");
        let _ = std::fs::remove_dir_all(&root);
    }
}
