//! The window's vital signs, as one model the bar and the orphan line draw:
//! what the chats are doing (counts), each account's usage, the Luna update,
//! the CLIs' downloads, the power rule, and the sessions no chat reaches.
//!
//! Nothing here ticks. What the host says arrives as events; the rest is
//! asked for on the slow schedules the page kept: the accounts' usage on the
//! page's cadence (usage.rs), the orphans every 30 seconds, the update once
//! at start. The one clock is the power countdown, one second at a time and
//! only while a countdown runs. A model notifies only when what is drawn
//! changed, so an idle window draws nothing.

use super::dto::{CliStatus, Orphan, PowerAction, PowerState};
use super::updates::{self, UNSAVED, Update};
use super::usage::{self, Cadence, Round, Usage};
use super::{cmd, power};
use crate::desktop::{self, UpdateCheck, UpdateProgress};
use crate::dto::LinkState;
use crate::host::Host;
use crate::models::CoreLink;
use crate::state::Status;
use crate::term::paste::Provider;
use crate::workspace::Workspace;
use gpui::{App, AppContext, Context, Entity, Task, Window};
use serde_json::{Value, json};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

/// How often the sessions no chat reaches are looked for, as the page did.
pub const ORPHANS_EVERY: Duration = Duration::from_secs(30);
/// The countdown's clock, only while one runs.
pub const COUNTDOWN_EVERY: Duration = Duration::from_secs(1);

/// The providers, in the order the chips come.
pub const PROVIDERS: [Provider; 2] = [Provider::Claude, Provider::Codex];

pub fn provider_key(p: Provider) -> &'static str {
    match p {
        Provider::Claude => "claude",
        Provider::Codex => "codex",
    }
}

/// What the chats are doing, as the bar counts it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub working: usize,
    pub waiting: usize,
    pub background: usize,
}

/// The panel open over the bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Popover {
    Power,
    Update,
}

/// What the power popover is set to arm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arm {
    pub action: PowerAction,
    pub quiet_s: u64,
    pub drain: bool,
}

impl Default for Arm {
    fn default() -> Arm {
        Arm { action: PowerAction::Shutdown, quiet_s: 120, drain: false }
    }
}

/// The quiet windows the popover offers, in seconds.
pub const QUIET: [(u64, &str); 5] = [(60, "1 min"), (120, "2 min"), (300, "5 min"), (600, "10 min"), (1800, "30 min")];

/// The actions the popover offers: restart is Windows' alone (the host
/// refuses it elsewhere).
pub fn actions() -> Vec<PowerAction> {
    let mut all = vec![PowerAction::Shutdown];
    if cfg!(windows) {
        all.push(PowerAction::Restart);
    }
    all.extend([PowerAction::Hibernate, PowerAction::Sleep]);
    all
}

/// The system's clock in epoch milliseconds.
pub fn system_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

pub struct Vitals {
    host: Arc<dyn Host>,
    workspace: Entity<Workspace>,
    core: Entity<CoreLink>,
    /// Epoch ms; the tests and the shots hold it still.
    clock: Rc<dyn Fn() -> u64>,
    /// When the numbers on the bar were last brought up to date.
    now_ms: u64,
    /// The pause between one account's ask and the next in a round.
    gap: Duration,

    counts: Counts,
    power: PowerState,
    clis: Vec<CliStatus>,
    accounts: Vec<Usage>,
    /// The accounts the workspace listed, by provider, name and path.
    listed: Vec<(Provider, String, String)>,
    cadence: Cadence,
    update: Update,
    checking: bool,
    /// The workspace has listed the accounts, even if there are none.
    accounts_loaded: bool,
    orphans: Vec<Orphan>,
    /// The chats the last look for orphans was told of.
    scanned_for: Option<Vec<String>>,
    /// Kill all was clicked once: the line asks before it kills.
    kill_asked: bool,
    popover: Option<Popover>,
    arm: Arm,
    /// What the tray was last told of the awake hold (`tray_awake`'s
    /// holders): None before the first time.
    tray_told: Option<Option<String>>,
    /// The usage as the core was last sent it (`account_usage`).
    usage_told: Option<String>,
    /// The core's epoch the window last refreshed itself in.
    epoch: u64,
    scans: u64,
    ticking: bool,

    _poll: Option<Task<()>>,
    _scan: Option<Task<()>>,
    _tick: Option<Task<()>>,
    /// Times the model told its views to draw, for the tests that hold the
    /// window still while idle.
    pub(crate) notifies: usize,
}

impl Vitals {
    pub fn new(host: Arc<dyn Host>, workspace: Entity<Workspace>, core: Entity<CoreLink>, cx: &mut Context<Self>) -> Vitals {
        cx.observe(&workspace, |this, _, cx| this.on_workspace(cx)).detach();
        cx.observe(&core, |this, _, cx| this.on_core(cx)).detach();
        cx.observe_global::<UpdateProgress>(|this, cx| this.on_progress(cx)).detach();
        Vitals {
            host,
            workspace,
            core,
            clock: Rc::new(system_ms),
            now_ms: system_ms(),
            gap: usage::GAP,
            counts: Counts::default(),
            power: PowerState::default(),
            clis: Vec::new(),
            accounts: Vec::new(),
            listed: Vec::new(),
            cadence: Cadence::default(),
            update: Update::Idle,
            checking: false,
            accounts_loaded: false,
            orphans: Vec::new(),
            scanned_for: None,
            kill_asked: false,
            popover: None,
            arm: Arm::default(),
            tray_told: None,
            usage_told: None,
            epoch: 0,
            scans: 0,
            ticking: false,
            _poll: None,
            _scan: None,
            _tick: None,
            notifies: 0,
        }
    }

    /// The clock the model reads the time from.
    pub fn set_clock(&mut self, clock: Rc<dyn Fn() -> u64>) {
        self.now_ms = clock();
        self.clock = clock;
    }

    /// The pause between the accounts' asks, for a shot that cannot wait.
    #[cfg(any(test, feature = "shots"))]
    pub fn set_gap(&mut self, gap: Duration) {
        self.gap = gap;
    }

    /// What the window does once it is up: the first look at the CLIs and the
    /// update, the orphans' schedule, and the tray's plain icon.
    pub fn start(&mut self, cx: &mut Context<Self>) {
        for p in PROVIDERS {
            let answer = self.host.call(cmd::CLI_STATUS, json!({ "provider": provider_key(p) }));
            cx.spawn(async move |this, cx| {
                if let Some(status) = answer.await.ok().and_then(|v| serde_json::from_value::<CliStatus>(v).ok()) {
                    let _ = this.update(cx, |this, cx| this.take_cli(status, cx));
                }
            })
            .detach();
        }
        // A release found at start lights a chip and waits there; a failed
        // check is quiet.
        self.check(false, cx);
        self.on_workspace(cx);
        self.on_core(cx);
        self.tell_tray(cx);
        self._scan = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(ORPHANS_EVERY).await;
                if this.update(cx, |this, cx| this.scan(cx)).is_err() {
                    return;
                }
            }
        }));
    }

    fn notify(&mut self, cx: &mut Context<Self>) {
        self.notifies += 1;
        cx.notify();
    }

    fn core_up(&self, cx: &App) -> bool {
        self.core.read(cx).link().state == LinkState::Up
    }

    // ---------------------------------------------------------------- reads

    pub fn counts(&self) -> Counts {
        self.counts
    }

    pub fn power(&self) -> &PowerState {
        &self.power
    }

    pub fn accounts(&self) -> &[Usage] {
        &self.accounts
    }

    /// Whether the accounts have been listed: before that the bar says
    /// nothing of them, after it and with none, that there are none.
    pub fn accounts_loaded(&self) -> bool {
        self.accounts_loaded
    }

    pub fn now_ms(&self) -> u64 {
        self.now_ms
    }

    /// The time now, by the model's clock: what a countdown is read against.
    pub fn clock_ms(&self) -> u64 {
        (self.clock)()
    }

    pub fn clis(&self) -> &[CliStatus] {
        &self.clis
    }

    pub fn phase(&self) -> &Update {
        &self.update
    }

    pub fn orphans(&self) -> &[Orphan] {
        &self.orphans
    }

    pub fn kill_asked(&self) -> bool {
        self.kill_asked
    }

    pub fn popover(&self) -> Option<Popover> {
        self.popover
    }

    pub fn arm(&self) -> Arm {
        self.arm
    }

    /// The bar's state in one line, for the test build's driver (drive.rs).
    #[cfg(luna_test)]
    pub fn summary(&self) -> String {
        let accounts: Vec<String> = self
            .accounts
            .iter()
            .map(|a| {
                let windows: Vec<String> = a.windows.iter().map(|w| format!("{} {}%", w.short, (w.used * 100.).round())).collect();
                format!("{}/{} {:?} [{}]", provider_key(a.provider), a.name, a.sync, windows.join(", "))
            })
            .collect();
        format!(
            "counts {:?} accounts {accounts:?} update {:?} power {{ keepAwake {} holding {} armed {:?} countdown {:?} sessions {} }} orphans {} clis {:?}",
            self.counts,
            self.update,
            self.power.keep_awake,
            self.power.holding,
            self.power.armed.as_ref().map(|a| (a.action.name(), a.quiet_s)),
            self.countdown_left(),
            self.power.summary.sessions.len(),
            self.orphans.len(),
            self.clis.iter().map(|c| (c.provider.map(provider_key), c.phase.as_str())).collect::<Vec<_>>()
        )
    }

    /// The seconds a countdown has left, while one runs.
    pub fn countdown_left(&self) -> Option<u64> {
        self.power.countdown_ends_at_ms.map(|ends| power::left_s(ends, (self.clock)()))
    }

    /// A chat's name by id, for what the popover and the tray say of the
    /// sessions; the id's start where it is not listed.
    pub fn chat_name(&self, id: &str, cx: &App) -> String {
        self.workspace.read(cx).state.find(id).map_or_else(|| id.chars().take(8).collect(), |c| c.name.clone())
    }

    // ------------------------------------------------------------- workspace

    /// The chats changed: the counts, the accounts, the chats the orphans are
    /// told apart by, the names the tray's tooltip uses.
    fn on_workspace(&mut self, cx: &mut Context<Self>) {
        let (counts, listed, first) = {
            let ws = self.workspace.read(cx);
            let mut counts = Counts::default();
            for c in ws.state.chats() {
                match ws.shown_status(c, cx) {
                    Status::Working => counts.working += 1,
                    Status::Waiting => counts.waiting += 1,
                    Status::Background => counts.background += 1,
                    Status::Resting | Status::NoCore => {}
                }
            }
            let listed = ws.accounts().map(|a| a.iter().map(|a| (a.provider, a.name.clone(), a.path.clone())).collect::<Vec<_>>());
            (counts, listed, ws.loaded_yet())
        };
        if counts != self.counts {
            self.counts = counts;
            self.notify(cx);
        }
        if let Some(listed) = listed {
            if !self.accounts_loaded {
                self.accounts_loaded = true;
                self.notify(cx);
            }
            if listed != self.listed {
                self.set_accounts(listed, cx);
            }
        }
        // A chat list that has loaded, or changed, says which sessions have
        // a chat; one not loaded yet would make every session an orphan.
        if first {
            let known = self.known_chats(cx);
            if self.scanned_for.as_ref() != Some(&known) {
                self.scan(cx);
            }
        }
        if self.power.holding {
            self.tell_tray(cx);
        }
    }

    fn known_chats(&self, cx: &App) -> Vec<String> {
        let mut ids: Vec<String> = self.workspace.read(cx).state.chats().map(|c| c.id.clone()).collect();
        ids.sort();
        ids
    }

    /// The core came up, went, or was replaced.
    fn on_core(&mut self, cx: &mut Context<Self>) {
        if !self.core_up(cx) {
            // Nothing is known to run while it is not there.
            if !self.orphans.is_empty() {
                self.orphans.clear();
                self.kill_asked = false;
                self.notify(cx);
            }
            return;
        }
        let epoch = self.core.read(cx).epoch();
        if epoch == self.epoch {
            return;
        }
        self.epoch = epoch;
        // A core that is new knows nothing the window told the old one, and
        // the power rule is its own.
        self.usage_told = None;
        self.push_usage(cx);
        let answer = self.host.call(cmd::POWER_STATE, json!({}));
        cx.spawn(async move |this, cx| {
            if let Ok(v) = answer.await {
                let _ = this.update(cx, |this, cx| this.take_power(v, cx));
            }
        })
        .detach();
        self.scan(cx);
    }

    // ----------------------------------------------------------------- usage

    fn set_accounts(&mut self, listed: Vec<(Provider, String, String)>, cx: &mut Context<Self>) {
        let mut accounts: Vec<Usage> = Vec::new();
        for (provider, name, path) in &listed {
            let kept = self.accounts.iter().find(|a| &a.path == path).cloned();
            accounts.push(kept.map_or_else(|| Usage::new(*provider, name.clone(), path.clone()), |a| Usage { name: name.clone(), ..a }));
        }
        self.accounts = accounts;
        self.listed = listed;
        self.notify(cx);
        self.start_polling(cx);
    }

    /// Asks every account for its usage now and again on the page's cadence
    /// (usage.rs): one account at a time with daylight between, the next
    /// round a minute on when every account has numbers, later when not.
    fn start_polling(&mut self, cx: &mut Context<Self>) {
        self._poll = None;
        self.cadence = Cadence::default();
        if self.accounts.is_empty() {
            return;
        }
        let gap = self.gap;
        self._poll = Some(cx.spawn(async move |this, cx| {
            loop {
                let Ok(todo) = this.update(cx, |this, _| this.accounts.iter().map(|a| (a.provider, a.path.clone())).collect::<Vec<_>>())
                else {
                    return;
                };
                let mut server_wait = 0;
                for (i, (provider, path)) in todo.iter().enumerate() {
                    if i > 0 {
                        cx.background_executor().timer(gap).await;
                    }
                    let command = if *provider == Provider::Codex { cmd::CODEX_LIMITS } else { cmd::ACCOUNT_LIMITS };
                    let Ok(answer) = this.update(cx, |this, _| this.host.call(command, json!({ "accountPath": path, "force": false })))
                    else {
                        return;
                    };
                    let round = answer.await.ok().and_then(|v| match provider {
                        Provider::Claude => serde_json::from_value(v).ok().map(Round::claude),
                        Provider::Codex => serde_json::from_value(v).ok().map(Round::codex),
                    });
                    let Ok(wait) = this.update(cx, |this, cx| this.take_usage(path, round, cx)) else { return };
                    server_wait = server_wait.max(wait);
                }
                let Ok(wait) = this.update(cx, |this, _| this.cadence.next(&this.accounts, server_wait)) else { return };
                cx.background_executor().timer(wait).await;
            }
        }));
    }

    /// One account's answer; the endpoint's own wait after a 429, in seconds.
    fn take_usage(&mut self, path: &str, round: Option<Round>, cx: &mut Context<Self>) -> u64 {
        let wait = round.as_ref().and_then(|r| r.rate_limited).unwrap_or(0);
        let before = self.reset_words();
        self.now_ms = (self.clock)();
        let changed = self.accounts.iter_mut().find(|a| a.path == path).is_some_and(|a| a.apply(round));
        // The countdowns are as old as the last answer: a changed reading, or
        // a minute rolled over, is a redraw; nothing else is.
        if changed || self.reset_words() != before {
            self.notify(cx);
        }
        if changed {
            self.push_usage(cx);
        }
        wait
    }

    fn reset_words(&self) -> Vec<String> {
        let now = self.now_ms;
        self.accounts.iter().flat_map(|a| a.windows.iter().map(move |w| usage::fmt_reset(w.reset_at.as_deref(), now))).collect()
    }

    /// Each account's usage to the core, as `luna_account_list` hands it to
    /// agents (the page's bridge): when it changed, and again for a new core.
    fn push_usage(&mut self, cx: &mut Context<Self>) {
        if !self.core_up(cx) || self.accounts.is_empty() {
            return;
        }
        let usage: serde_json::Map<String, Value> = self
            .accounts
            .iter()
            .map(|a| {
                let limits: Vec<Value> = if a.have_usage {
                    a.windows
                        .iter()
                        .map(|w| json!({ "label": w.label, "usedPct": (w.used * 100.).round() as i64, "resetsAt": w.reset_at }))
                        .collect()
                } else {
                    Vec::new()
                };
                let taken = a.fetched_at_ms.filter(|_| a.have_usage).map(|ms| usage::iso(ms as i64));
                (
                    format!("{}/{}", provider_key(a.provider), a.name),
                    json!({ "signedIn": a.signed_in, "plan": a.plan.clone().unwrap_or_else(|| "—".into()), "limits": limits, "takenAt": taken }),
                )
            })
            .collect();
        let told = Value::Object(usage);
        let key = told.to_string();
        if self.usage_told.as_ref() == Some(&key) {
            return;
        }
        self.usage_told = Some(key.clone());
        let answer = self.host.call(cmd::ACCOUNT_USAGE, json!({ "usage": told }));
        cx.spawn(async move |this, cx| {
            if answer.await.is_err() {
                // Tried again at the next change.
                let _ = this.update(cx, |this, _| {
                    if this.usage_told.as_ref() == Some(&key) {
                        this.usage_told = None;
                    }
                });
            }
        })
        .detach();
    }

    // ------------------------------------------------------------------ CLIs

    fn take_cli(&mut self, status: CliStatus, cx: &mut Context<Self>) {
        let Some(provider) = status.provider else { return };
        match self.clis.iter_mut().find(|c| c.provider == Some(provider)) {
            Some(old) if *old == status => return,
            Some(old) => *old = status,
            None => self.clis.push(status),
        }
        self.notify(cx);
    }

    /// A failed update pass's chip: check again and install what is newer.
    pub fn retry_cli(&mut self, provider: Provider, cx: &mut Context<Self>) {
        let answer = self.host.call(cmd::CLI_UPDATE_NOW, json!({ "provider": provider_key(provider) }));
        cx.background_spawn(async move {
            if let Err(e) = answer.await {
                log::warn!("cli_update_now: {e}");
            }
        })
        .detach();
    }

    // ----------------------------------------------------------------- events

    /// `power://state` and `cli://status`.
    pub fn on_event(&mut self, name: &str, payload: Value, cx: &mut Context<Self>) {
        match name {
            super::event::POWER_STATE => self.take_power(payload, cx),
            super::event::CLI_STATUS => match serde_json::from_value::<CliStatus>(payload) {
                Ok(status) => self.take_cli(status, cx),
                Err(e) => log::warn!("{name}: {e}"),
            },
            _ => {}
        }
    }

    /// `update://progress`, from the global the desktop's events set.
    fn on_progress(&mut self, cx: &mut Context<Self>) {
        let Some((got, total)) = cx.try_global::<UpdateProgress>().and_then(|p| p.0) else { return };
        if let Update::Downloading { got: g, total: t, .. } = &mut self.update
            && (*g, *t) != (got, total)
        {
            // The server's length, once told, stays.
            *g = got;
            *t = total.or(*t);
            self.notify(cx);
        }
    }

    // ------------------------------------------------------------------ power

    fn take_power(&mut self, v: Value, cx: &mut Context<Self>) {
        let state = match serde_json::from_value::<PowerState>(v) {
            Ok(s) => s,
            Err(e) => return log::warn!("power state: {e}"),
        };
        if state == self.power {
            return;
        }
        self.power = state;
        self.sync_tick(cx);
        self.tell_tray(cx);
        self.notify(cx);
    }

    /// The countdown's clock runs while a countdown does, once a second, and
    /// stops with it.
    fn sync_tick(&mut self, cx: &mut Context<Self>) {
        if self.power.countdown_ends_at_ms.is_none() {
            self._tick = None;
            self.ticking = false;
            return;
        }
        if self.ticking {
            return;
        }
        self.ticking = true;
        self._tick = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(COUNTDOWN_EVERY).await;
                let more = this.update(cx, |this, cx| {
                    if this.power.countdown_ends_at_ms.is_some() {
                        this.notify(cx);
                        true
                    } else {
                        this.ticking = false;
                        false
                    }
                });
                if !more.unwrap_or(false) {
                    return;
                }
            }
        }));
    }

    /// The tray's awake dot and who holds the PC: told when it changes, the
    /// plain icon the first time.
    fn tell_tray(&mut self, cx: &mut Context<Self>) {
        let held = power::tray_holders(&self.power, |id| self.chat_name(id, cx));
        if self.tray_told.as_ref() == Some(&held) {
            return;
        }
        if let Err(e) = self.host.send(desktop::cmd::TRAY_AWAKE, json!({ "holders": held })) {
            return log::warn!("tray_awake: {e}");
        }
        self.tray_told = Some(held);
    }

    fn power_call(&mut self, command: &'static str, args: Value, cx: &mut Context<Self>) {
        let answer = self.host.call(command, args);
        cx.spawn(async move |this, cx| match answer.await {
            Ok(v) => {
                let _ = this.update(cx, |this, cx| this.take_power(v, cx));
            }
            Err(e) => log::warn!("{command}: {e}"),
        })
        .detach();
    }

    /// The keep-awake switch.
    pub fn set_keep_awake(&mut self, on: bool, cx: &mut Context<Self>) {
        self.power_call(cmd::SET_KEEP_AWAKE, json!({ "on": on }), cx);
    }

    /// Arms the rule as the popover is set.
    pub fn arm_power_off(&mut self, cx: &mut Context<Self>) {
        let Arm { action, quiet_s, drain } = self.arm;
        self.popover = None;
        self.notify(cx);
        self.power_call(cmd::ARM_POWER_OFF, json!({ "action": action.name(), "quietS": quiet_s, "drain": drain }), cx);
    }

    /// Disarms the rule, and so cancels a countdown.
    pub fn disarm_power_off(&mut self, cx: &mut Context<Self>) {
        self.power_call(cmd::DISARM_POWER_OFF, json!({}), cx);
    }

    pub fn set_arm(&mut self, arm: Arm, cx: &mut Context<Self>) {
        if arm != self.arm {
            self.arm = arm;
            self.notify(cx);
        }
    }

    // --------------------------------------------------------------- popovers

    pub fn open_popover(&mut self, which: Popover, cx: &mut Context<Self>) {
        if which == Popover::Power
            && let Some(armed) = &self.power.armed
        {
            self.arm = Arm { action: armed.action, quiet_s: armed.quiet_s, drain: armed.drain };
        }
        self.popover = if self.popover == Some(which) { None } else { Some(which) };
        self.notify(cx);
    }

    /// Esc, or a click away: true when there was one to close.
    pub fn close_popover(&mut self, cx: &mut Context<Self>) -> bool {
        let open = self.popover.take().is_some();
        if open {
            self.notify(cx);
        }
        open
    }

    // ----------------------------------------------------------------- update

    /// Looks for a release. One asked for (`manual`) reports its failure on
    /// the chip; the one at startup is quiet about being offline.
    pub fn check(&mut self, manual: bool, cx: &mut Context<Self>) {
        // A second check on top of a download, or one with an install
        // pending a choice, would change the offer under it.
        if self.checking || self.update.busy() || matches!(self.update, Update::Unsaved { .. }) {
            return;
        }
        self.checking = true;
        let answer = self.host.call(desktop::cmd::UPDATE_CHECK, json!({}));
        cx.spawn(async move |this, cx| {
            let found = answer.await.and_then(|v| serde_json::from_value::<UpdateCheck>(v).map_err(|e| e.to_string()));
            let _ = this.update(cx, |this, cx| {
                this.checking = false;
                // What was asked of the update meanwhile stands.
                if this.update.busy() || matches!(this.update, Update::Unsaved { .. }) {
                    return;
                }
                match found {
                    Ok(UpdateCheck { offer: Some(offer), .. }) => this.update = Update::Available(offer),
                    Ok(_) => this.update = Update::Idle,
                    Err(why) => {
                        log::warn!("update check: {why}");
                        this.update = if manual { Update::Failed { offer: None, why } } else { Update::Idle };
                    }
                }
                this.notify(cx);
            });
        })
        .detach();
    }

    /// The chip's click: the confirmation for a release on offer, the choice
    /// for an install that could not save first, a new look for a failure.
    pub fn open_update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match &self.update {
            Update::Available(_) => {
                let seam = self.workspace.read(cx).seams.update_confirm.clone();
                match seam {
                    Some(seam) => seam(window, cx),
                    None => self.open_popover(Popover::Update, cx),
                }
            }
            Update::Unsaved { .. } => self.open_popover(Popover::Update, cx),
            Update::Failed { offer: Some(_), .. } => self.open_popover(Popover::Update, cx),
            Update::Failed { offer: None, .. } => self.check(true, cx),
            _ => {}
        }
    }

    /// The confirmation's yes: downloads the installer, stops the sessions
    /// if the release's core is not the running one, installs.
    pub fn install(&mut self, cx: &mut Context<Self>) {
        let offer = match &self.update {
            Update::Available(o) | Update::Failed { offer: Some(o), .. } => o.clone(),
            _ => return,
        };
        self.popover = None;
        self.update = Update::Downloading { offer: offer.clone(), got: 0, total: None };
        self.notify(cx);
        let download = self.host.call(desktop::cmd::UPDATE_DOWNLOAD, json!({}));
        cx.spawn(async move |this, cx| {
            let done = download.await;
            let _ = this.update(cx, |this, cx| match done {
                Ok(_) => this.stop_then_install(offer, cx),
                Err(why) => this.fail(offer, why, false, cx),
            });
        })
        .detach();
    }

    fn keeps_sessions(&self, offer: &desktop::Offer, cx: &App) -> bool {
        let link = self.core.read(cx);
        let running = link.link().core.as_ref().map(|c| c.fingerprint.as_str()).filter(|_| link.link().state == LinkState::Up);
        updates::keeps_sessions(offer, running)
    }

    /// Only once the installer is on disk, so a download that fails costs
    /// nothing, every session is asked to quit with its exit hooks, rather
    /// than going down with the old core mid-write.
    fn stop_then_install(&mut self, offer: desktop::Offer, cx: &mut Context<Self>) {
        let keep = self.keeps_sessions(&offer, cx);
        if keep {
            return self.run_install(offer, true, false, cx);
        }
        self.update = Update::Stopping(offer.clone());
        self.notify(cx);
        let stop = self.host.call(cmd::STOP_ALL_SESSIONS, json!({}));
        cx.spawn(async move |this, cx| {
            let done = stop.await;
            let _ = this.update(cx, |this, cx| match done {
                Ok(_) => this.run_install(offer, false, false, cx),
                Err(why) => this.fail(offer, why, true, cx),
            });
        })
        .detach();
    }

    /// The install itself. What the window holds back is sent first, then
    /// the host writes what it holds and starts the installer, after which
    /// Luna ends; where it cannot write first it refuses (`unsaved: `) and
    /// the person chooses.
    fn run_install(&mut self, offer: desktop::Offer, keep: bool, anyway: bool, cx: &mut Context<Self>) {
        self.workspace.update(cx, |w, cx| w.flush_now(cx));
        self.popover = None;
        self.update = Update::Installing(offer.clone());
        self.notify(cx);
        let answer = self.host.call(desktop::cmd::UPDATE_INSTALL, json!({ "reason": updates::reason(&offer, keep), "anyway": anyway }));
        cx.spawn(async move |this, cx| {
            let done = answer.await;
            let _ = this.update(cx, |this, cx| match done {
                // Luna is on its way out; where the window stays (not on
                // Windows), the chip keeps saying so.
                Ok(_) => {}
                Err(why) => match why.strip_prefix(UNSAVED) {
                    Some(rest) => {
                        this.update = Update::Unsaved { offer, why: rest.to_owned(), keep };
                        this.popover = Some(Popover::Update);
                        this.notify(cx);
                    }
                    None => this.fail(offer, why, !keep, cx),
                },
            });
        })
        .detach();
    }

    /// Retry: the same install, again.
    pub fn retry_install(&mut self, cx: &mut Context<Self>) {
        if let Update::Unsaved { offer, keep, .. } = self.update.clone() {
            self.run_install(offer, keep, false, cx);
        }
    }

    /// Install anyway: the person lets go of what could not be written.
    pub fn install_anyway(&mut self, cx: &mut Context<Self>) {
        if let Update::Unsaved { offer, keep, .. } = self.update.clone() {
            log::warn!("installing the update with what could not be written, as the person chose");
            self.run_install(offer, keep, true, cx);
        }
    }

    /// Cancel: the update is off, the sessions the stop ended resume.
    pub fn cancel_install(&mut self, cx: &mut Context<Self>) {
        if let Update::Unsaved { offer, keep, .. } = self.update.clone() {
            if !keep {
                self.call_off();
            }
            self.popover = None;
            self.update = Update::Available(offer);
            self.notify(cx);
        }
    }

    /// Tells the core the update is off, so it stops holding the chats' ties.
    fn call_off(&self) {
        // The call is made as it is asked; what it answers changes nothing.
        std::mem::drop(self.host.call(cmd::UPDATE_CALLED_OFF, json!({})));
    }

    fn fail(&mut self, offer: desktop::Offer, why: String, call_off: bool, cx: &mut Context<Self>) {
        log::warn!("update: {why}");
        if call_off {
            self.call_off();
        }
        self.update = Update::Failed { offer: Some(offer), why };
        self.notify(cx);
    }

    // ---------------------------------------------------------------- orphans

    /// Looks for sessions no chat reaches. Only the latest look may answer:
    /// one asked before a chat was adopted still counts its session as
    /// nobody's.
    pub fn scan(&mut self, cx: &mut Context<Self>) {
        if !self.core_up(cx) || !self.workspace.read(cx).loaded_yet() {
            return;
        }
        let known = self.known_chats(cx);
        self.scans += 1;
        let seq = self.scans;
        let answer = self.host.call(cmd::ORPHAN_SESSIONS, json!({ "known": known }));
        self.scanned_for = Some(known);
        cx.spawn(async move |this, cx| {
            let rows = answer.await.ok().and_then(|v| serde_json::from_value::<Vec<Orphan>>(v).ok()).unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                if seq == this.scans && rows != this.orphans {
                    this.orphans = rows;
                    if this.orphans.is_empty() {
                        this.kill_asked = false;
                    }
                    this.notify(cx);
                }
            });
        })
        .detach();
    }

    /// The line's Kill all: it asks first; the second click kills.
    pub fn kill_all(&mut self, cx: &mut Context<Self>) {
        if self.orphans.is_empty() {
            return;
        }
        if !self.kill_asked {
            self.kill_asked = true;
            return self.notify(cx);
        }
        self.kill_asked = false;
        let answers: Vec<_> = self.orphans.iter().map(|o| self.host.call(cmd::KILL_SESSION, json!({ "id": o.id }))).collect();
        self.notify(cx);
        cx.spawn(async move |this, cx| {
            for answer in answers {
                // One that beat us to exiting is the case the rescan handles.
                if let Err(e) = answer.await {
                    log::warn!("kill_session: {e}");
                }
            }
            let _ = this.update(cx, |this, cx| this.scan(cx));
        })
        .detach();
    }

    /// Keep: the line stops asking.
    pub fn keep_orphans(&mut self, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.kill_asked) {
            self.notify(cx);
        }
    }
}
