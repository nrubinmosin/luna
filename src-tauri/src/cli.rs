use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How long `--new-chat` waits for the app to confirm the session started.
const WAIT: Duration = Duration::from_secs(25);
const POLL: Duration = Duration::from_millis(200);

/// A chat the app was asked to open from the command line.
///
/// The permission mode is deliberately absent: it is the app's own default, not
/// something the caller chooses. Anything able to run this command could
/// otherwise start a full-access session without the user being involved.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct NewChatRequest {
    /// Identifies the acknowledgement file this request is waiting on.
    pub id: String,
    pub folder: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub worktree: Option<bool>,
    pub account: Option<String>,
    /// First message to type into the session once it is up.
    pub prompt: Option<String>,
}

pub fn requests_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("llm-desktop")
        .join("requests")
}

fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

/// Parses `--new-chat --folder <path> [--model ...] [--prompt "..."]`.
/// Returns None when the arguments are not a new-chat request — an ordinary
/// launch, or a second launch that just wants the window focused.
pub fn parse_new_chat(args: &[String]) -> Option<NewChatRequest> {
    if !args.iter().any(|a| a == "--new-chat") {
        return None;
    }

    let value = |name: &str| -> Option<String> {
        let i = args.iter().position(|a| a == name)?;
        let v = args.get(i + 1)?;
        // A missing value would otherwise swallow the next flag.
        if v.starts_with("--") {
            return None;
        }
        Some(v.clone())
    };

    let folder = value("--folder")?;
    if folder.trim().is_empty() {
        return None;
    }

    Some(NewChatRequest {
        id: format!("{}-{}", now_ms(), std::process::id()),
        folder,
        model: value("--model"),
        effort: value("--effort"),
        // --worktree opts in, --no-worktree opts out, absent leaves the default.
        worktree: if args.iter().any(|a| a == "--no-worktree") {
            Some(false)
        } else if args.iter().any(|a| a == "--worktree") {
            Some(true)
        } else {
            None
        },
        account: value("--account"),
        prompt: value("--prompt"),
    })
}

/// The `--new-chat` process: drop the request where the running app will see
/// it, then block until the app says whether the session started.
///
/// This is why the transport is a file rather than the second launch's argv —
/// argv is one-way, so the caller could only ever learn that it *asked*, never
/// that it worked.
pub fn submit_and_wait(req: &NewChatRequest) -> Result<(), String> {
    if !std::path::Path::new(&req.folder).is_dir() {
        return Err(format!("folder not found: {}", req.folder));
    }

    let dir = requests_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;

    let ack = dir.join(format!("{}.done", req.id));
    let _ = std::fs::remove_file(&ack);

    // Write beside the target and rename in, so the watcher can never observe
    // a half-written request.
    let tmp = dir.join(format!("{}.part", req.id));
    let body = serde_json::to_vec_pretty(req).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, body).map_err(|e| format!("cannot write request: {e}"))?;
    std::fs::rename(&tmp, dir.join(format!("{}.json", req.id)))
        .map_err(|e| format!("cannot submit request: {e}"))?;

    let started = Instant::now();
    while started.elapsed() < WAIT {
        if let Ok(answer) = std::fs::read_to_string(&ack) {
            let _ = std::fs::remove_file(&ack);
            let answer = answer.trim().to_string();
            return if answer == "ok" { Ok(()) } else { Err(answer) };
        }
        std::thread::sleep(POLL);
    }

    // Leave the request in place: the app picks up anything pending at startup,
    // so a chat asked for while it was closed still opens on next launch.
    Err("timed out waiting for llm-desktop — is it running?".into())
}

/// App side: hand back the outcome the waiting process is blocked on.
pub fn acknowledge(id: &str, error: Option<&str>) {
    let dir = requests_dir();
    let _ = std::fs::remove_file(dir.join(format!("{id}.json")));
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join(format!("{id}.done")), error.unwrap_or("ok"));
}

/// App side: every request currently waiting to be opened, oldest first.
pub fn pending_requests() -> Vec<NewChatRequest> {
    let dir = requests_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else { return vec![] };

    let mut out: Vec<NewChatRequest> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| {
            let text = std::fs::read_to_string(e.path()).ok()?;
            serde_json::from_str::<NewChatRequest>(&text).ok()
        })
        .collect();

    // Ids start with a millisecond timestamp, so this is submission order.
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// Drops acknowledgements and requests nobody came back for.
pub fn sweep_stale() {
    let dir = requests_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .map(|t| t.elapsed().map(|age| age > Duration::from_secs(3600)).unwrap_or(false))
            .unwrap_or(false);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn ignores_an_ordinary_launch() {
        assert!(parse_new_chat(&args(&["llm-desktop.exe"])).is_none());
    }

    #[test]
    fn requires_a_folder() {
        assert!(parse_new_chat(&args(&["x", "--new-chat"])).is_none());
        assert!(parse_new_chat(&args(&["x", "--new-chat", "--folder"])).is_none());
        // A flag where the value should be is a missing value, not a folder.
        assert!(parse_new_chat(&args(&["x", "--new-chat", "--folder", "--model"])).is_none());
    }

    #[test]
    fn reads_the_options() {
        let r = parse_new_chat(&args(&[
            "x", "--new-chat", "--folder", "E:\\P\\app", "--model", "opus", "--prompt", "fix the build",
        ]))
        .unwrap();
        assert_eq!(r.folder, "E:\\P\\app");
        assert_eq!(r.model.as_deref(), Some("opus"));
        assert_eq!(r.prompt.as_deref(), Some("fix the build"));
        assert_eq!(r.worktree, None);
        assert!(!r.id.is_empty());
    }

    #[test]
    fn worktree_opts_both_ways() {
        let on = parse_new_chat(&args(&["x", "--new-chat", "--folder", "f", "--worktree"])).unwrap();
        assert_eq!(on.worktree, Some(true));
        let off = parse_new_chat(&args(&["x", "--new-chat", "--folder", "f", "--no-worktree"])).unwrap();
        assert_eq!(off.worktree, Some(false));
    }
}
