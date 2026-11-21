//! The board: the panes of the board on screen in its layout, each with its
//! chat's title bar and terminal, and the peek, a chat held up full size over
//! the board (src/features/panes/PaneGrid.tsx and Pane.tsx, without drag and
//! drop: a pane's menu moves and swaps chats instead).
//!
//! - Layouts: one pane; two side by side; one left and two stacked right;
//!   two by two. Splits are even.
//! - A pane's title bar: the chat's status, a draft, its name, model and
//!   effort, account, context and the session id to copy; its menu renames,
//!   copies the id, moves the chat to another pane or swaps it with one;
//!   `_` takes it off the board.
//! - The peek: a chat not on the board comes up over it, with To board, Put
//!   in pane N and Close; a pane of the board held up says Back. It opens
//!   with the keyboard on its frame, where Esc closes it (root.rs); a click
//!   in its terminal, or a key typed, gives the terminal the keyboard, and
//!   Esc is the CLI's from then on. Shift+Esc closes it always.
//! - Each chat's terminal (term/) is made the first time it is shown and
//!   kept while it is, and for the `WARM` chats shown last, so going back to
//!   one reattaches nothing. One whose session ended starts it anew when
//!   shown again, and takes the keyboard where the old one had it. In a new
//!   core only the shown ones start again: the warm ones are let go, as the
//!   page's were. A chat whose account is not known yet says why it waits.

use crate::models::{Models, NewEpoch};
use crate::rename::Renaming;
use crate::scale::u;
use crate::selector::Selector;
use crate::state::{Peek, WindowState};
use crate::term::TermPane;
use crate::theme::Palette;
use crate::widgets::{button, status_chip, tag};
use crate::workspace::{Live, Workspace, provider_label};
use gpui::{
    AnyElement, AppContext, Context, Entity, FocusHandle, Focusable, FontWeight, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window, deferred, div,
};
use gpui_component::input::Input;
use std::collections::{HashMap, HashSet};

/// How many terminals stay alive with no pane showing them.
const WARM: usize = 3;

/// The width a pane's title bar needs to show ID and Take off the board as
/// buttons: its status, a stretch of the name and the menu's button take the
/// rest. Narrower, they are the menu's alone, so every control stays in reach
/// whatever the window or the sidebar leaves the panes.
const ROOMY_BAR: f32 = 240.;

/// The window's width from which the peek's strip has room for its hint.
const HINT_FROM: f32 = 900.;

/// About how wide a pane is: the window less the sidebar, the board's padding
/// and the gap between the layout's columns (one for a single pane, else two),
/// at 100% (scale.rs `width`).
fn pane_width(state: &WindowState, window: &Window) -> f32 {
    let sidebar = if state.sidebar.pinned { state.sidebar.width } else { crate::sidebar::STRIP };
    let columns = if state.layout() == 1 { 1. } else { 2. };
    (crate::scale::width(window) - 2. - sidebar - 8. - 4. * (columns - 1.)) / columns
}

/// How a pane's (or the peek's) title bar looks: lit when its chat is the
/// active one, and with room (`ROOMY_BAR`) for every button and detail.
#[derive(Clone, Copy)]
struct BarLook {
    active: bool,
    roomy: bool,
}

/// Where a pane's menu is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAt {
    Pane(usize),
    /// The chat up over the board with no pane.
    Peek,
}

pub struct Board {
    models: Models,
    /// Each chat's terminal, by chat id.
    terms: HashMap<String, Entity<TermPane>>,
    /// Chats shown, the latest last.
    recent: Vec<String>,
    shown_last: HashSet<String>,
    pub(crate) menu: Option<MenuAt>,
    pub(crate) renaming: Option<Renaming>,
    pub(crate) renders: usize,
    /// The peek's frame, where the keyboard is while the peek is up and its
    /// terminal has not been given it.
    pub(crate) peek_focus: FocusHandle,
    /// The peek as last seen: one that is new takes the keyboard.
    peek_seen: Option<Peek>,
}

impl Board {
    pub fn new(models: Models, window: &mut Window, cx: &mut Context<Self>) -> Board {
        cx.observe(&models.shell, |_, _, cx| cx.notify()).detach();
        cx.observe_in(&models.workspace, window, |this: &mut Board, ws, window, cx| {
            // The terminals first: one made anew (its session ended) is the
            // one the keyboard goes to.
            let state = ws.read(cx).state.clone();
            this.sync(&state, cx);
            let request = ws.update(cx, |w, _| w.focus_request.take());
            let opened = state.peek.is_some() && state.peek != this.peek_seen;
            this.peek_seen = state.peek.clone();
            if opened {
                window.focus(&this.peek_focus, cx);
            } else if let Some(id) = request {
                this.focus_chat(&id, window, cx);
            }
            this.keep_keyboard(window, cx);
            cx.notify();
        })
        .detach();
        cx.subscribe(&models.core, |this: &mut Board, _, _: &NewEpoch, cx| this.drop_hidden(cx)).detach();
        // The peek's hint says who has Esc: drawn again as that changes.
        let peek_focus = cx.focus_handle();
        cx.on_focus(&peek_focus, window, |_, _, cx| cx.notify()).detach();
        cx.on_blur(&peek_focus, window, |_, _, cx| cx.notify()).detach();
        Board {
            peek_focus,
            models,
            terms: HashMap::new(),
            recent: Vec::new(),
            shown_last: HashSet::new(),
            menu: None,
            renaming: None,
            renders: 0,
            peek_seen: None,
        }
    }

    /// A new core: the terminals not shown are let go, not started again
    /// there (term/ `TermPane::on_core`); showing one again makes it anew.
    fn drop_hidden(&mut self, cx: &mut Context<Self>) {
        let shown = self.workspace().read(cx).state.shown();
        self.terms.retain(|id, _| shown.contains(id));
        self.recent.retain(|id| shown.contains(id));
    }

    /// A terminal has the keyboard: Esc is its CLI's (root.rs).
    pub(crate) fn terminal_has_keyboard(&self, window: &Window, cx: &gpui::App) -> bool {
        self.terms.values().any(|t| t.read(cx).focus_handle(cx).is_focused(window))
    }

    fn workspace(&self) -> &Entity<Workspace> {
        &self.models.workspace
    }

    /// The terminal of chat `id`, made if it has none; why there is none
    /// yet otherwise.
    fn terminal(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<Entity<TermPane>, String> {
        if let Some(t) = self.terms.get(id) {
            return Ok(t.clone());
        }
        let spec = self.workspace().read(cx).spec_of(id)?;
        let models = self.models.clone();
        let term = cx.new(|cx| TermPane::new(models, spec, window, cx));
        self.terms.insert(id.to_owned(), term.clone());
        Ok(term)
    }

    /// The terminal in pane `pane` of the board on screen, if one is made.
    pub(crate) fn terminal_in(&self, pane: usize, cx: &gpui::App) -> Option<Entity<TermPane>> {
        let state = &self.workspace().read(cx).state;
        state.seat(pane).and_then(|id| self.terms.get(id).cloned())
    }

    /// The terminal of chat `id`, if one is made.
    #[cfg(any(test, feature = "shots"))]
    pub(crate) fn terminal_of(&self, id: &str) -> Option<Entity<TermPane>> {
        self.terms.get(id).cloned()
    }

    /// Every terminal made, by chat id.
    #[cfg(test)]
    pub(crate) fn terms_for_tests(&self) -> Vec<(String, Entity<TermPane>)> {
        self.terms.iter().map(|(id, t)| (id.clone(), t.clone())).collect()
    }

    /// The keyboard into chat `id`'s terminal.
    pub fn focus_chat(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Ok(term) = self.terminal(id, window, cx) {
            term.update(cx, |t, cx| t.focus(window, cx));
        }
    }

    /// The keyboard in a terminal the board no longer shows, or nowhere, goes
    /// back to the window's own focus, so the window's keys still reach it.
    fn keep_keyboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = &self.workspace().read(cx).state;
        let shown = state.shown();
        let hidden = self.terms.iter().any(|(id, t)| !shown.contains(id) && t.read(cx).focus_handle(cx).is_focused(window));
        let peek_gone = state.peek.is_none() && self.peek_focus.is_focused(window);
        if hidden || peek_gone || window.focused(cx).is_none() {
            window.focus(&self.models.home, cx);
        }
    }

    /// Esc: an open menu closes, then a name being edited is let go. False
    /// when there was neither.
    pub fn escape(&mut self, cx: &mut Context<Self>) -> bool {
        if self.menu.take().is_some() || self.renaming.take().is_some() {
            cx.notify();
            return true;
        }
        false
    }

    /// The terminals kept to what is shown and the warm few; a chat shown
    /// again after its session ended gets a new one.
    fn sync(&mut self, state: &WindowState, cx: &mut Context<Self>) {
        let shown = state.shown();
        let ws = self.workspace().clone();
        for id in &shown {
            if !self.shown_last.contains(id) && ws.update(cx, |w, _| w.take_ended(id)) {
                self.terms.remove(id);
            }
            self.recent.retain(|r| r != id);
            self.recent.push(id.clone());
        }
        self.recent.retain(|id| state.find(id).is_some());
        let warm: Vec<&String> = self.recent.iter().rev().filter(|id| !shown.contains(id)).take(WARM).collect();
        let keep: HashSet<String> = shown.iter().cloned().chain(warm.into_iter().cloned()).collect();
        self.terms.retain(|id, _| keep.contains(id));
        for (id, term) in &self.terms {
            let on = shown.contains(id);
            if term.read(cx).shown != on {
                term.update(cx, |t, cx| t.set_shown(on, cx));
            }
        }
        self.shown_last = shown.into_iter().collect();
    }

    pub(crate) fn start_rename(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.workspace().read(cx).state.find(id).map(|c| c.name.clone()) else { return };
        self.menu = None;
        self.renaming = Some(Renaming::start(id, &name, self.workspace().clone(), |b: &mut Board| &mut b.renaming, window, cx));
        cx.notify();
    }

    fn copy_id(&self, id: &str, cx: &mut Context<Self>) {
        let Some(session) = self.workspace().read(cx).state.find(id).and_then(|c| c.session_id.clone()) else { return };
        let written = self.models.host.write_clipboard_text(session);
        cx.background_spawn(async move {
            if let Err(e) = written.await {
                log::warn!("copy the session id: {e}");
            }
        })
        .detach();
    }

    /// A pane's (or the peek's) title bar for chat `id`.
    fn title_bar(&self, p: &Palette, state: &WindowState, at: MenuAt, id: &str, look: BarLook, cx: &mut Context<Self>) -> AnyElement {
        let BarLook { active, roomy } = look;
        let ws = self.workspace().read(cx);
        let Some(chat) = state.find(id) else { return div().into_any_element() };
        let live: Live = ws.live(id);
        let status = ws.shown_status(chat, cx);
        let (bg, fg) = if active { (p.title_bg, p.title_fg) } else { (p.text_dim, p.surface) };
        let key = match at {
            MenuAt::Pane(i) => format!("pane-{}", i + 1),
            MenuAt::Peek => "peek".into(),
        };
        let name: AnyElement = match &self.renaming {
            Some(r) if r.chat == id => div().flex_1().min_w_0().child(Input::new(&r.input).h(u(18.))).into_any_element(),
            // Enough of the name to tell chats apart, and no more: the bar's
            // buttons stay in reach in two panes side by side at the window's
            // minimum width (fit_tests.rs).
            _ => div()
                .flex_1()
                .min_w(u(if roomy { 40. } else { 24. }))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .font_weight(FontWeight::BOLD)
                .child(SharedString::from(chat.name.clone()))
                .into_any_element(),
        };
        // What says more about the chat gives way first: it shrinks, cut short,
        // before the name does.
        let detail = |s: String| div().flex_shrink(1.).min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(s);
        let context = live.context.map(|c| format!("ctx {:.0}%", (c * 100.).clamp(0., 100.)));
        let chat_id = id.to_owned();
        let menu_open = self.menu == Some(at);
        let mut bar = div()
            .id(SharedString::from(format!("{key}-bar")))
            .relative()
            .flex()
            .flex_none()
            .min_w_0()
            .items_center()
            .gap(u(4.))
            .px(u(4.))
            .h(u(24.))
            .bg(bg)
            .text_color(fg)
            .border_b_1()
            .border_color(p.frame)
            .child(status_chip(p, status))
            .children(live.draft_tag().map(|t| tag(p, Some(p.attention), t)))
            .child(name)
            .children(roomy.then(|| detail(chat.model_effort())))
            .children(roomy.then(|| detail(format!("{}: {}", provider_label(chat.provider), chat.account))))
            .children(context.filter(|_| roomy).map(detail));
        // A bar with no room leaves what the chat's details say out, and ID
        // and Take off the board to the menu, which has both.
        if chat.session_id.is_some() && roomy {
            let copy = chat_id.clone();
            bar = bar.child(
                div()
                    .id(SharedString::from(format!("{key}-copy-id")))
                    .selector({
                        let key = key.clone();
                        move || format!("{key}-copy-id")
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.copy_id(&copy, cx)))
                    .child(button(p, "ID")),
            );
        }
        bar = bar.child(
            div()
                .id(SharedString::from(format!("{key}-menu")))
                .selector({
                    let key = key.clone();
                    move || format!("{key}-menu")
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.menu = if this.menu == Some(at) { None } else { Some(at) };
                    cx.notify();
                }))
                .child(button(p, "...")),
        );
        if let (MenuAt::Pane(i), true) = (at, roomy) {
            bar = bar.child(
                div()
                    .id(SharedString::from(format!("{key}-unseat")))
                    .selector({
                        let key = key.clone();
                        move || format!("{key}-unseat")
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.workspace().clone().update(cx, |w, cx| w.unseat(i, cx));
                    }))
                    .child(button(p, "_")),
            );
        }
        if menu_open {
            bar = bar.child(deferred(self.menu_el(p, state, at, &chat_id, cx)).with_priority(1));
        }
        bar.into_any_element()
    }

    /// The menu under a title bar's `...`.
    fn menu_el(&self, p: &Palette, state: &WindowState, at: MenuAt, id: &str, cx: &mut Context<Self>) -> AnyElement {
        let layout = state.layout();
        let mut items: Vec<(String, String, Box<dyn Fn(&mut Board, &mut Window, &mut Context<Board>)>)> = Vec::new();
        let (rename_id, copy_id) = (id.to_owned(), id.to_owned());
        items.push(("rename".into(), "Rename".into(), Box::new(move |b, window, cx| b.start_rename(&rename_id, window, cx))));
        if state.find(id).is_some_and(|c| c.session_id.is_some()) {
            items.push(("copy-id".into(), "Copy session id".into(), Box::new(move |b, _, cx| b.copy_id(&copy_id, cx))));
        }
        match at {
            MenuAt::Pane(i) => {
                for n in (0..layout).filter(|n| *n != i) {
                    let chat = id.to_owned();
                    items.push((
                        format!("move-{}", n + 1),
                        format!("Move to pane {}", n + 1),
                        Box::new(move |b, _, cx| b.workspace().clone().update(cx, |w, cx| w.put(n, &chat, cx))),
                    ));
                }
                for n in (0..layout).filter(|n| *n != i) {
                    items.push((
                        format!("swap-{}", n + 1),
                        format!("Swap with pane {}", n + 1),
                        Box::new(move |b, _, cx| b.workspace().clone().update(cx, |w, cx| w.swap(i, n, cx))),
                    ));
                }
                items.push((
                    "unseat".into(),
                    "Take off the board".into(),
                    Box::new(move |b, _, cx| b.workspace().clone().update(cx, |w, cx| w.unseat(i, cx))),
                ));
            }
            MenuAt::Peek => {
                for n in 0..layout {
                    let chat = id.to_owned();
                    items.push((
                        format!("put-{}", n + 1),
                        format!("Put in pane {}", n + 1),
                        Box::new(move |b, _, cx| b.workspace().clone().update(cx, |w, cx| w.put(n, &chat, cx))),
                    ));
                }
            }
        }
        let rows: Vec<AnyElement> = items
            .into_iter()
            .map(|(key, label, run)| {
                div()
                    .id(SharedString::from(format!("menu-{key}")))
                    .selector(move || format!("menu-{key}"))
                    .px(u(8.))
                    .py(u(2.))
                    .bg(p.surface)
                    .text_color(p.text)
                    .whitespace_nowrap()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.menu = None;
                        run(this, window, cx);
                        cx.notify();
                    }))
                    .child(label)
                    .into_any_element()
            })
            .collect();
        div()
            .id("pane-menu")
            .absolute()
            .top(u(23.))
            .right(u(0.))
            .flex()
            .flex_col()
            .py(u(2.))
            .bg(p.surface)
            .border_1()
            // The lit edge: a black frame would vanish on the dark board.
            .border_color(p.hi)
            .occlude()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.menu = None;
                cx.notify();
            }))
            .children(rows)
            .into_any_element()
    }

    /// A terminal's place: the terminal, or why there is none yet.
    fn body(&mut self, p: &Palette, id: &str, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match self.terminal(id, window, cx) {
            // Not a cached view of its own: one reused from an earlier frame
            // under a board laid out anew lost its place on the keyboard's
            // path. The board is cached; the terminal redraws with it.
            Ok(term) => div().flex_1().min_h_0().child(term).into_any_element(),
            Err(why) => div().flex_1().min_h_0().p(u(8.)).bg(p.well).text_color(p.text).child(why).into_any_element(),
        }
    }

    fn pane(&mut self, p: &Palette, state: &WindowState, i: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let ws = self.workspace().clone();
        let frame = div()
            .id(SharedString::from(format!("pane-{}", i + 1)))
            .selector(move || format!("pane-{}", i + 1))
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .border_1()
            .border_color(p.frame)
            .bg(p.surface)
            .capture_any_mouse_down(move |_, _, cx| ws.update(cx, |w, cx| w.set_active_pane(i, cx)));
        let Some(id) = state.seat(i).map(str::to_owned) else {
            // The hint wraps within its pane: a narrow pane would otherwise
            // push it out over its neighbors.
            let hint = format!("Pane {} of {}: empty. A chat's row shows it; a pane's menu moves one here.", i + 1, state.layout());
            return frame
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .flex()
                        .items_center()
                        .justify_center()
                        .p(u(8.))
                        .text_color(p.text_dim)
                        .child(div().w_full().text_center().child(hint)),
                )
                .into_any_element();
        };
        let roomy = pane_width(state, window) >= ROOMY_BAR;
        let bar =
            self.title_bar(p, state, MenuAt::Pane(i), &id, BarLook { active: state.active_pane == i && state.peek.is_none(), roomy }, cx);
        let body = if state.peek == Some(Peek::Pane(i)) {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .bg(p.well)
                .text_color(p.text_dim)
                .child("Held up over the board: Esc puts it back.")
                .into_any_element()
        } else {
            self.body(p, &id, window, cx)
        };
        frame.child(bar).child(body).into_any_element()
    }

    /// The sheet over the board.
    fn peek(&mut self, p: &Palette, state: &WindowState, peek: Peek, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let ws = self.workspace().clone();
        let (id, at) = match &peek {
            Peek::Chat(id) => (id.clone(), MenuAt::Peek),
            Peek::Pane(i) => match state.seat(*i) {
                Some(id) => (id.to_owned(), MenuAt::Pane(*i)),
                None => return div().into_any_element(),
            },
        };
        let close = {
            let ws = ws.clone();
            div().id("peek-close").selector(|| "peek-close".into()).on_click(move |_, _, cx| {
                ws.update(cx, |w, cx| w.close_peek(cx));
            })
        };
        // Who has Esc, said where the person looks for it.
        let hint = if self.terms.get(&id).is_some_and(|t| t.read(cx).focus_handle(cx).is_focused(window)) {
            "Esc goes to the CLI; Shift+Esc closes"
        } else {
            "Esc closes; click or type in it to give it the keyboard"
        };
        // The hint is the first to go: it takes what the strip's words and
        // buttons leave, and where that is little (a window under the
        // minimum) it is left out rather than squeezed to nothing.
        let hint = (window.viewport_size().width >= u(HINT_FROM)).then(|| {
            div()
                .id("peek-hint")
                .selector(|| "peek-hint".into())
                .flex_shrink(1.)
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(hint)
        });
        // What the strip says first, cut short with an ellipsis before its
        // buttons leave the window.
        let lead = |text: String| {
            div()
                .flex_shrink(1.)
                .min_w(u(40.))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .font_weight(FontWeight::BOLD)
                .child(SharedString::from(text))
        };
        let strip = match &peek {
            Peek::Chat(_) => {
                let to_board = {
                    let (ws, id) = (ws.clone(), id.clone());
                    div()
                        .id("peek-to-board")
                        .selector(|| "peek-to-board".into())
                        .on_click(move |_, _, cx| ws.update(cx, |w, cx| w.to_board(&id, cx)))
                        .child(button(p, "To board"))
                };
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(u(4.))
                    .px(u(4.))
                    .h(u(26.))
                    .bg(p.info)
                    .text_color(p.on_status)
                    .border_b_1()
                    .border_color(p.frame)
                    .child(lead("Peek: this chat is not on the board.".to_owned()))
                    .children(hint.map(|hint| div().flex_1().min_w_0().child(hint)))
                    .child(to_board)
                    .child(close.child(button(p, "Close")))
            }
            Peek::Pane(i) => div()
                .flex()
                .flex_none()
                .items_center()
                .gap(u(4.))
                .px(u(4.))
                .h(u(26.))
                .bg(p.info)
                .text_color(p.on_status)
                .border_b_1()
                .border_color(p.frame)
                .child(lead(format!("Pane {} held up over the board.", i + 1)))
                .children(hint.map(|hint| div().flex_1().min_w_0().child(hint)))
                .child(close.child(button(p, "Back"))),
        };
        let bar = self.title_bar(p, state, at, &id, BarLook { active: true, roomy: true }, cx);
        let body = self.body(p, &id, window, cx);
        let typed_to = id.clone();
        div()
            .id("peek")
            .selector(|| "peek".into())
            .track_focus(&self.peek_focus)
            .on_key_down(cx.listener(move |this, ev: &KeyDownEvent, window, cx| {
                // Only what reaches the frame itself: a key its terminal
                // left alone bubbles here too. Esc is root.rs's.
                if !this.peek_focus.is_focused(window) || ev.keystroke.key == "escape" {
                    return;
                }
                if let Some(term) = this.terms.get(&typed_to).cloned() {
                    cx.stop_propagation();
                    term.update(cx, |t, cx| t.forward_key(ev, window, cx));
                    cx.notify();
                }
            }))
            .absolute()
            .top(u(0.))
            .left(u(0.))
            .right(u(0.))
            .bottom(u(0.))
            .flex()
            .flex_col()
            .bg(p.surface)
            .border_2()
            // The lit edge, which the dark board does not swallow.
            .border_color(p.hi)
            .occlude()
            .child(strip)
            .child(bar)
            .child(body)
            .into_any_element()
    }
}

impl Render for Board {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let p = self.models.shell.read(cx).palette();
        let state = self.workspace().read(cx).state.clone();
        self.sync(&state, cx);
        let layout = state.layout();
        let mut panes: Vec<AnyElement> = (0..layout).map(|i| self.pane(&p, &state, i, window, cx)).collect();
        let row = || div().flex().flex_1().min_w_0().min_h_0().gap(u(4.));
        let col = || div().flex().flex_col().flex_1().min_w_0().min_h_0().gap(u(4.));
        let mut take = |i: usize| std::mem::replace(&mut panes[i], div().into_any_element());
        let grid = match layout {
            1 => row().child(take(0)),
            2 => row().child(take(0)).child(take(1)),
            3 => row().child(take(0)).child(col().child(take(1)).child(take(2))),
            _ => col().child(row().child(take(0)).child(take(1))).child(row().child(take(2)).child(take(3))),
        };
        let peek = state.peek.clone().map(|peek| self.peek(&p, &state, peek, window, cx));
        div().id("board").relative().size_full().flex().p(u(4.)).bg(p.board).child(grid).children(peek)
    }
}
