//! A checkout's HEAD read off its git folder, loose and packed refs alike, with no
//! git run; and origin's default branch fetched before Claude Code makes a worktree
//! off it.

use super::*;

/// What a checkout has checked out: the branch, none on a detached HEAD, and
/// its commit, short, none on a branch with no commit yet.
#[derive(Debug, PartialEq, Default)]
pub struct Head {
    pub branch: Option<String>,
    pub sha: Option<String>,
}

/// How many symbolic refs are followed from HEAD: as many as git does.
const REF_HOPS: usize = 5;

/// A short sha as `git log --oneline` gives one.
const SHORT_SHA: usize = 7;

/// `head_at` for the checkout a folder is in, wherever in it.
pub fn head_of(dir: &str) -> Option<Head> {
    let dir = Path::new(dir);
    // One gone would be read as the repository around it.
    if !dir.is_dir() {
        return None;
    }
    head_at(&dir.ancestors().find(|d| d.join(".git").exists())?.to_string_lossy())
}

/// What the checkout at `root` has checked out, off its `.git` alone: `luna_session_get`
/// asks this of every row of every call, and a git process per row is what it
/// must not cost. As git resolves it: from HEAD through any symbolic refs, each
/// loose or packed. None for what this does not read (reftable, whose HEAD names a
/// ref that cannot exist), cannot read (a file it may not open) or does not lead to
/// a commit (a cycle, a chain past REF_HOPS).
pub fn head_at(root: &str) -> Option<Head> {
    let (git_dir, common) = git_dirs(Path::new(root))?;
    let mut value = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let mut chain: Vec<String> = Vec::new();
    // The last ref of a chain is the branch: `git worktree list` says that one.
    let branch = |chain: &[String]| chain.last().and_then(|name| name.strip_prefix("refs/heads/")).map(str::to_owned);
    loop {
        let Some(name) = value.trim().strip_prefix("ref:").map(str::trim) else {
            let id = value.trim();
            return is_object_id(id).then(|| Head { branch: branch(&chain), sha: Some(id[..SHORT_SHA].to_string()) });
        };
        if chain.len() >= REF_HOPS || chain.iter().any(|seen| seen == name) || !is_ref_name(name) {
            return None;
        }
        chain.push(name.to_string());
        match read_ref(&git_dir, &common, name).ok()? {
            Some(next) => value = next,
            // Nothing on it yet.
            None => return Some(Head { branch: branch(&chain), sha: None }),
        }
    }
}

/// The repository a folder is in, as one key whichever checkout of it the
/// folder is in: its common git folder (a linked worktree's `commondir`), off
/// its files with no git run, resolved and spelled as `path_key` has it. None
/// for a folder in no repository Luna can read.
pub fn repository_key(dir: &str) -> Option<String> {
    let dir = Path::new(dir);
    let root = dir.ancestors().find(|d| d.join(".git").exists())?;
    let (_, common) = git_dirs(root)?;
    let common = std::fs::canonicalize(&common).unwrap_or(common);
    Some(norm(&common.to_string_lossy()))
}

/// A checkout's own git folder, where its HEAD is, and the one its refs are in:
/// where its `commondir` says, as for a linked worktree, the repository's. `.git`
/// may be that folder or a file naming it.
fn git_dirs(root: &Path) -> Option<(PathBuf, PathBuf)> {
    let dot_git = root.join(".git");
    let git_dir = if dot_git.is_dir() {
        dot_git
    } else {
        let text = std::fs::read_to_string(&dot_git).ok()?;
        root.join(text.trim().strip_prefix("gitdir:")?.trim())
    };
    let common = match std::fs::read_to_string(git_dir.join("commondir")) {
        Ok(text) => git_dir.join(text.trim()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => git_dir.clone(),
        Err(_) => return None,
    };
    Some((git_dir, common))
}

/// A ref's value, loose or packed: an object id, or `ref: <name>`; Ok(None) with
/// neither. These few are each worktree's own. Packed refs are older than a loose
/// one, so only a ref that is not there falls back to them: a file that cannot be
/// read is an error, not a stale commit.
fn read_ref(git_dir: &Path, common: &Path, name: &str) -> std::io::Result<Option<String>> {
    use std::io::ErrorKind::{NotADirectory, NotFound};
    let own = ["refs/bisect/", "refs/worktree/", "refs/rewritten/"].iter().any(|p| name.starts_with(p));
    let loose = if own { git_dir } else { common }.join(name);
    match std::fs::read_to_string(&loose) {
        Ok(value) => return Ok(Some(value)),
        // A folder: `a/b` is a loose ref, `a` a packed one. Windows calls that a denial.
        Err(e) if matches!(e.kind(), NotFound | NotADirectory) || loose.is_dir() => {}
        Err(e) => return Err(e),
    }
    let packed = match std::fs::read_to_string(common.join("packed-refs")) {
        Ok(packed) => packed,
        Err(e) if e.kind() == NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    Ok(packed.lines().filter(|l| !l.starts_with(['#', '^'])).find_map(|l| {
        let (id, packed_name) = l.split_once(' ')?;
        (packed_name.trim() == name).then(|| id.to_string())
    }))
}

/// A ref as git would have it, so that what a file says is never a way out of
/// the git folder: its parts start with no dot, as `.invalid` of a reftable does.
fn is_ref_name(name: &str) -> bool {
    name.starts_with("refs/") && !name.contains(['\\', ':']) && name.split('/').all(|part| !part.is_empty() && !part.starts_with('.'))
}

/// SHA-1 or SHA-256, in hex.
fn is_object_id(id: &str) -> bool {
    matches!(id.len(), 40 | 64) && id.chars().all(|c| c.is_ascii_hexdigit())
}

/// origin's default branch as this clone last heard of it (`origin/main`),
/// if it has an origin that says.
fn origin_default(folder: &str) -> Option<String> {
    let out = git(folder).args(["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"]).output().ok()?;
    let r = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && r.starts_with("origin/")).then_some(r)
}

/// The longest a fetch before a worktree may hold up the chat it is for.
const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Brings origin's default branch up to date before Claude Code makes a
/// worktree off it. Claude Code bases its worktree on that branch but fetches it only when
/// the last fetch is a day old, so a repo fetched in the morning gave every
/// worktree that day the morning's tree. Only remote-tracking refs move.
/// Never interactive and bounded: a fetch that wants a password, or a network
/// that is not there, is given up on and the chat starts on what there is.
pub fn fetch_origin(folder: &str) {
    if !crate::settings::get().fetch_before_worktree
        || !git(folder).args(["remote", "get-url", "origin"]).output().is_ok_and(|o| o.status.success())
    {
        return;
    }
    // One at a time: a new chat's pane and its spawn can both get here, and
    // two fetches into one repo fail on each other's ref lock. The second
    // finds nothing left to fetch.
    static FETCHING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = FETCHING.lock().unwrap_or_else(|e| e.into_inner());
    let branch = origin_default(folder);
    let deadline = std::time::Instant::now() + FETCH_TIMEOUT;
    for attempt in 1..=2 {
        let mut cmd = git(folder);
        cmd.args(["fetch", "--quiet", "origin"]);
        if let Some(branch) = &branch {
            cmd.arg(&branch["origin/".len()..]);
        }
        cmd.env("GIT_TERMINAL_PROMPT", "0")
            .env("GCM_INTERACTIVE", "never")
            // Its message is read below.
            .env("LC_ALL", "C")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped());
        let Ok(mut child) = cmd.spawn() else { return };
        let (status, why) = loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => return,
                Ok(Some(status)) => {
                    let mut why = String::new();
                    if let Some(mut err) = child.stderr.take() {
                        let _ = std::io::Read::read_to_string(&mut err, &mut why);
                    }
                    break (status, why);
                }
                Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(std::time::Duration::from_millis(100)),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    crate::log::warn(
                        "worktree",
                        &format!("fetch before a worktree in {folder} took over {FETCH_TIMEOUT:?}; using origin as it was"),
                    );
                    return;
                }
            }
        };
        // Another git updating the same ref at that moment — an editor's own
        // fetch, Claude Code's — is done with it in moments, and a fetch
        // after it is all this needs.
        let retry = std::time::Duration::from_millis(500);
        if attempt == 1 && why.contains("cannot lock ref") && std::time::Instant::now() + retry < deadline {
            std::thread::sleep(retry);
            continue;
        }
        crate::log::warn(
            "worktree",
            &format!("fetch before a worktree in {folder} failed ({status}): {}; using origin as it was", why.trim()),
        );
        return;
    }
}

#[tauri::command]
pub fn fetch_before_worktree() -> bool {
    crate::settings::get().fetch_before_worktree
}

#[tauri::command]
pub fn set_fetch_before_worktree(on: bool) -> Result<bool, String> {
    crate::settings::update(|s| s.fetch_before_worktree = on)?;
    Ok(on)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway checkout's `.git`, with these files in it.
    fn git_folder(tag: &str, files: &[(&str, &str)]) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("luna-head-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let git = root.join(".git");
        std::fs::create_dir_all(&git).unwrap();
        for (name, text) in files {
            let file = git.join(name);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
        }
        (root, git)
    }

    const ID: &str = "0123456789abcdef0123456789abcdef01234567";

    fn on(branch: Option<&str>, sha: Option<&str>) -> Option<Head> {
        Some(Head { branch: branch.map(str::to_owned), sha: sha.map(str::to_owned) })
    }

    #[test]
    fn a_checkouts_head_is_read_off_its_git_folder() {
        let id = format!("{ID}\n");
        let cases: Vec<(&str, Vec<(&str, &str)>, Option<Head>)> = vec![
            ("loose", vec![("HEAD", "ref: refs/heads/main\n"), ("refs/heads/main", &id)], on(Some("main"), Some("0123456"))),
            ("nested", vec![("HEAD", "ref: refs/heads/fix/a\n"), ("refs/heads/fix/a", &id)], on(Some("fix/a"), Some("0123456"))),
            ("detached", vec![("HEAD", &id)], on(None, Some("0123456"))),
            ("unborn", vec![("HEAD", "ref: refs/heads/new\n")], on(Some("new"), None)),
            ("bad-id", vec![("HEAD", "ref: refs/heads/main\n"), ("refs/heads/main", "not an id\n")], None),
            ("short-id", vec![("HEAD", "0123456\n")], None),
            ("reftable", vec![("HEAD", "ref: refs/heads/.invalid\n"), ("refs/heads", "this repository uses the reftable format\n")], None),
            ("way-out", vec![("HEAD", "ref: refs/../../../secret\n")], None),
            ("no-ref", vec![("HEAD", "ref: HEAD\n")], None),
        ];
        for (tag, files, want) in cases {
            let (root, _) = git_folder(tag, &files);
            assert_eq!(head_at(&root.to_string_lossy()), want, "{tag}");
            let _ = std::fs::remove_dir_all(&root);
        }
        let sha256 = "a".repeat(64);
        let (root, _) = git_folder("sha256", &[("HEAD", &sha256)]);
        assert_eq!(head_at(&root.to_string_lossy()), on(None, Some("aaaaaaa")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_ref_is_loose_or_packed_at_every_step_of_its_chain() {
        let packed = format!("# pack-refs with: peeled fully-peeled sorted\n{ID} refs/heads/main\n^{}\n", "f".repeat(40));
        // HEAD -> loose alias -> packed branch: the branch is the last of the chain.
        let (root, _) = git_folder(
            "chain",
            &[("HEAD", "ref: refs/heads/alias\n"), ("refs/heads/alias", "ref: refs/heads/main\n"), ("packed-refs", &packed)],
        );
        assert_eq!(head_at(&root.to_string_lossy()), on(Some("main"), Some("0123456")));
        let _ = std::fs::remove_dir_all(&root);
        // A loose ref over a packed one wins, as it is the newer.
        let other = format!("{}\n", "b".repeat(40));
        let (root, _) = git_folder("over", &[("HEAD", "ref: refs/heads/main\n"), ("refs/heads/main", &other), ("packed-refs", &packed)]);
        assert_eq!(head_at(&root.to_string_lossy()), on(Some("main"), Some("bbbbbbb")));
        let _ = std::fs::remove_dir_all(&root);
        // A chain ending on nothing yet: an unborn branch, as HEAD alone would be.
        let (root, _) = git_folder("dangling", &[("HEAD", "ref: refs/heads/alias\n"), ("refs/heads/alias", "ref: refs/heads/new\n")]);
        assert_eq!(head_at(&root.to_string_lossy()), on(Some("new"), None));
        let _ = std::fs::remove_dir_all(&root);
        // Round and round, or on and on: no answer, and it ends.
        let (root, _) = git_folder(
            "cycle",
            &[("HEAD", "ref: refs/heads/a\n"), ("refs/heads/a", "ref: refs/heads/b\n"), ("refs/heads/b", "ref: refs/heads/a\n")],
        );
        assert_eq!(head_at(&root.to_string_lossy()), None);
        let _ = std::fs::remove_dir_all(&root);
        let (root, _) = git_folder("self", &[("HEAD", "ref: refs/heads/a\n"), ("refs/heads/a", "ref: refs/heads/a\n")]);
        assert_eq!(head_at(&root.to_string_lossy()), None);
        let _ = std::fs::remove_dir_all(&root);
        let mut long: Vec<(String, String)> = vec![("HEAD".into(), "ref: refs/heads/r0\n".into())];
        for n in 0..=REF_HOPS {
            long.push((format!("refs/heads/r{n}"), format!("ref: refs/heads/r{}\n", n + 1)));
        }
        long.push((format!("refs/heads/r{}", REF_HOPS + 1), format!("{ID}\n")));
        let files: Vec<(&str, &str)> = long.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        let (root, _) = git_folder("long", &files);
        assert_eq!(head_at(&root.to_string_lossy()), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A loose ref is newer than a packed one: unreadable, it does not fall back
    /// to what may be an old commit. Bytes that are no text are a read error any
    /// user gets, where a test run as root has no denied file to go by.
    #[test]
    fn a_ref_that_cannot_be_read_leaves_the_head_unknown() {
        let packed = format!("{ID} refs/heads/main\n");
        let (root, git) =
            git_folder("unreadable", &[("HEAD", "ref: refs/heads/main\n"), ("refs/heads/main", ""), ("packed-refs", &packed)]);
        std::fs::write(git.join("refs/heads/main"), [0xff, 0xfe]).unwrap();
        assert_eq!(head_at(&root.to_string_lossy()), None);
        // Gone, it is the packed one's to say.
        std::fs::remove_file(git.join("refs/heads/main")).unwrap();
        assert_eq!(head_at(&root.to_string_lossy()), on(Some("main"), Some("0123456")));
        // No packed-refs that can be read: no answer either, not a branch unborn.
        std::fs::write(git.join("packed-refs"), [0xff, 0xfe]).unwrap();
        assert_eq!(head_at(&root.to_string_lossy()), None);
        let _ = std::fs::remove_dir_all(&root);
        // And a commondir that cannot be read does not leave the wrong folder read.
        let (root, git) = git_folder("commondir", &[("HEAD", &format!("{ID}\n")), ("commondir", "")]);
        std::fs::write(git.join("commondir"), [0xff, 0xfe]).unwrap();
        assert_eq!(head_at(&root.to_string_lossy()), None);
        let _ = std::fs::remove_dir_all(&root);
        // A folder where a ref's name would be, or a file where its folder would:
        // not a read error, only no loose ref.
        let id = format!("{ID}\n");
        let packed = format!("{ID} refs/heads/fix\n{ID} refs/heads/main/old\n");
        let (root, _) =
            git_folder("in-a-folder", &[("HEAD", "ref: refs/heads/fix\n"), ("refs/heads/fix/a", &id), ("packed-refs", &packed)]);
        assert_eq!(head_at(&root.to_string_lossy()), on(Some("fix"), Some("0123456")));
        let _ = std::fs::remove_dir_all(&root);
        let (root, _) =
            git_folder("under-a-file", &[("HEAD", "ref: refs/heads/main/old\n"), ("refs/heads/main", &id), ("packed-refs", &packed)]);
        assert_eq!(head_at(&root.to_string_lossy()), on(Some("main/old"), Some("0123456")));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `commondir` says where the refs are whether `.git` is a folder or a file.
    #[test]
    fn a_git_folder_with_a_commondir_has_its_refs_there() {
        let shared = format!("{ID}\n");
        let stale = format!("{}\n", "b".repeat(40));
        let (root, git) =
            git_folder("common", &[("HEAD", "ref: refs/heads/main\n"), ("commondir", "../shared\n"), ("refs/heads/main", &stale)]);
        let refs = root.join("shared").join("refs").join("heads");
        std::fs::create_dir_all(&refs).unwrap();
        std::fs::write(refs.join("main"), &shared).unwrap();
        assert_eq!(head_at(&root.to_string_lossy()), on(Some("main"), Some("0123456")), "not the ref left in the folder itself");
        std::fs::remove_file(git.join("refs/heads/main")).unwrap();
        assert_eq!(head_at(&root.to_string_lossy()), on(Some("main"), Some("0123456")), "and not unborn with none there");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_linked_worktree_has_its_head_of_its_own_and_its_refs_from_the_repository() {
        let id = format!("{ID}\n");
        let (root, git) = git_folder(
            "linked",
            &[("refs/heads/feature", &id), ("worktrees/wt/HEAD", "ref: refs/heads/feature\n"), ("worktrees/wt/commondir", "../..\n")],
        );
        let wt = root.join("wt");
        std::fs::create_dir_all(wt.join("src")).unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", git.join("worktrees").join("wt").display())).unwrap();
        assert_eq!(head_at(&wt.to_string_lossy()), on(Some("feature"), Some("0123456")));
        // Anywhere in it; and one gone is no reason to read the repository around it.
        assert_eq!(head_of(&wt.join("src").to_string_lossy()), on(Some("feature"), Some("0123456")));
        assert_eq!(head_of(&wt.join("gone").to_string_lossy()), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// What it reads is what git says, where this runs with git.
    #[test]
    fn a_head_read_off_the_files_is_what_git_says() {
        let root = std::env::temp_dir().join(format!("luna-head-real-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let repo = root.to_string_lossy().into_owned();
        let run = |dir: &str, args: &[&str]| -> Option<String> {
            let o = git(dir).args(["-c", "user.email=t@t", "-c", "user.name=t"]).args(args).output().ok()?;
            o.status.success().then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
        };
        if run(&repo, &["init", "-q"]).is_none() || run(&repo, &["commit", "-q", "--allow-empty", "-m", "i"]).is_none() {
            let _ = std::fs::remove_dir_all(&root);
            return;
        }
        let says = |dir: &str| {
            on(run(dir, &["symbolic-ref", "--short", "-q", "HEAD"]).as_deref(), run(dir, &["rev-parse", "--short=7", "HEAD"]).as_deref())
        };
        assert_eq!(head_at(&repo), says(&repo));
        let wt = root.join("wt").to_string_lossy().into_owned();
        run(&repo, &["worktree", "add", "-q", "-b", "side", &wt]).unwrap();
        assert_eq!(head_at(&wt), says(&wt));
        // Packed, from the worktree too; then detached there.
        run(&repo, &["pack-refs", "--all"]).unwrap();
        assert_eq!(head_at(&wt), says(&wt));
        run(&wt, &["checkout", "-q", "--detach"]).unwrap();
        assert_eq!(head_at(&wt), says(&wt));
        let _ = run(&repo, &["worktree", "remove", "--force", &wt]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
