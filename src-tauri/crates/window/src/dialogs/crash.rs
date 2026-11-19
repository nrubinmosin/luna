//! What the window says when the run before it did not end by itself: its
//! reason and where the log is, and a Restart window button. A render that
//! panics cannot be caught (gpui does not), so the way to know is the next
//! start: the panic hook and the exit watcher write what they saw into the
//! run's record (lifecycle.rs), and the host hands the window what the record
//! of the run before says (`crash_report`).

use super::{DialogLayer, cmd};
use crate::models::Models;
use crate::panels::ask;
use crate::panels::ui::{self, Tone};
use gpui::{Context, InteractiveElement, IntoElement, ParentElement, Render, Styled, WeakEntity, Window, div, px};
use serde::Deserialize;
use serde_json::{Value, json};
use std::rc::Rc;

/// What the host knows of the run before.
#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Report {
    /// Why it ended, as far as anything saw: a panic's text and place, the
    /// exit code the watcher read, or that nothing did.
    pub reason: String,
    /// When it was last alive, on the wall clock.
    pub at: String,
    /// Where the log is.
    pub log: String,
}

pub struct CrashView {
    models: Models,
    layer: WeakEntity<DialogLayer>,
    pub report: Report,
    pub error: Option<String>,
    restarting: bool,
}

impl CrashView {
    pub fn new(models: Models, layer: WeakEntity<DialogLayer>, report: Report) -> CrashView {
        CrashView { models, layer, report, error: None, restarting: false }
    }

    /// Starts a new window of this kind and leaves the core, and so the
    /// sessions, alone (the tray's Restart window).
    pub fn restart(&mut self, cx: &mut Context<Self>) {
        if self.restarting {
            return;
        }
        self.restarting = true;
        self.error = None;
        let call = ask::<Value>(&self.models.host, cmd::RESTART_WINDOW, json!({}));
        cx.spawn(async move |this, cx| {
            let done = call.await;
            let _ = this.update(cx, |v, cx| {
                v.restarting = false;
                if let Err(e) = done {
                    v.error = Some(e);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

impl Render for CrashView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.models.shell.read(cx).palette();
        let this = cx.weak_entity();
        let close: ui::OnClick = {
            let layer = self.layer.clone();
            Rc::new(move |window, cx| {
                let _ = layer.update(cx, |l, cx| l.close(window, cx));
            })
        };
        let restart: ui::OnClick = Rc::new(move |_, cx| {
            let _ = this.update(cx, |v, cx| v.restart(cx));
        });
        let r = &self.report;
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(10.))
            .child(div().child("The last time Luna ran, it ended without closing itself. Your chats and their sessions are as they were: the core runs on its own."))
            .child(ui::alert(&p, Tone::Attention, if r.reason.is_empty() { "Nothing saw how it ended.".to_owned() } else { r.reason.clone() }).id("crash-reason").debug_selector(|| "crash-reason".into()));
        if !r.at.is_empty() {
            body = body.child(ui::note(&p, format!("Last alive {}.", r.at)));
        }
        body = body.child(
            div()
                .flex()
                .flex_col()
                .child(ui::note(&p, "The log, with what led up to it:"))
                .child(ui::path_text(&p, r.log.clone()).id("crash-log").debug_selector(|| "crash-log".into())),
        );
        if let Some(e) = &self.error {
            body = body.child(ui::alert(&p, Tone::Trouble, e.clone()));
        }
        let foot = div().flex().justify_end().gap(px(6.)).child(ui::button("crash-continue", &p, "Continue", true, close.clone())).child(
            ui::primary("crash-restart", &p, if self.restarting { "Restarting..." } else { "Restart window" }, !self.restarting, restart),
        );
        ui::frame(&p, "crash", "Luna stopped unexpectedly", px(520.), close, body.child(foot))
    }
}
