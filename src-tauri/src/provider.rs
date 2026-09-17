//! The two CLIs Luna can drive. Everything provider-specific lives under
//! `claude/` and `codex/`; this is only the tag that says which one a session,
//! an account or a CLI status belongs to.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Claude,
    Codex,
}

impl Provider {
    pub const ALL: [Provider; 2] = [Provider::Claude, Provider::Codex];

    pub fn as_str(self) -> &'static str {
        match self {
            Provider::Claude => "claude",
            Provider::Codex => "codex",
        }
    }

    /// Sub-folder of the accounts root that holds this provider's accounts.
    pub fn accounts_dir(self) -> &'static str {
        match self {
            Provider::Claude => "anthropic",
            Provider::Codex => "openai",
        }
    }

    pub fn index(self) -> usize {
        match self {
            Provider::Claude => 0,
            Provider::Codex => 1,
        }
    }
}
