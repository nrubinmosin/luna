//! The dialogs over the window: new chat, quick chat, delete chat, the
//! helpers popover and the crash screen. One is up at a time, over a scrim
//! that keeps the window below from being clicked; Esc or its close button
//! closes it, Enter does what its main button does. They plug into the
//! window through `Workspace::seams` (the New chat button and Ctrl+N, Ctrl+
//! Shift+N, a chat's Delete), the status bar's Background count through
//! `open_helpers`, and the crash screen opens itself at start when the host
//! says the run before did not end by itself. Each asks the host for what the
//! page's dialog asks for, by the same commands (`cmd`).
//!
//! The layer starts below the title bar, as the panels' does, so the window's
//! own buttons stay in reach.

mod choice;
mod crash;
mod create;
mod delete;
#[cfg(any(test, feature = "shots"))]
pub mod demo;
mod helpers;
mod new_chat;
pub mod rows;

#[cfg(test)]
mod tests;

pub use crash::Report;

use crate::models::Models;
use crate::panels::ask;
use crate::scale::u;
use crate::selector::Selector;
use crash::CrashView;
use delete::DeleteChat;
use gpui::{
    AnyView, App, AppContext, Context, Entity, FocusHandle, InteractiveElement, IntoElement, KeyBinding, ParentElement, Render, Styled,
    Window, actions, div,
};
use helpers::HelpersView;
use new_chat::NewChat;
use serde_json::json;
use std::rc::Rc;

/// The commands the dialogs name, beside `cmd::ALL` and the panels' (a test
/// in luna_lib holds the host's dispatch to these too). Their arguments and
/// answers are the page's (`src/ipc/defaults.ts`, `trust.ts`, `sessions.ts`,
/// `worktrees.ts`, `agents.ts`), but for `crash_report` and `restart_window`,
/// which are the native window's own.
pub mod cmd {
    pub const CLAUDE_DEFAULTS: &str = "claude_defaults";
    pub const CLAUDE_MODELS: &str = "claude_models";
    pub const CODEX_DEFAULTS: &str = "codex_defaults";
    pub const CODEX_MODELS: &str = "codex_models";
    pub const CHAT_FOLDER_CHECK: &str = "chat_folder_check";
    pub const CLAUDE_FOLDER_TRUSTED: &str = "claude_folder_trusted";
    pub const CODEX_FOLDER_TRUSTED: &str = "codex_folder_trusted";
    pub const CLAUDE_TRUST_FOLDER: &str = "claude_trust_folder";
    pub const CODEX_TRUST_FOLDER: &str = "codex_trust_folder";
    pub const DELETE_SESSION: &str = "delete_session";
    pub const BACKGROUND_HELPERS: &str = "background_helpers";
    pub const BACKGROUND_AT_STAKE: &str = "background_at_stake";
    pub const BACKGROUND_DELETE: &str = "background_delete";
    pub const BACKGROUND_KILL: &str = "background_kill";
    pub const BACKGROUND_PROMOTE: &str = "background_promote";
    pub const BACKGROUND_PROMOTED: &str = "background_promoted";
    pub const BACKGROUND_TRANSCRIPT: &str = "background_transcript";
    /// `{}`: what the record of the run before says of how it ended, or null
    /// when it ended by itself: `{ reason, at, log }`.
    pub const CRASH_REPORT: &str = "crash_report";
    /// `{}`: a new window of this kind, the core left alone (the tray's
    /// Restart window).
    pub const RESTART_WINDOW: &str = "restart_window";

    pub const CALLED: &[&str] = &[
        CLAUDE_DEFAULTS,
        CLAUDE_MODELS,
        CODEX_DEFAULTS,
        CODEX_MODELS,
        CHAT_FOLDER_CHECK,
        CLAUDE_FOLDER_TRUSTED,
        CODEX_FOLDER_TRUSTED,
        CLAUDE_TRUST_FOLDER,
        CODEX_TRUST_FOLDER,
        DELETE_SESSION,
        BACKGROUND_HELPERS,
        BACKGROUND_AT_STAKE,
        BACKGROUND_DELETE,
        BACKGROUND_KILL,
        BACKGROUND_PROMOTE,
        BACKGROUND_PROMOTED,
        BACKGROUND_TRANSCRIPT,
        CRASH_REPORT,
        RESTART_WINDOW,
    ];
}

actions!(luna_dialog, [Close, Confirm]);

/// The key context every dialog's frame sets.
pub(crate) const CONTEXT: &str = "LunaDialog";

/// Esc closes the dialog up; Enter does what its main button does.
pub(crate) fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("escape", Close, Some(CONTEXT)), KeyBinding::new("enter", Confirm, Some(CONTEXT))]);
}

/// The dialog up, its view.
#[derive(Clone)]
pub(crate) enum Dialog {
    NewChat(Entity<NewChat>),
    Delete(Entity<DeleteChat>),
    Helpers(Entity<HelpersView>),
    Crash(Entity<CrashView>),
}

impl Dialog {
    fn view(&self) -> AnyView {
        match self {
            Dialog::NewChat(v) => v.clone().into(),
            Dialog::Delete(v) => v.clone().into(),
            Dialog::Helpers(v) => v.clone().into(),
            Dialog::Crash(v) => v.clone().into(),
        }
    }

    pub(crate) fn name(&self) -> &'static str {
        match self {
            Dialog::NewChat(_) => "new-chat",
            Dialog::Delete(_) => "delete",
            Dialog::Helpers(_) => "helpers",
            Dialog::Crash(_) => "crash",
        }
    }
}

/// The layer the dialogs sit on, over the whole window (root.rs): nothing at
/// all while none is up.
pub struct DialogLayer {
    pub(crate) models: Models,
    pub(crate) open: Option<Dialog>,
    focus: FocusHandle,
}

/// The window's layer, for whoever opens a dialog from outside its views.
struct Layer(gpui::WeakEntity<DialogLayer>);

impl gpui::Global for Layer {}

impl DialogLayer {
    /// Plugs the dialogs into the window's seams, and asks the host whether
    /// the run before crashed.
    pub fn new(models: Models, window: &mut Window, cx: &mut Context<Self>) -> DialogLayer {
        cx.observe(&models.shell, |_, _, cx| cx.notify()).detach();
        let me = cx.weak_entity();
        cx.set_global(Layer(me.clone()));
        let (new, quick, delete, helpers) = (me.clone(), me.clone(), me.clone(), me);
        models.workspace.update(cx, |w, _| {
            w.seams.new_chat = Some(Rc::new(move |window, cx| {
                let _ = new.update(cx, |l, cx| l.open_new_chat(None, window, cx));
            }));
            w.seams.quick_chat = Some(Rc::new(move |window, cx| {
                // Read out first: the quick chat may open the dialog itself,
                // and the layer must not be borrowed then.
                let Ok(models) = quick.read_with(cx, |l, _| l.models.clone()) else { return };
                create::quick_chat(&models, window, cx);
            }));
            w.seams.delete_chat = Some(Rc::new(move |id, window, cx| {
                let _ = delete.update(cx, |l, cx| l.open_delete(&id, window, cx));
            }));
            // The status bar's Background count: every chat's helpers.
            w.seams.helpers = Some(Rc::new(move |window, cx| {
                let _ = helpers.update(cx, |l, cx| l.open_helpers(None, window, cx));
            }));
        });
        let asked = ask::<Option<Report>>(&models.host, cmd::CRASH_REPORT, json!({}));
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Some(report)) = asked.await {
                let _ = this.update_in(cx, |l, window, cx| l.open_crash(report, window, cx));
            }
        })
        .detach();
        DialogLayer { models, open: None, focus: cx.focus_handle() }
    }

    pub fn shown(&self) -> Option<&'static str> {
        self.open.as_ref().map(Dialog::name)
    }

    fn show(&mut self, dialog: Dialog, window: &mut Window, cx: &mut Context<Self>) {
        self.open = Some(dialog);
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// The new-chat dialog, on `folder` where it was asked for one.
    pub fn open_new_chat(&mut self, folder: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let (models, layer) = (self.models.clone(), cx.weak_entity());
        let dialog = cx.new(|cx| NewChat::new(models, layer, folder, cx));
        self.show(Dialog::NewChat(dialog), window, cx);
    }

    /// The question before a chat is deleted; nothing for a chat that is not
    /// listed.
    pub fn open_delete(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chat) = self.models.workspace.read(cx).state.find(id).cloned() else { return };
        let (models, layer) = (self.models.clone(), cx.weak_entity());
        let dialog = cx.new(|cx| DeleteChat::new(models, layer, chat, cx));
        self.show(Dialog::Delete(dialog), window, cx);
    }

    /// The helpers popover: `chat`'s helpers (its helpers' helpers with them),
    /// or all of them.
    pub fn open_helpers(&mut self, chat: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let (models, layer) = (self.models.clone(), cx.weak_entity());
        let dialog = cx.new(|cx| HelpersView::new(models, layer, chat, cx));
        self.show(Dialog::Helpers(dialog), window, cx);
    }

    pub fn open_crash(&mut self, report: Report, window: &mut Window, cx: &mut Context<Self>) {
        let (models, layer) = (self.models.clone(), cx.weak_entity());
        let dialog = cx.new(|_| CrashView::new(models, layer, report));
        self.show(Dialog::Crash(dialog), window, cx);
    }

    /// Takes the dialog down, at once: what asks first has asked already. The
    /// keyboard goes back to the terminal it belongs in.
    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open.take().is_some() {
            window.focus(&self.models.home, cx);
            self.models.workspace.update(cx, |w, cx| {
                w.focus_home();
                cx.notify();
            });
            cx.notify();
        }
    }

    /// Esc: the dialog says whether it may go (a list open, a chat being made).
    fn ask_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let go = match &self.open {
            Some(Dialog::NewChat(v)) => v.update(cx, |v, cx| v.may_close(cx)),
            Some(Dialog::Delete(v)) => v.update(cx, |v, cx| v.may_close(cx)),
            Some(Dialog::Helpers(v)) => v.update(cx, |v, cx| v.may_close(cx)),
            Some(Dialog::Crash(_)) => true,
            None => false,
        };
        if go {
            self.close(window, cx);
        }
    }

    /// Enter.
    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.open.clone() {
            Some(Dialog::NewChat(v)) => v.update(cx, |v, cx| v.create(window, cx)),
            Some(Dialog::Delete(v)) => v.update(cx, |v, cx| v.confirm(window, cx)),
            _ => {}
        }
    }
}

/// The helpers popover, for the status bar's Background count: `chat`'s
/// helpers, or all of them. Opens in the window whose layer this is.
pub fn open_helpers(chat: Option<&str>, window: &mut Window, cx: &mut App) {
    if let Some(layer) = cx.try_global::<Layer>().and_then(|l| l.0.upgrade()) {
        let chat = chat.map(str::to_owned);
        layer.update(cx, |l, cx| l.open_helpers(chat, window, cx));
    }
}

/// The new-chat dialog, on `folder` where one is asked for.
pub(crate) fn open_new_chat(folder: Option<String>, window: &mut Window, cx: &mut App) {
    if let Some(layer) = cx.try_global::<Layer>().and_then(|l| l.0.upgrade()) {
        layer.update(cx, |l, cx| l.open_new_chat(folder, window, cx));
    }
}

impl Render for DialogLayer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(open) = &self.open else { return div().id("dialogs") };
        let p = self.models.shell.read(cx).palette();
        let popover = matches!(open, Dialog::Helpers(_));
        let layer = div()
            .id("dialogs")
            .selector(|| "dialogs".into())
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &Close, window, cx| this.ask_close(window, cx)))
            .on_action(cx.listener(|this, _: &Confirm, window, cx| this.confirm(window, cx)))
            .absolute()
            .top(crate::title_bar::height())
            .left_0()
            .right_0()
            .bottom_0()
            .flex()
            .p(u(16.))
            // Nothing under it takes a click or a hover while a dialog is up.
            .occlude();
        // The popover hangs where the status bar's count is, bottom right,
        // over a scrim that hardly shows; a dialog is in the middle.
        if popover {
            layer.items_end().justify_end().bg(p.frame.opacity(0.12)).child(open.view())
        } else {
            layer.items_center().justify_center().bg(p.frame.opacity(0.45)).child(open.view())
        }
    }
}

/// What is up, for the driver's `dialogs` line.
#[cfg(luna_test)]
pub(crate) fn describe(cx: &App) -> String {
    let Some(layer) = cx.try_global::<Layer>().and_then(|l| l.0.upgrade()) else { return "no dialogs".into() };
    match &layer.read(cx).open {
        None => "no dialog up".into(),
        Some(Dialog::NewChat(v)) => {
            let v = v.read(cx);
            format!(
                "new-chat up, folder {:?}, account {:?}, worktree {}, tools {}, can create {}, creating {}, error {:?}",
                v.folder,
                v.account.as_ref().map(|a| &a.name),
                v.worktree,
                v.tools,
                v.can_create(),
                v.creating,
                v.error
            )
        }
        Some(d) => format!("{} up", d.name()),
    }
}

/// The driver's lines for the dialogs (drive.rs, and the shots' scenes):
/// `dialog new-chat [folder]|quick|delete <n>|helpers [<n>]|crash|none` (the
/// nth chat of the list, as `row` counts), `newchat folder <path>|account
/// <n>|worktree|tools|trust|model <label>|effort <level>|menu account|menu model|create`,
/// `deletechat helpers|stop|yes`, `helpers tab
/// working|idle`, `helpers watch|stop|delete|open|yes <id>`: each doing what
/// its click does. True when the line was one of these.
#[cfg(any(test, luna_test, feature = "shots"))]
pub(crate) fn drive(verb: &str, rest: &str, window: &mut Window, cx: &mut App) -> bool {
    let Some(layer) = cx.try_global::<Layer>().and_then(|l| l.0.upgrade()) else { return false };
    let open = layer.read(cx).open.clone();
    match (verb, &open) {
        ("dialog", _) => {
            let (what, arg) = rest.split_once(' ').map_or((rest, ""), |(a, b)| (a, b.trim()));
            match what {
                "new-chat" => layer.update(cx, |l, cx| l.open_new_chat((!arg.is_empty()).then(|| arg.to_owned()), window, cx)),
                "quick" => {
                    let models = layer.read(cx).models.clone();
                    create::quick_chat(&models, window, cx);
                }
                "delete" => {
                    let ws = layer.read(cx).models.workspace.clone();
                    let id = {
                        let ws = ws.read(cx);
                        arg.parse::<usize>()
                            .ok()
                            .and_then(|n| ws.state.numbered(ws.state.board).get(n.wrapping_sub(1)).cloned())
                            .unwrap_or_else(|| arg.to_owned())
                    };
                    layer.update(cx, |l, cx| l.open_delete(&id, window, cx));
                }
                "helpers" => {
                    let ws = layer.read(cx).models.workspace.clone();
                    let chat = {
                        let ws = ws.read(cx);
                        arg.parse::<usize>()
                            .ok()
                            .and_then(|n| ws.state.numbered(ws.state.board).get(n.wrapping_sub(1)).cloned())
                            .or_else(|| (!arg.is_empty()).then(|| arg.to_owned()))
                    };
                    layer.update(cx, |l, cx| l.open_helpers(chat, window, cx));
                }
                "crash" => layer.update(cx, |l, cx| {
                    l.open_crash(
                        Report {
                            reason: "a panic on the main thread: index out of bounds at src/board.rs:212".into(),
                            at: "14:02".into(),
                            log: "C:\\Luna\\luna.log".into(),
                        },
                        window,
                        cx,
                    )
                }),
                _ => layer.update(cx, |l, cx| l.close(window, cx)),
            }
            true
        }
        ("newchat", Some(Dialog::NewChat(v))) => {
            let (what, arg) = rest.split_once(' ').map_or((rest, ""), |(a, b)| (a, b.trim()));
            v.update(cx, |v, cx| match what {
                "folder" => v.picked(arg, cx),
                "account" => {
                    let account = arg.parse::<usize>().ok().and_then(|n| v.accounts(cx).get(n.wrapping_sub(1)).cloned());
                    if let Some(a) = account {
                        v.choose_account(a, cx);
                    }
                }
                "worktree" => {
                    v.worktree = !v.worktree;
                    cx.notify();
                }
                "tools" => {
                    v.tools = !v.tools;
                    cx.notify();
                }
                "trust" => v.tick_trust(cx),
                "model" => match v.account.as_ref().map(|a| a.provider) {
                    Some(crate::term::paste::Provider::Codex) => v.pick_codex_model(Some(arg.to_owned()), cx),
                    _ => v.pick_claude_model(arg, cx),
                },
                "effort" => v.pick_effort(arg, cx),
                "menu" => {
                    v.menu = ["account", "model"].into_iter().find(|m| *m == arg);
                    cx.notify();
                }
                "create" => v.create(window, cx),
                _ => log::warn!("drive: newchat {what}: not understood"),
            });
            true
        }
        ("deletechat", Some(Dialog::Delete(v))) => {
            v.update(cx, |v, cx| match rest {
                "helpers" => v.choices.with_helpers = !v.choices.with_helpers,
                "stop" => v.choices.stop_working = !v.choices.stop_working,
                "yes" => v.confirm(window, cx),
                _ => log::warn!("drive: deletechat {rest}: not understood"),
            });
            v.update(cx, |_, cx| cx.notify());
            true
        }
        ("helpers", Some(Dialog::Helpers(v))) => {
            let (what, id) = rest.split_once(' ').map_or((rest, ""), |(a, b)| (a, b.trim()));
            v.update(cx, |v, cx| v.drive(what, id, cx));
            true
        }
        _ => false,
    }
}
