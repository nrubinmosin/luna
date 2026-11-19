//! The accounts window's view, and what each of its controls asks of the host.

use super::{Account, Limits, SyncReport, SyncState};
use crate::models::Models;
use crate::panels::ui::{self, Confirm, DropItem, Dropdown, OnClick, Tone};
use crate::panels::{Panel, PanelLayer, Provider, ask, cmd};
use crate::theme::Palette;
use gpui::{
    AnyElement, AppContext, Context, Div, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, WeakEntity, Window, div, px,
};
use gpui_component::input::{Input, InputEvent, InputState};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// An account's limits, as far as they came.
#[derive(Clone, Debug, PartialEq)]
pub enum LimitsState {
    Loading,
    Got(Limits),
    Failed(String),
}

pub struct AccountsPanel {
    models: Models,
    layer: WeakEntity<PanelLayer>,
    pub accounts: Option<Vec<Account>>,
    /// By account path.
    pub limits: HashMap<String, LimitsState>,
    /// Accounts whose Refresh is waiting on its answer, by path.
    refreshing: HashSet<String>,
    pub sync: Option<SyncState>,
    /// What the last Sync now did, by CLI.
    pub sync_note: HashMap<Provider, String>,
    syncing: Option<Provider>,
    pub adding: bool,
    pub add_provider: Provider,
    add_name: Entity<InputState>,
    /// The account being renamed, and its name field.
    renaming: Option<(Account, Entity<InputState>)>,
    menu: Option<String>,
    /// The account whose deletion waits on a yes.
    deleting: Option<Account>,
    pub error: Option<String>,
    _subs: Vec<Subscription>,
}

impl AccountsPanel {
    pub fn new(models: Models, layer: WeakEntity<PanelLayer>, window: &mut Window, cx: &mut Context<Self>) -> AccountsPanel {
        let add_name = cx.new(|cx| InputState::new(window, cx).placeholder("account name"));
        let subs = vec![
            cx.observe(&models.shell, |_, _, cx| cx.notify()),
            cx.subscribe_in(&add_name, window, |this, _, e: &InputEvent, window, cx| {
                if matches!(e, InputEvent::PressEnter { .. }) {
                    this.create(window, cx);
                }
            }),
        ];
        let mut panel = AccountsPanel {
            models,
            layer,
            accounts: None,
            limits: HashMap::new(),
            refreshing: HashSet::new(),
            sync: None,
            sync_note: HashMap::new(),
            syncing: None,
            adding: false,
            add_provider: Provider::Claude,
            add_name,
            renaming: None,
            menu: None,
            deleting: None,
            error: None,
            _subs: subs,
        };
        panel.load(cx);
        panel.load_sync(cx);
        panel
    }

    fn act(&self, cx: &Context<Self>, f: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static) -> OnClick {
        let this = cx.weak_entity();
        Rc::new(move |window, cx| {
            let _ = this.update(cx, |v, cx| f(v, window, cx));
        })
    }

    /// The accounts, and each one's limits.
    pub fn load(&mut self, cx: &mut Context<Self>) {
        let answer = ask::<Vec<Account>>(&self.models.host, cmd::LIST_ACCOUNTS, json!({}));
        cx.spawn(async move |this, cx| {
            let got = answer.await;
            let _ = this.update(cx, |v, cx| {
                match got {
                    Ok(accounts) => {
                        let fresh: Vec<Account> = accounts.iter().filter(|a| !v.limits.contains_key(&a.path)).cloned().collect();
                        v.limits.retain(|path, _| accounts.iter().any(|a| a.path == *path));
                        v.accounts = Some(accounts);
                        for a in fresh {
                            v.ask_limits(&a, false, cx);
                        }
                    }
                    Err(e) => v.error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn load_sync(&mut self, cx: &mut Context<Self>) {
        let answer = ask::<SyncState>(&self.models.host, cmd::SETTINGS_SYNC_STATE, json!({}));
        cx.spawn(async move |this, cx| {
            let got = answer.await;
            let _ = this.update(cx, |v, cx| {
                match got {
                    Ok(s) => v.sync = Some(s),
                    Err(e) => v.error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// An account's limits; `force`, Refresh: past the CLI's cache and a
    /// 429 cool-off, renewing an expired token on the way.
    pub fn ask_limits(&mut self, a: &Account, force: bool, cx: &mut Context<Self>) {
        let command = match a.provider {
            Provider::Claude => cmd::ACCOUNT_LIMITS,
            Provider::Codex => cmd::CODEX_LIMITS,
        };
        let answer = self.models.host.call(command, json!({ "accountPath": a.path, "force": force }));
        let (path, provider) = (a.path.clone(), a.provider);
        if force {
            self.refreshing.insert(path.clone());
        }
        self.limits.entry(path.clone()).or_insert(LimitsState::Loading);
        cx.spawn(async move |this, cx| {
            let got = answer.await.and_then(|v: Value| Limits::read(provider, v));
            let _ = this.update(cx, |v, cx| {
                v.refreshing.remove(&path);
                v.limits.insert(path, got.map_or_else(LimitsState::Failed, LimitsState::Got));
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub fn may_close(&mut self, cx: &mut Context<Self>) -> bool {
        cx.notify();
        if self.deleting.take().is_some() || self.menu.take().is_some() || self.renaming.take().is_some() {
            return false;
        }
        if self.adding {
            self.adding = false;
            return false;
        }
        true
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if let Some(layer) = self.layer.upgrade() {
            layer.update(cx, |l, cx| l.close(cx));
        }
    }

    /// Answers `then` once `command` is done; a failure is said at the top.
    fn run(
        &mut self,
        command: &str,
        args: Value,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, Value, &mut Context<Self>) + 'static,
    ) {
        let answer = self.models.host.call(command, args);
        cx.spawn(async move |this, cx| {
            let got = answer.await;
            let _ = this.update(cx, |v, cx| {
                match got {
                    Ok(value) => {
                        v.error = None;
                        then(v, value, cx);
                    }
                    Err(e) => v.error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The name typed, made an account of the CLI chosen.
    pub fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.add_name.read(cx).value().trim().to_string();
        if name.is_empty() {
            return;
        }
        let field = self.add_name.clone();
        let provider = self.add_provider;
        let answer = self.models.host.call(cmd::CREATE_ACCOUNT, json!({ "provider": provider, "name": name }));
        cx.spawn_in(window, async move |this, cx| {
            let got = answer.await;
            let _ = this.update_in(cx, |v, window, cx| {
                match got {
                    Ok(_) => {
                        v.error = None;
                        v.adding = false;
                        field.update(cx, |f, cx| f.set_value("", window, cx));
                        v.load(cx);
                    }
                    Err(e) => v.error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn start_rename(&mut self, a: Account, window: &mut Window, cx: &mut Context<Self>) {
        let field = cx.new(|cx| InputState::new(window, cx).default_value(a.name.clone()));
        cx.subscribe_in(&field, window, |this, _, e: &InputEvent, _, cx| {
            if matches!(e, InputEvent::PressEnter { .. }) {
                this.commit_rename(cx);
            }
        })
        .detach();
        field.update(cx, |f, cx| f.focus(window, cx));
        self.renaming = Some((a, field));
        cx.notify();
    }

    /// Renames the account's folder; every chat on it follows (the core's).
    pub fn commit_rename(&mut self, cx: &mut Context<Self>) {
        let Some((a, field)) = self.renaming.take() else { return };
        let to = field.read(cx).value().trim().to_string();
        cx.notify();
        if to.is_empty() || to == a.name {
            return;
        }
        self.rename(&a, &to, cx);
    }

    pub fn rename(&mut self, a: &Account, to: &str, cx: &mut Context<Self>) {
        self.run(cmd::RENAME_ACCOUNT, json!({ "provider": a.provider, "name": a.name, "to": to }), cx, |v, _, cx| v.load(cx));
    }

    pub fn delete(&mut self, a: &Account, cx: &mut Context<Self>) {
        self.run(cmd::DELETE_ACCOUNT, json!({ "provider": a.provider, "name": a.name }), cx, |v, _, cx| v.load(cx));
    }

    pub fn set_parent(&mut self, p: Provider, parent: Option<String>, cx: &mut Context<Self>) {
        self.menu = None;
        self.run(cmd::SET_SETTINGS_PARENT, json!({ "provider": p, "parent": parent }), cx, |v, value, _| {
            v.sync = serde_json::from_value(value).ok();
        });
    }

    pub fn set_sync_on_start(&mut self, on: bool, cx: &mut Context<Self>) {
        self.run(cmd::SET_SETTINGS_SYNC_ON_START, json!({ "on": on }), cx, |v, value, _| {
            v.sync = serde_json::from_value(value).ok();
        });
    }

    /// Copies the parent's settings onto the others of its CLI now.
    pub fn sync_now(&mut self, p: Provider, cx: &mut Context<Self>) {
        self.syncing = Some(p);
        let answer = ask::<SyncReport>(&self.models.host, cmd::SYNC_SETTINGS_NOW, json!({ "provider": p }));
        cx.spawn(async move |this, cx| {
            let got = answer.await;
            let _ = this.update(cx, |v, cx| {
                v.syncing = None;
                v.sync_note.insert(p, got.map_or_else(|e| e, |r| r.said()));
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn open_other(&mut self, panel: Panel, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(layer) = self.layer.upgrade() {
            layer.update(cx, |l, cx| {
                l.close(cx);
                l.open(panel, window, cx);
            });
        }
    }
}

impl Render for AccountsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.models.shell.read(cx).palette();
        let close = self.act(cx, |v, _, cx| {
            if v.may_close(cx) {
                v.close(cx);
            }
        });
        let top = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .child(ui::button(
                "accounts-add",
                &p,
                if self.adding { "Adding..." } else { "Add account" },
                !self.adding,
                self.act(cx, |v, window, cx| {
                    v.adding = true;
                    v.add_name.update(cx, |f, cx| f.focus(window, cx));
                    cx.notify();
                }),
            ))
            .child(div().flex_1())
            .child(ui::button(
                "accounts-library",
                &p,
                "Skills & instructions",
                true,
                self.act(cx, |v, window, cx| v.open_other(Panel::Library, window, cx)),
            ))
            .child(ui::button(
                "accounts-settings",
                &p,
                "Settings",
                true,
                self.act(cx, |v, window, cx| v.open_other(Panel::Settings, window, cx)),
            ));
        let mut body = div().flex().flex_col().gap(px(6.)).p(px(8.)).child(top);
        if self.adding {
            body = body.child(self.add_row(&p, cx));
        }
        if let Some(e) = &self.error {
            body = body.child(ui::alert(&p, Tone::Trouble, e.clone()));
        }
        let list: AnyElement = match &self.accounts {
            None => ui::note(&p, "Reading the accounts...").into_any_element(),
            Some(a) if a.is_empty() => ui::note(&p, "No accounts yet: add one to start sessions on.").into_any_element(),
            Some(accounts) => {
                let mut list = div().flex().flex_col().gap(px(8.));
                for (i, a) in accounts.iter().enumerate() {
                    list = list.child(self.account(i, a, &p, cx));
                }
                list.into_any_element()
            }
        };
        body = body.child(self.shared(&p, cx)).child(list);
        let scroll = div().id("accounts-scroll").flex_1().min_h_0().overflow_y_scroll().child(body);
        let frame = ui::frame(&p, "accounts", "Accounts", px(720.), close, scroll).h(px(600.)).relative();
        match self.deleting.clone() {
            Some(a) => frame.child(self.question(a, cx).render(&p, "accounts-ask")),
            None => frame,
        }
    }
}

impl AccountsPanel {
    fn add_row(&self, p: &Palette, cx: &Context<Self>) -> Div {
        let empty = self.add_name.read(cx).value().trim().is_empty();
        let chosen = self.add_provider;
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(ui::segmented(
                "accounts-add-cli",
                p,
                Provider::ALL
                    .iter()
                    .map(|&prov| {
                        (
                            SharedString::from(prov.label()),
                            prov == chosen,
                            self.act(cx, move |v, _, cx| {
                                v.add_provider = prov;
                                cx.notify();
                            }),
                        )
                    })
                    .collect(),
                true,
            ))
            .child(div().flex_1().child(Input::new(&self.add_name).bg(p.well)))
            .child(ui::primary("accounts-create", p, "Create", !empty, self.act(cx, |v, window, cx| v.create(window, cx))))
            .child(ui::button(
                "accounts-add-cancel",
                p,
                "Cancel",
                true,
                self.act(cx, |v, _, cx| {
                    v.adding = false;
                    cx.notify();
                }),
            ))
    }

    fn account(&self, i: usize, a: &Account, p: &Palette, cx: &Context<Self>) -> Div {
        let limits = self.limits.get(&a.path).cloned().unwrap_or(LimitsState::Loading);
        let refreshing = self.refreshing.contains(&a.path);
        let (dot, state) = match &limits {
            LimitsState::Loading => (p.surface, "loading...".to_owned()),
            LimitsState::Failed(_) => (p.error, "limits unavailable".to_owned()),
            LimitsState::Got(l) if !l.signed_in => (p.error, "signed out".to_owned()),
            LimitsState::Got(l) => (if l.worst() >= 0.85 { p.error } else { p.ok }, l.plan.clone().unwrap_or_else(|| "-".into())),
        };
        let name: AnyElement = match &self.renaming {
            Some((r, field)) if r == a => div()
                .flex()
                .items_center()
                .gap(px(4.))
                .child(div().w(px(200.)).child(Input::new(field).bg(p.well)))
                .child(ui::primary(&format!("accounts-rename-ok-{i}"), p, "Rename", true, self.act(cx, |v, _, cx| v.commit_rename(cx))))
                .child(ui::button(
                    &format!("accounts-rename-cancel-{i}"),
                    p,
                    "Cancel",
                    true,
                    self.act(cx, |v, _, cx| {
                        v.renaming = None;
                        cx.notify();
                    }),
                ))
                .into_any_element(),
            _ => div().min_w_0().truncate().font_weight(FontWeight::BOLD).child(a.name.clone()).into_any_element(),
        };
        let (rename, refresh, delete) = (a.clone(), a.clone(), a.clone());
        let head = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(div().size(px(10.)).flex_none().bg(dot).border_1().border_color(p.frame))
            .child(div().flex_none().px(px(4.)).bg(p.title_bg).text_color(p.title_fg).child(a.provider.label()))
            .child(name)
            .child(div().flex_1())
            .child(div().flex_none().text_color(p.text_dim).child(state))
            .child(ui::button(
                &format!("accounts-rename-{i}"),
                p,
                "Rename",
                self.renaming.is_none(),
                self.act(cx, move |v, window, cx| v.start_rename(rename.clone(), window, cx)),
            ))
            .child(ui::button(
                &format!("accounts-refresh-{i}"),
                p,
                if refreshing { "Refreshing..." } else { "Refresh" },
                !refreshing,
                self.act(cx, move |v, _, cx| v.ask_limits(&refresh, true, cx)),
            ))
            .child(ui::button(
                &format!("accounts-delete-{i}"),
                p,
                "Delete...",
                true,
                self.act(cx, move |v, _, cx| {
                    v.deleting = Some(delete.clone());
                    cx.notify();
                }),
            ));
        let mut card = div().flex().flex_col().gap(px(3.)).p(px(6.)).border_1().border_color(p.frame).bg(p.surface).child(head);
        if let LimitsState::Got(l) = &limits {
            if let Some(email) = &l.email {
                card = card.child(ui::note(p, format!("{email} - {}", a.path)));
            } else {
                card = card.child(ui::path_text(p, a.path.clone()));
            }
            if l.have_usage {
                for (n, w) in l.windows.iter().enumerate() {
                    card = card.child(limit_bar(p, &format!("accounts-limit-{i}-{n}"), w));
                }
            }
            if let Some(why) = l.usage_note() {
                card = card.child(ui::note(p, why));
            }
            if let Some(e) = &l.refresh_error {
                card = card.child(ui::alert(p, Tone::Trouble, e.clone()));
            }
        } else {
            card = card.child(ui::path_text(p, a.path.clone()));
            if let LimitsState::Failed(e) = &limits {
                card = card.child(ui::alert(p, Tone::Trouble, e.clone()));
            }
        }
        card
    }

    /// The parent per CLI whose settings the others copy, and the box for
    /// doing it at every start.
    fn shared(&self, p: &Palette, cx: &Context<Self>) -> Div {
        let Some(state) = &self.sync else { return div() };
        let Some(accounts) = self.accounts.as_ref().filter(|a| !a.is_empty()) else { return div() };
        let mut d = div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .p(px(6.))
            .border_1()
            .border_color(p.frame)
            .child(div().font_weight(FontWeight::BOLD).child("Shared settings"));
        let mut any = false;
        for prov in Provider::ALL {
            let mine: Vec<&Account> = accounts.iter().filter(|a| a.provider == prov).collect();
            if mine.len() < 2 {
                if mine.len() == 1 {
                    d = d.child(ui::note(p, format!("{}: only one account, nothing to share its settings with yet.", prov.label())));
                }
                continue;
            }
            any = true;
            let parent = state.parent(prov).map(str::to_owned);
            let id = format!("accounts-parent-{}", prov.key());
            let open = self.menu.as_deref() == Some(id.as_str());
            let mut items = vec![DropItem {
                label: "- no parent -".into(),
                chosen: parent.is_none(),
                tick: false,
                enabled: true,
                pick: self.act(cx, move |v, _, cx| v.set_parent(prov, None, cx)),
            }];
            items.extend(mine.iter().map(|a| {
                let name = a.name.clone();
                DropItem {
                    label: a.name.clone().into(),
                    chosen: parent.as_deref() == Some(a.name.as_str()),
                    tick: false,
                    enabled: true,
                    pick: self.act(cx, move |v, _, cx| v.set_parent(prov, Some(name.clone()), cx)),
                }
            }));
            let toggle = id.clone();
            let syncing = self.syncing == Some(prov);
            d = d
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(div().w(px(150.)).child(format!("{} from", prov.label())))
                        .child(
                            Dropdown {
                                id,
                                face: parent.clone().unwrap_or_else(|| "- no parent -".into()).into(),
                                open,
                                enabled: true,
                                width: px(180.),
                                items,
                                on_toggle: self.act(cx, move |v, _, cx| {
                                    v.menu = if v.menu.as_deref() == Some(toggle.as_str()) { None } else { Some(toggle.clone()) };
                                    cx.notify();
                                }),
                                on_close: self.act(cx, |v, _, cx| {
                                    v.menu = None;
                                    cx.notify();
                                }),
                            }
                            .render(p),
                        )
                        .child(ui::button(
                            &format!("accounts-sync-{}", prov.key()),
                            p,
                            if syncing { "Syncing..." } else { "Sync now" },
                            parent.is_some() && !syncing,
                            self.act(cx, move |v, _, cx| v.sync_now(prov, cx)),
                        )),
                )
                .children(self.sync_note.get(&prov).map(|n| ui::note(p, format!("{}: {n}", prov.label()))));
        }
        if !any {
            return d;
        }
        let on = state.on_start;
        d.child(ui::check(
            "accounts-sync-on-start",
            p,
            "Copy from the parents every time Luna starts",
            on,
            true,
            self.act(cx, move |v, _, cx| v.set_sync_on_start(!on, cx)),
        ))
        .child(ui::note(
            p,
            "The other accounts get an exact copy of the parent's file: whatever was set on them alone is replaced. Logins and Codex's trusted folders stay each account's own.",
        ))
    }

    fn question(&self, a: Account, cx: &Context<Self>) -> Confirm {
        Confirm {
            title: "Delete account".into(),
            body: format!(
                "Deletes the folder {} and the login stored in it. Nothing signs you out anywhere else, and nothing here can bring it back.",
                a.path
            )
            .into(),
            yes: "Delete".into(),
            on_yes: self.act(cx, move |v, _, cx| {
                v.deleting = None;
                v.delete(&a, cx);
            }),
            on_no: self.act(cx, |v, _, cx| {
                v.deleting = None;
                cx.notify();
            }),
        }
    }

    /// The name field of an account to add, filled in (the driver's).
    #[cfg(any(test, luna_test, feature = "shots"))]
    pub(crate) fn add_field_value(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.add_name.update(cx, |f, cx| f.set_value(name.to_owned(), window, cx));
    }

    /// For the tests: the name field of an account to add.
    #[cfg(test)]
    pub fn add_field(&self) -> Entity<InputState> {
        self.add_name.clone()
    }

    /// For the tests: the account whose deletion asks.
    #[cfg(test)]
    pub fn asking_delete(&self) -> Option<&Account> {
        self.deleting.as_ref()
    }

    /// Delete... on the `i`th account: its deletion asks first.
    #[cfg(any(test, luna_test, feature = "shots"))]
    pub fn ask_delete(&mut self, i: usize, cx: &mut Context<Self>) {
        self.deleting = self.accounts.as_ref().and_then(|a| a.get(i)).cloned();
        cx.notify();
    }
}

/// One usage window: what it is, a meter, how full, and when it resets, as
/// the clock time it falls on.
fn limit_bar(p: &Palette, id: &str, w: &super::UsageWindow) -> gpui::Stateful<Div> {
    let fill = if w.used >= 0.85 {
        p.error
    } else if w.used >= 0.6 {
        p.attention
    } else {
        p.ok
    };
    let reset = w.resets_at.as_deref().and_then(crate::panels::local_reset).map(|t| format!("resets {t}")).unwrap_or_default();
    div()
        .id(SharedString::from(id.to_owned()))
        .flex()
        .items_center()
        .gap(px(8.))
        .child(div().w(px(130.)).flex_none().truncate().child(w.label.clone()))
        .child(ui::meter(p, Some(w.used), fill))
        .child(div().w(px(40.)).flex_none().flex().justify_end().child(format!("{}%", (w.used * 100.).round() as i32)))
        .child(div().w(px(190.)).flex_none().whitespace_nowrap().text_color(p.text_dim).child(reset))
}
