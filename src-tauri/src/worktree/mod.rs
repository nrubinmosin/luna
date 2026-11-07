//! Git worktrees for isolated chats. Each CLI makes its own from
//! `--worktree`: Claude Code under `<folder>/.claude/worktrees/<name>` on a
//! branch `worktree-<name>`, Codex under `<CODEX_HOME>/worktrees/<4 hex>/<repo>`
//! on a detached HEAD. Luna only finds them again — to remove one with its
//! chat, or sweep the ones no chat claims — and never makes one itself.
//!
//! This module holds the git command they all run. The rest is split by what
//! it does:
//!
//! - `paths`: which path is a worktree of which folder, by its place and by
//!   what git records, and the project a path is of.
//! - `refs`: a checkout's HEAD read off its files, and origin fetched before
//!   Claude Code makes a worktree off it.
//! - `remove`: removing one with its throwaway branches.
//! - `orphans`: the worktrees no chat claims, and the sweep of them.
//! - `named`: a worktree asked for by name, whether it is still as made, who is
//!   in one, and what a cut-off spawn may have begun.
//! - `inspect`: what deleting one would throw away.

use crate::paths::{path_key as norm, path_key_loose as norm_in_use, KEY_SEP};
use std::path::{Path, PathBuf};
use std::process::Command;

mod inspect;
mod named;
mod orphans;
mod paths;
mod refs;
mod remove;
#[cfg(test)]
mod testkit;

// Every file of the module reaches the others' items through `use super::*`,
// and the rest of the crate keeps its `crate::worktree::X` paths.
pub(crate) use inspect::*;
pub(crate) use named::*;
pub(crate) use orphans::*;
pub(crate) use paths::*;
pub(crate) use refs::*;
pub(crate) use remove::*;
#[cfg(test)]
use testkit::*;

fn git(folder: &str) -> Command {
    let mut c = Command::new("git");
    c.arg("-C").arg(folder);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

/// The 8.3 short spelling of an existing folder where its volume keeps one, else
/// the path as it is: what git says of a repository comes in long names, and a
/// test spelling its own in short ones has Luna tell the two apart as a user's
/// temp folder under a long user name (`C:\Users\RUNNER~1\…`) would.
#[cfg(test)]
pub(crate) fn short_spelling(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // `%~sI` is cmd's own GetShortPathName.
        let out = Command::new("cmd").raw_arg(format!("/d /c for %I in (\"{}\") do @echo %~sI", path.display())).output();
        if let Some(short) = out.ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()) {
            if !short.is_empty() && Path::new(&short).is_dir() {
                return PathBuf::from(short);
            }
        }
    }
    path.to_path_buf()
}
