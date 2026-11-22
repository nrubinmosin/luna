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
    AnyElement, App, Context, ElementId, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, Window, div,
};

/// What a button of the line does.
type Act = Box<dyn Fn(&mut Vitals, &mut Context<Vitals>)>;

/// A button as the line draws it: its element id, its selector, its label
/// and what a click on it does.
struct Button {
    id: ElementId,
    name: String,
    label: &'static str,
    act: Act,
}

impl Button {
    fn line(name: &'static str, label: &'static str, act: fn(&mut Vitals, &mut Context<Vitals>)) -> Button {
        Button { id: name.into(), name: name.into(), label, act: Box::new(act) }
    }

    fn row(name: &'static str, i: usize, label: &'static str, act: Act) -> Button {
        Button { id: (name, i).into(), name: format!("{name}-{i}"), label, act }
    }
}

/// The line's own buttons as it draws them now: Show (or Hide) and Kill
/// all, or, once Kill all has asked, Kill and Keep.
fn line_buttons(v: &Vitals) -> Vec<Button> {
    if v.kill_asked() {
        vec![Button::line("orphans-kill", "Kill", Vitals::kill_all), Button::line("orphans-keep", "Keep", Vitals::keep_orphans)]
    } else {
        vec![
            Button::line("orphans-show", if v.orphans_open() { "Hide" } else { "Show" }, Vitals::toggle_orphans),
            Button::line("orphans-kill-all", "Kill all", Vitals::kill_all),
        ]
    }
}

/// Row `i`'s buttons, for orphan `o`: Adopt where a chat could reach it,
/// and Kill; or, once that Kill has asked (`asking`), Kill and Keep.
fn row_buttons(v: &Vitals, i: usize, o: &Orphan, asking: bool, cx: &App) -> Vec<Button> {
    let id = o.id.clone();
    if asking {
        return vec![
            Button::row("orphan-kill-yes", i, "Kill", Box::new(move |v, cx| v.kill_one(&id, cx))),
            Button::line("orphan-kill-keep", "Keep", Vitals::keep_orphans),
        ];
    }
    let mut buttons = Vec::new();
    if v.adoptable(o, cx) {
        let adopt = id.clone();
        buttons.push(Button::row("orphan-adopt", i, "Adopt", Box::new(move |v, cx| v.adopt(&adopt, cx))));
    }
    buttons.push(Button::row("orphan-kill", i, "Kill", Box::new(move |v, cx| v.kill_one(&id, cx))));
    buttons
}

/// Every button the line would draw now, its own and, while they are
/// listed, the rows'.
fn all_buttons(v: &Vitals, cx: &App) -> Vec<Button> {
    if v.orphans().is_empty() {
        return Vec::new();
    }
    let mut all = line_buttons(v);
    if v.orphans_open() {
        let asked = v.kill_one_asked();
        for (i, o) in v.orphans().iter().enumerate() {
            all.extend(row_buttons(v, i, o, asked == Some(o.id.as_str()), cx));
        }
    }
    all
}

/// The test build's driver (drive.rs `orphans ...`): the button of that
/// name pressed, as a click on it would, only if the last frame drew it and
/// the line would draw it still. `show`, `kill-all`, `kill`, `keep` are the
/// line's own (`orphans-<name>`); `adopt 2`, `kill 2`, `kill-yes 2` the
/// second row's (`orphan-<name>-1`); `kill-keep` the Keep of a row's
/// question. False otherwise, so a script cannot press what a person could
/// not have seen: a kill's Kill before its question is on screen, a row's
/// button before Show has listed it, or a button gone since.
#[cfg(any(luna_test, test))]
pub(crate) fn press(line: &Entity<OrphanLine>, name: &str, cx: &mut App) -> bool {
    let selector = match name.split_whitespace().collect::<Vec<_>>().as_slice() {
        ["kill-keep"] => "orphan-kill-keep".to_owned(),
        [button] => format!("orphans-{button}"),
        [button, n] => match n.parse::<usize>() {
            Ok(n) if n > 0 => format!("orphan-{button}-{}", n - 1),
            _ => return false,
        },
        _ => return false,
    };
    let line = line.read(cx);
    if !line.drawn.contains(&selector) {
        return false;
    }
    let vitals = line.vitals.clone();
    let Some(button) = all_buttons(vitals.read(cx), cx).into_iter().find(|b| b.name == selector) else { return false };
    vitals.update(cx, |v, cx| (button.act)(v, cx));
    true
}

pub struct OrphanLine {
    models: Models,
    vitals: Entity<Vitals>,
    /// The buttons the last frame drew, by selector (`press`).
    drawn: Vec<String>,
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
        OrphanLine { models, vitals, drawn: Vec::new() }
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
pub(crate) fn title(o: &Orphan) -> String {
    match o.title.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => t.to_owned(),
        None => format!("session {}", o.id.chars().skip(o.id.chars().count().saturating_sub(6)).collect::<String>()),
    }
}

impl Render for OrphanLine {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let v = self.vitals.read(cx);
        let n = v.orphans().len();
        self.drawn = all_buttons(v, cx).into_iter().map(|b| b.name).collect();
        if n == 0 {
            return div().into_any_element();
        }
        let p = self.models.shell.read(cx).palette();
        let (asked, open, plural) = (v.kill_asked(), v.orphans_open(), if n == 1 { "" } else { "s" });
        let act = |b: Button| -> AnyElement {
            let Button { id, name, label, act } = b;
            let vitals = self.vitals.clone();
            div()
                .id(id)
                .selector(move || name)
                .flex_none()
                .on_click(move |_, _, cx| vitals.update(cx, |v, cx| act(v, cx)))
                .child(button(&p, label))
                .into_any_element()
        };
        let text = if asked {
            format!(
                "Kill {n} running session{plural} that no chat reaches? Each may be part-way through something; its transcript stays on disk."
            )
        } else {
            format!("{n} orphan session{plural}: running with no chat to reach {}", if n == 1 { "it" } else { "them" })
        };
        let buttons: Vec<AnyElement> = line_buttons(v).into_iter().map(&act).collect();
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
                    let asking = kill_one == Some(o.id.as_str());
                    let (first, second) = if asking {
                        (format!("Kill {}?", title(o)), "It may be part-way through something; its transcript stays on disk.".to_owned())
                    } else if v.adoptable(o, cx) {
                        (title(o), detail(o))
                    } else {
                        // Adopted, it could not reach the session: a chat
                        // on another account cannot start it.
                        (title(o), format!("{} · not on an account Luna lists", detail(o)))
                    };
                    let buttons: Vec<AnyElement> = row_buttons(v, i, o, asking, cx).into_iter().map(&act).collect();
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
