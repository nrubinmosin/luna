//! Everything that knows Codex: how a session is started, its GitHub
//! releases, `config.toml`, project trust, the ChatGPT usage endpoint, the
//! rollout files a session writes, and the SIDs its Windows sandbox keeps.

pub mod cli;
pub mod config;
pub mod defaults;
pub mod hooks;
pub mod launch;
pub mod limits;
pub mod models;
pub mod sandbox;
pub mod session;
pub mod trust;
