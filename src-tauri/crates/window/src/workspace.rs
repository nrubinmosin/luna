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
//!   No timer runs while no chat can be looked at; hidden to the tray for
//!   `HIDDEN_AFTER`, the looks come every `HIDDEN_EVERY`, and one comes at
//!   once as the window is shown again.
//! - The core is told every chat's name and board (`chat_names`, the page's
//!   bridge), which `luna_session_list` shows: once the list is read, when
//!   one changes and for each new core.
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
use crate::host::{Answer, Host, cmd};
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
/// How long the window is hidden to the tray before the looks slow to
/// `HIDDEN_EVERY`, as Chromium slowed the hidden page's timers to once a
/// minute after five minutes (its intensive wake-up throttling).
pub const HIDDEN_AFTER: Duration = Duration::from_secs(5 * 60);
pub const HIDDEN_EVERY: Duration = Duration::from_secs(60);
/// How long a look waits for its answer: past it, the round is given up and
/// the next tick asks again.
pub const POLL_TIMEOUT: Duration = Duration::from_secs(20);
/// The least time between two writes of the state.
pub const GAP: Duration = Duration::from_millis(500);
/// How many times a quit asks the host to flush again, because the list
/// changed while it did.
const QUIT_FLUSHES: usize = 5;
/// How long an update's last write of the list waits for what was under way
/// as the list froze (a round of looks, a rename, a helper being opened as
/// a chat) to land in it (`Workspace::settle`): past a round's own
/// `POLL_TIMEOUT` and the retries a promotion holds it through
/// (dialogs/helpers.rs `SETTLE_HELD`).
pub const SETTLE_LIMIT: Duration = Duration::from_secs(25);
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
    /// Its name: `quit`, `read`, `import`, `kept`, `aside`, `save` or `gpu`, which
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

/// Why a change to the chat list is refused while an update installs.
pub const FROZEN: &str = "an update is being installed: the chat list takes no changes until it is installed or called off";

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

/// The import took 0.19's theme, none having been kept here
/// (`import_prefs`): the window shows it (models.rs `Shell`).
pub struct ThemeTaken(pub crate::dto::ThemePref);

impl gpui::EventEmitter<ThemeTaken> for Workspace {}

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
    /// The preferences were taken from the host or Settings (`hold_prefs`):
    /// an answer to the first ask, come after, is older.
    prefs_taken: bool,
    /// Settings' changes of the preferences (`take_prefs`): the import's
    /// answer, asked before one, is older.
    prefs_changes: u64,
    /// 0.19's choices are with the host (`import_prefs`): Settings waits for
    /// its answer (`settled_prefs`).
    prefs_importing: bool,
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
    /// What the core's lifecycle record says each session does, by chat id
    /// (`set_lives`, from the power state): where it has one it is the status
    /// shown, as the counts have it at once; `sessions_meta` says the same
    /// from the same record, a round later.
    lives: HashMap<String, Status>,
    notices: FileNotices,
    /// The background helpers as the core last listed them: what a pane's
    /// chip says (`helper_chip`).
    helpers: Vec<crate::dialogs::rows::Helper>,
    /// Lists of them heard as events: an answer to `background_helpers`
    /// asked before the last one is older than it, and is dropped.
    helpers_heard: u64,
    /// Helpers this window is opening as chats right now
    /// (dialogs/helpers.rs `open_as_chat`, `settle_openings`): one opening
    /// of each at a time, which the settling as a core comes up leaves be.
    opening: HashSet<String>,
    /// A core came up, at the window's start or since: the openings a
    /// window or a core gone meanwhile left unsettled are settled from the
    /// first list of helpers it gives (`settle_openings`), once the chat
    /// list is read; `helpers_listed` once one has come.
    openings_due: bool,
    helpers_listed: bool,
    /// The host's word that G-SYNC is not off for Luna's exe in NVIDIA's
    /// driver profiles (`desktop::event::GPU_HINT`), until dismissed.
    gpu_hint: Option<String>,
    /// Why the orphan line's last Adopt listed nothing (`say_orphan`), until
    /// dismissed.
    orphan_note: Option<String>,
    /// The core was up when last looked at.
    core_was_up: bool,
    /// The core's epoch the names were last told to, and what was told
    /// (`tell_names`): None when it is to be told again.
    names_epoch: u64,
    names_told: Option<String>,
    /// Since when the window is hidden to the tray (`desktop::Shown`).
    hidden_since: Option<Instant>,
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
    /// The list's last write for an update is acknowledged
    /// (`save_for_install`): what changes after it, the core's looks
    /// included, is kept in memory only, so that nothing is written after
    /// the drain, until the update is called off (`thaw`).
    sealed: bool,
    /// Work let through before the list froze that changes it once it
    /// ends: a rename (`rename_account_on_host`), a helper opened as a chat
    /// (`admit`). The last write waits for it (`settle`).
    admitted: usize,
    /// Rounds whose saved titles are being looked up (`poll`).
    titles: usize,
    /// What waits for the work under way to end (`settle`), and its bound.
    settling: Vec<Then>,
    _settle_limit: Option<Task<()>>,
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
        // A core that comes up is asked at once, not at the next tick, and
        // told the chats' names; one that comes or goes changes what every
        // row shows (`shown_status`).
        cx.observe(&core, |this, core, cx| {
            let (up, epoch) = (core.read(cx).link().state == LinkState::Up, core.read(cx).epoch());
            let came_up = up && (!this.core_was_up || epoch != this.names_epoch);
            if up != this.core_was_up {
                this.core_was_up = up;
                cx.notify();
            }
            if up && this.accounts.is_none() {
                this.reload_accounts(cx);
            }
            if came_up {
                if epoch != this.names_epoch {
                    this.names_epoch = epoch;
                    this.names_told = None;
                }
                this.tell_names(cx);
                this.poll(cx);
                // Found again or new: what it was told of an opening may
                // never have reached it, and is told again.
                this.openings_due = true;
                this.helpers_listed = false;
                this.load_helpers(cx);
            }
        })
        .detach();
        // Hidden to the tray the looks slow down; shown, one comes at once.
        cx.observe_global::<crate::desktop::Shown>(|this, cx| {
            let hidden = cx.global::<crate::desktop::Shown>().hidden;
            this.set_hidden(hidden, cx);
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
            prefs_changes: 0,
            prefs_importing: false,
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
            lives: HashMap::new(),
            notices: FileNotices::default(),
            gpu_hint: None,
            orphan_note: None,
            helpers: Vec::new(),
            helpers_heard: 0,
            opening: HashSet::new(),
            openings_due: false,
            helpers_listed: false,
            core_was_up: false,
            names_epoch: 0,
            names_told: None,
            hidden_since: None,
            read_id: 0,
            reading: false,
            edits: 0,
            quitting: false,
            frozen: false,
            sealed: false,
            admitted: 0,
            titles: 0,
            settling: Vec::new(),
            _settle_limit: None,
            accounts_loading: false,
            accounts_again: false,
            accounts_wait: ACCOUNTS_RETRY_FIRST,
            _accounts_retry: None,
            _poll: None,
        }
    }

    /// Asks the host for the state and the accounts; the sessions are
    /// looked at once both have come and the core is up (`poll`).
    pub fn load(&mut self, cx: &mut Context<Self>) {
        self.read_state(cx);
        self.reload_accounts(cx);
        self.load_prefs(cx);
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
        let store = |key: &str| answer.get(key).and_then(|o| serde_json::from_value::<HashMap<String, String>>(o.clone()).ok());
        let (mut state, write) = match saved {
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
                // Written, so a later start does not look again; a store with
                // no list may still have choices (its theme, say).
                Import::Nothing => {
                    let prefs = store("old").map(|old| crate::import::prefs_of(&old)).filter(crate::import::OldPrefs::any);
                    (WindowState { prefs_to_import: prefs, ..WindowState::default() }, true)
                }
            },
        };
        // A list 0.20.0 imported 0.19's into, with the store read again for
        // what 0.20.0 did not keep of its choices (`reads_old_prefs_again`).
        if let Some(old) = store("oldPrefs") {
            let kept = state.prefs_to_import.take().unwrap_or_default();
            state.prefs_to_import = Some(crate::import::prefs_of(&old).or(kept)).filter(crate::import::OldPrefs::any);
        }
        // What was done before the state came (a board chosen) gives way to
        // it: nothing of it was kept.
        self.state = state;
        // The host keeps the move's mark beside the list too, where 0.20.0,
        // which rewrites the list without it, does not reach.
        if answer.get("codexBypassMoved") == Some(&Value::Bool(true)) {
            self.state.codex_bypass_moved = true;
        }
        let write = match self.state.move_codex_bypass() {
            Some(moved) => {
                for (id, name) in moved {
                    log::warn!("{}", WindowState::codex_bypass_moved_line(&id, &name));
                }
                true
            }
            None => write,
        };
        // A chat kept as waiting was told of in the run that saw it begin.
        self.announced = self.state.chats().filter(|c| c.status == Status::Waiting).map(|c| c.id.clone()).collect();
        self.loaded = true;
        if let Some(id) = self.pending_open.take() {
            self.open_chat(&id, cx);
        }
        if write {
            self.write(cx);
        }
        self.import_prefs(cx);
        cx.notify();
        self.tell_names(cx);
        self.poll(cx);
        self.settle_openings(cx);
    }

    /// 0.19's chat list from the host's read of its store. No store at all
    /// (a machine 0.19 never ran on), or one that kept no chat list, is
    /// nothing to import and says nothing. What the import left out is said
    /// as for the window's own file, naming the copy the host kept of the
    /// list as it was (`oldKept`).
    fn import(&mut self, answer: &Value) -> Import {
        use crate::import::NotRead;
        let old = answer.get("old").and_then(|o| serde_json::from_value::<HashMap<String, String>>(o.clone()).ok());
        let error = answer.get("oldError").and_then(Value::as_str).map(str::to_owned);
        let why = match (old, error) {
            (Some(old), _) => match crate::import::from_storage(&old) {
                Ok(Some((state, found))) => {
                    log::warn!("imported 0.19's chat list: {} chats in {} folders, {} groups", found.chats, found.folders, found.groups);
                    if found.lost.any() {
                        let what = found.lost.words();
                        log::warn!("0.19's chat list did not all read: {what}");
                        let kept = answer.get("oldKept").and_then(Value::as_str).map(str::to_owned);
                        self.notices.kept = Some((what, kept.unwrap_or_else(|| "nowhere: the host kept no copy".into())));
                    }
                    return Import::Done(state);
                }
                Ok(None) => return Import::Nothing,
                Err(NotRead::Older(version)) => {
                    log::warn!("0.19's chat list was not imported: it is of version {version}, older than this Luna reads");
                    return Import::Failed(crate::import::older(version));
                }
                Err(NotRead::Broken(why)) => why,
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
        if imported.older {
            return Some("The chat list from an older Luna could not be imported; the list starts empty.".into());
        }
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
        // A read while frozen would replace the list the update's last write
        // is to hold: Retry is back once the update is installed or off.
        let can_read = !self.frozen;
        if let Some(why) = &self.notices.read_error {
            out.push(n(
                "read",
                format!("The chat list could not be read ({why}). Nothing is saved over it until it is read: Retry reads it again."),
                can_read,
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
            // Retry cannot help a list too old to read.
            let older = self.state.imported.as_ref().is_some_and(|i| i.older);
            out.push(n("import", text, can_read && !older, true));
        }
        if let Some(text) = &self.gpu_hint {
            out.push(n("gpu", text.clone(), false, true));
        }
        if let Some(text) = &self.orphan_note {
            out.push(n("orphan", text.clone(), false, true));
        }
        out
    }

    /// The core's list of background helpers (`event::HELPERS`), stamped
    /// with the core that sent it (None: a host that does not stamp).
    pub fn on_helpers(&mut self, rows: Vec<crate::dialogs::rows::Helper>, core: Option<u64>, cx: &mut Context<Self>) {
        if core.is_some_and(|c| !self.of_this_core(Some(c), cx)) {
            return log::info!("a list of helpers from a core since replaced dropped");
        }
        self.helpers_heard += 1;
        self.set_helpers(rows, cx);
        self.helpers_listed = true;
        self.settle_openings(cx);
    }

    /// Whether a list of helpers stamped with, or asked of, the core whose
    /// `up` was `core` is the current core's. Only that one's counts, as the
    /// list `held_by_opening` goes by and `settle_openings` settles from: an
    /// old core's, late, would omit the helpers its replacement restored, and
    /// Delete would take one's row away.
    fn of_this_core(&self, core: Option<u64>, cx: &App) -> bool {
        self.core_up(cx) && self.core.read(cx).up_revision() == core
    }

    /// The link revision of the current core's `up`, to stamp a question
    /// about its helpers with (`of_this_core`).
    pub fn this_core(&self, cx: &App) -> Option<u64> {
        self.core.read(cx).up_revision()
    }

    fn set_helpers(&mut self, rows: Vec<crate::dialogs::rows::Helper>, cx: &mut Context<Self>) {
        if rows != self.helpers {
            self.helpers = rows;
            cx.notify();
        }
    }

    /// The helpers as they stand when the core comes up: the event comes
    /// only with a change.
    fn load_helpers(&mut self, cx: &mut Context<Self>) {
        let (heard, asked_of) = (self.helpers_heard, self.this_core(cx));
        let answer = self.host.call(crate::dialogs::cmd::BACKGROUND_HELPERS, json!({}));
        cx.spawn(async move |this, cx| {
            let rows = answer.await.and_then(|v| serde_json::from_value::<Vec<crate::dialogs::rows::Helper>>(v).map_err(|e| e.to_string()));
            let _ = this.update(cx, |this, cx| match rows {
                // Asked of a core since replaced: the new one is asked itself.
                Ok(_) if !this.of_this_core(asked_of, cx) => {}
                Ok(rows) => {
                    if this.helpers_heard == heard {
                        this.set_helpers(rows, cx);
                    }
                    this.helpers_listed = true;
                    this.settle_openings(cx);
                }
                Err(e) => log::warn!("background_helpers: {e}"),
            });
        })
        .detach();
    }

    /// Begins opening helper `id` as a chat: false while it is being opened
    /// already, by a click or by `settle_openings`.
    pub fn begin_opening(&mut self, id: &str) -> bool {
        self.opening.insert(id.to_owned())
    }

    /// Its opening is over: the helpers popover, which observes this, shows
    /// its Open as chat again.
    pub fn end_opening(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.opening.remove(id) {
            cx.notify();
        }
    }

    pub fn is_opening(&self, id: &str) -> bool {
        self.opening.contains(id)
    }

    /// Row `id` is a helper's being opened as a chat: by this window now, or
    /// still held by the core for an opening. Not deleted meanwhile: the
    /// session's delete would clear the helper's registry and token and
    /// leave the helper (pty/control.rs), and a retry could hand it to a
    /// row that is gone.
    /// The current core has listed its helpers since it came up: until then
    /// `held_by_opening` cannot tell.
    pub fn helpers_known(&self) -> bool {
        self.helpers_listed
    }

    pub fn held_by_opening(&self, id: &str) -> bool {
        self.opening.contains(id) || self.helpers.iter().any(|h| h.id == id)
    }

    /// Answers once the host says the list as it stands now is written
    /// (`flush_acked`): what an opening waits for before it tells the core
    /// its row is made, so a window gone after that word leaves the row on
    /// disk for the next to find.
    pub fn written(&mut self, cx: &mut Context<Self>) -> futures::channel::oneshot::Receiver<Result<(), String>> {
        let (tx, rx) = futures::channel::oneshot::channel();
        self.flush_acked(
            QUIT_FLUSHES,
            Box::new(move |_: &mut Workspace, wrote: Result<(), String>, _: &mut Context<Workspace>| {
                let _ = tx.send(wrote);
            }),
            cx,
        );
        rx
    }

    /// Openings a window that went away, or a core that did, left
    /// unsettled, settled by what this list holds, as 0.19's bridge did:
    /// its row made, the core is told so again and hands the helper over,
    /// and the row is seated; none, it goes on as its agent's helper. A row
    /// with a helper's id is made by nothing but an opening, so one whose
    /// opening the core has given up (PROMOTE_TIMEOUT_MS gave it back to its
    /// agent) is let go of. The core's word decides each, so a list a little
    /// behind it settles nothing wrongly; until the core lets go, a pane
    /// cannot start the row (pty/start.rs `not_a_helper`).
    ///
    /// As a core comes up (`openings_due`) all of them; at any later list,
    /// the rows made alone, so a row every word of whose opening went
    /// unanswered is not left listed and unstartable until a restart, nor
    /// one whose helper the core gave back meanwhile. Held off while frozen,
    /// until the update is called off (`thaw`).
    fn settle_openings(&mut self, cx: &mut Context<Self>) {
        if !self.helpers_listed || !self.loaded || self.frozen || !self.core_up(cx) {
            return;
        }
        let full = std::mem::take(&mut self.openings_due);
        let ws = cx.entity();
        for h in self.helpers.clone() {
            let made = self.state.find(&h.id).is_some();
            if !(made || (full && h.promoting)) || !self.begin_opening(&h.id) {
                continue;
            }
            // Work an update's last write of the list waits for (`settle`),
            // bounded as a click's held tries are; what is still unsettled then
            // is left to a later list.
            self.admitted += 1;
            let (host, ws) = (self.host.clone(), ws.clone());
            cx.spawn(async move |this, cx| {
                use crate::dialogs::helpers::{SETTLE_HELD, SETTLE_RETRY, bounded, settle};
                let handed = if made {
                    settle(&host, &ws, &h.id, &SETTLE_RETRY[..SETTLE_HELD], cx).await
                } else {
                    let why = "the window that was opening it as a chat went away";
                    let told = bounded(cx, host.call(crate::dialogs::cmd::BACKGROUND_PROMOTED, json!({ "id": h.id, "error": why }))).await;
                    if let Err(e) = told {
                        log::warn!("settling the opening of helper {} failed: {e}", h.id);
                    }
                    None
                };
                let _ = this.update(cx, |this, cx| {
                    this.end_opening(&h.id, cx);
                    this.opening_settled(&h.id, handed, cx);
                    this.release(cx);
                });
            })
            .detach();
        }
    }

    /// The helpers popover's own look at the list (`LOOK_EVERY`): kept as
    /// the list, and what is left of an opening settled by it.
    /// `asked_of`: the core it was asked of (`this_core`), whose list alone counts.
    pub fn on_polled_helpers(&mut self, rows: Vec<crate::dialogs::rows::Helper>, asked_of: Option<u64>, cx: &mut Context<Self>) {
        if !self.of_this_core(asked_of, cx) {
            return;
        }
        self.set_helpers(rows, cx);
        self.helpers_listed = true;
        self.settle_openings(cx);
    }

    /// What the core said of an opening whose row is made: the person's
    /// (`Some(true)`), the row seated; its agent's again (`Some(false)`),
    /// the row let go of, as it would only run the conversation beside the
    /// helper; nothing heard (None), the row stays listed for the next
    /// core to settle. Once an update's last write is made (`sealed`), what
    /// is on disk is what the next start settles: nothing changes here.
    pub fn opening_settled(&mut self, id: &str, handed: Option<bool>, cx: &mut Context<Self>) {
        if self.sealed {
            return log::warn!("helper {id}'s opening settled after the update's last write; the next start settles its row");
        }
        match handed {
            Some(true) => {
                self.place_chat(id, cx);
            }
            Some(false) => self.unlist_promoted(id, cx),
            None => {}
        }
    }

    /// What chat `id`'s pane says of the helpers under it, in a few
    /// characters where it has no room (`compact`); None with none.
    pub fn helper_chip(&self, id: &str, compact: bool) -> Option<String> {
        crate::dialogs::rows::chip(&crate::dialogs::rows::helpers_under(&self.helpers, id), compact)
    }

    /// A start of `id`'s pane has answered, attached or started: an adopted
    /// orphan's next start is any chat's (`Chat::adopt`).
    pub(crate) fn start_answered(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.state.find_mut(id).and_then(|c| c.adopt.take()).is_some() {
            self.changed(cx);
        }
    }

    /// An adopted orphan's first start was refused: the session it was
    /// adopted for is gone, and nothing was started. The chat rests, as one
    /// whose session ended does (`on_exit`), so the pane that shows it next
    /// is a new one, whose start, the person's, is any chat's.
    pub(crate) fn adopt_refused(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.state.find(id).is_none() {
            return;
        }
        self.ended.insert(id.to_owned());
        self.live.remove(id);
        if let Some(c) = self.state.find_mut(id) {
            c.adopt = None;
            c.status = Status::Resting;
        }
        self.changed(cx);
    }

    /// Why an Adopt of the orphan line's listed nothing: a line among the
    /// notices until its OK.
    pub fn say_orphan(&mut self, text: String, cx: &mut Context<Self>) {
        self.orphan_note = Some(text);
        cx.notify();
    }

    /// The account an orphan runs on, by its path, among those Luna lists
    /// for its CLI: the one its chat's pane can start on. None where none
    /// is, or the list has not come.
    pub fn orphan_account(&self, o: &crate::status::dto::Orphan) -> Option<&Account> {
        let provider = o.provider?;
        self.accounts()?.iter().find(|a| a.provider == provider && crate::state::same_path(&a.path, &o.account_path))
    }

    /// The host's G-SYNC hint (`desktop::event::GPU_HINT`): a line among the
    /// notices until its OK.
    pub fn on_gpu_hint(&mut self, text: String, cx: &mut Context<Self>) {
        self.gpu_hint = Some(text);
        cx.notify();
    }

    /// A notice's OK.
    pub fn dismiss(&mut self, key: &str, cx: &mut Context<Self>) {
        match key {
            "kept" => self.notices.kept = None,
            "aside" => self.notices.aside = None,
            "gpu" => self.gpu_hint = None,
            "orphan" => self.orphan_note = None,
            "import" => return self.dismiss_notice(cx),
            _ => return,
        }
        cx.notify();
    }

    /// A notice's Retry: the file read again, or the import tried again;
    /// neither while frozen for an update (`notices` offers no Retry then).
    pub fn retry(&mut self, key: &str, cx: &mut Context<Self>) {
        match key {
            "quit" => self.quit(cx),
            "read" | "import" if self.frozen => log::warn!("the chat list is not read again: {FROZEN}"),
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

    /// Why a chat cannot be made, nor an account renamed or deleted, yet:
    /// the list is still being read (an import of 0.19's can take seconds),
    /// and what was done before it came would be replaced by it. None once
    /// it has come.
    pub fn list_wait(&self) -> Option<String> {
        if self.loaded {
            return None;
        }
        Some(match &self.notices.read_error {
            Some(_) => "The chat list could not be read: Retry above the board first.".to_owned(),
            None => "Loading chats...".to_owned(),
        })
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

    /// The preferences for Settings to show and change: none until the
    /// host's are here, the list has come (with it, whether 0.19's choices
    /// are to go to the host) and the host has taken those. A change made
    /// from them is never one made over what the import is still writing.
    pub fn settled_prefs(&self) -> Option<&Prefs> {
        let listed = self.loaded || self.notices.read_error.is_some();
        (self.prefs_taken && listed && !self.prefs_importing).then_some(&self.prefs)
    }

    /// The host's preferences as Settings read them, as the first ask's
    /// answer is taken: where none was taken yet (that ask failed, say).
    pub fn offer_prefs(&mut self, prefs: Prefs, cx: &mut Context<Self>) {
        if !self.prefs_taken {
            self.hold_prefs(prefs, cx);
        }
    }

    /// Settings changed the preferences: the next dialog starts from these.
    pub fn take_prefs(&mut self, prefs: Prefs, cx: &mut Context<Self>) {
        self.prefs_changes += 1;
        self.hold_prefs(prefs, cx);
    }

    fn hold_prefs(&mut self, prefs: Prefs, cx: &mut Context<Self>) {
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
                Ok(prefs) if !this.prefs_taken => this.hold_prefs(prefs, cx),
                Ok(_) => {}
                Err(e) => log::warn!("{}: {e}", panels_cmd::WINDOW_PREFS),
            });
        })
        .detach();
    }

    /// 0.19's choices the list holds (`WindowState::prefs_to_import`) to
    /// the host, which keeps each where Settings does unless one was made
    /// here already (`IMPORT_WINDOW_PREFS`). Once it has them written, they
    /// leave the list, which is written then; until then they stay in it, so
    /// a write that failed, or a quit before the answer, has the next start
    /// hand them over again. The answer, the preferences as they then are,
    /// is taken unless Settings changed them meanwhile, and a theme it took
    /// is told (`ThemeTaken`).
    fn import_prefs(&mut self, cx: &mut Context<Self>) {
        #[derive(serde::Deserialize)]
        struct Taken {
            prefs: Prefs,
            #[serde(default)]
            theme: Option<crate::dto::ThemePref>,
        }
        let Some(old) = self.state.prefs_to_import.clone().filter(|_| !self.prefs_importing) else { return };
        self.prefs_importing = true;
        let changes = self.prefs_changes;
        let answer = self.host.call(panels_cmd::IMPORT_WINDOW_PREFS, json!(old));
        cx.spawn(async move |this, cx| {
            let taken = answer.await.and_then(|v| serde_json::from_value::<Taken>(v).map_err(|e| e.to_string()));
            let _ = this.update(cx, |this, cx| {
                this.prefs_importing = false;
                match taken {
                    Ok(taken) => {
                        if this.prefs_changes == changes {
                            this.hold_prefs(taken.prefs, cx);
                        }
                        if let Some(theme) = taken.theme {
                            cx.emit(ThemeTaken(theme));
                        }
                        if this.state.prefs_to_import.as_ref() == Some(&old) {
                            this.state.prefs_to_import = None;
                            this.changed(cx);
                        }
                    }
                    Err(e) => {
                        log::warn!("{}: {e}; 0.19's choices are handed over again at the next start", panels_cmd::IMPORT_WINDOW_PREFS)
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Renames an account's folder (the accounts window's `rename_account`)
    /// and, once the host has, has the chats on it follow: here, so they do
    /// whether or not the window that asked is still open. The answer is
    /// told back for that window to show. Refused while an update installs:
    /// the chats on it could not follow. One let through before the list
    /// froze still has them follow, and the list's last write waits for it
    /// (`settle`).
    pub fn rename_account_on_host(&mut self, provider: Provider, from: &str, to: &str, cx: &mut Context<Self>) -> Answer {
        if let Some(why) = self.list_wait().or_else(|| self.unfrozen().err()) {
            return Box::pin(async move { Err(why) });
        }
        self.admitted += 1;
        let (from, to) = (from.to_owned(), to.to_owned());
        let answer = self.host.call(panels_cmd::RENAME_ACCOUNT, json!({ "provider": provider, "name": from, "to": to }));
        let (tx, rx) = futures::channel::oneshot::channel();
        cx.spawn(async move |this, cx| {
            let got = answer.await;
            let _ = this.update(cx, |w, cx| {
                if got.is_ok() {
                    w.rename_account(provider, &from, &to, cx);
                }
                w.release(cx);
            });
            let _ = tx.send(got);
        })
        .detach();
        Box::pin(async move { rx.await.unwrap_or_else(|_| Err("no answer".into())) })
    }

    /// Lets through work that changes the list once it ends (a helper opened
    /// as a chat): refused while frozen for an update; one let through
    /// before is waited for by the list's last write (`settle`) until it is
    /// `release`d.
    pub fn admit(&mut self) -> Result<(), String> {
        self.unfrozen()?;
        self.admitted += 1;
        Ok(())
    }

    /// Work let through (`admit`) has ended, its change made.
    pub fn release(&mut self, cx: &mut Context<Self>) {
        self.admitted = self.admitted.saturating_sub(1);
        self.check_settled(cx);
    }

    /// An account made or deleted (the accounts window): once the host has,
    /// the accounts are listed again here, whether or not that window is
    /// still open.
    pub fn accounts_changed_by(&mut self, answer: Answer, cx: &mut Context<Self>) -> Answer {
        self.then_here(answer, cx, |w, cx| w.reload_accounts(cx))
    }

    /// `answer`, with `then` run on the workspace once it is a success, on a
    /// task of the workspace's own: what asked may be gone by then.
    fn then_here(&mut self, answer: Answer, cx: &mut Context<Self>, then: impl FnOnce(&mut Self, &mut Context<Self>) + 'static) -> Answer {
        let (tx, rx) = futures::channel::oneshot::channel();
        cx.spawn(async move |this, cx| {
            let got = answer.await;
            if got.is_ok() {
                let _ = this.update(cx, then);
            }
            let _ = tx.send(got);
        })
        .detach();
        Box::pin(async move { rx.await.unwrap_or_else(|_| Err("no answer".into())) })
    }

    /// An account was renamed: the chats on it follow, and the accounts are
    /// listed again. Frozen or not: the rename was let through before the
    /// list froze, and its last write waits for this (`settle`).
    fn rename_account(&mut self, provider: Provider, from: &str, to: &str, cx: &mut Context<Self>) {
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
        // A chat added, renamed or moved: the core is told, and its session
        // is looked at from now on.
        self.tell_names(cx);
        self.schedule(cx);
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

    /// Writes the state now, unless it is the text written last, or the
    /// update's last write is made (`sealed`).
    fn write(&mut self, cx: &mut Context<Self>) {
        self.saver.waiting = None;
        if self.sealed {
            return;
        }
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

    /// The update is off: the list takes changes again, and what changed
    /// since its last write (the core's looks) is written, as is a place of
    /// the window's held back.
    pub fn thaw(&mut self, cx: &mut Context<Self>) {
        crate::desktop::unseal_places(cx);
        if std::mem::take(&mut self.sealed) && self.loaded {
            self.write(cx);
        }
        if std::mem::take(&mut self.frozen) {
            cx.notify();
        }
        // What a core come up during the freeze left to settle.
        self.settle_openings(cx);
    }

    /// Whether the update's last write is made, for the tests (the one
    /// that asks runs in debug builds alone, as the release-built Windows
    /// tests leave it out).
    #[cfg(all(test, debug_assertions))]
    pub(crate) fn sealed(&self) -> bool {
        self.sealed
    }

    pub fn frozen(&self) -> bool {
        self.frozen
    }

    /// Ok unless an update is being installed: what a dialog asks before it
    /// does anything a chat-list change would follow (a chat made, deleted,
    /// promoted, an account renamed), so nothing is done that the list could
    /// not then record.
    pub fn unfrozen(&self) -> Result<(), String> {
        if self.frozen { Err(FROZEN.into()) } else { Ok(()) }
    }

    /// The list's last write for an install, made once the sessions are
    /// stopped or answered for (the status bar's `proceed`): first what was
    /// under way as the list froze lands in it (`settle`), then it is
    /// written by the quit's rule (`flush_acked`), and from the host's yes
    /// on nothing more is written (`sealed`): the core's looks still show,
    /// in memory only, until the update is called off. `then` is told Ok,
    /// or why not. With nothing read there is nothing to lose. The window's
    /// place still waiting to be kept is handed to the host first and later
    /// moves are held back (`desktop::seal_places`), so the install's drain
    /// of every writer (the chat list, the preferences, the theme, the
    /// place, the library's draft: native.rs `flush_for_install`) has the
    /// last; the others are sent as they change.
    pub fn save_for_install(&mut self, then: impl FnOnce(Result<(), String>, &mut App) + 'static, cx: &mut Context<Self>) {
        crate::desktop::seal_places(cx);
        // Asked again (Retry, Install anyway): written anew.
        self.sealed = false;
        if !self.loaded {
            return then(Ok(()), cx);
        }
        self.settle(
            Box::new(move |w: &mut Workspace, settled, cx: &mut Context<Workspace>| {
                if let Err(why) = settled {
                    return then(Err(why), cx);
                }
                w.flush_acked(
                    QUIT_FLUSHES,
                    Box::new(move |w: &mut Workspace, wrote, cx: &mut Context<Workspace>| {
                        w.sealed = wrote.is_ok();
                        then(wrote, cx)
                    }),
                    cx,
                );
            }),
            cx,
        );
    }

    /// Something under way will still change the list: a round of looks or
    /// its saved titles, or work let through before the freeze (`admit`).
    /// The work let through and not yet released (`admit`), for the tests.
    #[cfg(test)]
    pub(crate) fn admitted_for_tests(&self) -> usize {
        self.admitted
    }

    fn unsettled(&self) -> bool {
        self.polling || self.titles > 0 || self.admitted > 0
    }

    /// `then`, once nothing under way will still change the list; no new
    /// round of looks starts meanwhile. Told why not if it has not ended in
    /// `SETTLE_LIMIT`.
    fn settle(&mut self, then: Then, cx: &mut Context<Self>) {
        if !self.unsettled() {
            return then(self, Ok(()), cx);
        }
        self.settling.push(then);
        if self._settle_limit.is_none() {
            self._settle_limit = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(SETTLE_LIMIT).await;
                let _ = this.update(cx, |this, cx| {
                    this._settle_limit = None;
                    let why = format!("what was under way as the update began did not end within {} s", SETTLE_LIMIT.as_secs());
                    this.settled(Err(why), cx);
                });
            }));
        }
    }

    /// What `settle` waits for may have ended.
    fn check_settled(&mut self, cx: &mut Context<Self>) {
        if !self.settling.is_empty() && !self.unsettled() {
            self._settle_limit = None;
            self.settled(Ok(()), cx);
        }
    }

    fn settled(&mut self, result: Result<(), String>, cx: &mut Context<Self>) {
        for then in std::mem::take(&mut self.settling) {
            then(self, result.clone(), cx);
        }
        // The looks held back meanwhile go on.
        self.schedule(cx);
    }

    /// Whether a quit is under way or done, for the tests.
    #[cfg(test)]
    pub(crate) fn quitting(&self) -> bool {
        self.quitting
    }

    fn quit_failed(&mut self, why: String, cx: &mut Context<Self>) {
        log::warn!("the app did not quit: the chat list could not be saved ({why})");
        // The host takes back the way out it claimed for this quit: the core
        // is still up, and a later Quit or Restart is not refused.
        if let Err(e) = self.host.send(crate::desktop::cmd::QUIT_REFUSED, json!({ "why": why })) {
            log::warn!("quit_refused: {e}");
        }
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

    /// A divider let go of (board.rs): kept where it was dropped.
    pub fn set_split(&mut self, board: usize, layout: usize, split: crate::state::Split, share: f32, cx: &mut Context<Self>) {
        if self.frozen {
            return;
        }
        self.state.set_split(board, layout, split, share);
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

    /// The window's scale, in percent (scale.rs): held to the steps, and
    /// applied by the root view as it hears of the change.
    pub fn set_ui_scale(&mut self, pct: u16, cx: &mut Context<Self>) {
        let pct = crate::scale::snap(pct);
        if self.frozen || pct == self.state.ui_scale {
            return;
        }
        self.state.ui_scale = pct;
        self.changed(cx);
    }

    pub fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        if self.frozen {
            return;
        }
        self.state.sidebar.pinned = !self.state.sidebar.pinned;
        self.changed(cx);
    }

    /// The sidebar's edge dragged (sidebar.rs): its width at 100%, written
    /// as the page's throttled storage wrote it, the last one once the drag
    /// stops.
    pub fn set_sidebar_width(&mut self, width: f32, cx: &mut Context<Self>) {
        if !self.frozen && self.state.set_sidebar_width(width) {
            self.changed(cx);
        }
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

    /// Alt+digit (state.rs `toggle_peek`): false when it means nothing
    /// here, and the terminal takes the key. The keyboard goes as a click
    /// on the chat's row sends it: to the peek's frame, so Esc closes it
    /// until its terminal is clicked or typed in.
    pub fn peek_pane(&mut self, digit: usize, cx: &mut Context<Self>) -> bool {
        if self.frozen {
            return true;
        }
        if self.state.toggle_peek(digit).is_none() {
            return false;
        }
        self.focus_home();
        self.changed(cx);
        true
    }

    /// Ctrl+Shift+W. Not while the core is not up: no chat is known to wait
    /// then (`shown_status`).
    pub fn next_waiting(&mut self, cx: &mut Context<Self>) {
        if self.frozen || !self.core_up(cx) {
            return;
        }
        let lives = &self.lives;
        if let Some(id) = self.state.next_waiting(|c| lives.get(&c.id).copied().unwrap_or(c.status)) {
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
    /// None while frozen for an update: nothing is listed.
    pub fn add_chat(&mut self, folder: &str, chat: Chat, by_person: bool, cx: &mut Context<Self>) -> Option<Placed> {
        if self.frozen {
            log::warn!("chat {} not listed: {FROZEN}", chat.id);
            return None;
        }
        let id = chat.id.clone();
        let n = self.state.folders.len();
        self.state.add_chat(folder, chat, by_person, || new_id("f", n));
        let placed = self.state.auto_place(&id);
        self.focus_request = Some(id);
        self.changed(cx);
        Some(placed)
    }

    /// A chat the delete dialog deleted: off every board and out of the list.
    /// Kept while frozen for an update (the dialog refuses before then).
    pub fn remove_chat(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.frozen {
            log::warn!("chat {id} not taken off the list: {FROZEN}");
            return;
        }
        self.unlist(id, cx);
    }

    /// A row the delete dialog took off, put back where it stood: the core
    /// refused its session's delete, the id being a helper's it still holds
    /// for an opening (dialogs/rows.rs `HELD_AS_HELPER`). Its session is
    /// there still; gone from the list, the opening could never be settled
    /// to a row. Not while frozen: the update's write is made without it, and
    /// the next start settles the helper back to its agent.
    pub fn relist(&mut self, row: crate::state::Listed, cx: &mut Context<Self>) {
        if self.frozen {
            return log::warn!("chat {} not listed again: {FROZEN}", row.chat.id);
        }
        let n = self.state.folders.len();
        if self.state.relist(row, || new_id("f", n)) {
            self.changed(cx);
        }
    }

    /// The row listed for a helper opened as a chat, taken off again where
    /// the core did not settle its opening: frozen or not, as the opening
    /// was let through before the list froze (`admit`), and the list's last
    /// write waits for this.
    pub fn unlist_promoted(&mut self, id: &str, cx: &mut Context<Self>) {
        self.unlist(id, cx);
    }

    fn unlist(&mut self, id: &str, cx: &mut Context<Self>) {
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
        let mut spec = spec(chat, &folder, &account.path);
        // An adopted orphan's first start attaches to the session it was
        // adopted for, or starts nothing: one gone since, or another under
        // its id, is not started in its place.
        if let Some(start) = &chat.adopt {
            spec.args["attach"] = json!(start);
        }
        Ok(spec)
    }

    /// The account path a chat runs on, when known.
    fn account_path(&self, chat: &Chat) -> Option<String> {
        self.accounts.as_ref()?.iter().find(|a| a.provider == chat.provider && a.name == chat.account).map(|a| a.path.clone())
    }

    fn core_up(&self, cx: &App) -> bool {
        self.core.read(cx).link().state == LinkState::Up
    }

    /// The status a row or a pane shows for `chat`: the core's lifecycle
    /// record of it where it has one, else what it did last, while the core
    /// is up to say; `NoCore` while it is not, since whatever it was
    /// doing may have ended with the core.
    pub fn shown_status(&self, chat: &Chat, cx: &App) -> Status {
        if self.core_up(cx) { self.lives.get(&chat.id).copied().unwrap_or(chat.status) } else { Status::NoCore }
    }

    /// The core's lifecycle record of each session, as a status by chat id
    /// (the power state's, taken by the status bar): shown in place of the
    /// chat's own where there is one, as the bar's counts go by it.
    pub fn set_lives(&mut self, lives: HashMap<String, Status>, cx: &mut Context<Self>) {
        if lives != self.lives {
            self.lives = lives;
            cx.notify();
        }
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
        self.notify(Notice { title, body, chat_id, sound: self.prefs.sound });
    }

    fn notify(&self, notice: Notice) {
        if let Err(e) = self.host.notify(notice) {
            log::warn!("notify: {e}");
        }
    }

    /// There is a chat to look at: the list has come, the core is up and a
    /// chat has an account Luna knows.
    fn lookable(&self, cx: &App) -> bool {
        self.loaded && self.core_up(cx) && self.state.chats().any(|c| self.account_path(c).is_some())
    }

    /// The wait before the next look: `EVERY`, or `HIDDEN_EVERY` once the
    /// window has been hidden to the tray for `HIDDEN_AFTER`.
    fn every(&self, cx: &App) -> Duration {
        match self.hidden_since {
            Some(at) if cx.background_executor().now().duration_since(at) >= HIDDEN_AFTER => HIDDEN_EVERY,
            _ => EVERY,
        }
    }

    /// Arms the next look, unless one is armed or under way, or an update's
    /// last write waits for the one under way (`settle`), or there is
    /// nothing to look at: then no timer runs (one armed before the last
    /// chat to look at went is let go), and what makes a chat lookable (the
    /// list, the accounts, the core, a chat added) looks.
    fn schedule(&mut self, cx: &mut Context<Self>) {
        if !self.lookable(cx) {
            self._poll = None;
            return;
        }
        if self._poll.is_some() || self.polling || !self.settling.is_empty() {
            return;
        }
        let wait = self.every(cx);
        self._poll = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let _ = this.update(cx, |this, cx| {
                this._poll = None;
                this.poll(cx);
            });
        }));
    }

    /// Whether the next look waits armed, for the tests.
    #[cfg(test)]
    pub(crate) fn look_armed(&self) -> bool {
        self._poll.is_some()
    }

    /// The window was hidden to the tray or shown (`desktop::Shown`): shown
    /// again, the sessions are looked at now, not at the slow tick's end.
    fn set_hidden(&mut self, hidden: bool, cx: &mut Context<Self>) {
        match (hidden, self.hidden_since) {
            (true, None) => self.hidden_since = Some(cx.background_executor().now()),
            (false, Some(_)) => {
                self.hidden_since = None;
                self.poll(cx);
            }
            _ => {}
        }
    }

    /// Tells the core every chat's name and board (`chat_names`), as the
    /// page's bridge did: what `luna_session_list` calls the person's chats
    /// and where it puts them. Only what differs from the last told, and
    /// once the list has come: the names sent replace the core's, and an
    /// empty list sent before it would take every name away. One the core
    /// did not take is told at the next change, or to the next core.
    fn tell_names(&mut self, cx: &mut Context<Self>) {
        if !self.loaded || !self.core_up(cx) {
            return;
        }
        let names: serde_json::Map<String, Value> = self.state.chats().map(|c| (c.id.clone(), json!(c.name))).collect();
        let groups: serde_json::Map<String, Value> = self.state.chats().map(|c| (c.id.clone(), json!(c.board))).collect();
        let args = json!({ "names": names, "groups": groups });
        let key = args.to_string();
        if self.names_told.as_ref() == Some(&key) {
            return;
        }
        self.names_told = Some(key.clone());
        let answer = self.host.call(cmd::CHAT_NAMES, args);
        cx.spawn(async move |this, cx| {
            if let Err(e) = answer.await {
                log::warn!("chat_names: {e}");
                let _ = this.update(cx, |this, _| {
                    if this.names_told.as_ref() == Some(&key) {
                        this.names_told = None;
                    }
                });
            }
        })
        .detach();
    }

    /// One look at every chat with an account, unless one is under way or
    /// the core is not up. The answer is taken only for the core it was
    /// asked of, and for each chat only if its session did not exit since
    /// (`Fence`): an older one would put back a status, a draft or a session
    /// that is gone. A call that fails, or does not answer in
    /// `POLL_TIMEOUT`, says nothing: the next tick asks again.
    fn poll(&mut self, cx: &mut Context<Self>) {
        // Nor while an update's last write waits for the round under way:
        // the looks go on once it is made (`settled`).
        if !self.loaded || self.polling || !self.settling.is_empty() || !self.core_up(cx) {
            return;
        }
        // A round asked now replaces the one the timer waits for: the next
        // is armed as this one ends (`schedule`).
        self._poll = None;
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
                    let titles = match looks {
                        Ok(looks) => this.apply(&asked, &fence, looks, cx),
                        Err(e) => {
                            log::warn!("sessions_meta: {e}");
                            Vec::new()
                        }
                    };
                    if titles.is_empty() {
                        this.check_settled(cx);
                    } else {
                        this.titles += 1;
                    }
                    this.schedule(cx);
                    titles
                })
                .unwrap_or_default();
            if titles.is_empty() {
                return;
            }
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
            let _ = this.update(cx, |this, cx| {
                this.titles -= 1;
                this.check_settled(cx);
            });
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
                titles.extend(self.title_to_look(&chat, path));
                continue;
            };
            // A live session its CLI has left nothing on disk for yet answers
            // its record's status alone, with no title, prompt or session id:
            // its name is looked for as a resting chat's is.
            if meta.title.is_none() && meta.first_prompt.is_none() && meta.session_id.is_none() {
                titles.extend(self.title_to_look(&chat, path));
            }
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
                    sound: self.prefs.sound,
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

    /// The saved title to look up for `chat` (`saved_title`), once a chat:
    /// one with a session id and a name it may still lose.
    fn title_to_look(&mut self, chat: &Chat, path: &str) -> Option<(String, Value)> {
        let session = chat.session_id.as_ref().filter(|_| chat.name_source != Some(NameSource::User))?;
        if !self.title_looked.insert(chat.id.clone()) {
            return None;
        }
        let provider = if chat.provider == Provider::Claude { "claude" } else { "codex" };
        Some((chat.id.clone(), json!({ "provider": provider, "accountPath": path, "sessionId": session })))
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
pub(crate) fn is_worktree(provider: Provider, cwd: &str) -> bool {
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

/// The folder a Claude Code worktree at `cwd` was made in
/// (`<folder>/.claude/worktrees/<name>`): its chat's, which resumes it there.
pub(crate) fn claude_worktree_folder(cwd: &str) -> Option<String> {
    let path = cwd.trim_end_matches(['/', '\\']);
    let (root, _) = path.rsplit_once(['/', '\\'])?;
    let root = root.strip_suffix("worktrees")?.trim_end_matches(['/', '\\']);
    let root = root.strip_suffix(".claude")?.trim_end_matches(['/', '\\']);
    (!root.is_empty() && claude_worktree_name(root, Some(path)).is_some()).then(|| root.to_owned())
}

/// Luna's label for a Claude Code permission mode as the CLI takes it
/// (`perm_cli` back); None for one it has no label for.
pub(crate) fn perm_label(cli: &str) -> Option<&'static str> {
    match cli {
        "default" => Some("Ask"),
        "acceptEdits" => Some("Edits"),
        "plan" => Some("Plan only"),
        "auto" => Some("Auto"),
        "bypassPermissions" => Some("Bypass"),
        _ => None,
    }
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
        assert_eq!(claude_worktree_folder("C:\\repo\\.claude\\worktrees\\fix-1\\").as_deref(), Some("C:\\repo"));
        assert_eq!(claude_worktree_folder("/repo/.claude/worktrees/fix-1").as_deref(), Some("/repo"));
        assert_eq!(claude_worktree_folder("C:\\repo\\src"), None);
        assert_eq!(claude_worktree_folder("C:\\repo\\.claude\\worktrees"), None);
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
