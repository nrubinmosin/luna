//! What Luna applies to a CLI it starts, item by item, each with its class and
//! the reason it is there (docs/adr/0016-luna-is-a-runtime.md): the launch
//! builders (claude/launch.rs, codex/launch.rs, background/command.rs) return
//! these beside the arguments, the registry keeps a session's latest
//! (agents/registry.rs `set_applied`), and what Luna writes into an account
//! outside a launch goes in a ledger here (`record`). The window shows both
//! in a chat's "What Luna applied" (`applied_by_luna`).
//!
//! A value never holds a secret: a token is named by the variable it travels
//! in, a hub URL by what it is for, a file Luna writes by its path.

use serde::{Deserialize, Serialize};

/// Why Luna adds something to a CLI's start.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
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
            fn by_key(key: &str) -> Option<Item> {
                match key { $($key => Some(Item::$name),)* _ => None }
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
    NoAutoUpdate = "no-self-update", Runtime,
        "Luna installs and updates the CLI itself; the CLI's own updater would install or announce a copy nothing here runs.";
    ActivityHooks = "activity-hooks", Runtime,
        "How Luna learns that the chat works, waits or asks: each event is posted to Luna's local listener, and they print nothing.";
    CoreReadDeny = "core-read-deny", Boundary,
        "Keeps the session's file tools out of Luna's core folder, whose token reaches every session (threat model F3).";
    Guard = "guard", UserAction,
        "The Guard against endless waits box: a command that would wait forever is refused before it runs.";
    HelperGuard = "helper.guard", Runtime,
        "A helper has nobody to stop a command that waits forever: such a command is refused before it runs.";
    LunaTools = "luna-tools", UserAction,
        "The Luna tools box (or the agent that spawned it): Luna's MCP server, its tools allowed without a prompt.";
    HandoffTool = "handoff-tool", UserAction,
        "The Luna tools box: luna_session_handoff, which starts the chat's next conversation from a handoff it writes, answering only when Luna asks for one (Hand off now in the pane's menu, or auto-handoff). It writes the file in a private drafts folder of its own in Luna's data folder, given to it with the copies' folder (Claude Code, read without asking) or alone and writable (Codex, where its sandbox writes).";
    AutoHandoff = "auto-handoff", UserAction,
        "The Auto-handoff box: past this share of its context Luna asks the chat for a handoff through luna_session_handoff.";
    MovedChat = "moved-chat", UserAction,
        "You moved this chat here from another account: the document you confirmed for it is in this folder of Luna's, read without asking.";
    NotesDir = "notes-dir", Runtime,
        "Long prompts, messages and helpers' answers from Luna arrive as files there, readable without asking.";
    HelperIdentity = "helper.identity", Runtime,
        "A task from another agent carries a note after it: the session answers to that agent, not to a person. Its text is a system text, yours to change in the Library window.";
    RoleLayers = "role.layers", Runtime,
        "What the session is told of its role, at memory level: Luna's default for its role (a system text in the Library window), the library's instructions for its role, and its constraints, in that order. Claude Code reads them as the CLAUDE.md of a folder of Luna's; Codex in developer_instructions, after its config's own value.";
    RoleLayersUnsure = "role.layers-unsure", Runtime,
        "What Luna gives Codex in developer_instructions may not be all the session gets: a config layer Luna could not read, or one that comes above the command line.";
    RoleLayersOff = "role.layers-off", UserAction,
        "This chat starts without its role layers: with its config's own developer_instructions they do not fit on Codex's command line. Shorten the library's instructions for its role, or its role's text.";
    Constraints = "constraints", UserAction,
        "What the agents that started this session asked it, and every session under it, to keep to, word for word: the last of its role layers. Text it is told, not a permission it lacks.";
    HelperHeadless = "helper.headless", Runtime,
        "A helper runs without a terminal: its CLI's headless mode, its output read as a stream.";
    HelperPermissionPrompt = "helper.permission-prompt", Runtime,
        "A helper's permission prompts go to the chat that started it, through Luna's tool.";
    HelperApprovalNever = "helper.approval", Runtime,
        "codex exec has nobody to ask: the sandbox chosen for it is what holds it.";
    HelperReviewer = "helper.reviewer", Runtime,
        "What a helper would ask goes to the chat that started it, never to a reviewer model an account or project names.";
    HelperMcpAlone = "helper.luna-mcp-alone", Boundary,
        "A helper is briefed by an agent, not by you: it gets none of the account's connectors or MCP servers, unless the agent that spawned it asked for them (connectors: true). Luna's MCP server alone for Claude Code; a Codex helper may keep its project's and plugins' servers, listed beside this if it does.";
    HelperMcpLeftOn = "helper.mcp-left-on", Boundary,
        "What a Codex helper keeps although spawned without connectors: Codex has no switch to turn off for one run the servers of the project's .codex/config.toml, which the run trusts, or those a plugin brings.";
    HelperNoMemory = "helper.no-auto-memory", Boundary,
        "What a helper saved to the account's auto-memory would reach every later session of the account unseen: its auto-memory is off.";
    HelperTrust = "helper.trust", Runtime,
        "A helper has nobody to answer Codex's trust screen: the folder it was sent to is trusted for this run only, nothing written.";
    SkipGitRepoCheck = "helper.skip-git-repo-check", Runtime,
        "The folder is not a Git repository, where codex exec refuses to run without this.";
    ReadOnly = "read-only", UserAction,
        "The agent that spawned it asked for it without the edit tools.";
    Agent = "agent", UserAction,
        "The agent that spawned it asked for it to run as this agent.";
    TrustedRoot = "trusted-root", UserAction,
        "Settings › Trusted roots: the folder is under one, trusted for this run, nothing written.";
    SessionTrust = "session-trust", UserAction,
        "You asked for this folder to be trusted while a Codex session held the account's config.toml: trusted for runs of this Luna instead.";
    HooksUnasked = "hooks-unasked", UserAction,
        "The Hooks unasked box: the project's hooks run without Codex asking to trust them.";
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
    TrustWritten = "trust", UserAction,
        "You ticked Trust this folder in the new-chat dialog: Luna wrote the same mark the CLI's own trust prompt writes.";
    LibraryLayout = "library.layout", UserAction,
        "The library lays its skills and instructions out in this account's folder, as set in the Library window.";
}

/// One thing Luna applied: what, its class, the value as it went (secrets
/// left out), and why.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
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

/// A chat's "What Luna applied": its latest start, and what Luna wrote into
/// its account that concerns it.
#[derive(Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct AppliedView {
    /// The latest start of its CLI (a helper's latest turn); empty if it has
    /// not started under this core.
    pub launch: Vec<Applied>,
    /// Writes into its account outside a launch.
    pub account: Vec<Applied>,
    /// Why `account` may be missing writes Luna made: its ledger would not
    /// read, or a write was not noted in it. None when it is whole.
    pub incomplete: Option<String>,
    /// For a chat with auto-handoff on: the instructions it would be served
    /// to write its handoff by, as of now, and the skills of the same name
    /// its CLI may list (agents/handoff `applied_of`).
    pub handoff: Option<crate::agents::handoff::HandoffApplied>,
}

/// The window's "What Luna applied" for `chat_id`.
pub fn applied_by_luna(chat_id: String) -> AppliedView {
    let handoff = crate::agents::handoff::applied_of(&chat_id);
    let Some((launch, account_path, folder)) = crate::agents::applied_of(&chat_id) else {
        return AppliedView { handoff, ..AppliedView::default() };
    };
    let (recorded, incomplete) = match recorded(&account_path) {
        Ok(r) => (r, None),
        Err(e) => (Vec::new(), Some(e)),
    };
    let said = [incomplete, missed(), unnoted_said()].into_iter().flatten().collect::<Vec<_>>();
    let incomplete = (!said.is_empty()).then(|| said.join("\n"));
    let mut account: Vec<Applied> = recorded
        .into_iter()
        .filter(|a| a.key != Item::TrustWritten.key() || crate::paths::path_key(&a.value) == crate::paths::path_key(&folder))
        .collect();
    let laid = crate::library::laid_out_in(&account_path);
    if !laid.is_empty() {
        account.push(Item::LibraryLayout.with(laid.join("\n")));
    }
    AppliedView { launch, account, incomplete, handoff }
}

/// A write into an account outside a launch, as the ledger keeps it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Written {
    account: String,
    key: String,
    value: String,
}

#[derive(Serialize, Deserialize, Default)]
struct Ledger {
    written: Vec<Written>,
}

fn ledger_path() -> std::path::PathBuf {
    #[cfg(test)]
    if let Some(dir) = LEDGER_AT.with(|l| l.borrow().clone()) {
        return dir.join("applied-ledger.json");
    }
    crate::paths::data_dir().join("applied-ledger.json")
}

#[cfg(test)]
thread_local! {
    pub(crate) static LEDGER_AT: std::cell::RefCell<Option<std::path::PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// The ledger; an empty one where there is none yet. Err if it is there and
/// will not read or parse: it is then no inventory, and is not written over.
fn load() -> Result<Ledger, String> {
    let path = ledger_path();
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| format!("{} does not parse: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Ledger::default()),
        Err(e) => Err(format!("{} does not read: {e}", path.display())),
    }
}

/// Where a write the ledger missed is said, beside it: kept until someone
/// clears it by hand, as the inventory stays short of that write.
fn missed_path() -> std::path::PathBuf {
    ledger_path().with_file_name("applied-ledger.incomplete")
}

/// What `missed_path` says, if it is there.
fn missed() -> Option<String> {
    let text = std::fs::read_to_string(missed_path()).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// Writes this process could not note, by the ledger they belong in, each
/// with why: kept until a write of the ledger takes them in, so that a ledger
/// and a `missed_path` that both cannot be written still leave the inventory
/// called incomplete. Every ledger write is the core's (the trust commands
/// are core commands), the process that answers `applied_by_luna`.
fn unnoted() -> &'static std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, Vec<(Written, String)>>> {
    static U: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, Vec<(Written, String)>>>> =
        std::sync::OnceLock::new();
    U.get_or_init(Default::default)
}

/// Why `unnoted` holds what it holds for this ledger, if anything.
fn unnoted_said() -> Option<String> {
    let all = unnoted().lock().unwrap_or_else(|e| e.into_inner());
    let lines: Vec<&str> = all.get(&ledger_path())?.iter().map(|(_, why)| why.as_str()).collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// Notes a write Luna made into `account_path`. Called under the account
/// settings' lock (account_settings `editing`), which the window and the core
/// both take before writing an account's files: the same lock keeps their
/// ledger writes apart. Once per account, item and value. The account write
/// it notes has been made already, so a failure here does not undo it: the
/// write is kept in `unnoted`, for the next write of the ledger to take in,
/// and said beside the ledger (`missed_path`), and the window's "What Luna
/// applied" calls the inventory incomplete rather than show it as whole.
pub fn record(_editing: &crate::account_settings::Editing, account_path: &str, item: Item, value: &str) {
    let entry = Written { account: account_path.to_string(), key: item.key().to_string(), value: value.to_string() };
    let path = ledger_path();
    let waiting: Vec<Written> = unnoted()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&path)
        .map(|w| w.iter().map(|(e, _)| e.clone()).collect())
        .unwrap_or_default();
    let saved = load().and_then(|mut ledger| {
        let before = ledger.written.len();
        for w in waiting.iter().chain([&entry]) {
            if !ledger.written.contains(w) {
                ledger.written.push(w.clone());
            }
        }
        if ledger.written.len() == before {
            return Ok(());
        }
        let text = serde_json::to_string_pretty(&ledger).map_err(|e| e.to_string())?;
        crate::secret::write_private(&path, text.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))
    });
    let mut all = unnoted().lock().unwrap_or_else(|e| e.into_inner());
    let Err(e) = saved else {
        all.remove(&path);
        return;
    };
    let line = format!("{} {value} in {account_path} was not noted: {e}", item.key());
    crate::log::warn("applied", &line);
    let held = all.entry(path).or_default();
    if !held.iter().any(|(w, _)| *w == entry) {
        held.push((entry, line.clone()));
    }
    drop(all);
    let said = missed().map_or(line.clone(), |before| format!("{before}\n{line}"));
    if let Err(e) = crate::secret::write_private(&missed_path(), said.as_bytes()) {
        crate::log::error("applied", &format!("could not say so in {} either: {e}", missed_path().display()));
    }
}

/// What the ledger has of `account_path`; Err if it will not read.
fn recorded(account_path: &str) -> Result<Vec<Applied>, String> {
    let key = crate::paths::path_key(account_path);
    Ok(load()?
        .written
        .into_iter()
        .filter(|w| crate::paths::path_key(&w.account) == key)
        .filter_map(|w| Some(Item::by_key(&w.key)?.with(w.value)))
        .collect())
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
            assert_eq!(Item::by_key(item.key()), Some(*item));
            let a = item.with("v");
            assert_eq!((a.key, a.class, a.why), (item.key(), item.class(), item.why()));
        }
    }

    #[test]
    fn the_ledger_keeps_each_write_once_and_by_account() {
        let dir = std::env::temp_dir().join(format!("luna-applied-ledger-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        LEDGER_AT.with(|l| *l.borrow_mut() = Some(dir.clone()));
        let editing = crate::account_settings::editing().unwrap();
        record(&editing, "/acc/a", Item::TrustWritten, "/p/one");
        record(&editing, "/acc/a", Item::TrustWritten, "/p/one");
        record(&editing, "/acc/b", Item::TrustWritten, "/p/two");
        drop(editing);
        assert_eq!(recorded("/acc/a").unwrap(), [Item::TrustWritten.with("/p/one")]);
        assert_eq!(recorded("/acc/b").unwrap(), [Item::TrustWritten.with("/p/two")]);
        assert!(recorded("/acc/c").unwrap().is_empty());
        LEDGER_AT.with(|l| *l.borrow_mut() = None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_chats_view_is_its_latest_start_and_its_folders_trust() {
        let dir = std::env::temp_dir().join(format!("luna-applied-view-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        LEDGER_AT.with(|l| *l.borrow_mut() = Some(dir.clone()));
        let editing = crate::account_settings::editing().unwrap();
        record(&editing, "/acc/view", Item::TrustWritten, "/p/mine");
        record(&editing, "/acc/view", Item::TrustWritten, "/p/other");
        drop(editing);
        assert!(applied_by_luna("applied-view-chat".into()).launch.is_empty(), "not started: nothing");
        crate::agents::set_applied("applied-view-chat", vec![Item::Model.with("opus")], "/acc/view", "/p/mine");
        let view = applied_by_luna("applied-view-chat".into());
        assert_eq!(view.launch, [Item::Model.with("opus")]);
        assert_eq!(view.account, [Item::TrustWritten.with("/p/mine")], "the trust of its own folder, not the account's others");
        crate::agents::set_applied("applied-view-chat", vec![Item::Model.with("sonnet")], "/acc/view", "/p/mine");
        assert_eq!(applied_by_luna("applied-view-chat".into()).launch, [Item::Model.with("sonnet")], "the latest start replaces it");
        crate::agents::forget("applied-view-chat");
        assert!(applied_by_luna("applied-view-chat".into()).launch.is_empty(), "a chat deleted keeps nothing");
        LEDGER_AT.with(|l| *l.borrow_mut() = None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_ledger_that_will_not_read_or_a_write_it_missed_makes_the_inventory_incomplete() {
        let dir = std::env::temp_dir().join(format!("luna-applied-incomplete-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        LEDGER_AT.with(|l| *l.borrow_mut() = Some(dir.clone()));
        crate::agents::set_applied("applied-incomplete-chat", vec![Item::Model.with("opus")], "/acc/inc", "/p/inc");
        assert_eq!(applied_by_luna("applied-incomplete-chat".into()).incomplete, None, "no ledger yet: whole");
        // Corrupt: read as no inventory, and not written over.
        std::fs::write(dir.join("applied-ledger.json"), "{ not json").unwrap();
        let view = applied_by_luna("applied-incomplete-chat".into());
        assert!(view.incomplete.as_deref().is_some_and(|w| w.contains("does not parse")), "{:?}", view.incomplete);
        assert_eq!(view.launch, [Item::Model.with("opus")], "the start is still shown");
        let editing = crate::account_settings::editing().unwrap();
        record(&editing, "/acc/inc", Item::TrustWritten, "/p/inc");
        assert_eq!(std::fs::read_to_string(dir.join("applied-ledger.json")).unwrap(), "{ not json", "not written over");
        // A write it could not note is said beside it, and stays said.
        let _ = std::fs::remove_file(dir.join("applied-ledger.json"));
        std::fs::create_dir_all(dir.join("applied-ledger.json")).unwrap();
        record(&editing, "/acc/inc", Item::TrustWritten, "/p/inc");
        drop(editing);
        std::fs::remove_dir_all(dir.join("applied-ledger.json")).unwrap();
        let view = applied_by_luna("applied-incomplete-chat".into());
        assert!(view.incomplete.as_deref().is_some_and(|w| w.contains("trust /p/inc in /acc/inc was not noted")), "{:?}", view.incomplete);
        crate::agents::forget("applied-incomplete-chat");
        LEDGER_AT.with(|l| *l.borrow_mut() = None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_write_neither_the_ledger_nor_its_side_note_took_still_makes_it_incomplete_until_noted() {
        let dir = std::env::temp_dir().join(format!("luna-applied-unnoted-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        LEDGER_AT.with(|l| *l.borrow_mut() = Some(dir.clone()));
        crate::agents::set_applied("applied-unnoted-chat", vec![Item::Model.with("opus")], "/acc/un", "/p/un");
        // Both the ledger and the note beside it are folders: neither writes.
        std::fs::create_dir_all(dir.join("applied-ledger.json")).unwrap();
        std::fs::create_dir_all(dir.join("applied-ledger.incomplete")).unwrap();
        let editing = crate::account_settings::editing().unwrap();
        record(&editing, "/acc/un", Item::TrustWritten, "/p/un");
        let view = applied_by_luna("applied-unnoted-chat".into());
        assert!(view.incomplete.as_deref().is_some_and(|w| w.contains("trust /p/un in /acc/un was not noted")), "{:?}", view.incomplete);
        // The ledger writable again: the next write takes the missed one in.
        std::fs::remove_dir_all(dir.join("applied-ledger.json")).unwrap();
        std::fs::remove_dir_all(dir.join("applied-ledger.incomplete")).unwrap();
        record(&editing, "/acc/un", Item::TrustWritten, "/p/other");
        drop(editing);
        assert_eq!(recorded("/acc/un").unwrap(), [Item::TrustWritten.with("/p/un"), Item::TrustWritten.with("/p/other")]);
        assert_eq!(applied_by_luna("applied-unnoted-chat".into()).incomplete, None, "noted now: whole again");
        crate::agents::forget("applied-unnoted-chat");
        LEDGER_AT.with(|l| *l.borrow_mut() = None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_trust_luna_writes_is_noted_and_one_it_does_not_is_not() {
        let dir = std::env::temp_dir().join(format!("luna-applied-trust-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let account = dir.join("acc");
        std::fs::create_dir_all(&account).unwrap();
        LEDGER_AT.with(|l| *l.borrow_mut() = Some(dir.clone()));
        let acc = account.to_string_lossy().into_owned();
        crate::claude::trust::trust_folder(&acc, "/p/claude").unwrap();
        crate::codex::trust::codex_trust_folder(acc.clone(), "/p/codex".into()).unwrap();
        assert_eq!(recorded(&acc).unwrap(), [Item::TrustWritten.with("/p/claude"), Item::TrustWritten.with("/p/codex")]);
        // Trusted already: nothing written, nothing noted.
        crate::codex::trust::codex_trust_folder(acc.clone(), "/p/codex".into()).unwrap();
        assert_eq!(recorded(&acc).unwrap().len(), 2);
        LEDGER_AT.with(|l| *l.borrow_mut() = None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
