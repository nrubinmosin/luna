//! The window's state, as GPUI entities: one model per thing the host tells
//! the window about, updated on the window's thread by the one task that
//! drains the inbox (`deliver`). Views observe the models they show, so a
//! change re-renders only them; nothing polls.

use crate::dto::{CoreState, LinkState, ThemePref};
use crate::host::{Host, Message, cmd, event};
use crate::theme::Palette;
use gpui::{App, AppContext, Context, Entity};
use serde_json::{Value, json};
use std::sync::Arc;

/// The line to the core (corelink.rs), as `core://state` tells it.
pub struct CoreLink {
    link: CoreState,
    /// It has been up since this window started: a `starting` after that is
    /// a core being replaced (a restart, or one found again after it let the
    /// window go), not the first one being found.
    was_up: bool,
    /// `core://state` events taken so far. An answer to `core_state` asked
    /// before the last of them is older than what it says, and is dropped.
    events: u64,
}

impl CoreLink {
    pub fn link(&self) -> &CoreState {
        &self.link
    }

    pub fn was_up(&self) -> bool {
        self.was_up
    }

    /// Takes a state from a `core://state` event.
    pub fn take_event(&mut self, link: CoreState, cx: &mut Context<Self>) {
        self.events += 1;
        self.take(link, cx);
    }

    /// Takes the answer to a `core_state` asked when `events` events had come:
    /// one that has come since is newer than the answer.
    pub fn take_answer(&mut self, link: CoreState, asked_at: u64, cx: &mut Context<Self>) {
        if asked_at == self.events {
            self.take(link, cx);
        }
    }

    /// Takes a state the host reported; the views hear of it only when it
    /// changed.
    fn take(&mut self, link: CoreState, cx: &mut Context<Self>) {
        let was_up = self.was_up || link.state == LinkState::Up;
        if link != self.link || was_up != self.was_up {
            self.link = link;
            self.was_up = was_up;
            cx.notify();
        }
    }
}

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
}

impl Models {
    /// `theme` is the one the person chose last time; `system_dark` the
    /// system's appearance as the window starts.
    pub fn new(host: Arc<dyn Host>, theme: ThemePref, system_dark: bool, cx: &mut App) -> Models {
        Models {
            host,
            core: cx.new(|_| CoreLink { link: CoreState::default(), was_up: false, events: 0 }),
            shell: cx.new(|_| Shell { pref: theme, system_dark }),
        }
    }

    /// Asks for the core's state, as the page does at start. Events keep
    /// coming meanwhile, and the answer may land after some of them: it is
    /// taken only if none did (`CoreLink::take_answer`). A window busy with
    /// its first frame while the core came up kept a stale `starting` so.
    pub fn ask_core_state(&self, cx: &mut App) {
        let asked_at = self.core.read(cx).events;
        let answer = self.host.call(cmd::CORE_STATE, json!({}));
        let core = self.core.clone();
        cx.spawn(async move |cx| match answer.await.and_then(|v| serde_json::from_value::<CoreState>(v).map_err(|e| e.to_string())) {
            Ok(link) => core.update(cx, |c, cx| c.take_answer(link, asked_at, cx)),
            Err(e) => log::warn!("core_state: {e}"),
        })
        .detach();
    }

    /// The theme button: the next theme, kept by the host for the next start.
    pub fn cycle_theme(&self, cx: &mut App) {
        let pref = self.shell.update(cx, |s, cx| s.cycle(cx));
        let answer = self.host.call(cmd::SET_WINDOW_THEME, json!({ "theme": pref.name() }));
        cx.background_spawn(async move {
            if let Err(e) = answer.await {
                log::warn!("set_window_theme: {e}");
            }
        })
        .detach();
    }

    /// Restart on the core banner: the core's state says how it goes.
    pub fn restart_core(&self, cx: &mut App) {
        let answer = self.host.call(cmd::CORE_RESTART, json!({}));
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
    }
}

/// An event from the host, into the model it is about. One the window has
/// no use for yet is dropped.
pub fn on_event(models: &Models, name: &str, payload: Value, cx: &mut App) {
    if name == event::CORE_STATE {
        match serde_json::from_value::<CoreState>(payload) {
            Ok(link) => models.core.update(cx, |c, cx| c.take_event(link, cx)),
            Err(e) => log::warn!("{name}: {e}"),
        }
    }
}
