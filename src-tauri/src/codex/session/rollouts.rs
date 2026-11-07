//! The rollout files: where a day's are, which is a thread's, and which one is a
//! chat's — the newest of its folder that appeared after its spawn, or the one
//! its resume or its TUI's title names — remembered per chat so two chats in one
//! folder never share a file.

use super::*;

fn sessions_dir(account_path: &str) -> PathBuf {
    Path::new(account_path).join("sessions")
}

/// `rollout-2026-09-17T14-03-22-<uuid>.jsonl` → (local time, uuid). A name
/// with a `_<rollout id>` suffix (after a `/revert`) still names the thread.
pub fn parse_name(file_name: &str) -> Option<(NaiveDateTime, String)> {
    let rest = file_name.strip_prefix("rollout-")?.strip_suffix(".jsonl")?;
    // "YYYY-MM-DDTHH-MM-SS" is 19 chars, then '-' then the uuid. Any file
    // in the folder is looked at, so the cut must land on a char boundary.
    if rest.len() < 21 || !rest.is_char_boundary(19) {
        return None;
    }
    let (ts, id) = rest.split_at(19);
    let ts = NaiveDateTime::parse_from_str(ts, "%Y-%m-%dT%H-%M-%S").ok()?;
    let id = id.strip_prefix('-')?;
    let id = id.split('_').next()?.to_string();
    (!id.is_empty()).then_some((ts, id))
}

pub(super) fn first_line(path: &Path) -> Option<Value> {
    use std::io::{BufRead, BufReader};
    let f = std::fs::File::open(path).ok()?;
    let mut line = String::new();
    BufReader::new(f).read_line(&mut line).ok()?;
    serde_json::from_str(&line).ok()
}

pub(super) fn day_dir(account_path: &str, day: NaiveDate) -> PathBuf {
    sessions_dir(account_path).join(day.year().to_string()).join(format!("{:02}", day.month())).join(format!("{:02}", day.day()))
}

/// Rollout files written on the given days, newest first.
pub(super) fn rollouts_on(account_path: &str, days: impl Iterator<Item = NaiveDate>) -> Vec<(NaiveDateTime, String, PathBuf)> {
    let mut out = vec![];
    for day in days {
        let Ok(entries) = std::fs::read_dir(day_dir(account_path, day)) else { continue };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if let Some((ts, id)) = parse_name(&name) {
                out.push((ts, id, e.path()));
            }
        }
    }
    out.sort_by_key(|e| std::cmp::Reverse(e.0));
    out
}

pub(super) fn days_back(from: NaiveDate, n: usize) -> impl Iterator<Item = NaiveDate> {
    (0..n as i64).filter_map(move |i| from.checked_sub_days(chrono::Days::new(i as u64)))
}

/// The rollout of a thread, by id, wherever it was written. Walks day
/// directories newest first and stops at the first hit; a thread months old
/// costs a longer walk once, after which the answer is cached.
fn rollout_by_id(account_path: &str, id: &str) -> Option<PathBuf> {
    rollout_where(account_path, |of| of == id)
}

/// `rollout_by_id` for the first whose thread id passes `is`.
fn rollout_where(account_path: &str, is: impl Fn(&str) -> bool) -> Option<PathBuf> {
    let root = sessions_dir(account_path);
    let mut years: Vec<PathBuf> = std::fs::read_dir(&root).ok()?.flatten().map(|e| e.path()).collect();
    years.sort();
    for y in years.into_iter().rev() {
        let Ok(months) = std::fs::read_dir(&y) else { continue };
        let mut months: Vec<PathBuf> = months.flatten().map(|e| e.path()).collect();
        months.sort();
        for m in months.into_iter().rev() {
            let Ok(days) = std::fs::read_dir(&m) else { continue };
            let mut days: Vec<PathBuf> = days.flatten().map(|e| e.path()).collect();
            days.sort();
            for d in days.into_iter().rev() {
                let Ok(files) = std::fs::read_dir(&d) else { continue };
                for f in files.flatten() {
                    let name = f.file_name().to_string_lossy().into_owned();
                    if parse_name(&name).is_some_and(|(_, i)| is(&i)) {
                        return Some(f.path());
                    }
                }
            }
        }
    }
    None
}

/// The rollout of a thread no live pty session claims — a background
/// helper's, whose `codex exec` names the thread on stdout. Remembered once
/// found, as a thread's file never moves.
pub fn rollout_of_thread(account_path: &str, id: &str) -> Option<PathBuf> {
    static FOUND: OnceLock<Mutex<HashMap<String, PathBuf>>> = OnceLock::new();
    let found = FOUND.get_or_init(Default::default);
    if let Some(p) = found.lock().ok()?.get(id).filter(|p| p.is_file()) {
        return Some(p.clone());
    }
    let path = rollout_by_id(account_path, id)?;
    found.lock().ok()?.insert(id.to_string(), path.clone());
    Some(path)
}

/// Where a thread works, per its rollout: a `--worktree` one names the
/// checkout Codex made.
pub fn thread_cwd(account_path: &str, id: &str) -> Option<String> {
    first_line(&rollout_of_thread(account_path, id)?)?["payload"]["cwd"].as_str().map(str::to_owned)
}

fn claims() -> &'static Mutex<HashMap<String, PathBuf>> {
    static C: OnceLock<Mutex<HashMap<String, PathBuf>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// Forget a chat's rollout, e.g. when its session is respawned.
pub fn release(chat_id: &str) {
    if let Ok(mut m) = claims().lock() {
        m.remove(chat_id);
    }
    if let Ok(mut m) = turn_counts().lock() {
        m.remove(chat_id);
    }
    if let Ok(mut m) = children().lock() {
        m.remove(chat_id);
    }
}

/// Whether a rollout's cwd is this folder, or a worktree of it: under `--worktree`
/// the rollout names the checkout Codex made, which points back at the folder the
/// session was started in. That repository is in git's spelling (long names) and
/// the folder may be in 8.3 short ones, so the two are compared by place.
fn in_folder(cwd: &str, folder: &str) -> bool {
    use crate::worktree::{codex_shaped, project_of, same_place};
    norm_path(cwd) == norm_path(folder) || (codex_shaped(cwd) && project_of(cwd).is_some_and(|p| same_place(&p, folder)))
}

/// The rollouts a terminal session started at or after `since_ms` in this folder, or
/// in a worktree of it, whichever chat's they are.
pub fn rollouts_since(account_path: &str, folder: &str, since_ms: u64) -> Vec<PathBuf> {
    let Some(since) = Local.timestamp_millis_opt(since_ms as i64).single().map(|t| t.naive_local()) else { return Vec::new() };
    let ours = |cwd: &str| in_folder(cwd, folder);
    // Its name has it to the second, and the clocks may differ by a few.
    let not_before = since - chrono::Duration::seconds(5);
    rollouts_on(account_path, days_back(Local::now().date_naive(), 2))
        .into_iter()
        .filter(|(ts, _, _)| *ts >= not_before)
        .filter(|(_, _, p)| {
            first_line(p).is_some_and(|v| v["payload"]["source"] == "cli" && v["payload"]["cwd"].as_str().is_some_and(ours))
        })
        .map(|(_, _, p)| p)
        .collect()
}

/// The rollout of a thread a live session's TUI says it is on, by the start of
/// its id that the title holds (pty/screen.rs `thread_in_title`): looked for where a
/// thread of now is written, the last two days, and once in all the rest, for a
/// thread `/resume` went back to. One not written yet (Codex writes it at the
/// thread's first turn) costs only the first look after that. Two of now that
/// start alike are no answer.
fn thread_file(account_path: &str, thread: &str) -> Option<PathBuf> {
    // What the walk found, kept, as a thread's file never moves; and the
    // threads it found nothing for, not walked again.
    static FOUND: OnceLock<Mutex<HashMap<String, PathBuf>>> = OnceLock::new();
    static MISSED: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    let key = format!("{account_path}\n{thread}");
    let found = FOUND.get_or_init(Default::default);
    if let Some(p) = found.lock().ok()?.get(&key).filter(|p| p.is_file()) {
        return Some(p.clone());
    }
    let mut recent =
        rollouts_on(account_path, days_back(Local::now().date_naive(), 2)).into_iter().filter(|(_, id, _)| id.starts_with(thread));
    if let Some((_, _, p)) = recent.next() {
        return recent.next().is_none().then_some(p);
    }
    let missed = MISSED.get_or_init(Default::default);
    if missed.lock().ok()?.contains(&key) {
        return None;
    }
    match rollout_where(account_path, |id| id.starts_with(thread)) {
        Some(p) => {
            found.lock().ok()?.insert(key, p.clone());
            Some(p)
        }
        None => {
            missed.lock().ok()?.insert(key);
            None
        }
    }
}

/// The rollout file this live session writes to, once it exists. Codex says
/// which thread its TUI is on (codex/launch.rs `CODEX_TITLE`), and that settles it: its
/// file once written, a `/new`, `/resume` or `/fork` in it followed, and until a
/// new thread's file is written the one it was on, never a guess.
pub fn rollout_for(live: &Live) -> Option<PathBuf> {
    rollout_on(live, crate::pty::manager().codex_thread(live.chat_id))
}

/// `rollout_for`, its TUI on `thread`, if it has said.
pub(super) fn rollout_on(live: &Live, thread: Option<String>) -> Option<PathBuf> {
    let claimed = claims().lock().ok().and_then(|m| m.get(live.chat_id).cloned()).filter(|p| p.is_file());
    if let Some(thread) = thread {
        let of_it =
            |p: &PathBuf| p.file_name().and_then(|n| parse_name(&n.to_string_lossy())).is_some_and(|(_, id)| id.starts_with(&thread));
        if claimed.as_ref().is_some_and(of_it) {
            return claimed;
        }
        match thread_file(live.account_path, &thread) {
            // Its TUI watching one of its subagents (`/subagents`) names that
            // one's thread, which runs in the session but is not its own: the
            // session stays on the rollout it was on. So it does while the
            // file's first line does not read yet, half written: a claim
            // made on it would stand once it read.
            Some(found) if first_line(&found).is_none_or(|meta| spawned_by(&meta).is_some()) => {}
            Some(found) => {
                crate::log::info(
                    "codex",
                    &format!(
                        "{} is on thread {thread}{}",
                        crate::agents::label(live.chat_id),
                        claimed.as_ref().map_or(String::new(), |p| format!(", no longer {}", p.display()))
                    ),
                );
                if let Ok(mut m) = claims().lock() {
                    m.insert(live.chat_id.to_string(), found.clone());
                }
                return Some(found);
            }
            None => return claimed,
        }
    }
    if claimed.is_some() {
        return claimed;
    }

    // A resumed thread is found by its id, which says whose file it is.
    if let Some(id) = live.resume {
        let found = rollout_by_id(live.account_path, id)?;
        if let Ok(mut m) = claims().lock() {
            m.insert(live.chat_id.to_string(), found.clone());
        }
        return Some(found);
    }

    let spawned = Local.timestamp_millis_opt(live.spawned_at_ms as i64).single().map(|t| t.naive_local())?;
    // Codex writes the file right after start; allow the clock to disagree
    // by a few seconds either way.
    let not_before = spawned - chrono::Duration::seconds(5);
    let taken: Vec<PathBuf> =
        claims().lock().map(|m| m.iter().filter(|(k, _)| *k != live.chat_id).map(|(_, v)| v.clone()).collect()).unwrap_or_default();
    let ours = |cwd: &str| in_folder(cwd, live.cwd);
    // Only what a terminal session writes can be this one's. A background
    // helper's `codex exec` says `exec`, and the subagents either kind starts
    // say `{"subagent":…}`, all in this folder and on this account, and any
    // moment now: told apart by the file, they stay told apart after a helper
    // is deleted or the core restarts, and with no list of ids to keep.
    let meta_fits = |v: &Value| v["payload"]["source"] == "cli" && v["payload"]["cwd"].as_str().is_some_and(ours);
    let today = Local::now().date_naive();
    let mut candidates: Vec<(NaiveDateTime, PathBuf)> = rollouts_on(live.account_path, days_back(today, 2))
        .into_iter()
        .filter(|(ts, _, p)| *ts >= not_before && !taken.contains(p))
        .filter(|(_, _, p)| first_line(p).is_some_and(|v| meta_fits(&v)))
        .map(|(ts, _, p)| (ts, p))
        .collect();
    // Earliest after the spawn: the one that appeared when this session
    // started, not one a later chat in the same folder made. A file named in
    // the second the session started or later goes first: one from just
    // before it is a chat started a moment earlier that has not claimed its
    // own yet.
    let started = spawned.with_nanosecond(0).unwrap_or(spawned);
    candidates.sort_by_key(|(ts, _)| (*ts < started, *ts));
    let (_, found) = candidates.into_iter().next()?;

    // A guessed rollout is one chat's. The look above ran without the lock,
    // and another chat may have claimed the same file since: the first claim
    // stands, and this chat looks again next time.
    let mut m = claims().lock().ok()?;
    if m.iter().any(|(k, v)| *k != live.chat_id && *v == found) {
        return None;
    }
    m.insert(live.chat_id.to_string(), found.clone());
    Some(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rollout_in_a_worktree_codex_made_is_of_the_folder_however_that_is_spelled() {
        let root = std::env::temp_dir().join(format!("luna-rollout-folder-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // Short where the volume keeps 8.3 names: the worktree's `.git` file names
        // the repository in long ones.
        let root = crate::worktree::short_spelling(&root);
        let (repo, other) = (root.join("repo"), root.join("other"));
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let wt = root.join("account").join("worktrees").join("0a1b").join("repo");
        std::fs::create_dir_all(wt.parent().unwrap()).unwrap();
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(["-c", "user.email=t@t", "-c", "user.name=t"])
                .args(args)
                .output()
                .is_ok_and(|o| o.status.success())
        };
        if !git(&["init", "-q"]) || !git(&["commit", "-q", "--allow-empty", "-m", "i"]) {
            return;
        }
        assert!(git(&["worktree", "add", "-q", "--detach", &s(&wt)]));
        assert!(in_folder(&s(&wt), &s(&repo)), "Codex's worktree of it");
        assert!(in_folder(&s(&repo), &s(&repo)));
        assert!(!in_folder(&s(&wt), &s(&other)), "another folder's");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn parses_rollout_names() {
        let (ts, id) = parse_name("rollout-2026-09-17T14-03-22-0192a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b.jsonl").unwrap();
        assert_eq!(ts.to_string(), "2026-09-17 14:03:22");
        assert_eq!(id, "0192a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b");
        let (_, id) = parse_name("rollout-2026-09-17T14-03-22-abc_def.jsonl").unwrap();
        assert_eq!(id, "abc");
        assert!(parse_name("notes.txt").is_none());
        assert!(parse_name("rollout-garbage.jsonl").is_none());
    }

    #[test]
    fn two_chats_in_one_folder_get_a_rollout_each() {
        let root = std::env::temp_dir().join(format!("luna-codex-claims-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let acct = root.to_string_lossy().into_owned();
        let t0 = crate::throttle::now_ms() - 60_000;
        // A rollout appears a second after its chat starts; chat b starts two
        // seconds after chat a.
        let write = |at_ms: u64, uuid: &str| {
            let ts = Local.timestamp_millis_opt(at_ms as i64).single().unwrap().naive_local();
            let dir = day_dir(&acct, ts.date());
            std::fs::create_dir_all(&dir).unwrap();
            let name = format!("rollout-{}-{uuid}.jsonl", ts.format("%Y-%m-%dT%H-%M-%S"));
            std::fs::write(dir.join(name), format!("{}\n", r#"{"type":"session_meta","payload":{"cwd":"E:\\p","source":"cli"}}"#)).unwrap();
        };
        write(t0 + 1_000, "aaaa");
        write(t0 + 3_000, "bbbb");
        let (a, b, c) = (format!("claim-a-{t0}"), format!("claim-b-{t0}"), format!("claim-c-{t0}"));
        let live = |chat: &str, at_ms: u64| {
            rollout_for(&Live {
                chat_id: chat,
                cwd: "E:\\p",
                spawned_at_ms: at_ms as u128,
                resume: None,
                account_path: &acct,
                last_output_ms: 0,
            })
            .and_then(|p| parse_name(&p.file_name()?.to_string_lossy()).map(|(_, id)| id))
        };
        // b looks first: the file of the chat started just before it is not b's.
        assert_eq!(live(b.as_str(), t0 + 2_000).as_deref(), Some("bbbb"));
        assert_eq!(live(a.as_str(), t0).as_deref(), Some("aaaa"));
        // Both taken: a third chat in the folder gets neither.
        assert_eq!(live(c.as_str(), t0), None);
        for chat in [a, b, c] {
            release(&chat);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `/subagents` in its TUI watching one of its subagents names that one's
    /// thread in the title: the chat stays on its own rollout, also while the
    /// subagent's first line is still half written, and goes on with its own
    /// once the TUI is back.
    #[test]
    fn watching_a_subagent_keeps_the_chat_on_its_own_thread() {
        let (root, t0) = rollout_bench("watch");
        let acct = root.to_string_lossy().into_owned();
        let chat = format!("chat-watch-{t0}");
        let (own, kid, young) = (format!("ownwatch{t0}"), format!("kidwatch{t0}"), format!("youngwatch{t0}"));
        write_rollout(&root, t0 + 1_000, &own, Some(r#""cli""#));
        write_rollout(&root, t0 + 2_000, &kid, Some(&spawned_by_json(&own)));
        let live = Live { chat_id: &chat, cwd: "E:\\p", spawned_at_ms: t0 as u128, resume: None, account_path: &acct, last_output_ms: 0 };
        let on = |thread: &str| {
            rollout_on(&live, Some(thread.to_string())).and_then(|p| parse_name(&p.file_name()?.to_string_lossy()).map(|(_, id)| id))
        };
        assert_eq!(on(&own).as_deref(), Some(own.as_str()));
        assert_eq!(on(&kid).as_deref(), Some(own.as_str()), "watching its subagent");
        let half = rollout_path(&root, &young, t0 + 3_000);
        std::fs::write(&half, r#"{"type":"session_meta","payload":{"cwd":"E:\\p","sou"#).unwrap();
        assert_eq!(on(&young).as_deref(), Some(own.as_str()), "its first line half written");
        std::fs::write(
            &half,
            format!(r#"{{"type":"session_meta","payload":{{"cwd":"E:\\p","source":{}}}}}"#, spawned_by_json(&own)) + "\n",
        )
        .unwrap();
        assert_eq!(on(&young).as_deref(), Some(own.as_str()), "and once it reads");
        assert_eq!(on(&own).as_deref(), Some(own.as_str()));
        release(&chat);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A `/new` in its TUI, and a `/resume` back: the chat goes where it says it
    /// is, once that thread's file is written, never to another chat's file, and
    /// its turns count on over all of them, each once.
    #[test]
    fn a_chat_follows_the_thread_its_tui_is_on_and_counts_on() {
        let (root, t0) = rollout_bench("follow");
        let acct = root.to_string_lossy().into_owned();
        let chat = format!("chat-follow-{t0}");
        let (a, b) = (format!("aaaa{t0}"), format!("bbbb{t0}"));
        let at = |ms: u64| chrono::DateTime::from_timestamp_millis(ms as i64).unwrap().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let turn =
            |ms: u64| format!("{{\"timestamp\":\"{}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"task_complete\"}}}}\n", at(ms));
        let file = |uuid: &str, ms: u64| {
            let ts = Local.timestamp_millis_opt(ms as i64).single().unwrap().naive_local();
            day_dir(&acct, ts.date()).join(format!("rollout-{}-{uuid}.jsonl", ts.format("%Y-%m-%dT%H-%M-%S")))
        };
        let (fa, fb) = (file(&a, t0 + 1_000), file(&b, t0 + 5_000));
        std::fs::create_dir_all(fa.parent().unwrap()).unwrap();
        let meta = r#"{"type":"session_meta","payload":{"cwd":"E:\\p","source":"cli"}}"#.to_string() + "\n";
        std::fs::write(&fa, meta.clone() + &turn(t0 + 2_000)).unwrap();
        let live = Live { chat_id: &chat, cwd: "E:\\p", spawned_at_ms: t0 as u128, resume: None, account_path: &acct, last_output_ms: 0 };
        let on = |thread: &str| rollout_on(&live, Some(thread.to_string()));
        let count = |path: Option<PathBuf>| tracked_at(&live, path.unwrap()).map(|s| s.completed);
        assert_eq!(on(&a), Some(fa.clone()));
        assert_eq!(count(on(&a)), Some(1));
        // A turn that ends just before the `/new`, never looked at on its file.
        std::fs::write(&fa, meta.clone() + &turn(t0 + 2_000) + &turn(t0 + 3_000)).unwrap();
        // `/new`: a thread with no file yet keeps it on the one it was on.
        assert_eq!(on(&b), Some(fa.clone()), "not written yet: no guess, and no move");
        // Another chat's file in the folder is never this one's.
        let other = file(&format!("cccc{t0}"), t0 + 6_000);
        std::fs::write(&other, meta.clone() + &turn(t0 + 7_000)).unwrap();
        assert_eq!(on(&b), Some(fa.clone()));
        std::fs::write(&fb, meta.clone() + &turn(t0 + 8_000)).unwrap();
        assert_eq!(on(&b), Some(fb.clone()), "written: it moves there");
        assert_eq!(count(on(&b)), Some(3), "on from the first thread's turns, the one ended last among them");
        // `/resume` back to the first, which gets a turn more.
        std::fs::write(&fa, meta + &turn(t0 + 2_000) + &turn(t0 + 3_000) + &turn(t0 + 9_000)).unwrap();
        assert_eq!(count(on(&a)), Some(4), "its earlier turns not counted again");
        release(&chat);
        let _ = std::fs::remove_dir_all(&root);
    }

    fn takes(root: &Path, chat: &str, at_ms: u64, resume: Option<&str>) -> Option<String> {
        rollout_for(&Live {
            chat_id: chat,
            cwd: "E:\\p",
            spawned_at_ms: at_ms as u128,
            resume,
            account_path: &root.to_string_lossy(),
            last_output_ms: 0,
        })
        .and_then(|p| parse_name(&p.file_name()?.to_string_lossy()).map(|(_, id)| id))
    }

    #[test]
    fn a_chat_takes_only_what_a_terminal_session_wrote() {
        let (root, t0) = rollout_bench("source");
        let own = format!("ownsource{t0}");
        let chat = format!("chat-source-{t0}");
        // All in its folder and on its account, around when it started, and
        // nothing in memory says whose they are: as after a core restart.
        let subagent = r#"{"subagent":{"thread_spawn":{"parent_thread_id":"p","depth":1}}}"#;
        for (i, (at, source)) in [
            (1_000, Some(r#""exec""#)),
            (3_000, Some(r#""exec""#)),
            (1_000, Some(subagent)),
            (3_000, Some(subagent)),
            (3_000, Some(r#""mcp""#)),
            (3_000, Some(r#""vscode""#)),
            (3_000, None),
        ]
        .into_iter()
        .enumerate()
        {
            write_rollout(&root, t0 + at, &format!("other{i}x{t0}"), source);
        }
        assert_eq!(takes(&root, &chat, t0 + 2_000, None), None, "it waits for its own");
        write_rollout(&root, t0 + 4_000, &own, Some(r#""cli""#));
        assert_eq!(takes(&root, &chat, t0 + 2_000, None).as_deref(), Some(own.as_str()));
        release(&chat);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_helper_opened_as_a_chat_still_finds_its_own_by_id() {
        let (root, t0) = rollout_bench("helper-open");
        let helper = format!("helperopen{t0}");
        let chat = format!("chat-open-{t0}");
        write_rollout(&root, t0, &helper, Some(r#""exec""#));
        assert_eq!(takes(&root, &chat, t0 + 30_000, Some(&helper)).as_deref(), Some(helper.as_str()));
        release(&chat);
        let _ = std::fs::remove_dir_all(&root);
    }
}
