//! Off Windows a job is the keeper's (keeper/), and no process is read: there a
//! job lists nothing.

pub use crate::keeper::{adopt, also_marked, await_environment, end_last_ties, from_args as keeper_from_args, join, let_go, tie, Job};

/// Nothing holds a pid here: the CLI is reaped as it ends, and what kills by
/// pid looks again before it does (keeper/job.rs `end_tree`).
pub struct Pin;

pub fn pin(_pid: u32) -> Pin {
    Pin
}

pub fn end(_pid: u32, _started_ms: u64) -> bool {
    false
}
pub fn windowed() -> std::collections::HashSet<u32> {
    Default::default()
}
pub fn parents() -> Vec<(u32, u32)> {
    Vec::new()
}
pub fn members(_job: &Job) -> Vec<u32> {
    Vec::new()
}
pub fn descendants(root: u32) -> Vec<u32> {
    vec![root]
}
pub fn read(_pid: u32) -> Option<super::ProcRead> {
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
pub fn names(_pids: &[u32]) -> Vec<(u32, String)> {
    Vec::new()
}
