//! Everything that knows Claude Code: its release channel, settings files,
//! trust bit, OAuth usage endpoint and token refresh, and the registry +
//! transcript a session leaves behind.

pub mod cli;
pub mod defaults;
pub mod limits;
pub mod models;
pub mod oauth;
pub mod session;
pub mod trust;
