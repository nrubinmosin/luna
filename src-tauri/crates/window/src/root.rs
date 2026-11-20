//! The window's root view: the title bar, the core banner, the notices
//! about the chat list's file and the import, the sidebar beside the
//! board, and the orphan line and the status bar at its foot, inside a
//! black 1 px frame, in
//! the one font. The title bar, the sidebar, the board and the status bar are cached views:
//! a frame drawn for one of them reuses the others as they were. The
//! window's own keys (keys.rs) are handled here, in its key context.

use crate::banner::Banner;
use crate::board::Board;
use crate::dialogs::DialogLayer;
use crate::font::FAMILY;
use crate::keys::{self, CONTEXT};
use crate::models::Models;
use crate::panels::PanelLayer;
use crate::selector::Selector;
use crate::sidebar::{STRIP, Sidebar};
use crate::status::{self, OrphanLine, StatusBar};
use crate::theme::TEXT_SIZE;
use crate::title_bar::{self, TitleBar};
use crate::widgets::button;
use crate::workspace::StateNotice;
use gpui::{
    AppContext, Context, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, StyleRefinement, Styled, Window, WindowAppearance, div, px,
};

pub struct LunaWindow {
    models: Models,
    pub(crate) title_bar: Entity<TitleBar>,
    pub(crate) banner: Entity<Banner>,
    pub(crate) sidebar: Entity<Sidebar>,
    pub(crate) board: Entity<Board>,
    pub(crate) orphans: Entity<OrphanLine>,
    pub(crate) status_bar: Entity<StatusBar>,
    /// The library, settings and accounts windows, over all of it.
    pub(crate) panels: Entity<PanelLayer>,
    /// The new-chat, delete, helpers and crash dialogs, over the panels.
    pub(crate) dialogs: Entity<DialogLayer>,
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
            orphans: cx.new(|cx| OrphanLine::new(models.clone(), cx)),
            status_bar: cx.new(|cx| StatusBar::new(models.clone(), cx)),
            panels: cx.new(|cx| PanelLayer::new(models.clone(), cx)),
            dialogs: cx.new(|cx| DialogLayer::new(models.clone(), window, cx)),
            models,
        }
    }

    #[cfg(any(test, luna_test))]
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
    /// board while its terminal has not been given the keyboard (a click or
    /// a key typed in it, board.rs); otherwise the terminal has it: Esc is
    /// how a CLI is interrupted or a prompt turned down. `force` is
    /// Shift+Esc, which closes the peek whoever has the keyboard.
    fn escape(&mut self, force: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.status_bar.update(cx, |s, cx| s.close_panel(cx))
            || self.board.update(cx, |b, cx| b.escape(cx))
            || self.sidebar.update(cx, |s, cx| s.cancel_rename(cx))
        {
            return;
        }
        let terminal = self.board.read(cx).terminal_has_keyboard(window, cx);
        if (force || !terminal) && self.models.workspace.update(cx, |w, cx| w.close_peek(cx)) {
            return;
        }
        cx.propagate();
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
        let notices = ws.notices();
        let side = if ws.state.sidebar.pinned { ws.state.sidebar.width } else { STRIP };
        // Laid out at these sizes rather than measured: a cached view needs
        // a definite size.
        let bar = StyleRefinement::default().w_full().h(title_bar::HEIGHT).flex_none();
        let sidebar = StyleRefinement::default().w(px(side)).h_full().flex_none();
        let board = StyleRefinement::default().h_full().flex_1().min_w_0();
        let status = StyleRefinement::default().w_full().h(status::HEIGHT).flex_none();
        let notices: Vec<_> = notices.into_iter().map(|n| notice(&p, &self.models, n)).collect();
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
            .on_action(cx.listener(|this, _: &keys::ClosePeek, window, cx| this.escape(false, window, cx)))
            .on_action(cx.listener(|this, _: &keys::ForceClosePeek, window, cx| this.escape(true, window, cx)))
            .size_full()
            .relative()
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
            .children(notices)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(self.sidebar.clone().cached(sidebar))
                    .child(self.board.clone().cached(board)),
            )
            .child(self.orphans.clone())
            .child(self.status_bar.clone().cached(status))
            .child(self.panels.clone())
            .child(self.dialogs.clone())
    }
}

/// One notice's line: its words, and Retry, OK and Quit anyway where it
/// offers them.
fn notice(p: &crate::theme::Palette, models: &Models, n: StateNotice) -> gpui::Stateful<gpui::Div> {
    let key = n.key;
    let action = |what: &'static str, label: &'static str| {
        let ws = models.workspace.clone();
        div()
            .id(SharedString::from(format!("{key}-notice-{what}")))
            .selector(move || format!("{key}-notice-{what}"))
            .on_click(move |_, _, cx| {
                ws.update(cx, |w, cx| match what {
                    "retry" => w.retry(key, cx),
                    "quit" => w.quit_anyway(cx),
                    _ => w.dismiss(key, cx),
                });
            })
            .child(button(p, label))
    };
    div()
        .id(SharedString::from(format!("{key}-notice")))
        .selector(move || format!("{key}-notice"))
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
        .child(div().flex_1().min_w_0().font_weight(FontWeight::BOLD).child(n.text))
        .children(n.retry.then(|| action("retry", "Retry")))
        .children(n.ok.then(|| action("ok", "OK")))
        .children(n.quit_anyway.then(|| action("quit", "Quit anyway")))
}
