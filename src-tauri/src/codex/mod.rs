//! Everything that knows Codex: its GitHub releases, `config.toml`, project
//! trust, the ChatGPT usage endpoint, and the rollout files a session writes.

pub mod cli;
pub mod config;
pub mod defaults;
pub mod limits;
pub mod session;
pub mod trust;
