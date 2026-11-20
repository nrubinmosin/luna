//! The window's vital signs, as one model the bar and the orphan line draw:
//! what the chats are doing (counts), each account's usage, the Luna update,
//! the CLIs' downloads, the power rule, and the sessions no chat reaches.
//!
//! Nothing here ticks. What the host says arrives as events; the rest is
//! asked for on the slow schedules the page kept: the accounts' usage on the
//! page's cadence (usage.rs), the orphans every 30 seconds while the core is
//! up (one look at a time, given up after `SCAN_TIMEOUT`), the update once at
//! start. The one clock is the power countdown, one second at a time and
//! only while a countdown runs. A model notifies only when what is drawn
//! changed, so an idle window draws nothing.
//!
//! The power rule is the core's: while the core is not reachable the chip
//! says so and shows no countdown, and an answer or an event older than the
//! state shown (by core epoch and the state's revision) is dropped, so a
//! late answer cannot put back a countdown that was canceled. A Cancel that
//! fails stays on the chip, with Retry.

use super::dto::{CliStatus, Orphan, PowerAction, PowerState};
use super::updates::{self, UNSAVED, Update};
use super::usage::{self, Cadence, Round, Usage};
use super::{cmd, power};
use crate::desktop::{self, CORE_CHANGED, CoreId, UpdateCheck, UpdateProgress};
use crate::dto::LinkState;
use crate::host::Host;
use crate::models::CoreLink;
use crate::state::Status;
use crate::term::TermPane;
use crate::term::paste::Provider;
use crate::workspace::Workspace;
use gpui::{App, AppContext, Context, Entity, Task, WeakEntity};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

/// How often the sessions no chat reaches are looked for, as the page did.
pub const ORPHANS_EVERY: Duration = Duration::from_secs(30);
/// How long a look for them waits for its answer: past it, the look is given
/// up and the next one may go.
pub const SCAN_TIMEOUT: Duration = Duration::from_secs(20);
/// How long an update waits for the core to stop the sessions
/// (`stop_all_sessions`, each given `pty::GRACE`, 10 s, to quit): past it
/// the update is called off, the list thawed and the chip says why.
pub const STOP_LIMIT: Duration = Duration::from_secs(60);
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

/// What the update's confirmation tells the person of the sessions, and the
/// core it was worked out against (`Vitals::confirmation`): bar.rs draws
/// its words from it, and its button hands it back to `Vitals::install`,
/// which goes ahead only on what was shown, and holds the whole install to
/// it: the stop names this core and the sessions the person agreed to stop
/// (`stops`), and the host and the core refuse it beyond them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Confirmation {
    /// The core up as it was worked out (`Vitals::running_core`).
    pub core: Option<CoreId>,
    /// The release's core is that one's: the sessions keep running.
    pub keep: bool,
    /// The sessions running then, by id, sorted, as the core's board listed
    /// them (`PowerState::summary`) and the line on them counts them; None
    /// while the board is not known (no core up, or a new one not heard
    /// from yet): the line then promises nothing, and no install goes on it.
    pub running: Option<Vec<String>>,
}

impl Confirmation {
    /// Whether an install may go on it: it names a core and what runs there.
    pub fn known(&self) -> bool {
        self.core.is_some() && self.running.is_some()
    }

    /// How many sessions the line counts, None where it could not.
    pub fn live(&self) -> Option<usize> {
        self.running.as_ref().map(Vec::len)
    }

    /// The sessions the person agreed to stop, by id: none where the line
    /// said they keep running or none runs, else the ones it counted.
    pub fn stops(&self) -> Vec<String> {
        if self.keep { Vec::new() } else { self.running.clone().unwrap_or_default() }
    }
}

/// Why Retry or Install anyway asks again: the core is not the one the
/// sessions were stopped, or kept running, in (`Vitals::core_changed`); and
/// why an install whose download is through does, where another core is up
/// than the one its panel was drawn for (`Vitals::proceed`).
const CONFIRMED_FOR_ANOTHER_CORE: &str =
    "the core changed since this install was confirmed, so nothing is stopped or installed on that word: look again before installing";

/// Why an install whose download is through asks again where the core's
/// board lists a session its panel did not count among those to stop
/// (`Vitals::proceed`): one started since (a chat opened, a helper an agent
/// spawned), or any, where it said none stops. The core refuses the stop as
/// well, for one the window has not heard of yet (pty `stop_all_sessions`).
const SESSIONS_CHANGED: &str =
    "a session runs that this install's confirmation did not count among those it stops, so nothing is stopped or installed on that word: look again before installing";

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

/// The sessions Kill all was confirmed for, as the line showed them: each
/// one's id and process, so a session gone and another started under the
/// same id is not the one the person said yes to.
type Shown = Vec<(String, Option<u32>)>;

/// A Kill all's first click: what the line showed, and a number no other
/// confirmation has, so a look again that outlived its question (a scan
/// answered meanwhile ended it) is told from the one now asked.
#[derive(Clone, Debug)]
struct Confirm {
    id: u64,
    shown: Shown,
}

fn shown(rows: &[Orphan]) -> Shown {
    rows.iter().map(|o| (o.id.clone(), o.pid)).collect()
}

/// What a CLI's chip shows of its status (bar.rs): its phase, a download's
/// percent, and whether it is the first download.
fn cli_shown(s: &CliStatus) -> (String, Option<u64>, bool) {
    let percent = if s.phase == "downloading" { updates::percent(s.got, s.total) } else { None };
    (s.phase.clone(), percent, s.version.is_none())
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
    /// A state of the core's power rule has come since the core was last up:
    /// without one the chip says the rule is not known.
    power_known: bool,
    /// The revision of the state shown (`PowerState::revision`), in `epoch`.
    power_rev: u64,
    /// Why the last Cancel (a disarm) failed, until the rule is seen
    /// disarmed: the chip says so and offers Retry.
    cancel_failed: Option<String>,
    clis: Vec<CliStatus>,
    accounts: Vec<Usage>,
    /// The accounts the workspace listed, by provider, name and path.
    listed: Vec<(Provider, String, String)>,
    cadence: Cadence,
    update: Update,
    checking: bool,
    /// When a check last found this build the newest (epoch ms), for the
    /// Settings' Updates page.
    current_at: Option<u64>,
    /// The workspace has listed the accounts, even if there are none.
    accounts_loaded: bool,
    orphans: Vec<Orphan>,
    /// The chats the last look for orphans was told of.
    scanned_for: Option<Vec<String>>,
    /// A look for orphans is under way, and another was asked meanwhile.
    scanning: bool,
    rescan: bool,
    /// Kill all was clicked once: the sessions the line showed then, which
    /// the second click kills if they are still the ones there.
    confirm: Option<Confirm>,
    /// The id the next confirmation takes.
    confirms: u64,
    /// The number of the look for orphans that may take its answer: one
    /// asked before a Kill all's recheck began is let go of (`kill_all`).
    scan_id: u64,
    /// The second click's look again is under way.
    killing: bool,
    popover: Option<Popover>,
    arm: Arm,
    /// What the tray was last told of the awake hold (`tray_awake`'s
    /// holders): None before the first time.
    tray_told: Option<Option<String>>,
    /// The usage as the core was last sent it (`account_usage`).
    usage_told: Option<String>,
    /// The core's epoch the window last refreshed itself in.
    epoch: u64,
    ticking: bool,
    /// The panes open (models.rs), and those whose sessions an update's stop
    /// ended: started again if the update is called off.
    panes: Rc<RefCell<Vec<WeakEntity<TermPane>>>>,
    stopped_panes: Vec<WeakEntity<TermPane>>,
    /// The sessions were stopped for the update under way: calling it off
    /// tells the core, and starts them again.
    update_stopped: bool,
    /// The wait for the sessions' stop: dropped, the update is called off
    /// (`call_off_stop`, `STOP_LIMIT`).
    _stop: Option<Task<()>>,
    /// An update holds the core, from the sessions' stop (or the choice to
    /// keep them) to the installer's start: no restart of it is asked
    /// meanwhile (`core_held`), and the install names the core its last stop
    /// took (`stopped_core`: the one the stop's answer names, or the one
    /// kept), by its process and build; the host starts the installer only
    /// under that one (`cmd::UPDATE_INSTALL`), and none is named without it.
    core_held: bool,
    stopped_core: Option<CoreId>,
    /// The confirmation the update under way was clicked on, as it was
    /// drawn (`install`): every stop it makes, a Retry's and an Install
    /// anyway's too, is held to it.
    confirmed: Option<Confirmation>,

    _poll: Option<Task<()>>,
    _scan: Option<Task<()>>,
    _tick: Option<Task<()>>,
    /// Times the model told its views to draw, for the tests that hold the
    /// window still while idle.
    pub(crate) notifies: usize,
}

impl Vitals {
    pub fn new(
        host: Arc<dyn Host>,
        workspace: Entity<Workspace>,
        core: Entity<CoreLink>,
        panes: Rc<RefCell<Vec<WeakEntity<TermPane>>>>,
        cx: &mut Context<Self>,
    ) -> Vitals {
        cx.observe(&workspace, |this, _, cx| this.on_workspace(cx)).detach();
        cx.observe(&core, |this, _, cx| this.on_core(cx)).detach();
        cx.observe_global::<UpdateProgress>(|this, cx| this.on_progress(cx)).detach();
        // Hidden to the tray the orphan line is not seen: no look until it
        // is shown again, and one then.
        cx.observe_global::<desktop::Shown>(|this, cx| this.scan(cx)).detach();
        Vitals {
            host,
            workspace,
            core,
            clock: Rc::new(system_ms),
            now_ms: system_ms(),
            gap: usage::GAP,
            counts: Counts::default(),
            power: PowerState::default(),
            power_known: false,
            power_rev: 0,
            cancel_failed: None,
            clis: Vec::new(),
            accounts: Vec::new(),
            listed: Vec::new(),
            cadence: Cadence::default(),
            update: Update::Idle,
            checking: false,
            current_at: None,
            accounts_loaded: false,
            orphans: Vec::new(),
            scanned_for: None,
            scanning: false,
            rescan: false,
            confirm: None,
            confirms: 0,
            scan_id: 0,
            killing: false,
            popover: None,
            arm: Arm::default(),
            tray_told: None,
            usage_told: None,
            epoch: 0,
            ticking: false,
            panes,
            stopped_panes: Vec::new(),
            _stop: None,
            core_held: false,
            stopped_core: None,
            confirmed: None,
            update_stopped: false,
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
    /// update, the tray's plain icon, and what the core is asked once it is
    /// up (`on_core`).
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

    /// A check is under way.
    pub fn checking(&self) -> bool {
        self.checking
    }

    /// When a check last found this build the newest; None after one found
    /// a release, or before any came back.
    pub fn current_at(&self) -> Option<u64> {
        self.current_at
    }

    pub fn orphans(&self) -> &[Orphan] {
        &self.orphans
    }

    pub fn kill_asked(&self) -> bool {
        self.confirm.is_some()
    }

    /// A state of the power rule has come from the core that is up.
    pub fn power_known(&self) -> bool {
        self.power_known
    }

    /// Why the last Cancel failed, while the rule is still armed.
    pub fn cancel_failed(&self) -> Option<&str> {
        self.cancel_failed.as_deref()
    }

    /// The power chip as the bar draws it: the rule's own words, or that it
    /// is not known while the core is not reachable, and a failed Cancel.
    pub fn power_chip(&self) -> power::Chip {
        if !self.power_known {
            return power::Chip { kind: power::Kind::Unknown, text: "power: no core".into() };
        }
        let mut chip = power::chip(&self.power, (self.clock)());
        if self.cancel_failed.is_some() {
            chip.text += " · cancel failed";
        }
        chip
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

    /// The seconds a countdown has left, while one runs as far as the core
    /// last said.
    pub fn countdown_left(&self) -> Option<u64> {
        self.power.countdown_ends_at_ms.filter(|_| self.power_known).map(|ends| power::left_s(ends, (self.clock)()))
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
            // Nothing is looked for while it is not there, and nothing is
            // known to run.
            self._scan = None;
            let mut changed = false;
            if !self.orphans.is_empty() || self.confirm.is_some() {
                self.orphans.clear();
                self.confirm = None;
                changed = true;
            }
            // Nor what its power rule does: a countdown shown from before may
            // be over, or still running where no Cancel reaches it.
            if self.power_known {
                self.power_known = false;
                self.sync_tick(cx);
                if self.popover == Some(Popover::Power) {
                    self.popover = None;
                }
                changed = true;
            }
            if changed {
                self.notify(cx);
            }
            return;
        }
        let epoch = self.core.read(cx).epoch();
        if epoch != self.epoch {
            self.epoch = epoch;
            // A core that is new knows nothing the window told the old one,
            // and the power rule is its own, its states counted from 1.
            self.usage_told = None;
            self.push_usage(cx);
            self.power_rev = 0;
            // A Cancel that failed was for the old core's rule, and a yes to
            // Kill all for the old core's sessions.
            self.cancel_failed = None;
            if self.confirm.take().is_some() {
                self.notify(cx);
            }
            if self.power_known {
                self.power_known = false;
                self.sync_tick(cx);
                self.notify(cx);
            }
        } else if self._scan.is_some() {
            return;
        }
        // Up, in a new core or the same one found again: its power rule is
        // asked for, and the orphans looked for now and every `ORPHANS_EVERY`.
        let answer = self.host.call(cmd::POWER_STATE, json!({}));
        cx.spawn(async move |this, cx| match answer.await {
            Ok(v) => {
                let _ = this.update(cx, |this, cx| this.take_power(v, Some(epoch), cx));
            }
            Err(e) => log::warn!("power_state: {e}"),
        })
        .detach();
        self._scan = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(ORPHANS_EVERY).await;
                if this.update(cx, |this, cx| this.scan(cx)).is_err() {
                    return;
                }
            }
        }));
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
        // a minute rolled over, is a redraw; nothing else is, the time the
        // numbers were taken included.
        if changed || self.reset_words() != before {
            self.notify(cx);
        }
        // That time is the agents' to see: sent when it moved, as a reading
        // that changed is (`push_usage` sends only what is new).
        self.push_usage(cx);
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

    /// A CLI's status, kept whole; a redraw only when its chip would change:
    /// a download tells its bytes five times a second, and its chip moves a
    /// percent at a time.
    fn take_cli(&mut self, status: CliStatus, cx: &mut Context<Self>) {
        let Some(provider) = status.provider else { return };
        let shown = cli_shown(&status);
        let before = match self.clis.iter_mut().find(|c| c.provider == Some(provider)) {
            Some(old) => Some(cli_shown(&std::mem::replace(old, status))),
            None => {
                self.clis.push(status);
                None
            }
        };
        if before != Some(shown) {
            self.notify(cx);
        }
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
            super::event::POWER_STATE => {
                // One stamped with another core's `up` is a late state of a
                // core since gone: the new core's states count from 1 again,
                // so its revision alone would let it pass.
                if super::event::core_of(&payload).is_some_and(|c| Some(c) != self.core.read(cx).up_revision()) {
                    return;
                }
                self.take_power(payload, None, cx);
            }
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

    /// A state of the power rule: an event (`asked_in` None) or the answer
    /// to a call asked in core epoch `asked_in`. Taken only from the core up
    /// now and only if it is not older than the state shown: a reply that
    /// comes late, after a newer state, would put back what that one ended.
    /// False where it was not taken.
    fn take_power(&mut self, v: Value, asked_in: Option<u64>, cx: &mut Context<Self>) -> bool {
        if asked_in.is_some_and(|e| e != self.epoch) || !self.core_up(cx) {
            return false;
        }
        let mut state = match serde_json::from_value::<PowerState>(v) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("power state: {e}");
                return false;
            }
        };
        if state.revision < self.power_rev {
            return false;
        }
        self.power_rev = state.revision;
        // Kept apart, so an unchanged state with a newer number draws nothing.
        state.revision = 0;
        let mut changed = !self.power_known;
        self.power_known = true;
        if state.armed.is_none() && self.cancel_failed.take().is_some() {
            changed = true;
        }
        if state != self.power {
            self.power = state;
            changed = true;
        }
        if changed {
            self.sync_tick(cx);
            self.tell_tray(cx);
            self.notify(cx);
        }
        true
    }

    /// The countdown's clock runs while a countdown does, once a second, and
    /// stops with it.
    fn sync_tick(&mut self, cx: &mut Context<Self>) {
        if self.power.countdown_ends_at_ms.is_none() || !self.power_known {
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
                    if this.power.countdown_ends_at_ms.is_some() && this.power_known {
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
        let epoch = self.epoch;
        cx.spawn(async move |this, cx| {
            let done = answer.await;
            let _ = this.update(cx, |this, cx| match done {
                Ok(v) => {
                    this.take_power(v, Some(epoch), cx);
                }
                Err(e) => {
                    log::warn!("{command}: {e}");
                    // A Cancel that did not reach the core leaves the rule
                    // armed there: said on the chip, with Retry, until the
                    // rule is seen disarmed.
                    // Not one asked of a core since replaced or gone: that
                    // rule is not the one on the chip.
                    if command == cmd::DISARM_POWER_OFF && epoch == this.epoch && this.core_up(cx) {
                        this.cancel_failed = Some(e);
                        this.notify(cx);
                    }
                }
            });
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
        // A rule not known is not set from here: the panel would show, and
        // arm, against a state that may be gone.
        if which == Popover::Power && !self.power_known {
            return;
        }
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
                this.current_at = None;
                match found {
                    Ok(UpdateCheck { offer: Some(offer), .. }) => this.update = Update::Available(offer),
                    Ok(_) => {
                        this.update = Update::Idle;
                        this.current_at = Some(this.clock_ms());
                    }
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

    /// The chip's click, and the Settings' Update...: the confirmation for a
    /// release on offer, the choice for an install that could not save first,
    /// a new look for a failure. The update panel is the one confirmation
    /// every install of Luna's goes through, so each takes the same steps
    /// (`proceed`).
    pub fn open_update(&mut self, cx: &mut Context<Self>) {
        match &self.update {
            Update::Available(_) | Update::Unsaved { .. } | Update::Failed { offer: Some(_), .. } | Update::Stopping(_) => {
                self.open_popover(Popover::Update, cx)
            }
            Update::Failed { offer: None, .. } => self.check(true, cx),
            _ => {}
        }
    }

    /// The confirmation's yes: downloads the installer, then installs
    /// (`proceed`). Not while a check is under way: its answer may put
    /// another offer up, which the host would then refuse this one for.
    /// `seen` is what the confirmation said (bar.rs `update_panel`), and the
    /// install goes only on that: where it could not say what runs (no core
    /// up, or its board not heard yet), or the core up now is another, or
    /// what the install means for the sessions has changed since it was
    /// drawn, nothing starts, and the panel, drawn again, says what an
    /// install would do now.
    pub fn install(&mut self, seen: &Confirmation, cx: &mut Context<Self>) {
        let offer = match &self.update {
            Update::Available(o) | Update::Failed { offer: Some(o), .. } => o.clone(),
            _ => return,
        };
        if self.checking {
            return log::info!("update: not installed while a check is under way");
        }
        if !seen.known() {
            log::info!("update: not installed on a confirmation that could not say which sessions run");
            return self.notify(cx);
        }
        if self.confirmation(&offer, cx) != *seen {
            log::info!("update: not installed on a confirmation drawn for another core or another word on the sessions; it is shown again");
            return self.notify(cx);
        }
        self.popover = None;
        self.confirmed = Some(seen.clone());
        let confirmed = seen.clone();
        self.update = Update::Downloading { offer: offer.clone(), got: 0, total: None };
        self.notify(cx);
        let download = self.host.call(desktop::cmd::UPDATE_DOWNLOAD, json!({ "generation": offer.generation }));
        cx.spawn(async move |this, cx| {
            let done = download.await;
            let _ = this.update(cx, |this, cx| match done {
                Ok(got) => this.downloaded(offer, confirmed, &got, cx),
                Err(why) => this.fail(offer, why, cx),
            });
        })
        .detach();
    }

    /// What the confirmation for `offer` says now: the core up, whether the
    /// release keeps its sessions running (by the core fingerprint the offer
    /// names: `latest.json`'s until the download, the signed one after it),
    /// and which sessions run, where the core's board is known.
    pub fn confirmation(&self, offer: &desktop::Offer, cx: &App) -> Confirmation {
        Confirmation { core: self.running_core(cx), keep: self.keeps_sessions(offer, cx), running: self.running_sessions() }
    }

    /// The sessions on the core's board, by id, sorted; None where the board
    /// is not known (`power_known`): what was there before the core went, or
    /// before a new one was heard from, is not what runs.
    fn running_sessions(&self) -> Option<Vec<String>> {
        self.power_known.then(|| {
            let mut ids: Vec<String> = self.power.summary.sessions.iter().map(|s| s.id.clone()).collect();
            ids.sort();
            ids.dedup();
            ids
        })
    }

    /// The download checked out: the install goes on with the core
    /// fingerprint the setup's signature names (`cmd::UPDATE_DOWNLOAD`), in
    /// place of the offer's, which `latest.json` gave unsigned, and holds to
    /// what the confirmation said (`proceed`).
    fn downloaded(&mut self, offer: desktop::Offer, confirmed: Confirmation, got: &Value, cx: &mut Context<Self>) {
        let signed = got.get("coreFingerprint").and_then(Value::as_str).map(str::to_owned);
        self.proceed(desktop::Offer { core_fingerprint: signed, ..offer }, false, confirmed, cx)
    }

    /// The core up now, as an update tells it from another (`CoreId`): by
    /// its process, not the window's epoch, so the same core found again
    /// after it let the window go (corelink.rs `rejoin`) is still the one.
    fn running_core(&self, cx: &App) -> Option<CoreId> {
        let link = self.core.read(cx);
        link.link().core.as_ref().filter(|_| link.link().state == LinkState::Up).map(CoreId::of)
    }

    /// An update holds the core (`core_held`): its restart is not offered.
    pub fn core_held(&self) -> bool {
        self.core_held
    }

    fn keeps_sessions(&self, offer: &desktop::Offer, cx: &App) -> bool {
        let link = self.core.read(cx);
        let running = link.link().core.as_ref().map(|c| c.fingerprint.as_str()).filter(|_| link.link().state == LinkState::Up);
        updates::keeps_sessions(offer, running)
    }

    /// The install, once the installer is on disk (so a download that fails
    /// costs nothing), and again for Retry and Install anyway. The chat list
    /// takes no change from here until the installer has it or the update is
    /// off (`Workspace::freeze`). Whether the sessions must stop is decided
    /// each time, against the core up now: if the release's core is not the
    /// running one, the sessions are asked to quit with their exit hooks,
    /// rather than go down with the old core mid-write.
    ///
    /// All of it is held to what the person confirmed (`confirmed`), as it
    /// was drawn: nothing goes in another core than the one it named, and no
    /// session is stopped that it did not count among those to stop (none,
    /// where it said they keep running or none runs). Where the window sees
    /// either already, the update fails and the panel, drawn again, says
    /// what an install would do now. Beyond what the window sees, the stop
    /// carries both: the host sends it to the core named alone, waiting for
    /// it where it is being found again, and the core, its start gate sealed,
    /// refuses it where it would stop a session not named (pty
    /// `stop_all_sessions`), before it stops any.
    fn proceed(&mut self, offer: desktop::Offer, anyway: bool, confirmed: Confirmation, cx: &mut Context<Self>) {
        if self.running_core(cx).is_some_and(|up| Some(up) != confirmed.core) {
            return self.fail(offer, CONFIRMED_FOR_ANOTHER_CORE.to_owned(), cx);
        }
        let keep = self.keeps_sessions(&offer, cx);
        let stops = confirmed.stops();
        // The board, where it is known, is the core's up now: the one named.
        let unconfirmed = self.running_sessions().is_some_and(|running| running.iter().any(|id| !stops.contains(id)));
        if !keep && unconfirmed {
            let why = if confirmed.keep {
                "the installer's signature names another core than the running one, so the sessions would stop, which the confirmation said they would not: look again before installing"
            } else {
                SESSIONS_CHANGED
            };
            return self.fail(offer, why.to_owned(), cx);
        }
        self.popover = None;
        self.workspace.update(cx, |w, cx| w.freeze(cx));
        // The core whose sessions this pass stops (or keeps): the one the
        // confirmation named, which the stop's answer names too, the host
        // having sent it there alone.
        self.stopped_core = None;
        if !std::mem::replace(&mut self.core_held, true) {
            self.notify(cx);
        }
        if keep {
            self.stopped_core = confirmed.core;
            return self.save_then_install(offer, anyway, cx);
        }
        self.update = Update::Stopping(offer.clone());
        self.notify(cx);
        if !self.update_stopped {
            self.update_stopped = true;
            // The panes whose sessions this ends, to start again if the
            // update is called off.
            self.stopped_panes =
                self.panes.borrow().iter().filter_map(WeakEntity::upgrade).filter(|p| !p.read(cx).ended()).map(|p| p.downgrade()).collect();
        }
        // Bounded, and called off by dropping it (`call_off_stop`): a core
        // that never answers would leave the list frozen.
        let stop = self.host.call(cmd::STOP_ALL_SESSIONS, json!({ "core": confirmed.core, "confirmed": stops }));
        self._stop = Some(cx.spawn(async move |this, cx| {
            let limit = cx.background_executor().timer(STOP_LIMIT);
            let done = match futures::future::select(stop, limit).await {
                futures::future::Either::Left((done, _)) => done,
                futures::future::Either::Right(_) => {
                    Err(format!("the core did not stop the sessions within {} s; the update is called off", STOP_LIMIT.as_secs()))
                }
            };
            let _ = this.update(cx, |this, cx| {
                this._stop = None;
                match done {
                    Ok(stopped) => {
                        this.stopped_core = stopped.get("core").and_then(|c| serde_json::from_value(c.clone()).ok());
                        this.save_then_install(offer, anyway, cx)
                    }
                    Err(why) => this.fail(offer, why, cx),
                }
            });
        }));
    }

    /// The update panel's Call off while the sessions are being stopped: the
    /// stop's answer is no longer waited for, the sessions it ended start
    /// again and the list takes changes again (`resume`); the release stays
    /// on offer.
    pub fn call_off_stop(&mut self, cx: &mut Context<Self>) {
        if let Update::Stopping(offer) = self.update.clone() {
            self._stop = None;
            self.resume(cx);
            self.popover = None;
            self.update = Update::Available(offer);
            self.notify(cx);
        }
    }

    /// The chat list written and the write acknowledged (the quit's rule:
    /// the state the host says it wrote is the latest sent), then the host
    /// asked to write what it holds and start the installer, after which
    /// Luna ends. Where either cannot write, the person chooses.
    fn save_then_install(&mut self, offer: desktop::Offer, anyway: bool, cx: &mut Context<Self>) {
        self.update = Update::Installing(offer.clone());
        self.notify(cx);
        let me = cx.weak_entity();
        let workspace = self.workspace.clone();
        // From outside this model's update: the workspace answers at once
        // where it has nothing to write.
        cx.spawn(async move |_, cx| {
            workspace.update(cx, |w, cx| {
                w.save_for_install(
                    move |saved, cx| {
                        let _ = me.update(cx, |this, cx| this.saved(offer, anyway, saved, cx));
                    },
                    cx,
                )
            });
        })
        .detach();
    }

    fn saved(&mut self, offer: desktop::Offer, anyway: bool, saved: Result<(), String>, cx: &mut Context<Self>) {
        if !matches!(self.update, Update::Installing(_)) {
            return;
        }
        match saved {
            Err(why) if !anyway => return self.unsaved(offer, why, cx),
            Err(why) => log::warn!("installing the update with the chat list not saved ({why}), as the person chose"),
            Ok(()) => {}
        }
        // The installer replaces the files under the core the sessions were
        // stopped (or kept) in; another one started meanwhile would run on
        // under it. The install names that core, and the host decides as the
        // installer is about to start: what the window knows of the link
        // lags behind it (`starting` still for the core the stop waited for
        // and stopped them in, say), so it is not looked at again here.
        let Some(core) = self.stopped_core.clone() else {
            // Called off from outside the workspace's update this is told in:
            // the call off thaws the list.
            cx.spawn(async move |this, cx| {
                let _ = this.update(cx, |this, cx| this.fail(offer, CORE_CHANGED.into(), cx));
            })
            .detach();
            return;
        };
        let keep = !self.update_stopped;
        let answer = self.host.call(
            desktop::cmd::UPDATE_INSTALL,
            json!({ "generation": offer.generation, "reason": updates::reason(&offer, keep), "anyway": anyway, "core": core, "keep": keep }),
        );
        cx.spawn(async move |this, cx| {
            let done = answer.await;
            let _ = this.update(cx, |this, cx| match done {
                // Luna is on its way out; where the window stays (not on
                // Windows), the chip keeps saying so, and the list stays
                // still.
                Ok(_) => {}
                Err(why) => match why.strip_prefix(UNSAVED) {
                    Some(rest) => this.unsaved(offer, rest.to_owned(), cx),
                    None => this.fail(offer, why, cx),
                },
            });
        })
        .detach();
    }

    /// What Luna holds could not be written first: the choice is put in front
    /// of the person, the list still frozen meanwhile.
    fn unsaved(&mut self, offer: desktop::Offer, why: String, cx: &mut Context<Self>) {
        self.update = Update::Unsaved { offer, why, keep: !self.update_stopped };
        self.popover = Some(Popover::Update);
        self.notify(cx);
    }

    /// Retry: the install again, looked at anew (the core up now, the
    /// sessions it means stopping), refusing to lose anything. Only over the
    /// core the sessions were stopped or kept in (`core_changed`).
    pub fn retry_install(&mut self, cx: &mut Context<Self>) {
        if let Update::Unsaved { offer, .. } = self.update.clone() {
            match self.confirmed.clone().filter(|_| !self.core_changed(cx)) {
                Some(confirmed) => self.proceed(offer, false, confirmed, cx),
                None => self.fail(offer, CONFIRMED_FOR_ANOTHER_CORE.to_owned(), cx),
            }
        }
    }

    /// Install anyway: the person lets go of what could not be written. Only
    /// over the core the sessions were stopped or kept in (`core_changed`).
    pub fn install_anyway(&mut self, cx: &mut Context<Self>) {
        if let Update::Unsaved { offer, .. } = self.update.clone() {
            match self.confirmed.clone().filter(|_| !self.core_changed(cx)) {
                Some(confirmed) => {
                    log::warn!("installing the update with what could not be written, as the person chose");
                    self.proceed(offer, true, confirmed, cx)
                }
                None => self.fail(offer, CONFIRMED_FOR_ANOTHER_CORE.to_owned(), cx),
            }
        }
    }

    /// Whether the core up now is not the one this install stopped the
    /// sessions in, or kept them running in (`stopped_core`). The choice put
    /// before the person after an install that could not write first says
    /// what was done in that core; in another, Retry or Install anyway would
    /// stop sessions nobody agreed to stop, or keep ones nobody was told of,
    /// so they ask again instead (`CONFIRMED_FOR_ANOTHER_CORE`): the update
    /// is called off, the sessions stopped for it start again, and the panel
    /// shows the confirmation for the core up now.
    fn core_changed(&self, cx: &App) -> bool {
        self.stopped_core.is_none() || self.running_core(cx) != self.stopped_core
    }

    /// Cancel: the update is off, and the sessions its stop ended start
    /// again (`resume`).
    pub fn cancel_install(&mut self, cx: &mut Context<Self>) {
        if let Update::Unsaved { offer, .. } = self.update.clone() {
            self.resume(cx);
            self.popover = None;
            self.update = Update::Available(offer);
            self.notify(cx);
        }
    }

    /// The update is off. Where its sessions were stopped, the core is told,
    /// so it stops holding the chats' ties, and each pane whose session the
    /// stop ended starts it again, resumed from its transcript as after a
    /// core restart. The chat list takes changes again.
    fn resume(&mut self, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.update_stopped) {
            // The call is made as it is asked; what it answers changes nothing.
            drop(self.host.call(cmd::UPDATE_CALLED_OFF, json!({})));
            // Every pane the stop asked to end, its exit come or not: the
            // stop can return before the exits do. A late exit of the
            // session before is told from the new one's by its generation
            // (`TermPane::on_exit`).
            for pane in std::mem::take(&mut self.stopped_panes).iter().filter_map(WeakEntity::upgrade) {
                pane.update(cx, |p, cx| p.start_over(cx));
            }
        }
        self.core_held = false;
        self.stopped_core = None;
        self.confirmed = None;
        self.workspace.update(cx, |w, cx| w.thaw(cx));
    }

    /// The update failed: called off. Where the host refused the offer as no
    /// longer the one on offer (a check put another up), or the core changed
    /// under it, it is let go, so the next click checks again rather than
    /// ask for it once more.
    fn fail(&mut self, offer: desktop::Offer, why: String, cx: &mut Context<Self>) {
        log::warn!("update: {why}");
        self.resume(cx);
        let offer = (why != desktop::OFFER_CHANGED && why != CORE_CHANGED).then_some(offer);
        self.update = Update::Failed { offer, why };
        self.notify(cx);
    }

    // ---------------------------------------------------------------- orphans

    /// Looks for sessions no chat reaches, one look at a time, each given up
    /// after `SCAN_TIMEOUT`. A look asked while one is under way goes once
    /// that one is done, and that one's answer is let go of: it was asked
    /// before a chat was adopted, say, and still counts its session as
    /// nobody's. A look that fails changes nothing.
    pub fn scan(&mut self, cx: &mut Context<Self>) {
        let hidden = cx.try_global::<desktop::Shown>().is_some_and(|s| s.hidden);
        if hidden || !self.core_up(cx) || !self.workspace.read(cx).loaded_yet() {
            return;
        }
        // Nor beside a Kill all's recheck: two looks at once could end the
        // question it is checking, or put back sessions it is killing.
        if self.scanning || self.killing {
            self.rescan = true;
            return;
        }
        self.scanning = true;
        self.rescan = false;
        self.scan_id += 1;
        let (id, epoch) = (self.scan_id, self.epoch);
        let known = self.known_chats(cx);
        let answer = self.ask_orphans(&known, cx);
        self.scanned_for = Some(known);
        cx.spawn(async move |this, cx| {
            let rows = answer.await;
            let _ = this.update(cx, |this, cx| {
                // Canceled by a Kill all's recheck, which has the line now.
                if id != this.scan_id {
                    return;
                }
                this.scanning = false;
                if this.rescan {
                    return this.scan(cx);
                }
                if epoch != this.epoch || !this.core_up(cx) {
                    return;
                }
                match rows {
                    Ok(rows) => this.take_orphans(rows, cx),
                    Err(e) => log::warn!("orphan_sessions: {e}"),
                }
            });
        })
        .detach();
    }

    /// `orphan_sessions` for the chats `known`, given up after `SCAN_TIMEOUT`.
    fn ask_orphans(&self, known: &[String], cx: &mut Context<Self>) -> impl Future<Output = Result<Vec<Orphan>, String>> + use<> {
        let answer = self.host.call(cmd::ORPHAN_SESSIONS, json!({ "known": known }));
        let timeout = cx.background_executor().timer(SCAN_TIMEOUT);
        async move {
            match futures::future::select(answer, timeout).await {
                futures::future::Either::Left((answer, _)) => {
                    answer.and_then(|v| serde_json::from_value::<Vec<Orphan>>(v).map_err(|e| e.to_string()))
                }
                futures::future::Either::Right(_) => Err(format!("no answer in {} s", SCAN_TIMEOUT.as_secs())),
            }
        }
    }

    /// The orphans as a look found them. A Kill all asked for sessions that
    /// are no longer the ones there is asked again.
    fn take_orphans(&mut self, rows: Vec<Orphan>, cx: &mut Context<Self>) {
        if rows == self.orphans {
            return;
        }
        if self.confirm.as_ref().is_some_and(|c| c.shown != shown(&rows)) {
            self.confirm = None;
        }
        self.orphans = rows;
        self.notify(cx);
    }

    /// The line's Kill all: it asks first, for the sessions shown; the
    /// second click looks again and kills those sessions only if they are
    /// still the ones there and still nobody's. A look under way is let go
    /// of (its answer is not taken) and none starts beside the recheck, so
    /// the recheck is the one look that speaks for the line.
    pub fn kill_all(&mut self, cx: &mut Context<Self>) {
        if self.orphans.is_empty() || self.killing {
            return;
        }
        let Some(Confirm { id, shown: confirmed }) = self.confirm.clone() else {
            self.confirms += 1;
            self.confirm = Some(Confirm { id: self.confirms, shown: shown(&self.orphans) });
            return self.notify(cx);
        };
        self.killing = true;
        self.scan_id += 1;
        self.scanning = false;
        self.rescan = false;
        let known = self.known_chats(cx);
        let answer = self.ask_orphans(&known, cx);
        cx.spawn(async move |this, cx| {
            let rows = answer.await;
            let _ = this.update(cx, |this, cx| this.kill_confirmed(id, confirmed, rows, cx));
        })
        .detach();
    }

    /// The recheck's answer for confirmation `id`. Looks that came while it
    /// was under way waited (`scan`) and go now.
    fn kill_confirmed(&mut self, id: u64, confirmed: Shown, rows: Result<Vec<Orphan>, String>, cx: &mut Context<Self>) {
        self.killing = false;
        self.settle_kill(id, confirmed, rows, cx);
        if std::mem::take(&mut self.rescan) {
            self.scan(cx);
        }
    }

    fn settle_kill(&mut self, id: u64, confirmed: Shown, rows: Result<Vec<Orphan>, String>, cx: &mut Context<Self>) {
        // The question is gone (the core went, say): the person's yes was
        // to a question that no longer stands.
        if self.confirm.as_ref().map(|c| c.id) != Some(id) {
            return;
        }
        let rows = match rows {
            Ok(rows) => rows,
            // Not known to be the same sessions: none is killed, and the
            // line asks again.
            Err(e) => {
                log::warn!("orphan_sessions before a kill: {e}");
                self.confirm = None;
                return self.notify(cx);
            }
        };
        if shown(&rows) != confirmed {
            // Another set than the person said yes to: shown, and asked again.
            self.confirm = None;
            self.orphans = rows;
            return self.notify(cx);
        }
        // The core kills by id and cannot tell an orphan from a chat's
        // session: one a chat took over since the look is left alone.
        let known = self.known_chats(cx);
        let answers: Vec<_> = confirmed
            .iter()
            .filter(|(id, _)| known.binary_search(id).is_err())
            .map(|(id, _)| self.host.call(cmd::KILL_SESSION, json!({ "id": id })))
            .collect();
        self.confirm = None;
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
        if self.confirm.take().is_some() {
            self.notify(cx);
        }
    }
}
