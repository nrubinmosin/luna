//! The settings window's view, and what each of its rows asks of the host.

use super::{AccountsRoot, CONTEXT_PCTS, CliState, CliStatus, CliVersions, HelperSettings, Page, RolledBack};
use crate::desktop::{Pick, PickKind};
use crate::dto::LinkState;
use crate::models::Models;
use crate::panels::ui::{self, Confirm, OnClick, Tone};
use crate::panels::{NewChatStart, PanelLayer, Prefs, Provider, ask, cmd};
use crate::status::Update;
use crate::theme::Palette;
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, Context, Div, FontWeight, InteractiveElement, IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement,
    Styled, Subscription, WeakEntity, Window, div, px,
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::rc::Rc;

/// A question the window waits on.
#[derive(Clone, Debug, PartialEq)]
enum Ask {
    /// A CLI's rollback, from the version the person saw current to the one
    /// the record keeps: made only while `from` still is the current one.
    RollBack { provider: Provider, from: String, to: String },
}

pub struct SettingsPanel {
    models: Models,
    layer: WeakEntity<PanelLayer>,
    pub page: Page,
    /// Luna's version, as the host says it.
    pub version: Option<String>,
    pub prefs: Option<Prefs>,
    pub root: Option<AccountsRoot>,
    pub helpers: Option<HelperSettings>,
    pub trusted: Option<Vec<String>>,
    pub clis: [CliState; 2],
    /// Why the last change of a setting failed, by the page it is on.
    pub error: Option<(Page, String)>,
    ask: Option<Ask>,
    _subs: Vec<Subscription>,
}

fn slot(p: Provider) -> usize {
    match p {
        Provider::Claude => 0,
        Provider::Codex => 1,
    }
}

impl SettingsPanel {
    pub fn new(models: Models, layer: WeakEntity<PanelLayer>, _window: &mut Window, cx: &mut Context<Self>) -> SettingsPanel {
        let subs = vec![
            cx.observe(&models.shell, |_, _, cx| cx.notify()),
            cx.observe(&models.core, |_, _, cx| cx.notify()),
            // Luna's update is the status bar's (`Vitals`): the page shows it
            // as it stands there.
            cx.observe(&models.status, |_, _, cx| cx.notify()),
        ];
        let mut panel = SettingsPanel {
            models,
            layer,
            page: Page::General,
            version: None,
            prefs: None,
            root: None,
            helpers: None,
            trusted: None,
            clis: Default::default(),
            error: None,
            ask: None,
            _subs: subs,
        };
        panel.load(cx);
        panel
    }

    fn act(&self, cx: &Context<Self>, f: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static) -> OnClick {
        let this = cx.weak_entity();
        Rc::new(move |window, cx| {
            let _ = this.update(cx, |v, cx| f(v, window, cx));
        })
    }

    /// Asks `command` and hands its answer to `then`; a failure is said on
    /// `page`.
    fn call<T: DeserializeOwned + 'static>(
        &self,
        page: Page,
        command: &str,
        args: Value,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
    ) {
        let answer = ask::<T>(&self.models.host, command, args);
        cx.spawn(async move |this, cx| {
            let got = answer.await;
            let _ = this.update(cx, |v, cx| {
                match got {
                    Ok(t) => {
                        if v.error.as_ref().is_some_and(|(p, _)| *p == page) {
                            v.error = None;
                        }
                        then(v, t, cx)
                    }
                    Err(e) => v.error = Some((page, e)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Every setting, as the window opens.
    fn load(&mut self, cx: &mut Context<Self>) {
        self.call(Page::General, cmd::WINDOW_PREFS, json!({}), cx, |v, p: Prefs, _| v.prefs = Some(p));
        self.call(Page::Updates, cmd::APP_VERSION, json!({}), cx, |v, version: String, _| v.version = Some(version));
        self.call(Page::General, cmd::GET_ACCOUNTS_ROOT, json!({}), cx, |v, r: AccountsRoot, _| v.root = Some(r));
        self.call(Page::Agents, cmd::HELPER_SETTINGS, json!({}), cx, |v, h: HelperSettings, _| v.helpers = Some(h));
        self.call(Page::Agents, cmd::CODEX_TRUSTED_ROOTS, json!({}), cx, |v, r: Vec<String>, _| v.trusted = Some(r));
        for p in Provider::ALL {
            self.refresh_cli(p, cx);
        }
    }

    fn refresh_cli(&mut self, p: Provider, cx: &mut Context<Self>) {
        self.call(Page::Clis, cmd::CLI_STATUS, json!({ "provider": p }), cx, move |v, s: Option<CliStatus>, _| v.clis[slot(p)].status = s);
        self.call(Page::Clis, cmd::CLI_VERSIONS, json!({ "provider": p }), cx, move |v, s: Option<CliVersions>, _| {
            v.clis[slot(p)].versions = s
        });
    }

    /// A CLI's pass moved (`cli://status`): shown as it comes; its versions
    /// read again once it is done.
    pub fn cli_status(&mut self, payload: Value, cx: &mut Context<Self>) {
        let Ok(status) = serde_json::from_value::<CliStatus>(payload) else { return };
        let Some(p) = status.provider else { return };
        let was = self.clis[slot(p)].status.as_ref().is_some_and(CliStatus::passing);
        let done = was && !status.passing();
        self.clis[slot(p)].status = Some(status);
        if done {
            self.call(Page::Clis, cmd::CLI_VERSIONS, json!({ "provider": p }), cx, move |v, s: Option<CliVersions>, _| {
                v.clis[slot(p)].versions = s
            });
        }
        cx.notify();
    }

    pub fn may_close(&mut self, cx: &mut Context<Self>) -> bool {
        if self.ask.take().is_some() {
            cx.notify();
            return false;
        }
        true
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if let Some(layer) = self.layer.upgrade() {
            layer.update(cx, |l, cx| l.close(cx));
        }
    }

    pub fn show_page(&mut self, page: Page, cx: &mut Context<Self>) {
        self.page = page;
        cx.notify();
    }

    /// A preference changed: kept at once, sent to the host to keep.
    pub fn set_prefs(&mut self, change: impl FnOnce(&mut Prefs), cx: &mut Context<Self>) {
        let Some(prefs) = &mut self.prefs else { return };
        change(prefs);
        let prefs = prefs.clone();
        if let Err(e) = self.models.host.send(cmd::SET_WINDOW_PREFS, json!({ "prefs": prefs })) {
            self.error = Some((Page::General, e));
        }
        // The new-chat dialog starts from these (`Workspace::new_chat_start`).
        self.models.workspace.update(cx, |w, cx| w.take_prefs(prefs, cx));
        cx.notify();
    }

    /// The accounts' folder: a path, or back to the default (`""`).
    pub fn set_root(&mut self, path: String, cx: &mut Context<Self>) {
        self.call(Page::General, cmd::SET_ACCOUNTS_ROOT, json!({ "path": path }), cx, |v, r: AccountsRoot, cx| {
            v.root = Some(r);
            // The chats' accounts are the ones in the new folder.
            v.models.workspace.update(cx, |w, cx| w.reload_accounts(cx));
        });
    }

    /// Change...: the accounts' folder picked in the system's picker.
    pub fn change_root(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pick_folder("The accounts folder", window, cx, |v, path, cx| v.set_root(path, cx));
    }

    pub fn set_max_helpers(&mut self, n: u32, cx: &mut Context<Self>) {
        self.call(Page::Agents, cmd::SET_MAX_HELPERS, json!({ "n": n }), cx, |v, h: HelperSettings, _| v.helpers = Some(h));
    }

    pub fn set_context_pct(&mut self, pct: u32, cx: &mut Context<Self>) {
        self.call(Page::Agents, cmd::SET_HELPER_CONTEXT_PCT, json!({ "pct": pct }), cx, |v, h: HelperSettings, _| v.helpers = Some(h));
    }

    pub fn set_trusted(&mut self, roots: Vec<String>, cx: &mut Context<Self>) {
        self.call(Page::Agents, cmd::SET_CODEX_TRUSTED_ROOTS, json!({ "roots": roots }), cx, |v, r: Vec<String>, _| v.trusted = Some(r));
    }

    /// Check now: the CLI's release is looked at and, if newer, installed.
    /// What it does next comes as `cli://status`.
    pub fn check_cli(&mut self, p: Provider, cx: &mut Context<Self>) {
        self.clis[slot(p)].error = None;
        let answer = self.models.host.call(cmd::CLI_UPDATE_NOW, json!({ "provider": p }));
        cx.spawn(async move |this, cx| {
            let got = answer.await;
            let _ = this.update(cx, |v, cx| {
                match got {
                    Ok(_) => v.refresh_cli(p, cx),
                    Err(e) => v.clis[slot(p)].error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Roll back: asks first, naming the two versions, when the record keeps
    /// a way back that nothing refuses and a current version is shown.
    pub fn ask_roll_back(&mut self, p: Provider, cx: &mut Context<Self>) {
        let Some(versions) = self.clis[slot(p)].versions.as_ref().filter(|v| v.roll_back_offer().2) else { return };
        let (Some(from), Some(back)) = (versions.current(), versions.way_back.as_ref()) else { return };
        self.ask = Some(Ask::RollBack { provider: p, from: from.version.clone(), to: back.version.clone() });
        cx.notify();
    }

    /// The rollback itself, `from` being the version that was current when
    /// it was asked for. The core makes it only while that still is, and says
    /// so when it is not: the versions are read again then, and again after
    /// it went through.
    fn roll_back(&mut self, p: Provider, from: String, cx: &mut Context<Self>) {
        self.clis[slot(p)].rolled = None;
        self.clis[slot(p)].rolling = true;
        let answer = ask::<RolledBack>(&self.models.host, cmd::CLI_ROLL_BACK, json!({ "provider": p, "from": from }));
        cx.spawn(async move |this, cx| {
            let got = answer.await;
            let _ = this.update(cx, |v, cx| {
                let moved = match &got {
                    Ok(_) => true,
                    Err(e) => e.contains("look again"),
                };
                v.clis[slot(p)].rolling = false;
                v.clis[slot(p)].rolled = Some(got);
                if moved {
                    v.refresh_cli(p, cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Check for Luna's own update: the status bar's check, so the chip and
    /// the page say the same.
    pub fn check_update(&mut self, cx: &mut Context<Self>) {
        self.models.status.update(cx, |v, cx| v.check(true, cx));
    }

    /// Update...: the status bar's confirmation, the one every install of
    /// Luna's goes through (`Vitals::open_update`), so the install freezes
    /// the chat list, stops the sessions where the core changes and saves
    /// before it starts, as from the chip. The panel goes so that the
    /// confirmation, and what the install says next, are not under it.
    pub fn open_update(&mut self, cx: &mut Context<Self>) {
        self.close(cx);
        self.models.status.update(cx, |v, cx| {
            if v.popover() != Some(crate::status::Popover::Update) {
                v.open_update(cx);
            }
        });
    }

    fn pick_folder(&mut self, title: &str, window: &mut Window, cx: &mut Context<Self>, then: fn(&mut Self, String, &mut Context<Self>)) {
        let pick = Pick { kind: PickKind::Folder, title: Some(title.to_owned()), start: None, filters: Vec::new() };
        let picked = self.models.host.pick(pick, Some(window));
        cx.spawn_in(window, async move |this, cx| match picked.await {
            Ok(Some(path)) => {
                let _ = this.update(cx, |v, cx| then(v, path.to_string_lossy().into_owned(), cx));
            }
            Ok(None) => {}
            Err(e) => log::warn!("folder picker: {e}"),
        })
        .detach();
    }

    fn yes(&mut self, cx: &mut Context<Self>) {
        match self.ask.take() {
            Some(Ask::RollBack { provider, from, .. }) => self.roll_back(provider, from, cx),
            None => {}
        }
        cx.notify();
    }
}

impl Render for SettingsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.models.shell.read(cx).palette();
        let close = self.act(cx, |v, _, cx| {
            if v.may_close(cx) {
                v.close(cx);
            }
        });
        let nav = div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(170.))
            .gap(px(2.))
            .children(Page::ALL.map(|page| {
                let chosen = page == self.page;
                let id = format!("settings-page-{}", page.key());
                let selector = id.clone();
                div()
                    .id(SharedString::from(id))
                    .debug_selector(move || selector.clone())
                    .px(px(6.))
                    .h(px(20.))
                    .flex()
                    .items_center()
                    .when(chosen, |d| d.bg(p.title_bg).text_color(p.title_fg).font_weight(FontWeight::BOLD))
                    .child(page.title())
                    .on_click({
                        let act = self.act(cx, move |v, _, cx| v.show_page(page, cx));
                        move |_, window, cx| act(window, cx)
                    })
            }))
            .child(div().flex_1())
            .child(ui::note(&p, format!("Luna {}", self.version.as_deref().unwrap_or("-"))));
        let page = match self.page {
            Page::General => self.general(&p, cx),
            Page::Chats => self.chats(&p, cx),
            Page::Agents => self.agents(&p, cx),
            Page::Clis => self.cli_page(&p, cx),
            Page::Updates => self.updates(&p, cx),
        };
        let error = self.error.as_ref().filter(|(page, _)| *page == self.page).map(|(_, e)| ui::alert(&p, Tone::Trouble, e.clone()));
        let body = div().flex().flex_1().min_h_0().gap(px(10.)).p(px(8.)).child(nav).child(
            div()
                .id("settings-scroll")
                .flex_1()
                .min_w_0()
                .overflow_y_scroll()
                .child(div().font_weight(FontWeight::BOLD).text_size(px(14.)).child(self.page.title()))
                .children(error)
                .child(page),
        );
        let frame = ui::frame(&p, "settings", "Settings", px(820.), close, body).h(px(600.)).relative();
        match self.ask.clone() {
            Some(ask) => frame.child(self.question(ask, &p, cx).render(&p, "settings-ask")),
            None => frame,
        }
    }
}

impl SettingsPanel {
    fn general(&self, p: &Palette, cx: &Context<Self>) -> Div {
        let sound = self.prefs.as_ref().map(|x| x.sound);
        let root = self.root.clone();
        div()
            .flex()
            .flex_col()
            .child(ui::section(p, "Notifications"))
            .child(ui::check(
                "settings-sound",
                p,
                "Play a sound: desktop notifications play the system's notification sound",
                sound.unwrap_or(true),
                sound.is_some(),
                self.act(cx, |v, _, cx| v.set_prefs(|x| x.sound = !x.sound, cx)),
            ))
            .child(ui::section(p, "Accounts"))
            .child(ui::row(
                p,
                "Accounts folder",
                Some("Claude Code accounts live under anthropic/, Codex accounts under openai/. Only the location changes: move the account folders yourself. Running sessions keep their paths.".into()),
                div()
                    .flex()
                    .gap(px(4.))
                    .child(ui::button(
                        "settings-root-change",
                        p,
                        "Change...",
                        root.is_some(),
                        self.act(cx, |v, window, cx| v.change_root(window, cx)),
                    ))
                    .child(ui::button(
                        "settings-root-default",
                        p,
                        "Default",
                        root.as_ref().is_some_and(|r| !r.is_default),
                        self.act(cx, |v, _, cx| v.set_root(String::new(), cx)),
                    )),
            ))
            .child(ui::path_text(p, root.map(|r| r.path).unwrap_or_else(|| "...".into())))
    }

    fn chats(&self, p: &Palette, cx: &Context<Self>) -> Div {
        let start = self.prefs.as_ref().map(|x| x.new_chat);
        let on = start.is_some();
        let NewChatStart { worktree, tools } = start.unwrap_or_default();
        div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(ui::section(p, "New chats start with"))
            .child(ui::check(
                "settings-new-worktree",
                p,
                "Git worktree: a new chat in a folder in git starts in a worktree of its own",
                worktree,
                on,
                self.act(cx, |v, _, cx| v.set_prefs(|x| x.new_chat.worktree = !x.new_chat.worktree, cx)),
            ))
            .child(ui::check(
                "settings-new-tools",
                p,
                "Luna tools: a new chat gets Luna's tools, such as starting helpers",
                tools,
                on,
                self.act(cx, |v, _, cx| v.set_prefs(|x| x.new_chat.tools = !x.new_chat.tools, cx)),
            ))
            .child(ui::note(
                p,
                "How the boxes start in the new-chat dialog. Flipping one there is for that chat alone; a folder that should start otherwise keeps its own.",
            ))
    }

    fn agents(&self, p: &Palette, cx: &Context<Self>) -> Div {
        let h = self.helpers.clone();
        let ready = h.is_some();
        let h = h.unwrap_or_default();
        let n = h.max_helpers;
        let (min, max) = (h.min, h.max);
        let stepper = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .child(ui::button(
                "settings-helpers-less",
                p,
                "-",
                ready && n > min,
                self.act(cx, move |v, _, cx| v.set_max_helpers(n - 1, cx)),
            ))
            .child(div().w(px(28.)).flex().justify_center().font_weight(FontWeight::BOLD).child(n.to_string()))
            .child(ui::button(
                "settings-helpers-more",
                p,
                "+",
                ready && n < max,
                self.act(cx, move |v, _, cx| v.set_max_helpers(n + 1, cx)),
            ));
        let pct = ui::segmented(
            "settings-helper-context",
            p,
            CONTEXT_PCTS
                .iter()
                .map(|&n| {
                    let label = if n == 0 { "Never".to_owned() } else { format!("{n}%") };
                    (SharedString::from(label), h.helper_context_pct == n, self.act(cx, move |v, _, cx| v.set_context_pct(n, cx)))
                })
                .collect(),
            ready,
        );
        let roots = self.trusted.clone();
        let mut list = div().flex().flex_col();
        match &roots {
            None => list = list.child(ui::note(p, "...")),
            Some(r) if r.is_empty() => list = list.child(ui::note(p, "No trusted folders: Codex asks in every project.")),
            Some(r) => {
                for (i, root) in r.iter().enumerate() {
                    let rest: Vec<String> = r.iter().filter(|x| *x != root).cloned().collect();
                    list = list.child(
                        div().flex().items_center().gap(px(6.)).py(px(1.)).child(ui::path_text(p, root.clone()).flex_1()).child(
                            ui::button(
                                &format!("settings-trusted-remove-{i}"),
                                p,
                                "Remove",
                                true,
                                self.act(cx, move |v, _, cx| v.set_trusted(rest.clone(), cx)),
                            ),
                        ),
                    );
                }
            }
        }
        div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(ui::section(p, "Helpers"))
            .child(ui::row(p, "Helpers per chat", Some("The most sessions at work under one of your chats, its helpers' helpers included.".into()), stepper))
            .child(ui::row(
                p,
                "Tell me when a helper's context passes",
                Some("Notices about a helper past this share of its context window say how full it is; past 85% one comes on its own.".into()),
                div(),
            ))
            .child(pct)
            .child(ui::section(p, "Trusted folders"))
            .child(ui::note(p, "A Codex session in a project under any of these folders starts trusted: its own .codex/ config, hooks and rules load, with nothing written to the account."))
            .child(list)
            .child(div().pt(px(4.)).flex().child(ui::button(
                "settings-trusted-add",
                p,
                "Add folder...",
                roots.is_some(),
                self.act(cx, |v, window, cx| {
                    v.pick_folder("A folder whose projects Codex trusts", window, cx, |v, path, cx| {
                        let mut roots = v.trusted.clone().unwrap_or_default();
                        if !roots.contains(&path) {
                            roots.push(path);
                            v.set_trusted(roots, cx);
                        }
                    })
                }),
            )))
    }

    fn cli_page(&self, p: &Palette, cx: &Context<Self>) -> Div {
        let mut page = div().flex().flex_col().gap(px(2.));
        for provider in Provider::ALL {
            page = page.child(self.cli_group(provider, p, cx));
        }
        page.child(ui::note(
            p,
            "Luna keeps its own copy of each CLI and installs each new release as it comes, checking every 6 hours. The first sessions \
             on a new version are watched: when they fail soon after starting, Luna goes back to the version before by itself. Running \
             sessions keep the version they started with.",
        ))
    }

    fn cli_group(&self, provider: Provider, p: &Palette, cx: &Context<Self>) -> Div {
        let state = &self.clis[slot(provider)];
        let status = state.status.clone();
        let versions = state.versions.clone().unwrap_or_default();
        let passing = status.as_ref().is_some_and(CliStatus::passing);
        let version = versions
            .current()
            .map(|r| r.version.clone())
            .or_else(|| status.as_ref().and_then(|s| s.version.clone()))
            .unwrap_or_else(|| "-".into());
        let key = provider.key();
        let line = match &status {
            Some(s) if s.phase == "error" => ui::alert(p, Tone::Trouble, s.line()),
            Some(s) if s.passing() => ui::alert(p, Tone::Working, s.line()),
            Some(s) => ui::note(p, s.line()),
            None => ui::note(p, "No status yet."),
        };
        let mut group = div()
            .flex()
            .flex_col()
            .gap(px(3.))
            .child(ui::section(p, provider.label()))
            .child(ui::row(
                p,
                format!("Version {version}"),
                None,
                ui::button(
                    &format!("settings-cli-check-{key}"),
                    p,
                    if passing { "Checking..." } else { "Check" },
                    !passing,
                    self.act(cx, move |v, _, cx| v.check_cli(provider, cx)),
                ),
            ))
            .child(line);
        group = group.child(self.roll_back_row(provider, &versions, p, cx));
        if let Some(c) = versions.canary.as_ref().filter(|c| !c.failures.is_empty()) {
            group = group.child(ui::alert(
                p,
                Tone::Attention,
                format!(
                    "{} of the {} sessions started on {} {} so far failed soon after starting.",
                    c.failures.len(),
                    c.watched,
                    provider.label(),
                    c.version
                ),
            ));
        }
        for row in versions.versions.iter().filter(|r| r.bad.as_deref().is_some_and(|b| b.starts_with("rolled back"))) {
            group = group.child(ui::note(p, format!("Rolled back from {}: {}", row.version, row.bad.clone().unwrap_or_default())));
        }
        let installed: Vec<String> = versions
            .versions
            .iter()
            .map(|r| {
                let tags: Vec<&str> = [
                    r.current.then_some("current"),
                    r.previous.then_some("previous"),
                    (r.in_use && !r.current).then_some("in use"),
                    r.bad.is_some().then_some("turned away"),
                    r.below_floor.then_some("too old to run"),
                ]
                .into_iter()
                .flatten()
                .collect();
                if tags.is_empty() { r.version.clone() } else { format!("{} ({})", r.version, tags.join(", ")) }
            })
            .collect();
        if !installed.is_empty() {
            group = group.child(ui::note(p, format!("Installed: {}", installed.join("; "))));
        }
        group.children(state.error.clone().map(|e| ui::alert(p, Tone::Trouble, e)))
    }

    /// The Roll back row: the version it goes to, or why it does not, and
    /// how the last one went.
    fn roll_back_row(&self, provider: Provider, versions: &CliVersions, p: &Palette, cx: &Context<Self>) -> Div {
        let state = &self.clis[slot(provider)];
        let key = provider.key();
        let (label, desc, open) = versions.roll_back_offer();
        let button = ui::button(
            &format!("settings-cli-rollback-{key}"),
            p,
            if state.rolling { "Rolling back...".to_owned() } else { label },
            open && !state.rolling,
            self.act(cx, move |v, _, cx| v.ask_roll_back(provider, cx)),
        );
        let done = state.rolled.as_ref().map(|r| match r {
            Ok(done) => ui::alert(p, Tone::Done, format!("Rolled back {}: {} \u{2192} {}.", provider.label(), done.from, done.to)),
            Err(e) => ui::alert(p, Tone::Trouble, e.clone()),
        });
        div().flex().flex_col().child(ui::row(p, "Roll back", Some(desc.into()), button)).children(done)
    }

    fn updates(&self, p: &Palette, cx: &Context<Self>) -> Div {
        let vitals = self.models.status.read(cx);
        let check =
            |on: bool| ui::button("settings-update-check", p, "Check", on, self.act(cx, |v, _, cx| v.check_update(cx))).into_any_element();
        let open = |label: &'static str| {
            ui::primary("settings-update-install", p, label, true, self.act(cx, |v, _, cx| v.open_update(cx))).into_any_element()
        };
        let (status, button): (AnyElement, AnyElement) = match vitals.phase() {
            _ if vitals.checking() => (ui::alert(p, Tone::Working, "Checking for an update...").into_any_element(), check(false)),
            Update::Idle => match vitals.current_at() {
                Some(at) => (
                    ui::note(p, format!("This is the newest version (checked {}).", crate::panels::local_time(at as i64)))
                        .into_any_element(),
                    check(true),
                ),
                None => (ui::note(p, "Not checked yet.").into_any_element(), check(true)),
            },
            Update::Available(offer) => {
                (ui::alert(p, Tone::Attention, format!("Version {} is out.", offer.version)).into_any_element(), open("Update..."))
            }
            Update::Downloading { offer, got, total } => {
                let pct = crate::status::percent(*got, *total).map(|p| format!(": {p}%")).unwrap_or_default();
                (ui::alert(p, Tone::Working, format!("Downloading {}{pct}", offer.version)).into_any_element(), check(false))
            }
            Update::Stopping(offer) => (
                ui::alert(p, Tone::Working, format!("Stopping the sessions before installing {}...", offer.version)).into_any_element(),
                check(false),
            ),
            Update::Installing(offer) => (
                ui::alert(p, Tone::Working, format!("Starting the installer of {}: Luna closes and comes back on it.", offer.version))
                    .into_any_element(),
                check(false),
            ),
            Update::Unsaved { offer, .. } => (
                ui::alert(p, Tone::Trouble, format!("{} waits: Luna could not write what it holds first.", offer.version))
                    .into_any_element(),
                open("Choose..."),
            ),
            Update::Failed { offer, why } => (
                ui::alert(p, Tone::Trouble, format!("The update failed: {why}")).into_any_element(),
                if offer.is_some() { open("Update...") } else { check(true) },
            ),
        };
        let core = self.models.core.read(cx).link().clone();
        let (core_version, core_status) = match (&core.state, &core.core) {
            (LinkState::Up, Some(c)) => (
                format!("{} - {}", c.version, &c.fingerprint[..c.fingerprint.len().min(8)]),
                format!("Running since {}, pid {}", crate::panels::local_time(c.started as i64), c.pid),
            ),
            (LinkState::Starting, _) => ("-".into(), "Starting...".into()),
            (_, c) => (
                c.as_ref().map(|c| c.version.clone()).unwrap_or_else(|| "-".into()),
                format!("Not running{}", core.error.as_ref().map(|e| format!(": {e}")).unwrap_or_default()),
            ),
        };
        let up = core.state == LinkState::Up;
        let starting = core.state == LinkState::Starting;
        // Not while an update holds the core (`Vitals::core_held`).
        let held = self.models.status.read(cx).core_held();
        div()
            .flex()
            .flex_col()
            .gap(px(3.))
            .child(ui::section(p, "Luna"))
            .child(ui::row(p, format!("Luna {}", self.version.as_deref().unwrap_or("-")), None, button))
            .child(status)
            .child(ui::section(p, "Core"))
            .child(ui::row(
                p,
                format!("Core {core_version}"),
                Some(core_status.into()),
                ui::button(
                    "settings-core-restart",
                    p,
                    if up { "Restart" } else { "Start" },
                    !starting && !held,
                    self.act(cx, |v, _, cx| v.models.restart_core(cx)),
                ),
            ))
            .child(ui::note(p, "Restarting the core ends the sessions in it; the chats resume in the new one from their transcripts."))
    }

    fn question(&self, ask: Ask, _p: &Palette, cx: &Context<Self>) -> Confirm {
        let (title, body, yes): (String, String, String) = match &ask {
            Ask::RollBack { provider, from, to } => (
                roll_back_title(*provider, from, to),
                format!(
                    "Luna goes back to {to}, the version it kept before {from}. {from} is not installed again while it is the newest \
                     release, only a different one. Sessions already running keep the version they started with."
                ),
                "Roll back".to_owned(),
            ),
        };
        Confirm {
            title: title.into(),
            body: body.into(),
            yes: yes.into(),
            on_yes: self.act(cx, |v, _, cx| v.yes(cx)),
            on_no: self.act(cx, |v, _, cx| {
                v.ask = None;
                cx.notify();
            }),
        }
    }

    /// For the tests: the question up, by what it asks.
    #[cfg(test)]
    pub fn asking(&self) -> Option<&'static str> {
        self.ask.as_ref().map(|a| match a {
            Ask::RollBack { .. } => "roll-back",
        })
    }

    /// The question answered yes, as its button does.
    #[cfg(test)]
    pub fn answer_yes(&mut self, cx: &mut Context<Self>) {
        self.yes(cx);
    }

    /// The question's Cancel.
    #[cfg(test)]
    pub fn answer_no(&mut self, cx: &mut Context<Self>) {
        self.ask = None;
        cx.notify();
    }
}

fn roll_back_title(provider: Provider, from: &str, to: &str) -> String {
    format!("Roll back {} from {from} to {to}?", provider.label())
}

/// The question's title where it is a rollback's, for the tests.
#[cfg(test)]
pub fn roll_back_asked(panel: &SettingsPanel) -> Option<String> {
    match &panel.ask {
        Some(Ask::RollBack { provider, from, to }) => Some(roll_back_title(*provider, from, to)),
        _ => None,
    }
}

/// The panel's text for a CLI, for the tests.
#[cfg(test)]
pub fn cli_line(panel: &SettingsPanel, p: Provider) -> Option<String> {
    panel.clis[slot(p)].status.as_ref().map(CliStatus::line)
}
