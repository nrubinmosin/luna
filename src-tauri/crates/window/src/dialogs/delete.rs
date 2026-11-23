//! The delete-chat dialog (src/features/chats/DeleteChatDialog.tsx): the one
//! gate every Delete goes through, the sidebar row's, a pane's menu's and its
//! title bar's ×. It says what goes (the session, its scrollback, the
//! attachments; the transcript is kept), owns "stop or keep its helpers", and
//! then deletes: the helpers it was told to, the chat's session and the row.
//! Luna removes no worktree (ADR-0030): one the CLI made stays on disk.
//! Passing a helper to another chat is cut too: a helper left alone answers
//! to no chat and deletes itself once idle.

use super::rows::{self, AtStake, Choices, Helper, Line};
use super::{DialogLayer, cmd};
use crate::models::Models;
use crate::panels::ask;
use crate::panels::ui::{self, OnClick, Tone};
use crate::scale::u;
use crate::state::Chat;
use gpui::{Context, InteractiveElement, IntoElement, ParentElement, Render, StatefulInteractiveElement, Styled, WeakEntity, Window, div};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

pub struct DeleteChat {
    models: Models,
    layer: WeakEntity<DialogLayer>,
    chat: Chat,
    /// The helpers its agent runs, once listed, and what each has at stake as
    /// the core sees it now (the rows alone until it answers).
    helpers: Vec<Helper>,
    stake: HashMap<String, AtStake>,
    pub choices: Choices,
    pub error: Option<String>,
}

/// How long a delete counts as work an update's last write waits for
/// (`Workspace::admit`): until the core has answered its session's delete,
/// whose refusal for a helper it holds comes at once, an accepted one within
/// the session's quit (10 s, pty/control.rs `GRACE`). Under the update's
/// `SETTLE_LIMIT`.
pub const DELETE_HELD: Duration = Duration::from_secs(12);
const _: () = assert!(DELETE_HELD.as_secs() < crate::workspace::SETTLE_LIMIT.as_secs());

/// What the dialog says while the core is not up: nothing it deletes could
/// be done, the session's delete among it, and the core's list of helpers a
/// row may still be one of is not there to go by.
pub const NO_CORE: &str = "Luna's core is not running, so nothing can be deleted now. Delete again once it is back.";

impl DeleteChat {
    pub fn new(models: Models, layer: WeakEntity<DialogLayer>, chat: Chat, cx: &mut Context<Self>) -> DeleteChat {
        // Redrawn as the core comes and goes: `NO_CORE` says so while it is
        // away, and goes once it is back.
        cx.observe(&models.core, |v: &mut DeleteChat, _, cx| {
            if v.core_up(cx) && v.error.as_deref() == Some(NO_CORE) {
                v.error = None;
            }
            cx.notify();
        })
        .detach();
        let mut dialog =
            DeleteChat { models, layer, chat, helpers: Vec::new(), stake: HashMap::new(), choices: Choices::default(), error: None };
        dialog.look(cx);
        dialog
    }

    fn core_up(&self, cx: &gpui::App) -> bool {
        self.models.core.read(cx).link().state == crate::dto::LinkState::Up
    }

    /// The chat's helpers, then what each has at stake.
    fn look(&mut self, cx: &mut Context<Self>) {
        let all = ask::<Vec<Helper>>(&self.models.host, cmd::BACKGROUND_HELPERS, json!({}));
        let host = self.models.host.clone();
        let chat = self.chat.id.clone();
        cx.spawn(async move |this, cx| {
            let Ok(all) = all.await else { return };
            let mine = rows::helpers_under(&all, &chat);
            let ids: Vec<String> = mine.iter().map(|h| h.id.clone()).collect();
            let _ = this.update(cx, |v, cx| {
                v.helpers = mine;
                cx.notify();
            });
            if ids.is_empty() {
                return;
            }
            if let Ok(rows) = ask::<Vec<AtStake>>(&host, cmd::BACKGROUND_AT_STAKE, json!({ "ids": ids })).await {
                let _ = this.update(cx, |v, cx| {
                    v.stake = rows.into_iter().map(|r| (r.id.clone(), r)).collect();
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn lines(&self) -> Vec<Line> {
        rows::lines(&self.helpers, &self.stake)
    }

    pub fn may_close(&mut self, _cx: &mut Context<Self>) -> bool {
        true
    }

    /// Deletes: what the person ticked of the helpers, the chat's session,
    /// and the row, off every board and out of the list.
    pub fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Nothing is stopped or deleted that the list could not then drop.
        if let Err(why) = self.models.workspace.read(cx).unfrozen() {
            self.error = Some(why);
            return cx.notify();
        }
        if !self.core_up(cx) {
            self.error = Some(NO_CORE.into());
            return cx.notify();
        }
        // Until the core has listed its helpers, a row of an opening a window
        // gone left (shown from the saved list at start) looks like any chat.
        if !self.models.workspace.read(cx).helpers_known() {
            self.error = Some("Waiting for the core's list of helpers: try again in a moment.".into());
            return cx.notify();
        }
        // A helper's row still being opened as this chat: the session's delete
        // would leave the helper with neither a row nor its registry.
        if self.models.workspace.read(cx).held_by_opening(&self.chat.id) {
            self.error = Some(
                "This chat is still being opened from a helper. It can be deleted once it has opened, or once the helper is back with its agent."
                    .into(),
            );
            return cx.notify();
        }
        // Work the update's last write waits for, until the core has answered:
        // a refusal puts the row back before that write, not after it.
        if let Err(why) = self.models.workspace.update(cx, |w, _| w.admit()) {
            self.error = Some(why);
            return cx.notify();
        }
        let lines = self.lines();
        let shown: Vec<String> = lines.iter().map(|l| l.id.clone()).collect();
        let host = self.models.host.clone();
        let chat = self.chat.clone();
        let deletions = rows::deletions(&lines, self.choices, &shown);
        // The session, which one command stops and clears.
        let session = ask::<Value>(&host, cmd::DELETE_SESSION, json!({ "id": chat.id }));
        let helpers: Vec<_> = deletions
            .into_iter()
            .map(|d| {
                let call =
                    ask::<Value>(&host, cmd::BACKGROUND_DELETE, json!({ "id": d.id, "stopWorking": d.stop_working, "ofChat": chat.id }));
                (d.id, call)
            })
            .collect();
        // Where the row stood, for the core's refusal to put it back: a list of
        // helpers a little behind the core can still miss one it holds.
        let listed = self.models.workspace.read(cx).state.listed(&chat.id);
        let workspace = self.models.workspace.downgrade();
        cx.spawn(async move |_, cx| {
            let timeout = cx.background_executor().timer(DELETE_HELD);
            let (answer, held) = match futures::future::select(Box::pin(session), timeout).await {
                futures::future::Either::Left((answer, _)) => (answer, true),
                // The update waits no longer: a refusal comes at once, so what
                // comes now is the delete's own end.
                futures::future::Either::Right((_, session)) => {
                    let _ = workspace.update(cx, |w, cx| w.release(cx));
                    (session.await, false)
                }
            };
            match answer {
                Err(e) if e.contains(rows::HELD_AS_HELPER) => {
                    log::warn!("deleting the session refused, its row kept: {e}");
                    if let Some(row) = listed {
                        let _ = workspace.update(cx, |w, cx| w.relist(row, cx));
                    }
                }
                Err(e) => log::warn!("deleting the session failed: {e}"),
                Ok(_) => {}
            }
            if held {
                let _ = workspace.update(cx, |w, cx| w.release(cx));
            }
            for (id, call) in helpers {
                if let Err(e) = call.await {
                    log::warn!("deleting helper {id} failed: {e}");
                }
            }
        })
        .detach();
        let id = chat.id;
        self.models.workspace.update(cx, |w, cx| w.remove_chat(&id, cx));
        let _ = self.layer.update(cx, |l, cx| l.close(window, cx));
    }

    fn act(&self, cx: &Context<Self>, f: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static) -> OnClick {
        let this = cx.weak_entity();
        Rc::new(move |window, cx| {
            let _ = this.update(cx, |v, cx| f(v, window, cx));
        })
    }
}

fn list(names: impl Iterator<Item = String>) -> String {
    names.collect::<Vec<_>>().join(", ")
}

impl Render for DeleteChat {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.models.shell.read(cx).palette();
        let close = self.act(cx, |v, window, cx| {
            let _ = v.layer.update(cx, |l, cx| l.close(window, cx));
        });
        let name = if self.chat.name.chars().count() > 38 {
            format!("{}...", self.chat.name.chars().take(38).collect::<String>())
        } else {
            self.chat.name.clone()
        };
        let lines = self.lines();
        let c = self.choices;
        let working: Vec<&Line> = lines.iter().filter(|l| l.working.is_some()).collect();

        let mut text = String::from(
            "The session is stopped and its scrollback is lost. The transcript is kept, so the session can still be resumed from the CLI.",
        );
        if self.chat.worktree {
            text.push_str(" The git worktree its CLI made stays on disk: Luna removes no worktree.");
        }
        let mut body = div().flex().flex_col().gap(u(8.)).p(u(10.)).child(div().child(text));
        if let (true, Some(path)) = (self.chat.worktree, &self.chat.worktree_path) {
            body = body.child(ui::path_text(&p, path.clone()).px(u(4.)).py(u(2.)).bg(p.well));
        }
        if !lines.is_empty() {
            let mut stakes = div().id("delete-helpers").flex().flex_col().gap(u(2.));
            for l in &lines {
                stakes = stakes.child(div().child(format!("- {}: {}", l.name, l.working.as_deref().unwrap_or("done"))));
            }
            body = body.child(stakes);
        }
        match &self.error {
            Some(e) => body = body.child(ui::alert(&p, Tone::Trouble, e.clone())),
            None if !self.core_up(cx) => body = body.child(ui::alert(&p, Tone::Trouble, NO_CORE)),
            None => {}
        }
        let flip = |f: fn(&mut Choices)| {
            self.act(cx, move |v, _, cx| {
                f(&mut v.choices);
                cx.notify();
            })
        };
        if !lines.is_empty() {
            let n = lines.len();
            body = body.child(ui::check(
                "delete-helpers-too",
                &p,
                format!(
                    "Also delete the {n} helper{} its agent runs. Left alone, the chat's own helpers answer to no chat from then on, and each deletes itself half an hour after it is done.",
                    if n > 1 { "s" } else { "" }
                ),
                c.with_helpers,
                true,
                flip(|c| c.with_helpers = !c.with_helpers),
            ));
        }
        if c.with_helpers && !working.is_empty() {
            body = body.child(ui::check(
                "delete-stop-working",
                &p,
                format!(
                    "Also stop and delete the {} still working ({}). Left unticked, {} first.",
                    if working.len() == 1 { "one".to_owned() } else { working.len().to_string() },
                    list(working.iter().map(|l| l.name.clone())),
                    if working.len() == 1 { "it finishes" } else { "they finish" }
                ),
                c.stop_working,
                true,
                flip(|c| c.stop_working = !c.stop_working),
            ));
        }
        // The buttons stay in reach on a short screen: the body scrolls under them.
        let foot = div()
            .flex()
            .flex_none()
            .justify_end()
            .gap(u(6.))
            .p(u(8.))
            .border_t_1()
            .border_color(p.frame)
            .child(ui::button("delete-cancel", &p, "Cancel", true, close.clone()))
            .child(ui::primary("delete-yes", &p, "Delete", true, self.act(cx, |v, window, cx| v.confirm(window, cx))));
        let scroll = div().id("delete-scroll").flex_1().min_h_0().overflow_y_scroll().child(body);
        ui::frame(
            &p,
            "delete",
            format!("Delete \"{name}\"?"),
            u(520.),
            close,
            div().flex().flex_col().flex_1().min_h_0().child(scroll).child(foot),
        )
    }
}
