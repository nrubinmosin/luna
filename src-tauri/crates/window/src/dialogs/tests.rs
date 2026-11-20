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
use std::sync::Arc;

struct D<'a> {
    host: Arc<FakeHost>,
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
    let (_mailbox, inbox) = mailbox();
    let models = cx.update(|cx| crate::start(host.clone(), inbox, ThemePref::Light, cx));
    let window_models = models.clone();
    let (view, cx) = cx.add_window_view(move |window, cx| LunaWindow::new(window_models, window, cx));
    cx.run_until_parked();
    D { host, models, view, cx }
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
    assert_eq!(
        (s.model.as_deref(), s.effort.as_str(), s.approval.as_str(), s.sandbox.as_str()),
        (Some("gpt-6"), "medium", "on-request", "workspace-write")
    );
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
    assert_eq!(d.calls(cmd::BACKGROUND_PROMOTE), vec![json!({ "id": "h-done" })]);
    let state = d.state();
    let chat = state.find("h-done").expect("a row with the helper's own id").clone();
    assert_eq!((chat.provider, chat.account.as_str()), (Provider::Codex, "personal"));
    assert_eq!(chat.name, "write-tests");
    assert_eq!(chat.session_id.as_deref(), Some("s-1"));
    assert!(chat.tools);
    // What it ran with, over what its account opens on.
    assert_eq!(
        (chat.model.as_deref(), chat.approval.as_deref(), chat.sandbox.as_deref()),
        (Some("gpt-6-mini"), Some("never"), Some("workspace-write"))
    );
    // The core is told the row is made, and then it is seated.
    assert_eq!(d.calls(cmd::BACKGROUND_PROMOTED), vec![json!({ "id": "h-done", "error": null })]);
    assert!(state.shown().contains(&"h-done".to_owned()));
    // A helper that is not a person's own is never remembered as their last choice.
    assert_ne!(state.new_chat.last_account.as_ref().map(|a| a.name.as_str()), Some("personal"));
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
