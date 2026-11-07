//! What a Claude Code session leaves on disk, and how Luna reads it back:
//! the live registry at `<config>/sessions/<pid>.json` (name, status, cwd,
//! session id) and the transcript at `<config>/projects/<cwd>/<sid>.jsonl`
//! (context usage, the titles the CLI gave it, opening prompt). All of it is
//! free to read — no tokens.
//!
//! This module holds the bound its caches share. The rest is split by what it
//! reads:
//!
//! - `registry`: the live registry, and what a live session is found and
//!   described by through it.
//! - `transcript`: where a session's transcript is, and the conversation read
//!   from it for another agent.
//! - `ending`: what the end of a transcript says: the context and model of the
//!   last answer, the last words, an API error, the tool call it stands on and
//!   when what it waits on is due.
//! - `titles`: the titles the CLI gives a session, and its opening prompt.

use crate::paths::path_key_loose as norm_path;
use crate::pty::{sane_title, CliTitle, SessionMeta};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

mod ending;
mod registry;
mod titles;
mod transcript;

// Every file of the module reaches the others' items through `use super::*`,
// and the rest of the crate keeps its `crate::claude::session::X` paths.
pub use ending::*;
pub use registry::*;
pub use titles::*;
pub use transcript::*;

/// How many conversations the per-conversation caches hold before starting
/// over: each `/clear` and resume is a new one, and the app runs for weeks.
const CACHE_CAP: usize = 512;
