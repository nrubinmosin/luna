//! The window's root view: the title bar, the core banner and the board,
//! inside a black 1 px frame, in the one font. The title bar and the board are
//! cached views: a frame drawn for the banner reuses them as they were.

use crate::banner::Banner;
use crate::board::Board;
use crate::font::FAMILY;
use crate::models::Models;
use crate::term::SessionSpec;
use crate::theme::TEXT_SIZE;
use crate::title_bar::{self, TitleBar};
use gpui::{AppContext, Context, Entity, IntoElement, ParentElement, Render, StyleRefinement, Styled, Window, WindowAppearance, div};

pub struct LunaWindow {
    models: Models,
    pub(crate) title_bar: Entity<TitleBar>,
    pub(crate) banner: Entity<Banner>,
    pub(crate) board: Entity<Board>,
}

/// Dark, as GPUI names the system's appearances.
pub fn is_dark(appearance: WindowAppearance) -> bool {
    matches!(appearance, WindowAppearance::Dark | WindowAppearance::VibrantDark)
}

impl LunaWindow {
    pub fn new(models: Models, terminals: Vec<SessionSpec>, window: &mut Window, cx: &mut Context<Self>) -> LunaWindow {
        cx.observe(&models.shell, |_, _, cx| cx.notify()).detach();
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
            board: cx.new(|cx| Board::new(models.clone(), terminals, window, cx)),
            models,
        }
    }
}

impl Render for LunaWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.models.shell.read(cx).palette();
        // Laid out at these sizes rather than measured: a cached view needs
        // a definite size.
        let bar = StyleRefinement::default().w_full().h(title_bar::HEIGHT).flex_none();
        let board = StyleRefinement::default().w_full().flex_1().min_h_0();
        div()
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
            .child(self.board.clone().cached(board))
    }
}
