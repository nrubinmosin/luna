//! What outlives a core. A background helper's ties go with its record
//! (background/); what the person's chats are owed and held, the spawns under
//! way and the lines still to be told go into `chat-ties.json`, and the next
//! core takes both up.

use crate::agents::held::{GoingIn, Held};
use crate::agents::owed::Owed;
use crate::agents::read::transcripts;
use crate::agents::registry::{account_name, is_dev, label, reg, register, without_retired, Known, Registry, CODEX_TRUST};
use crate::agents::spawn::SpawnTaken;
use crate::agents::teardown::Auto;
use crate::provider::Provider;
use crate::throttle::now_ms;
use serde::{Deserialize, Serialize};
use std::sync::{Mutex, OnceLock};

/// How long into a core's life a chat the last core knew (`restore_chats`) is
/// given to start again before whoever is owed by it hears it has not: the
/// window starts the ones in panes within seconds, the rest when opened.
pub(super) const SESSION_RETURN_MS: u64 = 60_000;

/// What the agent layer holds of a background helper, saved with it
/// (background/persist.rs `save`) so a core started later takes it up where this one
/// left it: whose it is, what it is called, and what its callers are owed. The same
/// of a chat of the person's that is owed something, in `chat-ties.json`. A file an
/// earlier Luna wrote may hold more (a visible helper's opening, the worktrees
/// it made): what is not read here is let go.
#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Ties {
    pub parent: Option<String>,
    pub name: Option<String>,
    pub settings: serde_json::Value,
    pub tools: bool,
    pub auto_delete: bool,
    /// See `Known::orphan`.
    #[serde(default)]
    pub orphan: bool,
    /// (caller, baseline, cursor) of every answer owed by it (see `Owed`).
    pub owed: Vec<(String, u32, u64)>,
    /// (caller, turns ended) each caller was last shown of it (see `seen`).
    pub seen: Vec<(String, u32)>,
    /// The callers among `owed` whose turn went with a core (`Owed::cut_short`):
    /// the count alone reads as an answer once that turn is counted as ended.
    #[serde(default)]
    pub cut_short: Vec<String>,
    /// The callers among `owed` told it is not done (`Owed::told_pending`): the
    /// count alone reads as nothing having happened, and what it had running goes
    /// with the core.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub told_pending: Vec<String>,
    /// See `Registry::tasks`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
}

/// One helper's ties, as a save keeps them (`ties_of_each`).
#[cfg(test)]
pub(super) fn ties_of(id: &str) -> Option<Ties> {
    ties_in(&reg().lock().unwrap_or_else(|e| e.into_inner()), id)
}

/// Read all helper ties for a save in one registry hold.
pub fn ties_of_each(ids: &[&str]) -> Vec<Option<Ties>> {
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    ids.iter().map(|id| ties_in(&r, id)).collect()
}

pub(super) fn ties_in(r: &Registry, id: &str) -> Option<Ties> {
    let k = r.known.get(id)?;
    Some(Ties {
        parent: k.parent.clone(),
        name: r.names.get(id).cloned(),
        settings: k.settings.clone(),
        tools: k.tools,
        auto_delete: r.auto.contains_key(id),
        orphan: k.orphan,
        owed: r.owed.iter().filter(|((_, s), _)| s == id).map(|((c, _), o)| (c.clone(), o.baseline, o.cursor)).collect(),
        seen: r.seen.iter().filter(|((_, s), _)| s == id).map(|((c, _), n)| (c.clone(), *n)).collect(),
        cut_short: r.owed.iter().filter(|((_, s), o)| s == id && o.cut_short).map(|((c, _), _)| c.clone()).collect(),
        told_pending: r.owed.iter().filter(|((_, s), o)| s == id && o.told_pending).map(|((c, _), _)| c.clone()).collect(),
        task: r.tasks.get(id).cloned(),
    })
}

/// An answer a caller had been told was not done yet, as a core that saved it
/// is followed by the next: that turn was over, and what it had left running
/// went with the core. The same for a background helper and a chat; None for
/// any other answer owed.
fn told_job_went(told_pending: &[String], caller: &str, baseline: u32, cursor: u64) -> Option<Owed> {
    told_pending.iter().any(|c| c == caller).then(|| Owed { job_went: true, ..Owed::pending(baseline, cursor) })
}

/// A helper a previous core saved is known again, as it was. Returns the
/// token for its MCP config, a fresh one: the old core's went with it.
pub fn tie_back(id: &str, provider: Provider, folder: &str, account_path: &str, ties: Ties) -> Option<String> {
    let token = register(id, provider, folder, account_path, ties.parent.as_deref(), ties.tools, ties.settings);
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(name) = ties.name {
        r.names.insert(id.to_string(), name);
    }
    if let Some(k) = r.known.get_mut(id) {
        k.orphan = ties.orphan;
    }
    // Its idle time counts afresh: nobody has had the chance to look at it.
    if ties.auto_delete {
        r.auto.insert(id.to_string(), Auto::default());
    }
    for (caller, baseline, cursor) in ties.owed {
        let o = told_job_went(&ties.told_pending, &caller, baseline, cursor).unwrap_or_else(|| Owed::from(baseline, cursor));
        r.owed.insert((caller, id.to_string()), o);
    }
    for (caller, n) in ties.seen {
        r.seen.insert((caller, id.to_string()), n);
    }
    if let Some(task) = ties.task {
        r.tasks.insert(id.to_string(), task);
    }
    token
}

/// A chat of the person's, as `<data>/chat-ties.json` keeps it for the core
/// started after this one: what its callers are owed by it. The window starts it
/// again, and `register` takes it from there.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SavedChat {
    pub(super) id: String,
    pub(super) provider: Provider,
    pub(super) folder: String,
    pub(super) account_path: String,
    /// See `activity::turns_counted`.
    pub(super) turns_ended: u32,
    pub(super) transcript: Option<std::path::PathBuf>,
    /// Its turn was running as this was written.
    #[serde(default)]
    pub(super) mid_turn: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) codex_trust: Option<luna_window::dto::ProjectTrust>,
    pub(super) ties: Ties,
}

/// Text Luna told its sender it would type in (`Held`).
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SavedSend {
    pub(super) caller: String,
    pub(super) id: String,
    pub(super) text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) going_in: Option<GoingIn>,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(super) struct SavedChats {
    pub(super) chats: Vec<SavedChat>,
    pub(super) sends: Vec<SavedSend>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) spawns: Vec<SpawnTaken>,
    /// Lines still to be told, by caller (`Registry::news`): a helper the person
    /// took over is told of once, whatever comes between.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) news: Vec<(String, String)>,
    /// Written by a core with no handoff (0.20 on): nothing in it is 0.19's
    /// handoff's to sort out (`of_retired_handoff`).
    #[serde(default)]
    pub(super) no_handoff: bool,
}

fn chat_ties_file() -> std::path::PathBuf {
    crate::paths::data_dir().join("chat-ties.json")
}

/// Set once `restore_chats` has read the file: an empty registry before it is
/// not to be written over what the last core left.
static CHATS_READ: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// What the last write says stands, for the core after this one. Set as every
/// session is stopped (a core leaving, an update): a turn asked to quit is a turn
/// ended to its record, Codex writing an abort for it that counts as its end, and
/// written down that would have the next core take it for one that finished.
pub(super) static CHATS_FROZEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// What `save_chats` last wrote, and the lock its saves take in turn
/// (`save_in_order`).
static CHATS_LAST: Mutex<Vec<u8>> = Mutex::new(Vec::new());

/// Why every session is being stopped (`freeze_chats`). Each stop holds the
/// freeze until its caller knows the process goes on after all
/// (`thaw_chats`), or the process ends with it: a session started or still
/// running meanwhile (a pane asking for its chat again, a helper's turn) is
/// no sign of either, and a save then would write what the stopping did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// The core leaving (`coreproc::leave`): never called off.
    Leaving,
    /// The PC shut down or restarted at Luna's word: called off where the OS
    /// is seen to take it back and the chats start over (power/machine.rs).
    PowerOff,
    /// An update about to be installed (`pty::stop_all_sessions`): called off
    /// where the install fails (`pty::update_called_off`), where the stop
    /// panics, or where a window of this core's own build comes while the one
    /// that asked for the stop is gone (`window_came`): the install never came,
    /// as it would have brought a window of another build. Nothing lifts it by
    /// the clock: an installer may wait on a UAC prompt for as long as nobody
    /// answers. Not called off while the sessions are still being stopped for
    /// it (`UpdateStopping`): a call-off then takes effect as the stop
    /// returns. Kept on disk while it stands (`UPDATE_STOP_FILE`), so a core
    /// of the same build started meanwhile starts with it
    /// (`restore_update_stop`).
    Update,
}

impl Stop {
    fn bit(self) -> u8 {
        match self {
            Stop::Leaving => 1,
            Stop::PowerOff => 2,
            Stop::Update => 4,
        }
    }
}

/// A window process, by pid and, where the OS says, when it was created, so
/// that a pid given out again is not taken for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Window {
    pub pid: u32,
    pub created_ms: Option<u64>,
}

impl Window {
    /// The process `pid` as the OS has it now.
    pub fn of(pid: u32) -> Window {
        Window { pid, created_ms: crate::procs::created_ms(pid) }
    }

    fn gone(&self) -> bool {
        !crate::procs::alive(self.pid) || self.created_ms.is_some_and(|at| crate::procs::created_ms(self.pid) != Some(at))
    }
}

/// The stops under way; `CHATS_FROZEN` is set and cleared under their lock,
/// so a stop called off never lifts another's freeze, and a stop said twice
/// is one.
struct Stops {
    /// A `Stop::bit` each.
    bits: u8,
    /// How many stops of every session for an update have not returned yet
    /// (`UpdateStopping`).
    updating: usize,
    /// The update called off while they had not (`call_off_update`): it is
    /// called off as the last of them returns.
    called_off: bool,
    /// The window whose stop froze the chats for the update
    /// (`freeze_for_update`).
    asker: Option<Window>,
    /// A `shutdown` admitted (`admit_shutdown`): the core is leaving, and no
    /// update's stop begins.
    leaving: bool,
    /// How many times an update was called off (`call_off_update`), whether
    /// a stop stood or not: a stop asked before one of them but freezing
    /// after it is refused (`freeze_for_update`).
    call_offs: u64,
}

static STOPS: Mutex<Stops> = Mutex::new(Stops::NONE);

fn stops() -> std::sync::MutexGuard<'static, Stops> {
    STOPS.lock().unwrap_or_else(|e| e.into_inner())
}

impl Stops {
    const NONE: Stops = Stops { bits: 0, updating: 0, called_off: false, asker: None, leaving: false, call_offs: 0 };

    fn thaw(&mut self, why: Stop) {
        let was = self.bits & why.bit() != 0;
        self.bits &= !why.bit();
        if why == Stop::Update {
            self.asker = None;
            crate::starts::set_update(false);
            if was {
                forget_update_stop();
            }
        }
        if self.bits == 0 {
            CHATS_FROZEN.store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }

    /// The last of the update's stops returned: a call-off said meanwhile
    /// takes effect.
    fn update_returned(&mut self) {
        self.updating = self.updating.saturating_sub(1);
        if self.updating == 0 && std::mem::take(&mut self.called_off) {
            self.thaw(Stop::Update);
        }
    }

    /// Whether `window`, of this core's build if `ours`, coming says the
    /// update this core was stopped for is not coming: one stands, none of
    /// its stops is under way, and the window that asked for it is neither
    /// this one nor still there (`gone`). Not known, it is taken for gone.
    fn update_not_coming(&self, window: Window, ours: bool, gone: impl Fn(&Window) -> bool) -> bool {
        let standing = self.bits & Stop::Update.bit() != 0 && self.updating == 0;
        standing && ours && self.asker.is_none_or(|asker| asker != window && gone(&asker))
    }

    /// Whether a `shutdown` asked by the process `caller` is refused: while
    /// an update's stop stands, its stop under way or returned, only the
    /// window that froze it (`asker`) stops the core, or a caller whose hello
    /// says it is a newer version of another build (`newer`, `newer_build`).
    /// A caller not known, or a stop asked on no window's connection, is no
    /// asker.
    fn refuses_shutdown(&self, caller: Option<Window>, newer: bool) -> bool {
        let standing = self.bits & Stop::Update.bit() != 0;
        standing && !newer && (caller.is_none() || caller != self.asker)
    }
}

/// Said to a `shutdown` while an update's stop stands (`admit_shutdown`).
const HELD_FOR_UPDATE: &str = "the core is held for an update being installed: only the window that stopped its sessions \
    for it may stop it, until the update is installed or called off";

/// Said to an update's stop asked once a `shutdown` is admitted
/// (`freeze_for_update`).
const LEAVING: &str = "the core is stopping";
/// Why an update's stop asked before a call off of the update, and freezing
/// after it, stops nothing (`freeze_for_update`).
const CALLED_OFF: &str = "the update was called off before its sessions' stop began";

/// Whether a caller's hello (`Hello.fingerprint`, `Hello.version`) says it
/// is another build than this core's, of a newer version: as the window an
/// update installs does, replacing the core held for it (corelink.rs
/// `verdict`). A compatibility exception, not proof that an install came:
/// it is the caller's word, and a newer build run beside this one (a dev
/// build) says the same.
pub fn newer_build(fingerprint: Option<&str>, version: Option<&str>) -> bool {
    let other = fingerprint.is_some_and(|f| f != env!("LUNA_CORE_FP"));
    other && version.is_some_and(|v| crate::cli::version_cmp(v, env!("CARGO_PKG_VERSION")).is_gt())
}

/// A `shutdown` admitted (`admit_shutdown`): while it is held, the core is
/// leaving and no update's stop begins. Dropped where the shutdown does not
/// go after all (`ifLeftOnly` refused); the core's leaving never drops it,
/// as the process ends with it held.
#[must_use = "the core counts as leaving for as long as this lives"]
pub struct Leaving(());

impl Drop for Leaving {
    fn drop(&mut self) {
        stops().leaving = false;
    }
}

/// The protocol's `shutdown` asked by the process `caller`, its hello
/// `newer` (`newer_build`) or not (coreproc.rs `requests`), admitted or
/// refused under the stops' lock, so that it and an update's stop exclude
/// each other: one admitted first has the stop refused (`freeze_for_update`),
/// one asked once the stop has frozen the chats is refused unless the caller
/// is the window that froze them or newer (`Stops::refuses_shutdown`). Let
/// through, it would end the core under the installer, or with its stop
/// recorded for no install. Not the core's own leaving: its idle exit, and
/// the PC's shutdown or sign-out, go as ever.
pub fn admit_shutdown(caller: Option<Window>, newer: bool) -> Result<Leaving, &'static str> {
    let mut stops = stops();
    if stops.refuses_shutdown(caller, newer) {
        return Err(HELD_FOR_UPDATE);
    }
    stops.leaving = true;
    Ok(Leaving(()))
}

/// Freezes the chats' ties for the core's leaving or the PC's shutdown, until
/// that stop is called off (`thaw_chats`) and no other is under way. An
/// update freezes them through `freeze_for_update`.
pub fn freeze_chats(why: Stop) {
    debug_assert_ne!(why, Stop::Update, "an update freezes through freeze_for_update");
    let mut stops = stops();
    stops.bits |= why.bit();
    freeze(&CHATS_LAST, &CHATS_FROZEN);
}

/// The stop `why` called off, its caller knowing the process goes on: what
/// the chats' ties say is news once more, unless another stop is under way.
pub fn thaw_chats(why: Stop) {
    stops().thaw(why);
}

/// Every session being stopped for an update (`freeze_for_update`), until it
/// is dropped once they are.
#[must_use = "the sessions are being stopped for as long as the guard lives"]
pub struct UpdateStopping(());

impl UpdateStopping {
    /// The stop goes no further (it would end a session nobody agreed to
    /// stop, pty `stopped_for_update`): the update is called off as it
    /// returns, as the window would once it hears, so the start gate opens
    /// again and the freeze lifts unless another stop is still under way.
    pub fn call_off(self) {
        stops().called_off = true;
    }
}

impl Drop for UpdateStopping {
    /// A stop that panicked calls the update off, as the window does once
    /// it hears the stop failed: the start gate is not left closed over an
    /// update that is not coming.
    fn drop(&mut self) {
        let mut stops = stops();
        if std::thread::panicking() {
            stops.called_off = true;
        }
        stops.update_returned();
    }
}

/// Freezes the chats' ties for an update (`Stop::Update`) as every session
/// is stopped for it, until the update is called off (`call_off_update`,
/// `window_came`, a panic in the stop): a save would write what the stopping
/// still does. The start gate (`crate::starts`) is closed with it, before the
/// stop lists anything, and opens as it lifts: nothing starts that the stop
/// would not see, nor before the installer, unless the update is off. Kept on
/// disk for a core of this build started before then.
///
/// `asker`: the window process the stop was asked on (coreproc.rs
/// `requests`). The stop that freezes the chats makes it the update's asker,
/// whom alone the core stops for meanwhile (`admit_shutdown`) and whose
/// going lets a window of this build call the update off (`window_came`);
/// a stop asked again while the freeze stands, by it or anyone, changes
/// nothing of that. Refused once a `shutdown` is admitted: the core is
/// leaving, and the window is not to install as if it stayed.
///
/// `call_offs`: the update's call offs counted as the stop's request was
/// read (`update_call_offs`, coreproc.rs `requests`). The stop and a call
/// off read after it each run on a thread of their own, and the stop writes
/// what helpers do before it freezes: a call off whose thread got there
/// first found no stop to call off, so the stop it was meant for is refused
/// here, with `CALLED_OFF`, rather than freeze over an update the window has
/// let go.
pub fn freeze_for_update(asker: Option<Window>, call_offs: u64) -> Result<UpdateStopping, &'static str> {
    let mut stops = stops();
    if stops.leaving {
        return Err(LEAVING);
    }
    if call_offs != stops.call_offs {
        return Err(CALLED_OFF);
    }
    if stops.bits & Stop::Update.bit() == 0 {
        stops.asker = asker;
    }
    stops.updating += 1;
    stops.bits |= Stop::Update.bit();
    crate::starts::set_update(true);
    keep_update_stop(&KeptStop { fingerprint: env!("LUNA_CORE_FP").into(), asker: stops.asker });
    freeze(&CHATS_LAST, &CHATS_FROZEN);
    Ok(UpdateStopping(()))
}

/// How many times an update was called off so far (`call_off_update`): what
/// a stop's request is stamped with as it is read (`freeze_for_update`).
pub fn update_call_offs() -> u64 {
    stops().call_offs
}

/// Whether the sessions are being or have been stopped for an update that is
/// not called off (`Stop::Update`), as a window asks a core of another
/// version in its hello (ipc.rs `Welcome::stopped_for_update`).
pub fn stopped_for_update() -> bool {
    stops().bits & Stop::Update.bit() != 0
}

/// The window's word that the update the sessions were stopped for is not
/// being installed (its install failed): the freeze lifts at once or, while
/// the sessions are still being stopped for it, as that stop returns.
pub fn call_off_update() {
    let mut stops = stops();
    stops.call_offs += 1;
    if stops.updating > 0 {
        stops.called_off = true;
    } else {
        stops.thaw(Stop::Update);
    }
}

/// A window said hello (coreproc.rs `serve`), of the build `fingerprint`
/// names. One of this core's own build, while the update stands and the
/// window that asked for it is gone, says no install came, which would have
/// brought a window of another build: the update is called off. The asker
/// connecting again, or a window of the build installed, leaves it standing.
/// So does the asker's going alone: on Windows the window ends itself as it
/// hands the installer over, before UAC asks.
pub fn window_came(window: Window, fingerprint: Option<&str>) {
    let mut stops = stops();
    if stops.update_not_coming(window, fingerprint == Some(env!("LUNA_CORE_FP")), Window::gone) {
        crate::log::warn(
            "update",
            &format!(
                "a window of this build came (pid {}) with the one that asked for the update's stop gone: no install came, so the update is called off",
                window.pid
            ),
        );
        stops.thaw(Stop::Update);
    }
}

/// An update's stop as kept on disk while it stands: the build of the core
/// that stopped for it, and the window that asked.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
struct KeptStop {
    fingerprint: String,
    asker: Option<Window>,
}

/// Beside the chats' ties: what a core of this build started during an
/// update reads first (`restore_update_stop`).
const UPDATE_STOP_FILE: &str = "update-stop.json";

fn update_stop_file() -> std::path::PathBuf {
    crate::paths::data_dir().join(UPDATE_STOP_FILE)
}

fn keep_update_stop(kept: &KeptStop) {
    let written = serde_json::to_vec(kept)
        .map_err(|e| e.to_string())
        .and_then(|bytes| crate::secret::write_private(&update_stop_file(), &bytes).map_err(|e| e.to_string()));
    if let Err(e) = written {
        crate::log::warn(
            "update",
            &format!("could not keep the update's stop on disk: {e}; a core started before the install starts open"),
        );
    }
}

fn forget_update_stop() {
    let _ = std::fs::remove_file(update_stop_file());
}

/// At a core's start, before any helper or window: an update's stop the last
/// core of this build kept stands for this one too, the start gate closed and
/// the chats' ties frozen, until it is called off as it would have been
/// there (the core restarted or gone during an update). One kept by a core of
/// another build was installed over, and is let go.
pub fn restore_update_stop() {
    let kept = std::fs::read(update_stop_file()).ok().and_then(|bytes| serde_json::from_slice::<KeptStop>(&bytes).ok());
    if !take_up_update_stop(kept, env!("LUNA_CORE_FP")) {
        forget_update_stop();
    }
}

/// `restore_update_stop` on what was read, for a core of build `ours`:
/// whether it stands.
fn take_up_update_stop(kept: Option<KeptStop>, ours: &str) -> bool {
    let Some(kept) = kept.filter(|k| k.fingerprint == ours) else { return false };
    let mut stops = stops();
    stops.bits |= Stop::Update.bit();
    stops.asker = kept.asker;
    crate::starts::set_update(true);
    CHATS_FROZEN.store(true, std::sync::atomic::Ordering::SeqCst);
    crate::log::warn(
        "update",
        "the last core stopped every session for an update neither installed nor called off: nothing starts until it is",
    );
    true
}

/// One test at a time that stops every session or reads the freeze, each
/// starting with no stop under way and nothing frozen.
#[cfg(test)]
pub(crate) fn stops_for_test() -> std::sync::MutexGuard<'static, ()> {
    static ONE: Mutex<()> = Mutex::new(());
    let alone = ONE.lock().unwrap_or_else(|e| e.into_inner());
    *stops() = Stops::NONE;
    CHATS_FROZEN.store(false, std::sync::atomic::Ordering::SeqCst);
    crate::starts::set_update(false);
    alone
}

/// Whether the chats' ties are frozen.
#[cfg(test)]
pub(crate) fn chats_frozen() -> bool {
    CHATS_FROZEN.load(std::sync::atomic::Ordering::SeqCst)
}

/// Sets `frozen` under the saves' lock: a save already taking its snapshot
/// or writing it is through before the sessions are stopped, so nothing of
/// the stopping goes into a snapshot or onto disk after the freeze.
fn freeze(last: &Mutex<Vec<u8>>, frozen: &std::sync::atomic::AtomicBool) {
    let _saving = last.lock().unwrap_or_else(|e| e.into_inner());
    frozen.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// What there is to keep of the sessions the window runs, and the rest the next
/// core is to tell. A chat also keeps the person's Codex trust choice.
pub(super) fn chats_now() -> SavedChats {
    let (mut chats, sends, mut spawns, news) = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let sends: Vec<SavedSend> = r
            .held
            .iter()
            .map(|h| SavedSend { caller: h.caller.clone(), id: h.id.clone(), text: h.text.clone(), going_in: h.going_in.clone() })
            .collect();
        let chats: Vec<SavedChat> = r
            .known
            .iter()
            .filter(|(id, k)| {
                k.settings["background"] != true
                    && !is_dev(id.as_str())
                    && (k.codex_trust.is_some() || r.owed.keys().any(|(_, s)| s == *id) || sends.iter().any(|h| h.id == **id))
            })
            .filter_map(|(id, k)| {
                Some(SavedChat {
                    id: id.clone(),
                    provider: k.provider,
                    folder: k.folder.clone(),
                    account_path: k.account_path.clone(),
                    turns_ended: 0,
                    transcript: None,
                    mid_turn: false,
                    codex_trust: k.codex_trust.clone(),
                    ties: ties_in(&r, id)?,
                })
            })
            .collect();
        (chats, sends, r.spawns.values().cloned().collect::<Vec<SpawnTaken>>(), r.news.clone())
    };
    for c in &mut chats {
        c.turns_ended = crate::activity::turns_counted(&c.id).unwrap_or(0);
        c.transcript = transcripts().lock().unwrap_or_else(|e| e.into_inner()).get(&c.id).cloned();
        c.mid_turn = crate::activity::turn_state(&c.id)
            .is_some_and(|t| matches!(t.turn, crate::activity::Turn::Busy | crate::activity::Turn::Waiting));
    }
    chats.sort_by(|a, b| a.id.cmp(&b.id));
    spawns.sort_by_key(|s| s.request);
    SavedChats { chats, sends, spawns, news, no_handoff: true }
}

/// Writes `chats_now` out if it changed since the last write (background/persist.rs
/// `save` keeps the beat).
pub fn save_chats() {
    use std::sync::atomic::Ordering;
    if !CHATS_READ.load(Ordering::SeqCst) {
        return;
    }
    let path = chat_ties_file();
    save_in_order(
        &CHATS_LAST,
        || snapshot_unless(&CHATS_FROZEN, || serde_json::to_vec_pretty(&chats_now()).ok()),
        |bytes| match crate::secret::write_private(&path, bytes) {
            Ok(()) => true,
            Err(e) => {
                crate::log::warn("agents", &format!("could not save the chats' ties to {}: {e}", path.display()));
                false
            }
        },
    );
}

/// `snapshot`, unless `frozen` is set: asked as the snapshot is taken, under
/// the save's lock (`save_in_order`), so a save that waited there behind
/// another while the sessions were stopped (`freeze_chats`) takes nothing of
/// what the stopping did and writes nothing over what was written before it.
fn snapshot_unless(frozen: &std::sync::atomic::AtomicBool, snapshot: impl FnOnce() -> Option<Vec<u8>>) -> Option<Vec<u8>> {
    if frozen.load(std::sync::atomic::Ordering::SeqCst) {
        return None;
    }
    snapshot()
}

/// Takes a snapshot and writes it if it differs from the last one written,
/// all under `last`: a save that took its snapshot ahead of another's and
/// wrote behind it would leave the older one on disk (the notifier's and the
/// periodic save run at once), for a core killed before
/// the next save to restore. `write` says whether the bytes are on disk.
fn save_in_order(last: &Mutex<Vec<u8>>, snapshot: impl FnOnce() -> Option<Vec<u8>>, write: impl FnOnce(&[u8]) -> bool) {
    let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
    let Some(bytes) = snapshot() else { return };
    if *last == bytes {
        return;
    }
    if write(&bytes) {
        *last = bytes;
    }
}

/// Takes up what the last core saved of the sessions the window runs, before
/// anything can ask after them. They are known again as they were, though not
/// running until the window starts them (`Known::waiting`).
pub fn restore_chats() {
    let saved: SavedChats = match std::fs::read(chat_ties_file()) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            crate::log::warn("agents", &format!("could not read chat-ties.json: {e}"));
            SavedChats::default()
        }),
        Err(_) => SavedChats::default(),
    };
    let n = saved.chats.len();
    take_up_chats(saved);
    CHATS_READ.store(true, std::sync::atomic::Ordering::SeqCst);
    if n > 0 {
        crate::log::warn("agents", &format!("took up the ties of {n} chat(s) the last core left"));
    }
}

pub(super) fn take_up_chats(saved: SavedChats) {
    let mut counts: Vec<(String, u32, Option<std::path::PathBuf>)> = Vec::new();
    let mut migrated: Vec<String> = Vec::new();
    let mut retired = 0usize;
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    for c in saved.chats {
        let ties = c.ties;
        // A chat the window runs is the person's own: one an earlier Luna had
        // spawned as a visible helper, or had a helper of passed to, is theirs now,
        // and nobody's to stop, delete or answer for. What it owes callers stands.
        if ties.parent.is_some() || ties.orphan {
            migrated.push(c.id.clone());
        }
        let mut settings = without_retired(ties.settings);
        if let Some(o) = settings.as_object_mut() {
            for key in HELPER_SETTINGS {
                o.remove(key);
            }
            o.remove(CODEX_TRUST);
            o.remove("trustedInLuna");
        }
        r.known.entry(c.id.clone()).or_insert(Known {
            provider: c.provider,
            account: account_name(&c.account_path),
            folder: c.folder,
            account_path: c.account_path,
            parent: None,
            settings,
            codex_trust: c.codex_trust.filter(|_| c.provider == Provider::Codex),
            tools: ties.tools,
            waiting: true,
            orphan: false,
        });
        if let Some(name) = ties.name {
            r.names.insert(c.id.clone(), name);
        }
        // An answer not in by the time the core went will not come: the CLI
        // it was being written by went with it, and the one the window starts
        // is between turns. That turn counts as ended, as a helper's does.
        let mut count = c.turns_ended;
        for (caller, baseline, cursor) in ties.owed {
            // Its turn was over, told so: what it left went with the core. Not
            // one it was in the middle of again: that turn is cut short below.
            if let Some(o) = told_job_went(&ties.told_pending, &caller, baseline, cursor).filter(|_| !c.mid_turn) {
                r.owed.insert((caller, c.id.clone()), o);
                continue;
            }
            let mut o = Owed::from(baseline, cursor);
            if c.turns_ended <= baseline || ties.cut_short.contains(&caller) {
                o.cut_short = true;
                count = count.max(baseline.saturating_add(1));
            }
            r.owed.insert((caller, c.id.clone()), o);
        }
        for (caller, n) in ties.seen {
            r.seen.insert((caller, c.id.clone()), n);
        }
        if let Some(task) = ties.task {
            r.tasks.insert(c.id.clone(), task);
        }
        counts.push((c.id, count, c.transcript));
    }
    for h in saved.sends {
        r.held.push(Held { caller: h.caller, id: h.id, text: h.text, since: now_ms(), going_in: h.going_in });
    }
    for mut s in saved.spawns {
        s.left_by_core = true;
        r.next_request = r.next_request.max(s.request.saturating_add(1));
        r.spawns.insert(s.request, s);
    }
    for line in saved.news {
        // 0.19's handoff and move lines, an ask, a lapse or a failure with
        // the continuation it carried, are dropped: they point at a tool, a
        // document and a conversation there no longer are (ADR-0030).
        if !saved.no_handoff && of_retired_handoff(&line.1) {
            retired += 1;
        } else if !r.news.contains(&line) {
            r.news.push(line);
        }
    }
    drop(r);
    if retired > 0 {
        crate::log::info("agents", &format!("dropped {retired} handoff line(s) the last core had queued: Luna has no handoff since 0.20"));
    }
    for id in migrated {
        crate::log::warn("agents", &format!("{} was a visible helper: the person's own chat from now on", label(&id)));
    }
    for (id, count, transcript) in counts {
        crate::activity::keep_count(&id, count);
        if let Some(path) = transcript {
            transcripts().lock().unwrap_or_else(|e| e.into_inner()).insert(id, path);
        }
    }
}

/// Whether `line`, queued by a 0.19 core, is one of its handoff's or a move's
/// lines as it wrote them (ADR-0030 removed both), Luna's notice tag aside:
/// a chat's threshold ask, the person's ask, its lapse or withdrawal, a
/// failure line and what took its place, a lead's ask and failure, the
/// prompts it typed. Not to be typed into anything. Only a file 0.19 wrote
/// is looked at so (`SavedChats::no_handoff`, background `Saved::no_handoff`).
pub(crate) fn of_retired_handoff(line: &str) -> bool {
    let line = line.trim_start();
    let line = line.strip_prefix(crate::agents::NOTICE_TAG).unwrap_or(line).trim_start();
    if retired_opening(line) {
        return true;
    }
    if let Some(rest) = line.strip_prefix("Handoff ").and_then(numbered) {
        const AFTER: [&str; 5] = [
            " cleared the conversation, but Luna",
            " did not go through: ",
            " of this chat did not go through: ",
            " of this chat cleared the conversation that wrote it",
            " started no new conversation: ",
        ];
        return AFTER.iter().any(|a| rest.starts_with(a));
    }
    const WHOLE: [&str; 4] = [
        "The person asks for a handoff of this chat now: ",
        "The person's request for a handoff of this chat lapsed: ",
        "The person asked for a handoff of this chat, but Luna could not deliver the request",
        "A handoff of this chat cleared the conversation that wrote it",
    ];
    WHOLE.iter().any(|w| line.starts_with(w)) || (line.starts_with("This chat's context is at ") && line.contains("luna_session_handoff"))
}

/// A run of digits at the start of `s`, and what follows it.
fn numbered(s: &str) -> Option<&str> {
    let rest = s.trim_start_matches(|c: char| c.is_ascii_digit());
    (rest.len() < s.len()).then_some(rest)
}

/// Whether `text` opens on the tag 0.19's handoff or a move typed its
/// prompts under, as it wrote it: `[Luna handoff <n>, not typed by the
/// user]`, `[Luna move <id>, not typed by the user]` or `[Luna move <id>
/// request, not typed by the user]`.
pub(crate) fn retired_opening(text: &str) -> bool {
    let text = text.trim_start();
    if let Some(rest) = text.strip_prefix("[Luna handoff ").and_then(numbered) {
        return rest.starts_with(", not typed by the user]");
    }
    let Some(rest) = text.strip_prefix("[Luna move ") else { return false };
    let (id, tail) = rest.split_at(rest.find([' ', ',']).unwrap_or(rest.len()));
    !id.is_empty()
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && (tail.starts_with(", not typed by the user]") || tail.starts_with(" request, not typed by the user]"))
}

/// `text`, a notice 0.19 put together for a session above a lead, without
/// the line it added there of the leads under it that handed off (`Lead <id>
/// handed off (<n>): …`, `Leads handed off: …`), what else it says kept.
/// None where nothing else is left of it.
pub(crate) fn without_handed_off(text: &str) -> Option<String> {
    const ONE: (&str, &str) = ("Lead ", ": it goes on in a fresh conversation; its helpers stay with it.");
    const MANY: (&str, &str) = ("Leads handed off: ", ": each goes on in a fresh conversation; their helpers stay with them.");
    let mut text = text.to_string();
    for (start, end) in [ONE, MANY] {
        while let Some(e) = text.find(end) {
            let Some(s) = text[..e].rfind(start) else { break };
            if start == ONE.0 && !(text[s..e].contains(" handed off (") && text[s..e].ends_with(')')) {
                break;
            }
            let from = if text[..s].ends_with(' ') { s - 1 } else { s };
            text.replace_range(from..e + end.len(), "");
        }
    }
    let said = text.trim_start();
    let said = said.strip_prefix(crate::agents::NOTICE_TAG).unwrap_or(said).trim();
    (!said.is_empty()).then_some(text)
}

/// What only a helper's settings hold (`registry::CORE_SETTINGS`, `background`):
/// a chat of the person's has none of them.
pub(super) const HELPER_SETTINGS: [&str; 4] = ["background", "resumeOnRestart", "connectors", "constraints"];

/// How long this core has been up.
pub(super) fn core_up_ms() -> u64 {
    static UP: OnceLock<u64> = OnceLock::new();
    now_ms().saturating_sub(*UP.get_or_init(now_ms))
}

/// Whether a session the last core knew may yet be started by the window.
pub(super) fn returning(k: &Known, up_ms: u64) -> bool {
    k.waiting && up_ms < SESSION_RETURN_MS
}

/// A spawn the last core left that had come to a helper it saved, and that had not
/// got as far as `owe_from`: that helper answers its caller as any other, so owes
/// it now, its turns since (if any) the answer. Once for each spawn (`owed_placed`):
/// under a later core the debt may have been told and settled, and the helper be
/// long done. Once both registries are read.
pub fn take_up_spawns() {
    let due: Vec<(u64, String, String)> = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        r.spawns
            .values()
            .filter(|s| !s.owed_placed)
            .filter_map(|s| s.helper.as_ref().filter(|h| r.known.contains_key(*h)).map(|h| (s.request, s.caller.clone(), h.clone())))
            .collect()
    };
    for (request, caller, id) in due {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = r.spawns.get_mut(&request) {
            s.owed_placed = true;
        }
        let key = (caller, id);
        if r.owed.contains_key(&key) {
            continue;
        }
        r.owed.insert(key.clone(), Owed::from(0, 0));
        r.seen.entry(key).or_insert(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::held::{deliver_held, is_held};
    use crate::agents::notifier::lines::notice_line;
    use crate::agents::notifier::{owed_stands, Settled, CALLER_RETURN_MS};
    use crate::agents::owed::{last_shown, owe_from};
    use crate::agents::registry::{gone_from_the_window, parent_of, reg, register};
    use crate::agents::said::{first_words, TASK_CHARS};
    use crate::agents::spawn::SpawnTaken;
    use crate::agents::teardown::forget;
    use crate::agents::test_support::{owed_chat, spawn_taken, spawns_to_myself, take_up};
    use std::collections::HashMap;

    /// Of two saves at once, the one that took its snapshot later writes
    /// later: a snapshot taken while another save is writing waits for it.
    #[test]
    fn a_save_snapshotted_later_is_the_one_left_on_disk() {
        static LAST: Mutex<Vec<u8>> = Mutex::new(Vec::new());
        let disk = std::sync::Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
        let write = |disk: std::sync::Arc<Mutex<Vec<Vec<u8>>>>| {
            move |bytes: &[u8]| {
                disk.lock().unwrap().push(bytes.to_vec());
                true
            }
        };
        let (snapping, snapped) = std::sync::mpsc::channel();
        let (newer_done, newer_written) = std::sync::mpsc::channel::<()>();
        let older = {
            let write = write(disk.clone());
            std::thread::spawn(move || {
                let snapshot = || {
                    snapping.send(()).unwrap();
                    // Let in, the newer save writes in this time.
                    let _ = newer_written.recv_timeout(std::time::Duration::from_millis(300));
                    Some(b"older".to_vec())
                };
                save_in_order(&LAST, snapshot, write);
            })
        };
        snapped.recv().unwrap();
        let newer = {
            let write = write(disk.clone());
            std::thread::spawn(move || {
                save_in_order(&LAST, || Some(b"newer".to_vec()), write);
                let _ = newer_done.send(());
            })
        };
        older.join().unwrap();
        newer.join().unwrap();
        assert_eq!(disk.lock().unwrap().last().map(Vec::as_slice), Some(&b"newer"[..]));
        // Unchanged since the last write: nothing written.
        save_in_order(&LAST, || Some(b"newer".to_vec()), |_| panic!("written again"));
    }

    /// A save that waited behind another while the chats were frozen takes
    /// no snapshot and writes nothing over what was written before.
    #[test]
    fn a_save_queued_behind_the_freeze_writes_nothing() {
        static LAST: Mutex<Vec<u8>> = Mutex::new(Vec::new());
        static FROZEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        let (writing, wrote) = std::sync::mpsc::channel();
        let (go, wait) = std::sync::mpsc::channel::<()>();
        let first = std::thread::spawn(move || {
            save_in_order(
                &LAST,
                || snapshot_unless(&FROZEN, || Some(b"before".to_vec())),
                |_| {
                    writing.send(()).unwrap();
                    wait.recv().unwrap();
                    true
                },
            )
        });
        wrote.recv().unwrap();
        let queued = std::thread::spawn(|| {
            save_in_order(
                &LAST,
                || snapshot_unless(&FROZEN, || panic!("a snapshot after the freeze")),
                |_| panic!("written after the freeze"),
            )
        });
        std::thread::sleep(std::time::Duration::from_millis(50));
        FROZEN.store(true, std::sync::atomic::Ordering::SeqCst);
        go.send(()).unwrap();
        first.join().unwrap();
        queued.join().unwrap();
        assert_eq!(*LAST.lock().unwrap(), b"before");
    }

    /// A freeze that comes while a save is taking its snapshot waits for that
    /// save to be written: nothing goes onto disk once the freeze has
    /// returned and the sessions are being stopped.
    #[test]
    fn a_freeze_waits_for_the_save_under_way() {
        static LAST: Mutex<Vec<u8>> = Mutex::new(Vec::new());
        static FROZEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        let said = std::sync::Arc::new(Mutex::new(Vec::<&str>::new()));
        let (snapping, snapped) = std::sync::mpsc::channel();
        let save = {
            let said = said.clone();
            std::thread::spawn(move || {
                save_in_order(
                    &LAST,
                    || {
                        snapshot_unless(&FROZEN, || {
                            snapping.send(()).unwrap();
                            // Let in, the freeze returns in this time.
                            std::thread::sleep(std::time::Duration::from_millis(200));
                            Some(b"before".to_vec())
                        })
                    },
                    |_| {
                        said.lock().unwrap().push("written");
                        true
                    },
                )
            })
        };
        snapped.recv().unwrap();
        freeze(&LAST, &FROZEN);
        said.lock().unwrap().push("frozen");
        save.join().unwrap();
        assert_eq!(*said.lock().unwrap(), ["written", "frozen"]);
    }

    /// A session that starts while every session is being stopped (a pane
    /// asking for its chat again, a helper's turn), or after, leaves the
    /// chats' ties frozen: the process may yet end with the stop, and a save
    /// then would write what the stopping did. Only the stop called off lifts
    /// it, and only once no other stop is under way; the core's leaving is
    /// never called off.
    #[test]
    fn a_stop_keeps_the_chats_frozen_until_it_is_called_off() {
        let _alone = stops_for_test();
        let start = || register("fz-start", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, false, serde_json::json!({}));
        let stopping = freeze_for_update(None, update_call_offs()).unwrap();
        start();
        assert!(chats_frozen(), "a start while it stops");
        drop(stopping);
        start();
        thaw_chats(Stop::PowerOff);
        assert!(chats_frozen(), "another stop called off");
        call_off_update();
        assert!(!chats_frozen(), "the update called off");
        drop(freeze_for_update(None, update_call_offs()).unwrap());
        freeze_chats(Stop::Leaving);
        call_off_update();
        start();
        assert!(chats_frozen(), "the core leaves all the same");
        forget("fz-start");
        // As a test that does not stop every session expects to find them.
        *stops() = Stops::NONE;
        CHATS_FROZEN.store(false, std::sync::atomic::Ordering::SeqCst);
    }

    /// The window's word that the update is off, said while the sessions
    /// are still being stopped for it, takes effect as the stop returns and
    /// not before: a save meanwhile would write what the stopping does. It is
    /// not kept for the next update; said once the stop has returned, it
    /// lifts the freeze at once.
    #[test]
    fn an_update_called_off_during_its_stop_is_called_off_as_it_returns() {
        let _alone = stops_for_test();
        crate::starts::under_update_for_test("fz-gate");
        let stopping = freeze_for_update(None, update_call_offs()).unwrap();
        call_off_update();
        assert!(chats_frozen(), "not while the sessions are stopped");
        assert!(!crate::starts::open("fz-gate"), "nothing starts while they are");
        assert!(stopped_for_update(), "a window's hello hears it is stopped for an update");
        drop(stopping);
        assert!(!chats_frozen(), "as the stop returns");
        assert!(!stopped_for_update(), "and hears it no more once that is called off");
        assert!(crate::starts::open("fz-gate"), "and the gate is open again");
        drop(freeze_for_update(None, update_call_offs()).unwrap());
        assert!(!crate::starts::open("fz-gate"), "closed through the install's handoff");
        assert!(chats_frozen(), "the next update's freeze is its own");
        assert!(stopped_for_update(), "past the stop, until the update is called off");
        call_off_update();
        assert!(!chats_frozen(), "called off at once once the stop has returned");
        assert!(!stopped_for_update());
        assert!(crate::starts::open("fz-gate"));
    }

    /// A call off read after a stop's request whose thread gets there before
    /// the stop freezes (the stop still writing what helpers do) finds no
    /// stop to call off; the stop, stamped with the call offs before it, is
    /// refused as it would freeze, and nothing is frozen or closed. One
    /// stamped after the call off freezes as before.
    #[test]
    fn a_call_off_that_beats_its_stop_to_the_freeze_refuses_that_stop() {
        let _alone = stops_for_test();
        crate::starts::under_update_for_test("fz-beaten");
        let asked = update_call_offs();
        call_off_update();
        assert_eq!(freeze_for_update(None, asked).err(), Some(CALLED_OFF));
        assert!(!chats_frozen() && !stopped_for_update());
        assert!(crate::starts::open("fz-beaten"), "the gate stays open");
        let stopping = freeze_for_update(None, update_call_offs()).expect("asked after it");
        assert!(stopped_for_update());
        call_off_update();
        drop(stopping);
        assert!(!stopped_for_update() && crate::starts::open("fz-beaten"));
    }

    /// Nothing lifts an update's stop by the clock: an installer waiting on
    /// UAC for as long as nobody answers, its window gone as it handed the
    /// install over, finds it standing, saves and windows of the build
    /// installed and the asker itself coming back notwithstanding. A window of
    /// this core's own build, with the asker gone, says no install came, and
    /// calls it off; with the asker still there it does not. The freeze of the
    /// core's leaving or a PC's shutdown stays through it.
    #[test]
    fn an_updates_stop_stands_until_a_window_of_this_build_comes_with_the_asker_gone() {
        let _alone = stops_for_test();
        crate::starts::under_update_for_test("fz-uac");
        let asker = Window { pid: 4242, created_ms: Some(7) };
        let other = Window { pid: 4343, created_ms: Some(8) };
        let stopping = freeze_for_update(Some(asker), update_call_offs()).unwrap();
        let gone = |_: &Window| true;
        assert!(!stops().update_not_coming(other, true, gone), "not while its stop is under way");
        drop(stopping);
        save_chats();
        assert!(stopped_for_update() && !crate::starts::open("fz-uac"), "no save lifts it");
        assert!(!stops().update_not_coming(other, false, gone), "a window of the build installed");
        assert!(!stops().update_not_coming(asker, true, gone), "the asker itself, connecting again");
        assert!(!stops().update_not_coming(other, true, |_| false), "the asker still there, as on UAC before it went");
        assert!(stops().update_not_coming(other, true, gone), "this build again, its asker gone: no install came");
        // As `window_came` does then.
        stops().thaw(Stop::Update);
        assert!(crate::starts::open("fz-uac") && !chats_frozen());
        // Asked for on no window's connection, any window of this build calls it off.
        drop(freeze_for_update(None, update_call_offs()).unwrap());
        assert!(stops().update_not_coming(asker, true, |_| false));
        stops().thaw(Stop::Update);
        for why in [Stop::PowerOff, Stop::Leaving] {
            freeze_chats(why);
            drop(freeze_for_update(None, update_call_offs()).unwrap());
            stops().thaw(Stop::Update);
            assert!(chats_frozen(), "{why:?} holds once the update is called off");
            if why == Stop::PowerOff {
                thaw_chats(why);
                assert!(!chats_frozen(), "called off as the OS takes the shutdown back");
            }
        }
        // As a test that does not stop every session expects to find them.
        *stops() = Stops::NONE;
        CHATS_FROZEN.store(false, std::sync::atomic::Ordering::SeqCst);
    }

    /// While an update's stop stands, the protocol's `shutdown` stops the core
    /// only for the window that froze it, by its process (not another
    /// process of that pid), or for a caller whose hello names a newer
    /// version of another build: another window, `luna --stop-core` or a
    /// caller not known is refused, the stop under way or returned. Once it
    /// lifts, by its call-off, a window of this build with the asker gone, or
    /// a panic in the stop, anyone stops it.
    #[test]
    fn a_core_held_for_an_update_stops_only_for_the_window_that_froze_it() {
        let _alone = stops_for_test();
        let asker = Window { pid: 4242, created_ms: Some(7) };
        let other = Window { pid: 4343, created_ms: Some(8) };
        // Admitted, the core would leave: the guard is let go at once here.
        let refused = |caller: Option<Window>, newer: bool| admit_shutdown(caller, newer).map(drop).is_err();
        let lifted = |how: &str| {
            for caller in [Some(other), Some(asker), None] {
                assert!(!refused(caller, false), "{how}: anyone stops it");
            }
        };
        lifted("nothing frozen");

        let stopping = freeze_for_update(Some(asker), update_call_offs()).unwrap();
        assert_eq!(admit_shutdown(Some(other), false).err(), Some(HELD_FOR_UPDATE), "another window, while it stops");
        assert!(!refused(Some(asker), false), "the asker, while it stops");
        drop(stopping);
        assert!(refused(Some(other), false), "another window or `--stop-core`");
        assert!(refused(Some(Window { pid: 4242, created_ms: Some(9) }), false), "another process given the asker's pid");
        assert!(refused(None, false), "a caller not known");
        assert!(!refused(Some(asker), false), "the asker: its Quit, its restart, on any connection of its");
        assert!(!refused(Some(other), true), "a newer build, replacing it");
        call_off_update();
        lifted("called off");

        drop(freeze_for_update(None, update_call_offs()).unwrap());
        assert!(refused(Some(other), false));
        // As `window_came` does, a window of this build with the asker gone.
        stops().thaw(Stop::Update);
        lifted("no install came");

        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _stopping = freeze_for_update(None, update_call_offs()).unwrap();
            assert!(refused(Some(other), false));
            panic!("the stop failed");
        }));
        assert!(panicked.is_err());
        lifted("the stop panicked");

        // Asked for on no window's connection: no one is its asker.
        drop(freeze_for_update(None, update_call_offs()).unwrap());
        assert!(refused(Some(asker), false) && refused(None, false));
        call_off_update();
        // The core's own leaving or a PC's shutdown is no update's stop.
        freeze_chats(Stop::Leaving);
        lifted("the core leaving");
        *stops() = Stops::NONE;
        CHATS_FROZEN.store(false, std::sync::atomic::Ordering::SeqCst);
    }

    /// The stop that freezes the chats for an update makes its window the
    /// asker; a stop asked again while the freeze stands, by another window
    /// or on no window's connection, changes nothing of it, in memory or on
    /// disk. Once the freeze lifts, the next stop's window is the asker.
    #[test]
    fn only_the_stop_that_freezes_names_the_asker() {
        let _alone = stops_for_test();
        let asker = Window { pid: 4242, created_ms: Some(7) };
        let other = Window { pid: 4343, created_ms: Some(8) };
        let kept = || {
            let read = std::fs::read(update_stop_file()).ok();
            read.and_then(|b| serde_json::from_slice::<KeptStop>(&b).ok()).and_then(|k| k.asker)
        };
        let stopping = freeze_for_update(Some(asker), update_call_offs()).unwrap();
        drop(freeze_for_update(Some(other), update_call_offs()).unwrap());
        drop(freeze_for_update(None, update_call_offs()).unwrap());
        drop(stopping);
        assert_eq!((stops().asker, kept()), (Some(asker), Some(asker)), "the first stop's window");
        assert!(admit_shutdown(Some(other), false).is_err() && admit_shutdown(None, false).is_err());
        assert!(admit_shutdown(Some(asker), false).is_ok(), "the asker's Quit still goes");
        call_off_update();
        drop(freeze_for_update(Some(other), update_call_offs()).unwrap());
        assert_eq!((stops().asker, kept()), (Some(other), Some(other)), "a new freeze, a new asker");
        call_off_update();
    }

    /// A `shutdown` and an update's stop exclude each other: one admitted
    /// first has the stop refused, nothing frozen and the gate left open, so
    /// the window does not install as if the core stayed; let go (an
    /// `ifLeftOnly` refused), the stop goes again. The other way round, see
    /// `a_core_held_for_an_update_stops_only_for_the_window_that_froze_it`.
    #[test]
    fn a_shutdown_admitted_first_refuses_the_updates_stop() {
        let _alone = stops_for_test();
        crate::starts::under_update_for_test("fz-leaving");
        let leaving = admit_shutdown(None, false).unwrap();
        assert_eq!(freeze_for_update(None, update_call_offs()).err(), Some(LEAVING));
        assert!(!stopped_for_update() && !chats_frozen() && crate::starts::open("fz-leaving"), "nothing of the stop");
        drop(leaving);
        drop(freeze_for_update(None, update_call_offs()).unwrap());
        assert!(stopped_for_update());
        call_off_update();
    }

    /// Only a hello naming another build and a newer version passes for the
    /// window an update installs; an older or same-version build, or one
    /// that names no version or build, does not.
    #[test]
    fn a_newer_build_is_another_build_of_a_newer_version() {
        let (ours, other) = (Some(env!("LUNA_CORE_FP")), Some("0000000000000000"));
        assert!(newer_build(other, Some("999.0.0")));
        assert!(!newer_build(other, Some(env!("CARGO_PKG_VERSION"))), "a dev build of this version");
        assert!(!newer_build(other, Some("0.0.1")), "an older version");
        assert!(!newer_build(ours, Some("999.0.0")), "this build");
        assert!(!newer_build(other, None) && !newer_build(None, Some("999.0.0")));
    }

    /// An update's stop is kept on disk while it stands, with its build and
    /// asker, and let go as it is called off. A core of that build started
    /// meanwhile (restarted, or gone and started again during the update)
    /// starts with it, the gate closed and the ties frozen; one of another
    /// build, the build installed, starts free of it.
    #[test]
    fn a_core_of_the_same_build_started_during_an_update_starts_stopped() {
        let _alone = stops_for_test();
        crate::starts::under_update_for_test("fz-kept");
        let asker = Window { pid: 4242, created_ms: None };
        drop(freeze_for_update(Some(asker), update_call_offs()).unwrap());
        let read = || std::fs::read(update_stop_file()).ok().and_then(|b| serde_json::from_slice::<KeptStop>(&b).ok());
        let kept = read().expect("kept while it stands");
        assert_eq!(kept, KeptStop { fingerprint: env!("LUNA_CORE_FP").into(), asker: Some(asker) });
        call_off_update();
        assert!(read().is_none(), "let go as it is called off");
        // A new core of the same build, as it starts.
        *stops() = Stops::NONE;
        CHATS_FROZEN.store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(take_up_update_stop(Some(KeptStop { fingerprint: "fp-a".into(), asker: Some(asker) }), "fp-a"));
        assert!(stopped_for_update() && chats_frozen() && !crate::starts::open("fz-kept"), "it starts stopped");
        assert!(stops().update_not_coming(Window { pid: 1, created_ms: None }, true, |w| *w == asker), "its asker kept");
        call_off_update();
        assert!(crate::starts::open("fz-kept"));
        assert!(!take_up_update_stop(Some(KeptStop { fingerprint: "fp-a".into(), asker: None }), "fp-b"), "installed over");
        assert!(!take_up_update_stop(None, "fp-a"));
        assert!(!stopped_for_update() && crate::starts::open("fz-kept"));
    }

    /// What a helper was spawned on is kept with it, not with `agents.jsonl`,
    /// which is cut with the helper still there.
    #[test]
    fn a_helpers_task_is_one_line_and_comes_back_with_it() {
        assert_eq!(first_words("\n# Fix the **build**\n\nThen the rest.", TASK_CHARS).as_deref(), Some("Fix the build"));
        let long = "word ".repeat(40);
        let cut = first_words(&long, TASK_CHARS).unwrap();
        assert!(cut.ends_with('…') && cut.chars().count() <= TASK_CHARS + 1, "{cut}");
        let none = serde_json::json!({});
        register("tk-h", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("tk-c"), false, none);
        reg().lock().unwrap().tasks.insert("tk-h".into(), first_words("Review the diff\nin detail", TASK_CHARS).unwrap());
        let saved: Ties = serde_json::from_slice(&serde_json::to_vec(&ties_of("tk-h").unwrap()).unwrap()).unwrap();
        assert_eq!(saved.task.as_deref(), Some("Review the diff"));
        forget("tk-h");
        assert!(ties_of("tk-h").is_none() && !reg().lock().unwrap().tasks.contains_key("tk-h"), "it goes with the helper");
        tie_back("tk-h", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", saved);
        assert_eq!(reg().lock().unwrap().tasks["tk-h"], "Review the diff");
        forget("tk-h");
        // A file from before there were tasks reads as none, and one a Luna with
        // visible helpers wrote reads all the same.
        let older: Ties = serde_json::from_value(serde_json::json!({
            "parent": null, "name": null, "settings": {}, "tools": false, "borrowed": false, "autoDelete": false, "owed": [], "seen": [],
            "opening": "go", "openingLost": true, "worktreeName": "luna-x", "worktreesBefore": ["/p/.claude/worktrees/a"],
            "worktreesLeft": ["/p/.claude/worktrees/b"]
        }))
        .unwrap();
        assert_eq!(older.task, None);
    }

    #[test]
    fn an_answer_restored_ahead_of_its_caller_waits_for_it() {
        let none = serde_json::json!({});
        // The order a core goes in: the helper first, as `restore` ties it back...
        let ties = Ties {
            parent: Some("rb-caller".into()),
            owed: vec![("rb-caller".into(), 3, 120)],
            seen: vec![("rb-caller".into(), 3)],
            ..Default::default()
        };
        tie_back("rb-helper", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", ties);
        let stands = |up_ms| owed_stands(&reg().lock().unwrap().known, "rb-caller", "rb-helper", up_ms);
        assert!(stands(0), "a caller not back yet is not one that is gone");
        assert!(stands(CALLER_RETURN_MS - 1));
        assert!(!stands(CALLER_RETURN_MS), "one that never comes back must not hold the helper from `reap` for good");
        // ... and its caller seconds later, as the window starts its CLI again.
        register("rb-caller", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none);
        assert!(stands(CALLER_RETURN_MS * 100), "known, it is owed for as long as it takes");
        let key = ("rb-caller".to_string(), "rb-helper".to_string());
        assert_eq!(reg().lock().unwrap().owed.get(&key).map(|o| (o.baseline, o.cursor)), Some((3, 120)));
        assert_eq!(last_shown("rb-caller", "rb-helper"), Some(3));
        // Deleted, it purges what it was owed as ever.
        forget("rb-caller");
        assert!(!reg().lock().unwrap().owed.contains_key(&key));
        assert!(!owed_stands(&reg().lock().unwrap().known, "rb-caller", "nobody", 0), "and an unknown session is owed nothing");
        forget("rb-helper");
    }

    #[test]
    fn trust_in_luna_survives_a_core_restart_for_that_chat_only() {
        const ID: &str = "persist-codex-trust";
        const ACCOUNT: &str = "/accounts/openai/test";
        let folder = crate::golden::project();
        let grant = crate::codex::trust::project_trust(&folder).unwrap();
        let key = super::super::registry::codex_chat_trust_key;
        let trusted = || key(ID, ACCOUNT, &folder);
        register(ID, Provider::Codex, &folder, ACCOUNT, None, false, serde_json::json!({ (CODEX_TRUST): grant }));
        assert_eq!(crate::agents::settings_of(ID).get(CODEX_TRUST), None, "the grant stays out of MCP settings");
        let mut saved = chats_now();
        saved.chats.retain(|chat| chat.id == ID);
        let mut json = serde_json::to_value(SavedChats { chats: saved.chats, ..Default::default() }).unwrap();
        assert_eq!(json["chats"][0][CODEX_TRUST], serde_json::json!(grant));
        assert_eq!(json["chats"][0]["ties"]["settings"].get(CODEX_TRUST), None);
        forget(ID);
        take_up_chats(serde_json::from_value(json.clone()).unwrap());
        assert_eq!(trusted(), Some(grant.key));
        assert_eq!(key("another-chat", ACCOUNT, &folder), None);
        assert_eq!(key(ID, "/accounts/openai/other", &folder), None);
        assert_eq!(key(ID, ACCOUNT, "/other"), None);
        forget(ID);
        assert_eq!(trusted(), None);
        assert!(chats_now().chats.iter().all(|chat| chat.id != ID), "deleting removes the saved grant");
        json["chats"][0].as_object_mut().unwrap().remove(CODEX_TRUST);
        json["chats"][0]["trustedInLuna"] = serde_json::json!(true);
        take_up_chats(serde_json::from_value(json).unwrap());
        assert_eq!(trusted(), None, "an older boolean cannot name an approved project");
        forget(ID);
        crate::activity::forget(ID);
    }

    #[test]
    fn what_the_last_core_knew_of_a_chat_comes_back_with_it() {
        let mut chat = owed_chat("tc-chat", "tc-peer", 5, 20);
        chat.provider = Provider::Codex;
        // In the platform's own separators: the account's name is its last part.
        chat.account_path = if cfg!(windows) { "C:\\acc\\openai\\two" } else { "/acc/openai/two" }.into();
        chat.ties.name = Some("the chat".into());
        // One whose answer was in, one whose turn went with the core.
        chat.ties.owed.insert(0, ("tc-other".into(), 4, 10));
        chat.ties.seen = vec![("tc-other".into(), 5)];
        take_up(
            vec![chat],
            vec![SavedSend { caller: "tc-other".into(), id: "tc-chat".into(), text: "held across".into(), going_in: None }],
            Vec::new(),
        );
        let key = |caller: &str| (caller.to_string(), "tc-chat".to_string());
        {
            let r = reg().lock().unwrap();
            let k = &r.known["tc-chat"];
            assert!(k.waiting && k.parent.is_none() && k.account == "two");
            assert_eq!(r.names.get("tc-chat").map(String::as_str), Some("the chat"));
            assert!(!r.owed[&key("tc-other")].cut_short, "its answer was in before the core went");
            assert!(r.owed[&key("tc-peer")].cut_short, "this one never will be");
            assert_eq!(r.seen.get(&key("tc-other")), Some(&5));
            assert!(r.held.iter().any(|h| h.id == "tc-chat" && h.text == "held across"));
        }
        assert_eq!(crate::activity::turns_counted("tc-chat"), Some(6), "the stopped turn counts as ended, as a helper's does");
        let k = reg().lock().unwrap().known["tc-chat"].clone();
        assert!(returning(&k, 0) && returning(&k, SESSION_RETURN_MS - 1));
        assert!(!returning(&k, SESSION_RETURN_MS), "it has had its chance");
        // Held off while it may come back; then the caller hears.
        let o = reg().lock().unwrap().owed[&key("tc-peer")];
        assert!(notice_line(crate::pty::manager(), "tc-peer", "Session", "tc-chat", &k, o).is_none(), "the window has yet to start it");
        let mut gone = k.clone();
        gone.waiting = false;
        let (said, how) = notice_line(crate::pty::manager(), "tc-peer", "Session", "tc-chat", &gone, o).unwrap();
        assert!(matches!(how, Settled::Exited));
        assert!(said.unwrap().contains("has not started again"));

        // Text going in before that word is out does not swallow it.
        owe_from("tc-peer", "tc-chat", 6, 30);
        let o = reg().lock().unwrap().owed[&key("tc-peer")];
        assert!(o.resumed && !o.cut_short && o.cursor == 20, "this turn answers; the last one's end is still to be told");

        // The window has no such row: what was kept goes.
        {
            let r = reg().lock().unwrap();
            let names: HashMap<String, String> = [("other".to_string(), "x".to_string())].into_iter().collect();
            assert!(gone_from_the_window(&r, &names).contains(&"tc-chat".to_string()));
            assert!(gone_from_the_window(&r, &HashMap::new()).is_empty(), "a window with no list yet says nothing");
            let names: HashMap<String, String> = [("tc-chat".to_string(), "x".to_string())].into_iter().collect();
            assert!(!gone_from_the_window(&r, &names).contains(&"tc-chat".to_string()));
        }

        // Started again, it is any chat, and what it keeps stays for the next core.
        register("tc-chat", Provider::Codex, "C:\\p", "C:\\acc\\openai\\two", None, false, serde_json::json!({}));
        assert!(!reg().lock().unwrap().known["tc-chat"].waiting);
        let now = chats_now();
        let c = now.chats.iter().find(|c| c.id == "tc-chat").expect("a chat with debts is saved");
        assert_eq!(c.turns_ended, 6);
        assert_eq!(c.ties.owed.len(), 2);
        assert!(now.sends.iter().any(|h| h.id == "tc-chat"));
        reg().lock().unwrap().held.retain(|h| h.id != "tc-chat");
        forget("tc-chat");
        crate::activity::forget("tc-chat");
    }

    /// A chat an earlier Luna ran as a visible helper comes back as the person's
    /// own, with no parent to stop, delete or answer for it, and none of a
    /// helper's settings; what its callers were owed stands.
    #[test]
    fn a_visible_helper_an_earlier_luna_saved_comes_back_as_the_persons_chat() {
        let mut chat = owed_chat("vm-chat", "vm-parent", 3, 40);
        chat.ties.parent = Some("vm-parent".into());
        chat.ties.settings = serde_json::json!({ "model": "sonnet", "resumeOnRestart": true, "connectors": true, "budgetTokens": 9 });
        chat.ties.auto_delete = true;
        let mut orphan = owed_chat("vm-orphan", "vm-parent", 3, 40);
        orphan.ties.orphan = true;
        take_up(vec![chat, orphan], Vec::new(), Vec::new());
        assert_eq!(parent_of("vm-chat"), None);
        {
            let r = reg().lock().unwrap();
            let k = &r.known["vm-chat"];
            assert_eq!(k.settings, serde_json::json!({ "model": "sonnet" }), "a helper's settings go");
            assert!(!r.known["vm-orphan"].orphan);
            assert!(!r.auto.contains_key("vm-chat"), "nothing deletes it but the person");
            assert!(r.owed.contains_key(&("vm-parent".to_string(), "vm-chat".to_string())), "what it was asked is still owed");
        }
        // Started again by the window, which names no parent: still nobody's.
        register("vm-chat", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, false, serde_json::json!({}));
        assert_eq!(parent_of("vm-chat"), None);
        assert!(crate::agents::registry::authorize("vm-parent", "vm-chat").is_err(), "no claim on it left");
        for id in ["vm-chat", "vm-orphan"] {
            forget(id);
            crate::activity::forget(id);
        }
    }

    /// A chat 0.19 saved with auto-handoff on comes back without its
    /// threshold, and so does one the window starts with it again: no row a
    /// tool gives says it, to the chat itself or to anyone else.
    #[test]
    fn a_chat_saved_with_auto_handoff_comes_back_without_it() {
        let (id, other) = ("ah-chat", "ah-other");
        let mut chat = owed_chat(id, "ah-peer", 2, 10);
        chat.ties.settings = serde_json::json!({ "model": "opus", "handoff": 70 });
        take_up(vec![chat], Vec::new(), Vec::new());
        assert_eq!(reg().lock().unwrap().known[id].settings, serde_json::json!({ "model": "opus" }), "taken up without it");
        let from_window = serde_json::json!({ "model": "opus", "handoff": 70 });
        register(id, Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, from_window);
        register(other, Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, serde_json::json!({}));
        for caller in [id, other] {
            let detail = super::super::list::session_get(caller, id, true, &["settings".into()]).unwrap();
            assert_eq!(detail["settings"], serde_json::json!({ "model": "opus" }), "{caller}");
        }
        for s in [id, other] {
            forget(s);
            crate::activity::forget(s);
        }
    }

    /// A chat the person started in Claude Code's auto mode is saved and taken
    /// up with it.
    #[test]
    fn a_chat_saved_in_auto_mode_comes_back_in_it() {
        let mut chat = owed_chat("am-chat", "am-peer", 2, 10);
        chat.ties.settings = serde_json::json!({ "model": "opus", "permissionMode": "auto" });
        take_up(vec![chat], Vec::new(), Vec::new());
        let settings = reg().lock().unwrap().known["am-chat"].settings.clone();
        assert_eq!(settings["permissionMode"], "auto");
        forget("am-chat");
        crate::activity::forget("am-chat");
    }

    #[test]
    fn a_chat_with_nothing_to_keep_or_a_helper_is_not_saved_as_one() {
        let none = serde_json::json!({});
        register("sv-plain", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, false, none.clone());
        let bg = serde_json::json!({ "background": true });
        register("sv-bg", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("sv-plain"), false, bg);
        register("sv-owed", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, false, none);
        owe_from("sv-plain", "sv-owed", 0, 0);
        owe_from("sv-plain", "sv-bg", 0, 0);
        let ids: Vec<String> = chats_now().chats.into_iter().map(|c| c.id).filter(|id| id.starts_with("sv-")).collect();
        assert_eq!(ids, ["sv-owed"], "helpers.json has the background one");
        for id in ["sv-plain", "sv-bg", "sv-owed"] {
            forget(id);
        }
    }

    /// A line still to be told goes into the file and comes back with the next
    /// core, once, however often it is written and read.
    #[test]
    fn a_line_still_to_be_told_outlives_a_core() {
        let line = ("nl-caller".to_string(), "The person took over your helper nl-h".to_string());
        reg().lock().unwrap().news.push(line.clone());
        assert!(chats_now().news.contains(&line));
        // Only this line is taken up again: the rest of what `chats_now` saw is
        // other tests' sessions, which a take-up would write back over theirs.
        let saved = SavedChats { news: vec![line.clone()], ..Default::default() };
        reg().lock().unwrap().news.retain(|l| *l != line);
        let back: SavedChats = serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
        let again: SavedChats = serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
        take_up_chats(back);
        take_up_chats(again);
        assert_eq!(reg().lock().unwrap().news.iter().filter(|l| **l == line).count(), 1);
        reg().lock().unwrap().news.retain(|l| *l != line);
        let older: SavedChats = serde_json::from_str(r#"{"chats":[],"sends":[]}"#).unwrap();
        assert!(older.news.is_empty(), "a file from before reads as none");
    }

    /// What 0.19's handoff and moves queued for a chat, word for word as it
    /// wrote them, is not typed after the upgrade (ADR-0030); a line of
    /// anything else queued beside them still is.
    #[test]
    fn a_line_0_19_queued_for_a_handoff_is_dropped_as_a_core_takes_it_up() {
        let chat = "rh-chat";
        let retired = [
            "This chat's context is at 72% (auto-handoff at 70%): hand off to a fresh conversation at the next point where your work can pause. luna_session_handoff without arguments says how.",
            "The person asks for a handoff of this chat now: hand off to a fresh conversation at the next point where your work can pause. luna_session_handoff without arguments says how.",
            "The person's request for a handoff of this chat lapsed: no handoff came within 10 minutes of it. luna_session_handoff refuses again until Luna asks for one; go on as you were.",
            "The person asked for a handoff of this chat, but Luna could not deliver the request within 10 minutes, so it is withdrawn. luna_session_handoff refuses until Luna asks for one; go on as you were.",
            "Handoff 2 cleared the conversation, but Luna's prompt for the new one did not go in (timed out). It was:\n[Luna handoff 2, not typed by the user] This is a new conversation of this chat.",
            "Handoff 3 did not go through: it is busy. This conversation goes on as it was; your handoff's copy is at C:\\d\\handoffs\\c\\3.md. Call luna_session_handoff again once what stood in the way is done.",
            "A handoff of this chat cleared the conversation that wrote it, but Luna's prompt for the new one did not go in, and this chat may be on another conversation since, so that prompt is not typed here.",
            "[Luna notice, not typed by the user] Handoff 1 did not go through: no file. Luna's copy of it stays at C:\\d\\1.md.",
            "[Luna move 3fa9c2, not typed by the user] This chat goes on from…",
        ];
        let kept = (chat.to_string(), "Your helper h1 answered.".to_string());
        let mut news: Vec<(String, String)> = retired.iter().map(|l| (chat.to_string(), l.to_string())).collect();
        news.push(kept.clone());
        take_up_chats(SavedChats { news, ..Default::default() });
        let queued: Vec<(String, String)> = reg().lock().unwrap().news.iter().filter(|(c, _)| c == chat).cloned().collect();
        assert_eq!(queued, [kept], "only the line that is not the handoff's");
        reg().lock().unwrap().news.retain(|(c, _)| c != chat);
        for line in [
            "Handoffs are gone.",
            "Handoff the keys to the reviewer",
            "Handoff 2FA ownership to support",
            "Your helper said: This chat's context is at 72%",
            "[Luna movement notes] for the release",
            "[Luna handoff notes] the plan",
        ] {
            assert!(!of_retired_handoff(line), "{line}");
        }
        // A file this core wrote is no 0.19 file: what it queued goes in as it is.
        let line = (chat.to_string(), retired[0].to_string());
        take_up_chats(SavedChats { news: vec![line.clone()], no_handoff: true, ..Default::default() });
        assert!(reg().lock().unwrap().news.contains(&line), "kept");
        reg().lock().unwrap().news.retain(|(c, _)| c != chat);
    }

    /// The line 0.19 added to a notice for a session above a lead of the
    /// leads below it that handed off goes, alone or after what else the
    /// notice said, which stays.
    #[test]
    fn the_handed_off_line_0_19_added_to_a_notice_goes_and_the_rest_stays() {
        let one = "Lead l2 handed off (1): it goes on in a fresh conversation; its helpers stay with it.";
        let many = "Leads handed off: l2 (1), l3 (2): each goes on in a fresh conversation; their helpers stay with them.";
        let update = format!("{} Your helper h1 answered: lane 2 is green.", crate::agents::NOTICE_TAG);
        assert_eq!(without_handed_off(&format!("{update} {one}")).as_deref(), Some(update.as_str()));
        assert_eq!(without_handed_off(&format!("{update} {many}")).as_deref(), Some(update.as_str()));
        assert_eq!(without_handed_off(&format!("{} {one}", crate::agents::NOTICE_TAG)), None, "nothing else in it");
        assert_eq!(without_handed_off(&update).as_deref(), Some(update.as_str()), "a notice without it as it was");
        let plain = "Lead the review: it goes on in a fresh conversation; its helpers stay with it.";
        assert_eq!(without_handed_off(plain).as_deref(), Some(plain), "not 0.19's line");
    }

    #[test]
    fn text_held_across_a_core_waits_for_the_chat_to_come_back() {
        take_up(
            vec![owed_chat("ho-chat", "ho-parent", 3, 40)],
            vec![SavedSend { caller: "ho-parent".into(), id: "ho-chat".into(), text: "and this".into(), going_in: None }],
            Vec::new(),
        );
        assert!(is_held("ho-parent", "ho-chat"));
        // Not running here, and so the same in either case while it may yet return.
        deliver_held(crate::pty::manager());
        assert!(is_held("ho-parent", "ho-chat"));
        reg().lock().unwrap().held.retain(|h| h.id != "ho-chat");
        forget("ho-chat");
    }

    #[test]
    fn a_helper_a_later_core_finds_done_is_not_owed_again() {
        let _one = spawns_to_myself();
        register("fd-caller", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, serde_json::json!({}));
        let bg = serde_json::json!({ "background": true });
        // The same spawn as a third core finds it: under the second the helper went on,
        // finished, and its caller was told; the first left it no debt.
        register("fd-done", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("fd-caller"), false, bg.clone());
        let placed = SpawnTaken { owed_placed: true, ..spawn_taken(9_000_600, "fd-caller", Some("fd-done"), true) };
        // Under the first core: owed as any helper is.
        register("fd-ans", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("fd-caller"), false, bg);
        let first = spawn_taken(9_000_601, "fd-caller", Some("fd-ans"), true);
        take_up(Vec::new(), Vec::new(), vec![placed, first]);
        take_up_spawns();
        {
            let r = reg().lock().unwrap();
            assert!(!r.owed.contains_key(&("fd-caller".to_string(), "fd-done".to_string())), "told already");
            assert!(r.owed.contains_key(&("fd-caller".to_string(), "fd-ans".to_string())));
            assert!(r.spawns[&9_000_601].owed_placed, "and not again under the next");
        }
        reg().lock().unwrap().spawns.retain(|request, _| ![9_000_600, 9_000_601].contains(request));
        forget("fd-done");
        forget("fd-ans");
        forget("fd-caller");
    }
}
