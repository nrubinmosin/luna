//! The delete-chat dialog (src/features/chats/DeleteChatDialog.tsx): the one
//! gate every Delete goes through, the sidebar row's and a pane's menu's. It
//! says what goes (the session, its scrollback, the attachments; the
//! transcript is kept), owns the worktree box and "stop or keep its helpers",
//! and then deletes: the helpers it was told to, the chat's session and the
//! row. Passing a helper to another chat is cut (ADR-0030): a helper left
//! alone answers to no chat and deletes itself once idle.

use super::rows::{self, AtStake, Choices, Helper, Line};
use super::{DialogLayer, cmd};
use crate::models::Models;
use crate::panels::ask;
use crate::panels::ui::{self, OnClick, Tone};
use crate::state::Chat;
use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, Render, StatefulInteractiveElement, Styled, WeakEntity, Window, div, px,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::rc::Rc;

pub struct DeleteChat {
    models: Models,
    layer: WeakEntity<DialogLayer>,
    chat: Chat,
    folder: String,
    /// Another chat in the same worktree: never dropped under it, whatever
    /// the box says.
    shared_with: Option<String>,
    /// The helpers its agent runs, once listed, and what each has at stake as
    /// the core sees it now (the rows alone until it answers).
    helpers: Vec<Helper>,
    stake: HashMap<String, AtStake>,
    pub choices: Choices,
    pub error: Option<String>,
}

impl DeleteChat {
    pub fn new(models: Models, layer: WeakEntity<DialogLayer>, chat: Chat, cx: &mut Context<Self>) -> DeleteChat {
        let ws = models.workspace.read(cx);
        let folder = ws.state.folder_of(&chat.id).map(|f| f.path.clone()).unwrap_or_default();
        let shared_with = chat.worktree_path.as_deref().and_then(|wt| {
            ws.state
                .chats()
                .find(|c| c.id != chat.id && c.worktree_path.as_deref().is_some_and(|p| super::choice::same_path(p, wt)))
                .map(|c| c.name.clone())
        });
        // Starts where Settings says; flipping it here is for this delete only.
        let choices = Choices { drop_worktree: ws.state.delete.drop_worktree, ..Choices::default() };
        let mut dialog =
            DeleteChat { models, layer, chat, folder, shared_with, helpers: Vec::new(), stake: HashMap::new(), choices, error: None };
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

    /// Whether the worktree box applies: the chat has one nobody else is in,
    /// or its helpers' worktrees go with them.
    fn worktree_box(&self) -> bool {
        (self.chat.worktree && self.shared_with.is_none()) || (self.choices.with_helpers && self.helpers.iter().any(|h| h.worktree))
    }

    pub fn may_close(&mut self, _cx: &mut Context<Self>) -> bool {
        true
    }

    /// Deletes: what the person ticked of the helpers, the chat's session and
    /// worktree, and the row, off every board and out of the list.
    pub fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let lines = self.lines();
        let shown: Vec<String> = lines.iter().map(|l| l.id.clone()).collect();
        let choices = self.choices;
        let drop_worktree = choices.drop_worktree && self.shared_with.is_none();
        let host = self.models.host.clone();
        let chat = self.chat.clone();
        let folder = self.folder.clone();
        let deletions = rows::deletions(&lines, choices, &shown);
        // The session, which one command stops and clears, and the worktree
        // and its branch only if that was asked for.
        let session = ask::<Value>(
            &host,
            cmd::DELETE_SESSION,
            json!({ "id": chat.id, "folder": folder, "worktreePath": chat.worktree_path, "dropWorktree": drop_worktree }),
        );
        let helpers: Vec<_> = deletions
            .into_iter()
            .map(|d| {
                let call = ask::<Value>(
                    &host,
                    cmd::BACKGROUND_DELETE,
                    json!({ "id": d.id, "dropWorktree": d.drop_worktree, "stopWorking": d.stop_working, "dropUnsaved": d.drop_unsaved, "ofChat": chat.id }),
                );
                (d.id, call)
            })
            .collect();
        cx.spawn(async move |_, _| {
            for (id, call) in helpers {
                match call.await {
                    Ok(r) => {
                        if let Some(kept) = r.get("worktreeKept").and_then(Value::as_str) {
                            log::warn!("helper {id} deleted, its worktree kept for its unsaved work: {kept}");
                        }
                    }
                    Err(e) => log::warn!("deleting helper {id} failed: {e}"),
                }
            }
            match session.await {
                Ok(r) => {
                    if let Some(kept) = r.get("worktreeKept").and_then(|k| k.as_bool()).filter(|k| *k) {
                        log::warn!("the worktree was kept for another session in it: {kept}");
                    }
                }
                Err(e) => log::warn!("deleting the session failed: {e}"),
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
        let unsaved: Vec<&Line> = lines.iter().filter(|l| l.unsaved.is_some()).collect();

        let mut text = String::from(
            "The session is stopped and its scrollback is lost. Pasted attachments are removed. The transcript is kept, so the session can still be resumed from the CLI.",
        );
        if self.chat.worktree {
            match &self.shared_with {
                Some(other) => text.push_str(&format!(" Its git worktree stays: \"{other}\" works in it too.")),
                None if c.drop_worktree => text.push_str(" Its git worktree goes too, unless you untick the box below."),
                None => text.push_str(" Its git worktree stays on disk unless you say otherwise."),
            }
        }
        let mut body = div().flex().flex_col().gap(px(8.)).p(px(10.)).child(div().child(text));
        if let (true, Some(path)) = (self.chat.worktree, &self.chat.worktree_path) {
            body = body.child(ui::path_text(&p, path.clone()).px(px(4.)).py(px(2.)).bg(p.well));
        }
        if !lines.is_empty() {
            let mut stakes = div().id("delete-helpers").flex().flex_col().gap(px(2.));
            for l in &lines {
                let mut line = format!("{}: {}", l.name, l.working.as_deref().unwrap_or("done"));
                if let Some(u) = &l.unsaved {
                    line.push_str(&format!(" · its worktree holds {u}"));
                }
                stakes = stakes.child(div().child(format!("- {line}")));
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
        if self.worktree_box() {
            body = body.child(ui::check(
                "delete-worktree",
                &p,
                "Delete the worktree too, including any uncommitted work.",
                c.drop_worktree,
                true,
                flip(|c| c.drop_worktree = !c.drop_worktree),
            ));
        }
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
        if c.with_helpers && c.drop_worktree && !unsaved.is_empty() {
            body = body.child(ui::check(
                "delete-unsaved",
                &p,
                format!(
                    "Also delete the worktree{} of {}, with the uncommitted work in {}. Left unticked, {} on disk.",
                    if unsaved.len() > 1 { "s" } else { "" },
                    list(unsaved.iter().map(|l| l.name.clone())),
                    if unsaved.len() > 1 { "them" } else { "it" },
                    if unsaved.len() > 1 { "they stay" } else { "it stays" }
                ),
                c.drop_unsaved,
                true,
                flip(|c| c.drop_unsaved = !c.drop_unsaved),
            ));
        }
        // The buttons stay in reach on a short screen: the body scrolls under them.
        let foot = div()
            .flex()
            .flex_none()
            .justify_end()
            .gap(px(6.))
            .p(px(8.))
            .border_t_1()
            .border_color(p.frame)
            .child(ui::button("delete-cancel", &p, "Cancel", true, close.clone()))
            .child(ui::primary("delete-yes", &p, "Delete", true, self.act(cx, |v, window, cx| v.confirm(window, cx))));
        let scroll = div().id("delete-scroll").flex_1().min_h_0().overflow_y_scroll().child(body);
        ui::frame(
            &p,
            "delete",
            format!("Delete \"{name}\"?"),
            px(520.),
            close,
            div().flex().flex_col().flex_1().min_h_0().child(scroll).child(foot),
        )
    }
}
