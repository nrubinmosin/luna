//! Spawns that never completed: what their caller is told, and those started
//! afresh under a later core.

use crate::agents::lost::{left_words, sentence};
use crate::agents::notifier::CALLER_RETURN_MS;
use crate::agents::persist::SESSION_RETURN_MS;
use crate::agents::registry::{label, reg, Registry, DEV_CALLER};
use crate::agents::said::note_file;
use crate::agents::spawn::{spawn_with, SpawnTaken};
use std::collections::HashMap;

/// How a spawn is named to its caller, who may have several: as it was named,
/// else by its prompt.
fn spawn_called(s: &SpawnTaken) -> String {
    let name = s.params.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).map(str::to_owned);
    match name.or_else(|| crate::pty::title_from_prompt(&s.params.prompt)) {
        Some(name) => format!("\"{name}\""),
        None => format!("number {}", s.request),
    }
}

/// What a notice says of a worktree this spawn had begun that nothing came of. It
/// is not removed: it may hold what a person would want, and nothing has claimed
/// it. A worktree another chat made meanwhile that has no session running in it
/// reads the same; it is only named.
fn worktrees_left(s: &SpawnTaken) -> String {
    let (Some(true), Some(folder)) = (s.params.worktree, s.params.folder.as_deref()) else { return String::new() };
    left_words(&crate::worktree::left_since(
        crate::worktree::made_for(folder, &s.account_path),
        &s.worktrees_before,
        &crate::pty::live_cwds(),
    ))
}

/// The notice of a spawn nothing came of. It says why the call itself failed: the
/// caller's CLI went with the same core, and all it has of that call is an error.
fn never_completed(s: &SpawnTaken, why: Option<&str>, left: &str) -> String {
    let prompt = match note_file(&format!("spawn-{}", s.request), &s.params.prompt) {
        Some(path) => format!("Its prompt is in {}", path.display()),
        None => "Its prompt was not kept".to_string(),
    };
    let why = why.map_or(String::new(), |w| format!(" (started afresh, it failed too: {w})"));
    format!(
        "Your luna_spawn {} never completed: Luna's core restarted while it was being made, which is also why that call ended in an error or with no answer. No helper came of it{why}. {prompt}; spawn it again if it still matters.{left}",
        spawn_called(s),
    )
}

/// Asks for it again as it was asked (`resumeOnRestart`), as `spawn` would have it
/// had it not settled anything yet, and leaves what its caller is to hear. Blocks
/// as a spawn does.
fn spawn_afresh(s: SpawnTaken) {
    let mut params = s.params.clone();
    params.visible = Some(s.visible);
    params.auto_delete = s.auto_delete;
    params.resume_on_restart = Some(true);
    params.budget_tokens = s.budget_tokens;
    if let Some(worktree) = &s.in_worktree {
        params.folder = Some(worktree.clone());
    }
    // Before this one can make a worktree of its own, which is no leftover.
    let left = worktrees_left(&s);
    let mut still_coming = false;
    let made = spawn_with(&s.caller, params, Some(s.request), &mut still_coming);
    let told = match &made {
        Ok(made) => {
            crate::log::warn(
                "agents",
                &format!("{}'s spawn {} started afresh as {} after a core cut it off", label(&s.caller), s.request, label(&made.id)),
            );
            format!(
                "Your luna_spawn {} was cut off when Luna's core restarted, which is also why that call ended in an error or with no answer. As it asked with resumeOnRestart, Luna started it afresh as {}: do not spawn it again, and its answer comes as a notice.{left}",
                spawn_called(&s),
                made.id,
            )
        }
        // The later request stands and the window may yet answer it, so this is no
        // failure: told to spawn again, the caller would have two.
        Err(why) if still_coming => {
            crate::log::warn("agents", &format!("{}'s spawn {} asked for afresh is not answered yet: {why}", label(&s.caller), s.request));
            format!(
                "Your luna_spawn {} was cut off when Luna's core restarted, which is also why that call ended in an error or with no answer. As it asked with resumeOnRestart, Luna asked for it afresh, and the window has not started it yet. It may still come up: luna_list will show it and its answer comes as a notice, or a notice says it failed. Do not spawn it again.{left}",
                spawn_called(&s),
            )
        }
        Err(why) => {
            crate::log::warn(
                "agents",
                &format!("{}'s spawn {} could not start afresh after a core cut it off: {why}", label(&s.caller), s.request),
            );
            never_completed(&s, Some(why), &left)
        }
    };
    let helper = made.ok().map(|made| made.id);
    let owed_placed = helper.is_some();
    let told = SpawnTaken { news: Some(told), helper, owed_placed, settled: true, retried: still_coming, ..s };
    reg().lock().unwrap_or_else(|e| e.into_inner()).spawns.insert(told.request, told);
    crate::background::save();
}

/// A spawn that had timed out, its caller told it may yet come up, and the window
/// says it did not.
pub(in crate::agents) fn spawn_failed_late(s: SpawnTaken, why: &str) {
    crate::log::warn("agents", &format!("{}'s spawn {} failed after it timed out: {why}", label(&s.caller), s.request));
    let prompt = match note_file(&format!("spawn-{}", s.request), &s.params.prompt) {
        Some(path) => format!("Its prompt is in {}", path.display()),
        None => "Its prompt was not kept".to_string(),
    };
    let news = format!(
        "Your luna_spawn {}, which had not started in time, did not come up after all: {} No helper came of it. {prompt}; spawn it again if it still matters.{}",
        spawn_called(&s),
        sentence(why),
        worktrees_left(&s)
    );
    if let Some(left) = reg().lock().unwrap_or_else(|e| e.into_inner()).spawns.get_mut(&s.request) {
        left.news = Some(news);
    }
    crate::background::save();
}

/// What the spawns the last core left have come to, once a window has had the time
/// to finish any it was part way through (`SESSION_RETURN_MS`, as for a chat coming
/// back): a session known by then is one that came of it, and answers as any other
/// (`take_up_spawns`, and a row since deleted is no longer known); for the rest the
/// caller is told, or with `resumeOnRestart` it is started afresh and then told.
/// What it came to is decided under the lock that gives it up (`SpawnTaken::settled`):
/// past that a session the window is still starting for it is refused (`spawn_is`),
/// and before it that session is the one that came of it.
pub(in crate::agents) fn settle_spawns(up_ms: u64) {
    if up_ms < SESSION_RETURN_MS {
        return;
    }
    let due: Vec<(SpawnTaken, bool)> = {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let Registry { spawns, known, abandoned, .. } = &mut *r;
        let mut due = Vec::new();
        for s in spawns.values_mut().filter(|s| s.left_by_core && !s.settled) {
            let made = s.helper.as_ref().is_some_and(|h| known.contains_key(h));
            s.settled = true;
            if !made && s.resume_on_restart {
                abandoned.remove(&s.request);
            }
            due.push((s.clone(), made));
        }
        for (s, made) in &due {
            if *made {
                spawns.remove(&s.request);
            }
        }
        due
    };
    for (s, made) in due {
        if made {
            crate::log::info(
                "agents",
                &format!("{}'s spawn {} had come to {} when a core went", label(&s.caller), s.request, s.helper.as_deref().unwrap_or("")),
            );
            crate::background::save();
        } else if s.resume_on_restart {
            let _ = std::thread::Builder::new().name("luna-spawn-again".into()).spawn(move || spawn_afresh(s));
        } else {
            let news = never_completed(&s, None, &worktrees_left(&s));
            crate::log::warn("agents", &format!("{}'s spawn {} went with a core: {}", label(&s.caller), s.request, news));
            if let Some(left) = reg().lock().unwrap_or_else(|e| e.into_inner()).spawns.get_mut(&s.request) {
                left.news = Some(news);
            }
        }
    }
}

/// What each caller is to hear of its spawns (`settle_spawns`), by caller: the
/// request and the line, once it is there to hear it. One that has not come back
/// in the time a caller gets (CALLER_RETURN_MS), or the dev caller with no terminal
/// to be told in, goes to the log, which names the file.
pub(in crate::agents) fn spawn_news(up_ms: u64) -> HashMap<String, Vec<(u64, String)>> {
    let mut news: HashMap<String, Vec<(u64, String)>> = HashMap::new();
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let Registry { spawns, known, .. } = &mut *r;
    let mut gave_up = Vec::new();
    let mut told: Vec<&SpawnTaken> = spawns.values().filter(|s| s.news.is_some()).collect();
    told.sort_by_key(|s| s.request);
    for s in told {
        let line = s.news.clone().unwrap_or_default();
        match known.get(&s.caller) {
            Some(k) if !k.waiting && s.caller != DEV_CALLER => news.entry(s.caller.clone()).or_default().push((s.request, line)),
            Some(k) if k.waiting && up_ms < CALLER_RETURN_MS => {}
            _ => gave_up.push((s.request, s.caller.clone(), line)),
        }
    }
    for (request, caller, line) in gave_up {
        spawns.remove(&request);
        crate::log::warn("agents", &format!("{caller} is not there to be told: {line}"));
    }
    news
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::notifier::CALLER_RETURN_MS;
    use crate::agents::persist::{take_up_spawns, SESSION_RETURN_MS};
    use crate::agents::registry::{reg, register};
    use crate::agents::spawn::SpawnTaken;
    use crate::agents::teardown::forget;
    use crate::agents::test_support::{cut_short_chat, spawn_taken, spawns_to_myself, take_up};
    use crate::provider::Provider;

    #[test]
    fn what_a_spawn_came_to_decides_what_its_caller_is_told() {
        let _one = spawns_to_myself();
        // One that had come to a session the last core saved: nothing to say of the
        // spawn, and that session owes its caller as if the call had got there.
        register("ws-caller", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, serde_json::json!({}));
        let mut made = cut_short_chat("ws-made", "ws-caller", true, false, false);
        made.turns_ended = 0;
        made.ties.owed.clear();
        made.ties.opening = Some("the opening".into());
        // Asked for again by a request above any the file holds.
        let lost = spawn_taken(9_000_001, "ws-caller", None, false);
        take_up(vec![made], Vec::new(), vec![spawn_taken(9_000_000, "ws-caller", Some("ws-made"), true), lost]);
        assert!(reg().lock().unwrap().next_request > 9_000_001);
        take_up_spawns();
        {
            let r = reg().lock().unwrap();
            let o = r.owed[&("ws-caller".to_string(), "ws-made".to_string())];
            assert!(o.cut_short && o.baseline == 0 && o.cursor == 0);
            assert_eq!(r.resumes["ws-made"].opening.as_deref(), Some("the opening"));
        }
        assert_eq!(crate::activity::turns_counted("ws-made"), Some(1), "its turn went with the core");
        // Nothing before the window has had its chance.
        settle_spawns(SESSION_RETURN_MS - 1);
        assert!(reg().lock().unwrap().spawns[&9_000_001].news.is_none());
        settle_spawns(SESSION_RETURN_MS);
        {
            let r = reg().lock().unwrap();
            assert!(!r.spawns.contains_key(&9_000_000), "a helper came of it");
            let news = r.spawns[&9_000_001].news.clone().unwrap();
            assert!(news.contains("\"the job\"") && news.contains("never completed") && news.contains("core restarted"), "{news}");
            assert!(news.contains("ended in an error"), "it says what became of the call: {news}");
            assert!(!news.contains("worktreeLeft"), "{news}");
            let file = news.split("Its prompt is in ").nth(1).and_then(|l| l.split("; spawn it again").next()).unwrap();
            assert_eq!(std::fs::read_to_string(file).unwrap(), "the whole job, word for word");
            let _ = std::fs::remove_file(file);
        }
        // Told only once it is back, and what was told goes.
        let news = spawn_news(1000);
        assert_eq!(news["ws-caller"].len(), 1);
        reg().lock().unwrap().known.get_mut("ws-caller").unwrap().waiting = true;
        assert!(!spawn_news(1000).contains_key("ws-caller"), "not started yet");
        assert!(reg().lock().unwrap().spawns.contains_key(&9_000_001));
        assert!(!spawn_news(CALLER_RETURN_MS).contains_key("ws-caller"));
        assert!(!reg().lock().unwrap().spawns.contains_key(&9_000_001), "not back in time: to the log");
        forget("ws-made");
        forget("ws-caller");
    }

    #[test]
    fn a_row_the_window_dropped_makes_it_a_spawn_nothing_came_of() {
        let _one = spawns_to_myself();
        register("wd-caller", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, serde_json::json!({}));
        let mut made = cut_short_chat("wd-gone", "wd-caller", false, false, false);
        made.ties.owed.clear();
        take_up(vec![made], Vec::new(), vec![spawn_taken(9_000_100, "wd-caller", Some("wd-gone"), false)]);
        take_up_spawns();
        // `chat_names` finds no row for it: the caller's debt goes with it.
        forget("wd-gone");
        assert!(!reg().lock().unwrap().owed.contains_key(&("wd-caller".to_string(), "wd-gone".to_string())));
        settle_spawns(SESSION_RETURN_MS);
        let news = reg().lock().unwrap().spawns[&9_000_100].news.clone().unwrap();
        assert!(news.contains("never completed"), "{news}");
        forget("wd-caller");
        assert!(!reg().lock().unwrap().spawns.contains_key(&9_000_100));
    }

    #[test]
    fn a_spawn_with_the_flag_is_started_afresh_and_says_so_or_why_not() {
        let _one = spawns_to_myself();
        // No such caller to spawn for: as a spawn would fail, and the caller told.
        let s = SpawnTaken { left_by_core: true, ..spawn_taken(9_000_300, "af-nobody", None, true) };
        spawn_afresh(s);
        let r = reg().lock().unwrap();
        let news = r.spawns[&9_000_300].news.clone().unwrap();
        assert!(news.contains("never completed") && news.contains("started afresh, it failed too"), "{news}");
        drop(r);
        reg().lock().unwrap().spawns.remove(&9_000_300);
    }
}
