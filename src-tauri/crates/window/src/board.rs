//! The board the panes sit on: the terminals the window was opened with,
//! side by side, each in a black frame. The board's own controls (layouts,
//! peek, a pane's menu) come later.

use crate::models::Models;
use crate::term::{SessionSpec, TermPane};
use gpui::{AppContext, Context, Entity, IntoElement, ParentElement, Render, Styled, Window, div, px};

pub struct Board {
    models: Models,
    pub(crate) panes: Vec<Entity<TermPane>>,
    pub(crate) renders: usize,
}

impl Board {
    pub fn new(models: Models, terminals: Vec<SessionSpec>, window: &mut Window, cx: &mut Context<Self>) -> Board {
        cx.observe(&models.shell, |_, _, cx| cx.notify()).detach();
        let panes: Vec<Entity<TermPane>> =
            terminals.into_iter().map(|spec| cx.new(|cx| TermPane::new(models.clone(), spec, window, cx))).collect();
        if let Some(first) = panes.first() {
            first.update(cx, |p, cx| p.focus(window, cx));
        }
        Board { models, panes, renders: 0 }
    }
}

impl Render for Board {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let p = self.models.shell.read(cx).palette();
        div()
            .size_full()
            .flex()
            .gap(px(4.))
            .p(px(4.))
            .bg(p.board)
            .children(self.panes.iter().map(|pane| div().flex_1().min_w_0().h_full().border_1().border_color(p.frame).child(pane.clone())))
    }
}
