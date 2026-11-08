//! A rollout read whole from its bytes alone: everything the rest of Luna
//! reads of one, by the same functions, with no file, lock or cache between.
//! What the fuzz target drives (fuzz/, through `luna_lib::transcripts`), and
//! what the fixture tests hold to the file-based readings.

use super::*;

/// What Luna reads of a rollout, each part as the function named on it reads
/// it of a file whose tail is the whole of it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RolloutRead {
    /// Where a reader from the top stops: the end of the last complete line
    /// (`messages_from`, `scan_rollout`).
    pub consumed: usize,
    /// `messages_from`.
    pub messages: Vec<crate::agents::Message>,
    /// The opening prompt, as a title (`read_first_prompt`).
    pub first_prompt: Option<String>,
    /// What the first line says: where the session works (`rollout_cwd`) and
    /// the thread that spawned it, for a subagent's (`spawned_by`).
    pub cwd: Option<String>,
    pub spawned_by: Option<String>,
    /// `read_tail`.
    pub tail: Tail,
    /// `last_model`, `answered_model`, `due_ms`.
    pub last_model: Option<String>,
    pub answered_model: Option<String>,
    pub due_ms: Option<u64>,
    /// The scan of the whole file (`scan_rollout`), turn ends counted from
    /// `since_ms` on: the turns ended, whether one is on, the question open
    /// with the call it came in, why the last turn stopped short, and what it
    /// ended saying, when.
    pub completed: u32,
    pub working: Option<bool>,
    pub ask: Option<(String, crate::agents::Ask)>,
    pub failed: Option<String>,
    pub last_said: Option<String>,
    pub last_said_at: Option<String>,
}

/// Everything Luna reads of a rollout, from its bytes, turn ends counted
/// from `since_ms` on as for a process spawned then.
pub fn read_rollout(bytes: &[u8], since_ms: i64) -> RolloutRead {
    let mut messages = Vec::new();
    let mut scan = Scan::default();
    let consumed = crate::pty::complete_lines_in(bytes, |line| {
        messages.extend(message_in(line));
        scan.line(line, since_ms);
    });
    // As `first_line` reads it: up to its newline, and none if it is no text.
    let first_end = bytes.iter().position(|b| *b == b'\n').map_or(bytes.len(), |i| i + 1);
    let first = std::str::from_utf8(&bytes[..first_end]).ok().and_then(|line| serde_json::from_str::<Value>(line).ok());
    // As a tail read has it.
    let text = String::from_utf8_lossy(bytes);
    let Scan { completed, working, ask, failed, last_said, last_said_at } = scan;
    RolloutRead {
        consumed,
        messages,
        first_prompt: first_prompt_in(crate::pty::text_lines(bytes)),
        cwd: first.as_ref().and_then(|v| v["payload"]["cwd"].as_str().map(str::to_owned)),
        spawned_by: first.as_ref().and_then(spawned_by),
        tail: tail_in(&text),
        last_model: last_model_in(&text),
        answered_model: answered_model_in(&text, None).or_else(|| answered_model_in(&text, Some(0))),
        due_ms: due_in(&text),
        completed,
        working,
        ask,
        failed,
        last_said,
        last_said_at,
    }
}
