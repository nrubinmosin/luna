//! The tie to the core off Windows: what `procs::tie` is, by other means.
//!
//! Windows ends a job when the core's last handle to it closes. Linux and macOS
//! have no such object, so a small process stands in for it: the keeper, this
//! same exe started with `--keep-ties`, one to a core, once a helper has a turn
//! to tie. It reads a pipe only the core holds the other end of. However the
//! core goes the OS closes that end, so the end of its input is the core's
//! death: it ends every job it was told of and not since told to let go of. A
//! core that leaves lets go of them all first, and the same end of input ends
//! nothing. `PR_SET_PDEATHSIG` would not do: it fires as the *thread* that
//! spawned the process ends, and turns start on threads that do not last; it
//! reaches that one child and nothing it started; macOS has no such thing.
//!
//! The end of its input is not all it goes by: on macOS a pipe becomes
//! close-on-exec a moment after it is made, and a process started in that moment
//! keeps the write end for its life. So it also looks at the core itself now and
//! then, by pid and start time.
//!
//! A keeper can go with its core (`killall -9`, the OOM killer), so a core also
//! writes down what it has tied as it ties it, a file to a job, and the next one
//! ends what is left of those before it takes the helpers up (`end_last_ties`).
//! That alone would keep a resumed turn from overlapping the old one, but it
//! would leave a dead core's turns running until a next core starts, which may be
//! never. A file to a job so that letting go of one is one unlink, which a full
//! disk does not refuse: a released job must not be there for the next core to
//! end. A job not seen out stays written down, for the core after to try again.
//!
//! What is in a job. A process group is not enough: both CLIs start each tool
//! command in a session of its own (Claude Code spawns it `detached`, Codex
//! calls `setsid`), and what such a command leaves behind is re-parented away
//! from the turn. So a job is, at any one look, whatever has its mark in the
//! environment (`MARK`, which goes down through a new session and a new parent),
//! the turns' own processes by pid and start time with their groups, and
//! everything below any of those by parent link. Codex filters what its tools
//! inherit by its config, so there the mark may not get through; `CODEX_THREAD_ID`
//! it sets after the filter, and that counts too (`also_marked`). A turn whose own
//! process does not show its mark is not counted as tied at all (`tie`). Only a
//! process that both cleared its environment and lost its parents gets away.
//!
//! No window spared and none could be: a dead core takes the programs a helper
//! opened, as it does on Windows. Every way out that is not a death lets go.
//!
//! A stop that is chosen ends a tree instead (`end_tree`, for `pty::kill_tree`),
//! which is `taskkill /T` here: the process and all that hangs from it, the tool
//! commands with what they started. The same walk without the mark: what only
//! the mark ties to the job has outlived its tool, and nothing here tells a job
//! left running from a program with a window, so it is left, as `let_go` leaves
//! it.
//!
//! This module holds the names and timings all of it shares, a process as one
//! look finds it, a turn's process and the record of a tied job. The rest is
//! split by what it does:
//!
//! - `system`: what the OS says of a process — the list of them, one's
//!   parents, start time, state and environment — and signals, per OS.
//! - `job`: what a job is at one look, and ending one: a tied job at a core's
//!   death, a tree for a stop that is chosen.
//! - `process`: the keeper itself, the process started with `--keep-ties`.
//! - `ties`: the core's side: the keeper it starts, what it tells it, the
//!   records it writes down, and what the next core ends of them.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::io::{Read, Write};
use std::os::fd::FromRawFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

mod job;
mod process;
mod system;
#[cfg(test)]
mod testkit;
mod ties;

// Every file of the module reaches the others' items through `use super::*`,
// and the rest of the crate keeps its `crate::keeper::X` paths.
pub use job::*;
pub use process::*;
use system::*;
#[cfg(test)]
use testkit::*;
pub use ties::*;

const FLAG: &str = "--keep-ties";
/// Set on a helper's turn, so on all it starts. No KEY, SECRET or TOKEN in the
/// name: Codex keeps those from its tools by default.
pub const MARK: &str = "LUNA_JOB";
/// The keeper's own value of `MARK`: an `end_tree` of its core leaves it, to do
/// its work as for any other death.
const KEEPER: &str = "keeper";
/// How long a look at a job goes on finding what will not die: a process stuck
/// in the kernel. The job then counts as not ended.
const ENDING_TAKES: Duration = Duration::from_secs(5);
const LOOK_AGAIN: Duration = Duration::from_millis(25);
/// Looks at a tree being frozen before it is ended whatever is still moving: what
/// has stopped cannot fork, so only a fork bomb, or one stuck in the kernel and not
/// stopping, takes more than a few.
const FREEZE_LOOKS: usize = 50;
/// How long the keeper listens before it looks at the core itself. The pipe says
/// at once; this is for a pipe something else holds open as well.
const CORE_LOOK_MS: i32 = 2000;
/// A keeper that lasted less than this is not started again by itself.
const KEEPER_LASTS: Duration = Duration::from_secs(5);
/// How long a turn just started may take to have an environment to read.
const ENVIRONMENT_WITHIN: Duration = Duration::from_secs(1);

/// One process, as one look found it. One that has exited is not looked at.
#[derive(Clone, Debug, PartialEq)]
struct Proc {
    pid: i32,
    parent: i32,
    group: i32,
    session: i32,
    /// Ticks since boot on Linux, microseconds since the epoch on macOS: only
    /// ever compared with its like, on this machine.
    started: u64,
    name: String,
}

/// A turn's process. Its pid may be another's by now; the start time tells.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
struct Leader {
    pid: i32,
    started: u64,
}

fn leader(pid: u32) -> Option<Leader> {
    os::read(pid as i32).map(|p| Leader { pid: p.pid, started: p.started })
}

/// A tied job, as told to the keeper and written down for the next core.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Record {
    id: String,
    mark: String,
    /// The boot it was written in: of another, all it names ended with that.
    #[serde(default)]
    boot: Option<String>,
    /// What else in an environment says it is of this job (`also_marked`): for a
    /// process born since the job began, as what a core let go of may have it too.
    #[serde(default)]
    also: Option<String>,
    #[serde(default)]
    since: u64,
    leaders: Vec<Leader>,
}
