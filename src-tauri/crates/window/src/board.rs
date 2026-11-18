//! The board the panes will sit on: empty in this window so far.

use crate::models::Models;
use gpui::{Context, IntoElement, ParentElement, Render, Styled, Window, div};

pub struct Board {
    models: Models,
    pub(crate) renders: usize,
}

impl Board {
    pub fn new(models: Models, cx: &mut Context<Self>) -> Board {
        cx.observe(&models.shell, |_, _, cx| cx.notify()).detach();
        Board { models, renders: 0 }
    }
}

impl Render for Board {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let p = self.models.shell.read(cx).palette();
        div().size_full().bg(p.board).child(div())
    }
}
