//! Which project a folder belongs to, and the argument that asks a CLI for a
//! worktree. Luna makes, inspects and removes no worktree: each CLI makes its
//! own from `--worktree` (Claude Code under `<folder>/.claude/worktrees/<name>`
//! on a branch `worktree-<name>`, Codex under `<CODEX_HOME>/worktrees/<4 hex>/<repo>`
//! on a detached HEAD), and what is there afterwards is the user's. This reads
//! only where a session's folder is: a session in such a worktree belongs to
//! the project it was made from.

use crate::paths::{path_key as norm, KEY_SEP};
use std::path::{Path, PathBuf};
use std::process::Command;

fn git(folder: &str) -> Command {
    let mut c = Command::new("git");
    c.arg("-C").arg(folder);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
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

/// The repository a linked worktree belongs to, off its `.git` file
/// (`gitdir: <repo>/.git/worktrees/<id>`). None when the worktree is gone or
/// is not a linked one.
pub(crate) fn repo_of_linked(worktree: &str) -> Option<String> {
    let text = std::fs::read_to_string(Path::new(worktree).join(".git")).ok()?;
    repo_of_linked_text(worktree, &text)
}

/// `repo_of_linked` off the `.git` file's text: the text alone, nothing looked at.
fn repo_of_linked_text(worktree: &str, text: &str) -> Option<String> {
    let gitdir = PathBuf::from(text.trim().strip_prefix("gitdir:")?.trim());
    let gitdir = if gitdir.is_absolute() { gitdir } else { Path::new(worktree).join(gitdir) };
    // <repo>/.git/worktrees/<id> → <repo>. A submodule's points at `.git/modules`.
    let worktrees = gitdir.parent()?;
    let dot_git = worktrees.parent()?;
    if worktrees.file_name()? != "worktrees" || dot_git.file_name()? != ".git" {
        return None;
    }
    // The file is the checkout writer's: a repository it names on a share the
    // worktree is not on is none, before anything there is looked at.
    dot_git.parent().map(|r| r.to_string_lossy().into_owned()).filter(|r| !crate::boundary::foreign_share(r, worktree))
}

/// True when `candidate` is a worktree a CLI made for this folder: under its
/// `.claude/worktrees`, or one Codex made (`codex_made`) whose checkout points
/// back at it.
pub fn is_worktree_of(folder: &str, candidate: &str) -> bool {
    under_claude_worktrees(folder, candidate)
        || (!has_dot_parts(candidate) && codex_made(candidate) && repo_of_linked(candidate).is_some_and(|r| same_place(&r, folder)))
}

/// The project folder a CLI-made worktree belongs to, given that worktree or a
/// folder of its checkout: for Claude Code the part of the path before
/// `.claude\worktrees`, for Codex the repository its checkout points back at. The
/// shape of a path only says where to look: its `.git` file says whether that is a
/// worktree of the repository. Taken by shape alone, any folder under a worktree (a
/// test project in its `test-build`) read as the main checkout. One read of a file, and git
/// only for a folder below a worktree's root, which nothing asks after on a beat.
pub fn project_of(path: &str) -> Option<String> {
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
        if !Path::new(root).exists() {
            // Gone: Claude Code removes a clean one as its session quits, and makes
            // it again by name. Where it was is all there is to go by, in a repository
            // or somewhere itself gone.
            if !Path::new(project).exists() || Path::new(project).join(".git").exists() {
                return Some(project.to_string());
            }
            continue;
        }
        let Some(repo) = repo_of_linked(root).filter(|r| Path::new(r).is_dir()) else { continue };
        if !in_checkout(root, below[name..].trim_matches(['\\', '/'])) {
            break;
        }
        return Some(if same_place(&repo, project) { project.to_string() } else { repo });
    }
    codex_shaped(path).then(|| repo_of_linked(path)).flatten()
}

/// Whether a folder below a worktree's root (`rest`, empty for the root) is part of
/// its checkout: no repository of its own on the way down, and nothing git ignores,
/// as build output and test folders are.
fn in_checkout(root: &str, rest: &str) -> bool {
    if rest.is_empty() {
        return true;
    }
    let mut dir = PathBuf::from(root);
    for part in rest.split(['\\', '/']).filter(|c| !c.is_empty()) {
        dir.push(part);
        if dir.join(".git").exists() {
            return false;
        }
    }
    // 0: ignored, 1: not. Anything else says nothing, and it goes as its root does.
    !git(root).args(["check-ignore", "-q", "--", &rest.replace('\\', "/")]).output().is_ok_and(|o| o.status.code() == Some(0))
}

/// Whether two paths lead to one folder, however each is written. What git writes
/// (a `.git` file) has its long names, and a folder may come spelled in 8.3 short
/// ones (`C:\Users\RUNNER~1\…`, as a temp folder under a long user name is): only
/// the file system can tell those are one.
pub fn same_place(a: &str, b: &str) -> bool {
    norm(a) == norm(b) || matches!((std::fs::canonicalize(a), std::fs::canonicalize(b)), (Ok(x), Ok(y)) if x == y)
}

/// The CLI-made worktree `path` is in, itself or a folder of its checkout, with
/// the project it belongs to: a session's cwd follows its shell's `cd` (Claude Code
/// moves its own with it), and where it works is the worktree all the same. Its
/// root is the nearest folder up that is one of the project's worktrees by shape,
/// or by its `.git` file (one made from inside another worktree). `project_of`
/// knows a Codex worktree by its root alone, so a folder of one is looked up from.
pub fn root_of(path: &str) -> Option<(String, String)> {
    let codex =
        || Path::new(path).ancestors().map(|d| d.to_string_lossy().into_owned()).find(|d| codex_shaped(d) && repo_of_linked(d).is_some());
    let project = project_of(path).or_else(|| project_of(&codex()?))?;
    let mut dir = Path::new(path);
    loop {
        let d = dir.to_string_lossy();
        if is_worktree_of(&project, &d) || repo_of_linked(&d).is_some_and(|r| same_place(&r, &project)) {
            return Some((d.into_owned(), project));
        }
        dir = dir.parent()?;
    }
}

/// The 8.3 short spelling of an existing folder where its volume keeps one, else
/// the path as it is: what git says of a repository comes in long names, and a
/// test spelling its own in short ones has Luna tell the two apart as a user's
/// temp folder under a long user name (`C:\Users\RUNNER~1\…`) would.
#[cfg(test)]
pub(crate) fn short_spelling(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // `%~sI` is cmd's own GetShortPathName.
        let out = Command::new("cmd").raw_arg(format!("/d /c for %I in (\"{}\") do @echo %~sI", path.display())).output();
        if let Some(short) = out.ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()) {
            if !short.is_empty() && Path::new(&short).is_dir() {
                return PathBuf::from(short);
            }
        }
    }
    path.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(repo_of_linked(&s(&claude)), None);
        assert_eq!(repo_of_linked(&s(&codex)), None);
        assert_eq!(project_of(&s(&claude.join("src"))), None, "below a worktree whose repository is a share");
        assert_eq!(project_of(&s(&codex)), None);
        assert_eq!(root_of(&s(&codex.join("src"))), None);
        // A worktree on the share it names is that host's already.
        assert_eq!(
            repo_of_linked_text(r"\\luna-no-such-host\share\wt", r"gitdir: \\luna-no-such-host\share\repo\.git\worktrees\w").as_deref(),
            Some(r"\\luna-no-such-host\share\repo")
        );
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
        let root = std::env::temp_dir().join(format!("luna-wt-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).ok()?;
        let root = short_spelling(&root);
        TEST_CODEX_ACCOUNTS.lock().unwrap().push(root.to_string_lossy().into_owned());
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).ok()?;
        let repo_s = repo.to_string_lossy().into_owned();
        let ok = |c: &mut Command| c.output().is_ok_and(|o| o.status.success());
        if !ok(git(&repo_s).args(["init", "-q"]))
            || !ok(git(&repo_s).args(["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "i"]))
        {
            return None;
        }
        let wt = root.join("account").join("worktrees").join("0a1b").join("repo");
        std::fs::create_dir_all(wt.parent()?).ok()?;
        let wt_s = wt.to_string_lossy().into_owned();
        ok(git(&repo_s).args(["worktree", "add", "-q", "--detach", &wt_s])).then_some((root, repo_s, wt_s))
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
        assert_eq!(project_of(&native("E:/p/x/.claude/worktrees/a-b-c")), Some(native(r"E:\p\x")));
        assert_eq!(project_of(&native(r"E:\p\x")), None);
        let Some((root, repo, wt)) = codex_layout("project") else { return };
        assert_eq!(project_of(&wt).map(|p| place(&p)), Some(place(&repo)));
        assert!(is_worktree_of(&repo, &wt));
        assert!(!is_worktree_of(&root.to_string_lossy(), &wt), "another folder's");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_folder_is_of_a_worktree_only_if_git_says_so_whatever_its_path_looks_like() {
        let root = std::env::temp_dir().join(format!("luna-wt-of-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let root = short_spelling(&root);
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let ok = |dir: &Path, args: &[&str]| {
            git(&s(dir)).args(["-c", "user.email=t@t", "-c", "user.name=t"]).args(args).output().is_ok_and(|o| o.status.success())
        };
        if !ok(&repo, &["init", "-q"]) {
            return;
        }
        std::fs::write(repo.join(".gitignore"), "test-build/\n").unwrap();
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

        // What its git ignores is no part of it: the test folder this is about.
        let plain = wt.join("test-build").join("harness").join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        assert_eq!(of(&plain), None, "a plain folder under <wt>\\test-build");
        assert_eq!(of(&wt.join("test-build")), None);

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
        assert!(git(&repo).args(["worktree", "add", "-q", "-b", "worktree-real", &s(&claude)]).output().is_ok_and(|o| o.status.success()));
        std::fs::create_dir_all(claude.join("src").join("deep")).unwrap();
        assert_eq!(root(&claude.join("src").join("deep")), Some((place(&s(&claude)), place(&repo))));
        let _ = std::fs::remove_dir_all(&tmp);
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
