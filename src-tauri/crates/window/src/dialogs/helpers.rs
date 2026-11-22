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
use super::rows::{self, Helper, Names, Tab};
use super::{DialogLayer, cmd};
use crate::models::Models;
use crate::panels::ui::{self, Confirm, OnClick, Tone};
use crate::panels::{ask, now_ms};
use crate::scale::u;
use crate::selector::Selector;
use crate::term::paste::Provider;
use crate::theme::Palette;
use gpui::prelude::FluentBuilder;
use gpui::{
    Context, Div, InteractiveElement, IntoElement, ParentElement, Render, ScrollHandle, SharedString, StatefulInteractiveElement, Styled,
    Task, WeakEntity, Window, div,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// How often the list is asked for while the popover is up.
pub const LOOK_EVERY: Duration = Duration::from_secs(2);
/// How often a watched transcript is read while a turn runs, and between turns.
pub const TAIL_RUNNING: Duration = Duration::from_secs(1);
pub const TAIL_IDLE: Duration = Duration::from_secs(5);
/// The most messages a watch keeps: the newest.
pub const VIEW_KEEP: usize = 400;
/// How near its end, in pixels, a watch counts as read to the end: new output
/// is followed from there, and a reader scrolled further up is left where
/// they are (0.19's HelperView).
const AT_END_SLACK: f32 = 24.;
/// How long to wait before each try again to tell the core a row is made, as
/// 0.19's bridge did: a core restarting, or taken over by another, has it on a
/// later try. Past the last, the next core to come up is told
/// (workspace.rs `settle_openings`), and one that never is gives the helper
/// back to its agent (background/rows.rs `PROMOTE_TIMEOUT_MS`).
pub(crate) const SETTLE_RETRY: [Duration; 8] = [
    Duration::from_millis(500),
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
    Duration::from_secs(15),
    Duration::from_secs(30),
    Duration::from_secs(60),
];
/// How many of `SETTLE_RETRY` an update's last write of the list waits
/// through (workspace.rs `SETTLE_LIMIT`); the rest are tried with the row
/// in the list, unseated, the write no longer waiting.
pub(crate) const SETTLE_HELD: usize = 4;
/// How long one word to the core about an opening waits for its answer: a
/// core going away may take a minute to refuse the line, or never answer.
pub(crate) const SETTLE_CALL: Duration = Duration::from_secs(3);
/// How long the part of an opening an update's last write waits for takes at
/// most, from the helper asked for to the held tries to tell the core its row
/// is made (`promote_admitted`, workspace.rs `settle_openings`): every word to
/// the core in it is bounded to end by then (`bounded_until`), the word that
/// it failed kept `SETTLE_CALL` for. Under the update's `SETTLE_LIMIT`, with
/// room for the round of looks it may also wait for to end.
pub(crate) const OPENING_HELD: Duration = Duration::from_secs(20);
const _: () = assert!(OPENING_HELD.as_secs() + SETTLE_CALL.as_secs() < crate::workspace::SETTLE_LIMIT.as_secs());

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
    _look: Task<()>,
    /// The workspace's openings end without a word here (`settle_openings`):
    /// redrawn as it changes, Open as chat comes back at once.
    _openings: gpui::Subscription,
}

impl HelpersView {
    pub fn new(models: Models, layer: WeakEntity<DialogLayer>, chat: Option<String>, cx: &mut Context<Self>) -> HelpersView {
        let host = models.host.clone();
        let look = cx.spawn(async move |this, cx| {
            loop {
                let Ok(asked_of) = this.update(cx, |v, cx| v.models.workspace.read(cx).this_core(cx)) else { return };
                let rows = ask::<Vec<Helper>>(&host, cmd::BACKGROUND_HELPERS, json!({})).await;
                let alive = this
                    .update(cx, |v, cx| {
                        match rows {
                            Ok(rows) => {
                                // The workspace's list too, which settles what is
                                // left of an opening: a core with nothing to say
                                // sends no list of its own.
                                v.models.workspace.update(cx, |w, cx| w.on_polled_helpers(rows.clone(), asked_of, cx));
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
        let openings = cx.observe(&models.workspace, |_, _, cx| cx.notify());
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
            _look: look,
            _openings: openings,
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

    /// The watched helper's scroll, for the tests.
    #[cfg(test)]
    pub(crate) fn watch_scroll(&self) -> Option<ScrollHandle> {
        self.watching.as_ref().map(|w| w.scroll.clone())
    }

    // ----------------------------------------------------------- the rows

    /// Stops the turn it is on, or between turns what its turns left running;
    /// the conversation stays.
    pub fn stop(&mut self, id: &str, cx: &mut Context<Self>) {
        self.error = None;
        let call = ask::<Value>(&self.models.host, cmd::BACKGROUND_KILL, json!({ "id": id }));
        self.finish(call, "stopping a helper", cx);
    }

    /// Deletes it; a worktree its CLI made stays on disk (ADR-0030).
    pub fn delete(&mut self, id: &str, cx: &mut Context<Self>) {
        self.error = None;
        let call = ask::<Value>(&self.models.host, cmd::BACKGROUND_DELETE, json!({ "id": id, "stopWorking": false, "ofChat": null }));
        let id = id.to_owned();
        cx.spawn(async move |this, cx| {
            let done = call.await;
            let _ = this.update(cx, |v, cx| {
                match done {
                    Ok(_) => v.all.retain(|h| h.id != id),
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
                            // Measured before the new messages lay out: where
                            // the reader was, not where they would be.
                            let follow = fresh || at_end(w.scroll.offset().y.into(), w.scroll.max_offset().y.into());
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
                                // A reader away from the end keeps the text they
                                // read where it was: the offset moves up by what
                                // goes from the top, as it was laid out.
                                if !follow {
                                    let top = |i: usize| w.scroll.bounds_for_item(i).map(|b| f32::from(b.origin.y));
                                    let y = kept_in_place(w.scroll.offset().y.into(), top(0), top(cut));
                                    w.scroll.set_offset(gpui::point(w.scroll.offset().x, gpui::px(y)));
                                }
                                w.messages.drain(..cut);
                                w.left_out += cut as u64;
                            }
                            w.left_out += t.left_out;
                            w.passed_over |= t.passed_over;
                            w.running = t.running;
                            w.loading = false;
                            w.gone = None;
                            if follow && (fresh || !t.messages.is_empty()) {
                                w.scroll.scroll_to_bottom();
                            }
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
    /// A second click while the first is on its way makes one row, once, and
    /// the settling of openings as a core comes up meanwhile leaves it be
    /// (`Workspace::begin_opening`).
    pub fn open_as_chat(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.models.workspace.update(cx, |w, _| w.begin_opening(id)) {
            return;
        }
        self.error = None;
        let (models, id) = (self.models.clone(), id.to_owned());
        cx.spawn(async move |this, cx| {
            let done = promote(&models, &id, cx).await;
            let ws = models.workspace.clone();
            cx.update(|cx| ws.update(cx, |w, cx| w.end_opening(&id, cx)));
            let _ = this.update(cx, |v, cx| {
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

/// Whether a list scrolled to `offset` (zero or less, as GPUI has it) of
/// `max` is read to its end (`AT_END_SLACK`).
fn at_end(offset: f32, max: f32) -> bool {
    max + offset < AT_END_SLACK
}

/// The offset that keeps the text in view where it was once the messages
/// above the one laid out at `kept` (from the first, at `first`) are let go
/// of; with no layout to go by, or all of what was there let go of, the top.
fn kept_in_place(offset: f32, first: Option<f32>, kept: Option<f32>) -> f32 {
    match (first, kept) {
        (Some(first), Some(kept)) => (offset + (kept - first)).min(0.),
        _ => 0.,
    }
}

/// The flags a helper ran with, over what its account would open on.
pub fn promoted_settings(provider: Provider, flags: &serde_json::Map<String, Value>, defaults: Settings) -> Settings {
    let word = |k: &str| flags.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
    match (provider, defaults) {
        (Provider::Claude, Settings::Claude(d)) => Settings::Claude(ClaudeSettings {
            model: model_from_setting(&word("model")).unwrap_or(d.model),
            effort: effort_from_setting(&word("effort")).unwrap_or(d.effort),
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

/// The whole of opening one helper as a chat. The helper is not taken from
/// its agent for a row the list could not then record: refused while an
/// update installs, and one begun before the list froze is waited for by
/// the update's last write of the list, its row kept or let go by then
/// (`Workspace::admit`).
async fn promote(models: &Models, id: &str, cx: &mut gpui::AsyncApp) -> Result<(), String> {
    let ws = models.workspace.clone();
    cx.update(|cx| ws.update(cx, |w, _| w.admit()))?;
    let promoted = promote_admitted(models, id, cx).await;
    cx.update(|cx| ws.update(cx, |w, cx| w.release(cx)));
    if promoted? {
        return Ok(());
    }
    // Its row listed, the core not yet told: tried on, the list's last write
    // no longer waiting for it.
    cx.background_executor().timer(SETTLE_RETRY[SETTLE_HELD]).await;
    let handed = settle(&models.host, &ws, id, &SETTLE_RETRY[SETTLE_HELD + 1..], None, cx).await;
    if handed.is_none() {
        log::warn!("helper {id}'s opening is not settled; its row stays listed, and the next core to come up is told");
    }
    cx.update(|cx| ws.update(cx, |w, cx| w.opening_settled(id, handed, cx)));
    Ok(())
}

/// True once the core has said how the opening came out, the row seated or
/// let go of by it; false while it has not.
async fn promote_admitted(models: &Models, id: &str, cx: &mut gpui::AsyncApp) -> Result<bool, String> {
    let host = &models.host;
    let until = cx.background_executor().now() + OPENING_HELD;
    // What comes before the word that it failed leaves that word its time.
    let made_by = until - SETTLE_CALL;
    // The clock each word is bounded by, apart from what the words borrow.
    let clock = cx.clone();
    // This opening's own id, in its begin and in the word that it failed: a
    // begin given up on that lands after that word holds nothing in the core
    // (background/rows.rs `background_promote_as`).
    static OPENINGS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let opening = crate::workspace::new_id("o", OPENINGS.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    let failed = |why: &str| json!({ "id": id, "error": why, "opening": opening });
    let begin = ask(host, cmd::BACKGROUND_PROMOTE, json!({ "id": id, "opening": opening }));
    let p: Promoted = match bounded_until(&clock, made_by, begin).await {
        Ok(p) => p,
        // Unanswered, the core may hold it for this opening by now: told it
        // failed, it lets go, and a core that never had the word ignores it.
        // A refusal is the core's own word: it holds nothing of this opening.
        Err(why) if why.starts_with(UNANSWERED) => {
            let told = bounded(cx, ask::<Value>(host, cmd::BACKGROUND_PROMOTED, failed(&why))).await;
            if let Err(e) = told {
                log::warn!("telling the core helper {id}'s opening failed: {e}");
            }
            return Err(why);
        }
        Err(why) => return Err(why),
    };
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
        let defaults =
            bounded_until(&clock, made_by, async { Ok::<_, String>(super::new_chat::defaults_of(host, &account, &p.folder).await) })
                .await?;
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
        bounded_until(&clock, made_by, create::create(models, make, cx)).await
    }
    .await;
    let ws = models.workspace.clone();
    // The row on disk before the core hears it is made: a window gone after
    // that word would otherwise leave the helper with neither its agent nor a
    // row (`Workspace::written`). One that could not be written is let go of,
    // and the helper stays its agent's.
    let made = match made {
        Ok(_) => {
            let wrote = cx.update(|cx| ws.update(cx, |w, cx| w.written(cx)));
            let wrote = bounded_until(&clock, made_by, async { wrote.await.unwrap_or_else(|_| Err("the window went away".into())) }).await;
            wrote.map_err(|why| {
                cx.update(|cx| ws.update(cx, |w, cx| w.unlist_promoted(id, cx)));
                format!("its row was not saved: {why}")
            })
        }
        Err(why) => Err(why),
    };
    if let Err(why) = &made
        && let Err(e) = bounded(cx, ask::<Value>(host, cmd::BACKGROUND_PROMOTED, failed(why))).await
    {
        log::warn!("telling the core helper {id}'s opening failed: {e}");
    }
    made?;
    let handed = settle(host, &ws, id, &SETTLE_RETRY[..SETTLE_HELD], Some(until), cx).await;
    cx.update(|cx| ws.update(cx, |w, cx| w.opening_settled(id, handed, cx)));
    Ok(handed.is_some())
}

/// Tells the core the row for helper `id` is made, trying again after each
/// of `waits`: whether the helper is the person's now, None when the core
/// never answered. Only while the row is listed: one gone meanwhile is never
/// said made, and the core is told it was not. With `until`, no word goes
/// that could not be answered by then: what is left is tried later.
pub(crate) async fn settle(
    host: &std::sync::Arc<dyn crate::host::Host>,
    ws: &gpui::Entity<crate::workspace::Workspace>,
    id: &str,
    waits: &[Duration],
    until: Option<Instant>,
    cx: &mut gpui::AsyncApp,
) -> Option<bool> {
    // A wait, then a word, both within `until`.
    let fits = |cx: &gpui::AsyncApp, wait: Duration| until.is_none_or(|u| cx.background_executor().now() + wait + SETTLE_CALL <= u);
    for i in 0.. {
        if cx.update(|cx| ws.read(cx).state.find(id).is_none()) {
            let why = "its row was taken off the list before it opened";
            if let Err(e) = bounded(cx, ask::<Value>(host, cmd::BACKGROUND_PROMOTED, json!({ "id": id, "error": why }))).await {
                log::warn!("telling the core helper {id}'s row is gone failed: {e}");
            }
            return Some(false);
        }
        if !fits(cx, Duration::ZERO) {
            return None;
        }
        match bounded(cx, ask::<bool>(host, cmd::BACKGROUND_PROMOTED, json!({ "id": id, "error": null }))).await {
            Ok(handed) => {
                if !handed {
                    log::warn!("helper {id} went back to its agent before its opening was settled; its row is let go");
                }
                return Some(handed);
            }
            Err(e) => match waits.get(i).filter(|wait| fits(cx, **wait)) {
                Some(wait) => cx.background_executor().timer(*wait).await,
                None => {
                    log::warn!("telling the core helper {id} is opened failed: {e}");
                    return None;
                }
            },
        }
    }
    None
}

/// What a word `bounded_until` gave up on says first.
const UNANSWERED: &str = "the core did not answer";

/// `call`'s answer, or a failure past `SETTLE_CALL`.
pub(crate) async fn bounded<T>(cx: &gpui::AsyncApp, call: impl Future<Output = Result<T, String>>) -> Result<T, String> {
    let until = cx.background_executor().now() + SETTLE_CALL;
    bounded_until(cx, until, call).await
}

/// `call`'s answer, or a failure past `SETTLE_CALL` or `until`, whichever
/// comes first; none waited for at all once `until` has passed.
pub(crate) async fn bounded_until<T>(
    cx: &gpui::AsyncApp,
    until: Instant,
    call: impl Future<Output = Result<T, String>>,
) -> Result<T, String> {
    let left = until.saturating_duration_since(cx.background_executor().now()).min(SETTLE_CALL);
    if left.is_zero() {
        return Err(format!("{UNANSWERED} in the time an update waits for an opening"));
    }
    let timeout = cx.background_executor().timer(left);
    match futures::future::select(Box::pin(call), timeout).await {
        futures::future::Either::Left((answer, _)) => answer,
        futures::future::Either::Right(_) => Err(format!("{UNANSWERED} within {:.1} s", left.as_secs_f32())),
    }
}

impl Render for HelpersView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.models.shell.read(cx).palette();
        let close = self.act(cx, |v, window, cx| {
            let _ = v.layer.update(cx, |l, cx| l.close(window, cx));
        });
        if let Some(w) = &self.watching {
            return self.watch_frame(&p, w, &self.names(cx), cx).into_any_element();
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
        let names = self.names(cx);
        let mut list = div().id("helpers-rows").flex().flex_col().gap(u(6.)).max_h(u(420.)).overflow_y_scroll();
        if !self.loaded {
            list = list.child(ui::note(&p, "Asking the core for the helpers..."));
        } else if shown.is_empty() {
            list = list.child(ui::note(&p, if tab == Tab::Working { "None at work." } else { "None idle." }));
        }
        for h in shown {
            list = list.child(self.row(&p, h, now, &names, cx));
        }
        let mut body = div().flex().flex_col().gap(u(6.)).p(u(8.)).child(tabs).child(list);
        if let Some(e) = &self.error {
            body = body.child(ui::alert(&p, Tone::Trouble, e.clone()));
        }
        let title = match &self.chat {
            Some(_) => format!("Background helpers of this chat ({})", helpers.len()),
            None => format!("Background helpers ({})", helpers.len()),
        };
        let mut frame = ui::frame(&p, "helpers", title, u(440.), close, body).relative();
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
    /// The names its lines give the chats and the helpers they speak of.
    fn names(&self, cx: &Context<Self>) -> Names {
        let ws = self.models.workspace.read(cx);
        Names::new(ws.state.chats().map(|c| (c.id.as_str(), c.name.as_str())), &self.all)
    }

    fn row(&self, p: &Palette, h: &Helper, now: i64, names: &Names, cx: &Context<Self>) -> gpui::Stateful<Div> {
        let id = h.id.clone();
        let busy = h.busy();
        let left = h.state == "background" && !h.left_running.is_empty();
        let opening = self.models.workspace.read(cx).is_opening(&h.id);
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
            .selector({
                let id = id.clone();
                move || format!("helper-{id}")
            })
            .flex()
            .flex_col()
            .gap(u(2.))
            .p(u(4.))
            .border_1()
            .border_color(if h.asks() { p.attention } else { p.frame })
            .child(
                div()
                    .flex()
                    .gap(u(6.))
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
        // Whose it is: wrapped rather than cut, as nothing here shows a cut
        // line whole.
        if let Some(marks) = h.ownership_line(names) {
            let marks_id = format!("helper-marks-{id}");
            row = row.child(
                div().id(SharedString::from(marks_id.clone())).selector(move || marks_id.clone()).text_color(p.text_dim).child(marks),
            );
        }
        if let Some(ask) = h.ask_line() {
            row = row.child(div().truncate().bg(p.attention).text_color(p.on_status).px(u(4.)).child(ask));
        }
        row.child(
            div()
                .flex()
                .gap(u(4.))
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

    fn watch_frame(&self, p: &Palette, w: &Watch, names: &Names, cx: &Context<Self>) -> gpui::Stateful<Div> {
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
            .gap(u(8.))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&w.scroll)
            .bg(p.well)
            .p(u(6.));
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
                    .selector(move || format!("helpers-msg-{i}"))
                    .flex()
                    .flex_col()
                    .child(div().font_weight(gpui::FontWeight::BOLD).text_color(p.text_dim).child(who.to_owned()))
                    .child(div().child(m.text.clone())),
            );
        }
        let marks = self.all.iter().find(|h| h.id == w.id).and_then(|h| h.ownership_line(names));
        let body = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap(u(6.))
            .p(u(8.))
            .child(div().flex().items_center().gap(u(6.)).child(ui::button("helpers-back", p, "Back to the list", true, back)).child(
                div().id("helpers-watch-status").selector(|| "helpers-watch-status".into()).flex_1().text_color(p.text_dim).child(head),
            ))
            .when_some(marks, |body, marks| {
                body.child(div().id("helpers-watch-marks").selector(|| "helpers-watch-marks".into()).text_color(p.text_dim).child(marks))
            })
            .child(messages);
        ui::frame(p, "helpers-watch", format!("{} - read-only", w.name), u(760.), close, body).h(u(560.))
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
    fn a_watch_keeps_the_text_in_place_as_its_oldest_messages_go() {
        // Scrolled to 300 down, the first two messages (80 px) go: 220 down.
        assert_eq!(kept_in_place(-300., Some(100.), Some(180.)), -220.);
        // Reading among those that went: the top.
        assert_eq!(kept_in_place(-50., Some(100.), Some(180.)), 0.);
        assert_eq!(kept_in_place(-300., Some(100.), None), 0.);
    }

    #[test]
    fn a_watch_follows_new_output_only_from_its_end() {
        // Nothing to scroll yet, or at the end: followed.
        assert!(at_end(0., 0.));
        assert!(at_end(-500., 500.));
        assert!(at_end(-490., 500.), "near enough");
        // Scrolled up to read: left there.
        assert!(!at_end(-200., 500.));
        assert!(!at_end(0., 500.));
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
        assert!(matches!(s, Settings::Claude(c) if c.perm == "Don't ask"));
        // Nothing named: what the account would open on.
        assert_eq!(promoted_settings(Provider::Claude, &flags(json!({})), defaults.clone()), defaults);
        let codex = Settings::Codex(choice::codex_stock());
        let s =
            promoted_settings(Provider::Codex, &flags(json!({ "model": "gpt-6", "approval": "on-request", "sandbox": "bogus" })), codex);
        assert!(
            matches!(s, Settings::Codex(c) if c.model.as_deref() == Some("gpt-6") && c.approval == "on-request" && c.sandbox == "read-only")
        );
        // One that ran in Codex's own keeps it: no flag at its next start either.
        let s = promoted_settings(
            Provider::Codex,
            &flags(json!({ "approval": "codex", "sandbox": "codex" })),
            Settings::Codex(choice::codex_stock()),
        );
        assert!(matches!(s, Settings::Codex(c) if c.approval == choice::CODEX_OWN && c.sandbox == choice::CODEX_OWN));
    }
}
