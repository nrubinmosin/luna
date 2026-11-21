//! The orphan line: "N orphan sessions", between the board and the status
//! bar, only while there are any. A session no chat reaches burns an
//! account's tokens with nothing to see it, stop it or talk to it; the line
//! is the way back to it. Show lists each one (what it works on, where, its
//! CLI, what it does, its process), with Adopt, which puts it in a chat of
//! its own, its terminal and scrollback with it, and Kill, which ends that
//! one; Kill all ends them all. A kill asks once more before it kills, since
//! a session may be part-way through something; its transcript stays on
//! disk either way.

use super::dto::Orphan;
use super::vitals::Vitals;
use crate::models::Models;
use crate::scale::u;
use crate::selector::Selector;
use crate::widgets::button;
use crate::workspace::provider_label;
use gpui::{
    AnyElement, Context, ElementId, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, Render, StatefulInteractiveElement,
    Styled, Window, div,
};

pub struct OrphanLine {
    models: Models,
    vitals: Entity<Vitals>,
}

/// What the line draws of the model: the count and the asking, and the rows
/// while they are listed.
#[derive(PartialEq)]
struct Drawn {
    count: usize,
    kill_asked: bool,
    rows: Option<Vec<Orphan>>,
    kill_one: Option<String>,
}

fn drawn(v: &Vitals) -> Drawn {
    Drawn {
        count: v.orphans().len(),
        kill_asked: v.kill_asked(),
        rows: v.orphans_open().then(|| v.orphans().to_vec()),
        kill_one: v.kill_one_asked().map(str::to_owned),
    }
}

impl OrphanLine {
    pub fn new(models: Models, cx: &mut Context<Self>) -> OrphanLine {
        cx.observe(&models.shell, |_, _, cx| cx.notify()).detach();
        let vitals = models.status.clone();
        // Only what the line shows.
        let mut shown = drawn(vitals.read(cx));
        cx.observe(&vitals, move |_, v, cx| {
            let was = std::mem::replace(&mut shown, drawn(v.read(cx)));
            if was != shown {
                cx.notify();
            }
        })
        .detach();
        OrphanLine { models, vitals }
    }
}

/// The last two parts of a path, as a row shows where a session runs.
fn tail2(path: &str) -> String {
    let parts: Vec<&str> = path.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
    parts[parts.len().saturating_sub(2)..].join(" / ")
}

/// A row's second line: where it runs, its CLI, what it does, its process.
fn detail(o: &Orphan) -> String {
    let mut d = tail2(&o.cwd);
    if let Some(p) = o.provider {
        d += &format!(" · {}", provider_label(p));
    }
    if let Some(s) = o.status.as_deref().filter(|s| !s.is_empty()) {
        d += &format!(" · {s}");
    }
    if let Some(pid) = o.pid {
        d += &format!(" · pid {pid}");
    }
    d
}

/// A row's first line: the session's title, else its id's end.
fn title(o: &Orphan) -> String {
    match o.title.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => t.to_owned(),
        None => format!("session {}", o.id.chars().skip(o.id.chars().count().saturating_sub(6)).collect::<String>()),
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
        let (asked, open, plural) = (v.kill_asked(), v.orphans_open(), if n == 1 { "" } else { "s" });
        let act = |id: ElementId, name: String, label: &'static str, f: Box<dyn Fn(&mut Vitals, &mut Context<Vitals>)>| -> AnyElement {
            let vitals = self.vitals.clone();
            div()
                .id(id)
                .selector(move || name)
                .flex_none()
                .on_click(move |_, _, cx| vitals.update(cx, |v, cx| f(v, cx)))
                .child(button(&p, label))
                .into_any_element()
        };
        let simple =
            |id: &'static str, label: &'static str, f: fn(&mut Vitals, &mut Context<Vitals>)| act(id.into(), id.into(), label, Box::new(f));
        let (text, buttons) = if asked {
            (
                format!(
                    "Kill {n} running session{plural} that no chat reaches? Each may be part-way through something; its transcript stays on disk."
                ),
                vec![simple("orphans-kill", "Kill", Vitals::kill_all), simple("orphans-keep", "Keep", Vitals::keep_orphans)],
            )
        } else {
            (
                format!("{n} orphan session{plural}: running with no chat to reach {}", if n == 1 { "it" } else { "them" }),
                vec![
                    simple("orphans-show", if open { "Hide" } else { "Show" }, Vitals::toggle_orphans),
                    simple("orphans-kill-all", "Kill all", Vitals::kill_all),
                ],
            )
        };
        let line = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(u(8.))
            .px(u(6.))
            .py(u(3.))
            .bg(p.attention)
            .text_color(p.on_status)
            .border_t_1()
            .border_color(p.frame)
            .child(div().flex_1().min_w_0().font_weight(FontWeight::BOLD).child(text))
            .children(buttons);
        let rows = open.then(|| {
            let kill_one = v.kill_one_asked();
            v.orphans()
                .iter()
                .enumerate()
                .map(|(i, o)| {
                    let id = o.id.clone();
                    let asking = kill_one == Some(o.id.as_str());
                    let (first, second) = if asking {
                        (format!("Kill {}?", title(o)), "It may be part-way through something; its transcript stays on disk.".to_owned())
                    } else {
                        (title(o), detail(o))
                    };
                    let buttons = if asking {
                        vec![
                            act(
                                ("orphan-kill-yes", i).into(),
                                format!("orphan-kill-yes-{i}"),
                                "Kill",
                                Box::new(move |v, cx| v.kill_one(&id, cx)),
                            ),
                            simple("orphan-kill-keep", "Keep", Vitals::keep_orphans),
                        ]
                    } else {
                        let adopt = id.clone();
                        vec![
                            act(
                                ("orphan-adopt", i).into(),
                                format!("orphan-adopt-{i}"),
                                "Adopt",
                                Box::new(move |v, cx| v.adopt(&adopt, cx)),
                            ),
                            act(("orphan-kill", i).into(), format!("orphan-kill-{i}"), "Kill", Box::new(move |v, cx| v.kill_one(&id, cx))),
                        ]
                    };
                    let text = |s: String| div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(s);
                    div()
                        .flex()
                        .items_center()
                        .gap(u(8.))
                        .px(u(6.))
                        .py(u(3.))
                        .border_t_1()
                        .border_color(p.frame)
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w_0()
                                .child(text(first).font_weight(FontWeight::BOLD))
                                .child(text(second).text_color(p.text_dim)),
                        )
                        .children(buttons)
                })
                .collect::<Vec<_>>()
        });
        let mut all = div().id("orphan-line").selector(|| "orphan-line".into()).flex().flex_col().flex_none().child(line);
        if let Some(rows) = rows {
            // Many are rare; past a few the list scrolls rather than take the board.
            all = all.child(
                div()
                    .id("orphan-rows")
                    .selector(|| "orphan-rows".into())
                    .max_h(u(160.))
                    .overflow_y_scroll()
                    .bg(p.surface)
                    .text_color(p.text)
                    .children(rows),
            );
        }
        all.into_any_element()
    }
}
