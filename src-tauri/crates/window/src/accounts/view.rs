//! The accounts window's view, and what each of its controls asks of the host.

use super::login::{Closed, LoginWindow};
use super::{Account, Identity, Limits, SyncReport, SyncState};
use crate::host::Answer;
use crate::models::Models;
use crate::panels::ui::{self, Confirm, DropItem, Dropdown, OnClick, Tone};
use crate::panels::{Panel, PanelLayer, Provider, ask, cmd};
use crate::scale::u;
use crate::theme::Palette;
use gpui::{
    AnyElement, AppContext, Context, Div, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, WeakEntity, Window, div,
};
use gpui_component::input::{Input, InputEvent, InputState};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// An account as `create_account` answers it: with its folder's identity.
#[derive(serde::Deserialize)]
struct Made {
    #[serde(flatten)]
    account: Account,
    id: String,
}

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
    /// The accounts agents may not spawn on, as `provider/name`, once read.
    pub blocked: Option<Vec<String>>,
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
    /// The drop-down list open, by its selector.
    pub menu: Option<String>,
    /// The account whose deletion waits on a yes, with the folder the
    /// question names (`account_identity`), to which the yes is bound.
    deleting: Option<(Account, Identity)>,
    /// The sign-in window over this one, and its end told.
    login: Option<(Entity<LoginWindow>, Subscription)>,
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
            blocked: None,
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
            login: None,
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

    /// The accounts, and each one's limits; the switch of each, as it stands.
    pub fn load(&mut self, cx: &mut Context<Self>) {
        self.load_blocked(cx);
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

    /// The accounts agents may not spawn on (`agent_blocked_accounts`).
    fn load_blocked(&mut self, cx: &mut Context<Self>) {
        let answer = ask::<Vec<String>>(&self.models.host, cmd::AGENT_BLOCKED_ACCOUNTS, json!({}));
        cx.spawn(async move |this, cx| {
            let got = answer.await;
            let _ = this.update(cx, |v, cx| {
                match got {
                    Ok(list) => v.blocked = Some(list),
                    Err(e) => v.error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Whether agents may spawn on the account; so until the list is read.
    pub fn agents_may(&self, a: &Account) -> bool {
        let key = format!("{}/{}", a.provider.key(), a.name);
        self.blocked.as_ref().is_none_or(|b| !b.contains(&key))
    }

    /// The account's switch: agents may, or may not, spawn on it. Refused
    /// while an update installs, as any change of an account is. The core
    /// answers the list as it is then.
    pub fn set_agents_may(&mut self, a: &Account, allowed: bool, cx: &mut Context<Self>) {
        if let Err(why) = self.models.workspace.read(cx).unfrozen() {
            self.error = Some(why);
            cx.notify();
            return;
        }
        let args = json!({ "provider": a.provider, "name": a.name, "allowed": allowed });
        self.run(cmd::SET_AGENT_ACCOUNT, args, cx, |v, list, _| {
            v.blocked = serde_json::from_value(list).ok();
        });
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
    /// 429 cool-off, renewing an expired token on the way. What comes back
    /// goes to the status bar's model too, which tells the core
    /// (`Vitals::took_limits`): a Refresh or a sign-in reaches the bar and the
    /// agents' usage at once, not at the bar's next poll.
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
        let status = self.models.status.clone();
        cx.spawn(async move |this, cx| {
            let answer = answer.await;
            if let Ok(value) = &answer {
                status.update(cx, |s, cx| s.took_limits(provider.into(), &path, value.clone(), cx));
            }
            let got = answer.and_then(|v: Value| Limits::read(provider, v));
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
        if let Some((login, _)) = &self.login {
            login.update(cx, |l, cx| l.close(cx));
            return false;
        }
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
        self.show(answer, cx, then);
    }

    /// Answers `then` once `answer` has come; a failure is said at the top.
    fn show(&mut self, answer: Answer, cx: &mut Context<Self>, then: impl FnOnce(&mut Self, Value, &mut Context<Self>) + 'static) {
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

    /// The name typed, made an account of the CLI chosen, and its sign-in
    /// opened over this window, as the page's store did: a fresh folder has
    /// no login, and the CLI's own login run in it is what puts one there.
    /// The account signed in is the one the core says it made, and only on
    /// the very folder it made (`Made::id`): one put in its place meanwhile,
    /// or the root led elsewhere, gets no login.
    pub fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.add_name.read(cx).value().trim().to_string();
        if name.is_empty() {
            return;
        }
        let field = self.add_name.clone();
        let provider = self.add_provider;
        let answer = self.models.host.call(cmd::CREATE_ACCOUNT, json!({ "provider": provider, "name": name }));
        // The workspace lists the accounts again itself, this window open or not.
        let answer = self.models.workspace.update(cx, |w, cx| w.accounts_changed_by(answer, cx));
        cx.spawn_in(window, async move |this, cx| {
            let got = answer.await;
            let _ = this.update_in(cx, |v, window, cx| {
                match got {
                    Ok(made) => {
                        v.error = None;
                        v.adding = false;
                        field.update(cx, |f, cx| f.set_value("", window, cx));
                        v.load(cx);
                        match serde_json::from_value::<Made>(made) {
                            Ok(Made { account, id }) => v.open_login(account, Some(id), window, cx),
                            Err(e) => log::warn!("accounts: the new account's answer is not one: {e}"),
                        }
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

    /// The chats on it follow, as the page's store does: the workspace
    /// renames and has them follow (`rename_account_on_host`), so they do
    /// even when this window is closed before the answer; here only what
    /// it said is shown. Not before the chat list has come, which would
    /// put the old name back.
    pub fn rename(&mut self, a: &Account, to: &str, cx: &mut Context<Self>) {
        let renamed = self.models.workspace.update(cx, |w, cx| w.rename_account_on_host(a.provider.into(), &a.name, to, cx));
        self.show(renamed, cx, |v, _, cx| v.load(cx));
    }

    /// Delete...: the account's folder named first (`account_identity`),
    /// its path and its identity on the drive; the question asks of that one.
    /// Not before the chat list has come: the question names the chats on it.
    pub fn ask_delete_of(&mut self, a: Account, cx: &mut Context<Self>) {
        if let Some(why) = self.models.workspace.read(cx).list_wait() {
            self.error = Some(why);
            cx.notify();
            return;
        }
        let named = ask::<Identity>(&self.models.host, cmd::ACCOUNT_IDENTITY, json!({ "provider": a.provider, "name": a.name }));
        cx.spawn(async move |this, cx| {
            let got = named.await;
            let _ = this.update(cx, |v, cx| {
                match got {
                    Ok(id) => v.deleting = Some((a, id)),
                    Err(e) => v.error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The yes: deleted only while the folder is still the one the question
    /// named (`delete_account_checked`).
    pub fn delete(&mut self, a: &Account, id: &Identity, cx: &mut Context<Self>) {
        if let Some(why) = self.models.workspace.read(cx).list_wait() {
            self.error = Some(why);
            cx.notify();
            return;
        }
        let args = json!({ "provider": a.provider, "name": a.name, "path": id.path, "id": id.id });
        let answer = self.models.host.call(cmd::DELETE_ACCOUNT_CHECKED, args);
        let answer = self.models.workspace.update(cx, |w, cx| w.accounts_changed_by(answer, cx));
        self.show(answer, cx, |v, _, cx| v.load(cx));
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

    /// Sign in: the account's CLI login in a terminal over this window.
    pub fn sign_in(&mut self, a: Account, window: &mut Window, cx: &mut Context<Self>) {
        self.open_login(a, None, window, cx);
    }

    /// The login of `a`; `bound`, the identity its folder must still have
    /// where it is the one just made.
    fn open_login(&mut self, a: Account, bound: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        if self.login.is_some() {
            return;
        }
        let models = self.models.clone();
        let login = cx.new(|cx| LoginWindow::new(models, a, bound, window, cx));
        let sub = cx.subscribe(&login, |this, _, _: &Closed, cx| this.login_closed(cx));
        self.login = Some((login, sub));
        cx.notify();
    }

    /// The sign-in window is done (the CLI exited, or it was closed): the
    /// accounts are listed again, and the account's limits asked past the
    /// cache, as its sign-in state may have changed.
    fn login_closed(&mut self, cx: &mut Context<Self>) {
        let Some((login, _)) = self.login.take() else { return };
        let account = login.read(cx).account().clone();
        login.update(cx, |l, cx| l.end(cx));
        self.load(cx);
        self.ask_limits(&account, true, cx);
        cx.notify();
    }

    /// Whether the sign-in terminal has the keyboard: Esc is then the CLI's,
    /// not this window's.
    pub fn login_has_keyboard(&self, window: &Window, cx: &gpui::App) -> bool {
        self.login.as_ref().is_some_and(|(l, _)| l.read(cx).terminal_has_keyboard(window, cx))
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
            .gap(u(4.))
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
        let mut body = div().flex().flex_col().gap(u(6.)).p(u(8.)).child(top);
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
                let mut list = div().flex().flex_col().gap(u(6.));
                for (i, a) in accounts.iter().enumerate() {
                    list = list.child(self.account(i, a, &p, cx));
                }
                list.into_any_element()
            }
        };
        body = body.child(self.shared(&p, cx)).child(list);
        let scroll = div().id("accounts-scroll").flex_1().min_h_0().overflow_y_scroll().child(body);
        let frame = ui::frame(&p, "accounts", "Accounts", u(720.), close, scroll).h(u(600.)).relative();
        let frame = match self.deleting.clone() {
            Some((a, id)) => frame.child(self.question(a, id, cx).render(&p, "accounts-ask")),
            None => frame,
        };
        match &self.login {
            Some((login, _)) => frame.child(login.clone()),
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
            .gap(u(6.))
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
            // The field gives way before the row's buttons do, so they stay
            // inside the card in a narrow window.
            Some((r, field)) if r == a => div()
                .flex()
                .min_w_0()
                .items_center()
                .gap(u(4.))
                .child(div().w(u(200.)).min_w(u(60.)).child(Input::new(field).bg(p.well)))
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
        let (rename, refresh, delete, sign_in) = (a.clone(), a.clone(), a.clone(), a.clone());
        // As the page's: offered where there are no usable credentials.
        let signed_out = matches!(&limits, LimitsState::Got(l) if !l.signed_in) || matches!(&limits, LimitsState::Failed(_));
        let head = div()
            .flex()
            .items_center()
            .gap(u(6.))
            .child(div().size(u(10.)).flex_none().bg(dot).border_1().border_color(p.frame))
            .child(div().flex_none().px(u(4.)).bg(p.title_bg).text_color(p.title_fg).child(a.provider.label()))
            .child(name)
            .child(div().flex_1())
            .child(div().flex_none().text_color(p.text_dim).child(state))
            .children(signed_out.then(|| {
                ui::primary(
                    &format!("accounts-signin-{i}"),
                    p,
                    "Sign in",
                    self.login.is_none(),
                    self.act(cx, move |v, window, cx| v.sign_in(sign_in.clone(), window, cx)),
                )
            }))
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
                self.act(cx, move |v, _, cx| {
                    v.load_blocked(cx);
                    v.ask_limits(&refresh, true, cx);
                }),
            ))
            .child(ui::button(
                &format!("accounts-delete-{i}"),
                p,
                "Delete...",
                true,
                self.act(cx, move |v, _, cx| v.ask_delete_of(delete.clone(), cx)),
            ));
        let may = self.agents_may(a);
        let toggle = a.clone();
        // The switch shares the line of the folder (or the sign-in email and
        // folder), which gives way to it, so a row stays short enough for the
        // window's small sizes.
        let place: AnyElement = match &limits {
            LimitsState::Got(Limits { email: Some(email), .. }) => ui::path_text(p, format!("{email} - {}", a.path)).into_any_element(),
            _ => ui::path_text(p, a.path.clone()).into_any_element(),
        };
        let switch = ui::check(
            &format!("accounts-agents-{i}"),
            p,
            if may { "Agents may spawn sessions on this account" } else { "Agents may not use this account: tick to allow" },
            may,
            self.blocked.is_some(),
            self.act(cx, move |v, _, cx| {
                let now = v.agents_may(&toggle);
                v.set_agents_may(&toggle, !now, cx);
            }),
        );
        let mut card = div().flex().flex_col().gap(u(2.)).p(u(6.)).border_1().border_color(p.frame).bg(p.surface).child(head);
        card = card.child(div().flex().items_center().gap(u(12.)).child(div().flex_1().min_w_0().child(place)).child(switch));
        if let LimitsState::Got(l) = &limits {
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
        } else if let LimitsState::Failed(e) = &limits {
            card = card.child(ui::alert(p, Tone::Trouble, e.clone()));
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
            .gap(u(4.))
            .p(u(6.))
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
                        .gap(u(6.))
                        .child(div().w(u(150.)).child(format!("{} from", prov.label())))
                        .child(
                            Dropdown {
                                id,
                                face: parent.clone().unwrap_or_else(|| "- no parent -".into()).into(),
                                open,
                                enabled: true,
                                width: u(180.),
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

    fn question(&self, a: Account, id: Identity, cx: &Context<Self>) -> Confirm {
        Confirm {
            title: "Delete account".into(),
            body: self.question_text(&a, &id, cx).into(),
            yes: "Delete".into(),
            on_yes: self.act(cx, move |v, _, cx| {
                v.deleting = None;
                v.delete(&a, &id, cx);
            }),
            on_no: self.act(cx, |v, _, cx| {
                v.deleting = None;
                cx.notify();
            }),
        }
    }

    /// What the question says: the folder, and the chats set to the account,
    /// which have nowhere to start once it is gone.
    fn question_text(&self, a: &Account, id: &Identity, cx: &gpui::App) -> String {
        let mut text = format!(
            "Deletes the folder {} and the login stored in it. Nothing signs you out anywhere else, and nothing here can bring it back.",
            id.path
        );
        let chats = self.models.workspace.read(cx).chats_on(a.provider.into(), &a.name);
        if !chats.is_empty() {
            const NAMED: usize = 5;
            let named: Vec<&str> = chats.iter().take(NAMED).map(String::as_str).collect();
            let more = chats.len().saturating_sub(NAMED);
            text += &format!(
                " Chats are still set to this account, and have nowhere to start until you point them at another one: {}{}.",
                named.join(", "),
                if more > 0 { format!(" and {more} more") } else { String::new() }
            );
        }
        text
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
        self.deleting.as_ref().map(|(a, _)| a)
    }

    /// Delete... on the `i`th account: its deletion asks first.
    #[cfg(any(test, luna_test, feature = "shots"))]
    pub fn ask_delete(&mut self, i: usize, cx: &mut Context<Self>) {
        if let Some(a) = self.accounts.as_ref().and_then(|a| a.get(i)).cloned() {
            self.ask_delete_of(a, cx);
        }
    }

    /// The add form open, as Add account opens it (the driver's).
    #[cfg(any(test, luna_test, feature = "shots"))]
    pub fn open_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.adding = true;
        self.add_name.update(cx, |f, cx| f.focus(window, cx));
        cx.notify();
    }

    /// The `i`th account's name in edit, as its Rename starts it (the driver's).
    #[cfg(any(test, luna_test, feature = "shots"))]
    pub fn rename_nth(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(a) = self.accounts.as_ref().and_then(|a| a.get(i)).cloned() {
            self.start_rename(a, window, cx);
        }
    }

    /// Sign in on the `i`th account, as its button does (the driver's).
    #[cfg(any(test, luna_test, feature = "shots"))]
    pub fn sign_in_nth(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(a) = self.accounts.as_ref().and_then(|a| a.get(i)).cloned() {
            self.sign_in(a, window, cx);
        }
    }

    /// What the Delete question says, while one is up.
    #[cfg(test)]
    pub fn asking_text(&self, cx: &gpui::App) -> Option<String> {
        self.deleting.as_ref().map(|(a, id)| self.question_text(a, id, cx))
    }

    /// The sign-in window's session and its phase, for the driver's `panels`.
    #[cfg(luna_test)]
    pub fn login_state(&self, cx: &gpui::App) -> Option<String> {
        self.login.as_ref().map(|(l, _)| l.read(cx).state(cx))
    }

    /// The sign-in window up, for the tests.
    #[cfg(test)]
    pub fn login(&self) -> Option<Entity<LoginWindow>> {
        self.login.as_ref().map(|(l, _)| l.clone())
    }

    /// The question up answered yes, as its button does (the driver's).
    #[cfg(any(test, luna_test, feature = "shots"))]
    pub fn answer_yes(&mut self, cx: &mut Context<Self>) {
        if let Some((a, id)) = self.deleting.take() {
            self.delete(&a, &id, cx);
        }
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
        .gap(u(8.))
        .child(div().w(u(130.)).flex_none().truncate().child(w.label.clone()))
        .child(ui::meter(p, Some(w.used), fill))
        .child(div().w(u(40.)).flex_none().flex().justify_end().child(format!("{}%", (w.used * 100.).round() as i32)))
        .child(div().w(u(190.)).flex_none().whitespace_nowrap().text_color(p.text_dim).child(reset))
}
