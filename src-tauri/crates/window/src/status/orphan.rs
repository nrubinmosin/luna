//! The orphan line: "N orphan sessions: kill all", between the board and the
//! status bar, only while there are any. A session no chat reaches burns an
//! account's tokens with nothing to see it, stop it or talk to it; the line
//! is the way to end them. Kill all asks once more before it kills, since a
//! session may be part-way through something; its transcript stays on disk
//! either way.

use super::vitals::Vitals;
use crate::models::Models;
use crate::widgets::button;
use gpui::{
    AnyElement, Context, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, Render, StatefulInteractiveElement, Styled,
    Window, div, px,
};

pub struct OrphanLine {
    models: Models,
    vitals: Entity<Vitals>,
}

impl OrphanLine {
    pub fn new(models: Models, cx: &mut Context<Self>) -> OrphanLine {
        cx.observe(&models.shell, |_, _, cx| cx.notify()).detach();
        let vitals = models.status.clone();
        // Only what the line shows: a count change, or the asking.
        let mut shown = (0, false);
        cx.observe(&vitals, move |_, v, cx| {
            let v = v.read(cx);
            let now = (v.orphans().len(), v.kill_asked());
            if std::mem::replace(&mut shown, now) != now {
                cx.notify();
            }
        })
        .detach();
        OrphanLine { models, vitals }
    }
}

impl Render for OrphanLine {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let v = self.vitals.read(cx);
        let n = v.orphans().len();
        if n == 0 {
            return div().into_any_element();
        }
        let p = self.models.shell.read(cx).palette();
        let (asked, plural) = (v.kill_asked(), if n == 1 { "" } else { "s" });
        let act = |id: &'static str, label: &'static str, f: fn(&mut Vitals, &mut Context<Vitals>)| -> AnyElement {
            let vitals = self.vitals.clone();
            div()
                .id(id)
                .debug_selector(move || id.into())
                .flex_none()
                .on_click(move |_, _, cx| vitals.update(cx, f))
                .child(button(&p, label))
                .into_any_element()
        };
        let (text, buttons) = if asked {
            (
                format!(
                    "Kill {n} running session{plural} that no chat reaches? Each may be part-way through something; its transcript stays on disk."
                ),
                vec![act("orphans-kill", "Kill", Vitals::kill_all), act("orphans-keep", "Keep", Vitals::keep_orphans)],
            )
        } else {
            (
                format!("{n} orphan session{plural}: running with no chat to reach {}", if n == 1 { "it" } else { "them" }),
                vec![act("orphans-kill-all", "Kill all", Vitals::kill_all)],
            )
        };
        div()
            .id("orphan-line")
            .debug_selector(|| "orphan-line".into())
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .px(px(6.))
            .py(px(3.))
            .bg(p.attention)
            .text_color(p.on_status)
            .border_t_1()
            .border_color(p.frame)
            .child(div().flex_1().min_w_0().font_weight(FontWeight::BOLD).child(text))
            .children(buttons)
            .into_any_element()
    }
}
