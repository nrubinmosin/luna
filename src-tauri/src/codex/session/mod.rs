//! What a Codex session leaves on disk, and how Luna reads it back.
//!
//! Codex has no live registry; it writes one rollout file per thread at
//! `<CODEX_HOME>/sessions/YYYY/MM/DD/rollout-<local ts>-<uuid>.jsonl`, one
//! JSON object per line: `{"timestamp","type","payload"}`. The first line is
//! `session_meta` (thread id, cwd); `turn_context` lines carry the model;
//! `event_msg` lines mark the turn — `task_started` / `task_complete` since
//! 0.157, `turn_started` / `turn_complete` / `turn_aborted` before — and carry
//! `token_count` (context usage plus rate limits); `response_item` lines of
//! type `message` are the conversation itself, `role` user / assistant /
//! developer with `input_text` / `output_text` blocks. All of it is free to
//! read — no tokens.
//!
//! Which file is ours is the one thing to work out: a fresh session gets the
//! newest rollout whose cwd matches and that appeared after the spawn, and a
//! resumed one is found by the uuid in its name. Either way the answer is
//! remembered per chat, so two chats in one folder never share a file.
//!
//! This module holds a live session as the rest of Luna describes it. The
//! rest is split by what it does:
//!
//! - `rollouts`: the rollout files by day and by thread, and which one is a
//!   chat's.
//! - `scan`: the running read of a chat's rollout: its turns, a failure, an
//!   open question, the subagents at work.
//! - `read`: what is read of a rollout on demand: the opening prompt, the
//!   conversation, the tail, the model, the status and meta of a session.

use crate::paths::path_key_loose as norm_path;
use crate::pty::{sane_title, tail, title_from_prompt, CliTitle, SessionMeta};
use chrono::{Datelike, Local, NaiveDate, NaiveDateTime, TimeZone, Timelike};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

mod read;
mod rollouts;
mod scan;
#[cfg(test)]
mod testkit;

// Every file of the module reaches the others' items through `use super::*`,
// and the rest of the crate keeps its `crate::codex::session::X` paths.
pub(crate) use read::*;
pub(crate) use rollouts::*;
pub(crate) use scan::*;
#[cfg(test)]
use testkit::*;

/// A turn is in flight but the screen has not changed for this long: Codex
/// paints its spinner continuously while it works, so a still screen mid-turn
/// is an approval prompt (which the rollout does not record).
const STILL_MS: u64 = 3000;

pub struct Live<'a> {
    pub chat_id: &'a str,
    pub cwd: &'a str,
    pub spawned_at_ms: u128,
    /// The thread id the session was resumed with, if any.
    pub resume: Option<&'a str>,
    pub account_path: &'a str,
    pub last_output_ms: u64,
}
