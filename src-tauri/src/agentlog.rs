//! What agents have spawned, kept past the rows: a helper deleted when its
//! work was done leaves nothing in `luna_sessions`, and the next run that
//! means to repeat it — same model, account, prompt — has nothing to go by.
//! One JSON line per spawn and one per end in `<data>/agents.jsonl`, read
//! back by `luna_sessions` with `history: true`.

use serde::Serialize;
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Past this many lines the file is cut to its newest half on the next write.
const MAX_LINES: usize = 2000;
/// How much of an opening prompt is kept: enough to tell runs apart and to
/// see what a slash command was given.
const PROMPT_CHARS: usize = 300;
/// The most one lookup returns, newest first.
const MAX_RUNS: usize = 30;

fn file() -> PathBuf {
    crate::paths::data_dir().join("agents.jsonl")
}

fn lock() -> &'static Mutex<()> {
    static L: Mutex<()> = Mutex::new(());
    &L
}

/// A spawn as it is written down.
pub struct Spawn<'a> {
    pub id: &'a str,
    pub parent: &'a str,
    pub parent_name: Option<String>,
    pub folder: &'a str,
    pub worktree: bool,
    pub provider: &'a str,
    pub account: &'a str,
    pub settings: &'a Value,
    pub name: Option<String>,
    pub prompt: &'a str,
    pub auto_delete: bool,
    /// Runs with no terminal and no row (background.rs).
    pub background: bool,
}

pub fn spawned(s: &Spawn) {
    let prompt: String = s.prompt.chars().take(PROMPT_CHARS).collect();
    append(
        &file(),
        json!({
            "at": now(),
            "event": "spawn",
            "id": s.id,
            "parent": s.parent,
            "parentName": s.parent_name,
            "folder": s.folder,
            "worktree": s.worktree,
            "provider": s.provider,
            "account": s.account,
            "settings": s.settings,
            "name": s.name,
            "prompt": prompt,
            "autoDelete": s.auto_delete,
            "background": s.background,
        }),
    );
}

/// A spawned session's row is gone: `deleted` (by its agent or by hand) or
/// `auto-deleted`.
pub fn ended(id: &str, how: &str) {
    append(&file(), json!({ "at": now(), "event": "end", "id": id, "how": how }));
}

fn now() -> String {
    chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%:z").to_string()
}

fn append(path: &Path, line: Value) {
    let _held = lock().lock().unwrap_or_else(|e| e.into_inner());
    if let Ok(text) = std::fs::read_to_string(path) {
        let lines: Vec<&str> = text.lines().collect();
        if lines.len() >= MAX_LINES {
            let keep = lines[lines.len() - MAX_LINES / 2..].join("\n") + "\n";
            if let Err(e) = std::fs::write(path, keep) {
                crate::log::warn("agents", &format!("could not trim {}: {e}", path.display()));
            }
        }
    }
    let written = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut f| writeln!(f, "{line}"));
    if let Err(e) = written {
        crate::log::warn("agents", &format!("could not write {}: {e}", path.display()));
    }
}

/// One past run: the spawn with its end, if it has ended.
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub id: String,
    pub at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub how: Option<String>,
    pub parent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_name: Option<String>,
    pub folder: String,
    pub worktree: bool,
    pub provider: String,
    pub account: String,
    pub settings: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub prompt: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub auto_delete: bool,
}

/// Past runs, newest first, those with `q` in their name, folder, account,
/// id, prompt or parent's name.
pub fn runs(q: Option<&str>) -> Vec<Run> {
    let _held = lock().lock().unwrap_or_else(|e| e.into_inner());
    runs_in(&file(), q)
}

fn runs_in(path: &Path, q: Option<&str>) -> Vec<Run> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut runs: Vec<Run> = Vec::new();
    for v in text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()) {
        let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
        let opt = |k: &str| v[k].as_str().map(str::to_owned);
        match v["event"].as_str() {
            Some("spawn") => runs.push(Run {
                id: s("id"),
                at: s("at"),
                ended: None,
                how: None,
                parent: s("parent"),
                parent_name: opt("parentName"),
                folder: s("folder"),
                worktree: v["worktree"].as_bool().unwrap_or(false),
                provider: s("provider"),
                account: s("account"),
                settings: v["settings"].clone(),
                name: opt("name"),
                prompt: s("prompt"),
                auto_delete: v["autoDelete"].as_bool().unwrap_or(false),
            }),
            Some("end") => {
                let id = s("id");
                if let Some(run) = runs.iter_mut().rev().find(|r| r.id == id && r.ended.is_none()) {
                    run.ended = opt("at");
                    run.how = opt("how");
                }
            }
            _ => {}
        }
    }
    let q = q.map(str::trim).filter(|q| !q.is_empty()).map(str::to_lowercase);
    if let Some(q) = q {
        runs.retain(|r| {
            r.name
                .iter()
                .chain(r.parent_name.iter())
                .chain([&r.folder, &r.account, &r.id, &r.prompt])
                .any(|f| f.to_lowercase().contains(&q))
        });
    }
    runs.reverse();
    runs.truncate(MAX_RUNS);
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_fold_their_ends_and_come_newest_first() {
        let path = std::env::temp_dir().join(format!("luna-agentlog-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let settings = json!({ "model": "sonnet" });
        for (id, folder, prompt) in [("a1", "C:\\work\\hh", "/hh-process"), ("a2", "C:\\work\\luna", "fix the build")] {
            append(&path, json!({ "at": "t", "event": "spawn", "id": id, "parent": "p", "folder": folder,
                "provider": "claude", "account": "team", "settings": settings, "prompt": prompt }));
        }
        append(&path, json!({ "at": "t2", "event": "end", "id": "a1", "how": "deleted" }));
        std::fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(b"not json\n").unwrap();

        let all = runs_in(&path, None);
        assert_eq!(all.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["a2", "a1"]);
        assert_eq!(all[1].how.as_deref(), Some("deleted"));
        assert_eq!(all[0].ended, None);
        let hh = runs_in(&path, Some("HH-"));
        assert_eq!(hh.len(), 1, "the prompt and the folder are searched, any case");
        assert_eq!(hh[0].settings["model"], "sonnet");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_full_file_keeps_its_newest_half() {
        let path = std::env::temp_dir().join(format!("luna-agentlog-trim-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&path);
        for i in 0..MAX_LINES {
            append(&path, json!({ "n": i }));
        }
        append(&path, json!({ "n": "last" }));
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), MAX_LINES / 2 + 1);
        assert!(text.lines().last().unwrap().contains("last"));
        let _ = std::fs::remove_file(&path);
    }
}
