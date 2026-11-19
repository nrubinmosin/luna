//! The panels over a fake host, on GPUI's test platform: each control asks
//! the host what the page's does, by the same command and arguments, and
//! shows what the host answers.

use super::{Open, Panel, cmd, demo};
use crate::accounts::AccountsPanel;
use crate::demo::FakeHost;
use crate::dto::{CoreState, LinkState, ThemePref};
use crate::host::{Mailbox, mailbox};
use crate::library::{AccountRef, Audience, Entry, LibraryPanel, Sel};
use crate::panels::Provider;
use crate::root::LunaWindow;
use crate::settings::{Page, SettingsPanel, UpdateState};
use gpui::{Entity, TestAppContext, VisualTestContext};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

struct Fixture<'a> {
    host: Arc<FakeHost>,
    mailbox: Mailbox,
    view: Entity<LunaWindow>,
    cx: &'a mut VisualTestContext,
}

fn open(cx: &mut TestAppContext) -> Fixture<'_> {
    let host = FakeHost::new();
    demo::answer_all(&host);
    let up = CoreState { state: LinkState::Up, core: Some(crate::demo::core()), error: None, revision: 1 };
    host.answer(crate::cmd::CORE_STATE, serde_json::to_value(up).unwrap());
    let (mailbox, inbox) = mailbox();
    let models = cx.update(|cx| crate::start(host.clone(), inbox, ThemePref::Light, cx));
    let (view, cx) = cx.add_window_view(move |window, cx| LunaWindow::new(models, window, cx));
    cx.run_until_parked();
    Fixture { host, mailbox, view, cx }
}

impl Fixture<'_> {
    fn show(&mut self, panel: Panel) -> Open {
        self.cx.update(|window, cx| super::open(panel, window, cx));
        self.cx.run_until_parked();
        self.open().expect("the panel is up")
    }

    fn open(&mut self) -> Option<Open> {
        self.view.read_with(self.cx, |v, cx| v.panels.read(cx).open.clone())
    }

    fn library(&mut self) -> Entity<LibraryPanel> {
        match self.show(Panel::Library) {
            Open::Library(v) => v,
            _ => unreachable!(),
        }
    }

    fn settings(&mut self) -> Entity<SettingsPanel> {
        match self.show(Panel::Settings) {
            Open::Settings(v) => v,
            _ => unreachable!(),
        }
    }

    fn accounts(&mut self) -> Entity<AccountsPanel> {
        match self.show(Panel::Accounts) {
            Open::Accounts(v) => v,
            _ => unreachable!(),
        }
    }

    fn calls(&self, cmd: &str) -> Vec<Value> {
        self.host.calls_of(cmd)
    }

    /// Esc, as the panel's key context takes it.
    fn escape(&mut self) {
        self.cx.dispatch_action(super::Close);
        self.cx.run_until_parked();
    }

    fn select(&mut self, lib: &Entity<LibraryPanel>, sel: Sel) {
        lib.update_in(self.cx, |v, window, cx| v.select(sel, window, cx));
        self.cx.run_until_parked();
    }

    /// Types `text` into the editor open, as keys would.
    fn type_in_editor(&mut self, lib: &Entity<LibraryPanel>, text: &str) {
        let state = lib.read_with(self.cx, |v, cx| v.editor.clone().unwrap().read(cx).state.clone());
        state.update_in(self.cx, |s, window, cx| s.focus(window, cx));
        self.cx.simulate_input(text);
        self.cx.run_until_parked();
    }
}

#[gpui::test]
fn a_panel_opens_over_the_window_and_esc_takes_it_down(cx: &mut TestAppContext) {
    let mut f = open(cx);
    assert!(f.open().is_none());
    f.show(Panel::Settings);
    assert!(matches!(f.open(), Some(Open::Settings(_))));
    // Another one takes its place.
    f.show(Panel::Accounts);
    assert!(matches!(f.open(), Some(Open::Accounts(_))));
    f.escape();
    assert!(f.open().is_none());
}

#[gpui::test]
fn the_library_reads_its_overview_and_shows_it(cx: &mut TestAppContext) {
    let mut f = open(cx);
    let lib = f.library();
    assert_eq!(f.calls(cmd::LIBRARY_OVERVIEW).len(), 1);
    let names = lib.read_with(f.cx, |v, cx| {
        let o = v.library.read(cx).overview.clone().unwrap();
        (o.instructions.len(), o.skills.len(), crate::library::shown_system(&o).count())
    });
    // The handoff's skill, which ADR-0030 cut, is not shown.
    assert_eq!(names, (3, 3, 2));
}

#[gpui::test]
fn a_new_skill_is_made_picked_and_opened_in_the_editor(cx: &mut TestAppContext) {
    let mut f = open(cx);
    let lib = f.library();
    let field = lib.read_with(f.cx, |v, _| v.new_skill_field());
    field.update_in(f.cx, |s, window, cx| s.set_value("triage", window, cx));
    lib.update_in(f.cx, |v, window, cx| v.create(true, window, cx));
    f.cx.run_until_parked();
    assert_eq!(f.calls(cmd::LIBRARY_NEW_SKILL), vec![json!({ "name": "triage" })]);
    assert_eq!(lib.read_with(f.cx, |v, _| v.selected.clone()), Some(Sel::Skill("triage".into())));
    assert_eq!(f.calls(cmd::LIBRARY_READ).last(), Some(&json!({ "entry": { "skill": "triage" } })));
    assert_eq!(field.read_with(f.cx, |s, _| s.value().to_string()), "", "the field is cleared");
    // New instructions likewise.
    let field = lib.read_with(f.cx, |v, _| v.new_instructions_field());
    field.update_in(f.cx, |s, window, cx| s.set_value("short", window, cx));
    lib.update_in(f.cx, |v, window, cx| v.create(false, window, cx));
    f.cx.run_until_parked();
    assert_eq!(f.calls(cmd::LIBRARY_NEW_INSTRUCTIONS), vec![json!({ "name": "short" })]);
    assert_eq!(lib.read_with(f.cx, |v, _| v.selected.clone()), Some(Sel::Instructions("short".into())));
}

#[gpui::test]
fn the_editor_loads_tracks_its_changes_checks_and_saves(cx: &mut TestAppContext) {
    let mut f = open(cx);
    let lib = f.library();
    f.select(&lib, Sel::Skill("review".into()));
    let doc = lib.read_with(f.cx, |v, _| v.editor.clone().unwrap());
    assert_eq!(doc.read_with(f.cx, |d, cx| (d.text(cx), d.dirty(cx))), (demo::SKILL_TEXT.to_owned(), false));
    let checks = f.calls(cmd::LIBRARY_CHECK_SKILL).len();
    assert_eq!(checks, 1, "checked as it is read");

    f.type_in_editor(&lib, "More. ");
    assert!(doc.read_with(f.cx, |d, cx| d.dirty(cx)));
    // Checked a moment after the last key, not at every one.
    assert_eq!(f.calls(cmd::LIBRARY_CHECK_SKILL).len(), checks);
    f.cx.executor().advance_clock(Duration::from_millis(300));
    f.cx.run_until_parked();
    assert_eq!(f.calls(cmd::LIBRARY_CHECK_SKILL).len(), checks + 1);

    // A problem found is said, the text saved all the same.
    f.host.answer(cmd::LIBRARY_CHECK_SKILL, json!("The front matter has no description"));
    f.type_in_editor(&lib, "x");
    f.cx.executor().advance_clock(Duration::from_millis(300));
    f.cx.run_until_parked();
    assert_eq!(doc.read_with(f.cx, |d, _| d.problem.clone()).as_deref(), Some("The front matter has no description"));

    f.cx.dispatch_action(super::Save);
    f.cx.run_until_parked();
    let text = doc.read_with(f.cx, |d, cx| d.text(cx));
    assert_eq!(f.calls(cmd::LIBRARY_WRITE), vec![json!({ "entry": { "skill": "review" }, "text": text })]);
    assert!(doc.read_with(f.cx, |d, cx| d.saved && !d.dirty(cx)));
    // Nothing to save: nothing is sent.
    f.cx.dispatch_action(super::Save);
    f.cx.run_until_parked();
    assert_eq!(f.calls(cmd::LIBRARY_WRITE).len(), 1);

    // A failed save keeps the text unsaved and says why.
    f.type_in_editor(&lib, "y");
    f.host.fail(cmd::LIBRARY_WRITE, Some("disk full"));
    f.cx.dispatch_action(super::Save);
    f.cx.run_until_parked();
    assert!(doc.read_with(f.cx, |d, cx| d.dirty(cx) && d.error.as_deref() == Some("disk full")));
}

#[gpui::test]
fn unsaved_text_asks_before_it_goes(cx: &mut TestAppContext) {
    let mut f = open(cx);
    let lib = f.library();
    f.select(&lib, Sel::Instructions("main".into()));
    f.type_in_editor(&lib, "edit ");
    // Picking another entry asks; Esc takes the question down, not the text.
    f.select(&lib, Sel::Skill("review".into()));
    assert_eq!(lib.read_with(f.cx, |v, _| v.selected.clone()), Some(Sel::Instructions("main".into())));
    assert!(lib.read_with(f.cx, |v, _| v.asking()));
    f.escape();
    assert!(!lib.read_with(f.cx, |v, _| v.asking()));
    assert!(f.open().is_some());
    // Esc on the window with unsaved text asks too; yes closes it.
    f.escape();
    assert!(lib.read_with(f.cx, |v, _| v.asking()));
    lib.update_in(f.cx, |v, window, cx| v.answer_yes(window, cx));
    f.cx.run_until_parked();
    assert!(f.open().is_none());
    assert!(f.calls(cmd::LIBRARY_WRITE).is_empty());
}

#[gpui::test]
fn rename_and_delete_go_to_the_host_by_kind(cx: &mut TestAppContext) {
    let mut f = open(cx);
    let lib = f.library();
    let library = lib.read_with(f.cx, |v, _| v.library.clone());
    library.update(f.cx, |l, cx| {
        l.rename(&Entry::Instructions("terse".into()), " short ", cx).detach();
        l.rename(&Entry::Skill("review".into()), "audit", cx).detach();
        // Unchanged or blank: nothing to send.
        l.rename(&Entry::Skill("review".into()), "review", cx).detach();
        l.rename(&Entry::Skill("review".into()), "  ", cx).detach();
    });
    f.cx.run_until_parked();
    assert_eq!(f.calls(cmd::LIBRARY_RENAME_INSTRUCTIONS), vec![json!({ "from": "terse", "to": "short" })]);
    assert_eq!(f.calls(cmd::LIBRARY_RENAME_SKILL), vec![json!({ "from": "review", "to": "audit" })]);

    // Delete asks first; yes takes it out and the editor with it.
    f.select(&lib, Sel::Skill("draft".into()));
    lib.update(f.cx, |v, cx| v.ask_delete(cx));
    f.cx.run_until_parked();
    assert!(f.calls(cmd::LIBRARY_DELETE_SKILL).is_empty());
    lib.update_in(f.cx, |v, window, cx| v.answer_yes(window, cx));
    f.cx.run_until_parked();
    assert_eq!(f.calls(cmd::LIBRARY_DELETE_SKILL), vec![json!({ "name": "draft" })]);
    assert!(lib.read_with(f.cx, |v, _| v.selected.is_none() && v.editor.is_none()));
    library.update(f.cx, |l, cx| l.delete(&Entry::Instructions("terse".into()), cx).detach());
    f.cx.run_until_parked();
    assert_eq!(f.calls(cmd::LIBRARY_DELETE_INSTRUCTIONS), vec![json!({ "name": "terse" })]);
}

#[gpui::test]
fn reach_audience_and_each_accounts_instructions_go_to_the_host(cx: &mut TestAppContext) {
    let mut f = open(cx);
    let lib = f.library();
    let library = lib.read_with(f.cx, |v, _| v.library.clone());
    let work = AccountRef { provider: Provider::Claude, name: "work".into() };
    library.update(f.cx, |l, cx| {
        l.set_reach(&Entry::Skill("release".into()), vec!["claude".into(), "claude/work".into(), "codex/main".into()], cx).detach();
        l.set_audience(&Entry::Skill("review".into()), Audience { chat: true, lead: true, helper: false }, cx).detach();
        // The radio: main for work, which terse names now.
        l.give(Some("main"), &work, cx).detach();
        l.give(None, &work, cx).detach();
    });
    f.cx.run_until_parked();
    assert_eq!(f.calls(cmd::LIBRARY_SET_ACCOUNTS), vec![json!({ "skill": "release", "accounts": ["claude", "codex/main"] })]);
    assert_eq!(f.calls(cmd::LIBRARY_SET_AUDIENCE), vec![json!({ "entry": { "skill": "review" }, "audience": ["chat", "lead"] })]);
    assert_eq!(
        f.calls(cmd::LIBRARY_SET_INSTRUCTIONS),
        vec![json!({ "name": "main", "accounts": ["all", "claude/work"] }), json!({ "name": null, "accounts": ["claude/work"] })]
    );
}

#[gpui::test]
fn a_folder_is_added_through_the_picker_and_says_its_own(cx: &mut TestAppContext) {
    let mut f = open(cx);
    let lib = f.library();
    *f.host.desktop.picked.lock().unwrap() = Some("D:\\work\\repo".into());
    lib.update_in(f.cx, |v, window, cx| v.add_folder(window, cx));
    f.cx.run_until_parked();
    assert_eq!(f.host.desktop.picks.lock().unwrap().len(), 1);
    assert_eq!(f.calls(cmd::LIBRARY_SET_FOLDER), vec![json!({ "path": "D:\\work\\repo", "listed": true })]);
    assert_eq!(lib.read_with(f.cx, |v, _| v.selected.clone()), Some(Sel::Folder("D:\\work\\repo".into())));
    // Cancelled: nothing.
    *f.host.desktop.picked.lock().unwrap() = None;
    lib.update_in(f.cx, |v, window, cx| v.add_folder(window, cx));
    f.cx.run_until_parked();
    assert_eq!(f.calls(cmd::LIBRARY_SET_FOLDER).len(), 1);

    let library = lib.read_with(f.cx, |v, _| v.library.clone());
    let work = AccountRef { provider: Provider::Claude, name: "work".into() };
    library.update(f.cx, |l, cx| {
        l.set_folder_skill("E:\\Projects\\luna", "review", Some(vec!["claude/work".into()]), cx).detach();
        l.set_folder_skill("E:\\Projects\\luna", "release", None, cx).detach();
        l.set_folder_instructions("E:\\Projects\\luna", &work, None, cx).detach();
        l.set_folder_instructions("E:\\Projects\\luna", &work, Some(None), cx).detach();
        l.set_folder_instructions("E:\\Projects\\luna", &work, Some(Some("terse".into())), cx).detach();
        l.set_folder("E:\\Projects\\luna", false, cx).detach();
    });
    f.cx.run_until_parked();
    assert_eq!(
        f.calls(cmd::LIBRARY_SET_FOLDER_SKILL),
        vec![
            json!({ "path": "E:\\Projects\\luna", "skill": "review", "accounts": ["claude/work"] }),
            json!({ "path": "E:\\Projects\\luna", "skill": "release", "accounts": null }),
        ]
    );
    assert_eq!(
        f.calls(cmd::LIBRARY_SET_FOLDER_INSTRUCTIONS),
        vec![
            json!({ "path": "E:\\Projects\\luna", "account": "claude/work", "inherit": true, "name": null }),
            json!({ "path": "E:\\Projects\\luna", "account": "claude/work", "inherit": false, "name": null }),
            json!({ "path": "E:\\Projects\\luna", "account": "claude/work", "inherit": false, "name": "terse" }),
        ]
    );
    assert_eq!(f.calls(cmd::LIBRARY_SET_FOLDER).last(), Some(&json!({ "path": "E:\\Projects\\luna", "listed": false })));
}

#[gpui::test]
fn a_system_text_opens_compares_resets_and_adopts(cx: &mut TestAppContext) {
    let mut f = open(cx);
    let lib = f.library();
    f.select(&lib, Sel::System("role-lead".into()));
    assert_eq!(f.calls(cmd::LIBRARY_READ).last(), Some(&json!({ "entry": { "system": "role-lead" } })));
    lib.update(f.cx, |v, cx| v.compare_with_luna("role-lead".into(), cx));
    f.cx.run_until_parked();
    assert_eq!(f.calls(cmd::LIBRARY_SYSTEM_COMPARE), vec![json!({ "name": "role-lead" })]);
    assert!(lib.read_with(f.cx, |v, _| v.comparing() == Some("role-lead")));
    lib.update(f.cx, |v, cx| v.ask_reset("role-lead".into(), cx));
    lib.update_in(f.cx, |v, window, cx| v.answer_yes(window, cx));
    f.cx.run_until_parked();
    assert_eq!(f.calls(cmd::LIBRARY_SYSTEM_RESET), vec![json!({ "name": "role-lead" })]);
    // The editor opens again, on Luna's own.
    assert_eq!(f.calls(cmd::LIBRARY_READ).last(), Some(&json!({ "entry": { "system": "role-lead" } })));
    let library = lib.read_with(f.cx, |v, _| v.library.clone());
    library.update(f.cx, |l, cx| l.system_adopt("role-chat", cx).detach());
    f.cx.run_until_parked();
    assert_eq!(f.calls(cmd::LIBRARY_SYSTEM_ADOPT), vec![json!({ "skill": "role-chat" })]);
}

#[gpui::test]
fn a_sync_says_where_it_is_in_words(cx: &mut TestAppContext) {
    let mut f = open(cx);
    let lib = f.library();
    let library = lib.read_with(f.cx, |v, _| v.library.clone());
    library.update(f.cx, |l, cx| l.sync_now(cx));
    assert_eq!(library.read_with(f.cx, |l, _| l.sync.clone()), crate::library::SyncState::Syncing);
    f.cx.run_until_parked();
    assert_eq!(library.read_with(f.cx, |l, _| l.sync.clone()), crate::library::SyncState::Synced);
    f.host.fail(cmd::LIBRARY_SYNC, Some("no lock"));
    library.update(f.cx, |l, cx| l.sync_now(cx));
    f.cx.run_until_parked();
    assert_eq!(library.read_with(f.cx, |l, _| l.sync.clone()), crate::library::SyncState::Failed("no lock".into()));
}

/// An editor left alone schedules nothing: its caret is steady and nothing
/// polls.
#[gpui::test]
fn the_library_editor_is_still_while_left_alone(cx: &mut TestAppContext) {
    let mut f = open(cx);
    let lib = f.library();
    f.select(&lib, Sel::Skill("review".into()));
    let state = lib.read_with(f.cx, |v, cx| v.editor.clone().unwrap().read(cx).state.clone());
    state.update_in(f.cx, |s, window, cx| s.focus(window, cx));
    f.cx.run_until_parked();
    let seen = std::rc::Rc::new(std::cell::Cell::new(0));
    let count = seen.clone();
    let _watch = f.cx.update(|_, cx| cx.observe(&state, move |_, _| count.set(count.get() + 1)));
    f.cx.executor().advance_clock(Duration::from_secs(60));
    f.cx.run_until_parked();
    assert_eq!(seen.get(), 0, "the editor notified while nothing happened");
    let calls = f.host.calls().len();
    f.cx.executor().advance_clock(Duration::from_secs(600));
    f.cx.run_until_parked();
    assert_eq!(f.host.calls().len(), calls, "nothing asked of the host on its own");
}

#[gpui::test]
fn settings_read_every_row_and_write_each_change(cx: &mut TestAppContext) {
    let mut f = open(cx);
    let s = f.settings();
    for c in [cmd::WINDOW_PREFS, cmd::GET_ACCOUNTS_ROOT, cmd::HELPERS_ABOVE_CALLER, cmd::HELPER_SETTINGS, cmd::CODEX_TRUSTED_ROOTS] {
        assert_eq!(f.calls(c).len(), 1, "{c}");
    }
    assert_eq!(f.calls(cmd::CLI_STATUS), vec![json!({ "provider": "claude" }), json!({ "provider": "codex" })]);

    s.update(f.cx, |v, cx| v.set_prefs(|p| p.sound = false, cx));
    s.update(f.cx, |v, cx| v.set_prefs(|p| p.new_chat.tools = true, cx));
    assert_eq!(
        f.host.sends_of(cmd::SET_WINDOW_PREFS),
        vec![
            json!({ "prefs": { "sound": false, "newChat": { "worktree": true, "tools": false } } }),
            json!({ "prefs": { "sound": false, "newChat": { "worktree": true, "tools": true } } }),
        ]
    );

    *f.host.desktop.picked.lock().unwrap() = Some("D:\\accounts".into());
    s.update_in(f.cx, |v, window, cx| v.change_root(window, cx));
    f.cx.run_until_parked();
    s.update(f.cx, |v, cx| v.set_root(String::new(), cx));
    f.cx.run_until_parked();
    assert_eq!(f.calls(cmd::SET_ACCOUNTS_ROOT), vec![json!({ "path": "D:\\accounts" }), json!({ "path": "" })]);

    s.update(f.cx, |v, cx| {
        v.set_max_helpers(9, cx);
        v.set_helper_mode("account", cx);
        v.set_context_pct(0, cx);
        v.set_trusted(vec!["E:\\Projects".into(), "D:\\work".into()], cx);
    });
    f.cx.run_until_parked();
    assert_eq!(f.calls(cmd::SET_MAX_HELPERS), vec![json!({ "n": 9 })]);
    assert_eq!(f.calls(cmd::SET_HELPER_MODE), vec![json!({ "mode": "account" })]);
    assert_eq!(f.calls(cmd::SET_HELPER_CONTEXT_PCT), vec![json!({ "pct": 0 })]);
    assert_eq!(f.calls(cmd::SET_CODEX_TRUSTED_ROOTS), vec![json!({ "roots": ["E:\\Projects", "D:\\work"] })]);
    assert_eq!(s.read_with(f.cx, |v, _| v.trusted.clone()), Some(vec!["E:\\Projects".into(), "D:\\work".into()]));

    // A failure is said on its page.
    f.host.fail(cmd::SET_MAX_HELPERS, Some("refused"));
    s.update(f.cx, |v, cx| v.set_max_helpers(40, cx));
    f.cx.run_until_parked();
    assert_eq!(s.read_with(f.cx, |v, _| v.error.clone()), Some((Page::Agents, "refused".into())));
}

#[gpui::test]
fn helpers_above_their_caller_go_on_only_after_the_windows_yes(cx: &mut TestAppContext) {
    let mut f = open(cx);
    let s = f.settings();
    s.update(f.cx, |v, cx| v.set_above(true, false, cx));
    f.cx.run_until_parked();
    assert!(f.calls(cmd::SET_HELPERS_ABOVE_CALLER).is_empty(), "nothing before the yes");
    assert_eq!(s.read_with(f.cx, |v, _| v.asking()), Some("above"));
    s.update(f.cx, |v, cx| v.answer_no(cx));
    assert!(f.calls(cmd::SET_HELPERS_ABOVE_CALLER).is_empty());
    s.update(f.cx, |v, cx| v.set_above(true, false, cx));
    s.update(f.cx, |v, cx| v.answer_yes(cx));
    f.cx.run_until_parked();
    assert_eq!(f.calls(cmd::SET_HELPERS_ABOVE_CALLER), vec![json!({ "on": true, "confirmed": true })]);
    assert_eq!(s.read_with(f.cx, |v, _| v.above), Some(true));
    // Off takes no yes.
    s.update(f.cx, |v, cx| v.set_above(false, false, cx));
    f.cx.run_until_parked();
    assert_eq!(f.calls(cmd::SET_HELPERS_ABOVE_CALLER).last(), Some(&json!({ "on": false, "confirmed": false })));
}

#[gpui::test]
fn a_clis_check_goes_to_the_host_and_its_pass_is_followed(cx: &mut TestAppContext) {
    let mut f = open(cx);
    let s = f.settings();
    s.update(f.cx, |v, cx| v.check_cli(Provider::Codex, cx));
    f.cx.run_until_parked();
    assert_eq!(f.calls(cmd::CLI_UPDATE_NOW), vec![json!({ "provider": "codex" })]);
    let versions = f.calls(cmd::CLI_VERSIONS).len();
    // The pass, as `cli://status` tells it.
    let status =
        |phase: &str| json!({ "provider": "codex", "phase": phase, "version": "0.170.0", "latest": "0.171.0", "got": 5, "total": 10 });
    assert!(f.mailbox.event(super::event::CLI_STATUS, status("downloading")));
    f.cx.run_until_parked();
    assert_eq!(s.read_with(f.cx, |v, _| crate::settings::cli_line(v, Provider::Codex)).as_deref(), Some("Downloading 0.171.0: 50%"));
    assert!(f.mailbox.event(super::event::CLI_STATUS, status("idle")));
    f.cx.run_until_parked();
    assert_eq!(f.calls(cmd::CLI_VERSIONS).len(), versions + 1, "its versions read again once it is done");

    // A failure is said under its CLI.
    f.host.fail(cmd::CLI_UPDATE_NOW, Some("offline"));
    s.update(f.cx, |v, cx| v.check_cli(Provider::Claude, cx));
    f.cx.run_until_parked();
    assert_eq!(s.read_with(f.cx, |v, _| v.clis[0].error.clone()).as_deref(), Some("offline"));
}

#[gpui::test]
fn an_update_is_installed_only_once_confirmed_with_its_version_and_notes(cx: &mut TestAppContext) {
    let mut f = open(cx);
    let s = f.settings();
    s.update(f.cx, |v, cx| v.check_update(cx));
    f.cx.run_until_parked();
    let offer = match s.read_with(f.cx, |v, _| v.update.clone()) {
        UpdateState::Available(o) => o,
        other => panic!("{other:?}"),
    };
    assert_eq!(offer.version, "0.20.1");
    assert!(offer.notes.as_deref().unwrap().contains("steady"));
    s.update(f.cx, |v, cx| v.ask_install(cx));
    assert_eq!(s.read_with(f.cx, |v, _| v.asking()), Some("install"));
    assert!(f.calls(crate::desktop::cmd::UPDATE_DOWNLOAD).is_empty());
    s.update(f.cx, |v, cx| v.answer_yes(cx));
    f.cx.run_until_parked();
    assert_eq!(f.calls(crate::desktop::cmd::UPDATE_DOWNLOAD), vec![json!({ "version": "0.20.1" })]);
    assert_eq!(f.calls(crate::desktop::cmd::UPDATE_INSTALL), vec![json!({ "reason": "installing the update to 0.20.1" })]);
    // None on offer: this one is the newest.
    f.host.answer(crate::desktop::cmd::UPDATE_CHECK, json!({ "current": "0.20.0", "offer": null }));
    s.update(f.cx, |v, cx| v.check_update(cx));
    f.cx.run_until_parked();
    assert!(matches!(s.read_with(f.cx, |v, _| v.update.clone()), UpdateState::Current(_)));
    // A download that fails says so and installs nothing.
    f.host.answer(crate::desktop::cmd::UPDATE_CHECK, json!({ "current": "0.20.0", "offer": { "version": "0.20.2" } }));
    f.host.fail(crate::desktop::cmd::UPDATE_DOWNLOAD, Some("signature does not match"));
    s.update(f.cx, |v, cx| v.check_update(cx));
    f.cx.run_until_parked();
    s.update(f.cx, |v, cx| v.ask_install(cx));
    s.update(f.cx, |v, cx| v.answer_yes(cx));
    f.cx.run_until_parked();
    assert_eq!(s.read_with(f.cx, |v, _| v.update.clone()), UpdateState::Failed("signature does not match".into()));
    assert_eq!(f.calls(crate::desktop::cmd::UPDATE_INSTALL).len(), 1);
}

#[gpui::test]
fn accounts_list_with_their_limits_and_add_rename_delete_and_sync(cx: &mut TestAppContext) {
    let mut f = open(cx);
    let a = f.accounts();
    assert_eq!(f.calls(cmd::LIST_ACCOUNTS).len(), 1);
    let claude = f.calls(cmd::ACCOUNT_LIMITS);
    assert_eq!(claude.len(), 2);
    assert_eq!(claude[0], json!({ "accountPath": "C:\\Users\\me\\luna-accounts\\anthropic\\work", "force": false }));
    assert_eq!(f.calls(cmd::CODEX_LIMITS), vec![json!({ "accountPath": "C:\\Users\\me\\luna-accounts\\openai\\main", "force": false })]);
    let worst = a.read_with(f.cx, |v, _| match v.limits.get("C:\\Users\\me\\luna-accounts\\anthropic\\work") {
        Some(crate::accounts::LimitsState::Got(l)) => l.worst(),
        other => panic!("{other:?}"),
    });
    assert!((worst - 0.88).abs() < 1e-6);

    // Refresh: past the cache.
    let work = a.read_with(f.cx, |v, _| v.accounts.clone().unwrap()[0].clone());
    a.update(f.cx, |v, cx| v.ask_limits(&work, true, cx));
    f.cx.run_until_parked();
    assert_eq!(f.calls(cmd::ACCOUNT_LIMITS).last(), Some(&json!({ "accountPath": work.path, "force": true })));

    // Add.
    a.update(f.cx, |v, cx| {
        v.adding = true;
        v.add_provider = Provider::Codex;
        cx.notify();
    });
    let field = a.read_with(f.cx, |v, _| v.add_field());
    field.update_in(f.cx, |s, window, cx| s.set_value(" spare ", window, cx));
    a.update_in(f.cx, |v, window, cx| v.create(window, cx));
    f.cx.run_until_parked();
    assert_eq!(f.calls(cmd::CREATE_ACCOUNT), vec![json!({ "provider": "codex", "name": "spare" })]);
    assert_eq!(f.calls(cmd::LIST_ACCOUNTS).len(), 2, "the list read again");
    assert!(!a.read_with(f.cx, |v, _| v.adding));

    // Rename and delete; delete asks first.
    a.update(f.cx, |v, cx| v.rename(&work, "office", cx));
    f.cx.run_until_parked();
    assert_eq!(f.calls(cmd::RENAME_ACCOUNT), vec![json!({ "provider": "claude", "name": "work", "to": "office" })]);
    a.update(f.cx, |v, cx| v.ask_delete(1, cx));
    assert_eq!(a.read_with(f.cx, |v, _| v.asking_delete().map(|x| x.name.clone())).as_deref(), Some("home"));
    assert!(f.calls(cmd::DELETE_ACCOUNT).is_empty());
    let home = a.read_with(f.cx, |v, _| v.asking_delete().cloned().unwrap());
    a.update(f.cx, |v, cx| v.delete(&home, cx));
    f.cx.run_until_parked();
    assert_eq!(f.calls(cmd::DELETE_ACCOUNT), vec![json!({ "provider": "claude", "name": "home" })]);

    // A failure is said at the top.
    f.host.fail(cmd::RENAME_ACCOUNT, Some("the account is in use"));
    a.update(f.cx, |v, cx| v.rename(&work, "x", cx));
    f.cx.run_until_parked();
    assert_eq!(a.read_with(f.cx, |v, _| v.error.clone()).as_deref(), Some("the account is in use"));

    // The settings' parent, sync now and at every start.
    assert_eq!(f.calls(cmd::SETTINGS_SYNC_STATE).len(), 1);
    a.update(f.cx, |v, cx| {
        v.set_parent(Provider::Claude, Some("home".into()), cx);
        v.set_parent(Provider::Codex, None, cx);
        v.set_sync_on_start(true, cx);
        v.sync_now(Provider::Claude, cx);
    });
    f.cx.run_until_parked();
    assert_eq!(
        f.calls(cmd::SET_SETTINGS_PARENT),
        vec![json!({ "provider": "claude", "parent": "home" }), json!({ "provider": "codex", "parent": null })]
    );
    assert_eq!(f.calls(cmd::SET_SETTINGS_SYNC_ON_START), vec![json!({ "on": true })]);
    assert_eq!(f.calls(cmd::SYNC_SETTINGS_NOW), vec![json!({ "provider": "claude" })]);
    assert_eq!(a.read_with(f.cx, |v, _| v.sync_note.get(&Provider::Claude).cloned()).as_deref(), Some("Copied to 1 account"));
}

/// The driver's lines do what the clicks do, in the panel up.
#[gpui::test]
fn the_drivers_lines_reach_the_panels(cx: &mut TestAppContext) {
    let mut f = open(cx);
    let drive = |f: &mut Fixture<'_>, line: &str| {
        let (verb, rest) = line.split_once(' ').unwrap_or((line, ""));
        let done = f.cx.update(|window, cx| super::drive(verb, rest, window, cx));
        f.cx.run_until_parked();
        done
    };
    assert!(drive(&mut f, "panel library"));
    assert!(drive(&mut f, "select skill review"));
    assert!(!drive(&mut f, "page clis"), "not the library's");
    assert!(drive(&mut f, "panel settings"));
    assert!(drive(&mut f, "setting sound off"));
    assert_eq!(f.host.sends_of(cmd::SET_WINDOW_PREFS).last().map(|v| v["prefs"]["sound"].clone()), Some(json!(false)));
    assert!(drive(&mut f, "cli-check claude"));
    assert_eq!(f.calls(cmd::CLI_UPDATE_NOW), vec![json!({ "provider": "claude" })]);
    assert!(drive(&mut f, "panel none"));
    assert!(f.open().is_none());
}

#[test]
fn the_panels_commands_are_named_once_and_beside_the_others() {
    let mut all: Vec<&str> = cmd::CALLED.iter().chain(cmd::SENT).copied().collect();
    all.sort_unstable();
    assert!(all.windows(2).all(|w| w[0] != w[1]), "{all:?}");
    for c in cmd::CALLED.iter().chain(cmd::SENT) {
        assert!(!crate::cmd::ALL.contains(c) && !crate::desktop::cmd::CALLED.contains(c), "{c}");
    }
    assert!(crate::event::wants(super::event::CLI_STATUS));
}
