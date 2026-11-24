//! The registry every agent tool works on: one `Registry` behind one lock
//! (`reg`), and what it says of a session — who it is (`Known`), what the window
//! calls it, which token is its, whose it is and whom it may reach.

use crate::agents::handover::taken_over_error;
use crate::agents::held::Held;
use crate::agents::notifier::lines::{NoticeTurn, Woken};
use crate::agents::notifier::usage::QuotaTold;
use crate::agents::owed::{Owed, Receipt, Requests};
use crate::agents::spawn::{PlacesRefused, SpawnTaken};
use crate::agents::teardown::{forget, Auto};
use crate::provider::Provider;
use crate::throttle::now_ms;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

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
    /// The person's per-chat Codex trust choice, kept out of MCP listings.
    pub(super) codex_trust: Option<luna_window::dto::ProjectTrust>,
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
    pub(super) next_request: u64,
    /// Spawns taken on and not yet seen through, by request id.
    pub(super) spawns: HashMap<u64, SpawnTaken>,
    /// What each helper was spawned to do, one line of it (`first_words`), by id.
    /// Kept in its ties: `agents.jsonl` is cut, and a helper can outlive its line.
    pub(super) tasks: HashMap<String, String>,
    /// Helpers the person took over as their own chats, by id, with the parent
    /// each was taken from (handover.rs): what that parent's calls are refused with.
    pub(super) taken_over: HashMap<String, String>,
    /// Orders parent changes so a late write to the background record cannot
    /// replace a newer parent (`background::set_parent`).
    pub(super) parent_changes: u64,
    /// How many times each session's ownership has changed hands, counted once
    /// what was queued for it has been taken back out (handover.rs): a send that
    /// put text in before that owes nothing of the person's chat after it
    /// (owed.rs `owe_from_at`).
    pub(super) handovers: HashMap<String, u64>,
    /// How many times each session has changed hands, counted in the hold that
    /// does it (handover.rs `change_hands`): what a keyed call's kept result is
    /// kept by beside the caller's id (`key_generation`), so a CLI started
    /// again keeps its keys and a hand-over drops them.
    pub(super) hands_changed: HashMap<String, u64>,
    /// Past which turn count a helper's answer to a caller's text lies, the text
    /// having missed the turn it joined (`retried`): above any count the caller holds.
    pub(super) delivery_floors: HashMap<(String, String), u32>,
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
    /// Each caller's requests to each session, by (caller, session) (owed.rs
    /// `Requests`): which one an answer is to.
    pub(super) requests: HashMap<(String, String), Requests>,
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
    /// The last number taken from each sender of that usage (`account_usage`).
    pub(super) usage_seqs: HashMap<String, u64>,
    /// When each caller last raised a desktop notification (`notify`).
    pub(super) notified: HashMap<String, u64>,
    /// The tools a Claude Code session's hooks say are up for an answer
    /// (`AskUserQuestion`, or one held for permission), oldest first, each
    /// with its input and who asks, the session or a subagent of it:
    /// subagents at work side by side may each hold one.
    pub(super) hook_asks: HashMap<String, Vec<(String, serde_json::Value, crate::agents::ask::AskSource)>>,
    /// Text sent to the person's chats where they were typing or had a draft
    /// Luna could not put aside, in the order it came: typed in once the way is
    /// clear (see `deliver_held`).
    pub(super) held: Vec<Held>,
    /// Notices a caller's draft holds back, by caller, for the chat row.
    pub(super) notices_held: HashMap<String, usize>,
    /// The turns Luna's notices started, by the session they went into.
    pub(super) notice_turns: HashMap<String, Vec<NoticeTurn>>,
    /// Leads Luna has told their delegates finished (lines.rs `wake_lead`), by id.
    pub(super) woken: HashMap<String, Woken>,
    /// What each lead was last seen waiting on (lines.rs `note_delegates`):
    /// what its wake names.
    pub(super) delegates: HashMap<String, Vec<String>>,
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
    /// Sessions refused a place on a line since its top chat was last told, by
    /// that chat (spawn/ `refused_a_place`): a line that rides with its notices.
    pub(super) places_refused: HashMap<String, PlacesRefused>,
}

pub(super) fn reg() -> &'static Mutex<Registry> {
    static R: OnceLock<Mutex<Registry>> = OnceLock::new();
    let r = R.get_or_init(|| {
        Mutex::new(Registry {
            tokens: HashMap::new(),
            known: HashMap::new(),
            // Above any request a core gone since may have left on disk.
            next_request: now_ms(),
            spawns: HashMap::new(),
            tasks: HashMap::new(),
            taken_over: HashMap::new(),
            parent_changes: 0,
            handovers: HashMap::new(),
            hands_changed: HashMap::new(),
            delivery_floors: HashMap::new(),
            seen: HashMap::new(),
            names: HashMap::new(),
            groups: HashMap::new(),
            answered: HashMap::new(),
            owed: HashMap::new(),
            requests: HashMap::new(),
            waits: HashMap::new(),
            reserved: HashMap::new(),
            receipts: HashMap::new(),
            auto: HashMap::new(),
            usage: HashMap::new(),
            usage_seqs: HashMap::new(),
            notified: HashMap::new(),
            hook_asks: HashMap::new(),
            held: Vec::new(),
            notices_held: HashMap::new(),
            notice_turns: HashMap::new(),
            woken: HashMap::new(),
            delegates: HashMap::new(),
            notice_times: HashMap::new(),
            stopped: HashMap::new(),
            exits: HashMap::new(),
            quota_told: HashMap::new(),
            news: Vec::new(),
            places_refused: HashMap::new(),
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
/// with none the one given (a helper being made), never for an orphan.
fn parent_by_record(record: Option<&Known>, given: Option<&str>) -> Option<String> {
    let orphan = record.is_some_and(|k| k.orphan);
    record.and_then(|k| k.parent.clone()).or_else(|| given.filter(|_| !orphan).map(str::to_owned))
}

/// A session is starting. Returns the bearer token for its MCP config when it
/// gets tools. Called at every start of its CLI, so a session going on is
/// registered again with the same lineage and a fresh token. `parent` is the
/// spawning session's for a helper being made; a chat the window starts is the
/// person's own and has none.
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
    // Whose it is since it last ran, if anything has changed that (`forget`).
    let orphan = previous.as_ref().is_some_and(|k| k.orphan);
    let parent = parent_by_record(previous.as_ref(), parent);
    // The window's settings are the flags it starts a CLI with: this one is the
    // core's, kept across every start.
    let mut settings = without_retired(settings);
    let grant = settings.as_object_mut().and_then(|s| s.remove(CODEX_TRUST));
    let codex_trust =
        grant.filter(|_| provider == Provider::Codex && parent.is_none()).and_then(|grant| serde_json::from_value(grant).ok());
    if let Some(k) = &previous {
        // And what a Codex helper taken over still carries (handover.rs).
        for key in CORE_SETTINGS.into_iter().chain([crate::agents::handover::STARTED_WITH]) {
            if let Some(v) = k.settings.get(key) {
                settings[key] = v.clone();
            }
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
            codex_trust,
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
/// in the test build (`luna_test`; scripts/test-build.ps1 `-DevCaller`), and
/// there only when `LUNA_DEV_TOKEN` is set in the environment, with
/// `LUNA_DEV_FOLDER` as its project folder and `LUNA_DEV_ACCOUNT` as its
/// account directory; spawns it asks for come up as top-level chats, since it
/// has no row of its own. A release has none of this compiled in, so no
/// environment can give it a caller that stands in for the person.
pub fn register_dev_caller() {
    #[cfg(luna_test)]
    {
        let var = |name: &str| std::env::var(name).ok();
        let Some(dev) = dev_caller(var("LUNA_DEV_TOKEN"), var("LUNA_DEV_FOLDER"), var("LUNA_DEV_ACCOUNT")) else { return };
        let Ok(dev) = dev else {
            crate::log::warn("agents", "LUNA_DEV_TOKEN needs LUNA_DEV_FOLDER and LUNA_DEV_ACCOUNT too; dev caller off");
            return;
        };
        register(DEV_CALLER, dev.provider, &dev.folder, &dev.account_path, None, false, serde_json::json!({}));
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        r.tokens.insert(token_key(&dev.token), DEV_CALLER.to_string());
        let url = crate::hub::mcp_url().unwrap_or_default();
        crate::log::warn("agents", &format!("dev caller registered for {} ({}); MCP at {url}", dev.folder, dev.account_path));
    }
}

/// The dev caller the test build's environment names.
#[cfg(luna_test)]
#[derive(Debug, PartialEq)]
struct DevCaller {
    token: String,
    folder: String,
    account_path: String,
    provider: Provider,
}

/// The dev caller `token`, `folder` and `account_path` (the `LUNA_DEV_*`
/// variables) name: None with no token, Err with a token and not the rest.
#[cfg(luna_test)]
fn dev_caller(token: Option<String>, folder: Option<String>, account_path: Option<String>) -> Option<Result<DevCaller, ()>> {
    let token = token?;
    let (folder, account_path) = (folder.unwrap_or_default(), account_path.unwrap_or_default());
    if token.trim().is_empty() || folder.is_empty() || account_path.is_empty() {
        return Some(Err(()));
    }
    let provider = if account_path.replace('\\', "/").contains("/openai/") { Provider::Codex } else { Provider::Claude };
    Some(Ok(DevCaller { token: token.trim().to_string(), folder, account_path, provider }))
}

/// A pty session's process has gone, with this exit code if it left one.
pub fn session_exited(id: &str, code: Option<u32>) {
    reg().lock().unwrap_or_else(|e| e.into_inner()).exits.insert(id.to_string(), code);
}

/// The id of the dev caller (see `register_dev_caller`).
pub const DEV_CALLER: &str = "dev";

/// Whether `id` is the dev caller, which stands in for the person: in the
/// test build alone. A release has no dev caller, and a session that is
/// given the id `dev` there is an ordinary one, with no reach beyond its
/// own. Every check of the dev caller goes through here.
pub fn is_dev(id: &str) -> bool {
    cfg!(luna_test) && id == DEV_CALLER
}

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

/// The first line of what a helper was spawned on (spawn/ `TASK_CHARS`), for
/// the window's list of a chat's helpers.
pub fn task_of(id: &str) -> Option<String> {
    reg().lock().unwrap_or_else(|e| e.into_inner()).tasks.get(id).cloned()
}

#[cfg(test)]
pub fn set_task_for_test(id: &str, task: &str) {
    reg().lock().unwrap_or_else(|e| e.into_inner()).tasks.insert(id.to_string(), task.to_string());
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

/// The caller an MCP request on this thread runs as, with the key of the token
/// it came with (`presenting`).
pub type Presented = (String, [u8; 32]);

thread_local! {
    static PRESENTED: std::cell::RefCell<Option<Presented>> = const { std::cell::RefCell::new(None) };
}

/// While held, what this thread decides of `caller`'s rights holds only as long
/// as `token` still names it (`stands`). The hub looks a token up before the
/// call runs and the call decides its rights later, each under a lock of its
/// own: a helper handed over to the person in between (its token revoked,
/// handover.rs `change_hands`) would otherwise run that call with a person's
/// chat's reach.
pub fn presenting(caller: &str, token: &str) -> Presenting {
    carry(Some((caller.to_string(), token_key(token))))
}

/// What this thread presents, for a thread it starts to carry on (`carry`).
pub fn presented() -> Option<Presented> {
    PRESENTED.with(|p| p.borrow().clone())
}

/// `presented` of another thread, held on this one.
pub fn carry(presented: Option<Presented>) -> Presenting {
    Presenting(PRESENTED.with(|p| p.replace(presented)))
}

/// What a thread presented before, put back when this goes.
pub struct Presenting(Option<Presented>);

impl Drop for Presenting {
    fn drop(&mut self) {
        let before = self.0.take();
        PRESENTED.with(|p| *p.borrow_mut() = before);
    }
}

/// What a call is told whose token no longer names its caller (`stands`).
pub(super) const TOKEN_GONE: &str = "your session's token is no longer valid: it was handed over or started again";

/// Which of its owners' a keyed call of `caller` is (mcp/dispatch.rs
/// `key_owner`): how many times it has changed hands. None for a call whose
/// token no longer names it (`stands`), which keeps and replays nothing.
pub fn key_generation(caller: &str) -> Option<u64> {
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    stands(&r, caller).then(|| r.hands_changed.get(caller).copied().unwrap_or(0))
}

/// Whether `id`'s rights stand for what this thread decides: always, but for
/// the caller of an MCP request whose token no longer names it. That one has
/// none, not even over its own helpers. Asked under the registry lock that the
/// right itself is decided under, so no hand-over lands between them.
pub(super) fn stands(r: &Registry, id: &str) -> bool {
    PRESENTED.with(|p| match &*p.borrow() {
        Some((caller, key)) if caller == id => r.tokens.get(key).is_some_and(|s| s == id),
        _ => true,
    })
}

/// Whether a session was given Luna's tools (the dev caller always has them).
pub fn has_tools(id: &str) -> bool {
    is_dev(id) || reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(id).is_some_and(|k| k.tools)
}

/// What a helper's row says of whose it is (background/rows.rs `Row`).
#[derive(Default)]
pub struct Ownership {
    /// The chat at the top of its line, if any.
    pub top: Option<String>,
    /// The person's chats that wait on it now without being above it.
    pub watched_by: Vec<String>,
}

pub fn ownership_of(id: &str) -> Ownership {
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let Some(k) = r.known.get(id) else { return Ownership::default() };
    let top = k.parent.as_ref().map(|_| top_of(&r, id)).filter(|t| persons_chat_in(&r, t) || !r.known.contains_key(t));
    let mut watched_by: Vec<String> = r
        .waits
        .keys()
        .filter(|(c, s)| s == id && !is_dev(c) && persons_chat_in(&r, c) && !is_descendant(&r, c, id))
        .map(|(c, _)| c.clone())
        .collect();
    watched_by.sort();
    watched_by.dedup();
    Ownership { top, watched_by }
}

pub fn parent_of(id: &str) -> Option<String> {
    reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(id)?.parent.clone()
}

pub fn settings_of(id: &str) -> serde_json::Value {
    reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(id).map(|k| k.settings.clone()).unwrap_or_default()
}

pub(crate) const CODEX_TRUST: &str = "codexTrust";

/// The core's launch snapshot can reuse the grant for that chat and start.
pub(crate) fn codex_chat_trust_key(id: &str, account_path: &str, folder: &str) -> Option<String> {
    let grant = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let k = r.known.get(id)?;
        (k.provider == Provider::Codex && k.parent.is_none() && k.account_path == account_path && k.folder == folder)
            .then(|| k.codex_trust.clone())
            .flatten()?
    };
    crate::codex::trust::granted_key(&grant, folder)
}

/// What a helper's settings hold that its CLI is not started with: `register` keeps
/// them across every start, and ties across cores. `resumeOnRestart`,
/// `connectors` (`Spec::connectors`) and
/// `constraints` (agents/spawn `Constraint`), which a spawn of its own passes on.
const CORE_SETTINGS: [&str; 3] = ["resumeOnRestart", "connectors", "constraints"];

/// Retired handoff and context thresholds are dropped as session settings
/// come in (`register`, persist.rs `take_up_chats`).
const RETIRED_SETTINGS: [&str; 5] = ["handoff", "budgetPct", "budgetTokens", "budgetBreach", "budgetTold"];

/// `settings` without `RETIRED_SETTINGS`.
pub(super) fn without_retired(mut settings: serde_json::Value) -> serde_json::Value {
    if let Some(o) = settings.as_object_mut() {
        for key in RETIRED_SETTINGS {
            o.remove(key);
        }
    }
    settings
}

/// Whether a helper was spawned with `resumeOnRestart`.
pub fn resumes_on_restart(id: &str) -> bool {
    reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(id).is_some_and(|k| k.settings["resumeOnRestart"] == true)
}

/// Whether `id` has helpers of its own still to answer it (`except` aside): it
/// is not through with what it was asked until it has their answers.
/// For each session, how many helpers under it still owe it an answer
/// (`waits_on_helpers`, counted): what keeps a chat whose turn is over in
/// the background in the window.
pub fn helpers_owing() -> HashMap<String, u32> {
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let mut out: HashMap<String, u32> = HashMap::new();
    for (caller, helper) in r.owed.keys() {
        if is_descendant(&r, caller, helper) {
            *out.entry(caller.clone()).or_default() += 1;
        }
    }
    out
}

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
    if !stands(r, of) {
        return false;
    }
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
    if crate::background::promoting(target) {
        return Err(format!("the person is opening {target} as a chat of their own; it is theirs once it opens"));
    }
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    if r.taken_over.get(target).is_some_and(|from| from == caller) {
        return Err(taken_over_error(target));
    }
    if !is_descendant(&r, caller, target) {
        return Err(format!("session {target} is not one you spawned"));
    }
    r.known.get(target).cloned().ok_or_else(|| format!("unknown session {target}"))
}

/// Reading a session and waiting on it reach any session but the caller's
/// own for a person's chat: the transcript is on disk for anyone
/// `luna_session_list` names it to, and a caller that typed into a session has
/// to be able to collect the answer. A helper reaches only what is under it
/// (`persons_chat_in`).
pub(super) fn authorize_view(caller: &str, target: &str) -> Result<Known, String> {
    if caller == target {
        return Err("that is your own session".into());
    }
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let known = r.known.get(target).cloned().ok_or_else(|| format!("unknown session {target}; luna_session_list lists them"))?;
    beneath(&r, caller, target, "read or wait on")?;
    Ok(known)
}

/// Whether `id` is a person's chat, or the dev caller standing in for the
/// person: the only callers whose reach goes past the sessions under them
/// (`scope: "all"`), and that may notify the person or arm the PC's power. A
/// helper (one with a parent, or run in the background), an orphan and a
/// session Luna does not know reach only their own: a prompt injected into a
/// helper deep in a tree stays in that tree.
pub(super) fn persons_chat_in(r: &Registry, id: &str) -> bool {
    if !stands(r, id) {
        return false;
    }
    is_dev(id) || r.known.get(id).is_some_and(|k| k.parent.is_none() && !k.orphan && k.settings["background"] != true)
}

/// `persons_chat_in`, for the tools.
pub fn is_persons_chat(id: &str) -> bool {
    persons_chat_in(&reg().lock().unwrap_or_else(|e| e.into_inner()), id)
}

/// What a helper hears when it reaches past the sessions under it.
fn not_under_you(id: &str, verb: &str) -> String {
    format!(
        "session {id} is not under you: a helper can {verb} only its own helpers and theirs. What is for anyone else goes \
         in your reply to the session that spawned you"
    )
}

/// A helper's reach: `target` is under it. A person's chat goes by its scope
/// (`in_scope`), checked before this.
fn beneath(r: &Registry, caller: &str, target: &str, verb: &str) -> Result<(), String> {
    if persons_chat_in(r, caller) || is_descendant(r, caller, target) {
        return Ok(());
    }
    // One the person took from it says so, as `in_scope` does.
    if r.taken_over.get(target).is_some_and(|from| from == caller) {
        return Err(taken_over_error(target));
    }
    Err(not_under_you(target, verb))
}

/// Whether `id` is the caller's: spawned by it, directly or through a helper.
pub(super) fn is_mine(caller: &str, id: &str) -> bool {
    is_descendant(&reg().lock().unwrap_or_else(|e| e.into_inner()), caller, id)
}

/// Whether a call's `scope` says `all`: past the caller's own (`mine`, the
/// default). Only a person's chat may say it (`persons_chat_in`).
pub fn scope_all(caller: &str, scope: Option<&str>) -> Result<bool, String> {
    scope_all_in(&reg().lock().unwrap_or_else(|e| e.into_inner()), caller, scope)
}

pub(super) fn scope_all_in(r: &Registry, caller: &str, scope: Option<&str>) -> Result<bool, String> {
    match scope {
        None | Some("mine") => Ok(false),
        Some("all") if persons_chat_in(r, caller) => Ok(true),
        Some("all") => Err(SCOPE_ALL_REFUSED.into()),
        Some(other) => Err(format!("unknown scope {other:?}; use mine (the default) or all")),
    }
}

/// A helper's `scope: "all"`, refused in every tool.
pub(super) const SCOPE_ALL_REFUSED: &str =
    "scope \"all\" is for a person's chats: a helper reaches only the sessions under it (its own helpers and theirs). Leave scope out";

/// Whether `id` is the caller or one of its own: what `scope` `mine` reaches.
pub fn own_or_self(caller: &str, id: &str) -> bool {
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    is_self(&r, caller, id) || is_descendant(&r, caller, id)
}

/// Whether `id` is the caller itself, as far as what it may see of itself
/// goes: not for a call whose token no longer names it (`stands`), whose
/// session is a person's chat by now, not the helper that made the call.
pub(super) fn is_self(r: &Registry, caller: &str, id: &str) -> bool {
    caller == id && stands(r, caller)
}

/// Whether this core knows session `id`: whose it is then goes by its owner now.
pub fn is_known(id: &str) -> bool {
    reg().lock().unwrap_or_else(|e| e.into_inner()).known.contains_key(id)
}

/// What a read, a wait or a send may reach, by its `scope`: the caller's own
/// helpers and theirs, unless it says `all`. Text read off or typed into a
/// session no one asked it to touch is the reach a prompt injection wants, so
/// going past its own takes an argument that says so, and only a person's chat
/// may say it (`scope_all_in`). A write (`WRITE_VERBS`) into a helper goes only
/// from above it, `all` or not (`write_reach`). An unknown session, or the
/// caller itself, is left to the tool's own refusal, which says more.
pub fn in_scope(caller: &str, id: &str, scope: Option<&str>, verb: &str) -> Result<(), String> {
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let all = scope_all_in(&r, caller, scope)?;
    if is_self(&r, caller, id) || !r.known.contains_key(id) || is_descendant(&r, caller, id) {
        return Ok(());
    }
    let writes = WRITE_VERBS.contains(&verb);
    if writes {
        if let Err(why) = write_reach(&r, caller, id) {
            return Err(write_refusal(&r, &why, id, verb));
        }
    }
    if all {
        return Ok(());
    }
    if r.taken_over.get(id).is_some_and(|from| from == caller) {
        return Err(taken_over_error(id));
    }
    if !persons_chat_in(&r, caller) {
        return Err(not_under_you(id, verb));
    }
    // Past the write check, a write here is into another of the person's chats.
    let past = if writes && persons_chat_in(&r, id) {
        "another of the user's chats"
    } else {
        "a session that is not yours (the user's own chats, another's helpers)"
    };
    Err(format!("session {id} is not one you spawned; to {verb} {past} pass scope: \"all\""))
}

/// The verbs `in_scope` is asked for that write into a session: what
/// `luna_session_send` and `luna_session_answer` pass it.
const WRITE_VERBS: [&str; 2] = ["send to", "answer"];

/// Why a write into a session (text sent, an answer) is refused (`write_reach`).
#[derive(Debug, PartialEq)]
pub(super) enum WriteRefused {
    /// A person's chat writing into a helper not under it: a helper takes text
    /// and answers only from the sessions above it (ADR-0028).
    NotAbove,
    /// A helper (or an orphan, or one run in the background) writing past the
    /// sessions under it.
    NotUnderYou,
    /// A helper the person took from the caller as a chat of their own.
    TakenOver,
}

/// Whether `caller` may write into `target`: send it text or answer what it
/// asks (ADR-0028). Into a person's chat a person's chat may (R1), whatever the
/// target was before becoming a chat, as
/// may the sessions above a helper into it (its parent, a lead between, the
/// chat at the top), and the dev caller, which stands in for the person,
/// anywhere. Nobody else: not a sibling, not another chat or its helpers, and
/// nobody into a helper with no chat above it. One check for every place a
/// write is decided: the tool's gate (`in_scope`), the write itself
/// (`authorize_send`), and wherever ownership can change under a write and it
/// is asked again.
pub(super) fn write_reach(r: &Registry, caller: &str, target: &str) -> Result<(), WriteRefused> {
    if is_dev(caller) || is_descendant(r, caller, target) {
        return Ok(());
    }
    let chat = persons_chat_in(r, caller);
    if chat && persons_chat_in(r, target) {
        return Ok(());
    }
    if r.taken_over.get(target).is_some_and(|from| from == caller) {
        return Err(WriteRefused::TakenOver);
    }
    if !chat {
        return Err(WriteRefused::NotUnderYou);
    }
    Err(WriteRefused::NotAbove)
}

/// What a caller is told of a refused write into `id`; `verb` as `in_scope`'s.
pub(super) fn write_refusal(r: &Registry, why: &WriteRefused, id: &str, verb: &str) -> String {
    match why {
        WriteRefused::NotUnderYou => not_under_you(id, verb),
        WriteRefused::TakenOver => taken_over_error(id),
        WriteRefused::NotAbove => not_above(r, id),
    }
}

/// A session as a refusal names it: by its name in the window, if it has
/// one, with its id.
pub(super) fn named(r: &Registry, id: &str) -> String {
    match r.names.get(id).map(|n| n.trim()).filter(|n| !n.is_empty()) {
        Some(n) => format!("\"{n}\" ({id})"),
        None => id.to_string(),
    }
}

/// Names the helper's owner and the reads and waits still open to this chat.
fn not_above(r: &Registry, id: &str) -> String {
    let whose = match r.known.get(id).and_then(|k| k.parent.clone()) {
        None => "has no chat above it, so no session sends to it or answers it".to_string(),
        Some(parent) => {
            let top = top_of(r, id);
            let of = if parent == top {
                format!("chat {}", named(r, &top))
            } else {
                format!("{}, under chat {}", named(r, &parent), named(r, &top))
            };
            format!("is a helper of {of}: only the sessions above it send to it or answer it")
        }
    };
    format!("session {id} {whose}. You may still read it and wait on it, with scope: \"all\".")
}

/// Text from a person's chat may go to the user's other chats — that is how
/// one session passes a message to another — and into a helper only from the
/// sessions above it (`write_reach`); never back into the caller's own
/// composer. A helper's goes only to the sessions under it.
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
    let known = r.known.get(target).cloned().ok_or_else(|| format!("unknown session {target}; luna_session_list lists them"))?;
    if let Err(why) = write_reach(&r, caller, target) {
        return Err(write_refusal(&r, &why, target, "send to or answer"));
    }
    // Not counted here against its line's places: the turn this text starts,
    // if any, is, as it is claimed (background/places.rs), and waits for a
    // place there with its text queued.
    // Text on its way starts the idle count over now, under the lock `reap`
    // decides under: it cannot be taken for deletion while this is delivered.
    if let Some(a) = r.auto.get_mut(target) {
        a.idle_since = None;
    }
    Ok(known)
}

/// Every chat's name and window group, sent whenever one changes: what
/// `luna_session_list` calls the user's own chats and where it puts them, which
/// the core otherwise knows only by id.
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
        assert!(authorize_send("a", "x").is_ok(), "a person's chat's text may go to the user's own chats");
        assert!(authorize_send("a", "a").is_err(), "but not into its own composer");
        assert!(authorize_send("a", "nobody").is_err());
        assert!(authorize_view("a", "x").is_ok(), "a session written to can be read and waited on");
        assert!(authorize_view("a", "a").is_err(), "but not one's own");
        // A helper reaches only what is under it: not upwards, not sideways.
        assert!(authorize_send("b", "c").is_ok() && authorize_view("b", "c").is_ok(), "its own helper");
        for target in ["a", "x"] {
            assert!(authorize_send("b", target).err().unwrap().contains("not under you"), "{target}");
            assert!(authorize_view("b", target).err().unwrap().contains("not under you"), "{target}");
        }
        let r = reg().lock().unwrap();
        assert_eq!(depth_of(&r, "c"), 2);
        let mut d = descendants(&r, "a");
        d.sort();
        assert_eq!(d, vec!["b", "c"]);
    }

    #[test]
    fn reaching_past_ones_own_takes_scope_all() {
        let (account, none) = ("C:\\acc\\anthropic\\one", serde_json::json!({}));
        register("sc-me", Provider::Claude, "C:\\p", account, None, true, none.clone());
        register("sc-kid", Provider::Claude, "C:\\p", account, Some("sc-me"), true, none.clone());
        register("sc-grand", Provider::Codex, "C:\\p", "C:\\acc\\openai\\two", Some("sc-kid"), false, none.clone());
        register("sc-chat", Provider::Claude, "C:\\p", account, None, false, none.clone());
        register("sc-theirs", Provider::Codex, "C:\\p", "C:\\acc\\openai\\two", Some("sc-chat"), false, none);
        // Its own, by default or said.
        assert_eq!(in_scope("sc-me", "sc-kid", None, "read"), Ok(()));
        assert_eq!(in_scope("sc-me", "sc-grand", Some("mine"), "read"), Ok(()));
        // From a person's chat, another chat and another chat's helper are read
        // and waited on only when told.
        for id in ["sc-chat", "sc-theirs"] {
            for verb in ["read", "wait on"] {
                let e = in_scope("sc-me", id, None, verb).unwrap_err();
                assert!(e.contains("not one you spawned") && e.contains("scope: \"all\""), "{id}: {e}");
                assert!(in_scope("sc-me", id, Some("mine"), verb).is_err());
                assert_eq!(in_scope("sc-me", id, Some("all"), verb), Ok(()));
            }
        }
        // Another chat is written to when told; another chat's helper never, told
        // or not: only the sessions above it write into it.
        for verb in ["send to", "answer"] {
            let e = in_scope("sc-me", "sc-chat", None, verb).unwrap_err();
            assert!(e.contains("another of the user's chats") && e.contains("scope: \"all\""), "{e}");
            assert_eq!(in_scope("sc-me", "sc-chat", Some("all"), verb), Ok(()));
            for scope in [None, Some("mine"), Some("all")] {
                let e = in_scope("sc-me", "sc-theirs", scope, verb).unwrap_err();
                assert!(e.contains("only the sessions above it") && e.contains("chat sc-chat"), "{verb}: {e}");
            }
        }
        // A helper: its own, and nothing past it, told or not, in every verb.
        assert_eq!(in_scope("sc-kid", "sc-grand", None, "read"), Ok(()));
        for id in ["sc-me", "sc-chat", "sc-theirs"] {
            for verb in ["read", "wait on", "send to", "answer"] {
                let e = in_scope("sc-kid", id, None, verb).unwrap_err();
                assert!(e.contains("not under you") && !e.contains("scope: \"all\""), "{id}: {e}");
                assert_eq!(in_scope("sc-kid", id, Some("all"), verb).unwrap_err(), SCOPE_ALL_REFUSED);
            }
        }
        assert_eq!(scope_all("sc-kid", Some("all")).unwrap_err(), SCOPE_ALL_REFUSED, "nor even of its own");
        assert_eq!(scope_all("sc-me", Some("all")), Ok(true));
        assert_eq!(scope_all(DEV_CALLER, Some("all")), Ok(true), "the dev caller stands in for the person");
        // A session run in the background with no parent left is no person's chat.
        register("sc-alone", Provider::Claude, "C:\\p", account, None, true, serde_json::json!({ "background": true }));
        assert!(!is_persons_chat("sc-alone") && is_persons_chat("sc-me") && !is_persons_chat("sc-kid"));
        assert!(!is_persons_chat("sc-nobody"), "nor is one Luna does not know");
        forget("sc-alone");
        // An unknown session and itself are the tool's to refuse, in its words.
        assert_eq!(in_scope("sc-me", "sc-nobody", None, "read"), Ok(()));
        assert_eq!(in_scope("sc-me", "sc-me", None, "read"), Ok(()));
        assert!(in_scope("sc-me", "sc-kid", Some("everything"), "read").unwrap_err().contains("unknown scope"));
        assert!(own_or_self("sc-me", "sc-me") && own_or_self("sc-me", "sc-grand") && !own_or_self("sc-me", "sc-theirs"));
        for id in ["sc-grand", "sc-kid", "sc-theirs", "sc-chat", "sc-me"] {
            forget(id);
        }
    }

    /// Writes into a helper (ADR-0028): only the sessions above it, by `in_scope`
    /// and `authorize_send` alike; reads and waits as before.
    #[test]
    fn writes_into_a_helper_go_only_from_above_it() {
        let (account, none) = ("C:/acc/anthropic/one", serde_json::json!({}));
        let background = serde_json::json!({ "background": true });
        register("wr-top", Provider::Claude, "C:/p", account, None, true, none.clone());
        register("wr-lead", Provider::Claude, "C:/p", account, Some("wr-top"), true, background.clone());
        register("wr-grand", Provider::Codex, "C:/p", "C:/acc/openai/two", Some("wr-lead"), false, background.clone());
        register("wr-sib", Provider::Claude, "C:/p", account, Some("wr-top"), false, background.clone());
        register("wr-other", Provider::Claude, "C:/p", account, None, true, none.clone());
        register("wr-theirs", Provider::Claude, "C:/p", account, Some("wr-other"), true, background.clone());
        // No chat above either: one whose chat was deleted, one run on its own.
        register("wr-orphan", Provider::Claude, "C:/p", account, None, false, background.clone());
        reg().lock().unwrap().known.get_mut("wr-orphan").unwrap().orphan = true;
        register("wr-alone", Provider::Codex, "C:/p", "C:/acc/openai/two", None, false, background);
        set_name("wr-top", "Docs");
        set_name("wr-lead", "Release lead");
        // The tool's gate for both verbs, with scope "all" where the caller may
        // say it, then the write's own check.
        let writes = |caller: &str, id: &str| -> Vec<Result<(), String>> {
            let scope = is_persons_chat(caller).then_some("all");
            let gate = ["send to", "answer"].map(|verb| in_scope(caller, id, scope, verb));
            gate.into_iter().chain([authorize_send(caller, id).map(|_| ())]).collect()
        };
        let allowed = |caller: &str, id: &str| {
            let all = writes(caller, id);
            assert!(all.iter().all(Result::is_ok), "{caller} -> {id}: {all:?}");
        };
        let refused = |caller: &str, id: &str| -> Vec<String> {
            writes(caller, id).into_iter().map(|w| w.err().unwrap_or_else(|| panic!("{caller} -> {id}"))).collect()
        };
        // The parent, a lead between and the chat at the top.
        allowed("wr-lead", "wr-grand");
        allowed("wr-top", "wr-grand");
        allowed("wr-top", "wr-lead");
        // A person's chat to another of the person's chats, either way (R1).
        allowed("wr-other", "wr-top");
        allowed("wr-top", "wr-other");
        // Another chat, into this one's helpers: named whose they are, with what
        // is open to it.
        for id in ["wr-lead", "wr-grand", "wr-sib"] {
            for e in refused("wr-other", id) {
                assert!(e.contains("only the sessions above it") && e.contains("\"Docs\" (wr-top)"), "{id}: {e}");
                assert!(e.contains("read it and wait on it, with scope: \"all\""), "{e}");
            }
            for verb in ["read", "wait on"] {
                assert_eq!(in_scope("wr-other", id, Some("all"), verb), Ok(()), "{id}: {verb}");
            }
            assert!(authorize_view("wr-other", id).is_ok(), "{id}");
        }
        let e = authorize_send("wr-other", "wr-grand").err().unwrap();
        assert!(e.contains("helper of \"Release lead\" (wr-lead), under chat \"Docs\" (wr-top)"), "{e}");
        // A helper cannot write into a sibling or anything else outside its line.
        for (caller, id) in [("wr-sib", "wr-lead"), ("wr-sib", "wr-grand"), ("wr-lead", "wr-sib"), ("wr-theirs", "wr-grand")] {
            for e in refused(caller, id) {
                assert!(e.contains("not under you"), "{caller} -> {id}: {e}");
            }
            assert!(authorize_send(caller, id).err().unwrap().contains("not under you"));
        }
        // A helper with no chat above it takes no agent's writes.
        for id in ["wr-orphan", "wr-alone"] {
            for caller in ["wr-top", "wr-other"] {
                for e in refused(caller, id) {
                    assert!(e.contains("has no chat above it"), "{caller} -> {id}: {e}");
                }
                assert_eq!(in_scope(caller, id, Some("all"), "read"), Ok(()));
            }
            for caller in ["wr-lead", "wr-sib", "wr-theirs"] {
                assert!(authorize_send(caller, id).err().unwrap().contains("not under you"), "{caller} -> {id}");
            }
            assert_eq!(write_reach(&reg().lock().unwrap(), "wr-top", id), Err(WriteRefused::NotAbove));
        }
        // The dev caller stands in for the person, as the live suite needs.
        assert!(authorize_send(DEV_CALLER, "wr-grand").is_ok() && authorize_send(DEV_CALLER, "wr-orphan").is_ok());
        for id in ["wr-grand", "wr-lead", "wr-sib", "wr-theirs", "wr-orphan", "wr-alone", "wr-other", "wr-top"] {
            forget(id);
        }
    }

    #[test]
    fn a_line_left_with_no_chat_hangs_from_no_chat() {
        let (chat, helper, child) = ("nt-chat", "nt-helper", "nt-child");
        let account = "C:/acc/anthropic/one";
        let background = serde_json::json!({ "background": true });
        register(chat, Provider::Claude, "C:/p", account, None, true, serde_json::json!({}));
        register(helper, Provider::Claude, "C:/p", account, Some(chat), true, background.clone());
        register(child, Provider::Claude, "C:/p", account, Some(helper), false, background);
        assert_eq!(ownership_of(child).top.as_deref(), Some(chat));
        forget(chat);
        assert_eq!(ownership_of(helper).top, None);
        assert_eq!(ownership_of(child).top, None);
        forget(child);
        forget(helper);
    }

    #[test]
    fn a_helper_is_told_of_the_parent_it_passes_to() {
        let (account, none) = ("C:\\acc\\anthropic\\one", serde_json::json!({}));
        let background = serde_json::json!({ "background": true });
        register("tp-grand", Provider::Claude, "C:\\p", account, None, true, none.clone());
        register("tp-parent", Provider::Claude, "C:\\p", account, Some("tp-grand"), true, background.clone());
        register("tp-kid", Provider::Claude, "C:\\p", account, Some("tp-parent"), false, background);
        crate::background::take_up_for_test("tp-kid", "C:\\p", 1, &[], None);
        crate::background::set_parent_for_test("tp-kid", Some("tp-parent".into()));

        forget("tp-parent");
        assert_eq!(crate::background::told_parent_for_test("tp-kid").as_deref(), Some("tp-grand"), "a background helper's next turn");
        // Registered again, as a core taking it up does, it keeps the record's.
        register("tp-kid", Provider::Claude, "C:\\p", account, Some("tp-parent"), false, serde_json::json!({ "background": true }));
        assert_eq!(parent_of("tp-kid").as_deref(), Some("tp-grand"));

        forget("tp-grand");
        assert_eq!(crate::background::told_parent_for_test("tp-kid"), None, "nobody left to answer to");
        register("tp-kid", Provider::Claude, "C:\\p", account, Some("tp-parent"), false, serde_json::json!({ "background": true }));
        assert_eq!(parent_of("tp-kid"), None, "an orphan stays one");
        crate::background::remove("tp-kid");
        forget("tp-kid");
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
    fn retired_budget_state_does_not_survive_a_restart() {
        let settings = serde_json::json!({ "model": "m", "budgetTokens": 7, "budgetBreach": { "at": 1 }, "budgetTold": true });
        register("bs-kid", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("bs-p"), false, settings);
        // The window starts its CLI again, on flags that say nothing of it.
        register("bs-kid", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("bs-p"), false, serde_json::json!({ "model": "m2" }));
        let s = settings_of("bs-kid");
        assert_eq!(s["model"], "m2");
        for key in ["budgetTokens", "budgetBreach", "budgetTold"] {
            assert!(s.get(key).is_none());
        }
        forget("bs-kid");
    }

    /// The dev caller is the test build's alone: a release has no code that
    /// would register it, whatever its environment holds. In the test build
    /// it takes a token, a folder and an account, and its provider is the
    /// account's.
    #[test]
    fn only_the_test_build_has_a_dev_caller() {
        // `luna_test` is set exactly for the test flavor (build.rs), which a
        // release is never built as.
        assert_eq!(cfg!(luna_test), env!("LUNA_FLAVOR") == "test");
        // A session named `dev` is the dev caller in the test build alone.
        assert_eq!(is_dev(DEV_CALLER), cfg!(luna_test));
        assert!(!is_dev("dev ") && !is_dev("Dev") && !is_dev("cmw8yfdg51"));
        #[cfg(luna_test)]
        {
            let some = |s: &str| Some(s.to_string());
            assert_eq!(dev_caller(None, some(r"C:\p"), some(r"C:\acc")), None);
            assert_eq!(dev_caller(some(" "), some(r"C:\p"), some(r"C:\acc")), Some(Err(())));
            assert_eq!(dev_caller(some("t0k3n"), None, some(r"C:\acc")), Some(Err(())));
            assert_eq!(dev_caller(some("t0k3n"), some(r"C:\p"), None), Some(Err(())));
            let codex = dev_caller(some(" t0k3n "), some(r"C:\p"), some(r"C:\tb\accounts\openai\dev")).unwrap().unwrap();
            assert_eq!((codex.token.as_str(), codex.provider), ("t0k3n", Provider::Codex));
            let claude = dev_caller(some("t0k3n"), some("/p"), some("/tb/accounts/anthropic/dev")).unwrap().unwrap();
            assert_eq!(claude.provider, Provider::Claude);
        }
    }

    /// Every check of the dev caller asks `is_dev`, so what it stands in for
    /// is the test build's alone: no comparison with its id anywhere else.
    /// Not on Windows: it reads the checkout as it runs, and the Windows test
    /// executables run away from the container they were built in.
    #[cfg(not(windows))]
    #[test]
    fn the_dev_caller_is_asked_for_only_through_is_dev() {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push(p);
                }
            }
        }
        let mut files = Vec::new();
        walk(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
        assert!(files.len() > 50, "the walk found the sources");
        // Made here, so that this test's own text holds none of them.
        let comparisons: Vec<String> =
            ["==", "!="].iter().flat_map(|op| [format!("{op} {}", "DEV_CALLER"), format!("{} {op}", "DEV_CALLER")]).collect();
        for f in files {
            let text = std::fs::read_to_string(&f).unwrap();
            for (n, line) in text.lines().enumerate() {
                let compared = comparisons.iter().any(|c| line.contains(c.as_str()));
                let here = f.ends_with("registry.rs") && line.trim() == format!("cfg!(luna_test) && id {} DEV_CALLER", "==");
                assert!(!compared || here, "{}:{}: compares with DEV_CALLER outside is_dev", f.display(), n + 1);
            }
        }
    }
}
