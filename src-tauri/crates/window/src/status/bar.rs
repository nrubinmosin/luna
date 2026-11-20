//! The status bar, the full width of the window at its foot: what the chats
//! are doing, every account at a glance with its usage meters, the update,
//! the CLIs' downloads, the power rule, and the way into Settings.
//!
//! Static: it redraws when what it shows changed, nothing else; the one
//! number that counts down (the power rule's) is the model's one-second
//! clock, which runs only while a countdown does. The accounts give up detail
//! a step at a time as the window narrows (usage.rs `Fit`); the other fields
//! never shrink. A panel (the power rule, the update's confirmation) opens
//! upward from the bar, over the board, and closes on Esc or a click away.

use super::power::{self, Kind};
use super::updates::{self, Tone, Update};
use super::usage::{self, CELL, Fit, METER_W, Usage};
use super::vitals::{self, Arm, Popover, Vitals};
use crate::models::Models;
use crate::theme::Palette;
use crate::widgets::{bevel, button, tag, toggle};
use gpui::{
    AnyElement, Context, Div, Entity, FontWeight, Hsla, InteractiveElement, IntoElement, MouseDownEvent, ParentElement, Pixels, Render,
    SharedString, StatefulInteractiveElement, Styled, Window, deferred, div, px,
};

/// Its height, fixed: the view is cached at this size (root.rs).
pub const HEIGHT: Pixels = px(24.);
/// The width of a panel opened from the bar.
const PANEL_W: f32 = 420.;

pub struct StatusBar {
    models: Models,
    vitals: Entity<Vitals>,
    /// Times rendered, for the tests that hold renders to what changed.
    pub(crate) renders: usize,
}

impl StatusBar {
    pub fn new(models: Models, cx: &mut Context<Self>) -> StatusBar {
        cx.observe(&models.shell, |_, _, cx| cx.notify()).detach();
        let vitals = models.status.clone();
        cx.observe(&vitals, |_, _, cx| cx.notify()).detach();
        StatusBar { models, vitals, renders: 0 }
    }

    /// Closes the panel: Esc, or a click away. True when one was open.
    pub fn close_panel(&mut self, cx: &mut Context<Self>) -> bool {
        self.vitals.update(cx, |v, cx| v.close_popover(cx))
    }
}

/// A field of the bar: sunken for a reading, raised for what a click does,
/// `fill` a status fill with its text where the field is loud.
fn field(p: &Palette, raised: bool, fill: Option<(Hsla, Hsla)>, content: impl IntoElement) -> Div {
    let (bg, fg) = fill.unwrap_or((p.surface, p.text_dim));
    let inner = div().flex().items_center().gap(px(5.)).px(px(6.)).h(px(18.)).bg(bg).text_color(fg).whitespace_nowrap().child(content);
    bevel(p, raised, if fill.is_some() { inner.font_weight(FontWeight::BOLD) } else { inner })
}

/// A small framed square: a status dot.
fn dot(p: &Palette, fill: Hsla) -> Div {
    div().flex_none().w(px(8.)).h(px(8.)).bg(fill).border_1().border_color(p.frame)
}

/// How full a window is, as a fill: green, then yellow, then red.
fn limit_fill(p: &Palette, used: f64) -> Hsla {
    if used >= 0.85 {
        p.error
    } else if used >= 0.6 {
        p.attention
    } else {
        p.ok
    }
}

/// A window's meter: a sunken track, filled in proportion.
fn meter(p: &Palette, used: f64) -> Div {
    let fill = (METER_W - 2.) * used.clamp(0., 1.) as f32;
    div()
        .flex_none()
        .w(px(METER_W))
        .h(px(8.))
        .bg(p.well)
        .border_1()
        .border_color(p.frame)
        .child(div().h_full().w(px(fill)).bg(limit_fill(p, used)))
}

fn provider_tag(provider: crate::term::paste::Provider) -> &'static str {
    match provider {
        crate::term::paste::Provider::Claude => "CC",
        crate::term::paste::Provider::Codex => "CX",
    }
}

/// One account as the bar shows it: a dot, its provider, its name, and its
/// windows as far as `fit` leaves room for.
fn glance(p: &Palette, a: &Usage, fit: Fit, now_ms: u64) -> Div {
    let loading = a.sync == usage::Standing::Loading && !a.have_usage;
    let dot_fill = if loading {
        p.well
    } else if !a.signed_in || a.worst() >= 0.85 {
        p.error
    } else if a.have_usage {
        p.ok
    } else {
        p.well
    };
    let row = div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(4.))
        .child(dot(p, dot_fill))
        .child(tag(p, None, provider_tag(a.provider)))
        .child(div().font_weight(FontWeight::BOLD).text_color(p.text).child(SharedString::from(a.name.clone())));
    // Signed-in is only known once the first answer is in: until then the
    // account is loading, not signed out.
    if loading {
        return row.child(div().text_color(p.text_dim).child("…"));
    }
    if !a.signed_in {
        return row.child(div().text_color(p.text).font_weight(FontWeight::BOLD).child("signed out"));
    }
    let windows = usage::shown(a, fit);
    if windows.is_empty() {
        return row.child(div().text_color(p.text_dim).child("—"));
    }
    let mut row = row;
    for w in windows {
        let mut one = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(4.))
            .pl(px(4.))
            .child(div().text_color(p.text_dim).child(SharedString::from(w.short.clone())));
        if fit < Fit::NoMeter {
            one = one.child(meter(p, w.used));
        }
        let pct = div().text_color(p.text).child(usage::percent(w));
        one = one.child(if w.used >= 0.85 { pct.font_weight(FontWeight::BOLD) } else { pct });
        if fit == Fit::Full && w.reset_at.is_some() {
            one = one.child(div().text_color(p.text_dim).child(usage::fmt_reset(w.reset_at.as_deref(), now_ms)));
        }
        row = row.child(one);
    }
    row
}

/// About how wide a chip with `text` is, in pixels.
fn chip_w(text: &str) -> f32 {
    text.chars().count() as f32 * CELL + 16.
}

impl Render for StatusBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let p = self.models.shell.read(cx).palette();
        let v = self.vitals.read(cx);
        let counts = v.counts();
        let now = v.now_ms();
        let seams = self.models.workspace.read(cx).seams.clone();
        let vitals = self.vitals.clone();
        let ws = self.models.workspace.clone();

        // The three counts: filled while there is something, a plain reading
        // while there is not.
        let loud = |n: usize, fill: Hsla| (n > 0).then_some((fill, p.on_status));
        let working = field(&p, false, loud(counts.working, p.ok), format!("{} working", counts.working));
        let waiting = {
            let ws = ws.clone();
            div()
                .id("status-waiting")
                .debug_selector(|| "status-waiting".into())
                .flex_none()
                .on_click(move |_, _, cx| ws.update(cx, |w, cx| w.next_waiting(cx)))
                .child(field(&p, true, loud(counts.waiting, p.attention), format!("{} waiting", counts.waiting)))
        };
        let background = {
            let seam = seams.helpers.clone();
            div()
                .id("status-background")
                .debug_selector(|| "status-background".into())
                .flex_none()
                .on_click(move |_, window, cx| match &seam {
                    Some(seam) => seam(window, cx),
                    None => log::warn!("background helpers: their popover is not in this build yet"),
                })
                .child(field(&p, true, loud(counts.background, p.info), format!("{} background", counts.background)))
        };

        // The update, the CLIs' downloads, the power rule, in the order of
        // how rarely they speak.
        let mut chips: Vec<AnyElement> = Vec::new();
        let mut reserve = 0.;
        if let Some((tone, text)) = updates::chip(v.phase()) {
            let fill = match tone {
                Tone::Loud => (p.attention, p.on_status),
                Tone::Busy => (p.info, p.on_status),
                Tone::Bad => (p.error, p.on_status),
            };
            let clickable = matches!(v.phase(), Update::Available(_) | Update::Unsaved { .. } | Update::Failed { .. });
            reserve += chip_w(&text);
            let vitals = vitals.clone();
            chips.push(
                div()
                    .id("status-update")
                    .debug_selector(|| "status-update".into())
                    .flex_none()
                    .when_clickable(clickable, move |d| d.on_click(move |_, _, cx| vitals.update(cx, |v, cx| v.open_update(cx))))
                    .child(field(&p, clickable, Some(fill), text))
                    .into_any_element(),
            );
        }
        for cli in v.clis() {
            let Some(provider) = cli.provider else { continue };
            let key = vitals::provider_key(provider);
            match cli.phase.as_str() {
                "downloading" => {
                    let pct = updates::percent(cli.got, cli.total).map_or("…".to_owned(), |n| format!("{n}%"));
                    // The first download is the one that matters: without it a
                    // fresh install has nothing to run, so it alone is loud.
                    let fill = cli.version.is_none().then_some((p.info, p.on_status));
                    let text = format!("{} download {pct}", provider_tag(provider));
                    reserve += chip_w(&text);
                    chips.push(
                        div()
                            .id(SharedString::from(format!("status-cli-{key}")))
                            .debug_selector(move || format!("status-cli-{key}"))
                            .flex_none()
                            .child(field(&p, false, fill, text))
                            .into_any_element(),
                    );
                }
                "error" => {
                    let text = format!("{} update failed", provider_tag(provider));
                    reserve += chip_w(&text);
                    let vitals = vitals.clone();
                    chips.push(
                        div()
                            .id(SharedString::from(format!("status-cli-{key}")))
                            .debug_selector(move || format!("status-cli-{key}"))
                            .flex_none()
                            .on_click(move |_, _, cx| vitals.update(cx, |v, cx| v.retry_cli(provider, cx)))
                            .child(field(&p, true, Some((p.error, p.on_status)), text))
                            .into_any_element(),
                    );
                }
                _ => {}
            }
        }

        let chip = v.power_chip();
        let power_fill = match chip.kind {
            Kind::Countdown | Kind::Armed => Some((p.attention, p.on_status)),
            Kind::Awake => Some((p.ok, p.on_status)),
            Kind::Idle | Kind::Unknown => None,
        };
        reserve += chip_w(&chip.text);
        let mut power_chip = div().flex().flex_none().items_center().gap(px(3.)).child({
            let vitals = vitals.clone();
            div()
                .id("status-power")
                .debug_selector(|| "status-power".into())
                .flex_none()
                .on_click(move |_, _, cx| vitals.update(cx, |v, cx| v.open_popover(Popover::Power, cx)))
                .child(field(&p, true, power_fill, chip.text.clone()))
        });
        // Cancel while it counts down; Retry, armed or counting, after a
        // Cancel that did not reach the core.
        let failed = v.cancel_failed().is_some() && matches!(chip.kind, Kind::Countdown | Kind::Armed);
        if chip.kind == Kind::Countdown || failed {
            reserve += 60.;
            let vitals = vitals.clone();
            let (id, label) = if failed { ("status-power-retry", "Retry") } else { ("status-power-cancel", "Cancel") };
            power_chip = power_chip.child(
                div()
                    .id(id)
                    .debug_selector(move || id.into())
                    .flex_none()
                    .on_click(move |_, _, cx| vitals.update(cx, |v, cx| v.disarm_power_off(cx)))
                    .child(button(&p, label)),
            );
        }
        chips.push(power_chip.into_any_element());

        let settings = {
            let seam = seams.settings.clone();
            div()
                .id("status-settings")
                .debug_selector(|| "status-settings".into())
                .flex_none()
                .on_click(move |_, window, cx| match &seam {
                    Some(seam) => seam(window, cx),
                    None => log::warn!("settings: the panels are not in this build yet"),
                })
                .child(button(&p, "Settings"))
        };

        // The accounts take what the rest leaves.
        let fixed = [counts.working, counts.waiting, counts.background]
            .iter()
            .zip(["working", "waiting", "background"])
            .map(|(n, word)| chip_w(&format!("{n} {word}")))
            .sum::<f32>()
            + reserve
            + chip_w("Settings")
            + 4. * (chips.len() as f32 + 6.);
        let width: f32 = f32::from(window.viewport_size().width);
        // The accounts' own field has its bevel and padding, and the estimate errs short.
        let room = (width - 4. - fixed - 28.).max(0.);
        let accounts_el = if !v.accounts_loaded() {
            div()
        } else if v.accounts().is_empty() {
            div().text_color(p.text_dim).child("No accounts yet")
        } else {
            let fit = usage::fit_in(v.accounts(), room, now);
            div().flex().items_center().gap(px(12.)).overflow_hidden().children(v.accounts().iter().map(|a| glance(&p, a, fit, now)))
        };
        let accounts = div()
            .id("status-accounts")
            .debug_selector(|| "status-accounts".into())
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .child(field(&p, false, None, accounts_el).w_full());

        let panel = match v.popover() {
            Some(Popover::Power) => Some(self.power_panel(&p, v, cx)),
            Some(Popover::Update) => Some(self.update_panel(&p, v, cx)),
            None => None,
        };

        div()
            .id("status-bar")
            .size_full()
            .flex()
            .items_center()
            .gap(px(4.))
            .px(px(2.))
            .bg(p.surface)
            .text_color(p.text)
            .border_t_1()
            .border_color(p.frame)
            .child(working)
            .child(waiting)
            .child(background)
            .child(accounts)
            .children(chips)
            .child(settings)
            .children(panel.map(|panel| deferred(panel).with_priority(1)))
    }
}

/// A panel's frame: bevelled, lit edge so a black frame does not vanish on a
/// dark board, over what is under it.
fn panel_frame(
    p: &Palette,
    id: &'static str,
    title: &str,
    close: impl Fn(&mut Window, &mut gpui::App) + 'static,
    body: Div,
) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .debug_selector(move || id.into())
        .absolute()
        .bottom(HEIGHT)
        .right(px(4.))
        .w(px(PANEL_W))
        .flex()
        .flex_col()
        .bg(p.surface)
        .text_color(p.text)
        .border_1()
        .border_color(p.hi)
        .occlude()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .px(px(6.))
                .h(px(22.))
                .bg(p.title_bg)
                .text_color(p.title_fg)
                .font_weight(FontWeight::BOLD)
                .child(div().flex_1().child(SharedString::from(title.to_owned())))
                .child(
                    div()
                        .id(SharedString::from(format!("{id}-close")))
                        .debug_selector(move || format!("{id}-close"))
                        .on_click(move |_, window, cx| close(window, cx))
                        .child(button(p, "X")),
                ),
        )
        .child(body.p(px(8.)).gap(px(6.)))
}

impl StatusBar {
    fn act(
        &self,
        f: impl Fn(&mut Vitals, &mut Context<Vitals>) + 'static,
    ) -> impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static {
        let vitals = self.vitals.clone();
        move |_, _, cx| vitals.update(cx, |v, cx| f(v, cx))
    }

    fn close_fn(&self) -> impl Fn(&mut Window, &mut gpui::App) + 'static {
        let vitals = self.vitals.clone();
        move |_, cx| {
            vitals.update(cx, |v, cx| {
                v.close_popover(cx);
            });
        }
    }

    /// A press outside the panel closes it, but not one on the bar itself:
    /// the chip that opened it toggles it, and another chip replaces it.
    fn away(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if e.position.y < window.viewport_size().height - HEIGHT - px(1.) {
            self.close_panel(cx);
        }
    }

    /// A line of lowercase explanation under a control.
    fn fine(p: &Palette, text: impl IntoElement) -> Div {
        div().text_color(p.text_dim).text_size(px(11.)).child(text)
    }

    fn power_panel(&self, p: &Palette, v: &Vitals, cx: &Context<Self>) -> AnyElement {
        let state = v.power();
        let name = |id: &str| v.chat_name(id, cx);
        let arm = v.arm();
        let armed = state.armed.is_some();
        let on_cx = |label: &'static str, id: &'static str, on: bool, f: Box<dyn Fn(&mut Vitals, &mut Context<Vitals>)>| {
            div().id(id).debug_selector(move || id.into()).flex_none().on_click(self.act(move |v, cx| f(v, cx))).child(toggle(p, on, label))
        };

        let mut body = div().flex().flex_col();
        // The keep-awake switch.
        let awake = state.keep_awake;
        body = body
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(on_cx(if awake { "On" } else { "Off" }, "power-keep-awake", awake, Box::new(move |v, cx| v.set_keep_awake(!awake, cx))))
                    .child("Keep the PC awake while sessions work"),
            )
            .child(Self::fine(
                p,
                "Held while any session is mid-turn, still has anything it started running, or is still printing; let go a minute after the last one goes quiet. A session waiting for your answer does not hold it.",
            ));
        if state.holding {
            let held = power::holders(state, name);
            body = body.child(Self::fine(
                p,
                if held.is_empty() {
                    "Keeping the PC awake now: a session that just went quiet.".to_owned()
                } else {
                    format!("Keeping the PC awake now:\n{}", held.join("\n"))
                },
            ));
        }

        // The rule.
        body = body.child(div().mt(px(6.)).font_weight(FontWeight::BOLD).child("When every session is done"));
        let mut actions = div().flex().gap(px(2.));
        for action in vitals::actions() {
            let a = Arm { action, ..arm };
            actions = actions.child(
                div()
                    .id(SharedString::from(format!("power-action-{}", action.name())))
                    .debug_selector(move || format!("power-action-{}", action.name()))
                    .flex_none()
                    .when_clickable(!armed, |d| d.on_click(self.act(move |v, cx| v.set_arm(a, cx))))
                    .child(toggle(p, arm.action == action, action.label())),
            );
        }
        let mut quiet = div().flex().items_center().gap(px(2.)).child(div().pr(px(6.)).child("Once quiet for"));
        for (secs, label) in vitals::QUIET {
            let a = Arm { quiet_s: secs, ..arm };
            quiet = quiet.child(
                div()
                    .id(SharedString::from(format!("power-quiet-{secs}")))
                    .debug_selector(move || format!("power-quiet-{secs}"))
                    .flex_none()
                    .when_clickable(!armed, |d| d.on_click(self.act(move |v, cx| v.set_arm(a, cx))))
                    .child(toggle(p, arm.quiet_s == secs, label)),
            );
        }
        let drain = Arm { drain: !arm.drain, ..arm };
        body = body.child(actions).child(Self::fine(p, arm.action.note())).child(quiet).child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .id("power-drain")
                        .debug_selector(|| "power-drain".into())
                        .flex_none()
                        .when_clickable(!armed, |d| d.on_click(self.act(move |v, cx| v.set_arm(drain, cx))))
                        .child(toggle(p, arm.drain, if arm.drain { "Drain: on" } else { "Drain: off" })),
                )
                .child("Agents start nothing new until it fires"),
        );
        if armed {
            let blockers = power::blockers(state, name);
            let stalled = power::stalled(state, name);
            body = body.child(Self::fine(
                p,
                if blockers.is_empty() {
                    format!("All quiet: it fires once it stays that way for {} min.", arm.quiet_s / 60)
                } else {
                    format!("Waiting on:\n{}", blockers.join("\n"))
                },
            ));
            if !stalled.is_empty() {
                body = body.child(Self::fine(p, format!("Counted as done:\n{}", stalled.join("\n"))));
            }
            body = body.child(
                div()
                    .id("power-disarm")
                    .debug_selector(|| "power-disarm".into())
                    .flex_none()
                    .on_click(self.act(|v, cx| v.disarm_power_off(cx)))
                    .child(button(p, "Disarm")),
            );
        } else {
            let label = format!("Arm: {} when done", arm.action.name());
            body = body.child(
                div()
                    .id("power-arm")
                    .debug_selector(|| "power-arm".into())
                    .flex_none()
                    .on_click(self.act(|v, cx| v.arm_power_off(cx)))
                    .child(button(p, label)),
            );
        }
        panel_frame(p, "power-panel", "Power", self.close_fn(), body).on_mouse_down_out(cx.listener(Self::away)).into_any_element()
    }

    fn update_panel(&self, p: &Palette, v: &Vitals, cx: &Context<Self>) -> AnyElement {
        let live = v.power().summary.sessions.len();
        let mut body = div().flex().flex_col();
        let buttons = |items: Vec<(&'static str, &'static str, Box<dyn Fn(&mut Vitals, &mut Context<Vitals>)>)>| {
            div().flex().gap(px(6.)).children(items.into_iter().map(|(id, label, f)| {
                div().id(id).debug_selector(move || id.into()).flex_none().on_click(self.act(move |v, cx| f(v, cx))).child(button(p, label))
            }))
        };
        let close: Box<dyn Fn(&mut Vitals, &mut Context<Vitals>)> = Box::new(|v, cx| {
            v.close_popover(cx);
        });
        let title;
        match v.phase() {
            Update::Available(offer) | Update::Failed { offer: Some(offer), .. } => {
                title = format!("Update to {}?", offer.version);
                let keep = updates::keeps_sessions(offer, self.core_fingerprint(v, cx).as_deref());
                if let Update::Failed { why, .. } = v.phase() {
                    body = body.child(div().font_weight(FontWeight::BOLD).child(format!("The last try failed: {why}")));
                }
                body = body
                    .child("The installer is downloaded and checked against this build's signing key, and then Luna closes itself: an installer cannot replace the exe of a running app. It starts Luna again when it is done.")
                    .child(Self::fine(p, updates::sessions_line(live, keep)));
                if let Some(notes) = offer.notes.as_ref().filter(|n| !n.trim().is_empty()) {
                    body = body.child(
                        div()
                            .id("update-notes")
                            .debug_selector(|| "update-notes".into())
                            .max_h(px(140.))
                            .overflow_y_scroll()
                            .p(px(4.))
                            .bg(p.well)
                            .text_color(p.text)
                            .child(SharedString::from(notes.trim().to_owned())),
                    );
                }
                body = body.child(buttons(vec![
                    ("update-install", "Download and install", Box::new(|v, cx| v.install(cx))),
                    ("update-later", "Later", close),
                ]));
            }
            Update::Unsaved { offer, why, keep } => {
                title = format!("Install {}?", offer.version);
                body = body
                    .child(div().font_weight(FontWeight::BOLD).child("Luna could not write what it holds before installing."))
                    .child(Self::fine(p, why.clone()))
                    .child(Self::fine(
                        p,
                        if *keep {
                            "Anything not written is lost when the installer closes Luna: the chat list, the theme, the window's place."
                        } else {
                            "The sessions are stopped. Anything not written is lost when the installer closes Luna; Cancel resumes the chats."
                        },
                    ))
                    .child(buttons(vec![
                        ("update-retry", "Retry", Box::new(|v, cx| v.retry_install(cx))),
                        ("update-anyway", "Install anyway", Box::new(|v, cx| v.install_anyway(cx))),
                        ("update-cancel", "Cancel", Box::new(|v, cx| v.cancel_install(cx))),
                    ]));
            }
            _ => {
                title = "Update".to_owned();
                body = body.child("Nothing to install.").child(buttons(vec![("update-later", "Close", close)]));
            }
        }
        panel_frame(p, "update-panel", &title, self.close_fn(), body).on_mouse_down_out(cx.listener(Self::away)).into_any_element()
    }

    fn core_fingerprint(&self, _v: &Vitals, cx: &Context<Self>) -> Option<String> {
        let link = self.models.core.read(cx);
        let link = link.link();
        (link.state == crate::dto::LinkState::Up).then(|| link.core.as_ref().map(|c| c.fingerprint.clone())).flatten()
    }
}

/// `when`, for a click handler that is there only sometimes.
trait WhenClickable: Sized {
    fn when_clickable(self, on: bool, f: impl FnOnce(Self) -> Self) -> Self {
        if on { f(self) } else { self }
    }
}

impl<T: Sized> WhenClickable for T {}
