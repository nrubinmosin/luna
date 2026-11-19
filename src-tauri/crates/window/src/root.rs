//! The window's root view: the title bar, the core banner, the import's
//! notice, and the sidebar beside the board, inside a black 1 px frame, in
//! the one font. The title bar, the sidebar and the board are cached views:
//! a frame drawn for one of them reuses the others as they were. The
//! window's own keys (keys.rs) are handled here, in its key context.

use crate::banner::Banner;
use crate::board::Board;
use crate::font::FAMILY;
use crate::keys::{self, CONTEXT};
use crate::models::Models;
use crate::sidebar::{STRIP, Sidebar};
use crate::theme::TEXT_SIZE;
use crate::title_bar::{self, TitleBar};
use crate::widgets::button;
use gpui::{
    AppContext, Context, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, Render, StatefulInteractiveElement,
    StyleRefinement, Styled, Window, WindowAppearance, div, px,
};

pub struct LunaWindow {
    models: Models,
    pub(crate) title_bar: Entity<TitleBar>,
    pub(crate) banner: Entity<Banner>,
    pub(crate) sidebar: Entity<Sidebar>,
    pub(crate) board: Entity<Board>,
}

/// Dark, as GPUI names the system's appearances.
pub fn is_dark(appearance: WindowAppearance) -> bool {
    matches!(appearance, WindowAppearance::Dark | WindowAppearance::VibrantDark)
}

impl LunaWindow {
    pub fn new(models: Models, window: &mut Window, cx: &mut Context<Self>) -> LunaWindow {
        cx.observe(&models.shell, |_, _, cx| cx.notify()).detach();
        cx.observe(&models.workspace, |_, _, cx| cx.notify()).detach();
        window.focus(&models.home, cx);
        // "system" follows the window's appearance, which follows the
        // system's light or dark mode.
        let shell = models.shell.clone();
        shell.update(cx, |s, cx| s.set_system_dark(is_dark(window.appearance()), cx));
        cx.observe_window_appearance(window, move |_, window, cx| {
            let dark = is_dark(window.appearance());
            shell.update(cx, |s, cx| s.set_system_dark(dark, cx));
        })
        .detach();
        LunaWindow {
            title_bar: cx.new(|cx| TitleBar::new(models.clone(), cx)),
            banner: cx.new(|cx| Banner::new(models.clone(), cx)),
            sidebar: cx.new(|cx| Sidebar::new(models.clone(), cx)),
            board: cx.new(|cx| Board::new(models.clone(), window, cx)),
            models,
        }
    }

    #[cfg(luna_test)]
    pub(crate) fn models(&self) -> &Models {
        &self.models
    }

    fn layout(&mut self, n: usize, cx: &mut Context<Self>) {
        self.models.workspace.update(cx, |w, cx| w.set_layout(n, cx));
    }

    /// Ctrl+digit: a key that means nothing here goes on to the terminal.
    fn digit(&mut self, n: usize, cx: &mut Context<Self>) {
        if !self.models.workspace.update(cx, |w, cx| w.digit(n, cx)) {
            cx.propagate();
        }
    }

    /// Esc: a menu, then a name being edited, then what is up over the
    /// board; with none of those the terminal has it.
    fn escape(&mut self, cx: &mut Context<Self>) {
        if self.board.update(cx, |b, cx| b.escape(cx)) || self.sidebar.update(cx, |s, cx| s.cancel_rename(cx)) {
            return;
        }
        if !self.models.workspace.update(cx, |w, cx| w.close_peek(cx)) {
            cx.propagate();
        }
    }

    fn seam(&mut self, quick: bool, window: &mut Window, cx: &mut Context<Self>) {
        let seams = self.models.workspace.read(cx).seams.clone();
        match if quick { seams.quick_chat } else { seams.new_chat } {
            Some(seam) => seam(window, cx),
            None if quick => log::warn!("quick chat: not in this build yet"),
            None => log::warn!("new chat: the new-chat dialog is not in this build yet"),
        }
    }
}

impl Render for LunaWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.models.shell.read(cx).palette();
        let ws = self.models.workspace.read(cx);
        let notice = ws.notice();
        let side = if ws.state.sidebar.pinned { ws.state.sidebar.width } else { STRIP };
        // Laid out at these sizes rather than measured: a cached view needs
        // a definite size.
        let bar = StyleRefinement::default().w_full().h(title_bar::HEIGHT).flex_none();
        let sidebar = StyleRefinement::default().w(px(side)).h_full().flex_none();
        let board = StyleRefinement::default().h_full().flex_1().min_w_0();
        let notice = notice.map(|text| {
            let ws = self.models.workspace.clone();
            div()
                .id("import-notice")
                .debug_selector(|| "import-notice".into())
                .flex()
                .flex_none()
                .items_center()
                .gap(px(8.))
                .px(px(6.))
                .py(px(3.))
                .bg(p.attention)
                .text_color(p.on_status)
                .border_b_1()
                .border_color(p.frame)
                .child(div().flex_1().min_w_0().font_weight(FontWeight::BOLD).child(text))
                .child(
                    div()
                        .id("import-notice-ok")
                        .debug_selector(|| "import-notice-ok".into())
                        .on_click(move |_, _, cx| ws.update(cx, |w, cx| w.dismiss_notice(cx)))
                        .child(button(&p, "OK")),
                )
        });
        div()
            .id("luna-window")
            .key_context(CONTEXT)
            .track_focus(&self.models.home)
            .on_action(cx.listener(|this, _: &keys::NewChat, window, cx| this.seam(false, window, cx)))
            .on_action(cx.listener(|this, _: &keys::QuickChat, window, cx| this.seam(true, window, cx)))
            .on_action(cx.listener(|this, _: &keys::Layout1, _, cx| this.layout(1, cx)))
            .on_action(cx.listener(|this, _: &keys::Layout2, _, cx| this.layout(2, cx)))
            .on_action(cx.listener(|this, _: &keys::Layout3, _, cx| this.layout(3, cx)))
            .on_action(cx.listener(|this, _: &keys::Layout4, _, cx| this.layout(4, cx)))
            .on_action(cx.listener(|this, _: &keys::Pane1, _, cx| this.digit(1, cx)))
            .on_action(cx.listener(|this, _: &keys::Pane2, _, cx| this.digit(2, cx)))
            .on_action(cx.listener(|this, _: &keys::Pane3, _, cx| this.digit(3, cx)))
            .on_action(cx.listener(|this, _: &keys::Pane4, _, cx| this.digit(4, cx)))
            .on_action(cx.listener(|this, _: &keys::NextWaiting, _, cx| this.models.workspace.update(cx, |w, cx| w.next_waiting(cx))))
            .on_action(cx.listener(|this, _: &keys::NextBoard, _, cx| this.models.workspace.update(cx, |w, cx| w.next_board(cx))))
            .on_action(cx.listener(|this, _: &keys::ClosePeek, _, cx| this.escape(cx)))
            .size_full()
            .flex()
            .flex_col()
            .font_family(FAMILY)
            .text_size(TEXT_SIZE)
            .bg(p.surface)
            .text_color(p.text)
            .border_1()
            .border_color(p.frame)
            .child(self.title_bar.clone().cached(bar))
            .child(self.banner.clone())
            .children(notice)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(self.sidebar.clone().cached(sidebar))
                    .child(self.board.clone().cached(board)),
            )
    }
}
