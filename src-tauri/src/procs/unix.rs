//! Off Windows a job is the keeper's (keeper/): its turns' processes, what
//! hangs from them by parent, group or session, and what carries its mark,
//! read off the process table at each look (`keeper::listed`). A start time
//! here is the OS's own count, comparable only with another's.

pub use crate::keeper::{
    adopt, also_marked, await_environment, end_last_ties, from_args as keeper_from_args, join, let_go, marked, started_of, tie, Job,
};

/// Nothing holds a pid here: the CLI is reaped as it ends, and what kills by
/// pid looks again before it does (keeper/job.rs `end_tree`).
pub struct Pin;

pub fn pin(_pid: u32) -> Pin {
    Pin
}

/// Ends one process if it is still the one that started at `started`.
pub fn end(pid: u32, started: u64) -> bool {
    crate::keeper::end_listed(pid, started)
}

/// Nothing tells a window here: none is spared.
pub fn windowed() -> std::collections::HashSet<u32> {
    Default::default()
}

pub fn parents() -> Vec<(u32, u32)> {
    Vec::new()
}

/// What the job holds now, by pid, start and name; None where the process
/// table cannot be read.
pub fn list(job: &Job) -> Option<Vec<(super::ProcRead, String)>> {
    Some(crate::keeper::listed(job)?.into_iter().map(|(pid, started_ms, name)| (super::ProcRead { pid, started_ms }, name)).collect())
}

/// What in the job the turn whose process was `cli` started: its process
/// group and session (`keeper::listed_of`), as no parent links are read here.
pub fn tree_of(job: &Job, cli: u32) -> Option<Vec<(super::ProcRead, String)>> {
    Some(
        crate::keeper::listed_of(job, cli)?
            .into_iter()
            .map(|(pid, started_ms, name)| (super::ProcRead { pid, started_ms }, name))
            .collect(),
    )
}

/// Every CLI here has a job (`adopt` never refuses): with none, nothing is known.
pub fn list_below(_root: u32) -> Option<Vec<(super::ProcRead, String)>> {
    None
}

/// `kill -0` answers while the process lives.
pub fn alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}
