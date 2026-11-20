//! Whether Luna starts anything new now: one gate, closed for either of two
//! reasons, and asked by every start of a session's work — a helper's turn
//! (`background::run`, a new helper's first one included), Luna's own writes
//! into a pane (`pty::write`) and, for an update only, a pane's CLI
//! (`pty::spawn`, both CLIs; `launching` says why).
//!
//! - **Power**: a power-off's countdown has run out and is being decided, or
//!   what it fired is under way (power/ `close`, which sets `POWER` under the
//!   power lock with its own `State::closing`).
//! - **Update**: every session is being stopped for an update, or has been and
//!   the update is not called off (agents/persist.rs `Stop::Update`, which
//!   sets it with its bit): from before the stop takes its snapshot of what
//!   runs until the updated window replaces the core, the window says the
//!   install failed, the stop panics, or a window of this build comes with
//!   the one that asked gone (agents `window_came`). A start refused then is not lost: a helper's batch waits in its
//!   queue (kept for its senders by the next core, or run once the gate opens
//!   again), a pane is started again by the updated window.
//!
//! A helper's turn is claimed in the helpers' registry before it asks, so a
//! stop that closes the gate before it lists the turns either sees the claim
//! or is seen by the gate. A pane is registered only once its process is up,
//! so its launch is counted from before it asks to after it is registered
//! (`launching`); the update's stop waits for those under way (`seal`), then
//! takes its snapshot. One that outlasts the wait is no session the person
//! was asked about, nor one the stop may end: the stop is refused and the
//! update called off, nothing sealed, and the launch registers as it would
//! have. Never a session the stop did not see, and none it ends unasked.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard};

/// What everything the gate refuses for a power-off says first.
pub const SHUTTING_DOWN: &str = "Luna is shutting down";
/// What everything the gate refuses for an update says first.
pub const UPDATING: &str = "Luna is updating";

/// Closed for a power-off (power/ `close`): set and cleared under the power
/// lock with `State::closing`, read here with none, so it may be asked under
/// any lock.
pub(crate) static POWER: AtomicBool = AtomicBool::new(false);
/// Closed for an update (`set_update`), set and cleared under the stops' lock
/// (agents/persist.rs) with `Stop::Update`'s bit.
static UPDATE: AtomicBool = AtomicBool::new(false);

/// Why `id`'s work may not start now, if it may not. Takes no lock.
fn refusal(id: &str) -> Option<String> {
    if POWER.load(Ordering::SeqCst) {
        return Some(format!("{SHUTTING_DOWN}: the PC is about to be powered off or put to sleep, so nothing new starts."));
    }
    update_refusal(id)
}

fn update_refusal(id: &str) -> Option<String> {
    (UPDATE.load(Ordering::SeqCst) && under_update(id)).then(updating)
}

/// What a start the update refuses is told.
pub fn updating() -> String {
    format!("{UPDATING}: every session is being stopped for the update to be installed, so nothing new starts.")
}

/// Whether `id`'s work may start now. Takes no lock.
pub fn open(id: &str) -> bool {
    refusal(id).is_none()
}

/// Whether work that names no session may start now: the gate's own word,
/// outside tests. In a test only what it names is under the update
/// (`under_update_for_test`), so this is open to every update there.
pub fn open_to_all() -> bool {
    !POWER.load(Ordering::SeqCst) && (cfg!(test) || !UPDATE.load(Ordering::SeqCst))
}

/// `open` as the error the work it refuses is given.
pub fn open_or_refuse(id: &str) -> Result<(), String> {
    refusal(id).map_or(Ok(()), Err)
}

/// Whether `err` is the gate's: work that was refused, not work that failed.
pub fn refused(err: &str) -> bool {
    err.starts_with(SHUTTING_DOWN) || err.starts_with(UPDATING)
}

/// One reason let go of: once neither holds, the helper turns refused meanwhile
/// start on what waited (`background::resume_waiting`), on a thread of their
/// own, as this runs under the power lock or the stops', and a turn's start
/// takes the helpers' registry lock. Not in a test, where a turn would be real.
pub(crate) fn reopened() {
    #[cfg(not(test))]
    if !POWER.load(Ordering::SeqCst) && !UPDATE.load(Ordering::SeqCst) {
        std::thread::spawn(crate::background::resume_waiting);
    }
}

/// Closes the gate for an update or opens it again. Opened, the launches it
/// sealed off (`seal`) may register again.
pub(crate) fn set_update(closed: bool) {
    let was = UPDATE.swap(closed, Ordering::SeqCst);
    if !closed {
        launches().sealed = false;
        if was {
            reopened();
        }
    }
}

/// The pane launches counted under the gate.
struct Launches {
    /// From `launching` to the ticket's drop, its registration included.
    under_way: usize,
    /// The update's stop has taken, or is about to take, its snapshot of the
    /// panes (`seal`), none being under way then: one counted after it,
    /// which the closed gate does not let happen, is refused as it would
    /// create or register.
    sealed: bool,
}

static LAUNCHES: Mutex<Launches> = Mutex::new(Launches { under_way: 0, sealed: false });
/// Told when a launch's ticket is dropped.
static LAUNCH_ENDED: Condvar = Condvar::new();

fn launches() -> MutexGuard<'static, Launches> {
    LAUNCHES.lock().unwrap_or_else(|e| e.into_inner())
}

/// A pane's launch under way, let in by the gate, until it is dropped: an
/// update's stop waits for it before its snapshot (`seal`).
#[must_use = "the launch is counted for as long as the ticket lives"]
pub struct Launching {
    counted: bool,
}

/// Lets `id`'s pane launch in, counted until the ticket is dropped, or refuses
/// it for an update. Asked under the launches' lock, so a stop that seals
/// after it has closed the gate either waits for this launch or this launch
/// was refused. A power-off does not refuse it: a pane is started by a person,
/// whom a power-off never refuses, and its CLI's start is activity that
/// cancels the power-off's decision (power/ `barrier`).
pub fn launching(id: &str) -> Result<Launching, String> {
    let mut l = launches();
    if let Some(why) = update_refusal(id) {
        return Err(why);
    }
    let counted = under_update(id);
    if counted {
        l.under_way += 1;
    }
    Ok(Launching { counted })
}

impl Launching {
    /// Creates the launch's process (`create`) under the launches' lock,
    /// unless the update's stop has sealed the launches off without it (a
    /// guard: it seals only with none under way, and the closed gate lets
    /// none in after): then nothing is created. The seal takes the same
    /// lock, so no process is created once it has, nor after the stop has
    /// returned.
    pub fn create<T>(&self, create: impl FnOnce() -> T) -> Result<T, String> {
        let l = launches();
        if l.sealed && self.counted {
            return Err(updating());
        }
        let made = create();
        drop(l);
        Ok(made)
    }

    /// Puts `session` where the stop's snapshot finds it (`put`), under the
    /// launches' lock, unless the update's stop has sealed the launches off
    /// without it (the guard `create` keeps): then it is handed back, for the
    /// caller to end.
    pub fn register<T>(&self, session: T, put: impl FnOnce(T)) -> Result<(), T> {
        let l = launches();
        if l.sealed && self.counted {
            return Err(session);
        }
        put(session);
        drop(l);
        Ok(())
    }
}

impl Drop for Launching {
    fn drop(&mut self) {
        if self.counted {
            let mut l = launches();
            l.under_way = l.under_way.saturating_sub(1);
            drop(l);
            LAUNCH_ENDED.notify_all();
        }
    }
}

/// The update's stop, the gate closed by now: waits up to `wait` for the pane
/// launches under way to register, then seals them off, so the snapshot it
/// takes next holds every pane that will ever run before the install. Where
/// some are still under way when the wait is up, each a CLI started that
/// will register as a session nobody was asked about, nothing is sealed:
/// Err with how many, and the stop goes no further (pty
/// `stopped_for_update`), so they register as they would have.
pub fn seal(wait: std::time::Duration) -> Result<(), usize> {
    let deadline = std::time::Instant::now() + wait;
    let mut l = launches();
    while l.under_way > 0 {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            break;
        }
        l = LAUNCH_ENDED.wait_timeout(l, left).unwrap_or_else(|e| e.into_inner()).0;
    }
    let late = l.under_way;
    l.sealed = late == 0;
    drop(l);
    if late > 0 {
        crate::log::warn("update", &format!("{late} pane launch(es) not registered within {wait:?}; the update's stop goes no further"));
        return Err(late);
    }
    Ok(())
}

/// Whether the update reason is `id`'s: every id's, outside tests. A test
/// closes the gate for the ids it names (`under_update_for_test`) only, as
/// tests that start turns and panes run beside it.
#[cfg(not(test))]
fn under_update(_id: &str) -> bool {
    true
}

#[cfg(test)]
static UNDER_UPDATE: Mutex<Vec<String>> = Mutex::new(Vec::new());

#[cfg(test)]
fn under_update(id: &str) -> bool {
    UNDER_UPDATE.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|i| i == id)
}

/// Has the update reason apply to `id` in this test run.
#[cfg(test)]
pub(crate) fn under_update_for_test(id: &str) {
    UNDER_UPDATE.lock().unwrap_or_else(|e| e.into_inner()).push(id.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A launch let in before the update closed the gate is waited for and
    /// registers; one after it is refused up front, and a refusal leaves
    /// nothing counted.
    #[test]
    fn a_launch_under_way_is_waited_for_and_one_after_the_close_is_refused() {
        let _alone = crate::agents::stops_for_test();
        let id = "sg-waited";
        under_update_for_test(id);
        let early = launching(id).expect("open");
        set_update(true);
        assert!(launching(id).err().unwrap().starts_with(UPDATING));
        assert!(refused(&open_or_refuse(id).unwrap_err()));
        assert!(open("sg-elsewhere"), "a test's gate is its own ids'");
        std::thread::scope(|s| {
            let sealing = s.spawn(|| seal(std::time::Duration::from_secs(30)));
            std::thread::sleep(std::time::Duration::from_millis(50));
            assert!(!sealing.is_finished(), "the stop waits for the launch under way");
            let mut map = Vec::new();
            assert!(early.register("pane", |p| map.push(p)).is_ok(), "registered before the seal");
            assert_eq!(map, ["pane"]);
            drop(early);
            assert_eq!(sealing.join().unwrap(), Ok(()));
        });
        assert_eq!(launches().under_way, 0);
        set_update(false);
        assert!(open(id));
        assert!(!launches().sealed, "opened, unsealed");
    }

    /// One that outlasts the wait leaves the launches unsealed, the seal
    /// saying how many: it creates its process and registers as it would
    /// have, and the gate opened again lets launches through.
    #[test]
    fn a_launch_that_outlasts_the_wait_leaves_the_launches_unsealed() {
        let _alone = crate::agents::stops_for_test();
        let id = "sg-late";
        under_update_for_test(id);
        let late = launching(id).expect("open");
        set_update(true);
        assert_eq!(seal(std::time::Duration::from_millis(10)), Err(1));
        assert!(!launches().sealed);
        assert_eq!(late.create(|| "process"), Ok("process"));
        let mut map: Vec<&str> = Vec::new();
        assert_eq!(late.register("pane", |p| map.push(p)), Ok(()));
        assert_eq!(map, ["pane"]);
        drop(late);
        set_update(false);
        let again = launching(id).expect("open again");
        assert!(again.register("pane", |p| map.push(p)).is_ok());
    }
}
