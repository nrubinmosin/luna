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
    /// What keeps a session out of Luna's own secrets.
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
    NotesDir = "notes-dir", Runtime,
        "Long prompts, messages and helpers' answers from Luna arrive as files there, readable without asking.";
    HelperIdentity = "helper.identity", Runtime,
        "Tells a helper it answers to the chat that started it, not to a person.";
    HelperHeadless = "helper.headless", Runtime,
        "A helper runs without a terminal: its CLI's headless mode, its output read as a stream.";
    HelperPermissionPrompt = "helper.permission-prompt", Runtime,
        "A helper's permission prompts go to the chat that started it, through Luna's tool.";
    HelperApprovalNever = "helper.approval", Runtime,
        "codex exec has nobody to ask: the sandbox chosen for it is what holds it.";
    HelperReviewer = "helper.reviewer", Runtime,
        "What a helper would ask goes to the chat that started it, never to a reviewer model an account or project names.";
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
        "The library keeps these skills from sessions in this folder.";
    LibraryAccountInstructions = "library.account-instructions-off", UserAction,
        "The library gives this folder other instructions than the account's own.";
    LibraryInstructions = "library.instructions", UserAction,
        "The library's instructions for this folder, read as memory from a folder of Luna's.";
    MemoryNarrowed = "library.memory-narrowed", Runtime,
        "Reading the library's instructions as memory would read the other folders added to the session too: their CLAUDE.md files are left out.";
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
}

/// The window's "What Luna applied" for `chat_id`.
pub fn applied_by_luna(chat_id: String) -> AppliedView {
    let Some((launch, account_path, folder)) = crate::agents::applied_of(&chat_id) else {
        return AppliedView::default();
    };
    let mut account: Vec<Applied> = recorded(&account_path)
        .into_iter()
        .filter(|a| a.key != Item::TrustWritten.key() || crate::paths::path_key(&a.value) == crate::paths::path_key(&folder))
        .collect();
    let laid = crate::library::laid_out_in(&account_path);
    if !laid.is_empty() {
        account.push(Item::LibraryLayout.with(laid.join("\n")));
    }
    AppliedView { launch, account }
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

fn load() -> Ledger {
    std::fs::read_to_string(ledger_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// Notes a write Luna made into `account_path`. Called under the account
/// settings' lock (account_settings `editing`), which the window and the core
/// both take before writing an account's files: the same lock keeps their
/// ledger writes apart. Once per account, item and value.
pub fn record(_editing: &crate::account_settings::Editing, account_path: &str, item: Item, value: &str) {
    let mut ledger = load();
    let entry = Written { account: account_path.to_string(), key: item.key().to_string(), value: value.to_string() };
    if ledger.written.contains(&entry) {
        return;
    }
    ledger.written.push(entry);
    let path = ledger_path();
    let saved = serde_json::to_string_pretty(&ledger)
        .map_err(|e| e.to_string())
        .and_then(|text| crate::secret::write_private(&path, text.as_bytes()).map_err(|e| e.to_string()));
    if let Err(e) = saved {
        crate::log::warn("applied", &format!("could not note {} in {}: {e}", item.key(), path.display()));
    }
}

/// What the ledger has of `account_path`.
fn recorded(account_path: &str) -> Vec<Applied> {
    let key = crate::paths::path_key(account_path);
    load()
        .written
        .into_iter()
        .filter(|w| crate::paths::path_key(&w.account) == key)
        .filter_map(|w| Some(Item::by_key(&w.key)?.with(w.value)))
        .collect()
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
        assert_eq!(recorded("/acc/a"), [Item::TrustWritten.with("/p/one")]);
        assert_eq!(recorded("/acc/b"), [Item::TrustWritten.with("/p/two")]);
        assert!(recorded("/acc/c").is_empty());
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
    fn a_trust_luna_writes_is_noted_and_one_it_does_not_is_not() {
        let dir = std::env::temp_dir().join(format!("luna-applied-trust-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let account = dir.join("acc");
        std::fs::create_dir_all(&account).unwrap();
        LEDGER_AT.with(|l| *l.borrow_mut() = Some(dir.clone()));
        let acc = account.to_string_lossy().into_owned();
        crate::claude::trust::trust_folder(&acc, "/p/claude").unwrap();
        crate::codex::trust::codex_trust_folder(acc.clone(), "/p/codex".into()).unwrap();
        assert_eq!(recorded(&acc), [Item::TrustWritten.with("/p/claude"), Item::TrustWritten.with("/p/codex")]);
        // Trusted already: nothing written, nothing noted.
        crate::codex::trust::codex_trust_folder(acc.clone(), "/p/codex".into()).unwrap();
        assert_eq!(recorded(&acc).len(), 2);
        LEDGER_AT.with(|l| *l.borrow_mut() = None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
