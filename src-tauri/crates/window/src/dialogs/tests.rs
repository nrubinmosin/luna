//! The dialogs over a fake host, on GPUI's test platform: each control asks
//! the host what the page's does, by the same command and arguments, and the
//! chat it makes, or the one it deletes, is the workspace's.

use super::{Dialog, cmd, demo};
use crate::demo::FakeHost;
use crate::dto::{CoreState, LinkState, ThemePref};
use crate::host::mailbox;
use crate::models::Models;
use crate::root::LunaWindow;
use crate::state::WindowState;
use crate::term::paste::Provider;
#[cfg(debug_assertions)]
use gpui::Modifiers;
use gpui::{Entity, TestAppContext, VisualTestContext};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

struct D<'a> {
    host: Arc<FakeHost>,
    mailbox: crate::host::Mailbox,
    models: Models,
    view: Entity<LunaWindow>,
    cx: &'a mut VisualTestContext,
}

fn open(cx: &mut TestAppContext) -> D<'_> {
    open_as(cx, |_| {})
}

/// The window over the fixture's chats and the dialogs' answers, the host told
/// `first` before the window starts.
fn open_as(cx: &mut TestAppContext, first: impl FnOnce(&FakeHost)) -> D<'_> {
    cx.executor().allow_parking();
    let host = FakeHost::new();
    demo::answer_all(&host);
    host.answer(crate::cmd::LIST_ACCOUNTS, crate::demo::accounts());
    host.answer(crate::cmd::SESSIONS_META, json!([]));
    host.answer(crate::cmd::WINDOW_STATE, crate::demo::kept(&crate::demo::chats_state(2, true)));
    host.answer(crate::cmd::ENSURE_CLAUDE_SESSION, crate::demo::attached("", 1));
    host.answer(crate::cmd::ENSURE_CODEX_SESSION, crate::demo::attached("", 1));
    first(&host);
    let up = CoreState { state: LinkState::Up, core: Some(crate::demo::core()), error: None, revision: 1 };
    host.answer(crate::cmd::CORE_STATE, serde_json::to_value(up).unwrap());
    let (mailbox, inbox) = mailbox();
    let models = cx.update(|cx| crate::start(host.clone(), inbox, ThemePref::Light, cx));
    let window_models = models.clone();
    let (view, cx) = cx.add_window_view(move |window, cx| LunaWindow::new(window_models, window, cx));
    cx.run_until_parked();
    D { host, mailbox, models, view, cx }
}

impl D<'_> {
    fn dialog(&mut self) -> Option<Dialog> {
        self.view.read_with(self.cx, |v, cx| v.dialogs.read(cx).open.clone())
    }

    fn name(&mut self) -> Option<&'static str> {
        self.dialog().map(|d| d.name())
    }

    fn state(&mut self) -> WindowState {
        self.models.workspace.read_with(self.cx, |w, _| w.state.clone())
    }

    fn calls(&self, cmd: &str) -> Vec<Value> {
        self.host.calls_of(cmd)
    }

    fn settle(&mut self) {
        self.cx.run_until_parked();
    }

    #[cfg(debug_assertions)]
    fn click(&mut self, selector: &'static str) {
        let at = self.cx.debug_bounds(selector).unwrap_or_else(|| panic!("no {selector} drawn")).center();
        self.cx.simulate_click(at, Modifiers::none());
        self.settle();
    }

    #[cfg(debug_assertions)]
    fn drawn(&mut self, selector: &'static str) -> bool {
        self.cx.debug_bounds(selector).is_some()
    }

    /// Ctrl+N.
    fn new_chat(&mut self) -> Entity<super::NewChat> {
        self.cx.dispatch_action(crate::keys::NewChat);
        self.settle();
        match self.dialog() {
            Some(Dialog::NewChat(v)) => v,
            _ => panic!("no new-chat dialog is up"),
        }
    }

    fn drive(&mut self, verb: &'static str, rest: &'static str) {
        self.cx.update(|window, cx| assert!(super::drive(verb, rest, window, cx), "{verb} {rest}"));
        self.settle();
    }

    fn escape(&mut self) {
        self.cx.dispatch_action(super::Close);
        self.settle();
    }
}

fn chat_ids(state: &WindowState) -> Vec<String> {
    state.chats().map(|c| c.id.clone()).collect()
}

#[gpui::test]
fn ctrl_n_opens_the_dialog_on_what_the_accounts_settings_say(cx: &mut TestAppContext) {
    let mut d = open(cx);
    assert_eq!(d.name(), None);
    let v = d.new_chat();
    assert_eq!(d.name(), Some("new-chat"));
    let (folder, account, claude, can) =
        v.read_with(d.cx, |v, _| (v.folder.clone(), v.account.clone().unwrap(), v.claude.settings.clone(), v.can_create()));
    // Where the last chat was made: the folder, and the account.
    assert_eq!(folder, "C:\\work\\parser");
    assert_eq!((account.provider, account.name.as_str()), (Provider::Claude, "work"));
    assert_eq!((claude.model.as_str(), claude.effort.as_str(), claude.perm.as_str()), ("Opus", "high", "Edits"));
    assert!(can, "every answer is in");
    let args = json!({ "accountPath": "C:\\accounts\\anthropic\\work", "folder": "C:\\work\\parser" });
    assert_eq!(d.calls(cmd::CLAUDE_DEFAULTS), vec![args]);
    assert_eq!(d.calls(cmd::CLAUDE_MODELS), vec![json!({ "accountPath": "C:\\accounts\\anthropic\\work", "ask": true })]);
    // The folder's check, as the start would make it, which says whether it is in git.
    assert_eq!(d.calls(cmd::CHAT_FOLDER_CHECK), vec![json!({ "folder": "C:\\work\\parser", "worktree": false })]);
    // Esc closes it, and nothing was made.
    let before = chat_ids(&d.state());
    d.escape();
    assert_eq!(d.name(), None);
    assert_eq!(chat_ids(&d.state()), before);
}

#[cfg(debug_assertions)]
#[gpui::test]
fn create_makes_the_chat_through_the_one_path_and_remembers_where(cx: &mut TestAppContext) {
    let mut d = open(cx);
    d.new_chat();
    // The folder keeps a start of its own: no worktree, Luna's tools. One box flipped, for this chat only.
    d.click("newchat-worktree");
    d.click("newchat-create");
    assert_eq!(d.name(), None, "the dialog closes once the chat is made");
    let state = d.state();
    let chat = state
        .chats()
        .find(|c| !["c-parser", "c-codex", "c-notes", "c-site", "c-old", "c-far", "c-helper"].contains(&c.id.as_str()))
        .expect("a new chat")
        .clone();
    assert_eq!((chat.provider, chat.account.as_str()), (Provider::Claude, "work"));
    assert_eq!((chat.model.as_deref(), chat.effort.as_str(), chat.perm.as_deref()), (Some("Opus"), "high", Some("Edits")));
    assert!(chat.worktree && chat.tools);
    assert_eq!(
        state.folders.iter().find(|f| f.path == r"C:\work\parser").and_then(|f| f.start.clone()).map(|s| (s.worktree, s.tools)),
        Some((false, true)),
        "a flip made for one chat is not a new habit"
    );
    assert!(chat.name.starts_with("chat "));
    assert_eq!(state.folder_of(&chat.id).map(|f| f.path.as_str()), Some("C:\\work\\parser"));
    assert_eq!(chat.board, state.board);
    // The check at creation names the flag the CLI is given.
    assert_eq!(d.calls(cmd::CHAT_FOLDER_CHECK).last(), Some(&json!({ "folder": "C:\\work\\parser", "worktree": true })));
    // Seated where it will be seen, and the folder and account kept.
    assert!(state.shown().contains(&chat.id));
    assert_eq!(state.new_chat.last_folder.as_deref(), Some("C:\\work\\parser"));
    // No trust was written: it was never asked for.
    assert!(d.calls(cmd::CLAUDE_TRUST_FOLDER).is_empty());
}

#[cfg(debug_assertions)]
#[gpui::test]
fn the_worktree_box_is_for_folders_in_git_only(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |host| {
        host.answer(cmd::CHAT_FOLDER_CHECK, json!({ "refusal": null, "inGit": false }));
    });
    let v = d.new_chat();
    assert!(!d.drawn("newchat-worktree"), "no box where no worktree can be made");
    assert!(d.drawn("newchat-tools"));
    // Outside git the folder is fine all the same: Create is open, and the chat asks for no worktree.
    assert!(v.read_with(d.cx, |v, _| v.can_create()));
    d.click("newchat-create");
    let made = d.state().chats().find(|c| c.id.starts_with('c') && !c.id.starts_with("c-")).cloned().expect("made");
    assert!(!made.worktree);
    assert_eq!(d.calls(cmd::CHAT_FOLDER_CHECK).last().unwrap()["worktree"], json!(false));
}

#[cfg(debug_assertions)]
#[gpui::test]
fn a_hidden_worktree_box_carries_nothing_into_the_chat_or_the_saved_start(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |host| host.answer(cmd::CHAT_FOLDER_CHECK, json!({ "refusal": null, "inGit": false })));
    d.models.workspace.update(d.cx, |w, _| {
        let f = w.state.folders.iter_mut().find(|f| f.path == "C:\\work\\parser").unwrap();
        f.start = Some(crate::state::FolderStart { worktree: true, tools: false });
    });
    let v = d.new_chat();
    // The box is off the page but its value is on, from the saved start.
    assert!(v.read_with(d.cx, |v, _| v.worktree));
    assert!(!d.drawn("newchat-worktree"));
    d.click("newchat-create");
    let made = d.state().chats().find(|c| c.id.starts_with('c') && !c.id.starts_with("c-")).cloned().expect("made");
    assert!(!made.worktree);
    assert_eq!(d.calls(cmd::CHAT_FOLDER_CHECK).last().unwrap()["worktree"], json!(false));
    let start = d.state().folders.iter().find(|f| f.path == r"C:\work\parser").and_then(|f| f.start.clone());
    assert_eq!(start.map(|s| s.worktree), Some(true), "the saved start is not overwritten by a box that was hidden");
}

#[cfg(debug_assertions)]
#[gpui::test]
fn a_folder_the_start_would_refuse_keeps_create_shut(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |host| host.answer(cmd::CHAT_FOLDER_CHECK, json!({ "refusal": "The folder does not exist.", "inGit": false })));
    let v = d.new_chat();
    assert!(d.drawn("newchat-refusal"));
    assert!(!v.read_with(d.cx, |v, _| v.can_create()));
    let before = chat_ids(&d.state());
    v.update_in(d.cx, |v, window, cx| v.create(window, cx));
    d.settle();
    assert_eq!(chat_ids(&d.state()), before);
    assert_eq!(d.name(), Some("new-chat"));
}

#[cfg(debug_assertions)]
#[gpui::test]
fn trust_is_asked_and_written_only_when_ticked(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |host| host.answer(cmd::CLAUDE_FOLDER_TRUSTED, json!(false)));
    let v = d.new_chat();
    assert!(d.drawn("newchat-trust"));
    assert!(v.read_with(d.cx, |v, _| v.can_create()));
    // Claude Code cannot ask about trust in a worktree it makes: with one on, Create is shut.
    d.click("newchat-worktree");
    assert!(!v.read_with(d.cx, |v, _| v.can_create()));
    d.click("newchat-worktree");
    assert!(v.read_with(d.cx, |v, _| v.can_create()));
    // Unticked: nothing is written, and the CLI asks in the chat itself.
    d.click("newchat-create");
    assert!(d.calls(cmd::CLAUDE_TRUST_FOLDER).is_empty());
    assert_eq!(d.name(), None);
}

#[cfg(debug_assertions)]
#[gpui::test]
fn a_ticked_trust_is_written_before_the_chat_exists(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |host| host.answer(cmd::CLAUDE_FOLDER_TRUSTED, json!(false)));
    d.new_chat();
    d.click("newchat-trust");
    // Ticked, the worktree is fine.
    d.click("newchat-create");
    assert_eq!(
        d.calls(cmd::CLAUDE_TRUST_FOLDER),
        vec![json!({ "accountPath": "C:\\accounts\\anthropic\\work", "folder": "C:\\work\\parser" })]
    );
    let all = d.host.calls();
    let at = |name: &str| all.iter().position(|(c, _)| c == name).unwrap_or_else(|| panic!("{name} was never called"));
    assert!(at(cmd::CHAT_FOLDER_CHECK) < at(cmd::CLAUDE_TRUST_FOLDER));
    assert_eq!(d.name(), None);
    assert!(d.state().chats().any(|c| c.id.starts_with('c') && !c.id.starts_with("c-")));
}

#[gpui::test]
fn the_effort_follows_the_model_it_is_run_on(cx: &mut TestAppContext) {
    let mut d = open(cx);
    let v = d.new_chat();
    let effort = |v: &Entity<super::NewChat>, d: &mut D<'_>| v.read_with(d.cx, |v, _| v.claude.settings.effort.clone());
    // A model that takes no effort runs with none.
    v.update(d.cx, |v, cx| v.pick_claude_model("Haiku", cx));
    assert_eq!(effort(&v, &mut d), "");
    // Sonnet starts on the level the settings files keep for it.
    v.update(d.cx, |v, cx| v.pick_claude_model("Sonnet", cx));
    assert_eq!(effort(&v, &mut d), "low");
    // One picked by hand stays through another model, where that takes it.
    v.update(d.cx, |v, cx| v.pick_effort("high", cx));
    v.update(d.cx, |v, cx| v.pick_claude_model("Opus", cx));
    assert_eq!(effort(&v, &mut d), "high");
    // And moves to what the model takes where it does not: its own default.
    v.update(d.cx, |v, cx| v.pick_effort("max", cx));
    v.update(d.cx, |v, cx| v.pick_claude_model("Sonnet", cx));
    assert_eq!(effort(&v, &mut d), "medium");
}

/// The older models are listed only under "More…", after the main ones,
/// and the model the files give is the default whichever is picked.
#[cfg(debug_assertions)]
#[gpui::test]
fn older_models_are_under_more_and_the_files_model_is_the_default(cx: &mut TestAppContext) {
    let mut d = open(cx);
    let v = d.new_chat();
    let entries = |v: &Entity<super::NewChat>, d: &mut D<'_>| v.read_with(d.cx, |v, _| v.model_entries());
    let lines = |e: &[super::choice::ModelEntry]| e.iter().map(|e| (e.label.clone(), e.group)).collect::<Vec<_>>();
    let more = Some(super::choice::MORE);
    assert_eq!(
        lines(&entries(&v, &mut d)),
        [("Opus · default".to_owned(), None), ("Sonnet".into(), None), ("Haiku".into(), more), ("Opus 4.7".into(), more)]
    );
    d.drive("newchat", "menu model");
    // One heading, over the first older model.
    assert!(d.drawn("newchat-model-group-2") && !d.drawn("newchat-model-group-3"));
    assert!(!d.drawn("newchat-model-group-0"));
    // An older model runs by its full id; the default stays the files'.
    d.click("newchat-model-3");
    assert_eq!(v.read_with(d.cx, |v, _| v.claude.settings.model.clone()), "claude-opus-4-7");
    assert_eq!(entries(&v, &mut d)[0].label, "Opus · default");
}

/// The model chosen already, picked again, is no change of the person's:
/// another folder's settings still move it.
#[cfg(debug_assertions)]
#[gpui::test]
fn picking_the_chosen_model_again_changes_nothing(cx: &mut TestAppContext) {
    let mut d = open(cx);
    let v = d.new_chat();
    d.drive("newchat", "menu model");
    d.click("newchat-model-0");
    let touched = |v: &Entity<super::NewChat>, d: &mut D<'_>| v.read_with(d.cx, |v, _| v.claude.touched.contains("model"));
    assert!(!touched(&v, &mut d));
    assert_eq!(v.read_with(d.cx, |v, _| v.menu), None, "the list closed");
    d.host.answer(cmd::CLAUDE_DEFAULTS, json!({ "model": { "value": "sonnet", "source": "project" } }));
    v.update(d.cx, |v, cx| v.choose_folder("C:\\work\\site", cx));
    d.settle();
    assert_eq!(v.read_with(d.cx, |v, _| v.claude.settings.model.clone()), "Sonnet");
    // Another row is a change.
    d.drive("newchat", "menu model");
    d.click("newchat-model-0");
    assert!(touched(&v, &mut d));
}

/// Enter with the model list open closes the list, as Esc does, and makes
/// no chat with what was chosen before.
#[cfg(debug_assertions)]
#[gpui::test]
fn enter_on_an_open_list_closes_it_and_makes_nothing(cx: &mut TestAppContext) {
    let mut d = open(cx);
    let v = d.new_chat();
    let before = chat_ids(&d.state());
    for key in ["enter", "escape"] {
        d.drive("newchat", "menu model");
        assert!(d.drawn("newchat-model-list"));
        if key == "enter" {
            d.cx.dispatch_action(super::Confirm);
        } else {
            d.cx.dispatch_action(super::Close);
        }
        d.settle();
        assert_eq!(v.read_with(d.cx, |v, _| v.menu), None, "{key} closed the list");
        assert_eq!(d.name(), Some("new-chat"), "{key} left the dialog up");
        assert_eq!(chat_ids(&d.state()), before, "{key} made no chat");
    }
    // With no list open, Enter makes the chat.
    d.cx.dispatch_action(super::Confirm);
    d.settle();
    assert_eq!(d.name(), None);
    assert_ne!(chat_ids(&d.state()), before);
}

/// Codex's model is marked as the default where config.toml gives it, the
/// mark stays on it when another row is picked, and the chat is made on the
/// one picked.
#[cfg(debug_assertions)]
#[gpui::test]
fn codexs_default_model_is_marked_and_another_picked_is_made(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |host| {
        let mut defaults = demo::codex_defaults();
        defaults["model"] = json!({ "value": "gpt-6-mini", "source": "account" });
        host.answer(cmd::CODEX_DEFAULTS, defaults);
    });
    let v = d.new_chat();
    d.drive("newchat", "account 2");
    let labels = |v: &Entity<super::NewChat>, d: &mut D<'_>| {
        v.read_with(d.cx, |v, _| v.model_entries().into_iter().map(|e| e.label).collect::<Vec<_>>())
    };
    assert_eq!(labels(&v, &mut d), ["GPT-6", "GPT-6 mini · default"]);
    d.drive("newchat", "menu model");
    d.click("newchat-model-0");
    assert_eq!(v.read_with(d.cx, |v, _| v.codex.settings.model.clone()).as_deref(), Some("gpt-6"));
    assert_eq!(labels(&v, &mut d), ["GPT-6", "GPT-6 mini · default"]);
    d.click("newchat-create");
    let chat = d.state().chats().find(|c| c.provider == Provider::Codex && c.name.starts_with("chat ")).cloned().expect("a Codex chat");
    assert_eq!(chat.model.as_deref(), Some("gpt-6"));
}

/// Until the files have answered for the account and folder on screen, no
/// model is said to be the default: what the dialog holds may be another's.
#[cfg(debug_assertions)]
#[gpui::test]
fn no_model_is_the_default_while_the_files_have_not_answered(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |host| host.defer(cmd::CLAUDE_DEFAULTS));
    let v = d.new_chat();
    let defaults = |v: &Entity<super::NewChat>, d: &mut D<'_>| {
        v.read_with(d.cx, |v, _| {
            v.model_entries().into_iter().filter(|e| e.label.ends_with("· default")).map(|e| e.value).collect::<Vec<_>>()
        })
    };
    assert!(defaults(&v, &mut d).is_empty());
    assert!(d.host.resolve(cmd::CLAUDE_DEFAULTS, Ok(demo::claude_defaults())));
    d.settle();
    assert_eq!(defaults(&v, &mut d), ["Opus"]);
    v.update(d.cx, |v, cx| v.choose_folder("C:\\work\\site", cx));
    d.settle();
    assert!(defaults(&v, &mut d).is_empty(), "the first folder's default is not this one's");
    assert!(d.host.resolve(cmd::CLAUDE_DEFAULTS, Ok(json!({ "model": { "value": "sonnet", "source": "project" } }))));
    d.settle();
    assert_eq!(defaults(&v, &mut d), ["Sonnet"]);
}

/// Each level shows the hint the model list gives it as its tooltip, and one
/// without a hint shows none.
#[cfg(debug_assertions)]
#[gpui::test]
fn an_effort_levels_hint_is_its_tooltip(cx: &mut TestAppContext) {
    use gpui::{Modifiers, point, px};
    use std::time::Duration;
    let mut d = open(cx);
    d.new_chat();
    let tip_over = |d: &mut D<'_>, selector: &'static str| {
        d.cx.simulate_mouse_move(point(px(0.), px(0.)), None, Modifiers::none());
        d.settle();
        crate::widgets::SHOWN_TIP.with(|t| t.borrow_mut().clear());
        let at = d.cx.debug_bounds(selector).unwrap_or_else(|| panic!("no {selector} drawn")).center();
        d.cx.simulate_mouse_move(at, None, Modifiers::none());
        d.cx.executor().advance_clock(Duration::from_secs(1));
        d.settle();
        let shown = crate::widgets::SHOWN_TIP.with(|t| t.borrow().iter().map(ToString::to_string).collect::<Vec<_>>());
        assert_eq!(d.cx.debug_bounds("tip").is_some(), !shown.is_empty(), "{selector}");
        shown
    };
    // Opus's levels: low, medium, high, ...; only high has a hint.
    assert_eq!(tip_over(&mut d, "newchat-effort-2"), ["deeper thinking"]);
    assert!(tip_over(&mut d, "newchat-effort-0").is_empty(), "low has none");
    // Codex's, the same way.
    d.drive("newchat", "account 2");
    assert_eq!(tip_over(&mut d, "newchat-effort-0"), ["Fast responses with lighter reasoning"]);
    assert!(tip_over(&mut d, "newchat-effort-1").is_empty(), "medium has none");
    assert_eq!(tip_over(&mut d, "newchat-effort-2"), ["Greater reasoning depth for complex problems"]);
}

#[gpui::test]
fn what_was_changed_here_survives_another_folder_and_the_boxes_start_afresh(cx: &mut TestAppContext) {
    let mut d = open(cx);
    let v = d.new_chat();
    v.update(d.cx, |v, cx| v.pick_perm("Plan only", cx));
    v.update(d.cx, |v, _| {
        v.worktree = false;
        v.tools = true;
    });
    v.update(d.cx, |v, cx| v.choose_folder("C:\\work\\site", cx));
    d.settle();
    let (perm, worktree, tools, ready) = v.read_with(d.cx, |v, _| (v.claude.settings.perm.clone(), v.worktree, v.tools, v.can_create()));
    assert_eq!(perm, "Plan only", "a touched value is the person's");
    // A flip made for the first folder was not made for this one.
    assert!(worktree && !tools);
    assert!(ready);
    assert_eq!(d.calls(cmd::CLAUDE_DEFAULTS).last().unwrap()["folder"], json!("C:\\work\\site"));
}

#[cfg(debug_assertions)]
#[gpui::test]
fn a_codex_account_opens_on_its_own_settings_and_auto_review_brings_its_sandbox(cx: &mut TestAppContext) {
    let mut d = open(cx);
    d.new_chat();
    d.drive("newchat", "account 2");
    let v = match d.dialog() {
        Some(Dialog::NewChat(v)) => v,
        _ => unreachable!(),
    };
    let s = v.read_with(d.cx, |v, _| v.codex.settings.clone());
    // The core read the files: Codex's own for both, offered and chosen.
    assert_eq!(
        (s.model.as_deref(), s.effort.as_str(), s.approval.as_str(), s.sandbox.as_str()),
        (Some("gpt-6"), "medium", "codex", "codex")
    );
    assert!(d.drawn("newchat-approval-codex") && d.drawn("newchat-sandbox-codex"));
    assert_eq!(d.calls(cmd::CODEX_DEFAULTS).len(), 1);
    // The sandbox it brings is not one to pick.
    v.update(d.cx, |v, cx| v.pick_sandbox("read-only", cx));
    v.update(d.cx, |v, cx| v.pick_approval("auto-review", cx));
    v.update(d.cx, |v, cx| v.pick_sandbox("read-only", cx));
    assert_eq!(v.read_with(d.cx, |v, _| v.codex.settings.sandbox.clone()), "workspace-write");
    d.click("newchat-create");
    let chat = d.state().chats().find(|c| c.provider == Provider::Codex && c.name.starts_with("chat ")).cloned().expect("a Codex chat");
    assert_eq!(
        (chat.model.as_deref(), chat.approval.as_deref(), chat.sandbox.as_deref()),
        (Some("gpt-6"), Some("auto-review"), Some("workspace-write"))
    );
    assert_eq!(chat.perm, None);
    assert_eq!(d.calls(cmd::CHAT_FOLDER_CHECK).last(), Some(&json!({ "folder": "C:\\work\\parser", "worktree": false })));
}

/// A Codex chat made with the modes left alone keeps the core's word for
/// Codex's own and starts with it, so no flag goes for either: never the
/// full bypass a config.toml did not ask for.
#[cfg(debug_assertions)]
#[gpui::test]
fn a_codex_chat_left_to_codex_starts_on_codexs_own(cx: &mut TestAppContext) {
    let mut d = open(cx);
    d.new_chat();
    d.drive("newchat", "account 2");
    d.click("newchat-create");
    let chat = d.state().chats().find(|c| c.provider == Provider::Codex && c.name.starts_with("chat ")).cloned().expect("a Codex chat");
    assert_eq!((chat.approval.as_deref(), chat.sandbox.as_deref()), (Some("codex"), Some("codex")));
    let started = d.calls(crate::cmd::ENSURE_CODEX_SESSION).into_iter().find(|a| a["id"] == json!(chat.id)).expect("started");
    assert!(chat.default_mode);
    assert_eq!((&started["approval"], &started["sandbox"]), (&json!(""), &json!("")));
}

#[cfg(debug_assertions)]
#[gpui::test]
fn new_chat_preselects_lunas_modes_and_keeps_an_explicit_pick(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |host| {
        host.answer(cmd::CLAUDE_DEFAULTS, demo::claude_luna_defaults());
        host.answer(cmd::CODEX_DEFAULTS, demo::codex_luna_defaults());
    });
    let v = d.new_chat();
    assert_eq!(v.read_with(d.cx, |v, _| v.claude.settings.perm.clone()), "Auto");
    d.click("newchat-perm-Auto");
    assert!(d.drawn("newchat-back-perm"), "picking the preselection is still explicit");
    d.click("newchat-back-perm");
    assert!(!d.drawn("newchat-back-perm"));
    d.drive("newchat", "account 2");
    let (approval, sandbox) = v.read_with(d.cx, |v, _| (v.codex.settings.approval.clone(), v.codex.settings.sandbox.clone()));
    assert_eq!((approval.as_str(), sandbox.as_str()), ("auto-review", "workspace-write"));
    d.click("newchat-approval-never");
    let (approval, sandbox) = v.read_with(d.cx, |v, _| (v.codex.settings.approval.clone(), v.codex.settings.sandbox.clone()));
    assert_eq!((approval.as_str(), sandbox.as_str()), ("never", "codex"));
    d.click("newchat-create");
    let chat = d.state().chats().find(|c| c.provider == Provider::Codex && c.name.starts_with("chat ")).cloned().expect("a Codex chat");
    assert!(!chat.default_mode);
    let started = d.calls(crate::cmd::ENSURE_CODEX_SESSION).into_iter().find(|a| a["id"] == json!(chat.id)).expect("started");
    assert_eq!((&started["approval"], &started["sandbox"]), (&json!("never"), &json!("codex")));
}

#[gpui::test]
fn a_sandbox_pick_wins_over_a_late_auto_review_default(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |host| host.defer(cmd::CODEX_DEFAULTS));
    let v = d.new_chat();
    d.drive("newchat", "account 2");
    v.update(d.cx, |v, cx| v.pick_sandbox("read-only", cx));
    assert!(d.host.resolve(cmd::CODEX_DEFAULTS, Ok(demo::codex_luna_defaults())));
    d.settle();
    let (approval, sandbox) = v.read_with(d.cx, |v, _| (v.codex.settings.approval.clone(), v.codex.settings.sandbox.clone()));
    assert_eq!((approval.as_str(), sandbox.as_str()), ("codex", "read-only"));
}

/// Where the core could not read the files it says so, and the dialog opens
/// on its safe choice under that name, never on "closed".
#[cfg(debug_assertions)]
#[gpui::test]
fn a_config_luna_cannot_read_opens_on_the_safe_choice_said_as_such(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |host| {
        host.answer(
            cmd::CODEX_DEFAULTS,
            json!({ "approval": { "value": "on-request", "source": "closed" }, "sandbox": { "value": "read-only", "source": "closed" } }),
        )
    });
    d.new_chat();
    d.drive("newchat", "account 2");
    let v = match d.dialog() {
        Some(Dialog::NewChat(v)) => v,
        _ => unreachable!(),
    };
    let (s, from) = v.read_with(d.cx, |v, _| (v.codex.settings.clone(), v.codex.origins.clone()));
    assert_eq!((s.approval.as_str(), s.sandbox.as_str()), ("on-request", "read-only"));
    assert!(from["approval"].contains("safe choice") && !from["approval"].starts_with("closed"), "{}", from["approval"]);
}

/// Leaving `auto-review` gives back the sandbox it took: the files' one,
/// untouched, or the one picked by hand before it.
#[cfg(debug_assertions)]
#[gpui::test]
fn leaving_auto_review_gives_the_sandbox_back(cx: &mut TestAppContext) {
    let mut d = open(cx);
    d.new_chat();
    d.drive("newchat", "account 2");
    let v = match d.dialog() {
        Some(Dialog::NewChat(v)) => v,
        _ => unreachable!(),
    };
    let modes = |d: &mut D<'_>| v.read_with(d.cx, |v, _| (v.codex.settings.sandbox.clone(), v.codex.touched.contains("sandbox")));
    v.update(d.cx, |v, cx| v.pick_approval("auto-review", cx));
    assert_eq!(modes(&mut d), ("workspace-write".to_owned(), true));
    v.update(d.cx, |v, cx| v.pick_approval("on-request", cx));
    assert_eq!(modes(&mut d), ("codex".to_owned(), false), "only the approval was changed");
    // One picked by hand comes back as picked, and stays the person's.
    v.update(d.cx, |v, cx| v.pick_sandbox("read-only", cx));
    v.update(d.cx, |v, cx| v.pick_approval("auto-review", cx));
    v.update(d.cx, |v, cx| v.pick_approval("never", cx));
    assert_eq!(modes(&mut d), ("read-only".to_owned(), true));
    // Out of auto-review and back to the start: Create passes no sandbox flag.
    v.update(d.cx, |v, cx| v.pick_sandbox("codex", cx));
    v.update(d.cx, |v, cx| v.pick_approval("auto-review", cx));
    v.update(d.cx, |v, cx| v.pick_approval("codex", cx));
    d.click("newchat-create");
    let chat = d.state().chats().find(|c| c.provider == Provider::Codex && c.name.starts_with("chat ")).cloned().expect("a Codex chat");
    assert_eq!((chat.approval.as_deref(), chat.sandbox.as_deref()), (Some("codex"), Some("codex")));
}

/// A Codex chat started again in a core that replaced the last one passes
/// the modes it saved, Codex's own still no flag.
#[cfg(debug_assertions)]
#[gpui::test]
fn a_codex_chat_started_again_in_another_core_keeps_codexs_own(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |host| {
        let mut attached = crate::demo::attached("", 1);
        attached["permission"] = json!({ "modeFrom": "cli", "approval": "codex", "sandbox": "codex" });
        host.answer(crate::cmd::ENSURE_CODEX_SESSION, attached);
    });
    d.new_chat();
    d.drive("newchat", "account 2");
    d.click("newchat-create");
    let chat = d.state().chats().find(|c| c.provider == Provider::Codex && c.name.starts_with("chat ")).cloned().expect("a Codex chat");
    let starts =
        |d: &D<'_>| d.calls(crate::cmd::ENSURE_CODEX_SESSION).into_iter().filter(|a| a["id"] == json!(chat.id)).collect::<Vec<_>>();
    let before = starts(&d).len();
    assert!(before >= 1);
    let core = crate::dto::Core { pid: 5151, ..crate::demo::core() };
    let up = CoreState { state: LinkState::Up, core: Some(core), error: None, revision: 3 };
    assert!(d.mailbox.event(crate::host::event::CORE_STATE, serde_json::to_value(up).unwrap()));
    d.settle();
    let again = starts(&d);
    assert!(again.len() > before, "started over in the new core");
    let last = again.last().unwrap();
    assert_eq!((&last["approval"], &last["sandbox"], &last["modeFrom"]), (&json!("codex"), &json!("codex"), &json!("cli")));
}

#[cfg(debug_assertions)]
#[gpui::test]
fn a_default_chat_saves_the_first_start_mode_for_resume_after_reload(cx: &mut TestAppContext) {
    use crate::host::cmd as host_cmd;

    let (id, text) = {
        let mut d = open_as(cx, |host| {
            host.answer(cmd::CODEX_DEFAULTS, demo::codex_luna_defaults());
            let mut attached = crate::demo::attached("", 1);
            // The account changed after the dialog opened, before the first
            // start. Save the core's narrower choice, not the preview.
            attached["permission"] = json!({ "modeFrom": "closed", "approval": "on-request", "sandbox": "read-only" });
            host.answer(host_cmd::ENSURE_CODEX_SESSION, attached);
        });
        d.new_chat();
        d.drive("newchat", "account 2");
        d.click("newchat-create");
        let chat = d.state().chats().find(|c| c.provider == Provider::Codex && c.name.starts_with("chat ")).cloned().unwrap();
        assert!(!chat.default_mode);
        assert_eq!(chat.sandbox.as_deref(), Some("read-only"));
        let saved = d.host.sends_of(host_cmd::SAVE_WINDOW_STATE);
        let text: Value = serde_json::from_str(saved.last().unwrap()["text"].as_str().unwrap()).unwrap();
        (chat.id, text)
    };
    let (mut state, _) = WindowState::read(&text).expect("saved window state");
    let sid = "019a0000-0000-7000-8000-00000000abed";
    state.find_mut(&id).unwrap().session_id = Some(sid.into());
    let d = open_as(cx, |host| {
        host.answer(host_cmd::WINDOW_STATE, crate::demo::kept(&state));
        host.answer(cmd::CODEX_DEFAULTS, demo::codex_defaults());
    });
    let resumed = d.calls(host_cmd::ENSURE_CODEX_SESSION).into_iter().find(|a| a["id"] == id).expect("the reloaded chat resumes");
    assert_eq!(resumed["resume"], sid);
    assert_eq!(resumed["modeFrom"], "closed");
    assert_eq!(resumed["approval"], "on-request");
    assert_eq!(resumed["sandbox"], "read-only");
}

/// A folder is one folder by any spelling of its path the system folds
/// (`same_path`): its saved start is found, kept and forgotten through any
/// of them, and its saved spelling stays.
#[gpui::test]
fn a_folder_is_one_folder_by_any_spelling(cx: &mut TestAppContext) {
    // Another spelling: on Windows forward slashes, another case and a
    // trailing slash; elsewhere the trailing slash alone.
    let other = |p: &str| match cfg!(windows) {
        true => format!("{}/", p.replace('\\', "/").to_uppercase()),
        false => format!("{p}/"),
    };
    let mut d = open(cx);
    let rows = |d: &mut D<'_>| d.state().folders.iter().filter(|f| crate::state::same_path(&f.path, r"C:\work\parser")).count();
    assert_eq!(rows(&mut d), 1);
    let ws = d.models.workspace.clone();
    assert_eq!(ws.read_with(d.cx, |w, _| w.start_in(&other(r"C:\work\parser"))), (false, true), "the fixture's start, by another spelling");
    ws.update(d.cx, |w, cx| w.keep_start(&other(r"C:\work\parser"), crate::state::FolderStart { worktree: true, tools: true }, cx));
    assert_eq!(rows(&mut d), 1, "no second row");
    assert_eq!(ws.read_with(d.cx, |w, _| w.start_in(r"C:\work\parser")), (true, true));
    assert!(d.state().folders.iter().any(|f| f.path == r"C:\work\parser"), "the saved spelling stays");
    // Browsed to by another spelling, the dialog takes the saved one.
    let v = d.new_chat();
    v.update(d.cx, |v, cx| v.picked(&other(r"C:\work\parser"), cx));
    assert_eq!(v.read_with(d.cx, |v, _| v.folder.clone()), r"C:\work\parser");
    assert_eq!(rows(&mut d), 1);
    // A chat made there by another spelling goes into the same row.
    ws.update(d.cx, |w, _| {
        let chat = w.state.find("c-parser").cloned().unwrap();
        w.state.add_chat(&other(r"C:\work\parser"), crate::state::Chat { id: "c-spelled".into(), ..chat }, true, || "f-new".into());
    });
    assert_eq!(rows(&mut d), 1);
    // A folder without chats is forgotten by any spelling.
    ws.update(d.cx, |w, cx| {
        w.remember_folder(r"C:\work\fresh", cx);
    });
    assert!(ws.update(d.cx, |w, cx| w.forget_folder(&other(r"C:\work\fresh"), cx)));
    assert!(!d.state().folders.iter().any(|f| crate::state::same_path(&f.path, r"C:\work\fresh")));
}

/// The quick chat takes Codex's own as the dialog does.
#[gpui::test]
fn a_quick_codex_chat_starts_on_codexs_own(cx: &mut TestAppContext) {
    let mut d = open(cx);
    d.models.workspace.update(d.cx, |w, _| {
        w.state.new_chat.last_account = Some(crate::state::AccountRef { provider: Provider::Codex, name: "personal".into() });
        w.state.peek = Some(crate::state::Peek::Chat("c-codex".into()));
    });
    let before = chat_ids(&d.state());
    d.cx.dispatch_action(crate::keys::QuickChat);
    d.settle();
    let state = d.state();
    let chat = state.chats().find(|c| !before.contains(&c.id)).expect("a quick chat").clone();
    assert_eq!(chat.provider, Provider::Codex);
    assert_eq!((chat.approval.as_deref(), chat.sandbox.as_deref()), (Some("codex"), Some("codex")));
}

/// "back to …" under a control changed here puts back what the files said.
#[cfg(debug_assertions)]
#[gpui::test]
fn a_changed_control_offers_the_way_back(cx: &mut TestAppContext) {
    let mut d = open(cx);
    let v = d.new_chat();
    assert!(!d.drawn("newchat-back-perm"));
    d.click("newchat-perm-Plan only");
    assert!(d.drawn("newchat-back-perm"));
    d.click("newchat-back-perm");
    let (perm, touched) = v.read_with(d.cx, |v, _| (v.claude.settings.perm.clone(), v.claude.touched.contains("perm")));
    assert_eq!((perm.as_str(), touched), ("Edits", false));
    assert!(!d.drawn("newchat-back-perm"));
    // The Codex modes go back together, as auto-review moved both.
    d.drive("newchat", "account 2");
    d.click("newchat-approval-auto-review");
    d.click("newchat-back-approval");
    let s = v.read_with(d.cx, |v, _| v.codex.settings.clone());
    assert_eq!((s.approval.as_str(), s.sandbox.as_str()), ("codex", "codex"));
}

/// The boxes flipped: "back to …" undoes it, "keep for this folder" makes it
/// the folder's start, and a start that is Settings' anyway goes back to
/// following Settings.
#[cfg(debug_assertions)]
#[gpui::test]
fn the_boxes_go_back_or_are_kept_for_the_folder(cx: &mut TestAppContext) {
    let mut d = open(cx);
    let v = d.new_chat();
    let start = |d: &mut D<'_>| {
        d.state().folders.iter().find(|f| f.path == r"C:\work\parser").and_then(|f| f.start.clone()).map(|s| (s.worktree, s.tools))
    };
    assert_eq!(start(&mut d), Some((false, true)), "the fixture's own start");
    assert!(!d.drawn("newchat-boxes-keep"));
    d.click("newchat-worktree");
    d.click("newchat-boxes-back");
    assert!(!v.read_with(d.cx, |v, _| v.worktree), "back to the folder's start");
    d.click("newchat-worktree");
    d.click("newchat-boxes-keep");
    assert_eq!(start(&mut d), Some((true, true)));
    assert!(!d.drawn("newchat-boxes-keep"), "nothing flipped any more");
    // Settings' own start (worktree on, tools off) kept: the folder follows Settings.
    d.click("newchat-tools");
    d.click("newchat-boxes-keep");
    assert_eq!(start(&mut d), None);
}

#[gpui::test]
fn browsing_remembers_the_folder_for_the_next_time(cx: &mut TestAppContext) {
    let mut d = open(cx);
    let v = d.new_chat();
    d.drive("newchat", "folder C:\\work\\fresh");
    assert_eq!(v.read_with(d.cx, |v, _| v.folder.clone()), "C:\\work\\fresh");
    let state = d.state();
    assert!(state.folders.iter().any(|f| f.path == "C:\\work\\fresh" && f.picked));
    // Forgetting one that holds no chats takes it out of the list; one with chats stays.
    let gone = d.models.workspace.update(d.cx, |w, cx| w.forget_folder("C:\\work\\fresh", cx));
    let stays = d.models.workspace.update(d.cx, |w, cx| w.forget_folder("C:\\work\\parser", cx));
    assert!(gone && !stays);
}

/// While the chat list is still being read (0.19's import can take
/// seconds), no chat is made: the list read after would replace it. The
/// dialog says why, and takes the last chat's folder once the list comes.
#[gpui::test]
fn no_chat_is_made_before_the_chat_list_comes(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |host| host.defer(crate::cmd::WINDOW_STATE));
    // Nothing on screen to go on: the quick chat opens the dialog.
    d.cx.dispatch_action(crate::keys::QuickChat);
    d.settle();
    let v = match d.dialog() {
        Some(Dialog::NewChat(v)) => v,
        _ => panic!("no new-chat dialog is up"),
    };
    let (can, wait) = v.read_with(d.cx, |v, _| (v.can_create(), v.list_wait.clone()));
    assert!(!can);
    assert_eq!(wait.as_deref(), Some("Loading chats..."));
    // The one path refuses too, before anything is asked or written.
    let (account, settings) = v.read_with(d.cx, |v, _| (v.account.clone().unwrap(), v.claude.settings.clone()));
    let make = super::create::Make::new("C:\\work\\parser", account, super::choice::Settings::Claude(settings));
    let made = Rc::new(RefCell::new(None));
    let (models, out) = (d.models.clone(), made.clone());
    d.cx.update(|_, cx| {
        cx.spawn(async move |cx| {
            let got = super::create::create(&models, make, cx).await;
            *out.borrow_mut() = Some(got);
        })
        .detach()
    });
    d.settle();
    assert_eq!(made.borrow().clone(), Some(Err("Loading chats...".to_owned())));
    assert!(d.calls(cmd::CHAT_FOLDER_CHECK).is_empty());
    // The list comes: the dialog opens on where the last chat was made.
    assert!(d.host.resolve(crate::cmd::WINDOW_STATE, Ok(crate::demo::kept(&crate::demo::chats_state(2, true)))));
    d.settle();
    let (folder, wait) = v.read_with(d.cx, |v, _| (v.folder.clone(), v.list_wait.clone()));
    assert_eq!(wait, None);
    assert_eq!(folder, "C:\\work\\parser");
    assert!(v.read_with(d.cx, |v, _| v.can_create()));
}

#[gpui::test]
fn the_quick_chat_is_made_at_once_in_the_folder_and_account_on_screen(cx: &mut TestAppContext) {
    let mut d = open(cx);
    let before = chat_ids(&d.state());
    d.cx.dispatch_action(crate::keys::QuickChat);
    d.settle();
    assert_eq!(d.name(), None, "no dialog: everything was decided");
    let state = d.state();
    assert_eq!(state.chats().count(), before.len() + 1);
    let chat = state.chats().find(|c| !before.contains(&c.id)).unwrap().clone();
    // The chat on screen is c-parser: its folder and account, the files' settings.
    assert_eq!((state.folder_of(&chat.id).map(|f| f.path.as_str()), chat.account.as_str()), (Some("C:\\work\\parser"), "work"));
    assert_eq!((chat.model.as_deref(), chat.effort.as_str(), chat.perm.as_deref()), (Some("Opus"), "high", Some("Edits")));
    // Asked as the dialog asks, through the same path.
    assert_eq!(d.calls(cmd::CLAUDE_MODELS), vec![json!({ "accountPath": "C:\\accounts\\anthropic\\work", "ask": false })]);
    assert!(!d.calls(cmd::CHAT_FOLDER_CHECK).is_empty());
}

/// The quick chat made with the saved worktree start on, in a folder whose check says `in_git`.
fn quick_worktree(cx: &mut TestAppContext, in_git: bool) -> (bool, Value) {
    let mut d = open_as(cx, |host| host.answer(cmd::CHAT_FOLDER_CHECK, json!({ "refusal": null, "inGit": in_git })));
    d.models.workspace.update(d.cx, |w, _| {
        let f = w.state.folders.iter_mut().find(|f| f.path == "C:\\work\\parser").unwrap();
        f.start = Some(crate::state::FolderStart { worktree: true, tools: false });
    });
    assert!(d.models.workspace.read_with(d.cx, |w, _| w.start_in("C:\\work\\parser").0), "the saved start is worktree on");
    let before = chat_ids(&d.state());
    d.cx.dispatch_action(crate::keys::QuickChat);
    d.settle();
    let state = d.state();
    let chat = state.chats().find(|c| !before.contains(&c.id)).expect("a quick chat").clone();
    (chat.worktree, d.calls(cmd::CHAT_FOLDER_CHECK).last().unwrap().clone())
}

#[gpui::test]
fn a_quick_chat_outside_git_asks_for_no_worktree_whatever_the_saved_start_says(cx: &mut TestAppContext) {
    let (worktree, last_check) = quick_worktree(cx, false);
    assert!(!worktree);
    assert_eq!(last_check["worktree"], json!(false));
}

#[gpui::test]
fn a_quick_chat_in_git_takes_the_saved_worktree_start(cx: &mut TestAppContext) {
    let (worktree, last_check) = quick_worktree(cx, true);
    assert!(worktree);
    assert_eq!(last_check["worktree"], json!(true));
}

#[cfg(debug_assertions)]
#[gpui::test]
fn the_quick_chat_leaves_trust_to_the_dialog(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |host| host.answer(cmd::CLAUDE_FOLDER_TRUSTED, json!(false)));
    let before = chat_ids(&d.state());
    d.cx.dispatch_action(crate::keys::QuickChat);
    d.settle();
    assert_eq!(chat_ids(&d.state()), before, "no chat: whether to trust is the person's to say");
    assert_eq!(d.name(), Some("new-chat"));
    assert!(d.drawn("newchat-trust"));
    assert!(d.calls(cmd::CLAUDE_TRUST_FOLDER).is_empty());
}

#[cfg(debug_assertions)]
#[gpui::test]
fn a_quick_chat_that_fails_falls_back_to_the_dialog_with_the_reason(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |host| host.answer(cmd::CHAT_FOLDER_CHECK, json!({ "refusal": "The folder does not exist.", "inGit": false })));
    let before = chat_ids(&d.state());
    d.cx.dispatch_action(crate::keys::QuickChat);
    d.settle();
    assert_eq!(chat_ids(&d.state()), before);
    assert_eq!(d.name(), Some("new-chat"));
    assert!(d.drawn("newchat-refusal"));
}

/// A signed-out account is marked in the picker; one not yet asked about is not.
#[gpui::test]
fn the_picker_marks_signed_out_accounts(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |host| {
        host.answer(
            crate::status::cmd::ACCOUNT_LIMITS,
            json!({ "signedIn": true, "plan": "Max", "haveUsage": true, "h5": 0.1, "week": 0.1 }),
        );
        host.answer(crate::status::cmd::CODEX_LIMITS, json!({ "signedIn": false, "haveUsage": false }));
    });
    d.cx.executor().advance_clock(std::time::Duration::from_secs(5));
    d.settle();
    let v = d.new_chat();
    let accounts = d.models.workspace.read_with(d.cx, |w, _| w.accounts().unwrap().to_vec());
    let labels: Vec<String> = v.read_with(d.cx, |v, cx| accounts.iter().map(|a| v.account_label(a, cx)).collect());
    assert_eq!(labels, ["Claude Code · work", "Codex · personal · signed out"]);
}

/// An account whose every limits ask failed is not said to be signed out:
/// nothing said so.
#[gpui::test]
fn a_failed_limits_ask_is_no_sign_out(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |host| {
        host.fail(crate::status::cmd::ACCOUNT_LIMITS, Some("offline"));
        host.fail(crate::status::cmd::CODEX_LIMITS, Some("offline"));
    });
    d.cx.executor().advance_clock(std::time::Duration::from_secs(5));
    d.settle();
    let v = d.new_chat();
    let accounts = d.models.workspace.read_with(d.cx, |w, _| w.accounts().unwrap().to_vec());
    let labels: Vec<String> = v.read_with(d.cx, |v, cx| accounts.iter().map(|a| v.account_label(a, cx)).collect());
    assert_eq!(labels, ["Claude Code · work", "Codex · personal"]);
}

/// A dialog opened before the accounts are listed takes the last account
/// once they come, not the first.
#[gpui::test]
fn accounts_listed_late_still_open_on_the_last_one(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |host| host.answer(crate::cmd::LIST_ACCOUNTS, json!([])));
    d.models.workspace.update(d.cx, |w, _| {
        w.state.new_chat.last_account = Some(crate::state::AccountRef { provider: Provider::Codex, name: "personal".into() });
    });
    let v = d.new_chat();
    assert!(v.read_with(d.cx, |v, _| v.account.is_none()), "no accounts yet");
    d.host.answer(crate::cmd::LIST_ACCOUNTS, crate::demo::accounts());
    d.models.workspace.update(d.cx, |w, cx| w.reload_accounts(cx));
    d.settle();
    let account = v.read_with(d.cx, |v, _| v.account.clone()).expect("an account once listed");
    assert_eq!((account.provider, account.name.as_str()), (Provider::Codex, "personal"));
}

/// A list 0.20.0 wrote, its Codex chat in the full bypass: on load the
/// chat is on Codex's own, starts with it, and the list is written marked.
#[gpui::test]
fn a_list_from_0_20_0_moves_its_codex_bypass_on_load(cx: &mut TestAppContext) {
    let d = open_as(cx, |host| {
        let mut state = crate::demo::chats_state(2, true);
        let c = state.find_mut("c-codex").unwrap();
        (c.approval, c.sandbox) = (Some("never".into()), Some("danger-full-access".into()));
        let mut kept = crate::demo::kept(&state);
        kept["saved"].as_object_mut().unwrap().remove("codexBypassMoved");
        host.answer(crate::cmd::WINDOW_STATE, kept);
    });
    let c = d.models.workspace.read_with(d.cx, |w, _| w.state.find("c-codex").cloned().unwrap());
    assert_eq!((c.approval.as_deref(), c.sandbox.as_deref()), (Some("codex"), Some("codex")));
    let saved = d.host.sends_of(crate::cmd::SAVE_WINDOW_STATE);
    let last = saved.last().expect("the list is written");
    let text: Value = serde_json::from_str(last["text"].as_str().unwrap()).unwrap();
    assert_eq!(text["codexBypassMoved"], json!(true));
}

/// A list 0.20.0 rewrote after the move, without the list's own mark: the
/// host's mark beside it says the move was made, and a Bypass chosen since
/// stays.
#[gpui::test]
fn the_hosts_mark_keeps_a_bypass_chosen_after_the_move(cx: &mut TestAppContext) {
    let d = open_as(cx, |host| {
        let mut state = crate::demo::chats_state(2, true);
        let c = state.find_mut("c-codex").unwrap();
        (c.approval, c.sandbox) = (Some("never".into()), Some("danger-full-access".into()));
        let mut kept = crate::demo::kept(&state);
        kept["saved"].as_object_mut().unwrap().remove("codexBypassMoved");
        kept["codexBypassMoved"] = json!(true);
        host.answer(crate::cmd::WINDOW_STATE, kept);
    });
    let c = d.models.workspace.read_with(d.cx, |w, _| w.state.find("c-codex").cloned().unwrap());
    assert_eq!((c.approval.as_deref(), c.sandbox.as_deref()), (Some("never"), Some("danger-full-access")));
}

/// The accounts listed before the chat list: the dialog takes the first,
/// then the one the last chat was made on once the list says it, unless
/// one was picked meanwhile.
#[gpui::test]
fn the_list_coming_after_the_accounts_brings_the_last_account(cx: &mut TestAppContext) {
    let state = || {
        let mut state = crate::demo::chats_state(2, true);
        state.new_chat.last_account = Some(crate::state::AccountRef { provider: Provider::Codex, name: "personal".into() });
        crate::demo::kept(&state)
    };
    for picked in [false, true] {
        let mut d = open_as(cx, |host| host.defer(crate::cmd::WINDOW_STATE));
        let v = d.new_chat();
        let first = v.read_with(d.cx, |v, _| v.account.clone()).expect("the first account");
        assert_eq!(first.name, "work");
        if picked {
            v.update(d.cx, |v, cx| v.choose_account(first.clone(), cx));
        }
        assert!(d.host.resolve(crate::cmd::WINDOW_STATE, Ok(state())));
        d.settle();
        let now = v.read_with(d.cx, |v, _| v.account.clone()).unwrap();
        assert_eq!(now.name, if picked { "work" } else { "personal" }, "picked: {picked}");
    }
}

/// Renaming the account the dialog last used keeps it the one it opens on.
#[gpui::test]
fn a_renamed_account_stays_the_last_one_used(cx: &mut TestAppContext) {
    let mut d = open(cx);
    d.host.answer(crate::panels::cmd::RENAME_ACCOUNT, json!(null));
    d.models.workspace.update(d.cx, |w, _| {
        w.state.new_chat.last_account = Some(crate::state::AccountRef { provider: Provider::Codex, name: "personal".into() });
    });
    let _renamed = d.models.workspace.update(d.cx, |w, cx| w.rename_account_on_host(Provider::Codex, "personal", "home", cx));
    d.settle();
    let last = d.state().new_chat.last_account.clone().unwrap();
    assert_eq!((last.provider, last.name.as_str()), (Provider::Codex, "home"));
}

#[cfg(debug_assertions)]
#[gpui::test]
fn the_delete_dialog_asks_first_and_deletes_through_one_path(cx: &mut TestAppContext) {
    let mut d = open(cx);
    let seam = d.models.workspace.read_with(d.cx, |w, _| w.seams.delete_chat.clone()).expect("the dialog plugs in");
    d.cx.update(|window, cx| seam("c-notes".into(), window, cx));
    d.settle();
    assert_eq!(d.name(), Some("delete"));
    // Esc and Cancel delete nothing.
    d.escape();
    assert!(d.state().find("c-notes").is_some());
    assert!(d.calls(cmd::DELETE_SESSION).is_empty());
    d.cx.update(|window, cx| seam("c-notes".into(), window, cx));
    d.settle();
    d.click("delete-cancel");
    assert!(d.state().find("c-notes").is_some());
    // No chat is no dialog.
    d.cx.update(|window, cx| seam("c-nothing".into(), window, cx));
    d.settle();
    assert_eq!(d.name(), None);

    d.cx.update(|window, cx| seam("c-notes".into(), window, cx));
    d.settle();
    d.click("delete-yes");
    assert_eq!(d.name(), None);
    assert!(d.state().find("c-notes").is_none(), "off the list and every board");
    assert_eq!(d.calls(cmd::DELETE_SESSION), vec![json!({ "id": "c-notes" })]);
    assert!(d.calls(cmd::BACKGROUND_DELETE).is_empty());
}

#[cfg(debug_assertions)]
#[gpui::test]
fn a_chats_helpers_are_kept_unless_the_person_says_otherwise(cx: &mut TestAppContext) {
    let mut d = open(cx);
    let seam = d.models.workspace.read_with(d.cx, |w, _| w.seams.delete_chat.clone()).unwrap();
    d.cx.update(|window, cx| seam("c-parser".into(), window, cx));
    d.settle();
    // Its three helpers are listed with what each has at stake, the stray one is not.
    assert_eq!(d.calls(cmd::BACKGROUND_AT_STAKE), vec![json!({ "ids": ["h-review", "h-sub", "h-done"] })]);
    assert!(d.drawn("delete-helpers-too"));
    assert!(!d.drawn("delete-stop-working"), "only once the helpers box is ticked");
    d.click("delete-yes");
    assert!(d.calls(cmd::BACKGROUND_DELETE).is_empty(), "left alone, they go by themselves once idle");
    assert_eq!(d.calls(cmd::DELETE_SESSION).len(), 1);
}

#[cfg(debug_assertions)]
#[gpui::test]
fn helpers_at_work_are_stopped_only_with_a_yes_of_their_own(cx: &mut TestAppContext) {
    let mut d = open(cx);
    d.drive("dialog", "delete c-parser");
    d.click("delete-helpers-too");
    assert!(d.drawn("delete-stop-working"));
    d.click("delete-yes");
    let gone: Vec<Value> = d.calls(cmd::BACKGROUND_DELETE);
    assert_eq!(gone.len(), 1);
    assert_eq!(gone[0]["id"], json!("h-done"));
    assert_eq!((gone[0]["stopWorking"].clone(), gone[0]["ofChat"].clone()), (json!(false), json!("c-parser")));
}

#[cfg(debug_assertions)]
#[gpui::test]
fn every_box_is_a_yes_of_its_own(cx: &mut TestAppContext) {
    let mut d = open(cx);
    d.drive("dialog", "delete c-parser");
    assert!(!d.drawn("delete-worktree"), "Luna removes no worktree: there is no box for one");
    d.drive("deletechat", "helpers");
    d.drive("deletechat", "stop");
    d.click("delete-yes");
    let gone = d.calls(cmd::BACKGROUND_DELETE);
    assert_eq!(gone.iter().map(|g| g["id"].as_str().unwrap()).collect::<Vec<_>>(), ["h-review", "h-sub", "h-done"]);
    assert_eq!(gone[0], json!({ "id": "h-review", "stopWorking": true, "ofChat": "c-parser" }));
    assert_eq!(gone[2], json!({ "id": "h-done", "stopWorking": false, "ofChat": "c-parser" }));
    assert_eq!(d.calls(cmd::DELETE_SESSION), vec![json!({ "id": "c-parser" })]);
}

#[cfg(debug_assertions)]
#[gpui::test]
fn the_helpers_popover_lists_a_chats_helpers_and_nothing_else(cx: &mut TestAppContext) {
    let mut d = open(cx);
    d.cx.update(|window, cx| super::open_helpers(Some("c-parser"), window, cx));
    d.settle();
    assert_eq!(d.name(), Some("helpers"));
    // Working tab first: the asking one, then the one at work.
    assert!(d.drawn("helper-h-review") && d.drawn("helper-h-sub"));
    assert!(!d.drawn("helper-h-done"), "the idle one is under its tab");
    assert!(!d.drawn("helper-h-loose"), "another chat's helper is not listed");
    d.drive("helpers", "tab idle");
    assert!(d.drawn("helper-h-done") && !d.drawn("helper-h-review"));
    // With no chat named: all of them.
    d.escape();
    d.cx.update(|window, cx| super::open_helpers(None, window, cx));
    d.settle();
    d.drive("helpers", "tab idle");
    assert!(d.drawn("helper-h-loose"));
    d.escape();
    assert_eq!(d.name(), None);
}

#[cfg(debug_assertions)]
#[gpui::test]
fn a_helper_is_watched_as_plain_text_stopped_and_deleted(cx: &mut TestAppContext) {
    let mut d = open(cx);
    d.cx.update(|window, cx| super::open_helpers(Some("c-parser"), window, cx));
    d.settle();
    d.click("helper-watch-h-review");
    assert_eq!(d.calls(cmd::BACKGROUND_TRANSCRIPT), vec![json!({ "id": "h-review", "since": null, "file": null })]);
    assert!(d.drawn("helpers-msg-2"), "its three messages");
    assert!(d.drawn("helpers-watch-status"));
    // Esc goes back to the list first, then closes it.
    d.escape();
    assert_eq!(d.name(), Some("helpers"));
    assert!(d.drawn("helper-h-review"));
    d.click("helper-stop-h-review");
    assert_eq!(d.calls(cmd::BACKGROUND_KILL), vec![json!({ "id": "h-review" })]);
    // One at work is not deleted before it is stopped.
    assert!(d.calls(cmd::BACKGROUND_DELETE).is_empty());
    d.drive("helpers", "tab idle");
    d.click("helper-delete-h-done");
    assert_eq!(d.calls(cmd::BACKGROUND_DELETE), vec![json!({ "id": "h-done", "stopWorking": false, "ofChat": null })]);
}

/// A watch follows new output while the reader is at its end, and leaves a
/// reader scrolled up where they are, whatever comes in meanwhile.
#[gpui::test]
fn a_watch_follows_new_output_only_while_read_to_the_end(cx: &mut TestAppContext) {
    let long: Vec<Value> = (0..40)
        .map(|i| {
            json!({ "role": "assistant", "text": format!("line {i}
of output") })
        })
        .collect();
    let mut d = open_as(cx, |host| {
        host.answer(cmd::BACKGROUND_TRANSCRIPT, json!({ "messages": long, "cursor": 40, "file": "h.jsonl", "running": true }));
    });
    d.cx.update(|window, cx| super::open_helpers(Some("c-parser"), window, cx));
    d.settle();
    d.drive("helpers", "watch h-review");
    let view = match d.dialog() {
        Some(Dialog::Helpers(v)) => v,
        _ => panic!("no helpers popover"),
    };
    let scroll = view.read_with(d.cx, |v, _| v.watch_scroll()).expect("watching");
    let y = |s: &gpui::ScrollHandle| (f32::from(s.offset().y), f32::from(s.max_offset().y));
    let poll = |d: &mut D| {
        d.cx.executor().advance_clock(super::helpers::TAIL_RUNNING);
        d.settle();
    };
    poll(&mut d);
    let (at, max) = y(&scroll);
    assert!(max > 0. && at <= -max + 1., "longer than the view, and at its end: {at} of {max}");
    // Scrolled up to read: neither an empty look nor new messages move it.
    scroll.set_offset(gpui::point(gpui::px(0.), gpui::px(0.)));
    d.host.answer(cmd::BACKGROUND_TRANSCRIPT, json!({ "messages": [], "cursor": 40, "file": "h.jsonl", "running": true }));
    poll(&mut d);
    assert_eq!(y(&scroll).0, 0.);
    d.host.answer(
        cmd::BACKGROUND_TRANSCRIPT,
        json!({ "messages": [{ "role": "assistant", "text": "more" }], "cursor": 41, "file": "h.jsonl", "running": true }),
    );
    poll(&mut d);
    assert_eq!(y(&scroll).0, 0., "left where the reader is");
    // Back at the end: followed again.
    let max = y(&scroll).1;
    scroll.set_offset(gpui::point(gpui::px(0.), gpui::px(-max)));
    poll(&mut d);
    let (at, max_now) = y(&scroll);
    assert!(max_now > max && at <= -max_now + 1., "followed: {at} of {max_now}");
}

/// Each row says whose the helper is (`top`, `watchedBy`), and so does its
/// read-only view; one with nothing to say has no such line.
#[cfg(debug_assertions)]
#[gpui::test]
fn a_helpers_row_and_view_say_who_manages_and_watches_it(cx: &mut TestAppContext) {
    let mut d = open(cx);
    d.cx.update(|window, cx| super::open_helpers(None, window, cx));
    d.settle();
    assert!(d.drawn("helper-marks-h-review") && d.drawn("helper-marks-h-sub"));
    d.drive("helpers", "tab idle");
    assert!(d.drawn("helper-marks-h-done"), "managed by chat Parser, watched by the notes chat");
    d.drive("helpers", "watch h-done");
    assert!(d.drawn("helpers-watch-marks"));
}

#[cfg(debug_assertions)]
#[gpui::test]
fn a_helper_opened_as_a_chat_is_the_persons_for_good(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |host| {
        host.answer(
            cmd::BACKGROUND_PROMOTE,
            json!({ "id": "h-done", "provider": "codex", "accountPath": "C:\\accounts\\openai\\personal", "folder": "C:\\work\\parser", "worktreePath": null,
                    "sessionId": "s-1", "name": "write-tests", "settings": { "model": "gpt-6-mini", "approval": "never" }, "tools": true }),
        );
        host.answer(cmd::BACKGROUND_PROMOTED, json!(true));
    });
    d.cx.update(|window, cx| super::open_helpers(Some("c-parser"), window, cx));
    d.settle();
    d.drive("helpers", "tab idle");
    d.click("helper-open-h-done");
    // It asks first: this takes the helper from its agent.
    assert!(d.calls(cmd::BACKGROUND_PROMOTE).is_empty());
    d.click("helpers-ask-yes");
    let begun = d.calls(cmd::BACKGROUND_PROMOTE);
    assert!(begun.len() == 1 && begun[0]["id"] == "h-done" && begun[0]["opening"].is_string(), "{begun:?}");
    let state = d.state();
    let chat = state.find("h-done").expect("a row with the helper's own id").clone();
    assert_eq!((chat.provider, chat.account.as_str()), (Provider::Codex, "personal"));
    assert_eq!(chat.name, "write-tests");
    assert_eq!(chat.session_id.as_deref(), Some("s-1"));
    assert!(chat.tools);
    // What it ran with, over what its account opens on: no sandbox named,
    // the account's, Codex's own.
    assert_eq!(
        (chat.model.as_deref(), chat.approval.as_deref(), chat.sandbox.as_deref()),
        (Some("gpt-6-mini"), Some("never"), Some("codex"))
    );
    // The core is told the row is made, and then it is seated.
    assert_eq!(d.calls(cmd::BACKGROUND_PROMOTED), vec![json!({ "id": "h-done", "error": null })]);
    assert!(state.shown().contains(&"h-done".to_owned()));
    // A helper that is not a person's own is never remembered as their last choice.
    assert_ne!(state.new_chat.last_account.as_ref().map(|a| a.name.as_str()), Some("personal"));
}

/// A helper's opening begun just before an update froze the list ends in
/// it: where the core does not settle it, the row listed for it is taken
/// off though the list is frozen, and the update's last write of the list
/// waits for that.
#[gpui::test]
fn an_opening_begun_before_the_freeze_is_rolled_back_and_waited_for(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |host| {
        host.answer(
            cmd::BACKGROUND_PROMOTE,
            json!({ "id": "h-done", "provider": "codex", "accountPath": "C:\\accounts\\openai\\personal", "folder": "C:\\work\\parser", "worktreePath": null,
                    "sessionId": "s-1", "name": "write-tests", "settings": {}, "tools": true }),
        );
        host.defer(cmd::BACKGROUND_PROMOTED);
    });
    d.cx.update(|window, cx| super::open_helpers(Some("c-parser"), window, cx));
    d.settle();
    d.drive("helpers", "open h-done");
    d.drive("helpers", "yes");
    assert!(d.state().find("h-done").is_some(), "listed while the core settles it");
    d.models.workspace.update(d.cx, |w, cx| w.freeze(cx));
    let saved = Rc::new(RefCell::new(None));
    let told = saved.clone();
    d.models.workspace.update(d.cx, |w, cx| w.save_for_install(move |r, _| *told.borrow_mut() = Some(r), cx));
    d.settle();
    assert_eq!(*saved.borrow(), None, "the last write waits for the opening");
    assert!(d.host.resolve(cmd::BACKGROUND_PROMOTED, Ok(json!(false))));
    d.settle();
    assert!(d.state().find("h-done").is_none(), "taken off though frozen");
    assert_eq!(*saved.borrow(), Some(Ok(())));
    let last = d.host.sends_of(crate::host::cmd::SAVE_WINDOW_STATE).last().cloned().expect("sent");
    assert!(!last["text"].as_str().unwrap().contains("h-done"), "the last write is without it");
}

#[gpui::test]
fn a_row_that_could_not_be_made_leaves_the_helper_its_agents(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |host| {
        host.answer(
            cmd::BACKGROUND_PROMOTE,
            json!({ "id": "h-done", "provider": "codex", "accountPath": "C:\\accounts\\openai\\gone", "folder": "C:\\work\\parser", "worktreePath": null,
                    "sessionId": null, "name": null, "settings": {}, "tools": false }),
        );
    });
    d.cx.update(|window, cx| super::open_helpers(Some("c-parser"), window, cx));
    d.settle();
    d.drive("helpers", "open h-done");
    d.drive("helpers", "yes");
    assert!(d.state().find("h-done").is_none());
    let told = d.calls(cmd::BACKGROUND_PROMOTED);
    assert_eq!(told.len(), 1);
    assert!(told[0]["error"].as_str().unwrap().contains("no codex account"));
}

/// The fixture's chats with a row made for the helper `h-done`, listed and
/// not seated, as an opening leaves it until the core lets the helper go.
fn with_promoted_row() -> Value {
    let mut state = crate::demo::chats_state(2, true);
    let folder = state.folder_of("c-parser").expect("listed").path.clone();
    let mut row = state.find("c-notes").expect("listed").clone();
    (row.id, row.name) = ("h-done".into(), "write-tests".into());
    state.add_chat(&folder, row, false, || "f-promoted".into());
    crate::demo::kept(&state)
}

/// The demo's helpers, `h-done` among them being opened as a chat, and one
/// more being opened that no row was made for.
fn helpers_opening() -> Value {
    let mut rows = demo::helpers();
    let list = rows.as_array_mut().unwrap();
    list.iter_mut().find(|h| h["id"] == "h-done").unwrap()["promoting"] = json!(true);
    list.push(json!({ "id": "h-half", "parent": "c-parser", "name": "half", "provider": "claude", "state": "idle", "account": "work",
                      "effort": "", "startedMs": 1, "promoting": true }));
    rows
}

fn new_core(d: &mut D, revision: u64) {
    let mut core = crate::demo::core();
    core.pid += revision as u32;
    let up = CoreState { state: LinkState::Up, core: Some(core), error: None, revision };
    assert!(d.mailbox.event(crate::host::event::CORE_STATE, serde_json::to_value(up).unwrap()));
    d.settle();
}

/// What a window gone before it was opening as chats is settled as it
/// starts, by what it has: the row made, the core is told and the row
/// seated; none made, the helper goes back to its agent. A helper that is
/// not being opened is left be.
#[gpui::test]
fn openings_left_unsettled_are_settled_as_the_window_starts(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |host| {
        host.answer(crate::cmd::WINDOW_STATE, with_promoted_row());
        host.answer(cmd::BACKGROUND_HELPERS, helpers_opening());
        host.answer(cmd::BACKGROUND_PROMOTED, json!(true));
    });
    let told = d.calls(cmd::BACKGROUND_PROMOTED);
    assert_eq!(told.len(), 2, "{told:?}");
    assert!(told.contains(&json!({ "id": "h-done", "error": null })));
    assert!(told.contains(&json!({ "id": "h-half", "error": "the window that was opening it as a chat went away" })));
    let state = d.state();
    assert!(state.find("h-done").is_some() && state.shown().contains(&"h-done".to_owned()), "seated once the core let go");
}

/// A row made for an opening the core has given up since (the helper back
/// with its agent) is let go of as the window starts: a pane would only run
/// its conversation beside the helper.
#[gpui::test]
fn a_row_for_an_opening_the_core_gave_up_is_let_go_at_start(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |host| {
        host.answer(crate::cmd::WINDOW_STATE, with_promoted_row());
        host.answer(cmd::BACKGROUND_PROMOTED, json!(false));
    });
    assert_eq!(d.calls(cmd::BACKGROUND_PROMOTED), vec![json!({ "id": "h-done", "error": null })]);
    assert!(d.state().find("h-done").is_none());
}

/// An opening whose row is made but which the core never answers keeps its
/// row, listed and not seated, through every try; the next core to come up
/// is told, and the row is seated then. The update's last write waits for
/// the first tries alone.
#[gpui::test]
fn an_opening_the_core_never_answered_is_settled_by_the_next_core(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |host| {
        host.answer(
            cmd::BACKGROUND_PROMOTE,
            json!({ "id": "h-done", "provider": "codex", "accountPath": "C:\\accounts\\openai\\personal", "folder": "C:\\work\\parser", "worktreePath": null,
                    "sessionId": "s-1", "name": "write-tests", "settings": {}, "tools": true }),
        );
        host.fail(cmd::BACKGROUND_PROMOTED, Some("the core went away"));
    });
    d.cx.update(|window, cx| super::open_helpers(Some("c-parser"), window, cx));
    d.settle();
    d.drive("helpers", "open h-done");
    d.drive("helpers", "yes");
    d.escape();
    let admitted = |d: &mut D| d.models.workspace.read_with(d.cx, |w, _| w.admitted_for_tests());
    assert_eq!(admitted(&mut d), 1, "an update's last write waits for the first tries");
    d.cx.executor().advance_clock(std::time::Duration::from_secs(8));
    d.settle();
    assert_eq!(admitted(&mut d), 0, "and no longer");
    d.cx.executor().advance_clock(std::time::Duration::from_secs(120));
    d.settle();
    let tries = d.calls(cmd::BACKGROUND_PROMOTED).len();
    assert_eq!(tries, 9, "every try of SETTLE_RETRY");
    let state = d.state();
    assert!(state.find("h-done").is_some() && !state.shown().contains(&"h-done".to_owned()), "kept, listed and not seated");
    assert!(!d.models.workspace.read_with(d.cx, |w, _| w.is_opening("h-done")));
    // The core back, still holding it: told now, and seated.
    d.host.fail(cmd::BACKGROUND_PROMOTED, None);
    d.host.answer(cmd::BACKGROUND_PROMOTED, json!(true));
    d.host.answer(cmd::BACKGROUND_HELPERS, helpers_opening());
    new_core(&mut d, 2);
    assert_eq!(d.calls(cmd::BACKGROUND_PROMOTED)[tries], json!({ "id": "h-done", "error": null }));
    assert!(d.state().shown().contains(&"h-done".to_owned()));
}

/// While this window is opening a helper, a core coming up settles nothing
/// of it: one opening, one word from it.
#[gpui::test]
fn a_core_coming_up_leaves_an_opening_under_way_to_it(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |host| {
        host.answer(
            cmd::BACKGROUND_PROMOTE,
            json!({ "id": "h-done", "provider": "codex", "accountPath": "C:\\accounts\\openai\\personal", "folder": "C:\\work\\parser", "worktreePath": null,
                    "sessionId": "s-1", "name": "write-tests", "settings": {}, "tools": true }),
        );
        host.defer(cmd::BACKGROUND_PROMOTED);
    });
    d.cx.update(|window, cx| super::open_helpers(Some("c-parser"), window, cx));
    d.settle();
    d.drive("helpers", "open h-done");
    d.drive("helpers", "yes");
    assert_eq!(d.calls(cmd::BACKGROUND_PROMOTED).len(), 1);
    d.host.answer(cmd::BACKGROUND_HELPERS, helpers_opening());
    new_core(&mut d, 2);
    let told = d.calls(cmd::BACKGROUND_PROMOTED);
    assert_eq!(told.iter().filter(|t| t["id"] == "h-done").count(), 1, "{told:?}");
    assert!(d.host.resolve(cmd::BACKGROUND_PROMOTED, Ok(json!(true))));
    d.settle();
    assert!(d.state().shown().contains(&"h-done".to_owned()));
}

fn promote_h_done(host: &FakeHost) {
    host.answer(
        cmd::BACKGROUND_PROMOTE,
        json!({ "id": "h-done", "provider": "codex", "accountPath": "C:\\accounts\\openai\\personal", "folder": "C:\\work\\parser", "worktreePath": null,
                "sessionId": "s-1", "name": "write-tests", "settings": {}, "tools": true }),
    );
}

fn open_h_done(d: &mut D) {
    d.cx.update(|window, cx| super::open_helpers(Some("c-parser"), window, cx));
    d.settle();
    d.drive("helpers", "open h-done");
    d.drive("helpers", "yes");
}

/// The core hears the row is made only once the host says the list holding
/// it is on disk: a window gone between would leave the helper nobody's.
/// A row that could not be written is let go of, and the helper stays its
/// agent's.
#[gpui::test]
fn an_opening_is_said_made_only_once_its_row_is_written(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |host| {
        promote_h_done(host);
        host.answer(cmd::BACKGROUND_PROMOTED, json!(true));
        host.defer(crate::cmd::FLUSH_WINDOW_STATE);
    });
    open_h_done(&mut d);
    assert!(d.state().find("h-done").is_some(), "listed");
    assert!(d.calls(cmd::BACKGROUND_PROMOTED).is_empty(), "not said made before it is written");
    let seq = d.host.sends_of(crate::cmd::SAVE_WINDOW_STATE).iter().filter_map(|a| a["seq"].as_u64()).max().unwrap();
    assert!(d.host.resolve(crate::cmd::FLUSH_WINDOW_STATE, Ok(json!({ "seq": seq }))));
    d.settle();
    assert_eq!(d.calls(cmd::BACKGROUND_PROMOTED), vec![json!({ "id": "h-done", "error": null })]);
    assert!(d.state().shown().contains(&"h-done".to_owned()));
}

#[gpui::test]
fn a_row_that_could_not_be_written_leaves_the_helper_its_agents(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |host| {
        promote_h_done(host);
        host.fail(crate::cmd::FLUSH_WINDOW_STATE, Some("disk full"));
    });
    open_h_done(&mut d);
    assert!(d.state().find("h-done").is_none(), "let go of");
    let told = d.calls(cmd::BACKGROUND_PROMOTED);
    assert_eq!(told.len(), 1, "{told:?}");
    assert!(told[0]["error"].as_str().unwrap().contains("not saved"), "{told:?}");
}

/// A row whose opening is under way, or which the core still holds for one,
/// is not deleted: the dialog says why and nothing is asked of the host.
#[cfg(debug_assertions)]
#[gpui::test]
fn a_row_being_opened_is_not_deleted(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |host| {
        host.answer(crate::cmd::WINDOW_STATE, with_promoted_row());
        host.answer(cmd::BACKGROUND_HELPERS, helpers_opening());
        host.defer(cmd::BACKGROUND_PROMOTED);
    });
    assert!(d.models.workspace.read_with(d.cx, |w, _| w.is_opening("h-done")), "being settled");
    let seam = d.models.workspace.read_with(d.cx, |w, _| w.seams.delete_chat.clone()).expect("the dialog plugs in");
    d.cx.update(|window, cx| seam("h-done".into(), window, cx));
    d.settle();
    d.click("delete-yes");
    assert!(d.calls(cmd::DELETE_SESSION).is_empty());
    let why = match d.dialog() {
        Some(Dialog::Delete(v)) => v.read_with(d.cx, |v, _| v.error.clone()),
        _ => panic!("the dialog stays"),
    };
    assert!(why.is_some_and(|w| w.contains("being opened from a helper")));
    assert!(d.state().find("h-done").is_some());
}

/// A list of helpers from a core since replaced, late, says nothing of the
/// helpers its replacement holds: neither its event nor an answer asked of
/// it counts, and Delete waits for the new core's own.
#[gpui::test]
fn a_late_list_from_a_core_since_replaced_does_not_count(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |host| host.defer(cmd::BACKGROUND_HELPERS));
    let known = |d: &mut D| d.models.workspace.read_with(d.cx, |w, _| w.helpers_known());
    assert!(!known(&mut d), "the first core's list not in yet");
    new_core(&mut d, 2);
    // The first core's answer, asked of it before it went.
    assert!(d.host.resolve(cmd::BACKGROUND_HELPERS, Ok(json!([]))));
    d.settle();
    assert!(!known(&mut d), "asked of the core since replaced");
    let mut late = json!([]);
    crate::status::event::stamp(crate::host::event::HELPERS, &mut late, 1);
    assert!(d.mailbox.event(crate::host::event::HELPERS, late));
    d.settle();
    assert!(!known(&mut d), "sent by the core since replaced");
    let mut now = helpers_opening();
    crate::status::event::stamp(crate::host::event::HELPERS, &mut now, 2);
    assert!(d.mailbox.event(crate::host::event::HELPERS, now));
    d.settle();
    assert!(known(&mut d), "the new core's own");
    assert!(d.models.workspace.read_with(d.cx, |w, _| w.held_by_opening("h-done")));
}

/// A first list of helpers the core could not give is asked for again while
/// that core is up: a quiet core sends none of its own, and Delete would wait
/// on it for good.
#[gpui::test]
fn a_failed_first_list_of_helpers_is_asked_for_again(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |host| host.fail(cmd::BACKGROUND_HELPERS, Some("busy")));
    let known = |d: &mut D| d.models.workspace.read_with(d.cx, |w, _| w.helpers_known());
    assert!(!known(&mut d));
    d.host.fail(cmd::BACKGROUND_HELPERS, None);
    d.host.answer(cmd::BACKGROUND_HELPERS, json!([]));
    d.cx.executor().advance_clock(crate::workspace::HELPERS_RETRY_FIRST);
    d.settle();
    assert!(known(&mut d), "asked again");
}

/// A first list the core never answers, connected all the while, counts as
/// failed past `HELPERS_ASK` and is asked for again.
#[gpui::test]
fn an_unanswered_first_list_of_helpers_is_asked_for_again(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |host| host.defer(cmd::BACKGROUND_HELPERS));
    let asked = |d: &mut D| d.calls(cmd::BACKGROUND_HELPERS).len();
    let first = asked(&mut d);
    d.cx.executor().advance_clock(crate::workspace::HELPERS_ASK + crate::workspace::HELPERS_RETRY_FIRST);
    d.settle();
    assert!(asked(&mut d) > first, "asked again");
    assert!(d.host.resolve_newest(cmd::BACKGROUND_HELPERS, Ok(json!([]))));
    d.settle();
    assert!(d.models.workspace.read_with(d.cx, |w, _| w.helpers_known()));
}

/// With the core down, Delete says so, in the dialog as it opens and on the
/// click, and asks nothing of the host; once it is back, the words go.
#[cfg(debug_assertions)]
#[gpui::test]
fn delete_says_when_the_core_is_down(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |_| {});
    let down = CoreState { state: LinkState::Down, core: None, error: None, revision: 2 };
    assert!(d.mailbox.event(crate::host::event::CORE_STATE, serde_json::to_value(down).unwrap()));
    d.settle();
    let seam = d.models.workspace.read_with(d.cx, |w, _| w.seams.delete_chat.clone()).expect("the dialog plugs in");
    d.cx.update(|window, cx| seam("c-notes".into(), window, cx));
    d.settle();
    d.click("delete-yes");
    let error = |d: &mut D| match d.dialog() {
        Some(Dialog::Delete(v)) => v.read_with(d.cx, |v, _| v.error.clone()),
        _ => panic!("the dialog stays"),
    };
    assert_eq!(error(&mut d).as_deref(), Some(super::delete::NO_CORE));
    assert!(d.calls(cmd::DELETE_SESSION).is_empty());
    assert!(d.state().find("c-notes").is_some());
    new_core(&mut d, 3);
    assert_eq!(error(&mut d), None, "the core is back");
}

/// The core refusing a row's session delete, the id a helper it still holds
/// for an opening, the row is listed again where it stood: gone, the
/// opening could never be settled to it.
#[cfg(debug_assertions)]
#[gpui::test]
fn a_row_whose_delete_the_core_refuses_is_kept(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |host| {
        host.answer(cmd::BACKGROUND_HELPERS, json!([]));
        host.fail(cmd::DELETE_SESSION, Some(&format!("c-notes {}; it can be deleted once it has opened", super::rows::HELD_AS_HELPER)));
    });
    let before = d.state().listed("c-notes").expect("listed");
    let seam = d.models.workspace.read_with(d.cx, |w, _| w.seams.delete_chat.clone()).expect("the dialog plugs in");
    d.cx.update(|window, cx| seam("c-notes".into(), window, cx));
    d.settle();
    d.click("delete-yes");
    d.settle();
    assert_eq!(d.calls(cmd::DELETE_SESSION), vec![json!({ "id": "c-notes" })]);
    assert_eq!(d.state().listed("c-notes"), Some(before), "back where it stood");
}

/// A delete is work an update's last write waits for until the core has
/// answered: an update that freezes the list meanwhile writes the row the
/// core's refusal puts back, not its removal. One the core takes past
/// `DELETE_HELD` to answer holds the update no longer.
#[cfg(debug_assertions)]
#[gpui::test]
fn an_update_waits_for_the_cores_word_on_a_delete(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |host| {
        host.answer(cmd::BACKGROUND_HELPERS, json!([]));
        host.defer(cmd::DELETE_SESSION);
    });
    let admitted = |d: &mut D| d.models.workspace.read_with(d.cx, |w, _| w.admitted_for_tests());
    let delete = |d: &mut D, id: &str| {
        let seam = d.models.workspace.read_with(d.cx, |w, _| w.seams.delete_chat.clone()).expect("the dialog plugs in");
        let id = id.to_owned();
        d.cx.update(|window, cx| seam(id, window, cx));
        d.settle();
        d.click("delete-yes");
        d.settle();
    };
    let before = d.state().listed("c-notes").expect("listed");
    delete(&mut d, "c-notes");
    assert_eq!(admitted(&mut d), 1, "held until the core answers");
    d.models.workspace.update(d.cx, |w, cx| w.freeze(cx));
    let refusal = format!("c-notes {}; it can be deleted once it has opened", super::rows::HELD_AS_HELPER);
    assert!(d.host.resolve(cmd::DELETE_SESSION, Err(refusal)));
    d.settle();
    assert_eq!(admitted(&mut d), 0);
    assert_eq!(d.state().listed("c-notes"), Some(before), "back, frozen or not");
    d.models.workspace.update(d.cx, |w, cx| w.thaw(cx));
    d.settle();
    let other = d.state().chats().map(|c| c.id.clone()).find(|id| id != "c-notes").expect("another row");
    delete(&mut d, &other);
    assert_eq!(admitted(&mut d), 1);
    d.cx.executor().advance_clock(super::delete::DELETE_HELD);
    d.settle();
    assert_eq!(admitted(&mut d), 0, "let go past DELETE_HELD");
}

/// A row taken off the list while its opening is tried again is never said
/// made: the core hears it was not, and the helper stays its agent's.
#[gpui::test]
fn a_row_gone_meanwhile_is_never_said_made(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |host| {
        promote_h_done(host);
        host.fail(cmd::BACKGROUND_PROMOTED, Some("the core went away"));
    });
    open_h_done(&mut d);
    d.models.workspace.update(d.cx, |w, cx| w.unlist_promoted("h-done", cx));
    d.host.fail(cmd::BACKGROUND_PROMOTED, None);
    d.host.answer(cmd::BACKGROUND_PROMOTED, json!(false));
    d.cx.executor().advance_clock(std::time::Duration::from_secs(1));
    d.settle();
    let told = d.calls(cmd::BACKGROUND_PROMOTED);
    let last = told.last().unwrap();
    assert!(last["error"].as_str().is_some_and(|e| e.contains("taken off the list")), "{told:?}");
}

/// A core that came up while an update froze the list is settled once the
/// update is called off.
#[gpui::test]
fn openings_held_off_by_an_update_are_settled_when_it_is_called_off(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |host| {
        host.answer(crate::cmd::WINDOW_STATE, with_promoted_row());
        host.answer(cmd::BACKGROUND_PROMOTED, json!(true));
    });
    let before = d.calls(cmd::BACKGROUND_PROMOTED).len();
    d.models.workspace.update(d.cx, |w, cx| w.freeze(cx));
    d.host.answer(cmd::BACKGROUND_HELPERS, helpers_opening());
    new_core(&mut d, 2);
    assert_eq!(d.calls(cmd::BACKGROUND_PROMOTED).len(), before, "nothing while frozen");
    d.models.workspace.update(d.cx, |w, cx| w.thaw(cx));
    d.settle();
    assert!(d.calls(cmd::BACKGROUND_PROMOTED).contains(&json!({ "id": "h-done", "error": null })));
    assert!(d.state().shown().contains(&"h-done".to_owned()));
}

/// A row every word of whose opening went unanswered, the core up all the
/// while, is settled by the next list of helpers, not left until a restart.
#[gpui::test]
fn an_unsettled_row_is_settled_by_a_later_list(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |host| {
        promote_h_done(host);
        host.fail(cmd::BACKGROUND_PROMOTED, Some("busy"));
    });
    open_h_done(&mut d);
    d.escape();
    d.cx.executor().advance_clock(std::time::Duration::from_secs(130));
    d.settle();
    assert!(!d.state().shown().contains(&"h-done".to_owned()), "not yet");
    d.host.fail(cmd::BACKGROUND_PROMOTED, None);
    d.host.answer(cmd::BACKGROUND_PROMOTED, json!(true));
    assert!(d.mailbox.event(crate::host::event::HELPERS, helpers_opening()));
    d.settle();
    assert!(d.state().shown().contains(&"h-done".to_owned()));
}

/// Past VIEW_KEEP the oldest messages go; a reader away from the end keeps
/// the text they read where it was.
#[gpui::test]
fn a_watch_keeps_its_reader_in_place_as_old_messages_go(cx: &mut TestAppContext) {
    let msgs = |from: usize, n: usize| -> Vec<Value> {
        (from..from + n).map(|i| json!({ "role": "assistant", "text": format!("line {i}") })).collect()
    };
    let first = msgs(0, super::helpers::VIEW_KEEP);
    let mut d = open_as(cx, |host| {
        host.answer(cmd::BACKGROUND_TRANSCRIPT, json!({ "messages": first, "cursor": 1, "file": "h.jsonl", "running": true }));
    });
    d.cx.update(|window, cx| super::open_helpers(Some("c-parser"), window, cx));
    d.settle();
    d.drive("helpers", "watch h-review");
    let view = match d.dialog() {
        Some(Dialog::Helpers(v)) => v,
        _ => panic!("no helpers popover"),
    };
    let scroll = view.read_with(d.cx, |v, _| v.watch_scroll()).expect("watching");
    d.host.answer(cmd::BACKGROUND_TRANSCRIPT, json!({ "messages": [], "cursor": 1, "file": "h.jsonl", "running": true }));
    d.cx.executor().advance_clock(super::helpers::TAIL_RUNNING);
    d.settle();
    let max = f32::from(scroll.max_offset().y);
    scroll.set_offset(gpui::point(gpui::px(0.), gpui::px(-max / 2.)));
    d.cx.update(|window, _| window.refresh());
    d.settle();
    let top = |i: usize| f32::from(scroll.bounds_for_item(i).expect("laid out").origin.y);
    let gone = top(20) - top(0);
    assert!(gone > 0.);
    let before = f32::from(scroll.offset().y);
    d.host.answer(cmd::BACKGROUND_TRANSCRIPT, json!({ "messages": msgs(1000, 20), "cursor": 2, "file": "h.jsonl", "running": true }));
    d.cx.executor().advance_clock(super::helpers::TAIL_RUNNING);
    d.settle();
    let after = f32::from(scroll.offset().y);
    assert!((after - (before + gone)).abs() < 1., "moved up by what went: {before} + {gone}, now {after}");
}

/// A core silent at a word of an opening, `at`, holds an update's last write
/// no longer than `OPENING_HELD`, and is told the opening failed.
fn silent_at(cx: &mut TestAppContext, at: &str, first: impl FnOnce(&FakeHost)) {
    let mut d = open_as(cx, first);
    open_h_done(&mut d);
    d.escape();
    let admitted = |d: &mut D| d.models.workspace.read_with(d.cx, |w, _| w.admitted_for_tests());
    assert_eq!(admitted(&mut d), 1, "{at}");
    d.cx.executor().advance_clock(super::helpers::OPENING_HELD);
    d.settle();
    assert_eq!(admitted(&mut d), 0, "{at}: let go within OPENING_HELD");
    let told = d.calls(cmd::BACKGROUND_PROMOTED);
    let opening = d.calls(cmd::BACKGROUND_PROMOTE)[0]["opening"].clone();
    assert!(opening.is_string(), "{at}: the begin names its opening");
    assert!(
        told.iter().any(|w| w["error"].is_string() && w["opening"] == opening),
        "{at}: the core is told it failed, of which opening: {told:?}"
    );
    assert!(d.state().find("h-done").is_none(), "{at}: no row");
}

/// The first word, the helper asked for: unanswered, the core may hold it by
/// now, and hears the opening failed.
#[gpui::test]
fn a_core_silent_at_the_first_word_lets_an_update_go_in_time(cx: &mut TestAppContext) {
    silent_at(cx, "the helper asked for", |host| host.defer(cmd::BACKGROUND_PROMOTE));
}

#[gpui::test]
fn a_core_silent_at_the_accounts_defaults_lets_an_update_go_in_time(cx: &mut TestAppContext) {
    silent_at(cx, "the account's defaults", |host| {
        promote_h_done(host);
        host.defer(cmd::CODEX_DEFAULTS);
    });
}

/// The word that the row could not be saved, which comes last.
#[gpui::test]
fn a_core_silent_at_the_word_that_it_failed_lets_an_update_go_in_time(cx: &mut TestAppContext) {
    silent_at(cx, "the word that it failed", |host| {
        promote_h_done(host);
        host.fail(crate::cmd::FLUSH_WINDOW_STATE, Some("disk full"));
        host.defer(cmd::BACKGROUND_PROMOTED);
    });
}

/// A core that never answers holds an update's last write no longer than
/// the held tries take, each word bounded (`SETTLE_CALL`).
#[gpui::test]
fn a_silent_core_does_not_hold_an_update_past_its_limit(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |host| {
        promote_h_done(host);
        host.defer(cmd::BACKGROUND_PROMOTED);
    });
    open_h_done(&mut d);
    d.escape();
    let admitted = |d: &mut D| d.models.workspace.read_with(d.cx, |w, _| w.admitted_for_tests());
    assert_eq!(admitted(&mut d), 1);
    d.cx.executor().advance_clock(crate::workspace::SETTLE_LIMIT - std::time::Duration::from_secs(1));
    d.settle();
    assert_eq!(admitted(&mut d), 0, "let go within SETTLE_LIMIT");
}

/// A settling begun as the window starts is work an update's last write
/// waits for, as a click's opening is.
#[gpui::test]
fn a_settling_under_way_holds_the_updates_last_write(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |host| {
        host.answer(crate::cmd::WINDOW_STATE, with_promoted_row());
        host.answer(cmd::BACKGROUND_HELPERS, helpers_opening());
        host.defer(cmd::BACKGROUND_PROMOTED);
    });
    let admitted = |d: &mut D| d.models.workspace.read_with(d.cx, |w, _| w.admitted_for_tests());
    assert_eq!(admitted(&mut d), 2, "h-done's word and h-half's");
    assert!(d.host.resolve(cmd::BACKGROUND_PROMOTED, Ok(json!(true))));
    assert!(d.host.resolve(cmd::BACKGROUND_PROMOTED, Ok(json!(false))));
    d.settle();
    assert_eq!(admitted(&mut d), 0);
    assert!(d.state().shown().contains(&"h-done".to_owned()));
}

/// With the core quiet, the helpers popover's own looks settle what is
/// left of an opening.
#[gpui::test]
fn the_popovers_looks_settle_what_is_left_of_an_opening(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |host| {
        host.answer(crate::cmd::WINDOW_STATE, with_promoted_row());
        host.answer(cmd::BACKGROUND_HELPERS, helpers_opening());
        host.fail(cmd::BACKGROUND_PROMOTED, Some("busy"));
    });
    d.cx.executor().advance_clock(std::time::Duration::from_secs(30));
    d.settle();
    assert!(!d.state().shown().contains(&"h-done".to_owned()), "the start's tries went unanswered");
    d.host.fail(cmd::BACKGROUND_PROMOTED, None);
    d.host.answer(cmd::BACKGROUND_PROMOTED, json!(true));
    d.cx.update(|window, cx| super::open_helpers(Some("c-parser"), window, cx));
    d.settle();
    assert!(d.state().shown().contains(&"h-done".to_owned()));
}

/// Until the core has listed its helpers, no chat is deleted: a row a window
/// gone left of an opening looks like any chat until then.
#[cfg(debug_assertions)]
#[gpui::test]
fn no_chat_is_deleted_before_the_helpers_are_listed(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |host| host.defer(cmd::BACKGROUND_HELPERS));
    let seam = d.models.workspace.read_with(d.cx, |w, _| w.seams.delete_chat.clone()).expect("the dialog plugs in");
    d.cx.update(|window, cx| seam("c-notes".into(), window, cx));
    d.settle();
    d.click("delete-yes");
    assert!(d.calls(cmd::DELETE_SESSION).is_empty());
    assert!(d.state().find("c-notes").is_some());
    while d.host.resolve(cmd::BACKGROUND_HELPERS, Ok(demo::helpers())) {}
    d.settle();
    d.click("delete-yes");
    assert_eq!(d.calls(cmd::DELETE_SESSION), vec![json!({ "id": "c-notes" })]);
}

#[cfg(debug_assertions)]
#[gpui::test]
fn a_run_that_did_not_end_by_itself_is_told_at_start(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |host| {
        host.answer(cmd::CRASH_REPORT, json!({ "reason": "a panic: index out of bounds at src/board.rs:212", "at": "2026-11-19 14:02:11", "log": "C:\\Luna\\luna.log" }));
    });
    assert_eq!(d.name(), Some("crash"));
    assert!(d.drawn("crash-reason") && d.drawn("crash-log"));
    d.click("crash-restart");
    assert_eq!(d.calls(cmd::RESTART_WINDOW).len(), 1);
    d.click("crash-continue");
    assert_eq!(d.name(), None);
}

#[gpui::test]
fn the_accounts_models_are_not_waited_for(cx: &mut TestAppContext) {
    // Asking the CLI for its models takes seconds: the dialog is usable on the aliases meanwhile.
    let mut d = open_as(cx, |host| host.defer(cmd::CLAUDE_MODELS));
    let v = d.new_chat();
    assert!(v.read_with(d.cx, |v, _| v.can_create()));
    assert_eq!(v.read_with(d.cx, |v, _| v.claude.settings.model.clone()), "Opus");
    // Haiku is among the aliases, and takes no effort once the list comes: Sonnet is not in it.
    assert!(d.host.resolve(
        cmd::CLAUDE_MODELS,
        Ok(json!([{ "slug": "opus", "displayName": "Opus", "efforts": ["low", "high"], "defaultEffort": "low" }]))
    ));
    d.settle();
    assert_eq!(v.read_with(d.cx, |v, _| v.claude.settings.effort.clone()), "high");
    v.update(d.cx, |v, cx| v.pick_effort("max", cx));
    assert_eq!(
        v.read_with(d.cx, |v, _| v.claude.settings.effort.clone()),
        "low",
        "held to what the account's own list says the model takes"
    );
}

/// While an update installs (the chat list frozen until it is installed or
/// called off), no chat is made, quick or not, none is deleted and no helper
/// is opened as one: each is refused before anything is asked of the host,
/// so nothing happens that the list's last write would not hold. Thawed, the
/// same clicks go through.
#[cfg(debug_assertions)]
#[gpui::test]
fn while_an_update_installs_no_chat_is_made_deleted_or_promoted(cx: &mut TestAppContext) {
    let mut d = open_as(cx, |host| {
        host.answer(
            cmd::BACKGROUND_PROMOTE,
            json!({ "id": "h-done", "provider": "codex", "accountPath": "C:\\accounts\\openai\\personal", "folder": "C:\\work\\parser", "worktreePath": null,
                    "sessionId": "s-1", "name": "write-tests", "settings": {}, "tools": true }),
        );
        host.answer(cmd::BACKGROUND_PROMOTED, json!(true));
    });
    let before = chat_ids(&d.state());
    d.models.workspace.update(d.cx, |w, cx| w.freeze(cx));
    d.settle();

    // The new-chat dialog: Create says why, and asks the host nothing.
    d.new_chat();
    let checks = d.calls(cmd::CHAT_FOLDER_CHECK).len();
    d.click("newchat-create");
    assert_eq!(d.name(), Some("new-chat"), "the dialog stays, with the reason");
    assert!(d.cx.debug_bounds("newchat-error").is_some());
    assert_eq!(d.calls(cmd::CHAT_FOLDER_CHECK).len(), checks);
    d.escape();
    // A quick chat: refused, it falls back to the dialog, nothing made.
    d.cx.dispatch_action(crate::keys::QuickChat);
    d.settle();
    d.escape();
    assert_eq!(chat_ids(&d.state()), before);

    // Delete: refused before the session is stopped.
    let seam = d.models.workspace.read_with(d.cx, |w, _| w.seams.delete_chat.clone()).expect("the dialog plugs in");
    d.cx.update(|window, cx| seam("c-notes".into(), window, cx));
    d.settle();
    d.click("delete-yes");
    assert!(d.calls(cmd::DELETE_SESSION).is_empty());
    assert!(d.state().find("c-notes").is_some());
    d.escape();

    // A helper opened as a chat: not taken from its agent.
    d.cx.update(|window, cx| super::open_helpers(Some("c-parser"), window, cx));
    d.settle();
    d.drive("helpers", "tab idle");
    d.click("helper-open-h-done");
    d.click("helpers-ask-yes");
    assert!(d.calls(cmd::BACKGROUND_PROMOTE).is_empty());
    assert!(d.state().find("h-done").is_none());
    d.escape();

    // Called off: the list takes changes again.
    d.models.workspace.update(d.cx, |w, cx| w.thaw(cx));
    d.settle();
    d.cx.update(|window, cx| seam("c-notes".into(), window, cx));
    d.settle();
    d.click("delete-yes");
    assert_eq!(d.calls(cmd::DELETE_SESSION), vec![json!({ "id": "c-notes" })]);
    assert!(d.state().find("c-notes").is_none());
}
