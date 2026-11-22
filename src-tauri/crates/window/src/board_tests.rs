//! The chats, the sidebar, the boards, the peek and the keys over a fake
//! host, on GPUI's test platform: the state comes from the host or the
//! import, goes back to it as the page's throttled storage wrote it, and
//! every surface's controls do what they say.

use crate::demo::{self, FakeHost};
use crate::dto::{CoreState, LinkState, ThemePref};
use crate::host::{Mailbox, cmd, event, mailbox};
use crate::models::Models;
use crate::panels::{Prefs, cmd as panels_cmd};
use crate::root::LunaWindow;
use crate::state::{Peek, Status, WindowState};
use crate::workspace::{self, Workspace};
use gpui::{Context, Entity, Focusable, TestAppContext, VisualTestContext};
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
    open_as(cx, kept, core, looks, |_| {})
}

/// As `open_with`, the host also answering as `setup` has it.
fn open_as(cx: &mut TestAppContext, kept: Value, core: Value, looks: Value, setup: impl FnOnce(&FakeHost)) -> W<'_> {
    cx.executor().allow_parking();
    let host = FakeHost::new();
    setup(&host);
    host.answer(cmd::CORE_STATE, core);
    host.answer(cmd::WINDOW_STATE, kept);
    host.answer(cmd::LIST_ACCOUNTS, demo::accounts());
    host.answer(cmd::SESSIONS_META, looks);
    host.answer(cmd::SAVED_TITLE, json!({ "title": "Saved title", "renamed": false }));
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
            .map(|a| WindowState::read(&serde_json::from_str(a["text"].as_str().unwrap()).unwrap()).unwrap().0)
            .collect()
    }

    fn notices(&mut self) -> Vec<workspace::StateNotice> {
        self.models.workspace.read_with(self.cx, |w, _| w.notices())
    }

    fn notice_of(&mut self, key: &str) -> Option<String> {
        self.notices().into_iter().find(|n| n.key == key).map(|n| n.text)
    }

    fn tick(&mut self) {
        self.cx.executor().advance_clock(workspace::EVERY);
        self.cx.run_until_parked();
    }

    fn event(&mut self, name: &str, payload: Value) {
        assert!(self.mailbox.event(name, payload));
        self.cx.run_until_parked();
    }

    fn core(&mut self, link: Value) {
        self.event(event::CORE_STATE, link);
    }

    fn focused_terminal(&mut self) -> Option<String> {
        let board = self.view.read_with(self.cx, |v, _| v.board.clone());
        self.cx.update(|window, cx| {
            let b = board.read(cx);
            b.terms_for_tests().into_iter().find(|(_, t)| t.read(cx).focus_handle(cx).is_focused(window)).map(|(id, _)| id)
        })
    }

    fn peek_frame_focused(&mut self) -> bool {
        let board = self.view.read_with(self.cx, |v, _| v.board.clone());
        self.cx.update(|window, cx| board.read(cx).peek_focus.is_focused(window))
    }

    fn typed(&self, id: &str) -> Vec<String> {
        self.host.sends_of(cmd::WRITE_SESSION).iter().filter(|a| a["id"] == id).map(|a| a["data"].as_str().unwrap().to_owned()).collect()
    }
}

/// The core up anew: another pid, a newer revision (a core replaced).
fn replaced(revision: u64) -> Value {
    let mut core = demo::core();
    core.pid += revision as u32;
    serde_json::to_value(CoreState { state: LinkState::Up, core: Some(core), error: None, revision }).unwrap()
}

fn down(revision: u64) -> Value {
    serde_json::to_value(CoreState { state: LinkState::Down, core: None, error: Some("gone".into()), revision }).unwrap()
}

impl W<'_> {
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

/// 0.19's sound and new-chat boxes go to the host as its list is
/// imported, which keeps them where Settings does; the window takes the
/// host's answer, so the new-chat dialog starts as 0.19's did, and a
/// notification plays as Settings says from then on.
#[gpui::test]
fn the_import_hands_0_19s_choices_to_settings_and_the_window_follows_settings(cx: &mut TestAppContext) {
    let old = serde_json::to_value(crate::import::fixture()).unwrap();
    let mut w = open_as(cx, json!({ "saved": null, "old": old }), up(), json!([]), |h| {
        h.respond(panels_cmd::IMPORT_WINDOW_PREFS, |a| json!({ "prefs": { "sound": a["sound"], "newChat": a["newChat"] }, "theme": null }));
    });
    let asked = w.host.calls_of(panels_cmd::IMPORT_WINDOW_PREFS);
    assert_eq!(asked, [json!({ "sound": false, "newChat": { "worktree": false, "tools": true }, "theme": "dark" })]);
    assert_eq!(w.models.shell.read_with(w.cx, |s, _| s.pref()), ThemePref::Light, "the host kept a theme of its own");
    let prefs = w.models.workspace.read_with(w.cx, |w, _| w.prefs().clone());
    assert!(!prefs.sound);
    // The list was written with them at once, and again without them once
    // the host had them.
    w.cx.executor().advance_clock(workspace::GAP);
    w.cx.run_until_parked();
    let saves = w.saves();
    assert!(saves[0].prefs_to_import.is_some() && saves.last().unwrap().prefs_to_import.is_none());
    let start = w.models.workspace.read_with(w.cx, |w, _| w.new_chat_start());
    assert!(!start.worktree && start.tools, "as 0.19's dialog started");
    w.ws(|w, _| w.on_notice("Done".into(), "It is done".into(), None));
    // Settings turns it on: the next one plays it.
    w.ws(|w, cx| w.take_prefs(Prefs { sound: true, ..prefs }, cx));
    w.ws(|w, _| w.on_notice("Done".into(), "Again".into(), None));
    let sounds: Vec<bool> = w.host.desktop.notices.lock().unwrap().iter().map(|n| n.sound).collect();
    assert_eq!(sounds, [false, true]);
}

/// The 0.19 → 0.20.0 → 0.20.1 path: a list 0.20.0 wrote after importing
/// 0.19's kept its sound and new-chat boxes in the window's own file, where
/// Settings did not look, and no theme; the host reads 0.19's store again
/// for it (`oldPrefs`). All go to the host, and once it has them the list
/// is written without them, so the store is not read again.
#[gpui::test]
fn choices_0_20_0_imported_into_the_list_go_to_settings(cx: &mut TestAppContext) {
    let mut saved = serde_json::to_value(demo::chats_state(1, true)).unwrap();
    saved["notify"] = json!({ "sound": false });
    saved["newChat"]["worktree"] = json!(false);
    saved["newChat"]["tools"] = json!(true);
    let store = json!({ "luna.theme": "dark", "luna.chats": "{}" });
    let mut w = open_as(cx, json!({ "saved": saved, "oldPrefs": store }), up(), json!([]), |h| {
        h.respond(
            panels_cmd::IMPORT_WINDOW_PREFS,
            |a| json!({ "prefs": { "sound": a["sound"], "newChat": a["newChat"] }, "theme": a["theme"] }),
        );
    });
    let asked = w.host.calls_of(panels_cmd::IMPORT_WINDOW_PREFS);
    assert_eq!(asked, [json!({ "sound": false, "newChat": { "worktree": false, "tools": true }, "theme": "dark" })]);
    assert!(!w.models.workspace.read_with(w.cx, |w, _| w.prefs().sound));
    assert_eq!(w.models.shell.read_with(w.cx, |s, _| s.pref()), ThemePref::Dark);
    assert_eq!(w.state().chats().count(), 7, "the list is not imported again");
    let written: Value = serde_json::from_str(w.host.sends_of(cmd::SAVE_WINDOW_STATE).last().unwrap()["text"].as_str().unwrap()).unwrap();
    assert_eq!(written.get("notify"), None);
    assert_eq!(written["newChat"].get("worktree"), None);
    assert_eq!(written.get("prefsToImport"), None);
    assert!(!crate::import::reads_old_prefs_again(&written));
}

/// 0.19's choices stay in the list until the host has written them: the
/// import's write of the list comes first and holds them, and a host that
/// could not write them (or a quit before its answer) leaves them there for
/// the next start, which hands them over again.
#[gpui::test]
fn choices_the_host_did_not_keep_are_handed_over_at_the_next_start(cx: &mut TestAppContext) {
    let old = serde_json::to_value(crate::import::fixture()).unwrap();
    let mut w = open_as(cx, json!({ "saved": null, "old": old }), up(), json!([]), |h| {
        h.answer(panels_cmd::WINDOW_PREFS, json!({}));
        h.fail(panels_cmd::IMPORT_WINDOW_PREFS, Some("disk full"));
    });
    let saves = w.host.sends_of(cmd::SAVE_WINDOW_STATE);
    assert!(!saves.is_empty());
    let first: Value = serde_json::from_str(saves[0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(first["prefsToImport"]["sound"], json!(false), "written with the list, before the host has them");
    assert!(w.state().prefs_to_import.is_some(), "still to hand over");
    // Settings is not kept waiting by a failure.
    assert!(w.models.workspace.read_with(w.cx, |w, _| w.settled_prefs().is_some()));
}

#[gpui::test]
fn choices_kept_in_the_list_are_handed_over_again(cx: &mut TestAppContext) {
    let mut saved = serde_json::to_value(demo::chats_state(1, true)).unwrap();
    saved["prefsToImport"] = json!({ "sound": false, "theme": "dark" });
    let mut w = open_as(cx, json!({ "saved": saved }), up(), json!([]), |h| {
        h.answer(panels_cmd::IMPORT_WINDOW_PREFS, json!({ "prefs": { "sound": false }, "theme": "dark" }));
    });
    assert_eq!(w.host.calls_of(panels_cmd::IMPORT_WINDOW_PREFS), [json!({ "sound": false, "theme": "dark" })]);
    assert_eq!(w.state().prefs_to_import, None);
    let written: Value = serde_json::from_str(w.host.sends_of(cmd::SAVE_WINDOW_STATE).last().unwrap()["text"].as_str().unwrap()).unwrap();
    assert_eq!(written.get("prefsToImport"), None);
}

/// 0.19's theme, taken by the host where none was kept here, is the
/// window's at once; one chosen with the button meanwhile stays. A store
/// that kept only its theme has no list to import, and still gives it.
#[gpui::test]
fn the_theme_the_host_took_from_0_19_is_shown(cx: &mut TestAppContext) {
    let old = json!({ "luna.theme": "dark", "luna.sidebar": "off" });
    let mut w = open_as(cx, json!({ "saved": null, "old": old }), up(), json!([]), |h| {
        h.answer(panels_cmd::IMPORT_WINDOW_PREFS, json!({ "prefs": {}, "theme": "dark" }));
    });
    assert_eq!(w.host.calls_of(panels_cmd::IMPORT_WINDOW_PREFS), [json!({ "theme": "dark" })]);
    assert_eq!(w.models.shell.read_with(w.cx, |s, _| s.pref()), ThemePref::Dark);
    w.cx.update(|_, cx| w.models.cycle_theme(cx));
    w.ws(|_, cx| cx.emit(workspace::ThemeTaken(ThemePref::Light)));
    assert_eq!(w.models.shell.read_with(w.cx, |s, _| s.pref()), ThemePref::System, "the button's choice stays");
}

#[gpui::test]
fn a_list_that_imported_nothing_hands_no_choices_to_settings(cx: &mut TestAppContext) {
    let mut saved = serde_json::to_value(demo::chats_state(1, true)).unwrap();
    saved["notify"] = json!({ "sound": false });
    saved["newChat"]["tools"] = json!(true);
    saved.as_object_mut().unwrap().remove("imported");
    let w = open(cx, json!({ "saved": saved }));
    assert!(w.host.calls_of(panels_cmd::IMPORT_WINDOW_PREFS).is_empty());
}

/// Settings has no preferences to show or change while the host takes
/// 0.19's, and then has the host's answer: a change made then is never
/// made over a copy older than the import.
#[gpui::test]
fn settings_waits_for_the_import_of_0_19s_choices(cx: &mut TestAppContext) {
    let old = serde_json::to_value(crate::import::fixture()).unwrap();
    let w = open_as(cx, json!({ "saved": null, "old": old }), up(), json!([]), |h| h.defer(panels_cmd::IMPORT_WINDOW_PREFS));
    assert_eq!(w.models.workspace.read_with(w.cx, |w, _| w.settled_prefs().cloned()), None);
    let imported = json!({ "sound": false, "newChat": { "worktree": false, "tools": true } });
    assert!(w.host.resolve(panels_cmd::IMPORT_WINDOW_PREFS, Ok(json!({ "prefs": imported }))));
    w.cx.run_until_parked();
    let settled = w.models.workspace.read_with(w.cx, |w, _| w.settled_prefs().cloned()).unwrap();
    assert_eq!(serde_json::to_value(settled).unwrap(), imported);
}

#[gpui::test]
fn an_import_that_fails_starts_empty_and_says_so_until_dismissed(cx: &mut TestAppContext) {
    let mut w = open(cx, json!({ "saved": null, "oldError": "the store does not open" }));
    assert_eq!(w.state().chats().count(), 0);
    let notice = w.models.workspace.read_with(w.cx, |w, _| w.notice()).unwrap();
    assert!(notice.contains("could not be read") && notice.contains("does not open"), "{notice}");
    // Not written: the next start (or Retry) tries again.
    assert!(w.saves().is_empty());
    let import = w.notices().into_iter().find(|n| n.key == "import").unwrap();
    assert!(import.retry && import.ok);
    w.ws(|w, cx| w.dismiss_notice(cx));
    assert_eq!(w.models.workspace.read_with(w.cx, |w, _| w.notice()), None);
    w.cx.executor().advance_clock(workspace::GAP);
    w.cx.run_until_parked();
    assert!(w.saves().last().unwrap().imported.as_ref().unwrap().seen);
}

#[gpui::test]
fn a_store_without_a_chat_list_is_nothing_to_import(cx: &mut TestAppContext) {
    let mut w = open(cx, json!({ "saved": null, "old": { "luna.theme": "dark", "luna.sidebar": "off" } }));
    assert_eq!(w.models.workspace.read_with(w.cx, |w, _| w.notice()), None);
    assert!(w.notices().is_empty());
    assert_eq!(w.saves().len(), 1, "written, so a later start does not look again");
}

#[gpui::test]
fn a_list_that_holds_no_folders_is_a_failed_import_not_an_empty_one(cx: &mut TestAppContext) {
    let old = json!({ "luna.chats": r#"{"state":{},"version":2}"# });
    let mut w = open(cx, json!({ "saved": null, "old": old }));
    assert!(w.notice_of("import").unwrap().contains("no folder list"));
    assert!(w.saves().is_empty());
}

/// A list older than the import reads (v0.1.0's, version 0) is said in a
/// line with no Retry, nothing of it is written, and once seen it is not
/// asked for again.
#[gpui::test]
fn a_list_too_old_to_read_says_so_without_retry_and_writes_nothing(cx: &mut TestAppContext) {
    let old = json!({ "luna.chats": r#"{"state":{"folders":[{"path":"/x","chats":[]}]},"version":0}"# });
    let mut w = open(cx, json!({ "saved": null, "old": old }));
    let import = w.notices().into_iter().find(|n| n.key == "import").unwrap();
    assert!(import.text.contains("from an older Luna could not be imported"), "{}", import.text);
    assert!(!import.retry && import.ok);
    assert_eq!(w.state().folders.len(), 0);
    assert!(w.saves().is_empty(), "nothing written");
    assert!(!w.state().import_retryable());
    w.ws(|w, cx| w.retry("import", cx));
    assert_eq!(w.host.calls_of(cmd::WINDOW_STATE).len(), 1, "Retry asks nothing");
    w.ws(|w, cx| w.dismiss_notice(cx));
    w.cx.executor().advance_clock(workspace::GAP);
    w.cx.run_until_parked();
    let imported = w.saves().last().unwrap().imported.clone().unwrap();
    assert!(imported.older && imported.seen && imported.why.unwrap().contains("version 0"));
}

/// An import that left rows out says so, naming the copy the host kept.
#[gpui::test]
fn an_import_that_left_rows_out_names_the_copy_kept(cx: &mut TestAppContext) {
    let list = json!({ "state": { "folders": [
        { "id": "f1", "path": "/a", "chats": [{ "id": "c1", "provider": "claude", "account": "w" }, "not a chat"] },
        { "id": "f2" },
    ] }, "version": 2 });
    let old = json!({ "luna.chats": list.to_string() });
    let mut w = open(cx, json!({ "saved": null, "old": old, "oldKept": "C:\\data\\window\\chats.kept-1-ab.json" }));
    assert_eq!(w.state().chats().count(), 1);
    let kept = w.notice_of("kept").unwrap();
    assert!(kept.contains("1 chat could not be read") && kept.contains("1 folder could not be read"), "{kept}");
    assert!(kept.contains("chats.kept-1-ab.json"), "{kept}");
    assert_eq!(w.saves().last().unwrap().chats().count(), 1, "what read is written");
}

/// A failed import is tried again by Retry while the list is still empty:
/// it then imports, and is written.
#[gpui::test]
fn retry_imports_what_failed_before(cx: &mut TestAppContext) {
    let mut w = open(cx, json!({ "saved": null, "oldError": "the store changed while it was copied" }));
    assert_eq!(w.state().chats().count(), 0);
    let old = serde_json::to_value(crate::import::fixture()).unwrap();
    w.host.answer(cmd::WINDOW_STATE, json!({ "saved": null, "old": old }));
    w.ws(|w, cx| w.retry("import", cx));
    assert_eq!(w.state().chats().count(), 7);
    assert_eq!(w.notice_of("import"), None);
    assert_eq!(w.saves().last().unwrap().chats().count(), 7);
    // Once there are chats, Retry asks nothing.
    w.ws(|w, cx| w.retry("import", cx));
    assert_eq!(w.host.calls_of(cmd::WINDOW_STATE).len(), 2);
}

/// Retry asks one read at a time, and an answer that comes after the list
/// was changed is let go of: it would put the list back over the change.
#[gpui::test]
fn a_retry_is_asked_once_and_a_late_answer_does_not_undo_a_change(cx: &mut TestAppContext) {
    let mut w = open(cx, json!({ "saved": null, "oldError": "the store changed while it was copied" }));
    let pinned = w.state().sidebar.pinned;
    w.host.defer(cmd::WINDOW_STATE);
    w.ws(|w, cx| w.retry("import", cx));
    w.ws(|w, cx| w.retry("import", cx));
    assert_eq!(w.host.calls_of(cmd::WINDOW_STATE).len(), 2, "one read under way at a time");
    w.ws(|w, cx| w.toggle_sidebar(cx));
    let old = serde_json::to_value(crate::import::fixture()).unwrap();
    assert!(w.host.resolve(cmd::WINDOW_STATE, Ok(json!({ "saved": null, "old": old.clone() }))));
    w.cx.run_until_parked();
    assert_eq!(w.state().chats().count(), 0, "the late answer let go of");
    assert_eq!(w.state().sidebar.pinned, !pinned, "the change stays");
    // Asked again, with nothing changed meanwhile: taken.
    w.ws(|w, cx| w.retry("import", cx));
    assert_eq!(w.host.calls_of(cmd::WINDOW_STATE).len(), 3);
    assert!(w.host.resolve(cmd::WINDOW_STATE, Ok(json!({ "saved": null, "old": old }))));
    w.cx.run_until_parked();
    assert_eq!(w.state().chats().count(), 7);
}

/// The app quits once the host says the list is written: until then it
/// waits, and a quit asked again asks nothing more. Where the host could
/// not write it, the window comes up and says so, with Retry and Quit
/// anyway; one the host would not even take is said at once.
#[gpui::test]
fn a_quit_waits_for_the_list_and_one_not_saved_keeps_the_window(cx: &mut TestAppContext) {
    let mut w = chats(cx, 2);
    w.cx.executor().advance_clock(workspace::GAP);
    w.ws(|w, cx| w.set_layout(3, cx));
    w.ws(|w, cx| w.set_layout(4, cx));
    w.host.defer(cmd::FLUSH_WINDOW_STATE);
    w.cx.update(|_, cx| cx.default_global::<crate::desktop::Shown>().hidden = true);
    w.cx.update(|_, cx| workspace::quit(cx));
    w.cx.run_until_parked();
    assert_eq!(w.saves().last().unwrap().layout(), 4, "what waited for the gap sent first");
    w.cx.update(|_, cx| workspace::quit(cx));
    w.cx.run_until_parked();
    assert_eq!(w.host.calls_of(cmd::FLUSH_WINDOW_STATE).len(), 1, "one quit under way");
    assert!(w.host.resolve(cmd::FLUSH_WINDOW_STATE, Err("cannot keep the chat list: disk full".into())));
    w.cx.run_until_parked();
    let notice = w.notices().into_iter().find(|n| n.key == "quit").unwrap();
    assert!(notice.text.contains("disk full") && notice.retry && notice.quit_anyway && !notice.ok, "{notice:?}");
    // The host is told, to let go of the way out it claimed for the quit.
    let refused = w.host.sends_of(crate::desktop::cmd::QUIT_REFUSED);
    assert_eq!(refused.len(), 1);
    assert!(refused[0]["why"].as_str().unwrap().contains("disk full"));
    w.cx.update(|_, cx| assert!(!cx.global::<crate::desktop::Shown>().hidden, "the window came up"));
    // Retry asks again; a failure again keeps the notice.
    w.ws(|w, cx| w.retry("quit", cx));
    assert_eq!(w.host.calls_of(cmd::FLUSH_WINDOW_STATE).len(), 2);
    assert!(w.host.resolve(cmd::FLUSH_WINDOW_STATE, Err("cannot keep the chat list: disk full".into())));
    w.cx.run_until_parked();
    assert!(w.notice_of("quit").is_some());
    // A change the host would not take, as it came nor as the quit sent
    // it: said at once, nothing asked.
    w.host.refuse(cmd::SAVE_WINDOW_STATE, 2);
    w.cx.executor().advance_clock(workspace::GAP);
    w.ws(|w, cx| w.set_layout(1, cx));
    w.ws(|w, cx| w.retry("quit", cx));
    assert_eq!(w.host.calls_of(cmd::FLUSH_WINDOW_STATE).len(), 2);
    assert!(w.notice_of("quit").unwrap().contains("did not take"));
}

/// A change that lands after the host finished its flush, before the window
/// handles the answer, is not covered by it: the app does not quit on that
/// answer, it flushes again, and quits on the one that names the newest state.
#[gpui::test]
fn a_change_made_during_the_flush_is_flushed_again_before_the_app_quits(cx: &mut TestAppContext) {
    let mut w = chats(cx, 2);
    w.cx.executor().advance_clock(workspace::GAP);
    w.ws(|w, cx| w.set_layout(3, cx));
    w.host.defer(cmd::FLUSH_WINDOW_STATE);
    w.cx.update(|_, cx| workspace::quit(cx));
    w.cx.run_until_parked();
    let sent = |w: &W| w.host.sends_of(cmd::SAVE_WINDOW_STATE).last().unwrap()["seq"].as_u64().unwrap();
    let first = sent(&w);
    // The host's flush wrote `first`; the person changes the layout (it
    // waits for the gap) before the answer is handled.
    w.ws(|w, cx| w.set_layout(4, cx));
    assert_eq!(sent(&w), first, "waiting for the gap");
    assert!(w.host.resolve(cmd::FLUSH_WINDOW_STATE, Ok(json!({ "seq": first }))));
    w.cx.run_until_parked();
    let second = sent(&w);
    assert!(second > first, "the change was sent when the answer came");
    assert_eq!(w.host.calls_of(cmd::FLUSH_WINDOW_STATE).len(), 2, "asked again");
    assert!(w.notice_of("quit").is_none());
    // Answered for the newest: the quit goes through.
    assert!(w.host.resolve(cmd::FLUSH_WINDOW_STATE, Ok(json!({ "seq": second }))));
    w.cx.run_until_parked();
    assert_eq!(w.host.calls_of(cmd::FLUSH_WINDOW_STATE).len(), 2);
    assert!(w.models.workspace.read_with(w.cx, |w, _| w.quitting()), "quitting, not stopped by a notice");
    assert!(w.notice_of("quit").is_none());
}

/// An answer that says nothing of what it wrote is no acknowledgment, and
/// changes that never stop are not flushed for ever: the window stays and says so.
#[gpui::test]
fn a_flush_that_does_not_cover_the_latest_state_never_quits(cx: &mut TestAppContext) {
    let mut w = chats(cx, 2);
    w.cx.executor().advance_clock(workspace::GAP);
    w.host.answer(cmd::FLUSH_WINDOW_STATE, json!(null));
    w.host.defer(cmd::FLUSH_WINDOW_STATE);
    w.ws(|w, cx| w.set_layout(3, cx));
    w.cx.update(|_, cx| workspace::quit(cx));
    w.cx.run_until_parked();
    assert!(w.host.resolve(cmd::FLUSH_WINDOW_STATE, Ok(json!(null))));
    w.cx.run_until_parked();
    assert!(w.notice_of("quit").unwrap().contains("names no state"));
    // The host keeps answering for an old state while the list changes.
    w.ws(|w, cx| w.retry("quit", cx));
    for round in 0..10 {
        let layout = if round % 2 == 0 { 4 } else { 1 };
        w.cx.executor().advance_clock(workspace::GAP);
        w.ws(|w, cx| w.set_layout(layout, cx));
        if !w.host.resolve(cmd::FLUSH_WINDOW_STATE, Ok(json!({ "seq": 0 }))) {
            break;
        }
        w.cx.run_until_parked();
    }
    assert!(w.notice_of("quit").unwrap().contains("kept changing"));
    assert!(!w.models.workspace.read_with(w.cx, |w, _| w.quitting()));
}

/// The core is told every chat's name and board (`chat_names`, the page's
/// bridge) once the list has come, when one changes and for a new core;
/// a look that changes no name tells it nothing.
#[gpui::test]
fn the_core_is_told_the_chats_names_and_boards(cx: &mut TestAppContext) {
    let mut w = chats(cx, 2);
    let told = |w: &W| w.host.calls_of(cmd::CHAT_NAMES);
    let last = told(&w).last().cloned().expect("told once the list came");
    let state = w.state();
    assert_eq!(last["names"].as_object().unwrap().len(), state.chats().count());
    for c in state.chats() {
        assert_eq!(last["names"][&c.id], json!(c.name), "{}", c.id);
        assert_eq!(last["groups"][&c.id], json!(c.board), "{}", c.id);
    }
    let n = told(&w).len();
    w.tick();
    assert_eq!(told(&w).len(), n, "nothing new, nothing told");
    w.ws(|w, cx| w.rename("c-parser", "Parser, renamed", cx));
    assert_eq!(told(&w).len(), n + 1);
    assert_eq!(told(&w).last().unwrap()["names"]["c-parser"], json!("Parser, renamed"));
    // A new core knows nothing the old one was told.
    w.core(replaced(5));
    assert_eq!(told(&w).len(), n + 2);
    // One the core did not take is told again at the next change.
    w.host.fail(cmd::CHAT_NAMES, Some("the core went away"));
    w.ws(|w, cx| w.rename("c-parser", "Parser again", cx));
    w.host.fail(cmd::CHAT_NAMES, None);
    w.ws(|w, cx| w.rename("c-notes", "Notes, renamed", cx));
    assert_eq!(told(&w).len(), n + 4);
}

/// With no chat to look at, no timer runs at all; a chat added is looked at
/// from then on.
#[gpui::test]
fn no_timer_runs_while_there_is_no_chat_to_look_at(cx: &mut TestAppContext) {
    let mut w = open(cx, demo::kept(&WindowState::default()));
    let armed = |w: &mut W| w.models.workspace.read_with(w.cx, |w, _| w.look_armed());
    let rounds = |w: &mut W| w.models.workspace.read_with(w.cx, |w, _| w.rounds);
    assert!(!armed(&mut w));
    w.cx.executor().advance_clock(Duration::from_secs(60));
    w.cx.run_until_parked();
    assert_eq!(rounds(&mut w), 0);
    let mut chat = demo::chats_state(2, true).find("c-parser").unwrap().clone();
    chat.id = "c-new".into();
    w.ws(|w, cx| w.add_chat("C:\\work\\parser", chat, true, cx));
    assert!(armed(&mut w), "the chat is looked at from now on");
    w.tick();
    assert_eq!(rounds(&mut w), 1);
    w.ws(|w, cx| w.remove_chat("c-new", cx));
    assert!(!armed(&mut w), "none left to look at: the timer armed for it goes with it");
    w.tick();
    assert_eq!(rounds(&mut w), 1);
}

/// Hidden to the tray, the sessions are looked at as before for five
/// minutes, then once a minute, as the page's throttled timers did; shown
/// again, at once.
#[gpui::test]
fn hidden_to_the_tray_the_looks_slow_down_and_come_at_once_when_shown(cx: &mut TestAppContext) {
    let mut w = chats(cx, 2);
    let rounds = |w: &mut W| w.models.workspace.read_with(w.cx, |w, _| w.rounds);
    assert!(w.mailbox.request(crate::desktop::Request::Hide));
    w.cx.run_until_parked();
    let r = rounds(&mut w);
    w.tick();
    assert_eq!(rounds(&mut w), r + 1, "as before at first");
    w.cx.executor().advance_clock(workspace::HIDDEN_AFTER);
    w.cx.run_until_parked();
    let r = rounds(&mut w);
    w.cx.executor().advance_clock(workspace::HIDDEN_EVERY / 2);
    w.cx.run_until_parked();
    assert_eq!(rounds(&mut w), r, "once a minute after five");
    w.cx.executor().advance_clock(workspace::HIDDEN_EVERY / 2);
    w.cx.run_until_parked();
    assert_eq!(rounds(&mut w), r + 1);
    assert!(w.mailbox.request(crate::desktop::Request::Show));
    w.cx.run_until_parked();
    assert_eq!(rounds(&mut w), r + 2, "shown, looked at at once");
    w.tick();
    assert_eq!(rounds(&mut w), r + 3, "and every four seconds again");
}

/// The models ask the host nothing until the window has opened (`begin`):
/// a window that cannot open leaves nothing begun, no import, no update
/// checked.
#[gpui::test]
fn nothing_is_asked_of_the_host_before_the_window_opens(cx: &mut TestAppContext) {
    let host = FakeHost::new();
    let (_mailbox, inbox) = mailbox();
    let models = cx.update(|cx| crate::prepare(host.clone(), inbox, ThemePref::Light, cx));
    cx.run_until_parked();
    assert!(host.calls().is_empty(), "{:?}", host.calls());
    assert!(host.sends().is_empty(), "{:?}", host.sends());
    cx.update(|cx| crate::begin(&models, cx));
    cx.run_until_parked();
    assert_eq!(host.calls_of(cmd::WINDOW_STATE).len(), 1);
    assert_eq!(host.calls_of(crate::desktop::cmd::UPDATE_CHECK).len(), 1);
}

/// The accounts are asked for again after a failure, ever later, until a
/// list comes: the sessions are looked at again once it does.
#[gpui::test]
fn accounts_that_failed_to_load_are_asked_for_again(cx: &mut TestAppContext) {
    let mut w = chats(cx, 2);
    w.host.defer(cmd::LIST_ACCOUNTS);
    let asked = |w: &W| w.host.calls_of(cmd::LIST_ACCOUNTS).len();
    let before = asked(&w);
    w.ws(|w, cx| w.reload_accounts(cx));
    w.ws(|w, cx| w.reload_accounts(cx));
    assert_eq!(asked(&w), before + 1, "one ask at a time");
    assert!(w.host.resolve(cmd::LIST_ACCOUNTS, Err("the accounts' folder is not reachable".into())));
    w.cx.run_until_parked();
    w.cx.executor().advance_clock(workspace::ACCOUNTS_RETRY_FIRST);
    w.cx.run_until_parked();
    assert_eq!(asked(&w), before + 2, "asked again");
    assert!(w.host.resolve(cmd::LIST_ACCOUNTS, Err("still not".into())));
    w.cx.run_until_parked();
    w.cx.executor().advance_clock(workspace::ACCOUNTS_RETRY_FIRST);
    w.cx.run_until_parked();
    assert_eq!(asked(&w), before + 2, "the wait doubled");
    w.cx.executor().advance_clock(workspace::ACCOUNTS_RETRY_FIRST);
    w.cx.run_until_parked();
    assert_eq!(asked(&w), before + 3);
    let rounds = w.models.workspace.read_with(w.cx, |w, _| w.rounds);
    assert!(w.host.resolve(cmd::LIST_ACCOUNTS, Ok(demo::accounts())));
    w.cx.run_until_parked();
    assert!(w.models.workspace.read_with(w.cx, |w, _| w.rounds) > rounds, "the sessions looked at once it came");
    w.cx.executor().advance_clock(workspace::ACCOUNTS_RETRY_MAX);
    w.cx.run_until_parked();
    assert_eq!(asked(&w), before + 3, "nothing more asked once it came");
}

/// A file that is there and could not be read is no list: nothing is
/// imported, nothing written over it, whatever is done meanwhile; Retry
/// reads it.
#[gpui::test]
fn a_list_that_could_not_be_read_is_never_written_over(cx: &mut TestAppContext) {
    let mut w = open(cx, json!({ "readError": "chats.json: sharing violation (os error 32)" }));
    assert!(w.notice_of("read").unwrap().contains("sharing violation"));
    assert!(!w.models.workspace.read_with(w.cx, |w, _| w.loaded_yet()));
    w.ws(|w, cx| w.toggle_sidebar(cx));
    w.cx.executor().advance_clock(workspace::GAP * 4);
    w.cx.run_until_parked();
    assert!(w.saves().is_empty(), "nothing written over the list");
    w.host.answer(cmd::WINDOW_STATE, demo::kept(&demo::chats_state(2, true)));
    w.ws(|w, cx| w.retry("read", cx));
    assert_eq!(w.state().chats().count(), 7);
    assert_eq!(w.notice_of("read"), None);
}

/// A list that did not all read says what was left out and where the file
/// is kept; one put aside says where.
#[gpui::test]
fn a_list_read_in_part_or_put_aside_says_so(cx: &mut TestAppContext) {
    let mut v: Value = serde_json::from_str(&demo::chats_state(2, true).to_text()).unwrap();
    v["version"] = 2.into();
    v["folders"][0]["chats"][0]["provider"] = "gemini".into();
    let mut w = open(cx, json!({ "saved": v, "kept": "C:\\data\\window\\chats.kept-1-ab.json" }));
    assert_eq!(w.state().chats().count(), 6);
    let kept = w.notice_of("kept").unwrap();
    assert!(kept.contains("version 2") && kept.contains("1 chat ") && kept.contains("chats.kept-1-ab.json"), "{kept}");
    w.ws(|w, cx| w.dismiss("kept", cx));
    assert!(w.notices().is_empty());
}

#[gpui::test]
fn a_list_put_aside_starts_empty_and_says_where(cx: &mut TestAppContext) {
    let mut w = open(cx, json!({ "saved": null, "aside": "C:\\data\\window\\chats.unreadable-1-ab.json" }));
    assert!(w.notice_of("aside").unwrap().contains("chats.unreadable-1-ab.json"));
    assert_eq!(w.saves().len(), 1, "the empty list written in its place");
    assert!(w.host.calls_of(cmd::WINDOW_STATE).len() == 1);
}

/// A save the host failed is said until one as new is written; one it
/// could not take is sent again.
#[gpui::test]
fn a_failed_save_is_said_and_a_refused_one_sent_again(cx: &mut TestAppContext) {
    let mut w = chats(cx, 2);
    w.cx.executor().advance_clock(workspace::GAP);
    w.ws(|w, cx| w.set_layout(3, cx));
    w.cx.executor().advance_clock(workspace::GAP);
    w.cx.run_until_parked();
    let seq = w.host.sends_of(cmd::SAVE_WINDOW_STATE).last().unwrap()["seq"].as_u64().unwrap();
    w.event(event::STATE_SAVED, json!({ "seq": seq, "error": "disk full" }));
    assert!(w.notice_of("save").unwrap().contains("disk full"));
    assert!(!w.models.workspace.read_with(w.cx, |w, _| w.saved_all()));
    w.event(event::STATE_SAVED, json!({ "seq": seq, "error": null }));
    assert_eq!(w.notice_of("save"), None);
    assert!(w.models.workspace.read_with(w.cx, |w, _| w.saved_all()));
    // The host would not take it: sent again shortly, unasked.
    w.host.refuse(cmd::SAVE_WINDOW_STATE, 1);
    let before = w.saves().len();
    w.cx.executor().advance_clock(workspace::GAP);
    w.ws(|w, cx| w.set_layout(4, cx));
    assert_eq!(w.saves().len(), before);
    w.cx.executor().advance_clock(workspace::SEND_RETRY);
    w.cx.run_until_parked();
    assert_eq!(w.saves().len(), before + 1);
    assert_eq!(w.saves().last().unwrap().layout(), 4);
}

#[gpui::test]
fn a_machine_0_19_never_ran_on_starts_empty_and_quiet(cx: &mut TestAppContext) {
    let mut w = open(cx, json!({ "saved": null }));
    assert_eq!(w.state().chats().count(), 0);
    assert_eq!(w.models.workspace.read_with(w.cx, |w, _| w.notice()), None);
}

#[gpui::test]
fn a_kept_state_is_read_and_nothing_is_imported_or_written(cx: &mut TestAppContext) {
    // No core: no session's news changes anything either.
    let starting = serde_json::to_value(CoreState::default()).unwrap();
    let mut w = open_with(cx, demo::kept(&demo::chats_state(3, true)), starting, json!([]));
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
    let looks = json!([
        { "id": "c-parser", "meta": { "status": "waiting", "context": 0.5, "draft": true, "held": 0, "title": "Parser fix" } },
        { "id": "c-notes", "meta": { "status": "busy", "context": 0.2, "firstPrompt": "write the notes", "sessionId": "sess-notes" } },
    ]);
    // The first round as the accounts come, then one every four seconds.
    let mut w = open_with(cx, demo::kept(&demo::chats_state(2, true)), up(), looks);
    assert_eq!(w.models.workspace.read_with(w.cx, |w, _| w.rounds), 1);
    w.cx.executor().advance_clock(workspace::EVERY);
    w.cx.run_until_parked();
    assert_eq!(w.models.workspace.read_with(w.cx, |w, _| w.rounds), 2);
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
    // It rests, then waits again: one notification, with its chat, and
    // Settings' sound off.
    w.ws(|w, cx| w.take_prefs(Prefs { sound: false, ..Prefs::default() }, cx));
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
    assert!(!notices[0].sound, "Settings has the sound off");
    assert_eq!(w.host.calls_of(cmd::SAVED_TITLE).iter().filter(|a| a["sessionId"] == "0d6f1c2e-0000-4000-8000-000000000002").count(), 1);
    // Every chat with an account is asked about, by its account's path.
    let asked = w.host.calls_of(cmd::SESSIONS_META).last().cloned().unwrap();
    assert!(
        asked["chats"].as_array().unwrap().iter().any(|c| c["id"] == "c-codex" && c["accountPath"] == "C:\\accounts\\openai\\personal")
    );
}

/// A live session its CLI has left nothing on disk for answers its record's
/// status alone: the row says it, and its name is looked for in its saved
/// title once, as a resting chat's is.
#[gpui::test]
fn a_session_known_only_by_its_record_keeps_its_saved_title(cx: &mut TestAppContext) {
    let looks = json!([{ "id": "c-codex", "meta": { "status": "busy", "draft": false, "held": 0 } }]);
    let mut w = open_with(cx, demo::kept(&demo::chats_state(2, true)), up(), looks);
    w.tick();
    let chat = w.state().find("c-codex").unwrap().clone();
    assert_eq!((chat.status, chat.name.as_str()), (Status::Working, "Saved title"));
    let session = chat.session_id.clone().expect("the fixture's chat has a session");
    let host = w.host.clone();
    let asked = || host.calls_of(cmd::SAVED_TITLE).iter().filter(|a| a["sessionId"] == session.as_str()).count();
    assert_eq!(asked(), 1);
    w.tick();
    assert_eq!(asked(), 1, "once a chat");
}

/// A look asked before a session exited, or of a core since replaced, puts
/// nothing back; a look that fails says nothing; a wait is told once.
#[gpui::test]
fn stale_or_failed_looks_change_nothing(cx: &mut TestAppContext) {
    let mut w = chats(cx, 1);
    w.host.answer(cmd::SESSIONS_META, json!([{ "id": "c-parser", "meta": { "status": "busy", "sessionId": "s-1" } }]));
    w.tick();
    assert_eq!(w.state().find("c-parser").unwrap().status, Status::Working);
    // Asked; then the session exits; then the answer, from before it.
    w.host.defer(cmd::SESSIONS_META);
    w.tick();
    w.host.desktop.notices.lock().unwrap().clear();
    let before = w.state().find("c-parser").unwrap().clone();
    w.ws(|w, cx| w.on_exit("c-parser", cx));
    let late = json!([{ "id": "c-parser", "meta": { "status": "waiting", "draft": true, "sessionId": "s-old" } }]);
    assert!(w.host.resolve(cmd::SESSIONS_META, Ok(late.clone())));
    w.cx.run_until_parked();
    let after = w.state().find("c-parser").unwrap().clone();
    assert_eq!(after.status, Status::Resting);
    assert_eq!(after.session_id, before.session_id, "the old session is not put back");
    assert!(!w.models.workspace.read_with(w.cx, |w, _| w.live("c-parser").draft));
    assert!(w.host.desktop.notices.lock().unwrap().is_empty());
    // Asked; then the core is replaced; then the answer: dropped.
    w.tick();
    w.core(replaced(5));
    assert!(w.host.resolve(cmd::SESSIONS_META, Ok(late.clone())));
    w.cx.run_until_parked();
    assert_eq!(w.state().find("c-parser").unwrap().status, Status::Resting);
    // A look that fails is not "nothing runs".
    w.ws(|w, cx| {
        w.state.find_mut("c-parser").unwrap().status = Status::Working;
        cx.notify();
    });
    w.tick();
    assert!(w.host.resolve(cmd::SESSIONS_META, Err("the core went away".into())));
    w.cx.run_until_parked();
    assert_eq!(w.state().find("c-parser").unwrap().status, Status::Working);
    // One that never answers is given up, and the next tick asks again.
    w.tick();
    let rounds = w.models.workspace.read_with(w.cx, |w, _| w.rounds);
    w.cx.executor().advance_clock(workspace::POLL_TIMEOUT + workspace::EVERY);
    w.cx.run_until_parked();
    assert!(w.models.workspace.read_with(w.cx, |w, _| w.rounds) > rounds, "not stuck behind the hung one");
}

#[gpui::test]
fn a_wait_is_told_once_and_rows_wait_for_the_core(cx: &mut TestAppContext) {
    let mut w = chats(cx, 1);
    w.host.answer(cmd::SESSIONS_META, json!([{ "id": "c-parser", "meta": { "status": "busy" } }]));
    w.tick();
    w.host.answer(cmd::SESSIONS_META, json!([{ "id": "c-parser", "meta": { "status": "waiting" } }]));
    w.tick();
    assert_eq!(w.host.desktop.notices.lock().unwrap().len(), 1);
    // The core goes: every row says so, and Ctrl+Shift+W finds no wait.
    w.core(down(5));
    let shown = w.models.workspace.read_with(w.cx, |w, cx| w.shown_status(w.state.find("c-parser").unwrap(), cx));
    assert_eq!(shown, Status::NoCore);
    w.ws(|w, cx| w.set_board(1, cx));
    w.keys("ctrl-shift-w");
    assert_eq!(w.state().board, 1, "no wait known while the core is down");
    // Back, the same wait: not told again.
    w.core(replaced(6));
    w.tick();
    assert_eq!(w.host.desktop.notices.lock().unwrap().len(), 1);
    let shown = w.models.workspace.read_with(w.cx, |w, cx| w.shown_status(w.state.find("c-parser").unwrap(), cx));
    assert_eq!(shown, Status::Waiting);
}

/// A terminal made before the first look resumes the session the look
/// found when the core is replaced; the warm ones out of sight are let go,
/// not started again.
#[gpui::test]
fn a_restart_resumes_the_session_as_it_is_now_and_lets_hidden_ones_go(cx: &mut TestAppContext) {
    let mut w = chats(cx, 1);
    w.ws(|w, cx| {
        w.state.find_mut("c-parser").unwrap().session_id = None;
        cx.notify();
    });
    w.ws(|w, cx| w.show_chat("c-codex", cx));
    w.ws(|w, cx| w.show_chat("c-parser", cx));
    assert!(w.terminal_of("c-codex").is_some(), "warm");
    w.host.answer(cmd::SESSIONS_META, json!([{ "id": "c-parser", "meta": { "status": "busy", "sessionId": "s-now" } }]));
    w.tick();
    let codex_starts = w.host.calls_of(cmd::ENSURE_CODEX_SESSION).len();
    w.core(replaced(5));
    let last = w.host.calls_of(cmd::ENSURE_CLAUDE_SESSION).into_iter().rfind(|a| a["id"] == "c-parser").unwrap();
    assert_eq!(last["resume"], "s-now");
    assert_eq!(w.host.calls_of(cmd::ENSURE_CODEX_SESSION).len(), codex_starts, "the hidden one is not started");
    assert!(w.terminal_of("c-codex").is_none(), "and let go");
}

/// An exit of a spawn before the one the pane runs rests nothing.
#[gpui::test]
fn an_exit_of_an_older_spawn_rests_nothing(cx: &mut TestAppContext) {
    let mut w = chats(cx, 1);
    w.host.answer(cmd::SESSIONS_META, json!([{ "id": "c-parser", "meta": { "status": "busy" } }]));
    w.tick();
    w.event(event::PTY_EXIT, json!({ "id": "c-parser", "code": 0, "generation": 0 }));
    assert_eq!(w.state().find("c-parser").unwrap().status, Status::Working);
    w.event(event::PTY_EXIT, json!({ "id": "c-parser", "code": 0, "generation": 1 }));
    assert_eq!(w.state().find("c-parser").unwrap().status, Status::Resting);
}

/// A chat whose session ended, shown again, gets a new terminal, and the
/// keyboard goes there, not to the one let go of.
#[gpui::test]
fn a_chat_reopened_after_its_exit_has_the_keyboard(cx: &mut TestAppContext) {
    let mut w = chats(cx, 1);
    w.event(event::PTY_EXIT, json!({ "id": "c-parser", "code": 0, "generation": 1 }));
    w.ws(|w, cx| w.show_chat("c-codex", cx));
    let old = w.terminal_of("c-parser");
    w.ws(|w, cx| w.show_chat("c-parser", cx));
    assert_eq!(w.seats(), [s("c-parser")]);
    assert_ne!(w.terminal_of("c-parser"), old);
    assert_eq!(w.focused_terminal().as_deref(), Some("c-parser"));
}

/// Ctrl+Shift+W goes by the status each chat shows, the core's lifecycle
/// record where it has one, not the chat's own a round behind it: a chat
/// the record has back at work is passed over, one it has waiting gone to.
#[gpui::test]
fn the_next_waiting_chat_is_the_one_shown_waiting(cx: &mut TestAppContext) {
    let mut w = chats(cx, 1);
    w.ws(|w, _| {
        let ids: Vec<String> = w.state.chats().map(|c| c.id.clone()).collect();
        for id in ids {
            w.state.find_mut(&id).unwrap().status = Status::Resting;
        }
        w.state.find_mut("c-parser").unwrap().status = Status::Waiting;
    });
    let lives = std::collections::HashMap::from([("c-parser".to_owned(), Status::Working), ("c-notes".to_owned(), Status::Waiting)]);
    // In one go: the status bar hands the workspace the records again as it
    // changes, from a power state this window has not been told.
    w.ws(|w, cx| {
        w.set_lives(lives, cx);
        w.next_waiting(cx);
    });
    assert_eq!(w.state().active.as_deref(), Some("c-notes"));
}

/// Esc in a peek: the frame has the keyboard as it opens, and Esc closes
/// it at once, for a chat or a pane; once its terminal has the keyboard
/// (a key typed, a click), Esc is the CLI's and Shift+Esc closes.
#[gpui::test]
fn esc_closes_a_peek_until_its_terminal_has_the_keyboard(cx: &mut TestAppContext) {
    let mut w = chats(cx, 2);
    w.ws(|w, cx| w.show_chat("c-notes", cx));
    assert!(w.peek_frame_focused());
    w.keys("escape");
    assert_eq!(w.state().peek, None, "one press");
    // A pane held up: one press too.
    w.ws(|w, cx| w.show_chat("c-codex", cx));
    assert_eq!(w.state().peek, Some(Peek::Pane(1)));
    w.keys("escape");
    assert_eq!(w.state().peek, None);
    // A key typed in the frame goes to the terminal, which takes the keyboard.
    w.ws(|w, cx| w.show_chat("c-notes", cx));
    w.keys("x");
    assert_eq!(w.typed("c-notes"), ["x"]);
    assert_eq!(w.focused_terminal().as_deref(), Some("c-notes"));
    w.keys("escape");
    assert!(w.state().peek.is_some(), "Esc is the CLI's now");
    assert_eq!(w.typed("c-notes").last().map(String::as_str), Some("\u{1b}"));
    w.keys("shift-escape");
    assert_eq!(w.state().peek, None);
}

/// Ctrl+digit by the key pressed, whatever the layout names it: on French
/// AZERTY the digit row's first key is `&`, and `1` with Shift.
#[gpui::test]
fn ctrl_digits_go_by_the_key_pressed(cx: &mut TestAppContext) {
    let mut w = chats(cx, 2);
    // The virtual keys down as the keystroke comes.
    let down = std::rc::Rc::new(std::cell::RefCell::new(Vec::<u16>::new()));
    let reader = down.clone();
    let models = w.models.clone();
    // What French AZERTY calls the digit row's first key.
    let names = |vk: u16| {
        if vk == 0x31 {
            crate::keys::KeyNames { plain: vec!["&".to_owned(), "1".to_owned()], shifted: vec!["1".to_owned()] }
        } else {
            crate::keys::KeyNames::default()
        }
    };
    w.cx.update(|_, cx| crate::keys::watch_digit_row(models, move |vk| reader.borrow().contains(&vk), names, cx));
    // VK_1 down, named `&`: the keyboard into pane 1.
    w.ws(|w, cx| w.set_active_pane(1, cx));
    *down.borrow_mut() = vec![0x31];
    w.keys("ctrl-&");
    assert_eq!(w.state().active_pane, 0);
    // Shift and VK_1, named `1`: layout 1, not pane 1.
    *down.borrow_mut() = vec![0x10, 0x31];
    w.keys("ctrl-1");
    assert_eq!(w.state().layout(), 1);
    // 1 held, and Ctrl+/ typed: the terminal's, not pane 1's or layout 1's.
    w.ws(|w, cx| w.set_layout(2, cx));
    w.ws(|w, cx| w.set_active_pane(1, cx));
    *down.borrow_mut() = vec![0x31];
    w.keys("ctrl-/");
    assert_eq!((w.state().layout(), w.state().active_pane), (2, 1));
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

/// Alt+digit holds the chat in that pane up over the board from wherever
/// the keyboard is, a terminal included, and the same key puts it back; Esc
/// closes it as it closes a peek opened by a click. One that means nothing
/// (a pane past the layout) goes on to the terminal.
#[gpui::test]
fn alt_digits_toggle_the_peek_of_a_pane(cx: &mut TestAppContext) {
    let mut w = chats(cx, 2);
    w.focus("c-parser");
    w.keys("alt-2");
    assert_eq!(w.state().peek, Some(Peek::Pane(1)));
    assert!(w.peek_frame_focused(), "the frame, not the terminal, has the keyboard");
    w.keys("alt-2");
    assert_eq!(w.state().peek, None, "the same key puts it back");
    w.keys("alt-1");
    assert_eq!(w.state().peek, Some(Peek::Pane(0)));
    w.keys("alt-2");
    assert_eq!(w.state().peek, Some(Peek::Pane(1)), "another pane's key moves the peek");
    w.keys("escape");
    assert_eq!(w.state().peek, None);
    w.focus("c-parser");
    let before = w.typed("c-parser").len();
    w.keys("alt-3");
    assert_eq!(w.state().peek, None);
    assert_eq!(w.typed("c-parser")[before..], ["\u{1b}3"], "past the layout: the terminal's");
}

/// Ctrl+= and Ctrl+- step the window's scale, Ctrl+0 puts it back; they
/// work with a terminal focused and type nothing into it; the scale is
/// kept and applied at once: the chrome, GPUI's rem and the terminal's
/// cells, whose PTY is told its new size.
#[gpui::test]
fn the_scale_keys_step_the_window_scale_and_the_terminal_follows(cx: &mut TestAppContext) {
    let mut w = chats(cx, 2);
    w.focus("c-parser");
    let resized = |w: &W| w.host.sends_of(cmd::RESIZE_SESSION).iter().filter(|a| a["id"] == "c-parser").count();
    let cols = |w: &mut W| {
        let board = w.view.read_with(w.cx, |v, _| v.board.clone());
        w.cx.update(|_, cx| board.read(cx).terminal_of("c-parser").map(|t| t.read(cx).geom().cols)).unwrap()
    };
    let (typed, sized, before) = (w.typed("c-parser").len(), resized(&w), cols(&mut w));
    w.keys("ctrl-=");
    assert_eq!(w.state().ui_scale, 110);
    assert!((crate::scale::current() - 1.1).abs() < 1e-6);
    w.keys("ctrl-+ ctrl-= ctrl-=");
    assert_eq!(w.state().ui_scale, 175);
    assert!((crate::scale::current() - 1.75).abs() < 1e-6);
    assert_eq!(w.cx.update(|window, _| window.rem_size()), gpui::px(28.));
    assert!(cols(&mut w) < before, "bigger cells, fewer of them");
    assert!(resized(&w) > sized, "the PTY is told");
    w.keys("ctrl--");
    assert_eq!(w.state().ui_scale, 150);
    w.keys("ctrl-0");
    assert_eq!(w.state().ui_scale, 100);
    assert_eq!(crate::scale::current(), 1.);
    assert_eq!(cols(&mut w), before);
    assert_eq!(w.typed("c-parser").len(), typed, "nothing typed into the terminal");
    w.tick();
    assert_eq!(w.saves().last().map(|s| s.ui_scale), Some(100));
    w.ws(|w, cx| w.set_ui_scale(250, cx));
    w.keys("ctrl-=");
    assert_eq!(w.state().ui_scale, 250, "the top step stays");
    // The scale is the thread's: left as it was found.
    w.keys("ctrl-0");
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
    assert_eq!(placed, Some(crate::state::Placed::Pane(2)));
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
    let before = w.state().find("c-codex").unwrap().name.clone();
    w.cx.update(|window, cx| board.update(cx, |b, cx| b.start_rename("c-codex", window, cx)));
    w.cx.run_until_parked();
    w.keys("escape");
    assert!(board.read_with(w.cx, |b, _| b.renaming.is_none()));
    assert_eq!(w.state().find("c-codex").unwrap().name, before);
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
