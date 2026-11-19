//! Making a chat, in one place: the new-chat dialog, the quick chat and a
//! helper opened as a chat all come through here, so they cannot drift on
//! trust, naming or placement (src/features/new-chat/create.ts). A folder the
//! chat could not start in is refused before anything is written for it;
//! Luna creates no worktree, the CLI makes its own from the flag the chat
//! keeps.

use super::choice::{ClaudeSettings, CodexSettings, Settings, same_path};
use super::cmd;
use crate::host::Host;
use crate::models::Models;
use crate::panels::ask;
use crate::state::{Chat, Folder, Placed, Status};
use crate::term::paste::Provider;
use crate::workspace::{Account, Workspace, new_id};
use gpui::{App, AsyncApp, Context};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

/// `chat_folder_check`'s answer: what keeps a chat from starting in a folder,
/// and what may still make it fail where Luna cannot tell.
#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct FolderCheck {
    pub refusal: Option<String>,
    pub warning: Option<String>,
}

/// The provider as the host's commands name it.
pub fn key(p: Provider) -> &'static str {
    match p {
        Provider::Claude => "claude",
        Provider::Codex => "codex",
    }
}

pub fn trusted_cmd(p: Provider) -> &'static str {
    match p {
        Provider::Claude => cmd::CLAUDE_FOLDER_TRUSTED,
        Provider::Codex => cmd::CODEX_FOLDER_TRUSTED,
    }
}

pub fn trust_cmd(p: Provider) -> &'static str {
    match p {
        Provider::Claude => cmd::CLAUDE_TRUST_FOLDER,
        Provider::Codex => cmd::CODEX_TRUST_FOLDER,
    }
}

/// Whether the account has accepted the CLI's trust prompt for the folder;
/// true when it cannot be told.
pub async fn folder_trusted(host: &Arc<dyn Host>, p: Provider, account: &str, folder: &str) -> Result<bool, String> {
    ask::<bool>(host, trusted_cmd(p), json!({ "accountPath": account, "folder": folder })).await
}

/// What a chat is made from.
#[derive(Clone, Debug)]
pub struct Make {
    pub folder: String,
    pub account: Account,
    pub settings: Settings,
    pub worktree: bool,
    /// Luna's MCP tools attached.
    pub tools: bool,
    /// Mark the folder trusted in the account, as the CLI's own prompt
    /// would: ticked in the dialog, never for a helper opened as a chat.
    pub trust: bool,
    pub name: Option<String>,
    /// A helper opened as a chat keeps the id its transcript and Luna's log
    /// know it by.
    pub id: Option<String>,
    /// A helper opened as a chat: never remembered as the person's last
    /// choice, and seated by the caller once the core has let go of it.
    pub from_helper: bool,
    /// An existing worktree of `folder` to run in: the helper's caller's.
    pub in_worktree: Option<String>,
    pub read_only: bool,
    pub agent: Option<String>,
    pub session_id: Option<String>,
    /// The guard against endless waits.
    pub guard: bool,
}

impl Make {
    pub fn new(folder: &str, account: Account, settings: Settings) -> Make {
        Make {
            folder: folder.to_owned(),
            account,
            settings,
            worktree: false,
            tools: false,
            trust: false,
            name: None,
            id: None,
            from_helper: false,
            in_worktree: None,
            read_only: false,
            agent: None,
            session_id: None,
            guard: false,
        }
    }
}

/// What the start would refuse, and the trust the person ticked, written
/// before the session spawns. Nothing is written for a refused folder.
async fn prepare(host: &Arc<dyn Host>, make: &Make) -> Result<(), String> {
    let (p, account) = (make.account.provider, make.account.path.as_str());
    if !make.from_helper && !account.is_empty() {
        let worktree = make.worktree && make.in_worktree.is_none();
        let check: FolderCheck = ask(
            host,
            cmd::CHAT_FOLDER_CHECK,
            json!({ "provider": key(p), "accountPath": account, "folder": make.folder, "worktree": worktree }),
        )
        .await?;
        if let Some(why) = check.refusal {
            return Err(why);
        }
    }
    if make.trust && !make.from_helper && !account.is_empty() && !folder_trusted(host, p, account, &make.folder).await? {
        ask::<serde_json::Value>(host, trust_cmd(p), json!({ "accountPath": account, "folder": make.folder })).await?;
    }
    Ok(())
}

/// The chat `make` describes, named `chat N` unless it has a name.
fn build(make: &Make, ws: &Workspace) -> Chat {
    let board = ws.state.board;
    let n = ws.state.chats().filter(|c| c.board == board).count() + 1;
    let name = make.name.as_deref().map(str::trim).filter(|n| !n.is_empty());
    let mut chat = Chat {
        id: make.id.clone().unwrap_or_else(|| new_id("c", n)),
        name: name.map_or_else(|| format!("chat {n}"), str::to_owned),
        name_source: name.map(|_| crate::state::NameSource::User),
        status: Status::Resting,
        provider: make.account.provider,
        account: make.account.name.clone(),
        board,
        worktree: make.worktree,
        worktree_path: make.in_worktree.clone(),
        session_id: make.session_id.clone(),
        tools: make.tools,
        guard: make.guard,
        model: None,
        effort: String::new(),
        perm: None,
        approval: None,
        sandbox: None,
        read_only: make.read_only,
        agent: make.agent.clone(),
        model_seen: None,
    };
    match &make.settings {
        Settings::Claude(ClaudeSettings { model, effort, perm }) => {
            chat.model = Some(model.clone());
            chat.effort = effort.clone();
            chat.perm = Some(perm.clone());
        }
        Settings::Codex(CodexSettings { model, effort, approval, sandbox }) => {
            chat.model = model.clone();
            chat.effort = effort.clone();
            chat.approval = Some(approval.clone());
            chat.sandbox = Some(sandbox.clone());
        }
    }
    chat
}

/// Makes the chat: the checks first, then listed and, for a person's own,
/// seated where it will be seen, and the folder and account remembered. A
/// helper opened as a chat is listed only (`Workspace::place_chat` seats it
/// once the core has let go). Errs with nothing written for a folder its
/// start would refuse.
pub async fn create(models: &Models, make: Make, cx: &mut AsyncApp) -> Result<String, String> {
    prepare(&models.host, &make).await?;
    let ws = models.workspace.clone();
    Ok(cx.update(|cx| {
        ws.update(cx, |w, cx| {
            let chat = build(&make, w);
            let id = chat.id.clone();
            if make.from_helper {
                w.list_chat(&make.folder, chat, cx);
            } else {
                w.add_chat(&make.folder, chat, true, cx);
                w.remember_start(&make.folder, &make.account, cx);
            }
            id
        })
    }))
}

impl Workspace {
    /// A chat listed but not seated: a helper opened as a chat, until the
    /// core has let go of it.
    pub fn list_chat(&mut self, folder: &str, chat: Chat, cx: &mut Context<Self>) {
        let n = self.state.folders.len();
        self.state.add_chat(folder, chat, false, || new_id("f", n));
        self.changed(cx);
    }

    /// Seats a chat listed before, where it will be seen.
    pub fn place_chat(&mut self, id: &str, cx: &mut Context<Self>) -> Placed {
        let placed = self.state.auto_place(id);
        self.focus_request = Some(id.to_owned());
        self.changed(cx);
        placed
    }

    /// Where the last chat was made: folder and account have no settings
    /// file to come from, so they are remembered instead of asked again.
    pub fn remember_start(&mut self, folder: &str, account: &Account, cx: &mut Context<Self>) {
        self.state.new_chat.last_folder = Some(folder.to_owned());
        self.state.new_chat.last_account = Some(crate::state::AccountRef { provider: account.provider, name: account.name.clone() });
        self.changed(cx);
    }

    /// A folder looked for is one that will be looked for again: offered by
    /// the dialog from now on.
    pub fn remember_folder(&mut self, path: &str, cx: &mut Context<Self>) {
        match self.state.folders.iter_mut().find(|f| same_path(&f.path, path)) {
            Some(f) => f.picked = true,
            None => {
                let n = self.state.folders.len();
                self.state.folders.push(Folder {
                    id: new_id("f", n),
                    path: path.to_owned(),
                    open: true,
                    chats: Vec::new(),
                    picked: true,
                    start: None,
                });
            }
        }
        self.changed(cx);
    }

    /// Takes a folder out of the dialog's list; one that still holds chats
    /// stays (delete those first). True when it went.
    pub fn forget_folder(&mut self, path: &str, cx: &mut Context<Self>) -> bool {
        let Some(at) = self.state.folders.iter().position(|f| f.path == path && f.chats.is_empty()) else { return false };
        self.state.folders.remove(at);
        self.changed(cx);
        true
    }

    /// How the dialog's boxes start in `folder`: its own start where one was
    /// kept, else the defaults from Settings. Nothing a chat is made with
    /// changes either.
    pub fn start_in(&self, folder: &str) -> (bool, bool) {
        match self.state.folders.iter().find(|f| f.path == folder).and_then(|f| f.start.as_ref()) {
            Some(s) => (s.worktree, s.tools),
            None => {
                let d = self.new_chat_start();
                (d.worktree, d.tools)
            }
        }
    }

    /// The chat on screen, if any: the one a new chat should take after.
    pub fn working_on(&self) -> Option<&Chat> {
        let s = &self.state;
        let id = s.peeked_chat().or_else(|| s.seat(s.active_pane)).or(s.active.as_deref())?;
        s.find(id)
    }

    /// The account of `provider` and `name`.
    pub fn account_named(&self, provider: Provider, name: &str) -> Option<Account> {
        self.accounts()?.iter().find(|a| a.provider == provider && a.name == name).cloned()
    }

    /// The folders the dialog offers: those a person launched in, and
    /// `also`, where it was opened.
    pub fn offered_folders(&self, also: Option<&str>) -> Vec<String> {
        self.state.folders.iter().filter(|f| f.picked || also == Some(f.path.as_str())).map(|f| f.path.clone()).collect()
    }
}

/// Quick chat (Ctrl+Shift+N): everything is already decided, the folder and
/// account of the chat on screen and the rest from the settings files that
/// would govern it anyway. Falls back to the dialog when there is nothing to
/// go on, when the folder is not trusted in the account (whether to trust it
/// is the person's to say, and the dialog is where it is asked), or when
/// creating fails and the reason wants showing.
pub fn quick_chat(models: &Models, window: &mut gpui::Window, cx: &mut App) {
    let handle = window.window_handle();
    let ws = models.workspace.read(cx);
    let near = ws.working_on().cloned();
    let (folder, account) = {
        let s = &ws.state;
        let near_folder = near.as_ref().and_then(|c| s.folder_of(&c.id)).map(|f| f.path.clone());
        let folder = near_folder
            .or_else(|| s.new_chat.last_folder.clone())
            .or_else(|| s.folders.iter().find(|f| f.picked && !f.chats.is_empty()).map(|f| f.path.clone()))
            .or_else(|| s.folders.iter().find(|f| f.picked).map(|f| f.path.clone()))
            .unwrap_or_default();
        let account = near
            .as_ref()
            .and_then(|c| ws.account_named(c.provider, &c.account))
            .or_else(|| s.new_chat.last_account.as_ref().and_then(|a| ws.account_named(a.provider, &a.name)))
            .or_else(|| ws.accounts().and_then(|a| a.first().cloned()));
        (folder, account)
    };
    let (worktree, tools) = ws.start_in(&folder);
    let models = models.clone();
    let Some(account) = account.filter(|_| !folder.is_empty()) else {
        super::open_new_chat((!folder.is_empty()).then_some(folder), window, cx);
        return;
    };
    cx.spawn(async move |cx| {
        let made = quick(&models, &folder, account, worktree, tools, cx).await;
        if let Err(why) = made {
            log::info!("quick chat: the dialog takes it from here: {why}");
            let _ = handle.update(cx, |_, window, cx| super::open_new_chat(Some(folder), window, cx));
        }
    })
    .detach();
}

async fn quick(models: &Models, folder: &str, account: Account, worktree: bool, tools: bool, cx: &mut AsyncApp) -> Result<String, String> {
    let host = &models.host;
    if !folder_trusted(host, account.provider, &account.path, folder).await.unwrap_or(false) {
        return Err("the folder is not trusted in the account".into());
    }
    let settings = super::new_chat::defaults_of(host, &account, folder).await;
    let mut make = Make::new(folder, account, settings);
    make.worktree = worktree;
    make.tools = tools;
    create(models, make, cx).await
}
