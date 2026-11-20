//! What Luna applies to a CLI it starts, item by item, each with its class and
//! the reason it is there (docs/adr/0016-luna-is-a-runtime.md): the launch
//! builders (claude/launch.rs, codex/launch.rs, background/command.rs) return
//! these beside the arguments. The launch goldens (golden.rs
//! `assert_shown`) check that every flag, `-c` key and variable of a start is
//! named by an item.
//!
//! A value never holds a secret: a token is named by the variable it travels
//! in, a hub URL by what it is for, a file Luna writes by its path.

/// Why Luna adds something to a CLI's start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// What Luna needs to run the CLI as one of its sessions.
    Runtime,
    /// What keeps a session out of Luna's own secrets, and a helper, briefed
    /// by an agent rather than the person, within what Luna lets it reach.
    Boundary,
    /// What the person chose: in the new-chat dialog, Settings, the library,
    /// or an agent's request they gave it tools to make.
    UserAction,
    /// A fix for something the CLI cannot do on this machine without it.
    Workaround,
}

/// Declares every item Luna may apply: its key, its class and its why, once.
/// Each item is a variant, so a builder cannot report one without them, and
/// `ALL` lets a test check every one.
macro_rules! items {
    ($($name:ident = $key:literal, $class:ident, $why:literal;)*) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Item { $($name),* }
        impl Item {
            #[cfg(test)]
            pub const ALL: &'static [Item] = &[$(Item::$name),*];
            pub fn key(self) -> &'static str {
                match self { $(Item::$name => $key),* }
            }
            pub fn class(self) -> Class {
                match self { $(Item::$name => Class::$class),* }
            }
            pub fn why(self) -> &'static str {
                match self { $(Item::$name => $why),* }
            }
        }
    };
}

items! {
    Account = "account", Runtime,
        "The account this chat belongs to: the CLI reads its settings and sign-in from there.";
    Folder = "folder", Runtime,
        "Where the chat works.";
    Cwd = "cwd", Runtime,
        "The folder the CLI's process starts in: the chat's own, or the worktree it resumes in.";
    Terminal = "terminal", Runtime,
        "The CLI draws into Luna's terminal, which speaks xterm with 256 colors.";
    InheritedCleared = "inherited-env-cleared", Runtime,
        "Variables a session that started Luna would pass down are removed: they would make the CLI take itself for that session's child.";
    SessionId = "session-id", Runtime,
        "Tells the session, and what it runs, which Luna chat it is (and a helper which chat started it).";
    Resume = "resume", Runtime,
        "Continues this chat's own conversation.";
    Model = "model", UserAction,
        "As chosen for this chat: in the new-chat dialog, or from the account's settings.";
    Effort = "effort", UserAction,
        "As chosen for this chat: in the new-chat dialog, or from the account's settings.";
    PermissionMode = "permission-mode", UserAction,
        "As chosen for this chat: in the new-chat dialog, or from the account's settings.";
    Approval = "approval", UserAction,
        "As chosen for this chat: in the new-chat dialog, or from the account's config.toml.";
    Sandbox = "sandbox", UserAction,
        "As chosen for this chat: in the new-chat dialog, or from the account's config.toml.";
    Worktree = "worktree", UserAction,
        "The Git worktree box: the CLI works in a checkout of its own.";
    Fullscreen = "claude.fullscreen", Runtime,
        "Claude Code draws on the alternate screen with its own scrollback, as Luna's terminal wants, whatever the account's settings say: asked for at every start, nothing written into the account.";
    TuiSettings = "codex.tui-settings", Runtime,
        "Codex's TUI is told, at every start and nothing written into the account, to raise no desktop notification (Luna says when a chat waits) and to copy a selection only by the terminal's own copy.";
    NoAutoUpdate = "no-self-update", Runtime,
        "Luna installs and updates the CLI itself; the CLI's own updater would install or announce a copy nothing here runs.";
    ActivityHooks = "activity-hooks", Runtime,
        "How Luna learns that the chat works, waits or asks: each event is posted to Luna's local listener, and they print nothing.";
    CoreReadDeny = "core-read-deny", Boundary,
        "Keeps the session's file tools out of Luna's core folder, whose token reaches every session (threat model F3).";
    LunaTools = "luna-tools", UserAction,
        "The Luna tools box (or the agent that spawned it): Luna's MCP server, its tools allowed without a prompt.";
    NotesDir = "notes-dir", Runtime,
        "Long prompts, messages and helpers' answers from Luna arrive as files there, readable without asking.";
    HelperIdentity = "helper.identity", Runtime,
        "A task from another agent carries a note after it: the session answers to that agent, not to a person. Its text is a system text, yours to change in the Library window.";
    RoleLayers = "role.layers", Runtime,
        "What the session is told of its role, at memory level: Luna's default for its role (a system text in the Library window), the library's instructions for its role, and its constraints, in that order. Claude Code reads them as the CLAUDE.md of a folder of Luna's; Codex in developer_instructions, after its config's own value.";
    RoleLayersUnsure = "role.layers-unsure", Runtime,
        "What Luna gives Codex in developer_instructions may not be all the session gets: a config layer Luna could not read, or one that comes above the command line.";
    RoleLayersOff = "role.layers-off", UserAction,
        "This chat starts without its role layers: on Codex, its config's own developer_instructions leave no room on the command line even for a pointer to the file that holds the layers, or Luna could not write that file (shorten the config's value); on Claude Code, Luna could not write the file it reads them from.";
    RoleLayersKept = "role.layers-kept", Runtime,
        "You took this Codex conversation over from an agent: codex resume keeps the developer message its thread was started with, so it still has the role and constraints it was given as a helper, and a chat's role layers do not reach it until Codex compacts its context. Luna no longer counts it a helper: no helper it starts inherits them. Start a new chat for one without them.";
    Constraints = "constraints", UserAction,
        "What the agents that started this session asked it, and every session under it, to keep to, word for word: the last of its role layers. Text it is told, not a permission it lacks.";
    HelperHeadless = "helper.headless", Runtime,
        "A helper runs without a terminal: its CLI's headless mode, its output read as a stream.";
    HelperPermissionPrompt = "helper.permission-prompt", Runtime,
        "A helper's permission prompts and questions go to the session that started it: Claude Code's over its own stdin and stdout, Codex's as its app-server's requests.";
    HelperMcpAlone = "helper.luna-mcp-alone", Boundary,
        "A helper is briefed by an agent, not by you: it gets none of the account's connectors or MCP servers, unless the agent that spawned it asked for them (connectors: true). Luna's MCP server alone: for Codex, every server its account, the system and the project define and every plugin but the library's are turned off for the run.";
    HelperNoMemory = "helper.no-auto-memory", Boundary,
        "What a helper saved to the account's auto-memory would reach every later session of the account unseen: its auto-memory is off.";
    HelperBackgroundWait = "helper.background-wait", Runtime,
        "A helper's turn ends with its CLI's stdin closed, and Claude Code would then stop the subagents and jobs it put in the background after 10 minutes, their work unfinished: it waits for them up to 3 hours, so a forgotten watcher cannot hold the helper for good. Until then the turn goes on; one that waits half an hour without a word is told to the chat that started it, with what it waits on, and luna_session_stop ends it.";
    HelperTrust = "helper.trust", Runtime,
        "A helper has nobody to answer Codex's trust screen: the folder it was sent to is trusted for this run only, nothing written.";
    ReadOnly = "read-only", UserAction,
        "The agent that spawned it asked for it without the edit tools.";
    Agent = "agent", UserAction,
        "The agent that spawned it asked for it to run as this agent.";
    TrustedRoot = "trusted-root", UserAction,
        "Settings › Trusted roots: the folder is under one, trusted for this run, nothing written.";
    SessionTrust = "session-trust", UserAction,
        "You asked for this folder to be trusted while a Codex session held the account's config.toml: trusted for runs of this Luna instead.";
    NoDaemon = "codex.no-daemon", Runtime,
        "The session runs in Luna's terminal, not in Codex's shared background server, so Luna can stop and count it.";
    ThreadTitle = "codex.thread-title", Runtime,
        "Codex puts the thread's id in the terminal's title, which no pane shows: how Luna follows the chat's thread.";
    WindowsSandbox = "codex.windows-sandbox", Workaround,
        "Without a Windows sandbox Codex refuses every command in this sandbox mode; you set none, so Luna picks the unelevated one.";
    SandboxPath = "codex.sandbox-path", Workaround,
        "PATH without the Store's app aliases, which Codex's Windows sandbox cannot start: it would fail every command.";
    GitSslBackend = "codex.git-ssl-backend", Workaround,
        "git's HTTPS through OpenSSL: Windows' own TLS fails under the sandbox's restricted token.";
    LibrarySkills = "library.skills", UserAction,
        "The library gives sessions in this folder these skills.";
    LibraryHidden = "library.hidden-skills", UserAction,
        "The library keeps these skills from this session: a folder it works in, or whose sessions you set them for.";
    LibraryAccountInstructions = "library.account-instructions-off", UserAction,
        "The library gives this session other instructions than the account's own, or none: a folder it works in, or whose sessions you set them for.";
    LibraryAccountInstructionsKept = "library.account-instructions-kept", Workaround,
        "The account's instructions are read after all: the only pattern that leaves them out would also match this project's own instructions in Claude Code's matcher.";
    LibraryAudienceUnkept = "library.audience-unkept", UserAction,
        "In the Library window you set these for the person's own chats alone, but this session reads them all the same: Codex has no way to leave its account's AGENTS.md out, and the copy there does not say whose it is.";
    LibraryAudience = "library.audience", UserAction,
        "In the Library window you set these for some roles only: given to this session alone, or kept from it, by its role.";
    LibraryInstructions = "library.instructions", UserAction,
        "The session's role layers — the library's instructions for this folder or its role, its role's text, its constraints — read as memory from a folder of Luna's.";
    MemoryNarrowed = "library.memory-narrowed", Runtime,
        "Reading the library's instructions as memory would read the other folders added to the session too: their CLAUDE.md files are left out.";
    MemoryNotNarrowed = "library.memory-not-narrowed", Workaround,
        "This folder's memory files are read after all: the only pattern that leaves them out would also match this project's own instructions in Claude Code's matcher.";
}

/// One thing Luna applied: what, its class, the value as it went (secrets
/// left out), and why.
#[derive(Clone, Debug, PartialEq)]
pub struct Applied {
    pub key: &'static str,
    pub class: Class,
    pub value: String,
    pub why: &'static str,
}

impl Item {
    pub fn with(self, value: impl Into<String>) -> Applied {
        Applied { key: self.key(), class: self.class(), value: value.into(), why: self.why() }
    }
}

/// 0.19's ledger of what Luna wrote into accounts outside a launch, and the
/// note beside it: nothing reads them now, so they are taken out of the data
/// folder, which is Luna's.
pub fn drop_0_19_ledger() {
    for name in ["applied-ledger.json", "applied-ledger.incomplete"] {
        let path = crate::paths::data_dir().join(name);
        match std::fs::remove_file(&path) {
            Ok(()) => crate::log::info("applied", &format!("0.19's {} removed: nothing reads it now", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => crate::log::warn("applied", &format!("{} left where it is: {e}", path.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// New code that applies something to a CLI declares it as an item, and
    /// an item cannot be declared without a class; this keeps each one's key
    /// its own and its why a sentence.
    #[test]
    fn every_item_has_a_class_a_why_and_a_key_of_its_own() {
        let mut keys = std::collections::HashSet::new();
        for item in Item::ALL {
            assert!(keys.insert(item.key()), "{item:?}: key {} taken twice", item.key());
            assert!(item.why().len() > 20 && item.why().ends_with('.'), "{item:?}: why is a sentence: {}", item.why());
            let a = item.with("v");
            assert_eq!((a.key, a.class, a.why), (item.key(), item.class(), item.why()));
        }
    }
}
