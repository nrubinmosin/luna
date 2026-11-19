//! The helpers popover (src/features/agents/BackgroundChip.tsx, HelperView):
//! the background helpers a chat's agent runs, with what each is doing, and
//! what a person can do with one: watch it as plain text, read-only and live
//! while its turn runs; open it as a chat of their own (which takes it from
//! its agent for good); stop it; delete it. Its agent is the one told what the
//! helpers say; this is for a person who wants to look. Moving a helper to
//! another chat is cut (ADR-0030), and so is handing a chat's context off.
//!
//! The list is asked of the core when the popover opens and again every
//! `LOOK_EVERY` while it is up: the page was sent every change, the window
//! asks only while someone looks.

use super::choice::{
    CODEX_APPROVALS, CODEX_SANDBOXES, ClaudeSettings, CodexSettings, Settings, effort_from_setting, model_from_setting, perm_from_setting,
    same_path,
};
use super::create::{self, Make};
use super::rows::{self, Helper, Tab};
use super::{DialogLayer, cmd};
use crate::models::Models;
use crate::panels::ui::{self, Confirm, OnClick, Tone};
use crate::panels::{ask, now_ms};
use crate::term::paste::Provider;
use crate::theme::Palette;
use gpui::{
    Context, Div, InteractiveElement, IntoElement, ParentElement, Render, ScrollHandle, SharedString, StatefulInteractiveElement, Styled,
    Task, WeakEntity, Window, div, px,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::rc::Rc;
use std::time::Duration;

/// How often the list is asked for while the popover is up.
pub const LOOK_EVERY: Duration = Duration::from_secs(2);
/// How often a watched transcript is read while a turn runs, and between turns.
pub const TAIL_RUNNING: Duration = Duration::from_secs(1);
pub const TAIL_IDLE: Duration = Duration::from_secs(5);
/// The most messages a watch keeps: the newest.
pub const VIEW_KEEP: usize = 400;
/// How long to wait before each try again to tell the core a row is made: a
/// core restarting has it on a later try.
const SETTLE_RETRY: [Duration; 4] = [Duration::from_millis(500), Duration::from_secs(1), Duration::from_secs(2), Duration::from_secs(4)];

/// A helper to be handed over to the person as a chat (`background_promote`).
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Promoted {
    pub id: String,
    pub provider: String,
    pub account_path: String,
    pub folder: String,
    pub worktree_path: Option<String>,
    pub session_id: Option<String>,
    pub name: Option<String>,
    /// The flags it ran with, in the CLI's own words.
    pub settings: serde_json::Map<String, Value>,
    pub tools: bool,
}

/// What a look at a helper's conversation answers (`background_transcript`).
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
struct Transcript {
    messages: Vec<Message>,
    cursor: i64,
    file: Option<String>,
    running: bool,
    left_out: u64,
    passed_over: bool,
}

#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Message {
    pub role: String,
    pub text: String,
}

/// A helper watched: its conversation, tailed.
struct Watch {
    id: String,
    name: String,
    messages: Vec<Message>,
    cursor: Option<i64>,
    file: Option<String>,
    running: bool,
    left_out: u64,
    passed_over: bool,
    loading: bool,
    /// Why there is nothing more to show: deleted, or opened as a chat.
    gone: Option<String>,
    scroll: ScrollHandle,
    _tail: Option<Task<()>>,
}

pub struct HelpersView {
    models: Models,
    layer: WeakEntity<DialogLayer>,
    /// The chat whose helpers are listed; None for all of them.
    chat: Option<String>,
    all: Vec<Helper>,
    loaded: bool,
    tab: Option<Tab>,
    pub error: Option<String>,
    watching: Option<Watch>,
    /// The helper whose opening as a chat waits on a yes.
    handing: Option<Helper>,
    /// Helpers being opened: a second click while the first is on its way
    /// makes one row, once.
    opening: HashSet<String>,
    _look: Task<()>,
}

impl HelpersView {
    pub fn new(models: Models, layer: WeakEntity<DialogLayer>, chat: Option<String>, cx: &mut Context<Self>) -> HelpersView {
        let host = models.host.clone();
        let look = cx.spawn(async move |this, cx| {
            loop {
                let rows = ask::<Vec<Helper>>(&host, cmd::BACKGROUND_HELPERS, json!({})).await;
                let alive = this
                    .update(cx, |v, cx| {
                        match rows {
                            Ok(rows) => {
                                v.all = rows;
                                v.loaded = true;
                            }
                            Err(e) => {
                                v.loaded = true;
                                v.error = Some(e);
                            }
                        }
                        cx.notify();
                    })
                    .is_ok();
                if !alive {
                    return;
                }
                cx.background_executor().timer(LOOK_EVERY).await;
            }
        });
        HelpersView {
            models,
            layer,
            chat,
            all: Vec::new(),
            loaded: false,
            tab: None,
            error: None,
            watching: None,
            handing: None,
            opening: HashSet::new(),
            _look: look,
        }
    }

    /// The helpers listed: a chat's, its helpers' helpers with them, or all.
    pub fn shown(&self) -> Vec<Helper> {
        match &self.chat {
            Some(chat) => rows::helpers_under(&self.all, chat),
            None => self.all.clone(),
        }
    }

    fn act(&self, cx: &Context<Self>, f: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static) -> OnClick {
        let this = cx.weak_entity();
        Rc::new(move |window, cx| {
            let _ = this.update(cx, |v, cx| f(v, window, cx));
        })
    }

    /// Esc: a question first, then a helper being watched, then the list.
    pub fn may_close(&mut self, cx: &mut Context<Self>) -> bool {
        if self.handing.take().is_some() || self.watching.take().is_some() {
            cx.notify();
            return false;
        }
        true
    }

    /// The driver's lines (mod.rs `drive`): each does what its click does.
    #[cfg(any(test, luna_test, feature = "shots"))]
    pub(crate) fn drive(&mut self, what: &str, id: &str, cx: &mut Context<Self>) {
        let helper = self.all.iter().find(|h| h.id == id).cloned();
        match (what, helper) {
            ("tab", _) => {
                self.tab = Some(if id == "idle" { Tab::Idle } else { Tab::Working });
                cx.notify();
            }
            ("watch", _) => self.watch(id, cx),
            ("stop", _) => self.stop(id, cx),
            ("delete", _) => self.delete(id, cx),
            ("open", Some(h)) => {
                self.handing = Some(h);
                cx.notify();
            }
            ("yes", _) => {
                if let Some(h) = self.handing.take() {
                    self.open_as_chat(&h.id, cx);
                }
                cx.notify();
            }
            _ => log::warn!("drive: helpers {what} {id}: not understood"),
        }
    }

    // ----------------------------------------------------------- the rows

    /// Stops the turn it is on, or between turns what its turns left running;
    /// the conversation stays.
    pub fn stop(&mut self, id: &str, cx: &mut Context<Self>) {
        self.error = None;
        let call = ask::<Value>(&self.models.host, cmd::BACKGROUND_KILL, json!({ "id": id }));
        self.finish(call, "stopping a helper", cx);
    }

    /// Deletes it, the worktree it made too.
    pub fn delete(&mut self, id: &str, cx: &mut Context<Self>) {
        self.error = None;
        let call = ask::<Value>(
            &self.models.host,
            cmd::BACKGROUND_DELETE,
            json!({ "id": id, "dropWorktree": true, "stopWorking": false, "dropUnsaved": false, "ofChat": null }),
        );
        let id = id.to_owned();
        cx.spawn(async move |this, cx| {
            let done = call.await;
            let _ = this.update(cx, |v, cx| {
                match done {
                    // Its worktree held work saved nowhere else: kept, and said.
                    Ok(r) => {
                        if let Some(kept) = r.get("worktreeKept").and_then(Value::as_str) {
                            v.error = Some(format!("Deleted; its worktree was kept for its uncommitted work: {kept}"));
                        }
                        v.all.retain(|h| h.id != id);
                    }
                    Err(e) => v.error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn finish(&mut self, call: impl Future<Output = Result<Value, String>> + 'static, what: &'static str, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            if let Err(e) = call.await {
                log::warn!("{what} failed: {e}");
                let _ = this.update(cx, |v, cx| {
                    v.error = Some(e);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    // ----------------------------------------------------------- watching

    /// Watches helper `id` as plain text, read-only.
    pub fn watch(&mut self, id: &str, cx: &mut Context<Self>) {
        let name = self.all.iter().find(|h| h.id == id).map_or_else(|| id.to_owned(), |h| h.label().to_owned());
        let host = self.models.host.clone();
        let helper = id.to_owned();
        let tail = cx.spawn(async move |this, cx| {
            loop {
                let (since, file) = match this.update(cx, |v, _| v.watching.as_ref().map(|w| (w.cursor, w.file.clone()))) {
                    Ok(Some(at)) => at,
                    _ => return,
                };
                let look =
                    ask::<Transcript>(&host, cmd::BACKGROUND_TRANSCRIPT, json!({ "id": helper, "since": since, "file": file })).await;
                let mut wait = TAIL_IDLE;
                let alive = this.update(cx, |v, cx| {
                    let Some(w) = v.watching.as_mut().filter(|w| w.id == helper) else { return false };
                    match &look {
                        Ok(t) => {
                            let fresh = t.file != w.file;
                            w.cursor = Some(t.cursor);
                            w.file = t.file.clone();
                            if fresh {
                                w.messages.clear();
                                w.left_out = 0;
                                w.passed_over = false;
                            }
                            w.messages.extend(t.messages.iter().cloned());
                            if w.messages.len() > VIEW_KEEP {
                                let cut = w.messages.len() - VIEW_KEEP;
                                w.messages.drain(..cut);
                                w.left_out += cut as u64;
                            }
                            w.left_out += t.left_out;
                            w.passed_over |= t.passed_over;
                            w.running = t.running;
                            w.loading = false;
                            w.gone = None;
                            w.scroll.scroll_to_bottom();
                            if t.running {
                                wait = TAIL_RUNNING;
                            }
                        }
                        // Gone (deleted, or opened as a chat): nothing more to show.
                        Err(e) => {
                            w.running = false;
                            w.loading = false;
                            w.gone = Some(e.clone());
                        }
                    }
                    cx.notify();
                    look.is_ok()
                });
                if !matches!(alive, Ok(true)) {
                    return;
                }
                cx.background_executor().timer(wait).await;
            }
        });
        self.watching = Some(Watch {
            id: id.to_owned(),
            name,
            messages: Vec::new(),
            cursor: None,
            file: None,
            running: false,
            left_out: 0,
            passed_over: false,
            loading: true,
            gone: None,
            scroll: ScrollHandle::new(),
            _tail: Some(tail),
        });
        cx.notify();
    }

    // ------------------------------------------------------- open as chat

    /// Opens a helper as a chat, handing it over to the person for good: the
    /// agent that spawned it has no claim on it from then on, and hears so
    /// once. The core holds the helper, with no turn starting, until the row
    /// is made, and lets it go only then: a row that could not be made leaves
    /// the helper as it was, its agent's, never neither. The row is seated in
    /// a pane, which resumes the conversation, only once the core has let go.
    pub fn open_as_chat(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.opening.insert(id.to_owned()) {
            return;
        }
        self.error = None;
        let (models, id) = (self.models.clone(), id.to_owned());
        cx.spawn(async move |this, cx| {
            let done = promote(&models, &id, cx).await;
            let _ = this.update(cx, |v, cx| {
                v.opening.remove(&id);
                if let Err(e) = done {
                    log::warn!("opening a helper failed: {e}");
                    v.error = Some(e);
                }
                cx.notify();
            });
        })
        .detach();
    }
}

/// The flags a helper ran with, over what its account would open on.
pub fn promoted_settings(provider: Provider, flags: &serde_json::Map<String, Value>, defaults: Settings) -> Settings {
    let word = |k: &str| flags.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
    match (provider, defaults) {
        (Provider::Claude, Settings::Claude(d)) => Settings::Claude(ClaudeSettings {
            model: model_from_setting(&word("model")).unwrap_or(d.model),
            effort: effort_from_setting(&word("effort")).unwrap_or(d.effort),
            // A mode no chat runs in (`dontAsk`) opens on Ask, its rung, never
            // on the account's default, which may go further.
            perm: match word("permissionMode") {
                m if m.is_empty() => d.perm,
                m => perm_from_setting(&m).unwrap_or("Ask").to_owned(),
            },
        }),
        (_, Settings::Codex(d)) => Settings::Codex(CodexSettings {
            model: Some(word("model")).filter(|m| !m.is_empty()).or(d.model),
            effort: effort_from_setting(&word("effort")).unwrap_or(d.effort),
            approval: CODEX_APPROVALS.iter().find(|a| **a == word("approval").to_lowercase()).map_or(d.approval, |a| (*a).to_owned()),
            sandbox: CODEX_SANDBOXES.iter().find(|a| **a == word("sandbox").to_lowercase()).map_or(d.sandbox, |a| (*a).to_owned()),
        }),
        (_, other) => other,
    }
}

/// The whole of opening one helper as a chat.
async fn promote(models: &Models, id: &str, cx: &mut gpui::AsyncApp) -> Result<(), String> {
    let host = &models.host;
    let p: Promoted = ask(host, cmd::BACKGROUND_PROMOTE, json!({ "id": id })).await?;
    let made = async {
        let provider = if p.provider == "codex" { Provider::Codex } else { Provider::Claude };
        let ws = models.workspace.clone();
        let account = cx
            .update(|cx| {
                ws.read(cx)
                    .accounts()
                    .and_then(|a| a.iter().find(|a| a.provider == provider && same_path(&a.path, &p.account_path)).cloned())
            })
            .ok_or_else(|| format!("no {} account at {}", p.provider, p.account_path))?;
        let defaults = super::new_chat::defaults_of(host, &account, &p.folder).await;
        let settings = promoted_settings(provider, &p.settings, defaults);
        let mut make = Make::new(&p.folder, account, settings);
        make.id = Some(p.id.clone());
        make.worktree = p.worktree_path.is_some();
        make.tools = p.tools;
        make.name = p.name.clone();
        make.in_worktree = p.worktree_path.clone();
        make.from_helper = true;
        make.read_only = p.settings.get("readOnly").and_then(Value::as_bool) == Some(true);
        make.agent = p.settings.get("agent").and_then(Value::as_str).filter(|a| !a.is_empty()).map(str::to_owned);
        make.session_id = p.session_id.clone();
        // A helper always runs with the wait guard; the chat keeps it until
        // the person turns it off in its panel, rather than losing it on the
        // handover.
        make.guard = true;
        create::create(models, make, cx).await
    }
    .await;
    if let Err(why) = &made {
        let _ = ask::<Value>(host, cmd::BACKGROUND_PROMOTED, json!({ "id": id, "error": why })).await;
    }
    made?;
    let ws = models.workspace.clone();
    if settle(host, id, cx).await {
        cx.update(|cx| ws.update(cx, |w, cx| w.place_chat(id, cx)));
    } else {
        // Went back to its agent before its opening was settled, or the core
        // never said: the row would only run its conversation beside it.
        cx.update(|cx| ws.update(cx, |w, cx| w.remove_chat(id, cx)));
    }
    Ok(())
}

/// Tells the core the row for helper `id` is made, trying again as long as
/// `SETTLE_RETRY` says. True once the helper is the person's.
async fn settle(host: &std::sync::Arc<dyn crate::host::Host>, id: &str, cx: &mut gpui::AsyncApp) -> bool {
    for i in 0.. {
        match ask::<bool>(host, cmd::BACKGROUND_PROMOTED, json!({ "id": id, "error": null })).await {
            Ok(handed) => {
                if !handed {
                    log::warn!("helper {id} went back to its agent before its opening was settled; its row is let go");
                }
                return handed;
            }
            Err(e) => match SETTLE_RETRY.get(i) {
                Some(wait) => cx.background_executor().timer(*wait).await,
                None => {
                    log::warn!("telling the core helper {id} is opened failed: {e}; the next core to come up is told");
                    return false;
                }
            },
        }
    }
    false
}

impl Render for HelpersView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.models.shell.read(cx).palette();
        let close = self.act(cx, |v, window, cx| {
            let _ = v.layer.update(cx, |l, cx| l.close(window, cx));
        });
        if let Some(w) = &self.watching {
            return self.watch_frame(&p, w, cx).into_any_element();
        }
        let helpers = self.shown();
        let tab = self.tab.unwrap_or_else(|| rows::default_tab(&helpers));
        let (working, idle) = (rows::in_tab(&helpers, Tab::Working), rows::in_tab(&helpers, Tab::Idle));
        let tabs = ui::segmented(
            "helpers-tabs",
            &p,
            vec![
                (
                    SharedString::from(format!("Working {}", working.len())),
                    tab == Tab::Working,
                    self.act(cx, |v, _, cx| {
                        v.tab = Some(Tab::Working);
                        cx.notify();
                    }),
                ),
                (
                    SharedString::from(format!("Idle {}", idle.len())),
                    tab == Tab::Idle,
                    self.act(cx, |v, _, cx| {
                        v.tab = Some(Tab::Idle);
                        cx.notify();
                    }),
                ),
            ],
            true,
        );
        let shown = if tab == Tab::Working { working } else { idle };
        let now = now_ms();
        let mut list = div().id("helpers-rows").flex().flex_col().gap(px(6.)).max_h(px(420.)).overflow_y_scroll();
        if !self.loaded {
            list = list.child(ui::note(&p, "Asking the core for the helpers..."));
        } else if shown.is_empty() {
            list = list.child(ui::note(&p, if tab == Tab::Working { "None at work." } else { "None idle." }));
        }
        for h in shown {
            list = list.child(self.row(&p, h, now, cx));
        }
        let mut body = div().flex().flex_col().gap(px(6.)).p(px(8.)).child(tabs).child(list);
        if let Some(e) = &self.error {
            body = body.child(ui::alert(&p, Tone::Trouble, e.clone()));
        }
        let title = match &self.chat {
            Some(_) => format!("Background helpers of this chat ({})", helpers.len()),
            None => format!("Background helpers ({})", helpers.len()),
        };
        let mut frame = ui::frame(&p, "helpers", title, px(440.), close, body).relative();
        if let Some(h) = self.handing.clone() {
            let id = h.id.clone();
            frame = frame.child(
                Confirm {
                    title: format!("Open \"{}\" as your chat?", h.label()).into(),
                    body: "It becomes a chat of your own, for good: its agent can no longer send to it, wait on it, stop or delete it, and is told so once. Anything still queued for it is not delivered; whoever sent it hears where it is.".into(),
                    yes: "Open as chat".into(),
                    on_yes: self.act(cx, move |v, _, cx| {
                        v.handing = None;
                        v.open_as_chat(&id, cx);
                        cx.notify();
                    }),
                    on_no: self.act(cx, |v, _, cx| {
                        v.handing = None;
                        cx.notify();
                    }),
                }
                .render(&p, "helpers-ask"),
            );
        }
        frame.into_any_element()
    }
}

impl HelpersView {
    fn row(&self, p: &Palette, h: &Helper, now: i64, cx: &Context<Self>) -> gpui::Stateful<Div> {
        let id = h.id.clone();
        let busy = h.busy();
        let left = h.state == "background" && !h.left_running.is_empty();
        let opening = self.opening.contains(&h.id);
        let button = |what: &str, label: &str, enabled: bool, f: Rc<dyn Fn(&mut Self, &mut Context<Self>)>| {
            let this = cx.weak_entity();
            ui::button(
                &format!("helper-{what}-{id}"),
                p,
                label.to_owned(),
                enabled,
                Rc::new(move |_, cx| {
                    let _ = this.update(cx, |v, cx| f(v, cx));
                }),
            )
        };
        let (watch_id, stop_id, delete_id, open) = (id.clone(), id.clone(), id.clone(), h.clone());
        let mut row = div()
            .id(SharedString::from(format!("helper-{id}")))
            .debug_selector({
                let id = id.clone();
                move || format!("helper-{id}")
            })
            .flex()
            .flex_col()
            .gap(px(2.))
            .p(px(4.))
            .border_1()
            .border_color(if h.asks() { p.attention } else { p.frame })
            .child(
                div()
                    .flex()
                    .gap(px(6.))
                    .child(div().flex_1().min_w_0().truncate().font_weight(gpui::FontWeight::BOLD).child(h.label().to_owned()))
                    .child(div().flex_none().text_color(p.text_dim).child(h.state_line(now))),
            )
            .child(
                div()
                    .truncate()
                    .text_color(p.text_dim)
                    .child([Some(h.setup_line()), h.activity_line(now)].into_iter().flatten().collect::<Vec<_>>().join(" · ")),
            );
        if let Some(task) = &h.task {
            row = row.child(div().truncate().child(task.clone()));
        }
        if let Some(ask) = h.ask_line() {
            row = row.child(div().truncate().bg(p.attention).text_color(p.on_status).px(px(4.)).child(ask));
        }
        row.child(
            div()
                .flex()
                .gap(px(4.))
                .child(button("watch", "Watch", true, Rc::new(move |v, cx| v.watch(&watch_id, cx))))
                .child(button(
                    "open",
                    "Open as chat",
                    !busy && !left && !opening,
                    Rc::new(move |v, cx| {
                        v.handing = Some(open.clone());
                        cx.notify();
                    }),
                ))
                .child(button("stop", "Stop", busy || left, Rc::new(move |v, cx| v.stop(&stop_id, cx))))
                .child(button("delete", "Delete", !busy && !left, Rc::new(move |v, cx| v.delete(&delete_id, cx)))),
        )
    }

    fn watch_frame(&self, p: &Palette, w: &Watch, cx: &Context<Self>) -> gpui::Stateful<Div> {
        let back = self.act(cx, |v, _, cx| {
            v.watching = None;
            cx.notify();
        });
        let close = self.act(cx, |v, window, cx| {
            let _ = v.layer.update(cx, |l, cx| l.close(window, cx));
        });
        let status = match (&w.gone, w.loading, w.running) {
            (Some(why), _, _) => format!("no longer a helper: {why}"),
            (None, true, _) => "reading...".to_owned(),
            (None, false, true) => "working, updates as it goes".to_owned(),
            (None, false, false) => "between turns".to_owned(),
        };
        let mut head = status;
        if w.left_out > 0 {
            head.push_str(&format!(" · {} older message{} not shown", w.left_out, if w.left_out > 1 { "s" } else { "" }));
        }
        if w.passed_over {
            head.push_str(" · earlier parts and very long entries not read");
        }
        let mut messages = div()
            .id("helpers-watch-messages")
            .flex()
            .flex_col()
            .gap(px(8.))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&w.scroll)
            .bg(p.well)
            .p(px(6.));
        if w.messages.is_empty() && !w.loading {
            messages = messages.child(ui::note(p, "Nothing in its conversation yet."));
        }
        for (i, m) in w.messages.iter().enumerate() {
            let who = match m.role.as_str() {
                "user" => "Asked",
                "assistant" => "Said",
                other => other,
            };
            messages = messages.child(
                div()
                    .id(SharedString::from(format!("helpers-msg-{i}")))
                    .debug_selector(move || format!("helpers-msg-{i}"))
                    .flex()
                    .flex_col()
                    .child(div().font_weight(gpui::FontWeight::BOLD).text_color(p.text_dim).child(who.to_owned()))
                    .child(div().child(m.text.clone())),
            );
        }
        let body = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap(px(6.))
            .p(px(8.))
            .child(
                div().flex().items_center().gap(px(6.)).child(ui::button("helpers-back", p, "Back to the list", true, back)).child(
                    div()
                        .id("helpers-watch-status")
                        .debug_selector(|| "helpers-watch-status".into())
                        .flex_1()
                        .text_color(p.text_dim)
                        .child(head),
                ),
            )
            .child(messages);
        ui::frame(p, "helpers-watch", format!("{} - read-only", w.name), px(760.), close, body).h(px(560.))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialogs::choice;

    fn flags(v: Value) -> serde_json::Map<String, Value> {
        v.as_object().cloned().unwrap()
    }

    #[test]
    fn a_helper_opened_as_a_chat_keeps_the_flags_it_ran_with() {
        let defaults = Settings::Claude(choice::claude_stock());
        let s = promoted_settings(
            Provider::Claude,
            &flags(json!({ "model": "sonnet", "effort": "low", "permissionMode": "plan" })),
            defaults.clone(),
        );
        assert_eq!(s, Settings::Claude(ClaudeSettings { model: "Sonnet".into(), effort: "low".into(), perm: "Plan only".into() }));
        // A mode no chat runs in opens on Ask, never on the account's default.
        let s = promoted_settings(Provider::Claude, &flags(json!({ "permissionMode": "dontAsk" })), defaults.clone());
        assert!(matches!(s, Settings::Claude(c) if c.perm == "Ask"));
        // Nothing named: what the account would open on.
        assert_eq!(promoted_settings(Provider::Claude, &flags(json!({})), defaults.clone()), defaults);
        let codex = Settings::Codex(choice::codex_stock());
        let s =
            promoted_settings(Provider::Codex, &flags(json!({ "model": "gpt-6", "approval": "on-request", "sandbox": "bogus" })), codex);
        assert!(
            matches!(s, Settings::Codex(c) if c.model.as_deref() == Some("gpt-6") && c.approval == "on-request" && c.sandbox == "danger-full-access")
        );
    }
}
