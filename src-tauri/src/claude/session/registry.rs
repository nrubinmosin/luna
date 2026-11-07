//! The live registry at `<config>/sessions/<pid>.json` — name, status, cwd,
//! session id — and what a live session is found and described by through it.

use super::*;

fn parse_session_file(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Name, status, cwd, session id, context, title and opening prompt of a live
/// session, off the registry entry the CLI writes a moment after the pty
/// starts — so a missing entry is a normal early answer, not a failure.
pub fn meta(pid: Option<u32>, resume: Option<&str>, cwd: &str, spawned_at_ms: u128, account_path: &str) -> Option<SessionMeta> {
    let dir = Path::new(account_path).join("sessions");
    let v = registry_entry(&dir, pid, resume, cwd, spawned_at_ms)?;
    let mut m = SessionMeta {
        name: v["name"].as_str().and_then(sane_title),
        status: v["status"].as_str().map(str::to_owned),
        cwd: v["cwd"].as_str().map(str::to_owned),
        session_id: v["sessionId"].as_str().map(str::to_owned),
        name_source: v["nameSource"].as_str().map(str::to_owned),
        ..Default::default()
    };
    if let (Some(scwd), Some(sid)) = (m.cwd.clone(), m.session_id.clone()) {
        let path = located_transcript(account_path, &scwd, &sid);
        let text = crate::pty::tail(&path, 128 * 1024);
        let end = ending(&path);
        if let Some(c) = end.context {
            m.context_tokens = Some(c.tokens);
            m.context_window = Some(c.window);
            m.context = Some((c.tokens / c.window).min(1.0));
        }
        let titles = titles_of(&sid, &path, text.as_deref());
        // The registry name is the cwd folder when `derived` — a worktree's
        // random codename — and a kebab-case handle when `auto`, neither of
        // them a title; a rename there is as good as one in the transcript.
        let renamed = titles.custom.or_else(|| m.name.clone().filter(|_| m.name_source.as_deref() == Some("user")));
        m.title_renamed = renamed.is_some();
        m.title = renamed.or(titles.ai);
        m.first_prompt = cached_first_prompt(account_path, &scwd, &sid);
        m.last_said = end.last_said;
        m.last_said_at = end.last_said_at;
    }
    Some(m)
}

/// Just the registry's status word for a live session — one small file, no
/// transcript — for the activity sampler, which asks every few seconds; with
/// when the CLI last set it (`statusUpdatedAt`, ms), where it says.
pub fn status(pid: Option<u32>, resume: Option<&str>, cwd: &str, spawned_at_ms: u128, account_path: &str) -> Option<(String, Option<u64>)> {
    let dir = Path::new(account_path).join("sessions");
    let v = registry_entry(&dir, pid, resume, cwd, spawned_at_ms)?;
    Some((v["status"].as_str()?.to_owned(), v["statusUpdatedAt"].as_u64()))
}

/// The cwd the CLI reports for a live pty session — under `--worktree` that
/// is the worktree, not the folder the session was spawned in.
pub fn session_cwd(pid: Option<u32>, resume: Option<&str>, cwd: &str, spawned_at_ms: u128, account_path: &str) -> Option<String> {
    let dir = Path::new(account_path).join("sessions");
    let v = registry_entry(&dir, pid, resume, cwd, spawned_at_ms)?;
    v["cwd"].as_str().map(str::to_owned)
}

/// Every cwd the CLI currently has a session in, per this account's registry.
pub fn live_cwds(account_path: &str) -> Vec<String> {
    let dir = Path::new(account_path).join("sessions");
    let Ok(entries) = std::fs::read_dir(&dir) else { return vec![] };
    entries.flatten().filter_map(|e| parse_session_file(&e.path())).filter_map(|v| v["cwd"].as_str().map(str::to_owned)).collect()
}

/// `live_cwds`, of the entries whose process is still there: one killed leaves its
/// entry behind.
pub fn running_cwds(account_path: &str) -> Vec<String> {
    let dir = Path::new(account_path).join("sessions");
    let Ok(entries) = std::fs::read_dir(&dir) else { return vec![] };
    entries
        .flatten()
        .filter_map(|e| parse_session_file(&e.path()))
        .filter(|v| v["pid"].as_u64().is_some_and(|pid| crate::procs::alive(pid as u32)))
        .filter_map(|v| v["cwd"].as_str().map(str::to_owned))
        .collect()
}

/// Finds the CLI's registry entry for a pty session: by pid when that matches;
/// else, for a resumed one, by the session it was told to resume; else by cwd +
/// start time. Launcher shims give the pty a different pid than the CLI process
/// that writes the registry, and the entry comes a moment after the pty. Going
/// by those last two, anything of Claude Code's in the folder would look like
/// this chat: only a terminal session counts, as `entrypoint` says. `claude -p`,
/// a background helper, says `sdk-cli` (its `kind` is "interactive" all the
/// same); the SDKs `sdk-ts` and `sdk-py`, an editor's its own.
fn registry_entry(dir: &Path, pid: Option<u32>, resume: Option<&str>, cwd: &str, spawned_at_ms: u128) -> Option<serde_json::Value> {
    if let Some(pid) = pid {
        if let Some(v) = parse_session_file(&dir.join(format!("{pid}.json"))) {
            return Some(v);
        }
    }
    let entries: Vec<serde_json::Value> =
        std::fs::read_dir(dir).ok()?.flatten().filter_map(|entry| parse_session_file(&entry.path())).collect();
    if let Some(v) = resume.and_then(|sid| entries.iter().find(|v| v["sessionId"] == sid)) {
        return Some(v.clone());
    }

    // The folder itself or one under it (a `--worktree` session reports its
    // worktree), not a sibling that shares the prefix: `luna` is not `luna-site`.
    let want = norm_path(cwd);
    let under = format!("{want}{}", crate::paths::KEY_SEP);
    let mut best: Option<(u64, serde_json::Value)> = None;
    for v in entries {
        if v["entrypoint"] != "cli" {
            continue;
        }
        let scwd = norm_path(v["cwd"].as_str().unwrap_or(""));
        let started = v["startedAt"].as_u64().unwrap_or(0);
        if (scwd == want || scwd.starts_with(&under)) && (started as u128) + 5000 >= spawned_at_ms {
            let updated = v["updatedAt"].as_u64().unwrap_or(started);
            if best.as_ref().map(|(u, _)| updated > *u).unwrap_or(true) {
                best = Some((updated, v));
            }
        }
    }
    best.map(|(_, v)| v)
}

/// The API error the last turn of a live session ended on — a usage limit, a
/// lost login — when the conversation's last word is such an error. That is
/// how a session is seen to have stopped on one when no `StopFailure` hook
/// was heard: one that has sat on the limit dialog since before Luna started.
pub fn last_api_error(pid: Option<u32>, resume: Option<&str>, cwd: &str, spawned_at_ms: u128, account_path: &str) -> Option<&'static str> {
    let path = transcript_of(pid, resume, cwd, spawned_at_ms, account_path)?;
    let v = ending(&path).api_error?;
    Some(crate::activity::api_error_kind(v["error"].as_str().unwrap_or("")))
}

/// The transcript file of a live session, once the registry knows it.
pub fn transcript_of(pid: Option<u32>, resume: Option<&str>, cwd: &str, spawned_at_ms: u128, account_path: &str) -> Option<PathBuf> {
    let dir = Path::new(account_path).join("sessions");
    let v = registry_entry(&dir, pid, resume, cwd, spawned_at_ms)?;
    let scwd = v["cwd"].as_str()?;
    let sid = v["sessionId"].as_str()?;
    Some(located_transcript(account_path, scwd, sid))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_terminal_sessions_registry_entry_can_be_a_chats() {
        let dir = std::env::temp_dir().join(format!("luna-claude-registry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let entry = |pid: u32, sid: &str, entrypoint: Option<&str>, updated: u64| {
            let mut v = serde_json::json!({ "pid": pid, "sessionId": sid, "cwd": "E:\\p", "startedAt": 2000, "updatedAt": updated, "kind": "interactive" });
            if let Some(e) = entrypoint {
                v["entrypoint"] = e.into();
            }
            std::fs::write(dir.join(format!("{pid}.json")), v.to_string()).unwrap();
        };
        let found = |pid: u32, resume: Option<&str>| {
            registry_entry(&dir, Some(pid), resume, "E:\\p", 1000).map(|v| v["sessionId"].as_str().unwrap_or("").to_string())
        };
        // Each newer than any chat's, so each would win going by the time.
        for (pid, entrypoint) in
            [(101, Some("sdk-cli")), (102, Some("sdk-py")), (103, Some("sdk-ts")), (104, Some("claude-vscode")), (105, None)]
        {
            entry(pid, &format!("other-{pid}"), entrypoint, 9000);
        }
        assert_eq!(found(999, None), None, "none of them is this chat, whose entry is yet to come");
        entry(333, "a-chat", Some("cli"), 3000);
        assert_eq!(found(999, None).as_deref(), Some("a-chat"), "the shim's case still finds a terminal session's");
        assert_eq!(found(101, None).as_deref(), Some("other-101"), "its own pid is an exact answer");

        // A helper opened as a chat, behind a shim: what it resumes is exact,
        // over a newer chat in the folder.
        entry(444, "promoted", Some("cli"), 3500);
        entry(555, "newer-chat", Some("cli"), 8000);
        assert_eq!(found(999, None).as_deref(), Some("newer-chat"));
        assert_eq!(found(999, Some("promoted")).as_deref(), Some("promoted"));
        assert_eq!(found(999, Some("gone")).as_deref(), Some("newer-chat"), "a resume that took another id goes by the rest");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
