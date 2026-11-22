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
//! dialog's seam (workspace.rs `Seams`). A row's tooltip is the chat's full
//! name and its session id, a folder's its path; a folder's + opens the
//! new-chat dialog on it. Pinned, its right edge is a grip: dragged, the
//! width follows the pointer and is kept with the list (state.rs
//! `set_sidebar_width`).

use crate::models::Models;
use crate::rename::Renaming;
use crate::scale::u;
use crate::selector::Selector;
use crate::state::{Chat, Folder, Status, board_label};
use crate::theme::Palette;
use crate::widgets::{Tipped, button, status_square, tag, tips_on};
use crate::workspace::{Live, Workspace};
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, Context, CursorStyle, DispatchPhase, Entity, FontWeight, InteractiveElement, IntoElement, MouseButton, MouseMoveEvent,
    MouseUpEvent, ParentElement, Pixels, Render, SharedString, StatefulInteractiveElement, Styled, Window, canvas, div,
};
use gpui_component::input::Input;
use std::cell::Cell;
use std::rc::Rc;

/// The collapsed strip's width.
pub const STRIP: f32 = 26.;

/// Where the grip was pressed (the pointer's x, in pixels) and the width
/// then, until the button is let go: a drag moves the edge by as much as
/// the pointer has moved.
type Grab = Rc<Cell<Option<(f32, f32)>>>;

/// A chat's tooltip: its full name, which the row cuts short, and its
/// session id, by which issue claims and logs name it, as 0.19's row had.
pub(crate) fn chat_tip(chat: &Chat) -> Vec<SharedString> {
    let mut lines = vec![SharedString::from(chat.name.clone())];
    if let Some(id) = &chat.session_id {
        let short = id.chars().take(8).collect::<String>().to_lowercase();
        lines.push(format!("Session {short}  ({id})").into());
    }
    lines
}

pub struct Sidebar {
    models: Models,
    pub(crate) renaming: Option<Renaming>,
    pub(crate) renders: usize,
    grab: Grab,
    /// Whether this draw offers tooltips (widgets.rs `tips_on`).
    tips: bool,
}

impl Sidebar {
    pub fn new(models: Models, cx: &mut Context<Self>) -> Sidebar {
        cx.observe(&models.workspace, |_, _, cx| cx.notify()).detach();
        cx.observe(&models.shell, |_, _, cx| cx.notify()).detach();
        Sidebar { models, renaming: None, renders: 0, grab: Grab::default(), tips: true }
    }

    fn workspace(&self) -> &Entity<Workspace> {
        &self.models.workspace
    }

    /// Esc while a name is being edited: it is let go, the keyboard back
    /// where it belongs. False when none was.
    pub fn cancel_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(r) = self.renaming.take() else { return false };
        r.give_back(window, cx);
        cx.notify();
        true
    }

    pub(crate) fn rename(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.workspace().read(cx).state.find(id).map(|c| c.name.clone()) else { return };
        let (ws, home) = (self.workspace().clone(), self.models.home.clone());
        self.renaming = Some(Renaming::start(id, &name, ws, home, |s: &mut Sidebar| &mut s.renaming, window, cx));
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
            Some(r) if r.chat == chat.id => {
                div().selector(|| "rename-input".into()).flex_1().min_w_0().child(Input::new(&r.input).h(u(18.))).into_any_element()
            }
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
            .selector({
                let id = id.clone();
                move || format!("row-{id}")
            })
            .flex()
            .flex_none()
            .items_center()
            .gap(u(4.))
            .pl(u(14.))
            .pr(u(4.))
            .h(u(24.))
            .bg(bg)
            .text_color(fg)
            .on_click({
                let id = id.clone();
                move |_, _, cx| ws_entity.update(cx, |w, cx| w.show_chat(&id, cx))
            })
            .tip(self.tips, p, chat_tip(chat))
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
                        .selector(|| "row-rename".into())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.rename(&rename_id, window, cx);
                        }))
                        .child(button(p, "Rename")),
                )
                .child(div().id("row-delete").selector(|| "row-delete".into()).child(button(p, "Delete")).on_click(cx.listener(
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
        let new_here = {
            let path = folder.path.clone();
            let id = format!("folder-{}-new", folder.id);
            div()
                .id(SharedString::from(id.clone()))
                .selector(move || id)
                .flex_none()
                .tip(self.tips, p, vec!["New chat in this folder".into()])
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    crate::dialogs::open_new_chat(Some(path.clone()), window, cx);
                })
                .child(button(p, "+"))
        };
        let head = div()
            .id(SharedString::from(format!("folder-{}", folder.id)))
            .selector({
                let id = folder.id.clone();
                move || format!("folder-{id}")
            })
            .flex()
            .flex_none()
            .items_center()
            .gap(u(4.))
            .px(u(4.))
            .h(u(22.))
            .bg(p.surface)
            .text_color(p.text)
            .border_b_1()
            .border_color(p.lo)
            .font_weight(FontWeight::BOLD)
            .on_click(move |_, _, cx| ws.update(cx, |w, cx| w.toggle_folder(&folder_id, cx)))
            .tip(self.tips, p, vec![SharedString::from(folder.path.clone())])
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
            .child(div().text_color(p.text_dim).child(format!("{}", chats.len())))
            .child(new_here);
        let rows: Vec<AnyElement> = if folder.open {
            chats.iter().map(|c| self.row(p, c, numbers.iter().position(|n| *n == c.id).map(|i| i + 1), cx)).collect()
        } else {
            Vec::new()
        };
        div().flex().flex_col().flex_none().child(head).children(rows).into_any_element()
    }
}

impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        self.tips = tips_on(window);
        let tips = self.tips;
        let p = self.models.shell.read(cx).palette();
        let ws = self.workspace().clone();
        let state = &ws.read(cx).state;
        let board = state.board;
        let pinned = state.sidebar.pinned;
        let width = state.sidebar.width;
        let numbers = state.numbered(board);
        let new_chat = {
            let ws = ws.clone();
            move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
                let seam = ws.read(cx).seams.new_chat.clone();
                match seam {
                    Some(seam) => seam(window, cx),
                    None => log::warn!("new chat: the new-chat dialog is not in this build yet"),
                }
            }
        };
        let pin = {
            let ws = ws.clone();
            move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut App| ws.update(cx, |w, cx| w.toggle_sidebar(cx))
        };

        if !pinned {
            let squares: Vec<AnyElement> = state
                .folders_of_board(board)
                .into_iter()
                .flat_map(|(_, chats)| chats.into_iter().map(|c| (c.id.clone(), ws.read(cx).shown_status(c, cx), chat_tip(c))))
                .map(|(id, status, lines)| {
                    let ws = ws.clone();
                    div()
                        .id(SharedString::from(format!("strip-{id}")))
                        .flex()
                        .justify_center()
                        .py(u(2.))
                        .tip(tips, &p, lines)
                        .on_click(move |_, _, cx| ws.update(cx, |w, cx| w.show_chat(&id, cx)))
                        .child(status_square(&p, status))
                        .into_any_element()
                })
                .collect();
            return div()
                .id("sidebar")
                .w(u(STRIP))
                .h_full()
                .flex()
                .flex_col()
                .items_center()
                .gap(u(4.))
                .py(u(4.))
                .bg(p.surface)
                .border_r_1()
                .border_color(p.frame)
                .child(
                    div()
                        .id("sidebar-pin")
                        .selector(|| "sidebar-pin".into())
                        .tip(tips, &p, vec!["Pin the sidebar".into()])
                        .on_click(pin)
                        .child(button(&p, ">")),
                )
                .child(
                    div()
                        .id("new-chat")
                        .selector(|| "new-chat".into())
                        .tip(tips, &p, vec!["New chat".into()])
                        .on_click(new_chat)
                        .child(button(&p, "+")),
                )
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
            div().p(u(8.)).text_color(p.text_dim).child(format!("No chats on board {} yet: New chat starts one here.", board_label(board)))
        });
        // The grip takes the press from the row or header under it (`occlude`),
        // and the drag follows every move and the release wherever the pointer
        // is, through the window's own listeners: the first move and a release
        // right after it count too, which gpui's drag would not see before the
        // move that starts it.
        let grip = {
            let (grab, ws) = (self.grab.clone(), ws.clone());
            div()
                .id("sidebar-grip")
                .selector(|| "sidebar-grip".into())
                .absolute()
                .top_0()
                .right_0()
                .w(u(4.))
                .h_full()
                .occlude()
                .cursor_col_resize()
                .on_mouse_down(MouseButton::Left, move |e, _, cx| {
                    grab.set(Some((f32::from(e.position.x), ws.read(cx).state.sidebar.width)));
                    cx.stop_propagation();
                })
        };
        let follow = {
            let (grab, ws) = (self.grab.clone(), ws.clone());
            canvas(
                |_, _, _| {},
                move |_, _, window, _| {
                    if grab.get().is_some() {
                        window.set_window_cursor_style(CursorStyle::ResizeLeftRight);
                    }
                    let to = |grab: &Grab, ws: &Entity<Workspace>, x: Pixels, cx: &mut App| {
                        if let Some((from, width)) = grab.get() {
                            let width = width + (f32::from(x) - from) / crate::scale::current();
                            ws.update(cx, |w, cx| w.set_sidebar_width(width, cx));
                        }
                    };
                    let (g, w) = (grab.clone(), ws.clone());
                    window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
                        if phase != DispatchPhase::Capture || g.get().is_none() {
                            return;
                        }
                        // A release the window did not see (outside it) ends
                        // the drag where it was.
                        if e.pressed_button != Some(MouseButton::Left) {
                            g.set(None);
                            return;
                        }
                        to(&g, &w, e.position.x, cx);
                        cx.stop_propagation();
                    });
                    let (g, w) = (grab.clone(), ws.clone());
                    window.on_mouse_event(move |e: &MouseUpEvent, phase, _, cx| {
                        if phase == DispatchPhase::Capture && e.button == MouseButton::Left && g.get().is_some() {
                            to(&g, &w, e.position.x, cx);
                            g.set(None);
                            cx.stop_propagation();
                        }
                    });
                },
            )
            .absolute()
            .size_0()
        };
        div()
            .id("sidebar")
            .relative()
            .w(u(width))
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
                    .gap(u(4.))
                    .px(u(4.))
                    .h(u(26.))
                    .border_b_1()
                    .border_color(p.frame)
                    // The title gives way, so the buttons stay in the sidebar
                    // at its narrowest.
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .font_weight(FontWeight::BOLD)
                            .child(format!("Chats on board {}", board_label(board))),
                    )
                    .child(div().id("new-chat").selector(|| "new-chat".into()).on_click(new_chat).child(button(&p, "New chat")))
                    .child(
                        div()
                            .id("sidebar-pin")
                            .selector(|| "sidebar-pin".into())
                            .tip(tips, &p, vec!["Collapse the sidebar".into()])
                            .on_click(pin)
                            .child(button(&p, "<")),
                    ),
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
            .child(grip)
            .child(follow)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    /// A chat's tooltip: its full name, then its session id short and whole;
    /// a chat not started yet has its name only.
    #[test]
    fn a_chats_tooltip_is_its_name_and_its_session() {
        let state = crate::demo::chats_state(1, true);
        let mut chat = state.find("c-parser").expect("the fixture's chat").clone();
        let lines = |chat: &crate::state::Chat| super::chat_tip(chat).iter().map(|l| l.to_string()).collect::<Vec<_>>();
        assert_eq!(lines(&chat), [chat.name.clone(), "Session 0d6f1c2e  (0d6f1c2e-0000-4000-8000-000000000001)".into()]);
        chat.session_id = None;
        assert_eq!(lines(&chat), [chat.name.clone()]);
    }
}
