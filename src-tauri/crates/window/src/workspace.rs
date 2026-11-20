//! The chats and the boards as the window works with them: the state the
//! host keeps (state.rs) and what is only this run's beside it, each chat's
//! session as the core last told it (status, title, context, draft), the
//! accounts to start sessions on, and the choices the person makes on the
//! board and in the sidebar.
//!
//! - The state comes from the host at start (`cmd::WINDOW_STATE`), or from
//!   0.19's chat list (import.rs) where there is none, and goes back as the
//!   page's throttled storage wrote it (`Saver`): a change after a quiet
//!   spell at once, then at most one write per `GAP`; a change of the chats
//!   themselves, their sessions, accounts or boards at once; what waits when
//!   the app quits written then (`flush`). The host says of each write
//!   whether it was made (`on_saved`). A list the host could not read is
//!   never written over, a failed import never written in place of one, and
//!   each is said in a notice above the board (`notices`), with Retry. One
//!   read is asked at a time, and its answer is let go of where the list
//!   was changed meanwhile (`read_state`).
//! - The app quits only once the host says the latest state is written
//!   (`quit`, `cmd::FLUSH_WINDOW_STATE`, whose answer carries the `seq` it
//!   wrote: a change made after it is flushed again); where it is not, the window stays,
//!   shown, with a notice offering Retry or Quit anyway. The Windows
//!   session's end does not ask: the host tries once more and logs.
//! - An update's install freezes the list (`freeze`): from its last write,
//!   held to the same rule (`save_for_install`), until the installer has it
//!   or the update is called off (`thaw`), the person's changes are refused
//!   and a notice says why. What the sessions change meanwhile (an exit, a
//!   title) is still written.
//! - Every chat with an account is looked at every `EVERY` through
//!   `sessions_meta`, as the page's `useSessionWatch` did: status, title,
//!   context, draft and session id, a session that is not running resting.
//!   An answer is taken only for the core and the sessions it was asked of
//!   (`Fence`); a call that fails or hangs changes nothing. The accounts
//!   are asked for again, ever later, until a list comes, and when the core
//!   comes up without one. A chat turning
//!   to waiting is a desktop notification, once per wait, as is what the
//!   core asks to be one (`event::NOTIFY`); a click on one opens the chat.
//! - What another slice adds (the new-chat, quick-chat and delete dialogs)
//!   comes in through `Seams`.

use crate::desktop::{Notice, OpenChat};
use crate::dto::LinkState;
use crate::host::{Host, cmd};
use crate::models::CoreLink;
use crate::panels::cmd as panels_cmd;
use crate::panels::{NewChatStart, Prefs};
use crate::state::{Chat, NameSource, Placed, Status, WindowState};
use crate::term::SessionSpec;
use crate::term::paste::Provider;
use gpui::{App, Context, Entity, Global, Task, WeakEntity, Window};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How often every chat's session is looked at.
pub const EVERY: Duration = Duration::from_secs(4);
/// How long a look waits for its answer: past it, the round is given up and
/// the next tick asks again.
pub const POLL_TIMEOUT: Duration = Duration::from_secs(20);
/// The least time between two writes of the state.
pub const GAP: Duration = Duration::from_millis(500);
/// How many times a quit asks the host to flush again, because the list
/// changed while it did.
const QUIT_FLUSHES: usize = 5;
/// How long a state the host could not take waits before it is sent again.
pub const SEND_RETRY: Duration = Duration::from_secs(1);
/// How long the accounts wait to be asked for again after a failure, at
/// first and at most.
pub const ACCOUNTS_RETRY_FIRST: Duration = Duration::from_secs(1);
pub const ACCOUNTS_RETRY_MAX: Duration = Duration::from_secs(30);

/// A one-line notice above the board about the chat list's file or the
/// import (root.rs draws them).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateNotice {
    /// Its name: `quit`, `read`, `import`, `kept`, `aside` or `save`, which
    /// its element and buttons are named by (`<key>-notice`, `-retry`,
    /// `-ok`, `-quit`).
    pub key: &'static str,
    pub text: String,
    /// It offers Retry (`Workspace::retry`).
    pub retry: bool,
    /// It offers OK (`Workspace::dismiss`).
    pub ok: bool,
    /// It offers Quit anyway (`Workspace::quit_anyway`).
    pub quit_anyway: bool,
}

/// What the host's answer said of the chat list's file, this run only.
#[derive(Default)]
struct FileNotices {
    /// It could not be read: nothing is written over it until it is.
    read_error: Option<String>,
    /// It did not all read: what was left out, and where the host kept the
    /// file as it was.
    kept: Option<(String, String)>,
    /// It was no JSON, and the host put it aside here.
    aside: Option<String>,
    /// The host's last write of it failed, and it tries again.
    save_error: Option<String>,
    /// The app was to quit and the host could not write the latest state.
    quit_error: Option<String>,
}

/// An account Luna knows (`AccountInfo` in src/ipc/accounts.ts).
#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct Account {
    pub provider: Provider,
    pub name: String,
    pub path: String,
}

/// What a chat's session is doing now, as the core last said: never kept.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Live {
    /// The context used, 0 to 1.
    pub context: Option<f64>,
    /// An unsent draft in its composer.
    pub draft: bool,
    /// Messages Luna holds back for it.
    pub held: u64,
}

impl Live {
    /// The tag a row and a pane wear for the draft, or None.
    pub fn draft_tag(&self) -> Option<String> {
        match (self.draft, self.held) {
            (true, 0) => Some("draft".into()),
            (true, n) => Some(format!("draft · {n}")),
            (false, 0) => None,
            (false, n) => Some(format!("{n} held")),
        }
    }
}

/// `SessionMetaDto`, as far as the window reads it.
#[derive(Deserialize, Default, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Meta {
    pub status: Option<String>,
    pub cwd: Option<String>,
    pub session_id: Option<String>,
    pub model: Option<String>,
    pub context: Option<f64>,
    pub title: Option<String>,
    pub title_renamed: bool,
    pub first_prompt: Option<String>,
    pub draft: bool,
    pub held: u64,
}

/// `ChatLookDto`: a chat and its session, None when its CLI does not run.
#[derive(Deserialize, Debug)]
pub struct Look {
    pub id: String,
    #[serde(default)]
    pub meta: Option<Meta>,
}

/// What a look was asked under: the core's epoch (models.rs `CoreLink`) and
/// each chat's exits heard by then (`Workspace::apply`).
#[derive(Clone, Debug, Default)]
pub(crate) struct Fence {
    pub epoch: u64,
    pub exits: HashMap<String, u64>,
}

/// `CliTitleDto`.
#[derive(Deserialize)]
struct SavedTitle {
    title: String,
    #[serde(default)]
    renamed: bool,
}

/// Something done for the window by a dialog of another slice: its window
/// and the app to open in.
pub type Seam = Rc<dyn Fn(&mut Window, &mut App)>;
/// The same, about one chat.
pub type ChatSeam = Rc<dyn Fn(String, &mut Window, &mut App)>;

/// Where the dialogs of the next slices plug in. Until they do, what calls
/// for one is logged and does nothing.
#[derive(Default, Clone)]
pub struct Seams {
    /// The New chat button and Ctrl+N: the new-chat dialog, which then
    /// calls `Workspace::add_chat`.
    pub new_chat: Option<Seam>,
    /// Ctrl+Shift+N: a chat in the last folder and account, at once.
    pub quick_chat: Option<Seam>,
    /// A chat's Delete: the dialog that asks first, with "stop or keep its
    /// helpers", which then calls `Workspace::remove_chat`.
    pub delete_chat: Option<ChatSeam>,
    /// The status bar's Settings button: the settings panels.
    pub settings: Option<Seam>,
    /// The Background count's click: the helpers' popover.
    pub helpers: Option<Seam>,
}

/// The state's writes, as the page's throttled storage made them.
#[derive(Default)]
struct Saver {
    /// The last write's time.
    last_at: Option<Instant>,
    /// What it wrote, and the chats it had: a write of the same text is
    /// skipped, one of other chats never waits.
    last_text: Option<String>,
    last_key: Option<Vec<(String, String, Option<String>, String, usize, Option<String>)>>,
    /// A write waits for the gap, or to be sent again.
    waiting: Option<Task<()>>,
    /// The number of the last state sent; the host says of each whether it
    /// was written (`event::STATE_SAVED`).
    seq: u64,
    /// The newest state the host wrote, and the newest it failed to.
    saved: u64,
    failed: u64,
}

/// The workspace the app's quit flushes (`flush`).
struct Current(WeakEntity<Workspace>);

impl Global for Current {}

pub struct Workspace {
    host: Arc<dyn Host>,
    core: Entity<CoreLink>,
    pub(crate) state: WindowState,
    /// The host's state has come: before it, nothing is written.
    loaded: bool,
    live: HashMap<String, Live>,
    /// None until the host has listed them.
    accounts: Option<Vec<Account>>,
    /// What the window keeps of its own beside its theme (`window_prefs`),
    /// which Settings writes: the defaults stand until the host has answered.
    prefs: Prefs,
    /// A change of the preferences was taken (`take_prefs`): an answer to the
    /// first ask, come after, is older.
    prefs_taken: bool,
    /// Sessions that exited: a pane showing one again starts it anew.
    ended: HashSet<String>,
    /// Chats whose saved title was looked up this run.
    title_looked: HashSet<String>,
    /// A notification's chat, come before the state.
    pending_open: Option<String>,
    /// The chat whose terminal should take the keyboard, for the board.
    pub(crate) focus_request: Option<String>,
    saver: Saver,
    pub seams: Seams,
    /// Times the meta was asked, for the tests.
    pub(crate) rounds: usize,
    /// A round is under way: the next tick waits for it to end.
    polling: bool,
    /// Each chat's session's exits heard this run: a look asked before one
    /// says nothing of the session after it (`apply`).
    exits: HashMap<String, u64>,
    /// Chats whose wait was told as a notification, until they stop waiting.
    announced: HashSet<String>,
    notices: FileNotices,
    /// The core was up when last looked at.
    core_was_up: bool,
    /// The reads of the state (`read_state`): the number of the latest
    /// asked, and whether it is under way.
    read_id: u64,
    reading: bool,
    /// The changes made to the state since it was loaded (`changed`).
    edits: u64,
    /// The app is to quit once the host says the state is written.
    quitting: bool,
    /// An update is being installed: the person's changes are refused.
    frozen: bool,
    /// The accounts are being asked for, and are to be asked again once
    /// they come; after a failure, the wait before the next ask and the
    /// task waiting it out.
    accounts_loading: bool,
    accounts_again: bool,
    accounts_wait: Duration,
    _accounts_retry: Option<Task<()>>,
    _poll: Option<Task<()>>,
}

/// What an import of 0.19's list came to.
enum Import {
    /// The state it made: written at once, so the next start imports nothing.
    Done(WindowState),
    /// It failed: the list starts empty and says why, and is not written, so
    /// a later start (or Retry) tries again while it stays empty.
    Failed(WindowState),
    /// There was nothing to import: no store, or one with no chat list.
    Nothing,
}

/// What a pane of a chat says while it cannot show a terminal.
pub type Why = String;

/// What an acknowledged flush (`Workspace::flush_acked`) does once it knows.
type Then = Box<dyn FnOnce(&mut Workspace, Result<(), String>, &mut Context<Workspace>)>;

impl Workspace {
    pub fn new(host: Arc<dyn Host>, core: Entity<CoreLink>, cx: &mut Context<Self>) -> Workspace {
        let me = cx.weak_entity();
        cx.set_global(Current(me));
        cx.observe_global::<OpenChat>(|this, cx| {
            // Taken only when there is one: taking it is a change of the
            // global too, which calls this again.
            let id = cx.global::<OpenChat>().0.clone();
            if let Some(id) = id {
                cx.global_mut::<OpenChat>().0 = None;
                this.open_chat(&id, cx);
            }
        })
        .detach();
        // A core that comes up is asked at once, not at the next tick; one
        // that comes or goes changes what every row shows (`shown_status`).
        cx.observe(&core, |this, core, cx| {
            let up = core.read(cx).link().state == LinkState::Up;
            if up != this.core_was_up {
                this.core_was_up = up;
                cx.notify();
            }
            if up && this.accounts.is_none() {
                this.reload_accounts(cx);
            }
            if up && this.rounds == 0 {
                this.poll(cx);
            }
        })
        .detach();
        Workspace {
            host,
            core,
            state: WindowState::default(),
            loaded: false,
            live: HashMap::new(),
            accounts: None,
            prefs: Prefs::default(),
            prefs_taken: false,
            ended: HashSet::new(),
            title_looked: HashSet::new(),
            pending_open: None,
            focus_request: None,
            saver: Saver::default(),
            seams: Seams::default(),
            rounds: 0,
            polling: false,
            exits: HashMap::new(),
            announced: HashSet::new(),
            notices: FileNotices::default(),
            core_was_up: false,
            read_id: 0,
            reading: false,
            edits: 0,
            quitting: false,
            frozen: false,
            accounts_loading: false,
            accounts_again: false,
            accounts_wait: ACCOUNTS_RETRY_FIRST,
            _accounts_retry: None,
            _poll: None,
        }
    }

    /// Asks the host for the state and the accounts, and starts looking at
    /// the sessions.
    pub fn load(&mut self, cx: &mut Context<Self>) {
        self.read_state(cx);
        self.reload_accounts(cx);
        self.load_prefs(cx);
        self._poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(EVERY).await;
                if this.update(cx, |this, cx| this.poll(cx)).is_err() {
                    break;
                }
            }
        }));
    }

    /// Asks the host for the accounts again: one ask at a time (one asked
    /// meanwhile is asked once that one is answered), and one that fails is
    /// asked again `ACCOUNTS_RETRY_FIRST` later, twice as long each time up
    /// to `ACCOUNTS_RETRY_MAX`, until a list comes.
    pub fn reload_accounts(&mut self, cx: &mut Context<Self>) {
        if self.accounts_loading {
            self.accounts_again = true;
            return;
        }
        self.accounts_loading = true;
        self._accounts_retry = None;
        let answer = self.host.call(cmd::LIST_ACCOUNTS, json!({}));
        cx.spawn(async move |this, cx| {
            let accounts = answer.await.and_then(|v| serde_json::from_value::<Vec<Account>>(v).map_err(|e| e.to_string()));
            let _ = this.update(cx, |this, cx| {
                this.accounts_loading = false;
                match accounts {
                    Ok(accounts) => {
                        this.accounts = Some(accounts);
                        this.accounts_wait = ACCOUNTS_RETRY_FIRST;
                        cx.notify();
                        if std::mem::take(&mut this.accounts_again) {
                            return this.reload_accounts(cx);
                        }
                        // The chats with an account are looked at now, not
                        // at the next tick.
                        this.poll(cx);
                    }
                    Err(e) => {
                        this.accounts_again = false;
                        let wait = this.accounts_wait;
                        this.accounts_wait = (wait * 2).min(ACCOUNTS_RETRY_MAX);
                        log::warn!("list_accounts: {e}; asked again in {} ms", wait.as_millis());
                        this._accounts_retry = Some(cx.spawn(async move |this, cx| {
                            cx.background_executor().timer(wait).await;
                            let _ = this.update(cx, |this, cx| this.reload_accounts(cx));
                        }));
                    }
                }
            });
        })
        .detach();
    }

    /// Asks the host for the state: at start, and for a notice's Retry. One
    /// read at a time, its answer taken only where it is the latest asked
    /// and the list was not changed since it was asked: a Retry answered
    /// late must not put a list back over what was done meanwhile.
    fn read_state(&mut self, cx: &mut Context<Self>) {
        if self.reading {
            return;
        }
        self.reading = true;
        self.read_id += 1;
        let (id, at_edit) = (self.read_id, self.edits);
        let state = self.host.call(cmd::WINDOW_STATE, json!({}));
        cx.spawn(async move |this, cx| {
            let answer = state.await;
            let _ = this.update(cx, |this, cx| {
                if id != this.read_id {
                    return;
                }
                this.reading = false;
                if this.loaded && this.edits != at_edit {
                    log::warn!("a read of the chat list was answered after the list was changed, and let go");
                    return;
                }
                this.loaded(answer, cx);
            });
        })
        .detach();
    }

    /// The host's state, or 0.19's chat list on the first start. A file the
    /// host could not read is no list at all: nothing is imported and
    /// nothing written over it until a read (Retry, or the next start) gets
    /// it.
    fn loaded(&mut self, answer: Result<Value, String>, cx: &mut Context<Self>) {
        let answer = answer.unwrap_or_else(|e| json!({ "readError": format!("the host did not answer: {e}") }));
        let text = |key: &str| answer.get(key).and_then(Value::as_str).map(str::to_owned);
        if let Some(why) = text("readError") {
            log::warn!("the chat list could not be read: {why}");
            self.notices.read_error = Some(why);
            cx.notify();
            return;
        }
        self.notices.read_error = None;
        self.notices.aside = text("aside");
        let saved = answer.get("saved").filter(|v| !v.is_null());
        let (state, write) = match saved {
            Some(saved) => {
                let Some((state, lost)) = WindowState::read(saved) else {
                    self.notices.read_error = Some("the kept file holds no chat list".into());
                    cx.notify();
                    return;
                };
                if lost.any() {
                    let what = lost.words();
                    log::warn!("the chat list did not all read: {what}");
                    self.notices.kept = Some((what, text("kept").unwrap_or_else(|| "nowhere: the host kept no copy".into())));
                }
                // An import that failed and left the list empty is tried
                // again, where the host read the old store for it.
                let asked = answer.get("old").is_some_and(|o| !o.is_null()) || answer.get("oldError").is_some_and(|o| !o.is_null());
                match (state.import_retryable() && asked).then(|| self.import(&answer)) {
                    Some(Import::Done(imported)) => (imported, true),
                    Some(Import::Failed(failed)) => (failed, false),
                    Some(Import::Nothing) | None => (state, false),
                }
            }
            None => match self.import(&answer) {
                Import::Done(imported) => (imported, true),
                Import::Failed(failed) => (failed, false),
                // Written, so a later start does not look again.
                Import::Nothing => (WindowState::default(), true),
            },
        };
        // What was done before the state came (a board chosen) gives way to
        // it: nothing of it was kept.
        self.state = state;
        // A chat kept as waiting was told of in the run that saw it begin.
        self.announced = self.state.chats().filter(|c| c.status == Status::Waiting).map(|c| c.id.clone()).collect();
        self.loaded = true;
        if let Some(id) = self.pending_open.take() {
            self.open_chat(&id, cx);
        }
        if write {
            self.write(cx);
        }
        cx.notify();
        self.poll(cx);
    }

    /// 0.19's chat list from the host's read of its store. No store at all
    /// (a machine 0.19 never ran on), or one that kept no chat list, is
    /// nothing to import and says nothing.
    fn import(&mut self, answer: &Value) -> Import {
        let old = answer.get("old").and_then(|o| serde_json::from_value::<HashMap<String, String>>(o.clone()).ok());
        let error = answer.get("oldError").and_then(Value::as_str).map(str::to_owned);
        let why = match (old, error) {
            (Some(old), _) => match crate::import::from_storage(&old) {
                Ok(Some((state, found))) => {
                    log::warn!(
                        "imported 0.19's chat list: {} chats in {} folders, {} groups ({} skipped)",
                        found.chats,
                        found.folders,
                        found.groups,
                        found.skipped
                    );
                    return Import::Done(state);
                }
                Ok(None) => return Import::Nothing,
                Err(why) => why,
            },
            (None, Some(why)) => why,
            (None, None) => return Import::Nothing,
        };
        log::warn!("0.19's chat list was not imported: {why}");
        Import::Failed(crate::import::failed(why))
    }

    /// The one-line notice that 0.19's chat list could not be read, until it
    /// is dismissed.
    pub fn notice(&self) -> Option<String> {
        let imported = self.state.imported.as_ref().filter(|i| !i.ok && !i.seen)?;
        let why = imported.why.as_deref().unwrap_or("no reason given");
        Some(format!("Luna 0.19's chat list could not be read ({why}); the list starts empty until it can be."))
    }

    /// Every notice up now, the most pressing first.
    pub fn notices(&self) -> Vec<StateNotice> {
        let n = |key, text: String, retry, ok| StateNotice { key, text, retry, ok, quit_anyway: false };
        let mut out = Vec::new();
        if self.frozen {
            out.push(n(
                "frozen",
                "Installing the update: the chat list takes no changes until it is installed or called off.".into(),
                false,
                false,
            ));
        }
        if let Some(why) = &self.notices.quit_error {
            let text = format!(
                "The chat list could not be saved before quitting ({why}). Retry, or Quit anyway and lose what changed since it was last saved."
            );
            out.push(StateNotice { quit_anyway: true, ..n("quit", text, true, false) });
        }
        if let Some(why) = &self.notices.read_error {
            out.push(n(
                "read",
                format!("The chat list could not be read ({why}). Nothing is saved over it until it is read: Retry reads it again."),
                true,
                false,
            ));
        }
        if let Some(why) = &self.notices.save_error {
            out.push(n("save", format!("The chat list could not be saved ({why}). Luna keeps trying."), false, false));
        }
        if let Some((what, path)) = &self.notices.kept {
            out.push(n(
                "kept",
                format!("Part of the chat list could not be read: {what}. The file as it was is kept as {path}."),
                false,
                true,
            ));
        }
        if let Some(path) = &self.notices.aside {
            out.push(n(
                "aside",
                format!("The chat list's file did not read and was put aside as {path}; the list starts empty."),
                false,
                true,
            ));
        }
        if let Some(text) = self.notice() {
            out.push(n("import", text, true, true));
        }
        out
    }

    /// A notice's OK.
    pub fn dismiss(&mut self, key: &str, cx: &mut Context<Self>) {
        match key {
            "kept" => self.notices.kept = None,
            "aside" => self.notices.aside = None,
            "import" => return self.dismiss_notice(cx),
            _ => return,
        }
        cx.notify();
    }

    /// A notice's Retry: the file read again, or the import tried again.
    pub fn retry(&mut self, key: &str, cx: &mut Context<Self>) {
        match key {
            "quit" => self.quit(cx),
            "read" => self.read_state(cx),
            "import" if self.state.import_retryable() => self.read_state(cx),
            _ => {}
        }
    }

    pub fn dismiss_notice(&mut self, cx: &mut Context<Self>) {
        if self.frozen {
            return;
        }
        if let Some(i) = self.state.imported.as_mut() {
            i.seen = true;
            self.changed(cx);
        }
    }

    pub fn loaded_yet(&self) -> bool {
        self.loaded
    }

    pub fn live(&self, id: &str) -> Live {
        self.live.get(id).cloned().unwrap_or_default()
    }

    pub fn accounts(&self) -> Option<&[Account]> {
        self.accounts.as_deref()
    }

    /// The window's own preferences, as Settings last wrote them.
    pub fn prefs(&self) -> &Prefs {
        &self.prefs
    }

    /// How the new-chat dialog's boxes start in a folder with no start of
    /// its own (Settings, New chats).
    pub fn new_chat_start(&self) -> NewChatStart {
        self.prefs.new_chat
    }

    /// Settings changed the preferences: the next dialog starts from these.
    pub fn take_prefs(&mut self, prefs: Prefs, cx: &mut Context<Self>) {
        self.prefs_taken = true;
        if self.prefs != prefs {
            self.prefs = prefs;
            cx.notify();
        }
    }

    /// Asks the host for the preferences: what Settings wrote in an earlier
    /// run. A failure leaves the defaults, which are what a first run has.
    fn load_prefs(&mut self, cx: &mut Context<Self>) {
        let answer = self.host.call(panels_cmd::WINDOW_PREFS, json!({}));
        cx.spawn(async move |this, cx| {
            let prefs = answer.await.and_then(|v| serde_json::from_value::<Prefs>(v).map_err(|e| e.to_string()));
            let _ = this.update(cx, |this, cx| match prefs {
                Ok(prefs) if !this.prefs_taken => this.take_prefs(prefs, cx),
                Ok(_) => {}
                Err(e) => log::warn!("{}: {e}", panels_cmd::WINDOW_PREFS),
            });
        })
        .detach();
    }

    /// An account was renamed (the accounts window): the chats on it follow,
    /// and the accounts are listed again.
    pub fn rename_account(&mut self, provider: Provider, from: &str, to: &str, cx: &mut Context<Self>) {
        if self.state.rename_account(provider, from, to) {
            self.changed(cx);
        }
        self.reload_accounts(cx);
    }

    /// The names of the chats set to an account, as the question to delete
    /// it lists them.
    pub fn chats_on(&self, provider: Provider, account: &str) -> Vec<String> {
        self.state.chats_on(provider, account).map(|c| c.name.clone()).collect()
    }

    // --------------------------------------------------------------- writes

    /// The state changed: the views redraw, and it goes to the host as the
    /// page's throttled storage wrote it.
    pub(crate) fn changed(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        if !self.loaded {
            return;
        }
        self.edits += 1;
        let critical = self.saver.last_key.as_ref() != Some(&self.state.critical_key());
        let now = cx.background_executor().now();
        let quiet = self.saver.last_at.is_none_or(|at| now.duration_since(at) >= GAP);
        if critical || quiet {
            self.write(cx);
        } else if self.saver.waiting.is_none() {
            let wait = GAP.saturating_sub(now.duration_since(self.saver.last_at.unwrap_or(now)));
            self.saver.waiting = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(wait).await;
                let _ = this.update(cx, |this, cx| this.write(cx));
            }));
        }
    }

    /// Writes the state now, unless it is the text written last.
    fn write(&mut self, cx: &mut Context<Self>) {
        self.saver.waiting = None;
        self.saver.last_at = Some(cx.background_executor().now());
        self.saver.last_key = Some(self.state.critical_key());
        let text = self.state.to_text();
        if self.saver.last_text.as_ref() == Some(&text) {
            return;
        }
        let seq = self.saver.seq + 1;
        match self.host.send(cmd::SAVE_WINDOW_STATE, json!({ "text": text, "seq": seq })) {
            Ok(()) => {
                self.saver.seq = seq;
                self.saver.last_text = Some(text);
            }
            Err(e) => {
                // Not handed over: sent again in a moment, or sooner with
                // the next change.
                log::warn!("save_window_state: {e}");
                self.saver.last_text = None;
                self.saver.waiting = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(SEND_RETRY).await;
                    let _ = this.update(cx, |this, cx| this.write(cx));
                }));
            }
        }
    }

    /// The host wrote state `seq`, or failed to (`event::STATE_SAVED`); it
    /// tries a failed one again itself, until a newer one replaces it. The
    /// notice says so until a write as new as the failure succeeds.
    pub fn on_saved(&mut self, seq: u64, error: Option<String>, cx: &mut Context<Self>) {
        match error {
            Some(e) => {
                self.saver.failed = self.saver.failed.max(seq);
                if self.notices.save_error.as_ref() != Some(&e) {
                    log::warn!("the chat list was not saved: {e}");
                    self.notices.save_error = Some(e);
                    cx.notify();
                }
            }
            None => {
                self.saver.saved = self.saver.saved.max(seq);
                if self.saver.saved >= self.saver.failed && self.notices.save_error.take().is_some() {
                    cx.notify();
                }
            }
        }
    }

    /// The last state sent was written: nothing of the list waits.
    pub fn saved_all(&self) -> bool {
        self.saver.waiting.is_none() && self.saver.saved >= self.saver.seq
    }

    /// What waits for the gap, written now: the app quits.
    pub fn flush_now(&mut self, cx: &mut Context<Self>) {
        if self.saver.waiting.is_some() {
            self.write(cx);
        }
    }

    /// The app is to quit (the tray's Quit, an update, a Restart, the window
    /// closed with no tray): what waits is sent, and the app quits once the
    /// host says the latest state is written. Where it is not, the window
    /// stays, shown, and says so, with Retry and Quit anyway. With nothing
    /// read there is nothing to lose: a list the host could not read was
    /// never written.
    pub fn quit(&mut self, cx: &mut Context<Self>) {
        if self.quitting {
            return;
        }
        if !self.loaded {
            return cx.quit();
        }
        self.quitting = true;
        self.notices.quit_error = None;
        self.flush_acked(
            QUIT_FLUSHES,
            Box::new(|this: &mut Workspace, wrote: Result<(), String>, cx: &mut Context<Workspace>| match wrote {
                Ok(()) => cx.quit(),
                Err(why) => this.quit_failed(why, cx),
            }),
            cx,
        );
    }

    /// One flush that must be acknowledged, for a quit or an install: what
    /// waits is sent, the host is asked to write it, and `then` is told Ok
    /// only if the state the host says it wrote (its answer's `seq`) is the
    /// latest sent by the time the answer is handled. A change made
    /// meanwhile is a later state the answer does not cover (and the exit
    /// hook's own write, which can fail, would be the only one left): the
    /// flush is asked again, `left` more times at most.
    fn flush_acked(&mut self, left: usize, then: Then, cx: &mut Context<Self>) {
        self.flush_now(cx);
        if self.saver.waiting.is_some() {
            return then(self, Err("the host did not take the last change".into()), cx);
        }
        let answer = self.host.call(cmd::FLUSH_WINDOW_STATE, json!({}));
        cx.spawn(async move |this, cx| {
            let answer = answer.await;
            let _ = this.update(cx, |this, cx| match answer {
                Ok(v) => {
                    let Some(wrote) = v.get("seq").and_then(Value::as_u64) else {
                        return then(this, Err("the host's answer names no state it wrote".into()), cx);
                    };
                    // What changed since is sent now, so that it is the
                    // latest this compares with.
                    this.flush_now(cx);
                    if this.saver.waiting.is_some() {
                        return then(this, Err("the host did not take the last change".into()), cx);
                    }
                    if wrote >= this.saver.seq {
                        then(this, Ok(()), cx);
                    } else if left > 0 {
                        this.flush_acked(left - 1, then, cx);
                    } else {
                        then(this, Err("the chat list kept changing".into()), cx);
                    }
                }
                Err(why) => then(this, Err(why), cx),
            });
        })
        .detach();
    }

    /// An update is about to be installed: the person's changes are refused
    /// from here (the notice says why) until it is called off (`thaw`), so
    /// that none comes after the list's last write.
    pub fn freeze(&mut self, cx: &mut Context<Self>) {
        if !std::mem::replace(&mut self.frozen, true) {
            cx.notify();
        }
    }

    /// The update is off: the list takes changes again.
    pub fn thaw(&mut self, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.frozen) {
            cx.notify();
        }
    }

    pub fn frozen(&self) -> bool {
        self.frozen
    }

    /// The list written for an install, by the quit's rule (`flush_acked`):
    /// `then` is told Ok once the host says the latest state is written, or
    /// why not. With nothing read there is nothing to lose.
    pub fn save_for_install(&mut self, then: impl FnOnce(Result<(), String>, &mut App) + 'static, cx: &mut Context<Self>) {
        if !self.loaded {
            return then(Ok(()), cx);
        }
        self.flush_acked(QUIT_FLUSHES, Box::new(move |_: &mut Workspace, wrote, cx: &mut Context<Workspace>| then(wrote, cx)), cx);
    }

    /// Whether a quit is under way or done, for the tests.
    #[cfg(test)]
    pub(crate) fn quitting(&self) -> bool {
        self.quitting
    }

    fn quit_failed(&mut self, why: String, cx: &mut Context<Self>) {
        log::warn!("the app did not quit: the chat list could not be saved ({why})");
        self.quitting = false;
        self.notices.quit_error = Some(why);
        crate::desktop::on_request(crate::desktop::Request::Show, cx);
        cx.notify();
    }

    /// The quit notice's Quit anyway: the person lets go of what was not
    /// saved.
    pub fn quit_anyway(&mut self, cx: &mut Context<Self>) {
        log::warn!("quitting with the chat list's last change not saved, as the person chose");
        cx.quit();
    }

    // -------------------------------------------------------- the person's

    pub fn set_board(&mut self, board: usize, cx: &mut Context<Self>) {
        if self.frozen {
            return;
        }
        self.state.set_board(board);
        self.focus_home();
        self.changed(cx);
    }

    pub fn next_board(&mut self, cx: &mut Context<Self>) {
        if self.frozen {
            return;
        }
        self.state.next_board();
        self.focus_home();
        self.changed(cx);
    }

    pub fn set_layout(&mut self, n: usize, cx: &mut Context<Self>) {
        if self.frozen {
            return;
        }
        self.state.set_layout(n);
        self.focus_home();
        self.changed(cx);
    }

    pub fn clear_board(&mut self, cx: &mut Context<Self>) {
        if self.frozen {
            return;
        }
        self.state.clear_board(self.state.board);
        self.changed(cx);
    }

    pub fn set_auto_tile(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.frozen {
            return;
        }
        self.state.auto_tile = on;
        self.changed(cx);
    }

    pub fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        if self.frozen {
            return;
        }
        self.state.sidebar.pinned = !self.state.sidebar.pinned;
        self.changed(cx);
    }

    pub fn toggle_folder(&mut self, folder_id: &str, cx: &mut Context<Self>) {
        if self.frozen {
            return;
        }
        self.state.toggle_folder(folder_id);
        self.changed(cx);
    }

    /// A row's click.
    pub fn show_chat(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.frozen {
            return;
        }
        self.state.show_chat(id);
        self.focus_home();
        self.changed(cx);
    }

    /// A notification's click, or the host's `ui://open-chat`.
    pub fn open_chat(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.frozen {
            return;
        }
        if !self.loaded {
            self.pending_open = Some(id.to_owned());
            return;
        }
        if self.state.find(id).is_none() {
            return;
        }
        // Its own board, so the sidebar lists it.
        let board = self.state.find(id).map_or(self.state.board, |c| c.board);
        self.state.set_board(board);
        self.state.open_chat(id);
        self.focus_request = Some(id.to_owned());
        self.changed(cx);
    }

    pub fn put(&mut self, pane: usize, id: &str, cx: &mut Context<Self>) {
        if self.frozen {
            return;
        }
        self.state.put(pane, id);
        self.focus_request = Some(id.to_owned());
        self.changed(cx);
    }

    pub fn swap(&mut self, a: usize, b: usize, cx: &mut Context<Self>) {
        if self.frozen {
            return;
        }
        self.state.swap(a, b);
        self.focus_home();
        self.changed(cx);
    }

    pub fn unseat(&mut self, pane: usize, cx: &mut Context<Self>) {
        if self.frozen {
            return;
        }
        self.state.unseat(pane);
        self.focus_home();
        self.changed(cx);
    }

    /// "To board" on the peek.
    pub fn to_board(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.frozen {
            return;
        }
        self.state.to_board(id);
        self.focus_request = Some(id.to_owned());
        self.changed(cx);
    }

    /// Esc, or the peek's Close: false when nothing was up.
    pub fn close_peek(&mut self, cx: &mut Context<Self>) -> bool {
        if self.frozen || self.state.peek.take().is_none() {
            return false;
        }
        self.focus_home();
        self.changed(cx);
        true
    }

    /// The pane a click or a key put the keyboard in. Not while the list is
    /// frozen for an update: the chat that is active is written with it.
    pub fn set_active_pane(&mut self, pane: usize, cx: &mut Context<Self>) {
        if !self.frozen && self.state.active_pane != pane {
            self.state.active_pane = pane;
            if let Some(id) = self.state.seat(pane) {
                self.state.active = Some(id.to_owned());
            }
            cx.notify();
        }
    }

    /// Ctrl+digit: false when it means nothing here, and the terminal takes
    /// the key.
    pub fn digit(&mut self, digit: usize, cx: &mut Context<Self>) -> bool {
        if self.frozen {
            // Taken, so it is not typed into the terminal either.
            return true;
        }
        let layout = self.state.layout();
        if digit == 0 || (layout > 1 && digit > layout) || (layout == 1 && self.state.numbered(self.state.board).len() < digit) {
            return false;
        }
        let chat = self.state.digit(digit);
        self.focus_request = chat;
        self.changed(cx);
        true
    }

    /// Ctrl+Shift+W. Not while the core is not up: no chat is known to wait
    /// then (`shown_status`).
    pub fn next_waiting(&mut self, cx: &mut Context<Self>) {
        if self.frozen || !self.core_up(cx) {
            return;
        }
        if let Some(id) = self.state.next_waiting() {
            self.focus_request = Some(id);
            self.changed(cx);
        }
    }

    /// A rename from the sidebar or a pane's menu: final.
    pub fn rename(&mut self, id: &str, name: &str, cx: &mut Context<Self>) {
        if self.frozen {
            return;
        }
        if self.state.set_name(id, name, NameSource::User) {
            self.changed(cx);
        }
    }

    /// A chat the new-chat dialog made, listed and seated where it will be
    /// seen (`WindowState::auto_place`).
    pub fn add_chat(&mut self, folder: &str, chat: Chat, by_person: bool, cx: &mut Context<Self>) -> Placed {
        let id = chat.id.clone();
        let n = self.state.folders.len();
        self.state.add_chat(folder, chat, by_person, || new_id("f", n));
        let placed = self.state.auto_place(&id);
        self.focus_request = Some(id);
        self.changed(cx);
        placed
    }

    /// A chat the delete dialog deleted: off every board and out of the list.
    pub fn remove_chat(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.state.delete_chat(id) {
            self.live.remove(id);
            self.ended.remove(id);
            self.changed(cx);
        }
    }

    /// The keyboard goes back to the terminal it belongs in.
    pub(crate) fn focus_home(&mut self) {
        self.focus_request = self.state.home_chat().map(str::to_owned);
    }

    // ------------------------------------------------------------- sessions

    /// The session a chat's pane starts or reattaches to, as the page's
    /// `startSession` asked for it; the reason when it cannot yet.
    pub fn spec_of(&self, id: &str) -> Result<SessionSpec, Why> {
        let chat = self.state.find(id).ok_or_else(|| "This chat is no longer listed.".to_string())?;
        let folder = self.state.folder_of(id).map(|f| f.path.clone()).unwrap_or_default();
        let Some(accounts) = &self.accounts else {
            return Err("Waiting for the list of accounts: the session starts once it has come.".into());
        };
        let Some(account) = accounts.iter().find(|a| a.provider == chat.provider && a.name == chat.account) else {
            return Err(format!(
                "The {} account \"{}\" is not one Luna knows, so the session cannot start.",
                provider_label(chat.provider),
                chat.account
            ));
        };
        Ok(spec(chat, &folder, &account.path))
    }

    /// The account path a chat runs on, when known.
    fn account_path(&self, chat: &Chat) -> Option<String> {
        self.accounts.as_ref()?.iter().find(|a| a.provider == chat.provider && a.name == chat.account).map(|a| a.path.clone())
    }

    fn core_up(&self, cx: &App) -> bool {
        self.core.read(cx).link().state == LinkState::Up
    }

    /// The status a row or a pane shows for `chat`: what it did last, while
    /// the core is up to say; `NoCore` while it is not, since whatever it was
    /// doing may have ended with the core.
    pub fn shown_status(&self, chat: &Chat, cx: &App) -> Status {
        if self.core_up(cx) { chat.status } else { Status::NoCore }
    }

    /// The session ended (`pty://exit`, of the generation its pane runs,
    /// models.rs): the chat rests, and a pane showing it again starts it
    /// anew. A look asked before it says nothing of the session now.
    pub fn on_exit(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.state.find(id).is_none() {
            return;
        }
        *self.exits.entry(id.to_owned()).or_default() += 1;
        self.announced.remove(id);
        self.ended.insert(id.to_owned());
        self.live.remove(id);
        if let Some(c) = self.state.find_mut(id)
            && c.status != Status::Resting
        {
            c.status = Status::Resting;
            self.changed(cx);
        } else {
            cx.notify();
        }
    }

    /// Whether `id`'s session ended since its pane started it; taken, so the
    /// next pane starts it anew.
    pub(crate) fn take_ended(&mut self, id: &str) -> bool {
        self.ended.remove(id)
    }

    /// A notification the core asks for (`event::NOTIFY`).
    pub fn on_notice(&mut self, title: String, body: String, chat_id: Option<String>) {
        self.notify(Notice { title, body, chat_id, sound: self.state.notify.sound });
    }

    fn notify(&self, notice: Notice) {
        if let Err(e) = self.host.notify(notice) {
            log::warn!("notify: {e}");
        }
    }

    /// One look at every chat with an account, unless one is under way or
    /// the core is not up. The answer is taken only for the core it was
    /// asked of, and for each chat only if its session did not exit since
    /// (`Fence`): an older one would put back a status, a draft or a session
    /// that is gone. A call that fails, or does not answer in
    /// `POLL_TIMEOUT`, says nothing: the next tick asks again.
    fn poll(&mut self, cx: &mut Context<Self>) {
        if !self.loaded || self.polling || !self.core_up(cx) {
            return;
        }
        let asked: Vec<(String, String)> =
            self.state.chats().filter_map(|c| self.account_path(c).map(|path| (c.id.clone(), path))).collect();
        // A chat deleted leaves the titles looked up.
        let listed: HashSet<&str> = self.state.chats().map(|c| c.id.as_str()).collect();
        self.title_looked.retain(|id| listed.contains(id.as_str()));
        if asked.is_empty() {
            return;
        }
        self.rounds += 1;
        self.polling = true;
        let fence = Fence { epoch: self.core.read(cx).epoch(), exits: self.exits.clone() };
        let chats: Vec<Value> = asked.iter().map(|(id, path)| json!({ "id": id, "accountPath": path })).collect();
        let answer = self.host.call(cmd::SESSIONS_META, json!({ "chats": chats }));
        cx.spawn(async move |this, cx| {
            let timeout = cx.background_executor().timer(POLL_TIMEOUT);
            let answer = match futures::future::select(answer, timeout).await {
                futures::future::Either::Left((answer, _)) => answer,
                futures::future::Either::Right(_) => Err(format!("no answer in {} s", POLL_TIMEOUT.as_secs())),
            };
            let looks = answer.and_then(|v| serde_json::from_value::<Vec<Look>>(v).map_err(|e| e.to_string()));
            let titles = this
                .update(cx, |this, cx| {
                    this.polling = false;
                    match looks {
                        Ok(looks) => this.apply(&asked, &fence, looks, cx),
                        Err(e) => {
                            log::warn!("sessions_meta: {e}");
                            Vec::new()
                        }
                    }
                })
                .unwrap_or_default();
            for (id, ask) in titles {
                let answer = this.update(cx, |this, _| this.host.call(cmd::SAVED_TITLE, ask));
                let Ok(answer) = answer else { return };
                if let Ok(Ok(saved)) = answer.await.map(serde_json::from_value::<Option<SavedTitle>>)
                    && let Some(saved) = saved
                {
                    let source = if saved.renamed { NameSource::Rename } else { NameSource::Cli };
                    let _ = this.update(cx, |this, cx| {
                        if this.state.set_name(&id, &saved.title, source) {
                            this.changed(cx);
                        }
                    });
                }
            }
        })
        .detach();
    }

    /// Each chat's look, all at once: the saved titles to look up after come
    /// back. Nothing of it is taken once the core it was asked of is gone,
    /// nor of a chat whose session exited since it was asked (`fence`).
    pub(crate) fn apply(
        &mut self,
        asked: &[(String, String)],
        fence: &Fence,
        looks: Vec<Look>,
        cx: &mut Context<Self>,
    ) -> Vec<(String, Value)> {
        if !self.core_up(cx) || self.core.read(cx).epoch() != fence.epoch {
            return Vec::new();
        }
        let mut by: HashMap<String, Meta> = looks.into_iter().filter_map(|l| Some((l.id, l.meta?))).collect();
        let (mut changed, mut redraw) = (false, false);
        let mut titles = Vec::new();
        for (id, path) in asked {
            let Some(chat) = self.state.find(id).cloned() else { continue };
            if self.exits.get(id) != fence.exits.get(id) {
                continue;
            }
            let Some(meta) = by.remove(id) else {
                // No live session: anything but resting would be a lie.
                self.announced.remove(id);
                if chat.status != Status::Resting {
                    self.state.find_mut(id).expect("found").status = Status::Resting;
                    changed = true;
                }
                redraw |= self.live.remove(id).is_some();
                if let Some(session) = chat.session_id.as_ref().filter(|_| chat.name_source != Some(NameSource::User))
                    && self.title_looked.insert(id.clone())
                {
                    let provider = if chat.provider == Provider::Claude { "claude" } else { "codex" };
                    titles.push((id.clone(), json!({ "provider": provider, "accountPath": path, "sessionId": session })));
                }
                continue;
            };
            // The CLI's own title, a rename or the one it made up; the
            // opening prompt only until that exists.
            if let Some(title) = meta.title.as_deref().filter(|t| !t.trim().is_empty()) {
                changed |= self.state.set_name(id, title, if meta.title_renamed { NameSource::Rename } else { NameSource::Cli });
            } else if let Some(prompt) = meta.first_prompt.as_deref() {
                changed |= self.state.set_name(id, prompt, NameSource::Prompt);
            }
            let live = Live { context: meta.context, draft: meta.draft, held: meta.held };
            if self.live.get(id) != Some(&live) {
                self.live.insert(id.clone(), live);
                redraw = true;
            }
            let next = Status::of(meta.status.as_deref());
            // A wait is told once, however its status was shown meanwhile.
            if next != Status::Waiting {
                self.announced.remove(id);
            } else if self.announced.insert(id.clone()) {
                self.notify(Notice {
                    title: format!("{} is waiting", provider_label(chat.provider)),
                    body: format!("\u{201c}{}\u{201d} needs your input", self.state.find(id).map_or(&chat.name, |c| &c.name)),
                    chat_id: Some(id.clone()),
                    sound: self.state.notify.sound,
                });
            }
            let c = self.state.find_mut(id).expect("found");
            if next != c.status {
                c.status = next;
                changed = true;
            }
            // Where the CLI's own worktree is, to resume in.
            if let Some(cwd) = meta.cwd.filter(|cwd| is_worktree(chat.provider, cwd) && c.worktree_path.as_ref() != Some(cwd)) {
                c.worktree_path = Some(cwd);
                changed = true;
            }
            if let Some(session) = meta.session_id.filter(|s| c.session_id.as_ref() != Some(s)) {
                c.session_id = Some(session);
                changed = true;
            }
            if c.provider == Provider::Codex
                && let Some(model) = meta.model.filter(|m| c.model_seen.as_ref() != Some(m))
            {
                c.model_seen = Some(model);
                changed = true;
            }
        }
        if changed {
            self.changed(cx);
        } else if redraw {
            cx.notify();
        }
        titles
    }
}

/// Writes what the state holds back, as the app quits (lib.rs).
pub fn flush(cx: &mut App) {
    if let Some(ws) = cx.try_global::<Current>().and_then(|c| c.0.upgrade()) {
        ws.update(cx, |w, cx| w.flush_now(cx));
    }
}

/// The app is to quit: once the workspace's state is written
/// (`Workspace::quit`), at once where there is no workspace.
pub fn quit(cx: &mut App) {
    match cx.try_global::<Current>().and_then(|c| c.0.upgrade()) {
        Some(ws) => ws.update(cx, |w, cx| w.quit(cx)),
        None => cx.quit(),
    }
}

/// An id for something new: `prefix`, the time and a count.
pub fn new_id(prefix: &str, n: usize) -> String {
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    format!("{prefix}{ms:x}{n:x}")
}

pub fn provider_label(p: Provider) -> &'static str {
    match p {
        Provider::Claude => "Claude Code",
        Provider::Codex => "Codex",
    }
}

/// `cwd` is a worktree the chat's CLI made: Claude Code's under
/// `.claude/worktrees/`, Codex's `worktrees/<4 hex>/<name>`.
fn is_worktree(provider: Provider, cwd: &str) -> bool {
    let parts: Vec<String> = cwd.split(['/', '\\']).filter(|p| !p.is_empty()).map(str::to_lowercase).collect();
    match provider {
        Provider::Claude => {
            parts.windows(2).any(|w| w[0] == ".claude" && w[1] == "worktrees") && parts.last().is_some_and(|l| l != "worktrees")
        }
        Provider::Codex => {
            let n = parts.len();
            n >= 3 && parts[n - 3] == "worktrees" && parts[n - 2].len() == 4 && parts[n - 2].chars().all(|c| c.is_ascii_hexdigit())
        }
    }
}

/// The name `claude --worktree <name>` knows `path` by, when it is a
/// worktree Claude Code made right in `folder`.
fn claude_worktree_name(folder: &str, path: Option<&str>) -> Option<String> {
    let path = path?.trim_end_matches(['/', '\\']);
    let (root, name) = path.rsplit_once(['/', '\\'])?;
    let root = root.strip_suffix("worktrees")?.trim_end_matches(['/', '\\']);
    let root = root.strip_suffix(".claude")?.trim_end_matches(['/', '\\']);
    let ok = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
    let norm = |p: &str| p.replace('/', "\\").trim_end_matches('\\').to_lowercase();
    (ok && norm(root) == norm(folder)).then(|| name.to_owned())
}

/// Claude Code's permission mode as the CLI takes it, from Luna's label.
fn perm_cli(perm: Option<&str>) -> &'static str {
    match perm.unwrap_or("Ask") {
        "Edits" => "acceptEdits",
        "Plan only" => "plan",
        "Auto" => "auto",
        "Bypass" => "bypassPermissions",
        _ => "default",
    }
}

/// The session spec of a chat in `folder` on the account at `account`: the
/// page's `startSession`, without the handoff 0.20 cut.
pub fn spec(chat: &Chat, folder: &str, account: &str) -> SessionSpec {
    let wt_path = chat.worktree_path.as_deref().filter(|p| !p.is_empty());
    match chat.provider {
        Provider::Claude => {
            let name = if chat.worktree { claude_worktree_name(folder, wt_path) } else { None };
            let model = chat.model.as_deref().unwrap_or("").to_lowercase().split(' ').next().unwrap_or("").to_owned();
            SessionSpec {
                id: chat.id.clone(),
                provider: Provider::Claude,
                start: cmd::ENSURE_CLAUDE_SESSION,
                args: json!({
                    "id": chat.id,
                    "folder": if name.is_some() { folder } else { wt_path.unwrap_or(folder) },
                    "accountPath": account,
                    "model": model,
                    "effort": chat.effort,
                    "permissionMode": perm_cli(chat.perm.as_deref()),
                    "worktree": chat.worktree && (wt_path.is_none() || name.is_some()),
                    "worktreeName": name,
                    "resume": chat.session_id,
                    "tools": chat.tools,
                    "readOnly": chat.read_only,
                    "agent": chat.agent,
                }),
            }
        }
        Provider::Codex => SessionSpec {
            id: chat.id.clone(),
            provider: Provider::Codex,
            start: cmd::ENSURE_CODEX_SESSION,
            args: json!({
                "id": chat.id,
                "folder": wt_path.unwrap_or(folder),
                "worktree": chat.worktree && wt_path.is_none(),
                "accountPath": account,
                "model": chat.model,
                "effort": chat.effort,
                "approval": chat.approval.as_deref().unwrap_or("on-request"),
                "sandbox": chat.sandbox.as_deref().unwrap_or("read-only"),
                "resume": chat.session_id,
                "login": false,
                "tools": chat.tools,
            }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktrees_are_told_by_each_clis_own_layout() {
        assert!(is_worktree(Provider::Claude, "C:\\repo\\.claude\\worktrees\\fix"));
        assert!(is_worktree(Provider::Claude, "/repo/.Claude/worktrees/fix/"));
        assert!(!is_worktree(Provider::Claude, "/repo/src"));
        assert!(is_worktree(Provider::Codex, "C:\\acct\\worktrees\\1a2b\\repo"));
        assert!(!is_worktree(Provider::Codex, "C:\\acct\\worktrees\\zzzz\\repo"));
        assert_eq!(claude_worktree_name("C:\\repo", Some("C:/repo/.claude/worktrees/fix-1")).as_deref(), Some("fix-1"));
        assert_eq!(claude_worktree_name("C:\\other", Some("C:/repo/.claude/worktrees/fix-1")), None);
        assert_eq!(claude_worktree_name("C:\\repo", Some("C:/repo/src")), None);
    }

    #[test]
    fn drafts_read_as_the_page_tagged_them() {
        assert_eq!(Live { draft: true, ..Live::default() }.draft_tag().as_deref(), Some("draft"));
        assert_eq!(Live { draft: true, held: 2, ..Live::default() }.draft_tag().as_deref(), Some("draft · 2"));
        assert_eq!(Live { held: 1, ..Live::default() }.draft_tag().as_deref(), Some("1 held"));
        assert_eq!(Live::default().draft_tag(), None);
    }
}
