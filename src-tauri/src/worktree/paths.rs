//! Which path is a worktree of which folder: Claude Code's by its place under the
//! folder, Codex's by its shape in an account and the `.git` file pointing back,
//! either only as git records it; and the project a path is of.

use super::*;

pub(super) fn claude_worktrees(folder: &str) -> PathBuf {
    Path::new(folder).join(".claude").join("worktrees")
}

/// Whether the path has a `.` or `..` in it: the text it reads as is not
/// where it leads, and nothing is deleted by such a path.
fn has_dot_parts(path: &str) -> bool {
    path.split(['\\', '/']).any(|c| c == "." || c == "..")
}

pub(super) fn under_claude_worktrees(folder: &str, candidate: &str) -> bool {
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
/// is where Codex puts what `--worktree` makes (seen on 0.167.0:
/// `$CODEX_HOME/worktrees/<4 hex>/<repo>`, detached). That shape anywhere
/// else is a folder nobody can show Codex made, and nothing deletes it as one.
pub(super) fn codex_made(path: &str) -> bool {
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
pub(super) static TEST_CODEX_ACCOUNTS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Whether git keeps `path` as one of `folder`'s worktrees (`git worktree
/// list`). Anyone can write a `.git` file pointing at a repository; git's own
/// record takes a write into that repository.
pub(super) fn registered(folder: &str, path: &str) -> bool {
    listed(folder).iter().any(|(p, _)| same_place(p, path))
}

/// Each worktree of `folder` as git lists it, with the branch it has checked
/// out. From `--porcelain -z`, whose fields end in NUL: a path is the one thing
/// in it anyone can name, and a newline in one would read, line by line, as a
/// record of its own. A field that is not UTF-8 is left out rather than read
/// lossily into something it is not.
fn listed(folder: &str) -> Vec<(String, Option<String>)> {
    let Ok(out) = git(folder).args(["worktree", "list", "--porcelain", "-z"]).output() else { return Vec::new() };
    if !out.status.success() {
        return Vec::new();
    }
    let mut all: Vec<(String, Option<String>)> = Vec::new();
    for field in out.stdout.split(|&b| b == 0).filter_map(|f| std::str::from_utf8(f).ok()) {
        if let Some(p) = field.strip_prefix("worktree ") {
            all.push((p.to_string(), None));
        } else if let (Some(b), Some(last)) = (field.strip_prefix("branch refs/heads/"), all.last_mut()) {
            last.1 = Some(b.to_string());
        }
    }
    all
}

/// The repository a linked worktree belongs to, off its `.git` file
/// (`gitdir: <repo>/.git/worktrees/<id>`). None when the worktree is gone or
/// is not a linked one.
fn repo_of_linked(worktree: &str) -> Option<String> {
    let text = std::fs::read_to_string(Path::new(worktree).join(".git")).ok()?;
    let gitdir = PathBuf::from(text.trim().strip_prefix("gitdir:")?.trim());
    let gitdir = if gitdir.is_absolute() { gitdir } else { Path::new(worktree).join(gitdir) };
    // <repo>/.git/worktrees/<id> → <repo>. A submodule's points at `.git/modules`.
    let worktrees = gitdir.parent()?;
    let dot_git = worktrees.parent()?;
    if worktrees.file_name()? != "worktrees" || dot_git.file_name()? != ".git" {
        return None;
    }
    dot_git.parent().map(|r| r.to_string_lossy().into_owned())
}

/// True when `candidate` is a worktree a CLI made for this folder: under its
/// `.claude/worktrees`, or one Codex made (`codex_made`) whose checkout points
/// back at it. Every destructive path below is gated on this.
pub fn is_worktree_of(folder: &str, candidate: &str) -> bool {
    under_claude_worktrees(folder, candidate)
        || (!has_dot_parts(candidate) && codex_made(candidate) && repo_of_linked(candidate).is_some_and(|r| same_place(&r, folder)))
}

/// The project folder a CLI-made worktree belongs to, given that worktree or a
/// folder of its checkout: for Claude Code the part of the path before
/// `.claude\worktrees`, for Codex the repository its checkout points back at. The
/// shape of a path only says where to look: its `.git` file says whether that is a
/// worktree of the repository. Taken by shape alone, any folder under a worktree (a
/// test project in its `test-build`) read as the main checkout, and a helper sent there
/// with a worktree of its own made it in that checkout. One read of a file, and git
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
/// (a `.git` file, `git worktree list`) has its long names, and a folder may come
/// spelled in 8.3 short ones (`C:\Users\RUNNER~1\…`, as a temp folder under a long
/// user name is): only the file system can tell those are one.
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

/// Branch checked out in the given worktree, per `git worktree list`.
pub fn branch_of(folder: &str, worktree_path: &str) -> Option<String> {
    listed(folder).into_iter().find(|(p, b)| b.is_some() && same_place(p, worktree_path)).and_then(|(_, b)| b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_worktree_path_names_its_project() {
        assert_eq!(project_of(&native("E:/p/x/.claude/worktrees/a-b-c")), Some(native(r"E:\p\x")));
        assert_eq!(project_of(&native(r"E:\p\x")), None);
        let Some((root, repo, wt)) = codex_layout("project") else { return };
        assert_eq!(project_of(&wt).map(|p| place(&p)), Some(place(&repo)));
        assert!(is_worktree_of(&repo, &wt));
        assert!(!is_worktree_of(&root.to_string_lossy(), &wt), "another folder's");
        let account = root.join("account").to_string_lossy().into_owned();
        assert_eq!(candidates(&repo, &[account]).len(), 1);
        remove_worktree_now(repo.clone(), wt.clone()).unwrap();
        assert!(!root.join("account").join("worktrees").join("0a1b").exists(), "the bucket goes too");
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
        let _ = remove_worktree_now(repo.clone(), codex);
        let _ = remove_worktree_now(repo, s(&claude));
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

    /// A worktree whose path has a newline and what reads, line by line, as
    /// another worktree's record: that other is no worktree of git's.
    #[cfg(unix)]
    #[test]
    fn a_path_cannot_write_a_record_of_gits_into_its_list() {
        let Some((root, repo, wt)) = codex_layout("newline") else { return };
        let forged = root.join("account").join("worktrees").join("eeee").join("repo");
        std::fs::create_dir_all(&forged).unwrap();
        std::fs::write(forged.join(".git"), std::fs::read_to_string(Path::new(&wt).join(".git")).unwrap()).unwrap();
        let forged = forged.to_string_lossy().into_owned();
        let evil = root.join(format!("evil\nworktree {forged}\nHEAD {}\ndetached", "0".repeat(40)));
        let added = git(&repo).args(["worktree", "add", "-q", "--detach"]).arg(&evil).output().unwrap();
        assert!(added.status.success(), "{}", String::from_utf8_lossy(&added.stderr));
        assert!(registered(&repo, &evil.to_string_lossy()), "the odd one is git's");
        assert!(is_worktree_of(&repo, &forged) && !registered(&repo, &forged), "the one it names is not");
        assert!(remove_worktree_now(repo.clone(), forged.clone()).is_err());
        assert!(Path::new(&forged).exists());
        let _ = remove_worktree_now(repo.clone(), wt);
        let _ = git(&repo).args(["worktree", "remove", "--force"]).arg(&evil).output();
        let _ = std::fs::remove_dir_all(&root);
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
    fn a_folder_that_differs_by_case_is_another_folders_before_anything_is_deleted() {
        // The guard: only Windows is sure of it. On macOS that volume may be
        // case-sensitive, and the worse mistake is deleting another's worktree.
        let wt = native(r"E:\p\.claude\worktrees\x");
        assert_eq!(is_worktree_of(&native(r"E:\P"), &wt), cfg!(windows));
        assert_eq!(is_worktree_of(&native(r"E:\p"), &native(r"E:\p\.CLAUDE\worktrees\x")), cfg!(windows));
        let Some((root, repo, wt)) = codex_layout("case") else { return };
        assert!(is_worktree_of(&repo, &wt));
        assert_eq!(is_worktree_of(&repo.to_uppercase(), &wt), cfg!(windows), "by what its checkout points back at");
        let _ = std::fs::remove_dir_all(&root);
    }
}
