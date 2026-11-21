//! The new-chat dialog (src/features/new-chat/NewChatDialog.tsx): folder,
//! account (which says the CLI), model, effort, the CLI's modes, Luna's tools,
//! trust, and a worktree where the folder is in git. Everything opens on what
//! the CLI's own settings files say for the account in that folder, asked of
//! the core as the page asked (`claude_defaults`, `codex_defaults`); a value
//! changed here is for this chat only. Create goes through create.rs, the
//! path the quick chat takes too.

use super::choice::{
    self, AUTO_REVIEW_SANDBOX, CODEX_APPROVALS, CODEX_EFFORTS, CODEX_SANDBOXES, ClaudeDefaults, ClaudeModel, ClaudeSettings, CodexDefaults,
    CodexModel, CodexSettings, Origins, PERMS, Setting, Settings, codex_mode_label, effort_from_setting, model_label_of, next_effort,
};
use super::create::{self, FolderCheck, Make, key};
use super::{DialogLayer, cmd};
use crate::desktop::{Pick, PickKind};
use crate::host::Host;
use crate::models::Models;
use crate::panels::ask;
use crate::panels::ui::{self, DropItem, Dropdown, OnClick, Tone};
use crate::scale::u;
use crate::selector::Selector;
use crate::state::FolderStart;
use crate::term::paste::Provider;
use crate::theme::Palette;
use crate::workspace::{Account, provider_label};
use gpui::{
    AnyElement, Context, Div, InteractiveElement, IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, WeakEntity, Window, div,
};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

/// The settings a new chat will open on, for one CLI: what the files say
/// (`resolved`, with `origins` naming the file per field) and what the dialog
/// shows now (`settings`), the same until a control is touched. A touched
/// control is the person's: resolving again after a change of account or
/// folder never takes it back.
#[derive(Clone, Debug)]
pub struct Draft<S> {
    pub settings: S,
    pub resolved: S,
    pub origins: Origins,
    pub touched: HashSet<&'static str>,
    /// The account and folder the files' answer is for. Until it is in,
    /// `settings` holds the stock values, which may go further than the files
    /// say (Bypass, no sandbox), so no chat is made from them.
    pub ready_for: Option<String>,
}

impl<S: Clone> Draft<S> {
    fn new(stock: S) -> Draft<S> {
        Draft { settings: stock.clone(), resolved: stock, origins: Origins::new(), touched: HashSet::new(), ready_for: None }
    }
}

/// What the files said for an account in a folder, and the account's models.
struct Resolved {
    claude: Option<(ClaudeSettings, Origins, HashMap<String, Setting>)>,
    codex: Option<(CodexSettings, Origins)>,
}

/// The settings `account` opens on in `folder`, as a start that cannot wait
/// on the CLI takes them: from the models as far as they are known, without
/// asking the CLI (the quick chat's, a helper's).
pub async fn defaults_of(host: &Arc<dyn Host>, account: &Account, folder: &str) -> Settings {
    let args = json!({ "accountPath": account.path, "folder": folder });
    match account.provider {
        Provider::Claude => {
            let dto = ask::<ClaudeDefaults>(host, cmd::CLAUDE_DEFAULTS, args).await.ok();
            let models = ask::<Vec<ClaudeModel>>(host, cmd::CLAUDE_MODELS, json!({ "accountPath": account.path, "ask": false }))
                .await
                .unwrap_or_default();
            Settings::Claude(choice::claude_settings(dto.as_ref(), &models).0)
        }
        Provider::Codex => {
            let dto = ask::<CodexDefaults>(host, cmd::CODEX_DEFAULTS, args).await.ok();
            Settings::Codex(choice::codex_settings(dto.as_ref()).0)
        }
    }
}

/// A folder's last two parts, "parent / leaf".
pub fn tail2(path: &str) -> (String, String) {
    let parts: Vec<&str> = path.trim_end_matches(['/', '\\']).split(['/', '\\']).filter(|p| !p.is_empty()).collect();
    match parts.as_slice() {
        [] => (String::new(), path.to_owned()),
        [leaf] => (String::new(), (*leaf).to_owned()),
        [.., parent, leaf] => ((*parent).to_owned(), (*leaf).to_owned()),
    }
}

/// What a folder, an account and a worktree box were asked about.
#[derive(Clone, Debug, PartialEq)]
struct Check {
    refusal: Option<String>,
    /// A worktree is possible here: the folder is in git.
    worktree_ok: bool,
}

pub struct NewChat {
    models: Models,
    layer: WeakEntity<DialogLayer>,
    /// The folder the dialog was opened on, offered even if only an agent
    /// has been there.
    also: Option<String>,
    pub folder: String,
    pub account: Option<Account>,
    pub worktree: bool,
    pub tools: bool,
    pub claude: Draft<ClaudeSettings>,
    pub codex: Draft<CodexSettings>,
    claude_models: Option<Vec<ClaudeModel>>,
    codex_models: Option<Vec<CodexModel>>,
    models_for: String,
    by_model: HashMap<String, Setting>,
    /// Whether the account has trusted the folder, for the one key it was
    /// asked for; the tick belongs to one key too, so no trust is ever written
    /// for a folder nobody ticked it for.
    trust: Option<(String, bool)>,
    ticked: Option<String>,
    check: Option<(String, Check)>,
    pub creating: bool,
    pub error: Option<String>,
    /// The drop-down list open: "account" or "model".
    pub menu: Option<&'static str>,
    /// Why no chat can be made yet: the chat list has not come
    /// (`Workspace::list_wait`).
    pub list_wait: Option<String>,
    _subs: Vec<Subscription>,
}

impl NewChat {
    pub fn new(models: Models, layer: WeakEntity<DialogLayer>, folder: Option<String>, cx: &mut Context<Self>) -> NewChat {
        let subs =
            vec![cx.observe(&models.shell, |_, _, cx| cx.notify()), cx.observe(&models.workspace, |this, _, cx| this.accounts_came(cx))];
        let (start, last) = {
            let ws = models.workspace.read(cx);
            let first = ws.offered_folders(folder.as_deref()).into_iter().next();
            let start = folder.clone().or_else(|| ws.state.new_chat.last_folder.clone()).or(first).unwrap_or_default();
            (start, ws.state.new_chat.last_account.clone())
        };
        let (worktree, tools) = models.workspace.read(cx).start_in(&start);
        let account = {
            let ws = models.workspace.read(cx);
            last.and_then(|a| ws.account_named(a.provider, &a.name)).or_else(|| ws.accounts().and_then(|a| a.first().cloned()))
        };
        let mut dialog = NewChat {
            models,
            layer,
            also: folder,
            folder: start,
            account,
            worktree,
            tools,
            claude: Draft::new(choice::claude_stock()),
            codex: Draft::new(choice::codex_stock()),
            claude_models: None,
            codex_models: None,
            models_for: String::new(),
            by_model: HashMap::new(),
            trust: None,
            ticked: None,
            check: None,
            creating: false,
            error: None,
            menu: None,
            list_wait: None,
            _subs: subs,
        };
        dialog.list_wait = dialog.models.workspace.read(cx).list_wait();
        dialog.refresh(cx);
        dialog
    }

    fn act(&self, cx: &Context<Self>, f: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static) -> OnClick {
        let this = cx.weak_entity();
        Rc::new(move |window, cx| {
            let _ = this.update(cx, |v, cx| f(v, window, cx));
        })
    }

    /// The accounts or the chat list came after the dialog opened: the first
    /// account taken, and the last chat's folder, as a dialog opened with
    /// them would.
    fn accounts_came(&mut self, cx: &mut Context<Self>) {
        let wait = self.models.workspace.read(cx).list_wait();
        if self.list_wait.is_some() && wait.is_none() && self.folder.is_empty() {
            let ws = self.models.workspace.read(cx);
            let start = ws.state.new_chat.last_folder.clone().or_else(|| ws.offered_folders(self.also.as_deref()).into_iter().next());
            if let Some(start) = start {
                self.choose_folder(&start, cx);
            }
        }
        self.list_wait = wait;
        if self.account.is_none() {
            let ws = self.models.workspace.read(cx);
            let account = ws.accounts().and_then(|a| a.first().cloned());
            if account.is_some() {
                self.account = account;
                self.refresh(cx);
            }
        }
        cx.notify();
    }

    /// The accounts the dialog offers, in the list's order.
    pub(crate) fn accounts(&self, cx: &gpui::App) -> Vec<Account> {
        self.models.workspace.read(cx).accounts().map(<[Account]>::to_vec).unwrap_or_default()
    }

    fn provider(&self) -> Provider {
        self.account.as_ref().map_or(Provider::Claude, |a| a.provider)
    }

    /// What one account and folder are asked about: the draft, trust, the
    /// folder's check. A stale answer is let go of.
    fn key(&self) -> String {
        let a = self.account.as_ref();
        format!("{}\0{}\0{}", a.map_or("claude", |a| key(a.provider)), a.map_or("", |a| a.path.as_str()), self.folder)
    }

    fn draft_key(&self) -> String {
        format!("{}\0{}", self.account.as_ref().map_or("", |a| a.path.as_str()), self.folder)
    }

    /// Asks again for everything that depends on the account and the folder.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let (folder, Some(account)) = (self.folder.clone(), self.account.clone()) else {
            self.trust = Some((self.key(), true));
            self.check = Some((self.key(), Check { refusal: None, worktree_ok: false }));
            cx.notify();
            return;
        };
        let host = self.models.host.clone();
        let key = self.key();
        let draft = self.draft_key();
        let models_asked = self.models_for != account.path;
        if models_asked {
            self.models_for = account.path.clone();
            self.claude_models = None;
            self.codex_models = None;
        }
        // The settings the files give, and the account's models.
        {
            let (host, account, folder, draft) = (host.clone(), account.clone(), folder.clone(), draft.clone());
            cx.spawn(async move |this, cx| {
                let got = resolve(&host, &account, &folder).await;
                let _ = this.update(cx, |v, cx| v.resolved(draft, got, cx));
            })
            .detach();
        }
        if models_asked {
            let (host, account) = (host.clone(), account.clone());
            cx.spawn(async move |this, cx| {
                let (claude, codex) = fetch_models(&host, &account).await;
                let _ = this.update(cx, |v, cx| {
                    if v.models_for == account.path {
                        v.claude_models = claude.or(v.claude_models.take());
                        v.codex_models = codex.or(v.codex_models.take());
                        v.settle_claude();
                        cx.notify();
                    }
                });
            })
            .detach();
        }
        // Whether the account trusts the folder.
        if !folder.is_empty() {
            let (host, account, folder, key) = (host.clone(), account.clone(), folder.clone(), key.clone());
            cx.spawn(async move |this, cx| {
                let trusted = create::folder_trusted(&host, account.provider, &account.path, &folder).await.unwrap_or(true);
                let _ = this.update(cx, |v, cx| {
                    if v.key() == key {
                        v.trust = Some((key, trusted));
                        cx.notify();
                    }
                });
            })
            .detach();
        } else {
            self.trust = Some((key.clone(), true));
        }
        // What the chat would meet in the folder, said before Create; and
        // whether a worktree is possible there.
        if !folder.is_empty() {
            let (host, folder, key) = (host, folder, key);
            cx.spawn(async move |this, cx| {
                let asked = ask::<FolderCheck>(&host, cmd::CHAT_FOLDER_CHECK, json!({ "folder": folder, "worktree": false }));
                let check = match asked.await {
                    Ok(c) => Check { refusal: c.refusal, worktree_ok: c.in_git },
                    Err(e) => Check { refusal: Some(e), worktree_ok: false },
                };
                let _ = this.update(cx, |v, cx| {
                    if v.key() == key {
                        v.check = Some((key, check));
                        cx.notify();
                    }
                });
            })
            .detach();
        } else {
            self.check = Some((key, Check { refusal: None, worktree_ok: false }));
        }
        cx.notify();
    }

    /// The files' answer for `draft`'s account and folder is in.
    fn resolved(&mut self, draft: String, got: Resolved, cx: &mut Context<Self>) {
        if self.draft_key() == draft {
            if let Some((values, origins, by_model)) = got.claude {
                let d = &mut self.claude;
                let before = d.settings.clone();
                d.resolved = values.clone();
                d.origins = origins;
                d.settings = ClaudeSettings {
                    model: if d.touched.contains("model") { before.model } else { values.model },
                    effort: if d.touched.contains("effort") { before.effort } else { values.effort },
                    perm: if d.touched.contains("perm") { before.perm } else { values.perm },
                };
                d.ready_for = Some(draft.clone());
                self.by_model = by_model;
            }
            if let Some((values, origins)) = got.codex {
                let d = &mut self.codex;
                let before = d.settings.clone();
                d.resolved = values.clone();
                d.origins = origins;
                d.settings = CodexSettings {
                    model: if d.touched.contains("model") { before.model } else { values.model },
                    effort: if d.touched.contains("effort") { before.effort } else { values.effort },
                    approval: if d.touched.contains("approval") { before.approval } else { values.approval },
                    sandbox: if d.touched.contains("sandbox") { before.sandbox } else { values.sandbox },
                };
                d.ready_for = Some(draft);
            }
        }
        self.settle_claude();
        cx.notify();
    }

    fn claude_list(&self) -> Vec<ClaudeModel> {
        match &self.claude_models {
            Some(m) if !m.is_empty() => m.clone(),
            _ => choice::stock_models(),
        }
    }

    fn listed_claude(&self) -> Option<ClaudeModel> {
        self.claude_list().into_iter().find(|m| model_label_of(&m.slug) == self.claude.settings.model)
    }

    /// Claude Code keeps effort per model: a model picked here starts on the
    /// level the settings files give it unless one was picked by hand, and no
    /// chat runs at a level its model lacks.
    fn settle_claude(&mut self) {
        let listed = self.listed_claude();
        let own = listed.as_ref().and_then(|m| self.by_model.get(&m.slug)).cloned();
        let own_effort = own.as_ref().and_then(|o| effort_from_setting(&o.value));
        let touched = self.claude.touched.contains("effort");
        let next = next_effort(listed.as_ref(), &self.claude.settings.effort, own_effort.as_deref(), touched);
        if let (Some(own), Some(level)) = (&own, &own_effort) {
            let held = next_effort(listed.as_ref(), level, Some(level), false);
            self.claude.resolved.effort = held.clone();
            if held == *level {
                let said = choice::claude_source(&own.source);
                self.claude.origins.insert("effort", own.key.as_ref().map_or(said.clone(), |k| format!("{said}, {k}")));
            } else {
                self.claude.origins.insert("effort", choice::claude_source("model"));
            }
        }
        self.claude.settings.effort = next;
    }

    // ------------------------------------------------------------- choices

    /// Another folder: the boxes start the way its own start does, so a flip
    /// made for the first was not made for this one.
    pub fn choose_folder(&mut self, path: &str, cx: &mut Context<Self>) {
        self.folder = path.to_owned();
        let (worktree, tools) = self.models.workspace.read(cx).start_in(path);
        self.worktree = worktree;
        self.tools = tools;
        self.error = None;
        self.refresh(cx);
    }

    pub fn choose_account(&mut self, account: Account, cx: &mut Context<Self>) {
        self.menu = None;
        self.account = Some(account);
        self.error = None;
        self.refresh(cx);
    }

    pub fn pick_claude_model(&mut self, label: &str, cx: &mut Context<Self>) {
        self.menu = None;
        self.claude.settings.model = label.to_owned();
        self.claude.touched.insert("model");
        self.settle_claude();
        cx.notify();
    }

    pub fn pick_codex_model(&mut self, slug: Option<String>, cx: &mut Context<Self>) {
        self.menu = None;
        self.codex.settings.model = slug.clone();
        self.codex.touched.insert("model");
        // The effort follows the model to its own default level, unless a
        // settings file named one or it was picked by hand.
        let from_file = self.codex.origins.get("effort").is_some_and(|s| s != &choice::codex_source("codex"));
        if !from_file && !self.codex.touched.contains("effort") {
            let own = self.codex_models.iter().flatten().find(|m| Some(&m.slug) == slug.as_ref()).and_then(|m| m.default_effort.clone());
            if let Some(level) = own {
                self.codex.settings.effort = level;
            }
        }
        cx.notify();
    }

    pub fn pick_effort(&mut self, effort: &str, cx: &mut Context<Self>) {
        match self.provider() {
            Provider::Claude => {
                self.claude.settings.effort = effort.to_owned();
                self.claude.touched.insert("effort");
                self.settle_claude();
            }
            Provider::Codex => {
                self.codex.settings.effort = effort.to_owned();
                self.codex.touched.insert("effort");
            }
        }
        cx.notify();
    }

    pub fn pick_perm(&mut self, perm: &str, cx: &mut Context<Self>) {
        self.claude.settings.perm = perm.to_owned();
        self.claude.touched.insert("perm");
        cx.notify();
    }

    pub fn pick_approval(&mut self, approval: &str, cx: &mut Context<Self>) {
        self.codex.settings.approval = approval.to_owned();
        self.codex.touched.insert("approval");
        // `--approve-for-me` brings its own sandbox.
        if approval == "auto-review" {
            self.codex.settings.sandbox = AUTO_REVIEW_SANDBOX.to_owned();
            self.codex.touched.insert("sandbox");
        }
        cx.notify();
    }

    pub fn pick_sandbox(&mut self, sandbox: &str, cx: &mut Context<Self>) {
        if self.codex.settings.approval != "auto-review" {
            self.codex.settings.sandbox = sandbox.to_owned();
            self.codex.touched.insert("sandbox");
            cx.notify();
        }
    }

    /// "back to …" under a control: the value the files gave, no longer the
    /// person's. The modes go back together, as `auto-review` moves both.
    pub fn revert(&mut self, field: &'static str, cx: &mut Context<Self>) {
        match self.provider() {
            Provider::Claude => {
                let r = self.claude.resolved.clone();
                let d = &mut self.claude;
                match field {
                    "model" => d.settings.model = r.model,
                    "effort" => d.settings.effort = r.effort,
                    _ => d.settings.perm = r.perm,
                }
                d.touched.remove(field);
                self.settle_claude();
            }
            Provider::Codex => {
                let r = self.codex.resolved.clone();
                let d = &mut self.codex;
                match field {
                    "model" => d.settings.model = r.model,
                    "effort" => d.settings.effort = r.effort,
                    _ => {
                        d.settings.approval = r.approval;
                        d.settings.sandbox = r.sandbox;
                        d.touched.remove("sandbox");
                    }
                }
                d.touched.remove(field);
            }
        }
        cx.notify();
    }

    /// How the boxes start in this folder: its own start, else Settings'.
    fn folder_start(&self, cx: &gpui::App) -> (bool, bool) {
        self.models.workspace.read(cx).start_in(&self.folder)
    }

    /// The folder keeps a start of its own.
    fn kept_here(&self, cx: &gpui::App) -> bool {
        self.models.workspace.read(cx).state.folders.iter().any(|f| f.path == self.folder && f.start.is_some())
    }

    /// The boxes differ from how they start in this folder. A worktree box
    /// not shown (the folder is not in git) is no flip.
    fn flipped(&self, cx: &gpui::App) -> bool {
        let (worktree, tools) = self.folder_start(cx);
        self.tools != tools || (self.worktree_shown() && self.worktree != worktree)
    }

    /// "back to …" beside the boxes: as the folder starts.
    pub fn boxes_back(&mut self, cx: &mut Context<Self>) {
        (self.worktree, self.tools) = self.folder_start(cx);
        cx.notify();
    }

    /// "keep for this folder": the boxes as they are become the folder's
    /// start. A worktree box not shown keeps what the folder's start says of
    /// it (`Workspace::keep_start` drops a start that is Settings' anyway).
    pub fn keep_for_folder(&mut self, cx: &mut Context<Self>) {
        let worktree = if self.worktree_shown() { self.worktree } else { self.folder_start(cx).0 };
        let (folder, tools) = (self.folder.clone(), self.tools);
        self.models.workspace.update(cx, |w, cx| w.keep_start(&folder, FolderStart { worktree, tools }, cx));
        cx.notify();
    }

    pub fn tick_trust(&mut self, cx: &mut Context<Self>) {
        let key = self.key();
        self.ticked = if self.ticked.as_ref() == Some(&key) { None } else { Some(key) };
        cx.notify();
    }

    fn pick_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pick = Pick { kind: PickKind::Folder, title: Some("Folder for the new chat".to_owned()), start: None, filters: Vec::new() };
        let picked = self.models.host.pick(pick, Some(window));
        cx.spawn_in(window, async move |this, cx| match picked.await {
            Ok(Some(path)) => {
                let _ = this.update(cx, |v, cx| v.picked(&path.to_string_lossy(), cx));
            }
            Ok(None) => {}
            Err(e) => log::warn!("folder picker: {e}"),
        })
        .detach();
    }

    /// A folder looked for is one that will be looked for again, even if
    /// this dialog is closed now.
    pub fn picked(&mut self, path: &str, cx: &mut Context<Self>) {
        self.models.workspace.update(cx, |w, cx| w.remember_folder(path, cx));
        self.choose_folder(path, cx);
    }

    fn forget(&mut self, path: &str, cx: &mut Context<Self>) {
        let gone = self.models.workspace.update(cx, |w, cx| w.forget_folder(path, cx));
        if gone && self.folder == path {
            self.folder.clear();
            self.refresh(cx);
        }
        cx.notify();
    }

    // -------------------------------------------------------------- state

    fn trusted(&self) -> Option<bool> {
        self.trust.as_ref().filter(|(k, _)| *k == self.key()).map(|(_, t)| *t)
    }

    fn trust_it(&self) -> bool {
        self.trusted() == Some(false) && self.ticked.as_ref() == Some(&self.key())
    }

    /// Claude Code cannot show its trust prompt in a worktree it makes: such
    /// a chat needs the folder trusted first, or no worktree.
    fn trust_needed(&self) -> bool {
        self.provider() == Provider::Claude && self.worktree_on() && self.trusted() == Some(false) && !self.trust_it()
    }

    fn check(&self) -> Option<&Check> {
        self.check.as_ref().filter(|(k, _)| *k == self.key()).map(|(_, c)| c)
    }

    /// The worktree box is shown: the folder is in git.
    fn worktree_shown(&self) -> bool {
        self.check().is_some_and(|c| c.worktree_ok)
    }

    /// The worktree box is shown, and on.
    fn worktree_on(&self) -> bool {
        self.worktree && self.worktree_shown()
    }

    fn settings_ready(&self) -> bool {
        let want = Some(self.draft_key());
        match self.provider() {
            Provider::Claude => self.claude.ready_for == want,
            Provider::Codex => self.codex.ready_for == want,
        }
    }

    pub fn can_create(&self) -> bool {
        !self.folder.is_empty()
            && self.account.is_some()
            && self.trusted().is_some()
            && !self.trust_needed()
            && self.check().is_some_and(|c| c.refusal.is_none())
            && self.settings_ready()
            && !self.creating
            && self.list_wait.is_none()
    }

    fn settings(&self) -> Settings {
        match self.provider() {
            Provider::Claude => Settings::Claude(self.claude.settings.clone()),
            Provider::Codex => Settings::Codex(self.codex.settings.clone()),
        }
    }

    /// Esc: the open list goes first; nothing closes while a chat is made.
    pub fn may_close(&mut self, cx: &mut Context<Self>) -> bool {
        if self.menu.take().is_some() {
            cx.notify();
            return false;
        }
        !self.creating
    }

    pub fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_create() {
            return;
        }
        let Some(account) = self.account.clone() else { return };
        let mut make = Make::new(&self.folder, account, self.settings());
        make.worktree = self.worktree_on();
        make.tools = self.tools;
        make.trust = self.trust_it();
        self.creating = true;
        self.error = None;
        let models = self.models.clone();
        let layer = self.layer.clone();
        let handle = window.window_handle();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let made = create::create(&models, make, cx).await;
            let _ = handle.update(cx, |_, window, cx| {
                let _ = this.update(cx, |v, cx| {
                    v.creating = false;
                    match made {
                        Ok(_) => {
                            let _ = layer.update(cx, |l, cx| l.close(window, cx));
                        }
                        Err(e) => v.error = Some(e),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn close(&self, window: &mut Window, cx: &mut Context<Self>) {
        let _ = self.layer.update(cx, |l, cx| l.close(window, cx));
    }
}

/// The files' answer for an account in a folder, and the account's models.
async fn resolve(host: &Arc<dyn Host>, account: &Account, folder: &str) -> Resolved {
    let args = json!({ "accountPath": account.path, "folder": folder });
    match account.provider {
        Provider::Claude => {
            let dto = ask::<ClaudeDefaults>(host, cmd::CLAUDE_DEFAULTS, args).await.ok();
            let (values, origins) = choice::claude_settings(dto.as_ref(), &[]);
            let by_model = dto.map(|d| d.effort_by_model).unwrap_or_default();
            Resolved { claude: Some((values, origins, by_model)), codex: None }
        }
        Provider::Codex => {
            let dto = ask::<CodexDefaults>(host, cmd::CODEX_DEFAULTS, args).await.ok();
            let (values, origins) = choice::codex_settings(dto.as_ref());
            Resolved { claude: None, codex: Some((values, origins)) }
        }
    }
}

/// The account's models: asked of the CLI where its catalog does not say,
/// which takes seconds, so the dialog is usable on the four aliases meanwhile.
async fn fetch_models(host: &Arc<dyn Host>, account: &Account) -> (Option<Vec<ClaudeModel>>, Option<Vec<CodexModel>>) {
    match account.provider {
        Provider::Claude => {
            let m = ask::<Vec<ClaudeModel>>(host, cmd::CLAUDE_MODELS, json!({ "accountPath": account.path, "ask": true })).await;
            (Some(m.unwrap_or_default()), None)
        }
        Provider::Codex => {
            let m = ask::<Vec<CodexModel>>(host, cmd::CODEX_MODELS, json!({ "accountPath": account.path })).await;
            (None, Some(m.unwrap_or_default()))
        }
    }
}

impl Render for NewChat {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.models.shell.read(cx).palette();
        let close = self.act(cx, |v, window, cx| {
            if v.may_close(cx) {
                v.close(window, cx);
            }
        });
        let provider = self.provider();
        let mut body = div().flex().flex_col().gap(u(8.)).p(u(8.)).child(ui::note(
            &p,
            match provider {
                Provider::Claude => "Opens on this account's Claude Code settings; anything changed here applies to this chat only",
                Provider::Codex => "Opens on this account's Codex config.toml; anything changed here applies to this chat only",
            },
        ));
        body = body.child(self.folders(&p, cx)).child(self.account_row(&p, cx));
        body = match provider {
            Provider::Claude => body.child(self.claude_fields(&p, cx)),
            Provider::Codex => body.child(self.codex_fields(&p, cx)),
        };
        let can = self.can_create();
        let foot = div()
            .flex()
            .flex_none()
            .justify_end()
            .gap(u(6.))
            .p(u(8.))
            .border_t_1()
            .border_color(p.frame)
            .child(ui::button("newchat-cancel", &p, "Cancel", !self.creating, close.clone()))
            .child(ui::primary(
                "newchat-create",
                &p,
                if self.creating { "Creating..." } else { "Create" },
                can,
                self.act(cx, |v, window, cx| v.create(window, cx)),
            ));
        let scroll = div().id("newchat-scroll").flex_1().min_h_0().overflow_y_scroll().child(body);
        ui::frame(&p, "newchat", "New chat", u(520.), close, div().flex().flex_col().flex_1().min_h_0().child(scroll).child(foot))
    }
}

impl NewChat {
    fn folders(&self, p: &Palette, cx: &Context<Self>) -> Div {
        let offered = self.models.workspace.read(cx).offered_folders(self.also.as_deref());
        let mut list = div().id("newchat-folders").flex().flex_col().max_h(u(132.)).overflow_y_scroll().bg(p.well).p(u(2.));
        if offered.is_empty() {
            list = list.child(div().px(u(6.)).py(u(4.)).text_color(p.text_dim).child("Nothing here yet: browse for a folder."));
        }
        for (i, path) in offered.into_iter().enumerate() {
            let on = path == self.folder;
            let (parent, leaf) = tail2(&path);
            let (pick, forget) = (path.clone(), path.clone());
            let id = format!("newchat-folder-{i}");
            let sel = id.clone();
            let x_id = format!("{id}-x");
            let x_sel = x_id.clone();
            let this = cx.weak_entity();
            let this_x = cx.weak_entity();
            list = list.child(
                div()
                    .id(SharedString::from(id))
                    .selector(move || sel.clone())
                    .flex()
                    .items_center()
                    .gap(u(6.))
                    .px(u(6.))
                    .h(u(20.))
                    .when_else(on, |d| d.bg(p.title_bg).text_color(p.title_fg), |d| d.text_color(p.text))
                    .on_click(move |_, _, cx| {
                        let _ = this.update(cx, |v, cx| v.choose_folder(&pick, cx));
                    })
                    .child(div().flex_1().min_w_0().truncate().child(if parent.is_empty() { leaf } else { format!("{parent} / {leaf}") }))
                    .child(
                        div()
                            .id(SharedString::from(x_id))
                            .selector(move || x_sel.clone())
                            .flex_none()
                            .px(u(4.))
                            .font_weight(gpui::FontWeight::BOLD)
                            .on_click(move |_, _, cx| {
                                cx.stop_propagation();
                                let _ = this_x.update(cx, |v, cx| v.forget(&forget, cx));
                            })
                            .child("x"),
                    ),
            );
        }
        let mut section =
            div().flex().flex_col().gap(u(4.)).child(div().font_weight(gpui::FontWeight::BOLD).child("Folder")).child(list).child(
                div()
                    .flex()
                    .items_center()
                    .gap(u(8.))
                    .child(ui::button("newchat-browse", p, "Browse...", true, self.act(cx, |v, window, cx| v.pick_folder(window, cx))))
                    .child(
                        ui::path_text(p, if self.folder.is_empty() { "no folder chosen".to_owned() } else { self.folder.clone() }).flex_1(),
                    ),
            );
        if let (Some(account), false, false) = (&self.account, self.folder.is_empty(), self.trusted().unwrap_or(true)) {
            let ticked = self.trust_it();
            let note = match account.provider {
                Provider::Claude => {
                    "Ticked, Luna writes the same mark Claude Code's own trust prompt writes, and the chat opens on its prompt. Unticked, nothing is written and Claude Code asks in the chat itself."
                }
                Provider::Codex => {
                    "Ticked, Luna writes the same entry in config.toml that Codex's own trust screen writes, and the chat opens on its prompt. Unticked, nothing is written and Codex asks in the chat itself, then on Windows about its sandbox."
                }
            };
            let mut trust = div()
                .flex()
                .flex_col()
                .gap(u(3.))
                .p(u(6.))
                .border_1()
                .border_color(p.attention)
                .child(ui::check(
                    "newchat-trust",
                    p,
                    format!("Trust this folder in \"{}\"", account.name),
                    ticked,
                    true,
                    self.act(cx, |v, _, cx| v.tick_trust(cx)),
                ))
                .child(ui::note(p, note));
            if self.trust_needed() {
                trust = trust
                    .child(div().child("Claude Code cannot ask about trust in a worktree it makes: tick this, or turn Git worktree off."));
            }
            section = section.child(trust);
        }
        if let Some(why) = self.check().and_then(|c| c.refusal.as_ref()) {
            section = section.child(ui::alert(p, Tone::Trouble, why.clone()).id("newchat-refusal").selector(|| "newchat-refusal".into()));
        }
        if let Some(why) = &self.list_wait {
            section = section.child(ui::alert(p, Tone::Working, why.clone()).id("newchat-loading").selector(|| "newchat-loading".into()));
        }
        if let Some(e) = &self.error {
            section = section.child(
                ui::alert(p, Tone::Trouble, format!("Could not create the chat: {e}"))
                    .id("newchat-error")
                    .selector(|| "newchat-error".into()),
            );
        }
        section
    }

    fn account_row(&self, p: &Palette, cx: &Context<Self>) -> Div {
        let accounts: Vec<Account> = self.models.workspace.read(cx).accounts().map(<[Account]>::to_vec).unwrap_or_default();
        let face = match &self.account {
            Some(a) => format!("{} · {}", provider_label(a.provider), a.name),
            None if accounts.is_empty() => "no accounts".to_owned(),
            None => "choose an account".to_owned(),
        };
        let items = accounts
            .iter()
            .map(|a| {
                let chosen = self.account.as_ref() == Some(a);
                let a = a.clone();
                DropItem {
                    label: format!("{} · {}", provider_label(a.provider), a.name).into(),
                    chosen,
                    tick: false,
                    enabled: true,
                    pick: self.act(cx, move |v, _, cx| v.choose_account(a.clone(), cx)),
                }
            })
            .collect();
        let account = self.dropdown(p, cx, "account", face, items, u(220.), true);
        let mut isolation = div().flex().flex_col().gap(u(2.));
        if self.worktree_shown() {
            isolation = isolation.child(ui::check(
                "newchat-worktree",
                p,
                "Git worktree",
                self.worktree,
                true,
                self.act(cx, |v, _, cx| {
                    v.worktree = !v.worktree;
                    cx.notify();
                }),
            ));
        }
        isolation = isolation.child(ui::check(
            "newchat-tools",
            p,
            "Luna tools",
            self.tools,
            true,
            self.act(cx, |v, _, cx| {
                v.tools = !v.tools;
                cx.notify();
            }),
        ));
        let row = div()
            .flex()
            .gap(u(16.))
            .child(div().flex().flex_col().gap(u(4.)).child(div().font_weight(gpui::FontWeight::BOLD).child("Account")).child(account))
            .child(div().flex().flex_col().gap(u(4.)).child(div().font_weight(gpui::FontWeight::BOLD).child("Isolation")).child(isolation));
        if self.folder.is_empty() {
            return row;
        }
        // Where the boxes stand against the folder's start, and the ways
        // back and forward.
        let kept = self.kept_here(cx);
        let said = if !self.flipped(cx) {
            div().child(if kept { "as kept for this folder" } else { "as set in Settings" })
        } else {
            div()
                .flex()
                .gap(u(4.))
                .child("just for this chat:")
                .child(link(
                    "newchat-boxes-back",
                    p,
                    if kept { "back to this folder's start" } else { "back to the default" },
                    self.act(cx, |v, _, cx| v.boxes_back(cx)),
                ))
                .child("·")
                .child(link("newchat-boxes-keep", p, "keep for this folder", self.act(cx, |v, _, cx| v.keep_for_folder(cx))))
        };
        div().flex().flex_col().gap(u(2.)).child(row).child(div().flex().justify_end().text_color(p.text_dim).text_size(u(11.)).child(said))
    }

    #[allow(clippy::too_many_arguments)]
    fn dropdown(
        &self,
        p: &Palette,
        cx: &Context<Self>,
        id: &'static str,
        face: String,
        items: Vec<DropItem>,
        width: gpui::Pixels,
        enabled: bool,
    ) -> gpui::Stateful<Div> {
        Dropdown {
            id: format!("newchat-{id}"),
            face: face.into(),
            open: self.menu == Some(id),
            enabled,
            width,
            items,
            on_toggle: self.act(cx, move |v, _, cx| {
                v.menu = if v.menu == Some(id) { None } else { Some(id) };
                cx.notify();
            }),
            on_close: self.act(cx, move |v, _, cx| {
                if v.menu == Some(id) {
                    v.menu = None;
                    cx.notify();
                }
            }),
        }
        .render(p)
    }

    fn origin(&self, p: &Palette, cx: &Context<Self>, field: &'static str) -> Option<Div> {
        let (origins, changed) = match self.provider() {
            Provider::Claude => {
                let (s, r) = (&self.claude.settings, &self.claude.resolved);
                (
                    &self.claude.origins,
                    match field {
                        "model" => s.model != r.model,
                        "effort" => s.effort != r.effort,
                        _ => s.perm != r.perm,
                    },
                )
            }
            Provider::Codex => {
                let (s, r) = (&self.codex.settings, &self.codex.resolved);
                (
                    &self.codex.origins,
                    match field {
                        "model" => s.model != r.model,
                        "effort" => s.effort != r.effort,
                        // One line under both modes.
                        _ => s.approval != r.approval || s.sandbox != r.sandbox,
                    },
                )
            }
        };
        if changed {
            let back = format!("back to {}", self.describe_resolved(field));
            let id = format!("newchat-back-{field}");
            return Some(div().flex().gap(u(4.)).text_size(u(11.)).text_color(p.text_dim).child("just for this chat:").child(link(
                &id,
                p,
                back,
                self.act(cx, move |v, _, cx| v.revert(field, cx)),
            )));
        }
        let said = match origins.get(field) {
            Some(src) => format!("from {src}"),
            None => "nothing in settings says: Luna's own default".to_owned(),
        };
        Some(ui::note(p, said).text_size(u(11.)))
    }

    /// What a field goes back to, as its control shows it.
    fn describe_resolved(&self, field: &str) -> String {
        match self.provider() {
            Provider::Claude => {
                let r = &self.claude.resolved;
                match field {
                    "model" => r.model.clone(),
                    "effort" if r.effort.is_empty() => "no effort".to_owned(),
                    "effort" => r.effort.clone(),
                    _ => r.perm.clone(),
                }
            }
            Provider::Codex => {
                let r = &self.codex.resolved;
                match field {
                    "model" => match &r.model {
                        Some(slug) => {
                            self.codex_models.iter().flatten().find(|m| &m.slug == slug).map_or(slug.clone(), |m| m.display_name.clone())
                        }
                        None => "Codex's default model".to_owned(),
                    },
                    "effort" => r.effort.clone(),
                    _ => format!("{} / {}", codex_mode_label(&r.approval), codex_mode_label(&r.sandbox)),
                }
            }
        }
    }

    fn claude_fields(&self, p: &Palette, cx: &Context<Self>) -> Div {
        let s = &self.claude.settings;
        let list = self.claude_list();
        let listed = self.listed_claude();
        let mut items: Vec<DropItem> = list
            .iter()
            .map(|m| {
                let label = model_label_of(&m.slug);
                DropItem {
                    label: m.display_name.clone().into(),
                    chosen: label == s.model,
                    tick: false,
                    enabled: true,
                    pick: self.act(cx, move |v, _, cx| v.pick_claude_model(&label, cx)),
                }
            })
            .collect();
        // A model a settings file names that the list does not know still
        // gets an entry of its own, chosen.
        if listed.is_none() {
            items.push(DropItem {
                label: s.model.clone().into(),
                chosen: true,
                tick: false,
                enabled: true,
                pick: self.act(cx, |_, _, _| {}),
            });
        }
        let model = self.dropdown(
            p,
            cx,
            "model",
            listed.as_ref().map_or_else(|| s.model.clone(), |m| m.display_name.clone()),
            items,
            u(260.),
            true,
        );
        let mut efforts = choice::efforts_for(listed.as_ref());
        if !efforts.is_empty() && !s.effort.is_empty() && !efforts.contains(&s.effort) {
            efforts.push(s.effort.clone());
        }
        let effort: AnyElement = if efforts.is_empty() {
            ui::note(p, format!("{} takes no effort level.", listed.as_ref().map_or(s.model.as_str(), |m| m.display_name.as_str())))
                .into_any_element()
        } else {
            self.efforts(p, cx, &efforts, &s.effort)
        };
        let mut perms = div().flex().flex_col().gap(u(3.));
        for (perm, hint) in PERMS {
            perms = perms.child(
                div()
                    .flex()
                    .items_center()
                    .gap(u(8.))
                    .child(ui::radio(
                        &format!("newchat-perm-{perm}"),
                        p,
                        perm,
                        s.perm == perm,
                        true,
                        self.act(cx, move |v, _, cx| v.pick_perm(perm, cx)),
                    ))
                    .child(ui::note(p, hint)),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap(u(8.))
            .child(self.field(p, cx, "Model", model.into_any_element(), "model"))
            .child(self.field(p, cx, "Effort", effort, if efforts.is_empty() { "" } else { "effort" }))
            .child(self.field(p, cx, "Permission mode", perms.into_any_element(), "perm"))
    }

    fn codex_fields(&self, p: &Palette, cx: &Context<Self>) -> Div {
        let s = &self.codex.settings;
        let list = self.codex_models.clone().unwrap_or_default();
        let listed = list.iter().find(|m| Some(&m.slug) == s.model.as_ref());
        let mut items: Vec<DropItem> = list
            .iter()
            .map(|m| {
                let slug = m.slug.clone();
                DropItem {
                    label: m.display_name.clone().into(),
                    chosen: Some(&m.slug) == s.model.as_ref(),
                    tick: false,
                    enabled: true,
                    pick: self.act(cx, move |v, _, cx| v.pick_codex_model(Some(slug.clone()), cx)),
                }
            })
            .collect();
        if let (Some(m), None) = (&s.model, listed) {
            items.push(DropItem { label: m.clone().into(), chosen: true, tick: false, enabled: true, pick: self.act(cx, |_, _, _| {}) });
        }
        let model: AnyElement = if items.is_empty() {
            ui::note(
                p,
                if self.codex_models.is_none() {
                    "Asking Codex for its models..."
                } else {
                    "Codex gave no model list: the chat runs on its default model."
                },
            )
            .into_any_element()
        } else {
            let face = s
                .model
                .clone()
                .map(|m| list.iter().find(|x| x.slug == m).map_or(m.clone(), |x| x.display_name.clone()))
                .unwrap_or_else(|| list[0].display_name.clone());
            self.dropdown(p, cx, "model", face, items, u(260.), true).into_any_element()
        };
        let mut efforts: Vec<String> = match listed {
            Some(m) if !m.efforts.is_empty() => m.efforts.clone(),
            _ => CODEX_EFFORTS.iter().map(|e| (*e).to_owned()).collect(),
        };
        if !efforts.contains(&s.effort) {
            efforts.push(s.effort.clone());
        }
        let auto = s.approval == "auto-review";
        let mut approvals = div().flex().flex_col().gap(u(3.));
        for a in CODEX_APPROVALS {
            approvals = approvals.child(ui::radio(
                &format!("newchat-approval-{a}"),
                p,
                codex_mode_label(a),
                s.approval == a,
                true,
                self.act(cx, move |v, _, cx| v.pick_approval(a, cx)),
            ));
        }
        let mut sandboxes = div().flex().flex_col().gap(u(3.));
        for sb in CODEX_SANDBOXES {
            sandboxes = sandboxes.child(ui::radio(
                &format!("newchat-sandbox-{sb}"),
                p,
                codex_mode_label(sb),
                s.sandbox == sb,
                !auto,
                self.act(cx, move |v, _, cx| v.pick_sandbox(sb, cx)),
            ));
        }
        let modes = div()
            .flex()
            .flex_col()
            .gap(u(4.))
            .child(div().flex().gap(u(24.)).child(approvals).child(sandboxes))
            .child(ui::note(p, choice::codex_mode_hint(&s.approval, &s.sandbox)));
        div()
            .flex()
            .flex_col()
            .gap(u(8.))
            .child(self.field(p, cx, "Model", model, "model"))
            .child(self.field(p, cx, "Reasoning effort", self.efforts(p, cx, &efforts, &s.effort), "effort"))
            .child(self.field(p, cx, "Approval & sandbox", modes.into_any_element(), "approval"))
    }

    fn efforts(&self, p: &Palette, cx: &Context<Self>, levels: &[String], current: &str) -> AnyElement {
        ui::segmented(
            "newchat-effort",
            p,
            levels
                .iter()
                .map(|e| {
                    let level = e.clone();
                    (SharedString::from(e.clone()), e == current, self.act(cx, move |v, _, cx| v.pick_effort(&level, cx)))
                })
                .collect(),
            true,
        )
        .into_any_element()
    }

    fn field(&self, p: &Palette, cx: &Context<Self>, label: &'static str, control: AnyElement, origin: &'static str) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(u(3.))
            .child(div().font_weight(gpui::FontWeight::BOLD).child(label))
            .child(control)
            .children(if origin.is_empty() { None } else { self.origin(p, cx, origin) })
    }
}

/// A word to click in a line of notes: "back to …", "keep for this folder".
fn link(id: &str, p: &Palette, label: impl Into<SharedString>, on_click: OnClick) -> gpui::Stateful<Div> {
    let selector = id.to_owned();
    div()
        .id(SharedString::from(id.to_owned()))
        .selector(move || selector.clone())
        .text_color(p.text)
        .font_weight(gpui::FontWeight::BOLD)
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            on_click(window, cx)
        })
        .child(label.into())
}

trait WhenElse: Sized {
    fn when_else(self, cond: bool, yes: impl FnOnce(Self) -> Self, no: impl FnOnce(Self) -> Self) -> Self {
        if cond { yes(self) } else { no(self) }
    }
}

impl<T: Styled> WhenElse for T {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_is_named_by_its_last_two_parts() {
        assert_eq!(tail2("E:\\Projects\\luna\\"), ("Projects".into(), "luna".into()));
        assert_eq!(tail2("/home/me/repo"), ("me".into(), "repo".into()));
        assert_eq!(tail2("repo"), (String::new(), "repo".into()));
    }
}
