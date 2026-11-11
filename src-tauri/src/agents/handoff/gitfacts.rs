//! The state of a moved chat's working folder as git tells it, for the
//! document Luna writes (document.rs): branch and commit, `git status`, `git
//! diff --stat` against HEAD, and the subjects of the commits since the chat
//! began. No diff bodies. Each command has its own time and output limits
//! (`run`): one past its time is killed, and its output is waited for no
//! longer than that. Git runs without the repository's file monitor and
//! without the optional locks a running chat's own git would meet, and the
//! diff without external diff or text conversion drivers: nothing the
//! repository configures runs for Luna's look.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long one git command gets.
pub(super) const GIT_WAIT: Duration = Duration::from_secs(5);
/// The most of one command's output read.
const OUTPUT_MAX: u64 = 256 * 1024;
pub(super) const STATUS_LINES: usize = 200;
pub(super) const STAT_LINES: usize = 100;
pub(super) const COMMITS: usize = 20;

/// What git said of a folder.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Git {
    pub branch: Option<String>,
    pub sha: Option<String>,
    pub status: Vec<String>,
    pub status_cut: usize,
    pub stat: Vec<String>,
    pub stat_cut: usize,
    pub commits: Vec<String>,
    /// What could not be had, and why.
    pub missing: Vec<String>,
    pub at: String,
}

/// `git -C folder args`, its output as text if it ended well within `wait`,
/// with every filter driver the configuration names turned off
/// (`filter_overrides`): `git status` and `git diff` run a file's clean or
/// process filter on what they compare, as Luna's user, outside any CLI's
/// sandbox.
/// Where the drivers cannot be told for certain, git is not asked at all.
#[cfg(test)]
pub(super) fn run(folder: &str, args: &[&str], wait: Duration) -> Result<String, String> {
    let overrides = filter_overrides(folder)?;
    run_with(folder, &overrides, args, wait)
}

/// `-c` pairs that leave each filter driver configured for `folder` with
/// no command and not required, so none runs: a driver's empty command is no
/// filter to git. Read with `git config`, which runs nothing.
/// Fails closed: a listing that failed, or was cut at the output's cap, may
/// leave a driver out, and then nothing is run.
fn filter_overrides(folder: &str) -> Result<Vec<String>, String> {
    let (code, bytes) = run_out(folder, &[], &["config", "--name-only", "--get-regexp", r"^filter\."], GIT_WAIT)?;
    overrides_of(code, &bytes)
}

/// The overrides from `git config --get-regexp`'s exit code and output: 1 is
/// no match, anything else but 0 a listing not to be trusted.
fn overrides_of(code: i32, bytes: &[u8]) -> Result<Vec<String>, String> {
    if bytes.len() as u64 >= OUTPUT_MAX {
        return Err("the repository names more filters than Luna reads".into());
    }
    let listed = match code {
        0 => String::from_utf8_lossy(bytes).into_owned(),
        1 => String::new(),
        _ => return Err("git could not list the repository's filters".into()),
    };
    let mut drivers: Vec<String> = Vec::new();
    for name in listed.lines() {
        // filter.<driver>.<key>, the driver's name possibly holding dots.
        let Some(rest) = name.trim().strip_prefix("filter.") else { continue };
        let Some((driver, _)) = rest.rsplit_once('.') else { continue };
        if !driver.is_empty() && !drivers.iter().any(|d| d == driver) {
            drivers.push(driver.to_string());
        }
    }
    let mut out = Vec::new();
    for d in drivers {
        for (key, value) in [("clean", ""), ("smudge", ""), ("process", ""), ("required", "false")] {
            out.push("-c".to_string());
            out.push(format!("filter.{d}.{key}={value}"));
        }
    }
    Ok(out)
}

/// `run` with `overrides` given before the command.
fn run_with(folder: &str, overrides: &[String], args: &[&str], wait: Duration) -> Result<String, String> {
    match run_out(folder, overrides, args, wait)? {
        (0, bytes) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
        _ => Err(format!("git {} failed", args.first().unwrap_or(&""))),
    }
}

/// What the reader handed back, or why there is nothing to go by.
fn output_of(got: Result<Result<Vec<u8>, String>, std::sync::mpsc::RecvTimeoutError>) -> Result<Vec<u8>, String> {
    match got {
        Ok(Ok(bytes)) => Ok(bytes),
        Ok(Err(e)) => Err(format!("git's output could not be read: {e}")),
        Err(_) => Err("git's output did not come back in time".into()),
    }
}

/// git run as `run_with` says: its exit code and its output, up to the cap.
fn run_out(folder: &str, overrides: &[String], args: &[&str], wait: Duration) -> Result<(i32, Vec<u8>), String> {
    let mut c = Command::new("git");
    c.arg("-C").arg(folder).args(["-c", "core.fsmonitor=false", "--no-optional-locks"]);
    c.args(overrides).args(args);
    // No system configuration, no prompt, nothing an environment variable of
    // git's would run or redirect.
    c.env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_TERMINAL_PROMPT", "0");
    for var in ["GIT_EXTERNAL_DIFF", "GIT_DIR", "GIT_WORK_TREE", "GIT_CONFIG_PARAMETERS", "GIT_CONFIG_COUNT", "GIT_ASKPASS", "SSH_ASKPASS"]
    {
        c.env_remove(var);
    }
    c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = c.spawn().map_err(|e| format!("git did not start: {e}"))?;
    let mut out = child.stdout.take().ok_or("git gave no output")?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("luna-move-git".into())
        .spawn(move || {
            let mut buf = Vec::new();
            let read = out.by_ref().take(OUTPUT_MAX).read_to_end(&mut buf).map(|_| buf).map_err(|e| e.to_string());
            let _ = tx.send(read);
            // What is past the cap is read and dropped, so git is not left
            // blocked on a full pipe until it is killed.
            let _ = std::io::copy(&mut out, &mut std::io::sink());
        })
        .map_err(|e| format!("could not read git: {e}"))?;
    let deadline = Instant::now() + wait;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if Instant::now() >= deadline => {
                // The tree: what git started (a hook, a helper) goes with it.
                crate::pty::kill_tree(Some(child.id()));
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(format!("git: {e}")),
        }
    };
    // Its output, waited for no longer than its time: a child of git's that
    // kept the pipe open would hold the reader, never this.
    // A read that failed or did not come back is no output: never taken for
    // an empty one, which a filter listing would read as no filters.
    let bytes = output_of(rx.recv_timeout(deadline.saturating_duration_since(Instant::now()) + Duration::from_millis(200)))?;
    match status {
        None => Err(format!("git {} took longer than {} s", args.first().unwrap_or(&""), wait.as_secs())),
        Some(s) => Ok((s.code().unwrap_or(-1), bytes)),
    }
}

fn kept(text: &str, max: usize) -> (Vec<String>, usize) {
    let lines: Vec<String> = text.lines().filter(|l| !l.trim().is_empty()).map(str::to_owned).collect();
    let cut = lines.len().saturating_sub(max);
    (lines.into_iter().take(max).collect(), cut)
}

/// What git says of `folder` now; `since` (seconds since the epoch) is when
/// the chat began, for its commits.
pub(super) fn of(folder: &str, since: Option<i64>) -> Git {
    let mut g = Git { at: crate::background::iso(crate::throttle::now_ms()).unwrap_or_default(), ..Default::default() };
    // Nothing is asked of a repository whose filters Luna cannot all turn off.
    let overrides = match filter_overrides(folder) {
        Ok(o) => o,
        Err(e) => {
            g.missing.push(format!("git: {e}, so Luna asked git nothing"));
            return g;
        }
    };
    let run = |folder: &str, args: &[&str], wait: Duration| run_with(folder, &overrides, args, wait);
    match run(folder, &["rev-parse", "--abbrev-ref", "HEAD"], GIT_WAIT) {
        Ok(b) => g.branch = Some(b.trim().to_string()).filter(|b| !b.is_empty()),
        Err(e) => {
            // Not a repository, or no git: nothing more to ask.
            g.missing.push(format!("branch: {e}"));
            return g;
        }
    }
    match run(folder, &["rev-parse", "--short", "HEAD"], GIT_WAIT) {
        Ok(s) => g.sha = Some(s.trim().to_string()).filter(|s| !s.is_empty()),
        Err(e) => g.missing.push(format!("commit: {e}")),
    }
    match run(folder, &["-c", "core.quotepath=off", "status", "--porcelain", "--untracked-files=normal"], GIT_WAIT) {
        Ok(s) => (g.status, g.status_cut) = kept(&s, STATUS_LINES),
        Err(e) => g.missing.push(format!("status: {e}")),
    }
    match run(folder, &["-c", "core.quotepath=off", "diff", "--no-ext-diff", "--no-textconv", "--stat=120", "HEAD"], GIT_WAIT) {
        Ok(s) => (g.stat, g.stat_cut) = kept(&s, STAT_LINES),
        Err(e) => g.missing.push(format!("diff --stat: {e}")),
    }
    if let Some(since) = since {
        let since = format!("--since=@{since}");
        let n = format!("-n{COMMITS}");
        match run(folder, &["log", &since, &n, "--format=%h %s"], GIT_WAIT) {
            Ok(s) => g.commits = kept(&s, COMMITS).0,
            Err(e) => g.missing.push(format!("log: {e}")),
        }
    }
    g
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_that_is_no_repository_says_so_and_nothing_more() {
        let dir = std::env::temp_dir().join(format!("luna-gitfacts-none-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let g = of(&dir.to_string_lossy(), Some(0));
        assert!(g.branch.is_none() && g.status.is_empty());
        assert_eq!(g.missing.len(), 1, "{:?}", g.missing);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_read_of_gits_output_that_failed_is_no_output() {
        use std::sync::mpsc::RecvTimeoutError;
        assert_eq!(output_of(Ok(Ok(b"x".to_vec()))), Ok(b"x".to_vec()));
        assert!(output_of(Ok(Err("broken pipe".into()))).is_err());
        assert!(output_of(Err(RecvTimeoutError::Timeout)).is_err());
        assert!(output_of(Err(RecvTimeoutError::Disconnected)).is_err());
    }

    #[test]
    fn a_filter_listing_luna_cannot_trust_stops_git_altogether() {
        assert_eq!(overrides_of(1, b""), Ok(Vec::new()), "no filter at all");
        let one = overrides_of(0, b"filter.lfs.clean\nfilter.lfs.process\n").unwrap();
        assert!(one.contains(&"filter.lfs.process=".to_string()) && one.contains(&"filter.lfs.required=false".to_string()));
        assert!(overrides_of(128, b"").is_err(), "git failed to list them");
        let full = vec![b'x'; OUTPUT_MAX as usize];
        assert!(overrides_of(0, &full).is_err(), "a listing cut at the cap may hide a driver");
    }

    #[test]
    fn a_command_past_its_time_is_killed_and_said() {
        // `git` reading a pager-less help forever is not to be had; a wait of
        // nothing is: the first look finds it still running.
        let e = run(".", &["--version"], Duration::ZERO);
        assert!(e.is_err() || e.as_deref().is_ok_and(|v| v.starts_with("git version")), "{e:?}");
        assert_eq!(kept("a\n\nb\nc\n", 2), (vec!["a".to_string(), "b".to_string()], 1));
    }

    #[test]
    fn no_filter_a_repository_configures_runs_for_lunas_look() {
        if Command::new("git").arg("--version").output().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("luna-gitfacts-filter-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let d = dir.to_string_lossy().into_owned();
        let git = |args: &[&str]| Command::new("git").arg("-C").arg(&d).args(args).output().unwrap();
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        git(&["add", "a.txt"]);
        git(&["commit", "-q", "-m", "one"]);
        // A filter that leaves a mark wherever it runs.
        let mark = dir.join("ran");
        let cmd = format!("sh -c 'touch \"{}\"; cat'", mark.to_string_lossy().replace('\\', "/"));
        git(&["config", "filter.spy.clean", &cmd]);
        git(&["config", "filter.spy.process", &cmd]);
        std::fs::write(dir.join(".gitattributes"), "*.txt filter=spy\n").unwrap();
        std::fs::write(dir.join("a.txt"), "two\n").unwrap();
        let g = of(&d, Some(0));
        assert!(g.status.iter().any(|l| l.contains("a.txt")), "{g:?}");
        assert!(!mark.exists(), "the repository's filter ran");
        assert!(filter_overrides(&d).unwrap().contains(&"filter.spy.clean=".to_string()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
