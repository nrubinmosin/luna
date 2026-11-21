//! The status bar and the orphan line over a fake host, on GPUI's test
//! platform: the counts from the chats' state, the Waiting jump, the
//! accounts' meters and their cadence, the update chip through every state,
//! the CLI chips, the power chip with its countdown and the tray's dot, the
//! orphan line, and that nothing draws while the window is idle.

#![cfg_attr(not(debug_assertions), allow(dead_code, unused_imports))]

use super::dto::{Armed, PowerAction, PowerState};
use super::updates::Update;
use super::vitals::{Counts, Popover, Vitals};
use super::{cmd, event, power, usage};
use crate::demo::{self, FakeHost};
use crate::desktop;
use crate::dto::{CoreState, LinkState, ThemePref};
use crate::host::{Mailbox, cmd as host_cmd, event as host_event, mailbox};
use crate::models::Models;
use crate::root::LunaWindow;
use gpui::{App, Entity, TestAppContext, VisualTestContext, Window};
use serde_json::{Value, json};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

/// The demo core's fingerprint: an update built with it leaves the sessions.
const CORE: &str = "0123456789abcdef";
/// The instant the tests' clock starts at: 2026-11-19T12:00:00Z.
const T0: u64 = 1_795_089_600_000;

struct S<'a> {
    host: Arc<FakeHost>,
    mailbox: Mailbox,
    models: Models,
    view: Entity<LunaWindow>,
    clock: Rc<Cell<u64>>,
    cx: &'a mut VisualTestContext,
}

fn up() -> Value {
    serde_json::to_value(CoreState { state: LinkState::Up, core: Some(demo::core()), error: None, revision: 1 }).unwrap()
}

fn claude_limits() -> Value {
    json!({
        "signedIn": true, "plan": "Max", "haveUsage": true, "h5": 0.42, "week": 0.17,
        "resetH5": "2026-11-19T15:05:00Z", "resetWeek": "2026-11-24T12:00:00Z",
        "scoped": [{ "kind": "weekly", "label": "Fable", "used": 0.9, "resetsAt": "2026-11-24T12:00:00Z" }], "other": [],
        "fetchedAtMs": T0 as f64, "stale": false, "refreshError": null, "rateLimited": null
    })
}

fn codex_limits() -> Value {
    json!({
        "signedIn": true, "plan": "Pro", "haveUsage": true,
        "windows": [
            { "id": "primary", "label": "5 hours", "used": 0.9, "resetsAt": "2026-11-19T12:40:00Z" },
            { "id": "secondary", "label": "week", "used": 0.3, "resetsAt": "2026-11-22T12:00:00Z" }
        ],
        "fetchedAtMs": T0 as f64, "stale": false, "rateLimited": null
    })
}

/// The demo core as an update tells it (`desktop::CoreId`).
fn demo_core() -> Value {
    serde_json::to_value(desktop::CoreId::of(&demo::core())).unwrap()
}

/// The sessions' stop as the host answers it: stopped in the demo core.
fn stopped() -> Value {
    json!({ "stopped": 2, "core": demo_core() })
}

fn offer(core: &str) -> Value {
    json!({ "current": "0.20.0", "offer": { "version": "0.20.1", "notes": "- the status bar", "pubDate": null, "coreFingerprint": core, "generation": 7 } })
}

/// The download's answer as the host gives it: the core the setup's
/// signature names, the one its offer said unless a test says otherwise.
fn downloaded(core: &str) -> Value {
    json!({ "version": "0.20.1", "coreFingerprint": core })
}

/// A power state: held awake by `c-codex`, mid-turn, nothing armed.
fn holding() -> Value {
    json!({
        "keepAwake": true, "holding": true, "armed": null, "countdownEndsAtMs": null,
        "summary": { "busy": 1, "waiting": 0, "stalled": 0, "idleSinceMs": null, "sessions": [
            { "id": "c-codex", "turn": "busy", "procs": [], "outputFresh": false, "busy": true, "stalled": null }
        ] }
    })
}

/// The window over the fixture's chats (their sessions answered by
/// `sessions_meta` as in the board's tests), the accounts of the demo, the
/// core up, the clock held at `T0`.
fn open_with(cx: &mut TestAppContext, setup: impl FnOnce(&FakeHost)) -> S<'_> {
    cx.executor().allow_parking();
    let host = FakeHost::new();
    host.answer(host_cmd::CORE_STATE, up());
    host.answer(host_cmd::WINDOW_STATE, demo::kept(&demo::chats_state(2, true)));
    host.answer(host_cmd::LIST_ACCOUNTS, demo::accounts());
    host.answer(host_cmd::SESSIONS_META, demo::looks());
    host.answer(host_cmd::ENSURE_CLAUDE_SESSION, demo::attached("", 1));
    host.answer(host_cmd::ENSURE_CODEX_SESSION, demo::attached("", 1));
    host.answer(cmd::ACCOUNT_LIMITS, claude_limits());
    host.answer(cmd::CODEX_LIMITS, codex_limits());
    host.answer(cmd::ORPHAN_SESSIONS, json!([]));
    host.answer(cmd::POWER_STATE, serde_json::to_value(PowerState::default()).unwrap());
    host.answer(desktop::cmd::UPDATE_CHECK, json!({ "current": "0.20.0", "offer": null }));
    setup(&host);
    let (mailbox, inbox) = mailbox();
    let clock = Rc::new(Cell::new(T0));
    let models = cx.update(|cx| crate::start(host.clone(), inbox, ThemePref::Light, cx));
    let held = clock.clone();
    models.status.update(cx, |v, _| v.set_clock(Rc::new(move || held.get())));
    let window_models = models.clone();
    let (view, cx) = cx.add_window_view(move |window, cx| LunaWindow::new(window_models, window, cx));
    cx.run_until_parked();
    S { host, mailbox, models, view, clock, cx }
}

fn open(cx: &mut TestAppContext) -> S<'_> {
    open_with(cx, |_| {})
}

impl S<'_> {
    fn v<R>(&mut self, f: impl FnOnce(&Vitals) -> R) -> R {
        self.models.status.read_with(self.cx, |v, _| f(v))
    }

    fn do_v<R>(&mut self, f: impl FnOnce(&mut Vitals, &mut gpui::Context<Vitals>) -> R) -> R {
        let r = self.models.status.update(self.cx, f);
        self.cx.run_until_parked();
        r
    }

    /// The clock moves, and the executor's with it.
    fn tick(&mut self, d: Duration) {
        self.clock.set(self.clock.get() + d.as_millis() as u64);
        self.cx.executor().advance_clock(d);
        self.cx.run_until_parked();
    }

    /// The executor's time passes and the clock's does not: what an idle window
    /// does through hours that change no reading.
    fn wait(&mut self, d: Duration) {
        self.cx.executor().advance_clock(d);
        self.cx.run_until_parked();
    }

    /// The window's chats looked at again (the sessions' status come in).
    fn look(&mut self) {
        self.tick(crate::workspace::EVERY);
    }

    fn tell(&mut self, name: &str, payload: Value) {
        assert!(self.mailbox.event(name, payload));
        self.cx.run_until_parked();
    }

    #[cfg(debug_assertions)]
    fn click(&mut self, selector: &'static str) {
        let at = self.cx.debug_bounds(selector).unwrap_or_else(|| panic!("no {selector} drawn")).center();
        self.cx.simulate_click(at, gpui::Modifiers::none());
        self.cx.run_until_parked();
    }

    #[cfg(debug_assertions)]
    fn drawn(&mut self, selector: &'static str) -> bool {
        self.cx.debug_bounds(selector).is_some()
    }

    fn phase(&mut self) -> Update {
        self.v(|v| v.phase().clone())
    }

    fn popover(&mut self) -> Option<Popover> {
        self.v(Vitals::popover)
    }

    fn notifies(&mut self) -> usize {
        self.v(|v| v.notifies)
    }

    fn bar_renders(&mut self) -> usize {
        self.view.read_with(self.cx, |v, cx| v.status_bar.read(cx).renders)
    }

    fn active(&mut self) -> Option<String> {
        self.models.workspace.read_with(self.cx, |w, _| w.state.active.clone())
    }
}

// ----------------------------------------------------------------- counts

#[gpui::test]
fn the_counts_follow_the_chats_and_go_with_the_core(cx: &mut TestAppContext) {
    let mut s = open(cx);
    s.look();
    // The fixture's: two working, one waiting, one in the background.
    assert_eq!(s.v(Vitals::counts), Counts { working: 2, waiting: 1, background: 1 });
    // Where no core is up, nothing is known to work.
    let down = CoreState { state: LinkState::Down, core: None, error: Some("gone".into()), revision: 2 };
    s.tell(host_event::CORE_STATE, serde_json::to_value(down).unwrap());
    assert_eq!(s.v(Vitals::counts), Counts::default());
}

/// A power state whose one session, `c-old`, has this lifecycle record.
fn recorded(life: Value) -> Value {
    json!({
        "keepAwake": true, "holding": true, "armed": null, "countdownEndsAtMs": null,
        "summary": { "busy": 1, "waiting": 0, "stalled": 0, "idleSinceMs": null, "sessions": [
            { "id": "c-old", "turn": "busy", "procs": [], "outputFresh": false, "busy": true, "stalled": null, "life": life }
        ] }
    })
}

#[gpui::test]
fn the_counts_go_by_the_cores_lifecycle_record_where_it_has_one(cx: &mut TestAppContext) {
    let mut s = open(cx);
    s.look();
    // `c-old` has no live session in `sessions_meta` (resting), and nothing
    // says more of it: it counts as nothing.
    assert_eq!(s.v(Vitals::counts), Counts { working: 2, waiting: 1, background: 1 });
    // The core's record has it not known yet (a session with no hooks, no
    // transcript): unknown counts as active, so as working, as the power
    // popover says "mid-turn".
    s.tell(event::POWER_STATE, recorded(json!({ "phase": "unknown", "background": {} })));
    assert_eq!(s.v(Vitals::counts), Counts { working: 3, waiting: 1, background: 1 });
    // The record moves, and the bar with it.
    s.tell(event::POWER_STATE, recorded(json!({ "phase": "asking", "background": {} })));
    assert_eq!(s.v(Vitals::counts), Counts { working: 2, waiting: 2, background: 1 });
    s.tell(event::POWER_STATE, recorded(json!({ "phase": "ended", "background": { "shells": 1 } })));
    assert_eq!(s.v(Vitals::counts), Counts { working: 2, waiting: 1, background: 2 });
    s.tell(event::POWER_STATE, recorded(json!({ "phase": "ended", "background": {} })));
    assert_eq!(s.v(Vitals::counts), Counts { working: 2, waiting: 1, background: 1 });
    // A state with no record for it is no word on it.
    s.tell(event::POWER_STATE, holding());
    assert_eq!(s.v(Vitals::counts), Counts { working: 2, waiting: 1, background: 1 });
    // And nothing is known to work where no core is up.
    s.tell(event::POWER_STATE, recorded(json!({ "phase": "unknown" })));
    let down = CoreState { state: LinkState::Down, core: None, error: Some("gone".into()), revision: 2 };
    s.tell(host_event::CORE_STATE, serde_json::to_value(down).unwrap());
    assert_eq!(s.v(Vitals::counts), Counts::default());
}

/// What a row, a pane's title and the strip show for `id`.
fn shown(s: &mut S<'_>, id: &str) -> crate::state::Status {
    s.models.workspace.read_with(s.cx, |w, cx| {
        let chat = w.state.find(id).expect("the chat is there").clone();
        w.shown_status(&chat, cx)
    })
}

#[gpui::test]
fn a_row_and_a_title_say_what_the_counts_do(cx: &mut TestAppContext) {
    use crate::state::Status;
    let mut s = open(cx);
    s.look();
    // `c-old` has no meta: it rests, in the row as in the counts.
    assert_eq!(shown(&mut s, "c-old"), Status::Resting);
    // The core's record has it not known yet: working, as the bar counts it.
    s.tell(event::POWER_STATE, recorded(json!({ "phase": "unknown", "background": {} })));
    assert_eq!(shown(&mut s, "c-old"), Status::Working);
    assert_eq!(s.v(Vitals::counts).working, 3);
    // It moves with the record, and a chat with none keeps its meta's word.
    s.tell(event::POWER_STATE, recorded(json!({ "phase": "asking", "background": {} })));
    assert_eq!(shown(&mut s, "c-old"), Status::Waiting);
    s.tell(event::POWER_STATE, recorded(json!({ "phase": "ended", "background": { "crons": 1 } })));
    assert_eq!(shown(&mut s, "c-old"), Status::Background);
    assert_eq!(shown(&mut s, "c-parser"), Status::Waiting);
    // A state without the record gives the meta's word back.
    s.tell(event::POWER_STATE, holding());
    assert_eq!(shown(&mut s, "c-old"), Status::Resting);
    // And with no core up, no word at all.
    s.tell(event::POWER_STATE, recorded(json!({ "phase": "unknown" })));
    let down = CoreState { state: LinkState::Down, core: None, error: Some("gone".into()), revision: 2 };
    s.tell(host_event::CORE_STATE, serde_json::to_value(down).unwrap());
    assert_eq!(shown(&mut s, "c-old"), Status::NoCore);
}

#[cfg(debug_assertions)]
#[gpui::test]
fn waiting_goes_to_the_next_waiting_chat_and_background_and_settings_open_theirs(cx: &mut TestAppContext) {
    let mut s = open(cx);
    s.look();
    s.models.workspace.update(s.cx, |w, cx| w.show_chat("c-codex", cx));
    s.cx.run_until_parked();
    assert_eq!(s.active().as_deref(), Some("c-codex"));
    s.click("status-waiting");
    assert_eq!(s.active().as_deref(), Some("c-parser"), "the one chat that waits, as Ctrl+Shift+W goes to it");

    // The dialogs' layer plugs the Background count into the helpers'
    // popover (all chats' helpers), the panels' the Settings button into
    // the settings panel.
    let open = |s: &mut S<'_>| {
        let dialog = s.view.read_with(s.cx, |v, cx| v.dialogs.read(cx).shown());
        let panel = s.view.read_with(s.cx, |v, cx| v.panels.read(cx).shown());
        (dialog, panel)
    };
    assert_eq!(open(&mut s), (None, None));
    s.click("status-background");
    assert_eq!(open(&mut s), (Some("helpers"), None));
    s.cx.dispatch_action(crate::dialogs::Close);
    s.cx.run_until_parked();
    s.click("status-settings");
    assert_eq!(open(&mut s), (None, Some(crate::panels::Panel::Settings)));
}

/// A seam plugged by something else is the one called.
#[cfg(debug_assertions)]
#[gpui::test]
fn a_seam_replaced_is_the_one_the_buttons_call(cx: &mut TestAppContext) {
    let mut s = open(cx);
    let (helpers, settings) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
    let (h, st) = (helpers.clone(), settings.clone());
    s.models.workspace.update(s.cx, |w, _| {
        w.seams.helpers = Some(Rc::new(move |_: &mut Window, _: &mut App| h.set(h.get() + 1)));
        w.seams.settings = Some(Rc::new(move |_: &mut Window, _: &mut App| st.set(st.get() + 1)));
    });
    s.click("status-background");
    s.click("status-settings");
    s.click("status-settings");
    assert_eq!((helpers.get(), settings.get()), (1, 2));
}

// --------------------------------------------------------------- accounts

#[gpui::test]
fn each_account_is_asked_in_its_turn_and_then_on_the_pages_cadence(cx: &mut TestAppContext) {
    let mut s = open(cx);
    let asked = |s: &S<'_>| (s.host.calls_of(cmd::ACCOUNT_LIMITS).len(), s.host.calls_of(cmd::CODEX_LIMITS).len());
    // The first account at once; the second a gap later, not together.
    assert_eq!(asked(&s), (1, 0));
    s.tick(usage::GAP);
    assert_eq!(asked(&s), (1, 1));
    let args = s.host.calls_of(cmd::ACCOUNT_LIMITS).remove(0);
    assert_eq!(args, json!({ "accountPath": "C:\\accounts\\anthropic\\work", "force": false }));

    let accounts = s.v(|v| v.accounts().to_vec());
    assert_eq!(accounts.len(), 2);
    let work = &accounts[0];
    assert_eq!((work.plan.as_deref(), work.signed_in, work.have_usage), (Some("Max"), true, true));
    assert_eq!(
        work.windows.iter().map(|w| (w.short.as_str(), (w.used * 100.).round() as i64)).collect::<Vec<_>>(),
        [("5H", 42), ("W", 17), ("F", 90)]
    );
    assert_eq!(accounts[1].windows.iter().map(|w| w.short.as_str()).collect::<Vec<_>>(), ["5H", "W"]);
    assert_eq!(usage::fmt_reset(accounts[0].windows[0].reset_at.as_deref(), T0), "3h 5m");

    // A minute between rounds when every account has numbers; never sooner.
    s.tick(Duration::from_secs(56));
    assert_eq!(asked(&s), (1, 1));
    s.tick(Duration::from_secs(5));
    assert_eq!(asked(&s), (2, 1), "the next round began");
    s.tick(usage::GAP);
    assert_eq!(asked(&s), (2, 2));
}

#[gpui::test]
fn the_usage_goes_to_the_core_for_the_agents_once_it_changes(cx: &mut TestAppContext) {
    let mut s = open(cx);
    s.tick(usage::GAP);
    let sent = s.host.calls_of(cmd::ACCOUNT_USAGE);
    let last = sent.last().expect("sent").clone();
    let work = &last["usage"]["claude/work"];
    assert_eq!(work["signedIn"], true);
    assert_eq!(work["plan"], "Max");
    assert_eq!(work["limits"][0], json!({ "label": "5 hours", "usedPct": 42, "resetsAt": "2026-11-19T15:05:00Z" }));
    assert_eq!(work["takenAt"], "2026-11-19T12:00:00Z");
    assert_eq!(last["usage"]["codex/personal"]["limits"][1]["usedPct"], 30);
    // The same answers again say nothing new.
    let n = sent.len();
    s.tick(Duration::from_secs(70));
    s.tick(usage::GAP);
    assert_eq!(s.host.calls_of(cmd::ACCOUNT_USAGE).len(), n);
}

#[gpui::test]
fn an_account_that_failed_keeps_its_last_numbers_and_is_asked_less_often(cx: &mut TestAppContext) {
    let mut s = open(cx);
    s.tick(usage::GAP);
    let throttled = json!({ "signedIn": true, "plan": "Max", "haveUsage": false, "rateLimited": 30 });
    s.host.answer(cmd::ACCOUNT_LIMITS, throttled);
    s.tick(Duration::from_secs(65));
    let work = s.v(|v| v.accounts()[0].clone());
    assert_eq!(work.windows.len(), 3, "the meters stay");
    assert_eq!(work.sync, usage::Standing::Throttled);
    // After a 429 the next round waits three minutes, not one.
    let before = s.host.calls_of(cmd::ACCOUNT_LIMITS).len();
    s.tick(usage::GAP);
    s.tick(Duration::from_secs(170));
    assert_eq!(s.host.calls_of(cmd::ACCOUNT_LIMITS).len(), before);
    s.tick(Duration::from_secs(20));
    assert_eq!(s.host.calls_of(cmd::ACCOUNT_LIMITS).len(), before + 1);
}

// ----------------------------------------------------------------- update

#[cfg(debug_assertions)]
#[gpui::test]
fn a_release_lights_a_chip_that_opens_the_confirmation(cx: &mut TestAppContext) {
    let mut s = open_with(cx, |h| h.answer(desktop::cmd::UPDATE_CHECK, offer(CORE)));
    assert!(matches!(s.phase(), Update::Available(o) if o.version == "0.20.1"));
    assert_eq!(s.host.calls_of(desktop::cmd::UPDATE_CHECK).len(), 1, "one check, at start");
    assert!(s.drawn("status-update"));
    assert_eq!(s.popover(), None);
    s.click("status-update");
    assert_eq!(s.popover(), Some(Popover::Update));
    assert!(s.drawn("update-panel") && s.drawn("update-notes") && s.drawn("update-install"));
    // Later closes it; the chip stays.
    s.click("update-later");
    assert_eq!(s.popover(), None);
    assert!(s.drawn("status-update"));
}

#[cfg(debug_assertions)]
#[gpui::test]
fn no_release_no_chip(cx: &mut TestAppContext) {
    let mut s = open(cx);
    assert_eq!(s.phase(), Update::Idle);
    assert!(!s.drawn("status-update"));
}

#[cfg(debug_assertions)]
#[gpui::test]
fn a_startup_check_that_fails_is_quiet(cx: &mut TestAppContext) {
    let mut s = open_with(cx, |h| h.defer(desktop::cmd::UPDATE_CHECK));
    assert!(s.host.resolve(desktop::cmd::UPDATE_CHECK, Err("offline".into())));
    s.cx.run_until_parked();
    assert_eq!(s.phase(), Update::Idle);
    assert!(!s.drawn("status-update"));
}

#[cfg(debug_assertions)]
#[gpui::test]
fn an_install_that_keeps_the_core_downloads_then_installs_without_stopping(cx: &mut TestAppContext) {
    let mut s = open_with(cx, |h| {
        h.answer(desktop::cmd::UPDATE_CHECK, offer(CORE));
        h.defer(desktop::cmd::UPDATE_DOWNLOAD);
        h.defer(desktop::cmd::UPDATE_INSTALL);
    });
    s.click("status-update");
    s.click("update-install");
    assert!(matches!(s.phase(), Update::Downloading { got: 0, total: None, .. }));
    assert_eq!(s.popover(), None);
    assert_eq!(s.host.calls_of(desktop::cmd::UPDATE_DOWNLOAD).len(), 1);
    // The download's progress, as the host tells it.
    s.tell(desktop::event::UPDATE_PROGRESS, json!({ "got": 50, "total": 200 }));
    assert!(matches!(s.phase(), Update::Downloading { got: 50, total: Some(200), .. }));
    assert_eq!(super::updates::chip(&s.phase()).unwrap().1, "downloading 25%");
    // A second click, or a check meanwhile, starts nothing.
    s.do_v(|v, cx| {
        install_as_shown(v, cx);
        v.check(true, cx);
    });
    assert_eq!(s.host.calls_of(desktop::cmd::UPDATE_DOWNLOAD).len(), 1);
    assert_eq!(s.host.calls_of(desktop::cmd::UPDATE_CHECK).len(), 1);

    // The setup's signature names the running core too.
    assert!(s.host.resolve(desktop::cmd::UPDATE_DOWNLOAD, Ok(json!({ "version": "0.20.1", "coreFingerprint": CORE }))));
    s.cx.run_until_parked();
    assert!(matches!(s.phase(), Update::Installing(_)));
    assert!(s.host.calls_of(cmd::STOP_ALL_SESSIONS).is_empty(), "the release's core is the running one: the sessions stay");
    let install = s.host.calls_of(desktop::cmd::UPDATE_INSTALL);
    assert_eq!(
        install,
        [json!({
            "generation": 7,
            "reason": "installing the update to 0.20.1; the sessions keep running in the core",
            "anyway": false,
            "core": demo_core(),
            "keep": true
        })]
    );
}

/// latest.json, which is not signed, says the release keeps the running
/// core; the setup's signature says another. Nothing is stopped or
/// installed on latest.json's word: the update fails with the offer as
/// signed, the panel now says the sessions stop, and the next try stops
/// them first.
#[cfg(debug_assertions)]
#[gpui::test]
fn the_signed_core_decides_and_one_latest_json_misstated_is_asked_again(cx: &mut TestAppContext) {
    let mut s = open_with(cx, |h| {
        h.answer(desktop::cmd::UPDATE_CHECK, offer(CORE));
        h.defer(desktop::cmd::UPDATE_DOWNLOAD);
        h.defer(cmd::STOP_ALL_SESSIONS);
        h.defer(desktop::cmd::UPDATE_INSTALL);
    });
    // A session runs, which the confirmation says keeps running.
    s.tell(event::POWER_STATE, holding());
    s.click("status-update");
    s.click("update-install");
    assert!(s.host.resolve(desktop::cmd::UPDATE_DOWNLOAD, Ok(json!({ "version": "0.20.1", "coreFingerprint": "fedcba9876543210" }))));
    s.cx.run_until_parked();
    let Update::Failed { offer: Some(signed), why } = s.phase() else { panic!("{:?}", s.phase()) };
    assert_eq!(signed.core_fingerprint.as_deref(), Some("fedcba9876543210"));
    assert!(why.contains("another core than the running one"), "{why}");
    assert!(s.host.calls_of(cmd::STOP_ALL_SESSIONS).is_empty());
    assert!(s.host.calls_of(desktop::cmd::UPDATE_INSTALL).is_empty());
    assert!(s.host.calls_of(cmd::UPDATE_CALLED_OFF).is_empty(), "no session was stopped, so nothing to call off");

    // Asked again, on the signed core: the sessions stop before the install.
    s.click("status-update");
    s.click("update-install");
    assert_eq!(s.host.calls_of(desktop::cmd::UPDATE_DOWNLOAD).len(), 2);
    assert!(s.host.resolve(desktop::cmd::UPDATE_DOWNLOAD, Ok(json!({ "version": "0.20.1", "coreFingerprint": "fedcba9876543210" }))));
    s.cx.run_until_parked();
    assert!(matches!(s.phase(), Update::Stopping(_)));
    assert!(s.host.resolve(cmd::STOP_ALL_SESSIONS, Ok(stopped())));
    s.cx.run_until_parked();
    assert_eq!(
        s.host.calls_of(desktop::cmd::UPDATE_INSTALL),
        [json!({ "generation": 7, "reason": "installing the update to 0.20.1", "anyway": false, "core": demo_core(), "keep": false })]
    );
}

/// The confirmation said the sessions keep running, and when the download
/// is through the core is not connected (being found again, say): the
/// window cannot tell whether the core it was drawn for comes back, so the
/// stop carries that core and nothing to stop there, and the host and the
/// core refuse it otherwise. Their refusal calls the update off with the
/// offer kept; nothing is installed.
#[cfg(debug_assertions)]
#[gpui::test]
fn a_promise_to_keep_the_sessions_holds_the_stop_to_nothing_when_the_core_is_gone(cx: &mut TestAppContext) {
    let mut s = open_with(cx, |h| {
        h.answer(desktop::cmd::UPDATE_CHECK, offer(CORE));
        h.defer(desktop::cmd::UPDATE_DOWNLOAD);
        h.defer(cmd::STOP_ALL_SESSIONS);
        h.defer(desktop::cmd::UPDATE_INSTALL);
    });
    s.tell(event::POWER_STATE, holding());
    s.click("status-update");
    s.click("update-install");
    s.tell(host_event::CORE_STATE, down(2));
    assert!(s.host.resolve(desktop::cmd::UPDATE_DOWNLOAD, Ok(downloaded(CORE))));
    s.cx.run_until_parked();
    assert_eq!(s.host.calls_of(cmd::STOP_ALL_SESSIONS), [json!({ "core": demo_core(), "confirmed": [] })]);
    assert!(s.host.resolve(cmd::STOP_ALL_SESSIONS, Err("a session runs that the update's confirmation did not count".into())));
    s.cx.run_until_parked();
    assert!(matches!(s.phase(), Update::Failed { offer: Some(_), .. }), "{:?}", s.phase());
    assert!(s.host.calls_of(desktop::cmd::UPDATE_INSTALL).is_empty());
    assert!(!frozen(&mut s));
}

/// The other way round: latest.json names another core, the signature the
/// running one. The signed one decides, and the sessions keep running.
#[cfg(debug_assertions)]
#[gpui::test]
fn a_release_signed_for_the_running_core_keeps_the_sessions(cx: &mut TestAppContext) {
    let mut s = open_with(cx, |h| {
        h.answer(desktop::cmd::UPDATE_CHECK, offer("another-core"));
        h.answer(desktop::cmd::UPDATE_DOWNLOAD, json!({ "version": "0.20.1", "coreFingerprint": CORE }));
        h.defer(desktop::cmd::UPDATE_INSTALL);
    });
    s.click("status-update");
    s.click("update-install");
    s.cx.run_until_parked();
    assert!(matches!(s.phase(), Update::Installing(_)));
    assert!(s.host.calls_of(cmd::STOP_ALL_SESSIONS).is_empty());
    assert_eq!(
        s.host.calls_of(desktop::cmd::UPDATE_INSTALL)[0]["reason"],
        "installing the update to 0.20.1; the sessions keep running in the core"
    );
}

#[cfg(debug_assertions)]
#[gpui::test]
fn another_core_stops_the_sessions_first_and_a_failed_download_costs_nothing(cx: &mut TestAppContext) {
    let mut s = open_with(cx, |h| {
        h.answer(desktop::cmd::UPDATE_CHECK, offer("another-core"));
        h.defer(desktop::cmd::UPDATE_DOWNLOAD);
        h.defer(cmd::STOP_ALL_SESSIONS);
        h.defer(desktop::cmd::UPDATE_INSTALL);
    });
    s.click("status-update");
    s.click("update-install");
    // The download fails: nothing stopped, the chip says so, and its click
    // offers the install again.
    assert!(s.host.resolve(desktop::cmd::UPDATE_DOWNLOAD, Err("no space left".into())));
    s.cx.run_until_parked();
    assert!(matches!(s.phase(), Update::Failed { offer: Some(_), .. }));
    assert!(s.host.calls_of(cmd::STOP_ALL_SESSIONS).is_empty());
    assert!(s.host.calls_of(cmd::UPDATE_CALLED_OFF).is_empty(), "no session was stopped, so nothing to call off");
    s.click("status-update");
    assert_eq!(s.popover(), Some(Popover::Update));
    s.click("update-install");
    assert_eq!(s.host.calls_of(desktop::cmd::UPDATE_DOWNLOAD).len(), 2);

    // This time it downloads: every session is asked to quit, and only then
    // is the install asked for.
    assert!(s.host.resolve(desktop::cmd::UPDATE_DOWNLOAD, Ok(downloaded("another-core"))));
    s.cx.run_until_parked();
    assert!(matches!(s.phase(), Update::Stopping(_)));
    assert_eq!(super::updates::chip(&s.phase()).unwrap().1, "stopping sessions…");
    assert!(s.host.calls_of(desktop::cmd::UPDATE_INSTALL).is_empty());
    assert!(s.host.resolve(cmd::STOP_ALL_SESSIONS, Ok(stopped())));
    s.cx.run_until_parked();
    assert_eq!(
        s.host.calls_of(desktop::cmd::UPDATE_INSTALL),
        [json!({ "generation": 7, "reason": "installing the update to 0.20.1", "anyway": false, "core": demo_core(), "keep": false })]
    );
}

#[cfg(debug_assertions)]
#[gpui::test]
fn a_stop_that_fails_calls_the_update_off_so_the_core_thaws_the_chats(cx: &mut TestAppContext) {
    let mut s = open_with(cx, |h| {
        h.answer(desktop::cmd::UPDATE_CHECK, offer("another-core"));
        h.answer(desktop::cmd::UPDATE_DOWNLOAD, downloaded("another-core"));
        h.defer(cmd::STOP_ALL_SESSIONS);
    });
    s.click("status-update");
    s.click("update-install");
    assert!(s.host.resolve(cmd::STOP_ALL_SESSIONS, Err("the core did not answer".into())));
    s.cx.run_until_parked();
    assert!(matches!(s.phase(), Update::Failed { offer: Some(_), .. }));
    assert_eq!(s.host.calls_of(cmd::UPDATE_CALLED_OFF).len(), 1);
    assert!(s.host.calls_of(desktop::cmd::UPDATE_INSTALL).is_empty(), "nothing was installed");
}

#[cfg(debug_assertions)]
#[gpui::test]
fn an_install_refused_as_unsaved_offers_retry_install_anyway_or_cancel(cx: &mut TestAppContext) {
    let mut s = open_with(cx, |h| {
        h.answer(desktop::cmd::UPDATE_CHECK, offer("another-core"));
        h.answer(desktop::cmd::UPDATE_DOWNLOAD, downloaded("another-core"));
        h.answer(cmd::STOP_ALL_SESSIONS, stopped());
        h.defer(desktop::cmd::UPDATE_INSTALL);
    });
    s.click("status-update");
    s.click("update-install");
    // The sessions are stopped by now, and the install is asked for.
    assert_eq!(s.host.calls_of(cmd::STOP_ALL_SESSIONS).len(), 1);
    assert_eq!(s.host.calls_of(desktop::cmd::UPDATE_INSTALL).len(), 1);
    assert!(s.host.resolve(desktop::cmd::UPDATE_INSTALL, Err("unsaved: chats.json is locked (os error 32)".into())));
    s.cx.run_until_parked();
    let Update::Unsaved { why, keep, .. } = s.phase() else { panic!("{:?}", s.phase()) };
    assert_eq!((why.as_str(), keep), ("chats.json is locked (os error 32)", false));
    assert_eq!(s.popover(), Some(Popover::Update), "the choice is put in front of the person");
    assert!(s.drawn("update-retry") && s.drawn("update-anyway") && s.drawn("update-cancel"));

    // Retry: the same install again, refusing to lose anything.
    s.host.defer(desktop::cmd::UPDATE_INSTALL);
    s.click("update-retry");
    assert!(matches!(s.phase(), Update::Installing(_)));
    let installs = s.host.calls_of(desktop::cmd::UPDATE_INSTALL);
    assert_eq!(installs.len(), 2);
    assert_eq!(installs[1]["anyway"], false);
    assert!(s.host.resolve(desktop::cmd::UPDATE_INSTALL, Err("unsaved: still locked".into())));
    s.cx.run_until_parked();
    assert!(matches!(s.phase(), Update::Unsaved { .. }));

    // Install anyway: the person's choice, passed on.
    s.host.defer(desktop::cmd::UPDATE_INSTALL);
    s.click("update-anyway");
    assert!(matches!(s.phase(), Update::Installing(_)));
    assert_eq!(s.host.calls_of(desktop::cmd::UPDATE_INSTALL)[2]["anyway"], true);
    assert!(s.host.resolve(desktop::cmd::UPDATE_INSTALL, Err("unsaved: still locked".into())));
    s.cx.run_until_parked();

    // Cancel: the sessions were stopped for it, so it is called off, and the
    // release stays on offer.
    s.click("update-cancel");
    assert_eq!(s.host.calls_of(cmd::UPDATE_CALLED_OFF).len(), 1);
    assert!(matches!(s.phase(), Update::Available(_)));
    assert_eq!(s.popover(), None);
}

#[cfg(debug_assertions)]
#[gpui::test]
fn any_other_install_failure_calls_the_update_off_and_can_be_tried_again(cx: &mut TestAppContext) {
    let mut s = open_with(cx, |h| {
        h.answer(desktop::cmd::UPDATE_CHECK, offer("another-core"));
        h.answer(desktop::cmd::UPDATE_DOWNLOAD, downloaded("another-core"));
        h.answer(cmd::STOP_ALL_SESSIONS, stopped());
        h.defer(desktop::cmd::UPDATE_INSTALL);
    });
    s.click("status-update");
    s.click("update-install");
    assert!(s.host.resolve(desktop::cmd::UPDATE_INSTALL, Err("the installer was blocked".into())));
    s.cx.run_until_parked();
    assert!(matches!(s.phase(), Update::Failed { offer: Some(_), ref why } if why == "the installer was blocked"));
    assert_eq!(s.host.calls_of(cmd::UPDATE_CALLED_OFF).len(), 1, "the sessions it stopped resume");
    // The chip's click offers the install again.
    s.click("status-update");
    assert_eq!(s.popover(), Some(Popover::Update));
    assert!(s.drawn("update-install"));
}

#[cfg(debug_assertions)]
#[gpui::test]
fn a_check_that_was_asked_for_reports_its_failure_on_the_chip(cx: &mut TestAppContext) {
    let mut s = open(cx);
    s.host.defer(desktop::cmd::UPDATE_CHECK);
    s.do_v(|v, cx| v.check(true, cx));
    assert!(s.host.resolve(desktop::cmd::UPDATE_CHECK, Err("offline".into())));
    s.cx.run_until_parked();
    assert!(matches!(s.phase(), Update::Failed { offer: None, .. }));
    assert!(s.drawn("status-update"));
    // The click looks again.
    s.click("status-update");
    assert!(s.host.resolve(desktop::cmd::UPDATE_CHECK, Ok(offer(CORE))));
    s.cx.run_until_parked();
    assert!(matches!(s.phase(), Update::Available(_)));
}

// ------------------------------------------------------------------- CLIs

#[cfg(debug_assertions)]
#[gpui::test]
fn a_cli_chip_shows_a_download_or_a_failure_and_nothing_else(cx: &mut TestAppContext) {
    let mut s = open(cx);
    assert!(!s.drawn("status-cli-claude") && !s.drawn("status-cli-codex"));
    let status = |provider: &str, phase: &str, version: Value| json!({ "provider": provider, "phase": phase, "version": version, "got": 5, "total": 10, "error": null });
    s.tell(event::CLI_STATUS, status("claude", "downloading", Value::Null));
    assert!(s.drawn("status-cli-claude") && !s.drawn("status-cli-codex"));
    s.tell(event::CLI_STATUS, status("codex", "checking", json!("0.1.0")));
    assert!(!s.drawn("status-cli-codex"), "a check is no news");
    s.tell(event::CLI_STATUS, status("claude", "idle", json!("2.1.0")));
    assert!(!s.drawn("status-cli-claude"));
    s.tell(event::CLI_STATUS, json!({ "provider": "codex", "phase": "error", "version": "0.1.0", "error": "offline" }));
    assert!(s.drawn("status-cli-codex"));
    // The failed one's click tries again.
    s.click("status-cli-codex");
    assert_eq!(s.host.calls_of(cmd::CLI_UPDATE_NOW), [json!({ "provider": "codex" })]);
    // The first look at start asked both.
    assert_eq!(s.host.calls_of(cmd::CLI_STATUS).len(), 2);
}

// ------------------------------------------------------------------ power

#[gpui::test]
fn the_power_chip_follows_the_core_and_the_tray_is_told_who_holds_the_pc(cx: &mut TestAppContext) {
    let mut s = open(cx);
    // The plain icon first, as the page sends it at start.
    assert_eq!(s.host.sends_of(desktop::cmd::TRAY_AWAKE), [json!({ "holders": null })]);
    let chip = |s: &mut S<'_>| s.v(|v| power::chip(v.power(), v.clock_ms()));
    assert_eq!(chip(&mut s).text, "Power");
    s.tell(event::POWER_STATE, holding());
    assert_eq!(chip(&mut s).text, "awake · 1");
    let sends = s.host.sends_of(desktop::cmd::TRAY_AWAKE);
    assert_eq!(sends.len(), 2);
    let name = s.models.workspace.read_with(s.cx, |w, _| w.state.find("c-codex").unwrap().name.clone());
    assert_eq!(sends[1], json!({ "holders": format!("{name} — mid-turn") }));
    // The same state again changes nothing, nor tells the tray again.
    let n = s.notifies();
    s.tell(event::POWER_STATE, holding());
    assert_eq!((s.notifies(), s.host.sends_of(desktop::cmd::TRAY_AWAKE).len()), (n, 2));
    // Released: the dot goes.
    s.tell(event::POWER_STATE, json!({ "keepAwake": true, "holding": false }));
    assert_eq!(s.host.sends_of(desktop::cmd::TRAY_AWAKE).last(), Some(&json!({ "holders": null })));
    // The core's own answer at start is taken too.
    assert_eq!(s.host.calls_of(cmd::POWER_STATE).len(), 1, "asked once the core was up");
}

#[cfg(debug_assertions)]
#[gpui::test]
fn the_panel_switches_keep_awake_and_arms_and_disarms_the_rule(cx: &mut TestAppContext) {
    let mut s = open(cx);
    s.click("status-power");
    assert_eq!(s.popover(), Some(Popover::Power));
    assert!(s.drawn("power-panel"));
    // Keep awake off: the host's answer is the new state.
    s.host.answer(cmd::SET_KEEP_AWAKE, json!({ "keepAwake": false }));
    s.click("power-keep-awake");
    assert_eq!(s.host.calls_of(cmd::SET_KEEP_AWAKE), [json!({ "on": false })]);
    assert!(!s.v(|v| v.power().keep_awake));
    s.click("power-keep-awake");
    assert_eq!(s.host.calls_of(cmd::SET_KEEP_AWAKE).len(), 2);

    // The rule: what is chosen is what is armed.
    s.click("power-action-sleep");
    s.click("power-quiet-300");
    s.click("power-drain");
    let armed = PowerState {
        armed: Some(Armed { action: PowerAction::Sleep, quiet_s: 300, armed_at_ms: T0, drain: true }),
        ..PowerState::default()
    };
    s.host.answer(cmd::ARM_POWER_OFF, serde_json::to_value(&armed).unwrap());
    s.click("power-arm");
    assert_eq!(s.host.calls_of(cmd::ARM_POWER_OFF), [json!({ "action": "sleep", "quietS": 300, "drain": true })]);
    assert_eq!(s.popover(), None, "arming closes the panel");
    let said = s.v(|v| power::chip(v.power(), v.clock_ms()).text);
    assert_eq!(said, "sleep when done · draining");

    // Reopened, it shows the armed rule and disarms it.
    s.click("status-power");
    assert!(s.drawn("power-disarm") && !s.drawn("power-arm"));
    s.host.answer(cmd::DISARM_POWER_OFF, serde_json::to_value(PowerState::default()).unwrap());
    s.click("power-disarm");
    assert_eq!(s.host.calls_of(cmd::DISARM_POWER_OFF).len(), 1);
    assert!(s.v(|v| v.power().armed.is_none()));
    // Esc closes the panel.
    assert!(s.do_v(|v, cx| v.close_popover(cx)));
}

#[cfg(debug_assertions)]
#[gpui::test]
fn a_countdown_ticks_once_a_second_while_it_runs_and_cancel_ends_it(cx: &mut TestAppContext) {
    let mut s = open(cx);
    // The accounts' first round is over, so what follows is the clock alone.
    s.tick(usage::GAP);
    let counting = |ends: u64| {
        json!({
            "keepAwake": true, "holding": false,
            "armed": { "action": "shutdown", "quietS": 120, "armedAtMs": T0, "drain": false },
            "countdownEndsAtMs": ends,
            "summary": { "busy": 0, "waiting": 0, "stalled": 0, "idleSinceMs": null, "sessions": [] }
        })
    };
    let now = s.clock.get();
    s.tell(event::POWER_STATE, counting(now + 60_000));
    assert_eq!(s.v(Vitals::countdown_left), Some(60));
    assert!(s.drawn("status-power-cancel"));
    let said = |s: &mut S<'_>| s.v(|v| power::chip(v.power(), v.clock_ms()).text);
    assert_eq!(said(&mut s), "shutdown in 60s");
    let renders = s.bar_renders();
    s.tick(Duration::from_secs(1));
    assert_eq!(said(&mut s), "shutdown in 59s");
    s.tick(Duration::from_secs(1));
    assert_eq!(said(&mut s), "shutdown in 58s");
    assert_eq!(s.bar_renders(), renders + 2, "one redraw a second, no more");

    // Cancel is the rule disarmed; the countdown stops and nothing ticks on.
    s.host.answer(cmd::DISARM_POWER_OFF, serde_json::to_value(PowerState::default()).unwrap());
    s.click("status-power-cancel");
    assert_eq!(s.host.calls_of(cmd::DISARM_POWER_OFF).len(), 1);
    assert!(!s.drawn("status-power-cancel"));
    let idle = (s.bar_renders(), s.notifies());
    s.tick(Duration::from_secs(5));
    s.tick(Duration::from_secs(5));
    assert_eq!((s.bar_renders(), s.notifies()), idle, "no clock runs once the countdown is gone");

    // One that ends by itself (the host fired it, or something woke) stops
    // the clock as well.
    let now = s.clock.get();
    s.tell(event::POWER_STATE, counting(now + 100_000));
    s.tell(event::POWER_STATE, json!({ "keepAwake": true, "holding": false }));
    let idle = (s.bar_renders(), s.notifies());
    s.tick(Duration::from_secs(10));
    assert_eq!((s.bar_renders(), s.notifies()), idle);
}

// ----------------------------------------------------------------- orphans

#[cfg(debug_assertions)]
#[gpui::test]
fn the_orphan_line_shows_only_while_there_are_orphans_and_kills_after_asking(cx: &mut TestAppContext) {
    let rows = json!([
        { "id": "s-1", "provider": "claude", "pid": 11, "cwd": "C:\\work\\a", "accountPath": "C:\\accounts\\anthropic\\work", "title": "lost", "status": "busy" },
        { "id": "s-2", "provider": "codex", "pid": 12, "cwd": "C:\\work\\b", "accountPath": "C:\\accounts\\openai\\personal", "title": null, "status": null }
    ]);
    let mut s = open(cx);
    assert!(!s.drawn("orphan-line"), "none, no line");
    // The scan was told every chat's id.
    let known = s.host.calls_of(cmd::ORPHAN_SESSIONS).pop().expect("looked once the chats had loaded");
    let mut ids: Vec<String> = known["known"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
    assert!(ids.contains(&"c-parser".to_owned()));
    let sorted = ids.clone();
    ids.sort();
    assert_eq!(ids, sorted, "in a stable order");

    s.host.answer(cmd::ORPHAN_SESSIONS, rows.clone());
    s.tick(super::vitals::ORPHANS_EVERY);
    assert!(s.drawn("orphan-line") && s.drawn("orphans-kill-all"));
    assert_eq!(s.v(|v| v.orphans().len()), 2);

    // The first click asks; Keep goes back; nothing is killed yet.
    s.click("orphans-kill-all");
    assert!(s.drawn("orphans-kill") && s.drawn("orphans-keep") && !s.drawn("orphans-kill-all"));
    assert!(s.host.calls_of(cmd::KILL_SESSION).is_empty());
    s.click("orphans-keep");
    assert!(s.drawn("orphans-kill-all"));

    // The second looks again, kills each of the sessions asked about, and
    // the next look finds none: the line goes.
    s.click("orphans-kill-all");
    s.host.defer(cmd::ORPHAN_SESSIONS);
    s.click("orphans-kill");
    assert!(s.host.calls_of(cmd::KILL_SESSION).is_empty(), "not before the look again");
    assert!(s.host.resolve(cmd::ORPHAN_SESSIONS, Ok(rows)));
    s.cx.run_until_parked();
    assert_eq!(s.host.calls_of(cmd::KILL_SESSION), [json!({ "id": "s-1" }), json!({ "id": "s-2" })]);
    assert!(s.host.resolve(cmd::ORPHAN_SESSIONS, Ok(json!([]))));
    s.cx.run_until_parked();
    assert!(!s.drawn("orphan-line"));
}

#[cfg(debug_assertions)]
#[gpui::test]
fn an_adopted_chat_is_no_orphan_and_no_core_means_none(cx: &mut TestAppContext) {
    let mut s = open_with(cx, |h| {
        h.answer(cmd::ORPHAN_SESSIONS, json!([{ "id": "s-1", "provider": "claude", "cwd": "C:\\w", "accountPath": "C:\\a" }]));
    });
    assert!(s.drawn("orphan-line"));
    // One look at a time; one asked meanwhile goes once it is done, and the
    // older one's answer is let go of: only the latest answers.
    s.host.defer(cmd::ORPHAN_SESSIONS);
    let before = s.host.calls_of(cmd::ORPHAN_SESSIONS).len();
    s.do_v(|v, cx| {
        v.scan(cx);
        v.scan(cx);
        v.scan(cx);
    });
    assert_eq!(s.host.calls_of(cmd::ORPHAN_SESSIONS).len(), before + 1);
    assert!(s.host.resolve(cmd::ORPHAN_SESSIONS, Ok(json!([]))));
    s.cx.run_until_parked();
    assert_eq!(s.host.calls_of(cmd::ORPHAN_SESSIONS).len(), before + 2);
    assert!(s.drawn("orphan-line"), "the older answer changed nothing");
    assert!(
        s.host.resolve(cmd::ORPHAN_SESSIONS, Ok(json!([{ "id": "s-9", "provider": "codex", "cwd": "C:\\w", "accountPath": "C:\\a" }])))
    );
    s.cx.run_until_parked();
    assert_eq!(s.v(|v| v.orphans().iter().map(|o| o.id.clone()).collect::<Vec<_>>()), ["s-9"]);
    // The core going takes the line with it.
    let down = CoreState { state: LinkState::Down, core: None, error: Some("gone".into()), revision: 2 };
    s.tell(host_event::CORE_STATE, serde_json::to_value(down).unwrap());
    assert!(!s.drawn("orphan-line"));
}

// -------------------------------------------------------------------- idle

#[gpui::test]
fn an_idle_window_draws_nothing_however_long_it_waits(cx: &mut TestAppContext) {
    let mut s = open_with(cx, |h| h.answer(desktop::cmd::UPDATE_CHECK, offer(CORE)));
    s.look();
    s.tick(usage::GAP);
    s.tell(event::POWER_STATE, holding());
    let (renders, notifies) = (s.bar_renders(), s.notifies());
    let (limits, scans) = (s.host.calls_of(cmd::ACCOUNT_LIMITS).len(), s.host.calls_of(cmd::ORPHAN_SESSIONS).len());
    // An hour: the polls run on their slow schedules and find nothing new.
    for _ in 0..60 {
        s.wait(Duration::from_secs(60));
    }
    assert!(s.host.calls_of(cmd::ACCOUNT_LIMITS).len() > limits, "the accounts were looked at");
    assert!(s.host.calls_of(cmd::ORPHAN_SESSIONS).len() > scans, "the orphans were looked for");
    assert_eq!((s.bar_renders(), s.notifies()), (renders, notifies), "and not one redraw came of it");
    // One check at start and no other: the page's schedule.
    assert_eq!(s.host.calls_of(desktop::cmd::UPDATE_CHECK).len(), 1);
}

/// Needs the selectors gpui records only with debug assertions, as the other
/// click tests do.
#[cfg(debug_assertions)]
#[gpui::test]
fn every_field_of_the_bar_is_drawn(cx: &mut TestAppContext) {
    let mut s = open(cx);
    s.look();
    for selector in ["status-waiting", "status-background", "status-accounts", "status-power", "status-settings"] {
        assert!(s.drawn(selector), "{selector}");
    }
}

// ----------------------------------------------- power: reach and fences

/// A countdown armed for a shutdown, as the core numbers its states.
fn counting(ends: u64, revision: u64) -> Value {
    json!({
        "keepAwake": true, "holding": false,
        "armed": { "action": "shutdown", "quietS": 120, "armedAtMs": T0, "drain": false },
        "countdownEndsAtMs": ends, "revision": revision,
        "summary": { "busy": 0, "waiting": 0, "stalled": 0, "idleSinceMs": null, "sessions": [] }
    })
}

fn down(revision: u64) -> Value {
    serde_json::to_value(CoreState { state: LinkState::Down, core: None, error: Some("gone".into()), revision }).unwrap()
}

/// The core up at `revision`, the demo's but for its pid: another process
/// unless `pid` is the demo's.
fn up_as(revision: u64, pid: u32) -> Value {
    let core = crate::dto::Core { pid, ..demo::core() };
    serde_json::to_value(CoreState { state: LinkState::Up, core: Some(core), error: None, revision }).unwrap()
}

fn up_again(revision: u64, fingerprint: &str) -> Value {
    let core = crate::dto::Core { fingerprint: fingerprint.into(), ..demo::core() };
    serde_json::to_value(CoreState { state: LinkState::Up, core: Some(core), error: None, revision }).unwrap()
}

#[gpui::test]
fn a_core_out_of_reach_leaves_the_power_rule_unknown_with_no_countdown(cx: &mut TestAppContext) {
    let mut s = open(cx);
    s.tick(usage::GAP);
    let now = s.clock.get();
    s.tell(event::POWER_STATE, counting(now + 60_000, 3));
    assert_eq!(s.v(Vitals::countdown_left), Some(60));
    s.tell(host_event::CORE_STATE, down(2));
    assert!(!s.v(Vitals::power_known));
    assert_eq!(s.v(Vitals::countdown_left), None, "a countdown that may be over, or still running out of reach");
    assert_eq!(s.v(|v| v.power_chip()), power::Chip { kind: power::Kind::Unknown, text: "power: no core".into() });
    // No clock runs for it, and nothing it is told meanwhile is taken.
    let idle = s.notifies();
    s.tick(Duration::from_secs(5));
    s.tell(event::POWER_STATE, counting(now + 60_000, 4));
    assert_eq!((s.notifies(), s.v(Vitals::power_known)), (idle, false));
    // The core back: asked again, and its answer is the rule.
    s.tell(host_event::CORE_STATE, up_again(3, CORE));
    assert_eq!(s.host.calls_of(cmd::POWER_STATE).len(), 2);
    assert!(s.v(Vitals::power_known));
    assert_eq!(s.v(|v| v.power_chip().text), "Power");
}

#[gpui::test]
fn a_late_answer_cannot_put_back_a_canceled_countdown(cx: &mut TestAppContext) {
    let mut s = open(cx);
    let now = s.clock.get();
    s.tell(event::POWER_STATE, counting(now + 60_000, 3));
    // A call answered late, with the state from before the cancel the core
    // told of meanwhile.
    s.host.defer(cmd::SET_KEEP_AWAKE);
    s.do_v(|v, cx| v.set_keep_awake(true, cx));
    s.tell(event::POWER_STATE, json!({ "keepAwake": true, "revision": 5 }));
    assert_eq!(s.v(Vitals::countdown_left), None);
    assert!(s.host.resolve(cmd::SET_KEEP_AWAKE, Ok(counting(now + 60_000, 4))));
    s.cx.run_until_parked();
    assert_eq!(s.v(Vitals::countdown_left), None, "older than the state shown: dropped");
    // One asked of a core since replaced is dropped, whatever its number.
    s.do_v(|v, cx| v.set_keep_awake(true, cx));
    s.tell(host_event::CORE_STATE, down(2));
    s.tell(host_event::CORE_STATE, up_again(3, "fedcba9876543210"));
    assert!(s.host.resolve(cmd::SET_KEEP_AWAKE, Ok(counting(now + 60_000, 99))));
    s.cx.run_until_parked();
    assert_eq!(s.v(Vitals::countdown_left), None);
    // The new core's states count from 1 again.
    s.tell(event::POWER_STATE, counting(now + 30_000, 1));
    assert_eq!(s.v(Vitals::countdown_left), Some(30));
}

/// `counting`, stamped as the native host stamps what it reads off the
/// core's line: with the link revision of the core's `up`
/// (`event::stamp`).
fn counting_from(core: u64, ends: u64, revision: u64) -> Value {
    let mut state = counting(ends, revision);
    state["core"] = json!(core);
    state
}

#[gpui::test]
fn a_power_state_of_a_core_since_replaced_is_dropped_whatever_its_number(cx: &mut TestAppContext) {
    let mut s = open(cx);
    let now = s.clock.get();
    s.tell(event::POWER_STATE, counting_from(1, now + 60_000, 3));
    assert_eq!(s.v(Vitals::countdown_left), Some(60));
    s.tell(host_event::CORE_STATE, down(2));
    s.tell(host_event::CORE_STATE, up_again(3, "fedcba9876543210"));
    assert_eq!(s.v(Vitals::countdown_left), None, "the new core says it has none");
    // The old core's state, queued before it went, comes after the new
    // core's numbering began again: higher than any the new core has sent.
    s.tell(event::POWER_STATE, counting_from(1, now + 30_000, 9));
    assert_eq!(s.v(Vitals::countdown_left), None);
    s.tell(event::POWER_STATE, counting_from(3, now + 20_000, 1));
    assert_eq!(s.v(Vitals::countdown_left), Some(20));
}

#[gpui::test]
fn a_cancel_that_failed_in_a_core_since_replaced_says_nothing(cx: &mut TestAppContext) {
    let mut s = open(cx);
    let now = s.clock.get();
    s.tell(event::POWER_STATE, counting_from(1, now + 60_000, 3));
    s.host.defer(cmd::DISARM_POWER_OFF);
    s.do_v(|v, cx| v.disarm_power_off(cx));
    s.tell(host_event::CORE_STATE, down(2));
    s.tell(host_event::CORE_STATE, up_again(3, "fedcba9876543210"));
    assert!(s.host.resolve(cmd::DISARM_POWER_OFF, Err("the core stopped".into())));
    s.cx.run_until_parked();
    assert!(s.v(|v| v.cancel_failed().is_none()), "that rule is not the one on the chip");
}

#[cfg(debug_assertions)]
#[gpui::test]
fn a_cancel_that_fails_says_so_on_the_chip_and_retries(cx: &mut TestAppContext) {
    let mut s = open(cx);
    s.tick(usage::GAP);
    let now = s.clock.get();
    s.tell(event::POWER_STATE, counting(now + 60_000, 3));
    s.host.defer(cmd::DISARM_POWER_OFF);
    s.click("status-power-cancel");
    assert!(s.host.resolve(cmd::DISARM_POWER_OFF, Err("the core did not answer".into())));
    s.cx.run_until_parked();
    assert_eq!(s.v(|v| v.cancel_failed().map(str::to_owned)).as_deref(), Some("the core did not answer"));
    assert_eq!(s.v(|v| v.power_chip().text), "shutdown in 60s · cancel failed");
    assert!(s.drawn("status-power-retry") && !s.drawn("status-power-cancel"));
    // Retry reaches it: the rule is off, and the chip says nothing of it.
    s.click("status-power-retry");
    assert!(s.host.resolve(cmd::DISARM_POWER_OFF, Ok(json!({ "keepAwake": true, "revision": 4 }))));
    s.cx.run_until_parked();
    assert_eq!(s.host.calls_of(cmd::DISARM_POWER_OFF).len(), 2);
    assert!(s.v(|v| v.cancel_failed().is_none()));
    assert!(!s.drawn("status-power-retry") && !s.drawn("status-power-cancel"));
    assert_eq!(s.v(|v| v.power_chip().text), "Power");
}

// ------------------------------------------------- orphans: what is killed

#[gpui::test]
fn no_orphan_is_looked_for_while_the_core_is_down(cx: &mut TestAppContext) {
    let mut s = open(cx);
    s.tell(host_event::CORE_STATE, down(2));
    let n = s.host.calls_of(cmd::ORPHAN_SESSIONS).len();
    s.wait(Duration::from_secs(600));
    assert_eq!(s.host.calls_of(cmd::ORPHAN_SESSIONS).len(), n);
    // Back up: looked for at once, and on the schedule again.
    s.tell(host_event::CORE_STATE, up_again(3, CORE));
    assert_eq!(s.host.calls_of(cmd::ORPHAN_SESSIONS).len(), n + 1);
    s.wait(super::vitals::ORPHANS_EVERY);
    assert_eq!(s.host.calls_of(cmd::ORPHAN_SESSIONS).len(), n + 2);
}

#[cfg(debug_assertions)]
#[gpui::test]
fn kill_all_kills_only_the_sessions_the_person_was_asked_about(cx: &mut TestAppContext) {
    let row = |id: &str, pid: u32| json!({ "id": id, "provider": "claude", "pid": pid, "cwd": "C:\\w", "accountPath": "C:\\a" });
    let mut s = open_with(cx, |h| h.answer(cmd::ORPHAN_SESSIONS, json!([row("s-1", 11), row("s-2", 12)])));
    s.click("orphans-kill-all");
    assert!(s.v(Vitals::kill_asked));
    // A look meanwhile finds another set: the question goes, to be put again.
    s.host.answer(cmd::ORPHAN_SESSIONS, json!([row("s-1", 11), row("s-3", 13)]));
    s.wait(super::vitals::ORPHANS_EVERY);
    assert!(!s.v(Vitals::kill_asked) && s.drawn("orphans-kill-all"));
    // Asked again; by the second click s-1 is another process under the same
    // id: nothing is killed, and the question goes.
    s.click("orphans-kill-all");
    s.host.answer(cmd::ORPHAN_SESSIONS, json!([row("s-1", 21), row("s-3", 13)]));
    s.click("orphans-kill");
    assert!(s.host.calls_of(cmd::KILL_SESSION).is_empty());
    assert!(!s.v(Vitals::kill_asked));
    // Asked again, the same sessions still there: killed, but for one a chat
    // took over between the look and the kill.
    s.click("orphans-kill-all");
    s.host.defer(cmd::ORPHAN_SESSIONS);
    s.click("orphans-kill");
    s.models.workspace.update(s.cx, |w, cx| {
        let mut adopted = w.state.chats().next().unwrap().clone();
        adopted.id = "s-3".into();
        w.add_chat("C:\\w", adopted, false, cx);
    });
    assert!(s.host.resolve(cmd::ORPHAN_SESSIONS, Ok(json!([row("s-1", 21), row("s-3", 13)]))));
    s.cx.run_until_parked();
    assert_eq!(s.host.calls_of(cmd::KILL_SESSION), [json!({ "id": "s-1" })]);
}

#[cfg(debug_assertions)]
#[gpui::test]
fn a_kill_that_cannot_look_again_kills_nothing(cx: &mut TestAppContext) {
    let row = json!([{ "id": "s-1", "provider": "claude", "pid": 11, "cwd": "C:\\w", "accountPath": "C:\\a" }]);
    let mut s = open_with(cx, |h| h.answer(cmd::ORPHAN_SESSIONS, row));
    s.click("orphans-kill-all");
    s.host.defer(cmd::ORPHAN_SESSIONS);
    s.click("orphans-kill");
    assert!(s.host.resolve(cmd::ORPHAN_SESSIONS, Err("the core did not answer".into())));
    s.cx.run_until_parked();
    assert!(s.host.calls_of(cmd::KILL_SESSION).is_empty());
    assert!(!s.v(Vitals::kill_asked) && s.drawn("orphans-kill-all"));
}

#[cfg(debug_assertions)]
#[gpui::test]
fn a_recheck_whose_question_was_ended_meanwhile_kills_nothing(cx: &mut TestAppContext) {
    let row = json!([{ "id": "s-1", "provider": "claude", "pid": 11, "cwd": "C:\\w", "accountPath": "C:\\a" }]);
    let mut s = open_with(cx, |h| h.answer(cmd::ORPHAN_SESSIONS, row.clone()));
    s.click("orphans-kill-all");
    s.host.defer(cmd::ORPHAN_SESSIONS);
    s.click("orphans-kill");
    // Keep, while the look again is out: the person said yes to a question
    // that is gone, and the same sessions answering changes none of it.
    s.do_v(|v, cx| v.keep_orphans(cx));
    assert!(s.host.resolve(cmd::ORPHAN_SESSIONS, Ok(row.clone())));
    s.cx.run_until_parked();
    assert!(s.host.calls_of(cmd::KILL_SESSION).is_empty());
    assert!(s.drawn("orphans-kill-all"), "asked again");
}

#[cfg(debug_assertions)]
#[gpui::test]
fn a_look_under_way_is_let_go_of_by_the_recheck_and_none_starts_beside_it(cx: &mut TestAppContext) {
    let row = |id: &str, pid: u32| json!({ "id": id, "provider": "claude", "pid": pid, "cwd": "C:\\w", "accountPath": "C:\\a" });
    let mut s = open_with(cx, |h| h.answer(cmd::ORPHAN_SESSIONS, json!([row("s-1", 11)])));
    s.click("orphans-kill-all");
    let looks = |s: &S<'_>| s.host.calls_of(cmd::ORPHAN_SESSIONS).len();
    // A look is out when the second click comes (the recheck goes beside it).
    s.host.defer(cmd::ORPHAN_SESSIONS);
    s.wait(super::vitals::ORPHANS_EVERY);
    let before = looks(&s);
    s.click("orphans-kill");
    assert_eq!(looks(&s), before + 1, "the recheck is asked at once");
    // The look under way answers with another set: let go of, it ends no
    // question and shows nothing.
    assert!(s.host.resolve(cmd::ORPHAN_SESSIONS, Ok(json!([row("s-9", 19)]))));
    s.cx.run_until_parked();
    assert!(s.v(Vitals::kill_asked));
    assert_eq!(s.v(|v| v.orphans()[0].id.clone()), "s-1");
    // A look asked meanwhile (the schedule's) waits for the recheck, and
    // goes once it is done.
    s.do_v(|v, cx| v.scan(cx));
    assert_eq!(looks(&s), before + 1);
    assert!(s.host.resolve(cmd::ORPHAN_SESSIONS, Ok(json!([row("s-1", 11)]))));
    s.cx.run_until_parked();
    assert_eq!(s.host.calls_of(cmd::KILL_SESSION), [json!({ "id": "s-1" })]);
    assert_eq!(looks(&s), before + 2, "the look that waited");
}

// --------------------------------------------- update: save, stop, resume

/// The update panel's Download and install as its button gives it: what the
/// confirmation says now, for the release on offer (`Vitals::confirmation`),
/// handed to `install`.
fn install_as_shown(v: &mut Vitals, cx: &mut gpui::Context<Vitals>) {
    let shown = match v.phase() {
        Update::Available(o) | Update::Failed { offer: Some(o), .. } | Update::Downloading { offer: o, .. } => v.confirmation(o, cx),
        other => panic!("nothing on offer: {other:?}"),
    };
    v.install(&shown, cx);
}

fn frozen(s: &mut S<'_>) -> bool {
    s.models.workspace.read_with(s.cx, |w, _| w.frozen())
}

#[cfg(debug_assertions)]
#[gpui::test]
fn an_install_freezes_the_list_and_starts_once_the_latest_state_is_written(cx: &mut TestAppContext) {
    let mut s = open_with(cx, |h| {
        h.answer(desktop::cmd::UPDATE_CHECK, offer(CORE));
        h.answer(desktop::cmd::UPDATE_DOWNLOAD, downloaded(CORE));
        h.defer(host_cmd::FLUSH_WINDOW_STATE);
        h.defer(desktop::cmd::UPDATE_INSTALL);
    });
    s.models.workspace.update(s.cx, |w, cx| w.toggle_sidebar(cx));
    s.cx.run_until_parked();
    s.click("status-update");
    s.click("update-install");
    assert!(frozen(&mut s));
    // What waited for the gap was sent as the write began.
    let latest = s.host.sends_of(host_cmd::SAVE_WINDOW_STATE).last().expect("sent")["seq"].as_u64().unwrap();
    let notices: Vec<String> = s.models.workspace.read_with(s.cx, |w, _| w.notices().into_iter().map(|n| n.key.to_owned()).collect());
    assert!(notices.contains(&"frozen".to_owned()), "{notices:?}");
    // The person's changes are refused meanwhile.
    let pinned = s.models.workspace.read_with(s.cx, |w, _| w.state.sidebar.pinned);
    s.models.workspace.update(s.cx, |w, cx| w.toggle_sidebar(cx));
    s.cx.run_until_parked();
    assert_eq!(s.models.workspace.read_with(s.cx, |w, _| w.state.sidebar.pinned), pinned);
    // The host wrote an older state than the latest sent: it is asked again,
    // and the installer waits.
    assert!(s.host.resolve(host_cmd::FLUSH_WINDOW_STATE, Ok(json!({ "seq": latest - 1 }))));
    s.cx.run_until_parked();
    assert!(s.host.calls_of(desktop::cmd::UPDATE_INSTALL).is_empty());
    assert!(s.host.resolve(host_cmd::FLUSH_WINDOW_STATE, Ok(json!({ "seq": latest }))));
    s.cx.run_until_parked();
    assert_eq!(s.host.calls_of(desktop::cmd::UPDATE_INSTALL).len(), 1);
    assert!(frozen(&mut s), "frozen until the installer has it");
    // A failed install thaws it.
    assert!(s.host.resolve(desktop::cmd::UPDATE_INSTALL, Err("the installer was blocked".into())));
    s.cx.run_until_parked();
    assert!(!frozen(&mut s));
}

/// The Settings' Updates page, opened from the bar's button.
#[cfg(debug_assertions)]
fn settings_updates(s: &mut S<'_>) {
    s.click("status-settings");
    let open = s.view.read_with(s.cx, |v, cx| v.panels.read(cx).open.clone());
    let Some(crate::panels::Open::Settings(panel)) = open else { panic!("no settings panel") };
    panel.update(s.cx, |v, cx| v.show_page(crate::settings::Page::Updates, cx));
    s.cx.run_until_parked();
}

/// The Settings' Update... is the chip's install: one confirmation, the
/// bar's, and then the same steps, the list frozen, the sessions stopped for
/// another core, the latest state confirmed written before the installer.
#[cfg(debug_assertions)]
#[gpui::test]
fn the_settings_install_freezes_stops_and_saves_as_the_chips_does(cx: &mut TestAppContext) {
    let mut s = open_with(cx, |h| {
        h.answer(desktop::cmd::UPDATE_CHECK, offer("another-core"));
        h.answer(desktop::cmd::UPDATE_DOWNLOAD, downloaded("another-core"));
        h.defer(cmd::STOP_ALL_SESSIONS);
        h.defer(host_cmd::FLUSH_WINDOW_STATE);
        h.defer(desktop::cmd::UPDATE_INSTALL);
    });
    // The window's place watched, as the app's window is (lib.rs), and
    // moved a moment ago: not kept yet when the install begins.
    let host: Arc<dyn crate::host::Host> = s.host.clone();
    let _keeper = s.cx.update(|window, cx| desktop::Keeper::watch(host, window, cx));
    s.cx.simulate_resize(gpui::size(gpui::px(1100.), gpui::px(700.)));
    s.cx.run_until_parked();
    assert!(s.host.desktop.kept.lock().unwrap().is_empty());
    settings_updates(&mut s);
    assert!(s.drawn("settings-update-install"));
    s.click("settings-update-install");
    // The panel gave way to the bar's confirmation, and nothing has started.
    assert_eq!(s.view.read_with(s.cx, |v, cx| v.panels.read(cx).shown()), None);
    assert_eq!(s.popover(), Some(Popover::Update));
    assert!(s.drawn("update-panel") && s.drawn("update-notes"));
    assert!(s.host.calls_of(desktop::cmd::UPDATE_DOWNLOAD).is_empty());
    s.click("update-install");
    // The download and the install name the offer confirmed, the one the
    // chip and the page share.
    assert_eq!(s.host.calls_of(desktop::cmd::UPDATE_DOWNLOAD), [json!({ "generation": 7 })]);
    assert!(frozen(&mut s), "the list takes no change from the download on");
    assert!(matches!(s.phase(), Update::Stopping(_)));
    assert!(s.host.calls_of(host_cmd::FLUSH_WINDOW_STATE).is_empty(), "the sessions stop before the save");
    assert!(s.host.resolve(cmd::STOP_ALL_SESSIONS, Ok(stopped())));
    s.cx.run_until_parked();
    let latest = s.host.sends_of(host_cmd::SAVE_WINDOW_STATE).last().expect("sent")["seq"].as_u64().unwrap();
    assert!(s.host.calls_of(desktop::cmd::UPDATE_INSTALL).is_empty(), "the installer waits for the write");
    // The place still waiting was handed to the host before its drain.
    assert_eq!(s.host.desktop.kept.lock().unwrap().last().map(|p| p.bounds.width), Some(1100.));
    assert!(s.host.resolve(host_cmd::FLUSH_WINDOW_STATE, Ok(json!({ "seq": latest }))));
    s.cx.run_until_parked();
    assert_eq!(
        s.host.calls_of(desktop::cmd::UPDATE_INSTALL),
        [json!({ "generation": 7, "reason": "installing the update to 0.20.1", "anyway": false, "core": demo_core(), "keep": false })]
    );
    assert!(frozen(&mut s));
}

fn chat_of(s: &mut S<'_>, id: &str) -> crate::state::Chat {
    s.models.workspace.read_with(s.cx, |w, _| w.state.find(id).cloned()).expect("listed")
}

/// Nothing is written after an update's last write of the list until the
/// update is called off: a look that names a session later shows at once
/// but is kept in memory only, a notice's Retry reads nothing over the list,
/// a move of the window is held back from the host's drain. Called off, the
/// list and the place are written.
#[cfg(debug_assertions)]
#[gpui::test]
fn nothing_is_written_after_the_updates_last_write_until_it_is_called_off(cx: &mut TestAppContext) {
    let mut s = open_with(cx, |h| {
        h.answer(desktop::cmd::UPDATE_CHECK, offer(CORE));
        h.answer(desktop::cmd::UPDATE_DOWNLOAD, downloaded(CORE));
        h.defer(desktop::cmd::UPDATE_INSTALL);
    });
    let host: Arc<dyn crate::host::Host> = s.host.clone();
    let _keeper = s.cx.update(|window, cx| desktop::Keeper::watch(host, window, cx));
    s.click("status-update");
    s.click("update-install");
    s.cx.run_until_parked();
    assert_eq!(s.host.calls_of(desktop::cmd::UPDATE_INSTALL).len(), 1);
    assert!(s.models.workspace.read_with(s.cx, |w, _| w.sealed()));
    let sent = s.host.sends_of(host_cmd::SAVE_WINDOW_STATE).len();
    let kept = s.host.desktop.kept.lock().unwrap().len();
    let reads = s.host.calls_of(host_cmd::WINDOW_STATE).len();

    s.host.answer(host_cmd::SESSIONS_META, json!([{ "id": "c-parser", "meta": { "status": "busy", "sessionId": "s-new" } }]));
    s.cx.executor().advance_clock(crate::workspace::EVERY);
    s.cx.run_until_parked();
    assert_eq!(chat_of(&mut s, "c-parser").session_id.as_deref(), Some("s-new"), "shown");
    assert_eq!(s.host.sends_of(host_cmd::SAVE_WINDOW_STATE).len(), sent, "not written");
    s.models.workspace.update(s.cx, |w, cx| w.retry("read", cx));
    s.models.workspace.update(s.cx, |w, cx| w.retry("import", cx));
    s.cx.simulate_resize(gpui::size(gpui::px(1000.), gpui::px(650.)));
    s.cx.executor().advance_clock(Duration::from_secs(1));
    s.cx.run_until_parked();
    assert_eq!(s.host.calls_of(host_cmd::WINDOW_STATE).len(), reads);
    assert_eq!(s.host.desktop.kept.lock().unwrap().len(), kept);

    assert!(s.host.resolve(desktop::cmd::UPDATE_INSTALL, Err("the installer was blocked".into())));
    s.cx.run_until_parked();
    assert!(!frozen(&mut s));
    let last = s.host.sends_of(host_cmd::SAVE_WINDOW_STATE).last().cloned().expect("sent");
    assert!(last["text"].as_str().unwrap().contains("s-new"), "written once called off");
    assert_eq!(s.host.desktop.kept.lock().unwrap().last().map(|p| p.bounds.width), Some(1000.));
}

/// A rename let through just before the list froze still has its chats
/// follow, and the update's last write waits for it; one asked once frozen
/// is refused before the host is asked.
#[cfg(debug_assertions)]
#[gpui::test]
fn the_updates_last_write_holds_a_rename_let_through_before_the_freeze(cx: &mut TestAppContext) {
    let mut s = open_with(cx, |h| {
        h.answer(desktop::cmd::UPDATE_CHECK, offer(CORE));
        h.answer(desktop::cmd::UPDATE_DOWNLOAD, downloaded(CORE));
        h.defer(crate::panels::cmd::RENAME_ACCOUNT);
        h.defer(desktop::cmd::UPDATE_INSTALL);
    });
    let chat = chat_of(&mut s, "c-parser");
    let _renamed = s.models.workspace.update(s.cx, |w, cx| w.rename_account_on_host(chat.provider, &chat.account, "renamed", cx));
    s.click("status-update");
    s.click("update-install");
    s.cx.run_until_parked();
    assert!(frozen(&mut s));
    assert!(s.host.calls_of(desktop::cmd::UPDATE_INSTALL).is_empty(), "the last write waits for the rename");
    let _refused = s.models.workspace.update(s.cx, |w, cx| w.rename_account_on_host(chat.provider, &chat.account, "again", cx));
    assert_eq!(s.host.calls_of(crate::panels::cmd::RENAME_ACCOUNT).len(), 1);

    assert!(s.host.resolve(crate::panels::cmd::RENAME_ACCOUNT, Ok(Value::Null)));
    s.cx.run_until_parked();
    assert_eq!(chat_of(&mut s, "c-parser").account, "renamed");
    assert_eq!(s.host.calls_of(desktop::cmd::UPDATE_INSTALL).len(), 1);
    let last = s.host.sends_of(host_cmd::SAVE_WINDOW_STATE).last().cloned().expect("sent");
    assert!(last["text"].as_str().unwrap().contains("renamed"), "the last write has it");
}

/// An install is not started while a check is under way, and an offer the
/// host refuses as no longer on offer is let go: the next click checks
/// again instead of asking for the old one once more.
#[cfg(debug_assertions)]
#[gpui::test]
fn an_offer_the_host_no_longer_has_is_let_go_for_a_new_check(cx: &mut TestAppContext) {
    let mut s = open_with(cx, |h| h.answer(desktop::cmd::UPDATE_CHECK, offer(CORE)));
    assert!(matches!(s.phase(), Update::Available(_)));
    s.host.defer(desktop::cmd::UPDATE_CHECK);
    s.do_v(|v, cx| v.check(true, cx));
    s.do_v(install_as_shown);
    s.cx.run_until_parked();
    assert!(s.host.calls_of(desktop::cmd::UPDATE_DOWNLOAD).is_empty(), "not while a check is under way");
    assert!(s.host.resolve(desktop::cmd::UPDATE_CHECK, Ok(offer(CORE))));
    s.cx.run_until_parked();

    s.host.fail(desktop::cmd::UPDATE_DOWNLOAD, Some(desktop::OFFER_CHANGED));
    s.do_v(install_as_shown);
    s.cx.run_until_parked();
    assert!(matches!(s.phase(), Update::Failed { offer: None, .. }), "{:?}", s.phase());
    assert!(!frozen(&mut s));
    let checks = s.host.calls_of(desktop::cmd::UPDATE_CHECK).len();
    s.click("status-update");
    assert_eq!(s.host.calls_of(desktop::cmd::UPDATE_CHECK).len(), checks + 1);
}

/// A stop of the sessions the core does not answer leaves nothing frozen:
/// the update panel calls it off, and past `STOP_LIMIT` it is called off by
/// itself, the chip saying why.
#[cfg(debug_assertions)]
#[gpui::test]
fn a_stop_the_core_does_not_answer_is_called_off(cx: &mut TestAppContext) {
    let mut s = open_with(cx, |h| {
        h.answer(desktop::cmd::UPDATE_CHECK, offer("another-core"));
        h.answer(desktop::cmd::UPDATE_DOWNLOAD, downloaded("another-core"));
        h.defer(cmd::STOP_ALL_SESSIONS);
    });
    s.click("status-update");
    s.click("update-install");
    assert!(matches!(s.phase(), Update::Stopping(_)));
    assert!(frozen(&mut s));
    s.click("status-update");
    assert_eq!(s.popover(), Some(Popover::Update));
    s.click("update-call-off");
    assert!(matches!(s.phase(), Update::Available(_)));
    assert!(!frozen(&mut s));
    assert_eq!(s.host.calls_of(cmd::UPDATE_CALLED_OFF).len(), 1);

    s.click("status-update");
    s.click("update-install");
    assert!(matches!(s.phase(), Update::Stopping(_)));
    s.cx.executor().advance_clock(super::vitals::STOP_LIMIT);
    s.cx.run_until_parked();
    assert!(matches!(s.phase(), Update::Failed { offer: Some(_), .. }), "{:?}", s.phase());
    assert!(!frozen(&mut s));
    assert_eq!(s.host.calls_of(cmd::UPDATE_CALLED_OFF).len(), 2);
    assert!(s.host.calls_of(host_cmd::FLUSH_WINDOW_STATE).is_empty(), "nothing was installed");
}

/// From the sessions' stop to the installer's start no restart of the core
/// is asked, from the Settings' Restart (shown disabled) or any other way
/// (`Models::restart_core`, the banner's too); called off, it is offered
/// again.
#[cfg(debug_assertions)]
#[gpui::test]
fn no_core_restart_is_asked_while_an_update_holds_the_core(cx: &mut TestAppContext) {
    let mut s = open_with(cx, |h| {
        h.answer(desktop::cmd::UPDATE_CHECK, offer("another-core"));
        h.answer(desktop::cmd::UPDATE_DOWNLOAD, downloaded("another-core"));
        h.defer(cmd::STOP_ALL_SESSIONS);
    });
    assert!(!s.v(Vitals::core_held));
    s.click("status-update");
    s.click("update-install");
    assert!(matches!(s.phase(), Update::Stopping(_)));
    assert!(s.v(Vitals::core_held));
    settings_updates(&mut s);
    s.click("settings-core-restart");
    let models = s.models.clone();
    s.cx.update(|_, cx| models.restart_core(cx));
    s.cx.run_until_parked();
    assert!(s.host.calls_of(host_cmd::CORE_RESTART).is_empty());

    s.do_v(|v, cx| v.call_off_stop(cx));
    assert!(!s.v(Vitals::core_held));
    s.click("settings-core-restart");
    assert_eq!(s.host.calls_of(host_cmd::CORE_RESTART).len(), 1, "offered again once called off");
}

/// The installer starts only under the core the sessions were stopped in:
/// one replaced meanwhile (another process, here of the same build) has the
/// host refuse the install, which names the core the stop answered from,
/// and that calls the update off with the reason, the list thawed, and the
/// next click checks again.
#[cfg(debug_assertions)]
#[gpui::test]
fn an_install_is_refused_when_the_core_changed_since_the_stop(cx: &mut TestAppContext) {
    let mut s = open_with(cx, |h| {
        h.answer(desktop::cmd::UPDATE_CHECK, offer("another-core"));
        h.answer(desktop::cmd::UPDATE_DOWNLOAD, downloaded("another-core"));
        h.defer(cmd::STOP_ALL_SESSIONS);
        h.fail(desktop::cmd::UPDATE_INSTALL, Some(desktop::CORE_CHANGED));
    });
    s.click("status-update");
    s.click("update-install");
    assert!(matches!(s.phase(), Update::Stopping(_)));
    s.tell(host_event::CORE_STATE, down(2));
    s.tell(host_event::CORE_STATE, up_as(3, 4343));
    assert!(s.host.resolve(cmd::STOP_ALL_SESSIONS, Ok(stopped())));
    s.cx.run_until_parked();
    let installs = s.host.calls_of(desktop::cmd::UPDATE_INSTALL);
    assert_eq!(installs.len(), 1);
    assert_eq!(installs[0]["core"], demo_core(), "the stop's, not the one the window sees up");
    let Update::Failed { offer, why } = s.phase() else { panic!("{:?}", s.phase()) };
    assert_eq!((offer, why.as_str()), (None, desktop::CORE_CHANGED));
    assert!(!frozen(&mut s));
    assert!(!s.v(Vitals::core_held));
    assert_eq!(s.host.calls_of(cmd::UPDATE_CALLED_OFF).len(), 1);
}

/// Once the stop has answered with the core it stopped the sessions in, the
/// install names that core whatever the window's view of the link says as
/// the save is done (`starting` again here, the core being found again):
/// the host decides, as the installer is about to start.
#[cfg(debug_assertions)]
#[gpui::test]
fn the_install_goes_by_the_stops_answer_whatever_the_link_says_after(cx: &mut TestAppContext) {
    let mut s = open_with(cx, |h| {
        h.answer(desktop::cmd::UPDATE_CHECK, offer("another-core"));
        h.answer(desktop::cmd::UPDATE_DOWNLOAD, downloaded("another-core"));
        h.defer(cmd::STOP_ALL_SESSIONS);
        h.defer(host_cmd::FLUSH_WINDOW_STATE);
        h.defer(desktop::cmd::UPDATE_INSTALL);
    });
    s.click("status-update");
    s.click("update-install");
    assert!(s.host.resolve(cmd::STOP_ALL_SESSIONS, Ok(stopped())));
    s.cx.run_until_parked();
    let latest = s.host.sends_of(host_cmd::SAVE_WINDOW_STATE).last().expect("sent")["seq"].as_u64().unwrap();
    s.tell(host_event::CORE_STATE, json!({ "state": "starting", "core": null, "error": null, "revision": 2 }));
    assert!(s.host.resolve(host_cmd::FLUSH_WINDOW_STATE, Ok(json!({ "seq": latest }))));
    s.cx.run_until_parked();
    let installs = s.host.calls_of(desktop::cmd::UPDATE_INSTALL);
    assert_eq!(installs.len(), 1, "{:?}", s.phase());
    assert_eq!(installs[0]["core"], demo_core());
}

/// The same core process found again after it let the window go (a new
/// epoch for the panes, corelink.rs `rejoin`) is still the one the sessions
/// were stopped in: the install goes ahead, naming it.
#[cfg(debug_assertions)]
#[gpui::test]
fn the_same_core_found_again_still_takes_the_install(cx: &mut TestAppContext) {
    let mut s = open_with(cx, |h| {
        h.answer(desktop::cmd::UPDATE_CHECK, offer("another-core"));
        h.answer(desktop::cmd::UPDATE_DOWNLOAD, downloaded("another-core"));
        h.defer(cmd::STOP_ALL_SESSIONS);
        h.defer(desktop::cmd::UPDATE_INSTALL);
    });
    s.click("status-update");
    s.click("update-install");
    let epoch = s.models.core.read_with(s.cx, |c, _| c.epoch());
    s.tell(host_event::CORE_STATE, json!({ "state": "starting", "core": null, "error": null, "revision": 2 }));
    s.tell(host_event::CORE_STATE, up_as(3, demo::core().pid));
    assert!(s.models.core.read_with(s.cx, |c, _| c.epoch()) > epoch, "the panes start over");
    assert!(s.host.resolve(cmd::STOP_ALL_SESSIONS, Ok(stopped())));
    s.cx.run_until_parked();
    let installs = s.host.calls_of(desktop::cmd::UPDATE_INSTALL);
    assert_eq!(installs.len(), 1, "{:?}", s.phase());
    assert_eq!(installs[0]["core"], demo_core());
}

/// A panel drawn while the core is starting, or once it went, cannot say
/// which sessions run: what the window last heard of them is the old
/// core's. It promises nothing ("nothing stops" least of all) and offers
/// no install, and a click on what it drew installs nothing. Once the core
/// is up and its board heard, the panel says what runs, and its click goes.
#[cfg(debug_assertions)]
#[gpui::test]
fn a_panel_that_cannot_tell_what_runs_offers_no_install(cx: &mut TestAppContext) {
    let mut s = open_with(cx, |h| {
        h.answer(desktop::cmd::UPDATE_CHECK, offer("another-core"));
        h.answer(desktop::cmd::UPDATE_DOWNLOAD, downloaded("another-core"));
        h.defer(cmd::STOP_ALL_SESSIONS);
    });
    let shown = |s: &mut S<'_>| {
        s.do_v(|v, cx| {
            let Update::Available(offer) = v.phase().clone() else { panic!("{:?}", v.phase()) };
            v.confirmation(&offer, cx)
        })
    };
    // Known, no session running: the line says nothing stops.
    assert_eq!(shown(&mut s).live(), Some(0));
    // The core goes: what was heard of it no longer counts.
    s.tell(host_event::CORE_STATE, down(2));
    s.click("status-update");
    let drawn = shown(&mut s);
    assert_eq!((drawn.core.as_ref(), drawn.live()), (None, None));
    assert!(super::updates::sessions_line(drawn.live(), drawn.keep).contains("not known"));
    assert!(s.drawn("update-panel") && !s.drawn("update-install"), "no install offered");
    s.do_v(|v, cx| v.install(&drawn, cx));
    assert!(s.host.calls_of(desktop::cmd::UPDATE_DOWNLOAD).is_empty(), "nothing on a panel that promised nothing");
    // Starting, then up with its board not heard yet: still not known.
    s.tell(host_event::CORE_STATE, json!({ "state": "starting", "core": null, "error": null, "revision": 3 }));
    assert_eq!(shown(&mut s).live(), None);
    s.host.defer(cmd::POWER_STATE);
    s.tell(host_event::CORE_STATE, up_as(4, demo::core().pid));
    let drawn = shown(&mut s);
    assert!(drawn.core.is_some() && drawn.live().is_none(), "{drawn:?}");
    assert!(!s.drawn("update-install"));
    s.do_v(|v, cx| v.install(&drawn, cx));
    assert!(s.host.calls_of(desktop::cmd::UPDATE_DOWNLOAD).is_empty());
    // Heard: a session runs, and the click stops that one, in that core.
    assert!(s.host.resolve(cmd::POWER_STATE, Ok(holding())));
    s.cx.run_until_parked();
    assert_eq!(shown(&mut s).live(), Some(1));
    s.click("update-install");
    assert_eq!(s.host.calls_of(desktop::cmd::UPDATE_DOWNLOAD).len(), 1);
    assert_eq!(s.host.calls_of(cmd::STOP_ALL_SESSIONS), [json!({ "core": demo_core(), "confirmed": ["c-codex"] })]);
}

/// A power state with `ids` running, nothing armed.
fn running(ids: &[&str]) -> Value {
    let sessions: Vec<Value> = ids
        .iter()
        .map(|id| json!({ "id": id, "turn": "busy", "procs": [], "outputFresh": false, "busy": true, "stalled": null }))
        .collect();
    json!({
        "keepAwake": false, "holding": false, "armed": null, "countdownEndsAtMs": null,
        "summary": { "busy": ids.len(), "waiting": 0, "stalled": 0, "idleSinceMs": null, "sessions": sessions }
    })
}

/// The panel counted one session to stop; during the download another
/// starts in the same core (a chat opened, a helper an agent spawned). Seen
/// by the window, nothing is stopped or installed: the update fails with the
/// offer kept, and the panel, drawn again, counts both. Not seen yet, the
/// stop goes, naming only the ones counted, and the core's refusal calls the
/// update off.
#[cfg(debug_assertions)]
#[gpui::test]
fn a_session_started_during_the_download_is_not_stopped_without_asking(cx: &mut TestAppContext) {
    let mut s = open_with(cx, |h| {
        h.answer(desktop::cmd::UPDATE_CHECK, offer("another-core"));
        h.defer(desktop::cmd::UPDATE_DOWNLOAD);
        h.defer(cmd::STOP_ALL_SESSIONS);
        h.defer(desktop::cmd::UPDATE_INSTALL);
    });
    s.tell(event::POWER_STATE, running(&["c-codex"]));
    s.click("status-update");
    s.click("update-install");
    s.tell(event::POWER_STATE, running(&["c-codex", "h-new"]));
    assert!(s.host.resolve(desktop::cmd::UPDATE_DOWNLOAD, Ok(downloaded("another-core"))));
    s.cx.run_until_parked();
    let Update::Failed { offer: Some(_), why } = s.phase() else { panic!("{:?}", s.phase()) };
    assert!(why.contains("did not count among those it stops"), "{why}");
    assert!(s.host.calls_of(cmd::STOP_ALL_SESSIONS).is_empty());
    assert!(s.host.calls_of(cmd::UPDATE_CALLED_OFF).is_empty(), "nothing was stopped");
    assert!(!frozen(&mut s));

    // Drawn again it counts both; confirmed, the stop names both.
    s.click("status-update");
    s.click("update-install");
    assert!(s.host.resolve(desktop::cmd::UPDATE_DOWNLOAD, Ok(downloaded("another-core"))));
    s.cx.run_until_parked();
    assert_eq!(s.host.calls_of(cmd::STOP_ALL_SESSIONS), [json!({ "core": demo_core(), "confirmed": ["c-codex", "h-new"] })]);
    // One the window has not heard of: the core refuses, and nothing goes.
    assert!(s.host.resolve(cmd::STOP_ALL_SESSIONS, Err("a session runs that the update's confirmation did not count".into())));
    s.cx.run_until_parked();
    assert!(matches!(s.phase(), Update::Failed { offer: Some(_), .. }), "{:?}", s.phase());
    assert!(s.host.calls_of(desktop::cmd::UPDATE_INSTALL).is_empty());
    assert!(!frozen(&mut s));
}

/// The panel was drawn in one core; by the time the download is through
/// another is up. Its sessions were never shown to anyone: nothing is
/// stopped or installed there, and the update asks again.
#[cfg(debug_assertions)]
#[gpui::test]
fn a_core_replaced_during_the_download_has_nothing_stopped(cx: &mut TestAppContext) {
    let mut s = open_with(cx, |h| {
        h.answer(desktop::cmd::UPDATE_CHECK, offer("another-core"));
        h.defer(desktop::cmd::UPDATE_DOWNLOAD);
        h.defer(cmd::STOP_ALL_SESSIONS);
    });
    s.tell(event::POWER_STATE, holding());
    s.click("status-update");
    s.click("update-install");
    s.tell(host_event::CORE_STATE, down(2));
    s.tell(host_event::CORE_STATE, up_as(3, 4343));
    s.tell(event::POWER_STATE, holding());
    assert!(s.host.resolve(desktop::cmd::UPDATE_DOWNLOAD, Ok(downloaded("another-core"))));
    s.cx.run_until_parked();
    let Update::Failed { offer: Some(_), why } = s.phase() else { panic!("{:?}", s.phase()) };
    assert!(why.contains("the core changed since this install was confirmed"), "{why}");
    assert!(s.host.calls_of(cmd::STOP_ALL_SESSIONS).is_empty());
    assert!(!frozen(&mut s));
}

/// A stop whose answer names no core holds the install to none: refused.
#[cfg(debug_assertions)]
#[gpui::test]
fn a_stop_that_names_no_core_takes_no_install(cx: &mut TestAppContext) {
    let mut s = open_with(cx, |h| {
        h.answer(desktop::cmd::UPDATE_CHECK, offer("another-core"));
        h.answer(desktop::cmd::UPDATE_DOWNLOAD, downloaded("another-core"));
        h.answer(cmd::STOP_ALL_SESSIONS, json!({ "stopped": 0 }));
    });
    s.click("status-update");
    s.click("update-install");
    s.cx.run_until_parked();
    assert!(s.host.calls_of(desktop::cmd::UPDATE_INSTALL).is_empty());
    assert!(matches!(s.phase(), Update::Failed { offer: None, ref why } if why == desktop::CORE_CHANGED), "{:?}", s.phase());
}

/// The page shows the update as the bar has it, and its Check is the bar's.
#[cfg(debug_assertions)]
#[gpui::test]
fn the_settings_page_follows_the_bars_update(cx: &mut TestAppContext) {
    let mut s = open_with(cx, |h| {
        h.answer(desktop::cmd::UPDATE_CHECK, offer(CORE));
        h.defer(desktop::cmd::UPDATE_DOWNLOAD);
    });
    s.click("status-update");
    s.click("update-install");
    settings_updates(&mut s);
    assert!(!s.drawn("settings-update-install"), "nothing to confirm while it downloads");
    // A check from the page while it downloads starts nothing.
    let checks = s.host.calls_of(desktop::cmd::UPDATE_CHECK).len();
    let open = s.view.read_with(s.cx, |v, cx| v.panels.read(cx).open.clone());
    let Some(crate::panels::Open::Settings(panel)) = open else { panic!("no settings panel") };
    panel.update(s.cx, |v, cx| v.check_update(cx));
    s.cx.run_until_parked();
    assert_eq!(s.host.calls_of(desktop::cmd::UPDATE_CHECK).len(), checks);
    // A failed download offers the bar's confirmation again from the page.
    assert!(s.host.resolve(desktop::cmd::UPDATE_DOWNLOAD, Err("no space left".into())));
    s.cx.run_until_parked();
    assert!(s.drawn("settings-update-install"));
    // None on offer: the page's Check is the bar's, and the chip goes.
    s.host.answer(desktop::cmd::UPDATE_CHECK, json!({ "current": "0.20.0", "offer": null }));
    panel.update(s.cx, |v, cx| v.check_update(cx));
    s.cx.run_until_parked();
    assert_eq!(s.phase(), Update::Idle);
    assert_eq!(s.v(Vitals::current_at), Some(T0));
    assert!(!s.drawn("status-update") && !s.drawn("settings-update-install"));
}

#[cfg(debug_assertions)]
#[gpui::test]
fn a_list_that_cannot_be_written_holds_the_install_for_the_persons_choice(cx: &mut TestAppContext) {
    let mut s = open_with(cx, |h| {
        h.answer(desktop::cmd::UPDATE_CHECK, offer(CORE));
        h.answer(desktop::cmd::UPDATE_DOWNLOAD, downloaded(CORE));
        h.defer(host_cmd::FLUSH_WINDOW_STATE);
    });
    s.click("status-update");
    s.click("update-install");
    assert!(s.host.resolve(host_cmd::FLUSH_WINDOW_STATE, Err("chats.json is locked".into())));
    s.cx.run_until_parked();
    assert!(matches!(s.phase(), Update::Unsaved { ref why, .. } if why == "chats.json is locked"));
    assert!(s.host.calls_of(desktop::cmd::UPDATE_INSTALL).is_empty());
    assert!(frozen(&mut s), "the list stays still while the person chooses");
    // Install anyway goes on without the write.
    s.click("update-anyway");
    assert!(s.host.resolve(host_cmd::FLUSH_WINDOW_STATE, Err("chats.json is locked".into())));
    s.cx.run_until_parked();
    assert_eq!(s.host.calls_of(desktop::cmd::UPDATE_INSTALL)[0]["anyway"], true);
}

#[cfg(debug_assertions)]
#[gpui::test]
fn a_retry_looks_again_at_whether_the_sessions_must_stop(cx: &mut TestAppContext) {
    let mut s = open_with(cx, |h| {
        h.answer(desktop::cmd::UPDATE_CHECK, offer(CORE));
        h.answer(desktop::cmd::UPDATE_DOWNLOAD, downloaded(CORE));
        h.answer(cmd::STOP_ALL_SESSIONS, stopped());
        h.defer(desktop::cmd::UPDATE_INSTALL);
    });
    s.click("status-update");
    s.click("update-install");
    assert!(s.host.calls_of(cmd::STOP_ALL_SESSIONS).is_empty(), "the running core is the release's");
    assert!(s.host.resolve(desktop::cmd::UPDATE_INSTALL, Err("unsaved: locked".into())));
    s.cx.run_until_parked();
    // Meanwhile another core came up: the release's core is no longer the
    // running one. The sessions were kept, so Retry stops none on that
    // promise: it asks again, and the confirmation now says they stop.
    s.tell(host_event::CORE_STATE, down(2));
    s.tell(host_event::CORE_STATE, up_again(3, "fedcba9876543210"));
    s.tell(event::POWER_STATE, holding());
    s.click("update-retry");
    let Update::Failed { offer: Some(_), why } = s.phase() else { panic!("{:?}", s.phase()) };
    assert!(why.contains("another core than the running one"), "{why}");
    assert!(s.host.calls_of(cmd::STOP_ALL_SESSIONS).is_empty());
    assert!(!frozen(&mut s));
    // The panel stays open on it; confirmed again, the sessions stop first.
    assert_eq!(s.popover(), Some(Popover::Update));
    s.click("update-install");
    assert_eq!(s.host.calls_of(cmd::STOP_ALL_SESSIONS).len(), 1);
    let installs = s.host.calls_of(desktop::cmd::UPDATE_INSTALL);
    assert_eq!(
        installs[1],
        json!({ "generation": 7, "reason": "installing the update to 0.20.1", "anyway": false, "core": demo_core(), "keep": false })
    );
    assert!(frozen(&mut s));
}

/// An install that could not write first (`Update::Unsaved`), its sessions
/// stopped (`stop`) or kept running, and then another core up, another
/// process, in place of the one it was for.
#[cfg(debug_assertions)]
fn unsaved_then_another_core(cx: &mut TestAppContext, stop: bool) -> S<'_> {
    let core = if stop { "another-core" } else { CORE };
    let mut s = open_with(cx, |h| {
        h.answer(desktop::cmd::UPDATE_CHECK, offer(core));
        h.answer(desktop::cmd::UPDATE_DOWNLOAD, downloaded(core));
        h.answer(cmd::STOP_ALL_SESSIONS, stopped());
        h.defer(desktop::cmd::UPDATE_INSTALL);
    });
    s.tell(event::POWER_STATE, holding());
    s.click("status-update");
    s.click("update-install");
    assert_eq!(s.host.calls_of(cmd::STOP_ALL_SESSIONS).len(), usize::from(stop));
    assert!(s.host.resolve(desktop::cmd::UPDATE_INSTALL, Err("unsaved: locked".into())));
    s.cx.run_until_parked();
    assert!(matches!(s.phase(), Update::Unsaved { keep, .. } if keep != stop), "{:?}", s.phase());
    s.tell(host_event::CORE_STATE, down(2));
    s.tell(host_event::CORE_STATE, up_as(3, 4343));
    s.tell(event::POWER_STATE, holding());
    s
}

/// Retry, then Install anyway, once another core is up: what the choice
/// says (the sessions are stopped, or keep running) was done in the old
/// one, so neither stops the new core's sessions or installs on it; each
/// calls the update off and asks again.
#[cfg(debug_assertions)]
fn another_core_asks_again(mut s: S<'_>, button: &'static str, stopped_before: usize) {
    s.click(button);
    let Update::Failed { offer: Some(_), why } = s.phase() else { panic!("{:?}", s.phase()) };
    assert!(why.contains("the core changed since this install was confirmed"), "{why}");
    assert_eq!(s.host.calls_of(cmd::STOP_ALL_SESSIONS).len(), stopped_before, "no stop in the new core");
    assert_eq!(s.host.calls_of(desktop::cmd::UPDATE_INSTALL).len(), 1, "no install on it");
    // Called off: the core told where sessions were stopped for it, which
    // start again.
    assert_eq!(s.host.calls_of(cmd::UPDATE_CALLED_OFF).len(), stopped_before);
    assert!(!frozen(&mut s));
    // The confirmation is shown again, for the core up now.
    assert_eq!(s.popover(), Some(Popover::Update));
    assert!(s.drawn("update-install"));
}

#[cfg(debug_assertions)]
#[gpui::test]
fn a_retry_after_the_stop_stops_nothing_in_another_core(cx: &mut TestAppContext) {
    another_core_asks_again(unsaved_then_another_core(cx, true), "update-retry", 1);
}

#[cfg(debug_assertions)]
#[gpui::test]
fn install_anyway_after_the_stop_stops_nothing_in_another_core(cx: &mut TestAppContext) {
    another_core_asks_again(unsaved_then_another_core(cx, true), "update-anyway", 1);
}

/// The sessions were kept running, and another core is up: Install anyway
/// keeps no word given for the old one in it, as Retry does not
/// (`a_retry_looks_again_at_whether_the_sessions_must_stop`).
#[cfg(debug_assertions)]
#[gpui::test]
fn install_anyway_keeps_no_promise_made_in_another_core(cx: &mut TestAppContext) {
    another_core_asks_again(unsaved_then_another_core(cx, false), "update-anyway", 0);
}

/// The panel said the session keeps running, in the core up then; another
/// is up by the time the click lands on what was drawn. Nothing is
/// downloaded or stopped on that word: the panel, drawn again, says what an
/// install would do now, and the click on that goes ahead on it.
#[cfg(debug_assertions)]
#[gpui::test]
fn the_install_goes_only_on_the_confirmation_as_it_was_drawn(cx: &mut TestAppContext) {
    let mut s = open_with(cx, |h| {
        h.answer(desktop::cmd::UPDATE_CHECK, offer(CORE));
        h.defer(desktop::cmd::UPDATE_DOWNLOAD);
        h.defer(cmd::STOP_ALL_SESSIONS);
    });
    s.tell(event::POWER_STATE, holding());
    s.click("status-update");
    let drawn = s.do_v(|v, cx| {
        let Update::Available(offer) = v.phase().clone() else { panic!("{:?}", v.phase()) };
        v.confirmation(&offer, cx)
    });
    assert!(drawn.keep && drawn.live() == Some(1));
    s.tell(host_event::CORE_STATE, down(2));
    s.tell(host_event::CORE_STATE, up_as(3, 4343));
    s.tell(event::POWER_STATE, holding());
    s.do_v(|v, cx| v.install(&drawn, cx));
    assert!(s.host.calls_of(desktop::cmd::UPDATE_DOWNLOAD).is_empty(), "nothing on a confirmation drawn for another core");
    assert!(matches!(s.phase(), Update::Available(_)), "{:?}", s.phase());
    assert_eq!(s.popover(), Some(Popover::Update), "the panel stays, drawn again");
    // Drawn for the core up now, its click goes ahead on what it says.
    s.click("update-install");
    assert_eq!(s.host.calls_of(desktop::cmd::UPDATE_DOWNLOAD).len(), 1, "what the panel says now is acted on");
}

/// The confirmation said nothing stops, and the setup's signature names
/// another core than the running one; but no session runs, so the stop
/// ends nothing and breaks no word: the install goes on.
#[cfg(debug_assertions)]
#[gpui::test]
fn with_no_session_running_a_stop_breaks_no_promise(cx: &mut TestAppContext) {
    let mut s = open_with(cx, |h| {
        h.answer(desktop::cmd::UPDATE_CHECK, offer(CORE));
        h.answer(desktop::cmd::UPDATE_DOWNLOAD, downloaded("fedcba9876543210"));
        h.defer(cmd::STOP_ALL_SESSIONS);
    });
    assert!(s.v(|v| v.power().summary.sessions.is_empty()));
    s.click("status-update");
    s.click("update-install");
    assert!(matches!(s.phase(), Update::Stopping(_)), "{:?}", s.phase());
    assert_eq!(s.host.calls_of(cmd::STOP_ALL_SESSIONS).len(), 1);
}

#[cfg(debug_assertions)]
#[gpui::test]
fn cancel_starts_again_the_sessions_the_stop_ended(cx: &mut TestAppContext) {
    let mut s = open_with(cx, |h| {
        h.answer(desktop::cmd::UPDATE_CHECK, offer("another-core"));
        h.answer(desktop::cmd::UPDATE_DOWNLOAD, downloaded("another-core"));
        h.answer(cmd::STOP_ALL_SESSIONS, stopped());
        h.defer(desktop::cmd::UPDATE_INSTALL);
    });
    s.look();
    let panes: Vec<_> = s.models.panes.borrow().iter().filter_map(gpui::WeakEntity::upgrade).collect();
    assert!(!panes.is_empty(), "the fixture shows sessions");
    s.click("status-update");
    s.click("update-install");
    // The stop ends every session.
    for pane in &panes {
        let id = pane.read_with(s.cx, |p, _| p.id().to_owned());
        s.tell(host_event::PTY_EXIT, json!({ "id": id, "code": 0, "generation": 1 }));
    }
    assert!(panes.iter().all(|p| p.read_with(s.cx, |p, _| p.ended())));
    assert!(s.host.resolve(desktop::cmd::UPDATE_INSTALL, Err("unsaved: locked".into())));
    s.cx.run_until_parked();
    let starts = |s: &S<'_>| s.host.calls_of(host_cmd::ENSURE_CLAUDE_SESSION).len() + s.host.calls_of(host_cmd::ENSURE_CODEX_SESSION).len();
    let before = starts(&s);
    s.click("update-cancel");
    assert_eq!(s.host.calls_of(cmd::UPDATE_CALLED_OFF).len(), 1);
    assert_eq!(starts(&s), before + panes.len(), "each pane starts its session again");
    assert!(panes.iter().all(|p| !p.read_with(s.cx, |p, _| p.ended())));
    assert!(!frozen(&mut s));
}

#[cfg(debug_assertions)]
#[gpui::test]
fn cancel_starts_again_the_sessions_whose_exit_has_not_come_yet(cx: &mut TestAppContext) {
    let mut s = open_with(cx, |h| {
        h.answer(desktop::cmd::UPDATE_CHECK, offer("another-core"));
        h.answer(desktop::cmd::UPDATE_DOWNLOAD, downloaded("another-core"));
        h.answer(cmd::STOP_ALL_SESSIONS, stopped());
        h.defer(desktop::cmd::UPDATE_INSTALL);
    });
    s.look();
    let panes: Vec<_> = s.models.panes.borrow().iter().filter_map(gpui::WeakEntity::upgrade).collect();
    assert!(!panes.is_empty(), "the fixture shows sessions");
    s.click("status-update");
    s.click("update-install");
    // The stop returned before any exit came.
    assert!(panes.iter().all(|p| !p.read_with(s.cx, |p, _| p.ended())));
    assert!(s.host.resolve(desktop::cmd::UPDATE_INSTALL, Err("unsaved: locked".into())));
    s.cx.run_until_parked();
    // The core numbers the new spawns again.
    s.host.answer(host_cmd::ENSURE_CLAUDE_SESSION, demo::attached("", 2));
    s.host.answer(host_cmd::ENSURE_CODEX_SESSION, demo::attached("", 2));
    let starts = |s: &S<'_>| s.host.calls_of(host_cmd::ENSURE_CLAUDE_SESSION).len() + s.host.calls_of(host_cmd::ENSURE_CODEX_SESSION).len();
    let before = starts(&s);
    s.click("update-cancel");
    assert_eq!(starts(&s), before + panes.len(), "each pane the stop asked to end starts again, its exit come or not");
    // The exits of the sessions before come late: not the new ones' own.
    for pane in &panes {
        let id = pane.read_with(s.cx, |p, _| p.id().to_owned());
        s.tell(host_event::PTY_EXIT, json!({ "id": id, "code": 0, "generation": 1 }));
    }
    assert!(panes.iter().all(|p| !p.read_with(s.cx, |p, _| p.ended())));
    let id = panes[0].read_with(s.cx, |p, _| p.id().to_owned());
    s.tell(host_event::PTY_EXIT, json!({ "id": id, "code": 0, "generation": 2 }));
    assert!(panes[0].read_with(s.cx, |p, _| p.ended()), "the new session's own exit ends it");
}

#[gpui::test]
fn a_click_on_a_pane_changes_nothing_while_the_list_is_frozen(cx: &mut TestAppContext) {
    let mut s = open(cx);
    assert_eq!(s.active().as_deref(), Some("c-parser"));
    s.models.workspace.update(s.cx, |w, cx| w.freeze(cx));
    s.models.workspace.update(s.cx, |w, cx| w.set_active_pane(1, cx));
    assert_eq!(s.active().as_deref(), Some("c-parser"), "what is written for the install stays as it is");
    s.models.workspace.update(s.cx, |w, cx| w.thaw(cx));
    s.models.workspace.update(s.cx, |w, cx| w.set_active_pane(1, cx));
    assert_eq!(s.active().as_deref(), Some("c-codex"));
}

// ------------------------------------------------------- usage: no redraws

#[gpui::test]
fn numbers_taken_again_unchanged_draw_nothing_but_reach_the_agents(cx: &mut TestAppContext) {
    let mut s = open(cx);
    s.tick(usage::GAP);
    let (notifies, sent) = (s.notifies(), s.host.calls_of(cmd::ACCOUNT_USAGE).len());
    let mut later = claude_limits();
    later["fetchedAtMs"] = json!((T0 + 60_000) as f64);
    s.host.answer(cmd::ACCOUNT_LIMITS, later);
    s.wait(Duration::from_secs(61));
    s.wait(usage::GAP);
    assert_eq!(s.notifies(), notifies, "only the time they were taken moved");
    let usage = s.host.calls_of(cmd::ACCOUNT_USAGE);
    assert_eq!(usage.len(), sent + 1);
    assert_eq!(usage.last().unwrap()["usage"]["claude/work"]["takenAt"], "2026-11-19T12:01:00Z");
}

#[gpui::test]
fn a_cli_download_redraws_only_when_its_percent_moves(cx: &mut TestAppContext) {
    let mut s = open(cx);
    let at = |got: u64| json!({ "provider": "codex", "phase": "downloading", "version": null, "got": got, "total": 1000, "error": null });
    s.tell(event::CLI_STATUS, at(0));
    let n = s.notifies();
    s.tell(event::CLI_STATUS, at(3));
    s.tell(event::CLI_STATUS, at(9));
    assert_eq!(s.notifies(), n, "still 0%");
    s.tell(event::CLI_STATUS, at(10));
    assert_eq!(s.notifies(), n + 1);
    assert_eq!(s.v(|v| v.clis()[0].got), 10, "the status is kept whole");
}
