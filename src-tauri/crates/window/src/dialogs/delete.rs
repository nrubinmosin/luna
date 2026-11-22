//! The delete-chat dialog (src/features/chats/DeleteChatDialog.tsx): the one
//! gate every Delete goes through, the sidebar row's and a pane's menu's. It
//! says what goes (the session, its scrollback, the attachments; the
//! transcript is kept), owns "stop or keep its helpers", and then deletes:
//! the helpers it was told to, the chat's session and the row. Luna removes
//! no worktree (ADR-0030): one the CLI made stays on disk. Passing a helper
//! to another chat is cut too: a helper left alone answers to no chat and
//! deletes itself once idle.

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

impl DeleteChat {
    pub fn new(models: Models, layer: WeakEntity<DialogLayer>, chat: Chat, cx: &mut Context<Self>) -> DeleteChat {
        let mut dialog =
            DeleteChat { models, layer, chat, helpers: Vec::new(), stake: HashMap::new(), choices: Choices::default(), error: None };
        dialog.look(cx);
        dialog
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
        cx.spawn(async move |_, _| {
            for (id, call) in helpers {
                if let Err(e) = call.await {
                    log::warn!("deleting helper {id} failed: {e}");
                }
            }
            if let Err(e) = session.await {
                log::warn!("deleting the session failed: {e}");
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
        if let Some(e) = &self.error {
            body = body.child(ui::alert(&p, Tone::Trouble, e.clone()));
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
