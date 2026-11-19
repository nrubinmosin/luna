//! The window over a fake host, on GPUI's test platform: what the host says
//! reaches the models and the views showing them, and only those redraw;
//! the theme follows the button and the system; the banner's Restart asks
//! the host.

use crate::demo::{self, FakeHost};
use crate::dto::{CoreState, LinkState, ThemePref};
use crate::host::{Mailbox, cmd, event, mailbox};
use crate::models::Models;
use crate::root::LunaWindow;
use gpui::{Entity, TestAppContext, VisualTestContext};
use serde_json::json;
use std::sync::Arc;

struct Fixture<'a> {
    host: Arc<FakeHost>,
    mailbox: Mailbox,
    models: Models,
    view: Entity<LunaWindow>,
    cx: &'a mut VisualTestContext,
    /// The revision of the last state told.
    revision: u64,
}

/// The window as `run` makes it, on the test platform, its core first
/// answered as `first`.
fn open(cx: &mut TestAppContext, theme: ThemePref, first: CoreState) -> Fixture<'_> {
    let host = FakeHost::new();
    host.answer(cmd::CORE_STATE, serde_json::to_value(first).unwrap());
    let (mailbox, inbox) = mailbox();
    let models = cx.update(|cx| crate::start(host.clone(), inbox, theme, cx));
    let window_models = models.clone();
    let (view, cx) = cx.add_window_view(move |window, cx| LunaWindow::new(window_models, Vec::new(), window, cx));
    cx.run_until_parked();
    Fixture { host, mailbox, models, view, cx, revision: 0 }
}

fn state(state: LinkState) -> CoreState {
    CoreState { state, core: (state != LinkState::Starting).then(demo::core), error: None, revision: 0 }
}

fn at(link: LinkState, revision: u64) -> CoreState {
    CoreState { revision, ..state(link) }
}

impl Fixture<'_> {
    /// Tells the window a state, a revision newer than the last.
    fn tell(&mut self, link: CoreState) {
        self.revision += 1;
        self.tell_at(CoreState { revision: self.revision, ..link });
    }

    /// Tells the window a state with the revision it has.
    fn tell_at(&mut self, link: CoreState) {
        assert!(self.mailbox.event(event::CORE_STATE, serde_json::to_value(link).unwrap()));
        self.cx.run_until_parked();
    }

    fn link(&mut self) -> LinkState {
        self.models.core.read_with(self.cx, |c, _| c.link().state)
    }

    /// Renders so far of the title bar, the banner and the board.
    fn renders(&mut self) -> (usize, usize, usize) {
        self.view.read_with(self.cx, |v, cx| (v.title_bar.read(cx).renders, v.banner.read(cx).renders, v.board.read(cx).renders))
    }

    fn dark(&mut self) -> bool {
        self.models.shell.read_with(self.cx, |s, _| s.dark())
    }
}

#[gpui::test]
fn the_first_answer_and_every_event_reach_the_core_model(cx: &mut TestAppContext) {
    let mut f = open(cx, ThemePref::Light, state(LinkState::Starting));
    assert_eq!(f.host.calls_of(cmd::CORE_STATE).len(), 1, "the window asks once, at start");
    assert_eq!(f.link(), LinkState::Starting);

    f.tell(state(LinkState::Up));
    assert_eq!(f.link(), LinkState::Up);
    assert!(f.models.core.read_with(f.cx, |c, _| c.was_up()));

    // An event the window has no use for, or one it cannot read, changes
    // nothing and stops nothing.
    assert!(f.mailbox.event("pty://output", json!({ "id": "x" })));
    assert!(f.mailbox.event(event::CORE_STATE, json!({ "state": "sideways" })));
    f.cx.run_until_parked();
    assert_eq!(f.link(), LinkState::Up);
    f.tell(state(LinkState::Down));
    assert_eq!(f.link(), LinkState::Down);
}

/// The window busy while the core comes up: the answer to `core_state`,
/// asked at start, lands after the events that followed it, and must not
/// put the older state back.
#[gpui::test]
fn an_answer_older_than_an_event_is_dropped(cx: &mut TestAppContext) {
    let mut f = open(cx, ThemePref::Light, state(LinkState::Starting));
    f.host.answer(cmd::CORE_STATE, serde_json::to_value(at(LinkState::Starting, 1)).unwrap());
    f.cx.update(|_, cx| {
        f.models.ask_core_state(cx);
        // Taken before the answer's task gets to run.
        crate::models::on_event(&f.models, event::CORE_STATE, serde_json::to_value(at(LinkState::Up, 2)).unwrap(), cx);
    });
    f.cx.run_until_parked();
    assert_eq!(f.link(), LinkState::Up);
    // An answer newer than the last event stands.
    f.host.answer(cmd::CORE_STATE, serde_json::to_value(at(LinkState::Starting, 3)).unwrap());
    f.cx.update(|_, cx| f.models.ask_core_state(cx));
    f.cx.run_until_parked();
    assert_eq!(f.link(), LinkState::Starting);
}

/// A state sent before a newer one but come after it (corelink.rs emits
/// outside its lock) is dropped: the banner does not go back to `down` over
/// a core that is up.
#[gpui::test]
fn an_event_older_than_the_last_taken_is_dropped(cx: &mut TestAppContext) {
    let mut f = open(cx, ThemePref::Light, state(LinkState::Starting));
    f.tell_at(at(LinkState::Up, 5));
    let before = f.renders();
    f.tell_at(at(LinkState::Down, 4));
    assert_eq!(f.link(), LinkState::Up);
    assert_eq!(f.renders(), before, "nothing redraws for it");
    // A newer revision of what is shown already is no change to draw.
    f.tell_at(at(LinkState::Up, 6));
    assert_eq!(f.renders(), before);
    f.tell_at(at(LinkState::Down, 7));
    assert_eq!(f.link(), LinkState::Down);
}

#[gpui::test]
fn work_handed_to_the_window_runs_on_its_thread_in_order(cx: &mut TestAppContext) {
    let f = open(cx, ThemePref::Light, state(LinkState::Up));
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    for i in 0..3 {
        let seen = seen.clone();
        assert!(f.mailbox.run_on_ui(move || seen.lock().unwrap().push(i)));
    }
    f.cx.run_until_parked();
    assert_eq!(*seen.lock().unwrap(), vec![0, 1, 2]);
}

#[gpui::test]
fn a_core_state_redraws_the_banner_and_nothing_else(cx: &mut TestAppContext) {
    let down = |why: &str| CoreState { state: LinkState::Down, core: None, error: Some(why.into()), revision: 0 };
    let mut f = open(cx, ThemePref::Light, down("pipe closed: a"));
    let (bar, banner, board) = f.renders();
    assert!(bar >= 1 && banner >= 1 && board >= 1, "drawn once at least");

    f.tell(down("pipe closed: b"));
    let after = f.renders();
    assert!(after.1 > banner, "the banner shows the change");
    assert_eq!((after.0, after.2), (bar, board), "the cached title bar and board are reused");

    // The same state again is no change at all.
    f.tell(down("pipe closed: b"));
    assert_eq!(f.renders(), after);

    // Up: the banner goes, and the board, given its room, lays out again;
    // the title bar still does not.
    f.tell(state(LinkState::Up));
    let up = f.renders();
    assert!(up.1 > after.1);
    assert_eq!(up.0, bar);
}

#[gpui::test]
fn nothing_redraws_while_nothing_happens(cx: &mut TestAppContext) {
    let mut f = open(cx, ThemePref::Light, state(LinkState::Up));
    let before = f.renders();
    f.cx.executor().advance_clock(std::time::Duration::from_secs(60));
    f.cx.run_until_parked();
    assert_eq!(f.renders(), before);
}

#[gpui::test]
fn the_theme_button_goes_round_and_the_host_keeps_each_choice(cx: &mut TestAppContext) {
    let mut f = open(cx, ThemePref::System, state(LinkState::Up));
    // The test platform's appearance is light.
    assert!(!f.dark());
    for (pref, dark) in [(ThemePref::Light, false), (ThemePref::Dark, true), (ThemePref::System, false)] {
        f.cx.update(|_, cx| f.models.cycle_theme(cx));
        f.cx.run_until_parked();
        assert_eq!(f.models.shell.read_with(f.cx, |s, _| s.pref()), pref);
        assert_eq!(f.dark(), dark, "{pref:?}");
        assert_eq!(f.host.sends_of(cmd::SET_WINDOW_THEME).last(), Some(&demo::theme_args(pref)));
    }
    // gpui-component's theme went along with it.
    f.cx.update(|_, cx| f.models.cycle_theme(cx));
    f.cx.update(|_, cx| f.models.cycle_theme(cx));
    f.cx.run_until_parked();
    assert!(f.dark());
    assert!(f.cx.update(|_, cx| gpui_component::Theme::global(cx).is_dark()));
}

#[gpui::test]
fn system_follows_the_windows_appearance_and_a_choice_does_not(cx: &mut TestAppContext) {
    let mut f = open(cx, ThemePref::System, state(LinkState::Up));
    f.models.shell.update(f.cx, |s, cx| s.set_system_dark(true, cx));
    f.cx.run_until_parked();
    assert!(f.dark());
    assert!(f.cx.update(|_, cx| gpui_component::Theme::global(cx).is_dark()));
    let (bar, _, board) = f.renders();
    f.models.shell.update(f.cx, |s, cx| s.set_system_dark(false, cx));
    f.cx.run_until_parked();
    assert!(!f.dark());
    let after = f.renders();
    assert!(after.0 > bar && after.2 > board, "every view shows the new theme");

    // A theme chosen outright stays whatever the system does.
    f.cx.update(|_, cx| f.models.cycle_theme(cx));
    f.cx.update(|_, cx| f.models.cycle_theme(cx));
    assert_eq!(f.models.shell.read_with(f.cx, |s, _| s.pref()), ThemePref::Dark);
    f.models.shell.update(f.cx, |s, cx| s.set_system_dark(false, cx));
    assert!(f.dark());
}

#[gpui::test]
fn restart_asks_the_host_once_and_the_state_tells_the_rest(cx: &mut TestAppContext) {
    let mut f = open(cx, ThemePref::Light, state(LinkState::Down));
    f.tell(state(LinkState::Down));
    f.cx.update(|_, cx| f.models.restart_core(cx));
    f.cx.run_until_parked();
    // For the state the banner shows, by its revision.
    assert_eq!(f.host.calls_of(cmd::CORE_RESTART), vec![json!({ "revision": 1 })]);
    // The model moves only on what the host says next.
    assert_eq!(f.link(), LinkState::Down);
    f.tell(state(LinkState::Starting));
    assert_eq!(f.link(), LinkState::Starting);
}

/// The buttons themselves, clicked where they are drawn. Selectors are
/// recorded only in builds with debug assertions: the Linux tests run these,
/// the release-built Windows tests do not.
#[cfg(debug_assertions)]
mod clicks {
    use super::*;
    use gpui::Modifiers;

    fn click(f: &mut Fixture<'_>, selector: &'static str) {
        let at = f.cx.debug_bounds(selector).unwrap_or_else(|| panic!("no {selector} drawn")).center();
        f.cx.simulate_click(at, Modifiers::none());
        f.cx.run_until_parked();
    }

    #[gpui::test]
    fn the_banners_restart_button_restarts_the_core(cx: &mut TestAppContext) {
        let mut f = open(cx, ThemePref::Light, state(LinkState::Down));
        click(&mut f, "core-restart");
        assert_eq!(f.host.calls_of(cmd::CORE_RESTART).len(), 1);
    }

    #[gpui::test]
    fn a_waiting_banner_offers_no_restart(cx: &mut TestAppContext) {
        let mut f = open(cx, ThemePref::Light, state(LinkState::Starting));
        assert!(f.cx.debug_bounds("core-restart").is_none());
        f.tell(state(LinkState::Mismatch));
        assert!(f.cx.debug_bounds("core-restart").is_some());
        f.tell(state(LinkState::Up));
        assert!(f.cx.debug_bounds("core-restart").is_none());
    }

    #[gpui::test]
    fn the_theme_button_cycles_the_theme(cx: &mut TestAppContext) {
        let mut f = open(cx, ThemePref::System, state(LinkState::Up));
        click(&mut f, "theme");
        assert_eq!(f.models.shell.read_with(f.cx, |s, _| s.pref()), ThemePref::Light);
        assert_eq!(f.host.sends_of(cmd::SET_WINDOW_THEME), vec![demo::theme_args(ThemePref::Light)]);
    }
}

#[test]
fn the_host_contract_lists_what_the_window_names() {
    // Every command the window calls is in the list luna_lib's dispatch is
    // held to, and so is every event it listens to.
    for c in [cmd::CORE_STATE, cmd::CORE_RESTART] {
        assert!(cmd::ALL.contains(&c), "{c}");
    }
    for c in [cmd::ENSURE_CLAUDE_SESSION, cmd::ENSURE_CODEX_SESSION, cmd::SESSION_MODES] {
        assert!(cmd::ALL.contains(&c), "{c}");
    }
    assert_eq!(cmd::SENT, [cmd::SET_WINDOW_THEME, cmd::WRITE_SESSION, cmd::RESIZE_SESSION]);
    for e in [event::CORE_STATE, event::PTY_EXIT, event::PTY_INPUT_REFUSED] {
        assert!(event::ALL.contains(&e), "{e}");
    }
    // A session's output goes to its sink, never the inbox.
    assert!(!event::wants(event::PTY_OUTPUT));
    for e in event::LATEST {
        assert!(event::ALL.contains(e), "{e}");
    }
    assert_eq!(serde_json::to_value(ThemePref::System).unwrap(), json!("system"));
}
