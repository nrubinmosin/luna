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

/// `git -C folder args`, its output as text if it ended well within `wait`.
pub(super) fn run(folder: &str, args: &[&str], wait: Duration) -> Result<String, String> {
    let mut c = Command::new("git");
    c.arg("-C").arg(folder).args(["-c", "core.fsmonitor=false", "--no-optional-locks"]).args(args);
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
            let _ = out.by_ref().take(OUTPUT_MAX).read_to_end(&mut buf);
            let _ = tx.send(buf);
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
    let bytes = rx.recv_timeout(deadline.saturating_duration_since(Instant::now()) + Duration::from_millis(200)).unwrap_or_default();
    match status {
        None => Err(format!("git {} took longer than {} s", args.first().unwrap_or(&""), wait.as_secs())),
        Some(s) if !s.success() => Err(format!("git {} failed", args.first().unwrap_or(&""))),
        Some(_) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
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
    fn a_command_past_its_time_is_killed_and_said() {
        // `git` reading a pager-less help forever is not to be had; a wait of
        // nothing is: the first look finds it still running.
        let e = run(".", &["--version"], Duration::ZERO);
        assert!(e.is_err() || e.as_deref().is_ok_and(|v| v.starts_with("git version")), "{e:?}");
        assert_eq!(kept("a\n\nb\nc\n", 2), (vec!["a".to_string(), "b".to_string()], 1));
    }
}
