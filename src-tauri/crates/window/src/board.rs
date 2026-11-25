//! The board: the panes of the board on screen in its layout, each with its
//! chat's title bar and terminal, and the peek, a chat held up full size over
//! the board (src/features/panes/PaneGrid.tsx and Pane.tsx, without drag and
//! drop: a pane's menu moves and swaps chats instead).
//!
//! - Layouts: one pane; two side by side; one left and two stacked right;
//!   two columns of two. Each divider drags (`Held`, `follow`), and is drawn
//!   held to a share that leaves either pane room; where it is let go is
//!   kept per layout of the board (state.rs `Splits`). While one is dragged
//!   the panes follow it, their PTYs are told their size at most every
//!   `HELD_GAP` (term/io.rs), and the state is written once, as it is let
//!   go. Another board or layout under a drag ends it, keeping nothing.
//! - A pane's title bar: the chat's status, a draft, its name, model and
//!   effort, account, launch folder, context and the session id to copy; its
//!   menu renames, copies the id, moves the chat to another pane or swaps it
//!   with one; `_` takes it off the board.
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
use crate::state::{Chat, Peek, SPLIT_MAX, SPLIT_MIN, Split, Splits, WindowState, folder_leaf};
use crate::term::TermPane;
use crate::theme::Palette;
use crate::widgets::{Tipped, button, status_chip, tag, tips_on};
use crate::workspace::{Live, Workspace, provider_label};
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, AppContext, Bounds, Context, CursorStyle, DispatchPhase, Entity, FocusHandle, Focusable, FontWeight,
    InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point,
    Render, SharedString, Size, StatefulInteractiveElement, Styled, Window, canvas, deferred, div, px, relative,
};
use gpui_component::input::Input;
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// How many terminals stay alive with no pane showing them.
const WARM: usize = 3;

/// The width a pane's title bar needs to show ID and Take off the board as
/// buttons: its status, a stretch of the name and the menu's button take the
/// rest. Narrower, they are the menu's alone, so every control stays in reach
/// whatever the window or the sidebar leaves the panes.
const ROOMY_BAR: f32 = 240.;

/// The width from which the bar has room for Delete (×) too, beside ID and
/// Take off the board; narrower, Delete is the menu's alone. A pane of two
/// side by side in a 900 px window, sidebar pinned, is about 293 px wide:
/// its ID and _ fit, the × would not.
const DELETE_BAR: f32 = 310.;

/// A row of a pane's menu as laid out at 100%: a line of text and its
/// padding, for whether the menu fits below its bar.
const MENU_ROW: f32 = 23.;

/// The least a pane's menu keeps from the window's edges.
const MENU_MARGIN: f32 = 4.;

/// The window's width from which the peek's strip has room for its hint.
const HINT_FROM: f32 = 900.;

/// The divider between panes, as thick as the gap it stands in.
const DIVIDER: f32 = 4.;

/// The least a divider leaves the pane on either side of it, where the row
/// or column has room for two: the title bar's controls across, the bar and
/// a few lines down. Held to as the board is laid out, so a window made
/// smaller narrows the wider pane first; the share kept stays as it was.
const MIN_PANE_W: f32 = 160.;
const MIN_PANE_H: f32 = 120.;

/// The span a divider splits (the board's inner width for `Split::Col`, its
/// height for the others: every column is as high as the board) and the
/// least either side keeps of it.
fn span_of(split: Split, inner: Size<Pixels>) -> (Pixels, f32) {
    if split == Split::Col { (inner.width, MIN_PANE_W) } else { (inner.height, MIN_PANE_H) }
}

/// `share` held so that either side keeps `least` of `span` less the
/// divider, where there is room for both; else even at most.
fn held_to(share: f32, span: Pixels, least: f32) -> f32 {
    let room = span - u(DIVIDER);
    if room <= px(0.) {
        return share;
    }
    let floor = (u(least) / room).min(0.5);
    share.clamp(floor, 1. - floor)
}

struct BarDetail {
    label: String,
    tip: Option<(&'static str, Vec<SharedString>)>,
}

/// Each detail after the first carries its own dot, which shrinks and is
/// cut short with it.
fn bar_details(chat: &Chat, folder: Option<&str>, live: &Live) -> Vec<BarDetail> {
    let launch_folder = chat.worktree_path.as_deref().filter(|p| !p.is_empty()).or(folder).filter(|p| !p.is_empty());
    let parts = [
        Some((chat.model_effort(), None)),
        Some((format!("{}: {}", provider_label(chat.provider), chat.account), None)),
        launch_folder.map(|path| (folder_leaf(path).to_owned(), Some(("folder", vec![path.into()])))),
        live.context.map(|c| {
            (format!("ctx {:.0}%", (c * 100.).clamp(0., 100.)), Some(("ctx", ctx_tip(live.context_auto_compact, live.to_compact))))
        }),
    ];
    parts
        .into_iter()
        .flatten()
        .filter(|(label, _)| !label.is_empty())
        .enumerate()
        .map(|(i, (label, tip))| BarDetail { label: if i == 0 { label } else { format!("· {label}") }, tip })
        .collect()
}

/// Which side of its bar (where it was last laid out, in a window that
/// high) a menu `want` tall opens on, above or not, and the most it may take
/// there: below where it fits or where there is no more room above, else
/// above. Before the bar has been laid out, below, as tall as it is.
fn menu_side(laid: Option<(Bounds<Pixels>, Pixels)>, want: Pixels) -> (bool, Pixels) {
    let Some((bar, window)) = laid else { return (false, want) };
    let below = window - bar.bottom() - u(MENU_MARGIN);
    let above = bar.top() - u(MENU_MARGIN);
    if want <= below || below >= above { (false, below.max(px(0.))) } else { (true, above.max(px(0.))) }
}

/// About how wide pane `i` is: the window less the sidebar and the board's
/// padding, and with two columns less the divider, by its column's share, at
/// 100% (scale.rs `width`).
fn pane_width(state: &WindowState, splits: Splits, i: usize, window: &Window) -> f32 {
    let sidebar = if state.sidebar.pinned { state.sidebar.width } else { crate::sidebar::STRIP };
    let inner = crate::scale::width(window) - 2. - sidebar - 8.;
    if state.layout() == 1 {
        return inner;
    }
    let left = i == 0 || (state.layout() == 4 && i == 2);
    (inner - DIVIDER) * if left { splits.col } else { 1. - splits.col }
}

/// A divider held down: the board, layout and divider it was pressed on,
/// where the pointer was then and the share drawn then, and the share it
/// gives now. Not gpui's drag, which starts only in the bubble phase of the
/// move that crosses its threshold, so a press, one move and the release
/// would move nothing: the window's own listeners (`follow`) take every
/// move and the release from the press on, wherever the pointer is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Held {
    board: usize,
    layout: usize,
    split: Split,
    from: Point<Pixels>,
    from_share: f32,
    share: f32,
    moved: bool,
}

/// How a pane's (or the peek's) title bar looks: lit when its chat is the
/// active one, with room (`ROOMY_BAR`) for its details, ID and Take off the
/// board, and with room (`DELETE_BAR`) for Delete too.
#[derive(Clone, Copy)]
struct BarLook {
    active: bool,
    roomy: bool,
    delete: bool,
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
    /// The divider being dragged; the state takes its share as it is let go.
    pub(crate) held: Option<Held>,
    /// The board's panes' area as last laid out (`span_of`), what a drag
    /// moves through and the least a pane keeps is held to.
    inner: Rc<Cell<Option<Size<Pixels>>>>,
    /// Whether this draw offers tooltips (widgets.rs `tips_on`).
    tips: bool,
    /// Where the bar whose menu is open was last laid out, in the window,
    /// and the window's height then: the menu opens above it where it does
    /// not fit below and there is more room above.
    menu_bar: Rc<Cell<Option<(Bounds<Pixels>, Pixels)>>>,
}

impl Board {
    pub fn new(models: Models, window: &mut Window, cx: &mut Context<Self>) -> Board {
        cx.observe(&models.shell, |_, _, cx| cx.notify()).detach();
        cx.observe_in(&models.workspace, window, |this: &mut Board, ws, window, cx| {
            // The terminals first: one made anew (its session ended) is the
            // one the keyboard goes to.
            let state = ws.read(cx).state.clone();
            let dropped = this.sync(&state, cx);
            this.drop_stale_hold(state.board, state.layout());
            let request = ws.update(cx, |w, _| w.focus_request.take());
            let opened = state.peek.is_some() && state.peek != this.peek_seen;
            this.peek_seen = state.peek.clone();
            if opened {
                window.focus(&this.peek_focus, cx);
            } else if let Some(id) = request {
                this.focus_chat(&id, window, cx);
            }
            this.keep_keyboard(&dropped, window, cx);
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
            held: None,
            inner: Rc::default(),
            tips: true,
            menu_bar: Rc::default(),
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
    pub(crate) fn terminal_has_keyboard(&self, window: &Window, cx: &App) -> bool {
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
    pub(crate) fn terminal_in(&self, pane: usize, cx: &App) -> Option<Entity<TermPane>> {
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

    /// The keyboard in a terminal the board no longer shows, one `sync` just
    /// let go of (`dropped`) included, or nowhere, goes back to the window's
    /// own focus, so the window's keys still reach it. Left in a terminal
    /// that is gone, gpui would send every key from the window's root, above
    /// the window's key context, and none of its keys would work.
    fn keep_keyboard(&mut self, dropped: &[Entity<TermPane>], window: &mut Window, cx: &mut Context<Self>) {
        let state = &self.workspace().read(cx).state;
        let shown = state.shown();
        let hidden = self.terms.iter().any(|(id, t)| !shown.contains(id) && t.read(cx).focus_handle(cx).is_focused(window))
            || dropped.iter().any(|t| t.read(cx).focus_handle(cx).is_focused(window));
        // A peek closed, or one of a pane left empty, holds nothing up.
        let peek_gone = state.peeked_chat().is_none() && self.peek_focus.is_focused(window);
        if hidden || peek_gone || window.focused(cx).is_none() {
            window.focus(&self.models.home, cx);
        }
    }

    /// Esc: an open menu closes, then a name being edited is let go, the
    /// keyboard back where it belongs. False when there was neither.
    pub fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.menu.take().is_some() {
            cx.notify();
            return true;
        }
        if let Some(r) = self.renaming.take() {
            r.give_back(window, cx);
            cx.notify();
            return true;
        }
        false
    }

    /// The terminals kept to what is shown and the warm few; a chat shown
    /// again after its session ended gets a new one. Returns the terminals
    /// let go of, for the caller to see whether one had the keyboard.
    fn sync(&mut self, state: &WindowState, cx: &mut Context<Self>) -> Vec<Entity<TermPane>> {
        let shown = state.shown();
        let ws = self.workspace().clone();
        let mut dropped = Vec::new();
        for id in &shown {
            if !self.shown_last.contains(id) && ws.update(cx, |w, _| w.take_ended(id)) {
                dropped.extend(self.terms.remove(id));
            }
            self.recent.retain(|r| r != id);
            self.recent.push(id.clone());
        }
        self.recent.retain(|id| state.find(id).is_some());
        let warm: Vec<&String> = self.recent.iter().rev().filter(|id| !shown.contains(id)).take(WARM).collect();
        let keep: HashSet<String> = shown.iter().cloned().chain(warm.into_iter().cloned()).collect();
        let gone: Vec<String> = self.terms.keys().filter(|id| !keep.contains(*id)).cloned().collect();
        dropped.extend(gone.iter().filter_map(|id| self.terms.remove(id)));
        for (id, term) in &self.terms {
            let on = shown.contains(id);
            if term.read(cx).shown != on {
                term.update(cx, |t, cx| t.set_shown(on, cx));
            }
        }
        self.shown_last = shown.into_iter().collect();
        dropped
    }

    pub(crate) fn start_rename(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.workspace().read(cx).state.find(id).map(|c| c.name.clone()) else { return };
        self.menu = None;
        let (ws, home) = (self.workspace().clone(), self.models.home.clone());
        self.renaming = Some(Renaming::start(id, &name, ws, home, |b: &mut Board| &mut b.renaming, window, cx));
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

    /// The delete dialog's question for chat `id`, the one gate a Delete
    /// goes through: nothing goes before it is answered.
    fn ask_delete(&self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let seam = self.workspace().read(cx).seams.delete_chat.clone();
        match seam {
            Some(seam) => seam(id.to_owned(), window, cx),
            None => log::warn!("delete chat: the delete dialog is not in this build"),
        }
    }

    /// A pane's (or the peek's) title bar for chat `id`.
    fn title_bar(&self, p: &Palette, state: &WindowState, at: MenuAt, id: &str, look: BarLook, cx: &mut Context<Self>) -> AnyElement {
        let BarLook { active, roomy, delete } = look;
        let ws = self.workspace().read(cx);
        let Some(chat) = state.find(id) else { return div().into_any_element() };
        let live: Live = ws.live(id);
        let status = ws.shown_status(chat, cx);
        let (bg, fg) = if active { (p.title_bg, p.title_fg) } else { (p.text_dim, p.surface) };
        let key = match at {
            MenuAt::Pane(i) => format!("pane-{}", i + 1),
            MenuAt::Peek => "peek".into(),
        };
        let renaming = self.renaming.as_ref().filter(|r| r.chat == id);
        let name_min = u(if roomy { 40. } else { 24. });
        let name: AnyElement = match renaming {
            Some(r) => div()
                .debug_selector(|| format!("{key}-rename"))
                .flex_1()
                .min_w(name_min)
                .child(Input::new(&r.input).h(u(18.)))
                .into_any_element(),
            None => div()
                .debug_selector(|| format!("{key}-name"))
                .flex_1()
                .min_w(name_min)
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .font_weight(FontWeight::BOLD)
                .child(SharedString::from(chat.name.clone()))
                .into_any_element(),
        };
        // The inline editor gets the room normally shared by the details.
        let details = (roomy && renaming.is_none()).then(|| {
            div().flex().flex_shrink(1.).min_w_0().overflow_hidden().items_center().gap(u(4.)).children(
                bar_details(chat, state.folder_of(id).map(|f| f.path.as_str()), &live).into_iter().map(|BarDetail { label, tip }| {
                    let detail = div().flex_shrink(1.).min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(label);
                    match tip {
                        Some((suffix, lines)) => detail
                            // Give the folder the room left by the other details.
                            .when(suffix == "folder", |d| d.flex_basis(relative(0.)).flex_grow(1.))
                            .id(SharedString::from(format!("{key}-{suffix}")))
                            .debug_selector(|| format!("{key}-{suffix}"))
                            .tip(self.tips, p, lines)
                            .into_any_element(),
                        None => detail.into_any_element(),
                    }
                }),
            )
        });
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
            .children(ws.helper_chip(id, !roomy).map(|t| {
                // Its helpers, as 0.19's pill had them; a click lists them.
                let chat = chat_id.clone();
                div()
                    .id(SharedString::from(format!("{key}-helpers")))
                    .selector({
                        let key = key.clone();
                        move || format!("{key}-helpers")
                    })
                    // In words it gives way, cut short, before the name
                    // does, but stays a click wide; "3 !1" keeps its size.
                    .when(roomy, |d| d.flex_shrink(1.).min_w(u(32.)).overflow_hidden())
                    .when(!roomy, |d| d.flex_none())
                    .on_click(move |_, window, cx| crate::dialogs::open_helpers(Some(&chat), window, cx))
                    .child(tag(p, Some(p.info), t).whitespace_nowrap().overflow_hidden().text_ellipsis())
            }))
            .child(name)
            .children(details);
        // A bar with no room leaves what the chat's details say out, and ID,
        // Take off the board and Delete to the menu, which has all three;
        // Delete goes first (`DELETE_BAR`).
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
        if let (MenuAt::Pane(_), true) = (at, delete) {
            let delete = chat_id.clone();
            bar = bar.child(
                div()
                    .id(SharedString::from(format!("{key}-delete")))
                    .selector({
                        let key = key.clone();
                        move || format!("{key}-delete")
                    })
                    .on_click(cx.listener(move |this, _, window, cx| this.ask_delete(&delete, window, cx)))
                    .child(button(p, "×")),
            );
        }
        if menu_open {
            // Notes where this bar is, for the menu to open on the side with
            // room (drawn again where that moved).
            let (laid, board) = (self.menu_bar.clone(), cx.entity().downgrade());
            bar = bar.child(
                canvas(
                    move |bounds, window, cx| {
                        let now = Some((bounds, window.viewport_size().height));
                        if laid.replace(now) != now {
                            cx.defer(move |cx| {
                                let _ = board.update(cx, |_, cx| cx.notify());
                            });
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            );
            bar = bar.child(deferred(self.menu_el(p, state, at, &chat_id, cx)).with_priority(1));
        }
        bar.into_any_element()
    }

    /// The menu under a title bar's `...`, or over the bar where it does not
    /// fit below and there is more room above (a bottom pane's on a low
    /// window); no taller than that room, its rows scrolling.
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
        // Last, as the bar's × is: the question first, as from the × or the
        // sidebar's row.
        let delete_id = id.to_owned();
        items.push(("delete".into(), "Delete".into(), Box::new(move |b, window, cx| b.ask_delete(&delete_id, window, cx))));
        let rows: Vec<AnyElement> = items
            .into_iter()
            .map(|(key, label, run)| {
                div()
                    .id(SharedString::from(format!("menu-{key}")))
                    .selector(move || format!("menu-{key}"))
                    .flex_none()
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
        let want = u(MENU_ROW) * rows.len() as f32 + u(6.);
        let (above, room) = menu_side(self.menu_bar.get(), want);
        div()
            .id("pane-menu")
            .selector(|| "pane-menu".into())
            .absolute()
            .when(above, |d| d.bottom(u(23.)))
            .when(!above, |d| d.top(u(23.)))
            .right(u(0.))
            .max_h(room)
            .overflow_y_scroll()
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

    /// The board's dividers as drawn: where they were left, the one being
    /// dragged on it where it is now, each held to the least a pane keeps
    /// in the area as last laid out.
    fn splits(&self, state: &WindowState) -> Splits {
        let mut splits = state.current().splits();
        if let Some(h) = self.held.filter(|h| (h.board, h.layout) == (state.board, state.layout())) {
            splits = splits.with(h.split, h.share);
        }
        if let Some(inner) = self.inner.get() {
            for split in [Split::Col, Split::RowL, Split::RowR] {
                let (span, least) = span_of(split, inner);
                splits = splits.with(split, held_to(splits.get(split), span, least));
            }
        }
        splits
    }

    /// A drag lasts only while what it was pressed on is on screen: another
    /// board or layout (a key, auto-tile) ends it, and nothing is kept.
    fn drop_stale_hold(&mut self, board: usize, layout: usize) {
        if self.held.is_some_and(|h| (h.board, h.layout) != (board, layout)) {
            self.held = None;
        }
    }

    /// The held divider followed to the pointer at `at`. A move with the
    /// button up is a release the window did not see (let go outside it, or
    /// the mouse's capture taken away): the divider stays where it was.
    fn held_moved(&mut self, at: Point<Pixels>, pressed: bool, cx: &mut Context<Self>) {
        let on = {
            let s = &self.workspace().read(cx).state;
            (s.board, s.layout())
        };
        self.drop_stale_hold(on.0, on.1);
        let (Some(h), Some(inner)) = (self.held, self.inner.get()) else {
            self.held = None;
            cx.notify();
            return;
        };
        if !pressed {
            self.let_go(cx);
            return;
        }
        let (span, least) = span_of(h.split, inner);
        let moved = if h.split == Split::Col { at.x - h.from.x } else { at.y - h.from.y };
        let room = span - u(DIVIDER);
        if room <= px(0.) {
            return;
        }
        let share = held_to((h.from_share + moved / room).clamp(SPLIT_MIN, SPLIT_MAX), span, least);
        if share != h.share {
            self.held = Some(Held { share, moved: true, ..h });
            cx.notify();
        }
    }

    /// The button let go: the state keeps where a moved divider is.
    fn let_go(&mut self, cx: &mut Context<Self>) {
        let Some(h) = self.held.take() else { return };
        if h.moved {
            self.workspace().clone().update(cx, |w, cx| w.set_split(h.board, h.layout, h.split, h.share, cx));
        }
        cx.notify();
    }

    /// The divider `split` of the board on screen, at `share` as drawn.
    fn divider(&self, p: &Palette, state: &WindowState, split: Split, share: f32, cx: &mut Context<Self>) -> AnyElement {
        let (board, layout) = (state.board, state.layout());
        let hi = p.hi;
        let key = split.key();
        div()
            .id(SharedString::from(format!("split-{key}")))
            .selector(move || format!("split-{key}"))
            .flex_none()
            .map(|d| {
                if split == Split::Col {
                    d.w(u(DIVIDER)).h_full().cursor_col_resize()
                } else {
                    d.h(u(DIVIDER)).w_full().cursor_row_resize()
                }
            })
            .when(self.held.is_some_and(|h| h.split == split), |d| d.bg(hi))
            .hover(move |s| s.bg(hi))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    let from = e.position;
                    this.held = Some(Held { board, layout, split, from, from_share: share, share, moved: false });
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    /// `a` and `b` side by side (`Split::Col`) or one over the other, `a`
    /// taking `share` of the room the divider between them leaves.
    #[allow(clippy::too_many_arguments)]
    fn split_box(
        &self,
        p: &Palette,
        state: &WindowState,
        split: Split,
        share: f32,
        a: AnyElement,
        b: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let slot = |share: f32, el: AnyElement| div().flex().flex_basis(relative(0.)).flex_grow(share).min_w_0().min_h_0().child(el);
        div()
            .flex()
            .when(split != Split::Col, |d| d.flex_col())
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(slot(share, a))
            .child(self.divider(p, state, split, share, cx))
            .child(slot(1. - share, b))
            .into_any_element()
    }

    /// Over the panes' area, drawing nothing: it notes the area's size as
    /// laid out (drawing the board again where that changes the least a
    /// pane keeps), and while a divider is held takes every move and the
    /// release wherever the pointer is, before anything under it does.
    fn follow(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (inner, board) = (self.inner.clone(), cx.entity().downgrade());
        let cursor = self.held.map(|h| if h.split == Split::Col { CursorStyle::ResizeLeftRight } else { CursorStyle::ResizeUpDown });
        canvas(
            move |bounds, _, cx| {
                if inner.replace(Some(bounds.size)) != Some(bounds.size) {
                    let board = board.clone();
                    cx.defer(move |cx| {
                        let _ = board.update(cx, |_, cx| cx.notify());
                    });
                }
                board
            },
            move |_, board, window, _| {
                if let Some(cursor) = cursor {
                    window.set_window_cursor_style(cursor);
                }
                let held = |board: &gpui::WeakEntity<Board>, cx: &App| board.upgrade().filter(|b| b.read(cx).held.is_some());
                let moves = board.clone();
                window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
                    if phase != DispatchPhase::Capture {
                        return;
                    }
                    if let Some(b) = held(&moves, cx) {
                        b.update(cx, |b, cx| b.held_moved(e.position, e.pressed_button == Some(MouseButton::Left), cx));
                        cx.stop_propagation();
                    }
                });
                window.on_mouse_event(move |e: &MouseUpEvent, phase, _, cx| {
                    if phase != DispatchPhase::Capture || e.button != MouseButton::Left {
                        return;
                    }
                    if let Some(b) = held(&board, cx) {
                        b.update(cx, |b, cx| {
                            b.held_moved(e.position, true, cx);
                            b.let_go(cx);
                        });
                        cx.stop_propagation();
                    }
                });
            },
        )
        .absolute()
        .top(u(4.))
        .left(u(4.))
        .right(u(4.))
        .bottom(u(4.))
    }

    fn pane(
        &mut self,
        p: &Palette,
        state: &WindowState,
        splits: Splits,
        i: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
        let width = pane_width(state, splits, i, window);
        let (roomy, delete) = (width >= ROOMY_BAR, width >= DELETE_BAR);
        let bar = self.title_bar(
            p,
            state,
            MenuAt::Pane(i),
            &id,
            BarLook { active: state.active_pane == i && state.peek.is_none(), roomy, delete },
            cx,
        );
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
        let bar = self.title_bar(p, state, at, &id, BarLook { active: true, roomy: true, delete: true }, cx);
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

/// The ctx meter's denominator and, when known, the remaining tokens.
pub(crate) fn ctx_tip(auto_compact: bool, to_compact: Option<u64>) -> Vec<SharedString> {
    let basis = if auto_compact { "Share of auto-compaction limit" } else { "Share of model window" };
    let mut lines = vec![basis.into()];
    if let Some(tokens) = to_compact {
        lines.push(format!("~{}k tokens to auto-compaction (estimated)", tokens / 1000).into());
    }
    lines
}

impl Render for Board {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        self.tips = tips_on(window);
        let p = self.models.shell.read(cx).palette();
        let state = self.workspace().read(cx).state.clone();
        // A terminal let go of here with the keyboard in it hands it back to
        // the window (`keep_keyboard`), once this frame is drawn.
        let dropped = self.sync(&state, cx);
        if dropped.iter().any(|t| t.read(cx).focus_handle(cx).is_focused(window)) {
            let home = self.models.home.clone();
            window.defer(cx, move |window, cx| window.focus(&home, cx));
        }
        self.drop_stale_hold(state.board, state.layout());
        let splits = self.splits(&state);
        // Let go, the PTYs are told the size their panes settled at.
        for term in self.terms.values() {
            term.read(cx).hold_size(self.held.is_some());
        }
        let layout = state.layout();
        let mut panes: Vec<AnyElement> = (0..layout).map(|i| self.pane(&p, &state, splits, i, window, cx)).collect();
        let mut take = |i: usize| std::mem::replace(&mut panes[i], div().into_any_element());
        let grid = match layout {
            1 => div().flex().flex_1().min_w_0().min_h_0().child(take(0)).into_any_element(),
            2 => {
                let (a, b) = (take(0), take(1));
                self.split_box(&p, &state, Split::Col, splits.col, a, b, cx)
            }
            3 => {
                let (a, b, c) = (take(0), take(1), take(2));
                let right = self.split_box(&p, &state, Split::RowR, splits.row_r, b, c, cx);
                self.split_box(&p, &state, Split::Col, splits.col, a, right, cx)
            }
            _ => {
                let (a, b, c, d) = (take(0), take(1), take(2), take(3));
                let left = self.split_box(&p, &state, Split::RowL, splits.row_l, a, c, cx);
                let right = self.split_box(&p, &state, Split::RowR, splits.row_r, b, d, cx);
                self.split_box(&p, &state, Split::Col, splits.col, left, right, cx)
            }
        };
        let peek = state.peek.clone().map(|peek| self.peek(&p, &state, peek, window, cx));
        div().id("board").relative().size_full().flex().p(u(4.)).bg(p.board).child(grid).children(peek).child(self.follow(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::{BarDetail, bar_details, menu_side};
    use crate::{demo, workspace::Live};
    use gpui::{Bounds, point, px, size};

    /// Below its bar where it fits or there is no more room above; else
    /// above. Either way no taller than the room on its side.
    #[test]
    fn a_menu_opens_on_the_side_with_room() {
        let bar = |y: f32, h: f32| Some((Bounds::new(point(px(0.), px(y)), size(px(300.), px(24.))), px(h)));
        assert_eq!(menu_side(bar(30., 600.), px(200.)), (false, px(542.)));
        assert_eq!(menu_side(bar(330., 600.), px(300.)), (true, px(326.)));
        assert_eq!(menu_side(bar(200., 400.), px(300.)), (true, px(196.)), "scrolls above");
        assert_eq!(menu_side(bar(100., 400.), px(300.)), (false, px(272.)), "scrolls below");
        assert_eq!(menu_side(None, px(300.)), (false, px(300.)));
    }

    #[test]
    fn a_bars_details_are_set_apart_and_those_it_lacks_leave_no_dot() {
        let state = demo::term_state();
        let mut chat = state.find(demo::DEMO_CHAT).unwrap().clone();
        let live = Live { context: Some(0.09), ..Live::default() };
        let labels = |details: Vec<BarDetail>| details.into_iter().map(|d| d.label).collect::<Vec<_>>();
        assert_eq!(labels(bar_details(&chat, Some("/work/luna"), &live)), ["Opus · high", "· Claude Code: work", "· luna", "· ctx 9%"]);
        chat.model = None;
        chat.effort.clear();
        assert_eq!(labels(bar_details(&chat, None, &live)), ["Claude Code: work", "· ctx 9%"]);
        assert_eq!(labels(bar_details(&chat, Some(""), &Live::default())), ["Claude Code: work"]);
    }

    #[test]
    fn a_bars_folder_prefers_the_worktree_and_tips_its_full_path() {
        let state = demo::term_state();
        let mut chat = state.find(demo::DEMO_CHAT).unwrap().clone();
        let path = r"C:\work\luna\.claude\worktrees\wt-130\";
        chat.worktree_path = Some(path.into());
        let details = bar_details(&chat, Some(r"C:\work\luna"), &Live::default());
        assert_eq!(details[2].label, "· wt-130");
        assert_eq!(details[2].tip, Some(("folder", vec![path.into()])));
    }
}
