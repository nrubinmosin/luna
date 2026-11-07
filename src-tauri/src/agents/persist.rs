//! What outlives a core. A background helper's ties go with its record
//! (background.rs), the visible chats' and the spawns under way into
//! `chat-ties.json`, and the next core takes both up.

use crate::agents::delivery::{words, GoingIn, Held, NEEDLE};
use crate::agents::owed::Owed;
use crate::agents::read::{transcript_path, transcripts};
use crate::agents::registry::{account_name, issue_token, label, reg, register, Known, Registry, DEV_CALLER};
use crate::agents::resume::{said_to, ChatResume, WentIn};
use crate::agents::said::messages_at;
use crate::agents::spawn::SpawnTaken;
use crate::agents::teardown::Auto;
use crate::provider::Provider;
use crate::throttle::now_ms;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// How long into a core's life a chat the last core knew (`restore_chats`) is
/// given to start again before whoever is owed by it hears it has not: the
/// window starts the ones in panes within seconds, the rest when opened.
pub(super) const SESSION_RETURN_MS: u64 = 60_000;

/// What a visible helper was first asked, word for word, kept until it has a
/// conversation that holds it: for its CLI should it come back with none, and for
/// its caller should nothing then take it up.
pub(super) struct Opening {
    pub(super) text: String,
    /// The turn count that says nothing of it: one under this core past that has a
    /// conversation to go on with. A count kept across a core may be only a turn
    /// that went with it, and 1 or more here is that (`take_up_chats`).
    pub(super) floor: u32,
    /// Its CLI came back afresh, with no conversation, and nothing has been given
    /// this since: its caller is told (`first_turn_lost`).
    pub(super) lost: bool,
    /// When it was asked: a conversation that holds this and began since is its own,
    /// should nothing have said so before a core went (`conversation_on_disk`).
    pub(super) at_ms: u64,
}

/// What the agent layer holds of a background helper, saved with it
/// (background.rs `save`) so a core started later takes it up where this one
/// left it: whose it is, what it is called, and what its callers are owed.
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
    /// See `Opening`: written only until a first turn has ended, or its caller has
    /// heard it was lost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opening: Option<String>,
    /// See `Opening::lost`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub opening_lost: bool,
    /// See `Opening::at_ms`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opening_at: Option<u64>,
    /// See `Registry::worktree_names`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_name: Option<String>,
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
}

pub fn ties_of(id: &str) -> Option<Ties> {
    ties_in(&reg().lock().unwrap_or_else(|e| e.into_inner()), id)
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
        opening: r.openings.get(id).map(|o| o.text.clone()),
        opening_lost: r.openings.get(id).is_some_and(|o| o.lost),
        opening_at: r.openings.get(id).map(|o| o.at_ms),
        worktree_name: r.worktree_names.get(id).cloned(),
        told_pending: r.owed.iter().filter(|((_, s), o)| s == id && o.told_pending).map(|((c, _), _)| c.clone()).collect(),
        worktrees_before: r.worktrees_before.get(id).cloned(),
        worktrees_left: r.worktrees_left.get(id).cloned().unwrap_or_default(),
        task: r.tasks.get(id).cloned(),
    })
}

/// An answer a caller had been told was not done yet, as a core that saved it
/// is followed by the next: that turn was over, and what it had left running
/// went with the core. The same for a background helper and a visible one;
/// None for any other answer owed.
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
        r.auto.insert(id.to_string(), Auto::default());
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

/// A session the window runs, as `<data>/chat-ties.json` keeps it for the core
/// started after this one, as `helpers.json` does a background helper's: whose
/// it is, what it was handed and what its callers are owed. The window starts
/// it again, and `register` takes it from there.
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
    /// Its turn was running as this was written, or was yet to be carried on
    /// (`Registry::resumes`).
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

pub fn freeze_chats() {
    CHATS_FROZEN.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// What there is to keep of the sessions the window runs. A chat nobody spawned,
/// wrote to or handed anything has nothing to keep: it comes back as it was.
pub(super) fn chats_now() -> SavedChats {
    let (mut chats, sends, mut spawns, floors) = {
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
                    && (k.parent.is_some()
                        || k.orphan
                        || r.auto.contains_key(*id)
                        || r.borrowed.contains(*id)
                        || r.owed.keys().any(|(_, s)| s == *id)
                        || sends.iter().any(|h| h.id == **id))
            })
            .filter_map(|(id, k)| {
                Some(SavedChat {
                    id: id.clone(),
                    provider: k.provider,
                    folder: k.folder.clone(),
                    account_path: k.account_path.clone(),
                    turns_ended: 0,
                    transcript: None,
                    mid_turn: r.resumes.contains_key(id),
                    ties: ties_in(&r, id)?,
                })
            })
            .collect();
        let floors: HashMap<String, u32> = r.openings.iter().filter(|(_, o)| !o.lost).map(|(id, o)| (id.clone(), o.floor)).collect();
        (chats, sends, r.spawns.values().filter(|s| !s.retried).cloned().collect::<Vec<SpawnTaken>>(), floors)
    };
    let pty = crate::pty::manager();
    for c in &mut chats {
        c.turns_ended = crate::activity::turns_counted(&c.id).unwrap_or(0);
        let first_turn = floors.get(&c.id).filter(|floor| c.turns_ended <= **floor).is_some();
        // Where its conversation is, while it may be its first: a core after this
        // goes by that as to whether it has one to come back to (`first_turn_lost`).
        if first_turn {
            transcript_path(pty, &c.id);
        }
        c.transcript = transcripts().lock().unwrap_or_else(|e| e.into_inner()).get(&c.id).cloned();
        c.mid_turn |= crate::activity::turn_state(&c.id)
            .is_some_and(|t| matches!(t.turn, crate::activity::Turn::Busy | crate::activity::Turn::Waiting));
        if !first_turn && floors.contains_key(&c.id) {
            c.ties.opening = None;
            c.ties.worktrees_before = None;
        }
    }
    chats.sort_by(|a, b| a.id.cmp(&b.id));
    spawns.sort_by_key(|s| s.request);
    SavedChats { chats, sends, spawns }
}

/// Writes `chats_now` out if it changed since the last write (background.rs
/// `save` keeps the beat).
pub fn save_chats() {
    use std::sync::atomic::Ordering;
    static LAST: Mutex<Vec<u8>> = Mutex::new(Vec::new());
    if !CHATS_READ.load(Ordering::SeqCst) || CHATS_FROZEN.load(Ordering::SeqCst) {
        return;
    }
    let Ok(bytes) = serde_json::to_vec_pretty(&chats_now()) else { return };
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if *last == bytes {
        return;
    }
    let path = chat_ties_file();
    match crate::secret::write_private(&path, &bytes) {
        Ok(()) => *last = bytes,
        Err(e) => crate::log::warn("agents", &format!("could not save the chats' ties to {}: {e}", path.display())),
    }
}

/// Takes up what the last core saved of the sessions the window runs, before
/// anything can ask after them. They are known again as they were, lineage
/// and all, though not running until the window starts them (`Known::waiting`).
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
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    for c in saved.chats {
        let ties = c.ties;
        let resumes = ties.settings["resumeOnRestart"] == true;
        r.known.entry(c.id.clone()).or_insert(Known {
            provider: c.provider,
            account: account_name(&c.account_path),
            folder: c.folder,
            account_path: c.account_path,
            parent: ties.parent,
            settings: ties.settings,
            tools: ties.tools,
            waiting: true,
            orphan: ties.orphan,
        });
        if let Some(name) = ties.name {
            r.names.insert(c.id.clone(), name);
        }
        if ties.borrowed {
            r.borrowed.insert(c.id.clone());
        }
        if ties.auto_delete {
            r.auto.insert(c.id.clone(), Auto::default());
        }
        // An answer not in by the time the core went will not come: the CLI
        // it was being written by went with it, and the one the window starts
        // is between turns. That turn counts as ended, as a helper's does.
        let mut count = c.turns_ended;
        // One cut short under an earlier core and not carried on by now was called
        // off (`call_off_resume`): the mark would still be there otherwise.
        let mut went_now = c.mid_turn;
        for (caller, baseline, cursor) in ties.owed {
            // Its turn was over, told so: what it left went with the core. Not
            // one it was in the middle of again: that turn is cut short below.
            if let Some(o) = told_job_went(&ties.told_pending, &caller, baseline, cursor).filter(|_| !c.mid_turn) {
                r.owed.insert((caller, c.id.clone()), o);
                continue;
            }
            let mut o = Owed::from(baseline, cursor);
            let already = ties.cut_short.contains(&caller);
            if c.turns_ended <= baseline || already {
                o.cut_short = true;
                count = count.max(baseline.saturating_add(1));
                went_now |= !already;
            }
            r.owed.insert((caller, c.id.clone()), o);
        }
        for (caller, n) in ties.seen {
            r.seen.insert((caller, c.id.clone()), n);
        }
        if let Some(opening) = ties.opening {
            // As asked: a file an older core wrote has it as typed, with any note.
            let text = crate::pty::without_note(&opening).to_string();
            r.openings.insert(c.id.clone(), Opening { text, floor: count, lost: ties.opening_lost, at_ms: ties.opening_at.unwrap_or(0) });
        }
        if let Some(name) = ties.worktree_name {
            r.worktree_names.insert(c.id.clone(), name);
        }
        if let Some(before) = ties.worktrees_before {
            r.worktrees_before.insert(c.id.clone(), before);
        }
        if let Some(task) = ties.task {
            r.tasks.insert(c.id.clone(), task);
        }
        if !ties.worktrees_left.is_empty() {
            r.worktrees_left.insert(c.id.clone(), ties.worktrees_left);
        }
        if resumes && went_now {
            let opening = r.openings.get(&c.id).map(|o| o.text.clone());
            r.resumes.insert(c.id.clone(), ChatResume { opening, looks: 0 });
        }
        counts.push((c.id, count, c.transcript));
    }
    for h in saved.sends {
        r.held.push(Held { caller: h.caller, id: h.id, text: h.text, since: now_ms(), told: false, going_in: h.going_in });
    }
    // The window may be part way through one yet, and reach this core with it.
    for mut s in saved.spawns {
        s.left_by_core = true;
        r.next_request = r.next_request.max(s.request.saturating_add(1));
        r.abandoned.insert(s.request, (s.caller.clone(), s.in_worktree.is_some()));
        r.spawns.insert(s.request, s);
    }
    drop(r);
    for (id, count, transcript) in counts {
        crate::activity::keep_count(&id, count);
        if let Some(path) = transcript {
            transcripts().lock().unwrap_or_else(|e| e.into_inner()).insert(id, path);
        }
    }
}

/// How long this core has been up.
pub(super) fn core_up_ms() -> u64 {
    static UP: OnceLock<u64> = OnceLock::new();
    now_ms().saturating_sub(*UP.get_or_init(now_ms))
}

/// Whether a session the last core knew may yet be started by the window.
pub(super) fn returning(k: &Known, up_ms: u64) -> bool {
    k.waiting && up_ms < SESSION_RETURN_MS
}

/// A spawn the last core left that had come to a session it saved, and that had not
/// got as far as `owe_from`: that session answers its caller as any other, so owes
/// it now. With no turn of it ended that turn went with the core, as `take_up_chats`
/// has it for a chat's debt the count had not passed, and it may yet go on by itself.
/// With one ended, it is the answer. Once for each spawn (`owed_placed`): under a later
/// core the debt may have been told and settled, and the helper be long done. Once
/// both registries are read.
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
        let chat = !crate::background::has(&id);
        let ended = crate::activity::turns_counted(&id).unwrap_or(0);
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = r.spawns.get_mut(&request) {
            s.owed_placed = true;
        }
        let key = (caller, id.clone());
        if r.owed.contains_key(&key) {
            continue;
        }
        let went = chat && ended == 0;
        r.owed.insert(key.clone(), Owed { cut_short: went, ..Owed::from(0, 0) });
        r.seen.entry(key).or_insert(0);
        if !went {
            continue;
        }
        if let Some(opening) = r.openings.get_mut(&id) {
            opening.floor = opening.floor.max(1);
        }
        if r.known.get(&id).is_some_and(|k| k.settings["resumeOnRestart"] == true) && !r.resumes.contains_key(&id) {
            let opening = r.openings.get(&id).map(|o| o.text.clone());
            r.resumes.insert(id.clone(), ChatResume { opening, looks: 0 });
        }
        drop(r);
        crate::activity::keep_count(&id, 1);
    }
}

/// The conversation a visible helper's CLI was on as a core went, where nothing has
/// said so: it is told to the window by polling, every few seconds, and a core can go
/// first. A conversation is on disk or it is not, and that is what decides whether it
/// comes back to it, not whether the row has heard. Its session id as the CLI knows it.
pub fn conversation_of(id: &str, provider: Provider, folder: &str, account_path: &str) -> Option<String> {
    let path = conversation_on_disk(id, provider, folder, account_path)?;
    let file = path.file_name()?.to_string_lossy().into_owned();
    let session = match provider {
        Provider::Claude => file.strip_suffix(".jsonl")?.to_string(),
        Provider::Codex => crate::codex::session::parse_name(&file)?.1,
    };
    crate::log::warn("agents", &format!("{} goes back to its conversation {session}, which the window had not heard of", label(id)));
    transcripts().lock().unwrap_or_else(|e| e.into_inner()).insert(id.to_string(), path);
    Some(session)
}

/// The transcript of a conversation a helper whose first turn went with a core began:
/// the one the last core knew of if it holds anything, else one begun since it was asked
/// that holds what it was asked, in where it was to work, and no other session's.
pub(super) fn conversation_on_disk(id: &str, provider: Provider, folder: &str, account_path: &str) -> Option<std::path::PathBuf> {
    let (text, at_ms, name) = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let o = r.openings.get(id).filter(|o| o.floor >= 1)?;
        (o.text.clone(), o.at_ms, r.worktree_names.get(id).cloned())
    };
    let (kept, others) = {
        let known = transcripts().lock().unwrap_or_else(|e| e.into_inner());
        let others: Vec<std::path::PathBuf> = known.iter().filter(|(k, _)| k.as_str() != id).map(|(_, p)| p.clone()).collect();
        (known.get(id).cloned(), others)
    };
    if let Some(path) = kept.filter(|p| p.is_file() && !messages_at(p, provider, 0).is_empty()) {
        return Some(path);
    }
    let found = match provider {
        Provider::Claude => {
            let mut cwds = vec![folder.to_string()];
            cwds.extend(name.map(|name| crate::worktree::named_path(folder, &name)));
            crate::claude::session::transcripts_since(account_path, &cwds, at_ms)
        }
        Provider::Codex => crate::codex::session::rollouts_since(account_path, folder, at_ms),
    };
    let needle: String = words(&text).chars().take(NEEDLE).collect();
    found.into_iter().filter(|p| !others.contains(p)).find(|p| said_to(&messages_at(p, provider, 0), &needle) == WentIn::Yes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::delivery::{deliver_held, is_held};
    use crate::agents::owed::{last_shown, owe_from};
    use crate::agents::registry::{gone_from_the_window, parent_of, reg, register};
    use crate::agents::said::{first_words, TASK_CHARS};
    use crate::agents::spawn::{spawn_is, SpawnParams, SpawnTaken, SpawnUnderWay};
    use crate::agents::teardown::forget;
    use crate::agents::tests::{cut_short_chat, spawn_taken, spawns_to_myself, take_up};
    use crate::agents::{notice_line, owed_stands, Settled, CALLER_RETURN_MS};

    /// What a helper was spawned on is kept with it, not with `agents.jsonl`,
    /// which is cut with the helper still there.
    #[test]
    fn a_helpers_task_is_one_line_and_comes_back_with_it() {
        assert_eq!(first_words("\n# Fix the **build**\n\nThen the rest.", TASK_CHARS).as_deref(), Some("Fix the build"));
        let long = "word ".repeat(40);
        let cut = first_words(&long, TASK_CHARS).unwrap();
        assert!(cut.ends_with('…') && cut.chars().count() <= TASK_CHARS + 1, "{cut}");
        let none = serde_json::json!({});
        // A background helper, through helpers.json.
        register("tk-h", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("tk-c"), false, none.clone());
        reg().lock().unwrap().tasks.insert("tk-h".into(), first_words("Review the diff\nin detail", TASK_CHARS).unwrap());
        let saved: Ties = serde_json::from_slice(&serde_json::to_vec(&ties_of("tk-h").unwrap()).unwrap()).unwrap();
        assert_eq!(saved.task.as_deref(), Some("Review the diff"));
        forget("tk-h");
        assert!(ties_of("tk-h").is_none() && !reg().lock().unwrap().tasks.contains_key("tk-h"), "it goes with the helper");
        tie_back("tk-h", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", saved);
        assert_eq!(reg().lock().unwrap().tasks["tk-h"], "Review the diff");
        forget("tk-h");
        // A visible one, through chat-ties.json.
        register("tk-v", Provider::Codex, "C:\\p", "C:\\acc\\openai\\one", Some("tk-c"), false, none);
        reg().lock().unwrap().tasks.insert("tk-v".into(), "Run the lane".into());
        let ties = ties_of("tk-v").unwrap();
        let chat = SavedChat {
            id: "tk-v".into(),
            provider: Provider::Codex,
            folder: "C:\\p".into(),
            account_path: "C:\\acc\\openai\\one".into(),
            turns_ended: 0,
            transcript: None,
            mid_turn: false,
            ties,
        };
        let saved: SavedChats =
            serde_json::from_slice(&serde_json::to_vec(&SavedChats { chats: vec![chat], ..Default::default() }).unwrap()).unwrap();
        forget("tk-v");
        take_up_chats(saved);
        assert_eq!(reg().lock().unwrap().tasks["tk-v"], "Run the lane");
        forget("tk-v");
        // A file from before there were tasks reads as none.
        let older: Ties = serde_json::from_value(serde_json::json!({ "parent": null, "name": null, "settings": {}, "tools": false, "borrowed": false, "autoDelete": false, "owed": [], "seen": [] })).unwrap();
        assert_eq!(older.task, None);
    }

    /// A visible spawn the window answers past SPAWN_TIMEOUT never gets past its
    /// wait, and a core may go before it does: its task is there from the moment
    /// a session comes of it, in what the next core reads.
    #[test]
    fn a_spawn_has_its_task_as_soon_as_a_session_comes_of_it() {
        let _one = spawns_to_myself();
        let params: SpawnParams =
            serde_json::from_value(serde_json::json!({ "prompt": "# Late to the window\n\nThe rest.", "visible": true })).unwrap();
        let under_way = SpawnUnderWay::begin("tl-caller", &params, "C:\\acc\\anthropic\\one", None, None).unwrap();
        let request = under_way.0;
        register("tl-helper", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("tl-caller"), false, serde_json::json!({}));
        spawn_is(request, "tl-helper").unwrap();
        // Nothing past that has run: the spawn's own thread has long given up.
        let saved = chats_now();
        assert_eq!(saved.chats.iter().find(|c| c.id == "tl-helper").and_then(|c| c.ties.task.as_deref()), Some("Late to the window"));
        // Its own alone: the registry holds every other test's chats too.
        let chats: Vec<SavedChat> = saved.chats.into_iter().filter(|c| c.id == "tl-helper").collect();
        let saved: SavedChats = serde_json::from_slice(&serde_json::to_vec(&SavedChats { chats, ..Default::default() }).unwrap()).unwrap();
        forget("tl-helper");
        take_up_chats(saved);
        assert_eq!(reg().lock().unwrap().tasks["tl-helper"], "Late to the window", "across a core");
        drop(under_way);
        forget("tl-helper");
        // A spawn given up on leaves nothing behind.
        let under_way = SpawnUnderWay::begin("tl-caller", &params, "C:\\acc\\anthropic\\one", None, None).unwrap();
        reg().lock().unwrap().spawns.get_mut(&under_way.0).unwrap().settled = true;
        register("tl-late", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("tl-caller"), false, serde_json::json!({}));
        assert!(spawn_is(under_way.0, "tl-late").is_err());
        assert!(!reg().lock().unwrap().tasks.contains_key("tl-late"));
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
        let saved = SavedChats {
            chats: vec![SavedChat {
                id: "tc-chat".into(),
                provider: Provider::Codex,
                folder: "C:\\p".into(),
                // In the platform's own separators: the account's name is its last part.
                account_path: if cfg!(windows) { "C:\\acc\\openai\\two" } else { "/acc/openai/two" }.into(),
                turns_ended: 5,
                transcript: None,
                mid_turn: false,
                ties: Ties {
                    parent: Some("tc-parent".into()),
                    name: Some("the chat".into()),
                    settings: serde_json::json!({ "sandbox": "read-only" }),
                    tools: false,
                    borrowed: true,
                    auto_delete: true,
                    // One whose answer was in, one whose turn went with the core.
                    owed: vec![("tc-parent".into(), 4, 10), ("tc-peer".into(), 5, 20)],
                    seen: vec![("tc-parent".into(), 5)],
                    cut_short: Vec::new(),
                    orphan: false,
                    ..Default::default()
                },
            }],
            sends: vec![SavedSend { caller: "tc-parent".into(), id: "tc-chat".into(), text: "held across".into(), going_in: None }],
            spawns: Vec::new(),
        };
        // As the file has it.
        let saved: SavedChats = serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
        take_up_chats(saved);
        let key = |caller: &str| (caller.to_string(), "tc-chat".to_string());
        {
            let r = reg().lock().unwrap();
            let k = &r.known["tc-chat"];
            assert!(k.waiting && k.parent.as_deref() == Some("tc-parent") && k.account == "two");
            assert!(r.borrowed.contains("tc-chat") && r.auto.contains_key("tc-chat"));
            assert_eq!(r.names.get("tc-chat").map(String::as_str), Some("the chat"));
            assert!(!r.owed[&key("tc-parent")].cut_short, "its answer was in before the core went");
            assert!(r.owed[&key("tc-peer")].cut_short, "this one never will be");
            assert_eq!(r.seen.get(&key("tc-parent")), Some(&5));
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

        // Not back, it is not known for good to its callers either.
        let stands = |up: u64| owed_stands(&reg().lock().unwrap().known, "tc-chat", "someone", up);
        reg().lock().unwrap().known.entry("someone".into()).or_insert_with(|| k.clone());
        assert!(stands(0) && !stands(CALLER_RETURN_MS), "a waiting caller is given as long as an unknown one");
        reg().lock().unwrap().known.remove("someone");

        // The window has no such row: what was kept goes.
        {
            let r = reg().lock().unwrap();
            let names: HashMap<String, String> = [("other".to_string(), "x".to_string())].into_iter().collect();
            assert!(gone_from_the_window(&r, &names).contains(&"tc-chat".to_string()));
            assert!(gone_from_the_window(&r, &HashMap::new()).is_empty(), "a window with no list yet says nothing");
            let names: HashMap<String, String> = [("tc-chat".to_string(), "x".to_string())].into_iter().collect();
            assert!(!gone_from_the_window(&r, &names).contains(&"tc-chat".to_string()));
        }

        // The turn it counted as ended stays one that did not finish, core after core.
        let again = ties_of("tc-chat").unwrap();
        let again = SavedChat {
            id: "tc-second".into(),
            provider: Provider::Codex,
            folder: "C:\\p".into(),
            account_path: "C:\\acc\\openai\\two".into(),
            turns_ended: 9,
            transcript: None,
            mid_turn: false,
            ties: again,
        };
        take_up_chats(SavedChats { chats: vec![again], ..Default::default() });
        assert!(!reg().lock().unwrap().owed[&("tc-parent".to_string(), "tc-second".to_string())].cut_short, "this debt was answered");
        let cut: Ties = Ties { owed: vec![("tc-peer".into(), 5, 20)], cut_short: vec!["tc-peer".into()], ..Default::default() };
        let cut = SavedChat {
            id: "tc-third".into(),
            provider: Provider::Codex,
            folder: "C:\\p".into(),
            account_path: "C:\\acc\\openai\\two".into(),
            turns_ended: 6,
            transcript: None,
            mid_turn: false,
            ties: cut,
        };
        take_up_chats(SavedChats { chats: vec![cut], ..Default::default() });
        assert!(
            reg().lock().unwrap().owed[&("tc-peer".to_string(), "tc-third".to_string())].cut_short,
            "6 > 5 reads as answered but for the mark"
        );
        forget("tc-second");
        forget("tc-third");

        // Started again, it is any chat, and what it keeps stays for the next core.
        register("tc-chat", Provider::Codex, "C:\\p", "C:\\acc\\openai\\two", Some("tc-parent"), false, serde_json::json!({}));
        assert!(!reg().lock().unwrap().known["tc-chat"].waiting);
        let now = chats_now();
        let c = now.chats.iter().find(|c| c.id == "tc-chat").expect("a chat with ties is saved");
        assert_eq!(c.turns_ended, 6);
        assert!(c.ties.borrowed && c.ties.auto_delete && c.ties.owed.len() == 2);
        assert!(now.sends.iter().any(|h| h.id == "tc-chat"));
        reg().lock().unwrap().held.retain(|h| h.id != "tc-chat");
        forget("tc-chat");
        crate::activity::forget("tc-chat");
    }

    #[test]
    fn a_chat_with_nothing_to_keep_or_a_helper_is_not_saved_as_one() {
        let none = serde_json::json!({});
        register("sv-plain", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, false, none.clone());
        register(
            "sv-bg",
            Provider::Claude,
            "C:\\p",
            "C:\\acc\\anthropic\\one",
            Some("sv-plain"),
            false,
            serde_json::json!({ "background": true }),
        );
        register("sv-child", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("sv-plain"), false, none);
        let ids: Vec<String> = chats_now().chats.into_iter().map(|c| c.id).filter(|id| id.starts_with("sv-")).collect();
        assert_eq!(ids, ["sv-child"], "helpers.json has the background one");
        for id in ["sv-plain", "sv-bg", "sv-child"] {
            forget(id);
        }
    }

    #[test]
    fn the_core_keeps_the_one_record_of_a_chats_parent_across_a_restart() {
        // Passed up while the window was not there to hear, and saved so.
        let saved = |id: &str, parent: Option<&str>, orphan: bool| SavedChat {
            id: id.into(),
            provider: Provider::Claude,
            folder: "C:\\p".into(),
            account_path: "C:\\acc\\anthropic\\one".into(),
            turns_ended: 0,
            transcript: None,
            mid_turn: false,
            ties: Ties { parent: parent.map(str::to_owned), orphan, ..Default::default() },
        };
        take_up_chats(SavedChats {
            chats: vec![saved("rp-kid", Some("rp-grand"), false), saved("rp-orphan", None, true)],
            ..Default::default()
        });
        // The window starts each under the parent it last heard of.
        register("rp-kid", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("rp-gone"), false, serde_json::json!({}));
        assert_eq!(parent_of("rp-kid").as_deref(), Some("rp-grand"), "the core's record stands");
        register("rp-orphan", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("rp-gone"), false, serde_json::json!({}));
        assert_eq!(parent_of("rp-orphan"), None, "an orphan stays one");
        // With no record, as of a chat with nothing kept, the window's word is the one.
        register("rp-new", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("rp-parent"), false, serde_json::json!({}));
        assert_eq!(parent_of("rp-new").as_deref(), Some("rp-parent"));
        // A chat's orphan mark is itself kept for the next core.
        assert!(chats_now().chats.iter().any(|c| c.id == "rp-orphan" && c.ties.orphan));
        for id in ["rp-kid", "rp-orphan", "rp-new"] {
            forget(id);
        }
    }

    #[test]
    fn text_held_across_a_core_waits_for_the_resume_and_a_saved_opening_comes_back() {
        let mut chat = cut_short_chat("ho-chat", "ho-parent", true, true, false);
        chat.turns_ended = 0;
        chat.ties.owed = vec![("ho-parent".into(), 0, 0)];
        chat.ties.opening = Some("do it".into());
        take_up(
            vec![chat],
            vec![SavedSend { caller: "ho-parent".into(), id: "ho-chat".into(), text: "and this".into(), going_in: None }],
            Vec::new(),
        );
        assert_eq!(reg().lock().unwrap().resumes["ho-chat"].opening.as_deref(), Some("do it"));
        assert_eq!(crate::activity::turns_counted("ho-chat"), Some(1), "its turn counts as gone with the core");
        let saved = || chats_now().chats.into_iter().find(|c| c.id == "ho-chat").unwrap().ties.opening;
        assert_eq!(saved().as_deref(), Some("do it"), "that count is no first turn: a second restart still has the prompt");
        crate::activity::keep_count("ho-chat", 2);
        assert_eq!(saved(), None, "one that ended under this core is");
        assert!(is_held("ho-parent", "ho-chat"));
        // `deliver_held` looks and leaves it (`resume_pending`), the session not
        // running here and so the same in either case while it may yet return.
        deliver_held(crate::pty::manager());
        assert!(is_held("ho-parent", "ho-chat"));
        reg().lock().unwrap().held.retain(|h| h.id != "ho-chat");
        forget("ho-chat");
        assert!(!reg().lock().unwrap().openings.contains_key("ho-chat"));
    }

    #[test]
    fn a_helper_a_later_core_finds_done_is_neither_owed_again_nor_carried_on() {
        let _one = spawns_to_myself();
        register("fd-caller", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, serde_json::json!({}));
        // The same spawn as a third core finds it: under the second the helper went on,
        // finished, and its caller was told; the first left it no debt.
        let mut done = cut_short_chat("fd-done", "fd-caller", true, false, false);
        done.turns_ended = 2;
        done.ties.owed.clear();
        let placed = SpawnTaken { owed_placed: true, ..spawn_taken(9_000_600, "fd-caller", Some("fd-done"), true) };
        // Under the first core, with a turn ended before `owe_from`: that is its answer.
        let mut answered = cut_short_chat("fd-ans", "fd-caller", true, false, false);
        answered.turns_ended = 1;
        answered.ties.owed.clear();
        let first = spawn_taken(9_000_601, "fd-caller", Some("fd-ans"), true);
        take_up(vec![done, answered], Vec::new(), vec![placed, first]);
        take_up_spawns();
        {
            let r = reg().lock().unwrap();
            assert!(!r.owed.contains_key(&("fd-caller".to_string(), "fd-done".to_string())), "told already: no false did NOT finish");
            assert!(!r.resumes.contains_key("fd-done"), "and no carry on");
            let o = r.owed[&("fd-caller".to_string(), "fd-ans".to_string())];
            assert!(!o.cut_short, "its turn ended: it finished");
            assert!(!r.resumes.contains_key("fd-ans"));
            assert!(r.spawns[&9_000_601].owed_placed, "and not again under the next");
        }
        forget("fd-done");
        forget("fd-ans");
        forget("fd-caller");
    }
}
