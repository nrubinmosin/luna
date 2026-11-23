//! The library window: the library's folder and its sync across the top; on
//! the left every entry (instructions, skills, Luna's system texts, the
//! folders that say otherwise for their sessions); on the right the one
//! picked: which accounts it is meant for, whose sessions it is for, what
//! each account or folder gets, and its file in the editor.

use super::editor::{StatusKind, forget};
use super::{
    AccountRef, Audience, Draft, EditorDoc, Entry, Library, Overview, Role, SyncState, SystemCompare, by_cli, describe, how, shown_system,
};
use crate::desktop::{Pick, PickKind};
use crate::models::Models;
use crate::panels::ui::{self, Confirm, DropItem, Dropdown, OnClick, Tone};
use crate::panels::{PanelLayer, Provider, Save, ask, cmd};
use crate::scale::u;
use crate::selector::Selector;
use crate::theme::Palette;
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, AppContext, Context, Div, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, WeakEntity, Window, div,
};
use gpui_component::input::{Editor, Input, InputEvent, InputState};
use serde_json::json;
use std::rc::Rc;

/// What is picked on the left.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sel {
    Instructions(String),
    /// The accounts that get no instructions.
    NoInstructions,
    Skill(String),
    System(String),
    Folder(String),
}

impl Sel {
    /// The file it has in the editor.
    fn entry(&self) -> Option<Entry> {
        match self {
            Sel::Instructions(n) => Some(Entry::Instructions(n.clone())),
            Sel::Skill(n) => Some(Entry::Skill(n.clone())),
            Sel::System(n) => Some(Entry::System(n.clone())),
            Sel::NoInstructions | Sel::Folder(_) => None,
        }
    }
}

/// A question the panel waits on.
#[derive(Clone, Debug, PartialEq)]
enum Ask {
    Delete(Entry),
    Reset(String),
    /// Unsaved text in the editor, before what replaces or closes it.
    Discard(Next),
}

/// What replaces or closes the editor: each goes through the discard
/// question while the editor has text not saved (`LibraryPanel::guard`).
#[derive(Clone, Debug, PartialEq)]
pub enum Next {
    Select(Sel),
    /// The window closes.
    Close,
    Rename(Entry, String),
    /// Delete..., Back to Luna's...: their own question after.
    Delete(Entry),
    Reset(String),
    /// Another library folder ("": Luna's own).
    Source(String),
    AddFolder(String),
    /// The draft kept from before, in the editor again.
    Restore(Draft),
}

pub struct LibraryPanel {
    models: Models,
    layer: WeakEntity<PanelLayer>,
    pub library: Entity<Library>,
    pub selected: Option<Sel>,
    pub editor: Option<Entity<EditorDoc>>,
    pub(crate) new_instructions: Entity<InputState>,
    pub(crate) new_skill: Entity<InputState>,
    /// The entry being renamed, and its name field.
    renaming: Option<(Entry, Entity<InputState>)>,
    /// The drop-down list open, by its id.
    menu: Option<String>,
    /// The reach last sent from the open list, which the next tick builds
    /// on before the answer to the last one is in.
    draft: Option<Option<Vec<String>>>,
    ask: Option<Ask>,
    /// Luna's own text of a system text beside yours, while it is shown.
    compare: Option<(String, Result<SystemCompare, String>)>,
    /// Text not saved when the window last went (`library_draft`), offered
    /// until it is restored or thrown away.
    pub offer: Option<Draft>,
    _subs: Vec<Subscription>,
}

impl LibraryPanel {
    pub fn new(models: Models, layer: WeakEntity<PanelLayer>, window: &mut Window, cx: &mut Context<Self>) -> LibraryPanel {
        let host = models.host.clone();
        let library = cx.new(|cx| Library::new(host, cx));
        let new_instructions = cx.new(|cx| InputState::new(window, cx).placeholder("new-instructions-name"));
        let new_skill = cx.new(|cx| InputState::new(window, cx).placeholder("new-skill-name"));
        let subs = vec![
            cx.observe(&library, |_, _, cx| cx.notify()),
            cx.observe(&models.shell, |_, _, cx| cx.notify()),
            cx.subscribe_in(&new_instructions, window, |this, _, e: &InputEvent, window, cx| {
                if matches!(e, InputEvent::PressEnter { .. }) {
                    this.create(false, window, cx);
                }
            }),
            cx.subscribe_in(&new_skill, window, |this, _, e: &InputEvent, window, cx| {
                if matches!(e, InputEvent::PressEnter { .. }) {
                    this.create(true, window, cx);
                }
            }),
        ];
        let kept = ask::<Option<Draft>>(&models.host, cmd::LIBRARY_DRAFT, json!({}));
        cx.spawn(async move |this, cx| {
            if let Ok(Some(draft)) = kept.await {
                let _ = this.update(cx, |v, cx| {
                    v.offer = Some(draft);
                    cx.notify();
                });
            }
        })
        .detach();
        LibraryPanel {
            models,
            layer,
            library,
            selected: None,
            editor: None,
            new_instructions,
            new_skill,
            renaming: None,
            menu: None,
            draft: None,
            ask: None,
            compare: None,
            offer: None,
            _subs: subs,
        }
    }

    fn act(&self, cx: &Context<Self>, f: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static) -> OnClick {
        let this = cx.weak_entity();
        Rc::new(move |window, cx| {
            let _ = this.update(cx, |v, cx| f(v, window, cx));
        })
    }

    fn dirty(&self, cx: &App) -> bool {
        self.editor.as_ref().is_some_and(|e| e.read(cx).dirty(cx))
    }

    /// Esc: a question, a list or a rename goes first, then unsaved text
    /// asks once; the window closes after.
    pub fn may_close(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> bool {
        cx.notify();
        if self.ask.take().is_some() || self.menu.take().is_some() || self.renaming.take().is_some() {
            self.draft = None;
            return false;
        }
        if self.dirty(cx) {
            self.ask = Some(Ask::Discard(Next::Close));
            return false;
        }
        true
    }

    /// `next`, which replaces or closes the editor: asked first while the
    /// editor has text not saved.
    pub(crate) fn guard(&mut self, next: Next, window: &mut Window, cx: &mut Context<Self>) {
        if self.dirty(cx) {
            self.ask = Some(Ask::Discard(next));
            cx.notify();
            return;
        }
        self.go(next, window, cx);
    }

    fn go(&mut self, next: Next, window: &mut Window, cx: &mut Context<Self>) {
        match next {
            Next::Select(sel) => self.show(sel, None, window, cx),
            Next::Close => self.close(cx),
            Next::Rename(entry, to) => self.rename(entry, to, window, cx),
            Next::Delete(entry) => self.ask = Some(Ask::Delete(entry)),
            Next::Reset(name) => self.ask = Some(Ask::Reset(name)),
            Next::Source(path) => {
                let task = self.library.update(cx, |l, cx| l.set_source(&path, cx));
                // The editor's file was the old library's: nothing of it is
                // left open on the new one.
                cx.spawn(async move |this, cx| {
                    if task.await {
                        let _ = this.update(cx, |v, cx| v.reset(cx));
                    }
                })
                .detach();
            }
            Next::AddFolder(path) => {
                let task = self.library.update(cx, |l, cx| l.set_folder(&path, true, cx));
                cx.spawn(async move |this, cx| {
                    if task.await {
                        let _ = this.update(cx, |v, cx| {
                            v.reset(cx);
                            v.selected = Some(Sel::Folder(path));
                        });
                    }
                })
                .detach();
            }
            Next::Restore(draft) => {
                self.offer = None;
                let sel = match &draft.entry {
                    Entry::Skill(n) => Sel::Skill(n.clone()),
                    Entry::Instructions(n) => Sel::Instructions(n.clone()),
                    Entry::System(n) => Sel::System(n.clone()),
                };
                self.selected = None;
                self.show(sel, Some(draft), window, cx);
            }
        }
        cx.notify();
    }

    /// Nothing picked, nothing in the editor.
    fn reset(&mut self, cx: &mut Context<Self>) {
        self.selected = None;
        self.editor = None;
        self.renaming = None;
        self.menu = None;
        self.compare = None;
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if let Some(layer) = self.layer.upgrade() {
            layer.update(cx, |l, cx| l.close(cx));
        }
    }

    /// Picks `sel`; unsaved text in the editor asks first.
    pub fn select(&mut self, sel: Sel, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.as_ref() == Some(&sel) {
            return;
        }
        self.guard(Next::Select(sel), window, cx);
    }

    /// `sel` in the editor; with `draft`, its text over the file's.
    fn show(&mut self, sel: Sel, draft: Option<Draft>, window: &mut Window, cx: &mut Context<Self>) {
        self.renaming = None;
        self.menu = None;
        self.compare = None;
        self.editor = sel.entry().map(|entry| {
            let (host, library) = (self.models.host.clone(), self.library.clone());
            cx.new(|cx| EditorDoc::new(host, library, entry, draft, window, cx))
        });
        if let Some(e) = &self.editor {
            cx.observe(e, |_, _, cx| cx.notify()).detach();
        }
        self.selected = Some(sel);
        cx.notify();
    }

    /// The name typed under Instructions or Skills, made and picked.
    pub(crate) fn create(&mut self, skill: bool, window: &mut Window, cx: &mut Context<Self>) {
        let field = if skill { self.new_skill.clone() } else { self.new_instructions.clone() };
        let name = field.read(cx).value().trim().to_string();
        if name.is_empty() {
            return;
        }
        let entry = if skill { Entry::Skill(name.clone()) } else { Entry::Instructions(name.clone()) };
        let task = self.library.update(cx, |l, cx| l.create(&entry, cx));
        cx.spawn_in(window, async move |this, cx| {
            if task.await {
                let _ = this.update_in(cx, |v, window, cx| {
                    field.update(cx, |f, cx| f.set_value("", window, cx));
                    let sel = if skill { Sel::Skill(name) } else { Sel::Instructions(name) };
                    v.select(sel, window, cx);
                });
            }
        })
        .detach();
    }

    fn start_rename(&mut self, entry: Entry, window: &mut Window, cx: &mut Context<Self>) {
        let name = entry.name().to_owned();
        let field = cx.new(|cx| InputState::new(window, cx).default_value(name));
        cx.subscribe_in(&field, window, |this, _, e: &InputEvent, window, cx| {
            if matches!(e, InputEvent::PressEnter { .. }) {
                this.commit_rename(window, cx);
            }
        })
        .detach();
        field.update(cx, |f, cx| f.focus(window, cx));
        self.renaming = Some((entry, field));
        cx.notify();
    }

    fn commit_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((entry, field)) = self.renaming.take() else { return };
        let to = field.read(cx).value().trim().to_string();
        cx.notify();
        if to.is_empty() || to == entry.name() {
            return;
        }
        // The editor opens again on the renamed file: text not saved asks.
        self.guard(Next::Rename(entry, to), window, cx);
    }

    fn rename(&mut self, entry: Entry, to: String, window: &mut Window, cx: &mut Context<Self>) {
        let task = self.library.update(cx, |l, cx| l.rename(&entry, &to, cx));
        cx.spawn_in(window, async move |this, cx| {
            if task.await {
                let _ = this.update_in(cx, |v, window, cx| {
                    let sel = match entry {
                        Entry::Skill(_) => Sel::Skill(to),
                        _ => Sel::Instructions(to),
                    };
                    v.editor = None;
                    v.selected = None;
                    v.show(sel, None, window, cx);
                });
            }
        })
        .detach();
    }

    /// The question answered yes.
    fn yes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ask) = self.ask.take() else { return };
        match ask {
            Ask::Delete(entry) => {
                let task = self.library.update(cx, |l, cx| l.delete(&entry, cx));
                cx.spawn(async move |this, cx| {
                    if task.await {
                        let _ = this.update(cx, |v, cx| {
                            v.selected = None;
                            v.editor = None;
                            cx.notify();
                        });
                    }
                })
                .detach();
            }
            Ask::Reset(name) => {
                self.compare = None;
                let task = self.library.update(cx, |l, cx| l.system_reset(&name, cx));
                // The editor had the override; it opens again on Luna's own.
                cx.spawn_in(window, async move |this, cx| {
                    if task.await {
                        let _ = this.update_in(cx, |v, window, cx| {
                            v.editor = None;
                            v.selected = None;
                            v.show(Sel::System(name), None, window, cx);
                        });
                    }
                })
                .detach();
            }
            Ask::Discard(next) => {
                if let Some(e) = self.editor.take() {
                    e.update(cx, |e, _| e.forget_draft());
                }
                self.go(next, window, cx);
            }
        }
        cx.notify();
    }

    fn save(&mut self, _: &Save, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(e) = &self.editor {
            e.update(cx, |e, cx| e.save(cx));
        }
    }

    /// The system's folder picker, modal to the window; `then` of what was
    /// picked, guarded (`guard`).
    fn pick_folder(&mut self, title: &str, window: &mut Window, cx: &mut Context<Self>, then: fn(String) -> Next) {
        let pick = Pick { kind: PickKind::Folder, title: Some(title.to_owned()), start: None, filters: Vec::new() };
        let picked = self.models.host.pick(pick, Some(window));
        cx.spawn_in(window, async move |this, cx| match picked.await {
            Ok(Some(path)) => {
                let _ = this.update_in(cx, |v, window, cx| v.guard(then(path.to_string_lossy().into_owned()), window, cx));
            }
            Ok(None) => {}
            Err(e) => log::warn!("folder picker: {e}"),
        })
        .detach();
    }

    /// The draft on offer thrown away.
    pub(crate) fn drop_offer(&mut self, cx: &mut Context<Self>) {
        // That draft alone: one an editor has kept since stays.
        if let Some(d) = self.offer.take() {
            forget(&self.models.host, &d.id);
        }
        cx.notify();
    }

    pub(crate) fn toggle_menu(&mut self, id: &str, cx: &mut Context<Self>) {
        self.menu = if self.menu.as_deref() == Some(id) { None } else { Some(id.to_owned()) };
        self.draft = None;
        cx.notify();
    }

    fn close_menu(&mut self, cx: &mut Context<Self>) {
        self.menu = None;
        self.draft = None;
        cx.notify();
    }

    pub(crate) fn compare_with_luna(&mut self, name: String, cx: &mut Context<Self>) {
        if self.compare.as_ref().is_some_and(|(n, _)| *n == name) {
            self.compare = None;
            cx.notify();
            return;
        }
        let answer = ask::<SystemCompare>(&self.models.host, cmd::LIBRARY_SYSTEM_COMPARE, json!({ "name": name }));
        cx.spawn(async move |this, cx| {
            let got = answer.await;
            let _ = this.update(cx, |v, cx| {
                v.compare = Some((name, got));
                cx.notify();
            });
        })
        .detach();
    }
}

/// One line of a reach picker's list.
struct ReachItem {
    label: String,
    checked: bool,
    enabled: bool,
    /// The reach it sends; None: the folder says nothing of it.
    next: Option<Vec<String>>,
}

/// The reach picker's lines (the page's ReachPicker): every account, each
/// CLI's every account, each account; for a folder's, first a line for
/// going by what holds outside it, whose accounts the ticks show then.
fn reach_items(own: Option<&[String]>, inherit: Option<&[String]>, accounts: &[AccountRef]) -> Vec<ReachItem> {
    let reach: Vec<String> = own.or(inherit).unwrap_or(&[]).to_vec();
    let without = |keys: &[&str]| reach.iter().filter(|r| !keys.contains(&r.as_str())).cloned().collect::<Vec<_>>();
    let with = |key: &str| {
        let mut r = without(&[key]);
        r.push(key.to_owned());
        r
    };
    let mut out = Vec::new();
    if inherit.is_some() {
        let outside = own.is_none();
        out.push(ReachItem {
            label: "As outside this folder".into(),
            checked: outside,
            enabled: true,
            next: if outside { Some(reach.clone()) } else { None },
        });
    }
    let all = reach.iter().any(|r| r == super::ALL);
    out.push(ReachItem {
        label: "All accounts".into(),
        checked: all,
        enabled: true,
        next: Some(if all { Vec::new() } else { vec![super::ALL.into()] }),
    });
    for p in Provider::ALL {
        let mine: Vec<&AccountRef> = accounts.iter().filter(|a| a.provider == p).collect();
        let named = reach.iter().any(|r| r == p.key());
        if mine.is_empty() && !named {
            continue;
        }
        let wide = all || named;
        out.push(ReachItem {
            label: format!("All {}", p.label()),
            checked: wide,
            enabled: !all,
            next: Some(if named { without(&[p.key()]) } else { with(p.key()) }),
        });
        for a in mine {
            let key = a.key();
            let has = reach.contains(&key);
            out.push(ReachItem {
                label: format!("    {}", a.name),
                checked: wide || has,
                enabled: !wide,
                next: Some(if has { without(&[key.as_str()]) } else { with(&key) }),
            });
        }
    }
    out
}

impl LibraryPanel {
    /// A reach picker: `own` the reach as saved (None: the folder says
    /// nothing, `inherit` who gets it then); `set` sends a new one.
    fn reach_picker(
        &self,
        id: &str,
        own: Option<Vec<String>>,
        inherit: Option<Vec<String>>,
        accounts: &[AccountRef],
        set: Rc<dyn Fn(&mut Self, Option<Vec<String>>, &mut Context<Self>)>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let p = &self.models.shell.read(cx).palette();
        let open = self.menu.as_deref() == Some(id);
        // While open, the last sent reach stands for the saved one.
        let own = if open { self.draft.clone().unwrap_or(own) } else { own };
        let face = match (&own, &inherit) {
            (None, Some(gets)) => format!("As outside: {}", if gets.is_empty() { "none".into() } else { describe(gets) }),
            (Some(r), _) => describe(r),
            (None, None) => describe(&[]),
        };
        let items = reach_items(own.as_deref(), inherit.as_deref(), accounts)
            .into_iter()
            .map(|item| {
                let set = set.clone();
                let next = item.next.clone();
                DropItem {
                    label: item.label.into(),
                    chosen: item.checked,
                    tick: true,
                    enabled: item.enabled,
                    group: None,
                    pick: self.act(cx, move |v, _, cx| {
                        v.draft = Some(next.clone());
                        set(v, next.clone(), cx);
                        cx.notify();
                    }),
                }
            })
            .collect();
        let toggle_id = id.to_owned();
        Dropdown {
            id: id.to_owned(),
            face: face.into(),
            open,
            enabled: true,
            width: u(220.),
            items,
            on_toggle: self.act(cx, move |v, _, cx| v.toggle_menu(&toggle_id, cx)),
            on_close: self.act(cx, |v, _, cx| v.close_menu(cx)),
        }
        .render(p)
        .into_any_element()
    }

    fn audience_boxes(&self, entry: &Entry, audience: Audience, p: &Palette, cx: &Context<Self>) -> Div {
        div().flex().gap(u(12.)).children(Role::ALL.map(|role| {
            let on = audience.has(role);
            let next = audience.flipped(role);
            let entry = entry.clone();
            let id = format!("lib-audience-{}", role.key());
            // The last role ticked stays: nobody's is no audience at all.
            ui::check(
                &id,
                p,
                role.label(),
                on,
                next.is_some(),
                self.act(cx, move |v, _, cx| {
                    if let Some(next) = next {
                        v.library.update(cx, |l, cx| l.set_audience(&entry, next, cx)).detach();
                    }
                }),
            )
        }))
    }
}

impl Render for LibraryPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.models.shell.read(cx).palette();
        let library = self.library.read(cx);
        let (overview, error, busy, sync) = (library.overview.clone(), library.error.clone(), library.busy(), library.sync.clone());
        let close = self.act(cx, |v, window, cx| {
            if v.may_close(window, cx) {
                v.close(cx);
            }
        });
        let body = match &overview {
            None => div().p(u(10.)).child(match error {
                Some(e) => ui::alert(&p, Tone::Trouble, e),
                None => ui::note(&p, "Reading the library..."),
            }),
            Some(o) => div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .p(u(8.))
                .gap(u(6.))
                .child(self.header(o, &sync, busy, &p, cx))
                .children(error.map(|e| ui::alert(&p, Tone::Trouble, e)))
                .child(div().flex().flex_1().min_h_0().gap(u(8.)).child(self.list(o, busy, &p, cx)).child(self.detail(o, busy, &p, cx))),
        };
        let frame = ui::frame(&p, "library", "Skills & instructions", u(960.), close, body).h(u(660.)).relative();
        let frame = frame.on_action(cx.listener(Self::save));
        match self.ask.clone() {
            Some(ask) => frame.child(self.question(ask, &p, cx).render(&p, "lib-ask")),
            None => frame,
        }
    }
}

impl LibraryPanel {
    fn header(&self, o: &Overview, sync: &SyncState, busy: bool, p: &Palette, cx: &Context<Self>) -> Div {
        let state = match sync {
            SyncState::Idle => None,
            SyncState::Syncing => Some(ui::alert(p, Tone::Working, "Syncing: laying the library out into every account...")),
            SyncState::Synced(r) if r.clean() => Some(ui::alert(p, Tone::Done, r.said())),
            SyncState::Synced(r) => Some(ui::alert(p, Tone::Attention, r.said())),
            SyncState::Failed(e) => Some(ui::alert(p, Tone::Trouble, format!("Sync failed: {e}"))),
        };
        let offer = self.offer.as_ref().map(|d| {
            let restore = d.clone();
            div()
                .flex()
                .items_center()
                .gap(u(6.))
                .child(div().flex_1().min_w_0().child(ui::alert(
                    p,
                    Tone::Attention,
                    format!("{} has text not saved from when the window last went.", d.entry.name()),
                )))
                .child(ui::button(
                    "lib-draft-restore",
                    p,
                    "Restore",
                    !busy,
                    self.act(cx, move |v, window, cx| v.guard(Next::Restore(restore.clone()), window, cx)),
                ))
                .child(ui::button("lib-draft-drop", p, "Throw away", true, self.act(cx, |v, _, cx| v.drop_offer(cx))))
        });
        div()
            .flex()
            .flex_col()
            .gap(u(4.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(u(6.))
                    .child(div().flex_none().font_weight(FontWeight::BOLD).child("Library"))
                    .child(ui::path_text(p, o.root.clone()).flex_1())
                    .child(ui::button(
                        "lib-open",
                        p,
                        "Open",
                        true,
                        self.act(cx, |v, _, cx| v.library.update(cx, |l, cx| l.open_folder(None, cx))),
                    ))
                    .child(ui::button(
                        "lib-change",
                        p,
                        "Change...",
                        !busy,
                        self.act(cx, |v, window, cx| v.pick_folder("The library's folder", window, cx, Next::Source)),
                    ))
                    .child(ui::button(
                        "lib-default",
                        p,
                        "Default",
                        !busy && !o.is_default,
                        self.act(cx, |v, window, cx| v.guard(Next::Source(String::new()), window, cx)),
                    ))
                    .child(ui::button(
                        "lib-sync",
                        p,
                        if matches!(sync, SyncState::Syncing) { "Syncing..." } else { "Sync" },
                        !busy,
                        self.act(cx, |v, _, cx| v.library.update(cx, |l, cx| l.sync_now(cx))),
                    )),
            )
            .when(!o.found, |d| {
                d.child(ui::note(
                    p,
                    "The folder is not there yet: nothing is laid out until it is. Writing instructions or a new skill makes it.",
                ))
            })
            .children(state)
            .children(offer)
    }

    fn line(&self, id: String, label: String, sel: Sel, flag: Option<&str>, p: &Palette, cx: &Context<Self>) -> AnyElement {
        let chosen = self.selected.as_ref() == Some(&sel);
        let selector = id.clone();
        div()
            .id(SharedString::from(id))
            .selector(move || selector.clone())
            .flex()
            .items_center()
            .gap(u(4.))
            .px(u(4.))
            .h(u(18.))
            .when(chosen, |d| d.bg(p.title_bg).text_color(p.title_fg))
            .child(div().flex_1().min_w_0().truncate().child(label))
            .children(flag.map(|f| {
                div().flex_none().px(u(3.)).bg(p.attention).text_color(p.on_status).font_weight(FontWeight::BOLD).child(f.to_owned())
            }))
            .on_click({
                let act = self.act(cx, move |v, window, cx| v.select(sel.clone(), window, cx));
                move |_, window, cx| act(window, cx)
            })
            .into_any_element()
    }

    fn list(&self, o: &Overview, busy: bool, p: &Palette, cx: &Context<Self>) -> Div {
        let heading = |label: &str| div().mt(u(6.)).px(u(4.)).text_color(p.text_dim).font_weight(FontWeight::BOLD).child(label.to_owned());
        let new_row = |field: &Entity<InputState>, id: &str, skill: bool| {
            let empty = field.read(cx).value().trim().is_empty();
            div()
                .flex()
                .gap(u(4.))
                .px(u(4.))
                .py(u(2.))
                .child(div().flex_1().min_w_0().child(Input::new(field).bg(p.surface)))
                .child(ui::button(id, p, "New", !busy && !empty, self.act(cx, move |v, window, cx| v.create(skill, window, cx))))
        };
        let mut col = div().flex().flex_col().pb(u(6.));
        col = col.child(heading("Instructions"));
        for i in &o.instructions {
            let flag = i.problem.as_ref().map(|_| "!");
            col = col.child(self.line(format!("lib-ins-{}", i.name), i.name.clone(), Sel::Instructions(i.name.clone()), flag, p, cx));
        }
        col = col.child(self.line("lib-ins-none".into(), "(no instructions)".into(), Sel::NoInstructions, None, p, cx));
        col = col.child(new_row(&self.new_instructions, "lib-new-ins", false));
        col = col.child(heading("Skills"));
        for s in &o.skills {
            let flag = s.problem.as_ref().map(|_| "!");
            col = col.child(self.line(format!("lib-skill-{}", s.name), s.name.clone(), Sel::Skill(s.name.clone()), flag, p, cx));
        }
        col = col.child(new_row(&self.new_skill, "lib-new-skill", true));
        let system: Vec<_> = shown_system(o).collect();
        if !system.is_empty() {
            col = col.child(heading("System texts"));
            for s in system {
                let flag = match s.state.as_str() {
                    "library" => Some("yours"),
                    "broken" => Some("!"),
                    _ => None,
                };
                col = col.child(self.line(format!("lib-sys-{}", s.name), s.name.clone(), Sel::System(s.name.clone()), flag, p, cx));
            }
        }
        col = col.child(heading("Folders"));
        for (n, f) in o.folders.iter().enumerate() {
            let flag = (!f.found).then_some("gone");
            col = col.child(self.line(format!("lib-folder-{n}"), ui::base_name(&f.path), Sel::Folder(f.path.clone()), flag, p, cx));
        }
        col = col.child(div().px(u(4.)).py(u(2.)).child(ui::button(
            "lib-add-folder",
            p,
            "Add folder...",
            !busy,
            self.act(cx, |v, window, cx| v.add_folder(window, cx)),
        )));
        ui_well(p, div().id("lib-list").size_full().overflow_y_scroll().child(col)).w(u(250.)).flex_none()
    }

    fn detail(&self, o: &Overview, busy: bool, p: &Palette, cx: &Context<Self>) -> Div {
        let info: AnyElement = match self.selected.clone() {
            None => ui::note(
                p,
                "Pick instructions, a skill, a system text or a folder on the left. Instructions are read by both CLIs at the start of every \
                 session (CLAUDE.md, AGENTS.md): an account gets one at most. A skill is a folder with a SKILL.md, whose description is all \
                 the model sees until it opens it.",
            )
            .into_any_element(),
            Some(Sel::Instructions(name)) => match o.instructions.iter().find(|i| i.name == name) {
                Some(i) => self.instructions_info(o, i, busy, p, cx).into_any_element(),
                None => ui::note(p, "Not in the library any more.").into_any_element(),
            },
            Some(Sel::NoInstructions) => self.none_info(o, busy, p, cx).into_any_element(),
            Some(Sel::Skill(name)) => match o.skills.iter().find(|s| s.name == name) {
                Some(s) => self.skill_info(o, s, busy, p, cx).into_any_element(),
                None => ui::note(p, "Not in the library any more.").into_any_element(),
            },
            Some(Sel::System(name)) => match o.system.iter().find(|s| s.name == name) {
                Some(s) => self.system_info(s, busy, p, cx).into_any_element(),
                None => ui::note(p, "Not a system text of this Luna.").into_any_element(),
            },
            Some(Sel::Folder(path)) => match o.folders.iter().find(|f| f.path == path) {
                Some(f) => self.folder_info(o, f, busy, p, cx).into_any_element(),
                None => ui::note(p, "Not listed any more.").into_any_element(),
            },
        };
        let Some(editor) = self.editor.clone() else {
            return div().flex_1().min_w_0().child(div().id("lib-detail").size_full().overflow_y_scroll().child(info));
        };
        let doc = editor.read(cx);
        let (kind, said) = doc.status(cx);
        let (dirty, saving, loaded, conflict) = (doc.dirty(cx), doc.saving, doc.file.is_some(), doc.conflict.is_some());
        let state = doc.state.clone();
        let status = match kind {
            StatusKind::Plain => ui::note(p, said),
            StatusKind::Working => ui::alert(p, Tone::Working, said),
            StatusKind::Attention => ui::alert(p, Tone::Attention, said),
            StatusKind::Trouble => ui::alert(p, Tone::Trouble, said),
            StatusKind::Done => ui::alert(p, Tone::Done, said),
        };
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(u(6.))
            // What the file is, which scrolls and gives way first where the
            // window is short, so that the editor keeps its room and Save
            // stays in the window under it.
            .child(div().id("lib-detail").flex_shrink(1.).max_h(u(250.)).overflow_y_scroll().child(info))
            .child(
                div()
                    .flex_1()
                    .min_h(u(100.))
                    .border_1()
                    .border_color(p.frame)
                    // No typing before the file is in: what was typed would
                    // be lost under it.
                    .child(Editor::new(&state).h_full().bg(p.well).disabled(!loaded)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(u(6.))
                    .child(div().flex_1().min_w_0().child(status))
                    .when(conflict, |d| {
                        d.child(ui::button(
                            "lib-reload",
                            p,
                            "Reload",
                            !saving,
                            self.act(cx, |v, window, cx| {
                                if let Some(e) = &v.editor {
                                    e.update(cx, |e, cx| e.reload(window, cx));
                                }
                            }),
                        ))
                        .child(ui::button(
                            "lib-overwrite",
                            p,
                            "Overwrite",
                            !saving && !busy,
                            self.act(cx, |v, _, cx| {
                                if let Some(e) = &v.editor {
                                    e.update(cx, |e, cx| e.overwrite(cx));
                                }
                            }),
                        ))
                    })
                    .child(ui::primary(
                        "lib-save",
                        p,
                        if saving { "Saving..." } else { "Save" },
                        dirty && !saving && !busy,
                        self.act(cx, |v, _, cx| {
                            if let Some(e) = &v.editor {
                                e.update(cx, |e, cx| e.save(cx));
                            }
                        }),
                    )),
            )
    }

    /// The entry's name onto the clipboard, to name it in a chat.
    fn copy_name(&self, entry: &Entry, cx: &mut Context<Self>) {
        let written = self.models.host.write_clipboard_text(entry.name().to_owned());
        cx.background_spawn(async move {
            if let Err(e) = written.await {
                log::warn!("copy a library entry's name: {e}");
            }
        })
        .detach();
    }

    /// An entry's name with its copy button, and Rename beside it or the
    /// field renaming it.
    fn title_row(&self, entry: &Entry, extra: Vec<AnyElement>, busy: bool, p: &Palette, cx: &Context<Self>) -> Div {
        let row = div().flex().items_center().gap(u(6.));
        if let Some((renamed, field)) = &self.renaming
            && renamed == entry
        {
            return row
                .child(div().w(u(240.)).child(Input::new(field).bg(p.well)))
                .child(ui::primary("lib-rename-ok", p, "Rename", !busy, self.act(cx, |v, window, cx| v.commit_rename(window, cx))))
                .child(ui::button(
                    "lib-rename-cancel",
                    p,
                    "Cancel",
                    true,
                    self.act(cx, |v, _, cx| {
                        v.renaming = None;
                        cx.notify();
                    }),
                ));
        }
        let (copy, rename, delete) = (entry.clone(), entry.clone(), entry.clone());
        row.child(div().flex_1().min_w_0().truncate().font_weight(FontWeight::BOLD).child(entry.name().to_owned()))
            .child(ui::button("lib-copy-name", p, "Copy name", true, self.act(cx, move |v, _, cx| v.copy_name(&copy, cx))))
            .child(ui::button(
                "lib-rename",
                p,
                "Rename",
                !busy,
                self.act(cx, move |v, window, cx| v.start_rename(rename.clone(), window, cx)),
            ))
            .children(extra)
            .child(ui::button(
                "lib-delete",
                p,
                "Delete...",
                !busy,
                self.act(cx, move |v, window, cx| v.guard(Next::Delete(delete.clone()), window, cx)),
            ))
    }

    /// Add folder...: one picked in the system's picker, listed and picked.
    pub(crate) fn add_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pick_folder("Give skills to the sessions Luna starts in a folder", window, cx, Next::AddFolder);
    }

    fn instructions_info(&self, o: &Overview, i: &super::Instructions, busy: bool, p: &Palette, cx: &Context<Self>) -> Div {
        let entry = Entry::Instructions(i.name.clone());
        let set_entry = entry.clone();
        let reach = self.reach_picker(
            "lib-reach",
            Some(i.accounts.clone()),
            None,
            &o.accounts,
            Rc::new(move |v, r, cx| {
                v.library.update(cx, |l, cx| l.set_reach(&set_entry, r.unwrap_or_default(), cx)).detach();
            }),
            cx,
        );
        let mut accounts = div().flex().flex_col();
        for a in by_cli(&o.accounts) {
            let key = a.key();
            let way = how(&i.accounts, &a);
            let id = format!("lib-gets-{key}");
            let line = if i.per_session {
                // For some roles only: no account's file, so no radio among
                // the others; whether its sessions on the account get it.
                let named = i.accounts.contains(&key);
                let (name, reach) = (i.name.clone(), i.accounts.clone());
                let k = key.clone();
                ui::check(
                    &id,
                    p,
                    format!("{} ({}){}", a.name, a.provider.label(), way.as_ref().map(|w| format!(": {w}")).unwrap_or_default()),
                    way.is_some(),
                    !busy && (way.is_none() || named),
                    self.act(cx, move |v, _, cx| {
                        let next = if named {
                            reach.iter().filter(|r| **r != k).cloned().collect()
                        } else {
                            [reach.clone(), vec![k.clone()]].concat()
                        };
                        v.library.update(cx, |l, cx| l.set_reach(&Entry::Instructions(name.clone()), next, cx)).detach();
                    }),
                )
            } else {
                let gets = o.instructions_for.get(&key).is_some_and(|n| *n == i.name);
                let other = o.instructions_for.get(&key).filter(|n| **n != i.name).cloned();
                let label = match (gets, other) {
                    (true, _) => format!("{} ({}) gets it, {}", a.name, a.provider.label(), way.unwrap_or_default()),
                    (false, Some(n)) => format!("{} ({}) gets {n}", a.name, a.provider.label()),
                    (false, None) => format!("{} ({}) gets none", a.name, a.provider.label()),
                };
                let name = i.name.clone();
                ui::radio(
                    &id,
                    p,
                    label,
                    gets,
                    !busy,
                    self.act(cx, move |v, _, cx| {
                        v.library.update(cx, |l, cx| l.give(Some(&name), &a, cx)).detach();
                    }),
                )
            };
            accounts = accounts.child(line);
        }
        div()
            .flex()
            .flex_col()
            .gap(u(4.))
            .child(self.title_row(&entry, Vec::new(), busy, p, cx))
            .child(match &i.problem {
                Some(problem) => ui::alert(p, Tone::Attention, problem.clone()),
                None => ui::note(p, if i.description.is_empty() { "No heading yet.".to_owned() } else { i.description.clone() }),
            })
            .child(ui::row(p, "Meant for", Some("Every account, every one of a CLI, or those you name.".into()), reach))
            .child(ui::row(
                p,
                "For",
                Some(if i.per_session {
                    "For some roles only: given to each of their sessions as Luna starts it, never put into an account.".into()
                } else {
                    "Whose sessions get it: your chats, leads (helpers with Luna's tools), helpers.".into()
                }),
                self.audience_boxes(&entry, i.audience, p, cx),
            ))
            .child(ui::section(p, "Which accounts get it"))
            .child(accounts)
            .when(!o.overridden.is_empty(), |d| {
                d.child(ui::note(
                    p,
                    format!(
                        "Codex reads AGENTS.override.md instead of AGENTS.md in {}: the library's instructions do not reach it there.",
                        o.overridden.join(", ")
                    ),
                ))
            })
            .when_some(o.old_instructions.clone(), |d, old| {
                d.child(ui::note(
                    p,
                    format!(
                        "The library still has the instructions.md of before there could be several ({old}): nothing is laid out from it."
                    ),
                ))
            })
    }

    fn none_info(&self, o: &Overview, busy: bool, p: &Palette, cx: &Context<Self>) -> Div {
        let mut accounts = div().flex().flex_col();
        for a in by_cli(&o.accounts) {
            let key = a.key();
            let none = !o.instructions_for.contains_key(&key);
            accounts = accounts.child(ui::radio(
                &format!("lib-none-{key}"),
                p,
                format!("{} ({}){}", a.name, a.provider.label(), if none { " gets none" } else { "" }),
                none,
                !busy,
                self.act(cx, move |v, _, cx| {
                    v.library.update(cx, |l, cx| l.give(None, &a, cx)).detach();
                }),
            ));
        }
        div()
            .flex()
            .flex_col()
            .gap(u(4.))
            .child(div().font_weight(FontWeight::BOLD).child("No instructions"))
            .child(ui::note(p, "Nothing from the library; the account's own file, if any, is left alone."))
            .child(accounts)
    }

    fn skill_info(&self, o: &Overview, s: &super::Skill, busy: bool, p: &Palette, cx: &Context<Self>) -> Div {
        let entry = Entry::Skill(s.name.clone());
        let open = s.name.clone();
        let extra = vec![
            ui::button(
                "lib-open-skill",
                p,
                "Open folder",
                true,
                self.act(cx, move |v, _, cx| v.library.update(cx, |l, cx| l.open_folder(Some(&open), cx))),
            )
            .into_any_element(),
        ];
        let mut d = div().flex().flex_col().gap(u(4.)).child(self.title_row(&entry, extra, busy, p, cx)).child(match &s.problem {
            Some(problem) => ui::alert(p, Tone::Attention, problem.clone()),
            None => ui::note(p, s.description.clone()),
        });
        let set_entry = entry.clone();
        d = d
            .child(ui::row(
                p,
                "Meant for",
                Some("The accounts whose skills/ it goes into, wherever no folder says otherwise.".into()),
                self.reach_picker(
                    "lib-reach",
                    Some(s.accounts.clone()),
                    None,
                    &o.accounts,
                    Rc::new(move |v, r, cx| {
                        v.library.update(cx, |l, cx| l.set_reach(&set_entry, r.unwrap_or_default(), cx)).detach();
                    }),
                    cx,
                ),
            ))
            .child(ui::row(
                p,
                "For",
                Some("Whose sessions get it: your chats, leads (helpers with Luna's tools), helpers.".into()),
                self.audience_boxes(&entry, s.audience, p, cx),
            ));
        if !o.folders.is_empty() {
            d = d.child(ui::section(p, "Sessions in folders"));
            for (n, f) in o.folders.iter().enumerate() {
                let own = f.skills.get(&s.name).cloned();
                let inherited = f.inherited.get(&s.name).cloned().unwrap_or_default();
                let (path, skill) = (f.path.clone(), s.name.clone());
                d = d.child(ui::row(
                    p,
                    ui::base_name(&f.path),
                    Some(f.path.clone().into()),
                    self.reach_picker(
                        &format!("lib-folder-reach-{n}"),
                        own,
                        Some(inherited),
                        &o.accounts,
                        Rc::new(move |v, r, cx| {
                            v.library.update(cx, |l, cx| l.set_folder_skill(&path, &skill, r, cx)).detach();
                        }),
                        cx,
                    ),
                ));
            }
        }
        d
    }

    fn system_info(&self, s: &super::SystemText, busy: bool, p: &Palette, cx: &Context<Self>) -> Div {
        let yours = s.yours();
        let (compare, reset) = (s.name.clone(), s.name.clone());
        let compared = self.compare.as_ref().filter(|(n, _)| *n == s.name).map(|(_, c)| c.clone());
        let shown = compared.is_some();
        let mut d = div()
            .flex()
            .flex_col()
            .gap(u(4.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(u(6.))
                    .child(div().flex_1().min_w_0().truncate().font_weight(FontWeight::BOLD).child(s.name.clone()))
                    .child(ui::button(
                        "lib-compare",
                        p,
                        if shown { "Hide Luna's" } else { "Compare with Luna's" },
                        yours,
                        self.act(cx, move |v, _, cx| v.compare_with_luna(compare.clone(), cx)),
                    ))
                    .child(ui::button(
                        "lib-reset",
                        p,
                        "Back to Luna's...",
                        yours && !busy,
                        self.act(cx, move |v, window, cx| v.guard(Next::Reset(reset.clone()), window, cx)),
                    )),
            )
            .child(ui::note(p, s.what()))
            .child(if s.state == "broken" { ui::alert(p, Tone::Attention, s.state_words()) } else { ui::note(p, s.state_words()) });
        if let Some(c) = compared {
            d = d.child(match c {
                Err(e) => ui::alert(p, Tone::Trouble, e),
                Ok(c) => match c.yours {
                    None => ui::note(p, "There is no override: Luna's own is served."),
                    Some(y) if y == c.luna => ui::note(p, "Yours is Luna's own, word for word."),
                    Some(_) => div()
                        .flex()
                        .flex_col()
                        .gap(u(2.))
                        .child(div().font_weight(FontWeight::BOLD).child("Luna's own text (yours is in the editor):"))
                        .child(ui_well(p, div().id("lib-luna-text").max_h(u(150.)).overflow_y_scroll().p(u(4.)).child(c.luna))),
                },
            });
        }
        d
    }

    fn folder_info(&self, o: &Overview, f: &super::Folder, busy: bool, p: &Palette, cx: &Context<Self>) -> Div {
        let path = f.path.clone();
        let mut d = div()
            .flex()
            .flex_col()
            .gap(u(4.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(u(6.))
                    .child(div().flex_1().min_w_0().truncate().font_weight(FontWeight::BOLD).child(ui::base_name(&f.path)))
                    .child(ui::button(
                        "lib-folder-remove",
                        p,
                        "Stop giving skills here",
                        !busy,
                        self.act(cx, move |v, _, cx| {
                            v.library.update(cx, |l, cx| l.set_folder(&path, false, cx)).detach();
                            v.selected = None;
                            cx.notify();
                        }),
                    )),
            )
            .child(ui::path_text(p, f.path.clone()))
            .when(!f.found, |d| d.child(ui::alert(p, Tone::Attention, "The folder is not there.")))
            .child(ui::note(
                p,
                "For the sessions Luna starts here, worktrees included: given at their start, nothing written into the folder. \
                 In a folder inside another, the inner one decides.",
            ))
            .child(ui::section(p, "Instructions here, in place of the account's"));
        let names: Vec<String> = o.instructions.iter().map(|i| i.name.clone()).collect();
        for (n, a) in by_cli(&o.accounts).into_iter().enumerate() {
            let label = format!("{} ({})", a.name, a.provider.label());
            if a.provider == Provider::Codex {
                d = d.child(ui::row(
                    p,
                    label,
                    Some("Codex reads its account's AGENTS.md in every session: a folder cannot change it.".into()),
                    div(),
                ));
                continue;
            }
            let decides = f.instructions_for(&a);
            let face = match &decides {
                None => "As outside".to_owned(),
                Some((_, None)) => "None".to_owned(),
                Some((_, Some(name))) => name.clone(),
            };
            let id = format!("lib-folder-ins-{n}");
            let open = self.menu.as_deref() == Some(id.as_str());
            let mut choices: Vec<(String, Option<Option<String>>)> = vec![("As outside".into(), None), ("None".into(), Some(None))];
            choices.extend(names.iter().map(|n| (n.clone(), Some(Some(n.clone())))));
            let items = choices
                .into_iter()
                .map(|(label, choice)| {
                    let chosen = label == face;
                    let (path, a) = (f.path.clone(), a.clone());
                    DropItem {
                        label: label.into(),
                        chosen,
                        tick: false,
                        enabled: !busy,
                        group: None,
                        pick: self.act(cx, move |v, _, cx| {
                            v.library.update(cx, |l, cx| l.set_folder_instructions(&path, &a, choice.clone(), cx)).detach();
                            v.close_menu(cx);
                        }),
                    }
                })
                .collect();
            let toggle = id.clone();
            d = d.child(ui::row(
                p,
                label,
                None,
                Dropdown {
                    id,
                    face: face.into(),
                    open,
                    enabled: !busy,
                    width: u(180.),
                    items,
                    on_toggle: self.act(cx, move |v, _, cx| v.toggle_menu(&toggle, cx)),
                    on_close: self.act(cx, |v, _, cx| v.close_menu(cx)),
                }
                .render(p),
            ));
        }
        d.child(ui::note(p, "Which skills the sessions here get: pick a skill on the left, under Sessions in folders."))
    }

    fn question(&self, ask: Ask, _p: &Palette, cx: &Context<Self>) -> Confirm {
        let (title, body, yes) = match &ask {
            Ask::Delete(Entry::Skill(n)) => (
                "Take the skill out",
                format!(
                    "Moves {n} out of the library into Luna's backups, and takes its copies out wherever they are still exactly as Luna wrote them."
                ),
                "Take out",
            ),
            Ask::Delete(entry) => (
                "Take the instructions out",
                format!(
                    "Moves {} out of the library into Luna's backups. The accounts it went into get the next instructions that take them in, or none.",
                    entry.name()
                ),
                "Take out",
            ),
            Ask::Reset(n) => (
                "Back to Luna's own",
                format!(
                    "Moves your override of {n} out of the library into Luna's backups, kept for 30 days. Luna's own text is served from then on."
                ),
                "Back to Luna's",
            ),
            Ask::Discard(_) => ("Unsaved changes", "The editor has changes not saved. Throw them away?".to_owned(), "Throw away"),
        };
        Confirm {
            title: title.into(),
            body: body.into(),
            yes: yes.into(),
            on_yes: self.act(cx, |v, window, cx| v.yes(window, cx)),
            on_no: self.act(cx, |v, _, cx| {
                v.ask = None;
                cx.notify();
            }),
        }
    }
}

/// What the driver's lines and the tests do as a click would.
#[cfg(any(test, luna_test, feature = "shots"))]
impl LibraryPanel {
    /// The question up answered yes, as its button does.
    pub fn answer_yes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.yes(window, cx);
    }

    /// Rename typed and committed, as its field and button do.
    pub fn rename_to(&mut self, entry: Entry, to: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.start_rename(entry, window, cx);
        if let Some((_, field)) = &self.renaming {
            field.update(cx, |f, cx| f.set_value(to.to_owned(), window, cx));
        }
        self.commit_rename(window, cx);
    }

    /// The editor's Reload or Overwrite, as its buttons do.
    pub fn settle_conflict(&mut self, overwrite: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(e) = self.editor.clone() {
            e.update(cx, |e, cx| if overwrite { e.overwrite(cx) } else { e.reload(window, cx) });
        }
    }

    /// The draft on offer restored, as its button does.
    pub fn restore_offer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(d) = self.offer.clone() {
            self.guard(Next::Restore(d), window, cx);
        }
    }
}

#[cfg(test)]
impl LibraryPanel {
    pub fn new_skill_field(&self) -> Entity<InputState> {
        self.new_skill.clone()
    }

    pub fn new_instructions_field(&self) -> Entity<InputState> {
        self.new_instructions.clone()
    }

    /// A question is up.
    pub fn asking(&self) -> bool {
        self.ask.is_some()
    }

    /// Delete... on the entry picked, as its button does.
    pub fn ask_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(entry) = self.selected.as_ref().and_then(Sel::entry) {
            self.guard(Next::Delete(entry), window, cx);
        }
    }

    /// The question up is the discard question.
    pub fn asking_discard(&self) -> bool {
        matches!(self.ask, Some(Ask::Discard(_)))
    }

    pub fn ask_reset(&mut self, name: String, cx: &mut Context<Self>) {
        self.ask = Some(Ask::Reset(name));
        cx.notify();
    }

    /// The system text Luna's own is shown beside.
    pub fn comparing(&self) -> Option<&str> {
        self.compare.as_ref().map(|(n, _)| n.as_str())
    }
}

/// A sunken well around `content`, white (black in the dark theme).
fn ui_well(p: &Palette, content: impl IntoElement) -> Div {
    crate::widgets::bevel(p, false, div().size_full().bg(p.well).child(content))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acc(p: Provider, n: &str) -> AccountRef {
        AccountRef { provider: p, name: n.into() }
    }

    /// The picker's ticks send the reach the page's would: a CLI's line takes
    /// its accounts' in, an account's goes once the CLI's is ticked, and a
    /// folder's first line hands it back to what holds outside.
    #[test]
    fn the_reach_pickers_ticks_send_what_the_page_sends() {
        let accounts = vec![acc(Provider::Claude, "work"), acc(Provider::Claude, "home"), acc(Provider::Codex, "x")];
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let items = reach_items(Some(&s(&["claude/work"])), None, &accounts);
        let labels: Vec<&str> = items.iter().map(|i| i.label.trim()).collect();
        assert_eq!(labels, ["All accounts", "All Claude Code", "work", "home", "All Codex", "x"]);
        assert!(items[2].checked && !items[3].checked);
        assert_eq!(items[3].next, Some(s(&["claude/work", "claude/home"])));
        assert_eq!(items[2].next, Some(s(&[])));
        assert_eq!(items[1].next, Some(s(&["claude/work", "claude"])));
        assert_eq!(items[0].next, Some(s(&["all"])));
        // Every Claude Code account: its accounts' boxes are ticked and fixed.
        let items = reach_items(Some(&s(&["claude"])), None, &accounts);
        assert!(items[2].checked && !items[2].enabled);
        // A folder that says nothing: its ticks show who gets it outside.
        let items = reach_items(None, Some(&s(&["codex/x"])), &accounts);
        assert_eq!(items[0].label, "As outside this folder");
        assert!(items[0].checked);
        assert_eq!(items[0].next, Some(s(&["codex/x"])), "unticked, the folder's own starts from it");
        assert!(items.iter().find(|i| i.label.trim() == "x").unwrap().checked);
        let items = reach_items(Some(&s(&[])), Some(&s(&["codex/x"])), &accounts);
        assert_eq!(items[0].next, None, "ticked, the folder says nothing again");
    }
}
