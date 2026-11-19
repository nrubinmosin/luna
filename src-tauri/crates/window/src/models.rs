//! The window's state, as GPUI entities: one model per thing the host tells
//! the window about, updated on the window's thread by the one task that
//! drains the inbox (`deliver`). Views observe the models they show, so a
//! change re-renders only them; nothing polls.

use crate::dto::{CoreState, LinkState, ThemePref};
use crate::host::{Host, Message, cmd, event};
use crate::term::TermPane;
use crate::theme::Palette;
use crate::workspace::Workspace;
use gpui::{App, AppContext, Context, Entity, EventEmitter, WeakEntity};
use serde::Deserialize;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

/// The line to the core (corelink.rs), as `core://state` tells it.
pub struct CoreLink {
    link: CoreState,
    /// It has been up since this window started: a `starting` after that is
    /// a core being replaced (a restart, or one found again after it let the
    /// window go), not the first one being found.
    was_up: bool,
    /// Counts the cores the panes' sessions were found in, as the page's
    /// `epoch` does (core.store.ts): each `up` that is not the one taken last
    /// starts a new one, and the panes start their sessions over in it.
    epoch: u64,
    /// The last `up` taken: its core's pid and its revision.
    last_up: Option<(u32, u64)>,
    /// A state other than `up` was taken since that one.
    left: bool,
}

impl CoreLink {
    pub fn link(&self) -> &CoreState {
        &self.link
    }

    pub fn was_up(&self) -> bool {
        self.was_up
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Takes a state the host reported, from a `core://state` event or as
    /// the answer to `core_state`, unless it is older than the one taken last
    /// (its `revision`): a `down` that lost a race to the `up` after it, or
    /// an answer asked before the event that came first. The views hear of it
    /// only when what they show changed; a new epoch is `NewEpoch`.
    ///
    /// An `up` begins a new epoch unless it is the one taken last: the same
    /// core (pid) and revision, nothing else taken in between. The events in
    /// between may never come: only the latest state waits in the inbox
    /// (host.rs `event::LATEST`), so `up`, `starting`, `up` of another core
    /// can arrive as the last `up` alone. corelink.rs sets a state other than
    /// `up` before every `up`, so an `up` of a newer revision is a core
    /// replaced or found again (`rejoin`, whose output meanwhile never came)
    /// whatever came of the events between. A host that does not count
    /// revisions (0) goes by the pid and the states it did tell.
    pub fn take(&mut self, link: CoreState, cx: &mut Context<Self>) {
        if link.revision < self.link.revision {
            return;
        }
        let was_up = self.was_up || link.state == LinkState::Up;
        let epoch = self.epoch;
        if link.state == LinkState::Up {
            let now = (link.core.as_ref().map_or(0, |c| c.pid), link.revision);
            let same = self
                .last_up
                .is_some_and(|(pid, revision)| !self.left && pid == now.0 && (revision == now.1 || revision == 0 || now.1 == 0));
            if !same {
                self.epoch += 1;
            }
            self.last_up = Some(now);
            self.left = false;
        } else if self.last_up.is_some() {
            self.left = true;
        }
        let changed = !link.shows_as(&self.link) || was_up != self.was_up;
        self.link = link;
        self.was_up = was_up;
        if changed {
            cx.notify();
        }
        if epoch != self.epoch {
            cx.emit(NewEpoch);
        }
    }
}

/// A new epoch began (`CoreLink::take`): the panes start their sessions
/// over. An event rather than a change to draw, so the views that show the
/// link do not redraw for it.
pub struct NewEpoch;

impl EventEmitter<NewEpoch> for CoreLink {}

/// The window's own furniture: for now the theme.
pub struct Shell {
    pref: ThemePref,
    /// The window's appearance is dark: what `System` follows.
    system_dark: bool,
}

impl Shell {
    pub fn pref(&self) -> ThemePref {
        self.pref
    }

    pub fn dark(&self) -> bool {
        match self.pref {
            ThemePref::System => self.system_dark,
            ThemePref::Light => false,
            ThemePref::Dark => true,
        }
    }

    pub fn palette(&self) -> Palette {
        Palette::of(self.dark())
    }

    /// The window's appearance changed (the system's light or dark mode).
    pub fn set_system_dark(&mut self, dark: bool, cx: &mut Context<Self>) {
        if dark != self.system_dark {
            self.system_dark = dark;
            cx.notify();
        }
    }

    /// The theme button: the next theme, in the page's order.
    pub fn cycle(&mut self, cx: &mut Context<Self>) -> ThemePref {
        self.pref = self.pref.next();
        cx.notify();
        self.pref
    }
}

/// Every model, and the host they answer to.
#[derive(Clone)]
pub struct Models {
    pub host: Arc<dyn Host>,
    pub core: Entity<CoreLink>,
    pub shell: Entity<Shell>,
    /// The chats, the boards and the sidebar (workspace.rs).
    pub workspace: Entity<Workspace>,
    /// The window's own focus, where the keyboard is when no terminal has
    /// it: the window's keys (keys.rs) are bound in its context, which a key
    /// reaches only from something focused inside it.
    pub home: gpui::FocusHandle,
    /// The terminal panes open, for the events about their sessions.
    pub(crate) panes: Rc<RefCell<Vec<WeakEntity<TermPane>>>>,
}

impl Models {
    /// `theme` is the one the person chose last time; `system_dark` the
    /// system's appearance as the window starts.
    pub fn new(host: Arc<dyn Host>, theme: ThemePref, system_dark: bool, cx: &mut App) -> Models {
        let core = cx.new(|_| CoreLink { link: CoreState::default(), was_up: false, epoch: 0, last_up: None, left: false });
        let workspace = cx.new(|cx| Workspace::new(host.clone(), core.clone(), cx));
        let home = cx.focus_handle();
        Models { host, core, shell: cx.new(|_| Shell { pref: theme, system_dark }), workspace, home, panes: Rc::default() }
    }

    /// The panes showing `session`, the ones gone let go of.
    pub(crate) fn panes_of(&self, session: &str, cx: &App) -> Vec<Entity<TermPane>> {
        self.all_panes().into_iter().filter(|p| p.read(cx).id() == session).collect()
    }

    /// Every pane open, the ones gone let go of.
    fn all_panes(&self) -> Vec<Entity<TermPane>> {
        let mut panes = self.panes.borrow_mut();
        panes.retain(|p| p.upgrade().is_some());
        panes.iter().filter_map(WeakEntity::upgrade).collect()
    }

    /// Asks for the core's state, as the page does at start. Events keep
    /// coming meanwhile, and the answer may land after some of them: its
    /// revision says whether it is still news (`CoreLink::take`). A window
    /// busy with its first frame while the core came up kept a stale
    /// `starting` without it.
    pub fn ask_core_state(&self, cx: &mut App) {
        let answer = self.host.call(cmd::CORE_STATE, json!({}));
        let core = self.core.clone();
        cx.spawn(async move |cx| match answer.await.and_then(|v| serde_json::from_value::<CoreState>(v).map_err(|e| e.to_string())) {
            Ok(link) => core.update(cx, |c, cx| c.take(link, cx)),
            Err(e) => log::warn!("core_state: {e}"),
        })
        .detach();
    }

    /// The theme button: the next theme, kept by the host for the next start.
    /// Sent, so the host has the choices in the order they were made.
    pub fn cycle_theme(&self, cx: &mut App) {
        let pref = self.shell.update(cx, |s, cx| s.cycle(cx));
        if let Err(e) = self.host.send(cmd::SET_WINDOW_THEME, json!({ "theme": pref.name() })) {
            log::warn!("set_window_theme: {e}");
        }
    }

    /// Restart on the core banner: the core's state says how it goes. For
    /// the state the banner shows: one that has moved on since (the core
    /// came back meanwhile) restarts nothing.
    pub fn restart_core(&self, cx: &mut App) {
        let revision = self.core.read(cx).link.revision;
        let answer = self.host.call(cmd::CORE_RESTART, json!({ "revision": revision }));
        cx.background_spawn(async move {
            if let Err(e) = answer.await {
                log::warn!("core_restart: {e}");
            }
        })
        .detach();
    }
}

/// One message from the inbox, on the window's thread.
pub(crate) fn deliver(models: &Models, message: Message, cx: &mut App) {
    match message {
        Message::Run(f) => f(),
        Message::Event { name, payload } => on_event(models, &name, payload, cx),
        Message::Request(request) => crate::desktop::on_request(request, cx),
        // The inbox hands these out as the event they stand for.
        Message::Latest(_) => {}
    }
}

/// `pty://exit`'s payload, as the window reads it.
#[derive(Deserialize)]
struct PtyExit {
    id: String,
    #[serde(default)]
    generation: u64,
    /// The CLI's exit code; none when the host could not read it.
    #[serde(default)]
    code: Option<u32>,
}

/// `window://state-saved`'s.
#[derive(Deserialize)]
struct StateSaved {
    seq: u64,
    #[serde(default)]
    error: Option<String>,
}

/// `ui://notify`'s.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CoreNotice {
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    chat_id: Option<String>,
}

/// `pty://input-refused`'s.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InputRefused {
    id: String,
    #[serde(default)]
    waiting_kb: u64,
}

/// An event from the host, into the model or the panes it is about, or to
/// the desktop's side of the window (desktop.rs).
pub fn on_event(models: &Models, name: &str, payload: Value, cx: &mut App) {
    match name {
        event::CORE_STATE => match serde_json::from_value::<CoreState>(payload) {
            Ok(link) => models.core.update(cx, |c, cx| c.take(link, cx)),
            Err(e) => log::warn!("{name}: {e}"),
        },
        event::PTY_EXIT => match serde_json::from_value::<PtyExit>(payload) {
            Ok(exit) => {
                let panes = models.panes_of(&exit.id, cx);
                if panes.is_empty() {
                    // No pane to say which spawn runs: the chat rests, and a
                    // pane showing it starts or finds it anew.
                    models.workspace.update(cx, |w, cx| w.on_exit(&exit.id, cx));
                }
                // A pane takes only its own spawn's exit, and then tells the
                // workspace (term/ `TermPane::exited`): an older one's, come
                // late, rests nothing.
                for pane in panes {
                    pane.update(cx, |p, cx| p.on_exit(exit.generation, exit.code, cx));
                }
            }
            Err(e) => log::warn!("{name}: {e}"),
        },
        event::STATE_SAVED => match serde_json::from_value::<StateSaved>(payload) {
            Ok(saved) => models.workspace.update(cx, |w, cx| w.on_saved(saved.seq, saved.error, cx)),
            Err(e) => log::warn!("{name}: {e}"),
        },
        // Every session was closed for a shutdown of the PC that was taken
        // back: each pane starts its own over, as after a core restart.
        event::START_OVER => {
            for pane in models.all_panes() {
                pane.update(cx, |p, cx| p.start_over(cx));
            }
        }
        event::PTY_INPUT_REFUSED => match serde_json::from_value::<InputRefused>(payload) {
            Ok(refused) => {
                for pane in models.panes_of(&refused.id, cx) {
                    pane.update(cx, |p, cx| p.on_refused(refused.waiting_kb, cx));
                }
            }
            Err(e) => log::warn!("{name}: {e}"),
        },
        event::NOTIFY => match serde_json::from_value::<CoreNotice>(payload) {
            Ok(n) => models.workspace.update(cx, |w, _| w.on_notice(n.title, n.body, n.chat_id)),
            Err(e) => log::warn!("{name}: {e}"),
        },
        _ if crate::panels::event::ALL.contains(&name) => crate::panels::on_event(name, &payload, cx),
        _ => crate::desktop::on_event(name, &payload, cx),
    }
}
