//! The registry every agent tool works on: one `Registry` behind one lock
//! (`reg`), and what it says of a session — who it is (`Known`), what the window
//! calls it, which token is its, whose it is and whom it may reach.

use crate::agents::delivery::Held;
use crate::agents::owed::{Owed, Receipt};
use crate::agents::resume::ChatResume;
use crate::agents::teardown::{forget, Auto, Reparented};
use crate::agents::{NoticeTurn, Opening, Pending, QuotaTold, SpawnTaken, CHATS_FROZEN};
use crate::provider::Provider;
use crate::throttle::now_ms;
use std::collections::HashMap;
use std::sync::{Condvar, Mutex, OnceLock};

/// A session the agent layer knows: every one started with tools, and every
/// one an agent spawned.
#[derive(Clone)]
pub(super) struct Known {
    pub(super) provider: Provider,
    pub(super) folder: String,
    pub(super) account_path: String,
    pub(super) account: String,
    pub(super) parent: Option<String>,
    /// What the session was started with, as the CLI's flags spell it:
    /// model, effort and permission mode (Claude) or approval and sandbox
    /// (Codex). Echoed by `list` and `spawn`.
    pub(super) settings: serde_json::Value,
    /// Luna's tools were given to it: `tools/list` offers them.
    pub(super) tools: bool,
    /// Known from what the last core saved (`restore_chats`): its CLI has not
    /// started under this one yet.
    pub(super) waiting: bool,
    /// Spawned by a chat since deleted, with no parent left to hand it to (see
    /// `forget`): no agent reaches it, only the window.
    pub(super) orphan: bool,
}

/// The chat at the top of `id`'s lineage, whose sessions the cap counts.
pub(super) fn top_of(r: &Registry, id: &str) -> String {
    let mut cur = id.to_string();
    for _ in 0..16 {
        match r.known.get(&cur).and_then(|k| k.parent.clone()) {
            Some(p) => cur = p,
            None => break,
        }
    }
    cur
}

pub(super) struct Registry {
    /// Sessions by the SHA-256 of their bearer token (`token_key`).
    pub(super) tokens: HashMap<[u8; 32], String>,
    pub(super) known: HashMap<String, Known>,
    pub(super) pending: HashMap<u64, std::sync::Arc<(Mutex<Pending>, Condvar)>>,
    /// Spawns whose caller stopped waiting (SPAWN_TIMEOUT), or which the last
    /// core left, by request id: the caller to hand the chat to should the
    /// frontend still make it.
    pub(super) abandoned: HashMap<u64, (String, bool)>,
    pub(super) next_request: u64,
    /// Spawns taken on and not yet seen through, by request id.
    pub(super) spawns: HashMap<u64, SpawnTaken>,
    /// Visible helpers that are to go on by themselves, by id.
    pub(super) resumes: HashMap<String, ChatResume>,
    /// What each visible helper was first asked, by id.
    pub(super) openings: HashMap<String, Opening>,
    /// What each helper was spawned to do, one line of it (`first_words`), by id.
    /// Kept in its ties: `agents.jsonl` is cut, and a helper can outlive its line.
    pub(super) tasks: HashMap<String, String>,
    /// What Luna named the worktree of a visible Claude Code helper, by id: the one
    /// it knows to be that helper's, what a fresh start goes back into if it is still
    /// as made (`worktree_for_start`). A background helper's is in its `Spec`.
    pub(super) worktree_names: HashMap<String, String>,
    /// A Codex helper to make a worktree of its own, by id, until its CLI has said
    /// where: the worktrees there were as it was spawned. Codex takes no name, so one
    /// new since is all that tells what a core may have cut it off making.
    pub(super) worktrees_before: HashMap<String, Vec<String>>,
    /// What a helper began and is not going on in, by id, until its parent has heard
    /// (`worktrees_left_line`).
    pub(super) worktrees_left: HashMap<String, Vec<String>>,
    /// Past which turn count a helper's answer to a caller's text lies, the text
    /// having missed the turn it joined (`retried`): above any count the caller holds.
    pub(super) delivery_floors: HashMap<(String, String), u32>,
    /// Helpers that run in a worktree they were handed rather than made:
    /// deleting one never drops that worktree, whose work is its caller's.
    pub(super) borrowed: std::collections::HashSet<String>,
    /// The `turnsEnded` each caller was last shown for each session, by
    /// (caller, session). `wait(turn_done)` without `afterTurn` waits for a
    /// turn past that — the one the caller has not seen — rather than past
    /// the count at the moment of the call, which a fast reply had already
    /// moved on: `spawn`, a two-second answer, `wait` → timeout.
    pub(super) seen: HashMap<(String, String), u32>,
    /// The chat rows' names by chat id, as the frontend last sent them. Every
    /// chat's, not only the known ones: a name may arrive before its session
    /// registers.
    pub(super) names: HashMap<String, String>,
    /// The window group each chat row sits in, 0-based, sent with the names:
    /// sessions the user put side by side in one group are the ones meant to
    /// work things out between them.
    pub(super) groups: HashMap<String, u32>,
    /// When each caller last typed into a session that was stopped on a
    /// prompt, by (caller, session): the answer to the prompt. For a few
    /// seconds after it the prompt may still look open, and `wait` must not
    /// hand it back as if it were a new one.
    pub(super) answered: HashMap<(String, String), u64>,
    /// Answers owed, by (caller, session): the caller spawned the session or
    /// typed into it, and has not been shown a turn past `baseline` since.
    /// The notifier tells the caller when the answer comes.
    pub(super) owed: HashMap<(String, String), Owed>,
    /// `wait` calls in flight, by (caller, session).
    pub(super) waits: HashMap<(String, String), u32>,
    /// Sessions a `wait_many` in flight will answer for, by (caller, session):
    /// their notices wait for it, however long the others take.
    pub(super) reserved: HashMap<(String, String), u32>,
    /// Answers handed over so far, by (caller, session), the latest being the
    /// only one a cancel may take back (see `hand_over`).
    pub(super) receipts: HashMap<(String, String), Receipt>,
    /// Helpers spawned with `autoDelete`, by id.
    pub(super) auto: HashMap<String, Auto>,
    /// Each account's usage as the window last showed it, by `provider/name`.
    pub(super) usage: HashMap<String, serde_json::Value>,
    /// When each caller last raised a desktop notification (`notify`).
    pub(super) notified: HashMap<String, u64>,
    /// The tools a Claude Code session's hooks say are up for an answer
    /// (`AskUserQuestion`, or one held for permission), oldest first, each
    /// with its input and the subagent that asks, if one does: subagents at
    /// work side by side may each hold one.
    pub(super) hook_asks: HashMap<String, Vec<(String, serde_json::Value, Option<String>)>>,
    /// Text sent to visible sessions where a person was typing or had a
    /// draft Luna could not put aside, in the order it came: the notifier
    /// types each in once the way is clear (see `deliver_held`).
    pub(super) held: Vec<Held>,
    /// Notices a caller's draft holds back, by caller, for the chat row.
    pub(super) notices_held: HashMap<String, usize>,
    /// The turns Luna's notices started, by the session they went into.
    pub(super) notice_turns: HashMap<String, Vec<NoticeTurn>>,
    /// When each turn-end notice went, by (caller, session), within
    /// BREAKER_WINDOW_MS.
    pub(super) notice_times: HashMap<(String, String), Vec<u64>>,
    /// The pairs Luna has stopped telling (BREAKER_NOTICES), and since when.
    pub(super) stopped: HashMap<(String, String), u64>,
    /// How each pty session last exited, by id, until it runs again.
    pub(super) exits: HashMap<String, Option<u32>>,
    /// How far each caller has been told a usage window has gone, by (caller,
    /// `provider/name`, the window's label).
    pub(super) quota_told: HashMap<(String, String, String), QuotaTold>,
    /// Lines for a caller's next notice that no debt brings: by caller.
    pub(super) news: Vec<(String, String)>,
}

pub(super) fn reg() -> &'static Mutex<Registry> {
    static R: OnceLock<Mutex<Registry>> = OnceLock::new();
    let r = R.get_or_init(|| {
        Mutex::new(Registry {
            tokens: HashMap::new(),
            known: HashMap::new(),
            pending: HashMap::new(),
            abandoned: HashMap::new(),
            borrowed: Default::default(),
            // A page can still answer a request of a core gone since, which is
            // not to land on one of this core's.
            next_request: now_ms(),
            spawns: HashMap::new(),
            resumes: HashMap::new(),
            openings: HashMap::new(),
            tasks: HashMap::new(),
            worktree_names: HashMap::new(),
            worktrees_before: HashMap::new(),
            worktrees_left: HashMap::new(),
            delivery_floors: HashMap::new(),
            seen: HashMap::new(),
            names: HashMap::new(),
            groups: HashMap::new(),
            answered: HashMap::new(),
            owed: HashMap::new(),
            waits: HashMap::new(),
            reserved: HashMap::new(),
            receipts: HashMap::new(),
            auto: HashMap::new(),
            usage: HashMap::new(),
            notified: HashMap::new(),
            hook_asks: HashMap::new(),
            held: Vec::new(),
            notices_held: HashMap::new(),
            notice_turns: HashMap::new(),
            notice_times: HashMap::new(),
            stopped: HashMap::new(),
            exits: HashMap::new(),
            quota_told: HashMap::new(),
            news: Vec::new(),
        })
    });
    // Tests assert with this held and all of them share it: one that fails is
    // that test's failure, not that of every test after it.
    #[cfg(test)]
    r.clear_poison();
    r
}

pub(super) fn account_name(account_path: &str) -> String {
    std::path::Path::new(account_path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Whose a session is: the core's record (`pass_up`, kept across cores), and only
/// with none the window's word, never for an orphan.
fn parent_by_record(record: Option<&Known>, window_says: Option<&str>) -> Option<String> {
    let orphan = record.is_some_and(|k| k.orphan);
    record.and_then(|k| k.parent.clone()).or_else(|| window_says.filter(|_| !orphan).map(str::to_owned))
}

/// Whom a session the window is starting is to be told it answers to.
pub fn parent_on_start(id: &str, window_says: Option<String>) -> Option<String> {
    parent_by_record(reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(id), window_says.as_deref())
}

/// A session is starting. Returns the bearer token for its MCP config when it
/// gets tools. Called at every spawn, so a resumed session is re-registered
/// with the same lineage and a fresh token.
pub fn register(
    id: &str,
    provider: Provider,
    folder: &str,
    account_path: &str,
    parent: Option<&str>,
    tools: bool,
    settings: serde_json::Value,
) -> Option<String> {
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let account = account_name(account_path);
    let previous = r.known.get(id).cloned();
    r.exits.remove(id);
    // Whose it is since it last ran, if anything has changed that (`forget`):
    // the window still names the parent it was spawned by.
    let orphan = previous.as_ref().is_some_and(|k| k.orphan);
    let window_says = parent;
    let parent = parent_by_record(previous.as_ref(), parent);
    // The window's settings are the flags it starts a CLI with: this one is the
    // core's, kept across every start.
    let mut settings = settings;
    if let Some(k) = &previous {
        for key in CORE_SETTINGS {
            if let Some(v) = k.settings.get(key) {
                settings[key] = v.clone();
            }
        }
    }
    // A session running again: what the chats' ties say is news once more.
    CHATS_FROZEN.store(false, std::sync::atomic::Ordering::SeqCst);
    // Kept across cores too (`restore_chats`), so a new parent the window never
    // heard of before its core went is said again, and this stays the one record.
    if let (Some(says), Some(is)) = (window_says, &parent) {
        if says != is {
            crate::emit::to_ui("agent://reparent", Reparented { id: id.to_string(), parent: is.clone() });
        }
    }
    r.known.insert(
        id.to_string(),
        Known {
            provider,
            folder: folder.to_string(),
            account_path: account_path.to_string(),
            account,
            parent,
            settings,
            tools,
            waiting: false,
            orphan,
        },
    );
    // One token per session: a respawn invalidates the last one.
    r.tokens.retain(|_, chat| chat != id);
    if !tools {
        return None;
    }
    let token = mint();
    r.tokens.insert(token_key(&token), id.to_string());
    Some(token)
}

/// A caller that is not a session: for driving the agent layer from outside
/// — a test against a build, `curl` against the MCP endpoint. Registered only
/// when `LUNA_DEV_TOKEN` is set in the environment, with `LUNA_DEV_FOLDER`
/// as its project folder and `LUNA_DEV_ACCOUNT` as its account directory;
/// spawns it asks for come up as top-level chats, since it has no row of its
/// own. Never set for a Luna a person uses.
pub fn register_dev_caller() {
    let Ok(token) = std::env::var("LUNA_DEV_TOKEN") else { return };
    let folder = std::env::var("LUNA_DEV_FOLDER").unwrap_or_default();
    let account_path = std::env::var("LUNA_DEV_ACCOUNT").unwrap_or_default();
    if token.trim().is_empty() || folder.is_empty() || account_path.is_empty() {
        crate::log::warn("agents", "LUNA_DEV_TOKEN needs LUNA_DEV_FOLDER and LUNA_DEV_ACCOUNT too; dev caller off");
        return;
    }
    let provider = if account_path.replace('\\', "/").contains("/openai/") { Provider::Codex } else { Provider::Claude };
    register(DEV_CALLER, provider, &folder, &account_path, None, false, serde_json::json!({}));
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    r.tokens.insert(token_key(token.trim()), DEV_CALLER.to_string());
    let url = crate::hub::mcp_url().unwrap_or_default();
    crate::log::warn("agents", &format!("dev caller registered for {folder} ({account_path}); MCP at {url}"));
}

/// A pty session's process has gone, with this exit code if it left one.
pub fn session_exited(id: &str, code: Option<u32>) {
    reg().lock().unwrap_or_else(|e| e.into_inner()).exits.insert(id.to_string(), code);
}

/// The id of the dev caller (see `register_dev_caller`).
pub const DEV_CALLER: &str = "dev";

/// The frontend's name for a chat — set when it spawns one, ahead of the next
/// `chat_names`.
pub fn set_name(id: &str, name: &str) {
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    r.names.insert(id.to_string(), name.to_string());
}

/// A chat's name as the user sees it, if the frontend has sent one.
pub fn chat_name(id: &str) -> Option<String> {
    reg().lock().unwrap_or_else(|e| e.into_inner()).names.get(id).cloned().filter(|n| !n.trim().is_empty())
}

/// A session as the log names it: the id with the chat's name, which is what
/// is left to go by once the session is deleted.
pub fn label(id: &str) -> String {
    match chat_name(id) {
        Some(name) => format!("{id} \"{name}\""),
        None => id.to_string(),
    }
}

/// A bearer token: 160 bits from the OS's CSPRNG, as 40 hex characters.
pub(crate) fn mint() -> String {
    crate::secret::random_hex(20)
}

/// What the registry keys a token by. A map lookup compares the presented
/// token against a stored one byte by byte and stops at the first difference;
/// over digests, how far a guess got says nothing about the token itself.
fn token_key(token: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(token.as_bytes()).into()
}

/// The session behind a bearer token.
pub fn caller_of(token: &str) -> Option<String> {
    reg().lock().unwrap_or_else(|e| e.into_inner()).tokens.get(&token_key(token)).cloned()
}

/// A token for a session registered without tools: a background Claude Code
/// helper takes its permission prompts through Luna's MCP server all the
/// same, and `tools/list` offers it that one tool only.
pub(super) fn issue_token(id: &str) -> String {
    let token = mint();
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    r.tokens.retain(|_, chat| chat != id);
    r.tokens.insert(token_key(&token), id.to_string());
    token
}

/// Whether a session was given Luna's tools (the dev caller always has them).
pub fn has_tools(id: &str) -> bool {
    id == DEV_CALLER || reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(id).is_some_and(|k| k.tools)
}

pub fn parent_of(id: &str) -> Option<String> {
    reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(id)?.parent.clone()
}

pub fn settings_of(id: &str) -> serde_json::Value {
    reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(id).map(|k| k.settings.clone()).unwrap_or_default()
}

/// A background helper has become a chat the window runs: no longer one
/// Luna deletes on its own, and the window registers it again when its
/// session starts.
pub fn promoted(id: &str) {
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    r.auto.remove(id);
    if let Some(k) = r.known.get_mut(id) {
        if let Some(o) = k.settings.as_object_mut() {
            o.remove("background");
        }
    }
}

/// What a session's settings hold that no window sets: `register` keeps them across
/// every start of its CLI, and ties across cores. `resumeOnRestart`, and a budget
/// with where it stands (`budget_news`).
const CORE_SETTINGS: [&str; 4] = ["resumeOnRestart", "budgetTokens", "budgetBreach", "budgetTold"];

/// Whether a helper was spawned with `resumeOnRestart`.
pub fn resumes_on_restart(id: &str) -> bool {
    reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(id).is_some_and(|k| k.settings["resumeOnRestart"] == true)
}

/// Whether `id` has helpers of its own still to answer it (`except` aside): it
/// is not through with what it was asked until it has their answers.
pub(super) fn waits_on_helpers(id: &str, except: &str) -> bool {
    waits_on_helpers_in(&reg().lock().unwrap_or_else(|e| e.into_inner()), id, except)
}

pub(super) fn waits_on_helpers_in(r: &Registry, id: &str, except: &str) -> bool {
    r.owed.keys().any(|(caller, helper)| caller == id && helper != except && is_descendant(r, id, helper))
}

pub(super) fn depth_of(r: &Registry, id: &str) -> usize {
    let mut d = 0;
    let mut cur = id.to_string();
    while let Some(p) = r.known.get(&cur).and_then(|k| k.parent.clone()) {
        d += 1;
        cur = p;
        if d > 16 {
            break;
        }
    }
    d
}

pub(super) fn is_descendant(r: &Registry, of: &str, id: &str) -> bool {
    let mut cur = id.to_string();
    for _ in 0..16 {
        match r.known.get(&cur).and_then(|k| k.parent.clone()) {
            Some(p) if p == of => return true,
            Some(p) => cur = p,
            None => return false,
        }
    }
    false
}

pub(super) fn descendants(r: &Registry, of: &str) -> Vec<String> {
    r.known.keys().filter(|id| is_descendant(r, of, id)).cloned().collect()
}

/// The target must be something the caller spawned, directly or through a
/// child. Anything else — the user's own chats, a sibling — is out of reach.
pub(super) fn authorize(caller: &str, target: &str) -> Result<Known, String> {
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    if !is_descendant(&r, caller, target) {
        return Err(format!("session {target} is not one you spawned"));
    }
    r.known.get(target).cloned().ok_or_else(|| format!("unknown session {target}"))
}

/// Reading a session and waiting on it reach any session but the caller's
/// own: the transcript is on disk for anyone `luna_sessions` names it to, and
/// a caller that typed into a session has to be able to collect the answer.
pub(super) fn authorize_view(caller: &str, target: &str) -> Result<Known, String> {
    if caller == target {
        return Err("that is your own session".into());
    }
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    r.known.get(target).cloned().ok_or_else(|| format!("unknown session {target}; luna_sessions lists them"))
}

/// Text may go to any session Luna runs, the user's own chats included —
/// that is how one session passes a message to another — but not back into
/// the caller's own composer.
pub(super) fn authorize_send(caller: &str, target: &str) -> Result<Known, String> {
    if caller == target {
        return Err("that is your own session".into());
    }
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    // `reap` has let the lock go to remove it: text taken now would start a
    // turn that goes with it, and a question nobody hears the end of.
    if r.auto.get(target).is_some_and(|a| a.deleting) {
        return Err(format!("{target} is being deleted (autoDelete, idle since its answer); spawn a helper again for more"));
    }
    let known = r.known.get(target).cloned().ok_or_else(|| format!("unknown session {target}; luna_sessions lists them"))?;
    // Text on its way starts the idle count over now, under the lock `reap`
    // decides under: it cannot be taken for deletion while this is delivered.
    if let Some(a) = r.auto.get_mut(target) {
        a.idle_since = None;
    }
    Ok(known)
}

/// Every chat's name and window group, sent whenever one changes: what
/// `luna_sessions` calls the user's own chats and where it puts them, which
/// the core otherwise knows only by id.
#[tauri::command]
pub fn chat_names(mut names: HashMap<String, String>, groups: Option<HashMap<String, u32>>) {
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    // A row deleted while no core ran told nobody.
    let gone = gone_from_the_window(&r, &names);
    // A background helper has no row to be named by: it keeps the name it
    // was spawned with (`set_name`) until the window runs it as a chat.
    for (id, name) in &r.names {
        if !names.contains_key(id) && r.known.get(id).is_some_and(|k| k.settings["background"] == true) {
            names.insert(id.clone(), name.clone());
        }
    }
    r.names = names;
    if let Some(groups) = groups {
        r.groups = groups;
    }
    drop(r);
    for id in gone {
        crate::log::info("agents", &format!("{id} has no row any more; letting go of what the last core kept of it"));
        forget(&id);
        crate::activity::forget(&id);
    }
}

/// The sessions kept from the last core (`Known::waiting`) that the window, now
/// it has said which chats it has, does not have.
pub(super) fn gone_from_the_window(r: &Registry, names: &HashMap<String, String>) -> Vec<String> {
    if names.is_empty() {
        return Vec::new();
    }
    r.known.iter().filter(|(id, k)| k.waiting && !names.contains_key(*id)).map(|(id, _)| id.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::teardown::forget;

    #[test]
    fn lineage_and_reach() {
        let none = serde_json::json!({});
        let a = register("a", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone()).expect("token");
        assert_eq!(caller_of(&a).as_deref(), Some("a"));
        register("b", Provider::Codex, "C:\\p", "C:\\acc\\openai\\two", Some("a"), false, none.clone());
        register("c", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("b"), false, none.clone());
        register("x", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, false, none);
        assert!(authorize("a", "b").is_ok());
        assert!(authorize("a", "c").is_ok(), "grandchildren are reachable");
        assert!(authorize("b", "a").is_err(), "not upwards");
        assert!(authorize("a", "x").is_err(), "not the user's own chats");
        assert!(authorize("a", "a").is_err(), "not itself");
        assert!(authorize_send("a", "x").is_ok(), "text may go to the user's own chats");
        assert!(authorize_send("b", "a").is_ok(), "and upwards");
        assert!(authorize_send("a", "a").is_err(), "but not into its own composer");
        assert!(authorize_send("a", "nobody").is_err());
        assert!(authorize_view("b", "a").is_ok(), "a session written to can be read and waited on");
        assert!(authorize_view("a", "a").is_err(), "but not one's own");
        let r = reg().lock().unwrap();
        assert_eq!(depth_of(&r, "c"), 2);
        let mut d = descendants(&r, "a");
        d.sort();
        assert_eq!(d, vec!["b", "c"]);
    }

    #[test]
    fn a_helper_is_told_of_the_parent_it_passes_to_and_a_chat_as_it_starts() {
        let (account, none) = ("C:\\acc\\anthropic\\one", serde_json::json!({}));
        let background = serde_json::json!({ "background": true });
        register("tp-grand", Provider::Claude, "C:\\p", account, None, true, none.clone());
        register("tp-parent", Provider::Claude, "C:\\p", account, Some("tp-grand"), true, none.clone());
        register("tp-kid", Provider::Claude, "C:\\p", account, Some("tp-parent"), false, background);
        crate::background::take_up_for_test("tp-kid", "C:\\p", 1, &[], None);
        crate::background::set_parent("tp-kid", Some("tp-parent".into()));
        register("tp-chat", Provider::Claude, "C:\\p", account, Some("tp-parent"), false, none);

        forget("tp-parent");
        assert_eq!(crate::background::told_parent_for_test("tp-kid").as_deref(), Some("tp-grand"), "a background helper's next turn");
        // A running chat keeps what it started with; its next start goes by the
        // record, whatever parent the window last heard of.
        assert_eq!(parent_on_start("tp-chat", Some("tp-parent".into())).as_deref(), Some("tp-grand"));
        assert_eq!(parent_on_start("tp-unknown", Some("tp-parent".into())).as_deref(), Some("tp-parent"), "no record: the window's word");

        forget("tp-grand");
        assert_eq!(crate::background::told_parent_for_test("tp-kid"), None, "nobody left to answer to");
        assert_eq!(parent_on_start("tp-chat", Some("tp-parent".into())), None, "an orphan, whatever its row still names");
        crate::background::remove("tp-kid");
        forget("tp-kid");
        forget("tp-chat");
    }

    #[test]
    fn a_respawn_replaces_the_token() {
        let none = serde_json::json!({});
        let t1 = register("r", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none.clone()).unwrap();
        let t2 = register("r", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", None, true, none).unwrap();
        assert_ne!(t1, t2);
        assert_eq!(caller_of(&t1), None);
        assert_eq!(caller_of(&t2).as_deref(), Some("r"));
    }

    #[test]
    fn where_a_budget_stands_outlives_every_start_of_its_cli() {
        let settings = serde_json::json!({ "model": "m", "budgetTokens": 7, "budgetBreach": { "at": 1 }, "budgetTold": true });
        register("bs-kid", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("bs-p"), false, settings);
        // The window starts its CLI again, on flags that say nothing of it.
        register("bs-kid", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("bs-p"), false, serde_json::json!({ "model": "m2" }));
        let s = settings_of("bs-kid");
        assert_eq!(
            (&s["model"], &s["budgetTokens"], &s["budgetTold"]),
            (&serde_json::json!("m2"), &serde_json::json!(7), &serde_json::json!(true))
        );
        assert_eq!(s["budgetBreach"]["at"], 1);
        forget("bs-kid");
    }
}
