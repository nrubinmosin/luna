//! The titles the CLI keeps in a transcript — a `/rename`, and the one it
//! generated — kept per session as the tail moves on, and the opening prompt the
//! chat goes by until one lands.

use super::*;

/// The titles the CLI keeps in the transcript. Both are re-appended with the
/// session's other metadata as the transcript grows, so the last of each is
/// the current one and sits near the end.
#[derive(Default, Clone)]
pub(super) struct Titles {
    /// `/rename`.
    pub(super) custom: Option<String>,
    /// What the CLI generated from the opening of the conversation — the same
    /// title it puts on the terminal tab. Never a plan's handle.
    pub(super) ai: Option<String>,
}

/// Accepting a plan makes the CLI write a second `ai-title`: the plan's
/// kebab-case handle (`fix-account-status-migration`), echoed by an
/// `agent-name` line. It is not a title anyone would give the chat, and in
/// every transcript on disk it is the only thing that ever replaced the first.
fn is_handle(s: &str) -> bool {
    s.contains('-') && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

pub(super) fn titles_in(text: &str) -> Titles {
    let mut t = Titles::default();
    // The `agent-name` follows its `ai-title`, so reading backwards it is
    // known by the time the title it rules out comes up.
    let mut handles: Vec<String> = Vec::new();
    for line in text.lines().rev() {
        if t.custom.is_some() && t.ai.is_some() {
            break;
        }
        if !line.contains("-title\"") && !line.contains("\"agent-name\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        match v["type"].as_str() {
            Some("agent-name") => handles.extend(v["agentName"].as_str().map(str::to_owned)),
            Some("custom-title") if t.custom.is_none() => t.custom = v["customTitle"].as_str().and_then(sane_title),
            Some("ai-title") if t.ai.is_none() => {
                t.ai = v["aiTitle"].as_str().and_then(sane_title).filter(|s| !is_handle(s) && !handles.contains(s))
            }
            _ => {}
        }
    }
    t
}

fn title_cache() -> &'static Mutex<HashMap<String, Titles>> {
    static CACHE: std::sync::OnceLock<Mutex<HashMap<String, Titles>>> = std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// The titles of a live session. A long tool run can push the last title out
/// of the tail for a while, so the titles seen before are kept per session and
/// the tail only updates them. The first look reads the whole transcript: a
/// long session Luna has just restarted on may hold no title in its tail, and
/// the opening prompt would take the chat's name until one came round again.
pub(super) fn titles_of(session_id: &str, path: &Path, tail: Option<&str>) -> Titles {
    let first_look = !title_cache().lock().unwrap().contains_key(session_id);
    let seen = if first_look { std::fs::read_to_string(path).ok().map(|s| titles_in(&s)) } else { tail.map(titles_in) };
    remembered_titles(session_id, seen.unwrap_or_default())
}

fn remembered_titles(session_id: &str, seen: Titles) -> Titles {
    let mut map = title_cache().lock().unwrap();
    if map.len() >= CACHE_CAP && !map.contains_key(session_id) {
        map.clear();
    }
    let t = map.entry(session_id.to_string()).or_default();
    if seen.custom.is_some() {
        t.custom = seen.custom;
    }
    if seen.ai.is_some() {
        t.ai = seen.ai;
    }
    t.clone()
}

/// The CLI's title for a session that is not running — a chat restored from
/// an earlier run, which session_meta cannot see. The transcript is found by
/// name: the folder it sits in comes from a cwd that, for a worktree session,
/// is long gone.
pub fn saved_title(account_path: &str, session_id: &str) -> Option<CliTitle> {
    let path = find_transcript(account_path, session_id)?;
    let mut t = crate::pty::tail(&path, 256 * 1024).map(|s| titles_in(&s)).unwrap_or_default();
    if t.custom.is_none() && t.ai.is_none() {
        // Asked once per chat per run, so the whole file is affordable when
        // a session ended with its last title further back than the tail.
        t = titles_in(&std::fs::read_to_string(&path).ok()?);
    }
    match (t.custom, t.ai) {
        (Some(title), _) => Some(CliTitle { title, renamed: true }),
        (None, Some(title)) => Some(CliTitle { title, renamed: false }),
        (None, None) => None,
    }
}

/// The opening prompt of a session never changes, but session_meta asks for it
/// every few seconds per pane — so read the file once and remember the answer.
pub(super) fn cached_first_prompt(account_path: &str, cwd: &str, session_id: &str) -> Option<String> {
    static CACHE: std::sync::OnceLock<Mutex<HashMap<String, Option<String>>>> = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);

    if let Ok(map) = cache.lock() {
        if let Some(hit) = map.get(session_id) {
            return hit.clone();
        }
    }
    let found = read_first_prompt(account_path, cwd, session_id);
    // A miss is worth caching too, but only once the transcript exists: before
    // the first turn there is genuinely nothing to read yet.
    if found.is_some() {
        if let Ok(mut map) = cache.lock() {
            if map.len() >= CACHE_CAP {
                map.clear();
            }
            map.insert(session_id.to_string(), found.clone());
        }
    }
    found
}

/// The session's opening prompt, trimmed to a title — what the chat is called
/// for the few seconds before the CLI's own title lands in the transcript.
fn read_first_prompt(account_path: &str, cwd: &str, session_id: &str) -> Option<String> {
    use std::io::{BufRead, BufReader};
    let path = located_transcript(account_path, cwd, session_id);
    let f = std::fs::File::open(&path).ok()?;
    first_prompt_in(BufReader::new(f).lines().map_while(Result::ok))
}

/// `read_first_prompt` of the transcript's lines, up to the first that is no
/// text.
pub(super) fn first_prompt_in(lines: impl Iterator<Item = impl AsRef<str>>) -> Option<String> {
    // The opening prompt is near the top; no need to walk a 700KB transcript.
    for line in lines.take(80) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line.as_ref()) else { continue };
        // `isMeta` marks a turn the CLI injected rather than one the user
        // typed — a hook's instructions, a resumed-session preamble. One of
        // those titled a chat "A session-scoped Stop hook is now active…".
        if v["type"] != "user" || v["isSidechain"].as_bool().unwrap_or(false) || v["isMeta"].as_bool().unwrap_or(false) {
            continue;
        }
        let Some(text) = content_text(&v["message"]["content"]) else { continue };
        if let Some(t) = crate::pty::title_from_prompt(&text) {
            return Some(t);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_latest_titles_from_the_tail() {
        let text = concat!(
            r#"{"type":"ai-title","aiTitle":"First guess","sessionId":"s"}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":"mention \"type\":\"ai-title\" in passing"}}"#,
            "\n",
            r#"{"type":"ai-title","aiTitle":"Token refresh and chat titles","sessionId":"s"}"#,
            "\n",
            r#"{"type":"last-prompt","lastPrompt":"…","sessionId":"s"}"#,
            "\n",
        );
        let t = titles_in(text);
        assert_eq!(t.ai.as_deref(), Some("Token refresh and chat titles"));
        assert_eq!(t.custom, None);

        let renamed = format!("{text}{}\n", r#"{"type":"custom-title","customTitle":"Luna auth","sessionId":"s"}"#);
        let t = titles_in(&renamed);
        assert_eq!(t.custom.as_deref(), Some("Luna auth"));
        assert_eq!(t.ai.as_deref(), Some("Token refresh and chat titles"));
    }

    #[test]
    fn a_plan_handle_is_not_a_title() {
        // As a transcript has it after a plan is accepted.
        let text = concat!(
            r#"{"type":"ai-title","aiTitle":"Issue #5244","sessionId":"s"}"#,
            "\n",
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"ExitPlanMode"}]}}"#,
            "\n",
            r#"{"type":"ai-title","aiTitle":"fix-connector-logging-and-model-picker","sessionId":"s"}"#,
            "\n",
            r#"{"type":"agent-name","agentName":"fix-connector-logging-and-model-picker","sessionId":"s"}"#,
            "\n",
        );
        assert_eq!(titles_in(text).ai.as_deref(), Some("Issue #5244"));
        // The first title already out of the tail: nothing beats the handle.
        let tail = text.lines().skip(1).collect::<Vec<_>>().join("\n");
        assert_eq!(titles_in(&tail).ai, None);
        assert!(!is_handle("Fix login-flow"));
        // lang-check: allow
        assert!(!is_handle("Онборд нового юзера"));
    }

    #[test]
    fn keeps_a_title_the_tail_no_longer_shows() {
        let sid = format!("test-{}", std::process::id());
        let seen = Titles { ai: Some("Issue 4489".into()), custom: None };
        assert_eq!(remembered_titles(&sid, seen).ai.as_deref(), Some("Issue 4489"));
        assert_eq!(remembered_titles(&sid, Titles::default()).ai.as_deref(), Some("Issue 4489"));
    }

    #[test]
    fn the_first_look_reads_past_the_tail() {
        let sid = format!("first-look-{}", std::process::id());
        let path = std::env::temp_dir().join(format!("{sid}.jsonl"));
        std::fs::write(&path, r#"{"type":"ai-title","aiTitle":"Chat titles","sessionId":"s"}"#).unwrap();
        // A tail with no title in it, as a long tool run leaves one.
        assert_eq!(titles_of(&sid, &path, Some("{}")).ai.as_deref(), Some("Chat titles"));
        assert_eq!(titles_of(&sid, &path, Some("{}")).ai.as_deref(), Some("Chat titles"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn finds_a_saved_title_by_session_id() {
        let root = std::env::temp_dir().join(format!("luna-claude-title-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("projects").join("E--p--claude-worktrees-gone");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("28c18d5b-64d5-40a8-a1ec-fb55573139d9.jsonl"),
            // lang-check: allow
            r#"{"type":"ai-title","aiTitle":"Онборд нового юзера","sessionId":"28c18d5b-64d5-40a8-a1ec-fb55573139d9"}"#,
        )
        .unwrap();
        let acct = root.to_string_lossy();
        assert_eq!(
            saved_title(&acct, "28c18d5b-64d5-40a8-a1ec-fb55573139d9"),
            // lang-check: allow
            Some(CliTitle { title: "Онборд нового юзера".into(), renamed: false })
        );
        assert_eq!(saved_title(&acct, "../../etc"), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
