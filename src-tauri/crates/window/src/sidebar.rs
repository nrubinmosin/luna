//! The sidebar: the chats of the board on screen, by the folder they were
//! launched from (src/features/chats/ChatList.tsx and its rows). Pinned, it
//! holds its place beside the board; collapsed, it is a strip with the same
//! chats as status squares. There is no hover overlay: it is one or the
//! other until the person switches it.
//!
//! A row says its chat's status, a draft and where it shows: its pane's
//! number on a board of more panes, its place in the list on a board of one,
//! which is what Ctrl+digit counts. A click shows the chat (`show_chat`); the
//! active row offers Rename and Delete, the latter through the delete
//! dialog's seam (workspace.rs `Seams`).

use crate::models::Models;
use crate::rename::Renaming;
use crate::state::{Chat, Folder, Status, board_label};
use crate::theme::Palette;
use crate::widgets::{button, status_square, tag};
use crate::workspace::{Live, Workspace};
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, Context, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};
use gpui_component::input::Input;

/// The collapsed strip's width.
pub const STRIP: f32 = 26.;

pub struct Sidebar {
    models: Models,
    pub(crate) renaming: Option<Renaming>,
    pub(crate) renders: usize,
}

impl Sidebar {
    pub fn new(models: Models, cx: &mut Context<Self>) -> Sidebar {
        cx.observe(&models.workspace, |_, _, cx| cx.notify()).detach();
        cx.observe(&models.shell, |_, _, cx| cx.notify()).detach();
        Sidebar { models, renaming: None, renders: 0 }
    }

    fn workspace(&self) -> &Entity<Workspace> {
        &self.models.workspace
    }

    /// Esc while a name is being edited: it is let go. False when none was.
    pub fn cancel_rename(&mut self, cx: &mut Context<Self>) -> bool {
        let was = self.renaming.take().is_some();
        if was {
            cx.notify();
        }
        was
    }

    fn rename(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.workspace().read(cx).state.find(id).map(|c| c.name.clone()) else { return };
        self.renaming = Some(Renaming::start(id, &name, self.workspace().clone(), |s: &mut Sidebar| &mut s.renaming, window, cx));
        cx.notify();
    }

    fn row(&self, p: &Palette, chat: &Chat, number: Option<usize>, cx: &mut Context<Self>) -> AnyElement {
        let ws = self.workspace().read(cx);
        let state = &ws.state;
        let live: Live = ws.live(&chat.id);
        let status = ws.shown_status(chat, cx);
        let active = state.active.as_deref() == Some(chat.id.as_str());
        // A board of panes numbers them; a board of one, the list.
        let (badge, on_screen) = if state.layout() > 1 {
            (state.pane_of(&chat.id).map(|i| i + 1), state.pane_of(&chat.id).is_some())
        } else {
            (number.filter(|n| *n <= 9), state.seat(0) == Some(chat.id.as_str()))
        };
        let (bg, fg) = if active { (p.title_bg, p.title_fg) } else { (p.well, p.text) };
        let id = chat.id.clone();
        let name: AnyElement = match &self.renaming {
            Some(r) if r.chat == chat.id => div().flex_1().min_w_0().child(Input::new(&r.input).h(px(18.))).into_any_element(),
            _ => div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .when(status == Status::Waiting, |d| d.font_weight(FontWeight::BOLD))
                .child(SharedString::from(chat.name.clone()))
                .into_any_element(),
        };
        let ws_entity = self.workspace().clone();
        let mut row = div()
            .id(SharedString::from(format!("row-{id}")))
            .debug_selector({
                let id = id.clone();
                move || format!("row-{id}")
            })
            .flex()
            .flex_none()
            .items_center()
            .gap(px(4.))
            .pl(px(14.))
            .pr(px(4.))
            .h(px(24.))
            .bg(bg)
            .text_color(fg)
            .on_click({
                let id = id.clone();
                move |_, _, cx| ws_entity.update(cx, |w, cx| w.show_chat(&id, cx))
            })
            .child(status_square(p, status))
            .child(name)
            .children(live.draft_tag().map(|t| tag(p, Some(p.attention), t)))
            .children(badge.map(|n| tag(p, on_screen.then_some(p.info), n.to_string())));
        if active && self.renaming.is_none() {
            let rename_id = id.clone();
            let delete_id = id.clone();
            row = row
                .child(
                    div()
                        .id("row-rename")
                        .debug_selector(|| "row-rename".into())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.rename(&rename_id, window, cx);
                        }))
                        .child(button(p, "Rename")),
                )
                .child(div().id("row-delete").debug_selector(|| "row-delete".into()).child(button(p, "Delete")).on_click(cx.listener(
                    move |this, _, window, cx| {
                        cx.stop_propagation();
                        let seam = this.workspace().read(cx).seams.delete_chat.clone();
                        match seam {
                            Some(seam) => seam(delete_id.clone(), window, cx),
                            None => log::warn!("delete chat: the delete dialog is not in this build yet"),
                        }
                    },
                )));
        }
        row.into_any_element()
    }

    fn section(&self, p: &Palette, folder: &Folder, chats: &[&Chat], numbers: &[String], cx: &mut Context<Self>) -> AnyElement {
        let ws = self.workspace().clone();
        let folder_id = folder.id.clone();
        let head = div()
            .id(SharedString::from(format!("folder-{}", folder.id)))
            .debug_selector({
                let id = folder.id.clone();
                move || format!("folder-{id}")
            })
            .flex()
            .flex_none()
            .items_center()
            .gap(px(4.))
            .px(px(4.))
            .h(px(22.))
            .bg(p.surface)
            .text_color(p.text)
            .border_b_1()
            .border_color(p.lo)
            .font_weight(FontWeight::BOLD)
            .on_click(move |_, _, cx| ws.update(cx, |w, cx| w.toggle_folder(&folder_id, cx)))
            .child(if folder.open { "[-]" } else { "[+]" })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(SharedString::from(folder.leaf().to_owned())),
            )
            .child(div().text_color(p.text_dim).child(format!("{}", chats.len())));
        let rows: Vec<AnyElement> = if folder.open {
            chats.iter().map(|c| self.row(p, c, numbers.iter().position(|n| *n == c.id).map(|i| i + 1), cx)).collect()
        } else {
            Vec::new()
        };
        div().flex().flex_col().flex_none().child(head).children(rows).into_any_element()
    }
}

impl Render for Sidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let p = self.models.shell.read(cx).palette();
        let ws = self.workspace().clone();
        let state = &ws.read(cx).state;
        let board = state.board;
        let pinned = state.sidebar.pinned;
        let width = state.sidebar.width;
        let numbers = state.numbered(board);
        let new_chat = {
            let ws = ws.clone();
            move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut gpui::App| {
                let seam = ws.read(cx).seams.new_chat.clone();
                match seam {
                    Some(seam) => seam(window, cx),
                    None => log::warn!("new chat: the new-chat dialog is not in this build yet"),
                }
            }
        };
        let pin = {
            let ws = ws.clone();
            move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut gpui::App| ws.update(cx, |w, cx| w.toggle_sidebar(cx))
        };

        if !pinned {
            let squares: Vec<AnyElement> = state
                .folders_of_board(board)
                .into_iter()
                .flat_map(|(_, chats)| chats.into_iter().map(|c| (c.id.clone(), ws.read(cx).shown_status(c, cx))))
                .map(|(id, status)| {
                    let ws = ws.clone();
                    div()
                        .id(SharedString::from(format!("strip-{id}")))
                        .flex()
                        .justify_center()
                        .py(px(2.))
                        .on_click(move |_, _, cx| ws.update(cx, |w, cx| w.show_chat(&id, cx)))
                        .child(status_square(&p, status))
                        .into_any_element()
                })
                .collect();
            return div()
                .id("sidebar")
                .w(px(STRIP))
                .h_full()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(4.))
                .py(px(4.))
                .bg(p.surface)
                .border_r_1()
                .border_color(p.frame)
                .child(div().id("sidebar-pin").debug_selector(|| "sidebar-pin".into()).on_click(pin).child(button(&p, ">")))
                .child(div().id("new-chat").debug_selector(|| "new-chat".into()).on_click(new_chat).child(button(&p, "+")))
                .children(squares)
                .into_any_element();
        }

        let sections: Vec<AnyElement> = {
            let folders: Vec<(Folder, Vec<Chat>)> =
                state.folders_of_board(board).into_iter().map(|(f, chats)| (f.clone(), chats.into_iter().cloned().collect())).collect();
            folders
                .iter()
                .map(|(f, chats)| {
                    let refs: Vec<&Chat> = chats.iter().collect();
                    self.section(&p, f, &refs, &numbers, cx)
                })
                .collect()
        };
        let empty = sections.is_empty().then(|| {
            div().p(px(8.)).text_color(p.text_dim).child(format!("No chats on board {} yet: New chat starts one here.", board_label(board)))
        });
        div()
            .id("sidebar")
            .w(px(width))
            .h_full()
            .flex()
            .flex_col()
            .flex_none()
            .bg(p.surface)
            .border_r_1()
            .border_color(p.frame)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(4.))
                    .px(px(4.))
                    .h(px(26.))
                    .border_b_1()
                    .border_color(p.frame)
                    .child(div().flex_1().font_weight(FontWeight::BOLD).child(format!("Chats on board {}", board_label(board))))
                    .child(div().id("new-chat").debug_selector(|| "new-chat".into()).on_click(new_chat).child(button(&p, "New chat")))
                    .child(div().id("sidebar-pin").debug_selector(|| "sidebar-pin".into()).on_click(pin).child(button(&p, "<"))),
            )
            .child(
                div()
                    .id("chat-list")
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .overflow_y_scroll()
                    .bg(p.well)
                    .children(sections)
                    .children(empty),
            )
            .into_any_element()
    }
}
