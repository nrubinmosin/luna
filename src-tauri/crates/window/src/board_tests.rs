//! The chats, the sidebar, the boards, the peek and the keys over a fake
//! host, on GPUI's test platform: the state comes from the host or the
//! import, goes back to it as the page's throttled storage wrote it, and
//! every surface's controls do what they say.

use crate::demo::{self, FakeHost};
use crate::dto::{CoreState, LinkState, ThemePref};
use crate::host::{Mailbox, cmd, event, mailbox};
use crate::models::Models;
use crate::root::LunaWindow;
use crate::state::{Peek, Status, WindowState};
use crate::workspace::{self, Workspace};
use gpui::{Context, Entity, TestAppContext, VisualTestContext};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

struct W<'a> {
    host: Arc<FakeHost>,
    mailbox: Mailbox,
    models: Models,
    view: Entity<LunaWindow>,
    cx: &'a mut VisualTestContext,
}

fn up() -> Value {
    serde_json::to_value(CoreState { state: LinkState::Up, core: Some(demo::core()), error: None, revision: 1 }).unwrap()
}

/// The window over a host whose `window_state` answers `kept`, the core up,
/// no session running.
fn open(cx: &mut TestAppContext, kept: Value) -> W<'_> {
    open_with(cx, kept, up(), json!([]))
}

fn open_with(cx: &mut TestAppContext, kept: Value, core: Value, looks: Value) -> W<'_> {
    cx.executor().allow_parking();
    let host = FakeHost::new();
    host.answer(cmd::CORE_STATE, core);
    host.answer(cmd::WINDOW_STATE, kept);
    host.answer(cmd::LIST_ACCOUNTS, demo::accounts());
    host.answer(cmd::SESSIONS_META, looks);
    host.answer(cmd::ENSURE_CLAUDE_SESSION, demo::attached("", 1));
    host.answer(cmd::ENSURE_CODEX_SESSION, demo::attached("", 1));
    let (mailbox, inbox) = mailbox();
    let models = cx.update(|cx| crate::start(host.clone(), inbox, ThemePref::Light, cx));
    let window_models = models.clone();
    let (view, cx) = cx.add_window_view(move |window, cx| LunaWindow::new(window_models, window, cx));
    cx.run_until_parked();
    W { host, mailbox, models, view, cx }
}

/// The fixture's chats kept, on board I in `layout`.
fn chats(cx: &mut TestAppContext, layout: usize) -> W<'_> {
    open(cx, demo::kept(&demo::chats_state(layout, true)))
}

impl W<'_> {
    fn state(&mut self) -> WindowState {
        self.models.workspace.read_with(self.cx, |w, _| w.state.clone())
    }

    fn ws<R>(&mut self, f: impl FnOnce(&mut Workspace, &mut Context<Workspace>) -> R) -> R {
        let r = self.models.workspace.update(self.cx, f);
        self.cx.run_until_parked();
        r
    }

    fn saves(&self) -> Vec<WindowState> {
        self.host
            .sends_of(cmd::SAVE_WINDOW_STATE)
            .iter()
            .map(|a| WindowState::read(&serde_json::from_str(a["text"].as_str().unwrap()).unwrap()).unwrap())
            .collect()
    }

    fn keys(&mut self, keys: &str) {
        self.cx.simulate_keystrokes(keys);
        self.cx.run_until_parked();
    }

    fn seats(&mut self) -> Vec<Option<String>> {
        let s = self.state();
        (0..s.layout()).map(|i| s.seat(i).map(str::to_owned)).collect()
    }

    fn renders(&mut self) -> (usize, usize, usize) {
        self.view.read_with(self.cx, |v, cx| (v.title_bar.read(cx).renders, v.sidebar.read(cx).renders, v.board.read(cx).renders))
    }

    fn terminal_of(&mut self, id: &str) -> Option<gpui::EntityId> {
        self.view.read_with(self.cx, |v, cx| v.board.read(cx).terminal_of(id).map(|t| t.entity_id()))
    }

    fn focus(&mut self, id: &str) {
        let board = self.view.read_with(self.cx, |v, _| v.board.clone());
        let id = id.to_owned();
        self.cx.update(|window, cx| board.update(cx, |b, cx| b.focus_chat(&id, window, cx)));
        self.cx.run_until_parked();
    }
}

fn s(v: &str) -> Option<String> {
    Some(v.to_owned())
}

#[gpui::test]
fn the_first_start_imports_0_19s_list_and_writes_it_at_once(cx: &mut TestAppContext) {
    let old = serde_json::to_value(crate::import::fixture()).unwrap();
    let mut w = open(cx, json!({ "saved": null, "old": old }));
    let state = w.state();
    assert_eq!(state.chats().count(), 7);
    assert_eq!(state.folders.len(), 3);
    assert_eq!(state.layout(), 2);
    assert!(state.imported.as_ref().unwrap().ok);
    assert_eq!(w.models.workspace.read_with(w.cx, |w, _| w.notice()), None);
    let saves = w.saves();
    assert!(!saves.is_empty(), "written at once");
    assert_eq!(saves[0].chats().count(), 7);
}

#[gpui::test]
fn an_import_that_fails_starts_empty_and_says_so_until_dismissed(cx: &mut TestAppContext) {
    let mut w = open(cx, json!({ "saved": null, "oldError": "the store does not open" }));
    assert_eq!(w.state().chats().count(), 0);
    let notice = w.models.workspace.read_with(w.cx, |w, _| w.notice()).unwrap();
    assert!(notice.contains("could not be read") && notice.contains("does not open"), "{notice}");
    // Written, so the next start imports nothing again.
    assert_eq!(w.saves()[0].imported.as_ref().map(|i| i.ok), Some(false));
    w.ws(|w, cx| w.dismiss_notice(cx));
    assert_eq!(w.models.workspace.read_with(w.cx, |w, _| w.notice()), None);
    w.cx.executor().advance_clock(workspace::GAP);
    w.cx.run_until_parked();
    assert!(w.saves().last().unwrap().imported.as_ref().unwrap().seen);
}

#[gpui::test]
fn a_store_without_a_chat_list_is_a_failure(cx: &mut TestAppContext) {
    let w = open(cx, json!({ "saved": null, "old": { "luna.sidebar": "off" } }));
    assert!(w.models.workspace.read_with(w.cx, |w, _| w.notice()).is_some());
}

#[gpui::test]
fn a_machine_0_19_never_ran_on_starts_empty_and_quiet(cx: &mut TestAppContext) {
    let mut w = open(cx, json!({ "saved": null }));
    assert_eq!(w.state().chats().count(), 0);
    assert_eq!(w.models.workspace.read_with(w.cx, |w, _| w.notice()), None);
}

#[gpui::test]
fn a_kept_state_is_read_and_nothing_is_imported_or_written(cx: &mut TestAppContext) {
    let mut w = chats(cx, 3);
    assert_eq!(w.state().layout(), 3);
    assert_eq!(w.seats(), [s("c-parser"), s("c-codex"), s("c-notes")]);
    assert!(w.saves().is_empty(), "nothing changed");
    assert_eq!(w.host.calls_of(cmd::WINDOW_STATE).len(), 1);
}

#[gpui::test]
fn writes_come_at_once_after_a_quiet_spell_then_one_per_gap(cx: &mut TestAppContext) {
    let mut w = chats(cx, 2);
    w.ws(|w, cx| w.set_layout(3, cx));
    assert_eq!(w.saves().len(), 1, "after a quiet spell, at once");
    w.ws(|w, cx| w.set_auto_tile(false, cx));
    w.ws(|w, cx| w.set_auto_tile(true, cx));
    w.ws(|w, cx| w.toggle_sidebar(cx));
    assert_eq!(w.saves().len(), 1, "within the gap they wait");
    w.cx.executor().advance_clock(workspace::GAP);
    w.cx.run_until_parked();
    assert_eq!(w.saves().len(), 2, "and go as one");
    assert!(!w.saves()[1].sidebar.pinned);
    // A chat deleted is written at once, gap or not.
    w.ws(|w, cx| w.set_layout(2, cx));
    let before = w.saves().len();
    w.ws(|w, cx| w.remove_chat("c-old", cx));
    assert_eq!(w.saves().len(), before + 1);
    assert!(w.saves().last().unwrap().find("c-old").is_none());
    // What waits is written as the app quits.
    w.ws(|w, cx| w.set_layout(4, cx));
    let before = w.saves().len();
    w.cx.update(|_, cx| workspace::flush(cx));
    w.cx.run_until_parked();
    assert_eq!(w.saves().len(), before + 1);
    assert_eq!(w.saves().last().unwrap().layout(), 4);
}

#[gpui::test]
fn the_sessions_meta_every_four_seconds_keeps_rows_and_notifies_once(cx: &mut TestAppContext) {
    let mut w = chats(cx, 2);
    w.host.answer(cmd::SAVED_TITLE, json!({ "title": "Saved title", "renamed": false }));
    w.host.answer(
        cmd::SESSIONS_META,
        json!([
            { "id": "c-parser", "meta": { "status": "waiting", "context": 0.5, "draft": true, "held": 0, "title": "Parser fix" } },
            { "id": "c-notes", "meta": { "status": "busy", "context": 0.2, "firstPrompt": "write the notes", "sessionId": "sess-notes" } },
        ]),
    );
    w.cx.executor().advance_clock(workspace::EVERY);
    w.cx.run_until_parked();
    let state = w.state();
    assert_eq!(state.find("c-parser").unwrap().status, Status::Waiting);
    assert_eq!(state.find("c-notes").unwrap().status, Status::Working);
    assert_eq!(state.find("c-notes").unwrap().session_id.as_deref(), Some("sess-notes"));
    assert_eq!(state.find("c-codex").unwrap().status, Status::Resting, "not running");
    // Not running, with a session and a name it may still lose: its saved
    // title, looked up once.
    assert_eq!(state.find("c-codex").unwrap().name, "Saved title");
    let live = w.models.workspace.read_with(w.cx, |w, _| w.live("c-parser"));
    assert_eq!((live.context, live.draft_tag().as_deref()), (Some(0.5), Some("draft")));
    let notices = w.host.desktop.notices.lock().unwrap().clone();
    // c-parser was imported waiting: it was announced already, no new one.
    assert!(notices.is_empty(), "{notices:?}");
    // It rests, then waits again: one notification, with its chat.
    w.host.answer(cmd::SESSIONS_META, json!([]));
    w.cx.executor().advance_clock(workspace::EVERY);
    w.cx.run_until_parked();
    w.host.answer(cmd::SESSIONS_META, json!([{ "id": "c-parser", "meta": { "status": "waiting_for_input" } }]));
    for _ in 0..2 {
        w.cx.executor().advance_clock(workspace::EVERY);
        w.cx.run_until_parked();
    }
    let notices = w.host.desktop.notices.lock().unwrap().clone();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].title, "Claude Code is waiting");
    assert_eq!(notices[0].chat_id.as_deref(), Some("c-parser"));
    assert!(!notices[0].sound, "the fixture's sound setting is off");
    assert_eq!(w.host.calls_of(cmd::SAVED_TITLE).iter().filter(|a| a["sessionId"] == "0d6f1c2e-0000-4000-8000-000000000002").count(), 1);
    // Every chat with an account is asked about, by its account's path.
    let asked = w.host.calls_of(cmd::SESSIONS_META).last().cloned().unwrap();
    assert!(
        asked["chats"].as_array().unwrap().iter().any(|c| c["id"] == "c-codex" && c["accountPath"] == "C:\\accounts\\openai\\personal")
    );
}

#[gpui::test]
fn the_cores_notices_and_a_sessions_exit_reach_the_window(cx: &mut TestAppContext) {
    let mut w = chats(cx, 1);
    w.host.answer(cmd::SESSIONS_META, json!([{ "id": "c-parser", "meta": { "status": "busy" } }]));
    w.cx.executor().advance_clock(workspace::EVERY);
    w.cx.run_until_parked();
    assert_eq!(w.state().find("c-parser").unwrap().status, Status::Working);
    assert!(w.mailbox.event(event::NOTIFY, json!({ "title": "Shutting down", "body": "in 60 s", "chatId": null })));
    assert!(w.mailbox.event(event::PTY_EXIT, json!({ "id": "c-parser", "code": 0, "generation": 1 })));
    w.cx.run_until_parked();
    assert_eq!(w.host.desktop.notices.lock().unwrap()[0].title, "Shutting down");
    assert_eq!(w.state().find("c-parser").unwrap().status, Status::Resting);
}

#[gpui::test]
fn a_notifications_click_opens_its_chat_over_the_board(cx: &mut TestAppContext) {
    let mut w = chats(cx, 2);
    assert!(w.mailbox.event(crate::desktop::event::OPEN_CHAT, json!({ "chatId": "c-site" })));
    w.cx.run_until_parked();
    let state = w.state();
    assert_eq!(state.board, 1, "its own board");
    assert_eq!(state.peek, Some(Peek::Chat("c-site".into())));
}

#[gpui::test]
fn panes_show_their_chats_terminals_and_a_peek_its_own(cx: &mut TestAppContext) {
    let mut w = chats(cx, 2);
    let parser = w.terminal_of("c-parser").expect("a terminal for a seated chat");
    assert!(w.terminal_of("c-codex").is_some());
    assert!(w.terminal_of("c-site").is_none());
    let started = w.host.calls_of(cmd::ENSURE_CLAUDE_SESSION);
    let args = started.iter().find(|a| a["id"] == "c-parser").unwrap();
    assert_eq!(args["accountPath"], "C:\\accounts\\anthropic\\work");
    assert_eq!(args["folder"], "C:\\work\\parser");
    assert_eq!(args["permissionMode"], "default");
    assert_eq!(args["model"], "opus");
    assert_eq!(args["resume"], "0d6f1c2e-0000-4000-8000-000000000001");
    w.ws(|w, cx| w.set_board(0, cx));
    // A row's click on a chat not on the board: up over it, its terminal
    // made; To board seats it, the board growing by one.
    w.ws(|w, cx| w.show_chat("c-notes", cx));
    assert_eq!(w.state().peek, Some(Peek::Chat("c-notes".into())));
    assert!(w.terminal_of("c-notes").is_some());
    w.ws(|w, cx| w.to_board("c-notes", cx));
    assert_eq!(w.state().peek, None);
    assert_eq!(w.seats(), [s("c-parser"), s("c-codex"), s("c-notes")]);
    // The same terminal all along.
    assert_eq!(w.terminal_of("c-parser"), Some(parser));
}

#[gpui::test]
fn put_swap_unseat_layouts_boards_and_clear(cx: &mut TestAppContext) {
    let mut w = chats(cx, 2);
    w.ws(|w, cx| w.swap(0, 1, cx));
    assert_eq!(w.seats(), [s("c-codex"), s("c-parser")]);
    w.ws(|w, cx| w.put(0, "c-notes", cx));
    assert_eq!(w.seats(), [s("c-notes"), s("c-parser")]);
    w.ws(|w, cx| w.unseat(1, cx));
    assert_eq!(w.seats(), [s("c-notes")], "auto-tile closed the gap");
    w.ws(|w, cx| w.set_layout(4, cx));
    assert_eq!(w.state().layout(), 4);
    w.ws(|w, cx| w.next_board(cx));
    assert_eq!(w.state().board, 1);
    w.ws(|w, cx| w.set_board(0, cx));
    w.ws(|w, cx| w.clear_board(cx));
    assert!(w.seats().iter().all(Option::is_none));
    assert_eq!(w.state().chats().count(), 7);
}

#[gpui::test]
fn a_chat_whose_session_ended_starts_anew_when_shown_again(cx: &mut TestAppContext) {
    let mut w = chats(cx, 1);
    let first = w.terminal_of("c-parser").unwrap();
    assert!(w.mailbox.event(event::PTY_EXIT, json!({ "id": "c-parser", "code": 0, "generation": 1 })));
    w.cx.run_until_parked();
    assert_eq!(w.terminal_of("c-parser"), Some(first), "shown on: it says it exited");
    w.ws(|w, cx| w.show_chat("c-codex", cx));
    w.ws(|w, cx| w.show_chat("c-parser", cx));
    let again = w.terminal_of("c-parser").unwrap();
    assert_ne!(again, first);
    // A chat that did not end comes back to the terminal it had (warm).
    let codex = w.terminal_of("c-codex").unwrap();
    w.ws(|w, cx| w.show_chat("c-codex", cx));
    assert_eq!(w.terminal_of("c-codex"), Some(codex));
}

#[gpui::test]
fn a_pane_waits_for_the_core_and_for_an_account_it_knows(cx: &mut TestAppContext) {
    let starting = serde_json::to_value(CoreState::default()).unwrap();
    let mut w = open_with(cx, demo::kept(&demo::chats_state(1, true)), starting, json!([]));
    assert!(w.terminal_of("c-parser").is_some(), "a terminal waiting for the core");
    assert!(w.host.calls_of(cmd::ENSURE_CLAUDE_SESSION).is_empty(), "nothing asked of no core");
    w.ws(|w, cx| {
        let c = w.state.find_mut("c-parser").unwrap();
        c.account = "nobody".into();
        cx.notify();
    });
    let why = w.models.workspace.read_with(w.cx, |w, _| w.spec_of("c-parser").unwrap_err());
    assert!(why.contains("not one Luna knows"), "{why}");
}

#[gpui::test]
fn the_windows_keys_work_with_no_terminal_focused(cx: &mut TestAppContext) {
    let mut w = chats(cx, 2);
    w.keys("ctrl-shift-1");
    assert_eq!(w.state().layout(), 1);
    w.keys("ctrl-shift-4");
    assert_eq!(w.state().layout(), 4);
    // The keyboard's terminal taken off the board: the keys still work.
    w.focus("c-parser");
    w.ws(|w, cx| w.unseat(0, cx));
    w.keys("ctrl-shift-2");
    assert_eq!(w.state().layout(), 2);
}

#[gpui::test]
fn the_windows_keys_do_what_the_pages_did(cx: &mut TestAppContext) {
    let mut w = chats(cx, 2);
    w.focus("c-parser");
    w.keys("ctrl-shift-3");
    assert_eq!(w.state().layout(), 3);
    w.keys("ctrl-shift-1");
    assert_eq!(w.state().layout(), 1);
    // One pane: the digits count through the list.
    w.keys("ctrl-2");
    assert_eq!(w.seats(), [s("c-codex")]);
    w.keys("ctrl-shift-2");
    w.keys("ctrl-2");
    assert_eq!(w.state().active_pane, 1);
    w.keys("ctrl-tab");
    assert_eq!(w.state().board, 1);
    w.keys("ctrl-tab ctrl-tab ctrl-tab ctrl-tab");
    assert_eq!(w.state().board, 0);
    // Ctrl+Shift+W: the next chat waiting, up over the board if it has no pane.
    w.ws(|w, cx| {
        w.state.find_mut("c-site").unwrap().status = Status::Waiting;
        w.state.find_mut("c-parser").unwrap().status = Status::Resting;
        cx.notify();
    });
    w.keys("ctrl-shift-w");
    assert_eq!(w.state().board, 1);
    assert_eq!(w.state().peek, Some(Peek::Chat("c-site".into())));
    // Esc puts it back.
    w.keys("escape");
    assert_eq!(w.state().peek, None);
}

#[gpui::test]
fn esc_and_a_digit_that_mean_nothing_go_on_to_the_terminal(cx: &mut TestAppContext) {
    let mut w = chats(cx, 2);
    w.focus("c-parser");
    let before = w.host.sends_of(cmd::WRITE_SESSION).len();
    w.keys("escape");
    w.keys("ctrl-4");
    let typed: Vec<Value> = w.host.sends_of(cmd::WRITE_SESSION)[before..].to_vec();
    assert!(typed.iter().any(|a| a["id"] == "c-parser" && a["data"] == "\u{1b}"), "{typed:?}");
    assert_eq!(w.state().layout(), 2);
}

#[gpui::test]
fn new_chat_and_delete_go_to_their_seams(cx: &mut TestAppContext) {
    let mut w = chats(cx, 2);
    let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (a, b, c) = (asked.clone(), asked.clone(), asked.clone());
    w.ws(move |w, _| {
        w.seams.new_chat = Some(std::rc::Rc::new(move |_, _| a.lock().unwrap().push("new".to_string())));
        w.seams.quick_chat = Some(std::rc::Rc::new(move |_, _| b.lock().unwrap().push("quick".to_string())));
        w.seams.delete_chat = Some(std::rc::Rc::new(move |id, _, _| c.lock().unwrap().push(format!("delete {id}"))));
    });
    w.focus("c-parser");
    w.keys("ctrl-n");
    w.keys("ctrl-shift-n");
    assert_eq!(*asked.lock().unwrap(), ["new", "quick"]);
    // What the new-chat dialog then does: the chat listed and seated.
    let mut chat = w.state().find("c-parser").unwrap().clone();
    chat.id = "c-new".into();
    let placed = w.ws(|w, cx| w.add_chat("C:\\work\\parser", chat, true, cx));
    assert_eq!(placed, crate::state::Placed::Pane(2));
    assert_eq!(w.saves().last().unwrap().find("c-new").map(|c| c.id.as_str()), Some("c-new"));
}

#[gpui::test]
fn renaming_in_a_pane_keeps_the_name_on_enter_and_lets_it_go_on_esc(cx: &mut TestAppContext) {
    let mut w = chats(cx, 2);
    let board = w.view.read_with(w.cx, |v, _| v.board.clone());
    w.cx.update(|window, cx| board.update(cx, |b, cx| b.start_rename("c-parser", window, cx)));
    w.cx.run_until_parked();
    let input = board.read_with(w.cx, |b, _| b.renaming.as_ref().unwrap().input.clone());
    w.cx.update(|window, cx| input.update(cx, |i, cx| i.set_value("Parser, renamed", window, cx)));
    w.keys("enter");
    let c = w.state().find("c-parser").unwrap().clone();
    assert_eq!((c.name.as_str(), c.name_source), ("Parser, renamed", Some(crate::state::NameSource::User)));
    w.cx.update(|window, cx| board.update(cx, |b, cx| b.start_rename("c-codex", window, cx)));
    w.cx.run_until_parked();
    w.keys("escape");
    assert!(board.read_with(w.cx, |b, _| b.renaming.is_none()));
    assert_eq!(w.state().find("c-codex").unwrap().name, "Lexer tests");
}

/// An idle window with chats on its board: the sessions are looked at every
/// four seconds, and with nothing new nothing redraws.
#[gpui::test]
fn nothing_redraws_while_the_sessions_say_nothing_new(cx: &mut TestAppContext) {
    let mut w = open_with(cx, demo::kept(&demo::chats_state(4, true)), up(), demo::looks());
    w.cx.executor().advance_clock(workspace::EVERY);
    w.cx.run_until_parked();
    let before = w.renders();
    let rounds = w.models.workspace.read_with(w.cx, |w, _| w.rounds);
    w.cx.executor().advance_clock(Duration::from_secs(60));
    w.cx.run_until_parked();
    assert!(w.models.workspace.read_with(w.cx, |w, _| w.rounds) >= rounds + 14);
    assert_eq!(w.renders(), before);
}

/// The controls themselves, clicked where they are drawn. Selectors are
/// recorded only in builds with debug assertions.
#[cfg(debug_assertions)]
mod clicks {
    use super::*;
    use gpui::Modifiers;

    fn click(w: &mut W<'_>, selector: &'static str) {
        let at = w.cx.debug_bounds(selector).unwrap_or_else(|| panic!("no {selector} drawn")).center();
        w.cx.simulate_click(at, Modifiers::none());
        w.cx.run_until_parked();
    }

    #[gpui::test]
    fn the_title_bars_boards_layouts_clear_and_auto_tile(cx: &mut TestAppContext) {
        let mut w = chats(cx, 2);
        click(&mut w, "board-3");
        assert_eq!(w.state().board, 2);
        click(&mut w, "board-1");
        click(&mut w, "layout-4");
        assert_eq!(w.state().layout(), 4);
        click(&mut w, "auto-tile");
        assert!(!w.state().auto_tile);
        click(&mut w, "clear-board");
        assert!(w.seats().iter().all(Option::is_none));
    }

    #[gpui::test]
    fn the_sidebars_rows_folders_and_pin(cx: &mut TestAppContext) {
        let mut w = chats(cx, 2);
        click(&mut w, "row-c-notes");
        assert_eq!(w.state().peek, Some(Peek::Chat("c-notes".into())));
        click(&mut w, "peek-to-board");
        assert_eq!(w.seats(), [s("c-parser"), s("c-codex"), s("c-notes")]);
        click(&mut w, "folder-f1");
        assert!(!w.state().folders[0].open);
        assert!(w.cx.debug_bounds("row-c-codex").is_none(), "folded away");
        click(&mut w, "sidebar-pin");
        assert!(!w.state().sidebar.pinned);
        click(&mut w, "sidebar-pin");
        assert!(w.state().sidebar.pinned);
    }

    #[gpui::test]
    fn a_panes_menu_moves_swaps_and_unseats_and_its_id_is_copied(cx: &mut TestAppContext) {
        let mut w = chats(cx, 2);
        w.ws(|w, cx| w.set_auto_tile(false, cx));
        click(&mut w, "pane-1-menu");
        click(&mut w, "menu-swap-2");
        assert_eq!(w.seats(), [s("c-codex"), s("c-parser")]);
        click(&mut w, "pane-2-menu");
        click(&mut w, "menu-move-1");
        assert_eq!(w.seats(), [s("c-parser"), None]);
        click(&mut w, "pane-1-copy-id");
        assert_eq!(*w.host.desktop.written.lock().unwrap(), ["0d6f1c2e-0000-4000-8000-000000000001"]);
        click(&mut w, "pane-1-unseat");
        assert_eq!(w.seats(), [None, None]);
    }

    #[gpui::test]
    fn the_peeks_close_and_a_rows_delete(cx: &mut TestAppContext) {
        let mut w = chats(cx, 2);
        let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
        let a = asked.clone();
        w.ws(move |w, _| w.seams.delete_chat = Some(std::rc::Rc::new(move |id, _, _| a.lock().unwrap().push(id))));
        click(&mut w, "row-c-notes");
        click(&mut w, "peek-close");
        assert_eq!(w.state().peek, None);
        // The active row offers Delete.
        click(&mut w, "row-delete");
        assert_eq!(*asked.lock().unwrap(), ["c-notes"]);
    }

    #[gpui::test]
    fn the_import_notices_ok_dismisses_it(cx: &mut TestAppContext) {
        let mut w = open(cx, json!({ "saved": null, "oldError": "x" }));
        click(&mut w, "import-notice-ok");
        assert!(w.cx.debug_bounds("import-notice").is_none());
    }
}
