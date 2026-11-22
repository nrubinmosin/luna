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
use crate::settings::{Page, SettingsPanel};
use gpui::{Entity, TestAppContext, VisualTestContext};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

struct Fixture<'a> {
    host: Arc<FakeHost>,
    mailbox: Mailbox,
    view: Entity<LunaWindow>,
    cx: &'a mut VisualTestContext,
    /// Calls made by the time the window had started: the status bar's own
    /// first asks (the accounts' usage, the CLIs) are not the panels'.
    started: usize,
}

fn open(cx: &mut TestAppContext) -> Fixture<'_> {
    open_as(cx, |_| {})
}

/// `open`, the host told `first` before the window starts.
fn open_as(cx: &mut TestAppContext, first: impl FnOnce(&FakeHost)) -> Fixture<'_> {
    // A sign-in terminal's parser has a thread of its own.
    cx.executor().allow_parking();
    let host = FakeHost::new();
    demo::answer_all(&host);
    host.answer(crate::cmd::ENSURE_CLAUDE_SESSION, crate::demo::attached("", 1));
    host.answer(crate::cmd::ENSURE_CODEX_SESSION, crate::demo::attached("", 1));
    first(&host);
    let up = CoreState { state: LinkState::Up, core: Some(crate::demo::core()), error: None, revision: 1 };
    host.answer(crate::cmd::CORE_STATE, serde_json::to_value(up).unwrap());
    let (mailbox, inbox) = mailbox();
    let models = cx.update(|cx| crate::start(host.clone(), inbox, ThemePref::Light, cx));
    let (view, cx) = cx.add_window_view(move |window, cx| LunaWindow::new(models, window, cx));
    cx.run_until_parked();
    let started = host.calls().len();
    Fixture { host, mailbox, view, cx, started }
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

    /// The calls of `cmd`; of one the status bar asks too, only those since
    /// the window started.
    fn calls(&self, cmd: &str) -> Vec<Value> {
        let status = crate::status::cmd::CALLED.contains(&cmd);
        let from = if status { self.started } else { 0 };
        self.host.calls().into_iter().skip(from).filter(|(c, _)| c == cmd).map(|(_, a)| a).collect()
    }

    /// Every call that is not the status bar's own polling.
    fn panel_calls(&self) -> usize {
        self.host.calls().iter().filter(|(c, _)| !crate::status::cmd::CALLED.contains(&c.as_str())).count()
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
    // A skill is not judged: what SKILL.md says is the CLIs' to read.
    assert!(f.calls(cmd::LIBRARY_CHECK_SYSTEM).is_empty());

    f.type_in_editor(&lib, "More. ");
    assert!(doc.read_with(f.cx, |d, cx| d.dirty(cx)));
    f.cx.executor().advance_clock(Duration::from_millis(300));
    f.cx.run_until_parked();
    assert!(f.calls(cmd::LIBRARY_CHECK_SYSTEM).is_empty());
    assert_eq!(doc.read_with(f.cx, |d, _| d.problem.clone()), None);

    f.cx.dispatch_action(super::Save);
    f.cx.run_until_parked();
    let text = doc.read_with(f.cx, |d, cx| d.text(cx));
    // Bound to the library and the revision it was read from.
    assert_eq!(
        f.calls(cmd::LIBRARY_WRITE_CHECKED),
        vec![json!({
            "entry": { "skill": "review" },
            "text": text,
            "root": demo::LIBRARY_ROOT,
            "rootId": demo::LIBRARY_ROOT_ID,
            "revision": "r1",
            "force": false,
        })]
    );
    assert!(doc.read_with(f.cx, |d, cx| d.saved && !d.dirty(cx)));
    assert_eq!(
        doc.read_with(f.cx, |d, _| d.file.as_ref().unwrap().revision.clone()).as_deref(),
        Some("r2"),
        "the next save goes on from it"
    );
    // Nothing to save: nothing is sent.
    f.cx.dispatch_action(super::Save);
    f.cx.run_until_parked();
    assert_eq!(f.calls(cmd::LIBRARY_WRITE_CHECKED).len(), 1);
    assert!(f.calls(cmd::LIBRARY_WRITE).is_empty(), "never the unbound save");

    // A failed save keeps the text unsaved and says why.
    f.type_in_editor(&lib, "y");
    f.host.fail(cmd::LIBRARY_WRITE_CHECKED, Some("disk full"));
    f.cx.dispatch_action(super::Save);
    f.cx.run_until_parked();
    assert!(doc.read_with(f.cx, |d, cx| d.dirty(cx) && d.error.as_deref() == Some("disk full")));
}

/// The file changed on disk since it was read: nothing is saved, the text
/// stays, and Overwrite or Reload answers it.
#[gpui::test]
fn a_save_over_a_file_changed_on_disk_keeps_the_text_and_offers_both_ways(cx: &mut TestAppContext) {
    let mut f = open(cx);
    let lib = f.library();
    f.select(&lib, Sel::Skill("review".into()));
    let doc = lib.read_with(f.cx, |v, _| v.editor.clone().unwrap());
    f.type_in_editor(&lib, "Mine. ");
    f.host.answer(
        cmd::LIBRARY_WRITE_CHECKED,
        json!({ "conflict": "The file was changed on disk since it was opened. Nothing was saved.", "revision": "r9", "overview": null }),
    );
    f.cx.dispatch_action(super::Save);
    f.cx.run_until_parked();
    let (dirty, conflict, saved) = doc.read_with(f.cx, |d, cx| (d.dirty(cx), d.conflict.clone(), d.saved));
    assert!(dirty && !saved);
    assert!(conflict.is_some_and(|c| c.contains("changed on disk")));
    assert!(doc.read_with(f.cx, |d, cx| d.status(cx).1.contains("Reload takes the file's")));
    // Overwrite: the same save, forced.
    f.host.answer(cmd::LIBRARY_WRITE_CHECKED, json!({ "conflict": null, "revision": "r10", "overview": null }));
    doc.update(f.cx, |d, cx| d.overwrite(cx));
    f.cx.run_until_parked();
    assert_eq!(f.calls(cmd::LIBRARY_WRITE_CHECKED).last().unwrap()["force"], json!(true));
    assert!(doc.read_with(f.cx, |d, cx| d.saved && !d.dirty(cx) && d.conflict.is_none()));
    // Reload: the file read again, in place of the text.
    f.type_in_editor(&lib, "More. ");
    let reads = f.calls(cmd::LIBRARY_READ).len();
    doc.update_in(f.cx, |d, window, cx| d.reload(window, cx));
    f.cx.run_until_parked();
    assert_eq!(f.calls(cmd::LIBRARY_READ).len(), reads + 1);
    assert_eq!(doc.read_with(f.cx, |d, cx| (d.text(cx), d.dirty(cx))), (demo::SKILL_TEXT.to_owned(), false));
}

/// Text not saved is kept with the host as it changes and forgotten once
/// saved; kept from before, it is offered at the next open, and restored it
/// is bound to what it was read from.
#[gpui::test]
fn unsaved_text_is_kept_on_disk_and_offered_at_the_next_open(cx: &mut TestAppContext) {
    let mut f = open(cx);
    let lib = f.library();
    f.select(&lib, Sel::Instructions("main".into()));
    f.type_in_editor(&lib, "edit ");
    let kept = f.host.sends_of(cmd::SET_LIBRARY_DRAFT);
    let last = kept.last().unwrap();
    assert_eq!(last["draft"]["entry"], json!({ "instructions": "main" }));
    assert_eq!(last["draft"]["revision"], json!("r1"));
    f.cx.dispatch_action(super::Save);
    f.cx.run_until_parked();
    let id = last["draft"]["id"].clone();
    assert!(id.as_str().is_some_and(|s| !s.is_empty()));
    assert_eq!(f.host.sends_of(cmd::SET_LIBRARY_DRAFT).last(), Some(&json!({ "draft": null, "forget": id })), "forgotten once saved");
    f.escape();

    // The window went with text not saved: offered when the library opens.
    let draft = json!({ "id": "before", "entry": { "skill": "review" }, "root": demo::LIBRARY_ROOT, "rootId": "id0", "revision": "r0", "text": "kept text" });
    f.host.answer(cmd::LIBRARY_DRAFT, draft);
    let lib = f.library();
    assert!(lib.read_with(f.cx, |v, _| v.offer.is_some()));
    let offer = lib.read_with(f.cx, |v, _| v.offer.clone().unwrap());
    lib.update_in(f.cx, |v, window, cx| v.guard(crate::library::Next::Restore(offer), window, cx));
    f.cx.run_until_parked();
    let doc = lib.read_with(f.cx, |v, _| v.editor.clone().unwrap());
    assert_eq!(lib.read_with(f.cx, |v, _| v.selected.clone()), Some(Sel::Skill("review".into())));
    assert_eq!(doc.read_with(f.cx, |d, cx| (d.text(cx), d.dirty(cx))), ("kept text".to_owned(), true));
    // Nothing has the focus in the restored editor: its Save button, not Ctrl+S.
    doc.update(f.cx, |d, cx| d.save(cx));
    f.cx.run_until_parked();
    let write = f.calls(cmd::LIBRARY_WRITE_CHECKED).last().unwrap().clone();
    assert_eq!((&write["revision"], &write["rootId"]), (&json!("r0"), &json!("id0")), "held to what the draft was read from");
    // Saved, the restored draft is forgotten as its own.
    assert_eq!(f.host.sends_of(cmd::SET_LIBRARY_DRAFT).last(), Some(&json!({ "draft": null, "forget": "before" })));
    // Thrown away: forgotten.
    f.escape();
    let lib = f.library();
    lib.update(f.cx, |v, cx| v.drop_offer(cx));
    assert!(lib.read_with(f.cx, |v, _| v.offer.is_none()));
    assert_eq!(f.host.sends_of(cmd::SET_LIBRARY_DRAFT).last(), Some(&json!({ "draft": null, "forget": "before" })));
}

/// The draft on offer thrown away after another file's text was kept: the
/// forget names the old draft alone, so the newer one's recovery stays.
#[gpui::test]
fn throwing_an_old_draft_away_leaves_a_newer_one(cx: &mut TestAppContext) {
    let mut f = open(cx);
    let old = json!({ "id": "old", "entry": { "skill": "review" }, "root": demo::LIBRARY_ROOT, "revision": "r0", "text": "kept text" });
    f.host.answer(cmd::LIBRARY_DRAFT, old);
    let lib = f.library();
    assert!(lib.read_with(f.cx, |v, _| v.offer.is_some()));
    f.select(&lib, Sel::Instructions("main".into()));
    f.type_in_editor(&lib, "newer ");
    let newer = f.host.sends_of(cmd::SET_LIBRARY_DRAFT).last().unwrap()["draft"]["id"].clone();
    assert!(newer.as_str().is_some_and(|id| !id.is_empty() && id != "old"));
    lib.update(f.cx, |v, cx| v.drop_offer(cx));
    assert_eq!(f.host.sends_of(cmd::SET_LIBRARY_DRAFT).last(), Some(&json!({ "draft": null, "forget": "old" })));
}

/// Saved, but laying the library out failed after: the save stands and the
/// failure is said apart, not as a save that failed.
#[gpui::test]
fn a_save_that_held_is_not_failed_by_the_layout_after(cx: &mut TestAppContext) {
    let mut f = open(cx);
    let lib = f.library();
    f.select(&lib, Sel::Skill("review".into()));
    let doc = lib.read_with(f.cx, |v, _| v.editor.clone().unwrap());
    f.type_in_editor(&lib, "Mine. ");
    f.host.answer(
        cmd::LIBRARY_WRITE_CHECKED,
        json!({ "conflict": null, "revision": "r3", "overview": null, "layoutError": "no lock on library-state.json.lock" }),
    );
    f.cx.dispatch_action(super::Save);
    f.cx.run_until_parked();
    assert!(doc.read_with(f.cx, |d, cx| d.saved && !d.dirty(cx) && d.error.is_none()));
    let (kind, said) = doc.read_with(f.cx, |d, cx| d.status(cx));
    assert_eq!(kind, crate::library::StatusKind::Attention);
    assert!(said.starts_with("Saved, but laying it out failed: no lock"), "{said}");
}

/// Every way the editor's text is replaced or closed asks first while it is
/// not saved: a rename, Delete..., Back to Luna's..., Add folder..., the
/// library's folder changed or back to Luna's own. A changed folder leaves
/// nothing picked.
#[gpui::test]
fn every_transition_that_replaces_the_editor_asks_first(cx: &mut TestAppContext) {
    use crate::library::Next;
    let mut f = open(cx);
    let lib = f.library();
    let dirty = |f: &mut Fixture, lib: &Entity<LibraryPanel>| {
        f.select(lib, Sel::Skill("review".into()));
        f.type_in_editor(lib, "edit ");
    };
    dirty(&mut f, &lib);
    for next in [
        Next::Rename(Entry::Skill("review".into()), "audit".into()),
        Next::Delete(Entry::Skill("review".into())),
        Next::Reset("role-lead".into()),
        Next::AddFolder("D:\\work".into()),
        Next::Source("D:\\other-library".into()),
        Next::Source(String::new()),
    ] {
        lib.update_in(f.cx, |v, window, cx| v.guard(next.clone(), window, cx));
        f.cx.run_until_parked();
        assert!(lib.read_with(f.cx, |v, _| v.asking_discard()), "{next:?} asks");
        f.escape();
    }
    assert!(f.calls(cmd::LIBRARY_RENAME_SKILL).is_empty() && f.calls(cmd::LIBRARY_SET_SOURCE).is_empty());
    assert!(f.calls(cmd::LIBRARY_SET_FOLDER).is_empty());
    // The rename's field and button go the same way.
    lib.update_in(f.cx, |v, window, cx| v.rename_to(Entry::Skill("review".into()), "audit", window, cx));
    f.cx.run_until_parked();
    assert!(lib.read_with(f.cx, |v, _| v.asking_discard()));
    f.escape();
    // Yes: the text goes, and the library's folder changes, nothing picked.
    lib.update_in(f.cx, |v, window, cx| v.guard(Next::Source("D:\\other-library".into()), window, cx));
    lib.update_in(f.cx, |v, window, cx| v.answer_yes(window, cx));
    f.cx.run_until_parked();
    assert_eq!(f.calls(cmd::LIBRARY_SET_SOURCE), vec![json!({ "path": "D:\\other-library" })]);
    assert!(lib.read_with(f.cx, |v, _| v.selected.is_none() && v.editor.is_none()));
    // Clean, it goes at once.
    f.select(&lib, Sel::Skill("review".into()));
    lib.update_in(f.cx, |v, window, cx| v.guard(Next::Source(String::new()), window, cx));
    f.cx.run_until_parked();
    assert_eq!(f.calls(cmd::LIBRARY_SET_SOURCE).last(), Some(&json!({ "path": "" })));
    assert!(lib.read_with(f.cx, |v, _| v.editor.is_none()));
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
    lib.update_in(f.cx, |v, window, cx| v.ask_delete(window, cx));
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

/// A skill's rename says how it went: done, put back as it was, or left
/// partly done.
#[gpui::test]
fn a_skill_rename_says_whether_it_was_done_undone_or_left_partly(cx: &mut TestAppContext) {
    use crate::library::Outcome;
    let mut f = open(cx);
    let lib = f.library();
    let library = lib.read_with(f.cx, |v, _| v.library.clone());
    let rename = |f: &mut Fixture, answer: Value| {
        f.host.answer(cmd::LIBRARY_RENAME_SKILL, answer);
        let got = std::rc::Rc::new(std::cell::Cell::new(None));
        let put = got.clone();
        library.update(f.cx, |l, cx| {
            let task = l.rename_skill("review", "audit", cx);
            cx.spawn(async move |_, _| put.set(task.await)).detach();
        });
        f.cx.run_until_parked();
        (got.get(), library.read_with(f.cx, |l, _| l.error.clone()))
    };
    let (outcome, error) = rename(&mut f, json!({ "outcome": "committed", "why": null, "overview": demo::overview() }));
    assert_eq!((outcome, error), (Some(Outcome::Committed), None));
    let (outcome, error) = rename(&mut f, json!({ "outcome": "rolledBack", "why": "no settings", "overview": null }));
    assert_eq!(outcome, Some(Outcome::RolledBack));
    assert!(error.is_some_and(|e| e.contains("not renamed") && e.contains("as it was")));
    let (outcome, error) = rename(&mut f, json!({ "outcome": "rollbackFailed", "why": "the folder is left at audit", "overview": null }));
    assert_eq!(outcome, Some(Outcome::RollbackFailed));
    assert!(error.is_some_and(|e| e.contains("only partly") && e.contains("left at audit")));
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
    // Canceled: nothing.
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
    // A system text is checked as it is read and a moment after the last
    // key; a problem found is said, the text saved all the same.
    let checks = f.calls(cmd::LIBRARY_CHECK_SYSTEM).len();
    assert_eq!(checks, 1, "checked as it is read");
    f.type_in_editor(&lib, "More. ");
    assert_eq!(f.calls(cmd::LIBRARY_CHECK_SYSTEM).len(), checks);
    f.host.answer(cmd::LIBRARY_CHECK_SYSTEM, json!("It names no role"));
    f.cx.executor().advance_clock(Duration::from_millis(300));
    f.cx.run_until_parked();
    assert_eq!(f.calls(cmd::LIBRARY_CHECK_SYSTEM).len(), checks + 1);
    let doc = lib.read_with(f.cx, |v, _| v.editor.clone().unwrap());
    assert_eq!(doc.read_with(f.cx, |d, _| d.problem.clone()).as_deref(), Some("It names no role"));
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
}

#[gpui::test]
fn a_sync_says_where_it_is_in_words(cx: &mut TestAppContext) {
    let mut f = open(cx);
    let lib = f.library();
    let library = lib.read_with(f.cx, |v, _| v.library.clone());
    library.update(f.cx, |l, cx| l.sync_now(cx));
    assert_eq!(library.read_with(f.cx, |l, _| l.sync.clone()), crate::library::SyncState::Syncing);
    f.cx.run_until_parked();
    assert_eq!(library.read_with(f.cx, |l, _| l.sync.clone()), crate::library::SyncState::Synced(Default::default()));
    assert_eq!(f.calls(cmd::LIBRARY_SYNC).len(), 1, "one pass, nothing after it");
    f.host.fail(cmd::LIBRARY_SYNC, Some("no lock"));
    library.update(f.cx, |l, cx| l.sync_now(cx));
    f.cx.run_until_parked();
    assert_eq!(library.read_with(f.cx, |l, _| l.sync.clone()), crate::library::SyncState::Failed("no lock".into()));
}

/// Sync is the core's pass alone (ADR-0030): one command, after which it
/// says what it wrote and took out, what it left alone as not Luna's, and
/// what it could not do.
#[gpui::test]
fn a_sync_says_what_it_wrote_took_out_left_and_could_not_do(cx: &mut TestAppContext) {
    let mut f = open(cx);
    let lib = f.library();
    let library = lib.read_with(f.cx, |v, _| v.library.clone());
    let mut o = demo::overview();
    o["places"] = json!([
        { "dest": "C:\\a\\skills\\review", "status": "failed", "problem": "not written over, its backup would not write" },
        { "dest": "C:\\b\\skills\\review", "status": "behind" },
        { "dest": "C:\\c\\CLAUDE.md", "status": "taken" },
        { "dest": "C:\\d\\skills\\review", "status": "current" },
    ]);
    o["written"] = json!(["C:\\d\\skills\\review"]);
    o["removed"] = json!(["C:\\e\\skills\\old", "C:\\e\\CLAUDE.md"]);
    o["notRemoved"] = json!(["C:\\f\\skills\\old: a link on the way"]);
    f.host.answer(cmd::LIBRARY_SYNC, o);
    library.update(f.cx, |l, cx| l.sync_now(cx));
    f.cx.run_until_parked();
    assert_eq!(f.calls(cmd::LIBRARY_SYNC), vec![json!({})]);
    let crate::library::SyncState::Synced(r) = library.read_with(f.cx, |l, _| l.sync.clone()) else { panic!("not synced") };
    assert_eq!((r.written, r.removed, r.foreign), (1, 2, 1));
    assert!(!r.clean(), "a copy not laid out is no success");
    assert!(r.said().contains("Not done: C:\\a\\skills\\review: not written over, its backup would not write."), "{}", r.said());
    assert!(r.said().contains("C:\\b\\skills\\review: changed there while Sync was at it"), "{}", r.said());
    assert!(r.said().contains("Not done: C:\\f\\skills\\old: a link on the way."), "{}", r.said());
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
    // The highlighter parses within a budget of real time, and past it in
    // the background after a 150 ms wait (gpui-component's PARSE_DEBOUNCE):
    // on a slow machine that parse lands later and redraws once. Waited out
    // here, so what is counted below is only what comes of itself.
    f.cx.executor().advance_clock(Duration::from_secs(1));
    f.cx.run_until_parked();
    let seen = std::rc::Rc::new(std::cell::Cell::new(0));
    let count = seen.clone();
    let _watch = f.cx.update(|_, cx| cx.observe(&state, move |_, _| count.set(count.get() + 1)));
    f.cx.executor().advance_clock(Duration::from_secs(60));
    f.cx.run_until_parked();
    assert_eq!(seen.get(), 0, "the editor notified while nothing happened");
    let calls = f.panel_calls();
    f.cx.executor().advance_clock(Duration::from_secs(600));
    f.cx.run_until_parked();
    assert_eq!(f.panel_calls(), calls, "nothing asked of the host on its own (the status bar's polling aside)");
}

#[gpui::test]
fn settings_read_every_row_and_write_each_change(cx: &mut TestAppContext) {
    let mut f = open(cx);
    let s = f.settings();
    for c in [cmd::WINDOW_PREFS, cmd::GET_ACCOUNTS_ROOT, cmd::HELPER_SETTINGS, cmd::CODEX_TRUSTED_ROOTS] {
        // The window's workspace asked for the preferences at its start too.
        let asked = if c == cmd::WINDOW_PREFS { 2 } else { 1 };
        assert_eq!(f.calls(c).len(), asked, "{c}");
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
        v.set_context_pct(0, cx);
        v.set_trusted(vec!["E:\\Projects".into(), "D:\\work".into()], cx);
    });
    f.cx.run_until_parked();
    assert_eq!(f.calls(cmd::SET_MAX_HELPERS), vec![json!({ "n": 9 })]);
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

/// What the Roll back row of `provider` offers: its label, its line and
/// whether the button works.
fn roll_back_offer(f: &mut Fixture<'_>, provider: Provider) -> (String, String, bool) {
    let s = f.settings();
    s.read_with(f.cx, |v, _| v.clis[usize::from(provider == Provider::Codex)].versions.clone().unwrap_or_default().roll_back_offer())
}

#[gpui::test]
fn roll_back_is_open_only_where_a_way_back_is_kept_and_nothing_refuses_it(cx: &mut TestAppContext) {
    let mut f = open(cx);
    // Open: the version it goes to is in the label.
    let (label, line, open) = roll_back_offer(&mut f, Provider::Claude);
    assert_eq!((label.as_str(), open), ("Roll back to 2.1.328", true));
    assert_eq!(line, "Goes back to 2.1.328, the version kept before this one.");
    // Refused: shut, and the core's words for why are the line.
    let (label, line, open) = roll_back_offer(&mut f, Provider::Codex);
    assert_eq!((label.as_str(), open), ("Roll back to 0.170.0", false));
    assert_eq!(line, "Roll back is refused: 0.170.0 was turned away (rolled back by hand).");
    let s = f.settings();
    s.update(f.cx, |v, cx| v.ask_roll_back(Provider::Codex, cx));
    assert_eq!(s.read_with(f.cx, |v, _| v.asking()), None, "a refused rollback asks nothing");
    // Nothing kept.
    assert_eq!(
        crate::settings::CliVersions::default().roll_back_offer(),
        ("Roll back".into(), "No earlier version is kept to go back to.".into(), false)
    );
    // No current version shown to go back from.
    let mut versions = crate::settings::CliVersions { way_back: Some(Default::default()), ..Default::default() };
    assert!(!versions.roll_back_offer().2);
    versions.versions.push(crate::settings::VersionRow { version: "2.1.330".into(), current: true, ..Default::default() });
    assert!(versions.roll_back_offer().2);
    assert!(f.calls(cmd::CLI_ROLL_BACK).is_empty());
}

#[gpui::test]
fn roll_back_asks_once_naming_both_versions_and_then_goes_back_from_the_one_seen(cx: &mut TestAppContext) {
    let mut f = open(cx);
    let s = f.settings();
    s.update(f.cx, |v, cx| v.ask_roll_back(Provider::Claude, cx));
    assert_eq!(s.read_with(f.cx, |v, _| v.asking()), Some("roll-back"));
    assert_eq!(
        s.read_with(f.cx, |v, _| crate::settings::roll_back_asked(v)).as_deref(),
        Some("Roll back Claude Code from 2.1.330 to 2.1.328?")
    );
    // No: nothing is sent.
    s.update(f.cx, |v, cx| v.answer_no(cx));
    f.cx.run_until_parked();
    assert!(f.calls(cmd::CLI_ROLL_BACK).is_empty());

    // Yes: the version the person saw is the `from`, and what the core says
    // it did is shown; the CLI's versions are read again.
    let (statuses, versions) = (f.calls(cmd::CLI_STATUS).len(), f.calls(cmd::CLI_VERSIONS).len());
    s.update(f.cx, |v, cx| v.ask_roll_back(Provider::Claude, cx));
    s.update(f.cx, |v, cx| v.answer_yes(cx));
    f.cx.run_until_parked();
    assert_eq!(f.calls(cmd::CLI_ROLL_BACK), vec![json!({ "provider": "claude", "from": "2.1.330" })]);
    assert_eq!(
        s.read_with(f.cx, |v, _| v.clis[0].rolled.clone()),
        Some(Ok(crate::settings::RolledBack { from: "2.1.330".into(), to: "2.1.328".into() }))
    );
    assert_eq!((f.calls(cmd::CLI_STATUS).len(), f.calls(cmd::CLI_VERSIONS).len()), (statuses + 1, versions + 1));
    assert!(!s.read_with(f.cx, |v, _| v.clis[0].rolling));
    assert_eq!(s.read_with(f.cx, |v, _| v.asking()), None);
}

#[gpui::test]
fn a_rollback_the_core_refuses_says_why_and_a_moved_version_is_looked_at_again(cx: &mut TestAppContext) {
    let mut f = open(cx);
    let s = f.settings();
    let versions = f.calls(cmd::CLI_VERSIONS).len();
    // Something else moved the CLI between the look and the click.
    let moved = "the current version changed (it is 2.1.331 now); look again";
    f.host.fail(cmd::CLI_ROLL_BACK, Some(moved));
    s.update(f.cx, |v, cx| v.ask_roll_back(Provider::Claude, cx));
    s.update(f.cx, |v, cx| v.answer_yes(cx));
    f.cx.run_until_parked();
    assert_eq!(s.read_with(f.cx, |v, _| v.clis[0].rolled.clone()), Some(Err(moved.into())));
    assert_eq!(f.calls(cmd::CLI_VERSIONS).len(), versions + 1, "the versions are read again");
    assert!(!s.read_with(f.cx, |v, _| v.clis[0].rolling));

    // Any other refusal is said and nothing is read again; asking again
    // clears it, and a success replaces it.
    f.host.fail(cmd::CLI_ROLL_BACK, Some("2.1.328: no longer whole"));
    s.update(f.cx, |v, cx| v.ask_roll_back(Provider::Claude, cx));
    s.update(f.cx, |v, cx| v.answer_yes(cx));
    f.cx.run_until_parked();
    assert_eq!(s.read_with(f.cx, |v, _| v.clis[0].rolled.clone()), Some(Err("2.1.328: no longer whole".into())));
    assert_eq!(f.calls(cmd::CLI_VERSIONS).len(), versions + 1);
    assert_eq!(f.calls(cmd::CLI_ROLL_BACK).len(), 2);
    f.host.fail(cmd::CLI_ROLL_BACK, None);
    s.update(f.cx, |v, cx| v.ask_roll_back(Provider::Claude, cx));
    s.update(f.cx, |v, cx| v.answer_yes(cx));
    f.cx.run_until_parked();
    assert!(s.read_with(f.cx, |v, _| v.clis[0].rolled.clone()).is_some_and(|r| r.is_ok()));
}

#[gpui::test]
fn accounts_list_with_their_limits_and_add_rename_delete_and_sync(cx: &mut TestAppContext) {
    let mut f = open(cx);
    // The window's own chat list asked for them at its start.
    let listed = f.calls(cmd::LIST_ACCOUNTS).len();
    let a = f.accounts();
    assert_eq!(f.calls(cmd::LIST_ACCOUNTS).len(), listed + 1);
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
    assert!(f.calls(cmd::LIST_ACCOUNTS).len() >= listed + 2, "the list read again, by this window and the chat list's");
    assert!(!a.read_with(f.cx, |v, _| v.adding));

    // Rename and delete; delete asks first.
    a.update(f.cx, |v, cx| v.rename(&work, "office", cx));
    f.cx.run_until_parked();
    assert_eq!(f.calls(cmd::RENAME_ACCOUNT), vec![json!({ "provider": "claude", "name": "work", "to": "office" })]);
    // The question names the folder as it is now, and the yes is bound to it.
    a.update(f.cx, |v, cx| v.ask_delete(1, cx));
    f.cx.run_until_parked();
    assert_eq!(f.calls(cmd::ACCOUNT_IDENTITY), vec![json!({ "provider": "claude", "name": "home" })]);
    assert_eq!(a.read_with(f.cx, |v, _| v.asking_delete().map(|x| x.name.clone())).as_deref(), Some("home"));
    assert!(f.calls(cmd::DELETE_ACCOUNT_CHECKED).is_empty());
    a.update(f.cx, |v, cx| v.answer_yes(cx));
    f.cx.run_until_parked();
    assert_eq!(
        f.calls(cmd::DELETE_ACCOUNT_CHECKED),
        vec![json!({ "provider": "claude", "name": "home", "path": "C:\\Users\\me\\luna-accounts\\anthropic\\work", "id": "f00d:01" })]
    );
    assert!(f.calls(cmd::DELETE_ACCOUNT).is_empty(), "never the unbound one");
    // A folder that cannot be named: no question, the reason said.
    f.host.fail(cmd::ACCOUNT_IDENTITY, Some("not an account folder of Luna's (a link)"));
    a.update(f.cx, |v, cx| v.ask_delete(0, cx));
    f.cx.run_until_parked();
    assert!(a.read_with(f.cx, |v, _| v.asking_delete().is_none() && v.error.as_deref().is_some_and(|e| e.contains("link"))));

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

/// A click on the drawn control, as a person's.
#[cfg(debug_assertions)]
fn press(f: &mut Fixture<'_>, selector: &'static str) {
    let at = f.cx.debug_bounds(selector).unwrap_or_else(|| panic!("no {selector} drawn")).center();
    f.cx.simulate_click(at, gpui::Modifiers::none());
    f.cx.run_until_parked();
}

/// Each account's switch for agents shows the core's list as it was read
/// (as the window opens and on Refresh), and flipping it asks the core for
/// that account alone and takes the list it answers.
#[cfg(debug_assertions)]
#[gpui::test]
fn an_accounts_agent_switch_shows_the_core_list_and_sets_it(cx: &mut TestAppContext) {
    let mut f = open(cx);
    let a = f.accounts();
    f.cx.run_until_parked();
    let may = |f: &mut Fixture<'_>, i: usize| {
        let account = account_of(f, &a, i);
        a.read_with(f.cx, |v, _| v.agents_may(&account))
    };
    // The demo's core keeps agents off claude/home.
    assert_eq!(f.calls(cmd::AGENT_BLOCKED_ACCOUNTS).len(), 1);
    assert_eq!((may(&mut f, 0), may(&mut f, 1), may(&mut f, 2)), (true, false, true));

    // Switching work off asks for that account alone, and shows the list the
    // core answers.
    f.host.answer(cmd::SET_AGENT_ACCOUNT, json!(["claude/home", "claude/work"]));
    press(&mut f, "accounts-agents-0");
    assert_eq!(f.calls(cmd::SET_AGENT_ACCOUNT), vec![json!({ "provider": "claude", "name": "work", "allowed": false })]);
    assert_eq!((may(&mut f, 0), may(&mut f, 1)), (false, false));
    // And home on again.
    f.host.answer(cmd::SET_AGENT_ACCOUNT, json!(["claude/work"]));
    press(&mut f, "accounts-agents-1");
    assert_eq!(f.calls(cmd::SET_AGENT_ACCOUNT).last(), Some(&json!({ "provider": "claude", "name": "home", "allowed": true })));
    assert_eq!((may(&mut f, 0), may(&mut f, 1)), (false, true));

    // Refresh reads the list again: a change made elsewhere shows.
    f.host.answer(cmd::AGENT_BLOCKED_ACCOUNTS, json!([]));
    press(&mut f, "accounts-refresh-0");
    assert_eq!(f.calls(cmd::AGENT_BLOCKED_ACCOUNTS).len(), 2);
    assert!(may(&mut f, 0));

    // A refusal from the core is said, and the switch stays as it was.
    f.host.fail(cmd::SET_AGENT_ACCOUNT, Some("settings.json cannot be written"));
    press(&mut f, "accounts-agents-0");
    assert_eq!(a.read_with(f.cx, |v, _| v.error.clone()).as_deref(), Some("settings.json cannot be written"));
    assert!(may(&mut f, 0));
}

/// While an update installs, as for a rename, the switch is refused before
/// the core is asked; thawed, the same click goes through.
#[cfg(debug_assertions)]
#[gpui::test]
fn an_accounts_agent_switch_is_refused_while_an_update_installs(cx: &mut TestAppContext) {
    let mut f = open(cx);
    let a = f.accounts();
    let workspace = f.view.read_with(f.cx, |v, _| v.models().workspace.clone());
    workspace.update(f.cx, |w, cx| w.freeze(cx));
    f.cx.run_until_parked();
    press(&mut f, "accounts-agents-0");
    assert!(f.calls(cmd::SET_AGENT_ACCOUNT).is_empty(), "nothing asked of the core");
    assert_eq!(a.read_with(f.cx, |v, _| v.error.clone()).as_deref(), Some(crate::workspace::FROZEN));
    let work = account_of(&mut f, &a, 0);
    assert!(a.read_with(f.cx, |v, _| v.agents_may(&work)), "the switch is as it was");

    workspace.update(f.cx, |w, cx| w.thaw(cx));
    f.cx.run_until_parked();
    press(&mut f, "accounts-agents-0");
    assert_eq!(f.calls(cmd::SET_AGENT_ACCOUNT), vec![json!({ "provider": "claude", "name": "work", "allowed": false })]);
    assert_eq!(a.read_with(f.cx, |v, _| v.error.clone()), None);
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

/// The title bar's buttons, drawn and clicked as a person does.
#[cfg(debug_assertions)]
mod title_bar {
    use super::*;
    use gpui::Modifiers;

    fn click(f: &mut Fixture<'_>, selector: &'static str) {
        let at = f.cx.debug_bounds(selector).unwrap_or_else(|| panic!("no {selector} drawn")).center();
        f.cx.simulate_click(at, Modifiers::none());
        f.cx.run_until_parked();
    }

    fn shown(f: &mut Fixture<'_>) -> Option<Panel> {
        f.open().map(|o| o.panel())
    }

    #[gpui::test]
    fn its_menu_opens_the_library_and_the_accounts_and_the_status_bar_the_settings(cx: &mut TestAppContext) {
        let mut f = open(cx);
        assert_eq!(shown(&mut f), None);
        assert!(f.cx.debug_bounds("title-library").is_none(), "the list is closed");
        for (key, panel) in [("library", Panel::Library), ("accounts", Panel::Accounts)] {
            // The layer starts below the bar: the Menu is one click away
            // with a panel up, and the window's own buttons stay reachable.
            click(&mut f, "title-menu");
            let item: &'static str = Box::leak(format!("title-{key}").into_boxed_str());
            click(&mut f, item);
            assert_eq!(shown(&mut f), Some(panel));
            assert!(f.cx.debug_bounds("title-library").is_none(), "the list closes once one is picked");
        }
        assert!(f.cx.debug_bounds("title-settings").is_none(), "Settings is the status bar's");
        // The layer covers the status bar while a panel is up.
        f.escape();
        click(&mut f, "status-settings");
        assert_eq!(shown(&mut f), Some(Panel::Settings));
        let bar = f.cx.debug_bounds("title-menu").unwrap();
        let scrim = f.cx.debug_bounds("panels").expect("the layer is drawn");
        assert!(scrim.top() >= bar.bottom(), "the scrim covers the bar");
        f.escape();
        assert_eq!(shown(&mut f), None);
    }
}

/// Settings opened while the host takes 0.19's choices shows none and
/// changes none; once they are in, a change is made over them, never over
/// the copy the host had before the import.
#[gpui::test]
fn settings_opened_during_the_import_writes_over_none_of_it(cx: &mut TestAppContext) {
    let old = serde_json::to_value(crate::import::fixture()).unwrap();
    let mut f = open_as(cx, |h| {
        h.answer(crate::cmd::WINDOW_STATE, json!({ "saved": null, "old": old }));
        h.defer(cmd::IMPORT_WINDOW_PREFS);
    });
    let s = f.settings();
    s.update(f.cx, |v, cx| v.set_prefs(|p| p.sound = true, cx));
    assert!(f.host.sends_of(cmd::SET_WINDOW_PREFS).is_empty(), "nothing to change yet");
    let imported = json!({ "sound": false, "newChat": { "worktree": false, "tools": true } });
    assert!(f.host.resolve(cmd::IMPORT_WINDOW_PREFS, Ok(json!({ "prefs": imported }))));
    f.cx.run_until_parked();
    s.update(f.cx, |v, cx| v.set_prefs(|p| p.sound = true, cx));
    assert_eq!(
        f.host.sends_of(cmd::SET_WINDOW_PREFS),
        vec![json!({ "prefs": { "sound": true, "newChat": { "worktree": false, "tools": true } } })]
    );
}

#[gpui::test]
fn the_new_chat_defaults_settings_write_are_the_workspaces(cx: &mut TestAppContext) {
    // What an earlier run kept comes with the start.
    let mut f = open_as(cx, |h| {
        h.answer(cmd::WINDOW_PREFS, json!({ "sound": false, "newChat": { "worktree": false, "tools": true } }));
    });
    let workspace = f.view.read_with(f.cx, |v, _| v.models().workspace.clone());
    let start = workspace.read_with(f.cx, |w, _| w.new_chat_start());
    assert!(!start.worktree && start.tools);
    assert!(!workspace.read_with(f.cx, |w, _| w.prefs().sound));
    // A change in Settings is the next dialog's start.
    let s = f.settings();
    s.update(f.cx, |v, cx| v.set_prefs(|p| p.new_chat.worktree = true, cx));
    assert!(workspace.read_with(f.cx, |w, _| w.new_chat_start().worktree));
    assert_eq!(f.host.sends_of(cmd::SET_WINDOW_PREFS).last().map(|v| v["prefs"]["newChat"]["worktree"].clone()), Some(json!(true)));
}

fn account_of(f: &mut Fixture<'_>, a: &Entity<AccountsPanel>, i: usize) -> crate::accounts::Account {
    a.read_with(f.cx, |v, _| v.accounts.clone().unwrap()[i].clone())
}

fn exit_of(f: &mut Fixture<'_>, id: &str, code: u32) {
    assert!(f.mailbox.event(crate::event::PTY_EXIT, json!({ "id": id, "code": code, "generation": 1 })));
    f.cx.run_until_parked();
}

#[gpui::test]
fn signing_in_runs_the_clis_login_in_a_terminal_and_closes_with_it(cx: &mut TestAppContext) {
    let mut f = open(cx);
    let a = f.accounts();
    let (work, main) = (account_of(&mut f, &a, 0), account_of(&mut f, &a, 2));
    // Claude Code: a bare session in the account's folder.
    a.update_in(f.cx, |v, window, cx| v.sign_in(work.clone(), window, cx));
    f.cx.run_until_parked();
    assert!(a.read_with(f.cx, |v, _| v.login().is_some()));
    let started = f.calls(crate::cmd::ENSURE_CLAUDE_SESSION);
    assert_eq!(started.len(), 1);
    assert_eq!(started[0]["id"], "login:claude:work");
    assert_eq!(started[0]["folder"].as_str(), Some(work.path.as_str()));
    assert_eq!(started[0]["accountPath"].as_str(), Some(work.path.as_str()));
    assert_eq!(started[0]["model"], "sonnet");
    assert_eq!(started[0]["permissionMode"], "default");
    assert_eq!(started[0]["worktree"], false);
    // The CLI exits, as it does once the login is done: the window closes
    // and the account list and its limits are read again.
    let (listed, asked) = (f.calls(cmd::LIST_ACCOUNTS).len(), f.calls(cmd::ACCOUNT_LIMITS).len());
    exit_of(&mut f, "login:claude:work", 0);
    assert!(a.read_with(f.cx, |v, _| v.login().is_none()));
    assert!(f.calls(cmd::LIST_ACCOUNTS).len() > listed);
    assert_eq!(f.calls(cmd::ACCOUNT_LIMITS).len(), asked + 1);
    assert_eq!(f.calls(cmd::ACCOUNT_LIMITS).last(), Some(&json!({ "accountPath": work.path, "force": true })));
    assert!(f.calls(crate::cmd::KILL_SESSION).is_empty(), "one that exited has nothing to kill");

    // Codex: `codex login`.
    a.update_in(f.cx, |v, window, cx| v.sign_in(main.clone(), window, cx));
    f.cx.run_until_parked();
    let started = f.calls(crate::cmd::ENSURE_CODEX_SESSION);
    assert_eq!(started[0]["id"], "login:codex:main");
    assert_eq!(started[0]["login"], true);
    assert_eq!(started[0]["accountPath"].as_str(), Some(main.path.as_str()));
}

#[gpui::test]
fn a_login_that_failed_stays_for_its_words_and_closing_a_live_one_ends_it(cx: &mut TestAppContext) {
    let mut f = open(cx);
    let a = f.accounts();
    let work = account_of(&mut f, &a, 0);
    a.update_in(f.cx, |v, window, cx| v.sign_in(work.clone(), window, cx));
    f.cx.run_until_parked();
    exit_of(&mut f, "login:claude:work", 1);
    assert!(a.read_with(f.cx, |v, _| v.login().is_some()), "what the CLI said stays to be read");
    // Closed by hand: Esc with the terminal not holding the keyboard.
    a.update(f.cx, |v, cx| assert!(!v.may_close(cx)));
    f.cx.run_until_parked();
    assert!(a.read_with(f.cx, |v, _| v.login().is_none()));
    assert!(f.calls(crate::cmd::KILL_SESSION).is_empty());

    // One still running is killed when its window is closed.
    a.update_in(f.cx, |v, window, cx| v.sign_in(work.clone(), window, cx));
    f.cx.run_until_parked();
    a.update(f.cx, |v, cx| assert!(!v.may_close(cx)));
    f.cx.run_until_parked();
    assert_eq!(f.calls(crate::cmd::KILL_SESSION), vec![json!({ "id": "login:claude:work" })]);
}

#[gpui::test]
fn a_window_opened_on_an_account_waits_for_its_last_session_to_end(cx: &mut TestAppContext) {
    let mut f = open(cx);
    let a = f.accounts();
    let work = account_of(&mut f, &a, 0);
    a.update_in(f.cx, |v, window, cx| v.sign_in(work.clone(), window, cx));
    f.cx.run_until_parked();
    f.host.defer(crate::cmd::KILL_SESSION);
    a.update(f.cx, |v, cx| assert!(!v.may_close(cx)));
    f.cx.run_until_parked();
    assert_eq!(f.calls(crate::cmd::KILL_SESSION).len(), 1);
    // Opened again while the kill is unanswered: no session of its own yet,
    // so none attaches to the one being killed.
    a.update_in(f.cx, |v, window, cx| v.sign_in(work.clone(), window, cx));
    f.cx.run_until_parked();
    assert_eq!(f.calls(crate::cmd::ENSURE_CLAUDE_SESSION).len(), 1);
    assert!(f.host.resolve(crate::cmd::KILL_SESSION, Ok(Value::Null)));
    f.cx.run_until_parked();
    assert_eq!(f.calls(crate::cmd::ENSURE_CLAUDE_SESSION).len(), 2, "started once the last one was gone");
}

#[gpui::test]
fn the_delete_question_names_the_chats_set_to_the_account(cx: &mut TestAppContext) {
    let state = crate::demo::chats_state(2, true);
    let mut f = open_as(cx, |h| h.answer(crate::cmd::WINDOW_STATE, crate::demo::kept(&state)));
    let workspace = f.view.read_with(f.cx, |v, _| v.models().workspace.clone());
    let on_work = workspace.read_with(f.cx, |w, _| w.chats_on(crate::term::paste::Provider::Claude, "work"));
    assert!(!on_work.is_empty(), "the fixture has chats on claude/work");
    let a = f.accounts();
    a.update(f.cx, |v, cx| v.ask_delete(0, cx));
    f.cx.run_until_parked();
    let said = a.read_with(f.cx, |v, cx| v.asking_text(cx)).expect("the question is up");
    for name in &on_work {
        assert!(said.contains(name.as_str()), "{said}");
    }
    // An account with no chat on it says nothing of chats.
    a.update(f.cx, |v, cx| v.ask_delete(2, cx));
    f.cx.run_until_parked();
    let said = a.read_with(f.cx, |v, cx| v.asking_text(cx)).expect("the question is up");
    assert!(!said.contains("Chats are still set"), "{said}");
}

#[gpui::test]
fn renaming_an_account_takes_its_chats_along(cx: &mut TestAppContext) {
    let state = crate::demo::chats_state(2, true);
    let mut f = open_as(cx, |h| h.answer(crate::cmd::WINDOW_STATE, crate::demo::kept(&state)));
    let workspace = f.view.read_with(f.cx, |v, _| v.models().workspace.clone());
    let claude = crate::term::paste::Provider::Claude;
    let before = workspace.read_with(f.cx, |w, _| w.chats_on(claude, "work"));
    assert!(!before.is_empty());
    let a = f.accounts();
    let work = account_of(&mut f, &a, 0);
    a.update(f.cx, |v, cx| v.rename(&work, "office", cx));
    f.cx.run_until_parked();
    assert!(workspace.read_with(f.cx, |w, _| w.chats_on(claude, "work")).is_empty());
    assert_eq!(workspace.read_with(f.cx, |w, _| w.chats_on(claude, "office")), before);
}

/// The accounts window closed before the rename is answered: the chats
/// follow all the same, since the workspace takes the answer, not the window.
#[gpui::test]
fn a_rename_answered_after_the_accounts_window_closed_still_takes_the_chats(cx: &mut TestAppContext) {
    let state = crate::demo::chats_state(2, true);
    let mut f = open_as(cx, |h| h.answer(crate::cmd::WINDOW_STATE, crate::demo::kept(&state)));
    let workspace = f.view.read_with(f.cx, |v, _| v.models().workspace.clone());
    let claude = crate::term::paste::Provider::Claude;
    let before = workspace.read_with(f.cx, |w, _| w.chats_on(claude, "work"));
    f.host.defer(cmd::RENAME_ACCOUNT);
    let a = f.accounts();
    let work = account_of(&mut f, &a, 0);
    a.update(f.cx, |v, cx| v.rename(&work, "office", cx));
    f.cx.run_until_parked();
    let gone = a.downgrade();
    drop(a);
    f.show(Panel::Library);
    assert!(gone.upgrade().is_none(), "the accounts window is gone");
    assert!(f.host.resolve(cmd::RENAME_ACCOUNT, Ok(Value::Null)));
    f.cx.run_until_parked();
    assert!(workspace.read_with(f.cx, |w, _| w.chats_on(claude, "work")).is_empty());
    assert_eq!(workspace.read_with(f.cx, |w, _| w.chats_on(claude, "office")), before);
}

/// Before the chat list has come, an account is neither renamed nor
/// deleted: the list read after would put the old name back.
#[gpui::test]
fn accounts_are_not_renamed_or_deleted_before_the_chat_list_comes(cx: &mut TestAppContext) {
    let mut f = open_as(cx, |h| h.defer(crate::cmd::WINDOW_STATE));
    let a = f.accounts();
    let work = account_of(&mut f, &a, 0);
    a.update(f.cx, |v, cx| v.rename(&work, "office", cx));
    f.cx.run_until_parked();
    assert!(f.calls(cmd::RENAME_ACCOUNT).is_empty());
    assert_eq!(a.read_with(f.cx, |v, _| v.error.clone()).as_deref(), Some("Loading chats..."));
    a.update(f.cx, |v, cx| v.ask_delete(0, cx));
    f.cx.run_until_parked();
    assert!(f.calls(cmd::ACCOUNT_IDENTITY).is_empty());
    assert!(f.host.resolve(crate::cmd::WINDOW_STATE, Ok(crate::demo::kept(&crate::demo::chats_state(2, true)))));
    f.cx.run_until_parked();
    a.update(f.cx, |v, cx| v.rename(&work, "office", cx));
    f.cx.run_until_parked();
    assert_eq!(f.calls(cmd::RENAME_ACCOUNT).len(), 1);
}

#[gpui::test]
fn another_panel_taking_the_place_ends_a_login_still_running(cx: &mut TestAppContext) {
    let mut f = open(cx);
    let a = f.accounts();
    let work = account_of(&mut f, &a, 0);
    a.update_in(f.cx, |v, window, cx| v.sign_in(work.clone(), window, cx));
    f.cx.run_until_parked();
    assert!(f.calls(crate::cmd::KILL_SESSION).is_empty());
    // The test's own handle would keep the panel alive.
    drop(a);
    f.show(Panel::Library);
    f.cx.run_until_parked();
    assert_eq!(f.calls(crate::cmd::KILL_SESSION), vec![json!({ "id": "login:claude:work" })]);
}
