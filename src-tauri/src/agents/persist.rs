//! What outlives a core. A background helper's ties go with its record
//! (background/); what the person's chats are owed and held, the spawns under
//! way and the lines still to be told go into `chat-ties.json`, and the next
//! core takes both up.

use crate::agents::held::{GoingIn, Held};
use crate::agents::owed::Owed;
use crate::agents::read::transcripts;
use crate::agents::registry::{account_name, issue_token, label, reg, register, Known, Registry, DEV_CALLER};
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
/// earlier Luna wrote may hold more (a visible helper's opening, its worktree's
/// name): what is not read here is let go.
#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Ties {
    pub parent: Option<String>,
    pub name: Option<String>,
    pub settings: serde_json::Value,
    pub tools: bool,
    pub borrowed: bool,
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
    /// See `Registry::worktrees_before`: written only until its CLI has said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktrees_before: Option<Vec<String>>,
    /// See `Registry::worktrees_left`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub worktrees_left: Vec<String>,
    /// See `Registry::tasks`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// Its `autoDelete` was switched on by its chat's going (`Auto::by_pass_up`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto_delete_passed_up: bool,
    /// The person's last move of it to another chat (reparent.rs `Moved`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moved: Option<crate::agents::reparent::Moved>,
}

/// One helper's ties, as a save keeps them (`ties_of_each`).
#[cfg(test)]
pub(super) fn ties_of(id: &str) -> Option<Ties> {
    ties_in(&reg().lock().unwrap_or_else(|e| e.into_inner()), id)
}

/// `ties_of` each of `ids`, in one hold of the registry: a save never keeps one
/// helper of a line as before a change of several (a move, reparent.rs) and
/// another as after it.
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
        borrowed: r.borrowed.contains(id),
        auto_delete: r.auto.contains_key(id),
        orphan: k.orphan,
        owed: r.owed.iter().filter(|((_, s), _)| s == id).map(|((c, _), o)| (c.clone(), o.baseline, o.cursor)).collect(),
        seen: r.seen.iter().filter(|((_, s), _)| s == id).map(|((c, _), n)| (c.clone(), *n)).collect(),
        cut_short: r.owed.iter().filter(|((_, s), o)| s == id && o.cut_short).map(|((c, _), _)| c.clone()).collect(),
        told_pending: r.owed.iter().filter(|((_, s), o)| s == id && o.told_pending).map(|((c, _), _)| c.clone()).collect(),
        worktrees_before: r.worktrees_before.get(id).cloned(),
        worktrees_left: r.worktrees_left.get(id).cloned().unwrap_or_default(),
        task: r.tasks.get(id).cloned(),
        auto_delete_passed_up: r.auto.get(id).is_some_and(|a| a.by_pass_up),
        moved: r.moved.get(id).cloned(),
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
    let token = token.or_else(|| (provider == Provider::Claude).then(|| issue_token(id)));
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(name) = ties.name {
        r.names.insert(id.to_string(), name);
    }
    if let Some(k) = r.known.get_mut(id) {
        k.orphan = ties.orphan;
    }
    if ties.borrowed {
        r.borrowed.insert(id.to_string());
    }
    // Its idle time counts afresh: nobody has had the chance to look at it.
    if ties.auto_delete {
        r.auto.insert(id.to_string(), Auto { by_pass_up: ties.auto_delete_passed_up, ..Auto::default() });
    }
    if let Some(moved) = ties.moved {
        r.moved.insert(id.to_string(), moved);
    }
    for (caller, baseline, cursor) in ties.owed {
        let o = told_job_went(&ties.told_pending, &caller, baseline, cursor).unwrap_or_else(|| Owed::from(baseline, cursor));
        r.owed.insert((caller, id.to_string()), o);
    }
    for (caller, n) in ties.seen {
        r.seen.insert((caller, id.to_string()), n);
    }
    if let Some(before) = ties.worktrees_before {
        r.worktrees_before.insert(id.to_string(), before);
    }
    if !ties.worktrees_left.is_empty() {
        r.worktrees_left.insert(id.to_string(), ties.worktrees_left);
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
}

fn chat_ties_file() -> std::path::PathBuf {
    crate::paths::data_dir().join("chat-ties.json")
}

/// Set once `restore_chats` has read the file: an empty registry before it is
/// not to be written over what the last core left.
static CHATS_READ: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// What the last write says stands, for the core after this one. Set as every
/// session is stopped (a core leaving, an update): a turn asked to quit is a turn
/// ended to the sampler, Codex writing an abort for it that counts as its end, and
/// written down that would have the next core take it for one that finished.
pub(super) static CHATS_FROZEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// What `save_chats` last wrote, and the lock its saves take in turn
/// (`save_in_order`).
static CHATS_LAST: Mutex<Vec<u8>> = Mutex::new(Vec::new());

/// How many stops of every session are under way (`freeze_chats`): while
/// one is, a session starting does not lift the freeze (`thaw_chats`).
static WINDING: Mutex<usize> = Mutex::new(0);

/// Every session being stopped, from `freeze_chats` until it is dropped.
pub struct Winding(());

impl Drop for Winding {
    fn drop(&mut self) {
        let over = {
            let mut winding = WINDING.lock().unwrap_or_else(|e| e.into_inner());
            *winding = winding.saturating_sub(1);
            *winding == 0
        };
        // A session still running once the stop is over came after it had
        // taken the sessions, started while it was under way: it escaped
        // the stop and runs on, and its registration, which found the stop
        // counted, left the freeze. Lifted here, as a start after the stop
        // lifts it; one listed after this lifts it itself (`thaw_chats`).
        if over && crate::pty::manager().live_count() > 0 {
            thaw_chats();
        }
    }
}

/// Freezes the chats' ties for a stop of every session (a core leaving, the
/// PC shutting down, an update), held until the guard is dropped once the
/// sessions are stopped. The freeze itself stays until a session starts
/// after that: the core goes on where an update or a shutdown did not come.
#[must_use = "the stop lasts as long as the guard"]
pub fn freeze_chats() -> Winding {
    // Counted before the freeze, and neither under the other's lock: a start
    // that finds no stop counted thaws before the freeze, never after it.
    *WINDING.lock().unwrap_or_else(|e| e.into_inner()) += 1;
    freeze(&CHATS_LAST, &CHATS_FROZEN);
    Winding(())
}

/// A session running again: what the chats' ties say is news once more,
/// unless every session is being stopped, where it is one more that will
/// be, or one the stop's end finds running (`Winding`), and the freeze
/// holds. Called as a session registers, under the registry's lock, and
/// once it is listed in the session map; nothing takes either under
/// `WINDING`.
pub(crate) fn thaw_chats() {
    let winding = WINDING.lock().unwrap_or_else(|e| e.into_inner());
    if *winding == 0 {
        CHATS_FROZEN.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// One test at a time that stops every session or reads the freeze: each
/// counts on no other stop being under way.
#[cfg(test)]
pub(crate) fn stops_for_test() -> std::sync::MutexGuard<'static, ()> {
    static ONE: Mutex<()> = Mutex::new(());
    ONE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Whether the chats' ties are frozen, and, given `to`, set so first.
#[cfg(test)]
pub(crate) fn chats_frozen(to: Option<bool>) -> bool {
    use std::sync::atomic::Ordering;
    if let Some(to) = to {
        CHATS_FROZEN.store(to, Ordering::SeqCst);
    }
    CHATS_FROZEN.load(Ordering::SeqCst)
}

/// Sets `frozen` under the saves' lock: a save already taking its snapshot
/// or writing it is through before the sessions are stopped, so nothing of
/// the stopping goes into a snapshot or onto disk after the freeze.
fn freeze(last: &Mutex<Vec<u8>>, frozen: &std::sync::atomic::AtomicBool) {
    let _saving = last.lock().unwrap_or_else(|e| e.into_inner());
    frozen.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// What there is to keep of the sessions the window runs, and the rest the next
/// core is to tell. A chat nobody wrote to has nothing to keep: it comes back as
/// it was.
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
                    && id.as_str() != DEV_CALLER
                    && (r.owed.keys().any(|(_, s)| s == *id) || sends.iter().any(|h| h.id == **id))
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
    SavedChats { chats, sends, spawns, news }
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
/// wrote behind it would leave the older one on disk (the notifier's, a
/// handoff's and the periodic save run at once), for a core killed before
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
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    for c in saved.chats {
        let ties = c.ties;
        // A chat the window runs is the person's own: one an earlier Luna had
        // spawned as a visible helper, or had a helper of passed to, is theirs now,
        // and nobody's to stop, delete or answer for. What it owes callers stands.
        if ties.parent.is_some() || ties.orphan {
            migrated.push(c.id.clone());
        }
        let mut settings = ties.settings;
        if let Some(o) = settings.as_object_mut() {
            for key in HELPER_SETTINGS {
                o.remove(key);
            }
        }
        r.known.entry(c.id.clone()).or_insert(Known {
            provider: c.provider,
            account: account_name(&c.account_path),
            folder: c.folder,
            account_path: c.account_path,
            parent: None,
            settings,
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
        r.held.push(Held { caller: h.caller, id: h.id, text: h.text, since: now_ms(), told: false, going_in: h.going_in });
    }
    for mut s in saved.spawns {
        s.left_by_core = true;
        r.next_request = r.next_request.max(s.request.saturating_add(1));
        r.spawns.insert(s.request, s);
    }
    for line in saved.news {
        if !r.news.contains(&line) {
            r.news.push(line);
        }
    }
    drop(r);
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

/// What only a helper's settings hold (`registry::CORE_SETTINGS`, `background`):
/// a chat of the person's has none of them.
pub(super) const HELPER_SETTINGS: [&str; 8] =
    ["background", "resumeOnRestart", "budgetTokens", "budgetPct", "budgetBreach", "budgetTold", "connectors", "constraints"];

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
    /// asking for its chat again, a helper's turn) leaves the chats' ties
    /// frozen: a save then would write what the stopping did. One that
    /// starts once the stop is over lifts the freeze.
    #[test]
    fn a_start_while_the_sessions_are_stopped_leaves_the_chats_frozen() {
        use std::sync::atomic::Ordering;
        let _alone = stops_for_test();
        let start = || register("fz-start", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, false, serde_json::json!({}));
        let stopping = freeze_chats();
        start();
        assert!(CHATS_FROZEN.load(Ordering::SeqCst), "still frozen");
        drop(stopping);
        start();
        assert!(!CHATS_FROZEN.load(Ordering::SeqCst), "a start after the stop lifts it");
        forget("fz-start");
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
            "opening": "go", "openingLost": true, "worktreeName": "luna-x", "worktreesBefore": []
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
        chat.ties.borrowed = true;
        let mut orphan = owed_chat("vm-orphan", "vm-parent", 3, 40);
        orphan.ties.orphan = true;
        take_up(vec![chat, orphan], Vec::new(), Vec::new());
        assert_eq!(parent_of("vm-chat"), None);
        {
            let r = reg().lock().unwrap();
            let k = &r.known["vm-chat"];
            assert_eq!(k.settings, serde_json::json!({ "model": "sonnet" }), "a helper's settings go");
            assert!(!r.known["vm-orphan"].orphan);
            assert!(!r.auto.contains_key("vm-chat") && !r.borrowed.contains("vm-chat"), "nothing deletes it but the person");
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
