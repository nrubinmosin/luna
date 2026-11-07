//! Rollouts written for the tests of finding and scanning them: in a sessions
//! folder of a test's own, with turn events and a subagent's parent in them.

use super::*;

/// An account folder to write rollouts into, and a moment long enough ago
/// for what is written after it to be in the past.
pub(super) fn rollout_bench(tag: &str) -> (PathBuf, u64) {
    let root = std::env::temp_dir().join(format!("luna-codex-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    (root, crate::throttle::now_ms() - 60_000)
}

/// A rollout saying `source`, as JSON; none for a file that says nothing.
pub(super) fn write_rollout(root: &Path, at_ms: u64, uuid: &str, source: Option<&str>) {
    let ts = Local.timestamp_millis_opt(at_ms as i64).single().unwrap().naive_local();
    let dir = day_dir(&root.to_string_lossy(), ts.date());
    std::fs::create_dir_all(&dir).unwrap();
    let name = format!("rollout-{}-{uuid}.jsonl", ts.format("%Y-%m-%dT%H-%M-%S"));
    let source = source.map(|s| format!(r#","source":{s}"#)).unwrap_or_default();
    std::fs::write(dir.join(name), format!(r#"{{"type":"session_meta","payload":{{"cwd":"E:\\p"{source}}}}}"#) + "\n").unwrap();
}

/// Where `write_rollout` puts the rollout of `uuid` begun at `at_ms`.
pub(super) fn rollout_path(root: &Path, uuid: &str, at_ms: u64) -> PathBuf {
    let ts = Local.timestamp_millis_opt(at_ms as i64).single().unwrap().naive_local();
    day_dir(&root.to_string_lossy(), ts.date()).join(format!("rollout-{}-{uuid}.jsonl", ts.format("%Y-%m-%dT%H-%M-%S")))
}

/// One turn event, `task_started` or an end, written at `ms`.
pub(super) fn turn_event(kind: &str, ms: u64) -> String {
    let at = chrono::DateTime::from_timestamp_millis(ms as i64).unwrap().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    format!("{{\"timestamp\":\"{at}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"{kind}\"}}}}\n")
}

pub(super) fn append(path: PathBuf, text: String) {
    use std::io::Write;
    std::fs::OpenOptions::new().append(true).open(path).unwrap().write_all(text.as_bytes()).unwrap();
}

/// The `source` of a subagent's rollout spawned by `parent`, as JSON.
pub(super) fn spawned_by_json(parent: &str) -> String {
    format!(r#"{{"subagent":{{"thread_spawn":{{"parent_thread_id":"{parent}","depth":1}}}}}}"#)
}
