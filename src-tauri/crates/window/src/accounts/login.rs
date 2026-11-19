//! Signing an account in: the account's CLI login in a terminal, over the
//! accounts window, as the page's LoginModal.tsx has it. The session is the
//! CLI's own login screen run in the account's folder, so the credentials
//! land there: a bare `claude` (which walks a fresh folder through its login),
//! or `codex login` (`login: true`). Its id is the account's, so one account
//! has one login session at a time.
//!
//! It closes when the CLI exits, unless the exit says it failed (a code that
//! is not 0): the terminal then keeps what the CLI said until the person
//! closes the window. Closing it ends the session. The session a closed
//! window ends is still on its way out for a moment, and a window opened on
//! the same account in that time would attach to the very process being
//! killed: it waits for the end (`Ending`) before it starts its own.

use super::Account;
use crate::host::{Host, cmd};
use crate::models::Models;
use crate::panels::Provider;
use crate::panels::ui::{self, OnClick};
use crate::term::{Exited, Phase, SessionSpec, TermPane};
use gpui::{
    App, AppContext, BorrowAppContext, Context, Entity, EventEmitter, Global, InteractiveElement, IntoElement, ParentElement, Render,
    Styled, Subscription, Window, div, px,
};
use serde_json::json;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

/// How often a closed window's pane is looked at while its start is still
/// being answered.
const LOOK: Duration = Duration::from_millis(100);

/// The login sessions a closed window is still ending, by id.
#[derive(Default)]
struct Ending(HashSet<String>);

impl Global for Ending {}

/// The session's id: the account's, as the page names it.
pub fn session_id(a: &Account) -> String {
    format!("login:{}:{}", a.provider.key(), a.name)
}

/// The login session of `a`: the page's `loginSession`.
pub fn spec(a: &Account) -> SessionSpec {
    let id = session_id(a);
    match a.provider {
        Provider::Claude => SessionSpec {
            id: id.clone(),
            provider: a.provider.into(),
            start: cmd::ENSURE_CLAUDE_SESSION,
            args: json!({
                "id": id,
                "folder": a.path,
                "accountPath": a.path,
                "model": "sonnet",
                "effort": "medium",
                "permissionMode": "default",
                "worktree": false,
                "worktreeName": null,
                "resume": null,
                "tools": false,
                "readOnly": false,
                "agent": null,
                "guard": false,
                "handoff": null,
            }),
        },
        Provider::Codex => SessionSpec {
            id: id.clone(),
            provider: a.provider.into(),
            start: cmd::ENSURE_CODEX_SESSION,
            args: json!({
                "id": id,
                "folder": a.path,
                "accountPath": a.path,
                "model": null,
                "effort": "medium",
                "approval": "on-request",
                "sandbox": "read-only",
                "worktree": false,
                "resume": null,
                "login": true,
                "tools": false,
                "hooksUnasked": false,
                "guard": false,
                "handoff": null,
            }),
        },
    }
}

/// The window is done: the CLI exited, or it was closed.
pub struct Closed;

pub struct LoginWindow {
    models: Models,
    account: Account,
    /// None while it waits for the last session of this account to end.
    pane: Option<Entity<TermPane>>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<Closed> for LoginWindow {}

impl LoginWindow {
    pub fn new(models: Models, account: Account, window: &mut Window, cx: &mut Context<Self>) -> LoginWindow {
        let subs = vec![
            cx.observe(&models.shell, |_, _, cx| cx.notify()),
            cx.observe_global_in::<Ending>(window, |this, window, cx| this.begin(window, cx)),
            // The panel under it let go (another panel opened, the layer
            // closed): the session still ends.
            cx.on_release(|this, cx| this.end(cx)),
        ];
        let mut login = LoginWindow { models, account, pane: None, _subs: subs };
        login.begin(window, cx);
        login
    }

    pub fn account(&self) -> &Account {
        &self.account
    }

    /// Starts the session, unless the last one of this account is still
    /// ending: then it is asked again when one ends.
    fn begin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pane.is_some() {
            return;
        }
        if cx.try_global::<Ending>().is_some_and(|e| e.0.contains(&session_id(&self.account))) {
            cx.notify();
            return;
        }
        let (models, spec) = (self.models.clone(), spec(&self.account));
        let pane = cx.new(|cx| TermPane::new(models, spec, window, cx));
        cx.subscribe(&pane, |this, _, exit: &Exited, cx| {
            if exit.code.is_none_or(|c| c == 0) {
                this.close(cx);
            }
        })
        .detach();
        pane.update(cx, |p, cx| p.focus(window, cx));
        self.pane = Some(pane);
        cx.notify();
    }

    /// Its session and where it stands, for the driver's `panels`.
    #[cfg(luna_test)]
    pub fn state(&self, cx: &App) -> String {
        let id = session_id(&self.account);
        match &self.pane {
            Some(pane) => format!("{id} {:?}", pane.read(cx).phase()),
            None => format!("{id} waiting"),
        }
    }

    /// Whether the CLI's terminal has the keyboard: Esc is then the CLI's.
    pub fn terminal_has_keyboard(&self, window: &Window, cx: &App) -> bool {
        use gpui::Focusable;
        self.pane.as_ref().is_some_and(|p| p.read(cx).focus_handle(cx).is_focused(window))
    }

    /// The window is done with: its session ended, and whoever shows it told.
    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.end(cx);
        cx.emit(Closed);
    }

    /// Ends the session, if there is one still to end. A start the core has
    /// not answered yet is ended once it has: a kill sent sooner finds no
    /// session and leaves it running.
    pub fn end(&mut self, cx: &mut App) {
        let Some(pane) = self.pane.take() else { return };
        // A pane let go is not started again in a core found anew.
        pane.update(cx, |p, _| p.shown = false);
        let id = session_id(&self.account);
        cx.default_global::<Ending>().0.insert(id.clone());
        end_session(self.models.host.clone(), id, pane, cx);
    }
}

/// Kills the session once it is known to run, and takes `id` off `Ending`.
fn end_session(host: Arc<dyn Host>, id: String, pane: Entity<TermPane>, cx: &mut App) {
    cx.spawn(async move |cx| {
        let phase = loop {
            let phase = pane.read_with(cx, |p, _| p.phase());
            if phase != Phase::Starting {
                break phase;
            }
            cx.background_executor().timer(LOOK).await;
        };
        // One that exited, or never started, has nothing to kill: asking
        // would only log a session the core does not have.
        if matches!(phase, Phase::Live(_)) {
            let _ = host.call(cmd::KILL_SESSION, json!({ "id": id })).await;
        }
        cx.update(|cx| {
            cx.update_global(|e: &mut Ending, _| e.0.remove(&id));
        });
    })
    .detach();
}

impl Render for LoginWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.models.shell.read(cx).palette();
        let this = cx.weak_entity();
        let close: OnClick = Rc::new(move |_, cx| {
            let _ = this.update(cx, |v, cx| v.close(cx));
        });
        let a = &self.account;
        let screen = match &self.pane {
            Some(pane) => div().flex_1().min_h_0().child(pane.clone()),
            None => div().flex_1().p(px(10.)).child(ui::note(&p, "Waiting for the last sign-in session to end...")),
        };
        let body = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(div().flex_none().px(px(6.)).py(px(3.)).child(ui::path_text(&p, a.path.clone())))
            .child(screen);
        div()
            .id("login-scrim")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(p.frame.opacity(0.35))
            .occlude()
            .child(ui::frame(&p, "login", format!("Sign in - {} - {}", a.name, a.provider.label()), px(680.), close, body).h(px(500.)))
    }
}
